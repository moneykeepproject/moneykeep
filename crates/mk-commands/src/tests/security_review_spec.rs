//! security-review-1 acceptance tests SR-B1 and SR-B2 (the files; the screens' part
//! of SR-B2 is in mk-views' `security_review` tests, the CLI's in its own tests).
#![allow(clippy::panic)]

use std::path::{Path, PathBuf};

use crate::backups::{self, BackupEnv, BackupSettings};
use crate::{KdfParams, Secret, Session};
use serde_json::json;

const PASSWORD: &str = "correct horse battery";

fn env_in(dir: &Path) -> BackupEnv {
    let second = dir.join("USB");
    std::fs::create_dir_all(&second).unwrap();
    BackupEnv::new(
        BackupSettings {
            folder: Some(dir.join("Backups").to_str().unwrap().to_owned()),
            second_place: Some(second.to_str().unwrap().to_owned()),
            ..BackupSettings::default()
        },
        0,
    )
}

fn spend(s: &mut Session) {
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

#[test]
fn sr_b1_restore_when_the_name_appears_during_the_copy() {
    let dir = tempfile::tempdir().unwrap();
    let env = env_in(dir.path());
    let book = dir.path().join("Household.moneykeep");
    let mut s = Session::create(&book, "Household", "USD", "test").unwrap();
    s.set_backup_env(env.clone());
    spend(&mut s);
    let made = s.invoke("book.backup", json!({})).unwrap();
    let backup = PathBuf::from(made["path"].as_str().unwrap());
    let target = dir
        .path()
        .join("Household restored 2026-11-07 1420.moneykeep");
    let other = b"another program's file".to_vec();
    // Another program writes the name while the copy is made (into the reserved
    // empty file, as a program that opens it for writing would).
    let r = backups::restore_copy_with(&backup, &target, None, None, |reserved| {
        assert_eq!(reserved, target, "the name is reserved first");
        assert_eq!(std::fs::metadata(reserved).unwrap().len(), 0);
        std::fs::write(reserved, &other).unwrap();
    })
    .unwrap();
    let two = dir
        .path()
        .join("Household restored 2026-11-07 1420 (2).moneykeep");
    assert_eq!(r.path, two);
    assert_eq!(std::fs::read(&target).unwrap(), other, "left byte for byte");
    let mut restored = Session::open(&two, "test").unwrap();
    let t = restored
        .invoke("report.trial_balance", json!({"as_of": "2026-12-31"}))
        .unwrap();
    assert!(t.to_string().contains("12.5"), "{t}");
    // Or it removes the placeholder and puts its own file there: same outcome.
    let target2 = dir.path().join("Second restore.moneykeep");
    let r = backups::restore_copy_with(&backup, &target2, None, None, |reserved| {
        std::fs::remove_file(reserved).unwrap();
        std::fs::write(reserved, b"theirs").unwrap();
    })
    .unwrap();
    assert_eq!(r.path, dir.path().join("Second restore (2).moneykeep"));
    assert_eq!(std::fs::read(&target2).unwrap(), b"theirs");
    // No temporary files are left behind.
    let leftovers: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".part") || n.contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn sr_b1_the_unencrypted_copy_never_replaces_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("Household.moneykeep");
    let (mut s, _) = Session::create_encrypted(
        &book,
        "Household",
        "USD",
        "test",
        PASSWORD,
        false,
        KdfParams::TEST,
    )
    .unwrap();
    let out = dir.path().join("Household (unencrypted).moneykeep");
    std::fs::write(&out, b"keep me").unwrap();
    let e = s.save_unencrypted_copy(&out).unwrap_err();
    assert_eq!(e.code(), "store_error", "{e}");
    assert_eq!(std::fs::read(&out).unwrap(), b"keep me");
    let fresh = dir.path().join("Copy.moneykeep");
    assert_eq!(s.save_unencrypted_copy(&fresh).unwrap(), fresh);
    assert!(
        std::fs::read(&fresh)
            .unwrap()
            .starts_with(b"SQLite format 3")
    );
}

/// A plain book with 3 backups (2 in the backup folder, 1 in the second place).
fn three_plain_backups(dir: &Path) -> (Session, PathBuf, BackupEnv) {
    let env = env_in(dir);
    let book = dir.join("Household.moneykeep");
    let mut s = Session::create(&book, "Household", "USD", "test").unwrap();
    s.set_kdf(KdfParams::TEST);
    s.set_backup_env(env.clone());
    spend(&mut s);
    let mut env2 = env.clone();
    env2.settings.second_place = None;
    s.set_backup_env(env2);
    s.invoke("book.backup", json!({})).unwrap();
    s.set_backup_env(env.clone());
    s.invoke("book.backup", json!({})).unwrap();
    assert_eq!(backups::list(&book, &env).len(), 3);
    (s, book, env)
}

#[test]
fn sr_b2_adding_a_password_counts_and_protects_earlier_backups() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, book, env) = three_plain_backups(dir.path());
    s.encrypt(PASSWORD, false).unwrap();
    let n = s.invoke("book.unprotected_backups", json!({})).unwrap();
    assert_eq!(n["count"], 3, "{n}");
    assert_eq!(
        n["message"],
        "3 earlier backups of this book aren't password-protected."
    );
    let r = s.invoke("book.protect_backups", json!({})).unwrap();
    assert_eq!(r["done"], 3, "{r}");
    assert_eq!(r["failed"], json!([]));
    let list = backups::list(&book, &env);
    assert_eq!(list.len(), 3, "the same backups, under the same names");
    assert!(
        backups::unprotected(&book, &env).is_empty(),
        "no plain ones"
    );
    let pw = Secret::Password(PASSWORD.into());
    for e in &list {
        let bytes = std::fs::read(&e.path).unwrap();
        assert!(bytes.starts_with(b"MCBOOKE1"), "{}", e.path.display());
        assert!(!bytes.windows(6).any(|w| w == b"Market"));
        assert_eq!(
            Session::open(&e.path, "test").unwrap_err().code(),
            "password_required"
        );
        let mut b = Session::open_with(&e.path, "test", Some(&pw)).unwrap();
        let t = b
            .invoke("report.trial_balance", json!({"as_of": "2026-12-31"}))
            .unwrap();
        assert!(t.to_string().contains("12.5"), "{t}");
    }
    let n = s.invoke("book.unprotected_backups", json!({})).unwrap();
    assert_eq!(n["count"], 0);
}

#[test]
fn sr_b2_delete_them_and_plain_books_cannot_protect() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, book, env) = three_plain_backups(dir.path());
    let e = s.invoke("book.protect_backups", json!({})).unwrap_err();
    assert_eq!(e.code(), "not_encrypted", "{e}");
    s.encrypt(PASSWORD, false).unwrap();
    // A backup made after the password is protected already: it stays.
    s.invoke("book.backup", json!({})).unwrap();
    assert_eq!(backups::unprotected(&book, &env).len(), 3);
    let r = s
        .invoke("book.delete_unprotected_backups", json!({}))
        .unwrap();
    assert_eq!(r["done"], 3, "{r}");
    let left = backups::list(&book, &env);
    assert_eq!(left.len(), 2, "the protected backups stay (both places)");
    assert!(left.iter().all(backups::Entry::is_encrypted));
    for c in ["book.protect_backups", "book.delete_unprotected_backups"] {
        assert!(crate::NOT_OVER_MCP.contains(&c), "{c}");
    }
}
