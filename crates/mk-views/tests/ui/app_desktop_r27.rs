//! app-desktop.md revision 27 acceptance tests (170–174): sort transactions by
//! date, scroll bar width, scroll bars always visible, the pop-up's fast entry
//! toggle and × button.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_arithmetic
)]

use crate::common::*;
use mk_ui::{AppEvent, DateOrder, Theme, UserConfig, View};
use mk_views::testing::Key;
use mk_views::testing::{Role, Toggled};
use serde_json::json;

type H = mk_views::testing::Harness;

/// A book with the standard chart and three Checking transactions entered out of
/// date order: 10-01 (+1,000.00), 10-05 (−50.00), 10-03 (−20.00).
fn book_with(config: UserConfig) -> (H, TestBook) {
    let (mut h, b) = harness_with_chart_config(config);
    h.state_mut().set_today("2026-10-22");
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-01", "payee": "Opening balance", "splits": [
        {"account": "Assets:Checking", "value": "1000.00"}, {"account": "Salary", "value": "-1000.00"}]}),
    );
    spend(&mut h, "2026-10-05", "Hardware Store", "Nails", "50.00");
    spend(&mut h, "2026-10-03", "Corner Grocery", "Milk", "20.00");
    h.run();
    (h, b)
}

fn book() -> (H, TestBook) {
    book_with(iso_config())
}

fn spend(h: &mut H, date: &str, payee: &str, memo: &str, amount: &str) -> String {
    let t = cmd(
        h,
        "txn.create",
        json!({"date": date, "payee": payee, "memo": memo, "splits": [
        {"account": "Expenses:Groceries", "value": amount},
        {"account": "Assets:Checking", "value": format!("-{amount}")}]}),
    );
    t["id"].as_str().unwrap().to_owned()
}

/// The register's rows, top to bottom: (y of the row, its accessible summary).
fn rows_top_to_bottom(h: &H) -> Vec<String> {
    let mut rows: Vec<(f32, String)> = h
        .query_all_by_label_contains("Row ")
        // Register rows are table rows on Slint (buttons on the old toolkit).
        .filter(|n| n.accesskit_node().role() == Role::Row)
        .map(|n| {
            (
                n.rect().min.y,
                n.accesskit_node().label().unwrap_or_default(),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, l)| l).collect()
}

/// (date, balance) of each row, top to bottom.
fn dates_and_balances(h: &H) -> Vec<(String, String)> {
    rows_top_to_bottom(h)
        .iter()
        .map(|l| {
            let date = l.split_whitespace().nth(2).unwrap_or_default().to_owned();
            let balance = l.rsplit("balance ").next().unwrap_or_default().to_owned();
            (date, balance)
        })
        .collect()
}

fn operation(h: &mut H) {
    click(h, "Settings");
    settings_section(h, "Operation");
}

fn radio_on(h: &H, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().toggled() == Some(Toggled::True)
}

fn round_trip(config: &UserConfig) -> UserConfig {
    let saved = serde_json::to_string(config).unwrap();
    serde_json::from_str(&saved).unwrap()
}

#[test]
fn app_desktop_at170_descending_by_default_without_newest_first() {
    let (mut h, _b) = book();
    assert_eq!(UserConfig::default().date_order, DateOrder::Descending);
    // An old config without the field reads as Descending.
    let old: UserConfig = serde_json::from_str(r#"{"recent": []}"#).unwrap();
    assert_eq!(old.date_order, DateOrder::Descending);
    operation(&mut h);
    assert!(has(&h, "Sort transactions by date"));
    assert!(radio_on(&h, "Descending (newest first)"));
    assert!(!radio_on(&h, "Ascending (oldest first)"));
    open_register(&mut h, "Assets:Checking");
    assert_eq!(
        dates_and_balances(&h),
        vec![
            ("2026-10-05".to_owned(), "930.00".to_owned()),
            ("2026-10-03".to_owned(), "980.00".to_owned()),
            ("2026-10-01".to_owned(), "1000.00".to_owned()),
        ],
        "{:?}",
        rows_top_to_bottom(&h)
    );
    assert!(!has(&h, "Newest first"), "the checkbox is gone");
}

#[test]
fn app_desktop_at171_ascending_lists_oldest_first_and_round_trips() {
    let (mut h, _b) = book();
    operation(&mut h);
    click(&mut h, "Ascending (oldest first)");
    assert_eq!(h.state().config.date_order, DateOrder::Ascending);
    assert!(events(&mut h).contains(&AppEvent::ConfigChanged));
    assert!(radio_on(&h, "Ascending (oldest first)"));
    open_register(&mut h, "Assets:Checking");
    assert_eq!(
        dates_and_balances(&h),
        vec![
            ("2026-10-01".to_owned(), "1000.00".to_owned()),
            ("2026-10-03".to_owned(), "980.00".to_owned()),
            ("2026-10-05".to_owned(), "930.00".to_owned()),
        ]
    );
    let back = round_trip(&h.state().config);
    assert_eq!(back.date_order, DateOrder::Ascending);
    // Starting with the saved config lists oldest first too.
    let (mut h2, _b2) = book_with(back);
    open_register(&mut h2, "Assets:Checking");
    assert_eq!(
        dates_and_balances(&h2).first().map(|d| d.0.clone()),
        Some("2026-10-01".to_owned())
    );
}

#[test]
fn app_desktop_r27_order_change_keeps_selected_row() {
    let (mut h, _b) = book();
    open_register(&mut h, "Assets:Checking");
    // Select the 10-03 row (second from the top) with the keyboard.
    h.state_mut().blur_for_tests();
    h.run();
    key(&mut h, Key::ArrowDown);
    key(&mut h, Key::ArrowDown);
    let selected = |h: &H| {
        h.query_all_by_label_contains("Row ")
            .filter(|n| n.accesskit_node().toggled() == Some(Toggled::True))
            .map(|n| n.accesskit_node().label().unwrap_or_default())
            .collect::<Vec<_>>()
    };
    let before = selected(&h);
    assert_eq!(before.len(), 1);
    assert!(before[0].contains("2026-10-03"), "{before:?}");
    h.state_mut().config.date_order = DateOrder::Ascending;
    h.run();
    let after = selected(&h);
    assert_eq!(after.len(), 1);
    assert!(after[0].contains("2026-10-03"), "{after:?}");
}

#[test]
fn app_desktop_r27_ascending_opens_at_the_newest_row() {
    let config = UserConfig {
        date_order: DateOrder::Ascending,
        ..iso_config()
    };
    let (mut h, _b) = book_with(config);
    for d in 6..29 {
        spend(
            &mut h,
            &format!("2026-10-{d:02}"),
            "Corner Grocery",
            "",
            "1.00",
        );
    }
    open_register(&mut h, "Assets:Checking");
    h.run();
    // The newest (10-28) row is on screen, the oldest scrolled away above.
    // (On screen now: the harness's queries would scroll to find a row.)
    assert!(h.on_screen_contains("2026-10-28 Corner Grocery"));
    assert!(!h.on_screen_contains("2026-10-01 Opening balance"));
    // Descending opens at the top: the newest first.
    h.state_mut().config.date_order = DateOrder::Descending;
    h.state_mut().navigate(View::Home);
    h.run();
    open_register(&mut h, "Assets:Checking");
    h.run();
    let rows = rows_top_to_bottom(&h);
    assert!(rows[0].contains("2026-10-28"), "{rows:?}");
}

#[test]
fn app_desktop_r27_investment_register_follows_the_setting() {
    let (mut h, _b) = harness_with_chart();
    cmd(&mut h, "module.enable", json!({"module": "investments"}));
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    for (date, amount) in [
        ("2026-10-01", "100.00"),
        ("2026-10-05", "50.00"),
        ("2026-10-03", "20.00"),
    ] {
        cmd(
            &mut h,
            "txn.create",
            json!({"date": date, "splits": [
            {"account": "Assets:Brokerage", "value": amount},
            {"account": "Assets:Checking", "value": format!("-{amount}")}]}),
        );
    }
    let activity = |h: &H| {
        let mut v: Vec<(f32, String)> = h
            .query_all_by_label_contains("Activity 2026")
            .map(|n| {
                (
                    n.rect().min.y,
                    n.accesskit_node().label().unwrap_or_default(),
                )
            })
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v.into_iter().map(|(_, l)| l).collect::<Vec<_>>()
    };
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "Activity");
    let rows = activity(&h);
    assert_eq!(rows.len(), 3, "{rows:?}");
    // Newest first, with the running cash balance of each date.
    assert!(rows[0].contains("2026-10-05"), "{rows:?}");
    assert!(rows[1].contains("2026-10-03"), "{rows:?}");
    assert!(rows[2].contains("2026-10-01"), "{rows:?}");
    h.state_mut().config.date_order = DateOrder::Ascending;
    h.run();
    let rows = activity(&h);
    assert!(rows[0].contains("2026-10-01"), "{rows:?}");
    assert!(rows[2].contains("2026-10-05"), "{rows:?}");
}

/// The register's vertical scroll bar: the scroll bar node right of the Balance
/// header, level with the rows.
fn register_bar(h: &H) -> mk_views::testing::Rect {
    let balance = h.get_by_label("Balance").rect();
    h.query_all_by_role(Role::ScrollBar)
        .map(|n| n.rect())
        .filter(|r| r.min.x >= balance.max.x - 0.5 && r.min.y >= balance.min.y)
        .min_by(|a, b| a.min.y.total_cmp(&b.min.y))
        .unwrap_or_else(|| panic!("no register scroll bar"))
}

#[test]
fn app_desktop_at172_scroll_bar_width_setting() {
    let (mut h, _b) = book();
    click(&mut h, "Settings");
    settings_section(&mut h, "Display");
    assert!(h.query_all_by_label("Scroll bar width").count() >= 1);
    let slider = |h: &H| {
        h.get_by_role_and_label(Role::Slider, "Scroll bar width")
            .accesskit_node()
            .numeric_value()
    };
    assert_eq!(slider(&h), Some(14.0));
    assert_eq!(h.state().scroll_bar_width(), 14);
    // Ten steps to the right: 24.
    h.get_by_role_and_label(Role::Slider, "Scroll bar width")
        .focus();
    h.run();
    for _ in 0..10 {
        h.key_press(Key::ArrowRight);
        h.run();
    }
    assert_eq!(slider(&h), Some(24.0));
    assert_eq!(h.state().config.scroll_bar_width, 24);
    assert!(events(&mut h).contains(&AppEvent::ConfigChanged));
    for d in 6..29 {
        spend(
            &mut h,
            &format!("2026-10-{d:02}"),
            "Corner Grocery",
            "",
            "1.00",
        );
    }
    open_register(&mut h, "Assets:Checking");
    h.run();
    let bar = register_bar(&h);
    assert!((bar.width() - 24.0).abs() < 0.01, "24 points wide: {bar:?}");
    // The Balance column stays left of the wider bar (revision 9).
    assert!(h.get_by_label("Balance").rect().max.x <= bar.min.x + 0.5);
    // A config round trip keeps it.
    let back = round_trip(&h.state().config);
    assert_eq!(back.scroll_bar_width, 24);
    // Old configs read the default; values outside 6–30 are clamped.
    let old: UserConfig = serde_json::from_str(r#"{"recent": []}"#).unwrap();
    assert_eq!(old.scroll_bar_width, 14);
    for (saved, used) in [(2, 6), (100, 30), (6, 6), (30, 30)] {
        let (mut h, _b) = harness_with_chart_config(UserConfig {
            scroll_bar_width: saved,
            ..iso_config()
        });
        assert_eq!(h.state().scroll_bar_width(), used);
        for d in 6..29 {
            spend(
                &mut h,
                &format!("2026-10-{d:02}"),
                "Corner Grocery",
                "",
                "1.00",
            );
        }
        open_register(&mut h, "Assets:Checking");
        h.run();
        assert!(
            (register_bar(&h).width() - used as f32).abs() < 0.01,
            "{saved} drawn as {used}"
        );
    }
    // Reset display settings restores 14.
    click(&mut h, "Settings");
    settings_section(&mut h, "Display");
    click(&mut h, "Reset display settings");
    assert_eq!(h.state().config.scroll_bar_width, 14);
    assert_eq!(slider(&h), Some(14.0));
}

/// sRGB relative luminance contrast of two 0xRRGGBB colours (WCAG).
#[allow(clippy::float_arithmetic)]
fn contrast(a: u32, b: u32) -> f32 {
    let lum = |c: u32| {
        let ch = |v: u32| {
            let v = (v & 0xff) as f32 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(c >> 16) + 0.7152 * ch(c >> 8) + 0.0722 * ch(c)
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

#[test]
fn app_desktop_at173_scroll_bar_visible_without_hover_with_contrast() {
    for theme in [Theme::Light, Theme::Dark] {
        let config = UserConfig {
            theme,
            ..iso_config()
        };
        let (mut h, _b) = book_with(config);
        for d in 6..29 {
            spend(
                &mut h,
                &format!("2026-10-{d:02}"),
                "Corner Grocery",
                "",
                "1.00",
            );
        }
        let id = cmd(&mut h, "account.get", json!({"account": "Assets:Checking"}))["id"]
            .as_str()
            .unwrap()
            .to_owned();
        h.remove_cursor();
        h.state_mut().navigate(View::Register { account: id });
        // The first frame of the register, the pointer nowhere near it: the bar
        // is drawn (a solid track with a handle), not only on hover.
        h.step();
        let bar = register_bar(&h);
        assert!(bar.height() > 100.0, "{theme:?}: {bar:?}");
        assert_eq!(h.is_dark(), theme == Theme::Dark);
        let (track, handle) = (
            h.theme_color("scroll-track"),
            h.theme_color("scroll-handle"),
        );
        let ratio = contrast(handle, track);
        assert!(ratio >= 3.0, "{theme:?}: handle/track contrast {ratio:.2}");
        assert!(
            contrast(track, h.theme_color("surface")) > 1.0,
            "the track differs from the panel"
        );
    }
}

#[test]
fn app_desktop_at174_fast_entry_left_of_save_and_close_button() {
    let (mut h, _b) = book();
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    let fast = h.get_by_role_and_label(Role::Button, "Fast entry").rect();
    let save = h.get_by_label("Save").rect();
    let cancel = h.get_by_label("Cancel").rect();
    // Immediately left of Save, on the bottom row, as high as the buttons.
    assert!(fast.max.x <= save.min.x, "{fast:?} left of {save:?}");
    assert!(
        save.min.x - fast.max.x < 12.0,
        "immediately left: {fast:?} {save:?}"
    );
    assert!(save.max.x <= cancel.min.x);
    assert!((fast.center().y - save.center().y).abs() < 1.0);
    assert!(
        (fast.height() - save.height()).abs() < 1.0,
        "{fast:?} {save:?}"
    );
    // The × is in the title bar's top-right corner, above the fields.
    let close = h.get_by_role_and_label(Role::Button, "Close").rect();
    let title = h.get_by_label("New transaction").rect();
    assert!(close.min.x > title.max.x);
    assert!(close.max.y <= h.get_by_label("New date").rect().min.y);
    assert!(close.max.x >= cancel.max.x - 1.0, "at the right edge");
    // Tooltip "Close" on hover.
    assert_eq!(h.query_all_by_label("Close").count(), 1);
    h.get_by_role_and_label(Role::Button, "Close").hover();
    h.run_steps(4);
    assert_eq!(h.query_all_by_label("Close").count(), 2, "the tooltip");
    // × closes like Cancel.
    h.get_by_role_and_label(Role::Button, "Close").click();
    h.run();
    assert!(!has(&h, "New transaction"));
    // With fast entry on and a partly filled form it asks first.
    h.state_mut().config.fast_entry = true;
    open_new(&mut h);
    fill_new(&mut h, &[("payee", "Half typed")]);
    h.get_by_role_and_label(Role::Button, "Close").click();
    h.run();
    assert!(has(&h, "New transaction"));
    assert!(has(&h, "Discard this transaction?"));
    click(&mut h, "Discard");
    assert!(!has(&h, "New transaction"));
    assert_eq!(
        cmd(&mut h, "txn.list", json!({})).as_array().unwrap().len(),
        3,
        "nothing saved"
    );
    // Edit transaction: the same row and ×.
    h.get_by_label("Nails").click();
    h.run();
    assert!(has(&h, "Edit transaction"));
    let fast = h.get_by_role_and_label(Role::Button, "Fast entry").rect();
    let save = h.get_by_label("Save").rect();
    assert!(fast.max.x <= save.min.x && save.min.x - fast.max.x < 12.0);
    h.get_by_role_and_label(Role::Button, "Close").click();
    h.run();
    assert!(!has(&h, "Edit transaction"));
}

#[test]
fn app_desktop_r27_split_editor_close_button_is_its_cancel() {
    let (mut h, _b) = book();
    open_register(&mut h, "Assets:Checking");
    h.get_by_label("Nails").click();
    h.run();
    click(&mut h, "Split…");
    assert!(has(&h, "Split transaction"));
    // Two × buttons: the pop-up's and the split editor's (level with its title).
    let title = h.get_by_label("Split transaction").rect();
    let split_close = h
        .query_all_by_role(Role::Button)
        .filter(|n| n.accesskit_node().label().as_deref() == Some("Close"))
        .find(|n| {
            let r = n.rect();
            r.min.y < title.max.y && title.min.y < r.max.y
        })
        .unwrap_or_else(|| panic!("no × on the split editor's title bar"));
    // At the editor's right edge, which stays as wide as its lines.
    let add = h.get_by_label("Add line").rect();
    let line = h.get_by_label("Line 1 amount").rect();
    let close = split_close.rect();
    assert!(close.max.x >= line.max.x - 1.0, "{close:?} {line:?}");
    assert!(
        close.max.x - add.min.x < 900.0,
        "not stretched: {close:?} {add:?}"
    );
    split_close.click();
    h.run();
    assert!(!has(&h, "Split transaction"), "the split editor closed");
    assert!(has(&h, "Edit transaction"), "the pop-up stays open");
}
