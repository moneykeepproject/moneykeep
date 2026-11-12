//! SnapTrade with personal keys (connectivity-brokerage.md part B): request signing,
//! JSON read with numbers kept as decimal text (never a binary float), accounts,
//! activities, positions and balances, and the activity mapping to investment rows
//! (import-investments.md). Pure: the caller sends the requests.
//!
//! From the vendor documentation:
//! - Personal keys: <https://docs.snaptrade.com/docs/personal-vs-commercial> — requests
//!   are signed with the personal consumer key; `userId`/`userSecret` are omitted and
//!   the register-user endpoint is never called.
//! - Signing: <https://docs.snaptrade.com/docs/request-signatures> — `clientId` and
//!   `timestamp` (Unix seconds) in the query; HMAC-SHA256 with the consumer key over
//!   the canonical JSON (keys sorted at every level, no whitespace, UTF-8) of
//!   `{"content": body or null, "path": <the path as requested>, "query": raw query}`;
//!   Base64 in the `Signature` header. The page (read 2026-11-08) says endpoints are
//!   served from the host root and `/api/v1` is a legacy prefix kept for existing
//!   integrations, and "never sign one path form and send the other": MoneyKeep
//!   requests root paths and signs exactly that path (connectivity-brokerage.md
//!   revision 2).
//! - API reference index: <https://docs.snaptrade.com/reference> (read 2026-11-08;
//!   server `https://api.snaptrade.com`, Account Information operations).
//! - Accounts: <https://docs.snaptrade.com/reference/Account%20Information/AccountInformation_listAllUserAccounts>
//!   (read 2026-11-08: `GET https://api.snaptrade.com/accounts/all`, not deprecated; `{"results": [...]}` with `kind`, `id`, `display_name`,
//!   `masked_account_number`, `sync_status.transactions.initial_sync_completed` /
//!   `last_successful_sync`, `net_value.currency`).
//! - Activities: <https://docs.snaptrade.com/reference/Account%20Information/AccountInformation_getAccountActivities>
//!   (read 2026-11-08: `GET https://api.snaptrade.com/accounts/{accountId}/activities`,
//!   "not deprecated"; the `Deprecation` header is sent only for the legacy `/api/v1`
//!   prefix; `?startDate&endDate&offset&limit` (≤ 1000),
//!   `{"data": [...], "pagination": {"offset","limit","total"}}`; activity `id`,
//!   `symbol` (`symbol`, `raw_symbol`, `description`, `figi_code`, `currency.code`),
//!   `option_symbol`, `price`, `units`, `amount`, `currency.code`, `type`,
//!   `description`, `trade_date`, `fee`; the id can change when a transaction is
//!   deleted and re-added).
//! - Positions: <https://docs.snaptrade.com/reference/Account%20Information/AccountInformation_getAllAccountPositions>
//!   ("List all account positions", read 2026-11-08: `GET
//!   https://api.snaptrade.com/accounts/{accountId}/positions/all`, answer
//!   `{"results": [...], "data_freshness": {"as_of"}}`; each result has `instrument`
//!   (`kind` — stock, option, crypto, future, future_option, etf, mutualfund, cef,
//!   adr, cfd, tokenized_asset, cash, bond, other, preferred_stock — `symbol`,
//!   `raw_symbol`, `description`, `currency` (an ISO code string),
//!   `figi_instrument.figi_code`), `units`, `price`, `cost_basis` ("Book price or
//!   average purchase price for the position"; per share: the example has price
//!   123.45 and cost_basis 118.2), `currency`, `cash_equivalent`, optional
//!   `tax_lots` (ignored)). The older per-account `GET /accounts/{accountId}/positions`
//!   is deprecated and answers 410 for newer keys (connectivity-brokerage.md
//!   revision 2); answers in its shape (`symbol.symbol`, `average_purchase_price`)
//!   are still read.
//! - Balances: <https://docs.snaptrade.com/reference/Account%20Information/AccountInformation_getUserAccountBalance>
//!   (read 2026-11-08: `GET https://api.snaptrade.com/accounts/{accountId}/balances`,
//!   an array of `{"currency": {"id","code","name"}, "cash", "buying_power"}`; money
//!   market funds are included in `cash` and also listed in positions with
//!   `cash_equivalent` true).

use std::collections::BTreeMap;

use base64::Engine;
use hmac::{Hmac, Mac};
use mk_core::{Date, Decimal};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::statement::{InvAction, InvRow, RawRow, SourceSecurity};
use crate::{ImportError, ImportResult};

/// The only host requests go to.
pub const HOST: &str = "api.snaptrade.com";
/// The legacy path prefix, no longer used: requests go to the host root (see the
/// signing notes above). Kept for the fake server, which still answers it with a
/// `Deprecation` header as SnapTrade does.
pub const LEGACY_PREFIX: &str = "/api/v1";
/// Largest answer accepted (connectivity-brokerage.md R2 limits).
pub const MAX_RESPONSE_BYTES: usize = 25 * 1024 * 1024;
/// Activities per page.
pub const PAGE: u64 = 1000;

/// An amount at cents (`0.30`, not `0.3`).
fn cents(d: Decimal) -> Decimal {
    let mut x = d.round_dp(2);
    x.rescale(2);
    x
}

fn invalid(m: impl Into<String>) -> ImportError {
    ImportError::Invalid(m.into())
}

fn clean(s: &str, max: usize) -> String {
    mk_core::text::clean(s, max).text.trim().to_owned()
}

// ----- signing ------------------------------------------------------------------------

/// Canonical JSON: object keys sorted at every level, no whitespace.
pub fn canonical(v: &Value) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}

fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let sorted: BTreeMap<&String, &Value> = m.iter().collect();
            out.push('{');
            for (i, (k, x)) in sorted.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write_canonical(x, out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// The `Signature` header value of a request: `path` is the path exactly as requested
/// (from the host root), `query` the raw query string as sent (with `clientId` and
/// `timestamp`), `content` the JSON body.
pub fn signature(consumer_key: &str, path: &str, query: &str, content: Option<&Value>) -> String {
    let obj = serde_json::json!({
        "content": content.cloned().unwrap_or(Value::Null),
        "path": path,
        "query": query,
    });
    let text = canonical(&obj);
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(consumer_key.as_bytes()) else {
        return String::new();
    };
    mac.update(text.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

// ----- numbers as text ------------------------------------------------------------------

/// JSON text with every number token turned into a string token, so amounts are read
/// into decimals from their text and never through a binary float.
pub fn numbers_as_text(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 64);
    let mut i = 0;
    let mut in_str = false;
    let mut start = 0;
    while let Some(&c) = b.get(i) {
        if in_str {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            in_str = true;
            i += 1;
            continue;
        }
        if c == b'-' || c.is_ascii_digit() {
            let s = i;
            while let Some(&d) = b.get(i) {
                if d.is_ascii_digit() || matches!(d, b'-' | b'+' | b'.' | b'e' | b'E') {
                    i += 1;
                } else {
                    break;
                }
            }
            out.push_str(text.get(start..s).unwrap_or_default());
            out.push('"');
            out.push_str(text.get(s..i).unwrap_or_default());
            out.push('"');
            start = i;
            continue;
        }
        i += 1;
    }
    out.push_str(text.get(start..).unwrap_or_default());
    out
}

/// Parse an answer with the input-safety limits, numbers kept as text.
pub fn parse(body: &[u8]) -> ImportResult<Value> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(invalid("the answer is larger than 25 MB"));
    }
    let text = std::str::from_utf8(body).map_err(|_| invalid("the answer is not UTF-8 text"))?;
    crate::safety::parse_json(&numbers_as_text(text))
}

/// A decimal from a number kept as text (`0.1`, `1e-7`, `-2200`).
pub fn decimal_of(v: &Value) -> Option<Decimal> {
    let t = match v {
        Value::String(s) => s.trim().to_owned(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if t.is_empty() || t.len() > 60 {
        return None;
    }
    if t.contains(['e', 'E']) {
        Decimal::from_scientific(&t).ok().map(|d| d.normalize())
    } else {
        t.parse::<Decimal>().ok()
    }
}

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(x)) => x.clone(),
        _ => String::new(),
    }
}

fn opt(v: &Value, k: &str) -> Option<String> {
    Some(clean(&s(v, k), 200)).filter(|x| !x.is_empty())
}

fn date_of(t: &str) -> Option<Date> {
    t.get(..10).and_then(|d| d.parse::<Date>().ok())
}

// ----- accounts -----------------------------------------------------------------------

/// A remote account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    /// `investment`, `deposit`, `line_of_credit`.
    pub kind: String,
    pub name: String,
    /// The number masked to its last 4 characters (`****8443`).
    pub masked: String,
    pub institution: String,
    pub connection_id: String,
    pub initial_sync_completed: bool,
    pub last_successful_sync: Option<String>,
    pub currency: Option<String>,
    /// SnapTrade reports the brokerage connection broken.
    pub disabled: bool,
}

/// The last 4 characters of an account number, masked.
pub fn mask(number: &str) -> String {
    let digits: Vec<char> = number
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let tail: String = digits.iter().skip(digits.len().saturating_sub(4)).collect();
    if tail.is_empty() {
        String::new()
    } else {
        format!("****{tail}")
    }
}

/// The list-all-accounts answer (`{"results": [...]}`, or a bare array).
pub fn accounts(body: &[u8]) -> ImportResult<Vec<Account>> {
    let v = parse(body)?;
    let list = match &v {
        Value::Array(a) => a.clone(),
        _ => v
            .get("results")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| invalid("the accounts answer has no results"))?,
    };
    let mut out = Vec::new();
    for a in &list {
        let id = s(a, "id");
        if id.is_empty() {
            continue;
        }
        let number = s(a, "masked_account_number");
        let number = if number.is_empty() {
            s(a, "number")
        } else {
            number
        };
        let sync = a.get("sync_status").and_then(|x| x.get("transactions"));
        let kind = s(a, "kind");
        let category = s(a, "account_category").to_ascii_lowercase();
        out.push(Account {
            kind: if !kind.is_empty() {
                kind
            } else if category.is_empty() || category == "investment" {
                "investment".into()
            } else {
                category
            },
            name: opt(a, "display_name")
                .or_else(|| opt(a, "name"))
                .unwrap_or_else(|| "Account".into()),
            masked: mask(&number),
            institution: opt(a, "institution_name").unwrap_or_default(),
            connection_id: opt(a, "connection_id")
                .or_else(|| opt(a, "brokerage_authorization"))
                .unwrap_or_default(),
            initial_sync_completed: sync
                .and_then(|x| x.get("initial_sync_completed"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            last_successful_sync: sync.and_then(|x| opt(x, "last_successful_sync")),
            currency: a
                .get("net_value")
                .or_else(|| a.get("balance").and_then(|b| b.get("total")))
                .and_then(|n| opt(n, "currency")),
            disabled: a.get("disabled").and_then(Value::as_bool).unwrap_or(false)
                || s(a, "status").eq_ignore_ascii_case("disabled"),
            id: clean(&id, 64),
        });
    }
    Ok(out)
}

// ----- activities ---------------------------------------------------------------------

/// A security named by SnapTrade.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Sym {
    pub ticker: Option<String>,
    pub name: Option<String>,
    pub figi: Option<String>,
    pub currency: Option<String>,
}

impl Sym {
    /// The source security key: ticker, else FIGI, else name.
    pub fn key(&self) -> Option<String> {
        if let Some(t) = &self.ticker {
            return Some(format!("TICKER:{t}"));
        }
        if let Some(f) = &self.figi {
            return Some(format!("FIGI:{f}"));
        }
        self.name.as_ref().map(|n| format!("NAME:{n}"))
    }

    fn source(&self) -> Option<SourceSecurity> {
        Some(SourceSecurity {
            key: self.key()?,
            cusip: None,
            ticker: self.ticker.clone(),
            name: self.name.clone(),
            kind: Some("stock".into()),
            price: None,
            price_date: None,
            figi: self.figi.clone(),
        })
    }
}

fn sym_of(v: Option<&Value>) -> Option<Sym> {
    let mut v = v.filter(|x| x.is_object())?;
    // The deprecated positions answer nests the security one level deeper
    // (`symbol.symbol`).
    if let Some(inner) = v.get("symbol").filter(|x| x.is_object()) {
        v = inner;
    }
    let ticker = opt(v, "raw_symbol")
        .or_else(|| opt(v, "symbol"))
        .map(|t| t.to_ascii_uppercase());
    let sym = Sym {
        ticker,
        name: opt(v, "description"),
        figi: opt(v, "figi_code")
            .or_else(|| v.get("figi_instrument").and_then(|f| opt(f, "figi_code"))),
        // An object (`{"code"}`) in activities, an ISO code string in an instrument.
        currency: v
            .get("currency")
            .and_then(|c| opt(c, "code"))
            .or_else(|| opt(v, "currency"))
            .map(|c| c.to_ascii_uppercase()),
    };
    (sym.ticker.is_some() || sym.figi.is_some() || sym.name.is_some()).then_some(sym)
}

/// One activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub id: String,
    /// `type`, upper case.
    pub kind: String,
    pub symbol: Option<Sym>,
    pub option: bool,
    pub price: Option<Decimal>,
    pub units: Option<Decimal>,
    pub amount: Option<Decimal>,
    pub currency: Option<String>,
    pub date: Option<Date>,
    pub fee: Decimal,
    pub description: String,
}

impl Activity {
    /// The content of the activity without its id (a re-sent activity with a new id
    /// has the same hash).
    pub fn content_hash(&self) -> String {
        let text = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            self.kind,
            self.date.map(|d| d.to_string()).unwrap_or_default(),
            self.symbol.as_ref().and_then(Sym::key).unwrap_or_default(),
            self.units
                .map(|x| x.normalize().to_string())
                .unwrap_or_default(),
            self.price
                .map(|x| x.normalize().to_string())
                .unwrap_or_default(),
            self.amount
                .map(|x| x.normalize().to_string())
                .unwrap_or_default(),
            self.fee.normalize(),
            self.currency.clone().unwrap_or_default(),
        );
        Sha256::digest(text.as_bytes())
            .iter()
            .take(16)
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// An activities page: the activities and `pagination.total`.
pub fn activities(body: &[u8]) -> ImportResult<(Vec<Activity>, u64)> {
    let v = parse(body)?;
    let (list, total) = match &v {
        Value::Array(a) => (a.clone(), a.len() as u64),
        _ => (
            v.get("data")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| invalid("the activities answer has no data"))?,
            v.get("pagination")
                .and_then(|p| p.get("total"))
                .and_then(decimal_of)
                .and_then(|d| u64::try_from(d.trunc().mantissa()).ok())
                .unwrap_or(0),
        ),
    };
    let mut out = Vec::new();
    for a in &list {
        let id = clean(&s(a, "id"), 100);
        if id.is_empty() {
            continue;
        }
        let num = |k: &str| a.get(k).and_then(decimal_of);
        out.push(Activity {
            id,
            kind: clean(&s(a, "type"), 40).to_ascii_uppercase(),
            symbol: sym_of(a.get("symbol")),
            option: a.get("option_symbol").is_some_and(|o| o.is_object()),
            price: num("price"),
            units: num("units"),
            amount: num("amount"),
            currency: a
                .get("currency")
                .and_then(|c| opt(c, "code"))
                .map(|c| c.to_ascii_uppercase()),
            date: date_of(&s(a, "trade_date")).or_else(|| date_of(&s(a, "settlement_date"))),
            fee: num("fee").unwrap_or_default().abs(),
            description: clean(&s(a, "description"), 200),
        });
    }
    Ok((out, total))
}

/// A holding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holding {
    pub symbol: Sym,
    pub units: Decimal,
    pub price: Option<Decimal>,
    /// Average purchase price per share (`cost_basis`).
    pub average_price: Option<Decimal>,
}

/// The instrument kinds imported as holdings (securities); other kinds are counted
/// in the "not imported" note (connectivity-brokerage.md revision 2).
const SECURITY_KINDS: [&str; 7] = [
    "stock",
    "etf",
    "mutualfund",
    "cef",
    "adr",
    "preferred_stock",
    "bond",
];

/// What the positions answer gave.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Positions {
    /// Security positions.
    pub holdings: Vec<Holding>,
    /// The value of cash-equivalent positions (`cash_equivalent`, or kind `cash`) in
    /// the book's currency; `None` when there are none.
    pub cash_equivalent: Option<Decimal>,
    /// Positions of other kinds, not imported in this version: kind → count.
    pub skipped: BTreeMap<String, usize>,
}

impl Positions {
    /// "2 option positions not imported" (one note per kind).
    pub fn notes(&self) -> Vec<String> {
        self.skipped
            .iter()
            .map(|(kind, n)| {
                let word = match kind.as_str() {
                    "future_option" => "future option",
                    "tokenized_asset" => "tokenized asset",
                    "cfd" => "CFD",
                    "" => "unknown",
                    k => k,
                };
                let s = if *n == 1 { "" } else { "s" };
                format!("{n} {word} position{s} not imported")
            })
            .collect()
    }
}

/// The list-all-positions answer (`{"results": [...]}`; an array or `{"data": [...]}`
/// is read too). `base` is the book's currency (cash-equivalent positions in another
/// currency are not counted).
pub fn positions(body: &[u8], base: &str) -> ImportResult<Positions> {
    let v = parse(body)?;
    let list = match &v {
        Value::Array(a) => a.clone(),
        _ => v
            .get("results")
            .or_else(|| v.get("data"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };
    let mut out = Positions::default();
    for p in &list {
        let instrument = p.get("instrument").filter(|x| x.is_object());
        let kind = instrument
            .map(|i| clean(&s(i, "kind"), 40).to_ascii_lowercase())
            .unwrap_or_default();
        let num = |k: &str| p.get(k).and_then(decimal_of);
        let Some(units) = num("units").or_else(|| num("fractional_units")) else {
            continue;
        };
        let cash_eq =
            p.get("cash_equivalent").and_then(Value::as_bool) == Some(true) || kind == "cash";
        if cash_eq {
            let currency = opt(p, "currency")
                .or_else(|| instrument.and_then(|i| opt(i, "currency")))
                .unwrap_or_else(|| base.to_owned());
            if currency.eq_ignore_ascii_case(base) {
                let value = num("price")
                    .and_then(|pr| pr.checked_mul(units))
                    .unwrap_or(units);
                let total = out.cash_equivalent.unwrap_or_default();
                out.cash_equivalent = Some(cents(total.checked_add(value).unwrap_or(total)));
            }
            continue;
        }
        if !kind.is_empty() && !SECURITY_KINDS.contains(&kind.as_str()) {
            *out.skipped.entry(kind).or_insert(0) += 1;
            continue;
        }
        let Some(sym) = sym_of(instrument.or_else(|| p.get("symbol"))) else {
            continue;
        };
        out.holdings.push(Holding {
            symbol: sym,
            units,
            price: num("price"),
            average_price: num("cost_basis").or_else(|| num("average_purchase_price")),
        });
    }
    Ok(out)
}

/// Cash in `currency` from the balances answer (`[{"currency": {"code"}, "cash"}]`).
pub fn cash(body: &[u8], currency: &str) -> ImportResult<Option<Decimal>> {
    let v = parse(body)?;
    let list = match &v {
        Value::Array(a) => a.clone(),
        _ => v
            .get("data")
            .or_else(|| v.get("results"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };
    Ok(list.iter().find_map(|b| {
        let code = b.get("currency").and_then(|c| opt(c, "code"))?;
        if code.eq_ignore_ascii_case(currency) {
            b.get("cash").and_then(decimal_of)
        } else {
            None
        }
    }))
}

// ----- mapping ------------------------------------------------------------------------

/// What the mapping produced for one account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mapped {
    pub securities: Vec<SourceSecurity>,
    pub inv_rows: Vec<InvRow>,
    /// Cash rows (taxes, contributions, withdrawals, transfers of cash).
    pub cash_rows: Vec<RawRow>,
    pub warnings: Vec<String>,
}

fn inv_row(a: &Activity, date: Date, action: InvAction, line: usize) -> InvRow {
    InvRow {
        external_id: Some(a.id.clone()),
        date,
        action,
        security: a.symbol.as_ref().and_then(Sym::key),
        quantity: a.units.map(|u| u.abs()),
        price: a.price,
        commission: a.fee,
        amount: a.amount.map(|x| x.abs()),
        kind: None,
        ratio: None,
        basis: None,
        acquired: None,
        expense_account: None,
        memo: a.description.clone(),
        reversal_of: None,
        unsupported: None,
        needs_review: false,
        notes: Vec::new(),
        line,
        raw: format!("{} {}", a.kind, a.description).trim().to_owned(),
        cleaned: Vec::new(),
    }
}

fn unsupported(a: &Activity, date: Date, why: String, line: usize) -> InvRow {
    let mut r = inv_row(a, date, InvAction::Unsupported, line);
    r.unsupported = Some(why);
    r
}

fn cash_row(a: &Activity, date: Date, line: usize) -> RawRow {
    RawRow {
        date_kind: crate::DateKind::Unknown,
        external_id: Some(a.id.clone()),
        date,
        amount: a.amount.unwrap_or_default(),
        payee: if a.description.is_empty() {
            a.kind.clone()
        } else {
            a.description.clone()
        },
        memo: a.kind.clone(),
        number: String::new(),
        category_hint: String::new(),
        cleared: None,
        splits: Vec::new(),
        trntype: None,
        review: None,
        correct_fitid: None,
        correct_action: None,
        line,
        raw: format!("{} {}", a.kind, a.description).trim().to_owned(),
        cleaned: Vec::new(),
        class_hint: None,
        pending: false,
        invoices: Vec::new(),
    }
}

/// Map activities to investment rows and cash rows (the table of
/// connectivity-brokerage.md part B). `held_before(key, date)` is the book's quantity
/// of a security before a date (split ratios); `holdings` give the average purchase
/// price for shares transferred in.
pub fn map(
    acts: &[Activity],
    base: &str,
    holdings: &[Holding],
    held_before: &dyn Fn(&str, Date) -> Option<Decimal>,
) -> Mapped {
    let mut m = Mapped::default();
    let mut keys: BTreeMap<String, SourceSecurity> = BTreeMap::new();
    let mut acts: Vec<&Activity> = acts.iter().collect();
    acts.sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)));
    // REI rows paired with a DIVIDEND of the same security, date and amount.
    let mut used: Vec<String> = Vec::new();
    let mut line = 0;
    for a in &acts {
        line += 1;
        if let Some(src) = a.symbol.as_ref().and_then(Sym::source) {
            keys.entry(src.key.clone()).or_insert(src);
        }
        let Some(date) = a.date else {
            m.warnings
                .push(format!("Activity {} has no date and was left out", a.id));
            continue;
        };
        if a.option || a.kind.starts_with("OPTION") {
            m.inv_rows.push(unsupported(
                a,
                date,
                "Options are not supported yet".into(),
                line,
            ));
            continue;
        }
        if let Some(c) = a
            .currency
            .as_deref()
            .filter(|c| !c.eq_ignore_ascii_case(base))
        {
            m.inv_rows.push(unsupported(
                a,
                date,
                format!("Foreign currency ({c}) is not supported yet"),
                line,
            ));
            continue;
        }
        let key = a.symbol.as_ref().and_then(Sym::key);
        match a.kind.as_str() {
            "BUY" | "SELL" => {
                let action = if a.kind == "BUY" {
                    InvAction::Buy
                } else {
                    InvAction::Sell
                };
                let mut r = inv_row(a, date, action, line);
                if r.amount.is_none() {
                    r.amount = match (a.price, a.units) {
                        (Some(p), Some(u)) => p.checked_mul(u.abs()).map(cents),
                        _ => None,
                    };
                }
                if key.is_none() {
                    r.needs_review = true;
                    r.notes.push("The trade names no security".into());
                }
                m.inv_rows.push(r);
            }
            "DIVIDEND" | "SUBSTITUTE_DIVIDEND" => {
                if used.contains(&a.id) {
                    continue;
                }
                // Reinvested: a REI of the same security, date and amount.
                let rei = acts.iter().find(|b| {
                    b.kind == "REI"
                        && !used.contains(&b.id)
                        && b.date == a.date
                        && b.symbol.as_ref().and_then(Sym::key) == key
                        && b.amount.map(|x| x.abs()) == a.amount.map(|x| x.abs())
                });
                if let Some(b) = rei {
                    used.push(b.id.clone());
                    let mut r = inv_row(b, date, InvAction::Reinvest, line);
                    r.amount = a.amount.map(|x| x.abs());
                    r.kind = Some("ordinary".into());
                    r.notes.push(format!("Dividend {} reinvested", a.id));
                    m.inv_rows.push(r);
                    continue;
                }
                let mut r = inv_row(a, date, InvAction::Dividend, line);
                r.kind = Some("ordinary".into());
                if a.kind == "SUBSTITUTE_DIVIDEND" {
                    r.memo = format!("Substitute dividend {}", r.memo).trim().to_owned();
                }
                m.inv_rows.push(r);
            }
            "REI" => {
                if used.contains(&a.id) {
                    continue;
                }
                let paired = acts.iter().any(|b| {
                    matches!(b.kind.as_str(), "DIVIDEND" | "SUBSTITUTE_DIVIDEND")
                        && b.date == a.date
                        && b.symbol.as_ref().and_then(Sym::key) == key
                        && b.amount.map(|x| x.abs()) == a.amount.map(|x| x.abs())
                });
                if paired {
                    // Written with its dividend.
                    continue;
                }
                let mut r = inv_row(a, date, InvAction::Buy, line);
                r.needs_review = true;
                r.notes
                    .push("Reinvestment without a matching dividend".into());
                m.inv_rows.push(r);
            }
            "INTEREST" => m.inv_rows.push(inv_row(a, date, InvAction::Interest, line)),
            "FEE" => m.inv_rows.push(inv_row(a, date, InvAction::Fee, line)),
            "TAX" => {
                let mut r = cash_row(a, date, line);
                r.category_hint = "Taxes withheld".into();
                r.review = Some("Taxes withheld: check the account".into());
                m.cash_rows.push(r);
            }
            "CONTRIBUTION" | "WITHDRAWAL" => m.cash_rows.push(cash_row(a, date, line)),
            "TRANSFER" | "EXTERNAL_ASSET_TRANSFER_IN" | "EXTERNAL_ASSET_TRANSFER_OUT" => {
                if key.is_none() {
                    m.cash_rows.push(cash_row(a, date, line));
                    continue;
                }
                let incoming = match a.kind.as_str() {
                    "EXTERNAL_ASSET_TRANSFER_IN" => true,
                    "EXTERNAL_ASSET_TRANSFER_OUT" => false,
                    _ => a.units.is_none_or(|u| u >= Decimal::ZERO),
                };
                let mut r = inv_row(
                    a,
                    date,
                    if incoming {
                        InvAction::SharesIn
                    } else {
                        InvAction::SharesOut
                    },
                    line,
                );
                if incoming {
                    let avg = holdings
                        .iter()
                        .find(|h| h.symbol.key() == key)
                        .and_then(|h| h.average_price);
                    match (avg, r.quantity) {
                        (Some(p), Some(q)) => {
                            r.basis = p.checked_mul(q).map(cents);
                            r.acquired = Some(date);
                        }
                        _ => {
                            r.needs_review = true;
                            r.notes.push("Enter the basis of these shares".into());
                        }
                    }
                }
                m.inv_rows.push(r);
            }
            "STOCK_DIVIDEND" => {
                let mut r = inv_row(a, date, InvAction::SharesIn, line);
                r.basis = Some(Decimal::ZERO);
                r.acquired = Some(date);
                r.needs_review = true;
                r.notes
                    .push("Check the basis rules for this stock dividend".into());
                m.inv_rows.push(r);
            }
            "SPLIT" => {
                let mut r = inv_row(a, date, InvAction::Split, line);
                let held = key.as_ref().and_then(|k| held_before(k, date));
                match (held, a.units) {
                    (Some(h), Some(u)) if !h.is_zero() => {
                        if let Some(new) = h.checked_add(u) {
                            r.ratio = Some((new.normalize(), h.normalize()));
                        }
                    }
                    _ => {
                        r.needs_review = true;
                        r.notes.push(
                            "The split ratio could not be worked out from the holdings".into(),
                        );
                    }
                }
                r.quantity = None;
                m.inv_rows.push(r);
            }
            other => m.inv_rows.push(unsupported(
                a,
                date,
                format!("Unknown activity: {other}"),
                line,
            )),
        }
    }
    for h in holdings {
        if let Some(src) = h.symbol.source() {
            keys.entry(src.key.clone()).or_insert(src);
        }
    }
    m.securities = keys.into_values().collect();
    m
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_text() {
        let t = numbers_as_text(r#"{"a": 0.1, "b": [1e-7, -3], "c": "x 1.5 \" 2"}"#);
        assert_eq!(t, r#"{"a": "0.1", "b": ["1e-7", "-3"], "c": "x 1.5 \" 2"}"#);
        let v = parse(br#"{"price": 0.1, "units": 3, "tiny": 1e-7}"#).unwrap();
        let p = decimal_of(&v["price"]).unwrap();
        let u = decimal_of(&v["units"]).unwrap();
        assert_eq!(cents(p.checked_mul(u).unwrap()).to_string(), "0.30");
        assert_eq!(decimal_of(&v["tiny"]).unwrap().to_string(), "0.0000001");
    }

    #[test]
    fn canonical_json_sorts_keys() {
        let v = serde_json::json!({"query": "a=1", "content": null, "path": "/p", "x": {"b": 1, "a": [2, {"d": 1, "c": 2}]}});
        assert_eq!(
            canonical(&v),
            r#"{"content":null,"path":"/p","query":"a=1","x":{"a":[2,{"c":2,"d":1}],"b":1}}"#
        );
    }

    /// connectivity-brokerage.md AT10. The expected value was worked out outside the
    /// code: the canonical text below (keys sorted, no spaces, `content` null for an
    /// empty body) was signed with HMAC-SHA256 under the key `test-consumer-key` and
    /// the 32-byte digest Base64-encoded (Python `hmac.new(key, text, sha256)` +
    /// `base64.b64encode`).
    #[test]
    fn signature_matches_the_hand_computed_vector() {
        let path = "/api/v1/accounts/all";
        let query = "clientId=TESTCLIENT&timestamp=1791072000";
        let text = canonical(&serde_json::json!({"query": query, "path": path, "content": null}));
        assert_eq!(
            text,
            r#"{"content":null,"path":"/api/v1/accounts/all","query":"clientId=TESTCLIENT&timestamp=1791072000"}"#
        );
        assert_eq!(
            signature("test-consumer-key", path, query, None),
            "HbwLo620Twz8qe5+oC4H9wPrX0e7qPsOyfUMpZfrQxI="
        );
    }

    /// connectivity-brokerage.md revision 2, test 19 (the parsing): the reference's
    /// positions/all shape.
    #[test]
    fn positions_all_shape() {
        let body = br#"{"results": [
            {"instrument": {"kind": "etf", "id": "i1", "symbol": "VTI", "raw_symbol": "VTI",
              "description": "Vanguard Total Stock Market ETF", "currency": "USD",
              "figi_instrument": {"figi_code": "BBG000HXBZB2", "figi_share_class": "BBG001"}},
             "units": "10.5", "price": "123.45", "cost_basis": "118.2", "currency": "USD",
             "cash_equivalent": false, "tax_lots": [{"quantity": "10", "cost_basis": "1005.00"}]},
            {"instrument": {"kind": "mutualfund", "symbol": "SWVXX", "description": "Money market", "currency": "USD"},
             "units": 250.5, "price": 1, "currency": "USD", "cash_equivalent": true},
            {"instrument": {"kind": "option", "symbol": "VTI 261218C00300000", "currency": "USD"},
             "units": 1, "price": 2.5, "currency": "USD", "cash_equivalent": false},
            {"instrument": {"kind": "option", "symbol": "VTI 261218P00200000", "currency": "USD"},
             "units": 1, "price": 1.5, "currency": "USD", "cash_equivalent": false}
        ], "data_freshness": {"as_of": "2026-11-08T12:00:00Z"}}"#;
        let p = positions(body, "USD").unwrap();
        assert_eq!(p.holdings.len(), 1);
        let h = &p.holdings[0];
        assert_eq!(h.symbol.ticker.as_deref(), Some("VTI"));
        assert_eq!(h.symbol.figi.as_deref(), Some("BBG000HXBZB2"));
        assert_eq!(h.symbol.currency.as_deref(), Some("USD"));
        assert_eq!(h.units.to_string(), "10.5");
        assert_eq!(h.price.unwrap().to_string(), "123.45");
        assert_eq!(h.average_price.unwrap().to_string(), "118.2");
        assert_eq!(p.cash_equivalent.unwrap().to_string(), "250.50");
        assert_eq!(
            p.notes(),
            vec!["2 option positions not imported".to_owned()]
        );
    }

    #[test]
    fn masks() {
        assert_eq!(mask("Q6542138443"), "****8443");
        assert_eq!(mask("****8443"), "****8443");
    }
}
