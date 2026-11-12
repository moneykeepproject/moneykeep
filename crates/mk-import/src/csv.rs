//! CSV parser with institution profiles (spec: import-csv.md, RFC 4180 quoting).

use mk_core::{Date, Decimal};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::amount::{AmountStyle, parse_amount};
use crate::decode::decode_text;
use crate::statement::{ClosingBalance, DateKind, Parser, RawRow, Section, Statement};
use crate::{ImportError, ImportResult};

/// A column, by header name (case-insensitive, trimmed) or 1-based index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ColumnRef {
    Index(usize),
    Name(String),
}

/// Supported date formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum DateFormat {
    #[serde(rename = "YYYY-MM-DD")]
    Iso,
    #[serde(rename = "MM/DD/YYYY")]
    MmDdYyyy,
    #[serde(rename = "DD/MM/YYYY")]
    DdMmYyyy,
    #[serde(rename = "M/D/YYYY")]
    MDYyyy,
    #[serde(rename = "D/M/YYYY")]
    DMYyyy,
    #[serde(rename = "DD.MM.YYYY")]
    DdMmYyyyDots,
    #[serde(rename = "YYYYMMDD")]
    Compact,
}

impl DateFormat {
    pub(crate) const ALL: [DateFormat; 7] = [
        DateFormat::Iso,
        DateFormat::MmDdYyyy,
        DateFormat::DdMmYyyy,
        DateFormat::MDYyyy,
        DateFormat::DMYyyy,
        DateFormat::DdMmYyyyDots,
        DateFormat::Compact,
    ];

    /// Parse a date in this format (strict about field widths where the format is).
    pub fn parse(self, text: &str) -> Option<Date> {
        let t = text.trim();
        let num = |s: &str, widths: &[usize]| -> Option<u32> {
            (widths.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()))
                .then(|| s.parse().ok())
                .flatten()
        };
        let three = |sep: char| -> Option<Vec<&str>> {
            let v: Vec<&str> = t.split(sep).collect();
            (v.len() == 3).then_some(v)
        };
        let (y, m, d) = match self {
            DateFormat::Iso => {
                let v = three('-')?;
                (
                    num(v.first()?, &[4])?,
                    num(v.get(1)?, &[2])?,
                    num(v.get(2)?, &[2])?,
                )
            }
            DateFormat::MmDdYyyy => {
                let v = three('/')?;
                (
                    num(v.get(2)?, &[4])?,
                    num(v.first()?, &[2])?,
                    num(v.get(1)?, &[2])?,
                )
            }
            DateFormat::DdMmYyyy => {
                let v = three('/')?;
                (
                    num(v.get(2)?, &[4])?,
                    num(v.get(1)?, &[2])?,
                    num(v.first()?, &[2])?,
                )
            }
            DateFormat::MDYyyy => {
                let v = three('/')?;
                (
                    num(v.get(2)?, &[4])?,
                    num(v.first()?, &[1, 2])?,
                    num(v.get(1)?, &[1, 2])?,
                )
            }
            DateFormat::DMYyyy => {
                let v = three('/')?;
                (
                    num(v.get(2)?, &[4])?,
                    num(v.get(1)?, &[1, 2])?,
                    num(v.first()?, &[1, 2])?,
                )
            }
            DateFormat::DdMmYyyyDots => {
                let v = three('.')?;
                (
                    num(v.get(2)?, &[4])?,
                    num(v.get(1)?, &[2])?,
                    num(v.first()?, &[2])?,
                )
            }
            DateFormat::Compact => (
                num(t.get(0..4)?, &[4])?,
                num(t.get(4..6)?, &[2])?,
                num(t.get(6..)?, &[2])?,
            ),
        };
        Date::from_ymd(
            i32::try_from(y).ok()?,
            u8::try_from(m).ok()?,
            u8::try_from(d).ok()?,
        )
        .ok()
    }
}

/// How amounts are expressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AmountMode {
    #[default]
    Signed,
    Split,
    TypeColumn,
}

/// Ignore rows whose column equals a value (case-insensitive, trimmed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkipIf {
    pub column: ColumnRef,
    pub equals: String,
}

fn default_delimiter() -> String {
    ",".into()
}
fn yes() -> bool {
    true
}

/// A CSV layout description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CsvProfile {
    /// Unique in the book.
    pub name: String,
    /// `,` `;` `\t` or `|`.
    #[serde(default = "default_delimiter")]
    pub delimiter: String,
    #[serde(default)]
    pub skip_rows: usize,
    #[serde(default = "yes")]
    pub has_header: bool,
    /// Role → column: date, amount, debit, credit, payee, memo, number, external_id,
    /// category, type, balance.
    pub columns: std::collections::BTreeMap<String, ColumnRef>,
    pub date_format: DateFormat,
    #[serde(default)]
    pub amount_mode: AmountMode,
    #[serde(default)]
    pub negate: bool,
    #[serde(default)]
    pub decimal_comma: bool,
    #[serde(default)]
    pub debit_values: Vec<String>,
    /// Default target account (id, full name or code).
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub skip_if: Vec<SkipIf>,
    /// Which date the date column holds (`transaction` / `posted`); unset rows are
    /// `unknown` (revision 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_kind: Option<DateKind>,
}

const ROLES: [&str; 11] = [
    "date",
    "amount",
    "debit",
    "credit",
    "payee",
    "memo",
    "number",
    "external_id",
    "category",
    "type",
    "balance",
];

impl CsvProfile {
    pub fn delimiter_char(&self) -> ImportResult<char> {
        match self.delimiter.as_str() {
            "," => Ok(','),
            ";" => Ok(';'),
            "\t" | "\\t" | "tab" => Ok('\t'),
            "|" => Ok('|'),
            other => Err(ImportError::Invalid(format!(
                "delimiter must be one of , ; tab |, not {other:?}"
            ))),
        }
    }

    /// Check the profile is self-consistent.
    pub fn validate(&self) -> ImportResult<()> {
        let name_len = self.name.trim().chars().count();
        if name_len == 0 || name_len > 100 {
            return Err(ImportError::Invalid(
                "profile name must be 1-100 characters".into(),
            ));
        }
        self.delimiter_char()?;
        for role in self.columns.keys() {
            if !ROLES.contains(&role.as_str()) {
                return Err(ImportError::Invalid(format!(
                    "unknown column role {role:?}"
                )));
            }
        }
        for c in self
            .columns
            .values()
            .chain(self.skip_if.iter().map(|s| &s.column))
        {
            match c {
                ColumnRef::Index(0) => {
                    return Err(ImportError::Invalid("column indexes are 1-based".into()));
                }
                ColumnRef::Name(_) if !self.has_header => {
                    return Err(ImportError::Invalid(
                        "columns can be named only when the file has a header".into(),
                    ));
                }
                _ => {}
            }
        }
        let need = |r: &str| -> ImportResult<()> {
            if self.columns.contains_key(r) {
                Ok(())
            } else {
                Err(ImportError::Invalid(format!(
                    "the profile needs a {r} column"
                )))
            }
        };
        need("date")?;
        match self.amount_mode {
            AmountMode::Signed => need("amount")?,
            AmountMode::Split => {
                need("debit")?;
                need("credit")?;
            }
            AmountMode::TypeColumn => {
                need("amount")?;
                need("type")?;
                if self.debit_values.is_empty() {
                    return Err(ImportError::Invalid(
                        "type_column mode needs debit_values".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Header names this profile refers to (for auto-detection).
    fn header_names(&self) -> Vec<String> {
        self.columns
            .values()
            .filter_map(|c| match c {
                ColumnRef::Name(n) => Some(n.trim().to_lowercase()),
                ColumnRef::Index(_) => None,
            })
            .collect()
    }
}

/// One parsed CSV record with the line it starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub line: usize,
    pub fields: Vec<String>,
    pub raw: String,
}

/// Split text into records per RFC 4180 (quoted fields may hold the delimiter, `""`
/// and line breaks). Entirely empty lines are skipped.
pub fn parse_records(text: &str, delim: char) -> ImportResult<Vec<Record>> {
    let mut out = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut raw = String::new();
    let mut in_quotes = false;
    let mut was_quoted = false;
    let mut line = 1usize;
    let mut start_line = 1usize;
    let mut chars = text.chars().peekable();
    let end_record = |fields: &mut Vec<String>,
                      field: &mut String,
                      raw: &mut String,
                      out: &mut Vec<Record>,
                      start: usize| {
        fields.push(std::mem::take(field));
        let f = std::mem::take(fields);
        let r = std::mem::take(raw);
        let empty = f.len() == 1 && f.first().is_some_and(String::is_empty);
        if !empty {
            out.push(Record {
                line: start,
                fields: f,
                raw: r,
            });
        }
    };
    while let Some(c) = chars.next() {
        crate::safety::check_field(&field, line)?;
        if in_quotes {
            raw.push(c);
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    raw.push('"');
                    field.push('"');
                }
                '"' => in_quotes = false,
                '\r' | '\n' => {
                    if c == '\r' && chars.peek() == Some(&'\n') {
                        chars.next();
                        raw.push('\n');
                    }
                    line += 1;
                    field.push('\n');
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !was_quoted => {
                in_quotes = true;
                was_quoted = true;
                raw.push(c);
            }
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                end_record(&mut fields, &mut field, &mut raw, &mut out, start_line);
                was_quoted = false;
                line += 1;
                start_line = line;
            }
            c if c == delim => {
                raw.push(c);
                fields.push(std::mem::take(&mut field));
                was_quoted = false;
            }
            c => {
                raw.push(c);
                field.push(c);
            }
        }
    }
    if in_quotes {
        return Err(ImportError::at(start_line, "unterminated quoted field"));
    }
    if !field.is_empty() || !fields.is_empty() {
        end_record(&mut fields, &mut field, &mut raw, &mut out, start_line);
    }
    Ok(out)
}

/// The CSV parser for one profile.
#[derive(Debug, Clone)]
pub struct CsvParser {
    pub profile: CsvProfile,
}

impl Parser for CsvParser {
    fn format(&self) -> &'static str {
        "csv"
    }

    fn parse(&self, bytes: &[u8]) -> ImportResult<Statement> {
        parse_with_profile(&decode_text(bytes), &self.profile)
    }
}

struct Columns {
    map: std::collections::BTreeMap<String, usize>,
}

impl Columns {
    fn get<'a>(&self, role: &str, rec: &'a Record) -> &'a str {
        self.map
            .get(role)
            .and_then(|i| rec.fields.get(*i))
            .map_or("", |s| s.trim())
    }
}

fn resolve_column(c: &ColumnRef, header: Option<&[String]>, role: &str) -> ImportResult<usize> {
    match c {
        ColumnRef::Index(i) => i
            .checked_sub(1)
            .ok_or_else(|| ImportError::Invalid("column indexes are 1-based".into())),
        ColumnRef::Name(n) => {
            let want = n.trim().to_lowercase();
            header
                .and_then(|h| h.iter().position(|x| x.trim().to_lowercase() == want))
                .ok_or_else(|| {
                    ImportError::Invalid(format!(
                        "required column {n:?} ({role}) is missing from the file"
                    ))
                })
        }
    }
}

fn data_records(
    text: &str,
    profile: &CsvProfile,
) -> ImportResult<(Option<Vec<String>>, Vec<Record>)> {
    let delim = profile.delimiter_char()?;
    // Skip preamble lines by physical line before RFC 4180 parsing.
    let body = skip_lines(text, profile.skip_rows);
    let offset = profile.skip_rows;
    let mut records = parse_records(body, delim)?;
    for r in &mut records {
        r.line += offset;
    }
    let header = if profile.has_header && !records.is_empty() {
        Some(records.remove(0).fields)
    } else {
        None
    };
    Ok((header, records))
}

fn skip_lines(text: &str, n: usize) -> &str {
    let mut rest = text;
    for _ in 0..n {
        match rest.find(['\n', '\r']) {
            Some(i) => {
                let after = rest.get(i..).unwrap_or("");
                let skip = if after.starts_with("\r\n") { 2 } else { 1 };
                rest = rest.get(i + skip..).unwrap_or("");
            }
            None => return "",
        }
    }
    rest
}

fn parse_with_profile(text: &str, profile: &CsvProfile) -> ImportResult<Statement> {
    profile.validate()?;
    let (header, records) = data_records(text, profile)?;
    let mut map = std::collections::BTreeMap::new();
    for (role, c) in &profile.columns {
        map.insert(role.clone(), resolve_column(c, header.as_deref(), role)?);
    }
    let cols = Columns { map };
    let skip: Vec<(usize, String)> = profile
        .skip_if
        .iter()
        .map(|s| {
            Ok((
                resolve_column(&s.column, header.as_deref(), "skip_if")?,
                s.equals.trim().to_lowercase(),
            ))
        })
        .collect::<ImportResult<_>>()?;
    let style = AmountStyle {
        decimal_comma: profile.decimal_comma,
    };
    let debit_values: Vec<String> = profile
        .debit_values
        .iter()
        .map(|v| v.trim().to_lowercase())
        .collect();
    let mut rows = Vec::new();
    let mut balances: Vec<(Date, Decimal, usize)> = Vec::new();
    for rec in &records {
        let skip_row = skip.iter().any(|(i, v)| {
            rec.fields
                .get(*i)
                .is_some_and(|f| f.trim().to_lowercase() == *v)
        });
        if skip_row {
            continue;
        }
        let at = |e: ImportError| match e {
            ImportError::Invalid(m) => ImportError::at(rec.line, m),
            other => other,
        };
        let date_text = cols.get("date", rec);
        let date = profile.date_format.parse(date_text).ok_or_else(|| {
            ImportError::at(
                rec.line,
                format!(
                    "cannot read date {date_text:?} as {:?}",
                    profile.date_format
                ),
            )
        })?;
        let amt = |role: &str| -> ImportResult<Option<Decimal>> {
            parse_amount(cols.get(role, rec), style).map_err(at)
        };
        let mut amount = match profile.amount_mode {
            AmountMode::Signed => {
                amt("amount")?.ok_or_else(|| ImportError::at(rec.line, "empty amount"))?
            }
            AmountMode::Split => {
                let debit = amt("debit")?.unwrap_or(Decimal::ZERO);
                let credit = amt("credit")?.unwrap_or(Decimal::ZERO);
                if !debit.is_zero() && !credit.is_zero() {
                    return Err(ImportError::at(
                        rec.line,
                        "both debit and credit are filled",
                    ));
                }
                credit.abs() - debit.abs()
            }
            AmountMode::TypeColumn => {
                let v = amt("amount")?
                    .ok_or_else(|| ImportError::at(rec.line, "empty amount"))?
                    .abs();
                let ty = cols.get("type", rec).to_lowercase();
                if debit_values.contains(&ty) { -v } else { v }
            }
        };
        if profile.negate {
            amount = -amount;
        }
        if profile.columns.contains_key("balance")
            && let Some(b) = amt("balance")?
        {
            balances.push((date, b, rows.len()));
        }
        let ext = cols.get("external_id", rec);
        rows.push(RawRow {
            date_kind: profile.date_kind.unwrap_or_default(),
            cleaned: Vec::new(),
            class_hint: None,
            pending: false,
            invoices: Vec::new(),
            external_id: (!ext.is_empty()).then(|| ext.to_owned()),
            date,
            amount,
            payee: cols.get("payee", rec).to_owned(),
            memo: cols.get("memo", rec).to_owned(),
            number: cols.get("number", rec).to_owned(),
            category_hint: cols.get("category", rec).to_owned(),
            cleared: None,
            splits: Vec::new(),
            trntype: None,
            review: None,
            correct_fitid: None,
            correct_action: None,
            line: rec.line,
            raw: rec.raw.clone(),
        });
    }
    let closing = closing_balance(&rows, &balances);
    let warnings = header
        .as_deref()
        .and_then(|h| posting_date_warning(h, &records, &cols))
        .into_iter()
        .collect();
    Ok(Statement {
        sections: vec![Section {
            closing,
            rows,
            ..Section::default()
        }],
        lists: Vec::new(),
        warnings,
    })
}

fn is_posting_header(h: &str) -> bool {
    let l = h.trim().to_lowercase();
    l.contains("post") || l.contains("settle")
}

fn is_transaction_header(h: &str) -> bool {
    let l = h.trim().to_lowercase();
    l.contains("trans") || l == "date"
}

/// Whether column `i` looks like a date: its header says "date", or it is named for a
/// transaction or posting ("Trans.", "Posted") and its sample values all read as dates
/// in one of the supported formats.
fn looks_like_date(header: &str, i: usize, sample: &[Vec<String>]) -> bool {
    if header.trim().to_lowercase().contains("date") {
        return true;
    }
    if !(header.to_lowercase().contains("trans") || is_posting_header(header)) {
        return false;
    }
    let values: Vec<&str> = sample
        .iter()
        .filter_map(|r| r.get(i).map(String::as_str))
        .filter(|v| !v.trim().is_empty())
        .collect();
    !values.is_empty()
        && DateFormat::ALL
            .into_iter()
            .any(|f| values.iter().all(|v| f.parse(v).is_some()))
}

/// Revision 2: the warning when a profile reads a posting-date column while the file
/// also has a transaction-date column.
fn posting_date_warning(header: &[String], records: &[Record], cols: &Columns) -> Option<String> {
    let used = *cols.map.get("date")?;
    let used_name = header.get(used)?;
    if !is_posting_header(used_name) {
        return None;
    }
    let sample: Vec<Vec<String>> = records.iter().take(5).map(|r| r.fields.clone()).collect();
    let other = header.iter().enumerate().find(|(i, h)| {
        *i != used && h.trim().to_lowercase().contains("trans") && looks_like_date(h, *i, &sample)
    })?;
    Some(format!(
        "This profile uses the posting date ({}). The file also has {}; edit the profile to use the transaction date.",
        used_name.trim(),
        other.1.trim()
    ))
}

/// The closing balance: the last row's balance by file order, or the first row's if
/// the file is newest-first; dated the latest row date.
fn closing_balance(rows: &[RawRow], balances: &[(Date, Decimal, usize)]) -> Option<ClosingBalance> {
    let first = rows.first()?;
    let last = rows.last()?;
    let latest = rows.iter().map(|r| r.date).max()?;
    let newest_first = first.date > last.date;
    let pick = if newest_first {
        balances.first()
    } else {
        balances.last()
    }?;
    Some(ClosingBalance {
        date: latest,
        amount: pick.1,
    })
}

/// The answer when no profile applies: what the file looks like and a suggestion.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NeedsProfile {
    pub header: Vec<String>,
    pub sample: Vec<Vec<String>>,
    pub suggested: CsvProfile,
}

/// The single saved profile whose header names all appear in the file's header.
pub fn detect_profile<'a>(text: &str, profiles: &'a [CsvProfile]) -> Option<&'a CsvProfile> {
    let matching: Vec<&CsvProfile> = profiles
        .iter()
        .filter(|p| {
            let names = p.header_names();
            if names.is_empty() || !p.has_header {
                return false;
            }
            let Ok(delim) = p.delimiter_char() else {
                return false;
            };
            let Ok(records) = parse_records(skip_lines(text, p.skip_rows), delim) else {
                return false;
            };
            let Some(header) = records.first() else {
                return false;
            };
            let have: Vec<String> = header
                .fields
                .iter()
                .map(|h| h.trim().to_lowercase())
                .collect();
            names.iter().all(|n| have.contains(n))
        })
        .collect();
    match matching.as_slice() {
        [one] => Some(one),
        _ => None,
    }
}

/// Guess a profile from the file: delimiter, header roles and date format. The date
/// role prefers a transaction-date column (revision 2).
pub fn suggest_profile(text: &str) -> ImportResult<NeedsProfile> {
    suggest_profile_with(text, false)
}

/// [`suggest_profile`], preferring a posting-date column when `prefer_posted`
/// (`import.date_source` = `posted`).
pub fn suggest_profile_with(text: &str, prefer_posted: bool) -> ImportResult<NeedsProfile> {
    let first_line = text.lines().next().unwrap_or("");
    let delim = [',', ';', '\t', '|']
        .into_iter()
        .max_by_key(|d| first_line.matches(*d).count())
        .unwrap_or(',');
    let records = parse_records(text, delim)?;
    let header = records
        .first()
        .map(|r| r.fields.clone())
        .unwrap_or_default();
    let sample: Vec<Vec<String>> = records
        .iter()
        .skip(1)
        .take(5)
        .map(|r| r.fields.clone())
        .collect();
    let mut columns = std::collections::BTreeMap::new();
    // The date role: among date-like headers, rank transaction dates (or a plain
    // "date") first, posting/settlement dates last; reversed for `posted`.
    let rank = |h: &str| -> u8 {
        let (first, last) = if prefer_posted {
            (is_posting_header(h), is_transaction_header(h))
        } else {
            (is_transaction_header(h), is_posting_header(h))
        };
        if first {
            0
        } else if last {
            2
        } else {
            1
        }
    };
    let date_header = header
        .iter()
        .enumerate()
        .filter(|(i, h)| looks_like_date(h, *i, &sample))
        .min_by_key(|(_, h)| rank(h))
        .map(|(_, h)| h.trim().to_owned());
    let date_kind = date_header.as_deref().and_then(|h| {
        if is_posting_header(h) {
            Some(DateKind::Posted)
        } else if h.to_lowercase().contains("trans") {
            Some(DateKind::Transaction)
        } else {
            None
        }
    });
    if let Some(h) = &date_header {
        columns.insert("date".to_owned(), ColumnRef::Name(h.clone()));
    }
    for h in &header {
        let l = h.trim().to_lowercase();
        let role = if Some(h.trim()) == date_header.as_deref() || l.contains("date") {
            continue;
        } else if l.contains("debit") || l == "withdrawal" || l.contains("withdrawals") {
            "debit"
        } else if l.contains("credit") || l == "deposit" || l.contains("deposits") {
            "credit"
        } else if l.contains("balance") {
            "balance"
        } else if l.contains("amount") {
            "amount"
        } else if l.contains("description") || l.contains("payee") || l == "name" {
            "payee"
        } else if l.contains("memo") || l.contains("note") {
            "memo"
        } else if l.contains("check") || l.contains("cheque") || l == "number" || l == "ref" {
            "number"
        } else if l.contains("category") {
            "category"
        } else if l == "type" {
            "type"
        } else {
            continue;
        };
        columns
            .entry(role.to_owned())
            .or_insert_with(|| ColumnRef::Name(h.trim().to_owned()));
    }
    let date_idx = columns.get("date").and_then(|c| match c {
        ColumnRef::Name(n) => header.iter().position(|h| h.trim() == n),
        ColumnRef::Index(i) => i.checked_sub(1),
    });
    let date_values: Vec<&str> = date_idx
        .map(|i| {
            sample
                .iter()
                .filter_map(|r| r.get(i).map(String::as_str))
                .collect()
        })
        .unwrap_or_default();
    let date_format = DateFormat::ALL
        .into_iter()
        .find(|f| !date_values.is_empty() && date_values.iter().all(|v| f.parse(v).is_some()))
        .unwrap_or(DateFormat::Iso);
    let amount_mode = if columns.contains_key("amount") || !columns.contains_key("debit") {
        AmountMode::Signed
    } else {
        AmountMode::Split
    };
    if amount_mode == AmountMode::Signed {
        columns.remove("debit");
        columns.remove("credit");
    }
    Ok(NeedsProfile {
        header,
        sample,
        suggested: CsvProfile {
            name: "Suggested".into(),
            delimiter: delim.to_string(),
            skip_rows: 0,
            has_header: true,
            columns,
            date_format,
            amount_mode,
            negate: false,
            decimal_comma: false,
            debit_values: Vec::new(),
            account: None,
            skip_if: Vec::new(),
            date_kind,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_formats() {
        assert_eq!(
            DateFormat::MmDdYyyy
                .parse("10/05/2026")
                .unwrap()
                .to_string(),
            "2026-10-05"
        );
        assert_eq!(
            DateFormat::DdMmYyyy
                .parse("05/10/2026")
                .unwrap()
                .to_string(),
            "2026-10-05"
        );
        assert_eq!(
            DateFormat::MDYyyy.parse("1/5/2026").unwrap().to_string(),
            "2026-01-05"
        );
        assert_eq!(
            DateFormat::Compact.parse("20261005").unwrap().to_string(),
            "2026-10-05"
        );
        assert_eq!(
            DateFormat::DdMmYyyyDots
                .parse("05.10.2026")
                .unwrap()
                .to_string(),
            "2026-10-05"
        );
        assert!(DateFormat::MmDdYyyy.parse("1/5/2026").is_none());
        assert!(DateFormat::Iso.parse("2026-13-01").is_none());
    }

    #[test]
    fn records_skip_blank_lines_and_keep_crlf_inside_quotes() {
        let r = parse_records("a,b\r\n\r\n\"x\r\ny\",z\r\n", ',').unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].fields, ["x\ny", "z"]);
        assert_eq!(r[1].line, 3);
        assert!(parse_records("\"open", ',').is_err());
    }
}
