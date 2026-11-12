//! app-desktop.md revision 36 (mileage.md): the Mileage screen with its Trips and
//! Vehicles tabs, the trip and vehicle dialogs (round trip, odometer, duplicate
//! check), the Mileage report dialog, Settings → Mileage, reimbursable trips in the
//! Expenses screen's Record reimbursement… and the Tax summary's Mileage section.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::{AppEvent, View};
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

fn book() -> (H, TestBook) {
    let (mut h, b) = harness_with_chart();
    h.state_mut().set_today("2026-03-05");
    cmd(&mut h, "module.enable", json!({"module": "mileage"}));
    cmd(&mut h, "module.enable", json!({"module": "expenses"}));
    h.run();
    (h, b)
}

fn type_field(h: &mut H, label: &str, text: &str) {
    h.get_by_role_and_label(Role::TextInput, label).focus();
    h.run();
    h.key_press_modifiers(
        mk_views::testing::Modifiers::COMMAND,
        mk_views::testing::Key::A,
    );
    h.get_by_role_and_label(Role::TextInput, label)
        .type_text(text);
    h.run();
}

fn trips(h: &mut H) -> Vec<serde_json::Value> {
    cmd(h, "trip.list", json!({})).as_array().unwrap().clone()
}

#[test]
fn app_desktop_r36_mileage_screen_vehicle_and_round_trip() {
    let (mut h, _b) = book();
    let items = feature_items(&mut h);
    assert!(items.contains(&"Mileage".to_owned()), "{items:?}");
    open_feature(&mut h, "Mileage");
    assert_eq!(h.state().view(), &View::Mileage);
    h.get_by_role_and_label(Role::Tab, "Trips");
    h.get_by_role_and_label(Role::Tab, "Vehicles");
    assert!(has(
        &h,
        "Add a vehicle first: trips are logged per vehicle."
    ));
    // The first trip offers Add vehicle inline.
    click(&mut h, "+ New trip");
    assert!(has(&h, "New trip"));
    type_field(&mut h, "New vehicle name", "2019 Honda CR-V");
    click(&mut h, "Add");
    assert_eq!(value(&h, "Vehicle"), "2019 Honda CR-V");
    type_field(&mut h, "Trip date", "2026-03-02");
    type_field(&mut h, "Purpose", "Client visit");
    type_field(&mut h, "From", "Office");
    type_field(&mut h, "To", "Client");
    type_field(&mut h, "Miles", "12.4");
    click(&mut h, "Round trip");
    assert!(has(&h, "One way 12.4 mi → 24.8 mi"));
    assert!(
        has(&h, "24.8 mi × 72.5¢ = 17.98"),
        "{:?}",
        labels_containing(&h, "mi ×")
    );
    click(&mut h, "Save");
    assert!(!has(&h, "New trip"), "the dialog closed");
    let list = trips(&mut h);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["miles"], "24.8");
    assert_eq!(list[0]["amount"], "17.98");
    assert!(has(&h, "Open trip 2026-03-02 Office → Client → Office"));
    assert!(has(&h, "Log only"));
    // The same date, vehicle and miles asks first.
    click(&mut h, "+ New trip");
    type_field(&mut h, "Trip date", "2026-03-02");
    type_field(&mut h, "Purpose", "Second visit");
    type_field(&mut h, "To", "Client");
    type_field(&mut h, "Miles", "24.8");
    click(&mut h, "Save");
    assert!(has(
        &h,
        "A trip with these details already exists. Save anyway?"
    ));
    assert_eq!(trips(&mut h).len(), 1);
    click(&mut h, "Save anyway");
    assert_eq!(trips(&mut h).len(), 2);
    // Odometer readings: miles computed, Round trip off and disabled.
    click(&mut h, "+ New trip");
    type_field(&mut h, "Purpose", "Supplies");
    type_field(&mut h, "To", "Store");
    click(&mut h, "Enter odometer readings");
    type_field(&mut h, "Odometer start", "45210.3");
    type_field(&mut h, "Odometer end", "45236.0");
    assert!(disabled(&h, "Round trip"));
    assert!(has(&h, "25.7 mi × 72.5¢ = 18.63"));
    click(&mut h, "Save");
    assert!(trips(&mut h).iter().any(|t| t["miles"] == "25.7"));
    // Vehicles tab: odometer for the year.
    h.get_by_role_and_label(Role::Tab, "Vehicles").click();
    h.run();
    click(&mut h, "2019 Honda CR-V");
    assert!(has(&h, "Edit vehicle"));
    type_field(&mut h, "Odometer at start of year", "30000");
    type_field(&mut h, "Odometer at end of year", "42000");
    click(&mut h, "Save");
    let v = cmd(&mut h, "vehicle.list", json!({}));
    assert_eq!(v[0]["odometer"]["2026"]["miles"], "12000.0");
    assert!(has(&h, "12,000.0"));
}

#[test]
fn app_desktop_r36_report_dialog_and_settings() {
    let (mut h, _b) = book();
    cmd(&mut h, "vehicle.create", json!({"name": "Van"}));
    cmd(
        &mut h,
        "trip.create",
        json!({"date": "2026-02-01", "miles": "10", "purpose": "Delivery", "to_place": "Pantry",
               "purpose_kind": "charity"}),
    );
    open_feature(&mut h, "Mileage");
    click(&mut h, "Mileage report…");
    assert!(has(&h, "Mileage report"));
    assert_eq!(value(&h, "Report from"), "2026-01-01");
    assert_eq!(value(&h, "Report to"), "2026-12-31");
    let _ = events(&mut h);
    click(&mut h, "Save report…");
    let ev = events(&mut h);
    assert!(
        ev.iter().any(|e| matches!(e, AppEvent::SaveBytes { file_name, data }
            if file_name == "mileage-report-2026-01-01-to-2026-12-31.pdf" && data.starts_with(b"%PDF"))),
        "{ev:?}"
    );
    // CSV.
    click(&mut h, "Mileage report…");
    click(&mut h, "CSV (spreadsheet)");
    click(&mut h, "Save report…");
    let ev = events(&mut h);
    assert!(
        ev.iter()
            .any(|e| matches!(e, AppEvent::SaveFile { file_name, content }
            if file_name.ends_with(".csv") && content.starts_with("Section,"))),
        "{ev:?}"
    );
    // Settings → Mileage: defaults, the rate table and saved trips.
    h.state_mut().navigate(View::Settings);
    h.run();
    settings_section(&mut h, "Mileage");
    for l in [
        "Default purpose",
        "Standard rates",
        "Edit rate Business from 2026-07-01",
        "Re-apply rates…",
        "+ Add rate",
        "+ New saved trip",
    ] {
        assert!(has(&h, l), "{l}");
    }
    assert!(has(&h, "76.0¢"));
    // Add a rate, then re-apply: the preview counts the trip, Apply changes it.
    click(&mut h, "+ Add rate");
    h.get_by_role_and_label(Role::ComboBox, "Rate purpose kind")
        .click();
    h.run();
    h.get_by_role_and_label(Role::Button, "Charity").click();
    h.run();
    type_field(&mut h, "Starts on", "2026-02-01");
    type_field(&mut h, "Cents per mile", "15");
    click(&mut h, "Save");
    assert!(has(&h, "15.0¢"));
    click(&mut h, "Re-apply rates…");
    click(&mut h, "Preview");
    assert!(
        has(&h, "1 trip change; total difference 0.10."),
        "{:?}",
        labels_containing(&h, "change")
    );
    click(&mut h, "Apply");
    assert_eq!(trips(&mut h)[0]["rate_cents"], "15.0");
}

#[test]
fn app_desktop_r36_reimbursable_trip_in_record_reimbursement() {
    let (mut h, _b) = book();
    cmd(&mut h, "vehicle.create", json!({"name": "Van"}));
    let t = cmd(
        &mut h,
        "trip.create",
        json!({"date": "2026-02-01", "miles": "24.8", "purpose": "Client", "to_place": "Client",
               "reimbursable": true, "driver": "Pat"}),
    );
    cmd(&mut h, "trip.post", json!({"trip": t["id"]}));
    open_feature(&mut h, "Expenses");
    assert!(!disabled(&h, "Record reimbursement…"));
    click(&mut h, "Record reimbursement…");
    assert!(has_contains(&h, "Mileage — Van"));
    assert!(has(&h, "Total 17.98"));
    h.get_by_role_and_label(Role::ComboBox, "Reimburse from")
        .click();
    h.run();
    h.get_by_role_and_label(Role::Button, "Assets:Checking")
        .click();
    h.run();
    click(&mut h, "Record");
    let t = cmd(&mut h, "trip.get", json!({"trip": t["id"]}));
    assert!(!t["reimbursement"].is_null());
    // The Tax summary (last year by default) shows the Mileage section.
    h.state_mut().set_today("2027-01-10");
    h.state_mut().navigate(View::Reports);
    h.run();
    click(&mut h, "Tax summary");
    assert!(
        has_contains(&h, "Mileage (standard rates)"),
        "{:?}",
        labels_containing(&h, "Mileage")
    );
    assert!(has(
        &h,
        "Information for your tax preparer; not entered in the books."
    ));
}
