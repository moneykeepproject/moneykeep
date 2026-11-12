//! security-review-1 on the desktop UI: SR-B2 (adding a password asks about the
//! book's earlier plain backups; Keep them leaves a Home note with the same two
//! actions) and B4 (About MoneyKeep's Third-party licences link). The files' part
//! of SR-B2 is mk-commands' `security_review_spec`.

use std::path::Path;

use crate::common::*;
use mk_commands::Session;
use mk_ui::ctl::App;
use mk_ui::{AppEvent, BackupPrefs, UserConfig, View};
use serde_json::json;

const PW: &str = "correct horse battery";

/// A plain book "Household" with 3 backups in the backup folder, and the app with
/// it open (no backup on open, so the count stays 3).
fn setup(dir: &Path) -> Harness {
    let folder = dir.join("Backups");
    let cfg = UserConfig {
        backup: BackupPrefs {
            folder: Some(folder.to_str().unwrap().to_owned()),
            on_open: false,
            on_close: false,
            daily: false,
            ..BackupPrefs::default()
        },
        ..iso_config()
    };
    let path = dir.join("Household.moneykeep");
    let mut s = Session::create(&path, "Household", "USD", "test").unwrap();
    s.invoke("account.create", json!({"name": "Cash", "type": "asset"}))
        .unwrap();
    s.set_backup_env(mk_commands::backups::BackupEnv::new(
        mk_commands::backups::BackupSettings::from_json(
            &serde_json::to_value(&cfg.backup).unwrap(),
        ),
        0,
    ));
    for _ in 0..3 {
        s.invoke("book.backup", json!({})).unwrap();
    }
    drop(s);
    let app = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        cfg,
    );
    let mut h = harness(app);
    h.state_mut().open_book(path.to_str().unwrap());
    h.run();
    h
}

fn type_secret(h: &mut Harness, label: &str, text: &str) {
    h.get_by_label(label).focus();
    h.run();
    h.get_by_label(label).type_text(text);
    h.run();
}

fn add_password(h: &mut Harness) {
    click(h, "Settings");
    click(h, "Security");
    click(h, "Protect with a password…");
    type_secret(h, "New password", PW);
    type_secret(h, "Confirm new password", PW);
    click(h, "Protect");
    // The recovery key first, then the question.
    assert!(has(h, "Your recovery key"));
    assert!(!has(h, "Earlier backups"));
    click(h, "I've stored this key somewhere safe");
    click(h, "Done");
}

fn files(dir: &Path) -> Vec<Vec<u8>> {
    let mut v: Vec<_> = std::fs::read_dir(dir.join("Backups").join("Household"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| std::fs::read(e.path()).unwrap())
        .collect();
    v.sort();
    v
}

const QUESTION: &str = "3 earlier backups of this book aren't password-protected.";

#[test]
fn sr_b2_adding_a_password_asks_and_protect_now_protects_them() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = setup(dir.path());
    add_password(&mut h);
    assert!(has(&h, "Earlier backups"));
    assert!(has(&h, QUESTION), "the count is shown");
    for b in ["Protect them now", "Delete them", "Keep them"] {
        assert!(has(&h, b), "{b}");
    }
    click(&mut h, "Protect them now");
    assert!(!has(&h, "Earlier backups"));
    let msg = h.state().message_for_tests().unwrap().0;
    assert_eq!(
        msg,
        "Protected 3 earlier backups with this book's password."
    );
    let f = files(dir.path());
    assert_eq!(f.len(), 3, "3 backups, no plain copies left");
    assert!(f.iter().all(|b| b.starts_with(b"MCBOOKE1")));
    // Each opens with the password.
    let pw = mk_commands::Secret::Password(PW.into());
    for e in std::fs::read_dir(dir.path().join("Backups").join("Household")).unwrap() {
        let p = e.unwrap().path();
        Session::open_with(&p, "test", Some(&pw)).unwrap();
    }
    // No Home note afterwards.
    h.state_mut().navigate(View::Home);
    h.run();
    assert!(
        !h.state_mut()
            .backup_notices()
            .iter()
            .any(|n| n.contains("password-protected")),
    );
}

#[test]
fn sr_b2_keep_them_leaves_a_home_note_with_the_two_actions() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = setup(dir.path());
    add_password(&mut h);
    assert_eq!(
        h.state().unprotected_backups_question().as_deref(),
        Some(QUESTION)
    );
    click(&mut h, "Keep them");
    assert!(!has(&h, "Earlier backups"));
    assert!(h.state().unprotected_backups_question().is_none());
    let note = "3 backups of Household aren't password-protected.";
    let notices = h.state_mut().backup_notice_buttons();
    let (_, buttons) = notices
        .iter()
        .find(|(t, _)| t == note)
        .unwrap_or_else(|| panic!("{notices:?}"));
    assert_eq!(buttons, &["Protect them now", "Delete them"]);
    h.state_mut().navigate(View::Home);
    h.run();
    assert!(has(&h, note));
    // Delete them from the note.
    click(&mut h, &format!("Delete them: {note}"));
    let msg = h.state().message_for_tests().unwrap().0;
    assert_eq!(
        msg,
        "Deleted 3 earlier backups that weren't password-protected."
    );
    assert!(files(dir.path()).is_empty());
    h.run();
    assert!(!has(&h, note));
}

#[test]
fn sr_b4_about_links_to_the_third_party_licences() {
    let (mut h, _b) = harness_with_chart();
    h.get_by_role_and_label(mk_views::testing::Role::Button, "About MoneyKeep")
        .click();
    h.run();
    events(&mut h);
    assert!(has(&h, "Third-party components"));
    click(&mut h, "Third-party licences");
    assert!(
        events(&mut h)
            .iter()
            .any(|e| matches!(e, AppEvent::OpenThirdPartyLicences))
    );
}
