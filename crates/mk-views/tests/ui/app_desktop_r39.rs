//! app-desktop.md revision 39: one pairing button, and with both Wi-Fi capture and
//! Cloudflare capture set up, Pair a phone… first asks "How will this phone send
//! expenses?". Wi-Fi capture and Cloudflare are the scripted computer, Cloudflare API
//! and capture server of `mk_commands::capture_setup::testing`; nothing touches the
//! network or installs anything.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::common::*;
use mk_commands::Session;
use mk_commands::capture_setup::LocalAddress;
use mk_commands::capture_setup::testing::{FakeCloudflare, FakeHost, TEST_TOKEN};
use mk_commands::connect::{Connector, MemorySecrets};
use mk_ui::UserConfig;
use mk_ui::ctl::App;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

const QUESTION: &str = "How will this phone send expenses?";
const WIFI: &str = "Wi-Fi at home (recommended)";
const WIFI_HELP: &str = "Sends when the phone is on the same network as this computer. Faster, and works without the internet.";
const CLOUDFLARE: &str = "From anywhere (Cloudflare)";
const CLOUDFLARE_HELP: &str = "Sends from any network through your Cloudflare address. Use this if the phone is often away from home.";
const NOTE: &str = "A phone pairs with one of them. You can pair it again later to switch.";
const WIFI_QR: &str = "QR code 2: https://192.168.1.20:14443/#pair=ABCD2345";
const CLOUDFLARE_QR: &str = "QR code for https://capture.example.com/#pair=ABCD2345";

/// Settings → Phone capture of a book with Wi-Fi capture set up (through its
/// dialog), and Cloudflare capture too when `cloudflare`.
fn app(cloudflare: bool) -> (H, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("p.moneykeep"), "Pair Books", "USD", "app").unwrap();
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
    if cloudflare {
        s.invoke("capture.setup_check_token", json!({"token": TEST_TOKEN}))
            .unwrap();
        s.invoke(
            "capture.setup_apply",
            json!({"emails": ["owner@example.com"]}),
        )
        .unwrap();
    }
    let app = App::new(
        Box::new(LocalBackend {
            session: Some(s),
            name: Some("Pair Books".into()),
        }),
        UserConfig::default(),
    );
    let mut h = harness(app);
    h.run();
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Phone capture").click();
    h.run_steps(3);
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

fn buttons(h: &H, label: &str) -> usize {
    h.query_all_by_role_and_label(Role::Button, label).count()
}

/// With both set up, Pair a phone… shows the choice with both explanations and the
/// note; Wi-Fi at home opens the Wi-Fi QR flow.
#[test]
fn r39_both_set_up_wifi_choice_opens_the_wifi_qr_flow() {
    let (mut h, _d) = app(true);
    // The Wi-Fi card has no pairing button of its own.
    assert_eq!(buttons(&h, "Pair a phone…"), 1);
    assert!(!has_contains(&h, "Pair a phone over Wi-Fi"));
    press(&mut h, "Pair a phone…");
    wait_for(&mut h, QUESTION);
    for part in [WIFI_HELP, CLOUDFLARE_HELP, NOTE] {
        assert!(
            shows(&h, part),
            "{part:?}: {:#?}",
            labels_containing(&h, "")
        );
    }
    assert_eq!(buttons(&h, WIFI), 1);
    assert_eq!(buttons(&h, CLOUDFLARE), 1);
    // Each choice sits above its explanation; the note comes last.
    let y = |label: &str| {
        h.query_all_by_label_contains(label)
            .map(|n| n.rect().min.y)
            .reduce(f32::min)
            .unwrap()
    };
    assert!(y(WIFI) < y(WIFI_HELP));
    assert!(y(WIFI_HELP) < y(CLOUDFLARE));
    assert!(y(CLOUDFLARE) < y(CLOUDFLARE_HELP));
    assert!(y(CLOUDFLARE_HELP) < y(NOTE));
    assert!(!shows(&h, "QR code 2:") && !shows(&h, "QR code for"));
    press(&mut h, WIFI);
    wait_for(&mut h, WIFI_QR);
    assert!(!shows(&h, QUESTION));
    assert!(!shows(&h, CLOUDFLARE_QR));
    assert!(shows(&h, "Pairing code ABCD2345"));
}

/// With both set up, From anywhere (Cloudflare) opens the Cloudflare setup's QR flow;
/// Cancel closes the question without opening either.
#[test]
fn r39_both_set_up_cloudflare_choice_opens_its_qr_flow() {
    let (mut h, _d) = app(true);
    press(&mut h, "Pair a phone…");
    wait_for(&mut h, QUESTION);
    press(&mut h, "Cancel");
    assert!(!shows(&h, QUESTION));
    assert!(!shows(&h, "QR code 2:") && !shows(&h, "QR code for"));
    press(&mut h, "Pair a phone…");
    wait_for(&mut h, QUESTION);
    press(&mut h, CLOUDFLARE);
    wait_for(&mut h, CLOUDFLARE_QR);
    assert!(!shows(&h, QUESTION));
    assert!(!shows(&h, WIFI_QR));
    assert!(shows(&h, "Pairing code ABCD2345"));
}

/// With Wi-Fi only, the question is skipped: the Wi-Fi QR flow opens directly.
#[test]
fn r39_wifi_only_skips_the_question() {
    let (mut h, _d) = app(false);
    assert_eq!(buttons(&h, "Pair a phone…"), 1);
    assert!(!has_contains(&h, "Pair a phone over Wi-Fi"));
    press(&mut h, "Pair a phone…");
    wait_for(&mut h, WIFI_QR);
    assert!(!shows(&h, QUESTION));
}
