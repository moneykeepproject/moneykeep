//! app-desktop.md revision 17 acceptance tests (105–107): bank feeds. The provider is
//! the registry's scripted SimpleFIN server; nothing touches the network.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_commands::Session;
use mk_commands::connect::{Connector, FakeSimplefin, MemorySecrets};
use mk_ui::UserConfig;
use mk_ui::ctl::App;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

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
    {"id": "a2", "name": "Card", "conn_id": "c1", "currency": "USD",
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

/// Click while a spinner may be showing (the window keeps repainting).
fn click_s(h: &mut H, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(4);
}

/// Run frames until a label containing `part` shows (background syncs finish).
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

fn all_text(h: &H) -> String {
    let mut out = String::new();
    for n in h.query_all_by_label_contains("") {
        let node = n.accesskit_node();
        out.push_str(&format!("{:?} {:?}\n", node.label(), node.value()));
    }
    out
}

#[test]
fn app_desktop_at105_add_link_sync_attention() {
    let fake = FakeSimplefin::new(SET);
    let secrets = MemorySecrets::new(true);
    let (mut h, _d) = feeds_app(&fake, &secrets);
    open_connections(&mut h);
    click(&mut h, "Add connection…");
    assert!(has_contains(&h, "never sees your bank password"));
    let token = FakeSimplefin::token("ui105");
    type_into(&mut h, "Setup token", &token);
    type_into(&mut h, "Connection name", "My bank");
    click(&mut h, "Continue");
    assert!(
        has(
            &h,
            "MoneyKeep will contact bridge.test to claim this token. Continue?"
        ),
        "{:?}",
        labels_containing(&h, "contact")
    );
    assert!(fake.requests().is_empty(), "nothing sent before confirming");
    click(&mut h, "Claim");
    assert!(has(&h, "Link Everyday") && has(&h, "Link Card"));
    choose(&mut h, "Link Everyday", "Assets:Checking");
    click(&mut h, "Done");
    // opening-balances.md: the last step offers to set opening balances.
    assert!(has(&h, "Set opening balances now (recommended)"));
    click(&mut h, "Later");
    assert!(!has(&h, "Add connection"), "the dialog closed");
    wait_for(&mut h, "My bank: 3 new transactions to review.");
    h.state_mut().open_view_spec("home");
    h.run();
    assert!(
        has(&h, "SimpleFIN: 3 new transactions to review"),
        "{:?}",
        labels_containing(&h, "SimpleFIN")
    );
    click(&mut h, "SimpleFIN: 3 new transactions to review");
    assert!(
        has_contains(&h, "SimpleFIN — My bank"),
        "{:?}",
        labels_containing(&h, "review")
    );
    // The Accounts list marks the linked account; its register can sync.
    h.state_mut().open_view_spec("accounts");
    h.run();
    assert!(has(&h, "Feed for Assets:Checking"));
    h.state_mut().open_view_spec("register:Assets:Checking");
    h.run();
    assert!(has(&h, "Sync now"));
    // The settings list shows the connection.
    open_connections(&mut h);
    assert!(has(&h, "My bank") && has(&h, "Active"));
    assert!(has(&h, "SimpleFIN · bridge.test"));
}

#[test]
fn app_desktop_at106_token_masked_and_forgotten() {
    let fake = FakeSimplefin::new(SET);
    let secrets = MemorySecrets::new(true);
    let (mut h, _d) = feeds_app(&fake, &secrets);
    open_connections(&mut h);
    click(&mut h, "Add connection…");
    let token = FakeSimplefin::token("secret106");
    type_into(&mut h, "Setup token", &token);
    let shown = value(&h, "Setup token");
    assert!(!shown.contains(&token[..12]), "masked: {shown:?}");
    assert!(!all_text(&h).contains(&token[..12]));
    click(&mut h, "Continue");
    click(&mut h, "Claim");
    click(&mut h, "Done");
    assert!(!all_text(&h).contains(&token[..12]), "the token is gone");
    // Nor do the access credentials appear anywhere.
    let text = all_text(&h);
    assert!(!text.contains("s3cret") && !text.contains("demo:"));
}

#[test]
fn app_desktop_at107_revoked_chip_and_message() {
    let fake = FakeSimplefin::new(SET);
    let secrets = MemorySecrets::new(true);
    let (mut h, _d) = feeds_app(&fake, &secrets);
    let c = cmd(
        &mut h,
        "connect.add",
        json!({"setup_input": FakeSimplefin::token("r107"), "label": "My bank", "confirm": true}),
    );
    cmd(
        &mut h,
        "connect.link_account",
        json!({"connection": c["id"], "remote_account": "a1", "account": "Assets:Checking", "sync_from": "2026-09-01"}),
    );
    fake.set_status(Some(403));
    open_connections(&mut h);
    click(&mut h, "Sync now My bank");
    wait_for(&mut h, "Revoked");
    assert!(
        h.query_all_by_label_contains("reconnect with a new setup token")
            .count()
            >= 1,
        "{:?}",
        labels_containing(&h, "reconnect")
    );
    assert!(disabled(&h, "Sync now My bank"));
    h.state_mut().open_view_spec("home");
    h.run();
    assert!(
        h.query_all_by_label_contains("My bank: Access was revoked")
            .count()
            >= 1
    );
}

#[test]
fn app_desktop_r17_no_secure_storage_and_disconnect() {
    let fake = FakeSimplefin::new(SET);
    let (mut h, _d) = feeds_app(&fake, &MemorySecrets::new(false));
    open_connections(&mut h);
    click(&mut h, "Add connection…");
    assert!(has_contains(&h, "can't be saved securely"));
    click(&mut h, "Open Settings, Security");
    assert_eq!(h.state().config.settings_section, "Security");
    // Disconnect explains revoking at the provider.
    let secrets = MemorySecrets::new(true);
    let (mut h, _d) = feeds_app(&fake, &secrets);
    cmd(
        &mut h,
        "connect.add",
        json!({"setup_input": FakeSimplefin::token("d17"), "label": "My bank", "confirm": true}),
    );
    open_connections(&mut h);
    click(&mut h, "Disconnect My bank");
    assert!(has_contains(&h, "also revoke MoneyKeep's access"));
    h.get_by_role_and_label(Role::Button, "Disconnect").click();
    h.run();
    assert!(secrets.all().is_empty());
    click(&mut h, "Rename My bank");
    retype_all(&mut h, "Connection name", "Old bank");
    click(&mut h, "Save");
    assert!(has(&h, "Old bank"));
}

fn retype_all(h: &mut H, label: &str, text: &str) {
    h.get_by_label(label).focus();
    h.run();
    h.key_press_modifiers(
        mk_views::testing::Modifiers::COMMAND,
        mk_views::testing::Key::A,
    );
    h.run();
    h.get_by_label(label).type_text(text);
    h.run();
}

#[test]
fn app_desktop_at108_background_sync_keeps_the_window_responsive() {
    let fake = FakeSimplefin::new(SET);
    let secrets = MemorySecrets::new(true);
    let (mut h, _d) = feeds_app(&fake, &secrets);
    let c = cmd(
        &mut h,
        "connect.add",
        json!({"setup_input": FakeSimplefin::token("bg108"), "label": "My bank", "confirm": true}),
    );
    cmd(
        &mut h,
        "connect.link_account",
        json!({"connection": c["id"], "remote_account": "a1", "account": "Assets:Checking", "sync_from": "2026-09-01"}),
    );
    // The request takes 12 s; the window must answer well before that (the
    // test harness itself needs a second or two per step under load).
    fake.state.lock().unwrap().delay_ms = 12_000;
    open_connections(&mut h);
    let t = std::time::Instant::now();
    click_s(&mut h, "Sync now My bank");
    assert!(has(&h, "Syncing…"), "{:?}", labels_containing(&h, "Sync"));
    assert!(has(&h, "Cancel sync My bank"));
    // Another view opens at once while the request waits.
    h.get_by_role_and_label(Role::Tab, "Accounts").click();
    h.run_steps(4);
    assert!(has(&h, "New account…"));
    assert!(
        t.elapsed() < std::time::Duration::from_secs(10),
        "{:?}",
        t.elapsed()
    );
    cmd(&mut h, "account.list", json!({}));
    assert!(t.elapsed() < std::time::Duration::from_secs(10));
    // The sync then finishes with its Home attention line.
    h.state_mut().open_view_spec("home");
    wait_for(&mut h, "SimpleFIN: 3 new transactions to review");
    assert!(t.elapsed() >= std::time::Duration::from_secs(11));
}

#[test]
fn app_desktop_r17a_cancel_and_resume() {
    let fake = FakeSimplefin::new(SET);
    let secrets = MemorySecrets::new(true);
    let (mut h, _d) = feeds_app(&fake, &secrets);
    // A claim whose account listing fails leaves "Not finished — resume".
    fake.state.lock().unwrap().list_status = Some(500);
    open_connections(&mut h);
    click(&mut h, "Add connection…");
    type_into(&mut h, "Setup token", &FakeSimplefin::token("a17"));
    type_into(&mut h, "Connection name", "My bank");
    click(&mut h, "Continue");
    click(&mut h, "Claim");
    assert!(
        h.query_all_by_label_contains("Not finished — resume")
            .count()
            >= 1,
        "{:?}",
        labels_containing(&h, "finish")
    );
    click(&mut h, "Close");
    assert!(has(&h, "Not finished — resume"));
    fake.state.lock().unwrap().list_status = None;
    click(&mut h, "Resume My bank");
    assert!(has(&h, "Link Everyday"), "resumed without a new token");
    choose(&mut h, "Link Everyday", "Assets:Checking");
    // Long enough that the harness reaches Cancel before the request ends.
    fake.state.lock().unwrap().delay_ms = 20_000;
    click_s(&mut h, "Done");
    assert!(
        has(&h, "Cancel sync My bank"),
        "{:?}",
        labels_containing(&h, "Sync")
    );
    let audit = cmd(&mut h, "audit.list", json!({}))
        .as_array()
        .unwrap()
        .len();
    click_s(&mut h, "Cancel sync My bank");
    wait_for(&mut h, "Sync cancelled; nothing was recorded.");
    assert_eq!(
        cmd(&mut h, "audit.list", json!({}))
            .as_array()
            .unwrap()
            .len(),
        audit
    );
    assert!(
        cmd(&mut h, "import.list", json!({}))
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// opening-balances.md "Where the wizards start": after linking, Set opening
/// balances now opens the checklist limited to the accounts just linked.
#[test]
fn opening_balances_offer_after_linking() {
    let fake = FakeSimplefin::new(SET);
    let secrets = MemorySecrets::new(true);
    let (mut h, _d) = feeds_app(&fake, &secrets);
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Savings", "type": "asset", "subtype": "bank", "parent": "Assets"}),
    );
    open_connections(&mut h);
    click(&mut h, "Add connection…");
    type_into(&mut h, "Setup token", &FakeSimplefin::token("uiob"));
    type_into(&mut h, "Connection name", "My bank");
    click(&mut h, "Continue");
    click(&mut h, "Claim");
    choose(&mut h, "Link Everyday", "Assets:Checking");
    click(&mut h, "Done");
    click(&mut h, "Set opening balances now (recommended)");
    h.run_steps(2);
    assert!(!has(&h, "Add connection"));
    assert!(has(&h, "Set up opening balances"));
    assert!(
        has(&h, "0 of 1 accounts set up"),
        "{:?}",
        labels_containing(&h, "of")
    );
    assert!(has_contains(&h, "Status of Assets:Checking"));
    assert!(
        !has_contains(&h, "Status of Assets:Savings"),
        "limited to the linked account"
    );
}
