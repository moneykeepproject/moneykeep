//! app-desktop.md revision 35 (ledger-accounts.md revision 3): Account type in the
//! Edit account dialog; connectivity.md revision 4 acceptance tests 18–19: the Kind
//! of a new account in the link step, and Review from Settings → Connections, the
//! register band and the sync notice. The provider is the registry's scripted
//! SimpleFIN server; nothing touches the network.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_commands::Session;
use mk_commands::connect::{Connector, FakeSimplefin, MemorySecrets};
use mk_ui::UserConfig;
use mk_ui::ctl::App;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

#[test]
fn app_desktop_r35_bank_account_becomes_a_credit_card() {
    let (mut h, _book) = harness_with_chart();
    for (date, payee, amount) in [
        ("2026-07-01", "Corner Grocery", "45.10"),
        ("2026-07-02", "Bakery", "12.00"),
    ] {
        cmd(
            &mut h,
            "txn.create",
            json!({"date": date, "payee": payee, "splits": [
                {"account": "Expenses:Groceries", "value": amount},
                {"account": "Assets:Checking", "value": format!("-{amount}")}]}),
        );
    }
    h.state_mut().open_view_spec("accounts");
    h.run();
    click(&mut h, "Actions for Assets:Checking");
    click(&mut h, "Edit…");
    assert_eq!(value(&h, "Account type"), "Bank");
    let group_combo = |h: &H| {
        h.query_all_by_label("Group")
            .any(|n| n.accesskit_node().role() == Role::ComboBox)
    };
    assert!(!group_combo(&h), "no Group while the side stays");
    click(&mut h, "Account type");
    h.get_by_role_and_label(Role::Button, "Credit card").click();
    h.run();
    // Liabilities holds Visa (a credit card), so it is preselected.
    assert!(group_combo(&h));
    let group = h
        .get_by_role_and_label(Role::ComboBox, "Group")
        .value()
        .unwrap_or_default();
    assert_eq!(group, "Liabilities");
    assert!(has(
        &h,
        "Balances and reports will show this account as a liability. Its transactions do not change."
    ));
    click(&mut h, "Save");
    assert!(!has(&h, "Edit account"), "the dialog closed");
    let acc = cmd(
        &mut h,
        "account.get",
        json!({"account": "Liabilities:Checking"}),
    );
    assert_eq!(acc["type"], "liability");
    assert_eq!(acc["subtype"], "credit-card");
    h.run();
    assert!(has(&h, "Balance of Liabilities:Checking: 57.10"));
    // The register shows the same transactions, the balance as owed.
    open_register(&mut h, "Liabilities:Checking");
    assert!(has_contains(
        &h,
        "Corner Grocery Expenses:Groceries, Charge 45.10"
    ));
    assert!(has_contains(&h, "Bakery Expenses:Groceries, Charge 12.00"));
    assert!(
        has(&h, "Closing balance 57.10"),
        "{:?}",
        labels_containing(&h, "Closing")
    );
    // Other accounts show their type as text.
    h.state_mut().open_view_spec("accounts");
    h.run();
    click(&mut h, "Actions for Expenses:Groceries");
    click(&mut h, "Edit…");
    assert!(has(&h, "Expense"));
    assert!(
        h.query_all_by_label("Account type")
            .all(|n| n.accesskit_node().role() != Role::ComboBox)
    );
}

const SET: &str = r#"{
  "errlist": [],
  "connections": [{"conn_id": "c1", "name": "First Bank"}],
  "accounts": [
    {"id": "a1", "name": "Everyday", "conn_id": "c1", "currency": "USD",
     "balance": "1200.50", "balance-date": 1790812800,
     "transactions": [
       {"id": "t1", "posted": 1790553600, "amount": "-45.10", "description": "Corner Grocery"},
       {"id": "t2", "posted": 1790640000, "amount": "2000.00", "description": "Payroll"},
       {"id": "t3", "posted": 1790726400, "amount": "-12.00", "description": "Bakery"}
     ]},
    {"id": "a2", "name": "Chase Sapphire Visa", "conn_id": "c1", "currency": "USD",
     "balance": "-300.00", "balance-date": 1790812800, "transactions": []}
  ]
}"#;

fn feeds_app(fake: &FakeSimplefin, secrets: &MemorySecrets) -> (H, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("feeds.moneykeep");
    let mut s = Session::create(&path, "Test Books", "USD", "app").unwrap();
    s.set_connector(Connector {
        fetcher: std::sync::Arc::new(fake.clone()),
        secrets: Box::new(secrets.clone()),
        utc_offset_secs: 0,
        now: Some(1_790_812_800),
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
        UserConfig::default(),
    );
    let mut h = harness(app);
    h.state_mut().set_today("2026-10-01");
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

/// Add the connection and reach the link step.
fn to_link_step(h: &mut H, token: &str) {
    open_connections(h);
    click(h, "Add connection…");
    type_into(h, "Setup token", &FakeSimplefin::token(token));
    type_into(h, "Connection name", "My bank");
    click(h, "Continue");
    click(h, "Claim");
    assert!(has(h, "Link Everyday") && has(h, "Link Chase Sapphire Visa"));
}

#[test]
fn connectivity_rev4_at18_new_card_defaults_to_credit_card() {
    let fake = FakeSimplefin::new(SET);
    let secrets = MemorySecrets::new(true);
    let (mut h, _d) = feeds_app(&fake, &secrets);
    to_link_step(&mut h, "k18");
    assert!(!has(&h, "Kind of Chase Sapphire Visa"));
    choose(&mut h, "Link Chase Sapphire Visa", "Create a new account");
    assert_eq!(value(&h, "Kind of Chase Sapphire Visa"), "Credit card");
    // Investment is not offered: SimpleFIN reports no holdings.
    click(&mut h, "Kind of Chase Sapphire Visa");
    assert!(
        h.query_all_by_label("Investment")
            .all(|n| n.accesskit_node().role() != Role::Button)
    );
    h.get_by_role_and_label(Role::Button, "Credit card").click();
    h.run();
    choose(&mut h, "Link Everyday", "Create a new account");
    assert_eq!(value(&h, "Kind of Everyday"), "Bank");
    click(&mut h, "Done");
    let card = cmd(
        &mut h,
        "account.get",
        json!({"account": "Chase Sapphire Visa"}),
    );
    assert_eq!(card["type"], "liability");
    assert_eq!(card["subtype"], "credit-card");
    let bank = cmd(&mut h, "account.get", json!({"account": "Everyday"}));
    assert_eq!(bank["type"], "asset");
    assert_eq!(bank["subtype"], "bank");
}

#[test]
fn connectivity_rev4_at19_review_from_settings_register_and_notice() {
    let fake = FakeSimplefin::new(SET);
    let secrets = MemorySecrets::new(true);
    let (mut h, _d) = feeds_app(&fake, &secrets);
    to_link_step(&mut h, "k19");
    choose(&mut h, "Link Everyday", "Assets:Checking");
    click(&mut h, "Done");
    // opening-balances.md: the offer to set opening balances, answered Later.
    click(&mut h, "Later");
    wait_for(&mut h, "My bank: 3 new transactions to review.");
    // The notice offers Review, which opens the batch.
    h.get_by_role_and_label(Role::Link, "Review").click();
    h.run();
    assert!(has_contains(&h, "SimpleFIN — My bank"));
    // Settings → Connections.
    open_connections(&mut h);
    assert!(has(&h, "3 new transactions to review"));
    click(&mut h, "Review My bank");
    assert!(has_contains(&h, "SimpleFIN — My bank"));
    // The register of the linked account.
    open_register(&mut h, "Assets:Checking");
    assert!(
        has(&h, "3 transactions from SimpleFIN wait for review"),
        "{:?}",
        labels_containing(&h, "review")
    );
    // The same when the register is opened by the account's full name.
    h.state_mut().open_view_spec("register:Assets:Checking");
    h.run();
    assert!(has(&h, "3 transactions from SimpleFIN wait for review"));
    click(&mut h, "Review staged transactions");
    assert!(has_contains(&h, "SimpleFIN — My bank"));
    // Committing the batch removes the band.
    let att = cmd(&mut h, "connect.attention", json!({}));
    let batch = att[0]["batch"].clone();
    cmd(&mut h, "import.commit", json!({"batch": batch}));
    open_register(&mut h, "Assets:Checking");
    assert!(!has_contains(&h, "wait for review"));
}
