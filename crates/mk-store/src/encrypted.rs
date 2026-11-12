//! Encrypted books on disk: the locked file, atomic saves with `<file>.prev`, and the
//! key operations (encrypt, change password, recovery key, decrypt). The open book
//! lives in an in-memory SQLite database; nothing in plain form touches the disk.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;
use rusqlite::{Connection, MAIN_DB};
use zeroize::Zeroizing;

use crate::crypt::{self, Header, KdfParams, Key, Secret, SlotKind};
use crate::{AuditRecord, Store, StoreError, StoreResult, WriteTx};

/// The key material and file of an open encrypted book.
pub(crate) struct Encrypted {
    pub path: PathBuf,
    /// Handle on the current book file, holding the exclusive lock.
    pub file: File,
    pub header: Header,
    pub data_key: Key,
    /// The last saved file, for rolling back the in-memory book after a failed save.
    pub last: Vec<u8>,
    pub fail_next_save: bool,
}

impl std::fmt::Debug for Encrypted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encrypted")
            .field("path", &self.path)
            .finish()
    }
}

/// Whether a book file is plain SQLite or encrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookKind {
    Plain,
    Encrypted,
}

/// Windows' error for reading bytes another handle has locked.
const ERROR_LOCK_VIOLATION: i32 = 33;

/// What kind of book `path` holds, from its first bytes. `BookInUse` when another
/// session has the book open (Windows).
pub fn book_kind(path: &Path) -> StoreResult<BookKind> {
    let mut head = [0u8; 16];
    let mut f = File::open(path).map_err(|e| io_err("open", path, &e))?;
    let n = f.read(&mut head).map_err(|e| {
        // On Windows another session's exclusive lock covers the bytes, so the read
        // itself fails (ERROR_LOCK_VIOLATION); on Unix the lock is advisory and the
        // second open finds it in `lock`.
        if cfg!(windows) && e.raw_os_error() == Some(ERROR_LOCK_VIOLATION) {
            StoreError::BookInUse
        } else {
            io_err("open", path, &e)
        }
    })?;
    let head = head.get(..n).unwrap_or(&[]);
    if head.starts_with(crypt::MAGIC) {
        Ok(BookKind::Encrypted)
    } else if head == crypt::SQLITE_MAGIC {
        Ok(BookKind::Plain)
    } else {
        Err(StoreError::NotABook)
    }
}

pub(crate) fn io_err(action: &'static str, path: &Path, e: &dyn std::fmt::Display) -> StoreError {
    StoreError::Io {
        action,
        path: path.display().to_string(),
        message: e.to_string(),
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// `<file>.prev`
pub fn prev_path(path: &Path) -> PathBuf {
    sibling(path, ".prev")
}

/// Open options that let a locked, open file be renamed on Windows.
fn options() -> OpenOptions {
    // Changed only on Windows.
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut o = OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
        o.share_mode(0x1 | 0x2 | 0x4);
    }
    o
}

fn lock(file: &File, path: &Path) -> StoreResult<()> {
    match file.try_lock_exclusive() {
        Ok(true) => Ok(()),
        Ok(false) => Err(StoreError::BookInUse),
        Err(e) => Err(io_err("lock", path, &e)),
    }
}

/// Write `bytes` as `path` atomically: a locked temp file in the same folder, synced,
/// then (with `keep_prev`) the current file renamed to `<file>.prev`, then the temp
/// renamed into place. Returns the new file's handle, which holds the lock.
pub(crate) fn write_atomic(
    path: &Path,
    bytes: &[u8],
    keep_prev: bool,
    fail: bool,
) -> StoreResult<File> {
    let tag = crypt::random::<6>()?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let tmp = sibling(path, &format!(".tmp-{tag}"));
    let result = (|| -> std::io::Result<File> {
        let mut f = options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        if fail {
            return Err(std::io::Error::other("simulated write failure"));
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        match f.try_lock_exclusive() {
            Ok(true) => {}
            Ok(false) => return Err(std::io::Error::other("the new file is locked")),
            Err(e) => return Err(e),
        }
        let prev = prev_path(path);
        let moved = keep_prev && path.exists();
        if moved {
            std::fs::rename(path, &prev)?;
        }
        if let Err(e) = std::fs::rename(&tmp, path) {
            if moved {
                let _ = std::fs::rename(&prev, path);
            }
            return Err(e);
        }
        #[cfg(unix)]
        if let Some(dir) = path.parent() {
            let dir = if dir.as_os_str().is_empty() {
                Path::new(".")
            } else {
                dir
            };
            if let Ok(d) = File::open(dir) {
                let _ = d.sync_all();
            }
        }
        Ok(f)
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io_err("save", path, &e)
    })
}

fn load_image(conn: &mut Connection, image: &[u8]) -> StoreResult<()> {
    conn.deserialize_read_exact(MAIN_DB, image, image.len(), false)?;
    Ok(())
}

fn image_of(conn: &Connection) -> StoreResult<Zeroizing<Vec<u8>>> {
    Ok(Zeroizing::new(conn.serialize(MAIN_DB)?.to_vec()))
}

fn memory_connection() -> StoreResult<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(conn)
}

/// Build a header with a password slot and optionally a recovery slot; returns the
/// recovery key's display form.
fn new_header(
    password: &str,
    recovery: bool,
    kdf: KdfParams,
    data_key: &Key,
    keep_recovery: Option<&crypt::Slot>,
) -> StoreResult<(Header, Option<String>)> {
    let mut h = Header {
        kdf,
        salt: crypt::random::<16>()?,
        slots: Vec::new(),
    };
    let pk = crypt::derive(password, kdf, &h.salt)?;
    h.slots
        .push(crypt::wrap(&h, SlotKind::Password, &pk, data_key)?);
    let mut shown = None;
    if let Some(slot) = keep_recovery {
        h.slots.push(slot.clone());
    } else if recovery {
        let rk = Zeroizing::new(crypt::random::<32>()?);
        h.slots
            .push(crypt::wrap(&h, SlotKind::Recovery, &rk, data_key)?);
        shown = Some(crypt::format_recovery_key(&rk));
    }
    Ok((h, shown))
}

fn session_audit(
    tx: &WriteTx<'_>,
    actor: &str,
    command: &str,
    args: serde_json::Value,
) -> StoreResult<()> {
    tx.append_audit_record(&AuditRecord {
        actor,
        command,
        args,
        result: serde_json::json!({}),
        before: None,
        after: None,
        undoes: None,
        redoes: None,
    })?;
    Ok(())
}

impl Store {
    /// Create a new encrypted book; it is never written in plain form. Returns the
    /// recovery key (display form) when one was asked for.
    pub fn create_encrypted(
        path: &Path,
        name: &str,
        base_currency: &mk_core::CommodityCode,
        password: &str,
        recovery: bool,
        kdf: KdfParams,
    ) -> StoreResult<(Self, Option<String>)> {
        crypt::check_password_rules(password)?;
        if path.exists() {
            return Err(StoreError::AlreadyExists(path.display().to_string()));
        }
        let mut store = Self::configure(memory_connection()?)?;
        store.migrate()?;
        store.init_book(name, base_currency)?;
        let data_key = crypt::random_key()?;
        let (header, shown) = new_header(password, recovery, kdf, &data_key, None)?;
        let image = image_of(&store.conn)?;
        let bytes = crypt::encode(&header, &data_key, &image)?;
        let file = write_atomic(path, &bytes, false, false).map_err(|e| match e {
            StoreError::Io { path, message, .. } => StoreError::Io {
                action: "create",
                path,
                message,
            },
            other => other,
        })?;
        store.path = Some(path.to_path_buf());
        store.enc = Some(Encrypted {
            path: path.to_path_buf(),
            file,
            header,
            data_key,
            last: bytes,
            fail_next_save: false,
        });
        Ok((store, shown))
    }

    /// Open an encrypted book with its password or recovery key.
    pub(crate) fn open_encrypted(
        path: &Path,
        secret: &Secret,
        before_update: crate::backup::BeforeUpdate<'_>,
    ) -> StoreResult<Self> {
        let mut file = options()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| io_err("open", path, &e))?;
        lock(&file, path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|e| io_err("open", path, &e))?;
        let damaged = |e: StoreError| match e {
            StoreError::BookDamaged(_) => {
                let prev = prev_path(path);
                StoreError::BookDamaged(if prev.exists() {
                    format!(" A previous version is at {}.", prev.display())
                } else {
                    String::new()
                })
            }
            other => other,
        };
        let (header, header_len) = crypt::parse(&bytes).map_err(damaged)?;
        let data_key = crypt::unlock(&header, secret).map_err(damaged)?;
        let image = crypt::decrypt(&bytes, header_len, &data_key).map_err(damaged)?;
        let mut conn = memory_connection()?;
        load_image(&mut conn, &image)?;
        if let Some(hook) = before_update {
            let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
            if crate::backup::pending(version) {
                hook(&crate::backup::encrypted_source(&header, &data_key, &image))?;
            }
        }
        drop(image);
        let mut store = Self::configure(conn)?;
        store.path = Some(path.to_path_buf());
        store.enc = Some(Encrypted {
            path: path.to_path_buf(),
            file,
            header,
            data_key,
            last: bytes,
            fail_next_save: false,
        });
        if store.migrate()? {
            store.save()?;
        }
        store.info()?;
        Ok(store)
    }

    /// Whether the open book is encrypted.
    pub fn is_encrypted(&self) -> bool {
        self.enc.is_some()
    }

    /// Whether the open encrypted book has a recovery key slot.
    pub fn has_recovery_key(&self) -> bool {
        self.enc
            .as_ref()
            .is_some_and(|e| e.header.slots.iter().any(|s| s.kind == SlotKind::Recovery))
    }

    /// The book file's path (None for an in-memory book).
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The open encrypted book's file as it is on disk, read through the handle that
    /// holds the lock (tests: on Windows no other handle can read a locked file).
    pub fn file_bytes_for_tests(&self) -> StoreResult<Vec<u8>> {
        use std::io::{Seek, SeekFrom};
        let e = self.enc.as_ref().ok_or(StoreError::NotEncrypted)?;
        let mut file = &e.file;
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.read_to_end(&mut bytes))
            .map_err(|err| io_err("read", &e.path, &err))?;
        Ok(bytes)
    }

    /// Make the next save fail (tests of the rollback path).
    pub fn fail_next_save_for_tests(&mut self) {
        if let Some(e) = self.enc.as_mut() {
            e.fail_next_save = true;
        }
    }

    /// Encrypt and write the current image (after every write transaction).
    pub(crate) fn save(&mut self) -> StoreResult<()> {
        let Some(e) = self.enc.as_ref() else {
            return Ok(());
        };
        let image = image_of(&self.conn)?;
        let bytes = crypt::encode(&e.header, &e.data_key, &image)?;
        drop(image);
        let fail = e.fail_next_save;
        let path = e.path.clone();
        if let Some(e) = self.enc.as_mut() {
            e.fail_next_save = false;
        }
        let file = write_atomic(&path, &bytes, true, fail)?;
        if let Some(e) = self.enc.as_mut() {
            e.file = file;
            e.last = bytes;
        }
        Ok(())
    }

    /// After a failed save: put the in-memory book back to the last saved state.
    pub(crate) fn roll_back(&mut self) -> StoreResult<()> {
        let Some(e) = self.enc.as_ref() else {
            return Ok(());
        };
        let (_, len) = crypt::parse(&e.last)?;
        let image = crypt::decrypt(&e.last, len, &e.data_key)?;
        load_image(&mut self.conn, &image)?;
        self.conn.pragma_update(None, "foreign_keys", true)?;
        Ok(())
    }

    /// Check a password or recovery key against the open encrypted book.
    pub fn verify(&self, current: &Secret) -> StoreResult<()> {
        let e = self.enc.as_ref().ok_or(StoreError::NotEncrypted)?;
        let key = crypt::unlock(&e.header, current)?;
        if *key != *e.data_key {
            return Err(StoreError::WrongPassword);
        }
        Ok(())
    }

    /// Replace the header, record `command` in the audit log and save; on any failure
    /// the old header and in-memory book come back.
    fn rekey(&mut self, header: Header, actor: &str, command: &str) -> StoreResult<()> {
        let old = self
            .enc
            .as_mut()
            .map(|e| std::mem::replace(&mut e.header, header))
            .ok_or(StoreError::NotEncrypted)?;
        let r = self.write(|tx| session_audit(tx, actor, command, serde_json::json!({})));
        if r.is_err()
            && let Some(e) = self.enc.as_mut()
        {
            e.header = old;
        }
        r
    }

    /// Change the password (new salt; the data key and recovery slot are kept).
    pub fn change_password(
        &mut self,
        current: &Secret,
        new: &str,
        kdf: KdfParams,
        actor: &str,
    ) -> StoreResult<()> {
        crypt::check_password_rules(new)?;
        self.verify(current)?;
        let (key, recovery) = {
            let e = self.enc.as_ref().ok_or(StoreError::NotEncrypted)?;
            (
                Zeroizing::new(*e.data_key),
                e.header
                    .slots
                    .iter()
                    .find(|s| s.kind == SlotKind::Recovery)
                    .cloned(),
            )
        };
        let (header, _) = new_header(new, false, kdf, &key, recovery.as_ref())?;
        self.rekey(header, actor, "book.password_changed")
    }

    /// Replace (or add) the recovery key; returns the new key once.
    pub fn new_recovery_key(&mut self, current: &Secret, actor: &str) -> StoreResult<String> {
        self.verify(current)?;
        let e = self.enc.as_ref().ok_or(StoreError::NotEncrypted)?;
        let mut header = e.header.clone();
        header.slots.retain(|s| s.kind != SlotKind::Recovery);
        let rk = Zeroizing::new(crypt::random::<32>()?);
        header
            .slots
            .push(crypt::wrap(&header, SlotKind::Recovery, &rk, &e.data_key)?);
        self.rekey(header, actor, "book.recovery_key_changed")?;
        Ok(crypt::format_recovery_key(&rk))
    }

    /// Delete the recovery key slot.
    pub fn remove_recovery_key(&mut self, current: &Secret, actor: &str) -> StoreResult<()> {
        self.verify(current)?;
        let e = self.enc.as_ref().ok_or(StoreError::NotEncrypted)?;
        let mut header = e.header.clone();
        header.slots.retain(|s| s.kind != SlotKind::Recovery);
        self.rekey(header, actor, "book.recovery_key_changed")
    }

    /// Write a plain SQLite copy to another path, which must not exist. The name is
    /// reserved first and the copy moved into it without replacing anything
    /// (security-review-1 B1): if the name was taken meanwhile, the copy goes to the
    /// next free name (`… (2).moneykeep`). Returns where the copy is.
    pub fn save_unencrypted_copy(&mut self, target: &Path) -> StoreResult<PathBuf> {
        let own = self.path.clone();
        let same = own.as_ref().is_some_and(|p| {
            p == target
                || (std::fs::canonicalize(p).ok().is_some()
                    && std::fs::canonicalize(p).ok() == std::fs::canonicalize(target).ok())
        });
        if same {
            return Err(StoreError::InvalidPassword(
                "choose a different file; the copy cannot replace the book itself".into(),
            ));
        }
        let tag = crypt::random::<6>()?
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let reserved = crate::newfile::reserve(target).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                StoreError::AlreadyExists(target.display().to_string())
            } else {
                io_err("create", target, &e)
            }
        })?;
        let tmp = sibling(target, &format!(".tmp-{tag}"));
        let placed = match self.write_unencrypted_image(&tmp) {
            Ok(()) => reserved
                .fill_from(&tmp)
                .map_err(|e| io_err("create", target, &e)),
            Err(e) => {
                reserved.release();
                Err(e)
            }
        };
        if placed.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        placed
    }

    /// The unencrypted copy's bytes, written to a new file `tmp`.
    fn write_unencrypted_image(&mut self, tmp: &Path) -> StoreResult<()> {
        // The copy is not protected: credentials stay behind (connectivity.md).
        let image = image_of(&self.conn)?;
        let mut copy = memory_connection()?;
        load_image(&mut copy, &image)?;
        drop(image);
        // Overwrite the deleted rows too, so no credential bytes stay in free pages.
        copy.pragma_update(None, "secure_delete", "ON")?;
        copy.execute("DELETE FROM secret", [])?;
        let image = image_of(&copy)?;
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(tmp)
            .map_err(|e| io_err("create", tmp, &e))?;
        f.write_all(&image)
            .and_then(|()| f.sync_all())
            .map_err(|e| io_err("create", tmp, &e))
    }

    /// Convert the open plain book into an encrypted one in place. Returns the
    /// recovery key once when asked for.
    pub fn encrypt(
        &mut self,
        password: &str,
        recovery: bool,
        kdf: KdfParams,
        actor: &str,
    ) -> StoreResult<Option<String>> {
        crypt::check_password_rules(password)?;
        if self.enc.is_some() {
            return Err(StoreError::InvalidPassword(
                "the book is already password protected".into(),
            ));
        }
        let path = self.path.clone().ok_or(StoreError::NotEncrypted)?;
        // Exclusive access: leaving WAL mode fails while another connection has it open.
        self.conn
            .pragma_update_and_check(None, "journal_mode", "DELETE", |r| r.get::<_, String>(0))
            .map_err(|e| match StoreError::from(e) {
                StoreError::Busy => StoreError::BookInUse,
                other => other,
            })
            .and_then(|mode| {
                if mode.eq_ignore_ascii_case("delete") {
                    Ok(())
                } else {
                    Err(StoreError::BookInUse)
                }
            })?;
        let image = image_of(&self.conn)?;
        let mut conn = memory_connection()?;
        load_image(&mut conn, &image)?;
        drop(image);
        let mut mem = Self::configure(conn)?;
        mem.write(|tx| {
            session_audit(
                tx,
                actor,
                "book.encrypted",
                serde_json::json!({"recovery_key": recovery}),
            )
        })?;
        let data_key = crypt::random_key()?;
        let (header, shown) = new_header(password, recovery, kdf, &data_key, None)?;
        let image = image_of(&mem.conn)?;
        let bytes = crypt::encode(&header, &data_key, &image)?;
        drop(image);
        // Release the plain file before replacing it (Windows cannot replace an open file).
        let plain = std::mem::replace(&mut self.conn, mem.conn);
        drop(plain);
        let file = match write_atomic(&path, &bytes, false, false) {
            Ok(f) => f,
            Err(e) => {
                // Back to the plain file, unchanged.
                if let Ok(c) = Connection::open(&path)
                    && let Ok(s) = Self::configure(c)
                {
                    self.conn = s.conn;
                }
                return Err(e);
            }
        };
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(sibling(&path, suffix));
        }
        self.enc = Some(Encrypted {
            path,
            file,
            header,
            data_key,
            last: bytes,
            fail_next_save: false,
        });
        Ok(shown)
    }

    /// Replace the encrypted file with a plain book in place and delete `<file>.prev`.
    pub fn remove_encryption(&mut self, current: &Secret, actor: &str) -> StoreResult<()> {
        self.verify(current)?;
        // A plain book keeps no credentials: they are deleted (connections then ask to
        // be reconnected).
        self.conn.pragma_update(None, "secure_delete", "ON")?;
        self.write(|tx| {
            tx.tx.execute("DELETE FROM secret", [])?;
            session_audit(tx, actor, "book.decrypted", serde_json::json!({}))
        })?;
        let path = self.path.clone().ok_or(StoreError::NotEncrypted)?;
        let image = image_of(&self.conn)?;
        let new_file = write_atomic(&path, &image, false, false)?;
        drop(image);
        drop(new_file);
        let _ = std::fs::remove_file(prev_path(&path));
        self.enc = None;
        let conn = Connection::open(&path)?;
        let plain = Self::configure(conn)?;
        self.conn = plain.conn;
        Ok(())
    }
}
