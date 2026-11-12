//! mcp-server.md revision 3 acceptance tests for `keep-mcp` (AT16–AT22, AT24, AT26,
//! AT27's keep-mcp part), in process through the route chooser and as the real
//! program over stdio.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use keep_mcp::{Router, Target};
use mk_commands::ai_access::{self, AiAccess, Group, Level};
use mk_commands::{KdfParams, Registry, Session};
use mk_control::server::{CloseReason, ControlEvent, ControlServer, SessionHost, SystemClock};
use mk_core::CommodityCode;
use mk_mcp::Server;
use mk_store::Store;
use serde_json::{Value, json};

const MODERN: &str = "2026-07-28";

struct Book {
    dir: tempfile::TempDir,
    path: PathBuf,
    control: PathBuf,
}

fn plain(access: Option<&AiAccess>) -> Book {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.moneykeep");
    let control = dir.path().join("config");
    let mut store = Store::create(&path, "T", &CommodityCode::new("USD").unwrap()).unwrap();
    if let Some(a) = access {
        ai_access::set(&mut store, "cli", a).unwrap();
    }
    Book { dir, path, control }
}

fn access(level: Level, groups: &[Group], app_only: bool) -> AiAccess {
    AiAccess {
        enabled: true,
        level,
        groups: groups.iter().copied().collect(),
        app_only,
    }
}

fn router(b: &Book) -> Server {
    let r = Router::new(
        Target::Book(b.path.clone()),
        Some(b.control.clone()),
        Box::new(Registry::with_builtins),
    );
    Server::new(Box::new(r))
}

fn meta(client: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": client, "version": "1"},
    })
}

fn modern(s: &mut Server, id: i64, method: &str, mut params: Value) -> Vec<Value> {
    params["_meta"] = meta("agent");
    s.handle_line(
        &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string(),
    )
}

fn call(s: &mut Server, id: i64, tool: &str, args: Value) -> Vec<Value> {
    modern(
        s,
        id,
        "tools/call",
        json!({"name": tool, "arguments": args}),
    )
}

fn names(s: &mut Server) -> Vec<String> {
    let out = modern(s, 99, "tools/list", json!({}));
    out[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

fn protocol_error(msg: &Value) -> (i64, String) {
    (
        msg["error"]["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("not an error: {msg}")),
        msg["error"]["message"].as_str().unwrap().to_owned(),
    )
}

fn set(b: &Book, a: &AiAccess) {
    let mut store = Store::open(&b.path).unwrap();
    ai_access::set(&mut store, "cli", a).unwrap();
}

#[test]
fn at16_off_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let s = Session::create(&dir.path().join("n.moneykeep"), "N", "USD", "cli").unwrap();
    let a = s.ai_access().unwrap();
    assert_eq!(a, AiAccess::default());
    assert!(!a.enabled && a.level == Level::ReadOnly && a.groups.is_empty() && a.app_only);
    let b = plain(None);
    let mut srv = router(&b);
    assert!(names(&mut srv).is_empty());
    let (code, message) = protocol_error(&call(&mut srv, 1, "account_list", json!({}))[0]);
    assert_eq!(code, -32602);
    assert!(message.contains("AI access is turned off"), "{message}");
    // The instructions say so too.
    let d = modern(&mut srv, 2, "server/discover", json!({}));
    assert!(
        d[0]["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("turned off")
    );
}

#[test]
fn at17_read_only() {
    let b = plain(Some(&access(Level::ReadOnly, &[], false)));
    let mut srv = router(&b);
    let out = modern(&mut srv, 1, "tools/list", json!({}));
    let tools = out[0]["result"]["tools"].as_array().unwrap();
    assert!(!tools.is_empty());
    assert!(
        tools
            .iter()
            .all(|t| t["annotations"]["readOnlyHint"] == true)
    );
    let (code, message) = protocol_error(&call(&mut srv, 2, "txn_create", json!({}))[0]);
    assert_eq!(code, -32602);
    assert!(
        message.contains("txn_create") && message.contains("read only"),
        "{message}"
    );
    let d = modern(&mut srv, 3, "server/discover", json!({}));
    assert!(
        d[0]["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("Access: read only.")
    );
}

#[test]
fn at18_groups() {
    let b = plain(Some(&access(Level::ReadWrite, &[], false)));
    {
        let mut store = Store::open(&b.path).unwrap();
        let r = Registry::with_builtins();
        let actor = mk_commands::Actor("cli".into());
        r.invoke(
            &mut store,
            &actor,
            "module.enable",
            json!({"module": "bank-feeds"}),
        )
        .unwrap();
    }
    let mut srv = router(&b);
    let base = names(&mut srv);
    for absent in [
        "txn_delete",
        "module_enable",
        "import_stage",
        "connect_sync",
    ] {
        assert!(!base.contains(&absent.to_owned()), "{absent}");
    }
    call(
        &mut srv,
        1,
        "account_create",
        json!({"name": "Checking", "type": "asset"}),
    );
    let out = call(
        &mut srv,
        2,
        "txn_create",
        json!({"date": "2026-07-01", "splits": [
            {"account": "Checking", "value": "10.00"},
            {"account": "Opening Balances", "value": "-10.00"}]}),
    );
    assert_eq!(out[0]["result"]["isError"], false, "{}", out[0]);
    let (_, m) = protocol_error(&call(&mut srv, 3, "txn_delete", json!({}))[0]);
    assert!(
        m.contains("not 'Delete and void'") && m.contains("Settings → Security"),
        "{m}"
    );
    // Each group in turn adds exactly that group's commands.
    let registry = Registry::with_builtins();
    let store = Store::open(&b.path).unwrap();
    let available = registry.available_specs(&store).unwrap();
    for (g, example) in [
        (Group::Delete, "txn_delete"),
        (Group::Settings, "module_enable"),
        (Group::Files, "import_stage"),
        (Group::Connections, "connect_sync"),
    ] {
        set(&b, &access(Level::ReadWrite, &[g], false));
        let now = names(&mut srv);
        let mut added: Vec<String> = now.iter().filter(|n| !base.contains(n)).cloned().collect();
        added.sort();
        let mut expected: Vec<String> = available
            .iter()
            .filter(|s| {
                let gs = ai_access::groups_of(s);
                !gs.is_empty()
                    && gs.iter().all(|x| *x == g)
                    && !mk_commands::NOT_OVER_MCP.contains(&s.name)
            })
            .map(|s| mk_mcp::tool_name(s.name))
            .collect();
        expected.sort();
        assert_eq!(added, expected, "{g:?}");
        assert!(added.contains(&example.to_owned()), "{g:?}: {added:?}");
        assert!(base.iter().all(|n| now.contains(n)));
    }
}

#[test]
fn at19_live_settings_change() {
    let b = plain(Some(&access(Level::ReadOnly, &[], false)));
    let mut srv = router(&b);
    let out = modern(
        &mut srv,
        7,
        "subscriptions/listen",
        json!({"notifications": {"toolsListChanged": true}}),
    );
    assert_eq!(out.len(), 1);
    // Another Store handle on the same plain book changes the level.
    set(&b, &access(Level::ReadWrite, &[], false));
    let out = modern(&mut srv, 8, "tools/list", json!({}));
    assert_eq!(out.len(), 2, "{out:?}");
    assert_eq!(out[0]["id"], 8);
    assert_eq!(out[1]["method"], "notifications/tools/list_changed");
    assert_eq!(
        out[1]["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        7
    );
    assert_eq!(modern(&mut srv, 9, "tools/list", json!({})).len(), 1);
}

#[test]
fn at20_settings_cannot_be_changed_by_commands() {
    let b = plain(Some(&access(Level::ReadWrite, &Group::ALL, false)));
    // Actor cli.
    let mut store = Store::open(&b.path).unwrap();
    let r = Registry::with_builtins();
    let cli = mk_commands::Actor("cli".into());
    for (cmd, args) in [
        (
            "setting.set",
            json!({"module": "core", "key": "ai_access.enabled", "value": false}),
        ),
        (
            "setting.set",
            json!({"module": "core", "key": "ai_access.level", "value": "read_write"}),
        ),
        (
            "module.set_setting",
            json!({"key": "ai_access.enabled", "value": true}),
        ),
    ] {
        let e = r.invoke(&mut store, &cli, cmd, args).unwrap_err();
        assert_eq!(e.code(), "invalid_args");
        assert!(e.to_string().contains("Settings → Security"), "{e}");
    }
    // setting.get may read them.
    let v = r
        .invoke(
            &mut store,
            &cli,
            "setting.get",
            json!({"module": "core", "key": "ai_access.enabled"}),
        )
        .unwrap();
    assert_eq!(v["value"], true);
    drop(store);
    // Over MCP.
    let mut srv = router(&b);
    let out = call(
        &mut srv,
        1,
        "setting_set",
        json!({"module": "core", "key": "ai_access.enabled", "value": false}),
    );
    assert_eq!(out[0]["result"]["isError"], true, "{}", out[0]);
    assert_eq!(
        out[0]["result"]["structuredContent"]["error"]["code"],
        "invalid_args"
    );
    // set_ai_access records book.ai_access_changed, which history.undo skips.
    let mut s = Session::open(&b.path, "cli").unwrap();
    s.invoke("account.create", json!({"name": "Cash", "type": "asset"}))
        .unwrap();
    s.set_ai_access(&access(Level::ReadOnly, &[], false))
        .unwrap();
    let log = s.invoke("history.list", json!({"limit": 1})).unwrap();
    assert_eq!(log[0]["command"], "book.ai_access_changed");
    assert_eq!(log[0]["args"]["old"]["level"], "read_write");
    assert_eq!(log[0]["args"]["new"]["level"], "read_only");
    let undone = s.invoke("history.undo", json!({})).unwrap();
    assert!(undone.to_string().contains("account.create"), "{undone}");
    assert_eq!(s.ai_access().unwrap().level, Level::ReadOnly);
    // Invalid stored values read as the defaults.
    let mut store = Store::open(&b.path).unwrap();
    store
        .write(|tx| tx.set_setting("core", "ai_access.enabled", &json!("yes"), "x"))
        .unwrap();
    assert!(!ai_access::read(&store).unwrap().enabled);
}

fn keep_mcp() -> Command {
    Command::new(env!("CARGO_BIN_EXE_keep-mcp"))
}

/// Run keep-mcp over stdio with `lines`, returning its output messages.
fn run_stdio(args: &[&str], control: &Path, envs: &[(&str, &str)], lines: &[Value]) -> Vec<Value> {
    let mut cmd = keep_mcp();
    cmd.args(args)
        .env("KEEP_MCP_CONTROL_DIR", control)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        for l in lines {
            writeln!(stdin, "{l}").unwrap();
        }
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn req(id: i64, method: &str, mut params: Value) -> Value {
    params["_meta"] = meta("stdio-test");
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

#[test]
fn at21_encrypted_book_without_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("locked.moneykeep");
    let (mut s, _) = Session::create_encrypted(
        &path,
        "Locked",
        "USD",
        "cli",
        "correct horse battery",
        true,
        KdfParams::TEST,
    )
    .unwrap();
    s.set_ai_access(&access(Level::ReadWrite, &Group::ALL, true))
        .unwrap();
    // A password-protected book cannot be set to direct use.
    assert_eq!(
        s.set_ai_access(&access(Level::ReadWrite, &[], false))
            .unwrap_err()
            .code(),
        "invalid_args"
    );
    drop(s);
    let book = path.to_str().unwrap();
    let out = run_stdio(
        &[book],
        &dir.path().join("config"),
        &[("MONEYKEEP_PASSWORD", "correct horse battery")],
        &[
            req(1, "tools/list", json!({})),
            req(
                2,
                "tools/call",
                json!({"name": "account_list", "arguments": {}}),
            ),
        ],
    );
    assert_eq!(out[0]["result"]["tools"], json!([]));
    let (code, m) = protocol_error(&out[1]);
    assert_eq!(code, -32602);
    assert_eq!(m, ai_access::MSG_PASSWORD);
}

/// A test control-channel server (the real app-side handler, no window) for `book`,
/// answering approvals with `allow`.
struct App {
    server: Arc<ControlServer>,
    host: Arc<SessionHost>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    answerer: Option<std::thread::JoinHandle<Vec<ControlEvent>>>,
}

impl App {
    fn start(book: &Path, control: &Path, allow: bool) -> App {
        let host = Arc::new(SessionHost::new(Session::open(book, "app").unwrap()));
        let server = Arc::new(
            ControlServer::start(control, host.clone(), Arc::new(SystemClock::default())).unwrap(),
        );
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (s, st) = (server.clone(), stop.clone());
        let answerer = std::thread::spawn(move || {
            let mut seen = Vec::new();
            while !st.load(std::sync::atomic::Ordering::SeqCst) {
                for e in s.poll_events() {
                    if let ControlEvent::ApprovalRequested { id, .. } = &e {
                        s.answer(*id, allow);
                    }
                    seen.push(e);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            seen
        });
        App {
            server,
            host,
            stop,
            answerer: Some(answerer),
        }
    }

    fn events(&mut self) -> Vec<ControlEvent> {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        self.answerer
            .take()
            .map(|h| h.join().unwrap())
            .unwrap_or_default()
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn at22_app_only() {
    let b = plain(Some(&access(Level::ReadWrite, &[], true)));
    let mut srv = router(&b);
    assert!(names(&mut srv).is_empty());
    let (_, m) = protocol_error(&call(&mut srv, 1, "account_list", json!({}))[0]);
    assert_eq!(m, ai_access::MSG_APP_ONLY);
    // The app opens the book and allows the assistant.
    let mut app = App::start(&b.path, &b.control, true);
    let n = names(&mut srv);
    assert!(n.contains(&"txn_create".to_owned()), "{n:?}");
    let out = call(
        &mut srv,
        2,
        "account_create",
        json!({"name": "Cash", "type": "asset"}),
    );
    assert_eq!(out[0]["result"]["isError"], false, "{}", out[0]);
    let log = app
        .host
        .with(|s| s.invoke("history.list", json!({"limit": 3})).unwrap());
    let entry = log
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["command"] == "account.create")
        .unwrap();
    assert_eq!(entry["actor"], "mcp:agent");
    assert!(log.to_string().contains("ai.session_allowed"));
    let events = app.events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ControlEvent::BookChanged))
    );
    assert!(events.iter().any(
        |e| matches!(e, ControlEvent::Connections(c) if c.iter().any(|c| c.client == "agent"))
    ));
}

#[test]
fn at24_keep_mcp_lists_no_tools_after_lock() {
    let b = plain(Some(&access(Level::ReadOnly, &[], true)));
    let app = App::start(&b.path, &b.control, true);
    let mut srv = router(&b);
    let out = modern(
        &mut srv,
        7,
        "subscriptions/listen",
        json!({"notifications": {"toolsListChanged": true}}),
    );
    assert_eq!(out.len(), 1);
    let listed = names(&mut srv);
    let exposed: Vec<String> = app.host.with(|s| {
        let mut v: Vec<String> = s
            .ai_exposed()
            .unwrap()
            .iter()
            .map(|c| mk_mcp::tool_name(c.name))
            .collect();
        v.sort();
        v
    });
    assert_eq!(listed, exposed);
    // A call outside the exposed set (read only) is refused.
    let (code, m) = protocol_error(&call(&mut srv, 2, "txn_create", json!({}))[0]);
    assert_eq!(code, -32602);
    assert!(m.contains("read only"), "{m}");
    app.server.stop(CloseReason::Locked);
    assert!(!mk_control::discovery_path(&b.control).exists());
    std::thread::sleep(Duration::from_millis(50));
    let out = modern(&mut srv, 3, "tools/list", json!({}));
    assert_eq!(out[0]["result"]["tools"], json!([]));
    assert_eq!(out[1]["method"], "notifications/tools/list_changed");
    let (_, m) = protocol_error(&call(&mut srv, 4, "account_list", json!({}))[0]);
    assert_eq!(m, ai_access::MSG_APP_ONLY);
}

/// The app's tools_changed event reaches a connected keep-mcp between requests.
#[test]
fn app_tools_changed_wakes_keep_mcp() {
    let b = plain(Some(&access(Level::ReadOnly, &[], true)));
    let app = App::start(&b.path, &b.control, true);
    let router = Router::new(
        Target::Book(b.path.clone()),
        Some(b.control.clone()),
        Box::new(Registry::with_builtins),
    );
    let mut srv = Server::new(Box::new(router));
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let reader = ChannelReader {
        rx,
        buf: Vec::new(),
    };
    let (otx, orx) = std::sync::mpsc::channel::<Vec<u8>>();
    let writer = ChannelWriter(otx);
    // The server runs here; another thread plays the client and the owner.
    let driver = std::thread::spawn(move || {
        tx.send(format!(
            "{}\n",
            req(
                7,
                "subscriptions/listen",
                json!({"notifications": {"toolsListChanged": true}})
            )
        ))
        .unwrap();
        let ack = recv_json(&orx);
        assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
        // The owner changes the level in the app.
        app.host.with(|s| {
            s.set_ai_access(&access(Level::ReadWrite, &[], true))
                .unwrap()
        });
        app.server.settings_changed();
        let n = recv_json(&orx);
        assert_eq!(n["method"], "notifications/tools/list_changed");
        assert_eq!(
            n["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
            7
        );
        drop(tx);
        drop(app);
    });
    srv.serve(std::io::BufReader::new(reader), writer).unwrap();
    driver.join().unwrap();
}

struct ChannelReader {
    rx: std::sync::mpsc::Receiver<String>,
    buf: Vec<u8>,
}

impl std::io::Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.buf.is_empty() {
            match self.rx.recv() {
                Ok(s) => self.buf = s.into_bytes(),
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.buf.len());
        out[..n].copy_from_slice(&self.buf[..n]);
        self.buf.drain(..n);
        Ok(n)
    }
}

struct ChannelWriter(std::sync::mpsc::Sender<Vec<u8>>);

impl Write for ChannelWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let _ = self.0.send(b.to_vec());
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn recv_json(rx: &std::sync::mpsc::Receiver<Vec<u8>>) -> Value {
    let mut acc = Vec::new();
    loop {
        let chunk = rx.recv_timeout(Duration::from_secs(20)).unwrap();
        acc.extend(chunk);
        if acc.ends_with(b"\n") {
            let text = String::from_utf8(acc).unwrap();
            return serde_json::from_str(text.trim()).unwrap();
        }
    }
}

#[test]
fn at26_no_secrets() {
    const PASSWORD: &str = "correct horse battery staple";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret.moneykeep");
    let control = dir.path().join("config");
    let (mut s, key) = Session::create_encrypted(
        &path,
        "Secret",
        "USD",
        "app",
        PASSWORD,
        true,
        KdfParams::TEST,
    )
    .unwrap();
    let key = key.unwrap();
    s.set_ai_access(&access(Level::ReadWrite, &Group::ALL, true))
        .unwrap();
    s.invoke("module.enable", json!({"module": "expenses"}))
        .unwrap();
    s.invoke("module.enable", json!({"module": "bank-feeds"}))
        .unwrap();
    // The exposed set never holds a NOT_OVER_MCP command or a session operation.
    let exposed = s.ai_exposed().unwrap();
    for c in &exposed {
        assert!(!mk_commands::NOT_OVER_MCP.contains(&c.name), "{}", c.name);
        for word in [
            "encrypt",
            "decrypt",
            "password",
            "recovery",
            "passwd",
            "ai_access",
        ] {
            assert!(
                !c.name.contains(word),
                "{} looks like a session operation",
                c.name
            );
        }
    }
    let host = Arc::new(SessionHost::new(s));
    let server = Arc::new(
        ControlServer::start(&control, host.clone(), Arc::new(SystemClock::default())).unwrap(),
    );
    let token = mk_control::read_discovery(&control).unwrap().token;
    let (st, srv2) = (
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        server.clone(),
    );
    let st2 = st.clone();
    let answerer = std::thread::spawn(move || {
        while !st2.load(std::sync::atomic::Ordering::SeqCst) {
            for e in srv2.poll_events() {
                if let ControlEvent::ApprovalRequested { id, .. } = e {
                    srv2.answer(id, true);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    // Every MCP response of a session through keep-mcp's app route.
    let r = Router::new(
        Target::Book(path.clone()),
        Some(control.clone()),
        Box::new(Registry::with_builtins),
    );
    let mut mcp = Server::new(Box::new(r));
    let mut all = String::new();
    for (i, (m, p)) in [
        ("server/discover", json!({})),
        ("tools/list", json!({})),
        ("tools/call", json!({"name": "book_info", "arguments": {}})),
        ("tools/call", json!({"name": "connect_list", "arguments": {}})),
        ("tools/call", json!({"name": "connect_add", "arguments": {"setup_input": "x"}})),
        ("tools/call", json!({"name": "receipt_get", "arguments": {}})),
        ("tools/call", json!({"name": "capture_set_server", "arguments": {}})),
        ("tools/call", json!({"name": "history_list", "arguments": {"limit": 50}})),
        ("tools/call", json!({"name": "setting_get", "arguments": {"module": "core", "key": "ai_access.enabled"}})),
    ]
    .into_iter()
    .enumerate()
    {
        for msg in modern(&mut mcp, i64::try_from(i).unwrap(), m, p) {
            all.push_str(&msg.to_string());
        }
    }
    assert!(all.contains("book_info"), "{all}");
    for nope in mk_commands::NOT_OVER_MCP {
        assert!(
            !all.contains(&format!("\"name\":\"{}\"", mk_mcp::tool_name(nope))),
            "{nope}"
        );
    }
    // Raw control-channel responses too.
    let d = mk_control::read_discovery(&control).unwrap();
    let mut raw = std::net::TcpStream::connect(("127.0.0.1", d.port)).unwrap();
    writeln!(
        raw,
        "{}",
        json!({"hello": "moneykeep-control/1", "token": token, "client": {"name": "agent", "kind": "keep-mcp"}, "book_path": null})
    )
    .unwrap();
    writeln!(raw, "{}", json!({"id": 1, "op": "list"})).unwrap();
    writeln!(
        raw,
        "{}",
        json!({"id": 2, "op": "call", "command": "connect.add", "args": {}})
    )
    .unwrap();
    writeln!(
        raw,
        "{}",
        json!({"id": 3, "op": "call", "command": "book.info", "args": {}})
    )
    .unwrap();
    let mut reader = std::io::BufReader::new(raw.try_clone().unwrap());
    for _ in 0..4 {
        let mut l = String::new();
        std::io::BufRead::read_line(&mut reader, &mut l).unwrap();
        all.push_str(&l);
    }
    assert!(all.contains("not_available"), "{all}");
    for secret in [PASSWORD, key.as_str(), token.as_str()] {
        assert!(!all.contains(secret), "a secret appeared in a response");
    }
    st.store(true, std::sync::atomic::Ordering::SeqCst);
    answerer.join().unwrap();
    drop(server);
    drop(dir);
}

#[test]
fn at27_config_prints_an_absolute_command() {
    let b = plain(None);
    let out = keep_mcp()
        .args([
            "config",
            "--client",
            "claude-desktop",
            b.path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let server = &v["mcpServers"]["moneykeep"];
    let command = Path::new(server["command"].as_str().unwrap());
    assert!(command.is_absolute(), "{command:?}");
    assert!(
        command
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("keep-mcp")
    );
    assert_eq!(server["args"][0], mk_control::canonical(&b.path));
    // It wrote nothing next to the book.
    let files: Vec<_> = std::fs::read_dir(b.dir.path()).unwrap().collect();
    assert_eq!(files.len(), 1);
    let app = keep_mcp().args(["config", "--app"]).output().unwrap();
    let v: Value = serde_json::from_slice(&app.stdout).unwrap();
    assert_eq!(v["args"], json!(["--app"]));
    // No read-only flag and no password option any more.
    for gone in [
        &["--read-only", "x"][..],
        &["--password-file", "p", "x"][..],
    ] {
        assert!(!keep_mcp().args(gone).output().unwrap().status.success());
    }
}

#[test]
fn keep_mcp_speaks_json_rpc_over_stdio() {
    let b = plain(Some(&access(Level::ReadOnly, &[], false)));
    let out = run_stdio(
        &[b.path.to_str().unwrap()],
        &b.control,
        &[],
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"cli-test"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        ],
    );
    assert_eq!(out.len(), 2);
    let tools = out[1]["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"] == "account_list"));
    assert!(!tools.iter().any(|t| t["name"] == "txn_create"));
}

#[test]
fn keep_mcp_modern_request_and_eof_closure() {
    let b = plain(Some(&access(Level::ReadWrite, &[], false)));
    let out = run_stdio(
        &[b.path.to_str().unwrap()],
        &b.control,
        &[],
        &[
            req(1, "server/discover", json!({})),
            req(
                2,
                "subscriptions/listen",
                json!({"notifications": {"toolsListChanged": true}}),
            ),
        ],
    );
    assert_eq!(out.len(), 3);
    assert_eq!(out[0]["result"]["resultType"], "complete");
    assert_eq!(out[1]["method"], "notifications/subscriptions/acknowledged");
    assert_eq!(out[2]["id"], 2);
    assert_eq!(out[2]["result"]["resultType"], "complete");
}

/// app-desktop.md revision 20, acceptance test 124: keep-mcp keeps ISO dates.
#[test]
fn app_desktop_at124_keep_mcp_keeps_iso_dates() {
    let b = plain(Some(&access(Level::ReadOnly, &[], false)));
    {
        let mut s = Session::open(&b.path, "cli").unwrap();
        s.invoke(
            "account.create",
            json!({"name":"Checking","type":"asset","subtype":"bank"}),
        )
        .unwrap();
        s.invoke("txn.create", json!({"date":"2026-10-22","splits":[{"account":"Checking","value":"5.00"},{"account":"Opening Balances","value":"-5.00"}]}))
            .unwrap();
    }
    let out = run_stdio(
        &[b.path.to_str().unwrap()],
        &b.control,
        &[],
        &[req(
            1,
            "tools/call",
            json!({"name": "txn_list", "arguments": {}}),
        )],
    );
    let last = out.last().unwrap().to_string();
    assert!(last.contains("2026-10-22"), "{last}");
    assert!(!last.contains("10-22-2026"), "{last}");
}
