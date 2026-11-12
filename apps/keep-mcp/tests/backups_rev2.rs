//! backups.md revision 2, BK12: a book needing a format update opened through
//! keep-mcp gets a `before-update` backup (and no other); with the backup folder
//! unwritable the MCP call returns the message, the book stays closed and its file
//! is byte-for-byte unchanged.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use std::path::{Path, PathBuf};

use keep_mcp::{Router, Target};
use mk_commands::Registry;
use mk_commands::backups::{BackupEnv, BackupSettings};
use mk_mcp::Server;
use serde_json::{Value, json};

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

fn server(book: &Path, env: BackupEnv) -> Server {
    let r = Router::new(
        Target::Book(book.to_path_buf()),
        None,
        Box::new(Registry::with_builtins),
    )
    .with_backup_env(env);
    Server::new(Box::new(r))
}

fn call(s: &mut Server, id: i64, tool: &str) -> Value {
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "agent", "version": "1"},
    });
    let out = s.handle_line(
        &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {"name": tool, "arguments": {}, "_meta": meta}})
        .to_string(),
    );
    out.into_iter().next().unwrap()
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
fn backups_bk12_keep_mcp_backs_up_before_an_update() {
    let dir = tempfile::tempdir().unwrap();
    let book = old_book(dir.path());
    let folder = dir.path().join("Backups");
    let mut s = server(&book, env(&folder));
    // The call opens the book (AI access is off in this book, so the call itself
    // is refused, but the book was opened and updated).
    let reply = call(&mut s, 1, "book.info");
    assert!(reply.get("error").is_some() || reply.get("result").is_some());
    let made = files(&folder.join("Household"));
    assert_eq!(made.len(), 1, "{made:?}");
    assert!(made[0].ends_with(" before-update.moneykeep"), "{made:?}");
    let checked = mk_store::check_backup(
        &folder.join("Household").join(&made[0]),
        mk_store::Unlock::None,
    )
    .unwrap();
    assert_eq!(
        checked.info.schema_version,
        mk_store::latest_schema_version() - 1,
        "taken before the update"
    );
    // The book itself was updated.
    let store = mk_store::Store::open(&book).unwrap();
    assert_eq!(
        store.info().unwrap().schema_version,
        mk_store::latest_schema_version()
    );
    drop(store);
    // Only the before-update backup: more calls make no opened/closed backups.
    let _ = call(&mut s, 2, "book.info");
    drop(s);
    assert_eq!(files(&folder.join("Household")), made);
}

#[test]
fn backups_bk12_keep_mcp_backup_failure_leaves_the_book_closed_and_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let book = old_book(dir.path());
    // A file where the folder should be: the backup folder cannot be written (this
    // also holds when the tests run as root).
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, b"not a folder").unwrap();
    let bytes = std::fs::read(&book).unwrap();
    let mut s = server(&book, env(&blocked));
    for id in 1..=2 {
        let reply = call(&mut s, id, "book.info");
        let message = reply["error"]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("an error: {reply}"));
        assert!(message.starts_with(MSG), "{message}");
    }
    drop(s);
    assert_eq!(
        std::fs::read(&book).unwrap(),
        bytes,
        "byte-for-byte unchanged"
    );
    for suffix in ["-wal", "-shm"] {
        let mut p = book.as_os_str().to_owned();
        p.push(suffix);
        let p = PathBuf::from(p);
        assert!(
            !p.exists() || std::fs::metadata(&p).unwrap().len() == 0,
            "no journal written: {}",
            p.display()
        );
    }
}
