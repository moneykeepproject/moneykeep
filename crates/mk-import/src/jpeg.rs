//! A baseline JPEG encoder (ITU-T T.81: sequential DCT, Huffman coding, 8-bit
//! samples) for receipt pictures (expenses.md). Written from the standard: YCbCr
//! colour conversion per JFIF, 4:2:0 chroma subsampling, the example quantisation
//! tables of Annex K.1 scaled by a quality, and Huffman tables built for each picture
//! from its own symbol counts (Annex K.2). Integer arithmetic only. The output holds
//! no metadata beyond the JFIF header: no EXIF, no GPS, no comments, no profiles.

use crate::image::Rgb;
use crate::{ImportError, ImportResult};

/// The 1-D DCT basis in 12-bit fixed point: `round(4096 · a(u) · cos((2x+1)uπ/16))`
/// with `a(0) = √(1/8)` and `a(u) = 1/2`.
const COS: [[i64; 8]; 8] = [
    [1448, 1448, 1448, 1448, 1448, 1448, 1448, 1448],
    [2009, 1703, 1138, 400, -400, -1138, -1703, -2009],
    [1892, 784, -784, -1892, -1892, -784, 784, 1892],
    [1703, -400, -2009, -1138, 1138, 2009, 400, -1703],
    [1448, -1448, -1448, 1448, 1448, -1448, -1448, 1448],
    [1138, -2009, 400, 1703, -1703, -400, 2009, -1138],
    [784, -1892, 1892, -784, -784, 1892, -1892, 784],
    [400, -1138, 1703, -2009, 2009, -1703, 1138, -400],
];

/// Annex K.1, Table K.1: luminance quantisation (row-major).
const LUMA_Q: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

/// Annex K.1, Table K.2: chrominance quantisation (row-major).
const CHROMA_Q: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

fn bad(m: impl Into<String>) -> ImportError {
    ImportError::Invalid(m.into())
}

/// Zig-zag position → row-major index (Figure A.6).
fn zigzag() -> [usize; 64] {
    let mut out = [0usize; 64];
    let mut k = 0usize;
    for s in 0..15usize {
        let lo = s.saturating_sub(7);
        let hi = s.min(7);
        let rows: Vec<usize> = if s % 2 == 0 {
            (lo..=hi).rev().collect()
        } else {
            (lo..=hi).collect()
        };
        for row in rows {
            let col = s - row;
            if let Some(slot) = out.get_mut(k) {
                *slot = row * 8 + col;
            }
            k += 1;
        }
    }
    out
}

/// A quantisation table scaled for `quality` (1–100), row-major.
fn scaled(base: &[u16; 64], quality: u8) -> [u16; 64] {
    let q = u32::from(quality.clamp(1, 100));
    let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };
    let mut out = [1u16; 64];
    for (o, b) in out.iter_mut().zip(base) {
        let v = (u32::from(*b) * scale + 50) / 100;
        *o = u16::try_from(v.clamp(1, 255)).unwrap_or(255);
    }
    out
}

/// Forward DCT and quantisation of one level-shifted block; the result is in
/// zig-zag order.
fn fdct_quant(block: &[i64; 64], q: &[u16; 64], zz: &[usize; 64]) -> [i16; 64] {
    let mut tmp = [0i64; 64];
    for y in 0..8 {
        for (u, basis) in COS.iter().enumerate() {
            let mut s = 0i64;
            for (x, c) in basis.iter().enumerate() {
                s += c * block.get(y * 8 + x).copied().unwrap_or(0);
            }
            if let Some(t) = tmp.get_mut(y * 8 + u) {
                *t = s;
            }
        }
    }
    let mut coef = [0i64; 64];
    for (v, basis) in COS.iter().enumerate() {
        for u in 0..8 {
            let mut s = 0i64;
            for (y, c) in basis.iter().enumerate() {
                s += c * tmp.get(y * 8 + u).copied().unwrap_or(0);
            }
            if let Some(t) = coef.get_mut(v * 8 + u) {
                *t = s;
            }
        }
    }
    let mut out = [0i16; 64];
    for (k, idx) in zz.iter().enumerate() {
        let d = i64::from(q.get(*idx).copied().unwrap_or(1)) << 24;
        let c = coef.get(*idx).copied().unwrap_or(0);
        // Round half away from zero.
        let v = if c >= 0 {
            (c + d / 2) / d
        } else {
            -((-c + d / 2) / d)
        };
        if let Some(o) = out.get_mut(k) {
            *o = i16::try_from(v.clamp(-1023, 1023)).unwrap_or(0);
        }
    }
    out
}

/// Number of bits of |v| (the T.81 magnitude category).
fn category(v: i32) -> u8 {
    let mut a = v.unsigned_abs();
    let mut n = 0u8;
    while a > 0 {
        n += 1;
        a >>= 1;
    }
    n
}

/// The additional bits of a value in category `n` (one's complement for negatives).
fn extra_bits(v: i32, n: u8) -> u32 {
    if v >= 0 {
        v.unsigned_abs()
    } else {
        let mask = (1u32 << n) - 1;
        u32::from_ne_bytes((v - 1).to_ne_bytes()) & mask
    }
}

/// A Huffman table: counts of codes of each length 1–16, symbols in code order, and
/// the (code, length) of every symbol.
struct Huff {
    bits: [u8; 16],
    vals: Vec<u8>,
    codes: [(u16, u8); 256],
}

/// Build a table from symbol counts (Annex K.2): Huffman code sizes, limited to 16
/// bits, with a reserved symbol so that no code is all ones.
fn build_huff(freq: &[u32; 256]) -> Huff {
    // Leaves: (count, symbols); a reserved symbol 256 with count 1.
    let mut size = [0usize; 257];
    let mut nodes: Vec<(u64, Vec<usize>)> = freq
        .iter()
        .enumerate()
        .filter(|(_, f)| **f > 0)
        .map(|(s, f)| (u64::from(*f), vec![s]))
        .collect();
    nodes.push((1, vec![256]));
    if nodes.len() == 1 {
        nodes.push((0, Vec::new()));
    }
    while nodes.len() > 1 {
        nodes.sort_by_key(|n| std::cmp::Reverse(n.0));
        let (Some(a), Some(b)) = (nodes.pop(), nodes.pop()) else {
            break;
        };
        for s in a.1.iter().chain(b.1.iter()) {
            if let Some(sz) = size.get_mut(*s) {
                *sz += 1;
            }
        }
        let mut syms = a.1;
        syms.extend(b.1);
        nodes.push((a.0 + b.0, syms));
    }
    let max = size.iter().copied().max().unwrap_or(1).max(16);
    let mut bits = vec![0u32; max + 2];
    for s in size.iter().filter(|s| **s > 0) {
        if let Some(b) = bits.get_mut(*s) {
            *b += 1;
        }
    }
    // Limit code lengths to 16 (Figure K.3).
    let mut i = max;
    while i > 16 {
        while bits.get(i).copied().unwrap_or(0) > 0 {
            let mut j = i - 2;
            while j > 0 && bits.get(j).copied().unwrap_or(0) == 0 {
                j -= 1;
            }
            if let Some(b) = bits.get_mut(i) {
                *b -= 2;
            }
            if let Some(b) = bits.get_mut(i - 1) {
                *b += 1;
            }
            if let Some(b) = bits.get_mut(j + 1) {
                *b += 2;
            }
            if let Some(b) = bits.get_mut(j) {
                *b = b.saturating_sub(1);
            }
        }
        i -= 1;
    }
    // Remove the reserved symbol from the longest codes.
    let mut i = 16;
    while i > 0 && bits.get(i).copied().unwrap_or(0) == 0 {
        i -= 1;
    }
    if let Some(b) = bits.get_mut(i) {
        *b = b.saturating_sub(1);
    }
    // Symbols by code size (then value), sizes re-assigned from the limited counts.
    let mut order: Vec<usize> = (0..256)
        .filter(|s| freq.get(*s).copied().unwrap_or(0) > 0)
        .collect();
    order.sort_by_key(|s| (size.get(*s).copied().unwrap_or(0), *s));
    let mut out_bits = [0u8; 16];
    for (k, b) in out_bits.iter_mut().enumerate() {
        *b = u8::try_from(bits.get(k + 1).copied().unwrap_or(0)).unwrap_or(0);
    }
    let vals: Vec<u8> = order.iter().filter_map(|s| u8::try_from(*s).ok()).collect();
    let mut codes = [(0u16, 0u8); 256];
    let mut code = 0u32;
    let mut it = vals.iter();
    for (len, n) in out_bits.iter().enumerate() {
        for _ in 0..*n {
            if let Some(v) = it.next()
                && let Some(c) = codes.get_mut(usize::from(*v))
            {
                *c = (
                    u16::try_from(code).unwrap_or(0),
                    u8::try_from(len + 1).unwrap_or(16),
                );
            }
            code += 1;
        }
        code <<= 1;
    }
    Huff {
        bits: out_bits,
        vals,
        codes,
    }
}

struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    n: u8,
}

impl BitWriter {
    fn put(&mut self, code: u32, len: u8) {
        for i in (0..len).rev() {
            self.acc = (self.acc << 1) | ((code >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                let byte = u8::try_from(self.acc & 0xFF).unwrap_or(0);
                self.out.push(byte);
                if byte == 0xFF {
                    self.out.push(0);
                }
                self.acc = 0;
                self.n = 0;
            }
        }
    }

    fn flush(&mut self) {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put((1 << pad) - 1, pad);
        }
    }
}

/// Visit the symbols of one block: DC category, then AC (run, size) symbols, with
/// their additional bits.
fn block_symbols(b: &[i16; 64], pred: &mut i32, mut f: impl FnMut(bool, u8, u32, u8)) {
    let dc = i32::from(b.first().copied().unwrap_or(0));
    let diff = dc - *pred;
    *pred = dc;
    let n = category(diff);
    f(true, n, extra_bits(diff, n), n);
    let mut run = 0u8;
    for c in b.iter().skip(1) {
        let v = i32::from(*c);
        if v == 0 {
            run += 1;
            continue;
        }
        while run >= 16 {
            f(false, 0xF0, 0, 0);
            run -= 16;
        }
        let n = category(v);
        f(false, (run << 4) | n, extra_bits(v, n), n);
        run = 0;
    }
    if run > 0 {
        f(false, 0x00, 0, 0);
    }
}

fn segment(out: &mut Vec<u8>, marker: u8, body: &[u8]) -> ImportResult<()> {
    let len = u16::try_from(body.len() + 2).map_err(|_| bad("JPEG segment too long"))?;
    out.extend([0xFF, marker]);
    out.extend(len.to_be_bytes());
    out.extend_from_slice(body);
    Ok(())
}

/// Encode an RGB raster as a baseline JFIF file at `quality` (1–100).
pub fn encode_jpeg(img: &Rgb, quality: u8) -> ImportResult<Vec<u8>> {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 || w > 65_535 || h > 65_535 {
        return Err(bad("the image cannot be written as JPEG"));
    }
    let (wu, hu) = (
        usize::try_from(w).map_err(|_| bad("too large"))?,
        usize::try_from(h).map_err(|_| bad("too large"))?,
    );
    if img.data.len() != wu.saturating_mul(hu).saturating_mul(3) {
        return Err(bad("the image data is short"));
    }
    let zz = zigzag();
    let lq = scaled(&LUMA_Q, quality);
    let cq = scaled(&CHROMA_Q, quality);
    let (mcu_x, mcu_y) = (wu.div_ceil(16), hu.div_ceil(16));
    let mut blocks: Vec<[i16; 64]> = Vec::with_capacity(mcu_x * mcu_y * 6);
    // Y, Cb, Cr of the pixel at (x, y), edges repeated.
    let ycc = |x: usize, y: usize| -> (i64, i64, i64) {
        let (x, y) = (x.min(wu - 1), y.min(hu - 1));
        let at = (y * wu + x) * 3;
        let px = img.data.get(at..at + 3).unwrap_or(&[0, 0, 0]);
        let (r, g, b) = (
            i64::from(px.first().copied().unwrap_or(0)),
            i64::from(px.get(1).copied().unwrap_or(0)),
            i64::from(px.get(2).copied().unwrap_or(0)),
        );
        let yy = (19_595 * r + 38_470 * g + 7_471 * b + 32_768) >> 16;
        let cb = ((-11_059 * r - 21_709 * g + 32_768 * b + 32_768) >> 16) + 128;
        let cr = ((32_768 * r - 27_439 * g - 5_329 * b + 32_768) >> 16) + 128;
        (yy.clamp(0, 255), cb.clamp(0, 255), cr.clamp(0, 255))
    };
    for my in 0..mcu_y {
        for mx in 0..mcu_x {
            let (x0, y0) = (mx * 16, my * 16);
            let mut ys = [[0i64; 64]; 4];
            let mut cb = [0i64; 64];
            let mut cr = [0i64; 64];
            let mut sums = [[0i64; 2]; 64];
            for dy in 0..16 {
                for dx in 0..16 {
                    let (yv, cbv, crv) = ycc(x0 + dx, y0 + dy);
                    let blk = (dy / 8) * 2 + dx / 8;
                    if let Some(b) = ys.get_mut(blk)
                        && let Some(s) = b.get_mut((dy % 8) * 8 + dx % 8)
                    {
                        *s = yv - 128;
                    }
                    if let Some(s) = sums.get_mut((dy / 2) * 8 + dx / 2) {
                        s[0] += cbv;
                        s[1] += crv;
                    }
                }
            }
            for (i, s) in sums.iter().enumerate() {
                if let Some(c) = cb.get_mut(i) {
                    *c = (s[0] + 2) / 4 - 128;
                }
                if let Some(c) = cr.get_mut(i) {
                    *c = (s[1] + 2) / 4 - 128;
                }
            }
            for b in &ys {
                blocks.push(fdct_quant(b, &lq, &zz));
            }
            blocks.push(fdct_quant(&cb, &cq, &zz));
            blocks.push(fdct_quant(&cr, &cq, &zz));
        }
    }
    // Pass 1: symbol counts per table (0 luminance, 1 chrominance).
    let mut dc_freq = [[0u32; 256]; 2];
    let mut ac_freq = [[0u32; 256]; 2];
    let mut preds = [0i32; 3];
    for (i, b) in blocks.iter().enumerate() {
        let comp = match i % 6 {
            0..=3 => 0,
            4 => 1,
            _ => 2,
        };
        let t = usize::from(comp > 0);
        let Some(pred) = preds.get_mut(comp) else {
            continue;
        };
        block_symbols(b, pred, |dc, sym, _, _| {
            let table = if dc { &mut dc_freq } else { &mut ac_freq };
            if let Some(f) = table.get_mut(t).and_then(|x| x.get_mut(usize::from(sym))) {
                *f += 1;
            }
        });
    }
    let [dl, dch] = &dc_freq;
    let [al, ach] = &ac_freq;
    let tables = [
        build_huff(dl),
        build_huff(dch),
        build_huff(al),
        build_huff(ach),
    ];

    let mut out = vec![0xFF, 0xD8];
    segment(
        &mut out,
        0xE0,
        &[b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0],
    )?;
    let mut dqt = Vec::with_capacity(130);
    for (id, q) in [(0u8, &lq), (1u8, &cq)] {
        dqt.push(id);
        for idx in &zz {
            dqt.push(u8::try_from(q.get(*idx).copied().unwrap_or(1)).unwrap_or(255));
        }
    }
    segment(&mut out, 0xDB, &dqt)?;
    let (wb, hb) = (
        u16::try_from(w).unwrap_or(0).to_be_bytes(),
        u16::try_from(h).unwrap_or(0).to_be_bytes(),
    );
    segment(
        &mut out,
        0xC0,
        &[
            8, hb[0], hb[1], wb[0], wb[1], 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1,
        ],
    )?;
    let mut dht = Vec::new();
    for (class_id, t) in [0x00u8, 0x01, 0x10, 0x11].iter().zip(&tables) {
        dht.push(*class_id);
        dht.extend_from_slice(&t.bits);
        dht.extend_from_slice(&t.vals);
    }
    segment(&mut out, 0xC4, &dht)?;
    segment(&mut out, 0xDA, &[3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0])?;

    // Pass 2: the entropy-coded data.
    let mut bw = BitWriter { out, acc: 0, n: 0 };
    let mut preds = [0i32; 3];
    let [hdl, hdc, hal, hac] = &tables;
    for (i, b) in blocks.iter().enumerate() {
        let comp = match i % 6 {
            0..=3 => 0,
            4 => 1,
            _ => 2,
        };
        let (dc_t, ac_t) = if comp == 0 { (hdl, hal) } else { (hdc, hac) };
        let Some(pred) = preds.get_mut(comp) else {
            continue;
        };
        let mut emits: Vec<(u32, u8)> = Vec::with_capacity(32);
        block_symbols(b, pred, |dc, sym, extra, n| {
            let t = if dc { dc_t } else { ac_t };
            let (code, len) = t.codes.get(usize::from(sym)).copied().unwrap_or((0, 0));
            emits.push((u32::from(code), len));
            if n > 0 {
                emits.push((extra, n));
            }
        });
        for (code, len) in emits {
            bw.put(code, len);
        }
    }
    bw.flush();
    let mut out = bw.out;
    out.extend([0xFF, 0xD9]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn decode(bytes: &[u8]) -> Rgb {
        crate::image::decode_image(bytes, 64 << 20, 6000).unwrap()
    }

    #[test]
    fn zigzag_matches_the_standard_start_and_end() {
        let z = zigzag();
        assert_eq!(&z[..10], &[0, 1, 8, 16, 9, 2, 3, 10, 17, 24]);
        assert_eq!(&z[60..], &[47, 55, 62, 63]);
        let mut sorted = z.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..64).collect::<Vec<_>>());
    }

    #[test]
    fn round_trips_through_the_decoder() {
        // Quadrants of different colours, odd size (edge blocks padded).
        let (w, h) = (101u32, 67u32);
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let c = match (x < w / 2, y < h / 2) {
                    (true, true) => [220, 30, 30],
                    (false, true) => [30, 200, 40],
                    (true, false) => [20, 40, 210],
                    (false, false) => [240, 240, 240],
                };
                data.extend(c);
            }
        }
        let img = Rgb {
            width: w,
            height: h,
            data,
        };
        let jpg = encode_jpeg(&img, 85).unwrap();
        assert!(jpg.starts_with(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(jpg.ends_with(&[0xFF, 0xD9]));
        let back = decode(&jpg);
        assert_eq!((back.width, back.height), (w, h));
        let px = |x: u32, y: u32| {
            let at = ((y * w + x) * 3) as usize;
            [back.data[at], back.data[at + 1], back.data[at + 2]]
        };
        let close = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) < 24);
        assert!(close(px(10, 10), [220, 30, 30]), "{:?}", px(10, 10));
        assert!(close(px(90, 10), [30, 200, 40]), "{:?}", px(90, 10));
        assert!(close(px(10, 60), [20, 40, 210]), "{:?}", px(10, 60));
        assert!(close(px(90, 60), [240, 240, 240]), "{:?}", px(90, 60));
        // A flat image and a 1 × 1 image.
        let flat = Rgb {
            width: 16,
            height: 16,
            data: vec![128; 16 * 16 * 3],
        };
        assert_eq!(decode(&encode_jpeg(&flat, 85).unwrap()).data[0], 128);
        let one = Rgb {
            width: 1,
            height: 1,
            data: vec![0, 0, 0],
        };
        assert_eq!(decode(&encode_jpeg(&one, 85).unwrap()).width, 1);
    }

    #[test]
    fn noisy_pictures_use_long_codes_correctly() {
        // Pseudo-random pixels exercise many symbols and the length limit.
        let (w, h) = (64u32, 48u32);
        let mut seed = 12345u32;
        let data: Vec<u8> = (0..w * h * 3)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (seed >> 16) as u8
            })
            .collect();
        let img = Rgb {
            width: w,
            height: h,
            data,
        };
        for q in [10u8, 85, 100] {
            let back = decode(&encode_jpeg(&img, q).unwrap());
            assert_eq!((back.width, back.height), (w, h));
        }
    }
}
