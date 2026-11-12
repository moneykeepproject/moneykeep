//! app-desktop.md revision 26: the book profile commands (acceptance tests 160, 162,
//! 163, 165 and 167 at the command level).

use base64::Engine;

use super::*;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn message(e: &CommandError) -> String {
    e.to_string()
}

#[test]
fn app_desktop_at160_default_profile() {
    let mut b = Book::new();
    b.run("book.update_profile", json!({"name": "Household"}));
    let info = b.run("book.info", json!({}));
    assert_eq!(info["name"], "Household");
    assert_eq!(info["description"], "");
    assert_eq!(info["icon"], json!({"kind": "letter", "text": "H"}));
    // Index (sum of character codes) mod 10.
    let sum: u32 = "Household".chars().map(u32::from).sum();
    let expected = book_profile::COLORS[(sum % 10) as usize];
    assert_eq!(info["color"], expected);
    // A name starting with a non-letter uses its first character.
    b.run("book.update_profile", json!({"name": "2026 budget"}));
    assert_eq!(b.run("book.info", json!({}))["icon"]["text"], "2");
}

#[test]
fn app_desktop_at162_update_profile_and_undo() {
    let mut b = Book::new();
    let before = b.run("book.info", json!({}));
    let n = b.audit_len();
    let out = b.run(
        "book.update_profile",
        json!({"description": "Weekly\nofferings", "icon": {"kind": "symbol", "symbol": "church"}, "color": "purple"}),
    );
    assert_eq!(
        out["description"], "Weekly offerings",
        "line breaks become spaces"
    );
    assert_eq!(out["icon"], json!({"kind": "symbol", "symbol": "church"}));
    assert_eq!(out["color"], "purple");
    assert_eq!(b.audit_len(), n + 1, "one audit entry, one undo step");
    // Only the given fields change.
    b.run("book.update_profile", json!({"color": "green"}));
    let info = b.run("book.info", json!({}));
    assert_eq!(info["description"], "Weekly offerings");
    assert_eq!(info["color"], "green");
    b.run("history.undo", json!({}));
    b.run("history.undo", json!({}));
    let after = b.run("book.info", json!({}));
    for k in ["name", "description", "icon", "color"] {
        assert_eq!(after[k], before[k], "{k}");
    }
    // Redo brings it back.
    b.run("history.redo", json!({}));
    assert_eq!(b.run("book.info", json!({}))["color"], "purple");
    // The same values again record nothing.
    let n = b.audit_len();
    b.run("book.update_profile", json!({"color": "purple"}));
    assert_eq!(b.audit_len(), n);
}

#[test]
fn app_desktop_at163_icon_picture() {
    let mut b = Book::new();
    let png = mk_import::image::solid_png(400, 300, [0x6A, 0x1B, 0x9A]);
    let info = b.run("book.set_icon_image", json!({"content_base64": b64(&png)}));
    assert_eq!(info["icon"]["kind"], "image");
    let stored = base64::engine::general_purpose::STANDARD
        .decode(info["icon"]["image_png_base64"].as_str().unwrap())
        .unwrap();
    let rgb = mk_import::image::decode_logo(&stored).unwrap();
    assert_eq!(
        (rgb.width, rgb.height),
        (256, 256),
        "256 × 256 since revision 40"
    );
    assert_eq!(&rgb.data[..3], &[0x6A, 0x1B, 0x9A]);
    // The audit entry records the size, not the picture.
    let log = b.s.audit_entries(1).unwrap();
    assert_eq!(log[0].args, json!({"bytes": png.len()}));
    // A 3 MB file and a text file are refused with the message.
    let refused = "Choose a PNG or JPEG picture up to 2 MB.";
    let mut big = png.clone();
    big.resize(3 * 1024 * 1024, 0);
    let e = b.err("book.set_icon_image", json!({"content_base64": b64(&big)}));
    assert_eq!(message(&e), refused);
    let e = b.err(
        "book.set_icon_image",
        json!({"content_base64": b64(b"just some text")}),
    );
    assert_eq!(message(&e), refused);
    assert_eq!(e.code(), "invalid_args");
    // Clearing returns to the default letter; undo brings the picture back.
    let info = b.run("book.clear_icon_image", json!({}));
    assert_eq!(info["icon"], json!({"kind": "letter", "text": "T"}));
    b.run("history.undo", json!({}));
    assert_eq!(b.run("book.info", json!({}))["icon"]["kind"], "image");
    // Choosing a letter replaces the picture.
    b.run(
        "book.update_profile",
        json!({"icon": {"kind": "letter", "text": "ab"}}),
    );
    assert_eq!(
        b.run("book.info", json!({}))["icon"],
        json!({"kind": "letter", "text": "AB"})
    );
}

#[test]
fn app_desktop_at163_phone_photo_and_too_wide_picture() {
    let mut b = Book::new();
    // A 4,000 × 3,000 JPEG under 2 MB (a phone photo) is accepted.
    let jpeg = include_bytes!("fixtures/photo_4000x3000.jpg");
    assert!(jpeg.len() < 2 * 1024 * 1024);
    let info = b.run("book.set_icon_image", json!({"content_base64": b64(jpeg)}));
    let stored = base64::engine::general_purpose::STANDARD
        .decode(info["icon"]["image_png_base64"].as_str().unwrap())
        .unwrap();
    let rgb = mk_import::image::decode_logo(&stored).unwrap();
    assert_eq!(
        (rgb.width, rgb.height),
        (256, 256),
        "256 × 256 since revision 40"
    );
    // 7,000 pixels wide is refused with the same message (checked from the header).
    let wide = mk_import::image::solid_png(7000, 4, [1, 2, 3]);
    assert!(wide.len() < 2 * 1024 * 1024);
    let e = b.err("book.set_icon_image", json!({"content_base64": b64(&wide)}));
    assert_eq!(message(&e), "Choose a PNG or JPEG picture up to 2 MB.");
    // Exactly 6,000 is still fine.
    let edge = mk_import::image::solid_png(6000, 4, [9, 9, 9]);
    b.run("book.set_icon_image", json!({"content_base64": b64(&edge)}));
}

#[test]
fn app_desktop_at165_rename_keeps_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("household.moneykeep");
    let mut s = Session::create(&path, "Household", "USD", "test").unwrap();
    s.invoke("book.update_profile", json!({"name": "  Our Home  "}))
        .unwrap();
    assert_eq!(
        s.invoke("book.info", json!({})).unwrap()["name"],
        "Our Home"
    );
    drop(s);
    assert!(path.exists(), "the file keeps its name");
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".moneykeep"))
        .collect();
    assert_eq!(names, ["household.moneykeep"]);
    let mut s = Session::open(&path, "test").unwrap();
    assert_eq!(
        s.invoke("book.info", json!({})).unwrap()["name"],
        "Our Home"
    );
    // Undo restores the old name.
    s.invoke("history.undo", json!({})).unwrap();
    assert_eq!(
        s.invoke("book.info", json!({})).unwrap()["name"],
        "Household"
    );
}

#[test]
fn app_desktop_at167_update_profile_refusals() {
    let mut b = Book::new();
    let n = b.audit_len();
    let cases = [
        (
            json!({"description": "x".repeat(201)}),
            "The description can be at most 200 characters.",
        ),
        (
            json!({"icon": {"kind": "letter", "text": "abc"}}),
            "The icon letter can be at most 2 characters.",
        ),
        (
            json!({"icon": {"kind": "symbol", "symbol": "rocket"}}),
            "Unknown symbol \"rocket\"",
        ),
        (
            json!({"color": "chartreuse"}),
            "Unknown colour \"chartreuse\"",
        ),
        (json!({"name": "   "}), "Give the book a name."),
        (
            json!({"name": "n".repeat(81)}),
            "The book name can be at most 80 characters.",
        ),
    ];
    for (args, text) in cases {
        let e = b.err("book.update_profile", args.clone());
        assert!(message(&e).starts_with(text), "{args}: {e}");
        assert_eq!(e.code(), "invalid_args");
    }
    // A refused field leaves the others unchanged.
    b.err(
        "book.update_profile",
        json!({"description": "fine", "color": "chartreuse"}),
    );
    assert_eq!(b.run("book.info", json!({}))["description"], "");
    assert_eq!(b.audit_len(), n);
    // 200 characters are fine; control and bidi characters are removed.
    let ok = "y".repeat(200);
    b.run("book.update_profile", json!({"description": ok}));
    let out = b.run(
        "book.update_profile",
        json!({"description": "Office\u{202E} \u{1b}[2Jfund"}),
    );
    assert_eq!(out["description"], "Office [2Jfund");
    // The profile is not reachable through setting.set.
    let e = b.err(
        "setting.set",
        json!({"module": "book", "key": "book.color", "value": "nope"}),
    );
    assert_eq!(e.code(), "invalid_args");
}
