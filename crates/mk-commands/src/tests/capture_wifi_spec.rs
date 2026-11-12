//! mobile-capture.md revision 10, tests 71, 72 and 74 (command side): Wi-Fi setup
//! through a fake computer and a fake capture server; nothing is installed and no
//! network is used.

use std::sync::Arc;

use super::*;
use crate::capture_setup::testing::{FakeCloudflare, FakeHost};
use crate::capture_setup::{LocalAddress, ServiceState};
use crate::capture_wifi::text;
use crate::connect::{Connector, MemorySecrets};

const NOW: i64 = 1_790_812_800;

struct T {
    b: Book,
    cf: FakeCloudflare,
    host: FakeHost,
}

fn wifi_book() -> T {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "expenses"}));
    let cf = FakeCloudflare::new();
    let host = FakeHost::windows();
    host.with(|s| {
        s.addresses = vec![
            LocalAddress {
                address: "192.168.1.20".into(),
                adapter: "Wi-Fi".into(),
                mac: "a4:b1:c2:d3:e4:f5".into(),
                profile: "Private".into(),
                dhcp: true,
            },
            LocalAddress {
                address: "10.0.0.7".into(),
                adapter: "Ethernet".into(),
                mac: "00:11:22:33:44:55".into(),
                profile: "Public".into(),
                dhcp: false,
            },
            LocalAddress {
                address: "127.0.0.1".into(),
                adapter: "Loopback".into(),
                ..LocalAddress::default()
            },
        ];
    });
    b.r.set_connector(Connector {
        fetcher: Arc::new(cf.clone()),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(NOW),
        host: Some(Arc::new(host.clone())),
    });
    T { b, cf, host }
}

fn apply(t: &mut T) -> Value {
    t.b.run(
        "capture.wifi_apply",
        json!({"address": "192.168.1.20", "pin": "246813"}),
    )
}

/// Test 71: addresses, PIN rules, firewall rule (Private and Domain only), QR
/// addresses, resume, Remove.
#[test]
fn at71_wifi_setup_commands() {
    let mut t = wifi_book();
    // The address list: home and office addresses with adapters, no loopback.
    let a = t.b.run("capture.wifi_addresses", json!({}));
    let list = a["addresses"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["adapter"], "Wi-Fi");
    assert_eq!(list[1]["profile"], "Public");
    assert!(a["why_fixed"].as_str().unwrap().contains("fixed address"));
    // PIN and address rules.
    for (args, what) in [
        (
            json!({"address": "192.168.1.20", "pin": "12345"}),
            "short PIN",
        ),
        (
            json!({"address": "192.168.1.20", "pin": "1234567890123"}),
            "long PIN",
        ),
        (
            json!({"address": "192.168.1.20", "pin": "12a456"}),
            "letters",
        ),
        (json!({"address": "192.168.1.20"}), "no PIN yet"),
        (
            json!({"address": "8.8.8.8", "pin": "246813"}),
            "internet address",
        ),
        (
            json!({"address": "192.168.1.20", "pin": "246813", "https_port": 80}),
            "low port",
        ),
    ] {
        let e = t.b.err("capture.wifi_apply", args);
        assert!(matches!(e, CommandError::InvalidArgs(_)), "{what}: {e}");
    }
    assert!(t.host.with(|s| s.wifi_installs.is_empty()).unwrap());
    let v = apply(&mut t);
    assert_eq!(v["pin_set"], true);
    assert_eq!(v["answered"], true);
    assert_eq!(v["cert_url"], "http://192.168.1.20:14080/");
    assert_eq!(v["app_url"], "https://192.168.1.20:14443/");
    // The firewall rule through the fake host: both ports, Private and Domain only.
    let req = t.host.with(|s| s.wifi_installs[0].clone()).unwrap();
    assert_eq!(req.firewall.name, "MoneyKeep capture (Wi-Fi)");
    assert_eq!(req.firewall.ports, vec![14443, 14080]);
    assert_eq!(
        req.firewall.profiles,
        vec!["Private".to_owned(), "Domain".to_owned()]
    );
    assert!(req.new_desktop_token, "no desktop connection yet");
    // The PIN went to the server only; the book keeps no copy.
    assert_eq!(
        t.cf.with(|s| s.pins.clone()).unwrap(),
        vec!["246813".to_owned()]
    );
    let s = t.b.run("capture.settings", json!({}));
    assert!(!s.to_string().contains("246813"));
    assert_eq!(s["configured"], true, "the desktop token was kept");
    let w = &s["wifi"];
    assert_eq!(w["installed"], true);
    assert_eq!(w["pin_set"], true);
    assert_eq!(w["firewall_rule"], "MoneyKeep capture (Wi-Fi)");
    assert_eq!(w["fingerprint"], "A1B2 C3D4 E5F6 0718 293A 4B5C 6D7E 8F90");
    assert_eq!(w["fingerprint"].as_str().unwrap().split(' ').count(), 8);
    // QR 2: the pairing address with the code.
    let p = t.b.run("capture.wifi_pair_code", json!({}));
    assert_eq!(p["pair_url"], "https://192.168.1.20:14443/#pair=ABCD2345");
    assert_eq!(p["cert_url"], "http://192.168.1.20:14080/");
    // Change PIN.
    t.b.run("capture.wifi_set_pin", json!({"pin": "13572468"}));
    assert_eq!(t.cf.with(|s| s.pins.len()).unwrap(), 2);
    assert!(
        t.b.try_run("capture.wifi_set_pin", json!({"pin": "1"}))
            .is_err()
    );
    // Resume: what was chosen is kept in the book (the dialog reopens there).
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["wifi"]["address"], "192.168.1.20");
    // Remove: the Wi-Fi phones, the firewall rule and the certificates go; a
    // Cloudflare phone stays.
    t.cf.with(|s| {
        s.devices = vec![
            json!({"id": "0123456789abcdef0123456789abcdef", "name": "Pixel", "paired_at": NOW, "via": "wifi"}),
            json!({"id": "fedcba9876543210fedcba9876543210", "name": "Away phone", "paired_at": NOW, "via": "cloudflare"}),
        ];
    });
    let r = t.b.run("capture.wifi_remove", json!({}));
    assert_eq!(r["phones"], 1);
    assert_eq!(r["phones_disconnected"], 1);
    let left = t.cf.with(|s| s.devices.clone()).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["via"], "cloudflare");
    t.host
        .with(|s| {
            assert!(s.firewall_rules.is_empty());
            assert!(!s.wifi_certs);
            assert_eq!(s.wifi_removes[0].firewall_rule, "MoneyKeep capture (Wi-Fi)");
            assert_eq!(
                s.report.server,
                ServiceState::Running,
                "kept for Cloudflare"
            );
        })
        .unwrap();
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["wifi"]["started"], false);
}

/// Test 72: Change address — the warning, a new certificate for the new address,
/// and the Wi-Fi phones not seen for a day.
#[test]
fn at72_change_address() {
    let mut t = wifi_book();
    apply(&mut t);
    t.cf.with(|s| {
        s.devices = vec![
            json!({"id": "0123456789abcdef0123456789abcdef", "name": "Old phone", "paired_at": NOW - 9 * 86_400, "last_seen": NOW - 2 * 86_400, "via": "wifi"}),
            json!({"id": "1123456789abcdef0123456789abcdef", "name": "Daily phone", "paired_at": NOW - 9 * 86_400, "last_seen": NOW - 3600, "via": "wifi"}),
            json!({"id": "2123456789abcdef0123456789abcdef", "name": "Away phone", "paired_at": NOW - 9 * 86_400, "via": "cloudflare"}),
        ];
    });
    // The name leads to the new address on this computer (revision 12).
    t.host.with(|s| {
        s.names = vec![("moneykeep.home.arpa".into(), vec!["192.168.1.31".into()])];
    });
    let v = t.b.run(
        "capture.wifi_change_address",
        json!({"address": "192.168.1.31", "name": "moneykeep.home.arpa"}),
    );
    assert_eq!(v["warning"], text::CHANGE_WARNING);
    // mobile-capture.md revision 13 correction: the Outbox first, then the app again.
    assert_eq!(
        text::CHANGE_WARNING,
        "Before you change it, send everything in the phone's Outbox. Afterwards install the new certificate, open the new address on the phone, install the app again and pair it."
    );
    assert_eq!(v["stale_phones"], json!(["Old phone"]));
    assert_eq!(v["ca_created"], true);
    assert_eq!(v["host"], "moneykeep.home.arpa");
    let reqs = t.host.with(|s| s.wifi_installs.clone()).unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1].address, "192.168.1.31");
    assert_eq!(reqs[1].name, "moneykeep.home.arpa");
    assert!(!reqs[1].new_desktop_token, "the desktop connection is kept");
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["wifi"]["address"], "192.168.1.31");
    assert_ne!(
        s["wifi"]["fingerprint"],
        "A1B2 C3D4 E5F6 0718 293A 4B5C 6D7E 8F90"
    );
    let p = t.b.run("capture.wifi_pair_code", json!({}));
    assert_eq!(
        p["pair_url"],
        "https://moneykeep.home.arpa:14443/#pair=ABCD2345"
    );
}

/// Test 74 (command side): both ways in at once; the devices list carries each
/// phone's way in; the light names a lost address.
#[test]
fn at74_both_ways_and_devices() {
    let mut t = wifi_book();
    apply(&mut t);
    t.cf.with(|s| {
        s.devices = vec![
            json!({"id": "0123456789abcdef0123456789abcdef", "name": "Pixel", "paired_at": NOW, "via": "wifi"}),
            json!({"id": "fedcba9876543210fedcba9876543210", "name": "iPhone", "paired_at": NOW, "via": "cloudflare"}),
        ];
        s.wifi = Some(json!({"enabled": true, "address": "192.168.1.20", "address_present": false}));
    });
    let d = t.b.run("capture.devices", json!({}));
    assert_eq!(d["devices"][0]["via"], "wifi");
    assert_eq!(d["devices"][1]["via"], "cloudflare");
    let st = t.b.run("capture.status", json!({}));
    assert_eq!(st["state"], "yellow");
    assert_eq!(st["text"], text::address_gone("192.168.1.20"));
    t.cf.with(|s| {
        s.wifi = Some(json!({"enabled": true, "address": "192.168.1.20", "address_present": true}));
    });
    let st = t.b.run("capture.status", json!({}));
    assert_eq!(st["state"], "green", "{st}");
}

#[test]
fn wifi_commands_are_not_offered_over_mcp() {
    for c in [
        "capture.wifi_apply",
        "capture.wifi_set_pin",
        "capture.wifi_pair_code",
        "capture.wifi_change_address",
        "capture.wifi_remove",
    ] {
        assert!(crate::NOT_OVER_MCP.contains(&c), "{c}");
    }
}

/// mobile-capture.md revision 12, test 77 (command side): with an empty name the
/// certificate and both QR addresses use the address alone, and nothing is looked
/// up.
#[test]
fn r12_at77_blank_name_uses_the_address() {
    let mut t = wifi_book();
    let v = t.b.run(
        "capture.wifi_apply",
        json!({"address": "192.168.1.20", "name": "", "pin": "246813"}),
    );
    assert_eq!(v["host"], "192.168.1.20");
    assert_eq!(v["cert_url"], "http://192.168.1.20:14080/");
    assert_eq!(v["app_url"], "https://192.168.1.20:14443/");
    let req = t.host.with(|s| s.wifi_installs[0].clone()).unwrap();
    assert_eq!(
        req.name, "",
        "the certificate is made for the address alone"
    );
    assert!(t.host.with(|s| s.lookups.is_empty()).unwrap());
    let p = t.b.run("capture.wifi_pair_code", json!({}));
    assert_eq!(p["cert_url"], "http://192.168.1.20:14080/");
    assert_eq!(p["pair_url"], "https://192.168.1.20:14443/#pair=ABCD2345");
}

/// Revision 12, test 78 (command side): a name that does not lead to the address
/// is refused before anything is changed; Use it anyway (`name_anyway`) goes on; a
/// name that leads to the address goes on without asking. Change address the same.
#[test]
fn r12_at78_name_must_lead_to_the_address() {
    let mut t = wifi_book();
    t.host.with(|s| {
        s.names = vec![
            ("elsewhere.home.arpa".into(), vec!["192.168.1.99".into()]),
            ("moneykeep.home.arpa".into(), vec!["192.168.1.20".into()]),
        ];
    });
    for name in ["nowhere.home.arpa", "elsewhere.home.arpa"] {
        let e = t.b.err(
            "capture.wifi_apply",
            json!({"address": "192.168.1.20", "name": name, "pin": "246813"}),
        );
        assert_eq!(e.code(), "name_unresolved", "{name}: {e}");
        assert_eq!(
            e.to_string(),
            text::name_unresolved("192.168.1.20"),
            "{name}"
        );
    }
    assert_eq!(
        text::name_unresolved("192.168.1.20"),
        "This name does not lead to 192.168.1.20 on this computer. Leave it blank to use the address."
    );
    // Nothing was changed: no install, nothing recorded.
    assert!(t.host.with(|s| s.wifi_installs.is_empty()).unwrap());
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["wifi"]["started"], false);
    // Use it anyway.
    let v = t.b.run(
        "capture.wifi_apply",
        json!({"address": "192.168.1.20", "name": "nowhere.home.arpa", "pin": "246813", "name_anyway": true}),
    );
    assert_eq!(v["host"], "nowhere.home.arpa");
    // A name that leads to the address needs no question.
    let v = t.b.run(
        "capture.wifi_apply",
        json!({"address": "192.168.1.20", "name": "MoneyKeep.home.arpa"}),
    );
    assert_eq!(v["host"], "moneykeep.home.arpa");
    // Change address checks the same way; clearing the name needs no lookup and
    // makes the certificate for the address alone.
    let e = t.b.err(
        "capture.wifi_change_address",
        json!({"address": "192.168.1.20", "name": "elsewhere.home.arpa"}),
    );
    assert_eq!(e.code(), "name_unresolved");
    let before = t.host.with(|s| s.lookups.len()).unwrap();
    let v = t.b.run(
        "capture.wifi_change_address",
        json!({"address": "192.168.1.20", "name": ""}),
    );
    assert_eq!(v["host"], "192.168.1.20");
    assert_eq!(t.host.with(|s| s.lookups.len()).unwrap(), before);
    let req = t
        .host
        .with(|s| s.wifi_installs.last().cloned())
        .unwrap()
        .unwrap();
    assert_eq!(req.name, "");
}

/// mobile-capture.md test 106 (command side, a fake for the firewall calls): a
/// missing rule gives the Fix action and Fix makes the rule again with the right
/// ports and profiles; a Block rule for moneykeep-server.exe is reported and removed
/// by Fix while an Allow rule for it is left alone; a Public network gives the
/// Public message with the link to Windows' network settings.
#[test]
fn t106_firewall_check_and_fix() {
    use crate::capture_setup::FoundRule;
    let mut t = wifi_book();
    // Before Wi-Fi setup: nothing to check.
    let v = t.b.run("capture.wifi_firewall_check", json!({}));
    assert_eq!(v["wifi"], false);
    apply(&mut t);
    // All well.
    let v = t.b.run("capture.wifi_firewall_check", json!({}));
    assert_eq!(v["rule"], "ok");
    assert!(v["problems"].as_array().unwrap().is_empty(), "{v}");
    // Uninstalled and installed again: the rule is gone; Windows made its own.
    let exe = r"C:\Program Files\MoneyKeep\moneykeep-server.exe";
    t.host.with(|s| {
        s.firewall_rules.clear();
        s.windows_rules = vec![
            FoundRule {
                id: "{block}".into(),
                display_name: "moneykeep-server.exe".into(),
                enabled: true,
                action: "Block".into(),
                direction: "Inbound".into(),
                program: exe.into(),
            },
            FoundRule {
                id: "{allow}".into(),
                display_name: "moneykeep-server.exe".into(),
                enabled: true,
                action: "Allow".into(),
                direction: "Inbound".into(),
                program: exe.into(),
            },
        ];
        s.public_networks = vec!["Cafe Wi-Fi".into()];
    });
    let v = t.b.run("capture.wifi_firewall_check", json!({}));
    assert_eq!(v["rule"], "missing");
    let problems = v["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 3, "{v}");
    assert_eq!(problems[0]["text"], text::RULE_MISSING);
    assert_eq!(
        text::RULE_MISSING,
        "Phones can't reach this computer: the firewall rule is missing."
    );
    assert_eq!(problems[0]["fix"], "rule");
    assert_eq!(
        problems[1]["text"],
        "Windows is blocking the capture server"
    );
    assert_eq!(problems[1]["fix"], "block");
    assert_eq!(
        problems[2]["text"],
        "Windows treats this network as public, so phones can't connect over Wi-Fi"
    );
    assert_eq!(problems[2]["link"], "ms-settings:network-status");
    assert!(
        problems[2].get("fix").is_none(),
        "MoneyKeep doesn't change it"
    );
    assert_eq!(
        v["block_rules"].as_array().unwrap().len(),
        1,
        "Allow is not listed"
    );
    // Declined permission: nothing changed, said so.
    t.host.with(|s| s.decline_firewall_fix = true);
    let e = t.b.err("capture.wifi_firewall_fix", json!({}));
    assert!(e.to_string().contains("permission was not given"), "{e}");
    assert!(t.host.with(|s| s.firewall_rules.is_empty()).unwrap());
    t.host.with(|s| s.decline_firewall_fix = false);
    // Fix: the rule again with the right ports and profiles, the Block rule gone,
    // the Allow rule kept.
    let v = t.b.run("capture.wifi_firewall_fix", json!({}));
    assert_eq!(v["fixed"], true);
    let fix = t
        .host
        .with(|s| s.firewall_fixes.last().cloned().unwrap())
        .unwrap();
    let rule = fix.add_rule.unwrap();
    assert_eq!(rule.name, "MoneyKeep capture (Wi-Fi)");
    assert_eq!(rule.ports, vec![14443, 14080]);
    assert_eq!(
        rule.profiles,
        vec!["Private".to_owned(), "Domain".to_owned()]
    );
    assert_eq!(fix.remove_block_rules, vec!["{block}".to_owned()]);
    let left: Vec<String> = t
        .host
        .with(|s| s.windows_rules.iter().map(|r| r.id.clone()).collect())
        .unwrap();
    assert_eq!(left, vec!["{allow}".to_owned()]);
    let v = t.b.run("capture.wifi_firewall_check", json!({}));
    let codes: Vec<&str> = v["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, vec!["public_network"]);
    // Only the rule (the question at start): the Block rules are not asked for.
    t.host.with(|s| s.rules_disabled = true);
    let v = t.b.run("capture.wifi_firewall_check", json!({}));
    assert_eq!(v["problems"][0]["text"], text::RULE_DISABLED);
    t.b.run("capture.wifi_firewall_fix", json!({"what": "rule"}));
    let fix = t
        .host
        .with(|s| s.firewall_fixes.last().cloned().unwrap())
        .unwrap();
    assert!(fix.add_rule.is_some() && fix.remove_block_rules.is_empty());
    // Nothing to fix: no permission prompt.
    let n = t.host.with(|s| s.firewall_fixes.len()).unwrap();
    let v = t.b.run("capture.wifi_firewall_fix", json!({}));
    assert_eq!(v["fixed"], false);
    assert_eq!(t.host.with(|s| s.firewall_fixes.len()).unwrap(), n);
    assert!(
        t.b.try_run("capture.wifi_firewall_fix", json!({"what": "all"}))
            .is_err()
    );
}

/// security-review-1 SR-A6 (command side): the service is gone but its data is
/// still on the computer. The status says the capture server isn't installed and
/// offers Install the server again (not Start); the firewall check shows no problem
/// and its Fix is refused; Install the server again asks the computer once, with
/// Wi-Fi capture's firewall rule and ports; a declined prompt or a failure is
/// said so. A stopped (installed) service offers Start instead.
#[test]
fn sr_a6_install_the_server_again() {
    use crate::capture_setup::text as setup_text;
    let mut t = wifi_book();
    apply(&mut t);
    // The owner's computer: no service, the data in the old folder, no rule.
    t.host.with(|s| {
        s.report.server = ServiceState::NotInstalled;
        s.report.data_found = true;
        s.firewall_rules.clear();
    });
    t.cf.with(|s| s.local_health = false);
    let v = t.b.run("capture.status", json!({}));
    assert_eq!(v["state"], "red");
    assert_eq!(v["text"], setup_text::NOT_INSTALLED);
    assert_eq!(
        setup_text::NOT_INSTALLED,
        "Phone capture isn't working: the capture server isn't installed"
    );
    assert_eq!(v["problems"][0], setup_text::NOT_INSTALLED);
    assert_eq!(v["problems"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["can_reinstall"], true);
    assert_eq!(v["can_start"], false);
    // No firewall banner while it isn't installed, and no Fix.
    let f = t.b.run("capture.wifi_firewall_check", json!({}));
    assert!(f["problems"].as_array().unwrap().is_empty(), "{f}");
    assert_eq!(f["server_installed"], false);
    let e =
        t.b.err("capture.wifi_firewall_fix", json!({"what": "rule"}));
    assert!(e.to_string().contains("Install the server again"), "{e}");
    assert!(t.host.with(|s| s.firewall_fixes.is_empty()).unwrap());
    // Declined, then failed: said so, nothing installed.
    t.host.with(|s| s.decline_reinstall = true);
    let e = t.b.err("capture.reinstall_server", json!({}));
    assert!(e.to_string().contains("permission was not given"), "{e}");
    t.host.with(|s| {
        s.decline_reinstall = false;
        s.fail_reinstall = Some("The capture server did not answer its health check.".into());
    });
    let e = t.b.err("capture.reinstall_server", json!({}));
    assert!(
        e.to_string()
            .contains("could not be installed again: The capture server did not answer"),
        "{e}"
    );
    assert_eq!(
        t.host.with(|s| s.report.server).unwrap(),
        ServiceState::NotInstalled
    );
    t.host.with(|s| s.fail_reinstall = None);
    // Install the server again: one request, with the Wi-Fi rule and its ports.
    let v = t.b.run("capture.reinstall_server", json!({}));
    assert_eq!(v["installed"], true, "{v}");
    let req = t
        .host
        .with(|s| s.reinstalls.last().cloned().unwrap())
        .unwrap();
    assert!(!req.allow_lan);
    let rule = req.firewall_rule.unwrap();
    assert_eq!(rule.name, "MoneyKeep capture (Wi-Fi)");
    assert_eq!(rule.ports, vec![14443, 14080]);
    assert_eq!(
        rule.profiles,
        vec!["Private".to_owned(), "Domain".to_owned()]
    );
    t.cf.with(|s| s.local_health = true);
    let v = t.b.run("capture.status", json!({}));
    assert_ne!(v["state"], "red", "{v}");
    assert_eq!(v["can_reinstall"], false);
    let f = t.b.run("capture.wifi_firewall_check", json!({}));
    assert_eq!(f["rule"], "ok", "{f}");
    // A stopped service: the server isn't running, with Start (no reinstall).
    t.host.with(|s| {
        s.report.server = ServiceState::Stopped;
        s.report.data_found = false;
    });
    t.cf.with(|s| s.local_health = false);
    let v = t.b.run("capture.status", json!({}));
    assert_eq!(v["text"], setup_text::RED);
    assert_eq!(v["can_start"], true);
    assert_eq!(v["can_reinstall"], false);
    t.b.run("capture.start_server", json!({}));
    assert_eq!(
        t.host.with(|s| s.report.server).unwrap(),
        ServiceState::Running
    );
    // Not installed and no data anywhere: as before (nothing to install again).
    t.host
        .with(|s| s.report.server = ServiceState::NotInstalled);
    let v = t.b.run("capture.status", json!({}));
    assert_eq!(v["text"], setup_text::RED);
    assert_eq!(v["can_reinstall"], false);
    // Not offered over MCP.
    assert!(crate::NOT_OVER_MCP.contains(&"capture.reinstall_server"));
}

/// macos-release.md C5 (command side): on a Mac with Wi-Fi capture, the firewall
/// check reports no Windows rule problems; when macOS's firewall is on it gives
/// the "may be blocking" problem with the link to Firewall settings.
#[test]
fn mac_c5_firewall_card() {
    let mut t = wifi_book();
    t.host.with(|s| {
        s.report.os = "macos".into();
        s.report.can_download_cloudflared = false;
    });
    apply(&mut t);
    let v = t.b.run("capture.wifi_firewall_check", json!({}));
    assert_eq!(v["managed"], false, "{v}");
    assert!(v["problems"].as_array().unwrap().is_empty(), "{v}");
    t.host.with(|s| s.app_firewall = Some(true));
    let v = t.b.run("capture.wifi_firewall_check", json!({}));
    let p = v["problems"].as_array().unwrap();
    assert_eq!(p.len(), 1, "{v}");
    assert_eq!(p[0]["code"], "macos_firewall");
    assert_eq!(p[0]["text"], text::MAC_FIREWALL);
    assert_eq!(
        p[0]["text"],
        "macOS's firewall may be blocking phone capture"
    );
    assert_eq!(p[0]["link"], text::MAC_FIREWALL_SETTINGS);
    assert_eq!(p[0]["link_label"], "Open Firewall settings");
    assert!(p[0].get("fix").is_none(), "no Fix on a Mac");
    t.host.with(|s| s.app_firewall = Some(false));
    let v = t.b.run("capture.wifi_firewall_check", json!({}));
    assert!(v["problems"].as_array().unwrap().is_empty(), "{v}");
}

/// macos-release.md C8: Remove phone capture from this Mac removes the agents
/// (the capture server's and cloudflared's), keeps the data folder unless asked,
/// and is refused on other systems.
#[test]
fn mac_c8_remove_from_this_mac() {
    use crate::capture_setup::text as st;
    let mut t = wifi_book();
    // Windows: refused, nothing removed.
    let e = t.b.err("capture.remove_from_computer", json!({}));
    assert_eq!(e.code(), "not_mac", "{e}");
    assert!(t.host.with(|s| s.start_overs.is_empty()).unwrap());
    t.host.with(|s| {
        s.report.os = "macos".into();
        s.report.can_download_cloudflared = false;
    });
    apply(&mut t);
    t.host.with(|s| {
        s.report.cloudflared_service = Some(ServiceState::Running);
        s.data_folder = true;
    });
    // Default: the data folder is kept.
    let v = t.b.run("capture.remove_from_computer", json!({}));
    assert_eq!(v["removed"], true, "{v}");
    assert_eq!(v["text"], st::MAC_REMOVED);
    assert_eq!(v["data_deleted"], false);
    let req = t
        .host
        .with(|s| s.start_overs.last().cloned())
        .unwrap()
        .unwrap();
    assert!(req.server && req.cloudflared && !req.data_folder, "{req:?}");
    assert!(req.firewall_rule.is_none() && req.cloudflared_tunnel.is_none());
    assert!(t.host.with(|s| s.data_folder).unwrap());
    assert_eq!(
        t.host.with(|s| s.report.server).unwrap(),
        ServiceState::NotInstalled
    );
    // Asked: the data folder goes too.
    let v =
        t.b.run("capture.remove_from_computer", json!({"delete_data": true}));
    assert_eq!(v["data_deleted"], true);
    let req = t
        .host
        .with(|s| s.start_overs.last().cloned())
        .unwrap()
        .unwrap();
    assert!(req.data_folder && !req.cloudflared, "{req:?}");
    assert!(!t.host.with(|s| s.data_folder).unwrap());
    // Only for local front ends.
    assert!(crate::NOT_OVER_MCP.contains(&"capture.remove_from_computer"));
}
