//! Cloudflare Access token checks.
//!
//! Written from Cloudflare's "Validate JWTs" documentation
//! (https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/validating-json/):
//! the token arrives in the `Cf-Access-Jwt-Assertion` header; it is an RS256 JWS
//! (RFC 7515/7519) whose key is published at
//! `https://<team domain>/cdn-cgi/access/certs` (a JWK set under `keys`, matched by
//! `kid`); `iss` must be the team domain and `aud` must contain the application's AUD
//! tag. Keys rotate, so they are fetched at run time and fetched again when a token
//! names an unknown `kid` (at most once a minute).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use base64::Engine;
use mk_capture_api::{CERTS_REFRESH_SECS, JWT_LEEWAY_SECS};
use serde::Deserialize;

/// Where the JWK set comes from.
pub trait CertsSource: Send + Sync {
    fn fetch(&self) -> Result<Vec<u8>, String>;
}

/// Why a token was refused (the caller only learns that it was refused,
/// mobile-capture.md revision 8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JwtError {
    Malformed,
    Algorithm,
    UnknownKey,
    Signature,
    Issuer,
    /// The token's `aud` does not contain the configured AUD tag: what was expected
    /// and what the token carried (AUD tags are identifiers, not secrets; revision 8).
    Audience {
        expected: String,
        got: Vec<String>,
    },
    /// Expired or not yet valid beyond the leeway (often a wrong clock).
    Time,
    /// Neither a person's email nor a service token's Client ID.
    Identity,
    Certs(String),
}

impl JwtError {
    /// A log line (no token content).
    pub fn describe(&self) -> String {
        match self {
            Self::Malformed => "access token malformed".into(),
            Self::Algorithm => "access token algorithm is not RS256".into(),
            Self::UnknownKey => "access token signed by an unknown key".into(),
            Self::Signature => "access token signature invalid".into(),
            Self::Issuer => "access token issuer mismatch".into(),
            Self::Audience { expected, got } => {
                let got = if got.is_empty() {
                    "none".to_owned()
                } else {
                    got.join(", ")
                };
                let expected = if expected.is_empty() {
                    "(access_aud is empty)"
                } else {
                    expected.as_str()
                };
                format!("access token audience mismatch: expected {expected}, got {got}")
            }
            Self::Time => format!(
                "access token outside its validity time (beyond {JWT_LEEWAY_SECS} s leeway) - check this computer's clock"
            ),
            Self::Identity => "access token names neither a person nor a service token".into(),
            Self::Certs(e) => format!("access certificates could not be fetched: {e}"),
        }
    }
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Aud {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Claims {
    #[serde(default)]
    iss: Option<String>,
    #[serde(default)]
    aud: Option<Aud>,
    #[serde(default)]
    exp: Option<i64>,
    #[serde(default)]
    nbf: Option<i64>,
    #[serde(default)]
    email: Option<String>,
    /// A service token's Client ID (absent for people).
    #[serde(default)]
    common_name: Option<String>,
}

/// Who an Access token stands for. Cloudflare's "Application token" documentation
/// (https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/application-token/)
/// says a person's token carries `email`, while a service token's carries
/// `common_name` (its Client ID) and no `email`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// A person who signed in (the phone).
    User,
    /// A service token, with its Client ID (the desktop's status check).
    Service(String),
}

#[derive(Deserialize)]
struct Jwk {
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    kty: String,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
}

#[derive(Deserialize)]
struct JwkSet {
    #[serde(default)]
    keys: Vec<Jwk>,
}

#[derive(Default)]
struct Cache {
    keys: BTreeMap<String, (Vec<u8>, Vec<u8>)>,
    /// When the certificates were last fetched (Unix seconds).
    fetched_at: Option<i64>,
}

/// Verifies Access tokens for one team and application.
pub struct Verifier {
    issuer: String,
    aud: String,
    source: Arc<dyn CertsSource>,
    cache: Mutex<Cache>,
}

impl std::fmt::Debug for Verifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verifier")
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}

/// An identifier from a token, fit for a log line: printable ASCII only, at most
/// 80 characters (a signed token is still the sender's text).
fn log_safe(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_graphic())
        .take(80)
        .collect()
}

fn b64(s: &str) -> Result<Vec<u8>, JwtError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .map_err(|_| JwtError::Malformed)
}

impl Verifier {
    pub fn new(team_domain: &str, aud: &str, source: Arc<dyn CertsSource>) -> Self {
        Self {
            issuer: format!("https://{team_domain}"),
            aud: aud.trim().to_owned(),
            source,
            cache: Mutex::new(Cache::default()),
        }
    }

    /// Fetch the certificates again (at most once a minute unless `force`).
    fn refresh(&self, now: i64, force: bool) -> Result<(), JwtError> {
        {
            let c = self.cache.lock().map_err(|_| JwtError::UnknownKey)?;
            if !force && c.fetched_at.is_some_and(|t| now - t < CERTS_REFRESH_SECS) {
                return Ok(());
            }
        }
        let body = self.source.fetch().map_err(JwtError::Certs)?;
        let set: JwkSet =
            serde_json::from_slice(&body).map_err(|e| JwtError::Certs(e.to_string()))?;
        let mut keys = BTreeMap::new();
        for k in set.keys.into_iter().filter(|k| k.kty == "RSA") {
            if let (Some(kid), Some(n), Some(e)) = (k.kid, k.n, k.e)
                && let (Ok(n), Ok(e)) = (b64(&n), b64(&e))
            {
                keys.insert(kid, (n, e));
            }
        }
        let mut c = self.cache.lock().map_err(|_| JwtError::UnknownKey)?;
        c.keys = keys;
        c.fetched_at = Some(now);
        Ok(())
    }

    fn key(&self, kid: &str) -> Option<(Vec<u8>, Vec<u8>)> {
        self.cache.lock().ok()?.keys.get(kid).cloned()
    }

    /// Check a token at time `now` (Unix seconds); returns who it stands for.
    pub fn verify(&self, token: &str, now: i64) -> Result<Identity, JwtError> {
        let mut parts = token.trim().split('.');
        let (Some(h), Some(p), Some(s), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(JwtError::Malformed);
        };
        if token.len() > 16 * 1024 {
            return Err(JwtError::Malformed);
        }
        let header: Header = serde_json::from_slice(&b64(h)?).map_err(|_| JwtError::Malformed)?;
        if header.alg != "RS256" {
            return Err(JwtError::Algorithm);
        }
        let kid = header.kid.ok_or(JwtError::UnknownKey)?;
        let first = self
            .cache
            .lock()
            .map(|c| c.fetched_at.is_none())
            .unwrap_or(true);
        if first {
            self.refresh(now, true)?;
        }
        let (n, e) = match self.key(&kid) {
            Some(k) => k,
            None => {
                // A rotated key: fetch again, at most once a minute.
                self.refresh(now, false)?;
                self.key(&kid).ok_or(JwtError::UnknownKey)?
            }
        };
        let sig = b64(s)?;
        let signing_input = token
            .trim()
            .get(..h.len() + 1 + p.len())
            .ok_or(JwtError::Malformed)?;
        ring::signature::RsaPublicKeyComponents { n: &n, e: &e }
            .verify(
                &ring::signature::RSA_PKCS1_2048_8192_SHA256,
                signing_input.as_bytes(),
                &sig,
            )
            .map_err(|_| JwtError::Signature)?;
        let claims: Claims = serde_json::from_slice(&b64(p)?).map_err(|_| JwtError::Malformed)?;
        if claims.iss.as_deref().map(|i| i.trim_end_matches('/')) != Some(self.issuer.as_str()) {
            return Err(JwtError::Issuer);
        }
        let aud_ok = match &claims.aud {
            Some(Aud::One(a)) => *a == self.aud,
            Some(Aud::Many(v)) => v.contains(&self.aud),
            None => false,
        };
        if !aud_ok || self.aud.is_empty() {
            let got = match claims.aud {
                Some(Aud::One(a)) => vec![a],
                Some(Aud::Many(v)) => v,
                None => Vec::new(),
            };
            return Err(JwtError::Audience {
                expected: self.aud.clone(),
                got: got.iter().take(4).map(|a| log_safe(a)).collect(),
            });
        }
        let exp = claims.exp.ok_or(JwtError::Time)?;
        if now > exp + JWT_LEEWAY_SECS {
            return Err(JwtError::Time);
        }
        if claims.nbf.is_some_and(|nbf| now + JWT_LEEWAY_SECS < nbf) {
            return Err(JwtError::Time);
        }
        let email = claims.email.as_deref().map(str::trim).unwrap_or("");
        match claims.common_name.as_deref().map(str::trim) {
            Some(cn) if email.is_empty() && !cn.is_empty() => Ok(Identity::Service(cn.to_owned())),
            _ if !email.is_empty() => Ok(Identity::User),
            _ => Err(JwtError::Identity),
        }
    }
}

/// The certificates fetched over HTTPS from the team domain (OS trust store). Plain
/// `http://` is accepted only for a loopback host (tests).
#[derive(Debug, Clone)]
pub struct HttpCerts {
    pub url: String,
}

impl HttpCerts {
    pub fn for_team(team_domain: &str) -> Self {
        Self {
            url: format!("https://{team_domain}/cdn-cgi/access/certs"),
        }
    }
}

fn loopback_http(url: &str) -> bool {
    url.strip_prefix("http://")
        .map(|r| r.split(['/', '?']).next().unwrap_or(""))
        .is_some_and(|auth| {
            let host = auth.rsplit_once(':').map_or(auth, |(h, _)| h);
            host == "127.0.0.1" || host == "localhost" || host == "[::1]"
        })
}

impl CertsSource for HttpCerts {
    fn fetch(&self) -> Result<Vec<u8>, String> {
        use std::io::Read;
        let plain = loopback_http(&self.url);
        if !plain && !self.url.starts_with("https://") {
            return Err("the certificates address must be https".into());
        }
        let roots: Vec<ureq::tls::Certificate<'static>> = rustls_native_certs::load_native_certs()
            .certs
            .into_iter()
            .map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned())
            .collect();
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(ureq::tls::RootCerts::Specific(Arc::new(roots)))
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let mut cfg = ureq::Agent::config_builder()
            .tls_config(tls)
            .https_only(!plain)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(std::time::Duration::from_secs(20)));
        if plain {
            cfg = cfg.proxy(None);
        }
        let agent = cfg.build().new_agent();
        let mut resp = agent.get(&self.url).call().map_err(|e| e.to_string())?;
        if resp.status().as_u16() != 200 {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        let mut body = Vec::new();
        resp.body_mut()
            .with_config()
            .limit(1024 * 1024)
            .reader()
            .take(1024 * 1024)
            .read_to_end(&mut body)
            .map_err(|e| e.to_string())?;
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_only_for_plain_http() {
        assert!(loopback_http("http://127.0.0.1:9/certs"));
        assert!(loopback_http("http://localhost/certs"));
        assert!(!loopback_http("http://example.com/certs"));
        assert!(!loopback_http("https://127.0.0.1/certs"));
    }
}
