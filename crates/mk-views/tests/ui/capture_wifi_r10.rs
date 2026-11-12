//! mobile-capture.md revision 10, tests 71, 74 and 75 (desktop): Wi-Fi capture's
//! setup dialog in Settings > Phone capture, against the scripted capture server and
//! computer of `mk_commands::capture_setup::testing`. Nothing touches the network
//! or installs anything.
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

struct T {
    h: H,
    cf: FakeCloudflare,
    host: FakeHost,
    _dir: tempfile::TempDir,
}

fn start(public: bool) -> T {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("w.moneykeep"), "Wi-Fi Books", "USD", "app").unwrap();
    let cf = FakeCloudflare::new();
    let host = FakeHost::windows();
    host.with(|st| {
        st.addresses = vec![
            LocalAddress {
                address: "192.168.1.20".into(),
                adapter: "Wi-Fi".into(),
                mac: "a4:b1:c2:d3:e4:f5".into(),
                profile: if public { "Public" } else { "Private" }.into(),
                dhcp: true,
            },
            LocalAddress {
                address: "10.0.0.7".into(),
                adapter: "Ethernet".into(),
                mac: "00:11:22:33:44:55".into(),
                profile: "Private".into(),
                dhcp: false,
            },
        ];
    });
    s.set_connector(Connector {
        fetcher: Arc::new(cf.clone()),
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
    T {
        h,
        cf,
        host,
        _dir: dir,
    }
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

fn enabled(h: &H, label: &str) -> bool {
    !h.get_by_role_and_label(Role::Button, label).is_disabled()
}

/// Test 71 (desktop) and 74: Settings > Phone capture opens Wi-Fi setup; the
/// steps, PIN rules, the firewall step with its Public warning, the two QR codes,
/// resuming after closing, Paired: <name>, the devices' way in, and Remove.
#[test]
fn at71_wifi_setup_dialog() {
    let mut t = start(true);
    // Test 74: Phone capture is Wi-Fi; Cloudflare is under Advanced.
    assert!(!has(&t.h, "Set up phone capture over Cloudflare…"));
    press(&mut t.h, "Set up Wi-Fi capture…");
    // 1. Address: the computer's addresses with their adapters.
    wait_for(
        &mut t.h,
        "192.168.1.20 — Wi-Fi (a4:b1:c2:d3:e4:f5) · Public network",
    );
    assert!(shows(&t.h, "10.0.0.7 — Ethernet"));
    assert!(shows(&t.h, "How do I make it fixed?"));
    assert_eq!(value(&t.h, "Address"), "192.168.1.20");
    press(&mut t.h, "Next");
    // 2. Capture PIN, twice, 6–12 digits.
    assert!(shows(&t.h, "not your book password") || shows(&t.h, "Not your book password"));
    assert!(!enabled(&t.h, "Next"));
    type_pin(&mut t.h, "Capture PIN", "12345");
    type_pin(&mut t.h, "Capture PIN again", "12345");
    assert!(shows(&t.h, "The Capture PIN is 6 to 12 digits."));
    assert!(!enabled(&t.h, "Next"));
    type_pin(&mut t.h, "Capture PIN", "6");
    type_pin(&mut t.h, "Capture PIN again", "7");
    assert!(shows(&t.h, "The two PINs are not the same."));
    assert!(!enabled(&t.h, "Next"));
    // Replace the second entry.
    t.h.get_by_role_and_label(Role::PasswordInput, "Capture PIN again")
        .set_text("123456");
    t.h.run_steps(2);
    assert!(enabled(&t.h, "Next"));
    press(&mut t.h, "Next");
    // 3. Firewall: Private and Domain only; the Public network warning.
    assert!(shows(&t.h, "Private and Domain networks only"));
    assert!(shows(&t.h, "Windows treats this network as Public"));
    // Closing keeps the place: the dialog resumes at the same step.
    press(&mut t.h, "Close");
    assert!(!shows(&t.h, "Private and Domain networks only"));
    press(&mut t.h, "Continue Wi-Fi setup…");
    assert!(shows(&t.h, "Private and Domain networks only"), "resumed");
    press(&mut t.h, "Next");
    // 4. Start: the firewall rule request goes through the fake computer.
    press(&mut t.h, "Start");
    wait_for(&mut t.h, "Running.");
    let req = t.host.with(|s| s.wifi_installs[0].clone()).unwrap();
    assert_eq!(req.address, "192.168.1.20");
    assert_eq!(
        req.firewall.profiles,
        vec!["Private".to_owned(), "Domain".to_owned()]
    );
    assert_eq!(
        t.cf.with(|s| s.pins.clone()).unwrap(),
        vec!["123456".to_owned()]
    );
    press(&mut t.h, "Next");
    // 5. Certificate: QR code 1, the fingerprint, both phones' steps.
    assert!(has(&t.h, "QR code 1: http://192.168.1.20:14080/"));
    assert!(has(
        &t.h,
        "Fingerprint A1B2 C3D4 E5F6 0718 293A 4B5C 6D7E 8F90"
    ));
    assert!(shows(&t.h, "Encryption & credentials"));
    assert!(shows(&t.h, "Certificate Trust Settings"));
    press(&mut t.h, "Next");
    // 6. Pair: QR code 2 with the code; the new phone is seen.
    wait_for(
        &mut t.h,
        "QR code 2: https://192.168.1.20:14443/#pair=ABCD2345",
    );
    t.cf.with(|s| {
        s.devices = vec![
            json!({"id": "0123456789abcdef0123456789abcdef", "name": "Pixel 8",
                                 "paired_at": 1_790_812_800, "via": "wifi"}),
        ];
    });
    for _ in 0..40 {
        t.h.advance(1.0);
        t.h.run_steps(2);
        if shows(&t.h, "Paired: Pixel 8") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    assert!(shows(&t.h, "Paired: Pixel 8"));
    assert!(shows(&t.h, "Add to Home Screen"));
    // Test 74: the devices list shows the way in.
    t.cf.with(|s| {
        s.devices.push(
            json!({"id": "fedcba9876543210fedcba9876543210", "name": "Travel phone",
                              "paired_at": 1_790_812_800, "via": "cloudflare"}),
        );
    });
    press(&mut t.h, "Done");
    wait_for(&mut t.h, "Travel phone");
    assert!(has(&t.h, "Wi-Fi"));
    assert!(has(&t.h, "Cloudflare"));
    assert!(shows(&t.h, "https://192.168.1.20:14443/"));
    // Advanced: Cloudflare setup.
    t.h.get_by_role_and_label(Role::Tab, "Advanced").click();
    t.h.run_steps(3);
    assert!(has(&t.h, "Set up phone capture over Cloudflare…"));
    t.h.get_by_role_and_label(Role::Tab, "Phone capture")
        .click();
    t.h.run_steps(3);
    // Remove Wi-Fi capture removes what was made (after a warning).
    press(&mut t.h, "Remove Wi-Fi capture…");
    assert!(shows(
        &t.h,
        "remove the firewall rule \"MoneyKeep capture (Wi-Fi)\""
    ));
    assert!(shows(&t.h, "disconnect the phones paired over Wi-Fi"));
    press(&mut t.h, "Remove");
    wait_for(&mut t.h, "Set up Wi-Fi capture…");
    t.host
        .with(|s| {
            assert!(s.firewall_rules.is_empty());
            assert!(!s.wifi_certs);
        })
        .unwrap();
    let left = t.cf.with(|s| s.devices.clone()).unwrap();
    assert_eq!(left.len(), 1, "the Cloudflare phone stays");
}

/// Test 75 (desktop): the dialog's Help shows the deployment guide.
#[test]
fn at75_help_opens_the_guide() {
    let mut t = start(false);
    press(&mut t.h, "Set up Wi-Fi capture…");
    press(&mut t.h, "Help");
    assert!(has(&t.h, "2. Give the computer a fixed address"));
    assert!(has(&t.h, "Corporate or office network"));
    // The corporate-network paragraph's text is checked in mk-ui (at75_help_sections);
    // here the help shows its section.
    assert!(has(&t.h, "OPNsense or pfSense firewalls"));
    press(&mut t.h, "Close help");
    assert!(!has(&t.h, "Corporate or office network"));
}
