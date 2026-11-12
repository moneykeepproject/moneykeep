//! The data folder's setup steps: `init`, `configure`, `set-aud` and the `wifi`
//! commands, shared by the command line and the service helper.
//!
//! security-review-1 A1: on Windows the data folder belongs to the service's own
//! account and the desktop cannot open it, so the desktop asks the helper (`services
//! apply`) to do these steps in the same elevated run that installs the service.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{self, CONFIG_FILE};
use crate::{Clock, Config, KeyHome, store, wifi};

/// `configure`: server.toml's Access and address settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureStep {
    pub team_domain: String,
    pub aud: String,
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default)]
    pub client_url: Option<String>,
    #[serde(default)]
    pub service_client_id: String,
}

fn default_listen() -> String {
    mk_capture_api::DEFAULT_LISTEN.to_owned()
}

/// `wifi configure`: Wi-Fi capture at an address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WifiStep {
    pub address: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_https")]
    pub https_port: u16,
    #[serde(default = "default_http")]
    pub http_port: u16,
    #[serde(default)]
    pub computer: Option<String>,
}

fn default_https() -> u16 {
    mk_capture_api::DEFAULT_WIFI_HTTPS_PORT
}

fn default_http() -> u16 {
    mk_capture_api::DEFAULT_WIFI_HTTP_PORT
}

/// Create the data folder (owner-only on Unix) and `server.toml` when missing.
pub fn ensure_folder(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let cfg = dir.join(CONFIG_FILE);
    if !cfg.exists() {
        store::write_private(&cfg, config::template().as_bytes())?;
    }
    // upgrade-resilience-1 U1.4: the data folder's format number.
    crate::upgrade::write_format(dir)
}

/// Set the desktop token by its SHA-256 hash (the desktop made the token; only the
/// hash reaches the helper, so no token is ever written to a file). The old desktop
/// token stops working.
pub fn set_desktop_token_hash(
    dir: &Path,
    key_home: KeyHome,
    hash_hex: &str,
    now: i64,
) -> Result<(), String> {
    let hash = parse_hash(hash_hex).ok_or("the desktop token's hash is not valid")?;
    ensure_folder(dir)?;
    let s = store::Store::open(dir, key_home)?;
    s.set_desktop_token(&hash, now)
}

/// 64 hexadecimal digits as 32 bytes.
pub fn parse_hash(hex: &str) -> Option<Vec<u8>> {
    let h = hex.trim();
    if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..32)
        .map(|i| {
            h.get(i * 2..i * 2 + 2)
                .and_then(|b| u8::from_str_radix(b, 16).ok())
        })
        .collect()
}

/// `configure`.
pub fn configure(data: &Path, step: &ConfigureStep) -> Result<(), String> {
    let path = data.join(CONFIG_FILE);
    let mut config = match std::fs::read_to_string(&path) {
        Ok(text) => Config::parse(&text)?,
        Err(_) => Config::default(),
    };
    config.access_required = true;
    config.access_team_domain.clone_from(&step.team_domain);
    config.access_aud.clone_from(&step.aud);
    config.listen.clone_from(&step.listen);
    config.client_url.clone_from(&step.client_url);
    config
        .access_service_client_id
        .clone_from(&step.service_client_id);
    let allow_lan = config
        .listen
        .parse::<std::net::SocketAddr>()
        .is_ok_and(|a| !a.ip().is_loopback());
    config.check(allow_lan)?;
    store::write_private(&path, config::render(&config).as_bytes())
}

/// `set-aud`: whether server.toml changed.
pub fn set_aud(data: &Path, aud: &str) -> Result<bool, String> {
    let path = data.join(CONFIG_FILE);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    match config::with_aud(&text, aud)? {
        Some(new) => {
            store::write_private(&path, new.as_bytes())?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// The Wi-Fi ports in server.toml, when Wi-Fi capture is on there.
pub fn wifi_ports(data: &Path) -> Option<(u16, u16)> {
    let text = std::fs::read_to_string(data.join(CONFIG_FILE)).ok()?;
    let c = Config::parse(&text).ok()?;
    if c.wifi_address.trim().is_empty() {
        return None;
    }
    Some((c.wifi_https_port, c.wifi_http_port))
}

/// `wifi configure`: what was made, as JSON.
pub fn wifi_configure(data: &Path, step: &WifiStep, key_home: KeyHome) -> Result<Value, String> {
    let ip = wifi::check_address(&step.address)?;
    let name = wifi::check_name(&step.name)?;
    if !wifi::address_present(ip) {
        return Err(format!(
            "This computer does not have the address {ip} now. Check that it is connected to the network, or choose another address."
        ));
    }
    let ports = (step.https_port, step.http_port);
    let path = data.join(CONFIG_FILE);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let current = Config::parse(&text)
        .ok()
        .and_then(|c| c.wifi().ok().flatten());
    // A port our own running server holds is not "in use by another program".
    for (port, other) in [(ports.0, ports.1), (ports.1, ports.0)] {
        let ours = current
            .as_ref()
            .is_some_and(|c| c.target.ip == ip && (c.https_port == port || c.http_port == port));
        if !ours {
            wifi::check_port(ip, port, other)?;
        }
    }
    let new = config::with_wifi(
        &text,
        Some((
            &ip.to_string(),
            name.as_deref().unwrap_or(""),
            ports.0,
            ports.1,
        )),
    )?;
    let secrets = crate::secrets::Secrets::open(data, key_home, None)?;
    let computer = step
        .computer
        .as_ref()
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(wifi::computer_name);
    let target = wifi::Target { ip, name };
    let m = wifi::ensure(data, &secrets, &target, &computer, Clock::default().now())?;
    store::write_private(&path, new.as_bytes())?;
    Ok(json!({
        "address": ip.to_string(),
        "name": target.name,
        "https_port": ports.0,
        "http_port": ports.1,
        "ca_name": m.info.ca_name,
        "fingerprint": m.info.fingerprint,
        "ca_created": m.ca_created,
        "renewed": m.renewed,
        "ca_not_after": m.info.ca_not_after,
        "cert_not_before": m.info.cert_not_before,
        "cert_not_after": m.info.cert_not_after,
        "keys": secrets.location(wifi::CA_KEY),
    }))
}

/// `wifi remove`: what was done.
pub fn wifi_remove(data: &Path, key_home: KeyHome) -> Result<Vec<String>, String> {
    let path = data.join(CONFIG_FILE);
    let mut actions = Vec::new();
    if let Ok(text) = std::fs::read_to_string(&path) {
        let new = config::with_wifi(&text, None)?;
        store::write_private(&path, new.as_bytes())?;
        actions.push("Wi-Fi capture turned off in server.toml".to_owned());
    }
    let secrets = crate::secrets::Secrets::open(data, key_home, None)?;
    actions.extend(wifi::remove(data, &secrets));
    Ok(actions)
}

/// `wifi check`: `{"ok": bool, "error": …}`.
pub fn wifi_check(data: &Path, wait: u64) -> Result<Value, String> {
    let text = std::fs::read_to_string(data.join(CONFIG_FILE))
        .map_err(|e| format!("cannot read server.toml: {e}"))?;
    let w = Config::parse(&text)?
        .wifi()?
        .ok_or("Wi-Fi capture is not turned on in server.toml")?;
    let ca = std::fs::read_to_string(data.join(wifi::CA_CERT_FILE))
        .ok()
        .and_then(|t| wifi::from_pem(&t))
        .ok_or("the Wi-Fi certificate authority is missing")?;
    let addr = std::net::SocketAddr::from((w.target.ip, w.https_port));
    let started = std::time::Instant::now();
    let result = loop {
        let r = wifi::https_get(
            addr,
            &w.target.ip.to_string(),
            &ca,
            mk_capture_api::HEALTH_PATH,
        );
        match r {
            Ok((200, _)) => break Ok(()),
            Ok((s, _)) => break Err(format!("the server answered HTTP {s}")),
            Err(e) if started.elapsed().as_secs() >= wait => break Err(e),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(500)),
        }
    };
    Ok(json!({ "ok": result.is_ok(), "error": result.err() }))
}
