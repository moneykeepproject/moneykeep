//! release-prep.md acceptance tests: RP-T1–RP-T3 (the Tax summary's investment
//! income section), the billed-link rule and the billable income setting of
//! projects (§4).

use super::*;

fn crypto_book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "investments"}));
    b.account(
        json!({"name": "Exchange", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    b.transfer(
        "2025-01-02",
        "Assets:Exchange",
        "Assets:Checking",
        "20000.00",
    );
    b.run(
        "security.create",
        json!({"symbol": "BTC", "name": "Bitcoin", "kind": "crypto"}),
    );
    ex(
        &mut b,
        "inv.buy",
        json!({"security": "BTC", "date": "2026-01-10", "quantity": "0.1", "amount": "4000.00"}),
    );
    // A coin fee worth 6.00 (basis 4.00: a short-term gain of 2.00).
    ex(
        &mut b,
        "inv.crypto_fee",
        json!({"security": "BTC", "date": "2026-02-10", "quantity": "0.0001", "value": "6.00"}),
    );
    b
}

fn ex(b: &mut Book, cmd: &str, args: Value) -> Value {
    let mut a = json!({"account": "Assets:Exchange"});
    for (k, v) in args.as_object().unwrap() {
        a[k] = v.clone();
    }
    b.run(cmd, a)
}

fn summary(b: &mut Book) -> Value {
    b.run(
        "report.tax_summary",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    )
}

fn row<'a>(t: &'a Value, key: &str) -> Vec<&'a Value> {
    t["investment_income"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["key"] == key)
        .collect()
}

/// The activity rows (tax line, account, amount) that name `account`.
fn under_tax_lines(t: &Value, account: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for g in t["activity"].as_array().unwrap() {
        for r in g["accounts"].as_array().unwrap() {
            if r["full_name"] == account {
                out.push((
                    g["tax_line"].as_str().unwrap().to_owned(),
                    r["amount"].as_str().unwrap().to_owned(),
                ));
            }
        }
    }
    out
}

/// RP-T1. Investment Fees tagged with a tax line and a coin fee of 6.00: the fee is
/// listed once under that tax line and marked "Included above under …" in the
/// investment income section, whose total leaves it out.
#[test]
fn rp_t1_tagged_fee_included_above() {
    let mut b = crypto_book();
    b.run(
        "account.update",
        json!({"account": "Investment Fees", "tax_line": "Sch A L16"}),
    );
    let t = summary(&mut b);
    assert_eq!(
        under_tax_lines(&t, "Investment Fees"),
        vec![("Sch A L16".to_owned(), "6.00".to_owned())]
    );
    let fees = row(&t, "fees");
    assert_eq!(fees.len(), 1, "{t}");
    assert_eq!(fees[0]["amount"], "6.00");
    assert_eq!(fees[0]["included_in"], "Sch A L16");
    assert_eq!(fees[0]["note"], "Included above under Sch A L16");
    let inc = &t["investment_income"];
    assert_eq!(
        inc["note"],
        "Details of investment income. Amounts already listed under a tax line above are marked and not added again."
    );
    // Only the 2.00 gain counts.
    assert_eq!(inc["total"], "2.00", "{inc}");
    // CSV: the tax line in the Included in column.
    let csv = b.run(
        "report.export",
        json!({"report": "report.tax_summary",
               "args": {"from": "2026-01-01", "to": "2026-12-31"}}),
    );
    let csv = csv["content"].as_str().unwrap();
    assert!(
        csv.lines().next().unwrap().ends_with(",Included in"),
        "{csv}"
    );
    assert!(
        csv.contains("Investment income,,Fees,,6.00,,,Sch A L16"),
        "{csv}"
    );
    // PDF builds with the note.
    let pdf = b.run(
        "report.export",
        json!({"report": "tax_summary", "format": "pdf",
               "args": {"from": "2026-01-01", "to": "2026-12-31"}}),
    );
    assert!(pdf.to_string().contains("tax-summary"), "{pdf}");
}

/// RP-T2. The same book without the tag: the fee counts in the section total and is
/// not under any tax line.
#[test]
fn rp_t2_untagged_fee_counts() {
    let mut b = crypto_book();
    let t = summary(&mut b);
    assert!(under_tax_lines(&t, "Investment Fees").is_empty(), "{t}");
    let fees = row(&t, "fees");
    assert_eq!(fees.len(), 1);
    assert!(fees[0]["included_in"].is_null());
    assert!(fees[0].get("note").is_none());
    // 2.00 gain − 6.00 fee.
    assert_eq!(t["investment_income"]["total"], "-4.00");
}

/// RP-T3. A realized gain always counts in the section total, even when the Realized
/// Gains account has a tax line.
#[test]
fn rp_t3_realized_gain_always_counts() {
    let mut b = crypto_book();
    ex(
        &mut b,
        "inv.sell",
        json!({"security": "BTC", "date": "2026-03-01", "quantity": "0.05", "amount": "2500.00"}),
    );
    b.run(
        "account.update",
        json!({"account": "Realized Gains", "tax_line": "Sch D L7"}),
    );
    let t = summary(&mut b);
    let gains = row(&t, "realized_short");
    assert_eq!(gains.len(), 1, "{t}");
    // 2.00 (fee) + 500.00 (sale).
    assert_eq!(gains[0]["amount"], "502.00");
    assert!(gains[0]["included_in"].is_null());
    assert_eq!(t["investment_income"]["total"], "496.00");
}

fn billed_book() -> (Book, String, Value) {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "projects"}));
    b.run("module.enable", json!({"module": "invoicing"}));
    b.account(json!({"name": "Materials", "type": "expense", "parent": "Expenses"}));
    b.account(json!({"name": "Supplies", "type": "expense", "parent": "Expenses"}));
    b.run(
        "party.create",
        json!({"name": "Smith Family", "roles": ["customer"]}),
    );
    b.run(
        "project.create",
        json!({"name": "Roof repair", "customer": "Smith Family", "billable": true}),
    );
    let id = b.run(
        "txn.create",
        json!({"date": "2026-07-02", "payee": "Home Depot", "splits": [
            {"account": "Expenses:Materials", "value": "200.00", "project": "Roof repair", "billable": true},
            {"account": "Expenses:Supplies", "value": "30.00"},
            {"account": "Assets:Checking", "value": "-230.00"}]}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let u = b.run("project.unbilled", json!({}));
    let keys: Vec<Value> = u["projects"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].clone())
        .collect();
    assert_eq!(keys.len(), 1);
    assert!(keys[0].as_str().unwrap().starts_with("split:"), "{keys:?}");
    let lines = b.run("project.billable_lines", json!({"items": keys}));
    let inv = b.run(
        "invoice.create",
        json!({"customer": "Smith Family", "date": "2026-07-31", "lines": lines["lines"]}),
    );
    (b, id, inv)
}

fn billed(b: &mut Book) -> (Vec<Value>, usize) {
    let g = b.run("project.get", json!({"project": "Roof repair"}));
    (
        g["billable_items"]["billed"].as_array().unwrap().clone(),
        g["billable_items"]["unbilled"].as_array().unwrap().len(),
    )
}

/// release-prep.md §4: an invoice line's link to the cost it billed is kept by the
/// split's own id: an edit that moves the billed line to another position (lines
/// are stored debits first, largest first) keeps the item billed with its invoice
/// number, and so does editing its amount.
#[test]
fn rp_billed_link_survives_reordering_lines() {
    let (mut b, id, inv) = billed_book();
    let (done, open) = billed(&mut b);
    assert_eq!((done.len(), open), (1, 0));
    let t = b.run("txn.get", json!({"id": id}));
    assert_eq!(t["splits"][0]["account_name"], "Expenses:Materials");
    // Supplies now larger: the billed Materials line moves from first to second.
    b.run(
        "txn.update",
        json!({"id": id, "date": "2026-07-02", "payee": "Home Depot", "splits": [
            {"account": "Assets:Checking", "value": "-700.00"},
            {"account": "Expenses:Supplies", "value": "500.00"},
            {"account": "Expenses:Materials", "value": "200.00"}]}),
    );
    let t = b.run("txn.get", json!({"id": id}));
    assert_eq!(t["splits"][1]["account_name"], "Expenses:Materials", "{t}");
    let (done, open) = billed(&mut b);
    assert_eq!(done.len(), 1, "still billed");
    assert_eq!(open, 0);
    assert_eq!(done[0]["invoice_number"], inv["number"]);
    assert_eq!(done[0]["amount"], "200.00");
    // An amount edit keeps the line's id too.
    b.run(
        "txn.update",
        json!({"id": id, "date": "2026-07-02", "payee": "Home Depot", "splits": [
            {"account": "Expenses:Materials", "value": "210.00"},
            {"account": "Assets:Checking", "value": "-710.00"},
            {"account": "Expenses:Supplies", "value": "500.00"}]}),
    );
    let (done, open) = billed(&mut b);
    assert_eq!((done.len(), open), (1, 0));
    assert_eq!(done[0]["amount"], "210.00");
    // Deleting the transaction and undoing the delete keeps the link.
    b.run("txn.delete", json!({"id": id}));
    assert!(billed(&mut b).0.is_empty());
    b.run("history.undo", json!({}));
    let (done, _) = billed(&mut b);
    assert_eq!(done.len(), 1);
    assert_eq!(done[0]["invoice_number"], inv["number"]);
}

/// release-prep.md §4: "Billable costs post to" shows Billable Expense Income once
/// the account exists (it is made on first use).
#[test]
fn rp_billable_income_setting_shows_the_account() {
    let label = |b: &mut Book| {
        let s = b.run("module.settings", json!({"module": "projects"}));
        s["settings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["key"] == "projects.billable_income_account")
            .unwrap()["default_label"]
            .clone()
    };
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "projects"}));
    assert_eq!(label(&mut b), "Billable Expense Income (made on first use)");
    let (mut b, _, _) = billed_book();
    assert_eq!(label(&mut b), "Billable Expense Income");
}
