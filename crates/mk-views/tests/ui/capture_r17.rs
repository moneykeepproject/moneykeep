//! mobile-capture.md revision 17 tests 92, 95 and 97 in the app (Settings → Phone
//! capture → Start over…, and "Stuck? Start over…" on a setup failure screen), and
//! revision 16a's choice at the Install step. Against the scripted Cloudflare API
//! and computer of `mk_commands::capture_setup::testing`: no network, nothing
//! installed.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::common::*;
use mk_commands::Session;
use mk_commands::capture_setup::testing::{FakeCloudflare, FakeHost, TEST_TOKEN};
use mk_commands::capture_setup::{ServiceState, text};
use mk_commands::capture_start_over::text as so;
use mk_commands::connect::{Connector, MemorySecrets};
use mk_ui::UserConfig;
use mk_ui::ctl::App;
use mk_views::testing::{Role, Toggled};
use serde_json::json;

type H = mk_views::testing::Harness;

struct T {
    h: H,
    cf: FakeCloudflare,
    host: FakeHost,
    _dir: tempfile::TempDir,
}

fn start(set_up: bool, other_account: bool) -> T {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("p.moneykeep"), "Phone Books", "USD", "app").unwrap();
    let cf = FakeCloudflare::new();
    let host = FakeHost::windows();
    if other_account {
        host.with(|s| {
            s.report.cloudflared_installed = true;
            s.report.cloudflared_service = Some(ServiceState::Running);
            s.report.cloudflared_tunnel = Some("0d5c0a2e-6f1b-4c3d-9e8f-112233445566".into());
            s.report.cloudflared_account = Some("other-account".into());
        });
    }
    s.set_connector(Connector {
        fetcher: Arc::new(cf.clone()),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(1_790_812_800),
        host: Some(Arc::new(host.clone())),
    });
    s.invoke("module.enable", json!({"module": "expenses"}))
        .unwrap();
    if set_up {
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
            name: Some("Phone Books".into()),
        }),
        UserConfig::default(),
    );
    let mut h = harness(app);
    h.run();
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

fn button_disabled(h: &H, label: &str) -> bool {
    h.get_by_role_and_label(Role::Button, label)
        .accesskit_node()
        .is_disabled()
}

fn type_field(h: &mut H, label: &str, text: &str) {
    let role = if label.contains("token") {
        Role::PasswordInput
    } else {
        Role::TextInput
    };
    h.get_by_role_and_label(role, label).focus();
    h.run();
    h.get_by_role_and_label(role, label).type_text(text);
    h.run_steps(2);
}

fn open_section(h: &mut H, tab: &str) {
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, tab).click();
    h.run_steps(3);
}

/// Tests 92 and 97: the preview in three groups and Kept, Start over enabled only
/// after typing START OVER; a declined permission prompt leaves the computer part
/// with Try again, which finishes it; then Set up phone capture.
#[test]
fn r17_t92_t97_start_over_in_the_app() {
    let mut t = start(true, false);
    t.host.with(|s| s.decline_start_over = true);
    open_section(&mut t.h, "Phone capture");
    press(&mut t.h, "Start over…");
    wait_for(&mut t.h, "Kept");
    for part in [
        "In Cloudflare",
        "The DNS record capture.example.com — Cloudflare dashboard: your domain > DNS > Records",
        "On this computer",
        "The capture server service",
        so::PERMISSION,
        "In MoneyKeep",
        "The saved Cloudflare API token",
        "Your books",
        "Your backups",
    ] {
        assert!(
            shows(&t.h, part),
            "{part}: {:#?}",
            labels_containing(&t.h, "")
        );
    }
    assert!(button_disabled(&t.h, "Start over"));
    type_field(&mut t.h, "Type START OVER to confirm", "start over");
    assert!(!button_disabled(&t.h, "Start over"));
    assert!(
        t.cf.with(|s| !s.dns.is_empty()).unwrap(),
        "nothing changed yet"
    );
    press(&mut t.h, "Start over");
    wait_for(&mut t.h, so::LEFT);
    assert!(shows(&t.h, so::DECLINED));
    assert!(t.cf.with(|s| s.dns.is_empty()).unwrap());
    press(&mut t.h, "Try again");
    t.host.with(|s| s.decline_start_over = false);
    for _ in 0..3 {
        // The first Try again was still declined (the switch flipped after it).
        if shows(&t.h, so::CLEARED) {
            break;
        }
        wait_for(&mut t.h, "Try again");
        press(&mut t.h, "Try again");
        for _ in 0..100 {
            t.h.run_steps(2);
            if shows(&t.h, so::CLEARED) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }
    assert!(
        shows(&t.h, so::CLEARED),
        "{:#?}",
        labels_containing(&t.h, "")
    );
    assert_eq!(
        t.host.with(|s| s.report.server).unwrap(),
        ServiceState::NotInstalled
    );
    press(&mut t.h, "Set up phone capture");
    assert!(!shows(&t.h, so::CLEARED));
}

/// Test 95: the uncollected count with Collect now, which collects them and
/// counts again.
#[test]
fn r17_t95_collect_now_before_starting_over() {
    let mut t = start(true, false);
    t.cf.with(|s| {
        s.inbox = (1..=3u8)
            .map(|n| {
                json!({"id": format!("{n:032x}"), "client_id": format!("c{n}"),
                       "device_id": format!("{:032x}", 9), "device_name": "Pixel",
                       "submitted_at": 1_790_800_000,
                       "fields": {"date": "2026-10-01", "merchant": "Cafe", "amount": "4.50"},
                       "receipts": []})
            })
            .collect();
    });
    open_section(&mut t.h, "Phone capture");
    press(&mut t.h, "Start over…");
    wait_for(&mut t.h, &so::uncollected(3));
    press(&mut t.h, "Collect now");
    for _ in 0..400 {
        t.h.run_steps(2);
        if !shows(&t.h, &so::uncollected(3)) && shows(&t.h, "Kept") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    assert!(!shows(&t.h, "not yet collected"));
    assert!(t.cf.with(|s| s.inbox.is_empty()).unwrap());
}

/// A setup failure screen offers "Stuck? Start over…".
#[test]
fn r17_stuck_start_over_from_a_failed_setup() {
    let mut t = start(false, false);
    open_section(&mut t.h, "Advanced");
    press(&mut t.h, "Set up phone capture over Cloudflare…");
    press(&mut t.h, "Next");
    type_field(
        &mut t.h,
        "Cloudflare API token",
        "wrong-token-0123456789abcdef",
    );
    press(&mut t.h, "Check token");
    wait_for(&mut t.h, "Cloudflare did not accept this token");
    press(&mut t.h, "Stuck? Start over…");
    wait_for(&mut t.h, "Kept");
    assert!(shows(&t.h, so::NOTHING_CLOUDFLARE));
}

/// Test 89a in the app: the other-account message with Leave it preselected
/// (Install unavailable), Replace it with the warning.
#[test]
fn r16a_t89a_other_account_leave_it_preselected() {
    let mut t = start(false, true);
    open_section(&mut t.h, "Advanced");
    press(&mut t.h, "Set up phone capture over Cloudflare…");
    press(&mut t.h, "Next");
    type_field(&mut t.h, "Cloudflare API token", TEST_TOKEN);
    press(&mut t.h, "Check token");
    wait_for(&mut t.h, "This token has every permission needed.");
    press(&mut t.h, "Next");
    type_field(&mut t.h, "Email addresses allowed", "owner@example.com");
    press(&mut t.h, "Next");
    press(&mut t.h, "Next");
    wait_for(&mut t.h, text::OTHER_ACCOUNT_MESSAGE);
    let checked =
        |h: &H, l: &str| h.get_by_label(l).accesskit_node().toggled() == Some(Toggled::True);
    assert!(checked(&t.h, "Leave it"));
    assert!(!checked(&t.h, "Replace it"));
    assert!(shows(&t.h, text::OTHER_ACCOUNT_WARNING));
    assert!(shows(&t.h, text::OTHER_ACCOUNT_LEAVE_REASON));
    assert!(button_disabled(&t.h, "Install"));
    t.h.get_by_label("Replace it").click();
    for _ in 0..200 {
        t.h.run_steps(2);
        if !shows(&t.h, text::OTHER_ACCOUNT_LEAVE_REASON) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    assert!(checked(&t.h, "Replace it"));
    assert!(!button_disabled(&t.h, "Install"));
}
