//! Bank feeds (app-desktop revision 17): Settings → Connections, the Add connection
//! flow (masked setup token, confirmation, remote accounts), Sync now and the
//! scheduled syncs while the app is open. Revision 19 adds OFX Direct Connect
//! (profile, host acknowledgement, extra credentials and questions) and SnapTrade.

use serde_json::{Value, json};

use super::form::{choice, pick, put, set};
use super::ui::{Tone, Ui};
use super::util::Json;
use super::{App, AppEvent};

/// One remote account in the Add connection flow.
#[derive(Debug, Clone, Default)]
pub(crate) struct RemoteChoice {
    pub remote: String,
    pub name: String,
    pub currency: String,
    /// "" ignore, "+new" create an account, else an account's full name.
    pub target: String,
    /// OFX/SnapTrade: whether it can be linked, and its status in words.
    pub linkable: bool,
    pub status: String,
    /// `bank`, `card`, `investment`, `deposit`… ("" for SimpleFIN).
    pub kind: String,
    /// The Kind of a new account (index in `NEW_KINDS`; connectivity.md revision 4),
    /// starting at the provider's hint.
    pub new_kind: usize,
}

/// The kinds a new account from a feed can be: (label, type, subtype).
/// Investment only for providers that report holdings.
pub(crate) const NEW_KINDS: [(&str, &str, &str); 4] = [
    ("Bank", "asset", "bank"),
    ("Credit card", "liability", "credit-card"),
    ("Loan", "liability", "loan"),
    ("Investment", "asset", "brokerage"),
];

fn kind_index(hint: &str) -> usize {
    match hint {
        "credit-card" => 1,
        "loan" => 2,
        "investment" => 3,
        _ => 0,
    }
}

/// The Add connection dialog.
#[derive(Default)]
pub(crate) struct AddFlow {
    /// 0 token, 1 confirm, 2 accounts, 3 done, 4 resumable, 5 OFX host, 6 questions,
    /// 7 offer to set opening balances.
    pub step: u8,
    /// "" (SimpleFIN), `ofx`, `snaptrade`, `stripe`, `square` or `paypal`.
    pub provider: String,
    /// The pasted setup token, key or client id (cleared once used or cancelled).
    pub token: String,
    /// PayPal / SnapTrade: the secret (cleared once used).
    pub secret: String,
    pub show: bool,
    pub label: String,
    pub host: String,
    pub connection: Option<String>,
    pub accounts: Vec<RemoteChoice>,
    pub start: String,
    pub pending: bool,
    pub auto: bool,
    pub error: Option<String>,
    /// OFX: the installed profile chosen ("" = enter details).
    pub profile: String,
    pub inst: OfxForm,
    pub user_id: String,
    /// OFX password (cleared once used).
    pub password: String,
    pub cred1: String,
    pub cred2: String,
    /// (field, label) of extra credentials the bank asks for.
    pub cred_labels: Vec<(String, String)>,
    /// "I recognise this address as my bank's".
    pub ack: bool,
    pub ask_each: bool,
    pub client_uid: String,
    /// The bank's questions: (id, label, automatic, answer).
    pub questions: Vec<(String, String, bool, String)>,
    pub auth_token_label: String,
    pub auth_token: String,
    /// Add an account by hand.
    pub manual: Option<ManualForm>,
    /// Step 7 (opening-balances.md): the accounts just linked, for the checklist.
    pub linked: Vec<String>,
}

impl std::fmt::Debug for AddFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the token.
        f.debug_struct("AddFlow")
            .field("step", &self.step)
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

/// An OFX institution entered by hand.
#[derive(Debug, Clone, Default)]
pub(crate) struct OfxForm {
    pub name: String,
    pub url: String,
    pub org: String,
    pub fid: String,
    pub broker_id: String,
    pub version: usize,
    pub app_id: String,
    pub app_ver: String,
    pub advanced: bool,
}

/// A remote account added by hand.
#[derive(Debug, Clone, Default)]
pub(crate) struct ManualForm {
    /// 0 bank, 1 card, 2 investment.
    pub kind: usize,
    pub id: String,
    pub number: String,
    /// Index in `BANK_TYPES`.
    pub acct_type: usize,
}

const OFX_VERSIONS: [&str; 8] = ["103", "102", "151", "160", "200", "203", "211", "220"];
const BANK_TYPES: [&str; 5] = ["CHECKING", "SAVINGS", "MONEYMRKT", "CREDITLINE", "CD"];
const NOT_SECURE: &str = "This connection can't be saved securely: the book has no password and this computer has no credential store.";

/// The extra questions / password dialog of a sync (revision 19).
#[derive(Clone, Default)]
pub(crate) struct ChallengeForm {
    pub connection: String,
    pub label: String,
    /// (id, label, automatic, answer).
    pub questions: Vec<(String, String, bool, String)>,
    pub token: bool,
    pub token_text: String,
    pub password: bool,
    pub password_text: String,
    pub error: Option<String>,
}

impl std::fmt::Debug for ChallengeForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChallengeForm")
            .field("connection", &self.connection)
            .finish_non_exhaustive()
    }
}

/// A background sync: its job id, connection ("" for all) and whether its outcome is
/// shown (Sync now) or quiet (scheduled).
#[derive(Debug, Clone)]
pub(crate) struct Job {
    pub id: u64,
    pub connection: String,
    pub report: bool,
}

/// Processor settings (revision 18).
#[derive(Debug, Clone, Default)]
pub(crate) struct ProcForm {
    pub connection: String,
    pub label: String,
    /// 0 summary, 1 detailed.
    pub mode: usize,
    pub clearing: String,
    /// (category key, label, chosen account full name or "" for the default, default text)
    pub accounts: Vec<(String, String, String, String)>,
    pub match_invoices: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct ConnState {
    pub proc_form: Option<ProcForm>,
    pub add: Option<AddFlow>,
    /// Extra questions, a one-time code or this sync's password (revision 19).
    pub challenge: Option<ChallengeForm>,
    /// (connection id, label, new host, ticked): switching to a profile's new host.
    pub switch_host: Option<(String, String, String, bool)>,
    /// (connection id, new label) being renamed.
    pub rename: Option<(String, String)>,
    /// Connection id whose Disconnect… is being confirmed.
    pub disconnect: Option<String>,
    /// (connection id, label, earliest imported date) whose "Fix dates of imported
    /// transactions…" is being confirmed (revision 21).
    pub fix_dates: Option<(String, String, String)>,
    pub error: Option<String>,
    /// The clock time of the last scheduled sync check.
    pub last_check: Option<f64>,
    /// Background syncs.
    pub jobs: Vec<Job>,
}

/// (provider key, title) offered by Add connection.
fn providers(app: &mut App) -> Vec<(&'static str, &'static str)> {
    let mut v = Vec::new();
    if app.module_on("bank-feeds") {
        v.push(("", "SimpleFIN Bridge (bank and card feeds)"));
        v.push(("ofx", "Bank (OFX Direct Connect)"));
    }
    if app.module_on("investments") {
        v.push(("snaptrade", "Brokerage (SnapTrade)"));
    }
    if app.module_on("payment-processing") {
        v.extend([
            ("stripe", "Stripe"),
            ("square", "Square"),
            ("paypal", "PayPal"),
        ]);
    }
    v
}

/// A secret field (shown as typed with `show`).
fn masked(ui: &mut Ui, label: &str, text: &mut String, show: bool) {
    ui.form(label, |ui| {
        ui.width(320);
        if show {
            ui.field(label, text);
        } else {
            ui.password(label, text);
        }
    });
}

fn name_row(ui: &mut Ui, f: &mut AddFlow) {
    ui.form("Name", |ui| {
        ui.width(220);
        ui.field("Connection name", &mut f.label);
    });
}

fn conns(app: &mut App) -> Vec<Value> {
    super::util::array(app.read("connect.list", json!({})))
}

/// "2026-10-01T12:30:05.1Z" → "2026-10-01 12:30 UTC".
fn when(t: &str) -> String {
    if t.is_empty() {
        return "never".into();
    }
    match (t.get(..10), t.get(11..16)) {
        (Some(d), Some(h)) => format!("{} {h} UTC", crate::fmt::date(d)),
        _ => t.to_owned(),
    }
}

/// The result of a sync in words (banner).
fn sync_message(v: &Value) -> String {
    let r = v
        .at("connections")
        .as_array()
        .and_then(|a| a.first())
        .cloned();
    match r {
        Some(r) if r.at("challenge").is_array() => {
            format!("{}: Your bank asks extra questions.", r.str("label"))
        }
        Some(r) if r.at("error").is_object() => {
            format!("{}: {}", r.str("label"), r.at("error").str("message"))
        }
        Some(r) if !r.str("skipped").is_empty() => {
            format!("{}: {}", r.str("label"), r.str("skipped"))
        }
        Some(r) => {
            let n = r.at("new").as_u64().unwrap_or(0);
            let s = if n == 1 { "" } else { "s" };
            format!("{}: {n} new transaction{s} to review.", r.str("label"))
        }
        None => "Nothing to sync.".into(),
    }
}

/// The sync result notice; when it staged rows that wait for review it offers
/// **Review** (connectivity.md revision 4).
fn report_sync(app: &mut App, v: &Value) {
    let text = sync_message(v);
    let first = v
        .at("connections")
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(Value::Null);
    let batch = first.str("batch").to_owned();
    let waiting = first.at("new").as_u64().unwrap_or(0) > 0 && first.at("committed").is_null();
    if !batch.is_empty() && waiting {
        app.notify_with(
            text,
            super::MessageAction::Review(crate::View::Import { batch }),
        );
    } else {
        app.notify(text);
    }
}

/// Start a sync in the background (revision 17a); `connection` "" syncs every
/// connection on its schedule. Backends without background jobs run it at once.
fn start(app: &mut App, connection: &str, args: Value, report: bool) {
    start_cmd(app, "connect.sync", connection, args, report);
}

fn start_cmd(app: &mut App, cmd: &str, connection: &str, args: Value, report: bool) {
    match app.backend.net_start(cmd, args.clone()) {
        Ok(id) => app.conn.jobs.push(Job {
            id,
            connection: connection.to_owned(),
            report,
        }),
        Err(e) if e.code == "unsupported" => match app.run(cmd, args) {
            Ok(v) if report => {
                report_sync(app, &v);
                open_challenge(app, &v);
            }
            Ok(_) => {}
            Err(e) if report => app.notify_error(&e),
            Err(_) => {}
        },
        Err(e) if report => app.notify_error(&e),
        Err(_) => {}
    }
}

/// Sync one connection now (Sync now buttons).
pub(crate) fn sync_now(app: &mut App, connection: &str) {
    if syncing(app, connection) {
        return;
    }
    start(
        app,
        connection,
        json!({"connection": connection, "now": true}),
        true,
    );
}

/// The running background sync of a connection ("" for a scheduled run of all).
pub(crate) fn job_of(app: &App, connection: &str) -> Option<u64> {
    app.conn
        .jobs
        .iter()
        .find(|j| j.connection == connection || j.connection.is_empty())
        .map(|j| j.id)
}

pub(crate) fn syncing(app: &App, connection: &str) -> bool {
    job_of(app, connection).is_some()
}

/// "Syncing…" with a busy mark and Cancel (connection rows, register toolbar).
pub(crate) fn syncing_row(app: &mut App, ui: &mut Ui, id: u64, label: &str) {
    ui.progress("Syncing", -1);
    ui.label("Syncing…");
    ui.named(format!("Cancel sync {label}"));
    if ui.button("Cancel").clicked() {
        app.backend.net_cancel(id);
    }
}

/// Apply finished background syncs (every tick while any runs). Returns whether
/// anything changed.
pub(crate) fn poll(app: &mut App) -> bool {
    if app.conn.jobs.is_empty() && !super::capture::has_jobs(app) {
        return false;
    }
    let mut changed = false;
    for (id, out) in app.backend.net_poll() {
        changed = true;
        let Some(pos) = app.conn.jobs.iter().position(|j| j.id == id) else {
            // Phone capture jobs share the backend's network runner.
            super::capture::claim(app, id, out);
            continue;
        };
        let job = app.conn.jobs.remove(pos);
        match out {
            Ok(v) => {
                app.generation += 1;
                if job.report {
                    report_sync(app, &v);
                    open_challenge(app, &v);
                }
            }
            Err(e) if e.code == "cancelled" => {
                if job.report {
                    app.notify("Sync cancelled; nothing was recorded.");
                }
            }
            Err(e) if job.report => app.notify_error(&e),
            Err(_) => {}
        }
    }
    changed
}

/// On book open and then every hour while the app is open: sync connections whose
/// last sync is older than `bank-feeds.sync_interval_hours` (the command decides).
pub(crate) fn scheduled(app: &mut App) {
    let now = app.now;
    if app
        .conn
        .last_check
        .is_some_and(|t| !crate::layout::elapsed(now, t, 3600.0))
    {
        return;
    }
    app.conn.last_check = Some(now);
    if !super::accounts::connections_on(app) || !app.conn.jobs.is_empty() {
        return;
    }
    start(app, "", json!({"scheduled": true}), false);
}

/// A Sync now that came back with the bank's questions, a request for a one-time
/// code, or "Sync needs your password" opens the dialog that answers it.
fn open_challenge(app: &mut App, v: &Value) {
    let Some(r) = v
        .at("connections")
        .as_array()
        .and_then(|a| a.first())
        .cloned()
    else {
        return;
    };
    let qs: Vec<(String, String, bool, String)> = r
        .list("challenge")
        .iter()
        .map(|q| {
            (
                q.str("id").to_owned(),
                q.str("label").to_owned(),
                q.at("automatic").as_bool() == Some(true),
                String::new(),
            )
        })
        .collect();
    let token = r.at("error").str("code") == "auth_token";
    let password = r.str("skipped") == "Sync needs your password";
    if qs.is_empty() && !token && !password {
        return;
    }
    app.conn.challenge = Some(ChallengeForm {
        connection: r.str("connection").to_owned(),
        label: r.str("label").to_owned(),
        questions: qs,
        token,
        password,
        ..ChallengeForm::default()
    });
}

/// Ask for this sync's password ("Ask for my password each time") from Sync now.
pub(crate) fn sync_with_password(app: &mut App, connection: &str, label: &str) {
    app.conn.challenge = Some(ChallengeForm {
        connection: connection.to_owned(),
        label: label.to_owned(),
        password: true,
        ..ChallengeForm::default()
    });
}

fn questions(ui: &mut Ui, qs: &mut [(String, String, bool, String)]) {
    for (i, (_, label, auto, answer)) in qs.iter_mut().enumerate() {
        if *auto {
            ui.secondary(format!("{label}: answered by MoneyKeep"));
            continue;
        }
        ui.form(label, |ui| {
            ui.width(320);
            ui.password(&format!("Answer {}", i + 1), answer);
        });
    }
}

fn challenge_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.conn.challenge.take() else {
        return;
    };
    let mut send = false;
    let mut cancel = ui.dialog_closed("connection-challenge") || ui.key("escape");
    let only_password = f.password && f.questions.is_empty() && !f.token;
    let title = if only_password {
        "Password"
    } else {
        "Extra questions"
    };
    ui.dialog("connection-challenge", title, 560, |ui| {
        if only_password {
            ui.label(format!(
                "{}: Sync needs your password. It is used for this sync only and not stored.",
                f.label
            ));
            ui.group(|ui| masked(ui, "Password", &mut f.password_text, false));
        } else {
            ui.label(format!(
                "{}: your bank asks extra questions before it sends your statements.",
                f.label
            ));
            ui.group(|ui| {
                questions(ui, &mut f.questions);
                if f.token {
                    masked(ui, "One-time code", &mut f.token_text, false);
                }
                if f.password {
                    masked(ui, "Password", &mut f.password_text, false);
                }
            });
        }
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            cancel |= ui.button("Cancel").clicked();
            send = ui.primary("Send and sync").clicked();
        });
    });
    if cancel {
        // The connection keeps needing attention.
        return;
    }
    if send {
        let answers: Vec<Value> = f
            .questions
            .iter()
            .map(|(id, _, _, a)| json!({"id": id, "answer": a}))
            .collect();
        let mut a = json!({"connection": f.connection, "answers": answers});
        put(&mut a, "auth_token", &f.token_text);
        put(&mut a, "password", &f.password_text);
        // The answers and password go to this sync only.
        f.password_text.clear();
        f.token_text.clear();
        let id = f.connection.clone();
        start_cmd(app, "connect.answer_challenge", &id, a, true);
        return;
    }
    app.conn.challenge = Some(f);
}

fn switch_host_dialog(app: &mut App, ui: &mut Ui) {
    let Some((id, label, host, mut ticked)) = app.conn.switch_host.take() else {
        return;
    };
    let mut go = false;
    let mut cancel = ui.dialog_closed("connection-switch-host") || ui.key("escape");
    ui.dialog(
        "connection-switch-host",
        &format!("Switch {label}"),
        520,
        |ui| {
            ui.big_heading(host.clone());
            ui.label(format!("Your user id and password will be sent to {host}."));
            ui.checkbox(&mut ticked, "I recognise this address as my bank's");
            ui.actions(|ui| {
                cancel |= ui.button("Cancel").clicked();
                ui.enabled(ticked);
                go = ui.primary("Switch").clicked();
            });
        },
    );
    if go {
        app.conn.error = app
            .run(
                "connect.set_ofx_profile",
                json!({"connection": id, "accept_new_host": true}),
            )
            .err()
            .map(|e| e.display());
        return;
    }
    if !cancel {
        app.conn.switch_host = Some((id, label, host, ticked));
    }
}

fn month_start(app: &App) -> String {
    app.today()
        .get(..8)
        .map_or_else(String::new, |m| crate::fmt::date(&format!("{m}01")))
}

/// Settings → Connections.
pub(crate) fn settings_section(app: &mut App, ui: &mut Ui) {
    ui.card("Connections", |ui| {
        ui.label("Bank feeds bring transactions in through a provider you authorise. New transactions wait for review like an imported file.");
        ui.row(|ui| {
            if ui.primary("Add connection…").clicked() {
                app.conn.add = Some(AddFlow { start: month_start(app), ..AddFlow::default() });
            }
        });
        if let Some(e) = app.conn.error.clone() {
            ui.error_text(e);
        }
    });
    let list = conns(app);
    if list.is_empty() {
        ui.card("", |ui| ui.secondary("No connections yet."));
    }
    // Credentials stored by a claim whose connection was never saved (after a day).
    let unused = super::util::array(app.read("connect.unused", json!({})));
    if !unused.is_empty() {
        ui.card("Unused credentials", |ui| {
            for (i, u) in unused.iter().enumerate() {
                ui.row(|ui| {
                    ui.label("Unused credential");
                    ui.secondary("from a connection that was never finished");
                    ui.named(format!("Delete unused credential {}", i + 1));
                    if ui.button("Delete").clicked() {
                        app.conn.error = app
                            .run(
                                "connect.delete_credential",
                                json!({"credential": u.str("credential")}),
                            )
                            .err()
                            .map(|e| e.display());
                    }
                });
            }
        });
    }
    let staged = staged_batches(app);
    for c in &list {
        let id = c.str("id").to_owned();
        let label = c.str("label").to_owned();
        let mine: Vec<Value> = staged
            .iter()
            .filter(|b| b.str("connection") == id)
            .cloned()
            .collect();
        ui.scope(id.clone(), |ui| {
            connection_card(app, ui, c, &id, &label, &mine)
        });
    }
}

/// Staged feed batches with rows waiting for review (`connect.attention` items of
/// kind `batch`: batch, connection, provider, count and per-account counts).
pub(crate) fn staged_batches(app: &mut App) -> Vec<Value> {
    if !super::accounts::connections_on(app) {
        return Vec::new();
    }
    super::util::array(app.backend.invoke("connect.attention", json!({})).ok())
        .into_iter()
        .filter(|i| i.str("kind") == "batch")
        .collect()
}

/// Open a staged batch for review.
pub(crate) fn review(app: &mut App, batch: &str) {
    app.navigate(crate::View::Import {
        batch: batch.to_owned(),
    });
}

fn connection_card(app: &mut App, ui: &mut Ui, c: &Value, id: &str, label: &str, staged: &[Value]) {
    ui.card(label, |ui| {
        ui.wrap(|ui| {
            let mut what = format!("{} · {}", provider_title(c.str("provider")), c.str("host"));
            if c.at("ofx").is_object() {
                what.push_str(&format!(" · OFX {}", c.at("ofx").str("ofx_version")));
            }
            ui.secondary(what);
            let chip = c.str("status_label");
            ui.tip("Connection status");
            if c.str("status") == "active" {
                ui.badge(chip, Tone::Success);
            } else {
                ui.badge(chip, Tone::Error);
            }
        });
        ui.secondary(format!("Last sync: {}", when(c.str("last_sync_at"))));
        // app-desktop revision 38: the Attention notice can be dismissed here too; a
        // dismissed one is not shown again until a different one comes.
        if let Some(e) = c.at("last_error").as_object()
            && c.at("notice_dismissed") != &json!(true)
        {
            let msg = e
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let dismissible = matches!(c.str("status"), "needs_attention" | "revoked")
                && e.get("code").and_then(Value::as_str) != Some("disconnected");
            ui.row(|ui| {
                ui.error_text(msg.clone());
                if dismissible {
                    ui.spacer();
                    ui.tip("Dismiss");
                    if ui.icon_button("close", "Dismiss", false).clicked() {
                        super::dashboard::dismiss_notice(
                            app,
                            json!({"connection": id, "message": msg}),
                        );
                    }
                }
            });
        }
        // connectivity-brokerage.md revision 2: part of the last sync could not be
        // read while the rest was imported — a yellow line naming what failed.
        let partial = c.str("partial").to_owned();
        if !partial.is_empty() && !c.at("last_error").is_object() {
            ui.tone(Tone::Warning);
            ui.label(partial);
        }
        let notice = c.at("ofx").str("notice").to_owned();
        if !notice.is_empty() {
            ui.row(|ui| {
                ui.bold();
                ui.label("Profile changed");
                ui.label(notice.clone());
                let host = notice.rsplit(' ').next().unwrap_or_default().to_owned();
                ui.named(format!("Switch {label} to {host}"));
                if ui.button(format!("Switch to {host}…")).clicked() {
                    app.conn.switch_host = Some((id.to_owned(), label.to_owned(), host, false));
                }
            });
        }
        for a in c.list("accounts") {
            let target = if a.str("account_name").is_empty() {
                "ignored".to_owned()
            } else {
                format!("to {}", a.str("account_name"))
            };
            let mut line = if a.at("remote").is_object() {
                format!("{} {target}", a.str("name"))
            } else {
                format!("{} ({}) {target}", a.str("name"), a.str("currency"))
            };
            let att = a.str("attention").to_owned();
            if att.is_empty() || a.at("attention_dismissed") == &json!(true) {
                ui.secondary(line);
                continue;
            }
            line.push_str(&format!(" — {att}"));
            let linked = a.at("active") != &json!(false) && !a.str("account").is_empty();
            ui.row(|ui| {
                ui.secondary(line.clone());
                if linked {
                    ui.spacer();
                    ui.tip("Dismiss");
                    if ui.icon_button("close", "Dismiss", false).clicked() {
                        super::dashboard::dismiss_notice(
                            app,
                            json!({"connection": id, "link": a.str("id"), "message": att}),
                        );
                    }
                }
            });
        }
        // connectivity.md revision 4: what waits for review, with Review.
        for (i, b) in staged.iter().enumerate() {
            let text = b
                .str("message")
                .split_once(": ")
                .map_or_else(|| b.str("message").to_owned(), |(_, t)| t.to_owned());
            ui.row(|ui| {
                ui.bold();
                ui.label(text.clone());
                ui.named(if i == 0 {
                    format!("Review {label}")
                } else {
                    format!("Review {label} {}", i + 1)
                });
                if ui.primary("Review").clicked() {
                    review(app, b.str("batch"));
                }
            });
        }
        if c.str("status") == "unfinished" {
            ui.row(|ui| {
                ui.label("Not finished — resume");
                ui.named(format!("Resume {label}"));
                if ui.button("Resume").clicked() {
                    resume(app, id);
                }
            });
        }
        ui.wrap(|ui| {
            let active = matches!(c.str("status"), "active" | "needs_attention");
            if let Some(job) = job_of(app, id) {
                syncing_row(app, ui, job, label);
            } else {
                ui.enabled(active).named(format!("Sync now {label}"));
                if ui.button("Sync now").clicked() {
                    if c.at("ofx").at("ask_password").as_bool() == Some(true) {
                        sync_with_password(app, id, label);
                    } else {
                        sync_now(app, id);
                    }
                }
            }
            ui.named(format!("Rename {label}"));
            if ui.button("Rename…").clicked() {
                app.conn.rename = Some((id.to_owned(), label.to_owned()));
            }
            if matches!(c.str("provider"), "stripe" | "square" | "paypal") {
                ui.named(format!("Processor settings {label}"));
                if ui.button("Processor settings…").clicked() {
                    open_proc_form(app, id);
                }
            }
            match c.str("status") {
                "disabled" => {
                    ui.named(format!("Enable {label}"));
                    if ui.button("Enable").clicked() {
                        run(app, "connect.enable", id);
                    }
                }
                "revoked" => {}
                _ => {
                    ui.named(format!("Disable {label}"));
                    if ui.button("Disable").clicked() {
                        run(app, "connect.disable", id);
                    }
                }
            }
            // Bank feeds (SimpleFIN, OFX) can fetch again to correct dates of
            // imported transactions (revision 21).
            let feed = matches!(c.str("provider"), "simplefin" | "ofx" | "");
            let earliest = c.str("earliest_imported").to_owned();
            if feed && !earliest.is_empty() && job_of(app, id).is_none() {
                ui.enabled(active)
                    .named(format!("Fix dates of imported transactions {label}"));
                if ui.button("Fix dates of imported transactions…").clicked() {
                    app.conn.fix_dates = Some((id.to_owned(), label.to_owned(), earliest));
                }
            }
            if !c.str("credential_store").is_empty() {
                ui.named(format!("Disconnect {label}"));
                if ui.button("Disconnect…").clicked() {
                    app.conn.disconnect = Some(id.to_owned());
                }
            }
        });
    });
}

fn provider_title(p: &str) -> &'static str {
    match p {
        "stripe" => "Stripe",
        "square" => "Square",
        "paypal" => "PayPal",
        "ofx" => "OFX Direct Connect",
        "snaptrade" => "SnapTrade",
        _ => "SimpleFIN",
    }
}

const MODES: [(&str, &str); 2] = [("summary", "Summary"), ("detailed", "Detailed")];

fn open_proc_form(app: &mut App, id: &str) {
    let Some(v) = app.read("processor.settings", json!({"connection": id})) else {
        return;
    };
    let mut accounts: Vec<(String, String, String, String)> = v
        .at("accounts")
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, x)| {
                    (
                        k.clone(),
                        x.str("label").to_owned(),
                        x.at("account").str("name").to_owned(),
                        x.str("default").to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    // Categories in their natural order.
    let order = [
        "sale",
        "refund",
        "fee",
        "dispute",
        "dispute_reversal",
        "adjustment",
        "reserve",
        "tax",
        "other",
    ];
    accounts.sort_by_key(|a| order.iter().position(|o| *o == a.0).unwrap_or(99));
    app.conn.proc_form = Some(ProcForm {
        connection: id.to_owned(),
        label: v.str("label").to_owned(),
        mode: MODES.iter().position(|m| m.0 == v.str("mode")).unwrap_or(0),
        clearing: v.at("clearing").str("name").to_owned(),
        accounts,
        match_invoices: v.at("match_invoices").as_bool() != Some(false),
        error: None,
    });
}

fn proc_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.conn.proc_form.take() else {
        return;
    };
    let all: Vec<Value> = app.lists().accounts.clone();
    let names = |pred: &dyn Fn(&Value) -> bool| -> Vec<String> {
        all.iter()
            .filter(|a| a.at("placeholder").as_bool() != Some(true) && pred(a))
            .map(|a| a.str("full_name").to_owned())
            .collect()
    };
    let assets = names(&|a| a.str("type") == "asset");
    let any = names(&|_| true);
    let mut save = false;
    let mut close = ui.dialog_closed("processor-settings") || ui.key("escape");
    ui.dialog("processor-settings", &format!("Processor settings: {}", f.label), 600, |ui| {
        ui.group(|ui| {
            ui.form("Mode", |ui| {
                choice(ui, "Posting mode", &mut f.mode, &MODES.map(|m| m.1));
            });
            ui.form("Clearing account", |ui| {
                pick(ui, "Clearing account", &mut f.clearing, &assets, None);
            });
            for (_, label, chosen, default) in &mut f.accounts {
                let none = format!("Default ({default})");
                ui.form(label, |ui| {
                    pick(ui, &format!("{label} account"), chosen, &any, Some(&none));
                });
            }
        });
        ui.checkbox(&mut f.match_invoices, "Match invoice numbers");
        ui.secondary("Summary posts one transaction per payout; detailed posts every sale, refund and fee.");
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            close |= ui.button("Cancel").clicked();
            save = ui.primary("Save settings").clicked();
        });
    });
    if save {
        let accounts: serde_json::Map<String, Value> = f
            .accounts
            .iter()
            .map(|(k, _, chosen, _)| {
                (
                    k.clone(),
                    if chosen.is_empty() {
                        Value::Null
                    } else {
                        json!(chosen)
                    },
                )
            })
            .collect();
        let mut a = json!({"connection": f.connection, "accounts": accounts,
            "mode": MODES.get(f.mode).map_or("summary", |m| m.0), "match_invoices": f.match_invoices});
        put(&mut a, "clearing_account", &f.clearing);
        match app.run("processor.set", a) {
            Ok(_) => return,
            Err(e) => f.error = Some(e.display()),
        }
    }
    if !close {
        app.conn.proc_form = Some(f);
    }
}

/// Finish an interrupted connection (no new token): list its accounts and open the
/// linking step.
fn resume(app: &mut App, id: &str) {
    match app.run("connect.resume", json!({"connection": id})) {
        Ok(v) if v.at("resumable").as_bool() == Some(true) => {
            app.conn.error = Some(format!(
                "Not finished: {}",
                v.at("last_error").str("message")
            ));
        }
        Ok(v) => {
            app.conn.error = None;
            app.conn.add = Some(AddFlow {
                step: 2,
                connection: Some(id.to_owned()),
                accounts: remote_choices(&v),
                start: month_start(app),
                ..AddFlow::default()
            });
        }
        Err(e) => app.conn.error = Some(e.display()),
    }
}

fn remote_choices(v: &Value) -> Vec<RemoteChoice> {
    v.list("accounts")
        .iter()
        .map(|a| {
            let r = a.at("remote");
            RemoteChoice {
                remote: a.str("remote_account_id").to_owned(),
                name: a.str("name").to_owned(),
                currency: a.str("currency").to_owned(),
                target: String::new(),
                linkable: !r.is_object() || r.at("linkable").as_bool() == Some(true),
                status: r.str("status").to_owned(),
                kind: r.str("kind").to_owned(),
                new_kind: kind_index(a.str("kind_hint")),
            }
        })
        .collect()
}

fn run(app: &mut App, cmd: &str, id: &str) {
    app.conn.error = app
        .run(cmd, json!({"connection": id}))
        .err()
        .map(|e| e.display());
}

/// The dialogs (drawn over any view).
pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    add_dialog(app, ui);
    proc_dialog(app, ui);
    challenge_dialog(app, ui);
    switch_host_dialog(app, ui);
    rename_dialog(app, ui);
    disconnect_dialog(app, ui);
    fix_dates_dialog(app, ui);
}

/// Revision 21: confirm, then sync the connection again from its earliest imported
/// date (`connect.sync` with `from`); the import pipeline corrects the dates.
fn fix_dates_dialog(app: &mut App, ui: &mut Ui) {
    let Some((id, label, earliest)) = app.conn.fix_dates.take() else {
        return;
    };
    let mut go = false;
    let mut cancel = ui.dialog_closed("connection-fix-dates") || ui.key("escape");
    ui.dialog("connection-fix-dates", &format!("Fix dates: {label}"), 540, |ui| {
        ui.label(format!(
            "Fetch transactions again from {} and correct dates of ones already imported? Reconciled transactions and ones whose date you changed are left alone.",
            crate::fmt::date(&earliest)
        ));
        ui.actions(|ui| {
            cancel |= ui.button("Cancel").clicked();
            go = ui.primary("Fetch again").clicked();
        });
    });
    if go {
        if !syncing(app, &id) {
            start(
                app,
                &id,
                json!({"connection": id, "now": true, "from": earliest}),
                true,
            );
        }
    } else if !cancel {
        app.conn.fix_dates = Some((id, label, earliest));
    }
}

fn rename_dialog(app: &mut App, ui: &mut Ui) {
    let Some((id, mut label)) = app.conn.rename.take() else {
        return;
    };
    let mut save = false;
    let mut cancel = ui.dialog_closed("connection-rename") || ui.key("escape");
    ui.dialog("connection-rename", "Rename connection", 440, |ui| {
        ui.width(260);
        ui.field("Connection name", &mut label);
        ui.actions(|ui| {
            cancel |= ui.button("Cancel").clicked();
            save = ui.primary("Save").clicked();
        });
    });
    if save {
        match app.run(
            "connect.set_label",
            json!({"connection": id, "label": label}),
        ) {
            Ok(_) => return,
            Err(e) => app.conn.error = Some(e.display()),
        }
    }
    if !cancel && !save {
        app.conn.rename = Some((id, label));
    }
}

fn disconnect_dialog(app: &mut App, ui: &mut Ui) {
    let Some(id) = app.conn.disconnect.take() else {
        return;
    };
    let mut yes = false;
    let mut no = ui.dialog_closed("connection-disconnect") || ui.key("escape");
    ui.dialog("connection-disconnect", "Disconnect", 520, |ui| {
        ui.label("MoneyKeep deletes the saved access. SimpleFIN cannot be told to revoke it from here: also revoke MoneyKeep's access on your SimpleFIN Bridge account. Linked accounts and their transactions stay.");
        ui.actions(|ui| {
            no |= ui.button("Cancel").clicked();
            yes = ui.primary("Disconnect").clicked();
        });
    });
    if yes {
        run(app, "connect.disconnect", &id);
        return;
    }
    if !no {
        app.conn.disconnect = Some(id);
    }
}

fn not_secure(ui: &mut Ui, security: &mut bool) {
    ui.error_text(NOT_SECURE);
    if ui.row(|ui| ui.button("Open Settings, Security").clicked()) {
        *security = true;
    }
}

fn add_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.conn.add.take() else {
        return;
    };
    let secure = app
        .read("connect.providers", json!({}))
        .and_then(|p| p.as_array().and_then(|a| a.first()).cloned())
        .is_some_and(|p| p.at("secure_storage").as_bool() == Some(true));
    let (mut next, mut security, mut manual_add) = (false, false, false);
    let mut cancel = ui.dialog_closed("add-connection") || ui.key("escape");
    let offered = providers(app);
    if !offered.iter().any(|p| p.0 == f.provider) {
        f.provider = offered.first().map(|p| p.0.to_owned()).unwrap_or_default();
    }
    let installed = if f.step == 0 && f.provider == "ofx" {
        super::util::array(app.read("connect.ofx_institutions", json!({})))
    } else {
        Vec::new()
    };
    let accounts = if f.step == 2 {
        app.lists().accounts.clone()
    } else {
        Vec::new()
    };
    let mut copy = None;
    let mut opening = false;
    ui.dialog("add-connection", "Add connection", 600, |ui| {
        if f.step == 0 && offered.len() > 1 {
            let titles: Vec<String> = offered.iter().map(|p| p.1.to_owned()).collect();
            let mut idx = offered.iter().position(|p| p.0 == f.provider).unwrap_or(0);
            ui.form("Provider", |ui| {
                ui.width(320);
                if ui.combo("Provider", &mut idx, &titles).changed()
                    && let Some(p) = offered.get(idx)
                {
                    f.provider = p.0.to_owned();
                }
            });
        }
        match f.step {
            0 if f.provider == "ofx" => {
                ui.label("OFX Direct Connect talks to your bank's own OFX server with your online-banking user id and password. Only some banks still run one.");
                ui.group(|ui| {
                    let names: Vec<String> = installed.iter().map(|i| i.str("name").to_owned()).collect();
                    ui.form("Institution profile", |ui| {
                        pick(ui, "Institution profile", &mut f.profile, &names, Some("Enter details"));
                    });
                    if f.profile.is_empty() {
                        super::form::text_row(ui, "Institution name", &mut f.inst.name, "");
                        super::form::text_row(ui, "OFX address", &mut f.inst.url, "https://");
                        super::form::text_row(ui, "ORG", &mut f.inst.org, "");
                        super::form::text_row(ui, "FID", &mut f.inst.fid, "");
                        super::form::text_row(ui, "Broker id", &mut f.inst.broker_id, "");
                        ui.form("", |ui| {
                            ui.checkbox(&mut f.inst.advanced, "Advanced");
                        });
                        if f.inst.advanced {
                            ui.form("OFX version", |ui| {
                                choice(ui, "OFX version", &mut f.inst.version, &OFX_VERSIONS);
                            });
                            super::form::text_row(ui, "Application id", &mut f.inst.app_id, "");
                            super::form::text_row(ui, "Application version", &mut f.inst.app_ver, "");
                            ui.form("", |ui| ui.secondary("Use only values your institution allows."));
                        }
                    }
                    name_row(ui, &mut f);
                });
            }
            0 if f.provider == "snaptrade" => {
                ui.label("SnapTrade reads the brokerage accounts you connect in SnapTrade's dashboard, with a free personal key.");
                ui.label("1. Create a personal key in the SnapTrade dashboard.");
                ui.label("2. Connect your brokerages there.");
                ui.label("3. Paste the client id and consumer key here.");
                ui.group(|ui| {
                    masked(ui, "Client id", &mut f.token, f.show);
                    masked(ui, "Consumer key", &mut f.secret, f.show);
                    ui.form("", |ui| {
                        ui.checkbox(&mut f.show, "Show");
                    });
                    name_row(ui, &mut f);
                });
                if !secure {
                    not_secure(ui, &mut security);
                }
            }
            5 => {
                ui.big_heading(f.host.clone());
                ui.label(format!("Your user id and password will be sent to {}.", f.host));
                ui.checkbox(&mut f.ack, "I recognise this address as my bank's");
                ui.group(|ui| {
                    super::form::text_row(ui, "User id", &mut f.user_id, "");
                    masked(ui, "Password", &mut f.password, false);
                    let labels = f.cred_labels.clone();
                    for (field, label) in &labels {
                        let text = if field == "cred2" { &mut f.cred2 } else { &mut f.cred1 };
                        masked(ui, label, text, false);
                    }
                });
                ui.checkbox(&mut f.ask_each, "Ask for my password each time");
                if !secure && !f.ask_each {
                    ui.error_text(NOT_SECURE);
                }
                ui.row(|ui| {
                    ui.label(format!("Client id: {}", f.client_uid));
                    ui.named("Copy client id");
                    if ui.button("Copy").clicked() {
                        copy = Some(f.client_uid.clone());
                    }
                });
                ui.secondary("Some banks ask you to register this client id first.");
            }
            6 => {
                ui.heading("Extra questions");
                ui.group(|ui| {
                    questions(ui, &mut f.questions);
                    if !f.auth_token_label.is_empty() {
                        let l = f.auth_token_label.clone();
                        masked(ui, &l, &mut f.auth_token, false);
                    }
                });
            }
            0 if !f.provider.is_empty() => {
                ui.group(|ui| {
                    match f.provider.as_str() {
                        "stripe" => {
                            ui.label("Stripe payouts arrive in your bank as one deposit; MoneyKeep splits them into sales, refunds and fees.");
                            ui.label("Create a restricted key with read access only (Stripe Dashboard: Developers, API keys, Create restricted key). A secret key is refused.");
                            masked(ui, "Restricted key", &mut f.token, f.show);
                        }
                        "square" => {
                            ui.label("Square payouts arrive as one deposit; MoneyKeep splits them into sales, refunds and fees.");
                            ui.label("Create a personal access token with payout read permission (Square Developer Dashboard) and paste it here.");
                            masked(ui, "Access token", &mut f.token, f.show);
                        }
                        _ => {
                            ui.label("PayPal withdrawals arrive as one deposit; MoneyKeep splits them into sales, refunds and fees.");
                            ui.label("Create a REST app with Transaction Search (PayPal Developer Dashboard) and copy its client id and secret. MoneyKeep never stores the short-lived token.");
                            super::form::text_row(ui, "Client id", &mut f.token, "");
                            masked(ui, "Client secret", &mut f.secret, f.show);
                        }
                    }
                    ui.form("", |ui| {
                        ui.checkbox(&mut f.show, "Show");
                    });
                    name_row(ui, &mut f);
                });
                if !secure {
                    not_secure(ui, &mut security);
                }
            }
            0 => {
                ui.label("SimpleFIN Bridge connects to your bank for you and gives MoneyKeep read-only access to transactions.");
                ui.label("MoneyKeep never sees your bank password: paste the setup token from your SimpleFIN Bridge account.");
                if !secure {
                    not_secure(ui, &mut security);
                }
                ui.group(|ui| {
                    ui.form("Setup token", |ui| {
                        ui.width(320);
                        if f.show {
                            ui.field("Setup token", &mut f.token);
                        } else {
                            ui.password("Setup token", &mut f.token);
                        }
                        ui.checkbox(&mut f.show, "Show");
                    });
                    name_row(ui, &mut f);
                });
            }
            1 if !f.provider.is_empty() => {
                ui.label(format!("MoneyKeep will contact {} to check this credential. Continue?", f.host));
            }
            1 => {
                ui.label(format!("MoneyKeep will contact {} to claim this token. Continue?", f.host));
            }
            4 => {
                ui.label("The token was claimed and the access is saved. Resume when the provider answers; no new token is needed.");
            }
            7 => {
                ui.heading("Opening balances");
                ui.label("Your accounts are linked. Each one needs an opening balance so its balance matches your bank: the bank gives only recent transactions.");
                ui.secondary("MoneyKeep works it out from the balance your bank reports. You can change it any time.");
            }
            _ => accounts_step(ui, &mut f, &accounts, &mut manual_add),
        }
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if f.step == 7 {
                cancel |= ui.button("Later").clicked();
                ui.spacer();
                opening = ui.primary("Set opening balances now (recommended)").clicked();
                return;
            }
            if f.step < 2 || matches!(f.step, 5 | 6) {
                cancel |= ui.button("Cancel").clicked();
            } else if f.step == 4 {
                cancel |= ui.button("Close").clicked();
            }
            let go = match f.step {
                0 => "Continue",
                1 if !f.provider.is_empty() => "Connect",
                1 => "Claim",
                4 => "Resume",
                5 | 6 => "Connect",
                _ => "Done",
            };
            // OFX: Connect stays disabled until the host is acknowledged.
            ui.enabled(f.step != 5 || f.ack);
            next = ui.primary(go).clicked();
        });
    });
    if let Some(c) = copy {
        app.copy(c);
    }
    if security {
        app.config.settings_section = "Security".into();
        app.emit(AppEvent::ConfigChanged);
        return;
    }
    if opening {
        super::opening::open_checklist(app, Some(std::mem::take(&mut f.linked)));
        return;
    }
    if cancel {
        return;
    }
    if manual_add {
        add_manual(app, &mut f);
    }
    if next {
        advance(app, &mut f);
        if f.step == 3 {
            return;
        }
    }
    app.conn.add = Some(f);
}

/// Step 2: link each remote account, the start date and the options.
fn accounts_step(ui: &mut Ui, f: &mut AddFlow, accounts: &[Value], manual_add: &mut bool) {
    ui.heading("Accounts at this connection");
    // OFX and SnapTrade report holdings, so only they offer Investment.
    let holdings = matches!(f.provider.as_str(), "ofx" | "snaptrade");
    let kinds: Vec<&str> = NEW_KINDS
        .iter()
        .take(if holdings { 4 } else { 3 })
        .map(|k| k.0)
        .collect();
    ui.group(|ui| {
        for r in f.accounts.iter_mut() {
            if !r.linkable {
                // Inactive at the bank (or not an investment account): shown, not linkable.
                let status = if r.status.is_empty() {
                    "not linkable"
                } else {
                    r.status.as_str()
                };
                ui.named(format!("{} — {status} (not linkable)", r.name));
                ui.secondary(format!("{} — {status}", r.name));
                continue;
            }
            let caption = if r.kind.is_empty() {
                format!("{} ({})", r.name, r.currency)
            } else if r.status.is_empty() || r.status == "ACTIVE" {
                r.name.clone()
            } else {
                format!("{} — {}", r.name, r.status)
            };
            let mut options = vec!["Ignore".to_owned(), "Create a new account".to_owned()];
            options.extend(
                accounts
                    .iter()
                    .filter(|a| {
                        a.at("placeholder").as_bool() != Some(true)
                            && matches!(a.str("type"), "asset" | "liability")
                            && (r.currency.len() != 3 || a.str("commodity") == r.currency)
                            && (r.kind != "investment" || a.str("subtype") == "brokerage")
                    })
                    .map(|a| a.str("full_name").to_owned()),
            );
            let mut idx = match r.target.as_str() {
                "" => 0,
                "+new" => 1,
                t => options.iter().position(|o| o == t).unwrap_or(0),
            };
            ui.form(&caption, |ui| {
                ui.width(300);
                if ui
                    .combo(&format!("Link {}", r.name), &mut idx, &options)
                    .changed()
                {
                    r.target = match idx {
                        0 => String::new(),
                        1 => "+new".to_owned(),
                        _ => options.get(idx).cloned().unwrap_or_default(),
                    };
                }
            });
            if r.target == "+new" {
                if r.new_kind >= kinds.len() {
                    r.new_kind = 0;
                }
                ui.form("Kind", |ui| {
                    choice(ui, &format!("Kind of {}", r.name), &mut r.new_kind, &kinds);
                });
            }
        }
    });
    if f.provider == "ofx" {
        if let Some(m) = f.manual.as_mut() {
            ui.heading("Add an account by hand");
            ui.group(|ui| {
                ui.form("Account kind", |ui| {
                    choice(
                        ui,
                        "Account kind",
                        &mut m.kind,
                        &["Bank", "Credit card", "Investment"],
                    );
                });
                if m.kind == 0 {
                    super::form::text_row(ui, "Bank id", &mut m.id, "");
                    ui.form("Account type", |ui| {
                        choice(ui, "Account type", &mut m.acct_type, &BANK_TYPES);
                    });
                } else if m.kind == 2 {
                    super::form::text_row(ui, "Broker id", &mut m.id, "");
                }
                super::form::text_row(ui, "Account number", &mut m.number, "");
                ui.form("", |ui| {
                    if ui.button("Add account").clicked() {
                        *manual_add = true;
                    }
                });
            });
        } else if ui.row(|ui| ui.button("Add an account by hand…").clicked()) {
            f.manual = Some(ManualForm::default());
        }
    }
    ui.group(|ui| {
        super::form::date_row(ui, "Start from", &mut f.start);
        ui.form("", |ui| {
            ui.checkbox(&mut f.pending, "Include pending transactions");
        });
        ui.form("", |ui| {
            ui.checkbox(
                &mut f.auto,
                "Add new transactions automatically when nothing needs review",
            );
        });
    });
}

fn advance(app: &mut App, f: &mut AddFlow) {
    f.error = None;
    let input = if f.provider == "paypal" {
        format!("{}:{}", f.token.trim(), f.secret.trim())
    } else {
        f.token.clone()
    };
    match f.step {
        0 if f.provider == "ofx" => ofx_profile_step(app, f),
        5 | 6 => ofx_connect(app, f),
        0 if f.provider == "snaptrade" => {
            let mut a = json!({"provider": "snaptrade", "client_id": f.token.trim(), "consumer_key": f.secret.trim()});
            put(&mut a, "label", &f.label);
            let out = app.run("connect.add", a);
            f.secret.clear();
            match out {
                Ok(v) => {
                    f.token.clear();
                    f.connection = Some(v.str("id").to_owned());
                    f.accounts = remote_choices(&v);
                    f.step = 2;
                }
                Err(e) => f.error = Some(e.display()),
            }
        }
        0 => {
            let mut a = json!({"setup_input": input});
            put(&mut a, "label", &f.label);
            put(&mut a, "provider", &f.provider);
            match app.backend.invoke("connect.add", a) {
                Ok(v) => {
                    f.host = v.at("confirm").str("host").to_owned();
                    f.step = 1;
                }
                Err(e) => f.error = Some(e.display()),
            }
        }
        1 => {
            let mut a = json!({"setup_input": input, "confirm": true});
            put(&mut a, "label", &f.label);
            put(&mut a, "provider", &f.provider);
            let out = app.run("connect.add", a);
            // The token is used up either way: forget it.
            f.token.clear();
            f.secret.clear();
            match out {
                Ok(v) if !f.provider.is_empty() => {
                    // A processor connection is ready: sync it in the background.
                    let id = v.str("id").to_owned();
                    sync_now(app, &id);
                    f.step = 3;
                }
                Ok(v) if v.at("resumable").as_bool() == Some(true) => {
                    // The credential is kept: the connection can be resumed later
                    // without a new token.
                    f.connection = Some(v.str("id").to_owned());
                    f.error = Some(format!(
                        "Not finished — resume: {}",
                        v.at("last_error").str("message")
                    ));
                    f.step = 4;
                }
                Ok(v) => {
                    f.connection = Some(v.str("id").to_owned());
                    f.accounts = remote_choices(&v);
                    f.step = 2;
                }
                Err(e) => {
                    f.error = Some(e.display());
                    f.step = 0;
                }
            }
        }
        4 => {
            let Some(conn) = f.connection.clone() else {
                return;
            };
            match app.run("connect.resume", json!({"connection": conn})) {
                Ok(v) if v.at("resumable").as_bool() == Some(true) => {
                    f.error = Some(format!(
                        "Not finished — resume: {}",
                        v.at("last_error").str("message")
                    ));
                }
                Ok(v) => {
                    f.accounts = remote_choices(&v);
                    f.step = 2;
                }
                Err(e) => f.error = Some(e.display()),
            }
        }
        _ => link_accounts(app, f),
    }
}

/// Step 2's Done: create the new accounts, link every chosen one and sync.
fn link_accounts(app: &mut App, f: &mut AddFlow) {
    let Some(conn) = f.connection.clone() else {
        return;
    };
    let mut linked = Vec::new();
    for r in &f.accounts {
        let mut target = r.target.clone();
        if target == "+new" {
            let (_, ty, sub) = NEW_KINDS
                .get(r.new_kind)
                .copied()
                .unwrap_or(("Bank", "asset", "bank"));
            let mut acc = json!({"name": r.name, "type": ty, "subtype": sub});
            if r.currency.len() == 3 {
                set(&mut acc, "commodity", json!(r.currency));
            }
            match app.run("account.create", acc) {
                Ok(v) => target = v.str("full_name").to_owned(),
                Err(e) => {
                    f.error = Some(e.display());
                    return;
                }
            }
        }
        let mut a = json!({"connection": conn, "remote_account": r.remote,
            "account": target, "include_pending": f.pending, "auto_commit": f.auto});
        put(&mut a, "sync_from", &crate::fmt::date_arg_lenient(&f.start));
        if let Err(e) = app.run("connect.link_account", a) {
            f.error = Some(e.display());
            return;
        }
        if !target.is_empty() {
            linked.push(target);
        }
    }
    if f.accounts.iter().any(|r| !r.target.is_empty()) {
        sync_now(app, &conn);
    }
    // opening-balances.md: the last step offers to set opening balances.
    f.step = if linked.is_empty() { 3 } else { 7 };
    f.linked = linked;
}

/// OFX: the profile is chosen; show its host for the user to acknowledge.
fn ofx_profile_step(app: &mut App, f: &mut AddFlow) {
    let url = if f.profile.is_empty() {
        f.inst.url.trim().to_owned()
    } else {
        super::util::array(app.read("connect.ofx_institutions", json!({})))
            .iter()
            .find(|i| i.str("name") == f.profile)
            .map(|i| i.str("url").to_owned())
            .unwrap_or_default()
    };
    let Some(rest) = url.strip_prefix("https://") else {
        f.error = Some("The OFX address must start with https://".into());
        return;
    };
    if f.profile.is_empty() && f.inst.name.trim().is_empty() {
        f.error = Some("Give the institution's name".into());
        return;
    }
    let auth = rest.split(['/', '?', '#']).next().unwrap_or_default();
    f.host = auth
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    f.client_uid = app
        .read("connect.client_uid", json!({}))
        .map(|v| v.str("client_uid").to_owned())
        .unwrap_or_default();
    f.ack = false;
    f.step = 5;
}

/// OFX: connect (profile, then account information; extra credentials or questions
/// bring the user back here).
fn ofx_connect(app: &mut App, f: &mut AddFlow) {
    let mut a = json!({"provider": "ofx", "user_id": f.user_id.trim(), "password": f.password,
        "host_acknowledged": f.ack, "ask_password_each_time": f.ask_each});
    if f.profile.is_empty() {
        let mut inst = json!({"name": f.inst.name.trim(), "url": f.inst.url.trim()});
        put(&mut inst, "org", f.inst.org.trim());
        put(&mut inst, "fid", f.inst.fid.trim());
        put(&mut inst, "broker_id", f.inst.broker_id.trim());
        if f.inst.advanced {
            put(
                &mut inst,
                "ofx_version",
                OFX_VERSIONS.get(f.inst.version).copied().unwrap_or("103"),
            );
            put(&mut inst, "app_id", f.inst.app_id.trim());
            put(&mut inst, "app_ver", f.inst.app_ver.trim());
        }
        set(&mut a, "institution", inst);
    } else {
        set(&mut a, "institution_key", json!(f.profile));
    }
    put(&mut a, "label", &f.label);
    put(&mut a, "cred1", &f.cred1);
    put(&mut a, "cred2", &f.cred2);
    put(&mut a, "auth_token", &f.auth_token);
    if !f.questions.is_empty() {
        set(
            &mut a,
            "answers",
            Value::Array(
                f.questions
                    .iter()
                    .map(|(id, _, _, ans)| json!({"id": id, "answer": ans}))
                    .collect(),
            ),
        );
    }
    match app.run("connect.add", a) {
        Ok(v) if v.at("needs_credentials").is_array() => {
            f.cred_labels = v
                .list("needs_credentials")
                .iter()
                .map(|c| (c.str("field").to_owned(), c.str("label").to_owned()))
                .collect();
            f.error = Some(v.str("message").to_owned());
            f.step = 5;
        }
        Ok(v) if v.at("challenge").is_array() => {
            f.questions = v
                .list("challenge")
                .iter()
                .map(|q| {
                    (
                        q.str("id").to_owned(),
                        q.str("label").to_owned(),
                        q.at("automatic").as_bool() == Some(true),
                        String::new(),
                    )
                })
                .collect();
            f.step = 6;
        }
        Ok(v) if !v.str("needs_auth_token").is_empty() => {
            f.auth_token_label = v.str("needs_auth_token").to_owned();
            f.step = 6;
        }
        Ok(v) => {
            f.password.clear();
            f.cred1.clear();
            f.cred2.clear();
            f.auth_token.clear();
            f.connection = Some(v.str("id").to_owned());
            f.accounts = remote_choices(&v);
            if !v.str("message").is_empty() {
                f.error = Some(v.str("message").to_owned());
                f.manual = Some(ManualForm::default());
            }
            f.step = 2;
        }
        Err(e) => {
            f.error = Some(e.display());
            if f.step == 6 {
                f.questions.clear();
                f.step = 5;
            }
        }
    }
}

/// OFX: a remote account entered by hand.
fn add_manual(app: &mut App, f: &mut AddFlow) {
    let (Some(conn), Some(m)) = (f.connection.clone(), f.manual.clone()) else {
        return;
    };
    let kind = ["bank", "card", "investment"]
        .get(m.kind)
        .copied()
        .unwrap_or("bank");
    let mut a = json!({"connection": conn, "kind": kind, "account_number": m.number.trim()});
    match kind {
        "bank" => {
            put(&mut a, "bank_id", m.id.trim());
            put(
                &mut a,
                "account_type",
                BANK_TYPES.get(m.acct_type).copied().unwrap_or("CHECKING"),
            );
        }
        "investment" => put(&mut a, "broker_id", m.id.trim()),
        _ => {}
    }
    match app.run("connect.add_manual_account", a) {
        Ok(v) => {
            let keep: Vec<RemoteChoice> = f.accounts.clone();
            f.accounts = remote_choices(&v);
            for r in &mut f.accounts {
                if let Some(old) = keep.iter().find(|k| k.remote == r.remote) {
                    r.target.clone_from(&old.target);
                }
            }
            f.manual = None;
            f.error = None;
        }
        Err(e) => f.error = Some(e.display()),
    }
}
