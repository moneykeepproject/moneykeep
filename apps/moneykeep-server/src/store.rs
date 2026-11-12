//! The inbox: one SQLite file in the data folder. Expense fields, receipt bytes and
//! the lists document are encrypted at rest with XChaCha20-Poly1305 (the primitive of
//! `book-encryption.md`) under a 256-bit key kept in the operating system's credential
//! store, or in `inbox.key` (owner-only) where there is none. Tokens are stored only
//! as SHA-256 hashes.

use std::path::{Path, PathBuf};

use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use mk_capture_api::{
    ACK_RETENTION_SECS, COLLECTED_SHOWN, Device, ExpenseFields, InboxItem, ReceiptMeta, Submission,
    SubmissionStatus, TripAck, TripFields, TripInboxItem, TripStatus, TripSubmission,
};
use mk_core::brand;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const DB_FILE: &str = "inbox.sqlite";
pub const KEY_FILE: &str = "inbox.key";
/// Receipts never attached to a submission are removed after this long.
const ORPHAN_SECS: i64 = 7 * 24 * 3600;

pub type StoreResult<T> = Result<T, String>;

fn db_err(e: rusqlite::Error) -> String {
    format!("inbox database error: {e}")
}

/// Where the inbox key lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyHome {
    /// The OS credential store when it works, else `inbox.key`.
    Auto,
    /// Always `inbox.key` (tests, or by choice).
    File,
}

/// Random bytes from the operating system.
pub fn random<const N: usize>() -> StoreResult<[u8; N]> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|e| format!("no random source: {e}"))?;
    Ok(b)
}

/// A new id: 16 random bytes as 32 lower-case hex digits.
pub fn new_id() -> StoreResult<String> {
    Ok(hex(&random::<16>()?))
}

pub fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

/// SHA-256 of a token (only hashes are stored).
pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// A new bearer token: 32 random bytes, Base64URL without padding.
pub fn new_token() -> StoreResult<Zeroizing<String>> {
    let b = Zeroizing::new(random::<32>()?);
    Ok(Zeroizing::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(*b),
    ))
}

/// Equal byte strings, compared without stopping at the first difference.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a
        .iter()
        .zip(b)
        .fold(0u8, |acc, (x, y)| acc | std::hint::black_box(x ^ y));
    std::hint::black_box(diff) == 0
}

thread_local! {
    /// [`cut_writes`]: tests cut this thread's writes of one file short.
    static CUT: std::cell::RefCell<Option<(String, usize)>> = const { std::cell::RefCell::new(None) };
}

/// Tests (upgrade-resilience-1 UR-2): every write of a file named `name` on this
/// thread stops after `bytes` bytes with an error, as a full disk or a power cut
/// would; `None` ends it.
#[doc(hidden)]
pub fn cut_writes(name: Option<&str>, bytes: usize) {
    CUT.with(|c| *c.borrow_mut() = name.map(|n| (n.to_owned(), bytes)));
}

/// The temporary name a write goes to first: hidden, random, in the same folder.
fn temp_name(path: &Path) -> StoreResult<PathBuf> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| format!("cannot write {}: no file name", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    Ok(dir.join(format!(".{name}.{}.tmp", hex(&random::<6>()?))))
}

/// Write `bytes` to `path` readable by the owner only (on Unix), so that a failed or
/// cut-short write leaves the old file as it was (upgrade-resilience-1 U1.2): the
/// bytes go to a new temporary file in the same folder, are flushed to the disk
/// (`FlushFileBuffers` on Windows), and the file is then renamed over `path` (on
/// Windows `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`). A link at `path` is
/// replaced, never written through.
pub fn write_private(path: &Path, bytes: &[u8]) -> StoreResult<()> {
    use std::io::Write;
    let err = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
    let tmp = temp_name(path)?;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(&tmp).map_err(err)?;
    let cut = CUT.with(|c| {
        c.borrow().as_ref().and_then(|(name, n)| {
            path.file_name()
                .is_some_and(|f| f.to_string_lossy() == name.as_str())
                .then_some(*n)
        })
    });
    let written = match cut {
        Some(n) => f
            .write_all(bytes.get(..n.min(bytes.len())).unwrap_or(&[]))
            .and_then(|()| Err(std::io::Error::other("the write was cut short"))),
        None => f.write_all(bytes),
    }
    .and_then(|()| f.sync_all());
    drop(f);
    if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err(e));
    }
    // The rename reaches the disk with its folder (Unix).
    #[cfg(unix)]
    {
        let dir = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

fn keyring_id(dir: &Path) -> String {
    let canon = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let h = Sha256::digest(canon.to_string_lossy().as_bytes());
    format!("capture-inbox:{}", hex(h.get(..8).unwrap_or(&[])))
}

fn decode_key(text: &str) -> StoreResult<Zeroizing<[u8; 32]>> {
    let raw = Zeroizing::new(
        base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .map_err(|_| "the inbox key is damaged".to_owned())?,
    );
    let arr: [u8; 32] = raw
        .as_slice()
        .try_into()
        .map_err(|_| "the inbox key is damaged".to_owned())?;
    Ok(Zeroizing::new(arr))
}

/// The inbox key sealed by the operating system (security-review-1 L5): on Windows
/// with DPAPI for the machine, so the service's own account can open it and a copy
/// of the file taken elsewhere cannot; on macOS with a key in the user's login
/// keychain (macos-release.md C4).
pub const SEALED_KEY_FILE: &str = "inbox.key.sealed";

/// Seals and opens the inbox key file (security-review-1 L5). Windows: DPAPI with
/// machine scope ([`crate::services::windows::Dpapi`]); macOS: the login keychain
/// ([`crate::secrets::KeychainSealer`]); elsewhere there is none and `inbox.key`
/// stays an owner-only file.
pub trait KeySealer: Send + Sync {
    fn seal(&self, plain: &[u8]) -> StoreResult<Vec<u8>>;
    fn unseal(&self, sealed: &[u8]) -> StoreResult<Zeroizing<Vec<u8>>>;
}

/// This system's sealer for the data folder `dir`, if it has one.
pub fn platform_sealer(dir: &Path) -> Option<Box<dyn KeySealer>> {
    if cfg!(windows) {
        Some(Box::new(crate::services::windows::Dpapi))
    } else if cfg!(target_os = "macos") {
        Some(Box::new(crate::secrets::KeychainSealer::new(
            dir,
            std::sync::Arc::new(crate::secrets::OsService(crate::secrets::KEYCHAIN_SERVICE)),
        )))
    } else {
        None
    }
}

/// Load the inbox key, creating it on first use.
pub fn load_key(dir: &Path, home: KeyHome) -> StoreResult<Zeroizing<[u8; 32]>> {
    let sealer = platform_sealer(dir);
    load_key_with(dir, home, sealer.as_deref())
}

fn write_sealed(dir: &Path, sealer: &dyn KeySealer, text: &str) -> StoreResult<()> {
    let sealed = sealer.seal(text.as_bytes())?;
    write_private(&dir.join(SEALED_KEY_FILE), &sealed)
}

/// upgrade-resilience-1 U1.2: no key in any of its places opens; the server refuses
/// to start rather than make a new one.
pub const KEY_UNOPENABLE: &str = "The inbox key can't be opened.";

/// The tables whose rows make an inbox worth keeping (everything but `meta`).
const DATA_TABLES: [&str; 6] = ["token", "pairing", "receipt", "submission", "trip", "lists"];

/// U1.2: the inbox database in `dir` holds rows (paired phones, the desktop token,
/// expenses…), so a new key would lose them. A database that can't be read counts
/// as holding rows.
pub fn inbox_has_rows(dir: &Path) -> bool {
    let path = dir.join(DB_FILE);
    match std::fs::metadata(&path) {
        Err(_) => return false,
        Ok(m) if m.len() == 0 => return false,
        Ok(_) => {}
    }
    let Ok(conn) = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return true;
    };
    DATA_TABLES.iter().any(|t| {
        let exists = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![t],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(1)
            > 0;
        exists
            && conn
                .query_row(&format!("SELECT EXISTS (SELECT 1 FROM {t})"), [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap_or(1)
                != 0
    })
}

/// The plain `inbox.key` file's text and key, when it is there and opens.
fn plain_key(file: &Path) -> Option<(Zeroizing<String>, Zeroizing<[u8; 32]>)> {
    let text = Zeroizing::new(std::fs::read_to_string(file).ok()?);
    let key = decode_key(&text).ok()?;
    Some((Zeroizing::new(text.trim().to_owned()), key))
}

/// [`load_key`] with a given sealer. With one, the key file is never plain text: a
/// plain `inbox.key` left from before is sealed and then removed.
///
/// upgrade-resilience-1 U1.2: when `inbox.key.sealed` doesn't open (damaged, cut
/// short) and a plain `inbox.key` that opens is present, the plain one is used and
/// sealed again; when no key opens, [`KEY_UNOPENABLE`]. A new key is made only for
/// an inbox without rows.
pub fn load_key_with(
    dir: &Path,
    home: KeyHome,
    sealer: Option<&dyn KeySealer>,
) -> StoreResult<Zeroizing<[u8; 32]>> {
    let file = dir.join(KEY_FILE);
    let sealed_file = dir.join(SEALED_KEY_FILE);
    if let Some(sealer) = sealer
        && sealed_file.exists()
    {
        let opened = std::fs::read(&sealed_file)
            .map_err(|e| e.to_string())
            .and_then(|blob| sealer.unseal(&blob))
            .and_then(|plain| {
                let text = Zeroizing::new(String::from_utf8_lossy(&plain).into_owned());
                decode_key(&text)
            });
        return match opened {
            Ok(key) => Ok(key),
            Err(_) => match plain_key(&file) {
                Some((text, key)) => {
                    write_sealed(dir, sealer, &text)?;
                    let _ = std::fs::remove_file(&file);
                    Ok(key)
                }
                None => Err(KEY_UNOPENABLE.into()),
            },
        };
    }
    if file.exists() {
        let (text, key) = plain_key(&file).ok_or(KEY_UNOPENABLE)?;
        if let Some(sealer) = sealer {
            write_sealed(dir, sealer, &text)?;
            std::fs::remove_file(&file)
                .map_err(|e| format!("cannot remove {}: {e}", file.display()))?;
        }
        return Ok(key);
    }
    let entry = match home {
        KeyHome::Auto => keyring::Entry::new(brand::CREDENTIAL_SERVICE, &keyring_id(dir)).ok(),
        KeyHome::File => None,
    };
    if let Some(e) = &entry {
        match e.get_password() {
            Ok(text) => {
                return decode_key(&Zeroizing::new(text)).map_err(|_| KEY_UNOPENABLE.into());
            }
            Err(keyring::Error::NoEntry) => {}
            Err(_) => {}
        }
    }
    // Never a new key for an inbox that holds rows: they could not be read again.
    if inbox_has_rows(dir) {
        return Err(KEY_UNOPENABLE.into());
    }
    let key = Zeroizing::new(random::<32>()?);
    let text = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(*key));
    let in_store = entry.is_some_and(|e| {
        e.set_password(&text).is_ok() && e.get_password().ok().as_deref() == Some(text.as_str())
    });
    if !in_store {
        match sealer {
            Some(sealer) => write_sealed(dir, sealer, &text)?,
            None => write_private(&file, text.as_bytes())?,
        }
    }
    Ok(key)
}

/// Where the inbox key is kept, in words.
pub fn key_location(dir: &Path) -> &'static str {
    if dir.join(SEALED_KEY_FILE).exists() {
        if cfg!(target_os = "macos") {
            "inbox.key.sealed file (sealed with a key in your login keychain)"
        } else {
            "inbox.key.sealed file (sealed by Windows for this computer)"
        }
    } else if dir.join(KEY_FILE).exists() {
        "inbox.key file"
    } else {
        "credential store"
    }
}

/// What a bearer token belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRow {
    pub id: String,
    /// `desktop` or `device`.
    pub kind: String,
    pub name: String,
    pub revoked: bool,
}

/// The result of a pairing attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairOutcome {
    Paired,
    Wrong,
    /// The code was right but the Capture PIN was not (revision 10): counted
    /// toward the code's failure limit.
    PinWrong,
}

/// A stored receipt's owner and state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptRow {
    pub device_id: String,
    pub submission_id: Option<String>,
    pub collected: bool,
    pub kind: String,
}

pub struct Store {
    conn: Connection,
    key: Zeroizing<[u8; 32]>,
    path: PathBuf,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS token (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    hash BLOB NOT NULL UNIQUE,
    name TEXT NOT NULL,
    paired_at INTEGER NOT NULL,
    last_seen INTEGER,
    revoked INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS pairing (
    id INTEGER PRIMARY KEY,
    hash BLOB NOT NULL,
    expires_at INTEGER NOT NULL,
    wrong INTEGER NOT NULL DEFAULT 0,
    done INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS receipt (
    id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    kind TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    width INTEGER NOT NULL,
    height INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    data BLOB,
    submission_id TEXT
);
CREATE TABLE IF NOT EXISTS submission (
    id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    status TEXT NOT NULL,
    collected_at INTEGER,
    delete_after INTEGER,
    fields BLOB,
    receipt_ids TEXT NOT NULL,
    UNIQUE (device_id, client_id)
);
CREATE TABLE IF NOT EXISTS trip (
    id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    status TEXT NOT NULL,
    done_at INTEGER,
    delete_after INTEGER,
    fields BLOB,
    result BLOB,
    UNIQUE (device_id, client_id)
);
CREATE TABLE IF NOT EXISTS lists (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    book_id TEXT NOT NULL,
    data BLOB NOT NULL,
    published_at INTEGER NOT NULL
);
";

/// upgrade-resilience-1 U1.4: the inbox database's schema number this program knows
/// (`meta.schema`), the last of [`MIGRATIONS`].
pub const SCHEMA_VERSION: i64 = 2;

/// The numbered migrations: 1 the tables, 2 each phone's way in (revision 10).
const MIGRATIONS: [i64; 2] = [1, 2];

/// The refusal for an inbox made by a newer MoneyKeep (U1.4).
pub const NEWER_DATA: &str =
    "This phone capture data was made by a newer MoneyKeep. Install that version or a newer one.";

/// The schema number of the inbox in `dir`, read without its key (`None`: no inbox
/// yet). An inbox from before the numbers counts as 1; a number that isn't one, as
/// newer than any.
pub fn schema_of(dir: &Path) -> StoreResult<Option<i64>> {
    let path = dir.join(DB_FILE);
    if std::fs::metadata(&path).ok().is_none_or(|m| m.len() == 0) {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(db_err)?;
    let has_meta = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(db_err)?
        > 0;
    if !has_meta {
        return Ok(None);
    }
    let v: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| {
            r.get(0)
        })
        .optional()
        .map_err(db_err)?;
    Ok(v.map(|v| v.trim().parse::<i64>().unwrap_or(i64::MAX)))
}

/// Migration `n`, then `meta.schema = n`. Each can run again safely (an inbox from
/// before the numbers already has some of them).
fn migrate(conn: &Connection, n: i64) -> StoreResult<()> {
    match n {
        1 => conn.execute_batch(SCHEMA).map_err(db_err)?,
        2 => {
            // Revision 10: each phone's way in. Phones paired before came through
            // the Cloudflare address (or a manual setup of it).
            let has_via = conn
                .prepare("SELECT 1 FROM pragma_table_info('token') WHERE name = 'via'")
                .and_then(|mut st| st.exists([]))
                .map_err(db_err)?;
            if !has_via {
                conn.execute(
                    "ALTER TABLE token ADD COLUMN via TEXT NOT NULL DEFAULT 'cloudflare'",
                    [],
                )
                .map_err(db_err)?;
            }
        }
        _ => return Err(format!("unknown inbox migration {n}")),
    }
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('schema', ?1)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![n.to_string()],
    )
    .map(|_| ())
    .map_err(db_err)
}

impl Store {
    /// Open (creating if needed) the inbox in `dir`. An inbox whose schema is newer
    /// than [`SCHEMA_VERSION`] is refused before its key is touched (U1.4).
    pub fn open(dir: &Path, home: KeyHome) -> StoreResult<Self> {
        if schema_of(dir)?.is_some_and(|v| v > SCHEMA_VERSION) {
            return Err(NEWER_DATA.into());
        }
        let key = load_key(dir, home)?;
        let path = dir.join(DB_FILE);
        let fresh = !path.exists();
        if fresh {
            write_private(&path, &[])?;
        }
        let conn = Connection::open(&path).map_err(db_err)?;
        conn.execute_batch(
            "PRAGMA journal_mode = DELETE; PRAGMA secure_delete = ON; PRAGMA foreign_keys = ON;",
        )
        .map_err(db_err)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(db_err)?;
        let at = schema_of(dir)?.unwrap_or(0);
        for m in MIGRATIONS.into_iter().filter(|m| *m > at) {
            migrate(&conn, m)?;
        }
        Ok(Self { conn, key, path })
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(self.key.as_ref().into())
    }

    /// Encrypt with a fresh random nonce; the result is nonce ‖ ciphertext.
    fn seal(&self, aad: &str, plain: &[u8]) -> StoreResult<Vec<u8>> {
        let nonce = random::<24>()?;
        let ct = self
            .cipher()
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plain,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| "encryption failed".to_owned())?;
        let mut out = nonce.to_vec();
        out.extend(ct);
        Ok(out)
    }

    fn open_blob(&self, aad: &str, blob: &[u8]) -> StoreResult<Vec<u8>> {
        let (nonce, ct) = blob
            .split_at_checked(24)
            .ok_or_else(|| "stored data is damaged".to_owned())?;
        self.cipher()
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ct,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| "stored data is damaged or the inbox key is wrong".to_owned())
    }

    // ----- tokens --------------------------------------------------------------------

    /// Replace the desktop token's hash.
    pub fn set_desktop_token(&self, hash: &[u8], now: i64) -> StoreResult<()> {
        self.conn
            .execute("DELETE FROM token WHERE kind = 'desktop'", [])
            .map_err(db_err)?;
        self.conn
            .execute(
                "INSERT INTO token (id, kind, hash, name, paired_at) VALUES (?1, 'desktop', ?2, 'desktop', ?3)",
                params![new_id()?, hash, now],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn has_desktop_token(&self) -> StoreResult<bool> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM token WHERE kind = 'desktop'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .map_err(db_err)
    }

    /// Find a token by hash. Hashes are compared by the database's index lookup over
    /// fixed-length SHA-256 values of 256-bit random tokens; a final constant-time
    /// comparison guards the match itself.
    pub fn find_token(&self, hash: &[u8]) -> StoreResult<Option<TokenRow>> {
        let row: Option<(String, String, Vec<u8>, String, i64)> = self
            .conn
            .query_row(
                "SELECT id, kind, hash, name, revoked FROM token WHERE hash = ?1",
                params![hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
            .map_err(db_err)?;
        Ok(row.and_then(|(id, kind, stored, name, revoked)| {
            ct_eq(&stored, hash).then_some(TokenRow {
                id,
                kind,
                name,
                revoked: revoked != 0,
            })
        }))
    }

    pub fn touch(&self, id: &str, now: i64) -> StoreResult<()> {
        self.conn
            .execute(
                "UPDATE token SET last_seen = ?2 WHERE id = ?1",
                params![id, now],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// A newly paired phone; `via` is its way in (`wifi` or `cloudflare`).
    pub fn add_device(&self, name: &str, hash: &[u8], now: i64, via: &str) -> StoreResult<String> {
        let id = new_id()?;
        self.conn
            .execute(
                "INSERT INTO token (id, kind, hash, name, paired_at, via) VALUES (?1, 'device', ?2, ?3, ?4, ?5)",
                params![id, hash, name, now, via],
            )
            .map_err(db_err)?;
        Ok(id)
    }

    /// The Capture PIN's stored hash (revision 10), if one is set.
    pub fn pin(&self) -> StoreResult<Option<String>> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = 'pin'", [], |r| r.get(0))
            .optional()
            .map_err(db_err)
    }

    /// Keep the Capture PIN's hash (never the PIN).
    pub fn set_pin(&self, hash: &str) -> StoreResult<()> {
        self.conn
            .execute(
                "INSERT INTO meta (key, value) VALUES ('pin', ?1)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![hash],
            )
            .map(|_| ())
            .map_err(db_err)
    }

    pub fn devices(&self) -> StoreResult<Vec<Device>> {
        let mut st = self
            .conn
            .prepare(
                "SELECT id, name, paired_at, last_seen, via FROM token
                 WHERE kind = 'device' AND revoked = 0 ORDER BY paired_at",
            )
            .map_err(db_err)?;
        let rows = st
            .query_map([], |r| {
                Ok(Device {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    paired_at: r.get(2)?,
                    last_seen: r.get(3)?,
                    via: r.get(4)?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<Result<_, _>>().map_err(db_err)
    }

    /// Revoke a phone; false when there is no such phone.
    pub fn revoke(&self, id: &str) -> StoreResult<bool> {
        self.conn
            .execute(
                "UPDATE token SET revoked = 1 WHERE id = ?1 AND kind = 'device' AND revoked = 0",
                params![id],
            )
            .map(|n| n > 0)
            .map_err(db_err)
    }

    // ----- pairing -------------------------------------------------------------------

    /// A new pairing code (earlier unused codes stop working).
    pub fn new_pairing(&self, hash: &[u8], expires_at: i64) -> StoreResult<()> {
        self.conn
            .execute("UPDATE pairing SET done = 1 WHERE done = 0", [])
            .map_err(db_err)?;
        self.conn
            .execute(
                "INSERT INTO pairing (hash, expires_at) VALUES (?1, ?2)",
                params![hash, expires_at],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Try a code against the active one: right → used up; wrong → counted, and the
    /// fifth wrong try ends the code.
    pub fn try_pair(&self, hash: &[u8], now: i64, max_wrong: u32) -> StoreResult<PairOutcome> {
        self.try_pair_pin(hash, now, max_wrong, true)
    }

    /// [`Store::try_pair`] with the Capture PIN's check (revision 10): a right code
    /// with a wrong PIN is not used up but counts as a wrong try, and the fifth wrong
    /// try (code or PIN) ends the code.
    pub fn try_pair_pin(
        &self,
        hash: &[u8],
        now: i64,
        max_wrong: u32,
        pin_ok: bool,
    ) -> StoreResult<PairOutcome> {
        let active: Option<(i64, Vec<u8>, i64)> = self
            .conn
            .query_row(
                "SELECT id, hash, wrong FROM pairing WHERE done = 0 AND expires_at > ?1
                 ORDER BY id DESC LIMIT 1",
                params![now],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(db_err)?;
        let Some((id, stored, wrong)) = active else {
            return Ok(PairOutcome::Wrong);
        };
        let code_ok = ct_eq(&stored, hash);
        if code_ok && pin_ok {
            self.conn
                .execute("UPDATE pairing SET done = 1 WHERE id = ?1", params![id])
                .map_err(db_err)?;
            return Ok(PairOutcome::Paired);
        }
        let wrong = wrong + 1;
        let done = i64::from(wrong >= i64::from(max_wrong));
        self.conn
            .execute(
                "UPDATE pairing SET wrong = ?2, done = ?3 WHERE id = ?1",
                params![id, wrong, done],
            )
            .map_err(db_err)?;
        Ok(if code_ok {
            PairOutcome::PinWrong
        } else {
            PairOutcome::Wrong
        })
    }

    // ----- receipts ------------------------------------------------------------------

    /// Bytes held: receipts not yet purged.
    pub fn inbox_bytes(&self) -> StoreResult<u64> {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(bytes), 0) FROM receipt WHERE data IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| u64::try_from(n).unwrap_or(0))
            .map_err(db_err)
    }

    pub fn add_receipt(
        &self,
        device: &str,
        kind: &str,
        content: &[u8],
        width: u32,
        height: u32,
        now: i64,
    ) -> StoreResult<ReceiptMeta> {
        let id = new_id()?;
        let sha = hex(&Sha256::digest(content));
        let data = self.seal(&format!("receipt:{id}"), content)?;
        self.conn
            .execute(
                "INSERT INTO receipt (id, device_id, created_at, kind, bytes, width, height, sha256, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    device,
                    now,
                    kind,
                    i64::try_from(content.len()).unwrap_or(i64::MAX),
                    width,
                    height,
                    sha,
                    data
                ],
            )
            .map_err(db_err)?;
        Ok(ReceiptMeta {
            id,
            kind: kind.to_owned(),
            bytes: u64::try_from(content.len()).unwrap_or(0),
            width,
            height,
            sha256: sha,
        })
    }

    pub fn receipt(&self, id: &str) -> StoreResult<Option<ReceiptRow>> {
        let row: Option<(String, Option<String>, String)> = self
            .conn
            .query_row(
                "SELECT device_id, submission_id, kind FROM receipt WHERE id = ?1 AND data IS NOT NULL",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(db_err)?;
        let Some((device_id, submission_id, kind)) = row else {
            return Ok(None);
        };
        let collected = match &submission_id {
            Some(s) => self.status_of(s)? == Some(SubmissionStatus::Collected),
            None => false,
        };
        Ok(Some(ReceiptRow {
            device_id,
            submission_id,
            collected,
            kind,
        }))
    }

    pub fn receipt_bytes(&self, id: &str) -> StoreResult<Option<(String, Vec<u8>)>> {
        let row: Option<(String, Option<Vec<u8>>)> = self
            .conn
            .query_row(
                "SELECT kind, data FROM receipt WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(db_err)?;
        match row {
            Some((kind, Some(blob))) => Ok(Some((
                kind,
                self.open_blob(&format!("receipt:{id}"), &blob)?,
            ))),
            _ => Ok(None),
        }
    }

    fn receipt_meta(&self, id: &str) -> StoreResult<Option<ReceiptMeta>> {
        self.conn
            .query_row(
                "SELECT id, kind, bytes, width, height, sha256 FROM receipt WHERE id = ?1 AND data IS NOT NULL",
                params![id],
                |r| {
                    Ok(ReceiptMeta {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        bytes: u64::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
                        width: r.get(3)?,
                        height: r.get(4)?,
                        sha256: r.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(db_err)
    }

    pub fn delete_receipt(&self, id: &str) -> StoreResult<()> {
        self.conn
            .execute("DELETE FROM receipt WHERE id = ?1", params![id])
            .map_err(db_err)?;
        Ok(())
    }

    // ----- submissions ---------------------------------------------------------------

    fn status_of(&self, id: &str) -> StoreResult<Option<SubmissionStatus>> {
        let s: Option<String> = self
            .conn
            .query_row(
                "SELECT status FROM submission WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        Ok(s.map(|s| {
            if s == "collected" {
                SubmissionStatus::Collected
            } else {
                SubmissionStatus::Waiting
            }
        }))
    }

    fn row_to_submission(
        &self,
        r: (
            String,
            String,
            i64,
            String,
            Option<i64>,
            Option<Vec<u8>>,
            String,
        ),
    ) -> StoreResult<Submission> {
        let (id, client_id, created_at, status, collected_at, fields, receipts) = r;
        let fields = match fields {
            Some(b) => serde_json::from_slice(&self.open_blob(&format!("submission:{id}"), &b)?)
                .map_err(|e| e.to_string())?,
            None => ExpenseFields::default(),
        };
        Ok(Submission {
            id,
            client_id,
            status: if status == "collected" {
                SubmissionStatus::Collected
            } else {
                SubmissionStatus::Waiting
            },
            created_at,
            collected_at,
            fields,
            receipt_ids: serde_json::from_str(&receipts).unwrap_or_default(),
        })
    }

    fn query_submissions(
        &self,
        sql: &str,
        p: impl rusqlite::Params,
    ) -> StoreResult<Vec<Submission>> {
        let mut st = self.conn.prepare(sql).map_err(db_err)?;
        let rows = st
            .query_map(p, |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        rows.into_iter()
            .map(|r| self.row_to_submission(r))
            .collect()
    }

    const COLS: &'static str =
        "id, client_id, created_at, status, collected_at, fields, receipt_ids";

    pub fn submission(&self, device: &str, id: &str) -> StoreResult<Option<Submission>> {
        Ok(self
            .query_submissions(
                &format!(
                    "SELECT {} FROM submission WHERE id = ?1 AND device_id = ?2",
                    Self::COLS
                ),
                params![id, device],
            )?
            .pop())
    }

    pub fn by_client_id(&self, device: &str, client_id: &str) -> StoreResult<Option<Submission>> {
        Ok(self
            .query_submissions(
                &format!(
                    "SELECT {} FROM submission WHERE client_id = ?1 AND device_id = ?2",
                    Self::COLS
                ),
                params![client_id, device],
            )?
            .pop())
    }

    fn link_receipts(&self, sub: &str, ids: &[String]) -> StoreResult<()> {
        self.conn
            .execute(
                "UPDATE receipt SET submission_id = NULL WHERE submission_id = ?1",
                params![sub],
            )
            .map_err(db_err)?;
        for r in ids {
            self.conn
                .execute(
                    "UPDATE receipt SET submission_id = ?1 WHERE id = ?2",
                    params![sub, r],
                )
                .map_err(db_err)?;
        }
        Ok(())
    }

    pub fn add_submission(
        &self,
        device: &str,
        client_id: &str,
        fields: &ExpenseFields,
        receipt_ids: &[String],
        now: i64,
    ) -> StoreResult<Submission> {
        let id = new_id()?;
        let plain = serde_json::to_vec(fields).map_err(|e| e.to_string())?;
        let blob = self.seal(&format!("submission:{id}"), &plain)?;
        let ids = serde_json::to_string(receipt_ids).map_err(|e| e.to_string())?;
        self.conn
            .execute(
                "INSERT INTO submission (id, device_id, client_id, created_at, status, fields, receipt_ids)
                 VALUES (?1, ?2, ?3, ?4, 'waiting', ?5, ?6)",
                params![id, device, client_id, now, blob, ids],
            )
            .map_err(db_err)?;
        self.link_receipts(&id, receipt_ids)?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('last_activity', ?1)",
                params![now.to_string()],
            )
            .map_err(db_err)?;
        Ok(Submission {
            id,
            client_id: client_id.to_owned(),
            status: SubmissionStatus::Waiting,
            created_at: now,
            collected_at: None,
            fields: fields.clone(),
            receipt_ids: receipt_ids.to_vec(),
        })
    }

    pub fn update_submission(
        &self,
        id: &str,
        fields: &ExpenseFields,
        receipt_ids: &[String],
    ) -> StoreResult<()> {
        let plain = serde_json::to_vec(fields).map_err(|e| e.to_string())?;
        let blob = self.seal(&format!("submission:{id}"), &plain)?;
        let ids = serde_json::to_string(receipt_ids).map_err(|e| e.to_string())?;
        self.conn
            .execute(
                "UPDATE submission SET fields = ?2, receipt_ids = ?3 WHERE id = ?1 AND status = 'waiting'",
                params![id, blob, ids],
            )
            .map_err(db_err)?;
        self.link_receipts(id, receipt_ids)
    }

    /// Delete a waiting submission and its receipts.
    pub fn delete_submission(&self, id: &str) -> StoreResult<()> {
        self.conn
            .execute("DELETE FROM receipt WHERE submission_id = ?1", params![id])
            .map_err(db_err)?;
        self.conn
            .execute(
                "DELETE FROM submission WHERE id = ?1 AND status = 'waiting'",
                params![id],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Take a receipt out of its waiting submission's list.
    pub fn detach_receipt(&self, sub: &str, receipt: &str) -> StoreResult<()> {
        let ids: Option<String> = self
            .conn
            .query_row(
                "SELECT receipt_ids FROM submission WHERE id = ?1",
                params![sub],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        if let Some(ids) = ids {
            let mut v: Vec<String> = serde_json::from_str(&ids).unwrap_or_default();
            v.retain(|x| x != receipt);
            self.conn
                .execute(
                    "UPDATE submission SET receipt_ids = ?2 WHERE id = ?1",
                    params![sub, serde_json::to_string(&v).map_err(|e| e.to_string())?],
                )
                .map_err(db_err)?;
        }
        Ok(())
    }

    /// A phone's waiting submissions and its last collected ones.
    pub fn for_device(&self, device: &str) -> StoreResult<(Vec<Submission>, Vec<Submission>)> {
        let waiting = self.query_submissions(
            &format!(
                "SELECT {} FROM submission WHERE device_id = ?1 AND status = 'waiting' ORDER BY created_at",
                Self::COLS
            ),
            params![device],
        )?;
        let collected = self.query_submissions(
            &format!(
                "SELECT {} FROM submission WHERE device_id = ?1 AND status = 'collected'
                 ORDER BY collected_at DESC LIMIT {COLLECTED_SHOWN}",
                Self::COLS
            ),
            params![device],
        )?;
        Ok((waiting, collected))
    }

    /// Every waiting submission with its receipts, for the desktop.
    pub fn inbox(&self) -> StoreResult<Vec<InboxItem>> {
        let mut st = self
            .conn
            .prepare(&format!(
                "SELECT {}, s.device_id, COALESCE(t.name, '') FROM submission s
                 LEFT JOIN token t ON t.id = s.device_id
                 WHERE s.status = 'waiting' ORDER BY s.created_at",
                Self::COLS
                    .split(", ")
                    .map(|c| format!("s.{c}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .map_err(db_err)?;
        type Row = (
            (
                String,
                String,
                i64,
                String,
                Option<i64>,
                Option<Vec<u8>>,
                String,
            ),
            String,
            String,
        );
        let rows: Vec<Row> = st
            .query_map([], |r| {
                Ok((
                    (
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ),
                    r.get(7)?,
                    r.get(8)?,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<_, _>>()
            .map_err(db_err)?;
        let mut out = Vec::new();
        for (row, device_id, device_name) in rows {
            let s = self.row_to_submission(row)?;
            let mut receipts = Vec::new();
            for r in &s.receipt_ids {
                if let Some(m) = self.receipt_meta(r)? {
                    receipts.push(m);
                }
            }
            out.push(InboxItem {
                id: s.id,
                client_id: s.client_id,
                device_id,
                device_name,
                submitted_at: s.created_at,
                fields: s.fields,
                receipts,
            });
        }
        Ok(out)
    }

    /// Mark a submission collected; false when there is no such submission.
    pub fn ack(&self, id: &str, now: i64) -> StoreResult<bool> {
        let n = self
            .conn
            .execute(
                "UPDATE submission SET status = 'collected', collected_at = ?2, delete_after = ?3
                 WHERE id = ?1 AND status = 'waiting'",
                params![id, now, now + ACK_RETENTION_SECS],
            )
            .map_err(db_err)?;
        if n > 0 {
            return Ok(true);
        }
        Ok(self.status_of(id)?.is_some())
    }

    // ----- lists ---------------------------------------------------------------------

    pub fn put_lists(&self, book_id: &str, json: &[u8], now: i64) -> StoreResult<()> {
        let blob = self.seal("lists", json)?;
        self.conn
            .execute(
                "INSERT INTO lists (id, book_id, data, published_at) VALUES (1, ?1, ?2, ?3)
                 ON CONFLICT (id) DO UPDATE SET book_id = excluded.book_id, data = excluded.data,
                 published_at = excluded.published_at",
                params![book_id, blob, now],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn lists(&self) -> StoreResult<Option<(String, Vec<u8>, i64)>> {
        let row: Option<(String, Vec<u8>, i64)> = self
            .conn
            .query_row(
                "SELECT book_id, data, published_at FROM lists WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(db_err)?;
        match row {
            Some((b, blob, at)) => Ok(Some((b, self.open_blob("lists", &blob)?, at))),
            None => Ok(None),
        }
    }

    // ----- housekeeping --------------------------------------------------------------

    /// Remove the bytes and fields of submissions acknowledged over 24 hours ago,
    /// orphaned receipts, old pairing codes, and collected entries beyond the last 50
    /// per phone.
    pub fn purge(&self, now: i64) -> StoreResult<()> {
        let c = &self.conn;
        c.execute(
            "DELETE FROM receipt WHERE submission_id IN
               (SELECT id FROM submission WHERE status = 'collected' AND delete_after <= ?1)",
            params![now],
        )
        .map_err(db_err)?;
        c.execute(
            "UPDATE submission SET fields = NULL, receipt_ids = '[]'
             WHERE status = 'collected' AND delete_after <= ?1 AND fields IS NOT NULL",
            params![now],
        )
        .map_err(db_err)?;
        // Trips (revision 11): the fields go after the same 24 hours; the result
        // (reason, the book's amount) stays with the last 50 per phone.
        c.execute(
            "UPDATE trip SET fields = NULL
             WHERE status != 'waiting' AND delete_after <= ?1 AND fields IS NOT NULL",
            params![now],
        )
        .map_err(db_err)?;
        c.execute(
            &format!(
                "DELETE FROM trip WHERE status != 'waiting' AND fields IS NULL AND id NOT IN
                   (SELECT id FROM (SELECT id, ROW_NUMBER() OVER
                      (PARTITION BY device_id ORDER BY done_at DESC) AS n
                    FROM trip WHERE status != 'waiting') WHERE n <= {COLLECTED_SHOWN})"
            ),
            [],
        )
        .map_err(db_err)?;
        c.execute(
            "DELETE FROM receipt WHERE submission_id IS NULL AND created_at <= ?1",
            params![now - ORPHAN_SECS],
        )
        .map_err(db_err)?;
        c.execute(
            "DELETE FROM pairing WHERE expires_at <= ?1",
            params![now - 3600],
        )
        .map_err(db_err)?;
        c.execute(
            &format!(
                "DELETE FROM submission WHERE status = 'collected' AND fields IS NULL AND id NOT IN
                   (SELECT id FROM (SELECT id, ROW_NUMBER() OVER
                      (PARTITION BY device_id ORDER BY collected_at DESC) AS n
                    FROM submission WHERE status = 'collected') WHERE n <= {COLLECTED_SHOWN})"
            ),
            [],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// Unix seconds of the newest submission received (kept after it is collected).
    pub fn last_activity(&self) -> StoreResult<Option<i64>> {
        let v: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'last_activity'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;
        Ok(v.and_then(|t| t.parse().ok()))
    }

    /// Counts for `moneykeep-server status`: (phones, waiting, inbox bytes).
    pub fn counts(&self) -> StoreResult<(usize, usize, u64)> {
        let devices = self.devices()?.len();
        let waiting: i64 = self
            .conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM submission WHERE status = 'waiting')
                      + (SELECT COUNT(*) FROM trip WHERE status = 'waiting')",
                [],
                |r| r.get(0),
            )
            .map_err(db_err)?;
        Ok((
            devices,
            usize::try_from(waiting).unwrap_or(0),
            self.inbox_bytes()?,
        ))
    }
}

// ----- trips (mobile-capture.md revision 11) ---------------------------------------------

/// A trip row as read: id, client id, created, status, done, fields, result.
type TripRow = (
    String,
    String,
    i64,
    String,
    Option<i64>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
);

const TRIP_COLS: &str = "id, client_id, created_at, status, done_at, fields, result";

fn trip_status(s: &str) -> TripStatus {
    match s {
        "taken" => TripStatus::Taken,
        "refused" => TripStatus::Refused,
        _ => TripStatus::Waiting,
    }
}

impl Store {
    fn row_to_trip(&self, r: TripRow) -> StoreResult<TripSubmission> {
        let (id, client_id, created_at, status, done_at, fields, result) = r;
        let fields: TripFields = match fields {
            Some(b) => serde_json::from_slice(&self.open_blob(&format!("trip:{id}"), &b)?)
                .map_err(|e| e.to_string())?,
            None => TripFields::default(),
        };
        let result: TripAck = match result {
            Some(b) => serde_json::from_slice(&self.open_blob(&format!("trip-result:{id}"), &b)?)
                .map_err(|e| e.to_string())?,
            None => TripAck::default(),
        };
        Ok(TripSubmission {
            id,
            client_id,
            status: trip_status(&status),
            created_at,
            done_at,
            reason: result.refused,
            amount: result.amount,
            fields,
        })
    }

    fn query_trips(&self, sql: &str, p: impl rusqlite::Params) -> StoreResult<Vec<TripSubmission>> {
        let mut st = self.conn.prepare(sql).map_err(db_err)?;
        let rows: Vec<TripRow> = st
            .query_map(p, |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })
            .map_err(db_err)?
            .collect::<Result<_, _>>()
            .map_err(db_err)?;
        rows.into_iter().map(|r| self.row_to_trip(r)).collect()
    }

    /// One of this phone's trips.
    pub fn trip(&self, device: &str, id: &str) -> StoreResult<Option<TripSubmission>> {
        Ok(self
            .query_trips(
                &format!("SELECT {TRIP_COLS} FROM trip WHERE id = ?1 AND device_id = ?2"),
                params![id, device],
            )?
            .pop())
    }

    /// This phone's trip with `client_id` (a resend).
    pub fn trip_by_client_id(
        &self,
        device: &str,
        client_id: &str,
    ) -> StoreResult<Option<TripSubmission>> {
        Ok(self
            .query_trips(
                &format!("SELECT {TRIP_COLS} FROM trip WHERE client_id = ?1 AND device_id = ?2"),
                params![client_id, device],
            )?
            .pop())
    }

    /// Store a new trip, its fields encrypted like an expense's.
    pub fn add_trip(
        &self,
        device: &str,
        client_id: &str,
        fields: &TripFields,
        now: i64,
    ) -> StoreResult<TripSubmission> {
        let id = new_id()?;
        let plain = serde_json::to_vec(fields).map_err(|e| e.to_string())?;
        let blob = self.seal(&format!("trip:{id}"), &plain)?;
        self.conn
            .execute(
                "INSERT INTO trip (id, device_id, client_id, created_at, status, fields)
                 VALUES (?1, ?2, ?3, ?4, 'waiting', ?5)",
                params![id, device, client_id, now, blob],
            )
            .map_err(db_err)?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('last_activity', ?1)",
                params![now.to_string()],
            )
            .map_err(db_err)?;
        Ok(TripSubmission {
            id,
            client_id: client_id.to_owned(),
            status: TripStatus::Waiting,
            created_at: now,
            done_at: None,
            reason: None,
            amount: None,
            fields: fields.clone(),
        })
    }

    /// Replace a waiting trip's fields.
    pub fn update_trip(&self, id: &str, fields: &TripFields) -> StoreResult<()> {
        let plain = serde_json::to_vec(fields).map_err(|e| e.to_string())?;
        let blob = self.seal(&format!("trip:{id}"), &plain)?;
        self.conn
            .execute(
                "UPDATE trip SET fields = ?2 WHERE id = ?1 AND status = 'waiting'",
                params![id, blob],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Delete a waiting trip.
    pub fn delete_trip(&self, id: &str) -> StoreResult<()> {
        self.conn
            .execute(
                "DELETE FROM trip WHERE id = ?1 AND status = 'waiting'",
                params![id],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// A phone's waiting trips and its last ones taken or refused.
    pub fn trips_for_device(
        &self,
        device: &str,
    ) -> StoreResult<(Vec<TripSubmission>, Vec<TripSubmission>)> {
        let waiting = self.query_trips(
            &format!(
                "SELECT {TRIP_COLS} FROM trip WHERE device_id = ?1 AND status = 'waiting'
                 ORDER BY created_at"
            ),
            params![device],
        )?;
        let done = self.query_trips(
            &format!(
                "SELECT {TRIP_COLS} FROM trip WHERE device_id = ?1 AND status != 'waiting'
                 ORDER BY done_at DESC LIMIT {COLLECTED_SHOWN}"
            ),
            params![device],
        )?;
        Ok((waiting, done))
    }

    /// Every waiting trip, for the desktop.
    pub fn trip_inbox(&self) -> StoreResult<Vec<TripInboxItem>> {
        let mut st = self
            .conn
            .prepare(
                "SELECT t.id, t.device_id, COALESCE(k.name, '') FROM trip t
                 LEFT JOIN token k ON k.id = t.device_id
                 WHERE t.status = 'waiting' ORDER BY t.created_at",
            )
            .map_err(db_err)?;
        let rows: Vec<(String, String, String)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(db_err)?
            .collect::<Result<_, _>>()
            .map_err(db_err)?;
        let mut out = Vec::new();
        for (id, device_id, device_name) in rows {
            if let Some(t) = self.trip(&device_id, &id)? {
                out.push(TripInboxItem {
                    id: t.id,
                    client_id: t.client_id,
                    device_id,
                    device_name,
                    submitted_at: t.created_at,
                    fields: t.fields,
                });
            }
        }
        Ok(out)
    }

    /// Mark a waiting trip taken (with the book's amount) or refused (with the
    /// reason); false when there is no such trip. Acknowledging again is harmless.
    pub fn ack_trip(&self, id: &str, ack: &TripAck, now: i64) -> StoreResult<bool> {
        let plain = serde_json::to_vec(ack).map_err(|e| e.to_string())?;
        let blob = self.seal(&format!("trip-result:{id}"), &plain)?;
        let status = if ack.refused.is_some() {
            "refused"
        } else {
            "taken"
        };
        let n = self
            .conn
            .execute(
                "UPDATE trip SET status = ?2, done_at = ?3, delete_after = ?4, result = ?5
                 WHERE id = ?1 AND status = 'waiting'",
                params![id, status, now, now + ACK_RETENTION_SECS, blob],
            )
            .map_err(db_err)?;
        if n > 0 {
            return Ok(true);
        }
        let known: Option<i64> = self
            .conn
            .query_row("SELECT 1 FROM trip WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(db_err)?;
        Ok(known.is_some())
    }
}
