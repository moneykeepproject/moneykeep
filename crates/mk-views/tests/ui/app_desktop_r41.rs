//! app-desktop.md revision 41 test 230 in the app: Settings → Advanced → Reset
//! MoneyKeep settings… (shown with or without phone capture): the preview of what is
//! cleared and what is not touched, the saved copy's name and place, Reset enabled
//! only after typing RESET, then the start screen as on first run. (The files —
//! the saved copy, books and backups byte for byte — are tested in the app
//! crate's `config` module.)
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::ctl::App;
use mk_ui::{AppEvent, Screen, Theme};
use mk_views::testing::Role;

fn shows(h: &Harness, part: &str) -> bool {
    labels_containing(h, "").iter().any(|l| l.contains(part))
}

#[test]
fn t230_reset_settings_needs_reset_and_returns_to_first_run() {
    let mut config = iso_config();
    config.theme = Theme::Dark;
    config.start_sort = "name".into();
    config.backup_memo.made = 4;
    let (mut app, _book): (App, _) = new_book_app_with(config);
    app.set_today("2026-11-09");
    app.set_settings_folder("/home/owner/.config/MoneyKeep");
    let mut h = harness(app);
    h.run();
    assert!(!h.state_mut().config.recent.is_empty());
    h.state_mut().open_view_spec("settings");
    h.run();
    // Advanced is there without phone capture (the expenses module is off).
    h.get_by_role_and_label(Role::Tab, "Advanced").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Button, "Reset MoneyKeep settings…")
        .click();
    h.run_steps(3);
    for part in [
        "Cleared on this computer:",
        "The recent books list",
        "Backup settings",
        "Not touched:",
        "Your books and everything stored inside them",
        "Your backup files",
        "Phone capture (it has its own Start over",
        "settings-before-reset-2026-11-09.json in /home/owner/.config/MoneyKeep",
        "start screen as on first run",
    ] {
        assert!(shows(&h, part), "{part}: {:#?}", labels_containing(&h, ""));
    }
    let reset_disabled = |h: &Harness| {
        h.get_by_role_and_label(Role::Button, "Reset")
            .accesskit_node()
            .is_disabled()
    };
    assert!(reset_disabled(&h));
    let field = "Type RESET to confirm";
    h.get_by_role_and_label(Role::TextInput, field).focus();
    h.run();
    h.get_by_role_and_label(Role::TextInput, field)
        .type_text("RESE");
    h.run_steps(2);
    assert!(reset_disabled(&h));
    h.get_by_role_and_label(Role::TextInput, field)
        .type_text("T");
    h.run_steps(2);
    assert!(!reset_disabled(&h));
    let _ = events(&mut h);
    h.get_by_role_and_label(Role::Button, "Reset").click();
    h.run_steps(3);
    let ev = events(&mut h);
    assert!(
        ev.contains(&AppEvent::ResetSettings {
            copy_name: "settings-before-reset-2026-11-09.json".into()
        }),
        "{ev:?}"
    );
    // Nothing changed until the shell saved the copy; a failed copy changes nothing.
    assert_eq!(h.state_mut().config.theme, Theme::Dark);
    h.state_mut().finish_settings_reset(Err("disk full".into()));
    h.run();
    assert_eq!(h.state_mut().config.theme, Theme::Dark);
    assert!(h.state_mut().screen() == Screen::Main);
    h.state_mut().finish_settings_reset(Ok(
        "/home/owner/.config/MoneyKeep/settings-before-reset-2026-11-09.json".into(),
    ));
    h.run_steps(3);
    {
        let app = h.state_mut();
        assert!(app.screen() == Screen::Start);
        assert!(app.config.recent.is_empty());
        assert_eq!(app.config.theme, Theme::default());
        assert_eq!(app.config.start_sort, "");
        assert_eq!(app.config.backup_memo.made, 4);
    }
    assert!(events(&mut h).contains(&AppEvent::ConfigChanged));
    assert!(shows(&h, "MoneyKeep settings were reset."));
}
