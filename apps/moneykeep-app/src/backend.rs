//! The app's backend: one worker thread owns the open book, so only one command runs
//! against it at a time. Every change goes through `Registry::invoke` (via
//! `Session`) with actor `app`.

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use mk_commands::{BookKind, Secret, Session};
use mk_control::server::{CallReply, Host, HostState};
use mk_ui::{Backend, SecurityOp, UiError, UiSecret};
use serde_json::{Value, json};

enum Request {
    Create {
        path: String,
        name: String,
        currency: String,
        preset: Option<String>,
    },
    Open {
        path: String,
    },
    CreateProtected {
        path: String,
        name: String,
        currency: String,
        password: String,
        recovery: bool,
        preset: Option<String>,
    },
    OpenWith {
        path: String,
        secret: Secret,
    },
    OpenPrevious {
        path: String,
        secret: Secret,
    },
    Security(SecurityOp),
    Protection,
    /// One watched-folder pass on the open book (used for encrypted books, which
    /// cannot be opened a second time by a separate watcher).
    WatchPass,
    Close,
    Invoke {
        command: String,
        args: Value,
    },
    /// Start a bank-feed command in the background (its requests run on their own
    /// thread; the worker keeps serving).
    NetStart {
        command: String,
        args: Value,
    },
    /// Apply finished background answers; returns `[[id, result or error]]`.
    NetPoll,
    NetCancel(u64),
    /// The open book's AI access settings (mcp-server.md revision 3).
    AiAccess,
    /// Change them (the session operation `set_ai_access`).
    SetAiAccess(Value),
    /// Backups (backups.md): the settings; start one (copied on its own thread);
    /// the finished ones; whether the book changed since the last; wait for all;
    /// list a book's; restore one as a copy.
    BackupSettings(Value),
    BackupStart(String),
    BackupPoll,
    BackupChanged,
    BackupWait,
    BackupsList(String),
    RestoreCopy {
        backup: String,
        target: String,
        secret: Option<Secret>,
    },
    /// Back up a book file that is not open, without opening it (start screen card
    /// … → Back up this book…, backups.md revision 2).
    BackupFile {
        book: String,
        secret: Option<Secret>,
    },
    /// Run a function on the worker with the open session (the control channel's
    /// host: its calls run here, one at a time, like any GUI action).
    With(Job),
    Stop,
}

/// A function the control channel runs on the worker.
type Job = Box<dyn FnOnce(Option<&mut Session>) + Send>;

type Reply = Result<Value, UiError>;

/// A backend whose book lives on a worker thread.
pub struct WorkerBackend {
    tx: Sender<(Request, Sender<Reply>)>,
    book: Option<String>,
    worker: Option<JoinHandle<()>>,
    /// The before-update backup made when the book last opened.
    open_backup: Option<Value>,
}

impl std::fmt::Debug for WorkerBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerBackend")
            .field("book", &self.book)
            .finish()
    }
}

fn err(e: &mk_commands::CommandError) -> UiError {
    UiError::new(e.code(), e.to_string())
}

fn secret(s: &UiSecret) -> Secret {
    match s {
        UiSecret::Password(p) => Secret::Password(p.clone()),
        UiSecret::RecoveryKey(k) => Secret::RecoveryKey(k.clone()),
    }
}

fn security(
    session: &mut Session,
    op: SecurityOp,
) -> Result<Option<String>, mk_commands::CommandError> {
    match op {
        SecurityOp::Encrypt { password, recovery } => session.encrypt(&password, recovery),
        SecurityOp::ChangePassword { current, new } => session
            .change_password(&secret(&current), &new)
            .map(|()| None),
        SecurityOp::NewRecoveryKey { current } => {
            session.new_recovery_key(&secret(&current)).map(Some)
        }
        SecurityOp::RemoveRecoveryKey { current } => session
            .remove_recovery_key(&secret(&current))
            .map(|()| None),
        SecurityOp::Verify { current } => session.verify_secret(&secret(&current)).map(|()| None),
        SecurityOp::SaveUnencryptedCopy { path } => session
            .save_unencrypted_copy(std::path::Path::new(&path))
            .map(|p| Some(p.display().to_string())),
        SecurityOp::RemoveEncryption { current } => {
            session.remove_encryption(&secret(&current)).map(|()| None)
        }
    }
}

/// Keep `<path>.damaged` and put `<path>.prev` in place of the damaged file.
fn restore_previous(path: &std::path::Path) -> std::io::Result<()> {
    let mut damaged = path.as_os_str().to_owned();
    damaged.push(".damaged");
    std::fs::rename(path, &damaged)?;
    std::fs::copy(mk_commands::prev_path(path), path)?;
    Ok(())
}

/// Give a session its network access (connectivity.md) once.
fn networked(s: &mut Session) -> &mut Session {
    if s.connector_mut().is_none() {
        s.set_connector(mk_connect::connector());
    }
    s
}

/// A backup being copied on its own thread.
struct BackupJob {
    handle: JoinHandle<Result<mk_commands::backups::Made, mk_commands::CommandError>>,
    book: std::path::PathBuf,
    stamp: (i64, i64),
}

/// The worker's backups: settings, running copies and finished results.
#[derive(Default)]
struct Backups {
    env: Option<mk_commands::backups::BackupEnv>,
    jobs: Vec<BackupJob>,
    done: Vec<Value>,
}

fn error_json(e: &mk_commands::CommandError) -> Value {
    json!({"error": {"code": e.code(), "message": e.to_string()}})
}

impl Backups {
    /// Start copying the open book; the worker keeps serving meanwhile.
    fn start(&mut self, session: &mut Session, reason: &str) -> Reply {
        let env = self
            .env
            .clone()
            .ok_or_else(|| UiError::new("unsupported", "Backups are not set up."))?;
        let reason = mk_commands::backups::Reason::parse(reason)
            .ok_or_else(|| UiError::new("invalid_args", format!("unknown reason {reason}")))?;
        let book = session
            .path()
            .map(std::path::Path::to_path_buf)
            .ok_or_else(|| UiError::new("backup_failed", "this book has no file"))?;
        let (source, stamp) = session.backup_capture().map_err(|e| err(&e))?;
        let path = book.clone();
        let handle = std::thread::Builder::new()
            .name("moneykeep-backup".into())
            .spawn(move || mk_commands::backups::make(&source, &path, reason, &env))
            .map_err(|e| UiError::new("backup_failed", e.to_string()))?;
        self.jobs.push(BackupJob {
            handle,
            book,
            stamp,
        });
        Ok(Value::Null)
    }

    /// Collect finished copies (all of them when `wait`).
    fn collect(&mut self, session: Option<&mut Session>, wait: bool) {
        let mut session = session;
        let mut running = Vec::new();
        for job in std::mem::take(&mut self.jobs) {
            if !wait && !job.handle.is_finished() {
                running.push(job);
                continue;
            }
            let out = match job.handle.join() {
                Ok(Ok(made)) => {
                    if let Some(s) = session.as_deref_mut()
                        && s.path() == Some(job.book.as_path())
                    {
                        s.mark_backed_up(job.stamp);
                    }
                    json!({"ok": made.to_json()})
                }
                Ok(Err(e)) => error_json(&e),
                Err(_) => {
                    json!({"error": {"code": "backup_failed", "message": "the backup stopped unexpectedly"}})
                }
            };
            self.done.push(out);
        }
        self.jobs = running;
    }

    fn open(
        &self,
        path: &std::path::Path,
        secret: Option<&Secret>,
    ) -> Result<(Session, Value), mk_commands::CommandError> {
        match &self.env {
            Some(env) => Session::open_backed_up(path, "app", secret, env)
                .map(|(s, made)| (s, json!({"before_update": made.map(|m| m.to_json())}))),
            None => Session::open_with(path, "app", secret).map(|s| (s, Value::Null)),
        }
    }
}

fn serve(rx: &Receiver<(Request, Sender<Reply>)>) {
    let mut session: Option<Session> = None;
    let mut backups = Backups::default();
    let mut net = mk_commands::connect::NetRunner::default();
    let mut watcher = mk_watch::Watcher::new(std::time::Duration::from_secs(2));
    while let Ok((req, reply)) = rx.recv() {
        // Background jobs belong to the book that started them.
        if matches!(
            req,
            Request::Create { .. }
                | Request::Open { .. }
                | Request::CreateProtected { .. }
                | Request::OpenWith { .. }
                | Request::OpenPrevious { .. }
                | Request::Close
        ) {
            net = mk_commands::connect::NetRunner::default();
        }
        let out = match req {
            Request::CreateProtected {
                path,
                name,
                currency,
                password,
                recovery,
                preset,
            } => Session::create_encrypted_with_preset(
                std::path::Path::new(&path),
                &name,
                &currency,
                "app",
                &password,
                recovery,
                mk_commands::KdfParams::DEFAULT,
                preset.as_deref(),
            )
            .map(|(s, key)| {
                session = Some(s);
                json!(key)
            })
            .map_err(|e| err(&e)),
            Request::OpenWith { path, secret } => {
                session = None;
                backups
                    .open(std::path::Path::new(&path), Some(&secret))
                    .map(|(s, v)| {
                        session = Some(s);
                        v
                    })
                    .map_err(|e| err(&e))
            }
            Request::OpenPrevious { path, secret } => {
                session = None;
                let p = std::path::Path::new(&path);
                match restore_previous(p) {
                    Err(e) => Err(UiError::new(
                        "io",
                        format!("Couldn't restore the previous version: {e}"),
                    )),
                    Ok(()) => backups
                        .open(p, Some(&secret))
                        .map(|(s, v)| {
                            session = Some(s);
                            v
                        })
                        .map_err(|e| err(&e)),
                }
            }
            Request::Security(op) => match session.as_mut() {
                Some(s) => security(s, op).map(|k| json!(k)).map_err(|e| err(&e)),
                None => Err(UiError::new("no_book", "No book is open.")),
            },
            Request::Protection => Ok(match session.as_ref() {
                Some(s) => json!([s.is_encrypted(), s.has_recovery_key()]),
                None => json!([false, false]),
            }),
            Request::WatchPass => match session.as_mut() {
                Some(s) if s.is_encrypted() => s
                    .with_store(|registry, store| watcher.pass(registry, store))
                    .map(|outcomes| {
                        json!(
                            outcomes
                                .iter()
                                .map(|o| match o {
                                    mk_watch::Outcome::Staged { file, .. } => {
                                        format!("Imported {} for review.", file.display())
                                    }
                                    mk_watch::Outcome::Failed { file, error } => {
                                        format!("Could not import {}: {error}", file.display())
                                    }
                                    mk_watch::Outcome::Skipped { error, .. } => error.clone(),
                                })
                                .collect::<Vec<_>>()
                        )
                    })
                    .map_err(|e| UiError::new("watch", e)),
                _ => Ok(json!([])),
            },
            Request::Create {
                path,
                name,
                currency,
                preset,
            } => Session::create_with_preset(
                std::path::Path::new(&path),
                &name,
                &currency,
                "app",
                preset.as_deref(),
            )
                .map(|s| {
                    session = Some(s);
                    Value::Null
                })
                .map_err(|e| err(&e)),
            Request::Open { path } => backups
                .open(std::path::Path::new(&path), None)
                .map(|(s, v)| {
                    session = Some(s);
                    v
                })
                .map_err(|e| err(&e)),
            Request::BackupSettings(v) => {
                let env = mk_commands::backups::BackupEnv::new(
                    mk_commands::backups::BackupSettings::from_json(&v),
                    mk_connect::local_offset_secs(),
                );
                if let Some(s) = session.as_mut() {
                    s.set_backup_env(env.clone());
                }
                backups.env = Some(env);
                Ok(Value::Null)
            }
            Request::BackupStart(reason) => match session.as_mut() {
                Some(s) => backups.start(s, &reason),
                None => Err(UiError::new("no_book", "No book is open.")),
            },
            Request::BackupPoll => {
                backups.collect(session.as_mut(), false);
                Ok(Value::Array(std::mem::take(&mut backups.done)))
            }
            Request::BackupWait => {
                backups.collect(session.as_mut(), true);
                Ok(Value::Null)
            }
            Request::BackupChanged => Ok(json!(
                session.as_mut().is_some_and(Session::changed_since_backup)
            )),
            Request::BackupsList(book) => match &backups.env {
                Some(env) => Ok(Value::Array(
                    mk_commands::backups::list(std::path::Path::new(&book), env)
                        .iter()
                        .map(|e| e.to_json(env))
                        .collect(),
                )),
                None => Err(UiError::new("unsupported", "Backups are not set up.")),
            },
            Request::RestoreCopy {
                backup,
                target,
                secret,
            } => mk_commands::backups::restore_copy(
                std::path::Path::new(&backup),
                std::path::Path::new(&target),
                secret.as_ref(),
                None,
            )
            .map(|r| json!({"path": r.path.display().to_string(), "encrypted": r.encrypted}))
            .map_err(|e| err(&e)),
            Request::BackupFile { book, secret } => match &backups.env {
                Some(env) => mk_commands::backups::back_up_file(
                    std::path::Path::new(&book),
                    secret.as_ref(),
                    mk_commands::backups::Reason::Manual,
                    env,
                )
                .map(|m| m.to_json())
                .map_err(|e| err(&e)),
                None => Err(UiError::new("unsupported", "Backups are not set up.")),
            },
            Request::Close => {
                session = None;
                Ok(Value::Null)
            }
            Request::Invoke { command, args } => match session.as_mut() {
                Some(s) => networked(s).invoke(&command, args).map_err(|e| err(&e)),
                None => Err(UiError::new("no_book", "No book is open.")),
            },
            Request::NetStart { command, args } => match session.as_mut() {
                Some(s) => Ok(json!(net.start(networked(s), &command, args))),
                None => Err(UiError::new("no_book", "No book is open.")),
            },
            Request::NetPoll => Ok(match session.as_mut() {
                Some(s) => json!(
                    net.poll(s)
                        .into_iter()
                        .map(|(id, out)| match out {
                            Ok(v) => json!([id, {"ok": v}]),
                            Err(e) => json!([id, {"error": {"code": e.code(), "message": e.to_string()}}]),
                        })
                        .collect::<Vec<_>>()
                ),
                None => json!([]),
            }),
            Request::NetCancel(id) => {
                net.cancel(id);
                Ok(Value::Null)
            }
            Request::AiAccess => match session.as_ref() {
                Some(s) => s.ai_access().map(|a| a.to_json()).map_err(|e| err(&e)),
                None => Err(UiError::new("no_book", "No book is open.")),
            },
            Request::SetAiAccess(v) => match session.as_mut() {
                Some(s) => s
                    .set_ai_access(&mk_commands::ai_access::AiAccess::from_json(&v))
                    .map(|a| a.to_json())
                    .map_err(|e| err(&e)),
                None => Err(UiError::new("no_book", "No book is open.")),
            },
            Request::With(f) => {
                f(session.as_mut().map(networked));
                Ok(Value::Null)
            }
            Request::Stop => {
                // A backup still copying finishes before the app exits.
                backups.collect(None, true);
                let _ = reply.send(Ok(Value::Null));
                return;
            }
        };
        // A book just created gets the backup settings too.
        if let (Some(s), Some(env)) = (session.as_mut(), backups.env.as_ref())
            && s.backup_env().is_none()
        {
            s.set_backup_env(env.clone());
        }
        let _ = reply.send(out);
    }
}

impl WorkerBackend {
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel::<(Request, Sender<Reply>)>();
        let worker = std::thread::Builder::new()
            .name("moneykeep-book".into())
            .spawn(move || serve(&rx))
            .ok();
        Self {
            tx,
            book: None,
            worker,
            open_backup: None,
        }
    }

    /// The control channel's host: runs its calls on this worker.
    pub fn control_host(&self) -> WorkerHost {
        WorkerHost {
            tx: Mutex::new(self.tx.clone()),
        }
    }

    /// A handle the watch thread uses to run passes on the worker's book.
    pub fn watch_handle(&self) -> WatchHandle {
        WatchHandle {
            tx: self.tx.clone(),
        }
    }

    fn call(&self, req: Request) -> Reply {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send((req, reply_tx))
            .map_err(|_| UiError::new("internal", "the book worker stopped"))?;
        reply_rx
            .recv()
            .map_err(|_| UiError::new("internal", "the book worker stopped"))?
    }
}

/// Runs watched-folder passes on the worker's open (encrypted) book.
#[derive(Clone)]
pub struct WatchHandle {
    tx: Sender<(Request, Sender<Reply>)>,
}

impl WatchHandle {
    /// One pass; returns messages for the user.
    pub fn pass(&self) -> Result<Vec<String>, String> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send((Request::WatchPass, reply_tx))
            .map_err(|_| "the book worker stopped".to_owned())?;
        let v = reply_rx
            .recv()
            .map_err(|_| "the book worker stopped".to_owned())?
            .map_err(|e| e.message)?;
        Ok(v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Whether the open book is password protected.
    pub fn encrypted(&self) -> bool {
        let (reply_tx, reply_rx) = mpsc::channel();
        if self.tx.send((Request::Protection, reply_tx)).is_err() {
            return false;
        }
        reply_rx
            .recv()
            .ok()
            .and_then(Result::ok)
            .and_then(|v| v.get(0).and_then(Value::as_bool))
            .unwrap_or(false)
    }
}

/// The control channel's [`Host`]: every call runs on the worker, on the open
/// session, so it is serialised with the GUI's own actions.
pub struct WorkerHost {
    tx: Mutex<Queue>,
}

/// The worker's request queue.
type Queue = Sender<(Request, Sender<Reply>)>;

impl std::fmt::Debug for WorkerHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkerHost")
    }
}

impl WorkerHost {
    fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(Option<&mut Session>) -> T + Send + 'static,
    ) -> Option<T> {
        let (out_tx, out_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let job: Job = Box::new(move |s| {
            let _ = out_tx.send(f(s));
        });
        self.tx
            .lock()
            .ok()?
            .send((Request::With(job), reply_tx))
            .ok()?;
        let _ = reply_rx.recv();
        out_rx.recv().ok()
    }

    /// Whether the open book allows AI assistants.
    pub fn ai_enabled(&self) -> bool {
        self.run(|s| {
            s.and_then(|s| s.ai_access().ok())
                .is_some_and(|a| a.enabled)
        })
        .unwrap_or(false)
    }
}

impl Host for WorkerHost {
    fn state(&self) -> Option<HostState> {
        self.run(|s| s.and_then(mk_control::server::session_state))
            .flatten()
    }

    fn call(&self, actor: &str, command: &str, args: Value) -> CallReply {
        let (actor, command) = (actor.to_owned(), command.to_owned());
        self.run(move |s| match s {
            Some(s) => mk_control::server::session_call(s, &actor, &command, args),
            None => CallReply::Failed {
                code: "no_book".into(),
                message: "No book is open.".into(),
            },
        })
        .unwrap_or(CallReply::Failed {
            code: "internal".into(),
            message: "the book worker stopped".into(),
        })
    }

    fn session_allowed(&self, client: &str) {
        let client = client.to_owned();
        let _ = self.run(move |s| {
            if let Some(s) = s {
                let _ = s.record_ai_session_allowed(&client);
            }
        });
    }
}

impl Drop for WorkerBackend {
    fn drop(&mut self) {
        let _ = self.call(Request::Stop);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

fn file_stem(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_owned()
}

impl Backend for WorkerBackend {
    fn invoke(&mut self, command: &str, args: Value) -> Result<Value, UiError> {
        self.call(Request::Invoke {
            command: command.to_owned(),
            args,
        })
    }

    fn book_name(&self) -> Option<String> {
        self.book.clone()
    }

    fn save_tile_image(&mut self, book_path: &str, png: Option<&[u8]>) {
        if let Some(icons) = crate::config::icons_dir() {
            crate::config::save_tile_image(&icons, book_path, png);
        }
    }

    fn load_tile_image(&self, book_path: &str) -> Option<Vec<u8>> {
        crate::config::load_tile_image(&crate::config::icons_dir()?, book_path)
    }

    fn chart_presets(&mut self) -> Result<Value, UiError> {
        mk_commands::chart::presets_json().map_err(|e| err(&e))
    }

    fn create_book(
        &mut self,
        path: &str,
        name: &str,
        currency: &str,
        preset: Option<&str>,
    ) -> Result<(), UiError> {
        self.call(Request::Create {
            path: path.to_owned(),
            name: name.to_owned(),
            currency: currency.to_owned(),
            preset: preset.map(str::to_owned),
        })?;
        self.book = Some(name.to_owned());
        Ok(())
    }

    fn open_book(&mut self, path: &str) -> Result<(), UiError> {
        let v = self.call(Request::Open {
            path: path.to_owned(),
        })?;
        self.open_backup = v.get("before_update").filter(|b| b.is_object()).cloned();
        let name = self
            .invoke("book.info", serde_json::json!({}))
            .ok()
            .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| file_stem(path));
        self.book = Some(name);
        Ok(())
    }

    fn close_book(&mut self) {
        let _ = self.call(Request::Close);
        self.book = None;
    }

    fn create_protected_book(
        &mut self,
        path: &str,
        name: &str,
        currency: &str,
        password: &str,
        recovery: bool,
        preset: Option<&str>,
    ) -> Result<Option<String>, UiError> {
        let key = self.call(Request::CreateProtected {
            path: path.to_owned(),
            name: name.to_owned(),
            currency: currency.to_owned(),
            password: password.to_owned(),
            recovery,
            preset: preset.map(str::to_owned),
        })?;
        self.book = Some(name.to_owned());
        Ok(key.as_str().map(str::to_owned))
    }

    fn open_book_with(&mut self, path: &str, s: &UiSecret) -> Result<(), UiError> {
        let v = self.call(Request::OpenWith {
            path: path.to_owned(),
            secret: secret(s),
        })?;
        self.open_backup = v.get("before_update").filter(|b| b.is_object()).cloned();
        self.book = Some(self.name_of(path));
        Ok(())
    }

    fn open_previous(&mut self, path: &str, s: &UiSecret) -> Result<(), UiError> {
        self.call(Request::OpenPrevious {
            path: path.to_owned(),
            secret: secret(s),
        })?;
        self.book = Some(self.name_of(path));
        Ok(())
    }

    fn is_protected_file(&self, path: &str) -> Option<bool> {
        match mk_commands::book_kind(std::path::Path::new(path)) {
            Ok(kind) => Some(kind == BookKind::Encrypted),
            // Windows: an open encrypted book's lock covers its first bytes. Only an
            // encrypted book holds that lock (plain books use SQLite's own locks), so
            // the file is protected; opening it then says the book is in use.
            Err(e) if e.code() == "book_in_use" => Some(true),
            Err(_) => None,
        }
    }

    fn protection(&self) -> (bool, bool) {
        self.call(Request::Protection)
            .ok()
            .map(|v| {
                (
                    v.get(0).and_then(Value::as_bool).unwrap_or(false),
                    v.get(1).and_then(Value::as_bool).unwrap_or(false),
                )
            })
            .unwrap_or((false, false))
    }

    fn security(&mut self, op: SecurityOp) -> Result<Option<String>, UiError> {
        let v = self.call(Request::Security(op))?;
        Ok(v.as_str().map(str::to_owned))
    }

    fn net_start(&mut self, command: &str, args: Value) -> Result<u64, UiError> {
        let v = self.call(Request::NetStart {
            command: command.to_owned(),
            args,
        })?;
        v.as_u64()
            .ok_or_else(|| UiError::new("internal", "no job id"))
    }

    fn net_poll(&mut self) -> Vec<(u64, Result<Value, UiError>)> {
        let Ok(v) = self.call(Request::NetPoll) else {
            return Vec::new();
        };
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        let id = x.get(0)?.as_u64()?;
                        let r = x.get(1)?;
                        Some(match (r.get("ok"), r.get("error")) {
                            (Some(ok), _) => (id, Ok(ok.clone())),
                            (_, Some(e)) => (
                                id,
                                Err(UiError::new(
                                    e.get("code").and_then(Value::as_str).unwrap_or("error"),
                                    e.get("message").and_then(Value::as_str).unwrap_or(""),
                                )),
                            ),
                            _ => (id, Ok(Value::Null)),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn net_cancel(&mut self, id: u64) {
        let _ = self.call(Request::NetCancel(id));
    }

    fn ai_access(&self) -> Option<Value> {
        self.call(Request::AiAccess).ok()
    }

    fn set_backup_settings(&mut self, settings: Value) {
        let _ = self.call(Request::BackupSettings(settings));
    }

    fn default_backup_folder(&self) -> Option<String> {
        mk_commands::backups::default_folder().map(|p| p.display().to_string())
    }

    fn test_backup_folder(&mut self, folder: &str) -> Result<(), UiError> {
        mk_commands::backups::test_folder(
            std::path::Path::new(folder),
            &mk_commands::backups::BackupEnv::default(),
        )
        .map_err(|e| UiError::new(e.code(), e.to_string()))
    }

    fn backup_start(&mut self, reason: &str) -> Result<(), UiError> {
        self.call(Request::BackupStart(reason.to_owned()))
            .map(|_| ())
    }

    fn backup_poll(&mut self) -> Vec<Result<Value, UiError>> {
        let Ok(Value::Array(done)) = self.call(Request::BackupPoll) else {
            return Vec::new();
        };
        done.into_iter()
            .map(|r| match (r.get("ok"), r.get("error")) {
                (Some(ok), _) => Ok(ok.clone()),
                (_, Some(e)) => Err(UiError::new(
                    e.get("code").and_then(Value::as_str).unwrap_or("error"),
                    e.get("message").and_then(Value::as_str).unwrap_or(""),
                )),
                _ => Ok(Value::Null),
            })
            .collect()
    }

    fn backup_changed(&mut self) -> bool {
        self.call(Request::BackupChanged)
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    fn take_open_backup(&mut self) -> Option<Value> {
        self.open_backup.take()
    }

    fn backups_list(&mut self, book: &str) -> Result<Value, UiError> {
        self.call(Request::BackupsList(book.to_owned()))
    }

    fn restore_copy(
        &mut self,
        backup: &str,
        target: &str,
        s: Option<&UiSecret>,
    ) -> Result<Value, UiError> {
        self.call(Request::RestoreCopy {
            backup: backup.to_owned(),
            target: target.to_owned(),
            secret: s.map(secret),
        })
    }

    fn backup_book_file(&mut self, book: &str, s: Option<&UiSecret>) -> Result<Value, UiError> {
        self.call(Request::BackupFile {
            book: book.to_owned(),
            secret: s.map(secret),
        })
    }

    fn finish_backups(&mut self) {
        let _ = self.call(Request::BackupWait);
    }

    fn set_ai_access(&mut self, settings: Value) -> Result<Value, UiError> {
        self.call(Request::SetAiAccess(settings))
    }
}

impl WorkerBackend {
    fn name_of(&self, path: &str) -> String {
        self.call(Request::Invoke {
            command: "book.info".into(),
            args: json!({}),
        })
        .ok()
        .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| file_stem(path))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;
    use serde_json::json;

    #[test]
    fn worker_runs_commands_one_at_a_time_as_app() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.moneykeep");
        let mut b = WorkerBackend::start();
        assert_eq!(
            b.invoke("book.info", json!({})).unwrap_err().code,
            "no_book"
        );
        b.create_book(path.to_str().unwrap(), "Worker", "USD", None)
            .unwrap();
        assert_eq!(b.book_name().as_deref(), Some("Worker"));
        b.invoke("account.create", json!({"name": "Cash", "type": "asset"}))
            .unwrap();
        let log = b.invoke("history.list", json!({"limit": 1})).unwrap();
        assert_eq!(log[0]["actor"], "app");
        b.close_book();
        assert!(b.book_name().is_none());
        b.open_book(path.to_str().unwrap()).unwrap();
        assert_eq!(b.book_name().as_deref(), Some("Worker"));
        let e = b
            .open_book(dir.path().join("missing.moneykeep").to_str().unwrap())
            .unwrap_err();
        assert_eq!(
            e.code, "io",
            "app-desktop revision 4: file failures are `io`"
        );
    }

    #[test]
    fn worker_creates_and_unlocks_protected_books() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.moneykeep");
        let p = path.to_str().unwrap();
        let mut b = WorkerBackend::start();
        let key = b
            .create_protected_book(p, "Protected", "USD", "correct horse battery", true, None)
            .unwrap();
        assert!(key.unwrap().starts_with("MCRK-"));
        assert_eq!(b.protection(), (true, true));
        assert_eq!(b.is_protected_file(p), Some(true));
        b.close_book();
        let wrong = b
            .open_book_with(p, &UiSecret::Password("nope nope nope".into()))
            .unwrap_err();
        assert_eq!(wrong.code, "wrong_password");
        b.open_book_with(p, &UiSecret::Password("correct horse battery".into()))
            .unwrap();
        assert_eq!(b.book_name().as_deref(), Some("Protected"));
        assert!(b.watch_handle().encrypted());
        assert_eq!(
            b.watch_handle().pass().unwrap_or_default(),
            Vec::<String>::new()
        );
    }

    /// backups.md: backups are copied on their own thread while the worker keeps
    /// serving; the before-update backup comes with the open.
    #[test]
    fn backups_run_beside_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Household.moneykeep");
        let p = path.to_str().unwrap();
        let folder = dir.path().join("Backups");
        let mut b = WorkerBackend::start();
        b.set_backup_settings(json!({"folder": folder.to_str().unwrap()}));
        b.create_book(p, "Household", "USD", None).unwrap();
        b.invoke("account.create", json!({"name": "Cash", "type": "asset"}))
            .unwrap();
        assert!(b.backup_changed());
        b.backup_start("manual").unwrap();
        // The worker answers while the copy runs.
        b.invoke("book.info", json!({})).unwrap();
        let mut done = Vec::new();
        for _ in 0..200 {
            done.extend(b.backup_poll());
            if !done.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let made = done.pop().unwrap().unwrap();
        assert!(
            made["path"]
                .as_str()
                .unwrap()
                .ends_with(" manual.moneykeep")
        );
        assert!(!b.backup_changed(), "marked once the backup finished");
        let list = b.backups_list(p).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
        // Closing: the copy finishes even when the book closes at once.
        b.invoke("account.create", json!({"name": "Bank", "type": "asset"}))
            .unwrap();
        b.backup_start("closed").unwrap();
        b.close_book();
        b.finish_backups();
        let done = b.backup_poll();
        assert_eq!(done.len(), 1);
        assert!(
            done[0].as_ref().unwrap()["path"]
                .as_str()
                .unwrap()
                .ends_with(" closed.moneykeep")
        );
        // An old book: the before-update backup is reported with the open.
        let old = dir.path().join("Old.moneykeep");
        mk_store_old(&old);
        b.open_book(old.to_str().unwrap()).unwrap();
        let v = b.take_open_backup().unwrap();
        assert_eq!(v["reason"], "before-update");
    }

    fn mk_store_old(path: &std::path::Path) {
        // An old-format book (the format before this build's).
        let old = mk_commands::latest_schema_version() - 1;
        mk_commands::create_old_book_for_tests(path, "Old", old).unwrap();
    }
}
