//! The app side of the control channel: listens on `127.0.0.1` while a book with AI
//! access is open, checks each client's hello, asks the owner (through
//! [`ControlEvent::ApprovalRequested`], answered with [`ControlServer::answer`]) and
//! serves `list` and `call` through a [`Host`] that runs the calls on the app's
//! worker. It has no window: the app (or a test) polls [`ControlServer::poll_events`].

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mk_commands::CommandSpec;
use serde_json::{Value, json};

use crate::{
    APPROVAL_TIMEOUT, AiAccess, Discovery, MAX_CONNECTIONS, MAX_LINE, PROTOCOL, discovery_path,
    live_discovery, new_token, token_eq,
};

/// Connections still in their hello at once; more are closed at once.
const MAX_PENDING_SOCKETS: usize = 16;
/// How long a new connection may take to send its hello.
const HELLO_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// The open book as the channel sees it.
#[derive(Debug, Clone)]
pub struct HostState {
    pub book_name: String,
    /// Canonical path of the book file.
    pub book_path: String,
    pub access: AiAccess,
    /// The exposed set (the shared `mk_commands::ai_access::exposed`).
    pub exposed: Vec<CommandSpec>,
}

/// The result of a call.
#[derive(Debug, Clone, PartialEq)]
pub enum CallReply {
    /// The command ran; `changed` when it changed the book.
    Ok { value: Value, changed: bool },
    /// The command ran and failed (the CLI's error code).
    Failed { code: String, message: String },
    /// Outside the exposed set.
    NotAvailable(String),
}

/// What the channel needs from the app: the open book, and calls run on its worker.
pub trait Host: Send + Sync {
    /// The open book, or `None` when no book is open.
    fn state(&self) -> Option<HostState>;
    /// Run `command` as `actor` (`mcp:<client>`) if it is in the exposed set.
    fn call(&self, actor: &str, command: &str, args: Value) -> CallReply;
    /// The owner allowed `client`: record `ai.session_allowed`.
    fn session_allowed(&self, client: &str);
}

/// A clock for the approval timeout (tests inject a manual one).
pub trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}

/// The real clock.
#[derive(Debug)]
pub struct SystemClock(Instant);

impl Default for SystemClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

/// A clock that moves only when told (tests).
#[derive(Debug, Clone, Default)]
pub struct ManualClock(Arc<Mutex<Duration>>);

impl ManualClock {
    pub fn advance(&self, by: Duration) {
        let mut t = lock(&self.0);
        *t = t.saturating_add(by);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Duration {
        *lock(&self.0)
    }
}

/// Why the app closes every connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    BookClosed,
    Locked,
    Disconnected,
    AccessOff,
    AppExit,
}

impl CloseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            CloseReason::BookClosed => "book_closed",
            CloseReason::Locked => "locked",
            CloseReason::Disconnected => "disconnected",
            CloseReason::AccessOff => "access_off",
            CloseReason::AppExit => "app_exit",
        }
    }
}

/// A connected (allowed) assistant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connected {
    pub client: String,
    pub access: AiAccess,
}

/// Something the app shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlEvent {
    /// Show the approval dialog; answer with [`ControlServer::answer`].
    ApprovalRequested {
        id: u64,
        client: String,
        book_name: String,
        access: AiAccess,
    },
    /// The approval is no longer waited for (timed out, or the client left).
    ApprovalEnded { id: u64 },
    /// The assistants connected now.
    Connections(Vec<Connected>),
    /// A call changed the book: refresh the screens.
    BookChanged,
}

/// Why listening did not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// A live `control.json` of another app window exists.
    OtherWindow,
    /// No book is open.
    NoBook,
    Io(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::OtherWindow => {
                f.write_str("Another MoneyKeep window is serving AI assistants.")
            }
            StartError::NoBook => f.write_str("No book is open."),
            StartError::Io(e) => write!(f, "The AI assistant channel could not start: {e}"),
        }
    }
}

type Writer = Arc<Mutex<TcpStream>>;

#[derive(Debug)]
struct Conn {
    client: String,
    writer: Writer,
    /// Allowed (after `ok`); pending connections count towards the limit too.
    ok: bool,
    access: AiAccess,
}

#[derive(Debug, Default)]
struct State {
    next_id: u64,
    conns: BTreeMap<u64, Conn>,
    allowed: BTreeSet<String>,
    answers: BTreeMap<u64, bool>,
    pending_sockets: usize,
    /// Exposed command names last announced.
    names: Vec<String>,
    events: Vec<ControlEvent>,
}

struct Shared {
    host: Arc<dyn Host>,
    clock: Arc<dyn Clock>,
    token: String,
    stopping: AtomicBool,
    state: Mutex<State>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn send(w: &Writer, v: &Value) -> bool {
    let mut s = lock(w);
    writeln!(s, "{v}").and_then(|()| s.flush()).is_ok()
}

fn close(w: &Writer) {
    let _ = lock(w).shutdown(Shutdown::Both);
}

/// The listening control channel. Dropping it stops listening (as `app_exit`).
pub struct ControlServer {
    shared: Arc<Shared>,
    listener: Mutex<Option<JoinHandle<()>>>,
    file: PathBuf,
    port: u16,
    stopped: AtomicBool,
}

impl std::fmt::Debug for ControlServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlServer")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

/// Write `control.json` atomically, readable only by the user on Unix.
fn write_discovery(file: &Path, d: &Discovery) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    let text = serde_json::to_string(d).map_err(std::io::Error::other)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, file)
}

impl ControlServer {
    /// Start listening for the host's open book; writes `control.json` in `dir`. A
    /// live file of another app window stops it; a stale one is replaced.
    pub fn start(
        dir: &Path,
        host: Arc<dyn Host>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StartError> {
        if live_discovery(dir).is_some() {
            return Err(StartError::OtherWindow);
        }
        let state = host.state().ok_or(StartError::NoBook)?;
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).map_err(|e| StartError::Io(e.to_string()))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| StartError::Io(e.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|e| StartError::Io(e.to_string()))?
            .port();
        let token = new_token().map_err(StartError::Io)?;
        let file = discovery_path(dir);
        write_discovery(
            &file,
            &Discovery {
                version: 1,
                port,
                token: token.clone(),
                pid: std::process::id(),
                book_path: state.book_path.clone(),
            },
        )
        .map_err(|e| StartError::Io(e.to_string()))?;
        let shared = Arc::new(Shared {
            host,
            clock,
            token,
            stopping: AtomicBool::new(false),
            state: Mutex::new(State {
                names: state.exposed.iter().map(|s| s.name.to_owned()).collect(),
                ..State::default()
            }),
        });
        let s = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("moneykeep-control".into())
            .spawn(move || accept_loop(&s, &listener))
            .map_err(|e| StartError::Io(e.to_string()))?;
        Ok(Self {
            shared,
            listener: Mutex::new(Some(handle)),
            file,
            port,
            stopped: AtomicBool::new(false),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The discovery file this server wrote.
    pub fn discovery_file(&self) -> &Path {
        &self.file
    }

    /// Whether the server stopped.
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Events since the last call.
    pub fn poll_events(&self) -> Vec<ControlEvent> {
        std::mem::take(&mut lock(&self.shared.state).events)
    }

    /// The owner answered approval `id`.
    pub fn answer(&self, id: u64, allow: bool) {
        lock(&self.shared.state).answers.insert(id, allow);
    }

    /// The owner pressed Disconnect for `client`: close its connections and forget
    /// its approval.
    pub fn disconnect(&self, client: &str) {
        let mut st = lock(&self.shared.state);
        st.allowed.remove(client);
        let ids: Vec<u64> = st
            .conns
            .iter()
            .filter(|(_, c)| c.client == client)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(c) = st.conns.remove(&id) {
                send(
                    &c.writer,
                    &json!({"event": "closed", "reason": CloseReason::Disconnected.as_str()}),
                );
                close(&c.writer);
            }
        }
        push_connections(&mut st);
    }

    /// The AI access settings changed (and access stays on): approvals are
    /// forgotten and connected clients are told the tools changed.
    pub fn settings_changed(&self) {
        lock(&self.shared.state).allowed.clear();
        announce(&self.shared, true);
    }

    /// The book changed in the app (a module turned on, say): connected clients are
    /// told when the exposed command names differ from those last announced.
    pub fn book_changed(&self) {
        announce(&self.shared, false);
    }

    /// Stop listening: every connection gets `closed` with `reason` and is closed,
    /// and `control.json` is deleted.
    pub fn stop(&self, reason: CloseReason) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shared.stopping.store(true, Ordering::SeqCst);
        {
            let mut st = lock(&self.shared.state);
            for (_, c) in std::mem::take(&mut st.conns) {
                send(
                    &c.writer,
                    &json!({"event": "closed", "reason": reason.as_str()}),
                );
                close(&c.writer);
            }
            st.allowed.clear();
            st.events.push(ControlEvent::Connections(Vec::new()));
        }
        if let Some(h) = lock(&self.listener).take() {
            let _ = h.join();
        }
        // Only our own file (another window may have replaced it since).
        if crate::read_discovery(self.file.parent().unwrap_or(Path::new(".")))
            .is_some_and(|d| d.token == self.shared.token)
        {
            let _ = std::fs::remove_file(&self.file);
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.stop(CloseReason::AppExit);
    }
}

fn push_connections(st: &mut State) {
    let list = st
        .conns
        .values()
        .filter(|c| c.ok)
        .map(|c| Connected {
            client: c.client.clone(),
            access: c.access.clone(),
        })
        .collect();
    st.events.push(ControlEvent::Connections(list));
}

/// Send `tools_changed` to every allowed connection when the exposed names changed
/// (or always, with `force`).
fn announce(shared: &Shared, force: bool) {
    let names: Vec<String> = shared
        .host
        .state()
        .map(|s| s.exposed.iter().map(|c| c.name.to_owned()).collect())
        .unwrap_or_default();
    let mut st = lock(&shared.state);
    if !force && names == st.names {
        return;
    }
    st.names = names;
    for c in st.conns.values().filter(|c| c.ok) {
        send(&c.writer, &json!({"event": "tools_changed"}));
    }
}

fn accept_loop(shared: &Arc<Shared>, listener: &TcpListener) {
    while !shared.stopping.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, addr)) => {
                if !addr.ip().is_loopback() {
                    continue;
                }
                {
                    let mut st = lock(&shared.state);
                    if st.pending_sockets >= MAX_PENDING_SOCKETS {
                        continue;
                    }
                    st.pending_sockets += 1;
                }
                let s = Arc::clone(shared);
                let spawned = std::thread::Builder::new()
                    .name("moneykeep-control-conn".into())
                    .spawn(move || {
                        let mut counted = true;
                        connection(&s, stream, &mut counted);
                        if counted {
                            let mut st = lock(&s.state);
                            st.pending_sockets = st.pending_sockets.saturating_sub(1);
                        }
                    });
                if spawned.is_err() {
                    let mut st = lock(&shared.state);
                    st.pending_sockets = st.pending_sockets.saturating_sub(1);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// One line of at most [`MAX_LINE`] bytes.
pub(crate) enum Line {
    Text(String),
    Eof,
    TooLong,
}

pub(crate) fn read_line(r: &mut impl BufRead) -> Line {
    let mut buf = Vec::new();
    let limit = u64::try_from(MAX_LINE)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    match r.by_ref().take(limit).read_until(b'\n', &mut buf) {
        Ok(0) | Err(_) => Line::Eof,
        Ok(_) => {
            if buf.last() == Some(&b'\n') {
                buf.pop();
                if buf.last() == Some(&b'\r') {
                    buf.pop();
                }
            } else if buf.len() > MAX_LINE {
                return Line::TooLong;
            }
            match String::from_utf8(buf) {
                Ok(s) => Line::Text(s),
                Err(_) => Line::Text(String::new()),
            }
        }
    }
}

fn hello_error(w: &Writer, code: &str, message: &str) {
    send(w, &json!({"error": {"code": code, "message": message}}));
    close(w);
}

/// Serve one connection: hello, approval, then requests until it closes.
fn connection(shared: &Arc<Shared>, stream: TcpStream, counted: &mut bool) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(HELLO_READ_TIMEOUT));
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let writer: Writer = Arc::new(Mutex::new(stream));
    let mut reader = BufReader::new(read_half);
    let hello = match read_line(&mut reader) {
        Line::Text(t) => t,
        Line::Eof | Line::TooLong => return close(&writer),
    };
    // Not a JSON object (an HTTP request line, say): close with no reply.
    let Ok(hello) = serde_json::from_str::<Value>(&hello) else {
        return close(&writer);
    };
    if !hello.is_object() || hello.get("hello").and_then(Value::as_str) != Some(PROTOCOL) {
        return close(&writer);
    }
    let token = hello.get("token").and_then(Value::as_str).unwrap_or("");
    if !token_eq(token, &shared.token) {
        return hello_error(&writer, "bad_token", "The token is not valid.");
    }
    let Some(state) = shared.host.state() else {
        return hello_error(&writer, "access_off", mk_commands::ai_access::MSG_OFF);
    };
    if let Some(p) = hello.get("book_path").and_then(Value::as_str)
        && crate::canonical(Path::new(p)) != state.book_path
    {
        return hello_error(
            &writer,
            "wrong_book",
            "MoneyKeep has a different book open.",
        );
    }
    if !state.access.enabled {
        return hello_error(&writer, "access_off", mk_commands::ai_access::MSG_OFF);
    }
    let client = hello
        .pointer("/client/name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("unknown")
        .to_owned();
    // Register the connection (pending) within the limit.
    let (conn_id, known) = {
        let mut st = lock(&shared.state);
        if st.conns.len() >= MAX_CONNECTIONS {
            drop(st);
            return hello_error(
                &writer,
                "busy",
                "Too many AI assistants are connected to MoneyKeep.",
            );
        }
        st.next_id += 1;
        let id = st.next_id;
        st.conns.insert(
            id,
            Conn {
                client: client.clone(),
                writer: Arc::clone(&writer),
                ok: false,
                access: state.access.clone(),
            },
        );
        st.pending_sockets = st.pending_sockets.saturating_sub(1);
        *counted = false;
        (id, st.allowed.contains(&client))
    };
    if !known {
        match approval(shared, conn_id, &client, &state) {
            Some(true) => {
                shared.host.session_allowed(&client);
                lock(&shared.state).allowed.insert(client.clone());
            }
            Some(false) => {
                lock(&shared.state).conns.remove(&conn_id);
                return hello_error(&writer, "denied", "The owner did not allow this assistant.");
            }
            None => {
                let gone = lock(&shared.state).conns.remove(&conn_id).is_none();
                if !gone && !shared.stopping.load(Ordering::SeqCst) {
                    return hello_error(
                        &writer,
                        "approval_timeout",
                        "Nobody answered in MoneyKeep in time.",
                    );
                }
                return close(&writer);
            }
        }
    }
    // Allowed.
    {
        let mut st = lock(&shared.state);
        let Some(c) = st.conns.get_mut(&conn_id) else {
            return close(&writer);
        };
        c.ok = true;
        push_connections(&mut st);
    }
    send(
        &writer,
        &json!({
            "ok": true,
            "book": {"name": state.book_name, "path": state.book_path},
            "level": state.access.level.as_str(),
            "groups": state.access.group_names(),
        }),
    );
    if let Ok(s) = reader.get_ref().try_clone() {
        let _ = s.set_read_timeout(None);
    }
    let actor = format!("mcp:{client}");
    while let Line::Text(line) = read_line(&mut reader) {
        if line.trim().is_empty() {
            continue;
        }
        if !lock(&shared.state).conns.contains_key(&conn_id) {
            break;
        }
        let reply = request(shared, &actor, &line);
        if !send(&writer, &reply) {
            break;
        }
    }
    close(&writer);
    let mut st = lock(&shared.state);
    if st.conns.remove(&conn_id).is_some() {
        push_connections(&mut st);
    }
}

/// Ask the owner; `None` when nobody answered in time or the channel stopped.
fn approval(shared: &Shared, conn_id: u64, client: &str, state: &HostState) -> Option<bool> {
    let id = {
        let mut st = lock(&shared.state);
        st.next_id += 1;
        let id = st.next_id;
        st.events.push(ControlEvent::ApprovalRequested {
            id,
            client: client.to_owned(),
            book_name: state.book_name.clone(),
            access: state.access.clone(),
        });
        id
    };
    let start = shared.clock.now();
    loop {
        {
            let mut st = lock(&shared.state);
            if let Some(a) = st.answers.remove(&id) {
                return Some(a);
            }
            let gone = !st.conns.contains_key(&conn_id);
            if gone
                || shared.stopping.load(Ordering::SeqCst)
                || shared.clock.now().saturating_sub(start) >= APPROVAL_TIMEOUT
            {
                st.events.push(ControlEvent::ApprovalEnded { id });
                return None;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Answer one request line.
fn request(shared: &Shared, actor: &str, line: &str) -> Value {
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return json!({"id": null, "error": {"code": "invalid_args", "message": "not JSON"}});
    };
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    match msg.get("op").and_then(Value::as_str) {
        Some("list") => match shared.host.state() {
            Some(s) => json!({
                "id": id,
                "commands": s.exposed,
                "level": s.access.level.as_str(),
                "groups": s.access.group_names(),
            }),
            None => json!({"id": id, "error": {"code": "no_book", "message": "No book is open."}}),
        },
        Some("call") => {
            let Some(command) = msg.get("command").and_then(Value::as_str) else {
                return json!({"id": id, "error": {"code": "invalid_args", "message": "call needs a command"}});
            };
            let args = match msg.get("args") {
                None | Some(Value::Null) => json!({}),
                Some(a) => a.clone(),
            };
            match shared.host.call(actor, command, args) {
                CallReply::Ok { value, changed } => {
                    if changed {
                        lock(&shared.state).events.push(ControlEvent::BookChanged);
                        announce(shared, false);
                    }
                    json!({"id": id, "result": value})
                }
                CallReply::Failed { code, message } => {
                    json!({"id": id, "error": {"code": code, "message": message}})
                }
                CallReply::NotAvailable(message) => {
                    json!({"id": id, "error": {"code": "not_available", "message": message}})
                }
            }
        }
        _ => json!({"id": id, "error": {"code": "invalid_args", "message": "unknown op"}}),
    }
}

/// A host over a [`mk_commands::Session`] behind a mutex: for tests and tools that
/// drive the channel without the app's worker.
pub struct SessionHost {
    session: Mutex<mk_commands::Session>,
}

impl std::fmt::Debug for SessionHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionHost")
    }
}

impl SessionHost {
    pub fn new(session: mk_commands::Session) -> Self {
        Self {
            session: Mutex::new(session),
        }
    }

    /// Run `f` with the session (tests change settings this way).
    pub fn with<T>(&self, f: impl FnOnce(&mut mk_commands::Session) -> T) -> T {
        f(&mut lock(&self.session))
    }
}

/// [`Host::state`] for a session (shared by the app's worker and [`SessionHost`]).
pub fn session_state(s: &mut mk_commands::Session) -> Option<HostState> {
    let access = s.ai_access().ok()?;
    let exposed = s.ai_exposed().ok()?;
    let book_name = s
        .invoke("book.info", json!({}))
        .ok()
        .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    let book_path = s
        .canonical_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    Some(HostState {
        book_name,
        book_path,
        access,
        exposed,
    })
}

/// Reads files for the `files` group's commands (only reached when the owner
/// ticked "Read files on this computer").
#[derive(Debug)]
pub struct LocalFiles;

impl mk_commands::FileReader for LocalFiles {
    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        std::fs::read(path).map_err(|e| e.to_string())
    }
}

/// [`Host::call`] for a session.
pub fn session_call(
    s: &mut mk_commands::Session,
    actor: &str,
    command: &str,
    args: Value,
) -> CallReply {
    let mutates = s
        .ai_exposed()
        .ok()
        .is_some_and(|e| e.iter().any(|c| c.name == command && c.mutates));
    match s.invoke_exposed(actor, command, args, Some(&LocalFiles)) {
        Ok(value) => CallReply::Ok {
            value,
            changed: mutates,
        },
        Err(mk_commands::ai_access::ExposedError::NotAvailable(m)) => CallReply::NotAvailable(m),
        Err(mk_commands::ai_access::ExposedError::Failed(e)) => CallReply::Failed {
            code: e.code().to_owned(),
            message: e.to_string(),
        },
    }
}

impl Host for SessionHost {
    fn state(&self) -> Option<HostState> {
        session_state(&mut lock(&self.session))
    }
    fn call(&self, actor: &str, command: &str, args: Value) -> CallReply {
        session_call(&mut lock(&self.session), actor, command, args)
    }
    fn session_allowed(&self, client: &str) {
        let _ = lock(&self.session).record_ai_session_allowed(client);
    }
}
