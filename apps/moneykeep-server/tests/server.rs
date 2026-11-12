//! mobile-capture.md acceptance tests 1–10: an in-process server on 127.0.0.1 with
//! test RSA keys (tests/fixtures, generated with `openssl genpkey` for these tests
//! only) and a fake Access certificates endpoint on 127.0.0.1. No internet.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use mk_capture_api::{ExpenseFields, NewExpense};
use moneykeep_server::testing::call;
use moneykeep_server::{Clock, Config, HttpCerts, KeyHome, Options, PhoneApp, Running};
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair, RsaPublicKeyComponents};
use serde_json::{Value, json};

const TEAM: &str = "test-team.cloudflareaccess.com";
const AUD: &str = "test-aud-tag";

fn key(name: &str) -> RsaKeyPair {
    let der = std::fs::read(format!(
        "{}/tests/fixtures/test-rsa-{name}.p8",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    RsaKeyPair::from_pkcs8(&der)
        .map_err(|e| format!("{e:?}"))
        .unwrap()
}

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn jwk(kid: &str, k: &RsaKeyPair) -> Value {
    let c: RsaPublicKeyComponents<Vec<u8>> = k.public().into();
    json!({"kid": kid, "kty": "RSA", "alg": "RS256", "use": "sig", "e": b64(&c.e), "n": b64(&c.n)})
}

fn sign(k: &RsaKeyPair, kid: &str, claims: &Value) -> String {
    let h = b64(json!({"alg": "RS256", "kid": kid, "typ": "JWT"})
        .to_string()
        .as_bytes());
    let p = b64(claims.to_string().as_bytes());
    let input = format!("{h}.{p}");
    let mut sig = vec![0u8; k.public().modulus_len()];
    k.sign(
        &RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        input.as_bytes(),
        &mut sig,
    )
    .unwrap();
    format!("{input}.{}", b64(&sig))
}

/// A fake `https://<team>/cdn-cgi/access/certs` on 127.0.0.1, counting fetches.
struct FakeCerts {
    body: Arc<Mutex<String>>,
    hits: Arc<AtomicUsize>,
    url: String,
    _server: Arc<tiny_http::Server>,
}

impl FakeCerts {
    fn start(keys: Value) -> Self {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let url = format!(
            "http://{}/cdn-cgi/access/certs",
            server.server_addr().to_ip().unwrap()
        );
        let body = Arc::new(Mutex::new(json!({"keys": keys}).to_string()));
        let hits = Arc::new(AtomicUsize::new(0));
        let (s, b, h) = (server.clone(), body.clone(), hits.clone());
        std::thread::spawn(move || {
            for req in s.incoming_requests() {
                h.fetch_add(1, Ordering::SeqCst);
                let text = b.lock().unwrap().clone();
                let _ = req.respond(tiny_http::Response::from_string(text));
            }
        });
        Self {
            body,
            hits,
            url,
            _server: server,
        }
    }
    fn set(&self, keys: Value) {
        *self.body.lock().unwrap() = json!({"keys": keys}).to_string();
    }
    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

struct T {
    running: Running,
    desktop: String,
    clock: Arc<Clock>,
    logs: Arc<Mutex<Vec<String>>>,
    certs: FakeCerts,
    a: RsaKeyPair,
    dir: tempfile::TempDir,
}

fn start_with(cfg: impl FnOnce(&mut Config)) -> T {
    start_full(cfg, None)
}

fn start_full(cfg: impl FnOnce(&mut Config), phone_app: Option<PhoneApp>) -> T {
    let dir = tempfile::tempdir().unwrap();
    let a = key("a");
    let certs = FakeCerts::start(json!([jwk("kid-a", &a)]));
    let clock = Arc::new(Clock::default());
    let desktop = moneykeep_server::init(dir.path(), KeyHome::File, clock.now())
        .unwrap()
        .to_string();
    let mut config = Config {
        listen: "127.0.0.1:0".into(),
        access_required: true,
        access_team_domain: TEAM.into(),
        access_aud: AUD.into(),
        ..Config::default()
    };
    cfg(&mut config);
    let logs = Arc::new(Mutex::new(Vec::new()));
    let sink = logs.clone();
    let running = moneykeep_server::start(Options {
        data_dir: dir.path().to_path_buf(),
        config,
        allow_lan: false,
        key_home: KeyHome::File,
        certs: Some(Arc::new(HttpCerts {
            url: certs.url.clone(),
        })),
        log: Arc::new(move |l: &str| sink.lock().unwrap().push(l.to_owned())),
        clock: clock.clone(),
        threads: 2,
        phone_app,
        credentials: None,
        pin_cost: moneykeep_server::pin::Cost { m: 64, t: 1, p: 1 },
        bind_wait: moneykeep_server::BIND_WAIT,
        address_present: None,
        wifi_recheck: moneykeep_server::WIFI_RECHECK,
    })
    .unwrap();
    T {
        running,
        desktop,
        clock,
        logs,
        certs,
        a,
        dir,
    }
}

fn start() -> T {
    start_with(|_| {})
}

impl T {
    fn base(&self) -> String {
        format!("http://{}", self.running.addr())
    }
    fn claims(&self) -> Value {
        let now = self.clock.now();
        json!({"iss": format!("https://{TEAM}"), "aud": [AUD], "exp": now + 300,
               "nbf": now - 10, "iat": now - 10, "email": "owner@example.test"})
    }
    fn jwt(&self) -> String {
        sign(&self.a, "kid-a", &self.claims())
    }
    fn phone(
        &self,
        method: &str,
        path: &str,
        jwt: Option<&str>,
        token: Option<&str>,
        ct: Option<&str>,
        body: Option<&[u8]>,
    ) -> (u16, Vec<u8>) {
        let bearer = token.map(|t| format!("Bearer {t}"));
        let mut h: Vec<(&str, &str)> = Vec::new();
        if let Some(j) = jwt {
            h.push(("Cf-Access-Jwt-Assertion", j));
        }
        if let Some(b) = &bearer {
            h.push(("Authorization", b));
        }
        if let Some(c) = ct {
            h.push(("Content-Type", c));
        }
        call(&self.base(), method, &format!("/api/v1{path}"), &h, body).unwrap()
    }
    fn desktop(&self, method: &str, path: &str, body: Option<&[u8]>) -> (u16, Vec<u8>) {
        let bearer = format!("Bearer {}", self.desktop);
        call(
            &self.base(),
            method,
            &format!("/api/v1/desktop{path}"),
            &[
                ("Authorization", &bearer),
                ("Content-Type", "application/json"),
            ],
            body,
        )
        .unwrap()
    }
    fn code(&self) -> String {
        let (s, b) = self.desktop("POST", "/pairing-codes", None);
        assert_eq!(s, 201);
        let v: Value = serde_json::from_slice(&b).unwrap();
        v["code"].as_str().unwrap().to_owned()
    }
    fn pair_with(&self, code: &str) -> (u16, Value) {
        let body = json!({"code": code, "device_name": "Test phone"}).to_string();
        let (s, b) = self.phone(
            "POST",
            "/pair",
            Some(&self.jwt()),
            None,
            Some("application/json"),
            Some(body.as_bytes()),
        );
        (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }
    fn pair(&self) -> String {
        let code = self.code();
        let (s, v) = self.pair_with(&code);
        assert_eq!(s, 201, "{v}");
        v["device_token"].as_str().unwrap().to_owned()
    }
    fn get(&self, token: &str, path: &str) -> u16 {
        self.phone("GET", path, Some(&self.jwt()), Some(token), None, None)
            .0
    }
    fn post_json(&self, token: &str, path: &str, v: &Value) -> (u16, Value) {
        let body = v.to_string();
        let (s, b) = self.phone(
            "POST",
            path,
            Some(&self.jwt()),
            Some(token),
            Some("application/json"),
            Some(body.as_bytes()),
        );
        (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }
    fn upload(&self, token: &str, ct: &str, bytes: &[u8]) -> (u16, Value) {
        let (s, b) = self.phone(
            "POST",
            "/receipts",
            Some(&self.jwt()),
            Some(token),
            Some(ct),
            Some(bytes),
        );
        (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }
    fn db(&self) -> Vec<u8> {
        std::fs::read(self.dir.path().join("inbox.sqlite")).unwrap()
    }
    fn log_text(&self) -> String {
        self.logs.lock().unwrap().join("\n")
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// A noisy test photo with an EXIF block (orientation 6 and a GPS IFD).
fn photo(w: u32, h: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    let mut x: u32 = 12345;
    for _ in 0..w * h * 3 {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
        data.push((x >> 16) as u8);
    }
    let img = mk_import::image::Rgb {
        width: w,
        height: h,
        data,
    };
    let jpeg = mk_import::jpeg::encode_jpeg(&img, 85).unwrap();
    mk_import::receipt::with_test_exif(&jpeg, 6)
}

fn expense(client: &str, amount: &str, receipts: &[String]) -> Value {
    json!({"client_id": client, "date": mk_capture_api::format_date(
        mk_capture_api::today_from_unix(Clock::default().now(), 0)),
        "merchant": "Zebra Stationers Unique", "amount": amount, "receipt_ids": receipts})
}

/// AT1: Access token checks; a valid token plus a valid device token is accepted.
#[test]
fn at01_access_tokens_are_checked() {
    let t = start();
    let token = t.pair();
    let now = t.clock.now();
    assert_eq!(t.get(&token, "/lists"), 200);
    let no_jwt = t.phone("GET", "/lists", None, Some(&token), None, None).0;
    assert_eq!(no_jwt, 401);
    // Change one character in the middle of the signature.
    let good = t.jwt();
    let at = good.rfind('.').unwrap() + 20;
    let mut bad_sig = good.clone();
    let c = if &good[at..=at] == "A" { "B" } else { "A" };
    bad_sig.replace_range(at..=at, c);
    let b = key("b");
    let other_key = sign(&b, "kid-a", &t.claims());
    let mut wrong_aud = t.claims();
    wrong_aud["aud"] = json!(["someone-else"]);
    let mut wrong_iss = t.claims();
    wrong_iss["iss"] = json!("https://other.cloudflareaccess.com");
    let mut expired = t.claims();
    expired["exp"] = json!(now - 120);
    let mut future = t.claims();
    future["nbf"] = json!(now + 600);
    for (what, jwt) in [
        ("bad signature", bad_sig),
        ("other key", other_key),
        ("wrong aud", sign(&t.a, "kid-a", &wrong_aud)),
        ("wrong iss", sign(&t.a, "kid-a", &wrong_iss)),
        ("expired", sign(&t.a, "kid-a", &expired)),
        ("not yet valid", sign(&t.a, "kid-a", &future)),
        ("garbage", "a.b.c".into()),
    ] {
        let (s, body) = t.phone("GET", "/lists", Some(&jwt), Some(&token), None, None);
        assert_eq!(s, 401, "{what}");
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "access_refused", "{what}");
        assert_eq!(
            v["error"]["message"],
            mk_capture_api::msg::ACCESS_REFUSED,
            "{what}"
        );
    }
    // Leeway: expired 30 s ago is still accepted.
    let mut leeway = t.claims();
    leeway["exp"] = json!(now - 30);
    let ok = t
        .phone(
            "GET",
            "/lists",
            Some(&sign(&t.a, "kid-a", &leeway)),
            Some(&token),
            None,
            None,
        )
        .0;
    assert_eq!(ok, 200);
    // Clock skew is logged as such.
    assert!(t.log_text().contains("check this computer's clock"));
    // A valid JWT without a device token is refused too.
    assert_eq!(
        t.phone("GET", "/lists", Some(&t.jwt()), None, None, None).0,
        401
    );
}

/// Test 56 (revision 8): a token for another Access application gets 401
/// `access_refused` and the log names both AUDs; no token gets 401 `access_missing`
/// and a log line; `desktop/status` reports the latest problem until a good request.
#[test]
fn at56_refused_access_tokens_are_explained() {
    let t = start();
    let token = t.pair();
    let status = |t: &T| -> Value {
        let (s, b) = t.desktop("GET", "/status", None);
        assert_eq!(s, 200);
        serde_json::from_slice(&b).unwrap()
    };
    assert_eq!(status(&t)["access_problem"], Value::Null);

    let mut other = t.claims();
    other["aud"] = json!(["0123abcd-other-application"]);
    let (s, body) = t.phone(
        "GET",
        "/lists",
        Some(&sign(&t.a, "kid-a", &other)),
        Some(&token),
        None,
        None,
    );
    assert_eq!(s, 401);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["code"], "access_refused");
    assert_eq!(v["error"]["message"], mk_capture_api::msg::ACCESS_REFUSED);
    let line =
        format!("access token audience mismatch: expected {AUD}, got 0123abcd-other-application");
    assert!(t.log_text().contains(&line), "{}", t.log_text());
    let p = status(&t)["access_problem"].clone();
    assert_eq!(p["reason"], line.as_str());
    assert_eq!(p["at"], t.clock.now());

    // No token at all on a phone route.
    let (s, body) = t.phone("GET", "/lists", None, Some(&token), None, None);
    assert_eq!(s, 401);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["code"], "access_missing");
    assert_eq!(v["error"]["message"], mk_capture_api::msg::ACCESS_REFUSED);
    let missing = "request without a Cloudflare Access token on /api/v1/lists \u{2014} is this address behind Access?";
    assert!(t.log_text().contains(missing), "{}", t.log_text());
    assert_eq!(status(&t)["access_problem"]["reason"], missing);
    // The server no longer sends `not_signed_in`, and the log holds no token.
    assert!(!t.log_text().contains("not_signed_in"));
    assert!(!t.log_text().contains(&t.jwt()[..40]));

    // A good request clears it (the desktop's own status calls do not).
    assert_eq!(t.get(&token, "/lists"), 200);
    assert_eq!(status(&t)["access_problem"], Value::Null);
}

/// AT2: an unknown `kid` triggers one certificates refresh; a rotated key works after.
#[test]
fn at02_unknown_kid_refreshes_once() {
    let t = start();
    let token = t.pair();
    let base_hits = t.certs.hits();
    assert_eq!(base_hits, 1, "fetched once on first use");
    let b = key("b");
    let jwt_b = sign(&b, "kid-b", &t.claims());
    // Unknown key, refresh within a minute is not repeated.
    assert_eq!(
        t.phone("GET", "/lists", Some(&jwt_b), Some(&token), None, None)
            .0,
        401
    );
    assert_eq!(
        t.phone("GET", "/lists", Some(&jwt_b), Some(&token), None, None)
            .0,
        401
    );
    assert_eq!(t.certs.hits(), base_hits, "no refetch within a minute");
    // Keys rotate: the new key is published, a minute later the token is accepted
    // after exactly one refresh.
    t.certs.set(json!([jwk("kid-a", &t.a), jwk("kid-b", &b)]));
    t.clock.advance(61);
    let jwt_b = sign(&b, "kid-b", &t.claims());
    assert_eq!(
        t.phone("GET", "/lists", Some(&jwt_b), Some(&token), None, None)
            .0,
        200
    );
    assert_eq!(t.certs.hits(), base_hits + 1);
    assert_eq!(
        t.phone("GET", "/lists", Some(&jwt_b), Some(&token), None, None)
            .0,
        200
    );
    assert_eq!(t.certs.hits(), base_hits + 1, "cached afterwards");
}

/// AT3: pairing codes are single use, expire after 10 minutes, and die after five
/// wrong tries; tokens are stored only as hashes.
#[test]
fn at03_pairing() {
    let t = start();
    let code = t.code();
    assert_eq!(code.len(), 8);
    let (s, v) = t.pair_with(&code.to_lowercase());
    assert_eq!(s, 201);
    let token = v["device_token"].as_str().unwrap().to_owned();
    assert_eq!(token.len(), 43);
    // Reuse.
    assert_eq!(t.pair_with(&code).0, 401);
    // Expiry.
    let code = t.code();
    t.clock.advance(601);
    assert_eq!(t.pair_with(&code).0, 401);
    // Five wrong tries end the code: the sixth try, with the right code, fails.
    let code = t.code();
    for _ in 0..5 {
        let (s, v) = t.pair_with("ZZZZZZZZ");
        assert_eq!(s, 401);
        assert_eq!(v["error"]["code"], "pair_failed");
    }
    assert_eq!(t.pair_with(&code).0, 401);
    // A fresh code still works (the failure pause is per minute and source; wait it
    // out on the test clock).
    t.clock.advance(901);
    let code = t.code();
    assert_eq!(t.pair_with(&code).0, 201);
    let db = t.db();
    assert!(!contains(&db, token.as_bytes()), "device token in the file");
    assert!(
        !contains(&db, t.desktop.as_bytes()),
        "desktop token in the file"
    );
    assert!(!contains(&db, code.as_bytes()), "pairing code in the file");
}

/// AT4: a revoked phone's next request is refused.
#[test]
fn at04_revoked_device() {
    let t = start();
    let token = t.pair();
    assert_eq!(t.get(&token, "/expenses"), 200);
    let (_, b) = t.desktop("GET", "/devices", None);
    let v: Value = serde_json::from_slice(&b).unwrap();
    let id = v["devices"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(v["devices"][0]["name"], "Test phone");
    assert!(v["devices"][0]["last_seen"].is_i64());
    assert_eq!(t.desktop("DELETE", &format!("/devices/{id}"), None).0, 200);
    let (s, b) = t.phone("GET", "/expenses", Some(&t.jwt()), Some(&token), None, None);
    assert_eq!(s, 401);
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["error"]["code"], "not_paired");
    assert_eq!(v["error"]["message"], mk_capture_api::msg::DISCONNECTED);
    let (_, b) = t.desktop("GET", "/devices", None);
    assert!(b.windows(2).any(|w| w == b"[]"));
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let mut c = kind.to_vec();
    c.extend_from_slice(data);
    out.extend_from_slice(&c);
    out.extend(crc32(&c).to_be_bytes());
}

/// A PNG declaring 60,000 × 60,000 pixels whose compressed data is tiny.
fn png_bomb() -> Vec<u8> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend(60_000u32.to_be_bytes());
    ihdr.extend(60_000u32.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let zeros = vec![0u8; 4 * 1024 * 1024];
    let mut z = vec![0x78, 0x9C];
    z.extend(mk_import::image::deflate(&zeros));
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// AT5: uploads are stripped of EXIF; 12 MB → 413; a PNG bomb → 422.
#[test]
fn at05_uploads() {
    let t = start();
    let token = t.pair();
    let jpeg = photo(1200, 900);
    assert!(contains(&jpeg, b"GPSTESTMARK") && contains(&jpeg, b"Exif"));
    let (s, v) = t.upload(&token, "image/jpeg", &jpeg);
    assert_eq!(s, 201, "{v}");
    // Turned upright by the EXIF orientation (6: rotate 90°).
    assert_eq!(
        (v["width"].as_u64(), v["height"].as_u64()),
        (Some(900), Some(1200))
    );
    let id = v["receipt_id"].as_str().unwrap().to_owned();
    let (s, stored) = t.desktop("GET", &format!("/receipts/{id}"), None);
    assert_eq!(s, 200);
    assert!(stored.starts_with(&[0xFF, 0xD8]));
    assert!(!contains(&stored, b"GPSTESTMARK"));
    assert!(!contains(&stored, b"Exif"));
    // 12 MB body.
    let big = vec![0xFFu8; 12 * 1024 * 1024];
    assert_eq!(t.upload(&token, "image/jpeg", &big).0, 413);
    // Decompression bomb.
    let (s, v) = t.upload(&token, "image/png", &png_bomb());
    assert_eq!(s, 422);
    assert_eq!(v["error"]["message"], "This picture can't be read.");
    // Not a picture, and HEIC.
    assert_eq!(t.upload(&token, "application/pdf", b"%PDF-1.4").0, 415);
    let (s, v) = t.upload(&token, "image/heic", b"\0\0\0\x18ftypheic");
    assert_eq!(s, 415);
    assert_eq!(v["error"]["message"], mk_capture_api::msg::PHOTO_TYPE);
}

/// AT6: the same `client_id` twice creates one submission.
#[test]
fn at06_idempotent_submissions() {
    let t = start();
    let token = t.pair();
    let client = "0b0e9c3a-1111-4222-8333-944455556666";
    let (s1, v1) = t.post_json(&token, "/expenses", &expense(client, "45.20", &[]));
    assert_eq!(s1, 201, "{v1}");
    let (s2, v2) = t.post_json(&token, "/expenses", &expense(client, "45.20", &[]));
    assert_eq!(s2, 200);
    assert_eq!(v1["id"], v2["id"]);
    let (_, b) = t.phone("GET", "/expenses", Some(&t.jwt()), Some(&token), None, None);
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["waiting"].as_array().unwrap().len(), 1);
    let (_, b) = t.desktop("GET", "/inbox", None);
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
}

/// AT7: refused fields come back with messages.
#[test]
fn at07_field_validation() {
    let t = start();
    let token = t.pair();
    let today = mk_capture_api::today_from_unix(t.clock.now(), 0);
    let cases = [
        (json!({"amount": "0"}), "The amount must be more than zero."),
        (
            json!({"amount": "1.005"}),
            "The amount can have at most 2 decimal places.",
        ),
        (
            json!({"purpose": "p".repeat(501)}),
            "Purpose can be at most 500 characters.",
        ),
        (
            json!({"date": mk_capture_api::format_date(today + 730)}),
            "The date must be within 400 days of today.",
        ),
    ];
    for (i, (change, message)) in cases.into_iter().enumerate() {
        let mut e = expense(
            &format!("0b0e9c3a-1111-4222-8333-94445555000{i}"),
            "1.00",
            &[],
        );
        for (k, v) in change.as_object().unwrap() {
            e[k] = v.clone();
        }
        let (s, v) = t.post_json(&token, "/expenses", &e);
        assert_eq!(s, 422, "{message}");
        assert_eq!(v["error"]["message"], message);
        assert_eq!(v["error"]["fields"][0]["message"], message);
    }
}

/// AT8: the inbox file holds neither the merchant nor the picture in clear.
#[test]
fn at08_encrypted_at_rest() {
    let t = start();
    let token = t.pair();
    let jpeg = photo(800, 600);
    let (_, v) = t.upload(&token, "image/jpeg", &jpeg);
    let id = v["receipt_id"].as_str().unwrap().to_owned();
    let (s, _) = t.post_json(
        &token,
        "/expenses",
        &expense(
            "0b0e9c3a-1111-4222-8333-944455550008",
            "45.20",
            std::slice::from_ref(&id),
        ),
    );
    assert_eq!(s, 201);
    let (_, stored) = t.desktop("GET", &format!("/receipts/{id}"), None);
    let db = t.db();
    assert!(!contains(&db, b"Zebra Stationers Unique"));
    assert!(!contains(&db, b"45.20"));
    for window in [&jpeg[..], &stored[..]] {
        let mid = window.len() / 2;
        assert!(!contains(&db, &window[mid..mid + 64]));
    }
}

/// AT9: unsafe listen settings refuse to start.
#[test]
fn at09_unsafe_listen_refused() {
    let dir = tempfile::tempdir().unwrap();
    let base = |config: Config, allow_lan: bool| Options {
        data_dir: dir.path().to_path_buf(),
        config,
        allow_lan,
        key_home: KeyHome::File,
        certs: None,
        log: Arc::new(|_: &str| {}),
        clock: Arc::new(Clock::default()),
        threads: 1,
        phone_app: None,
        credentials: None,
        pin_cost: moneykeep_server::pin::COST,
        bind_wait: moneykeep_server::BIND_WAIT,
        address_present: None,
        wifi_recheck: moneykeep_server::WIFI_RECHECK,
    };
    let lan = Config {
        listen: "0.0.0.0:0".into(),
        access_team_domain: TEAM.into(),
        access_aud: AUD.into(),
        ..Config::default()
    };
    let e = moneykeep_server::start(base(lan.clone(), false)).unwrap_err();
    assert!(e.contains("--allow-lan"), "{e}");
    let open = Config {
        access_required: false,
        ..lan
    };
    let e = moneykeep_server::start(base(open, true)).unwrap_err();
    assert!(e.contains("access_required"), "{e}");
}

/// AT10: logs of a full submission hold no token, merchant or amount.
#[test]
fn at10_logs_hold_no_secrets() {
    let t = start();
    let token = t.pair();
    let (_, v) = t.upload(&token, "image/jpeg", &photo(640, 480));
    let id = v["receipt_id"].as_str().unwrap().to_owned();
    let (s, v) = t.post_json(
        &token,
        "/expenses",
        &expense("0b0e9c3a-1111-4222-8333-944455550010", "45.20", &[id]),
    );
    assert_eq!(s, 201);
    let sub = v["id"].as_str().unwrap().to_owned();
    t.desktop("GET", "/inbox", None);
    assert_eq!(t.desktop("POST", &format!("/inbox/{sub}/ack"), None).0, 200);
    let logs = t.log_text();
    assert!(logs.contains("POST /api/v1/expenses 201"), "{logs}");
    assert!(
        logs.contains("POST /api/v1/desktop/inbox/{id}/ack 200"),
        "{logs}"
    );
    for secret in [
        token.as_str(),
        t.desktop.as_str(),
        "Zebra",
        "45.20",
        sub.as_str(),
    ] {
        assert!(!logs.contains(secret), "log contains {secret}");
    }
}

/// The inbox limit answers 507 with the storage message.
#[test]
fn inbox_full() {
    let t = start_with(|c| c.inbox_limit_mb = 1);
    let token = t.pair();
    let mut last = 0;
    for _ in 0..8 {
        let (s, v) = t.upload(&token, "image/jpeg", &photo(1600, 1200));
        last = s;
        if s == 507 {
            assert_eq!(v["error"]["message"], mk_capture_api::msg::STORAGE_FULL);
            break;
        }
    }
    assert_eq!(last, 507);
}

/// Waiting submissions can be changed or deleted; collected ones cannot.
#[test]
fn edits_only_while_waiting() {
    let t = start();
    let token = t.pair();
    let (_, v) = t.post_json(
        &token,
        "/expenses",
        &expense("0b0e9c3a-1111-4222-8333-944455550011", "3.00", &[]),
    );
    let id = v["id"].as_str().unwrap().to_owned();
    let patch = json!({"date": v["date"], "merchant": "Cafe", "amount": "4.00"}).to_string();
    let (s, b) = t.phone(
        "PATCH",
        &format!("/expenses/{id}"),
        Some(&t.jwt()),
        Some(&token),
        Some("application/json"),
        Some(patch.as_bytes()),
    );
    assert_eq!(s, 200);
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["amount"], "4.00");
    assert_eq!(t.desktop("POST", &format!("/inbox/{id}/ack"), None).0, 200);
    let (s, _) = t.phone(
        "PATCH",
        &format!("/expenses/{id}"),
        Some(&t.jwt()),
        Some(&token),
        Some("application/json"),
        Some(patch.as_bytes()),
    );
    assert_eq!(s, 409);
    let (s, _) = t.phone(
        "DELETE",
        &format!("/expenses/{id}"),
        Some(&t.jwt()),
        Some(&token),
        None,
        None,
    );
    assert_eq!(s, 409);
    let (_, b) = t.phone("GET", "/expenses", Some(&t.jwt()), Some(&token), None, None);
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["collected"][0]["status"], "collected");
    assert!(v["waiting"].as_array().unwrap().is_empty());
    let _ = ExpenseFields::default();
    let _ = NewExpense::default();
}

/// A GET returning status, headers and body (no proxy).
fn get_full(
    base: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> (u16, Vec<(String, String)>, Vec<u8>) {
    use std::io::Read;
    let agent = ureq::Agent::config_builder()
        .proxy(None)
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut r = agent.get(&format!("{base}{path}"));
    for (k, v) in headers {
        r = r.header(*k, *v);
    }
    let mut resp = r.call().unwrap();
    let status = resp.status().as_u16();
    let hs = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned()))
        .collect();
    let mut body = Vec::new();
    resp.body_mut().as_reader().read_to_end(&mut body).unwrap();
    (status, hs, body)
}

fn header_of<'a>(hs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    hs.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// The security headers exactly as listed in mobile-capture.md.
fn assert_security_headers(hs: &[(String, String)], what: &str) {
    for (k, v) in [
        (
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; img-src 'self' blob: data:; connect-src 'self'; style-src 'self' 'unsafe-inline'; worker-src 'self'; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        ),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("Permissions-Policy", "camera=(self), geolocation=()"),
    ] {
        assert_eq!(header_of(hs, k), Some(v), "{what}: {k}");
    }
}

/// AT19 (revision 2) and AT29: the server serves the phone app at `/` with the
/// security headers; API answers carry them too.
#[test]
fn at19_at29_phone_app_with_security_headers() {
    let app = PhoneApp::from_files([
        (
            "/index.html".to_owned(),
            b"<!DOCTYPE html><canvas id=\"capture_canvas\"></canvas>".to_vec(),
        ),
        (
            "/moneykeep-capture_bg.wasm".to_owned(),
            vec![0, 97, 115, 109, 1],
        ),
        (
            "/start.js".to_owned(),
            b"import init from './moneykeep-capture.js';".to_vec(),
        ),
    ]);
    let t = start_full(|_| {}, Some(app));
    let (s, hs, body) = get_full(&t.base(), "/", &[]);
    assert_eq!(s, 200);
    assert!(String::from_utf8_lossy(&body).contains("capture_canvas"));
    assert_eq!(
        header_of(&hs, "Content-Type"),
        Some("text/html; charset=utf-8")
    );
    assert_security_headers(&hs, "/");
    let (s, hs, body) = get_full(&t.base(), "/moneykeep-capture_bg.wasm", &[]);
    assert_eq!((s, body.len()), (200, 5));
    assert_eq!(header_of(&hs, "Content-Type"), Some("application/wasm"));
    assert_security_headers(&hs, "wasm");
    let (s, hs, _) = get_full(&t.base(), "/start.js", &[]);
    assert_eq!(s, 200);
    assert_eq!(
        header_of(&hs, "Content-Type"),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(get_full(&t.base(), "/missing.js", &[]).0, 404);
    // API answers carry the same headers.
    let (s, hs, _) = get_full(&t.base(), "/api/v1/lists", &[]);
    assert_eq!(s, 401);
    assert_security_headers(&hs, "api");
}

/// AT29: a server built without the phone app explains how to build it.
#[test]
fn at29_without_embedded_app_explains() {
    let t = start();
    for path in ["/", "/index.html"] {
        let (s, hs, body) = get_full(&t.base(), path, &[]);
        assert_eq!(s, 200);
        let text = String::from_utf8_lossy(&body).to_string();
        assert!(
            text.contains(
                "The phone app wasn't built into this server — run cargo xtask dist-capture"
            ),
            "{text}"
        );
        assert_security_headers(&hs, path);
    }
}

/// AT28: a service-token identity may call the health route and the desktop API
/// (with the desktop token) but not the phone API; health shows no counts or names.
#[test]
fn at28_service_token_identity() {
    let t = start_with(|c| c.access_service_client_id = "status-id.access".into());
    let phone_token = t.pair();
    let now = t.clock.now();
    let service = |cn: &str| {
        sign(
            &t.a,
            "kid-a",
            &json!({"iss": format!("https://{TEAM}"), "aud": [AUD], "exp": now + 300,
                    "iat": now - 10, "common_name": cn, "sub": "", "type": "app"}),
        )
    };
    let svc = service("status-id.access");
    // Health: only ok, version, (release-packaging.md) the capture version, which
    // is the one the desktop was built with, and (upgrade-resilience-1) the API level.
    let (s, _, body) = get_full(
        &t.base(),
        "/api/v1/health",
        &[("Cf-Access-Jwt-Assertion", &svc)],
    );
    assert_eq!(s, 200);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        v,
        json!({"ok": true, "version": moneykeep_server::VERSION,
               "capture_version": mk_capture_api::CAPTURE_VERSION,
               "api_level": mk_capture_api::API_LEVEL})
    );
    assert_eq!(
        moneykeep_server::CAPTURE_VERSION,
        mk_capture_api::CAPTURE_VERSION
    );
    // Health on the local address needs nothing.
    assert_eq!(get_full(&t.base(), "/api/v1/health", &[]).0, 200);
    // The desktop API with the desktop token.
    let bearer = format!("Bearer {}", t.desktop);
    let (s, _, body) = get_full(
        &t.base(),
        "/api/v1/desktop/status",
        &[
            ("Cf-Access-Jwt-Assertion", &svc),
            ("Authorization", &bearer),
        ],
    );
    assert_eq!(s, 200);
    let st: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(st["waiting"], 0);
    assert_eq!(st["version"], moneykeep_server::VERSION);
    // ... but not without it.
    assert_eq!(
        get_full(
            &t.base(),
            "/api/v1/desktop/status",
            &[("Cf-Access-Jwt-Assertion", &svc)]
        )
        .0,
        401
    );
    // Not the phone API, even with a valid device token.
    assert_eq!(
        t.phone("GET", "/lists", Some(&svc), Some(&phone_token), None, None)
            .0,
        401
    );
    let pair = json!({"code": t.code(), "device_name": "x"}).to_string();
    assert_eq!(
        t.phone(
            "POST",
            "/pair",
            Some(&svc),
            None,
            Some("application/json"),
            Some(pair.as_bytes())
        )
        .0,
        401
    );
    // Another service token's identity is refused everywhere.
    let other = service("someone-else.access");
    assert_eq!(
        get_full(
            &t.base(),
            "/api/v1/health",
            &[("Cf-Access-Jwt-Assertion", &other)]
        )
        .0,
        401
    );
    // A person's identity still works for the phone API.
    assert_eq!(t.get(&phone_token, "/lists"), 200);
    // A token naming nobody is refused.
    let nobody = sign(
        &t.a,
        "kid-a",
        &json!({"iss": format!("https://{TEAM}"), "aud": [AUD], "exp": now + 300}),
    );
    assert_eq!(
        t.phone(
            "GET",
            "/lists",
            Some(&nobody),
            Some(&phone_token),
            None,
            None
        )
        .0,
        401
    );
}

/// Revision 2 section C: the desktop status shows waiting items and the newest
/// submission's time.
#[test]
fn desktop_status_counts_and_activity() {
    let t = start();
    let token = t.pair();
    let (s, b) = t.desktop("GET", "/status", None);
    assert_eq!(s, 200);
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["waiting"], 0);
    assert!(v["last_activity"].is_null());
    let (s, _) = t.post_json(
        &token,
        "/expenses",
        &expense("0b0e9c3a-1111-4222-8333-944455550028", "3.00", &[]),
    );
    assert_eq!(s, 201);
    let (_, b) = t.desktop("GET", "/status", None);
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["waiting"], 1);
    assert!(v["last_activity"].as_i64().unwrap() >= t.clock.now() - 5);
    let logs = t.log_text();
    assert!(logs.contains("GET /api/v1/desktop/status 200"), "{logs}");
}

/// AT26 (server part): even a request target carrying a pairing fragment is logged
/// by its template only, so a code could never reach the logs.
#[test]
fn at26_fragment_never_logged() {
    use std::io::{Read, Write};
    let t = start();
    let mut s = std::net::TcpStream::connect(t.running.addr()).unwrap();
    s.write_all(b"GET /#pair=ABCD2345 HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    assert!(String::from_utf8_lossy(&out).starts_with("HTTP/1.1 200"));
    let logs = t.log_text();
    assert!(!logs.contains("ABCD2345"), "{logs}");
    assert!(logs.contains("GET (page) 200"), "{logs}");
}

/// The phone app's real web files (manifest, service worker, icons) as the server
/// would embed them.
fn web_files() -> PhoneApp {
    let web = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../moneykeep-capture/web");
    let index = web.join("../index.html");
    let mut files = vec![("/index.html".to_owned(), std::fs::read(index).unwrap())];
    for name in ["manifest.webmanifest", "sw.js", "start.js", "shell.js"] {
        files.push((format!("/{name}"), std::fs::read(web.join(name)).unwrap()));
    }
    for name in [
        "icon-192.png",
        "icon-512.png",
        "icon-512-maskable.png",
        "apple-touch-icon.png",
    ] {
        files.push((
            format!("/icons/{name}"),
            std::fs::read(web.join("icons").join(name)).unwrap(),
        ));
    }
    PhoneApp::from_files(files)
}

/// mobile-capture.md AT31 (serving) and AT35: the manifest as
/// application/manifest+json, the service worker with Cache-Control: no-cache, the
/// icons as PNG, and the CSP allowing the worker and the manifest.
#[test]
fn at31_at35_installable_app_is_served() {
    let t = start_full(|_| {}, Some(web_files()));
    let (s, hs, body) = get_full(&t.base(), "/manifest.webmanifest", &[]);
    assert_eq!(s, 200);
    assert_eq!(
        header_of(&hs, "Content-Type"),
        Some("application/manifest+json")
    );
    let m: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(m["start_url"], "/");
    let (s, hs, body) = get_full(&t.base(), "/sw.js", &[]);
    assert_eq!(s, 200);
    assert_eq!(header_of(&hs, "Cache-Control"), Some("no-cache"));
    assert!(
        header_of(&hs, "Content-Type")
            .unwrap()
            .starts_with("text/javascript")
    );
    assert!(String::from_utf8_lossy(&body).contains("/api/"));
    // The page script start.js imports (mobile-capture.md revision 9).
    let (s, hs, _) = get_full(&t.base(), "/shell.js", &[]);
    assert_eq!(s, 200);
    assert!(
        header_of(&hs, "Content-Type")
            .unwrap()
            .starts_with("text/javascript")
    );
    for (name, side) in [
        ("icon-192.png", 192),
        ("icon-512.png", 512),
        ("apple-touch-icon.png", 180),
    ] {
        let (s, hs, body) = get_full(&t.base(), &format!("/icons/{name}"), &[]);
        assert_eq!(s, 200, "{name}");
        assert_eq!(header_of(&hs, "Content-Type"), Some("image/png"));
        // The width and height in the PNG header (IHDR).
        let w = u32::from_be_bytes(body[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(body[20..24].try_into().unwrap());
        assert_eq!((w, h), (side, side), "{name}");
    }
    let csp = header_of(&hs_of(&t, "/"), "Content-Security-Policy")
        .unwrap()
        .to_owned();
    assert!(
        csp.contains("worker-src 'self'") && csp.contains("manifest-src 'self'"),
        "{csp}"
    );
}

fn hs_of(t: &T, path: &str) -> Vec<(String, String)> {
    get_full(&t.base(), path, &[]).1
}

fn trip_json(client: &str) -> Value {
    json!({"client_id": client, "date": mk_capture_api::format_date(
        mk_capture_api::today_from_unix(Clock::default().now(), 0)),
        "vehicle_id": "veh-1", "purpose_kind": "business", "purpose": "Client visit",
        "from_place": "Office", "to_place": "Client", "miles": "12.4", "round_trip": true,
        "reimbursable": false})
}

/// mileage.md M12: a trip with miles 0, an unknown purpose kind or a 600-character
/// purpose is refused with 400 and the reason; an older-API phone (lists without
/// asking for mileage, expenses as before) keeps working.
#[test]
fn m12_trips_are_checked_and_older_phones_keep_working() {
    let t = start();
    let token = t.pair();
    let cid = "8d2f1b6c-3e4a-4b5c-9d6e-7f8091a2b3c4";
    for (field, value, reason) in [
        ("miles", json!("0"), "Miles must be more than 0."),
        (
            "purpose_kind",
            json!("vacation"),
            "The purpose kind must be business, medical, moving, charity or personal.",
        ),
        (
            "purpose",
            json!("p".repeat(600)),
            "Purpose can be at most 500 characters.",
        ),
    ] {
        let mut body = trip_json(cid);
        body[field] = value;
        let (s, v) = t.post_json(&token, "/trips", &body);
        assert_eq!(s, 400, "{field}: {v}");
        assert_eq!(v["error"]["message"], reason, "{field}");
        assert_eq!(v["error"]["fields"][0]["field"], field);
    }
    // Nothing was stored.
    let (_, inbox) = t.desktop("GET", "/inbox", None);
    let inbox: Value = serde_json::from_slice(&inbox).unwrap();
    assert!(inbox.get("trips").is_none(), "{inbox}");
    // The desktop publishes lists with mileage; a phone that does not ask gets the
    // lists it knows (its parser refuses unknown fields), one that asks gets them.
    let lists = json!({"book_id": "b1", "categories": ["Expenses:Office"],
        "mileage_enabled": true, "default_purpose": "business",
        "vehicles": [{"id": "veh-1", "name": "2019 Honda CR-V"}],
        "saved_trips": [{"id": "s1", "name": "Office to client", "from_place": "Office",
            "to_place": "Client", "miles": "12.4", "round_trip": true,
            "purpose_kind": "business", "purpose": "Client visit"}],
        "mileage_rates": [{"kind": "business", "from": "2026-01-01", "cents": "72.5"}]});
    let (s, _) = t.desktop("PUT", "/lists", Some(lists.to_string().as_bytes()));
    assert_eq!(s, 200);
    let (s, old) = t.phone("GET", "/lists", Some(&t.jwt()), Some(&token), None, None);
    assert_eq!(s, 200);
    let old: mk_capture_api::Lists = serde_json::from_slice(&old).unwrap();
    assert!(!old.has_mileage());
    assert_eq!(old.categories, ["Expenses:Office"]);
    let (s, new) = t.phone(
        "GET",
        "/lists?mileage=1",
        Some(&t.jwt()),
        Some(&token),
        None,
        None,
    );
    assert_eq!(s, 200);
    let new: mk_capture_api::Lists = serde_json::from_slice(&new).unwrap();
    assert!(new.mileage_enabled);
    assert_eq!(new.vehicles[0].name, "2019 Honda CR-V");
    assert_eq!(new.saved_trips[0].miles, "12.4");
    // Expenses work as before.
    let (s, _) = t.post_json(
        &token,
        "/expenses",
        &expense("6a1c2b3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d", "4.50", &[]),
    );
    assert_eq!(s, 201);
}

/// mileage.md M11 (server part, the Cloudflare way in): a trip is stored
/// encrypted, a resend of its client_id makes nothing new, it can be changed and
/// deleted while waiting, the desktop sees it in the inbox, and its status (taken
/// with the book's amount, or refused with the reason) reaches the phone.
#[test]
fn m11_trips_wait_are_taken_and_refused() {
    let t = start();
    let token = t.pair();
    let cid = "8d2f1b6c-3e4a-4b5c-9d6e-7f8091a2b3c4";
    let mut body = trip_json(cid);
    body["purpose"] = json!("Zebra pantry delivery unique");
    let (s, first) = t.post_json(&token, "/trips", &body);
    assert_eq!(s, 201, "{first}");
    assert_eq!(first["status"], "waiting");
    let (s, again) = t.post_json(&token, "/trips", &body);
    assert_eq!(s, 200, "a resend returns the stored trip");
    assert_eq!(again["id"], first["id"]);
    assert!(
        !contains(&t.db(), b"Zebra pantry"),
        "trip fields are encrypted"
    );
    let id = first["id"].as_str().unwrap().to_owned();
    // Changed while waiting.
    body["miles"] = json!("13");
    let b = body.to_string();
    let (s, v) = t.phone(
        "PATCH",
        &format!("/trips/{id}"),
        Some(&t.jwt()),
        Some(&token),
        Some("application/json"),
        Some(b.as_bytes()),
    );
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&v));
    // The desktop's inbox lists it; the status counts it.
    let (_, inbox) = t.desktop("GET", "/inbox", None);
    let inbox: mk_capture_api::Inbox = serde_json::from_slice(&inbox).unwrap();
    assert_eq!(inbox.trips.len(), 1);
    assert_eq!(inbox.trips[0].fields.miles, "13.0");
    assert_eq!(inbox.trips[0].device_name, "Test phone");
    let (_, st) = t.desktop("GET", "/status", None);
    let st: Value = serde_json::from_slice(&st).unwrap();
    assert_eq!(st["waiting"], 1);
    // Taken, with the book's amount.
    let ack = json!({"amount": "18.85"}).to_string();
    let (s, _) = t.desktop("POST", &format!("/trips/{id}/ack"), Some(ack.as_bytes()));
    assert_eq!(s, 200);
    let (s, list) = t.phone("GET", "/trips", Some(&t.jwt()), Some(&token), None, None);
    assert_eq!(s, 200);
    let list: mk_capture_api::TripList = serde_json::from_slice(&list).unwrap();
    assert!(list.waiting.is_empty());
    assert_eq!(list.done[0].status, mk_capture_api::TripStatus::Taken);
    assert_eq!(list.done[0].amount.as_deref(), Some("18.85"));
    // Taken trips cannot change or go.
    let (s, _) = t.phone(
        "DELETE",
        &format!("/trips/{id}"),
        Some(&t.jwt()),
        Some(&token),
        None,
        None,
    );
    assert_eq!(s, 409);
    // A second trip, refused by the desktop with a reason; a third deleted while
    // waiting.
    let (_, second) = t.post_json(
        &token,
        "/trips",
        &trip_json("1b2c3d4e-5f60-4718-8293-a4b5c6d7e8f9"),
    );
    let second = second["id"].as_str().unwrap().to_owned();
    let ack = json!({"refused": "Mileage is turned off in this book."}).to_string();
    let (s, _) = t.desktop(
        "POST",
        &format!("/trips/{second}/ack"),
        Some(ack.as_bytes()),
    );
    assert_eq!(s, 200);
    let (_, third) = t.post_json(
        &token,
        "/trips",
        &trip_json("2b2c3d4e-5f60-4718-8293-a4b5c6d7e8f9"),
    );
    let third = third["id"].as_str().unwrap().to_owned();
    let (s, _) = t.phone(
        "DELETE",
        &format!("/trips/{third}"),
        Some(&t.jwt()),
        Some(&token),
        None,
        None,
    );
    assert_eq!(s, 200);
    let (_, list) = t.phone("GET", "/trips", Some(&t.jwt()), Some(&token), None, None);
    let list: mk_capture_api::TripList = serde_json::from_slice(&list).unwrap();
    let refused = list.done.iter().find(|x| x.id == second).unwrap();
    assert_eq!(refused.status, mk_capture_api::TripStatus::Refused);
    assert_eq!(
        refused.reason.as_deref(),
        Some("Mileage is turned off in this book.")
    );
    assert_eq!(list.done.len(), 2);
    // Without the Access sign-in the trip route is refused like every other.
    let (s, _) = t.phone("GET", "/trips", None, Some(&token), None, None);
    assert_eq!(s, 401);
    // The log has no trip contents.
    assert!(!t.log_text().contains("Client visit"));
}

/// mobile-capture.md test 84 (served): `sw.js` as the server answers it names this
/// build's cache (`moneykeep-capture-<capture_version>`), not the placeholder and
/// not the old `-v4`; the page carries the same build.
#[test]
fn at84_service_worker_cache_is_named_for_the_build() {
    let t = start_full(|_| {}, Some(web_files()));
    let (s, _, body) = get_full(&t.base(), "/sw.js", &[]);
    assert_eq!(s, 200);
    let sw = String::from_utf8(body).unwrap();
    let name = format!(
        "const VERSION = \"moneykeep-capture-{}\";",
        moneykeep_server::CAPTURE_VERSION
    );
    assert!(sw.contains(&name), "{name}");
    assert!(!sw.contains("__MK_CAPTURE_VERSION__"));
    assert!(!sw.contains("moneykeep-capture-v4"));
    let (s, _, body) = get_full(&t.base(), "/", &[]);
    assert_eq!(s, 200);
    let page = String::from_utf8(body).unwrap();
    assert!(page.contains(&format!(
        "<meta name=\"moneykeep-capture-version\" content=\"{}\">",
        moneykeep_server::CAPTURE_VERSION
    )));
}

/// mobile-capture.md test 102 (revision 18): `Accept-Encoding: br` gets the Brotli
/// copy with `Content-Encoding: br` and `Vary`; gzip only gets gzip; none (or
/// `br;q=0`) gets the file itself; the copies decompress to the original; the
/// program carries `X-MK-Program-Size` with its uncompressed size.
#[test]
fn t102_precompressed_phone_files() {
    use std::io::Read;
    let program: Vec<u8> = (0..200_000u32)
        .map(|i| u8::try_from((i / 7) % 50).unwrap())
        .collect();
    let glue = "export default function init() { return 1; }\n".repeat(200);
    let index = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../moneykeep-capture/index.html"),
    )
    .unwrap();
    let app = PhoneApp::from_files([
        ("/index.html".to_owned(), index),
        ("/moneykeep-capture_bg.wasm".to_owned(), program.clone()),
        (
            "/moneykeep-capture.js".to_owned(),
            glue.clone().into_bytes(),
        ),
        ("/icons/icon-192.png".to_owned(), vec![137, 80, 78, 71]),
    ]);
    let t = start_full(|_| {}, Some(app));
    let unbrotli = |b: &[u8]| {
        let mut out = Vec::new();
        brotli::Decompressor::new(b, 4096)
            .read_to_end(&mut out)
            .unwrap();
        out
    };
    let ungzip = |b: &[u8]| {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(b)
            .read_to_end(&mut out)
            .unwrap();
        out
    };
    let path = "/moneykeep-capture_bg.wasm";
    // Brotli first when both are accepted.
    let (s, hs, body) = get_full(&t.base(), path, &[("Accept-Encoding", "gzip, deflate, br")]);
    assert_eq!(s, 200);
    assert_eq!(header_of(&hs, "Content-Encoding"), Some("br"));
    assert_eq!(header_of(&hs, "Vary"), Some("Accept-Encoding"));
    assert_eq!(header_of(&hs, "Content-Type"), Some("application/wasm"));
    let size = program.len().to_string();
    assert_eq!(header_of(&hs, "X-MK-Program-Size"), Some(size.as_str()));
    assert!(body.len() < program.len() / 4, "{}", body.len());
    assert_eq!(unbrotli(&body), program);
    assert_security_headers(&hs, "br");
    // gzip only.
    let (_, hs, body) = get_full(&t.base(), path, &[("Accept-Encoding", "gzip")]);
    assert_eq!(header_of(&hs, "Content-Encoding"), Some("gzip"));
    assert_eq!(header_of(&hs, "Vary"), Some("Accept-Encoding"));
    assert_eq!(ungzip(&body), program);
    // Brotli refused with q=0.
    let (_, hs, _) = get_full(
        &t.base(),
        path,
        &[("Accept-Encoding", "br;q=0, gzip;q=0.5")],
    );
    assert_eq!(header_of(&hs, "Content-Encoding"), Some("gzip"));
    // None: the file itself, still with Vary and the size.
    for accept in [None, Some("identity"), Some("br;q=0, gzip;q=0")] {
        let headers: Vec<(&str, &str)> =
            accept.map(|a| ("Accept-Encoding", a)).into_iter().collect();
        let (_, hs, body) = get_full(&t.base(), path, &headers);
        assert_eq!(header_of(&hs, "Content-Encoding"), None, "{accept:?}");
        assert_eq!(header_of(&hs, "Vary"), Some("Accept-Encoding"));
        assert_eq!(header_of(&hs, "X-MK-Program-Size"), Some(size.as_str()));
        assert_eq!(body, program);
    }
    // The glue and the page are compressed too; only the program carries the size.
    let (_, hs, body) = get_full(
        &t.base(),
        "/moneykeep-capture.js",
        &[("Accept-Encoding", "br")],
    );
    assert_eq!(header_of(&hs, "Content-Encoding"), Some("br"));
    assert_eq!(header_of(&hs, "X-MK-Program-Size"), None);
    assert_eq!(unbrotli(&body), glue.as_bytes());
    let (_, hs, body) = get_full(&t.base(), "/", &[("Accept-Encoding", "br")]);
    assert_eq!(header_of(&hs, "Content-Encoding"), Some("br"));
    let page = String::from_utf8(unbrotli(&body)).unwrap();
    // The copy carries the build, written in before compressing.
    assert!(page.contains(moneykeep_server::CAPTURE_VERSION), "stamped");
    assert!(!page.contains("__MK_CAPTURE_VERSION__"));
    // Pictures are sent as they are.
    let (_, hs, body) = get_full(
        &t.base(),
        "/icons/icon-192.png",
        &[("Accept-Encoding", "br")],
    );
    assert_eq!(header_of(&hs, "Content-Encoding"), None);
    assert_eq!(header_of(&hs, "Vary"), None);
    assert_eq!(body, vec![137, 80, 78, 71]);
}

/// upgrade-resilience-1 UR-7 (M8), the server's half: every phone app file carries
/// its build in `X-MK-Build` (the capture version stamped into `sw.js` and the
/// page), a query string such as the page's `?b=<build>` is ignored, and the page
/// asks for start.js with its build.
#[test]
fn ur7_phone_files_carry_their_build() {
    let index = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../moneykeep-capture/index.html"),
    )
    .unwrap();
    let sw = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../moneykeep-capture/web/sw.js"),
    )
    .unwrap();
    let app = PhoneApp::from_files([
        ("/index.html".to_owned(), index),
        ("/sw.js".to_owned(), sw),
        ("/start.js".to_owned(), b"// start".to_vec()),
        (
            "/moneykeep-capture_bg.wasm".to_owned(),
            vec![0, 97, 115, 109],
        ),
    ]);
    let t = start_full(|_| {}, Some(app));
    let build = moneykeep_server::CAPTURE_VERSION;
    for path in [
        "/",
        "/index.html",
        "/sw.js",
        "/start.js",
        &format!("/start.js?b={build}"),
        &format!("/moneykeep-capture_bg.wasm?b={build}"),
    ] {
        let (s, hs, _) = get_full(&t.base(), path, &[]);
        assert_eq!(s, 200, "{path}");
        assert_eq!(header_of(&hs, "X-MK-Build"), Some(build), "{path}");
    }
    let (_, _, page) = get_full(&t.base(), "/", &[]);
    let page = String::from_utf8(page).unwrap();
    assert!(
        page.contains(&format!(r#"src="start.js?b={build}""#)),
        "the page asks for its own build's start.js"
    );
    let (_, _, sw) = get_full(&t.base(), "/sw.js", &[]);
    assert!(
        String::from_utf8(sw)
            .unwrap()
            .contains(&format!(r#""moneykeep-capture-{build}""#))
    );
    let (s, hs, _) = get_full(&t.base(), "/nothing.js", &[]);
    assert_eq!(s, 404);
    assert_eq!(header_of(&hs, "X-MK-Build"), None);
}

/// security-review-1 SR-A5 (L2): a forged `CF-Connecting-IP` without an Access token
/// is not believed: tunnel requests without a valid token count together, so
/// changing the header does not escape the failure limit, and they never block this
/// computer's own requests.
#[test]
fn sr_a5_l2_forged_connecting_ip_is_ignored() {
    let t = start();
    let mut last = 0;
    for i in 0..12 {
        let ip = format!("203.0.113.{i}");
        let (s, _) = call(
            &t.base(),
            "GET",
            "/api/v1/lists",
            &[("CF-Connecting-IP", &ip), ("CF-Ray", "8f0c0ffee-LHR")],
            None,
        )
        .unwrap();
        last = s;
    }
    assert_eq!(last, 429, "the forged addresses count as one");
    // This computer's own (desktop) requests are not affected.
    let (s, _) = t.desktop("GET", "/devices", None);
    assert_eq!(s, 200);
    // With a valid Access token, the header is the phone's address again.
    let jwt = t.jwt();
    let (s, _) = call(
        &t.base(),
        "GET",
        "/api/v1/health",
        &[
            ("CF-Connecting-IP", "198.51.100.7"),
            ("Cf-Access-Jwt-Assertion", &jwt),
        ],
        None,
    )
    .unwrap();
    assert_eq!(s, 200);
}

/// SR-A5 (L3): every route reached through the tunnel needs a valid Access token:
/// the desktop API (with the desktop token) and the phone app's files get 403
/// without one; from this computer the desktop API works as before.
#[test]
fn sr_a5_l3_tunnel_routes_need_access() {
    let t = start();
    let bearer = format!("Bearer {}", t.desktop);
    for headers in [
        vec![("CF-Ray", "8f0c0ffee-LHR")],
        vec![("CF-Connecting-IP", "198.51.100.7")],
        vec![("CDN-Loop", "cloudflare")],
    ] {
        let mut h = headers.clone();
        h.push(("Authorization", &bearer));
        let (s, body) = call(&t.base(), "GET", "/api/v1/desktop/devices", &h, None).unwrap();
        assert_eq!(s, 403, "{headers:?}");
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "access_missing");
        let (s, _) = call(&t.base(), "GET", "/", &headers, None).unwrap();
        assert_eq!(s, 403, "page through the tunnel: {headers:?}");
    }
    // With a valid token through the tunnel, the desktop API answers.
    let jwt = t.jwt();
    let (s, _) = call(
        &t.base(),
        "GET",
        "/api/v1/desktop/devices",
        &[
            ("CF-Ray", "8f0c0ffee-LHR"),
            ("Cf-Access-Jwt-Assertion", &jwt),
            ("Authorization", &bearer),
        ],
        None,
    )
    .unwrap();
    assert_eq!(s, 200);
    let (s, _) = call(
        &t.base(),
        "GET",
        "/",
        &[
            ("CF-Ray", "8f0c0ffee-LHR"),
            ("Cf-Access-Jwt-Assertion", &jwt),
        ],
        None,
    )
    .unwrap();
    assert_eq!(s, 200);
    // From this computer, as before.
    let (s, _) = t.desktop("GET", "/devices", None);
    assert_eq!(s, 200);
    // Access off (Wi-Fi only): nothing comes through a tunnel.
    let w = start_with(|c| c.access_required = false);
    let wb = format!("Bearer {}", w.desktop);
    let (s, _) = call(
        &w.base(),
        "GET",
        "/api/v1/desktop/devices",
        &[("CF-Ray", "x"), ("Authorization", &wb)],
        None,
    )
    .unwrap();
    assert_eq!(s, 403);
}

/// SR-A5 (L6): `server.log` rotates at 5 MB keeping 3 old files; a logged method is
/// at most 16 printable characters.
#[test]
fn sr_a5_l6_log_rotates() {
    assert_eq!(moneykeep_server::LOG_ROTATE_BYTES, 5 * 1024 * 1024);
    assert_eq!(moneykeep_server::LOG_KEEP, 3);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.log");
    std::fs::write(&path, vec![b'x'; 5 * 1024 * 1024]).unwrap();
    let log = moneykeep_server::file_log(&path);
    log("after the limit");
    let rotated = dir.path().join("server.log.1");
    assert_eq!(std::fs::metadata(&rotated).unwrap().len(), 5 * 1024 * 1024);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .ends_with("after the limit\n")
    );
    // Three old files are kept, the oldest dropped.
    let small = moneykeep_server::file_log_sized(&path, 10);
    for i in 0..6 {
        small(&format!("line {i}"));
    }
    for n in 1..=3 {
        assert!(dir.path().join(format!("server.log.{n}")).is_file(), "{n}");
    }
    assert!(!dir.path().join("server.log.4").exists());
    assert_eq!(
        moneykeep_server::log_method("GET\u{1}\nAAAAAAAAAAAAAAAAAAAAAAAA"),
        "GETAAAAAAAAAAAAA"
    );
    // A request's method is logged that way.
    let t = start();
    let _ = call(
        &t.base(),
        "PROPFINDXXXXXXXXXXXXXXXXXX",
        "/api/v1/health",
        &[],
        None,
    );
    let logs = t.logs.lock().unwrap().join("\n");
    assert!(!logs.contains("PROPFINDXXXXXXXXXX"), "{logs}");
}
