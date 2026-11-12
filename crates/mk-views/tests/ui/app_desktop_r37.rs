//! app-desktop.md revision 37: status messages close by themselves (Settings →
//! Operation, Close status messages automatically, After 2–15 seconds); errors
//! stay; the pointer or keyboard focus on the band holds a message. Time is the
//! harness's mocked clock.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::ctl::Ev;
use mk_ui::{MessagePrefs, UserConfig};
use mk_views::testing::Role;

type H = mk_views::testing::Harness;

/// One test step of the harness (its clock moves in quarter seconds).
const STEP: f64 = 0.25;

fn with(prefs: MessagePrefs) -> (H, TestBook) {
    harness_with_chart_config(UserConfig {
        ui: prefs,
        ..iso_config()
    })
}

fn shown(h: &H) -> Option<String> {
    h.state().message_for_tests().map(|(t, _)| t)
}

fn notify(h: &mut H, text: &str, error: bool) {
    h.state_mut().notify_for_tests(text, error);
    h.views().refresh();
    assert_eq!(shown(h).as_deref(), Some(text));
    assert!(has(h, "Dismiss"), "the band shows");
}

/// With the default (on, 3 s) an informational message disappears after 3 s; an
/// error message is still shown after 10 s.
#[test]
fn app_desktop_r37_default_closes_after_three_seconds_errors_stay() {
    let prefs = UserConfig::default().ui;
    assert!(prefs.message_autoclose);
    assert_eq!(prefs.message_seconds, 3);
    let (mut h, _b) = with(prefs);
    notify(&mut h, "Saved.", false);
    h.advance(2.5);
    assert_eq!(shown(&h).as_deref(), Some("Saved."), "at 2.5 s");
    h.advance(0.5 + STEP);
    assert_eq!(shown(&h), None, "gone after 3 s");
    assert!(!has(&h, "Dismiss"), "the band is gone");
    // A new message restarts the time.
    notify(&mut h, "First.", false);
    h.advance(2.0);
    notify(&mut h, "Second.", false);
    h.advance(2.0);
    assert_eq!(shown(&h).as_deref(), Some("Second."), "the time restarted");
    // Errors stay until Dismiss.
    notify(&mut h, "That did not work.", true);
    h.advance(10.0);
    assert_eq!(shown(&h).as_deref(), Some("That did not work."));
    h.get_by_role_and_label(Role::Button, "Dismiss").click();
    h.run();
    assert_eq!(shown(&h), None);
}

/// With 8 s chosen the message is shown at 7 s and gone at 8 s; hovering at 7 s
/// keeps it past 8 s and it closes 8 s after the pointer leaves.
#[test]
fn app_desktop_r37_eight_seconds_and_hover() {
    let (mut h, _b) = with(MessagePrefs {
        message_autoclose: true,
        message_seconds: 8,
    });
    notify(&mut h, "Imported 3 transactions.", false);
    h.advance(7.0);
    assert!(shown(&h).is_some(), "shown at 7 s");
    h.advance(1.0 + STEP);
    assert_eq!(shown(&h), None, "gone at 8 s");

    notify(&mut h, "Imported 4 transactions.", false);
    h.advance(7.0);
    h.get_by_role_and_label(Role::Button, "Dismiss").hover();
    h.advance(3.0);
    assert!(shown(&h).is_some(), "the pointer holds it past 8 s");
    // The pointer leaves the band (to the bottom of the window).
    let size = h.size();
    h.hover_at(pos2(size.x / 2.0, size.y - 10.0));
    h.advance(7.5);
    assert!(shown(&h).is_some(), "7.5 s after the pointer left");
    h.advance(0.5 + STEP);
    assert_eq!(shown(&h), None, "8 s after the pointer left");
}

/// Keyboard focus in the band holds the message too; it closes the full time
/// after the focus leaves.
#[test]
fn app_desktop_r37_focus_holds_the_message() {
    let (mut h, _b) = with(MessagePrefs::default());
    notify(&mut h, "Saved.", false);
    let id = h
        .get_by_role_and_label(Role::Button, "Dismiss")
        .item_id()
        .to_owned();
    assert!(id.starts_with("msg/"), "{id}");
    h.state_mut().act(&id, Ev::Focus, "");
    h.advance(6.0);
    assert!(shown(&h).is_some(), "focus holds it");
    h.state_mut().act(&id, Ev::Blur, "");
    h.advance(2.5);
    assert!(shown(&h).is_some(), "2.5 s after the focus left");
    h.advance(0.5 + STEP);
    assert_eq!(shown(&h), None);
}

/// Off: the message is shown after 60 s.
#[test]
fn app_desktop_r37_off_keeps_messages() {
    let (mut h, _b) = with(MessagePrefs {
        message_autoclose: false,
        message_seconds: 3,
    });
    notify(&mut h, "Saved.", false);
    h.advance(60.0);
    assert_eq!(shown(&h).as_deref(), Some("Saved."));
}

/// Settings → Operation offers the setting (on, 3 seconds by default) and the
/// six choices; changing it is saved per user.
#[test]
fn app_desktop_r37_operation_setting() {
    let (mut h, _b) = with(MessagePrefs::default());
    h.state_mut().open_view_spec("settings");
    h.run();
    settings_section(&mut h, "Operation");
    let check = h.get_by_role_and_label(Role::CheckBox, "Close status messages automatically");
    assert_eq!(check.toggled(), Some(Toggled::True));
    let after = h.get_by_role_and_label(Role::ComboBox, "After");
    assert_eq!(after.value().as_deref(), Some("3 seconds"));
    assert!(has_contains(
        &h,
        "Error messages stay until you press Dismiss"
    ));
    h.state_mut().take_events();
    h.get_by_role_and_label(Role::CheckBox, "Close status messages automatically")
        .click();
    h.run();
    assert!(!h.state().config.ui.message_autoclose);
    assert!(
        h.state_mut()
            .take_events()
            .iter()
            .any(|e| matches!(e, mk_ui::AppEvent::ConfigChanged))
    );
    assert!(
        h.get_by_role_and_label(Role::ComboBox, "After")
            .is_disabled(),
        "After waits for the switch"
    );
    // The setting is kept with the user's other settings.
    let json = serde_json::to_value(&h.state().config).unwrap();
    assert_eq!(json["ui"]["message_autoclose"], false);
    assert_eq!(json["ui"]["message_seconds"], 3);
    let old: UserConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(
        old.ui,
        MessagePrefs::default(),
        "older configs read as on, 3 s"
    );
    assert_eq!(mk_ui::MESSAGE_SECONDS, [2, 3, 5, 8, 10, 15]);
}
