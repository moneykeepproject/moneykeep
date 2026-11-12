//! backups.md acceptance tests BK1–BK5 and BK9 (the files; the screens' tests BK6–BK8,
//! BK10 and BK11 are in mk-views' `backups` tests).
#![allow(clippy::panic)]

use std::path::{Path, PathBuf};

use crate::backups::{self, BackupEnv, BackupSettings, Entry, Place, Reason};
use crate::{KdfParams, Secret, Session};
use serde_json::{Value, json};

struct Fixture {
    dir: tempfile::TempDir,
    env: BackupEnv,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Backups");
    let env = BackupEnv::new(
        BackupSettings {
            folder: Some(folder.to_str().unwrap().to_owned()),
            ..BackupSettings::default()
        },
        0,
    );
    Fixture { dir, env }
}

impl Fixture {
    fn book(&self) -> PathBuf {
        self.dir.path().join("Household.moneykeep")
    }
    fn folder(&self) -> PathBuf {
        self.dir.path().join("Backups").join("Household")
    }
    fn files(&self, dir: &Path) -> Vec<String> {
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
}

fn add_spending(s: &mut Session) {
    s.invoke("account.create", json!({"name": "Cash", "type": "asset"}))
        .unwrap();
    s.invoke("account.create", json!({"name": "Food", "type": "expense"}))
        .unwrap();
    s.invoke(
        "txn.create",
        json!({"date": "2026-11-01", "payee": "Market", "splits": [
            {"account": "Food", "value": "12.50"},
            {"account": "Cash", "value": "-12.50"}]}),
    )
    .unwrap();
}

fn trial(s: &mut Session) -> Value {
    s.invoke("report.trial_balance", json!({"as_of": "2026-12-31"}))
        .unwrap()
}

#[test]
fn backups_bk1_opening_backup_has_the_same_transactions() {
    let f = fixture();
    let mut s = Session::create(&f.book(), "Household", "USD", "test").unwrap();
    add_spending(&mut s);
    let before = trial(&mut s);
    drop(s);
    let (mut s, made) = Session::open_backed_up(&f.book(), "app", None, &f.env).unwrap();
    assert!(made.is_none(), "no update was pending");
    let r = s
        .invoke("book.backup", json!({"reason": "opened"}))
        .unwrap();
    let path = PathBuf::from(r["path"].as_str().unwrap());
    assert_eq!(path.parent().unwrap(), f.folder());
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("Household 20") && name.ends_with(" opened.moneykeep"),
        "{name}"
    );
    assert_eq!(
        r["size"].as_u64().unwrap(),
        std::fs::metadata(&path).unwrap().len()
    );
    // No temporary file is left.
    assert_eq!(f.files(&f.folder()), vec![name.to_owned()]);
    let mut copy = Session::open(&path, "test").unwrap();
    assert_eq!(trial(&mut copy), before);
    // Making a backup writes no audit entry.
    let log = s.invoke("history.list", json!({"limit": 50})).unwrap();
    assert!(
        log.as_array()
            .unwrap()
            .iter()
            .all(|e| !e["command"].as_str().unwrap().starts_with("book.backup")),
        "{log}"
    );
    // book.backups lists it.
    let list = s.invoke("book.backups", json!({})).unwrap();
    assert_eq!(list[0]["reason"], "opened");
    assert_eq!(list[0]["place"], "folder");
}

#[cfg(unix)]
fn make_unwritable(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(p).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o555)).unwrap();
}

#[cfg(unix)]
fn running_as_root() -> bool {
    // Root writes through permissions; then a file stands in for the folder.
    let probe = tempfile::tempdir().unwrap();
    let ro = probe.path().join("ro");
    make_unwritable(&ro);
    std::fs::write(ro.join("x"), b"x").is_ok()
}

#[test]
fn backups_bk2_before_update_backup_or_the_book_is_unchanged() {
    let f = fixture();
    let old = mk_store::latest_schema_version() - 1;
    mk_store::create_old_book_for_tests(&f.book(), "Household", old).unwrap();
    // A failing backup folder: opening stops and the file is byte-for-byte the same.
    let blocked = f.dir.path().join("blocked");
    #[cfg(unix)]
    let blocked_ok = if running_as_root() {
        std::fs::write(&blocked, b"not a folder").unwrap();
        true
    } else {
        make_unwritable(&blocked);
        true
    };
    #[cfg(not(unix))]
    let blocked_ok = {
        std::fs::write(&blocked, b"not a folder").unwrap();
        true
    };
    assert!(blocked_ok);
    let bad = BackupEnv::new(
        BackupSettings {
            folder: Some(blocked.to_str().unwrap().to_owned()),
            ..BackupSettings::default()
        },
        0,
    );
    let bytes = std::fs::read(f.book()).unwrap();
    let e = Session::open_backed_up(&f.book(), "app", None, &bad).unwrap_err();
    assert_eq!(e.code(), "backup_failed");
    assert!(
        e.to_string().starts_with(
            "MoneyKeep couldn't make a backup before updating this book, so it wasn't changed: "
        ),
        "{e}"
    );
    assert_eq!(
        std::fs::read(f.book()).unwrap(),
        bytes,
        "the book file is unchanged"
    );
    // With a working folder the before-update backup exists, in the old format.
    let (mut s, made) = Session::open_backed_up(&f.book(), "app", None, &f.env).unwrap();
    let made = made.unwrap();
    assert_eq!(made.reason, Reason::BeforeUpdate);
    assert!(
        made.path
            .to_str()
            .unwrap()
            .ends_with(" before-update.moneykeep")
    );
    let checked = mk_store::check_backup(&made.path, mk_store::Unlock::None).unwrap();
    assert_eq!(
        checked.info.schema_version, old,
        "taken before the update ran"
    );
    assert_eq!(
        s.invoke("book.info", json!({})).unwrap()["schema_version"],
        mk_store::latest_schema_version()
    );
}

#[test]
fn backups_bk3_closed_backup_only_after_a_change() {
    let f = fixture();
    drop(Session::create(&f.book(), "Household", "USD", "test").unwrap());
    let (mut s, _) = Session::open_backed_up(&f.book(), "app", None, &f.env).unwrap();
    assert!(
        !s.changed_since_backup(),
        "nothing changed: no closed backup"
    );
    s.invoke("account.create", json!({"name": "Cash", "type": "asset"}))
        .unwrap();
    assert!(s.changed_since_backup());
    s.invoke("book.backup", json!({"reason": "closed"}))
        .unwrap();
    assert!(!s.changed_since_backup());
    let list = backups::list(&f.book(), &f.env);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].reason, Reason::Closed);
}

#[test]
fn backups_bk4_encrypted_backup_stays_encrypted() {
    let f = fixture();
    let (mut s, _) = Session::create_encrypted(
        &f.book(),
        "Household",
        "USD",
        "test",
        "correct horse battery",
        false,
        KdfParams::TEST,
    )
    .unwrap();
    s.set_backup_env(f.env.clone());
    add_spending(&mut s);
    let before = trial(&mut s);
    // Watch the folder while the backup is made: every file that ever appears in
    // it (temporary or final) is encrypted.
    std::fs::create_dir_all(f.folder()).unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, Vec<u8>)>::new()));
    let watcher = {
        let (stop, seen, dir) = (stop.clone(), seen.clone(), f.folder());
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    for e in rd.filter_map(Result::ok) {
                        if let Ok(b) = std::fs::read(e.path())
                            && !b.is_empty()
                        {
                            let head = b.iter().take(16).copied().collect();
                            seen.lock()
                                .unwrap()
                                .push((e.file_name().to_string_lossy().into(), head));
                        }
                    }
                }
            }
        })
    };
    let r = s.invoke("book.backup", json!({})).unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    watcher.join().unwrap();
    assert_eq!(r["encrypted"], true);
    for (name, head) in seen.lock().unwrap().iter() {
        assert!(head.starts_with(b"MCBOOKE1"), "{name} was not encrypted");
        assert!(!head.starts_with(b"SQLite format 3"), "{name}");
    }
    let path = PathBuf::from(r["path"].as_str().unwrap());
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"MCBOOKE1"));
    assert!(!bytes.windows(6).any(|w| w == b"Market"));
    drop(s);
    let e = Session::open(&path, "test").unwrap_err();
    assert_eq!(e.code(), "password_required");
    let pw = Secret::Password("correct horse battery".into());
    let mut copy = Session::open_with(&path, "test", Some(&pw)).unwrap();
    assert_eq!(trial(&mut copy), before);
}

fn entry(at: &str, reason: Reason) -> Entry {
    let (d, t) = at.split_once(' ').unwrap();
    let name = backups::file_name(
        "B",
        time::PrimitiveDateTime::parse(
            &format!("{d} {}:{}", &t[..2], &t[2..]),
            time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]"),
        )
        .unwrap(),
        reason,
        1,
    );
    let (at, reason) = backups::parse_name("B", &name).unwrap();
    Entry {
        path: PathBuf::from(name),
        at,
        reason,
        size: 1,
        place: Place::Folder,
    }
}

#[test]
fn backups_bk5_rotation() {
    let f = fixture();
    let dir = f.dir.path().join("Backups").join("B");
    std::fs::create_dir_all(&dir).unwrap();
    // 40 backups over 400 simulated days (one every 10 days), every third manual,
    // then six more in the last week (one a day), five of them manual on top.
    let start = time::macros::date!(2025 - 10 - 03);
    let today = start + time::Duration::days(400);
    let mut made = Vec::new();
    for i in 0..40 {
        let day = start + time::Duration::days(i * 10);
        let reason = if i % 3 == 0 {
            Reason::Manual
        } else {
            Reason::Daily
        };
        made.push((day, reason));
    }
    for i in 1..7 {
        made.push((today - time::Duration::days(i), Reason::Opened));
    }
    for i in 0..5 {
        made.push((today - time::Duration::days(i), Reason::Manual));
    }
    let mut env = f.env.clone();
    env.settings.folder = Some(f.dir.path().join("Backups").to_str().unwrap().to_owned());
    for (day, reason) in &made {
        let at = time::PrimitiveDateTime::new(*day, time::macros::time!(12:00));
        let name = backups::file_name("B", at, *reason, 1);
        std::fs::write(dir.join(name), b"x").unwrap();
    }
    std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
    std::fs::write(dir.join("B copy.moneykeep"), b"mine").unwrap();
    // A real backup today triggers the rotation.
    let book = f.dir.path().join("B.moneykeep");
    let mut s = Session::create(&book, "B", "USD", "test").unwrap();
    env.now_unix = Some(
        time::PrimitiveDateTime::new(today, time::macros::time!(18:00))
            .assume_utc()
            .unix_timestamp(),
    );
    s.set_backup_env(env.clone());
    s.invoke("book.backup", json!({"reason": "daily"})).unwrap();
    let left = backups::list(&book, &env);
    let names = f.files(&dir);
    assert!(
        names.contains(&"notes.txt".to_owned()) && names.contains(&"B copy.moneykeep".to_owned())
    );
    let ordinary: Vec<&Entry> = left
        .iter()
        .filter(|e| !matches!(e.reason, Reason::Manual | Reason::BeforeUpdate))
        .collect();
    // The 10 newest ordinary backups.
    let mut all_ordinary: Vec<(time::Date, Reason)> = made
        .iter()
        .filter(|(_, r)| *r != Reason::Manual)
        .copied()
        .collect();
    all_ordinary.push((today, Reason::Daily));
    all_ordinary.sort_by_key(|b| std::cmp::Reverse(b.0));
    for (d, _) in all_ordinary.iter().take(10) {
        assert!(
            ordinary.iter().any(|e| e.at.date() == *d),
            "recent {d} kept"
        );
    }
    // One per day for the last 7 days.
    for i in 0..7 {
        let d = today - time::Duration::days(i);
        assert!(left.iter().any(|e| e.at.date() == d), "day {d}");
    }
    // One per month for the last 12 months (each had an ordinary backup).
    let mut m = (today.year(), today.month());
    for _ in 0..12 {
        let had = all_ordinary
            .iter()
            .any(|(d, _)| d.year() == m.0 && d.month() == m.1);
        let kept = ordinary
            .iter()
            .any(|e| e.at.year() == m.0 && e.at.month() == m.1);
        assert_eq!(kept, had, "month {m:?}");
        if m.1 == time::Month::January {
            m.0 -= 1;
        }
        m.1 = m.1.previous();
    }
    // At most 10 manual, the newest ones.
    let manual: Vec<&Entry> = left.iter().filter(|e| e.reason == Reason::Manual).collect();
    assert_eq!(manual.len(), 10);
    let mut all_manual: Vec<time::Date> = made
        .iter()
        .filter(|(_, r)| *r == Reason::Manual)
        .map(|(d, _)| *d)
        .collect();
    all_manual.sort_by_key(|b| std::cmp::Reverse(*b));
    for (e, d) in manual.iter().zip(all_manual.iter()) {
        assert_eq!(e.at.date(), *d);
    }
    // Nothing older than 12 months survives among the ordinary ones.
    assert!(ordinary.len() < all_ordinary.len());
    // The pure plan agrees with what is on disk.
    let fresh = [entry("2026-01-01 1000", Reason::Daily)];
    assert!(backups::plan_rotation(&fresh, today, &env.settings).is_empty());
}

#[test]
fn backups_bk9_a_copy_failing_its_check_is_deleted_and_reported() {
    let f = fixture();
    let mut s = Session::create(&f.book(), "Household", "USD", "test").unwrap();
    let mut env = f.env.clone();
    env.fail_check = true;
    s.set_backup_env(env);
    let e = s.invoke("book.backup", json!({})).unwrap_err();
    assert_eq!(e.code(), "backup_check");
    assert!(e.to_string().contains("failed its check"), "{e}");
    assert!(
        f.files(&f.folder()).is_empty(),
        "{:?}",
        f.files(&f.folder())
    );
}

#[test]
fn backups_second_place_and_restore_copy() {
    let f = fixture();
    let second = f.dir.path().join("USB");
    std::fs::create_dir_all(&second).unwrap();
    let mut env = f.env.clone();
    env.settings.second_place = Some(second.to_str().unwrap().to_owned());
    let mut s = Session::create(&f.book(), "Household", "USD", "test").unwrap();
    s.set_backup_env(env.clone());
    add_spending(&mut s);
    let r = s.invoke("book.backup", json!({})).unwrap();
    let copy = PathBuf::from(r["second_place_path"].as_str().unwrap());
    assert_eq!(copy.parent().unwrap(), second.join("Household"));
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        std::fs::read(r["path"].as_str().unwrap()).unwrap()
    );
    // Unplugged: the backup still succeeds; the second copy is skipped.
    std::fs::remove_dir_all(&second).unwrap();
    let r2 = s.invoke("book.backup", json!({})).unwrap();
    assert!(r2["second_place_path"].is_null());
    assert!(
        r2["second_place_error"]
            .as_str()
            .unwrap()
            .contains("isn't reachable")
    );
    let list = backups::list(&f.book(), &env);
    assert_eq!(list.len(), 2);
    // Restore as a copy.
    let book_bytes = std::fs::read(f.book()).unwrap();
    let target = f.dir.path().join("Household restored.moneykeep");
    let out = s
        .invoke(
            "book.restore_copy",
            json!({"backup": r["path"], "target": target.to_str().unwrap()}),
        )
        .unwrap();
    assert_eq!(out["path"], target.to_str().unwrap());
    let mut restored = Session::open(&target, "test").unwrap();
    assert_eq!(trial(&mut restored), trial(&mut s));
    assert_eq!(std::fs::read(f.book()).unwrap(), book_bytes);
    // The target must not exist.
    let again = s
        .invoke(
            "book.restore_copy",
            json!({"backup": r["path"], "target": target.to_str().unwrap()}),
        )
        .unwrap_err();
    assert_eq!(again.code(), "store_error");
}

#[test]
fn backups_newer_backups_are_refused_and_commands_need_a_front_end() {
    let f = fixture();
    let mut s = Session::create(&f.book(), "Household", "USD", "test").unwrap();
    let e = s.invoke("book.backup", json!({})).unwrap_err();
    assert_eq!(e.code(), "unsupported");
    drop(s);
    let newer = f.dir.path().join("newer.moneykeep");
    std::fs::copy(f.book(), &newer).unwrap();
    {
        let c = rusqlite_free_bump(&newer);
        assert!(c);
    }
    let e =
        backups::restore_copy(&newer, &f.dir.path().join("t.moneykeep"), None, None).unwrap_err();
    assert_eq!(e.code(), "too_new");
    assert!(e.to_string().contains("newer MoneyKeep"), "{e}");
    assert!(!f.dir.path().join("t.moneykeep").exists());
}

/// Set a book's format version far ahead (bytes 60–63 of the SQLite header hold
/// `user_version`, big-endian).
fn rusqlite_free_bump(path: &Path) -> bool {
    let mut b = std::fs::read(path).unwrap();
    if b.len() < 64 {
        return false;
    }
    b[60..64].copy_from_slice(&999u32.to_be_bytes());
    std::fs::write(path, b).is_ok()
}

#[test]
fn backups_are_not_offered_over_mcp() {
    for c in ["book.backup", "book.restore_copy"] {
        assert!(crate::NOT_OVER_MCP.contains(&c), "{c}");
    }
}

// ----- revision 3 (default folder, folder tests, Windows blocking) -----------------------

#[test]
fn backups_bk16_default_folder_and_the_old_default_rule() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    assert_eq!(
        backups::default_folder_in(home),
        home.join("MoneyKeep Backups")
    );
    let old = home.join("Documents").join("MoneyKeep Backups");
    // The old default, never written to: the new default.
    assert_eq!(backups::resolve_folder_setting(None, Some(&old)), None);
    std::fs::create_dir_all(old.join("Household")).unwrap();
    std::fs::write(old.join("Household").join("notes.txt"), b"x").unwrap();
    assert_eq!(
        backups::resolve_folder_setting(Some(old.to_str().unwrap()), Some(&old)),
        None,
        "only MoneyKeep's backups count"
    );
    // With a backup there: kept (written out, so the new default doesn't take it).
    std::fs::write(
        old.join("Household")
            .join("Household 2026-11-01 0900 opened.moneykeep"),
        b"x",
    )
    .unwrap();
    assert_eq!(
        backups::resolve_folder_setting(None, Some(&old)).as_deref(),
        old.to_str()
    );
    assert_eq!(
        backups::resolve_folder_setting(Some(old.to_str().unwrap()), Some(&old)).as_deref(),
        old.to_str()
    );
    // A folder the user chose is kept.
    let mine = home.join("Elsewhere").display().to_string();
    assert_eq!(
        backups::resolve_folder_setting(Some(&mine), Some(&old)).as_deref(),
        Some(mine.as_str())
    );
    // An empty setting means the default.
    let s = BackupSettings::default();
    assert_eq!(
        s.folder_path(),
        backups::default_folder(),
        "the default is the user folder's"
    );
    if let Some(d) = backups::default_folder() {
        assert!(d.ends_with("MoneyKeep Backups"));
        assert!(!d.parent().unwrap().ends_with("Documents"));
    }
}

#[test]
fn backups_bk17_choosing_a_folder_tests_it() {
    let dir = tempfile::tempdir().unwrap();
    let ok = dir.path().join("Good");
    std::fs::create_dir_all(&ok).unwrap();
    backups::test_folder(&ok, &BackupEnv::default()).unwrap();
    assert_eq!(
        std::fs::read_dir(&ok).unwrap().count(),
        0,
        "the test file is gone"
    );
    // A folder that can't be written (test hook).
    let env = BackupEnv {
        simulate_folder_error: Some(std::io::ErrorKind::PermissionDenied),
        ..BackupEnv::default()
    };
    let e = backups::test_folder(&ok, &env).unwrap_err();
    assert_eq!(e.code(), "backup_folder");
    assert!(
        e.to_string().starts_with("MoneyKeep can't write to "),
        "{e}"
    );
    // A real failure: a "folder" that is a file.
    let file = dir.path().join("file.txt");
    std::fs::write(&file, b"x").unwrap();
    let e = backups::test_folder(&file, &BackupEnv::default()).unwrap_err();
    assert_eq!(e.code(), "backup_folder", "{e}");
}

#[test]
fn backups_bk18_windows_blocking_is_explained() {
    let dir = tempfile::tempdir().unwrap();
    let docs = dir.path().join("Documents");
    let blocked = docs.join("MoneyKeep Backups");
    let protected = vec![docs.clone()];
    let m = backups::blocked_message(&blocked, std::io::ErrorKind::NotFound, &protected).unwrap();
    assert_eq!(
        m,
        format!(
            "Windows blocked MoneyKeep from writing to {}. This is usually Windows' ransomware protection (Controlled folder access).",
            blocked.display()
        )
    );
    assert!(
        backups::blocked_message(&blocked, std::io::ErrorKind::PermissionDenied, &protected)
            .is_some()
    );
    assert!(
        backups::blocked_message(&blocked, std::io::ErrorKind::Other, &protected).is_none(),
        "another failure keeps its plain message"
    );
    let other = dir.path().join("Documents2").join("B");
    assert!(
        backups::blocked_message(&other, std::io::ErrorKind::NotFound, &protected).is_none(),
        "a folder beside Documents is not inside it"
    );
    // Making a backup there (test hook): the explanation, code backup_blocked.
    let book = dir.path().join("Household.moneykeep");
    let s = Session::create(&book, "Household", "USD", "test").unwrap();
    drop(s);
    let mut env = BackupEnv::new(
        BackupSettings {
            folder: Some(blocked.display().to_string()),
            ..BackupSettings::default()
        },
        0,
    );
    env.simulate_folder_error = Some(std::io::ErrorKind::NotFound);
    env.protected_override = Some(protected.clone());
    let e = backups::back_up_file(&book, None, Reason::Manual, &env).unwrap_err();
    assert_eq!(e.code(), backups::BLOCKED);
    assert!(e.to_string().contains("Controlled folder access"), "{e}");
    let e = backups::test_folder(&blocked, &env).unwrap_err();
    assert_eq!(e.code(), backups::BLOCKED);
    // Under another path: the plain message.
    env.settings.folder = Some(dir.path().join("Plain").display().to_string());
    let e = backups::back_up_file(&book, None, Reason::Manual, &env).unwrap_err();
    assert_eq!(e.code(), "backup_folder");
    assert!(
        e.to_string()
            .starts_with("can't write to the backup folder"),
        "{e}"
    );
}

// ----- revision 4 (copies flushed through a writable handle; second-place failures) --------

#[test]
fn backups_bk20_copies_are_flushed_through_a_writable_handle() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("a.bin");
    let dst = dir.path().join("b.bin");
    std::fs::write(&src, b"backup bytes").unwrap();
    std::fs::write(&dst, b"a longer file that is replaced").unwrap();
    assert_eq!(backups::copy_flushed(&src, &dst).unwrap(), 12);
    assert_eq!(std::fs::read(&dst).unwrap(), b"backup bytes");
    // The second-place copy and the restore copy go through it (and succeed).
    let f = fixture();
    let second = f.dir.path().join("USB");
    std::fs::create_dir_all(&second).unwrap();
    let mut env = f.env.clone();
    env.settings.second_place = Some(second.to_str().unwrap().to_owned());
    let mut s = Session::create(&f.book(), "Household", "USD", "test").unwrap();
    s.set_backup_env(env);
    add_spending(&mut s);
    let r = s.invoke("book.backup", json!({})).unwrap();
    assert!(r["second_place_failed"].is_null(), "{r}");
    let copy = PathBuf::from(r["second_place_path"].as_str().unwrap());
    assert_eq!(
        std::fs::read(&copy).unwrap(),
        std::fs::read(r["path"].as_str().unwrap()).unwrap()
    );
    let target = f.dir.path().join("Household restored.moneykeep");
    backups::restore_copy(&copy, &target, None, None).unwrap();
    let mut restored = Session::open(&target, "test").unwrap();
    assert_eq!(trial(&mut restored), trial(&mut s));
}

#[test]
fn backups_bk21_a_failing_second_place_copy_is_reported() {
    let f = fixture();
    let second = f.dir.path().join("USB");
    std::fs::create_dir_all(&second).unwrap();
    let mut env = f.env.clone();
    env.settings.second_place = Some(second.to_str().unwrap().to_owned());
    env.fail_second = true;
    let mut s = Session::create(&f.book(), "Household", "USD", "test").unwrap();
    s.set_backup_env(env.clone());
    let r = s.invoke("book.backup", json!({})).unwrap();
    assert!(r["second_place_path"].is_null());
    assert_eq!(r["second_place_failed"], "simulated write failure", "{r}");
    assert!(
        f.files(&second.join("Household")).is_empty(),
        "no temporary file is left"
    );
    // Unplugged is not a failure to show.
    std::fs::remove_dir_all(&second).unwrap();
    env.fail_second = false;
    s.set_backup_env(env);
    let r = s.invoke("book.backup", json!({})).unwrap();
    assert!(r["second_place_failed"].is_null(), "{r}");
    assert!(!r["second_place_error"].is_null());
}
