//! book-encryption.md acceptance tests 1–10 and 13 (11 is in the CLI tests, 12 in
//! the MCP tests). Cheap Argon2 parameters keep them fast; the production defaults
//! are asserted separately.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use mk_commands::{KdfParams, Secret, Session};
use serde_json::json;

const PW: &str = "correct horse battery";

fn pw(s: &str) -> Secret {
    Secret::Password(s.into())
}

fn new_book(dir: &Path, recovery: bool) -> (Session, PathBuf, Option<String>) {
    let path = dir.join("home.moneykeep");
    let (s, key) = Session::create_encrypted(
        &path,
        "Household Books",
        "USD",
        "test",
        PW,
        recovery,
        KdfParams::TEST,
    )
    .unwrap();
    (s, path, key)
}

fn open(path: &Path, secret: &Secret) -> Session {
    let mut s = Session::open_with(path, "test", Some(secret)).unwrap();
    s.set_kdf(KdfParams::TEST);
    s
}

fn names(s: &mut Session) -> Vec<String> {
    s.invoke("account.list", json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["full_name"].as_str().unwrap().to_owned())
        .collect()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn code<T: std::fmt::Debug>(r: Result<T, mk_commands::CommandError>) -> &'static str {
    r.unwrap_err().code()
}

#[test]
fn production_kdf_defaults_are_the_spec_values() {
    assert_eq!(
        KdfParams::DEFAULT,
        KdfParams {
            memory_kib: 65_536,
            iterations: 3,
            parallelism: 1
        }
    );
}

#[test]
fn book_encryption_at1_create_add_close_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, path, _) = new_book(dir.path(), false);
    s.invoke(
        "account.create",
        json!({"name": "Zanzibar Savings", "type": "asset", "subtype": "bank"}),
    )
    .unwrap();
    drop(s);
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"MCBOOKE1"));
    assert!(!contains(&bytes, b"Zanzibar Savings"));
    assert!(!contains(&bytes, b"Household Books"));
    assert!(!contains(&bytes, b"SQLite format 3"));
    let mut s = open(&path, &pw(PW));
    assert!(names(&mut s).contains(&"Zanzibar Savings".to_owned()));
    assert!(s.is_encrypted());
}

#[test]
fn book_encryption_at2_missing_and_wrong_secret() {
    let dir = tempfile::tempdir().unwrap();
    let (s, path, _) = new_book(dir.path(), false);
    drop(s);
    assert_eq!(code(Session::open(&path, "t")), "password_required");
    assert_eq!(
        code(Session::open_with(&path, "t", None)),
        "password_required"
    );
    assert_eq!(
        code(Session::open_with(&path, "t", Some(&pw("wrong")))),
        "wrong_password"
    );
    // Outer whitespace is trimmed (and NFC applied) when opening too.
    open(&path, &pw("  correct horse battery "));
}

#[test]
fn book_encryption_at3_tampering_never_opens() {
    let dir = tempfile::tempdir().unwrap();
    let (s, path, _) = new_book(dir.path(), true);
    drop(s);
    let good = std::fs::read(&path).unwrap();
    // Offsets: magic 8, version 2, KDF block 29 (memory at 11..15, salt 23..39),
    // slot count at 39, first slot 40..113, second 113..186, payload nonce, payload.
    for (what, at) in [
        ("payload", good.len() - 20),
        ("kdf memory", 12),
        ("salt", 30),
        ("slot", 70),
        ("payload nonce", 190),
    ] {
        let mut bad = good.clone();
        bad[at] ^= 0x01;
        std::fs::write(&path, &bad).unwrap();
        let r = Session::open_with(&path, "t", Some(&pw(PW)));
        let c = code(r);
        assert!(c == "book_damaged" || c == "wrong_password", "{what}: {c}");
    }
    // Truncated: damaged.
    std::fs::write(&path, &good[..good.len() - 40]).unwrap();
    assert_eq!(
        code(Session::open_with(&path, "t", Some(&pw(PW)))),
        "book_damaged"
    );
    std::fs::write(&path, &good[..60]).unwrap();
    assert_eq!(
        code(Session::open_with(&path, "t", Some(&pw(PW)))),
        "book_damaged"
    );
    // Not a book at all.
    std::fs::write(&path, b"hello, this is not a book file at all").unwrap();
    assert_eq!(
        code(Session::open_with(&path, "t", Some(&pw(PW)))),
        "not_a_book"
    );
}

#[test]
fn book_encryption_at4_recovery_key_and_replacing_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.moneykeep");
    let mut s = Session::create(&path, "Plain", "USD", "test").unwrap();
    s.set_kdf(KdfParams::TEST);
    s.invoke(
        "account.create",
        json!({"name": "Checking", "type": "asset", "subtype": "bank"}),
    )
    .unwrap();
    let key = s.encrypt(PW, true).unwrap().unwrap();
    assert!(key.starts_with("MCRK-"));
    drop(s);
    assert!(std::fs::read(&path).unwrap().starts_with(b"MCBOOKE1"));
    assert!(
        !dir.path().join("plain.moneykeep.prev").exists(),
        "no plaintext .prev"
    );
    let typed = key.to_lowercase().replace('-', " ");
    let mut s = open(&path, &Secret::RecoveryKey(typed.clone()));
    assert!(names(&mut s).contains(&"Checking".to_owned()));
    let new_key = s.new_recovery_key(&pw(PW)).unwrap();
    assert_ne!(new_key, key);
    drop(s);
    assert_eq!(
        code(Session::open_with(
            &path,
            "t",
            Some(&Secret::RecoveryKey(typed))
        )),
        "wrong_password"
    );
    let mut s = open(&path, &Secret::RecoveryKey(new_key.clone()));
    assert!(s.has_recovery_key());
    s.remove_recovery_key(&pw(PW)).unwrap();
    assert!(!s.has_recovery_key());
    drop(s);
    assert_eq!(
        code(Session::open_with(
            &path,
            "t",
            Some(&Secret::RecoveryKey(new_key))
        )),
        "wrong_password"
    );
}

#[test]
fn book_encryption_at5_change_password_keeps_data_key_and_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, path, key) = new_book(dir.path(), true);
    let key = key.unwrap();
    s.set_kdf(KdfParams::TEST);
    let before = s.file_bytes_for_tests().unwrap();
    s.change_password(&pw(PW), "tr0ub4dor and three").unwrap();
    // Wrong current password is refused.
    assert_eq!(
        code(s.change_password(&pw("nope nope nope"), "another one here")),
        "wrong_password"
    );
    drop(s);
    let after = std::fs::read(&path).unwrap();
    assert_eq!(
        code(Session::open_with(&path, "t", Some(&pw(PW)))),
        "wrong_password"
    );
    open(&path, &pw("tr0ub4dor and three"));
    open(&path, &Secret::RecoveryKey(key.clone()));
    assert_ne!(before, after);
    // The data key is unchanged: the recovery slot (which wraps it and does not
    // depend on the salt) is byte-for-byte the same, and still opens the book.
    let old_recovery_slot = &before[113..186];
    let new_recovery_slot = &after[113..186];
    assert_eq!(
        old_recovery_slot, new_recovery_slot,
        "recovery slot untouched (same data key)"
    );
}

#[test]
fn book_encryption_at6_save_writes_new_nonce_and_prev() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, path, _) = new_book(dir.path(), false);
    s.invoke("account.create", json!({"name": "Before", "type": "asset"}))
        .unwrap();
    let nonce1 = mk_store_nonce(&s);
    s.invoke("account.create", json!({"name": "After", "type": "asset"}))
        .unwrap();
    let nonce2 = mk_store_nonce(&s);
    assert_ne!(nonce1, nonce2, "fresh payload nonce on every save");
    // Non-mutating commands never write.
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    s.invoke("account.list", json!({})).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);
    assert_eq!(mk_store_nonce(&s), nonce2);
    drop(s);
    let prev = dir.path().join("home.moneykeep.prev");
    assert!(prev.exists());
    let mut p = open(&prev, &pw(PW));
    let n = names(&mut p);
    assert!(n.contains(&"Before".to_owned()) && !n.contains(&"After".to_owned()));
}

/// The payload nonce of the open book's file (read through the session, which holds
/// the file's lock: on Windows no other handle can read it).
fn mk_store_nonce(s: &Session) -> Vec<u8> {
    let bytes = s.file_bytes_for_tests().unwrap();
    // Two-slot-free layout: header 40 + 73 per slot, then the 24-byte nonce.
    let slots = bytes[39] as usize;
    let at = 40 + 73 * slots;
    bytes[at..at + 24].to_vec()
}

#[test]
fn book_encryption_at7_failed_save_rolls_back() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, path, _) = new_book(dir.path(), false);
    s.invoke("account.create", json!({"name": "Kept", "type": "asset"}))
        .unwrap();
    let on_disk = s.file_bytes_for_tests().unwrap();
    s.fail_next_save_for_tests();
    let e = s
        .invoke("account.create", json!({"name": "Lost", "type": "asset"}))
        .unwrap_err();
    assert_eq!(e.code(), "io");
    assert!(
        !names(&mut s).contains(&"Lost".to_owned()),
        "rolled back in memory"
    );
    assert!(names(&mut s).contains(&"Kept".to_owned()));
    assert_eq!(s.file_bytes_for_tests().unwrap(), on_disk, "file unchanged");
    // The session keeps working afterwards.
    s.invoke("account.create", json!({"name": "Later", "type": "asset"}))
        .unwrap();
    drop(s);
    let mut s = open(&path, &pw(PW));
    let n = names(&mut s);
    assert!(n.contains(&"Later".to_owned()) && !n.contains(&"Lost".to_owned()));
}

#[test]
fn book_encryption_at8_second_open_is_book_in_use() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, path, _) = new_book(dir.path(), false);
    assert_eq!(
        code(Session::open_with(&path, "t", Some(&pw(PW)))),
        "book_in_use"
    );
    // Still exclusive after a save replaced the file.
    s.invoke("account.create", json!({"name": "A", "type": "asset"}))
        .unwrap();
    assert_eq!(
        code(Session::open_with(&path, "t", Some(&pw(PW)))),
        "book_in_use"
    );
    drop(s);
    open(&path, &pw(PW));
}

#[test]
fn book_encryption_at9_no_plaintext_side_files() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, _path, _) = new_book(dir.path(), false);
    for i in 0..3 {
        s.invoke(
            "account.create",
            json!({"name": format!("A{i}"), "type": "asset"}),
        )
        .unwrap();
        s.invoke("account.list", json!({})).unwrap();
        let mut files: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(
            files,
            ["home.moneykeep", "home.moneykeep.prev"],
            "{files:?}"
        );
    }
}

#[test]
fn book_encryption_at10_unencrypted_copy_matches() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, path, _) = new_book(dir.path(), false);
    s.invoke(
        "account.create",
        json!({"name": "Checking", "type": "asset", "subtype": "bank"}),
    )
    .unwrap();
    s.invoke("txn.create", json!({"date": "2026-10-01", "splits": [
        {"account": "Checking", "value": "100.00"}, {"account": "Opening Balances", "value": "-100.00"}]})).unwrap();
    let copy = dir.path().join("copy.moneykeep");
    assert_eq!(code(s.save_unencrypted_copy(&path)), "invalid_args");
    s.save_unencrypted_copy(&copy).unwrap();
    let tb = s.invoke("report.trial_balance", json!({})).unwrap();
    assert!(
        std::fs::read(&copy)
            .unwrap()
            .starts_with(b"SQLite format 3\0")
    );
    assert!(s.file_bytes_for_tests().unwrap().starts_with(b"MCBOOKE1"));
    let mut c = Session::open(&copy, "t").unwrap();
    assert_eq!(c.invoke("report.trial_balance", json!({})).unwrap(), tb);
}

#[test]
fn book_encryption_at13_audit_entries_without_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.moneykeep");
    let mut s = Session::create(&path, "Plain", "USD", "test").unwrap();
    s.set_kdf(KdfParams::TEST);
    s.invoke(
        "account.create",
        json!({"name": "Checking", "type": "asset"}),
    )
    .unwrap();
    let key = s.encrypt(PW, true).unwrap().unwrap();
    s.change_password(&pw(PW), "a brand new passphrase")
        .unwrap();
    let h = s.invoke("history.list", json!({"limit": 10})).unwrap();
    let cmds: Vec<&str> = h
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["command"].as_str().unwrap())
        .collect();
    assert_eq!(&cmds[..2], ["book.password_changed", "book.encrypted"]);
    let text = h.to_string();
    for secret in [PW, "a brand new passphrase", key.as_str(), &key[5..]] {
        assert!(!text.contains(secret), "secret in history");
    }
    for e in h.as_array().unwrap().iter().take(2) {
        assert!(e["before"].is_null() && e["after"].is_null());
    }
    // Undo passes over session records and undoes the account creation.
    let u = s.invoke("history.undo", json!({})).unwrap();
    assert_eq!(u["command"], "account.create");
    assert!(s.is_encrypted());
}

#[test]
fn remove_encryption_restores_a_plain_book() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, path, _) = new_book(dir.path(), false);
    s.invoke(
        "account.create",
        json!({"name": "Checking", "type": "asset"}),
    )
    .unwrap();
    assert_eq!(
        code(s.remove_encryption(&pw("not the password"))),
        "wrong_password"
    );
    s.remove_encryption(&pw(PW)).unwrap();
    assert!(!s.is_encrypted());
    assert!(!dir.path().join("home.moneykeep.prev").exists());
    s.invoke("account.create", json!({"name": "After", "type": "asset"}))
        .unwrap();
    drop(s);
    assert!(
        std::fs::read(&path)
            .unwrap()
            .starts_with(b"SQLite format 3\0")
    );
    let mut p = Session::open(&path, "t").unwrap();
    let n = names(&mut p);
    assert!(n.contains(&"Checking".to_owned()) && n.contains(&"After".to_owned()));
    let h = p.invoke("history.list", json!({"limit": 5})).unwrap();
    assert!(
        h.as_array()
            .unwrap()
            .iter()
            .any(|e| e["command"] == "book.decrypted")
    );
    // Short passwords are refused.
    assert_eq!(code(p.encrypt("short", false)), "invalid_args");
}
