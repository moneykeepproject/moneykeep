//! Phone capture setup dialog and status light (app-desktop.md revision 29,
//! mobile-capture.md revision 2 sections B and C).
//!
//! The dialog shows the numbered steps (check marks on the done ones), the current
//! step, Back / Next, and a collapsible Details list of the Cloudflare calls.
//! Closing it keeps its place; opening it again resumes there. Every step's work is
//! a registry command run in the background.

use std::rc::Rc;

use serde_json::{Value, json};

use super::capture::{Kind, busy, settings_json, start};
use super::form::set;
use super::ui::{Column, Picture, Tone, Ui, col};
use super::util::Json;
use super::{App, AppEvent};

pub(crate) const STEPS: [&str; 8] = [
    "Before you start",
    "Connect to Cloudflare",
    "Address and sign-in",
    "Where the tunnel runs",
    "Install",
    "Check",
    "Pair your phone",
    "Finish",
];

/// The permissions the API token needs, as Cloudflare's token page names them.
pub(crate) const PERMISSIONS: [&str; 6] = [
    "Account · Cloudflare Tunnel · Edit",
    "Account · Access: Apps and Policies · Edit",
    "Account · Access: Service Tokens · Edit",
    "Account · Access: Organizations, Identity Providers, and Groups · Read",
    "Zone · DNS · Edit (for the chosen zone)",
    "Zone · Zone · Read",
];
pub(crate) const TOKEN_PAGE: &str = "https://dash.cloudflare.com/profile/api-tokens";
const DELETE_TOKEN: &str = "Delete Cloudflare token — each change requires a new token";

/// The setup dialog's state (kept while closed, so it resumes).
#[derive(Debug)]
pub(crate) struct SetupDialog {
    pub open: bool,
    pub step: usize,
    pub done: [bool; 8],
    token: String,
    check: Option<Value>,
    subdomain: String,
    emails: String,
    /// Empty: a new tunnel; else an existing tunnel's id.
    tunnel: String,
    /// `this` or `lan`.
    device: String,
    lan: String,
    plan: Option<Value>,
    error: Option<String>,
    status: Option<Value>,
    /// (code, clock time it expires).
    pair: Option<(String, f64)>,
    address: String,
    delete_token: bool,
    finished: Option<bool>,
    details_open: bool,
    /// The QR code picture for (text).
    qr: Option<(String, Option<Rc<Picture>>)>,
    /// Revision 16: a cloudflared service here runs a deleted tunnel (Replace it
    /// preselected) or a tunnel in another account (revision 16a: Leave it
    /// preselected): the owner's choice, once made.
    replace: Option<bool>,
}

impl SetupDialog {
    fn is_done(&self, i: usize) -> bool {
        self.done.get(i).copied().unwrap_or(false)
    }
    fn mark_done(&mut self, i: usize) {
        if let Some(d) = self.done.get_mut(i) {
            *d = true;
        }
    }
}

impl Default for SetupDialog {
    fn default() -> Self {
        Self {
            open: false,
            step: 0,
            done: [false; 8],
            token: String::new(),
            check: None,
            subdomain: "capture".into(),
            emails: String::new(),
            tunnel: String::new(),
            device: "this".into(),
            lan: String::new(),
            plan: None,
            error: None,
            status: None,
            pair: None,
            address: String::new(),
            delete_token: true,
            finished: None,
            details_open: false,
            qr: None,
            replace: None,
        }
    }
}

/// Open the dialog: where it stopped, or at `step` for a later change.
pub(crate) fn open(app: &mut App, step: Option<usize>) {
    let s = settings_json(app);
    let setup = s.at("setup").clone();
    let d = &mut app.capture.setup;
    if d.address.is_empty() {
        d.address = setup.str("address").to_owned();
    }
    if d.emails.is_empty() {
        d.emails = setup
            .list("emails")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ");
    }
    if let Some(sub) = d.address.split('.').next().filter(|x| !x.is_empty()) {
        d.subdomain = sub.to_owned();
    }
    let complete = setup.at("complete").as_bool() == Some(true);
    let reopened = complete && !d.open;
    if complete && step.is_none() && !d.open {
        d.done = [true, true, true, true, true, false, false, false];
        d.step = 5;
    }
    if let Some(s) = step {
        // A change needs a token: ask for one when none is stored.
        d.step = if setup.at("has_cloudflare_token").as_bool() == Some(true) || d.check.is_some() {
            s
        } else {
            1
        };
        d.finished = None;
    }
    d.error = None;
    d.open = true;
    // Revision 8: reopening a complete setup runs Check in the background.
    if reopened {
        super::capture::check_access_quietly(app);
    }
}

/// Open the dialog at "Pair your phone" with a fresh QR code.
pub(crate) fn open_pairing(app: &mut App) {
    open(app, Some(6));
    let d = &mut app.capture.setup;
    d.step = 6;
    d.pair = None;
    start(app, "capture.pair_code", json!({}), Kind::QrCode);
}

/// A background setup job ended.
pub(crate) fn finished(app: &mut App, kind: Kind, out: Result<Value, crate::UiError>) {
    let now = app.now;
    let d = &mut app.capture.setup;
    match (kind, out) {
        (Kind::Check, Ok(v)) => {
            d.token.clear();
            d.error = None;
            let ok = v.at("ok").as_bool() == Some(true);
            if ok {
                d.mark_done(1);
            } else if let Some(x) = d.done.get_mut(1) {
                *x = false;
            }
            if d.tunnel.is_empty() && v.str("suggest") == "existing" {
                // Revision 16: the tunnel the cloudflared service here runs.
                let listed = v.list("tunnels");
                let pick = listed
                    .iter()
                    .find(|t| t.str("id") == v.str("suggest_tunnel"))
                    .or_else(|| listed.first());
                if let Some(t) = pick {
                    d.tunnel = t.str("id").to_owned();
                }
            }
            if d.lan.is_empty() {
                d.lan = v
                    .at("host")
                    .list("lan_addresses")
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
            }
            d.check = Some(v);
        }
        (Kind::Plan, Ok(v)) => {
            d.error = None;
            d.address = v.str("address").to_owned();
            d.mark_done(2);
            d.mark_done(3);
            d.plan = Some(v);
            d.step = 4;
        }
        (Kind::Apply, Ok(v)) => {
            d.error = None;
            d.address = v.str("address").to_owned();
            d.mark_done(4);
            d.step = 5;
            app.generation += 1;
            app.capture.status_gen = 0;
            start(
                app,
                "capture.status",
                json!({}),
                Kind::Status { dialog: true },
            );
        }
        (Kind::Status { dialog: true }, Ok(v)) => {
            if matches!(v.str("state"), "green" | "yellow") {
                d.mark_done(5);
            }
            d.status = Some(v.clone());
            app.capture.light = Some(v);
        }
        (Kind::QrCode, Ok(v)) => {
            d.error = None;
            let secs = v.at("expires_in").as_i64().unwrap_or(600);
            d.pair = Some((v.str("code").to_owned(), crate::layout::add_secs(now, secs)));
            d.mark_done(6);
        }
        (Kind::Check | Kind::Plan | Kind::Apply | Kind::QrCode, Err(e)) => {
            d.error = Some(e.display());
        }
        (Kind::Status { dialog: true }, Err(e)) => d.error = Some(e.display()),
        _ => {}
    }
}

fn step_ready(d: &SetupDialog) -> bool {
    match d.step {
        1 => d.is_done(1),
        2 => !d.subdomain.trim().is_empty() && !emails(&d.emails).is_empty(),
        4 => d.is_done(4),
        5 => d.is_done(5),
        _ => true,
    }
}

fn emails(text: &str) -> Vec<String> {
    text.split([',', ';', ' ', '\n'])
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_owned)
        .collect()
}

fn plan_args(d: &SetupDialog) -> Value {
    let mut a = json!({"subdomain": d.subdomain.trim(), "emails": emails(&d.emails)});
    if !d.tunnel.is_empty() {
        set(&mut a, "tunnel", json!(d.tunnel));
        set(&mut a, "device", json!(d.device));
        if d.device == "lan" && !d.lan.trim().is_empty() {
            set(&mut a, "lan_address", json!(d.lan.trim()));
        }
    }
    if let Some(r) = d.replace {
        set(&mut a, "replace_service", json!(r));
    }
    a
}

/// Work started when a step is entered.
fn entered(app: &mut App, step: usize) {
    match step {
        5 if !busy(app, Kind::Status { dialog: true }) => {
            start(
                app,
                "capture.status",
                json!({}),
                Kind::Status { dialog: true },
            );
        }
        6 if app.capture.setup.pair.is_none() && !busy(app, Kind::QrCode) => {
            start(app, "capture.pair_code", json!({}), Kind::QrCode);
        }
        _ => {}
    }
}

/// The setup dialog, when open.
pub(crate) fn dialog(app: &mut App, ui: &mut Ui) {
    if !app.capture.setup.open {
        return;
    }
    let details = settings_json(app).at("setup").list("details").to_vec();
    let mut go: Option<usize> = None;
    let mut close = ui.dialog_closed("capture-setup") || ui.key("escape");
    ui.dialog("capture-setup", "Set up phone capture", 820, |ui| {
        // The steps, with a check mark on the done ones.
        ui.wrap(|ui| {
            let d = &app.capture.setup;
            for (i, name) in STEPS.iter().enumerate() {
                let selected = d.step == i;
                let reachable =
                    i <= d.step || d.done.get(i.saturating_sub(1)).copied().unwrap_or(true);
                let done = d.is_done(i);
                ui.named(format!(
                    "Step {}: {name}{}",
                    i + 1,
                    if done { ", done" } else { "" }
                ));
                ui.enabled(reachable);
                if done {
                    ui.icon("check");
                }
                if ui.tab(format!("{}. {name}", i + 1), selected).clicked() && !selected {
                    go = Some(i);
                }
            }
        });
        ui.separator();
        step_content(app, ui);
        ui.actions(|ui| {
            let step = app.capture.setup.step;
            if step > 0 && step < 7 && ui.button("Back").clicked() {
                go = Some(step - 1);
            }
            ui.spacer();
            if ui.button("Close").clicked() {
                close = true;
            }
            if step < 7 {
                let working = busy(app, Kind::Plan);
                if working {
                    ui.progress("Planning", -1);
                }
                ui.enabled(step_ready(&app.capture.setup) && !working);
                if ui.primary("Next").clicked() {
                    if step == 3 {
                        app.capture.setup.error = None;
                        let args = plan_args(&app.capture.setup);
                        start(app, "capture.setup_plan", args, Kind::Plan);
                    } else {
                        if matches!(step, 0 | 2 | 6) {
                            app.capture.setup.mark_done(step);
                        }
                        go = Some(step + 1);
                    }
                }
            }
        });
        let mut open = app.capture.setup.details_open;
        ui.collapsing("Details", &mut open, |ui| {
            if details.is_empty() {
                ui.secondary("No Cloudflare calls yet.");
            }
            let cols = vec![
                Column {
                    width: 80,
                    ..col("Method")
                },
                Column {
                    weight: 3,
                    ..col("Path")
                },
                Column {
                    weight: 1,
                    ..col("Result")
                },
            ];
            ui.table("", cols, |ui| {
                for (i, d) in details.iter().enumerate() {
                    ui.table_row(&format!("call{i}"), false, false, |ui| {
                        ui.label(d.str("method"));
                        ui.label(d.str("path"));
                        ui.label(d.str("result"));
                    });
                }
            });
        });
        app.capture.setup.details_open = open;
    });
    if close {
        app.capture.setup.open = false;
    }
    if let Some(step) = go {
        app.capture.setup.error = None;
        app.capture.setup.step = step;
        entered(app, step);
    }
}

fn error_line(app: &mut App, ui: &mut Ui) {
    if let Some(e) = app.capture.setup.error.clone() {
        ui.error_text(e);
        super::capture_start_over::stuck(app, ui);
    }
}

fn step_content(app: &mut App, ui: &mut Ui) {
    let step = app.capture.setup.step;
    ui.heading(STEPS.get(step).copied().unwrap_or(""));
    match step {
        0 => before_you_start(app, ui),
        1 => connect(app, ui),
        2 => address(app, ui),
        3 => tunnel(app, ui),
        4 => install(app, ui),
        5 => check(app, ui),
        6 => pair(app, ui),
        _ => finish(app, ui),
    }
}

fn before_you_start(app: &mut App, ui: &mut Ui) {
    ui.label("You need:");
    ui.label("• your domain on Cloudflare;");
    ui.label("• Zero Trust turned on once in the Cloudflare dashboard (choose a team name; the free plan is enough);");
    ui.label("• an API token created with these permissions:");
    for p in PERMISSIONS {
        ui.label(format!("    • {p}"));
    }
    ui.row(|ui| {
        if ui.button("Copy list").clicked() {
            app.copy(PERMISSIONS.join("\n"));
            app.notify("The permission list was copied.");
        }
        if ui.button("Open Cloudflare token page").clicked() {
            app.emit(AppEvent::OpenLink {
                url: TOKEN_PAGE.to_owned(),
            });
        }
    });
}

fn connect(app: &mut App, ui: &mut Ui) {
    let stored = settings_json(app)
        .at("setup")
        .at("has_cloudflare_token")
        .as_bool()
        == Some(true);
    ui.label("Paste the API token you created:");
    let hint = if stored {
        "A token is stored — paste a new one to replace it"
    } else {
        "Cloudflare API token"
    };
    ui.row(|ui| {
        ui.width(380).hint(hint);
        ui.password("Cloudflare API token", &mut app.capture.setup.token);
    });
    let checking = busy(app, Kind::Check);
    ui.row(|ui| {
        let can = !checking && (stored || !app.capture.setup.token.trim().is_empty());
        ui.enabled(can);
        if ui.primary("Check token").clicked() {
            let t = app.capture.setup.token.trim().to_owned();
            let args = if t.is_empty() {
                json!({})
            } else {
                json!({"token": t})
            };
            app.capture.setup.error = None;
            start(app, "capture.setup_check_token", args, Kind::Check);
        }
        if checking {
            ui.progress("Checking", -1);
        }
    });
    let Some(v) = app.capture.setup.check.clone() else {
        error_line(app, ui);
        return;
    };
    let pick = |ui: &mut Ui, label: &str, list: &[Value], current: &Value| -> Option<String> {
        ui.form(label, |ui| {
            if list.len() > 1 {
                let names: Vec<String> = list.iter().map(|a| a.str("name").to_owned()).collect();
                let mut idx = list
                    .iter()
                    .position(|a| a.str("id") == current.str("id"))
                    .unwrap_or(usize::MAX);
                ui.width(260);
                if ui.combo(label, &mut idx, &names).changed() {
                    return list.get(idx).map(|a| a.str("id").to_owned());
                }
            } else {
                ui.bold();
                ui.label(current.str("name"));
            }
            None
        })
    };
    let acc = ui.group(|ui| {
        let acc = pick(ui, "Account", v.list("accounts"), v.at("account"));
        let zone = pick(ui, "Domain", v.list("zones"), v.at("zone"));
        (acc, zone)
    });
    if let Some(acc) = acc.0 {
        start(
            app,
            "capture.setup_check_token",
            json!({"account": acc}),
            Kind::Check,
        );
    }
    if let Some(zone) = acc.1 {
        start(
            app,
            "capture.setup_check_token",
            json!({"account": v.at("account").str("id"), "zone": zone}),
            Kind::Check,
        );
    }
    let missing = v.list("missing");
    if missing.is_empty() {
        ui.row(|ui| {
            ui.icon("check");
            ui.tone(Tone::Success);
            ui.label("This token has every permission needed.");
        });
    } else {
        ui.error_text("The token is missing these permissions:");
        for m in missing {
            ui.error_text(format!("    {}", m.as_str().unwrap_or("")));
        }
        ui.secondary("Edit the token at Cloudflare, then press Check token again.");
    }
    error_line(app, ui);
}

fn zone_name(app: &App) -> String {
    app.capture
        .setup
        .check
        .as_ref()
        .map(|c| c.at("zone").str("name").to_owned())
        .unwrap_or_default()
}

fn address(app: &mut App, ui: &mut Ui) {
    let zone = zone_name(app);
    ui.group(|ui| {
        ui.form("Subdomain", |ui| {
            ui.width(160);
            ui.field("Subdomain", &mut app.capture.setup.subdomain);
            ui.label(format!(".{zone}"));
        });
        ui.form("Address", |ui| {
            ui.bold();
            ui.label(format!("https://{}.{zone}", app.capture.setup.subdomain.trim()));
        });
        ui.form("Email addresses allowed", |ui| {
            ui.width(380);
            ui.field("Email addresses allowed", &mut app.capture.setup.emails);
        });
        ui.form("", |ui| {
            ui.secondary("People who may sign in on the phone (Cloudflare emails them a code). Separate several with commas.");
        });
    });
    ui.secondary("The address must not already exist in your DNS unless MoneyKeep made it.");
}

fn tunnel(app: &mut App, ui: &mut Ui) {
    let tunnels: Vec<Value> = app
        .capture
        .setup
        .check
        .as_ref()
        .map(|c| c.list("tunnels").to_vec())
        .unwrap_or_default();
    let d = &mut app.capture.setup;
    ui.radio(
        &mut d.tunnel,
        String::new(),
        "Create a new tunnel on this computer",
    );
    for t in &tunnels {
        let label = format!("Add to my existing tunnel {}", t.str("name"));
        ui.radio(&mut d.tunnel, t.str("id").to_owned(), &label);
    }
    if !d.tunnel.is_empty() {
        ui.label("Where does that tunnel run?");
        ui.radio(&mut d.device, "this".to_owned(), "This computer");
        ui.radio(
            &mut d.device,
            "lan".to_owned(),
            "Another device on my network (e.g. a router or server)",
        );
        if d.device == "lan" {
            ui.form("This computer's network address", |ui| {
                ui.width(160);
                ui.field("This computer's network address", &mut d.lan);
            });
            ui.secondary("The capture server will listen on this address only, and the tunnel will send requests to it on port 8787.");
        }
    }
    error_line(app, ui);
}

fn install(app: &mut App, ui: &mut Ui) {
    let plan = app.capture.setup.plan.clone().unwrap_or(Value::Null);
    ui.label(format!("Address: https://{}", plan.str("address")));
    ui.label("MoneyKeep will:");
    for a in plan.list("actions") {
        ui.label(format!("• {}", a.as_str().unwrap_or("")));
    }
    // Revision 16: a cloudflared service here runs a tunnel that is gone (Replace
    // it preselected); revision 16a: one of another account (Leave it preselected,
    // Replace it with the warning).
    let service = plan.at("service");
    let state = service.str("state");
    if state == "deleted" || state == "other_account" {
        ui.label(service.str("message"));
        let before = service
            .at("replace")
            .as_bool()
            .unwrap_or(state == "deleted");
        let mut choice = before;
        let planning = busy(app, Kind::Plan) || busy(app, Kind::Apply);
        ui.enabled(!planning);
        ui.radio(&mut choice, true, "Replace it");
        let warning = service.str("warning");
        if !warning.is_empty() {
            ui.secondary(warning);
        }
        ui.enabled(!planning);
        ui.radio(&mut choice, false, "Leave it");
        if choice != before {
            app.capture.setup.replace = Some(choice);
            app.capture.setup.error = None;
            let args = plan_args(&app.capture.setup);
            start(app, "capture.setup_plan", args, Kind::Plan);
        }
    }
    let problems = plan.list("problems");
    for p in problems {
        ui.error_text(p.as_str().unwrap_or(""));
    }
    // macos-release.md C3: no password on a Mac.
    if super::capture_mac::is_mac(app) {
        ui.secondary("On a Mac the services are installed for you alone; no password is asked.");
    } else {
        ui.secondary("Windows asks once for permission to install the services.");
    }
    // macos-release.md C6: after installing cloudflared (or fixing another
    // problem), plan again.
    let mut again = false;
    if !problems.is_empty() {
        ui.row(|ui| {
            ui.enabled(!busy(app, Kind::Plan));
            again = ui.button("Check again").clicked();
        });
    }
    if again {
        app.capture.setup.error = None;
        let args = plan_args(&app.capture.setup);
        start(app, "capture.setup_plan", args, Kind::Plan);
    }
    let working = busy(app, Kind::Apply);
    let installed = app.capture.setup.is_done(4);
    ui.row(|ui| {
        let label = if app.capture.setup.error.is_some() {
            "Try again"
        } else {
            "Install"
        };
        ui.enabled(!working && problems.is_empty() && !installed);
        if ui.primary(label).clicked() {
            app.capture.setup.error = None;
            let args = plan_args(&app.capture.setup);
            start(app, "capture.setup_apply", args, Kind::Apply);
        }
        if working {
            ui.progress("Installing", -1);
            ui.label("Installing…");
        }
        if installed {
            ui.icon("check");
            ui.label("Installed.");
        }
    });
    error_line(app, ui);
}

fn check(app: &mut App, ui: &mut Ui) {
    match app.capture.setup.status.clone() {
        Some(v) => {
            ui.row(|ui| {
                ui.tip(tooltip(&v));
                ui.light(v.str("state"), 28, v.str("accessible_name"));
                ui.bold();
                ui.label(v.str("text"));
            });
            for p in v.list("problems").iter().skip(1) {
                ui.label(p.as_str().unwrap_or(""));
            }
            super::capture::access_check(app, ui, &v, true);
        }
        None => ui.label("Checking…"),
    }
    let working = busy(app, Kind::Status { dialog: true });
    ui.row(|ui| {
        ui.enabled(!working);
        if ui.primary("Check now").clicked() {
            start(
                app,
                "capture.status",
                json!({}),
                Kind::Status { dialog: true },
            );
        }
        if working {
            ui.progress("Checking", -1);
        }
    });
    error_line(app, ui);
}

fn pair(app: &mut App, ui: &mut Ui) {
    let address = app.capture.setup.address.clone();
    match app.capture.setup.pair.clone() {
        Some((code, until)) => {
            let url = mk_capture_api::pair_url(&address, &code);
            ui.label("Scan this with your phone's camera. The phone signs in with Cloudflare's email code once, then pairs.");
            match qr_picture(app, &url) {
                Some(pic) => {
                    let w = i32::try_from(pic.width).unwrap_or(200);
                    ui.picture(&format!("QR code for {url}"), pic, w);
                }
                None => {
                    ui.error_text("The QR code could not be made; type the code below instead.")
                }
            }
            let pin = super::capture::pin_set(app);
            super::capture::cant_scan(ui, &code, pin);
            let left = crate::layout::remaining_secs(app.now, until);
            if left > 0 {
                ui.label(format!("Expires in {}:{:02}", left / 60, left % 60));
            } else {
                ui.error_text("This code has expired.");
                if ui.row(|ui| ui.button("New code").clicked()) {
                    app.capture.setup.pair = None;
                    start(app, "capture.pair_code", json!({}), Kind::QrCode);
                }
            }
        }
        None => {
            if busy(app, Kind::QrCode) {
                ui.label("Making a pairing code…");
            } else if ui.row(|ui| ui.primary("Show QR code").clicked()) {
                start(app, "capture.pair_code", json!({}), Kind::QrCode);
            }
        }
    }
    error_line(app, ui);
}

fn finish(app: &mut App, ui: &mut Ui) {
    match app.capture.setup.finished {
        None => {
            ui.label(format!(
                "Phone capture is set up at https://{}.",
                app.capture.setup.address
            ));
            ui.checkbox(&mut app.capture.setup.delete_token, DELETE_TOKEN);
            ui.secondary(
                "Unticked, MoneyKeep keeps the token for later changes (address, emails, removing phone capture).",
            );
            if ui.row(|ui| ui.primary("Finish").clicked()) {
                let erase = app.capture.setup.delete_token;
                if erase && let Err(e) = app.run("capture.forget_cloudflare_token", json!({})) {
                    app.capture.setup.error = Some(e.display());
                    return;
                }
                app.capture.setup.mark_done(7);
                app.capture.setup.finished = Some(erase);
            }
        }
        Some(erased) => {
            if erased {
                ui.label("MoneyKeep erased its copy of the Cloudflare token. Revoke it at Cloudflare too:");
                if ui.link("Open Cloudflare token page").clicked() {
                    app.emit(AppEvent::OpenLink {
                        url: TOKEN_PAGE.to_owned(),
                    });
                }
            } else {
                ui.label("The Cloudflare token stays in MoneyKeep's secrets for later changes.");
            }
            if ui.row(|ui| ui.primary("Done").clicked()) {
                app.capture.setup = SetupDialog::default();
            }
        }
    }
    error_line(app, ui);
}

/// Unix seconds as local "date HH:MM".
fn when(secs: i64, offset: i64) -> String {
    let local = secs.saturating_add(offset);
    let day = local.div_euclid(86_400);
    let rem = local.rem_euclid(86_400);
    format!(
        "{} {:02}:{:02}",
        crate::fmt::date(&mk_capture_api::format_date(day)),
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// The light's tooltip: the state, other problems, last phone activity, items
/// waiting, last check.
pub(crate) fn tooltip(v: &Value) -> String {
    let mut lines = vec![v.str("text").to_owned()];
    for p in v.list("problems").iter().skip(1) {
        lines.push(p.as_str().unwrap_or("").to_owned());
    }
    let offset = v.at("utc_offset_secs").as_i64().unwrap_or(0);
    match v.at("last_activity").as_i64() {
        Some(t) => lines.push(format!("Last phone activity: {}", when(t, offset))),
        None if v.at("local_ok").as_bool() == Some(true) => {
            lines.push("Last phone activity: none yet".into())
        }
        None => {}
    }
    if let Some(n) = v.at("waiting").as_u64() {
        lines.push(format!("Waiting to be collected: {n}"));
    }
    if let Some(t) = v.at("checked_at").as_i64() {
        lines.push(format!("Last checked: {}", when(t, offset)));
    }
    if v.at("can_update").as_bool() == Some(true) {
        lines.push("Update server: Settings > Phone capture".into());
    } else if v.at("update_ready").as_bool() == Some(true) {
        lines.push("Update now: Settings > Phone capture".into());
    }
    lines.join("\n")
}

// ----- the QR code ----------------------------------------------------------------------------

/// The QR code's modules (row-major, `size` × `size`), error correction level M
/// (ISO/IEC 18004).
pub(crate) fn qr_matrix(text: &str) -> Option<(usize, Vec<bool>)> {
    let code = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium).ok()?;
    let n = code.size();
    let size = usize::try_from(n).ok()?;
    let mut out = Vec::with_capacity(size.saturating_mul(size));
    for y in 0..n {
        for x in 0..n {
            out.push(code.get_module(x, y));
        }
    }
    Some((size, out))
}

/// The QR code of `text` as a picture: 5 pixels a module, a four-module quiet zone.
/// The (cached) QR code picture of `text`, for other pairing dialogs.
pub(crate) fn qr_for(app: &mut App, text: &str) -> Option<Rc<Picture>> {
    qr_picture(app, text)
}

fn qr_picture(app: &mut App, text: &str) -> Option<Rc<Picture>> {
    if let Some((t, pic)) = &app.capture.setup.qr
        && t == text
    {
        return pic.clone();
    }
    let pic = make_qr(text);
    app.capture.setup.qr = Some((text.to_owned(), pic.clone()));
    pic
}

/// The QR code of `text` as a picture: 5 pixels a module, a four-module quiet zone.
pub(crate) fn make_qr(text: &str) -> Option<Rc<Picture>> {
    qr_matrix(text).and_then(|(size, cells)| {
        const MODULE: usize = 5;
        const QUIET: usize = 4;
        let side = (size + 2 * QUIET) * MODULE;
        let mut rgba = vec![255u8; side * side * 4];
        for (i, on) in cells.iter().enumerate() {
            if !*on {
                continue;
            }
            let (mx, my) = (i % size + QUIET, i / size + QUIET);
            for py in my * MODULE..(my + 1) * MODULE {
                for px in mx * MODULE..(mx + 1) * MODULE {
                    let at = (py * side + px) * 4;
                    if let Some(p) = rgba.get_mut(at..at + 3) {
                        p.fill(0);
                    }
                }
            }
        }
        let w = u32::try_from(side).ok()?;
        Some(Rc::new(Picture {
            width: w,
            height: w,
            rgba,
        }))
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    /// mobile-capture.md AT26 (desktop part): the QR code decodes to
    /// `https://<address>/#pair=<code>`.
    #[test]
    fn at26_qr_code_decodes_to_the_pairing_address() {
        let url = mk_capture_api::pair_url("capture.example.com", "ABCD2345");
        let (size, cells) = qr_matrix(&url).unwrap();
        // Render with a quiet zone, 4 pixels per module, and decode.
        let scale = 4;
        let quiet = 4;
        let px = (size + 2 * quiet) * scale;
        let mut img = rqrr::PreparedImage::prepare_from_greyscale(px, px, |x, y| {
            let (mx, my) = (x / scale, y / scale);
            let inside = mx >= quiet && my >= quiet && mx < size + quiet && my < size + quiet;
            if inside && cells[(my - quiet) * size + (mx - quiet)] {
                0
            } else {
                255
            }
        });
        let grids = img.detect_grids();
        assert_eq!(grids.len(), 1);
        let (_, content) = grids[0].decode().unwrap();
        assert_eq!(content, "https://capture.example.com/#pair=ABCD2345");
    }

    #[test]
    fn tooltip_lines() {
        let v = json!({"text": "Phone capture is working", "problems": [], "last_activity": 86_400 + 3600,
                       "waiting": 2, "checked_at": 86_400 + 7200, "utc_offset_secs": 0, "local_ok": true});
        let t = tooltip(&v);
        assert!(
            t.starts_with("Phone capture is working\nLast phone activity: "),
            "{t}"
        );
        assert!(
            t.contains("01:00")
                && t.contains("Waiting to be collected: 2")
                && t.contains("Last checked: ")
        );
    }
}
