//! release-packaging.md: the setup's, uninstaller's and shortcuts' icon, a Windows
//! `.ico` built from MoneyKeep's calculator drawing at build time (never committed).
//!
//! `cargo xtask installer` runs this test with `MK_ICO_OUT=<file>` to write it; the
//! test always checks the file's structure.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

#[test]
fn app_ico_has_every_size() {
    let ico = mk_ui::app_icon_ico();
    if let Some(out) = std::env::var_os("MK_ICO_OUT") {
        let out = std::path::PathBuf::from(out);
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(&out, &ico).unwrap();
    }
    assert_eq!(u16_at(&ico, 0), 0, "reserved");
    assert_eq!(u16_at(&ico, 2), 1, "an icon");
    let count = usize::from(u16_at(&ico, 4));
    assert_eq!(count, mk_ui::ICO_SIZES.len());
    for (k, side) in mk_ui::ICO_SIZES.iter().enumerate() {
        let e = 6 + 16 * k;
        let dim = if *side == 256 { 0 } else { *side as u8 };
        assert_eq!((ico[e], ico[e + 1]), (dim, dim), "size {side}");
        assert_eq!(u16_at(&ico, e + 6), 32, "32 bits per pixel");
        let len = u32_at(&ico, e + 8) as usize;
        let at = u32_at(&ico, e + 12) as usize;
        assert!(at + len <= ico.len(), "image {side} inside the file");
        let img = &ico[at..at + len];
        if *side == 256 {
            assert_eq!(&img[..8], b"\x89PNG\r\n\x1a\n", "PNG at 256");
            let reader = png::Decoder::new(std::io::Cursor::new(img.to_vec()))
                .read_info()
                .unwrap();
            assert_eq!((reader.info().width, reader.info().height), (256, 256));
        } else {
            assert_eq!(u32_at(img, 0), 40, "BITMAPINFOHEADER");
            assert_eq!(u32_at(img, 4), *side);
            assert_eq!(u32_at(img, 8), side * 2, "colour rows and mask");
            let mask_row = (*side as usize).div_ceil(32) * 4;
            assert_eq!(
                len,
                40 + (*side as usize).pow(2) * 4 + mask_row * *side as usize
            );
        }
    }
}

/// macos-release.md P2: the app bundle's icon, a macOS `.icns` from the same drawing.
/// `packaging/macos/package.sh` runs this test with `MK_ICNS_OUT=<file>` to write it;
/// the test always checks the file's structure.
#[test]
fn app_icns_has_every_size() {
    let icns = mk_ui::app_icon_icns();
    if let Some(out) = std::env::var_os("MK_ICNS_OUT") {
        let out = std::path::PathBuf::from(out);
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(&out, &icns).unwrap();
    }
    let be32 = |i: usize| u32::from_be_bytes([icns[i], icns[i + 1], icns[i + 2], icns[i + 3]]);
    assert_eq!(&icns[..4], b"icns");
    assert_eq!(be32(4) as usize, icns.len(), "the file's length");
    let mut at = 8;
    for (kind, side) in mk_ui::ICNS_ENTRIES {
        assert_eq!(&icns[at..at + 4], &kind, "entry {side}");
        let len = be32(at + 4) as usize;
        let png = &icns[at + 8..at + len];
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG in {kind:?}");
        let reader = png::Decoder::new(std::io::Cursor::new(png.to_vec()))
            .read_info()
            .unwrap();
        assert_eq!((reader.info().width, reader.info().height), (side, side));
        at += len;
    }
    assert_eq!(at, icns.len(), "nothing after the last entry");
}
