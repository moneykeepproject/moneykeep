//! MoneyKeep storage layer.
//!
//! A book is one SQLite file. All writes happen inside [`Store::write`], which wraps
//! one SQLite transaction, so a command and its audit-log entry commit together or
//! not at all.

pub mod backup;
pub mod crypt;
mod encrypted;
mod ledger;
mod migrations;
pub mod newfile;

use std::path::{Path, PathBuf};

pub use backup::{
    BackupKey, BackupSource, CheckedBackup, Unlock, backup_source_of_file, check_backup,
};
pub use crypt::{
    KdfParams, Secret, check_password_rules, format_recovery_key, parse_recovery_key, payload_nonce,
};
pub use encrypted::{BookKind, book_kind, prev_path};

use mk_core::{AccountId, AccountSubtype, AccountType, BookId, CommodityCode, Date};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use time::OffsetDateTime;
use time::macros::format_description;

pub use ledger::{AccountSplit, Doc, DocTable, PayeeUse, RegisterSplit, SplitRow, TxnFilter};

use migrations::{MIGRATIONS, latest_version};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(rusqlite::Error),

    #[error(
        "the book is locked by another writer (another MoneyKeep process has it open \
         for writing); try again when it has finished"
    )]
    Busy,

    #[error("stored data is invalid: {0}")]
    Corrupt(String),

    #[error("{0}")]
    Core(#[from] mk_core::CoreError),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("time formatting error: {0}")]
    Time(#[from] time::error::Format),

    #[error("a file already exists at {0}")]
    AlreadyExists(String),

    #[error("book file schema v{found} is newer than this build supports (v{supported})")]
    TooNew { found: i64, supported: i64 },

    #[error("the file is not a MoneyKeep book")]
    NotABook,

    #[error("this book is password protected; give its password or recovery key")]
    PasswordRequired,

    #[error("that password or recovery key does not open this book")]
    WrongPassword,

    /// A slot unwrapped but the payload failed authentication, or the file is
    /// truncated or of an unknown version. The text names `<file>.prev` if it exists.
    #[error("the book file is damaged or was changed outside MoneyKeep.{0}")]
    BookDamaged(String),

    #[error("the book is open in another MoneyKeep window or process")]
    BookInUse,

    /// The key-derivation settings in the file are outside what MoneyKeep writes
    /// (security-review-1 L4); refused before any key is derived.
    #[error("This book's password settings aren't ones MoneyKeep makes.")]
    UnusualKdf,

    #[error("{0}")]
    InvalidPassword(String),

    #[error("the book is not password protected")]
    NotEncrypted,

    /// Opening stopped before a format update because the step before it (the
    /// backup, backups.md) failed; the caller reports that step's own error.
    #[error("opening stopped before the book was updated")]
    StoppedBeforeUpdate,

    /// The file or its folder cannot be read or written (permissions, missing
    /// folder, disk full, another program blocking access).
    #[error("cannot {action} the book file at {path}: {message}")]
    Io {
        /// "create" or "open".
        action: &'static str,
        path: String,
        message: String,
    },
}

impl StoreError {
    /// Turn an SQLite failure to open, read or write the file itself into `Io`.
    fn file_level(self, action: &'static str, path: &Path) -> Self {
        use rusqlite::ErrorCode as C;
        let io = match &self {
            Self::Sqlite(e) => matches!(
                e.sqlite_error_code(),
                Some(
                    C::CannotOpen
                        | C::ReadOnly
                        | C::PermissionDenied
                        | C::SystemIoFailure
                        | C::DiskFull
                )
            ),
            _ => false,
        };
        if io {
            Self::Io {
                action,
                path: path.display().to_string(),
                message: self.to_string(),
            }
        } else {
            self
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        match e.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                Self::Busy
            }
            _ => Self::Sqlite(e),
        }
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

/// Summary of the book held in a file.
#[derive(Debug, Clone, Serialize)]
pub struct BookInfo {
    pub id: String,
    pub name: String,
    pub base_currency: String,
    pub created_at: String,
    pub closing_date: Option<String>,
    pub schema_version: i64,
}

/// One audit-log row.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub seq: i64,
    pub at: String,
    pub actor: String,
    pub command: String,
    pub args: serde_json::Value,
    pub result: serde_json::Value,
    /// Snapshots of the entities the command changed or deleted, as they were before
    /// (null for pure creations and for entries written before schema v3).
    pub before: Option<serde_json::Value>,
    /// Snapshots of the entities created or changed, as they are after (null for
    /// pure deletions).
    pub after: Option<serde_json::Value>,
    /// Sequence number of the entry this one undoes.
    pub undoes: Option<i64>,
    /// Sequence number of the undo entry this one re-applies (redo).
    pub redoes: Option<i64>,
    /// Latest entry whose `undoes` points at this one (derived, never stored here).
    pub undone_by: Option<i64>,
    /// True if an undo of this entry is in effect (it was undone and that undo has not
    /// been redone). Derived.
    pub undone: bool,
}

/// What to write to the audit log for one command.
#[derive(Debug, Clone, Default)]
pub struct AuditRecord<'a> {
    pub actor: &'a str,
    pub command: &'a str,
    pub args: serde_json::Value,
    pub result: serde_json::Value,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
    pub undoes: Option<i64>,
    pub redoes: Option<i64>,
}

/// An open book file.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    /// Key material and locked file of an encrypted book.
    enc: Option<encrypted::Encrypted>,
    /// The book file (None for an in-memory book).
    path: Option<PathBuf>,
    /// The change stamp the newest backup (or, before one is made, the opened
    /// book) holds (backups.md: back up on close only when something changed).
    backup_mark: Option<(i64, i64)>,
}

impl Store {
    /// Create a new book file. Fails if the path already exists.
    pub fn create(path: &Path, name: &str, base_currency: &CommodityCode) -> StoreResult<Self> {
        if path.exists() {
            return Err(StoreError::AlreadyExists(path.display().to_string()));
        }
        let made = || -> StoreResult<Self> {
            let conn = Connection::open(path)?;
            let mut store = Self::configure(conn)?;
            store.migrate()?;
            store.init_book(name, base_currency)?;
            store.path = Some(path.to_path_buf());
            Ok(store)
        };
        made().map_err(|e| {
            // Leave nothing half-made behind (best effort).
            for suffix in ["", "-wal", "-shm"] {
                let mut p = path.as_os_str().to_owned();
                p.push(suffix);
                let _ = std::fs::remove_file(p);
            }
            e.file_level("create", path)
        })
    }

    /// Open an existing plain book file, applying any pending migrations. An
    /// encrypted book needs [`Store::open_with`] and a secret.
    pub fn open(path: &Path) -> StoreResult<Self> {
        Self::open_with(path, None)
    }

    /// Open a book file; `secret` is needed (and only used) for an encrypted book.
    pub fn open_with(path: &Path, secret: Option<&Secret>) -> StoreResult<Self> {
        Self::open_inner(path, secret, None)
    }

    /// Open a book file, calling `before_update` (if any) with a backup source when a
    /// migration is pending, before the file is changed in any way.
    pub(crate) fn open_inner(
        path: &Path,
        secret: Option<&Secret>,
        mut before_update: backup::BeforeUpdate<'_>,
    ) -> StoreResult<Self> {
        match book_kind(path) {
            Ok(BookKind::Encrypted) => {
                let secret = secret.ok_or(StoreError::PasswordRequired)?;
                let mut store = Self::open_encrypted(path, secret, before_update)?;
                store.set_baseline();
                return Ok(store);
            }
            Ok(BookKind::Plain) => {}
            Err(StoreError::BookInUse) => return Err(StoreError::BookInUse),
            // An empty or unreadable file falls through to SQLite's own error.
            Err(StoreError::NotABook) if std::fs::metadata(path).is_ok_and(|m| m.len() > 0) => {
                return Err(StoreError::NotABook);
            }
            Err(_) => {}
        }
        let mut opened = || -> StoreResult<Self> {
            let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            // The backup before an update runs before anything (even the journal
            // mode) is written to the file.
            if let Some(hook) = before_update.as_mut() {
                let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
                if backup::pending(version) {
                    hook(&backup::plain_source(path))?;
                }
            }
            let mut store = Self::configure(conn)?;
            store.migrate()?;
            store.info()?;
            store.path = Some(path.to_path_buf());
            store.set_baseline();
            Ok(store)
        };
        opened().map_err(|e| e.file_level("open", path))
    }

    /// The opened book is the baseline for "changed since the last backup".
    fn set_baseline(&mut self) {
        self.backup_mark = self.change_stamp().ok();
    }

    /// In-memory book, for tests and previews.
    pub fn in_memory(name: &str, base_currency: &CommodityCode) -> StoreResult<Self> {
        let mut store = Self::configure(Connection::open_in_memory()?)?;
        store.migrate()?;
        store.init_book(name, base_currency)?;
        Ok(store)
    }

    /// Insert the book row and its one default account, `Opening Balances`.
    fn init_book(&mut self, name: &str, base_currency: &CommodityCode) -> StoreResult<()> {
        let tx = self.conn.transaction()?;
        let now = now_rfc3339()?;
        let opening = AccountId::new().to_string();
        tx.execute(
            "INSERT INTO book (id, name, base_currency, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![BookId::new().to_string(), name, base_currency.as_str(), now],
        )?;
        tx.execute(
            "INSERT INTO account (id, name, type, subtype, commodity, created_at, role)
             VALUES (?1, 'Opening Balances', ?2, ?3, ?4, ?5, 'opening_balances')",
            params![
                opening,
                AccountType::Equity.as_str(),
                AccountSubtype::OpeningBalances.as_str(),
                base_currency.as_str(),
                now
            ],
        )?;
        tx.execute("UPDATE book SET opening_account_id = ?1", params![opening])?;
        tx.commit()?;
        Ok(())
    }

    fn configure(conn: Connection) -> StoreResult<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // Wait briefly for another writer, then fail with `StoreError::Busy`.
        conn.busy_timeout(std::time::Duration::from_millis(500))?;
        Ok(Self {
            conn,
            enc: None,
            path: None,
            backup_mark: None,
        })
    }

    /// Apply pending migrations; true when any ran.
    fn migrate(&mut self) -> StoreResult<bool> {
        let current: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        let supported = latest_version();
        if current > supported {
            return Err(StoreError::TooNew {
                found: current,
                supported,
            });
        }
        for m in MIGRATIONS.iter().filter(|m| m.version > current) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(m.sql)?;
            if let Some(fix) = m.fix {
                fix(&tx)?;
            }
            tx.execute(
                "INSERT INTO schema_migration (version, name, applied_at) VALUES (?1, ?2, ?3)",
                params![m.version, m.name, now_rfc3339()?],
            )?;
            tx.pragma_update(None, "user_version", m.version)?;
            tx.commit()?;
        }
        Ok(current < supported)
    }

    pub fn info(&self) -> StoreResult<BookInfo> {
        read_info(&self.conn)
    }

    pub fn get_setting(&self, module: &str, key: &str) -> StoreResult<Option<serde_json::Value>> {
        read_setting(&self.conn, module, key)
    }

    pub fn audit_entries(&self, limit: u32) -> StoreResult<Vec<AuditEntry>> {
        read_audit(&self.conn, limit)
    }

    /// Run `f` inside one SQLite write transaction. Committed only if `f` returns `Ok`.
    ///
    /// The write lock is taken up front, so a second concurrent writer fails at once
    /// with [`StoreError::Busy`] rather than part-way through.
    pub fn write<T, E>(&mut self, f: impl FnOnce(&WriteTx<'_>) -> Result<T, E>) -> Result<T, E>
    where
        E: From<StoreError>,
    {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StoreError::from)?;
        let value = f(&WriteTx { tx: &tx })?;
        tx.commit().map_err(StoreError::from)?;
        // An encrypted book is saved before the change reports success; if saving
        // fails the in-memory book goes back to the last saved state.
        if self.enc.is_some()
            && let Err(e) = self.save()
        {
            let _ = self.roll_back();
            return Err(e.into());
        }
        Ok(value)
    }

    /// Run `f` inside a read transaction (a consistent snapshot). Always rolled back,
    /// so nothing `f` does can persist.
    pub fn read<T, E>(&mut self, f: impl FnOnce(&WriteTx<'_>) -> Result<T, E>) -> Result<T, E>
    where
        E: From<StoreError>,
    {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(StoreError::from)?;
        let value = f(&WriteTx { tx: &tx });
        tx.rollback().map_err(StoreError::from)?;
        value
    }
}

/// Handle for writes inside [`Store::write`].
#[derive(Debug)]
pub struct WriteTx<'a> {
    tx: &'a rusqlite::Transaction<'a>,
}

impl WriteTx<'_> {
    pub fn info(&self) -> StoreResult<BookInfo> {
        read_info(self.tx)
    }

    pub fn get_setting(&self, module: &str, key: &str) -> StoreResult<Option<serde_json::Value>> {
        read_setting(self.tx, module, key)
    }

    pub fn audit_entries(&self, limit: u32) -> StoreResult<Vec<AuditEntry>> {
        read_audit(self.tx, limit)
    }

    /// A stamp that changes whenever the book changes: the newest audit sequence
    /// number and the number of transactions (caches of derived data compare it).
    pub fn change_stamp(&self) -> StoreResult<(i64, i64)> {
        Ok(self.tx.query_row(
            "SELECT (SELECT COALESCE(MAX(seq), 0) FROM audit_log), (SELECT COUNT(*) FROM txn)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    /// Store a credential (encrypted books only; the caller checks). Not audited.
    pub fn put_secret(&self, id: &str, value: &str) -> StoreResult<()> {
        self.tx.execute(
            "INSERT INTO secret (id, value) VALUES (?1, ?2)
             ON CONFLICT (id) DO UPDATE SET value = excluded.value",
            params![id, value],
        )?;
        Ok(())
    }

    pub fn get_secret(&self, id: &str) -> StoreResult<Option<String>> {
        Ok(self
            .tx
            .query_row("SELECT value FROM secret WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn delete_secret(&self, id: &str) -> StoreResult<()> {
        self.tx
            .execute("DELETE FROM secret WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn set_setting(
        &self,
        module: &str,
        key: &str,
        value: &serde_json::Value,
        actor: &str,
    ) -> StoreResult<()> {
        self.tx.execute(
            "INSERT INTO book_setting (module, key, value_json, changed_by, changed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (module, key) DO UPDATE SET
               value_json = excluded.value_json,
               changed_by = excluded.changed_by,
               changed_at = excluded.changed_at",
            params![
                module,
                key,
                serde_json::to_string(value)?,
                actor,
                now_rfc3339()?
            ],
        )?;
        Ok(())
    }

    /// Remove a setting row entirely (used when undo restores "never set").
    pub fn delete_setting(&self, module: &str, key: &str) -> StoreResult<()> {
        self.tx.execute(
            "DELETE FROM book_setting WHERE module = ?1 AND key = ?2",
            params![module, key],
        )?;
        Ok(())
    }

    /// Append an audit entry without snapshots (Phase 0 form).
    pub fn append_audit(
        &self,
        actor: &str,
        command: &str,
        args: &serde_json::Value,
        result: &serde_json::Value,
    ) -> StoreResult<i64> {
        self.append_audit_record(&AuditRecord {
            actor,
            command,
            args: args.clone(),
            result: result.clone(),
            ..AuditRecord::default()
        })
    }

    /// Append a full audit entry, with snapshots and undo/redo links.
    pub fn append_audit_record(&self, r: &AuditRecord<'_>) -> StoreResult<i64> {
        let opt = |v: &Option<serde_json::Value>| -> StoreResult<Option<String>> {
            v.as_ref()
                .map(|v| serde_json::to_string(v).map_err(StoreError::from))
                .transpose()
        };
        self.tx.execute(
            "INSERT INTO audit_log
               (at, actor, command, args_json, result_json, before_json, after_json, undoes, redoes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                now_rfc3339()?,
                r.actor,
                r.command,
                serde_json::to_string(&r.args)?,
                serde_json::to_string(&r.result)?,
                opt(&r.before)?,
                opt(&r.after)?,
                r.undoes,
                r.redoes,
            ],
        )?;
        Ok(self.tx.last_insert_rowid())
    }

    /// One audit entry by sequence number.
    pub fn audit_entry(&self, seq: i64) -> StoreResult<Option<AuditEntry>> {
        Ok(query_audit(self.tx, "WHERE a.seq = ?1", params![seq])?
            .into_iter()
            .next())
    }

    /// Audit entries with sequence greater than `seq`, oldest first.
    pub fn audit_entries_after(&self, seq: i64) -> StoreResult<Vec<AuditEntry>> {
        query_audit(self.tx, "WHERE a.seq > ?1 ORDER BY a.seq", params![seq])
    }

    /// Recent audit entries, newest first, optionally for one actor.
    pub fn audit_list(&self, actor: Option<&str>, limit: u32) -> StoreResult<Vec<AuditEntry>> {
        query_audit(
            self.tx,
            "WHERE (?1 IS NULL OR a.actor = ?1) ORDER BY a.seq DESC LIMIT ?2",
            params![actor, limit],
        )
    }

    /// All audit entries by one actor, newest first.
    pub fn audit_entries_by(&self, actor: &str) -> StoreResult<Vec<AuditEntry>> {
        query_audit(
            self.tx,
            "WHERE a.actor = ?1 ORDER BY a.seq DESC",
            params![actor],
        )
    }

    /// The book's closing date, if set.
    pub fn closing_date(&self) -> StoreResult<Option<Date>> {
        let raw: Option<String> = self
            .tx
            .query_row("SELECT closing_date FROM book", [], |r| r.get(0))?;
        raw.map(|s| s.parse::<Date>().map_err(StoreError::from))
            .transpose()
    }

    /// Rename the book (app-desktop.md revision 26); the file is never renamed.
    pub fn set_name(&self, name: &str) -> StoreResult<()> {
        self.tx
            .execute("UPDATE book SET name = ?1", params![name])?;
        Ok(())
    }

    pub fn set_closing_date(&self, date: Option<Date>) -> StoreResult<()> {
        self.tx.execute(
            "UPDATE book SET closing_date = ?1",
            params![date.map(|d| d.to_string())],
        )?;
        Ok(())
    }

    /// Id of the book's default `Opening Balances` account.
    pub fn opening_account_id(&self) -> StoreResult<Option<AccountId>> {
        let raw: Option<String> =
            self.tx
                .query_row("SELECT opening_account_id FROM book", [], |r| r.get(0))?;
        raw.map(|s| s.parse::<AccountId>().map_err(StoreError::from))
            .transpose()
    }
}

fn read_info(conn: &Connection) -> StoreResult<BookInfo> {
    let schema_version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    conn.query_row(
        "SELECT id, name, base_currency, created_at, closing_date FROM book",
        [],
        |r| {
            Ok(BookInfo {
                id: r.get(0)?,
                name: r.get(1)?,
                base_currency: r.get(2)?,
                created_at: r.get(3)?,
                closing_date: r.get(4)?,
                schema_version,
            })
        },
    )
    .optional()?
    .ok_or(StoreError::NotABook)
}

fn read_setting(
    conn: &Connection,
    module: &str,
    key: &str,
) -> StoreResult<Option<serde_json::Value>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value_json FROM book_setting WHERE module = ?1 AND key = ?2",
            params![module, key],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|s| serde_json::from_str(&s).map_err(StoreError::from))
        .transpose()
}

fn read_audit(conn: &Connection, limit: u32) -> StoreResult<Vec<AuditEntry>> {
    query_audit(conn, "ORDER BY a.seq DESC LIMIT ?1", params![limit])
}

type AuditRow = (
    i64,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    bool,
);

fn query_audit(
    conn: &Connection,
    tail: &str,
    args: impl rusqlite::Params,
) -> StoreResult<Vec<AuditEntry>> {
    let sql = format!(
        "SELECT a.seq, a.at, a.actor, a.command, a.args_json, a.result_json,
                a.before_json, a.after_json, a.undoes, a.redoes,
                (SELECT max(u.seq) FROM audit_log u WHERE u.undoes = a.seq),
                EXISTS (SELECT 1 FROM audit_log u WHERE u.undoes = a.seq
                        AND NOT EXISTS (SELECT 1 FROM audit_log r WHERE r.redoes = u.seq))
         FROM audit_log a {tail}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(args, |r| -> rusqlite::Result<AuditRow> {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
            r.get(7)?,
            r.get(8)?,
            r.get(9)?,
            r.get(10)?,
            r.get(11)?,
        ))
    })?;
    let parse_opt = |s: Option<String>| -> StoreResult<Option<serde_json::Value>> {
        s.map(|s| serde_json::from_str(&s).map_err(StoreError::from))
            .transpose()
    };
    let mut out = Vec::new();
    for row in rows {
        let (
            seq,
            at,
            actor,
            command,
            args,
            result,
            before,
            after,
            undoes,
            redoes,
            undone_by,
            undone,
        ) = row?;
        out.push(AuditEntry {
            seq,
            at,
            actor,
            command,
            args: serde_json::from_str(&args)?,
            result: serde_json::from_str(&result)?,
            before: parse_opt(before)?,
            after: parse_opt(after)?,
            undoes,
            redoes,
            undone_by,
            undone,
        });
    }
    Ok(out)
}

/// Tests and reviews: write a book in the format of an older MoneyKeep (schema
/// `version`, write-ahead logging as MoneyKeep sets it), so opening it updates it.
#[doc(hidden)]
pub fn create_old_book_for_tests(path: &Path, name: &str, version: i64) -> StoreResult<()> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    for m in MIGRATIONS.iter().filter(|m| m.version <= version) {
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)?;
        if let Some(fix) = m.fix {
            fix(&tx)?;
        }
        tx.pragma_update(None, "user_version", m.version)?;
        tx.commit()?;
    }
    conn.execute(
        "INSERT INTO book (id, name, base_currency, created_at) VALUES (?1, ?2, 'USD', ?3)",
        params![BookId::new().to_string(), name, now_rfc3339()?],
    )?;
    Ok(())
}

/// The newest book format this build reads.
pub fn latest_schema_version() -> i64 {
    latest_version()
}

/// Today's date (UTC).
pub fn today_utc() -> StoreResult<Date> {
    Ok(Date::from_time(OffsetDateTime::now_utc().date())?)
}

/// Current UTC time as fixed-width RFC 3339 (nanosecond precision), so timestamps
/// compare correctly as text.
pub fn now_rfc3339() -> StoreResult<String> {
    let fmt =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:9]Z");
    Ok(OffsetDateTime::now_utc().format(&fmt)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn usd() -> CommodityCode {
        CommodityCode::new("USD").unwrap()
    }

    #[test]
    fn create_then_open_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.moneykeep");
        Store::create(&path, "Household", &usd()).unwrap();
        let store = Store::open(&path).unwrap();
        let info = store.info().unwrap();
        assert_eq!(info.name, "Household");
        assert_eq!(info.base_currency, "USD");
        assert_eq!(info.schema_version, latest_version());
    }

    #[test]
    fn unwritable_or_missing_files_are_io_errors_with_the_path() {
        let dir = tempfile::tempdir().unwrap();
        // A folder that does not exist (a regular file stands where the folder should be).
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"x").unwrap();
        let path = blocker.join("book.moneykeep");
        match Store::create(&path, "A", &usd()) {
            Err(StoreError::Io {
                action, path: p, ..
            }) => {
                assert_eq!(action, "create");
                assert_eq!(p, path.display().to_string());
            }
            other => panic!("expected an io error, got {other:?}"),
        }
        let missing = dir.path().join("missing.moneykeep");
        match Store::open(&missing) {
            Err(StoreError::Io { action, .. }) => assert_eq!(action, "open"),
            other => panic!("expected an io error, got {other:?}"),
        }
    }

    #[test]
    fn create_refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.moneykeep");
        Store::create(&path, "A", &usd()).unwrap();
        assert!(matches!(
            Store::create(&path, "B", &usd()),
            Err(StoreError::AlreadyExists(_))
        ));
    }

    #[test]
    fn write_commits_setting_and_audit_together() {
        let mut store = Store::in_memory("T", &usd()).unwrap();
        store
            .write(|tx| -> StoreResult<()> {
                tx.set_setting("estimates", "enabled", &json!(true), "test")?;
                tx.append_audit("test", "setting.set", &json!({}), &json!(null))?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            store.get_setting("estimates", "enabled").unwrap(),
            Some(json!(true))
        );
        assert_eq!(store.audit_entries(10).unwrap().len(), 1);
    }

    #[test]
    fn failed_write_rolls_back_everything() {
        let mut store = Store::in_memory("T", &usd()).unwrap();
        let result = store.write(|tx| -> StoreResult<()> {
            tx.set_setting("estimates", "enabled", &json!(true), "test")?;
            tx.append_audit("test", "setting.set", &json!({}), &json!(null))?;
            Err(StoreError::NotABook)
        });
        assert!(result.is_err());
        assert_eq!(store.get_setting("estimates", "enabled").unwrap(), None);
        assert!(store.audit_entries(10).unwrap().is_empty());
    }

    #[test]
    fn audit_log_is_append_only() {
        let mut store = Store::in_memory("T", &usd()).unwrap();
        store
            .write(|tx| tx.append_audit("test", "x", &json!({}), &json!(null)))
            .unwrap();
        assert!(
            store
                .conn
                .execute("UPDATE audit_log SET actor = 'evil'", [])
                .is_err()
        );
        assert!(store.conn.execute("DELETE FROM audit_log", []).is_err());
    }

    #[test]
    fn new_book_has_opening_balances_account() {
        let mut store = Store::in_memory("T", &usd()).unwrap();
        let accounts = store
            .write(|tx| -> StoreResult<_> { Ok((tx.accounts()?, tx.opening_account_id()?)) })
            .unwrap();
        assert_eq!(accounts.0.len(), 1);
        assert_eq!(accounts.0[0].name, "Opening Balances");
        assert_eq!(Some(accounts.0[0].id), accounts.1);
    }

    #[test]
    fn phase0_book_migrates_and_gets_opening_balances() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.moneykeep");
        {
            // Build a v1 (Phase 0) book by hand: migration 1 only.
            let conn = Connection::open(&path).unwrap();
            let v1 = &MIGRATIONS[0];
            conn.execute_batch(v1.sql).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            conn.execute(
                "INSERT INTO book (id, name, base_currency, created_at)
                 VALUES ('b', 'Old', 'EUR', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO audit_log (at, actor, command, args_json, result_json)
                 VALUES ('2026-01-01T00:00:00Z', 'cli', 'module.enable', '{}', 'null')",
                [],
            )
            .unwrap();
        }
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.info().unwrap().schema_version, latest_version());
        let (accounts, opening) = store
            .write(|tx| -> StoreResult<_> { Ok((tx.accounts()?, tx.opening_account_id()?)) })
            .unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].name, "Opening Balances");
        assert_eq!(accounts[0].commodity.as_str(), "EUR");
        assert_eq!(Some(accounts[0].id), opening);
        // Old audit rows survive with no snapshots.
        let log = store.audit_entries(10).unwrap();
        assert_eq!(log.len(), 1);
        assert!(log[0].before.is_none() && log[0].after.is_none());
    }

    /// input-safety.md AT9: migration v8 cleans text imported before the rules.
    #[test]
    fn input_safety_at9_migration_v8_cleans_text_and_logs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v7.moneykeep");
        {
            let conn = Connection::open(&path).unwrap();
            for m in MIGRATIONS.iter().filter(|m| m.version <= 7) {
                conn.execute_batch(m.sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 7).unwrap();
            conn.execute(
                "INSERT INTO book (id, name, base_currency, created_at)
                 VALUES ('b', 'Old', 'USD', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO txn (id, date, currency, payee, memo, status, entered_at)
                 VALUES ('t1', '2026-01-02', 'USD', ?1, 'plain', 'posted', '2026-01-02T00:00:00Z')",
                ["Coffee \u{202E}poT"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO txn (id, date, currency, payee, status, entered_at)
                 VALUES ('t2', '2026-01-02', 'USD', 'Two  spaces', 'posted', '2026-01-02T00:00:00Z')",
                [],
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.info().unwrap().schema_version, latest_version());
        let conn = Connection::open(&path).unwrap();
        let payee: String = conn
            .query_row("SELECT payee FROM txn WHERE id = 't1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(payee, "Coffee poT");
        // Text without unsafe characters is left exactly as it was.
        let other: String = conn
            .query_row("SELECT payee FROM txn WHERE id = 't2'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(other, "Two  spaces");
        let log = store.audit_entries(10).unwrap();
        let entry = log.iter().find(|e| e.actor == "migration").unwrap();
        assert_eq!(entry.command, "migration.clean_text");
    }

    #[test]
    fn rejects_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.moneykeep");
        drop(Store::create(&path, "A", &usd()).unwrap());
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 999).unwrap();
        drop(conn);
        assert!(matches!(Store::open(&path), Err(StoreError::TooNew { .. })));
    }

    /// release-prep.md §4: migration v21 gives every split an id and converts an
    /// invoice line's links to billed project costs from line positions to split
    /// ids (a link to a line that no longer exists stays as it was).
    #[test]
    fn migration_v21_split_ids_and_billed_links() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v20.moneykeep");
        create_old_book_for_tests(&path, "Old", 20).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "INSERT INTO account (id, name, type, subtype, commodity, created_at)
                 VALUES ('a1', 'Materials', 'expense', 'expense', 'USD', '2026-01-01T00:00:00Z'),
                        ('a2', 'Checking', 'asset', 'bank', 'USD', '2026-01-01T00:00:00Z');
                 INSERT INTO txn (id, date, currency, status, entered_at)
                 VALUES ('t1', '2026-07-02', 'USD', 'posted', '2026-07-02T00:00:00Z');
                 INSERT INTO split (txn_id, idx, account_id, value, quantity, reconcile, billable)
                 VALUES ('t1', 0, 'a1', '200.00', '200.00', 'n', 1),
                        ('t1', 1, 'a2', '-200.00', '-200.00', 'n', 0);
                 INSERT INTO sales_doc (id, grp, json) VALUES ('d1', 'invoice',
                   '{\"id\":\"d1\",\"lines\":[{\"billable_items\":[\"txn:t1:0\",\"txn:t9:0\",\"trip:x\"]}]}');",
            )
            .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.info().unwrap().schema_version, latest_version());
        let ids: Vec<Option<String>> = {
            let mut st = store
                .conn
                .prepare("SELECT split_id FROM split WHERE txn_id = 't1' ORDER BY idx")
                .unwrap();
            st.query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(ids.len(), 2);
        let first = ids[0].clone().unwrap();
        assert!(first.parse::<mk_core::SplitId>().is_ok(), "{first}");
        assert_ne!(ids[0], ids[1]);
        let json: String = store
            .conn
            .query_row("SELECT json FROM sales_doc WHERE id = 'd1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v["lines"][0]["billable_items"],
            json!([format!("split:{first}"), "txn:t9:0", "trip:x"])
        );
        let n: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM audit_log WHERE command = 'migration.billed_links'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }
}
