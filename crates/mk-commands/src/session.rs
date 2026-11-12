//! A book opened for one front end: the registry, the store and the acting user.
//! Front ends that must not see the storage layer (the desktop UI) use this instead
//! of holding a `Store` themselves.

use std::path::Path;

use mk_core::CommodityCode;
use mk_store::{BookKind, KdfParams, Secret, Store};
use serde_json::Value;

use crate::ai_access::{self, AiAccess};
use crate::{Actor, CommandError, CommandResult, CommandSpec, FileReader, Registry};

/// An open book plus the registry and actor that act on it.
pub struct Session {
    registry: Registry,
    store: Store,
    actor: Actor,
    /// Key-derivation parameters for passwords set in this session.
    kdf: KdfParams,
}

/// Whether the file at `path` is a plain or an encrypted book.
pub fn book_kind(path: &Path) -> CommandResult<BookKind> {
    Ok(mk_store::book_kind(path)?)
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("actor", &self.actor)
            .finish()
    }
}

impl Session {
    /// Create a new book file.
    pub fn create(path: &Path, name: &str, currency: &str, actor: &str) -> CommandResult<Self> {
        Self::create_with_preset(path, name, currency, actor, None)
    }

    /// Create a new book file, optionally from a starter chart (chart-presets.md).
    /// An unknown preset is refused before any file is written.
    pub fn create_with_preset(
        path: &Path,
        name: &str,
        currency: &str,
        actor: &str,
        preset: Option<&str>,
    ) -> CommandResult<Self> {
        let code = CommodityCode::new(currency.trim())?;
        if let Some(p) = preset {
            crate::chart::check_preset(p)?;
        }
        let mut s = Self::wrap(Store::create(path, name, &code)?, actor);
        if let Some(p) = preset {
            s.apply_preset_or_remove(path, p)?;
        }
        Ok(s)
    }

    /// Apply a preset to the book just created; on failure the new file is removed.
    fn apply_preset_or_remove(&mut self, path: &Path, preset: &str) -> CommandResult<()> {
        if let Err(e) = self.apply_preset(preset) {
            for suffix in ["", "-wal", "-shm"] {
                let mut p = path.as_os_str().to_owned();
                p.push(suffix);
                let _ = std::fs::remove_file(p);
            }
            return Err(e);
        }
        Ok(())
    }

    /// The preset part of book creation: one audit entry (`book.create`), not an
    /// undo step of its own.
    fn apply_preset(&mut self, preset: &str) -> CommandResult<()> {
        static SPEC: crate::CommandSpec = crate::CommandSpec {
            name: "book.create",
            summary: "Create a book from a starter chart",
            module: crate::modules::CORE,
            mutates: true,
            args: crate::args_schema::<crate::builtin::NoArgs>,
        };
        let args = serde_json::json!({ "preset": preset });
        self.registry.execute(
            &mut self.store,
            &self.actor,
            &SPEC,
            args,
            None,
            &|ctx, a| {
                let p = a.get("preset").and_then(Value::as_str).unwrap_or_default();
                crate::chart::apply_at_creation(ctx, p)
            },
        )?;
        Ok(())
    }

    /// Open an existing book file (a plain book; an encrypted one fails with
    /// `password_required`). A book whose format must be updated is backed up first
    /// to the app's backup folder (see [`Session::open_with`]).
    pub fn open(path: &Path, actor: &str) -> CommandResult<Self> {
        Self::open_with(path, actor, None)
    }

    /// Open a book; the secret is used when the book is encrypted. When its format
    /// must be updated, the mandatory `before-update` backup is made first with the
    /// app's backup settings ([`crate::backups::open_book`]); if that fails the book
    /// is unchanged and opening fails with `backup_failed`.
    pub fn open_with(path: &Path, actor: &str, secret: Option<&Secret>) -> CommandResult<Self> {
        let env = crate::backups::BackupEnv::from_app_config();
        let (store, _) = crate::backups::open_book(path, secret, &env)?;
        Ok(Self::wrap(store, actor))
    }

    /// Open a book the way the app and the command line do (backups.md): when its
    /// format must be updated, a `before-update` backup is made first with `env`;
    /// if that fails the book is not updated, its file is unchanged and opening
    /// fails with `backup_failed`. The session's backup commands then use `env`.
    /// Returns the backup made, if one was.
    pub fn open_backed_up(
        path: &Path,
        actor: &str,
        secret: Option<&Secret>,
        env: &crate::backups::BackupEnv,
    ) -> CommandResult<(Self, Option<crate::backups::Made>)> {
        let (store, made) = crate::backups::open_book(path, secret, env)?;
        let mut s = Self::wrap(store, actor);
        s.set_backup_env(env.clone());
        Ok((s, made))
    }

    /// The backup settings this session's commands use, if set.
    pub fn backup_env(&self) -> Option<&crate::backups::BackupEnv> {
        self.registry.backup_env()
    }

    /// Let this session's backup commands run with `env`.
    pub fn set_backup_env(&mut self, env: crate::backups::BackupEnv) {
        self.registry.set_backup_env(env);
    }

    /// What a backup of the open book is copied from, and the change stamp it holds
    /// (pass the stamp to [`Session::mark_backed_up`] once the backup succeeded).
    pub fn backup_capture(&mut self) -> CommandResult<(mk_store::BackupSource, (i64, i64))> {
        let stamp = self.store.change_stamp()?;
        Ok((self.store.backup_source()?, stamp))
    }

    pub fn mark_backed_up(&mut self, stamp: (i64, i64)) {
        self.store.mark_backed_up(stamp);
    }

    /// Whether anything changed since the last backup (or since the book was opened).
    pub fn changed_since_backup(&mut self) -> bool {
        self.store.changed_since_backup()
    }

    /// Create a new encrypted book. Returns the recovery key (display form) when one
    /// was asked for. The creation is recorded as `book.encrypted`.
    pub fn create_encrypted(
        path: &Path,
        name: &str,
        currency: &str,
        actor: &str,
        password: &str,
        recovery: bool,
        kdf: KdfParams,
    ) -> CommandResult<(Self, Option<String>)> {
        Self::create_encrypted_with_preset(
            path, name, currency, actor, password, recovery, kdf, None,
        )
    }

    /// [`Session::create_encrypted`] with an optional starter chart.
    #[allow(clippy::too_many_arguments)]
    pub fn create_encrypted_with_preset(
        path: &Path,
        name: &str,
        currency: &str,
        actor: &str,
        password: &str,
        recovery: bool,
        kdf: KdfParams,
        preset: Option<&str>,
    ) -> CommandResult<(Self, Option<String>)> {
        let code = CommodityCode::new(currency.trim())?;
        if let Some(p) = preset {
            crate::chart::check_preset(p)?;
        }
        let (store, key) = Store::create_encrypted(path, name, &code, password, recovery, kdf)?;
        let mut s = Self::wrap(store, actor);
        s.kdf = kdf;
        s.record(
            "book.encrypted",
            serde_json::json!({"recovery_key": recovery}),
        )?;
        if let Some(p) = preset {
            s.apply_preset_or_remove(path, p)?;
        }
        Ok((s, key))
    }

    /// Use these key-derivation parameters for passwords set from now on (tests use
    /// cheap ones; the default is [`KdfParams::DEFAULT`]).
    pub fn set_kdf(&mut self, kdf: KdfParams) {
        self.kdf = kdf;
    }

    fn record(&mut self, command: &str, args: Value) -> CommandResult<()> {
        let actor = self.actor.0.clone();
        let command = command.to_owned();
        self.store.write(|tx| -> CommandResult<()> {
            tx.append_audit_record(&mk_store::AuditRecord {
                actor: &actor,
                command: &command,
                args,
                result: serde_json::json!({}),
                before: None,
                after: None,
                undoes: None,
                redoes: None,
            })?;
            Ok(())
        })
    }

    pub fn is_encrypted(&self) -> bool {
        self.store.is_encrypted()
    }

    pub fn has_recovery_key(&self) -> bool {
        self.store.has_recovery_key()
    }

    /// Encrypt the open plain book in place; returns the recovery key once.
    pub fn encrypt(&mut self, password: &str, recovery: bool) -> CommandResult<Option<String>> {
        let actor = self.actor.0.clone();
        Ok(self.store.encrypt(password, recovery, self.kdf, &actor)?)
    }

    pub fn change_password(&mut self, current: &Secret, new: &str) -> CommandResult<()> {
        let actor = self.actor.0.clone();
        Ok(self.store.change_password(current, new, self.kdf, &actor)?)
    }

    /// Replace (or add) the recovery key; returns the new key once.
    pub fn new_recovery_key(&mut self, current: &Secret) -> CommandResult<String> {
        let actor = self.actor.0.clone();
        Ok(self.store.new_recovery_key(current, &actor)?)
    }

    pub fn remove_recovery_key(&mut self, current: &Secret) -> CommandResult<()> {
        let actor = self.actor.0.clone();
        Ok(self.store.remove_recovery_key(current, &actor)?)
    }

    /// Write a plain copy of the book to another path, never replacing a file (a
    /// name taken meanwhile gets the next free name); returns where the copy is.
    pub fn save_unencrypted_copy(&mut self, path: &Path) -> CommandResult<std::path::PathBuf> {
        Ok(self.store.save_unencrypted_copy(path)?)
    }

    /// Turn the encrypted book back into a plain one in place.
    pub fn remove_encryption(&mut self, current: &Secret) -> CommandResult<()> {
        let actor = self.actor.0.clone();
        Ok(self.store.remove_encryption(current, &actor)?)
    }

    /// Check a password or recovery key against the open encrypted book.
    pub fn verify_secret(&self, current: &Secret) -> CommandResult<()> {
        Ok(self.store.verify(current)?)
    }

    /// Run `f` with the registry and store (front ends that drive a watcher pass on
    /// the session's own book, e.g. an encrypted one that cannot be opened twice).
    pub fn with_store<T>(&mut self, f: impl FnOnce(&Registry, &mut Store) -> T) -> T {
        f(&self.registry, &mut self.store)
    }

    /// The book file's path.
    pub fn path(&self) -> Option<&Path> {
        self.store.path()
    }

    /// The open encrypted book's file as it is on disk (tests; see
    /// [`mk_store::Store::file_bytes_for_tests`]).
    pub fn file_bytes_for_tests(&self) -> CommandResult<Vec<u8>> {
        Ok(self.store.file_bytes_for_tests()?)
    }

    /// Make the next save of an encrypted book fail (tests).
    pub fn fail_next_save_for_tests(&mut self) {
        self.store.fail_next_save_for_tests();
    }

    /// A book held in memory (tests and previews).
    pub fn in_memory(name: &str, currency: &str, actor: &str) -> CommandResult<Self> {
        let code = CommodityCode::new(currency.trim())?;
        Ok(Self::wrap(Store::in_memory(name, &code)?, actor))
    }

    /// A book held in memory, created from a starter chart (tests and previews).
    pub fn in_memory_with_preset(
        name: &str,
        currency: &str,
        actor: &str,
        preset: &str,
    ) -> CommandResult<Self> {
        crate::chart::check_preset(preset)?;
        let mut s = Self::in_memory(name, currency, actor)?;
        s.apply_preset(preset)?;
        Ok(s)
    }

    fn wrap(store: Store, actor: &str) -> Self {
        Self {
            registry: Registry::with_builtins(),
            store,
            actor: Actor(actor.to_owned()),
            kdf: KdfParams::DEFAULT,
        }
    }

    /// Give bank-feed commands network and credential access.
    pub fn set_connector(&mut self, c: crate::connect::Connector) {
        self.registry.set_connector(c);
    }

    pub fn connector_mut(&mut self) -> Option<&mut crate::connect::Connector> {
        self.registry.connector_mut()
    }

    /// Start a network command (see [`crate::connect::NetRunner`]).
    pub fn net_start(
        &mut self,
        command: &str,
        args: Value,
    ) -> CommandResult<crate::connect::NetStep> {
        self.registry
            .net_start(&mut self.store, &self.actor, command, args)
    }

    /// Continue a network command with its answers.
    pub fn net_resume(
        &mut self,
        fetched: crate::connect::Fetched,
    ) -> CommandResult<crate::connect::NetStep> {
        self.registry
            .net_resume(&mut self.store, &self.actor, fetched)
    }

    /// Run a registry command as this session's actor.
    pub fn invoke(&mut self, command: &str, args: Value) -> CommandResult<Value> {
        self.registry
            .invoke(&mut self.store, &self.actor, command, args)
    }

    /// Run a command with a file reader (for commands that accept a path).
    pub fn invoke_with(
        &mut self,
        command: &str,
        args: Value,
        files: &dyn FileReader,
    ) -> CommandResult<Value> {
        self.registry
            .invoke_with(&mut self.store, &self.actor, command, args, Some(files))
    }

    /// Hold this session's write lock while `f` runs (tests of the busy path).
    pub fn hold_write_lock_for_tests(&mut self, f: impl FnOnce()) -> CommandResult<()> {
        self.store.write(|_| -> CommandResult<()> {
            f();
            Ok(())
        })
    }

    pub fn actor(&self) -> &str {
        &self.actor.0
    }

    // ----- AI access (mcp-server.md revision 3) -------------------------------------

    /// The book's AI access settings (defaults when unset: access off).
    pub fn ai_access(&self) -> CommandResult<AiAccess> {
        ai_access::read(&self.store)
    }

    /// Change the AI access settings (a session operation: never a registry command,
    /// never a tool, never on the control channel). Records
    /// `book.ai_access_changed`.
    pub fn set_ai_access(&mut self, new: &AiAccess) -> CommandResult<AiAccess> {
        let actor = self.actor.0.clone();
        ai_access::set(&mut self.store, &actor, new)
    }

    /// The commands an AI assistant may use in this book now (the exposed set).
    pub fn ai_exposed(&self) -> CommandResult<Vec<CommandSpec>> {
        let access = ai_access::read(&self.store)?;
        Ok(ai_access::exposed(&self.registry, &self.store, &access)?
            .into_iter()
            .cloned()
            .collect())
    }

    /// Run a command for an AI assistant (`actor` = `mcp:<client>`), only if it is in
    /// the exposed set. `files` reads paths for the `files` group's commands.
    pub fn invoke_exposed(
        &mut self,
        actor: &str,
        command: &str,
        args: Value,
        files: Option<&dyn FileReader>,
    ) -> Result<Value, ai_access::ExposedError> {
        ai_access::invoke_exposed(
            &self.registry,
            &mut self.store,
            &Actor(actor.to_owned()),
            command,
            args,
            files,
        )
    }

    /// Record that the owner allowed an assistant (`ai.session_allowed`).
    pub fn record_ai_session_allowed(&mut self, client: &str) -> CommandResult<()> {
        let actor = self.actor.0.clone();
        let access = ai_access::read(&self.store)?;
        ai_access::record_session_allowed(&mut self.store, &actor, client, &access)
    }

    /// The canonical path of the book file, for the control channel.
    pub fn canonical_path(&self) -> Option<std::path::PathBuf> {
        self.store
            .path()
            .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()))
    }
}

/// `CommandError` is `Send`, so sessions can live on a worker thread.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Session>();
    assert_send::<CommandError>();
};
