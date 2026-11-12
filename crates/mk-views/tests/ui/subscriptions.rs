//! subscriptions.md on the desktop: the Features → Subscriptions screen (tabs,
//! answering a suggestion, coloured change chips), the detail view with a reason
//! note, Settings → Subscriptions with its colour bands, Review mode, and Home →
//! Attention with the "charged after cancelling" item first.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::View;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

fn pay(h: &mut H, date: &str, payee: &str, amount: &str) {
    cmd(
        h,
        "txn.create",
        json!({"date": date, "payee": payee, "splits": [
            {"account": "Expenses:Groceries", "value": amount},
            {"account": "Assets:Checking", "value": format!("-{amount}")}]}),
    );
}

fn book() -> H {
    let (mut h, _b) = harness_with_chart();
    h.state_mut().set_today("2026-11-05");
    for m in 4..=10 {
        let p = if m < 8 { "9.99" } else { "12.99" };
        pay(&mut h, &format!("2026-{m:02}-08"), "Spotify", p);
    }
    for m in 3..=9 {
        pay(&mut h, &format!("2026-{m:02}-20"), "Hulu", "7.99");
    }
    h.run();
    h
}

#[test]
fn subscriptions_screen_off_by_default_then_suggest_and_chip() {
    let mut h = book();
    assert!(!feature_items(&mut h).contains(&"Subscriptions".to_owned()));
    cmd(&mut h, "module.enable", json!({"module": "subscriptions"}));
    h.run();
    assert!(feature_items(&mut h).contains(&"Subscriptions".to_owned()));
    open_feature(&mut h, "Subscriptions");
    assert_eq!(h.state().view(), &View::Subscriptions);
    click(&mut h, "Suggested (2)");
    assert!(has(&h, "It's a subscription: Spotify"));
    click(&mut h, "It's a subscription: Spotify");
    assert!(has_contains(&h, "Spotify is now under Subscriptions."));
    click(&mut h, "Not recurring: Hulu");
    h.get_by_role_and_label(Role::Tab, "Subscriptions").click();
    h.run();
    // +30 % reaches the yellow band: a chip with the arrow and the percentage.
    // In the list and in Biggest increases.
    assert_eq!(labels_containing(&h, "Price up +30.0%").len(), 2);
    assert!(has(&h, "Open Spotify"));
    // The detail: price history with a reason note.
    click(&mut h, "Open Spotify");
    assert!(has(&h, "Price history"));
    let reason = labels_containing(&h, "Reason for");
    assert_eq!(reason.len(), 1, "{reason:?}");
    type_into(&mut h, &reason[0], "Family plan");
    key(&mut h, mk_views::testing::Key::Enter);
    let list = cmd(&mut h, "subscription.list", json!({"today": "2026-11-05"}));
    let id = list["subscriptions"][0]["id"].as_str().unwrap().to_owned();
    let g = cmd(&mut h, "subscription.get", json!({"subscription": id}));
    assert_eq!(g["history"][0]["reason"], "Family plan");
    click(&mut h, "← All subscriptions");
    assert!(has(&h, "Biggest increases (last 12 months)"));
    // The Not recurring answer is listed under Dismissed suggestions.
    assert_eq!(
        cmd(&mut h, "subscription.dismissed", json!({}))
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn subscriptions_settings_review_and_home() {
    let mut h = book();
    cmd(&mut h, "module.enable", json!({"module": "subscriptions"}));
    for s in cmd(&mut h, "subscription.suggestions", json!({}))
        .as_array()
        .unwrap()
        .clone()
    {
        cmd(
            &mut h,
            "subscription.answer",
            json!({"key": s["key"], "answer": "subscription"}),
        );
    }
    let list = cmd(&mut h, "subscription.list", json!({"today": "2026-11-05"}));
    let hulu = list["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Hulu")
        .unwrap()["id"]
        .clone();
    cmd(
        &mut h,
        "subscription.set_status",
        json!({"subscription": hulu, "status": "cancelled", "on": "2026-10-20"}),
    );
    pay(&mut h, "2026-11-03", "Hulu", "7.99");
    // Home → Attention: the charge after cancelling first, with its red chip.
    h.state_mut().navigate(View::Home);
    h.run();
    assert!(has(&h, "Charged after cancelling"));
    assert!(has(
        &h,
        "Hulu charged 7.99 on Nov 3 after you cancelled it on Oct 20."
    ));
    // Settings → Subscriptions: the alert fields and the bands.
    h.state_mut().navigate(View::Settings);
    h.run();
    settings_section(&mut h, "Subscriptions");
    assert!(has(&h, "Band 1 preview"));
    assert!(has(&h, "Band 2 preview"));
    assert_eq!(value(&h, "Band 1 colour (hex)"), "#F2C94C");
    click(&mut h, "Remove band 2");
    let s = cmd(
        &mut h,
        "module.settings",
        json!({"module": "subscriptions"}),
    );
    let bands = s["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == "subscriptions.bands")
        .unwrap()["value"]
        .clone();
    assert_eq!(bands.as_array().unwrap().len(), 1);
    // Review: cancel the one active subscription.
    open_feature(&mut h, "Subscriptions");
    click(&mut h, "Review…");
    assert!(has(&h, "Review subscriptions"));
    click(&mut h, "Cancel subscription");
    assert!(
        has(&h, "You've saved 155.88 a year"),
        "{:?}",
        labels_containing(&h, "saved")
    );
    assert!(has(&h, "Review finished"));
}
