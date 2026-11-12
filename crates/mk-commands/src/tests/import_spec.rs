//! import-pipeline.md, and command-level import-qif.md / import-csv.md acceptance tests.

use super::*;

pub(crate) fn fixture(path: &str) -> String {
    let p = format!(
        "{}/../../examples/fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn fixture_bytes(path: &str) -> Vec<u8> {
    let p = format!(
        "{}/../../examples/fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&p).unwrap()
}

fn classes(batch: &Value) -> Vec<String> {
    batch["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["class"].as_str().unwrap().to_owned())
        .collect()
}

fn checking_book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Savings", "type": "asset", "subtype": "bank", "parent": "Assets"}));
    b.account(json!({"name": "Utilities", "type": "expense", "parent": "Expenses"}));
    b
}

#[test]
fn import_pipeline_at1_staging_changes_no_ledger_data() {
    let mut b = checking_book();
    b.transfer(
        "2026-09-01",
        "Assets:Checking",
        "Opening Balances",
        "100.00",
    );
    let tb = b.run("report.trial_balance", json!({}));
    let n = b.txn_count();
    let batch = b.stage_qif(&fixture("qif/bank-three.qif"), "Assets:Checking");
    assert_eq!(batch["status"], "staged");
    assert_eq!(classes(&batch), ["new", "new", "new"]);
    assert_eq!(b.run("report.trial_balance", json!({})), tb);
    assert_eq!(b.txn_count(), n);
}

#[test]
fn import_pipeline_at2_restage_is_all_duplicates() {
    let mut b = checking_book();
    let batch = b.stage_qif(&fixture("qif/bank-three.qif"), "Assets:Checking");
    let out = b.commit(&batch);
    assert_eq!(out["created"], 3);
    let again = b.stage_qif(&fixture("qif/bank-three.qif"), "Assets:Checking");
    assert_eq!(classes(&again), ["duplicate", "duplicate", "duplicate"]);
    let n = b.txn_count();
    let out = b.commit(&again);
    assert_eq!(out["created"], 0);
    assert_eq!(b.txn_count(), n);
    // Overlapping file: overlap rows duplicate, the rest new.
    let overlap = "!Type:Bank\nD10/05/2026\nT-18.75\nPCorner Grocery\nC*\n^\nD10/09/2026\nT-3.00\nPKiosk\n^\n";
    assert_eq!(
        classes(&b.stage_qif(overlap, "Assets:Checking")),
        ["duplicate", "new"]
    );
}

#[test]
fn import_pipeline_at3_match_window() {
    for (window, expected) in [(3, "match"), (1, "new")] {
        let mut b = checking_book();
        b.run(
            "setting.set",
            json!({"module": "core", "key": "import.match_window_days", "value": window}),
        );
        let manual = b.run(
            "txn.create",
            json!({"date": "2026-10-03", "payee": "Water Co", "splits": [
            {"account": "Expenses:Utilities", "value": "42.10"},
            {"account": "Assets:Checking", "value": "-42.10"}]}),
        );
        let batch = b.stage_qif(
            "!Type:Bank\nD10/05/2026\nT-42.10\nPCITY WATER\n^\n",
            "Assets:Checking",
        );
        assert_eq!(classes(&batch), [expected], "window {window}");
        let n = b.txn_count();
        b.commit(&batch);
        if expected == "match" {
            assert_eq!(b.txn_count(), n, "a match creates no transaction");
            let t = b.run("txn.get", json!({"id": manual["id"]}));
            let s = t["splits"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["account_name"] == "Assets:Checking")
                .unwrap()
                .clone();
            assert_eq!(s["reconcile"], "c");
            assert!(s["fingerprint"].as_str().unwrap().starts_with("hash:"));
            // Re-staging now finds the duplicate.
            assert_eq!(
                classes(&b.stage_qif(
                    "!Type:Bank\nD10/05/2026\nT-42.10\nPCITY WATER\n^\n",
                    "Assets:Checking"
                )),
                ["duplicate"]
            );
        } else {
            assert_eq!(b.txn_count(), n + 1);
        }
    }
}

#[test]
fn import_pipeline_at4_identical_rows_are_two_transactions() {
    let mut b = checking_book();
    let file = "!Type:Bank\nD10/01/2026\nT-5.00\nPCoffee\n^\nD10/01/2026\nT-5.00\nPCoffee\n^\n";
    let batch = b.stage_qif(file, "Assets:Checking");
    assert_eq!(classes(&batch), ["new", "new"]);
    assert_eq!(b.commit(&batch)["created"], 2);
    assert_eq!(
        classes(&b.stage_qif(file, "Assets:Checking")),
        ["duplicate", "duplicate"]
    );
}

#[test]
fn import_pipeline_at5_uncategorized_accounts_created_on_first_use() {
    let mut b = checking_book();
    let batch = b.stage_qif("!Type:Bank\nD10/01/2026\nT-7.00\nPMystery Shop\n^\nD10/02/2026\nT20.00\nPMystery Refund\n^\n", "Assets:Checking");
    let rows = batch["rows"].as_array().unwrap();
    assert_eq!(rows[0]["other"][0]["account_name"], "Uncategorized Expense");
    assert_eq!(rows[1]["other"][0]["account_name"], "Uncategorized Income");
    assert_eq!(
        b.err("account.get", json!({"account": "Uncategorized Expense"}))
            .code(),
        "not_found"
    );
    b.commit(&batch);
    assert_eq!(
        b.run("account.get", json!({"account": "Uncategorized Expense"}))["type"],
        "expense"
    );
    assert_eq!(
        b.run("account.get", json!({"account": "Uncategorized Income"}))["type"],
        "income"
    );
    assert_eq!(b.shown("Uncategorized Expense", "USD"), "7.00");
    assert_eq!(b.shown("Uncategorized Income", "USD"), "20.00");
}

#[test]
fn import_pipeline_at6_transfer_across_two_batches_is_one_transaction() {
    let mut b = checking_book();
    let n = b.txn_count();
    let chk = b.stage_qif(
        "!Type:Bank\nD10/10/2026\nT-500.00\nPTransfer to savings\n^\n",
        "Assets:Checking",
    );
    let row = chk["rows"][0]["id"].clone();
    b.run(
        "import.set_row",
        json!({"batch": chk["id"], "row": row, "other_account": "Assets:Savings"}),
    );
    b.commit(&chk);
    let sav = b.stage_qif(
        "!Type:Bank\nD10/10/2026\nT500.00\nPTransfer from checking\n^\n",
        "Assets:Savings",
    );
    assert_eq!(classes(&sav), ["match"]);
    b.commit(&sav);
    assert_eq!(b.txn_count(), n + 1);
    assert_eq!(b.shown("Assets:Savings", "USD"), "500.00");
    // Both rows' fingerprints are on the one transaction, so both re-stage as duplicates.
    assert_eq!(
        classes(&b.stage_qif(
            "!Type:Bank\nD10/10/2026\nT500.00\nPTransfer from checking\n^\n",
            "Assets:Savings"
        )),
        ["duplicate"]
    );
}

#[test]
fn transfer_within_one_batch_and_reverse_order() {
    // Savings imported first (uncategorised), then checking with a hint: linked.
    let mut b = checking_book();
    let n = b.txn_count();
    let sav = b.stage_qif(
        "!Type:Bank\nD10/10/2026\nT500.00\nPFrom checking\n^\n",
        "Assets:Savings",
    );
    b.commit(&sav);
    let chk = b.stage_qif(
        "!Type:Bank\nD10/11/2026\nT-500.00\nPTo savings\nL[Savings]\n^\n",
        "Assets:Checking",
    );
    assert_eq!(classes(&chk), ["match"]);
    b.commit(&chk);
    assert_eq!(b.txn_count(), n + 1);
    assert_eq!(b.shown("Assets:Checking", "USD"), "-500.00");
    assert_eq!(b.shown("Uncategorized Income", "USD"), "0.00");
    // Two-account QIF file with both legs: one transaction.
    let mut c = checking_book();
    let file = "!Account\nNChecking\nTBank\n^\n!Type:Bank\nD10/1/2026\nT-50.00\nL[Savings]\n^\n!Account\nNSavings\nTBank\n^\n!Type:Bank\nD10/1/2026\nT50.00\nL[Checking]\n^\n";
    let batch = c.run("import.stage", json!({"format": "qif", "content": file}));
    c.run(
        "import.map_account",
        json!({"batch": batch["id"], "section": 0, "account": "Assets:Checking"}),
    );
    let batch = c.run(
        "import.map_account",
        json!({"batch": batch["id"], "section": 1, "account": "Assets:Savings"}),
    );
    assert_eq!(classes(&batch), ["new", "match"]);
    let n = c.txn_count();
    c.commit(&batch);
    assert_eq!(c.txn_count(), n + 1);
}

#[test]
fn import_pipeline_at7_unresolved_section_rejected() {
    let mut b = checking_book();
    let batch = b.run(
        "import.stage",
        json!({"format": "qif", "content": fixture("qif/bank-three.qif")}),
    );
    assert_eq!(classes(&batch), ["pending", "pending", "pending"]);
    let e = b.err("import.commit", json!({"batch": batch["id"]}));
    assert_eq!(e.code(), "rejected");
    assert!(e.to_string().contains("not resolved"), "{e}");
    let batch = b.run(
        "import.map_account",
        json!({"batch": batch["id"], "account": "Assets:Checking"}),
    );
    assert_eq!(classes(&batch), ["new", "new", "new"]);
    b.commit(&batch);
}

#[test]
fn import_pipeline_at8_commit_then_undo_restores_exactly() {
    let mut b = checking_book();
    b.run(
        "txn.create",
        json!({"date": "2026-10-03", "splits": [
        {"account": "Expenses:Utilities", "value": "42.10"},
        {"account": "Assets:Checking", "value": "-42.10"}]}),
    );
    let batch = b.stage_qif(&fixture("qif/bank-three.qif"), "Assets:Checking");
    assert_eq!(classes(&batch), ["new", "match", "new"]);
    let before = b.state();
    let batch_before = b.run("import.get", json!({"batch": batch["id"]}));
    b.commit(&batch);
    assert_ne!(b.state(), before);
    b.run("history.undo", json!({}));
    assert_eq!(
        b.state(),
        before,
        "matched split back to n, created transactions gone"
    );
    assert_eq!(
        b.run("import.get", json!({"batch": batch["id"]})),
        batch_before
    );
    // Undo is rejected once a created transaction was reconciled.
    b.run("history.redo", json!({}));
    let t = b.run("txn.list", json!({}))[0]["id"].clone();
    b.as_actor(
        "bob",
        "txn.set_reconcile",
        json!({"splits": [{"id": t, "split": 0, "state": "r"}]}),
    );
    assert_eq!(b.err("history.undo", json!({})).code(), "rejected");
}

#[test]
fn import_pipeline_at9_currency_mismatch_rejected() {
    let mut b = checking_book();
    let batch = b.run(
        "import.stage",
        json!({"format": "qif", "content": fixture("qif/bank-three.qif")}),
    );
    // Simulate a section in EUR (as an OFX file would carry) by staging through the
    // shared path with a currency-bearing statement.
    let _ = batch;
    let e =
        b.s.write(|tx| -> CommandResult<Value> {
            let a = Actor("t".into());
            let ctx = Ctx::new(tx, &a);
            let mut st = mk_import::QifParser::default()
                .parse_text(&fixture("qif/bank-three.qif"))
                .unwrap();
            st.sections[0].currency = Some(CommodityCode::new("EUR").unwrap());
            crate::import::stage_statement(
                &ctx,
                st,
                "qif",
                None,
                "x".into(),
                Some("Assets:Checking"),
                None,
            )
        })
        .unwrap_err();
    assert_eq!(e.code(), "rejected");
    assert!(e.to_string().contains("EUR"), "{e}");
}

#[test]
fn import_pipeline_misc_review_commands() {
    let mut b = checking_book();
    let batch = b.stage_qif(
        "!Type:Bank\nD10/01/2026\nT0.00\nPZero\n^\nD10/02/2026\nT-9.00\nPShop\n^\n",
        "Assets:Checking",
    );
    let rows = batch["rows"].as_array().unwrap().clone();
    assert_eq!(rows[0]["action"], "skip", "zero rows default to skip");
    // Override payee/memo/other and skip.
    let out = b.run("import.set_row", json!({"batch": batch["id"], "row": rows[1]["id"], "payee": "Better Name", "memo": "note", "other_account": "Expenses:Groceries"}));
    assert_eq!(out["rows"][1]["payee"], "Better Name");
    assert_eq!(
        out["rows"][1]["other"][0]["account_name"],
        "Expenses:Groceries"
    );
    assert_eq!(
        b.err(
            "import.set_row",
            json!({"batch": batch["id"], "row": rows[1]["id"], "other_account": "Assets:Checking"})
        )
        .code(),
        "rejected"
    );
    // Clearing the choice restores the automatic proposal.
    let cleared = b.run(
        "import.set_row",
        json!({"batch": batch["id"], "row": rows[1]["id"], "other_account": null}),
    );
    assert_eq!(
        cleared["rows"][1]["other"][0]["account_name"],
        "Uncategorized Expense"
    );
    assert_eq!(
        cleared["rows"][1]["payee"], "Better Name",
        "other overrides are kept"
    );
    b.run(
        "import.set_row",
        json!({"batch": batch["id"], "row": rows[1]["id"], "action": "skip"}),
    );
    assert_eq!(b.commit(&batch)["created"], 0);
    // Discard, list.
    let b2 = b.stage_qif(
        "!Type:Bank\nD10/03/2026\nT-1.00\nPX\n^\n",
        "Assets:Checking",
    );
    b.run("import.discard", json!({"batch": b2["id"]}));
    assert_eq!(
        b.err("import.commit", json!({"batch": b2["id"]})).code(),
        "rejected"
    );
    let list = b.run("import.list", json!({}));
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(
        b.run("import.list", json!({"status": "discarded"}))
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Audit records the file digest, not the content.
    let entry =
        b.s.audit_entries(100)
            .unwrap()
            .into_iter()
            .find(|e| e.command == "import.stage")
            .unwrap();
    assert!(entry.args.get("content").is_none());
    assert!(entry.args["sha256"].is_string());
}

#[test]
fn import_pipeline_manual_match_override_and_remembered_mapping() {
    let mut b = checking_book();
    let manual = b.run(
        "txn.create",
        json!({"date": "2026-09-01", "splits": [
        {"account": "Expenses:Utilities", "value": "42.10"},
        {"account": "Assets:Checking", "value": "-42.10"}]}),
    );
    let file = "!Account\nNMy Checking\nTBank\n^\n!Type:Bank\nD10/20/2026\nT-42.10\nPWater\n^\n";
    let batch = b.run("import.stage", json!({"format": "qif", "content": file}));
    let batch = b.run(
        "import.map_account",
        json!({"batch": batch["id"], "account": "Assets:Checking", "remember": true}),
    );
    assert_eq!(classes(&batch), ["new"], "outside the window");
    let row = batch["rows"][0]["id"].clone();
    let batch = b.run(
        "import.set_row",
        json!({"batch": batch["id"], "row": row, "action": "match", "match_txn": manual["id"]}),
    );
    assert_eq!(classes(&batch), ["match"]);
    let n = b.txn_count();
    b.commit(&batch);
    assert_eq!(b.txn_count(), n);
    // The mapping is remembered for the next file from the same account.
    let next = b.run("import.stage", json!({"format": "qif", "content": "!Account\nNMy Checking\nTBank\n^\n!Type:Bank\nD10/21/2026\nT-1.00\nPX\n^\n"}));
    assert_eq!(next["sections"][0]["resolved"], true);
}

// ----- QIF at command level -----------------------------------------------------

#[test]
fn import_qif_at4_transfer_resolves_to_savings() {
    let mut b = checking_book();
    let batch = b.stage_qif(
        "!Type:Bank\nD10/1/2026\nT-500.00\nPTransfer\nL[Savings]\n^\n",
        "Assets:Checking",
    );
    assert_eq!(batch["rows"][0]["category_hint"], "[Savings]");
    assert_eq!(
        batch["rows"][0]["other"][0]["account_name"],
        "Assets:Savings"
    );
    assert_eq!(batch["rows"][0]["other_source"], "hint");
    b.commit(&batch);
    assert_eq!(b.shown("Assets:Savings", "USD"), "500.00");
}

#[test]
fn import_qif_at5_split_transaction_and_needs_review() {
    let mut b = checking_book();
    b.account(json!({"name": "Household", "type": "expense"}));
    b.account(json!({"name": "Groceries", "type": "expense"}));
    let batch = b.stage_qif(&fixture("qif/splits.qif"), "Assets:Checking");
    let rows = batch["rows"].as_array().unwrap();
    assert_eq!(rows[0]["needs_review"], false);
    assert_eq!(rows[0]["other"].as_array().unwrap().len(), 2);
    assert_eq!(rows[1]["needs_review"], true);
    assert_eq!(rows[1]["other"][0]["account_name"], "Uncategorized Expense");
    b.commit(&batch);
    let t = &b.run("txn.list", json!({"account": "Household"}))[0];
    assert_eq!(t["splits"].as_array().unwrap().len(), 3);
}

#[test]
fn import_qif_opening_balance_maps_to_opening_balances() {
    let mut b = checking_book();
    let file = "!Account\nNChecking\nTBank\n^\n!Type:Bank\nD10/1/2026\nT1000.00\nPOpening Balance\nL[Checking]\n^\n";
    let batch = b.run(
        "import.stage",
        json!({"format": "qif", "content": file, "account": "Assets:Checking"}),
    );
    assert_eq!(
        batch["rows"][0]["other"][0]["account_name"],
        "Opening Balances"
    );
    b.commit(&batch);
    assert_eq!(b.shown("Opening Balances", "USD"), "1000.00");
}

#[test]
fn import_qif_at7_cp1252_via_base64() {
    use base64::Engine;
    let mut b = checking_book();
    let enc = base64::engine::general_purpose::STANDARD.encode(fixture_bytes("qif/cp1252.qif"));
    let batch = b.run(
        "import.stage",
        json!({"format": "qif", "content_base64": enc, "account": "Assets:Checking"}),
    );
    assert_eq!(batch["rows"][0]["payee"], "Café du Parc");
}

#[test]
fn import_qif_at6_cleared_rows_commit_as_cleared() {
    let mut b = checking_book();
    let batch = b.stage_qif(
        "!Type:Bank\nD10/1/2026\nT-1.00\nPA\nCX\n^\n",
        "Assets:Checking",
    );
    assert_eq!(batch["rows"][0]["cleared"], "reconciled");
    b.commit(&batch);
    let t = &b.run("txn.list", json!({}))[0];
    let s = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["account_name"] == "Assets:Checking")
        .unwrap()
        .clone();
    assert_eq!(s["reconcile"], "c");
}

#[test]
fn import_qif_at9_investment_skip_reported() {
    let mut b = checking_book();
    let batch = b.stage_qif(&fixture("qif/investment.qif"), "Assets:Checking");
    assert!(
        batch["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("skipped: investment records (2)"))
    );
}

// ----- CSV at command level -------------------------------------------------------

fn basic_profile(name: &str) -> Value {
    json!({"name": name, "columns": {"date": "Date", "payee": "Description", "amount": "Amount"},
           "date_format": "MM/DD/YYYY", "account": "Assets:Checking"})
}

#[test]
fn import_csv_at7_needs_profile_and_at8_auto_select() {
    let mut b = checking_book();
    let out = b.run(
        "import.stage",
        json!({"format": "csv", "content": fixture("csv/basic.csv")}),
    );
    let np = &out["needs_profile"];
    assert_eq!(np["header"], json!(["Date", "Description", "Amount"]));
    assert_eq!(np["suggested"]["columns"]["payee"], "Description");
    assert_eq!(np["suggested"]["date_format"], "MM/DD/YYYY");
    assert_eq!(b.run("import.list", json!({})), json!([]), "nothing staged");
    b.run(
        "csv.profile_save",
        json!({"profile": basic_profile("My Bank")}),
    );
    let batch = b.run(
        "import.stage",
        json!({"format": "csv", "content": fixture("csv/basic.csv")}),
    );
    assert_eq!(batch["sections"][0]["account_key"], "csv:My Bank");
    assert_eq!(batch["sections"][0]["resolved"], true);
    assert_eq!(batch["rows"][0]["payee"], "Smith, Jones & \"Co\"");
    assert_eq!(classes(&batch), ["new", "new", "new"]);
    // Two matching profiles: ambiguous, so needs_profile again.
    b.run(
        "csv.profile_save",
        json!({"profile": basic_profile("Other Bank")}),
    );
    let out = b.run(
        "import.stage",
        json!({"format": "csv", "content": fixture("csv/basic.csv")}),
    );
    assert!(out.get("needs_profile").is_some());
    let out = b.run(
        "import.stage",
        json!({"format": "csv", "content": fixture("csv/basic.csv"), "profile": "other bank"}),
    );
    assert_eq!(out["sections"][0]["account_key"], "csv:Other Bank");
}

#[test]
fn import_csv_at9_statement_captured() {
    let mut b = checking_book();
    b.run("csv.profile_save", json!({"profile": {"name": "FNB", "skip_rows": 3, "amount_mode": "split",
        "columns": {"date": "Date", "payee": "Description", "debit": "Debit", "credit": "Credit", "balance": "Balance"},
        "date_format": "YYYY-MM-DD"}}));
    let batch = b.run("import.stage", json!({"format": "csv", "content": fixture("csv/preamble-split-balance.csv"), "account": "Assets:Checking"}));
    let sid = batch["sections"][0]["statement"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(batch["sections"][0]["closing"]["amount"], "947.50");
    let s = b.run(
        "reconcile.start",
        json!({"account": "Assets:Checking", "statement": sid}),
    );
    assert_eq!(s["statement_balance"], "947.50");
    assert_eq!(s["statement_date"], "2026-10-05");
}

#[test]
fn import_csv_at10_profile_export_import_round_trip() {
    let mut a = checking_book();
    a.run("csv.profile_save", json!({"profile": {"name": "FNB", "delimiter": ";", "skip_rows": 2, "amount_mode": "type_column",
        "columns": {"date": 1, "amount": "Amt", "type": "Kind"}, "date_format": "DD.MM.YYYY", "negate": true,
        "decimal_comma": true, "debit_values": ["DR", "Debit"], "skip_if": [{"column": "Kind", "equals": "PENDING"}]}}));
    let exported = a.run("csv.profile_export", json!({"name": "fnb"}));
    let mut other = Book::new();
    other.run("csv.profile_import", json!({"document": exported}));
    assert_eq!(
        other.run("csv.profile_export", json!({"name": "FNB"})),
        exported
    );
    assert_eq!(
        other.run("csv.profile_list", json!({})),
        a.run("csv.profile_list", json!({}))
    );
    assert_eq!(
        other
            .err("csv.profile_import", json!({"document": exported}))
            .code(),
        "rejected"
    );
    other.run(
        "csv.profile_import",
        json!({"document": exported, "replace": true}),
    );
    other.run("csv.profile_delete", json!({"name": "FNB"}));
    assert_eq!(other.run("csv.profile_list", json!({})), json!([]));
    // Invalid profiles are refused.
    assert_eq!(other.err("csv.profile_save", json!({"profile": {"name": "X", "columns": {"amount": "A"}, "date_format": "YYYY-MM-DD"}})).code(), "invalid_args");
}

#[test]
fn import_limits_and_formats() {
    let mut b = checking_book();
    assert_eq!(
        b.err("import.stage", json!({"content": "x"})).code(),
        "invalid_args"
    );
    assert_eq!(
        b.err("import.stage", json!({"format": "ofx", "content": "x"}))
            .code(),
        "invalid_args"
    );
    assert_eq!(
        b.err(
            "import.stage",
            json!({"format": "qif", "content": "x", "content_base64": "eA=="})
        )
        .code(),
        "invalid_args"
    );
    let e = b.err(
        "import.stage",
        json!({"format": "qif", "path": "/tmp/x.qif"}),
    );
    assert!(e.to_string().contains("cannot read files"), "{e}");
    let big = "x".repeat(mk_import::MAX_FILE_BYTES + 1);
    assert!(
        b.err("import.stage", json!({"format": "qif", "content": big}))
            .to_string()
            .contains("50 MB")
    );
    // Format inferred from the name.
    let batch = b.run("import.stage", json!({"name": "oct.QIF", "content": fixture("qif/bank-three.qif"), "account": "Assets:Checking"}));
    assert_eq!(batch["format"], "qif");
}
