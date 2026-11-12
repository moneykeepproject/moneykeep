//! OFX Direct Connect connections (connectivity-brokerage.md part A): institution
//! profiles (installed from definition files or entered by hand), the credential and
//! the client unique id, connect (profile, account information, challenges), syncs
//! (statements in one request or one per account), status codes and commands.
//!
//! Requests and answers are never logged; error messages never include a request
//! body. Requests and their parsing are in `mk_import::ofx_dc`; statements are read by
//! the OFX file parser and staged like a file import.

use std::collections::BTreeMap;

use mk_core::{AccountId, Date};
use mk_import::ofx_dc::{self, Challenge, Institution, Reply, SignOn, Status, Txn};
use mk_import::{Parser, Statement};
use mk_store::{Doc, DocTable};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::connect::{
    ConnDoc, Connector, FetchError, HttpMethod, HttpRequest, HttpResponse, LinkDoc, backoff,
    note_error,
};
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) const PROVIDER: &str = "ofx";
pub(crate) const MODULE: &str = "bank-feeds";
/// Days re-read before the last statement's end.
const OVERLAP_DAYS: i32 = 7;

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

fn rejected(m: impl Into<String>) -> CommandError {
    CommandError::Rejected(m.into())
}

fn coded(code: &'static str, m: impl Into<String>) -> CommandError {
    CommandError::Coded {
        code,
        message: m.into(),
    }
}

// ----- records -------------------------------------------------------------------------

/// The stored credential: user id, password and what the server hands back for the
/// next signon (one secret, connectivity.md).
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct OfxSecret {
    pub user_id: String,
    /// None with "Ask for my password each time".
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub cred1: Option<String>,
    #[serde(default)]
    pub cred2: Option<String>,
    #[serde(default)]
    pub access_key: Option<String>,
    #[serde(default)]
    pub sess_cookie: Option<String>,
    #[serde(default)]
    pub user_key: Option<String>,
    #[serde(default)]
    pub key_expire: Option<String>,
}

impl std::fmt::Debug for OfxSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OfxSecret(hidden)")
    }
}

/// What the user supplies for one sync only (never stored).
#[derive(Clone, Default)]
pub struct Extra {
    pub(crate) answers: Vec<(String, String)>,
    pub(crate) auth_token: Option<String>,
    pub(crate) password: Option<String>,
}

impl std::fmt::Debug for Extra {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Extra(hidden)")
    }
}

/// A connection's OFX settings (no secrets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OfxSettings {
    /// The connection's own copy of the profile (a later definition update does not
    /// change it).
    pub institution: Institution,
    /// The installed profile it came from (its name).
    #[serde(default)]
    pub definition: Option<String>,
    /// The server refused several accounts in one request: one request per account.
    #[serde(default)]
    pub single_requests: bool,
    #[serde(default)]
    pub ask_password: bool,
    /// Questions waiting for answers (status 3000).
    #[serde(default)]
    pub challenge: Vec<Challenge>,
    /// The server asks for a one-time code (status 15512/15513).
    #[serde(default)]
    pub needs_auth_token: bool,
    #[serde(default)]
    pub cred1_label: Option<String>,
    #[serde(default)]
    pub cred2_label: Option<String>,
}

fn secret_text(s: &OfxSecret) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

fn secret_parse(text: &str) -> Option<OfxSecret> {
    serde_json::from_str(text).ok()
}

// ----- installed profiles (definition kind `ofx_institution`) ---------------------------

pub(crate) fn institutions(ctx: &Ctx<'_>) -> CommandResult<Vec<Institution>> {
    let mut v: Vec<Institution> = ctx
        .tx
        .docs(DocTable::OfxInstitution, None)?
        .into_iter()
        .filter_map(|d| serde_json::from_value(d.json).ok())
        .collect();
    v.sort_by_key(|i| i.name.to_lowercase());
    Ok(v)
}

pub(crate) fn installed(ctx: &Ctx<'_>, name: &str) -> CommandResult<Option<Institution>> {
    Ok(institutions(ctx)?
        .into_iter()
        .find(|i| i.name.eq_ignore_ascii_case(name.trim())))
}

pub(crate) fn save_institution(ctx: &Ctx<'_>, i: &Institution) -> CommandResult<()> {
    let id = i.name.to_lowercase();
    ctx.save_doc(
        DocTable::OfxInstitution,
        &id,
        Some(&Doc {
            id: id.clone(),
            grp: None,
            ord: 0,
            json: serde_json::to_value(i)?,
        }),
    )
}

pub(crate) fn delete_institution(ctx: &Ctx<'_>, name: &str) -> CommandResult<()> {
    ctx.save_doc(DocTable::OfxInstitution, &name.to_lowercase(), None)
}

// ----- client unique id --------------------------------------------------------------------

static FALLBACK_UIDS: std::sync::OnceLock<std::sync::Mutex<BTreeMap<String, String>>> =
    std::sync::OnceLock::new();

fn new_uid() -> String {
    mk_core::Id::<()>::new().to_string().to_ascii_uppercase()
}

/// The client unique id of this book on this computer: kept in the computer's
/// credential store (never in the book file, so a copied book gets its own id), or for
/// this run only when the computer has no store.
pub(crate) fn client_uid(book: &str, c: &Connector) -> String {
    let key = format!("{book}:ofx-client-uid");
    if c.secrets.available() {
        if let Ok(Some(v)) = c.secrets.get(&key)
            && !v.is_empty()
        {
            return v;
        }
        let uid = new_uid();
        if c.secrets.put(&key, &uid).is_ok() {
            return uid;
        }
    }
    let map = FALLBACK_UIDS.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()));
    match map.lock() {
        Ok(mut m) => m.entry(key).or_insert_with(new_uid).clone(),
        Err(_) => new_uid(),
    }
}

// ----- requests ------------------------------------------------------------------------------

/// A POST of an OFX request to the profile's URL (only its host is allowed).
pub(crate) fn http(inst: &Institution, body: String) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Post,
        url: inst.url.clone(),
        basic: None,
        authority: inst.authority(),
        max_bytes: ofx_dc::MAX_RESPONSE_BYTES,
        headers: vec![
            ("Content-Type".into(), "application/x-ofx".into()),
            ("Accept".into(), "application/x-ofx".into()),
        ],
        body: Some(body.into_bytes()),
        allow_http: false,
    }
}

fn uid_fn() -> impl FnMut() -> String {
    || mk_core::Id::<()>::new().to_string().to_ascii_uppercase()
}

fn build(inst: &Institution, s: &SignOn, txns: &[Txn], now: i64) -> HttpRequest {
    let mut uid = uid_fn();
    http(inst, ofx_dc::request(inst, s, txns, now, &mut uid))
}

/// The answers for a challenge: the user's, plus the client-answered ones MoneyKeep
/// gives itself (date and time, user agent) when the user left them empty.
fn answers_for(
    questions: &[Challenge],
    given: &[(String, String)],
    now: i64,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = given
        .iter()
        .filter(|(_, a)| !a.trim().is_empty())
        .map(|(i, a)| (i.clone(), a.trim().to_owned()))
        .collect();
    let ids: Vec<&str> = questions
        .iter()
        .map(|q| q.id.as_str())
        .chain(given.iter().map(|(i, _)| i.as_str()))
        .collect();
    for id in ids {
        if out.iter().any(|(i, _)| i == id) {
            continue;
        }
        if let Some(a) = ofx_dc::automatic_answer(id, now) {
            out.push((id.to_owned(), a));
        }
    }
    out
}

fn signon_of(s: &OfxSecret, uid: &str, extra: &Extra, questions: &[Challenge], now: i64) -> SignOn {
    SignOn {
        user_id: s.user_id.clone(),
        password: extra
            .password
            .clone()
            .or_else(|| s.password.clone())
            .unwrap_or_default(),
        cred1: s.cred1.clone(),
        cred2: s.cred2.clone(),
        client_uid: Some(uid.to_owned()),
        auth_token: extra.auth_token.clone(),
        access_key: s.access_key.clone(),
        sess_cookie: s.sess_cookie.clone(),
        user_key: s.user_key.clone(),
        answers: if extra.answers.is_empty() {
            Vec::new()
        } else {
            answers_for(questions, &extra.answers, now)
        },
    }
}

// ----- status codes ---------------------------------------------------------------------------

/// What a signon status means for the connection (table of connectivity-brokerage.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Signon {
    Ok,
    Challenge,
    AnswerRejected,
    AuthToken,
    /// Needs attention: (code, message, disable).
    Attention(&'static str, String, bool),
    /// Try again later with backoff.
    Retry(String),
}

pub(crate) fn classify(st: &Status, version: &str, uid: &str) -> Signon {
    let mfa = ofx_dc::mfa_version(version);
    let old = || {
        Signon::Attention(
            "ofx_version",
            "Choose OFX version 103 or later for this institution".into(),
            false,
        )
    };
    match st.code.as_str() {
        "0" => Signon::Ok,
        "3000" if mfa => Signon::Challenge,
        "3001" if mfa => Signon::AnswerRejected,
        "15512" | "15513" if mfa => Signon::AuthToken,
        "3000" | "3001" | "15510" | "15512" | "15513" if !mfa => old(),
        "15000" => Signon::Attention(
            "password_change",
            "Your bank requires a new password — change it on the bank's site, then update it here (MoneyKeep does not change passwords)".into(),
            false,
        ),
        "15500" => Signon::Attention(
            "signon_rejected",
            "User id or password not accepted".into(),
            false,
        ),
        "15502" => Signon::Attention(
            "locked",
            "Your bank has locked this sign-in".into(),
            true,
        ),
        "15510" => Signon::Attention(
            "client_uid",
            format!("Your bank does not accept this client id; register it with your bank: {uid}"),
            false,
        ),
        "15514" | "15515" | "15516" => Signon::Attention(
            "access_token",
            "This bank needs an access token MoneyKeep can't obtain".into(),
            false,
        ),
        "13504" => Signon::Attention(
            "fi",
            "Institution id (FI) missing or wrong — check the profile".into(),
            false,
        ),
        "13505" => Signon::Retry(
            "Your bank's OFX server is under maintenance; MoneyKeep will try again later.".into(),
        ),
        code => Signon::Attention(
            "ofx_status",
            if st.message.is_empty() {
                format!("Your bank answered with code {code}")
            } else {
                format!("Your bank answered with code {code}: {}", st.message)
            },
            false,
        ),
    }
}

const HTML: &str = "This address did not answer as an OFX server";

// ----- sync job --------------------------------------------------------------------------------

/// A sync's OFX work, done with no book transaction open.
#[derive(Clone)]
pub struct OfxJob {
    pub(crate) inst: Institution,
    pub(crate) signon: SignOn,
    /// (link id, statement request).
    pub(crate) accounts: Vec<(String, Txn)>,
    pub(crate) single: bool,
    pub(crate) interactive: bool,
    pub(crate) now: i64,
}

impl std::fmt::Debug for OfxJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OfxJob")
            .field("host", &self.inst.host())
            .field("accounts", &self.accounts.len())
            .finish_non_exhaustive()
    }
}

/// What an OFX sync read.
#[derive(Debug, Clone)]
pub enum OfxResult {
    Replies {
        replies: Vec<(Vec<u8>, Reply)>,
        fallback: bool,
    },
    Signon(Status),
    Challenge(Vec<Challenge>),
    Html,
    Http(u16),
    Fetch(FetchError),
    Parse(String),
    Cancelled,
}

type Send<'a> = dyn FnMut(HttpRequest) -> Option<Result<HttpResponse, FetchError>> + 'a;

fn send_one(send: &mut Send<'_>, req: HttpRequest) -> Result<(Vec<u8>, Reply), OfxResult> {
    let resp = match send(req) {
        None => return Err(OfxResult::Cancelled),
        Some(Err(e)) => return Err(OfxResult::Fetch(e)),
        Some(Ok(r)) => r,
    };
    if ofx_dc::looks_like_html(&resp.body) {
        return Err(OfxResult::Html);
    }
    if resp.status != 200 {
        return Err(OfxResult::Http(resp.status));
    }
    let reply = ofx_dc::parse_reply(&resp.body).map_err(|e| {
        if ofx_dc::looks_like_html(&resp.body) {
            OfxResult::Html
        } else {
            OfxResult::Parse(e.to_string())
        }
    })?;
    Ok((resp.body, reply))
}

/// Read the statements of a connection: one request for all accounts unless the
/// server refused that before (or refuses it now: a 2000 on a later statement), then
/// one request per account. A signon refusal stops at once.
pub(crate) fn run(job: &OfxJob, send: &mut Send<'_>) -> OfxResult {
    let multi = !job.single && job.accounts.len() > 1;
    let groups: Vec<Vec<Txn>> = if multi {
        vec![job.accounts.iter().map(|(_, t)| t.clone()).collect()]
    } else {
        job.accounts.iter().map(|(_, t)| vec![t.clone()]).collect()
    };
    let mut replies = Vec::new();
    let mut fallback = false;
    let mut i = 0;
    let mut groups = groups;
    while let Some(group) = groups.get(i).cloned() {
        let mut txns = Vec::new();
        if i == 0 {
            txns.push(Txn::AccountInfo);
        }
        txns.extend(group.iter().cloned());
        let (body, reply) = match send_one(send, build(&job.inst, &job.signon, &txns, job.now)) {
            Ok(x) => x,
            Err(e) => return e,
        };
        if !reply.signon.ok() {
            if reply.signon.code == "3000"
                && job.interactive
                && ofx_dc::mfa_version(&job.inst.ofx_version)
            {
                let req = build(&job.inst, &job.signon, &[Txn::Challenge], job.now);
                return match send_one(send, req) {
                    Ok((_, r)) if !r.challenges.is_empty() => OfxResult::Challenge(r.challenges),
                    Ok((_, r)) => OfxResult::Signon(r.challenge_status.unwrap_or(r.signon)),
                    Err(e) => e,
                };
            }
            return OfxResult::Signon(reply.signon);
        }
        if group.len() > 1
            && reply
                .statements
                .iter()
                .skip(1)
                .any(|s| s.status.code == "2000")
        {
            // The server takes one account per request: start again that way.
            fallback = true;
            groups = job.accounts.iter().map(|(_, t)| vec![t.clone()]).collect();
            replies.clear();
            i = 0;
            continue;
        }
        replies.push((body, reply));
        i += 1;
    }
    OfxResult::Replies { replies, fallback }
}

// ----- connect (network steps) ------------------------------------------------------------------

/// `connect.add` for an OFX institution.
#[derive(Clone)]
pub struct AddState {
    pub(crate) conn: String,
    pub(crate) home: String,
    pub(crate) label: Option<String>,
    pub(crate) inst: Institution,
    pub(crate) definition: Option<String>,
    pub(crate) secret: OfxSecret,
    pub(crate) ask_password: bool,
    pub(crate) extra: Extra,
    pub(crate) uid: String,
    pub(crate) now: i64,
    pub(crate) profile: Option<ofx_dc::SignonInfo>,
}

impl std::fmt::Debug for AddState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddState")
            .field("host", &self.inst.host())
            .finish_non_exhaustive()
    }
}

/// The step an OFX connect is at.
#[derive(Debug, Clone)]
pub enum AddStep {
    Profile(Box<AddState>),
    Accounts(Box<AddState>),
    Challenge(Box<AddState>),
}

/// The arguments of `connect.add` for OFX (parsed by `connect`).
pub(crate) struct AddInput {
    pub institution: Option<Institution>,
    pub institution_key: Option<String>,
    pub user_id: String,
    pub password: String,
    pub cred1: Option<String>,
    pub cred2: Option<String>,
    pub host_acknowledged: bool,
    pub ask_password: bool,
    pub answers: Vec<(String, String)>,
    pub auth_token: Option<String>,
    pub label: Option<String>,
}

/// Plan the first request of an OFX connect: the anonymous profile request.
pub(crate) fn start_add(
    ctx: &Ctx<'_>,
    c: &Connector,
    input: AddInput,
    home: &str,
) -> CommandResult<(AddState, HttpRequest)> {
    let (mut inst, definition) = match (input.institution, input.institution_key) {
        (Some(i), _) => (i, None),
        (None, Some(k)) => {
            let i = installed(ctx, &k)?
                .ok_or_else(|| CommandError::NotFound(format!("institution profile {k:?}")))?;
            let name = i.name.clone();
            (i, Some(name))
        }
        (None, None) => {
            return Err(invalid(
                "give the institution profile (institution) or an installed one (institution_key)",
            ));
        }
    };
    if inst.ofx_version.trim().is_empty() {
        inst.ofx_version = crate::module_settings::get_str(ctx, "bank-feeds.ofx_default_version")?;
    }
    inst.check().map_err(|e| invalid(e.to_string()))?;
    let host = inst.host();
    if !input.host_acknowledged {
        return Err(coded(
            "host_not_acknowledged",
            format!(
                "Your user id and password will be sent to {host}. Confirm that you recognise this address as your bank's (host_acknowledged) to connect."
            ),
        ));
    }
    let user_id = mk_core::text::clean(&input.user_id, 32)
        .text
        .trim()
        .to_owned();
    if user_id.is_empty() || input.password.is_empty() {
        return Err(invalid("give the user id and password"));
    }
    if input.password.chars().count() > 32 {
        return Err(invalid(
            "the password is longer than 32 characters (the OFX limit)",
        ));
    }
    let book = ctx.tx.info()?.id;
    let uid = client_uid(&book, c);
    let now = c.now();
    let st = AddState {
        conn: mk_core::Id::<()>::new().to_string(),
        home: home.to_owned(),
        label: input.label,
        inst,
        definition,
        secret: OfxSecret {
            user_id,
            password: Some(input.password),
            cred1: input.cred1.filter(|x| !x.is_empty()),
            cred2: input.cred2.filter(|x| !x.is_empty()),
            ..OfxSecret::default()
        },
        ask_password: input.ask_password,
        extra: Extra {
            answers: input.answers,
            auth_token: input.auth_token.filter(|x| !x.is_empty()),
            password: None,
        },
        uid,
        now,
        profile: None,
    };
    let req = build(&st.inst, &SignOn::anonymous(), &[Txn::Profile], now);
    Ok((st, req))
}

fn reply_of(first: &Result<HttpResponse, FetchError>, host: &str) -> CommandResult<Reply> {
    let resp = first
        .as_ref()
        .map_err(|e| rejected(format!("{host} could not be reached: {e}")))?;
    if ofx_dc::looks_like_html(&resp.body) {
        return Err(coded("not_ofx", HTML));
    }
    if resp.status != 200 {
        return Err(rejected(format!(
            "{host} answered with HTTP {}",
            resp.status
        )));
    }
    ofx_dc::parse_reply(&resp.body).map_err(|e| rejected(format!("The answer was refused: {e}")))
}

fn questions_json(q: &[Challenge]) -> Value {
    Value::Array(
        q.iter()
            .map(|c| json!({"id": c.id, "label": c.label, "automatic": c.automatic}))
            .collect(),
    )
}

/// What to do after an OFX connect answer: the next request, or the result.
#[allow(clippy::large_enum_variant)]
pub(crate) enum AddNext {
    Request(AddStep, HttpRequest),
    Done(Value),
    /// Account information came back: create the connection.
    Create(Box<AddState>, Reply),
}

pub(crate) fn resume_add(
    step: AddStep,
    first: &Result<HttpResponse, FetchError>,
) -> CommandResult<AddNext> {
    match step {
        AddStep::Profile(mut st) => {
            let host = st.inst.host();
            let reply = reply_of(first, &host)?;
            let info = reply.profile.clone().unwrap_or_default();
            let mut needs = Vec::new();
            if let Some(l) = &info.cred1_label
                && st.secret.cred1.is_none()
            {
                needs.push(json!({"field": "cred1", "label": l}));
            }
            if let Some(l) = &info.cred2_label
                && st.secret.cred2.is_none()
            {
                needs.push(json!({"field": "cred2", "label": l}));
            }
            if !needs.is_empty() {
                return Ok(AddNext::Done(json!({
                    "needs_credentials": needs, "host": host, "client_uid": st.uid,
                    "client_uid_required": info.client_uid_required,
                    "message": "Your bank asks for more sign-in details.",
                })));
            }
            let first_txn = if info.challenge_first && st.extra.answers.is_empty() {
                Txn::Challenge
            } else {
                Txn::AccountInfo
            };
            st.profile = Some(info);
            let s = signon_of(&st.secret, &st.uid, &st.extra, &[], st.now);
            let req = build(&st.inst, &s, std::slice::from_ref(&first_txn), st.now);
            Ok(AddNext::Request(
                if first_txn == Txn::Challenge {
                    AddStep::Challenge(st)
                } else {
                    AddStep::Accounts(st)
                },
                req,
            ))
        }
        AddStep::Accounts(st) => {
            let host = st.inst.host();
            let reply = reply_of(first, &host)?;
            match classify(&reply.signon, &st.inst.ofx_version, &st.uid) {
                Signon::Ok => Ok(AddNext::Create(st, reply)),
                Signon::Challenge => {
                    let s = signon_of(&st.secret, &st.uid, &Extra::default(), &[], st.now);
                    let req = build(&st.inst, &s, &[Txn::Challenge], st.now);
                    Ok(AddNext::Request(AddStep::Challenge(st), req))
                }
                Signon::AnswerRejected => Err(coded(
                    "answer_rejected",
                    "An answer was not accepted — try again",
                )),
                Signon::AuthToken => Ok(AddNext::Done(json!({
                    "needs_auth_token": st.profile.as_ref().and_then(|p| p.auth_token_label.clone())
                        .unwrap_or_else(|| "One-time code from your bank".into()),
                    "host": host, "client_uid": st.uid,
                    "message": "Your bank sent a one-time code; enter it to connect.",
                }))),
                Signon::Attention(code, m, _) => Err(coded(code, m)),
                Signon::Retry(m) => Err(rejected(m)),
            }
        }
        AddStep::Challenge(st) => {
            let host = st.inst.host();
            let reply = reply_of(first, &host)?;
            if reply.challenges.is_empty() {
                let s = reply.challenge_status.unwrap_or(reply.signon);
                return match classify(&s, &st.inst.ofx_version, &st.uid) {
                    Signon::Attention(code, m, _) => Err(coded(code, m)),
                    _ => Err(rejected(
                        "Your bank asked for extra questions but sent none",
                    )),
                };
            }
            Ok(AddNext::Done(json!({
                "challenge": questions_json(&reply.challenges),
                "host": host, "client_uid": st.uid,
                "message": "Your bank asks extra questions.",
            })))
        }
    }
}

/// A remote account's key within its connection.
fn remote_key(a: &ofx_dc::RemoteAccount) -> String {
    format!(
        "{}:{}:{}",
        a.kind,
        if a.kind == "investment" {
            &a.broker_id
        } else {
            &a.bank_id
        },
        a.acct_id
    )
}

fn masked(acct: &str) -> String {
    mk_import::snaptrade::mask(acct)
}

fn link_name(a: &ofx_dc::RemoteAccount) -> String {
    let kind = match a.kind.as_str() {
        "bank" => {
            let t = a.acct_type.to_lowercase();
            if t.is_empty() { "bank".to_owned() } else { t }
        }
        "card" => "credit card".into(),
        _ => "investment".into(),
    };
    let base = if a.description.is_empty() {
        let mut k = kind.clone();
        if let Some(f) = k.get_mut(..1) {
            f.make_ascii_uppercase();
        }
        k
    } else {
        a.description.clone()
    };
    format!("{base} {}", masked(&a.acct_id)).trim().to_owned()
}

/// Create or refresh links for the accounts the bank listed.
pub(crate) fn apply_accounts(
    ctx: &Ctx<'_>,
    conn: &ConnDoc,
    list: &[ofx_dc::RemoteAccount],
) -> CommandResult<()> {
    let existing = crate::connect::links_of(ctx, &conn.id)?;
    for a in list {
        let key = remote_key(a);
        let id = crate::connect::link_id_of(&conn.id, &key);
        let mut l = existing
            .iter()
            .find(|l| l.id == id)
            .cloned()
            .unwrap_or_else(|| crate::connect::new_link(conn, &id, &key, &format!("ofxdc:{id}")));
        l.name = link_name(a);
        let mut r = l.remote.clone().unwrap_or_default();
        r.kind = a.kind.clone();
        r.bank_id = a.bank_id.clone();
        r.broker_id = a.broker_id.clone();
        r.acct_type = a.acct_type.clone();
        r.status = a.status.clone();
        r.masked = masked(&a.acct_id);
        r.acct_id = a.acct_id.clone();
        l.remote = Some(r);
        crate::connect::save_link_doc(ctx, &l)?;
    }
    Ok(())
}

/// The connection record of a new OFX connection.
pub(crate) fn new_conn(ctx: &Ctx<'_>, st: &AddState, sid: &str) -> CommandResult<ConnDoc> {
    let label = st
        .label
        .clone()
        .map(|l| mk_core::text::clean(&l, 100).text.trim().to_owned())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| st.inst.name.clone());
    let mut doc = crate::connect::blank_conn(
        ctx,
        &st.conn,
        PROVIDER,
        &label,
        &st.home,
        sid,
        &st.inst.host(),
    )?;
    let info = st.profile.clone().unwrap_or_default();
    doc.ofx = Some(OfxSettings {
        institution: st.inst.clone(),
        definition: st.definition.clone(),
        single_requests: false,
        ask_password: st.ask_password,
        challenge: Vec::new(),
        needs_auth_token: false,
        cred1_label: info.cred1_label,
        cred2_label: info.cred2_label,
    });
    Ok(doc)
}

/// The secret to store for a new connection (no password with "ask each time").
pub(crate) fn stored_secret(st: &AddState, reply: &Reply) -> String {
    let mut s = st.secret.clone();
    if st.ask_password {
        s.password = None;
    }
    s.access_key = reply.access_key.clone();
    s.sess_cookie = reply.sess_cookie.clone();
    s.user_key = reply.user_key.clone();
    s.key_expire = reply.key_expire.clone();
    secret_text(&s)
}

// ----- sync: plan and apply -------------------------------------------------------------------

fn date_of_ofx(t: &str) -> Option<Date> {
    let d = t.get(..8)?;
    format!("{}-{}-{}", d.get(..4)?, d.get(4..6)?, d.get(6..8)?)
        .parse()
        .ok()
}

/// The work of one OFX connection's sync, or why it is skipped.
pub(crate) fn plan(
    ctx: &Ctx<'_>,
    c: &Connector,
    conn: &ConnDoc,
    secret_text: &str,
    by_user: bool,
    extra: Option<&Extra>,
    from: Option<Date>,
) -> CommandResult<Result<OfxJob, String>> {
    let Some(settings) = conn.ofx.clone() else {
        return Ok(Err("the connection has no OFX profile".into()));
    };
    let Some(secret) = secret_parse(secret_text) else {
        return Ok(Err("the stored credential is damaged; reconnect".into()));
    };
    let extra = extra.cloned().unwrap_or_default();
    if secret.password.is_none() && extra.password.is_none() {
        return Ok(Err(NEEDS_PASSWORD.into()));
    }
    let now = c.now();
    let today = mk_import::simplefin::epoch_date(now, c.utc_offset_secs)
        .map_err(|e| invalid(e.to_string()))?;
    let history = crate::module_settings::get_i64(ctx, "bank-feeds.history_days_first_sync")?;
    let history = i32::try_from(history).unwrap_or(90);
    let mut accounts = Vec::new();
    for l in crate::connect::links_of(ctx, &conn.id)? {
        let Some(r) = l.remote.clone() else { continue };
        if !l.active || l.account.is_none() || r.status != "ACTIVE" {
            continue;
        }
        let start = match r.last_dtend {
            Some(d) => d.add_days(-OVERLAP_DAYS).unwrap_or(d),
            None => l
                .sync_from
                .or_else(|| today.add_days(-history))
                .unwrap_or(today),
        };
        let start = ofx_dc::ofx_date(from.unwrap_or(start));
        let txn = match r.kind.as_str() {
            "bank" => Txn::Bank {
                bank_id: r.bank_id.clone(),
                acct_id: r.acct_id.clone(),
                acct_type: if r.acct_type.is_empty() {
                    "CHECKING".into()
                } else {
                    r.acct_type.clone()
                },
                start,
            },
            "card" => Txn::Card {
                acct_id: r.acct_id.clone(),
                start,
            },
            _ => Txn::Invest {
                broker_id: if r.broker_id.is_empty() {
                    settings.institution.broker_id.clone()
                } else {
                    r.broker_id.clone()
                },
                acct_id: r.acct_id.clone(),
                start,
            },
        };
        accounts.push((l.id.clone(), txn));
    }
    if accounts.is_empty() {
        return Ok(Err("no remote account is linked yet".into()));
    }
    let book = ctx.tx.info()?.id;
    let uid = client_uid(&book, c);
    Ok(Ok(OfxJob {
        signon: signon_of(&secret, &uid, &extra, &settings.challenge, now),
        inst: settings.institution,
        accounts,
        single: settings.single_requests,
        interactive: by_user,
        now,
    }))
}

/// The skip reason of a connection set to ask for the password.
pub(crate) const NEEDS_PASSWORD: &str = "Sync needs your password";

fn attention(conn: &mut ConnDoc, code: &str, message: &str) -> Value {
    conn.status = "needs_attention".into();
    note_error(conn, code, message);
    crate::connect::error_json(conn)
}

fn update_secret(ctx: &Ctx<'_>, c: &Connector, conn: &ConnDoc, reply: &Reply) -> CommandResult<()> {
    let Some(r) = conn.secret_ref.clone() else {
        return Ok(());
    };
    let Some(text) = crate::connect::get_secret(ctx, c, &r)? else {
        return Ok(());
    };
    let Some(mut s) = secret_parse(&text) else {
        return Ok(());
    };
    let before = s.clone();
    if reply.access_key.is_some() {
        s.access_key.clone_from(&reply.access_key);
    }
    if reply.sess_cookie.is_some() {
        s.sess_cookie.clone_from(&reply.sess_cookie);
    }
    if reply.user_key.is_some() {
        s.user_key.clone_from(&reply.user_key);
        s.key_expire.clone_from(&reply.key_expire);
    }
    if s != before {
        let text = secret_text(&s);
        if r.store == "book" {
            ctx.tx.put_secret(&r.id, &text)?;
        } else {
            c.secrets
                .put(&r.id, &text)
                .map_err(|e| rejected(format!("the credential store refused: {e}")))?;
        }
    }
    Ok(())
}

/// Apply an OFX sync in the sync's write.
pub(crate) fn apply(
    ctx: &Ctx<'_>,
    c: &Connector,
    conn: &mut ConnDoc,
    now: i64,
    result: OfxResult,
) -> CommandResult<Value> {
    let Some(mut settings) = conn.ofx.clone() else {
        return Ok(attention(
            conn,
            "no_profile",
            "The connection has no OFX profile",
        ));
    };
    let book = ctx.tx.info()?.id;
    let uid = client_uid(&book, c);
    let (replies, fallback) = match result {
        OfxResult::Replies { replies, fallback } => (replies, fallback),
        OfxResult::Html => return Ok(attention(conn, "not_ofx", HTML)),
        OfxResult::Http(s) if s >= 500 => {
            note_error(
                conn,
                "server",
                &format!(
                    "The bank's server answered with HTTP {s}; MoneyKeep will try again later."
                ),
            );
            backoff(conn, now);
            return Ok(crate::connect::error_json(conn));
        }
        OfxResult::Http(s) => {
            return Ok(attention(
                conn,
                "http",
                &format!("The bank's server answered with HTTP {s}"),
            ));
        }
        OfxResult::Fetch(e) => {
            let code = match e {
                FetchError::Refused(_) => "refused",
                FetchError::Network(_) => "network",
            };
            note_error(conn, code, &e.to_string());
            backoff(conn, now);
            return Ok(crate::connect::error_json(conn));
        }
        OfxResult::Parse(m) => {
            note_error(
                conn,
                "bad_response",
                &format!("The answer was refused: {m}"),
            );
            backoff(conn, now);
            return Ok(crate::connect::error_json(conn));
        }
        OfxResult::Cancelled => {
            return Err(coded("cancelled", "Cancelled; nothing was recorded."));
        }
        OfxResult::Challenge(q) => {
            settings.challenge = q.clone();
            conn.ofx = Some(settings);
            let mut v = attention(conn, "challenge", "Your bank asks extra questions");
            if let Some(o) = v.as_object_mut() {
                o.insert("challenge".into(), questions_json(&q));
            }
            return Ok(v);
        }
        OfxResult::Signon(st) => {
            let v = match classify(&st, &settings.institution.ofx_version, &uid) {
                Signon::Ok => attention(conn, "ofx_status", "The bank's answer was incomplete"),
                Signon::Challenge => attention(
                    conn,
                    "challenge",
                    "Your bank asks extra questions — open Sync to answer",
                ),
                Signon::AnswerRejected => {
                    attention(conn, "answer_rejected", "An answer was not accepted")
                }
                Signon::AuthToken => {
                    settings.needs_auth_token = true;
                    attention(
                        conn,
                        "auth_token",
                        "Your bank asks for a one-time code — open Sync to enter it",
                    )
                }
                Signon::Attention(code, m, disable) => {
                    let v = attention(conn, code, &m);
                    if disable {
                        conn.status = "disabled".into();
                    }
                    v
                }
                Signon::Retry(m) => {
                    note_error(conn, "maintenance", &m);
                    backoff(conn, now);
                    crate::connect::error_json(conn)
                }
            };
            conn.ofx = Some(settings);
            return Ok(v);
        }
    };
    settings.challenge.clear();
    settings.needs_auth_token = false;
    settings.single_requests |= fallback;
    conn.ofx = Some(settings);
    conn.status = "active".into();
    conn.last_error = None;
    conn.failures = 0;
    conn.next_retry_at = None;
    conn.last_success = Some(now);
    if let Some((_, last)) = replies.last() {
        update_secret(ctx, c, conn, last)?;
    }
    if let Some(list) = replies.first().and_then(|(_, r)| r.accounts.clone()) {
        apply_accounts(ctx, conn, &list)?;
    }
    let mut links = crate::connect::links_of(ctx, &conn.id)?;
    let mut st = Statement::default();
    let mut auto = true;
    let mut staged_links = Vec::new();
    let mut digest_input = Vec::new();
    for (body, reply) in &replies {
        digest_input.extend_from_slice(body);
        let parsed = crate::import::ofx_parser(ctx)?
            .parse(body)
            .map_err(|e| rejected(format!("The bank's statement could not be read: {e}")))?;
        st.warnings.extend(parsed.warnings);
        for info in &reply.statements {
            let kind = match info.kind.as_str() {
                "cc" => "card",
                k => k,
            };
            let Some(l) = links.iter_mut().find(|l| {
                l.remote
                    .as_ref()
                    .is_some_and(|r| r.kind == kind && r.acct_id == info.acct_id)
            }) else {
                continue;
            };
            if !info.status.ok() {
                l.attention = Some(format!(
                    "The bank refused this account's statement (code {}{})",
                    info.status.code,
                    if info.status.message.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", info.status.message)
                    }
                ));
                crate::connect::save_link_doc(ctx, l)?;
            }
        }
        for mut sec in parsed.sections {
            let Some(info) = reply
                .statements
                .iter()
                .find(|s| Some(&s.key) == sec.account_key.as_ref())
            else {
                continue;
            };
            let kind = match info.kind.as_str() {
                "cc" => "card",
                k => k,
            };
            let Some(l) = links.iter_mut().find(|l| {
                l.remote
                    .as_ref()
                    .is_some_and(|r| r.kind == kind && r.acct_id == info.acct_id)
            }) else {
                continue;
            };
            if l.account.is_none() || !l.active {
                continue;
            }
            if let Some(r) = l.remote.as_mut()
                && let Some(d) = info.dtend.as_deref().and_then(date_of_ofx)
            {
                r.last_dtend = Some(d);
            }
            if let Some(cl) = &sec.closing {
                l.balance = Some(cl.amount.to_string());
                l.balance_date = Some(crate::connect::date_epoch(cl.date));
            }
            l.attention = None;
            l.last_success = Some(now);
            l.last_seen = Some(
                mk_import::simplefin::epoch_date(now, c.utc_offset_secs)
                    .map_err(|e| invalid(e.to_string()))?
                    .to_string(),
            );
            crate::connect::save_link_doc(ctx, l)?;
            sec.account_key = Some(l.account_key.clone());
            sec.account_name = Some(format!("{} — {}", conn.label, l.name));
            auto &= l.auto_commit;
            staged_links.push(l.id.clone());
            st.sections.push(sec);
        }
    }
    let label = conn.label.clone();
    let source = format!("OFX — {label}");
    crate::inv_import::store_feed_prices(ctx, &st, &source)?;
    let staged_any = st
        .sections
        .iter()
        .any(|s| !s.rows.is_empty() || !s.inv_rows.is_empty());
    if !staged_any {
        return Ok(
            json!({"connection": conn.id, "label": conn.label, "status": conn.status,
            "batch": null, "new": 0, "warnings": st.warnings, "fallback": fallback}),
        );
    }
    let staged = crate::connect::stage_feed(ctx, conn, st, "ofx", source, &digest_input)?;
    let mut out = staged.clone();
    let batch_id = staged
        .get("batch")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let holdings = crate::inv_import::feed_holdings(ctx, conn, &batch_id, "your bank")?;
    if let Some(o) = out.as_object_mut() {
        o.insert("holdings".into(), holdings);
        o.insert("fallback".into(), json!(fallback));
    }
    if auto && !staged_links.is_empty() {
        let sealed = crate::import::sealed_json(ctx, &batch_id)?;
        if crate::import::auto_committable(&sealed) {
            let committed = crate::import::commit_batch(ctx, &batch_id)?;
            if let Some(o) = out.as_object_mut() {
                o.insert("committed".into(), committed);
            }
        }
    }
    Ok(out)
}

// ----- commands ------------------------------------------------------------------------------

pub(crate) fn register(r: &mut Registry) {
    let m = "bank-feeds|payment-processing|investments";
    r.add_in(
        m,
        "connect.answer_challenge",
        "Answer a bank's extra questions, give a one-time code or this sync's password, and sync (handles credentials; not offered over MCP)",
        true,
        answer_challenge,
    );
    r.add_in(
        m,
        "connect.ofx_profile",
        "An OFX connection's institution profile (host, version, application id; no secrets)",
        false,
        ofx_profile,
    );
    r.add_in(
        m,
        "connect.set_ofx_profile",
        "Change an OFX connection's application id, version or OFX version, or move it to the installed profile's new host",
        true,
        set_ofx_profile,
    );
    r.add_in(
        m,
        "connect.client_uid",
        "The OFX client id of this book on this computer (to register with a bank)",
        false,
        client_uid_cmd,
    );
    r.add_in(
        m,
        "connect.ofx_institutions",
        "Installed OFX institution profiles (name, host, version; no secrets)",
        false,
        institutions_cmd,
    );
    r.add_in(
        m,
        "connect.holdings_check",
        "The latest holdings check of an account linked to OFX or SnapTrade: provider quantities and cash against MoneyKeep's after the staged import",
        false,
        holdings_check,
    );
    r.add_in(
        m,
        "connect.add_manual_account",
        "Add a remote account by hand to an OFX connection whose bank does not list accounts",
        true,
        add_manual_account,
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Answer {
    /// The question's id (`MFAPHRASEID`).
    pub id: String,
    pub answer: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnswerArgs {
    pub connection: String,
    /// Answers to the bank's questions.
    #[serde(default)]
    pub answers: Vec<Answer>,
    /// A one-time code from the bank.
    #[serde(default)]
    pub auth_token: Option<String>,
    /// The password for this sync ("Ask for my password each time").
    #[serde(default)]
    pub password: Option<String>,
}

impl AnswerArgs {
    pub(crate) fn extra(&self) -> Extra {
        Extra {
            answers: self
                .answers
                .iter()
                .map(|a| (a.id.clone(), a.answer.clone()))
                .collect(),
            auth_token: self.auth_token.clone().filter(|t| !t.is_empty()),
            password: self.password.clone().filter(|t| !t.is_empty()),
        }
    }
}

/// Registered for the schema and listings; the work is done in network steps.
fn answer_challenge(_: &Ctx<'_>, _: AnswerArgs) -> CommandResult<Value> {
    Err(CommandError::Integrity(
        "connect.answer_challenge runs through the registry's network steps".into(),
    ))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ConnArg {
    connection: String,
}

fn ofx_conn(ctx: &Ctx<'_>, r: &str) -> CommandResult<(ConnDoc, OfxSettings)> {
    let c = crate::connect::find_conn(ctx, r)?;
    let s = c
        .ofx
        .clone()
        .ok_or_else(|| rejected(format!("{} is not an OFX connection", c.label)))?;
    Ok((c, s))
}

/// A profile notice: the installed profile it came from now names another host.
pub(crate) fn profile_notice(ctx: &Ctx<'_>, s: &OfxSettings) -> CommandResult<Option<String>> {
    let Some(name) = &s.definition else {
        return Ok(None);
    };
    let Some(i) = installed(ctx, name)? else {
        return Ok(None);
    };
    let (old, new) = (s.institution.host(), i.host());
    Ok((old != new).then(|| format!("The installed profile now points to {new}")))
}

pub(crate) fn profile_json(ctx: &Ctx<'_>, c: &ConnDoc, s: &OfxSettings) -> CommandResult<Value> {
    let installed_host = match &s.definition {
        Some(n) => installed(ctx, n)?.map(|i| i.host()),
        None => None,
    };
    Ok(json!({
        "connection": c.id, "label": c.label, "name": s.institution.name,
        "url": s.institution.url, "host": s.institution.host(),
        "org": s.institution.org, "fid": s.institution.fid, "broker_id": s.institution.broker_id,
        "ofx_version": s.institution.ofx_version, "app_id": s.institution.app_id,
        "app_ver": s.institution.app_ver, "notes": s.institution.notes,
        "definition": s.definition, "installed_host": installed_host,
        "notice": profile_notice(ctx, s)?, "single_requests": s.single_requests,
        "ask_password": s.ask_password, "challenge": questions_json(&s.challenge),
        "needs_auth_token": s.needs_auth_token,
        "cred1_label": s.cred1_label, "cred2_label": s.cred2_label,
    }))
}

fn ofx_profile(ctx: &Ctx<'_>, a: ConnArg) -> CommandResult<Value> {
    let (c, s) = ofx_conn(ctx, &a.connection)?;
    profile_json(ctx, &c, &s)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetProfileArgs {
    connection: String,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    app_ver: Option<String>,
    #[serde(default)]
    ofx_version: Option<String>,
    /// Move the connection to the installed profile's new address (the user confirmed
    /// they recognise it as their bank's). Refused over MCP.
    #[serde(default)]
    accept_new_host: bool,
}

fn set_ofx_profile(ctx: &Ctx<'_>, a: SetProfileArgs) -> CommandResult<Value> {
    let (mut c, mut s) = ofx_conn(ctx, &a.connection)?;
    let mut inst = s.institution.clone();
    if a.accept_new_host {
        if ctx.actor.0.starts_with("mcp:") {
            return Err(rejected(
                "an OFX connection's address can only be changed in the app (the user must confirm the new host)",
            ));
        }
        let name = s
            .definition
            .clone()
            .ok_or_else(|| rejected("this connection did not come from an installed profile"))?;
        let i = installed(ctx, &name)?
            .ok_or_else(|| rejected(format!("the profile {name:?} is no longer installed")))?;
        inst.url = i.url;
        inst.org = i.org;
        inst.fid = i.fid;
    }
    if let Some(v) = a.app_id {
        inst.app_id = v;
    }
    if let Some(v) = a.app_ver {
        inst.app_ver = v;
    }
    if let Some(v) = a.ofx_version {
        inst.ofx_version = v;
    }
    inst.check().map_err(|e| invalid(e.to_string()))?;
    c.host = inst.host();
    s.institution = inst;
    c.ofx = Some(s.clone());
    crate::connect::save_conn(ctx, &c)?;
    profile_json(ctx, &c, &s)
}

fn client_uid_cmd(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let c = ctx
        .connector
        .ok_or_else(|| rejected("this front end cannot reach the computer's credential store"))?;
    let book = ctx.tx.info()?.id;
    Ok(json!({"client_uid": client_uid(&book, c),
        "note": "Some banks ask you to register this id before MoneyKeep can connect."}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ManualArgs {
    connection: String,
    /// `bank`, `card` or `investment`.
    kind: String,
    #[serde(default)]
    bank_id: Option<String>,
    #[serde(default)]
    broker_id: Option<String>,
    account_number: String,
    /// Bank accounts: `CHECKING`, `SAVINGS`, `MONEYMRKT`, `CREDITLINE`, `CD`.
    #[serde(default)]
    account_type: Option<String>,
}

fn add_manual_account(ctx: &Ctx<'_>, a: ManualArgs) -> CommandResult<Value> {
    let (c, s) = ofx_conn(ctx, &a.connection)?;
    let num = mk_core::text::clean(&a.account_number, 22)
        .text
        .trim()
        .to_owned();
    if num.is_empty() {
        return Err(invalid("give the account number"));
    }
    let kind = a.kind.trim().to_lowercase();
    let bank_id = a.bank_id.unwrap_or_default().trim().to_owned();
    let broker_id = a
        .broker_id
        .unwrap_or_else(|| s.institution.broker_id.clone())
        .trim()
        .to_owned();
    let acct_type = a
        .account_type
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    match kind.as_str() {
        "bank" => {
            if bank_id.is_empty() {
                return Err(invalid("a bank account needs the bank id (routing number)"));
            }
            if !matches!(
                acct_type.as_str(),
                "CHECKING" | "SAVINGS" | "MONEYMRKT" | "CREDITLINE" | "CD"
            ) {
                return Err(invalid(
                    "account_type is CHECKING, SAVINGS, MONEYMRKT, CREDITLINE or CD",
                ));
            }
        }
        "card" => {}
        "investment" => {
            if broker_id.is_empty() {
                return Err(invalid("an investment account needs the broker id"));
            }
        }
        _ => return Err(invalid("kind is bank, card or investment")),
    }
    let remote = ofx_dc::RemoteAccount {
        kind,
        bank_id,
        broker_id,
        acct_id: num,
        acct_type,
        description: String::new(),
        status: "ACTIVE".into(),
    };
    apply_accounts(ctx, &c, std::slice::from_ref(&remote))?;
    crate::connect::conn_json(ctx, &c)
}

/// Home items for OFX connections: passwords and questions waiting.
pub(crate) fn attention_items(c: &ConnDoc) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(s) = &c.ofx {
        if s.ask_password && c.status != "disabled" {
            out.push(json!({"kind": "password", "connection": c.id,
                "message": format!("{}: {NEEDS_PASSWORD}", c.label)}));
        }
        if !s.challenge.is_empty() {
            out.push(json!({"kind": "challenge", "connection": c.id,
                "message": format!("{}: Your bank asks extra questions", c.label)}));
        }
    }
    out
}

/// A remote account by its MoneyKeep account (holdings check).
pub(crate) fn link_for_account(
    ctx: &Ctx<'_>,
    account: AccountId,
) -> CommandResult<Option<(ConnDoc, LinkDoc)>> {
    for c in crate::connect::connections(ctx)? {
        for l in crate::connect::links_of(ctx, &c.id)? {
            if l.account.as_deref() == Some(account.to_string().as_str()) {
                return Ok(Some((c, l)));
            }
        }
    }
    Ok(None)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AccountArg {
    /// A MoneyKeep account (id, full name or code).
    account: String,
}

fn holdings_check(ctx: &Ctx<'_>, a: AccountArg) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let acc = chart.resolve(&a.account)?;
    match link_for_account(ctx, acc.id)? {
        Some((c, l)) => Ok(l.remote.and_then(|r| r.holdings).unwrap_or_else(|| {
            json!({"account": acc.id, "account_name": chart.full_name(acc.id),
                "connection": c.id, "message": "No holdings check yet; sync the connection"})
        })),
        None => Err(rejected(format!(
            "{} is not linked to a brokerage or OFX connection",
            chart.full_name(acc.id)
        ))),
    }
}

// ----- a scripted OFX server for tests and reviews ---------------------------------------

/// A remote account of the scripted server.
#[derive(Debug, Clone)]
pub struct FakeAccount {
    /// `bank`, `card`, `investment`.
    pub kind: String,
    /// Bank id or broker id.
    pub id: String,
    pub acct: String,
    pub acct_type: String,
    pub status: String,
    pub desc: String,
}

/// A scripted OFX server answering through the fetcher interface (tests and screen
/// reviews; no network). Replies come in the request's own dialect (SGML for 1.x,
/// XML for 2.x), shaped after the OFX 2.2 examples.
#[derive(Debug, Clone, Default)]
pub struct FakeOfx {
    pub state: std::sync::Arc<std::sync::Mutex<FakeOfxState>>,
}

#[derive(Debug, Default)]
pub struct FakeOfxState {
    pub org: String,
    pub fid: String,
    /// The profile's `USERCRED1LABEL`.
    pub cred1_label: Option<String>,
    pub accounts: Vec<FakeAccount>,
    /// Bank and card transactions by account: (FITID, `YYYYMMDD`, amount, name).
    pub txns: BTreeMap<String, Vec<(String, String, String, String)>>,
    /// Investment account: cash (`AVAILCASH`) and the units held of VTI.
    pub inv_cash: String,
    pub inv_units: String,
    /// The user id and password accepted (others get 15500).
    pub user: (String, String),
    /// Extra credential required when `cred1_label` is set.
    pub cred1: String,
    /// Questions (id, label) asked until these answers come (id, answer).
    pub challenge: Vec<(String, String)>,
    pub answers: Vec<(String, String)>,
    /// Signon code forced on every request.
    pub force_code: Option<String>,
    /// A second statement wrapper in one request gets 2000.
    pub reject_multi: bool,
    /// Answer with a web page.
    pub html: bool,
    pub http_status: Option<u16>,
    pub access_key: Option<String>,
    /// Request bodies seen.
    pub requests: Vec<String>,
}

impl FakeOfx {
    pub fn with<T>(&self, f: impl FnOnce(&mut FakeOfxState) -> T) -> Option<T> {
        self.state.lock().ok().map(|mut s| f(&mut s))
    }

    pub fn requests(&self) -> Vec<String> {
        self.state
            .lock()
            .map(|s| s.requests.clone())
            .unwrap_or_default()
    }

    /// A server with a checking account, a card and an investment account (pending),
    /// three checking and two card transactions.
    pub fn standard() -> Self {
        let f = FakeOfx::default();
        f.with(|s| {
            s.org = "TESTBANK".into();
            s.fid = "1234".into();
            s.user = ("user1".into(), "pass1".into());
            s.cred1 = "4321".into();
            s.accounts = vec![
                FakeAccount {
                    kind: "bank".into(),
                    id: "123456789".into(),
                    acct: "00001234".into(),
                    acct_type: "CHECKING".into(),
                    status: "ACTIVE".into(),
                    desc: "Everyday Checking".into(),
                },
                FakeAccount {
                    kind: "card".into(),
                    id: String::new(),
                    acct: "4111000011115678".into(),
                    acct_type: String::new(),
                    status: "ACTIVE".into(),
                    desc: "Rewards Card".into(),
                },
                FakeAccount {
                    kind: "investment".into(),
                    id: "broker.example".into(),
                    acct: "INV9876".into(),
                    acct_type: String::new(),
                    status: "PEND".into(),
                    desc: "Brokerage".into(),
                },
            ];
            s.txns.insert(
                "00001234".into(),
                vec![
                    (
                        "C1".into(),
                        "20261001".into(),
                        "-42.50".into(),
                        "GROCER".into(),
                    ),
                    (
                        "C2".into(),
                        "20261002".into(),
                        "1500.00".into(),
                        "PAYROLL".into(),
                    ),
                    (
                        "C3".into(),
                        "20261003".into(),
                        "-9.99".into(),
                        "STREAMING".into(),
                    ),
                ],
            );
            s.txns.insert(
                "4111000011115678".into(),
                vec![
                    (
                        "K1".into(),
                        "20261001".into(),
                        "-25.00".into(),
                        "CAFE".into(),
                    ),
                    (
                        "K2".into(),
                        "20261002".into(),
                        "-60.00".into(),
                        "FUEL".into(),
                    ),
                ],
            );
            s.inv_cash = "500.00".into();
            s.inv_units = "10".into();
        });
        f
    }
}

/// Values of `<TAG>` in a request (SGML or XML).
fn values(text: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(&open) {
        let after = rest.get(i + open.len()..).unwrap_or_default();
        let end = after.find(['<', '\n', '\r']).unwrap_or(after.len());
        out.push(after.get(..end).unwrap_or_default().trim().to_owned());
        rest = after;
    }
    out
}

/// Segments of a request starting at each `<TAG>`.
fn segments<'a>(text: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let idx: Vec<usize> = text.match_indices(&open).map(|(i, _)| i).collect();
    idx.iter()
        .enumerate()
        .map(|(n, &i)| {
            let end = idx.get(n + 1).copied().unwrap_or(text.len());
            text.get(i..end).unwrap_or_default()
        })
        .collect()
}

struct W {
    xml: bool,
    out: String,
}

impl W {
    fn open(&mut self, t: &str) {
        self.out.push_str(&format!("<{t}>\n"));
    }
    fn close(&mut self, t: &str) {
        self.out.push_str(&format!("</{t}>\n"));
    }
    fn leaf(&mut self, t: &str, v: &str) {
        if self.xml {
            self.out.push_str(&format!("<{t}>{v}</{t}>\n"));
        } else {
            self.out.push_str(&format!("<{t}>{v}\n"));
        }
    }
    fn status(&mut self, code: &str, msg: &str) {
        self.open("STATUS");
        self.leaf("CODE", code);
        self.leaf("SEVERITY", if code == "0" { "INFO" } else { "ERROR" });
        if !msg.is_empty() {
            self.leaf("MESSAGE", msg);
        }
        self.close("STATUS");
    }
}

fn reply(s: &FakeOfxState, req: &str) -> String {
    let xml = !req.starts_with("OFXHEADER:100");
    let version = values(req, "VERSION").first().cloned().unwrap_or_default();
    let mut w = W {
        xml,
        out: String::new(),
    };
    let user = values(req, "USERID").first().cloned().unwrap_or_default();
    let pass = values(req, "USERPASS").first().cloned().unwrap_or_default();
    let anonymous = user == ofx_dc::ANONYMOUS;
    let given: Vec<(String, String)> = values(req, "MFAPHRASEID")
        .into_iter()
        .zip(values(req, "MFAPHRASEA"))
        .collect();
    let wants_challenge = req.contains("<MFACHALLENGERQ>");
    let code = if let Some(c) = &s.force_code {
        c.clone()
    } else if anonymous {
        "0".into()
    } else if (user, pass) != s.user.clone()
        || (s.cred1_label.is_some()
            && values(req, "USERCRED1").first().map(String::as_str) != Some(s.cred1.as_str()))
    {
        "15500".into()
    } else if !s.challenge.is_empty() && !wants_challenge {
        let ok = s.answers.iter().all(|a| given.contains(a));
        if ok {
            "0".into()
        } else if given.is_empty() {
            "3000".into()
        } else {
            "3001".into()
        }
    } else {
        "0".into()
    };
    w.open("OFX");
    w.open("SIGNONMSGSRSV1");
    w.open("SONRS");
    w.status(&code, if code == "0" { "" } else { "Signon problem" });
    w.leaf("DTSERVER", "20261004120000");
    w.leaf("LANGUAGE", "ENG");
    w.open("FI");
    w.leaf("ORG", &s.org);
    w.leaf("FID", &s.fid);
    w.close("FI");
    if let Some(k) = s.access_key.as_ref().filter(|_| code == "0" && !anonymous) {
        w.leaf("ACCESSKEY", k);
    }
    w.close("SONRS");
    if wants_challenge {
        w.open("MFACHALLENGETRNRS");
        w.leaf(
            "TRNUID",
            values(req, "TRNUID").first().map_or("1", String::as_str),
        );
        w.status("0", "");
        w.open("MFACHALLENGERS");
        for (id, label) in &s.challenge {
            w.open("MFACHALLENGE");
            w.leaf("MFAPHRASEID", id);
            if !label.is_empty() {
                w.leaf("MFAPHRASELABEL", label);
            }
            w.close("MFACHALLENGE");
        }
        w.close("MFACHALLENGERS");
        w.close("MFACHALLENGETRNRS");
    }
    w.close("SIGNONMSGSRSV1");
    if code == "0" && !wants_challenge {
        if req.contains("<ACCTINFORQ>") {
            w.open("SIGNUPMSGSRSV1");
            w.open("ACCTINFOTRNRS");
            w.leaf("TRNUID", "A1");
            w.status("0", "");
            w.open("ACCTINFORS");
            w.leaf("DTACCTUP", "20261004");
            for a in &s.accounts {
                w.open("ACCTINFO");
                w.leaf("DESC", &a.desc);
                match a.kind.as_str() {
                    "bank" => {
                        w.open("BANKACCTINFO");
                        w.open("BANKACCTFROM");
                        w.leaf("BANKID", &a.id);
                        w.leaf("ACCTID", &a.acct);
                        w.leaf("ACCTTYPE", &a.acct_type);
                        w.close("BANKACCTFROM");
                        w.leaf("SUPTXDL", "Y");
                        w.leaf("XFERSRC", "N");
                        w.leaf("XFERDEST", "N");
                        w.leaf("SVCSTATUS", &a.status);
                        w.close("BANKACCTINFO");
                    }
                    "card" => {
                        w.open("CCACCTINFO");
                        w.open("CCACCTFROM");
                        w.leaf("ACCTID", &a.acct);
                        w.close("CCACCTFROM");
                        w.leaf("SUPTXDL", "Y");
                        w.leaf("XFERSRC", "N");
                        w.leaf("XFERDEST", "N");
                        w.leaf("SVCSTATUS", &a.status);
                        w.close("CCACCTINFO");
                    }
                    _ => {
                        w.open("INVACCTINFO");
                        w.open("INVACCTFROM");
                        w.leaf("BROKERID", &a.id);
                        w.leaf("ACCTID", &a.acct);
                        w.close("INVACCTFROM");
                        w.leaf("USPRODUCTTYPE", "NORMAL");
                        w.leaf("CHECKING", "N");
                        w.leaf("SVCSTATUS", &a.status);
                        w.leaf("INVACCTTYPE", "INDIVIDUAL");
                        w.close("INVACCTINFO");
                    }
                }
                w.close("ACCTINFO");
            }
            w.close("ACCTINFORS");
            w.close("ACCTINFOTRNRS");
            w.close("SIGNUPMSGSRSV1");
        }
        let mut n_stmt = 0;
        let bank = segments(req, "STMTTRNRQ");
        let card = segments(req, "CCSTMTTRNRQ");
        let inv = segments(req, "INVSTMTTRNRQ");
        let mut stmt = |w: &mut W, seg: &str, kind: &str| {
            n_stmt += 1;
            let trnuid = values(seg, "TRNUID").first().cloned().unwrap_or_default();
            let acct = values(seg, "ACCTID").first().cloned().unwrap_or_default();
            let (wrap, rs) = match kind {
                "bank" => ("STMTTRNRS", "STMTRS"),
                "card" => ("CCSTMTTRNRS", "CCSTMTRS"),
                _ => ("INVSTMTTRNRS", "INVSTMTRS"),
            };
            w.open(wrap);
            w.leaf("TRNUID", &trnuid);
            if s.reject_multi && n_stmt > 1 {
                w.status("2000", "One account per request");
                w.close(wrap);
                return;
            }
            w.status("0", "");
            w.open(rs);
            if kind == "investment" {
                w.leaf("DTASOF", "20261004");
                w.leaf("CURDEF", "USD");
                w.open("INVACCTFROM");
                w.leaf(
                    "BROKERID",
                    values(seg, "BROKERID").first().map_or("", String::as_str),
                );
                w.leaf("ACCTID", &acct);
                w.close("INVACCTFROM");
                w.open("INVTRANLIST");
                w.leaf("DTSTART", "20260901");
                w.leaf("DTEND", "20261004");
                w.open("BUYSTOCK");
                w.open("INVBUY");
                w.open("INVTRAN");
                w.leaf("FITID", "B1");
                w.leaf("DTTRADE", "20261001");
                w.close("INVTRAN");
                w.open("SECID");
                w.leaf("UNIQUEID", "922908769");
                w.leaf("UNIQUEIDTYPE", "CUSIP");
                w.close("SECID");
                w.leaf("UNITS", "10");
                w.leaf("UNITPRICE", "220.00");
                w.leaf("COMMISSION", "0");
                w.leaf("TOTAL", "-2200.00");
                w.leaf("SUBACCTSEC", "CASH");
                w.leaf("SUBACCTFUND", "CASH");
                w.close("INVBUY");
                w.leaf("BUYTYPE", "BUY");
                w.close("BUYSTOCK");
                w.open("INCOME");
                w.open("INVTRAN");
                w.leaf("FITID", "D1");
                w.leaf("DTTRADE", "20261002");
                w.close("INVTRAN");
                w.open("SECID");
                w.leaf("UNIQUEID", "922908769");
                w.leaf("UNIQUEIDTYPE", "CUSIP");
                w.close("SECID");
                w.leaf("INCOMETYPE", "DIV");
                w.leaf("TOTAL", "12.34");
                w.leaf("SUBACCTSEC", "CASH");
                w.leaf("SUBACCTFUND", "CASH");
                w.close("INCOME");
                w.close("INVTRANLIST");
                w.open("INVPOSLIST");
                w.open("POSSTOCK");
                w.open("INVPOS");
                w.open("SECID");
                w.leaf("UNIQUEID", "922908769");
                w.leaf("UNIQUEIDTYPE", "CUSIP");
                w.close("SECID");
                w.leaf("HELDINACCT", "CASH");
                w.leaf("POSTYPE", "LONG");
                w.leaf("UNITS", &s.inv_units);
                w.leaf("UNITPRICE", "231.17");
                w.leaf("MKTVAL", "2311.70");
                w.leaf("DTPRICEASOF", "20261003");
                w.close("INVPOS");
                w.close("POSSTOCK");
                w.close("INVPOSLIST");
                w.open("INVBAL");
                w.leaf("AVAILCASH", &s.inv_cash);
                w.leaf("MARGINBALANCE", "0");
                w.leaf("SHORTBALANCE", "0");
                w.close("INVBAL");
            } else {
                w.leaf("CURDEF", "USD");
                if kind == "bank" {
                    w.open("BANKACCTFROM");
                    w.leaf(
                        "BANKID",
                        values(seg, "BANKID").first().map_or("", String::as_str),
                    );
                    w.leaf("ACCTID", &acct);
                    w.leaf(
                        "ACCTTYPE",
                        values(seg, "ACCTTYPE")
                            .first()
                            .map_or("CHECKING", String::as_str),
                    );
                    w.close("BANKACCTFROM");
                } else {
                    w.open("CCACCTFROM");
                    w.leaf("ACCTID", &acct);
                    w.close("CCACCTFROM");
                }
                w.open("BANKTRANLIST");
                w.leaf("DTSTART", "20260901");
                w.leaf("DTEND", "20261004");
                for (fitid, date, amt, name) in s.txns.get(&acct).cloned().unwrap_or_default() {
                    w.open("STMTTRN");
                    w.leaf(
                        "TRNTYPE",
                        if amt.starts_with('-') {
                            "DEBIT"
                        } else {
                            "CREDIT"
                        },
                    );
                    w.leaf("DTPOSTED", &date);
                    w.leaf("TRNAMT", &amt);
                    w.leaf("FITID", &fitid);
                    w.leaf("NAME", &name);
                    w.close("STMTTRN");
                }
                w.close("BANKTRANLIST");
                w.open("LEDGERBAL");
                w.leaf("BALAMT", "1000.00");
                w.leaf("DTASOF", "20261004");
                w.close("LEDGERBAL");
            }
            w.close(rs);
            w.close(wrap);
        };
        if !bank.is_empty() {
            w.open("BANKMSGSRSV1");
            for seg in &bank {
                stmt(&mut w, seg, "bank");
            }
            w.close("BANKMSGSRSV1");
        }
        if !card.is_empty() {
            w.open("CREDITCARDMSGSRSV1");
            for seg in &card {
                stmt(&mut w, seg, "card");
            }
            w.close("CREDITCARDMSGSRSV1");
        }
        if !inv.is_empty() {
            w.open("INVSTMTMSGSRSV1");
            for seg in &inv {
                stmt(&mut w, seg, "investment");
            }
            w.close("INVSTMTMSGSRSV1");
            w.open("SECLISTMSGSRSV1");
            w.open("SECLIST");
            w.open("STOCKINFO");
            w.open("SECINFO");
            w.open("SECID");
            w.leaf("UNIQUEID", "922908769");
            w.leaf("UNIQUEIDTYPE", "CUSIP");
            w.close("SECID");
            w.leaf("SECNAME", "Vanguard Total Stock Market ETF");
            w.leaf("TICKER", "VTI");
            w.close("SECINFO");
            w.close("STOCKINFO");
            w.close("SECLIST");
            w.close("SECLISTMSGSRSV1");
        }
        if req.contains("<PROFRQ>") {
            w.open("PROFMSGSRSV1");
            w.open("PROFTRNRS");
            w.leaf("TRNUID", "P1");
            w.status("0", "");
            w.open("PROFRS");
            w.open("SIGNONINFOLIST");
            w.open("SIGNONINFO");
            w.leaf("SIGNONREALM", "DEFAULT");
            w.leaf("MIN", "4");
            w.leaf("MAX", "32");
            w.leaf("CHARTYPE", "ALPHAORNUMERIC");
            w.leaf("CASESEN", "Y");
            w.leaf("SPECIAL", "Y");
            w.leaf("SPACES", "N");
            w.leaf("PINCH", "N");
            w.leaf("CHGPINFIRST", "N");
            if let Some(l) = &s.cred1_label {
                w.leaf("USERCRED1LABEL", l);
            }
            w.leaf("CLIENTUIDREQ", "Y");
            w.close("SIGNONINFO");
            w.close("SIGNONINFOLIST");
            w.leaf("DTPROFUP", "20261001");
            w.leaf("FINAME", "Test Bank");
            w.close("PROFRS");
            w.close("PROFTRNRS");
            w.close("PROFMSGSRSV1");
        }
    }
    w.close("OFX");
    let header = if xml {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<?OFX OFXHEADER=\"200\" VERSION=\"{version}\" SECURITY=\"NONE\" OLDFILEUID=\"NONE\" NEWFILEUID=\"NONE\"?>\n"
        )
    } else {
        format!(
            "OFXHEADER:100\r\nDATA:OFXSGML\r\nVERSION:{version}\r\nSECURITY:NONE\r\nENCODING:USASCII\r\nCHARSET:1252\r\nCOMPRESSION:NONE\r\nOLDFILEUID:NONE\r\nNEWFILEUID:NONE\r\n\r\n"
        )
    };
    header + &w.out
}

impl crate::connect::Fetcher for FakeOfx {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, FetchError> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| FetchError::Network("fake server unavailable".into()))?;
        let body = req
            .body
            .as_ref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        s.requests.push(body.clone());
        if !req.url.starts_with("https://") || req.authority.is_empty() {
            return Err(FetchError::Refused("not https".into()));
        }
        if let Some(code) = s.http_status {
            return Ok(HttpResponse {
                status: code,
                body: Vec::new(),
                retry_after: None,
                deprecation: None,
            });
        }
        if s.html {
            return Ok(HttpResponse {
                status: 200,
                body: b"<!DOCTYPE html><html><body>We moved</body></html>".to_vec(),
                retry_after: None,
                deprecation: None,
            });
        }
        let has = |h: &str, v: &str| req.headers.iter().any(|x| x.0 == h && x.1 == v);
        if req.method != HttpMethod::Post || !has("Content-Type", "application/x-ofx") {
            return Ok(HttpResponse {
                status: 400,
                body: Vec::new(),
                retry_after: None,
                deprecation: None,
            });
        }
        Ok(HttpResponse {
            status: 200,
            body: reply(&s, &body).into_bytes(),
            retry_after: None,
            deprecation: None,
        })
    }
}

fn institutions_cmd(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    Ok(Value::Array(
        institutions(ctx)?
            .iter()
            .map(|i| {
                json!({"name": i.name, "url": i.url, "host": i.host(), "org": i.org, "fid": i.fid,
                    "broker_id": i.broker_id, "ofx_version": i.ofx_version, "app_id": i.app_id,
                    "app_ver": i.app_ver, "notes": i.notes})
            })
            .collect(),
    ))
}
