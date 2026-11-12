//! Wi-Fi capture setup (mobile-capture.md revision 10): the normal way to set up
//! phone capture, on the home or office network, with no Cloudflare account.
//!
//! A guided dialog like the Cloudflare one: Address, Capture PIN, Firewall, Start,
//! Certificate on the phone (QR code 1) and Pair (QR code 2). It can be closed and
//! resumed, has Details and Help (the deployment guide, shown in the app), and the
//! Settings page offers Change address, Change PIN and Remove Wi-Fi capture. Every
//! step's work is a registry command run in the background.

use std::rc::Rc;

use serde_json::{Value, json};

use super::App;
use super::capture::{Kind, busy, settings_json, start};
use super::ui::{Picture, Tone, Ui};
use super::util::Json;

pub(crate) const STEPS: [&str; 6] = [
    "Address",
    "Capture PIN",
    "Firewall",
    "Start",
    "Certificate on the phone",
    "Pair",
];

/// The help: the deployment guide, the same text as `docs/deploy/phone-capture.md`.
pub(crate) const HELP: &str = include_str!("../../../../docs/deploy/phone-capture.md");

pub(crate) const WHY_FIXED: &str = "A phone keeps the capture app, its pairing and its unsent expenses for one address. If this computer's address changes, the phone sees a different app and must be set up again. So give this computer a fixed address first: a DHCP reservation in your router (or ask your network team on an office network).";
pub(crate) const PIN_WHY: &str = "Not your book password: the phone never needs the key to your book. The PIN only proves that a phone pairing on your network was allowed by you.";
pub(crate) const PUBLIC_WARNING: &str = "Windows treats this network as Public, so the firewall rule (Private and Domain networks only) will not let phones in. Change it: Settings > Network & internet > Wi-Fi (or Ethernet) > this network > Network profile type > Private network.";
/// Revision 12: the name field's label and the line under it.
pub(crate) const NAME_LABEL: &str = "Local name (optional — leave blank)";
pub(crate) const NAME_LINE: &str = "Blank is best: the IP address is used for the certificate and pairing. Use a name only if your router or DNS server answers for it.";
/// upgrade-resilience-1 U1.7: whenever an address is chosen.
pub(crate) fn reserve_tip(address: &str) -> String {
    format!(
        "Reserve {address} for this computer in your router (a DHCP reservation), so it never changes: a new address means installing the certificate on every phone again."
    )
}
/// U1.7: Change address, before saving.
pub(crate) const CERT_AGAIN: &str =
    "Every phone will need the new certificate installed again (Certificate QR code…).";
pub(crate) const CHANGE_WARNING: &str = "Before you change it, send everything in the phone's Outbox. Afterwards install the new certificate, open the new address on the phone, install the app again and pair it.";

pub(crate) const ANDROID_STEPS: [&str; 5] = [
    "Scan QR code 1 with the phone's camera and tap Download certificate (Android).",
    "Open Settings > Security (or Security & privacy) > More security settings > Encryption & credentials.",
    "Tap Install a certificate > CA certificate > Install anyway, and choose the downloaded file. The phone may ask for its screen lock.",
    "Check under Trusted credentials > User that \"MoneyKeep Capture CA\" is listed.",
    "Compare the fingerprint on the phone's page with the one here, then press Next.",
];
pub(crate) const IPHONE_STEPS: [&str; 5] = [
    "Scan QR code 1 with the Camera app; it opens in Safari. Tap Download certificate (iPhone or iPad), then Allow.",
    "Open Settings > Profile Downloaded (or General > VPN & Device Management) > Install, and enter the passcode.",
    "Open Settings > General > About > Certificate Trust Settings and turn on full trust for \"MoneyKeep Capture CA\".",
    "Compare the fingerprint on the phone's page with the one here.",
    "Press Next for the pairing QR code.",
];

/// The pairing code on show.
#[derive(Debug, Clone)]
struct Shown {
    code: String,
    until: f64,
    pair_url: String,
}

/// The dialog's state (kept while closed, so it resumes).
#[derive(Debug, Default)]
pub(crate) struct WifiDialog {
    pub open: bool,
    pub step: usize,
    done: [bool; 6],
    addresses: Option<Vec<Value>>,
    pub address: String,
    pub name: String,
    https: String,
    http: String,
    pin: String,
    pin2: String,
    error: Option<String>,
    result: Option<Value>,
    pair: Option<Shown>,
    /// Phones paired before this code (ids), to see the new one.
    before: Option<Vec<String>>,
    /// "Paired: <name>".
    pub paired: Option<String>,
    last_poll: Option<f64>,
    details_open: bool,
    pub help_open: bool,
    qr: Vec<(String, Option<Rc<Picture>>)>,
    /// Remove Wi-Fi capture is asked.
    pub confirm_remove: bool,
    /// Change address: open, its fields, and the answer.
    pub change: Option<(String, String)>,
    change_result: Option<Value>,
    /// Change PIN: open, and its two fields.
    pub pin_change: Option<(String, String)>,
    /// Revision 12: Start found that the typed name does not lead to the address
    /// (the words), and whether the user chose Use it anyway.
    name_warning: Option<String>,
    name_anyway: bool,
    /// The same for Change address.
    change_warning: Option<String>,
    change_anyway: bool,
    /// Revision 18: the firewall check was started after the app opened; its last
    /// answer; the question at start was asked (once); it is showing; the last
    /// Fix's outcome (ok, words).
    pub firewall_started: bool,
    pub firewall: Option<Value>,
    firewall_asked: bool,
    pub firewall_ask: bool,
    firewall_outcome: Option<(bool, String)>,
}

impl WifiDialog {
    fn is_done(&self, i: usize) -> bool {
        self.done.get(i).copied().unwrap_or(false)
    }
    fn mark(&mut self, i: usize) {
        if let Some(d) = self.done.get_mut(i) {
            *d = true;
        }
    }
}

/// The PIN as typed: an error, or `None` when it may be used.
pub(crate) fn pin_problem(pin: &str, again: &str) -> Option<&'static str> {
    let p = pin.trim();
    if !(6..=12).contains(&p.len()) || !p.bytes().all(|b| b.is_ascii_digit()) {
        Some("The Capture PIN is 6 to 12 digits.")
    } else if p != again.trim() {
        Some("The two PINs are not the same.")
    } else {
        None
    }
}

fn wifi(app: &mut App) -> Value {
    settings_json(app).at("wifi").clone()
}

/// Open the dialog where it stopped (or at `step`).
pub(crate) fn open(app: &mut App, step: Option<usize>) {
    let w = wifi(app);
    let d = &mut app.capture.wifi;
    if d.address.is_empty() {
        d.address = w.str("address").to_owned();
        d.name = w.str("name").to_owned();
    }
    if d.https.is_empty() {
        let p = w.at("https_port").as_u64().unwrap_or(0);
        d.https = if p == 0 {
            "14443".into()
        } else {
            p.to_string()
        };
        let p = w.at("http_port").as_u64().unwrap_or(0);
        d.http = if p == 0 {
            "14080".into()
        } else {
            p.to_string()
        };
    }
    let installed = w.at("installed").as_bool() == Some(true);
    if installed && !d.open {
        d.done = [true, true, true, true, false, false];
        d.step = 4;
    }
    if let Some(s) = step {
        d.step = s;
    }
    d.error = None;
    d.open = true;
    let s = d.step;
    entered(app, s);
}

/// Work started when a step is entered.
fn entered(app: &mut App, step: usize) {
    match step {
        0 if app.capture.wifi.addresses.is_none() && !busy(app, Kind::WifiAddresses) => {
            start(
                app,
                "capture.wifi_addresses",
                json!({}),
                Kind::WifiAddresses,
            );
        }
        5 if app.capture.wifi.pair.is_none() && !busy(app, Kind::WifiPair) => new_code(app),
        _ => {}
    }
}

/// Open the dialog at "Pair" with a fresh QR code (Settings → Phone capture → Paired
/// phones → Pair a phone…).
pub(crate) fn open_pairing(app: &mut App) {
    app.capture.wifi.pair = None;
    open(app, Some(5));
}

fn new_code(app: &mut App) {
    let before = app.capture.devices_ids().or_else(|| Some(Vec::new()));
    let d = &mut app.capture.wifi;
    d.pair = None;
    d.paired = None;
    d.before = before;
    start(app, "capture.wifi_pair_code", json!({}), Kind::WifiPair);
}

/// A background Wi-Fi job ended.
pub(crate) fn finished(app: &mut App, kind: Kind, out: Result<Value, crate::UiError>) {
    let now = app.now;
    let d = &mut app.capture.wifi;
    match (kind, out) {
        (Kind::WifiAddresses, Ok(v)) => {
            let list = v.list("addresses").to_vec();
            if d.address.is_empty()
                && let Some(first) = list.first()
            {
                d.address = first.str("address").to_owned();
            }
            d.addresses = Some(list);
        }
        (Kind::WifiApply, Ok(v)) => {
            d.error = None;
            d.pin.clear();
            d.pin2.clear();
            d.mark(1);
            d.mark(3);
            if v.at("answered").as_bool() != Some(true) {
                d.error = Some(format!(
                    "The server was started, but it did not answer on {} yet: {}. See Help > Troubleshooting.",
                    v.str("host"),
                    v.str("check_error")
                ));
            }
            d.result = Some(v);
            app.generation += 1;
            app.capture.status_gen = 0;
        }
        (Kind::WifiPin, Ok(_)) => {
            d.pin_change = None;
            app.notify("The Capture PIN was changed.");
        }
        (Kind::WifiPair, Ok(v)) => {
            d.error = None;
            let secs = v.at("expires_in").as_i64().unwrap_or(600);
            d.pair = Some(Shown {
                code: v.str("code").to_owned(),
                until: crate::layout::add_secs(now, secs),
                pair_url: v.str("pair_url").to_owned(),
            });
        }
        (Kind::WifiChange, Ok(v)) => {
            d.change_result = Some(v);
            d.qr.clear();
            d.pair = None;
            app.generation += 1;
            app.capture.status_gen = 0;
            app.capture.devices_gen = 0;
        }
        (Kind::WifiRemove, Ok(v)) => {
            *d = WifiDialog::default();
            let n = v.at("phones").as_u64().unwrap_or(0);
            app.notify(format!(
                "Wi-Fi capture was removed{}.",
                if n > 0 {
                    format!(" ({n} phone{} disconnected)", if n == 1 { "" } else { "s" })
                } else {
                    String::new()
                }
            ));
            app.generation += 1;
            app.capture.status_gen = 0;
            app.capture.devices_gen = 0;
        }
        (Kind::WifiPin, Err(e)) => {
            app.notify_error(&e);
        }
        // Revision 12: the name does not lead to the address. Step 1 says so; Next
        // waits for Use it anyway (or a blank name).
        (Kind::WifiApply, Err(e)) if e.code == "name_unresolved" => {
            d.error = None;
            d.name_warning = Some(e.display());
            d.name_anyway = false;
            d.step = 0;
        }
        (Kind::WifiChange, Err(e)) if e.code == "name_unresolved" => {
            d.error = None;
            d.change_warning = Some(e.display());
            d.change_anyway = false;
        }
        (_, Err(e)) => d.error = Some(e.display()),
        _ => {}
    }
}

/// While QR code 2 shows: notice the phone that pairs.
pub(crate) fn poll(app: &mut App) {
    let d = &app.capture.wifi;
    if !d.open || d.step != 5 || d.pair.is_none() || d.paired.is_some() {
        return;
    }
    if d.last_poll
        .is_some_and(|t| !crate::layout::elapsed(app.now, t, 3.0))
    {
        return;
    }
    if let (Some(before), Some(devices)) = (d.before.clone(), app.capture.devices_list())
        && let Some(new) = devices
            .iter()
            .find(|x| x.str("via") == "wifi" && !before.iter().any(|b| b == x.str("id")))
    {
        let name = new.str("name").to_owned();
        app.capture.wifi.paired = Some(name);
        app.capture.wifi.mark(5);
        return;
    }
    app.capture.wifi.last_poll = Some(app.now);
    if !busy(app, Kind::Devices) {
        start(app, "capture.devices", json!({}), Kind::Devices);
    }
}

fn qr(app: &mut App, text: &str) -> Option<Rc<Picture>> {
    if let Some((_, p)) = app.capture.wifi.qr.iter().find(|(t, _)| t == text) {
        return p.clone();
    }
    let pic = super::capture_setup::make_qr(text);
    app.capture.wifi.qr.push((text.to_owned(), pic.clone()));
    pic
}

fn qr_block(app: &mut App, ui: &mut Ui, label: &str, url: &str) {
    match qr(app, url) {
        Some(pic) => {
            let w = i32::try_from(pic.width).unwrap_or(200);
            ui.picture(&format!("{label}: {url}"), pic, w);
        }
        None => {
            ui.error_text("The QR code could not be made; type the address on the phone instead.")
        }
    }
    ui.mono();
    ui.label(url.to_owned());
}

fn step_ready(app: &mut App) -> bool {
    let d = &app.capture.wifi;
    match d.step {
        0 => !d.address.trim().is_empty() && (d.name_warning.is_none() || d.name_anyway),
        1 => {
            pin_problem(&d.pin, &d.pin2).is_none()
                || (d.pin.is_empty() && wifi(app).at("pin_set").as_bool() == Some(true))
        }
        3 => d.is_done(3),
        _ => true,
    }
}

/// The dialog, when open.
pub(crate) fn dialog(app: &mut App, ui: &mut Ui) {
    help_dialog(app, ui);
    // One dialog at a time: the help covers the setup until closed.
    if !app.capture.wifi.open || app.capture.wifi.help_open {
        return;
    }
    let w = wifi(app);
    let mut go: Option<usize> = None;
    let mut close = ui.dialog_closed("wifi-setup") || ui.key("escape");
    ui.dialog("wifi-setup", "Set up Wi-Fi capture", 860, |ui| {
        ui.wrap(|ui| {
            let d = &app.capture.wifi;
            for (i, name) in STEPS.iter().enumerate() {
                let selected = d.step == i;
                let reachable = i <= d.step || d.is_done(i.saturating_sub(1)) || i == 0;
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
        let step = app.capture.wifi.step;
        ui.heading(STEPS.get(step).copied().unwrap_or(""));
        match step {
            0 => address_step(app, ui),
            1 => pin_step(app, ui, &w),
            2 => firewall_step(app, ui),
            3 => start_step(app, ui),
            4 => certificate_step(app, ui, &w),
            _ => pair_step(app, ui, &w),
        }
        if let Some(e) = app.capture.wifi.error.clone() {
            ui.error_text(e);
            // mobile-capture.md revision 17: a way out from a failed setup.
            super::capture_start_over::stuck(app, ui);
        }
        ui.actions(|ui| {
            let step = app.capture.wifi.step;
            if step > 0 && ui.button("Back").clicked() {
                go = Some(step - 1);
            }
            if ui.button("Help").clicked() {
                app.capture.wifi.help_open = true;
            }
            ui.spacer();
            if ui.button("Close").clicked() {
                close = true;
            }
            if step < 5 {
                ui.enabled(step_ready(app));
                if ui.primary("Next").clicked() {
                    if matches!(step, 0 | 2 | 4) {
                        app.capture.wifi.mark(step);
                    }
                    if step == 1 {
                        app.capture.wifi.mark(1);
                    }
                    go = Some(step + 1);
                }
            } else if ui.primary("Done").clicked() {
                close = true;
            }
        });
        let mut open = app.capture.wifi.details_open;
        ui.collapsing("Details", &mut open, |ui| details(ui, &w));
        app.capture.wifi.details_open = open;
    });
    if close {
        app.capture.wifi.open = false;
        // A phone may have paired: list the phones again.
        app.capture.devices_gen = 0;
    }
    if let Some(step) = go {
        app.capture.wifi.error = None;
        app.capture.wifi.step = step;
        entered(app, step);
    }
}

fn details(ui: &mut Ui, w: &Value) {
    if w.str("address").is_empty() {
        ui.secondary("Nothing was made yet.");
        return;
    }
    let date = |k: &str| super::capture::epoch_date(w.at(k).as_i64().filter(|n| *n > 0));
    let rows = [
        ("Address", w.str("address").to_owned()),
        ("Local name", w.str("name").to_owned()),
        ("Adapter", w.str("adapter").to_owned()),
        (
            "MAC address (for a DHCP reservation)",
            w.str("mac").to_owned(),
        ),
        (
            "Ports",
            format!(
                "{} (HTTPS), {} (certificate page)",
                w.at("https_port"),
                w.at("http_port")
            ),
        ),
        (
            "Firewall rule",
            w.at("firewall_rule")
                .as_str()
                .unwrap_or("none added by MoneyKeep")
                .to_owned(),
        ),
        ("Certificate authority", w.str("ca_name").to_owned()),
        ("Fingerprint (SHA-256)", w.str("fingerprint").to_owned()),
        ("Certificate authority valid until", date("ca_not_after")),
        (
            "Server certificate",
            format!(
                "{} to {} (renewed automatically)",
                date("cert_not_before"),
                date("cert_not_after")
            ),
        ),
    ];
    ui.group(|ui| {
        for (k, v) in rows {
            if v.trim().is_empty() {
                continue;
            }
            ui.form(k, |ui| ui.label(v));
        }
    });
    for line in w.list("details") {
        ui.secondary(format!("• {}", line.as_str().unwrap_or("")));
    }
}

fn address_step(app: &mut App, ui: &mut Ui) {
    ui.label(WHY_FIXED);
    if ui.link("How do I make it fixed?").clicked() {
        app.capture.wifi.help_open = true;
    }
    let list = app.capture.wifi.addresses.clone();
    // A warning about the name holds for the address it was checked against.
    let before = app.capture.wifi.address.clone();
    match &list {
        None => ui.label("Looking up this computer's addresses…"),
        Some(l) if l.is_empty() => ui.secondary(
            "No home or office network address was found. Connect to the network, or type the address.",
        ),
        Some(l) => {
            ui.label("Choose this computer's address on the network your phone uses:");
            for a in l {
                let mut label = format!("{} — {}", a.str("address"), a.str("adapter"));
                if !a.str("mac").is_empty() {
                    label.push_str(&format!(" ({})", a.str("mac")));
                }
                if a.str("profile").eq_ignore_ascii_case("public") {
                    label.push_str(" · Public network");
                }
                ui.radio(
                    &mut app.capture.wifi.address,
                    a.str("address").to_owned(),
                    &label,
                );
            }
        }
    }
    let d = &mut app.capture.wifi;
    ui.group(|ui| {
        ui.form("Address", |ui| {
            ui.width(180);
            ui.field("Address", &mut d.address);
        });
        // Revision 12: blank is best; the field starts empty.
        let mut edited = false;
        ui.form(NAME_LABEL, |ui| {
            ui.width(260);
            edited = ui.field(NAME_LABEL, &mut d.name).changed();
        });
        ui.form("", |ui| ui.secondary(NAME_LINE));
        if edited || d.address != before {
            d.name_warning = None;
            d.name_anyway = false;
        }
        ui.form("Ports", |ui| {
            ui.width(90);
            ui.field("HTTPS port", &mut d.https);
            ui.width(90);
            ui.field("Certificate page port", &mut d.http);
        });
    });
    ui.secondary("Start checks that the address is on this computer, that the ports are free and that a typed name leads to the address.");
    let chosen = d.address.trim().to_owned();
    name_warning(ui, &mut d.name_warning, &mut d.name_anyway);
    if !chosen.is_empty() {
        ui.label(reserve_tip(&chosen));
        if ui.link("How to reserve the address").clicked() {
            app.capture.wifi.help_open = true;
        }
    }
    let d = &mut app.capture.wifi;
    if let Some(l) = &list
        && !d.address.trim().is_empty()
        && !l.iter().any(|a| a.str("address") == d.address.trim())
    {
        ui.error_text("This address is not one of this computer's addresses now.");
    }
}

/// "This name does not lead to …" with Use it anyway (revision 12).
fn name_warning(ui: &mut Ui, warning: &mut Option<String>, anyway: &mut bool) {
    let Some(text) = warning.clone() else {
        return;
    };
    ui.row(|ui| {
        ui.tone(Tone::Warning);
        ui.bold();
        ui.label(text);
    });
    if *anyway {
        ui.secondary("The name will be used anyway.");
    } else if ui.row(|ui| ui.button("Use it anyway").clicked()) {
        *anyway = true;
    }
}

fn pin_step(app: &mut App, ui: &mut Ui, w: &Value) {
    ui.label("Phones enter this PIN once, when they pair. 6 to 12 digits.");
    ui.secondary(PIN_WHY);
    if w.at("pin_set").as_bool() == Some(true) {
        ui.label("A Capture PIN is set. Leave these empty to keep it, or type a new one.");
    }
    let d = &mut app.capture.wifi;
    ui.group(|ui| {
        ui.form("Capture PIN", |ui| {
            ui.width(200);
            ui.password("Capture PIN", &mut d.pin);
        });
        ui.form("Capture PIN again", |ui| {
            ui.width(200);
            ui.password("Capture PIN again", &mut d.pin2);
        });
    });
    if !d.pin.is_empty()
        && let Some(p) = pin_problem(&d.pin, &d.pin2)
    {
        ui.error_text(p);
    }
}

fn selected_public(app: &App) -> bool {
    let d = &app.capture.wifi;
    d.addresses.as_ref().is_some_and(|l| {
        l.iter().any(|a| {
            a.str("address") == d.address.trim() && a.str("profile").eq_ignore_ascii_case("public")
        })
    })
}

fn firewall_step(app: &mut App, ui: &mut Ui) {
    let d = &app.capture.wifi;
    ui.label(format!(
        "On Windows, Start adds an inbound firewall rule named \"MoneyKeep capture (Wi-Fi)\" for ports {} and {}, for Private and Domain networks only (never Public). Windows asks once for permission.",
        d.https.trim(),
        d.http.trim()
    ));
    if selected_public(app) {
        ui.row(|ui| {
            ui.tone(Tone::Warning);
            ui.bold();
            ui.label(PUBLIC_WARNING);
        });
    }
    ui.secondary(format!(
        "On macOS: if its firewall is on, macOS asks once whether moneykeep-server may accept incoming connections; choose Allow. A build without a Developer ID signature may be asked about again after each update. On Linux: if a firewall (ufw, firewalld) is on, allow TCP ports {https} and {http} from your home network, e.g. sudo ufw allow from 192.168.0.0/16 to any port {https},{http} proto tcp.",
        https = d.https.trim(),
        http = d.http.trim()
    ));
    ui.secondary("Office networks: phones on the Wi-Fi may be kept from reaching computers (\"client isolation\"). Ask your network team, or use Cloudflare (Settings > Advanced).");
}

fn start_step(app: &mut App, ui: &mut Ui) {
    let d = &app.capture.wifi;
    let host = if d.name.trim().is_empty() {
        d.address.trim().to_owned()
    } else {
        d.name.trim().to_owned()
    };
    ui.label(format!(
        "MoneyKeep will install (or update) the capture server with a Wi-Fi listener at https://{host}:{}, make its certificate authority and certificate, add the firewall rule, set the Capture PIN, and check that it answers.",
        d.https.trim()
    ));
    let working = busy(app, Kind::WifiApply);
    let done = app.capture.wifi.is_done(3);
    ui.row(|ui| {
        ui.enabled(!working);
        let label = if done { "Start again" } else { "Start" };
        if ui.primary(label).clicked() {
            let d = &mut app.capture.wifi;
            d.error = None;
            let port = |s: &str, dflt: u16| s.trim().parse::<u16>().unwrap_or(dflt);
            let mut args = json!({
                "address": d.address.trim(),
                "name": d.name.trim(),
                "https_port": port(&d.https, 14443),
                "http_port": port(&d.http, 14080),
            });
            if d.name_anyway
                && !d.name.trim().is_empty()
                && let Some(o) = args.as_object_mut()
            {
                o.insert("name_anyway".into(), json!(true));
            }
            if !d.pin.is_empty()
                && let Some(o) = args.as_object_mut()
            {
                o.insert("pin".into(), json!(d.pin.trim()));
            }
            start(app, "capture.wifi_apply", args, Kind::WifiApply);
        }
        if working {
            ui.progress("Starting", -1);
            ui.label("Starting…");
        }
        if done && !working {
            ui.icon("check");
            ui.label("Running.");
        }
    });
    if let Some(r) = app.capture.wifi.result.clone() {
        for a in r.list("actions") {
            ui.secondary(format!("• {}", a.as_str().unwrap_or("")));
        }
    }
}

fn certificate_step(app: &mut App, ui: &mut Ui, w: &Value) {
    let url = w.str("cert_url").to_owned();
    if url.is_empty() {
        ui.label("Start Wi-Fi capture first (step 4).");
        return;
    }
    ui.label("Each phone trusts this computer's certificate once. On the phone, scan QR code 1:");
    ui.wrap(|ui| {
        ui.group(|ui| {
            ui.bold();
            ui.label("QR code 1 — certificate page");
            qr_block(app, ui, "QR code 1", &url);
            ui.label("Fingerprint (SHA-256) — it must match the phone's page:");
            ui.named(format!("Fingerprint {}", w.str("fingerprint")));
            ui.mono();
            ui.heading(w.str("fingerprint"));
        });
        ui.group(|ui| {
            ui.bold();
            ui.label("Android");
            for (i, s) in ANDROID_STEPS.iter().enumerate() {
                ui.label(format!("{}. {s}", i + 1));
            }
            ui.bold();
            ui.label("iPhone or iPad");
            for (i, s) in IPHONE_STEPS.iter().enumerate() {
                ui.label(format!("{}. {s}", i + 1));
            }
        });
    });
    ui.secondary("The certificate is valid only for this computer's address; it cannot be used for any other website.");
}

fn pair_step(app: &mut App, ui: &mut Ui, w: &Value) {
    if let Some(name) = app.capture.wifi.paired.clone() {
        ui.row(|ui| {
            ui.icon("check");
            ui.tone(Tone::Success);
            ui.heading(format!("Paired: {name}"));
        });
        ui.label("Now install the app on the phone's home screen:");
        ui.label(
            "• Android (Chrome): tap Install app in the capture app, or the menu ⋮ > Install app.",
        );
        ui.label("• iPhone or iPad (Safari): tap Share > Add to Home Screen, open the new icon, and pair again inside it with a new code — the installed app has its own storage.");
        if ui.row(|ui| ui.button("Pair another phone").clicked()) {
            new_code(app);
        }
        return;
    }
    match app.capture.wifi.pair.clone() {
        Some(p) => {
            ui.label("Scan QR code 2 with the phone (after its certificate is installed). The app asks for the Capture PIN and a name for the phone.");
            qr_block(app, ui, "QR code 2", &p.pair_url);
            // Pairing over Wi-Fi always asks for the Capture PIN.
            super::capture::cant_scan(ui, &p.code, true);
            let left = crate::layout::remaining_secs(app.now, p.until);
            if left > 0 {
                ui.label(format!("Expires in {}:{:02}", left / 60, left % 60));
                ui.secondary("Waiting for the phone to pair…");
            } else {
                ui.error_text("This code has expired.");
                if ui.row(|ui| ui.button("New code").clicked()) {
                    new_code(app);
                }
            }
        }
        None => {
            if busy(app, Kind::WifiPair) {
                ui.label("Making a pairing code…");
            } else if w.at("installed").as_bool() != Some(true) {
                ui.label("Start Wi-Fi capture first (step 4).");
            } else if ui.row(|ui| ui.primary("Show QR code").clicked()) {
                new_code(app);
            }
        }
    }
}

// ----- the help ------------------------------------------------------------------------------

/// The help's lines as (kind, text): `h1`, `h2`, `h3`, `item` or `text`; Markdown
/// marks removed.
pub(crate) fn help_lines(md: &str) -> Vec<(&'static str, String)> {
    let clean = |s: &str| s.replace("**", "").replace('`', "");
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let mut para = String::new();
    let flush = |para: &mut String, out: &mut Vec<(&'static str, String)>| {
        if !para.trim().is_empty() {
            out.push(("text", clean(para.trim())));
        }
        para.clear();
    };
    for line in md.lines() {
        let t = line.trim_end();
        if let Some(h) = t.strip_prefix("### ") {
            flush(&mut para, &mut out);
            out.push(("h3", clean(h)));
        } else if let Some(h) = t.strip_prefix("## ") {
            flush(&mut para, &mut out);
            out.push(("h2", clean(h)));
        } else if let Some(h) = t.strip_prefix("# ") {
            flush(&mut para, &mut out);
            out.push(("h1", clean(h)));
        } else if t.trim_start().starts_with("- ")
            || t.trim_start().starts_with("* ")
            || t.trim_start()
                .split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            flush(&mut para, &mut out);
            out.push(("item", clean(t.trim_start())));
        } else if t.trim().is_empty() || t.starts_with("```") {
            flush(&mut para, &mut out);
        } else if t.starts_with("  ")
            && out.last().is_some_and(|(k, _)| *k == "item")
            && para.is_empty()
        {
            if let Some((_, last)) = out.last_mut() {
                last.push(' ');
                last.push_str(&clean(t.trim()));
            }
        } else {
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(t.trim());
        }
    }
    flush(&mut para, &mut out);
    out
}

fn help_dialog(app: &mut App, ui: &mut Ui) {
    if !app.capture.wifi.help_open {
        return;
    }
    let mut close = ui.dialog_closed("wifi-help") || ui.key("escape");
    ui.dialog("wifi-help", "Phone capture help", 820, |ui| {
        for (kind, text) in help_lines(HELP) {
            match kind {
                "h1" => ui.big_heading(text),
                "h2" => ui.heading(text),
                "h3" => {
                    ui.bold();
                    ui.label(text);
                }
                "item" => ui.label(format!("  {text}")),
                _ => ui.label(text),
            }
        }
        ui.actions(|ui| {
            ui.spacer();
            if ui.primary("Close help").clicked() {
                close = true;
            }
        });
    });
    if close {
        app.capture.wifi.help_open = false;
    }
}

// ----- Settings > Phone capture ------------------------------------------------------------

/// The Wi-Fi card of Settings > Phone capture.
pub(crate) fn settings_card(app: &mut App, ui: &mut Ui) {
    let w = wifi(app);
    let installed = w.at("installed").as_bool() == Some(true);
    ui.card("Wi-Fi capture", |ui| {
        ui.secondary("Phones on this computer's Wi-Fi (or wired network) send expenses straight to MoneyKeep. Each phone installs a certificate once and pairs with a QR code and your Capture PIN. Expenses entered away from home wait on the phone and are sent when it is back on this Wi-Fi.");
        if installed {
            ui.row(|ui| {
                ui.label("Phone address:");
                ui.bold();
                ui.label(w.str("app_url"));
            });
            ui.row(|ui| {
                ui.label("Certificate fingerprint:");
                ui.mono();
                ui.label(w.str("fingerprint"));
            });
            if w.at("public_network").as_bool() == Some(true) {
                ui.error_text(PUBLIC_WARNING);
            }
        }
        ui.wrap(|ui| {
            if !installed {
                let label = if w.at("started").as_bool() == Some(true) || app.capture.wifi.step > 0
                {
                    "Continue Wi-Fi setup…"
                } else {
                    "Set up Wi-Fi capture…"
                };
                if ui.primary(label).clicked() {
                    open(app, None);
                }
            } else {
                // app-desktop revision 39: pairing is Paired phones' Pair a phone… only.
                if ui.button("Certificate QR code…").clicked() {
                    open(app, Some(4));
                }
                if ui.button("Change address…").clicked() {
                    app.capture.wifi.change = Some((w.str("address").to_owned(), w.str("name").to_owned()));
                    app.capture.wifi.change_result = None;
                    app.capture.wifi.change_warning = None;
                    app.capture.wifi.change_anyway = false;
                }
                if ui.button("Change PIN…").clicked() {
                    app.capture.wifi.pin_change = Some((String::new(), String::new()));
                }
            }
            if ui.button("Phone capture help").clicked() {
                app.capture.wifi.help_open = true;
            }
            if w.at("started").as_bool() == Some(true) && ui.button("Remove Wi-Fi capture…").clicked() {
                app.capture.wifi.confirm_remove = true;
            }
        });
        ui.secondary("To send from away from home, or on a network that keeps devices apart, use phone capture over Cloudflare instead (below).");
    });
}

/// Remove Wi-Fi capture, Change address and Change PIN.
pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    dialog(app, ui);
    remove_dialog(app, ui);
    change_dialog(app, ui);
    pin_dialog(app, ui);
}

fn remove_dialog(app: &mut App, ui: &mut Ui) {
    if !app.capture.wifi.confirm_remove {
        return;
    }
    let w = wifi(app);
    let mut go = false;
    let mut cancel = ui.dialog_closed("wifi-remove") || ui.key("escape");
    ui.dialog("wifi-remove", "Remove Wi-Fi capture?", 560, |ui| {
        ui.label("MoneyKeep will:");
        ui.label(format!("• stop the Wi-Fi listener on {}", w.str("address")));
        if let Some(r) = w.at("firewall_rule").as_str() {
            ui.label(format!("• remove the firewall rule \"{r}\""));
        }
        ui.label("• remove the certificate authority and the server certificate, with their keys");
        ui.label("• disconnect the phones paired over Wi-Fi");
        ui.secondary("Phones paired over Wi-Fi keep anything still in their Outbox; send it first. Remove the app and the certificate from each phone too (see Help > Removing). Phone capture over Cloudflare is not changed.");
        ui.actions(|ui| {
            cancel |= ui.button("Cancel").clicked();
            go = ui.primary("Remove").clicked();
        });
    });
    if go {
        app.capture.wifi.confirm_remove = false;
        start(app, "capture.wifi_remove", json!({}), Kind::WifiRemove);
    } else if cancel {
        app.capture.wifi.confirm_remove = false;
    }
}

fn change_dialog(app: &mut App, ui: &mut Ui) {
    let Some((mut address, mut name)) = app.capture.wifi.change.take() else {
        return;
    };
    let result = app.capture.wifi.change_result.clone();
    let working = busy(app, Kind::WifiChange);
    let mut close = ui.dialog_closed("wifi-change") || ui.key("escape");
    let mut go = false;
    ui.dialog("wifi-change", "Change address", 600, |ui| {
        match &result {
            None => {
                ui.row(|ui| {
                    ui.tone(Tone::Warning);
                    ui.bold();
                    ui.label(CHANGE_WARNING);
                });
                ui.group(|ui| {
                    ui.form("New address", |ui| {
                        ui.width(180);
                        ui.field("New address", &mut address);
                    });
                    ui.form(NAME_LABEL, |ui| {
                        ui.width(260);
                        if ui.field("New local name", &mut name).changed() {
                            app.capture.wifi.change_warning = None;
                            app.capture.wifi.change_anyway = false;
                        }
                    });
                    ui.form("", |ui| ui.secondary(NAME_LINE));
                });
                {
                    let d = &mut app.capture.wifi;
                    name_warning(ui, &mut d.change_warning, &mut d.change_anyway);
                }
                // upgrade-resilience-1 U1.7: said before saving.
                ui.label(CERT_AGAIN);
                if !address.trim().is_empty() {
                    ui.secondary(reserve_tip(address.trim()));
                }
                if let Some(e) = &app.capture.wifi.error {
                    ui.error_text(e.clone());
                }
                ui.actions(|ui| {
                    close |= ui.button("Cancel").clicked();
                    let d = &app.capture.wifi;
                    ui.enabled(
                        !working
                            && !address.trim().is_empty()
                            && (d.change_warning.is_none() || d.change_anyway),
                    );
                    go = ui.primary("Change address").clicked();
                    if working {
                        ui.progress("Changing", -1);
                    }
                });
            }
            Some(v) => {
                ui.label(format!(
                    "Wi-Fi capture now uses {}. Each phone must install the new certificate (QR code 1), open the new address, install the app again and pair it.",
                    v.str("host")
                ));
                let stale = v.list("stale_phones");
                if stale.is_empty() {
                    ui.secondary("Every phone paired over Wi-Fi was in contact within the last day.");
                } else {
                    ui.label("Phones not in contact for over a day (anything still in their Outbox stays there):");
                    for s in stale {
                        ui.label(format!("• {}", s.as_str().unwrap_or("")));
                    }
                }
                ui.actions(|ui| {
                    ui.spacer();
                    close |= ui.primary("Done").clicked();
                });
            }
        }
    });
    if go {
        app.capture.wifi.error = None;
        let mut args = json!({"address": address.trim(), "name": name.trim()});
        if app.capture.wifi.change_anyway
            && !name.trim().is_empty()
            && let Some(o) = args.as_object_mut()
        {
            o.insert("name_anyway".into(), json!(true));
        }
        start(app, "capture.wifi_change_address", args, Kind::WifiChange);
    }
    if close {
        app.capture.wifi.change_result = None;
    } else {
        app.capture.wifi.change = Some((address, name));
    }
}

fn pin_dialog(app: &mut App, ui: &mut Ui) {
    let Some((mut pin, mut again)) = app.capture.wifi.pin_change.take() else {
        return;
    };
    let mut close = ui.dialog_closed("wifi-pin") || ui.key("escape");
    let mut go = false;
    let working = busy(app, Kind::WifiPin);
    ui.dialog("wifi-pin", "Change Capture PIN", 480, |ui| {
        ui.label("Phones already paired keep working; the new PIN is asked when a phone pairs.");
        ui.group(|ui| {
            ui.form("New Capture PIN", |ui| {
                ui.width(200);
                ui.password("New Capture PIN", &mut pin);
            });
            ui.form("New Capture PIN again", |ui| {
                ui.width(200);
                ui.password("New Capture PIN again", &mut again);
            });
        });
        let problem = pin_problem(&pin, &again);
        if !pin.is_empty()
            && let Some(p) = problem
        {
            ui.error_text(p);
        }
        ui.actions(|ui| {
            close |= ui.button("Cancel").clicked();
            ui.enabled(problem.is_none() && !working);
            go = ui.primary("Change PIN").clicked();
        });
    });
    if go {
        start(
            app,
            "capture.wifi_set_pin",
            json!({"pin": pin.trim()}),
            Kind::WifiPin,
        );
        app.capture.wifi.pin_change = Some((String::new(), String::new()));
    } else if !close {
        app.capture.wifi.pin_change = Some((pin, again));
    }
}

// ----- the firewall (mobile-capture.md revision 18) ------------------------------------------

/// The question at start, when phones can't reach this computer.
pub(crate) const FIREWALL_ASK_TITLE: &str = "Phones can't reach this computer";
/// The question's text; its button is Fix (mk-commands' `text::RULE_MISSING`).
pub(crate) const FIREWALL_ASK_BODY: &str =
    "Phones can't reach this computer: the firewall rule is missing.";
pub(crate) const FIREWALL_FIXED: &str =
    "The firewall was fixed. Phones on this Wi-Fi can reach this computer again.";
pub(crate) const OPEN_NETWORK_SETTINGS: &str = "Open network settings";

/// Check the firewall again (Check now; after a fix).
pub(crate) fn check_firewall(app: &mut App) {
    if !super::capture::busy(app, Kind::FirewallCheck)
        && wifi(app).at("installed").as_bool() == Some(true)
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

fn has_problem(v: &Value, code: &str) -> bool {
    v.list("problems").iter().any(|p| p.str("code") == code)
}

/// A firewall job ended.
pub(crate) fn firewall_finished(app: &mut App, kind: Kind, out: Result<Value, crate::UiError>) {
    let d = &mut app.capture.wifi;
    match (kind, out) {
        (Kind::FirewallCheck, Ok(v)) => {
            if !d.firewall_asked && has_problem(&v, "rule_missing") {
                d.firewall_asked = true;
                d.firewall_ask = true;
            }
            d.firewall = Some(v);
        }
        (Kind::FirewallCheck, Err(_)) => d.firewall = None,
        (Kind::FirewallFix, Ok(v)) => {
            d.firewall_ask = false;
            let text = if v.at("fixed").as_bool() == Some(false) {
                "Nothing needed fixing.".to_owned()
            } else {
                FIREWALL_FIXED.to_owned()
            };
            d.firewall_outcome = Some((true, text.clone()));
            app.notify(text);
            check_firewall(app);
        }
        (Kind::FirewallFix, Err(e)) => {
            d.firewall_ask = false;
            d.firewall_outcome = Some((false, e.display()));
        }
        _ => {}
    }
}

fn fix(app: &mut App, what: Option<&str>) {
    app.capture.wifi.firewall_outcome = None;
    let args = what.map_or_else(|| json!({}), |w| json!({ "what": w }));
    start(app, "capture.wifi_firewall_fix", args, Kind::FirewallFix);
}

/// The Status card's firewall lines: the rule missing (Fix), Windows' own Block
/// rules (Fix), a Public network (a link to Windows' network settings).
pub(crate) fn firewall_status(app: &mut App, ui: &mut Ui) {
    let Some(v) = app.capture.wifi.firewall.clone() else {
        return;
    };
    let fixing = super::capture::busy(app, Kind::FirewallFix);
    for p in v.list("problems") {
        // macos-release.md C5: macOS's firewall only while phones don't get through.
        if !super::capture_mac::show_problem(app, p) {
            continue;
        }
        ui.banner(Tone::Warning, |ui| {
            ui.bold();
            ui.label(p.str("text"));
            if !p.str("detail").is_empty() {
                ui.label(p.str("detail"));
            }
            ui.row(|ui| {
                if let Some(what) = p.at("fix").as_str() {
                    ui.enabled(!fixing).tip(
                        "Windows asks for permission, because the firewall is set for all users",
                    );
                    if ui.primary("Fix").clicked() {
                        fix(app, Some(what));
                    }
                }
                let label = p
                    .at("link_label")
                    .as_str()
                    .unwrap_or(OPEN_NETWORK_SETTINGS)
                    .to_owned();
                if let Some(link) = p.at("link").as_str()
                    && ui.button(label).clicked()
                {
                    app.emit(crate::AppEvent::OpenLink {
                        url: link.to_owned(),
                    });
                }
            });
        });
    }
    if fixing {
        ui.progress("Fixing the firewall", -1);
    }
    if let Some((ok, text)) = app.capture.wifi.firewall_outcome.clone() {
        if ok {
            ui.secondary(text);
        } else {
            ui.error_text(text);
        }
    }
}

/// The question at start (once): the firewall rule is missing; Fix asks Windows.
pub(crate) fn firewall_dialog(app: &mut App, ui: &mut Ui) {
    if !app.capture.wifi.firewall_ask {
        return;
    }
    let mut go = false;
    let mut later = ui.dialog_closed("wifi-firewall") || ui.key("escape");
    let fixing = super::capture::busy(app, Kind::FirewallFix);
    ui.dialog("wifi-firewall", FIREWALL_ASK_TITLE, 520, |ui| {
        ui.label(FIREWALL_ASK_BODY);
        ui.secondary("MoneyKeep puts back its firewall rule for phone capture on Wi-Fi (ports for phones on Private and Domain networks only). Windows will ask for permission.");
        ui.actions(|ui| {
            later |= ui.button("Not now").clicked();
            ui.enabled(!fixing);
            go = ui.primary("Fix").clicked();
        });
    });
    if go {
        fix(app, Some("rule"));
    } else if later {
        app.capture.wifi.firewall_ask = false;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn pin_rules() {
        assert_eq!(
            pin_problem("12345", "12345"),
            Some("The Capture PIN is 6 to 12 digits.")
        );
        assert_eq!(
            pin_problem("1234567890123", "1234567890123"),
            Some("The Capture PIN is 6 to 12 digits.")
        );
        assert_eq!(
            pin_problem("12a456", "12a456"),
            Some("The Capture PIN is 6 to 12 digits.")
        );
        assert_eq!(
            pin_problem("123456", "123457"),
            Some("The two PINs are not the same.")
        );
        assert_eq!(pin_problem("123456", "123456"), None);
        assert_eq!(pin_problem("123456789012", "123456789012"), None);
    }

    /// Test 71: QR code 1 and QR code 2 decode to the certificate page and the
    /// pairing address.
    #[test]
    fn at71_qr_codes_decode() {
        for url in [
            mk_capture_api::cert_page_url("192.168.1.20", 14080),
            mk_capture_api::wifi_pair_url("192.168.1.20", 14443, "ABCD2345"),
        ] {
            let pic = super::super::capture_setup::make_qr(&url).unwrap();
            let (w, h) = (pic.width as usize, pic.height as usize);
            let mut img =
                rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| pic.rgba[(y * w + x) * 4]);
            let grids = img.detect_grids();
            assert_eq!(grids.len(), 1);
            assert_eq!(grids[0].decode().unwrap().1, url);
        }
    }

    #[test]
    fn help_is_the_deployment_guide() {
        let lines = help_lines(HELP);
        assert!(
            lines
                .iter()
                .any(|(k, t)| *k == "h1" && t.contains("Phone capture"))
        );
        assert!(lines.iter().any(|(k, _)| *k == "item"));
        assert!(!lines.iter().any(|(_, t)| t.contains("**")));
    }

    /// Revision 12: the help's name section starts with "Leave the name blank…",
    /// and says how to drop a name already in use.
    #[test]
    fn r12_help_name_section() {
        let lines = help_lines(HELP);
        let at = lines
            .iter()
            .position(|(k, t)| *k == "h3" && t.starts_with("A local name"))
            .unwrap();
        assert_eq!(
            lines[at + 1].1,
            "Leave the name blank unless your network has its own DNS (an OPNsense or router host override, or your company's DNS). With a fixed IP address no name is needed."
        );
        let flat = HELP.split_whitespace().collect::<Vec<_>>().join(" ");
        // mobile-capture.md revision 13 correction: the pairing is not kept.
        assert!(
            flat.contains(super::CHANGE_WARNING),
            "the help and the dialog agree"
        );
        assert!(!flat.contains("pairing is kept"));
        assert!(flat.contains("Use it anyway"));
    }

    /// Test 75: the help has the sections the spec lists, in order, including the
    /// corporate-network paragraph.
    #[test]
    fn at75_help_sections() {
        let heads: Vec<String> = HELP
            .lines()
            .filter(|l| l.starts_with("## ") || l.starts_with("### "))
            .map(|l| l.trim_start_matches('#').trim().to_owned())
            .collect();
        let want = [
            "1. What you need",
            "2. Give the computer a fixed address",
            "Home router",
            "OPNsense or pfSense firewalls",
            "Corporate or office network",
            "A static address set on the computer itself",
            "Laptops: Wi-Fi and Ethernet",
            "3. Run Wi-Fi setup in MoneyKeep",
            "4. Install the certificate on the phone",
            "Android",
            "iPhone or iPad",
            "5. Pair the phone",
            "6. Everyday use",
            "When your computer is off",
            "7. If the computer's address changes",
            "8. Removing",
            "9. Troubleshooting",
            "10. Security notes",
            "11. When to use Cloudflare instead",
        ];
        let mut at = 0;
        for w in want {
            let pos = heads[at..]
                .iter()
                .position(|h| h == w)
                .unwrap_or_else(|| panic!("{w} missing or out of order: {heads:?}"));
            at += pos + 1;
        }
        let flat = HELP.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains(
            "If on a corporate network, request a fixed IP address via DHCP reservation from your network team."
        ));
        for must in [
            "Encryption & credentials",
            "Install a certificate",
            "CA certificate",
            "Trusted credentials > User",
            "VPN & Device Management",
            "Certificate Trust Settings",
            "Services > DHCPv4",
            "static mapping",
            "client isolation",
            "Private DNS",
            "Your connection is not private",
            "That PIN is not right",
            "home.arpa",
            // mobile-capture.md revision 14.
            "Your computer isn't reachable",
            "Will send when your computer is on and connected",
            "Lists from Nov 8, 1:05 PM",
            "doesn't sleep while plugged in",
        ] {
            assert!(flat.contains(must), "{must}");
        }
    }
}
