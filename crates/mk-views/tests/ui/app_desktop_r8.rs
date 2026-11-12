//! app-desktop.md revision 8 acceptance tests (59–61): per-kind amount columns
//! (unsigned), and remembered window state and panel widths.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::ctl::App;
use mk_ui::{PanelWidths, ScreenRect, UserConfig, WindowState, place_window};
use serde_json::json;

type H = mk_views::testing::Harness;

fn xs(h: &H, label: &str) -> mk_views::testing::Rect {
    h.get_by_label(label).rect()
}

fn overlaps(a: mk_views::testing::Rect, b: mk_views::testing::Rect) -> bool {
    a.min.x < b.max.x && b.min.x < a.max.x
}

/// Whether some node labelled `text` sits under the column headed `header`.
fn under(h: &H, text: &str, header: &str) -> bool {
    let hr = xs(h, header);
    h.query_all_by_label(text).any(|n| overlaps(hr, n.rect()))
}

#[test]
fn app_desktop_at59_credit_card_charge_then_payment_unsigned() {
    let (mut h, _book) = harness_with_chart();
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-02", "payee": "ALDI INC", "splits": [
        {"account": "Expenses:Groceries", "value": "35.94"}, {"account": "Liabilities:Visa", "value": "-35.94"}]}),
    );
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-10", "payee": "Card payment", "splits": [
        {"account": "Liabilities:Visa", "value": "1003.15"}, {"account": "Assets:Checking", "value": "-1003.15"}]}),
    );
    open_register(&mut h, "Liabilities:Visa");
    let (charge, payment) = (xs(&h, "Charge"), xs(&h, "Payment"));
    assert!(charge.max.x <= payment.min.x, "Charge comes first");
    assert!(under(&h, "35.94", "Charge"), "35.94 under Charge");
    assert!(under(&h, "1,003.15", "Payment"), "1,003.15 under Payment");
    assert!(!under(&h, "35.94", "Payment"));
    assert!(!under(&h, "1,003.15", "Charge"));
    // No minus sign in either amount column.
    for header in ["Charge", "Payment"] {
        for signed in ["-35.94", "-1,003.15"] {
            assert!(!under(&h, signed, header), "{signed} under {header}");
        }
    }
    // The row summaries use the column words, unsigned.
    assert!(has_contains(&h, "Charge 35.94"));
    assert!(has_contains(&h, "Payment 1003.15"));
    // The New transaction pop-up's fields (revision 24): Charge, then Payment.
    open_new(&mut h);
    let (c, p) = (xs(&h, "New charge"), xs(&h, "New payment"));
    assert!(
        c.max.y <= p.min.y,
        "pop-up Charge {c:?} before Payment {p:?}"
    );
    // Typing a charge records an increase of the amount owed.
    type_into(&mut h, "New payee", "Corner Grocery");
    type_into(&mut h, "New category", "Expenses:Groceries");
    type_into(&mut h, "New charge", "12.00");
    type_into(&mut h, "New payment", "3.00");
    assert_eq!(
        value(&h, "New charge"),
        "",
        "typing in one clears the other"
    );
    type_into(&mut h, "New charge", "12.00");
    assert_eq!(value(&h, "New payment"), "");
    click(&mut h, "Save");
    let reg = cmd(
        &mut h,
        "account.register",
        json!({"account": "Liabilities:Visa", "order": "asc", "offset": 0, "limit": 100}),
    );
    let last = reg["rows"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["payee"], "Corner Grocery", "{reg}");
    assert_eq!(last["increase"], true, "a charge raises the amount owed");
}

#[test]
fn app_desktop_at60_bank_spent_then_received_unsigned() {
    let (mut h, _book) = harness_with_chart();
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-01", "payee": "Employer", "splits": [
        {"account": "Assets:Checking", "value": "1000.00"}, {"account": "Salary", "value": "-1000.00"}]}),
    );
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-05", "payee": "Hardware Store", "splits": [
        {"account": "Expenses:Groceries", "value": "50.00"}, {"account": "Assets:Checking", "value": "-50.00"}]}),
    );
    open_register(&mut h, "Assets:Checking");
    let (spent, received) = (xs(&h, "Spent"), xs(&h, "Received"));
    assert!(spent.max.x <= received.min.x, "Spent comes first");
    assert!(under(&h, "50.00", "Spent"));
    assert!(under(&h, "1,000.00", "Received"));
    assert!(!under(&h, "-50.00", "Spent"), "no minus sign");
    assert!(has_contains(&h, "Spent 50.00"));
    open_new(&mut h);
    let (s, r) = (xs(&h, "New spent"), xs(&h, "New received"));
    assert!(s.max.y <= r.min.y, "Spent above Received in the pop-up");
}

#[test]
fn app_desktop_at61_window_and_panel_widths_round_trip() {
    let saved = WindowState {
        width: 1400,
        height: 900,
        x: Some(100),
        y: Some(80),
        maximized: false,
    };
    let config = UserConfig {
        window: Some(saved),
        panels: PanelWidths {
            sidebar: Some(320),
            ..PanelWidths::default()
        },
        ..UserConfig::default()
    };
    let text = serde_json::to_string(&config).unwrap();
    let back: UserConfig = serde_json::from_str(&text).unwrap();
    assert_eq!(back.window, Some(saved));
    assert_eq!(back.panels.sidebar, Some(320));
    // A config written before revision 8 still loads.
    let old: UserConfig = serde_json::from_str(r#"{"recent": [], "window": null}"#).unwrap();
    assert_eq!(old.window, None);
    assert_eq!(old.panels, PanelWidths::default());

    // Restored where it was on a 1920×1080 screen.
    let screen = [ScreenRect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    }];
    let p = place_window(&saved, &screen);
    assert_eq!(
        (p.width, p.height, p.position),
        (1400, 900, Some((100, 80)))
    );
    assert!(!p.maximized);
    // A second screen to the left still counts as on screen.
    let two = [
        screen[0],
        ScreenRect {
            x: -1600,
            y: 0,
            width: 1600,
            height: 900,
        },
    ];
    let left = WindowState {
        x: Some(-1500),
        ..saved
    };
    assert_eq!(place_window(&left, &two).position, Some((-1500, 80)));
    // Mostly off every screen: centred at the saved size.
    let lost = WindowState {
        x: Some(-5000),
        y: Some(-5000),
        ..saved
    };
    let p = place_window(&lost, &screen);
    assert_eq!((p.width, p.height), (1400, 900));
    assert_eq!(p.position, Some((260, 90)));
    // Shrunk to fit a smaller screen.
    let small = [ScreenRect {
        x: 0,
        y: 0,
        width: 1280,
        height: 720,
    }];
    let p = place_window(&lost, &small);
    assert_eq!((p.width, p.height, p.position), (1280, 720, Some((0, 0))));

    // The sidebar opens at its saved width (nothing new to remember) …
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.moneykeep");
    let mut app = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        back,
    );
    app.create_book(path.to_str().unwrap(), "Test Books", "USD");
    let mut h = harness(app);
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Checking", "type": "asset", "subtype": "bank"}),
    );
    h.run();
    assert_eq!(h.state().config.panels.sidebar, Some(320));
    // The accounts pane (revision 23) opens at that width, right of the 40-point rail.
    open_pane(&mut h, "All accounts");
    assert_eq!(h.state().config.panels.sidebar, Some(320));
    let books = h.get_by_label("Account Checking").rect();
    // The rail's right edge (its width is the theme's, not a fixed 40 points).
    let rail = h.get_by_label("All accounts").rect().right() + 1.0;
    assert!(
        books.max.x <= rail + 320.0 && books.min.x >= rail,
        "account list inside the 320-wide pane: {books:?}"
    );
    // … and Reset display settings forgets panel widths, not the window.
    click(&mut h, "Settings");
    click(&mut h, "Display");
    click(&mut h, "Reset display settings");
    h.run();
    assert_eq!(h.state().config.panels.sidebar, None);
    assert_eq!(h.state().config.window, Some(saved));
}
