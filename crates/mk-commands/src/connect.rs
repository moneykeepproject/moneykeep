//! Bank feeds (connectivity.md): connections, account links and SimpleFIN syncs.
//!
//! The registry stays network-free: requests go through an injected [`Fetcher`] and
//! credentials through an injected [`SecretStore`] (the `mk-connect` crate supplies
//! the real ones; tests use recorded responses and an in-memory store). A feed never
//! writes to the ledger itself: it stages an import batch like a file import.

use std::collections::BTreeMap;

use mk_core::{AccountId, CommodityCode, Date};
use mk_import::simplefin::{self, AccessUrl, AccountSet, SfAccount};
use mk_import::{ClosingBalance, Section, Statement};
use mk_store::{Doc, DocTable, now_rfc3339};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::accounts::Chart;
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) const MODULE: &str = "bank-feeds";
/// `connect.*` serve bank feeds, payment processors and brokerage feeds.
const FEEDS_OR_PROCESSORS: &str = "bank-feeds|payment-processing|investments";
const PROVIDER: &str = "simplefin";
/// Days re-fetched before the last successful sync (late-posting transactions).
const OVERLAP_DAYS: i32 = 10;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;
const MAX_PER_DAY: usize = 24;

// ----- injected IO ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HttpMethod {
    #[default]
    Get,
    Post,
    Put,
    Delete,
}

/// One HTTPS request. The URL never carries credentials; Basic credentials travel in
/// `basic`. The fetcher must refuse any host other than `authority`.
#[derive(Clone, Default)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub basic: Option<(String, String)>,
    /// `host[:port]` the request (and any redirect) is restricted to.
    pub authority: String,
    /// Largest body accepted.
    pub max_bytes: usize,
    /// Extra headers (e.g. a bearer token, an API version). Values are never printed.
    pub headers: Vec<(String, String)>,
    /// A request body (POST), with its content type among `headers`.
    pub body: Option<Vec<u8>>,
    /// Plain `http://` is allowed for this request (the phone capture server on the
    /// loopback address or a trusted LAN, mobile-capture.md); never for providers.
    pub allow_http: bool,
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("authority", &self.authority)
            .field("basic", &self.basic.as_ref().map(|_| "(hidden)"))
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|h| h.0.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    /// `Retry-After` in seconds, when the server sent one.
    pub retry_after: Option<u64>,
    /// The `Deprecation` header's value, when the server sent one (logged by the
    /// SnapTrade sync with the path, connectivity-brokerage.md revision 2).
    pub deprecation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The network failed (DNS, connection, TLS handshake, timeout).
    Network(String),
    /// The request or answer broke a rule (another host, too large, not https).
    Refused(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(m) | Self::Refused(m) => f.write_str(m),
        }
    }
}

/// Sends HTTPS requests (the only way commands reach the network).
pub trait Fetcher: Send + Sync {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, FetchError>;
    /// Wait `ms` milliseconds between requests (`Retry-After`, request spacing). Test
    /// fetchers keep a fake clock instead of sleeping.
    fn wait(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    /// Milliseconds on a monotonic clock (request spacing); test fetchers answer
    /// their fake clock.
    fn clock_ms(&self) -> u64 {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let start = START.get_or_init(std::time::Instant::now);
        u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// The operating system's credential store (or a test double).
pub trait SecretStore: Send + Sync {
    /// Whether a store is usable on this computer.
    fn available(&self) -> bool;
    fn put(&self, id: &str, secret: &str) -> Result<(), String>;
    fn get(&self, id: &str) -> Result<Option<String>, String>;
    fn delete(&self, id: &str) -> Result<(), String>;
}

/// Network, credential store and clock for bank feeds.
pub struct Connector {
    pub fetcher: std::sync::Arc<dyn Fetcher>,
    pub secrets: Box<dyn SecretStore>,
    /// The user's UTC offset in seconds (dates of epoch times).
    pub utc_offset_secs: i32,
    /// Fixed current time in epoch seconds (tests and reviews); `None` uses the clock.
    pub now: Option<i64>,
    /// The capture server and `cloudflared` on this computer (phone capture setup,
    /// mobile-capture.md revision 2); `None` where services cannot be managed.
    pub host: Option<std::sync::Arc<dyn crate::capture_setup::CaptureHost>>,
}

impl std::fmt::Debug for Connector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connector")
            .field("utc_offset_secs", &self.utc_offset_secs)
            .field("now", &self.now)
            .field("host", &self.host.is_some())
            .finish_non_exhaustive()
    }
}

impl Connector {
    pub(crate) fn now(&self) -> i64 {
        self.now.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        })
    }
}

// ----- records -------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SecretRef {
    /// `book` (encrypted book's secrets table) or `os` (credential store).
    pub store: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ErrorNote {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ServerConn {
    pub conn_id: String,
    pub name: String,
    #[serde(default)]
    pub org_id: Option<String>,
    #[serde(default)]
    pub org_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConnDoc {
    pub id: String,
    pub provider: String,
    pub label: String,
    /// `active`, `needs_attention`, `revoked` or `disabled`.
    pub status: String,
    #[serde(default)]
    pub secret_ref: Option<SecretRef>,
    pub created_at: String,
    #[serde(default)]
    pub last_sync_at: Option<String>,
    /// Epoch seconds of the last successful fetch.
    #[serde(default)]
    pub last_success: Option<i64>,
    #[serde(default)]
    pub last_error: Option<ErrorNote>,
    pub host: String,
    pub version: u8,
    #[serde(default)]
    pub connections: Vec<ServerConn>,
    /// `errlist` of the last fetch (cleaned).
    #[serde(default)]
    pub errors: Vec<Value>,
    /// Epoch seconds of `/accounts` requests in the last 24 hours.
    #[serde(default)]
    pub requests: Vec<i64>,
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub next_retry_at: Option<i64>,
    /// Payment processors: accounting settings (connectivity-processors.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processor: Option<crate::processors::ProcSettings>,
    /// OFX Direct Connect: the institution profile and its state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ofx: Option<crate::ofx_conn::OfxSettings>,
    /// The Attention message the user dismissed (connectivity.md revision 5); cleared
    /// by a later successful sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed: Option<String>,
    /// What the last sync could not read while the rest was imported (the yellow
    /// line, connectivity-brokerage.md revision 2); cleared by a complete sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial: Option<String>,
    /// `Deprecation` headers seen in answers, with the path (newest last, at most
    /// 10; connectivity-brokerage.md revision 2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deprecations: Vec<String>,
}

/// What OFX and SnapTrade say about a remote account (connectivity-brokerage.md).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct RemoteInfo {
    /// OFX `bank`, `card`, `investment`; SnapTrade `investment`, `deposit`, …
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub bank_id: String,
    #[serde(default)]
    pub broker_id: String,
    #[serde(default)]
    pub acct_id: String,
    #[serde(default)]
    pub acct_type: String,
    /// OFX `SVCSTATUS`, or SnapTrade's sync status in words.
    #[serde(default)]
    pub status: String,
    /// The number masked to its last 4 (`****1234`).
    #[serde(default)]
    pub masked: String,
    #[serde(default)]
    pub institution: String,
    #[serde(default)]
    pub disabled: bool,
    /// OFX: `DTEND` of the last statement.
    #[serde(default)]
    pub last_dtend: Option<Date>,
    /// SnapTrade: the latest trade date synced.
    #[serde(default)]
    pub last_trade: Option<Date>,
    /// SnapTrade: content hash → (activity id, trade date) of activities seen.
    #[serde(default)]
    pub seen: BTreeMap<String, (String, Date)>,
    /// The latest holdings check.
    #[serde(default)]
    pub holdings: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LinkDoc {
    pub id: String,
    pub connection: String,
    pub remote_account_id: String,
    pub name: String,
    pub conn_id: String,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub balance: Option<String>,
    #[serde(default)]
    pub available: Option<String>,
    /// Epoch seconds.
    #[serde(default)]
    pub balance_date: Option<i64>,
    /// The linked MoneyKeep account (none = ignored).
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub sync_from: Option<Date>,
    #[serde(default)]
    pub include_pending: bool,
    #[serde(default)]
    pub auto_commit: bool,
    /// Date the account last appeared in a fetch.
    #[serde(default)]
    pub last_seen: Option<String>,
    /// Epoch seconds of the last successful fetch that included this account.
    #[serde(default)]
    pub last_success: Option<i64>,
    #[serde(default)]
    pub attention: Option<String>,
    #[serde(default = "yes")]
    pub active: bool,
    pub account_key: String,
    /// OFX and SnapTrade details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteInfo>,
    /// The dismissed `attention` message (connectivity.md revision 5); cleared when a
    /// sync finds the account without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed: Option<String>,
}

fn yes() -> bool {
    true
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

fn rejected(m: impl Into<String>) -> CommandError {
    CommandError::Rejected(m.into())
}

pub(crate) fn to_doc<T: Serialize>(id: &str, grp: Option<&str>, v: &T) -> CommandResult<Doc> {
    Ok(Doc {
        id: id.to_owned(),
        grp: grp.map(str::to_owned),
        ord: 0,
        json: serde_json::to_value(v)?,
    })
}

pub(crate) fn connections(ctx: &Ctx<'_>) -> CommandResult<Vec<ConnDoc>> {
    let mut v: Vec<ConnDoc> = ctx
        .tx
        .docs(DocTable::Connection, None)?
        .into_iter()
        .map(|d| serde_json::from_value(d.json))
        .collect::<Result<_, _>>()?;
    v.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(v)
}

pub(crate) fn find_conn(ctx: &Ctx<'_>, r: &str) -> CommandResult<ConnDoc> {
    let r = r.trim();
    connections(ctx)?
        .into_iter()
        .find(|c| c.id == r || c.label.eq_ignore_ascii_case(r))
        .ok_or_else(|| CommandError::NotFound(format!("connection {r:?}")))
}

pub(crate) fn save_conn(ctx: &Ctx<'_>, c: &ConnDoc) -> CommandResult<()> {
    if c.dismissed.is_some() && c.last_error.is_none() && c.status == "active" {
        // connectivity.md revision 5: a successful sync in between means the same
        // failure later is shown again.
        let mut c = c.clone();
        c.dismissed = None;
        return ctx.save_doc(DocTable::Connection, &c.id, Some(&to_doc(&c.id, None, &c)?));
    }
    ctx.save_doc(DocTable::Connection, &c.id, Some(&to_doc(&c.id, None, c)?))
}

/// SimpleFIN gives at most this many days of transactions per request.
pub(crate) const SIMPLEFIN_MAX_DAYS: i32 = 90;

/// The sync result's note when a SimpleFIN start date was clamped.
pub(crate) const SIMPLEFIN_CAP_NOTE: &str =
    "SimpleFIN gives at most 90 days of history; older transactions can be imported from a file.";

/// An `errlist` entry that only says a request was capped or adjusted
/// (connectivity.md revision 5): shown in the sync result, never flags the connection.
pub(crate) fn informational(code: &str, msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    code.starts_with("gen.")
        && (m.contains("capped") || m.contains("limit") || m.contains("adjusted"))
}

pub(crate) fn links_of(ctx: &Ctx<'_>, conn: &str) -> CommandResult<Vec<LinkDoc>> {
    links(ctx, conn)
}

pub(crate) fn save_link_doc(ctx: &Ctx<'_>, l: &LinkDoc) -> CommandResult<()> {
    save_link(ctx, l)
}

pub(crate) fn link_id_of(conn: &str, remote: &str) -> String {
    link_id(conn, remote)
}

/// A new, unlinked remote account of a connection.
pub(crate) fn new_link(conn: &ConnDoc, id: &str, remote: &str, account_key: &str) -> LinkDoc {
    LinkDoc {
        id: id.to_owned(),
        connection: conn.id.clone(),
        remote_account_id: remote.to_owned(),
        name: String::new(),
        conn_id: String::new(),
        currency: None,
        balance: None,
        available: None,
        balance_date: None,
        account: None,
        sync_from: None,
        include_pending: false,
        auto_commit: false,
        last_seen: None,
        last_success: None,
        attention: None,
        active: true,
        account_key: account_key.to_owned(),
        remote: None,
        dismissed: None,
    }
}

/// A new connection record (active, no accounts yet).
pub(crate) fn blank_conn(
    _ctx: &Ctx<'_>,
    id: &str,
    provider: &str,
    label: &str,
    home: &str,
    sid: &str,
    host: &str,
) -> CommandResult<ConnDoc> {
    Ok(ConnDoc {
        id: id.to_owned(),
        provider: provider.to_owned(),
        label: label.to_owned(),
        status: "active".into(),
        secret_ref: Some(SecretRef {
            store: home.to_owned(),
            id: sid.to_owned(),
        }),
        created_at: now_rfc3339()?,
        last_sync_at: None,
        last_success: None,
        last_error: None,
        host: host.to_owned(),
        version: 0,
        connections: Vec::new(),
        errors: Vec::new(),
        requests: Vec::new(),
        failures: 0,
        next_retry_at: None,
        processor: None,
        ofx: None,
        dismissed: None,
        partial: None,
        deprecations: Vec::new(),
    })
}

/// Epoch seconds of a date's midnight (UTC).
pub(crate) fn date_epoch(d: Date) -> i64 {
    simplefin::date_epoch(d)
}

/// Stage a feed's statement as a batch marked with the connection.
pub(crate) fn stage_feed(
    ctx: &Ctx<'_>,
    conn: &ConnDoc,
    statement: Statement,
    format: &str,
    source: String,
    digest_input: &[u8],
) -> CommandResult<Value> {
    let digest: String = Sha256::digest(digest_input)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let staged = crate::import::stage_statement_with(
        ctx,
        statement,
        format,
        Some(source),
        digest,
        None,
        None,
        None,
    )?;
    let batch_id = staged
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut b = crate::import::load_batch(ctx, &batch_id)?;
    b.feed = Some(conn.id.clone());
    crate::import::save_batch(ctx, &b)?;
    let sealed = crate::import::sealed_json(ctx, &batch_id)?;
    let counts = sealed.get("counts").cloned().unwrap_or(Value::Null);
    let new = counts.get("new").and_then(Value::as_u64).unwrap_or(0);
    *ctx.audit_args.borrow_mut() = None;
    Ok(json!({
        "connection": conn.id, "label": conn.label, "status": conn.status,
        "batch": batch_id, "counts": counts, "new": new, "warnings": sealed.get("warnings"),
    }))
}

fn links(ctx: &Ctx<'_>, conn: &str) -> CommandResult<Vec<LinkDoc>> {
    let mut v: Vec<LinkDoc> = ctx
        .tx
        .docs(DocTable::AccountLink, Some(conn))?
        .into_iter()
        .map(|d| serde_json::from_value(d.json))
        .collect::<Result<_, _>>()?;
    v.sort_by_key(|a| a.name.to_lowercase());
    Ok(v)
}

fn save_link(ctx: &Ctx<'_>, l: &LinkDoc) -> CommandResult<()> {
    ctx.save_doc(
        DocTable::AccountLink,
        &l.id,
        Some(&to_doc(&l.id, Some(&l.connection), l)?),
    )
}

fn link_id(conn: &str, remote: &str) -> String {
    let d = Sha256::digest(format!("{conn}:{remote}").as_bytes());
    let hex: String = d.iter().take(12).map(|b| format!("{b:02x}")).collect();
    format!("{conn}-{hex}")
}

fn status_label(s: &str) -> &'static str {
    match s {
        "active" => "Active",
        "needs_attention" => "Needs attention",
        "unfinished" => "Not finished",
        "revoked" => "Revoked",
        _ => "Disabled",
    }
}

/// The kind a new MoneyKeep account for a remote account starts as in the link step
/// (connectivity.md revision 4): `bank`, `credit-card`, `loan` or `investment`. OFX
/// and SnapTrade say what the account is; SimpleFIN does not, so its account name
/// (then its institution's name) is read for card and loan words.
pub(crate) fn kind_hint(l: &LinkDoc, org: &str) -> &'static str {
    if let Some(r) = &l.remote {
        match r.kind.as_str() {
            "card" => return "credit-card",
            "investment" => return "investment",
            _ => {}
        }
        if r.acct_type.eq_ignore_ascii_case("CREDITLINE") {
            return "loan";
        }
        if !r.kind.is_empty() {
            return "bank";
        }
    }
    let words = |text: &str| -> Option<&'static str> {
        // "Credit union" names an institution, not a card.
        let t = text.to_lowercase().replace("credit union", "");
        if ["loan", "mortgage"].iter().any(|w| t.contains(w)) {
            Some("loan")
        } else if ["card", "visa", "mastercard", "amex", "discover", "credit"]
            .iter()
            .any(|w| t.contains(w))
        {
            Some("credit-card")
        } else {
            None
        }
    };
    words(&l.name).or_else(|| words(org)).unwrap_or("bank")
}

/// The institution name the provider gave for a remote account's server connection.
fn org_of<'c>(c: &'c ConnDoc, l: &LinkDoc) -> &'c str {
    c.connections
        .iter()
        .find(|s| s.conn_id == l.conn_id)
        .map_or("", |s| s.name.as_str())
}

fn link_json(chart: &Chart, l: &LinkDoc, org: &str) -> Value {
    let account_name = l
        .account
        .as_deref()
        .and_then(|a| a.parse::<AccountId>().ok())
        .map(|a| chart.full_name(a));
    let remote = l.remote.as_ref().map(|r| {
        json!({"kind": r.kind, "status": r.status, "masked": r.masked,
            "institution": r.institution, "acct_type": r.acct_type, "disabled": r.disabled,
            "linkable": linkable(l).is_ok(), "last_dtend": r.last_dtend,
            "holdings": r.holdings})
    });
    json!({
        "remote": remote,
        "id": l.id, "remote_account_id": l.remote_account_id, "name": l.name,
        "conn_id": l.conn_id, "currency": l.currency, "balance": l.balance,
        "available": l.available, "balance_date": l.balance_date,
        "account": l.account, "account_name": account_name, "sync_from": l.sync_from,
        "include_pending": l.include_pending, "auto_commit": l.auto_commit,
        "last_seen": l.last_seen, "attention": l.attention, "active": l.active,
        "kind_hint": kind_hint(l, org),
        "attention_dismissed": l.attention.is_some() && l.dismissed == l.attention,
    })
}

/// Whether a remote account can be linked (OFX: an active service; SnapTrade: an
/// investment account), or why not.
fn linkable(l: &LinkDoc) -> Result<(), String> {
    let Some(r) = &l.remote else { return Ok(()) };
    if l.account_key.starts_with("ofxdc:") && r.status != "ACTIVE" {
        return Err(format!(
            "the bank shows this account's service as {}; it can be linked once it is ACTIVE",
            if r.status.is_empty() {
                "unknown"
            } else {
                r.status.as_str()
            }
        ));
    }
    if l.account_key.starts_with("snaptrade:") && r.kind != "investment" {
        return Err("only investment accounts can be linked from SnapTrade for now".into());
    }
    Ok(())
}

/// A connection for display: never a credential, only the host.
pub(crate) fn conn_json(ctx: &Ctx<'_>, c: &ConnDoc) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let accounts: Vec<Value> = links(ctx, &c.id)?
        .iter()
        .map(|l| link_json(&chart, l, org_of(c, l)))
        .collect();
    let ofx = match &c.ofx {
        Some(s) => Some(json!({
            "name": s.institution.name, "host": s.institution.host(),
            "ofx_version": s.institution.ofx_version, "ask_password": s.ask_password,
            "notice": crate::ofx_conn::profile_notice(ctx, s)?,
            "challenge": !s.challenge.is_empty(), "needs_auth_token": s.needs_auth_token,
            "single_requests": s.single_requests,
        })),
        None => None,
    };
    Ok(json!({
        "ofx": ofx,
        "id": c.id, "provider": c.provider, "label": c.label, "status": c.status,
        "status_label": status_label(&c.status), "host": c.host, "version": c.version,
        "credential_store": c.secret_ref.as_ref().map(|r| r.store.clone()),
        "created_at": c.created_at, "last_sync_at": c.last_sync_at,
        "last_error": c.last_error, "connections": c.connections, "errors": c.errors,
        "partial": c.partial, "deprecations": c.deprecations,
        "next_retry_at": c.next_retry_at, "accounts": accounts,
        "notice_dismissed": c.last_error.is_some()
            && c.dismissed.as_deref() == Some(conn_notice(c).as_str()),
        "earliest_imported": crate::import::earliest_imported(ctx, &c.id)?,
    }))
}

// ----- secrets -------------------------------------------------------------------------

const NO_SECURE_STORE: &str = "The connection can't be saved securely: this book has no password and this computer has no credential store. Protect the book with a password (Settings > Security) and try again.";

/// Where a new credential will be kept, or the refusal.
pub(crate) fn secret_home(ctx: &Ctx<'_>, c: &Connector) -> CommandResult<&'static str> {
    if ctx.encrypted {
        Ok("book")
    } else if c.secrets.available() {
        Ok("os")
    } else {
        Err(CommandError::Coded {
            code: "no_secure_storage",
            message: NO_SECURE_STORE.into(),
        })
    }
}

pub(crate) fn get_secret(
    ctx: &Ctx<'_>,
    c: &Connector,
    r: &SecretRef,
) -> CommandResult<Option<String>> {
    if r.store == "book" {
        Ok(ctx.tx.get_secret(&r.id)?)
    } else {
        c.secrets
            .get(&r.id)
            .map_err(|e| rejected(format!("the credential store could not be read: {e}")))
    }
}

fn delete_secret(ctx: &Ctx<'_>, c: Option<&Connector>, r: &SecretRef) -> CommandResult<()> {
    let book = ctx.tx.info()?.id;
    if r.store == "book" {
        ctx.tx.delete_secret(&r.id)?;
        book_index_change(ctx, &book, |idx| idx.retain(|e| e.id != r.id))?;
    } else if let Some(c) = c {
        c.secrets
            .delete(&r.id)
            .map_err(|e| rejected(format!("the credential store refused: {e}")))?;
        os_index_change(c, &book, |idx| idx.retain(|e| e.id != r.id));
    }
    Ok(())
}

// ----- requests ------------------------------------------------------------------------

fn get_req(access: &AccessUrl, path_query: &str, max: usize) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        url: format!("{}{path_query}", access.url.base()),
        basic: Some((access.user().to_owned(), access.password().to_owned())),
        authority: access.url.authority(),
        max_bytes: max,
        ..HttpRequest::default()
    }
}

// ----- commands ------------------------------------------------------------------------

pub(crate) fn register(r: &mut Registry) {
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.providers",
        "Bank-feed providers and whether credentials can be kept safely here",
        false,
        providers,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.add",
        "Connect a provider with a setup token (handles a credential; not offered over MCP)",
        true,
        add,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.list",
        "Bank-feed connections (host only, never credentials)",
        false,
        list,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.get",
        "One bank-feed connection with its remote accounts",
        false,
        get,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.link_account",
        "Link a remote account to a MoneyKeep account (or ignore it)",
        true,
        link_account,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.sync",
        "Fetch new transactions and stage them for review",
        true,
        sync,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.disconnect",
        "Delete a connection's credential (not offered over MCP)",
        true,
        disconnect,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.set_label",
        "Rename a connection",
        true,
        set_label,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.disable",
        "Stop syncing a connection",
        true,
        disable,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.enable",
        "Sync a disabled connection again",
        true,
        enable,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.resume",
        "Finish a connection whose setup was interrupted (no new token needed)",
        true,
        resume,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.unused",
        "Stored credentials without a connection for 24 hours",
        false,
        unused,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.delete_credential",
        "Delete an unused stored credential",
        true,
        delete_credential,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.attention",
        "Staged feed batches and connections needing attention",
        false,
        attention_cmd,
    );
    r.add_in(
        FEEDS_OR_PROCESSORS,
        "connect.dismiss",
        "Dismiss a connection's Attention notice until a different one appears",
        true,
        dismiss,
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AttentionArgs {
    /// The current date (default: today).
    #[serde(default)]
    today: Option<Date>,
}

fn attention_cmd(ctx: &Ctx<'_>, a: AttentionArgs) -> CommandResult<Value> {
    let today = match a.today {
        Some(d) => d,
        None => mk_store::today_utc()?,
    };
    let mut items = match attention(ctx)? {
        Value::Array(v) => v,
        _ => Vec::new(),
    };
    items.extend(crate::processors::unmatched_notices(ctx, today)?);
    Ok(Value::Array(items))
}

fn providers(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let secure = ctx.encrypted || ctx.connector.is_some_and(|c| c.secrets.available());
    let store = if ctx.encrypted {
        "book"
    } else if secure {
        "os"
    } else {
        "none"
    };
    let mut out = Vec::new();
    for (name, title, module) in [
        (PROVIDER, "SimpleFIN Bridge", MODULE),
        (
            crate::ofx_conn::PROVIDER,
            "Bank (OFX Direct Connect)",
            crate::ofx_conn::MODULE,
        ),
        (
            crate::snap_conn::PROVIDER,
            "Brokerage (SnapTrade)",
            crate::snap_conn::MODULE,
        ),
    ] {
        if crate::is_module_enabled(ctx.tx, module)? {
            out.push(json!({
                "name": name, "title": title, "network": ctx.connector.is_some(),
                "secure_storage": secure, "credential_store": store,
            }));
        }
    }
    if out.is_empty() {
        out.push(json!({
            "name": PROVIDER, "title": "SimpleFIN Bridge", "network": ctx.connector.is_some(),
            "secure_storage": secure, "credential_store": store,
        }));
    }
    Ok(Value::Array(out))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddArgs {
    /// `simplefin`, `stripe`, `square`, `paypal`, `ofx` or `snaptrade`.
    #[serde(default)]
    provider: Option<String>,
    /// The setup token (SimpleFIN) or key (processors).
    #[serde(default)]
    setup_input: String,
    #[serde(default)]
    label: Option<String>,
    /// Without it, the claim host is returned for the user to confirm and nothing is
    /// sent.
    #[serde(default)]
    confirm: bool,
    /// OFX: the institution profile entered by hand.
    #[serde(default)]
    institution: Option<mk_import::ofx_dc::Institution>,
    /// OFX: an installed institution profile (its name).
    #[serde(default)]
    institution_key: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    password: Option<String>,
    /// OFX: extra credentials the bank's profile asks for.
    #[serde(default)]
    cred1: Option<String>,
    #[serde(default)]
    cred2: Option<String>,
    /// OFX: the user confirmed the host is their bank's (required).
    #[serde(default)]
    host_acknowledged: bool,
    /// OFX: store only the user id; ask for the password on each sync.
    #[serde(default)]
    ask_password_each_time: bool,
    /// OFX: answers to the bank's extra questions.
    #[serde(default)]
    answers: Vec<crate::ofx_conn::Answer>,
    /// OFX: a one-time code from the bank.
    #[serde(default)]
    auth_token: Option<String>,
    /// SnapTrade: the personal client id.
    #[serde(default)]
    client_id: Option<String>,
    /// SnapTrade: the personal consumer key.
    #[serde(default)]
    consumer_key: Option<String>,
}

const TOKEN_USED: &str = "This setup token was already used or does not exist. If you did not use it, someone else may have — revoke access at your provider.";

/// Registered for the schema and listings; the work is done in network steps.
fn add(_: &Ctx<'_>, _: AddArgs) -> CommandResult<Value> {
    Err(CommandError::Integrity(
        "connect.add runs through the registry's network steps".into(),
    ))
}

fn resume(_: &Ctx<'_>, _: ConnRef) -> CommandResult<Value> {
    Err(CommandError::Integrity(
        "connect.resume runs through the registry's network steps".into(),
    ))
}

pub(crate) fn note_error(c: &mut ConnDoc, code: &str, message: &str) {
    c.last_error = Some(ErrorNote {
        code: code.into(),
        message: message.into(),
    });
}

/// Connection status for a non-200 answer.
pub(crate) fn apply_status(c: &mut ConnDoc, status: u16, now: i64) {
    match status {
        402 => {
            c.status = "needs_attention".into();
            note_error(
                c,
                "payment_required",
                "Your SimpleFIN subscription needs payment.",
            );
        }
        403 => {
            c.status = "revoked".into();
            note_error(
                c,
                "revoked",
                "Access was revoked or the credentials are wrong — reconnect with a new setup token.",
            );
        }
        s => {
            note_error(
                c,
                "server",
                &format!("The provider answered with HTTP {s}; MoneyKeep will try again later."),
            );
            backoff(c, now);
        }
    }
}

pub(crate) fn backoff(c: &mut ConnDoc, now: i64) {
    c.failures = c.failures.saturating_add(1);
    let hours = 1_i64 << c.failures.saturating_sub(1).min(5);
    c.next_retry_at = Some(now.saturating_add(hours.min(24) * HOUR));
}

/// Record the Account Set's connections, errors and remote accounts (links).
fn apply_set_meta(
    ctx: &Ctx<'_>,
    conn: &mut ConnDoc,
    set: &AccountSet,
    today: Option<&str>,
) -> CommandResult<()> {
    for sc in &set.connections {
        let s = ServerConn {
            conn_id: sc.conn_id.clone(),
            name: sc.name.clone(),
            org_id: sc.org_id.clone(),
            org_url: sc.org_url.clone(),
        };
        match conn.connections.iter_mut().find(|x| x.conn_id == s.conn_id) {
            Some(x) => *x = s,
            None => conn.connections.push(s),
        }
    }
    conn.errors = set
        .errors
        .iter()
        .filter(|e| !informational(&e.code, &e.msg))
        .map(|e| json!({"code": e.code, "msg": e.msg, "conn_id": e.conn_id, "account_id": e.account_id}))
        .collect();
    let general: Vec<&str> = set
        .errors
        .iter()
        .filter(|e| !informational(&e.code, &e.msg))
        .filter(|e| e.code.starts_with("gen.") || (e.conn_id.is_none() && e.account_id.is_none()))
        .map(|e| e.msg.as_str())
        .collect();
    if !general.is_empty() {
        conn.status = "needs_attention".into();
        note_error(conn, "provider", &general.join(" "));
    }
    let existing = links(ctx, &conn.id)?;
    for a in &set.accounts {
        let id = link_id(&conn.id, &a.id);
        let attention = set
            .errors
            .iter()
            .filter(|e| {
                (e.code.starts_with("con.") && e.conn_id.as_deref() == Some(a.conn_id.as_str()))
                    || (e.code.starts_with("act.")
                        && e.account_id.as_deref() == Some(a.id.as_str()))
            })
            .map(|e| e.msg.clone())
            .collect::<Vec<_>>();
        let mut l = existing
            .iter()
            .find(|l| l.id == id)
            .cloned()
            .unwrap_or_else(|| LinkDoc {
                id: id.clone(),
                connection: conn.id.clone(),
                remote_account_id: a.id.clone(),
                name: String::new(),
                conn_id: a.conn_id.clone(),
                currency: None,
                balance: None,
                available: None,
                balance_date: None,
                account: None,
                sync_from: None,
                include_pending: false,
                auto_commit: false,
                last_seen: None,
                last_success: None,
                attention: None,
                active: true,
                account_key: simplefin::account_key(&conn.id, &a.conn_id, &a.id),
                remote: None,
                dismissed: None,
            });
        l.name = a.name.clone();
        l.conn_id = a.conn_id.clone();
        l.currency = Some(
            a.currency
                .clone()
                .unwrap_or_else(|| a.currency_text.clone()),
        );
        l.balance = Some(a.balance.to_string());
        l.available = a.available.map(|x| x.to_string());
        l.balance_date = Some(a.balance_date).filter(|d| *d > 0);
        if let Some(t) = today {
            l.last_seen = Some(t.to_owned());
        }
        l.attention = (!attention.is_empty()).then(|| attention.join(" "));
        if l.attention.is_none() {
            l.dismissed = None;
        }
        save_link(ctx, &l)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ConnRef {
    /// Connection id or label.
    connection: String,
}

fn list(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    Ok(Value::Array(
        connections(ctx)?
            .iter()
            .map(|c| conn_json(ctx, c))
            .collect::<CommandResult<_>>()?,
    ))
}

fn get(ctx: &Ctx<'_>, a: ConnRef) -> CommandResult<Value> {
    conn_json(ctx, &find_conn(ctx, &a.connection)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LinkArgs {
    connection: String,
    /// The remote account's id (or its name).
    remote_account: String,
    /// A MoneyKeep account (id, full name or code); null or "" ignores the remote
    /// account.
    #[serde(default)]
    account: Option<String>,
    /// First day to fetch (default: `bank-feeds.history_days_first_sync` days ago).
    #[serde(default)]
    sync_from: Option<Date>,
    #[serde(default)]
    include_pending: Option<bool>,
    #[serde(default)]
    auto_commit: Option<bool>,
}

fn link_account(ctx: &Ctx<'_>, a: LinkArgs) -> CommandResult<Value> {
    let conn = find_conn(ctx, &a.connection)?;
    let mut l = links(ctx, &conn.id)?
        .into_iter()
        .find(|l| {
            l.remote_account_id == a.remote_account
                || l.name.eq_ignore_ascii_case(&a.remote_account)
        })
        .ok_or_else(|| CommandError::NotFound(format!("remote account {:?}", a.remote_account)))?;
    let chart = Chart::load(ctx)?;
    match a.account.as_deref().map(str::trim) {
        None | Some("") => l.account = None,
        Some(r) => {
            linkable(&l).map_err(rejected)?;
            let acc = chart.resolve(r)?;
            if acc.placeholder || acc.archived {
                return Err(rejected(format!(
                    "{} is a placeholder or archived account and cannot receive a feed",
                    chart.full_name(acc.id)
                )));
            }
            if let Some(cur) = l.currency.as_deref().filter(|c| c.len() == 3)
                && acc.commodity.as_str() != cur
            {
                return Err(rejected(format!(
                    "{} is in {cur}, but {} is in {}; pick an account in {cur}",
                    l.name,
                    chart.full_name(acc.id),
                    acc.commodity
                )));
            }
            l.account = Some(acc.id.to_string());
            // The import pipeline maps the feed's sections by this key.
            let doc = Doc {
                id: l.account_key.clone(),
                grp: None,
                ord: 0,
                json: json!({ "key": l.account_key, "account": acc.id }),
            };
            ctx.save_doc(DocTable::ImportAccountMap, &l.account_key, Some(&doc))?;
        }
    }
    if a.sync_from.is_some() {
        l.sync_from = a.sync_from;
    }
    if let Some(p) = a.include_pending {
        l.include_pending = p;
    }
    if let Some(x) = a.auto_commit {
        l.auto_commit = x;
    }
    save_link(ctx, &l)?;
    Ok(link_json(&chart, &l, org_of(&conn, &l)))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SyncArgs {
    /// Connection id or label (default: every active connection).
    #[serde(default)]
    connection: Option<String>,
    /// The user pressed Sync now: the hourly limit and the retry wait do not apply
    /// (the daily limit does).
    #[serde(default)]
    now: bool,
    /// Only connections whose last sync is older than this many hours
    /// (`bank-feeds.sync_interval_hours` when `scheduled`).
    #[serde(default)]
    scheduled: bool,
    /// Fetch from this date instead of the usual window (still limited by what the
    /// provider returns); used to correct dates of imported transactions
    /// (connectivity.md revision 3). Bank feeds (SimpleFIN, OFX) only.
    #[serde(default)]
    from: Option<Date>,
}

/// Registered for the schema and listings; the work is done in network steps.
fn sync(_: &Ctx<'_>, _: SyncArgs) -> CommandResult<Value> {
    Err(CommandError::Integrity(
        "connect.sync runs through the registry's network steps".into(),
    ))
}

fn skip(conn: &ConnDoc, reason: &str) -> Value {
    json!({"connection": conn.id, "label": conn.label, "status": conn.status, "skipped": reason})
}

// ----- network steps (connectivity.md revision 2) ----------------------------------------
//
// A request never runs inside a book transaction: a short read plans it, the requests
// run with no transaction open (on any thread, cancellable), and a short write applies
// the answers. Cancelling before the write records nothing.

/// Commands that make requests.
pub(crate) fn is_network(name: &str) -> bool {
    matches!(
        name,
        "connect.add" | "connect.resume" | "connect.sync" | "connect.answer_challenge"
    ) || crate::capture::is_network(name)
        || crate::capture_setup::is_network(name)
        || crate::capture_wifi::is_network(name)
}

#[derive(Debug, Clone)]
pub(crate) enum NetOp {
    /// Claim a setup token for a new connection.
    Claim {
        conn: String,
        home: String,
        label: Option<String>,
        provider: Option<String>,
    },
    /// `GET /info` of a connection.
    Info { conn: String },
    /// `GET /accounts` (balances) of a connection, finishing it.
    Accounts { conn: String, version: u8 },
    /// Fetch transactions of connections.
    Sync { by_user: bool, plans: Vec<SyncPlan> },
    /// Check a new processor credential.
    ProcVerify {
        conn: String,
        home: String,
        label: Option<String>,
        provider: String,
        credential: crate::processors::Credential,
    },
    /// Connect an OFX institution (profile, account information, challenge).
    OfxAdd(crate::ofx_conn::AddStep),
    /// Check a new SnapTrade personal key by listing accounts.
    SnapVerify {
        conn: String,
        home: String,
        label: Option<String>,
        key: crate::snap_conn::Key,
    },
    /// Phone capture requests (mobile-capture.md).
    Capture(crate::capture::Op),
    /// Phone capture setup and status (mobile-capture.md revision 2).
    CaptureSetup(Box<crate::capture_setup::Op>),
    /// Wi-Fi capture setup (mobile-capture.md revision 10).
    CaptureWifi(Box<crate::capture_wifi::Op>),
}

#[derive(Debug, Clone)]
pub(crate) struct SyncPlan {
    conn: String,
    action: PlanAction,
    /// Notes for the sync result (the SimpleFIN 90-day cap).
    notes: Vec<String>,
}

#[derive(Debug, Clone)]
enum PlanAction {
    Skip(String),
    MissingCredential,
    /// Index of the request.
    Fetch(usize),
    /// Index of the processor job.
    Processor(usize),
    /// Index of the OFX job.
    Ofx(usize),
    /// Index of the SnapTrade job.
    Snap(usize),
}

/// Requests to send with no book transaction open.
pub struct NetJob {
    op: NetOp,
    requests: Vec<HttpRequest>,
    /// Payment-processor reads (several requests each, paged).
    processors: Vec<crate::processors::ProcJob>,
    /// OFX Direct Connect reads.
    ofx: Vec<crate::ofx_conn::OfxJob>,
    /// SnapTrade reads.
    snap: Vec<crate::snap_conn::SnapJob>,
    /// The SnapTrade connect check (key, Unix seconds): signed when sent, its answer
    /// is the job's first response.
    snap_check: Option<(crate::snap_conn::Key, i64)>,
    fetcher: std::sync::Arc<dyn Fetcher>,
    /// A phone capture action on this computer, run after the requests.
    host: Option<(
        std::sync::Arc<dyn crate::capture_setup::CaptureHost>,
        crate::capture_setup::HostOp,
    )>,
}

impl std::fmt::Debug for NetJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetJob")
            .field("op", &self.op)
            .field("requests", &self.requests)
            .finish_non_exhaustive()
    }
}

/// The answers of a [`NetJob`].
#[derive(Debug)]
pub struct Fetched {
    op: NetOp,
    responses: Vec<Result<HttpResponse, FetchError>>,
    processed: Vec<Result<mk_import::processors::ProcessorData, crate::processors::ProcError>>,
    ofx_out: Vec<crate::ofx_conn::OfxResult>,
    snap_out: Vec<crate::snap_conn::SnapResult>,
    host_out: Option<crate::capture_setup::HostOut>,
    cancelled: bool,
}

/// What a network command needs next.
#[derive(Debug)]
// The job is moved on at once (to a thread or the next step); boxing buys nothing.
#[allow(clippy::large_enum_variant)]
pub enum NetStep {
    /// Send these requests (no book access needed), then resume with the answers.
    Fetch(NetJob),
    Done(Value),
}

impl NetJob {
    /// A job of one request.
    fn single(op: NetOp, req: HttpRequest, fetcher: std::sync::Arc<dyn Fetcher>) -> Self {
        NetJob {
            op,
            requests: vec![req],
            processors: Vec::new(),
            ofx: Vec::new(),
            snap: Vec::new(),
            snap_check: None,
            fetcher,
            host: None,
        }
    }

    /// A job of several requests sent in order.
    pub(crate) fn many(
        op: NetOp,
        requests: Vec<HttpRequest>,
        fetcher: std::sync::Arc<dyn Fetcher>,
    ) -> Self {
        NetJob {
            op,
            requests,
            processors: Vec::new(),
            ofx: Vec::new(),
            snap: Vec::new(),
            snap_check: None,
            fetcher,
            host: None,
        }
    }

    /// The SnapTrade connect check (security-review-1 B3: signed when sent).
    fn snap_check(
        op: NetOp,
        key: crate::snap_conn::Key,
        now: i64,
        fetcher: std::sync::Arc<dyn Fetcher>,
    ) -> Self {
        let mut job = Self::many(op, Vec::new(), fetcher);
        job.snap_check = Some((key, now));
        job
    }

    /// Requests followed by an action on this computer (phone capture setup).
    pub(crate) fn with_host(
        op: NetOp,
        requests: Vec<HttpRequest>,
        fetcher: std::sync::Arc<dyn Fetcher>,
        host: Option<(
            std::sync::Arc<dyn crate::capture_setup::CaptureHost>,
            crate::capture_setup::HostOp,
        )>,
    ) -> Self {
        let mut job = Self::many(op, requests, fetcher);
        job.host = host;
        job
    }

    /// Send the requests. Raising `cancel` stops waiting at once; the answers are
    /// discarded.
    pub fn fetch(self, cancel: &std::sync::atomic::AtomicBool) -> Fetched {
        let mut out = Fetched {
            op: self.op,
            responses: Vec::new(),
            processed: Vec::new(),
            ofx_out: Vec::new(),
            snap_out: Vec::new(),
            host_out: None,
            cancelled: false,
        };
        for req in &self.requests {
            match fetch_one(&self.fetcher, req, cancel) {
                Some(r) => out.responses.push(r),
                None => {
                    out.cancelled = true;
                    return out;
                }
            }
        }
        let fetcher = self.fetcher.clone();
        let mut send = |r: HttpRequest| fetch_one(&fetcher, &r, cancel);
        if let Some((key, now)) = &self.snap_check {
            match crate::snap_conn::check(key, *now, &mut send, fetcher.as_ref()) {
                Some(r) => out.responses.push(r),
                None => {
                    out.cancelled = true;
                    return out;
                }
            }
        }
        for job in &self.processors {
            let r = crate::processors::run(job, &mut send);
            if matches!(r, Err(crate::processors::ProcError::Cancelled)) {
                out.cancelled = true;
                return out;
            }
            out.processed.push(r);
        }
        for job in &self.ofx {
            let r = crate::ofx_conn::run(job, &mut send);
            if matches!(r, crate::ofx_conn::OfxResult::Cancelled) {
                out.cancelled = true;
                return out;
            }
            out.ofx_out.push(r);
        }
        for job in &self.snap {
            let r = crate::snap_conn::run(job, &mut send, fetcher.as_ref());
            if matches!(r, crate::snap_conn::SnapResult::Cancelled) {
                out.cancelled = true;
                return out;
            }
            out.snap_out.push(r);
        }
        if let Some((host, op)) = &self.host {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                out.cancelled = true;
                return out;
            }
            out.host_out = Some(crate::capture_setup::run_host(host.as_ref(), op));
        }
        out
    }
}

/// Send one request on its own thread; `None` when `cancel` was raised first.
fn fetch_one(
    fetcher: &std::sync::Arc<dyn Fetcher>,
    req: &HttpRequest,
    cancel: &std::sync::atomic::AtomicBool,
) -> Option<Result<HttpResponse, FetchError>> {
    use std::sync::atomic::Ordering;
    if cancel.load(Ordering::SeqCst) {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let f = fetcher.clone();
    let r = req.clone();
    let spawned = std::thread::Builder::new()
        .name("mk-fetch".into())
        .spawn(move || {
            let _ = tx.send(f.send(&r));
        });
    if spawned.is_err() {
        return Some(fetcher.send(req));
    }
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(r) => return Some(r),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if cancel.load(Ordering::SeqCst) {
                    return None;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Some(Err(FetchError::Network("the request stopped".into())));
            }
        }
    }
}

fn no_network() -> CommandError {
    rejected("this MoneyKeep front end cannot reach the network; use the app or `moneykeep sync`")
}

fn claim_request(claim: &simplefin::ClaimUrl) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Post,
        url: claim.url.base(),
        basic: None,
        authority: claim.url.authority(),
        max_bytes: 8 * 1024,
        ..HttpRequest::default()
    }
}

fn info_job(c: &Connector, conn: &str, access: &AccessUrl) -> NetJob {
    NetJob::single(
        NetOp::Info {
            conn: conn.to_owned(),
        },
        get_req(access, "/info", 64 * 1024),
        c.fetcher.clone(),
    )
}

fn access_of(ctx: &Ctx<'_>, c: &Connector, conn: &ConnDoc) -> CommandResult<AccessUrl> {
    let r = conn
        .secret_ref
        .as_ref()
        .ok_or_else(|| rejected("the connection has no credential; reconnect"))?;
    let cred = get_secret(ctx, c, r)?
        .ok_or_else(|| rejected("credential not found on this computer — reconnect"))?;
    simplefin::parse_access_url(cred.as_bytes())
        .map_err(|_| rejected("the stored credential is damaged; reconnect"))
}

impl Registry {
    fn connect_spec(&self, name: &str) -> CommandResult<&crate::CommandSpec> {
        self.spec(name)
            .ok_or_else(|| CommandError::UnknownCommand(name.to_owned()))
    }

    /// A short read transaction for planning requests.
    fn read_phase<T>(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        f: impl FnOnce(&Ctx<'_>) -> CommandResult<T>,
    ) -> CommandResult<T> {
        let encrypted = store.is_encrypted();
        let connector = self.connector.as_ref();
        store.read(|tx| {
            if !crate::is_module_enabled(tx, MODULE)?
                && !crate::is_module_enabled(tx, crate::processors::MODULE)?
                && !crate::is_module_enabled(tx, crate::snap_conn::MODULE)?
            {
                return Err(CommandError::ModuleDisabled {
                    command: "connect".into(),
                    module: MODULE.into(),
                });
            }
            let mut ctx = Ctx::new(tx, actor);
            ctx.connector = connector;
            ctx.encrypted = encrypted;
            f(&ctx)
        })
    }

    /// Start a network command (`connect.add`, `connect.resume`, `connect.sync`).
    pub fn net_start(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        name: &str,
        args: Value,
    ) -> CommandResult<NetStep> {
        if crate::capture::is_network(name) {
            return self.capture_start(store, actor, name, args);
        }
        if crate::capture_setup::is_network(name) {
            return self.setup_start(store, actor, name, args);
        }
        if crate::capture_wifi::is_network(name) {
            return self.wifi_start(store, actor, name, args);
        }
        match name {
            "connect.add" => {
                let a: AddArgs = crate::parse_args(args)?;
                let provider = a.provider.clone().unwrap_or_else(|| PROVIDER.into());
                let module = match provider.as_str() {
                    PROVIDER | "ofx" => MODULE,
                    "snaptrade" => crate::snap_conn::MODULE,
                    _ => crate::processors::MODULE,
                };
                self.read_phase(store, actor, |ctx| {
                    if crate::is_module_enabled(ctx.tx, module)? {
                        Ok(())
                    } else {
                        Err(CommandError::ModuleDisabled {
                            command: "connect.add".into(),
                            module: module.into(),
                        })
                    }
                })?;
                if let Some(p) = crate::processors::Provider::parse(&provider) {
                    // Checked before any request (a secret Stripe key is refused).
                    let credential = crate::processors::credential_from_input(p, &a.setup_input)?;
                    if !a.confirm {
                        return Ok(NetStep::Done(json!({
                            "confirm": {"host": p.host()},
                            "message": format!("MoneyKeep will contact {} to check this credential. Continue?", p.host()),
                        })));
                    }
                    let c = self.connector.as_ref().ok_or_else(no_network)?;
                    let home = self.read_phase(store, actor, |ctx| secret_home(ctx, c))?;
                    let req = crate::processors::verify_request(p, &credential);
                    return Ok(NetStep::Fetch(NetJob::single(
                        NetOp::ProcVerify {
                            conn: mk_core::Id::<()>::new().to_string(),
                            home: home.into(),
                            label: a.label,
                            provider: p.key().into(),
                            credential,
                        },
                        req,
                        c.fetcher.clone(),
                    )));
                }
                if provider == crate::ofx_conn::PROVIDER {
                    let c = self.connector.as_ref().ok_or_else(no_network)?;
                    let input = crate::ofx_conn::AddInput {
                        institution: a.institution,
                        institution_key: a.institution_key,
                        user_id: a.user_id.unwrap_or_default(),
                        password: a.password.unwrap_or_default(),
                        cred1: a.cred1,
                        cred2: a.cred2,
                        host_acknowledged: a.host_acknowledged,
                        ask_password: a.ask_password_each_time,
                        answers: a.answers.into_iter().map(|x| (x.id, x.answer)).collect(),
                        auth_token: a.auth_token,
                        label: a.label,
                    };
                    let (st, req) = self.read_phase(store, actor, |ctx| {
                        let home = secret_home(ctx, c)?;
                        crate::ofx_conn::start_add(ctx, c, input, home)
                    })?;
                    return Ok(NetStep::Fetch(NetJob::single(
                        NetOp::OfxAdd(crate::ofx_conn::AddStep::Profile(Box::new(st))),
                        req,
                        c.fetcher.clone(),
                    )));
                }
                if provider == crate::snap_conn::PROVIDER {
                    let key = crate::snap_conn::key_from_input(
                        a.client_id.as_deref().unwrap_or_default(),
                        a.consumer_key.as_deref().unwrap_or_default(),
                    )?;
                    let c = self.connector.as_ref().ok_or_else(no_network)?;
                    let home = self.read_phase(store, actor, |ctx| secret_home(ctx, c))?;
                    return Ok(NetStep::Fetch(NetJob::snap_check(
                        NetOp::SnapVerify {
                            conn: mk_core::Id::<()>::new().to_string(),
                            home: home.into(),
                            label: a.label,
                            key: key.clone(),
                        },
                        key,
                        c.now(),
                        c.fetcher.clone(),
                    )));
                }
                if provider != PROVIDER {
                    return Err(invalid(
                        "provider is simplefin, stripe, square, paypal, ofx or snaptrade",
                    ));
                }
                let claim = simplefin::decode_setup_token(&a.setup_input)
                    .map_err(|e| invalid(e.to_string()))?;
                if !a.confirm {
                    let host = claim.url.host.clone();
                    return Ok(NetStep::Done(json!({
                        "confirm": {"host": host},
                        "message": format!("MoneyKeep will contact {host} to claim this token. Continue?"),
                    })));
                }
                let c = self.connector.as_ref().ok_or_else(no_network)?;
                let home = self.read_phase(store, actor, |ctx| secret_home(ctx, c))?;
                Ok(NetStep::Fetch(NetJob::single(
                    NetOp::Claim {
                        conn: mk_core::Id::<()>::new().to_string(),
                        home: home.into(),
                        label: a.label,
                        provider: a.provider,
                    },
                    claim_request(&claim),
                    c.fetcher.clone(),
                )))
            }
            "connect.resume" => {
                let a: ConnRef = crate::parse_args(args)?;
                let c = self.connector.as_ref().ok_or_else(no_network)?;
                self.read_phase(store, actor, |ctx| {
                    let conn = find_conn(ctx, &a.connection)?;
                    if conn.status == "revoked" {
                        return Err(rejected(
                            "access was revoked; reconnect with a new setup token",
                        ));
                    }
                    let access = access_of(ctx, c, &conn)?;
                    Ok(NetStep::Fetch(info_job(c, &conn.id, &access)))
                })
            }
            "connect.sync" => {
                let a: SyncArgs = crate::parse_args(args)?;
                let c = self.connector.as_ref().ok_or_else(no_network)?;
                let mut job = self.read_phase(store, actor, |ctx| plan_sync(ctx, c, &a, None))?;
                job.fetcher = c.fetcher.clone();
                Ok(NetStep::Fetch(job))
            }
            "connect.answer_challenge" => {
                let a: crate::ofx_conn::AnswerArgs = crate::parse_args(args)?;
                let c = self.connector.as_ref().ok_or_else(no_network)?;
                let extra = a.extra();
                let sync = SyncArgs {
                    connection: Some(a.connection.clone()),
                    now: true,
                    scheduled: false,
                    from: None,
                };
                let mut job =
                    self.read_phase(store, actor, |ctx| plan_sync(ctx, c, &sync, Some(&extra)))?;
                job.fetcher = c.fetcher.clone();
                Ok(NetStep::Fetch(job))
            }
            other => Err(CommandError::UnknownCommand(other.to_owned())),
        }
    }

    /// Continue a network command with the answers of its requests.
    pub fn net_resume(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        f: Fetched,
    ) -> CommandResult<NetStep> {
        if f.cancelled {
            return Err(CommandError::Coded {
                code: "cancelled",
                message: "Cancelled; nothing was recorded.".into(),
            });
        }
        let c = self.connector.as_ref().ok_or_else(no_network)?;
        if let NetOp::Capture(op) = f.op {
            return self.capture_resume(store, actor, c, op, f.responses);
        }
        if let NetOp::CaptureSetup(op) = f.op {
            return self.setup_resume(store, actor, c, *op, f.responses, f.host_out);
        }
        if let NetOp::CaptureWifi(op) = f.op {
            return self.wifi_resume(store, actor, c, *op, f.responses, f.host_out);
        }
        let first = f
            .responses
            .first()
            .cloned()
            .unwrap_or_else(|| Err(FetchError::Network("no answer".into())));
        match f.op {
            NetOp::Claim {
                conn,
                home,
                label,
                provider,
            } => {
                let resp = first.map_err(|e| rejected(format!("the claim failed: {e}")))?;
                match resp.status {
                    200 => {}
                    403 => {
                        return Err(CommandError::Coded {
                            code: "token_used",
                            message: TOKEN_USED.into(),
                        });
                    }
                    s => return Err(rejected(format!("the claim failed (HTTP {s})"))),
                }
                let access =
                    simplefin::parse_access_url(&resp.body).map_err(|e| rejected(e.to_string()))?;
                let credential = String::from_utf8_lossy(&resp.body).trim().to_owned();
                // The credential is stored at once: a later failure leaves a connection
                // that can be resumed without a new token.
                let book = store.info()?.id;
                let sid = format!("{book}:{conn}");
                let now = c.now();
                if home == "book" {
                    store.write(|tx| -> CommandResult<()> {
                        tx.put_secret(&sid, &credential)?;
                        let mut idx = index_parse(tx.get_secret(&index_key(&book))?);
                        idx.push(IndexEntry {
                            id: sid.clone(),
                            at: now,
                        });
                        tx.put_secret(&index_key(&book), &index_text(&idx))?;
                        Ok(())
                    })?;
                } else {
                    c.secrets.put(&sid, &credential).map_err(|e| {
                        rejected(format!("the credential store refused the credential: {e}"))
                    })?;
                    os_index_change(c, &book, |idx| {
                        idx.push(IndexEntry {
                            id: sid.clone(),
                            at: now,
                        })
                    });
                }
                let spec = self.connect_spec("connect.add")?;
                let audit = json!({"provider": provider, "label": label});
                let host = access.url.host.clone();
                self.execute(store, actor, spec, audit, None, &|ctx, _| {
                    let label = label
                        .clone()
                        .map(|l| mk_core::text::clean(&l, 100).text.trim().to_owned())
                        .filter(|l| !l.is_empty())
                        .unwrap_or_else(|| host.clone());
                    let doc = ConnDoc {
                        id: conn.clone(),
                        provider: PROVIDER.into(),
                        label,
                        status: "unfinished".into(),
                        secret_ref: Some(SecretRef {
                            store: home.clone(),
                            id: sid.clone(),
                        }),
                        created_at: now_rfc3339()?,
                        last_sync_at: None,
                        last_success: None,
                        last_error: None,
                        host: host.clone(),
                        version: 1,
                        connections: Vec::new(),
                        errors: Vec::new(),
                        requests: Vec::new(),
                        failures: 0,
                        next_retry_at: None,
                        processor: None,
                        ofx: None,
                        dismissed: None,
                        partial: None,
                        deprecations: Vec::new(),
                    };
                    save_conn(ctx, &doc)?;
                    conn_json(ctx, &doc)
                })?;
                Ok(NetStep::Fetch(info_job(c, &conn, &access)))
            }
            NetOp::Info { conn } => {
                let version = match &first {
                    Ok(r) if r.status == 200 => simplefin::parse_info(&r.body)
                        .map(|v| simplefin::choose_version(&v))
                        .unwrap_or(1),
                    _ => 1,
                };
                self.read_phase(store, actor, |ctx| {
                    let doc = find_conn(ctx, &conn)?;
                    let access = access_of(ctx, c, &doc)?;
                    Ok(NetStep::Fetch(NetJob::single(
                        NetOp::Accounts {
                            conn: conn.clone(),
                            version,
                        },
                        get_req(
                            &access,
                            &format!("/accounts?version={version}&balances-only=1"),
                            simplefin::MAX_RESPONSE_BYTES,
                        ),
                        c.fetcher.clone(),
                    )))
                })
            }
            NetOp::Accounts { conn, version } => {
                let spec = self.connect_spec("connect.resume")?;
                let v = self.execute(
                    store,
                    actor,
                    spec,
                    json!({"connection": conn}),
                    None,
                    &|ctx, _| apply_accounts(ctx, c, &conn, version, &first),
                )?;
                Ok(NetStep::Done(v))
            }
            NetOp::Sync { by_user, ref plans } => {
                let spec = self.connect_spec("connect.sync")?;
                let v = self.execute(
                    store,
                    actor,
                    spec,
                    json!({"now": by_user}),
                    None,
                    &|ctx, _| apply_sync(ctx, c, plans, &f),
                )?;
                Ok(NetStep::Done(v))
            }
            NetOp::ProcVerify {
                conn,
                home,
                label,
                provider,
                credential,
            } => {
                let p = crate::processors::Provider::parse(&provider)
                    .ok_or_else(|| invalid("unknown provider"))?;
                let resp = first
                    .map_err(|e| rejected(format!("{} could not be reached: {e}", p.title())))?;
                match resp.status {
                    200 => {}
                    401 | 403 => {
                        let message = if p == crate::processors::Provider::Square {
                            "Square refused this token: it needs the MERCHANT_PROFILE_READ and PAYOUTS_READ permissions.".to_owned()
                        } else {
                            format!(
                                "{} rejected this credential; check it has read access and try again.",
                                p.title()
                            )
                        };
                        return Err(CommandError::Coded {
                            code: "credential_rejected",
                            message,
                        });
                    }
                    s => return Err(rejected(format!("{} answered with HTTP {s}", p.title()))),
                }
                let text = crate::processors::credential_text(&credential);
                let book = store.info()?.id;
                let sid = format!("{book}:{conn}");
                let now = c.now();
                if home == "book" {
                    store.write(|tx| -> CommandResult<()> {
                        tx.put_secret(&sid, &text)?;
                        let mut idx = index_parse(tx.get_secret(&index_key(&book))?);
                        idx.push(IndexEntry {
                            id: sid.clone(),
                            at: now,
                        });
                        tx.put_secret(&index_key(&book), &index_text(&idx))?;
                        Ok(())
                    })?;
                } else {
                    c.secrets.put(&sid, &text).map_err(|e| {
                        rejected(format!("the credential store refused the credential: {e}"))
                    })?;
                    os_index_change(c, &book, |idx| {
                        idx.push(IndexEntry {
                            id: sid.clone(),
                            at: now,
                        })
                    });
                }
                let spec = self.connect_spec("connect.add")?;
                let audit = json!({"provider": provider, "label": label});
                let v = self.execute(store, actor, spec, audit, None, &|ctx, _| {
                    let label = label
                        .clone()
                        .map(|l| mk_core::text::clean(&l, 100).text.trim().to_owned())
                        .filter(|l| !l.is_empty())
                        .unwrap_or_else(|| p.title().to_owned());
                    let doc = ConnDoc {
                        id: conn.clone(),
                        provider: p.key().into(),
                        label,
                        status: "active".into(),
                        secret_ref: Some(SecretRef {
                            store: home.clone(),
                            id: sid.clone(),
                        }),
                        created_at: now_rfc3339()?,
                        last_sync_at: None,
                        last_success: None,
                        last_error: None,
                        host: p.host().into(),
                        version: 0,
                        connections: Vec::new(),
                        errors: Vec::new(),
                        requests: Vec::new(),
                        failures: 0,
                        next_retry_at: None,
                        processor: Some(crate::processors::new_settings(ctx, p)?),
                        ofx: None,
                        dismissed: None,
                        partial: None,
                        deprecations: Vec::new(),
                    };
                    save_conn(ctx, &doc)?;
                    let mut v = conn_json(ctx, &doc)?;
                    if let Some(o) = v.as_object_mut() {
                        o.insert(
                            "processor".into(),
                            crate::processors::settings_json(ctx, &doc)?,
                        );
                    }
                    Ok(v)
                })?;
                Ok(NetStep::Done(v))
            }
            NetOp::OfxAdd(step) => self.resume_ofx_add(store, actor, c, step, &first),
            NetOp::SnapVerify {
                conn,
                home,
                label,
                key,
            } => {
                let resp =
                    first.map_err(|e| rejected(format!("SnapTrade could not be reached: {e}")))?;
                if resp.status != 200 {
                    let (code, m) = crate::snap_conn::refusal(resp.status, &resp.body, "accounts");
                    return Err(CommandError::Coded {
                        code: if code == "clock" {
                            "clock"
                        } else {
                            "credential_rejected"
                        },
                        message: m,
                    });
                }
                let accounts = mk_import::snaptrade::accounts(&resp.body)
                    .map_err(|e| rejected(format!("The answer was refused: {e}")))?;
                let book = store.info()?.id;
                let sid = format!("{book}:{conn}");
                self.store_credential(store, c, &home, &sid, &crate::snap_conn::key_text(&key))?;
                let spec = self.connect_spec("connect.add")?;
                let audit = json!({"provider": crate::snap_conn::PROVIDER, "label": label});
                let v = self.execute(store, actor, spec, audit, None, &|ctx, _| {
                    let doc = crate::snap_conn::new_conn(ctx, &conn, label.clone(), &home, &sid)?;
                    save_conn(ctx, &doc)?;
                    crate::snap_conn::apply_accounts(ctx, &doc, &accounts)?;
                    conn_json(ctx, &doc)
                })?;
                Ok(NetStep::Done(v))
            }
            // Handled above.
            NetOp::Capture(_) | NetOp::CaptureSetup(_) | NetOp::CaptureWifi(_) => {
                Err(rejected("unexpected phone capture step"))
            }
        }
    }

    /// Store a new connection's credential at once (before the connection record), in
    /// the encrypted book or the computer's store.
    fn store_credential(
        &self,
        store: &mut mk_store::Store,
        c: &Connector,
        home: &str,
        sid: &str,
        text: &str,
    ) -> CommandResult<()> {
        let book = store.info()?.id;
        let now = c.now();
        if home == "book" {
            store.write(|tx| -> CommandResult<()> {
                tx.put_secret(sid, text)?;
                let mut idx = index_parse(tx.get_secret(&index_key(&book))?);
                idx.push(IndexEntry {
                    id: sid.to_owned(),
                    at: now,
                });
                tx.put_secret(&index_key(&book), &index_text(&idx))?;
                Ok(())
            })?;
        } else {
            c.secrets.put(sid, text).map_err(|e| {
                rejected(format!("the credential store refused the credential: {e}"))
            })?;
            os_index_change(c, &book, |idx| {
                idx.push(IndexEntry {
                    id: sid.to_owned(),
                    at: now,
                })
            });
        }
        Ok(())
    }

    fn resume_ofx_add(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        c: &Connector,
        step: crate::ofx_conn::AddStep,
        first: &Result<HttpResponse, FetchError>,
    ) -> CommandResult<NetStep> {
        use crate::ofx_conn::AddNext;
        match crate::ofx_conn::resume_add(step, first)? {
            AddNext::Done(v) => Ok(NetStep::Done(v)),
            AddNext::Request(step, req) => Ok(NetStep::Fetch(NetJob::single(
                NetOp::OfxAdd(step),
                req,
                c.fetcher.clone(),
            ))),
            AddNext::Create(st, reply) => {
                let book = store.info()?.id;
                let sid = format!("{book}:{}", st.conn);
                let text = crate::ofx_conn::stored_secret(&st, &reply);
                self.store_credential(store, c, &st.home, &sid, &text)?;
                let spec = self.connect_spec("connect.add")?;
                let audit = json!({"provider": crate::ofx_conn::PROVIDER, "label": st.label,
                    "host": st.inst.host()});
                let v = self.execute(store, actor, spec, audit, None, &|ctx, _| {
                    let doc = crate::ofx_conn::new_conn(ctx, &st, &sid)?;
                    save_conn(ctx, &doc)?;
                    let listed = reply.accounts.clone();
                    if let Some(list) = &listed {
                        crate::ofx_conn::apply_accounts(ctx, &doc, list)?;
                    }
                    let mut v = conn_json(ctx, &doc)?;
                    if let Some(o) = v.as_object_mut() {
                        o.insert("client_uid".into(), json!(st.uid));
                        if listed.as_ref().is_none_or(Vec::is_empty) {
                            o.insert(
                                "message".into(),
                                json!("Your bank did not list accounts; add them by hand."),
                            );
                        }
                    }
                    Ok(v)
                })?;
                Ok(NetStep::Done(v))
            }
        }
    }

    /// Run a network command to the end on this thread (CLI, MCP, tests).
    pub(crate) fn run_network(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        name: &str,
        args: Value,
    ) -> CommandResult<Value> {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut step = self.net_start(store, actor, name, args)?;
        loop {
            match step {
                NetStep::Done(v) => return Ok(v),
                NetStep::Fetch(job) => {
                    let fetched = job.fetch(&cancel);
                    step = self.net_resume(store, actor, fetched)?;
                }
            }
        }
    }
}

/// The remote accounts answer of a new or resumed connection.
fn apply_accounts(
    ctx: &Ctx<'_>,
    c: &Connector,
    conn_id: &str,
    version: u8,
    resp: &Result<HttpResponse, FetchError>,
) -> CommandResult<Value> {
    let mut conn = find_conn(ctx, conn_id)?;
    conn.version = version;
    match resp {
        Ok(r) if r.status == 200 => match simplefin::parse_account_set(&r.body) {
            Ok(set) => {
                conn.status = "active".into();
                conn.last_error = None;
                apply_set_meta(ctx, &mut conn, &set, None)?;
            }
            Err(e) => note_error(
                &mut conn,
                "bad_response",
                &format!("The provider's answer was refused: {e}"),
            ),
        },
        Ok(r) => {
            if matches!(r.status, 402 | 403) {
                apply_status(&mut conn, r.status, c.now());
            } else {
                note_error(
                    &mut conn,
                    "server",
                    &format!(
                        "The provider answered with HTTP {}; resume to try again.",
                        r.status
                    ),
                );
            }
        }
        Err(e) => note_error(&mut conn, "network", &e.to_string()),
    }
    save_conn(ctx, &conn)?;
    let mut v = conn_json(ctx, &conn)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("resumable".into(), json!(conn.status == "unfinished"));
    }
    Ok(v)
}

/// The requests of a sync, decided in a read transaction.
fn plan_sync(
    ctx: &Ctx<'_>,
    c: &Connector,
    a: &SyncArgs,
    extra: Option<&crate::ofx_conn::Extra>,
) -> CommandResult<NetJob> {
    let now = c.now();
    let targets: Vec<ConnDoc> = match &a.connection {
        Some(r) => vec![find_conn(ctx, r)?],
        None => connections(ctx)?
            .into_iter()
            .filter(|x| matches!(x.status.as_str(), "active" | "needs_attention"))
            .collect(),
    };
    let interval = crate::module_settings::get_i64(ctx, "bank-feeds.sync_interval_hours")?.max(1);
    let mut plans = Vec::new();
    let mut requests = Vec::new();
    let mut processors = Vec::new();
    let mut ofx = Vec::new();
    let mut snap = Vec::new();
    for conn in targets {
        if a.scheduled
            && conn
                .last_success
                .is_some_and(|t| now.saturating_sub(t) < interval * HOUR)
        {
            continue;
        }
        let mut notes = Vec::new();
        let action = match plan_one(ctx, c, &conn, now, a.now, extra, a.from)? {
            Ok(Planned::Http(req, capped)) => {
                requests.push(req);
                if capped {
                    notes.push(SIMPLEFIN_CAP_NOTE.to_owned());
                }
                PlanAction::Fetch(requests.len() - 1)
            }
            Ok(Planned::Proc(job)) => {
                processors.push(job);
                PlanAction::Processor(processors.len() - 1)
            }
            Ok(Planned::Ofx(job)) => {
                ofx.push(*job);
                PlanAction::Ofx(ofx.len() - 1)
            }
            Ok(Planned::Snap(job)) => {
                snap.push(*job);
                PlanAction::Snap(snap.len() - 1)
            }
            Err(a) => a,
        };
        plans.push(SyncPlan {
            conn: conn.id.clone(),
            action,
            notes,
        });
    }
    Ok(NetJob {
        op: NetOp::Sync {
            by_user: a.now,
            plans,
        },
        requests,
        processors,
        ofx,
        snap,
        snap_check: None,
        fetcher: c.fetcher.clone(),
        host: None,
    })
}

/// The work of one connection's sync.
enum Planned {
    /// The request, and whether its start date was clamped to 90 days.
    Http(HttpRequest, bool),
    Proc(crate::processors::ProcJob),
    Ofx(Box<crate::ofx_conn::OfxJob>),
    Snap(Box<crate::snap_conn::SnapJob>),
}

/// One connection's request, or why it is not fetched.
fn plan_one(
    ctx: &Ctx<'_>,
    c: &Connector,
    conn: &ConnDoc,
    now: i64,
    by_user: bool,
    extra: Option<&crate::ofx_conn::Extra>,
    from: Option<Date>,
) -> CommandResult<Result<Planned, PlanAction>> {
    let skip = |m: &str| Ok(Err(PlanAction::Skip(m.to_owned())));
    match conn.status.as_str() {
        "disabled" => return skip("the connection is disabled"),
        "revoked" => return skip("access was revoked; reconnect with a new setup token"),
        "unfinished" => return skip("the connection is not finished; resume it"),
        _ => {}
    }
    let recent: Vec<i64> = conn
        .requests
        .iter()
        .copied()
        .filter(|t| now.saturating_sub(*t) < DAY)
        .collect();
    if recent.len() >= MAX_PER_DAY {
        return skip("24 syncs today already; try again tomorrow");
    }
    if !by_user {
        if recent.iter().any(|t| now.saturating_sub(*t) < HOUR) {
            return skip("synced less than an hour ago");
        }
        if conn.next_retry_at.is_some_and(|t| now < t) {
            return skip("waiting before the next try");
        }
    }
    let Some(sref) = conn.secret_ref.clone() else {
        return skip("the connection has no credential; reconnect");
    };
    let Some(credential) = get_secret(ctx, c, &sref)? else {
        return Ok(Err(PlanAction::MissingCredential));
    };
    if conn.provider == crate::ofx_conn::PROVIDER {
        return Ok(
            match crate::ofx_conn::plan(ctx, c, conn, &credential, by_user, extra, from)? {
                Ok(job) => Ok(Planned::Ofx(Box::new(job))),
                Err(m) => Err(PlanAction::Skip(m)),
            },
        );
    }
    if conn.provider == crate::snap_conn::PROVIDER {
        return Ok(
            match crate::snap_conn::plan(ctx, c, conn, &credential, by_user)? {
                Ok(job) => Ok(Planned::Snap(Box::new(job))),
                Err(m) => Err(PlanAction::Skip(m)),
            },
        );
    }
    if let Some(p) = crate::processors::Provider::parse(&conn.provider) {
        let Ok(cred) = crate::processors::credential_parse(p, &credential) else {
            return skip("the stored credential is damaged; reconnect");
        };
        return Ok(Ok(Planned::Proc(crate::processors::plan(
            ctx, conn, p, cred, now,
        )?)));
    }
    let Ok(access) = simplefin::parse_access_url(credential.as_bytes()) else {
        return skip("the stored credential is damaged; reconnect");
    };
    let all_links = links(ctx, &conn.id)?;
    let linked: Vec<&LinkDoc> = all_links
        .iter()
        .filter(|l| l.active && l.account.is_some())
        .collect();
    if linked.is_empty() {
        return skip("no remote account is linked yet");
    }
    let today =
        simplefin::epoch_date(now, c.utc_offset_secs).map_err(|e| invalid(e.to_string()))?;
    let history = crate::module_settings::get_i64(ctx, "bank-feeds.history_days_first_sync")?;
    let history = i32::try_from(history).unwrap_or(90);
    let mut start = today;
    for l in &linked {
        let s = match l.last_success {
            Some(t) => simplefin::epoch_date(t, c.utc_offset_secs)
                .map_err(|e| invalid(e.to_string()))?
                .add_days(-OVERLAP_DAYS)
                .unwrap_or(today),
            None => l
                .sync_from
                .or_else(|| today.add_days(-history))
                .unwrap_or(today),
        };
        if s < start {
            start = s;
        }
    }
    if let Some(f) = from {
        start = f;
    }
    // connectivity.md revision 5: never ask SimpleFIN for more than 90 days.
    let earliest = today.add_days(-SIMPLEFIN_MAX_DAYS).unwrap_or(today);
    let capped = start < earliest;
    if capped {
        start = earliest;
    }
    let mut q = format!(
        "/accounts?version={}&start-date={}",
        conn.version,
        simplefin::date_epoch(start)
    );
    if linked.iter().any(|l| l.include_pending) {
        q.push_str("&pending=1");
    }
    for l in &linked {
        q.push_str("&account=");
        q.push_str(&query_escape(&l.remote_account_id));
    }
    Ok(Ok(Planned::Http(
        get_req(&access, &q, simplefin::MAX_RESPONSE_BYTES),
        capped,
    )))
}

/// Apply a sync's answers in one write.
fn apply_sync(
    ctx: &Ctx<'_>,
    c: &Connector,
    plans: &[SyncPlan],
    f: &Fetched,
) -> CommandResult<Value> {
    let responses = &f.responses;
    let processed = &f.processed;
    ctx.quiet_noop.set(true);
    let now = c.now();
    let mut out = Vec::new();
    for plan in plans {
        let mut conn = find_conn(ctx, &plan.conn)?;
        match &plan.action {
            PlanAction::Skip(r) => out.push(skip(&conn, r)),
            PlanAction::MissingCredential => {
                let m = "credential not found on this computer — reconnect";
                conn.status = "needs_attention".into();
                note_error(&mut conn, "credential_missing", m);
                save_conn(ctx, &conn)?;
                out.push(skip(&conn, m));
            }
            PlanAction::Fetch(i) => {
                conn.requests.retain(|t| now.saturating_sub(*t) < DAY);
                conn.requests.push(now);
                conn.last_sync_at = Some(now_rfc3339()?);
                let resp = responses
                    .get(*i)
                    .cloned()
                    .unwrap_or_else(|| Err(FetchError::Network("no answer".into())));
                let mut v = apply_fetch(ctx, c, &mut conn, now, resp)?;
                save_conn(ctx, &conn)?;
                if !plan.notes.is_empty() {
                    let mut notes = plan.notes.clone();
                    if let Some(n) = v.get("notes").and_then(Value::as_array) {
                        notes.extend(n.iter().filter_map(Value::as_str).map(str::to_owned));
                    }
                    if let Some(o) = v.as_object_mut() {
                        o.insert("notes".into(), json!(notes));
                    }
                }
                out.push(v);
            }
            PlanAction::Processor(i) => {
                conn.requests.retain(|t| now.saturating_sub(*t) < DAY);
                conn.requests.push(now);
                conn.last_sync_at = Some(now_rfc3339()?);
                let result = processed.get(*i).cloned().unwrap_or_else(|| {
                    Err(crate::processors::ProcError::Fetch(FetchError::Network(
                        "no answer".into(),
                    )))
                });
                let v = crate::processors::apply(ctx, c, &mut conn, now, result)?;
                save_conn(ctx, &conn)?;
                out.push(v);
            }
            PlanAction::Ofx(i) => {
                conn.requests.retain(|t| now.saturating_sub(*t) < DAY);
                conn.requests.push(now);
                conn.last_sync_at = Some(now_rfc3339()?);
                let result = f.ofx_out.get(*i).cloned().unwrap_or_else(|| {
                    crate::ofx_conn::OfxResult::Fetch(FetchError::Network("no answer".into()))
                });
                let v = crate::ofx_conn::apply(ctx, c, &mut conn, now, result)?;
                save_conn(ctx, &conn)?;
                out.push(v);
            }
            PlanAction::Snap(i) => {
                conn.requests.retain(|t| now.saturating_sub(*t) < DAY);
                conn.requests.push(now);
                conn.last_sync_at = Some(now_rfc3339()?);
                let result = f.snap_out.get(*i).cloned().unwrap_or_else(|| {
                    crate::snap_conn::SnapResult::Fetch(FetchError::Network("no answer".into()))
                });
                let v = crate::snap_conn::apply(ctx, c, &mut conn, now, result)?;
                save_conn(ctx, &conn)?;
                out.push(v);
            }
        }
    }
    Ok(json!({ "connections": out }))
}

pub(crate) fn error_json(conn: &ConnDoc) -> Value {
    json!({"connection": conn.id, "label": conn.label, "status": conn.status,
        "error": conn.last_error})
}

/// One connection's `/accounts` answer: statuses, links and the staged batch.
fn apply_fetch(
    ctx: &Ctx<'_>,
    c: &Connector,
    conn: &mut ConnDoc,
    now: i64,
    resp: Result<HttpResponse, FetchError>,
) -> CommandResult<Value> {
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            let code = match e {
                FetchError::Refused(_) => "refused",
                FetchError::Network(_) => "network",
            };
            note_error(conn, code, &e.to_string());
            backoff(conn, now);
            return Ok(error_json(conn));
        }
    };
    if resp.status != 200 {
        apply_status(conn, resp.status, now);
        return Ok(error_json(conn));
    }
    let set = match simplefin::parse_account_set(&resp.body) {
        Ok(s) => s,
        Err(e) => {
            note_error(
                conn,
                "bad_response",
                &format!("The provider's answer was refused: {e}"),
            );
            backoff(conn, now);
            return Ok(error_json(conn));
        }
    };
    let today =
        simplefin::epoch_date(now, c.utc_offset_secs).map_err(|e| invalid(e.to_string()))?;
    conn.status = "active".into();
    conn.last_error = None;
    conn.failures = 0;
    conn.next_retry_at = None;
    apply_set_meta(ctx, conn, &set, Some(&today.to_string()))?;
    let notes: Vec<String> = set
        .errors
        .iter()
        .filter(|e| informational(&e.code, &e.msg))
        .map(|e| e.msg.clone())
        .collect();
    let mut all_links = links(ctx, &conn.id)?;
    let (statement, auto, staged_links) = build_statement(ctx, c, conn, &set, &all_links)?;
    for l in all_links
        .iter_mut()
        .filter(|l| staged_links.contains(&l.id))
    {
        l.last_success = Some(now);
        save_link(ctx, l)?;
    }
    conn.last_success = Some(now);
    retire_pending(ctx, conn)?;
    if statement.row_count() == 0 {
        return Ok(
            json!({"connection": conn.id, "label": conn.label, "status": conn.status,
            "batch": null, "new": 0, "warnings": statement.warnings, "notes": notes}),
        );
    }
    let digest: String = Sha256::digest(&resp.body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let source = format!("SimpleFIN — {}", conn.label);
    let staged = crate::import::stage_statement_with(
        ctx,
        statement,
        "simplefin",
        Some(source),
        digest,
        None,
        None,
        None,
    )?;
    let batch_id = staged
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut b = crate::import::load_batch(ctx, &batch_id)?;
    b.feed = Some(conn.id.clone());
    crate::import::save_batch(ctx, &b)?;
    let staged = crate::import::sealed_json(ctx, &batch_id)?;
    let counts = staged.get("counts").cloned().unwrap_or(Value::Null);
    let new = counts.get("new").and_then(Value::as_u64).unwrap_or(0);
    let mut committed = Value::Null;
    if auto && crate::import::auto_committable(&staged) {
        committed = crate::import::commit_batch(ctx, &batch_id)?;
    }
    // The audit entry keeps the sync's own arguments.
    *ctx.audit_args.borrow_mut() = None;
    Ok(json!({
        "connection": conn.id, "label": conn.label, "status": conn.status,
        "batch": batch_id, "counts": counts, "new": new, "committed": committed,
        "errors": conn.errors, "notes": notes,
    }))
}

// ----- credential index (unused credentials) -------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexEntry {
    id: String,
    /// Epoch seconds the credential was stored.
    at: i64,
}

fn index_key(book: &str) -> String {
    format!("{book}:index")
}

fn index_parse(raw: Option<String>) -> Vec<IndexEntry> {
    raw.and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_default()
}

fn index_text(idx: &[IndexEntry]) -> String {
    serde_json::to_string(idx).unwrap_or_else(|_| "[]".into())
}

/// Change the OS store's credential index (best effort: the index only feeds the
/// "Unused credential" list).
fn os_index_change(c: &Connector, book: &str, f: impl FnOnce(&mut Vec<IndexEntry>)) {
    let key = index_key(book);
    let mut idx = index_parse(c.secrets.get(&key).ok().flatten());
    f(&mut idx);
    let _ = c.secrets.put(&key, &index_text(&idx));
}

fn book_index_change(
    ctx: &Ctx<'_>,
    book: &str,
    f: impl FnOnce(&mut Vec<IndexEntry>),
) -> CommandResult<()> {
    let key = index_key(book);
    let mut idx = index_parse(ctx.tx.get_secret(&key)?);
    f(&mut idx);
    ctx.tx.put_secret(&key, &index_text(&idx))?;
    Ok(())
}

fn unused(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let book = ctx.tx.info()?.id;
    let now = ctx.connector.map(Connector::now).unwrap_or(i64::MAX);
    let known: Vec<String> = connections(ctx)?
        .into_iter()
        .filter_map(|c| c.secret_ref.map(|r| r.id))
        .collect();
    let mut entries: Vec<(IndexEntry, &str)> = index_parse(ctx.tx.get_secret(&index_key(&book))?)
        .into_iter()
        .map(|e| (e, "book"))
        .collect();
    if let Some(c) = ctx.connector.filter(|c| c.secrets.available()) {
        entries.extend(
            index_parse(c.secrets.get(&index_key(&book)).ok().flatten())
                .into_iter()
                .map(|e| (e, "os")),
        );
    }
    Ok(Value::Array(
        entries
            .into_iter()
            .filter(|(e, _)| !known.contains(&e.id) && now.saturating_sub(e.at) >= DAY)
            .map(|(e, store)| json!({"credential": e.id, "store": store, "stored_at": e.at}))
            .collect(),
    ))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CredentialRef {
    /// The credential's id (from `connect.unused`).
    credential: String,
}

fn delete_credential(ctx: &Ctx<'_>, a: CredentialRef) -> CommandResult<Value> {
    let book = ctx.tx.info()?.id;
    if !a.credential.starts_with(&format!("{book}:")) || a.credential == index_key(&book) {
        return Err(invalid("not a credential of this book"));
    }
    if connections(ctx)?
        .iter()
        .any(|c| c.secret_ref.as_ref().is_some_and(|r| r.id == a.credential))
    {
        return Err(rejected(
            "this credential belongs to a connection; disconnect it instead",
        ));
    }
    ctx.tx.delete_secret(&a.credential)?;
    book_index_change(ctx, &book, |idx| idx.retain(|e| e.id != a.credential))?;
    if let Some(c) = ctx.connector {
        let _ = c.secrets.delete(&a.credential);
        os_index_change(c, &book, |idx| idx.retain(|e| e.id != a.credential));
    }
    Ok(json!({"deleted": a.credential}))
}

pub(crate) fn query_escape(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// The statement of a fetch: one section per linked account that came back. Returns
/// the statement, whether every staged section commits automatically, and the links
/// staged.
fn build_statement(
    ctx: &Ctx<'_>,
    c: &Connector,
    conn: &ConnDoc,
    set: &AccountSet,
    all_links: &[LinkDoc],
) -> CommandResult<(Statement, bool, Vec<String>)> {
    let chart = Chart::load(ctx)?;
    let transacted =
        crate::module_settings::get_str(ctx, "bank-feeds.date_source")? == "transacted";
    let mut st = Statement::default();
    let mut auto = true;
    let mut staged = Vec::new();
    let names: BTreeMap<&str, &str> = conn
        .connections
        .iter()
        .map(|s| (s.conn_id.as_str(), s.name.as_str()))
        .collect();
    let mut line = 0_usize;
    for a in &set.accounts {
        let Some(l) = all_links
            .iter()
            .find(|l| l.remote_account_id == a.id && l.active)
        else {
            continue;
        };
        let Some(acc) = l
            .account
            .as_deref()
            .and_then(|x| x.parse::<AccountId>().ok())
            .and_then(|x| chart.get(x))
        else {
            continue;
        };
        let display = format!(
            "{} — {}",
            names
                .get(a.conn_id.as_str())
                .copied()
                .unwrap_or(&conn.label),
            a.name
        );
        let Some(code) = a.currency.as_deref() else {
            st.warnings.push(format!(
                "{display}: left out — its currency is not a standard currency code"
            ));
            continue;
        };
        if acc.commodity.as_str() != code {
            st.warnings.push(format!(
                "{display}: left out — it is in {code}, but {} is in {}",
                chart.full_name(acc.id),
                acc.commodity
            ));
            continue;
        }
        let section = section_of(c, l, a, display, code, transacted, &mut line)?;
        st.sections.push(section);
        staged.push(l.id.clone());
        auto &= l.auto_commit;
    }
    Ok((st, auto && !staged.is_empty(), staged))
}

fn section_of(
    c: &Connector,
    l: &LinkDoc,
    a: &SfAccount,
    display: String,
    code: &str,
    transacted: bool,
    line: &mut usize,
) -> CommandResult<Section> {
    let off = c.utc_offset_secs;
    let date = |t: i64| simplefin::epoch_date(t, off).map_err(|e| invalid(e.to_string()));
    let mut rows = Vec::new();
    for t in &a.transactions {
        if t.pending && !l.include_pending {
            continue;
        }
        *line += 1;
        rows.push(
            simplefin::row_of(t, transacted, off, *line).map_err(|e| invalid(e.to_string()))?,
        );
    }
    let closing = if a.balance_date > 0 {
        Some(ClosingBalance {
            date: date(a.balance_date)?,
            amount: a.balance,
        })
    } else {
        None
    };
    let available = match (a.available, &closing) {
        (Some(v), Some(cl)) => Some(ClosingBalance {
            date: cl.date,
            amount: v,
        }),
        _ => None,
    };
    let period = match (
        rows.iter().map(|r| r.date).min(),
        rows.iter().map(|r| r.date).max(),
    ) {
        (Some(x), Some(y)) => Some((x, y)),
        _ => None,
    };
    Ok(Section {
        account_key: Some(l.account_key.clone()),
        account_name: Some(display),
        account_hint: None,
        currency: CommodityCode::new(code).ok(),
        period,
        closing,
        available,
        rows,
        ..Section::default()
    })
}

/// Pending rows of this connection's earlier staged batches are skipped: the new
/// fetch stages their posted (or still pending) version, and a pending transaction
/// that disappeared at the bank is dropped.
fn retire_pending(ctx: &Ctx<'_>, conn: &ConnDoc) -> CommandResult<()> {
    for d in ctx.tx.docs(DocTable::ImportBatch, None)? {
        let Ok(b) = serde_json::from_value::<crate::import::BatchDoc>(d.json) else {
            continue;
        };
        if b.status != "staged" || b.feed.as_deref() != Some(conn.id.as_str()) {
            continue;
        }
        let mut changed = false;
        for mut r in crate::import::load_rows(ctx, &b.id)? {
            if r.raw.pending && r.overrides.action != Some(crate::import::RowAction::Skip) {
                r.overrides.action = Some(crate::import::RowAction::Skip);
                r.action = crate::import::RowAction::Skip;
                crate::import::save_row(ctx, &r)?;
                changed = true;
            }
        }
        if changed {
            crate::import::sealed_json(ctx, &b.id)?;
        }
    }
    Ok(())
}

fn disconnect(ctx: &Ctx<'_>, a: ConnRef) -> CommandResult<Value> {
    let mut conn = find_conn(ctx, &a.connection)?;
    if let Some(r) = conn.secret_ref.take() {
        delete_secret(ctx, ctx.connector, &r)?;
    }
    conn.status = "revoked".into();
    note_error(
        &mut conn,
        "disconnected",
        "Disconnected: the credential was deleted. Reconnect with a new setup token to sync again.",
    );
    save_conn(ctx, &conn)?;
    for mut l in links(ctx, &conn.id)? {
        l.active = false;
        save_link(ctx, &l)?;
    }
    let mut v = conn_json(ctx, &conn)?;
    if let Some(o) = v.as_object_mut() {
        o.insert(
            "message".into(),
            json!(match conn.provider.as_str() {
                crate::ofx_conn::PROVIDER => "The stored user id and password were deleted from this computer.",
                crate::snap_conn::PROVIDER => "The personal key was deleted from this computer; you can also delete it in the SnapTrade dashboard.",
                _ => "SimpleFIN cannot be told to revoke access from here: also revoke MoneyKeep's access on your provider's site.",
            }),
        );
    }
    Ok(v)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LabelArgs {
    connection: String,
    label: String,
}

fn set_label(ctx: &Ctx<'_>, a: LabelArgs) -> CommandResult<Value> {
    let mut conn = find_conn(ctx, &a.connection)?;
    let label = mk_core::text::clean(&a.label, 100).text.trim().to_owned();
    if label.is_empty() {
        return Err(invalid("give the label"));
    }
    conn.label = label;
    save_conn(ctx, &conn)?;
    conn_json(ctx, &conn)
}

fn disable(ctx: &Ctx<'_>, a: ConnRef) -> CommandResult<Value> {
    let mut conn = find_conn(ctx, &a.connection)?;
    if conn.status == "revoked" {
        return Err(rejected("the connection is revoked; reconnect instead"));
    }
    conn.status = "disabled".into();
    save_conn(ctx, &conn)?;
    conn_json(ctx, &conn)
}

fn enable(ctx: &Ctx<'_>, a: ConnRef) -> CommandResult<Value> {
    let mut conn = find_conn(ctx, &a.connection)?;
    if conn.status != "disabled" {
        return Err(rejected("the connection is not disabled"));
    }
    conn.status = "active".into();
    save_conn(ctx, &conn)?;
    conn_json(ctx, &conn)
}

/// Home's attention items for feeds: staged feed batches and connections needing
/// attention (used by `connect.attention`).
pub(crate) fn attention(ctx: &Ctx<'_>) -> CommandResult<Value> {
    let mut items = Vec::new();
    let conns = connections(ctx)?;
    for d in ctx.tx.docs(DocTable::ImportBatch, None)? {
        let Ok(b) = serde_json::from_value::<crate::import::BatchDoc>(d.json) else {
            continue;
        };
        if b.status != "staged" {
            continue;
        }
        let Some(feed) = b.feed.as_deref() else {
            continue;
        };
        let rows = crate::import::load_rows(ctx, &b.id)?;
        let waiting: Vec<&crate::import::RowDoc> = rows
            .iter()
            .filter(|r| {
                r.class == crate::import::Class::New && r.action != crate::import::RowAction::Skip
            })
            .collect();
        let n = waiting.len();
        if n == 0 {
            continue;
        }
        // connectivity.md revision 4: how many wait for each MoneyKeep account, so a
        // register can offer Review.
        let mut per: Vec<(AccountId, usize)> = Vec::new();
        for r in &waiting {
            let Some(acc) = b
                .sections
                .iter()
                .find(|s| s.index == r.section)
                .and_then(|s| s.account)
            else {
                continue;
            };
            match per.iter_mut().find(|(a, _)| *a == acc) {
                Some((_, k)) => *k += 1,
                None => per.push((acc, 1)),
            }
        }
        let per_account: Vec<Value> = per
            .iter()
            .map(|(a, k)| json!({"account": a, "count": k}))
            .collect();
        let provider = match conns.iter().find(|c| c.id == feed) {
            Some(c) if c.provider == crate::ofx_conn::PROVIDER => "OFX",
            Some(c) if c.provider == crate::snap_conn::PROVIDER => "SnapTrade",
            Some(c) => crate::processors::Provider::parse(&c.provider)
                .map_or("SimpleFIN", crate::processors::Provider::title),
            None => "Bank feed",
        };
        let what = match (n == 1, b.format == "processor") {
            (true, true) => "row",
            (false, true) => "rows",
            (true, false) => "transaction",
            (false, false) => "transactions",
        };
        items.push(json!({"kind": "batch", "batch": b.id, "connection": feed,
            "provider": provider, "count": n, "accounts": per_account,
            "message": format!("{provider}: {n} new {what} to review")}));
    }
    for c in &conns {
        items.extend(crate::ofx_conn::attention_items(c));
        let provider = if c.provider == crate::snap_conn::PROVIDER {
            "SnapTrade"
        } else {
            "your bank"
        };
        for l in links(ctx, &c.id)?.iter().filter(|l| l.active) {
            let Some(account) = l
                .account
                .as_deref()
                .and_then(|a| a.parse::<AccountId>().ok())
            else {
                continue;
            };
            let n = l
                .remote
                .as_ref()
                .and_then(|r| r.holdings.as_ref())
                .and_then(|h| h.get("differences"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if n > 0 {
                let chart = Chart::load(ctx)?;
                items.push(
                    json!({"kind": "holdings", "connection": c.id, "account": account,
                    "message": format!("{}: {n} holding{} differ{} from {provider}",
                        chart.full_name(account), if n == 1 { "" } else { "s" },
                        if n == 1 { "s" } else { "" })}),
                );
            }
        }
        if matches!(c.status.as_str(), "needs_attention" | "revoked")
            && c.last_error
                .as_ref()
                .is_none_or(|e| e.code != "disconnected")
        {
            let why = conn_notice(c);
            if c.dismissed.as_deref() != Some(why.as_str()) {
                items.push(json!({"kind": "connection", "connection": c.id,
                    "message": format!("{}: {why}", c.label),
                    "dismissible": true,
                    "dismiss": {"connection": c.id, "message": why}}));
            }
        }
        for l in links(ctx, &c.id)?
            .iter()
            .filter(|l| l.active && l.account.is_some())
        {
            if let Some(a) = &l.attention
                && l.dismissed.as_deref() != Some(a.as_str())
            {
                items.push(json!({"kind": "connection", "connection": c.id,
                    "link": l.id,
                    "message": format!("{} — {}: {a}", c.label, l.name),
                    "dismissible": true,
                    "dismiss": {"connection": c.id, "link": l.id, "message": a}}));
            }
        }
    }
    Ok(Value::Array(items))
}

/// The text of a connection's Attention notice (without its label).
fn conn_notice(c: &ConnDoc) -> String {
    c.last_error
        .as_ref()
        .map_or_else(|| status_label(&c.status).to_owned(), |e| e.message.clone())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DismissArgs {
    /// Connection id or label.
    connection: String,
    /// The remote account's link id (or remote account id) for an account's notice.
    #[serde(default)]
    link: Option<String>,
    /// The notice's message, as `connect.attention` gives it in `dismiss.message`
    /// (the full Attention line is accepted too).
    message: String,
}

/// Dismiss a connection's or remote account's Attention notice (connectivity.md
/// revision 5). Undoable and audited like any change.
fn dismiss(ctx: &Ctx<'_>, a: DismissArgs) -> CommandResult<Value> {
    let mut conn = find_conn(ctx, &a.connection)?;
    let msg = a.message.trim();
    match a.link.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(r) => {
            let mut l = links(ctx, &conn.id)?
                .into_iter()
                .find(|l| l.id == r || l.remote_account_id == r)
                .ok_or_else(|| CommandError::NotFound(format!("remote account {r:?}")))?;
            let Some(att) = l.attention.clone() else {
                return Err(rejected("that account has no notice to dismiss"));
            };
            let full = format!("{} — {}: {att}", conn.label, l.name);
            if msg != att && msg != full {
                return Err(rejected("that notice is no longer shown"));
            }
            l.dismissed = Some(att.clone());
            save_link(ctx, &l)?;
            Ok(json!({"connection": conn.id, "link": l.id, "dismissed": att}))
        }
        None => {
            if !matches!(conn.status.as_str(), "needs_attention" | "revoked") {
                return Err(rejected("the connection has no notice to dismiss"));
            }
            let why = conn_notice(&conn);
            let full = format!("{}: {why}", conn.label);
            if msg != why && msg != full {
                return Err(rejected("that notice is no longer shown"));
            }
            conn.dismissed = Some(why.clone());
            // A "Needs attention" status that came only from this message returns to
            // Active; Revoked stays.
            if conn.status == "needs_attention" {
                conn.status = "active".into();
            }
            save_conn(ctx, &conn)?;
            Ok(json!({"connection": conn.id, "dismissed": why, "status": conn.status}))
        }
    }
}

// ----- a scripted provider for tests and reviews ------------------------------------------

/// A scripted SimpleFIN server answering through the [`Fetcher`] interface (tests and
/// screen reviews; no network). Clone it to keep a handle for changing its answers.
#[derive(Debug, Clone, Default)]
pub struct FakeSimplefin {
    pub state: std::sync::Arc<std::sync::Mutex<FakeState>>,
}

#[derive(Debug, Default)]
pub struct FakeState {
    /// Claim tokens already used (their last path segment).
    pub used: Vec<String>,
    /// `/info` versions.
    pub versions: Vec<String>,
    /// The `/accounts` body.
    pub accounts: String,
    /// Answer `/accounts` with this status instead.
    pub status: Option<u16>,
    /// Requests seen: (method, url, had Basic credentials).
    pub requests: Vec<(HttpMethod, String, bool)>,
    /// The Access URL handed out by claims.
    pub access_url: String,
    /// Delay transaction fetches (`/accounts` with a start date) by this long.
    pub delay_ms: u64,
    /// Answer `/accounts` of balances (the account listing) with this status instead.
    pub list_status: Option<u16>,
}

impl FakeSimplefin {
    pub fn new(accounts: &str) -> Self {
        let f = Self::default();
        if let Ok(mut s) = f.state.lock() {
            s.versions = vec!["1".into(), "2".into()];
            s.accounts = accounts.to_owned();
            s.access_url = "https://demo:s3cret-pass@bridge.test/simplefin".into();
        }
        f
    }

    pub fn set_accounts(&self, body: &str) {
        if let Ok(mut s) = self.state.lock() {
            s.accounts = body.to_owned();
        }
    }

    pub fn set_status(&self, status: Option<u16>) {
        if let Ok(mut s) = self.state.lock() {
            s.status = status;
        }
    }

    pub fn requests(&self) -> Vec<(HttpMethod, String, bool)> {
        self.state
            .lock()
            .map(|s| s.requests.clone())
            .unwrap_or_default()
    }

    /// A setup token for `https://bridge.test/simplefin/claim/<id>`.
    pub fn token(id: &str) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .encode(format!("https://bridge.test/simplefin/claim/{id}"))
    }
}

impl Fetcher for FakeSimplefin {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, FetchError> {
        let delay = self.state.lock().map(|s| s.delay_ms).unwrap_or_default();
        if delay > 0 && req.url.contains("start-date=") {
            std::thread::sleep(std::time::Duration::from_millis(delay));
        }
        let mut s = self
            .state
            .lock()
            .map_err(|_| FetchError::Network("fake server unavailable".into()))?;
        s.requests
            .push((req.method, req.url.clone(), req.basic.is_some()));
        let path = req.url.split('?').next().unwrap_or_default().to_owned();
        let ok = |body: &str| {
            Ok(HttpResponse {
                status: 200,
                body: body.as_bytes().to_vec(),
                retry_after: None,
                deprecation: None,
            })
        };
        if req.method == HttpMethod::Post && path.contains("/claim/") {
            let id = path.rsplit('/').next().unwrap_or_default().to_owned();
            if s.used.contains(&id) {
                return Ok(HttpResponse {
                    status: 403,
                    body: Vec::new(),
                    retry_after: None,
                    deprecation: None,
                });
            }
            s.used.push(id);
            let url = s.access_url.clone();
            return ok(&url);
        }
        if path.ends_with("/info") {
            let v = json!({"versions": s.versions}).to_string();
            return ok(&v);
        }
        if path.ends_with("/accounts") {
            if req.basic.is_none() {
                return Ok(HttpResponse {
                    status: 403,
                    body: Vec::new(),
                    retry_after: None,
                    deprecation: None,
                });
            }
            let listing = req.url.contains("balances-only=1");
            if let (true, Some(st)) = (listing, s.list_status) {
                return Ok(HttpResponse {
                    status: st,
                    body: Vec::new(),
                    retry_after: None,
                    deprecation: None,
                });
            }
            if let Some(st) = s.status {
                return Ok(HttpResponse {
                    status: st,
                    body: Vec::new(),
                    retry_after: None,
                    deprecation: None,
                });
            }
            if s.accounts.len() > req.max_bytes {
                return Err(FetchError::Refused("the answer is too large".into()));
            }
            let body = s.accounts.clone();
            return ok(&body);
        }
        Ok(HttpResponse {
            status: 404,
            body: Vec::new(),
            retry_after: None,
            deprecation: None,
        })
    }
}

/// An in-memory credential store (tests; `available: false` plays a computer without
/// one).
#[derive(Debug, Clone, Default)]
pub struct MemorySecrets {
    pub available: bool,
    pub values: std::sync::Arc<std::sync::Mutex<BTreeMap<String, String>>>,
}

impl MemorySecrets {
    pub fn new(available: bool) -> Self {
        Self {
            available,
            values: std::sync::Arc::default(),
        }
    }

    /// The stored credentials (the "Unused credential" index left out).
    pub fn all(&self) -> BTreeMap<String, String> {
        self.values
            .lock()
            .map(|v| {
                v.iter()
                    .filter(|(k, _)| !k.ends_with(":index"))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl SecretStore for MemorySecrets {
    fn available(&self) -> bool {
        self.available
    }
    fn put(&self, id: &str, secret: &str) -> Result<(), String> {
        if !self.available {
            return Err("no credential store".into());
        }
        self.values
            .lock()
            .map_err(|_| "locked".to_owned())?
            .insert(id.to_owned(), secret.to_owned());
        Ok(())
    }
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        Ok(self
            .values
            .lock()
            .map_err(|_| "locked".to_owned())?
            .get(id)
            .cloned())
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.values
            .lock()
            .map_err(|_| "locked".to_owned())?
            .remove(id);
        Ok(())
    }
}

// ----- background runs ------------------------------------------------------------------

/// Runs network commands in the background for a front end that owns a [`Session`]:
/// requests go out on their own threads with no book transaction open, while the
/// session keeps serving other commands; [`NetRunner::poll`] applies finished answers
/// in short writes.
#[derive(Default)]
pub struct NetRunner {
    next: u64,
    jobs: Vec<(u64, std::sync::Arc<std::sync::atomic::AtomicBool>)>,
    done: std::sync::Arc<std::sync::Mutex<Vec<(u64, Fetched)>>>,
    finished: Vec<(u64, CommandResult<Value>)>,
}

impl std::fmt::Debug for NetRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetRunner")
            .field("running", &self.running())
            .finish_non_exhaustive()
    }
}

impl NetRunner {
    /// Start a network command; returns its job id.
    pub fn start(&mut self, s: &mut crate::Session, command: &str, args: Value) -> u64 {
        self.next += 1;
        let id = self.next;
        match s.net_start(command, args) {
            Ok(NetStep::Fetch(job)) => self.spawn(id, job),
            Ok(NetStep::Done(v)) => self.finished.push((id, Ok(v))),
            Err(e) => self.finished.push((id, Err(e))),
        }
        id
    }

    fn spawn(&mut self, id: u64, job: NetJob) {
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.jobs.push((id, cancel.clone()));
        let done = self.done.clone();
        let spawned = std::thread::Builder::new()
            .name("mk-net".into())
            .spawn(move || {
                let f = job.fetch(&cancel);
                if let Ok(mut d) = done.lock() {
                    d.push((id, f));
                }
            });
        if spawned.is_err() {
            self.jobs.retain(|j| j.0 != id);
            self.finished.push((
                id,
                Err(CommandError::Rejected("could not start the request".into())),
            ));
        }
    }

    /// Stop waiting for a job: its answers are discarded and nothing is recorded.
    pub fn cancel(&mut self, id: u64) {
        if let Some((_, c)) = self.jobs.iter().find(|j| j.0 == id) {
            c.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Jobs still waiting for answers.
    pub fn running(&self) -> Vec<u64> {
        self.jobs.iter().map(|j| j.0).collect()
    }

    /// Apply finished answers; returns the jobs that ended.
    pub fn poll(&mut self, s: &mut crate::Session) -> Vec<(u64, CommandResult<Value>)> {
        let ready: Vec<(u64, Fetched)> = self
            .done
            .lock()
            .map(|mut d| std::mem::take(&mut *d))
            .unwrap_or_default();
        for (id, f) in ready {
            self.jobs.retain(|j| j.0 != id);
            match s.net_resume(f) {
                Ok(NetStep::Fetch(job)) => self.spawn(id, job),
                Ok(NetStep::Done(v)) => self.finished.push((id, Ok(v))),
                Err(e) => self.finished.push((id, Err(e))),
            }
        }
        std::mem::take(&mut self.finished)
    }
}
