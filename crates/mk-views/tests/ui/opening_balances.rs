//! opening-balances.md in the desktop window: Home → Attention opens the setup
//! checklist; the Opening balance wizard for a feed checking account (OB1, each
//! step); New account → Help me work it out…; the checklist's started from zero
//! (OB10). The bank feed is the registry's scripted SimpleFIN server.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::common::*;
use mk_commands::Session;
use mk_commands::connect::{Connector, FakeSimplefin, MemorySecrets};
use mk_ui::ctl::App;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

/// 2026-11-06 00:00 UTC.
const NOV6: i64 = 1_793_923_200;

const FEED: &str = r#"{
  "errlist": [],
  "connections": [{"conn_id": "c1", "name": "First Bank"}],
  "accounts": [
    {"id": "a1", "name": "Everyday", "conn_id": "c1", "currency": "USD",
     "balance": "2341.10", "balance-date": 1793923200,
     "transactions": [
       {"id": "t1", "posted": 1785801600, "amount": "2000.00", "description": "Payroll"},
       {"id": "t2", "posted": 1789084800, "amount": "-1035.35", "description": "Rent"},
       {"id": "t3", "posted": 1793836800, "amount": "-500.00", "description": "Savings transfer"}
     ]}
  ]
}"#;

/// A book with Checking linked to the scripted feed, its rows committed.
fn feed_app() -> (H, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("o.moneykeep"), "Test Books", "USD", "app").unwrap();
    s.set_connector(Connector {
        fetcher: Arc::new(FakeSimplefin::new(FEED)),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(NOV6),
        host: None,
    });
    for (c, a) in [
        ("module.enable", json!({"module": "bank-feeds"})),
        (
            "account.create",
            json!({"name": "Assets", "type": "asset", "placeholder": true}),
        ),
        (
            "account.create",
            json!({"name": "Checking", "type": "asset", "subtype": "bank", "parent": "Assets"}),
        ),
    ] {
        s.invoke(c, a).unwrap();
    }
    let app = App::new(
        Box::new(LocalBackend {
            session: Some(s),
            name: Some("Test Books".into()),
        }),
        iso_config(),
    );
    let mut h = harness(app);
    h.state_mut().set_today("2026-11-06");
    h.run();
    let c = cmd(
        &mut h,
        "connect.add",
        json!({"setup_input": FakeSimplefin::token("ob"), "label": "My bank", "confirm": true}),
    );
    cmd(
        &mut h,
        "connect.link_account",
        json!({"connection": c["id"], "remote_account": "a1", "account": "Assets:Checking",
               "sync_from": "2026-08-01"}),
    );
    let out = cmd(&mut h, "connect.sync", json!({"now": true}));
    cmd(
        &mut h,
        "import.commit",
        json!({"batch": out["connections"][0]["batch"]}),
    );
    h.run();
    (h, dir)
}

/// OB1 and OB10 in the window: Attention → checklist → each wizard step → done.
#[test]
fn ob1_wizard_from_home_attention() {
    let (mut h, _d) = feed_app();
    h.state_mut().open_view_spec("home");
    h.run_steps(3);
    assert!(
        has_contains(&h, "1 account needs an opening balance"),
        "{:?}",
        labels_containing(&h, "")
    );
    click(&mut h, "1 account needs an opening balance");
    h.run_steps(2);
    assert!(has(&h, "0 of 1 accounts set up"));
    assert!(has(&h, "Status of Assets:Checking: Not set"));
    h.get_by_role_and_label(Role::Button, "Set up Assets:Checking")
        .click();
    h.run_steps(2);
    // 1 Account.
    assert!(has_contains(&h, "Step 1: Account"));
    assert!(has_contains(&h, "Bank account"));
    click(&mut h, "Next");
    // 2 Balance: the feed's balance.
    assert!(
        has_contains(&h, "Your bank reports"),
        "{:?}",
        labels_containing(&h, "")
    );
    assert!(has(&h, "2,341.10"));
    assert!(has_contains(&h, "Pending transactions are not included"));
    click(&mut h, "Yes, use it");
    // 3 Start date.
    assert!(has_contains(
        &h,
        "All 3 imported transactions stay. Your books start on 2026-08-03."
    ));
    click(&mut h, "Next");
    // 4 Check.
    assert!(has(&h, "1,876.45"), "{:?}", labels_containing(&h, ""));
    assert!(has_contains(&h, "from Opening Balances."));
    assert!(has(&h, "Matches your bank. ✓"));
    click(&mut h, "Save");
    h.run_steps(2);
    assert!(
        has(&h, "Checking is set up. ✓"),
        "{:?}",
        labels_containing(&h, "")
    );
    assert!(has_contains(&h, "All accounts are set up."));
    click(&mut h, "Done");
    h.run_steps(2);
    // Back at the checklist: done, with the amount.
    assert!(
        has(&h, "1 of 1 accounts set up"),
        "{:?}",
        labels_containing(&h, "")
    );
    assert!(has_contains(
        &h,
        "Status of Assets:Checking: Done (1,876.45 on 2026-08-03)"
    ));
    click(&mut h, "Close");
    h.run_steps(2);
    assert!(!has_contains(&h, "needs an opening balance"));
}

/// The checklist's ⋯: This account started from zero takes it off the count.
#[test]
fn ob10_started_from_zero_in_the_checklist() {
    let (mut h, _book) = harness_with_chart();
    h.state_mut().set_today("2026-11-06");
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-10", "splits": [
            {"account": "Expenses:Groceries", "value": "5.00"},
            {"account": "Liabilities:Visa", "value": "-5.00"}]}),
    );
    h.state_mut().open_view_spec("accounts");
    h.run();
    h.get_by_label("More account actions").click();
    h.run();
    click(&mut h, "Opening balances…");
    h.run_steps(2);
    assert!(
        has(&h, "0 of 2 accounts set up"),
        "{:?}",
        labels_containing(&h, "of")
    );
    h.get_by_label("More for Assets:Checking").click();
    h.run();
    click(&mut h, "This account started from zero");
    h.run_steps(2);
    assert!(has(&h, "0 of 1 accounts set up"));
    assert!(has(&h, "Status of Assets:Checking: Started from zero"));
}

/// New account → Help me work it out… creates the account and opens the wizard.
#[test]
fn new_account_help_me_work_it_out() {
    let (mut h, _book) = harness_with_chart();
    h.state_mut().open_view_spec("accounts");
    h.run();
    click(&mut h, "New account…");
    type_into(&mut h, "Account name", "Wallet");
    click(&mut h, "Help me work it out…");
    h.run_steps(2);
    assert!(
        has(&h, "Opening balance"),
        "{:?}",
        labels_containing(&h, "")
    );
    assert!(has_contains(&h, "Step 1: Account"));
    click(&mut h, "Next");
    assert!(has(&h, "How much is in this account?"));
    type_into(&mut h, "Balance you trust", "40");
    click(&mut h, "Next");
    assert!(has(&h, NOTHING_TEXT));
    click(&mut h, "Next");
    assert!(has(&h, "40.00"), "{:?}", labels_containing(&h, ""));
    click(&mut h, "Save");
    h.run_steps(2);
    assert!(has(&h, "Wallet is set up. ✓"));
    let t = cmd(&mut h, "txn.list", json!({"account": "Wallet"}));
    assert_eq!(t[0]["opening"], true);
}

const NOTHING_TEXT: &str =
    "Nothing to work back from — your opening balance is simply your balance today.";
