//! Wi-Fi capture, the normal phone capture setup (mobile-capture.md revision 10).
//!
//! Phones on the home or office network reach the capture server on this computer
//! directly over HTTPS, with a small certificate authority each phone trusts once,
//! and pair with the QR code and a Capture PIN. What runs on this computer (the
//! server, its certificates, the firewall rule) is behind
//! [`crate::capture_setup::CaptureHost`]; the PIN, pairing codes and the phones go
//! through the server's desktop API on the loopback address. Network calls and host
//! actions never happen inside a book write: each step is followed by a short write
//! recording what was made (book setting `capture.wifi`), so the dialog resumes and
//! removal undoes exactly what was made.

use mk_capture_api as api;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::capture::MODULE;
use crate::capture_setup::{
    FirewallFix, FirewallRule, FirewallState, HostOp, HostOut, PERMISSION_DECLINED, ServiceState,
    WifiOutcome, WifiRemove, WifiRequest,
};
use crate::connect::{
    Connector, FetchError, HttpMethod, HttpRequest, HttpResponse, NetJob, NetOp, NetStep,
};
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) const WIFI_KEY: &str = "capture.wifi";
const DAY: i64 = 86_400;
const MAX_JSON: usize = 256 * 1024;

pub mod text {
    /// Change address: shown before anything is changed.
    pub const CHANGE_WARNING: &str = "Before you change it, send everything in the phone's Outbox. Afterwards install the new certificate, open the new address on the phone, install the app again and pair it.";
    pub const PUBLIC_NETWORK: &str = "Windows treats this network as Public, so the firewall rule (Private and Domain networks only) does not let phones in. Change it: Settings > Network & internet > Wi-Fi (or Ethernet) > this network > Network profile type > Private network.";
    pub const NOT_WINDOWS_FIREWALL: &str = "On macOS: if its firewall is on, macOS asks once whether moneykeep-server may accept incoming connections; choose Allow. A build without a Developer ID signature may be asked about again after each update. On Linux: if a firewall (ufw, firewalld) is on, allow TCP ports {HTTPS} and {HTTP} from your home network, e.g. `sudo ufw allow from 192.168.0.0/16 to any port {HTTPS},{HTTP} proto tcp`.";
    pub const WHY_FIXED: &str = "A phone keeps the capture app, its pairing and its unsent expenses for one address. If the computer's address changes, the phone sees a different app and must be set up again. Give the computer a fixed address (a DHCP reservation in your router) before you start.";
    pub const WHY_NOT_BOOK_PASSWORD: &str = "This is not your book password: the phone never needs the key to your book. The PIN only proves that a phone pairing on your network was allowed by you.";
    /// Revision 12: a typed local name that does not lead to the chosen address
    /// on this computer.
    pub fn name_unresolved(address: &str) -> String {
        format!(
            "This name does not lead to {address} on this computer. Leave it blank to use the address."
        )
    }
    // Revision 18: the firewall check (Status card, and the question at start).
    /// MoneyKeep's rule is gone (the question at start ends with Fix).
    pub const RULE_MISSING: &str =
        "Phones can't reach this computer: the firewall rule is missing.";
    /// … or turned off.
    pub const RULE_DISABLED: &str =
        "Phones can't reach this computer: the firewall rule is turned off.";
    /// Windows' own Block rules for the capture server.
    pub const BLOCKED: &str = "Windows is blocking the capture server";
    pub const BLOCKED_DETAIL: &str = "Windows made a rule that blocks moneykeep-server.exe (this happens when its \"allow access\" pop-up is closed). Fix removes that rule; Windows asks for permission.";
    /// The network is Public.
    pub const PUBLIC: &str =
        "Windows treats this network as public, so phones can't connect over Wi-Fi";
    pub const PUBLIC_DETAIL: &str = "Change it in Windows' network settings: this network > Network profile type > Private network. MoneyKeep doesn't change it itself.";
    /// Windows' network settings page.
    pub const NETWORK_SETTINGS: &str = "ms-settings:network-status";
    /// What the Fix button's prompt is for.
    pub const FIX_PERMISSION: &str =
        "Windows will ask for permission, because the firewall is set for all users.";
    /// security-review-1 A6: a Fix asked for while the server isn't installed.
    pub const NOT_INSTALLED: &str = "The capture server isn't installed: press Install the server again, which also puts the firewall rule back.";
    pub const FIX_DECLINED: &str = "The firewall was not changed — Windows permission was not given. Press Fix again when ready.";
    // macos-release.md C5: macOS's application firewall.
    /// The Status card when the firewall is on and phones don't get through.
    pub const MAC_FIREWALL: &str = "macOS's firewall may be blocking phone capture";
    pub const MAC_FIREWALL_DETAIL: &str = "macOS's firewall is on. When it asks whether moneykeep-server may accept incoming connections, choose Allow. If you chose Deny, or no phone has reached this Mac lately, open Firewall settings > Options and set moneykeep-server to Allow incoming connections.";
    /// System Settings → Network → Firewall (opened exactly).
    pub const MAC_FIREWALL_SETTINGS: &str =
        "x-apple.systempreferences:com.apple.preference.security?Firewall";
    pub const MAC_FIREWALL_BUTTON: &str = "Open Firewall settings";
    /// The status light when the computer lost the address.
    pub fn address_gone(address: &str) -> String {
        format!("Wi-Fi capture: this computer no longer has {address}")
    }
}

/// What Wi-Fi setup made (no secret: the PIN is never kept here).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WifiRec {
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub https_port: u16,
    #[serde(default)]
    pub http_port: u16,
    #[serde(default)]
    pub adapter: String,
    #[serde(default)]
    pub mac: String,
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub ca_name: String,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub ca_not_after: i64,
    #[serde(default)]
    pub cert_not_before: i64,
    #[serde(default)]
    pub cert_not_after: i64,
    /// The firewall rule this setup added (Windows).
    #[serde(default)]
    pub firewall_rule: Option<String>,
    #[serde(default)]
    pub pin_set: bool,
    /// The server runs with the Wi-Fi listener (setup step 4 done).
    #[serde(default)]
    pub installed: bool,
    /// The server answered over HTTPS on the address after the install.
    #[serde(default)]
    pub answered: bool,
    #[serde(default)]
    pub check_error: Option<String>,
    /// What was done, in words (Details).
    #[serde(default)]
    pub details: Vec<String>,
    /// upgrade-resilience-1 U1.7: when Change address last made a new certificate
    /// for another address (Unix seconds): phones paired over Wi-Fi not seen since
    /// still need the new certificate.
    #[serde(default)]
    pub address_changed_at: Option<i64>,
}

impl WifiRec {
    fn host(&self) -> String {
        if self.name.is_empty() {
            self.address.clone()
        } else {
            self.name.clone()
        }
    }
    fn note(&mut self, line: String) {
        self.details.push(line);
        if self.details.len() > 100 {
            let cut = self.details.len() - 100;
            self.details.drain(..cut);
        }
    }
}

pub(crate) fn load(ctx: &Ctx<'_>) -> CommandResult<WifiRec> {
    Ok(ctx
        .tx
        .get_setting(MODULE, WIFI_KEY)?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

fn save(ctx: &Ctx<'_>, w: &WifiRec) -> CommandResult<()> {
    let v = serde_json::to_value(w).map_err(|e| CommandError::Integrity(e.to_string()))?;
    ctx.save_setting(MODULE, WIFI_KEY, Some(&v))
}

/// The certificate page and pairing addresses of a record.
fn urls(w: &WifiRec) -> (String, String) {
    let host = w.host();
    (
        api::cert_page_url(&host, w.http_port),
        format!("https://{host}:{}/", w.https_port),
    )
}

/// Wi-Fi capture for `capture.settings` (no secrets).
pub(crate) fn summary(ctx: &Ctx<'_>) -> CommandResult<Value> {
    let w = load(ctx)?;
    let (cert_url, app_url) = urls(&w);
    Ok(json!({
        "started": !w.address.is_empty(),
        "installed": w.installed,
        "address": w.address,
        "name": w.name,
        "host": w.host(),
        "https_port": w.https_port,
        "http_port": w.http_port,
        "adapter": w.adapter,
        "mac": w.mac,
        "profile": w.profile,
        "public_network": w.profile.eq_ignore_ascii_case("public"),
        "ca_name": w.ca_name,
        "fingerprint": w.fingerprint,
        "ca_not_after": w.ca_not_after,
        "cert_not_before": w.cert_not_before,
        "cert_not_after": w.cert_not_after,
        "firewall_rule": w.firewall_rule,
        "pin_set": w.pin_set,
        "answered": w.answered,
        "check_error": w.check_error,
        "cert_url": if w.address.is_empty() { String::new() } else { cert_url },
        "app_url": if w.address.is_empty() { String::new() } else { app_url },
        "details": w.details,
        "address_changed_at": w.address_changed_at,
    }))
}

pub(crate) fn is_network(name: &str) -> bool {
    matches!(
        name,
        "capture.wifi_addresses"
            | "capture.wifi_apply"
            | "capture.wifi_set_pin"
            | "capture.wifi_pair_code"
            | "capture.wifi_change_address"
            | "capture.wifi_remove"
            | "capture.wifi_firewall_check"
            | "capture.wifi_firewall_fix"
    )
}

fn network_only<A>(_: &Ctx<'_>, _: A) -> CommandResult<Value> {
    Err(CommandError::Rejected(
        "this MoneyKeep front end cannot reach the network; use the app".into(),
    ))
}

pub(crate) fn register(r: &mut Registry) {
    r.add_in(
        MODULE,
        "capture.wifi_addresses",
        "Wi-Fi capture: this computer's network addresses with their adapters, MAC addresses and network profiles",
        false,
        network_only::<crate::builtin::NoArgs>,
    );
    r.add_in(
        MODULE,
        "capture.wifi_apply",
        "Set up Wi-Fi capture: the capture server on this computer's address with its certificates, the firewall rule and the Capture PIN (handles a secret; not offered over MCP)",
        true,
        network_only::<ApplyArgs>,
    );
    r.add_in(
        MODULE,
        "capture.wifi_set_pin",
        "Change the Capture PIN phones enter when pairing (handles a secret; not offered over MCP)",
        true,
        network_only::<PinArgs>,
    );
    r.add_in(
        MODULE,
        "capture.wifi_pair_code",
        "A pairing code for a phone on the Wi-Fi, with the certificate page and pairing addresses for the two QR codes (not offered over MCP)",
        false,
        network_only::<crate::builtin::NoArgs>,
    );
    r.add_in(
        MODULE,
        "capture.wifi_change_address",
        "Move Wi-Fi capture to another address of this computer: new certificates; phones must install the new certificate and pair again (not offered over MCP)",
        true,
        network_only::<ChangeArgs>,
    );
    r.add_in(
        MODULE,
        "capture.wifi_remove",
        "Remove Wi-Fi capture: the Wi-Fi listener, the firewall rule, the certificates and the phones paired over Wi-Fi (not offered over MCP)",
        true,
        network_only::<crate::builtin::NoArgs>,
    );
    r.add_in(
        MODULE,
        "capture.wifi_firewall_check",
        "Check this computer's firewall for Wi-Fi capture: the firewall rule, Windows' own Block rules for the capture server, and a network Windows treats as Public (reads only)",
        false,
        network_only::<crate::builtin::NoArgs>,
    );
    r.add_in(
        MODULE,
        "capture.wifi_firewall_fix",
        "Fix this computer's firewall for Wi-Fi capture: put the firewall rule back and remove Windows' Block rules for the capture server (one Windows permission prompt; not offered over MCP)",
        true,
        network_only::<FixArgs>,
    );
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixArgs {
    /// What to fix: `rule` (put the firewall rule back), `block` (remove Windows'
    /// Block rules), or both when left out.
    #[serde(default)]
    what: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApplyArgs {
    /// This computer's IPv4 address on the home or office network.
    address: String,
    /// A local name for the address, e.g. `moneykeep.home.arpa` (optional).
    #[serde(default)]
    name: Option<String>,
    /// The HTTPS port (default 14443).
    #[serde(default)]
    https_port: Option<u16>,
    /// The certificate page's port (default 14080).
    #[serde(default)]
    http_port: Option<u16>,
    /// The Capture PIN, 6–12 digits (stored by the server only as an Argon2id hash).
    #[serde(default)]
    pin: Option<String>,
    /// Use the local name even though it does not lead to the address on this
    /// computer (the user chose Use it anyway).
    #[serde(default)]
    name_anyway: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PinArgs {
    /// The new Capture PIN, 6–12 digits.
    pin: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChangeArgs {
    /// The new IPv4 address.
    address: String,
    #[serde(default)]
    name: Option<String>,
    /// Use the local name even though it does not lead to the address.
    #[serde(default)]
    name_anyway: Option<bool>,
}

/// A checked address, name and ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Place {
    address: String,
    name: String,
    https_port: u16,
    http_port: u16,
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

fn coded(code: &'static str, m: impl Into<String>) -> CommandError {
    CommandError::Coded {
        code,
        message: m.into(),
    }
}

/// An IPv4 address of a home or office network.
pub fn check_address(a: &str) -> Result<String, String> {
    let ip: std::net::Ipv4Addr = a
        .trim()
        .parse()
        .map_err(|_| format!("\"{}\" is not an address such as 192.168.1.20.", a.trim()))?;
    if ip.is_private() || ip.is_link_local() {
        Ok(ip.to_string())
    } else {
        Err(format!(
            "{ip} is not a home or office network address (10.x, 172.16–31.x or 192.168.x)."
        ))
    }
}

/// A local name such as `moneykeep.home.arpa` (empty: none).
pub fn check_name(n: &str) -> Result<String, String> {
    let n = n.trim().trim_end_matches('.').to_ascii_lowercase();
    if n.is_empty() {
        return Ok(n);
    }
    let ok = n.len() <= 253
        && n.contains('.')
        && n.parse::<std::net::IpAddr>().is_err()
        && n.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    if ok {
        Ok(n)
    } else {
        Err(format!(
            "\"{n}\" is not a local name such as moneykeep.home.arpa."
        ))
    }
}

fn place(
    address: &str,
    name: Option<&str>,
    https: Option<u16>,
    http: Option<u16>,
) -> CommandResult<Place> {
    let address = check_address(address).map_err(invalid)?;
    let name = check_name(name.unwrap_or("")).map_err(invalid)?;
    let https_port = https.unwrap_or(api::DEFAULT_WIFI_HTTPS_PORT);
    let http_port = http.unwrap_or(api::DEFAULT_WIFI_HTTP_PORT);
    for p in [https_port, http_port] {
        if p < 1024 {
            return Err(invalid(format!("Port {p} must be between 1024 and 65535.")));
        }
    }
    if https_port == http_port {
        return Err(invalid("The two ports must differ."));
    }
    Ok(Place {
        address,
        name,
        https_port,
        http_port,
    })
}

/// A typed local name is looked up first, unless the user chose Use it anyway.
fn needs_lookup(p: &Place, anyway: Option<bool>) -> bool {
    !p.name.is_empty() && anyway != Some(true)
}

/// Look the name up on this computer (revision 12), then go on.
fn name_check(
    c: &Connector,
    place: Place,
    pin: Option<Zeroizing<String>>,
    change: bool,
) -> CommandResult<NetStep> {
    let name = place.name.clone();
    job(
        c,
        Op::NameChecked { place, pin, change },
        Vec::new(),
        Some(HostOp::WifiResolve(name)),
    )
}

/// The firewall rule for these ports: Private and Domain networks only.
pub fn firewall_rule(https_port: u16, http_port: u16) -> FirewallRule {
    FirewallRule {
        name: api::FIREWALL_RULE.to_owned(),
        ports: vec![https_port, http_port],
        profiles: vec!["Private".into(), "Domain".into()],
    }
}

/// The steps of Wi-Fi capture's network commands.
#[derive(Clone)]
pub(crate) enum Op {
    Addresses,
    /// Revision 12: the typed local name was looked up; go on with Apply (`pin`)
    /// or Change address (`change`) when it leads to the address.
    NameChecked {
        place: Place,
        pin: Option<Zeroizing<String>>,
        change: bool,
    },
    /// The host turned Wi-Fi capture on; then the PIN is set (when given).
    Installed {
        place: Place,
        pin: Option<Zeroizing<String>>,
        /// Change address: phones paired over Wi-Fi not seen for over a day.
        stale: Option<Vec<String>>,
    },
    /// The PIN was set on the server.
    PinSet {
        result: Value,
    },
    PairCode {
        host: String,
        https_port: u16,
        http_port: u16,
    },
    /// Remove: the phones, read first.
    RemoveDevices,
    /// Remove: Wi-Fi phones disconnected and the host undid its part.
    Removed {
        phones: usize,
    },
    /// Revision 18: the firewall checked (for the answer, or before a fix of
    /// `rule` and/or `block`).
    FirewallChecked {
        fix: Option<(bool, bool)>,
    },
    /// The fix ran.
    FirewallFixed,
}

impl std::fmt::Debug for Op {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Op::Addresses => f.write_str("Addresses"),
            Op::NameChecked { place, change, .. } => f
                .debug_struct("NameChecked")
                .field("place", place)
                .field("pin", &"(hidden)")
                .field("change", change)
                .finish(),
            Op::Installed { place, stale, .. } => f
                .debug_struct("Installed")
                .field("place", place)
                .field("pin", &"(hidden)")
                .field("stale", stale)
                .finish(),
            Op::PinSet { .. } => f.write_str("PinSet"),
            Op::PairCode { host, .. } => f.debug_struct("PairCode").field("host", host).finish(),
            Op::RemoveDevices => f.write_str("RemoveDevices"),
            Op::Removed { phones } => f.debug_struct("Removed").field("phones", phones).finish(),
            Op::FirewallChecked { fix } => {
                f.debug_struct("FirewallChecked").field("fix", fix).finish()
            }
            Op::FirewallFixed => f.write_str("FirewallFixed"),
        }
    }
}

fn job(
    c: &Connector,
    op: Op,
    calls: Vec<HttpRequest>,
    host: Option<HostOp>,
) -> CommandResult<NetStep> {
    let host = match host {
        Some(h) => Some((
            c.host.clone().ok_or_else(|| {
                coded(
                    "no_host",
                    "This MoneyKeep front end cannot set up services; use the desktop app.",
                )
            })?,
            h,
        )),
        None => None,
    };
    Ok(NetStep::Fetch(NetJob::with_host(
        NetOp::CaptureWifi(Box::new(op)),
        calls,
        c.fetcher.clone(),
        host,
    )))
}

fn answer(r: Option<&Result<HttpResponse, FetchError>>, ok: &[u16]) -> CommandResult<Vec<u8>> {
    match r {
        None => Err(CommandError::Rejected(
            "The capture server did not answer.".into(),
        )),
        Some(Err(e)) => Err(coded(
            "unreachable",
            format!("The capture server could not be reached: {e}"),
        )),
        Some(Ok(resp)) if ok.contains(&resp.status) => Ok(resp.body.clone()),
        Some(Ok(resp)) => {
            let m = serde_json::from_slice::<api::ApiError>(&resp.body)
                .map(|e| mk_core::text::clean(&e.error.message, 300).text)
                .unwrap_or_else(|_| format!("HTTP {}", resp.status));
            Err(coded(
                "server_refused",
                format!("The capture server refused: {m}"),
            ))
        }
    }
}

fn devices_of(body: &[u8]) -> Vec<api::Device> {
    serde_json::from_slice::<api::Devices>(body)
        .map(|d| d.devices)
        .unwrap_or_default()
}

impl Registry {
    fn wifi_spec(&self, name: &str) -> CommandResult<&crate::CommandSpec> {
        self.spec(name)
            .ok_or_else(|| CommandError::UnknownCommand(name.to_owned()))
    }

    /// A short audited write of the Wi-Fi record under `command`.
    fn wifi_write(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        command: &str,
        audit: Value,
        f: &dyn Fn(&Ctx<'_>, &mut WifiRec) -> CommandResult<()>,
    ) -> CommandResult<WifiRec> {
        let spec = self.wifi_spec(command)?;
        let v = self.execute(store, actor, spec, audit, None, &|ctx, _| {
            let mut w = load(ctx)?;
            f(ctx, &mut w)?;
            save(ctx, &w)?;
            serde_json::to_value(&w).map_err(|e| CommandError::Integrity(e.to_string()))
        })?;
        serde_json::from_value(v).map_err(|e| CommandError::Integrity(e.to_string()))
    }

    fn wifi_read<T>(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        f: impl FnOnce(&Ctx<'_>) -> CommandResult<T>,
    ) -> CommandResult<T> {
        let encrypted = store.is_encrypted();
        let connector = self.connector.as_ref();
        store.read(|tx| {
            if !crate::is_module_enabled(tx, MODULE)? {
                return Err(CommandError::ModuleDisabled {
                    command: "capture".into(),
                    module: MODULE.into(),
                });
            }
            let mut ctx = Ctx::new(tx, actor);
            ctx.connector = connector;
            ctx.encrypted = encrypted;
            f(&ctx)
        })
    }

    fn install_op(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        p: &Place,
    ) -> CommandResult<HostOp> {
        let (has_conf, lan) = self.wifi_read(store, actor, |ctx| {
            let lan = crate::capture_setup::load(ctx)?.device == "lan";
            Ok((crate::capture::conf(ctx)?.is_some(), lan))
        })?;
        Ok(HostOp::WifiInstall(WifiRequest {
            address: p.address.clone(),
            name: p.name.clone(),
            https_port: p.https_port,
            http_port: p.http_port,
            computer: String::new(),
            firewall: firewall_rule(p.https_port, p.http_port),
            new_desktop_token: !has_conf,
            allow_lan: lan,
        }))
    }

    pub(crate) fn wifi_start(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        name: &str,
        args: Value,
    ) -> CommandResult<NetStep> {
        self.wifi_read(store, actor, |_| Ok(()))?;
        let c = self.connector.as_ref().ok_or_else(|| {
            CommandError::Rejected("this MoneyKeep front end cannot reach the network".into())
        })?;
        match name {
            "capture.wifi_addresses" => {
                job(c, Op::Addresses, Vec::new(), Some(HostOp::WifiAddresses))
            }
            "capture.wifi_apply" => {
                let a: ApplyArgs = crate::parse_args(args)?;
                let p = place(&a.address, a.name.as_deref(), a.https_port, a.http_port)?;
                let pin = match a.pin.as_deref() {
                    Some(pin) => Some(Zeroizing::new(api::validate_pin(pin).map_err(invalid)?)),
                    None => None,
                };
                let had_pin = self.wifi_read(store, actor, |ctx| Ok(load(ctx)?.pin_set))?;
                if pin.is_none() && !had_pin {
                    return Err(invalid(api::msg::PIN_RULE));
                }
                if needs_lookup(&p, a.name_anyway) {
                    return name_check(c, p, pin, false);
                }
                self.wifi_apply_go(store, actor, c, p, pin)
            }
            "capture.wifi_set_pin" => {
                let a: PinArgs = crate::parse_args(args)?;
                let pin = Zeroizing::new(api::validate_pin(&a.pin).map_err(invalid)?);
                let ep = self.wifi_read(store, actor, |ctx| crate::capture::endpoint(ctx, c))?;
                let body = serde_json::to_vec(&api::SetPin {
                    pin: pin.to_string(),
                })
                .map_err(|e| CommandError::Integrity(e.to_string()))?;
                job(
                    c,
                    Op::PinSet {
                        result: json!({"pin_set": true}),
                    },
                    vec![ep.req(HttpMethod::Put, "/pin", Some(body), MAX_JSON)],
                    None,
                )
            }
            "capture.wifi_pair_code" => {
                let (ep, w) = self.wifi_read(store, actor, |ctx| {
                    Ok((crate::capture::endpoint(ctx, c)?, load(ctx)?))
                })?;
                if !w.installed {
                    return Err(coded("not_set_up", "Set up Wi-Fi capture first."));
                }
                job(
                    c,
                    Op::PairCode {
                        host: w.host(),
                        https_port: w.https_port,
                        http_port: w.http_port,
                    },
                    vec![ep.req(HttpMethod::Post, "/pairing-codes", None, MAX_JSON)],
                    None,
                )
            }
            "capture.wifi_change_address" => {
                let a: ChangeArgs = crate::parse_args(args)?;
                // The capture server must be known before anything is looked up.
                let w = self.wifi_read(store, actor, |ctx| {
                    crate::capture::endpoint(ctx, c)?;
                    load(ctx)
                })?;
                if !w.installed {
                    return Err(coded("not_set_up", "Set up Wi-Fi capture first."));
                }
                let p = place(
                    &a.address,
                    a.name.as_deref(),
                    Some(w.https_port),
                    Some(w.http_port),
                )?;
                if needs_lookup(&p, a.name_anyway) {
                    return name_check(c, p, None, true);
                }
                self.wifi_change_go(store, actor, c, p)
            }
            "capture.wifi_remove" => {
                let ep = self.wifi_read(store, actor, |ctx| crate::capture::endpoint(ctx, c))?;
                job(
                    c,
                    Op::RemoveDevices,
                    vec![ep.req(HttpMethod::Get, "/devices", None, MAX_JSON)],
                    None,
                )
            }
            "capture.wifi_firewall_check" | "capture.wifi_firewall_fix" => {
                let fix = if name == "capture.wifi_firewall_fix" {
                    let a: FixArgs = crate::parse_args(args)?;
                    Some(match a.what.as_deref() {
                        None => (true, true),
                        Some("rule") => (true, false),
                        Some("block") => (false, true),
                        Some(o) => return Err(invalid(format!("what: {o} (rule or block)"))),
                    })
                } else {
                    None
                };
                let w = self.wifi_read(store, actor, load)?;
                if !w.installed {
                    if fix.is_some() {
                        return Err(coded("not_set_up", "Set up Wi-Fi capture first."));
                    }
                    return Ok(NetStep::Done(json!({"wifi": false, "problems": []})));
                }
                job(
                    c,
                    Op::FirewallChecked { fix },
                    Vec::new(),
                    Some(HostOp::FirewallCheck(api::FIREWALL_RULE.to_owned())),
                )
            }
            other => Err(CommandError::UnknownCommand(other.to_owned())),
        }
    }

    /// Apply, after the checks: record the choice, then turn Wi-Fi capture on.
    fn wifi_apply_go(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        c: &Connector,
        p: Place,
        pin: Option<Zeroizing<String>>,
    ) -> CommandResult<NetStep> {
        // The choice is recorded first, so a stopped setup resumes with it.
        let p2 = p.clone();
        self.wifi_write(
            store,
            actor,
            "capture.wifi_apply",
            json!({"address": p.address, "step": "address"}),
            &|_, w| {
                w.address.clone_from(&p2.address);
                w.name.clone_from(&p2.name);
                w.https_port = p2.https_port;
                w.http_port = p2.http_port;
                Ok(())
            },
        )?;
        let op = self.install_op(store, actor, &p)?;
        job(
            c,
            Op::Installed {
                place: p,
                pin,
                stale: None,
            },
            Vec::new(),
            Some(op),
        )
    }

    /// Change address, after the checks.
    fn wifi_change_go(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        c: &Connector,
        p: Place,
    ) -> CommandResult<NetStep> {
        let ep = self.wifi_read(store, actor, |ctx| crate::capture::endpoint(ctx, c))?;
        let op = self.install_op(store, actor, &p)?;
        job(
            c,
            Op::Installed {
                place: p,
                pin: None,
                stale: Some(Vec::new()),
            },
            vec![ep.req(HttpMethod::Get, "/devices", None, MAX_JSON)],
            Some(op),
        )
    }

    pub(crate) fn wifi_resume(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        c: &Connector,
        op: Op,
        responses: Vec<Result<HttpResponse, FetchError>>,
        host: Option<HostOut>,
    ) -> CommandResult<NetStep> {
        let now = c.now();
        match op {
            Op::Addresses => match host {
                Some(HostOut::Addresses(list)) => {
                    let list: Vec<_> = list
                        .into_iter()
                        .filter(|a| check_address(&a.address).is_ok())
                        .collect();
                    Ok(NetStep::Done(json!({
                        "addresses": list,
                        "why_fixed": text::WHY_FIXED,
                    })))
                }
                Some(HostOut::Failed(e)) => Err(coded("host_failed", e)),
                _ => Ok(NetStep::Done(json!({"addresses": []}))),
            },
            Op::NameChecked { place, pin, change } => {
                let found = match host {
                    Some(HostOut::Resolved(list)) => list,
                    Some(HostOut::Failed(e)) => return Err(coded("host_failed", e)),
                    _ => Vec::new(),
                };
                if !found.iter().any(|a| a.trim() == place.address) {
                    return Err(coded(
                        "name_unresolved",
                        text::name_unresolved(&place.address),
                    ));
                }
                if change {
                    self.wifi_change_go(store, actor, c, place)
                } else {
                    self.wifi_apply_go(store, actor, c, place, pin)
                }
            }
            Op::Installed { place, pin, stale } => {
                let o: WifiOutcome = match host {
                    Some(HostOut::WifiInstalled(o)) => o,
                    Some(HostOut::Failed(e)) => {
                        return Err(coded("install_failed", e));
                    }
                    _ => {
                        return Err(CommandError::Integrity(
                            "installation result missing".into(),
                        ));
                    }
                };
                // Change address: the Wi-Fi phones not seen for over a day.
                let stale_names: Option<Vec<String>> = stale.map(|_| {
                    responses
                        .first()
                        .and_then(|r| r.as_ref().ok())
                        .filter(|r| r.status == 200)
                        .map(|r| devices_of(&r.body))
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|d| d.via == api::VIA_WIFI)
                        .filter(|d| d.last_seen.unwrap_or(d.paired_at) < now - DAY)
                        .map(|d| d.name)
                        .collect()
                });
                let o2 = o.clone();
                let place2 = place.clone();
                let command = if stale_names.is_some() {
                    "capture.wifi_change_address"
                } else {
                    "capture.wifi_apply"
                };
                let w = self.wifi_write(
                    store,
                    actor,
                    command,
                    json!({"address": place.address, "step": "install"}),
                    &|ctx, w| {
                        if let Some(token) = &o2.desktop_token {
                            crate::capture::store_server(ctx, &o2.local_url, false, token)?;
                        }
                        w.address.clone_from(&place2.address);
                        w.name.clone_from(&place2.name);
                        w.https_port = place2.https_port;
                        w.http_port = place2.http_port;
                        w.ca_name.clone_from(&o2.ca_name);
                        w.fingerprint.clone_from(&o2.fingerprint);
                        w.ca_not_after = o2.ca_not_after;
                        w.cert_not_before = o2.cert_not_before;
                        w.cert_not_after = o2.cert_not_after;
                        if o2.firewall_added {
                            w.firewall_rule = Some(api::FIREWALL_RULE.to_owned());
                        }
                        // U1.7: the phones need the new certificate from now on.
                        if command == "capture.wifi_change_address" && o2.ca_created {
                            w.address_changed_at = Some(now);
                        }
                        w.installed = true;
                        w.answered = o2.answered;
                        w.check_error.clone_from(&o2.check_error);
                        for a in &o2.actions {
                            w.note(a.clone());
                        }
                        Ok(())
                    },
                )?;
                let (cert_url, app_url) = urls(&w);
                let result = json!({
                    "address": w.address,
                    "host": w.host(),
                    "fingerprint": w.fingerprint,
                    "ca_created": o.ca_created,
                    "answered": w.answered,
                    "check_error": w.check_error,
                    "firewall_added": o.firewall_added,
                    "cert_url": cert_url,
                    "app_url": app_url,
                    "actions": o.actions,
                    "stale_phones": stale_names,
                    "warning": stale_names.as_ref().map(|_| text::CHANGE_WARNING),
                });
                match pin {
                    Some(pin) => {
                        let ep =
                            self.wifi_read(store, actor, |ctx| crate::capture::endpoint(ctx, c))?;
                        let body = serde_json::to_vec(&api::SetPin {
                            pin: pin.to_string(),
                        })
                        .map_err(|e| CommandError::Integrity(e.to_string()))?;
                        job(
                            c,
                            Op::PinSet { result },
                            vec![ep.req(HttpMethod::Put, "/pin", Some(body), MAX_JSON)],
                            None,
                        )
                    }
                    None => Ok(NetStep::Done(result)),
                }
            }
            Op::PinSet { mut result } => {
                answer(responses.first(), &[200])?;
                self.wifi_write(
                    store,
                    actor,
                    "capture.wifi_set_pin",
                    json!({"pin_set": true}),
                    &|_, w| {
                        w.pin_set = true;
                        w.note("Capture PIN set (kept by the server as an Argon2id hash)".into());
                        Ok(())
                    },
                )?;
                if let Some(o) = result.as_object_mut() {
                    o.insert("pin_set".into(), json!(true));
                }
                Ok(NetStep::Done(result))
            }
            Op::PairCode {
                host,
                https_port,
                http_port,
            } => {
                let body = answer(responses.first(), &[200, 201])?;
                let p: api::PairingCode = serde_json::from_slice(&body).map_err(|e| {
                    CommandError::Rejected(format!("The answer was not understood: {e}"))
                })?;
                Ok(NetStep::Done(json!({
                    "code": p.code,
                    "expires_at": p.expires_at,
                    "expires_in": p.expires_in,
                    "cert_url": api::cert_page_url(&host, http_port),
                    "pair_url": api::wifi_pair_url(&host, https_port, &p.code),
                })))
            }
            Op::RemoveDevices => {
                let body = answer(responses.first(), &[200])?;
                let wifi: Vec<api::Device> = devices_of(&body)
                    .into_iter()
                    .filter(|d| d.via == api::VIA_WIFI && api::is_receipt_id(&d.id))
                    .collect();
                let (ep, w, lan) = self.wifi_read(store, actor, |ctx| {
                    Ok((
                        crate::capture::endpoint(ctx, c)?,
                        load(ctx)?,
                        crate::capture_setup::load(ctx)?.device == "lan",
                    ))
                })?;
                let reqs = wifi
                    .iter()
                    .map(|d| {
                        ep.req(
                            HttpMethod::Delete,
                            &format!("/devices/{}", d.id),
                            None,
                            MAX_JSON,
                        )
                    })
                    .collect();
                job(
                    c,
                    Op::Removed { phones: wifi.len() },
                    reqs,
                    Some(HostOp::WifiRemove(WifiRemove {
                        firewall_rule: w
                            .firewall_rule
                            .clone()
                            .unwrap_or_else(|| api::FIREWALL_RULE.to_owned()),
                        allow_lan: lan,
                    })),
                )
            }
            Op::Removed { phones } => {
                let done = match host {
                    Some(HostOut::Removed(d)) => d,
                    Some(HostOut::Failed(e)) => return Err(coded("remove_failed", e)),
                    _ => Vec::new(),
                };
                let disconnected = responses
                    .iter()
                    .filter(|r| matches!(r, Ok(x) if x.status == 200))
                    .count();
                self.wifi_write(
                    store,
                    actor,
                    "capture.wifi_remove",
                    json!({"phones": disconnected}),
                    &|_, w| {
                        *w = WifiRec::default();
                        Ok(())
                    },
                )?;
                Ok(NetStep::Done(json!({
                    "removed": true,
                    "phones": phones,
                    "phones_disconnected": disconnected,
                    "actions": done,
                })))
            }
            Op::FirewallChecked { fix } => {
                let f = match host {
                    Some(HostOut::Firewall(f)) => f,
                    Some(HostOut::Failed(e)) => return Err(coded("host_failed", e)),
                    _ => FirewallState::default(),
                };
                let Some((rule, block)) = fix else {
                    return Ok(NetStep::Done(firewall_answer(&f)));
                };
                // security-review-1 A6: no Fix while the server isn't installed
                // (Install the server again recreates the rule).
                if f.server == Some(ServiceState::NotInstalled) {
                    return Err(coded("not_installed", text::NOT_INSTALLED));
                }
                let w = self.wifi_read(store, actor, load)?;
                let req = FirewallFix {
                    add_rule: (rule && f.managed && f.rule != "ok")
                        .then(|| firewall_rule(w.https_port, w.http_port)),
                    remove_block_rules: if block && f.managed {
                        f.block_rules.iter().map(|r| r.id.clone()).collect()
                    } else {
                        Vec::new()
                    },
                };
                if req.add_rule.is_none() && req.remove_block_rules.is_empty() {
                    let mut v = firewall_answer(&f);
                    if let Some(o) = v.as_object_mut() {
                        o.insert("fixed".into(), json!(false));
                        o.insert("actions".into(), json!([]));
                    }
                    return Ok(NetStep::Done(v));
                }
                job(
                    c,
                    Op::FirewallFixed,
                    Vec::new(),
                    Some(HostOp::FirewallFix(req)),
                )
            }
            Op::FirewallFixed => {
                let done = match host {
                    Some(HostOut::Removed(d)) => d,
                    Some(HostOut::Failed(e)) if e == PERMISSION_DECLINED => {
                        return Err(coded("permission_declined", text::FIX_DECLINED));
                    }
                    Some(HostOut::Failed(e)) => return Err(coded("fix_failed", e)),
                    _ => Vec::new(),
                };
                let done2 = done.clone();
                self.wifi_write(
                    store,
                    actor,
                    "capture.wifi_firewall_fix",
                    json!({"actions": done.len()}),
                    &|_, w| {
                        w.firewall_rule = Some(api::FIREWALL_RULE.to_owned());
                        for a in &done2 {
                            w.note(a.clone());
                        }
                        Ok(())
                    },
                )?;
                Ok(NetStep::Done(json!({"fixed": true, "actions": done})))
            }
        }
    }
}

/// The firewall check's answer (revision 18): the facts and the problems in words,
/// each with what can be done (`fix`, or a `link` to Windows' settings).
pub(crate) fn firewall_answer(f: &FirewallState) -> Value {
    let mut problems = Vec::new();
    // security-review-1 A6: while the capture server isn't installed, the Status
    // card shows that (with Install the server again, which recreates the rule)
    // and no firewall problem.
    if f.server == Some(ServiceState::NotInstalled) {
        return json!({
            "wifi": true,
            "managed": f.managed,
            "rule": f.rule,
            "server_installed": false,
            "problems": problems,
        });
    }
    if f.managed && f.rule != "ok" {
        problems.push(json!({
            "code": "rule_missing",
            "text": if f.rule == "disabled" { text::RULE_DISABLED } else { text::RULE_MISSING },
            "fix": "rule",
        }));
    }
    if f.managed && !f.block_rules.is_empty() {
        problems.push(json!({
            "code": "blocked",
            "text": text::BLOCKED,
            "detail": text::BLOCKED_DETAIL,
            "fix": "block",
        }));
    }
    // macos-release.md C5: the Mac's application firewall is on; the desktop shows
    // this while no phone on Wi-Fi has reached the server lately.
    if f.app_firewall == Some(true) {
        problems.push(json!({
            "code": "macos_firewall",
            "text": text::MAC_FIREWALL,
            "detail": text::MAC_FIREWALL_DETAIL,
            "link": text::MAC_FIREWALL_SETTINGS,
            "link_label": text::MAC_FIREWALL_BUTTON,
        }));
    }
    if !f.public_networks.is_empty() {
        problems.push(json!({
            "code": "public_network",
            "text": text::PUBLIC,
            "detail": text::PUBLIC_DETAIL,
            "networks": f.public_networks,
            "link": text::NETWORK_SETTINGS,
        }));
    }
    json!({
        "wifi": true,
        "managed": f.managed,
        "rule": f.rule,
        "block_rules": f.block_rules.iter().map(|r| json!({"id": r.id, "name": r.display_name})).collect::<Vec<_>>(),
        "public_networks": f.public_networks,
        "error": f.error,
        "app_firewall": f.app_firewall,
        "problems": problems,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn checks() {
        assert!(check_address("192.168.1.20").is_ok());
        assert!(check_address("8.8.8.8").is_err());
        assert!(check_address("x").is_err());
        assert_eq!(
            check_name("MoneyKeep.home.arpa").unwrap(),
            "moneykeep.home.arpa"
        );
        assert!(check_name("bad name").is_err());
        let r = firewall_rule(14443, 14080);
        assert_eq!(r.profiles, vec!["Private".to_owned(), "Domain".to_owned()]);
        assert_eq!(r.name, "MoneyKeep capture (Wi-Fi)");
        assert!(place("192.168.1.20", None, Some(14443), Some(14443)).is_err());
        assert!(place("192.168.1.20", None, Some(80), None).is_err());
    }
}
