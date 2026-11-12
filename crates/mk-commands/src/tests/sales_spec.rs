//! business-sales.md acceptance tests 1–15, plus undo and edge cases.

use super::*;
use base64::Engine;
use mk_core::Decimal;

fn dec(v: &Value) -> Decimal {
    match v {
        Value::String(s) => s.replace(',', "").parse().unwrap(),
        other => other.to_string().parse().unwrap(),
    }
}

/// The acceptance-test setup.
pub(super) fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    for m in ["invoicing", "items", "sales-tax", "estimates"] {
        b.run("module.enable", json!({"module": m}));
    }
    b.account(json!({"name": "Consulting Income", "type": "income"}));
    b.account(json!({"name": "Product Sales", "type": "income"}));
    b.run(
        "item.create",
        json!({"name": "Consulting", "kind": "service", "price": "125.00", "income_account": "Consulting Income"}),
    );
    b.run(
        "item.create",
        json!({"name": "Widget", "kind": "product", "price": "19.99", "income_account": "Product Sales"}),
    );
    b.run(
        "tax_rate.create",
        json!({"name": "State", "rate": "6.00", "agency": "State DOR"}),
    );
    b.run(
        "tax_rate.create",
        json!({"name": "County Rate", "rate": "1.25"}),
    );
    b.run(
        "tax_rate.create",
        json!({"kind": "group", "name": "County", "members": ["State", "County Rate"]}),
    );
    b.run(
        "party.create",
        json!({"name": "Acme LLC", "terms": {"type": "net", "days": 30}, "tax_rate": "County",
               "billing_address": ["1 Main St", "Springfield"]}),
    );
    b
}

fn lines() -> Value {
    json!([
        {"item": "Consulting", "quantity": "10"},
        {"item": "Widget", "quantity": "3"},
    ])
}

/// Test 2's invoice.
pub(super) fn invoice(b: &mut Book) -> Value {
    b.run(
        "invoice.create",
        json!({"customer": "Acme LLC", "date": "2026-09-01", "lines": lines()}),
    )
}

pub(super) fn pay(b: &mut Book, customer: &str, date: &str, amount: &str) -> Value {
    b.run(
        "payment.receive",
        json!({"customer": customer, "date": date, "amount": amount}),
    )
}

fn inv(b: &mut Book, number: &str) -> Value {
    b.run("invoice.get", json!({"invoice": number}))
}

#[test]
fn at1_module_settings() {
    let mut b = book();
    let s = b.run("module.settings", json!({"module": "estimates"}));
    let keys: Vec<&str> = s["settings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        [
            "estimates.number_prefix",
            "estimates.next_number",
            "estimates.default_valid_days",
            "estimates.require_acceptance",
            "estimates.convert_as",
            "estimates.default_message"
        ]
    );
    let valid = &s["settings"][2];
    assert_eq!(valid["value"], json!(30));
    assert_eq!(valid["default"], json!(30));
    assert_eq!(valid["type"], "int");

    let list = b.run("module.list", json!({}));
    let est = list
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "estimates")
        .unwrap()
        .clone();
    assert_eq!(est["has_settings"], true);
    assert_eq!(est["requires"], json!(["invoicing"]));

    let e = b.err(
        "module.set_setting",
        json!({"key": "estimates.default_valid_days", "value": 0}),
    );
    assert_eq!(e.code(), "invalid_args");
    let before = b.audit_len();
    b.run(
        "module.set_setting",
        json!({"key": "estimates.default_valid_days", "value": 45}),
    );
    assert_eq!(b.audit_len(), before + 1);
    let s = b.run("module.settings", json!({"module": "estimates"}));
    assert_eq!(s["settings"][2]["value"], json!(45));
    b.run("history.undo", json!({}));
    let s = b.run("module.settings", json!({"module": "estimates"}));
    assert_eq!(s["settings"][2]["value"], json!(30));
    b.run("history.redo", json!({}));

    // Off: hidden, but the value is kept.
    b.run("module.disable", json!({"module": "estimates"}));
    assert!(
        b.try_run("module.settings", json!({"module": "estimates"}))
            .is_err()
    );
    assert!(
        b.try_run(
            "module.set_setting",
            json!({"key": "estimates.default_valid_days", "value": 50})
        )
        .is_err()
    );
    b.run("module.enable", json!({"module": "estimates"}));
    let s = b.run("module.settings", json!({"module": "estimates"}));
    assert_eq!(s["settings"][2]["value"], json!(45));
    // Reset with null.
    b.run(
        "module.set_setting",
        json!({"key": "estimates.default_valid_days", "value": null}),
    );
    let s = b.run("module.settings", json!({"module": "estimates"}));
    assert_eq!(s["settings"][2]["value"], json!(30));
}

#[test]
fn at2_invoice_posting() {
    let mut b = book();
    let i = invoice(&mut b);
    assert_eq!(i["number"], "INV-1001");
    assert_eq!(i["status"], "open");
    assert_eq!(i["due_date"], "2026-10-01");
    let t = &i["totals"];
    assert_eq!(t["subtotal"], "1309.97");
    assert_eq!(t["tax"][0]["name"], "State");
    assert_eq!(t["tax"][0]["amount"], "3.60");
    assert_eq!(t["tax"][1]["amount"], "0.75");
    assert_eq!(t["tax_total"], "4.35");
    assert_eq!(t["total"], "1314.32");

    let txn = b.run("txn.get", json!({"id": i["txn"]}));
    assert_eq!(txn["payee"], "Acme LLC");
    let mut splits: Vec<(String, Decimal)> = txn["splits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["account_name"].as_str().unwrap().to_owned(),
                dec(&s["value"]),
            )
        })
        .collect();
    splits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let want: Vec<(String, Decimal)> = [
        ("Accounts Receivable", "1314.32"),
        ("Consulting Income", "-1250.00"),
        ("Product Sales", "-59.97"),
        ("Sales Tax Payable", "-3.60"),
        ("Sales Tax Payable", "-0.75"),
    ]
    .iter()
    .map(|(a, v)| ((*a).to_owned(), v.parse().unwrap()))
    .collect();
    let mut want = want;
    want.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    assert_eq!(splits, want);
    let sum: Decimal = splits.iter().map(|s| s.1).sum();
    assert!(sum.is_zero());
    assert_eq!(b.shown("Accounts Receivable", "USD"), "1314.32");
    assert_eq!(b.shown("Sales Tax Payable", "USD"), "4.35");
}

#[test]
fn at3_payment_auto_applies() {
    let mut b = book();
    invoice(&mut b);
    let p = pay(&mut b, "Acme LLC", "2026-09-20", "1000.00");
    assert_eq!(p["deposit_to_name"], "Payments to Deposit");
    assert_eq!(p["applied"][0]["number"], "INV-1001");
    let i = inv(&mut b, "INV-1001");
    assert_eq!(i["status"], "partial");
    assert_eq!(i["balance"], "314.32");
    assert_eq!(b.shown("Payments to Deposit", "USD"), "1000.00");
    assert_eq!(b.shown("Accounts Receivable", "USD"), "314.32");
}

#[test]
fn at4_deposit_groups_payments() {
    let mut b = book();
    invoice(&mut b);
    let p1 = pay(&mut b, "Acme LLC", "2026-09-20", "1000.00");
    b.run("party.create", json!({"name": "Beta Co"}));
    let p2 = pay(&mut b, "Beta Co", "2026-09-21", "200.00");
    let before = b.txn_count();
    let d = b.run(
        "deposit.create",
        json!({"bank_account": "Assets:Checking", "date": "2026-09-22", "payments": [p1["id"], p2["id"]]}),
    );
    assert_eq!(d["total"], "1200.00");
    assert_eq!(b.txn_count(), before + 1);
    assert_eq!(b.shown("Assets:Checking", "USD"), "1200.00");
    assert_eq!(b.shown("Payments to Deposit", "USD"), "0.00");
    let undeposited = b.run("payment.list", json!({"undeposited": true}));
    assert!(undeposited.as_array().unwrap().is_empty());
    // A deposited payment cannot be deleted.
    let e = b.err("payment.delete", json!({"payment": p1["id"]}));
    assert_eq!(e.code(), "rejected");
    b.run("deposit.delete", json!({"deposit": d["id"]}));
    b.run("payment.delete", json!({"payment": p1["id"]}));
}

#[test]
fn at5_overpayment_is_credit() {
    let mut b = book();
    invoice(&mut b);
    pay(&mut b, "Acme LLC", "2026-09-20", "1000.00");
    let p = pay(&mut b, "Acme LLC", "2026-10-10", "400.00");
    assert_eq!(p["unapplied"], "85.68");
    let i = inv(&mut b, "INV-1001");
    assert_eq!(i["status"], "paid");
    assert_eq!(i["balance"], "0.00");
    let s = b.run("customer.summary", json!({"party": "Acme LLC"}));
    assert_eq!(s["credit"], "85.68");
    assert_eq!(b.shown("Accounts Receivable", "USD"), "-85.68");

    // The credit pays a later invoice; the rest is refunded.
    let i2 = b.run(
        "invoice.create",
        json!({"customer": "Acme LLC", "date": "2026-10-15", "lines": [{"item": "Consulting", "quantity": "0.5"}]}),
    );
    assert_eq!(i2["totals"]["total"], "62.50");
    b.run("payment.apply", json!({"payment": p["id"]}));
    assert_eq!(inv(&mut b, "INV-1002")["status"], "paid");
    let r = b.run(
        "customer.refund",
        json!({"customer": "Acme LLC", "from_account": "Assets:Checking", "amount": "23.18", "date": "2026-10-20"}),
    );
    assert_eq!(r["amount"], "23.18");
    let s = b.run("customer.summary", json!({"party": "Acme LLC"}));
    assert_eq!(s["credit"], "0.00");
    assert_eq!(b.shown("Accounts Receivable", "USD"), "0.00");
    let e = b.err(
        "customer.refund",
        json!({"customer": "Acme LLC", "from_account": "Assets:Checking", "amount": "1.00", "date": "2026-10-21"}),
    );
    assert_eq!(e.code(), "rejected");
}

#[test]
fn at6_ar_aging() {
    let mut b = book();
    invoice(&mut b);
    pay(&mut b, "Acme LLC", "2026-09-20", "1000.00");
    let a = b.run(
        "report.ar_aging",
        json!({"as_of": "2026-11-15", "detail": true}),
    );
    assert_eq!(
        a["columns"],
        json!(["Current", "1-30", "31-60", "61-90", "Over 90"])
    );
    let acme = &a["customers"][0];
    assert_eq!(acme["customer_name"], "Acme LLC");
    assert_eq!(
        acme["buckets"],
        json!(["0.00", "0.00", "314.32", "0.00", "0.00"])
    );
    assert_eq!(acme["invoices"][0]["days_past_due"], 45);
    assert_eq!(a["totals"]["total"], "314.32");
    assert_eq!(a["ar_balance"], "314.32");
    assert_eq!(a["balanced"], true);
    // Before the invoice was due it is current; before the payment, the whole amount.
    let a = b.run("report.ar_aging", json!({"as_of": "2026-09-10"}));
    assert_eq!(a["customers"][0]["buckets"][0], "1314.32");
    assert_eq!(a["balanced"], true);
    // A manual entry to AR is reported as a difference, never hidden.
    b.transfer(
        "2026-09-05",
        "Accounts Receivable",
        "Assets:Checking",
        "10.00",
    );
    let a = b.run("report.ar_aging", json!({"as_of": "2026-11-15"}));
    assert_eq!(a["difference"], "10.00");
    assert_eq!(a["balanced"], false);
    let csv = b.run(
        "report.export",
        json!({"report": "report.ar_aging", "args": {"as_of": "2026-11-15"}}),
    );
    assert_eq!(csv["file_name"], "ar-aging-2026-11-15.csv");
    assert!(
        csv["content"]
            .as_str()
            .unwrap()
            .contains("Acme LLC,0.00,0.00,314.32")
    );
    let c = b.run("report.customer_balances", json!({"as_of": "2026-11-15"}));
    assert_eq!(c["total"], "314.32");
    let o = b.run("report.open_invoices", json!({"as_of": "2026-11-15"}));
    assert_eq!(o["invoices"][0]["days_overdue"], 45);
}

#[test]
fn at7_credit_memo() {
    let mut b = book();
    invoice(&mut b);
    pay(&mut b, "Acme LLC", "2026-09-20", "1000.00");
    let c = b.run(
        "credit_memo.create",
        json!({"customer": "Acme LLC", "date": "2026-09-25", "lines": [{"item": "Widget", "quantity": "1"}]}),
    );
    assert_eq!(c["number"], "CM-1001");
    assert_eq!(c["totals"]["tax"][0]["amount"], "1.20");
    assert_eq!(c["totals"]["tax"][1]["amount"], "0.25");
    assert_eq!(c["totals"]["total"], "21.44");
    assert_eq!(c["unapplied"], "21.44");
    b.run(
        "credit_memo.apply",
        json!({"credit_memo": "CM-1001", "apply": [{"invoice": "INV-1001", "amount": "21.44"}]}),
    );
    assert_eq!(inv(&mut b, "INV-1001")["balance"], "292.88");
    assert_eq!(b.shown("Accounts Receivable", "USD"), "292.88");
    assert_eq!(b.shown("Sales Tax Payable", "USD"), "2.90");
    // Unapply and re-apply automatically.
    b.run(
        "credit_memo.unapply",
        json!({"credit_memo": "CM-1001", "invoice": "INV-1001"}),
    );
    assert_eq!(inv(&mut b, "INV-1001")["balance"], "314.32");
    b.run("credit_memo.apply", json!({"credit_memo": "CM-1001"}));
    assert_eq!(inv(&mut b, "INV-1001")["balance"], "292.88");
}

#[test]
fn at8_estimate_conversion() {
    let mut b = book();
    invoice(&mut b);
    let e = b.run(
        "estimate.create",
        json!({"customer": "Acme LLC", "date": "2030-09-02", "lines": lines()}),
    );
    assert_eq!(e["number"], "EST-1001");
    assert_eq!(e["expires"], "2030-10-02");
    assert_eq!(e["status"], "draft");
    let out = b.run(
        "estimate.convert",
        json!({"estimate": "EST-1001", "date": "2026-09-05"}),
    );
    let i = &out["invoice"];
    assert_eq!(i["number"], "INV-1002");
    assert_eq!(i["status"], "draft");
    assert_eq!(i["totals"], e["totals"]);
    assert_eq!(i["linked"], e["id"]);
    assert_eq!(out["estimate"]["status"], "converted");
    assert_eq!(out["estimate"]["linked"], i["id"]);
    assert!(
        b.try_run("estimate.convert", json!({"estimate": "EST-1001"}))
            .is_err()
    );

    b.run(
        "module.set_setting",
        json!({"key": "estimates.require_acceptance", "value": true}),
    );
    b.run(
        "estimate.create",
        json!({"customer": "Acme LLC", "date": "2030-09-02", "lines": lines(), "status": "sent"}),
    );
    let err = b.err("estimate.convert", json!({"estimate": "EST-1002"}));
    assert_eq!(err.code(), "rejected");
    b.run(
        "estimate.set_status",
        json!({"estimate": "EST-1002", "status": "accepted"}),
    );
    b.run(
        "module.set_setting",
        json!({"key": "estimates.convert_as", "value": "open"}),
    );
    let out = b.run(
        "estimate.convert",
        json!({"estimate": "EST-1002", "date": "2026-09-06"}),
    );
    assert_eq!(out["invoice"]["status"], "open");
    // Expired is computed.
    let old = b.run(
        "estimate.create",
        json!({"customer": "Acme LLC", "date": "2020-01-01", "lines": lines(), "status": "sent"}),
    );
    assert_eq!(old["status"], "expired");
}

#[test]
fn at9_discount_line() {
    let mut b = book();
    b.account(json!({"name": "Discounts Given", "type": "income"}));
    b.run(
        "item.create",
        json!({"name": "10 % off", "kind": "discount", "percent": true, "price": "10", "income_account": "Discounts Given"}),
    );
    let i = b.run(
        "invoice.create",
        json!({"customer": "Acme LLC", "date": "2026-09-01", "lines": [
            {"item": "Widget", "quantity": "3"},
            {"item": "10 % off"},
        ]}),
    );
    let t = &i["totals"];
    assert_eq!(t["lines"][1]["amount"], "-6.00");
    assert_eq!(t["taxable_base"], "53.97");
    assert_eq!(t["tax"][0]["amount"], "3.24");
    assert_eq!(t["tax"][1]["amount"], "0.67");
    assert_eq!(t["tax_total"], "3.91");
    assert_eq!(t["total"], "57.88");
    assert_eq!(b.shown("Discounts Given", "USD"), "-6.00");

    // A subtotal limits the discount to the lines after it; a non-taxable line takes
    // its proportional share of the discount.
    let p = b.run(
        "invoice.preview",
        json!({"customer": "Acme LLC", "date": "2026-09-01", "lines": [
            {"item": "Consulting", "quantity": "1"},
            {"subtotal": true},
            {"item": "Widget", "quantity": "1", "rate": "100"},
            {"item": "Consulting", "quantity": "1", "rate": "100"},
            {"item": "10 % off"},
        ]}),
    );
    let t = &p["totals"];
    assert_eq!(t["lines"][1]["amount"], "125.00");
    assert_eq!(t["lines"][4]["amount"], "-20.00");
    assert_eq!(t["taxable_base"], "90.00");
    // Preview takes no number and posts nothing.
    assert_eq!(p["number"], "INV-1002");
    let p2 = b.run(
        "invoice.preview",
        json!({"customer": "Acme LLC", "lines": [{"item": "Widget"}]}),
    );
    assert_eq!(p2["number"], "INV-1002");
}

#[test]
fn at9b_fixed_discount_times_quantity() {
    // business-sales.md revision 2: a fixed discount is quantity × rate.
    let mut b = book();
    b.account(json!({"name": "Discounts Given", "type": "income"}));
    b.run(
        "item.create",
        json!({"name": "Discount", "kind": "discount", "price": "40", "income_account": "Discounts Given"}),
    );
    let p = b.run(
        "invoice.preview",
        json!({"customer": "Acme LLC", "date": "2026-09-01", "lines": [
            {"item": "Consulting", "quantity": "3", "rate": "80"},
            {"item": "Discount", "quantity": "3"},
            {"item": "Consulting", "quantity": "4", "rate": "50"},
            {"item": "Discount", "quantity": "4", "rate": "15"},
            {"item": "Discount"},
        ]}),
    );
    let t = &p["totals"];
    assert_eq!(t["lines"][1]["amount"], "-120.00");
    assert_eq!(t["lines"][1]["quantity"], "3");
    assert_eq!(t["lines"][3]["amount"], "-60.00");
    assert_eq!(t["lines"][4]["amount"], "-40.00", "no quantity: 1");
    assert_eq!(t["subtotal"], "220.00");
}

#[test]
fn at10_tax_exempt() {
    let mut b = book();
    b.run(
        "party.create",
        json!({"name": "School District", "tax_rate": "County", "tax_exempt": true, "exemption_note": "Certificate 12345"}),
    );
    let i = b.run(
        "invoice.create",
        json!({"customer": "School District", "date": "2026-09-01", "lines": lines()}),
    );
    assert_eq!(i["totals"]["tax_total"], "0.00");
    assert_eq!(i["totals"]["total"], "1309.97");
    let pdf = b.run("invoice.render_pdf", json!({"invoice": i["number"]}));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(pdf["content_base64"].as_str().unwrap())
        .unwrap();
    let text: String = bytes.iter().map(|b| char::from(*b)).collect();
    assert!(text.contains("Tax exempt: Certificate 12345"));

    // Marking a customer exempt later leaves existing invoices unchanged.
    let before = invoice(&mut b);
    b.run(
        "party.update",
        json!({"party": "Acme LLC", "tax_exempt": true}),
    );
    let after = inv(&mut b, before["number"].as_str().unwrap());
    assert_eq!(after["totals"], before["totals"]);
}

#[test]
fn at11_pdf() {
    let mut b = book();
    b.run(
        "module.set_setting",
        json!({"key": "invoicing.business_name", "value": "Sample Studio"}),
    );
    invoice(&mut b);
    let pdf = b.run("invoice.render_pdf", json!({"invoice": "INV-1001"}));
    assert_eq!(pdf["file_name"], "INV-1001.pdf");
    assert_eq!(pdf["warnings"], json!([]));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(pdf["content_base64"].as_str().unwrap())
        .unwrap();
    assert!(bytes.starts_with(b"%PDF-1."));
    let version: u32 = char::from(bytes[7]).to_digit(10).unwrap();
    assert!(version >= 4);
    assert!(bytes.ends_with(b"%%EOF\n"));
    let text: String = bytes.iter().map(|b| char::from(*b)).collect();
    // branding.md: the document information names the producer.
    assert!(text.contains("<< /Producer (MoneyKeep) >>"), "producer");
    assert!(text.contains(" 0 R >>\nstartxref"));
    for s in [
        "INV-1001",
        "1,314.32",
        "Balance due",
        "Sample Studio",
        "Due date",
        "Net 30",
    ] {
        assert!(text.contains(s), "missing {s}");
    }
    for k in [
        "/JavaScript",
        "/JS",
        "/AA",
        "/OpenAction",
        "/Launch",
        "/URI",
        "/EmbeddedFile",
        "/AcroForm",
        "/Annots",
        "/FontFile",
    ] {
        assert!(!text.contains(k), "contains {k}");
    }
    // The xref offsets point at the objects.
    let at = bytes
        .windows(10)
        .rposition(|w| w == b"startxref\n")
        .unwrap()
        + 10;
    let tail = std::str::from_utf8(&bytes[at..]).unwrap();
    let startxref: usize = tail.lines().next().unwrap().parse().unwrap();
    assert!(bytes[startxref..].starts_with(b"xref"));
    let table = std::str::from_utf8(&bytes[startxref..]).unwrap();
    let mut objects = 0;
    for line in table.lines().skip(3) {
        if !line.ends_with(" n ") {
            break;
        }
        let off: usize = line[..10].parse().unwrap();
        objects += 1;
        assert!(bytes[off..].starts_with(format!("{objects} 0 obj").as_bytes()));
    }
    assert!(objects >= 6);

    b.run("party.create", json!({"name": "Café Ünïcode 東京"}));
    b.run(
        "invoice.create",
        json!({"customer": "Café Ünïcode 東京", "date": "2026-09-01", "lines": lines()}),
    );
    let pdf = b.run("invoice.render_pdf", json!({"invoice": "INV-1002"}));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(pdf["content_base64"].as_str().unwrap())
        .unwrap();
    let text: String = bytes.iter().map(|b| char::from(*b)).collect();
    assert!(text.contains("(Café Ünïcode ??)"));
    let w = pdf["warnings"][0].as_str().unwrap();
    assert!(w.contains("東京"), "{w}");

    // Estimates and A4.
    b.run(
        "module.set_setting",
        json!({"key": "invoicing.paper", "value": "A4"}),
    );
    b.run(
        "estimate.create",
        json!({"customer": "Acme LLC", "date": "2026-09-02", "lines": lines()}),
    );
    let pdf = b.run("estimate.render_pdf", json!({"estimate": "EST-1001"}));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(pdf["content_base64"].as_str().unwrap())
        .unwrap();
    let text: String = bytes.iter().map(|b| char::from(*b)).collect();
    assert!(text.contains("Valid until") && text.contains("(Estimate)"));
    assert!(text.contains("/MediaBox [0 0 595 842]"));
}

#[test]
fn at12_pay_sales_tax() {
    let mut b = book();
    invoice(&mut b);
    let p = b.run(
        "sales_tax.pay",
        json!({"from_account": "Assets:Checking", "date": "2026-10-15", "as_of": "2026-09-30"}),
    );
    assert_eq!(p["total"], "4.35");
    assert_eq!(b.shown("Sales Tax Payable", "USD"), "0.00");
    let r = b.run(
        "report.sales_tax_liability",
        json!({"from": "2026-09-01", "to": "2026-10-31"}),
    );
    let state = r["rates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "State")
        .unwrap()
        .clone();
    assert_eq!(state["collected"], "3.60");
    assert_eq!(state["paid"], "3.60");
    assert_eq!(state["balance"], "0.00");
    assert_eq!(state["taxable_sales"], "59.97");
    assert_eq!(state["nontaxable_sales"], "1250.00");
    assert!(
        b.try_run(
            "sales_tax.pay",
            json!({"from_account": "Assets:Checking", "date": "2026-10-16"})
        )
        .is_err()
    );
}

#[test]
fn at13_balance_sheet() {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Sales", "type": "income"}));
    b.transfer("2025-06-01", "Assets:Checking", "Sales", "500.00");
    b.transfer("2026-03-01", "Assets:Checking", "Sales", "800.00");
    b.transfer(
        "2026-04-01",
        "Expenses:Groceries",
        "Assets:Checking",
        "300.00",
    );
    let s = b.run("report.balance_sheet", json!({"as_of": "2026-12-31"}));
    let checking = s["assets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["full_name"] == "Assets:Checking")
        .unwrap()
        .clone();
    assert_eq!(checking["balance"], "1000.00");
    assert_eq!(s["retained_earnings"], "500.00");
    assert_eq!(s["net_income"], "500.00");
    assert_eq!(s["total_assets"], "1000.00");
    assert_eq!(s["total_liabilities_and_equity"], "1000.00");
    assert_eq!(s["balanced"], true);
    // Another fiscal year start moves the line between the two.
    b.run(
        "module.set_setting",
        json!({"key": "book.fiscal_year_start", "value": "07-01"}),
    );
    let s = b.run("report.balance_sheet", json!({"as_of": "2026-12-31"}));
    assert_eq!(s["fiscal_year_start"], "2026-07-01");
    assert_eq!(s["retained_earnings"], "1000.00");
    assert_eq!(s["net_income"], "0.00");
    assert!(
        b.try_run(
            "module.set_setting",
            json!({"key": "book.fiscal_year_start", "value": "02-30"})
        )
        .is_err()
    );
    let csv = b.run(
        "report.export",
        json!({"report": "report.balance_sheet", "args": {"as_of": "2026-12-31", "depth": 1}}),
    );
    assert!(
        csv["content"]
            .as_str()
            .unwrap()
            .contains("Assets,Total assets,1000.00")
    );
}

#[test]
fn at14_void_rules() {
    let mut b = book();
    let i = invoice(&mut b);
    let p = pay(&mut b, "Acme LLC", "2026-09-20", "1000.00");
    let e = b.err("invoice.void", json!({"invoice": "INV-1001"}));
    assert_eq!(e.code(), "rejected");
    b.run(
        "payment.unapply",
        json!({"payment": p["id"], "invoice": "INV-1001"}),
    );
    let v = b.run("invoice.void", json!({"invoice": "INV-1001"}));
    assert_eq!(v["status"], "void");
    let t = b.run("txn.get", json!({"id": i["txn"]}));
    assert_eq!(t["status"], "voided");
    assert_eq!(b.shown("Accounts Receivable", "USD"), "-1000.00");
}

#[test]
fn at15_number_sequences() {
    let mut b = book();
    assert_eq!(invoice(&mut b)["number"], "INV-1001");
    assert_eq!(invoice(&mut b)["number"], "INV-1002");
    let e = b.err(
        "invoice.create",
        json!({"customer": "Acme LLC", "number": "INV-1002", "lines": lines()}),
    );
    assert_eq!(e.code(), "rejected");
    // A typed number does not move the sequence; the sequence skips taken numbers.
    b.run(
        "invoice.create",
        json!({"customer": "Acme LLC", "number": "INV-1003", "lines": lines()}),
    );
    assert_eq!(invoice(&mut b)["number"], "INV-1004");
    // Numbers are never reused after a delete.
    b.run("invoice.delete", json!({"invoice": "INV-1004"}));
    assert_eq!(invoice(&mut b)["number"], "INV-1005");
}

#[test]
fn everything_undoes() {
    let mut b = book();
    let s0 = b.state();
    let docs0 = b.run("invoice.list", json!({}));
    let i = invoice(&mut b);
    let p = pay(&mut b, "Acme LLC", "2026-09-20", "1000.00");
    b.run(
        "deposit.create",
        json!({"bank_account": "Assets:Checking", "date": "2026-09-22", "payments": [p["id"]]}),
    );
    b.run(
        "credit_memo.create",
        json!({"customer": "Acme LLC", "date": "2026-09-25", "lines": [{"item": "Widget", "quantity": "1"}]}),
    );
    b.run("credit_memo.apply", json!({"credit_memo": "CM-1001"}));
    b.run(
        "sales_tax.pay",
        json!({"from_account": "Assets:Checking", "date": "2026-10-15"}),
    );
    b.run(
        "invoice.update",
        json!({"invoice": i["number"], "memo": "edited"}),
    );
    for _ in 0..7 {
        b.run("history.undo", json!({}));
    }
    assert_eq!(b.state(), s0);
    assert_eq!(b.run("invoice.list", json!({})), docs0);
    assert!(
        b.run("payment.list", json!({}))
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        b.run("deposit.list", json!({}))
            .as_array()
            .unwrap()
            .is_empty()
    );
    for _ in 0..7 {
        b.run("history.redo", json!({}));
    }
    assert_eq!(inv(&mut b, "INV-1001")["memo"], "edited");
}

#[test]
fn edit_and_lifecycle() {
    let mut b = book();
    let d = b.run(
        "invoice.create",
        json!({"customer": "Acme LLC", "date": "2026-09-01", "lines": lines(), "status": "draft"}),
    );
    assert_eq!(d["status"], "draft");
    assert!(d["txn"].is_null());
    b.run(
        "invoice.update",
        json!({"invoice": "INV-1001", "lines": [{"item": "Consulting", "quantity": "2"}]}),
    );
    let f = b.run("invoice.finalize", json!({"invoice": "INV-1001"}));
    assert_eq!(f["status"], "open");
    assert_eq!(f["totals"]["total"], "250.00");
    // Editing an open invoice rewrites its one transaction.
    let n = b.txn_count();
    b.run(
        "invoice.update",
        json!({"invoice": "INV-1001", "lines": [{"item": "Consulting", "quantity": "3"}]}),
    );
    assert_eq!(b.txn_count(), n);
    assert_eq!(b.shown("Accounts Receivable", "USD"), "375.00");
    // Closed period.
    b.run("book.set_closing_date", json!({"date": "2026-09-30"}));
    assert!(
        b.try_run(
            "invoice.update",
            json!({"invoice": "INV-1001", "memo": "x"})
        )
        .is_err()
    );
    b.run("history.undo", json!({}));
    // Without items, lines name an income account.
    b.run("module.disable", json!({"module": "items"}));
    let i = b.run(
        "invoice.create",
        json!({"customer": "Acme LLC", "date": "2026-09-01", "lines": [{"account": "Consulting Income", "amount": "80.00", "taxable": true}]}),
    );
    assert_eq!(i["totals"]["tax_total"], "5.80");
    assert!(
        b.try_run(
            "invoice.create",
            json!({"customer": "Acme LLC", "lines": [{"item": "Widget"}]})
        )
        .is_err()
    );
    // Party rules.
    assert!(
        b.try_run("party.create", json!({"name": "acme llc"}))
            .is_err()
    );
    assert!(
        b.try_run("party.create", json!({"name": "X", "email": "no-at"}))
            .is_err()
    );
    assert!(
        b.try_run("party.delete", json!({"party": "Acme LLC"}))
            .is_err()
    );
    b.run("party.create", json!({"name": "Unused"}));
    b.run("party.delete", json!({"party": "Unused"}));
    // Invoice list filters.
    let overdue = b.run(
        "invoice.list",
        json!({"overdue": true, "as_of": "2026-12-01"}),
    );
    assert_eq!(overdue.as_array().unwrap().len(), 2);
    // Sales reports.
    let s = b.run(
        "report.sales_by_customer",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    assert_eq!(s["total"], "455.00");
    b.run("module.enable", json!({"module": "items"}));
    let s = b.run(
        "report.sales_by_item",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    assert_eq!(s["items"][0]["item"], "(No item) Consulting Income");
    assert_eq!(s["items"][1]["item"], "Consulting");
    assert_eq!(s["items"][1]["quantity"], "3");
}
