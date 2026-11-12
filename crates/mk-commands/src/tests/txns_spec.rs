//! ledger-transactions.md acceptance tests (AT-n) and edge cases.

use super::*;

fn grocery(b: &mut Book, date: &str, amount: &str) -> String {
    b.transfer(date, "Expenses:Groceries", "Assets:Checking", amount)
}

#[test]
fn at1_purchase_is_debit_expense_credit_asset_any_order() {
    let mut b = Book::new();
    b.standard();
    let one = b.run(
        "txn.create",
        json!({"date": "2026-07-01", "splits": [
        {"account": "Expenses:Groceries", "value": "50.00"},
        {"account": "Assets:Checking", "value": "-50.00"}]}),
    );
    let two = b.run(
        "txn.create",
        json!({"date": "2026-07-01", "splits": [
        {"account": "Assets:Checking", "value": "-50"},
        {"account": "Expenses:Groceries", "value": "50.00"}]}),
    );
    assert_eq!(one["splits"], two["splits"]);
    assert_eq!(one["splits"][0]["account_name"], "Expenses:Groceries");
    assert_eq!(one["splits"][0]["value"], "50.00");
    assert_eq!(one["splits"][1]["value"], "-50.00");
    assert_eq!(b.shown("Expenses:Groceries", "USD"), "100.00");
    assert_eq!(b.shown("Assets:Checking", "USD"), "-100.00");
}

#[test]
fn at2_unbalanced_rejected_with_difference() {
    let mut b = Book::new();
    b.standard();
    let e = b.err(
        "txn.create",
        json!({"date": "2026-07-01", "splits": [
        {"account": "Expenses:Groceries", "value": "100.00"},
        {"account": "Assets:Checking", "value": "-99.99"}]}),
    );
    assert_eq!(e.code(), "invalid_args");
    assert!(e.to_string().contains("difference 0.01"), "{e}");
    assert_eq!(b.audit_len(), 6, "only the account creations");
}

#[test]
fn at3_too_many_decimals_rejected() {
    let mut b = Book::new();
    b.standard();
    let e = b.err(
        "txn.create",
        json!({"date": "2026-07-01", "splits": [
        {"account": "Expenses:Groceries", "value": "10.005"},
        {"account": "Assets:Checking", "value": "-10.005"}]}),
    );
    assert_eq!(e.code(), "invalid_args");
    assert!(e.to_string().contains("decimal places"), "{e}");
}

#[test]
fn jpy_has_no_decimals() {
    let mut b = Book::with_currency("JPY");
    b.standard();
    assert_eq!(
        b.err(
            "txn.create",
            json!({"date": "2026-07-01", "splits": [
            {"account": "Expenses:Groceries", "value": "100.5"},
            {"account": "Assets:Checking", "value": "-100.5"}]})
        )
        .code(),
        "invalid_args"
    );
    let t = b.run(
        "txn.create",
        json!({"date": "2026-07-01", "splits": [
        {"account": "Expenses:Groceries", "value": "100"},
        {"account": "Assets:Checking", "value": "-100"}]}),
    );
    assert_eq!(t["splits"][0]["value"], "100");
}

fn brokerage(b: &mut Book) {
    b.account(
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "placeholder": true}),
    );
    b.account(json!({"name": "Cash", "type": "asset", "subtype": "cash", "parent": "Brokerage"}));
    b.account(json!({"name": "VTI", "type": "asset", "subtype": "security", "commodity": "VTI", "parent": "Brokerage"}));
}

#[test]
fn at4_security_purchase() {
    let mut b = Book::new();
    brokerage(&mut b);
    b.transfer(
        "2026-07-01",
        "Brokerage:Cash",
        "Opening Balances",
        "3000.00",
    );
    b.run(
        "txn.create",
        json!({"date": "2026-07-02", "splits": [
        {"account": "Brokerage:VTI", "value": "2500.00", "quantity": "10"},
        {"account": "Brokerage:Cash", "value": "-2500.00"}]}),
    );
    assert_eq!(b.shown("Brokerage:VTI", "VTI"), "10");
    let all = b.run(
        "account.balance",
        json!({"account": "Brokerage", "include_children": true}),
    );
    assert_eq!(all["display"], json!({"USD": "500.00", "VTI": "10"}));
}

#[test]
fn at5_sign_mismatch_rejected() {
    let mut b = Book::new();
    brokerage(&mut b);
    let e = b.err(
        "txn.create",
        json!({"date": "2026-07-02", "splits": [
        {"account": "Brokerage:VTI", "value": "2500.00", "quantity": "-10"},
        {"account": "Brokerage:Cash", "value": "-2500.00"}]}),
    );
    assert!(e.to_string().contains("same sign"), "{e}");
    // Quantity is required for a security split.
    let e = b.err(
        "txn.create",
        json!({"date": "2026-07-02", "splits": [
        {"account": "Brokerage:VTI", "value": "2500.00"},
        {"account": "Brokerage:Cash", "value": "-2500.00"}]}),
    );
    assert_eq!(e.code(), "invalid_args");
    // A stock split: zero value, non-zero quantity.
    b.run(
        "txn.create",
        json!({"date": "2026-07-02", "splits": [
        {"account": "Brokerage:VTI", "value": "0", "quantity": "10"},
        {"account": "Brokerage:Cash", "value": "0"}]}),
    );
}

#[test]
fn at6_closing_date() {
    let mut b = Book::new();
    b.standard();
    b.run("book.set_closing_date", json!({"date": "2026-06-30"}));
    let e = b.err(
        "txn.create",
        json!({"date": "2026-06-30", "splits": [
        {"account": "Expenses:Groceries", "value": "1.00"},
        {"account": "Assets:Checking", "value": "-1.00"}]}),
    );
    assert_eq!(e.code(), "rejected");
    let id = grocery(&mut b, "2026-07-01", "1.00");
    let e = b.err(
        "txn.update",
        json!({"id": id, "date": "2026-06-15", "splits": [
        {"account": "Expenses:Groceries", "value": "1.00"},
        {"account": "Assets:Checking", "value": "-1.00"}]}),
    );
    assert_eq!(e.code(), "rejected");
    assert!(e.to_string().contains("closed"), "{e}");
    // Clearing the closing date reopens the period.
    b.run("book.set_closing_date", json!({"date": null}));
    grocery(&mut b, "2026-06-30", "1.00");
}

#[test]
fn closed_period_blocks_void_delete_and_reconcile() {
    let mut b = Book::new();
    b.standard();
    let id = grocery(&mut b, "2026-06-01", "1.00");
    b.run("book.set_closing_date", json!({"date": "2026-06-30"}));
    for (cmd, args) in [
        ("txn.void", json!({"id": id})),
        ("txn.delete", json!({"id": id})),
        (
            "txn.set_reconcile",
            json!({"splits": [{"id": id, "split": 0, "state": "c"}]}),
        ),
    ] {
        assert_eq!(b.err(cmd, args).code(), "rejected", "{cmd}");
    }
    // Reversing into an open period is the correction path.
    b.run("txn.reverse", json!({"id": id, "date": "2026-07-01"}));
}

#[test]
fn at7_void_and_unvoid() {
    let mut b = Book::new();
    b.standard();
    let id = grocery(&mut b, "2026-07-01", "50.00");
    assert_eq!(b.shown("Expenses:Groceries", "USD"), "50.00");
    let v = b.run("txn.void", json!({"id": id, "reason": "duplicate"}));
    assert_eq!(v["status"], "voided");
    assert_eq!(v["void_reason"], "duplicate");
    assert_eq!(b.shown("Expenses:Groceries", "USD"), "0.00");
    // Still visible with include_voided.
    assert_eq!(b.run("txn.list", json!({})).as_array().unwrap().len(), 0);
    assert_eq!(
        b.run("txn.list", json!({"include_voided": true}))
            .as_array()
            .unwrap()
            .len(),
        1
    );
    b.run("txn.unvoid", json!({"id": id}));
    assert_eq!(b.shown("Expenses:Groceries", "USD"), "50.00");
}

#[test]
fn at8_reconciled_split_value_locked_memo_free() {
    let mut b = Book::new();
    b.standard();
    let id = grocery(&mut b, "2026-07-01", "50.00");
    b.run(
        "txn.set_reconcile",
        json!({"splits": [{"id": id, "split": 1, "state": "r"}]}),
    );
    let e = b.err(
        "txn.update",
        json!({"id": id, "date": "2026-07-01", "splits": [
        {"account": "Expenses:Groceries", "value": "60.00"},
        {"account": "Assets:Checking", "value": "-60.00"}]}),
    );
    assert_eq!(e.code(), "rejected");
    assert!(e.to_string().contains("reconciled"), "{e}");
    let t = b.run(
        "txn.update",
        json!({"id": id, "date": "2026-07-01", "payee": "Market", "splits": [
        {"account": "Expenses:Groceries", "value": "50.00"},
        {"account": "Assets:Checking", "value": "-50.00", "memo": "statement 7"}]}),
    );
    assert_eq!(t["splits"][1]["memo"], "statement 7");
    assert_eq!(t["splits"][1]["reconcile"], "r");
    assert_eq!(t["splits"][0]["reconcile"], "n");
    // Deleting a transaction with a reconciled split is rejected.
    assert_eq!(b.err("txn.delete", json!({"id": id})).code(), "rejected");
    // Unreconciling is allowed; then the value can change.
    b.run(
        "txn.set_reconcile",
        json!({"splits": [{"id": id, "split": 1, "state": "n"}]}),
    );
    b.run(
        "txn.update",
        json!({"id": id, "date": "2026-07-01", "splits": [
        {"account": "Expenses:Groceries", "value": "60.00"},
        {"account": "Assets:Checking", "value": "-60.00"}]}),
    );
}

#[test]
fn at9_reverse_negates_every_split() {
    let mut b = Book::new();
    b.standard();
    b.run(
        "txn.create",
        json!({"date": "2026-07-01", "number": "1001", "splits": [
        {"account": "Expenses:Groceries", "value": "30.00"},
        {"account": "Liabilities:Visa", "value": "20.00"},
        {"account": "Assets:Checking", "value": "-50.00"}]}),
    );
    let orig = b.run("txn.list", json!({}))[0].clone();
    let rev = b.run(
        "txn.reverse",
        json!({"id": orig["id"], "date": "2026-07-05"}),
    );
    assert_eq!(rev["memo"], "Reversal of 1001");
    assert_eq!(rev["reverses"], orig["id"]);
    assert_eq!(rev["date"], "2026-07-05");
    for acc in ["Expenses:Groceries", "Liabilities:Visa", "Assets:Checking"] {
        assert_eq!(b.shown(acc, "USD"), "0.00", "{acc}");
    }
}

#[test]
fn at10_delete_keeps_full_content_in_audit() {
    let mut b = Book::new();
    b.standard();
    let id = grocery(&mut b, "2026-07-01", "50.00");
    let before = b.run("txn.get", json!({"id": id}));
    b.run("txn.delete", json!({"id": id}));
    assert_eq!(b.err("txn.get", json!({"id": id})).code(), "not_found");
    let entry = &b.s.audit_entries(1).unwrap()[0];
    assert_eq!(entry.command, "txn.delete");
    assert!(entry.after.is_none());
    let snap = &entry.before.as_ref().unwrap()[0];
    assert_eq!(snap["kind"], "transaction");
    assert_eq!(snap["id"], json!(id));
    assert_eq!(snap["data"]["splits"].as_array().unwrap().len(), 2);
    assert_eq!(
        snap["data"]["splits"][0]["value"],
        before["splits"][0]["value"]
    );
    assert_eq!(entry.result["transaction"]["splits"], before["splits"]);
}

#[test]
fn foreign_transaction_gated_but_security_purchase_allowed() {
    let mut b = Book::new();
    b.standard();
    let e = b.err(
        "txn.create",
        json!({"date": "2026-07-01", "currency": "EUR", "splits": [
        {"account": "Expenses:Groceries", "value": "10.00", "quantity": "11.00"},
        {"account": "Assets:Checking", "value": "-10.00", "quantity": "-11.00"}]}),
    );
    assert_eq!(e.code(), "rejected");
    b.run("module.enable", json!({"module": "multi-currency"}));
    b.run(
        "txn.create",
        json!({"date": "2026-07-01", "currency": "EUR", "splits": [
        {"account": "Expenses:Groceries", "value": "10.00", "quantity": "11.00"},
        {"account": "Assets:Checking", "value": "-10.00", "quantity": "-11.00"}]}),
    );
    let tb = b.run("report.trial_balance", json!({}));
    assert_eq!(tb["value_totals"], json!({"EUR": "0.00"}));
}

#[test]
fn compound_paycheck_and_duplicate_account_splits() {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Salary", "type": "income"}));
    b.account(json!({"name": "Tax", "type": "expense", "parent": "Expenses"}));
    let t = b.run(
        "txn.create",
        json!({"date": "2026-07-01", "payee": "Employer", "splits": [
        {"account": "Salary", "value": "-3000.00"},
        {"account": "Expenses:Tax", "value": "600.00"},
        {"account": "Assets:Checking", "value": "2000.00", "memo": "net"},
        {"account": "Assets:Checking", "value": "400.00", "memo": "bonus"}]}),
    );
    assert_eq!(t["splits"].as_array().unwrap().len(), 4);
    assert_eq!(b.shown("Salary", "USD"), "3000.00");
    assert_eq!(b.shown("Assets:Checking", "USD"), "2400.00");
}

#[test]
fn list_filters_and_register_order() {
    let mut b = Book::new();
    b.standard();
    let late = grocery(&mut b, "2026-07-03", "3.00");
    let early = grocery(&mut b, "2026-07-01", "1.00");
    let same_day = grocery(&mut b, "2026-07-01", "2.00");
    b.transfer("2026-07-02", "Assets:Checking", "Opening Balances", "9.00");
    let ids = |v: Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let g = ids(b.run("txn.list", json!({"account": "Expenses:Groceries"})));
    assert_eq!(g, [early.clone(), same_day.clone(), late.clone()]);
    // Parent with children (default) vs without.
    assert_eq!(
        ids(b.run("txn.list", json!({"account": "Expenses"}))).len(),
        3
    );
    assert_eq!(
        ids(b.run(
            "txn.list",
            json!({"account": "Expenses", "include_children": false})
        ))
        .len(),
        0
    );
    let ranged = ids(b.run(
        "txn.list",
        json!({"from": "2026-07-02", "to": "2026-07-03"}),
    ));
    assert_eq!(ranged.len(), 2);
    let page = ids(b.run("txn.list", json!({"limit": 2, "offset": 1})));
    assert_eq!(page.len(), 2);
    assert_eq!(page[0], same_day);
}

#[test]
fn field_limits() {
    let mut b = Book::new();
    b.standard();
    let long = "x".repeat(201);
    let e = b.err(
        "txn.create",
        json!({"date": "2026-07-01", "payee": long, "splits": [
        {"account": "Expenses:Groceries", "value": "1.00"},
        {"account": "Assets:Checking", "value": "-1.00"}]}),
    );
    assert_eq!(e.code(), "invalid_args");
    let e = b.err(
        "txn.create",
        json!({"date": "2026-07-01", "splits": [
        {"account": "Expenses:Groceries", "value": "0"},
        {"account": "Assets:Checking", "value": "0"}]}),
    );
    assert_eq!(e.code(), "invalid_args");
    let e = b.err("txn.create", json!({"date": "2026-02-30", "splits": []}));
    assert_eq!(e.code(), "invalid_args");
    let e = b.err(
        "txn.create",
        json!({"date": "2026-07-01", "splits": [
        {"account": "Expenses:Groceries", "value": 1.0},
        {"account": "Assets:Checking", "value": "-1.00"}]}),
    );
    assert_eq!(
        e.code(),
        "invalid_args",
        "JSON numbers are not accepted for amounts"
    );
}
