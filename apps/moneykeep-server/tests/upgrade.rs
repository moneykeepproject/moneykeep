//! upgrade-resilience-1 (Batch U1) at the server: writes that survive being cut
//! short (UR-2), the start-up refusals and their report through `services status`
//! (UR-4), the loopback listener's bind retry (UR-5) and the Wi-Fi listener binding
//! again when its address comes back (UR-9, L8). Everything on 127.0.0.1; no
//! internet.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moneykeep_server::secrets::Secrets;
use moneykeep_server::services::{self, ServiceManager, State};
use moneykeep_server::store::{self, KEY_UNOPENABLE, NEWER_DATA};
use moneykeep_server::upgrade::{self, Refusal};
use moneykeep_server::wifi::{self, Target};
use moneykeep_server::{Clock, Config, KeyHome, Options, Running};
use serde_json::Value;

const NOW: i64 = 1_800_000_000;

fn target(ip: &str) -> Target {
    Target {
        ip: ip.parse().unwrap(),
        name: None,
    }
}

/// Every file of a folder and its bytes, sorted.
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap_or_default(),
            )
        })
        .collect();
    v.sort();
    v
}

fn restore(dir: &Path, snap: &[(String, Vec<u8>)]) {
    for e in std::fs::read_dir(dir).unwrap() {
        std::fs::remove_file(e.unwrap().path()).unwrap();
    }
    for (n, b) in snap {
        std::fs::write(dir.join(n), b).unwrap();
    }
}

fn no_temp_files(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .unwrap()
        .all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp"))
}

/// A stand-in for DPAPI: "sealed" bytes carry a prefix.
struct FakeSealer;

impl store::KeySealer for FakeSealer {
    fn seal(&self, plain: &[u8]) -> store::StoreResult<Vec<u8>> {
        Ok([b"SEALED:".as_slice(), plain].concat())
    }
    fn unseal(&self, sealed: &[u8]) -> store::StoreResult<zeroize::Zeroizing<Vec<u8>>> {
        sealed
            .strip_prefix(b"SEALED:".as_slice())
            .map(|p| zeroize::Zeroizing::new(p.to_vec()))
            .ok_or_else(|| "not sealed".to_owned())
    }
}

/// A Wi-Fi capture data folder as setup leaves it: server.toml, format.json, the
/// inbox and its key, the certificates and their sealed keys.
fn wifi_folder(dir: &Path, https: u16, http: u16) {
    moneykeep_server::init(dir, KeyHome::File, NOW).unwrap();
    let c = Config {
        listen: "127.0.0.1:0".into(),
        access_required: false,
        wifi_address: "127.0.0.1".into(),
        wifi_https_port: https,
        wifi_http_port: http,
        ..Config::default()
    };
    std::fs::write(
        dir.join("server.toml"),
        moneykeep_server::config::render(&c),
    )
    .unwrap();
    upgrade::write_format(dir).unwrap();
    let secrets = Secrets::open(dir, KeyHome::File, None).unwrap();
    let now = Clock::default().now();
    wifi::ensure(dir, &secrets, &target("127.0.0.1"), "Test PC", now).unwrap();
}

/// UR-2 (H2): a write cut short of each key, certificate and settings file leaves
/// the old file (and no temporary file).
#[test]
fn ur2_cut_writes_leave_the_old_file() {
    store::without_platform_sealer();
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    wifi_folder(dir, 14443, 14080);
    let snap = snapshot(dir);
    for file in [
        wifi::CA_CERT_FILE,
        wifi::SERVER_CERT_FILE,
        wifi::INFO_FILE,
        "wifi-ca-key.sealed",
        "wifi-tls-key.sealed",
    ] {
        let old = std::fs::read(dir.join(file)).unwrap();
        store::cut_writes(Some(file), 7);
        // A new address makes everything new (Wi-Fi setup's Change address).
        let secrets = Secrets::open(dir, KeyHome::File, None).unwrap();
        let r = wifi::ensure(dir, &secrets, &target("127.0.0.2"), "Test PC", NOW);
        store::cut_writes(None, 0);
        assert!(r.is_err(), "{file}: the write failed");
        assert_eq!(std::fs::read(dir.join(file)).unwrap(), old, "{file}");
        assert!(no_temp_files(dir), "{file}");
        restore(dir, &snap);
    }
    // server.toml (configure) and format.json.
    let old = std::fs::read(dir.join("server.toml")).unwrap();
    store::cut_writes(Some("server.toml"), 3);
    let r = moneykeep_server::setup::configure(
        dir,
        &moneykeep_server::setup::ConfigureStep {
            team_domain: "t.cloudflareaccess.com".into(),
            aud: "aud".into(),
            listen: "127.0.0.1:8787".into(),
            ..Default::default()
        },
    );
    store::cut_writes(None, 0);
    assert!(r.is_err());
    assert_eq!(std::fs::read(dir.join("server.toml")).unwrap(), old);
    std::fs::write(dir.join("format.json"), b"{\"format\": 0}").unwrap();
    store::cut_writes(Some("format.json"), 2);
    assert!(upgrade::write_format(dir).is_err());
    store::cut_writes(None, 0);
    assert_eq!(
        std::fs::read(dir.join("format.json")).unwrap(),
        b"{\"format\": 0}"
    );
    // The sealed inbox key: a cut-short seal leaves the plain key in place.
    let plain = std::fs::read(dir.join(store::KEY_FILE)).unwrap();
    store::cut_writes(Some(store::SEALED_KEY_FILE), 5);
    assert!(store::load_key_with(dir, KeyHome::File, Some(&FakeSealer)).is_err());
    store::cut_writes(None, 0);
    assert_eq!(std::fs::read(dir.join(store::KEY_FILE)).unwrap(), plain);
    assert!(!dir.join(store::SEALED_KEY_FILE).exists());
    assert!(no_temp_files(dir));
}

/// UR-2 (H2): an empty `inbox.key.sealed` with the plain key present opens with the
/// plain key and seals it again; with neither, or with an inbox holding rows and no
/// key at all, the key "can't be opened" and no new one is made.
#[test]
fn ur2_key_opening() {
    store::without_platform_sealer();
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    moneykeep_server::init(dir, KeyHome::File, NOW).unwrap();
    let plain = std::fs::read_to_string(dir.join(store::KEY_FILE)).unwrap();
    let key = store::load_key_with(dir, KeyHome::File, None).unwrap();
    std::fs::write(dir.join(store::SEALED_KEY_FILE), b"").unwrap();
    let opened = store::load_key_with(dir, KeyHome::File, Some(&FakeSealer)).unwrap();
    assert_eq!(*opened, *key);
    assert_eq!(
        std::fs::read(dir.join(store::SEALED_KEY_FILE)).unwrap(),
        [b"SEALED:".as_slice(), plain.trim().as_bytes()].concat(),
        "sealed again"
    );
    assert!(!dir.join(store::KEY_FILE).exists());
    // The re-sealed key opens the inbox: the desktop token is still there.
    assert_eq!(
        *store::load_key_with(dir, KeyHome::File, Some(&FakeSealer)).unwrap(),
        *key
    );
    // Damaged and no plain key: refused.
    std::fs::write(dir.join(store::SEALED_KEY_FILE), b"").unwrap();
    assert_eq!(
        store::load_key_with(dir, KeyHome::File, Some(&FakeSealer)).unwrap_err(),
        KEY_UNOPENABLE
    );
    // No key file at all while the inbox has rows: refused, nothing made.
    std::fs::remove_file(dir.join(store::SEALED_KEY_FILE)).unwrap();
    assert!(store::inbox_has_rows(dir));
    assert_eq!(
        store::load_key_with(dir, KeyHome::File, None).unwrap_err(),
        KEY_UNOPENABLE
    );
    assert!(!dir.join(store::KEY_FILE).exists());
    let e = moneykeep_server::Store::open(dir, KeyHome::File).unwrap_err();
    assert_eq!(e, KEY_UNOPENABLE);
    assert_eq!(moneykeep_server::refusal_for(dir, &e), Some(Refusal::Key));
    // A new, empty folder still gets its first key.
    let fresh = tempfile::tempdir().unwrap();
    assert!(!store::inbox_has_rows(fresh.path()));
    assert!(store::load_key_with(fresh.path(), KeyHome::File, None).is_ok());
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn options(dir: &Path) -> Options {
    let mut o = Options::for_dir(dir, false).unwrap();
    o.key_home = KeyHome::File;
    o.log = Arc::new(|_: &str| {});
    o.threads = 2;
    o.phone_app = None;
    o.pin_cost = moneykeep_server::pin::Cost { m: 64, t: 1, p: 1 };
    o.clock = Arc::new(Clock::default());
    o
}

fn health(r: &Running) -> Value {
    let (s, body) = moneykeep_server::testing::call(
        &format!("http://{}", r.addr()),
        "GET",
        mk_capture_api::HEALTH_PATH,
        &[],
        None,
    )
    .unwrap();
    assert_eq!(s, 200);
    serde_json::from_slice(&body).unwrap()
}

/// UR-2 (H2): a damaged `wifi.json` starts the server without Wi-Fi, keeps the
/// certificate authority's files as they are, and health reports `wifi_error`.
#[test]
fn ur2_damaged_wifi_files_start_without_wifi() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    wifi_folder(dir, free_port(), free_port());
    std::fs::write(dir.join(wifi::INFO_FILE), b"{ damaged").unwrap();
    let ca = std::fs::read(dir.join(wifi::CA_CERT_FILE)).unwrap();
    let ca_key = std::fs::read(dir.join("wifi-ca-key.sealed")).unwrap();
    let running = moneykeep_server::start(options(dir)).unwrap();
    let h = health(&running);
    assert_eq!(h["wifi_error"], wifi::DAMAGED, "{h}");
    assert_eq!(h["wifi_listening"], false);
    assert_eq!(h["api_level"], mk_capture_api::API_LEVEL);
    assert!(running.wifi_addr().is_none());
    drop(running);
    assert_eq!(std::fs::read(dir.join(wifi::CA_CERT_FILE)).unwrap(), ca);
    assert_eq!(
        std::fs::read(dir.join("wifi-ca-key.sealed")).unwrap(),
        ca_key
    );
    assert_eq!(
        std::fs::read(dir.join(wifi::INFO_FILE)).unwrap(),
        b"{ damaged",
        "nothing is made again at start"
    );
}

/// A service manager that reports a stopped service with this data folder and
/// exit code.
struct StoppedSm {
    data: PathBuf,
    exit_code: Option<u32>,
}

impl ServiceManager for StoppedSm {
    fn server_state(&self) -> State {
        State::Stopped
    }
    fn install_server(&self, _: &Path, _: &[String]) -> Result<(), String> {
        Ok(())
    }
    fn uninstall_server(&self) -> Result<(), String> {
        Ok(())
    }
    fn start_server(&self) -> Result<(), String> {
        Ok(())
    }
    fn stop_server(&self, _: Duration) -> Result<(), String> {
        Ok(())
    }
    fn server_path(&self) -> Option<PathBuf> {
        None
    }
    fn cloudflared_path(&self) -> Option<PathBuf> {
        None
    }
    fn cloudflared_service(&self) -> Option<(State, String)> {
        None
    }
    fn install_cloudflared_package(&self, _: &Path) -> Result<(), String> {
        Ok(())
    }
    fn install_cloudflared_service(&self, _: &Path, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn uninstall_cloudflared_service(&self, _: &Path) -> Result<(), String> {
        Ok(())
    }
    fn server_command(&self) -> Option<Vec<String>> {
        Some(services::server_args(&self.data, false)).map(|a| {
            std::iter::once("moneykeep-server".to_owned())
                .chain(a)
                .collect()
        })
    }
    fn server_exit_code(&self) -> Option<u32> {
        self.exit_code
    }
}

fn status_of(dir: &Path, exit_code: Option<u32>) -> services::Status {
    services::status(
        &StoppedSm {
            data: dir.to_path_buf(),
            exit_code,
        },
        None,
    )
}

/// UR-4 (M4, D2): a newer `format.json`, a newer `meta.schema` and an unknown
/// `server.toml` key each refuse the start with their message, and `services
/// status` reports it; a refusal's exit code is read back as its message.
#[test]
fn ur4_refusals_are_reported() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    moneykeep_server::init(dir, KeyHome::File, NOW).unwrap();
    std::fs::write(
        dir.join("server.toml"),
        "listen = \"127.0.0.1:0\"\naccess_required = false\n",
    )
    .unwrap();
    assert!(status_of(dir, None).refusal.is_none());
    // A newer format.
    std::fs::write(dir.join("format.json"), b"{\"format\": 2, \"later\": 1}").unwrap();
    let e = moneykeep_server::start(options(dir)).unwrap_err();
    assert_eq!(e, NEWER_DATA);
    assert_eq!(
        moneykeep_server::refusal_for(dir, &e),
        Some(Refusal::NewerFormat)
    );
    let st = status_of(dir, None);
    assert_eq!(st.refusal.as_deref(), Some(NEWER_DATA));
    assert_eq!(st.refusal_kind, Some(Refusal::NewerFormat));
    std::fs::remove_file(dir.join("format.json")).unwrap();
    // A newer inbox.
    {
        let c = rusqlite::Connection::open(dir.join(store::DB_FILE)).unwrap();
        c.execute("UPDATE meta SET value = '99' WHERE key = 'schema'", [])
            .unwrap();
    }
    assert_eq!(
        moneykeep_server::Store::open(dir, KeyHome::File).unwrap_err(),
        NEWER_DATA
    );
    let e = moneykeep_server::start(options(dir)).unwrap_err();
    assert_eq!(e, NEWER_DATA);
    assert_eq!(
        moneykeep_server::refusal_for(dir, &e),
        Some(Refusal::NewerDatabase)
    );
    assert_eq!(
        status_of(dir, None).refusal_kind,
        Some(Refusal::NewerDatabase)
    );
    {
        let c = rusqlite::Connection::open(dir.join(store::DB_FILE)).unwrap();
        c.execute("UPDATE meta SET value = '2' WHERE key = 'schema'", [])
            .unwrap();
    }
    // An unknown server.toml key: refused, and the refusal is in services status.
    std::fs::write(
        dir.join("server.toml"),
        "listen = \"127.0.0.1:0\"\naccess_required = false\nkey_from_later = 1\n",
    )
    .unwrap();
    let e = Options::for_dir(dir, false).unwrap_err();
    assert_eq!(
        moneykeep_server::refusal_for(dir, &e),
        Some(Refusal::Settings)
    );
    let st = status_of(dir, None);
    assert_eq!(st.refusal_kind, Some(Refusal::Settings));
    let m = st.refusal.unwrap();
    assert!(
        m.starts_with(upgrade::SETTINGS_REFUSED) && m.contains("key_from_later"),
        "{m}"
    );
    // On Windows the data folder can't be read by the user: the service's exit code
    // says which refusal it was.
    let empty = tempfile::tempdir().unwrap();
    let st = status_of(empty.path(), Some(u32::from(Refusal::Key.code())));
    assert_eq!(st.refusal.as_deref(), Some(KEY_UNOPENABLE));
    assert!(status_of(empty.path(), Some(1)).refusal.is_none());
}

/// UR-5 (M5): the loopback port held for 5 seconds, then freed: the server starts.
#[test]
fn ur5_loopback_bind_is_retried() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    moneykeep_server::init(dir, KeyHome::File, NOW).unwrap();
    let port = free_port();
    std::fs::write(
        dir.join("server.toml"),
        format!("listen = \"127.0.0.1:{port}\"\naccess_required = false\n"),
    )
    .unwrap();
    let held = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(5));
        drop(held);
    });
    let started = Instant::now();
    let running = moneykeep_server::start(options(dir)).unwrap();
    assert!(started.elapsed() >= Duration::from_secs(4));
    assert_eq!(running.addr().port(), port);
    assert_eq!(health(&running)["ok"], true);
    t.join().unwrap();
    // With no time to wait, a held port still fails at once.
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut o = options(dir);
    o.config.listen = busy.local_addr().unwrap().to_string();
    o.bind_wait = Duration::ZERO;
    assert!(
        moneykeep_server::start(o)
            .unwrap_err()
            .contains("cannot listen")
    );
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let started = Instant::now();
    while !f() {
        assert!(started.elapsed() < Duration::from_secs(20), "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// UR-9 (L8): the Wi-Fi address goes away (sleep, Wi-Fi dropped): the listener
/// stops and health says so; it comes back: the listener binds again and answers.
#[test]
fn ur9_l8_wifi_listener_binds_again() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path();
    let https = free_port();
    wifi_folder(dir, https, free_port());
    let present = Arc::new(AtomicBool::new(true));
    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut o = options(dir);
    let p2 = present.clone();
    o.address_present = Some(Arc::new(move |_| p2.load(Ordering::SeqCst)));
    o.wifi_recheck = Duration::from_millis(100);
    let sink = logs.clone();
    o.log = Arc::new(move |l: &str| sink.lock().unwrap().push(l.to_owned()));
    let running = moneykeep_server::start(o).unwrap();
    let ca =
        wifi::from_pem(&std::fs::read_to_string(dir.join(wifi::CA_CERT_FILE)).unwrap()).unwrap();
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], https));
    assert_eq!(running.wifi_addr(), Some(addr));
    assert_eq!(health(&running)["wifi_listening"], true);
    present.store(false, Ordering::SeqCst);
    wait_until("the listener stops", || running.wifi_addr().is_none());
    assert_eq!(health(&running)["wifi_listening"], false);
    wait_until("the port is free", || {
        std::net::TcpListener::bind(addr).is_ok()
    });
    present.store(true, Ordering::SeqCst);
    wait_until("the listener is back", || running.wifi_addr().is_some());
    let (s, _) = wifi::https_get(addr, "127.0.0.1", &ca, mk_capture_api::HEALTH_PATH).unwrap();
    assert_eq!(s, 200);
    assert_eq!(health(&running)["wifi_listening"], true);
    assert!(
        logs.lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("is gone from this computer")),
        "{:?}",
        logs.lock().unwrap()
    );
}
