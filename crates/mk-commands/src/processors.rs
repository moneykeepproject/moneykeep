//! Payment processors (connectivity-processors.md): Stripe, Square and PayPal payouts
//! staged through a clearing account. Requests run through the bank-feed network steps
//! (no book transaction open); answers are parsed by `mk_import::processors`.
//!
//! Endpoints, from the vendors' API documentation (checked in connectivity-processors.md
//! revision 2):
//! - Stripe: Basic auth with the restricted key as user name
//!   (<https://docs.stripe.com/api/authentication.md>);
//!   `GET /v1/payouts?created[gte]=…&created[lte]=…&limit=100&starting_after=…` lists
//!   payouts with their own status (<https://docs.stripe.com/api/payouts/list>);
//!   `GET /v1/balance_transactions?payout=po_…&limit=100&starting_after=…` lists what a
//!   payout contains, and `GET /v1/balance_transactions?type=stripe_fee&created[gte]=…`
//!   the fees, of which those in no payout read are staged on their own
//!   (<https://docs.stripe.com/api/balance_transactions/list.md>); pages continue while
//!   `has_more`. The connection test is `GET /v1/payouts?limit=1`.
//! - Square (<https://developer.squareup.com/reference/square/payouts-api/list-payouts>,
//!   <https://developer.squareup.com/reference/square/payouts-api/list-payout-entries>):
//!   Bearer token and `Square-Version` on every request; `GET /v2/locations`
//!   (MERCHANT_PROFILE_READ),
//!   `GET /v2/payouts?location_id=…&begin_time=…&end_time=…&limit=100&cursor=…` and
//!   `GET /v2/payouts/{payout_id}/payout-entries?limit=100&cursor=…` (PAYOUTS_READ);
//!   amounts in the currency's smallest unit.
//! - PayPal (<https://developer.paypal.com/docs/api/transaction-search/v1/>): a token from
//!   `POST /v1/oauth2/token` (client credentials, Basic auth with the client id and
//!   secret, body `grant_type=client_credentials`), then
//!   `GET /v1/reporting/transactions?start_date=…&end_date=…&fields=all&page_size=500&page=…`
//!   with windows of at most 31 days.

use std::collections::BTreeMap;

use mk_core::{AccountId, AccountSubtype, AccountType, CommodityCode, Date, Decimal};
use mk_import::processors::{self as pp, Category, Entry, Payout, ProcessorData};
use mk_import::{InvoiceApply, RawRow, RawSplit, Section, Statement};
use mk_store::{Doc, DocTable};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::connect::{
    ConnDoc, Connector, FetchError, HttpMethod, HttpRequest, HttpResponse, backoff, note_error,
};
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) const MODULE: &str = "payment-processing";
/// The Square API version sent with every request
/// (<https://developer.squareup.com/docs/build-basics/versioning-overview>); bumped
/// deliberately after checking the changes, never "latest".
pub const SQUARE_VERSION: &str = "2026-09-16";
/// Days re-read before the last successful sync.
const OVERLAP_DAYS: i64 = 10;

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

fn rejected(m: impl Into<String>) -> CommandError {
    CommandError::Rejected(m.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Provider {
    Stripe,
    Square,
    PayPal,
}

impl Provider {
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "stripe" => Some(Self::Stripe),
            "square" => Some(Self::Square),
            "paypal" => Some(Self::PayPal),
            _ => None,
        }
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Stripe => "stripe",
            Self::Square => "square",
            Self::PayPal => "paypal",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Stripe => "Stripe",
            Self::Square => "Square",
            Self::PayPal => "PayPal",
        }
    }

    /// The only host requests may go to.
    pub(crate) fn host(self) -> &'static str {
        match self {
            Self::Stripe => "api.stripe.com",
            Self::Square => "connect.squareup.com",
            Self::PayPal => "api-m.paypal.com",
        }
    }
}

/// A processor credential (never printed).
#[derive(Clone)]
pub(crate) enum Credential {
    Key(String),
    Client { id: String, secret: String },
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credential(hidden)")
    }
}

/// Check the pasted setup input before any request.
pub(crate) fn credential_from_input(p: Provider, input: &str) -> CommandResult<Credential> {
    let t = input.trim();
    match p {
        Provider::Stripe => {
            pp::check_stripe_key(t).map_err(|e| invalid(e.to_string()))?;
            Ok(Credential::Key(t.to_owned()))
        }
        Provider::Square => {
            if t.len() < 16 || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(invalid("paste the Square personal access token"));
            }
            Ok(Credential::Key(t.to_owned()))
        }
        Provider::PayPal => {
            let (id, secret) = match serde_json::from_str::<Value>(t) {
                Ok(v) => (
                    v.get("client_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    v.get("client_secret")
                        .or_else(|| v.get("secret"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                ),
                Err(_) => {
                    let (a, b) = t.split_once(':').unwrap_or((t, ""));
                    (a.trim().to_owned(), b.trim().to_owned())
                }
            };
            if id.is_empty() || secret.is_empty() {
                return Err(invalid(
                    "give the PayPal REST app's client id and secret (client_id:secret)",
                ));
            }
            Ok(Credential::Client { id, secret })
        }
    }
}

/// The stored form of a credential.
pub(crate) fn credential_text(c: &Credential) -> String {
    match c {
        Credential::Key(k) => k.clone(),
        Credential::Client { id, secret } => {
            json!({"client_id": id, "client_secret": secret}).to_string()
        }
    }
}

pub(crate) fn credential_parse(p: Provider, text: &str) -> CommandResult<Credential> {
    credential_from_input(p, text)
        .map_err(|_| rejected("the stored credential is damaged; reconnect"))
}

/// A sync's processor work, done with no book transaction open.
#[derive(Clone)]
pub struct ProcJob {
    pub(crate) provider: Provider,
    pub(crate) credential: Credential,
    /// Epoch seconds.
    pub(crate) since: i64,
    pub(crate) until: i64,
}

impl std::fmt::Debug for ProcJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcJob")
            .field("provider", &self.provider)
            .field("since", &self.since)
            .field("until", &self.until)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum ProcError {
    Status(u16),
    Fetch(FetchError),
    Parse(String),
    Cancelled,
    /// Square refused a request (401/403): the permission it needs.
    Permission(u16, &'static str),
}

fn url(p: Provider, path_query: &str) -> String {
    format!("https://{}{path_query}", p.host())
}

fn request(p: Provider, cred: &Credential, token: Option<&str>, path_query: &str) -> HttpRequest {
    let mut r = HttpRequest {
        method: HttpMethod::Get,
        url: url(p, path_query),
        authority: p.host().into(),
        max_bytes: pp::MAX_RESPONSE_BYTES,
        headers: vec![("Accept".into(), "application/json".into())],
        ..HttpRequest::default()
    };
    match (p, cred) {
        (Provider::Stripe, Credential::Key(k)) => r.basic = Some((k.clone(), String::new())),
        (Provider::Square, Credential::Key(k)) => {
            r.headers
                .push(("Authorization".into(), format!("Bearer {k}")));
            r.headers
                .push(("Square-Version".into(), SQUARE_VERSION.into()));
        }
        _ => {
            if let Some(t) = token {
                r.headers
                    .push(("Authorization".into(), format!("Bearer {t}")));
            }
        }
    }
    r
}

fn token_request(cred: &Credential) -> HttpRequest {
    let (id, secret) = match cred {
        Credential::Client { id, secret } => (id.clone(), secret.clone()),
        Credential::Key(k) => (k.clone(), String::new()),
    };
    HttpRequest {
        method: HttpMethod::Post,
        url: url(Provider::PayPal, "/v1/oauth2/token"),
        basic: Some((id, secret)),
        authority: Provider::PayPal.host().into(),
        max_bytes: 64 * 1024,
        headers: vec![
            ("Accept".into(), "application/json".into()),
            (
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            ),
        ],
        body: Some(b"grant_type=client_credentials".to_vec()),
        allow_http: false,
    }
}

/// The request that checks a new credential.
pub(crate) fn verify_request(p: Provider, cred: &Credential) -> HttpRequest {
    match p {
        Provider::Stripe => request(p, cred, None, "/v1/payouts?limit=1"),
        Provider::Square => request(p, cred, None, "/v2/locations"),
        Provider::PayPal => token_request(cred),
    }
}

type Send<'a> = dyn FnMut(HttpRequest) -> Option<Result<HttpResponse, FetchError>> + 'a;

fn call(send: &mut Send<'_>, req: HttpRequest) -> Result<Vec<u8>, ProcError> {
    match send(req) {
        None => Err(ProcError::Cancelled),
        Some(Err(e)) => Err(ProcError::Fetch(e)),
        Some(Ok(r)) if r.status == 200 => Ok(r.body),
        Some(Ok(r)) => Err(ProcError::Status(r.status)),
    }
}

fn parse_err(e: mk_import::ImportError) -> ProcError {
    ProcError::Parse(e.to_string())
}

/// A Square call whose refusal names the permission it needs.
fn square_call(
    send: &mut Send<'_>,
    req: HttpRequest,
    perm: &'static str,
) -> Result<Vec<u8>, ProcError> {
    call(send, req).map_err(|e| match e {
        ProcError::Status(s @ (401 | 403)) => ProcError::Permission(s, perm),
        e => e,
    })
}

/// Every page of a Stripe list (`has_more` / `starting_after`).
fn stripe_list(job: &ProcJob, send: &mut Send<'_>, base: &str) -> Result<Vec<Value>, ProcError> {
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut q = base.to_owned();
        if let Some(a) = &after {
            q.push_str(&format!(
                "&starting_after={}",
                crate::connect::query_escape(a)
            ));
        }
        let (items, more) = pp::stripe_page(&call(
            send,
            request(job.provider, &job.credential, None, &q),
        )?)
        .map_err(parse_err)?;
        after = items
            .last()
            .and_then(|v| v.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        out.extend(items);
        if !more || after.is_none() {
            break;
        }
    }
    Ok(out)
}

/// Most pages read per list (a runaway guard).
const MAX_PAGES: usize = 1000;

/// Read a processor's payouts between `since` and `until`.
pub(crate) fn run(job: &ProcJob, send: &mut Send<'_>) -> Result<ProcessorData, ProcError> {
    match job.provider {
        Provider::Stripe => run_stripe(job, send),
        Provider::Square => run_square(job, send),
        Provider::PayPal => run_paypal(job, send),
    }
}

fn run_stripe(job: &ProcJob, send: &mut Send<'_>) -> Result<ProcessorData, ProcError> {
    let mut data = ProcessorData::default();
    let window = format!(
        "created%5Bgte%5D={}&created%5Blte%5D={}&limit=100",
        job.since, job.until
    );
    let heads = stripe_list(job, send, &format!("/v1/payouts?{window}"))?;
    let mut seen = std::collections::BTreeSet::new();
    for v in &heads {
        let mut po = pp::stripe_payout(v).map_err(parse_err)?;
        let q = format!(
            "/v1/balance_transactions?payout={}&limit=100",
            crate::connect::query_escape(&po.id)
        );
        for v in stripe_list(job, send, &q)? {
            let e = pp::stripe_entry(&v).map_err(parse_err)?;
            seen.insert(e.id.clone());
            if e.category != Category::Transfer {
                po.entries.push(e);
            }
        }
        data.payouts.push(po);
    }
    // Fees in no payout read (billed on their own) are staged as their own rows.
    let fees = stripe_list(
        job,
        send,
        &format!("/v1/balance_transactions?type=stripe_fee&{window}"),
    )?;
    for v in &fees {
        let e = pp::stripe_entry(v).map_err(parse_err)?;
        if !seen.contains(&e.id) {
            data.loose.push(e);
        }
    }
    Ok(data)
}

fn run_square(job: &ProcJob, send: &mut Send<'_>) -> Result<ProcessorData, ProcError> {
    let p = job.provider;
    let mut data = ProcessorData::default();
    let (locations, _) = pp::square_page(
        &square_call(
            send,
            request(p, &job.credential, None, "/v2/locations"),
            "MERCHANT_PROFILE_READ",
        )?,
        "locations",
    )
    .map_err(parse_err)?;
    for loc in &locations {
        let lid = loc
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if lid.is_empty() {
            continue;
        }
        let lname = mk_core::text::clean(
            loc.get("name").and_then(Value::as_str).unwrap_or_default(),
            200,
        )
        .text;
        let mut cursor: Option<String> = None;
        let mut payouts = Vec::new();
        for _ in 0..MAX_PAGES {
            let mut q = format!(
                "/v2/payouts?location_id={}&begin_time={}&end_time={}&limit=100",
                crate::connect::query_escape(&lid),
                crate::connect::query_escape(&pp::rfc3339(job.since)),
                crate::connect::query_escape(&pp::rfc3339(job.until)),
            );
            if let Some(c) = &cursor {
                q.push_str(&format!("&cursor={}", crate::connect::query_escape(c)));
            }
            let (items, next) = pp::square_page(
                &square_call(send, request(p, &job.credential, None, &q), "PAYOUTS_READ")?,
                "payouts",
            )
            .map_err(parse_err)?;
            for v in &items {
                let mut po = pp::square_payout(v).map_err(parse_err)?;
                po.location = Some(lid.clone());
                po.location_name = Some(lname.clone());
                payouts.push(po);
            }
            cursor = next;
            if cursor.is_none() {
                break;
            }
        }
        for mut po in payouts {
            let mut cursor: Option<String> = None;
            for _ in 0..MAX_PAGES {
                let mut q = format!(
                    "/v2/payouts/{}/payout-entries?limit=100",
                    crate::connect::query_escape(&po.id)
                );
                if let Some(c) = &cursor {
                    q.push_str(&format!("&cursor={}", crate::connect::query_escape(c)));
                }
                let (items, next) = pp::square_page(
                    &square_call(send, request(p, &job.credential, None, &q), "PAYOUTS_READ")?,
                    "payout_entries",
                )
                .map_err(parse_err)?;
                for v in &items {
                    po.entries.push(pp::square_entry(v).map_err(parse_err)?);
                }
                cursor = next;
                if cursor.is_none() {
                    break;
                }
            }
            data.payouts.push(po);
        }
    }
    Ok(data)
}

fn run_paypal(job: &ProcJob, send: &mut Send<'_>) -> Result<ProcessorData, ProcError> {
    let p = job.provider;
    // The token lives only in this function (never stored).
    let token =
        pp::paypal_token(&call(send, token_request(&job.credential))?).map_err(parse_err)?;
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    for (a, b) in pp::paypal_windows(job.since, job.until) {
        let mut page = 1_i64;
        loop {
            let q = format!(
                "/v1/reporting/transactions?start_date={}&end_date={}&fields=all&page_size=500&page={page}",
                crate::connect::query_escape(&pp::rfc3339(a)),
                crate::connect::query_escape(&pp::rfc3339(b)),
            );
            let (items, total, w) =
                pp::paypal_page(&call(send, request(p, &job.credential, Some(&token), &q))?)
                    .map_err(parse_err)?;
            entries.extend(items);
            warnings.extend(w);
            if page >= total || page >= i64::try_from(MAX_PAGES).unwrap_or(1000) {
                break;
            }
            page += 1;
        }
    }
    let (payouts, rest, mut loose) = pp::paypal_group(entries);
    // Fees after the last withdrawal are staged on their own; the rest waits.
    let (fees, pending): (Vec<Entry>, Vec<Entry>) =
        rest.into_iter().partition(|e| e.category == Category::Fee);
    loose.extend(fees);
    Ok(ProcessorData {
        payouts,
        pending,
        loose,
        warnings,
    })
}

// ----- settings ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct Posted {
    pub net: String,
    #[serde(default)]
    pub totals: BTreeMap<String, String>,
    #[serde(default)]
    pub changes: u32,
    /// Posted in detailed mode (the payout went through the transit account).
    #[serde(default)]
    pub detailed: bool,
    /// A reversal was staged after the payout failed.
    #[serde(default)]
    pub reversed: bool,
}

/// A processor connection's accounting choices.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct ProcSettings {
    /// `summary` or `detailed`.
    pub mode: String,
    /// The clearing account (id).
    #[serde(default)]
    pub clearing: Option<String>,
    /// Detailed mode: payouts in transit to the bank (id).
    #[serde(default)]
    pub transit: Option<String>,
    /// Category → account id.
    #[serde(default)]
    pub accounts: BTreeMap<String, String>,
    #[serde(default = "yes")]
    pub match_invoices: bool,
    /// Payouts staged: their net and category totals (changes are flagged).
    #[serde(default)]
    pub posted: BTreeMap<String, Posted>,
    /// PayPal: the first entry still waiting for a withdrawal (epoch seconds).
    #[serde(default)]
    pub pending_since: Option<i64>,
    /// PayPal: entries pending payout at the last sync.
    #[serde(default)]
    pub pending_count: usize,
    /// Payouts still on the way at the last sync: the oldest one's creation (epoch
    /// seconds), so the next sync reads it again.
    #[serde(default)]
    pub waiting_since: Option<i64>,
    /// Entries staged on their own (outside a payout); a later payout that contains
    /// one does not post it again.
    #[serde(default)]
    pub loose: std::collections::BTreeSet<String>,
    /// The latest withdrawal to the bank (epoch seconds).
    #[serde(default)]
    pub last_withdrawal: Option<i64>,
}

fn yes() -> bool {
    true
}

fn label(c: Category) -> &'static str {
    match c {
        Category::Sale => "Sales",
        Category::Refund => "Refunds",
        Category::Fee => "Fees",
        Category::Dispute => "Disputes",
        Category::DisputeReversal => "Dispute reversals",
        Category::Adjustment => "Adjustments",
        Category::Reserve => "Reserves",
        Category::Tax => "Tax",
        Category::Transfer => "Payouts",
        Category::Other => "Other",
    }
}

/// The categories a user maps (the payout itself is not one).
const MAPPED: [Category; 9] = [
    Category::Sale,
    Category::Refund,
    Category::Fee,
    Category::Dispute,
    Category::DisputeReversal,
    Category::Adjustment,
    Category::Reserve,
    Category::Tax,
    Category::Other,
];

fn account_id(text: &str) -> Option<AccountId> {
    text.parse().ok()
}

fn sales_account(ctx: &Ctx<'_>) -> CommandResult<AccountId> {
    crate::module_settings::account_or_create(
        ctx,
        "processors.sales_account",
        "Sales",
        AccountType::Income,
        AccountSubtype::Income,
    )
}

fn fees_account(ctx: &Ctx<'_>) -> CommandResult<AccountId> {
    crate::module_settings::account_or_create(
        ctx,
        "processors.fees_account",
        "Merchant Fees",
        AccountType::Expense,
        AccountSubtype::Expense,
    )
}

/// The account a category posts to; `None` leaves it uncategorized for review.
fn account_for(ctx: &Ctx<'_>, s: &ProcSettings, c: Category) -> CommandResult<Option<AccountId>> {
    if let Some(a) = s.accounts.get(c.key()).and_then(|x| account_id(x))
        && ctx.tx.account(a)?.is_some()
    {
        return Ok(Some(a));
    }
    Ok(match c {
        Category::Sale | Category::Refund | Category::Dispute | Category::DisputeReversal => {
            Some(sales_account(ctx)?)
        }
        Category::Fee => Some(fees_account(ctx)?),
        _ => None,
    })
}

fn named_clearing(ctx: &Ctx<'_>, name: &str) -> CommandResult<AccountId> {
    let chart = Chart::load(ctx)?;
    if let Some(a) = chart.accounts.iter().find(|a| {
        a.parent.is_none()
            && a.name.eq_ignore_ascii_case(name)
            && a.subtype == AccountSubtype::Clearing
    }) {
        return Ok(a.id);
    }
    let v = crate::accounts::create_json(
        ctx,
        json!({"name": name, "type": "asset", "subtype": "clearing"}),
    )?;
    v.get("id")
        .and_then(Value::as_str)
        .and_then(account_id)
        .ok_or_else(|| CommandError::Integrity("account without id".into()))
}

/// Settings for a new connection: the clearing account (created) and the book's mode.
pub(crate) fn new_settings(ctx: &Ctx<'_>, p: Provider) -> CommandResult<ProcSettings> {
    let clearing = named_clearing(ctx, &format!("{} Clearing", p.title()))?;
    let mode = crate::module_settings::get_str(ctx, "processors.default_mode")?;
    Ok(ProcSettings {
        mode: if mode == "detailed" {
            mode
        } else {
            "summary".into()
        },
        clearing: Some(clearing.to_string()),
        match_invoices: true,
        ..ProcSettings::default()
    })
}

/// The accounts a processor connection keeps balances in (clearing and transit).
pub(crate) fn held_accounts(conn: &ConnDoc) -> Vec<AccountId> {
    conn.processor
        .iter()
        .flat_map(|s| [s.clearing.as_deref(), s.transit.as_deref()])
        .flatten()
        .filter_map(account_id)
        .collect()
}

// ----- sync ---------------------------------------------------------------------------------

/// The window of a sync (epoch seconds).
pub(crate) fn plan(
    ctx: &Ctx<'_>,
    conn: &ConnDoc,
    p: Provider,
    credential: Credential,
    now: i64,
) -> CommandResult<ProcJob> {
    let history =
        crate::module_settings::get_i64(ctx, "processors.history_days_first_sync")?.clamp(1, 1095);
    let mut since = match conn.last_success {
        Some(t) => t.saturating_sub(OVERLAP_DAYS * 86_400),
        None => now.saturating_sub(history * 86_400),
    };
    for ps in conn
        .processor
        .iter()
        .flat_map(|s| [s.pending_since, s.waiting_since])
        .flatten()
    {
        since = since.min(ps);
    }
    if p == Provider::PayPal {
        // The provider keeps three years.
        since = since.max(now.saturating_sub(1095 * 86_400));
    }
    Ok(ProcJob {
        provider: p,
        credential,
        since,
        until: now,
    })
}

/// Apply a processor fetch in the sync's write.
pub(crate) fn apply(
    ctx: &Ctx<'_>,
    c: &Connector,
    conn: &mut ConnDoc,
    now: i64,
    result: Result<ProcessorData, ProcError>,
) -> CommandResult<Value> {
    let p = Provider::parse(&conn.provider).ok_or_else(|| invalid("unknown provider"))?;
    let data = match result {
        Ok(d) => d,
        Err(ProcError::Status(401 | 403)) => {
            conn.status = "needs_attention".into();
            note_error(
                conn,
                "rejected",
                &format!(
                    "{} rejected the credential — reconnect with a new key.",
                    p.title()
                ),
            );
            return Ok(crate::connect::error_json(conn));
        }
        Err(ProcError::Status(429)) => {
            note_error(
                conn,
                "rate_limited",
                &format!(
                    "{} asked MoneyKeep to slow down; it will try again later.",
                    p.title()
                ),
            );
            backoff(conn, now);
            return Ok(crate::connect::error_json(conn));
        }
        Err(ProcError::Status(s)) => {
            note_error(
                conn,
                "server",
                &format!(
                    "{} answered with HTTP {s}; MoneyKeep will try again later.",
                    p.title()
                ),
            );
            backoff(conn, now);
            return Ok(crate::connect::error_json(conn));
        }
        Err(ProcError::Fetch(e)) => {
            note_error(conn, "network", &e.to_string());
            backoff(conn, now);
            return Ok(crate::connect::error_json(conn));
        }
        Err(ProcError::Parse(m)) => {
            note_error(
                conn,
                "bad_response",
                &format!("The answer was refused: {m}"),
            );
            backoff(conn, now);
            return Ok(crate::connect::error_json(conn));
        }
        Err(ProcError::Permission(_, perm)) => {
            conn.status = "needs_attention".into();
            note_error(
                conn,
                "rejected",
                &format!(
                    "{} refused access: the access token needs the {perm} permission — reconnect with a token that has it.",
                    p.title()
                ),
            );
            return Ok(crate::connect::error_json(conn));
        }
        Err(ProcError::Cancelled) => {
            return Err(CommandError::Coded {
                code: "cancelled",
                message: "Cancelled; nothing was recorded.".into(),
            });
        }
    };
    conn.status = "active".into();
    conn.last_error = None;
    conn.failures = 0;
    conn.next_retry_at = None;
    conn.last_success = Some(now);
    let mut settings = conn.processor.clone().unwrap_or_default();
    let (statement, listed) = build(ctx, c, conn, p, &mut settings, &data)?;
    conn.processor = Some(settings);
    if statement.row_count() == 0 {
        return Ok(
            json!({"connection": conn.id, "label": conn.label, "status": conn.status,
            "batch": null, "new": 0, "warnings": statement.warnings, "listed": listed}),
        );
    }
    let source = format!("{} — {}", p.title(), conn.label);
    let digest = {
        use sha2::Digest;
        let text = serde_json::to_vec(&data).unwrap_or_default();
        sha2::Sha256::digest(&text)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let staged = crate::import::stage_statement_with(
        ctx,
        statement,
        "processor",
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
    *ctx.audit_args.borrow_mut() = None;
    Ok(json!({
        "connection": conn.id, "label": conn.label, "status": conn.status,
        "batch": batch_id, "counts": counts, "new": new, "listed": listed,
    }))
}

fn date_of(c: &Connector, t: i64) -> CommandResult<Date> {
    mk_import::simplefin::epoch_date(t, c.utc_offset_secs).map_err(|e| invalid(e.to_string()))
}

fn add(a: Decimal, b: Decimal) -> Decimal {
    a.checked_add(b).unwrap_or(a)
}

fn sub(a: Decimal, b: Decimal) -> Decimal {
    a.checked_sub(b).unwrap_or(a)
}

/// An open invoice named in a text (`<prefix><digits>`), with its balance.
fn invoice_in(ctx: &Ctx<'_>, text: &str) -> CommandResult<Option<String>> {
    if !crate::is_module_enabled(ctx.tx, crate::sales::INVOICING)? {
        return Ok(None);
    }
    let prefix = crate::module_settings::get_str(ctx, "invoicing.number_prefix")?;
    if prefix.is_empty() {
        return Ok(None);
    }
    let mut rest = text;
    while let Some(i) = rest.find(&prefix) {
        let after = rest.get(i + prefix.len()..).unwrap_or("");
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            let number = format!("{prefix}{digits}");
            if crate::sales::open_invoice_balance(ctx, &number)?.is_some() {
                return Ok(Some(number));
            }
        }
        rest = after;
    }
    Ok(None)
}

/// Category totals of a payout's entries: gross by category, fees as one negative
/// total. Entries already staged on their own (`skip`) are left out.
fn totals(
    entries: &[Entry],
    skip: &std::collections::BTreeSet<String>,
) -> BTreeMap<Category, Decimal> {
    let mut t: BTreeMap<Category, Decimal> = BTreeMap::new();
    for e in entries
        .iter()
        .filter(|e| e.category != Category::Transfer && !skip.contains(&e.id))
    {
        let x = t.entry(e.category).or_default();
        *x = add(*x, e.gross);
        if !e.fee.is_zero() {
            let f = t.entry(Category::Fee).or_default();
            *f = sub(*f, e.fee);
        }
    }
    t
}

fn split_for(
    ctx: &Ctx<'_>,
    chart: &Chart,
    s: &ProcSettings,
    c: Category,
    amount: Decimal,
) -> CommandResult<(RawSplit, bool)> {
    let acc = account_for(ctx, s, c)?;
    // A default account may have just been created: read the chart again.
    let fresh;
    let chart = if acc.is_some_and(|a| chart.get(a).is_none()) {
        fresh = Chart::load(ctx)?;
        &fresh
    } else {
        chart
    };
    let review = acc.is_none() || c == Category::Other;
    Ok((
        RawSplit {
            category_hint: acc.map(|a| chart.full_name(a)).unwrap_or_default(),
            amount,
            memo: label(c).into(),
            class_hint: None,
        },
        review,
    ))
}

fn breakdown(po: &Payout, t: &BTreeMap<Category, Decimal>) -> Value {
    // Every total is shown at the payout's own scale (e.g. "0.00", not "0").
    let scale = po.net.scale();
    let fixed = |mut d: Decimal| {
        if d.scale() < scale {
            d.rescale(scale);
        }
        d.to_string()
    };
    let get = |c: Category| fixed(t.get(&c).copied().unwrap_or_default());
    let other = t
        .iter()
        .filter(|(c, _)| !matches!(c, Category::Sale | Category::Refund | Category::Fee))
        .fold(Decimal::ZERO, |s, (_, v)| add(s, *v));
    json!({
        "payout": po.id, "gross": get(Category::Sale),
        "refunds": get(Category::Refund), "fees": get(Category::Fee),
        "other": fixed(other), "net": po.net.to_string(),
        "entries": po.entries.iter().filter(|e| e.category != Category::Transfer).map(|e| json!({
            "id": e.id, "kind": e.kind, "category": e.category.key(), "gross": e.gross.to_string(),
            "fee": e.fee.to_string(), "net": e.net().to_string(), "description": e.description,
        })).collect::<Vec<_>>(),
    })
}

fn base_row(
    external: String,
    date: Date,
    amount: Decimal,
    payee: String,
    memo: String,
    raw: String,
) -> RawRow {
    RawRow {
        date_kind: mk_import::DateKind::Unknown,
        external_id: Some(external),
        date,
        amount,
        payee,
        memo,
        number: String::new(),
        category_hint: String::new(),
        cleared: None,
        splits: Vec::new(),
        trntype: None,
        review: None,
        correct_fitid: None,
        correct_action: None,
        line: 0,
        raw,
        cleaned: Vec::new(),
        class_hint: None,
        pending: false,
        invoices: Vec::new(),
    }
}

/// The section a payout (or entry) goes to, created on first use; the pipeline maps
/// it to the clearing account by its key.
#[allow(clippy::too_many_arguments)]
fn section_key(
    ctx: &Ctx<'_>,
    sections: &mut BTreeMap<String, Section>,
    conn: &ConnDoc,
    p: Provider,
    clearing: AccountId,
    base: &str,
    location: Option<&str>,
    location_name: Option<&str>,
) -> CommandResult<String> {
    let key = match location {
        Some(l) => format!("processor:{}:{l}", conn.id),
        None => format!("processor:{}", conn.id),
    };
    if ctx.tx.get_doc(DocTable::ImportAccountMap, &key)?.is_none() {
        ctx.save_doc(
            DocTable::ImportAccountMap,
            &key,
            Some(&Doc {
                id: key.clone(),
                grp: None,
                ord: 0,
                json: json!({"key": key, "account": clearing}),
            }),
        )?;
    }
    let name = match location_name {
        Some(n) if !n.is_empty() => format!("{} — {n}", p.title()),
        _ => p.title().to_owned(),
    };
    sections.entry(key.clone()).or_insert_with(|| Section {
        account_key: Some(key.clone()),
        account_name: Some(name),
        currency: CommodityCode::new(base).ok(),
        ..Section::default()
    });
    Ok(key)
}

/// Where a posted payout's money went: the account on the other side of the transfer
/// out of the clearing account (the matched bank deposit) on or after the payout.
fn deposit_account(
    ctx: &Ctx<'_>,
    clearing: AccountId,
    net: Decimal,
    from: Date,
) -> CommandResult<Option<AccountId>> {
    for sp in ctx.tx.account_splits(clearing, Some(from), None)? {
        if sp.quantity != -net {
            continue;
        }
        if let Some(t) = ctx.tx.transaction(sp.txn_id)?
            && let Some(o) = t.splits.iter().find(|x| x.account != clearing)
        {
            return Ok(Some(o.account));
        }
    }
    Ok(None)
}

/// "on the way", "failed" and the like: statuses of payouts that are not posted.
fn is_failed(status: &str) -> bool {
    matches!(status, "failed" | "canceled" | "cancelled")
}

/// The statement of a processor fetch: sections per location (Square) or one, rows per
/// payout (summary) or per entry plus the payout (detailed), reversals of posted
/// payouts that failed, and entries outside any payout as their own rows.
#[allow(clippy::too_many_lines)]
fn build(
    ctx: &Ctx<'_>,
    c: &Connector,
    conn: &ConnDoc,
    p: Provider,
    s: &mut ProcSettings,
    data: &ProcessorData,
) -> CommandResult<(Statement, Vec<Value>)> {
    let base = ctx.tx.info()?.base_currency;
    let chart = Chart::load(ctx)?;
    let clearing = s
        .clearing
        .as_deref()
        .and_then(account_id)
        .filter(|a| chart.get(*a).is_some())
        .map_or_else(
            || named_clearing(ctx, &format!("{} Clearing", p.title())),
            Ok,
        )?;
    s.clearing = Some(clearing.to_string());
    let detailed = s.mode == "detailed";
    let transit = if detailed {
        let t = s
            .transit
            .as_deref()
            .and_then(account_id)
            .filter(|a| chart.get(*a).is_some())
            .map_or_else(
                || named_clearing(ctx, &format!("{} Payouts in Transit", p.title())),
                Ok,
            )?;
        s.transit = Some(t.to_string());
        Some(t)
    } else {
        None
    };
    let chart = Chart::load(ctx)?;
    let mut st = Statement::default();
    let mut listed = Vec::new();
    st.warnings.extend(data.warnings.iter().cloned());
    let mut sections: BTreeMap<String, Section> = BTreeMap::new();
    let mut line = 0_usize;
    let mut waiting: Option<i64> = None;
    // Entries staged on their own before this sync (a payout holding one leaves it out).
    let earlier_loose = s.loose.clone();
    for po in &data.payouts {
        if po.currency != base {
            st.warnings.push(format!(
                "{} payout {} is in {}: multi-currency needed; not posted",
                p.title(),
                po.id,
                po.currency
            ));
            listed.push(
                json!({"payout": po.id, "currency": po.currency, "net": po.net.to_string(),
                "reason": "multi-currency needed"}),
            );
            continue;
        }
        let shown_net = mk_core::currency::format_amount(po.net, Some(po.net.scale()));
        if is_failed(&po.status) {
            let why = po
                .failure_code
                .as_deref()
                .map(|f| format!(" ({f})"))
                .unwrap_or_default();
            let posted = s.posted.get(&po.id).cloned();
            match posted {
                Some(old) if !old.reversed => {
                    // Posted earlier: the money comes back to the processor balance.
                    let key = section_key(
                        ctx,
                        &mut sections,
                        conn,
                        p,
                        clearing,
                        &base,
                        po.location.as_deref(),
                        po.location_name.as_deref(),
                    )?;
                    let old_net: Decimal = old.net.parse().unwrap_or(po.net);
                    let counter = if old.detailed {
                        s.transit.as_deref().and_then(account_id)
                    } else {
                        deposit_account(ctx, clearing, old_net, date_of(c, po.arrival)?)?
                    };
                    let mut row = base_row(
                        format!("{}:{}#reversal", p.key(), po.id),
                        date_of(c, c.now())?,
                        old_net,
                        format!("{} payout reversal", p.title()),
                        po.id.clone(),
                        json!({"payout": po.id, "status": po.status, "failure_code": po.failure_code})
                            .to_string(),
                    );
                    row.splits.push(RawSplit {
                        category_hint: counter.map(|a| chart.full_name(a)).unwrap_or_default(),
                        amount: old_net,
                        memo: "Payout reversed".into(),
                        class_hint: None,
                    });
                    row.review = Some(format!(
                        "{} payout {} {}{why} after it was posted: {} goes back to {}; check the account it was paid to",
                        p.title(),
                        po.id,
                        po.status,
                        mk_core::currency::format_amount(old_net, Some(old_net.scale())),
                        chart.full_name(clearing)
                    ));
                    line += 1;
                    row.line = line;
                    if let Some(sec) = sections.get_mut(&key) {
                        sec.rows.push(row);
                    }
                    s.posted.insert(
                        po.id.clone(),
                        Posted {
                            reversed: true,
                            ..old
                        },
                    );
                }
                Some(_) => {}
                None => {
                    st.warnings.push(format!(
                        "{} payout {} of {shown_net} {}{why}: nothing posted",
                        p.title(),
                        po.id,
                        po.status
                    ));
                    listed.push(
                        json!({"payout": po.id, "status": po.status, "net": po.net.to_string(),
                        "failure_code": po.failure_code, "reason": "failed"}),
                    );
                }
            }
            continue;
        }
        if po.status != "paid" {
            st.warnings.push(format!(
                "{} payout {} of {shown_net} is on the way ({})",
                p.title(),
                po.id,
                po.status
            ));
            listed.push(
                json!({"payout": po.id, "status": po.status, "net": po.net.to_string(),
                "reason": "on the way"}),
            );
            let since = if po.created > 0 {
                po.created
            } else {
                po.arrival
            };
            waiting = Some(waiting.map_or(since, |w| w.min(since)));
            continue;
        }
        s.last_withdrawal = Some(s.last_withdrawal.map_or(po.arrival, |w| w.max(po.arrival)));
        let key = section_key(
            ctx,
            &mut sections,
            conn,
            p,
            clearing,
            &base,
            po.location.as_deref(),
            po.location_name.as_deref(),
        )?;
        let Some(section) = sections.get_mut(&key) else {
            continue;
        };
        let date = date_of(c, po.arrival)?;
        let t = totals(&po.entries, &earlier_loose);
        // Entries of this payout already posted on their own: their net is already in
        // the clearing account.
        let already = po
            .entries
            .iter()
            .filter(|e| e.category != Category::Transfer && earlier_loose.contains(&e.id))
            .fold(Decimal::ZERO, |a, e| add(a, e.net()));
        let sum = add(t.values().fold(Decimal::ZERO, |a, v| add(a, *v)), already);
        let raw = breakdown(po, &t).to_string();
        let ext = format!("{}:{}", p.key(), po.id);
        // A payout seen before with other amounts: stage the difference for review.
        let posted_now = Posted {
            net: po.net.to_string(),
            totals: t
                .iter()
                .map(|(k, v)| (k.key().to_owned(), v.to_string()))
                .collect(),
            changes: 0,
            detailed,
            reversed: false,
        };
        if let Some(old) = s.posted.get(&po.id).cloned()
            && (old.net != posted_now.net || old.totals != posted_now.totals)
        {
            let old_net: Decimal = old.net.parse().unwrap_or_default();
            let diff = sub(po.net, old_net);
            let mut row = base_row(
                format!("{ext}#{}", old.changes + 1),
                date,
                diff,
                format!("{} payout change", p.title()),
                po.id.clone(),
                raw.clone(),
            );
            for c2 in MAPPED {
                let new_v = t.get(&c2).copied().unwrap_or_default();
                let old_v: Decimal = old
                    .totals
                    .get(c2.key())
                    .and_then(|x| x.parse().ok())
                    .unwrap_or_default();
                let d = sub(new_v, old_v);
                if !d.is_zero() {
                    row.splits.push(split_for(ctx, &chart, s, c2, d)?.0);
                }
            }
            row.review = Some(format!(
                "this payout changed after it was posted: it was {old_net}, now {} (difference {diff})",
                po.net
            ));
            line += 1;
            row.line = line;
            section.rows.push(row);
            s.posted.insert(
                po.id.clone(),
                Posted {
                    changes: old.changes + 1,
                    detailed: old.detailed,
                    ..posted_now
                },
            );
            continue;
        }
        s.posted.insert(po.id.clone(), posted_now);
        let mut notes: Vec<String> = po.entries.iter().filter_map(|e| e.note.clone()).collect();
        if sum != po.net {
            notes.push(format!(
                "the entries add up to {sum}, the payout is {}",
                po.net
            ));
        }
        if detailed {
            for e in po
                .entries
                .iter()
                .filter(|e| e.category != Category::Transfer && !earlier_loose.contains(&e.id))
            {
                let row = entry_row(ctx, c, &chart, s, p, e, Some(&po.id), po.arrival)?;
                line += 1;
                section.rows.push(RawRow { line, ..row });
            }
            let mut row = base_row(
                format!("{ext}:payout"),
                date,
                -po.net,
                format!("{} payout", p.title()),
                po.id.clone(),
                raw,
            );
            row.splits.push(RawSplit {
                category_hint: transit.map(|a| chart.full_name(a)).unwrap_or_default(),
                amount: -po.net,
                memo: "Payout".into(),
                class_hint: None,
            });
            if !notes.is_empty() {
                row.review = Some(notes.join("; "));
            }
            line += 1;
            row.line = line;
            section.rows.push(row);
            continue;
        }
        // Entries posted on their own earlier are already in the clearing account: the
        // row adds only the rest (the bank deposit still matches the payout's net, see
        // `deposit_source`).
        let mut row = base_row(
            ext,
            date,
            sub(po.net, already),
            format!("{} payout", p.title()),
            po.id.clone(),
            raw,
        );
        let mut review = false;
        for (cat, v) in &t {
            if v.is_zero() {
                continue;
            }
            let (sp, r) = split_for(ctx, &chart, s, *cat, *v)?;
            review |= r;
            row.splits.push(sp);
        }
        if sum != po.net {
            let (sp, _) = split_for(ctx, &chart, s, Category::Other, sub(po.net, sum))?;
            row.splits.push(sp);
            review = true;
        }
        if s.match_invoices {
            for e in po.entries.iter().filter(|e| e.category == Category::Sale) {
                if let Some(n) = invoice_in(ctx, &e.description)? {
                    row.invoices.push(InvoiceApply {
                        number: n,
                        amount: e.gross,
                    });
                }
            }
        }
        if review {
            notes.push("some entries have no account yet (processor settings)".into());
        }
        if !notes.is_empty() {
            row.review = Some(notes.join("; "));
        }
        line += 1;
        row.line = line;
        section.rows.push(row);
    }
    // Entries outside any payout: their own rows against the clearing account.
    for e in &data.loose {
        if e.currency != base {
            st.warnings.push(format!(
                "{} entry {} is in {}: multi-currency needed; not posted",
                p.title(),
                e.id,
                e.currency
            ));
            continue;
        }
        let key = section_key(ctx, &mut sections, conn, p, clearing, &base, None, None)?;
        let row = entry_row(ctx, c, &chart, s, p, e, None, c.now())?;
        line += 1;
        if let Some(sec) = sections.get_mut(&key) {
            sec.rows.push(RawRow { line, ..row });
        }
        s.loose.insert(e.id.clone());
    }
    if !data.pending.is_empty() {
        let n = data.pending.len();
        st.warnings.push(format!(
            "{n} {} transaction{} pending payout (held at {})",
            p.title(),
            if n == 1 { " is" } else { "s are" },
            p.title()
        ));
    }
    // PayPal fees after the last withdrawal are re-read with the pending entries, so
    // the payout that later holds them is seen whole.
    s.pending_since = data
        .pending
        .iter()
        .chain(
            data.loose
                .iter()
                .filter(|e| e.category == Category::Fee && p == Provider::PayPal),
        )
        .map(|e| e.created)
        .min();
    s.pending_count = data.pending.len();
    s.waiting_since = waiting;
    for (_, mut sec) in sections {
        let dates: Vec<Date> = sec.rows.iter().map(|r| r.date).collect();
        sec.period = match (dates.iter().min(), dates.iter().max()) {
            (Some(a), Some(b)) => Some((*a, *b)),
            _ => None,
        };
        st.sections.push(sec);
    }
    Ok((st, listed))
}

/// One entry as its own row against the clearing account: detailed mode, or an entry
/// outside any payout.
#[allow(clippy::too_many_arguments)]
fn entry_row(
    ctx: &Ctx<'_>,
    c: &Connector,
    chart: &Chart,
    s: &ProcSettings,
    p: Provider,
    e: &Entry,
    payout: Option<&str>,
    fallback_time: i64,
) -> CommandResult<RawRow> {
    let mut row = base_row(
        format!("{}:{}", p.key(), e.id),
        date_of(
            c,
            if e.created > 0 {
                e.created
            } else {
                fallback_time
            },
        )?,
        e.net(),
        if e.description.is_empty() {
            e.kind.clone()
        } else {
            e.description.clone()
        },
        e.id.clone(),
        json!({"payout": payout, "entry": e.id, "kind": e.kind}).to_string(),
    );
    let (sp, review) = split_for(ctx, chart, s, e.category, e.gross)?;
    let mut needs = review;
    row.splits.push(sp);
    if !e.fee.is_zero() {
        let (f, r2) = split_for(ctx, chart, s, Category::Fee, -e.fee)?;
        needs |= r2;
        row.splits.push(f);
    }
    if e.category == Category::Sale
        && s.match_invoices
        && let Some(n) = invoice_in(ctx, &e.description)?
    {
        row.invoices.push(InvoiceApply {
            number: n,
            amount: e.gross,
        });
    }
    let mut notes = Vec::new();
    if payout.is_none() {
        notes.push(format!("{} {} outside any payout", p.title(), e.kind));
    }
    if let Some(n) = &e.note {
        notes.push(n.clone());
    }
    if needs {
        notes.push(format!(
            "map the {} account",
            label(e.category).to_lowercase()
        ));
    }
    if !notes.is_empty() && (needs || e.note.is_some()) {
        row.review = Some(notes.join("; "));
    }
    Ok(row)
}

// ----- commands ---------------------------------------------------------------------------

pub(crate) fn register(r: &mut Registry) {
    r.add_in(
        MODULE,
        "processor.settings",
        "A processor connection's mode, clearing account and category accounts",
        false,
        settings_cmd,
    );
    r.add_in(
        MODULE,
        "processor.set",
        "Change a processor connection's mode, accounts or invoice matching",
        true,
        set_cmd,
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ConnArg {
    /// Connection id or label.
    connection: String,
}

fn processor_conn(ctx: &Ctx<'_>, r: &str) -> CommandResult<ConnDoc> {
    let c = crate::connect::find_conn(ctx, r)?;
    if c.processor.is_none() {
        return Err(rejected(format!(
            "{} is not a payment processor connection",
            c.label
        )));
    }
    Ok(c)
}

pub(crate) fn settings_json(ctx: &Ctx<'_>, c: &ConnDoc) -> CommandResult<Value> {
    let s = c.processor.clone().unwrap_or_default();
    let chart = Chart::load(ctx)?;
    let named = |id: Option<&str>| {
        id.and_then(account_id)
            .filter(|a| chart.get(*a).is_some())
            .map(|a| json!({"id": a, "name": chart.full_name(a)}))
            .unwrap_or(Value::Null)
    };
    let accounts: serde_json::Map<String, Value> = MAPPED
        .iter()
        .map(|cat| {
            (
                cat.key().to_owned(),
                json!({"label": label(*cat), "account": named(s.accounts.get(cat.key()).map(String::as_str)),
                       "default": match cat {
                           Category::Sale | Category::Refund | Category::Dispute | Category::DisputeReversal => "the sales account",
                           Category::Fee => "the fees account",
                           _ => "uncategorized (review)",
                       }}),
            )
        })
        .collect();
    Ok(json!({
        "connection": c.id, "provider": c.provider, "label": c.label, "mode": s.mode,
        "clearing": named(s.clearing.as_deref()), "transit": named(s.transit.as_deref()),
        "accounts": accounts, "match_invoices": s.match_invoices,
        "pending_payout": s.pending_count,
    }))
}

fn settings_cmd(ctx: &Ctx<'_>, a: ConnArg) -> CommandResult<Value> {
    settings_json(ctx, &processor_conn(ctx, &a.connection)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetArgs {
    connection: String,
    /// `summary` or `detailed`.
    #[serde(default)]
    mode: Option<String>,
    /// The clearing account (id, full name or code).
    #[serde(default)]
    clearing_account: Option<String>,
    /// Category (sale, refund, fee, dispute, dispute_reversal, adjustment, reserve,
    /// tax, other) → account (id, full name or code); null restores the default.
    #[serde(default)]
    accounts: Option<BTreeMap<String, Option<String>>>,
    #[serde(default)]
    match_invoices: Option<bool>,
}

fn set_cmd(ctx: &Ctx<'_>, a: SetArgs) -> CommandResult<Value> {
    let mut c = processor_conn(ctx, &a.connection)?;
    let mut s = c.processor.clone().unwrap_or_default();
    let chart = Chart::load(ctx)?;
    if let Some(m) = a.mode {
        if !matches!(m.as_str(), "summary" | "detailed") {
            return Err(invalid("mode is summary or detailed"));
        }
        s.mode = m;
    }
    if let Some(r) = a.clearing_account {
        let acc = chart.resolve(&r)?;
        if acc.account_type != AccountType::Asset || acc.placeholder {
            return Err(rejected(format!(
                "{} cannot hold processor balances (choose an asset account)",
                chart.full_name(acc.id)
            )));
        }
        s.clearing = Some(acc.id.to_string());
    }
    if let Some(map) = a.accounts {
        for (k, v) in map {
            let cat = Category::from_key(&k)
                .filter(|c| *c != Category::Transfer)
                .ok_or_else(|| invalid(format!("unknown category {k:?}")))?;
            match v {
                None => {
                    s.accounts.remove(cat.key());
                }
                Some(r) => {
                    let acc = chart.resolve(&r)?;
                    if acc.placeholder || acc.archived {
                        return Err(rejected(format!(
                            "{} is a placeholder or archived account",
                            chart.full_name(acc.id)
                        )));
                    }
                    s.accounts.insert(cat.key().to_owned(), acc.id.to_string());
                }
            }
        }
    }
    if let Some(m) = a.match_invoices {
        s.match_invoices = m;
    }
    c.processor = Some(s);
    crate::connect::save_conn(ctx, &c)?;
    settings_json(ctx, &c)
}

/// Home notices: processor balances left unmatched after the match window.
pub(crate) fn unmatched_notices(ctx: &Ctx<'_>, today: Date) -> CommandResult<Vec<Value>> {
    let mut out = Vec::new();
    let window = crate::import::match_window(ctx)?;
    let chart = Chart::load(ctx)?;
    for conn in crate::connect::connections(ctx)? {
        for acc in held_accounts(&conn) {
            let Some(a) = chart.get(acc) else { continue };
            let splits = ctx.tx.account_splits(acc, None, Some(today))?;
            let balance = splits.iter().fold(Decimal::ZERO, |s, x| add(s, x.quantity));
            if balance.is_zero() {
                continue;
            }
            let last = splits.iter().map(|x| x.date).max();
            if last.is_some_and(|d| d.days_until(today) > window) {
                let shown = mk_core::currency::format_amount(balance, chart.scale_of(&a.commodity));
                out.push(json!({"kind": "clearing", "connection": conn.id, "account": acc,
                    "message": format!("{} has {shown} not matched to a deposit", chart.full_name(acc))}));
            }
        }
    }
    Ok(out)
}

/// Notes for the Accounts list: a PayPal clearing account with a balance and no
/// withdrawal to the bank in the last 30 days is money "held at PayPal".
pub(crate) fn account_notes(
    ctx: &Ctx<'_>,
    today: Date,
) -> CommandResult<BTreeMap<AccountId, String>> {
    let mut out = BTreeMap::new();
    for conn in crate::connect::connections(ctx)? {
        if conn.provider != Provider::PayPal.key() {
            continue;
        }
        let Some(s) = &conn.processor else { continue };
        let Some(acc) = s.clearing.as_deref().and_then(account_id) else {
            continue;
        };
        let recent = match s.last_withdrawal {
            Some(t) => {
                mk_import::simplefin::epoch_date(t, 0).is_ok_and(|d| d.days_until(today) <= 30)
            }
            None => false,
        };
        if recent {
            continue;
        }
        let balance = ctx
            .tx
            .account_splits(acc, None, Some(today))?
            .iter()
            .fold(Decimal::ZERO, |a, x| add(a, x.quantity));
        if !balance.is_zero() {
            out.insert(acc, "held at PayPal".to_owned());
        }
    }
    Ok(out)
}

/// A bank deposit that equals a processor payout waiting in a clearing account (within
/// the match window): the clearing account is proposed as the other side.
pub(crate) fn deposit_source(
    ctx: &Ctx<'_>,
    target: AccountId,
    amount: Decimal,
    date: Date,
    window: i64,
) -> CommandResult<Option<(AccountId, Date)>> {
    if amount <= Decimal::ZERO {
        return Ok(None);
    }
    let w = i32::try_from(window).unwrap_or(3);
    for conn in crate::connect::connections(ctx)? {
        for acc in held_accounts(&conn) {
            if acc == target {
                continue;
            }
            let splits = ctx
                .tx
                .account_splits(acc, date.add_days(-w), date.add_days(w))?;
            // A payout row's amount, or a payout whose net is the amount (its row may
            // hold less when some entries were posted on their own earlier).
            let payouts: Vec<String> = conn
                .processor
                .iter()
                .flat_map(|s| s.posted.iter())
                .filter(|(_, x)| x.net.parse::<Decimal>().ok() == Some(amount))
                .map(|(id, _)| format!(":{}:{id}", conn.provider))
                .collect();
            let incoming = splits.iter().find(|s| {
                s.quantity == amount
                    || (s.quantity > Decimal::ZERO
                        && s.fingerprint.as_ref().is_some_and(|f| {
                            f.starts_with("ext:") && payouts.iter().any(|x| f.ends_with(x.as_str()))
                        }))
            });
            let Some(inc) = incoming else { continue };
            let taken = splits
                .iter()
                .any(|s| s.quantity == -amount && s.date >= inc.date);
            if !taken {
                return Ok(Some((acc, inc.date)));
            }
        }
    }
    Ok(None)
}

// ----- a scripted processor server for tests and reviews ---------------------------------

/// Scripted Stripe, Square and PayPal answers through the fetcher interface, shaped
/// after the vendors' documented responses (tests and screen reviews; no network).
#[derive(Debug, Clone, Default)]
pub struct FakeProcessors {
    pub state: std::sync::Arc<std::sync::Mutex<FakeProcState>>,
}

/// A request the scripted server saw: (method, url, header names, Basic credentials
/// given, body).
pub type SeenRequest = (HttpMethod, String, Vec<String>, bool, Option<String>);

#[derive(Debug, Default)]
pub struct FakeProcState {
    /// Stripe Payout objects (`/v1/payouts`).
    pub stripe_payouts: Vec<Value>,
    /// Stripe balance transactions by payout id.
    pub stripe_entries: BTreeMap<String, Vec<Value>>,
    /// Stripe `type=stripe_fee` balance transactions (all fees, in payouts or not).
    pub stripe_fees: Vec<Value>,
    pub square_locations: Vec<Value>,
    /// Square payouts by location id.
    pub square_payouts: BTreeMap<String, Vec<Value>>,
    /// Square payout entries by payout id.
    pub square_entries: BTreeMap<String, Vec<Value>>,
    /// PayPal `transaction_details` items.
    pub paypal: Vec<Value>,
    /// Items per page (all providers).
    pub page_size: usize,
    /// Answer every data request with this status.
    pub status: Option<u16>,
    /// Answer requests whose path ends with the text with the status.
    pub status_for: Vec<(String, u16)>,
    /// Pad answers to this many bytes (oversize tests).
    pub pad_to: usize,
    /// Requests seen: (method, url, header names, Basic credentials given, body).
    pub requests: Vec<SeenRequest>,
}

fn query(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| percent_decode(v))
    })
}

fn percent_decode(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if c == b'%'
            && let Some(h) = b
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(h);
            i += 3;
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl FakeProcessors {
    pub fn requests(&self) -> Vec<SeenRequest> {
        self.state
            .lock()
            .map(|s| s.requests.clone())
            .unwrap_or_default()
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut FakeProcState) -> T) -> Option<T> {
        self.state.lock().ok().map(|mut s| f(&mut s))
    }
}

fn ok_json(v: &Value, pad_to: usize) -> Result<HttpResponse, FetchError> {
    let mut body = v.to_string().into_bytes();
    if pad_to > body.len() {
        body.resize(pad_to, b' ');
    }
    Ok(HttpResponse {
        status: 200,
        body,
        retry_after: None,
        deprecation: None,
    })
}

fn page_of(items: &[Value], size: usize, after: Option<&str>) -> (Vec<Value>, bool) {
    let start = match after {
        Some(a) => items
            .iter()
            .position(|v| v.get("id").and_then(Value::as_str) == Some(a))
            .map_or(items.len(), |i| i + 1),
        None => 0,
    };
    let rest = items.get(start..).unwrap_or(&[]);
    let take = rest.len().min(size.max(1));
    (rest.get(..take).unwrap_or(&[]).to_vec(), rest.len() > take)
}

impl crate::connect::Fetcher for FakeProcessors {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, FetchError> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| FetchError::Network("fake server unavailable".into()))?;
        s.requests.push((
            req.method,
            req.url.clone(),
            req.headers.iter().map(|h| h.0.clone()).collect(),
            req.basic.is_some(),
            req.body
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned()),
        ));
        let size = if s.page_size == 0 { 100 } else { s.page_size };
        let path = req.url.split('?').next().unwrap_or_default().to_owned();
        let status = |code: u16| {
            Ok(HttpResponse {
                status: code,
                body: Vec::new(),
                retry_after: None,
                deprecation: None,
            })
        };
        let bearer = req
            .headers
            .iter()
            .any(|h| h.0 == "Authorization" && h.1.starts_with("Bearer "));
        let answer = |v: Value, s: &FakeProcState| -> Result<HttpResponse, FetchError> {
            let r = ok_json(&v, s.pad_to)?;
            if r.body.len() > req.max_bytes {
                return Err(FetchError::Refused(format!(
                    "the answer is larger than {} MB",
                    req.max_bytes / (1024 * 1024)
                )));
            }
            Ok(r)
        };
        if path.ends_with("/v1/oauth2/token") {
            return match (&req.basic, &req.body) {
                (Some((id, secret)), Some(b))
                    if !id.is_empty()
                        && !secret.is_empty()
                        && b == b"grant_type=client_credentials" =>
                {
                    answer(
                        json!({"access_token": "A21AAtesttoken123", "token_type": "Bearer", "expires_in": 32400}),
                        &s,
                    )
                }
                _ => status(401),
            };
        }
        if let Some(code) = s.status {
            return status(code);
        }
        if let Some((_, code)) = s
            .status_for
            .iter()
            .find(|(p, _)| path.ends_with(p.as_str()))
        {
            return status(*code);
        }
        // Square answers only requests that name the API version.
        if path.contains("/v2/")
            && !req
                .headers
                .iter()
                .any(|h| h.0 == "Square-Version" && h.1 == SQUARE_VERSION)
        {
            return Ok(HttpResponse {
                status: 400,
                body: br#"{"errors":[{"category":"INVALID_REQUEST_ERROR","code":"BAD_REQUEST"}]}"#
                    .to_vec(),
                retry_after: None,
                deprecation: None,
            });
        }
        if path.ends_with("/v1/payouts") {
            if !req
                .basic
                .as_ref()
                .is_some_and(|(k, _)| k.starts_with("rk_"))
            {
                return status(401);
            }
            let after = query(&req.url, "starting_after");
            let limit = query(&req.url, "limit")
                .and_then(|l| l.parse().ok())
                .unwrap_or(size);
            let (data, more) = page_of(&s.stripe_payouts, size.min(limit), after.as_deref());
            return answer(
                json!({"object": "list", "url": "/v1/payouts", "has_more": more, "data": data}),
                &s,
            );
        }
        if path.ends_with("/v1/balance_transactions") {
            let key_ok = req
                .basic
                .as_ref()
                .is_some_and(|(k, _)| k.starts_with("rk_"));
            if !key_ok {
                return status(401);
            }
            let after = query(&req.url, "starting_after");
            let items: Vec<Value> = if query(&req.url, "type").as_deref() == Some("stripe_fee") {
                s.stripe_fees.clone()
            } else if let Some(po) = query(&req.url, "payout") {
                s.stripe_entries.get(&po).cloned().unwrap_or_default()
            } else {
                Vec::new()
            };
            let (data, more) = page_of(&items, size, after.as_deref());
            return answer(
                json!({"object": "list", "url": "/v1/balance_transactions", "has_more": more, "data": data}),
                &s,
            );
        }
        if path.ends_with("/v2/locations") {
            if !bearer {
                return status(401);
            }
            return answer(json!({"locations": s.square_locations}), &s);
        }
        if path.ends_with("/payout-entries") {
            let id = path
                .trim_end_matches("/payout-entries")
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            let items = s.square_entries.get(&id).cloned().unwrap_or_default();
            let after = query(&req.url, "cursor");
            let (data, more) = page_of(&items, size, after.as_deref());
            let cursor = if more {
                data.last()
                    .and_then(|v| v.get("id"))
                    .cloned()
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            };
            return answer(json!({"payout_entries": data, "cursor": cursor}), &s);
        }
        if path.ends_with("/v2/payouts") {
            let loc = query(&req.url, "location_id").unwrap_or_default();
            let items = s.square_payouts.get(&loc).cloned().unwrap_or_default();
            let after = query(&req.url, "cursor");
            let (data, more) = page_of(&items, size, after.as_deref());
            let cursor = if more {
                data.last()
                    .and_then(|v| v.get("id"))
                    .cloned()
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            };
            return answer(json!({"payouts": data, "cursor": cursor}), &s);
        }
        if path.ends_with("/v1/reporting/transactions") {
            if !bearer {
                return status(401);
            }
            let start = query(&req.url, "start_date").unwrap_or_default();
            let end = query(&req.url, "end_date").unwrap_or_default();
            let within: Vec<Value> = s
                .paypal
                .iter()
                .filter(|d| {
                    let t = d
                        .get("transaction_info")
                        .and_then(|t| t.get("transaction_initiation_date"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    // RFC 3339 texts in UTC compare as text.
                    t >= start.as_str() && t < end.as_str()
                })
                .cloned()
                .collect();
            let page: usize = query(&req.url, "page")
                .and_then(|p| p.parse().ok())
                .unwrap_or(1)
                .max(1);
            let total = within.len().div_ceil(size).max(1);
            let data: Vec<Value> = within
                .iter()
                .skip((page - 1) * size)
                .take(size)
                .cloned()
                .collect();
            return answer(
                json!({"transaction_details": data, "page": page, "total_pages": total,
                       "total_items": within.len(), "start_date": start, "end_date": end}),
                &s,
            );
        }
        status(404)
    }
}
