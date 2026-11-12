//! Phone capture on a Mac (macos-release.md Part C): the logged-in note on the
//! Status card (C2), when the firewall card shows (C5), and Settings → Phone
//! capture → Remove phone capture from this Mac… (C8). Which system the capture
//! server is on comes from the status light (`os`).

use serde_json::{Value, json};

use super::App;
use super::capture::{Kind, busy, start};
use super::ui::Ui;
use super::util::Json;

/// C2 (the same words as mk-commands' `capture_setup::text::MAC_LOGGED_IN`).
pub(crate) const LOGGED_IN: &str = "On a Mac, phone capture works while you're logged in. Expenses sent while you're logged out wait on your phone.";
/// C8: the button (mk-commands' `capture_setup::text::MAC_REMOVE`).
pub(crate) const REMOVE_LABEL: &str = "Remove phone capture from this Mac…";
const REMOVE_TITLE: &str = "Remove phone capture from this Mac";
const REMOVE_INTRO: &str = "This stops the capture server and removes its background agents from this Mac (and cloudflared's, if MoneyKeep set it up). No password is needed. Your books and backups are never touched. Do this before you delete MoneyKeep from Applications.";
pub(crate) const DELETE_DATA: &str = "Also delete the capture server's data: paired phones, the Wi-Fi certificate and expenses not yet collected";
const KEEP_NOTE: &str =
    "Kept, the data lets you install the server again later without pairing phones again.";
/// C8, on the Status card for Install the server again (no permission prompt).
pub(crate) const REINSTALL_TEXT: &str = "The capture server's data is still on this Mac: paired phones, the Wi-Fi certificate and expenses not yet collected. Installing the server again keeps them. If you moved MoneyKeep to another folder, this points the server at its new place.";
/// C5: a phone on Wi-Fi seen this recently shows the firewall lets phones in.
const RECENT_SECS: i64 = 24 * 3600;

/// The Remove dialog.
#[derive(Debug, Default)]
pub(crate) struct MacState {
    pub(crate) open: bool,
    /// Delete the data folder too (default: keep).
    pub(crate) delete_data: bool,
    pub(crate) error: Option<String>,
}

/// The capture server is on a Mac (from the last status check).
pub(crate) fn is_mac(app: &App) -> bool {
    app.capture
        .light
        .as_ref()
        .is_some_and(|l| l.str("os") == "macos")
}

/// C2: the note on the Status card.
pub(crate) fn status_note(app: &App, ui: &mut Ui) {
    if is_mac(app) {
        ui.secondary(LOGGED_IN);
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// C5: a phone paired over Wi-Fi reached the server in the last day, at `now`.
pub(crate) fn wifi_phone_seen(devices: Option<&Vec<Value>>, now: i64) -> bool {
    devices.is_some_and(|list| {
        list.iter().filter(|d| d.str("via") == "wifi").any(|d| {
            d.at("last_seen")
                .as_i64()
                .is_some_and(|t| now.saturating_sub(t) < RECENT_SECS)
        })
    })
}

/// C5: show the firewall problem `p`: every other problem always; macOS's
/// firewall only while no phone on Wi-Fi has got through lately.
pub(crate) fn show_problem(app: &App, p: &Value) -> bool {
    p.str("code") != "macos_firewall"
        || !wifi_phone_seen(app.capture.devices_list().as_ref(), now_secs())
}

/// C8: the card in Settings → Phone capture (Macs only).
pub(crate) fn card(app: &mut App, ui: &mut Ui) {
    if !is_mac(app) {
        return;
    }
    ui.card("Remove from this Mac", |ui| {
        ui.secondary("Before deleting MoneyKeep from Applications, remove phone capture from this Mac, so nothing is left running in the background.");
        ui.row(|ui| {
            if ui.button(REMOVE_LABEL).clicked() {
                app.capture.mac = MacState {
                    open: true,
                    ..MacState::default()
                };
            }
        });
    });
}

/// C8: the dialog.
pub(crate) fn dialog(app: &mut App, ui: &mut Ui) {
    if !app.capture.mac.open {
        return;
    }
    let working = busy(app, Kind::RemoveFromMac);
    let mut close = ui.dialog_closed("capture-remove-mac") || ui.key("escape");
    let mut go = false;
    let error = app.capture.mac.error.clone();
    ui.dialog("capture-remove-mac", REMOVE_TITLE, 560, |ui| {
        ui.label(REMOVE_INTRO);
        ui.enabled(!working);
        ui.checkbox(&mut app.capture.mac.delete_data, DELETE_DATA);
        if !app.capture.mac.delete_data {
            ui.secondary(KEEP_NOTE);
        }
        if let Some(e) = &error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if working {
                ui.progress("Removing", -1);
                ui.label("Removing…");
            }
            close |= ui.button("Cancel").clicked();
            ui.enabled(!working);
            go = ui.primary("Remove").clicked();
        });
    });
    if go {
        app.capture.mac.error = None;
        let delete_data = app.capture.mac.delete_data;
        start(
            app,
            "capture.remove_from_computer",
            json!({ "delete_data": delete_data }),
            Kind::RemoveFromMac,
        );
    } else if close && !working {
        app.capture.mac = MacState::default();
    }
}

/// C8 ended; `true` when phone capture was removed (the status is checked again).
pub(crate) fn finished(app: &mut App, out: Result<Value, crate::UiError>) -> bool {
    match out {
        Ok(v) => {
            app.capture.mac = MacState::default();
            let text = match v.at("text").as_str() {
                Some(t) if !t.is_empty() => t.to_owned(),
                _ => "Phone capture was removed from this Mac.".to_owned(),
            };
            app.notify(text);
            app.generation += 1;
            true
        }
        Err(e) => {
            app.capture.mac.error = Some(e.display());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn c5_recent_wifi_phone() {
        let now = 1_800_000_000;
        assert!(!wifi_phone_seen(None, now));
        let list = vec![
            json!({"via": "cloudflare", "last_seen": now - 60}),
            json!({"via": "wifi", "last_seen": now - 2 * RECENT_SECS}),
        ];
        assert!(!wifi_phone_seen(Some(&list), now));
        let mut list = list;
        list.push(json!({"via": "wifi", "last_seen": now - 3600}));
        assert!(wifi_phone_seen(Some(&list), now));
    }

    #[test]
    fn texts_match_the_commands() {
        use mk_commands::capture_setup::text;
        assert_eq!(LOGGED_IN, text::MAC_LOGGED_IN);
        assert_eq!(REMOVE_LABEL, text::MAC_REMOVE);
    }
}
