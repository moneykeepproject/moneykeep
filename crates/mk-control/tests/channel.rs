//! mcp-server.md revision 3 acceptance tests for the app side of the control
//! channel (AT23–AT25), driving the real handler without a window.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mk_commands::Session;
use mk_commands::ai_access::{AiAccess, Group, Level};
use mk_control::client::{ConnectError, ControlClient, Hello};
use mk_control::server::{
    CallReply, CloseReason, ControlEvent, ControlServer, ManualClock, SessionHost, StartError,
    SystemClock,
};
use serde_json::{Value, json};

struct Fixture {
    _dir: tempfile::TempDir,
    book: PathBuf,
    config: PathBuf,
    host: Arc<SessionHost>,
}

fn fixture(access: &AiAccess) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("b.moneykeep");
    let config = dir.path().join("config");
    let mut s = Session::create(&book, "Channel", "USD", "app").unwrap();
    s.set_ai_access(access).unwrap();
    Fixture {
        _dir: dir,
        book,
        config,
        host: Arc::new(SessionHost::new(s)),
    }
}

fn on(level: Level) -> AiAccess {
    AiAccess {
        enabled: true,
        level,
        groups: Default::default(),
        app_only: true,
    }
}

fn start(f: &Fixture, clock: Arc<dyn mk_control::server::Clock>) -> Arc<ControlServer> {
    Arc::new(ControlServer::start(&f.config, f.host.clone(), clock).unwrap())
}

fn token(f: &Fixture) -> String {
    mk_control::read_discovery(&f.config).unwrap().token
}

fn connect(server: &ControlServer) -> (TcpStream, BufReader<TcpStream>) {
    let s = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let r = BufReader::new(s.try_clone().unwrap());
    (s, r)
}

fn hello(token: &str, client: &str, book: Option<&Path>) -> String {
    json!({"hello": "moneykeep-control/1", "token": token,
        "client": {"name": client, "kind": "keep-mcp"},
        "book_path": book.map(|b| b.display().to_string())})
    .to_string()
}

/// The next line, or `None` when the app closed the connection.
fn line(r: &mut BufReader<TcpStream>) -> Option<Value> {
    let mut s = String::new();
    match r.read_line(&mut s) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(serde_json::from_str(&s).unwrap()),
    }
}

fn closed(r: &mut BufReader<TcpStream>) -> bool {
    line(r).is_none()
}

/// Answer approval requests in the background until `stop` is set.
fn answerer(
    server: Arc<ControlServer>,
    allow: bool,
) -> (
    Arc<std::sync::atomic::AtomicBool>,
    std::thread::JoinHandle<Vec<ControlEvent>>,
) {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s = stop.clone();
    let h = std::thread::spawn(move || {
        let mut seen = Vec::new();
        while !s.load(std::sync::atomic::Ordering::SeqCst) {
            for e in server.poll_events() {
                if let ControlEvent::ApprovalRequested { id, .. } = &e {
                    server.answer(*id, allow);
                }
                seen.push(e);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        seen
    });
    (stop, h)
}

fn wait_for(server: &ControlServer, pred: impl Fn(&ControlEvent) -> bool) -> ControlEvent {
    for _ in 0..2000 {
        for e in server.poll_events() {
            if pred(&e) {
                return e;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("event not seen");
}

#[test]
fn at23_control_channel_hello() {
    let f = fixture(&on(Level::ReadWrite));
    let clock = ManualClock::default();
    let server = start(&f, Arc::new(clock.clone()));
    let t = token(&f);

    // Wrong token.
    let (mut s, mut r) = connect(&server);
    writeln!(s, "{}", hello("00", "a", None)).unwrap();
    assert_eq!(line(&mut r).unwrap()["error"]["code"], "bad_token");
    assert!(closed(&mut r));

    // Another book.
    let (mut s, mut r) = connect(&server);
    writeln!(
        s,
        "{}",
        hello(&t, "a", Some(&f.book.with_file_name("other.moneykeep")))
    )
    .unwrap();
    assert_eq!(line(&mut r).unwrap()["error"]["code"], "wrong_book");
    assert!(closed(&mut r));

    // Deny.
    let (mut s, mut r) = connect(&server);
    writeln!(s, "{}", hello(&t, "denied-client", Some(&f.book))).unwrap();
    let ControlEvent::ApprovalRequested { id, client, .. } = wait_for(&server, |e| {
        matches!(e, ControlEvent::ApprovalRequested { .. })
    }) else {
        unreachable!()
    };
    assert_eq!(client, "denied-client");
    server.answer(id, false);
    assert_eq!(line(&mut r).unwrap()["error"]["code"], "denied");
    assert!(closed(&mut r));

    // No answer in 120 seconds (the injected clock).
    let (mut s, mut r) = connect(&server);
    writeln!(s, "{}", hello(&t, "slow", None)).unwrap();
    wait_for(&server, |e| {
        matches!(e, ControlEvent::ApprovalRequested { .. })
    });
    clock.advance(Duration::from_secs(119));
    std::thread::sleep(Duration::from_millis(50));
    clock.advance(Duration::from_secs(2));
    assert_eq!(line(&mut r).unwrap()["error"]["code"], "approval_timeout");
    assert!(closed(&mut r));
    wait_for(&server, |e| matches!(e, ControlEvent::ApprovalEnded { .. }));

    // An HTTP request line: closed with no reply.
    let (mut s, mut r) = connect(&server);
    write!(s, "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
    assert!(closed(&mut r));

    // A line over 4 MiB: closed.
    let (mut s, mut r) = connect(&server);
    let big = "x".repeat(mk_control::MAX_LINE + 10);
    let _ = s.write_all(big.as_bytes());
    assert!(closed(&mut r));

    // Four connections; a fifth hello gets busy.
    let (stop, h) = answerer(server.clone(), true);
    let mut open = Vec::new();
    for _ in 0..4 {
        let (mut s, mut r) = connect(&server);
        writeln!(s, "{}", hello(&t, "four", Some(&f.book))).unwrap();
        assert_eq!(line(&mut r).unwrap()["ok"], true);
        open.push((s, r));
    }
    let (mut s, mut r) = connect(&server);
    writeln!(s, "{}", hello(&t, "four", None)).unwrap();
    assert_eq!(line(&mut r).unwrap()["error"]["code"], "busy");
    assert!(closed(&mut r));
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let events = h.join().unwrap();
    // Allowed once for the (book, client) pair: one approval for four connections.
    let asked = events
        .iter()
        .filter(|e| matches!(e, ControlEvent::ApprovalRequested { .. }))
        .count();
    assert_eq!(asked, 1, "{events:?}");
    drop(open);

    // Access off.
    f.host.with(|s| {
        let mut a = s.ai_access().unwrap();
        a.enabled = false;
        s.set_ai_access(&a).unwrap();
    });
    let (mut s, mut r) = connect(&server);
    writeln!(s, "{}", hello(&t, "late", None)).unwrap();
    assert_eq!(line(&mut r).unwrap()["error"]["code"], "access_off");
    assert!(closed(&mut r));
}

#[test]
fn at24_control_channel_scope() {
    let f = fixture(&on(Level::ReadOnly));
    let server = start(&f, Arc::new(SystemClock::default()));
    let (stop, h) = answerer(server.clone(), true);
    let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let w = wakes.clone();
    let d = mk_control::read_discovery(&f.config).unwrap();
    let (mut client, ok) = ControlClient::connect(
        &d,
        &Hello {
            client: "scope".into(),
            book_path: Some(mk_control::canonical(&f.book)),
        },
        Box::new(move || {
            w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }),
    )
    .unwrap();
    assert_eq!(ok.book_name, "Channel");
    assert_eq!(ok.access.level, Level::ReadOnly);
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    h.join().unwrap();
    // `list` is the exposed set.
    let (commands, access) = client.list().unwrap();
    let names: Vec<&str> = commands
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    let expected: Vec<String> = f.host.with(|s| {
        s.ai_exposed()
            .unwrap()
            .iter()
            .map(|c| c.name.to_owned())
            .collect()
    });
    assert_eq!(names, expected);
    assert_eq!(access.level, Level::ReadOnly);
    assert!(names.contains(&"account.list") && !names.contains(&"txn.create"));
    // A call outside it.
    match client.call("txn.create", json!({})).unwrap() {
        CallReply::NotAvailable(m) => assert!(m.contains("read only"), "{m}"),
        other => panic!("{other:?}"),
    }
    match client.call("account.list", json!({})).unwrap() {
        CallReply::Ok { value, .. } => assert!(value.is_array()),
        other => panic!("{other:?}"),
    }
    // Locking the book: closed/locked, and control.json is removed.
    assert!(mk_control::discovery_path(&f.config).exists());
    server.stop(CloseReason::Locked);
    for _ in 0..400 {
        if client.is_closed() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(client.is_closed());
    assert_eq!(client.close_reason().as_deref(), Some("locked"));
    assert!(wakes.load(std::sync::atomic::Ordering::SeqCst) >= 1);
    assert!(!mk_control::discovery_path(&f.config).exists());
}

#[test]
fn at25_discovery_file() {
    let f = fixture(&on(Level::ReadOnly));
    // A stale file whose process is not running is replaced.
    std::fs::create_dir_all(&f.config).unwrap();
    std::fs::write(
        mk_control::discovery_path(&f.config),
        json!({"version": 1, "port": 1, "token": "ab", "pid": u32::MAX - 7, "book_path": "/x"})
            .to_string(),
    )
    .unwrap();
    let server = start(&f, Arc::new(SystemClock::default()));
    let d = mk_control::read_discovery(&f.config).unwrap();
    assert_eq!(d.version, 1);
    assert_eq!(d.port, server.port());
    assert_eq!(d.pid, std::process::id());
    assert_eq!(d.token.len(), 64);
    assert!(d.token.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(d.book_path, mk_control::canonical(&f.book));
    assert!(mk_control::is_live(&d));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(mk_control::discovery_path(&f.config))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // A second window sees the live file and does not listen.
    let second = ControlServer::start(&f.config, f.host.clone(), Arc::new(SystemClock::default()));
    assert_eq!(second.err(), Some(StartError::OtherWindow));
    assert!(mk_control::discovery_path(&f.config).exists());
    // A new token each time listening starts; stopping deletes the file.
    server.stop(CloseReason::BookClosed);
    assert!(!mk_control::discovery_path(&f.config).exists());
    let again = start(&f, Arc::new(SystemClock::default()));
    assert_ne!(
        mk_control::read_discovery(&f.config).unwrap().token,
        d.token
    );
    drop(again);
    assert!(!mk_control::discovery_path(&f.config).exists());
}

/// Approval is forgotten when the AI access settings change and when the owner
/// presses Disconnect; Disconnect sends closed/disconnected.
#[test]
fn approvals_are_forgotten_on_settings_change_and_disconnect() {
    let mut a = on(Level::ReadWrite);
    a.groups.insert(Group::Delete);
    let f = fixture(&a);
    let server = start(&f, Arc::new(SystemClock::default()));
    let d = mk_control::read_discovery(&f.config).unwrap();
    let hello = Hello {
        client: "again".into(),
        book_path: None,
    };
    let (stop, h) = answerer(server.clone(), true);
    let (c1, _) = ControlClient::connect(&d, &hello, Box::new(|| {})).unwrap();
    let (c2, _) = ControlClient::connect(&d, &hello, Box::new(|| {})).unwrap();
    server.settings_changed();
    let (c3, _) = ControlClient::connect(&d, &hello, Box::new(|| {})).unwrap();
    server.disconnect("again");
    for _ in 0..400 {
        if c1.is_closed() && c2.is_closed() && c3.is_closed() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(c3.close_reason().as_deref(), Some("disconnected"));
    let (_c4, _) = ControlClient::connect(&d, &hello, Box::new(|| {})).unwrap();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let events = h.join().unwrap();
    let asked = events
        .iter()
        .filter(|e| matches!(e, ControlEvent::ApprovalRequested { .. }))
        .count();
    assert_eq!(asked, 3, "{events:?}");
    // Each allowed session is recorded.
    let log = f
        .host
        .with(|s| s.invoke("history.list", json!({"limit": 20})).unwrap());
    let allowed = log
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["command"] == "ai.session_allowed")
        .count();
    assert_eq!(allowed, 3);
    // A refused hello reports the code.
    let off = fixture(&AiAccess::default());
    let err = ControlServer::start(
        &off.config,
        off.host.clone(),
        Arc::new(SystemClock::default()),
    );
    let srv = err.unwrap();
    let d = mk_control::read_discovery(&off.config).unwrap();
    let e = ControlClient::connect(&d, &hello, Box::new(|| {})).unwrap_err();
    assert!(
        matches!(e, ConnectError::Refused { ref code, .. } if code == "access_off"),
        "{e:?}"
    );
    drop(srv);
}
