//! The phone app's icons (mobile-capture.md revision 3): MoneyKeep's calculator
//! drawing (app-desktop.md revision 25) at the sizes browsers need to install it,
//! written into apps/moneykeep-capture/web/icons. No third-party artwork.
//!
//! `cargo xtask capture-icons` regenerates the files (`MK_UPDATE_ICONS=1`); in CI
//! this test checks the committed files match the drawing.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

/// The page's background and theme colours (index.html, manifest.webmanifest).
const PAGE: [u8; 3] = [0xf6, 0xf6, 0xf2];
const THEME: [u8; 3] = [0x1f, 0x6f, 0x5c];

fn png(side: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, side, side);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(rgba).unwrap();
    }
    out
}

/// (file, side, PNG bytes)
fn icons() -> Vec<(&'static str, u32, Vec<u8>)> {
    vec![
        ("icon-192.png", 192, png(192, &mk_ui::app_icon_rgba(192))),
        ("icon-512.png", 512, png(512, &mk_ui::app_icon_rgba(512))),
        // Maskable: the drawing inside the central 80 % safe zone, full bleed.
        (
            "icon-512-maskable.png",
            512,
            png(512, &mk_ui::app_icon_on(512, 320, THEME)),
        ),
        // iPhone/iPad home screens show no transparency.
        (
            "apple-touch-icon.png",
            180,
            png(180, &mk_ui::app_icon_on(180, 160, PAGE)),
        ),
    ]
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/moneykeep-capture/web/icons")
}

#[test]
fn capture_icons_match_the_drawing() {
    let update = std::env::var_os("MK_UPDATE_ICONS").is_some();
    for (name, side, bytes) in icons() {
        let path = dir().join(name);
        if update {
            std::fs::create_dir_all(dir()).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            continue;
        }
        let committed = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("{}: {e} (run cargo xtask capture-icons)", path.display()));
        assert!(
            committed == bytes,
            "{name} differs from the drawing (run cargo xtask capture-icons)"
        );
        // mobile-capture.md AT31: the icons decode as PNG at their sizes.
        let decoder = png::Decoder::new(std::io::Cursor::new(committed));
        let reader = decoder.read_info().unwrap();
        assert_eq!(
            (reader.info().width, reader.info().height),
            (side, side),
            "{name}"
        );
    }
}
