//! The app's window and taskbar icon (app-desktop.md revision 25): MoneyKeep's own
//! small calculator, the toolbar icon's design in colour, drawn into an RGBA image
//! with integer arithmetic (no artwork files).

/// The icon's side in pixels.
pub const APP_ICON_SIZE: u32 = 256;

const BODY: [u8; 3] = [0x26, 0x2E, 0x36];
const DISPLAY: [u8; 3] = [0xDF, 0xEE, 0xE8];
const KEY: [u8; 3] = [0x4F, 0xB3, 0xAE];
const EQUALS_KEY: [u8; 3] = [0xF2, 0xA6, 0x3B];

/// Subsamples per pixel side (4 × 4 per pixel) for smooth edges.
const SUB: i64 = 4;

/// A rounded rectangle in units of 1/32 of the icon side.
#[derive(Debug, Clone, Copy)]
struct Shape {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
    /// Corner radius in 1/64 of the side.
    radius: i64,
    color: [u8; 3],
}

impl Shape {
    /// Whether the subsample point (in 1/(side·SUB) units) lies inside.
    fn contains(&self, x: i64, y: i64, side: i64) -> bool {
        let scale = |v: i64| v * side * SUB / 32;
        let (x0, y0, x1, y1) = (
            scale(self.x0),
            scale(self.y0),
            scale(self.x1),
            scale(self.y1),
        );
        let r = self.radius * side * SUB / 64;
        if x < x0 || x >= x1 || y < y0 || y >= y1 {
            return false;
        }
        let cx = x.clamp(x0 + r, (x1 - r).max(x0 + r));
        let cy = y.clamp(y0 + r, (y1 - r).max(y0 + r));
        let (dx, dy) = (x - cx, y - cy);
        dx * dx + dy * dy <= r * r
    }
}

fn shapes() -> Vec<Shape> {
    let mut v = vec![
        Shape {
            x0: 6,
            y0: 2,
            x1: 26,
            y1: 30,
            radius: 14,
            color: BODY,
        },
        Shape {
            x0: 9,
            y0: 5,
            x1: 23,
            y1: 11,
            radius: 3,
            color: DISPLAY,
        },
    ];
    // Three rows of three keys, four units square with one unit between them; the
    // bottom right one (equals) in a second accent.
    for row in 0..3 {
        for col in 0..3 {
            let (x0, y0) = (9 + col * 5, 14 + row * 5);
            v.push(Shape {
                x0,
                y0,
                x1: x0 + 4,
                y1: y0 + 4,
                radius: 2,
                color: if row == 2 && col == 2 {
                    EQUALS_KEY
                } else {
                    KEY
                },
            });
        }
    }
    v
}

/// The icon as RGBA bytes, `side` × `side`, rows top to bottom.
pub fn app_icon_rgba(side: u32) -> Vec<u8> {
    let n = i64::from(side);
    let shapes = shapes();
    let mut out = Vec::with_capacity((side as usize) * (side as usize) * 4);
    for py in 0..n {
        for px in 0..n {
            // Sum the colour of the topmost shape at each subsample.
            let (mut r, mut g, mut b, mut hits) = (0_i64, 0_i64, 0_i64, 0_i64);
            for sy in 0..SUB {
                for sx in 0..SUB {
                    let (x, y) = (px * SUB + sx, py * SUB + sy);
                    if let Some(s) = shapes.iter().rev().find(|s| s.contains(x, y, n)) {
                        r += i64::from(s.color[0]);
                        g += i64::from(s.color[1]);
                        b += i64::from(s.color[2]);
                        hits += 1;
                    }
                }
            }
            if hits == 0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let byte = |v: i64| u8::try_from(v).unwrap_or(u8::MAX);
                out.extend_from_slice(&[
                    byte(r / hits),
                    byte(g / hits),
                    byte(b / hits),
                    byte(hits * 255 / (SUB * SUB)),
                ]);
            }
        }
    }
    out
}

/// The icon drawn `inner` × `inner` in the middle of an opaque `side` × `side`
/// square of `background` (phone home screens that show no transparency, and
/// "maskable" icons whose edges may be cut, mobile-capture.md revision 3).
pub fn app_icon_on(side: u32, inner: u32, background: [u8; 3]) -> Vec<u8> {
    let inner = inner.min(side);
    let src = app_icon_rgba(inner);
    let off = (side - inner) / 2;
    let mut out = Vec::with_capacity((side as usize) * (side as usize) * 4);
    for y in 0..side {
        for x in 0..side {
            let inside = x >= off && y >= off && x < off + inner && y < off + inner;
            let px = if inside {
                let i = (((y - off) * inner + (x - off)) * 4) as usize;
                src.get(i..i + 4)
            } else {
                None
            };
            let [r, g, b, a] = match px {
                Some(&[r, g, b, a]) => [r, g, b, a],
                _ => [0, 0, 0, 0],
            };
            let mix = |fg: u8, bg: u8| -> u8 {
                let v = (u32::from(fg) * u32::from(a) + u32::from(bg) * (255 - u32::from(a))) / 255;
                u8::try_from(v).unwrap_or(u8::MAX)
            };
            out.extend_from_slice(&[
                mix(r, background[0]),
                mix(g, background[1]),
                mix(b, background[2]),
                255,
            ]);
        }
    }
    out
}

/// The sizes in [`app_icon_ico`]: what Windows shows in Explorer, the Start menu,
/// the taskbar, shortcuts, "Installed apps" and the setup's own window.
pub const ICO_SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];

/// The icon as a Windows `.ico` file (release-packaging.md: the setup's, the
/// uninstaller's and the shortcuts' icon), built from the same drawing. The ICO
/// container (an ICONDIR header, one ICONDIRENTRY per image, then the images) holds
/// 32-bit BMP images (BITMAPINFOHEADER, bottom-up BGRA rows and a 1-bit mask) up to
/// 64 pixels, and a PNG image at 256 pixels, as Windows Vista and later read them.
pub fn app_icon_ico() -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = ICO_SIZES
        .iter()
        .map(|&side| {
            let rgba = app_icon_rgba(side);
            let data = if side >= 256 {
                png_bytes(side, &rgba).unwrap_or_else(|| bmp_dib(side, &rgba))
            } else {
                bmp_dib(side, &rgba)
            };
            (side, data)
        })
        .collect();
    let le16 = |v: usize| u16::try_from(v).unwrap_or(u16::MAX).to_le_bytes();
    let le32 = |v: usize| u32::try_from(v).unwrap_or(u32::MAX).to_le_bytes();
    let mut out = Vec::new();
    // ICONDIR: reserved, type 1 (icon), image count.
    out.extend_from_slice(&le16(0));
    out.extend_from_slice(&le16(1));
    out.extend_from_slice(&le16(images.len()));
    let mut offset = 6 + 16 * images.len();
    for (side, data) in &images {
        // Width and height 0 mean 256.
        let dim = u8::try_from(*side).unwrap_or(0);
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&le16(1)); // colour planes
        out.extend_from_slice(&le16(32)); // bits per pixel
        out.extend_from_slice(&le32(data.len()));
        out.extend_from_slice(&le32(offset));
        offset += data.len();
    }
    for (_, data) in &images {
        out.extend_from_slice(data);
    }
    out
}

/// The images in [`app_icon_icns`]: (ICNS type, side in pixels). The `icp4`–`ic10`
/// entries are the 1x images from 16 to 1024 pixels; `ic11`–`ic14` the Retina (2x)
/// images of the 16, 32, 128 and 256 point sizes. Every image is a PNG (macOS 10.7
/// and later read PNG in all of these types).
pub const ICNS_ENTRIES: [([u8; 4], u32); 11] = [
    (*b"icp4", 16),
    (*b"icp5", 32),
    (*b"icp6", 64),
    (*b"ic07", 128),
    (*b"ic08", 256),
    (*b"ic09", 512),
    (*b"ic10", 1024),
    (*b"ic11", 32),
    (*b"ic12", 64),
    (*b"ic13", 256),
    (*b"ic14", 512),
];

/// The icon as a macOS `.icns` file (macos-release.md P2: the app bundle's icon), built
/// from the same drawing at build time. The container is the magic `icns` and the
/// file's length, then one entry per image: its four-byte type, its length including
/// that eight-byte header, and the PNG data; lengths are big-endian.
pub fn app_icon_icns() -> Vec<u8> {
    let mut pngs: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut body = Vec::new();
    for (kind, side) in ICNS_ENTRIES {
        if !pngs.iter().any(|(s, _)| *s == side) {
            let Some(png) = png_bytes(side, &app_icon_rgba(side)) else {
                continue;
            };
            pngs.push((side, png));
        }
        let Some((_, png)) = pngs.iter().find(|(s, _)| *s == side) else {
            continue;
        };
        let len = u32::try_from(png.len() + 8).unwrap_or(u32::MAX);
        body.extend_from_slice(&kind);
        body.extend_from_slice(&len.to_be_bytes());
        body.extend_from_slice(png);
    }
    let total = u32::try_from(body.len() + 8).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(&body);
    out
}

fn png_bytes(side: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, side, side);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(rgba).ok()?;
    }
    Some(out)
}

/// A 32-bit device-independent bitmap as an icon image: the header gives twice the
/// height (colour rows, then the AND mask), rows run bottom to top.
fn bmp_dib(side: u32, rgba: &[u8]) -> Vec<u8> {
    let n = side as usize;
    let mask_row = n.div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + n * n * 4 + mask_row * n);
    let le16 = |v: u16| v.to_le_bytes();
    let le32 = |v: u32| v.to_le_bytes();
    out.extend_from_slice(&le32(40)); // BITMAPINFOHEADER size
    out.extend_from_slice(&le32(side)); // width
    out.extend_from_slice(&le32(side.saturating_mul(2))); // height: colour + mask
    out.extend_from_slice(&le16(1)); // planes
    out.extend_from_slice(&le16(32)); // bits per pixel
    out.extend_from_slice(&le32(0)); // no compression
    out.extend_from_slice(&le32(0)); // image size (may be 0 uncompressed)
    out.extend_from_slice(&[0; 16]); // resolution and palette: unused
    for y in (0..n).rev() {
        for x in 0..n {
            let i = (y * n + x) * 4;
            let p = rgba.get(i..i + 4).unwrap_or(&[0, 0, 0, 0]);
            let (r, g, b, a) = (
                p.first().copied().unwrap_or(0),
                p.get(1).copied().unwrap_or(0),
                p.get(2).copied().unwrap_or(0),
                p.get(3).copied().unwrap_or(0),
            );
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    // The AND mask: 1 where fully transparent (older readers), rows padded to 32
    // bits, bottom to top.
    for y in (0..n).rev() {
        let mut row = vec![0_u8; mask_row];
        for x in 0..n {
            let a = rgba.get((y * n + x) * 4 + 3).copied().unwrap_or(0);
            if a == 0
                && let Some(byte) = row.get_mut(x / 8)
            {
                *byte |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&row);
    }
    out
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn pixel(img: &[u8], side: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * side + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    #[test]
    fn draws_a_calculator() {
        let side = APP_ICON_SIZE;
        let img = app_icon_rgba(side);
        assert_eq!(img.len(), (side * side * 4) as usize);
        // Corners are transparent; the body is dark, the display light.
        assert_eq!(pixel(&img, side, 0, 0)[3], 0);
        assert_eq!(pixel(&img, side, side - 1, side - 1)[3], 0);
        let body = pixel(&img, side, side / 2, side * 37 / 64);
        assert_eq!(&body[..3], &BODY, "between the keys");
        let display = pixel(&img, side, side / 2, side / 4);
        assert_eq!(&display[..3], &DISPLAY);
        assert_eq!(display[3], 255);
        let key = pixel(&img, side, side / 2, side / 2);
        assert_eq!(&key[..3], &KEY);
    }
}
