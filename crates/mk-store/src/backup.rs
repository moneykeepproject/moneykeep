//! Backup copies of a book (backups.md): a complete, consistent copy written to a
//! new file and checked, and the inspection of a backup file before it is restored.
//!
//! - A plain book is copied with SQLite's `VACUUM INTO` from a separate read-only
//!   connection (SQLite documentation, `VACUUM INTO`): the copy is a consistent
//!   snapshot even while the book is in use (write-ahead logging lets the reader see
//!   the last committed state).
//! - An encrypted book is copied from its in-memory image, encrypted again with the
//!   book's own header and data key (a fresh payload nonce), so the copy opens with
//!   the same password and no plain copy ever touches the disk (book-encryption.md).

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, MAIN_DB, OpenFlags};
use zeroize::Zeroizing;

use crate::crypt::{self, Header, Key, Secret};
use crate::encrypted::{BookKind, book_kind, io_err};
use crate::migrations::latest_version;
use crate::{BookInfo, Store, StoreError, StoreResult, read_info};

/// Books larger than this are checked with `PRAGMA quick_check` instead of the full
/// `integrity_check` (backups.md).
pub const QUICK_CHECK_BYTES: u64 = 200 * 1024 * 1024;

/// What a backup is copied from: taken on the thread that owns the book, then used
/// on any thread (it holds no connection).
pub struct BackupSource(Inner);

enum Inner {
    Plain {
        path: PathBuf,
    },
    Encrypted {
        header: Header,
        key: Key,
        image: Zeroizing<Vec<u8>>,
    },
}

impl std::fmt::Debug for BackupSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Inner::Plain { path } => f
                .debug_struct("BackupSource::Plain")
                .field("path", path)
                .finish(),
            Inner::Encrypted { image, .. } => f
                .debug_struct("BackupSource::Encrypted")
                .field("bytes", &image.len())
                .finish_non_exhaustive(),
        }
    }
}

/// The data key of an encrypted backup, kept after a check so the copy made from it
/// can be checked again without the password (no second key derivation).
pub struct BackupKey(Key);

impl std::fmt::Debug for BackupKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BackupKey(…)")
    }
}

/// How an encrypted backup file is unlocked for a check.
#[derive(Debug, Clone, Copy)]
pub enum Unlock<'a> {
    /// No secret: an encrypted backup fails with `password_required`.
    None,
    /// The password or recovery key the book had when the backup was made.
    Secret(&'a Secret),
    /// The data key of the open book (a backup of the same book opens with it).
    Source(&'a BackupSource),
    /// The key found by an earlier check.
    Key(&'a BackupKey),
}

/// A checked backup file.
#[derive(Debug)]
pub struct CheckedBackup {
    pub encrypted: bool,
    pub info: BookInfo,
    /// The data key of an encrypted backup.
    pub key: Option<BackupKey>,
}

fn sqlite_text(path: &Path) -> StoreResult<&str> {
    path.to_str()
        .ok_or_else(|| io_err("back up", path, &"the path is not valid Unicode"))
}

/// Run the integrity check on an open copy: `quick_check` when `quick`.
fn integrity(conn: &Connection, quick: bool) -> StoreResult<()> {
    let pragma = if quick {
        "quick_check"
    } else {
        "integrity_check"
    };
    let mut stmt = conn.prepare(&format!("PRAGMA {pragma}"))?;
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    if rows.len() == 1 && rows.first().is_some_and(|r| r == "ok") {
        Ok(())
    } else {
        Err(StoreError::Corrupt(format!(
            "the copy failed its integrity check: {}",
            rows.into_iter().take(3).collect::<Vec<_>>().join("; ")
        )))
    }
}

/// The version and book row of a copy, refusing one newer than this build.
fn version_and_info(conn: &Connection) -> StoreResult<BookInfo> {
    let found: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let supported = latest_version();
    if found > supported {
        return Err(StoreError::TooNew { found, supported });
    }
    read_info(conn)
}

fn memory_with(image: &[u8]) -> StoreResult<Connection> {
    let mut conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.deserialize_read_exact(MAIN_DB, image, image.len(), true)?;
    Ok(conn)
}

fn read_all(path: &Path) -> StoreResult<Vec<u8>> {
    let mut f = File::open(path).map_err(|e| io_err("open", path, &e))?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)
        .map_err(|e| io_err("open", path, &e))?;
    Ok(bytes)
}

/// Check an encrypted file with a data key; returns the book's info.
fn check_encrypted_with(bytes: &[u8], key: &[u8; 32], quick: bool) -> StoreResult<BookInfo> {
    let (_, len) = crypt::parse(bytes)?;
    let image = crypt::decrypt(bytes, len, key)?;
    let conn = memory_with(&image)?;
    drop(image);
    integrity(&conn, quick)?;
    version_and_info(&conn)
}

impl BackupSource {
    /// Whether the copy is encrypted (the book is password protected).
    pub fn is_encrypted(&self) -> bool {
        matches!(self.0, Inner::Encrypted { .. })
    }

    /// Write the copy to `target`, which must not exist or be an empty file (a
    /// placeholder the caller created to reserve the name).
    pub fn write_to(&self, target: &Path) -> StoreResult<()> {
        match &self.0 {
            Inner::Plain { path } => {
                let conn = Connection::open_with_flags(
                    path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )
                .map_err(|e| StoreError::from(e).file_level("open", path))?;
                conn.busy_timeout(std::time::Duration::from_secs(5))?;
                conn.execute("VACUUM INTO ?1", [sqlite_text(target)?])
                    .map_err(|e| StoreError::from(e).file_level("create", target))?;
                Ok(())
            }
            Inner::Encrypted { header, key, image } => {
                let bytes = crypt::encode(header, key, image)?;
                let mut f = OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(target)
                    .map_err(|e| io_err("create", target, &e))?;
                f.write_all(&bytes)
                    .and_then(|()| f.sync_all())
                    .map_err(|e| io_err("create", target, &e))?;
                Ok(())
            }
        }
    }

    /// Check a copy written by [`BackupSource::write_to`]: the integrity check
    /// (`quick_check` for books over [`QUICK_CHECK_BYTES`]) and the book row. An
    /// encrypted copy is checked in memory with the book's key.
    pub fn check(&self, target: &Path) -> StoreResult<BookInfo> {
        let quick = std::fs::metadata(target).map(|m| m.len()).unwrap_or(0) > QUICK_CHECK_BYTES;
        match &self.0 {
            Inner::Plain { .. } => {
                let conn = Connection::open_with_flags(target, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                integrity(&conn, quick)?;
                version_and_info(&conn)
            }
            Inner::Encrypted { key, .. } => check_encrypted_with(&read_all(target)?, key, quick),
        }
    }
}

impl BackupSource {
    /// A plain backup file of this (now password-protected) book, to be written
    /// encrypted with this book's header and data key, so it opens with the book's
    /// password (security-review-1 B2, Protect them now). The plain file is checked
    /// first (its kind, integrity and version) and not changed. Errors:
    /// `NotEncrypted` when this book is not protected, `InvalidPassword` when the
    /// file is already encrypted.
    pub fn protecting(&self, plain: &Path) -> StoreResult<BackupSource> {
        let Inner::Encrypted { header, key, .. } = &self.0 else {
            return Err(StoreError::NotEncrypted);
        };
        if book_kind(plain)? != BookKind::Plain {
            return Err(StoreError::InvalidPassword(format!(
                "{} is already password-protected",
                plain.display()
            )));
        }
        check_backup(plain, Unlock::None)?;
        let conn = Connection::open_with_flags(plain, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| StoreError::from(e).file_level("open", plain))?;
        let image = Zeroizing::new(conn.serialize(MAIN_DB)?.to_vec());
        Ok(encrypted_source(header, key, &image))
    }
}

/// Check a backup file before it is restored: its kind, its key (for an encrypted
/// one), its integrity and its version (a backup made by a newer MoneyKeep is
/// refused with [`StoreError::TooNew`]).
pub fn check_backup(path: &Path, unlock: Unlock<'_>) -> StoreResult<CheckedBackup> {
    let quick = std::fs::metadata(path)
        .map_err(|e| io_err("open", path, &e))?
        .len()
        > QUICK_CHECK_BYTES;
    match book_kind(path)? {
        BookKind::Plain => {
            let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| StoreError::from(e).file_level("open", path))?;
            // The version first: a newer format may not even pass this build's check.
            let found: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
            if found > latest_version() {
                return Err(StoreError::TooNew {
                    found,
                    supported: latest_version(),
                });
            }
            integrity(&conn, quick)?;
            Ok(CheckedBackup {
                encrypted: false,
                info: version_and_info(&conn)?,
                key: None,
            })
        }
        BookKind::Encrypted => {
            let bytes = read_all(path)?;
            let (header, len) = crypt::parse(&bytes)?;
            let try_key = |k: &[u8; 32]| crypt::decrypt(&bytes, len, k).is_ok();
            let key: Key = match unlock {
                Unlock::Key(k) if try_key(&k.0) => Zeroizing::new(*k.0),
                Unlock::Source(BackupSource(Inner::Encrypted { key, .. })) if try_key(key) => {
                    Zeroizing::new(**key)
                }
                Unlock::Secret(s) => crypt::unlock(&header, s)?,
                Unlock::None | Unlock::Source(_) => return Err(StoreError::PasswordRequired),
                Unlock::Key(_) => return Err(StoreError::WrongPassword),
            };
            let info = check_encrypted_with(&bytes, &key, quick)?;
            Ok(CheckedBackup {
                encrypted: true,
                info,
                key: Some(BackupKey(key)),
            })
        }
    }
}

/// What a backup of a book file that is not open is copied from (backups.md
/// revision 2, Back up this book… on the start screen). Nothing in the file is
/// changed and no format update is applied: a plain book is copied straight from its
/// file; an encrypted book is unlocked with `secret` (`password_required` without
/// one, `wrong_password` with a wrong one) and copied from its decrypted image, held
/// in memory only, so the backup stays encrypted with the same password.
pub fn backup_source_of_file(path: &Path, secret: Option<&Secret>) -> StoreResult<BackupSource> {
    match book_kind(path)? {
        BookKind::Plain => {
            // Readable as a book (and not from a newer MoneyKeep) before it is copied.
            let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| StoreError::from(e).file_level("open", path))?;
            conn.busy_timeout(std::time::Duration::from_secs(5))?;
            version_and_info(&conn).map_err(|e| e.file_level("open", path))?;
            Ok(plain_source(path))
        }
        BookKind::Encrypted => {
            let secret = secret.ok_or(StoreError::PasswordRequired)?;
            let bytes = read_all(path)?;
            let (header, len) = crypt::parse(&bytes)?;
            let key = crypt::unlock(&header, secret)?;
            let image = crypt::decrypt(&bytes, len, &key)?;
            Ok(BackupSource(Inner::Encrypted { header, key, image }))
        }
    }
}

impl Store {
    /// What a backup of the open book is copied from (taken now: for an encrypted
    /// book, the current in-memory image).
    pub fn backup_source(&self) -> StoreResult<BackupSource> {
        if let Some(e) = self.enc.as_ref() {
            return Ok(BackupSource(Inner::Encrypted {
                header: e.header.clone(),
                key: Zeroizing::new(*e.data_key),
                image: Zeroizing::new(self.conn.serialize(MAIN_DB)?.to_vec()),
            }));
        }
        let path = self
            .path
            .clone()
            .ok_or_else(|| io_err("back up", Path::new(""), &"the book has no file"))?;
        Ok(BackupSource(Inner::Plain { path }))
    }

    /// A stamp that changes whenever the book changes (see [`crate::WriteTx::change_stamp`]).
    pub fn change_stamp(&mut self) -> StoreResult<(i64, i64)> {
        self.read(|tx| tx.change_stamp())
    }

    /// Remember that a backup holds the book as of `stamp`.
    pub fn mark_backed_up(&mut self, stamp: (i64, i64)) {
        self.backup_mark = Some(stamp);
    }

    /// Whether the book changed since the last backup (or since it was opened, when
    /// none was made in this session).
    pub fn changed_since_backup(&mut self) -> bool {
        let mark = self.backup_mark;
        self.change_stamp().ok() != mark || mark.is_none()
    }

    /// Open a book like [`Store::open_with`], calling `before_update` with a backup
    /// source when a format update (migration) is pending, before anything in the
    /// file is changed. If it fails, the book is not updated and its file is left
    /// exactly as it was; the error is returned.
    pub fn open_with_hook<E: From<StoreError>>(
        path: &Path,
        secret: Option<&Secret>,
        before_update: impl FnOnce(&BackupSource) -> Result<(), E>,
    ) -> Result<Self, E> {
        let mut failed: Option<E> = None;
        let mut hook = Some(before_update);
        let opened = {
            let mut run = |src: &BackupSource| -> StoreResult<()> {
                if let Some(h) = hook.take()
                    && let Err(e) = h(src)
                {
                    failed = Some(e);
                    return Err(StoreError::StoppedBeforeUpdate);
                }
                Ok(())
            };
            Self::open_inner(path, secret, Some(&mut run))
        };
        match (opened, failed) {
            (_, Some(e)) => Err(e),
            (Ok(s), None) => Ok(s),
            (Err(e), None) => Err(e.into()),
        }
    }
}

/// The hook [`Store::open_inner`] calls before migrating.
pub(crate) type BeforeUpdate<'a> = Option<&'a mut dyn FnMut(&BackupSource) -> StoreResult<()>>;

/// The source of a plain book before it is configured (the hook runs first).
pub(crate) fn plain_source(path: &Path) -> BackupSource {
    BackupSource(Inner::Plain {
        path: path.to_path_buf(),
    })
}

/// The source of an encrypted book being opened (its decrypted image).
pub(crate) fn encrypted_source(header: &Header, key: &Key, image: &[u8]) -> BackupSource {
    BackupSource(Inner::Encrypted {
        header: header.clone(),
        key: Zeroizing::new(**key),
        image: Zeroizing::new(image.to_vec()),
    })
}

/// Whether a book at `version` needs a migration.
pub(crate) fn pending(version: i64) -> bool {
    version < latest_version()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    use crate::KdfParams;
    use mk_core::CommodityCode;

    fn usd() -> CommodityCode {
        CommodityCode::new("USD").unwrap()
    }

    #[test]
    fn plain_copy_is_consistent_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Household.moneykeep");
        let mut store = Store::create(&path, "Household", &usd()).unwrap();
        store
            .write(|tx| {
                tx.append_audit(
                    "test",
                    "x",
                    &serde_json::json!({}),
                    &serde_json::json!(null),
                )
            })
            .unwrap();
        let src = store.backup_source().unwrap();
        assert!(!src.is_encrypted());
        let copy = dir.path().join("copy.moneykeep");
        // An empty placeholder reserves the name.
        std::fs::write(&copy, b"").unwrap();
        src.write_to(&copy).unwrap();
        let info = src.check(&copy).unwrap();
        assert_eq!(info.name, "Household");
        assert_eq!(info.schema_version, latest_version());
        let other = Store::open(&copy).unwrap();
        assert_eq!(other.audit_entries(10).unwrap().len(), 1);
        let checked = check_backup(&copy, Unlock::None).unwrap();
        assert!(!checked.encrypted);
    }

    #[test]
    fn encrypted_copy_keeps_its_password_and_is_never_plain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Secret.moneykeep");
        let (store, _) = Store::create_encrypted(
            &path,
            "Secret",
            &usd(),
            "correct horse battery",
            false,
            KdfParams::TEST,
        )
        .unwrap();
        let src = store.backup_source().unwrap();
        assert!(src.is_encrypted());
        let copy = dir.path().join("copy.moneykeep");
        src.write_to(&copy).unwrap();
        assert!(std::fs::read(&copy).unwrap().starts_with(crypt::MAGIC));
        src.check(&copy).unwrap();
        assert!(matches!(
            check_backup(&copy, Unlock::None),
            Err(StoreError::PasswordRequired)
        ));
        let pw = Secret::Password("correct horse battery".into());
        let checked = check_backup(&copy, Unlock::Secret(&pw)).unwrap();
        assert!(checked.encrypted);
        let key = checked.key.unwrap();
        check_backup(&copy, Unlock::Key(&key)).unwrap();
        check_backup(&copy, Unlock::Source(&src)).unwrap();
        drop(store);
        Store::open_with(&copy, Some(&pw)).unwrap();
    }

    #[test]
    fn a_damaged_copy_fails_its_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("A.moneykeep");
        let store = Store::create(&path, "A", &usd()).unwrap();
        let src = store.backup_source().unwrap();
        let copy = dir.path().join("copy.moneykeep");
        src.write_to(&copy).unwrap();
        let mut bytes = std::fs::read(&copy).unwrap();
        for b in bytes.iter_mut().skip(4096).take(4096) {
            *b = 0x5A;
        }
        std::fs::write(&copy, &bytes).unwrap();
        assert!(src.check(&copy).is_err());
    }

    #[test]
    fn a_newer_backup_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("A.moneykeep");
        drop(Store::create(&path, "A", &usd()).unwrap());
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 999).unwrap();
        drop(conn);
        match check_backup(&path, Unlock::None) {
            Err(StoreError::TooNew { found: 999, .. }) => {}
            other => panic!("expected too new, got {other:?}"),
        }
    }

    #[test]
    fn changes_since_the_last_backup_are_seen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("A.moneykeep");
        drop(Store::create(&path, "A", &usd()).unwrap());
        let mut store = Store::open(&path).unwrap();
        assert!(
            !store.changed_since_backup(),
            "the baseline is the opened book"
        );
        store
            .write(|tx| {
                tx.append_audit(
                    "test",
                    "x",
                    &serde_json::json!({}),
                    &serde_json::json!(null),
                )
            })
            .unwrap();
        assert!(store.changed_since_backup());
        let stamp = store.change_stamp().unwrap();
        store.mark_backed_up(stamp);
        assert!(!store.changed_since_backup());
    }
}
