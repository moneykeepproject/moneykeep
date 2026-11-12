//! import-pipeline.md "Proposed transaction for a new row": reconcile flags of
//! created splits (spec revision b4909c9).

use base64::Engine;

use super::*;

fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b
}

fn ofx_b64(path: &str) -> String {
    let p = format!(
        "{}/../../examples/fixtures/ofx/{path}",
        env!("CARGO_MANIFEST_DIR")
    );
    base64::engine::general_purpose::STANDARD.encode(std::fs::read(p).unwrap())
}

/// Reconcile flags of every created transaction: (target split, other splits).
fn flags(b: &mut Book, target: &str) -> Vec<(String, Vec<String>)> {
    let txns = b.run("txn.list", json!({}));
    txns.as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let splits = t["splits"].as_array().unwrap();
            let tgt = splits.iter().find(|s| s["account_name"] == target).unwrap();
            let others = splits
                .iter()
                .filter(|s| s["account_name"] != target)
                .map(|s| s["reconcile"].as_str().unwrap().to_owned())
                .collect();
            (tgt["reconcile"].as_str().unwrap().to_owned(), others)
        })
        .collect()
}

fn commit_qif(b: &mut Book, c_field: Option<&str>) -> Vec<(String, Vec<String>)> {
    let c = c_field.map(|v| format!("C{v}\n")).unwrap_or_default();
    let batch = b.stage_qif(
        &format!("!Type:Bank\nD10/01/2026\nT-5.00\nPShop\n{c}^\n"),
        "Assets:Checking",
    );
    b.commit(&batch);
    flags(b, "Assets:Checking")
}

#[test]
fn import_cleared_ofx_rows_start_cleared() {
    let mut b = book();
    let batch = b.run("import.stage", json!({"name": "checking-v1.ofx", "content_base64": ofx_b64("checking-v1.ofx"), "account": "Assets:Checking"}));
    b.commit(&batch);
    let f = flags(&mut b, "Assets:Checking");
    assert_eq!(f.len(), 3);
    for (target, others) in f {
        assert_eq!(target, "c");
        assert_eq!(others, ["n"], "other side starts n");
    }
}

#[test]
fn import_cleared_csv_rows_start_cleared() {
    let mut b = book();
    b.run("csv.profile_save", json!({"profile": {"name": "P", "columns": {"date": "Date", "payee": "Description", "amount": "Amount"},
        "date_format": "MM/DD/YYYY", "account": "Assets:Checking"}}));
    let batch = b.run("import.stage", json!({"format": "csv", "content": "Date,Description,Amount\n10/05/2026,Shop,-12.40\n10/06/2026,Refund,3.00\n"}));
    b.commit(&batch);
    for (target, others) in flags(&mut b, "Assets:Checking") {
        assert_eq!(target, "c");
        assert_eq!(others, ["n"]);
    }
}

#[test]
fn import_cleared_qif_blank_is_new() {
    let mut b = book();
    assert_eq!(
        commit_qif(&mut b, None),
        [("n".to_owned(), vec!["n".to_owned()])]
    );
}

#[test]
fn import_cleared_qif_star_is_cleared() {
    let mut b = book();
    assert_eq!(
        commit_qif(&mut b, Some("*")),
        [("c".to_owned(), vec!["n".to_owned()])]
    );
}

#[test]
fn import_cleared_qif_reconciled_is_cleared_not_reconciled() {
    for v in ["X", "R"] {
        let mut b = book();
        assert_eq!(
            commit_qif(&mut b, Some(v)),
            [("c".to_owned(), vec!["n".to_owned()])],
            "C{v}"
        );
    }
}

#[test]
fn import_cleared_split_rows_other_sides_new() {
    let mut b = book();
    b.account(json!({"name": "Household", "type": "expense"}));
    let batch = b.stage_qif("!Type:Bank\nD10/02/2026\nT-100.00\nPMegaMart\nC*\nSExpenses:Groceries\n$-60.00\nSHousehold\n$-40.00\n^\n", "Assets:Checking");
    b.commit(&batch);
    assert_eq!(
        flags(&mut b, "Assets:Checking"),
        [("c".to_owned(), vec!["n".to_owned(), "n".to_owned()])]
    );
}

#[test]
fn import_cleared_transfer_link_target_split_cleared() {
    // A QIF savings row (blank C) linked into an earlier OFX-imported checking
    // transfer: the savings split follows the QIF rule (n), the checking split stays c.
    let mut b = book();
    b.account(json!({"name": "Savings", "type": "asset", "subtype": "bank", "parent": "Assets"}));
    let csv_profile = json!({"profile": {"name": "P", "columns": {"date": "Date", "payee": "Description", "amount": "Amount"},
        "date_format": "MM/DD/YYYY", "account": "Assets:Savings"}});
    b.run("csv.profile_save", csv_profile);
    let sav = b.run("import.stage", json!({"format": "csv", "content": "Date,Description,Amount\n10/10/2026,From checking,500.00\n"}));
    b.commit(&sav);
    let chk = b.stage_qif(
        "!Type:Bank\nD10/10/2026\nT-500.00\nPTo savings\nL[Savings]\n^\n",
        "Assets:Checking",
    );
    assert_eq!(chk["rows"][0]["class"], "match");
    b.commit(&chk);
    let t = &b.run("txn.list", json!({}))[0];
    let by = |name: &str| {
        t["splits"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["account_name"] == name)
            .unwrap()["reconcile"]
            .clone()
    };
    assert_eq!(by("Assets:Savings"), "c", "CSV row");
    assert_eq!(by("Assets:Checking"), "n", "QIF row with blank C");
}

#[test]
fn import_cleared_undo_restores_flags_exactly() {
    let mut b = book();
    let before = b.state();
    let batch = b.run("import.stage", json!({"name": "checking-v1.ofx", "content_base64": ofx_b64("checking-v1.ofx"), "account": "Assets:Checking"}));
    let staged = b.state();
    b.commit(&batch);
    b.run("history.undo", json!({}));
    assert_eq!(b.state(), staged);
    b.run("history.undo", json!({}));
    assert_eq!(b.state(), before);
}

#[test]
fn import_cleared_reconciliation_preselects_imported_ofx_rows() {
    // AT1-style: opening balance reconciled, then an OFX download; every imported row
    // is a pre-selected candidate and the captured statement reconciles at once.
    let mut b = book();
    let open = b.transfer(
        "2026-09-30",
        "Assets:Checking",
        "Opening Balances",
        "1000.00",
    );
    let t = b.run("txn.get", json!({"id": open}));
    let i = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .position(|s| s["account_name"] == "Assets:Checking")
        .unwrap();
    b.run(
        "txn.set_reconcile",
        json!({"splits": [{"id": open, "split": i, "state": "r"}]}),
    );
    let batch = b.run("import.stage", json!({"name": "checking-v1.ofx", "content_base64": ofx_b64("checking-v1.ofx"), "account": "Assets:Checking"}));
    b.commit(&batch);
    // 1000.00 + 1500.00 − 42.10 − 18.75 = 2439.15, the file's ledger balance.
    let s = b.run(
        "reconcile.start",
        json!({"account": "Assets:Checking", "statement": batch["sections"][0]["statement"]}),
    );
    assert_eq!(s["opening_balance"], "1000.00");
    assert_eq!(s["statement_balance"], "2439.15");
    let cands = s["candidates"].as_array().unwrap();
    assert_eq!(cands.len(), 3);
    assert!(
        cands
            .iter()
            .all(|c| c["selected"] == true && c["reconcile"] == "c"),
        "{cands:?}"
    );
    assert_eq!(s["difference"], "0.00");
    b.run("reconcile.finish", json!({"account": "Assets:Checking"}));
    assert_eq!(
        flags(&mut b, "Assets:Checking")
            .iter()
            .map(|f| f.0.as_str())
            .collect::<Vec<_>>(),
        ["r", "r", "r", "r"]
    );
    let bal = b.run(
        "account.balance",
        json!({"account": "Assets:Checking", "basis": "reconciled"}),
    );
    assert_eq!(bal["display"]["USD"], "2439.15");
}
