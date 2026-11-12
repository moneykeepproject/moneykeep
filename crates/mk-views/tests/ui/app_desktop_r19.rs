//! app-desktop.md revision 19 acceptance tests (112–114): OFX Direct Connect and
//! SnapTrade, against the registry's scripted servers (no network).
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_commands::Session;
use mk_commands::connect::{Connector, Fetcher, MemorySecrets};
use mk_commands::ofx_conn::FakeOfx;
use mk_commands::snap_conn::FakeSnaptrade;
use mk_ui::UserConfig;
use mk_ui::ctl::App;
use mk_views::testing::Role;
use serde_json::{Value, json};

type H = mk_views::testing::Harness;

/// 2026-10-04 00:00 UTC.
const NOW: i64 = 1_791_072_000;

fn app_with(fetcher: std::sync::Arc<dyn Fetcher>) -> (H, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("books.moneykeep");
    let mut s = Session::create(&path, "Test Books", "USD", "app").unwrap();
    s.set_connector(Connector {
        fetcher,
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(NOW),
        host: None,
    });
    for (c, a) in [
        ("module.enable", json!({"module": "bank-feeds"})),
        ("module.enable", json!({"module": "investments"})),
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
            json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
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
    h.state_mut().set_today("2026-10-04");
    h.run();
    (h, dir)
}

fn open_connections(h: &mut H) {
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Connections").click();
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

#[test]
fn app_desktop_at112_ofx_host_tick_box_and_pending_account() {
    let fake = FakeOfx::standard();
    let (mut h, _d) = app_with(std::sync::Arc::new(fake.clone()));
    open_connections(&mut h);
    click(&mut h, "Add connection…");
    click(&mut h, "Provider");
    h.get_by_role_and_label(Role::Button, "Bank (OFX Direct Connect)")
        .click();
    h.run();
    type_into(&mut h, "Institution name", "Test Bank");
    type_into(&mut h, "OFX address", "https://ofx.testbank.example/ofx");
    type_into(&mut h, "ORG", "TESTBANK");
    type_into(&mut h, "FID", "1234");
    click(&mut h, "Continue");
    assert!(
        has(&h, "ofx.testbank.example"),
        "{:?}",
        labels_containing(&h, "ofx")
    );
    assert!(has(
        &h,
        "Your user id and password will be sent to ofx.testbank.example."
    ));
    type_into(&mut h, "User id", "user1");
    type_into(&mut h, "Password", "pass1");
    assert!(disabled(&h, "Connect"), "Connect waits for the tick-box");
    assert!(fake.requests().is_empty());
    click(&mut h, "I recognise this address as my bank's");
    assert!(!disabled(&h, "Connect"));
    click(&mut h, "Connect");
    assert!(
        has(&h, "Brokerage ****9876 — PEND (not linkable)"),
        "{:?}",
        labels_containing(&h, "****")
    );
    assert!(
        has(&h, "Link Everyday Checking ****1234"),
        "the active account can be linked"
    );
    assert!(h.query_by_label("Link Brokerage ****9876").is_none());
}

#[test]
fn app_desktop_at113_extra_questions_during_sync_now() {
    let fake = FakeOfx::standard();
    let (mut h, _d) = app_with(std::sync::Arc::new(fake.clone()));
    let c = cmd(
        &mut h,
        "connect.add",
        json!({"provider": "ofx", "user_id": "user1", "password": "pass1", "host_acknowledged": true,
            "label": "Test Bank", "institution": {"name": "Test Bank",
            "url": "https://ofx.testbank.example/ofx", "org": "TESTBANK", "fid": "1234"}}),
    );
    let checking = c["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["remote"]["kind"] == "bank")
        .unwrap()["remote_account_id"]
        .clone();
    cmd(
        &mut h,
        "connect.link_account",
        json!({"connection": c["id"], "remote_account": checking, "account": "Assets:Checking"}),
    );
    fake.with(|s| {
        s.challenge = vec![
            ("MFA13".into(), String::new()),
            ("123".into(), "Which branch?".into()),
        ];
        s.answers = vec![
            ("MFA13".into(), "6789".into()),
            ("123".into(), "Main".into()),
        ];
    });
    open_connections(&mut h);
    click(&mut h, "Sync now Test Bank");
    wait_for(&mut h, "Extra questions");
    assert!(has(&h, "Last four digits of your social security number"));
    assert!(has(&h, "Which branch?"));
    type_into(&mut h, "Answer 1", "6789");
    type_into(&mut h, "Answer 2", "Main");
    click(&mut h, "Send and sync");
    wait_for(&mut h, "Test Bank: 3 new transactions to review.");
    assert!(fake.requests().last().unwrap().contains("<MFAPHRASEA>Main"));
}

fn act(id: &str, units: &str) -> Value {
    let text = format!(
        r#"{{"id": "{id}", "type": "BUY", "trade_date": "2026-09-02T00:00:00.000Z", "units": {units},
            "price": 220.00, "amount": -2200.00, "fee": 0, "currency": {{"code": "USD"}},
            "description": "BUY VTI", "option_symbol": null,
            "symbol": {{"symbol": "VTI", "raw_symbol": "VTI", "description": "Vanguard Total Stock Market ETF"}}}}"#
    );
    serde_json::from_str(&text).unwrap()
}

#[test]
fn app_desktop_at114_holdings_check_panel_and_home() {
    let fake = FakeSnaptrade::standard();
    fake.with(|s| {
        s.activities.insert("acct-1".into(), vec![act("b1", "10")]);
    });
    let (mut h, _d) = app_with(std::sync::Arc::new(fake.clone()));
    let c = cmd(
        &mut h,
        "connect.add",
        json!({"provider": "snaptrade", "client_id": "TESTCLIENT", "consumer_key": "test-consumer-key", "label": "Brokerages"}),
    );
    cmd(
        &mut h,
        "connect.link_account",
        json!({"connection": c["id"], "remote_account": "acct-1", "account": "Assets:Brokerage"}),
    );
    let out = cmd(&mut h, "connect.sync", json!({"now": true}));
    cmd(
        &mut h,
        "import.commit",
        json!({"batch": out["connections"][0]["batch"]}),
    );
    fake.with(|s| {
        s.positions.insert(
            "acct-1".into(),
            vec![serde_json::from_str(r#"{"symbol": {"symbol": {"symbol": "VTI", "raw_symbol": "VTI"}}, "units": 12.5, "price": 231.17}"#).unwrap()],
        );
    });
    let out = cmd(&mut h, "connect.sync", json!({"now": true}));
    let batch = out["connections"][0]["batch"].as_str().unwrap().to_owned();
    h.state_mut().open_view_spec("home");
    h.run();
    assert!(
        has(&h, "Assets:Brokerage: 1 holding differs from SnapTrade"),
        "{:?}",
        labels_containing(&h, "holding")
    );
    h.state_mut().open_view_spec(&format!("import:{batch}"));
    h.run();
    assert!(
        has(&h, "Holdings check"),
        "{:?}",
        labels_containing(&h, "Holdings")
    );
    assert!(has(
        &h,
        "SnapTrade holds 12.5 VTI; MoneyKeep will hold 10 after this import"
    ));
    click(&mut h, "Add a row for the VTI difference");
    let b = cmd(&mut h, "import.get", json!({"batch": batch}));
    assert!(b["inv_rows"].to_string().contains("shares_in"), "{b}");
}

/// connectivity-brokerage.md revision 2, test 20 on the screen: positions answering
/// 410 while activities and balances answer: the connection card shows the yellow
/// line naming holdings and the 410 wording, and no red line.
#[test]
fn brokerage_rev2_yellow_line_on_the_connection() {
    let fake = FakeSnaptrade::standard();
    fake.with(|s| {
        s.activities.insert("acct-1".into(), vec![act("b1", "10")]);
        s.fail.insert("positions".into(), (410, "{}".into()));
    });
    let (mut h, _d) = app_with(std::sync::Arc::new(fake.clone()));
    let c = cmd(
        &mut h,
        "connect.add",
        json!({"provider": "snaptrade", "client_id": "TESTCLIENT", "consumer_key": "test-consumer-key", "label": "Brokerages"}),
    );
    cmd(
        &mut h,
        "connect.link_account",
        json!({"connection": c["id"], "remote_account": "acct-1", "account": "Assets:Brokerage"}),
    );
    cmd(&mut h, "connect.sync", json!({"now": true}));
    open_connections(&mut h);
    let line = "Holdings couldn't be read (SnapTrade answered 410 for positions). SnapTrade no longer offers this request (positions). MoneyKeep needs an update. Transactions were imported.";
    assert!(has(&h, line), "{:?}", labels_containing(&h, "couldn't"));
    assert!(!has_contains(&h, "try again later"));
}
