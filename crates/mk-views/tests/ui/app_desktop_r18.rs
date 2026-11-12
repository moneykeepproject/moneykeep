//! app-desktop.md revision 18 acceptance tests (109–111): payment processors, against
//! the registry's scripted processor server (no network).
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_commands::Session;
use mk_commands::connect::{Connector, MemorySecrets};
use mk_commands::processors::FakeProcessors;
use mk_ui::UserConfig;
use mk_ui::ctl::App;
use mk_views::testing::Role;
use serde_json::{Value, json};

type H = mk_views::testing::Harness;

/// 2026-10-04 00:00 UTC.
const NOW: i64 = 1_791_072_000;
const DAY: i64 = 86_400;
const OCT3: i64 = NOW - DAY;

fn bt(id: &str, kind: &str, cat: &str, amount: i64, fee: i64, desc: &str) -> Value {
    json!({"id": id, "object": "balance_transaction", "amount": amount, "available_on": OCT3,
           "created": OCT3 - DAY, "currency": "usd", "description": desc, "fee": fee,
           "fee_details": [], "net": amount - fee, "reporting_category": cat,
           "source": format!("src_{id}"), "status": "available", "type": kind})
}

fn payout(fake: &FakeProcessors) {
    fake.with(|s| {
        let mut po = bt("txn_po1", "payout", "payout", -94_530, 0, "STRIPE PAYOUT");
        po["source"] = json!("po_1MiN3gLkdIwHu7ix");
        s.stripe_payouts = vec![json!({"id": "po_1MiN3gLkdIwHu7ix", "object": "payout",
            "amount": 94_530, "arrival_date": OCT3, "created": OCT3 - DAY, "currency": "usd",
            "failure_code": null, "method": "standard", "status": "paid", "type": "bank_account"})];
        s.stripe_entries.insert(
            "po_1MiN3gLkdIwHu7ix".into(),
            vec![
                bt("txn_c1", "charge", "charge", 40_000, 1_190, "Order 1001"),
                bt("txn_c2", "charge", "charge", 35_000, 1_045, "Order 1002"),
                bt("txn_c3", "charge", "charge", 25_000, 755, "Order 1003"),
                bt("txn_f1", "stripe_fee", "fee", -60, 0, "Radar screening"),
                bt(
                    "txn_r1",
                    "refund",
                    "refund",
                    -2_420,
                    0,
                    "Refund for Order 1001",
                ),
                po,
            ],
        );
    });
}

fn app_with(fake: &FakeProcessors, today: &str) -> (H, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shop.moneykeep");
    let mut s = Session::create(&path, "Test Books", "USD", "app").unwrap();
    s.set_connector(Connector {
        fetcher: std::sync::Arc::new(fake.clone()),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(NOW),
        host: None,
    });
    for (c, a) in [
        ("module.enable", json!({"module": "payment-processing"})),
        (
            "account.create",
            json!({"name": "Assets", "type": "asset", "placeholder": true}),
        ),
        (
            "account.create",
            json!({"name": "Checking", "type": "asset", "subtype": "bank", "parent": "Assets"}),
        ),
        (
            "account.create",
            json!({"name": "Expenses", "type": "expense", "placeholder": true}),
        ),
        (
            "account.create",
            json!({"name": "Card Fees", "type": "expense", "parent": "Expenses"}),
        ),
    ] {
        s.invoke(c, a).unwrap();
    }
    let app = App::new(
        Box::new(LocalBackend {
            session: Some(s),
            name: Some("Test Books".into()),
        }),
        UserConfig::default(),
    );
    let mut h = harness(app);
    h.state_mut().set_today(today);
    h.run();
    (h, dir)
}

fn open_connections(h: &mut H) {
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Connections").click();
    h.run();
}

fn choose(h: &mut H, combo: &str, option: &str) {
    click(h, combo);
    h.get_by_role_and_label(Role::Button, option).click();
    h.run();
}

fn wait_for(h: &mut H, part: &str) {
    for _ in 0..400 {
        h.run_steps(2);
        if h.query_all_by_label_contains(part).count() > 0 {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("{part:?} never showed: {:?}", labels_containing(h, ""));
}

fn add_stripe(h: &mut H) {
    open_connections(h);
    click(h, "Add connection…");
    choose(h, "Provider", "Stripe");
    assert!(has_contains(h, "restricted key with read access only"));
    type_into(h, "Restricted key", "rk_live_51TestRestrictedKey");
    type_into(h, "Connection name", "Shop");
    click(h, "Continue");
    assert!(
        has(
            h,
            "MoneyKeep will contact api.stripe.com to check this credential. Continue?"
        ),
        "{:?}",
        labels_containing(h, "contact")
    );
    click(h, "Connect");
}

#[test]
fn app_desktop_at109_stripe_payout_row_with_entries() {
    let fake = FakeProcessors::default();
    payout(&fake);
    let (mut h, _d) = app_with(&fake, "2026-10-04");
    add_stripe(&mut h);
    wait_for(&mut h, "Shop: 1 new transaction to review.");
    h.state_mut().open_view_spec("home");
    h.run();
    click(&mut h, "Stripe: 1 new row to review");
    assert!(
        has_contains(
            &h,
            "payout po_1MiN… · gross 1,000.00 · refunds -24.20 · fees -30.50 · other 0.00 · net 945.30"
        ),
        "{:?}",
        labels_containing(&h, "payout")
    );
    click(&mut h, "Entries (5)");
    assert!(has(&h, "Radar screening"));
    assert_eq!(h.query_all_by_label("charge").count(), 3);
    click(&mut h, "Add to books");
    let t = cmd(
        &mut h,
        "account.balance",
        json!({"account": "Stripe Clearing"}),
    );
    assert!(t.to_string().contains("945.30"), "{t}");
}

#[test]
fn app_desktop_at110_settings_change_the_fees_account() {
    let fake = FakeProcessors::default();
    let (mut h, _d) = app_with(&fake, "2026-10-04");
    add_stripe(&mut h);
    wait_for(&mut h, "Shop: ");
    click(&mut h, "Processor settings Shop");
    assert!(has_contains(&h, "Processor settings: Shop"));
    choose(&mut h, "Fees account", "Expenses:Card Fees");
    click(&mut h, "Save settings");
    let s = cmd(&mut h, "processor.settings", json!({"connection": "Shop"}));
    assert_eq!(
        s["accounts"]["fee"]["account"]["name"],
        "Expenses:Card Fees"
    );
    payout(&fake);
    let out = cmd(&mut h, "connect.sync", json!({"now": true}));
    let b = cmd(
        &mut h,
        "import.get",
        json!({"batch": out["connections"][0]["batch"]}),
    );
    let other = b["rows"][0]["other"].to_string();
    assert!(other.contains("Expenses:Card Fees"), "{other}");
    assert!(!other.contains("Merchant Fees"));
}

#[test]
fn app_desktop_at111_unmatched_clearing_on_home() {
    let fake = FakeProcessors::default();
    payout(&fake);
    let (mut h, _d) = app_with(&fake, "2026-10-04");
    cmd(
        &mut h,
        "connect.add",
        json!({"provider": "stripe", "setup_input": "rk_live_51TestRestrictedKey", "label": "Shop", "confirm": true}),
    );
    let out = cmd(&mut h, "connect.sync", json!({"now": true}));
    cmd(
        &mut h,
        "import.commit",
        json!({"batch": out["connections"][0]["batch"]}),
    );
    h.state_mut().open_view_spec("home");
    h.run();
    assert!(
        !has_contains(&h, "not matched to a deposit"),
        "within the window"
    );
    h.state_mut().set_today("2026-10-10");
    h.run();
    assert!(
        has(&h, "Stripe Clearing has 945.30 not matched to a deposit"),
        "{:?}",
        labels_containing(&h, "Clearing")
    );
    click(
        &mut h,
        "Stripe Clearing has 945.30 not matched to a deposit",
    );
    assert!(h.query_all_by_label_contains("Stripe payout").count() >= 1);
}

/// connectivity-processors.md revision 2 (R2.4, AT18) on screen: a PayPal clearing
/// balance with no recent withdrawal is marked "held at PayPal" on the Accounts list.
#[test]
fn held_at_paypal_note_on_the_accounts_list() {
    let fake = FakeProcessors::default();
    let (mut h, _d) = app_with(&fake, "2026-10-04");
    cmd(
        &mut h,
        "connect.add",
        json!({"provider": "paypal", "setup_input": "AbCdClientId:EfGhClientSecret", "label": "PayPal", "confirm": true}),
    );
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-09-01", "splits": [
            {"account": "PayPal Clearing", "value": "412.00"},
            {"account": "Assets:Checking", "value": "-412.00"}]}),
    );
    h.state_mut().open_view_spec("accounts");
    h.run();
    assert!(
        has(&h, "PayPal Clearing: held at PayPal"),
        "{:?}",
        labels_containing(&h, "PayPal")
    );
}
