//! mobile-capture.md revision 10, tests 67–70: Wi-Fi capture's certificates, keys,
//! listeners and pairing with the Capture PIN. The Wi-Fi listener runs on 127.0.0.1
//! (any address on this computer works the same); certificates are checked with
//! rustls' WebPKI verifier, which enforces name constraints. No internet.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use moneykeep_server::secrets::{MemoryCredentials, Secrets};
use moneykeep_server::testing::call;
use moneykeep_server::wifi::{self, Target};
use moneykeep_server::{Clock, Config, KeyHome, Options, Running};
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use serde_json::{Value, json};
use x509_parser::extensions::{GeneralName, ParsedExtension};

const DAY: i64 = 86_400;

fn target(ip: &str, name: Option<&str>) -> Target {
    Target {
        ip: ip.parse().unwrap(),
        name: name.map(str::to_owned),
    }
}

/// Whether `cert` verifies for `host` against the CA alone, at `now`.
fn verifies(ca: &[u8], cert: &[u8], host: &str, now: i64) -> Result<(), String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(ca.to_vec())).unwrap();
    let v = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::new(rustls::crypto::ring::default_provider()),
    )
    .build()
    .unwrap();
    let name = ServerName::try_from(host.to_owned()).unwrap();
    v.verify_server_cert(
        &CertificateDer::from(cert.to_vec()),
        &[],
        &name,
        &[],
        UnixTime::since_unix_epoch(Duration::from_secs(u64::try_from(now).unwrap())),
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Test 67: the CA and the server certificate.
#[test]
fn at67_ca_and_server_certificate() {
    let now = 1_800_000_000;
    let t = target("192.168.1.20", Some("moneykeep.home.arpa"));
    let (ca, ca_key) = wifi::new_ca("MoneyKeep Capture CA (PC)", &t, now).unwrap();
    let (_, x) = x509_parser::parse_x509_certificate(&ca).unwrap();
    assert!(
        x.subject()
            .to_string()
            .contains("CN=MoneyKeep Capture CA (PC)")
    );
    let days = (x.validity().not_after.timestamp() - x.validity().not_before.timestamp()) / DAY;
    assert_eq!(days, wifi::CA_DAYS, "10 years");
    let mut seen = (false, false, false);
    for ext in x.extensions() {
        match ext.parsed_extension() {
            ParsedExtension::BasicConstraints(b) => {
                assert!(b.ca);
                assert!(ext.critical);
                seen.0 = true;
            }
            ParsedExtension::KeyUsage(k) => {
                assert!(k.key_cert_sign());
                assert!(!k.digital_signature() && !k.crl_sign() && !k.key_encipherment());
                seen.1 = true;
            }
            ParsedExtension::NameConstraints(nc) => {
                let permitted: Vec<String> = nc
                    .permitted_subtrees
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|s| match &s.base {
                        GeneralName::IPAddress(b) => format!("ip:{b:?}"),
                        GeneralName::DNSName(d) => format!("dns:{d}"),
                        other => format!("{other:?}"),
                    })
                    .collect();
                assert_eq!(
                    permitted,
                    vec![
                        "ip:[192, 168, 1, 20, 255, 255, 255, 255]".to_owned(),
                        "dns:moneykeep.home.arpa".to_owned()
                    ]
                );
                assert!(nc.excluded_subtrees.is_some(), "IPv6 excluded");
                seen.2 = true;
            }
            _ => {}
        }
    }
    assert_eq!(seen, (true, true, true));

    // The server certificate: for the address and the name only.
    let (cert, _) =
        wifi::new_server_cert("MoneyKeep Capture CA (PC)", &t, now - DAY, &ca_key, &t, now)
            .unwrap();
    let (_, sx) = x509_parser::parse_x509_certificate(&cert).unwrap();
    let days = (sx.validity().not_after.timestamp() - sx.validity().not_before.timestamp()) / DAY;
    assert_eq!(days, wifi::SERVER_DAYS);
    assert_eq!(
        sx.validity().not_before.timestamp(),
        now - DAY,
        "a day early"
    );
    verifies(&ca, &cert, "192.168.1.20", now).unwrap();
    verifies(&ca, &cert, "moneykeep.home.arpa", now).unwrap();
    assert!(verifies(&ca, &cert, "192.168.1.21", now).is_err());
    assert!(verifies(&ca, &cert, "www.example.com", now).is_err());
    // The CA cannot vouch for another site even if its key signs one: the name
    // constraints refuse it.
    let other = target("192.168.1.99", Some("bank.example.com"));
    let (rogue, _) = wifi::new_server_cert(
        "MoneyKeep Capture CA (PC)",
        &t,
        now - DAY,
        &ca_key,
        &other,
        now,
    )
    .unwrap();
    let e = verifies(&ca, &rogue, "192.168.1.99", now).unwrap_err();
    assert!(e.to_lowercase().contains("name"), "{e}");
    assert!(verifies(&ca, &rogue, "bank.example.com", now).is_err());
    // Without a local name no DNS name passes.
    let t2 = target("10.0.0.5", None);
    let (ca2, k2) = wifi::new_ca("CA", &t2, now).unwrap();
    let (ok, _) = wifi::new_server_cert("CA", &t2, now - DAY, &k2, &t2, now).unwrap();
    verifies(&ca2, &ok, "10.0.0.5", now).unwrap();
    let named = target("10.0.0.5", Some("evil.example.com"));
    let (bad, _) = wifi::new_server_cert("CA", &t2, now - DAY, &k2, &named, now).unwrap();
    assert!(verifies(&ca2, &bad, "evil.example.com", now).is_err());

    // Renewal: within 30 days of the end a new certificate, same CA.
    let dir = tempfile::tempdir().unwrap();
    let secrets = Secrets::open(dir.path(), KeyHome::File, Some(MemoryCredentials::new())).unwrap();
    let first = wifi::ensure(dir.path(), &secrets, &t, "PC", now).unwrap();
    assert!(first.ca_created && first.renewed);
    let again = wifi::ensure(dir.path(), &secrets, &t, "PC", now + DAY).unwrap();
    assert!(!again.ca_created && !again.renewed);
    assert_eq!(again.cert_der, first.cert_der);
    let late = now + (wifi::SERVER_DAYS - 29) * DAY;
    let renewed = wifi::ensure(dir.path(), &secrets, &t, "PC", late).unwrap();
    assert!(renewed.renewed && !renewed.ca_created);
    assert_eq!(renewed.ca_der, first.ca_der);
    assert_eq!(renewed.info.fingerprint, first.info.fingerprint);
    assert_ne!(renewed.cert_der, first.cert_der);
    verifies(&first.ca_der, &renewed.cert_der, "192.168.1.20", late).unwrap();
    // A changed address makes a new CA (its constraints change).
    let moved = wifi::ensure(
        dir.path(),
        &secrets,
        &target("192.168.1.30", None),
        "PC",
        late,
    )
    .unwrap();
    assert!(moved.ca_created);
    assert_ne!(moved.info.fingerprint, first.info.fingerprint);
    verifies(&moved.ca_der, &moved.cert_der, "192.168.1.30", late).unwrap();
    assert!(verifies(&moved.ca_der, &moved.cert_der, "192.168.1.20", late).is_err());
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct W {
    running: Running,
    desktop: String,
    logs: Arc<Mutex<Vec<String>>>,
    creds: Arc<MemoryCredentials>,
    dir: tempfile::TempDir,
}

fn start_wifi(access: bool) -> W {
    start_wifi_with(access, None)
}

fn start_wifi_with(access: bool, client_url: Option<&str>) -> W {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(Clock::default());
    let desktop = moneykeep_server::init(dir.path(), KeyHome::File, clock.now())
        .unwrap()
        .to_string();
    let config = Config {
        listen: "127.0.0.1:0".into(),
        access_required: access,
        access_team_domain: "test-team.cloudflareaccess.com".into(),
        access_aud: "test-aud".into(),
        wifi_address: "127.0.0.1".into(),
        wifi_https_port: free_port(),
        wifi_http_port: free_port(),
        client_url: client_url.map(str::to_owned),
        ..Config::default()
    };
    let logs = Arc::new(Mutex::new(Vec::new()));
    let sink = logs.clone();
    let creds = MemoryCredentials::new();
    // Wi-Fi setup makes the certificates (upgrade-resilience-1 U1.2: the server
    // itself never makes a certificate authority).
    let secrets = Secrets::open(dir.path(), KeyHome::File, Some(creds.clone())).unwrap();
    wifi::ensure(
        dir.path(),
        &secrets,
        &target("127.0.0.1", None),
        "Test PC",
        clock.now(),
    )
    .unwrap();
    let running = moneykeep_server::start(Options {
        data_dir: dir.path().to_path_buf(),
        config,
        allow_lan: false,
        key_home: KeyHome::File,
        certs: None,
        log: Arc::new(move |l: &str| sink.lock().unwrap().push(l.to_owned())),
        clock,
        threads: 2,
        phone_app: None,
        credentials: Some(creds.clone()),
        pin_cost: moneykeep_server::pin::Cost { m: 64, t: 1, p: 1 },
        bind_wait: moneykeep_server::BIND_WAIT,
        address_present: None,
        wifi_recheck: moneykeep_server::WIFI_RECHECK,
    })
    .unwrap();
    W {
        running,
        desktop,
        logs,
        creds,
        dir,
    }
}

impl W {
    fn ca(&self) -> Vec<u8> {
        let t = std::fs::read_to_string(self.dir.path().join(wifi::CA_CERT_FILE)).unwrap();
        wifi::from_pem(&t).unwrap()
    }
    fn tls(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (u16, Value) {
        let addr = self.running.wifi_addr().unwrap();
        let bearer = token.map(|t| format!("Bearer {t}"));
        let mut h: Vec<(&str, &str)> = vec![("Content-Type", "application/json")];
        if let Some(b) = &bearer {
            h.push(("Authorization", b));
        }
        let b = body.map(|v| v.to_string().into_bytes());
        let (s, out) = wifi::https_call(
            addr,
            "127.0.0.1",
            &self.ca(),
            method,
            path,
            &h,
            b.as_deref(),
        )
        .unwrap();
        (s, serde_json::from_slice(&out).unwrap_or(Value::Null))
    }
    fn desktop(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let bearer = format!("Bearer {}", self.desktop);
        let b = body.map(|v| v.to_string().into_bytes());
        let (s, out) = call(
            &format!("http://{}", self.running.addr()),
            method,
            &format!("/api/v1/desktop{path}"),
            &[
                ("Authorization", &bearer),
                ("Content-Type", "application/json"),
            ],
            b.as_deref(),
        )
        .unwrap();
        (s, serde_json::from_slice(&out).unwrap_or(Value::Null))
    }
    fn code(&self) -> String {
        let (s, v) = self.desktop("POST", "/pairing-codes", None);
        assert_eq!(s, 201);
        v["code"].as_str().unwrap().to_owned()
    }
    fn pair(&self, code: &str, pin: Option<&str>) -> (u16, Value) {
        let mut body = json!({"code": code, "device_name": "Pixel"});
        if let Some(p) = pin {
            body["pin"] = json!(p);
        }
        self.tls("POST", "/api/v1/pair", None, Some(body))
    }
}

fn files_in(dir: &std::path::Path) -> Vec<Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| std::fs::read(e.path()).unwrap())
        .collect()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// Test 68: the keys go through the credential-store path and nowhere else.
#[test]
fn at68_keys_only_in_the_credential_store() {
    let w = start_wifi(false);
    let names = w.creds.names();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.iter().any(|n| n.starts_with("capture-wifi-ca-key:")));
    assert!(names.iter().any(|n| n.starts_with("capture-wifi-tls-key:")));
    assert!(!moneykeep_server::secrets::sealed_file(w.dir.path(), wifi::CA_KEY).exists());
    // Some answers the phone and the desktop get.
    let mut answers: Vec<Vec<u8>> = Vec::new();
    for p in ["/wifi", "/status"] {
        answers.push(w.desktop("GET", p, None).1.to_string().into_bytes());
    }
    answers.push(
        w.tls("GET", "/api/v1/pair-info", None, None)
            .1
            .to_string()
            .into_bytes(),
    );
    let helper = w.running.helper_addr().unwrap();
    for p in ["/", "/ca.crt", "/moneykeep-ca.mobileconfig"] {
        answers.push(wifi::http_get(helper, p, "test").unwrap().2);
    }
    let files = files_in(w.dir.path());
    let config = std::fs::read(w.dir.path().join("server.toml")).unwrap();
    let logs = w.logs.lock().unwrap().join("\n").into_bytes();
    for text in w.creds.values() {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&text)
            .unwrap();
        // The private key's bytes (the PKCS#8 document, and its middle part).
        let mid = &raw[raw.len() / 3..raw.len() / 3 + 24];
        for hay in files.iter().chain(answers.iter()).chain([&config, &logs]) {
            assert!(
                !contains(hay, mid),
                "a key appeared outside the credential store"
            );
            assert!(!contains(hay, text.as_bytes()));
        }
    }
}

/// Test 69: the Wi-Fi listener (TLS, no Access token), the helper port, and the
/// loopback listener still behind Access.
#[test]
fn at69_wifi_listener_helper_and_loopback() {
    let w = start_wifi(true);
    // Loopback: Access is still required for the phone API.
    let (s, _) = call(
        &format!("http://{}", w.running.addr()),
        "GET",
        "/api/v1/pair-info",
        &[],
        None,
    )
    .unwrap();
    assert_eq!(s, 401);
    // Wi-Fi: TLS with the server certificate, no Access token asked.
    let (s, v) = w.tls("GET", "/api/v1/pair-info", None, None);
    assert_eq!(s, 200);
    assert_eq!(v, json!({"pin_required": true, "way": "wifi"}));
    let (s, _) = w.tls("GET", "/api/v1/health", None, None);
    assert_eq!(s, 200);
    let addr = w.running.wifi_addr().unwrap();
    let (s, page) = wifi::https_get(addr, "127.0.0.1", &w.ca(), "/").unwrap();
    assert_eq!(s, 200);
    assert!(
        String::from_utf8_lossy(&page).contains("phone app"),
        "the app (or the not-built page)"
    );
    // The phone API needs the device token; the desktop API is not on the Wi-Fi.
    let (s, v) = w.tls("GET", "/api/v1/lists", None, None);
    assert_eq!((s, v["error"]["code"].as_str()), (401, Some("not_paired")));
    let bearer = w.desktop.clone();
    let (s, _) = w.tls("GET", "/api/v1/desktop/inbox", Some(&bearer), None);
    assert_eq!(s, 404);
    // A client that does not trust the CA is refused.
    let (other, _) = wifi::new_ca("Other", &target("127.0.0.1", None), 1_800_000_000).unwrap();
    assert!(wifi::https_get(addr, "127.0.0.1", &other, "/api/v1/health").is_err());
    // The helper: only the three public files; everything else goes to HTTPS.
    let helper = w.running.helper_addr().unwrap();
    let (s, h, body) = wifi::http_get(helper, "/", "Mozilla/5.0 (iPhone)").unwrap();
    assert_eq!(s, 200);
    let page = String::from_utf8(body).unwrap();
    let info = w.running.wifi_info().unwrap();
    assert!(page.contains(&info.fingerprint));
    assert!(page.find("iPhone or iPad</h2>").unwrap() < page.find("Android phone").unwrap());
    let csp = h
        .iter()
        .find(|(k, _)| k == "content-security-policy")
        .unwrap();
    assert!(csp.1.contains("default-src 'none'"));
    let (s, h, body) = wifi::http_get(helper, "/ca.crt", "x").unwrap();
    assert_eq!(s, 200);
    assert_eq!(body, w.ca());
    assert!(
        h.iter()
            .any(|(k, v)| k == "content-type" && v == "application/x-x509-ca-cert")
    );
    let (s, h, body) = wifi::http_get(helper, "/moneykeep-ca.mobileconfig", "x").unwrap();
    assert_eq!(s, 200);
    assert!(String::from_utf8_lossy(&body).contains("com.apple.security.root"));
    assert!(
        h.iter()
            .any(|(k, v)| k == "content-type" && v == "application/x-apple-aspen-config")
    );
    for p in ["/api/v1/lists", "/index.html", "/ca.crt.bak"] {
        let (s, h, _) = wifi::http_get(helper, p, "x").unwrap();
        assert_eq!(s, 301, "{p}");
        let loc = &h.iter().find(|(k, _)| k == "location").unwrap().1;
        assert_eq!(loc, &format!("https://127.0.0.1:{}{p}", addr.port()));
    }
    // The desktop sees the listener.
    let (s, v) = w.desktop("GET", "/wifi", None);
    assert_eq!(s, 200);
    assert_eq!(v["enabled"], true);
    assert_eq!(v["listening"], true);
    assert_eq!(v["address_present"], true);
    assert_eq!(v["fingerprint"], info.fingerprint);
    let (_, st) = w.desktop("GET", "/status", None);
    assert_eq!(st["wifi"]["address"], "127.0.0.1");
    // A renewal is picked up without a restart.
    assert!(!w.running.renew_if_due().unwrap());
}

/// Test 70: pairing with the Capture PIN.
#[test]
fn at70_pairing_with_the_pin() {
    let w = start_wifi(false);
    // No PIN yet: Wi-Fi pairing is refused.
    let code = w.code();
    let (s, v) = w.pair(&code, Some("123456"));
    assert_eq!((s, v["error"]["code"].as_str()), (409, Some("pin_not_set")));
    // 6–12 digits.
    for bad in ["12345", "1234567890123", "12a456"] {
        let (s, _) = w.desktop("PUT", "/pin", Some(json!({"pin": bad})));
        assert_eq!(s, 422, "{bad}");
    }
    let (s, _) = w.desktop("PUT", "/pin", Some(json!({"pin": "246813"})));
    assert_eq!(s, 200);
    // Stored only as an Argon2id hash.
    let db = std::fs::read(w.dir.path().join("inbox.sqlite")).unwrap();
    assert!(contains(&db, b"argon2id"));
    assert!(!contains(&db, b"246813"));
    let logs = w.logs.lock().unwrap().join("\n");
    assert!(!logs.contains("246813"));
    // Wrong PIN: refused with pin_wrong; right PIN then pairs (the code is kept).
    let code = w.code();
    let (s, v) = w.pair(&code, Some("111111"));
    assert_eq!((s, v["error"]["code"].as_str()), (401, Some("pin_wrong")));
    assert_eq!(
        v["error"]["message"],
        "That PIN is not right. Check it in MoneyKeep on your computer."
    );
    let (s, v) = w.pair(&code, Some("246813"));
    assert_eq!(s, 201, "{v}");
    let token = v["device_token"].as_str().unwrap().to_owned();
    let (s, _) = w.tls("GET", "/api/v1/lists", Some(&token), None);
    assert_eq!(s, 200);
    // Five wrong PINs end the code.
    let code = w.code();
    for _ in 0..5 {
        let (s, v) = w.pair(&code, Some("000000"));
        assert_eq!(v["error"]["code"], "pin_wrong", "{s}");
    }
    let (s, v) = w.pair(&code, Some("246813"));
    assert_eq!((s, v["error"]["code"].as_str()), (401, Some("pair_failed")));
    // Once a PIN is set, the Cloudflare (loopback) address asks for it too.
    let code = w.code();
    let loopback = |body: Value| {
        let (s, out) = call(
            &format!("http://{}", w.running.addr()),
            "POST",
            "/api/v1/pair",
            &[("Content-Type", "application/json")],
            Some(body.to_string().as_bytes()),
        )
        .unwrap();
        (s, serde_json::from_slice::<Value>(&out).unwrap())
    };
    let (s, v) = loopback(json!({"code": code, "device_name": "iPad"}));
    assert_eq!((s, v["error"]["code"].as_str()), (401, Some("pin_wrong")));
    let (s, _) = loopback(json!({"code": code, "device_name": "iPad", "pin": "246813"}));
    assert_eq!(s, 201);
    // The devices list shows each phone's way in.
    let (_, d) = w.desktop("GET", "/devices", None);
    assert_eq!(d["devices"][0]["via"], "wifi");
    assert_eq!(d["devices"][0]["name"], "Pixel");
    assert_eq!(d["devices"][1]["via"], "cloudflare");
}

/// mileage.md M11 (server part, the Wi-Fi way in): the same trip endpoints over
/// the Wi-Fi listener with the device token, and the desktop takes it from the
/// loopback inbox.
#[test]
fn m11_trips_over_the_wifi() {
    let w = start_wifi(false);
    let (s, _) = w.desktop("PUT", "/pin", Some(json!({"pin": "246813"})));
    assert_eq!(s, 200);
    let code = w.code();
    let (s, v) = w.pair(&code, Some("246813"));
    assert_eq!(s, 201, "{v}");
    let token = v["device_token"].as_str().unwrap().to_owned();
    let today =
        mk_capture_api::format_date(mk_capture_api::today_from_unix(Clock::default().now(), 0));
    let trip = json!({"client_id": "8d2f1b6c-3e4a-4b5c-9d6e-7f8091a2b3c4", "date": today,
        "vehicle_id": "veh-1", "purpose_kind": "charity", "purpose": "Food pantry delivery",
        "from_place": "Home", "to_place": "St. Mark's", "miles": "8.5"});
    let (s, v) = w.tls("POST", "/api/v1/trips", Some(&token), Some(trip.clone()));
    assert_eq!(s, 201, "{v}");
    let (s, again) = w.tls("POST", "/api/v1/trips", Some(&token), Some(trip));
    assert_eq!((s, &again["id"]), (200, &v["id"]));
    let (s, list) = w.tls("GET", "/api/v1/trips", Some(&token), None);
    assert_eq!(s, 200);
    assert_eq!(list["waiting"][0]["to_place"], "St. Mark's");
    // The desktop's acknowledgement is not on the Wi-Fi.
    let id = v["id"].as_str().unwrap().to_owned();
    let (s, _) = w.tls(
        "POST",
        &format!("/api/v1/desktop/trips/{id}/ack"),
        Some(&w.desktop.clone()),
        Some(json!({})),
    );
    assert_eq!(s, 404);
    let (_, inbox) = w.desktop("GET", "/inbox", None);
    assert_eq!(inbox["trips"][0]["fields"]["miles"], "8.5");
    let (s, _) = w.desktop("POST", &format!("/trips/{id}/ack"), Some(json!({})));
    assert_eq!(s, 200);
    let (_, list) = w.tls("GET", "/api/v1/trips", Some(&token), None);
    assert_eq!(list["done"][0]["status"], "taken");
}

/// Revision 18: on the Wi-Fi address, pair-info names the computer's Cloudflare
/// address (for Use Cloudflare on the phone's loading screen) when there is one.
#[test]
fn r18_pair_info_names_the_cloudflare_address() {
    let w = start_wifi_with(true, Some("https://capture.example.com"));
    let (s, v) = w.tls("GET", "/api/v1/pair-info", None, None);
    assert_eq!(s, 200);
    assert_eq!(
        v,
        json!({"pin_required": true, "way": "wifi", "cloudflare_url": "https://capture.example.com"})
    );
}

/// A TLS connection to the Wi-Fi listener that has sent `bytes` and stays open.
fn tls_open(
    w: &W,
    bytes: &[u8],
) -> rustls::StreamOwned<rustls::ClientConnection, std::net::TcpStream> {
    use std::io::Write;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(w.ca())).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let conn = rustls::ClientConnection::new(
        Arc::new(config),
        ServerName::try_from("127.0.0.1".to_owned()).unwrap(),
    )
    .unwrap();
    let tcp = std::net::TcpStream::connect(w.running.wifi_addr().unwrap()).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut s = rustls::StreamOwned::new(conn, tcp);
    s.write_all(bytes).unwrap();
    s.flush().unwrap();
    s
}

/// The server closed the connection (or answered and closed): how long it took.
fn closed_after(
    s: &mut rustls::StreamOwned<rustls::ClientConnection, std::net::TcpStream>,
) -> Duration {
    use std::io::Read;
    let started = std::time::Instant::now();
    let mut buf = [0u8; 4096];
    loop {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => return started.elapsed(),
            Ok(_) => {}
        }
    }
}

/// A pair request whose (large) body never comes: it reaches a worker.
const IDLE_PAIR: &[u8] = b"POST /api/v1/pair HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 50000\r\n\r\n";
/// The same with a small body (the HTTP server waits for it itself).
const IDLE_SMALL_PAIR: &[u8] = b"POST /api/v1/pair HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 200\r\n\r\n";

/// security-review-1 SR-A4: two pair requests whose body never comes do not block a
/// paired phone's upload, which succeeds within 2 seconds; a third unauthenticated
/// request from the same address is closed at once; a body that does not arrive in
/// 10 seconds is closed (large or small).
#[test]
fn sr_a4_pairing_cannot_stall_the_server() {
    let w = start_wifi(false);
    let (s, _) = w.desktop("PUT", "/pin", Some(json!({"pin": "246813"})));
    assert_eq!(s, 200);
    let code = w.code();
    let (s, v) = w.pair(&code, Some("246813"));
    assert_eq!(s, 201, "{v}");
    let token = v["device_token"].as_str().unwrap().to_owned();
    let opened = std::time::Instant::now();
    let mut a = tls_open(&w, IDLE_PAIR);
    let mut b = tls_open(&w, IDLE_PAIR);
    let mut d = tls_open(&w, IDLE_SMALL_PAIR);
    std::thread::sleep(Duration::from_millis(500));
    // The Wi-Fi workers are free: a paired upload goes through.
    let today =
        mk_capture_api::format_date(mk_capture_api::today_from_unix(Clock::default().now(), 0));
    let trip = json!({"client_id": "8d2f1b6c-3e4a-4b5c-9d6e-7f8091a2b3c4", "date": today,
        "vehicle_id": "veh-1", "purpose_kind": "charity", "purpose": "Food pantry delivery",
        "from_place": "Home", "to_place": "St. Mark's", "miles": "8.5"});
    let t = std::time::Instant::now();
    let (s, v) = w.tls("POST", "/api/v1/trips", Some(&token), Some(trip));
    assert_eq!(s, 201, "{v}");
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    // A third unauthenticated request from this address: closed at once.
    let mut c = tls_open(&w, IDLE_PAIR);
    let took = closed_after(&mut c);
    assert!(
        took < Duration::from_secs(2),
        "{took:?} {:?}",
        w.logs.lock().unwrap()
    );
    // The idle bodies are closed after 10 seconds.
    for s in [&mut a, &mut b, &mut d] {
        closed_after(s);
        let waited = opened.elapsed();
        assert!(
            waited >= Duration::from_secs(9) && waited < Duration::from_secs(13),
            "{waited:?}"
        );
    }
    // Pairing works again afterwards.
    let code = w.code();
    let (s, v) = w.pair(&code, Some("246813"));
    assert_eq!(s, 201, "{v}");
}

/// SR-A4: at most 4 unauthenticated requests at a time, 2 from one address; a fifth
/// is refused, and a place is given back when one ends.
#[test]
fn sr_a4_gate_limits() {
    let gate = wifi::Gate::new();
    let ip = |n: u8| std::net::IpAddr::from([192, 168, 1, n]);
    let one = gate.admit(ip(1)).unwrap();
    let _two = gate.admit(ip(1)).unwrap();
    assert!(gate.admit(ip(1)).is_none(), "two from one address");
    let _three = gate.admit(ip(2)).unwrap();
    let _four = gate.admit(ip(3)).unwrap();
    assert_eq!(gate.open(), wifi::MAX_UNAUTH);
    assert!(gate.admit(ip(4)).is_none(), "the fifth is refused");
    drop(one);
    assert!(gate.admit(ip(4)).is_some());
    assert_eq!(wifi::UNAUTH_BODY_TIMEOUT, Duration::from_secs(10));
    assert_eq!(wifi::MAX_UNAUTH_PER_ADDRESS, 2);
}
