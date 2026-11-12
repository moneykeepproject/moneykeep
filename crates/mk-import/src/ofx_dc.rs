//! OFX Direct Connect (connectivity-brokerage.md part A): institution profiles, request
//! writers for OFX 1.x (SGML) and 2.x (XML), and the parts of a server's reply that
//! steer a connection (signon status, multi-factor challenges, the FI profile, account
//! information, per-statement status). Statements themselves are read by the OFX file
//! parser. Pure: the caller sends the requests.
//!
//! Element names and order follow the OFX 2.2 specification: signon `SONRQ` and status
//! codes §2.5.1 (CLIENTUID, USERCRED1/2, AUTHTOKEN, ACCESSKEY, SESSCOOKIE; codes
//! §2.5.1.7), anonymous profile signon §2.5.1.3.1, multi-factor challenge §2.5.4, FI
//! profile §7 (`PROFRQ`, `SIGNONINFO`), account information §8.5 (`ACCTINFORQ`,
//! `SVCSTATUS`), statements §11.4 (`STMTRQ`, `CCSTMTRQ`) and §13.9.1 (`INVSTMTRQ`),
//! headers §2.2.

use serde::{Deserialize, Serialize};

use crate::ofx::{Node, bank_key, inv_key, parse_tree};
use crate::{ImportError, ImportResult};

/// OFX versions a profile may name.
pub const VERSIONS: [&str; 8] = ["102", "103", "151", "160", "200", "203", "211", "220"];
/// Versions that carry the multi-factor signon elements (OFX 2.2 §1.4).
pub const MFA_VERSIONS: [&str; 4] = ["103", "203", "211", "220"];
/// The anonymous user id and password of a first profile request (§2.5.1.3.1).
pub const ANONYMOUS: &str = "anonymous00000000000000000000000";
/// Largest answer accepted from an OFX server.
pub const MAX_RESPONSE_BYTES: usize = 25 * 1024 * 1024;

fn invalid(m: impl Into<String>) -> ImportError {
    ImportError::Invalid(m.into())
}

fn clean(s: &str, max: usize) -> String {
    mk_core::text::clean(s, max).text.trim().to_owned()
}

/// Whether a version sends `CLIENTUID`, `USERCRED1/2`, `AUTHTOKEN` and the challenge
/// elements.
pub fn mfa_version(v: &str) -> bool {
    MFA_VERSIONS.contains(&v)
}

fn default_version() -> String {
    String::new()
}
fn default_app_id() -> String {
    "MCRFT".into()
}
fn default_app_ver() -> String {
    "0100".into()
}

/// An institution's OFX server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Institution {
    pub name: String,
    /// `https://…` only.
    pub url: String,
    #[serde(default)]
    pub org: String,
    #[serde(default)]
    pub fid: String,
    /// Investment accounts.
    #[serde(default)]
    pub broker_id: String,
    /// Empty: `bank-feeds.ofx_default_version` (or 103).
    #[serde(default = "default_version")]
    pub ofx_version: String,
    /// At most 5 characters.
    #[serde(default = "default_app_id")]
    pub app_id: String,
    /// 4 digits.
    #[serde(default = "default_app_ver")]
    pub app_ver: String,
    #[serde(default)]
    pub notes: String,
}

impl Institution {
    /// Clean and check the fields (text rules of input-safety.md R3).
    pub fn check(&mut self) -> ImportResult<()> {
        self.name = clean(&self.name, 100);
        self.url = self.url.trim().to_owned();
        self.org = clean(&self.org, 32);
        self.fid = clean(&self.fid, 32);
        self.broker_id = clean(&self.broker_id, 22);
        self.ofx_version = self.ofx_version.trim().to_owned();
        if self.ofx_version.is_empty() {
            self.ofx_version = "103".into();
        }
        self.app_id = clean(&self.app_id, 5);
        self.app_ver = self.app_ver.trim().to_owned();
        self.notes = clean(&self.notes, 500);
        if self.name.is_empty() {
            return Err(invalid("give the institution's name"));
        }
        host_of(&self.url)?;
        if !VERSIONS.contains(&self.ofx_version.as_str()) {
            return Err(invalid(format!(
                "OFX version {:?} is not one of {}",
                self.ofx_version,
                VERSIONS.join(", ")
            )));
        }
        if self.app_id.is_empty() || self.app_id.chars().count() > 5 {
            return Err(invalid("the application id has 1 to 5 characters"));
        }
        if self.app_ver.len() != 4 || !self.app_ver.chars().all(|c| c.is_ascii_digit()) {
            return Err(invalid("the application version is 4 digits (e.g. 0100)"));
        }
        Ok(())
    }

    /// The server's host (where the password goes).
    pub fn host(&self) -> String {
        host_of(&self.url).unwrap_or_default()
    }

    /// `host[:port]` requests are restricted to.
    pub fn authority(&self) -> String {
        authority_of(&self.url).unwrap_or_default()
    }
}

fn authority_of(url: &str) -> ImportResult<String> {
    let rest = url
        .trim()
        .strip_prefix("https://")
        .ok_or_else(|| invalid("the OFX address must start with https://"))?;
    let auth = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if auth.is_empty() || auth.contains('@') || auth.contains(char::is_whitespace) {
        return Err(invalid("the OFX address has no valid host"));
    }
    Ok(auth.to_ascii_lowercase())
}

/// The host of an `https://` address.
pub fn host_of(url: &str) -> ImportResult<String> {
    let auth = authority_of(url)?;
    let host = match auth.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !h.ends_with(']') => h,
        _ => auth.as_str(),
    };
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '[' | ']' | ':'))
    {
        return Err(invalid("the OFX address has no valid host"));
    }
    Ok(host.to_owned())
}

/// The signon of a request (never printed).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SignOn {
    pub user_id: String,
    pub password: String,
    pub cred1: Option<String>,
    pub cred2: Option<String>,
    pub client_uid: Option<String>,
    pub auth_token: Option<String>,
    pub access_key: Option<String>,
    pub sess_cookie: Option<String>,
    /// Used instead of the user id and password while it is valid.
    pub user_key: Option<String>,
    /// (`MFAPHRASEID`, answer) pairs.
    pub answers: Vec<(String, String)>,
}

impl std::fmt::Debug for SignOn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SignOn(hidden)")
    }
}

impl SignOn {
    /// The anonymous signon of a first profile request.
    pub fn anonymous() -> Self {
        SignOn {
            user_id: ANONYMOUS.into(),
            password: ANONYMOUS.into(),
            ..SignOn::default()
        }
    }
}

/// One transaction of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Txn {
    Profile,
    AccountInfo,
    Challenge,
    Bank {
        bank_id: String,
        acct_id: String,
        acct_type: String,
        /// `YYYYMMDD`.
        start: String,
    },
    Card {
        acct_id: String,
        start: String,
    },
    Invest {
        broker_id: String,
        acct_id: String,
        start: String,
    },
}

enum El {
    Leaf(&'static str, String),
    Agg(&'static str, Vec<El>),
}

fn leaf(n: &'static str, v: impl Into<String>) -> El {
    El::Leaf(n, v.into())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn render(e: &El, xml: bool, depth: usize, out: &mut String) {
    let pad = " ".repeat(depth);
    match e {
        El::Leaf(n, v) => {
            if xml {
                out.push_str(&format!("{pad}<{n}>{}</{n}>\n", escape(v)));
            } else {
                out.push_str(&format!("{pad}<{n}>{}\n", escape(v)));
            }
        }
        El::Agg(n, kids) => {
            out.push_str(&format!("{pad}<{n}>\n"));
            for k in kids {
                render(k, xml, depth + 1, out);
            }
            out.push_str(&format!("{pad}</{n}>\n"));
        }
    }
}

/// `YYYYMMDDHHMMSS` (UTC) of epoch seconds.
pub fn datetime(t: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(t)
        .ok()
        .and_then(|d| {
            d.format(time::macros::format_description!(
                "[year][month][day][hour][minute][second]"
            ))
            .ok()
        })
        .unwrap_or_else(|| "19700101000000".into())
}

/// `YYYYMMDD` of a date.
pub fn ofx_date(d: mk_core::Date) -> String {
    d.to_string().replace('-', "")
}

fn signon(inst: &Institution, s: &SignOn, now: i64) -> El {
    let mfa = mfa_version(&inst.ofx_version);
    let mut k = vec![leaf("DTCLIENT", datetime(now))];
    match &s.user_key {
        Some(key) if !key.is_empty() => k.push(leaf("USERKEY", key.clone())),
        _ => {
            k.push(leaf("USERID", s.user_id.clone()));
            k.push(leaf("USERPASS", s.password.clone()));
        }
    }
    k.push(leaf("LANGUAGE", "ENG"));
    if !inst.org.is_empty() || !inst.fid.is_empty() {
        let mut fi = Vec::new();
        if !inst.org.is_empty() {
            fi.push(leaf("ORG", inst.org.clone()));
        }
        if !inst.fid.is_empty() {
            fi.push(leaf("FID", inst.fid.clone()));
        }
        k.push(El::Agg("FI", fi));
    }
    if let Some(c) = s.sess_cookie.as_ref().filter(|c| !c.is_empty()) {
        k.push(leaf("SESSCOOKIE", c.clone()));
    }
    k.push(leaf("APPID", inst.app_id.clone()));
    k.push(leaf("APPVER", inst.app_ver.clone()));
    if mfa {
        if let Some(u) = s.client_uid.as_ref().filter(|u| !u.is_empty()) {
            k.push(leaf("CLIENTUID", u.clone()));
        }
        for (n, v) in [
            ("USERCRED1", &s.cred1),
            ("USERCRED2", &s.cred2),
            ("AUTHTOKEN", &s.auth_token),
            ("ACCESSKEY", &s.access_key),
        ] {
            if let Some(v) = v.as_ref().filter(|v| !v.is_empty()) {
                k.push(leaf(n, v.clone()));
            }
        }
        for (id, a) in &s.answers {
            k.push(El::Agg(
                "MFACHALLENGEANSWER",
                vec![
                    leaf("MFAPHRASEID", id.clone()),
                    leaf("MFAPHRASEA", a.clone()),
                ],
            ));
        }
    }
    El::Agg("SONRQ", k)
}

fn inctran(start: &str) -> El {
    El::Agg(
        "INCTRAN",
        vec![leaf("DTSTART", start), leaf("INCLUDE", "Y")],
    )
}

/// An OFX request in the profile's version: the 1.x SGML header and SGML body for
/// `1xx`, the 2.x XML header and XML body for `2xx`. Each transaction gets a fresh
/// `TRNUID` from `uid`.
pub fn request(
    inst: &Institution,
    s: &SignOn,
    txns: &[Txn],
    now: i64,
    uid: &mut dyn FnMut() -> String,
) -> String {
    let xml = inst.ofx_version.starts_with('2');
    let mut signon_set = vec![signon(inst, s, now)];
    let mut signup = Vec::new();
    let mut bank = Vec::new();
    let mut card = Vec::new();
    let mut inv = Vec::new();
    let mut prof = Vec::new();
    for t in txns {
        match t {
            Txn::Challenge => signon_set.push(El::Agg(
                "MFACHALLENGETRNRQ",
                vec![
                    leaf("TRNUID", uid()),
                    El::Agg("MFACHALLENGERQ", vec![leaf("DTCLIENT", datetime(now))]),
                ],
            )),
            Txn::Profile => prof.push(El::Agg(
                "PROFTRNRQ",
                vec![
                    leaf("TRNUID", uid()),
                    El::Agg(
                        "PROFRQ",
                        vec![leaf("CLIENTROUTING", "NONE"), leaf("DTPROFUP", "19900101")],
                    ),
                ],
            )),
            Txn::AccountInfo => signup.push(El::Agg(
                "ACCTINFOTRNRQ",
                vec![
                    leaf("TRNUID", uid()),
                    El::Agg("ACCTINFORQ", vec![leaf("DTACCTUP", "19900101")]),
                ],
            )),
            Txn::Bank {
                bank_id,
                acct_id,
                acct_type,
                start,
            } => bank.push(El::Agg(
                "STMTTRNRQ",
                vec![
                    leaf("TRNUID", uid()),
                    El::Agg(
                        "STMTRQ",
                        vec![
                            El::Agg(
                                "BANKACCTFROM",
                                vec![
                                    leaf("BANKID", bank_id.clone()),
                                    leaf("ACCTID", acct_id.clone()),
                                    leaf("ACCTTYPE", acct_type.clone()),
                                ],
                            ),
                            inctran(start),
                        ],
                    ),
                ],
            )),
            Txn::Card { acct_id, start } => card.push(El::Agg(
                "CCSTMTTRNRQ",
                vec![
                    leaf("TRNUID", uid()),
                    El::Agg(
                        "CCSTMTRQ",
                        vec![
                            El::Agg("CCACCTFROM", vec![leaf("ACCTID", acct_id.clone())]),
                            inctran(start),
                        ],
                    ),
                ],
            )),
            Txn::Invest {
                broker_id,
                acct_id,
                start,
            } => inv.push(El::Agg(
                "INVSTMTTRNRQ",
                vec![
                    leaf("TRNUID", uid()),
                    El::Agg(
                        "INVSTMTRQ",
                        vec![
                            El::Agg(
                                "INVACCTFROM",
                                vec![
                                    leaf("BROKERID", broker_id.clone()),
                                    leaf("ACCTID", acct_id.clone()),
                                ],
                            ),
                            inctran(start),
                            leaf("INCOO", "N"),
                            El::Agg("INCPOS", vec![leaf("INCLUDE", "Y")]),
                            leaf("INCBAL", "Y"),
                        ],
                    ),
                ],
            )),
        }
    }
    let mut sets = vec![El::Agg("SIGNONMSGSRQV1", signon_set)];
    for (name, kids) in [
        ("SIGNUPMSGSRQV1", signup),
        ("BANKMSGSRQV1", bank),
        ("CREDITCARDMSGSRQV1", card),
        ("INVSTMTMSGSRQV1", inv),
        ("PROFMSGSRQV1", prof),
    ] {
        if !kids.is_empty() {
            sets.push(El::Agg(name, kids));
        }
    }
    let mut body = String::new();
    render(&El::Agg("OFX", sets), xml, 0, &mut body);
    let header = if xml {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<?OFX OFXHEADER=\"200\" VERSION=\"{}\" SECURITY=\"NONE\" OLDFILEUID=\"NONE\" NEWFILEUID=\"NONE\"?>\n",
            inst.ofx_version
        )
    } else {
        format!(
            "OFXHEADER:100\r\nDATA:OFXSGML\r\nVERSION:{}\r\nSECURITY:NONE\r\nENCODING:USASCII\r\nCHARSET:1252\r\nCOMPRESSION:NONE\r\nOLDFILEUID:NONE\r\nNEWFILEUID:NONE\r\n\r\n",
            inst.ofx_version
        )
    };
    header + &body
}

// ----- replies -----------------------------------------------------------------------

/// A `STATUS` aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Status {
    pub code: String,
    pub severity: String,
    /// Cleaned server text.
    pub message: String,
}

impl Status {
    pub fn ok(&self) -> bool {
        self.code == "0"
    }
}

/// A multi-factor challenge question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Challenge {
    pub id: String,
    /// The server's text, or the meaning of a reserved id.
    pub label: String,
    /// The client answers it itself (ids above MFA100 that MoneyKeep knows).
    pub automatic: bool,
}

/// The FI profile's signon information.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SignonInfo {
    pub cred1_label: Option<String>,
    pub cred2_label: Option<String>,
    pub client_uid_required: bool,
    pub challenge_first: bool,
    pub auth_token_label: Option<String>,
}

/// A remote account from account information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteAccount {
    /// `bank`, `card` or `investment`.
    pub kind: String,
    #[serde(default)]
    pub bank_id: String,
    #[serde(default)]
    pub broker_id: String,
    pub acct_id: String,
    /// Bank account type (`CHECKING`, `SAVINGS`, …).
    #[serde(default)]
    pub acct_type: String,
    #[serde(default)]
    pub description: String,
    /// `ACTIVE`, `PEND`, `AVAIL`.
    pub status: String,
}

/// A statement transaction of a reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatementInfo {
    /// `bank`, `cc` or `investment`.
    pub kind: String,
    pub status: Status,
    pub acct_id: String,
    /// The section key the file parser gives this statement.
    pub key: String,
    /// `DTEND` of the transaction list (`YYYYMMDD…`).
    pub dtend: Option<String>,
}

/// What a reply says beyond its statements.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Reply {
    pub signon: Status,
    pub access_key: Option<String>,
    pub sess_cookie: Option<String>,
    pub user_key: Option<String>,
    pub key_expire: Option<String>,
    pub challenge_status: Option<Status>,
    pub challenges: Vec<Challenge>,
    pub profile_status: Option<Status>,
    pub profile: Option<SignonInfo>,
    pub account_status: Option<Status>,
    pub accounts: Option<Vec<RemoteAccount>>,
    pub statements: Vec<StatementInfo>,
}

/// An answer that is a web page rather than OFX (a moved or retired server).
pub fn looks_like_html(body: &[u8]) -> bool {
    let head: String = String::from_utf8_lossy(body.get(..body.len().min(1024)).unwrap_or(&[]))
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase();
    !head.contains("ofxheader")
        && !head.contains("<ofx>")
        && (head.starts_with("<!doctype html")
            || head.starts_with("<html")
            || head.contains("<html"))
}

fn status(n: Option<&Node>) -> Option<Status> {
    let s = n?.child("STATUS")?;
    Some(Status {
        code: s.text_of("CODE").unwrap_or_default().trim().to_owned(),
        severity: s.text_of("SEVERITY").unwrap_or_default(),
        message: clean(&s.text_of("MESSAGE").unwrap_or_default(), 500),
    })
}

/// The meaning of a reserved `MFAPHRASEID` (§2.5.4.4.1).
pub fn phrase_meaning(id: &str) -> Option<&'static str> {
    Some(match id {
        "MFA1" => "City of birth",
        "MFA2" => "Date of birth (MM/DD/YYYY)",
        "MFA3" => "Debit card number",
        "MFA4" => "Father's middle name",
        "MFA5" => "Favorite color",
        "MFA6" => "First pet's name",
        "MFA7" => "Five digit ZIP code",
        "MFA8" => "Grandmother's maiden name on your father's side",
        "MFA9" => "Grandmother's maiden name on your mother's side",
        "MFA10" => "Last four digits of your cell phone number",
        "MFA11" => "Last four digits of your daytime phone number",
        "MFA12" => "Last four digits of your home phone number",
        "MFA13" => "Last four digits of your social security number",
        "MFA14" => "Last four digits of your tax ID",
        "MFA15" => "Month of birth of youngest sibling",
        "MFA16" => "Mother's maiden name",
        "MFA17" => "Mother's middle name",
        "MFA18" => "Name of the company where you had your first job",
        "MFA19" => "Name of the manufacturer of your first car",
        "MFA20" => "Name of the street you grew up on",
        "MFA21" => "Name of your high school football team",
        "MFA22" => "Recent deposit or recent withdrawal amount",
        "MFA23" => "Year of birth (YYYY)",
        "MFA101" => "Date and time",
        "MFA102" => "Host name",
        "MFA103" => "IP address",
        "MFA104" => "MAC address",
        "MFA105" => "Operating system version",
        "MFA106" => "Processor architecture",
        "MFA107" => "User agent",
        _ => return None,
    })
}

/// The answer MoneyKeep gives itself to a client-answered question, if any: only the
/// date and time and the user agent (it never reports host names, addresses or
/// hardware).
pub fn automatic_answer(id: &str, now: i64) -> Option<String> {
    match id {
        "MFA101" => Some(datetime(now)),
        "MFA107" => Some("MoneyKeep".into()),
        _ => None,
    }
}

/// Read a reply's signon, challenge, profile, account information and statement
/// statuses.
pub fn parse_reply(body: &[u8]) -> ImportResult<Reply> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(invalid("the answer is larger than 25 MB"));
    }
    let root = parse_tree(body)?;
    let ofx = root
        .child("OFX")
        .ok_or_else(|| invalid("not an OFX answer"))?;
    let mut r = Reply::default();
    let son_set = ofx.child("SIGNONMSGSRSV1");
    let sonrs = son_set.and_then(|s| s.child("SONRS"));
    r.signon = status(sonrs).ok_or_else(|| invalid("the answer has no signon status"))?;
    if let Some(s) = sonrs {
        let opt = |t: &str| {
            s.text_of(t)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        r.access_key = opt("ACCESSKEY");
        r.sess_cookie = opt("SESSCOOKIE");
        r.user_key = opt("USERKEY");
        r.key_expire = opt("TSKEYEXPIRE");
    }
    if let Some(t) = son_set.and_then(|s| s.child("MFACHALLENGETRNRS")) {
        r.challenge_status = status(Some(t));
        if let Some(rs) = t.child("MFACHALLENGERS") {
            for c in rs.all("MFACHALLENGE") {
                let id = clean(&c.text_of("MFAPHRASEID").unwrap_or_default(), 32);
                if id.is_empty() {
                    continue;
                }
                let label = clean(&c.text_of("MFAPHRASELABEL").unwrap_or_default(), 200);
                let label = if label.is_empty() {
                    phrase_meaning(&id).unwrap_or("Answer").to_owned()
                } else {
                    label
                };
                r.challenges.push(Challenge {
                    automatic: automatic_answer(&id, 0).is_some(),
                    id,
                    label,
                });
            }
        }
    }
    if let Some(t) = ofx.path(&["PROFMSGSRSV1", "PROFTRNRS"]) {
        r.profile_status = status(Some(t));
        if let Some(si) = t.path(&["PROFRS", "SIGNONINFOLIST", "SIGNONINFO"]) {
            let yes = |tag: &str| si.text_of(tag).is_some_and(|v| v.trim() == "Y");
            let label = |tag: &str| {
                si.text_of(tag)
                    .map(|v| clean(&v, 64))
                    .filter(|v| !v.is_empty())
            };
            r.profile = Some(SignonInfo {
                cred1_label: label("USERCRED1LABEL"),
                cred2_label: label("USERCRED2LABEL"),
                client_uid_required: yes("CLIENTUIDREQ"),
                challenge_first: yes("MFACHALLENGEFIRST"),
                auth_token_label: label("AUTHTOKENLABEL"),
            });
        }
    }
    if let Some(t) = ofx.path(&["SIGNUPMSGSRSV1", "ACCTINFOTRNRS"]) {
        r.account_status = status(Some(t));
        let mut list = Vec::new();
        if let Some(rs) = t.child("ACCTINFORS") {
            for a in rs.all("ACCTINFO") {
                let desc = clean(
                    &a.text_of("DESC")
                        .or_else(|| a.text_of("NAME"))
                        .unwrap_or_default(),
                    80,
                );
                for (tag, from, kind) in [
                    ("BANKACCTINFO", "BANKACCTFROM", "bank"),
                    ("CCACCTINFO", "CCACCTFROM", "card"),
                    ("INVACCTINFO", "INVACCTFROM", "investment"),
                ] {
                    let Some(info) = a.child(tag) else { continue };
                    let f = info.child(from);
                    let txt = |t: &str| {
                        f.and_then(|x| x.text_of(t))
                            .map(|v| clean(&v, 32))
                            .unwrap_or_default()
                    };
                    let acct_id = txt("ACCTID");
                    if acct_id.is_empty() {
                        continue;
                    }
                    list.push(RemoteAccount {
                        kind: kind.into(),
                        bank_id: txt("BANKID"),
                        broker_id: txt("BROKERID"),
                        acct_type: txt("ACCTTYPE"),
                        acct_id,
                        description: desc.clone(),
                        status: clean(&info.text_of("SVCSTATUS").unwrap_or_default(), 10)
                            .to_ascii_uppercase(),
                    });
                }
            }
        }
        r.accounts = Some(list);
    }
    let fi = sonrs.and_then(|s| s.child("FI"));
    let org = fi.and_then(|f| f.text_of("ORG")).unwrap_or_default();
    let fid = fi.and_then(|f| f.text_of("FID")).unwrap_or_default();
    for (set, wrapper, rs_tag, from_tag, kind) in [
        (
            "BANKMSGSRSV1",
            "STMTTRNRS",
            "STMTRS",
            "BANKACCTFROM",
            "bank",
        ),
        (
            "CREDITCARDMSGSRSV1",
            "CCSTMTTRNRS",
            "CCSTMTRS",
            "CCACCTFROM",
            "cc",
        ),
        (
            "INVSTMTMSGSRSV1",
            "INVSTMTTRNRS",
            "INVSTMTRS",
            "INVACCTFROM",
            "investment",
        ),
    ] {
        for m in ofx.all(set) {
            for t in m.all(wrapper) {
                let rs = t.child(rs_tag);
                let from = rs.and_then(|x| x.child(from_tag));
                let txt = |n: &str| from.and_then(|x| x.text_of(n)).unwrap_or_default();
                let acct_id = txt("ACCTID");
                let key = match kind {
                    "investment" => inv_key(&org, &txt("BROKERID"), &acct_id),
                    "bank" => {
                        bank_key(&org, &fid, &txt("BANKID"), &txt("BRANCHID"), &acct_id, kind)
                    }
                    _ => bank_key(&org, &fid, "", "", &acct_id, kind),
                };
                let list = rs.and_then(|x| {
                    x.child(if kind == "investment" {
                        "INVTRANLIST"
                    } else {
                        "BANKTRANLIST"
                    })
                });
                r.statements.push(StatementInfo {
                    kind: kind.into(),
                    status: status(Some(t)).unwrap_or_default(),
                    dtend: list.and_then(|l| l.text_of("DTEND")),
                    acct_id,
                    key,
                });
            }
        }
    }
    Ok(r)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn inst(v: &str) -> Institution {
        Institution {
            name: "Test Bank".into(),
            url: "https://ofx.testbank.example/ofx".into(),
            org: "TESTBANK".into(),
            fid: "1234".into(),
            broker_id: String::new(),
            ofx_version: v.into(),
            app_id: "MCRFT".into(),
            app_ver: "0100".into(),
            notes: String::new(),
        }
    }

    fn signon_full() -> SignOn {
        SignOn {
            user_id: "user1".into(),
            password: "p&ss".into(),
            cred1: Some("4321".into()),
            client_uid: Some("C1".into()),
            ..SignOn::default()
        }
    }

    #[test]
    fn sgml_and_xml_requests() {
        let mut n = 0;
        let mut uid = || {
            n += 1;
            format!("T{n}")
        };
        let txt = request(
            &inst("102"),
            &signon_full(),
            &[Txn::AccountInfo],
            0,
            &mut uid,
        );
        assert!(txt.starts_with("OFXHEADER:100\r\nDATA:OFXSGML\r\nVERSION:102"));
        assert!(txt.contains("<USERPASS>p&amp;ss\n"));
        assert!(!txt.contains("CLIENTUID") && !txt.contains("USERCRED1"));
        assert!(txt.contains("<ACCTINFOTRNRQ>") && txt.contains("<TRNUID>T1\n"));
        let txt = request(&inst("220"), &signon_full(), &[Txn::Profile], 0, &mut uid);
        assert!(txt.contains("<?OFX OFXHEADER=\"200\" VERSION=\"220\""));
        assert!(txt.contains("<CLIENTUID>C1</CLIENTUID>"));
        assert!(txt.contains("<USERCRED1>4321</USERCRED1>"));
        assert!(txt.contains("<DTPROFUP>19900101</DTPROFUP>"));
        // Both parse with the OFX reader.
        assert!(parse_tree(txt.as_bytes()).is_ok());
    }

    #[test]
    fn hosts_and_profiles() {
        assert_eq!(
            host_of("https://Ofx.Bank.example:443/x").unwrap(),
            "ofx.bank.example"
        );
        assert!(host_of("http://x.example/").is_err());
        assert!(host_of("https://u:p@x.example/").is_err());
        let mut i = inst("999");
        assert!(i.check().is_err());
        let mut i = inst("103");
        i.app_ver = "1".into();
        assert!(i.check().is_err());
        assert!(inst("103").check().is_ok());
    }

    #[test]
    fn html_is_not_ofx() {
        assert!(looks_like_html(
            b"<!DOCTYPE html><html><body>Moved</body></html>"
        ));
        assert!(!looks_like_html(
            b"OFXHEADER:100\r\nDATA:OFXSGML\r\n\r\n<OFX>"
        ));
    }
}
