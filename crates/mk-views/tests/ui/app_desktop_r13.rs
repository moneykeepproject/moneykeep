//! app-desktop.md revision 13 acceptance tests (87–90): performance, wash sales and
//! corporate actions.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_views::testing::Key;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

fn book(today: &str) -> (H, TestBook) {
    let (mut h, b) = harness_with_chart();
    cmd(&mut h, "module.enable", json!({"module": "investments"}));
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    h.state_mut().set_today(today);
    h.run();
    (h, b)
}

fn security(h: &mut H, sym: &str) {
    cmd(
        h,
        "security.create",
        json!({"symbol": sym, "name": format!("{sym} Inc"), "kind": "stock"}),
    );
}

fn fund(h: &mut H, date: &str, amount: &str) {
    cmd(
        h,
        "txn.create",
        json!({"date": date, "splits": [
            {"account": "Assets:Brokerage", "value": amount},
            {"account": "Assets:Checking", "value": format!("-{amount}")}]}),
    );
}

fn buy(h: &mut H, sym: &str, date: &str, q: &str, p: &str) {
    cmd(
        h,
        "inv.buy",
        json!({"account": "Assets:Brokerage", "security": sym, "date": date, "quantity": q, "price": p}),
    );
}

fn open_report(h: &mut H, title: &str, preset: &str) {
    h.state_mut().open_view_spec("reports");
    h.run();
    h.get_by_role_and_label(Role::Tab, title).click();
    h.run();
    click(h, "Period");
    click(h, preset);
}

#[test]
fn app_desktop_at87_performance_report() {
    let (mut h, _b) = book("2026-01-15");
    security(&mut h, "IDX");
    fund(&mut h, "2025-01-01", "1000.00");
    buy(&mut h, "IDX", "2025-01-01", "100", "10.00");
    cmd(
        &mut h,
        "price.set",
        json!({"security": "IDX", "date": "2025-06-30", "price": "11.00"}),
    );
    fund(&mut h, "2025-07-01", "1100.00");
    buy(&mut h, "IDX", "2025-07-01", "100", "11.00");
    cmd(
        &mut h,
        "price.set",
        json!({"security": "IDX", "date": "2025-12-31", "price": "12.00"}),
    );
    h.run();
    open_report(&mut h, "Performance", "Last year");
    assert!(has(&h, "20.00%"), "{:?}", labels_containing(&h, "%"));
    assert!(has(&h, "19.61%"), "{:?}", labels_containing(&h, "%"));
    assert!(
        has_contains(&h, "ending at 2,400.00"),
        "{:?}",
        labels_containing(&h, "chart")
    );
    assert!(has_contains(&h, "time-weighted return removes it"));
}

#[test]
fn app_desktop_at88_realized_gains_wash_sale_columns() {
    let (mut h, _b) = book("2026-06-30");
    security(&mut h, "XYZ");
    fund(&mut h, "2026-01-02", "5000.00");
    buy(&mut h, "XYZ", "2026-01-05", "10", "100.00");
    cmd(
        &mut h,
        "inv.sell",
        json!({"account": "Assets:Brokerage", "security": "XYZ", "date": "2026-03-02", "quantity": "10", "price": "80.00"}),
    );
    buy(&mut h, "XYZ", "2026-03-20", "6", "82.00");
    h.run();
    open_report(&mut h, "Realized gains", "This year");
    assert!(h.query_all_by_label("Code").count() >= 1);
    assert!(h.query_all_by_label("Adjustment").count() >= 1);
    assert!(has(&h, "W"), "{:?}", labels_containing(&h, "W"));
    assert!(has(&h, "120.00"));
    assert!(has(&h, "Disallowed (wash sales) 120.00"));
}

#[test]
fn app_desktop_at89_spinoff_form_previews_lots() {
    let (mut h, _b) = book("2026-06-01");
    security(&mut h, "PAR");
    security(&mut h, "NEW");
    fund(&mut h, "2024-04-01", "5000.00");
    buy(&mut h, "PAR", "2024-05-01", "100", "50.00");
    h.run();
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "More…");
    click(&mut h, "Spin-off…");
    for (label, text) in [
        ("Parent security field", "PAR"),
        ("New security field", "NEW"),
        ("New shares per parent share field", "0.5"),
        ("Basis moving to the new security % field", "12.5"),
    ] {
        type_into(&mut h, label, text);
    }
    h.run();
    assert!(
        has_contains(
            &h,
            "Preview lots of NEW: 50 shares, basis 625.00, acquired 2024-05-01"
        ),
        "{:?}",
        labels_containing(&h, "Preview")
    );
    assert!(has_contains(&h, "IRS Form 8937"));
    click(&mut h, "Save");
    h.get_by_label_contains("Holding NEW").click();
    h.run();
    assert!(has(&h, "Lots of NEW"));
    assert!(h.query_all_by_label("625.00").count() >= 1);
    click(&mut h, "Close");
    h.get_by_label_contains("Holding PAR").click();
    h.run();
    assert!(
        h.query_all_by_label("4,375.00").count() >= 1,
        "{:?}",
        labels_containing(&h, "4,")
    );
}

#[test]
fn app_desktop_at90_sell_form_wash_notice() {
    let (mut h, _b) = book("2026-03-15");
    security(&mut h, "XYZ");
    fund(&mut h, "2026-01-02", "5000.00");
    buy(&mut h, "XYZ", "2026-03-05", "10", "100.00");
    h.run();
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "Sell");
    type_into(&mut h, "Shares field", "10");
    type_into(&mut h, "Price field", "80.00");
    h.run();
    assert!(
        has(&h, "Possible wash sale"),
        "{:?}",
        labels_containing(&h, "wash")
    );
    assert!(has(&h, "Purchases in the last 30 days:"));
    assert!(
        has_contains(&h, "2026-03-05  10 shares of XYZ"),
        "{:?}",
        labels_containing(&h, "2026-03-05")
    );
    let _ = Key::Escape;
}

#[test]
fn app_desktop_r13_wash_groups_dialog() {
    let (mut h, _b) = book("2026-03-15");
    security(&mut h, "AAA");
    security(&mut h, "AAB");
    h.state_mut().open_view_spec("securities");
    h.run();
    click(&mut h, "Wash groups…");
    assert!(has(&h, "No wash groups yet."));
    type_into(&mut h, "Group name", "A shares");
    for sym in ["AAA", "AAB"] {
        type_into(&mut h, "Security", sym);
        click(&mut h, "Add to group");
    }
    let groups = cmd(&mut h, "security.wash_groups", json!({}));
    assert_eq!(
        groups[0]["members"].as_array().unwrap().len(),
        2,
        "{groups}"
    );
}
