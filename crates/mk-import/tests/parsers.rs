//! Parser acceptance tests for import-qif.md and import-csv.md (parser-level parts).
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use std::collections::BTreeMap;

use mk_import::{
    AmountMode, ClearedFlag, ColumnRef, CsvParser, CsvProfile, DateFormat, DateOrder, Parser,
    QifOptions, QifParser, Statement, parse_records, suggest_profile, suggest_profile_with,
};

fn fixture(path: &str) -> Vec<u8> {
    let p = format!(
        "{}/../../examples/fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn qif(bytes: &[u8]) -> Statement {
    QifParser::default().parse(bytes).unwrap()
}

fn qif_with(bytes: &[u8], order: DateOrder, comma: bool) -> Result<Statement, String> {
    QifParser {
        options: QifOptions {
            date_order: order,
            decimal_comma: comma,
        },
    }
    .parse(bytes)
    .map_err(|e| e.to_string())
}

fn d(s: &str) -> mk_core::Decimal {
    s.parse().unwrap()
}

// ----- QIF -------------------------------------------------------------------

#[test]
fn import_qif_at1_bank_file_three_records() {
    let st = qif(&fixture("qif/bank-three.qif"));
    let rows = &st.sections[0].rows;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].date.to_string(), "2026-10-01");
    assert_eq!(rows[0].amount, d("1500.00"));
    assert_eq!(rows[0].payee, "Employer Payroll");
    assert_eq!(rows[1].amount, d("-42.10"));
    assert_eq!(rows[1].number, "1042");
    assert_eq!(rows[1].category_hint, "Utilities");
    assert_eq!(rows[2].payee, "Corner Grocery");
    assert_eq!(rows[2].amount, d("-18.75"));
    assert!(st.sections[0].account_key.is_none());
}

#[test]
fn import_qif_at2_dates_and_order() {
    let st = qif(b"!Type:Bank\nD6/ 1'26\nT-1.00\n^\n");
    assert_eq!(st.sections[0].rows[0].date.to_string(), "2026-06-01");
    let err = qif_with(&fixture("qif/dmy.qif"), DateOrder::Mdy, false).unwrap_err();
    assert!(err.contains("dmy"), "{err}");
    let st = qif_with(&fixture("qif/dmy.qif"), DateOrder::Dmy, false).unwrap();
    assert_eq!(st.sections[0].rows[0].date.to_string(), "2026-12-25");
}

#[test]
fn import_qif_at3_amount_styles() {
    let st = qif(b"!Type:Bank\nD10/1/2026\nT-1,234.56\n^\n");
    assert_eq!(st.sections[0].rows[0].amount, d("-1234.56"));
    let st = qif_with(
        b"!Type:Bank\nD10/1/2026\nT-1.234,56\n^\n",
        DateOrder::Mdy,
        true,
    )
    .unwrap();
    assert_eq!(st.sections[0].rows[0].amount, d("-1234.56"));
    let st = qif(b"!Type:Bank\nD10/1/2026\nU-3.00\n^\n");
    assert_eq!(
        st.sections[0].rows[0].amount,
        d("-3.00"),
        "U used when T is absent"
    );
}

#[test]
fn import_qif_at4_transfer_hint() {
    let st = qif(b"!Type:Bank\nD10/1/2026\nT-500.00\nPTransfer\nL[Savings]\n^\nD10/2/2026\nT-5\nLDining/Personal\n^\n");
    assert_eq!(st.sections[0].rows[0].category_hint, "[Savings]");
    assert_eq!(
        st.sections[0].rows[1].category_hint, "Dining",
        "class part dropped"
    );
}

#[test]
fn import_qif_at5_splits_parsed() {
    let st = qif(&fixture("qif/splits.qif"));
    let rows = &st.sections[0].rows;
    assert_eq!(rows[0].splits.len(), 2);
    assert_eq!(rows[0].splits[0].category_hint, "Groceries");
    assert_eq!(rows[0].splits[0].amount, d("-60.00"));
    assert_eq!(rows[0].splits[0].memo, "Food");
    let sum: mk_core::Decimal = rows[1].splits.iter().map(|s| s.amount).sum();
    assert_eq!(sum, d("-90.00"));
    // A split without $ is an error.
    assert!(
        QifParser::default()
            .parse(b"!Type:Bank\nD10/1/2026\nT-1\nSFood\n^\n")
            .is_err()
    );
}

#[test]
fn import_qif_at6_cleared_flags() {
    let st = qif(b"!Type:Bank\nD10/1/2026\nT-1\nCX\n^\nD10/1/2026\nT-1\nCR\n^\nD10/1/2026\nT-1\nC*\n^\nD10/1/2026\nT-1\nCc\n^\nD10/1/2026\nT-1\n^\n");
    let flags: Vec<Option<ClearedFlag>> = st.sections[0].rows.iter().map(|r| r.cleared).collect();
    assert_eq!(
        flags,
        [
            Some(ClearedFlag::Reconciled),
            Some(ClearedFlag::Reconciled),
            Some(ClearedFlag::Cleared),
            Some(ClearedFlag::Cleared),
            None
        ]
    );
}

#[test]
fn import_qif_at7_encodings() {
    assert_eq!(
        qif(&fixture("qif/cp1252.qif")).sections[0].rows[0].payee,
        "Café du Parc"
    );
    assert_eq!(
        qif(&fixture("qif/utf8-bom.qif")).sections[0].rows[0].payee,
        "Café du Parc"
    );
}

#[test]
fn import_qif_at8_account_blocks_make_sections() {
    let st = qif(&fixture("qif/two-accounts.qif"));
    assert_eq!(st.sections.len(), 2);
    assert_eq!(st.sections[0].account_key.as_deref(), Some("qif:Checking"));
    assert_eq!(st.sections[0].rows.len(), 1);
    assert_eq!(st.sections[1].account_key.as_deref(), Some("qif:Visa"));
    assert_eq!(st.sections[1].rows.len(), 2);
}

#[test]
fn import_qif_at9_investment_records_skipped() {
    let st = qif(&fixture("qif/investment.qif"));
    assert_eq!(st.row_count(), 1);
    assert!(
        st.warnings
            .iter()
            .any(|w| w == "skipped: investment records (2)"),
        "{:?}",
        st.warnings
    );
}

#[test]
fn import_qif_at10_missing_final_caret() {
    let st = qif(&fixture("qif/no-final-caret.qif"));
    assert_eq!(st.row_count(), 2);
    assert!(
        st.warnings.iter().any(|w| w.contains("no closing ^")),
        "{:?}",
        st.warnings
    );
}

#[test]
fn qif_unknown_header_and_category_only_file() {
    let err = QifParser::default()
        .parse(b"!Type:Bogus\n")
        .unwrap_err()
        .to_string();
    assert!(err.contains("line 1"), "{err}");
    let st = qif(&fixture("qif/categories-only.qif"));
    assert_eq!(st.row_count(), 0);
    assert!(st.warnings.iter().any(|w| w.contains("no transactions")));
    let st = qif(b"!Option:AutoSwitch\n!Type:CCard\nD10/1/2026\nT-9.99\nPStore\n^\n");
    assert_eq!(st.sections[0].rows[0].amount, d("-9.99"));
}

// ----- CSV -------------------------------------------------------------------

fn profile(columns: &[(&str, ColumnRef)], fmt: DateFormat) -> CsvProfile {
    let mut c = BTreeMap::new();
    for (k, v) in columns {
        c.insert((*k).to_owned(), v.clone());
    }
    CsvProfile {
        name: "Test".into(),
        delimiter: ",".into(),
        skip_rows: 0,
        has_header: true,
        columns: c,
        date_format: fmt,
        amount_mode: AmountMode::Signed,
        negate: false,
        decimal_comma: false,
        debit_values: vec![],
        account: None,
        skip_if: vec![],
        date_kind: None,
    }
}

fn name(s: &str) -> ColumnRef {
    ColumnRef::Name(s.into())
}

fn basic_profile() -> CsvProfile {
    profile(
        &[
            ("date", name("Date")),
            ("payee", name("Description")),
            ("amount", name("Amount")),
        ],
        DateFormat::MmDdYyyy,
    )
}

fn csv(p: &CsvProfile, bytes: &[u8]) -> Result<Statement, String> {
    CsvParser { profile: p.clone() }
        .parse(bytes)
        .map_err(|e| e.to_string())
}

#[test]
fn import_csv_at1_rfc4180_quoting() {
    let st = csv(&basic_profile(), &fixture("csv/basic.csv")).unwrap();
    let rows = &st.sections[0].rows;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].payee, "Smith, Jones & \"Co\"");
    assert_eq!(rows[2].payee, "Refund\nwith note");
    let recs = parse_records("a,\"b\nc\",d\n", ',').unwrap();
    assert_eq!(recs.len(), 1);
}

#[test]
fn import_csv_at2_signed_negate() {
    let mut p = basic_profile();
    p.negate = true;
    let st = csv(&p, b"Date,Description,Amount\n10/05/2026,Shop,12.50\n").unwrap();
    assert_eq!(st.sections[0].rows[0].amount, d("-12.50"));
}

fn split_profile() -> CsvProfile {
    let mut p = profile(
        &[
            ("date", name("Date")),
            ("payee", name("Description")),
            ("debit", name("Debit")),
            ("credit", name("Credit")),
            ("balance", name("Balance")),
        ],
        DateFormat::Iso,
    );
    p.amount_mode = AmountMode::Split;
    p.skip_rows = 3;
    p
}

#[test]
fn import_csv_at3_split_mode() {
    let st = csv(&split_profile(), &fixture("csv/preamble-split-balance.csv")).unwrap();
    assert_eq!(st.sections[0].rows[1].amount, d("-40.00"));
    assert_eq!(st.sections[0].rows[0].amount, d("1000.00"));
    let mut p = split_profile();
    p.skip_rows = 0;
    let err = csv(
        &p,
        b"Date,Description,Debit,Credit,Balance\n2026-10-01,X,1.00,2.00,\n",
    )
    .unwrap_err();
    assert!(err.contains("line 2") && err.contains("both"), "{err}");
}

#[test]
fn import_csv_at4_type_column() {
    let mut p = profile(
        &[
            ("date", name("Posted")),
            ("type", name("Type")),
            ("amount", name("Amount")),
            ("memo", name("Memo")),
        ],
        DateFormat::Compact,
    );
    p.amount_mode = AmountMode::TypeColumn;
    p.debit_values = vec!["DR".into()];
    let st = csv(&p, &fixture("csv/type-column.csv")).unwrap();
    assert_eq!(st.sections[0].rows[0].amount, d("-15.00"));
    assert_eq!(st.sections[0].rows[1].amount, d("15.00"));
}

#[test]
fn import_csv_at5_parentheses_and_decimal_comma() {
    let st = csv(
        &basic_profile(),
        b"Date,Description,Amount\n10/05/2026,X,\"(1,234.56)\"\n",
    )
    .unwrap();
    assert_eq!(st.sections[0].rows[0].amount, d("-1234.56"));
    let mut p = profile(
        &[
            ("date", name("Datum")),
            ("payee", name("Beschreibung")),
            ("amount", name("Betrag")),
        ],
        DateFormat::DdMmYyyyDots,
    );
    p.delimiter = ";".into();
    p.decimal_comma = true;
    let st = csv(&p, &fixture("csv/decimal-comma.csv")).unwrap();
    assert_eq!(st.sections[0].rows[0].amount, d("1234.56"));
    assert_eq!(st.sections[0].rows[0].payee, "Bäckerei");
    assert_eq!(st.sections[0].rows[1].amount, d("-1234.56"));
}

#[test]
fn import_csv_at6_skip_rows_preamble() {
    let st = csv(&split_profile(), &fixture("csv/preamble-split-balance.csv")).unwrap();
    assert_eq!(st.sections[0].rows.len(), 3);
    assert_eq!(st.sections[0].rows[0].payee, "Opening deposit");
    assert_eq!(st.sections[0].rows[0].line, 5);
}

#[test]
fn import_csv_at7_suggestion() {
    let np = suggest_profile(&mk_import::decode_text(&fixture("csv/basic.csv"))).unwrap();
    assert_eq!(np.header, ["Date", "Description", "Amount"]);
    assert_eq!(np.sample.len(), 3);
    let s = &np.suggested;
    assert_eq!(s.columns.get("date"), Some(&name("Date")));
    assert_eq!(s.columns.get("payee"), Some(&name("Description")));
    assert_eq!(s.columns.get("amount"), Some(&name("Amount")));
    assert_eq!(s.date_format, DateFormat::MmDdYyyy);
    assert_eq!(s.amount_mode, AmountMode::Signed);
}

#[test]
fn import_csv_at9_balance_column_closing() {
    let st = csv(&split_profile(), &fixture("csv/preamble-split-balance.csv")).unwrap();
    let c = st.sections[0].closing.as_ref().unwrap();
    assert_eq!(c.amount, d("947.50"));
    assert_eq!(c.date.to_string(), "2026-10-05");
    // Newest-first: the first row's balance, dated the latest row.
    let mut p = split_profile();
    p.skip_rows = 0;
    let st = csv(&p, b"Date,Description,Debit,Credit,Balance\n2026-10-05,B,1.00,,99.00\n2026-10-01,A,,100.00,100.00\n").unwrap();
    let c = st.sections[0].closing.as_ref().unwrap();
    assert_eq!(c.amount, d("99.00"));
    assert_eq!(c.date.to_string(), "2026-10-05");
}

#[test]
fn csv_edge_cases() {
    // Missing required column.
    let err = csv(&basic_profile(), b"Date,Payee,Amount\n10/05/2026,X,1\n").unwrap_err();
    assert!(err.contains("Description"), "{err}");
    // Trailing summary line fails date parsing with its line number; skip_if fixes it.
    let body = b"Date,Description,Amount\n10/05/2026,X,1\nTotal,,1\n";
    let err = csv(&basic_profile(), body).unwrap_err();
    assert!(err.contains("line 3"), "{err}");
    let mut p = basic_profile();
    p.skip_if = vec![mk_import::SkipIf {
        column: name("Date"),
        equals: "total".into(),
    }];
    assert_eq!(csv(&p, body).unwrap().sections[0].rows.len(), 1);
    // External ids.
    let mut p = basic_profile();
    p.columns.insert("external_id".into(), ColumnRef::Index(4));
    let st = csv(
        &p,
        b"Date,Description,Amount,Id\n10/05/2026,X,1,T-9\n10/06/2026,Y,2,\n",
    )
    .unwrap();
    assert_eq!(st.sections[0].rows[0].external_id.as_deref(), Some("T-9"));
    assert_eq!(st.sections[0].rows[1].external_id, None);
}

const CARD_CSV: &str = "Posted Date,Transaction Date,Card No.,Description,Category,Debit,Credit\n10/20/2026,10/18/2026,1234,Furniture Store,Shopping,967.30,\n10/15/2026,10/15/2026,1234,Payment received,Payment,,200.00\n";

#[test]
fn import_csv_rev2_at13_suggestion_prefers_the_transaction_date() {
    let np = suggest_profile(CARD_CSV).unwrap();
    let s = &np.suggested;
    assert_eq!(s.columns.get("date"), Some(&name("Transaction Date")));
    assert_eq!(s.date_kind, Some(mk_import::DateKind::Transaction));
    assert_eq!(s.amount_mode, AmountMode::Split);
    // Column order does not matter.
    let swapped =
        "Transaction Date,Posted Date,Description,Amount\n10/18/2026,10/20/2026,X,-1.00\n";
    let s = suggest_profile(swapped).unwrap().suggested;
    assert_eq!(s.columns.get("date"), Some(&name("Transaction Date")));
    // import.date_source = posted reverses the preference.
    let s = suggest_profile_with(CARD_CSV, true).unwrap().suggested;
    assert_eq!(s.columns.get("date"), Some(&name("Posted Date")));
    assert_eq!(s.date_kind, Some(mk_import::DateKind::Posted));
    // Rows take the profile's date kind.
    let st = csv(
        &suggest_profile(CARD_CSV).unwrap().suggested,
        CARD_CSV.as_bytes(),
    )
    .unwrap();
    assert_eq!(st.sections[0].rows[0].date.to_string(), "2026-10-18");
    assert_eq!(
        st.sections[0].rows[0].date_kind,
        mk_import::DateKind::Transaction
    );
    // Older profiles without date_kind load unchanged; their rows are `unknown`.
    let st = csv(&basic_profile(), &fixture("csv/basic.csv")).unwrap();
    assert_eq!(
        st.sections[0].rows[0].date_kind,
        mk_import::DateKind::Unknown
    );
}

#[test]
fn import_csv_rev2_at14_posting_date_profile_warns() {
    let mut p = suggest_profile_with(CARD_CSV, true).unwrap().suggested;
    p.name = "Card".into();
    let st = csv(&p, CARD_CSV.as_bytes()).unwrap();
    assert_eq!(st.sections[0].rows[0].date.to_string(), "2026-10-20");
    assert_eq!(
        st.warnings,
        [
            "This profile uses the posting date (Posted Date). The file also has Transaction Date; edit the profile to use the transaction date."
        ]
    );
    // A transaction-date profile has no warning.
    let st = csv(
        &suggest_profile(CARD_CSV).unwrap().suggested,
        CARD_CSV.as_bytes(),
    )
    .unwrap();
    assert!(st.warnings.is_empty(), "{:?}", st.warnings);
}
