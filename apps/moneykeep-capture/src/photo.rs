//! Photo preparation rules (mobile-capture.md "Before upload"): the longer side is
//! scaled to at most 2,400 pixels and the picture re-encoded as JPEG at quality 0.85
//! through a canvas (which also drops all metadata); HEIC and other types the
//! browser cannot draw are refused with a message. The canvas work itself is in the
//! browser layer; the decisions are here so they can be tested natively.

use mk_capture_api::{MAX_PHOTO_SIDE, fit_within, msg};

/// JPEG quality passed to `canvas.toBlob` (HTML Living Standard: 0.0–1.0).
pub const JPEG_QUALITY: f64 = 0.85;
/// Thumbnails in the photo strip.
pub const THUMB_SIDE: u32 = 96;

/// The size a photo is drawn at before upload.
pub fn upload_size(w: u32, h: u32) -> (u32, u32) {
    fit_within(w, h, MAX_PHOTO_SIDE)
}

/// The thumbnail size.
pub fn thumb_size(w: u32, h: u32) -> (u32, u32) {
    fit_within(w, h, THUMB_SIDE)
}

/// Whether a picked file can be prepared, from its type, name and first bytes.
pub fn check_type(mime: &str, name: &str, head: &[u8]) -> Result<(), &'static str> {
    let mime = mime.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let heic_magic = head.get(4..8) == Some(b"ftyp")
        && head.get(8..12).is_some_and(|b| {
            matches!(
                b,
                b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"mif1" | b"msf1"
            )
        });
    if heic_magic
        || mime.contains("heic")
        || mime.contains("heif")
        || name.ends_with(".heic")
        || name.ends_with(".heif")
    {
        return Err(msg::PHOTO_TYPE);
    }
    let jpeg = head.starts_with(&[0xFF, 0xD8, 0xFF]);
    let png = head.starts_with(&[0x89, b'P', b'N', b'G']);
    let webp = head.get(0..4) == Some(b"RIFF") && head.get(8..12) == Some(b"WEBP");
    if jpeg || png || webp || matches!(mime.as_str(), "image/jpeg" | "image/png" | "image/webp") {
        Ok(())
    } else {
        Err(msg::PHOTO_TYPE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mobile-capture.md AT16 (client side): ≤ 2,400 px on the longer side, aspect
    /// kept.
    #[test]
    fn at16_upload_size() {
        assert_eq!(upload_size(4032, 3024), (2400, 1800));
        assert_eq!(upload_size(3024, 4032), (1800, 2400));
        assert_eq!(upload_size(1000, 800), (1000, 800));
        assert_eq!(upload_size(9000, 3000), (2400, 800));
        assert_eq!(thumb_size(2400, 1800), (96, 72));
    }

    #[test]
    fn types() {
        assert!(check_type("image/jpeg", "a.jpg", &[0xFF, 0xD8, 0xFF, 0xE0]).is_ok());
        assert!(check_type("", "a.png", &[0x89, b'P', b'N', b'G']).is_ok());
        assert_eq!(
            check_type("image/heic", "IMG_1.HEIC", b"\0\0\0\x18ftypheic"),
            Err(msg::PHOTO_TYPE)
        );
        assert_eq!(
            check_type("", "x", b"\0\0\0\x18ftypmif1"),
            Err(msg::PHOTO_TYPE)
        );
        assert!(check_type("application/pdf", "a.pdf", b"%PDF-").is_err());
    }
}
