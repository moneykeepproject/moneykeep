//! The watched-folder importer (desktop/server only, not part of the wasm build),
//! used by `moneykeep watch <book>` and the desktop app.
//!
//! Every new or changed file with a supported extension in the watch folder that has
//! been unchanged for the settle time is staged with actor `watch`, then moved to the
//! archive folder; files that fail to parse go to `failed/` with a `.error.txt` note.
//! The loop never touches files outside the watch folder and never deletes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use mk_commands::{Actor, CommandError, Registry, auto_committable};
use mk_store::Store;
use serde_json::{Value, json};

pub const EXTENSIONS: &[&str] = &["ofx", "qfx", "qbo", "qif", "csv"];

/// What happened to one file.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Staged {
        file: PathBuf,
        batch: String,
        committed: bool,
    },
    Failed {
        file: PathBuf,
        error: String,
    },
    /// The book was not opened for this run: its format must be updated and the
    /// mandatory backup before the update failed (backups.md revision 2). The book
    /// is unchanged; opening is tried again on the next run.
    Skipped {
        file: PathBuf,
        error: String,
    },
}

/// Settings read from the book for each pass.
struct Config {
    folder: PathBuf,
    archive: PathBuf,
    auto_commit: bool,
}

fn config(registry: &Registry, store: &mut Store) -> Result<Config, String> {
    let actor = Actor("watch".into());
    let get = |store: &mut Store, key: &str| -> Result<Value, String> {
        registry
            .invoke(
                store,
                &actor,
                "setting.get",
                json!({"module": "core", "key": key}),
            )
            .map(|v| v.get("value").cloned().unwrap_or(Value::Null))
            .map_err(|e| e.to_string())
    };
    let folder = get(store, "import.watch_folder")?
        .as_str()
        .map(PathBuf::from)
        .ok_or("setting import.watch_folder is not set (use setting.set module core)")?;
    let archive = get(store, "import.watch_archive")?
        .as_str()
        .map_or_else(|| folder.join("imported"), PathBuf::from);
    let auto_commit = get(store, "import.auto_commit")? == Value::Bool(true);
    Ok(Config {
        folder,
        archive,
        auto_commit,
    })
}

/// State kept between passes: size and modification time seen, and since when.
#[derive(Debug, Default)]
pub struct Watcher {
    seen: HashMap<PathBuf, (u64, Option<SystemTime>, Instant)>,
    pub settle: Duration,
    /// Why the last run did nothing (the book was skipped), for the watcher's status.
    status: Option<String>,
}

impl Watcher {
    pub fn new(settle: Duration) -> Self {
        Self {
            seen: HashMap::new(),
            settle,
            status: None,
        }
    }

    /// The watcher's status: why the last run skipped the book, if it did.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Open the watched (plain) book the way every program opens one
    /// ([`mk_commands::backups::open_book`]): only the mandatory `before-update`
    /// backup is made, with `env`. When that backup fails, the book is skipped for
    /// this run, unchanged: `Ok(Err(outcome))` with the message (also kept as the
    /// status). Other errors (the book cannot be opened at all) are `Err`.
    pub fn open_book(
        &mut self,
        path: &Path,
        env: &mk_commands::backups::BackupEnv,
    ) -> Result<Result<Store, Outcome>, String> {
        match mk_commands::backups::open_book(path, None, env) {
            Ok((store, _)) => {
                self.status = None;
                Ok(Ok(store))
            }
            Err(e) if e.code() == "backup_failed" => {
                let error = e.to_string();
                self.status = Some(error.clone());
                Ok(Err(Outcome::Skipped {
                    file: path.to_path_buf(),
                    error,
                }))
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Files seen but not yet settled.
    pub fn pending(&self) -> usize {
        self.seen.len()
    }

    /// One scan of the folder. Files that have settled are processed.
    pub fn pass(&mut self, registry: &Registry, store: &mut Store) -> Result<Vec<Outcome>, String> {
        let cfg = config(registry, store)?;
        let entries =
            std::fs::read_dir(&cfg.folder).map_err(|e| format!("{}: {e}", cfg.folder.display()))?;
        let now = Instant::now();
        let mut ready = Vec::new();
        let mut present = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() || !supported(&path) {
                continue;
            }
            present.push(path.clone());
            let key = (meta.len(), meta.modified().ok());
            match self.seen.get(&path) {
                Some((len, modified, since)) if (*len, *modified) == key => {
                    if now.duration_since(*since) >= self.settle {
                        ready.push(path);
                    }
                }
                _ => {
                    self.seen.insert(path, (key.0, key.1, now));
                }
            }
        }
        self.seen.retain(|p, _| present.contains(p));
        let mut out = Vec::new();
        for path in ready {
            self.seen.remove(&path);
            out.push(process(registry, store, &cfg, &path)?);
        }
        Ok(out)
    }
}

fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn process(
    registry: &Registry,
    store: &mut Store,
    cfg: &Config,
    path: &Path,
) -> Result<Outcome, String> {
    let actor = Actor("watch".into());
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_owned();
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let args = json!({
        "content_base64": base64_encode(&bytes),
        "name": name,
    });
    match registry.invoke(store, &actor, "import.stage", args) {
        Ok(batch) if batch.get("needs_profile").is_some() => fail(
            cfg,
            path,
            "no CSV profile matches this file; create one with csv.profile_save",
        ),
        Ok(batch) => {
            let id = batch
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let mut committed = false;
            if cfg.auto_commit && auto_committable(&batch) {
                registry
                    .invoke(store, &actor, "import.commit", json!({"batch": id}))
                    .map_err(|e| e.to_string())?;
                committed = true;
            }
            let file = move_to(path, &cfg.archive)?;
            Ok(Outcome::Staged {
                file,
                batch: id,
                committed,
            })
        }
        Err(
            e @ (CommandError::InvalidArgs(_)
            | CommandError::Rejected(_)
            | CommandError::NotFound(_)),
        ) => fail(cfg, path, &e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

fn fail(cfg: &Config, path: &Path, error: &str) -> Result<Outcome, String> {
    let failed = cfg.folder.join("failed");
    let file = move_to(path, &failed)?;
    let mut note = file.clone().into_os_string();
    note.push(".error.txt");
    std::fs::write(&note, format!("{error}\n")).map_err(|e| e.to_string())?;
    Ok(Outcome::Failed {
        file,
        error: error.to_owned(),
    })
}

/// Move `path` into `dir`, keeping the name; on a clash add ` (1)`, ` (2)`, …
fn move_to(path: &Path, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let mut target = dir.join(format!("{stem}{ext}"));
    let mut n = 1;
    while target.exists() {
        target = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::rename(path, &target).map_err(|e| format!("cannot move {}: {e}", path.display()))?;
    Ok(target)
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// How long the watcher waits before trying again to open a book it skipped.
pub const RETRY_SKIPPED: Duration = Duration::from_secs(60);

/// Watch the book at `path` until `stop` is set: open it (as actor `watch`), scan the
/// watch folder every half second, and report each outcome. A book whose
/// before-update backup fails is skipped (reported once as [`Outcome::Skipped`]) and
/// tried again after [`RETRY_SKIPPED`]; the backup uses the app's backup settings.
/// Errors that end the loop (the book cannot be opened, the folder setting is
/// missing) are returned.
pub fn watch_book(
    path: &Path,
    settle: Duration,
    stop: &std::sync::atomic::AtomicBool,
    report: impl FnMut(&Outcome),
) -> Result<(), String> {
    watch_book_with(
        path,
        settle,
        stop,
        &mk_commands::backups::BackupEnv::from_app_config(),
        RETRY_SKIPPED,
        report,
    )
}

/// [`watch_book`] with the backup settings and the retry interval given (tests).
pub fn watch_book_with(
    path: &Path,
    settle: Duration,
    stop: &std::sync::atomic::AtomicBool,
    env: &mk_commands::backups::BackupEnv,
    retry: Duration,
    mut report: impl FnMut(&Outcome),
) -> Result<(), String> {
    let registry = Registry::with_builtins();
    let mut watcher = Watcher::new(settle);
    let mut store: Option<Store> = None;
    let mut next_try = Instant::now();
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        if store.is_none() && Instant::now() >= next_try {
            let last = watcher.status().map(str::to_owned);
            match watcher.open_book(path, env)? {
                Ok(s) => store = Some(s),
                Err(skipped) => {
                    next_try = Instant::now() + retry;
                    // The same message is reported once, not on every try.
                    if last.as_deref() != watcher.status() {
                        report(&skipped);
                    }
                }
            }
        }
        if let Some(s) = store.as_mut() {
            for outcome in watcher.pass(&registry, s)? {
                report(&outcome);
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}
