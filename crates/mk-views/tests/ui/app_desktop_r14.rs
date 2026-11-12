//! app-desktop.md revision 14 acceptance tests (91–95): business sales screens.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::AppEvent;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

/// business-sales.md's setup through commands.
fn sales_book() -> (H, TestBook) {
    let (mut h, b) = harness_with_chart();
    for m in ["invoicing", "items", "sales-tax", "estimates"] {
        cmd(&mut h, "module.enable", json!({"module": m}));
    }
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Consulting Income", "type": "income"}),
    );
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Product Sales", "type": "income"}),
    );
    cmd(
        &mut h,
        "item.create",
        json!({"name": "Consulting", "kind": "service", "price": "125.00", "income_account": "Consulting Income"}),
    );
    cmd(
        &mut h,
        "item.create",
        json!({"name": "Widget", "kind": "product", "price": "19.99", "income_account": "Product Sales"}),
    );
    cmd(
        &mut h,
        "tax_rate.create",
        json!({"name": "State", "rate": "6.00"}),
    );
    cmd(
        &mut h,
        "tax_rate.create",
        json!({"name": "County Rate", "rate": "1.25"}),
    );
    cmd(
        &mut h,
        "tax_rate.create",
        json!({"kind": "group", "name": "County", "members": ["State", "County Rate"]}),
    );
    cmd(
        &mut h,
        "party.create",
        json!({"name": "Acme LLC", "terms": {"type": "net", "days": 30}, "tax_rate": "County"}),
    );
    h.state_mut().set_today("2026-09-20");
    h.run();
    (h, b)
}

fn invoice(h: &mut H) {
    cmd(
        h,
        "invoice.create",
        json!({"customer": "Acme LLC", "date": "2026-09-01", "lines": [
            {"item": "Consulting", "quantity": "10"}, {"item": "Widget", "quantity": "3"}]}),
    );
}

fn open_sales(h: &mut H, tab: &str) {
    open_feature(h, "Sales");
    h.get_by_role_and_label(Role::Tab, tab).click();
    h.run();
}

/// Replace a field's text.
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

fn choose(h: &mut H, combo: &str, option: &str) {
    click(h, combo);
    h.get_by_role_and_label(Role::Button, option).click();
    h.run();
}

#[test]
fn app_desktop_at91_module_settings_in_features() {
    let (app, _book) = new_book_app();
    let mut h = harness(app);
    click(&mut h, "Settings");
    settings_section(&mut h, "Features");
    assert!(!has(&h, "Default days valid"));
    click(&mut h, "Estimates");
    assert!(
        has(&h, "Default days valid"),
        "{:?}",
        labels_containing(&h, "valid")
    );
    assert!(has(&h, "Estimate number prefix"));
    assert_eq!(value(&h, "Default days valid"), "30");
    retype(&mut h, "Default days valid", "45");
    key(&mut h, mk_views::testing::Key::Enter);
    let s = cmd(&mut h, "module.settings", json!({"module": "estimates"}));
    let v = s["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == "estimates.default_valid_days")
        .unwrap()["value"]
        .clone();
    assert_eq!(v, json!(45), "{:?}", labels_containing(&h, "valid"));
    // A refused value shows its error next to the field.
    retype(&mut h, "Default days valid", "0");
    key(&mut h, mk_views::testing::Key::Enter);
    assert!(
        has_contains(&h, "must be from 1 to 365"),
        "{:?}",
        labels_containing(&h, "must")
    );
}

#[test]
fn app_desktop_at92_invoice_editor() {
    let (mut h, _b) = sales_book();
    open_sales(&mut h, "Invoices");
    click(&mut h, "New invoice");
    choose(&mut h, "Customer", "Acme LLC");
    retype(&mut h, "Date", "2026-09-01");
    choose(&mut h, "Line 1 item", "Consulting");
    type_into(&mut h, "Line 1 quantity", "10");
    click(&mut h, "Add line");
    choose(&mut h, "Line 2 item", "Widget");
    type_into(&mut h, "Line 2 quantity", "3");
    h.run();
    assert!(
        has(&h, "Total 1,314.32"),
        "{:?}",
        labels_containing(&h, "Total")
    );
    click(&mut h, "Save and finalise");
    let list = cmd(&mut h, "invoice.list", json!({}));
    assert_eq!(list[0]["number"], "INV-1001");
    assert_eq!(list[0]["status"], "open");
    assert_eq!(list[0]["total"], "1314.32");
    assert_eq!(list[0]["due_date"], "2026-10-01");
    events(&mut h);
    click(&mut h, "PDF…");
    let ev = events(&mut h);
    let pdf = ev.iter().find_map(|e| match e {
        AppEvent::SaveBytes { file_name, data } => Some((file_name.clone(), data.clone())),
        _ => None,
    });
    let (name, data) = pdf.expect("a PDF save request");
    assert_eq!(name, "INV-1001.pdf");
    assert!(data.starts_with(b"%PDF-1."));
}

#[test]
fn app_desktop_at93_record_payment() {
    let (mut h, _b) = sales_book();
    invoice(&mut h);
    open_sales(&mut h, "Payments");
    click(&mut h, "Record payment…");
    choose(&mut h, "Payment customer", "Acme LLC");
    type_into(&mut h, "Payment amount", "1000.00");
    h.run();
    assert_eq!(value(&h, "Apply to INV-1001"), "1000.00");
    click(&mut h, "Save payment");
    open_sales(&mut h, "Invoices");
    assert!(
        h.query_all_by_label("314.32").count() >= 1,
        "{:?}",
        labels_containing(&h, "314")
    );
    assert!(has(&h, "Partly paid"));
}

#[test]
fn app_desktop_at94_make_deposit() {
    let (mut h, _b) = sales_book();
    invoice(&mut h);
    cmd(&mut h, "party.create", json!({"name": "Beta Co"}));
    cmd(
        &mut h,
        "payment.receive",
        json!({"customer": "Acme LLC", "date": "2026-09-20", "amount": "1000.00"}),
    );
    cmd(
        &mut h,
        "payment.receive",
        json!({"customer": "Beta Co", "date": "2026-09-20", "amount": "200.00"}),
    );
    h.run();
    open_sales(&mut h, "Payments");
    click(&mut h, "Make deposit…");
    choose(&mut h, "Bank account", "Assets:Checking");
    click(&mut h, "2026-09-20 Acme LLC 1,000.00");
    click(&mut h, "2026-09-20 Beta Co 200.00");
    assert!(
        has(&h, "Deposit total 1,200.00"),
        "{:?}",
        labels_containing(&h, "Deposit")
    );
    click(&mut h, "Save deposit");
    let reg = cmd(
        &mut h,
        "account.register",
        json!({"account": "Assets:Checking"}),
    );
    let rows = reg["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{reg}");
    open_register(&mut h, "Assets:Checking");
    assert!(h.query_all_by_label("1,200.00").count() >= 1);
}

#[test]
fn app_desktop_at95_no_sales_without_invoicing() {
    let (mut h, _b) = harness_with_chart();
    assert!(feature_items(&mut h).is_empty(), "no Features menu");
    cmd(&mut h, "module.enable", json!({"module": "invoicing"}));
    h.state_mut().set_today("2026-09-20");
    h.run();
    assert_eq!(feature_items(&mut h), ["Sales"]);
    cmd(&mut h, "module.disable", json!({"module": "invoicing"}));
    h.state_mut().set_today("2026-09-21");
    h.run();
    assert!(feature_items(&mut h).is_empty(), "no Features menu");
}

#[test]
fn app_desktop_r14_sales_reports_listed() {
    let (mut h, _b) = sales_book();
    invoice(&mut h);
    h.state_mut().open_view_spec("reports");
    h.run();
    for t in [
        "Balance sheet",
        "A/R aging",
        "Customer balances",
        "Open invoices",
        "Sales by customer",
        "Sales by item",
        "Sales tax liability",
    ] {
        assert!(h.query_by_role_and_label(Role::Tab, t).is_some(), "{t}");
    }
    h.get_by_role_and_label(Role::Tab, "A/R aging").click();
    h.run();
    assert!(
        h.query_all_by_label("1,314.32").count() >= 1,
        "{:?}",
        labels_containing(&h, "1,")
    );
    assert!(has(&h, "Accounts receivable balance 1,314.32"));
}

#[test]
fn app_desktop_r14_customer_view_and_tabs() {
    let (mut h, _b) = sales_book();
    invoice(&mut h);
    open_sales(&mut h, "Customers");
    click(&mut h, "Acme LLC");
    assert!(has(&h, "INV-1001"));
    assert!(has(&h, "Refund…"));
    open_sales(&mut h, "Items");
    assert!(has(&h, "Widget"));
    open_sales(&mut h, "Sales tax");
    assert!(has(&h, "Pay sales tax…"));
    click(&mut h, "Pay sales tax…");
    assert_eq!(value(&h, "Amount for State"), "3.60");
}
