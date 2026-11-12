//! app-desktop.md revision 15 acceptance tests (96–99): purchasing screens.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

fn purchases_book(today: &str) -> (H, TestBook) {
    let (mut h, b) = harness_with_chart();
    for m in ["bills", "purchase-orders", "1099"] {
        cmd(&mut h, "module.enable", json!({"module": m}));
    }
    for n in ["Office Supplies", "Contract Labor", "Rent"] {
        cmd(
            &mut h,
            "account.create",
            json!({"name": n, "type": "expense", "parent": "Expenses"}),
        );
    }
    cmd(
        &mut h,
        "party.create",
        json!({"name": "Office Supply Co", "roles": ["vendor"], "vendor_terms": {"type": "net", "days": 30}}),
    );
    cmd(
        &mut h,
        "party.create",
        json!({"name": "J. Smith Design", "roles": ["vendor"], "tracks_1099": true, "tin_on_file": true,
               "billing_address": ["12 Elm St", "Springfield"]}),
    );
    cmd(
        &mut h,
        "party.create",
        json!({"name": "Landlord LLC", "roles": ["vendor"], "tracks_1099": true, "form_1099": "misc", "misc_box": "rents"}),
    );
    h.state_mut().set_today(today);
    h.run();
    (h, b)
}

fn open_purchases(h: &mut H, tab: &str) {
    open_feature(h, "Purchases");
    h.get_by_role_and_label(Role::Tab, tab).click();
    h.run();
}

fn choose(h: &mut H, combo: &str, option: &str) {
    click(h, combo);
    h.get_by_role_and_label(Role::Button, option).click();
    h.run();
}

fn retype(h: &mut H, label: &str, text: &str) {
    h.get_by_label(label).focus();
    h.run();
    h.key_press_modifiers(
        mk_views::testing::Modifiers::COMMAND,
        mk_views::testing::Key::A,
    );
    h.run();
    h.get_by_label(label).type_text(text);
    h.run();
}

fn bill(h: &mut H, vendor: &str, account: &str, amount: &str) -> String {
    cmd(
        h,
        "bill.create",
        json!({"vendor": vendor, "date": "2026-09-01", "lines": [{"account": account, "amount": amount}]}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn app_desktop_at96_bill_editor_and_check() {
    let (mut h, _b) = purchases_book("2026-09-20");
    open_purchases(&mut h, "Bills");
    click(&mut h, "New bill");
    choose(&mut h, "Vendor", "Office Supply Co");
    retype(&mut h, "Date", "2026-09-05");
    choose(&mut h, "Line 1 account", "Expenses:Office Supplies");
    type_into(&mut h, "Line 1 amount", "240.00");
    click(&mut h, "Add expense line");
    choose(&mut h, "Line 2 account", "Expenses:Office Supplies");
    type_into(&mut h, "Line 2 amount", "60.50");
    assert!(
        has(&h, "Total 300.50"),
        "{:?}",
        labels_containing(&h, "Total")
    );
    click(&mut h, "Save");
    let list = cmd(&mut h, "bill.list", json!({}));
    assert_eq!(list[0]["total"], "300.50");
    assert_eq!(list[0]["due_date"], "2026-10-05");
    click(&mut h, "Pay…");
    choose(&mut h, "Pay from", "Assets:Checking");
    type_into(&mut h, "Check number", "1001");
    assert_eq!(value(&h, "Payment amount"), "300.50");
    click(&mut h, "Pay");
    let pays = cmd(&mut h, "bill_payment.list", json!({}));
    assert_eq!(pays[0]["check_number"], 1001, "{pays}");
    open_register(&mut h, "Assets:Checking");
    assert!(
        h.query_all_by_label("1001").count() >= 1,
        "{:?}",
        labels_containing(&h, "100")
    );
    assert!(h.query_all_by_label_contains("Office Supply Co").count() >= 1);
}

#[test]
fn app_desktop_at97_pay_bills_run() {
    let (mut h, _b) = purchases_book("2026-09-20");
    bill(
        &mut h,
        "Office Supply Co",
        "Expenses:Office Supplies",
        "10.00",
    );
    bill(
        &mut h,
        "Office Supply Co",
        "Expenses:Office Supplies",
        "20.00",
    );
    bill(&mut h, "Landlord LLC", "Expenses:Rent", "1000.00");
    h.run();
    open_purchases(&mut h, "Bills");
    click(&mut h, "Pay bills…");
    choose(&mut h, "Pay from", "Assets:Checking");
    type_into(&mut h, "First check number", "1002");
    let boxes: Vec<String> = labels_containing(&h, "Pay ")
        .into_iter()
        .filter(|l| l.contains(" due ") && !l.starts_with("Amount"))
        .collect();
    assert_eq!(boxes.len(), 3, "{boxes:?}");
    for l in &boxes {
        if !l.starts_with("Amount") {
            click(&mut h, l);
        }
    }
    assert!(
        has(&h, "Total 1,030.00"),
        "{:?}",
        labels_containing(&h, "Total")
    );
    click(&mut h, "Pay selected");
    let pays = cmd(&mut h, "bill_payment.list", json!({}));
    let mut checks: Vec<i64> = pays
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["check_number"].as_i64().unwrap())
        .collect();
    checks.sort_unstable();
    assert_eq!(checks, vec![1002, 1003]);
}

#[test]
fn app_desktop_at98_1099_tab() {
    let (mut h, _b) = purchases_book("2026-12-31");
    let id = bill(
        &mut h,
        "J. Smith Design",
        "Expenses:Contract Labor",
        "2800.00",
    );
    for (amount, extra) in [
        (
            "1500.00",
            json!({"from_account": "Assets:Checking", "check_number": 501}),
        ),
        (
            "900.00",
            json!({"from_account": "Assets:Checking", "method": "bank_transfer"}),
        ),
        ("400.00", json!({"from_account": "Liabilities:Visa"})),
    ] {
        let mut a = json!({"vendor": "J. Smith Design", "date": "2026-10-01", "amount": amount, "apply": [{"bill": id, "amount": amount}]});
        for (k, v) in extra.as_object().unwrap() {
            a[k] = v.clone();
        }
        cmd(&mut h, "bill.pay", a);
    }
    h.run();
    open_purchases(&mut h, "1099");
    assert!(
        has(
            &h,
            "J. Smith Design paid 2,400.00, card payments excluded 400.00, flagged to file"
        ),
        "{:?}",
        labels_containing(&h, "J. Smith")
    );
    assert!(has(&h, "File 1099"));
    assert!(has_contains(&h, "does not file 1099 forms"));
    retype(&mut h, "Threshold MISC Box 1 Rents", "1500");
    click(&mut h, "Save thresholds");
    let s = cmd(&mut h, "module.settings", json!({"module": "1099"}));
    let t = s["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == "1099.thresholds")
        .unwrap()
        .clone();
    assert_eq!(t["value"]["2026"]["rents"], "1500.00");
}

#[test]
fn app_desktop_at99_no_purchases_without_bills() {
    let (mut h, _b) = harness_with_chart();
    assert!(feature_items(&mut h).is_empty(), "no Features menu");
    cmd(&mut h, "module.enable", json!({"module": "bills"}));
    h.state_mut().set_today("2026-09-20");
    h.run();
    assert_eq!(feature_items(&mut h), ["Purchases"]);
    cmd(&mut h, "module.disable", json!({"module": "bills"}));
    h.state_mut().set_today("2026-09-21");
    h.run();
    assert!(feature_items(&mut h).is_empty(), "no Features menu");
}

#[test]
fn app_desktop_r15_register_vendor_column_and_home() {
    let (mut h, _b) = purchases_book("2026-10-10");
    let t = cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-06-01", "payee": "Smith", "splits": [
            {"account": "Expenses:Contract Labor", "value": "250.00"},
            {"account": "Assets:Checking", "value": "-250.00"}]}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    bill(
        &mut h,
        "Office Supply Co",
        "Expenses:Office Supplies",
        "10.00",
    );
    h.run();
    assert!(
        has(&h, "1 bill overdue"),
        "{:?}",
        labels_containing(&h, "bill")
    );
    open_register(&mut h, "Assets:Checking");
    assert!(h.query_all_by_label("Vendor").count() >= 1);
    choose(&mut h, "Vendor for row 1", "J. Smith Design");
    let links = cmd(&mut h, "txn.vendor_links", json!({}));
    assert_eq!(links[&t]["vendor_name"], "J. Smith Design");
}

#[test]
fn app_desktop_r15_po_receive_and_reports() {
    let (mut h, _b) = purchases_book("2026-09-20");
    cmd(
        &mut h,
        "po.create",
        json!({"vendor": "Office Supply Co", "date": "2026-09-01", "lines": [{"account": "Expenses:Office Supplies", "amount": "50.00"}]}),
    );
    h.run();
    open_purchases(&mut h, "Purchase orders");
    click(&mut h, "PO-1001");
    click(&mut h, "Receive…");
    assert_eq!(value(&h, "Amount received for line 1"), "50");
    click(&mut h, "Create bill");
    let po = cmd(&mut h, "po.get", json!({"po": "PO-1001"}));
    assert_eq!(po["status"], "received");
    h.state_mut().open_view_spec("reports");
    h.run();
    for t in [
        "A/P aging",
        "Vendor balances",
        "Unpaid bills",
        "Expenses by vendor",
        "Open purchase orders",
        "Check register",
        "1099 summary",
    ] {
        assert!(h.query_by_role_and_label(Role::Tab, t).is_some(), "{t}");
    }
    h.get_by_role_and_label(Role::Tab, "A/P aging").click();
    h.run();
    assert!(
        has(&h, "Accounts payable balance 50.00"),
        "{:?}",
        labels_containing(&h, "payable")
    );
}
