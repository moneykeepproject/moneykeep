//! app-desktop.md revision 29 acceptance tests 180 and 181 (mobile-capture.md
//! revision 2): the phone capture status light and the setup dialog, against the
//! scripted Cloudflare API, capture server and computer of
//! `mk_commands::capture_setup::testing`. Nothing touches the network or installs
//! anything.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::common::*;
use mk_commands::Session;
use mk_commands::capture_setup::ServiceState;
use mk_commands::capture_setup::testing::{FakeCloudflare, FakeHost, TEST_TOKEN};
use mk_commands::connect::{Connector, MemorySecrets};
use mk_ui::ctl::App;
use mk_ui::{AppEvent, UserConfig};
use mk_views::testing::{Role, Toggled};
use serde_json::json;

type H = mk_views::testing::Harness;

struct T {
    h: H,
    cf: FakeCloudflare,
    host: FakeHost,
    secrets: MemorySecrets,
    _dir: tempfile::TempDir,
}

fn start(set_up: bool) -> T {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("p.moneykeep"), "Phone Books", "USD", "app").unwrap();
    let cf = FakeCloudflare::new();
    let host = FakeHost::windows();
    let secrets = MemorySecrets::new(true);
    s.set_connector(Connector {
        fetcher: Arc::new(cf.clone()),
        secrets: Box::new(secrets.clone()),
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
        secrets,
        _dir: dir,
    }
}

fn shows(h: &H, part: &str) -> bool {
    labels_containing(h, "").iter().any(|l| l.contains(part))
}

/// Run frames until a node whose label contains `part` shows (background jobs).
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

/// Settings > Advanced: phone capture over Cloudflare (revision 10).
fn open_advanced(h: &mut H) {
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Advanced").click();
    h.run_steps(3);
}

fn open_phone_capture(h: &mut H) {
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Phone capture").click();
    h.run_steps(3);
}

const GREEN: &str = "Phone capture status: Phone capture is working";

/// AT180: the light shows each state with its accessible name and tooltip; it sits
/// in the top bar while phone capture is set up and opens Settings → Phone capture.
#[test]
fn app_desktop_at180_status_light_states() {
    // Not set up: grey in Settings, nothing in the top bar.
    let mut t = start(false);
    open_phone_capture(&mut t.h);
    assert!(has(&t.h, "Phone capture status: Phone capture is off"));
    assert_eq!(
        t.h.query_all_by_label("Phone capture status: Phone capture is off")
            .count(),
        1
    );
    assert!(has(&t.h, "Set up Wi-Fi capture…"));
    open_advanced(&mut t.h);
    assert!(has(&t.h, "Set up phone capture over Cloudflare…"));

    // Set up: green, in the top bar and in Settings.
    let mut t = start(true);
    wait_for(&mut t.h, GREEN);
    assert_eq!(t.h.query_all_by_label(GREEN).count(), 1, "top bar only");
    // The tooltip names the state, the last activity, items waiting and the check.
    t.h.get_by_label(GREEN).hover();
    t.h.run_steps(6);
    assert!(
        shows(&t.h, "Last phone activity:"),
        "{:#?}",
        labels_containing(&t.h, "")
    );
    assert!(shows(&t.h, "Waiting to be collected: 0"));
    assert!(shows(&t.h, "Last checked:"));
    // Clicking it opens Settings → Phone capture.
    t.h.get_by_label(GREEN).click();
    t.h.run_steps(3);
    assert!(has(&t.h, "Check now"));
    assert_eq!(
        t.h.query_all_by_label(GREEN).count(),
        2,
        "top bar and Settings"
    );

    // The public address fails: yellow with the tunnel text.
    t.cf.with(|s| s.public_health = false);
    press(&mut t.h, "Check now");
    let yellow =
        "Phone capture status: The internet address isn't reaching your computer (tunnel down?)";
    wait_for(&mut t.h, yellow);
    assert_eq!(
        t.h.query_all_by_label(yellow).count(),
        2,
        "top bar and Settings"
    );

    // The server stops: red, with Start server.
    t.cf.with(|s| {
        s.public_health = true;
        s.local_health = false;
    });
    t.host.with(|s| s.report.server = ServiceState::Stopped);
    press(&mut t.h, "Check now");
    let red = "Phone capture status: Phone capture isn't working: the server isn't running";
    wait_for(&mut t.h, red);
    wait_for(&mut t.h, "Start the server");
    t.cf.with(|s| s.local_health = true);
    press(&mut t.h, "Start the server");
    wait_for(&mut t.h, "The capture server was started.");
    assert_eq!(t.host.with(|s| s.starts).unwrap(), 1);
    press(&mut t.h, "Check now");
    wait_for(&mut t.h, GREEN);

    // Phone capture turned off with its module: no light.
    t.h.state_mut().open_view_spec("home");
    t.h.run_steps(2);
    cmd(&mut t.h, "module.disable", json!({"module": "expenses"}));
    t.h.run_steps(4);
    assert!(!shows(&t.h, "Phone capture status"));
}

/// AT181: the setup dialog walks all steps against the faked Cloudflare API and ends
/// with the QR code and the Delete Cloudflare token box ticked.
#[test]
fn app_desktop_at181_setup_dialog_walkthrough() {
    let mut t = start(false);
    open_advanced(&mut t.h);
    press(&mut t.h, "Set up phone capture over Cloudflare…");
    // The step list.
    for (i, s) in [
        "Before you start",
        "Connect to Cloudflare",
        "Address and sign-in",
        "Where the tunnel runs",
        "Install",
        "Check",
        "Pair your phone",
        "Finish",
    ]
    .iter()
    .enumerate()
    {
        assert!(has(&t.h, &format!("Step {}: {s}", i + 1)), "{s}");
    }
    // 1. Before you start: the permissions checklist, Copy list, token page.
    assert!(shows(&t.h, "Account · Cloudflare Tunnel · Edit"));
    assert!(shows(&t.h, "Zone · Zone · Read"));
    press(&mut t.h, "Open Cloudflare token page");
    assert!(events(&mut t.h).iter().any(|e| matches!(e, AppEvent::OpenLink { url } if url == "https://dash.cloudflare.com/profile/api-tokens")));
    press(&mut t.h, "Next");
    // 2. Connect: Check token.
    assert!(disabled(&t.h, "Next"));
    type_field(&mut t.h, "Cloudflare API token", TEST_TOKEN);
    press(&mut t.h, "Check token");
    wait_for(&mut t.h, "This token has every permission needed.");
    assert!(shows(&t.h, "Owner's account"));
    assert!(has(&t.h, "Step 2: Connect to Cloudflare, done"));
    press(&mut t.h, "Next");
    // 3. Address and emails.
    assert!(shows(&t.h, "https://capture.example.com"));
    type_field(&mut t.h, "Email addresses allowed", "owner@example.com");
    press(&mut t.h, "Next");
    // 4. Where the tunnel runs: a new tunnel by default.
    assert!(has(&t.h, "Create a new tunnel on this computer"));
    press(&mut t.h, "Next");
    // 5. Install: the plan, then Install.
    wait_for(
        &mut t.h,
        "Install the MoneyKeep capture server as a background service",
    );
    assert!(shows(&t.h, "Create the tunnel \"moneykeep-capture\""));
    press(&mut t.h, "Install");
    // 6. Check: runs by itself and shows the light.
    wait_for(&mut t.h, GREEN);
    assert!(has(&t.h, "Step 5: Install, done"));
    assert_eq!(t.host.with(|s| s.installs.len()).unwrap(), 1);
    // The Details list holds the calls, never a token.
    t.h.get_by_label("Details").click();
    t.h.run_steps(2);
    assert!(shows(&t.h, "/accounts/{account}/cfd_tunnel"));
    assert!(
        !labels_containing(&t.h, "")
            .iter()
            .any(|l| l.contains(TEST_TOKEN))
    );
    press(&mut t.h, "Next");
    // 7. Pair your phone: the QR code of the address with the code in its fragment.
    wait_for(
        &mut t.h,
        "QR code for https://capture.example.com/#pair=ABCD2345",
    );
    assert!(shows(&t.h, "Pairing code ABCD2345"));
    assert!(shows(&t.h, "Expires in"));
    press(&mut t.h, "Next");
    // 8. Finish: Delete Cloudflare token is ticked.
    let tick =
        t.h.get_by_label("Delete Cloudflare token — each change requires a new token")
            .accesskit_node()
            .toggled();
    assert_eq!(tick, Some(Toggled::True));
    assert!(t.secrets.all().values().any(|v| v == TEST_TOKEN));
    press(&mut t.h, "Finish");
    assert!(
        !t.secrets.all().values().any(|v| v == TEST_TOKEN),
        "token erased"
    );
    assert!(shows(&t.h, "Revoke it at Cloudflare too"));
    press(&mut t.h, "Done");
    assert!(!shows(&t.h, "Step 1: Before you start"));
    // Settings now offers the later changes.
    assert!(has(&t.h, "Change address or emails…"));
    assert!(has(&t.h, "Remove phone capture over Cloudflare…"));
}

/// The dialog can be closed and resumes where it stopped; Remove phone capture asks
/// for a token when none is stored and lists what goes.
#[test]
fn setup_dialog_resumes_and_remove_confirms() {
    let mut t = start(false);
    open_advanced(&mut t.h);
    press(&mut t.h, "Set up phone capture over Cloudflare…");
    press(&mut t.h, "Next");
    type_field(&mut t.h, "Cloudflare API token", TEST_TOKEN);
    press(&mut t.h, "Check token");
    wait_for(&mut t.h, "This token has every permission needed.");
    press(&mut t.h, "Close");
    assert!(!has(&t.h, "Step 1: Before you start"));
    press(
        &mut t.h,
        "Continue setting up phone capture over Cloudflare…",
    );
    assert!(
        shows(&t.h, "This token has every permission needed."),
        "resumed at step 2"
    );
    press(&mut t.h, "Close");
    // After a full setup whose token was deleted, removal asks for one.
    let mut t = start(true);
    cmd(&mut t.h, "capture.forget_cloudflare_token", json!({}));
    open_advanced(&mut t.h);
    press(&mut t.h, "Remove phone capture over Cloudflare…");
    assert!(shows(
        &t.h,
        "The Access application for capture.example.com"
    ));
    assert!(disabled(&t.h, "Remove"));
    type_field(&mut t.h, "Cloudflare API token", TEST_TOKEN);
    press(&mut t.h, "Remove");
    wait_for(&mut t.h, "Phone capture was removed.");
    assert!(
        t.cf.with(|s| s.tunnels.is_empty() && s.apps.is_empty())
            .unwrap()
    );
    assert!(has(&t.h, "Set up phone capture over Cloudflare…"));
}

/// mobile-capture.md AT36 in Settings: Repair Cloudflare settings rewrites the
/// Access application with the 30-day session.
#[test]
fn repair_from_settings() {
    let mut t = start(true);
    open_advanced(&mut t.h);
    press(&mut t.h, "Repair Cloudflare settings");
    wait_for(&mut t.h, "Cloudflare settings repaired");
    assert!(
        t.cf.cf_calls()
            .iter()
            .any(|c| c.starts_with("PUT /accounts/acc1/access/apps/"))
    );
}

/// mobile-capture.md AT38 in the app: an older server (one without a capture
/// version) shows the yellow update text and, since release-packaging.md, the
/// update notice whose Update now replaces it.
#[test]
fn update_server_from_settings() {
    let mut t = start(true);
    t.cf.with(|s| {
        s.server_version = "0.0.0+100.aaaaaaaaaaaa".into();
        s.server_capture_version = None;
    });
    open_phone_capture(&mut t.h);
    press(&mut t.h, "Check now");
    wait_for(
        &mut t.h,
        "Phone capture status: The capture server is older than this app",
    );
    wait_for(&mut t.h, "Update now");
    t.h.get_by_role_and_label(mk_views::testing::Role::Button, "Update now")
        .click();
    t.h.run_steps(2);
    wait_for(&mut t.h, "Capture server updated to ");
    assert_eq!(t.host.with(|s| s.updates.len()).unwrap(), 1);
}

/// mobile-capture.md test 41 in the app: an undecidable version asks "Update
/// anyway?"; Update goes ahead.
#[test]
fn update_anyway_confirmation() {
    let mut t = start(true);
    t.cf.with(|s| {
        s.server_version = "0.0.1+100.aaaaaaaaaaaa".into();
        s.server_capture_version = None;
    });
    t.host
        .with(|s| s.report.program_version = "0.0.1+100.bbbbbbbbbbbb".into());
    open_phone_capture(&mut t.h);
    press(&mut t.h, "Check now");
    wait_for(&mut t.h, "Update now");
    t.h.get_by_role_and_label(mk_views::testing::Role::Button, "Update now")
        .click();
    t.h.run_steps(2);
    wait_for(&mut t.h, "Update anyway?");
    press(&mut t.h, "Update");
    wait_for(&mut t.h, "Capture server updated to ");
    assert_eq!(t.host.with(|s| s.updates.len()).unwrap(), 1);
}

/// mobile-capture.md revision 8, test 58 in the app: the capture server's refusal
/// shows in Phone capture's status with **Check**, which rewrites `server.toml`
/// with the Access application's AUD tag and restarts the server.
#[test]
fn check_access_from_settings() {
    let mut t = start(true);
    let id =
        t.cf.with(|s| s.apps.keys().next().cloned())
            .unwrap()
            .unwrap();
    t.cf.with(|s| {
        s.apps.get_mut(&id).unwrap()["aud"] = json!("rotated-aud");
        s.access_problem = Some(json!({
            "reason": "access token audience mismatch: expected old, got rotated-aud",
            "at": 1_790_812_700,
        }));
    });
    open_phone_capture(&mut t.h);
    press(&mut t.h, "Check now");
    wait_for(
        &mut t.h,
        "Phone requests are being refused by the capture server: access token audience mismatch",
    );
    t.cf.with(|s| s.access_problem = None);
    t.h.get_by_role_and_label(Role::Button, "Check").click();
    t.h.run_steps(2);
    wait_for(&mut t.h, "Fixed: ");
    assert!(
        t.host
            .with(|s| s.config_text.clone())
            .unwrap()
            .contains("access_aud = \"rotated-aud\"")
    );
    assert_eq!(t.host.with(|s| s.aud_restarts).unwrap(), 1);
    assert!(
        t.cf.cf_calls()
            .contains(&format!("GET /accounts/acc1/access/apps/{id}"))
    );
}
