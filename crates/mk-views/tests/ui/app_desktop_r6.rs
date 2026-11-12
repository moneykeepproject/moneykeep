//! app-desktop.md revision 6 acceptance tests (47–51): password-protected books and
//! the Phase 2b leftovers.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::time::Duration;

use crate::common::*;
use mk_commands::{KdfParams, Session};
use mk_ui::ctl::App;
use mk_ui::{AppEvent, Screen, UserConfig};
use mk_views::testing::Harness;
use serde_json::json;

type H = Harness;

const PW: &str = "correct horse battery";

fn empty_app(recent: Vec<String>) -> App {
    App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        UserConfig {
            recent: recent.into_iter().map(Into::into).collect(),
            ..UserConfig::default()
        },
    )
}

fn type_secret(h: &mut H, label: &str, text: &str) {
    h.get_by_label(label).focus();
    h.run();
    h.get_by_label(label).type_text(text);
    h.run();
}

fn protected_book(dir: &std::path::Path) -> String {
    let path = dir.join("locked.moneykeep");
    let (mut s, _) =
        Session::create_encrypted(&path, "Locked", "USD", "test", PW, true, KdfParams::TEST)
            .unwrap();
    s.invoke(
        "account.create",
        json!({"name": "Checking", "type": "asset", "subtype": "bank"}),
    )
    .unwrap();
    path.to_str().unwrap().to_owned()
}

#[test]
fn app_desktop_at47_create_protected_book_and_recovery_modal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("home.moneykeep");
    let mut h = harness(empty_app(Vec::new()));
    click(&mut h, "Create a New Book");
    click(&mut h, "Protect with a password");
    assert!(has(&h, "A long passphrase of several words is best."));
    type_secret(&mut h, "Password", PW);
    type_secret(&mut h, "Confirm password", PW);
    click(&mut h, "Create book…");
    let evs = events(&mut h);
    assert!(
        matches!(evs.as_slice(), [AppEvent::ChooseNewBookPath { .. }]),
        "{evs:?}"
    );
    h.state_mut()
        .create_book(path.to_str().unwrap(), "Home", "USD");
    h.run();
    assert_eq!(h.state().screen(), Screen::Main);
    assert!(encrypted_on_disk(&path));
    assert!(has(&h, "Your recovery key"));
    assert!(has_contains(
        &h,
        "nobody — including MoneyKeep — can open the book"
    ));
    assert!(disabled(&h, "Done"), "Done waits for the checkbox");
    click(&mut h, "I've stored this key somewhere safe");
    assert!(!disabled(&h, "Done"));
    click(&mut h, "Done");
    assert!(!has(&h, "Your recovery key"));
    // Mismatched confirmation is refused before any file is made.
    let mut h = harness(empty_app(Vec::new()));
    click(&mut h, "Create a New Book");
    click(&mut h, "Protect with a password");
    type_secret(&mut h, "Password", PW);
    type_secret(&mut h, "Confirm password", "something else");
    click(&mut h, "Create book…");
    assert!(events(&mut h).is_empty());
    assert!(has(&h, "The two passwords are different."));
}

#[test]
fn app_desktop_at48_unlock_prompt_wrong_then_right() {
    let dir = tempfile::tempdir().unwrap();
    let path = protected_book(dir.path());
    let mut h = harness(empty_app(vec![path.clone()]));
    assert!(
        has_contains(&h, "Password protected"),
        "{:?}",
        labels_containing(&h, "·")
    );
    click(&mut h, "Open locked");
    // The prompt names the book (revision 32: no other text).
    assert!(has(&h, "locked") && has(&h, "Open"));
    assert!(has(&h, "Use recovery key instead"));
    type_secret(&mut h, "Password", "not the password");
    click(&mut h, "Open");
    h.run();
    assert!(has(&h, "Wrong password."));
    assert_eq!(h.state().screen(), Screen::Start, "the book stays closed");
    assert_eq!(value(&h, "Password"), "", "the field is cleared");
    type_secret(&mut h, "Password", PW);
    click(&mut h, "Open");
    h.run();
    assert_eq!(h.state().screen(), Screen::Main);
    open_pane(&mut h, "All accounts");
    assert!(has(&h, "Account Assets") || !labels_containing(&h, "Checking").is_empty());
    // Opening it again from another session is refused while it is open.
    let e = Session::open_with(
        std::path::Path::new(&path),
        "t",
        Some(&mk_commands::Secret::Password(PW.into())),
    )
    .unwrap_err();
    assert_eq!(e.code(), "book_in_use");
}

#[test]
fn app_desktop_at49_protect_from_settings() {
    let (app, book) = new_book_app();
    let mut h = harness(app);
    click(&mut h, "Settings");
    click(&mut h, "Security");
    assert!(has(&h, "This book is not password protected."));
    click(&mut h, "Protect with a password…");
    assert!(has_contains(
        &h,
        "Earlier unencrypted copies of this file may remain"
    ));
    type_secret(&mut h, "New password", PW);
    type_secret(&mut h, "Confirm new password", PW);
    click(&mut h, "Protect");
    assert!(has(&h, "Your recovery key"));
    click(&mut h, "I've stored this key somewhere safe");
    click(&mut h, "Done");
    assert!(encrypted_on_disk(&book.path));
    assert!(has(&h, "This book is password protected."));
    for action in [
        "Change password…",
        "New recovery key…",
        "Remove recovery key…",
        "Save an unencrypted copy…",
        "Remove password protection…",
    ] {
        assert!(has(&h, action), "{action}");
    }
    // Change the password through its dialog.
    click(&mut h, "Change password…");
    type_secret(&mut h, "Current password", "wrong wrong wrong");
    type_secret(&mut h, "New password", "a second passphrase");
    type_secret(&mut h, "Confirm new password", "a second passphrase");
    click(&mut h, "Save new password");
    assert!(has(&h, "That password didn't work."));
    type_secret(&mut h, "Current password", PW);
    click(&mut h, "Save new password");
    assert!(has(&h, "Password changed."));
    let hist = cmd(&mut h, "history.list", json!({"limit": 5}));
    assert_eq!(hist[0]["command"], "book.password_changed");
}

#[test]
fn app_desktop_at50_lock_after_idle() {
    let dir = tempfile::tempdir().unwrap();
    let path = protected_book(dir.path());
    let mut app = empty_app(vec![path.clone()]);
    app.config.lock_after_minutes = 5;
    let mut h = harness(app);
    click(&mut h, "Open locked");
    type_secret(&mut h, "Password", PW);
    click(&mut h, "Open");
    h.run();
    assert_eq!(h.state().screen(), Screen::Main);
    // Four minutes idle: still open.
    h.state_mut()
        .advance_clock_for_tests(Duration::from_secs(4 * 60));
    h.run();
    assert_eq!(h.state().screen(), Screen::Main);
    // Five minutes without input: closed, and the prompt for the same book.
    h.state_mut()
        .advance_clock_for_tests(Duration::from_secs(60));
    h.run();
    assert_eq!(h.state().screen(), Screen::Start);
    // The prompt names the book as its tile does (opened before: "Locked").
    assert!(has(&h, "Locked") && has(&h, "Use recovery key instead"));
    // The lock was released: another session can open it now.
    drop(
        Session::open_with(
            std::path::Path::new(&path),
            "t",
            Some(&mk_commands::Secret::Password(PW.into())),
        )
        .unwrap(),
    );
    // Lock after is a per-user choice in Settings.
    type_secret(&mut h, "Password", PW);
    click(&mut h, "Open");
    h.run();
    click(&mut h, "Settings");
    click(&mut h, "Security");
    assert!(has(&h, "Lock after"));
    assert_eq!(h.state().config.lock_after_minutes, 5);
}

#[test]
fn app_desktop_at51_net_worth_last_column_scrollbar_and_axis_labels() {
    // The account list pinned open beside the main area, as it was when this test
    // was written, so the periods do not fit.
    let (app, book) = new_book_app_with(pinned_config(iso_config()));
    std::mem::forget(book);
    let mut h = harness(app);
    h.set_size(mk_views::testing::vec2(1200.0, 800.0));
    h.state_mut().set_today("2026-10-19");
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Checking", "type": "asset", "subtype": "bank"}),
    );
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Visa", "type": "liability", "subtype": "credit-card"}),
    );
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Salary", "type": "income"}),
    );
    for (m, amount) in [
        ("01", "1500.00"),
        ("04", "2500.00"),
        ("07", "12345.67"),
        ("10", "2500.00"),
    ] {
        cmd(
            &mut h,
            "txn.create",
            json!({"date": format!("2026-{m}-05"), "splits": [
                {"account": "Checking", "value": amount},
                {"account": "Salary", "value": format!("-{amount}")}]}),
        );
    }
    h.run();
    click(&mut h, "Reports");
    click(&mut h, "Net worth");
    click(&mut h, "Period");
    click(&mut h, "This year");
    h.run();
    let window = mk_views::testing::Rect::from_min_size(
        mk_views::testing::Pos2::ZERO,
        mk_views::testing::vec2(1200.0, 800.0),
    );
    let oct19: Vec<mk_views::testing::Rect> =
        h.query_all_by_label("10-19").map(|n| n.rect()).collect();
    assert!(!oct19.is_empty());
    assert!(oct19.iter().all(|r| window.contains_rect(*r)), "{oct19:?}");
    assert!(
        has(&h, "Periods scroll sideways"),
        "a horizontal scroll bar is shown"
    );
    // Chart labels: every text drawn inside the chart is fully inside it and no two
    // overlap.
    let chart = h.get_by_label_contains("Net worth chart:").rect();
    let mut texts: Vec<(String, mk_views::testing::Rect)> = Vec::new();
    for c in &h.output().shapes {
        if c.kind == "text" && !c.text.trim().is_empty() {
            texts.push((c.text.clone(), c.rect));
        }
    }
    let inside: Vec<(String, mk_views::testing::Rect)> = texts
        .into_iter()
        .filter(|(_, r)| r.intersects(chart))
        .collect();
    assert!(inside.iter().any(|(t, _)| t == "10-19"), "{inside:?}");
    assert!(inside.iter().any(|(t, _)| t == "Jan"), "{inside:?}");
    for (t, r) in &inside {
        assert!(
            chart.expand(0.5).contains_rect(*r),
            "{t} at {r:?} clipped by {chart:?}"
        );
    }
    for (i, (a, ra)) in inside.iter().enumerate() {
        for (b, rb) in inside.iter().skip(i + 1) {
            assert!(
                !ra.shrink(0.5).intersects(rb.shrink(0.5)),
                "{a} {ra:?} overlaps {b} {rb:?}"
            );
        }
    }
}
