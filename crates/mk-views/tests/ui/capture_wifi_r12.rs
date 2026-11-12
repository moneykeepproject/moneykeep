//! mobile-capture.md revision 12, tests 77–79 (desktop): the Wi-Fi setup's name
//! field is blank by default and best left so, a typed name must lead to the
//! address (or Use it anyway), and Settings > Phone capture lists Wi-Fi first and
//! Cloudflare after it. Against the scripted capture server and computer of
//! `mk_commands::capture_setup::testing`: no network, nothing installed.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::common::*;
use mk_commands::Session;
use mk_commands::capture_setup::LocalAddress;
use mk_commands::capture_setup::testing::{FakeCloudflare, FakeHost};
use mk_commands::connect::{Connector, MemorySecrets};
use mk_ui::UserConfig;
use mk_ui::ctl::App;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

const NAME_LABEL: &str = "Local name (optional — leave blank)";
const NAME_LINE: &str = "Blank is best: the IP address is used for the certificate and pairing. Use a name only if your router or DNS server answers for it.";

struct T {
    h: H,
    host: FakeHost,
    _dir: tempfile::TempDir,
}

fn start() -> T {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("w.moneykeep"), "Wi-Fi Books", "USD", "app").unwrap();
    let cf = FakeCloudflare::new();
    let host = FakeHost::windows();
    host.with(|st| {
        st.addresses = vec![LocalAddress {
            address: "192.168.1.20".into(),
            adapter: "Wi-Fi".into(),
            mac: "a4:b1:c2:d3:e4:f5".into(),
            profile: "Private".into(),
            dhcp: true,
        }];
        // The computer's resolver knows one name, for another address.
        st.names = vec![("printer.home.arpa".into(), vec!["192.168.1.50".into()])];
    });
    s.set_connector(Connector {
        fetcher: Arc::new(cf),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(1_790_812_800),
        host: Some(Arc::new(host.clone())),
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
    T { h, host, _dir: dir }
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

fn enabled(h: &H, label: &str) -> bool {
    !h.get_by_role_and_label(Role::Button, label).is_disabled()
}

fn type_pin(h: &mut H, label: &str, text: &str) {
    h.get_by_role_and_label(Role::PasswordInput, label).focus();
    h.run();
    h.get_by_role_and_label(Role::PasswordInput, label)
        .type_text(text);
    h.run_steps(2);
}

/// From step 1 to Start: the PIN, the firewall step, then Start.
fn to_start(t: &mut T, pin: bool) {
    press(&mut t.h, "Next");
    if pin {
        type_pin(&mut t.h, "Capture PIN", "246813");
        type_pin(&mut t.h, "Capture PIN again", "246813");
    }
    press(&mut t.h, "Next");
    press(&mut t.h, "Next");
    press(&mut t.h, "Start");
}

fn name_field(h: &H) -> String {
    h.get_by_role_and_label(Role::TextInput, NAME_LABEL)
        .value()
        .unwrap_or_default()
}

/// Test 77: step 1 shows the new label and line; the field starts empty; with an
/// empty name the QR codes and the certificate use the address only.
#[test]
fn r12_at77_blank_name_is_best() {
    let mut t = start();
    press(&mut t.h, "Set up Wi-Fi capture…");
    wait_for(&mut t.h, "192.168.1.20 — Wi-Fi");
    assert!(has(&t.h, NAME_LABEL) || shows(&t.h, NAME_LABEL));
    assert!(shows(&t.h, NAME_LINE));
    assert_eq!(name_field(&t.h), "", "never pre-filled");
    assert!(
        t.h.get_by_role_and_label(Role::TextInput, NAME_LABEL)
            .placeholder()
            .unwrap_or_default()
            .is_empty(),
        "no example name in the field"
    );
    to_start(&mut t, true);
    wait_for(&mut t.h, "Running.");
    let req = t.host.with(|s| s.wifi_installs[0].clone()).unwrap();
    assert_eq!(req.address, "192.168.1.20");
    assert_eq!(req.name, "", "the certificate is for the address alone");
    assert!(t.host.with(|s| s.lookups.is_empty()).unwrap());
    press(&mut t.h, "Next");
    assert!(has(&t.h, "QR code 1: http://192.168.1.20:14080/"));
    press(&mut t.h, "Next");
    wait_for(
        &mut t.h,
        "QR code 2: https://192.168.1.20:14443/#pair=ABCD2345",
    );
    // Change address… shows the same label and line.
    press(&mut t.h, "Done");
    press(&mut t.h, "Change address…");
    assert!(shows(&t.h, NAME_LABEL));
    assert!(shows(&t.h, NAME_LINE));
    assert_eq!(
        t.h.get_by_role_and_label(Role::TextInput, "New local name")
            .value()
            .unwrap_or_default(),
        ""
    );
}

/// Test 78: a name that does not lead to the address shows the warning in step 1
/// and needs Use it anyway before Next; then Start uses the name.
#[test]
fn r12_at78_unresolved_name_needs_use_it_anyway() {
    let warning = "This name does not lead to 192.168.1.20 on this computer. Leave it blank to use the address.";
    let mut t = start();
    press(&mut t.h, "Set up Wi-Fi capture…");
    wait_for(&mut t.h, "192.168.1.20 — Wi-Fi");
    t.h.get_by_role_and_label(Role::TextInput, NAME_LABEL)
        .set_text("printer.home.arpa");
    t.h.run_steps(2);
    to_start(&mut t, true);
    // Start looked the name up and went back to step 1 with the warning.
    wait_for(&mut t.h, warning);
    assert!(t.host.with(|s| s.wifi_installs.is_empty()).unwrap());
    assert_eq!(
        t.host.with(|s| s.lookups.clone()).unwrap(),
        vec!["printer.home.arpa".to_owned()]
    );
    assert!(shows(&t.h, NAME_LINE), "back at step 1");
    assert!(!enabled(&t.h, "Next"), "Next waits for Use it anyway");
    press(&mut t.h, "Use it anyway");
    assert!(enabled(&t.h, "Next"));
    // The PIN typed before is kept; the steps go on to Start again.
    to_start(&mut t, false);
    wait_for(&mut t.h, "Running.");
    let req = t.host.with(|s| s.wifi_installs[0].clone()).unwrap();
    assert_eq!(req.name, "printer.home.arpa");
    // Editing the name again asks again; clearing it needs no question.
    press(&mut t.h, "Back");
    press(&mut t.h, "Back");
    press(&mut t.h, "Back");
    t.h.get_by_role_and_label(Role::TextInput, NAME_LABEL)
        .set_text("");
    t.h.run_steps(2);
    assert!(!shows(&t.h, warning));
    assert!(enabled(&t.h, "Next"));
}

/// Test 79: Settings > Phone capture: the Wi-Fi heading comes before the
/// Cloudflare heading.
#[test]
fn r12_at79_wifi_before_cloudflare() {
    let t = start();
    let y = |label: &str| {
        t.h.query_all_by_label(label)
            .map(|n| n.rect().min.y)
            .reduce(f32::min)
            .unwrap_or_else(|| panic!("{label} missing"))
    };
    let wifi = y("Wi-Fi capture");
    let cloudflare = y("Advanced: phone capture over Cloudflare");
    assert!(wifi < cloudflare, "Wi-Fi {wifi} / Cloudflare {cloudflare}");
    // Setup, status and phones are the Wi-Fi part, above the Cloudflare part.
    for part in ["Set up Wi-Fi capture…", "Paired phones"] {
        assert!(y(part) < cloudflare, "{part}");
    }
}
