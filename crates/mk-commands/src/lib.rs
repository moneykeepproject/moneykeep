//! MoneyKeep command registry.
//!
//! Every user-visible action is a [`Command`]. The GUI, CLI, JSON control channel and
//! MCP server all go through [`Registry::invoke`], so they behave identically:
//! module gating, argument validation and audit logging happen here, once.

mod accounts;
pub mod ai_access;
pub mod backups;
mod balances;
mod book_profile;
mod builtin;
mod changes;
pub mod chart;
pub mod connect;
mod corporate;
mod dashboards;
mod definitions;
mod dimension_reports;
mod dimensions;
mod expense_pdf;
mod expenses;
mod history;
mod iif;
mod import;
mod inv_import;
mod inv_kinds;
mod inv_reports;
mod investments;
mod mileage;
mod mileage_pdf;
mod module_settings;
pub mod ofx_conn;
mod opening;
mod pdf;
mod performance;
pub mod processors;
mod purchase_reports;
mod purchases;
mod sales;
mod sales_reports;
pub mod snap_conn;
pub use import::auto_committable;
mod budgets;
mod capture;
pub mod capture_setup;
pub mod capture_start_over;
pub mod capture_wifi;
pub mod modules;
mod profiles;
mod projects;
mod reconcile;
mod register;
mod reports;
mod rules;
mod schedules;
mod session;
mod statements;
mod subscriptions;
mod templates;
mod txns;

pub use mk_store::{
    BackupSource, BookKind, KdfParams, Secret, create_old_book_for_tests, latest_schema_version,
    prev_path,
};
pub use session::{Session, book_kind};

use std::cell::{Cell, RefCell};

use mk_ledger::LedgerError;
use mk_store::{AuditRecord, Store, StoreError, WriteTx};
use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde::de::DeserializeOwned;
use serde::{Serialize, Serializer};
use serde_json::{Value, json};

use changes::ChangeLog;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("unknown command {0:?}")]
    UnknownCommand(String),

    #[error("command {command:?} needs module {module:?}, which is disabled for this book")]
    ModuleDisabled { command: String, module: String },

    #[error("invalid arguments: {0}")]
    InvalidArgs(String),

    #[error("{0}")]
    Rejected(String),

    #[error("not found: {0}")]
    NotFound(String),

    /// The change clashes with the current state (e.g. an occurrence already entered).
    #[error("{0}")]
    Conflict(String),

    /// A refusal with its own stable code (e.g. `insufficient_shares`).
    #[error("{message}")]
    Coded { code: &'static str, message: String },

    /// The book breaks an invariant that validation should have made impossible.
    #[error("ledger integrity error: {0}")]
    Integrity(String),

    #[error(transparent)]
    Store(#[from] StoreError),
}

impl CommandError {
    /// Stable machine-readable code for CLI/MCP clients.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownCommand(_) => "unknown_command",
            Self::ModuleDisabled { .. } => "module_disabled",
            Self::InvalidArgs(_) => "invalid_args",
            Self::Rejected(_) => "rejected",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Coded { code, .. } => code,
            Self::Integrity(_) => "integrity_error",
            Self::Store(StoreError::Busy) => "busy",
            Self::Store(StoreError::Io { .. }) => "io",
            Self::Store(StoreError::PasswordRequired) => "password_required",
            Self::Store(StoreError::WrongPassword) => "wrong_password",
            Self::Store(StoreError::BookDamaged(_)) => "book_damaged",
            Self::Store(StoreError::BookInUse) => "book_in_use",
            Self::Store(StoreError::UnusualKdf) => "unusual_kdf",
            Self::Store(StoreError::NotABook) => "not_a_book",
            Self::Store(StoreError::InvalidPassword(_) | StoreError::NotEncrypted) => {
                "invalid_args"
            }
            Self::Store(_) => "store_error",
        }
    }

    /// The `{"error":{"code","message"}}` body used by the CLI and the MCP server.
    pub fn to_json(&self) -> Value {
        json!({ "error": { "code": self.code(), "message": self.to_string() } })
    }
}

impl From<serde_json::Error> for CommandError {
    fn from(e: serde_json::Error) -> Self {
        Self::InvalidArgs(e.to_string())
    }
}

impl From<LedgerError> for CommandError {
    fn from(e: LedgerError) -> Self {
        match e {
            LedgerError::Invalid(m) => Self::InvalidArgs(m),
            LedgerError::Rejected(m) => Self::Rejected(m),
            LedgerError::Overflow => Self::Rejected("arithmetic overflow".into()),
        }
    }
}

impl From<mk_core::CoreError> for CommandError {
    fn from(e: mk_core::CoreError) -> Self {
        Self::InvalidArgs(e.to_string())
    }
}

pub type CommandResult<T> = Result<T, CommandError>;

/// Commands never offered over MCP: they handle a credential (connectivity.md).
/// Receipt upload and download move whole files and are for local front ends only
/// (expenses.md). Phone capture's server token and pairing codes stay local
/// (mobile-capture.md).
/// Backups (backups.md) write files on this computer: making one and restoring one
/// are for local front ends only (listing them is a read).
pub const NOT_OVER_MCP: &[&str] = &[
    "book.backup",
    "book.delete_unprotected_backups",
    "book.protect_backups",
    "book.restore_copy",
    "capture.check_access",
    "capture.forget_cloudflare_token",
    "capture.pair_code",
    "capture.reinstall_server",
    "capture.remove_from_computer",
    "capture.remove_old_copy",
    "capture.set_server",
    "capture.setup_apply",
    "capture.setup_check_token",
    "capture.setup_remove",
    "capture.setup_repair",
    "capture.start_over",
    "capture.start_over_preview",
    "capture.start_server",
    "capture.update_server",
    "capture.wifi_apply",
    "capture.wifi_change_address",
    "capture.wifi_firewall_fix",
    "capture.wifi_pair_code",
    "capture.wifi_remove",
    "capture.wifi_set_pin",
    "connect.add",
    "connect.disconnect",
    "connect.answer_challenge",
    "receipt.add",
    "receipt.get",
];

/// Static description of a command, used for help text, menus and MCP tool listings.
#[derive(Debug, Clone, Serialize)]
pub struct CommandSpec {
    /// Dotted name, e.g. `module.enable`.
    pub name: &'static str,
    pub summary: &'static str,
    /// Module the command belongs to; `core` commands are always available.
    pub module: &'static str,
    /// Whether the command changes the book (and is therefore audit-logged).
    pub mutates: bool,
    /// JSON Schema of the command's arguments, generated from the argument type.
    #[serde(serialize_with = "serialize_schema")]
    pub args: fn() -> Value,
}

impl CommandSpec {
    /// The JSON Schema (2020-12) of the command's arguments.
    pub fn args_schema(&self) -> Value {
        (self.args)()
    }
}

fn serialize_schema<S: Serializer>(f: &fn() -> Value, s: S) -> Result<S::Ok, S::Error> {
    f().serialize(s)
}

/// JSON Schema (2020-12, subschemas inlined, no `$schema`/`title`) for an argument type.
pub fn args_schema<T: JsonSchema>() -> Value {
    let generator = SchemaSettings::draft2020_12()
        .with(|s| {
            s.inline_subschemas = true;
            s.meta_schema = None;
        })
        .into_generator();
    let mut v: Value = generator.into_root_schema_for::<T>().into();
    if let Some(o) = v.as_object_mut() {
        o.remove("title");
        o.remove("description");
    }
    v
}

/// Reads files for commands that accept a path (`import.stage`). Front ends that can
/// touch the file system supply one; without it, such commands need inline content.
pub trait FileReader {
    fn read(&self, path: &str) -> Result<Vec<u8>, String>;
}

/// Who is acting, recorded in the audit log.
#[derive(Debug, Clone)]
pub struct Actor(pub String);

/// Undo/redo links for the audit entry being written.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Links {
    pub undoes: Option<i64>,
    pub redoes: Option<i64>,
}

/// Context a command runs in.
#[derive(Debug)]
pub struct Ctx<'a> {
    pub tx: &'a WriteTx<'a>,
    pub actor: &'a Actor,
    pub(crate) changes: RefCell<ChangeLog>,
    pub(crate) links: Cell<Links>,
    /// Set while applying an undo: removed splits do not count as account usage,
    /// because the undo restores the state in which they never existed.
    pub(crate) suppress_usage: Cell<bool>,
    /// Set while undo/redo restores entities to a state they already had: the
    /// multi-currency gate does not apply (a disabled module hides data, it does not
    /// forbid restoring it). Every other rule still applies.
    pub(crate) restoring: Cell<bool>,
    /// Replacement for the arguments recorded in the audit entry (e.g. a file's digest
    /// instead of its content).
    pub(crate) audit_args: RefCell<Option<Value>>,
    /// Replacement for the result recorded in the audit entry (e.g. a PDF's size
    /// instead of its bytes).
    pub(crate) audit_result: RefCell<Option<Value>>,
    pub(crate) files: Option<&'a dyn FileReader>,
    /// Set while undoing a reconciliation: its splits leave the reconciled state and
    /// its adjustment transaction (with a reconciled split) is removed.
    pub(crate) unreconciling: Cell<bool>,
    /// Records the audit entry under this actor instead of the caller's (the
    /// scheduler enters due occurrences as `schedule`).
    pub(crate) actor_as: RefCell<Option<String>>,
    /// When set and the command changed nothing, no audit entry is written (a
    /// scheduler run that found nothing due leaves no trace).
    pub(crate) quiet_noop: Cell<bool>,
    /// Set while an automatic fund release is being written (no recursion).
    pub(crate) syncing: Cell<bool>,
    /// Network and credential access for bank feeds (connectivity.md); `None` in front
    /// ends that cannot reach the network.
    pub(crate) connector: Option<&'a connect::Connector>,
    /// The book is password protected (its file is encrypted).
    pub(crate) encrypted: bool,
}

impl std::fmt::Debug for dyn FileReader + '_ {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileReader")
    }
}

impl<'a> Ctx<'a> {
    fn new(tx: &'a WriteTx<'a>, actor: &'a Actor) -> Self {
        Self {
            tx,
            actor,
            changes: RefCell::new(ChangeLog::default()),
            links: Cell::new(Links::default()),
            suppress_usage: Cell::new(false),
            restoring: Cell::new(false),
            audit_args: RefCell::new(None),
            audit_result: RefCell::new(None),
            files: None,
            unreconciling: Cell::new(false),
            actor_as: RefCell::new(None),
            syncing: Cell::new(false),
            quiet_noop: Cell::new(false),
            connector: None,
            encrypted: false,
        }
    }
}

pub trait Command: Send + Sync {
    fn spec(&self) -> &CommandSpec;
    fn run(&self, ctx: &Ctx<'_>, args: Value) -> CommandResult<Value>;
}

/// A command implemented by a function over typed arguments. The argument type both
/// parses the JSON and generates the schema, so the two cannot drift.
struct FnCommand<A> {
    spec: CommandSpec,
    run: fn(&Ctx<'_>, A) -> CommandResult<Value>,
}

impl<A: DeserializeOwned + 'static> Command for FnCommand<A> {
    fn spec(&self) -> &CommandSpec {
        &self.spec
    }
    fn run(&self, ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
        (self.run)(ctx, parse_args(args)?)
    }
}

pub(crate) fn parse_args<T: DeserializeOwned>(args: Value) -> CommandResult<T> {
    let args = if args.is_null() { json!({}) } else { args };
    Ok(serde_json::from_value(args)?)
}

/// The set of available commands.
pub struct Registry {
    commands: Vec<Box<dyn Command>>,
    connector: Option<connect::Connector>,
    /// Backup settings and clock (backups.md); `None` where front ends do not back up.
    backup: Option<backups::BackupEnv>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("commands", &self.commands.len())
            .finish()
    }
}

impl Registry {
    /// Registry with all built-in commands.
    pub fn with_builtins() -> Self {
        let mut r = Self {
            commands: Vec::new(),
            connector: None,
            backup: None,
        };
        builtin::register(&mut r);
        backups::register(&mut r);
        book_profile::register(&mut r);
        accounts::register(&mut r);
        txns::register(&mut r);
        balances::register(&mut r);
        history::register(&mut r);
        import::register(&mut r);
        rules::register(&mut r);
        profiles::register(&mut r);
        definitions::register(&mut r);
        corporate::register(&mut r);
        performance::register(&mut r);
        reconcile::register(&mut r);
        register::register(&mut r);
        schedules::register(&mut r);
        budgets::register(&mut r);
        reports::register(&mut r);
        investments::register(&mut r);
        inv_import::register(&mut r);
        module_settings::register(&mut r);
        sales::register(&mut r);
        sales_reports::register(&mut r);
        purchases::register(&mut r);
        dimensions::register(&mut r);
        dimension_reports::register(&mut r);
        iif::register(&mut r);
        templates::register(&mut r);
        statements::register(&mut r);
        purchase_reports::register(&mut r);
        connect::register(&mut r);
        processors::register(&mut r);
        ofx_conn::register(&mut r);
        expenses::register(&mut r);
        mileage::register(&mut r);
        subscriptions::register(&mut r);
        projects::register(&mut r);
        capture::register(&mut r);
        capture_setup::register(&mut r);
        capture_wifi::register(&mut r);
        dashboards::register(&mut r);
        chart::register(&mut r);
        opening::register(&mut r);
        r
    }

    /// Give bank-feed commands their network and credential access.
    pub fn set_connector(&mut self, c: connect::Connector) {
        self.connector = Some(c);
    }

    pub fn connector_mut(&mut self) -> Option<&mut connect::Connector> {
        self.connector.as_mut()
    }

    /// Let the backup commands run, with these settings (backups.md).
    pub fn set_backup_env(&mut self, env: backups::BackupEnv) {
        self.backup = Some(env);
    }

    pub fn backup_env(&self) -> Option<&backups::BackupEnv> {
        self.backup.as_ref()
    }

    pub fn register(&mut self, command: Box<dyn Command>) {
        self.commands.push(command);
    }

    /// Register a function command in the `core` module.
    pub(crate) fn add<A: DeserializeOwned + JsonSchema + 'static>(
        &mut self,
        name: &'static str,
        summary: &'static str,
        mutates: bool,
        run: fn(&Ctx<'_>, A) -> CommandResult<Value>,
    ) {
        self.add_in(modules::CORE, name, summary, mutates, run);
    }

    /// Register a function command in `module`.
    pub(crate) fn add_in<A: DeserializeOwned + JsonSchema + 'static>(
        &mut self,
        module: &'static str,
        name: &'static str,
        summary: &'static str,
        mutates: bool,
        run: fn(&Ctx<'_>, A) -> CommandResult<Value>,
    ) {
        self.register(Box::new(FnCommand {
            spec: CommandSpec {
                name,
                summary,
                module,
                mutates,
                args: args_schema::<A>,
            },
            run,
        }));
    }

    /// All command specs, sorted by name.
    pub fn specs(&self) -> Vec<&CommandSpec> {
        let mut specs: Vec<_> = self.commands.iter().map(|c| c.spec()).collect();
        specs.sort_by_key(|s| s.name);
        specs
    }

    /// Spec of one command by name.
    pub fn spec(&self, name: &str) -> Option<&CommandSpec> {
        self.commands
            .iter()
            .map(|c| c.spec())
            .find(|s| s.name == name)
    }

    /// Command specs available in this book (disabled modules filtered out).
    pub fn available_specs(&self, store: &Store) -> CommandResult<Vec<&CommandSpec>> {
        let mut out = Vec::new();
        for spec in self.specs() {
            if is_module_enabled_store(store, spec.module)? {
                out.push(spec);
            }
        }
        Ok(out)
    }

    /// Run a command by name. Mutating commands and their audit entry commit together;
    /// non-mutating commands run in a read transaction that is always rolled back.
    pub fn invoke(
        &self,
        store: &mut Store,
        actor: &Actor,
        name: &str,
        args: Value,
    ) -> CommandResult<Value> {
        self.invoke_with(store, actor, name, args, None)
    }

    /// Like [`Registry::invoke`], with a file reader for commands that take a path.
    pub fn invoke_with(
        &self,
        store: &mut Store,
        actor: &Actor,
        name: &str,
        args: Value,
        files: Option<&dyn FileReader>,
    ) -> CommandResult<Value> {
        // Bank feeds: requests run with no transaction open (connectivity.md rev. 2).
        if connect::is_network(name) {
            return self.run_network(store, actor, name, args);
        }
        // Backups are files, not changes to the book: no transaction, no audit entry.
        if backups::is_backup(name) {
            return backups::run(self, store, name, args);
        }
        let command = self
            .commands
            .iter()
            .find(|c| c.spec().name == name)
            .ok_or_else(|| CommandError::UnknownCommand(name.to_owned()))?;
        let spec = command.spec();
        self.execute(store, actor, spec, args, files, &|ctx, a| {
            command.run(ctx, a)
        })
    }

    /// Run `run` as command `spec` in one transaction (a write for mutating commands,
    /// with its audit entry), after the module check.
    pub(crate) fn execute(
        &self,
        store: &mut Store,
        actor: &Actor,
        spec: &CommandSpec,
        args: Value,
        files: Option<&dyn FileReader>,
        run: &dyn Fn(&Ctx<'_>, Value) -> CommandResult<Value>,
    ) -> CommandResult<Value> {
        let encrypted = store.is_encrypted();
        let connector = self.connector.as_ref();

        let body = |tx: &WriteTx<'_>| -> CommandResult<Value> {
            if !is_module_enabled(tx, spec.module)? {
                return Err(CommandError::ModuleDisabled {
                    command: spec.name.to_owned(),
                    module: spec.module.to_owned(),
                });
            }
            let mut ctx = Ctx::new(tx, actor);
            ctx.files = files;
            ctx.connector = connector;
            ctx.encrypted = encrypted;
            let result = run(&ctx, args.clone())?;
            let unchanged = ctx.changes.borrow().is_empty();
            if spec.mutates && !(unchanged && ctx.quiet_noop.get()) {
                let (before, after) = ctx.changes.borrow().to_audit();
                let links = ctx.links.get();
                let audit_args = ctx.audit_args.borrow_mut().take();
                let audit_result = ctx.audit_result.borrow_mut().take();
                let actor_name = ctx
                    .actor_as
                    .borrow()
                    .clone()
                    .unwrap_or_else(|| actor.0.clone());
                let seq = tx.append_audit_record(&AuditRecord {
                    actor: &actor_name,
                    command: spec.name,
                    args: audit_args.unwrap_or_else(|| args.clone()),
                    result: audit_result.unwrap_or_else(|| result.clone()),
                    before,
                    after,
                    undoes: links.undoes,
                    redoes: links.redoes,
                })?;
                tx.stamp_usage(seq)?;
            }
            Ok(result)
        };
        if spec.mutates {
            store.write(body)
        } else {
            store.read(body)
        }
    }
}

/// Every built-in command, for [`call`].
static BUILTINS: std::sync::OnceLock<Vec<Box<dyn Command>>> = std::sync::OnceLock::new();

/// Run a reading command by name inside the current command (dashboard blocks get
/// their data through the existing commands this way). Mutating commands are refused,
/// and module gating applies as for any other caller.
pub(crate) fn call(ctx: &Ctx<'_>, name: &str, args: Value) -> CommandResult<Value> {
    let commands = BUILTINS.get_or_init(|| Registry::with_builtins().commands);
    let command = commands
        .iter()
        .find(|c| c.spec().name == name)
        .ok_or_else(|| CommandError::UnknownCommand(name.to_owned()))?;
    let spec = command.spec();
    if spec.mutates || connect::is_network(name) {
        return Err(CommandError::Integrity(format!(
            "{name} changes the book and cannot be read from another command"
        )));
    }
    if !is_module_enabled(ctx.tx, spec.module)? {
        return Err(CommandError::ModuleDisabled {
            command: name.to_owned(),
            module: spec.module.to_owned(),
        });
    }
    command.run(ctx, args)
}

pub(crate) const ENABLED_KEY: &str = "enabled";

pub(crate) fn is_module_enabled(tx: &WriteTx<'_>, module: &str) -> CommandResult<bool> {
    if module == modules::CORE {
        return Ok(true);
    }
    // `a|b`: commands shared by modules (parties serve invoicing and bills).
    if module.contains('|') {
        for m in module.split('|') {
            if is_module_enabled(tx, m)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    Ok(tx.get_setting(module, ENABLED_KEY)? == Some(Value::Bool(true)))
}

fn is_module_enabled_store(store: &Store, module: &str) -> CommandResult<bool> {
    if module == modules::CORE {
        return Ok(true);
    }
    if module.contains('|') {
        for m in module.split('|') {
            if is_module_enabled_store(store, m)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    Ok(store.get_setting(module, ENABLED_KEY)? == Some(Value::Bool(true)))
}

#[cfg(test)]
mod tests;
