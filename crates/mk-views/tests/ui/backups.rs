//! backups.md acceptance tests on the desktop UI: backing up on open and close with
//! the status messages (BK1, BK3), the second place's Attention item (BK6), the
//! suggestion every 10th backup (BK7), Restore → Open as a copy with the note (BK8),
//! "No backup for 14 days" (BK10) and, from revision 2, the start-screen card's
//! Back up this book… (BK15). BK11 (screenshots) is the `backup_screens`
//! example; BK2, BK4, BK5 and BK9 are mk-commands' `backups_spec`.

use std::path::{Path, PathBuf};

use crate::common::*;
use mk_commands::Session;
use mk_ui::ctl::App;
use mk_ui::{AppEvent, BackupPrefs, Screen, UserConfig, View};
use serde_json::json;

const T0: i64 = 1_793_000_000;

fn config(dir: &Path) -> UserConfig {
    UserConfig {
        backup: BackupPrefs {
            folder: Some(dir.join("Backups").to_str().unwrap().to_owned()),
            ..BackupPrefs::default()
        },
        ..iso_config()
    }
}

/// A book on disk ("Household") and the app on the start screen.
fn setup(dir: &Path, cfg: UserConfig) -> (Harness, PathBuf) {
    let path = dir.join("Household.moneykeep");
    let mut s = Session::create(&path, "Household", "USD", "test").unwrap();
    s.invoke("account.create", json!({"name": "Cash", "type": "asset"}))
        .unwrap();
    drop(s);
    let app = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        cfg,
    );
    let mut h = harness(app);
    h.state_mut().set_backup_clock_for_tests(T0);
    (h, path)
}

fn backups_in(dir: &Path) -> Vec<String> {
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

fn open(h: &mut Harness, path: &Path) {
    h.state_mut().open_book(path.to_str().unwrap());
    h.advance(0.3);
}

fn back_up_now(h: &mut Harness) {
    if h.state().view() != &View::Settings || h.state().config.settings_section != "Backups" {
        h.state_mut().navigate(View::Settings);
        h.state_mut().config.settings_section = "Backups".into();
        h.run();
    }
    h.get_by_label("Back up now").click();
    h.run_steps(3);
}

#[test]
fn backups_bk1_bk3_open_and_close_with_status_messages() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = setup(dir.path(), config(dir.path()));
    h.state_mut().open_book(path.to_str().unwrap());
    h.run();
    let folder = dir.path().join("Backups").join("Household");
    let files = backups_in(&folder);
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].ends_with(" opened.moneykeep"));
    let msg = h.state().message_for_tests().unwrap().0;
    assert!(msg.starts_with("Backed up "), "{msg}");
    assert!(msg.ends_with(" AM") || msg.ends_with(" PM"), "{msg}");
    // Closing without a change makes no backup.
    h.state_mut().close_book();
    h.advance(0.3);
    assert_eq!(backups_in(&folder).len(), 1);
    // Closing after a change does.
    open(&mut h, &path);
    let n = backups_in(&folder).len();
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Bank", "type": "asset"}),
    );
    h.state_mut().close_book();
    h.advance(0.3);
    let files = backups_in(&folder);
    assert_eq!(files.len(), n + 1);
    assert!(
        files.iter().any(|f| f.ends_with(" closed.moneykeep")),
        "{files:?}"
    );
}

#[test]
fn backups_bk6_second_place_and_its_attention_item() {
    let dir = tempfile::tempdir().unwrap();
    let usb = dir.path().join("USB");
    std::fs::create_dir_all(&usb).unwrap();
    let (mut h, path) = setup(dir.path(), config(dir.path()));
    h.state_mut().set_backup_folder(usb.to_str().unwrap(), true);
    open(&mut h, &path);
    assert_eq!(backups_in(&usb.join("Household")).len(), 1, "copied there");
    // Unplugged: backups still succeed.
    std::fs::remove_dir_all(&usb).unwrap();
    h.state_mut().set_backup_clock_for_tests(T0 + 3 * 86_400);
    back_up_now(&mut h);
    assert_eq!(
        backups_in(&dir.path().join("Backups").join("Household")).len(),
        2
    );
    assert!(
        !h.state_mut()
            .backup_notices()
            .iter()
            .any(|n| n.starts_with("Your second backup place")),
        "not yet: 3 days"
    );
    h.state_mut().set_backup_clock_for_tests(T0 + 8 * 86_400);
    back_up_now(&mut h);
    let notices = h.state_mut().backup_notices();
    let item = notices
        .iter()
        .find(|n| n.starts_with("Your second backup place ("))
        .unwrap_or_else(|| panic!("{notices:?}"));
    assert!(item.contains(usb.to_str().unwrap()), "{item}");
    assert!(item.contains("hasn't been reachable since "), "{item}");
    h.state_mut().navigate(View::Home);
    h.run();
    assert!(has_contains(
        &h,
        "Choose a second place…: Your second backup place ("
    ));
}

#[test]
fn backups_bk7_suggest_a_second_place_every_10th_backup() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = setup(dir.path(), config(dir.path()));
    open(&mut h, &path);
    let suggested = |h: &mut Harness| {
        h.state_mut()
            .backup_notices()
            .iter()
            .any(|n| n == mk_ui_suggestion())
    };
    for _ in 0..8 {
        back_up_now(&mut h);
    }
    assert_eq!(h.state().config.backup_memo.made, 9);
    assert!(!suggested(&mut h));
    back_up_now(&mut h);
    assert_eq!(h.state().config.backup_memo.made, 10);
    assert!(suggested(&mut h), "the 10th backup shows the suggestion");
    h.state_mut().navigate(View::Home);
    h.run();
    click(&mut h, &format!("Not now: {}", mk_ui_suggestion()));
    assert!(!suggested(&mut h));
    for _ in 0..9 {
        back_up_now(&mut h);
    }
    assert!(!suggested(&mut h), "19 backups");
    back_up_now(&mut h);
    assert!(suggested(&mut h), "again at the 20th");
    h.state_mut().navigate(View::Home);
    h.run();
    click(&mut h, &format!("Don't ask again: {}", mk_ui_suggestion()));
    assert_eq!(h.state().config.backup.suggest_second_every, 0);
    assert!(!suggested(&mut h));
}

fn mk_ui_suggestion() -> &'static str {
    "Keep a copy of your backups somewhere else — a USB drive or a cloud folder — so one failed disk can't take your books and their backups together."
}

#[test]
fn backups_bk8_restore_opens_a_copy_with_the_note() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = setup(dir.path(), config(dir.path()));
    open(&mut h, &path);
    h.state_mut().close_book();
    h.advance(0.3);
    let before = std::fs::read(&path).unwrap();
    assert_eq!(h.state().screen(), Screen::Start);
    click(&mut h, "Restore a backup…");
    assert!(has(&h, "Restore a backup"));
    let row = h
        .all_labels()
        .into_iter()
        .find(|l| l.starts_with("Backup of ") && l.contains("Opened"))
        .unwrap_or_else(|| panic!("{:?}", h.all_labels()));
    click(&mut h, &row);
    events(&mut h);
    click(&mut h, "Open as a copy");
    let (dir_shown, file_name) = events(&mut h)
        .into_iter()
        .find_map(|e| match e {
            AppEvent::ChooseRestoreTarget { dir, file_name } => Some((dir, file_name)),
            _ => None,
        })
        .unwrap();
    assert_eq!(dir_shown.as_deref(), dir.path().to_str());
    assert!(
        file_name.starts_with("Household restored 20"),
        "{file_name}"
    );
    assert!(file_name.ends_with(".moneykeep"));
    let target = dir.path().join(&file_name);
    h.state_mut().restore_to(target.to_str().unwrap());
    h.advance(0.3);
    assert_eq!(h.state().screen(), Screen::Main);
    assert!(target.exists());
    assert!(has_contains(
        &h,
        "This is a copy restored from the backup of "
    ));
    assert!(has_contains(&h, "Your original book was not changed."));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the original is unchanged"
    );
    // Dismissible.
    // The note's ×, not the status message pop-up's (app-desktop.md revision 42).
    let r = h
        .get_all_by_label("Dismiss")
        .find(|n| !n.item_id().starts_with("msg/"))
        .map(|n| n.rect())
        .unwrap();
    h.click_at(r.center());
    h.run();
    assert!(!has_contains(
        &h,
        "This is a copy restored from the backup of "
    ));
}

#[test]
fn backups_bk10_no_backup_for_14_days() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path());
    cfg.backup.on_open = false;
    let (mut h, path) = setup(dir.path(), cfg);
    // A backup made 15 days before now (UTC, the test backend's time zone).
    let folder = dir.path().join("Backups").join("Household");
    std::fs::create_dir_all(&folder).unwrap();
    let old = T0 - 15 * 86_400;
    let name = mk_commands::backups::file_name_at(
        "Household",
        old,
        0,
        mk_commands::backups::Reason::Manual,
    );
    std::fs::copy(&path, folder.join(name)).unwrap();
    open(&mut h, &path);
    let notices = h.state_mut().backup_notices();
    assert!(
        notices.contains(&"No backup for 15 days.".to_owned()),
        "{notices:?}"
    );
    h.state_mut().navigate(View::Home);
    h.run();
    assert!(has(&h, "No backup for 15 days."));
    // 13 days old: no warning.
    h.state_mut().set_backup_clock_for_tests(old + 13 * 86_400);
    h.state_mut().close_book();
    open(&mut h, &path);
    assert!(
        !h.state_mut()
            .backup_notices()
            .iter()
            .any(|n| n.starts_with("No backup for")),
    );
}

#[test]
fn backups_restoring_a_protected_backup_asks_for_its_password() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(dir.path());
    let book = dir.path().join("Secret.moneykeep");
    let (mut s, _) = Session::create_encrypted(
        &book,
        "Secret",
        "USD",
        "test",
        "correct horse battery",
        false,
        mk_commands::KdfParams::TEST,
    )
    .unwrap();
    let env = mk_commands::backups::BackupEnv::new(
        mk_commands::backups::BackupSettings::from_json(
            &serde_json::to_value(&cfg.backup).unwrap(),
        ),
        0,
    );
    s.set_backup_env(env);
    let made = s.invoke("book.backup", json!({})).unwrap();
    drop(s);
    let app = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        cfg,
    );
    let mut h = harness(app);
    h.state_mut()
        .restore_from_file(made["path"].as_str().unwrap());
    h.run();
    let file_name = events(&mut h)
        .into_iter()
        .find_map(|e| match e {
            AppEvent::ChooseRestoreTarget { file_name, .. } => Some(file_name),
            _ => None,
        })
        .unwrap();
    assert!(file_name.starts_with("Secret restored "), "{file_name}");
    let target = dir.path().join(file_name);
    h.state_mut().restore_to(target.to_str().unwrap());
    h.run();
    assert!(!target.exists());
    assert!(has(&h, "Backup password"));
    type_into(&mut h, "Backup password", "wrong password here");
    click(&mut h, "Continue");
    assert!(has(&h, "Wrong password."));
    type_into(&mut h, "Backup password", "correct horse battery");
    click(&mut h, "Continue");
    assert_eq!(h.state().screen(), Screen::Main);
    assert!(encrypted_on_disk(&target));
    assert!(has_contains(
        &h,
        "This is a copy restored from the backup of "
    ));
}

/// backups.md revision 2, BK15: start screen card … → Back up this book… makes a
/// `manual` backup of a closed book, without opening it, and reports it.
#[test]
fn backups_bk15_card_backs_up_a_closed_book() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = setup(dir.path(), config(dir.path()));
    // On the list (opened once, then closed).
    open(&mut h, &path);
    h.state_mut().close_book();
    h.advance(0.3);
    assert_eq!(h.state().screen(), Screen::Start);
    let folder = dir.path().join("Backups").join("Household");
    let before = backups_in(&folder);
    let bytes = std::fs::read(&path).unwrap();
    click(&mut h, "More for Household");
    // No book is open here: the menu has Back up this book…, not Back up now.
    assert!(!has(&h, "Back up now"));
    h.get_by_role_and_label(Role::MenuItem, "Back up this book…")
        .click();
    h.run();
    let after = backups_in(&folder);
    let new: Vec<_> = after.iter().filter(|f| !before.contains(f)).collect();
    assert_eq!(new.len(), 1, "{after:?}");
    assert!(new[0].ends_with(" manual.moneykeep"), "{new:?}");
    let (msg, error) = h.state().message_for_tests().unwrap();
    assert!(!error);
    assert!(msg.starts_with("Backed up Household "), "{msg}");
    assert!(msg.ends_with(" AM") || msg.ends_with(" PM"), "{msg}");
    // The book was not opened.
    assert_eq!(h.state().screen(), Screen::Start);
    assert_eq!(h.state().book_name(), None);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "the book is unchanged"
    );
}

/// BK15: for a password-protected book the password is asked for first; a wrong one
/// is refused; the backup stays protected.
#[test]
fn backups_bk15_card_backup_of_a_protected_book_asks_for_the_password() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("Vault.moneykeep");
    drop(
        Session::create_encrypted(
            &vault,
            "Vault",
            "USD",
            "test",
            "correct horse",
            false,
            mk_commands::KdfParams::TEST,
        )
        .unwrap(),
    );
    let mut cfg = config(dir.path());
    cfg.recent = vec![mk_ui::RecentBook::from(vault.to_str().unwrap().to_owned())];
    let app = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        cfg,
    );
    let mut h = harness(app);
    h.state_mut().set_backup_clock_for_tests(T0);
    h.run();
    let folder = dir.path().join("Backups").join("Vault");
    click(&mut h, "More for Vault");
    h.get_by_role_and_label(Role::MenuItem, "Back up this book…")
        .click();
    h.run();
    // The password first: nothing is written yet.
    assert!(has(&h, "Book password"));
    assert!(backups_in(&folder).is_empty());
    type_into(&mut h, "Book password", "wrong");
    click(&mut h, "Back up");
    assert!(has(&h, "Wrong password."));
    assert!(backups_in(&folder).is_empty());
    h.get_by_label("Book password").set_text("");
    h.run();
    type_into(&mut h, "Book password", "correct horse");
    click(&mut h, "Back up");
    assert!(!has(&h, "Book password"), "the dialog closed");
    let files = backups_in(&folder);
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].ends_with(" manual.moneykeep"));
    let msg = h.state().message_for_tests().unwrap().0;
    assert!(msg.starts_with("Backed up Vault "), "{msg}");
    assert_eq!(h.state().screen(), Screen::Start, "not opened");
    // Still protected: it needs the password.
    let backup = folder.join(&files[0]);
    assert_eq!(
        mk_commands::book_kind(&backup).unwrap(),
        mk_commands::BookKind::Encrypted
    );
    assert!(Session::open(&backup, "test").is_err());
    let pw = mk_commands::Secret::Password("correct horse".into());
    Session::open_with(&backup, "test", Some(&pw)).unwrap();
}

// ----- revision 3 and 4 --------------------------------------------------------------------

fn hooks(f: impl FnOnce(&mut BackupHooks)) {
    BACKUP_HOOKS.with(|h| f(&mut h.borrow_mut()));
}

fn settings_backups(h: &mut Harness) {
    h.state_mut().navigate(View::Settings);
    h.state_mut().config.settings_section = "Backups".into();
    h.run();
}

/// backups.md BK17: a chosen folder is tested; one that can't be written is refused
/// with the message and the setting is unchanged; a writable one is saved and the
/// test file is gone.
#[test]
fn backups_bk17_a_chosen_folder_is_tested() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = setup(dir.path(), config(dir.path()));
    open(&mut h, &path);
    settings_backups(&mut h);
    let before = h.state().config.backup.folder.clone();
    let other = dir.path().join("Other");
    std::fs::create_dir_all(&other).unwrap();
    hooks(|k| k.folder_error = Some(std::io::ErrorKind::PermissionDenied));
    h.state_mut()
        .set_backup_folder(other.to_str().unwrap(), false);
    h.run();
    assert!(has(&h, "The backup folder was not changed."));
    assert!(
        has_contains(&h, "MoneyKeep can't write to "),
        "{:?}",
        labels_containing(&h, "write")
    );
    assert!(has(&h, "Choose another folder…"));
    assert!(!has(&h, "How to allow MoneyKeep"), "not a Windows block");
    assert_eq!(h.state().config.backup.folder, before, "not saved");
    click(&mut h, "Close");
    h.run();
    assert!(!has(&h, "The backup folder was not changed."));
    // The second place too.
    h.state_mut()
        .set_backup_folder(other.to_str().unwrap(), true);
    h.run();
    assert!(has(&h, "The second place was not changed."));
    assert_eq!(h.state().config.backup.second_place, None);
    click(&mut h, "Close");
    // Writable: saved, and the test file is gone.
    hooks(|k| k.folder_error = None);
    h.state_mut()
        .set_backup_folder(other.to_str().unwrap(), false);
    h.run();
    assert_eq!(
        h.state().config.backup.folder.as_deref(),
        other.to_str(),
        "saved"
    );
    assert_eq!(std::fs::read_dir(&other).unwrap().count(), 0);
}

/// backups.md BK18: a "not found" failure under a Documents path shows the
/// Controlled folder access message with both buttons; How to allow MoneyKeep opens
/// the guide at "Why not the Documents folder?"; under another path the plain
/// message.
#[test]
fn backups_bk18_windows_blocking_message_and_buttons() {
    let dir = tempfile::tempdir().unwrap();
    // A long path, as on Windows (C:\Users\…\AppData\Local\Temp\…, or under OneDrive):
    // the notice wraps over several lines and its buttons must still show.
    let docs = dir
        .path()
        .join("OneDrive - A Rather Long Organisation Name")
        .join("Shared household papers and records")
        .join("Documents");
    let folder = docs.join("MoneyKeep Backups");
    let mut cfg = config(dir.path());
    cfg.backup.folder = Some(folder.to_str().unwrap().to_owned());
    let (mut h, path) = setup(dir.path(), cfg);
    hooks(|k| {
        k.folder_error = Some(std::io::ErrorKind::NotFound);
        k.protected = Some(vec![docs.clone()]);
    });
    open(&mut h, &path);
    let notices = h.state_mut().backup_notice_buttons();
    let text = format!(
        "Windows blocked MoneyKeep from writing to {}. This is usually Windows' ransomware protection (Controlled folder access).",
        folder.display()
    );
    let (t, buttons) = notices
        .iter()
        .find(|(t, _)| t.contains("Controlled folder access"))
        .unwrap_or_else(|| panic!("{notices:?}"));
    assert_eq!(t, &text);
    assert_eq!(
        buttons,
        &["Choose another folder…", "How to allow MoneyKeep"]
    );
    h.state_mut().navigate(View::Home);
    h.run();
    assert!(has(&h, &text), "{:?}", labels_containing(&h, "Windows"));
    h.get_by_label(&format!("How to allow MoneyKeep: {text}"))
        .click();
    h.run();
    assert!(has(&h, "Why not the Documents folder?"));
    assert!(has_contains(
        &h,
        "Allow an app through Controlled folder access"
    ));
    assert!(!has(&h, "When a backup is made"), "opened at the section");
    click(&mut h, "Show the whole guide");
    h.run();
    assert!(has(&h, "Where your backups are"));
    click(&mut h, "Close help");
    h.run();
    h.get_by_label(&format!("Choose another folder…: {text}"))
        .click();
    h.run();
    assert!(
        events(&mut h)
            .iter()
            .any(|e| matches!(e, AppEvent::ChooseBackupFolder { second: false }))
    );
    // Under another path: the plain message.
    hooks(|k| k.protected = Some(vec![dir.path().join("Pictures")]));
    back_up_now(&mut h);
    let notices = h.state_mut().backup_notices();
    assert!(
        notices
            .iter()
            .any(|n| n.starts_with("MoneyKeep can't write to the backup folder")),
        "{notices:?}"
    );
    assert!(
        !notices
            .iter()
            .any(|n| n.contains("Controlled folder access"))
    );
    hooks(|k| *k = BackupHooks::default());
}

/// backups.md BK19: Settings → Backups → Help shows the guide.
#[test]
fn backups_bk19_settings_help_shows_the_guide() {
    let dir = tempfile::tempdir().unwrap();
    let (mut h, path) = setup(dir.path(), config(dir.path()));
    open(&mut h, &path);
    settings_backups(&mut h);
    h.get_by_label("Backups help").click();
    h.run();
    for heading in [
        "Where your backups are",
        "Why not the Documents folder?",
        "Keep a copy somewhere else (second place)",
        "Getting a book back (restore)",
    ] {
        assert!(has(&h, heading), "{heading}");
    }
    assert!(has_contains(
        &h,
        "MoneyKeep makes copies of your books by itself"
    ));
    click(&mut h, "Close help");
    h.run();
    assert!(!has(&h, "Getting a book back (restore)"));
}

/// backups.md BK21: a failing second-place copy shows the status message and the
/// Attention item with Choose….
#[test]
fn backups_bk21_a_failing_second_place_copy_is_shown() {
    let dir = tempfile::tempdir().unwrap();
    let usb = dir.path().join("USB");
    std::fs::create_dir_all(&usb).unwrap();
    let mut cfg = config(dir.path());
    cfg.backup.second_place = Some(usb.to_str().unwrap().to_owned());
    let (mut h, path) = setup(dir.path(), cfg);
    hooks(|k| k.fail_second = true);
    open(&mut h, &path);
    let msg = "Backed up. The copy to the second place failed: simulated write failure";
    assert!(
        has_contains(&h, msg),
        "{:?}",
        labels_containing(&h, "Backed up")
    );
    let notices = h.state_mut().backup_notice_buttons();
    let text = format!(
        "The copy to your second backup place ({}) failed: simulated write failure",
        usb.display()
    );
    let (_, buttons) = notices
        .iter()
        .find(|(t, _)| *t == text)
        .unwrap_or_else(|| panic!("{notices:?}"));
    assert_eq!(buttons, &["Choose…"]);
    h.state_mut().navigate(View::Home);
    h.run();
    h.get_by_label(&format!("Choose…: {text}")).click();
    h.run();
    assert!(
        events(&mut h)
            .iter()
            .any(|e| matches!(e, AppEvent::ChooseBackupFolder { second: true }))
    );
    // The next good copy clears it.
    hooks(|k| k.fail_second = false);
    back_up_now(&mut h);
    assert!(
        !h.state_mut()
            .backup_notices()
            .iter()
            .any(|n| n.contains("failed"))
    );
}
