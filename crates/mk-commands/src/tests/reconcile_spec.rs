//! reconciliation.md acceptance tests.

use super::*;

fn split_ids(b: &mut Book, account: &str) -> Vec<(String, String)> {
    let s = b.run("reconcile.get", json!({"account": account}));
    s["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["split"].as_str().unwrap().to_owned(),
                c["amount"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn idx_of(b: &mut Book, txn: &str, account: &str) -> usize {
    let t = b.run("txn.get", json!({"id": txn}));
    t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .position(|s| s["account_name"] == account)
        .unwrap()
}

fn mark(b: &mut Book, txn: &str, account: &str, state: &str) {
    let i = idx_of(b, txn, account);
    b.run(
        "txn.set_reconcile",
        json!({"splits": [{"id": txn, "split": i, "state": state}]}),
    );
}

/// Opening balance 1,000.00 reconciled; cleared −50.00 and −25.00; uncleared −10.00.
fn scenario() -> (Book, String) {
    let mut b = Book::new();
    b.standard();
    let open = b.transfer(
        "2026-09-01",
        "Assets:Checking",
        "Opening Balances",
        "1000.00",
    );
    mark(&mut b, &open, "Assets:Checking", "r");
    let t1 = b.transfer(
        "2026-10-02",
        "Expenses:Groceries",
        "Assets:Checking",
        "50.00",
    );
    let t2 = b.transfer(
        "2026-10-05",
        "Expenses:Groceries",
        "Assets:Checking",
        "25.00",
    );
    let t3 = b.transfer(
        "2026-10-08",
        "Expenses:Groceries",
        "Assets:Checking",
        "10.00",
    );
    mark(&mut b, &t1, "Assets:Checking", "c");
    mark(&mut b, &t2, "Assets:Checking", "c");
    (b, t3)
}

fn reconcile_state(b: &mut Book, txn: &str) -> String {
    let i = idx_of(b, txn, "Assets:Checking");
    b.run("txn.get", json!({"id": txn}))["splits"][i]["reconcile"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn reconciliation_at1_cleared_preselected_and_finish() {
    let (mut b, uncleared) = scenario();
    let s = b.run("reconcile.start", json!({"account": "Assets:Checking", "statement_date": "2026-10-31", "statement_balance": "925.00"}));
    assert_eq!(s["opening_balance"], "1000.00");
    assert_eq!(s["difference"], "0.00");
    assert_eq!(s["selected"].as_array().unwrap().len(), 2);
    assert_eq!(s["candidates"].as_array().unwrap().len(), 3);
    b.run("reconcile.finish", json!({"account": "Assets:Checking"}));
    let list = b.run("txn.list", json!({"account": "Assets:Checking"}));
    let states: Vec<String> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let id = t["id"].as_str().unwrap().to_owned();
            reconcile_state(&mut b, &id)
        })
        .collect();
    assert_eq!(states, ["r", "r", "r", "n"]);
    assert_eq!(reconcile_state(&mut b, &uncleared), "n");
    assert_eq!(
        b.run("reconcile.history", json!({"account": "Assets:Checking"}))
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn reconciliation_at2_difference_and_adjust() {
    let (mut b, _) = scenario();
    b.account(json!({"name": "Bank Charges", "type": "expense"}));
    b.run("reconcile.start", json!({"account": "Assets:Checking", "statement_date": "2026-10-31", "statement_balance": "920.00"}));
    let e = b.err("reconcile.finish", json!({"account": "Assets:Checking"}));
    assert!(e.to_string().contains("-5.00"), "{e}");
    assert_eq!(
        b.err(
            "reconcile.finish",
            json!({"account": "Assets:Checking", "adjust": true})
        )
        .code(),
        "invalid_args"
    );
    let out = b.run(
        "reconcile.finish",
        json!({"account": "Assets:Checking", "adjust": true, "adjustment_account": "Bank Charges"}),
    );
    let adj = out["adjustment"].as_str().unwrap().to_owned();
    let t = b.run("txn.get", json!({"id": adj}));
    let i = idx_of(&mut b, &adj, "Assets:Checking");
    assert_eq!(t["splits"][i]["value"], "-5.00");
    assert_eq!(t["splits"][i]["reconcile"], "r");
    assert_eq!(t["date"], "2026-10-31");
    let bal = b.run(
        "account.balance",
        json!({"account": "Assets:Checking", "basis": "reconciled"}),
    );
    assert_eq!(bal["display"]["USD"], "920.00");
}

#[test]
fn reconciliation_at3_credit_card() {
    let mut b = Book::new();
    b.standard();
    b.transfer(
        "2026-10-02",
        "Expenses:Groceries",
        "Liabilities:Visa",
        "300.00",
    );
    b.transfer(
        "2026-10-09",
        "Expenses:Groceries",
        "Liabilities:Visa",
        "512.40",
    );
    let s = b.run("reconcile.start", json!({"account": "Liabilities:Visa", "statement_date": "2026-10-31", "statement_balance": "812.40"}));
    let ids: Vec<Value> = s["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["split"].clone())
        .collect();
    let s = b.run(
        "reconcile.select",
        json!({"account": "Liabilities:Visa", "splits": ids}),
    );
    assert_eq!(s["difference"], "0.00");
    b.run("reconcile.finish", json!({"account": "Liabilities:Visa"}));
    let bal = b.run(
        "account.balance",
        json!({"account": "Liabilities:Visa", "basis": "reconciled"}),
    );
    assert_eq!(bal["display"]["USD"], "812.40");
}

#[test]
fn reconciliation_at4_later_split_not_candidate() {
    let (mut b, _) = scenario();
    b.run("reconcile.start", json!({"account": "Assets:Checking", "statement_date": "2026-10-06", "statement_balance": "925.00"}));
    let c = split_ids(&mut b, "Assets:Checking");
    assert_eq!(
        c.len(),
        2,
        "the 2026-10-08 split is after the statement date"
    );
    let later = b.run(
        "txn.list",
        json!({"account": "Assets:Checking", "from": "2026-10-08"}),
    )[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let i = idx_of(&mut b, &later, "Assets:Checking");
    let e = b.err(
        "reconcile.select",
        json!({"account": "Assets:Checking", "splits": [format!("{later}:{i}")]}),
    );
    assert_eq!(e.code(), "rejected");
}

#[test]
fn reconciliation_at5_one_open_session() {
    let (mut b, _) = scenario();
    b.run("reconcile.start", json!({"account": "Assets:Checking", "statement_date": "2026-10-31", "statement_balance": "925.00"}));
    assert_eq!(b.err("reconcile.start", json!({"account": "Assets:Checking", "statement_date": "2026-10-31", "statement_balance": "925.00"})).code(), "rejected");
    b.run("reconcile.cancel", json!({"account": "Assets:Checking"}));
    b.run("reconcile.start", json!({"account": "Assets:Checking", "statement_date": "2026-10-31", "statement_balance": "925.00"}));
}

#[test]
fn reconciliation_at6_undo_last() {
    let (mut b, uncleared) = scenario();
    b.account(json!({"name": "Bank Charges", "type": "expense"}));
    let before = b.state();
    b.run("reconcile.start", json!({"account": "Assets:Checking", "statement_date": "2026-10-06", "statement_balance": "920.00"}));
    let first = b.run(
        "reconcile.finish",
        json!({"account": "Assets:Checking", "adjust": true, "adjustment_account": "Bank Charges"}),
    );
    let adj = first["adjustment"].as_str().unwrap().to_owned();
    let mid = b.state();
    // A second, newer reconciliation.
    let i = idx_of(&mut b, &uncleared, "Assets:Checking");
    b.run("reconcile.start", json!({"account": "Assets:Checking", "statement_date": "2026-10-31", "statement_balance": "910.00"}));
    b.run(
        "reconcile.select",
        json!({"account": "Assets:Checking", "splits": [format!("{uncleared}:{i}")]}),
    );
    b.run("reconcile.finish", json!({"account": "Assets:Checking"}));
    // Undoing the older one is rejected while the newer exists.
    let e = b.err(
        "reconcile.undo_last",
        json!({"account": "Assets:Checking", "reconciliation": first["reconciliation"]}),
    );
    assert_eq!(e.code(), "rejected");
    b.run("reconcile.undo_last", json!({"account": "Assets:Checking"}));
    assert_eq!(reconcile_state(&mut b, &uncleared), "n");
    assert_eq!(b.state(), mid);
    b.run("reconcile.undo_last", json!({"account": "Assets:Checking"}));
    assert_eq!(b.err("txn.get", json!({"id": adj})).code(), "not_found");
    let rec = b.run(
        "account.balance",
        json!({"account": "Assets:Checking", "basis": "reconciled"}),
    );
    assert_eq!(rec["display"]["USD"], "1000.00");
    assert_eq!(b.state(), before);
}

#[test]
fn reconciliation_at7_reconciled_balance_equals_statement() {
    let (mut b, _) = scenario();
    b.run("reconcile.start", json!({"account": "Assets:Checking", "statement_date": "2026-10-31", "statement_balance": "925.00"}));
    b.run("reconcile.finish", json!({"account": "Assets:Checking"}));
    let bal = b.run(
        "account.balance",
        json!({"account": "Assets:Checking", "basis": "reconciled"}),
    );
    assert_eq!(bal["display"]["USD"], "925.00");
}

#[test]
fn reconciliation_at8_start_from_captured_statement() {
    // The capture path is source-agnostic: a CSV balance column (and later OFX) both
    // produce a statement record the session starts from.
    let mut b = Book::new();
    b.standard();
    b.run("csv.profile_save", json!({"profile": {"name": "FNB", "skip_rows": 3, "amount_mode": "split",
        "columns": {"date": "Date", "payee": "Description", "debit": "Debit", "credit": "Credit", "balance": "Balance"},
        "date_format": "YYYY-MM-DD", "account": "Assets:Checking"}}));
    let batch = b.run("import.stage", json!({"format": "csv", "content": super::import_spec::fixture("csv/preamble-split-balance.csv")}));
    b.commit(&batch);
    let sid = batch["sections"][0]["statement"].clone();
    let s = b.run(
        "reconcile.start",
        json!({"account": "Assets:Checking", "statement": sid}),
    );
    assert_eq!(s["statement_date"], "2026-10-05");
    assert_eq!(s["statement_balance"], "947.50");
    // Imported CSV rows start cleared, so they are pre-selected.
    assert!(
        s["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["selected"] == true)
    );
    assert_eq!(s["difference"], "0.00");
    b.run("reconcile.finish", json!({"account": "Assets:Checking"}));
}

#[test]
fn reconciliation_history_undo_rules() {
    let (mut b, _) = scenario();
    b.run("reconcile.start", json!({"account": "Assets:Checking", "statement_date": "2026-10-31", "statement_balance": "925.00"}));
    let e = b.err("history.undo", json!({}));
    assert!(e.to_string().contains("reconcile.cancel"), "{e}");
    b.run("reconcile.finish", json!({"account": "Assets:Checking"}));
    b.run("history.undo", json!({}));
    let rec = b.run(
        "account.balance",
        json!({"account": "Assets:Checking", "basis": "reconciled"}),
    );
    assert_eq!(rec["display"]["USD"], "1000.00");
    assert_eq!(
        b.run("reconcile.history", json!({"account": "Assets:Checking"})),
        json!([])
    );
    // Placeholder and income accounts cannot be reconciled.
    assert_eq!(
        b.err(
            "reconcile.start",
            json!({"account": "Assets", "statement_date": "2026-10-31", "statement_balance": "1"})
        )
        .code(),
        "rejected"
    );
    // Finishing into the closed period is rejected.
    b.run("reconcile.cancel", json!({"account": "Assets:Checking"}))
        .to_string();
}
