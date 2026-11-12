//! app-desktop.md revision 26 acceptance tests (160–169): book tiles on the start
//! screen and the book profile.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_commands::{KdfParams, Session};
use mk_ui::ctl::App;
use mk_ui::{AppEvent, RecentBook, Screen, TileCache, TileIcon, UserConfig};
use mk_views::testing::Harness;
use mk_views::testing::Key;
use serde_json::json;

type H = Harness;

const PW: &str = "correct horse battery";

fn start_app(recent: Vec<RecentBook>) -> App {
    App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        UserConfig {
            recent,
            ..UserConfig::default()
        },
    )
}

/// The accessible description of the node labelled `label`.
fn description(h: &H, label: &str) -> String {
    h.get_by_label(label)
        .accesskit_node()
        .description()
        .unwrap_or_default()
}

fn tile_of(h: &H, path: &str) -> TileCache {
    h.state()
        .config
        .recent
        .iter()
        .find(|r| r.path == path)
        .and_then(|r| r.tile.clone())
        .unwrap_or_else(|| panic!("no tile cache for {path}"))
}

fn default_color(name: &str) -> &'static str {
    let colors = [
        "teal", "blue", "indigo", "purple", "rose", "red", "orange", "amber", "green", "slate",
    ];
    let sum: u32 = name.chars().map(u32::from).sum();
    colors[(sum % 10) as usize]
}

fn open_book_settings(h: &mut H) {
    click(h, "Settings");
    click(h, "Book");
}

/// Texts drawn on screen with whether each was cut with "…", and its rectangle.
fn drawn_texts(h: &H) -> Vec<(String, bool, mk_views::testing::Rect)> {
    let mut out = Vec::new();
    for c in &h.output().shapes {
        if c.kind == "text" {
            out.push((c.text.clone(), c.elided, c.rect));
        }
    }
    out
}

#[test]
fn app_desktop_at160_new_book_tile_and_create_tile() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("household.moneykeep");
    let path = path.to_str().unwrap().to_owned();
    let mut app = start_app(Vec::new());
    app.create_book(&path, "Household", "USD");
    app.close_book();
    let mut h = harness(app);
    h.run();
    let tile = h.get_by_label("Open Household").rect();
    assert_eq!(description(&h, "Open Household"), "", "no description");
    let cache = tile_of(&h, &path);
    assert_eq!(cache.icon, TileIcon::Letter { text: "H".into() });
    assert_eq!(cache.color, default_color("Household"));
    assert_eq!(cache.name, "Household");
    // The name is drawn beside the coloured square (revision 40's card), and the H
    // inside it.
    let texts = drawn_texts(&h);
    let name = texts.iter().find(|(t, ..)| t == "Household").unwrap();
    assert!(
        tile.contains_rect(name.2) && !name.1,
        "{name:?} in {tile:?}"
    );
    let letter = texts.iter().find(|(t, ..)| t == "H").unwrap();
    assert!(letter.2.right() <= name.2.left(), "letter beside the name");
    // The last tile is Create a New Book, after the book's.
    let create = h.get_by_label("Create a New Book").rect();
    assert!(create.left() >= tile.right(), "{create:?} after {tile:?}");
    // With no recent books, only the Create tile.
    let mut h = harness(start_app(Vec::new()));
    h.run();
    assert!(has(&h, "Create a New Book"));
    assert_eq!(
        h.query_all_by_label_contains("Open ").count(),
        1,
        "only the Open a book… button"
    );
    assert!(has(&h, "Open a book…"));
}

#[test]
fn app_desktop_at161_create_dialog_with_description() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("church.moneykeep");
    let path = path.to_str().unwrap().to_owned();
    let mut h = harness(start_app(Vec::new()));
    h.run();
    click(&mut h, "Create a New Book");
    for label in [
        "Create a new book",
        "Book name",
        "Base currency",
        "Description (optional)",
        "Protect with a password",
    ] {
        assert!(has(&h, label), "{label}");
    }
    h.get_by_label("Book name").focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label("Book name").type_text("Church Office");
    h.run();
    type_into(&mut h, "Description (optional)", "Weekly offerings");
    click(&mut h, "Create book…");
    assert_eq!(
        events(&mut h),
        [AppEvent::ChooseNewBookPath {
            name: "Church Office".into(),
            currency: "USD".into()
        }]
    );
    assert!(!has(&h, "Book name"), "the dialog closed");
    h.state_mut().create_book(&path, "Church Office", "USD");
    h.run();
    assert_eq!(h.state().screen(), Screen::Main);
    assert_eq!(
        cmd(&mut h, "book.info", json!({}))["description"],
        "Weekly offerings"
    );
    click(&mut h, "Close book");
    h.run();
    assert_eq!(h.state().screen(), Screen::Start);
    assert_eq!(description(&h, "Open Church Office"), "Weekly offerings");
    assert_eq!(
        tile_of(&h, &path).icon,
        TileIcon::Letter { text: "C".into() }
    );
    // Cancel closes the dialog without an event.
    click(&mut h, "Create a New Book");
    click(&mut h, "Cancel");
    assert!(!has(&h, "Book name"));
    assert!(
        events(&mut h)
            .iter()
            .all(|e| !matches!(e, AppEvent::ChooseNewBookPath { .. }))
    );
}

#[test]
fn app_desktop_at162_settings_book_profile_and_undo() {
    let (app, book) = new_book_app();
    let path = book.path.to_str().unwrap().to_owned();
    let mut h = harness(app);
    h.run();
    let before = cmd(&mut h, "book.info", json!({}));
    open_book_settings(&mut h);
    assert!(has(&h, "Preview of Test Books"));
    type_into(&mut h, "Description", "Parish accounts");
    assert!(has(&h, "15 / 200"), "live count");
    key(&mut h, Key::Enter);
    assert_eq!(
        cmd(&mut h, "book.info", json!({}))["description"],
        "Parish accounts"
    );
    click(&mut h, "Symbol");
    click(&mut h, "church");
    click(&mut h, "purple");
    h.run();
    let info = cmd(&mut h, "book.info", json!({}));
    assert_eq!(info["icon"], json!({"kind": "symbol", "symbol": "church"}));
    assert_eq!(info["color"], "purple");
    // The toggles and the preview follow.
    for name in ["church", "purple"] {
        assert!(
            matches!(
                h.get_by_label(name).accesskit_node().toggled(),
                Some(mk_views::testing::Toggled::True)
            ),
            "{name} is on"
        );
    }
    assert_eq!(description(&h, "Preview of Test Books"), "Parish accounts");
    // Each change was one book.update_profile entry.
    let log = cmd(&mut h, "history.list", json!({"limit": 3}));
    assert!(
        log.as_array()
            .unwrap()
            .iter()
            .all(|e| e["command"] == "book.update_profile"),
        "{log}"
    );
    click(&mut h, "Close book");
    h.run();
    let cache = tile_of(&h, &path);
    assert_eq!(
        cache.icon,
        TileIcon::Symbol {
            symbol: "church".into()
        }
    );
    assert_eq!(cache.color, "purple");
    assert_eq!(description(&h, "Open Test Books"), "Parish accounts");
    // Reopen; undo three times restores the previous profile, and the tile follows.
    click(&mut h, "Open Test Books");
    for _ in 0..3 {
        click(&mut h, "Undo");
    }
    let after = cmd(&mut h, "book.info", json!({}));
    for k in ["description", "icon", "color"] {
        assert_eq!(after[k], before[k], "{k}");
    }
    let cache = tile_of(&h, &path);
    assert_eq!(cache.description, "");
    assert_eq!(cache.icon, TileIcon::Letter { text: "T".into() });
}

#[test]
fn app_desktop_at163_icon_picture() {
    let (app, book) = new_book_app();
    let path = book.path.to_str().unwrap().to_owned();
    let mut h = harness(app);
    h.run();
    open_book_settings(&mut h);
    click(&mut h, "Your picture");
    click(&mut h, "Choose picture…");
    assert!(events(&mut h).contains(&AppEvent::ChooseBookIconFile));
    let png = mk_import::image::solid_png(400, 300, [0x20, 0x60, 0xA0]);
    // Revision 40: the picture goes through the Picture editor.
    h.state_mut().set_book_icon(&png);
    h.run();
    click(&mut h, "Use picture");
    let info = cmd(&mut h, "book.info", json!({}));
    assert_eq!(info["icon"]["kind"], "image");
    // The tile cache keeps the PNG (256 × 256 since revision 40).
    let cached = TILE_IMAGES
        .with(|m| m.borrow().get(&path).cloned())
        .unwrap();
    let rgb = mk_import::image::decode_logo(&cached).unwrap();
    assert_eq!((rgb.width, rgb.height), (256, 256));
    assert_eq!(tile_of(&h, &path).icon, TileIcon::Image);
    // Refusals show the message under the field.
    let refused = "Choose a PNG or JPEG picture up to 2 MB.";
    let mut big = png.clone();
    big.resize(3 * 1024 * 1024, 0);
    h.state_mut().set_book_icon(&big);
    h.run();
    assert!(has(&h, refused));
    h.state_mut().set_book_icon(b"a text file, not a picture");
    h.run();
    assert!(has(&h, refused));
    assert_eq!(cmd(&mut h, "book.info", json!({}))["icon"]["kind"], "image");
    // The start screen draws the cached picture in the tile.
    click(&mut h, "Close book");
    h.run();
    h.run();
    let tile = h.get_by_label("Open Test Books").rect();
    let mut images = 0;
    for c in &h.output().shapes {
        if c.kind == "image" && tile.intersects(c.rect) {
            images += 1;
        }
    }
    assert!(images > 0, "the picture is drawn in the tile");
    // Remove picture returns to the letter.
    click(&mut h, "Open Test Books");
    open_book_settings(&mut h);
    click(&mut h, "Remove picture");
    assert_eq!(
        cmd(&mut h, "book.info", json!({}))["icon"],
        json!({"kind": "letter", "text": "T"})
    );
    assert!(TILE_IMAGES.with(|m| m.borrow().get(&path).is_none()));
}

#[test]
fn app_desktop_at164_protected_tile_without_password() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("locked.moneykeep");
    let path_s = path.to_str().unwrap().to_owned();
    drop(
        Session::create_encrypted(&path, "Locked", "USD", "test", PW, true, KdfParams::TEST)
            .unwrap(),
    );
    // Open it once with the password and give it a profile.
    let mut h = harness(start_app(vec![path_s.clone().into()]));
    h.run();
    click(&mut h, "Open locked");
    type_into(&mut h, "Password", PW);
    click(&mut h, "Open");
    h.run();
    assert_eq!(h.state().screen(), Screen::Main);
    cmd(
        &mut h,
        "book.update_profile",
        json!({"name": "Parish Funds", "description": "Building fund",
               "icon": {"kind": "symbol", "symbol": "church"}, "color": "rose"}),
    );
    click(&mut h, "Close book");
    let config = h.state().config.clone();
    // A new start: the tile shows the cache and the lock, with no password asked.
    let app = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        config,
    );
    let mut h = harness(app);
    h.run();
    assert!(has(&h, "Open Parish Funds"));
    assert_eq!(description(&h, "Open Parish Funds"), "Building fund");
    assert!(has(&h, "Password protected"));
    assert!(!has(&h, "Use recovery key instead"), "no password prompt");
    assert_eq!(
        tile_of(&h, &path_s).icon,
        TileIcon::Symbol {
            symbol: "church".into()
        }
    );
}

#[test]
fn app_desktop_at165_rename_changes_top_bar_and_tile_not_file() {
    let (app, book) = new_book_app();
    let path = book.path.to_str().unwrap().to_owned();
    let mut h = harness(app);
    h.run();
    open_book_settings(&mut h);
    h.get_by_label("Name").focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label("Name").type_text("Our Household");
    h.run();
    key(&mut h, Key::Enter);
    h.run();
    assert_eq!(cmd(&mut h, "book.info", json!({}))["name"], "Our Household");
    assert!(
        h.query_all_by_label("Our Household").next().is_some(),
        "top bar shows the new name"
    );
    assert!(!has(&h, "Test Books"));
    let files: Vec<String> = std::fs::read_dir(book.dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".moneykeep"))
        .collect();
    assert_eq!(files, ["test.moneykeep"], "the file keeps its name");
    click(&mut h, "Close book");
    h.run();
    assert!(has(&h, "Open Our Household"));
    assert_eq!(tile_of(&h, &path).name, "Our Household");
    assert_eq!(h.state().config.recent[0].path, path);
}

#[test]
fn app_desktop_at166_missing_file_tile_and_remove() {
    let gone = "/nonexistent-moneykeep-dir/gone.moneykeep".to_owned();
    let other = "/nonexistent-moneykeep-dir/other.moneykeep".to_owned();
    TILE_IMAGES.with(|m| m.borrow_mut().insert(gone.clone(), vec![1, 2, 3]));
    let recent = vec![
        RecentBook {
            path: gone.clone(),
            tile: Some(TileCache {
                name: "Old Books".into(),
                description: "Gone away".into(),
                icon: TileIcon::Image,
                color: "slate".into(),
                tagline: String::new(),
                currency: String::new(),
            }),
            last_opened: None,
        },
        other.clone().into(),
    ];
    let mut h = harness(start_app(recent));
    h.run();
    assert!(has(&h, "Open Old Books"));
    // A book with no cache shows its file name.
    assert!(has(&h, "Open other"));
    assert_eq!(
        h.query_all_by_label("This book's file can't be found or read.")
            .count(),
        2
    );
    let removes: Vec<_> = h.query_all_by_label("Remove from list").collect();
    assert_eq!(removes.len(), 2);
    removes[0].click();
    h.run();
    let paths: Vec<String> = h
        .state()
        .config
        .recent
        .iter()
        .map(|r| r.path.clone())
        .collect();
    assert_eq!(
        paths,
        std::slice::from_ref(&other),
        "its cache entry is gone"
    );
    assert!(TILE_IMAGES.with(|m| m.borrow().get(&gone).is_none()));
    assert!(events(&mut h).contains(&AppEvent::ConfigChanged));
    // The right-click menu offers Open and Remove from list.
    h.get_by_label("Open other").click_secondary();
    h.run();
    assert!(has(&h, "Open"));
    let menu_remove = h
        .query_all_by_label("Remove from list")
        .last()
        .map(|n| n.rect());
    assert!(menu_remove.is_some());
}

#[test]
fn app_desktop_at168_keyboard_tiles() {
    let dir = tempfile::tempdir().unwrap();
    let mut recent = Vec::new();
    for name in ["Alpha", "Bravo", "Charlie"] {
        let p = dir
            .path()
            .join(format!("{}.moneykeep", name.to_lowercase()));
        drop(Session::create(&p, name, "USD", "test").unwrap());
        recent.push(p.to_str().unwrap().to_owned());
    }
    // Caches as after opening each once.
    let mut h = harness(start_app(Vec::new()));
    for p in recent.iter().rev() {
        h.state_mut().open_book(p);
        h.state_mut().close_book();
    }
    h.run();
    for name in ["Alpha", "Bravo", "Charlie"] {
        assert!(has(&h, &format!("Open {name}")), "{name}");
    }
    // Revision 40: the toolbar (Open a book…, Sort, Search books) comes first.
    for _ in 0..6 {
        if h.get_by_label("Open Alpha").is_focused() {
            break;
        }
        key(&mut h, Key::Tab);
    }
    assert!(
        h.get_by_label("Open Alpha").is_focused(),
        "Tab reaches the first tile"
    );
    key(&mut h, Key::ArrowRight);
    assert!(h.get_by_label("Open Bravo").is_focused());
    key(&mut h, Key::ArrowRight);
    key(&mut h, Key::ArrowLeft);
    assert!(h.get_by_label("Open Bravo").is_focused());
    key(&mut h, Key::Tab);
    assert!(h.get_by_label("Open Charlie").is_focused(), "Tab moves on");
    key(&mut h, Key::ArrowLeft);
    key(&mut h, Key::Enter);
    h.run();
    assert_eq!(h.state().screen(), Screen::Main);
    assert_eq!(h.state().book_name().as_deref(), Some("Bravo"));
}

#[test]
fn app_desktop_at169_tiles_wrap_at_200_percent() {
    let names = [
        "Household",
        "St. Mary's Parish Office",
        "Rental Property on Elm",
        "Garden Club",
        "Youth Soccer League",
        "Family Trust",
        "Small Business",
    ];
    let recent: Vec<RecentBook> = names
        .iter()
        .enumerate()
        .map(|(i, n)| RecentBook {
            path: format!("/nonexistent-moneykeep-dir/b{i}.moneykeep"),
            tile: Some(TileCache {
                name: (*n).into(),
                description: "Accounts kept for the year".into(),
                icon: TileIcon::Letter {
                    text: n[..1].into(),
                },
                color: "blue".into(),
                tagline: String::new(),
                currency: String::new(),
            }),
            last_opened: None,
        })
        .collect();
    let app = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        UserConfig {
            recent,
            ..UserConfig::default()
        },
    );
    let mut h = harness(app);
    h.set_size(mk_views::testing::vec2(1400.0, 900.0));
    h.run();
    while h.state().text_size() < 200 {
        h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::Plus);
        h.run();
    }
    h.run();
    assert!((h.zoom() - 2.0).abs() < 1e-6);
    let window = h.window_rect();
    // Revision 40's cards are 374 × 312: one column fits at 200 %, so the cards wrap
    // onto a line each and the page scrolls. Each card is checked when shown (finding
    // one by name scrolls to it), with the card after it when that shows too.
    let mut labels: Vec<String> = names.iter().map(|n| format!("Open {n}")).collect();
    labels.push("Create a New Book".into());
    let mut tops = std::collections::BTreeSet::new();
    for (i, label) in labels.iter().enumerate() {
        let a = h.get_by_label(label).rect();
        assert!(
            a.left() >= window.left() && a.right() <= window.right(),
            "{a:?} inside {window:?}"
        );
        assert!(a.width() * 2.0 > window.width(), "one card a line: {a:?}");
        // The next card, when it shows in the same view, is below and apart.
        let nodes = h.nodes();
        if let Some(next) = labels.get(i + 1)
            && let Some(b) = nodes
                .iter()
                .find(|n| n.label().as_deref() == Some(next.as_str()))
        {
            let b = b.rect();
            assert!(
                !a.intersects(b) && b.top() >= a.bottom() - 0.5,
                "{a:?} overlaps {b:?}"
            );
        }
        drop(nodes);
        tops.insert(a.top().round() as i64);
        // The name is drawn inside its card (one line; revision 40 cuts a long one
        // with "…").
        if let Some(n) = names.get(i) {
            let t = h.get_by_label(n).rect();
            let a = h.get_by_label(label).rect();
            assert!(a.contains_rect(t), "{n}: {t:?} in {a:?}");
        }
    }
    assert!(tops.len() > 1, "the cards wrap");
}
