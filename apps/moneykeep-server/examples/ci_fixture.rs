//! upgrade-resilience-1 Batch U3: capture-server data and checks for the Windows CI
//! job (`.github/workflows/windows.yml`, run by `scripts/ci/windows-upgrade.ps1`).
//!
//! - `pre-a1 --data DIR --out FILE`: a data folder as servers from before
//!   security-review-1 A1 left it: a plain `inbox.key`, no `format.json`, one paired
//!   phone with one waiting expense, and Wi-Fi capture on 127.0.0.1 with its
//!   certificate authority. Access checks are off (allowed on loopback only). FILE
//!   gets the desktop token, the expense and the authority's fingerprint (JSON).
//! - `phone --url URL --token T --out FILE`: pair a phone with a running server and
//!   submit one expense; FILE as above, the fingerprint read from the server.
//! - `check --url URL --fixture FILE [--wait SECONDS]`: the server passes its health
//!   check, FILE's expense is in its inbox (so the inbox key still opens it) and its
//!   Wi-Fi certificate authority has FILE's fingerprint.
//!
//! `cargo run -p moneykeep-server --example ci_fixture -- <command> …`
#![allow(clippy::print_stdout)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use mk_capture_api::{Health, Inbox, PairResponse, PairingCode, WifiState};
use moneykeep_server::testing::{LocalServer, call};
use moneykeep_server::{Clock, KeyHome, config, setup, store, upgrade};
use serde::{Deserialize, Serialize};

/// What a later check looks for.
#[derive(Debug, Serialize, Deserialize)]
struct Fixture {
    desktop_token: String,
    client_id: String,
    merchant: String,
    fingerprint: String,
}

const MERCHANT: &str = "Upgrade Check Stationers";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let r = match args.first().map(String::as_str) {
        Some("pre-a1") => pre_a1(rest),
        Some("phone") => phone(rest),
        Some("check") => check(rest),
        _ => Err("usage: ci_fixture pre-a1 --data DIR --out FILE | phone --url URL --token T --out FILE | check --url URL --fixture FILE [--wait SECONDS]".into()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ci_fixture: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The value after `--name`.
fn opt(args: &[String], name: &str) -> Result<String, String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
        .ok_or_else(|| format!("{name} is missing"))
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// Pair a phone through the desktop and phone APIs; returns its device token.
fn pair(url: &str, desktop_token: &str) -> Result<String, String> {
    let auth = bearer(desktop_token);
    let (s, body) = call(
        url,
        "POST",
        "/api/v1/desktop/pairing-codes",
        &[("Authorization", &auth)],
        None,
    )?;
    if s != 201 {
        return Err(format!("pairing code: HTTP {s}"));
    }
    let code: PairingCode = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let req = serde_json::json!({"code": code.code, "device_name": "CI phone"}).to_string();
    let (s, body) = call(
        url,
        "POST",
        "/api/v1/pair",
        &[("Content-Type", "application/json")],
        Some(req.as_bytes()),
    )?;
    if s != 201 {
        return Err(format!("pair: HTTP {s} {}", String::from_utf8_lossy(&body)));
    }
    let p: PairResponse = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    Ok(p.device_token)
}

/// Submit one expense as the phone.
fn submit(url: &str, device_token: &str, client_id: &str) -> Result<(), String> {
    let today = mk_capture_api::today_from_unix(Clock::default().now(), 0);
    let body = serde_json::json!({
        "client_id": client_id,
        "date": mk_capture_api::format_date(today),
        "merchant": MERCHANT,
        "amount": "12.34",
        "receipt_ids": [],
    })
    .to_string();
    let auth = bearer(device_token);
    let (s, reply) = call(
        url,
        "POST",
        "/api/v1/expenses",
        &[
            ("Authorization", &auth),
            ("Content-Type", "application/json"),
        ],
        Some(body.as_bytes()),
    )?;
    if s != 200 && s != 201 {
        return Err(format!(
            "submit: HTTP {s} {}",
            String::from_utf8_lossy(&reply)
        ));
    }
    Ok(())
}

fn get<T: serde::de::DeserializeOwned>(url: &str, path: &str, token: &str) -> Result<T, String> {
    let auth = bearer(token);
    let (s, body) = call(url, "GET", path, &[("Authorization", &auth)], None)?;
    if s != 200 {
        return Err(format!(
            "GET {path}: HTTP {s} {}",
            String::from_utf8_lossy(&body)
        ));
    }
    serde_json::from_slice(&body).map_err(|e| format!("GET {path}: {e}"))
}

fn write_fixture(out: &Path, f: &Fixture) -> Result<(), String> {
    let text = serde_json::to_string_pretty(f).map_err(|e| e.to_string())?;
    std::fs::write(out, text).map_err(|e| format!("cannot write {}: {e}", out.display()))
}

fn pre_a1(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(opt(args, "--data")?);
    let out = PathBuf::from(opt(args, "--out")?);
    let client_id = "6f1c2b9e-5d3a-4c1e-9b7a-0000000000a1".to_owned();
    // A phone and its expense, through the API of a server on this folder.
    let server = LocalServer::start(&dir)?;
    let url = server.url();
    let desktop_token = server.desktop_token.clone();
    let device = pair(&url, &desktop_token)?;
    submit(&url, &device, &client_id)?;
    server.running.stop();
    // server.toml: Access checks off (the folder's listen address stays loopback).
    let cfg = dir.join(config::CONFIG_FILE);
    let text = std::fs::read_to_string(&cfg).map_err(|e| e.to_string())?;
    if !text.contains("access_required = true") {
        return Err(format!("{} has no access_required line", cfg.display()));
    }
    let text = text.replace("access_required = true", "access_required = false");
    store::write_private(&cfg, text.as_bytes())?;
    // Wi-Fi capture with its certificate authority.
    let wifi = setup::wifi_configure(
        &dir,
        &setup::WifiStep {
            address: "127.0.0.1".into(),
            name: String::new(),
            https_port: mk_capture_api::DEFAULT_WIFI_HTTPS_PORT,
            http_port: mk_capture_api::DEFAULT_WIFI_HTTP_PORT,
            computer: Some("CI".into()),
        },
        KeyHome::File,
    )?;
    let fingerprint = wifi
        .get("fingerprint")
        .and_then(|v| v.as_str())
        .filter(|f| !f.is_empty())
        .ok_or("wifi configure gave no fingerprint")?
        .to_owned();
    // Before upgrade-resilience-1 U1.4 there was no format number.
    let _ = std::fs::remove_file(dir.join(upgrade::FORMAT_FILE));
    // Before A1 the inbox key was a plain file; this system's sealer (Windows, macOS) has
    // sealed it on opening, so it is opened and written plain again.
    let plain = dir.join(store::KEY_FILE);
    let sealed = dir.join(store::SEALED_KEY_FILE);
    if let Some(sealer) = store::platform_sealer(&dir)
        && sealed.exists()
    {
        let blob = std::fs::read(&sealed).map_err(|e| e.to_string())?;
        let key = sealer.unseal(&blob)?;
        store::write_private(&plain, &key)?;
        std::fs::remove_file(&sealed).map_err(|e| e.to_string())?;
    }
    if !plain.is_file() || sealed.exists() {
        return Err(format!("{} is not a plain key file", plain.display()));
    }
    write_fixture(
        &out,
        &Fixture {
            desktop_token,
            client_id,
            merchant: MERCHANT.into(),
            fingerprint: fingerprint.clone(),
        },
    )?;
    println!(
        "{}",
        serde_json::json!({"data": dir, "fingerprint": fingerprint})
    );
    Ok(())
}

fn phone(args: &[String]) -> Result<(), String> {
    let url = opt(args, "--url")?;
    let desktop_token = opt(args, "--token")?;
    let out = PathBuf::from(opt(args, "--out")?);
    let client_id = "6f1c2b9e-5d3a-4c1e-9b7a-0000000000b2".to_owned();
    let device = pair(&url, &desktop_token)?;
    submit(&url, &device, &client_id)?;
    let wifi: WifiState = get(&url, "/api/v1/desktop/wifi", &desktop_token)?;
    if wifi.fingerprint.is_empty() {
        return Err("the server has no Wi-Fi certificate authority".into());
    }
    write_fixture(
        &out,
        &Fixture {
            desktop_token,
            client_id,
            merchant: MERCHANT.into(),
            fingerprint: wifi.fingerprint.clone(),
        },
    )?;
    println!("{}", serde_json::json!({"fingerprint": wifi.fingerprint}));
    Ok(())
}

fn check(args: &[String]) -> Result<(), String> {
    let url = opt(args, "--url")?;
    let file = PathBuf::from(opt(args, "--fixture")?);
    let wait: u64 = opt(args, "--wait")
        .ok()
        .map(|w| w.parse().map_err(|_| format!("--wait {w} is not a number")))
        .transpose()?
        .unwrap_or(60);
    let text = std::fs::read_to_string(&file)
        .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let f: Fixture = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    // The health check (the service may still be starting).
    let started = Instant::now();
    let health = loop {
        let last = match call(&url, "GET", "/api/v1/health", &[], None) {
            Ok((200, body)) => match serde_json::from_slice::<Health>(&body) {
                Ok(h) if h.ok => break h,
                Ok(_) => "health: ok is false".to_owned(),
                Err(e) => format!("health: {e}"),
            },
            Ok((s, _)) => format!("health: HTTP {s}"),
            Err(e) => format!("health: {e}"),
        };
        if started.elapsed() > Duration::from_secs(wait) {
            return Err(format!("no passing health check in {wait} s ({last})"));
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    let inbox: Inbox = get(&url, "/api/v1/desktop/inbox", &f.desktop_token)?;
    let found = inbox
        .items
        .iter()
        .any(|i| i.client_id == f.client_id && i.fields.merchant.as_deref() == Some(&f.merchant));
    if !found {
        return Err(format!(
            "the expense {} is not in the inbox ({} items)",
            f.client_id,
            inbox.items.len()
        ));
    }
    let wifi: WifiState = get(&url, "/api/v1/desktop/wifi", &f.desktop_token)?;
    if wifi.fingerprint != f.fingerprint {
        return Err(format!(
            "the Wi-Fi certificate authority changed: {:?}, was {:?}",
            wifi.fingerprint, f.fingerprint
        ));
    }
    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "version": health.version,
            "api_level": health.api_level,
            "wifi_listening": health.wifi_listening,
            "inbox_items": inbox.items.len(),
            "fingerprint": wifi.fingerprint,
        })
    );
    Ok(())
}
