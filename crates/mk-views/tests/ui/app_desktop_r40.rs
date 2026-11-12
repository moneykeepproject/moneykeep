//! app-desktop.md revision 40 acceptance tests (219–228): book cards on the start
//! screen, taglines, preset pictures and the Picture editor. Test 229 (screenshots)
//! is the `cards_screens` example.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_arithmetic,
    clippy::cast_precision_loss
)]

use crate::common::*;
use mk_commands::{KdfParams, Session};
use mk_ui::ctl::App;
use mk_ui::{RecentBook, Screen, TileCache, TileIcon, UserConfig};
use mk_views::testing::{Harness, Key, Rect, Role, pos2};
use serde_json::{Value, json};

type H = Harness;

fn start_app(config: UserConfig) -> App {
    App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        config,
    )
}

/// Books created in `dir` and opened once each (so their cards have caches), the
/// first named last so it comes first.
fn books(dir: &std::path::Path, names: &[(&str, Value)]) -> (H, Vec<String>) {
    let mut paths = Vec::new();
    for (name, profile) in names {
        let p = dir.join(format!(
            "{}.moneykeep",
            name.to_lowercase().replace(' ', "-")
        ));
        let mut s = Session::create(&p, name, "USD", "test").unwrap();
        s.invoke("book.update_profile", profile.clone()).unwrap();
        drop(s);
        paths.push(p.to_str().unwrap().to_owned());
    }
    let mut h = harness(start_app(UserConfig::default()));
    h.state_mut().set_today("2026-11-07");
    for p in paths.iter().rev() {
        h.state_mut().open_book(p);
        h.state_mut().close_book();
    }
    h.run();
    (h, paths)
}

fn card(h: &H, name: &str) -> Rect {
    h.get_by_label(&format!("Open {name}")).rect()
}

/// Texts drawn inside `r`.
fn texts_in(h: &H, r: Rect) -> Vec<(String, Rect)> {
    h.output()
        .shapes
        .into_iter()
        .filter(|s| s.kind == "text" && r.contains_rect(s.rect))
        .map(|s| (s.text, s.rect))
        .collect()
}

fn drawn(h: &H, r: Rect, text: &str) -> Option<Rect> {
    texts_in(h, r)
        .into_iter()
        .find(|(t, _)| t == text)
        .map(|(_, r)| r)
}

fn card_names(h: &H) -> Vec<String> {
    let mut v: Vec<(f32, f32, String)> = h
        .all_labels()
        .into_iter()
        .filter(|l| l.starts_with("Open ") && l != "Open a book…")
        .map(|l| {
            let r = h.get_by_label(&l).rect();
            (r.top(), r.left(), l["Open ".len()..].to_owned())
        })
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v.into_iter().map(|(_, _, n)| n).collect()
}

#[test]
fn app_desktop_at219_cards_and_top_bar() {
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked.moneykeep");
    drop(
        Session::create_encrypted(
            &locked,
            "Locked",
            "USD",
            "test",
            "pw pw pw pw",
            true,
            KdfParams::TEST,
        )
        .unwrap(),
    );
    let (mut h, paths) = books(dir.path(), &[("Household", json!({}))]);
    let mut config = h.state().config.clone();
    config
        .recent
        .push(RecentBook::from(locked.to_str().unwrap()));
    let mut h2 = harness(start_app(config));
    h2.run();
    h.run();
    let h = &mut h2;
    assert!(has(h, "MONEYKEEP BOOKS"), "the top bar label");
    assert!(has(h, "Switch between light and dark"), "the theme button");
    for name in ["Household", "locked", "Create a New Book"] {
        let label = if name.starts_with("Create") {
            name.to_owned()
        } else {
            format!("Open {name}")
        };
        let r = h.get_by_label(&label).rect();
        assert!(
            (r.width() - 374.0).abs() < 1.0 && (r.height() - 312.0).abs() < 1.0,
            "{name}: {r:?}"
        );
    }
    let protected = card(h, "locked");
    assert!(drawn(h, protected, "PASSWORD PROTECTED").is_some());
    assert!(drawn(h, card(h, "Household"), "PASSWORD PROTECTED").is_none());
    assert!(drawn(h, card(h, "Household"), "01").is_some(), "badge 01");
    assert!(drawn(h, protected, "02").is_some(), "badge 02");
    let _ = paths;
    // The theme button switches the theme and remembers it.
    let dark = h.is_dark();
    click(h, "Switch between light and dark");
    assert_ne!(h.is_dark(), dark);
    assert_eq!(
        h.state().config.theme,
        if dark {
            mk_ui::Theme::Light
        } else {
            mk_ui::Theme::Dark
        }
    );
}

#[test]
fn app_desktop_at220_card_contents_open_and_remove() {
    let dir = tempfile::tempdir().unwrap();
    let long = "Weekly offerings and the building fund, the youth group's trip, the \
                choir's music budget, the food pantry, the parish picnic, the roof and \
                the organ fund, kept by the office.";
    assert!(long.chars().count() <= 200);
    let (mut h, paths) = books(
        dir.path(),
        &[
            (
                "Parish",
                json!({"tagline": "Church office", "description": long}),
            ),
            ("Garden", json!({})),
        ],
    );
    let r = card(&h, "Parish");
    for text in [
        "Parish",
        "USD",
        "Church office",
        "Last opened: Nov 7, 2026",
        "OPEN",
    ] {
        assert!(
            drawn(&h, r, text).is_some(),
            "{text} in {:?}",
            texts_in(&h, r)
        );
    }
    // The description is cut after three lines.
    let d = drawn(&h, r, long).unwrap();
    assert!(d.height() <= 61.0, "three lines at most: {d:?}");
    // The name sits beside the picture, the tagline under the name.
    let name = drawn(&h, r, "Parish").unwrap();
    let tagline = drawn(&h, r, "Church office").unwrap();
    assert!(tagline.top() >= name.bottom() - 2.0);
    // … → Remove from list removes the card.
    click(&mut h, "More for Garden");
    h.get_by_role_and_label(Role::MenuItem, "Remove from list")
        .click();
    h.run();
    assert!(!has(&h, "Open Garden"));
    assert_eq!(h.state().config.recent.len(), 1);
    // OPEN opens the book.
    let open = drawn(&h, card(&h, "Parish"), "OPEN").unwrap();
    h.click_at(open.center());
    h.run();
    assert_eq!(h.state().screen(), Screen::Main);
    assert_eq!(h.state().book_name().as_deref(), Some("Parish"));
    let _ = paths;
}

#[test]
fn app_desktop_at221_search_and_sort() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, _paths) = books(
        dir.path(),
        &[
            ("Zion Fund", json!({"description": "Church building fund"})),
            ("Acme LLC", json!({})),
            ("Mission", json!({"tagline": "A church outreach"})),
        ],
    );
    assert_eq!(
        card_names(&h),
        ["Zion Fund", "Acme LLC", "Mission"],
        "last opened first"
    );
    type_into(&mut h, "Search books", "church");
    assert_eq!(card_names(&h), ["Zion Fund", "Mission"]);
    assert!(has(&h, "Create a New Book"));
    h.get_by_label("Search books").set_text("");
    h.run();
    assert_eq!(card_names(&h).len(), 3);
    click(&mut h, "Sort");
    h.get_by_role_and_label(Role::Button, "Name (A–Z)").click();
    h.run();
    assert_eq!(card_names(&h), ["Acme LLC", "Mission", "Zion Fund"]);
    assert_eq!(h.state().config.start_sort, "name");
    // Remembered after a restart.
    let mut h = harness(start_app(h.state().config.clone()));
    h.run();
    assert_eq!(card_names(&h), ["Acme LLC", "Mission", "Zion Fund"]);
}

fn open_book_settings(h: &mut H) {
    click(h, "Settings");
    click(h, "Book");
}

#[test]
fn app_desktop_at222_tagline_in_settings() {
    let (app, _book) = new_book_app();
    let mut h = harness(app);
    h.run();
    open_book_settings(&mut h);
    let n = cmd(&mut h, "history.list", json!({"limit": 50}))
        .as_array()
        .unwrap()
        .len();
    type_into(&mut h, "Tagline", "Our family's money");
    assert!(has(&h, "18 / 60"), "live count");
    // The preview card follows the typed text before it is saved.
    let preview = h.get_by_label("Preview of Test Books").rect();
    assert!(drawn(&h, preview, "Our family's money").is_some());
    key(&mut h, Key::Enter);
    assert_eq!(
        cmd(&mut h, "book.info", json!({}))["tagline"],
        "Our family's money"
    );
    let log = cmd(&mut h, "history.list", json!({"limit": 50}));
    assert_eq!(log.as_array().unwrap().len(), n + 1, "one undo step");
    assert_eq!(log[0]["command"], "book.update_profile");
    // Over 60 characters: refused, with the count shown.
    h.get_by_label("Tagline").set_text(&"t".repeat(61));
    h.run();
    assert!(has(&h, "61 / 60"));
    h.get_by_label("Tagline").focus();
    h.run();
    key(&mut h, Key::Enter);
    assert!(has(
        &h,
        "The tagline can be at most 60 characters (61 now)."
    ));
    assert_eq!(
        cmd(&mut h, "book.info", json!({}))["tagline"],
        "Our family's money"
    );
}

#[test]
fn app_desktop_at223_people_and_work() {
    let (app, book) = new_book_app();
    let path = book.path.to_str().unwrap().to_owned();
    let mut h = harness(app);
    h.run();
    open_book_settings(&mut h);
    click(&mut h, "People and work");
    let presets = [
        "farmer",
        "pastor",
        "teacher",
        "nurse",
        "doctor",
        "carpenter",
        "builder",
        "mechanic",
        "shopkeeper",
        "baker",
        "cook",
        "bookkeeper",
        "lawyer",
        "artist",
        "musician",
        "gardener",
        "fisherman",
        "seamstress",
        "scholar",
        "student",
        "family",
        "couple",
        "grandparents",
        "piggy bank",
    ];
    for p in presets {
        assert_eq!(h.query_all_by_label(p).count(), 1, "{p}");
    }
    click(&mut h, "baker");
    let info = cmd(&mut h, "book.info", json!({}));
    assert_eq!(info["icon"], json!({"kind": "preset", "preset": "baker"}));
    assert!(matches!(
        h.get_by_label("baker").accesskit_node().toggled(),
        Some(mk_views::testing::Toggled::True)
    ));
    // The preview card draws the picture.
    let preview = h.get_by_label("Preview of Test Books").rect();
    assert!(
        h.output()
            .shapes
            .iter()
            .any(|s| s.kind == "image" && preview.contains_rect(s.rect)),
        "the picture in the preview"
    );
    // The colour behind it changes with the colour.
    click(&mut h, "rose");
    assert_eq!(cmd(&mut h, "book.info", json!({}))["color"], "rose");
    click(&mut h, "Close book");
    h.run();
    let cache = h
        .state()
        .config
        .recent
        .iter()
        .find(|r| r.path == path)
        .and_then(|r| r.tile.clone())
        .unwrap();
    assert_eq!(
        cache.icon,
        TileIcon::Preset {
            preset: "baker".into()
        }
    );
    assert_eq!(cache.color, "rose");
    let r = card(&h, "Test Books");
    assert!(
        h.output()
            .shapes
            .iter()
            .any(|s| s.kind == "image" && s.text == "Book picture" && r.contains_rect(s.rect)),
        "the picture on the card"
    );
}

/// The test photo as a PNG file.
fn photo() -> Vec<u8> {
    mk_import::image::encode_png(&mk_import::engrave::test_photo(600)).unwrap()
}

/// Settings → Book → Your picture → Choose picture… with the test photo.
fn open_editor(h: &mut H) {
    open_book_settings(h);
    click(h, "Your picture");
    click(h, "Choose picture…");
    assert!(events(h).contains(&mk_ui::AppEvent::ChooseBookIconFile));
    h.state_mut().set_book_icon(&photo());
    h.run();
    assert!(has(h, "Picture editor"));
}

fn set_slider(h: &mut H, name: &str, value: i32) {
    h.get_by_role_and_label(Role::Slider, name)
        .set_text(&value.to_string());
    h.run();
    assert_eq!(
        h.get_by_role_and_label(Role::Slider, name)
            .accesskit_node()
            .numeric_value(),
        Some(f64::from(value)),
        "{name}"
    );
}

/// The editor's treated preview (RGBA, 256 × 256).
fn preview(h: &H) -> Vec<u8> {
    let (pic, _) = h.state().picture_editor_preview().unwrap();
    let pic = pic.unwrap();
    assert_eq!((pic.width, pic.height), (256, 256));
    pic.rgba.clone()
}

fn alpha(px: &[u8], x: usize, y: usize) -> u8 {
    px[(y * 256 + x) * 4 + 3]
}

/// Dark runs across the head's row.
fn lines(px: &[u8]) -> usize {
    let y = 105;
    let mut runs = 0;
    let mut dark = false;
    for x in 100..156 {
        let p = &px[(y * 256 + x) * 4..];
        let v = (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3;
        if v < 140 && !dark {
            runs += 1;
        }
        dark = v < 140;
    }
    runs
}

#[test]
fn app_desktop_at224_picture_editor() {
    let (app, _book) = new_book_app();
    let mut h = harness(app);
    h.run();
    let n = cmd(&mut h, "history.list", json!({"limit": 50}))
        .as_array()
        .unwrap()
        .len();
    open_editor(&mut h);
    for name in [
        "Crop",
        "Zoom",
        "Detail",
        "Line weight",
        "Contrast",
        "Brightness",
        "Outline",
        "Curve",
        "Background",
        "Keep",
        "Remove",
        "Brush size",
        "Black",
        "Navy",
        "Sepia",
        "White",
        "Cream",
        "Fine",
        "Classic",
        "Bold",
        "Reset",
        "Use picture",
        "Cancel",
        "Preview at card size",
    ] {
        assert!(has(&h, name), "{name}");
    }
    // Open the first time: the tips panel (the tips also show on the page behind,
    // under Choose picture…).
    let tips = |h: &H| {
        h.get_by_role_and_label(Role::Button, "Tips for a good picture")
            .is_expanded()
    };
    assert_eq!(h.state().config.picture_tips_open, Some(true));
    assert!(tips(&h));
    assert!(has_contains(&h, "one person, or a small group"));
    // Background 0: nothing transparent.
    assert!(preview(&h).chunks(4).all(|p| p[3] == 255));
    // Background 60: the corners go, the figure stays.
    set_slider(&mut h, "Background", 60);
    let px = preview(&h);
    assert_eq!(alpha(&px, 2, 2), 0);
    assert_eq!(alpha(&px, 253, 2), 0);
    assert_eq!(alpha(&px, 128, 100), 255);
    // Detail 100 draws more lines across the head than Detail 0.
    set_slider(&mut h, "Outline", 0);
    set_slider(&mut h, "Detail", 0);
    let coarse = lines(&preview(&h));
    set_slider(&mut h, "Detail", 100);
    let fine = lines(&preview(&h));
    assert!(fine > coarse, "Detail 100: {fine}, Detail 0: {coarse}");
    // Before / After.
    click(&mut h, "Before");
    assert!(has(&h, "Preview (before)"));
    click(&mut h, "After");
    assert!(!has(&h, "Preview (before)"));
    assert!(has(&h, "Preview (after)"));
    // Cancel changes nothing.
    click(&mut h, "Cancel");
    assert!(!has(&h, "Picture editor"));
    assert_eq!(
        cmd(&mut h, "book.info", json!({}))["icon"]["kind"],
        "letter"
    );
    assert_eq!(
        cmd(&mut h, "history.list", json!({"limit": 50}))
            .as_array()
            .unwrap()
            .len(),
        n
    );
    // The tips panel stays as the user left it.
    open_editor(&mut h);
    h.get_by_role_and_label(Role::Button, "Tips for a good picture")
        .click();
    h.run();
    assert!(!tips(&h), "folded");
    assert_eq!(h.state().config.picture_tips_open, Some(false));
    click(&mut h, "Cancel");
    open_editor(&mut h);
    assert!(!tips(&h), "still folded");
    click(&mut h, "Cancel");
    // A file that is not a picture is refused under Choose picture….
    h.state_mut().set_book_icon(b"not a picture");
    h.run();
    assert!(!has(&h, "Picture editor"));
    assert!(has(&h, "Choose a PNG or JPEG picture up to 2 MB."));
}

#[test]
fn app_desktop_at225_use_edit_remove() {
    let (app, book) = new_book_app();
    let path = book.path.to_str().unwrap().to_owned();
    let mut h = harness(app);
    h.run();
    open_editor(&mut h);
    set_slider(&mut h, "Detail", 70);
    set_slider(&mut h, "Background", 55);
    click(&mut h, "Sepia");
    click(&mut h, "Cream");
    click(&mut h, "Use picture");
    assert!(!has(&h, "Picture editor"));
    let info = cmd(&mut h, "book.info", json!({}));
    assert_eq!(info["icon"]["kind"], "image");
    assert_eq!(info["icon"]["editable"], true);
    let src = cmd(&mut h, "book.picture_source", json!({}));
    for (k, v) in [
        ("detail", json!(70)),
        ("background", json!(55)),
        ("ink", json!("sepia")),
        ("paper", json!("cream")),
    ] {
        assert_eq!(src["settings"][k], v, "{k}");
    }
    // The card cache keeps the 256 × 256 result.
    let cached = TILE_IMAGES
        .with(|m| m.borrow().get(&path).cloned())
        .unwrap();
    assert_eq!(
        mk_import::image::decode_logo(&cached)
            .map(|r| (r.width, r.height))
            .unwrap(),
        (256, 256)
    );
    // Edit picture… reopens with the same values.
    click(&mut h, "Edit picture…");
    assert!(has(&h, "Picture editor"));
    for (name, v) in [
        ("Detail", 70.0),
        ("Background", 55.0),
        ("Line weight", 50.0),
    ] {
        assert_eq!(
            h.get_by_role_and_label(Role::Slider, name)
                .accesskit_node()
                .numeric_value(),
            Some(v),
            "{name}"
        );
    }
    assert!(matches!(
        h.get_by_label("Sepia").accesskit_node().toggled(),
        Some(mk_views::testing::Toggled::True)
    ));
    click(&mut h, "Cancel");
    // Remove picture deletes all three and returns to the letter.
    click(&mut h, "Remove picture");
    assert_eq!(
        cmd(&mut h, "book.info", json!({}))["icon"],
        json!({"kind": "letter", "text": "T"})
    );
    assert_eq!(cmd(&mut h, "book.picture_source", json!({})), json!({}));
    assert!(TILE_IMAGES.with(|m| m.borrow().get(&path).is_none()));
}

#[test]
fn app_desktop_at227_fonts_and_licences() {
    let cards = include_str!("../../ui/cards.slint");
    for (file, family) in [
        ("Archivo-Variable.ttf", "\"Archivo\""),
        ("Inter-Variable.ttf", "\"Inter\""),
        ("GeistMono-Variable.ttf", "\"Geist Mono\""),
    ] {
        assert!(
            cards.contains(&format!("import \"assets/fonts/{file}\";")),
            "{file}"
        );
        assert!(cards.contains(family), "{family}");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/fonts")
            .join(file);
        assert!(path.is_file(), "{file} is bundled");
    }
    let attribution = include_str!("../../../../ATTRIBUTION.md");
    for name in ["Archivo", "Geist Mono", "assets/fonts/Inter-Variable.ttf"] {
        assert!(attribution.contains(name), "{name} in ATTRIBUTION.md");
    }
    let mut h = harness(start_app(UserConfig::default()));
    h.run();
    click(&mut h, "About");
    assert!(has(&h, "Third-party licences"));
    assert!(has_contains(&h, "Archivo, Inter and Geist Mono"));
}

/// Paint a stroke on the large preview from `a` to `b` (in 0–256 picture units).
fn stroke(h: &mut H, a: (f32, f32), b: (f32, f32)) {
    let r = h.get_by_label("Preview (after)").rect();
    let at = |p: (f32, f32)| {
        pos2(
            r.left() + p.0 * r.width() / 256.0,
            r.top() + p.1 * r.height() / 256.0,
        )
    };
    h.drag_at(at(a));
    h.run();
    for k in 1..=8 {
        let t = k as f32 / 8.0;
        h.hover_at(at((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)));
        h.run();
    }
    h.drop_at(at(b));
    h.run();
}

#[test]
fn app_desktop_at228_brushes() {
    let (app, _book) = new_book_app();
    let mut h = harness(app);
    h.run();
    open_editor(&mut h);
    set_slider(&mut h, "Background", 60);
    let px = preview(&h);
    assert_eq!(alpha(&px, 128, 215), 255, "shoulders kept");
    assert_eq!(alpha(&px, 3, 3), 0, "corner removed");
    // Remove across the shoulders.
    click(&mut h, "Remove");
    stroke(&mut h, (90.0, 215.0), (166.0, 215.0));
    assert_eq!(alpha(&preview(&h), 128, 215), 0, "Remove took the band");
    // Keep over the removed corner.
    click(&mut h, "Keep");
    stroke(&mut h, (2.0, 4.0), (30.0, 4.0));
    assert_eq!(alpha(&preview(&h), 10, 4), 255, "Keep restored the corner");
    // Undo stroke reverts the last one.
    click(&mut h, "Undo stroke");
    assert_eq!(alpha(&preview(&h), 10, 4), 0);
    assert_eq!(alpha(&preview(&h), 128, 215), 0, "the first stroke stays");
    click(&mut h, "Use picture");
    let src = cmd(&mut h, "book.picture_source", json!({}));
    assert!(!src["settings"]["mask_base64"].as_str().unwrap().is_empty());
    // Edit picture… brings the strokes back.
    click(&mut h, "Edit picture…");
    assert_eq!(
        alpha(&preview(&h), 128, 215),
        0,
        "the stroke is still there"
    );
    let (_, settings) = h.state().picture_editor_preview().unwrap();
    assert_eq!(settings["mask_base64"], src["settings"]["mask_base64"]);
    // Clear brushes forgets them.
    click(&mut h, "Clear brushes");
    assert_eq!(alpha(&preview(&h), 128, 215), 255);
}

/// The tile cache keeps the tagline and currency for the cards (revision 40).
#[test]
fn app_desktop_r40_cache_keeps_tagline_and_currency() {
    let dir = tempfile::tempdir().unwrap();
    let (h, paths) = books(dir.path(), &[("Shop", json!({"tagline": "Corner store"}))]);
    let r = &h.state().config.recent[0];
    assert_eq!(r.path, paths[0]);
    let tile: &TileCache = r.tile.as_ref().unwrap();
    assert_eq!(
        (tile.tagline.as_str(), tile.currency.as_str()),
        ("Corner store", "USD")
    );
    assert_eq!(r.last_opened.as_deref(), Some("2026-11-07"));
}
