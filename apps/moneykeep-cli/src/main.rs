//! `moneykeep` — the command-line front end.
//!
//! The CLI owns no behaviour of its own: every action goes through the shared command
//! registry, exactly as the GUI and the MCP server (`keep-mcp`, a separate program) do. Convenience subcommands (`accounts`,
//! `balance`, `undo`, …) only build the JSON arguments for one registry command.

// The CLI's job is to print to stdout.
#![allow(clippy::print_stdout)]

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mk_watch as watch;

use clap::{Parser, Subcommand};
use mk_commands::{
    Actor, BookKind, CommandError, FileReader, KdfParams, Registry, Secret, Session,
};
use mk_core::CommodityCode;
use mk_store::Store;
use serde_json::{Value, json};

#[derive(Parser)]
#[command(
    name = "moneykeep",
    display_name = "MoneyKeep",
    version,
    about = "MoneyKeep — Apps for Financial Management: personal and business accounting"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
    /// For a password-protected book: read the password from the first line of this
    /// file. (Otherwise MONEYKEEP_PASSWORD, which is less safe, or a prompt.)
    #[arg(long, global = true)]
    password_file: Option<PathBuf>,
    /// Give a recovery key instead of the password.
    #[arg(long, global = true)]
    recovery_key: bool,
    /// When setting a password without a terminal: read the new password from the
    /// first line of this file.
    #[arg(long, global = true)]
    new_password_file: Option<PathBuf>,
    /// Make every backup this command makes in this folder only (not the desktop
    /// app's backup folder and second place).
    #[arg(long, global = true)]
    backup_folder: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new book file.
    Init {
        /// Path of the new book file (e.g. household.moneykeep).
        path: PathBuf,
        /// Book name.
        #[arg(long, default_value = "My Books")]
        name: String,
        /// Base currency (ISO 4217).
        #[arg(long, default_value = "USD")]
        currency: String,
        /// Protect the new book with a password (asked twice).
        #[arg(long)]
        password: bool,
        /// With --password: do not create a recovery key.
        #[arg(long)]
        no_recovery_key: bool,
        /// Start from a built-in chart of accounts (an id listed by `chart.presets`,
        /// e.g. personal, personal_1040, schedule_c, small_business, nonprofit).
        #[arg(long)]
        preset: Option<String>,
    },
    /// Protect a plain book with a password (asked twice); prints the recovery key.
    Encrypt {
        path: PathBuf,
        #[arg(long)]
        no_recovery_key: bool,
    },
    /// Change a protected book's password.
    Passwd { path: PathBuf },
    /// Create a new recovery key (the old one stops working), or remove it.
    RecoveryKey {
        path: PathBuf,
        #[arg(long)]
        remove: bool,
    },
    /// Write an unencrypted copy of a protected book to another file.
    Decrypt {
        path: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Turn a protected book back into a plain one in place.
    RemoveEncryption {
        path: PathBuf,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// List commands. With a book path, only commands enabled in that book.
    Commands {
        /// Optional book file.
        path: Option<PathBuf>,
    },
    /// Run a registry command against a book.
    Run {
        /// Book file.
        path: PathBuf,
        /// Command name, e.g. module.enable.
        command: String,
        /// JSON arguments, e.g. '{"module":"estimates"}'.
        #[arg(default_value = "{}")]
        args: String,
        /// Actor name recorded in the audit log.
        #[arg(long, default_value = "cli")]
        actor: String,
    },
    /// Show or change whether AI assistants may use a book (through keep-mcp). With
    /// no flags, prints the current settings as JSON.
    AiAccess {
        /// Book file.
        path: PathBuf,
        /// Allow AI assistants to use this book.
        #[arg(long, conflicts_with = "off")]
        on: bool,
        /// Do not allow AI assistants.
        #[arg(long)]
        off: bool,
        /// read-only or read-write.
        #[arg(long, value_parser = ["read-only", "read-write"])]
        level: Option<String>,
        /// Permission groups, comma-separated: delete, settings, files, connections
        /// (an empty value or "none" for none).
        #[arg(long)]
        groups: Option<String>,
        /// Only through the running app, asking each session (the default).
        #[arg(long, conflicts_with = "direct")]
        app_only: bool,
        /// Allow keep-mcp to open this (plain) book directly.
        #[arg(long)]
        direct: bool,
    },
    /// List accounts (registry command account.list).
    Accounts {
        path: PathBuf,
        /// Include archived accounts.
        #[arg(long)]
        all: bool,
    },
    /// Show an account balance (registry command account.balance).
    Balance {
        path: PathBuf,
        /// Account id, full name or code.
        account: String,
        /// Balance as of this date (YYYY-MM-DD).
        #[arg(long)]
        as_of: Option<String>,
        /// Include sub-accounts.
        #[arg(long)]
        children: bool,
    },
    /// Trial balance (registry command report.trial_balance).
    TrialBalance {
        path: PathBuf,
        #[arg(long)]
        as_of: Option<String>,
    },
    /// Undo your most recent change (registry command history.undo).
    Undo {
        path: PathBuf,
        #[arg(long, default_value = "cli")]
        actor: String,
    },
    /// Redo your most recently undone change (registry command history.redo).
    Redo {
        path: PathBuf,
        #[arg(long, default_value = "cli")]
        actor: String,
    },
    /// Stage a bank file for review (registry command import.stage).
    Import {
        path: PathBuf,
        /// The file to import (.qif, .csv; .ofx/.qfx/.qbo later).
        file: PathBuf,
        /// Target account (id, full name or code).
        #[arg(long)]
        account: Option<String>,
        /// CSV profile name.
        #[arg(long)]
        profile: Option<String>,
        /// QIF dates are day-first.
        #[arg(long)]
        dmy: bool,
        #[arg(long, default_value = "cli")]
        actor: String,
    },
    /// Sync bank feeds and stage new transactions for review (connect.sync); for the
    /// user's own scheduler.
    Sync {
        path: PathBuf,
        /// Only this connection (id or label).
        #[arg(long)]
        connection: Option<String>,
        /// Sync even if synced within the hour.
        #[arg(long)]
        now: bool,
    },
    /// Watch the book's import folder until interrupted (setting import.watch_folder).
    Watch {
        path: PathBuf,
        /// Also sync bank feeds on their interval (bank-feeds.sync_interval_hours).
        #[arg(long)]
        feeds: bool,
        /// Seconds a file must stay unchanged before it is imported.
        #[arg(long, default_value_t = 2)]
        settle: u64,
        /// Scan once (after waiting for the settle time) and exit.
        #[arg(long)]
        once: bool,
    },
    /// Back up a book now (registry command book.backup): to the backup folder of
    /// the desktop app's settings (default Documents/MoneyKeep Backups) and its
    /// second place.
    Backup {
        path: PathBuf,
        /// manual (default), opened, closed or daily.
        #[arg(long, default_value = "manual")]
        reason: String,
        /// Use this backup folder instead of the app's setting.
        #[arg(long)]
        folder: Option<PathBuf>,
    },
    /// List a book's backups, newest first (registry command book.backups); the book
    /// is not opened. `backups protect <book>` protects the earlier plain backups of
    /// a password-protected book.
    #[command(args_conflicts_with_subcommands = true)]
    Backups {
        #[command(subcommand)]
        action: Option<BackupsCmd>,
        path: Option<PathBuf>,
        /// Look in this backup folder instead of the app's setting.
        #[arg(long)]
        folder: Option<PathBuf>,
    },
    /// Show recent audit history (registry command history.list).
    History {
        path: PathBuf,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
}

#[derive(Subcommand)]
enum BackupsCmd {
    /// Protect a password-protected book's earlier backups that aren't
    /// password-protected (registry command book.protect_backups): each is written
    /// again encrypted with the book's password, checked, then the plain copy is
    /// deleted.
    Protect {
        path: PathBuf,
        /// Look in this backup folder instead of the app's setting.
        #[arg(long)]
        folder: Option<PathBuf>,
    },
}

/// `--backup-folder`, for every backup this run makes.
static BACKUP_FOLDER: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(value) => {
            println!("{}", pretty(&value));
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{}", pretty(&err));
            ExitCode::FAILURE
        }
    }
}

/// The CLI can read files for commands that take a path.
struct LocalFiles;

impl FileReader for LocalFiles {
    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        std::fs::read(path).map_err(|e| e.to_string())
    }
}

/// Where secrets come from for this run.
struct Secrets {
    password_file: Option<PathBuf>,
    recovery_key: bool,
    new_password_file: Option<PathBuf>,
}

fn store_err(e: mk_store::StoreError) -> Value {
    CommandError::from(e).to_json()
}

fn first_line(path: &Path) -> Result<String, Value> {
    let text = std::fs::read_to_string(path).map_err(|e| error("io", e))?;
    Ok(text.lines().next().unwrap_or("").to_owned())
}

impl Secrets {
    fn wrap(&self, s: String) -> Secret {
        if self.recovery_key {
            Secret::RecoveryKey(s)
        } else {
            Secret::Password(s)
        }
    }

    /// The secret for an encrypted book: --password-file, MONEYKEEP_PASSWORD, or a
    /// prompt without echo on a terminal; otherwise `password_required`.
    fn current(&self, path: &Path) -> Result<Secret, Value> {
        if let Some(f) = &self.password_file {
            return Ok(self.wrap(first_line(f)?));
        }
        if let Ok(p) = std::env::var("MONEYKEEP_PASSWORD") {
            return Ok(self.wrap(p));
        }
        if std::io::stdin().is_terminal() {
            let what = if self.recovery_key {
                "Recovery key"
            } else {
                "Password"
            };
            let p = rpassword::prompt_password(format!("{what} for {}: ", path.display()))
                .map_err(|e| error("io", e))?;
            return Ok(self.wrap(p));
        }
        Err(store_err(mk_store::StoreError::PasswordRequired))
    }

    /// A new password: --new-password-file, or asked twice on a terminal.
    fn new_password(&self) -> Result<String, Value> {
        if let Some(f) = &self.new_password_file {
            return first_line(f);
        }
        if !std::io::stdin().is_terminal() {
            return Err(error(
                "invalid_args",
                "setting a password needs a terminal (or --new-password-file)",
            ));
        }
        eprintln!("A long passphrase of several words is best.");
        let a = rpassword::prompt_password("New password: ").map_err(|e| error("io", e))?;
        let b =
            rpassword::prompt_password("Repeat the new password: ").map_err(|e| error("io", e))?;
        if a != b {
            return Err(error("invalid_args", "the two passwords differ"));
        }
        Ok(a)
    }

    /// Open a book, asking for the secret only when it is encrypted.
    fn secret_for(&self, path: &Path) -> Result<Option<Secret>, Value> {
        match mk_store::book_kind(path) {
            Ok(BookKind::Encrypted) => Ok(Some(self.current(path)?)),
            _ => Ok(None),
        }
    }

    /// Open a book; a book whose format must be updated is backed up first
    /// (backups.md), and stays unchanged if that fails.
    fn store(&self, path: &Path) -> Result<Store, Value> {
        let secret = self.secret_for(path)?;
        let env = backup_env(None);
        mk_commands::backups::open_book(path, secret.as_ref(), &env)
            .map(|(store, _)| store)
            .map_err(|e: CommandError| e.to_json())
    }

    fn session(&self, path: &Path, actor: &str) -> Result<Session, Value> {
        let secret = self.secret_for(path)?;
        Session::open_backed_up(path, actor, secret.as_ref(), &backup_env(None))
            .map(|(s, _)| s)
            .map_err(|e| e.to_json())
    }
}

/// Run one registry command against a book file.
/// The registry with network access for bank feeds (connectivity.md).
fn networked() -> Registry {
    let mut r = Registry::with_builtins();
    r.set_connector(mk_connect::connector());
    r.set_backup_env(backup_env(None));
    r
}

/// The backup settings: the desktop app's (`backup` in its config), or `folder`;
/// with `--backup-folder`, that folder only.
fn backup_env(folder: Option<&Path>) -> mk_commands::backups::BackupEnv {
    let mut settings = mk_commands::backups::BackupSettings::from_app_config();
    if let Some(only) = BACKUP_FOLDER.get() {
        settings.folder = Some(only.display().to_string());
        settings.second_place = None;
    }
    if let Some(f) = folder {
        settings.folder = Some(f.display().to_string());
    }
    mk_commands::backups::BackupEnv::new(settings, mk_connect::local_offset_secs())
}

fn invoke(
    secrets: &Secrets,
    path: &Path,
    actor: &str,
    command: &str,
    args: Value,
) -> Result<Value, Value> {
    let registry = networked();
    let mut store = secrets.store(path)?;
    registry
        .invoke_with(
            &mut store,
            &Actor(actor.to_owned()),
            command,
            args,
            Some(&LocalFiles),
        )
        .map_err(|e| e.to_json())
}

/// Before a password change rewrites the book: the before-update backup, as the app
/// makes one when it opens a book (security-review-1 L7). If it fails, nothing is
/// changed (`backup_failed`). Skipped when opening already made one.
fn backup_first(
    session: &mut Session,
    path: &Path,
    made_on_open: Option<mk_commands::backups::Made>,
) -> Result<Value, Value> {
    if let Some(m) = made_on_open {
        return Ok(m.to_json());
    }
    let env = backup_env(None);
    let (source, stamp) = session.backup_capture().map_err(|e| e.to_json())?;
    let made =
        mk_commands::backups::before_update_backup(&source, path, &env).map_err(|e| e.to_json())?;
    session.mark_backed_up(stamp);
    Ok(made.to_json())
}

/// Open a book for a password command, then make the backup first (L7).
fn password_session(path: &Path, secret: Option<&Secret>) -> Result<(Session, Value), Value> {
    let (mut session, made) =
        Session::open_backed_up(path, "cli", secret, &backup_env(None)).map_err(|e| e.to_json())?;
    let backup = backup_first(&mut session, path, made)?;
    Ok((session, backup))
}

fn run(cli: Cli) -> Result<Value, Value> {
    if let Some(f) = cli.backup_folder.clone() {
        let _ = BACKUP_FOLDER.set(f);
    }
    let registry = Registry::with_builtins();
    let secrets = Secrets {
        password_file: cli.password_file,
        recovery_key: cli.recovery_key,
        new_password_file: cli.new_password_file,
    };
    let s = &secrets;
    match cli.command {
        Cmd::Init {
            path,
            name,
            currency,
            password,
            no_recovery_key,
            preset,
        } => {
            if let Some(p) = &preset {
                // An unknown preset is refused before any file is written.
                mk_commands::chart::check_preset(p).map_err(|e| e.to_json())?;
            }
            if password {
                let pw = s.new_password()?;
                let (mut session, key) = Session::create_encrypted_with_preset(
                    &path,
                    &name,
                    &currency,
                    "cli",
                    &pw,
                    !no_recovery_key,
                    KdfParams::DEFAULT,
                    preset.as_deref(),
                )
                .map_err(|e| e.to_json())?;
                let info = session
                    .invoke("book.info", json!({}))
                    .map_err(|e| e.to_json())?;
                return Ok(json!({ "created": path.display().to_string(), "book": info,
                    "password_protected": true, "recovery_key": key }));
            }
            if let Some(p) = &preset {
                let mut session =
                    Session::create_with_preset(&path, &name, &currency, "cli", Some(p))
                        .map_err(|e| e.to_json())?;
                let info = session
                    .invoke("book.info", json!({}))
                    .map_err(|e| e.to_json())?;
                let accounts = session
                    .invoke("account.list", json!({}))
                    .map_err(|e| e.to_json())?;
                let n = accounts.as_array().map_or(0, Vec::len);
                return Ok(json!({ "created": path.display().to_string(), "book": info,
                    "preset": p, "accounts": n }));
            }
            let currency = CommodityCode::new(&currency).map_err(|e| error("invalid_args", e))?;
            let store = Store::create(&path, &name, &currency).map_err(store_err)?;
            let info = store.info().map_err(store_err)?;
            Ok(json!({ "created": path.display().to_string(), "book": info }))
        }
        Cmd::Encrypt {
            path,
            no_recovery_key,
        } => {
            let pw = s.new_password()?;
            let (mut session, backup) = password_session(&path, None)?;
            let key = session
                .encrypt(&pw, !no_recovery_key)
                .map_err(|e| e.to_json())?;
            let mut out = json!({ "encrypted": path.display().to_string(), "recovery_key": key,
                "note": "Earlier unencrypted copies of this file may remain on the disk until overwritten.",
                "backup": backup });
            // Earlier backups are still plain (security-review-1 B2).
            let n = mk_commands::backups::unprotected(&path, &backup_env(None)).len();
            if n > 0 {
                let protect = format!("moneykeep backups protect \"{}\"", path.display());
                let message = mk_commands::backups::unprotected_text(n);
                eprintln!("{message} To protect them with the new password, run: {protect}");
                if let Some(o) = out.as_object_mut() {
                    o.insert(
                        "unprotected_backups".into(),
                        json!({ "count": n, "message": message, "protect": protect }),
                    );
                }
            }
            Ok(out)
        }
        Cmd::Passwd { path } => {
            let current = s.current(&path)?;
            let pw = s.new_password()?;
            let (mut session, backup) = password_session(&path, Some(&current))?;
            session
                .change_password(&current, &pw)
                .map_err(|e| e.to_json())?;
            Ok(json!({ "password_changed": path.display().to_string(), "backup": backup }))
        }
        Cmd::RecoveryKey { path, remove } => {
            let current = s.current(&path)?;
            let (mut session, backup) = password_session(&path, Some(&current))?;
            if remove {
                session
                    .remove_recovery_key(&current)
                    .map_err(|e| e.to_json())?;
                Ok(json!({ "recovery_key": null, "backup": backup }))
            } else {
                let key = session
                    .new_recovery_key(&current)
                    .map_err(|e| e.to_json())?;
                Ok(json!({ "recovery_key": key, "backup": backup }))
            }
        }
        Cmd::Decrypt { path, out } => {
            let mut session = s.session(&path, "cli")?;
            let at = session
                .save_unencrypted_copy(&out)
                .map_err(|e| e.to_json())?;
            Ok(json!({ "unencrypted_copy": at.display().to_string() }))
        }
        Cmd::RemoveEncryption { path, yes } => {
            if !yes {
                if !std::io::stdin().is_terminal() {
                    return Err(error(
                        "invalid_args",
                        "add --yes to confirm removing the password",
                    ));
                }
                eprint!(
                    "Remove password protection from {}? Type yes: ",
                    path.display()
                );
                let mut line = String::new();
                std::io::stdin()
                    .read_line(&mut line)
                    .map_err(|e| error("io", e))?;
                if line.trim() != "yes" {
                    return Err(error("invalid_args", "not confirmed"));
                }
            }
            let current = s.current(&path)?;
            let (mut session, backup) = password_session(&path, Some(&current))?;
            session
                .remove_encryption(&current)
                .map_err(|e| e.to_json())?;
            Ok(json!({ "decrypted": path.display().to_string(), "backup": backup }))
        }
        Cmd::Commands { path } => {
            let specs = match path {
                Some(p) => {
                    let store = s.store(&p)?;
                    registry
                        .available_specs(&store)
                        .map_err(|e| error(e.code(), e))?
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>()
                }
                None => registry.specs().into_iter().cloned().collect(),
            };
            serde_json::to_value(specs).map_err(|e| error("internal", e))
        }
        Cmd::Run {
            path,
            command,
            args,
            actor,
        } => {
            let args: Value = serde_json::from_str(&args).map_err(|e| error("invalid_args", e))?;
            invoke(s, &path, &actor, &command, args)
        }
        Cmd::AiAccess {
            path,
            on,
            off,
            level,
            groups,
            app_only,
            direct,
        } => {
            let mut session = s.session(&path, "cli")?;
            let mut a = session.ai_access().map_err(|e| e.to_json())?;
            let changing = on || off || level.is_some() || groups.is_some() || app_only || direct;
            if !changing {
                return Ok(a.to_json());
            }
            if on || off {
                a.enabled = on;
            }
            if let Some(l) = level {
                a.level = mk_commands::ai_access::Level::parse(&l)
                    .ok_or_else(|| error("invalid_args", format!("unknown level {l:?}")))?;
            }
            if let Some(g) = groups {
                a.groups = parse_groups(&g)?;
            }
            if app_only || direct {
                a.app_only = app_only;
            }
            session
                .set_ai_access(&a)
                .map(|a| a.to_json())
                .map_err(|e| e.to_json())
        }
        Cmd::Accounts { path, all } => invoke(
            s,
            &path,
            "cli",
            "account.list",
            json!({ "include_archived": all }),
        ),
        Cmd::Balance {
            path,
            account,
            as_of,
            children,
        } => invoke(
            s,
            &path,
            "cli",
            "account.balance",
            json!({ "account": account, "as_of": as_of, "include_children": children }),
        ),
        Cmd::TrialBalance { path, as_of } => invoke(
            s,
            &path,
            "cli",
            "report.trial_balance",
            json!({ "as_of": as_of }),
        ),
        Cmd::Undo { path, actor } => invoke(s, &path, &actor, "history.undo", json!({})),
        Cmd::Redo { path, actor } => invoke(s, &path, &actor, "history.redo", json!({})),
        Cmd::Import {
            path,
            file,
            account,
            profile,
            dmy,
            actor,
        } => invoke(
            s,
            &path,
            &actor,
            "import.stage",
            json!({
                "path": file.display().to_string(),
                "account": account,
                "profile": profile,
                "date_order": if dmy { "dmy" } else { "mdy" },
            }),
        ),
        Cmd::Sync {
            path,
            connection,
            now,
        } => invoke(
            s,
            &path,
            "sync",
            "connect.sync",
            json!({ "connection": connection, "now": now }),
        ),
        Cmd::Watch {
            path,
            settle,
            once,
            feeds,
        } => run_watch(s, &path, settle, once, feeds),
        Cmd::Backup {
            path,
            reason,
            folder,
        } => {
            let mut store = s.store(&path)?;
            let mut registry = networked();
            registry.set_backup_env(backup_env(folder.as_deref()));
            registry
                .invoke(
                    &mut store,
                    &Actor("cli".to_owned()),
                    "book.backup",
                    json!({ "reason": reason }),
                )
                .map_err(|e| e.to_json())
        }
        Cmd::Backups {
            action: Some(BackupsCmd::Protect { path, folder }),
            ..
        } => {
            let mut session = s.session(&path, "cli")?;
            session.set_backup_env(backup_env(folder.as_deref()));
            session
                .invoke("book.protect_backups", json!({}))
                .map_err(|e| e.to_json())
        }
        Cmd::Backups {
            action: None,
            path,
            folder,
        } => {
            let path = path.ok_or_else(|| error("invalid_args", "give a book file"))?;
            let env = backup_env(folder.as_deref());
            Ok(Value::Array(
                mk_commands::backups::list(&path, &env)
                    .iter()
                    .map(|e| e.to_json(&env))
                    .collect(),
            ))
        }
        Cmd::History { path, limit } => {
            invoke(s, &path, "cli", "history.list", json!({ "limit": limit }))
        }
    }
}

/// `--groups delete,settings`: permission group names.
fn parse_groups(
    text: &str,
) -> Result<std::collections::BTreeSet<mk_commands::ai_access::Group>, Value> {
    text.split(',')
        .map(str::trim)
        .filter(|g| !g.is_empty() && *g != "none")
        .map(|g| {
            mk_commands::ai_access::Group::parse(g).ok_or_else(|| {
                error(
                    "invalid_args",
                    format!("unknown group {g:?}: use delete, settings, files, connections"),
                )
            })
        })
        .collect()
}

fn run_watch(
    secrets: &Secrets,
    path: &Path,
    settle: u64,
    once: bool,
    feeds: bool,
) -> Result<Value, Value> {
    let registry = networked();
    let mut store = secrets.store(path)?;
    let mut watcher = watch::Watcher::new(std::time::Duration::from_secs(settle));
    let mut all = Vec::new();
    let mut last_feeds: Option<std::time::Instant> = None;
    loop {
        // Feeds: checked every 15 minutes; connect.sync applies the interval setting.
        if feeds && last_feeds.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(900)) {
            last_feeds = Some(std::time::Instant::now());
            let v = match registry.invoke(
                &mut store,
                &Actor("sync".into()),
                "connect.sync",
                json!({ "scheduled": true }),
            ) {
                Ok(v) => json!({ "feeds": v }),
                Err(e) => json!({ "feeds": e.to_json() }),
            };
            eprintln!("{}", mk_core::text::terminal_safe(&v.to_string()));
            all.push(v);
        }
        let outcomes = watcher
            .pass(&registry, &mut store)
            .map_err(|e| error("watch_error", e))?;
        for o in outcomes {
            let v = match o {
                watch::Outcome::Staged {
                    file,
                    batch,
                    committed,
                } => {
                    json!({"staged": file.display().to_string(), "batch": batch, "committed": committed})
                }
                watch::Outcome::Failed { file, error } => {
                    json!({"failed": file.display().to_string(), "error": error})
                }
                watch::Outcome::Skipped { file, error } => {
                    json!({"skipped": file.display().to_string(), "error": error})
                }
            };
            // Progress goes to stderr; stdout carries the final JSON only.
            eprintln!("{}", mk_core::text::terminal_safe(&v.to_string()));
            all.push(v);
        }
        if once && watcher_idle(&watcher) {
            return Ok(Value::Array(all));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// With `--once`, stop after the first pass that found nothing still settling.
fn watcher_idle(w: &watch::Watcher) -> bool {
    w.pending() == 0
}

fn error(code: &str, e: impl std::fmt::Display) -> Value {
    json!({ "error": { "code": code, "message": e.to_string() } })
}

/// JSON for the terminal: text from the books is filtered so no control or
/// bidirectional formatting character reaches it (input-safety.md R6).
fn pretty(v: &Value) -> String {
    mk_core::text::terminal_safe(&serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// input-safety.md AT2: the CLI prints no escape or bidirectional characters.
    #[test]
    fn input_safety_at2_cli_output_has_no_escape_characters() {
        let out = pretty(&serde_json::json!({"payee": "Shop\u{1b}[2J\u{9b}31m\u{202E}x"}));
        assert!(!out.contains(['\u{1b}', '\u{9b}', '\u{202E}']), "{out}");
        assert!(out.contains("Shop"));
    }
}
