//! app-desktop.md revision 9 acceptance tests (63–69): the edit pop-up and the
//! register's scroll bar gutter. Revision 24 removed the Edit with Pop-Up setting
//! and opens every row in the pop-up, so tests 65 (a cleared row only selects) and
//! 66 (the setting off) are gone; their replacements are in `app_desktop_r24.rs`.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_views::testing::Key;
use serde_json::json;

type H = mk_views::testing::Harness;

fn rect(h: &H, label: &str) -> mk_views::testing::Rect {
    h.get_by_label(label).rect()
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

fn opening(h: &mut H) {
    cmd(
        h,
        "txn.create",
        json!({"date": "2026-10-01", "payee": "Opening balance", "splits": [
        {"account": "Assets:Checking", "value": "1000.00"}, {"account": "Salary", "value": "-1000.00"}]}),
    );
}

fn popup_open(h: &H) -> bool {
    has(h, "Edit transaction")
}

fn row_selected(h: &H, part: &str) -> Option<bool> {
    let node = h.get_by_label_contains(part);
    let n = node.accesskit_node();
    n.is_selected()
        .or_else(|| n.toggled().map(|t| t == mk_views::testing::Toggled::True))
}

#[test]
fn app_desktop_at63_click_memo_opens_popup_with_memo_focused() {
    let (mut h, _book) = harness_with_chart();
    opening(&mut h);
    spend(&mut h, "2026-10-05", "Hardware Store", "Nails", "50.00");
    open_register(&mut h, "Assets:Checking");
    assert!(!popup_open(&h));
    h.get_by_label("Nails").click();
    h.run();
    assert!(popup_open(&h), "the pop-up opened");
    assert!(h.get_by_label("Edit memo").is_focused());
    assert_eq!(value(&h, "Edit memo"), "Nails");
    assert_eq!(row_selected(&h, "Hardware Store Expenses"), Some(true));
    // One field per line, in order, sharing a left edge.
    let order = [
        "Edit date",
        "Edit no.",
        "Edit payee",
        "Edit category",
        "Edit memo",
        "Edit spent",
        "Edit received",
        "Uncleared",
    ];
    let rects: Vec<mk_views::testing::Rect> = order.iter().map(|l| rect(&h, l)).collect();
    for (w, l) in rects.windows(2).zip(order.iter().skip(1)) {
        assert!(w[0].max.y <= w[1].min.y + 0.5, "{l} below the field before");
    }
    let left = rects[0].min.x;
    for (r, l) in rects.iter().zip(order) {
        assert!(
            (r.min.x - left).abs() < 1.5,
            "{l} {r:?} shares the left edge {left}"
        );
    }
    assert_eq!(value(&h, "Edit payee"), "Hardware Store");
    assert_eq!(value(&h, "Edit category"), "Expenses:Groceries");
    assert_eq!(value(&h, "Edit spent"), "50.00");
    for b in ["Save", "Cancel", "Delete…", "Void"] {
        assert!(has(&h, b), "{b}");
    }
    // Exactly one focus ring.
    assert_eq!(h.query_all_by(|n| n.is_focused()).count(), 1);
    // Esc cancels; the row stays selected.
    key(&mut h, Key::Escape);
    assert!(!popup_open(&h));
    assert_eq!(row_selected(&h, "Hardware Store Expenses"), Some(true));
    // Enter on the selected uncleared row opens it again, at Payee.
    key(&mut h, Key::Enter);
    assert!(popup_open(&h));
    assert!(h.get_by_label("Edit payee").is_focused());
}

#[test]
fn app_desktop_at64_change_amount_enter_saves_and_undo_reverts() {
    let (mut h, _book) = harness_with_chart();
    opening(&mut h);
    let id = spend(&mut h, "2026-10-05", "Hardware Store", "Nails", "50.00");
    open_register(&mut h, "Assets:Checking");
    assert!(has(&h, "Closing balance 950.00"));
    // A click on the amount starts in that field with its text selected.
    let spent = rect(&h, "Spent");
    h.query_all_by_label("50.00")
        .find(|n| n.rect().min.x < spent.max.x && spent.min.x < n.rect().max.x)
        .unwrap()
        .click();
    h.run();
    assert!(popup_open(&h));
    assert!(h.get_by_label("Edit spent").is_focused());
    h.get_by_label("Edit spent").type_text("75.00");
    h.run();
    assert_eq!(value(&h, "Edit spent"), "75.00", "typing replaced the text");
    key(&mut h, Key::Enter);
    assert!(!popup_open(&h), "saved and closed");
    let t = cmd(&mut h, "txn.get", json!({"id": id}));
    assert!(t["splits"].to_string().contains("75"), "{t}");
    assert!(has(&h, "Closing balance 925.00"));
    assert!(has_contains(&h, "Spent 75.00"));
    h.state_mut().blur_for_tests();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::Z);
    h.run();
    assert!(has(&h, "Closing balance 950.00"));
    assert!(has_contains(&h, "Spent 50.00"));
}

#[test]
fn app_desktop_at67_credit_card_popup_charge_then_payment() {
    let (mut h, _book) = harness_with_chart();
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-02", "payee": "ALDI INC", "memo": "Food", "splits": [
        {"account": "Expenses:Groceries", "value": "35.94"}, {"account": "Liabilities:Visa", "value": "-35.94"}]}),
    );
    open_register(&mut h, "Liabilities:Visa");
    h.get_by_label("Food").click();
    h.run();
    assert!(popup_open(&h));
    let (c, p) = (rect(&h, "Edit charge"), rect(&h, "Edit payment"));
    assert!(c.max.y <= p.min.y, "Charge above Payment");
    assert_eq!(value(&h, "Edit charge"), "35.94");
    assert_eq!(value(&h, "Edit payment"), "");
}

#[test]
fn app_desktop_at68_command_error_stays_open_with_typed_values() {
    let (mut h, _book) = harness_with_chart();
    opening(&mut h);
    spend(&mut h, "2026-10-05", "Hardware Store", "Nails", "50.00");
    cmd(
        &mut h,
        "book.set_closing_date",
        json!({"date": "2026-09-30"}),
    );
    open_register(&mut h, "Assets:Checking");
    h.get_by_label("Nails").click();
    h.run();
    h.get_by_label("Edit date").focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label("Edit date").type_text("2026-09-15");
    h.run();
    h.get_by_label("Edit memo").focus();
    h.run();
    h.get_by_label("Edit memo").type_text(" and screws");
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::Enter);
    h.run();
    assert!(popup_open(&h), "stays open");
    let errs = labels_containing(&h, "the books are closed through 2026-09-30");
    assert_eq!(errs.len(), 1, "the error shows in the pop-up");
    assert_eq!(value(&h, "Edit date"), "2026-09-15");
    assert!(value(&h, "Edit memo").contains("and screws"));
    // The error sits under the Date field.
    let (date, err, number) = (
        rect(&h, "Edit date"),
        h.get_by_label_contains("the books are closed").rect(),
        rect(&h, "Edit no."),
    );
    assert!(date.max.y <= err.min.y && err.max.y <= number.min.y);
    // Nothing changed.
    assert!(has(&h, "Closing balance 950.00"));
    click(&mut h, "Cancel");
    assert!(!popup_open(&h));
}

#[test]
fn app_desktop_at69_balance_left_of_scroll_bar_at_1000x700() {
    let (mut h, _book) = harness_with_chart();
    let size = mk_views::testing::vec2(1000.0, 700.0);
    h.set_size(size);
    opening(&mut h);
    for d in 2..30 {
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
    let ppp = h.zoom();
    // The user's scroll bar width (revision 27; default 14).
    let bar = h.state().scroll_bar_width() as f32 * ppp;
    // The bar sits at the right edge of the register; a full bar's width must be
    // free between every Balance cell and the window's right edge.
    let bar_left = size.x * ppp - bar;
    let balance = rect(&h, "Balance");
    assert!(
        balance.max.x <= bar_left + 0.5,
        "Balance header {balance:?} left of the scroll bar at {bar_left}"
    );
    // The newest row (28 spends of 1.00) is at the top, newest first (revision 27).
    let cells: Vec<mk_views::testing::Rect> =
        h.query_all_by_label("972.00").map(|n| n.rect()).collect();
    assert!(!cells.is_empty());
    for c in cells {
        assert!(c.max.x <= bar_left + 0.5, "balance cell {c:?}");
    }
}

#[test]
fn app_desktop_r9_popup_status_void_delete_and_splits() {
    let (mut h, _book) = harness_with_chart();
    opening(&mut h);
    let id = spend(&mut h, "2026-10-05", "Hardware Store", "Nails", "50.00");
    open_register(&mut h, "Assets:Checking");
    // Status → Cleared saves through txn.set_reconcile.
    h.get_by_label("Nails").click();
    h.run();
    click(&mut h, "Cleared");
    click(&mut h, "Save");
    assert!(!popup_open(&h));
    assert!(has_contains(
        &h,
        "Hardware Store Expenses:Groceries, Spent 50.00, status cleared"
    ));
    // A cleared row opens too (revision 24); Uncleared saves back to `n`.
    h.get_by_label("Nails").click();
    h.run();
    assert!(popup_open(&h));
    click(&mut h, "Uncleared");
    click(&mut h, "Save");
    assert!(has_contains(&h, "Spent 50.00, status uncleared"));
    let t = cmd(&mut h, "txn.get", json!({"id": id}));
    assert_eq!(t["splits"][1]["reconcile"], "n", "{t}");
    // Void.
    h.get_by_label("Nails").click();
    h.run();
    click(&mut h, "Void");
    assert!(!popup_open(&h));
    assert!(has_contains(&h, "status void"));
    // Delete asks first.
    let id2 = spend(&mut h, "2026-10-06", "Bakery", "Bread", "4.00");
    h.run();
    h.get_by_label("Bread").click();
    h.run();
    click(&mut h, "Delete…");
    assert!(popup_open(&h));
    click(&mut h, "Delete transaction");
    assert!(!popup_open(&h));
    assert!(!has(&h, "Bread"));
    let _ = id2;
    // A split transaction shows "Split (2)" and opens the split editor.
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-07", "payee": "Market", "memo": "Mixed", "splits": [
        {"account": "Expenses:Groceries", "value": "6.00"},
        {"account": "Expenses:Groceries", "value": "4.00"},
        {"account": "Assets:Checking", "value": "-10.00"}]}),
    );
    h.run();
    h.get_by_label("Mixed").click();
    h.run();
    assert!(popup_open(&h));
    assert_eq!(
        h.query_all_by_label("Split (2)").count(),
        2,
        "row and pop-up"
    );
    click(&mut h, "Edit splits…");
    // The split editor opens over the pop-up (revision 24).
    assert!(has(&h, "Split transaction"));
    assert_eq!(value(&h, "Line 2 amount"), "4.00");
    // Cancel goes back to the pop-up; Esc then closes it.
    click(&mut h, "Cancel split");
    assert!(!has(&h, "Split transaction"));
    assert!(popup_open(&h));
}
