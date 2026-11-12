//! The engraving treatment for book pictures (app-desktop.md revision 40, "Your
//! picture — the engraving treatment"): a photo becomes dark ink lines on paper, with
//! an optional transparent background, so the book's colour shows behind it.
//!
//! Everything here is exact integer arithmetic (the workspace denies floating point),
//! so the same input and settings always give the same bytes (acceptance test 226).
//! Lengths along the picture are in 1/256 pixel ("fp"); tones run from 0 (black) to
//! [`WHITE`].
//!
//! The steps, in order (MoneyKeep's own description, from the spec):
//! 1. crop to a square and scale to the working size ([`crop_square`]);
//! 2. the subject: a fill from the border through pixels close in colour to the
//!    border's median colour and without a strong edge, overridden by the Keep and
//!    Remove brushes; enclosed areas stay part of the subject; smoothed;
//! 3. tone: luminance stretched over the subject (2nd–98th percentile), local
//!    contrast (the tone minus a large blur), Contrast and Brightness, a light
//!    sharpening;
//! 4. hatching at 45°, −45° and 0° below about 92 %, 58 % and 30 % tone, lines
//!    growing with darkness and bending with the smoothed tone (Curve), with
//!    anti-aliased coverage;
//! 5. outline: strong tone edges by Outline, and a thin ink line along the
//!    silhouette when the background is removed;
//! 6. ink over paper, scaled down to the result size with the alpha.

use crate::image::Rgb;
use crate::{ImportError, ImportResult};

/// Working size of the stored treatment (the result is scaled down from it).
pub const WORK_SIDE: u32 = 512;
/// Side of the stored result (the cards' picture).
pub const RESULT_SIDE: u32 = 256;
/// Side of the brush mask.
pub const MASK_SIDE: u32 = 256;
/// Mask values: automatic, always subject (Keep), always background (Remove).
pub const MASK_AUTO: u8 = 0;
pub const MASK_KEEP: u8 = 1;
pub const MASK_REMOVE: u8 = 2;

/// The lightest tone.
const WHITE: i32 = 4095;
/// One pixel in fp units.
const PX: i64 = 256;

/// Ink colours (#16181B, #1B2340, #3B2A1E) and paper colours (white, #F6F1E4).
pub const INK_BLACK: [u8; 3] = [0x16, 0x18, 0x1B];
pub const INK_NAVY: [u8; 3] = [0x1B, 0x23, 0x40];
pub const INK_SEPIA: [u8; 3] = [0x3B, 0x2A, 0x1E];
pub const PAPER_WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];
pub const PAPER_CREAM: [u8; 3] = [0xF6, 0xF1, 0xE4];

/// The slider values and colours of a treatment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// 0–100: line spacing from about 9 px (0) to 3 px (100) at 512.
    pub detail: i32,
    /// 0–100: how thick lines grow in dark areas.
    pub line_weight: i32,
    /// −50–50.
    pub contrast: i32,
    /// −50–50.
    pub brightness: i32,
    /// 0–100: strength of drawn edges.
    pub outline: i32,
    /// 0–100: how much lines bend with the tones.
    pub curve: i32,
    /// 0–100: 0 keeps the background; higher removes more.
    pub background: i32,
    pub ink: [u8; 3],
    pub paper: [u8; 3],
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            detail: 50,
            line_weight: 50,
            contrast: 10,
            brightness: 0,
            outline: 40,
            curve: 30,
            background: 0,
            ink: INK_BLACK,
            paper: PAPER_WHITE,
        }
    }
}

impl Settings {
    /// The settings with every slider inside its range.
    pub fn clamped(&self) -> Settings {
        Settings {
            detail: self.detail.clamp(0, 100),
            line_weight: self.line_weight.clamp(0, 100),
            contrast: self.contrast.clamp(-50, 50),
            brightness: self.brightness.clamp(-50, 50),
            outline: self.outline.clamp(0, 100),
            curve: self.curve.clamp(0, 100),
            background: self.background.clamp(0, 100),
            ink: self.ink,
            paper: self.paper,
        }
    }
}

/// An 8-bit RGBA raster, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

fn bad(m: &str) -> ImportError {
    ImportError::Invalid(m.into())
}

fn to_usize(v: u32) -> usize {
    usize::try_from(v).unwrap_or(0)
}

fn to_u8(v: i64) -> u8 {
    u8::try_from(v.clamp(0, 255)).unwrap_or(0)
}

/// A square plane of integer values.
struct Plane {
    side: usize,
    v: Vec<i32>,
}

impl Plane {
    fn new(side: usize, fill: i32) -> Plane {
        Plane {
            side,
            v: vec![fill; side.saturating_mul(side)],
        }
    }

    /// The value at (x, y), coordinates clamped to the plane.
    fn at(&self, x: i64, y: i64) -> i32 {
        let max = i64::try_from(self.side).unwrap_or(1).saturating_sub(1);
        let (x, y) = (x.clamp(0, max), y.clamp(0, max));
        let i = usize::try_from(y * (max + 1) + x).unwrap_or(0);
        self.v.get(i).copied().unwrap_or(0)
    }

    /// Mean over the (2r+1)² window around each value (edges clamped), with prefix
    /// sums in each direction.
    fn blur(&self, r: usize) -> Plane {
        if r == 0 {
            return Plane {
                side: self.side,
                v: self.v.clone(),
            };
        }
        let horizontal = blur_lines(&self.v, self.side, r, false);
        Plane {
            side: self.side,
            v: blur_lines(&horizontal, self.side, r, true),
        }
    }
}

/// Box blur along rows (or columns), clamping the window to the line.
fn blur_lines(v: &[i32], side: usize, r: usize, columns: bool) -> Vec<i32> {
    let mut out = vec![0; v.len()];
    let mut prefix = vec![0i64; side + 1];
    for line in 0..side {
        let index = |k: usize| {
            if columns {
                k * side + line
            } else {
                line * side + k
            }
        };
        let mut acc = 0i64;
        for k in 0..side {
            acc += i64::from(v.get(index(k)).copied().unwrap_or(0));
            if let Some(p) = prefix.get_mut(k + 1) {
                *p = acc;
            }
        }
        for k in 0..side {
            let lo = k.saturating_sub(r);
            let hi = (k + r + 1).min(side);
            let sum = prefix.get(hi).copied().unwrap_or(0) - prefix.get(lo).copied().unwrap_or(0);
            let n = i64::try_from(hi - lo).unwrap_or(1).max(1);
            if let Some(o) = out.get_mut(index(k)) {
                *o = i32::try_from(sum / n).unwrap_or(0);
            }
        }
    }
    out
}

/// One output position along an axis, for bilinear sampling: the two source indices
/// and the weight of the second in 1/65536.
fn bilinear_axis(start: u32, size: u32, out: u32) -> Vec<(i64, i64, i64)> {
    let (start, size, out) = (i64::from(start), i64::from(size), i64::from(out.max(1)));
    (0..out)
        .map(|o| {
            // Centre of output pixel o in source coordinates, 16.16 fixed point.
            let pos = ((2 * o + 1) * size * 65536) / (2 * out) - 32768;
            let pos = pos.clamp(0, (size - 1).max(0) * 65536);
            let i0 = pos >> 16;
            let f = pos & 0xFFFF;
            let i1 = (i0 + 1).min(size - 1);
            (start + i0, start + i1, f)
        })
        .collect()
}

/// The square `size` × `size` at (`x`, `y`) of `img`, scaled to `out` × `out`:
/// an average of the covered pixels when shrinking, bilinear when enlarging.
pub fn crop_square(img: &Rgb, x: u32, y: u32, size: u32, out: u32) -> ImportResult<Rgb> {
    if size == 0 || out == 0 {
        return Err(bad("the crop is empty"));
    }
    let fits = |a: u32, extent: u32| a.checked_add(size).is_some_and(|end| end <= extent);
    if !fits(x, img.width) || !fits(y, img.height) {
        return Err(bad("the crop must lie inside the picture"));
    }
    let w = i64::from(img.width);
    let px = |sx: i64, sy: i64, c: usize| -> i64 {
        let i = usize::try_from((sy * w + sx) * 3).unwrap_or(0) + c;
        i64::from(img.data.get(i).copied().unwrap_or(0))
    };
    let mut data = Vec::with_capacity(to_usize(out).saturating_mul(to_usize(out)) * 3);
    if size >= out {
        let (s, o64) = (i64::from(size), i64::from(out));
        let span = |o: i64| {
            let a = o * s / o64;
            let b = ((o + 1) * s / o64).max(a + 1).min(s);
            (a, b)
        };
        for oy in 0..o64 {
            let (ya, yb) = span(oy);
            for ox in 0..o64 {
                let (xa, xb) = span(ox);
                let mut sum = [0i64; 3];
                for sy in ya..yb {
                    for sx in xa..xb {
                        for (c, acc) in sum.iter_mut().enumerate() {
                            *acc += px(sx + i64::from(x), sy + i64::from(y), c);
                        }
                    }
                }
                let n = ((yb - ya) * (xb - xa)).max(1);
                for acc in sum {
                    data.push(to_u8((acc + n / 2) / n));
                }
            }
        }
    } else {
        let xs = bilinear_axis(x, size, out);
        let ys = bilinear_axis(y, size, out);
        for &(y0, y1, fy) in &ys {
            for &(x0, x1, fx) in &xs {
                for c in 0..3 {
                    let top = px(x0, y0, c) * (65536 - fx) + px(x1, y0, c) * fx;
                    let bottom = px(x0, y1, c) * (65536 - fx) + px(x1, y1, c) * fx;
                    let v = (top * (65536 - fy) + bottom * fy + (1 << 31)) >> 32;
                    data.push(to_u8(v));
                }
            }
        }
    }
    Ok(Rgb {
        width: out,
        height: out,
        data,
    })
}

/// Luminance 0–255 of each pixel (Rec. 601 weights in 1/256).
fn luminance(img: &Rgb, side: usize) -> Plane {
    let mut p = Plane::new(side, 0);
    for (v, c) in p.v.iter_mut().zip(img.data.chunks_exact(3)) {
        if let [r, g, b] = c {
            *v = (77 * i32::from(*r) + 150 * i32::from(*g) + 29 * i32::from(*b)) >> 8;
        }
    }
    p
}

/// Sobel gradient magnitude (|gx| + |gy|) of a plane.
fn gradient(p: &Plane) -> Plane {
    let side = p.side;
    let mut out = Plane::new(side, 0);
    let s = i64::try_from(side).unwrap_or(0);
    for y in 0..s {
        for x in 0..s {
            let a = |dx: i64, dy: i64| i64::from(p.at(x + dx, y + dy));
            let gx = a(1, -1) + 2 * a(1, 0) + a(1, 1) - a(-1, -1) - 2 * a(-1, 0) - a(-1, 1);
            let gy = a(-1, 1) + 2 * a(0, 1) + a(1, 1) - a(-1, -1) - 2 * a(0, -1) - a(1, -1);
            if let Some(o) = out.v.get_mut(usize::try_from(y * s + x).unwrap_or(0)) {
                *o = i32::try_from(gx.abs() + gy.abs()).unwrap_or(i32::MAX);
            }
        }
    }
    out
}

/// The brush mark at working pixel (x, y) (the mask is [`MASK_SIDE`] square).
fn mark_at(mask: Option<&[u8]>, x: usize, y: usize, side: usize) -> u8 {
    let Some(m) = mask else {
        return MASK_AUTO;
    };
    let ms = to_usize(MASK_SIDE);
    let (mx, my) = (x * ms / side.max(1), y * ms / side.max(1));
    m.get(my * ms + mx).copied().unwrap_or(MASK_AUTO)
}

/// Median of a list of channel values.
fn median(mut v: Vec<u8>) -> i32 {
    v.sort_unstable();
    i32::from(v.get(v.len() / 2).copied().unwrap_or(0))
}

/// The subject: 1 for subject pixels, 0 for removed background.
fn subject(img: &Rgb, lum: &Plane, s: &Settings, mask: Option<&[u8]>) -> Vec<u8> {
    let side = lum.side;
    let n = side * side;
    let mut subj = vec![1u8; n];
    if s.background > 0 && side > 1 {
        let colour = |i: usize| -> [i32; 3] {
            let at = |c: usize| i32::from(img.data.get(i * 3 + c).copied().unwrap_or(0));
            [at(0), at(1), at(2)]
        };
        let border: Vec<usize> = (0..side)
            .flat_map(|k| [k, (side - 1) * side + k, k * side, k * side + side - 1])
            .collect();
        let channel = |c: usize| {
            median(
                border
                    .iter()
                    .map(|&i| to_u8(i64::from(colour(i).get(c).copied().unwrap_or(0))))
                    .collect(),
            )
        };
        let med = [channel(0), channel(1), channel(2)];
        let tolerance = s.background * 3;
        let edge_limit = 16 + s.background / 2;
        // Edges on lightly blurred luminance, scaled to about 0–255.
        let edges = gradient(&lum.blur(1));
        let passable = |i: usize, x: usize, y: usize| -> bool {
            match mark_at(mask, x, y, side) {
                MASK_KEEP => false,
                MASK_REMOVE => true,
                _ => {
                    let c = colour(i);
                    let d = (c[0] - med[0]).abs() + (c[1] - med[1]).abs() + (c[2] - med[2]).abs();
                    let e = edges.v.get(i).copied().unwrap_or(0) / 8;
                    d <= tolerance && e < edge_limit
                }
            }
        };
        let mut queue: Vec<usize> = Vec::new();
        for &i in &border {
            let (x, y) = (i % side, i / side);
            if subj.get(i) == Some(&1) && passable(i, x, y) {
                if let Some(v) = subj.get_mut(i) {
                    *v = 0;
                }
                queue.push(i);
            }
        }
        while let Some(i) = queue.pop() {
            let (x, y) = (i % side, i / side);
            let mut next = [None; 4];
            if x > 0 {
                next[0] = Some(i - 1);
            }
            if x + 1 < side {
                next[1] = Some(i + 1);
            }
            if y > 0 {
                next[2] = Some(i - side);
            }
            if y + 1 < side {
                next[3] = Some(i + side);
            }
            for j in next.into_iter().flatten() {
                if subj.get(j) == Some(&1) && passable(j, j % side, j / side) {
                    if let Some(v) = subj.get_mut(j) {
                        *v = 0;
                    }
                    queue.push(j);
                }
            }
        }
        // Smooth the border: a 3 × 3 majority, twice.
        for _ in 0..2 {
            let p = Plane {
                side,
                v: subj.iter().map(|&v| i32::from(v) * 9).collect(),
            };
            let b = p.blur(1);
            for (v, m) in subj.iter_mut().zip(&b.v) {
                *v = u8::from(*m >= 5);
            }
        }
    }
    // The brushes override everything.
    if mask.is_some() {
        for (i, v) in subj.iter_mut().enumerate() {
            match mark_at(mask, i % side, i / side, side) {
                MASK_KEEP => *v = 1,
                MASK_REMOVE => *v = 0,
                _ => {}
            }
        }
    }
    subj
}

/// The tone plane (0 black to [`WHITE`]) of the subject.
fn tone(lum: &Plane, subj: &[u8], s: &Settings) -> Plane {
    let side = lum.side;
    // Stretch over the subject's 2nd–98th percentile.
    let mut hist = [0u32; 256];
    let mut count = 0u32;
    for (v, m) in lum.v.iter().zip(subj) {
        if *m == 1
            && let Some(h) = hist.get_mut(usize::try_from(*v).unwrap_or(0).min(255))
        {
            *h += 1;
            count += 1;
        }
    }
    if count < 16 {
        hist = [0; 256];
        for v in &lum.v {
            if let Some(h) = hist.get_mut(usize::try_from(*v).unwrap_or(0).min(255)) {
                *h += 1;
            }
        }
        count = u32::try_from(lum.v.len()).unwrap_or(u32::MAX);
    }
    let percentile = |pct: u32| -> i32 {
        let target = u64::from(count) * u64::from(pct) / 100;
        let mut acc = 0u64;
        for (i, h) in hist.iter().enumerate() {
            acc += u64::from(*h);
            if acc > target {
                return i32::try_from(i).unwrap_or(0);
            }
        }
        255
    };
    let (lo, hi) = (percentile(2), percentile(98));
    let span = (hi - lo).max(16);
    let mut t = Plane::new(side, 0);
    for (o, v) in t.v.iter_mut().zip(&lum.v) {
        *o = ((*v - lo) * WHITE / span).clamp(0, WHITE);
    }
    // Local contrast: the tone minus a large blur, so faces (darker skin too) stay
    // readable against their surroundings.
    let large = t.blur((side / 16).max(1));
    for (o, b) in t.v.iter_mut().zip(&large.v) {
        *o = (*o + (*o - *b) * 3 / 5).clamp(0, WHITE);
    }
    // Contrast and Brightness.
    let mid = WHITE / 2;
    for o in &mut t.v {
        let c = (*o - mid) * (100 + s.contrast * 3 / 2) / 100 + mid + s.brightness * 24;
        *o = c.clamp(0, WHITE);
    }
    // A light sharpening.
    let small = t.blur(1);
    for (o, b) in t.v.iter_mut().zip(&small.v) {
        *o = (*o + (*o - *b) / 2).clamp(0, WHITE);
    }
    t
}

/// The ink coverage (0–256) of the hatching at one pixel.
struct Hatch {
    spacing: i64,
    min_half: i64,
    max_half: i64,
    curve: i64,
}

/// Tones below which each layer draws (about 92 %, 58 % and 30 %).
const LAYERS: [i64; 3] = [3768, 2375, 1229];

impl Hatch {
    fn new(s: &Settings, side: usize) -> Hatch {
        let side = i64::try_from(side).unwrap_or(512);
        // 9 px (Detail 0) to 3 px (Detail 100) at 512, in fp.
        let spacing = ((900 - 6 * i64::from(s.detail)) * PX / 100 * side / 512).max(PX);
        let max_half = spacing * (25 + i64::from(s.line_weight) / 4) / 100;
        Hatch {
            spacing,
            min_half: (PX / 4 * side / 512).max(PX / 8).min(max_half),
            max_half,
            curve: i64::from(s.curve),
        }
    }

    fn coverage(&self, x: i64, y: i64, t: i64, smooth: i64) -> i64 {
        let (xc, yc) = (x * PX + PX / 2, y * PX + PX / 2);
        let sp = self.spacing;
        // Lines move across their direction with the smoothed tone (up to two
        // spacings at Curve 100), so they flow around shapes.
        let shift =
            (smooth - i64::from(WHITE) / 2) * self.curve * sp * 2 / (i64::from(WHITE) / 2 * 100);
        let mut ink = 0;
        for (layer, threshold) in LAYERS.iter().enumerate() {
            if t >= *threshold {
                continue;
            }
            let frac = (threshold - t) * 256 / threshold;
            let half = self.min_half + (self.max_half - self.min_half) * frac / 256;
            let p = match layer {
                0 => (xc + yc) * 181 / 256,
                1 => (xc - yc) * 181 / 256 + sp / 2,
                _ => yc + sp / 3,
            } + shift;
            let m = p.rem_euclid(sp);
            let d = m.min(sp - m);
            let c = (half - d + PX / 2).clamp(0, PX);
            ink = ink.max(c);
        }
        ink
    }
}

/// Engrave a square RGB picture (`img`, any side ≥ 2, the working size) into an
/// `out` × `out` RGBA picture; `out` divides the side or equals it. `mask` is a
/// [`MASK_SIDE`]² brush mask (or `None`).
pub fn engrave(
    img: &Rgb,
    settings: &Settings,
    mask: Option<&[u8]>,
    out: u32,
) -> ImportResult<Rgba> {
    let s = settings.clamped();
    if img.width != img.height || img.width < 2 {
        return Err(bad("the picture must be square"));
    }
    if out == 0 || !img.width.is_multiple_of(out) {
        return Err(bad("the result size must divide the working size"));
    }
    if mask.is_some_and(|m| m.len() != to_usize(MASK_SIDE) * to_usize(MASK_SIDE)) {
        return Err(bad("the brush mask has the wrong size"));
    }
    let side = to_usize(img.width);
    let lum = luminance(img, side);
    let subj = subject(img, &lum, &s, mask);
    let removed = subj.contains(&0);
    let t = tone(&lum, &subj, &s);
    let smooth = t.blur((side / 24).max(1));
    let edges = gradient(&t.blur(1));
    let hatch = Hatch::new(&s, side);
    // Alpha: the subject with its border smoothed by one pixel.
    let alpha = Plane {
        side,
        v: subj.iter().map(|&v| i32::from(v) * 255).collect(),
    }
    .blur(1);
    // A thin ink line along the silhouette when the background is removed.
    let rim = (side / 256).max(1);
    let near_edge = if removed {
        Plane {
            side,
            v: subj.iter().map(|&v| i32::from(v) * 1000).collect(),
        }
        .blur(rim)
    } else {
        Plane::new(side, 1000)
    };
    let s64 = i64::try_from(side).unwrap_or(0);
    let (ink, paper) = (s.ink, s.paper);
    let outline = i64::from(s.outline);
    let mut work = vec![0u8; side * side * 4];
    for y in 0..s64 {
        for x in 0..s64 {
            let i = usize::try_from(y * s64 + x).unwrap_or(0);
            let tv = i64::from(t.v.get(i).copied().unwrap_or(WHITE));
            let sm = i64::from(smooth.v.get(i).copied().unwrap_or(WHITE));
            let mut cov = hatch.coverage(x, y, tv, sm);
            // Strong tone edges, by Outline.
            let g = i64::from(edges.v.get(i).copied().unwrap_or(0)) / 4;
            let edge = ((g - 1200) * outline * PX / (1600 * 100)).clamp(0, PX);
            cov = cov.max(edge);
            if subj.get(i) == Some(&1) && near_edge.v.get(i).copied().unwrap_or(1000) < 1000 {
                cov = PX;
            }
            let a = i64::from(alpha.v.get(i).copied().unwrap_or(255));
            if let Some(px) = work.get_mut(i * 4..i * 4 + 4) {
                for ((v, k), p) in px.iter_mut().zip(ink).zip(paper) {
                    let (k, p) = (i64::from(k), i64::from(p));
                    *v = to_u8(p + (k - p) * cov / PX);
                }
                if let Some(v) = px.get_mut(3) {
                    *v = to_u8(a);
                }
            }
        }
    }
    Ok(downscale(&work, side, to_usize(out), paper))
}

/// Average `f` × `f` blocks (alpha-weighted colour) down to `out` × `out`.
fn downscale(work: &[u8], side: usize, out: usize, paper: [u8; 3]) -> Rgba {
    let f = (side / out.max(1)).max(1);
    let mut data = Vec::with_capacity(out * out * 4);
    for oy in 0..out {
        for ox in 0..out {
            let mut sum = [0i64; 3];
            let mut alpha = 0i64;
            for y in oy * f..(oy + 1) * f {
                for x in ox * f..(ox + 1) * f {
                    let i = (y * side + x) * 4;
                    let a = i64::from(work.get(i + 3).copied().unwrap_or(0));
                    for (c, acc) in sum.iter_mut().enumerate() {
                        *acc += i64::from(work.get(i + c).copied().unwrap_or(0)) * a;
                    }
                    alpha += a;
                }
            }
            let n = i64::try_from(f * f).unwrap_or(1);
            for (c, acc) in sum.iter().enumerate() {
                let v = if alpha > 0 {
                    (acc + alpha / 2) / alpha
                } else {
                    i64::from(paper.get(c).copied().unwrap_or(255))
                };
                data.push(to_u8(v));
            }
            data.push(to_u8((alpha + n / 2) / n));
        }
    }
    let side = u32::try_from(out).unwrap_or(0);
    Rgba {
        width: side,
        height: side,
        data,
    }
}

/// Encode an RGBA raster as an 8-bit RGBA PNG file (no metadata).
pub fn encode_png_rgba(img: &Rgba) -> ImportResult<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc
            .write_header()
            .map_err(|e| ImportError::Invalid(format!("the PNG could not be written: {e}")))?;
        w.write_image_data(&img.data)
            .map_err(|e| ImportError::Invalid(format!("the PNG could not be written: {e}")))?;
    }
    Ok(out)
}

/// A test photo: a dark figure (head and shoulders) on a plain light background,
/// with a soft light from the left (acceptance tests 224 and 228).
pub fn test_photo(side: u32) -> Rgb {
    let s = i64::from(side);
    let mut data = Vec::with_capacity(to_usize(side) * to_usize(side) * 3);
    for y in 0..s {
        for x in 0..s {
            // Head: a circle; shoulders: a wide rounded block below.
            let (hx, hy, hr) = (s / 2, s * 2 / 5, s / 6);
            let head = (x - hx) * (x - hx) + (y - hy) * (y - hy) <= hr * hr;
            let body = y > s * 3 / 5 && (x - s / 2).abs() < s / 3 - (s * 3 / 5 + s / 8 - y).max(0);
            let shade = x * 40 / s.max(1);
            let px = if head {
                [70 + shade, 50 + shade, 40 + shade]
            } else if body {
                [30 + shade / 2, 34 + shade / 2, 48 + shade / 2]
            } else {
                [228, 226, 220]
            };
            data.extend(px.map(to_u8));
        }
    }
    Rgb {
        width: side,
        height: side,
        data,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn crop_and_engrave_shapes() {
        let photo = test_photo(300);
        let c = crop_square(&photo, 10, 20, 200, 512).unwrap();
        assert_eq!((c.width, c.height, c.data.len()), (512, 512, 512 * 512 * 3));
        let d = crop_square(&photo, 0, 0, 300, 256).unwrap();
        assert_eq!(d.data.len(), 256 * 256 * 3);
        assert!(crop_square(&photo, 200, 0, 200, 256).is_err());
        let out = engrave(&d, &Settings::default(), None, 256).unwrap();
        assert_eq!(out.data.len(), 256 * 256 * 4);
        assert!(out.data.chunks(4).all(|p| p[3] == 255), "background kept");
        let s = Settings {
            background: 60,
            ..Settings::default()
        };
        let cut = engrave(&d, &s, None, 256).unwrap();
        let a = |x: usize, y: usize| cut.data[(y * 256 + x) * 4 + 3];
        assert_eq!(a(2, 2), 0, "corner removed");
        assert_eq!(a(128, 100), 255, "head kept");
        assert_eq!(a(128, 230), 255, "body kept");
        assert_eq!(engrave(&d, &s, None, 256).unwrap(), cut, "deterministic");
    }
}
