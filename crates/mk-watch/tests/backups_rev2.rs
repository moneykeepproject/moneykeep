//! backups.md revision 2, BK13: the folder watcher opens a book needing a format
//! update the way every program does: a `before-update` backup is made (and no
//! other); when it fails the book is skipped for this run, unchanged, the message is
//! in the watcher's status and log, and it is tried again on the next run.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mk_commands::backups::{BackupEnv, BackupSettings};
use mk_watch::{Outcome, Watcher};

const MSG: &str =
    "MoneyKeep couldn't make a backup before updating this book, so it wasn't changed: ";

fn env(folder: &Path) -> BackupEnv {
    BackupEnv::new(
        BackupSettings {
            folder: Some(folder.to_str().unwrap().to_owned()),
            ..BackupSettings::default()
        },
        0,
    )
}

fn old_book(dir: &Path) -> PathBuf {
    let path = dir.join("Household.moneykeep");
    mk_store::create_old_book_for_tests(&path, "Household", mk_store::latest_schema_version() - 1)
        .unwrap();
    path
}

fn files(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
fn backups_bk13_watcher_skips_the_book_when_the_backup_fails_then_tries_again() {
    let dir = tempfile::tempdir().unwrap();
    let book = old_book(dir.path());
    let bytes = std::fs::read(&book).unwrap();
    // A file where the backup folder should be: it cannot be written (also as root).
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, b"not a folder").unwrap();
    let mut w = Watcher::new(Duration::from_secs(1));
    match w.open_book(&book, &env(&blocked)).unwrap() {
        Err(Outcome::Skipped { file, error }) => {
            assert_eq!(file, book);
            assert!(error.starts_with(MSG), "{error}");
        }
        other => panic!("skipped: {:?}", other.map(|_| ())),
    }
    assert!(w.status().unwrap().starts_with(MSG), "in the status");
    assert_eq!(std::fs::read(&book).unwrap(), bytes, "unchanged");
    // The next run, with a working folder: the before-update backup, then the update.
    let folder = dir.path().join("Backups");
    let store = w.open_book(&book, &env(&folder)).unwrap().unwrap();
    assert_eq!(w.status(), None);
    assert_eq!(
        store.info().unwrap().schema_version,
        mk_store::latest_schema_version()
    );
    let made = files(&folder.join("Household"));
    assert_eq!(made.len(), 1, "only the before-update backup: {made:?}");
    assert!(made[0].ends_with(" before-update.moneykeep"), "{made:?}");
}

#[test]
fn backups_bk13_watch_loop_reports_the_skip_once_and_keeps_running() {
    let dir = tempfile::tempdir().unwrap();
    let book = old_book(dir.path());
    let bytes = std::fs::read(&book).unwrap();
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, b"not a folder").unwrap();
    let stop = AtomicBool::new(false);
    let mut log = Vec::new();
    std::thread::scope(|scope| {
        let t = scope.spawn(|| {
            mk_watch::watch_book_with(
                &book,
                Duration::from_secs(1),
                &stop,
                &env(&blocked),
                Duration::ZERO,
                |o| log.push(o.clone()),
            )
        });
        // Several runs, each trying again.
        std::thread::sleep(Duration::from_millis(1600));
        stop.store(true, Ordering::Relaxed);
        assert_eq!(
            t.join().unwrap(),
            Ok(()),
            "the loop did not stop on the failure"
        );
    });
    assert_eq!(log.len(), 1, "the same message is logged once: {log:?}");
    match &log[0] {
        Outcome::Skipped { error, .. } => assert!(error.starts_with(MSG), "{error}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read(&book).unwrap(), bytes, "unchanged");
}
