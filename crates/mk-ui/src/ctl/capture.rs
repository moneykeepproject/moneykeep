//! Settings → Phone capture (app-desktop.md revision 28, mobile-capture.md): the
//! capture server's address and desktop token, Test connection, Pair a phone…,
//! paired phones with Revoke, automatic collection and Collect now. Requests run in
//! the background like bank-feed syncs; the window stays responsive.

use serde_json::{Value, json};

use super::App;
use super::form::set;
use super::ui::{Column, Tone, Ui, col};
use super::util::Json;

/// What a background job is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Test,
    Pair,
    Devices,
    Revoke,
    Collect {
        report: bool,
    },
    Publish,
    // Guided setup and the status light (revision 29).
    Check,
    Plan,
    Apply,
    Status {
        dialog: bool,
    },
    QrCode,
    Remove,
    Start,
    Repair,
    Update,
    /// Check (revision 8): the Access application against `server.toml`.
    Access,
    // Wi-Fi capture (mobile-capture.md revision 10).
    WifiAddresses,
    WifiApply,
    WifiPin,
    WifiPair,
    WifiChange,
    WifiRemove,
    // Start over (mobile-capture.md revision 17).
    StartOverPreview,
    StartOver,
    // The firewall check and its Fix (mobile-capture.md revision 18).
    FirewallCheck,
    FirewallFix,
    /// security-review-1 A1: "Remove the old copy".
    RemoveOldCopy,
    /// security-review-1 A6: "Install the server again".
    Reinstall,
    /// upgrade-resilience-1 U1.2: "Make a new certificate" (damaged Wi-Fi files).
    NewCertificate,
    /// macos-release.md C8: "Remove phone capture from this Mac…".
    RemoveFromMac,
}

/// Why the last Update now did not update.
#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdateOutcome {
    /// The Windows permission prompt was declined.
    Declined,
    /// It failed: the message in plain words, and the details.
    Failed { message: String, details: String },
}

/// The capture server update notice (release-packaging.md "Update notice and
/// Update now").
pub(crate) mod update_text {
    pub const TITLE: &str = "The capture server needs an update";
    pub const BODY: &str = "This MoneyKeep was updated, and the capture server that phones send to is still the older version. Updating replaces it with the version that came with this MoneyKeep and restarts it. It takes a few seconds; phones keep their pairing, and anything in a phone's Outbox is sent afterwards.";
    pub const PERMISSION: &str = "Windows will ask for permission, because the capture server runs in the background for this computer (it is installed for all users).";
    pub const NOT_BUILT: &str = "This copy of MoneyKeep was built without its capture server. Install MoneyKeep with its installer, or (developers) build with cargo xtask release.";
    pub const ATTENTION: &str = "A newer capture server is ready.";
    pub const LATER: &str = "The reminder on Home is hidden until MoneyKeep is opened again.";
    pub const UPDATE_NOW: &str = "Update now";
    pub const NOT_UPDATED: &str = "The capture server was not updated";
    /// The command's answer when the permission prompt was declined (the same
    /// words as mk-commands' `capture_setup::text::NOT_UPDATED`).
    pub const DECLINED: &str = "Not updated — Windows permission was not given. Phones can keep sending to the current server; press Update now when ready.";
}

/// How often the status light checks while the app is open (seconds).
const STATUS_EVERY: f64 = 60.0;

/// The pairing code on show.
#[derive(Debug, Clone)]
struct PairShown {
    code: String,
    /// Clock time at which the code expires.
    until: f64,
    client_url: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct CaptureState {
    jobs: Vec<(u64, Kind)>,
    url: Option<String>,
    token: String,
    trusted: Option<bool>,
    /// (ok, message) of the last Test connection or save.
    status: Option<(bool, String)>,
    pair: Option<PairShown>,
    devices: Option<Vec<Value>>,
    pub(crate) devices_gen: u64,
    /// When the last automatic collect started.
    last_collect: Option<f64>,
    /// upgrade-resilience-1 U1.9: Check now's collection runs; the status follows.
    status_after_collect: bool,
    /// Generation seen for the lists debounce, and when a change was first seen.
    lists_gen: u64,
    lists_dirty_since: Option<f64>,
    lists_hash: Option<String>,
    /// Revoke confirmation: (device id, name).
    confirm_revoke: Option<(String, String)>,
    /// Pair a phone… with both Wi-Fi and Cloudflare set up: "How will this phone
    /// send expenses?" is open (app-desktop revision 39).
    pair_choice: bool,
    /// The setup dialog (revision 29).
    pub(crate) setup: super::capture_setup::SetupDialog,
    /// The last `capture.status` answer (the light).
    pub(crate) light: Option<Value>,
    /// Generation at which "set up" was last read, and the answer.
    pub(crate) status_gen: u64,
    set_up: bool,
    last_status: Option<f64>,
    /// Remove phone capture: what will go, and whether a token must be typed.
    confirm_remove: Option<(Vec<String>, bool)>,
    /// Update server asked "Update anyway?" (revision 4a).
    confirm_update: bool,
    /// The last Update now that did not update (release-packaging.md).
    update_outcome: Option<UpdateOutcome>,
    /// Its Details are open.
    update_details: bool,
    remove_token: String,
    /// Revision 16: the last Remove phone capture that did not finish (what failed
    /// and why, what is left and where), shown with Retry undo.
    pub(crate) undo: Option<Value>,
    /// The token typed for Retry undo, when none is stored.
    undo_token: String,
    /// The last Check answer (revision 8): its text, and whether the Access
    /// application is gone (Create it again is offered).
    pub(crate) access: Option<(String, bool)>,
    /// Wi-Fi capture's setup dialog (revision 10).
    pub(crate) wifi: super::capture_wifi::WifiDialog,
    /// Start over (revision 17).
    pub(crate) start_over: super::capture_start_over::StartOver,
    /// macos-release.md C8: the Remove dialog.
    pub(crate) mac: super::capture_mac::MacState,
    /// security-review-1 A6: why the last Install the server again did not install.
    reinstall_error: Option<String>,
}

impl CaptureState {
    /// The paired phones as last listed.
    pub(crate) fn devices_list(&self) -> Option<Vec<Value>> {
        self.devices.clone()
    }
    /// Their ids.
    pub(crate) fn devices_ids(&self) -> Option<Vec<String>> {
        self.devices
            .as_ref()
            .map(|l| l.iter().map(|d| d.str("id").to_owned()).collect())
    }
}

/// Forget per-book state (a new book was opened).
pub(crate) fn reset(app: &mut App) {
    app.capture = CaptureState::default();
}

fn module_on(app: &mut App) -> bool {
    app.module_on(super::expenses::MODULE)
}

pub(crate) fn settings_json(app: &mut App) -> Value {
    app.backend
        .invoke("capture.settings", json!({}))
        .unwrap_or(Value::Null)
}

pub(crate) fn busy(app: &App, kind: Kind) -> bool {
    app.capture.jobs.iter().any(|(_, k)| *k == kind)
}

/// Start a capture command in the background (or at once on backends without
/// background jobs).
pub(crate) fn start(app: &mut App, cmd: &str, args: Value, kind: Kind) {
    match app.backend.net_start(cmd, args.clone()) {
        Ok(id) => app.capture.jobs.push((id, kind)),
        Err(e) if e.code == "unsupported" => {
            let out = app.backend.invoke(cmd, args);
            finished(app, kind, out);
        }
        Err(e) => finished(app, kind, Err(e)),
    }
}

/// A background job ended (called by the shared poll of network jobs). Returns
/// false when the job is not a phone capture job.
pub(crate) fn claim(app: &mut App, id: u64, out: Result<Value, crate::UiError>) -> bool {
    let Some(pos) = app.capture.jobs.iter().position(|(j, _)| *j == id) else {
        return false;
    };
    let (_, kind) = app.capture.jobs.remove(pos);
    finished(app, kind, out);
    true
}

pub(crate) fn has_jobs(app: &App) -> bool {
    !app.capture.jobs.is_empty()
}

fn finished(app: &mut App, kind: Kind, out: Result<Value, crate::UiError>) {
    match (kind, out) {
        (
            Kind::Check | Kind::Plan | Kind::Apply | Kind::QrCode | Kind::Status { dialog: true },
            out,
        ) => {
            super::capture_setup::finished(app, kind, out);
        }
        (
            Kind::WifiAddresses
            | Kind::WifiApply
            | Kind::WifiPin
            | Kind::WifiPair
            | Kind::WifiChange
            | Kind::WifiRemove,
            out,
        ) => super::capture_wifi::finished(app, kind, out),
        (Kind::FirewallCheck | Kind::FirewallFix, out) => {
            super::capture_wifi::firewall_finished(app, kind, out);
        }
        (Kind::RemoveFromMac, out) => {
            if super::capture_mac::finished(app, out) {
                app.capture.last_status = None;
            }
        }
        (Kind::StartOverPreview | Kind::StartOver, out) => {
            super::capture_start_over::finished(app, kind, out);
        }
        (Kind::Status { dialog: false }, Ok(v)) => app.capture.light = Some(v),
        (Kind::NewCertificate, Ok(_)) => {
            app.notify(NEW_CERTIFICATE_MADE);
            app.capture.last_status = None;
            app.generation += 1;
        }
        (Kind::Status { dialog: false }, Err(_)) => {}
        (Kind::Update, Err(e)) if e.code == "confirm_needed" => {
            app.capture.confirm_update = true;
        }
        (Kind::Update, Ok(v)) => {
            // release-packaging.md: "Capture server updated to 0.9.3."
            let text = match v.at("text").as_str() {
                Some(t) if !t.is_empty() => t.to_owned(),
                _ => "The capture server was updated.".to_owned(),
            };
            app.notify(text);
            app.capture.update_outcome = None;
            app.capture.last_status = None;
            // upgrade-resilience-1 U1.9: collect at once from the server that works.
            app.capture.last_collect = None;
            app.generation += 1;
        }
        (Kind::Update, Err(e)) if e.code == "permission_declined" => {
            app.capture.update_outcome = Some(UpdateOutcome::Declined);
        }
        (Kind::Update, Err(e)) => {
            let details = e.display();
            let message = details
                .strip_prefix("Updating the capture server failed: ")
                .unwrap_or(&details)
                .to_owned();
            app.capture.update_outcome = Some(UpdateOutcome::Failed { message, details });
            app.capture.update_details = false;
        }
        (Kind::Access, Ok(v)) => {
            let missing = v.at("app_missing").as_bool() == Some(true);
            app.capture.access = Some((v.str("text").to_owned(), missing));
            if !missing {
                // The light shows whether phones get through now.
                app.capture.last_status = Some(app.now);
                start(
                    app,
                    "capture.status",
                    json!({}),
                    Kind::Status { dialog: false },
                );
                if app.capture.setup.open {
                    start(
                        app,
                        "capture.status",
                        json!({}),
                        Kind::Status { dialog: true },
                    );
                }
            }
            app.generation += 1;
        }
        (Kind::Access, Err(e)) => app.capture.access = Some((e.display(), false)),
        (Kind::Repair, Ok(_)) => {
            app.notify("Cloudflare settings repaired: the phone stays signed in for 30 days.");
            app.generation += 1;
        }
        (Kind::Start, Ok(_)) => {
            app.notify("The capture server was started.");
            app.capture.last_status = None;
            app.capture.last_collect = None;
        }
        (Kind::Reinstall, Ok(v)) => {
            app.capture.reinstall_error = None;
            let text = match v.at("text").as_str() {
                Some(t) if !t.is_empty() => t.to_owned(),
                _ => "The capture server was installed again.".to_owned(),
            };
            app.notify(text);
            app.capture.last_status = None;
            app.capture.last_collect = None;
            // The firewall rule came back with it: check again.
            super::capture_wifi::check_firewall(app);
            app.generation += 1;
        }
        (Kind::Reinstall, Err(e)) => app.capture.reinstall_error = Some(e.display()),
        (Kind::RemoveOldCopy, Ok(_)) => {
            app.notify(OLD_COPY_REMOVED);
            if let Some(Value::Object(o)) = app.capture.light.as_mut() {
                o.remove("old_copy");
            }
            app.capture.last_status = None;
        }
        (Kind::RemoveOldCopy, Err(e)) => app.notify(e.display()),
        (Kind::Remove, Ok(v)) if v.at("removed").as_bool() == Some(false) => {
            app.capture.undo = Some(v);
            app.generation += 1;
        }
        (Kind::Remove, Ok(_)) => {
            app.notify("Phone capture was removed.");
            cleared(app);
        }
        (Kind::Test, Ok(v)) => {
            let n = v.at("devices").as_u64().unwrap_or(0);
            let s = if n == 1 { "" } else { "s" };
            app.capture.status = Some((
                true,
                format!("Connected to the capture server ({n} paired phone{s})."),
            ));
        }
        (Kind::Pair, Ok(v)) => {
            let secs = v.at("expires_in").as_i64().unwrap_or(600);
            app.capture.pair = Some(PairShown {
                code: v.str("code").to_owned(),
                until: crate::layout::add_secs(app.now, secs),
                client_url: v.at("client_url").as_str().map(str::to_owned),
            });
        }
        (Kind::Devices, Ok(v)) => {
            app.capture.devices = Some(v.list("devices").to_vec());
        }
        (Kind::Revoke, Ok(_)) => {
            app.notify("The phone was disconnected.");
            app.capture.devices_gen = 0;
        }
        (Kind::Collect { report }, Ok(v)) => {
            collected(app, report, &v);
            super::capture_start_over::collected(app);
            status_after_collect(app);
        }
        (Kind::Publish, Ok(v)) => {
            if let Some(h) = v.at("hash").as_str() {
                app.capture.lists_hash = Some(h.to_owned());
            }
        }
        (Kind::Test, Err(e)) => app.capture.status = Some((false, e.display())),
        (Kind::Collect { report: false }, Err(_)) => status_after_collect(app),
        (Kind::Publish, Err(_)) => {}
        (_, Err(e)) => app.notify_error(&e),
    }
}

/// U1.9: Check now collected first; now the status.
fn status_after_collect(app: &mut App) {
    if std::mem::take(&mut app.capture.status_after_collect) {
        app.capture.last_status = Some(app.now);
        start(
            app,
            "capture.status",
            json!({}),
            Kind::Status { dialog: false },
        );
    }
}

/// upgrade-resilience-1 U1.9: Check now. When collecting is on for this book it
/// collects first, so a fixed problem clears at once, then checks.
fn check_now(app: &mut App, s: &Value) {
    app.capture.last_status = Some(app.now);
    let collecting = s.at("configured").as_bool() == Some(true)
        && s.at("auto_collect").as_bool().unwrap_or(true);
    if collecting
        && !busy(app, Kind::Collect { report: false })
        && !busy(app, Kind::Collect { report: true })
    {
        app.capture.status_after_collect = true;
        app.capture.last_collect = Some(app.now);
        start(
            app,
            "capture.collect",
            json!({"scheduled": true}),
            Kind::Collect { report: false },
        );
    } else {
        start(
            app,
            "capture.status",
            json!({}),
            Kind::Status { dialog: false },
        );
    }
}

/// Phone capture was removed (or cleared by Start over): forget what this screen
/// knew about it.
pub(crate) fn cleared(app: &mut App) {
    app.capture.undo = None;
    app.generation += 1;
    app.capture.setup = super::capture_setup::SetupDialog::default();
    app.capture.wifi = super::capture_wifi::WifiDialog::default();
    app.capture.light = None;
    app.capture.status_gen = 0;
    app.capture.url = None;
    app.capture.token.clear();
    app.capture.devices = None;
    app.capture.pair = None;
}

fn collected(app: &mut App, report: bool, v: &Value) {
    if v.at("skipped").is_string() {
        return;
    }
    let e = v.at("expenses").as_u64().unwrap_or(0);
    let r = v.at("inbox_receipts").as_u64().unwrap_or(0);
    let trips = v.at("trips").as_u64().unwrap_or(0);
    let n = e + r + trips;
    if n > 0 {
        app.generation += 1;
    }
    if let Some(h) = v.at("lists_hash").as_str() {
        app.capture.lists_hash = Some(h.to_owned());
        app.capture.lists_gen = app.generation;
        app.capture.lists_dirty_since = None;
    }
    let warning = v.str("warning").to_owned();
    if report || n > 0 {
        let mut text = match (e, r) {
            (0, 0) => "Nothing new from your phone.".to_owned(),
            (e, 0) => format!("{e} expense{} from your phone collected.", plural(e)),
            (0, r) => format!(
                "{r} receipt{} from your phone added to the inbox.",
                plural(r)
            ),
            (e, r) => format!(
                "{e} expense{} and {r} receipt{} from your phone collected.",
                plural(e),
                plural(r)
            ),
        };
        if trips > 0 {
            // Trips (mobile-capture.md revision 11).
            let line = format!("{trips} trip{} from your phone taken.", plural(trips));
            text = if e + r == 0 {
                line
            } else {
                format!("{text} {line}")
            };
        }
        if !warning.is_empty() {
            text.push(' ');
            text.push_str(&warning);
        }
        app.notify(text);
    }
}

fn plural(n: u64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// On book open, then every `capture.collect_minutes` while open: collect when
/// automatic collection is on; and publish the lists a minute after the book changes.
pub(crate) fn scheduled(app: &mut App) {
    let now = app.now;
    if !module_on(app) {
        // Phone capture is off with its module: no light.
        app.capture.set_up = false;
        app.capture.status_gen = 0;
        return;
    }
    // The status light: whether phone capture is set up, then a check every minute.
    if app.capture.status_gen != app.generation {
        app.capture.status_gen = app.generation;
        let s = settings_json(app);
        app.capture.set_up = s.at("configured").as_bool() == Some(true)
            || s.at("setup").at("complete").as_bool() == Some(true);
        if !app.capture.set_up {
            app.capture.light = None;
        }
        // Revision 18: once after the app starts, with Wi-Fi capture on, the
        // firewall is checked (and Fix asked for once when the rule is missing).
        if !app.capture.wifi.firewall_started
            && s.at("wifi").at("installed").as_bool() == Some(true)
        {
            app.capture.wifi.firewall_started = true;
            start(
                app,
                "capture.wifi_firewall_check",
                json!({}),
                Kind::FirewallCheck,
            );
        }
    }
    if app.capture.set_up
        && app
            .capture
            .last_status
            .is_none_or(|t| crate::layout::elapsed(now, t, STATUS_EVERY))
        && !busy(app, Kind::Status { dialog: false })
    {
        app.capture.last_status = Some(now);
        start(
            app,
            "capture.status",
            json!({}),
            Kind::Status { dialog: false },
        );
    }
    let minutes = app
        .backend
        .invoke("module.settings", json!({"module": "expenses"}))
        .ok()
        .and_then(|s| {
            s.list("settings")
                .iter()
                .find(|d| d.str("key") == "capture.collect_minutes")
                .and_then(|d| d.at("value").as_i64())
        })
        .unwrap_or(15)
        .clamp(5, 240);
    let due = app.capture.last_collect.is_none_or(|t| {
        crate::layout::elapsed(now, t, crate::layout::secs_f64(minutes.saturating_mul(60)))
    });
    if due
        && !busy(app, Kind::Collect { report: false })
        && !busy(app, Kind::Collect { report: true })
    {
        app.capture.last_collect = Some(now);
        start(
            app,
            "capture.collect",
            json!({"scheduled": true}),
            Kind::Collect { report: false },
        );
        return;
    }
    // Lists: debounced one minute after a change in the book.
    if app.capture.lists_gen != app.generation {
        app.capture.lists_gen = app.generation;
        app.capture.lists_dirty_since = Some(now);
    }
    if let Some(since) = app.capture.lists_dirty_since
        && crate::layout::elapsed(now, since, 60.0)
        && !busy(app, Kind::Publish)
    {
        app.capture.lists_dirty_since = None;
        let s = settings_json(app);
        if s.at("configured").as_bool() == Some(true) {
            let args = match &app.capture.lists_hash {
                Some(h) => json!({"known_hash": h}),
                None => json!({}),
            };
            start(app, "capture.publish_lists", args, Kind::Publish);
        }
    }
}

/// The heading of the Cloudflare part of Settings → Phone capture (revision 12).
pub(crate) const CLOUDFLARE_HEADING: &str = "Advanced: phone capture over Cloudflare";

/// Settings → Phone capture: the Wi-Fi part first (setup, status, phones), then
/// collecting, then the Cloudflare part (mobile-capture.md revision 12).
pub(crate) fn settings_section(app: &mut App, ui: &mut Ui) {
    let s = settings_json(app);
    update_card(app, ui);
    super::capture_wifi::settings_card(app, ui);
    ui.card("Status", |ui| {
        ui.secondary(
            "Photograph receipts and enter expenses on your phone; they wait on your capture server until MoneyKeep collects them into this book.",
        );
        status_light(app, ui, &s);
        super::capture_mac::status_note(app, ui);
        super::capture_wifi::firewall_status(app, ui);
    });
    let configured = s.at("configured").as_bool().unwrap_or(false);
    phones(app, ui, configured);
    collecting(app, ui, &s, configured);
    super::capture_mac::card(app, ui);
    super::capture_start_over::card(app, ui);
    ui.card(CLOUDFLARE_HEADING, |ui| {
        ui.secondary(
            "For phones that must send from away from home, or networks that keep devices apart. It needs your own domain on Cloudflare; Wi-Fi capture is enough for most people.",
        );
        if ui.row(|ui| ui.button("Cloudflare settings…").clicked()) {
            app.config.settings_section = "Advanced".to_owned();
            app.emit(crate::AppEvent::ConfigChanged);
        }
    });
}

/// Settings > Advanced (revision 10): phone capture over Cloudflare, and a capture
/// server set up by hand.
pub(crate) fn advanced_section(app: &mut App, ui: &mut Ui) {
    let s = settings_json(app);
    ui.card("Phone capture over Cloudflare", |ui| {
        ui.secondary(
            "For phones that must send from away from home, or networks that keep devices apart: the capture server is reached through a Cloudflare Tunnel with Cloudflare Access sign-in. It needs your own domain on Cloudflare. It can be on together with Wi-Fi capture; each phone uses one of the two addresses.",
        );
        status_and_setup(app, ui, &s);
    });
    if app.capture.url.is_none() {
        app.capture.url = Some(s.str("url").to_owned());
    }
    if app.capture.trusted.is_none() {
        app.capture.trusted = Some(s.at("trusted").as_bool().unwrap_or(false));
    }
    let has_token = s.at("has_token").as_bool().unwrap_or(false);
    let configured = s.at("configured").as_bool().unwrap_or(false);
    let mut save = false;
    ui.card("Connection", |ui| {
        ui.secondary(
            "Filled in by the setup; you can also enter a server you set up yourself (see the deployment guide).",
        );
        ui.group(|ui| {
            ui.form("Server address", |ui| {
                let url = app.capture.url.get_or_insert_with(String::new);
                ui.width(320);
                ui.field("Server address", url);
            });
            ui.form("", |ui| {
                let mut trusted = app.capture.trusted.unwrap_or(false);
                ui.tip("Needed for a plain http:// address on your home network");
                if ui.checkbox(&mut trusted, "This network is trusted").changed() {
                    app.capture.trusted = Some(trusted);
                }
            });
            ui.form("", |ui| {
                ui.secondary("https://capture-api.example.com, or http://127.0.0.1:8787 when the server runs on this computer.");
            });
            ui.form("Desktop token", |ui| {
                let hint = if has_token { "Stored — enter a new one to replace it" } else { "Printed by moneykeep-server init" };
                ui.width(320).hint(hint);
                ui.password("Desktop token", &mut app.capture.token);
            });
        });
        ui.row(|ui| {
            save = ui.primary("Save").clicked();
            let testing = busy(app, Kind::Test);
            if testing {
                ui.progress("Testing", -1);
            }
            ui.enabled(configured && !testing);
            if ui.button("Test connection").clicked() {
                app.capture.status = None;
                start(app, "capture.test", json!({}), Kind::Test);
            }
        });
        if let Some((ok, m)) = app.capture.status.clone() {
            if ok {
                ui.label(m);
            } else {
                ui.error_text(m);
            }
        }
    });
    if save {
        let url = app.capture.url.clone().unwrap_or_default();
        let token = std::mem::take(&mut app.capture.token);
        let mut args = json!({"url": url.trim(), "trusted": app.capture.trusted.unwrap_or(false)});
        if !token.trim().is_empty() {
            set(&mut args, "token", json!(token.trim()));
        }
        match app.run("capture.set_server", args) {
            Ok(v) => {
                app.capture.url = Some(v.str("url").to_owned());
                app.capture.status = Some((true, "Saved.".into()));
                app.capture.devices_gen = 0;
            }
            Err(e) => {
                app.capture.token = token;
                app.capture.status = Some((false, e.display()));
            }
        }
    }
}

/// Pairing and paired phones.
fn phones(app: &mut App, ui: &mut Ui, configured: bool) {
    if configured && app.capture.devices_gen != app.generation && !busy(app, Kind::Devices) {
        app.capture.devices_gen = app.generation;
        start(app, "capture.devices", json!({}), Kind::Devices);
    }
    let devices = app.capture.devices.clone().unwrap_or_default();
    let mut revoke = None;
    ui.card("Paired phones", |ui| {
        // app-desktop revision 38: one Pair a phone… button, opening the QR pairing
        // flow of the active setup (Wi-Fi when set up, else Cloudflare).
        ui.row(|ui| {
            let pairing = busy(app, Kind::Pair);
            ui.enabled(configured && !pairing);
            if ui.button("Pair a phone…").clicked() {
                pair_a_phone(app);
            }
            if pairing {
                ui.progress("Pairing", -1);
            }
        });
        if !configured {
            ui.secondary("Set up Wi-Fi capture first (or Cloudflare under Settings > Advanced).");
        } else if devices.is_empty() {
            ui.secondary("No phones are paired.");
        } else {
            let cols = vec![
                Column {
                    weight: 2,
                    ..col("Name")
                },
                Column {
                    width: 110,
                    ..col("Way in")
                },
                Column {
                    width: 120,
                    ..col("Paired")
                },
                Column {
                    width: 120,
                    ..col("Last seen")
                },
                Column {
                    width: 120,
                    ..col("")
                },
            ];
            ui.table("", cols, |ui| {
                for d in &devices {
                    let name = d.str("name").to_owned();
                    ui.table_row(d.str("id"), false, false, |ui| {
                        ui.label(name.clone());
                        ui.label(if d.str("via") == "wifi" {
                            "Wi-Fi"
                        } else {
                            "Cloudflare"
                        });
                        ui.label(epoch_date(d.at("paired_at").as_i64()));
                        ui.label(epoch_date(d.at("last_seen").as_i64()));
                        ui.named(format!("Revoke {name}"));
                        if ui.button("Revoke").clicked() {
                            revoke = Some((d.str("id").to_owned(), name.clone()));
                        }
                    });
                }
            });
        }
    });
    if revoke.is_some() {
        app.capture.confirm_revoke = revoke;
    }
}

fn collecting(app: &mut App, ui: &mut Ui, s: &Value, configured: bool) {
    ui.card("Collecting", |ui| {
        let mut auto = s.at("auto_collect").as_bool().unwrap_or(true);
        if ui.checkbox(&mut auto, "Collect automatically").changed()
            && let Err(e) = app.run(
                "module.set_setting",
                json!({"key": "capture.auto_collect", "value": auto}),
            )
        {
            app.notify_error(&e);
        }
        ui.secondary(format!(
            "When the book opens and every {} minutes while it is open.",
            s.at("collect_minutes").as_i64().unwrap_or(15)
        ));
        ui.row(|ui| {
            let collecting = busy(app, Kind::Collect { report: true })
                || busy(app, Kind::Collect { report: false });
            ui.enabled(configured && !collecting);
            if ui.button("Collect now").clicked() {
                start(
                    app,
                    "capture.collect",
                    json!({}),
                    Kind::Collect { report: true },
                );
            }
            if collecting {
                ui.progress("Collecting", -1);
                ui.label("Collecting…");
            }
        });
        if let Some(at) = s.at("last_collect_at").as_str() {
            ui.secondary(format!(
                "Last collected: {}",
                crate::fmt::date(at.get(..10).unwrap_or(at))
            ));
        }
        // mobile-capture.md revision 16c: which way it collected.
        if let Some(way) = s.at("last_route_text").as_str() {
            ui.secondary(format!("{way}."));
        }
    });
}

/// The big status light with Check now, Update server and Start server.
/// security-review-1 A1: the old copy's notice.
pub(crate) const OLD_COPY_TEXT: &str = "The capture server's data now lives in a folder only the server and administrators can open. A copy of it is still in your user folder.";
pub(crate) const OLD_COPY_BUTTON: &str = "Remove the old copy";
pub(crate) const OLD_COPY_REMOVED: &str = "The old copy of the capture server's data was removed.";
/// security-review-1 A6: the service is gone but its data is still here.
pub(crate) const REINSTALL_BUTTON: &str = "Install the server again";
pub(crate) const REINSTALL_TIP: &str =
    "Windows asks for permission once; paired phones and waiting expenses are kept";
pub(crate) const REINSTALL_TEXT: &str = "The capture server's data is still on this computer: paired phones, the Wi-Fi certificate and expenses not yet collected. Installing the server again keeps them and, for Wi-Fi capture, puts the firewall rule back. Windows asks for permission once.";
/// The stopped service's button (A6: one permission prompt).
pub(crate) const START_BUTTON: &str = "Start the server";
/// upgrade-resilience-1 U1.2: after Make a new certificate.
pub(crate) const NEW_CERTIFICATE_MADE: &str = "A new certificate was made for Wi-Fi capture. Install it on each phone again: Certificate QR code….";
/// U1.4: going back to an earlier MoneyKeep.
pub(crate) const NO_DOWNGRADE: &str = "Going back to an earlier version of MoneyKeep isn't supported: install this version or a newer one (see Phone capture help).";
/// U1.4: the capture server's API level is lower than this desktop needs.
pub(crate) const UPDATE_SERVER_DETAIL: &str = "The capture server on this computer is older than this MoneyKeep needs. Press Update now, or install MoneyKeep again with its installer.";
/// U1.2: Make a new certificate, and what it means for the phones.
pub(crate) const NEW_CERTIFICATE: &str = "Make a new certificate";
pub(crate) const NEW_CERTIFICATE_DETAIL: &str = "Wi-Fi capture can't start because its certificate files are damaged. Make a new certificate: every phone will then need the certificate installed again (Certificate QR code…); pairings and waiting expenses are kept.";
/// U1.7: after Change address, the phones that haven't connected since.
pub(crate) const STALE_PHONES: &str = "These phones haven't connected since the address changed. Install the new certificate on each (Certificate QR code…), then open the new address:";
/// U1.3: Clean up (the About MoneyKeep dialog) after the Status card's 30 days.
pub(crate) const CLEAN_UP: &str = "Clean up";

/// "Remove the old copy" (security-review-1 A1): on the Status card for 30 days
/// after the move (upgrade-resilience-1 U1.3: then under Clean up).
fn old_copy(app: &mut App, ui: &mut Ui, status: &Value) {
    let oc = status.at("old_copy");
    let path = oc.str("path");
    if path.is_empty() || oc.at("on_card").as_bool() == Some(false) {
        return;
    }
    old_copy_banner(app, ui, path);
}

/// U1.3: the old copy still there after the Status card's 30 days (for Clean up).
pub(crate) fn old_copy_to_clean(app: &App) -> Option<String> {
    let oc = app.capture.light.as_ref()?.at("old_copy");
    (oc.at("on_card").as_bool() == Some(false) && !oc.str("path").is_empty())
        .then(|| oc.str("path").to_owned())
}

/// The old copy's notice with Remove the old copy (the Status card and Clean up).
pub(crate) fn old_copy_banner(app: &mut App, ui: &mut Ui, path: &str) {
    let removing = busy(app, Kind::RemoveOldCopy);
    ui.banner(Tone::Accent, |ui| {
        ui.label(OLD_COPY_TEXT);
        ui.secondary(path);
        ui.row(|ui| {
            ui.enabled(!removing)
                .tip("Phones stay paired: the capture server no longer uses this copy");
            if ui.button(OLD_COPY_BUTTON).clicked() {
                start(
                    app,
                    "capture.remove_old_copy",
                    json!({}),
                    Kind::RemoveOldCopy,
                );
            }
        });
    });
}

fn status_light(app: &mut App, ui: &mut Ui, s: &Value) {
    let setup = s.at("setup");
    let status = if app.capture.set_up {
        app.capture.light.clone().unwrap_or_else(|| {
            json!({"state": "grey", "text": "Checking…", "accessible_name": "Phone capture status: Checking…"})
        })
    } else {
        json!({"state": "grey", "text": "Phone capture is off",
               "accessible_name": "Phone capture status: Phone capture is off"})
    };
    ui.row(|ui| {
        ui.tip(super::capture_setup::tooltip(&status));
        ui.light(status.str("state"), 32, status.str("accessible_name"));
        ui.heading(status.str("text"));
    });
    for p in status.list("problems").iter().skip(1) {
        ui.label(p.as_str().unwrap_or(""));
    }
    // upgrade-resilience-1 U1.9: an earlier collection error, as a note.
    if let Some(note) = status.at("collect_note").as_str() {
        ui.secondary(note);
    }
    if status.at("level_low").as_bool() == Some(true) {
        ui.secondary(UPDATE_SERVER_DETAIL);
    }
    if status
        .at("refusal_kind")
        .as_str()
        .is_some_and(|k| k.starts_with("newer"))
    {
        ui.secondary(NO_DOWNGRADE);
        if ui.link("Phone capture help").clicked() {
            app.capture.wifi.help_open = true;
        }
    }
    new_certificate(app, ui, &status, s);
    stale_phones(app, ui, s);
    access_check(
        app,
        ui,
        &status,
        setup.at("complete").as_bool() == Some(true),
    );
    ui.wrap(|ui| {
        if app.capture.set_up {
            let checking = busy(app, Kind::Status { dialog: false });
            ui.enabled(!checking);
            if ui.button("Check now").clicked() {
                check_now(app, s);
                super::capture_wifi::check_firewall(app);
            }
            if checking {
                ui.progress("Checking", -1);
            }
        }
        // Update server stays for moving an installation under the old name
        // (branding.md step 6); a version update uses the notice's Update now.
        if status.at("can_update").as_bool() == Some(true) && !update_needed(app) {
            let updating = busy(app, Kind::Update);
            ui.enabled(!updating)
                .tip("Replace the capture server with the newly built one (Windows asks for permission once)");
            if ui.primary("Update server").clicked() {
                start(app, "capture.update_server", json!({}), Kind::Update);
            }
            if updating {
                ui.progress("Updating", -1);
                ui.label("Updating…");
            }
        }
        // security-review-1 A6: the service is gone but its data is here.
        if status.at("can_reinstall").as_bool() == Some(true) {
            let installing = busy(app, Kind::Reinstall);
            ui.enabled(!installing).tip(REINSTALL_TIP);
            if ui.primary(REINSTALL_BUTTON).clicked() {
                app.capture.reinstall_error = None;
                start(app, "capture.reinstall_server", json!({}), Kind::Reinstall);
            }
            if installing {
                ui.progress("Installing", -1);
                ui.label("Installing…");
            }
        }
        if status.at("can_start").as_bool() == Some(true) {
            ui.enabled(!busy(app, Kind::Start))
                .tip("Windows asks for permission once");
            if ui.primary(START_BUTTON).clicked() {
                start(app, "capture.start_server", json!({}), Kind::Start);
            }
        }
    });
    if status.at("can_reinstall").as_bool() == Some(true) {
        // macos-release.md C1: on a Mac no permission is asked, and a moved app
        // leaves the agent pointing at its old place.
        ui.secondary(if status.str("os") == "macos" {
            super::capture_mac::REINSTALL_TEXT
        } else {
            REINSTALL_TEXT
        });
        if let Some(e) = app.capture.reinstall_error.clone() {
            ui.error_text(e);
        }
    }
    if app.capture.set_up {
        ui.secondary(super::capture_setup::tooltip(&status));
    }
    old_copy(app, ui, &status);
}

/// upgrade-resilience-1 U1.2: Wi-Fi capture's certificate files are damaged; Make a
/// new certificate runs Wi-Fi setup's step again for the same address (the only
/// way, besides Wi-Fi setup, that a new certificate authority is made).
fn new_certificate(app: &mut App, ui: &mut Ui, status: &Value, s: &Value) {
    if status.at("wifi_damaged").as_bool() != Some(true) {
        return;
    }
    let w = s.at("wifi");
    let making = busy(app, Kind::NewCertificate);
    let mut go = false;
    ui.banner(Tone::Warning, |ui| {
        ui.label(NEW_CERTIFICATE_DETAIL);
        ui.row(|ui| {
            ui.enabled(!making && !w.str("address").is_empty())
                .tip("Windows asks for permission once");
            go = ui.primary(NEW_CERTIFICATE).clicked();
            if making {
                ui.progress("Making", -1);
            }
        });
    });
    if go {
        let mut args = json!({"address": w.str("address"), "name_anyway": true});
        if let Some(o) = args.as_object_mut() {
            if !w.str("name").is_empty() {
                o.insert("name".into(), json!(w.str("name")));
            }
            for k in ["https_port", "http_port"] {
                if let Some(p) = w.at(k).as_u64().filter(|p| *p > 0) {
                    o.insert(k.into(), json!(p));
                }
            }
        }
        start(app, "capture.wifi_apply", args, Kind::NewCertificate);
    }
}

/// upgrade-resilience-1 U1.7: after Change address, the phones paired over Wi-Fi
/// that haven't connected since, with Certificate QR code….
pub(crate) fn stale_phone_names(app: &App, s: &Value) -> Vec<String> {
    let Some(since) = s.at("wifi").at("address_changed_at").as_i64() else {
        return Vec::new();
    };
    app.capture
        .devices
        .as_ref()
        .map(|list| {
            list.iter()
                .filter(|d| d.str("via") == "wifi")
                .filter(|d| {
                    d.at("last_seen")
                        .as_i64()
                        .or_else(|| d.at("paired_at").as_i64())
                        .is_none_or(|t| t < since)
                })
                .map(|d| d.str("name").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn stale_phones(app: &mut App, ui: &mut Ui, s: &Value) {
    let names = stale_phone_names(app, s);
    if names.is_empty() {
        return;
    }
    let mut open = false;
    ui.banner(Tone::Accent, |ui| {
        ui.label(STALE_PHONES);
        for n in &names {
            ui.label(format!("• {n}"));
        }
        ui.row(|ui| {
            open = ui.button("Certificate QR code…").clicked();
        });
    });
    if open {
        super::capture_wifi::open(app, Some(4));
    }
}

/// The last status says the running server is not this build's (release-packaging.md).
fn update_needed(app: &App) -> bool {
    app.capture.set_up
        && app
            .capture
            .light
            .as_ref()
            .is_some_and(|l| l.at("update_needed").as_bool() == Some(true))
}

/// … and the program beside the app is this build's, so Update now can replace it.
fn update_ready(app: &App) -> bool {
    update_needed(app)
        && app
            .capture
            .light
            .as_ref()
            .is_some_and(|l| l.at("update_ready").as_bool() == Some(true))
}

/// Home → Attention: "A newer capture server is ready." (opens Settings → Phone
/// capture), until Later is pressed; never when Update now is not offered.
pub(crate) fn attention_item(app: &App) -> Option<Value> {
    (update_ready(app) && !app.capture_update_later).then(|| {
        json!({
            "label": update_text::ATTENTION,
            "target": {"screen": "settings", "section": "Phone capture"},
        })
    })
}

/// Settings → Phone capture: the update notice card. Update now is the only way the
/// update starts.
fn update_card(app: &mut App, ui: &mut Ui) {
    if !update_needed(app) {
        return;
    }
    let ready = update_ready(app);
    let windows = app
        .capture
        .light
        .as_ref()
        .is_some_and(|l| l.str("os") == "windows");
    ui.card(update_text::TITLE, |ui| {
        if !ready {
            ui.label(update_text::NOT_BUILT);
            return;
        }
        ui.label(update_text::BODY);
        if windows {
            ui.label(update_text::PERMISSION);
        }
        let updating = busy(app, Kind::Update);
        let mut now = false;
        let mut later = false;
        ui.row(|ui| {
            ui.enabled(!updating);
            now = ui.primary(update_text::UPDATE_NOW).clicked();
            ui.enabled(!updating);
            later = ui.button("Later").clicked();
            if updating {
                ui.progress("Updating", -1);
                ui.label("Updating…");
            }
        });
        if app.capture_update_later {
            ui.secondary(update_text::LATER);
        }
        match app.capture.update_outcome.clone() {
            Some(UpdateOutcome::Declined) => {
                ui.error_text(update_text::DECLINED);
            }
            Some(UpdateOutcome::Failed { message, details }) => {
                ui.error_text(format!("{}: {message}", update_text::NOT_UPDATED));
                let mut open = app.capture.update_details;
                ui.collapsing("Details", &mut open, |ui| {
                    ui.secondary(details.clone());
                });
                app.capture.update_details = open;
            }
            None => {}
        }
        if now {
            app.capture.update_outcome = None;
            start(app, "capture.update_server", json!({}), Kind::Update);
        }
        if later {
            app.capture_update_later = true;
        }
    });
}

/// Cloudflare's setup buttons (Settings > Advanced).
fn status_and_setup(app: &mut App, ui: &mut Ui, s: &Value) {
    let setup = s.at("setup");
    let status = app.capture.light.clone().unwrap_or(Value::Null);
    access_check(
        app,
        ui,
        &status,
        setup.at("complete").as_bool() == Some(true),
    );
    let complete = setup.at("complete").as_bool() == Some(true);
    ui.wrap(|ui| {
        if !complete {
            let label = if setup.at("started").as_bool() == Some(true) {
                "Continue setting up phone capture over Cloudflare…"
            } else {
                "Set up phone capture over Cloudflare…"
            };
            if ui.primary(label).clicked() {
                super::capture_setup::open(app, None);
            }
        } else {
            if ui.button("Change address or emails…").clicked() {
                super::capture_setup::open(app, Some(2));
            }
            ui.enabled(!busy(app, Kind::Repair))
                .tip("Sets the 30-day sign-in and the policies again (needs a Cloudflare API token)");
            if ui.button("Repair Cloudflare settings").clicked() {
                if setup.at("has_cloudflare_token").as_bool() == Some(true) {
                    start(app, "capture.setup_repair", json!({}), Kind::Repair);
                } else {
                    app.notify(
                        "Repairing needs a Cloudflare API token: paste one and press Check token, then Repair again.",
                    );
                    super::capture_setup::open(app, Some(1));
                }
            }
        }
        if setup.at("started").as_bool() == Some(true) && ui.button("Remove phone capture over Cloudflare…").clicked() {
            let list: Vec<String> =
                setup.list("resources").iter().filter_map(Value::as_str).map(str::to_owned).collect();
            let needs_token = setup.at("has_cloudflare_token").as_bool() != Some(true);
            app.capture.confirm_remove = Some((list, needs_token));
        }
    });
    if complete {
        ui.secondary(format!(
            "Cloudflare phone address: https://{}",
            setup.str("address")
        ));
    }
}

/// Check (mobile-capture.md revision 8): shown while the capture server reports a
/// refused Cloudflare sign-in, and with the last Check's answer (Create it again
/// when the Access application is gone).
pub(crate) fn access_check(app: &mut App, ui: &mut Ui, status: &Value, complete: bool) {
    let refused = !status.at("access_problem").is_null();
    let answer = app.capture.access.clone();
    if !complete || (!refused && answer.is_none()) {
        return;
    }
    let working = busy(app, Kind::Access);
    if let Some((text, missing)) = &answer {
        ui.label(text.clone());
        if *missing {
            ui.row(|ui| {
                ui.enabled(!working);
                if ui.primary("Create it again").clicked() {
                    start(
                        app,
                        "capture.check_access",
                        json!({"recreate": true}),
                        Kind::Access,
                    );
                }
            });
        }
    }
    if refused {
        ui.row(|ui| {
            ui.enabled(!working)
                .tip("Compare the Access application at Cloudflare with the capture server's settings and fix them (needs a Cloudflare API token)");
            if ui.primary("Check").clicked() {
                if has_token(app) {
                    app.capture.access = None;
                    start(app, "capture.check_access", json!({}), Kind::Access);
                } else {
                    app.notify(
                        "Checking needs a Cloudflare API token: paste one and press Check token, then Check again.",
                    );
                    super::capture_setup::open(app, Some(1));
                }
            }
            if working {
                ui.progress("Checking", -1);
            }
        });
    }
}

fn has_token(app: &mut App) -> bool {
    settings_json(app)
        .at("setup")
        .at("has_cloudflare_token")
        .as_bool()
        == Some(true)
}

/// Run Check in the background (opening setup on a complete setup), when a
/// Cloudflare token is stored.
pub(crate) fn check_access_quietly(app: &mut App) {
    if has_token(app) && !busy(app, Kind::Access) {
        start(app, "capture.check_access", json!({}), Kind::Access);
    }
}

/// The status light for the top bar, while phone capture is set up.
pub(crate) fn top_light(app: &App) -> Option<Value> {
    if !app.capture.set_up {
        return None;
    }
    Some(app.capture.light.clone().unwrap_or_else(|| {
        json!({"state": "grey", "text": "Checking phone capture…",
               "accessible_name": "Phone capture status: Checking phone capture…"})
    }))
}

/// "The built server can't be compared with the running one. Update anyway?"
fn update_dialog(app: &mut App, ui: &mut Ui) {
    if !app.capture.confirm_update {
        return;
    }
    let mut go = false;
    let mut cancel = ui.dialog_closed("capture-update") || ui.key("escape");
    ui.dialog("capture-update", "Update server?", 480, |ui| {
        ui.label("The built server can't be compared with the running one. Update anyway?");
        ui.actions(|ui| {
            cancel |= ui.button("Cancel").clicked();
            go = ui.primary("Update").clicked();
        });
    });
    if go {
        app.capture.confirm_update = false;
        start(
            app,
            "capture.update_server",
            json!({"force": true}),
            Kind::Update,
        );
    } else if cancel {
        app.capture.confirm_update = false;
    }
}

/// The removal confirmation.
fn remove_dialog(app: &mut App, ui: &mut Ui) {
    let Some((list, needs_token)) = app.capture.confirm_remove.clone() else {
        return;
    };
    let mut go = false;
    let mut cancel = ui.dialog_closed("capture-remove") || ui.key("escape");
    let working = busy(app, Kind::Remove);
    ui.dialog("capture-remove", "Remove phone capture?", 520, |ui| {
        ui.label("MoneyKeep will delete these, then forget the server address:");
        for item in &list {
            ui.label(format!("• {item}"));
        }
        if needs_token {
            ui.label("A Cloudflare API token is needed (the one used for setup was deleted):");
            ui.width(320);
            ui.password("Cloudflare API token", &mut app.capture.remove_token);
        }
        ui.actions(|ui| {
            if working {
                ui.progress("Removing", -1);
            }
            cancel |= ui.button("Cancel").clicked();
            let ready = !needs_token || !app.capture.remove_token.trim().is_empty();
            ui.enabled(ready && !working);
            go = ui.primary("Remove").clicked();
        });
    });
    if go {
        let token = std::mem::take(&mut app.capture.remove_token);
        let args = if token.trim().is_empty() {
            json!({})
        } else {
            json!({"token": token.trim()})
        };
        start(app, "capture.setup_remove", args, Kind::Remove);
        app.capture.confirm_remove = None;
    } else if cancel {
        app.capture.confirm_remove = None;
        app.capture.remove_token.clear();
    }
}

/// Revision 16: a removal that did not finish lists each failed step with its
/// reason, what is left and where to remove it by hand, and offers Retry undo.
fn undo_dialog(app: &mut App, ui: &mut Ui) {
    let Some(v) = app.capture.undo.clone() else {
        return;
    };
    let needs_token = !has_token(app);
    let working = busy(app, Kind::Remove);
    let mut retry = false;
    let mut close = ui.dialog_closed("capture-undo") || ui.key("escape");
    ui.dialog("capture-undo", "Remove phone capture", 620, |ui| {
        ui.bold();
        ui.label(v.str("text"));
        ui.label("These could not be removed:");
        for f in v.list("failed") {
            ui.error_text(format!("• {} — {}", f.str("what"), f.str("reason")));
        }
        let left = v.list("left");
        if !left.is_empty() {
            ui.label("Still left, and where to remove it by hand:");
            for l in left {
                ui.label(format!("• {} — {}", l.str("what"), l.str("where")));
            }
        }
        if needs_token {
            ui.label("A Cloudflare API token is needed to try again:");
            ui.width(320);
            ui.password("Cloudflare API token", &mut app.capture.undo_token);
        }
        ui.actions(|ui| {
            if working {
                ui.progress("Removing", -1);
            }
            close |= ui.button("Close").clicked();
            let ready = !needs_token || !app.capture.undo_token.trim().is_empty();
            ui.enabled(ready && !working);
            retry = ui.primary("Retry undo").clicked();
        });
    });
    if retry {
        let token = std::mem::take(&mut app.capture.undo_token);
        let args = if token.trim().is_empty() {
            json!({})
        } else {
            json!({"token": token.trim()})
        };
        start(app, "capture.setup_remove", args, Kind::Remove);
    } else if close {
        app.capture.undo = None;
        app.capture.undo_token.clear();
    }
}

/// "2026-10-23" of Unix seconds ("—" when unknown).
pub(crate) fn epoch_date(secs: Option<i64>) -> String {
    match secs {
        Some(s) => crate::fmt::date(&mk_capture_api::format_date(s.div_euclid(86_400))),
        None => "—".into(),
    }
}

/// The code with a space after its fourth character.
pub(crate) fn spaced(code: &str) -> String {
    format!(
        "{} {}",
        code.get(..4).unwrap_or(code),
        code.get(4..).unwrap_or("")
    )
}

/// The pairing code pop-up, the revoke confirmation, the setup dialog and the
/// removal confirmation.
pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    super::capture_wifi::poll(app);
    super::capture_wifi::dialogs(app, ui);
    super::capture_wifi::firewall_dialog(app, ui);
    super::capture_setup::dialog(app, ui);
    remove_dialog(app, ui);
    undo_dialog(app, ui);
    super::capture_start_over::dialog(app, ui);
    super::capture_mac::dialog(app, ui);
    update_dialog(app, ui);
    pair_choice_dialog(app, ui);
    pair_dialog(app, ui);
    revoke_dialog(app, ui);
}

/// The way a phone pairs, offered when both setups exist (app-desktop revision 39).
pub(crate) const PAIR_CHOICE_TITLE: &str = "How will this phone send expenses?";
pub(crate) const PAIR_WIFI: &str = "Wi-Fi at home (recommended)";
pub(crate) const PAIR_WIFI_HELP: &str = "Sends when the phone is on the same network as this computer. Faster, and works without the internet.";
pub(crate) const PAIR_CLOUDFLARE: &str = "From anywhere (Cloudflare)";
pub(crate) const PAIR_CLOUDFLARE_HELP: &str = "Sends from any network through your Cloudflare address. Use this if the phone is often away from home.";
pub(crate) const PAIR_CHOICE_NOTE: &str =
    "A phone pairs with one of them. You can pair it again later to switch.";

/// "How will this phone send expenses?": each choice opens its setup's QR flow.
fn pair_choice_dialog(app: &mut App, ui: &mut Ui) {
    if !app.capture.pair_choice {
        return;
    }
    let mut close = ui.dialog_closed("capture-pair-choice") || ui.key("escape");
    let (mut wifi, mut cloudflare) = (false, false);
    ui.dialog("capture-pair-choice", PAIR_CHOICE_TITLE, 480, |ui| {
        wifi = ui.primary(PAIR_WIFI).clicked();
        ui.secondary(PAIR_WIFI_HELP);
        cloudflare = ui.button(PAIR_CLOUDFLARE).clicked();
        ui.secondary(PAIR_CLOUDFLARE_HELP);
        ui.separator();
        ui.label(PAIR_CHOICE_NOTE);
        ui.actions(|ui| {
            close |= ui.button("Cancel").clicked();
        });
    });
    if wifi {
        app.capture.pair_choice = false;
        super::capture_wifi::open_pairing(app);
    } else if cloudflare {
        app.capture.pair_choice = false;
        super::capture_setup::open_pairing(app);
    } else if close {
        app.capture.pair_choice = false;
    }
}

/// "Can't scan?" and the pairing code to type (app-desktop revision 38), and under
/// it that the phone will also ask for the Capture PIN (mobile-capture.md revision
/// 15) when it will: always over Wi-Fi, over Cloudflare only when a Capture PIN is
/// set (`pin`). Used by every pairing window (Wi-Fi and Cloudflare).
pub(crate) fn cant_scan(ui: &mut Ui, code: &str, pin: bool) {
    ui.bold();
    ui.label("Can't scan?");
    ui.label("Type this code in the phone's capture app instead:");
    ui.named(format!("Pairing code {code}"));
    ui.mono();
    ui.big_heading(spaced(code));
    if pin {
        ui.label(mk_capture_api::msg::PHONE_ASKS_PIN);
    }
}

/// Whether a Capture PIN is set (pairing over Cloudflare asks for it only then).
pub(crate) fn pin_set(app: &mut App) -> bool {
    settings_json(app).at("wifi").at("pin_set").as_bool() == Some(true)
}

/// Pair a phone…: with both Wi-Fi capture and Cloudflare capture set up, first the
/// question "How will this phone send expenses?" (app-desktop revision 39); else the
/// Wi-Fi QR flow when Wi-Fi capture is set up, or the Cloudflare setup's QR step; a
/// server set up by hand gets the same QR code and typed code in a small dialog.
pub(crate) fn pair_a_phone(app: &mut App) {
    let s = settings_json(app);
    let wifi = s.at("wifi").at("installed").as_bool() == Some(true);
    let cloudflare = s.at("setup").at("complete").as_bool() == Some(true);
    if wifi && cloudflare {
        app.capture.pair_choice = true;
    } else if wifi {
        super::capture_wifi::open_pairing(app);
    } else if cloudflare {
        super::capture_setup::open_pairing(app);
    } else {
        start(app, "capture.pair_code", json!({}), Kind::Pair);
    }
}

fn pair_dialog(app: &mut App, ui: &mut Ui) {
    let Some(p) = app.capture.pair.take() else {
        return;
    };
    let left = crate::layout::remaining_secs(app.now, p.until);
    let pin = pin_set(app);
    let mut close = ui.dialog_closed("capture-pair") || ui.key("escape");
    let mut renew = false;
    ui.dialog("capture-pair", "Pair a phone", 480, |ui| {
        match &p.client_url {
            Some(u) => {
                let url = mk_capture_api::pair_url(u, &p.code);
                ui.label("Scan this with your phone's camera.");
                match super::capture_setup::qr_for(app, &url) {
                    Some(pic) => {
                        let w = i32::try_from(pic.width).unwrap_or(200);
                        ui.picture(&format!("QR code for {url}"), pic, w);
                    }
                    None => {
                        ui.error_text("The QR code could not be made; type the code below instead.")
                    }
                }
                cant_scan(ui, &p.code, pin);
                ui.secondary(format!("On the phone, open {u} and enter the code."));
            }
            None => {
                ui.label("On your phone, open the capture page and enter this code:");
                ui.named(format!("Pairing code {}", p.code));
                ui.mono();
                ui.big_heading(spaced(&p.code));
                if pin {
                    ui.label(mk_capture_api::msg::PHONE_ASKS_PIN);
                }
            }
        }
        if left > 0 {
            ui.label(format!("Expires in {}:{:02}", left / 60, left % 60));
        } else {
            ui.error_text("This code has expired. Create a new one.");
        }
        ui.actions(|ui| {
            if left == 0 && ui.button("New code").clicked() {
                renew = true;
            }
            if ui.primary("Done").clicked() {
                close = true;
            }
        });
    });
    if renew {
        start(app, "capture.pair_code", json!({}), Kind::Pair);
        close = true;
    }
    if close {
        app.capture.devices_gen = 0;
    } else if app.capture.pair.is_none() {
        app.capture.pair = Some(p);
    }
}

fn revoke_dialog(app: &mut App, ui: &mut Ui) {
    let Some((id, name)) = app.capture.confirm_revoke.take() else {
        return;
    };
    let mut go = false;
    let mut cancel = ui.dialog_closed("capture-revoke") || ui.key("escape");
    ui.dialog("capture-revoke", &format!("Revoke {name}?"), 480, |ui| {
        ui.label("The phone can no longer send expenses. Anything waiting on it stays there until you pair it again.");
        ui.actions(|ui| {
            cancel |= ui.button("Cancel").clicked();
            go = ui.primary("Revoke").clicked();
        });
    });
    if go {
        start(
            app,
            "capture.revoke_device",
            json!({"device": id}),
            Kind::Revoke,
        );
    } else if !cancel {
        app.capture.confirm_revoke = Some((id, name));
    }
}
