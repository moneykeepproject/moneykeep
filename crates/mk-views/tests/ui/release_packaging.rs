//! release-packaging.md tests RP3–RP5 in the app: the capture server update notice
//! (Settings → Phone capture), its Home → Attention item, Later, Update now's
//! answers and the "built without its capture server" case, against the scripted
//! capture server and computer of `mk_commands::capture_setup::testing`.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::common::*;
use mk_commands::Session;
use mk_commands::capture_setup::PERMISSION_DECLINED;
use mk_commands::capture_setup::testing::{FakeCloudflare, FakeHost, TEST_TOKEN};
use mk_commands::connect::{Connector, MemorySecrets};
use mk_ui::UserConfig;
use mk_ui::ctl::App;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

pub(crate) struct T {
    pub(crate) h: H,
    pub(crate) cf: FakeCloudflare,
    pub(crate) host: FakeHost,
    _dir: tempfile::TempDir,
}

const TITLE: &str = "The capture server needs an update";
const BODY_START: &str = "This MoneyKeep was updated, and the capture server that phones send to is still the older version.";
const PERMISSION: &str = "Windows will ask for permission, because the capture server runs in the background for this computer (it is installed for all users).";
const NOT_BUILT: &str = "This copy of MoneyKeep was built without its capture server. Install MoneyKeep with its installer, or (developers) build with cargo xtask release.";
const ATTENTION: &str = "A newer capture server is ready.";
const DECLINED: &str = "Not updated — Windows permission was not given. Phones can keep sending to the current server; press Update now when ready.";

/// A book with phone capture set up; the running server reports another capture
/// version when `older` (the program beside the app is this build's).
pub(crate) fn start(older: bool) -> T {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("p.moneykeep"), "Phone Books", "USD", "app").unwrap();
    let cf = FakeCloudflare::new();
    let host = FakeHost::windows();
    s.set_connector(Connector {
        fetcher: Arc::new(cf.clone()),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(1_790_812_800),
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
    if older {
        cf.with(|s| s.server_capture_version = Some("0.0.1+000000000000".into()));
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

/// Run frames until a label containing `part` shows (background jobs).
pub(crate) fn wait_for(h: &mut H, part: &str) {
    for _ in 0..400 {
        h.run_steps(2);
        if shows(h, part) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    panic!("{part:?} never showed: {:#?}", labels_containing(h, ""));
}

/// Let the background status checks finish.
fn settle(h: &mut H) {
    for _ in 0..12 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

pub(crate) fn open_phone_capture(h: &mut H) {
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Phone capture").click();
    h.run_steps(3);
}

pub(crate) fn open_home(h: &mut H) {
    h.state_mut().open_view_spec("home");
    h.run_steps(3);
}

fn has_button(h: &H, label: &str) -> bool {
    h.query_by_role_and_label(Role::Button, label).is_some()
}

/// RP3: a running server with another capture version, this build's program beside
/// the app: the card with the spec's words and Update now / Later, and the Home
/// Attention item that opens Settings → Phone capture.
#[test]
fn rp3_notice_card_and_attention() {
    let mut t = start(true);
    open_home(&mut t.h);
    wait_for(&mut t.h, ATTENTION);
    t.h.get_by_role_and_label(Role::Link, ATTENTION).click();
    t.h.run_steps(3);
    wait_for(&mut t.h, TITLE);
    assert!(shows(&t.h, BODY_START));
    assert!(shows(
        &t.h,
        "phones keep their pairing, and anything in a phone's Outbox is sent afterwards."
    ));
    assert!(shows(&t.h, PERMISSION));
    assert!(has_button(&t.h, "Update now"));
    assert!(has_button(&t.h, "Later"));
    // Nothing was updated by showing it.
    settle(&mut t.h);
    assert!(t.host.with(|s| s.updates.is_empty()).unwrap());
}

/// RP3: equal capture versions (even with another build version): neither.
#[test]
fn rp3_equal_capture_versions_show_nothing() {
    let mut t = start(false);
    t.cf.with(|s| s.server_version = "0.0.0+100.aaaaaaaaaaaa".into());
    open_phone_capture(&mut t.h);
    t.h.get_by_role_and_label(Role::Button, "Check now").click();
    settle(&mut t.h);
    wait_for(&mut t.h, "Phone capture is working");
    assert!(!shows(&t.h, TITLE));
    assert!(!has_button(&t.h, "Update now"));
    open_home(&mut t.h);
    settle(&mut t.h);
    assert!(!shows(&t.h, ATTENTION));
}

/// Later hides the Attention item until the app is opened again; the card stays.
#[test]
fn later_hides_the_attention_item() {
    let mut t = start(true);
    open_phone_capture(&mut t.h);
    wait_for(&mut t.h, TITLE);
    t.h.get_by_role_and_label(Role::Button, "Later").click();
    t.h.run_steps(3);
    assert!(shows(&t.h, TITLE), "the card stays");
    assert!(has_button(&t.h, "Update now"));
    open_home(&mut t.h);
    settle(&mut t.h);
    assert!(!shows(&t.h, ATTENTION));
    assert!(t.host.with(|s| s.updates.is_empty()).unwrap());
}

/// RP4: the program beside the app is not this build's: the "built without its
/// capture server" text, no Update now, no Attention item.
#[test]
fn rp4_built_without_its_capture_server() {
    let mut t = start(true);
    t.host
        .with(|s| s.report.program_capture_version = "0.0.1+111111111111".into());
    open_phone_capture(&mut t.h);
    wait_for(&mut t.h, NOT_BUILT);
    assert!(shows(&t.h, TITLE));
    assert!(!shows(&t.h, BODY_START));
    assert!(!has_button(&t.h, "Update now"));
    assert!(!has_button(&t.h, "Update server"));
    open_home(&mut t.h);
    settle(&mut t.h);
    assert!(!shows(&t.h, ATTENTION));
    assert!(t.host.with(|s| s.updates.is_empty()).unwrap());
}

/// RP5: Update now with the permission declined says so; accepted, it says
/// "Capture server updated to …" and the notice goes.
#[test]
fn rp5_update_now_declined_then_accepted() {
    let mut t = start(true);
    t.host
        .with(|s| s.fail_update = Some(PERMISSION_DECLINED.into()));
    open_phone_capture(&mut t.h);
    wait_for(&mut t.h, "Update now");
    t.h.get_by_role_and_label(Role::Button, "Update now")
        .click();
    t.h.run_steps(2);
    wait_for(&mut t.h, DECLINED);
    assert!(t.host.with(|s| s.updates.is_empty()).unwrap());
    t.host.with(|s| s.fail_update = None);
    t.h.get_by_role_and_label(Role::Button, "Update now")
        .click();
    t.h.run_steps(2);
    let short = mk_capture_api::package_version(mk_capture_api::BUILD_VERSION);
    wait_for(&mut t.h, &format!("Capture server updated to {short}."));
    assert_eq!(t.host.with(|s| s.updates.len()).unwrap(), 1);
    // The new server answers with this build's capture version: no more notice.
    t.cf.with(|s| s.server_capture_version = Some(mk_capture_api::CAPTURE_VERSION.into()));
    t.h.get_by_role_and_label(Role::Button, "Check now").click();
    settle(&mut t.h);
    wait_for(&mut t.h, "Phone capture is working");
    assert!(!shows(&t.h, TITLE));
}

/// A failed update says so in plain words with Details.
#[test]
fn update_now_failure_has_details() {
    let mut t = start(true);
    t.host.with(|s| {
        s.fail_update = Some(
            "The new server did not answer its health check. The previous server program was restored."
                .into(),
        )
    });
    open_phone_capture(&mut t.h);
    wait_for(&mut t.h, "Update now");
    t.h.get_by_role_and_label(Role::Button, "Update now")
        .click();
    t.h.run_steps(2);
    wait_for(
        &mut t.h,
        "The capture server was not updated: The new server did not answer its health check.",
    );
    assert!(shows(&t.h, "Details"));
}
