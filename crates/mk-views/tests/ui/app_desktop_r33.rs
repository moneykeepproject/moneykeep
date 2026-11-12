//! app-desktop.md revision 33 (AI assistant access) and mcp-server.md revision 3
//! acceptance test 28: Settings → Security → AI assistant access, the approval
//! dialog answering a real control-channel hello, the top-bar indicator and
//! Disconnect, and History's "Changes by AI assistants" filter.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use crate::common::*;
use mk_commands::ai_access::{AiAccess, Level};
use mk_commands::{KdfParams, Session};
use mk_control::server::{ControlEvent, ControlServer, SessionHost, SystemClock};
use mk_ui::AppEvent;
use mk_ui::ctl::App;
use mk_ui::ctl::ai::{AiClient, NOTE};
use mk_ui::{UserConfig, View};
use mk_views::testing::{Harness, Role, Toggled};
use serde_json::{Value, json};

type H = Harness;

const SWITCH: &str = "Allow AI assistants to use this book";
const APP_ONLY: &str = "Only through MoneyKeep, and ask me each time";

fn security(h: &mut H) {
    click(h, "Settings");
    settings_section(h, "Security");
}

fn toggled(h: &H, label: &str) -> bool {
    h.get_by_label(label).toggled() == Some(Toggled::True)
}

/// Forward control-channel events to the controller as the desktop shell does.
fn forward(h: &mut H, server: &ControlServer) -> Vec<ControlEvent> {
    let events = server.poll_events();
    for e in &events {
        let mut app = h.state_mut();
        match e {
            ControlEvent::ApprovalRequested {
                id,
                client,
                book_name,
                access,
            } => app.ai_approval_requested(*id, client, book_name, &access.access_text()),
            ControlEvent::ApprovalEnded { id } => app.ai_approval_ended(*id),
            ControlEvent::Connections(list) => app.ai_connections(
                list.iter()
                    .map(|c| AiClient {
                        name: c.client.clone(),
                        access: c.access.access_text(),
                    })
                    .collect(),
            ),
            ControlEvent::BookChanged => app.ai_book_changed(),
        }
    }
    h.run();
    events
}

fn wait_forward(h: &mut H, server: &ControlServer, pred: impl Fn(&ControlEvent) -> bool) {
    for _ in 0..2000 {
        if forward(h, server).iter().any(&pred) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("event not seen");
}

/// The controller's events for the shell: answers and disconnects go to the channel.
fn answer_events(h: &mut H, server: &ControlServer) -> Vec<AppEvent> {
    let evs = events(h);
    for e in &evs {
        match e {
            AppEvent::AiAnswer { id, allow } => server.answer(*id, *allow),
            AppEvent::AiDisconnect { client } => server.disconnect(client),
            _ => {}
        }
    }
    evs
}

/// A keep-mcp-like client saying hello on another thread; returns its replies.
fn hello(port: u16, token: &str, client: &str) -> std::thread::JoinHandle<Vec<Value>> {
    let msg = json!({"hello": "moneykeep-control/1", "token": token,
        "client": {"name": client, "kind": "keep-mcp"}, "book_path": null});
    std::thread::spawn(move || {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        writeln!(s, "{msg}").unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut out = Vec::new();
        loop {
            let mut l = String::new();
            match r.read_line(&mut l) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let v: Value = serde_json::from_str(&l).unwrap();
                    let stop = v.get("error").is_some() || v.get("event").is_some();
                    out.push(v);
                    if stop {
                        break;
                    }
                }
            }
        }
        out
    })
}

#[test]
fn at28_settings_security_ai_group() {
    let (app, _book) = new_book_app();
    let mut h = harness(app);
    security(&mut h);
    assert!(has(&h, "AI assistant access"));
    assert!(!toggled(&h, SWITCH), "off by default");
    assert!(has(&h, NOTE), "the privacy note is always visible");
    assert!(!has(&h, "Read only") && !has(&h, APP_ONLY));
    click(&mut h, SWITCH);
    assert!(events(&mut h).contains(&AppEvent::AiAccessChanged));
    assert!(toggled(&h, SWITCH));
    assert!(has(&h, NOTE));
    assert!(toggled(&h, "Read only") && !toggled(&h, "Read and write"));
    for g in [
        "Delete and void",
        "Change features and book settings",
        "Use bank feeds and connections",
    ] {
        assert!(disabled(&h, g), "{g} needs Read and write");
    }
    assert!(!disabled(&h, "Read files on this computer"));
    assert!(toggled(&h, APP_ONLY) && !disabled(&h, APP_ONLY));
    assert!(has_contains(&h, "keep-mcp is not installed."));
    click(&mut h, "Read and write");
    click(&mut h, "Delete and void");
    let stored = h.state_mut().test_invoke(
        "setting.get",
        json!({"module": "core", "key": "ai_access.groups"}),
    );
    assert_eq!(stored.unwrap()["value"], json!(["delete"]));
    assert!(!disabled(&h, "Delete and void") && toggled(&h, "Delete and void"));
    // keep-mcp found: Copy assistant setup… shows the snippet with Copy.
    h.state_mut().ai_keep_mcp(Some(vec![(
        "Claude Desktop".into(),
        r#"{"mcpServers":{"moneykeep":{"command":"/opt/keep-mcp","args":["/b.moneykeep"]}}}"#
            .into(),
    )]));
    h.run();
    assert!(!has_contains(&h, "keep-mcp is not installed."));
    click(&mut h, "Copy assistant setup…");
    assert!(has_contains(&h, "/opt/keep-mcp"));
    click(&mut h, "Copy");
    assert!(h.take_copied().iter().any(|c| c.contains("/opt/keep-mcp")));
    click(&mut h, "Done");
    // Another window serving assistants.
    h.state_mut().ai_other_window(true);
    h.run();
    assert!(has(
        &h,
        "Another MoneyKeep window is serving AI assistants."
    ));
}

#[test]
fn at28_app_only_box_is_ticked_and_disabled_for_a_protected_book() {
    const PW: &str = "correct horse battery";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("locked.moneykeep");
    let (mut s, _) =
        Session::create_encrypted(&path, "Locked", "USD", "test", PW, true, KdfParams::TEST)
            .unwrap();
    s.set_ai_access(&AiAccess {
        enabled: true,
        level: Level::ReadOnly,
        groups: Default::default(),
        app_only: true,
    })
    .unwrap();
    drop(s);
    let app = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        UserConfig::default(),
    );
    let mut h = harness(app);
    h.state_mut().open_book(path.to_str().unwrap());
    h.run();
    h.get_by_label("Password").focus();
    h.run();
    h.get_by_label("Password").type_text(PW);
    h.run();
    click(&mut h, "Open");
    h.run();
    assert!(
        events(&mut h)
            .iter()
            .any(|e| matches!(e, AppEvent::BookOpened { .. }))
    );
    security(&mut h);
    assert!(toggled(&h, SWITCH));
    assert!(toggled(&h, APP_ONLY) && disabled(&h, APP_ONLY));
    assert!(has(
        &h,
        "Password-protected books are only used through MoneyKeep."
    ));
}

#[test]
fn at28_approval_dialog_indicator_disconnect_and_ai_history() {
    let (app, book) = new_book_app();
    let mut h = harness(app);
    security(&mut h);
    click(&mut h, SWITCH);
    events(&mut h);
    // The real app-side handler on the same plain book.
    let host = Arc::new(SessionHost::new(Session::open(&book.path, "app").unwrap()));
    let config = book.dir.path().join("config");
    let server = ControlServer::start(&config, host, Arc::new(SystemClock::default())).unwrap();
    let token = mk_control::read_discovery(&config).unwrap().token;

    // Deny answers the hello with `denied`.
    let client = hello(server.port(), &token, "Helper");
    wait_forward(&mut h, &server, |e| {
        matches!(e, ControlEvent::ApprovalRequested { .. })
    });
    assert!(has(&h, "Allow AI assistant?"));
    assert!(has(
        &h,
        "Helper wants to use Test Books through keep-mcp. Access: Read only."
    ));
    click(&mut h, "Deny");
    answer_events(&mut h, &server);
    let replies = client.join().unwrap();
    assert_eq!(replies[0]["error"]["code"], "denied");
    assert!(!has(&h, "Allow AI assistant?"));
    assert!(
        h.query_by_role_and_label(Role::Button, "AI assistant connected")
            .is_none()
    );

    // Allow answers it with `ok`; the indicator appears.
    let client = hello(server.port(), &token, "Helper");
    wait_forward(&mut h, &server, |e| {
        matches!(e, ControlEvent::ApprovalRequested { .. })
    });
    click(&mut h, "Allow");
    answer_events(&mut h, &server);
    wait_forward(
        &mut h,
        &server,
        |e| matches!(e, ControlEvent::Connections(c) if !c.is_empty()),
    );
    let indicator = h.get_by_role_and_label(Role::Button, "AI assistant connected");
    assert_eq!(
        indicator.description().as_deref(),
        Some("Connected: Helper")
    );
    click(&mut h, "AI assistant connected");
    assert!(has(&h, "AI assistants"));
    assert!(has(&h, "Helper — Read only"));
    click(&mut h, "Disconnect Helper");
    answer_events(&mut h, &server);
    let replies = client.join().unwrap();
    assert_eq!(replies[0]["ok"], true);
    assert_eq!(
        replies.last().unwrap(),
        &json!({"event": "closed", "reason": "disconnected"})
    );
    h.run();
    assert!(
        h.query_by_role_and_label(Role::Button, "AI assistant connected")
            .is_none()
    );

    // History: only `mcp:` entries with the filter.
    let mut bot = Session::open(&book.path, "mcp:bot").unwrap();
    bot.invoke("account.create", json!({"name": "Robot", "type": "asset"}))
        .unwrap();
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Mine", "type": "asset"}),
    );
    h.state_mut().navigate(View::History);
    h.run();
    let rows = |h: &H, who: &str| h.query_all_by_label(who).count();
    assert!(rows(&h, "mcp:bot") == 1 && rows(&h, "app") > 0);
    click(&mut h, "Changes by AI assistants");
    assert_eq!(rows(&h, "mcp:bot"), 1);
    assert_eq!(rows(&h, "app"), 0, "{:?}", labels_containing(&h, "app"));
    // Session records appear in History (without the filter).
    click(&mut h, "Changes by AI assistants");
    assert!(has_contains(&h, "AI assistant access change"));
    assert!(has_contains(&h, "AI assistant allowed"));
    // "Open AI history" from the indicator's pop-up turns the filter on.
    h.state_mut().ai_connections(vec![AiClient {
        name: "Other".into(),
        access: "Read only".into(),
    }]);
    h.state_mut().navigate(View::Home);
    h.run();
    click(&mut h, "AI assistant connected");
    click(&mut h, "Open AI history");
    assert!(matches!(h.state().view(), View::History));
    assert!(rows(&h, "mcp:bot") == 1 && rows(&h, "app") == 0);
    drop(server);
}
