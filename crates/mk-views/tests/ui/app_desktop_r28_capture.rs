//! app-desktop.md revision 28 acceptance test 179 (Phase 6b): Settings → Phone
//! capture against the capture server running in-process on 127.0.0.1 (Access
//! checks off, loopback only). Nothing touches the internet.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::common::*;
use mk_capture_api::{ExpenseFields, NewExpense};
use mk_commands::Session;
use mk_commands::connect::{Connector, MemorySecrets};
use mk_ui::UserConfig;
use mk_ui::ctl::App;
use mk_views::testing::Role;
use moneykeep_server::testing::{LocalServer, call};
use serde_json::{Value, json};

type H = mk_views::testing::Harness;

fn capture_app(server: &LocalServer) -> (H, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("c.moneykeep"), "Test Books", "USD", "app").unwrap();
    s.set_connector(Connector {
        fetcher: Arc::new(mk_connect::HttpsFetcher::trusting(Vec::new())),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: None,
        host: None,
    });
    for (c, a) in [
        ("module.enable", json!({"module": "expenses"})),
        (
            "account.create",
            json!({"name": "Expenses", "type": "expense", "placeholder": true}),
        ),
        (
            "account.create",
            json!({"name": "Office Supplies", "type": "expense", "parent": "Expenses"}),
        ),
        (
            "account.create",
            json!({"name": "Visa", "type": "liability", "subtype": "credit-card"}),
        ),
        (
            "capture.set_server",
            json!({"url": server.url(), "token": server.desktop_token}),
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
    h.run();
    (h, dir)
}

/// Run frames until a label containing `part` shows (background requests finish).
fn wait_for(h: &mut H, part: &str) {
    for _ in 0..400 {
        h.run_steps(2);
        if shows(h, part) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("{part:?} never showed: {:?}", labels_containing(h, ""));
}

/// Whether any node's label or value contains `part`.
fn shows(h: &H, part: &str) -> bool {
    // On screen, or found by scrolling the page (the Slint views do not lay out
    // what is scrolled out of view for assistive technology).
    h.on_screen_contains(part) || h.query_all_by_label_contains(part).next().is_some()
}

fn click_s(h: &mut H, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(4);
}

#[test]
fn app_desktop_at179_phone_capture_settings() {
    let data = tempfile::tempdir().unwrap();
    let server = LocalServer::start(data.path()).unwrap();
    let (mut h, _book) = capture_app(&server);
    h.state_mut().open_view_spec("settings");
    h.run();
    // Revision 10: a server set up by hand is under Settings > Advanced.
    h.get_by_role_and_label(Role::Tab, "Advanced").click();
    h.run();
    assert!(has(&h, "Server address"));
    assert_eq!(value(&h, "Server address"), server.url());

    // Test connection.
    click_s(&mut h, "Test connection");
    wait_for(&mut h, "Connected to the capture server");
    h.get_by_role_and_label(Role::Tab, "Phone capture").click();
    h.run();

    // Pair a phone (app-desktop revision 38): the only pairing button opens the QR
    // flow; the code shows large under "Can't scan?", with its countdown and the page
    // address.
    assert_eq!(
        h.query_all_by_role_and_label(Role::Button, "Pair a phone…")
            .count(),
        1
    );
    click_s(&mut h, "Pair a phone…");
    wait_for(&mut h, "Pairing code");
    assert!(shows(&h, "Can't scan?"));
    let label = labels_containing(&h, "")
        .into_iter()
        .find(|l| l.starts_with("Pairing code"))
        .unwrap();
    let code = label.trim_start_matches("Pairing code ").to_owned();
    assert_eq!(code.len(), 8, "{label}");
    assert!(
        code.bytes()
            .all(|b| mk_capture_api::PAIRING_ALPHABET.contains(&b))
    );
    assert!(shows(&h, "Expires in"));
    // mobile-capture.md test 87, as corrected after revision 15: away from Wi-Fi the
    // phone asks for the Capture PIN only when one is set; none is set here.
    assert!(!shows(&h, "The phone will also ask for your Capture PIN."));
    assert!(shows(&h, "https://capture.example.test"));
    assert!(shows(
        &h,
        &format!("QR code for https://capture.example.test/#pair={code}")
    ));
    // The phone pairs with the code shown.
    let body = json!({"code": code, "device_name": "Owner's phone"}).to_string();
    let (s, b) = call(
        &server.url(),
        "POST",
        "/api/v1/pair",
        &[("Content-Type", "application/json")],
        Some(body.as_bytes()),
    )
    .unwrap();
    assert_eq!(s, 201);
    let phone: Value = serde_json::from_slice(&b).unwrap();
    let phone = phone["device_token"].as_str().unwrap().to_owned();
    click_s(&mut h, "Done");
    wait_for(&mut h, "Owner's phone");
    assert!(h.query_by_label("Revoke Owner's phone").is_some());

    // The phone sends an expense; Collect now brings it into Expenses.
    let today = mk_capture_api::format_date(mk_capture_api::today_from_unix(
        moneykeep_server::Clock::default().now(),
        0,
    ));
    let e = NewExpense {
        client_id: "7a1c2d3e-0000-4000-8000-000000000179".into(),
        fields: ExpenseFields {
            date: Some(today),
            merchant: Some("Paper Corner".into()),
            amount: Some("12.50".into()),
            category: Some("Expenses:Office Supplies".into()),
            paid_with: Some("Visa".into()),
            ..ExpenseFields::default()
        },
        receipt_ids: Vec::new(),
    };
    assert_eq!(server.submit(&phone, &e).unwrap(), 201);
    click_s(&mut h, "Collect now");
    wait_for(&mut h, "from your phone collected");

    h.state_mut().open_view_spec("expenses");
    h.run_steps(4);
    assert!(shows(&h, "Paper Corner"));
    assert!(shows(&h, "From phone"));

    h.state_mut().open_view_spec("home");
    h.run_steps(4);
    assert!(
        shows(&h, "1 expense from your phone to review"),
        "{:?}",
        labels_containing(&h, "")
    );
}
