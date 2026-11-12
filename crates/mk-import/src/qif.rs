//! QIF parser (spec: import-qif.md).

use mk_core::{Date, Decimal};
use serde::{Deserialize, Serialize};

use crate::amount::{AmountStyle, parse_amount};
use crate::decode::{decode_text, lines};
use crate::statement::{ClearedFlag, Parser, RawRow, RawSplit, Section, Statement};
use crate::{ImportError, ImportResult};

/// Day/month order of QIF dates.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum DateOrder {
    #[default]
    Mdy,
    Dmy,
}

/// Options given to `import.stage` for QIF files.
#[derive(Debug, Clone, Copy, Default)]
pub struct QifOptions {
    pub date_order: DateOrder,
    pub decimal_comma: bool,
}

/// The QIF parser.
#[derive(Debug, Clone, Copy, Default)]
pub struct QifParser {
    pub options: QifOptions,
}

impl QifParser {
    /// Parse already-decoded text.
    pub fn parse_text(&self, text: &str) -> ImportResult<Statement> {
        parse_with_hint(text, self.options)
    }
}

impl Parser for QifParser {
    fn format(&self) -> &'static str {
        "qif"
    }

    fn parse(&self, bytes: &[u8]) -> ImportResult<Statement> {
        parse_with_hint(&decode_text(bytes), self.options)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    /// Before any header.
    None,
    Bank,
    Invst,
    Account,
    Ignored,
}

#[derive(Debug, Default)]
struct Record {
    first_line: usize,
    fields: Vec<(char, String, usize)>,
    raw: Vec<String>,
}

#[derive(Debug, Default)]
struct DateCheck {
    /// (line, text) of every date.
    dates: Vec<(usize, String)>,
}

fn parse_qif(text: &str, opts: QifOptions) -> ImportResult<Statement> {
    let mut st = Statement::default();
    let mut block = Block::None;
    let mut current: Option<Section> = None;
    let mut pending_account: Option<(String, Option<String>)> = None;
    let mut rec = Record::default();
    let mut invst_count = 0usize;
    let mut checks = DateCheck::default();

    let all = lines(text);
    for (i, line) in all.iter().enumerate() {
        crate::safety::check_field(line, i + 1)?;
        let lineno = i + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(header) = trimmed.strip_prefix('!') {
            if !rec.fields.is_empty() {
                // A record not closed with ^ before a new header.
                finish_record(
                    &mut rec,
                    block,
                    &mut current,
                    &mut pending_account,
                    &mut invst_count,
                    &mut checks,
                    opts,
                )?;
                st.warnings
                    .push(format!("line {}: record not ended with ^", lineno - 1));
            }
            let h = header.trim();
            let lower = h.to_ascii_lowercase();
            if lower.starts_with("option:") || lower.starts_with("clear:") {
                continue;
            }
            if lower == "account" {
                block = Block::Account;
                continue;
            }
            let Some(kind) = lower.strip_prefix("type:") else {
                return Err(ImportError::at(lineno, format!("unknown QIF header !{h}")));
            };
            block = match kind.trim() {
                "bank" | "cash" | "ccard" | "oth a" | "oth l" => {
                    // A new bank-style block starts a section for the pending account,
                    // or continues/creates the unnamed one.
                    match pending_account.take() {
                        Some((name, _ty)) => {
                            if let Some(sec) = current.take() {
                                st.sections.push(sec);
                            }
                            current = Some(Section {
                                account_key: Some(format!("qif:{name}")),
                                account_name: Some(name),
                                ..Section::default()
                            });
                        }
                        None => {
                            if current.is_none() {
                                current = Some(Section::default());
                            }
                        }
                    }
                    Block::Bank
                }
                "invst" => {
                    pending_account = None;
                    Block::Invst
                }
                "cat" | "class" | "memorized" | "invoice" => Block::Ignored,
                other => {
                    return Err(ImportError::at(
                        lineno,
                        format!("unknown QIF header !Type:{other}"),
                    ));
                }
            };
            continue;
        }
        if trimmed == "^" {
            finish_record(
                &mut rec,
                block,
                &mut current,
                &mut pending_account,
                &mut invst_count,
                &mut checks,
                opts,
            )?;
            continue;
        }
        let mut chars = trimmed.chars();
        let Some(code) = chars.next() else { continue };
        if rec.fields.is_empty() {
            rec.first_line = lineno;
        }
        rec.fields
            .push((code, chars.as_str().trim().to_owned(), lineno));
        rec.raw.push((*line).to_owned());
    }
    if !rec.fields.is_empty() {
        let last = rec.fields.last().map_or(0, |f| f.2);
        finish_record(
            &mut rec,
            block,
            &mut current,
            &mut pending_account,
            &mut invst_count,
            &mut checks,
            opts,
        )?;
        st.warnings.push(format!(
            "line {last}: final record has no closing ^ (accepted)"
        ));
    }
    if let Some(sec) = current.take() {
        st.sections.push(sec);
    }
    if invst_count > 0 {
        st.warnings
            .push(format!("skipped: investment records ({invst_count})"));
    }
    if st.sections.is_empty() {
        st.sections.push(Section::default());
    }
    if st.row_count() == 0 {
        st.warnings.push("the file contains no transactions".into());
    }
    check_date_order(&checks, opts.date_order)?;
    Ok(st)
}

#[allow(clippy::too_many_arguments)]
fn finish_record(
    rec: &mut Record,
    block: Block,
    current: &mut Option<Section>,
    pending_account: &mut Option<(String, Option<String>)>,
    invst_count: &mut usize,
    checks: &mut DateCheck,
    opts: QifOptions,
) -> ImportResult<()> {
    let r = std::mem::take(rec);
    if r.fields.is_empty() {
        return Ok(());
    }
    match block {
        Block::Ignored => Ok(()),
        Block::Invst => {
            *invst_count += 1;
            Ok(())
        }
        Block::Account => {
            let name = last_field(&r, 'N');
            let ty = last_field(&r, 'T');
            match name {
                Some(n) if !n.is_empty() => {
                    *pending_account = Some((n, ty));
                    Ok(())
                }
                _ => Err(ImportError::at(
                    r.first_line,
                    "account block without a name (N)",
                )),
            }
        }
        Block::None => Err(ImportError::at(
            r.first_line,
            "transaction data before any !Type: header",
        )),
        Block::Bank => {
            let row = bank_row(&r, checks, opts)?;
            current.get_or_insert_with(Section::default).rows.push(row);
            Ok(())
        }
    }
}

fn last_field(r: &Record, code: char) -> Option<String> {
    r.fields
        .iter()
        .rev()
        .find(|(c, _, _)| *c == code)
        .map(|(_, v, _)| v.clone())
}

fn bank_row(r: &Record, checks: &mut DateCheck, opts: QifOptions) -> ImportResult<RawRow> {
    let style = AmountStyle {
        decimal_comma: opts.decimal_comma,
    };
    let amount_of = |v: &str, line: usize| -> ImportResult<Decimal> {
        parse_amount(v, style)
            .map_err(|e| ImportError::at(line, e.to_string()))?
            .ok_or_else(|| ImportError::at(line, "empty amount"))
    };
    let mut date = None;
    let mut amount_t = None;
    let mut amount_u = None;
    let mut payee = String::new();
    let mut memo = String::new();
    let mut number = String::new();
    let mut category = String::new();
    let mut cleared = None;
    // (category, memo, amount, line)
    let mut splits: Vec<(String, String, Option<Decimal>, usize)> = Vec::new();
    for (code, value, line) in &r.fields {
        let (line, v) = (*line, value.as_str());
        match code {
            'D' => {
                checks.dates.push((line, v.to_owned()));
                date =
                    Some(parse_qif_date(v, opts.date_order).map_err(|e| ImportError::at(line, e))?);
            }
            'T' => amount_t = Some(amount_of(v, line)?),
            'U' => amount_u = Some(amount_of(v, line)?),
            'P' => payee = v.to_owned(),
            'M' => memo = v.to_owned(),
            'N' => number = v.to_owned(),
            'C' => {
                cleared = match v {
                    "" | "*" | "c" | "C" => Some(ClearedFlag::Cleared),
                    "X" | "x" | "R" | "r" => Some(ClearedFlag::Reconciled),
                    _ => None,
                }
            }
            'L' => category = strip_class(v),
            'S' => splits.push((strip_class(v), String::new(), None, line)),
            'E' => {
                if let Some(s) = splits.last_mut() {
                    s.1 = v.to_owned();
                }
            }
            '$' => match splits.last_mut() {
                Some(s) => s.2 = Some(amount_of(v, line)?),
                None => {
                    return Err(ImportError::at(
                        line,
                        "split amount ($) without a split (S)",
                    ));
                }
            },
            _ => {} // A, F, %, and unknown codes are ignored.
        }
    }
    let date = date.ok_or_else(|| ImportError::at(r.first_line, "record has no date (D)"))?;
    let amount = amount_t
        .or(amount_u)
        .ok_or_else(|| ImportError::at(r.first_line, "record has no amount (T)"))?;
    let splits = splits
        .into_iter()
        .map(|(cat, memo, amt, line)| {
            Ok(RawSplit {
                category_hint: cat,
                amount: amt.ok_or_else(|| ImportError::at(line, "split has no amount ($)"))?,
                memo,
                class_hint: None,
            })
        })
        .collect::<ImportResult<Vec<_>>>()?;
    Ok(RawRow {
        date_kind: crate::DateKind::Unknown,
        cleaned: Vec::new(),
        class_hint: None,
        pending: false,
        invoices: Vec::new(),
        external_id: None,
        date,
        amount,
        payee,
        memo,
        number,
        category_hint: category,
        cleared,
        splits,
        trntype: None,
        review: None,
        correct_fitid: None,
        correct_action: None,
        line: r.first_line,
        raw: r.raw.join("\n"),
    })
}

/// Drop a `/Class` suffix from a category.
fn strip_class(v: &str) -> String {
    match v.find('/') {
        Some(i) => v.get(..i).unwrap_or(v).trim().to_owned(),
        None => v.trim().to_owned(),
    }
}

/// Parsed date parts before day/month ordering is applied.
struct DateParts {
    a: u32,
    b: u32,
    year: i32,
}

fn date_parts(text: &str) -> Result<Option<DateParts>, String> {
    let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    // ISO form.
    if t.len() == 10 && t.as_bytes().get(4) == Some(&b'-') && t.as_bytes().get(7) == Some(&b'-') {
        return Ok(None);
    }
    let bad = || format!("unrecognised date {text:?}");
    let (head, year_text, apostrophe) = match t.find('\'') {
        Some(i) => (
            t.get(..i).ok_or_else(bad)?,
            t.get(i + 1..).ok_or_else(bad)?,
            true,
        ),
        None => {
            let i = t.rfind(['/', '-', '.']).ok_or_else(bad)?;
            (
                t.get(..i).ok_or_else(bad)?,
                t.get(i + 1..).ok_or_else(bad)?,
                false,
            )
        }
    };
    let mut parts = head.split(['/', '-', '.']);
    let a = parts.next().ok_or_else(bad)?;
    let b = parts.next().ok_or_else(bad)?;
    if parts.next().is_some() {
        return Err(bad());
    }
    let num = |s: &str| -> Result<u32, String> {
        if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
            Err(bad())
        } else {
            s.parse().map_err(|_| bad())
        }
    };
    let y = num(year_text)?;
    let year = match (year_text.len(), apostrophe) {
        (2, true) => 2000 + i32::try_from(y).map_err(|_| bad())?,
        (2, false) if y <= 69 => 2000 + i32::try_from(y).map_err(|_| bad())?,
        (2, false) => 1900 + i32::try_from(y).map_err(|_| bad())?,
        (4, false) => i32::try_from(y).map_err(|_| bad())?,
        _ => return Err(bad()),
    };
    Ok(Some(DateParts {
        a: num(a)?,
        b: num(b)?,
        year,
    }))
}

fn make_date(year: i32, month: u32, day: u32) -> Option<Date> {
    Date::from_ymd(year, u8::try_from(month).ok()?, u8::try_from(day).ok()?).ok()
}

/// Parse one QIF date in the given order.
pub(crate) fn parse_qif_date(text: &str, order: DateOrder) -> Result<Date, String> {
    match date_parts(text)? {
        None => {
            let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            t.parse::<Date>().map_err(|e| e.to_string())
        }
        Some(p) => {
            let (m, d) = match order {
                DateOrder::Mdy => (p.a, p.b),
                DateOrder::Dmy => (p.b, p.a),
            };
            make_date(p.year, m, d).ok_or_else(|| format!("impossible date {text:?}"))
        }
    }
}

/// If a date failed in the chosen order but every date works in the other, suggest it.
fn check_date_order(checks: &DateCheck, order: DateOrder) -> ImportResult<()> {
    let other = match order {
        DateOrder::Mdy => DateOrder::Dmy,
        DateOrder::Dmy => DateOrder::Mdy,
    };
    for (line, text) in &checks.dates {
        if parse_qif_date(text, order).is_err() && parse_qif_date(text, other).is_ok() {
            let name = match other {
                DateOrder::Mdy => "mdy",
                DateOrder::Dmy => "dmy",
            };
            return Err(ImportError::at(
                *line,
                format!("date {text:?} is impossible in this order; try date_order \"{name}\""),
            ));
        }
    }
    Ok(())
}

/// Wraps `parse_qif` so date-order errors carry the suggestion even when the bad date
/// is the first failure. (`bank_row` fails fast; re-check to produce the hint.)
pub(crate) fn parse_with_hint(text: &str, opts: QifOptions) -> ImportResult<Statement> {
    match parse_qif(text, opts) {
        Ok(s) => Ok(s),
        Err(e) => {
            // Collect every D line and see whether the other order would work.
            let mut checks = DateCheck::default();
            for (i, line) in lines(text).iter().enumerate() {
                if let Some(v) = line.trim().strip_prefix('D') {
                    checks.dates.push((i + 1, v.trim().to_owned()));
                }
            }
            check_date_order(&checks, opts.date_order)?;
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str, o: DateOrder) -> String {
        parse_qif_date(s, o).unwrap().to_string()
    }

    #[test]
    fn date_shapes() {
        assert_eq!(d("6/ 1'26", DateOrder::Mdy), "2026-06-01");
        assert_eq!(d("06/01/2026", DateOrder::Mdy), "2026-06-01");
        assert_eq!(d("6-1-26", DateOrder::Mdy), "2026-06-01");
        assert_eq!(d("1.6.99", DateOrder::Dmy), "1999-06-01");
        assert_eq!(d("12/31/69", DateOrder::Mdy), "2069-12-31");
        assert_eq!(d("12/31/70", DateOrder::Mdy), "1970-12-31");
        assert_eq!(d("2026-06-01", DateOrder::Dmy), "2026-06-01");
        assert!(parse_qif_date("13/13/2026", DateOrder::Mdy).is_err());
        assert!(parse_qif_date("garbage", DateOrder::Mdy).is_err());
    }
}
