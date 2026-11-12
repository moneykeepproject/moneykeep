//! connectivity.md transport rules and AT9 against a local HTTPS server on 127.0.0.1
//! with a test-only certificate authority (tests/fixtures). Nothing touches the
//! internet.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

use mk_commands::Session;
use mk_commands::connect::{
    Connector, FetchError, Fetcher, HttpMethod, HttpRequest, MemorySecrets,
};
use mk_connect::HttpsFetcher;
use serde_json::json;

const CA: &[u8] = include_bytes!("fixtures/ca.der");
const CERT: &[u8] = include_bytes!("fixtures/server.der");
const KEY: &[u8] = include_bytes!("fixtures/server.key.der");

/// (status, extra headers, body) for a request (method, path with query, headers).
type Handler = dyn Fn(&str, &str, &str) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + Sync;

/// Start a server; returns its port. It serves until the test process ends.
fn serve(handler: Arc<Handler>) -> u16 {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(CERT.to_vec())],
            rustls::pki_types::PrivateKeyDer::Pkcs8(KEY.to_vec().into()),
        )
        .unwrap();
    let cfg = Arc::new(cfg);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(tcp) = stream else { continue };
            let cfg = cfg.clone();
            let handler = handler.clone();
            std::thread::spawn(move || {
                let Ok(conn) = rustls::ServerConnection::new(cfg) else {
                    return;
                };
                let mut tls = rustls::StreamOwned::new(conn, tcp);
                let mut buf = Vec::new();
                let mut chunk = [0_u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                // Read any request body too, so closing never resets unread data.
                let end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(0) + 4;
                let want = String::from_utf8_lossy(&buf[..end])
                    .lines()
                    .find_map(|l| {
                        let l = l.to_ascii_lowercase();
                        l.strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while buf.len() < end + want {
                    match tls.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf[..end]).into_owned();
                let mut first = head.lines().next().unwrap_or("").split(' ');
                let method = first.next().unwrap_or("").to_owned();
                let path = first.next().unwrap_or("").to_owned();
                let (status, headers, body) = handler(&method, &path, &head);
                let mut out = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                    body.len()
                );
                for (k, v) in headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                let _ = tls.write_all(out.as_bytes());
                let _ = tls.write_all(&body);
                let _ = tls.flush();
                tls.conn.send_close_notify();
                let _ = tls.flush();
            });
        }
    });
    port
}

fn fetcher() -> HttpsFetcher {
    HttpsFetcher::trusting(vec![CA.to_vec()])
}

fn get(
    port: u16,
    path: &str,
    max: usize,
) -> Result<mk_commands::connect::HttpResponse, FetchError> {
    fetcher().send(&HttpRequest {
        method: HttpMethod::Get,
        url: format!("https://localhost:{port}{path}"),
        basic: Some(("demo".into(), "pw".into())),
        authority: format!("localhost:{port}"),
        max_bytes: max,
        ..HttpRequest::default()
    })
}

#[test]
fn basic_auth_and_tls_with_the_test_root() {
    let port = serve(Arc::new(|_, _, head: &str| {
        let ok = head
            .to_ascii_lowercase()
            .contains("authorization: basic zgvtbzpwdw==");
        (if ok { 200 } else { 401 }, vec![], b"hello".to_vec())
    }));
    let r = get(port, "/x", 1024).unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"hello");
    // A fetcher that does not trust the test root refuses the connection.
    let untrusted = HttpsFetcher::trusting(Vec::new()).send(&HttpRequest {
        method: HttpMethod::Get,
        url: format!("https://localhost:{port}/x"),
        basic: None,
        authority: format!("localhost:{port}"),
        max_bytes: 1024,
        ..HttpRequest::default()
    });
    assert!(
        matches!(untrusted, Err(FetchError::Network(_))),
        "{untrusted:?}"
    );
}

#[test]
fn redirects_stay_on_the_host() {
    let port = serve(Arc::new(move |_, path: &str, _| match path {
        "/away" => (
            302,
            vec![("location".into(), "https://other.test/steal".into())],
            vec![],
        ),
        "/hop" => (302, vec![("location".into(), "/final".into())], vec![]),
        "/final" => (200, vec![], b"done".to_vec()),
        _ => (302, vec![("location".into(), "/loop".into())], vec![]),
    }));
    let e = get(port, "/away", 1024).unwrap_err();
    assert!(
        matches!(&e, FetchError::Refused(m) if m.contains("other.test")),
        "{e:?}"
    );
    assert_eq!(get(port, "/hop", 1024).unwrap().body, b"done");
    let e = get(port, "/loop", 1024).unwrap_err();
    assert!(
        matches!(&e, FetchError::Refused(m) if m.contains("redirects")),
        "{e:?}"
    );
    // Requests to another host or over http are refused before connecting.
    let e = fetcher()
        .send(&HttpRequest {
            method: HttpMethod::Get,
            url: "http://localhost/x".into(),
            basic: None,
            authority: "localhost".into(),
            max_bytes: 10,
            ..HttpRequest::default()
        })
        .unwrap_err();
    assert!(matches!(e, FetchError::Refused(_)));
}

#[test]
fn large_answers_refused() {
    let port = serve(Arc::new(|_, _, _| {
        (200, vec![], vec![b' '; 25 * 1024 * 1024])
    }));
    let e = get(port, "/big", 20 * 1024 * 1024).unwrap_err();
    assert!(
        matches!(&e, FetchError::Refused(m) if m.contains("larger than 20 MB")),
        "{e:?}"
    );
}

/// AT9 end to end: a claim and syncs through real HTTPS.
#[test]
fn at9_end_to_end_with_refusals() {
    let deep = format!("{}{}", "[".repeat(100), "]".repeat(100));
    let state = Arc::new(std::sync::Mutex::new(String::new()));
    let s2 = state.clone();
    let port_cell = Arc::new(std::sync::OnceLock::<u16>::new());
    let p2 = port_cell.clone();
    let port = serve(Arc::new(move |method: &str, path: &str, _| {
        let port = p2.get().copied().unwrap_or(0);
        match (method, path.split('?').next().unwrap_or("")) {
            ("POST", "/simplefin/claim/abc") => (
                200,
                vec![],
                format!("https://demo:pw@localhost:{port}/simplefin").into_bytes(),
            ),
            ("GET", "/simplefin/info") => (200, vec![], br#"{"versions":["2"]}"#.to_vec()),
            ("GET", "/simplefin/accounts") => {
                let body = s2.lock().unwrap().clone();
                if body == "redirect" {
                    (
                        302,
                        vec![("location".into(), "https://evil.test/accounts".into())],
                        vec![],
                    )
                } else {
                    (200, vec![], body.into_bytes())
                }
            }
            _ => (404, vec![], vec![]),
        }
    }));
    port_cell.set(port).unwrap();
    *state.lock().unwrap() = r#"{"connections":[{"conn_id":"c1","name":"Bank"}],"accounts":[{"id":"a1","name":"Everyday","conn_id":"c1","currency":"USD","balance":"1.00","balance-date":1790812800,"transactions":[{"id":"t1","posted":1790553600,"amount":"-4.00","description":"Shop"}]}]}"#.into();
    let mut s = Session::in_memory("T", "USD", "app").unwrap();
    s.set_connector(Connector {
        fetcher: std::sync::Arc::new(fetcher()),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(1_790_812_800),
        host: None,
    });
    s.invoke("module.enable", json!({"module": "bank-feeds"}))
        .unwrap();
    s.invoke(
        "account.create",
        json!({"name": "Checking", "type": "asset", "subtype": "bank"}),
    )
    .unwrap();
    use base64::Engine;
    let token = base64::engine::general_purpose::STANDARD
        .encode(format!("https://localhost:{port}/simplefin/claim/abc"));
    let c = s
        .invoke(
            "connect.add",
            json!({"setup_input": token, "confirm": true}),
        )
        .unwrap();
    assert_eq!(c["host"], "localhost");
    assert_eq!(c["version"], 2);
    s.invoke(
        "connect.link_account",
        json!({"connection": c["id"], "remote_account": "a1",
        "account": "Checking", "sync_from": "2026-09-01"}),
    )
    .unwrap();
    let out = s.invoke("connect.sync", json!({"now": true})).unwrap();
    assert_eq!(out["connections"][0]["new"], 1, "{out}");
    // A redirect to another host is refused.
    *state.lock().unwrap() = "redirect".into();
    let out = s.invoke("connect.sync", json!({"now": true})).unwrap();
    assert!(
        out["connections"][0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("evil.test"),
        "{out}"
    );
    // JSON nested 100 deep is refused.
    *state.lock().unwrap() = deep;
    let out = s.invoke("connect.sync", json!({"now": true})).unwrap();
    let msg = out["connections"][0]["error"]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(msg.contains("nesting"), "{msg}");
    assert!(!out.to_string().contains("demo:pw"));
}

/// Processor requests carry headers and a form body (PayPal token request shape).
#[test]
fn headers_and_body_are_sent() {
    let port = serve(Arc::new(|method: &str, _, head: &str| {
        let h = head.to_ascii_lowercase();
        let ok = method == "POST"
            && h.contains("content-type: application/x-www-form-urlencoded")
            && h.contains("accept: application/json");
        (if ok { 200 } else { 400 }, vec![], b"{}".to_vec())
    }));
    let r = fetcher()
        .send(&HttpRequest {
            method: HttpMethod::Post,
            url: format!("https://localhost:{port}/v1/oauth2/token"),
            basic: Some(("id".into(), "secret".into())),
            authority: format!("localhost:{port}"),
            max_bytes: 1024,
            headers: vec![
                ("Accept".into(), "application/json".into()),
                (
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                ),
            ],
            body: Some(b"grant_type=client_credentials".to_vec()),
            allow_http: false,
        })
        .unwrap();
    assert_eq!(r.status, 200);
}
