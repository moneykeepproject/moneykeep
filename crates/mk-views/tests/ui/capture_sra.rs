//! security-review-1 A1 in the app: after the capture server's data moved to its
//! protected folder, Settings → Phone capture's Status card offers "Remove the old
//! copy" (for 30 days), which removes it; A6: when the capture service is missing
//! with its data still there, "Install the server again", and "Start the server"
//! when it is stopped. Against the scripted Cloudflare API and
//! computer of `mk_commands::capture_setup::testing`: no network, nothing installed.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::common::*;
use mk_commands::Session;
use mk_commands::capture_setup::testing::{FakeCloudflare, FakeHost, TEST_TOKEN};
use mk_commands::capture_setup::{OldCopy, ServiceState};
use mk_commands::connect::{Connector, MemorySecrets};
use mk_ui::UserConfig;
use mk_ui::ctl::App;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

const NOW: i64 = 1_790_812_800;
const OLD: &str = r"C:\Users\Pat\AppData\Local\MoneyKeep\data\capture-server";

fn shows(h: &H, part: &str) -> bool {
    labels_containing(h, "").iter().any(|l| l.contains(part))
}

fn wait_for(h: &mut H, part: &str, present: bool) {
    for _ in 0..400 {
        h.run_steps(2);
        if shows(h, part) == present {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    panic!(
        "{part:?} never {}: {:#?}",
        if present { "showed" } else { "went" },
        labels_containing(h, "")
    );
}

#[test]
fn sr_a1_remove_the_old_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("p.moneykeep"), "Phone Books", "USD", "app").unwrap();
    let cf = FakeCloudflare::new();
    let host = FakeHost::windows();
    s.set_connector(Connector {
        fetcher: Arc::new(cf.clone()),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(NOW),
        host: Some(Arc::new(host.clone())),
    });
    s.invoke("module.enable", json!({"module": "expenses"}))
        .unwrap();
    s.invoke("capture.setup_check_token", json!({"token": TEST_TOKEN}))
        .unwrap();
    s.invoke(
        "capture.setup_apply",
        json!({"emails": ["owner@example.com"]}),
    )
    .unwrap();
    host.with(|st| {
        st.report.old_copy = Some(OldCopy {
            path: OLD.into(),
            moved_at: NOW - 86_400,
        });
    });
    let app = App::new(
        Box::new(LocalBackend {
            session: Some(s),
            name: Some("Phone Books".into()),
        }),
        UserConfig::default(),
    );
    let mut h = harness(app);
    h.run();
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Phone capture").click();
    h.run_steps(3);
    wait_for(&mut h, "A copy of it is still in your user folder", true);
    assert!(shows(&h, OLD));
    h.get_by_role_and_label(Role::Button, "Remove the old copy")
        .click();
    h.run_steps(3);
    wait_for(&mut h, "A copy of it is still in your user folder", false);
    assert!(host.with(|st| st.report.old_copy.is_none()).unwrap());
}

fn has_button(h: &H, label: &str) -> bool {
    h.query_all_by_label(label).count() > 0
}

fn press(h: &mut H, label: &str) {
    h.get_by_role_and_label(Role::Button, label).click();
    h.run_steps(3);
}

/// security-review-1 SR-A6 in the app: Wi-Fi capture was set up, then the capture
/// service went away with its data left in the user's old folder. The Status card
/// says the capture server isn't installed, offers Install the server again (and no
/// firewall banner or Fix); a failure is shown on the card; Install the server
/// again recreates the firewall rule and the light comes back. A stopped service
/// offers Start the server, which starts it.
#[test]
fn sr_a6_install_the_server_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("p.moneykeep"), "Phone Books", "USD", "app").unwrap();
    let cf = FakeCloudflare::new();
    let host = FakeHost::windows();
    host.with(|st| {
        st.addresses = vec![mk_commands::capture_setup::LocalAddress {
            address: "192.168.1.20".into(),
            adapter: "Wi-Fi".into(),
            mac: "a4:b1:c2:d3:e4:f5".into(),
            profile: "Private".into(),
            dhcp: true,
        }];
    });
    s.set_connector(Connector {
        fetcher: Arc::new(cf.clone()),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(NOW),
        host: Some(Arc::new(host.clone())),
    });
    s.invoke("module.enable", json!({"module": "expenses"}))
        .unwrap();
    s.invoke(
        "capture.wifi_apply",
        json!({"address": "192.168.1.20", "pin": "246813"}),
    )
    .unwrap();
    // The owner's computer: no service, the data in the old folder, no rule.
    cf.with(|c| c.local_health = false);
    host.with(|st| {
        st.report.server = ServiceState::NotInstalled;
        st.report.data_found = true;
        st.firewall_rules.clear();
        st.fail_reinstall = Some("The capture server did not answer its health check.".into());
    });
    let app = App::new(
        Box::new(LocalBackend {
            session: Some(s),
            name: Some("Phone Books".into()),
        }),
        UserConfig::default(),
    );
    let mut h = harness(app);
    h.run();
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Phone capture").click();
    h.run_steps(3);
    press(&mut h, "Check now");
    wait_for(
        &mut h,
        "Phone capture isn't working: the capture server isn't installed",
        true,
    );
    assert!(has_button(&h, "Install the server again"));
    assert!(!has_button(&h, "Start the server"));
    h.run_steps(10);
    assert!(
        !shows(&h, "the firewall rule is missing") && !has_button(&h, "Fix"),
        "no firewall banner while the server isn't installed: {:#?}",
        labels_containing(&h, "")
    );
    // A failure is shown on the card; nothing was installed.
    press(&mut h, "Install the server again");
    wait_for(&mut h, "could not be installed again", true);
    assert_eq!(
        host.with(|st| st.report.server).unwrap(),
        ServiceState::NotInstalled
    );
    // Install the server again: the rule comes back, the light too.
    host.with(|st| st.fail_reinstall = None);
    cf.with(|c| c.local_health = true);
    press(&mut h, "Install the server again");
    wait_for(
        &mut h,
        "The capture server was installed again and is running.",
        true,
    );
    let req = host
        .with(|st| st.reinstalls.last().cloned().unwrap())
        .unwrap();
    assert_eq!(req.firewall_rule.unwrap().ports, vec![14443, 14080]);
    assert_eq!(host.with(|st| st.firewall_rules.len()).unwrap(), 1);
    wait_for(&mut h, "the capture server isn't installed", false);
    assert!(!has_button(&h, "Install the server again"));
    // Installed but stopped: Start the server.
    cf.with(|c| c.local_health = false);
    host.with(|st| st.report.server = ServiceState::Stopped);
    press(&mut h, "Check now");
    wait_for(&mut h, "the server isn't running", true);
    assert!(has_button(&h, "Start the server"));
    assert!(!has_button(&h, "Install the server again"));
    cf.with(|c| c.local_health = true);
    press(&mut h, "Start the server");
    wait_for(&mut h, "The capture server was started.", true);
    assert_eq!(host.with(|st| st.starts).unwrap(), 1);
    assert_eq!(
        host.with(|st| st.report.server).unwrap(),
        ServiceState::Running
    );
}
