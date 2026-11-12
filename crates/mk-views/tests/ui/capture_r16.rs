//! mobile-capture.md revision 16 tests 88 and 91 in the app: the Install step's
//! "Replace it" / "Leave it" for a cloudflared service of a deleted tunnel, and a
//! removal that did not finish with Retry undo. Against the scripted Cloudflare API
//! and computer of `mk_commands::capture_setup::testing`: no network, nothing
//! installed.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::common::*;
use mk_commands::Session;
use mk_commands::capture_setup::testing::{FakeCloudflare, FakeHost, TEST_TOKEN};
use mk_commands::capture_setup::{PERMISSIONS, ServiceState, text};
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

fn start(set_up: bool, leftover: bool) -> T {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("p.moneykeep"), "Phone Books", "USD", "app").unwrap();
    let cf = FakeCloudflare::new();
    let host = FakeHost::windows();
    if leftover {
        host.with(|s| {
            s.report.cloudflared_installed = true;
            s.report.cloudflared_service = Some(ServiceState::Running);
            s.report.cloudflared_tunnel = Some("0d5c0a2e-6f1b-4c3d-9e8f-112233445566".into());
            s.report.cloudflared_account = Some("acc1".into());
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

fn open_advanced(h: &mut H) {
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Advanced").click();
    h.run_steps(3);
}

fn checked(h: &H, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().toggled() == Some(Toggled::True)
}

/// Walk the setup dialog to its Install step (a new tunnel).
fn to_install(t: &mut T) {
    open_advanced(&mut t.h);
    press(&mut t.h, "Set up phone capture over Cloudflare…");
    press(&mut t.h, "Next");
    type_field(&mut t.h, "Cloudflare API token", TEST_TOKEN);
    press(&mut t.h, "Check token");
    wait_for(&mut t.h, "This token has every permission needed.");
    press(&mut t.h, "Next");
    type_field(&mut t.h, "Email addresses allowed", "owner@example.com");
    press(&mut t.h, "Next");
    // The service runs a deleted tunnel: a new tunnel is suggested.
    assert!(checked(&t.h, "Create a new tunnel on this computer"));
    press(&mut t.h, "Next");
}

/// Test 88: the replace message, Replace it preselected and Install available;
/// Leave it keeps Install unavailable with the reason; Install replaces.
#[test]
fn r16_t88_replace_or_leave_at_install() {
    let mut t = start(false, true);
    to_install(&mut t);
    wait_for(&mut t.h, text::REPLACE_MESSAGE);
    assert!(checked(&t.h, "Replace it"));
    assert!(!checked(&t.h, "Leave it"));
    assert!(shows(&t.h, text::REPLACE_STEP));
    assert!(!disabled(&t.h, "Install"));
    // Leave it: Install unavailable, with the reason.
    t.h.get_by_label("Leave it").click();
    wait_for(&mut t.h, text::LEAVE_REASON);
    assert!(checked(&t.h, "Leave it"));
    assert!(disabled(&t.h, "Install"));
    assert!(!shows(&t.h, text::REPLACE_STEP));
    // Back to Replace it, then Install.
    t.h.get_by_label("Replace it").click();
    for _ in 0..200 {
        t.h.run_steps(2);
        if !shows(&t.h, text::LEAVE_REASON) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    assert!(!shows(&t.h, text::LEAVE_REASON));
    assert!(!disabled(&t.h, "Install"));
    press(&mut t.h, "Install");
    wait_for(&mut t.h, "Phone capture status: Phone capture is working");
    let installs = t.host.with(|s| s.installs.clone()).unwrap();
    assert_eq!(installs.len(), 1);
    assert_eq!(
        installs[0].replace_cloudflared_tunnel.as_deref(),
        Some("0d5c0a2e-6f1b-4c3d-9e8f-112233445566")
    );
}

/// Test 91: a removal step that fails is listed with its reason and where to
/// remove it by hand, with Retry undo; retrying finishes the removal.
#[test]
fn r16_t91_retry_undo() {
    let mut t = start(true, false);
    t.cf.with(|s| {
        s.granted.remove(PERMISSIONS[4]);
    });
    open_advanced(&mut t.h);
    press(&mut t.h, "Remove phone capture over Cloudflare…");
    press(&mut t.h, "Remove");
    wait_for(&mut t.h, text::UNDO_INCOMPLETE);
    assert!(shows(
        &t.h,
        "The DNS record capture.example.com — Cloudflare: Authentication error"
    ));
    assert!(shows(
        &t.h,
        "Cloudflare dashboard: your domain > DNS > Records"
    ));
    assert!(!shows(&t.h, "Phone capture was removed."));
    assert!(has(&t.h, "Remove phone capture over Cloudflare…"));
    t.cf.with(|s| s.granted = PERMISSIONS.iter().copied().collect());
    press(&mut t.h, "Retry undo");
    wait_for(&mut t.h, "Phone capture was removed.");
    assert!(!shows(&t.h, text::UNDO_INCOMPLETE));
    assert!(t.cf.with(|s| s.dns.is_empty()).unwrap());
    assert!(has(&t.h, "Set up phone capture over Cloudflare…"));
}
