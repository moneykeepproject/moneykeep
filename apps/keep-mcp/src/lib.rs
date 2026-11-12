//! `keep-mcp`: the MoneyKeep MCP server program (mcp-server.md revision 3).
//!
//! It reaches a book by one of two routes, chosen at the first request and again on
//! every request until one works:
//!
//! 1. the **app route**: the running MoneyKeep app advertises its control channel
//!    (`control.json`) for the requested book (or `--app` was given); the owner
//!    allows the session in the app;
//! 2. the **direct route**: a plain book whose owner turned AI access on and allowed
//!    use without the app; `keep-mcp` opens the file itself.
//!
//! Otherwise there is **no route**, and the server says why. `keep-mcp` never asks
//! for, reads or receives a book password.

use std::path::{Path, PathBuf};

use mk_commands::ai_access;
use mk_commands::{BookKind, CommandSpec, Registry};
use mk_control::client::{ConnectError, ControlClient, Hello, HelloOk, OnEvent};
use mk_control::server::CallReply;
use mk_mcp::{Access, Backend, DirectBackend, Outcome, Tool, Waker, registry_spec};
use serde_json::{Value, json};

/// `keep-mcp --app` with no app serving a book.
pub const MSG_NO_APP: &str = "MoneyKeep is not running with a book open for AI assistants. Open the book in MoneyKeep and turn on AI assistant access (Settings → Security).";
/// The owner pressed Deny in the app.
pub const MSG_DENIED: &str =
    "The owner did not allow this assistant in MoneyKeep. Restart the assistant to ask again.";

/// What `keep-mcp` serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `keep-mcp <book>`.
    Book(PathBuf),
    /// `keep-mcp --app`: whichever book the running app has open.
    App,
}

struct AppRoute {
    client: ControlClient,
    hello: HelloOk,
}

/// The route chooser: a [`Backend`] that uses the app route, the direct route, or
/// neither, deciding again whenever the current route stops working.
pub struct Router {
    target: Target,
    control_dir: Option<PathBuf>,
    make_registry: Box<dyn Fn() -> Registry>,
    lookup: Registry,
    direct: Option<DirectBackend>,
    app: Option<AppRoute>,
    /// The listening session (token) whose owner denied this assistant.
    denied_token: Option<String>,
    waker: Option<Waker>,
    /// The settings for the before-update backup the direct route makes when it
    /// opens a book whose format must be updated (backups.md revision 2).
    backup_env: mk_commands::backups::BackupEnv,
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router")
            .field("target", &self.target)
            .field("app", &self.app.is_some())
            .field("direct", &self.direct.is_some())
            .finish_non_exhaustive()
    }
}

/// Reads files for the `files` group's commands on the direct route.
#[derive(Debug)]
struct LocalFiles;

impl mk_commands::FileReader for LocalFiles {
    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        std::fs::read(path).map_err(|e| e.to_string())
    }
}

impl Router {
    /// `make_registry` builds the registry for the direct route (the program gives
    /// it network access for bank feeds); `control_dir` is where the app's
    /// `control.json` is looked for (`None`: never use the app route).
    pub fn new(
        target: Target,
        control_dir: Option<PathBuf>,
        make_registry: Box<dyn Fn() -> Registry>,
    ) -> Self {
        let lookup = make_registry();
        Self {
            target,
            control_dir,
            make_registry,
            lookup,
            direct: None,
            app: None,
            denied_token: None,
            waker: None,
            backup_env: mk_commands::backups::BackupEnv::from_app_config(),
        }
    }

    /// Use these backup settings for the before-update backup instead of the
    /// desktop app's (tests).
    #[must_use]
    pub fn with_backup_env(mut self, env: mk_commands::backups::BackupEnv) -> Self {
        self.backup_env = env;
        self
    }

    /// Whether the current route is the app's control channel.
    pub fn on_app_route(&self) -> bool {
        self.app.as_ref().is_some_and(|a| !a.client.is_closed())
    }

    fn app_access(&mut self) -> Option<Access> {
        let a = self.app.as_mut()?;
        if !a.client.is_closed()
            && let Ok((commands, access)) = a.client.list()
        {
            return Some(Access::Ready {
                tools: commands.iter().filter_map(Tool::from_json).collect(),
                access,
                book: a.hello.book_name.clone(),
            });
        }
        self.app = None;
        None
    }

    /// Try the app route. `None`: not advertised for this book (try direct).
    fn try_app(&mut self, client: &str) -> Option<Result<(), String>> {
        let dir = self.control_dir.as_ref()?;
        let d = mk_control::live_discovery(dir)?;
        let book_path = match &self.target {
            Target::Book(p) => {
                let c = mk_control::canonical(p);
                if c != d.book_path {
                    return None;
                }
                Some(c)
            }
            Target::App => None,
        };
        if self.denied_token.as_deref() == Some(d.token.as_str()) {
            return Some(Err(MSG_DENIED.to_owned()));
        }
        let waker = self.waker.clone();
        let on_event: OnEvent = Box::new(move || {
            if let Some(w) = &waker {
                w();
            }
        });
        let hello = Hello {
            client: client.to_owned(),
            book_path,
        };
        match ControlClient::connect(&d, &hello, on_event) {
            Ok((client, hello)) => {
                self.app = Some(AppRoute { client, hello });
                Some(Ok(()))
            }
            Err(ConnectError::Refused { code, .. }) if code == "denied" => {
                self.denied_token = Some(d.token);
                Some(Err(MSG_DENIED.to_owned()))
            }
            Err(ConnectError::Refused { code, .. })
                if code == "wrong_book" || code == "bad_token" =>
            {
                None
            }
            Err(ConnectError::Refused { message, .. }) => Some(Err(message)),
            Err(ConnectError::Io(_)) => None,
        }
    }

    fn direct_access(&mut self, client: &str) -> Access {
        let Target::Book(path) = &self.target else {
            return Access::NoRoute {
                message: MSG_NO_APP.to_owned(),
            };
        };
        // A password-protected book is never opened here.
        match mk_commands::book_kind(path) {
            Ok(BookKind::Encrypted) => {
                self.direct = None;
                return Access::NoRoute {
                    message: ai_access::MSG_PASSWORD.to_owned(),
                };
            }
            Ok(_) => {}
            Err(e) => {
                self.direct = None;
                return Access::NoRoute {
                    message: e.to_string(),
                };
            }
        }
        if self.direct.is_none() {
            // Opened the way every program opens a book: a book whose format must
            // be updated is backed up first (only that backup is made here); if
            // the backup fails the book is left closed and unchanged, and the
            // call that opened it returns the message.
            match mk_commands::backups::open_book(path, None, &self.backup_env) {
                Ok((store, _)) => {
                    self.direct = Some(
                        DirectBackend::new((self.make_registry)(), store)
                            .with_files(Box::new(LocalFiles)),
                    );
                }
                Err(e) => {
                    return Access::NoRoute {
                        message: e.to_string(),
                    };
                }
            }
        }
        match self.direct.as_mut() {
            Some(d) => d.access(client),
            None => Access::NoRoute {
                message: MSG_NO_APP.to_owned(),
            },
        }
    }
}

impl Backend for Router {
    fn access(&mut self, client: &str) -> Access {
        if let Some(acc) = self.app_access() {
            return acc;
        }
        // A direct route that still works is kept.
        if let Some(d) = self.direct.as_mut()
            && d.check().is_ok()
            && is_plain(&self.target)
        {
            return d.access(client);
        }
        match self.try_app(client) {
            Some(Ok(())) => {
                if let Some(acc) = self.app_access() {
                    return acc;
                }
            }
            Some(Err(message)) => return Access::NoRoute { message },
            None => {}
        }
        self.direct_access(client)
    }

    fn call(&mut self, client: &str, command: &str, args: Value) -> Outcome {
        if let Some(a) = self.app.as_mut() {
            return match a.client.call(command, args) {
                Ok(CallReply::Ok { value, .. }) => Outcome::Ok(value),
                Ok(CallReply::Failed { code, message }) => {
                    Outcome::Failed(json!({"error": {"code": code, "message": message}}))
                }
                Ok(CallReply::NotAvailable(m)) => Outcome::NotAvailable(m),
                Err(e) => {
                    self.app = None;
                    Outcome::Failed(json!({"error": {"code": "io", "message": e}}))
                }
            };
        }
        match self.direct.as_mut() {
            Some(d) => d.call(client, command, args),
            None => Outcome::NotAvailable(MSG_NO_APP.to_owned()),
        }
    }

    fn spec(&self, tool: &str) -> Option<CommandSpec> {
        match &self.direct {
            Some(d) => registry_spec(d.registry(), tool),
            None => registry_spec(&self.lookup, tool),
        }
    }

    fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }
}

fn is_plain(t: &Target) -> bool {
    matches!(t, Target::Book(p) if matches!(mk_commands::book_kind(p), Ok(BookKind::Plain)))
}

/// The absolute form of a path (canonical when it exists).
pub fn absolute(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|d| d.join(p))
                .unwrap_or_else(|_| p.to_path_buf())
        }
    })
}

/// The JSON `keep-mcp config` prints.
pub fn config_json(client: mk_control::ClientKind, exe: &Path, target: &Target) -> Value {
    let args = match target {
        Target::Book(p) => vec![absolute(p).display().to_string()],
        Target::App => vec!["--app".to_owned()],
    };
    mk_control::config_snippet(client, &absolute(exe).display().to_string(), &args)
}
