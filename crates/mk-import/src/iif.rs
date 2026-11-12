//! IIF (tab-separated interchange) reading and writing, limited to the keywords and
//! columns business-dimensions.md Part D documents: `TRNS`/`SPL`/`ENDTRNS`
//! transactions with `TRNSTYPE`, `DATE`, `ACCNT`, `NAME`, `CLASS`, `AMOUNT`, `DOCNUM`
//! and `MEMO`, and the `ACCNT`, `CUST`, `VEND`, `CLASS` lists. Everything else is
//! skipped with a note. Pure: bytes in, statement out.

use std::collections::BTreeMap;

use mk_core::{Date, Decimal};

use crate::amount::{AmountStyle, parse_amount};
use crate::decode::{decode_text, lines};
use crate::qif::DateOrder;
use crate::statement::{ListItem, Parser, RawRow, RawSplit, Section, Statement};
use crate::{ImportError, ImportResult};

/// The IIF parser.
#[derive(Debug, Clone, Copy, Default)]
pub struct IifParser {
    pub date_order: DateOrder,
}

impl Parser for IifParser {
    fn format(&self) -> &'static str {
        "iif"
    }

    fn parse(&self, bytes: &[u8]) -> ImportResult<Statement> {
        parse_iif(&decode_text(bytes), self.date_order)
    }
}

/// MoneyKeep wording for a transaction type.
pub fn trntype_word(t: &str) -> String {
    match t.trim().to_uppercase().as_str() {
        "CHECK" => "Check".into(),
        "DEPOSIT" => "Deposit".into(),
        "GENERAL JOURNAL" => "Journal".into(),
        "TRANSFER" => "Transfer".into(),
        "CREDIT CARD" => "Card charge".into(),
        "BILL" => "Bill".into(),
        "INVOICE" => "Invoice".into(),
        "PAYMENT" => "Payment".into(),
        _ => t.trim().to_owned(),
    }
}

fn parse_date(text: &str, order: DateOrder) -> ImportResult<Date> {
    let bad = || ImportError::Invalid(format!("invalid date {text:?}"));
    let parts: Vec<&str> = text.trim().split('/').collect();
    let [a, b, c] = parts.as_slice() else {
        return Err(bad());
    };
    let a: u8 = a.trim().parse().map_err(|_| bad())?;
    let b: u8 = b.trim().parse().map_err(|_| bad())?;
    let y: i32 = c.trim().parse().map_err(|_| bad())?;
    let year = match c.trim().len() {
        2 => {
            if y < 70 {
                2000 + y
            } else {
                1900 + y
            }
        }
        4 => y,
        _ => return Err(bad()),
    };
    let (m, d) = match order {
        DateOrder::Mdy => (a, b),
        DateOrder::Dmy => (b, a),
    };
    Date::from_ymd(year, m, d).map_err(|_| bad())
}

fn amount(text: &str) -> ImportResult<Decimal> {
    Ok(parse_amount(text, AmountStyle::default())?.unwrap_or_default())
}

struct Txn {
    line: usize,
    trns: BTreeMap<String, String>,
    splits: Vec<(usize, BTreeMap<String, String>)>,
    raw: Vec<String>,
}

fn field<'a>(m: &'a BTreeMap<String, String>, k: &str) -> &'a str {
    m.get(k).map_or("", String::as_str)
}

fn parse_iif(text: &str, order: DateOrder) -> ImportResult<Statement> {
    let mut st = Statement::default();
    let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut current: Option<Txn> = None;
    let mut txns: Vec<Txn> = Vec::new();
    for (i, line) in lines(text).iter().enumerate() {
        let n = i + 1;
        if line.trim().is_empty() {
            continue;
        }
        let cells: Vec<&str> = line.split('\t').collect();
        let key = cells.first().map_or("", |c| c.trim());
        if let Some(h) = key.strip_prefix('!') {
            headers.insert(
                h.to_uppercase(),
                cells
                    .iter()
                    .skip(1)
                    .map(|c| c.trim().to_uppercase())
                    .collect(),
            );
            continue;
        }
        let kw = key.to_uppercase();
        let row = |kw: &str| -> Option<BTreeMap<String, String>> {
            let cols = headers.get(kw)?;
            Some(
                cols.iter()
                    .zip(cells.iter().skip(1))
                    .map(|(c, v)| (c.clone(), v.trim().trim_matches('"').to_owned()))
                    .collect(),
            )
        };
        match kw.as_str() {
            "TRNS" => {
                if let Some(t) = current.take() {
                    st.warnings
                        .push(format!("line {}: a transaction without ENDTRNS", t.line));
                    txns.push(t);
                }
                let Some(r) = row("TRNS") else {
                    return Err(ImportError::Invalid(format!(
                        "line {n}: TRNS before its !TRNS header"
                    )));
                };
                current = Some(Txn {
                    line: n,
                    trns: r,
                    splits: Vec::new(),
                    raw: vec![(*line).to_owned()],
                });
            }
            "SPL" => {
                let Some(r) = row("SPL") else {
                    return Err(ImportError::Invalid(format!(
                        "line {n}: SPL before its !SPL header"
                    )));
                };
                match current.as_mut() {
                    Some(t) => {
                        t.splits.push((n, r));
                        t.raw.push((*line).to_owned());
                    }
                    None => st
                        .warnings
                        .push(format!("line {n}: SPL outside a transaction was skipped")),
                }
            }
            "ENDTRNS" => {
                if let Some(t) = current.take() {
                    txns.push(t);
                }
            }
            "ACCNT" | "CUST" | "VEND" | "CLASS" => {
                let Some(r) = row(&kw) else {
                    st.warnings
                        .push(format!("line {n}: {kw} without its header was skipped"));
                    continue;
                };
                let name = field(&r, "NAME").to_owned();
                if name.is_empty() {
                    continue;
                }
                let kind = match kw.as_str() {
                    "ACCNT" => "account",
                    "CUST" => "customer",
                    "VEND" => "vendor",
                    _ => "class",
                };
                let address = (1..=5)
                    .map(|k| field(&r, &format!("BADDR{k}")).to_owned())
                    .filter(|a| !a.is_empty())
                    .collect();
                st.lists.push(ListItem {
                    kind: kind.into(),
                    name,
                    account_type: Some(field(&r, "ACCNTTYPE").to_owned()).filter(|t| !t.is_empty()),
                    address,
                });
            }
            "HDR" => {}
            other => *skipped.entry(other.to_owned()).or_insert(0) += 1,
        }
        if txns.len() + st.lists.len() > crate::MAX_ROWS {
            return Err(ImportError::Invalid("the file has too many rows".into()));
        }
    }
    if let Some(t) = current.take() {
        st.warnings
            .push(format!("line {}: a transaction without ENDTRNS", t.line));
        txns.push(t);
    }
    for (kw, count) in skipped {
        st.warnings
            .push(format!("skipped: {kw} rows ({count}) are not read"));
    }
    let mut sections: Vec<Section> = Vec::new();
    for t in txns {
        let acct = field(&t.trns, "ACCNT").to_owned();
        let date = parse_date(field(&t.trns, "DATE"), order)
            .map_err(|e| ImportError::Invalid(format!("line {}: {e}", t.line)))?;
        let total = amount(field(&t.trns, "AMOUNT"))
            .map_err(|e| ImportError::Invalid(format!("line {}: {e}", t.line)))?;
        let mut splits = Vec::new();
        let mut sum = total;
        for (ln, s) in &t.splits {
            let a = amount(field(s, "AMOUNT"))
                .map_err(|e| ImportError::Invalid(format!("line {ln}: {e}")))?;
            sum = sum
                .checked_add(a)
                .ok_or_else(|| ImportError::Invalid("amount overflow".into()))?;
            splits.push(RawSplit {
                category_hint: field(s, "ACCNT").to_owned(),
                amount: -a,
                memo: field(s, "MEMO").to_owned(),
                class_hint: Some(field(s, "CLASS").to_owned()).filter(|c| !c.is_empty()),
            });
        }
        let review = if t.splits.is_empty() {
            Some("unbalanced: the transaction has no SPL rows".to_owned())
        } else if !sum.is_zero() {
            Some(format!("unbalanced: the amounts sum to {sum}"))
        } else {
            None
        };
        let kind = trntype_word(field(&t.trns, "TRNSTYPE"));
        // Columns beyond the documented ones stay in `raw` only.
        let memo = field(&t.trns, "MEMO").to_owned();
        let row = RawRow {
            date_kind: crate::DateKind::Unknown,
            external_id: None,
            date,
            amount: total,
            payee: field(&t.trns, "NAME").to_owned(),
            memo,
            number: field(&t.trns, "DOCNUM").to_owned(),
            category_hint: String::new(),
            cleared: None,
            splits,
            trntype: Some(kind).filter(|k| !k.is_empty()),
            review,
            correct_fitid: None,
            correct_action: None,
            line: t.line,
            raw: t.raw.join("\n"),
            cleaned: Vec::new(),
            class_hint: Some(field(&t.trns, "CLASS").to_owned()).filter(|c| !c.is_empty()),
            pending: false,
            invoices: Vec::new(),
        };
        let key = format!("iif:{acct}");
        match sections
            .iter_mut()
            .find(|s| s.account_key.as_deref() == Some(key.as_str()))
        {
            Some(sec) => sec.rows.push(row),
            None => sections.push(Section {
                account_key: Some(key),
                account_name: Some(acct),
                rows: vec![row],
                ..Section::default()
            }),
        }
    }
    st.sections = sections;
    Ok(st)
}

/// One exported transaction: (date, account full name, payee, class, amount) per split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IifTxn {
    pub date: Date,
    pub splits: Vec<IifSplit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IifSplit {
    pub account: String,
    pub name: String,
    pub class: String,
    pub amount: Decimal,
}

/// A value safe for an IIF cell: tabs, CR and LF become spaces; non-ASCII becomes `?`
/// (counted in `replaced`).
fn cell(v: &str, replaced: &mut usize) -> String {
    v.chars()
        .map(|c| match c {
            '\t' | '\r' | '\n' => ' ',
            c if c.is_ascii() && !c.is_ascii_control() => c,
            _ => {
                *replaced += 1;
                '?'
            }
        })
        .collect()
}

/// Write an IIF file (CRLF lines). Returns the text and how many characters were
/// replaced with `?`.
pub fn write_iif(
    txns: &[IifTxn],
    now_date: &str,
    now_time: &str,
    version: &str,
) -> (String, usize) {
    let mut replaced = 0;
    let mut out = String::new();
    let mut line = |cells: &[String]| {
        out.push_str(&cells.join("\t"));
        out.push_str("\r\n");
    };
    let s = |x: &str| x.to_owned();
    line(&[
        s("!HDR"),
        s("PROD"),
        s("VER"),
        s("REL"),
        s("IIFVER"),
        s("DATE"),
        s("TIME"),
    ]);
    line(&[
        s("HDR"),
        s("MoneyKeep"),
        s(version),
        s("0"),
        s("1"),
        s(now_date),
        s(now_time),
    ]);
    let cols = ["TRNSTYPE", "DATE", "ACCNT", "NAME", "CLASS", "AMOUNT"];
    let header = |k: &str| {
        let mut v = vec![format!("!{k}")];
        if k != "ENDTRNS" {
            v.extend(cols.iter().map(|c| (*c).to_owned()));
        }
        v
    };
    line(&header("TRNS"));
    line(&header("SPL"));
    line(&header("ENDTRNS"));
    for t in txns {
        let date = format!(
            "{:02}/{:02}/{:04}",
            t.date.month(),
            t.date.day(),
            t.date.year()
        );
        for (i, sp) in t.splits.iter().enumerate() {
            let kw = if i == 0 { "TRNS" } else { "SPL" };
            line(&[
                kw.to_owned(),
                "GENERAL JOURNAL".into(),
                date.clone(),
                cell(&sp.account, &mut replaced),
                cell(&sp.name, &mut replaced),
                cell(&sp.class, &mut replaced),
                sp.amount.to_string(),
            ]);
        }
        line(&[s("ENDTRNS")]);
    }
    (out, replaced)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    const CHECK: &str = "!TRNS\tTRNSTYPE\tDATE\tACCNT\tNAME\tCLASS\tAMOUNT\tDOCNUM\tMEMO\r\n!SPL\tTRNSTYPE\tDATE\tACCNT\tNAME\tCLASS\tAMOUNT\tDOCNUM\tMEMO\r\n!ENDTRNS\r\nTRNS\tCHECK\t9/5/2026\tChecking\tPost Office\t\t-70.00\t1001\tStamps\r\nSPL\tCHECK\t9/5/2026\tOffice Supplies\t\t\t10.00\t\t\r\nSPL\tCHECK\t9/5/2026\tPostage\t\tAdmin\t60.00\t\t\r\nENDTRNS\r\n!INVITEM\tNAME\r\nINVITEM\tWidget\r\n";

    #[test]
    fn reads_a_check() {
        let st = IifParser::default().parse(CHECK.as_bytes()).unwrap();
        assert_eq!(st.sections.len(), 1);
        let r = &st.sections[0].rows[0];
        assert_eq!(r.amount, Decimal::new(-7000, 2));
        assert_eq!(r.number, "1001");
        assert_eq!(r.trntype.as_deref(), Some("Check"));
        assert_eq!(r.splits.len(), 2);
        assert_eq!(r.splits[1].class_hint.as_deref(), Some("Admin"));
        assert!(r.review.is_none());
        assert_eq!(
            st.warnings,
            vec!["skipped: INVITEM rows (1) are not read".to_owned()]
        );
    }

    #[test]
    fn writes_and_reads_back() {
        let t = IifTxn {
            date: Date::from_ymd(2026, 9, 5).unwrap(),
            splits: vec![
                IifSplit {
                    account: "Checking".into(),
                    name: "Café".into(),
                    class: String::new(),
                    amount: Decimal::new(-500, 2),
                },
                IifSplit {
                    account: "Expenses:Food".into(),
                    name: String::new(),
                    class: String::new(),
                    amount: Decimal::new(500, 2),
                },
            ],
        };
        let (text, replaced) = write_iif(&[t], "10/21/2026", "12:00:00", "0.0.1");
        assert_eq!(replaced, 1);
        assert!(text.contains("\r\n") && text.starts_with("!HDR\t"));
        let st = IifParser::default().parse(text.as_bytes()).unwrap();
        let r = &st.sections[0].rows[0];
        assert_eq!(r.payee, "Caf?");
        assert_eq!(r.splits[0].category_hint, "Expenses:Food");
        assert_eq!(r.splits[0].amount, Decimal::new(-500, 2));
    }
}
