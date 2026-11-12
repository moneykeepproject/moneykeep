//! SnapTrade brokerage connections with personal keys (connectivity-brokerage.md part
//! B): connect with the client id and consumer key, list accounts, and sync activities
//! (paged), positions and balances into investment batches. Never calls register-user
//! or the connection-refresh operation. Requests are signed per
//! `mk_import::snaptrade` (doc URLs there).
//!
//! connectivity-brokerage.md revision 2: the paths, checked against SnapTrade's
//! current API reference (docs.snaptrade.com/reference, read 2026-11-08), are from
//! the host root without the legacy `/api/v1` prefix: `GET /accounts/all`,
//! `GET /accounts/{accountId}/activities`, `GET /accounts/{accountId}/positions/all`
//! ("List all account positions"; the per-account `/positions` it replaces answers
//! 410 for newer keys) and `GET /accounts/{accountId}/balances`. Each account's
//! activities, positions and balances are read independently: a failing one does not
//! discard the others. Calls for one account are spaced to stay under 10 a minute
//! (the SnapTrade SDK README, snaptrade-python-sdk 13.0.13: possibly 10/minute per
//! account for personal users) and a 429 honours `Retry-After`.

use std::collections::BTreeMap;

use mk_core::{Date, Decimal};
use mk_import::snaptrade::{self as st, Account, Activity, Holding};
use mk_import::{ClosingBalance, Position, Section, SectionKind, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::connect::{
    ConnDoc, Connector, FetchError, HttpMethod, HttpRequest, HttpResponse, backoff, note_error,
};
use crate::{CommandError, CommandResult, Ctx};

pub(crate) const PROVIDER: &str = "snaptrade";
pub(crate) const MODULE: &str = "investments";
/// Days re-read before the last synced trade date (data arrives a day late).
const OVERLAP_DAYS: i32 = 10;
/// Most waits on a 429 before giving up for this sync.
const MAX_RETRIES: usize = 3;
/// Longest wait honoured from `Retry-After` (seconds).
const MAX_WAIT: u64 = 30;
/// Activities re-sent with a new id: same content within this many days.
const SAME_WITHIN: i64 = 3;
/// Least time between two calls for the same account (under 10 a minute).
const SPACING_MS: u64 = 6_000;
/// Deprecation headers kept per connection.
const MAX_DEPRECATIONS: usize = 10;

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

/// The personal key (never printed).
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Key {
    pub client_id: String,
    pub consumer_key: String,
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(hidden)")
    }
}

pub(crate) fn key_from_input(client_id: &str, consumer_key: &str) -> CommandResult<Key> {
    let id = client_id.trim();
    let key = consumer_key.trim();
    if id.is_empty() || key.is_empty() {
        return Err(invalid(
            "paste the personal client id and consumer key from the SnapTrade dashboard",
        ));
    }
    let ok = |s: &str| s.len() <= 200 && s.chars().all(|c| c.is_ascii_graphic());
    if !ok(id) || !ok(key) {
        return Err(invalid(
            "the client id or consumer key has characters a key never has",
        ));
    }
    Ok(Key {
        client_id: id.to_owned(),
        consumer_key: key.to_owned(),
    })
}

pub(crate) fn key_text(k: &Key) -> String {
    serde_json::to_string(k).unwrap_or_default()
}

pub(crate) fn key_parse(text: &str) -> Option<Key> {
    serde_json::from_str(text).ok()
}

/// A signed GET of `path` (from the host root; the signature covers this path as
/// sent) with `extra` query parameters before `clientId` and `timestamp`.
pub(crate) fn request(k: &Key, path: &str, extra: &str, now: i64) -> HttpRequest {
    let mut query = String::new();
    if !extra.is_empty() {
        query.push_str(extra);
        query.push('&');
    }
    query.push_str(&format!(
        "clientId={}&timestamp={now}",
        crate::connect::query_escape(&k.client_id)
    ));
    let sig = st::signature(&k.consumer_key, path, &query, None);
    HttpRequest {
        method: HttpMethod::Get,
        url: format!("https://{}{path}?{query}", st::HOST),
        basic: None,
        authority: st::HOST.into(),
        max_bytes: st::MAX_RESPONSE_BYTES,
        headers: vec![
            ("Accept".into(), "application/json".into()),
            ("Signature".into(), sig),
        ],
        body: None,
        allow_http: false,
    }
}

/// The path of the accounts request (connect check and every sync).
const ACCOUNTS_PATH: &str = "/accounts/all";

/// The accounts request, signed at `now` (tests).
#[cfg(test)]
pub(crate) fn accounts_request(k: &Key, now: i64) -> HttpRequest {
    request(k, ACCOUNTS_PATH, "", now)
}

/// The connect check: the accounts request, signed when sent, a `401` tried once
/// more signed fresh (security-review-1 B3). `None` when cancelled.
pub(crate) fn check(
    key: &Key,
    now: i64,
    send: &mut Send<'_>,
    clock: &dyn crate::connect::Fetcher,
) -> Option<Result<HttpResponse, FetchError>> {
    let mut c = Caller::new(send, clock, key, now);
    match c.call(&Unsigned::new(ACCOUNTS_PATH, ""), None) {
        Ok(body) => Some(Ok(HttpResponse {
            status: 200,
            body,
            retry_after: None,
            deprecation: None,
        })),
        Err(CallErr::Cancelled) => None,
        Err(CallErr::Fetch(e)) => Some(Err(e)),
        Err(CallErr::Status(status, body)) => Some(Ok(HttpResponse {
            status,
            body,
            retry_after: None,
            deprecation: None,
        })),
    }
}

/// "SnapTrade no longer offers this request (positions). MoneyKeep needs an
/// update." — a 410: retrying cannot help.
pub(crate) fn gone_text(what: &str) -> String {
    format!("SnapTrade no longer offers this request ({what}). MoneyKeep needs an update.")
}

/// A refusal of the request for `what` (accounts, activities, …) in words: a
/// timestamp complaint points at the computer's clock.
pub(crate) fn refusal(status: u16, body: &[u8], what: &str) -> (String, String) {
    let text =
        String::from_utf8_lossy(body.get(..body.len().min(2000)).unwrap_or(&[])).to_lowercase();
    if status == 401 && (text.contains("timestamp") || text.contains("clock")) {
        return (
            "clock".into(),
            "SnapTrade refused the request time. Check your computer's clock".into(),
        );
    }
    match status {
        401 | 403 => (
            "rejected".into(),
            "SnapTrade rejected the personal key — paste a new client id and consumer key".into(),
        ),
        429 => (
            "rate_limited".into(),
            "SnapTrade asked MoneyKeep to slow down; it will try again later.".into(),
        ),
        410 => ("gone".into(), gone_text(what)),
        s => (
            "server".into(),
            format!("SnapTrade answered with HTTP {s}; MoneyKeep will try again later."),
        ),
    }
}

// ----- sync job -----------------------------------------------------------------------------

#[derive(Clone)]
pub struct SnapJob {
    pub(crate) key: Key,
    /// (link id, remote account id, first day).
    pub(crate) accounts: Vec<(String, String, Date)>,
    pub(crate) end: Date,
    pub(crate) base: String,
    pub(crate) now: i64,
}

impl std::fmt::Debug for SnapJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapJob")
            .field("accounts", &self.accounts.len())
            .finish_non_exhaustive()
    }
}

/// One part of an account's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Activities,
    Positions,
    Balances,
}

impl Part {
    /// The request's name in messages.
    pub fn request(self) -> &'static str {
        match self {
            Part::Activities => "activities",
            Part::Positions => "positions",
            Part::Balances => "balances",
        }
    }

    /// What the user gets from it.
    fn noun(self) -> &'static str {
        match self {
            Part::Activities => "transactions",
            Part::Positions => "holdings",
            Part::Balances => "the cash balance",
        }
    }
}

/// A part that could not be read.
#[derive(Debug, Clone)]
pub struct Failure {
    pub part: Part,
    /// The HTTP status, when SnapTrade answered.
    pub status: Option<u16>,
    /// Why, in words (without the part's name).
    pub why: String,
}

impl Failure {
    /// "SnapTrade answered 410 for positions".
    fn reason(&self) -> String {
        let what = self.part.request();
        match self.status {
            Some(429) => format!("SnapTrade asked MoneyKeep to slow down for {what}"),
            Some(s) => format!("SnapTrade answered {s} for {what}"),
            None => format!("{} for {what}", self.why),
        }
    }
}

/// One account's data.
#[derive(Debug, Clone)]
pub struct AccountData {
    pub link: String,
    pub activities: Vec<Activity>,
    pub holdings: Vec<Holding>,
    pub cash: Option<Decimal>,
    pub pages: Vec<u64>,
    /// The parts that could not be read (the rest is imported).
    pub failed: Vec<Failure>,
    /// Notes for the sync summary ("2 option positions not imported").
    pub notes: Vec<String>,
}

impl AccountData {
    fn read(&self, part: Part) -> bool {
        !self.failed.iter().any(|f| f.part == part)
    }
}

#[derive(Debug, Clone)]
pub enum SnapResult {
    Ok {
        accounts: Vec<Account>,
        data: Vec<AccountData>,
        /// `Deprecation` headers seen: "<path>: <value>".
        deprecations: Vec<String>,
    },
    Status(u16, Vec<u8>),
    Fetch(FetchError),
    Parse(String),
    Cancelled,
}

type Send<'a> = dyn FnMut(HttpRequest) -> Option<Result<HttpResponse, FetchError>> + 'a;

/// Why a call failed.
enum CallErr {
    Status(u16, Vec<u8>),
    Fetch(FetchError),
    Cancelled,
}

/// A request not signed yet: its path and the query parameters before `clientId`
/// and `timestamp` (security-review-1 B3: the timestamp and signature are made each
/// time it is sent).
#[derive(Debug, Clone)]
struct Unsigned {
    path: String,
    extra: String,
}

impl Unsigned {
    fn new(path: impl Into<String>, extra: impl Into<String>) -> Self {
        Unsigned {
            path: path.into(),
            extra: extra.into(),
        }
    }
}

/// Sends requests: each signed when sent (retries and later pages too), spaced per
/// account, `429` + `Retry-After` honoured (waiting with no transaction open), a
/// `401` tried once more signed fresh, `Deprecation` headers noted.
struct Caller<'a, 'b> {
    send: &'a mut Send<'b>,
    clock: &'a dyn crate::connect::Fetcher,
    key: &'a Key,
    /// Unix seconds when the job started, and the clock then (milliseconds).
    start: (i64, u64),
    /// The last call per remote account (clock milliseconds).
    last: BTreeMap<String, u64>,
    deprecations: Vec<String>,
}

impl<'a, 'b> Caller<'a, 'b> {
    fn new(
        send: &'a mut Send<'b>,
        clock: &'a dyn crate::connect::Fetcher,
        key: &'a Key,
        now: i64,
    ) -> Self {
        let start = (now, clock.clock_ms());
        Caller {
            send,
            clock,
            key,
            start,
            last: BTreeMap::new(),
            deprecations: Vec::new(),
        }
    }

    /// Unix seconds now: the job's start time plus the time passed on the clock.
    fn now(&self) -> i64 {
        let passed = self.clock.clock_ms().saturating_sub(self.start.1) / 1000;
        self.start
            .0
            .saturating_add(i64::try_from(passed).unwrap_or(i64::MAX))
    }
}

fn path_of(url: &str) -> &str {
    query_of(url).0
}

impl Caller<'_, '_> {
    fn space(&mut self, account: Option<&str>) {
        let Some(a) = account else { return };
        if let Some(last) = self.last.get(a) {
            let next = last.saturating_add(SPACING_MS);
            let now = self.clock.clock_ms();
            if now < next {
                self.clock.wait(next - now);
            }
        }
    }

    fn call(&mut self, u: &Unsigned, account: Option<&str>) -> Result<Vec<u8>, CallErr> {
        let mut refused_once = false;
        let mut attempt = 0;
        loop {
            self.space(account);
            // Signed now, so a retry or a later page never carries an old timestamp.
            let req = request(self.key, &u.path, &u.extra, self.now());
            let r = match (self.send)(req.clone()) {
                None => return Err(CallErr::Cancelled),
                Some(Err(e)) => return Err(CallErr::Fetch(e)),
                Some(Ok(r)) => r,
            };
            if let Some(a) = account {
                self.last.insert(a.to_owned(), self.clock.clock_ms());
            }
            if let Some(d) = &r.deprecation {
                let line = format!("{}: {d}", path_of(&req.url));
                if !self.deprecations.contains(&line) {
                    self.deprecations.push(line);
                }
            }
            if r.status == 200 {
                return Ok(r.body);
            }
            if r.status == 429 && attempt < MAX_RETRIES {
                attempt += 1;
                let wait = r.retry_after.unwrap_or(2).min(MAX_WAIT);
                self.clock.wait(wait.saturating_mul(1000));
                continue;
            }
            // A 401 counts as a rejected key only when the same request, signed
            // fresh, is refused a second time (security-review-1 B3).
            if r.status == 401 && !refused_once {
                refused_once = true;
                continue;
            }
            return Err(CallErr::Status(r.status, r.body));
        }
    }

    /// One part's call: the answer, or the failure to note (`Err(None)`: cancelled).
    fn part(
        &mut self,
        req: &Unsigned,
        account: &str,
        part: Part,
    ) -> Result<Vec<u8>, Option<Failure>> {
        match self.call(req, Some(account)) {
            Ok(b) => Ok(b),
            Err(CallErr::Cancelled) => Err(None),
            Err(CallErr::Status(s, _)) => Err(Some(Failure {
                part,
                status: Some(s),
                why: String::new(),
            })),
            Err(CallErr::Fetch(e)) => Err(Some(Failure {
                part,
                status: None,
                why: format!("the connection failed ({e})"),
            })),
        }
    }
}

fn parse_failure(part: Part, e: &mk_import::ImportError) -> Failure {
    Failure {
        part,
        status: None,
        why: format!("the answer was refused ({e})"),
    }
}

/// Read accounts, then each linked account's activities (paged by `offset` until
/// `offset + returned ≥ total`), positions (`/positions/all`) and balances — each
/// independently, so one failing keeps what the others read. The parts are read
/// for every account in turn (all activities, then all positions, then all
/// balances), so the spacing per account costs little time with several accounts.
pub(crate) fn run(
    job: &SnapJob,
    send: &mut Send<'_>,
    clock: &dyn crate::connect::Fetcher,
) -> SnapResult {
    let mut c = Caller::new(send, clock, &job.key, job.now);
    let accounts = match c.call(&Unsigned::new(ACCOUNTS_PATH, ""), None) {
        Ok(b) => match st::accounts(&b) {
            Ok(a) => a,
            Err(e) => return SnapResult::Parse(e.to_string()),
        },
        Err(CallErr::Cancelled) => return SnapResult::Cancelled,
        Err(CallErr::Fetch(e)) => return SnapResult::Fetch(e),
        Err(CallErr::Status(s, b)) => return SnapResult::Status(s, b),
    };
    let mut data: Vec<AccountData> = job
        .accounts
        .iter()
        .map(|(link, _, _)| AccountData {
            link: link.clone(),
            activities: Vec::new(),
            holdings: Vec::new(),
            cash: None,
            pages: Vec::new(),
            failed: Vec::new(),
            notes: Vec::new(),
        })
        .collect();
    // Activities.
    for ((_, remote, start), d) in job.accounts.iter().zip(data.iter_mut()) {
        let id = crate::connect::query_escape(remote);
        let mut offset: u64 = 0;
        loop {
            let extra = format!(
                "startDate={start}&endDate={}&offset={offset}&limit={}",
                job.end,
                st::PAGE
            );
            let req = Unsigned::new(format!("/accounts/{id}/activities"), extra);
            let body = match c.part(&req, remote, Part::Activities) {
                Ok(b) => b,
                Err(None) => return SnapResult::Cancelled,
                Err(Some(f)) => {
                    d.failed.push(f);
                    break;
                }
            };
            let (page, total) = match st::activities(&body) {
                Ok(x) => x,
                Err(e) => {
                    d.failed.push(parse_failure(Part::Activities, &e));
                    break;
                }
            };
            d.pages.push(offset);
            let n = page.len() as u64;
            d.activities.extend(page);
            offset = offset.saturating_add(n);
            if n == 0 || offset >= total || d.pages.len() >= 1000 {
                break;
            }
        }
        if !d.read(Part::Activities) {
            // Half a history is worse than none: nothing of it is imported.
            d.activities.clear();
            d.pages.clear();
        }
    }
    // Positions.
    let mut cash_eq: Vec<Option<Decimal>> = vec![None; data.len()];
    for (((_, remote, _), d), ce) in job
        .accounts
        .iter()
        .zip(data.iter_mut())
        .zip(cash_eq.iter_mut())
    {
        let id = crate::connect::query_escape(remote);
        let req = Unsigned::new(format!("/accounts/{id}/positions/all"), "");
        match c.part(&req, remote, Part::Positions) {
            Ok(b) => match st::positions(&b, &job.base) {
                Ok(p) => {
                    d.notes.extend(p.notes());
                    *ce = p.cash_equivalent;
                    d.holdings = p.holdings;
                }
                Err(e) => d.failed.push(parse_failure(Part::Positions, &e)),
            },
            Err(None) => return SnapResult::Cancelled,
            Err(Some(f)) => d.failed.push(f),
        }
    }
    // Balances. Their `cash` already includes money market funds (the balances
    // reference), so cash-equivalent positions stand in for it only when the
    // balances give no cash in the book's currency.
    for (((_, remote, _), d), ce) in job.accounts.iter().zip(data.iter_mut()).zip(cash_eq) {
        let id = crate::connect::query_escape(remote);
        let req = Unsigned::new(format!("/accounts/{id}/balances"), "");
        match c.part(&req, remote, Part::Balances) {
            Ok(b) => match st::cash(&b, &job.base) {
                Ok(cash) => d.cash = cash.or(ce),
                Err(e) => {
                    d.failed.push(parse_failure(Part::Balances, &e));
                    d.cash = ce;
                }
            },
            Err(None) => return SnapResult::Cancelled,
            Err(Some(f)) => {
                d.failed.push(f);
                d.cash = ce;
            }
        }
    }
    SnapResult::Ok {
        accounts,
        data,
        deprecations: c.deprecations,
    }
}

/// "Holdings and the cash balance".
fn list_text(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [a] => (*a).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// The yellow line for an account with failed parts: "Holdings couldn't be read
/// (SnapTrade answered 410 for positions). SnapTrade no longer offers this request
/// (positions). MoneyKeep needs an update. Transactions and the cash balance were
/// imported."
pub(crate) fn partial_text(d: &AccountData) -> String {
    let failed: Vec<&str> = d.failed.iter().map(|f| f.part.noun()).collect();
    let reasons: Vec<String> = d.failed.iter().map(Failure::reason).collect();
    let gone: Vec<&str> = d
        .failed
        .iter()
        .filter(|f| f.status == Some(410))
        .map(|f| f.part.request())
        .collect();
    let ok: Vec<&str> = [Part::Activities, Part::Positions, Part::Balances]
        .into_iter()
        // The cash balance only when one was given in the book's currency.
        .filter(|p| d.read(*p) && (*p != Part::Balances || d.cash.is_some()))
        .map(Part::noun)
        .collect();
    let mut text = format!(
        "{} couldn't be read ({}).",
        capital(&list_text(&failed)),
        reasons.join("; ")
    );
    if !gone.is_empty() {
        text.push(' ');
        text.push_str(&gone_text(&list_text(&gone)));
    }
    if !ok.is_empty() {
        let verb = if ok.len() == 1 && ok != ["transactions"] && ok != ["holdings"] {
            "was"
        } else {
            "were"
        };
        text.push_str(&format!(" {} {verb} imported.", capital(&list_text(&ok))));
    }
    text
}

// ----- links -----------------------------------------------------------------------------------

fn sync_text(a: &Account) -> String {
    if a.disabled {
        return "Reconnect this brokerage in the SnapTrade dashboard".into();
    }
    if !a.initial_sync_completed {
        return "initial sync not completed".into();
    }
    match &a.last_successful_sync {
        Some(t) => format!("last synced {}", t.get(..10).unwrap_or(t)),
        None => "synced".into(),
    }
}

/// Create or refresh links for SnapTrade's accounts.
pub(crate) fn apply_accounts(ctx: &Ctx<'_>, conn: &ConnDoc, list: &[Account]) -> CommandResult<()> {
    let existing = crate::connect::links_of(ctx, &conn.id)?;
    for a in list {
        let id = crate::connect::link_id_of(&conn.id, &a.id);
        let mut l = existing
            .iter()
            .find(|l| l.id == id)
            .cloned()
            .unwrap_or_else(|| {
                crate::connect::new_link(
                    conn,
                    &id,
                    &a.id,
                    &format!("snaptrade:{}:{}", conn.id, a.id),
                )
            });
        l.name = if a.masked.is_empty() {
            a.name.clone()
        } else {
            format!("{} {}", a.name, a.masked)
        };
        l.currency = a.currency.clone();
        let mut r = l.remote.clone().unwrap_or_default();
        r.kind = a.kind.clone();
        r.masked = a.masked.clone();
        r.institution = a.institution.clone();
        r.status = sync_text(a);
        r.acct_id = a.id.clone();
        r.disabled = a.disabled;
        l.attention = a
            .disabled
            .then(|| "Reconnect this brokerage in the SnapTrade dashboard".to_owned());
        l.remote = Some(r);
        crate::connect::save_link_doc(ctx, &l)?;
    }
    Ok(())
}

// ----- plan and apply ------------------------------------------------------------------------

pub(crate) fn plan(
    ctx: &Ctx<'_>,
    c: &Connector,
    conn: &ConnDoc,
    secret: &str,
    by_user: bool,
) -> CommandResult<Result<SnapJob, String>> {
    let Some(key) = key_parse(secret) else {
        return Ok(Err("the stored credential is damaged; reconnect".into()));
    };
    let now = c.now();
    if !by_user {
        let hours = crate::module_settings::get_i64(ctx, "brokerage.sync_interval_hours")?.max(6);
        if conn
            .last_success
            .is_some_and(|t| now.saturating_sub(t) < hours * 3600)
        {
            return Ok(Err(format!("synced less than {hours} hours ago")));
        }
    }
    let today = mk_import::simplefin::epoch_date(now, c.utc_offset_secs)
        .map_err(|e| invalid(e.to_string()))?;
    let history = crate::module_settings::get_i64(ctx, "brokerage.history_days_first_sync")?;
    let history = i32::try_from(history).unwrap_or(365);
    let mut accounts = Vec::new();
    for l in crate::connect::links_of(ctx, &conn.id)? {
        let Some(r) = l.remote.clone() else { continue };
        if !l.active || l.account.is_none() || r.kind != "investment" {
            continue;
        }
        let start = match r.last_trade {
            Some(d) => d.add_days(-OVERLAP_DAYS).unwrap_or(d),
            None => l
                .sync_from
                .or_else(|| today.add_days(-history))
                .unwrap_or(today),
        };
        accounts.push((l.id.clone(), l.remote_account_id.clone(), start));
    }
    if accounts.is_empty() {
        return Ok(Err("no remote account is linked yet".into()));
    }
    Ok(Ok(SnapJob {
        key,
        accounts,
        end: today,
        base: ctx.tx.info()?.base_currency,
        now,
    }))
}

fn fail(conn: &mut ConnDoc, now: i64, code: &str, m: &str, attention: bool) -> Value {
    // The red line replaces any yellow one.
    conn.partial = None;
    if attention {
        conn.status = "needs_attention".into();
    } else {
        backoff(conn, now);
    }
    note_error(conn, code, m);
    crate::connect::error_json(conn)
}

/// Apply a SnapTrade sync in the sync's write.
pub(crate) fn apply(
    ctx: &Ctx<'_>,
    c: &Connector,
    conn: &mut ConnDoc,
    now: i64,
    result: SnapResult,
) -> CommandResult<Value> {
    let (accounts, data, deprecations) = match result {
        SnapResult::Ok {
            accounts,
            data,
            deprecations,
        } => (accounts, data, deprecations),
        SnapResult::Status(s, body) => {
            let (code, m) = refusal(s, &body, "accounts");
            return Ok(fail(conn, now, &code, &m, matches!(s, 401 | 403)));
        }
        SnapResult::Fetch(e) => return Ok(fail(conn, now, "network", &e.to_string(), false)),
        SnapResult::Parse(m) => {
            return Ok(fail(
                conn,
                now,
                "bad_response",
                &format!("The answer was refused: {m}"),
                false,
            ));
        }
        SnapResult::Cancelled => {
            return Err(CommandError::Coded {
                code: "cancelled",
                message: "Cancelled; nothing was recorded.".into(),
            });
        }
    };
    for d in deprecations {
        if !conn.deprecations.contains(&d) {
            conn.deprecations.push(d);
        }
    }
    let over = conn.deprecations.len().saturating_sub(MAX_DEPRECATIONS);
    conn.deprecations.drain(..over);
    apply_accounts(ctx, conn, &accounts)?;
    // Nothing read at all: the red line (one reason for all).
    let read_any = data.iter().any(|d| {
        [Part::Activities, Part::Positions, Part::Balances]
            .into_iter()
            .any(|p| d.read(p))
    });
    if !read_any && let Some(first) = data.iter().find_map(|d| d.failed.first()) {
        let all_gone = data
            .iter()
            .flat_map(|d| d.failed.iter())
            .all(|f| f.status == Some(410));
        let (code, m) = if all_gone {
            let mut parts: Vec<&str> = Vec::new();
            for f in data.iter().flat_map(|d| d.failed.iter()) {
                if !parts.contains(&f.part.request()) {
                    parts.push(f.part.request());
                }
            }
            ("gone".to_owned(), gone_text(&list_text(&parts)))
        } else {
            match first.status {
                Some(s) => refusal(s, &[], first.part.request()),
                None => (
                    "server".to_owned(),
                    format!(
                        "{}; MoneyKeep will try again later.",
                        capital(&first.reason())
                    ),
                ),
            }
        };
        let attention = matches!(first.status, Some(401 | 403));
        return Ok(fail(conn, now, &code, &m, attention));
    }
    conn.status = "active".into();
    conn.last_error = None;
    conn.failures = 0;
    conn.next_retry_at = None;
    conn.last_success = Some(now);
    let base = ctx.tx.info()?.base_currency;
    let today = mk_import::simplefin::epoch_date(now, c.utc_offset_secs)
        .map_err(|e| invalid(e.to_string()))?;
    let mut links = crate::connect::links_of(ctx, &conn.id)?;
    let mut statement = Statement::default();
    let mut digest = Vec::new();
    let mut pages = Vec::new();
    let mut line = 0usize;
    let mut partial: Vec<String> = Vec::new();
    let several = data.len() > 1;
    for d in &data {
        let Some(l) = links.iter_mut().find(|l| l.id == d.link) else {
            continue;
        };
        if !d.failed.is_empty() {
            let text = partial_text(d);
            partial.push(if several {
                format!("{}: {text}", l.name)
            } else {
                text
            });
        }
        statement.warnings.extend(d.notes.iter().cloned());
        let Some(account) = l.account.clone().and_then(|a| a.parse().ok()) else {
            continue;
        };
        pages.push(json!({"account": l.name, "offsets": d.pages}));
        let mut r = l.remote.clone().unwrap_or_default();
        // Re-sent activities: a new id with the content of one seen in the last days
        // takes the earlier id (the pipeline then classes it a duplicate).
        let mut acts = d.activities.clone();
        let mut notes: BTreeMap<String, String> = BTreeMap::new();
        // Only activities of earlier syncs whose id is no longer listed count.
        let earlier = r.seen.clone();
        let listed: std::collections::HashSet<String> = acts.iter().map(|a| a.id.clone()).collect();
        for a in &mut acts {
            let hash = a.content_hash();
            if let Some((old_id, old_date)) = earlier.get(&hash).cloned()
                && old_id != a.id
                && !listed.contains(&old_id)
                && a.date
                    .is_some_and(|dt| old_date.days_until(dt).abs() <= SAME_WITHIN)
            {
                notes.insert(
                    old_id.clone(),
                    "Same as an earlier transaction (the provider changed its id)".into(),
                );
                a.id = old_id;
                continue;
            }
            if let Some(dt) = a.date {
                r.seen.insert(hash, (a.id.clone(), dt));
                if r.last_trade.is_none_or(|x| dt > x) {
                    r.last_trade = Some(dt);
                }
            }
        }
        // Keep the memory of seen activities bounded.
        if r.seen.len() > 5000 {
            let mut v: Vec<(String, (String, Date))> =
                std::mem::take(&mut r.seen).into_iter().collect();
            v.sort_by_key(|(_, (_, d))| std::cmp::Reverse(*d));
            v.truncate(5000);
            r.seen = v.into_iter().collect();
        }
        let held = |key: &str, date: Date| -> Option<Decimal> {
            crate::inv_import::book_quantity_before(ctx, account, key, date)
                .ok()
                .flatten()
        };
        let mut mapped = st::map(&acts, &base, &d.holdings, &held);
        for row in &mut mapped.inv_rows {
            line += 1;
            row.line = line;
            if let Some(n) = row.external_id.as_ref().and_then(|i| notes.get(i)) {
                row.notes.push(n.clone());
            }
        }
        for row in &mut mapped.cash_rows {
            line += 1;
            row.line = line;
        }
        statement.warnings.extend(mapped.warnings);
        let dates: Vec<Date> = mapped
            .inv_rows
            .iter()
            .map(|r| r.date)
            .chain(mapped.cash_rows.iter().map(|r| r.date))
            .collect();
        let positions = d
            .holdings
            .iter()
            .filter_map(|h| {
                Some(Position {
                    security: h.symbol.key()?,
                    quantity: h.units,
                    price: h.price,
                    market_value: None,
                    as_of: Some(today),
                })
            })
            .collect();
        statement.sections.push(Section {
            account_key: Some(l.account_key.clone()),
            account_name: Some(format!("{} — {}", conn.label, l.name)),
            account_hint: Some(r.masked.trim_start_matches('*').to_owned())
                .filter(|x| !x.is_empty()),
            currency: mk_core::CommodityCode::new(&base).ok(),
            period: match (dates.iter().min(), dates.iter().max()) {
                (Some(a), Some(b)) => Some((*a, *b)),
                _ => None,
            },
            kind: SectionKind::Investment,
            securities: mapped.securities,
            inv_rows: mapped.inv_rows,
            rows: mapped.cash_rows,
            positions,
            positions_unread: !d.read(Part::Positions),
            cash_balance: d.cash.map(|amount| ClosingBalance {
                date: today,
                amount,
            }),
            as_of: Some(today),
            ..Section::default()
        });
        digest.extend_from_slice(serde_json::to_string(&acts).unwrap_or_default().as_bytes());
        l.remote = Some(r);
        l.last_success = Some(now);
        l.last_seen = Some(today.to_string());
        crate::connect::save_link_doc(ctx, l)?;
    }
    conn.partial = (!partial.is_empty()).then(|| partial.join(" "));
    crate::inv_import::store_feed_prices(ctx, &statement, "SnapTrade")?;
    let any = statement.sections.iter().any(|s| {
        !s.inv_rows.is_empty()
            || !s.rows.is_empty()
            || !s.positions.is_empty()
            || s.cash_balance.is_some()
    });
    if !any {
        return Ok(
            json!({"connection": conn.id, "label": conn.label, "status": conn.status,
            "batch": null, "new": 0, "pages": pages, "warnings": statement.warnings,
            "partial": conn.partial}),
        );
    }
    let staged = crate::connect::stage_feed(
        ctx,
        conn,
        statement,
        "snaptrade",
        format!("SnapTrade — {}", conn.label),
        &digest,
    )?;
    let batch = staged
        .get("batch")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let holdings = crate::inv_import::feed_holdings(ctx, conn, &batch, "SnapTrade")?;
    let mut out = staged;
    if let Some(o) = out.as_object_mut() {
        o.insert("holdings".into(), holdings);
        o.insert("pages".into(), json!(pages));
        o.insert("partial".into(), json!(conn.partial));
    }
    Ok(out)
}

/// The new connection's record after the accounts check.
pub(crate) fn new_conn(
    ctx: &Ctx<'_>,
    conn: &str,
    label: Option<String>,
    home: &str,
    sid: &str,
) -> CommandResult<ConnDoc> {
    let label = label
        .map(|l| mk_core::text::clean(&l, 100).text.trim().to_owned())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "SnapTrade".to_owned());
    crate::connect::blank_conn(ctx, conn, PROVIDER, &label, home, sid, st::HOST)
}

// ----- a scripted SnapTrade API for tests and reviews ---------------------------------------

/// A scripted SnapTrade API answering through the fetcher interface (tests and screen
/// reviews; no network). It checks every request's signature with its consumer key,
/// shaped after the vendor's documented responses.
#[derive(Debug, Clone, Default)]
pub struct FakeSnaptrade {
    pub state: std::sync::Arc<std::sync::Mutex<FakeSnapState>>,
}

#[derive(Debug, Default)]
pub struct FakeSnapState {
    pub client_id: String,
    pub consumer_key: String,
    /// `results` of list-all-accounts.
    pub accounts: Vec<Value>,
    /// Activities by account id.
    pub activities: BTreeMap<String, Vec<Value>>,
    pub positions: BTreeMap<String, Vec<Value>>,
    pub balances: BTreeMap<String, Vec<Value>>,
    /// Answer this many requests with 429 and `Retry-After` first.
    pub throttle: usize,
    pub retry_after: u64,
    /// Answer this many requests with 401 first (a refusal that a fresh signature
    /// gets past).
    pub refuse: usize,
    /// Answer every request with this status (and body).
    pub status: Option<(u16, String)>,
    /// Answer one kind of request (`accounts`, `activities`, `positions`, `balances`)
    /// with this status (and body).
    pub fail: BTreeMap<String, (u16, String)>,
    /// URLs seen and whether the signature was right.
    pub requests: Vec<(String, bool)>,
    /// The fake clock (milliseconds): waits advance it instead of sleeping.
    pub clock_ms: u64,
    /// The waits asked for, in milliseconds.
    pub waits: Vec<u64>,
    /// The clock at each request (same order as `requests`).
    pub times: Vec<u64>,
    /// Send this `Deprecation` header with every answer.
    pub deprecation: Option<String>,
}

impl FakeSnaptrade {
    pub fn with<T>(&self, f: impl FnOnce(&mut FakeSnapState) -> T) -> Option<T> {
        self.state.lock().ok().map(|mut s| f(&mut s))
    }

    pub fn requests(&self) -> Vec<(String, bool)> {
        self.state
            .lock()
            .map(|s| s.requests.clone())
            .unwrap_or_default()
    }

    /// Two accounts (one investment, one deposit) with the test key.
    pub fn standard() -> Self {
        let f = FakeSnaptrade::default();
        f.with(|s| {
            s.client_id = "TESTCLIENT".into();
            s.consumer_key = "test-consumer-key".into();
            s.retry_after = 2;
            s.accounts = vec![
                json!({"kind": "investment", "id": "acct-1", "connection_id": "conn-1",
                    "display_name": "Schwab Individual", "masked_account_number": "****8443",
                    "institution_name": "Schwab",
                    "sync_status": {"transactions": {"initial_sync_completed": true,
                        "last_successful_sync": "2026-10-03T00:00:00.000Z",
                        "first_transaction_date": "2020-01-02T00:00:00.000Z"}},
                    "net_value": {"amount": 2311.7, "currency": "USD"}}),
                json!({"kind": "deposit", "id": "acct-2", "connection_id": "conn-1",
                    "display_name": "Schwab Checking", "masked_account_number": "****1290",
                    "sync_status": {"transactions": {"initial_sync_completed": false,
                        "last_successful_sync": null}},
                    "net_value": {"amount": 10.0, "currency": "USD"}}),
            ];
        });
        f
    }
}

fn query_of(url: &str) -> (&str, &str) {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path_q = rest
        .find('/')
        .map_or("", |i| rest.get(i..).unwrap_or_default());
    path_q.split_once('?').unwrap_or((path_q, ""))
}

fn param<'a>(query: &'a str, k: &str) -> Option<&'a str> {
    query
        .split('&')
        .find_map(|kv| kv.split_once('=').filter(|(a, _)| *a == k).map(|(_, v)| v))
}

impl crate::connect::Fetcher for FakeSnaptrade {
    fn wait(&self, ms: u64) {
        let _ = self.with(|s| {
            s.waits.push(ms);
            s.clock_ms = s.clock_ms.saturating_add(ms);
        });
    }

    fn clock_ms(&self) -> u64 {
        self.with(|s| s.clock_ms).unwrap_or(0)
    }

    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, FetchError> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| FetchError::Network("fake server unavailable".into()))?;
        let (path, query) = query_of(&req.url);
        let sig = req
            .headers
            .iter()
            .find(|h| h.0 == "Signature")
            .map(|h| h.1.clone())
            .unwrap_or_default();
        let ok = param(query, "clientId") == Some(s.client_id.as_str())
            && param(query, "timestamp").is_some()
            && sig == st::signature(&s.consumer_key, path, query, None);
        s.requests.push((req.url.clone(), ok));
        let at = s.clock_ms;
        s.times.push(at);
        // SnapTrade still answers the legacy prefix, with a `Deprecation` header.
        let (rest, deprecation) = match path.strip_prefix(st::LEGACY_PREFIX) {
            Some(r) => (r.to_owned(), Some("@1781222400".to_owned())),
            None => (path.to_owned(), s.deprecation.clone()),
        };
        let answer = |status: u16, v: Value| {
            Ok(HttpResponse {
                status,
                body: v.to_string().into_bytes(),
                retry_after: None,
                deprecation: deprecation.clone(),
            })
        };
        if !ok {
            return answer(
                401,
                json!({"detail": "Unable to verify signature sent", "status_code": 401, "code": "1076"}),
            );
        }
        if s.refuse > 0 {
            s.refuse -= 1;
            return answer(
                401,
                json!({"detail": "Unable to verify signature sent", "status_code": 401}),
            );
        }
        if let Some((code, body)) = s.status.clone() {
            return Ok(HttpResponse {
                status: code,
                body: body.into_bytes(),
                retry_after: None,
                deprecation: None,
            });
        }
        if s.throttle > 0 {
            s.throttle -= 1;
            return Ok(HttpResponse {
                status: 429,
                body: Vec::new(),
                retry_after: Some(s.retry_after),
                deprecation: None,
            });
        }
        let parts: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
        let kind = match parts.as_slice() {
            ["accounts", "all"] => "accounts",
            ["accounts", _, "activities"] => "activities",
            ["accounts", _, "positions", "all"] => "positions",
            ["accounts", _, "balances"] => "balances",
            _ => "",
        };
        if let Some((code, body)) = s.fail.get(kind).cloned() {
            return Ok(HttpResponse {
                status: code,
                body: body.into_bytes(),
                retry_after: None,
                deprecation: deprecation.clone(),
            });
        }
        if kind == "accounts" {
            return answer(200, json!({"results": s.accounts}));
        }
        match parts.as_slice() {
            ["accounts", id, "activities"] => {
                let all = s.activities.get(*id).cloned().unwrap_or_default();
                let offset: usize = param(query, "offset")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let limit: usize = param(query, "limit")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1000);
                let page: Vec<Value> = all
                    .iter()
                    .skip(offset)
                    .take(limit.min(1000))
                    .cloned()
                    .collect();
                answer(
                    200,
                    json!({"data": page, "pagination": {"offset": offset, "limit": limit, "total": all.len()}}),
                )
            }
            ["accounts", id, "positions", "all"] => answer(
                200,
                json!({"results": s.positions.get(*id).cloned().unwrap_or_default(),
                       "data_freshness": {"as_of": "2026-10-03T12:00:00Z"}}),
            ),
            // The deprecated per-account positions: 410 for newer keys.
            ["accounts", _, "positions"] => answer(
                410,
                json!({"detail": "This endpoint is no longer available", "status_code": 410}),
            ),
            ["accounts", id, "balances"] => {
                answer(200, json!(s.balances.get(*id).cloned().unwrap_or_default()))
            }
            _ => answer(404, json!({})),
        }
    }
}
