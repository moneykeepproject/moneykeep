//! upgrade-resilience-1 (Batch U1) in the app: Settings → Phone capture's Status
//! card after an upgrade went wrong. UR-4: a server that refused to start (data made
//! by a newer MoneyKeep) says so, and one whose API level is too low says "Update
//! the capture server"; U1.2: damaged Wi-Fi certificate files offer Make a new
//! certificate; UR-11 (U1.9): Check now collects first, so a stale collection error
//! clears at once, and a successful Start the server starts a collection. Against
//! the scripted Cloudflare API and computer of `mk_commands::capture_setup::testing`:
//! no network, nothing installed.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::common::*;
use mk_commands::Session;
use mk_commands::capture_setup::ServiceState;
use mk_commands::capture_setup::testing::{FakeCloudflare, FakeHost};
use mk_commands::connect::{Connector, MemorySecrets};
use mk_ui::UserConfig;
use mk_ui::ctl::App;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

const NOW: i64 = 1_790_812_800;

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

fn press(h: &mut H, label: &str) {
    h.get_by_role_and_label(Role::Button, label).click();
    h.run_steps(3);
}

/// Wi-Fi capture set up on the owner's Windows computer.
fn setup(dir: &std::path::Path) -> (Session, FakeCloudflare, FakeHost) {
    let mut s = Session::create(&dir.join("p.moneykeep"), "Phone Books", "USD", "app").unwrap();
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
    (s, cf, host)
}

fn open(s: Session) -> H {
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
    h
}

fn inbox_calls(cf: &FakeCloudflare) -> usize {
    cf.with(|c| {
        c.calls
            .iter()
            .filter(|k| k.path.ends_with("/desktop/inbox"))
            .count()
    })
    .unwrap()
}

/// UR-4 (desktop part): a refusal is the status, with the server's message and
/// that downgrades aren't supported; a server below the API level this desktop
/// needs says "Update the capture server".
#[test]
fn ur4_refusal_and_update_server() {
    let dir = tempfile::tempdir().unwrap();
    let (s, cf, host) = setup(dir.path());
    cf.with(|c| c.local_health = false);
    host.with(|st| {
        st.report.server = ServiceState::Stopped;
        st.report.refusal = Some("This phone capture data was made by a newer MoneyKeep. Install that version or a newer one.".into());
        st.report.refusal_kind = Some("newer_format".into());
    });
    let mut h = open(s);
    press(&mut h, "Check now");
    wait_for(&mut h, "the capture server refused to start", true);
    assert!(shows(&h, "made by a newer MoneyKeep"));
    assert!(shows(
        &h,
        "Going back to an earlier version of MoneyKeep isn't supported"
    ));
    assert_eq!(
        h.query_all_by_label("Start the server").count(),
        0,
        "starting again would only refuse again"
    );
    // The server runs again, but is older than this desktop needs.
    host.with(|st| {
        st.report.server = ServiceState::Running;
        st.report.refusal = None;
        st.report.refusal_kind = None;
    });
    cf.with(|c| {
        c.local_health = true;
        c.server_api_level = 0;
    });
    press(&mut h, "Check now");
    wait_for(&mut h, "Update the capture server", true);
    assert!(shows(&h, "older than this MoneyKeep needs"));
}

/// U1.2: damaged Wi-Fi certificate files: the card says so and offers Make a new
/// certificate, which runs Wi-Fi setup again for the same address.
#[test]
fn ur2_damaged_wifi_certificate_offers_a_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let (s, cf, host) = setup(dir.path());
    cf.with(|c| {
        c.wifi_error = Some("Wi-Fi capture can't start: its certificate files are damaged".into());
    });
    let before = host.with(|st| st.wifi_installs.len()).unwrap();
    let mut h = open(s);
    press(&mut h, "Check now");
    wait_for(&mut h, "its certificate files are damaged", true);
    assert!(shows(
        &h,
        "every phone will then need the certificate installed again"
    ));
    cf.with(|c| c.wifi_error = None);
    press(&mut h, "Make a new certificate");
    wait_for(&mut h, "A new certificate was made for Wi-Fi capture", true);
    let reqs = host.with(|st| st.wifi_installs.clone()).unwrap();
    assert_eq!(reqs.len(), before + 1);
    assert_eq!(reqs.last().unwrap().address, "192.168.1.20");
}

/// UR-11 (U1.9): a stored collection error and a server that works again: Check
/// now collects first and the card turns green; an error while the server answers
/// is a note with its time, not the status; a successful Start the server starts a
/// collection at once.
#[test]
fn ur11_stale_collection_error() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, cf, host) = setup(dir.path());
    // The collection failed while the server was down (e.g. during an upgrade).
    cf.with(|c| c.local_health = false);
    assert!(s.invoke("capture.collect", json!({})).is_err());
    let mut h = open(s);
    wait_for(&mut h, "the server isn't running", true);
    // The server is back: Check now collects, and nothing stale is left.
    cf.with(|c| c.local_health = true);
    let calls = inbox_calls(&cf);
    press(&mut h, "Check now");
    wait_for(&mut h, "Phone capture is working", true);
    assert!(inbox_calls(&cf) > calls, "Check now collected first");
    assert!(!shows(&h, "Collecting from the server failed"));
    // A collection that fails while the server answers: a note with its time.
    cf.with(|c| c.refuse_desktop_token = true);
    press(&mut h, "Check now");
    wait_for(&mut h, "Collecting from the server failed at 00:00", true);
    assert!(
        shows(&h, "Phone capture is working"),
        "{:#?}",
        labels_containing(&h, "")
    );
    cf.with(|c| c.refuse_desktop_token = false);
    // Stopped, then started: a collection follows at once.
    cf.with(|c| c.local_health = false);
    host.with(|st| st.report.server = ServiceState::Stopped);
    press(&mut h, "Check now");
    wait_for(&mut h, "the server isn't running", true);
    cf.with(|c| c.local_health = true);
    let calls = inbox_calls(&cf);
    press(&mut h, "Start the server");
    wait_for(&mut h, "The capture server was started.", true);
    for _ in 0..200 {
        h.run_steps(2);
        if inbox_calls(&cf) > calls {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    assert!(
        inbox_calls(&cf) > calls,
        "a collection started after Start the server"
    );
}
