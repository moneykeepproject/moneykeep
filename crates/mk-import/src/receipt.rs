//! Receipt files (expenses.md): PNG or JPEG pictures and PDF documents, checked and
//! normalised before they are stored in a book.
//!
//! Pictures are fully decoded with the safe decoders of [`crate::image`] (size limits
//! checked from the header before any pixel buffer is made), turned upright by their
//! EXIF orientation, scaled so the longer side is at most 2,400 pixels and written
//! again from the pixels alone: JPEG at quality 85, PNG as PNG. Nothing else of the
//! original file survives — no EXIF (and so no GPS position), comments or profiles.
//! A 256-pixel PNG thumbnail is made too. PDFs are kept as given after a structure
//! check; they are never rendered or run.
//!
//! The EXIF orientation is read by a small bounded parser written from the EXIF
//! (CIPA DC-008) and TIFF 6.0 specifications: only IFD0's Orientation tag is looked
//! at.

use crate::image::{self, Rgb};
use crate::{ImportError, ImportResult};

/// Largest receipt file.
pub const MAX_BYTES: usize = 10 * 1024 * 1024;
/// Largest picture width or height accepted.
pub const MAX_SIDE: u32 = 6000;
/// Stored pictures are at most this many pixels on the longer side.
pub const STORED_SIDE: u32 = 2400;
/// Thumbnails are at most this many pixels on the longer side.
pub const THUMB_SIDE: u32 = 256;
/// Most pages a PDF receipt may have.
pub const MAX_PDF_PAGES: usize = 200;
/// JPEG quality of stored photos.
pub const JPEG_QUALITY: u8 = 85;

pub const MSG_TOO_BIG: &str = "Receipts can be at most 10 MB.";
pub const MSG_PICTURE: &str = "This picture can't be read.";
pub const MSG_HEIC: &str = "Save the photo as JPEG and try again.";
pub const MSG_PDF: &str = "This PDF can't be read.";
pub const MSG_PDF_PAGES: &str = "PDF receipts can have at most 200 pages.";
pub const MSG_KIND: &str = "Receipts are PNG or JPEG pictures, or PDF files.";
pub const MSG_PIXELS: &str = "This picture is larger than 6,000 pixels on its longer side.";

/// What a stored receipt is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Jpeg,
    Png,
    Pdf,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Jpeg => "jpeg",
            Kind::Png => "png",
            Kind::Pdf => "pdf",
        }
    }
}

/// A receipt ready to store.
#[derive(Debug, Clone)]
pub struct Processed {
    pub kind: Kind,
    /// The file to store (re-encoded picture, or the PDF as given).
    pub content: Vec<u8>,
    /// A PNG thumbnail (pictures only).
    pub thumbnail_png: Option<Vec<u8>>,
    /// Size of the stored picture.
    pub width: Option<u32>,
    pub height: Option<u32>,
}

fn refuse(m: &str) -> ImportError {
    ImportError::Invalid(m.to_owned())
}

fn ext(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_heic(bytes: &[u8]) -> bool {
    bytes.get(4..8) == Some(b"ftyp")
        && bytes.get(8..12).is_some_and(|b| {
            matches!(
                b,
                b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1" | b"msf1"
            )
        })
}

/// Check and normalise a receipt file. Every refusal carries one of the `MSG_*`
/// texts.
pub fn process(bytes: &[u8], file_name: &str) -> ImportResult<Processed> {
    if bytes.len() > MAX_BYTES {
        return Err(refuse(MSG_TOO_BIG));
    }
    let e = ext(file_name);
    if bytes.starts_with(b"%PDF-") {
        return process_pdf(bytes);
    }
    if e == "pdf" {
        return Err(refuse(MSG_PDF));
    }
    if is_heic(bytes) || matches!(e.as_str(), "heic" | "heif") {
        return Err(refuse(MSG_HEIC));
    }
    let png = bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let jpeg = bytes.starts_with(&[0xFF, 0xD8, 0xFF]);
    if !png && !jpeg {
        return Err(refuse(
            if matches!(e.as_str(), "jpg" | "jpeg" | "png" | "jpe") {
                MSG_PICTURE
            } else {
                MSG_KIND
            },
        ));
    }
    let rgb = image::decode_image(bytes, MAX_BYTES, MAX_SIDE).map_err(|err| {
        if err.to_string().contains("pixels") && !err.to_string().contains("no pixels") {
            refuse(MSG_PIXELS)
        } else {
            refuse(MSG_PICTURE)
        }
    })?;
    let orientation = if jpeg { exif_orientation(bytes) } else { 1 };
    // Scale first (fewer pixels to turn), then turn upright.
    let (tw, th) = fit(rgb.width, rgb.height, STORED_SIDE);
    let scaled = if (tw, th) == (rgb.width, rgb.height) {
        rgb
    } else {
        resize(&rgb, tw, th).map_err(|_| refuse(MSG_PICTURE))?
    };
    let upright = orient(&scaled, orientation).map_err(|_| refuse(MSG_PICTURE))?;
    drop(scaled);
    let (kind, content) = if jpeg {
        (
            Kind::Jpeg,
            crate::jpeg::encode_jpeg(&upright, JPEG_QUALITY).map_err(|_| refuse(MSG_PICTURE))?,
        )
    } else {
        (
            Kind::Png,
            image::encode_png(&upright).map_err(|_| refuse(MSG_PICTURE))?,
        )
    };
    let thumbnail_png = Some(thumbnail(&upright).map_err(|_| refuse(MSG_PICTURE))?);
    Ok(Processed {
        kind,
        content,
        thumbnail_png,
        width: Some(upright.width),
        height: Some(upright.height),
    })
}

/// A PNG of the picture fitted within `side` pixels (never enlarged).
pub fn fitted_png(img: &Rgb, side: u32) -> ImportResult<Vec<u8>> {
    let (w, h) = fit(img.width, img.height, side);
    if (w, h) == (img.width, img.height) {
        return image::encode_png(img);
    }
    image::encode_png(&resize(img, w, h)?)
}

fn thumbnail(img: &Rgb) -> ImportResult<Vec<u8>> {
    fitted_png(img, THUMB_SIDE)
}

/// Decode a stored receipt picture (PNG or JPEG written by [`process`]).
pub fn decode_stored(bytes: &[u8]) -> ImportResult<Rgb> {
    image::decode_image(bytes, MAX_BYTES * 2, MAX_SIDE)
}

/// The PDF structure check: the `%PDF-` header and at most 200 page objects.
fn process_pdf(bytes: &[u8]) -> ImportResult<Processed> {
    let pages = count_pages(bytes);
    if pages > MAX_PDF_PAGES {
        return Err(refuse(MSG_PDF_PAGES));
    }
    Ok(Processed {
        kind: Kind::Pdf,
        content: bytes.to_vec(),
        thumbnail_png: None,
        width: None,
        height: None,
    })
}

/// Count `/Type /Page` entries (not `/Pages`), whitespace allowed after `/Type`.
fn count_pages(bytes: &[u8]) -> usize {
    let mut n = 0usize;
    let mut i = 0usize;
    while let Some(rest) = bytes.get(i..) {
        let Some(at) = find(rest, b"/Type") else {
            break;
        };
        let mut j = i + at + 5;
        while bytes
            .get(j)
            .is_some_and(|b| matches!(b, b' ' | b'\r' | b'\n' | b'\t' | 0x0C | 0))
        {
            j += 1;
        }
        if bytes.get(j..j + 5) == Some(b"/Page")
            && !bytes.get(j + 5).is_some_and(|b| b.is_ascii_alphanumeric())
        {
            n += 1;
            if n > MAX_PDF_PAGES {
                break;
            }
        }
        i = j.max(i + at + 1);
    }
    n
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Width and height fitted within `side` on the longer side (never enlarged).
pub fn fit(w: u32, h: u32, side: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= side || long == 0 {
        return (w, h);
    }
    let scale = |v: u32| {
        let s = (u64::from(v) * u64::from(side) + u64::from(long) / 2) / u64::from(long);
        u32::try_from(s.max(1)).unwrap_or(1)
    };
    (scale(w), scale(h))
}

/// Scale a raster to `w` × `h` (each output pixel is the average of the source
/// pixels it covers; integer arithmetic only).
pub fn resize(img: &Rgb, w: u32, h: u32) -> ImportResult<Rgb> {
    if img.width == 0 || img.height == 0 || w == 0 || h == 0 {
        return Err(ImportError::Invalid("the image has no pixels".into()));
    }
    let (sw, sh) = (u64::from(img.width), u64::from(img.height));
    let (dw, dh) = (u64::from(w), u64::from(h));
    let span = |o: u64, s: u64, d: u64| {
        let start = o * s / d;
        let end = ((o + 1) * s / d).max(start + 1).min(s);
        (start, end)
    };
    let xs: Vec<(u64, u64)> = (0..dw).map(|o| span(o, sw, dw)).collect();
    let row = usize::try_from(sw * 3).map_err(|_| ImportError::Invalid("too large".into()))?;
    let mut data = Vec::with_capacity(
        usize::try_from(dw * dh * 3).map_err(|_| ImportError::Invalid("too large".into()))?,
    );
    let mut acc: Vec<[u64; 3]> = vec![[0; 3]; xs.len()];
    for oy in 0..dh {
        let (ys, ye) = span(oy, sh, dh);
        for a in acc.iter_mut() {
            *a = [0; 3];
        }
        for y in ys..ye {
            let start = usize::try_from(y).unwrap_or(0).saturating_mul(row);
            let line = img
                .data
                .get(start..start.saturating_add(row))
                .ok_or_else(|| ImportError::Invalid("the image data is short".into()))?;
            for (a, (x0, x1)) in acc.iter_mut().zip(&xs) {
                let from = usize::try_from(*x0).unwrap_or(0) * 3;
                let to = usize::try_from(*x1).unwrap_or(0) * 3;
                for px in line.get(from..to).unwrap_or(&[]).chunks_exact(3) {
                    for (s, v) in a.iter_mut().zip(px) {
                        *s += u64::from(*v);
                    }
                }
            }
        }
        for (a, (x0, x1)) in acc.iter().zip(&xs) {
            let n = (x1 - x0) * (ye - ys);
            for s in a {
                data.push(u8::try_from((s + n / 2) / n.max(1)).unwrap_or(255));
            }
        }
    }
    Ok(Rgb {
        width: w,
        height: h,
        data,
    })
}

/// Turn a raster as EXIF orientation `o` (1–8) asks, so it displays upright.
pub fn orient(img: &Rgb, o: u16) -> ImportResult<Rgb> {
    if o <= 1 || o > 8 {
        return Ok(img.clone());
    }
    let (w, h) = (img.width, img.height);
    let (ow, oh) = if o >= 5 { (h, w) } else { (w, h) };
    let (wu, hu) = (
        usize::try_from(w).map_err(|_| ImportError::Invalid("too large".into()))?,
        usize::try_from(h).map_err(|_| ImportError::Invalid("too large".into()))?,
    );
    let (owu, ohu) = if o >= 5 { (hu, wu) } else { (wu, hu) };
    let mut data = Vec::with_capacity(img.data.len());
    for y in 0..ohu {
        for x in 0..owu {
            let (sx, sy) = match o {
                2 => (wu - 1 - x, y),
                3 => (wu - 1 - x, hu - 1 - y),
                4 => (x, hu - 1 - y),
                5 => (y, x),
                6 => (y, hu - 1 - x),
                7 => (wu - 1 - y, hu - 1 - x),
                _ => (wu - 1 - y, x),
            };
            let at = (sy * wu + sx) * 3;
            let px = img
                .data
                .get(at..at + 3)
                .ok_or_else(|| ImportError::Invalid("the image data is short".into()))?;
            data.extend_from_slice(px);
        }
    }
    Ok(Rgb {
        width: ow,
        height: oh,
        data,
    })
}

/// The EXIF orientation of a JPEG file (1 when absent or unreadable).
pub fn exif_orientation(bytes: &[u8]) -> u16 {
    exif_tiff(bytes).and_then(tiff_orientation).unwrap_or(1)
}

/// The TIFF structure inside the first `Exif` APP1 segment, if any.
fn exif_tiff(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.get(0..2) != Some(&[0xFF, 0xD8]) {
        return None;
    }
    let mut i = 2usize;
    // A bounded walk over the segments before the image data.
    for _ in 0..256 {
        if bytes.get(i) != Some(&0xFF) {
            return None;
        }
        let mut m = *bytes.get(i + 1)?;
        while m == 0xFF {
            i += 1;
            m = *bytes.get(i + 1)?;
        }
        i += 2;
        if m == 0xD9 || m == 0xDA {
            return None;
        }
        if m == 0x01 || (0xD0..=0xD7).contains(&m) {
            continue;
        }
        let len = usize::from(u16::from_be_bytes([*bytes.get(i)?, *bytes.get(i + 1)?]));
        if len < 2 {
            return None;
        }
        let body = bytes.get(i + 2..i + len)?;
        if m == 0xE1 && body.starts_with(b"Exif\0\0") {
            return body.get(6..);
        }
        i += len;
    }
    None
}

fn tiff_orientation(t: &[u8]) -> Option<u16> {
    let little = match t.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b = [*t.get(at)?, *t.get(at + 1)?];
        Some(if little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b = [
            *t.get(at)?,
            *t.get(at + 1)?,
            *t.get(at + 2)?,
            *t.get(at + 3)?,
        ];
        Some(if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    let count = usize::from(u16_at(ifd)?);
    for k in 0..count.min(512) {
        let e = ifd + 2 + k * 12;
        if u16_at(e)? == 0x0112 {
            // SHORT, count 1: the value sits in the first two bytes of the field.
            if u16_at(e + 2)? != 3 {
                return None;
            }
            let v = u16_at(e + 8)?;
            return (1..=8).contains(&v).then_some(v);
        }
    }
    None
}

/// Test support: a JPEG with an EXIF APP1 segment (orientation and a GPS IFD with a
/// latitude) inserted after SOI.
pub fn with_test_exif(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    // Big-endian TIFF: IFD0 with Orientation and a GPS IFD pointer; the GPS IFD
    // holds GPSLatitudeRef "N" and a marker string.
    let mut t: Vec<u8> = b"MM\0\x2A\0\0\0\x08".to_vec();
    // IFD0 at 8: 2 entries.
    t.extend(2u16.to_be_bytes());
    t.extend(0x0112u16.to_be_bytes());
    t.extend(3u16.to_be_bytes());
    t.extend(1u32.to_be_bytes());
    t.extend(orientation.to_be_bytes());
    t.extend([0, 0]);
    t.extend(0x8825u16.to_be_bytes());
    t.extend(4u16.to_be_bytes());
    t.extend(1u32.to_be_bytes());
    let gps_at = 8 + 2 + 2 * 12 + 4;
    t.extend(u32::try_from(gps_at).unwrap_or(0).to_be_bytes());
    t.extend(0u32.to_be_bytes());
    // GPS IFD: GPSLatitudeRef = "N", GPSAreaInformation pointing at "GPSTESTMARK".
    t.extend(2u16.to_be_bytes());
    t.extend(0x0001u16.to_be_bytes());
    t.extend(2u16.to_be_bytes());
    t.extend(2u32.to_be_bytes());
    t.extend([b'N', 0, 0, 0]);
    t.extend(0x001Cu16.to_be_bytes());
    t.extend(7u16.to_be_bytes());
    t.extend(11u32.to_be_bytes());
    let text_at = gps_at + 2 + 2 * 12 + 4;
    t.extend(u32::try_from(text_at).unwrap_or(0).to_be_bytes());
    t.extend(0u32.to_be_bytes());
    t.extend_from_slice(b"GPSTESTMARK");
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend(t);
    let len = u16::try_from(app1.len() + 2).unwrap_or(0);
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend(len.to_be_bytes());
    out.extend(app1);
    out.extend_from_slice(jpeg.get(2..).unwrap_or(&[]));
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn quadrants(w: u32, h: u32) -> Rgb {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.extend(match (x < w / 2, y < h / 2) {
                    (true, true) => [255, 0, 0],
                    (false, true) => [0, 255, 0],
                    (true, false) => [0, 0, 255],
                    (false, false) => [255, 255, 255],
                });
            }
        }
        Rgb {
            width: w,
            height: h,
            data,
        }
    }

    fn px(img: &Rgb, x: u32, y: u32) -> [u8; 3] {
        let at = ((y * img.width + x) * 3) as usize;
        [img.data[at], img.data[at + 1], img.data[at + 2]]
    }

    #[test]
    fn orientations_turn_the_picture_upright() {
        let img = quadrants(4, 2);
        let red = [255, 0, 0];
        // Where the source's top-left (red) ends up.
        for (o, (w, h), (x, y)) in [
            (1u16, (4u32, 2u32), (0u32, 0u32)),
            (2, (4, 2), (3, 0)),
            (3, (4, 2), (3, 1)),
            (4, (4, 2), (0, 1)),
            (5, (2, 4), (0, 0)),
            (6, (2, 4), (1, 0)),
            (7, (2, 4), (1, 3)),
            (8, (2, 4), (0, 3)),
        ] {
            let r = orient(&img, o).unwrap();
            assert_eq!((r.width, r.height), (w, h), "orientation {o}");
            assert_eq!(px(&r, x, y), red, "orientation {o}");
        }
    }

    #[test]
    fn exif_orientation_is_read_and_dropped() {
        let jpg = crate::jpeg::encode_jpeg(&quadrants(64, 32), 85).unwrap();
        assert_eq!(exif_orientation(&jpg), 1);
        let tagged = with_test_exif(&jpg, 6);
        assert_eq!(exif_orientation(&tagged), 6);
        let p = process(&tagged, "photo.jpg").unwrap();
        assert_eq!(p.kind, Kind::Jpeg);
        assert_eq!((p.width, p.height), (Some(32), Some(64)));
        assert!(find(&p.content, b"Exif").is_none());
        assert!(find(&p.content, b"GPSTESTMARK").is_none());
    }

    #[test]
    fn refusals() {
        let msg = |r: ImportResult<Processed>| r.unwrap_err().to_string();
        assert!(msg(process(&vec![0u8; MAX_BYTES + 1], "big.jpg")).contains(MSG_TOO_BIG));
        assert!(msg(process(b"hello, not a picture", "fake.jpg")).contains(MSG_PICTURE));
        assert!(msg(process(b"<html>", "page.pdf")).contains(MSG_PDF));
        assert!(msg(process(b"\0\0\0\x18ftypheic....", "IMG_1.HEIC")).contains(MSG_HEIC));
        assert!(msg(process(b"plain", "notes.txt")).contains(MSG_KIND));
        // A truncated JPEG.
        let jpg = crate::jpeg::encode_jpeg(&quadrants(64, 32), 85).unwrap();
        assert!(msg(process(&jpg[..40], "cut.jpg")).contains(MSG_PICTURE));
        let mut pdf = b"%PDF-1.4\n".to_vec();
        for _ in 0..201 {
            pdf.extend_from_slice(b"<< /Type /Page >>\n");
        }
        assert!(msg(process(&pdf, "many.pdf")).contains(MSG_PDF_PAGES));
        let ok = process(
            b"%PDF-1.4\n<< /Type /Pages /Count 1 >> << /Type/Page >>",
            "a.pdf",
        )
        .unwrap();
        assert_eq!(ok.kind, Kind::Pdf);
        assert_eq!(count_pages(b"<< /Type /Pages >> << /Type/Page >>"), 1);
    }

    #[test]
    fn png_stays_png_and_small_pictures_are_not_enlarged() {
        let png = image::encode_png(&quadrants(300, 100)).unwrap();
        let p = process(&png, "scan.png").unwrap();
        assert_eq!(p.kind, Kind::Png);
        assert_eq!((p.width, p.height), (Some(300), Some(100)));
        let thumb = image::decode_logo(p.thumbnail_png.as_ref().unwrap()).unwrap();
        assert_eq!((thumb.width, thumb.height), (256, 85));
        assert_eq!(fit(4000, 3000, 2400), (2400, 1800));
        assert_eq!(fit(10, 5, 2400), (10, 5));
    }
}
