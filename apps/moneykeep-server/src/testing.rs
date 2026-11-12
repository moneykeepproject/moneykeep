//! A capture server on 127.0.0.1 for tests (this crate's, the desktop's and the
//! app's), with a small phone client. Access checks are off (allowed on loopback
//! only), the inbox key lives in a file, and nothing touches the internet.

use std::path::Path;
use std::sync::{Arc, Mutex};

use mk_capture_api::{NewExpense, PairResponse, PairingCode, ReceiptCreated};

use crate::{Clock, Config, KeyHome, Options, Running};

/// A running test server.
#[derive(Debug)]
pub struct LocalServer {
    pub running: Running,
    pub desktop_token: String,
    pub logs: Arc<Mutex<Vec<String>>>,
    pub clock: Arc<Clock>,
}

impl LocalServer {
    /// Start a server with its data in `dir` (Access checks off).
    pub fn start(dir: &Path) -> Result<Self, String> {
        let clock = Arc::new(Clock::default());
        let token = crate::init(dir, KeyHome::File, clock.now())?;
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = logs.clone();
        let running = crate::start(Options {
            data_dir: dir.to_path_buf(),
            config: Config {
                listen: "127.0.0.1:0".into(),
                access_required: false,
                client_url: Some("https://capture.example.test".into()),
                ..Config::default()
            },
            allow_lan: false,
            key_home: KeyHome::File,
            certs: None,
            log: Arc::new(move |l: &str| {
                if let Ok(mut v) = sink.lock() {
                    v.push(l.to_owned());
                }
            }),
            clock: clock.clone(),
            threads: 2,
            credentials: None,
            pin_cost: crate::pin::Cost { m: 64, t: 1, p: 1 },
            bind_wait: crate::BIND_WAIT,
            address_present: None,
            wifi_recheck: crate::WIFI_RECHECK,
            phone_app: None,
        })?;
        Ok(Self {
            running,
            desktop_token: token.to_string(),
            logs,
            clock,
        })
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.running.addr())
    }

    /// Pair a phone through the desktop and phone APIs; returns its device token.
    pub fn pair_phone(&self, name: &str) -> Result<String, String> {
        let (s, body) = call(
            &self.url(),
            "POST",
            "/api/v1/desktop/pairing-codes",
            &[("Authorization", &format!("Bearer {}", self.desktop_token))],
            None,
        )?;
        if s != 201 {
            return Err(format!("pairing code: HTTP {s}"));
        }
        let code: PairingCode = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        let req = serde_json::json!({"code": code.code, "device_name": name}).to_string();
        let (s, body) = call(
            &self.url(),
            "POST",
            "/api/v1/pair",
            &[("Content-Type", "application/json")],
            Some(req.as_bytes()),
        )?;
        if s != 201 {
            return Err(format!("pair: HTTP {s}"));
        }
        let p: PairResponse = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        Ok(p.device_token)
    }

    /// Upload a JPEG as a phone; returns the receipt id.
    pub fn upload(&self, device_token: &str, jpeg: &[u8]) -> Result<String, String> {
        let (s, body) = call(
            &self.url(),
            "POST",
            "/api/v1/receipts",
            &[
                ("Authorization", &format!("Bearer {device_token}")),
                ("Content-Type", "image/jpeg"),
            ],
            Some(jpeg),
        )?;
        if s != 201 {
            return Err(format!(
                "upload: HTTP {s} {}",
                String::from_utf8_lossy(&body)
            ));
        }
        let r: ReceiptCreated = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        Ok(r.receipt_id)
    }

    /// Submit an expense as a phone; returns the HTTP status.
    pub fn submit(&self, device_token: &str, e: &NewExpense) -> Result<u16, String> {
        let body = serde_json::to_vec(e).map_err(|e| e.to_string())?;
        let (s, _) = call(
            &self.url(),
            "POST",
            "/api/v1/expenses",
            &[
                ("Authorization", &format!("Bearer {device_token}")),
                ("Content-Type", "application/json"),
            ],
            Some(&body),
        )?;
        Ok(s)
    }
}

/// One plain-HTTP request to a loopback server (no proxy); returns status and body.
pub fn call(
    base: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<(u16, Vec<u8>), String> {
    use std::io::Read;
    let agent = ureq::Agent::config_builder()
        .proxy(None)
        .http_status_as_error(false)
        .https_only(false)
        .max_redirects(0)
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .build()
        .new_agent();
    let url = format!("{base}{path}");
    let resp = match method {
        "GET" | "DELETE" => {
            let mut r = if method == "GET" {
                agent.get(&url)
            } else {
                agent.delete(&url)
            };
            for (k, v) in headers {
                r = r.header(*k, *v);
            }
            r.call()
        }
        _ => {
            let mut r = match method {
                "PUT" => agent.put(&url),
                "PATCH" => agent.patch(&url),
                _ => agent.post(&url),
            };
            for (k, v) in headers {
                r = r.header(*k, *v);
            }
            match body {
                Some(b) => r.send(b),
                None => r.send_empty(),
            }
        }
    };
    let mut resp = resp.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let mut out = Vec::new();
    resp.body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .reader()
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    Ok((status, out))
}
