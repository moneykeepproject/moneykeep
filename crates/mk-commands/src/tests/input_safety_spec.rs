//! input-safety.md acceptance tests (1–3, 6, 7; 4 and 5 are in mk-import, 8 in
//! xtask, 9 in mk-store).

use base64::Engine;

use super::*;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn csv_book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.run(
        "csv.profile_save",
        json!({"profile": {"name": "Plain", "columns": {"date": "Date", "payee": "Description", "amount": "Amount"},
               "date_format": "YYYY-MM-DD", "account": "Assets:Checking"}}),
    );
    b
}

fn stage_csv(b: &mut Book, body: &str) -> Value {
    let file = format!("Date,Description,Amount\n{body}");
    b.run(
        "import.stage",
        json!({"format": "csv", "content_base64": b64(file.as_bytes()), "profile": "Plain"}),
    )
}

fn first_row(batch: &Value) -> Value {
    batch["rows"][0].clone()
}

#[test]
fn input_safety_at1_formula_payee_exported_as_text() {
    let mut b = csv_book();
    let batch = stage_csv(
        &mut b,
        "2026-10-01,\"=HYPERLINK(\"\"http://x\"\",\"\"click\"\")\",-12.50\n",
    );
    assert_eq!(
        first_row(&batch)["payee"],
        "=HYPERLINK(\"http://x\",\"click\")"
    );
    b.run("import.commit", json!({"batch": batch["id"]}));
    let out = b.run(
        "report.export",
        json!({"report": "payees", "args": {"from": "2026-10-01", "to": "2026-10-31"}}),
    );
    let content = out["content"].as_str().unwrap();
    assert!(
        content.contains("\"'=HYPERLINK(\"\"http://x\"\",\"\"click\"\")\""),
        "{content}"
    );
    let out = b.run(
        "report.export",
        json!({"report": "income_expense", "args": {"from": "2026-10-01", "to": "2026-10-31"}}),
    );
    let content = out["content"].as_str().unwrap();
    assert!(content.contains(",-12.50"), "{content}");
    assert!(!content.contains("'-12.50"), "{content}");
}

#[test]
fn input_safety_at2_escape_and_nul_removed_with_note() {
    let mut b = csv_book();
    let batch = stage_csv(&mut b, "2026-10-01,Shop\u{1b}[2J\u{0}One,-5.00\n");
    let r = first_row(&batch);
    assert_eq!(r["payee"], "Shop[2JOne");
    let notes = r["raw"]["cleaned"].to_string();
    assert!(
        notes.contains("control characters removed from payee"),
        "{r}"
    );
}

#[test]
fn input_safety_at3_bidi_override_removed_with_note() {
    let mut b = csv_book();
    let batch = stage_csv(&mut b, "2026-10-01,Coffee \u{202E}poT,-5.00\n");
    let r = first_row(&batch);
    assert_eq!(r["payee"], "Coffee poT");
    assert!(
        r["raw"]["cleaned"]
            .to_string()
            .contains("invisible characters removed from payee"),
        "{r}"
    );
}

#[test]
fn input_safety_r4_out_of_range_rows_rejected_with_note() {
    let mut b = csv_book();
    let batch = stage_csv(
        &mut b,
        "1850-01-01,Old,-1.00\n2026-10-01,Huge,-1234567890123456.00\n2026-10-02,Fine,-2.00\n",
    );
    assert_eq!(batch["rows"].as_array().unwrap().len(), 1, "{batch}");
    let w = batch["warnings"].to_string();
    assert!(
        w.contains("outside 1900") && w.contains("15 integer digits"),
        "{w}"
    );
}

#[test]
fn input_safety_at7_altered_proposal_aborts_commit() {
    let mut b = csv_book();
    let batch = stage_csv(&mut b, "2026-10-01,Shop,-5.00\n");
    let row_id = first_row(&batch)["id"].as_str().unwrap().to_owned();
    // Simulate tampering between review and commit, bypassing the review commands.
    b.s.write(|tx| -> mk_store::StoreResult<()> {
        let mut d = tx.get_doc(mk_store::DocTable::ImportRow, &row_id)?.unwrap();
        d.json["payee"] = json!("Someone else");
        tx.put_doc(mk_store::DocTable::ImportRow, &d)
    })
    .unwrap();
    let err = b
        .try_run("import.commit", json!({"batch": batch["id"]}))
        .unwrap_err();
    assert!(
        err.to_string().contains("changed after it was reviewed"),
        "{err}"
    );
    assert!(
        b.run("txn.list", json!({"account": "Assets:Checking"}))
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Review commands re-seal the proposal, after which commit proceeds.
    b.run(
        "import.set_row",
        json!({"batch": batch["id"], "row": row_id, "payee": "Shop"}),
    );
    b.run("import.commit", json!({"batch": batch["id"]}));
}
