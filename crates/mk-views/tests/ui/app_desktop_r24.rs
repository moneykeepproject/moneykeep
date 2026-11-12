//! app-desktop.md revision 24 acceptance tests (138–148): every edit in a pop-up,
//! the + New button and fast entry.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::{AppEvent, UserConfig};
use mk_views::testing::Key;
use mk_views::testing::{Role, Toggled};
use serde_json::json;

type H = mk_views::testing::Harness;

fn book() -> (H, TestBook) {
    book_with(iso_config())
}

fn book_with(config: UserConfig) -> (H, TestBook) {
    let (mut h, b) = harness_with_chart_config(config);
    h.state_mut().set_today("2026-10-22");
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-01", "payee": "Opening balance", "splits": [
        {"account": "Assets:Checking", "value": "1000.00"}, {"account": "Salary", "value": "-1000.00"}]}),
    );
    h.run();
    (h, b)
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

/// The register's account split of a transaction is index 1 in these books.
fn set_state(h: &mut H, id: &str, state: &str) {
    cmd(
        h,
        "txn.set_reconcile",
        json!({"splits": [{"id": id, "split": 1, "state": state}]}),
    );
    h.run();
}

fn txn_count(h: &mut H) -> usize {
    cmd(h, "txn.list", json!({})).as_array().unwrap().len()
}

fn editing(h: &H) -> bool {
    has(h, "Edit transaction")
}

fn adding(h: &H) -> bool {
    has(h, "New transaction")
}

fn fast_button<'h>(h: &'h H) -> mk_views::testing::Node<'h> {
    h.get_by_role_and_label(Role::Button, "Fast entry")
}

fn fast_state(h: &H) -> Option<Toggled> {
    fast_button(h).accesskit_node().toggled()
}

fn ctrl_enter(h: &mut H) {
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::Enter);
    h.run();
}

fn retype(h: &mut H, label: &str, text: &str) {
    h.get_by_label(label).focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label(label).type_text(text);
    h.run();
}

#[test]
fn app_desktop_at138_no_entry_row_and_no_popup_setting() {
    let (mut h, _b) = book();
    spend(&mut h, "2026-10-05", "Hardware Store", "Nails", "50.00");
    open_register(&mut h, "Assets:Checking");
    assert!(has(&h, "+ New"));
    for gone in [
        "Entry date",
        "Entry payee",
        "Entry spent",
        "Add entry",
        "Save entry",
    ] {
        assert!(!has(&h, gone), "{gone}: the register has no entry row");
    }
    // Only the two transaction rows.
    assert_eq!(labels_containing(&h, "Row ").len(), 2);
    click(&mut h, "Settings");
    click(&mut h, "Operation");
    assert!(has(&h, "Date format"));
    assert!(!has(&h, "Edit with Pop-Up"));
    assert!(!has_contains(&h, "edit it in a pop-up"));
    // An old config that still has the setting reads fine; the field is ignored.
    let old: UserConfig = serde_json::from_str(r#"{"recent": [], "edit_popup": false}"#).unwrap();
    assert!(!old.fast_entry);
    let saved = serde_json::to_string(&old).unwrap();
    assert!(!saved.contains("edit_popup"), "{saved}");
}

#[test]
fn app_desktop_at139_imported_cleared_row_opens_and_saves() {
    let (mut h, _b) = harness_with_chart();
    let bytes = fixture("ofx/checking-v1.ofx");
    h.state_mut().import_file("checking-v1.ofx", &bytes);
    h.run();
    click(&mut h, "Account for section 1");
    click(&mut h, "Assets:Checking");
    click(&mut h, "Add to books");
    open_register(&mut h, "Assets:Checking");
    // Imported rows are cleared.
    let row = labels_containing(&h, "Employer Payroll")
        .into_iter()
        .find(|l| l.starts_with("Row "))
        .unwrap();
    assert!(row.contains("status cleared"), "{row}");
    h.get_by_label_contains("Employer Payroll  · imported")
        .click();
    h.run();
    assert!(editing(&h), "a cleared row opens the pop-up");
    assert!(h.get_by_label("Edit payee").is_focused(), "Payee focused");
    assert_eq!(
        h.get_by_role_and_label(Role::RadioButton, "Cleared")
            .accesskit_node()
            .toggled(),
        Some(Toggled::True),
        "Status shows Cleared"
    );
    // Change the memo, then Enter (in the next single-line field) saves.
    retype(&mut h, "Edit memo", "October pay");
    key(&mut h, Key::Tab);
    key(&mut h, Key::Enter);
    assert!(!editing(&h), "saved and closed (fast entry off)");
    let t = cmd(&mut h, "txn.list", json!({"account": "Assets:Checking"}));
    let pay = t
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["payee"] == "Employer Payroll")
        .unwrap()
        .clone();
    assert_eq!(pay["memo"], "October pay");
    let audit = cmd(&mut h, "audit.list", json!({"limit": 1}));
    assert_eq!(audit[0]["command"], "txn.update", "{audit}");
    // Still cleared.
    assert!(
        pay["splits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["reconcile"] == "c"),
        "{pay}"
    );
}

#[test]
fn app_desktop_at140_reconciled_row_keeps_date_and_amounts() {
    let (mut h, _b) = book();
    let id = spend(&mut h, "2026-10-05", "Hardware Store", "Nails", "50.00");
    set_state(&mut h, &id, "r");
    open_register(&mut h, "Assets:Checking");
    h.get_by_label("Nails").click();
    h.run();
    assert!(editing(&h));
    assert!(has(
        &h,
        "This transaction is reconciled. Its date and amounts can't be changed here."
    ));
    assert!(disabled(&h, "Edit date"));
    assert!(disabled(&h, "Edit spent"));
    assert!(disabled(&h, "Edit received"));
    assert!(!disabled(&h, "Edit memo"));
    assert!(!disabled(&h, "Edit payee"));
    assert!(!disabled(&h, "Edit category"));
    assert!(
        h.get_by_label("Edit memo").is_focused(),
        "the clicked column"
    );
    // Status shows Reconciled, not selectable; no Delete; Void is offered.
    assert!(has(&h, "Reconciled"));
    assert!(disabled(&h, "Reconciled") && disabled(&h, "Cleared"));
    assert!(!has(&h, "Delete…"));
    assert!(!has(&h, "Split…"), "split editing is read-only");
    assert!(has(&h, "Void"));
    retype(&mut h, "Edit memo", "Nails and screws");
    ctrl_enter(&mut h);
    assert!(!editing(&h), "saved");
    let t = cmd(&mut h, "txn.get", json!({"id": id}));
    assert_eq!(t["memo"], "Nails and screws");
    assert_eq!(t["splits"][1]["reconcile"], "r", "{t}");
    assert!(has(&h, "Closing balance 950.00"));
}

#[test]
fn app_desktop_at141_voided_row_read_only_with_unvoid() {
    let (mut h, _b) = book();
    let id = spend(&mut h, "2026-10-05", "Hardware Store", "Nails", "50.00");
    cmd(&mut h, "txn.void", json!({"id": id}));
    h.run();
    open_register(&mut h, "Assets:Checking");
    h.get_by_label("Nails").click();
    h.run();
    assert!(editing(&h), "a voided row opens read-only");
    for f in [
        "Edit date",
        "Edit no.",
        "Edit payee",
        "Edit category",
        "Edit memo",
        "Edit spent",
        "Edit received",
        "Uncleared",
        "Cleared",
    ] {
        assert!(disabled(&h, f), "{f} read-only");
    }
    for gone in ["Save", "Cancel", "Delete…", "Void", "Split…"] {
        assert!(
            h.query_by_role_and_label(Role::Button, gone).is_none(),
            "no {gone} button"
        );
    }
    // The bottom-row Close and the title bar's ×.
    assert_eq!(
        h.query_all_by_role_and_label(Role::Button, "Close").count(),
        2
    );
    click(&mut h, "Unvoid");
    assert!(!editing(&h));
    let t = cmd(&mut h, "txn.get", json!({"id": id}));
    assert_eq!(t["status"], "posted", "{t}");
    assert!(has(&h, "Closing balance 950.00"));
    // Close just closes.
    cmd(&mut h, "txn.void", json!({"id": id}));
    h.run();
    h.get_by_label("Nails").click();
    h.run();
    close_button(&h).click();
    h.run();
    assert!(!editing(&h));
}

/// The voided pop-up's bottom-row Close button (the title bar's × is also named
/// "Close", revision 27).
fn close_button<'h>(h: &'h H) -> mk_views::testing::Node<'h> {
    h.query_all_by_role_and_label(Role::Button, "Close")
        .max_by(|a, b| a.rect().min.y.total_cmp(&b.rect().min.y))
        .unwrap()
}

#[test]
fn app_desktop_at142_new_button_rightmost_opens_new_transaction() {
    let (mut h, _b) = book();
    open_register(&mut h, "Assets:Checking");
    let new = h.get_by_label("+ New").rect();
    let heading = h.get_by_label("Assets:Checking").rect();
    assert!(
        new.min.y < heading.max.y && heading.min.y < new.max.y,
        "on the header line: {new:?} / {heading:?}"
    );
    for other in ["Filter", "From date", "To date", "Show scheduled"] {
        let r = h.get_by_label(other).rect();
        assert!(r.max.x < new.min.x, "{other} {r:?} left of + New {new:?}");
    }
    assert!(
        new.max.x >= h.get_by_label("Balance").rect().max.x - 1.0,
        "at the far right"
    );
    let tip = h.get_by_label("+ New");
    tip.hover();
    h.run_steps(4);
    assert!(has(&h, "Add a transaction (Ctrl+N)"));
    click(&mut h, "+ New");
    assert!(adding(&h));
    assert_eq!(value(&h, "New date"), "2026-10-22", "today");
    assert!(h.get_by_label("New date").is_focused());
    assert_eq!(value(&h, "New payee"), "");
    assert!(has(&h, "Uncleared"));
    assert!(has(&h, "Save") && has(&h, "Cancel"));
    assert!(!has(&h, "Delete…") && !has(&h, "Void"));
    click(&mut h, "Cancel");
    assert!(!adding(&h));
    // Ctrl+N and Insert do the same.
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::N);
    h.run();
    h.run();
    assert!(adding(&h));
    assert!(h.get_by_label("New date").is_focused());
    key(&mut h, Key::Escape);
    assert!(!adding(&h));
    key(&mut h, Key::Insert);
    h.run();
    assert!(adding(&h));
}

#[test]
fn app_desktop_at143_fast_entry_off_enter_creates_and_closes() {
    let (mut h, _b) = book();
    open_register(&mut h, "Assets:Checking");
    assert!(!h.state().config.fast_entry, "off by default");
    open_new(&mut h);
    fill_new(
        &mut h,
        &[
            ("payee", "Corner Grocery"),
            ("category", "Expenses:Groceries"),
            ("spent", "25.00"),
        ],
    );
    key(&mut h, Key::Enter);
    assert!(!adding(&h), "closed");
    assert_eq!(txn_count(&mut h), 2);
    assert!(has_contains(
        &h,
        "Corner Grocery Expenses:Groceries, Spent 25.00, status uncleared, balance 975.00"
    ));
    // The next one starts on the date just used.
    open_new(&mut h);
    assert_eq!(value(&h, "New date"), "2026-10-22");
}

#[test]
fn app_desktop_at144_fast_entry_three_in_a_row_and_three_undos() {
    let (mut h, _b) = book();
    h.state_mut().config.fast_entry = true;
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    for (i, (payee, amount)) in [("Bakery", "4.00"), ("Butcher", "12.00"), ("Grocer", "9.00")]
        .into_iter()
        .enumerate()
    {
        if i == 0 {
            fill_new(&mut h, &[("date", "2026-10-07")]);
        }
        fill_new(
            &mut h,
            &[
                ("payee", payee),
                ("category", "Expenses:Groceries"),
                ("spent", amount),
            ],
        );
        key(&mut h, Key::Enter);
        h.run();
        assert!(adding(&h), "the pop-up stays open");
        // A blank form, dated like the last, focus in Date.
        assert_eq!(value(&h, "New date"), "2026-10-07");
        for f in [
            "New no.",
            "New payee",
            "New category",
            "New memo",
            "New spent",
        ] {
            assert_eq!(value(&h, f), "", "{f} cleared");
        }
        assert!(h.get_by_label("New date").is_focused());
    }
    assert!(has(&h, "3 transactions added"));
    assert_eq!(txn_count(&mut h), 4);
    // The register updated behind the pop-up.
    assert!(has(&h, "Closing balance 975.00"));
    // An empty form closes without asking.
    key(&mut h, Key::Escape);
    assert!(!adding(&h));
    // Each saved transaction is its own undo step.
    for left in [3, 2, 1] {
        h.get_by_role_and_label(Role::Button, "Undo").click();
        h.run();
        assert_eq!(txn_count(&mut h), left, "one undo removes one");
    }
    assert!(has(&h, "Closing balance 1,000.00"));
}

#[test]
fn app_desktop_r24_fast_entry_discard_asks_first() {
    let (mut h, _b) = book();
    h.state_mut().config.fast_entry = true;
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    fill_new(&mut h, &[("payee", "Half typed")]);
    click(&mut h, "Cancel");
    assert!(adding(&h), "a partly filled form asks first");
    assert!(has(&h, "Discard this transaction?"));
    click(&mut h, "Keep editing");
    assert!(!has(&h, "Discard this transaction?"));
    assert_eq!(value(&h, "New payee"), "Half typed");
    // Esc asks; Esc again discards.
    h.state_mut().blur_for_tests();
    h.run();
    key(&mut h, Key::Escape);
    assert!(has(&h, "Discard this transaction?"));
    key(&mut h, Key::Escape);
    assert!(!adding(&h));
    assert_eq!(txn_count(&mut h), 1, "nothing saved");
    // The Discard button confirms too.
    open_new(&mut h);
    fill_new(&mut h, &[("payee", "Half typed")]);
    click(&mut h, "Cancel");
    click(&mut h, "Discard");
    assert!(!adding(&h));
}

#[test]
fn app_desktop_at145_fast_entry_toggle_tooltip_state_and_config() {
    let (mut h, _b) = book();
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    assert_eq!(fast_state(&h), Some(Toggled::False));
    fast_button(&h).hover();
    h.run_steps(4);
    assert!(has(
        &h,
        "Fast entry is off. Turn it on to add several transactions in a row: Enter saves \
         each one and a new blank one opens, without closing this window."
    ));
    fast_button(&h).click();
    h.run();
    assert_eq!(fast_state(&h), Some(Toggled::True));
    assert!(h.state().config.fast_entry);
    assert!(events(&mut h).contains(&AppEvent::ConfigChanged));
    fast_button(&h).hover();
    h.run_steps(4);
    assert!(has(
        &h,
        "Fast entry is on: Enter saves and starts the next transaction. Click to turn it off."
    ));
    // The state survives a config round trip.
    let saved = serde_json::to_string(&h.state().config).unwrap();
    let back: UserConfig = serde_json::from_str(&saved).unwrap();
    assert!(back.fast_entry);
    assert!(!UserConfig::default().fast_entry, "default off");
    // Ctrl+Shift+F toggles it while the pop-up is open, in Edit transaction too.
    key(&mut h, Key::Escape);
    spend(&mut h, "2026-10-05", "Hardware Store", "Nails", "50.00");
    h.run();
    h.get_by_label("Nails").click();
    h.run();
    assert!(editing(&h));
    assert_eq!(fast_state(&h), Some(Toggled::True));
    h.key_press_modifiers(
        mk_views::testing::Modifiers::COMMAND | mk_views::testing::Modifiers::SHIFT,
        Key::F,
    );
    h.run();
    assert_eq!(fast_state(&h), Some(Toggled::False));
    assert!(!h.state().config.fast_entry);
    assert!(editing(&h), "the shortcut does not close or save");
}

#[test]
fn app_desktop_at146_fast_entry_error_keeps_values_and_count() {
    let (mut h, _b) = book();
    cmd(
        &mut h,
        "book.set_closing_date",
        json!({"date": "2026-09-30"}),
    );
    h.state_mut().config.fast_entry = true;
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    fill_new(
        &mut h,
        &[
            ("payee", "Bakery"),
            ("category", "Expenses:Groceries"),
            ("spent", "4.00"),
        ],
    );
    key(&mut h, Key::Enter);
    assert!(has(&h, "1 transaction added"));
    fill_new(
        &mut h,
        &[
            ("date", "2026-09-15"),
            ("payee", "Late Bakery"),
            ("category", "Expenses:Groceries"),
            ("spent", "5.00"),
        ],
    );
    key(&mut h, Key::Enter);
    assert!(adding(&h), "stays open");
    assert!(has_contains(&h, "the books are closed through 2026-09-30"));
    assert_eq!(value(&h, "New date"), "2026-09-15");
    assert_eq!(value(&h, "New payee"), "Late Bakery");
    assert_eq!(value(&h, "New spent"), "5.00");
    assert!(has(&h, "1 transaction added"), "the count is unchanged");
    assert_eq!(txn_count(&mut h), 2);
}

#[test]
fn app_desktop_at147_fast_entry_edit_save_leaves_a_new_form() {
    let (mut h, _b) = book();
    let id = spend(&mut h, "2026-10-05", "Hardware Store", "Nails", "50.00");
    h.state_mut().config.fast_entry = true;
    open_register(&mut h, "Assets:Checking");
    h.get_by_label("Nails").click();
    h.run();
    assert!(editing(&h));
    retype(&mut h, "Edit memo", "Nails and screws");
    ctrl_enter(&mut h);
    h.run();
    assert!(!editing(&h));
    assert!(adding(&h), "a New transaction form follows");
    assert_eq!(
        value(&h, "New date"),
        "2026-10-05",
        "dated like the edited one"
    );
    for f in ["New payee", "New category", "New memo", "New spent"] {
        assert_eq!(value(&h, f), "", "{f} empty");
    }
    assert!(h.get_by_label("New date").is_focused());
    assert!(!has_contains(&h, "transactions added"));
    let t = cmd(&mut h, "txn.get", json!({"id": id}));
    assert_eq!(t["memo"], "Nails and screws");
}

#[test]
fn app_desktop_at148_space_and_delete_work_without_the_popup() {
    let (mut h, _b) = book_with(ascending(iso_config()));
    spend(&mut h, "2026-10-05", "Hardware Store", "Nails", "50.00");
    open_register(&mut h, "Assets:Checking");
    // Select with the keyboard (a click would open the pop-up).
    h.state_mut().blur_for_tests();
    h.run();
    key(&mut h, Key::ArrowDown);
    key(&mut h, Key::ArrowDown);
    assert!(has_contains(
        &h,
        "Hardware Store Expenses:Groceries, Spent 50.00, status uncleared"
    ));
    key(&mut h, Key::Space);
    assert!(!editing(&h), "no pop-up");
    assert!(has_contains(
        &h,
        "Hardware Store Expenses:Groceries, Spent 50.00, status cleared"
    ));
    key(&mut h, Key::Delete);
    assert!(!editing(&h));
    assert!(has(
        &h,
        "Delete this transaction? You can undo it afterwards."
    ));
    click(&mut h, "Delete transaction");
    assert!(!has(&h, "Nails"));
    assert_eq!(txn_count(&mut h), 1);
}
