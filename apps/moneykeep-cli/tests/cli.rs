//! End-to-end CLI tests. Test helpers may unwrap (test code only).
#![allow(clippy::unwrap_used)]

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_moneykeep"))
}

fn run_ok(args: &[&str]) -> serde_json::Value {
    let out = bin().args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn init_enable_and_audit_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("t.moneykeep");
    let book = book.to_str().unwrap();

    let created = run_ok(&["init", book, "--name", "Church", "--currency", "USD"]);
    assert_eq!(created["book"]["name"], "Church");

    run_ok(&["run", book, "module.enable", r#"{"module":"funds"}"#]);
    let modules = run_ok(&["run", book, "module.list"]);
    let funds = modules
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "funds")
        .unwrap();
    assert_eq!(funds["enabled"], true);

    let audit = run_ok(&["run", book, "audit.list"]);
    assert_eq!(audit[0]["command"], "module.enable");
    assert_eq!(audit[0]["actor"], "cli");
}

#[test]
fn errors_are_json_with_codes_and_nonzero_exit() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("t.moneykeep");
    let book = book.to_str().unwrap();
    run_ok(&["init", book]);

    let out = bin()
        .args(["run", book, "no.such.command"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["error"]["code"], "unknown_command");
}

#[test]
fn init_refuses_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("t.moneykeep");
    let book = book.to_str().unwrap();
    run_ok(&["init", book]);
    assert!(
        !bin()
            .args(["init", book])
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn ledger_conveniences_wrap_registry_commands() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("t.moneykeep");
    let book = book.to_str().unwrap();
    run_ok(&["init", book]);
    run_ok(&[
        "run",
        book,
        "account.create",
        r#"{"name":"Checking","type":"asset","subtype":"bank"}"#,
    ]);
    run_ok(&[
        "run",
        book,
        "txn.create",
        r#"{"date":"2026-07-01","splits":[{"account":"Checking","value":"1000.00"},{"account":"Opening Balances","value":"-1000.00"}]}"#,
    ]);
    let accounts = run_ok(&["accounts", book]);
    assert_eq!(accounts.as_array().unwrap().len(), 2);
    let bal = run_ok(&["balance", book, "Checking"]);
    assert_eq!(bal["display"]["USD"], "1000.00");
    let tb = run_ok(&["trial-balance", book]);
    assert_eq!(tb["balanced"], true);
    run_ok(&["undo", book]);
    let after_undo = run_ok(&["balance", book, "Checking"]);
    assert_eq!(after_undo["raw"], serde_json::json!({}));
    assert_eq!(after_undo["display"]["USD"], "0.00");
    assert!(after_undo.get("shown").is_none());
    run_ok(&["redo", book]);
    let hist = run_ok(&["history", book, "--limit", "1"]);
    assert_eq!(hist[0]["command"], "history.redo");
}

#[test]
fn second_writer_gets_a_clear_busy_error() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("t.moneykeep");
    let book_s = book.to_str().unwrap();
    run_ok(&["init", book_s]);
    // Hold a write lock from another connection, as a running MCP server mid-write would.
    let mut holder = mk_store::Store::open(&book).unwrap();
    let out = std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        scope.spawn(move || {
            holder
                .write(|_| -> mk_store::StoreResult<()> {
                    tx.send(()).unwrap();
                    done_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap();
        });
        rx.recv().unwrap();
        let out = bin()
            .args(["run", book_s, "module.enable", r#"{"module":"estimates"}"#])
            .output()
            .unwrap();
        done_tx.send(()).unwrap();
        out
    });
    assert!(!out.status.success());
    let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["error"]["code"], "busy", "{err}");
}

fn fixture(path: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/fixtures")
        .join(path)
}

#[test]
fn import_pipeline_at10_watch_folder() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("t.moneykeep");
    let book = book.to_str().unwrap();
    let watch = dir.path().join("inbox");
    std::fs::create_dir(&watch).unwrap();
    // A file outside the watch folder must never be touched.
    let outside = dir.path().join("outside.qif");
    std::fs::copy(fixture("qif/bank-three.qif"), &outside).unwrap();
    run_ok(&["init", book]);
    run_ok(&[
        "run",
        book,
        "account.create",
        r#"{"name":"Checking","type":"asset","subtype":"bank"}"#,
    ]);
    let set = format!(
        r#"{{"module":"core","key":"import.watch_folder","value":{}}}"#,
        serde_json::json!(watch.to_str().unwrap())
    );
    run_ok(&["run", book, "setting.set", &set]);
    std::fs::copy(fixture("qif/bank-three.qif"), watch.join("october.qif")).unwrap();
    std::fs::write(watch.join("broken.qif"), "!Type:Nonsense\nD1/1/2026\n").unwrap();
    std::fs::write(watch.join("notes.txt"), "not an import").unwrap();

    let out = run_ok(&["watch", book, "--once", "--settle", "0"]);
    let out = out.as_array().unwrap();
    assert_eq!(out.len(), 2, "{out:?}");
    assert!(watch.join("imported/october.qif").exists());
    assert!(watch.join("failed/broken.qif").exists());
    let note = std::fs::read_to_string(watch.join("failed/broken.qif.error.txt")).unwrap();
    assert!(note.contains("line 1"), "{note}");
    assert!(!watch.join("october.qif").exists() && !watch.join("broken.qif").exists());
    assert!(
        watch.join("notes.txt").exists(),
        "unsupported files are left alone"
    );
    assert!(outside.exists());
    // auto_commit is false: staged, not committed, nothing in the ledger.
    let staged = out.iter().find(|o| o.get("staged").is_some()).unwrap();
    assert_eq!(staged["committed"], false);
    let batches = run_ok(&["run", book, "import.list", r#"{"status":"staged"}"#]);
    assert_eq!(batches.as_array().unwrap().len(), 1);
    assert_eq!(
        run_ok(&["run", book, "txn.list"]).as_array().unwrap().len(),
        0
    );
    let hist = run_ok(&["history", book, "--limit", "1"]);
    assert_eq!(hist[0]["actor"], "watch");
    // A second file with the same name is archived with a numeric suffix.
    std::fs::copy(fixture("qif/bank-three.qif"), watch.join("october.qif")).unwrap();
    run_ok(&["watch", book, "--once", "--settle", "0"]);
    assert!(watch.join("imported/october (1).qif").exists());
}

#[test]
fn import_subcommand_reads_a_path() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("t.moneykeep");
    let book = book.to_str().unwrap();
    run_ok(&["init", book]);
    run_ok(&[
        "run",
        book,
        "account.create",
        r#"{"name":"Checking","type":"asset","subtype":"bank"}"#,
    ]);
    let f = fixture("qif/bank-three.qif");
    let batch = run_ok(&["import", book, f.to_str().unwrap(), "--account", "Checking"]);
    assert_eq!(batch["rows"].as_array().unwrap().len(), 3);
    assert_eq!(batch["source"], "bank-three.qif");
    let commit = format!(r#"{{"batch":"{}"}}"#, batch["id"].as_str().unwrap());
    let out = run_ok(&["run", book, "import.commit", &commit]);
    assert_eq!(out["created"], 3);
}

// ----- book-encryption.md ----------------------------------------------------------

fn encrypted_book(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("locked.moneykeep");
    let (mut s, _) = mk_commands::Session::create_encrypted(
        &path,
        "Locked",
        "USD",
        "test",
        "correct horse battery",
        false,
        mk_store::KdfParams::TEST,
    )
    .unwrap();
    s.invoke(
        "account.create",
        serde_json::json!({"name": "Secret Savings", "type": "asset"}),
    )
    .unwrap();
    path
}

#[test]
fn book_encryption_at11_cli_password_sources() {
    let dir = tempfile::tempdir().unwrap();
    let path = encrypted_book(dir.path());
    let book = path.to_str().unwrap();
    // MONEYKEEP_PASSWORD.
    let out = bin()
        .args(["accounts", book])
        .env("MONEYKEEP_PASSWORD", "correct horse battery")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Secret Savings"));
    // No secret and no terminal: password_required.
    let out = bin()
        .args(["accounts", book])
        .env_remove("MONEYKEEP_PASSWORD")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["error"]["code"], "password_required");
    // Wrong password.
    let out = bin()
        .args(["accounts", book])
        .env("MONEYKEEP_PASSWORD", "wrong one")
        .output()
        .unwrap();
    let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["error"]["code"], "wrong_password");
    // --password-file (first line).
    let pwfile = dir.path().join("pw.txt");
    std::fs::write(&pwfile, "correct horse battery\nignored\n").unwrap();
    let out = bin()
        .args([
            "accounts",
            book,
            "--password-file",
            pwfile.to_str().unwrap(),
        ])
        .env_remove("MONEYKEEP_PASSWORD")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn cli_encrypt_passwd_recovery_decrypt_remove() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("plain.moneykeep");
    let book_s = book.to_str().unwrap();
    run_ok(&["init", book_s, "--name", "Plain"]);
    // Backups the password commands make go here (security-review-1 L7).
    let bf = dir.path().join("Backups");
    let bf = bf.to_str().unwrap();
    let newpw = dir.path().join("new.txt");
    std::fs::write(&newpw, "first passphrase here\n").unwrap();
    let out = bin()
        .args([
            "encrypt",
            book_s,
            "--backup-folder",
            bf,
            "--new-password-file",
            newpw.to_str().unwrap(),
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let key = v["recovery_key"].as_str().unwrap().to_owned();
    assert!(key.starts_with("MCRK-"));
    assert!(std::fs::read(&book).unwrap().starts_with(b"MCBOOKE1"));
    // Change the password, open with the recovery key, write a plain copy.
    std::fs::write(&newpw, "second passphrase here\n").unwrap();
    let out = bin()
        .args([
            "passwd",
            book_s,
            "--backup-folder",
            bf,
            "--new-password-file",
            newpw.to_str().unwrap(),
        ])
        .env("MONEYKEEP_PASSWORD", "first passphrase here")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let copy = dir.path().join("copy.moneykeep");
    let out = bin()
        .args([
            "decrypt",
            book_s,
            "--backup-folder",
            bf,
            "--out",
            copy.to_str().unwrap(),
            "--recovery-key",
        ])
        .env("MONEYKEEP_PASSWORD", &key)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        std::fs::read(&copy)
            .unwrap()
            .starts_with(b"SQLite format 3\0")
    );
    let out = bin()
        .args(["remove-encryption", book_s, "--yes", "--backup-folder", bf])
        .env("MONEYKEEP_PASSWORD", "second passphrase here")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        std::fs::read(&book)
            .unwrap()
            .starts_with(b"SQLite format 3\0")
    );
    let h = run_ok(&["history", book_s]);
    let cmds: Vec<&str> = h
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["command"].as_str().unwrap())
        .collect();
    assert_eq!(
        &cmds[..3],
        ["book.decrypted", "book.password_changed", "book.encrypted"]
    );
}

/// connectivity.md scheduling: `moneykeep sync` runs connect.sync (no connections:
/// nothing is fetched, nothing reaches the network).
#[test]
fn sync_subcommand_runs_connect_sync() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("t.moneykeep");
    let book = book.to_str().unwrap();
    run_ok(&["init", book, "--name", "Feeds", "--currency", "USD"]);
    let out = bin().args(["sync", book]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("module_disabled"));
    run_ok(&["run", book, "module.enable", r#"{"module":"bank-feeds"}"#]);
    let v = run_ok(&["sync", book]);
    assert_eq!(v["connections"], serde_json::json!([]));
    let p = run_ok(&["run", book, "connect.providers"]);
    assert_eq!(p[0]["network"], true);
}

/// app-desktop.md revision 20, acceptance test 124: the desktop's date format is a
/// per-user display setting; the CLI keeps ISO dates.
#[test]
fn app_desktop_at124_cli_keeps_iso_dates() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("t.moneykeep");
    let book = book.to_str().unwrap();
    run_ok(&["init", book]);
    run_ok(&[
        "run",
        book,
        "account.create",
        r#"{"name":"Checking","type":"asset","subtype":"bank"}"#,
    ]);
    run_ok(&[
        "run",
        book,
        "txn.create",
        r#"{"date":"2026-10-22","splits":[{"account":"Checking","value":"5.00"},{"account":"Opening Balances","value":"-5.00"}]}"#,
    ]);
    let list = run_ok(&["run", book, "txn.list"]);
    assert_eq!(list[0]["date"], "2026-10-22");
    // The MCP server (keep-mcp) keeps ISO dates too: apps/keep-mcp/tests/rev3.rs.
}

/// branding.md AT10: `moneykeep --version` and the help say MoneyKeep.
#[test]
fn branding_at10_version_says_moneykeep() {
    let out = bin().arg("--version").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        text.trim(),
        format!("MoneyKeep {}", env!("CARGO_PKG_VERSION"))
    );
    let help = bin().arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(
        help.contains("MoneyKeep — Apps for Financial Management"),
        "{help}"
    );
    assert!(help.contains("Usage: moneykeep"), "{help}");
}

/// mcp-server.md revision 3 AT27 (CLI part): `moneykeep mcp` no longer exists;
/// `moneykeep ai-access` changes and prints a book's AI access settings.
#[test]
fn at27_cli_ai_access_and_no_mcp_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("t.moneykeep");
    let book = book.to_str().unwrap();
    run_ok(&["init", book]);
    let out = bin().args(["mcp", book]).output().unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("unrecognized subcommand"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let help = bin().arg("--help").output().unwrap();
    assert!(!String::from_utf8_lossy(&help.stdout).contains("Model Context Protocol"));
    // Defaults: off, read only, no groups, only through the app.
    assert_eq!(
        run_ok(&["ai-access", book]),
        serde_json::json!({"enabled": false, "level": "read_only", "groups": [], "app_only": true})
    );
    run_ok(&[
        "ai-access",
        book,
        "--on",
        "--level",
        "read-write",
        "--groups",
        "delete",
        "--direct",
    ]);
    assert_eq!(
        run_ok(&["ai-access", book]),
        serde_json::json!({"enabled": true, "level": "read_write", "groups": ["delete"], "app_only": false})
    );
    // Recorded as a session record, and registry commands cannot change it.
    let hist = run_ok(&["history", book, "--limit", "1"]);
    assert_eq!(hist[0]["command"], "book.ai_access_changed");
    let out = bin()
        .args([
            "run",
            book,
            "setting.set",
            r#"{"module":"core","key":"ai_access.enabled","value":false}"#,
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["error"]["code"], "invalid_args");
    let bad = bin()
        .args(["ai-access", book, "--groups", "everything"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
}

#[test]
fn init_with_a_chart_preset() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("p.moneykeep");
    let book = book.to_str().unwrap();
    let created = run_ok(&["init", book, "--preset", "personal_1040"]);
    assert_eq!(created["preset"], "personal_1040");
    assert_eq!(created["book"]["chart"]["tax_year"], 2025);
    let w2 = run_ok(&["run", book, "account.get", r#"{"account":"4010"}"#]);
    assert_eq!(w2["tax_line"], "1040 L1a");
    // An unknown preset writes no file.
    let other = dir.path().join("x.moneykeep");
    let out = bin()
        .args(["init", other.to_str().unwrap(), "--preset", "x"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Unknown chart preset 'x'")
    );
    assert!(!other.exists());
}

/// backups.md: `moneykeep backup <book>` and `moneykeep backups <book>`.
#[test]
fn backups_backup_and_list_from_the_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("Household.moneykeep");
    let book = book.to_str().unwrap();
    let folder = dir.path().join("Backups");
    let folder = folder.to_str().unwrap();
    run_ok(&["init", book, "--name", "Household"]);
    let made = run_ok(&["backup", book, "--folder", folder]);
    let path = made["path"].as_str().unwrap();
    assert!(path.ends_with(" manual.moneykeep"), "{path}");
    assert!(std::path::Path::new(path).starts_with(dir.path().join("Backups").join("Household")));
    let list = run_ok(&["backups", book, "--folder", folder]);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["reason"], "manual");
    assert_eq!(list[0]["path"], path);
}

/// security-review-1 SR-B5 (L7) and SR-B2 on the command line: `encrypt` (adding a
/// password) makes the before-update backup first, then says how many earlier
/// backups aren't protected and offers `moneykeep backups protect <book>`.
#[test]
fn sr_b5_l7_and_sr_b2_password_add_backs_up_first_and_offers_protect() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("Household.moneykeep");
    let book_s = book.to_str().unwrap();
    let bf = dir.path().join("Backups");
    let bf = bf.to_str().unwrap();
    run_ok(&["init", book_s, "--name", "Household"]);
    run_ok(&["backup", book_s, "--folder", bf]);
    run_ok(&["backup", book_s, "--folder", bf]);
    let newpw = dir.path().join("new.txt");
    std::fs::write(&newpw, "first passphrase here\n").unwrap();
    let out = bin()
        .args([
            "encrypt",
            book_s,
            "--no-recovery-key",
            "--backup-folder",
            bf,
        ])
        .args(["--new-password-file", newpw.to_str().unwrap()])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    // The backup first: made from the book before the password was added.
    assert_eq!(v["backup"]["reason"], "before-update", "{v}");
    assert_eq!(v["backup"]["encrypted"], false, "{v}");
    let made = std::path::PathBuf::from(v["backup"]["path"].as_str().unwrap());
    assert!(
        std::fs::read(&made)
            .unwrap()
            .starts_with(b"SQLite format 3\0")
    );
    assert!(std::fs::read(&book).unwrap().starts_with(b"MCBOOKE1"));
    // The count and the offer.
    assert_eq!(v["unprotected_backups"]["count"], 3, "{v}");
    assert_eq!(
        v["unprotected_backups"]["message"],
        "3 earlier backups of this book aren't password-protected."
    );
    let protect = v["unprotected_backups"]["protect"].as_str().unwrap();
    assert!(
        protect.starts_with("moneykeep backups protect "),
        "{protect}"
    );
    assert!(stderr.contains("3 earlier backups of this book aren't password-protected."));
    assert!(stderr.contains("moneykeep backups protect"), "{stderr}");
    // moneykeep backups protect <book>.
    let out = bin()
        .args(["backups", "protect", book_s, "--folder", bf])
        .env("MONEYKEEP_PASSWORD", "first passphrase here")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["done"], 3, "{r}");
    let list = run_ok(&["backups", book_s, "--folder", bf]);
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 3);
    assert!(list.iter().all(|b| b["encrypted"] == true), "{list:?}");
    // A failed backup stops the password change: nothing is changed.
    let blocked = dir.path().join("not-a-folder");
    std::fs::write(&blocked, b"a file").unwrap();
    let before = std::fs::read(&book).unwrap();
    std::fs::write(&newpw, "second passphrase here\n").unwrap();
    let out = bin()
        .args([
            "passwd",
            book_s,
            "--backup-folder",
            blocked.to_str().unwrap(),
        ])
        .args(["--new-password-file", newpw.to_str().unwrap()])
        .env("MONEYKEEP_PASSWORD", "first passphrase here")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["error"]["code"], "backup_failed", "{err}");
    assert_eq!(std::fs::read(&book).unwrap(), before);
}
