//! import-ofx.md parser-level acceptance tests.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use mk_import::{OfxParser, Parser, Statement};

fn fixture(path: &str) -> Vec<u8> {
    let p = format!(
        "{}/../../examples/fixtures/ofx/{path}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn parse(path: &str) -> Statement {
    OfxParser::default().parse(&fixture(path)).unwrap()
}

fn d(s: &str) -> mk_core::Decimal {
    s.parse().unwrap()
}

#[test]
fn import_ofx_at1_sgml_checking() {
    let st = parse("checking-v1.ofx");
    assert_eq!(st.sections.len(), 1);
    let s = &st.sections[0];
    assert_eq!(s.currency.as_ref().unwrap().as_str(), "USD");
    assert_eq!(s.rows.len(), 3);
    let r = &s.rows;
    assert_eq!(r[0].date.to_string(), "2026-10-01");
    assert_eq!(r[0].amount, d("1500.00"));
    assert_eq!(r[0].payee, "Employer Payroll");
    assert_eq!(r[0].memo, "October salary");
    assert_eq!(r[0].external_id.as_deref(), Some("CHK-0001"));
    assert_eq!(r[0].trntype.as_deref(), Some("DIRECTDEP"));
    assert_eq!(r[1].number, "1042");
    assert_eq!(r[1].amount, d("-42.10"));
    assert_eq!(r[2].date.to_string(), "2026-10-05");
    let c = s.closing.as_ref().unwrap();
    assert_eq!(
        (c.date.to_string().as_str(), c.amount),
        ("2026-10-20", d("2439.15"))
    );
    assert_eq!(s.available.as_ref().unwrap().amount, d("2400.00"));
    assert_eq!(
        s.period.map(|(a, b)| (a.to_string(), b.to_string())),
        Some(("2026-10-01".into(), "2026-10-20".into()))
    );
    assert_eq!(s.account_hint.as_deref(), Some("6789"));
    assert!(s.account_key.as_ref().unwrap().starts_with("ofx:"));
}

#[test]
fn import_ofx_at2_xml_stages_identically() {
    let mut a = parse("checking-v1.ofx");
    let mut b = parse("checking-v2.ofx");
    // The raw record text is diagnostic only; line numbers differ by layout.
    for st in [&mut a, &mut b] {
        for r in &mut st.sections[0].rows {
            r.line = 0;
        }
    }
    assert_eq!(a, b);
}

#[test]
fn import_ofx_at3_credit_card_signs() {
    let st = parse("creditcard-v1.ofx");
    let s = &st.sections[0];
    assert_eq!(s.rows[0].amount, d("-45.10"));
    assert_eq!(s.rows[1].amount, d("200.00"));
    assert_eq!(s.closing.as_ref().unwrap().amount, d("-812.40"));
    assert_eq!(s.account_hint.as_deref(), Some("4444"));
}

#[test]
fn import_ofx_at5_dates_and_at6_amounts() {
    let st = parse("dates-amounts.ofx");
    let rows = &st.sections[0].rows;
    assert_eq!(rows.len(), 3, "the 20261305 transaction is rejected");
    assert!(rows.iter().all(|r| r.date.to_string() == "2026-10-05"));
    assert_eq!(rows[0].amount, d("550"));
    assert_eq!(rows[1].amount, d("-12.34"));
    assert_eq!(rows[2].amount, d("7.5"));
    assert!(
        st.warnings
            .iter()
            .any(|w| w.contains("invalid date") && w.contains("20261305")),
        "{:?}",
        st.warnings
    );
}

#[test]
fn import_ofx_at7_encodings_and_entities() {
    let st = parse("charset1252.ofx");
    assert_eq!(st.sections[0].rows[0].payee, "Café du Parc & Bar");
    let st = parse("utf8-v2.ofx");
    assert_eq!(st.sections[0].rows[2].payee, "Café du Parc & Bar");
}

#[test]
fn import_ofx_at8_pending_not_staged() {
    let st = parse("pending.ofx");
    assert_eq!(st.row_count(), 1);
    assert!(
        st.warnings
            .contains(&"pending transactions not imported (2)".to_owned()),
        "{:?}",
        st.warnings
    );
}

#[test]
fn import_ofx_at10_status_error() {
    let st = parse("status-error.ofx");
    assert_eq!(st.row_count(), 0);
    assert!(st.sections.is_empty());
    assert!(
        st.warnings
            .iter()
            .any(|w| w.contains("2003") && w.contains("account not found")),
        "{:?}",
        st.warnings
    );
}

#[test]
fn import_ofx_at12_qfx_extensions_ignored() {
    let mut a = parse("checking-v1.ofx");
    let mut b = parse("web-connect.qfx");
    for st in [&mut a, &mut b] {
        for r in &mut st.sections[0].rows {
            r.line = 0;
        }
    }
    assert_eq!(a, b);
}

#[test]
fn import_ofx_at13_doctype_rejected() {
    let err = OfxParser::default()
        .parse(&fixture("doctype-xxe.ofx"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("DOCTYPE"), "{err}");
}

#[test]
fn import_ofx_at14_foreign_currency_flagged() {
    let st = parse("foreign-and-fee.ofx");
    let r = &st.sections[0].rows;
    assert!(
        r[0].review
            .as_deref()
            .unwrap()
            .contains("foreign-currency transaction")
    );
    assert_eq!(r[1].review, None);
    assert_eq!(r[1].trntype.as_deref(), Some("FEE"));
}

#[test]
fn ofx_edge_cases() {
    assert!(
        OfxParser::default()
            .parse(b"Date,Amount\n")
            .unwrap_err()
            .to_string()
            .contains("not an OFX file")
    );
    // Missing FITID: skipped with a warning naming its position.
    let body = String::from_utf8(fixture("checking-v1.ofx"))
        .unwrap()
        .replace("<FITID>CHK-0002", "");
    let st = OfxParser::default().parse(body.as_bytes()).unwrap();
    assert_eq!(st.row_count(), 2);
    assert!(
        st.warnings.iter().any(|w| w.contains("transaction 2")),
        "{:?}",
        st.warnings
    );
    // TRNTYPE DEBIT with a positive amount: sign trusted, warning recorded.
    let body = String::from_utf8(fixture("checking-v1.ofx"))
        .unwrap()
        .replace("<TRNTYPE>DIRECTDEP", "<TRNTYPE>DEBIT");
    let st = OfxParser::default().parse(body.as_bytes()).unwrap();
    assert_eq!(st.sections[0].rows[0].amount, d("1500.00"));
    assert!(st.warnings.iter().any(|w| w.contains("DEBIT")));
    // A 255-character FITID is accepted; EXTDNAME wins over NAME.
    let long = "F".repeat(255);
    let body = String::from_utf8(fixture("checking-v1.ofx"))
        .unwrap()
        .replace("CHK-0001", &long)
        .replace(
            "<NAME>Employer Payroll",
            "<NAME>EMPLOYER<EXTDNAME>Employer Payroll Services Ltd",
        );
    let st = OfxParser::default().parse(body.as_bytes()).unwrap();
    assert_eq!(
        st.sections[0].rows[0].external_id.as_deref(),
        Some(long.as_str())
    );
    assert_eq!(
        st.sections[0].rows[0].payee,
        "Employer Payroll Services Ltd"
    );
    // Both a bank and a card statement: two sections.
    let bank = String::from_utf8(fixture("checking-v1.ofx")).unwrap();
    let card = String::from_utf8(fixture("creditcard-v1.ofx")).unwrap();
    let cc_part = &card[card.find("<CREDITCARDMSGSRSV1>").unwrap()..card.find("</OFX>").unwrap()];
    let both = bank.replace("</OFX>", &format!("{cc_part}</OFX>"));
    assert_eq!(
        OfxParser::default()
            .parse(both.as_bytes())
            .unwrap()
            .sections
            .len(),
        2
    );
    // Investment statements become investment sections (import-investments.md; the
    // command layer skips them when the investments module is off).
    let inv = bank.replace("</OFX>", "<INVSTMTMSGSRSV1><INVSTMTTRNRS><TRNUID>9<STATUS><CODE>0<SEVERITY>INFO</STATUS><INVSTMTRS></INVSTMTRS></INVSTMTTRNRS></INVSTMTMSGSRSV1></OFX>");
    let st = OfxParser::default().parse(inv.as_bytes()).unwrap();
    assert_eq!(st.sections.len(), 2);
    assert_eq!(st.sections[1].kind, mk_import::SectionKind::Investment);
    // In-file corrections: a later DELETE drops the earlier row; REPLACE replaces it.
    let corr = bank.replace("</BANKTRANLIST>", "<STMTTRN><TRNTYPE>POS<DTPOSTED>20261006<TRNAMT>-19.00<FITID>CHK-0003B<CORRECTFITID>CHK-0003<CORRECTACTION>REPLACE<NAME>Corner Grocery</STMTTRN><STMTTRN><TRNTYPE>CHECK<DTPOSTED>20261003<TRNAMT>-42.10<FITID>X<CORRECTFITID>CHK-0002<CORRECTACTION>DELETE<NAME>x</STMTTRN></BANKTRANLIST>");
    let st = OfxParser::default().parse(corr.as_bytes()).unwrap();
    let ids: Vec<&str> = st.sections[0]
        .rows
        .iter()
        .map(|r| r.external_id.as_deref().unwrap())
        .collect();
    assert_eq!(ids, ["CHK-0001", "CHK-0003B"]);
}

/// The credit-card fixture with `DTUSER` added to the third transaction.
fn card_with_dtuser(dtuser: &str) -> String {
    String::from_utf8(fixture("creditcard-v1.ofx"))
        .unwrap()
        .replace(
            "<DTPOSTED>20261020</DTPOSTED>",
            &format!("<DTPOSTED>20261020</DTPOSTED><DTUSER>{dtuser}</DTUSER>"),
        )
}

#[test]
fn import_ofx_at16_dtuser_is_the_transaction_date() {
    let body = card_with_dtuser("20261018");
    let st = OfxParser::default().parse(body.as_bytes()).unwrap();
    let r = &st.sections[0].rows[2];
    assert_eq!(r.date.to_string(), "2026-10-18");
    assert_eq!(r.date_kind, mk_import::DateKind::Transaction);
    let st = OfxParser { posted_dates: true }
        .parse(body.as_bytes())
        .unwrap();
    let r = &st.sections[0].rows[2];
    assert_eq!(r.date.to_string(), "2026-10-20");
    assert_eq!(r.date_kind, mk_import::DateKind::Posted);
}

#[test]
fn import_ofx_at17_without_dtuser_the_posting_date() {
    let st = parse("creditcard-v1.ofx");
    let r = &st.sections[0].rows[0];
    assert_eq!(r.date.to_string(), "2026-10-04");
    assert_eq!(r.date_kind, mk_import::DateKind::Posted);
}

#[test]
fn import_ofx_at18_invalid_dtuser_falls_back_with_a_warning() {
    let body = card_with_dtuser("20261340");
    let st = OfxParser::default().parse(body.as_bytes()).unwrap();
    let r = &st.sections[0].rows[2];
    assert_eq!(r.date.to_string(), "2026-10-20");
    assert_eq!(r.date_kind, mk_import::DateKind::Posted);
    assert!(
        st.warnings
            .iter()
            .any(|w| w.contains("transaction 3") && w.contains("DTUSER")),
        "{:?}",
        st.warnings
    );
}
