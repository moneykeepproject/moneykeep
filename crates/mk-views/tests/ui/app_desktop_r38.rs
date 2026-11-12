//! app-desktop.md revision 38: dismissing Attention notices (connectivity.md
//! revision 5) and one Pair a phone… button. The bank feed is the registry's
//! scripted SimpleFIN server and Wi-Fi capture the scripted computer of
//! `mk_commands::capture_setup::testing`; nothing touches the network.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::common::*;
use mk_commands::Session;
use mk_commands::capture_setup::LocalAddress;
use mk_commands::capture_setup::testing::{FakeCloudflare, FakeHost};
use mk_commands::connect::{Connector, FakeSimplefin, MemorySecrets};
use mk_ui::UserConfig;
use mk_ui::ctl::App;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

const FAILING: &str = r#"{
  "errlist": [{"code": "gen.auth", "msg": "Authentication failed"}],
  "connections": [{"conn_id": "c1", "name": "First Bank"}],
  "accounts": [
    {"id": "a1", "name": "Everyday", "conn_id": "c1", "currency": "USD",
     "balance": "1200.50", "balance-date": 1790812800,
     "transactions": [
       {"id": "t1", "posted": 1790553600, "amount": "-45.10", "description": "Corner Grocery"},
       {"id": "t2", "posted": 1790640000, "amount": "2000.00", "description": "Payroll"},
       {"id": "t3", "posted": 1790726400, "amount": "-12.00", "description": "Bakery"}
     ]}
  ]
}"#;

const NOTICE: &str = "My bank: Authentication failed";

fn feeds_app() -> (H, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("f.moneykeep"), "Test Books", "USD", "app").unwrap();
    s.set_connector(Connector {
        fetcher: Arc::new(FakeSimplefin::new(FAILING)),
        secrets: Box::new(MemorySecrets::new(true)),
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
    let c = cmd(
        &mut h,
        "connect.add",
        json!({"setup_input": FakeSimplefin::token("r38"), "label": "My bank", "confirm": true}),
    );
    cmd(
        &mut h,
        "connect.link_account",
        json!({"connection": c["id"], "remote_account": "a1", "account": "Assets:Checking",
               "sync_from": "2026-09-01"}),
    );
    cmd(&mut h, "connect.sync", json!({"now": true}));
    (h, dir)
}

fn dismiss_buttons(h: &H) -> usize {
    h.query_all_by_role_and_label(Role::Button, "Dismiss")
        .count()
}

/// Home with a dismissible connection notice: × removes it, Undo brings it back; the
/// "SimpleFIN: 3 new transactions to review" line has no ×.
#[test]
fn r38_home_attention_dismiss_and_undo() {
    let (mut h, _d) = feeds_app();
    h.state_mut().open_view_spec("home");
    h.run_steps(3);
    assert!(has_contains(&h, NOTICE), "{:?}", labels_containing(&h, ""));
    assert!(has_contains(&h, "SimpleFIN: 3 new transactions to review"));
    assert_eq!(dismiss_buttons(&h), 1, "only the connection notice has ×");
    h.get_by_role_and_label(Role::Button, "Dismiss").click();
    h.run_steps(3);
    assert!(!has_contains(&h, NOTICE), "{:?}", labels_containing(&h, ""));
    assert!(has_contains(&h, "SimpleFIN: 3 new transactions to review"));
    assert!(has_contains(&h, "Notice dismissed."));
    h.get_by_role_and_label(Role::Button, "Undo").click();
    h.run_steps(3);
    assert!(has_contains(&h, NOTICE), "{:?}", labels_containing(&h, ""));
}

/// The same × is in Settings → Connections; once dismissed the message is gone there.
#[test]
fn r38_connections_dismiss() {
    let (mut h, _d) = feeds_app();
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Connections").click();
    h.run_steps(3);
    assert!(has_contains(&h, "Authentication failed"));
    assert_eq!(dismiss_buttons(&h), 1);
    h.get_by_role_and_label(Role::Button, "Dismiss").click();
    h.run_steps(3);
    assert!(!has_contains(&h, "Authentication failed"));
    assert!(has_contains(&h, "Notice dismissed."));
    h.state_mut().open_view_spec("home");
    h.run_steps(3);
    assert!(!has_contains(&h, NOTICE));
}

// ----- one Pair a phone… -----------------------------------------------------------------

fn wifi_app() -> (H, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("w.moneykeep"), "Wi-Fi Books", "USD", "app").unwrap();
    let host = FakeHost::windows();
    host.with(|st| {
        st.addresses = vec![LocalAddress {
            address: "192.168.1.20".into(),
            adapter: "Wi-Fi".into(),
            mac: "a4:b1:c2:d3:e4:f5".into(),
            profile: "Private".into(),
            dhcp: true,
        }];
    });
    s.set_connector(Connector {
        fetcher: Arc::new(FakeCloudflare::new()),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(1_790_812_800),
        host: Some(Arc::new(host)),
    });
    s.invoke("module.enable", json!({"module": "expenses"}))
        .unwrap();
    let app = App::new(
        Box::new(LocalBackend {
            session: Some(s),
            name: Some("Wi-Fi Books".into()),
        }),
        UserConfig::default(),
    );
    let mut h = harness(app);
    h.run();
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Phone capture").click();
    h.run_steps(3);
    (h, dir)
}

fn shows(h: &H, part: &str) -> bool {
    labels_containing(h, "").iter().any(|l| l.contains(part))
}

fn wait_for(h: &mut H, part: &str) {
    for _ in 0..400 {
        h.run_steps(2);
        if shows(h, part) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    panic!("{part:?} never showed: {:#?}", labels_containing(h, ""));
}

fn press(h: &mut H, label: &str) {
    h.get_by_role_and_label(Role::Button, label).click();
    h.run_steps(3);
}

fn type_pin(h: &mut H, label: &str, text: &str) {
    h.get_by_role_and_label(Role::PasswordInput, label).focus();
    h.run();
    h.get_by_role_and_label(Role::PasswordInput, label)
        .type_text(text);
    h.run_steps(2);
}

/// Settings → Phone capture with Wi-Fi set up: exactly one "Pair a phone…" button,
/// and it opens the QR flow with the QR code and the code under "Can't scan?".
#[test]
fn r38_one_pair_a_phone_button_opens_the_qr_flow() {
    let (mut h, _d) = wifi_app();
    press(&mut h, "Set up Wi-Fi capture…");
    wait_for(&mut h, "192.168.1.20 — Wi-Fi");
    press(&mut h, "Next");
    type_pin(&mut h, "Capture PIN", "246813");
    type_pin(&mut h, "Capture PIN again", "246813");
    press(&mut h, "Next");
    press(&mut h, "Next");
    press(&mut h, "Start");
    wait_for(&mut h, "Running.");
    press(&mut h, "Close");
    h.run_steps(3);
    let pair = h
        .query_all_by_role_and_label(Role::Button, "Pair a phone…")
        .count();
    assert_eq!(pair, 1, "{:?}", labels_containing(&h, "Pair"));
    assert!(!has(&h, "Pair a phone with a QR code…"));
    press(&mut h, "Pair a phone…");
    wait_for(
        &mut h,
        "QR code 2: https://192.168.1.20:14443/#pair=ABCD2345",
    );
    assert!(shows(&h, "Can't scan?"));
    assert!(shows(&h, "Pairing code ABCD2345"));
    // The code comes after "Can't scan?".
    let y = |label: &str| {
        h.query_all_by_label_contains(label)
            .map(|n| n.rect().min.y)
            .reduce(f32::min)
            .unwrap()
    };
    assert!(y("Can't scan?") < y("Pairing code ABCD2345"));
    assert!(y("QR code 2:") < y("Can't scan?"));
    // mobile-capture.md test 87: under the code, that the phone asks for the PIN.
    assert!(shows(&h, "The phone will also ask for your Capture PIN."));
    assert!(y("Pairing code ABCD2345") < y("The phone will also ask for your Capture PIN."));
}
