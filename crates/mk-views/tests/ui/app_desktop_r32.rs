//! app-desktop.md revision 32 (tests 187–188): one click everywhere, and the
//! minimal password prompt.
//!
//! Test 187 clicks the way a mouse does: the pointer moves onto the control and the
//! left button goes down and up there, through the window's own hit testing (the
//! harness's `click()` uses the accessible default action instead, which never
//! sees what lies on top of a control).
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_arithmetic
)]

use crate::common::*;
use mk_commands::{KdfParams, Session};
use mk_ui::ctl::App;
use mk_ui::{Screen, UserConfig, View};
use mk_views::testing::{Harness, Key};

type H = Harness;

const PW: &str = "correct horse battery";

fn start_app(recent: Vec<String>) -> App {
    App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        UserConfig {
            recent: recent.into_iter().map(Into::into).collect(),
            ..iso_config()
        },
    )
}

fn plain_book(dir: &std::path::Path) -> String {
    let path = dir.join("parish.moneykeep");
    let mut s = Session::create(&path, "Parish", "USD", "test").unwrap();
    s.invoke(
        "account.create",
        serde_json::json!({"name": "Checking", "type": "asset", "subtype": "bank"}),
    )
    .unwrap();
    drop(s);
    path.to_str().unwrap().to_owned()
}

fn protected_book(dir: &std::path::Path) -> String {
    let path = dir.join("locked.moneykeep");
    let (s, _) =
        Session::create_encrypted(&path, "Locked", "USD", "test", PW, true, KdfParams::TEST)
            .unwrap();
    drop(s);
    path.to_str().unwrap().to_owned()
}

/// How the pointer reaches the control before the button goes down.
#[derive(Debug, Clone, Copy)]
enum Approach {
    /// It moves onto the control and clicks at once.
    Quick,
    /// It rests until the control's tooltip shows, then clicks.
    Rest,
    /// It rests, and the view model is rebuilt between press and release.
    Rebuild,
}

const APPROACHES: [Approach; 3] = [Approach::Quick, Approach::Rest, Approach::Rebuild];

/// One mouse click on the centre of `rect` (the pointer comes from the window's
/// corner, so the control sees it arrive).
fn mouse_click(h: &mut H, rect: Rect, how: Approach) {
    let c = rect.center();
    h.hover_at(pos2(2.0, 2.0));
    h.run();
    h.hover_at(c);
    if !matches!(how, Approach::Quick) {
        h.run();
    }
    h.press_at(c);
    if matches!(how, Approach::Rebuild) {
        h.views().refresh();
        h.run();
    }
    h.release_at(c);
    h.run();
}

/// Whether the tooltip bubble shows `text`.
fn tooltip_shows(h: &H, text: &str) -> bool {
    h.tooltip().as_deref() == Some(text)
}

fn tile_rect(h: &H, label: &str) -> Rect {
    h.get_by_label(label).rect()
}

#[test]
fn app_desktop_at187_one_click_book_tile() {
    for how in APPROACHES {
        let dir = tempfile::tempdir().unwrap();
        let path = plain_book(dir.path());
        let mut h = harness(start_app(vec![path]));
        let r = tile_rect(&h, "Open parish");
        if matches!(how, Approach::Rest) {
            h.hover_at(r.center());
            h.run();
            assert!(
                h.tooltip().is_some() || h.get_by_label("Open parish").description().is_none(),
                "{how:?}: the tile's tooltip shows while the pointer rests"
            );
        }
        mouse_click(&mut h, r, how);
        assert_eq!(
            h.state().screen(),
            Screen::Main,
            "{how:?}: one click on the tile opens the book"
        );
    }
}

#[test]
fn app_desktop_at187_one_click_top_bar_settings() {
    for how in APPROACHES {
        let (app, _book) = new_book_app();
        let mut h = harness(app);
        let r = h.get_by_label("Settings").rect();
        if matches!(how, Approach::Rest) {
            h.hover_at(pos2(2.0, 2.0));
            h.run();
            h.hover_at(r.center());
            h.run();
            assert!(
                tooltip_shows(&h, "Settings"),
                "the tooltip shows: {:?}",
                h.tooltip()
            );
        }
        mouse_click(&mut h, r, how);
        assert!(
            matches!(h.state().view(), View::Settings),
            "{how:?}: one click on Settings opens Settings, got {:?}",
            h.state().view()
        );
        // And one more click on Close book closes it.
        let r = h.get_by_label("Close book").rect();
        mouse_click(&mut h, r, how);
        assert_eq!(
            h.state().screen(),
            Screen::Start,
            "{how:?}: one click on Close book closes the book"
        );
    }
}

#[test]
fn app_desktop_at187_one_click_rail_tab() {
    for how in APPROACHES {
        let (mut h, _book) = harness_with_chart();
        // Another control holds keyboard focus.
        h.get_by_label("Settings").focus();
        h.run();
        let r = rail_tab(&h, "All accounts").rect();
        mouse_click(&mut h, r, how);
        assert_eq!(
            rail_tab(&h, "All accounts").is_selected(),
            Some(true),
            "{how:?}: one click on the rail tab opens the pane"
        );
    }
}

#[test]
fn app_desktop_at187_one_click_dialog_buttons() {
    for how in APPROACHES {
        let dir = tempfile::tempdir().unwrap();
        let path = protected_book(dir.path());
        let mut h = harness(start_app(vec![path]));
        let r = tile_rect(&h, "Open locked");
        mouse_click(&mut h, r, how);
        assert!(
            has(&h, "Cancel"),
            "{how:?}: one click opens the password prompt"
        );
        // The × (Cancel) closes it with one click.
        let r = h.get_by_label("Cancel").rect();
        mouse_click(&mut h, r, how);
        assert!(!has(&h, "Cancel"), "{how:?}: one click on × cancels");
        // Open with one click on the primary button.
        let r = tile_rect(&h, "Open locked");
        mouse_click(&mut h, r, how);
        type_into(&mut h, "Password", PW);
        let r = h.get_by_label("Open").rect();
        mouse_click(&mut h, r, how);
        h.run();
        assert_eq!(
            h.state().screen(),
            Screen::Main,
            "{how:?}: one click on Open unlocks"
        );
    }
}

/// Every focus scope says whether a click gives it focus. A focus scope drawn over
/// a touch area that does take focus on click swallows the first press (Slint
/// gives the scope focus and accepts the press, so the touch area never sees a
/// click until the scope already has focus): a control's own key handling must
/// not take clicks.
#[test]
fn app_desktop_at187_focus_scopes_do_not_take_clicks() {
    let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(&ui).unwrap() {
        let path = entry.unwrap().path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "slint" && ext != "template" {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (at, _) in text.match_indices("FocusScope {") {
            // The scope's own block (up to its matching brace), without nested
            // elements.
            let body = &text[at + "FocusScope {".len()..];
            let mut depth = 1;
            let mut own = String::new();
            for ch in body.chars() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                if depth == 1 {
                    own.push(ch);
                }
            }
            if !own.contains("focus-on-click:") {
                let line = text[..at].lines().count() + 1;
                missing.push(format!(
                    "{}:{line}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "focus scopes without focus-on-click: {missing:?}"
    );
}

/// The password prompt's card (the dialog group, found by its accessibility id).
fn dialog_card(h: &H) -> Rect {
    use i_slint_backend_testing::ElementQuery;
    let el = ElementQuery::from_root(h.views().window())
        .match_descendants()
        .match_predicate(|e| e.accessible_id().as_deref() == Some("dialog|dialog:unlock"))
        .find_first()
        .unwrap_or_else(|| panic!("no unlock dialog"));
    let p = el.absolute_position();
    let sz = el.size();
    Rect::from_min_size(pos2(p.x, p.y), vec2(sz.width, sz.height))
}

fn text_fields(h: &H) -> Vec<Rect> {
    h.query_all_by(|n| {
        matches!(
            n.role(),
            Role::TextInput | Role::PasswordInput | Role::MultilineTextInput
        )
    })
    .map(|n| n.rect())
    .collect()
}

#[test]
fn app_desktop_at188_minimal_password_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let path = protected_book(dir.path());
    let mut h = harness(start_app(vec![path]));
    click(&mut h, "Open locked");
    h.run();
    // A compact card: the book's name is the only heading and the only text.
    assert!(has(&h, "locked"), "the book's name heads the prompt");
    for gone in [
        "This book is password protected.",
        "Unlock locked",
        "Unlocking…",
    ] {
        assert!(!has(&h, gone), "{gone}");
    }
    // Exactly one text field, centred in the card, named and labelled
    // "Password", with focus already in it.
    let fields = text_fields(&h);
    assert_eq!(fields.len(), 1, "one text field: {fields:?}");
    let field = h.get_by_label("Password");
    assert_eq!(field.placeholder().as_deref(), Some("Password"));
    assert!(field.is_focused(), "focus is in the field");
    let card = dialog_card(&h);
    let f = fields[0];
    assert!(
        (f.center().x - card.center().x).abs() <= 2.0,
        "centred horizontally: field {f:?} card {card:?}"
    );
    assert!(
        (f.center().y - card.center().y).abs() <= 6.0,
        "centred vertically: field {f:?} card {card:?}"
    );
    // Icon buttons with tooltips and names, and no text labels.
    for (name, tip) in [
        ("Open", "Open"),
        ("Use recovery key instead", "Use recovery key instead"),
        ("Cancel", "Cancel"),
    ] {
        let b = h.get_by_label(name);
        assert_eq!(b.role(), Role::Button, "{name} is a button");
        assert_eq!(b.description().as_deref(), Some(tip), "{name}'s tooltip");
        assert!(
            b.rect().width() <= 40.0,
            "{name} is an icon button: {:?}",
            b.rect()
        );
    }
    // The Open button's tooltip shows under the pointer.
    let open = h.get_by_label("Open").rect();
    h.hover_at(pos2(2.0, 2.0));
    h.run();
    h.hover_at(open.center());
    h.run();
    assert_eq!(h.tooltip().as_deref(), Some("Open"));
    // The × is in the top-right corner of the card.
    let x = h.get_by_label("Cancel").rect();
    assert!(x.right() > card.right() - 48.0 && x.top() < card.top() + 48.0);

    // The key button switches to a recovery key field and back.
    click(&mut h, "Use recovery key instead");
    assert_eq!(text_fields(&h).len(), 1);
    let rk = h.get_by_label("Recovery key");
    assert_eq!(rk.placeholder().as_deref(), Some("Recovery key"));
    assert!(rk.is_focused());
    let back = h.get_by_label("Use password instead");
    assert_eq!(back.description().as_deref(), Some("Use password instead"));
    click(&mut h, "Use password instead");
    assert!(has(&h, "Password") && has(&h, "Use recovery key instead"));

    // A wrong password: one short line under the field; the field is cleared
    // and keeps focus.
    type_into(&mut h, "Password", "not the password");
    key(&mut h, Key::Enter);
    h.run();
    assert_eq!(h.state().screen(), Screen::Start);
    let err = h.get_by_label("Wrong password.");
    let f = h.get_by_label("Password");
    assert!(
        err.rect().top() >= f.rect().bottom() - 1.0,
        "under the field"
    );
    assert_eq!(value(&h, "Password"), "");
    assert!(
        h.get_by_label("Password").is_focused(),
        "the field keeps focus"
    );
    // The recovery key's own message.
    click(&mut h, "Use recovery key instead");
    type_into(&mut h, "Recovery key", "AAAA-BBBB-CCCC-DDDD");
    click(&mut h, "Open");
    h.run();
    assert!(has(&h, "Wrong recovery key."));
    // Esc cancels.
    key(&mut h, Key::Escape);
    assert!(!has(&h, "Use password instead"), "Esc closes the prompt");
    // Enter opens with the right password.
    click(&mut h, "Open locked");
    type_into(&mut h, "Password", PW);
    key(&mut h, Key::Enter);
    h.run();
    assert_eq!(h.state().screen(), Screen::Main);
}
