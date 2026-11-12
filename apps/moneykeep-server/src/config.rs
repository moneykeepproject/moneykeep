//! `server.toml` and the start-up safety checks (mobile-capture.md "Listen address"
//! and "Config").

use std::net::{IpAddr, SocketAddr};

use mk_capture_api::{
    DEFAULT_INBOX_LIMIT_MB, DEFAULT_LISTEN, DEFAULT_MAX_RECEIPT_MB,
    DEFAULT_MAX_RECEIPTS_PER_EXPENSE,
};
use serde::Deserialize;

pub const CONFIG_FILE: &str = "server.toml";

/// The settings in `server.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// `host:port`; anything but a loopback address needs `--allow-lan`.
    #[serde(default = "default_listen")]
    pub listen: String,
    /// Check Cloudflare Access tokens. `false` is allowed only on loopback (testing).
    #[serde(default = "yes")]
    pub access_required: bool,
    /// The Access team domain, e.g. `yourteam.cloudflareaccess.com`.
    #[serde(default)]
    pub access_team_domain: String,
    /// The Application Audience (AUD) tag of the API's Access application.
    #[serde(default)]
    pub access_aud: String,
    #[serde(default = "default_receipt_mb")]
    pub max_receipt_mb: u32,
    #[serde(default = "default_receipts")]
    pub max_receipts_per_expense: usize,
    #[serde(default = "default_inbox_mb")]
    pub inbox_limit_mb: u64,
    /// The phone page's address, shown with pairing codes (optional).
    #[serde(default)]
    pub client_url: Option<String>,
    /// The Client ID of the Access service token "MoneyKeep desktop status"; when
    /// set, a service-token identity must carry this ID (optional, not a secret).
    #[serde(default)]
    pub access_service_client_id: String,
    /// Wi-Fi capture (mobile-capture.md revision 10): the IPv4 address phones use on
    /// the home or office network; empty when Wi-Fi capture is off.
    #[serde(default)]
    pub wifi_address: String,
    /// The optional local name for that address (e.g. `moneykeep.home.arpa`).
    #[serde(default)]
    pub wifi_name: String,
    #[serde(default = "default_https_port")]
    pub wifi_https_port: u16,
    #[serde(default = "default_http_port")]
    pub wifi_http_port: u16,
}

fn default_https_port() -> u16 {
    mk_capture_api::DEFAULT_WIFI_HTTPS_PORT
}
fn default_http_port() -> u16 {
    mk_capture_api::DEFAULT_WIFI_HTTP_PORT
}

fn default_listen() -> String {
    DEFAULT_LISTEN.to_owned()
}
fn yes() -> bool {
    true
}
fn default_receipt_mb() -> u32 {
    DEFAULT_MAX_RECEIPT_MB
}
fn default_receipts() -> usize {
    DEFAULT_MAX_RECEIPTS_PER_EXPENSE
}
fn default_inbox_mb() -> u64 {
    DEFAULT_INBOX_LIMIT_MB
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            access_required: true,
            access_team_domain: String::new(),
            access_aud: String::new(),
            max_receipt_mb: DEFAULT_MAX_RECEIPT_MB,
            max_receipts_per_expense: DEFAULT_MAX_RECEIPTS_PER_EXPENSE,
            inbox_limit_mb: DEFAULT_INBOX_LIMIT_MB,
            client_url: None,
            access_service_client_id: String::new(),
            wifi_address: String::new(),
            wifi_name: String::new(),
            wifi_https_port: default_https_port(),
            wifi_http_port: default_http_port(),
        }
    }
}

/// The text `init` writes (comments explain each key).
pub fn template() -> String {
    format!(
        "# MoneyKeep capture server settings (see docs/deploy/phone-capture.md).\n\
         \n\
         # Where the server listens. Keep the loopback address: cloudflared connects to it.\n\
         listen = \"{DEFAULT_LISTEN}\"\n\
         \n\
         # Cloudflare Access: every phone request must carry a valid Access token.\n\
         access_required = true\n\
         # Your team domain, e.g. \"yourteam.cloudflareaccess.com\".\n\
         access_team_domain = \"\"\n\
         # The Application Audience (AUD) tag of the API hostname's Access application.\n\
         access_aud = \"\"\n\
         \n\
         # Limits.\n\
         max_receipt_mb = {DEFAULT_MAX_RECEIPT_MB}\n\
         max_receipts_per_expense = {DEFAULT_MAX_RECEIPTS_PER_EXPENSE}\n\
         inbox_limit_mb = {DEFAULT_INBOX_LIMIT_MB}\n\
         \n\
         # The phone page's address, shown on the computer when pairing a phone.\n\
         # client_url = \"https://capture.example.com\"\n"
    )
}

fn quoted(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `server.toml` text for these settings (the guided setup's `configure`).
pub fn render(c: &Config) -> String {
    let mut text = format!(
        "# MoneyKeep capture server settings, written by MoneyKeep's phone capture setup\n\
         # (see docs/deploy/phone-capture.md).\n\
         \n\
         listen = {}\n\
         access_required = {}\n\
         access_team_domain = {}\n\
         access_aud = {}\n\
         access_service_client_id = {}\n\
         max_receipt_mb = {}\n\
         max_receipts_per_expense = {}\n\
         inbox_limit_mb = {}\n",
        quoted(&c.listen),
        c.access_required,
        quoted(&c.access_team_domain),
        quoted(&c.access_aud),
        quoted(&c.access_service_client_id),
        c.max_receipt_mb,
        c.max_receipts_per_expense,
        c.inbox_limit_mb,
    );
    if let Some(u) = &c.client_url {
        text.push_str(&format!("client_url = {}\n", quoted(u)));
    }
    if !c.wifi_address.is_empty() {
        text.push_str(&format!(
            "\n# Wi-Fi capture: phones on this network use https://<address>:<port>.\n\
             wifi_address = {}\nwifi_name = {}\nwifi_https_port = {}\nwifi_http_port = {}\n",
            quoted(&c.wifi_address),
            quoted(&c.wifi_name),
            c.wifi_https_port,
            c.wifi_http_port
        ));
    }
    text
}

impl Config {
    /// Parse `server.toml` text.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| format!("{CONFIG_FILE} is not valid: {e}"))
    }

    /// The team domain without a scheme or trailing slash.
    pub fn team_domain(&self) -> String {
        let t = self.access_team_domain.trim();
        let t = t.strip_prefix("https://").unwrap_or(t);
        t.trim_end_matches('/').to_ascii_lowercase()
    }

    /// Check the settings against the listen address; returns the address.
    pub fn check(&self, allow_lan: bool) -> Result<SocketAddr, String> {
        let addr: SocketAddr = self.listen.trim().parse().map_err(|_| {
            format!(
                "listen = {:?} is not an address such as 127.0.0.1:8787",
                self.listen
            )
        })?;
        let loopback = is_loopback(addr.ip());
        if !loopback && !allow_lan {
            return Err(format!(
                "Refusing to listen on {addr}: only loopback addresses (127.0.0.1) are allowed \
                 without --allow-lan. cloudflared connects to the loopback address."
            ));
        }
        if !self.access_required && !loopback {
            return Err(
                "access_required = false is allowed only on a loopback address (local testing)."
                    .into(),
            );
        }
        if self.access_required {
            let domain = self.team_domain();
            if domain.is_empty()
                || !domain
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
            {
                return Err("access_team_domain must be your Cloudflare Access team domain, e.g. yourteam.cloudflareaccess.com.".into());
            }
            if self.access_aud.trim().is_empty() {
                return Err("access_aud must be the API application's Audience (AUD) tag.".into());
            }
        }
        if self.max_receipt_mb == 0 || self.max_receipt_mb > 50 {
            return Err("max_receipt_mb must be between 1 and 50.".into());
        }
        if self.max_receipts_per_expense == 0 || self.max_receipts_per_expense > 50 {
            return Err("max_receipts_per_expense must be between 1 and 50.".into());
        }
        if self.inbox_limit_mb == 0 {
            return Err("inbox_limit_mb must be more than 0.".into());
        }
        self.wifi()?;
        Ok(addr)
    }

    /// Wi-Fi capture's address and ports, when it is on (revision 10).
    pub fn wifi(&self) -> Result<Option<WifiListen>, String> {
        if self.wifi_address.trim().is_empty() {
            return Ok(None);
        }
        let ip = crate::wifi::check_address(&self.wifi_address)?;
        let name = crate::wifi::check_name(&self.wifi_name)?;
        for p in [self.wifi_https_port, self.wifi_http_port] {
            if p < 1024 {
                return Err(format!("wifi port {p} must be between 1024 and 65535."));
            }
        }
        if self.wifi_https_port == self.wifi_http_port {
            return Err("wifi_https_port and wifi_http_port must differ.".into());
        }
        Ok(Some(WifiListen {
            target: crate::wifi::Target { ip, name },
            https_port: self.wifi_https_port,
            http_port: self.wifi_http_port,
        }))
    }

    pub fn max_receipt_bytes(&self) -> usize {
        usize::try_from(self.max_receipt_mb)
            .unwrap_or(10)
            .saturating_mul(1024 * 1024)
    }

    pub fn inbox_limit_bytes(&self) -> u64 {
        self.inbox_limit_mb.saturating_mul(1024 * 1024)
    }
}

/// Wi-Fi capture's listeners (from `server.toml`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiListen {
    pub target: crate::wifi::Target,
    pub https_port: u16,
    pub http_port: u16,
}

/// Wi-Fi capture's settings in this `server.toml` text: on at `address` (with
/// `name` and the ports), or off (`None`). Other settings are kept. Without a
/// Cloudflare setup the loopback listener serves only this computer, so the Access
/// check is off there (it needs a team domain and AUD tag).
pub fn with_wifi(text: &str, wifi: Option<(&str, &str, u16, u16)>) -> Result<String, String> {
    let mut c = if text.trim().is_empty() {
        Config::default()
    } else {
        Config::parse(text)?
    };
    match wifi {
        Some((address, name, https, http)) => {
            c.wifi_address = address.trim().to_owned();
            c.wifi_name = name.trim().to_owned();
            c.wifi_https_port = https;
            c.wifi_http_port = http;
        }
        None => {
            c.wifi_address = String::new();
            c.wifi_name = String::new();
        }
    }
    if c.access_team_domain.trim().is_empty() && c.access_aud.trim().is_empty() {
        c.access_required = false;
    }
    let allow_lan = c
        .listen
        .parse::<SocketAddr>()
        .is_ok_and(|a| !a.ip().is_loopback());
    c.check(allow_lan)?;
    Ok(render(&c))
}

/// Make `access_aud` in this `server.toml` text `aud` (mobile-capture.md revision 8,
/// Check): the new text, or `None` when it already is. Other settings are kept.
pub fn with_aud(text: &str, aud: &str) -> Result<Option<String>, String> {
    let aud = aud.trim();
    if aud.is_empty()
        || !aud
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("The AUD tag must be letters, digits and dashes.".into());
    }
    let mut c = Config::parse(text)?;
    if c.access_aud.trim() == aud {
        return Ok(None);
    }
    c.access_aud = aud.to_owned();
    Ok(Some(render(&c)))
}

pub fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn cfg(listen: &str, access: bool) -> Config {
        Config {
            listen: listen.into(),
            access_required: access,
            access_team_domain: "team.cloudflareaccess.com".into(),
            access_aud: "aud".into(),
            ..Config::default()
        }
    }

    /// mobile-capture.md AT9 (the checks; the server test starts real listeners).
    #[test]
    fn start_up_checks() {
        assert!(cfg("127.0.0.1:8787", true).check(false).is_ok());
        assert!(cfg("0.0.0.0:8787", true).check(false).is_err());
        assert!(cfg("0.0.0.0:8787", true).check(true).is_ok());
        assert!(cfg("0.0.0.0:8787", false).check(true).is_err());
        assert!(cfg("127.0.0.1:8787", false).check(false).is_ok());
        let mut c = cfg("127.0.0.1:8787", true);
        c.access_aud = String::new();
        assert!(c.check(false).is_err());
        let parsed = Config::parse(&template()).unwrap();
        assert_eq!(parsed.listen, "127.0.0.1:8787");
        assert!(Config::parse("nonsense = 1").is_err());
        let c = Config {
            listen: "192.168.1.20:8787".into(),
            access_team_domain: "team.cloudflareaccess.com".into(),
            access_aud: "aud\"tag".into(),
            access_service_client_id: "abc.access".into(),
            client_url: Some("https://capture.example.com".into()),
            ..Config::default()
        };
        assert_eq!(Config::parse(&render(&c)).unwrap(), c);
        // Revision 8: only `access_aud` changes; the same AUD writes nothing.
        let text = render(&c);
        let new = with_aud(&text, "0123abcd-new").unwrap().unwrap();
        let back = Config::parse(&new).unwrap();
        assert_eq!(back.access_aud, "0123abcd-new");
        assert_eq!(
            Config {
                access_aud: c.access_aud.clone(),
                ..back
            },
            c
        );
        assert_eq!(with_aud(&new, "0123abcd-new").unwrap(), None);
        assert!(with_aud(&new, "bad\"aud").is_err());
        assert!(with_aud(&new, " ").is_err());
        // Revision 10: Wi-Fi settings round-trip; a fresh file turns Access off on
        // loopback (no Cloudflare), a Cloudflare one keeps it.
        let w = with_wifi(
            "",
            Some(("192.168.1.20", "moneykeep.home.arpa", 14443, 14080)),
        )
        .unwrap();
        let back = Config::parse(&w).unwrap();
        assert!(!back.access_required);
        let wl = back.wifi().unwrap().unwrap();
        assert_eq!(wl.target.ip.to_string(), "192.168.1.20");
        assert_eq!(wl.target.name.as_deref(), Some("moneykeep.home.arpa"));
        assert_eq!((wl.https_port, wl.http_port), (14443, 14080));
        let cf = with_wifi(&new, Some(("10.0.0.5", "", 15000, 15001))).unwrap();
        let back = Config::parse(&cf).unwrap();
        assert!(back.access_required);
        assert_eq!(back.access_aud, "0123abcd-new");
        assert!(with_wifi(&cf, Some(("8.8.8.8", "", 14443, 14080))).is_err());
        assert!(with_wifi(&cf, Some(("10.0.0.5", "", 14443, 14443))).is_err());
        assert!(with_wifi(&cf, Some(("10.0.0.5", "", 80, 14080))).is_err());
        let off = Config::parse(&with_wifi(&cf, None).unwrap()).unwrap();
        assert!(off.wifi().unwrap().is_none());
        assert_eq!(
            Config {
                access_team_domain: "https://Team.cloudflareaccess.com/".into(),
                ..Config::default()
            }
            .team_domain(),
            "team.cloudflareaccess.com"
        );
    }
}
