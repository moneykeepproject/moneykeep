//! app-desktop.md revision 20 (date format, acceptance tests 115–123) and revision 21
//! (imported dates are transaction dates, 125–126). Test 124 is in the CLI's tests.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::{AppEvent, UserConfig};
use mk_views::testing::Key;
use serde_json::json;

type H = mk_views::testing::Harness;

fn config(format: &str) -> UserConfig {
    UserConfig {
        date_format: format.into(),
        ..UserConfig::default()
    }
}

/// A harness on 2026-10-22 with the standard chart and the given date format.
fn book(format: Option<&str>) -> (H, TestBook) {
    let cfg = format.map_or_else(UserConfig::default, config);
    let (mut h, b) = harness_with_chart_config(cfg);
    h.state_mut().set_today("2026-10-22");
    h.run();
    (h, b)
}

fn spend(h: &mut H, date: &str, payee: &str) {
    cmd(
        h,
        "txn.create",
        json!({"date": date, "payee": payee, "splits": [
        {"account": "Expenses:Groceries", "value": "10.00"},
        {"account": "Assets:Checking", "value": "-10.00"}]}),
    );
    h.run();
}

fn dates(h: &mut H) -> Vec<String> {
    let mut d: Vec<String> = cmd(h, "txn.list", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["date"].as_str().unwrap().to_owned())
        .collect();
    d.sort();
    d
}

fn audit_len(h: &mut H) -> usize {
    cmd(h, "audit.list", json!({"limit": 1000}))
        .as_array()
        .unwrap()
        .len()
}

/// Type a date in the New transaction pop-up and save a 5.00 grocery entry.
fn enter_dated(h: &mut H, date: &str) {
    add_txn(
        h,
        &[
            ("date", date),
            ("payee", "Bakery"),
            ("category", "Expenses:Groceries"),
            ("spent", "5.00"),
        ],
    );
    h.run();
}

fn choose_format(h: &mut H, option: &str) {
    click(h, "Settings");
    click(h, "Operation");
    click(h, "Date format");
    // The list scrolls to the choice first.
    h.get_by_label_contains(option).scroll_to_me();
    h.run();
    h.get_by_label_contains(option).click();
    h.run();
}

#[test]
fn app_desktop_at115_default_format_mm_dd_yyyy() {
    let (mut h, _b) = book(None);
    assert_eq!(h.state().config.date_format, "MM-DD-YYYY");
    click(&mut h, "Settings");
    click(&mut h, "Operation");
    assert!(has(&h, "Date format"));
    assert_eq!(
        value(&h, "Date format"),
        "MM-DD-YYYY",
        "the drop-down shows the default"
    );
    assert!(has_contains(&h, "How dates are shown and typed."));
    spend(&mut h, "2026-10-22", "Corner Grocery");
    open_register(&mut h, "Assets:Checking");
    assert!(has(&h, "10-22-2026"), "{:?}", labels_containing(&h, "2026"));
    assert!(!has(&h, "2026-10-22"));
}

#[test]
fn app_desktop_at116_change_rerenders_without_a_book_write_and_persists() {
    let (mut h, _b) = book(None);
    spend(&mut h, "2026-10-22", "Corner Grocery");
    let before = audit_len(&mut h);
    choose_format(&mut h, "DD Mon YYYY  22 Oct 2026");
    assert_eq!(h.state().config.date_format, "DD Mon YYYY");
    assert!(events(&mut h).contains(&AppEvent::ConfigChanged));
    open_register(&mut h, "Assets:Checking");
    assert!(has(&h, "22 Oct 2026"));
    assert_eq!(audit_len(&mut h), before, "no book write");
    // The app shell saves the config; a restart reads it back.
    let saved = serde_json::to_string(&h.state().config).unwrap();
    let back: UserConfig = serde_json::from_str(&saved).unwrap();
    assert_eq!(back.date_format, "DD Mon YYYY");
    // Older configs without the setting get the default.
    let old: UserConfig = serde_json::from_str(r#"{"recent": []}"#).unwrap();
    assert_eq!(old.date_format, "MM-DD-YYYY");
}

#[test]
fn app_desktop_at117_typing_dates_under_mm_dd_yyyy() {
    let (mut h, _b) = book(None);
    open_register(&mut h, "Assets:Checking");
    enter_dated(&mut h, "1/5/26");
    assert_eq!(dates(&mut h), ["2026-01-05"]);
    assert!(has(&h, "01-05-2026"));
    enter_dated(&mut h, "2026-01-05");
    assert_eq!(dates(&mut h), ["2026-01-05", "2026-01-05"]);
    let n = audit_len(&mut h);
    enter_dated(&mut h, "13-01-2026");
    assert!(has(&h, "Enter the date as MM-DD-YYYY"));
    assert_eq!(audit_len(&mut h), n, "nothing saved");
    assert_eq!(dates(&mut h).len(), 2);
}

#[test]
fn app_desktop_at118_day_first() {
    let (mut h, _b) = book(Some("DD/MM/YYYY"));
    open_register(&mut h, "Assets:Checking");
    enter_dated(&mut h, "05/01/2026");
    assert_eq!(dates(&mut h), ["2026-01-05"]);
    assert!(has(&h, "05/01/2026"));
}

#[test]
fn app_desktop_at119_month_names() {
    let (mut h, _b) = book(Some("Mon DD, YYYY"));
    open_register(&mut h, "Assets:Checking");
    enter_dated(&mut h, "oct 22 2026");
    enter_dated(&mut h, "October 22, 2026");
    assert_eq!(dates(&mut h), ["2026-10-22", "2026-10-22"]);
    assert_eq!(h.query_all_by_label("Oct 22, 2026").count(), 2);
}

#[test]
fn app_desktop_at120_no_year_is_the_current_year() {
    let (mut h, _b) = book(None);
    open_register(&mut h, "Assets:Checking");
    enter_dated(&mut h, "10-22");
    assert_eq!(dates(&mut h), ["2026-10-22"]);
}

#[test]
fn app_desktop_at121_format_guide() {
    let (mut h, _b) = book(None);
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    h.get_by_label("New date").focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.key_press(Key::Backspace);
    h.run();
    assert_eq!(value(&h, "New date"), "");
    let node = h.get_by_label("New date");
    assert_eq!(
        node.accesskit_node().placeholder().as_deref(),
        Some("MM-DD-YYYY"),
        "greyed pattern"
    );
    assert_eq!(
        node.accesskit_node().description().as_deref(),
        Some("Date, format MM-DD-YYYY")
    );
    h.get_by_label("New date").type_text("10-2");
    h.run();
    assert_eq!(h.date_guide(), "D-YYYY");
    assert_eq!(value(&h, "New date"), "10-2", "the guide is not the value");
    // Text that no longer fits the pattern hides the guide.
    h.get_by_label("New date").type_text("x");
    h.run();
    assert_eq!(h.date_guide(), "");
}

#[test]
fn app_desktop_at122_plus_moves_the_date_in_the_chosen_format() {
    let (mut h, _b) = book(Some("DD.MM.YYYY"));
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    h.get_by_label("New date").focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label("New date").type_text("31.12.2026");
    h.run();
    h.get_by_label("New date").type_text("+");
    h.run();
    assert_eq!(value(&h, "New date"), "01.01.2027");
    h.get_by_label("New date").type_text("-");
    h.run();
    assert_eq!(value(&h, "New date"), "31.12.2026");
}

#[test]
fn app_desktop_at123_long_dates_fit_at_200_percent() {
    let cfg = UserConfig {
        text_size: 200,
        ..config("Month DD, YYYY")
    };
    let (mut h, _b) = harness_with_chart_config(cfg);
    h.state_mut().set_today("2026-10-22");
    spend(&mut h, "2026-09-30", "Corner Grocery");
    open_register(&mut h, "Assets:Checking");
    h.run();
    let node = h.get_by_label("September 30, 2026");
    // Node rects and text layout are both in points.
    let shown = node.rect().width();
    // The text's natural width at 200 % (the testing backend's fixed metrics).
    let needed = h.text_width("September 30, 2026", 14.0);
    assert!(
        shown + 0.5 >= needed,
        "the date is clipped: {shown} shown of {needed}"
    );
}

// ----- revision 21 ------------------------------------------------------------------

fn ofx_fixture() -> String {
    let p = format!(
        "{}/../../examples/fixtures/ofx/creditcard-v1.ofx",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(p).unwrap()
}

#[test]
fn app_desktop_at125_import_date_to_use() {
    let (mut h, _b) = book(None);
    click(&mut h, "Settings");
    click(&mut h, "Import");
    assert_eq!(value(&h, "Date to use"), "Transaction date when given");
    click(&mut h, "Date to use");
    click(&mut h, "Posting date");
    let v = cmd(
        &mut h,
        "setting.get",
        json!({"module": "core", "key": "import.date_source"}),
    );
    assert_eq!(v["value"], "posted");
}

#[test]
fn app_desktop_at126_reimport_with_dtuser_shows_date_corrections() {
    let (mut h, _b) = book(None);
    let first = cmd(
        &mut h,
        "import.stage",
        json!({"format": "ofx", "content": ofx_fixture(), "account": "Liabilities:Visa"}),
    );
    cmd(&mut h, "import.commit", json!({"batch": first["id"]}));
    let with_user = ofx_fixture().replace(
        "<DTPOSTED>20261020</DTPOSTED>",
        "<DTPOSTED>20261020</DTPOSTED><DTUSER>20261018</DTUSER>",
    );
    let again = cmd(
        &mut h,
        "import.stage",
        json!({"format": "ofx", "content": with_user, "account": "Liabilities:Visa"}),
    );
    let batch = again["id"].as_str().unwrap().to_owned();
    h.state_mut().open_view_spec(&format!("import:{batch}"));
    h.run();
    assert!(
        has(&h, "Date corrected"),
        "{:?}",
        labels_containing(&h, "Row")
    );
    // The arrow when the font has it (the test font does not), else "to".
    assert!(
        has(&h, "10-20-2026 → 10-18-2026") || has(&h, "10-20-2026 to 10-18-2026"),
        "{:?}",
        labels_containing(&h, "10-18-2026")
    );
    let before = dates(&mut h);
    assert!(before.contains(&"2026-10-20".to_owned()));
    click(&mut h, "Add to books");
    h.run();
    let after = dates(&mut h);
    assert!(after.contains(&"2026-10-18".to_owned()), "{after:?}");
    assert!(!after.contains(&"2026-10-20".to_owned()));
    let entry = cmd(&mut h, "audit.list", json!({"limit": 1}));
    assert_eq!(entry[0]["command"], "import.commit");
    cmd(&mut h, "history.undo", json!({}));
    assert_eq!(dates(&mut h), before);
}
