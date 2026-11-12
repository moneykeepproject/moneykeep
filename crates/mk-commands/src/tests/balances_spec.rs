//! ledger-balances.md acceptance tests (AT-n). AT-5 is in `props.rs`.

use super::*;

fn opened(b: &mut Book) -> String {
    b.standard();
    b.transfer(
        "2026-07-01",
        "Assets:Checking",
        "Opening Balances",
        "1000.00",
    );
    b.transfer(
        "2026-07-10",
        "Expenses:Groceries",
        "Assets:Checking",
        "50.00",
    )
}

#[test]
fn at1_balances_and_trial_balance() {
    let mut b = Book::new();
    opened(&mut b);
    assert_eq!(b.shown("Assets:Checking", "USD"), "950.00");
    assert_eq!(b.shown("Expenses:Groceries", "USD"), "50.00");
    assert_eq!(b.shown("Opening Balances", "USD"), "1000.00");
    let ob = b.run("account.balance", json!({"account": "Opening Balances"}));
    assert_eq!(ob["raw"]["USD"], "-1000.00");
    let tb = b.run("report.trial_balance", json!({}));
    assert_eq!(tb["balanced"], true);
    assert_eq!(tb["value_totals"], json!({"USD": "0.00"}));
    assert_eq!(tb["accounts"].as_array().unwrap().len(), 3);
    assert_eq!(tb["equation"]["holds"], true);
    assert_eq!(tb["equation"]["assets"], "950.00");
}

#[test]
fn at2_as_of_day_before() {
    let mut b = Book::new();
    opened(&mut b);
    let bal = b.run(
        "account.balance",
        json!({"account": "Assets:Checking", "as_of": "2026-07-09"}),
    );
    assert_eq!(bal["display"]["USD"], "1000.00");
    let before_all = b.run(
        "account.balance",
        json!({"account": "Assets:Checking", "as_of": "2026-06-30"}),
    );
    assert_eq!(before_all["raw"], json!({}));
    assert_eq!(before_all["display"], json!({"USD": "0.00"}));
    assert!(before_all.get("shown").is_none());
    let tb = b.run("report.trial_balance", json!({"as_of": "2026-07-09"}));
    assert_eq!(tb["accounts"].as_array().unwrap().len(), 2);
}

#[test]
fn at3_voiding_restores() {
    let mut b = Book::new();
    let purchase = opened(&mut b);
    b.run("txn.void", json!({"id": purchase}));
    assert_eq!(b.shown("Assets:Checking", "USD"), "1000.00");
}

#[test]
fn at4_cleared_basis() {
    let mut b = Book::new();
    let purchase = opened(&mut b);
    // Clear the purchase's Checking split only.
    let t = b.run("txn.get", json!({"id": purchase}));
    let idx = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .position(|s| s["account_name"] == "Assets:Checking")
        .unwrap();
    b.run(
        "txn.set_reconcile",
        json!({"splits": [{"id": purchase, "split": idx, "state": "c"}]}),
    );
    let cleared = b.run(
        "account.balance",
        json!({"account": "Assets:Checking", "basis": "cleared"}),
    );
    assert_eq!(cleared["display"]["USD"], "-50.00");
    let rec = b.run(
        "account.balance",
        json!({"account": "Assets:Checking", "basis": "reconciled"}),
    );
    assert_eq!(rec["raw"], json!({}));
    assert_eq!(rec["display"], json!({"USD": "0.00"}));
    let all = b.run("account.balance", json!({"account": "Assets:Checking"}));
    assert_eq!(all["display"]["USD"], "950.00");
}

#[test]
fn include_children_rolls_up() {
    let mut b = Book::new();
    opened(&mut b);
    let parent = b.run(
        "account.balance",
        json!({"account": "Assets", "include_children": true}),
    );
    assert_eq!(parent["display"]["USD"], "950.00");
    let alone = b.run("account.balance", json!({"account": "Assets"}));
    assert_eq!(alone["raw"], json!({}));
    assert_eq!(alone["display"], json!({"USD": "0.00"}));
}

/// Review item: AT-1 amounts are at the currency's minor-unit scale everywhere.
#[test]
fn at1_amounts_use_currency_scale() {
    let mut b = Book::new();
    b.standard();
    b.transfer("2026-07-01", "Assets:Checking", "Opening Balances", "1000");
    let bal = b.run("account.balance", json!({"account": "Assets:Checking"}));
    assert_eq!(bal["raw"], json!({"USD": "1000.00"}));
    assert_eq!(bal["display"], json!({"USD": "1000.00"}));
    let ob = b.run("account.balance", json!({"account": "Opening Balances"}));
    assert_eq!(ob["raw"], json!({"USD": "-1000.00"}));
    assert_eq!(ob["display"], json!({"USD": "1000.00"}));
    let tb = b.run("report.trial_balance", json!({}));
    assert_eq!(tb["value_totals"], json!({"USD": "0.00"}));
    assert_eq!(tb["accounts"][0]["raw"], json!({"USD": "1000.00"}));
    let eq = &tb["equation"];
    assert_eq!(eq["assets"], "1000.00");
    assert_eq!(eq["liabilities"], "0.00");
    assert_eq!(eq["equity"], "1000.00");
    assert_eq!(eq["income"], "0.00");
    assert_eq!(eq["expenses"], "0.00");
    let t = &b.run("txn.list", json!({}))[0];
    assert_eq!(t["splits"][0]["value"], "1000.00");
    assert_eq!(t["splits"][0]["quantity"], "1000.00");
    assert_eq!(t["splits"][1]["value"], "-1000.00");
}

/// Review item: JPY has no minor unit, so amounts carry no decimals.
#[test]
fn jpy_amounts_have_no_decimals() {
    let mut b = Book::with_currency("JPY");
    b.standard();
    b.transfer("2026-07-01", "Assets:Checking", "Opening Balances", "1000");
    let bal = b.run("account.balance", json!({"account": "Assets:Checking"}));
    assert_eq!(bal["raw"], json!({"JPY": "1000"}));
    assert_eq!(bal["display"], json!({"JPY": "1000"}));
    let empty = b.run("account.balance", json!({"account": "Expenses:Groceries"}));
    assert_eq!(empty["display"], json!({"JPY": "0"}));
    let tb = b.run("report.trial_balance", json!({}));
    assert_eq!(tb["value_totals"], json!({"JPY": "0"}));
    assert_eq!(tb["equation"]["assets"], "1000");
    let t = &b.run("txn.list", json!({}))[0];
    assert_eq!(t["splits"][0]["value"], "1000");
}

/// Security quantities keep their significant digits and are not padded.
#[test]
fn security_quantities_keep_their_digits() {
    let mut b = Book::new();
    b.account(json!({"name": "Cash", "type": "asset", "subtype": "cash"}));
    b.account(json!({"name": "VTI", "type": "asset", "subtype": "security", "commodity": "VTI"}));
    b.run(
        "txn.create",
        json!({"date": "2026-07-01", "splits": [
        {"account": "VTI", "value": "1000.00", "quantity": "4.125"},
        {"account": "Cash", "value": "-1000"}]}),
    );
    b.run(
        "txn.create",
        json!({"date": "2026-07-02", "splits": [
        {"account": "VTI", "value": "500.00", "quantity": "1.875000"},
        {"account": "Cash", "value": "-500"}]}),
    );
    assert_eq!(b.shown("VTI", "VTI"), "6");
    assert_eq!(b.shown("Cash", "USD"), "-1500.00");
    let first = &b.run("txn.list", json!({}))[0];
    let vti = first["splits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["account_name"] == "VTI")
        .unwrap();
    assert_eq!(vti["quantity"], "4.125");
}
