//! Investment profiles for broker CSV, JSON and XML files (spec:
//! import-investments.md, "layout profiles" and revision 2). A profile says where the
//! records are, which source field holds each neutral field, how dates and numbers
//! are written and how the broker's action texts map to MoneyKeep actions. Built-in
//! profiles are data (`builtin_profiles.json`).

use std::collections::{BTreeMap, BTreeSet};

use mk_core::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::csv::{DateFormat, parse_records};
use crate::decode::decode_text;
use crate::statement::{
    InvAction, InvRow, Position, RawRow, Section, SectionKind, SourceSecurity, Statement,
};
use crate::{ImportError, ImportResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileFormat {
    Csv,
    Json,
    Xml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchType {
    #[default]
    Equals,
    StartsWith,
    Contains,
}

/// One action rule: the broker's text → a MoneyKeep action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRule {
    #[serde(default)]
    pub match_type: MatchType,
    pub text: String,
    /// `buy`, `sell`, `dividend`, `reinvest`, `interest`, `fee`, `shares_in`,
    /// `shares_out`, `split`, `return_of_capital`, `cash`, `ignore`, `unsupported`,
    /// `sell_quantity`, `sell_proceeds`, `reinvest_shares`.
    pub action: String,
    /// Dividend kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `interest` with (true) or without (false) the row's security; default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub with_security: Option<bool>,
    /// The reason shown for `unsupported`, or a note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Not yet seen in real files: the first rows it matches need review.
    #[serde(default)]
    pub unverified: bool,
}

/// One security rule: sets id type and kind from the symbol and description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityRule {
    /// Regular expression the whole symbol must match (e.g. `^[A-Z0-9]{9}$`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_pattern: Option<String>,
    /// Text the description must contain (case-insensitive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_contains: Option<String>,
    /// `CUSIP` or `TICKER`.
    pub id_type: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumberFormat {
    /// Decimal point: `.` or `,`.
    #[serde(default = "dot")]
    pub decimal: String,
    #[serde(default = "comma", skip_serializing_if = "Option::is_none")]
    pub thousands: Option<String>,
    #[serde(default = "yes")]
    pub parens_negative: bool,
    #[serde(default = "symbols")]
    pub currency_symbols: Vec<String>,
}

fn dot() -> String {
    ".".into()
}
fn comma() -> Option<String> {
    Some(",".into())
}
fn yes() -> bool {
    true
}
fn symbols() -> Vec<String> {
    vec!["$".into(), "€".into(), "£".into()]
}

impl Default for NumberFormat {
    fn default() -> Self {
        Self {
            decimal: dot(),
            thousands: comma(),
            parens_negative: true,
            currency_symbols: symbols(),
        }
    }
}

/// Holdings in the file (optional).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionsSpec {
    pub records: String,
    pub fields: BTreeMap<String, String>,
}

/// A layout profile for broker CSV, JSON or XML files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvProfile {
    pub name: String,
    pub format: ProfileFormat,
    /// Built in (shipped with MoneyKeep; cannot be deleted).
    #[serde(default)]
    pub builtin: bool,
    /// How files are recognised: CSV header names; JSON keys present; XML root (and
    /// record) element names.
    #[serde(default)]
    pub signature: Vec<String>,
    /// CSV: ignored (data rows); JSON: path to the array (`data.transactions`, `[*]`);
    /// XML: path of the record element (`Statement/Transactions/Transaction`).
    #[serde(default)]
    pub records: String,
    #[serde(default)]
    pub skip_rows: usize,
    /// Neutral field → source column / key path / element or `@attribute` path.
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    #[serde(default = "mdy")]
    pub date_format: DateFormat,
    #[serde(default)]
    pub number_format: NumberFormat,
    #[serde(default)]
    pub actions: Vec<ActionRule>,
    /// Quantity and amount carry the cash direction; MoneyKeep uses absolute values
    /// and the action (always true; kept so the editor can explain it).
    #[serde(default = "yes")]
    pub signs: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_key: Option<String>,
    /// A pattern with one capture group, matched against the file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_from_file_name: Option<String>,
    #[serde(default)]
    pub security_rules: Vec<SecurityRule>,
    /// JSON/XML path of the file's own total of the amounts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub positions: Option<PositionsSpec>,
}

fn mdy() -> DateFormat {
    DateFormat::MmDdYyyy
}

/// Neutral fields a profile can map, in editor order.
pub const NEUTRAL_FIELDS: [&str; 13] = [
    "date",
    "action",
    "symbol",
    "cusip",
    "security_name",
    "quantity",
    "price",
    "commission",
    "fees",
    "amount",
    "description",
    "external_id",
    "account",
];

/// The built-in profiles (data only).
pub fn builtin_profiles() -> Vec<InvProfile> {
    serde_json::from_str::<Vec<InvProfile>>(include_str!("builtin_profiles.json"))
        .unwrap_or_default()
        .into_iter()
        .map(|mut p| {
            p.builtin = true;
            p
        })
        .collect()
}

// ----- numbers -----------------------------------------------------------------

/// Parse a money or number field: currency symbols anywhere, a minus sign before
/// or after the symbol, thousands separators, parentheses for negative. Empty → None.
pub fn parse_money(text: &str, nf: &NumberFormat) -> ImportResult<Option<Decimal>> {
    crate::safety::check_number_len(text)?;
    let bad = || ImportError::Invalid(format!("{text:?} is not a number"));
    let mut t: String = text.trim().to_owned();
    for sym in &nf.currency_symbols {
        if !sym.is_empty() {
            t = t.replace(sym.as_str(), "");
        }
    }
    let mut t: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    if t.is_empty() {
        return Ok(None);
    }
    let mut negative = false;
    if nf.parens_negative && t.starts_with('(') && t.ends_with(')') && t.len() >= 2 {
        negative = true;
        t = t.get(1..t.len() - 1).ok_or_else(bad)?.to_owned();
    }
    while let Some(rest) = t.strip_prefix('-') {
        negative = !negative;
        t = rest.to_owned();
    }
    if let Some(rest) = t.strip_prefix('+') {
        t = rest.to_owned();
    }
    if let Some(rest) = t.strip_suffix('-') {
        negative = !negative;
        t = rest.to_owned();
    }
    if let Some(th) = &nf.thousands
        && !th.is_empty()
        && th != &nf.decimal
    {
        t = t.replace(th.as_str(), "");
    }
    if nf.decimal != "." {
        t = t.replace(nf.decimal.as_str(), ".");
    }
    if t.is_empty()
        || !t.chars().all(|c| c.is_ascii_digit() || c == '.')
        || t.matches('.').count() > 1
    {
        return Err(bad());
    }
    let d: Decimal = t.parse().map_err(|_| bad())?;
    Ok(Some(if negative { -d } else { d }))
}

// ----- records -------------------------------------------------------------------

/// One source record: field path → text, plus its line and raw text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceRecord {
    pub line: usize,
    pub fields: BTreeMap<String, String>,
    pub raw: String,
}

impl SourceRecord {
    fn get(&self, path: &str) -> &str {
        self.fields.get(path).map_or("", String::as_str)
    }
}

/// A file read into records, with access to the whole document for totals.
struct Loaded {
    records: Vec<SourceRecord>,
    /// JSON root, if JSON.
    json: Option<Value>,
    /// XML root, if XML.
    xml: Option<crate::xml::Element>,
}

/// The format a file looks like.
pub fn detect_format(bytes: &[u8]) -> ProfileFormat {
    let text = decode_text(bytes);
    let t = text.trim_start();
    if t.starts_with('{') || t.starts_with('[') {
        ProfileFormat::Json
    } else if t.starts_with('<') {
        ProfileFormat::Xml
    } else {
        ProfileFormat::Csv
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn flatten_json(prefix: &str, v: &Value, out: &mut BTreeMap<String, String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_json(&key, x, out);
            }
        }
        Value::Array(_) => {
            out.insert(prefix.to_owned(), v.to_string());
        }
        _ => {
            out.insert(prefix.to_owned(), scalar(v));
        }
    }
}

/// The JSON value at a dot path (`[*]` or empty = the root).
fn json_at<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let p = path.trim();
    if p.is_empty() || p == "[*]" || p == "$" {
        return Some(root);
    }
    let mut cur = root;
    for part in p
        .trim_end_matches("[*]")
        .split('.')
        .filter(|s| !s.is_empty())
    {
        cur = cur.get(part)?;
    }
    Some(cur)
}

fn xml_path(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

fn load(
    bytes: &[u8],
    format: ProfileFormat,
    records: &str,
    skip_rows: usize,
) -> ImportResult<Loaded> {
    let text = decode_text(bytes);
    match format {
        ProfileFormat::Csv => {
            let all = parse_records(&text, ',')?;
            let mut it = all.into_iter().skip(skip_rows);
            let header = it
                .next()
                .ok_or_else(|| ImportError::Invalid("the CSV file has no header row".into()))?;
            let names: Vec<String> = header.fields.iter().map(|h| h.trim().to_owned()).collect();
            let records = it
                .map(|r| SourceRecord {
                    line: r.line,
                    fields: names
                        .iter()
                        .cloned()
                        .zip(r.fields.iter().map(|f| f.trim().to_owned()))
                        .collect(),
                    raw: r.raw.trim_end().to_owned(),
                })
                .collect();
            Ok(Loaded {
                records,
                json: None,
                xml: None,
            })
        }
        ProfileFormat::Json => {
            let root: Value = crate::safety::parse_json(&text)
                .map_err(|e| ImportError::Invalid(format!("invalid JSON: {e}")))?;
            let arr = json_at(&root, records)
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    ImportError::Invalid(format!("no list of records at {records:?}"))
                })?;
            let records = arr
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let mut fields = BTreeMap::new();
                    flatten_json("", v, &mut fields);
                    SourceRecord {
                        line: i + 1,
                        fields,
                        raw: v.to_string(),
                    }
                })
                .collect();
            Ok(Loaded {
                records,
                json: Some(root),
                xml: None,
            })
        }
        ProfileFormat::Xml => {
            let root = crate::xml::parse(&text)?;
            let path = xml_path(records);
            let found = match path.split_first() {
                Some((first, rest)) if *first == root.name => root.select(rest),
                Some(_) => root.select(&path),
                None => Vec::new(),
            };
            let records = found
                .iter()
                .map(|e| SourceRecord {
                    line: e.line,
                    fields: e.flatten().into_iter().collect(),
                    raw: e
                        .flatten()
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join("; "),
                })
                .collect();
            Ok(Loaded {
                records,
                json: None,
                xml: Some(root),
            })
        }
    }
}

fn total_of(l: &Loaded, path: &str) -> Option<String> {
    if let Some(j) = &l.json {
        return json_at(j, path).map(scalar);
    }
    if let Some(x) = &l.xml {
        let p = xml_path(path);
        let found = match p.split_first() {
            Some((first, rest)) if *first == x.name => x.select(rest),
            _ => x.select(&p),
        };
        return found.first().map(|e| e.text.clone());
    }
    None
}

// ----- matching ------------------------------------------------------------------

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether a file matches a profile's signature.
pub fn profile_matches(bytes: &[u8], p: &InvProfile) -> bool {
    if detect_format(bytes) != p.format || p.signature.is_empty() {
        return false;
    }
    let text = decode_text(bytes);
    match p.format {
        ProfileFormat::Csv => {
            let Ok(recs) = parse_records(&text, ',') else {
                return false;
            };
            let Some(h) = recs.into_iter().nth(p.skip_rows) else {
                return false;
            };
            let header: Vec<String> = h.fields.iter().map(|f| norm(f)).collect();
            p.signature.iter().all(|s| header.contains(&norm(s)))
        }
        ProfileFormat::Json => {
            let Ok(root) = crate::safety::parse_json(&text) else {
                return false;
            };
            let mut keys: BTreeSet<String> = BTreeSet::new();
            if let Some(o) = root.as_object() {
                keys.extend(o.keys().cloned());
            }
            if let Some(first) = json_at(&root, &p.records)
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_object)
            {
                keys.extend(first.keys().cloned());
            }
            p.signature.iter().all(|s| keys.contains(s))
        }
        ProfileFormat::Xml => {
            let Ok(root) = crate::xml::parse(&text) else {
                return false;
            };
            p.signature.first().is_some_and(|r| *r == root.name)
        }
    }
}

// ----- parsing with a profile ----------------------------------------------------

/// What an action rule maps to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Mapped {
    Inv(InvAction),
    Cash,
    Ignore,
    Unsupported(String),
    SellQuantity,
    SellProceeds,
    ReinvestShares,
}

fn mapped(rule: &ActionRule) -> Mapped {
    match rule.action.as_str() {
        "buy" => Mapped::Inv(InvAction::Buy),
        "sell" => Mapped::Inv(InvAction::Sell),
        "dividend" => Mapped::Inv(InvAction::Dividend),
        "reinvest" => Mapped::Inv(InvAction::Reinvest),
        "interest" => Mapped::Inv(InvAction::Interest),
        "fee" => Mapped::Inv(InvAction::Fee),
        "shares_in" => Mapped::Inv(InvAction::SharesIn),
        "shares_out" => Mapped::Inv(InvAction::SharesOut),
        "split" => Mapped::Inv(InvAction::Split),
        "return_of_capital" => Mapped::Inv(InvAction::ReturnOfCapital),
        "merger" => Mapped::Inv(InvAction::Merger),
        "spinoff" => Mapped::Inv(InvAction::Spinoff),
        "symbol_change" => Mapped::Inv(InvAction::SymbolChange),
        "cash_in_lieu" => Mapped::Inv(InvAction::CashInLieu),
        "cash" => Mapped::Cash,
        "ignore" => Mapped::Ignore,
        "sell_quantity" => Mapped::SellQuantity,
        "sell_proceeds" => Mapped::SellProceeds,
        "reinvest_shares" => Mapped::ReinvestShares,
        _ => Mapped::Unsupported(
            rule.note
                .clone()
                .unwrap_or_else(|| "this action is not supported".into()),
        ),
    }
}

fn find_rule<'a>(rules: &'a [ActionRule], text: &str) -> Option<&'a ActionRule> {
    let t = text.trim().to_lowercase();
    rules.iter().find(|r| {
        let x = r.text.trim().to_lowercase();
        match r.match_type {
            MatchType::Equals => t == x,
            MatchType::StartsWith => t.starts_with(&x),
            MatchType::Contains => t.contains(&x),
        }
    })
}

/// The id type and kind for a symbol and description.
fn security_kind(p: &InvProfile, symbol: &str, description: &str) -> (String, String) {
    for r in &p.security_rules {
        let sym_ok = r
            .symbol_pattern
            .as_deref()
            .is_none_or(|pat| crate::safety::regex(pat).is_ok_and(|re| re.is_match(symbol)));
        let desc_ok = r
            .description_contains
            .as_deref()
            .is_none_or(|d| description.to_lowercase().contains(&d.to_lowercase()));
        if sym_ok && desc_ok {
            return (r.id_type.to_ascii_uppercase(), r.kind.clone());
        }
    }
    ("TICKER".into(), "stock".into())
}

/// The description as a security name: a trailing country code removed, ≤ 100 chars.
fn security_name(description: &str) -> String {
    let d = description.trim();
    let d = d
        .strip_suffix(" US")
        .or_else(|| d.strip_suffix("US"))
        .unwrap_or(d)
        .trim();
    without_matured(d).chars().take(100).collect()
}

/// Whether a name carries a maturity marker (`**MATURED**`).
fn is_matured(name: &str) -> bool {
    name.contains("**MATURED**")
}

/// A name without its trailing maturity marker (`XXX**MATURED**` or `**MATURED**`,
/// with surrounding spaces).
fn without_matured(name: &str) -> String {
    let n = name.trim();
    let n = n.strip_suffix("**MATURED**").map_or(n, |rest| {
        let rest = rest.trim_end();
        rest.strip_suffix("XXX").unwrap_or(rest)
    });
    n.trim().to_owned()
}

fn per_hundred(kind: &str) -> bool {
    matches!(kind, "bond" | "cd")
}

struct Staged {
    row: InvRow,
    pairing: Option<Mapped>,
    signed_amount: Option<Decimal>,
}

/// The account key from the file name, if the profile has a pattern.
pub fn account_from_file_name(p: &InvProfile, file_name: Option<&str>) -> Option<String> {
    let pat = p.account_from_file_name.as_deref()?;
    let re = crate::safety::regex(pat).ok()?;
    let caps = re.captures(file_name?)?;
    caps.get(1).map(|m| m.as_str().to_owned())
}

/// Read a broker file with a profile into a statement with investment sections.
pub fn parse_with_profile(
    bytes: &[u8],
    p: &InvProfile,
    file_name: Option<&str>,
) -> ImportResult<Statement> {
    let loaded = load(bytes, p.format, &p.records, p.skip_rows)?;
    let nf = &p.number_format;
    let field = |rec: &SourceRecord, name: &str| -> String {
        p.fields
            .get(name)
            .filter(|f| !f.trim().is_empty())
            .map(|f| rec.get(f.trim()).trim().to_owned())
            .unwrap_or_default()
    };
    let has_action_field = p.fields.get("action").is_some_and(|f| !f.trim().is_empty());
    let file_key = account_from_file_name(p, file_name);
    let mut st = Statement::default();
    // Sections by account key, in file order.
    let mut sections: Vec<(Option<String>, Section, Vec<Staged>)> = Vec::new();
    // Securities whose name so far came from a matured row (a later row may do better).
    let mut matured_names: BTreeSet<String> = BTreeSet::new();
    let mut amount_sum = Decimal::ZERO;
    for rec in &loaded.records {
        let key = if p.account_key.is_some() {
            Some(field(rec, "account")).filter(|k| !k.is_empty())
        } else {
            file_key.clone()
        };
        let pos = match sections.iter().position(|(k, _, _)| *k == key) {
            Some(i) => i,
            None => {
                sections.push((
                    key.clone(),
                    Section {
                        account_key: key.clone(),
                        account_name: Some(match &key {
                            Some(k) => format!("{} account {k}", p.name),
                            None => p.name.clone(),
                        }),
                        account_hint: key.clone(),
                        kind: SectionKind::Investment,
                        ..Section::default()
                    },
                    Vec::new(),
                ));
                sections.len() - 1
            }
        };
        let Some((_, sec, staged)) = sections.get_mut(pos) else {
            continue;
        };
        let at = |m: &str| format!("record {} (line {}): {m}", rec.line, rec.line);
        let date_text = field(rec, "date");
        let Some(date) = p.date_format.parse(&date_text) else {
            st.warnings
                .push(at(&format!("skipped: {date_text:?} is not a date")));
            continue;
        };
        let num = |name: &str| -> ImportResult<Option<Decimal>> {
            parse_money(&field(rec, name), nf).map_err(|e| ImportError::Invalid(at(&e.to_string())))
        };
        let (quantity, price, commission, fees, amount) = match (
            num("quantity"),
            num("price"),
            num("commission"),
            num("fees"),
            num("amount"),
        ) {
            (Ok(q), Ok(pr), Ok(c), Ok(f), Ok(a)) => (q, pr, c, f, a),
            (q, pr, c, f, a) => {
                for e in [q.err(), pr.err(), c.err(), f.err(), a.err()]
                    .into_iter()
                    .flatten()
                {
                    st.warnings.push(format!("skipped: {e}"));
                }
                continue;
            }
        };
        if let Some(a) = amount {
            amount_sum = amount_sum.checked_add(a).unwrap_or(amount_sum);
        }
        let action_text = if has_action_field {
            field(rec, "action")
        } else {
            field(rec, "description")
        };
        let description = field(rec, "description");
        let symbol = field(rec, "symbol").to_ascii_uppercase();
        let cusip = field(rec, "cusip").to_ascii_uppercase();
        let name = field(rec, "security_name");
        let rule = find_rule(&p.actions, &action_text);
        let what = rule.map_or_else(
            || Mapped::Unsupported(format!("Unknown action: {action_text}")),
            mapped,
        );
        if what == Mapped::Ignore {
            continue;
        }
        if what == Mapped::Cash {
            sec.rows.push(RawRow {
                date_kind: crate::DateKind::Unknown,
                cleaned: Vec::new(),
                class_hint: None,
                pending: false,
                invoices: Vec::new(),
                external_id: Some(field(rec, "external_id")).filter(|x| !x.is_empty()),
                date,
                amount: amount.unwrap_or_default(),
                payee: description.clone(),
                memo: String::new(),
                number: String::new(),
                category_hint: String::new(),
                cleared: None,
                splits: Vec::new(),
                trntype: Some(action_text.clone()),
                review: None,
                correct_fitid: None,
                correct_action: None,
                line: rec.line,
                raw: rec.raw.clone(),
            });
            continue;
        }
        // The security, unless the action is unsupported or says "no security".
        let wants_security = !matches!(what, Mapped::Unsupported(_))
            && rule.and_then(|r| r.with_security) != Some(false);
        let mut kind = String::from("stock");
        let security =
            if wants_security && (!symbol.is_empty() || !cusip.is_empty() || !name.is_empty()) {
                let (id_type, k) = security_kind(p, &symbol, &description);
                kind = k;
                let (key, cusip_v, ticker) = if !cusip.is_empty() {
                    (
                        format!("CUSIP:{cusip}"),
                        Some(cusip.clone()),
                        Some(symbol.clone()).filter(|s| !s.is_empty()),
                    )
                } else if !symbol.is_empty() && id_type == "CUSIP" {
                    (format!("CUSIP:{symbol}"), Some(symbol.clone()), None)
                } else if !symbol.is_empty() {
                    (format!("TICKER:{symbol}"), None, Some(symbol.clone()))
                } else {
                    (format!("NAME:{name}"), None, None)
                };
                let matured_now = if name.is_empty() {
                    is_matured(&description)
                } else {
                    is_matured(&name)
                };
                let shown_now = if name.is_empty() {
                    security_name(&description)
                } else {
                    without_matured(&name)
                };
                if let Some(existing) = sec.securities.iter_mut().find(|s| s.key == key) {
                    // A later, more telling row upgrades the kind and the name.
                    if kind == "cd" && existing.kind.as_deref() == Some("bond") {
                        existing.kind = Some(kind.clone());
                    }
                    if matured_names.contains(&key) && !matured_now && !shown_now.is_empty() {
                        existing.name = Some(shown_now.clone());
                        matured_names.remove(&key);
                    }
                }
                if !sec.securities.iter().any(|s| s.key == key) {
                    if matured_now {
                        matured_names.insert(key.clone());
                    }
                    let shown = shown_now.clone();
                    sec.securities.push(SourceSecurity {
                        key: key.clone(),
                        cusip: cusip_v,
                        ticker,
                        name: Some(shown).filter(|n| !n.is_empty()),
                        kind: Some(kind.clone()),
                        price: None,
                        price_date: None,
                        figi: None,
                    });
                }
                Some(key)
            } else {
                None
            };
        let cost = commission
            .unwrap_or_default()
            .abs()
            .checked_add(fees.unwrap_or_default().abs())
            .unwrap_or_default();
        let mut row = InvRow {
            cleaned: Vec::new(),
            external_id: Some(field(rec, "external_id")).filter(|x| !x.is_empty()),
            date,
            action: InvAction::Unsupported,
            security,
            quantity: quantity.map(|q| q.abs()),
            price: price.map(|x| x.abs()),
            commission: cost,
            amount: amount.map(|a| a.abs()),
            kind: rule.and_then(|r| r.kind.clone()),
            ratio: None,
            basis: None,
            acquired: None,
            expense_account: None,
            memo: description.clone(),
            reversal_of: None,
            unsupported: None,
            needs_review: false,
            notes: Vec::new(),
            line: rec.line,
            raw: rec.raw.clone(),
        };
        if let Some(r) = rule.filter(|r| r.unverified) {
            row.notes
                .push(format!("unverified action text: {}", r.text));
        }
        let mut pairing = None;
        match &what {
            Mapped::Inv(a) => {
                row.action = *a;
                if *a == InvAction::Split {
                    row.needs_review = true;
                    row.notes.push("enter the split ratio".into());
                }
                let terms = match a {
                    InvAction::Merger => {
                        Some("enter the merger terms (new security, ratio, cash, tax treatment)")
                    }
                    InvAction::Spinoff => Some(
                        "enter the spin-off terms (new security, shares per parent share, basis percentage)",
                    ),
                    InvAction::SymbolChange => Some("enter the new symbol"),
                    InvAction::CashInLieu => Some("check the fractional quantity and cash"),
                    _ => None,
                };
                if let Some(t) = terms {
                    row.needs_review = true;
                    row.notes.push(t.into());
                }
                if *a == InvAction::SharesIn && row.basis.is_none() {
                    row.needs_review = true;
                    row.notes
                        .push("the transfer has no cost basis: enter it".into());
                }
            }
            Mapped::Unsupported(why) => {
                row.action = InvAction::Unsupported;
                row.unsupported = Some(why.clone());
            }
            m @ (Mapped::SellQuantity | Mapped::SellProceeds | Mapped::ReinvestShares) => {
                pairing = Some(m.clone());
            }
            Mapped::Cash | Mapped::Ignore => {}
        }
        if per_hundred(&kind) {
            row.notes.push("priced per 100 of face value".into());
        }
        staged.push(Staged {
            row,
            pairing,
            signed_amount: amount,
        });
    }
    // Pair rows (same date and security) and finish the sections.
    for (_, mut sec, staged) in sections {
        let kinds: BTreeMap<String, String> = sec
            .securities
            .iter()
            .map(|s| (s.key.clone(), s.kind.clone().unwrap_or_default()))
            .collect();
        let mut used = vec![false; staged.len()];
        let mut out: Vec<InvRow> = Vec::new();
        for i in 0..staged.len() {
            if used.get(i).copied().unwrap_or(true) {
                continue;
            }
            let Some(s) = staged.get(i) else { continue };
            let partner = |want: &dyn Fn(&Staged) -> bool| {
                staged.iter().enumerate().position(|(j, o)| {
                    j != i
                        && !used.get(j).copied().unwrap_or(true)
                        && o.row.date == s.row.date
                        && o.row.security == s.row.security
                        && want(o)
                })
            };
            let kind = s
                .row
                .security
                .as_ref()
                .and_then(|k| kinds.get(k))
                .cloned()
                .unwrap_or_default();
            match &s.pairing {
                Some(Mapped::SellQuantity | Mapped::SellProceeds) => {
                    let is_qty = s.pairing == Some(Mapped::SellQuantity);
                    let want_other = if is_qty {
                        Mapped::SellProceeds
                    } else {
                        Mapped::SellQuantity
                    };
                    let j = partner(&|o: &Staged| o.pairing.as_ref() == Some(&want_other));
                    if let Some(u) = used.get_mut(i) {
                        *u = true;
                    }
                    let mut row = s.row.clone();
                    row.action = InvAction::Sell;
                    match j.and_then(|j| staged.get(j).map(|o| (j, o))) {
                        Some((j, o)) => {
                            if let Some(u) = used.get_mut(j) {
                                *u = true;
                            }
                            let (qr, ar) = if is_qty { (s, o) } else { (o, s) };
                            row.quantity = qr.row.quantity;
                            row.amount = ar.row.amount;
                            row.raw = format!("{}\n{}", qr.row.raw, ar.row.raw);
                            row.line = qr.row.line.min(ar.row.line);
                            row.price = match (row.quantity, row.amount) {
                                (Some(q), Some(a)) if !q.is_zero() => {
                                    let base = if per_hundred(&kind) {
                                        a.checked_mul(Decimal::ONE_HUNDRED).unwrap_or(a)
                                    } else {
                                        a
                                    };
                                    base.checked_div(q).map(|p| p.round_dp(6).normalize())
                                }
                                _ => None,
                            };
                        }
                        None => {
                            row.needs_review = true;
                            if is_qty {
                                row.action = InvAction::SharesOut;
                                row.notes
                                    .push("Position change without the maturity cash".into());
                            } else {
                                row.notes
                                    .push("Maturity cash without the position change".into());
                            }
                        }
                    }
                    out.push(row);
                }
                Some(Mapped::ReinvestShares) => {
                    let j = partner(&|o: &Staged| {
                        o.pairing.is_none() && o.row.action == InvAction::Dividend
                    });
                    if let Some(u) = used.get_mut(i) {
                        *u = true;
                    }
                    let mut row = s.row.clone();
                    match j.and_then(|j| staged.get(j).map(|o| (j, o))) {
                        Some((j, o)) => {
                            if let Some(u) = used.get_mut(j) {
                                *u = true;
                            }
                            row.action = InvAction::Reinvest;
                            row.amount = o.row.amount;
                            row.kind = o.row.kind.clone().or(Some("ordinary".into()));
                            row.raw = format!("{}\n{}", o.row.raw, s.row.raw);
                            row.line = o.row.line.min(s.row.line);
                            row.notes.extend(o.row.notes.iter().cloned());
                        }
                        None => {
                            row.action = InvAction::Buy;
                            row.needs_review = true;
                            row.notes
                                .push("reinvested shares without the dividend: check it".into());
                        }
                    }
                    out.push(row);
                }
                _ => {
                    if let Some(u) = used.get_mut(i) {
                        *u = true;
                    }
                    let mut row = s.row.clone();
                    let _ = s.signed_amount;
                    if row.action == InvAction::Dividend {
                        // A reinvested dividend paired later is consumed above.
                        row.kind = row.kind.or(Some("ordinary".into()));
                    }
                    out.push(row);
                }
            }
        }
        // Rows in file order.
        out.sort_by_key(|r| r.line);
        sec.inv_rows = out;
        st.sections.push(sec);
    }
    // Positions.
    if let Some(ps) = &p.positions
        && let Ok(l) = load(bytes, p.format, &ps.records, p.skip_rows)
    {
        for rec in &l.records {
            let f = |n: &str| {
                ps.fields
                    .get(n)
                    .map(|k| rec.get(k).trim().to_owned())
                    .unwrap_or_default()
            };
            let sym = f("symbol").to_ascii_uppercase();
            let Ok(Some(q)) = parse_money(&f("quantity"), nf) else {
                continue;
            };
            if let Some(sec) = st.sections.first_mut() {
                let (id_type, _) = security_kind(p, &sym, &f("description"));
                let key = if id_type == "CUSIP" {
                    format!("CUSIP:{sym}")
                } else {
                    format!("TICKER:{sym}")
                };
                sec.positions.push(Position {
                    security: key,
                    quantity: q,
                    price: parse_money(&f("price"), nf).ok().flatten(),
                    market_value: parse_money(&f("market_value"), nf).ok().flatten(),
                    as_of: p.date_format.parse(&f("date")),
                });
            }
        }
    }
    // File total.
    if let Some(tf) = &p.total_field
        && let Some(total) = total_of(&loaded, tf)
    {
        match parse_money(&total, nf) {
            Ok(Some(t)) if t != amount_sum => st.warnings.push(format!(
                "the file's total {} differs from the sum of its amounts {}",
                t, amount_sum
            )),
            Ok(_) => {}
            Err(_) => st
                .warnings
                .push(format!("the file's total {total:?} is not a number")),
        }
    }
    if st.sections.is_empty() {
        st.sections.push(Section {
            account_key: file_key,
            account_name: Some(p.name.clone()),
            kind: SectionKind::Investment,
            ..Section::default()
        });
    }
    Ok(st)
}

// ----- suggest -------------------------------------------------------------------

/// What `inv_profile.suggest` proposes for a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    pub format: ProfileFormat,
    pub records: String,
    /// The first record's field paths and values.
    pub sample: Vec<(String, String)>,
    /// Every field path seen in records.
    pub keys: Vec<String>,
    pub fields: BTreeMap<String, String>,
    pub date_format: Option<DateFormat>,
    /// Distinct action texts (from the action field, else the description).
    pub action_texts: Vec<String>,
    /// A profile ready to edit (name blank, no action rules yet).
    pub profile: InvProfile,
}

const GUESSES: [(&str, &[&str]); 12] = [
    (
        "date",
        &[
            "date",
            "tradedate",
            "transactiondate",
            "settledate",
            "rundate",
        ],
    ),
    (
        "action",
        &[
            "action",
            "activity",
            "type",
            "transactiontype",
            "actiontype",
        ],
    ),
    ("symbol", &["symbol", "ticker"]),
    ("cusip", &["cusip"]),
    ("security_name", &["securityname", "security", "name"]),
    ("quantity", &["quantity", "shares", "units", "qty"]),
    ("price", &["price", "unitprice", "shareprice"]),
    ("commission", &["commission", "commissions"]),
    (
        "fees",
        &[
            "fees",
            "feescomm",
            "feesandcommission",
            "feesandcommissions",
            "fee",
        ],
    ),
    ("amount", &["amount", "netamount", "total", "value"]),
    ("description", &["description", "memo", "details"]),
    (
        "external_id",
        &["id", "transactionid", "tradeid", "reference"],
    ),
];

/// The path of the first array of objects in a JSON document (breadth first).
fn find_json_records(root: &Value) -> Option<String> {
    if root
        .as_array()
        .is_some_and(|a| a.first().is_some_and(Value::is_object))
    {
        return Some("[*]".into());
    }
    let mut queue: Vec<(String, &Value)> = vec![(String::new(), root)];
    let mut i = 0;
    while let Some((path, v)) = queue.get(i).cloned() {
        i += 1;
        if let Some(o) = v.as_object() {
            for (k, x) in o {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                if x.as_array()
                    .is_some_and(|a| a.first().is_some_and(Value::is_object))
                {
                    return Some(p);
                }
                if x.is_object() {
                    queue.push((p, x));
                }
            }
        }
        if i > 10_000 {
            break;
        }
    }
    None
}

/// The path of the most repeated element (with children) in an XML document.
fn find_xml_records(root: &crate::xml::Element) -> Option<String> {
    fn walk(e: &crate::xml::Element, path: &str, best: &mut Option<(usize, String)>) {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for c in e.children.iter().filter(|c| !c.children.is_empty()) {
            *counts.entry(c.name.as_str()).or_default() += 1;
        }
        for (name, n) in counts {
            if n > best.as_ref().map_or(0, |b| b.0) {
                *best = Some((n, format!("{path}/{name}")));
            }
        }
        for c in &e.children {
            walk(c, &format!("{path}/{}", c.name), best);
        }
    }
    let mut best: Option<(usize, String)> = None;
    walk(root, &root.name, &mut best);
    best.map(|b| b.1)
}

/// Propose a profile for a file: format, records, field mappings and action texts.
pub fn suggest(bytes: &[u8], file_name: Option<&str>) -> ImportResult<Suggestion> {
    let format = detect_format(bytes);
    let text = decode_text(bytes);
    let records = match format {
        ProfileFormat::Csv => String::new(),
        ProfileFormat::Json => {
            let root: Value = crate::safety::parse_json(&text)
                .map_err(|e| ImportError::Invalid(format!("invalid JSON: {e}")))?;
            find_json_records(&root)
                .ok_or_else(|| ImportError::Invalid("no list of records found".into()))?
        }
        ProfileFormat::Xml => {
            let root = crate::xml::parse(&text)?;
            find_xml_records(&root)
                .ok_or_else(|| ImportError::Invalid("no repeating record found".into()))?
        }
    };
    let loaded = load(bytes, format, &records, 0)?;
    let mut keys: Vec<String> = Vec::new();
    for r in &loaded.records {
        for k in r.fields.keys() {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
    }
    if format == ProfileFormat::Csv {
        // Keep the header order.
        if let Ok(recs) = parse_records(&text, ',')
            && let Some(h) = recs.first()
        {
            keys = h.fields.iter().map(|f| f.trim().to_owned()).collect();
        }
    }
    let mut fields = BTreeMap::new();
    for (neutral, names) in GUESSES {
        let hit = keys.iter().find(|k| {
            let base = k.rsplit(['/', '.']).next().unwrap_or(k);
            let n = norm(base);
            names.contains(&n.as_str())
        });
        if let Some(k) = hit
            && !fields.values().any(|v| v == k)
        {
            fields.insert(neutral.to_owned(), k.clone());
        }
    }
    let date_format = fields.get("date").and_then(|f| {
        let values: Vec<&str> = loaded
            .records
            .iter()
            .map(|r| r.get(f))
            .filter(|v| !v.trim().is_empty())
            .take(20)
            .collect();
        DateFormat::ALL
            .iter()
            .copied()
            .find(|df| !values.is_empty() && values.iter().all(|v| df.parse(v).is_some()))
    });
    let action_field = fields.get("action").or_else(|| fields.get("description"));
    let mut action_texts: Vec<String> = Vec::new();
    if let Some(f) = action_field {
        for r in &loaded.records {
            let t = r.get(f).trim().to_owned();
            if !t.is_empty() && !action_texts.contains(&t) {
                action_texts.push(t);
            }
        }
    }
    let signature = match format {
        ProfileFormat::Csv => keys.clone(),
        ProfileFormat::Json => {
            let first = records.split('.').next().unwrap_or("").to_owned();
            if records == "[*]" || first.is_empty() {
                keys.iter().take(4).cloned().collect()
            } else {
                vec![first]
            }
        }
        ProfileFormat::Xml => vec![records.split('/').next().unwrap_or("").to_owned()],
    };
    let sample = loaded
        .records
        .first()
        .map(|r| {
            r.fields
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    let _ = file_name;
    Ok(Suggestion {
        format,
        records: records.clone(),
        sample,
        keys,
        fields: fields.clone(),
        date_format,
        action_texts,
        profile: InvProfile {
            name: String::new(),
            format,
            builtin: false,
            signature,
            records,
            skip_rows: 0,
            fields,
            date_format: date_format.unwrap_or(DateFormat::MmDdYyyy),
            number_format: NumberFormat::default(),
            actions: Vec::new(),
            signs: true,
            account_key: None,
            account_from_file_name: None,
            security_rules: Vec::new(),
            total_field: None,
            positions: None,
        },
    })
}

/// Check a profile's own consistency (patterns compile, actions known).
pub fn validate_profile(p: &InvProfile) -> ImportResult<()> {
    if p.name.trim().is_empty() || p.name.chars().count() > 100 {
        return Err(ImportError::Invalid(
            "a profile name is 1-100 characters".into(),
        ));
    }
    if let Some(pat) = &p.account_from_file_name {
        let re = crate::safety::regex(pat)
            .map_err(|e| ImportError::Invalid(format!("account_from_file_name: {e}")))?;
        if re.captures_len() < 2 {
            return Err(ImportError::Invalid(
                "account_from_file_name needs one capture group".into(),
            ));
        }
    }
    for r in &p.security_rules {
        if let Some(pat) = &r.symbol_pattern {
            crate::safety::regex(pat)
                .map_err(|e| ImportError::Invalid(format!("security rule pattern: {e}")))?;
        }
    }
    const KNOWN: [&str; 20] = [
        "merger",
        "spinoff",
        "symbol_change",
        "cash_in_lieu",
        "buy",
        "sell",
        "dividend",
        "reinvest",
        "interest",
        "fee",
        "shares_in",
        "shares_out",
        "split",
        "return_of_capital",
        "cash",
        "ignore",
        "unsupported",
        "sell_quantity",
        "sell_proceeds",
        "reinvest_shares",
    ];
    for r in &p.actions {
        if !KNOWN.contains(&r.action.as_str()) {
            return Err(ImportError::Invalid(format!(
                "unknown action {:?} for {:?}",
                r.action, r.text
            )));
        }
    }
    if !p.fields.get("date").is_some_and(|f| !f.trim().is_empty()) {
        return Err(ImportError::Invalid("map the date field".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn at20_money_parsing() {
        let nf = NumberFormat::default();
        let p = |s: &str| parse_money(s, &nf).unwrap().unwrap().to_string();
        assert_eq!(p("-$1,065.66"), "-1065.66");
        assert_eq!(p("$-1,065.66"), "-1065.66");
        assert_eq!(p("-$1065.66"), "-1065.66");
        assert_eq!(p("40,000"), "40000");
        assert_eq!(p("(12.50)"), "-12.50");
        assert_eq!(parse_money("", &nf).unwrap(), None);
        assert!(parse_money("abc", &nf).is_err());
    }

    #[test]
    fn builtins_load() {
        let b = builtin_profiles();
        assert_eq!(b.len(), 3);
        for p in &b {
            validate_profile(p).unwrap();
            assert!(p.builtin);
        }
    }
}
