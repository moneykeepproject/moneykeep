//! MoneyKeep Definition files (`.mkdef`, definition-files.md): TOML 1.0 documents
//! describing import layouts, categories, rules and securities. This module is pure:
//! it parses and validates bytes into items, and writes items back as TOML. Nothing in
//! a definition is ever executed; the only pattern language is the linear-time regex
//! engine with size limits.

use std::collections::{BTreeMap, BTreeSet};

use mk_core::brand;
use mk_core::text::{is_bidi, is_control, is_invisible, mixed_script, nfc};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::csv::CsvProfile;
use crate::invprofile::{InvProfile, validate_profile};
use crate::rules::{Action, Condition, ConditionOp, validate_rule};

/// Largest definition file (1 MiB).
pub const MAX_DEFINITION_BYTES: usize = 1024 * 1024;
/// The format version this build reads and writes.
pub const FORMAT_VERSION: i64 = 1;
const MAX_DEPTH: usize = 8;
const MAX_ARRAY: usize = 1000;
const MAX_KEY: usize = 64;
const MAX_ITEMS: usize = 5000;
const MAX_ERRORS: usize = 100;

/// The item kinds of format version 1, in install order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Category,
    Security,
    BankProfile,
    InvestmentProfile,
    Rule,
    /// Document template presentation options (never the logo).
    DocumentTemplate,
    /// An OFX Direct Connect server (connectivity-brokerage.md): where a bank password
    /// would be sent, so inspect lists its host.
    OfxInstitution,
    /// A dashboard (dashboards.md): its blocks with account and dashboard references
    /// by name; never amounts or transactions.
    Dashboard,
}

impl ItemKind {
    pub const ALL: [ItemKind; 8] = [
        ItemKind::Category,
        ItemKind::Security,
        ItemKind::BankProfile,
        ItemKind::InvestmentProfile,
        ItemKind::Rule,
        ItemKind::DocumentTemplate,
        ItemKind::OfxInstitution,
        ItemKind::Dashboard,
    ];

    /// The TOML table name (`[[rule]]`).
    pub fn table(self) -> &'static str {
        match self {
            ItemKind::Category => "category",
            ItemKind::Security => "security",
            ItemKind::BankProfile => "bank_profile",
            ItemKind::InvestmentProfile => "investment_profile",
            ItemKind::Rule => "rule",
            ItemKind::DocumentTemplate => "document_template",
            ItemKind::OfxInstitution => "ofx_institution",
            ItemKind::Dashboard => "dashboard",
        }
    }

    pub fn from_table(t: &str) -> Option<Self> {
        ItemKind::ALL.into_iter().find(|k| k.table() == t)
    }

    /// Plain words for summaries: (singular, plural).
    pub fn words(self) -> (&'static str, &'static str) {
        match self {
            ItemKind::Category => ("category", "categories"),
            ItemKind::Security => ("security", "securities"),
            ItemKind::BankProfile => ("bank profile", "bank profiles"),
            ItemKind::InvestmentProfile => ("broker profile", "broker profiles"),
            ItemKind::Rule => ("rule", "rules"),
            ItemKind::DocumentTemplate => ("document template", "document templates"),
            ItemKind::OfxInstitution => ("bank connection address", "bank connection addresses"),
            ItemKind::Dashboard => ("dashboard", "dashboards"),
        }
    }

    fn limit(self) -> usize {
        match self {
            ItemKind::Category | ItemKind::Rule => 2000,
            _ => 500,
        }
    }

    /// Keys an item of this kind may have (besides `key` and `x_` extensions).
    fn known(self) -> &'static [&'static str] {
        match self {
            ItemKind::Category => &["path", "type", "description", "code", "tax_line"],
            ItemKind::Security => &["symbol", "name", "kind", "basis_method", "cusip"],
            ItemKind::BankProfile => &[
                "name",
                "delimiter",
                "skip_rows",
                "has_header",
                "columns",
                "date_format",
                "amount_mode",
                "negate",
                "decimal_comma",
                "debit_values",
                "skip_if",
            ],
            ItemKind::InvestmentProfile => &[
                "name",
                "format",
                "signature",
                "records",
                "skip_rows",
                "fields",
                "date_format",
                "number_format",
                "actions",
                "signs",
                "account_key",
                "account_from_file_name",
                "security_rules",
                "total_field",
                "positions",
            ],
            ItemKind::Rule => &[
                "name",
                "priority",
                "enabled",
                "conditions",
                "actions",
                "stop",
            ],
            ItemKind::DocumentTemplate => &TEMPLATE_KEYS,
            ItemKind::OfxInstitution => &OFX_KEYS,
            ItemKind::Dashboard => &["name", "icon", "blocks"],
        }
    }

    /// Fixed key order for written files.
    fn order(self) -> &'static [&'static str] {
        match self {
            ItemKind::Category => &["key", "path", "type", "description", "code", "tax_line"],
            ItemKind::Security => &["key", "symbol", "name", "kind", "basis_method", "cusip"],
            ItemKind::BankProfile => &[
                "key",
                "name",
                "delimiter",
                "has_header",
                "skip_rows",
                "date_format",
                "amount_mode",
                "negate",
                "decimal_comma",
                "debit_values",
                "columns",
                "skip_if",
            ],
            ItemKind::InvestmentProfile => &[
                "key",
                "name",
                "format",
                "signature",
                "records",
                "skip_rows",
                "date_format",
                "signs",
                "account_key",
                "account_from_file_name",
                "total_field",
                "number_format",
                "fields",
                "positions",
                "security_rules",
                "actions",
            ],
            ItemKind::Rule => &[
                "key",
                "name",
                "priority",
                "enabled",
                "stop",
                "conditions",
                "actions",
            ],
            ItemKind::OfxInstitution => &[
                "key",
                "name",
                "url",
                "org",
                "fid",
                "broker_id",
                "ofx_version",
                "app_id",
                "app_ver",
                "notes",
            ],
            ItemKind::Dashboard => &["key", "name", "icon", "blocks"],
            ItemKind::DocumentTemplate => &[
                "key",
                "name",
                "accent_color",
                "font",
                "show",
                "payment_instructions",
                "footer",
                "title_overrides",
            ],
        }
    }
}

/// Fields of an `[[ofx_institution]]`.
const OFX_KEYS: [&str; 9] = [
    "name",
    "url",
    "org",
    "fid",
    "broker_id",
    "ofx_version",
    "app_id",
    "app_ver",
    "notes",
];

/// The canonical form of an OFX institution profile.
pub fn canonical_ofx_institution(i: &crate::ofx_dc::Institution) -> Value {
    serde_json::to_value(i).unwrap_or(Value::Null)
}

/// Fields of a `[[document_template]]` (the logo is never part of a definition).
const TEMPLATE_KEYS: [&str; 7] = [
    "name",
    "accent_color",
    "font",
    "show",
    "payment_instructions",
    "footer",
    "title_overrides",
];

const TEMPLATE_SHOW: [&str; 6] = [
    "terms",
    "due_date",
    "sku",
    "quantity_rate",
    "customer_email",
    "payment_instructions",
];

const TEMPLATE_TITLES: [&str; 5] = [
    "invoice",
    "estimate",
    "credit_memo",
    "statement",
    "purchase_order",
];

/// The canonical form of a document template item (sorted, only known fields).
pub fn canonical_template(m: &Map<String, Value>) -> Value {
    let mut out = Map::new();
    for k in TEMPLATE_KEYS {
        if let Some(v) = m.get(k).filter(|v| !v.is_null()) {
            out.insert(k.to_owned(), v.clone());
        }
    }
    Value::Object(out)
}

fn check_template(m: &Map<String, Value>, path: &str, p: &mut Problems) -> Option<String> {
    let name = get_str(m, "name").unwrap_or_default().trim().to_owned();
    if name.is_empty() || name.chars().count() > 100 {
        p.error(format!("{path}.name: required, 1-100 characters"));
    }
    if let Some(c) = m.get("accent_color") {
        let ok = c.as_str().is_some_and(|c| {
            c.len() == 7 && c.starts_with('#') && c.chars().skip(1).all(|x| x.is_ascii_hexdigit())
        });
        if !ok {
            p.error(format!("{path}.accent_color: #RRGGBB"));
        }
    }
    if let Some(f) = m.get("font")
        && !matches!(f.as_str(), Some("helvetica" | "times"))
    {
        p.error(format!("{path}.font: \"helvetica\" or \"times\""));
    }
    for (k, max) in [("payment_instructions", 1000), ("footer", 500)] {
        if let Some(v) = m.get(k)
            && v.as_str().is_none_or(|s| s.chars().count() > max)
        {
            p.error(format!("{path}.{k}: text of at most {max} characters"));
        }
    }
    if let Some(s) = m.get("show") {
        match s.as_object() {
            Some(o) => {
                for (k, v) in o {
                    if !TEMPLATE_SHOW.contains(&k.as_str()) || !v.is_boolean() {
                        p.error(format!(
                            "{path}.show.{k}: one of {} set to true or false",
                            TEMPLATE_SHOW.join(", ")
                        ));
                    }
                }
            }
            None => p.error(format!("{path}.show: a table of true/false options")),
        }
    }
    if let Some(t) = m.get("title_overrides") {
        match t.as_object() {
            Some(o) => {
                for (k, v) in o {
                    if !TEMPLATE_TITLES.contains(&k.as_str())
                        || v.as_str().is_none_or(|s| s.chars().count() > 40)
                    {
                        p.error(format!("{path}.title_overrides.{k}: one of {} with a title of at most 40 characters", TEMPLATE_TITLES.join(", ")));
                    }
                }
            }
            None => p.error(format!("{path}.title_overrides: a table of titles")),
        }
    }
    if m.contains_key("logo") {
        p.error(format!(
            "{path}.logo: logos are not shared through definition files"
        ));
    }
    Some(name)
}

/// The canonical form of a dashboard item: name, icon and blocks in reading order
/// (each with type, title, x, y, w, h and settings).
pub fn canonical_dashboard(m: &Map<String, Value>) -> Value {
    let mut out = Map::new();
    if let Some(n) = get_str(m, "name") {
        out.insert("name".into(), Value::String(n.trim().to_owned()));
    }
    if let Some(i) = get_str(m, "icon").filter(|i| !i.trim().is_empty()) {
        out.insert("icon".into(), Value::String(i.trim().to_owned()));
    }
    let mut blocks: Vec<Value> = m
        .get("blocks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|b| {
            let mut o = Map::new();
            for k in ["type", "title", "x", "y", "w", "h", "settings"] {
                if let Some(v) = b.get(k).filter(|v| !v.is_null()) {
                    o.insert(k.to_owned(), v.clone());
                }
            }
            o.entry("settings")
                .or_insert_with(|| Value::Object(Map::new()));
            Value::Object(o)
        })
        .collect();
    let at = |b: &Value, k: &str| b.get(k).and_then(Value::as_u64).unwrap_or(0);
    blocks.sort_by_key(|b| (at(b, "y"), at(b, "x")));
    out.insert("blocks".into(), Value::Array(blocks));
    Value::Object(out)
}

fn check_dashboard(m: &Map<String, Value>, path: &str, p: &mut Problems) -> Option<String> {
    use mk_core::dashboard as grid;
    let name = get_str(m, "name").unwrap_or_default().trim().to_owned();
    if name.is_empty() || name.chars().count() > 60 {
        p.error(format!("{path}.name: required, 1-60 characters"));
    }
    if let Some(i) = m.get("icon") {
        let ok = i.as_str().is_some_and(|i| {
            i.len() <= 30 && i.chars().all(|c| c.is_ascii_lowercase() || c == '-')
        });
        if !ok {
            p.error(format!("{path}.icon: a symbol name such as \"church\""));
        }
    }
    let blocks = match m.get("blocks") {
        None => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        Some(_) => {
            p.error(format!(
                "{path}.blocks: must be an array of tables ([[dashboard.blocks]])"
            ));
            return None;
        }
    };
    if blocks.len() > grid::MAX_BLOCKS {
        p.error(format!(
            "{path}.blocks: {} blocks; the limit is {}",
            blocks.len(),
            grid::MAX_BLOCKS
        ));
        return None;
    }
    let mut cells = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        let bp = format!("{path}.blocks[{i}]");
        let Some(o) = b.as_object() else {
            p.error(format!("{bp}: must be a table"));
            continue;
        };
        let kind = get_str(o, "type").unwrap_or_default();
        let kind_ok = !kind.is_empty()
            && kind.len() <= 40
            && kind
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !kind_ok {
            p.error(format!(
                "{bp}.type: required, a block type such as \"note\""
            ));
        } else if grid::block_type(kind).is_none() {
            p.warn(format!(
                "{bp}: block type `{kind}` is not known to this version; it is kept and shows that it needs a newer MoneyKeep"
            ));
        }
        if let Some(t) = o.get("title")
            && t.as_str().is_none_or(|t| t.chars().count() > 60)
        {
            p.error(format!("{bp}.title: text of at most 60 characters"));
        }
        if o.get("settings").is_some_and(|s| !s.is_object()) {
            p.error(format!("{bp}.settings: must be a table"));
        }
        for k in o.keys() {
            if !["type", "title", "x", "y", "w", "h", "settings"].contains(&k.as_str())
                && !k.starts_with("x_")
            {
                p.warn(format!("ignored: unknown key `{k}` in {bp}"));
            }
        }
        let mut num = |k: &str| -> u32 {
            match o
                .get(k)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
            {
                Some(n) => n,
                None => {
                    p.error(format!("{bp}.{k}: required, a whole number"));
                    0
                }
            }
        };
        let cell = grid::Cell::new(num("x"), num("y"), num("w"), num("h"));
        cells.push((cell, grid::min_size(kind)));
    }
    if let Err(e) = grid::validate(&cells) {
        let what = match e {
            grid::LayoutError::TooMany(n) => {
                format!("{n} blocks; the limit is {}", grid::MAX_BLOCKS)
            }
            grid::LayoutError::OutOfGrid(i) => format!("block {i} is outside the 12-column grid"),
            grid::LayoutError::TooSmall(i) => format!("block {i} is smaller than its type allows"),
            grid::LayoutError::TooLarge(i) => format!("block {i} is larger than 12 x 12 cells"),
            grid::LayoutError::Overlap(i, j) => format!("blocks {i} and {j} overlap"),
        };
        p.error(format!("{path}.blocks: {what}"));
    }
    Some(name)
}

/// The package header.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Header {
    pub format: i64,
    pub id: String,
    pub version: String,
    pub name: String,
    pub description: Option<String>,
    pub authors: Vec<String>,
    pub license: Option<String>,
    pub homepage: Option<String>,
    pub created: Option<String>,
    pub requires: Vec<String>,
    pub tags: Vec<String>,
}

/// One item: its kind, key, display name and canonical fields (without `key`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DefItem {
    pub kind: ItemKind,
    pub key: String,
    /// Profile or security name, category path, rule name.
    pub name: String,
    pub fields: Value,
    /// Rules: priority relative within the package.
    pub priority: Option<i64>,
}

/// A parsed, valid definition file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Definition {
    pub header: Header,
    pub items: Vec<DefItem>,
    /// `x_` extension tables, kept for re-export.
    pub extensions: Map<String, Value>,
    pub warnings: Vec<String>,
    pub sha256: String,
}

/// SHA-256 of a file, hex.
pub fn file_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

struct Problems {
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl Problems {
    fn error(&mut self, e: impl Into<String>) {
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(e.into());
        }
    }
    fn warn(&mut self, w: impl Into<String>) {
        let w = w.into();
        if !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }
}

// ----- lexical pre-scan ----------------------------------------------------------------

/// Deepest bracket nesting (`[` / `{`) outside strings and comments, found without
/// parsing, so a hostile file is refused before the TOML parser recurses.
fn bracket_depth(text: &str) -> usize {
    let b = text.as_bytes();
    let (mut i, mut depth, mut max) = (0usize, 0usize, 0usize);
    let starts = |i: usize, pat: &[u8]| b.get(i..i + pat.len()) == Some(pat);
    while let Some(&c) = b.get(i) {
        if starts(i, b"\"\"\"") || starts(i, b"'''") {
            let close: &[u8] = if c == b'"' { b"\"\"\"" } else { b"'''" };
            i += 3;
            while i < b.len() && !starts(i, close) {
                if c == b'"' && b.get(i) == Some(&b'\\') {
                    i += 1;
                }
                i += 1;
            }
            i += 3;
            continue;
        }
        match c {
            b'"' | b'\'' => {
                i += 1;
                while let Some(&d) = b.get(i) {
                    if d == b'\n' {
                        break;
                    }
                    if c == b'"' && d == b'\\' {
                        i += 2;
                        continue;
                    }
                    if d == c {
                        break;
                    }
                    i += 1;
                }
            }
            b'#' => {
                while b.get(i).is_some_and(|d| *d != b'\n') {
                    i += 1;
                }
            }
            b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    max
}

// ----- TOML to JSON with structure limits ------------------------------------------------

fn to_json(v: toml::Value, path: &str, depth: usize, p: &mut Problems) -> Value {
    if depth > MAX_DEPTH {
        p.error(format!("{path}: nesting deeper than {MAX_DEPTH} levels"));
        return Value::Null;
    }
    match v {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::from(i),
        toml::Value::Float(_) => {
            p.error(format!(
                "{path}: write decimal numbers as strings (\"12.50\"); floating-point numbers are not exact"
            ));
            Value::Null
        }
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => {
            if d.time.is_some() || d.offset.is_some() || d.date.is_none() {
                p.error(format!("{path}: only plain dates (YYYY-MM-DD) are allowed"));
            }
            Value::String(d.to_string())
        }
        toml::Value::Array(a) => {
            // Item lists (`[[rule]]`) are bounded by the per-kind limits instead.
            if a.len() > MAX_ARRAY && depth > 1 {
                p.error(format!(
                    "{path}: an array has {} elements; the limit is {MAX_ARRAY}",
                    a.len()
                ));
                return Value::Null;
            }
            Value::Array(
                a.into_iter()
                    .enumerate()
                    .map(|(i, x)| to_json(x, &format!("{path}[{i}]"), depth + 1, p))
                    .collect(),
            )
        }
        toml::Value::Table(t) => {
            let mut m = Map::new();
            for (k, x) in t {
                let sub = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                if k.chars().count() > MAX_KEY {
                    p.error(format!("{sub}: a key is longer than {MAX_KEY} characters"));
                    continue;
                }
                let j = to_json(x, &sub, depth + 1, p);
                m.insert(k, j);
            }
            Value::Object(m)
        }
    }
}

// ----- text rules ---------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextClass {
    /// Descriptions and notes: tab and line feed allowed, invisible characters allowed.
    Long(usize),
    /// Names, keys, paths, symbols and match values.
    Strict { limit: usize, confusable: bool },
}

fn check_text(s: &mut String, path: &str, class: TextClass, p: &mut Problems) {
    *s = nfc(s);
    let multiline = matches!(class, TextClass::Long(_));
    if let Some(c) = s
        .chars()
        .find(|c| is_control(*c) && !(multiline && matches!(c, '\t' | '\n' | '\r')))
    {
        p.error(format!(
            "{path}: contains the control character U+{:04X}",
            u32::from(c)
        ));
    }
    if let Some(c) = s.chars().find(|c| is_bidi(*c)) {
        p.error(format!(
            "{path}: contains the bidirectional formatting character U+{:04X}",
            u32::from(c)
        ));
    }
    let limit = match class {
        TextClass::Long(l) => l,
        TextClass::Strict { limit, confusable } => {
            if let Some(c) = s.chars().find(|c| is_invisible(*c)) {
                p.error(format!(
                    "{path}: contains the invisible character U+{:04X}",
                    u32::from(c)
                ));
            }
            if confusable && mixed_script(s) {
                p.warn(format!(
                    "{path}: mixes Latin with Greek or Cyrillic letters that may look alike ({s:?})"
                ));
            }
            limit
        }
    };
    let n = s.chars().count();
    if n > limit {
        p.error(format!("{path}: {n} characters; the limit is {limit}"));
    }
}

/// Apply the text rules to every string inside an item value. `key` is the name of
/// the field holding `v`; `op` is the sibling condition operator, if any.
fn check_strings(v: &mut Value, path: &str, key: &str, op: Option<&str>, p: &mut Problems) {
    match v {
        Value::String(s) => {
            let class = match key {
                "description" => TextClass::Long(4000),
                "notes" => TextClass::Long(1000),
                // A dashboard note block's text (dashboards.md).
                "text" if path.contains(".blocks[") => TextClass::Long(2000),
                "name" => TextClass::Strict {
                    limit: 100,
                    confusable: true,
                },
                "path" | "symbol" => TextClass::Strict {
                    limit: 1000,
                    confusable: true,
                },
                "value" => TextClass::Strict {
                    limit: if op == Some("regex") { 200 } else { 500 },
                    confusable: true,
                },
                "text" => TextClass::Strict {
                    limit: 500,
                    confusable: true,
                },
                _ => TextClass::Strict {
                    limit: 1000,
                    confusable: false,
                },
            };
            check_text(s, path, class, p);
        }
        Value::Array(a) => {
            for (i, x) in a.iter_mut().enumerate() {
                check_strings(x, &format!("{path}[{i}]"), key, op, p);
            }
        }
        Value::Object(m) => {
            let op = m.get("op").and_then(Value::as_str).map(str::to_owned);
            for (k, x) in m.iter_mut() {
                check_strings(x, &format!("{path}.{k}"), k, op.as_deref(), p);
            }
        }
        _ => {}
    }
}

// ----- header -------------------------------------------------------------------------

const HEADER_KEYS: [&str; 11] = [
    brand::DEFINITION_HEADER,
    "id",
    "version",
    "name",
    "description",
    "authors",
    "license",
    "homepage",
    "created",
    "requires",
    "tags",
];

/// Feature names this reader implements (none in format version 1).
const FEATURES: [&str; 0] = [];

fn valid_id(id: &str) -> bool {
    let n = id.chars().count();
    (3..=100).contains(&n)
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id.contains('.')
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
}

/// SemVer 2.0.0 (semver.org): MAJOR.MINOR.PATCH with optional pre-release and build.
fn valid_semver(v: &str) -> bool {
    let (core_pre, build) = match v.split_once('+') {
        Some((a, b)) => (a, Some(b)),
        None => (v, None),
    };
    let (core, pre) = match core_pre.split_once('-') {
        Some((a, b)) => (a, Some(b)),
        None => (core_pre, None),
    };
    let numeric = |s: &str| {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
    };
    let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| numeric(p))
        && pre.is_none_or(|p| {
            p.split('.')
                .all(|x| ident(x) && (!x.chars().all(|c| c.is_ascii_digit()) || numeric(x)))
        })
        && build.is_none_or(|b| b.split('.').all(ident))
}

/// SPDX license expression syntax: ids joined by AND / OR / WITH, with parentheses.
fn valid_spdx(expr: &str) -> bool {
    let spaced = expr.replace('(', " ( ").replace(')', " ) ");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }
    let mut depth = 0i32;
    let mut expect_id = true;
    for t in tokens {
        match t {
            "(" if expect_id => depth += 1,
            ")" if !expect_id => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            "AND" | "OR" | "WITH" if !expect_id => expect_id = true,
            id if expect_id => {
                let core = id.strip_suffix('+').unwrap_or(id);
                if core.is_empty()
                    || !core
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
                {
                    return false;
                }
                expect_id = false;
            }
            _ => return false,
        }
    }
    depth == 0 && !expect_id
}

/// Homepage: display-only text that looks like an https URL.
fn valid_homepage(h: &str) -> bool {
    h.len() <= 200
        && h.strip_prefix("https://")
            .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
        && !h
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | '\\'))
}

fn string_list(
    v: Option<&Value>,
    path: &str,
    max: usize,
    each: usize,
    p: &mut Problems,
) -> Vec<String> {
    let Some(v) = v else { return Vec::new() };
    let Some(a) = v.as_array() else {
        p.error(format!("{path}: must be a list of strings"));
        return Vec::new();
    };
    if a.len() > max {
        p.error(format!("{path}: at most {max} entries"));
    }
    let mut out = Vec::new();
    for (i, x) in a.iter().enumerate() {
        match x.as_str() {
            Some(s) => {
                let mut s = s.to_owned();
                check_text(
                    &mut s,
                    &format!("{path}[{i}]"),
                    TextClass::Strict {
                        limit: each,
                        confusable: false,
                    },
                    p,
                );
                out.push(s);
            }
            None => p.error(format!("{path}[{i}]: must be a string")),
        }
    }
    out
}

fn header_string(
    root: &Map<String, Value>,
    key: &str,
    class: TextClass,
    required: bool,
    p: &mut Problems,
) -> Option<String> {
    match root.get(key) {
        None => {
            if required {
                p.error(format!("{key}: required"));
            }
            None
        }
        Some(Value::String(s)) => {
            let mut s = s.clone();
            check_text(&mut s, key, class, p);
            Some(s)
        }
        Some(_) => {
            p.error(format!("{key}: must be a string"));
            None
        }
    }
}

fn parse_header(root: &Map<String, Value>, p: &mut Problems) -> Header {
    let key = brand::DEFINITION_HEADER;
    let format = match root.get(key) {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(_) => {
            p.error(format!("{key}: must be an integer"));
            0
        }
        None => {
            p.error(format!(
                "{key}: required (this is not a {} definition file)",
                brand::PRODUCT
            ));
            0
        }
    };
    if format > FORMAT_VERSION {
        p.error(format!(
            "{key}: this file was made for a newer {} (format {format}; this version reads format {FORMAT_VERSION})",
            brand::PRODUCT
        ));
    } else if format < 1 && root.contains_key(key) {
        p.error(format!("{key}: must be 1 or more"));
    }
    let strict = |limit| TextClass::Strict {
        limit,
        confusable: false,
    };
    let id = header_string(root, "id", strict(100), true, p).unwrap_or_default();
    if !id.is_empty() && !valid_id(&id) {
        p.error("id: 3-100 characters of lowercase letters, digits, '.', '-' or '_', starting with a letter and containing a '.'");
    }
    let version = header_string(root, "version", strict(100), true, p).unwrap_or_default();
    if !version.is_empty() && !valid_semver(&version) {
        p.error(format!(
            "version: {version:?} is not a SemVer version (e.g. 1.2.0)"
        ));
    }
    let name = header_string(
        root,
        "name",
        TextClass::Strict {
            limit: 100,
            confusable: true,
        },
        true,
        p,
    )
    .unwrap_or_default();
    if name.trim().is_empty() && root.contains_key("name") {
        p.error("name: must not be empty");
    }
    let description = header_string(root, "description", TextClass::Long(4000), false, p);
    let license = header_string(root, "license", strict(200), false, p);
    if let Some(l) = &license
        && !valid_spdx(l)
    {
        p.error(format!(
            "license: {l:?} is not an SPDX license id or expression"
        ));
    }
    let homepage = header_string(root, "homepage", strict(200), false, p);
    if let Some(h) = &homepage
        && !valid_homepage(h)
    {
        p.error("homepage: must be an https:// address of at most 200 characters (it is shown as text only)");
    }
    let created = header_string(root, "created", strict(10), false, p);
    if let Some(c) = &created
        && c.parse::<mk_core::Date>().is_err()
    {
        p.error("created: must be a date (YYYY-MM-DD)");
    }
    let authors = string_list(root.get("authors"), "authors", 10, 100, p);
    let tags = string_list(root.get("tags"), "tags", 20, 30, p);
    let requires = string_list(root.get("requires"), "requires", 50, 100, p);
    for r in &requires {
        if !FEATURES.contains(&r.as_str()) {
            p.error(format!(
                "requires: this file needs the feature {r:?}, which this version of MoneyKeep does not have"
            ));
        }
    }
    Header {
        format,
        id,
        version,
        name,
        description,
        authors,
        license,
        homepage,
        created,
        requires,
        tags,
    }
}

// ----- items --------------------------------------------------------------------------

fn valid_key(k: &str) -> bool {
    let n = k.chars().count();
    (1..=64).contains(&n)
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

fn without(mut v: Value, keys: &[&str]) -> Value {
    if let Some(o) = v.as_object_mut() {
        for k in keys {
            o.remove(*k);
        }
    }
    v
}

/// A category path: 1-10 segments of 1-100 characters separated by `:`.
pub fn valid_category_path(path: &str) -> bool {
    let segs: Vec<&str> = path.split(':').collect();
    (1..=10).contains(&segs.len())
        && segs
            .iter()
            .all(|s| !s.trim().is_empty() && s.trim() == *s && s.chars().count() <= 100)
}

/// Canonical fields of an investment profile (as written and compared).
pub fn canonical_inv_profile(p: &InvProfile) -> Value {
    without(serde_json::to_value(p).unwrap_or(Value::Null), &["builtin"])
}

/// Canonical fields of a bank profile: account references are never part of it.
pub fn canonical_bank_profile(p: &CsvProfile) -> Value {
    without(serde_json::to_value(p).unwrap_or(Value::Null), &["account"])
}

/// Canonical fields of a rule (category references as paths).
pub fn canonical_rule(
    name: &str,
    enabled: bool,
    conditions: &[Condition],
    actions: &[Action],
    stop: bool,
) -> Value {
    serde_json::json!({
        "name": name.trim(),
        "enabled": enabled,
        "conditions": conditions,
        "actions": actions,
        "stop": stop,
    })
}

/// Canonical fields of a category.
pub fn canonical_category(path: &str, kind: &str, description: Option<&str>) -> Value {
    canonical_category_tax(path, kind, description, None)
}

/// Canonical fields of a category with its optional tax line (chart-presets.md).
pub fn canonical_category_tax(
    path: &str,
    kind: &str,
    description: Option<&str>,
    tax_line: Option<&str>,
) -> Value {
    let mut m = Map::new();
    m.insert("path".into(), Value::String(path.to_owned()));
    m.insert("type".into(), Value::String(kind.to_owned()));
    if let Some(d) = description.filter(|d| !d.trim().is_empty()) {
        m.insert("description".into(), Value::String(d.to_owned()));
    }
    if let Some(t) = tax_line.filter(|t| !t.trim().is_empty()) {
        m.insert("tax_line".into(), Value::String(t.trim().to_owned()));
    }
    Value::Object(m)
}

/// Canonical fields of a security.
pub fn canonical_security(
    symbol: &str,
    name: &str,
    kind: &str,
    basis_method: &str,
    cusip: Option<&str>,
) -> Value {
    let mut m = Map::new();
    m.insert("symbol".into(), Value::String(symbol.to_owned()));
    m.insert("name".into(), Value::String(name.to_owned()));
    m.insert("kind".into(), Value::String(kind.to_owned()));
    m.insert(
        "basis_method".into(),
        Value::String(basis_method.to_owned()),
    );
    if let Some(c) = cusip.filter(|c| !c.is_empty()) {
        m.insert("cusip".into(), Value::String(c.to_owned()));
    }
    Value::Object(m)
}

const SECURITY_KINDS: [&str; 7] = [
    "stock",
    "etf",
    "mutual_fund",
    "bond",
    "cd",
    "money_market",
    "other",
];
const BASIS_METHODS: [&str; 3] = ["fifo", "average", "specific"];

fn get_str<'a>(m: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    m.get(k).and_then(Value::as_str)
}

/// Validate one item's fields and return its canonical form.
fn parse_item(
    kind: ItemKind,
    mut m: Map<String, Value>,
    path: &str,
    p: &mut Problems,
) -> Option<(String, Value, Option<i64>)> {
    match kind {
        ItemKind::Dashboard => {
            let name = check_dashboard(&m, path, p)?;
            Some((name, canonical_dashboard(&m), None))
        }
        ItemKind::DocumentTemplate => {
            let name = check_template(&m, path, p)?;
            Some((name, canonical_template(&m), None))
        }
        ItemKind::OfxInstitution => {
            match serde_json::from_value::<crate::ofx_dc::Institution>(Value::Object(m)) {
                Ok(mut inst) => {
                    if let Err(e) = inst.check() {
                        p.error(format!("{path}: {e}"));
                    }
                    Some((inst.name.clone(), canonical_ofx_institution(&inst), None))
                }
                Err(e) => {
                    p.error(format!("{path}: {e}"));
                    None
                }
            }
        }
        ItemKind::Category => {
            let Some(cat_path) = get_str(&m, "path").map(str::to_owned) else {
                p.error(format!("{path}.path: required"));
                return None;
            };
            if !valid_category_path(&cat_path) {
                p.error(format!(
                    "{path}.path: {cat_path:?} is not a category path (names of 1-100 characters separated by ':')"
                ));
            }
            let t = get_str(&m, "type").unwrap_or_default().to_owned();
            if !matches!(t.as_str(), "income" | "expense") {
                p.error(format!("{path}.type: must be \"income\" or \"expense\""));
            }
            if m.get("code").is_some_and(|c| !c.is_string()) {
                p.error(format!("{path}.code: must be a string"));
            }
            let tax_line = get_str(&m, "tax_line");
            if m.get("tax_line").is_some_and(|c| !c.is_string())
                || tax_line.is_some_and(|t| {
                    let n = t.trim().chars().count();
                    n == 0 || n > 40 || t.chars().any(char::is_control)
                })
            {
                p.error(format!(
                    "{path}.tax_line: must be 1-40 characters of printable text"
                ));
            }
            let mut c = canonical_category_tax(&cat_path, &t, get_str(&m, "description"), tax_line);
            if let (Some(o), Some(code)) = (c.as_object_mut(), get_str(&m, "code")) {
                o.insert("code".into(), Value::String(code.to_owned()));
            }
            Some((cat_path, c, None))
        }
        ItemKind::Security => {
            let symbol = get_str(&m, "symbol").unwrap_or_default().trim().to_owned();
            let name = get_str(&m, "name").unwrap_or_default().trim().to_owned();
            if symbol.is_empty() || symbol.chars().count() > 20 {
                p.error(format!("{path}.symbol: required, 1-20 characters"));
            }
            if name.is_empty() {
                p.error(format!("{path}.name: required"));
            }
            let kind_s = get_str(&m, "kind").unwrap_or("stock").to_owned();
            if !SECURITY_KINDS.contains(&kind_s.as_str()) {
                p.error(format!("{path}.kind: one of {}", SECURITY_KINDS.join(", ")));
            }
            let method = get_str(&m, "basis_method").unwrap_or("fifo").to_owned();
            if !BASIS_METHODS.contains(&method.as_str()) {
                p.error(format!("{path}.basis_method: fifo, average or specific"));
            }
            let c = canonical_security(&symbol, &name, &kind_s, &method, get_str(&m, "cusip"));
            Some((symbol, c, None))
        }
        ItemKind::BankProfile => match serde_json::from_value::<CsvProfile>(Value::Object(m)) {
            Ok(prof) => {
                if let Err(e) = prof.validate() {
                    p.error(format!("{path}: {e}"));
                }
                Some((
                    prof.name.trim().to_owned(),
                    canonical_bank_profile(&prof),
                    None,
                ))
            }
            Err(e) => {
                p.error(format!("{path}: {e}"));
                None
            }
        },
        ItemKind::InvestmentProfile => {
            m.remove("builtin");
            match serde_json::from_value::<InvProfile>(Value::Object(m)) {
                Ok(mut prof) => {
                    prof.name = prof.name.trim().to_owned();
                    if let Err(e) = validate_profile(&prof) {
                        p.error(format!("{path}: {e}"));
                    }
                    Some((prof.name.clone(), canonical_inv_profile(&prof), None))
                }
                Err(e) => {
                    p.error(format!("{path}: {e}"));
                    None
                }
            }
        }
        ItemKind::Rule => {
            let name = get_str(&m, "name").unwrap_or_default().trim().to_owned();
            let priority = match m.get("priority") {
                None => None,
                Some(v) => match v.as_i64() {
                    Some(n) => Some(n),
                    None => {
                        p.error(format!("{path}.priority: must be an integer"));
                        None
                    }
                },
            };
            let flag = |m: &Map<String, Value>, k: &str, p: &mut Problems| match m.get(k) {
                None => true,
                Some(Value::Bool(b)) => *b,
                Some(_) => {
                    p.error(format!("{path}.{k}: must be true or false"));
                    true
                }
            };
            let enabled = flag(&m, "enabled", p);
            let stop = flag(&m, "stop", p);
            let conditions: Vec<Condition> = match serde_json::from_value(
                m.remove("conditions").unwrap_or(Value::Array(Vec::new())),
            ) {
                Ok(c) => c,
                Err(e) => {
                    p.error(format!("{path}.conditions: {e}"));
                    return None;
                }
            };
            let actions: Vec<Action> = match serde_json::from_value(
                m.remove("actions").unwrap_or(Value::Array(Vec::new())),
            ) {
                Ok(a) => a,
                Err(e) => {
                    p.error(format!("{path}.actions: {e}"));
                    return None;
                }
            };
            if let Err(e) = validate_rule(&name, &conditions, &actions) {
                p.error(format!("{path}: {e}"));
            }
            for c in &conditions {
                if c.op == ConditionOp::Regex && c.value.as_str().is_none() {
                    p.error(format!("{path}: a regex condition needs a string"));
                }
            }
            for a in &actions {
                let paths: Vec<&str> = match a {
                    Action::SetAccount { account } => vec![account.as_str()],
                    Action::Split { shares } => shares.iter().map(|s| s.account.as_str()).collect(),
                    _ => Vec::new(),
                };
                for ap in paths {
                    if !valid_category_path(ap) {
                        p.error(format!("{path}.actions: {ap:?} is not a category path"));
                    }
                }
            }
            let c = canonical_rule(&name, enabled, &conditions, &actions, stop);
            Some((name, c, priority))
        }
    }
}

/// Parse and validate a definition file. On failure, returns every error found (up to
/// 100), each naming its key path.
pub fn parse_definition(bytes: &[u8]) -> Result<Definition, Vec<String>> {
    if bytes.len() > MAX_DEFINITION_BYTES {
        return Err(vec![format!(
            "the file is {} bytes; the limit is 1 MiB ({MAX_DEFINITION_BYTES} bytes)",
            bytes.len()
        )]);
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|e| vec![format!("the file is not valid UTF-8 ({e})")])?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let depth = bracket_depth(text);
    if depth > MAX_DEPTH {
        return Err(vec![format!(
            "nesting deeper than {MAX_DEPTH} levels ({depth} found)"
        )]);
    }
    let table: toml::Table = toml::from_str(text)
        .map_err(|e| vec![format!("not valid TOML: {}", e.to_string().trim())])?;
    let mut p = Problems {
        errors: Vec::new(),
        warnings: Vec::new(),
    };
    let root = match to_json(toml::Value::Table(table), "", 0, &mut p) {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let header = parse_header(&root, &mut p);
    let mut items = Vec::new();
    let mut extensions = Map::new();
    let mut counts: BTreeMap<ItemKind, usize> = BTreeMap::new();
    for (top, v) in &root {
        if HEADER_KEYS.contains(&top.as_str()) {
            continue;
        }
        if top.starts_with("x_") {
            extensions.insert(top.clone(), v.clone());
            continue;
        }
        if top == "signature" {
            p.warn("signature: not checked (signatures are not part of format version 1)");
            continue;
        }
        let Some(kind) = ItemKind::from_table(top) else {
            let is_kind = v
                .as_array()
                .is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_object));
            if is_kind {
                p.warn(format!("ignored: unknown kind `{top}`"));
            } else {
                p.warn(format!("ignored: unknown key `{top}`"));
            }
            continue;
        };
        let Some(list) = v.as_array() else {
            p.error(format!("{top}: must be an array of tables ([[{top}]])"));
            continue;
        };
        *counts.entry(kind).or_default() += list.len();
        let mut keys: BTreeSet<String> = BTreeSet::new();
        for (i, entry) in list.iter().enumerate() {
            let Some(obj) = entry.as_object() else {
                p.error(format!("{top}[{i}]: must be a table"));
                continue;
            };
            let key = obj
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let path = if valid_key(&key) {
                format!("{top}[{key}]")
            } else {
                format!("{top}[{i}]")
            };
            if !valid_key(&key) {
                p.error(format!(
                    "{path}.key: required, 1-64 letters, digits, '-' or '_'"
                ));
            } else if !keys.insert(key.clone()) {
                p.error(format!("{path}.key: {key:?} is used twice"));
            }
            let mut fields = Map::new();
            for (k, x) in obj {
                if k == "key" || k.starts_with("x_") {
                    continue;
                }
                if !kind.known().contains(&k.as_str()) {
                    let what = if kind == ItemKind::BankProfile && k == "account" {
                        format!(
                            "ignored: `account` in bank profile `{key}` (definitions never name your accounts)"
                        )
                    } else {
                        format!("ignored: unknown key `{k}` in {} `{key}`", kind.words().0)
                    };
                    p.warn(what);
                    continue;
                }
                fields.insert(k.clone(), x.clone());
            }
            let mut fv = Value::Object(fields);
            check_strings(&mut fv, &path, "", None, &mut p);
            let Value::Object(fields) = fv else { continue };
            if let Some((name, canonical, priority)) = parse_item(kind, fields, &path, &mut p) {
                items.push(DefItem {
                    kind,
                    key,
                    name,
                    fields: canonical,
                    priority,
                });
            }
        }
    }
    let total: usize = counts.values().sum();
    if total > MAX_ITEMS {
        p.error(format!(
            "the file has {total} items; the limit is {MAX_ITEMS}"
        ));
    }
    for (kind, n) in &counts {
        if *n > kind.limit() {
            p.error(format!(
                "{}: {n} items; the limit is {} {}",
                kind.table(),
                kind.limit(),
                kind.words().1
            ));
        }
    }
    if !p.errors.is_empty() {
        return Err(p.errors);
    }
    items.sort_by(|a, b| (a.kind, &a.key).cmp(&(b.kind, &b.key)));
    Ok(Definition {
        header,
        items,
        extensions,
        warnings: p.warnings,
        sha256: file_sha256(bytes),
    })
}

// ----- writing ------------------------------------------------------------------------

fn bare_key(k: &str) -> bool {
    !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

fn toml_key(k: &str) -> String {
    if bare_key(k) {
        k.to_owned()
    } else {
        toml_string(k)
    }
}

fn escape(s: &str, multiline: bool) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' if multiline => out.push('\n'),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if is_control(c) => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out
}

fn toml_string(s: &str) -> String {
    if s.contains('\n') {
        format!("\"\"\"\n{}\"\"\"", escape(s, true))
    } else {
        format!("\"{}\"", escape(s, false))
    }
}

fn inline(v: &Value) -> Option<String> {
    Some(match v {
        Value::Null => return None,
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => toml_string(s),
        Value::Array(a) => {
            let parts: Vec<String> = a.iter().filter_map(inline).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(m) => {
            let parts: Vec<String> = m
                .iter()
                .filter_map(|(k, x)| inline(x).map(|s| format!("{} = {s}", toml_key(k))))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
    })
}

fn ordered_keys(kind: ItemKind, m: &Map<String, Value>) -> Vec<String> {
    let mut keys: Vec<String> = kind
        .order()
        .iter()
        .filter(|k| m.contains_key(**k))
        .map(|k| (*k).to_owned())
        .collect();
    let mut rest: Vec<String> = m.keys().filter(|k| !keys.contains(k)).cloned().collect();
    rest.sort();
    keys.extend(rest);
    keys
}

fn is_table_array(v: &Value) -> bool {
    v.as_array()
        .is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_object))
}

/// Write a definition as TOML: a generated comment header, header keys, then items
/// grouped by kind in install order, sorted by key, keys in a fixed order.
pub fn write_definition(
    header: &Header,
    items: &[DefItem],
    extensions: &Map<String, Value>,
) -> String {
    let mut out = String::new();
    out.push_str("# MoneyKeep Definition file. It holds data only (import layouts, rules,\n");
    out.push_str(
        "# categories, securities) and can be edited by hand; lines starting with # are\n",
    );
    out.push_str(
        "# comments. Install it from Settings > Definitions; every install is reviewed.\n",
    );
    out.push_str(&format!(
        "{} = {FORMAT_VERSION}\n",
        brand::DEFINITION_HEADER
    ));
    out.push_str(&format!("id = {}\n", toml_string(&header.id)));
    out.push_str(&format!("version = {}\n", toml_string(&header.version)));
    out.push_str(&format!("name = {}\n", toml_string(&header.name)));
    if let Some(d) = &header.description {
        out.push_str(&format!("description = {}\n", toml_string(d)));
    }
    let list = |l: &[String]| {
        format!(
            "[{}]",
            l.iter()
                .map(|s| toml_string(s))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if !header.authors.is_empty() {
        out.push_str(&format!("authors = {}\n", list(&header.authors)));
    }
    if let Some(l) = &header.license {
        out.push_str(&format!("license = {}\n", toml_string(l)));
    }
    if let Some(h) = &header.homepage {
        out.push_str(&format!("homepage = {}\n", toml_string(h)));
    }
    if let Some(c) = &header.created {
        out.push_str(&format!("created = {c}\n"));
    }
    if !header.tags.is_empty() {
        out.push_str(&format!("tags = {}\n", list(&header.tags)));
    }
    let mut sorted: Vec<&DefItem> = items.iter().collect();
    sorted.sort_by(|a, b| (a.kind, &a.key).cmp(&(b.kind, &b.key)));
    for item in sorted {
        let table = item.kind.table();
        out.push_str(&format!("\n[[{table}]]\n"));
        let mut m = item.fields.as_object().cloned().unwrap_or_default();
        m.insert("key".into(), Value::String(item.key.clone()));
        if let Some(pr) = item.priority {
            m.insert("priority".into(), Value::from(pr));
        }
        let keys = ordered_keys(item.kind, &m);
        let mut subtables = Vec::new();
        for k in &keys {
            let Some(v) = m.get(k) else { continue };
            if is_table_array(v) {
                subtables.push(k.clone());
            } else if let Some(s) = inline(v) {
                out.push_str(&format!("{} = {s}\n", toml_key(k)));
            }
        }
        for k in subtables {
            for entry in m
                .get(&k)
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[])
            {
                out.push_str(&format!("[[{table}.{}]]\n", toml_key(&k)));
                if let Some(o) = entry.as_object() {
                    for (ek, ev) in o {
                        if let Some(s) = inline(ev) {
                            out.push_str(&format!("{} = {s}\n", toml_key(ek)));
                        }
                    }
                }
            }
        }
    }
    for (k, v) in extensions {
        if let Some(o) = v.as_object() {
            out.push_str(&format!("\n[{}]\n", toml_key(k)));
            for (ek, ev) in o {
                if let Some(s) = inline(ev) {
                    out.push_str(&format!("{} = {s}\n", toml_key(ek)));
                }
            }
        }
    }
    out
}

/// A plain item key made from a name (`Coffee shops` → `coffee-shops`).
pub fn key_from_name(name: &str) -> String {
    let mut k = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            k.push(c.to_ascii_lowercase());
        } else if !k.ends_with('-') && !k.is_empty() {
            k.push('-');
        }
    }
    let k: String = k.trim_end_matches('-').chars().take(64).collect();
    if k.is_empty() { "item".into() } else { k }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    const GOOD: &str = r#"
moneykeep_definition = 1
id = "org.example.test"
version = "1.0.0"
name = "Test"

[[category]]
key = "food"
path = "Expenses:Food"
type = "expense"

[[rule]]
key = "coffee"
name = "Coffee"
[[rule.conditions]]
field = "payee"
op = "contains"
value = "coffee"
[[rule.actions]]
action = "set_account"
account = "Expenses:Food"
"#;

    #[test]
    fn parses_and_round_trips() {
        let d = parse_definition(GOOD.as_bytes()).unwrap();
        assert_eq!(d.items.len(), 2);
        let written = write_definition(&d.header, &d.items, &d.extensions);
        let again = parse_definition(written.as_bytes()).unwrap();
        assert_eq!(again.items, d.items);
        assert_eq!(again.header, d.header);
    }

    #[test]
    fn header_rules() {
        assert!(valid_semver("1.2.0") && valid_semver("1.0.0-rc.1+b.2"));
        assert!(!valid_semver("1.2") && !valid_semver("01.2.3"));
        assert!(valid_spdx("CC0-1.0") && valid_spdx("(MIT OR Apache-2.0) AND CC-BY-4.0"));
        assert!(!valid_spdx("MIT OR") && !valid_spdx("<script>"));
        assert!(valid_homepage("https://example.org/x"));
        assert!(!valid_homepage("javascript:alert(1)") && !valid_homepage("http://x.org"));
        assert!(valid_id("org.example.a-b") && !valid_id("Org.x") && !valid_id("nodot"));
    }

    #[test]
    fn bracket_depth_ignores_strings_and_comments() {
        assert_eq!(bracket_depth("a = \"[[[[\" # [[[[\nb = [[1]]"), 2);
        assert_eq!(bracket_depth("a = '''[[[[[['''\n"), 0);
    }
}
