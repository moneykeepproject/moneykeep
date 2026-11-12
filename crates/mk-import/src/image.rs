//! Logo images for document templates (business-dimensions.md Part C): PNG or JPEG
//! only, at most 512 KB and 2,000 × 2,000 pixels, fully decoded with pure-Rust decoders
//! (limits enforced before and during decoding) and re-encoded as a plain 8-bit RGB
//! raster. Nothing of the original file (metadata, profiles, comments) is kept.

use crate::{ImportError, ImportResult};

/// Largest logo file accepted.
pub const MAX_LOGO_BYTES: usize = 512 * 1024;
/// Largest logo width or height.
pub const MAX_LOGO_SIDE: u32 = 2000;

/// Largest width or height of a book icon picture (app-desktop.md revision 26):
/// phone photos are accepted, since icons are always scaled down to 128 × 128.
/// The size is checked from the file's header before any pixel buffer is made, so
/// decoding needs at most about `6,000² × 7` bytes.
pub const MAX_ICON_SIDE: u32 = 6000;

/// A decoded logo: 8-bit RGB, row-major, `width * height * 3` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgb {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

fn bad(m: impl Into<String>) -> ImportError {
    ImportError::Invalid(m.into())
}

fn too_big(w: u64, h: u64, side: u32) -> ImportError {
    bad(format!(
        "the image is {w} × {h} pixels; images are at most {side} × {side}"
    ))
}

/// Decode and re-encode a logo file.
pub fn decode_logo(bytes: &[u8]) -> ImportResult<Rgb> {
    decode_image(bytes, MAX_LOGO_BYTES, MAX_LOGO_SIDE)
}

/// Decode a PNG or JPEG file of at most `max_bytes` bytes and `max_side` pixels a
/// side into a plain RGB raster (the logo decoders and limits, reused for book icons,
/// app-desktop.md revision 26).
pub fn decode_image(bytes: &[u8], max_bytes: usize, max_side: u32) -> ImportResult<Rgb> {
    if bytes.len() > max_bytes {
        return Err(bad(format!(
            "the file is {} KB; images are at most {} KB",
            bytes.len().div_ceil(1024),
            max_bytes.div_ceil(1024)
        )));
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        decode_png(bytes, max_side)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        decode_jpeg(bytes, max_side)
    } else {
        Err(bad("images are PNG or JPEG files"))
    }
}

/// Centre-crop a raster to a square and scale it to `side` × `side` (each output
/// pixel is the average of the source pixels it covers; integer arithmetic only).
pub fn square_thumbnail(img: &Rgb, side: u32) -> ImportResult<Rgb> {
    let s = img.width.min(img.height);
    if s == 0 || side == 0 {
        return Err(bad("the image has no pixels"));
    }
    let (x0, y0) = ((img.width - s) / 2, (img.height - s) / 2);
    let (s64, side64) = (u64::from(s), u64::from(side));
    // Source span [start, end) along one axis for output index `o`.
    let span = |o: u64| {
        let start = o * s64 / side64;
        let end = ((o + 1) * s64 / side64).max(start + 1).min(s64);
        (start, end)
    };
    let width = usize::try_from(img.width).map_err(|_| bad("the image is too large"))?;
    let mut data = Vec::with_capacity(pixels(side, side)?.saturating_mul(3));
    for oy in 0..side64 {
        let (ys, ye) = span(oy);
        for ox in 0..side64 {
            let (xs, xe) = span(ox);
            let mut sum = [0u64; 3];
            let mut n = 0u64;
            for y in ys..ye {
                for x in xs..xe {
                    let at = usize::try_from(
                        (y + u64::from(y0)) * u64::try_from(width).unwrap_or(0) + x + u64::from(x0),
                    )
                    .map_err(|_| bad("the image is too large"))?
                    .saturating_mul(3);
                    let px = img
                        .data
                        .get(at..at.saturating_add(3))
                        .ok_or_else(|| bad("the image data is short"))?;
                    for (acc, v) in sum.iter_mut().zip(px) {
                        *acc += u64::from(*v);
                    }
                    n += 1;
                }
            }
            for acc in sum {
                data.push(u8::try_from((acc + n / 2) / n.max(1)).unwrap_or(255));
            }
        }
    }
    Ok(Rgb {
        width: side,
        height: side,
        data,
    })
}

/// Encode a raster as an 8-bit RGB PNG file (no metadata).
pub fn encode_png(img: &Rgb) -> ImportResult<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc
            .write_header()
            .map_err(|e| bad(format!("the PNG could not be written: {e}")))?;
        w.write_image_data(&img.data)
            .map_err(|e| bad(format!("the PNG could not be written: {e}")))?;
    }
    Ok(out)
}

fn decode_png(bytes: &[u8], max_side: u32) -> ImportResult<Rgb> {
    let limit = usize::try_from(max_side).unwrap_or(2000);
    let mut d = png::Decoder::new_with_limits(
        std::io::Cursor::new(bytes),
        png::Limits {
            // RGBA at 16 bits per sample at the largest size, plus working room.
            // The decoder's own buffers (rows, chunks); the pixel buffer is made
            // below, after the header's size is checked.
            bytes: limit
                .saturating_mul(limit)
                .saturating_mul(8)
                .saturating_add(1 << 20)
                .min(64 << 20),
        },
    );
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = d
        .read_info()
        .map_err(|e| bad(format!("the PNG could not be read: {e}")))?;
    let (w, h, animated) = {
        let info = reader.info();
        (info.width, info.height, info.animation_control.is_some())
    };
    if w > max_side || h > max_side {
        return Err(too_big(u64::from(w), u64::from(h), max_side));
    }
    if w == 0 || h == 0 {
        return Err(bad("the image has no pixels"));
    }
    if animated {
        return Err(bad("animated PNG images are not accepted"));
    }
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| bad("the PNG is too large"))?;
    let mut buf = vec![0; size];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| bad(format!("the PNG could not be decoded: {e}")))?;
    let (color, depth) = (frame.color_type, frame.bit_depth);
    if depth != png::BitDepth::Eight {
        return Err(bad("unsupported PNG bit depth"));
    }
    let channels = match color {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err(bad("unsupported PNG colour type")),
    };
    let line = frame.line_size;
    let mut out = Vec::with_capacity(pixels(w, h)?.saturating_mul(3));
    for row in 0..usize::try_from(h).unwrap_or(0) {
        let start = row.saturating_mul(line);
        let px = buf
            .get(start..start.saturating_add(line))
            .ok_or_else(|| bad("the PNG data is short"))?;
        for p in px
            .chunks_exact(channels)
            .take(usize::try_from(w).unwrap_or(0))
        {
            let (r, g, b, a) = match p {
                [v] => (*v, *v, *v, 255),
                [v, a] => (*v, *v, *v, *a),
                [r, g, b] => (*r, *g, *b, 255),
                [r, g, b, a] => (*r, *g, *b, *a),
                _ => return Err(bad("the PNG data is malformed")),
            };
            // Transparent parts become white paper.
            out.extend([over_white(r, a), over_white(g, a), over_white(b, a)]);
        }
    }
    if out.len() != pixels(w, h)?.saturating_mul(3) {
        return Err(bad("the PNG data is short"));
    }
    Ok(Rgb {
        width: w,
        height: h,
        data: out,
    })
}

fn over_white(c: u8, a: u8) -> u8 {
    let (c, a) = (u32::from(c), u32::from(a));
    u8::try_from((c * a + 255 * (255 - a) + 127) / 255).unwrap_or(255)
}

fn pixels(w: u32, h: u32) -> ImportResult<usize> {
    usize::try_from(u64::from(w) * u64::from(h)).map_err(|_| bad("the image is too large"))
}

fn decode_jpeg(bytes: &[u8], max_side: u32) -> ImportResult<Rgb> {
    use zune_jpeg::zune_core::bytestream::ZCursor;
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let side = usize::try_from(max_side).unwrap_or(2000);
    let options = DecoderOptions::default()
        .set_max_width(side)
        .set_max_height(side)
        .jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    d.decode_headers().map_err(|e| {
        let text = format!("{e:?}");
        if text.contains("width") || text.contains("height") || text.contains("Large") {
            bad(format!(
                "the image is larger than {max_side} × {max_side} pixels"
            ))
        } else {
            bad(format!("the JPEG could not be read: {text}"))
        }
    })?;
    let (w, h) = d.dimensions().ok_or_else(|| bad("the JPEG has no size"))?;
    let (w, h) = (
        u32::try_from(w).map_err(|_| bad("the image is too large"))?,
        u32::try_from(h).map_err(|_| bad("the image is too large"))?,
    );
    if w > max_side || h > max_side {
        return Err(too_big(u64::from(w), u64::from(h), max_side));
    }
    if w == 0 || h == 0 {
        return Err(bad("the image has no pixels"));
    }
    let data = d
        .decode()
        .map_err(|e| bad(format!("the JPEG could not be decoded: {e:?}")))?;
    if data.len() != pixels(w, h)?.saturating_mul(3) {
        return Err(bad("the JPEG did not decode to RGB"));
    }
    Ok(Rgb {
        width: w,
        height: h,
        data,
    })
}

/// zlib-compress a raster (the PDF FlateDecode filter and the stored form).
pub fn deflate(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
}

/// Undo [`deflate`], refusing output larger than `max` bytes.
pub fn inflate(data: &[u8], max: usize) -> ImportResult<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, max)
        .map_err(|_| bad("the stored logo is damaged"))
}

/// A PNG file of a solid colour (tests and examples).
pub fn solid_png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        if let Ok(mut w) = enc.write_header() {
            let px = usize::try_from(u64::from(width) * u64::from(height)).unwrap_or(0);
            let data: Vec<u8> = std::iter::repeat_n(rgb, px).flatten().collect();
            let _ = w.write_image_data(&data);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn png_round_trip_and_limits() {
        let png = solid_png(300, 100, [0x1F, 0x6F, 0x78]);
        let rgb = decode_logo(&png).unwrap();
        assert_eq!((rgb.width, rgb.height), (300, 100));
        assert_eq!(&rgb.data[..3], &[0x1F, 0x6F, 0x78]);
        let big = solid_png(3000, 3000, [0, 0, 0]);
        assert!(decode_logo(&big).unwrap_err().to_string().contains("2000"));
        assert!(decode_logo(b"GIF89a....").is_err());
        let mut corrupt = png.clone();
        let n = corrupt.len();
        for b in corrupt.iter_mut().skip(n / 2).take(20) {
            *b ^= 0xFF;
        }
        assert!(decode_logo(&corrupt).is_err());
        let huge = vec![0u8; 600 * 1024];
        assert!(
            decode_logo(&huge)
                .unwrap_err()
                .to_string()
                .contains("512 KB")
        );
        let packed = deflate(&rgb.data);
        // Book icons: a 2 MB limit, centre crop and scale to 128 × 128.
        let wide = decode_image(&solid_png(400, 300, [10, 20, 30]), 2 << 20, 2000).unwrap();
        let t = square_thumbnail(&wide, 128).unwrap();
        assert_eq!((t.width, t.height, t.data.len()), (128, 128, 128 * 128 * 3));
        assert_eq!(&t.data[..3], &[10, 20, 30]);
        let back = decode_logo(&encode_png(&t).unwrap()).unwrap();
        assert_eq!(back, t);
        let tiny = square_thumbnail(&decode_logo(&solid_png(3, 5, [1, 2, 3])).unwrap(), 128);
        assert_eq!(tiny.unwrap().data.len(), 128 * 128 * 3);
        assert_eq!(inflate(&packed, 1 << 20).unwrap(), rgb.data);
    }
}
