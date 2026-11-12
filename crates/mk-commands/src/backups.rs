//! Automatic backups and restore (backups.md): where backups go and what they are
//! called, making one (copy, check, rename, second place, rotation), listing them,
//! and restoring one as a separate copy. The commands `book.backup`, `book.backups`
//! and `book.restore_copy` run here, outside any transaction: a backup is files, not
//! a change to the book, and writes no audit entry.

use std::path::{Path, PathBuf};

use mk_store::{BackupSource, Secret, Store, StoreError, Unlock};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

use crate::{CommandError, CommandResult};

/// The extension of book files and backups.
const EXT: &str = mk_core::brand::BOOK_EXTENSION;

/// Why a backup was made (recorded in its file name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Opened,
    BeforeUpdate,
    Closed,
    Daily,
    Manual,
}

impl Reason {
    pub const ALL: [Reason; 5] = [
        Reason::Opened,
        Reason::BeforeUpdate,
        Reason::Closed,
        Reason::Daily,
        Reason::Manual,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Opened => "opened",
            Reason::BeforeUpdate => "before-update",
            Reason::Closed => "closed",
            Reason::Daily => "daily",
            Reason::Manual => "manual",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }

    /// Kept by their own count rather than by age (rotation).
    fn special(self) -> bool {
        matches!(self, Reason::BeforeUpdate | Reason::Manual)
    }
}

fn default_true() -> bool {
    true
}
fn d10() -> u32 {
    10
}
fn d7() -> u32 {
    7
}
fn d12() -> u32 {
    12
}
fn d14() -> u32 {
    14
}

/// The app settings for backups (`backup.*`; never stored in a book).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupSettings {
    /// The backup folder; `None` = `<user folder>\MoneyKeep Backups` (backups.md
    /// revision 3; it was `Documents\MoneyKeep Backups` before).
    #[serde(default)]
    pub folder: Option<String>,
    /// A second folder each backup is copied to (USB drive, cloud folder).
    #[serde(default)]
    pub second_place: Option<String>,
    #[serde(default = "default_true")]
    pub on_open: bool,
    #[serde(default = "default_true")]
    pub on_close: bool,
    #[serde(default = "default_true")]
    pub daily: bool,
    #[serde(default = "d10")]
    pub keep_recent: u32,
    #[serde(default = "d7")]
    pub keep_days: u32,
    #[serde(default = "d12")]
    pub keep_months: u32,
    #[serde(default = "d10")]
    pub keep_per_reason: u32,
    #[serde(default = "d10")]
    pub suggest_second_every: u32,
    #[serde(default = "d14")]
    pub warn_days: u32,
}

impl Default for BackupSettings {
    fn default() -> Self {
        Self {
            folder: None,
            second_place: None,
            on_open: true,
            on_close: true,
            daily: true,
            keep_recent: 10,
            keep_days: 7,
            keep_months: 12,
            keep_per_reason: 10,
            suggest_second_every: 10,
            warn_days: 14,
        }
    }
}

impl BackupSettings {
    /// Settings from JSON (missing or invalid fields take their defaults).
    pub fn from_json(v: &Value) -> Self {
        serde_json::from_value(v.clone()).unwrap_or_default()
    }

    /// The settings in the desktop app's per-user config (`config.json`, key
    /// `backup`), so the command line backs up to the same folders; defaults when
    /// there is none.
    pub fn from_app_config() -> Self {
        directories::ProjectDirs::from("org", mk_core::brand::FOLDER, mk_core::brand::FOLDER)
            .and_then(|d| std::fs::read_to_string(d.config_dir().join("config.json")).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .map(|v| Self::from_json(v.get("backup").unwrap_or(&Value::Null)))
            .unwrap_or_default()
            .with_old_default_rule()
    }

    /// The old-default rule of backups.md revision 3: a setting still holding the
    /// old default (`Documents\MoneyKeep Backups`, whether left empty or written out)
    /// keeps it only when backups were written there; otherwise it moves to the new
    /// default. A folder the user chose is kept.
    pub fn with_old_default_rule(mut self) -> Self {
        self.folder =
            resolve_folder_setting(self.folder.as_deref(), old_default_folder().as_deref());
        self
    }

    /// The backup folder in use.
    pub fn folder_path(&self) -> Option<PathBuf> {
        match self.folder.as_deref().map(str::trim) {
            Some(f) if !f.is_empty() => Some(PathBuf::from(f)),
            _ => default_folder(),
        }
    }

    fn second_path(&self) -> Option<PathBuf> {
        self.second_place
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    }
}

/// The name of the backup folder.
fn folder_name() -> String {
    format!("{} Backups", mk_core::brand::PRODUCT)
}

/// `<user folder>\MoneyKeep Backups` (backups.md revision 3: Windows
/// `C:\Users\<name>\MoneyKeep Backups`, macOS and Linux `~/MoneyKeep Backups`). Not in
/// Documents, which Windows' Controlled folder access may protect.
pub fn default_folder() -> Option<PathBuf> {
    let dirs = directories::UserDirs::new()?;
    Some(default_folder_in(dirs.home_dir()))
}

/// The default backup folder for the user folder `home`.
pub fn default_folder_in(home: &Path) -> PathBuf {
    home.join(folder_name())
}

/// The default before revision 3: `Documents\MoneyKeep Backups` (`None` when there is
/// no documents folder apart from the home folder).
pub fn old_default_folder() -> Option<PathBuf> {
    let dirs = directories::UserDirs::new()?;
    let docs = dirs.document_dir()?;
    (docs != dirs.home_dir()).then(|| docs.join(folder_name()))
}

/// Whether `root` holds at least one backup MoneyKeep made (a file matching the name
/// pattern in a book's subfolder).
pub fn has_backups(root: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(root) else {
        return false;
    };
    rd.filter_map(Result::ok).any(|book| {
        let stem = book.file_name().to_string_lossy().into_owned();
        std::fs::read_dir(book.path()).is_ok_and(|files| {
            files.filter_map(Result::ok).any(|f| {
                f.file_name()
                    .to_str()
                    .is_some_and(|n| parse_name(&stem, n).is_some())
                    && f.metadata().is_ok_and(|m| m.is_file())
            })
        })
    })
}

fn same_folder(a: &Path, b: &Path) -> bool {
    norm(a) == norm(b)
}

/// A path for comparing: `\` separators, lower case, no trailing separator.
fn norm(p: &Path) -> String {
    p.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// The backup-folder setting after the old-default rule (backups.md revision 3):
/// empty or the old default `old` → `old` written out when backups were made there,
/// else `None` (the new default); any other folder is kept.
pub fn resolve_folder_setting(folder: Option<&str>, old: Option<&Path>) -> Option<String> {
    let chosen = folder.map(str::trim).filter(|f| !f.is_empty());
    let Some(old) = old else {
        return chosen.map(str::to_owned);
    };
    match chosen {
        Some(f) if !same_folder(Path::new(f), old) => Some(f.to_owned()),
        _ => has_backups(old).then(|| old.display().to_string()),
    }
}

/// The folders Windows' Controlled folder access protects by default (Microsoft,
/// "Protect important folders with controlled folder access",
/// learn.microsoft.com/defender-endpoint/controlled-folders): Documents, Pictures,
/// Videos, Music, Favorites and Desktop. Empty on other systems.
pub fn protected_folders() -> Vec<PathBuf> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let Some(d) = directories::UserDirs::new() else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = [
        d.document_dir(),
        d.picture_dir(),
        d.video_dir(),
        d.audio_dir(),
        d.desktop_dir(),
    ]
    .into_iter()
    .flatten()
    .map(Path::to_path_buf)
    .collect();
    out.push(d.home_dir().join("Favorites"));
    out
}

fn under(path: &Path, root: &Path) -> bool {
    let (p, r) = (norm(path), norm(root));
    !r.is_empty()
        && (p == r
            || p.strip_prefix(&r)
                .is_some_and(|rest| rest.starts_with('\\')))
}

/// The explanation of a folder Windows blocked (backups.md revision 3): an "access
/// denied" or "not found" failure in a folder inside one of `protected`.
pub fn blocked_message(
    folder: &Path,
    kind: std::io::ErrorKind,
    protected: &[PathBuf],
) -> Option<String> {
    let denied = matches!(
        kind,
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
    );
    (denied && protected.iter().any(|r| under(folder, r))).then(|| {
        format!(
            "Windows blocked MoneyKeep from writing to {}. This is usually Windows' \
             ransomware protection (Controlled folder access).",
            folder.display()
        )
    })
}

/// A failure to write in the backup folder `root`: `backup_blocked` with the
/// explanation when Windows blocked it, else `backup_folder` in plain words.
fn folder_error(root: &Path, e: &std::io::Error, env: &BackupEnv) -> CommandError {
    match blocked_message(root, e.kind(), &env.protected()) {
        Some(m) => coded(BLOCKED, m),
        None => coded(
            "backup_folder",
            format!("can't write to the backup folder {}: {e}", root.display()),
        ),
    }
}

/// The error code of a folder Windows blocked.
pub const BLOCKED: &str = "backup_blocked";

/// Try writing (and deleting) a small file in `folder` before it is saved as the
/// backup folder or the second place (backups.md revision 3). Errors:
/// `backup_blocked` (Windows blocked it, with the explanation) or `backup_folder`.
pub fn test_folder(folder: &Path, env: &BackupEnv) -> CommandResult<()> {
    let name = format!(
        ".moneykeep-test-{}-{}.tmp",
        std::process::id(),
        OffsetDateTime::now_utc().unix_timestamp_nanos()
    );
    let probe = folder.join(name);
    let written = match env.simulate_folder_error {
        Some(kind) => Err(std::io::Error::new(kind, "simulated failure")),
        None => (|| {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&probe)?;
            f.write_all(b"MoneyKeep checks that it can write here.")?;
            f.sync_all()?;
            drop(f);
            std::fs::remove_file(&probe)
        })(),
    };
    written.map_err(|e| {
        let _ = std::fs::remove_file(&probe);
        match blocked_message(folder, e.kind(), &env.protected()) {
            Some(m) => coded(BLOCKED, m),
            None => coded(
                "backup_folder",
                format!("MoneyKeep can't write to {}: {e}", folder.display()),
            ),
        }
    })
}

/// Copy `src` to `dst` through a handle opened for writing and flush it to disk
/// before closing (backups.md revision 4: on Windows, flushing needs write access,
/// so a read-only handle cannot flush). `dst` is created or truncated.
pub fn copy_flushed(src: &Path, dst: &Path) -> std::io::Result<u64> {
    let mut from = std::fs::File::open(src)?;
    let mut to = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(dst)?;
    let n = std::io::copy(&mut from, &mut to)?;
    to.sync_all()?;
    Ok(n)
}

/// Everything a backup needs besides the book: the settings, the local time zone
/// and (tests) a fixed clock and a simulated failure.
#[derive(Debug, Clone, Default)]
pub struct BackupEnv {
    pub settings: BackupSettings,
    /// The user's UTC offset (file names use local time).
    pub utc_offset_secs: i32,
    /// Unix seconds to use instead of the clock (tests).
    pub now_unix: Option<i64>,
    /// Damage each copy before its check (tests of BK9).
    pub fail_check: bool,
    /// Fail writing in the backup folder (and the folder test) with this error kind
    /// (tests of BK17 and BK18).
    pub simulate_folder_error: Option<std::io::ErrorKind>,
    /// Fail the copy to the second place after its folder was made (tests of BK21).
    pub fail_second: bool,
    /// The folders Windows protects, instead of [`protected_folders`] (tests).
    pub protected_override: Option<Vec<PathBuf>>,
}

impl BackupEnv {
    pub fn new(settings: BackupSettings, utc_offset_secs: i32) -> Self {
        Self {
            settings,
            utc_offset_secs,
            now_unix: None,
            fail_check: false,
            simulate_folder_error: None,
            fail_second: false,
            protected_override: None,
        }
    }

    fn protected(&self) -> Vec<PathBuf> {
        self.protected_override
            .clone()
            .unwrap_or_else(protected_folders)
    }

    /// The desktop app's backup settings (backup folder, second place) and the local
    /// time zone: what every program that opens a book uses for its before-update
    /// backup (backups.md revision 2).
    pub fn from_app_config() -> Self {
        Self::new(
            BackupSettings::from_app_config(),
            time::UtcOffset::current_local_offset().map_or(0, time::UtcOffset::whole_seconds),
        )
    }

    fn now_unix(&self) -> i64 {
        self.now_unix
            .unwrap_or_else(|| OffsetDateTime::now_utc().unix_timestamp())
    }

    /// Local wall-clock time of `unix`.
    fn local(&self, unix: i64) -> PrimitiveDateTime {
        let t = OffsetDateTime::from_unix_timestamp(unix + i64::from(self.utc_offset_secs))
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        PrimitiveDateTime::new(t.date(), t.time())
    }

    fn unix_of(&self, local: PrimitiveDateTime) -> i64 {
        local.assume_utc().unix_timestamp() - i64::from(self.utc_offset_secs)
    }
}

/// Which folder a backup is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Folder,
    Second,
}

impl Place {
    fn as_str(self) -> &'static str {
        match self {
            Place::Folder => "folder",
            Place::Second => "second_place",
        }
    }
}

/// One backup file.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub at: PrimitiveDateTime,
    pub reason: Reason,
    pub size: u64,
    pub place: Place,
}

fn stamp(at: PrimitiveDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}{:02}",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute()
    )
}

fn iso(at: PrimitiveDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute()
    )
}

impl Entry {
    pub fn to_json(&self, env: &BackupEnv) -> Value {
        json!({
            "path": self.path.display().to_string(),
            "file": self.path.file_name().and_then(|f| f.to_str()).unwrap_or_default(),
            "at": iso(self.at),
            "unix": env.unix_of(self.at),
            "reason": self.reason.as_str(),
            "size": self.size,
            "place": self.place.as_str(),
            "encrypted": self.is_encrypted(),
        })
    }

    /// Whether the backup file is password-protected (an unreadable file counts as
    /// protected: it is not offered for protecting).
    pub fn is_encrypted(&self) -> bool {
        mk_store::book_kind(&self.path).map_or(true, |k| k == mk_store::BookKind::Encrypted)
    }
}

/// The book's name for its backups: the book file's name without the extension.
pub fn book_stem(book: &Path) -> String {
    book.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Book")
        .to_owned()
}

/// `<book> <YYYY-MM-DD HHMM> <reason>.moneykeep`, with ` 2`, ` 3`… before the
/// extension when that name is taken.
pub fn file_name(stem: &str, at: PrimitiveDateTime, reason: Reason, n: u32) -> String {
    let count = if n > 1 {
        format!(" {n}")
    } else {
        String::new()
    };
    format!("{stem} {} {}{count}.{EXT}", stamp(at), reason.as_str())
}

/// The backup file name for a backup made at Unix time `unix` (tests and reviews
/// that place backups made earlier).
pub fn file_name_at(stem: &str, unix: i64, utc_offset_secs: i32, reason: Reason) -> String {
    let env = BackupEnv::new(BackupSettings::default(), utc_offset_secs);
    file_name(stem, env.local(unix), reason, 1)
}

/// `<book> restored <YYYY-MM-DD HHMM>.moneykeep` (the time of the backup).
pub fn restored_file_name(stem: &str, at: Option<PrimitiveDateTime>) -> String {
    match at {
        Some(at) => format!("{stem} restored {}.{EXT}", stamp(at)),
        None => format!("{stem} restored.{EXT}"),
    }
}

fn parse_stamp(date: &str, hhmm: &str) -> Option<PrimitiveDateTime> {
    let mut d = date.split('-');
    let (y, m, day) = (d.next()?, d.next()?, d.next()?);
    if d.next().is_some() || y.len() != 4 || m.len() != 2 || day.len() != 2 || hhmm.len() != 4 {
        return None;
    }
    if !hhmm.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let date = Date::from_calendar_date(
        y.parse().ok()?,
        Month::try_from(m.parse::<u8>().ok()?).ok()?,
        day.parse().ok()?,
    )
    .ok()?;
    let time = Time::from_hms(
        hhmm.get(..2)?.parse().ok()?,
        hhmm.get(2..)?.parse().ok()?,
        0,
    )
    .ok()?;
    Some(PrimitiveDateTime::new(date, time))
}

/// The time and reason in a backup's file name, when it matches the pattern for
/// book `stem` (`None` for any other file).
pub fn parse_name(stem: &str, name: &str) -> Option<(PrimitiveDateTime, Reason)> {
    let rest = name.strip_prefix(stem)?.strip_prefix(' ')?;
    let rest = rest.strip_suffix(&format!(".{EXT}"))?;
    let parts: Vec<&str> = rest.split(' ').collect();
    let (date, hhmm, reason, n) = match parts.as_slice() {
        [d, t, r] => (*d, *t, *r, None),
        [d, t, r, n] => (*d, *t, *r, Some(*n)),
        _ => return None,
    };
    if let Some(n) = n
        && (n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) || n.starts_with('0'))
    {
        return None;
    }
    Some((parse_stamp(date, hhmm)?, Reason::parse(reason)?))
}

/// The backups of book `stem` in `dir` (only files matching the pattern).
fn scan(dir: &Path, stem: &str, place: Place) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            let (at, reason) = parse_name(stem, &name)?;
            let meta = e.metadata().ok().filter(std::fs::Metadata::is_file)?;
            Some(Entry {
                path: e.path(),
                at,
                reason,
                size: meta.len(),
                place,
            })
        })
        .collect();
    newest_first(&mut out);
    out
}

fn newest_first(v: &mut [Entry]) {
    v.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.path.cmp(&a.path)));
}

/// The backups of `book` in the backup folder and the second place, newest first.
pub fn list(book: &Path, env: &BackupEnv) -> Vec<Entry> {
    let stem = book_stem(book);
    let mut out = Vec::new();
    if let Some(f) = env.settings.folder_path() {
        out.extend(scan(&f.join(&stem), &stem, Place::Folder));
    }
    if let Some(s) = env.settings.second_path() {
        out.extend(scan(&s.join(&stem), &stem, Place::Second));
    }
    newest_first(&mut out);
    out
}

/// The backups to remove from one place's list (newest first): the 10 most recent,
/// the newest of each of the last 7 days and of the last 12 months are kept among
/// the ordinary backups (opened, closed, daily); `before-update` and `manual`
/// backups are kept by their own count (the newest 10 of each). Decided for the
/// spec's "kept until there are more than 10 of each, then the oldest of that
/// reason is removed": the age rules do not keep those two reasons, so there are
/// never more than 10 of each.
pub fn plan_rotation(entries: &[Entry], today: Date, s: &BackupSettings) -> Vec<usize> {
    let mut keep = vec![false; entries.len()];
    let ordinary: Vec<usize> = (0..entries.len())
        .filter(|i| entries.get(*i).is_some_and(|e| !e.reason.special()))
        .collect();
    for i in ordinary.iter().take(s.keep_recent as usize) {
        if let Some(k) = keep.get_mut(*i) {
            *k = true;
        }
    }
    let mut day = today;
    for _ in 0..s.keep_days {
        if let Some(i) = ordinary
            .iter()
            .find(|i| entries.get(**i).is_some_and(|e| e.at.date() == day))
            && let Some(k) = keep.get_mut(*i)
        {
            *k = true;
        }
        match day.previous_day() {
            Some(d) => day = d,
            None => break,
        }
    }
    let (mut y, mut m) = (today.year(), today.month());
    for _ in 0..s.keep_months {
        if let Some(i) = ordinary.iter().find(|i| {
            entries
                .get(**i)
                .is_some_and(|e| e.at.year() == y && e.at.month() == m)
        }) && let Some(k) = keep.get_mut(*i)
        {
            *k = true;
        }
        if m == Month::January {
            y -= 1;
        }
        m = m.previous();
    }
    for reason in [Reason::BeforeUpdate, Reason::Manual] {
        for (i, _) in entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.reason == reason)
            .take(s.keep_per_reason as usize)
        {
            if let Some(k) = keep.get_mut(i) {
                *k = true;
            }
        }
    }
    keep.iter()
        .enumerate()
        .filter(|(_, k)| !**k)
        .map(|(i, _)| i)
        .collect()
}

/// Remove what the rules do not keep in `dir`; returns how many were removed.
/// Leftover temporary copies of this book (`.<backup name>.part`) go too.
fn rotate(dir: &Path, stem: &str, env: &BackupEnv, place: Place) -> usize {
    let entries = scan(dir, stem, place);
    let today = env.local(env.now_unix()).date();
    let mut removed = 0;
    for i in plan_rotation(&entries, today, &env.settings) {
        if let Some(e) = entries.get(i)
            && std::fs::remove_file(&e.path).is_ok()
        {
            removed += 1;
        }
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            let ours = name
                .strip_prefix('.')
                .and_then(|n| n.strip_suffix(".part"))
                .is_some_and(|n| parse_name(stem, n).is_some());
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|d| d.as_secs() > 86_400);
            if ours && old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    removed
}

fn coded(code: &'static str, message: impl Into<String>) -> CommandError {
    CommandError::Coded {
        code,
        message: message.into(),
    }
}

fn temp_name(final_path: &Path) -> PathBuf {
    let name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    final_path.with_file_name(format!(".{name}.part"))
}

/// A temporary name beside `final_path` that no earlier attempt left behind
/// (`.<name>.<process>-<time>.part`).
fn unique_temp_name(final_path: &Path) -> PathBuf {
    let name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    final_path.with_file_name(format!(".{name}.{}-{nanos}.part", std::process::id()))
}

/// A free name in `dir` for a new backup.
fn free_name(dir: &Path, stem: &str, at: PrimitiveDateTime, reason: Reason) -> PathBuf {
    let mut n = 1;
    loop {
        let p = dir.join(file_name(stem, at, reason, n));
        if !p.exists() && !temp_name(&p).exists() || n > 999 {
            return p;
        }
        n += 1;
    }
}

/// Damage a copy (tests of the check): the first bytes are overwritten.
fn damage(path: &Path) {
    if let Ok(mut b) = std::fs::read(path) {
        for x in b.iter_mut().take(64) {
            *x = 0x5A;
        }
        let _ = std::fs::write(path, b);
    }
}

/// A backup that was made.
#[derive(Debug, Clone)]
pub struct Made {
    pub path: PathBuf,
    pub size: u64,
    pub at: PrimitiveDateTime,
    pub unix: i64,
    pub reason: Reason,
    pub encrypted: bool,
    pub second_place_path: Option<PathBuf>,
    /// Why the copy to the second place was skipped (not reachable, not writable).
    pub second_place_error: Option<String>,
    /// The copy to the second place failed for another reason than the folder not
    /// being there (backups.md revision 4: shown, unlike an unplugged drive).
    pub second_place_failed: Option<String>,
    pub removed: usize,
}

impl Made {
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path.display().to_string(),
            "size": self.size,
            "at": iso(self.at),
            "unix": self.unix,
            "reason": self.reason.as_str(),
            "encrypted": self.encrypted,
            "second_place_path": self.second_place_path.as_ref().map(|p| p.display().to_string()),
            "second_place_error": self.second_place_error,
            "second_place_failed": self.second_place_failed,
            "removed": self.removed,
        })
    }
}

/// Make a backup of the book at `book` from `source`: a complete copy written
/// under a temporary name in `<folder>\<book>\`, checked (a copy that fails its
/// check is deleted and reported), renamed into place, copied to the second place
/// when it is reachable, then rotated in each place.
///
/// Errors: `backup_folder` (the folder cannot be made or written), `backup_check`
/// (the copy failed its check), `backup_failed` (anything else).
pub fn make(
    source: &BackupSource,
    book: &Path,
    reason: Reason,
    env: &BackupEnv,
) -> CommandResult<Made> {
    let stem = book_stem(book);
    let root = env.settings.folder_path().ok_or_else(|| {
        coded(
            "backup_folder",
            "there is no backup folder; choose one in Settings → Backups",
        )
    })?;
    let dir = root.join(&stem);
    if let Some(kind) = env.simulate_folder_error {
        return Err(folder_error(
            &root,
            &std::io::Error::new(kind, "simulated failure"),
            env,
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|e| folder_error(&root, &e, env))?;
    let unix = env.now_unix();
    let at = env.local(unix);
    let path = free_name(&dir, &stem, at, reason);
    let tmp = temp_name(&path);
    // Reserve the temporary name (an empty file): this is where an unwritable
    // folder shows.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| folder_error(&root, &e, env))?;
    let written = source
        .write_to(&tmp)
        .map_err(|e| coded("backup_failed", format!("couldn't write the copy: {e}")));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if env.fail_check {
        damage(&tmp);
    }
    if let Err(e) = source.check(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(coded(
            "backup_check",
            format!("the copy failed its check and was deleted: {e}"),
        ));
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(coded(
            "backup_failed",
            format!("couldn't finish the copy: {e}"),
        ));
    }
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let mut removed = rotate(&dir, &stem, env, Place::Folder);
    let (mut second_place_path, mut second_place_error, mut second_place_failed) =
        (None, None, None);
    if let Some(second) = env.settings.second_path() {
        match copy_to_second(&path, &second, &stem, env) {
            Ok(p) => {
                removed += rotate(&second.join(&stem), &stem, env, Place::Second);
                second_place_path = Some(p);
            }
            Err(SecondErr::Unreachable(e)) => second_place_error = Some(e),
            Err(SecondErr::Failed(e)) => {
                second_place_error = Some(e.clone());
                second_place_failed = Some(e);
            }
        }
    }
    Ok(Made {
        path,
        size,
        at,
        unix,
        reason,
        encrypted: source.is_encrypted(),
        second_place_path,
        second_place_error,
        second_place_failed,
        removed,
    })
}

/// Why a copy to the second place was not made.
enum SecondErr {
    /// The folder isn't there (an unplugged drive): skipped quietly.
    Unreachable(String),
    /// Anything else: shown (backups.md revision 4).
    Failed(String),
}

/// Copy a finished backup to `<second>\<book>\` (temporary name, flushed through a
/// writable handle, then renamed). An unplugged drive (the folder is missing) is
/// skipped quietly; other failures are reported as such.
fn copy_to_second(
    backup: &Path,
    second: &Path,
    stem: &str,
    env: &BackupEnv,
) -> Result<PathBuf, SecondErr> {
    if !second.is_dir() {
        return Err(SecondErr::Unreachable(format!(
            "{} isn't reachable",
            second.display()
        )));
    }
    let failed = |e: &std::io::Error| {
        SecondErr::Failed(
            blocked_message(second, e.kind(), &env.protected()).unwrap_or_else(|| e.to_string()),
        )
    };
    let dir = second.join(stem);
    std::fs::create_dir_all(&dir).map_err(|e| failed(&e))?;
    let name = backup
        .file_name()
        .ok_or_else(|| SecondErr::Failed("no file name".into()))?;
    let target = dir.join(name);
    let tmp = temp_name(&target);
    let done = if env.fail_second {
        Err(std::io::Error::other("simulated write failure"))
    } else {
        copy_flushed(backup, &tmp).and_then(|_| std::fs::rename(&tmp, &target))
    };
    if let Err(e) = done {
        let _ = std::fs::remove_file(&tmp);
        return Err(failed(&e));
    }
    Ok(target)
}

/// A restored copy.
#[derive(Debug, Clone)]
pub struct Restored {
    pub path: PathBuf,
    pub encrypted: bool,
}

/// Copy `backup` to `target` (which must not exist), check the copy and return
/// it; the backup and the original book are never changed. A backup made by a newer
/// MoneyKeep is refused (`too_new`); an encrypted one needs its password
/// (`password_required`, `wrong_password`) unless `open_book` is the same book.
///
/// The copy never replaces a file (security-review-1 B1): `target` is reserved by
/// creating it, the copy is written under a temporary name and moved into place
/// with a move that refuses to replace; if the name was taken in the meantime, the
/// copy goes to the next free name (`… restored … (2).moneykeep`), which
/// [`Restored::path`] gives.
pub fn restore_copy(
    backup: &Path,
    target: &Path,
    secret: Option<&Secret>,
    open_book: Option<&BackupSource>,
) -> CommandResult<Restored> {
    restore_copy_with(backup, target, secret, open_book, |_| {})
}

/// [`restore_copy`], calling `during` with the reserved name while the copy is
/// written (tests: another program taking the name meanwhile).
pub fn restore_copy_with(
    backup: &Path,
    target: &Path,
    secret: Option<&Secret>,
    open_book: Option<&BackupSource>,
    during: impl FnOnce(&Path),
) -> CommandResult<Restored> {
    let io = |e: &std::io::Error| {
        CommandError::from(StoreError::Io {
            action: "create",
            path: target.display().to_string(),
            message: e.to_string(),
        })
    };
    if target.exists() {
        return Err(StoreError::AlreadyExists(target.display().to_string()).into());
    }
    let unlock = match (secret, open_book) {
        (Some(s), _) => Unlock::Secret(s),
        (None, Some(b)) => Unlock::Source(b),
        (None, None) => Unlock::None,
    };
    let checked = mk_store::check_backup(backup, unlock).map_err(too_new)?;
    let reserved = mk_store::newfile::reserve(target).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            StoreError::AlreadyExists(target.display().to_string()).into()
        } else {
            io(&e)
        }
    })?;
    let tmp = unique_temp_name(target);
    let copied = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|_| copy_flushed(backup, &tmp))
        .map_err(|e| io(&e));
    during(reserved.path());
    let checked_copy = copied.and_then(|_| {
        let unlock = checked.key.as_ref().map_or(Unlock::None, Unlock::Key);
        mk_store::check_backup(&tmp, unlock).map_err(too_new)?;
        Ok(())
    });
    let placed = match checked_copy {
        Ok(()) => reserved.fill_from(&tmp).map_err(|e| io(&e)),
        Err(e) => {
            reserved.release();
            Err(e)
        }
    };
    match placed {
        Ok(path) => Ok(Restored {
            path,
            encrypted: checked.encrypted,
        }),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

// ----- earlier plain backups of a protected book (security-review-1 B2) ------------------

/// The backups of `book` in the backup folder and the second place that aren't
/// password-protected, newest first.
pub fn unprotected(book: &Path, env: &BackupEnv) -> Vec<Entry> {
    list(book, env)
        .into_iter()
        .filter(|e| !e.is_encrypted())
        .collect()
}

/// "3 earlier backups of this book aren't password-protected."
pub fn unprotected_text(n: usize) -> String {
    if n == 1 {
        "1 earlier backup of this book isn't password-protected.".to_owned()
    } else {
        format!("{n} earlier backups of this book aren't password-protected.")
    }
}

/// What Protect them now or Delete them did.
#[derive(Debug, Clone, Default)]
pub struct Handled {
    /// Backups now encrypted (Protect) or deleted (Delete).
    pub done: Vec<PathBuf>,
    /// Backups left as they were, with the reason.
    pub failed: Vec<(PathBuf, String)>,
}

impl Handled {
    pub fn to_json(&self) -> Value {
        json!({
            "done": self.done.len(),
            "paths": self.done.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "failed": self.failed.iter().map(|(p, why)| json!({
                "path": p.display().to_string(),
                "reason": why,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Protect them now: each plain backup of `book` is written again encrypted like
/// `source` (the open, protected book: same header and data key, so it opens with
/// the book's password and recovery key), the encrypted copy is checked, then it
/// takes the plain copy's place (the plain file is gone once this returns). A
/// backup that fails is left as it was and reported.
pub fn protect(source: &BackupSource, book: &Path, env: &BackupEnv) -> CommandResult<Handled> {
    if !source.is_encrypted() {
        return Err(coded(
            "not_encrypted",
            "this book isn't password-protected; add a password first",
        ));
    }
    let mut out = Handled::default();
    for e in unprotected(book, env) {
        match protect_one(source, &e.path) {
            Ok(()) => out.done.push(e.path),
            Err(why) => out.failed.push((e.path, why)),
        }
    }
    Ok(out)
}

fn protect_one(source: &BackupSource, plain: &Path) -> Result<(), String> {
    let enc = source.protecting(plain).map_err(|e| e.to_string())?;
    let tmp = unique_temp_name(plain);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| e.to_string())?;
    let done = enc
        .write_to(&tmp)
        .and_then(|()| enc.check(&tmp).map(|_| ()))
        .map_err(|e| format!("the protected copy failed its check: {e}"))
        .and_then(|()| {
            // Still the plain backup that was protected (not replaced meanwhile).
            match mk_store::book_kind(plain) {
                Ok(mk_store::BookKind::Plain) => {
                    std::fs::rename(&tmp, plain).map_err(|e| e.to_string())
                }
                _ => Err("the backup changed while it was protected".to_owned()),
            }
        });
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

/// Delete them: the plain backups of `book` are deleted (the protected ones stay).
pub fn delete_unprotected(book: &Path, env: &BackupEnv) -> Handled {
    let mut out = Handled::default();
    for e in unprotected(book, env) {
        match std::fs::remove_file(&e.path) {
            Ok(()) => out.done.push(e.path),
            Err(err) => out.failed.push((e.path, err.to_string())),
        }
    }
    out
}

fn too_new(e: StoreError) -> CommandError {
    match e {
        StoreError::TooNew { found, supported } => coded(
            "too_new",
            format!(
                "This backup was made by a newer MoneyKeep: it needs a MoneyKeep that \
                 reads book format {found} (this one reads up to format {supported}). \
                 Update MoneyKeep to restore it."
            ),
        ),
        other => other.into(),
    }
}

// ----- the registry commands -----------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BackupArgs {
    /// `manual` (default), `opened`, `closed`, `daily` or `before-update`.
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListArgs {
    /// A book file (default: the open book).
    #[serde(default)]
    pub book: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreArgs {
    /// The backup file.
    pub backup: String,
    /// Where to save the copy (must not exist).
    pub target: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoArgs {}

pub(crate) const COMMANDS: [&str; 6] = [
    "book.backup",
    "book.backups",
    "book.restore_copy",
    "book.unprotected_backups",
    "book.protect_backups",
    "book.delete_unprotected_backups",
];

pub(crate) fn is_backup(name: &str) -> bool {
    COMMANDS.contains(&name)
}

fn not_here(_: &crate::Ctx<'_>, _: Value) -> CommandResult<Value> {
    Err(CommandError::Integrity(
        "backup commands run outside a transaction".into(),
    ))
}

pub(crate) fn register(r: &mut crate::Registry) {
    fn add<A: serde::de::DeserializeOwned + JsonSchema + 'static>(
        r: &mut crate::Registry,
        name: &'static str,
        summary: &'static str,
    ) {
        r.add(name, summary, false, |ctx, _: A| not_here(ctx, Value::Null));
    }
    add::<BackupArgs>(
        r,
        "book.backup",
        "Back up the open book now (to the backup folder and the second place)",
    );
    add::<ListArgs>(r, "book.backups", "List a book's backups, newest first");
    add::<RestoreArgs>(
        r,
        "book.restore_copy",
        "Copy a backup to a new file and check it (restore never replaces a book)",
    );
    add::<ListArgs>(
        r,
        "book.unprotected_backups",
        "List a book's backups that aren't password-protected, newest first",
    );
    add::<NoArgs>(
        r,
        "book.protect_backups",
        "Protect the open book's earlier plain backups with its password (each is encrypted, checked, then the plain copy deleted)",
    );
    add::<NoArgs>(
        r,
        "book.delete_unprotected_backups",
        "Delete the open book's backups that aren't password-protected",
    );
}

fn env_of(registry: &crate::Registry) -> CommandResult<&BackupEnv> {
    registry
        .backup_env()
        .ok_or_else(|| coded("unsupported", "backups are not set up in this front end"))
}

/// Run a backup command on `store`.
pub(crate) fn run(
    registry: &crate::Registry,
    store: &mut Store,
    name: &str,
    args: Value,
) -> CommandResult<Value> {
    let env = env_of(registry)?;
    match name {
        "book.backup" => {
            let a: BackupArgs = crate::parse_args(args)?;
            let reason = match a.reason.as_deref() {
                None => Reason::Manual,
                Some(r) => Reason::parse(r)
                    .ok_or_else(|| CommandError::InvalidArgs(format!("unknown reason {r:?}")))?,
            };
            let book = store
                .path()
                .map(Path::to_path_buf)
                .ok_or_else(|| coded("backup_failed", "this book has no file"))?;
            let stamp = store.change_stamp()?;
            let source = store.backup_source()?;
            let made = make(&source, &book, reason, env)?;
            store.mark_backed_up(stamp);
            Ok(made.to_json())
        }
        "book.backups" => {
            let a: ListArgs = crate::parse_args(args)?;
            let book = match a.book {
                Some(b) => PathBuf::from(b),
                None => store
                    .path()
                    .map(Path::to_path_buf)
                    .ok_or_else(|| CommandError::InvalidArgs("give a book file".into()))?,
            };
            Ok(Value::Array(
                list(&book, env).iter().map(|e| e.to_json(env)).collect(),
            ))
        }
        "book.restore_copy" => {
            let a: RestoreArgs = crate::parse_args(args)?;
            let source = store.backup_source().ok();
            let r = restore_copy(
                Path::new(&a.backup),
                Path::new(&a.target),
                None,
                source.as_ref(),
            )?;
            Ok(json!({"path": r.path.display().to_string(), "encrypted": r.encrypted}))
        }
        "book.unprotected_backups" => {
            let a: ListArgs = crate::parse_args(args)?;
            let book = match a.book {
                Some(b) => PathBuf::from(b),
                None => store
                    .path()
                    .map(Path::to_path_buf)
                    .ok_or_else(|| CommandError::InvalidArgs("give a book file".into()))?,
            };
            let rows: Vec<Value> = unprotected(&book, env)
                .iter()
                .map(|e| e.to_json(env))
                .collect();
            Ok(
                json!({"count": rows.len(), "message": unprotected_text(rows.len()), "backups": rows}),
            )
        }
        "book.protect_backups" | "book.delete_unprotected_backups" => {
            let _: NoArgs = crate::parse_args(args)?;
            let book = store
                .path()
                .map(Path::to_path_buf)
                .ok_or_else(|| coded("backup_failed", "this book has no file"))?;
            let done = if name == "book.protect_backups" {
                protect(&store.backup_source()?, &book, env)?
            } else {
                delete_unprotected(&book, env)
            };
            Ok(done.to_json())
        }
        _ => Err(CommandError::UnknownCommand(name.to_owned())),
    }
}

/// Back up a book file that is not open, without opening it (no format update, no
/// change to the file): the start screen's **Back up this book…** (backups.md
/// revision 2). A password-protected book needs `secret` (`password_required`,
/// `wrong_password`); its backup stays protected with the same password.
pub fn back_up_file(
    book: &Path,
    secret: Option<&Secret>,
    reason: Reason,
    env: &BackupEnv,
) -> CommandResult<Made> {
    let source = mk_store::backup_source_of_file(book, secret)?;
    make(&source, book, reason, env)
}

/// **The** way a program opens a book file (backups.md revision 2): like
/// [`Store::open_with`], but when the book's format must be updated a
/// `before-update` backup is made first with `env` (no other backup is made). If
/// that backup fails the book is not updated, its file is left exactly as it was and
/// opening fails with `backup_failed` ("MoneyKeep couldn't make a backup before
/// updating this book, so it wasn't changed: <reason>."). Returns the backup made,
/// if one was.
///
/// The desktop app, the command line, `keep-mcp`, the folder watcher and
/// [`crate::Session`]'s opening functions all come here; the plain `Store::open` is
/// left for tests and for opening backup copies (a guard test, BK14, checks this).
pub fn open_book(
    path: &Path,
    secret: Option<&Secret>,
    env: &BackupEnv,
) -> CommandResult<(Store, Option<Made>)> {
    let mut made = None;
    let store = Store::open_with_hook(path, secret, |src| -> CommandResult<()> {
        made = Some(before_update_backup(src, path, env)?);
        Ok(())
    })?;
    Ok((store, made))
}

/// Before updating a book's format: the mandatory `before-update` backup, or the
/// error opening stops with (`backup_failed`).
pub fn before_update_backup(
    source: &BackupSource,
    book: &Path,
    env: &BackupEnv,
) -> CommandResult<Made> {
    make(source, book, Reason::BeforeUpdate, env).map_err(|e| {
        let reason = match &e {
            CommandError::Coded { message, .. } => message.clone(),
            other => other.to_string(),
        };
        coded(
            "backup_failed",
            format!(
                "MoneyKeep couldn't make a backup before updating this book, so it wasn't \
                 changed: {reason}."
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn at(s: &str) -> PrimitiveDateTime {
        let (d, t) = s.split_once(' ').unwrap();
        parse_stamp(d, t).unwrap()
    }

    #[test]
    fn names_round_trip_and_other_files_do_not_match() {
        let t = at("2026-11-07 1420");
        let n = file_name("Household", t, Reason::Opened, 1);
        assert_eq!(n, "Household 2026-11-07 1420 opened.moneykeep");
        assert_eq!(parse_name("Household", &n), Some((t, Reason::Opened)));
        let n2 = file_name("Household", t, Reason::Manual, 2);
        assert_eq!(n2, "Household 2026-11-07 1420 manual 2.moneykeep");
        assert_eq!(parse_name("Household", &n2), Some((t, Reason::Manual)));
        for other in [
            "Household.moneykeep",
            "Household 2026-11-07 1420 opened.moneykeep.part",
            "Household 2026-11-07 1420 copied.moneykeep",
            "Household 2026-13-07 1420 opened.moneykeep",
            "Household 2026-11-07 2520 opened.moneykeep",
            "Household 2026-11-07 1420 opened 0.moneykeep",
            "Other 2026-11-07 1420 opened.moneykeep",
            "notes.txt",
        ] {
            assert_eq!(parse_name("Household", other), None, "{other}");
        }
        assert_eq!(
            restored_file_name("Household", Some(t)),
            "Household restored 2026-11-07 1420.moneykeep"
        );
    }
}
