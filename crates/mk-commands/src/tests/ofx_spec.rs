//! import-ofx.md command-level acceptance tests and the `trntype` rule condition.

use base64::Engine;

use super::*;

fn ofx(path: &str) -> String {
    let p = format!(
        "{}/../../examples/fixtures/ofx/{path}",
        env!("CARGO_MANIFEST_DIR")
    );
    let bytes = std::fs::read(&p).unwrap_or_else(|e| panic!("{p}: {e}"));
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn stage(b: &mut Book, path: &str, account: Option<&str>) -> Value {
    let mut args = json!({"name": path, "content_base64": ofx(path)});
    if let Some(a) = account {
        args["account"] = json!(a);
    }
    b.run("import.stage", args)
}

fn classes(batch: &Value) -> Vec<String> {
    batch["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["class"].as_str().unwrap().to_owned())
        .collect()
}

fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Bank Fees", "type": "expense"}));
    b
}

#[test]
fn import_ofx_at1_staged_through_pipeline() {
    let mut b = book();
    let batch = stage(&mut b, "checking-v1.ofx", Some("Assets:Checking"));
    assert_eq!(batch["format"], "ofx");
    assert_eq!(classes(&batch), ["new", "new", "new"]);
    assert_eq!(batch["sections"][0]["currency"], "USD");
    assert_eq!(batch["sections"][0]["closing"]["amount"], "2439.15");
    assert!(
        batch["sections"][0]["statement"].is_string(),
        "ledger balance captured"
    );
    assert_eq!(
        batch["rows"][0]["fingerprint"].as_str().unwrap().get(..4),
        Some("ext:")
    );
}

#[test]
fn import_ofx_at3_credit_card_reconciles() {
    let mut b = book();
    let batch = stage(&mut b, "creditcard-v1.ofx", Some("Liabilities:Visa"));
    let rows = batch["rows"].as_array().unwrap();
    assert_eq!(rows[0]["amount"], "-45.10");
    assert_eq!(rows[1]["amount"], "200.00");
    b.commit(&batch);
    assert_eq!(b.shown("Liabilities:Visa", "USD"), "812.40", "812.40 owed");
    let sid = batch["sections"][0]["statement"].clone();
    let s = b.run(
        "reconcile.start",
        json!({"account": "Liabilities:Visa", "statement": sid}),
    );
    assert_eq!(s["statement_balance"], "812.40");
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
}

#[test]
fn import_ofx_at4_restage_all_duplicates() {
    let mut b = book();
    let batch = stage(&mut b, "checking-v1.ofx", Some("Assets:Checking"));
    b.commit(&batch);
    assert_eq!(
        classes(&stage(&mut b, "checking-v1.ofx", Some("Assets:Checking"))),
        ["duplicate"; 3]
    );
    // The XML version of the same statement is also all duplicates (same FITIDs).
    assert_eq!(
        classes(&stage(&mut b, "checking-v2.ofx", Some("Assets:Checking"))),
        ["duplicate"; 3]
    );
}

fn correction_setup(reconcile: bool) -> (Book, Value) {
    let mut b = book();
    b.account(json!({"name": "Hardware", "type": "expense"}));
    let batch = stage(&mut b, "correction-original.ofx", Some("Assets:Checking"));
    let row = batch["rows"][0]["id"].clone();
    b.run(
        "import.set_row",
        json!({"batch": batch["id"], "row": row, "other_account": "Hardware"}),
    );
    b.commit(&batch);
    let t = b.run("txn.list", json!({}))[0].clone();
    if reconcile {
        let i = t["splits"]
            .as_array()
            .unwrap()
            .iter()
            .position(|s| s["account_name"] == "Assets:Checking")
            .unwrap();
        b.run(
            "txn.set_reconcile",
            json!({"splits": [{"id": t["id"], "split": i, "state": "r"}]}),
        );
    }
    (b, t)
}

#[test]
fn import_ofx_at9_corrections() {
    // REPLACE on a two-split transaction updates both splits.
    let (mut b, t) = correction_setup(false);
    let batch = stage(&mut b, "correction-replace.ofx", Some("Assets:Checking"));
    assert_eq!(classes(&batch), ["correction"]);
    b.commit(&batch);
    let after = b.run("txn.get", json!({"id": t["id"]}));
    assert_eq!(after["date"], "2026-10-08");
    assert_eq!(after["payee"], "Hardware Store Inc");
    let by = |name: &str| {
        after["splits"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["account_name"] == name)
            .unwrap()
            .clone()
    };
    assert_eq!(by("Assets:Checking")["value"], "-35.00");
    assert_eq!(by("Hardware")["value"], "35.00");
    assert!(
        by("Assets:Checking")["fingerprint"]
            .as_str()
            .unwrap()
            .ends_with(":A1-FIX")
    );
    assert_eq!(b.txn_count(), 1);
    // Undo restores the original.
    b.run("history.undo", json!({}));
    assert_eq!(b.run("txn.get", json!({"id": t["id"]})), t);

    // DELETE voids it.
    let (mut b, t) = correction_setup(false);
    let batch = stage(&mut b, "correction-delete.ofx", Some("Assets:Checking"));
    assert_eq!(classes(&batch), ["correction_delete"]);
    b.commit(&batch);
    let after = b.run("txn.get", json!({"id": t["id"]}));
    assert_eq!(after["status"], "voided");
    assert_eq!(after["void_reason"], "correction from bank");

    // With a reconciled split both become needs_review and nothing changes.
    for file in ["correction-replace.ofx", "correction-delete.ofx"] {
        let (mut b, _) = correction_setup(true);
        let before = b.state();
        let batch = stage(&mut b, file, Some("Assets:Checking"));
        assert_eq!(batch["rows"][0]["needs_review"], true, "{file}");
        assert_eq!(batch["rows"][0]["action"], "skip", "{file}");
        b.commit(&batch);
        assert_eq!(b.state(), before, "{file}");
    }

    // A DELETE of a FITID never imported is ignored with a note.
    let mut b = book();
    let batch = stage(&mut b, "correction-delete.ofx", Some("Assets:Checking"));
    assert_eq!(batch["rows"][0]["action"], "skip");
    assert!(
        batch["rows"][0]["warnings"][0]
            .as_str()
            .unwrap()
            .contains("never imported")
    );
    // A REPLACE of an unknown FITID is a normal new row.
    let batch = stage(&mut b, "correction-replace.ofx", Some("Assets:Checking"));
    assert_eq!(classes(&batch), ["new"]);
}

#[test]
fn import_ofx_at10_status_error_reported() {
    let mut b = book();
    let batch = stage(&mut b, "status-error.ofx", Some("Assets:Checking"));
    assert_eq!(batch["rows"], json!([]));
    assert!(
        batch["warnings"][0]
            .as_str()
            .unwrap()
            .contains("account not found")
    );
}

#[test]
fn import_ofx_at11_full_account_number_never_stored() {
    let dir = std::env::temp_dir().join(format!("mk-ofx-{}", mk_core::Id::<()>::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("book.moneykeep");
    {
        let mut b = Book {
            r: Registry::with_builtins(),
            s: Store::create(&path, "T", &CommodityCode::new("USD").unwrap()).unwrap(),
            a: Actor("test".into()),
        };
        b.standard();
        let batch = b.run(
            "import.stage",
            json!({"name": "checking-v1.ofx", "content_base64": ofx("checking-v1.ofx")}),
        );
        let sec = &batch["sections"][0];
        assert_eq!(sec["account_hint"], "6789");
        assert!(
            sec["account_name"]
                .as_str()
                .unwrap()
                .ends_with("ending 6789")
        );
        let key = sec["account_key"].as_str().unwrap().to_owned();
        assert!(key.starts_with("ofx:") && key.len() == 4 + 64);
        let batch = b.run(
            "import.map_account",
            json!({"batch": batch["id"], "account": "Assets:Checking", "remember": true}),
        );
        b.commit(&batch);
        let text = batch.to_string();
        assert!(!text.contains("000123456789") && !text.contains("123456789"));
    }
    // Scan every byte of the book (and its WAL) for the account number.
    for entry in std::fs::read_dir(&dir).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        for needle in [&b"000123456789"[..], b"123456789", b"121000358"] {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle),
                "{:?} found",
                String::from_utf8_lossy(needle)
            );
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn import_ofx_at12_qfx_by_extension() {
    let mut b = book();
    let batch = stage(&mut b, "web-connect.qfx", Some("Assets:Checking"));
    assert_eq!(batch["format"], "ofx");
    assert_eq!(classes(&batch), ["new", "new", "new"]);
}

#[test]
fn import_ofx_at13_xxe_rejected() {
    let mut b = book();
    let e = b.err("import.stage", json!({"name": "doctype-xxe.ofx", "content_base64": ofx("doctype-xxe.ofx"), "account": "Assets:Checking"}));
    assert_eq!(e.code(), "invalid_args");
    assert!(e.to_string().contains("DOCTYPE"), "{e}");
}

#[test]
fn import_ofx_at14_foreign_row_needs_review_and_blocks_auto_commit() {
    let mut b = book();
    b.run(
        "rule.create",
        json!({"name": "Fees", "conditions": [{"field": "trntype", "op": "equals", "value": "FEE"}],
        "actions": [{"action": "set_account", "account": "Bank Fees"}]}),
    );
    let batch = stage(&mut b, "foreign-and-fee.ofx", Some("Assets:Checking"));
    let rows = batch["rows"].as_array().unwrap();
    assert_eq!(rows[0]["needs_review"], true);
    assert!(
        rows[0]["warnings"][0]
            .as_str()
            .unwrap()
            .contains("foreign-currency transaction")
    );
    assert!(!crate::auto_committable(&batch));
    // Without the foreign row the batch would be auto-committable (the fee is rule-categorised).
    assert_eq!(rows[1]["other_source"], "rule");
}

#[test]
fn import_ofx_at15_and_rules_trntype_condition() {
    let mut b = book();
    let r = b.run(
        "rule.create",
        json!({"name": "Fees", "conditions": [{"field": "trntype", "op": "equals", "value": "fee"}],
        "actions": [{"action": "set_account", "account": "Bank Fees"}]}),
    );
    let batch = stage(&mut b, "foreign-and-fee.ofx", Some("Assets:Checking"));
    let fee = &batch["rows"][1];
    assert_eq!(fee["other"][0]["account_name"], "Bank Fees");
    assert_eq!(fee["rule_ids"], json!([r["id"]]));
    // Rows from other formats have no trntype and never match.
    let qif = b.stage_qif(
        "!Type:Bank\nD10/01/2026\nT-2.50\nPMonthly service fee\n^\n",
        "Assets:Checking",
    );
    assert_eq!(
        qif["rows"][0]["other"][0]["account_name"],
        "Uncategorized Expense"
    );
    let t = b.run(
        "rule.test",
        json!({"account": "Assets:Checking", "amount": "-1", "trntype": "FEE"}),
    );
    assert_eq!(t["rule_ids"], json!([r["id"]]));
    assert_eq!(
        b.run(
            "rule.test",
            json!({"account": "Assets:Checking", "amount": "-1"})
        )["rule_ids"],
        json!([])
    );
    // trntype only supports equals.
    assert_eq!(b.err("rule.create", json!({"name": "X", "conditions": [{"field": "trntype", "op": "contains", "value": "FE"}],
        "actions": [{"action": "skip"}]})).code(), "invalid_args");
}

#[test]
fn import_ofx_r2_memo_equal_to_name_left_empty() {
    let mut b = book();
    let body = "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\n\n<OFX><SIGNONMSGSRSV1><SONRS><STATUS><CODE>0<SEVERITY>INFO</STATUS><DTSERVER>20261020<LANGUAGE>ENG</SONRS></SIGNONMSGSRSV1><CREDITCARDMSGSRSV1><CCSTMTTRNRS><TRNUID>1<STATUS><CODE>0<SEVERITY>INFO</STATUS><CCSTMTRS><CURDEF>USD<CCACCTFROM><ACCTID>4111000011112222</CCACCTFROM><BANKTRANLIST><DTSTART>20261001<DTEND>20261020<STMTTRN><TRNTYPE>DEBIT<DTPOSTED>20261002<TRNAMT>-35.94<FITID>CC-1<NAME>ALDI INC<MEMO>ALDI INC</STMTTRN><STMTTRN><TRNTYPE>DEBIT<DTPOSTED>20261003<TRNAMT>-12.00<FITID>CC-2<NAME>ALDI INC<MEMO> aldi inc </STMTTRN><STMTTRN><TRNTYPE>DEBIT<DTPOSTED>20261004<TRNAMT>-8.00<FITID>CC-3<NAME>ALDI INC<MEMO>Store 42</STMTTRN></BANKTRANLIST><LEDGERBAL><BALAMT>-55.94<DTASOF>20261020</LEDGERBAL></CCSTMTRS></CCSTMTTRNRS></CREDITCARDMSGSRSV1></OFX>";
    let content = base64::engine::general_purpose::STANDARD.encode(body);
    let batch = b.run(
        "import.stage",
        json!({"name": "card.ofx", "content_base64": content, "account": "Liabilities:Visa"}),
    );
    let rows = batch["rows"].as_array().unwrap();
    let by_fitid = |id: &str| {
        rows.iter()
            .find(|r| {
                r["fingerprint"]
                    .as_str()
                    .unwrap()
                    .ends_with(&format!(":{id}"))
            })
            .unwrap_or_else(|| panic!("row {id} in {batch}"))
    };
    for id in ["CC-1", "CC-2"] {
        assert_eq!(by_fitid(id)["payee"], "ALDI INC");
        assert_eq!(by_fitid(id)["memo"], "");
    }
    assert_eq!(by_fitid("CC-3")["payee"], "ALDI INC");
    assert_eq!(by_fitid("CC-3")["memo"], "Store 42");
}

// ----- import-pipeline.md revision 2: date corrections ------------------------------

/// The credit-card fixture with `DTUSER` two days before every `DTPOSTED`.
fn card_with_dtuser() -> String {
    let p = format!(
        "{}/../../examples/fixtures/ofx/creditcard-v1.ofx",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(p).unwrap();
    [
        ("20261004", "20261002"),
        ("20261015", "20261013"),
        ("20261020", "20261018"),
    ]
    .iter()
    .fold(text, |t, (posted, user)| {
        t.replace(
            &format!("<DTPOSTED>{posted}</DTPOSTED>"),
            &format!("<DTPOSTED>{posted}</DTPOSTED><DTUSER>{user}</DTUSER>"),
        )
    })
}

fn stage_dtuser(b: &mut Book) -> Value {
    b.run(
        "import.stage",
        json!({"format": "ofx", "content": card_with_dtuser(), "account": "Liabilities:Visa"}),
    )
}

fn visa_dates(b: &mut Book) -> Vec<String> {
    let mut d: Vec<String> = b
        .run("txn.list", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["date"].as_str().unwrap().to_owned())
        .collect();
    d.sort();
    d
}

fn txn_with_date(b: &mut Book, date: &str) -> Value {
    b.run("txn.list", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["date"] == date)
        .unwrap()
        .clone()
}

#[test]
fn import_pipeline_rev2_at13_restage_with_dtuser_corrects_dates() {
    let mut b = book();
    let first = stage(&mut b, "creditcard-v1.ofx", Some("Liabilities:Visa"));
    b.commit(&first);
    assert_eq!(
        visa_dates(&mut b),
        ["2026-10-04", "2026-10-15", "2026-10-20"]
    );
    let again = stage_dtuser(&mut b);
    assert_eq!(
        classes(&again),
        ["date_correction", "date_correction", "date_correction"]
    );
    assert_eq!(again["rows"][2]["date"], "2026-10-18");
    assert_eq!(again["rows"][2]["date_from"], "2026-10-20");
    assert_eq!(again["counts"]["date_correction"], 3);
    let before = b.state();
    let n = b.txn_count();
    let out = b.commit(&again);
    assert_eq!(out["date_corrected"], 3);
    assert_eq!(out["created"], 0);
    assert_eq!(b.txn_count(), n, "no new transaction");
    assert_eq!(
        visa_dates(&mut b),
        ["2026-10-02", "2026-10-13", "2026-10-18"]
    );
    // One audited update per transaction, inside the commit's entry.
    let entry = b.s.audit_entries(1).unwrap().into_iter().next().unwrap();
    assert_eq!(entry.command, "import.commit");
    let txns: Vec<&Value> = entry
        .after
        .as_ref()
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "transaction")
        .collect();
    assert_eq!(txns.len(), 3);
    // A further re-stage is all duplicates: the corrected date is the imported date.
    let third = stage_dtuser(&mut b);
    assert_eq!(classes(&third), ["duplicate", "duplicate", "duplicate"]);
    b.run("import.discard", json!({"batch": third["id"]}));
    // Undo of the commit restores the dates (the discard is undone first).
    b.run("history.undo", json!({}));
    b.run("history.undo", json!({}));
    b.run("history.undo", json!({}));
    assert_eq!(b.state(), before);
    assert_eq!(
        visa_dates(&mut b),
        ["2026-10-04", "2026-10-15", "2026-10-20"]
    );
}

#[test]
fn import_pipeline_rev2_at14_edited_and_reconciled_stay_duplicates() {
    let mut b = book();
    let first = stage(&mut b, "creditcard-v1.ofx", Some("Liabilities:Visa"));
    b.commit(&first);
    // The user moves the first transaction's date.
    let t = txn_with_date(&mut b, "2026-10-04");
    let splits: Vec<Value> = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| json!({"account": s["account"], "value": s["value"]}))
        .collect();
    b.run(
        "txn.update",
        json!({"id": t["id"], "date": "2026-10-05", "payee": t["payee"], "splits": splits}),
    );
    // The second has a reconciled split.
    let t = txn_with_date(&mut b, "2026-10-15");
    let idx = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .position(|s| s["account_name"] == "Liabilities:Visa")
        .unwrap();
    b.run(
        "txn.set_reconcile",
        json!({"splits": [{"id": t["id"], "split": idx, "state": "r"}]}),
    );
    let again = stage_dtuser(&mut b);
    assert_eq!(
        classes(&again),
        ["duplicate", "duplicate", "date_correction"]
    );
    assert_eq!(
        again["rows"][0]["warnings"][0],
        "Date differs from the bank (2026-10-02); left unchanged because it was edited"
    );
    assert_eq!(
        again["rows"][1]["warnings"][0],
        "Date differs from the bank (2026-10-13); left unchanged because it is reconciled"
    );
    // A closed period keeps the third one too.
    b.run("book.set_closing_date", json!({"date": "2026-10-18"}));
    let closed = stage_dtuser(&mut b);
    assert_eq!(classes(&closed), ["duplicate", "duplicate", "duplicate"]);
    assert_eq!(
        closed["rows"][2]["warnings"][0],
        "Date differs from the bank (2026-10-18); left unchanged because the period is closed"
    );
}

#[test]
fn import_pipeline_rev2_at15_posted_rows_stay_plain_duplicates() {
    let mut b = book();
    let first = stage(&mut b, "creditcard-v1.ofx", Some("Liabilities:Visa"));
    b.commit(&first);
    // With import.date_source = posted the rows are read with DTPOSTED (date_kind
    // posted); a different posting date is still a plain duplicate.
    b.run(
        "setting.set",
        json!({"module": "core", "key": "import.date_source", "value": "posted"}),
    );
    let moved = card_with_dtuser().replace("<DTPOSTED>20261020", "<DTPOSTED>20261021");
    let again = b.run(
        "import.stage",
        json!({"format": "ofx", "content": moved, "account": "Liabilities:Visa"}),
    );
    assert_eq!(classes(&again), ["duplicate", "duplicate", "duplicate"]);
    assert_eq!(again["rows"][2]["raw"]["date_kind"], "posted");
    assert_eq!(again["rows"][2]["date"], "2026-10-21");
    assert!(again["rows"][2]["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn import_ofx_rev3_date_source_setting_reaches_the_reader() {
    let mut b = book();
    let batch = stage_dtuser(&mut b);
    assert_eq!(batch["rows"][0]["date"], "2026-10-02");
    assert_eq!(batch["rows"][0]["raw"]["date_kind"], "transaction");
    b.run("import.discard", json!({"batch": batch["id"]}));
    b.run(
        "setting.set",
        json!({"module": "core", "key": "import.date_source", "value": "posted"}),
    );
    let batch = stage_dtuser(&mut b);
    assert_eq!(batch["rows"][0]["date"], "2026-10-04");
}
