//! app-desktop.md revision 30 test 182 (UI part): a paste reaches a password field
//! and a normal field, and the calculator's Copy hands the result to the clipboard.
//! A headless test cannot reach the operating system clipboard, so the platform
//! side is checked on the built feature set (apps/moneykeep-app,
//! `app_desktop_at182_system_clipboard_is_built_in`).
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use crate::common::*;
use mk_commands::Session;
use mk_commands::capture_setup::testing::{FakeCloudflare, FakeHost, TEST_TOKEN};
use mk_commands::connect::{Connector, MemorySecrets};
use mk_ui::UserConfig;
use mk_ui::ctl::App;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

fn app() -> (H, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut s =
        Session::create(&dir.path().join("c.moneykeep"), "Clip Books", "USD", "app").unwrap();
    s.set_connector(Connector {
        fetcher: Arc::new(FakeCloudflare::new()),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(1_790_812_800),
        host: Some(Arc::new(FakeHost::windows())),
    });
    s.invoke("module.enable", json!({"module": "expenses"}))
        .unwrap();
    let app = App::new(
        Box::new(LocalBackend {
            session: Some(s),
            name: Some("Clip Books".into()),
        }),
        UserConfig::default(),
    );
    let mut h = harness(app);
    h.run();
    (h, dir)
}

fn shows(h: &H, part: &str) -> bool {
    labels_containing(h, "").iter().any(|l| l.contains(part))
}

fn paste_into(h: &mut H, role: Role, label: &str, text: &str) {
    h.get_by_role_and_label(role, label).focus();
    h.run();
    h.paste(text);
    h.run_steps(2);
}

#[test]
fn app_desktop_at182_paste_and_copy() {
    let (mut h, _dir) = app();
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Advanced").click();
    h.run_steps(2);
    // A normal field.
    paste_into(
        &mut h,
        Role::TextInput,
        "Server address",
        "http://127.0.0.1:8787",
    );
    assert_eq!(value(&h, "Server address"), "http://127.0.0.1:8787");
    // A password field: the pasted token is the one checked.
    h.get_by_role_and_label(Role::Button, "Set up phone capture over Cloudflare…")
        .click();
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "Next").click();
    h.run_steps(2);
    paste_into(
        &mut h,
        Role::PasswordInput,
        "Cloudflare API token",
        TEST_TOKEN,
    );
    h.get_by_role_and_label(Role::Button, "Check token").click();
    for _ in 0..400 {
        h.run_steps(2);
        if shows(&h, "This token has every permission needed.") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    assert!(shows(&h, "This token has every permission needed."));
    h.get_by_role_and_label(Role::Button, "Close").click();
    h.run_steps(2);
    // The calculator's Copy sets the clipboard.
    h.get_by_label("Calculator").click();
    h.run_steps(2);
    h.get_by_label("seven").click();
    h.run_steps(2);
    h.get_by_label("Copy").click();
    h.step();
    let copied = h.output().copied.iter().any(|t| t == "7");
    assert!(copied, "{:?}", h.output().copied);
}
