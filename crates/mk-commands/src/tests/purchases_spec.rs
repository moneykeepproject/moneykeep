//! business-purchases.md acceptance tests 1–12, plus undo and edge cases.

use super::*;
use mk_core::Decimal;

fn dec(v: &Value) -> Decimal {
    v.as_str().unwrap().replace(',', "").parse().unwrap()
}

/// The acceptance-test setup.
fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    for m in ["bills", "purchase-orders", "1099", "items"] {
        b.run("module.enable", json!({"module": m}));
    }
    for n in ["Office Supplies", "Contract Labor", "Rent"] {
        b.account(json!({"name": n, "type": "expense", "parent": "Expenses"}));
    }
    b.run("party.create", json!({"name": "Office Supply Co", "roles": ["vendor"], "vendor_terms": {"type": "net", "days": 30}}));
    b.run(
        "party.create",
        json!({"name": "J. Smith Design", "roles": ["vendor"], "tracks_1099": true, "form_1099": "nec",
               "tin_on_file": true, "billing_address": ["12 Elm St", "Springfield"]}),
    );
    b.run(
        "party.create",
        json!({"name": "Landlord LLC", "roles": ["vendor"], "tracks_1099": true, "form_1099": "misc", "misc_box": "rents"}),
    );
    b
}

fn bill(b: &mut Book, vendor: &str, date: &str, lines: Value) -> Value {
    b.run(
        "bill.create",
        json!({"vendor": vendor, "date": date, "lines": lines}),
    )
}

fn at1_bill(b: &mut Book) -> Value {
    bill(
        b,
        "Office Supply Co",
        "2026-09-05",
        json!([{"account": "Expenses:Office Supplies", "amount": "240.00"},
               {"account": "Expenses:Office Supplies", "amount": "60.50"}]),
    )
}

fn pay(b: &mut Book, vendor: &str, date: &str, amount: &str, extra: Value) -> Value {
    let mut a = json!({"vendor": vendor, "date": date, "amount": amount, "from_account": "Assets:Checking"});
    for (k, v) in extra.as_object().unwrap() {
        a[k] = v.clone();
    }
    b.run("bill.pay", a)
}

#[test]
fn at1_bill_posts_to_ap() {
    let mut b = book();
    let d = at1_bill(&mut b);
    assert_eq!(d["total"], "300.50");
    assert_eq!(d["due_date"], "2026-10-05");
    assert_eq!(d["status"], "open");
    assert_eq!(b.shown("Accounts Payable", "USD"), "300.50");
    assert_eq!(b.shown("Expenses:Office Supplies", "USD"), "300.50");
    let t = b.run("txn.get", json!({"id": d["txn"]}));
    let sum: Decimal = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| dec(&s["value"]))
        .sum();
    assert!(sum.is_zero());
    assert_eq!(t["payee"], "Office Supply Co");
    // A duplicate vendor invoice number is rejected per vendor.
    b.run("bill.update", json!({"bill": d["id"], "number": "A-1"}));
    let e = b.err(
        "bill.create",
        json!({"vendor": "Office Supply Co", "number": "A-1", "lines": [{"account": "Expenses:Rent", "amount": "1.00"}]}),
    );
    assert_eq!(e.code(), "rejected");
    bill(
        &mut b,
        "Landlord LLC",
        "2026-09-05",
        json!([{"account": "Expenses:Rent", "amount": "1.00"}]),
    );
}

#[test]
fn at2_check_payment() {
    let mut b = book();
    let d = at1_bill(&mut b);
    // The first check asks for its number.
    let e = b.err(
        "bill.pay",
        json!({"vendor": "Office Supply Co", "date": "2026-09-20", "amount": "200.00", "from_account": "Assets:Checking"}),
    );
    assert!(e.to_string().contains("first check number"), "{e}");
    let p = pay(
        &mut b,
        "Office Supply Co",
        "2026-09-20",
        "200.00",
        json!({"check_number": 1001}),
    );
    assert_eq!(p["check_number"], 1001);
    assert_eq!(p["method"], "check");
    let d = b.run("bill.get", json!({"bill": d["id"]}));
    assert_eq!(d["status"], "partial");
    assert_eq!(d["balance"], "100.50");
    let t = b.run("txn.get", json!({"id": p["txn"]}));
    assert_eq!(t["number"], "1001");
    assert_eq!(t["payee"], "Office Supply Co");
    let next = b.run(
        "bill_payment.next_check",
        json!({"account": "Assets:Checking"}),
    );
    assert_eq!(next["next"], 1002);
    let s = b.run("module.settings", json!({"module": "bills"}));
    let v = s["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == "bills.next_check_number")
        .unwrap()
        .clone();
    assert_eq!(
        v["value"].as_object().unwrap().values().next().unwrap(),
        &json!(1002)
    );
    assert_eq!(b.shown("Assets:Checking", "USD"), "-200.00");
    // A used number is rejected.
    let e = b.err(
        "bill.pay",
        json!({"vendor": "Office Supply Co", "date": "2026-09-21", "amount": "1.00", "from_account": "Assets:Checking", "check_number": 1001}),
    );
    assert_eq!(e.code(), "rejected");
}

#[test]
fn at3_ap_aging() {
    let mut b = book();
    at1_bill(&mut b);
    pay(
        &mut b,
        "Office Supply Co",
        "2026-09-20",
        "200.00",
        json!({"check_number": 1001}),
    );
    let a = b.run(
        "report.ap_aging",
        json!({"as_of": "2026-11-10", "detail": true}),
    );
    let v = &a["vendors"][0];
    assert_eq!(v["vendor_name"], "Office Supply Co");
    assert_eq!(
        v["buckets"],
        json!(["0.00", "0.00", "100.50", "0.00", "0.00"])
    );
    assert_eq!(v["bills"][0]["days_past_due"], 36);
    assert_eq!(a["totals"]["total"], "100.50");
    assert_eq!(a["ap_balance"], "100.50");
    assert_eq!(a["balanced"], true);
    let csv = b.run(
        "report.export",
        json!({"report": "report.ap_aging", "args": {"as_of": "2026-11-10"}}),
    );
    assert!(
        csv["content"]
            .as_str()
            .unwrap()
            .contains("Office Supply Co,0.00,0.00,100.50")
    );
}

#[test]
fn at4_overpayment_is_prepayment() {
    let mut b = book();
    at1_bill(&mut b);
    pay(
        &mut b,
        "Office Supply Co",
        "2026-09-20",
        "200.00",
        json!({"check_number": 1001}),
    );
    let p = pay(
        &mut b,
        "Office Supply Co",
        "2026-09-25",
        "150.00",
        json!({}),
    );
    assert_eq!(p["check_number"], 1002);
    assert_eq!(p["unapplied"], "49.50");
    let a = b.run("report.ap_aging", json!({"as_of": "2026-11-10"}));
    assert_eq!(a["vendors"][0]["credits"], "-49.50");
    assert_eq!(a["totals"]["total"], "-49.50");
    assert_eq!(a["balanced"], true);
    // The prepayment pays a later bill; then a refund for the rest.
    let d2 = bill(
        &mut b,
        "Office Supply Co",
        "2026-10-01",
        json!([{"account": "Expenses:Office Supplies", "amount": "20.00"}]),
    );
    b.run("bill_payment.apply", json!({"payment": p["id"]}));
    assert_eq!(
        b.run("bill.get", json!({"bill": d2["id"]}))["status"],
        "paid"
    );
    b.run(
        "vendor.refund_received",
        json!({"vendor": "Office Supply Co", "to_account": "Assets:Checking", "amount": "29.50", "date": "2026-10-05"}),
    );
    assert_eq!(
        b.run("vendor.summary", json!({"party": "Office Supply Co"}))["credit"],
        "0.00"
    );
    assert_eq!(b.shown("Accounts Payable", "USD"), "0.00");
}

#[test]
fn at5_vendor_credit() {
    let mut b = book();
    let c = b.run(
        "vendor_credit.create",
        json!({"vendor": "Office Supply Co", "date": "2026-09-10", "lines": [{"account": "Expenses:Office Supplies", "amount": "25.00"}]}),
    );
    assert_eq!(c["unapplied"], "25.00");
    assert_eq!(b.shown("Accounts Payable", "USD"), "-25.00");
    let d = bill(
        &mut b,
        "Office Supply Co",
        "2026-09-12",
        json!([{"account": "Expenses:Office Supplies", "amount": "80.00"}]),
    );
    b.run("vendor_credit.apply", json!({"vendor_credit": c["id"]}));
    let d = b.run("bill.get", json!({"bill": d["id"]}));
    assert_eq!(d["balance"], "55.00");
    assert_eq!(d["status"], "partial");
    b.run(
        "vendor_credit.unapply",
        json!({"vendor_credit": c["id"], "bill": d["id"]}),
    );
    assert_eq!(
        b.run("bill.get", json!({"bill": d["id"]}))["balance"],
        "80.00"
    );
}

fn three_bills(b: &mut Book) -> Vec<String> {
    let mut ids = Vec::new();
    for (v, amt) in [
        ("Office Supply Co", "10.00"),
        ("Office Supply Co", "20.00"),
        ("Landlord LLC", "1000.00"),
    ] {
        let acc = if v.starts_with("Land") {
            "Expenses:Rent"
        } else {
            "Expenses:Office Supplies"
        };
        let d = bill(b, v, "2026-09-01", json!([{"account": acc, "amount": amt}]));
        ids.push(d["id"].as_str().unwrap().to_owned());
    }
    ids
}

#[test]
fn at6_pay_many_and_undo() {
    let mut b = book();
    b.run(
        "module.set_setting",
        json!({"key": "bills.next_check_number", "value": {"Assets:Checking": 1002}}),
    );
    let ids = three_bills(&mut b);
    let before = b.txn_count();
    let out = b.run(
        "bill.pay_many",
        json!({"date": "2026-09-30", "from_account": "Assets:Checking", "bills": ids.iter().map(|i| json!({"bill": i})).collect::<Vec<_>>()}),
    );
    let pays = out["payments"].as_array().unwrap();
    assert_eq!(pays.len(), 2);
    assert_eq!(pays[0]["vendor_name"], "Landlord LLC");
    assert_eq!(pays[0]["check_number"], 1002);
    assert_eq!(pays[1]["check_number"], 1003);
    assert_eq!(pays[1]["amount"], "30.00");
    assert_eq!(b.txn_count(), before + 2);
    for i in &ids {
        assert_eq!(b.run("bill.get", json!({"bill": i}))["status"], "paid");
    }
    b.run("history.undo", json!({}));
    assert_eq!(b.txn_count(), before);
    assert!(
        b.run("bill_payment.list", json!({}))
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        b.run(
            "bill_payment.next_check",
            json!({"account": "Assets:Checking"})
        )["next"],
        1002
    );
}

#[test]
fn at7_void_check() {
    let mut b = book();
    b.run(
        "module.set_setting",
        json!({"key": "bills.next_check_number", "value": {"Assets:Checking": 1002}}),
    );
    let ids = three_bills(&mut b);
    let out = b.run(
        "bill.pay_many",
        json!({"date": "2026-09-30", "from_account": "Assets:Checking", "bills": ids.iter().map(|i| json!({"bill": i})).collect::<Vec<_>>()}),
    );
    let p = out["payments"][0].clone();
    assert_eq!(p["check_number"], 1002);
    let v = b.run("bill_payment.void", json!({"payment": p["id"]}));
    assert_eq!(v["void"], true);
    assert_eq!(b.run("bill.get", json!({"bill": ids[2]}))["status"], "open");
    let r = b.run(
        "report.check_register",
        json!({"account": "Assets:Checking", "from": "2026-01-01", "to": "2026-12-31"}),
    );
    let checks = r["checks"].as_array().unwrap();
    assert_eq!(checks[0]["number"], "1002");
    assert_eq!(checks[0]["voided"], true);
    assert_eq!(checks[1]["number"], "1003");
    assert_eq!(checks[1]["voided"], false);
    // The number stays used.
    assert_eq!(
        b.run(
            "bill_payment.next_check",
            json!({"account": "Assets:Checking"})
        )["next"],
        1004
    );
    let e = b.err(
        "bill.pay",
        json!({"vendor": "Landlord LLC", "date": "2026-10-01", "amount": "1000.00", "from_account": "Assets:Checking", "check_number": 1002}),
    );
    assert_eq!(e.code(), "rejected");
}

#[test]
fn at8_purchase_order_receipts() {
    let mut b = book();
    b.account(json!({"name": "Sales", "type": "income"}));
    b.run(
        "item.create",
        json!({"name": "Paper", "kind": "product", "price": "6.00", "cost": "4.25",
               "income_account": "Sales", "expense_account": "Expenses:Office Supplies"}),
    );
    let po = b.run(
        "po.create",
        json!({"vendor": "Office Supply Co", "date": "2026-09-01", "lines": [{"item": "Paper", "quantity": "10"}],
               "ship_to": ["1 Main St"], "expected_date": "2026-09-10"}),
    );
    assert_eq!(po["number"], "PO-1001");
    assert_eq!(po["total"], "42.50");
    assert_eq!(po["status"], "open");
    assert!(
        b.try_run("account.get", json!({"account": "Accounts Payable"}))
            .is_err()
    );
    let r = b.run(
        "po.receive",
        json!({"po": "PO-1001", "date": "2026-09-08", "lines": [{"po_line": 1, "quantity": "6"}]}),
    );
    assert_eq!(r["bill"]["total"], "25.50");
    assert_eq!(r["bill"]["po"], po["id"]);
    let po2 = b.run("po.get", json!({"po": "PO-1001"}));
    assert_eq!(po2["status"], "partly_received");
    assert_eq!(po2["received"], json!(["6"]));
    let r = b.run("po.receive", json!({"po": "PO-1001", "date": "2026-09-12"}));
    assert_eq!(r["bill"]["total"], "17.00");
    let po3 = b.run("po.get", json!({"po": "PO-1001"}));
    assert_eq!(po3["status"], "received");
    assert_eq!(po3["bills"].as_array().unwrap().len(), 2);
    let over = b.run(
        "po.receive",
        json!({"po": "PO-1001", "date": "2026-09-13", "lines": [{"po_line": 1, "quantity": "1"}]}),
    );
    assert_eq!(over["warnings"].as_array().unwrap().len(), 1);
    assert!(b.try_run("po.delete", json!({"po": "PO-1001"})).is_err());
    let pdf = b.run("po.render_pdf", json!({"po": "PO-1001"}));
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(pdf["content_base64"].as_str().unwrap())
        .unwrap();
    let text: String = bytes.iter().map(|c| char::from(*c)).collect();
    assert!(
        text.contains("(Purchase order)") && text.contains("PO-1001") && text.contains("(Vendor)")
    );
    for k in [
        "/JavaScript",
        "/JS",
        "/AA",
        "/OpenAction",
        "/Launch",
        "/URI",
        "/EmbeddedFile",
        "/AcroForm",
    ] {
        assert!(!text.contains(k));
    }
    let open = b.run("report.open_purchase_orders", json!({}));
    assert!(open["purchase_orders"].as_array().unwrap().is_empty());
}

#[test]
fn at9_1099_nec_with_card_excluded() {
    let mut b = book();
    let d = bill(
        &mut b,
        "J. Smith Design",
        "2026-02-01",
        json!([{"account": "Expenses:Contract Labor", "amount": "2800.00"}]),
    );
    pay(
        &mut b,
        "J. Smith Design",
        "2026-03-01",
        "1500.00",
        json!({"check_number": 501}),
    );
    pay(
        &mut b,
        "J. Smith Design",
        "2026-04-01",
        "900.00",
        json!({"method": "bank_transfer"}),
    );
    b.run(
        "bill.pay",
        json!({"vendor": "J. Smith Design", "date": "2026-05-01", "amount": "400.00", "from_account": "Liabilities:Visa"}),
    );
    assert_eq!(
        b.run("bill.get", json!({"bill": d["id"]}))["status"],
        "paid"
    );
    let r = b.run("report.1099", json!({"year": 2026}));
    let rows = r["vendors"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["vendor_name"], "J. Smith Design");
    assert_eq!(rows[0]["form"], "1099-NEC");
    assert_eq!(rows[0]["total"], "2400.00");
    assert_eq!(rows[0]["excluded_card"], "400.00");
    assert_eq!(rows[0]["threshold"], "2000.00");
    assert_eq!(rows[0]["file"], true);
    assert_eq!(rows[0]["flags"], json!(["file"]));
    // 2025: 700.00 by check is over that year's 600.00.
    bill(
        &mut b,
        "J. Smith Design",
        "2025-03-01",
        json!([{"account": "Expenses:Contract Labor", "amount": "700.00"}]),
    );
    pay(
        &mut b,
        "J. Smith Design",
        "2025-03-15",
        "700.00",
        json!({"check_number": 400}),
    );
    let r = b.run("report.1099", json!({"year": 2025}));
    assert_eq!(r["vendors"][0]["total"], "700.00");
    assert_eq!(r["vendors"][0]["threshold"], "600.00");
    assert_eq!(r["vendors"][0]["file"], true);
    let csv = b.run(
        "report.export",
        json!({"report": "report.1099", "args": {"year": 2026}}),
    );
    assert_eq!(csv["file_name"], "1099-2026.csv");
    assert!(
        csv["content"]
            .as_str()
            .unwrap()
            .contains("J. Smith Design,1099-NEC,1,2400.00,400.00")
    );
}

#[test]
fn at10_misc_rents_below_threshold() {
    let mut b = book();
    bill(
        &mut b,
        "Landlord LLC",
        "2026-01-01",
        json!([{"account": "Expenses:Rent", "amount": "1800.00"}]),
    );
    pay(
        &mut b,
        "Landlord LLC",
        "2026-01-05",
        "1800.00",
        json!({"check_number": 700}),
    );
    let r = b.run("report.1099", json!({"year": 2026}));
    assert!(r["vendors"].as_array().unwrap().is_empty());
    let r = b.run(
        "report.1099",
        json!({"year": 2026, "include_below_threshold": true}),
    );
    let land = r["vendors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["vendor_name"] == "Landlord LLC")
        .unwrap()
        .clone();
    assert_eq!(land["form"], "1099-MISC");
    assert_eq!(land["box"], "1");
    assert_eq!(land["total"], "1800.00");
    assert_eq!(land["file"], false);
    // An edited threshold applies to its year and later ones.
    b.run(
        "module.set_setting",
        json!({"key": "1099.thresholds", "value": {"2026": {"rents": "1500"}}}),
    );
    let r = b.run("report.1099", json!({"year": 2027}));
    assert_eq!(r["thresholds"]["rents"], "1500.00");
    assert_eq!(r["thresholds"]["nec"], "2000.00");
    let r = b.run("report.1099", json!({"year": 2026}));
    assert_eq!(r["vendors"][0]["file"], true);
    // Leaving Rent out of the 1099 accounts drops it.
    b.run(
        "module.set_setting",
        json!({"key": "1099.accounts", "value": ["Expenses:Contract Labor"]}),
    );
    assert!(
        b.run("report.1099", json!({"year": 2026}))["vendors"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn at11_tax_agency_vendor() {
    let mut b = book();
    for m in ["invoicing", "sales-tax"] {
        b.run("module.enable", json!({"module": m}));
    }
    b.account(json!({"name": "Sales", "type": "income"}));
    b.run(
        "party.create",
        json!({"name": "State DOR", "roles": ["vendor"]}),
    );
    b.run(
        "tax_rate.create",
        json!({"name": "State", "rate": "6", "agency": "state dor"}),
    );
    b.run("party.create", json!({"name": "Acme", "tax_rate": "State"}));
    b.run(
        "invoice.create",
        json!({"customer": "Acme", "date": "2026-09-01", "lines": [{"account": "Sales", "amount": "100.00", "taxable": true}]}),
    );
    let p = b.run(
        "sales_tax.pay",
        json!({"from_account": "Assets:Checking", "date": "2026-09-30"}),
    );
    let t = b.run("txn.get", json!({"id": p["txn"]}));
    assert_eq!(t["payee"], "State DOR");
    let s = b.run("vendor.summary", json!({"party": "State DOR"}));
    assert_eq!(s["tax_payments"][0]["amount"], "6.00");
    let r = b.run(
        "report.1099",
        json!({"year": 2026, "include_below_threshold": true}),
    );
    assert!(
        !r["vendors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["vendor_name"] == "State DOR")
    );
}

#[test]
fn at12_undo_payment() {
    let mut b = book();
    let d = at1_bill(&mut b);
    let s0 = b.state();
    let p = pay(
        &mut b,
        "Office Supply Co",
        "2026-09-20",
        "300.50",
        json!({"check_number": 1001}),
    );
    assert_eq!(
        b.run("bill.get", json!({"bill": d["id"]}))["status"],
        "paid"
    );
    b.run("history.undo", json!({}));
    assert_eq!(
        b.run("bill.get", json!({"bill": d["id"]}))["status"],
        "open"
    );
    assert!(b.try_run("txn.get", json!({"id": p["txn"]})).is_err());
    assert_eq!(b.state(), s0);
    b.run("history.redo", json!({}));
    assert_eq!(
        b.run("bill.get", json!({"bill": d["id"]}))["status"],
        "paid"
    );
}

#[test]
fn vendor_link_counts_for_1099_and_reports() {
    let mut b = book();
    let t = b.transfer(
        "2026-06-01",
        "Expenses:Contract Labor",
        "Assets:Checking",
        "2500.00",
    );
    b.run(
        "txn.set_vendor",
        json!({"txn": t, "vendor": "J. Smith Design"}),
    );
    let links = b.run("txn.vendor_links", json!({}));
    assert_eq!(links[&t]["vendor_name"], "J. Smith Design");
    let r = b.run("report.1099", json!({"year": 2026}));
    assert_eq!(r["vendors"][0]["total"], "2500.00");
    let e = b.run(
        "report.expenses_by_vendor",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    assert_eq!(e["total"], "2500.00");
    b.run("txn.set_vendor", json!({"txn": t, "vendor": null}));
    assert!(
        b.run("report.1099", json!({"year": 2026}))["vendors"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Bills due and attention.
    at1_bill(&mut b);
    let a = b.run("bill.attention", json!({"today": "2026-10-01"}));
    assert_eq!(a["due_next_7_days"], 1);
    let a = b.run("bill.attention", json!({"today": "2026-10-06"}));
    assert_eq!(a["overdue"], 1);
    let due = b.run(
        "report.bills_due",
        json!({"from": "2026-10-01", "to": "2026-10-31"}),
    );
    assert_eq!(due["total"], "300.50");
    let v = b.run("report.vendor_balances", json!({"as_of": "2026-10-10"}));
    assert_eq!(v["vendors"][0]["overdue"], "300.50");
    let u = b.run(
        "report.unpaid_bills",
        json!({"as_of": "2026-10-10", "due_before": "2026-10-31"}),
    );
    assert_eq!(u["bills"][0]["days_overdue"], 5);
}

#[test]
fn bill_lifecycle_and_undo_everything() {
    let mut b = book();
    let s0 = b.state();
    let n0 = b.audit_len();
    let d = at1_bill(&mut b);
    b.run("bill.update", json!({"bill": d["id"], "lines": [{"account": "Expenses:Office Supplies", "amount": "100.00"}]}));
    assert_eq!(b.shown("Accounts Payable", "USD"), "100.00");
    let p = pay(
        &mut b,
        "Office Supply Co",
        "2026-09-20",
        "40.00",
        json!({"check_number": 1}),
    );
    assert!(b.try_run("bill.void", json!({"bill": d["id"]})).is_err());
    assert!(b.try_run("bill.update", json!({"bill": d["id"], "lines": [{"account": "Expenses:Office Supplies", "amount": "10.00"}]})).is_err());
    b.run(
        "bill_payment.unapply",
        json!({"payment": p["id"], "bill": d["id"]}),
    );
    b.run("bill.void", json!({"bill": d["id"]}));
    let dup = b.run(
        "bill.duplicate",
        json!({"bill": d["id"], "date": "2026-10-01"}),
    );
    assert_eq!(dup["total"], "100.00");
    // Items without a purchase account are refused on bills.
    b.account(json!({"name": "Sales", "type": "income"}));
    b.run(
        "item.create",
        json!({"name": "Widget", "kind": "product", "price": "5", "income_account": "Sales"}),
    );
    assert!(
        b.try_run(
            "bill.create",
            json!({"vendor": "Office Supply Co", "lines": [{"item": "Widget"}]})
        )
        .is_err()
    );
    let preview = b.run("bill.preview", json!({"vendor": "Office Supply Co", "lines": [{"account": "Expenses:Rent", "amount": "3.00"}, {"account": "Expenses:Rent", "quantity": "2", "cost": "1.25"}]}));
    assert_eq!(preview["total"], "5.50");
    // A customer cannot be billed as a vendor.
    b.run("module.enable", json!({"module": "invoicing"}));
    b.run(
        "party.create",
        json!({"name": "Only Customer", "roles": ["customer"]}),
    );
    assert!(b.try_run("bill.create", json!({"vendor": "Only Customer", "lines": [{"account": "Expenses:Rent", "amount": "1.00"}]})).is_err());
    let n = b.audit_len();
    for _ in n0..n {
        b.run("history.undo", json!({}));
    }
    assert_eq!(b.state(), s0);
    assert!(
        b.run("bill_payment.list", json!({}))
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(b.run("bill.list", json!({})).as_array().unwrap().is_empty());
    // Parties work with only bills on.
    let mut c = Book::new();
    c.run("module.enable", json!({"module": "bills"}));
    let v = c.run("party.create", json!({"name": "Solo"}));
    assert_eq!(v["roles"], json!(["vendor"]));
    assert_eq!(v["tin_on_file"], false);
    assert!(
        c.try_run("party.create", json!({"name": "X", "tin": "12-3456789"}))
            .is_err()
    );
}
