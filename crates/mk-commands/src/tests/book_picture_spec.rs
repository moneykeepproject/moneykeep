//! app-desktop.md revision 40: taglines, preset pictures and the engraving treatment
//! at the command level (acceptance tests 222–226 and 228).

use base64::Engine;

use super::*;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(v.as_str().unwrap())
        .unwrap()
}

/// RGBA pixels of a PNG.
fn rgba(png: &[u8]) -> (u32, Vec<u8>) {
    let mut d = png::Decoder::new(std::io::Cursor::new(png));
    d.set_transformations(png::Transformations::EXPAND);
    let mut r = d.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let f = r.next_frame(&mut buf).unwrap();
    assert_eq!(f.color_type, png::ColorType::Rgba);
    buf.truncate(f.buffer_size());
    (f.width, buf)
}

/// The test photo: a dark figure on a plain light background, 600 × 600.
fn photo() -> String {
    let rgb = mk_import::engrave::test_photo(600);
    b64(&mk_import::image::encode_png(&rgb).unwrap())
}

fn whole() -> Value {
    json!({"x": 0, "y": 0, "size": 600})
}

#[test]
fn app_desktop_at222_tagline() {
    let mut b = Book::new();
    let n = b.audit_len();
    let out = b.run(
        "book.update_profile",
        json!({"tagline": "Weekly\nofferings"}),
    );
    assert_eq!(out["tagline"], "Weekly offerings", "one line");
    assert_eq!(b.audit_len(), n + 1, "one undo step");
    let e = b.err("book.update_profile", json!({"tagline": "t".repeat(61)}));
    assert_eq!(
        e.to_string(),
        "The tagline can be at most 60 characters (61 now)."
    );
    b.run("book.update_profile", json!({"tagline": "t".repeat(60)}));
    b.run("history.undo", json!({}));
    assert_eq!(b.run("book.info", json!({}))["tagline"], "Weekly offerings");
}

#[test]
fn app_desktop_at223_preset_picture() {
    let mut b = Book::new();
    let out = b.run(
        "book.update_profile",
        json!({"icon": {"kind": "preset", "preset": "baker"}, "color": "amber"}),
    );
    assert_eq!(out["icon"], json!({"kind": "preset", "preset": "baker"}));
    assert_eq!(book_profile::PRESETS.len(), 24);
    let e = b.err(
        "book.update_profile",
        json!({"icon": {"kind": "preset", "preset": "astronaut"}}),
    );
    assert!(
        e.to_string()
            .starts_with("Unknown preset picture \"astronaut\"")
    );
    // A preset replaces a stored picture with its source.
    b.run(
        "book.set_picture",
        json!({"source_png_base64": photo(), "crop": whole()}),
    );
    b.run(
        "book.update_profile",
        json!({"icon": {"kind": "preset", "preset": "piggy-bank"}}),
    );
    assert_eq!(b.run("book.picture_source", json!({})), json!({}));
}

#[test]
fn app_desktop_at224_treatment() {
    let mut b = Book::new();
    let preview = |b: &mut Book, settings: Value, size: u32| {
        let out = b.run(
            "book.preview_picture",
            json!({"source_png_base64": photo(), "crop": whole(), "settings": settings, "size": size}),
        );
        rgba(&unb64(&out["png_base64"]))
    };
    // Background 0: nothing transparent.
    let (side, px) = preview(&mut b, json!({}), 256);
    assert_eq!(side, 256);
    assert!(px.chunks(4).all(|p| p[3] == 255));
    // Background 60: the corners go, the figure stays.
    let (_, px) = preview(&mut b, json!({"background": 60}), 256);
    let alpha = |x: usize, y: usize| px[(y * 256 + x) * 4 + 3];
    for (x, y) in [(1, 1), (254, 1), (1, 254), (254, 254)] {
        if y < 200 {
            assert_eq!(alpha(x, y), 0, "corner {x},{y}");
        }
    }
    assert_eq!(alpha(128, 100), 255, "head");
    assert_eq!(alpha(128, 220), 255, "shoulders");
    // Detail 100 draws more lines across a row of the figure than Detail 0.
    let lines = |b: &mut Book, detail: i32| {
        let (side, px) = preview(b, json!({"detail": detail, "outline": 0}), 512);
        let side = side as usize;
        let y = side * 41 / 100; // across the head
        let lum: Vec<i32> = (side * 2 / 5..side * 3 / 5)
            .map(|x| {
                let p = &px[(y * side + x) * 4..];
                (i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2])) / 3
            })
            .collect();
        // Count dark runs (ink lines) along the row.
        let mut runs = 0;
        let mut dark = false;
        for v in lum {
            if v < 128 && !dark {
                runs += 1;
            }
            dark = v < 128;
        }
        runs
    };
    let (coarse, fine) = (lines(&mut b, 0), lines(&mut b, 100));
    assert!(
        fine > coarse + 5,
        "Detail 100: {fine} lines, Detail 0: {coarse}"
    );
    // Before is the cropped photo itself.
    let out = b.run(
        "book.preview_picture",
        json!({"source_png_base64": photo(), "crop": {"x": 100, "y": 100, "size": 400}, "include_photo": true}),
    );
    let before = mk_import::image::decode_logo(&unb64(&out["before_png_base64"])).unwrap();
    assert_eq!((before.width, before.height), (256, 256));
    assert_eq!(
        (out["width"].as_u64(), out["height"].as_u64()),
        (Some(600), Some(600))
    );
    let photo_small = mk_import::image::decode_logo(&unb64(&out["photo_png_base64"])).unwrap();
    assert_eq!((photo_small.width, photo_small.height), (512, 512));
    // A preview changes nothing; bad input is refused clearly.
    let n = b.audit_len();
    let e = b.err(
        "book.preview_picture",
        json!({"source_png_base64": photo(), "crop": {"x": 300, "y": 0, "size": 400}}),
    );
    assert!(
        e.to_string().contains("inside the 600 × 600 picture"),
        "{e}"
    );
    let e = b.err(
        "book.preview_picture",
        json!({"source_png_base64": b64(b"text"), "crop": whole()}),
    );
    assert_eq!(e.to_string(), "Choose a PNG or JPEG picture up to 2 MB.");
    let e = b.err(
        "book.preview_picture",
        json!({"source_png_base64": photo(), "crop": whole(), "settings": {"detail": 101}}),
    );
    assert_eq!(e.to_string(), "Detail runs from 0 to 100.");
    let e = b.err(
        "book.preview_picture",
        json!({"source_png_base64": photo(), "crop": whole(), "settings": {"ink": "green"}}),
    );
    assert!(e.to_string().starts_with("Unknown ink"));
    assert_eq!(b.audit_len(), n);
    assert_eq!(b.run("book.info", json!({}))["icon"]["kind"], "letter");
}

#[test]
fn app_desktop_at225_storage_and_remove() {
    let mut b = Book::new();
    let settings = json!({"detail": 70, "line_weight": 40, "contrast": -5, "brightness": 8,
        "outline": 55, "curve": 10, "background": 60, "ink": "navy", "paper": "cream"});
    let n = b.audit_len();
    let info = b.run(
        "book.set_picture",
        json!({"source_png_base64": photo(), "crop": {"x": 50, "y": 0, "size": 500}, "settings": settings}),
    );
    assert_eq!(b.audit_len(), n + 1, "one undo step");
    assert_eq!(info["icon"]["kind"], "image");
    assert_eq!(info["icon"]["editable"], true);
    let (side, _) = rgba(&unb64(&info["icon"]["image_png_base64"]));
    assert_eq!(side, 256);
    // The audit entry holds the settings and crop, not the pictures.
    let log = b.s.audit_entries(1).unwrap();
    assert_eq!(log[0].args["crop"], json!({"x": 50, "y": 0, "size": 500}));
    assert!(log[0].args.get("source_png_base64").is_none());
    let src = b.run("book.picture_source", json!({}));
    assert_eq!(src["settings"], settings);
    let source = mk_import::image::decode_logo(&unb64(&src["source_png_base64"])).unwrap();
    assert_eq!((source.width, source.height), (512, 512));
    // Remove picture deletes all three; undo brings them back.
    let info = b.run("book.clear_icon_image", json!({}));
    assert_eq!(info["icon"], json!({"kind": "letter", "text": "T"}));
    assert_eq!(b.run("book.picture_source", json!({})), json!({}));
    b.run("history.undo", json!({}));
    assert_eq!(
        b.run("book.picture_source", json!({}))["settings"],
        settings
    );
    // A finished picture has no source to edit.
    let png = mk_import::image::solid_png(300, 300, [9, 9, 9]);
    let info = b.run("book.set_icon_image", json!({"content_base64": b64(&png)}));
    assert_eq!(info["icon"]["editable"], false);
    assert_eq!(b.run("book.picture_source", json!({})), json!({}));
}

#[test]
fn app_desktop_at226_deterministic() {
    let mut b = Book::new();
    let args = json!({"source_png_base64": photo(), "crop": {"x": 30, "y": 20, "size": 520},
        "settings": {"background": 40, "curve": 80, "ink": "sepia"}});
    let first = b.run("book.set_picture", args.clone())["icon"]["image_png_base64"].clone();
    let second = b.run("book.set_picture", args)["icon"]["image_png_base64"].clone();
    assert_eq!(first, second, "same PNG bytes");
    let mut other = Book::new();
    let third = other.run(
        "book.set_picture",
        json!({"source_png_base64": photo(), "crop": {"x": 30, "y": 20, "size": 520},
            "settings": {"background": 40, "curve": 80, "ink": "sepia"}}),
    )["icon"]["image_png_base64"]
        .clone();
    assert_eq!(first, third, "same bytes in another book");
}

#[test]
fn app_desktop_at228_brushes() {
    let mut b = Book::new();
    let side = mk_core::mask::SIDE;
    let mut mask = vec![0u8; mk_core::mask::LEN];
    // Remove a band across the shoulders; keep the top-left corner.
    for y in 200..215 {
        for x in 60..196 {
            mask[y * side + x] = 2;
        }
    }
    for y in 0..20 {
        for x in 0..20 {
            mask[y * side + x] = 1;
        }
    }
    let packed = b64(&mk_core::mask::pack(&mask));
    let run = |b: &mut Book, mask: &str| {
        let out = b.run(
            "book.preview_picture",
            json!({"source_png_base64": photo(), "crop": whole(),
                "settings": {"background": 60, "mask_base64": mask}}),
        );
        rgba(&unb64(&out["png_base64"])).1
    };
    let plain = run(&mut b, "");
    let brushed = run(&mut b, &packed);
    let alpha = |px: &[u8], x: usize, y: usize| px[(y * 256 + x) * 4 + 3];
    assert_eq!(alpha(&plain, 128, 207), 255);
    assert_eq!(
        alpha(&brushed, 128, 207),
        0,
        "Remove made the band transparent"
    );
    assert_eq!(alpha(&plain, 5, 5), 0);
    assert_eq!(alpha(&brushed, 5, 5), 255, "Keep restored the corner");
    // The strokes are stored with the settings.
    b.run(
        "book.set_picture",
        json!({"source_png_base64": photo(), "crop": whole(),
            "settings": {"background": 60, "mask_base64": packed}}),
    );
    let src = b.run("book.picture_source", json!({}));
    assert_eq!(src["settings"]["mask_base64"], json!(packed));
    let e = b.err(
        "book.preview_picture",
        json!({"source_png_base64": photo(), "crop": whole(), "settings": {"mask_base64": b64(&[9, 9])}}),
    );
    assert_eq!(e.to_string(), "The brush mask is damaged.");
}
