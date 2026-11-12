//! app-desktop.md revision 11 acceptance tests (79–82): investment import review and
//! the investment profile editor.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use serde_json::json;

type H = mk_views::testing::Harness;

const JSON_FILE: &str = r#"{"data":{"transactions":[{"tradeDate":"10/01/2026","activity":"Bought","symbol":"VTI","shares":"10","price":"200","fees":"5","netAmount":"-2005.00","id":"T1"}]}}"#;
const JSON_FILE_2: &str = r#"{"data":{"transactions":[{"tradeDate":"10/05/2026","activity":"Bought","symbol":"VTI","shares":"2","price":"210","fees":"0","netAmount":"-420.00","id":"T9"}]}}"#;

fn book() -> (H, TestBook) {
    let (mut h, b) = harness_with_chart();
    cmd(&mut h, "module.enable", json!({"module": "investments"}));
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    h.state_mut().set_today("2026-10-20");
    h.run();
    (h, b)
}

fn txn_count(h: &mut H) -> usize {
    cmd(h, "txn.list", json!({"account": "Assets:Brokerage"}))
        .as_array()
        .map_or(0, Vec::len)
}

fn ofx_review(h: &mut H) {
    let bytes = fixture("ofx/investment-v2.ofx");
    h.state_mut().import_file("investment-v2.ofx", &bytes);
    h.run();
    click(h, "Account for section 1");
    click(h, "Assets:Brokerage");
}

#[test]
fn app_desktop_at79_ofx_investment_review() {
    let (mut h, _b) = book();
    ofx_review(&mut h);
    assert!(has(&h, "Securities in this file"));
    assert!(h.query_all_by_label("New security").count() >= 1);
    assert_eq!(value(&h, "Symbol for CUSIP:922908769"), "VTI");
    assert!(
        has_contains(&h, ": Not supported"),
        "{:?}",
        labels_containing(&h, "Investment row")
    );
    assert!(has_contains(&h, "Buy VTI"));
    assert!(has(&h, "Holdings check"));
    // Brokerage accounts only in the picker: Checking is not offered.
    click(&mut h, "Account for section 1");
    assert!(!has(&h, "Assets:Checking"));
}

#[test]
fn app_desktop_at80_commit_shows_activity_and_holdings_undo_removes() {
    let (mut h, _b) = book();
    ofx_review(&mut h);
    h.run();
    assert!(!disabled(&h, "Add to books"));
    click(&mut h, "Add to books");
    assert!(has_contains(&h, "Added to books"));
    assert!(txn_count(&mut h) > 0);
    open_register(&mut h, "Assets:Brokerage");
    assert!(
        has_contains(&h, "Holding VTI"),
        "{:?}",
        labels_containing(&h, "VTI")
    );
    click(&mut h, "Activity");
    assert!(
        has_contains(&h, "Buy VTI"),
        "{:?}",
        labels_containing(&h, "Activity")
    );
    h.get_by_role_and_label(mk_views::testing::Role::Button, "Undo")
        .click();
    h.run();
    assert_eq!(txn_count(&mut h), 0);
}

/// AT81's steps: the editor opens prefilled; map Bought to Buy, save, continue.
fn editor_flow(h: &mut H) {
    h.state_mut()
        .import_file("broker-activity.json", JSON_FILE.as_bytes());
    h.run();
    assert!(has(h, "Read broker-activity.json"));
    for (label, key) in [
        ("Field Date", "tradeDate"),
        ("Field Symbol", "symbol"),
        ("Field Shares", "shares"),
        ("Field Price", "price"),
        ("Field Fees", "fees"),
        ("Field Amount", "netAmount"),
    ] {
        assert_eq!(value(h, label), key, "{label}");
    }
    assert!(has(h, "Bought"));
    click(h, "Action for Bought");
    click(h, "Buy");
    // The live preview reads the row as a buy.
    assert!(has(h, "buy"), "{:?}", labels_containing(h, "VTI"));
    type_into(h, "Profile name", "My broker");
    click(h, "Save profile and continue");
    assert!(
        has(h, "Import review: broker-activity.json"),
        "{:?}",
        labels_containing(h, "Import review")
    );
}

#[test]
fn app_desktop_at81_profile_editor_then_review() {
    let (mut h, _b) = book();
    editor_flow(&mut h);
    click(&mut h, "Account for section 1");
    click(&mut h, "Assets:Brokerage");
    assert!(
        has_contains(&h, "Investment row 1: Buy VTI 2,005.00: New"),
        "{:?}",
        labels_containing(&h, "Investment row")
    );
}

#[test]
fn app_desktop_at82_second_file_uses_saved_profile() {
    let (mut h, _b) = book();
    editor_flow(&mut h);
    h.state_mut()
        .import_file("broker-activity-2.json", JSON_FILE_2.as_bytes());
    h.run();
    assert!(!has(&h, "Read broker-activity-2.json"));
    assert!(has(&h, "Import review: broker-activity-2.json"));
}

#[test]
fn app_desktop_r11_settings_lists_investment_profiles() {
    let (mut h, _b) = book();
    h.state_mut().open_view_spec("settings");
    h.run();
    click(&mut h, "Import");
    assert!(has(&h, "Investment profiles"));
    assert_eq!(h.query_all_by_label_contains("Charles Schwab").count(), 3);
}
