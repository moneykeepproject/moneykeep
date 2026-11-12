//! SimpleFIN (connectivity.md; SimpleFIN Protocol v2.0.0 and v1): setup-token and
//! Access URL checks, `/info` versions and the Account Set, parsed with the
//! input-safety limits. Pure: bytes and text in, values out — the requests are made by
//! the caller through its fetcher.

use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use mk_core::{Date, Decimal};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::statement::{DateKind, RawRow};
use crate::{ImportError, ImportResult};

/// Longest setup-token URL and Access URL (characters).
pub const MAX_URL_CHARS: usize = 2000;
/// Largest Account Set accepted (bytes).
pub const MAX_RESPONSE_BYTES: usize = 20 * 1024 * 1024;
/// `extra` objects are kept in a row's raw text only up to this size (bytes).
pub const MAX_EXTRA_BYTES: usize = 4096;

fn invalid(m: impl Into<String>) -> ImportError {
    ImportError::Invalid(m.into())
}

/// An `https://` URL split into its parts (no query or fragment kept).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpsUrl {
    pub user: Option<String>,
    pub password: Option<String>,
    /// Host name, lower case (IPv6 literals keep their brackets).
    pub host: String,
    pub port: Option<u16>,
    /// Path starting with `/` (may be just `/`).
    pub path: String,
}

impl HttpsUrl {
    /// `https://host[:port]` without credentials.
    pub fn origin(&self) -> String {
        match self.port {
            Some(p) => format!("https://{}:{p}", self.host),
            None => format!("https://{}", self.host),
        }
    }

    /// The URL without credentials and without a trailing `/`.
    pub fn base(&self) -> String {
        format!("{}{}", self.origin(), self.path.trim_end_matches('/'))
    }

    /// `host[:port]`, the value requests are restricted to.
    pub fn authority(&self) -> String {
        match self.port {
            Some(p) => format!("{}:{p}", self.host),
            None => self.host.clone(),
        }
    }
}

fn percent_decode(s: &str) -> ImportResult<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| invalid("the URL has a bad %-escape"))?;
            out.push(hex);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| invalid("the URL's credentials are not UTF-8"))
}

/// Parse an `https://` URL (RFC 3986 subset: userinfo, host, port, path).
pub fn parse_https_url(text: &str) -> ImportResult<HttpsUrl> {
    let t = text.trim();
    if t.chars().count() > MAX_URL_CHARS {
        return Err(invalid(format!(
            "the URL is longer than {MAX_URL_CHARS} characters"
        )));
    }
    if t.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(invalid("the URL contains spaces or control characters"));
    }
    let lower = t.to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        t.get(8..).unwrap_or("")
    } else if lower.contains("://") {
        return Err(invalid("only https:// addresses are allowed"));
    } else {
        return Err(invalid("this is not a web address"));
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = rest.get(..end).unwrap_or("");
    let after = rest.get(end..).unwrap_or("");
    let path_end = after.find(['?', '#']).unwrap_or(after.len());
    let path = after.get(..path_end).unwrap_or("");
    let path = if path.is_empty() { "/" } else { path };
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    let (user, password) = match userinfo {
        Some(u) => {
            let (name, pass) = u.split_once(':').unwrap_or((u, ""));
            (
                Some(percent_decode(name)?),
                Some(percent_decode(pass)?).filter(|p| !p.is_empty()),
            )
        }
        None => (None, None),
    };
    let (host, port) = if let Some(stripped) = hostport.strip_prefix('[') {
        let close = stripped
            .find(']')
            .ok_or_else(|| invalid("the URL has a bad IPv6 host"))?;
        let host = format!("[{}]", stripped.get(..close).unwrap_or(""));
        let tail = stripped.get(close + 1..).unwrap_or("");
        let port = match tail.strip_prefix(':') {
            Some(p) => Some(
                p.parse::<u16>()
                    .map_err(|_| invalid("the URL has a bad port"))?,
            ),
            None if tail.is_empty() => None,
            None => return Err(invalid("the URL has a bad host")),
        };
        (host, port)
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (
                h.to_owned(),
                Some(
                    p.parse::<u16>()
                        .map_err(|_| invalid("the URL has a bad port"))?,
                ),
            ),
            None => (hostport.to_owned(), None),
        }
    };
    let host = host.to_ascii_lowercase();
    let host_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '[' | ']' | ':'));
    if !host_ok {
        return Err(invalid("the URL has no valid host"));
    }
    Ok(HttpsUrl {
        user,
        password,
        host,
        port,
        path: path.to_owned(),
    })
}

fn engine(alphabet: &base64::alphabet::Alphabet) -> GeneralPurpose {
    GeneralPurpose::new(
        alphabet,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    )
}

/// A decoded setup token: the claim URL (no credentials) and its host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimUrl {
    pub url: HttpsUrl,
}

/// Decode a setup token: trimmed, Base64 (standard or URL-safe, padding optional),
/// an `https://` URL whose path contains `/claim/`, at most 2,000 characters.
pub fn decode_setup_token(token: &str) -> ImportResult<ClaimUrl> {
    let t: String = token.chars().filter(|c| !c.is_whitespace()).collect();
    if t.is_empty() {
        return Err(invalid("paste the setup token"));
    }
    if t.len() > MAX_URL_CHARS * 2 {
        return Err(invalid("the setup token is too long"));
    }
    let bytes = engine(&base64::alphabet::URL_SAFE)
        .decode(&t)
        .or_else(|_| engine(&base64::alphabet::STANDARD).decode(&t))
        .map_err(|_| invalid("this is not a SimpleFIN setup token (it is not Base64)"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| invalid("this is not a SimpleFIN setup token (it is not text)"))?;
    let url = parse_https_url(&text)
        .map_err(|e| invalid(format!("this is not a SimpleFIN setup token: {e}")))?;
    if url.user.is_some() {
        return Err(invalid(
            "this is not a SimpleFIN setup token: it carries credentials",
        ));
    }
    if !url.path.contains("/claim/") {
        return Err(invalid(
            "this is not a SimpleFIN setup token: its address has no /claim/ part",
        ));
    }
    Ok(ClaimUrl { url })
}

/// The Access URL returned by a claim: `https://user:password@host/path`.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessUrl {
    pub url: HttpsUrl,
}

impl std::fmt::Debug for AccessUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the credentials.
        write!(f, "AccessUrl({})", self.url.base())
    }
}

impl AccessUrl {
    pub fn user(&self) -> &str {
        self.url.user.as_deref().unwrap_or_default()
    }
    pub fn password(&self) -> &str {
        self.url.password.as_deref().unwrap_or_default()
    }
}

/// Check a claim response body (or a stored credential) as an Access URL.
pub fn parse_access_url(body: &[u8]) -> ImportResult<AccessUrl> {
    let text = std::str::from_utf8(body)
        .map_err(|_| invalid("the provider's answer is not an access address"))?
        .trim();
    let url = parse_https_url(text).map_err(|e| {
        invalid(format!(
            "the provider's answer is not an access address: {e}"
        ))
    })?;
    if url.user.as_deref().is_none_or(str::is_empty) || url.password.is_none() {
        return Err(invalid(
            "the provider's answer is not an access address: it has no credentials",
        ));
    }
    if url.path == "/" {
        return Err(invalid(
            "the provider's answer is not an access address: it has no path",
        ));
    }
    Ok(AccessUrl { url })
}

/// The protocol versions in a `GET /info` answer.
pub fn parse_info(body: &[u8]) -> ImportResult<Vec<String>> {
    let text = std::str::from_utf8(body).map_err(|_| invalid("/info is not text"))?;
    let v = crate::safety::parse_json(text)?;
    Ok(v.get("versions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_owned())
                .collect()
        })
        .unwrap_or_default())
}

/// `2` when offered, else `1`.
pub fn choose_version(versions: &[String]) -> u8 {
    if versions.iter().any(|v| v == "2" || v.starts_with("2.")) {
        2
    } else {
        1
    }
}

/// An `errlist` entry (v1 `errors` strings get code `gen.v1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfError {
    pub code: String,
    /// The provider's message, cleaned (input-safety.md R3).
    pub msg: String,
    pub conn_id: Option<String>,
    pub account_id: Option<String>,
}

/// A server connection (v2 `connections`; v1: an account's `org`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfConnection {
    pub conn_id: String,
    pub name: String,
    pub org_id: Option<String>,
    pub org_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfTransaction {
    pub id: String,
    /// Epoch seconds; 0 while pending.
    pub posted: i64,
    pub transacted_at: Option<i64>,
    pub amount: Decimal,
    pub description: String,
    pub pending: bool,
    /// The transaction as received, `extra` cut to 4 KB.
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfAccount {
    pub id: String,
    pub name: String,
    pub conn_id: String,
    /// ISO code, or `None` for a custom currency (a URL).
    pub currency: Option<String>,
    pub currency_text: String,
    pub balance: Decimal,
    pub available: Option<Decimal>,
    pub balance_date: i64,
    pub transactions: Vec<SfTransaction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountSet {
    pub errors: Vec<SfError>,
    pub connections: Vec<SfConnection>,
    pub accounts: Vec<SfAccount>,
}

fn text(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn clean_msg(s: &str) -> String {
    mk_core::text::clean(s, crate::safety::TEXT_CHARS).text
}

fn epoch(v: &Value, key: &str) -> ImportResult<Option<i64>> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_i64()
            .map(Some)
            .ok_or_else(|| invalid(format!("{key} is not a whole number of seconds"))),
        Some(Value::String(s)) => s
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(|_| invalid(format!("{key} is not a whole number of seconds"))),
        Some(_) => Err(invalid(format!("{key} is not a number"))),
    }
}

fn decimal(v: &Value, key: &str, what: &str) -> ImportResult<Option<Decimal>> {
    let t = match v.get(key) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) => s.trim().to_owned(),
        Some(Value::Number(n)) => n.to_string(),
        Some(_) => return Err(invalid(format!("{what}: {key} is not a number"))),
    };
    crate::safety::check_number_len(&t)?;
    t.parse::<Decimal>()
        .map(Some)
        .map_err(|_| invalid(format!("{what}: {key} {t:?} is not a number")))
}

fn transaction(t: &Value) -> ImportResult<SfTransaction> {
    let id = text(t, "id");
    if id.is_empty() {
        return Err(invalid("a transaction has no id"));
    }
    let what = format!("transaction {id}");
    let posted = epoch(t, "posted")?.unwrap_or(0);
    let amount =
        decimal(t, "amount", &what)?.ok_or_else(|| invalid(format!("{what} has no amount")))?;
    let pending = t.get("pending").and_then(Value::as_bool) == Some(true) || posted == 0;
    let mut kept = t.clone();
    if let Some(o) = kept.as_object_mut()
        && let Some(extra) = o.get("extra")
        && extra.to_string().len() > MAX_EXTRA_BYTES
    {
        o.insert(
            "extra".into(),
            Value::String("(left out: larger than 4 KB)".into()),
        );
    }
    Ok(SfTransaction {
        id,
        posted,
        transacted_at: epoch(t, "transacted_at")?.filter(|x| *x > 0),
        amount,
        description: text(t, "description"),
        pending,
        raw: kept.to_string(),
    })
}

/// Parse an Account Set (`GET /accounts`), v2 or v1.
pub fn parse_account_set(body: &[u8]) -> ImportResult<AccountSet> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(invalid("the answer is larger than 20 MB"));
    }
    let text_body =
        std::str::from_utf8(body).map_err(|_| invalid("the answer is not UTF-8 text"))?;
    let v = crate::safety::parse_json(text_body)?;
    let mut set = AccountSet::default();
    for e in v
        .get("errlist")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        set.errors.push(SfError {
            code: text(e, "code"),
            msg: clean_msg(&text(e, "msg")),
            conn_id: Some(text(e, "conn_id")).filter(|s| !s.is_empty()),
            account_id: Some(text(e, "account_id")).filter(|s| !s.is_empty()),
        });
    }
    // v1: plain error strings.
    for e in v
        .get("errors")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        if let Some(s) = e.as_str() {
            set.errors.push(SfError {
                code: "gen.v1".into(),
                msg: clean_msg(s),
                conn_id: None,
                account_id: None,
            });
        }
    }
    for c in v
        .get("connections")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let conn_id = text(c, "conn_id");
        if conn_id.is_empty() {
            continue;
        }
        set.connections.push(SfConnection {
            conn_id,
            name: clean_msg(&text(c, "name")),
            org_id: Some(text(c, "org_id")).filter(|s| !s.is_empty()),
            org_url: Some(text(c, "org_url")).filter(|s| !s.is_empty()),
        });
    }
    for a in v
        .get("accounts")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let id = text(a, "id");
        if id.is_empty() {
            return Err(invalid("an account has no id"));
        }
        let what = format!("account {id}");
        // v1: the org stands in for the connection.
        let conn_id = match a.get("org") {
            Some(org) if a.get("conn_id").is_none() => {
                let key = [text(org, "id"), text(org, "domain"), text(org, "name")]
                    .into_iter()
                    .find(|s| !s.is_empty())
                    .unwrap_or_else(|| "org".into());
                if !set.connections.iter().any(|c| c.conn_id == key) {
                    let name = [text(org, "name"), text(org, "domain")]
                        .into_iter()
                        .find(|s| !s.is_empty())
                        .unwrap_or_else(|| key.clone());
                    set.connections.push(SfConnection {
                        conn_id: key.clone(),
                        name: clean_msg(&name),
                        org_id: Some(text(org, "id")).filter(|s| !s.is_empty()),
                        org_url: Some(text(org, "url")).filter(|s| !s.is_empty()),
                    });
                }
                key
            }
            _ => text(a, "conn_id"),
        };
        let currency_text = text(a, "currency").trim().to_owned();
        let currency = (currency_text.len() == 3
            && currency_text.chars().all(|c| c.is_ascii_alphabetic()))
        .then(|| currency_text.to_ascii_uppercase());
        let mut transactions = Vec::new();
        for t in a
            .get("transactions")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            transactions.push(transaction(t)?);
        }
        set.accounts.push(SfAccount {
            id,
            name: clean_msg(&text(a, "name")),
            conn_id,
            currency,
            currency_text,
            balance: decimal(a, "balance", &what)?.unwrap_or_default(),
            available: decimal(a, "available-balance", &what)?,
            balance_date: epoch(a, "balance-date")?.unwrap_or(0),
            transactions,
        });
    }
    Ok(set)
}

/// The date of an epoch time at a UTC offset (seconds east).
pub fn epoch_date(secs: i64, offset_secs: i32) -> ImportResult<Date> {
    let local = secs
        .checked_add(i64::from(offset_secs))
        .ok_or_else(|| invalid("a time is out of range"))?;
    let days =
        i32::try_from(local.div_euclid(86_400)).map_err(|_| invalid("a time is out of range"))?;
    Date::from_ymd(1970, 1, 1)
        .ok()
        .and_then(|d| d.add_days(days))
        .ok_or_else(|| invalid("a time is out of range"))
}

/// Epoch seconds of a date at 00:00 UTC.
pub fn date_epoch(d: Date) -> i64 {
    Date::from_ymd(1970, 1, 1)
        .map(|e| e.days_until(d).saturating_mul(86_400))
        .unwrap_or(0)
}

/// The import account key of a remote account (hashed like OFX keys).
pub fn account_key(connection: &str, conn_id: &str, account_id: &str) -> String {
    let material = format!("simplefin:{connection}:{conn_id}:{account_id}");
    let digest = Sha256::digest(material.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("simplefin:{hex}")
}

/// A transaction as a statement row. `transacted` picks `transacted_at` (when present)
/// over `posted` for the date.
pub fn row_of(
    t: &SfTransaction,
    transacted: bool,
    offset_secs: i32,
    line: usize,
) -> ImportResult<RawRow> {
    // Connectivity revision 3: a date from `transacted_at` is a transaction date.
    let (when, date_kind) = match (transacted, t.transacted_at) {
        (true, Some(x)) => (x, DateKind::Transaction),
        (false, Some(x)) if t.posted <= 0 => (x, DateKind::Transaction),
        _ => (t.posted, DateKind::Posted),
    };
    Ok(RawRow {
        date_kind,
        external_id: Some(t.id.clone()),
        date: epoch_date(when, offset_secs)?,
        amount: t.amount,
        payee: t.description.clone(),
        memo: String::new(),
        number: String::new(),
        category_hint: String::new(),
        cleared: None,
        splits: Vec::new(),
        trntype: None,
        review: t
            .pending
            .then(|| "pending at the bank: it may still change".to_owned()),
        correct_fitid: None,
        correct_action: None,
        line,
        raw: t.raw.clone(),
        cleaned: Vec::new(),
        class_hint: None,
        pending: t.pending,
        invoices: Vec::new(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s)
    }

    #[test]
    fn setup_tokens() {
        let c = decode_setup_token(&b64("https://bridge.test/simplefin/claim/abc")).unwrap();
        assert_eq!(c.url.host, "bridge.test");
        assert_eq!(c.url.base(), "https://bridge.test/simplefin/claim/abc");
        let std = base64::engine::general_purpose::STANDARD
            .encode("https://bridge.test/simplefin/claim/abc?");
        assert!(decode_setup_token(&format!("  {std}\n")).is_ok());
        for bad in [
            b64("http://bridge.test/claim/abc"),
            b64("https://bridge.test/other/abc"),
            b64("hello world"),
            "%%%not base64%%%".to_owned(),
            String::new(),
        ] {
            assert!(decode_setup_token(&bad).is_err(), "{bad}");
        }
        let long = format!("https://bridge.test/claim/{}", "a".repeat(2100));
        assert!(decode_setup_token(&b64(&long)).is_err());
    }

    #[test]
    fn access_urls() {
        let a = parse_access_url(b"https://user:p%40ss@bridge.test:8443/simplefin\n").unwrap();
        assert_eq!(a.user(), "user");
        assert_eq!(a.password(), "p@ss");
        assert_eq!(a.url.base(), "https://bridge.test:8443/simplefin");
        assert_eq!(a.url.authority(), "bridge.test:8443");
        assert!(!format!("{a:?}").contains("p@ss"));
        assert!(parse_access_url(b"https://bridge.test/simplefin").is_err());
        assert!(parse_access_url(b"http://u:p@bridge.test/x").is_err());
        assert!(parse_access_url(b"https://u:p@bridge.test/").is_err());
    }

    #[test]
    fn versions_and_dates() {
        let v = parse_info(br#"{"versions":["1","2"]}"#).unwrap();
        assert_eq!(choose_version(&v), 2);
        assert_eq!(choose_version(&["1".to_owned()]), 1);
        let d = Date::from_ymd(2026, 10, 1).unwrap();
        assert_eq!(epoch_date(date_epoch(d), 0).unwrap(), d);
        // 23:30 UTC on Oct 1 is Oct 2 at UTC+1.
        assert_eq!(
            epoch_date(date_epoch(d) + 84_600, 3600)
                .unwrap()
                .to_string(),
            "2026-10-02"
        );
    }

    #[test]
    fn deep_json_refused() {
        let deep = format!("{}{}", "[".repeat(100), "]".repeat(100));
        assert!(parse_account_set(deep.as_bytes()).is_err());
    }
}
