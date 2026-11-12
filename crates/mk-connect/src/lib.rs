//! MoneyKeep connectivity (connectivity.md): the only workspace crate that talks to
//! the network. It supplies the registry's injected [`Fetcher`] (HTTPS with the
//! allowed-host, redirect and size rules) and [`SecretStore`] (the operating system's
//! credential store); the providers' logic stays in the network-free crates. It also
//! supplies the phone capture setup's [`SystemHost`] (the capture server and
//! `cloudflared` services on this computer, mobile-capture.md revision 2).

mod host;

pub use host::SystemHost;

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use mk_commands::connect::{
    Connector, FetchError, Fetcher, HttpMethod, HttpRequest, HttpResponse, SecretStore,
};
use mk_core::brand;

/// Redirects within the allowed host followed at most this many times.
const MAX_REDIRECTS: usize = 3;
const TIMEOUT: Duration = Duration::from_secs(60);

/// Which certificates are trusted.
#[derive(Debug, Clone)]
pub enum Trust {
    /// The operating system's trust store.
    System,
    /// Only these certificates (DER) as roots (tests).
    Only(Vec<Vec<u8>>),
}

/// HTTPS requests for bank feeds.
#[derive(Debug, Clone)]
pub struct HttpsFetcher {
    trust: Trust,
    /// Use the proxy named by the environment (`HTTPS_PROXY`).
    env_proxy: bool,
}

impl HttpsFetcher {
    /// The production fetcher: OS trust store, environment proxy.
    pub fn system() -> Self {
        Self {
            trust: Trust::System,
            env_proxy: true,
        }
    }

    /// A fetcher trusting only `roots` and using no proxy (tests).
    pub fn trusting(roots: Vec<Vec<u8>>) -> Self {
        Self {
            trust: Trust::Only(roots),
            env_proxy: false,
        }
    }

    /// The agent; `plain_http` allows `http://` and uses no proxy (the phone capture
    /// server on the loopback address or a trusted LAN).
    fn agent_for(&self, plain_http: bool) -> ureq::Agent {
        let ders: Vec<Vec<u8>> = match &self.trust {
            Trust::System => system_roots(),
            Trust::Only(ders) => ders.clone(),
        };
        let roots = ureq::tls::RootCerts::Specific(Arc::new(
            ders.iter()
                .map(|d| ureq::tls::Certificate::from_der(d).to_owned())
                .collect(),
        ));
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(roots)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let mut cfg = ureq::Agent::config_builder()
            .tls_config(tls)
            .https_only(!plain_http)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(TIMEOUT));
        if !self.env_proxy || plain_http {
            cfg = cfg.proxy(None);
        }
        cfg.build().new_agent()
    }
}

/// The operating system's trusted roots (DER), loaded once.
fn system_roots() -> Vec<Vec<u8>> {
    static ROOTS: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();
    ROOTS
        .get_or_init(|| {
            rustls_native_certs::load_native_certs()
                .certs
                .into_iter()
                .map(|c| c.as_ref().to_vec())
                .collect()
        })
        .clone()
}

/// `host[:port]` of an `https://` URL, or of an `http://` URL when `http` is allowed.
fn authority_with(url: &str, http: bool) -> Option<String> {
    let rest = match url.strip_prefix("https://") {
        Some(r) => r,
        None if http => url.strip_prefix("http://")?,
        None => return None,
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let auth = rest.get(..end)?;
    if auth.contains('@') || auth.is_empty() {
        return None;
    }
    Some(auth.to_ascii_lowercase())
}

/// The target of a `Location` header relative to `base`.
fn resolve(base: &str, location: &str) -> Option<String> {
    let l = location.trim();
    if l.contains("://") {
        return Some(l.to_owned());
    }
    let rest = base.strip_prefix("https://")?;
    let end = rest.find('/').unwrap_or(rest.len());
    let origin = format!("https://{}", rest.get(..end)?);
    if l.starts_with("//") {
        return Some(format!("https:{l}"));
    }
    if l.starts_with('/') {
        return Some(format!("{origin}{l}"));
    }
    // A relative path: replace the last segment.
    let path = rest.get(end..).unwrap_or("/");
    let path = path.split('?').next().unwrap_or("/");
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
    Some(format!("{origin}{dir}/{l}"))
}

fn same_authority(a: &str, b: &str) -> bool {
    let strip = |s: &str| s.strip_suffix(":443").unwrap_or(s).to_owned();
    strip(a) == strip(b)
}

impl Fetcher for HttpsFetcher {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, FetchError> {
        let plain = req.allow_http && req.url.starts_with("http://");
        let agent = self.agent_for(plain);
        let mut url = req.url.clone();
        let auth = req.basic.as_ref().map(|(u, p)| {
            format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"))
            )
        });
        for hop in 0..=MAX_REDIRECTS {
            let Some(host) = authority_with(&url, plain) else {
                return Err(FetchError::Refused(
                    "only https:// addresses without credentials are allowed".into(),
                ));
            };
            if !same_authority(&host, &req.authority) {
                return Err(FetchError::Refused(format!(
                    "a request to {host} was refused: only {} is allowed",
                    req.authority
                )));
            }
            let result = match req.method {
                HttpMethod::Get => {
                    let mut r = agent.get(&url);
                    if let Some(a) = &auth {
                        r = r.header("Authorization", a);
                    }
                    for (k, v) in &req.headers {
                        r = r.header(k.as_str(), v.as_str());
                    }
                    r.call()
                }
                HttpMethod::Delete => {
                    let mut r = agent.delete(&url);
                    if let Some(a) = &auth {
                        r = r.header("Authorization", a);
                    }
                    for (k, v) in &req.headers {
                        r = r.header(k.as_str(), v.as_str());
                    }
                    r.call()
                }
                HttpMethod::Post | HttpMethod::Put => {
                    let mut r = if req.method == HttpMethod::Put {
                        agent.put(&url)
                    } else {
                        agent.post(&url)
                    };
                    if let Some(a) = &auth {
                        r = r.header("Authorization", a);
                    }
                    for (k, v) in &req.headers {
                        r = r.header(k.as_str(), v.as_str());
                    }
                    match &req.body {
                        Some(b) => r.send(&b[..]),
                        None => r.send_empty(),
                    }
                }
            };
            let mut resp = result.map_err(|e| FetchError::Network(describe(&e)))?;
            let status = resp.status().as_u16();
            if (300..400).contains(&status) {
                let location = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                let Some(next) = location.and_then(|l| resolve(&url, &l)) else {
                    return Err(FetchError::Refused("a redirect without a target".into()));
                };
                if hop == MAX_REDIRECTS {
                    return Err(FetchError::Refused("too many redirects".into()));
                }
                url = next;
                continue;
            }
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok());
            // A `Deprecation` header (RFC 9745): kept so the caller can log it.
            let deprecation = resp
                .headers()
                .get("deprecation")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.chars().take(200).collect::<String>());
            let limit = u64::try_from(req.max_bytes).unwrap_or(u64::MAX);
            if resp
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .is_some_and(|n| n > limit)
            {
                return Err(FetchError::Refused(format!(
                    "the answer is larger than {} MB",
                    req.max_bytes / (1024 * 1024)
                )));
            }
            let mut body = Vec::new();
            let mut reader = resp
                .body_mut()
                .with_config()
                .limit(limit.saturating_add(1))
                .reader()
                .take(limit.saturating_add(1));
            reader
                .read_to_end(&mut body)
                .map_err(|e| FetchError::Network(format!("the answer could not be read: {e}")))?;
            if body.len() > req.max_bytes {
                return Err(FetchError::Refused(format!(
                    "the answer is larger than {} MB",
                    req.max_bytes / (1024 * 1024)
                )));
            }
            return Ok(HttpResponse {
                status,
                body,
                retry_after,
                deprecation,
            });
        }
        Err(FetchError::Refused("too many redirects".into()))
    }
}

/// A transport error in words (never the request's credentials: they are not in the
/// URL, and headers are not printed).
fn describe(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Timeout(_) => "the provider did not answer in time".into(),
        ureq::Error::HostNotFound => "the provider's host was not found".into(),
        ureq::Error::Io(io) => format!("network error: {io}"),
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) => {
            "the secure connection failed (the certificate was not trusted)".into()
        }
        other => format!("network error: {other}"),
    }
}

/// The operating system's credential store (Keychain, Credential Manager, Secret
/// Service), under service `MoneyKeep`.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsSecrets;

impl OsSecrets {
    fn entry(id: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(brand::CREDENTIAL_SERVICE, id).map_err(|e| e.to_string())
    }
}

impl SecretStore for OsSecrets {
    fn available(&self) -> bool {
        // A store is usable when an entry can be read (missing is fine).
        match Self::entry("moneykeep-probe").and_then(|e| match e.get_password() {
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err.to_string()),
        }) {
            Ok(()) => true,
            Err(_) => false,
        }
    }
    fn put(&self, id: &str, secret: &str) -> Result<(), String> {
        Self::entry(id)?
            .set_password(secret)
            .map_err(|e| e.to_string())
    }
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        match Self::entry(id)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        match Self::entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// The user's UTC offset now (0 when the system cannot tell safely).
pub fn local_offset_secs() -> i32 {
    time::UtcOffset::current_local_offset().map_or(0, time::UtcOffset::whole_seconds)
}

/// The production connector: HTTPS with the OS trust store and the OS credential
/// store.
pub fn connector() -> Connector {
    Connector {
        fetcher: std::sync::Arc::new(HttpsFetcher::system()),
        secrets: Box::new(OsSecrets),
        utc_offset_secs: local_offset_secs(),
        now: None,
        host: Some(std::sync::Arc::new(SystemHost::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_targets() {
        assert_eq!(
            resolve("https://a.test/x/y?q=1", "/z").as_deref(),
            Some("https://a.test/z")
        );
        assert_eq!(
            resolve("https://a.test/x/y", "w").as_deref(),
            Some("https://a.test/x/w")
        );
        assert_eq!(
            resolve("https://a.test/x", "https://b.test/").as_deref(),
            Some("https://b.test/")
        );
        assert_eq!(authority_with("https://u:p@a.test/", false), None);
        assert_eq!(authority_with("http://a.test/", false), None);
        assert_eq!(
            authority_with("http://127.0.0.1:8787/x", true).as_deref(),
            Some("127.0.0.1:8787")
        );
        assert!(same_authority("a.test:443", "a.test"));
    }

    /// Phone capture setup requests go to api.cloudflare.com only.
    #[test]
    fn cloudflare_requests_stay_on_cloudflare() {
        let req = HttpRequest {
            url: "https://api.cloudflare.example/client/v4/accounts".into(),
            authority: mk_commands::capture_setup::CLOUDFLARE_HOST.into(),
            max_bytes: 1024,
            ..HttpRequest::default()
        };
        assert!(matches!(
            HttpsFetcher::trusting(Vec::new()).send(&req),
            Err(FetchError::Refused(m)) if m.contains("only api.cloudflare.com")
        ));
    }
}
