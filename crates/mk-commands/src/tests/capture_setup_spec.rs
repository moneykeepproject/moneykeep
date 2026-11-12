//! mobile-capture.md revision 2 acceptance tests 20–23, 25, 27 and 30 (24 is in
//! moneykeep-server, 26 in mk-capture-api, the phone app and mk-ui, 28 and 29 in
//! moneykeep-server). The Cloudflare API, the capture server and this computer's
//! services are faked; nothing touches the network or installs anything.

use std::sync::Arc;

use super::*;
use crate::capture_setup::testing::{DESKTOP_TOKEN, FakeCloudflare, FakeHost, TEST_TOKEN};
use crate::capture_setup::{OLD_COPY_DAYS, OldCopy, PERMISSIONS, ServiceState, text};
use crate::connect::{Connector, MemorySecrets};

struct T {
    b: Book,
    cf: FakeCloudflare,
    host: FakeHost,
    secrets: MemorySecrets,
}

fn setup_book() -> T {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "expenses"}));
    let cf = FakeCloudflare::new();
    let host = FakeHost::windows();
    let secrets = MemorySecrets::new(true);
    b.r.set_connector(Connector {
        fetcher: Arc::new(cf.clone()),
        secrets: Box::new(secrets.clone()),
        utc_offset_secs: 0,
        now: Some(1_790_812_800),
        host: Some(Arc::new(host.clone())),
    });
    T {
        b,
        cf,
        host,
        secrets,
    }
}

fn check(t: &mut T) -> Value {
    t.b.run("capture.setup_check_token", json!({"token": TEST_TOKEN}))
}

fn apply(t: &mut T, args: Value) -> Value {
    t.b.run("capture.setup_apply", args)
}

fn new_tunnel_args() -> Value {
    json!({"subdomain": "capture", "emails": ["Owner@Example.com"]})
}

fn call_body(t: &T, method: &str, path_part: &str) -> Vec<Value> {
    t.cf.with(|s| {
        s.calls
            .iter()
            .filter(|c| c.method == method && c.path.contains(path_part))
            .filter_map(|c| c.body.clone())
            .collect()
    })
    .unwrap()
}

fn position(calls: &[String], call: &str) -> usize {
    calls
        .iter()
        .position(|c| c.starts_with(call))
        .unwrap_or_else(|| panic!("{call} not in {calls:#?}"))
}

/// AT20: Check token lists accounts and zones and names missing permissions.
#[test]
fn at20_check_token_reports_missing_permissions() {
    let mut t = setup_book();
    t.cf.with(|s| {
        s.accounts.push(("acc2".into(), "Club account".into()));
        s.zones
            .push(("zone2".into(), "example.org".into(), "acc1".into()));
        s.granted.remove(PERMISSIONS[2]);
        s.granted.remove(PERMISSIONS[3]);
    });
    let v = check(&mut t);
    assert_eq!(v["ok"], false);
    let missing: Vec<&str> = v["missing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    assert_eq!(
        missing,
        [
            "Account · Access: Service Tokens · Edit",
            "Account · Access: Organizations, Identity Providers, and Groups · Read"
        ]
    );
    let accounts: Vec<&str> = v["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(accounts, ["Owner's account", "Club account"]);
    let zones: Vec<&str> = v["zones"]
        .as_array()
        .unwrap()
        .iter()
        .map(|z| z["name"].as_str().unwrap())
        .collect();
    assert_eq!(zones, ["example.com", "example.org"]);
    // Picking the other zone.
    let v =
        t.b.run("capture.setup_check_token", json!({"zone": "zone2"}));
    assert_eq!(v["zone"]["name"], "example.org");
    // All permissions: nothing missing.
    t.cf.with(|s| s.granted = PERMISSIONS.iter().copied().collect());
    let v = t.b.run("capture.setup_check_token", json!({}));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["missing"], json!([]));
    // No zone permission: both zone permissions named.
    t.cf.with(|s| {
        s.granted.remove(PERMISSIONS[5]);
    });
    let v = t.b.run("capture.setup_check_token", json!({}));
    assert_eq!(
        v["missing"],
        json!(["Zone · Zone · Read", "Zone · DNS · Edit"])
    );
    // A token Cloudflare refuses.
    let e =
        t.b.err("capture.setup_check_token", json!({"token": "wrong-token"}));
    assert_eq!(e.code(), "bad_cloudflare_token");
}

/// AT21: setup with "Create a new tunnel" makes everything in order and installs
/// both services.
#[test]
fn at21_new_tunnel_setup() {
    let mut t = setup_book();
    let c = check(&mut t);
    assert_eq!(c["suggest"], "new");
    // The dry run lists what will happen and calls nothing that creates.
    let plan = t.b.run("capture.setup_plan", new_tunnel_args());
    assert_eq!(plan["address"], "capture.example.com");
    assert_eq!(plan["problems"], json!([]));
    assert!(plan["actions"].to_string().contains("Create the tunnel"));
    assert!(t.cf.cf_calls().iter().all(|c| c.starts_with("GET")));
    let out = apply(&mut t, new_tunnel_args());
    assert_eq!(out["complete"], true, "{out}");
    let calls = t.cf.cf_calls();
    let create = position(&calls, "POST /accounts/acc1/cfd_tunnel");
    let config = position(&calls, "PUT /accounts/acc1/cfd_tunnel/");
    let dns = position(&calls, "POST /zones/zone1/dns_records");
    let token = position(&calls, "POST /accounts/acc1/access/service_tokens");
    let app = position(&calls, "POST /accounts/acc1/access/apps");
    assert!(
        create < config && config < dns && dns < token && token < app,
        "{calls:#?}"
    );
    let tunnel = &call_body(&t, "POST", "/cfd_tunnel")[0];
    assert_eq!(tunnel["config_src"], "cloudflare");
    let tunnel_id =
        t.cf.with(|s| s.tunnels.keys().next().cloned())
            .unwrap()
            .unwrap();
    // Configurations: the hostname, then the catch-all.
    let cfg = &call_body(&t, "PUT", "/configurations")[0];
    assert_eq!(
        cfg["config"]["ingress"],
        json!([
            {"hostname": "capture.example.com", "service": "http://127.0.0.1:8787", "originRequest": {}},
            {"service": "http_status:404"}
        ])
    );
    // A proxied CNAME to the tunnel.
    let rec = &call_body(&t, "POST", "/dns_records")[0];
    assert_eq!(rec["type"], "CNAME");
    assert_eq!(rec["name"], "capture.example.com");
    assert_eq!(rec["content"], format!("{tunnel_id}.cfargotunnel.com"));
    assert_eq!(rec["proxied"], true);
    // Access: the Allow policy with the emails, the service token and its Service
    // Auth policy, the application with both.
    let policies = call_body(&t, "POST", "/access/policies");
    assert_eq!(policies.len(), 2);
    assert_eq!(policies[0]["decision"], "allow");
    assert_eq!(
        policies[0]["include"],
        json!([{"email": {"email": "owner@example.com"}}])
    );
    assert_eq!(policies[1]["decision"], "non_identity");
    let st_id =
        t.cf.with(|s| s.service_tokens.keys().next().cloned())
            .unwrap()
            .unwrap();
    assert_eq!(
        policies[1]["include"],
        json!([{"service_token": {"token_id": st_id}}])
    );
    let app_body = &call_body(&t, "POST", "/access/apps")[0];
    assert_eq!(app_body["domain"], "capture.example.com");
    assert_eq!(app_body["type"], "self_hosted");
    assert_eq!(app_body["policies"].as_array().unwrap().len(), 2);
    // Both services installed with the AUD tag and team domain in server.toml.
    let (installs, config_text) = t
        .host
        .with(|s| (s.installs.clone(), s.config_text.clone()))
        .unwrap();
    assert_eq!(installs.len(), 1);
    let i = &installs[0];
    assert_eq!(i.listen, "127.0.0.1:8787");
    assert!(!i.allow_lan);
    assert_eq!(
        i.tunnel_token.as_deref(),
        Some(format!("TUNNEL-TOKEN-{tunnel_id}").as_str())
    );
    let app_id =
        t.cf.with(|s| s.apps.keys().next().cloned())
            .unwrap()
            .unwrap();
    assert!(
        config_text.contains(&format!("access_aud = \"aud-{app_id}\"")),
        "{config_text}"
    );
    assert!(config_text.contains("access_team_domain = \"owner.cloudflareaccess.com\""));
    assert!(config_text.contains("client_url = \"https://capture.example.com\""));
    // The desktop now uses the server on this computer with its token.
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["configured"], true);
    assert_eq!(s["url"], "http://127.0.0.1:8787");
    assert_eq!(s["setup"]["complete"], true);
    assert_eq!(s["setup"]["installed_cloudflared"], true);
    assert!(t.secrets.all().values().any(|v| v == DESKTOP_TOKEN));
    // Running apply again changes nothing at Cloudflare.
    let before = t.cf.cf_calls().len();
    apply(&mut t, new_tunnel_args());
    let again: Vec<String> = t.cf.cf_calls()[before..].to_vec();
    assert!(again.iter().all(|c| c.starts_with("GET")), "{again:#?}");
}

/// AT22: "Add to my existing tunnel" keeps its rules and puts the address before the
/// catch-all; "another device" routes to this computer's LAN address.
#[test]
fn at22_existing_tunnel_keeps_rules() {
    let nas = json!({"hostname": "nas.example.com", "service": "http://localhost:5000"});
    let catch_all = json!({"service": "http_status:404"});
    for device in ["this", "lan"] {
        let mut t = setup_book();
        t.cf.add_tunnel("tun-home", "home", json!([nas, catch_all]));
        t.host
            .with(|s| s.report.cloudflared_service = Some(ServiceState::Running));
        let c = check(&mut t);
        assert_eq!(c["suggest"], "existing");
        assert_eq!(c["tunnels"][0]["name"], "home");
        let mut args =
            json!({"emails": ["owner@example.com"], "tunnel": "tun-home", "device": device});
        if device == "lan" {
            args["lan_address"] = json!("192.168.1.20");
        }
        let out = apply(&mut t, args);
        assert_eq!(out["complete"], true);
        let service = if device == "lan" {
            "http://192.168.1.20:8787"
        } else {
            "http://127.0.0.1:8787"
        };
        let ingress =
            t.cf.with(|s| s.tunnels.get("tun-home").unwrap().1["ingress"].clone())
                .unwrap();
        assert_eq!(
            ingress,
            json!([nas, {"hostname": "capture.example.com", "service": service, "originRequest": {}}, catch_all]),
            "{device}"
        );
        let calls = t.cf.cf_calls();
        assert!(!calls.iter().any(|c| c == "POST /accounts/acc1/cfd_tunnel"));
        let rec = &call_body(&t, "POST", "/dns_records")[0];
        assert_eq!(rec["content"], "tun-home.cfargotunnel.com");
        let installs = t.host.with(|s| s.installs.clone()).unwrap();
        assert!(installs[0].tunnel_token.is_none(), "no cloudflared install");
        if device == "lan" {
            assert_eq!(installs[0].listen, "192.168.1.20:8787");
            assert!(installs[0].allow_lan);
            let s = t.b.run("capture.settings", json!({}));
            assert_eq!(s["url"], "http://192.168.1.20:8787");
            assert_eq!(s["trusted"], true);
        } else {
            assert_eq!(installs[0].listen, "127.0.0.1:8787");
        }
    }
}

/// AT23: an existing cloudflared service (another tunnel) is never replaced.
#[test]
fn at23_existing_cloudflared_not_replaced() {
    let mut t = setup_book();
    t.host.with(|s| {
        s.report.cloudflared_installed = true;
        s.report.cloudflared_service = Some(ServiceState::Running);
    });
    check(&mut t);
    let plan = t.b.run("capture.setup_plan", new_tunnel_args());
    assert!(
        plan["problems"].to_string().contains("it was not replaced"),
        "{plan}"
    );
    let e = t.b.err("capture.setup_apply", new_tunnel_args());
    assert_eq!(e.code(), "setup_blocked");
    assert!(e.to_string().contains("Add to my existing tunnel"));
    assert!(t.cf.cf_calls().iter().all(|c| c.starts_with("GET")));
    assert!(t.host.with(|s| s.installs.is_empty()).unwrap());
}

/// A setup stopped by an error resumes where it stopped, and the Details list shows
/// the failed call.
#[test]
fn stopped_setup_resumes() {
    let mut t = setup_book();
    check(&mut t);
    t.cf.with(|s| {
        s.granted.remove(PERMISSIONS[1]);
    });
    let e = t.b.err("capture.setup_apply", new_tunnel_args());
    assert_eq!(e.code(), "cloudflare");
    assert!(
        e.to_string()
            .contains("POST /accounts/{account}/access/policies"),
        "{e}"
    );
    let s = t.b.run("capture.settings", json!({}));
    let details = s["setup"]["details"].as_array().unwrap();
    let last = details.last().unwrap();
    assert_eq!(last["path"], "/accounts/{account}/access/policies");
    assert!(last["result"].as_str().unwrap().starts_with("HTTP 403"));
    t.cf.with(|s| s.granted = PERMISSIONS.iter().copied().collect());
    let out = apply(&mut t, new_tunnel_args());
    assert_eq!(out["complete"], true);
    assert_eq!(
        t.cf.with(|s| s.tunnels.len()).unwrap(),
        1,
        "one tunnel only"
    );
    assert_eq!(t.cf.with(|s| s.service_tokens.len()).unwrap(), 1);
    assert_eq!(t.cf.with(|s| s.dns.len()).unwrap(), 1);
    // A taken address is refused.
    let mut t2 = setup_book();
    t2.cf.with(|s| {
        s.dns.insert(
            "other".into(),
            json!({"id": "other", "name": "capture.example.com", "type": "A", "content": "203.0.113.5"}),
        );
    });
    check(&mut t2);
    let plan = t2.b.run("capture.setup_plan", new_tunnel_args());
    assert!(
        plan["problems"]
            .to_string()
            .contains("already exists in your DNS")
    );
}

/// AT25: Finish with "Delete Cloudflare token" erases the stored token; unticked
/// keeps it; the token never appears in logs, config or the Details list.
#[test]
fn at25_token_kept_or_erased_never_shown() {
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    let stored = |t: &T| t.secrets.all().values().any(|v| v == TEST_TOKEN);
    assert!(stored(&t), "kept while setting up (unticked keeps it)");
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["setup"]["has_cloudflare_token"], true);
    // Nowhere but the credential store.
    let settings_text = s.to_string();
    let history = t.b.run("history.list", json!({"limit": 500})).to_string();
    let config = t.host.with(|s| s.config_text.clone()).unwrap();
    let raw_setting =
        t.b.s
            .read(|tx| Ok::<_, CommandError>(tx.get_setting("expenses", "capture.setup")?))
            .unwrap()
            .unwrap()
            .to_string();
    for (what, text) in [
        ("settings", &settings_text),
        ("history", &history),
        ("server.toml", &config),
        ("setup record", &raw_setting),
    ] {
        assert!(!text.contains(TEST_TOKEN), "token in {what}");
        assert!(
            !text.contains("SERVICE-SECRET"),
            "service token secret in {what}"
        );
        assert!(!text.contains("TUNNEL-TOKEN"), "tunnel token in {what}");
        assert!(!text.contains(DESKTOP_TOKEN), "desktop token in {what}");
    }
    assert!(settings_text.contains("\"details\""));
    // Ticked: erased.
    let v = t.b.run("capture.forget_cloudflare_token", json!({}));
    assert_eq!(v["forgotten"], true);
    assert!(
        v["token_page"]
            .as_str()
            .unwrap()
            .starts_with("https://dash.cloudflare.com/")
    );
    assert!(!stored(&t));
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["setup"]["has_cloudflare_token"], false);
    // A later change asks for a token.
    let e = t.b.err("capture.setup_apply", new_tunnel_args());
    assert_eq!(e.code(), "token_needed");
}

fn status(t: &mut T) -> Value {
    t.b.run("capture.status", json!({}))
}

/// AT27: the status light's states, texts and accessible names.
#[test]
fn at27_status_light() {
    let mut t = setup_book();
    let v = status(&mut t);
    assert_eq!(v["state"], "grey");
    assert_eq!(v["text"], text::GREY);
    assert_eq!(
        v["accessible_name"],
        "Phone capture status: Phone capture is off"
    );
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    let v = status(&mut t);
    assert_eq!(v["state"], "green", "{v}");
    assert_eq!(
        v["accessible_name"],
        "Phone capture status: Phone capture is working"
    );
    assert_eq!(v["waiting"], 0);
    assert!(v["last_activity"].is_number());
    // The public address uses the service token.
    let public =
        t.cf.with(|s| {
            s.calls
                .iter()
                .rev()
                .find(|c| c.authority == "capture.example.com")
                .cloned()
        })
        .unwrap()
        .unwrap();
    assert!(
        public
            .headers
            .iter()
            .any(|(k, _)| k == "CF-Access-Client-Id")
    );
    // Public fails: yellow with the tunnel text.
    t.cf.with(|s| s.public_health = false);
    let v = status(&mut t);
    assert_eq!(v["state"], "yellow");
    assert_eq!(v["text"], text::TUNNEL);
    assert_eq!(
        v["accessible_name"],
        format!("Phone capture status: {}", text::TUNNEL)
    );
    t.cf.with(|s| s.public_health = true);
    // Version differs (a server without a capture version): yellow.
    t.cf.with(|s| {
        s.server_version = "9.9.9".into();
        s.server_capture_version = None;
    });
    let v = status(&mut t);
    assert_eq!(
        (v["state"].as_str(), v["text"].as_str()),
        (Some("yellow"), Some(text::NEWER))
    );
    t.cf.with(|s| {
        s.server_version = mk_capture_api::BUILD_VERSION.into();
        s.server_capture_version = Some(mk_capture_api::CAPTURE_VERSION.into());
    });
    // Local fails: red, with Start server when the service exists but is stopped.
    t.cf.with(|s| s.local_health = false);
    t.host.with(|s| s.report.server = ServiceState::Stopped);
    let v = status(&mut t);
    assert_eq!(v["state"], "red");
    assert_eq!(v["text"], text::RED);
    assert_eq!(
        v["accessible_name"],
        format!("Phone capture status: {}", text::RED)
    );
    assert_eq!(v["can_start"], true);
    // Collecting fails meanwhile: remembered for the light.
    assert!(t.b.try_run("capture.collect", json!({})).is_err());
    let out = t.b.run("capture.start_server", json!({}));
    assert_eq!(out["started"], true);
    assert_eq!(t.host.with(|s| s.starts).unwrap(), 1);
    t.cf.with(|s| s.local_health = true);
    // upgrade-resilience-1 U1.9: older than this passing health check, the error is
    // a note with its time, not the status.
    let v = status(&mut t);
    assert_eq!(v["state"], "green", "{v}");
    let note = v["collect_note"].as_str().unwrap();
    assert!(
        note.starts_with(&format!("{} at ", text::COLLECT)),
        "{note}"
    );
    // A collect that works clears it.
    t.b.run("capture.collect", json!({}));
    let v = status(&mut t);
    assert_eq!(v["state"], "green");
    assert!(v.get("collect_note").is_none(), "{v}");
    // Module off: grey.
    t.b.run("module.disable", json!({"module": "expenses"}));
    assert_eq!(status(&mut t)["state"], "grey");
}

/// security-review-1 A1: after the capture server's data moved to its protected
/// folder, the status offers "Remove the old copy" for 30 days; removing it clears
/// the offer.
#[test]
fn sr_a1_status_offers_removing_the_old_copy() {
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    let v = status(&mut t);
    assert!(v.get("old_copy").is_none(), "{v}");
    let now = 1_790_812_800;
    t.host.with(|s| {
        s.report.old_copy = Some(OldCopy {
            path: "C:\\Users\\Pat\\AppData\\Local\\MoneyKeep\\data\\capture-server".into(),
            moved_at: now - 86_400,
        });
    });
    let v = status(&mut t);
    assert_eq!(
        v["old_copy"]["path"],
        "C:\\Users\\Pat\\AppData\\Local\\MoneyKeep\\data\\capture-server"
    );
    assert_eq!(
        v["old_copy"]["until"],
        now - 86_400 + OLD_COPY_DAYS * 86_400
    );
    let out = t.b.run("capture.remove_old_copy", json!({}));
    assert_eq!(out["removed"], true, "{out}");
    assert!(status(&mut t).get("old_copy").is_none());
    // upgrade-resilience-1 U1.3: after 30 days, offered under Clean up instead of
    // on the card, while the folder exists.
    t.host.with(|s| {
        s.report.old_copy = Some(OldCopy {
            path: "x".into(),
            moved_at: now - 31 * 86_400,
        });
    });
    let v = status(&mut t);
    assert_eq!(v["old_copy"]["on_card"], false, "{v}");
    assert_eq!(v["old_copy"]["path"], "x");
    let e = t.b.try_run("capture.remove_old_copy", json!({}));
    assert!(e.is_ok(), "the fake still has it: {e:?}");
    assert!(t.b.try_run("capture.remove_old_copy", json!({})).is_err());
}

/// AT30: Remove phone capture deletes exactly what setup created and uninstalls the
/// services.
#[test]
fn at30_remove_deletes_exactly_what_was_made() {
    // New tunnel: everything goes.
    let mut t = setup_book();
    t.cf.with(|s| {
        s.dns.insert(
            "keep".into(),
            json!({"id": "keep", "name": "www.example.com", "type": "A", "content": "203.0.113.5"}),
        );
    });
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    let s = t.b.run("capture.settings", json!({}));
    let listed = s["setup"]["resources"].as_array().unwrap().len();
    assert_eq!(listed, 8, "{}", s["setup"]["resources"]);
    let before = t.cf.cf_calls().len();
    let out = t.b.run("capture.setup_remove", json!({}));
    assert_eq!(out["removed"], true);
    let deletes: Vec<String> = t.cf.cf_calls()[before..]
        .iter()
        .filter(|c| !c.starts_with("GET"))
        .cloned()
        .collect();
    assert_eq!(deletes.len(), 6, "{deletes:#?}");
    assert!(deletes.iter().all(|c| c.starts_with("DELETE")));
    assert!(
        deletes[0].contains("/access/apps/"),
        "the application goes first"
    );
    t.cf.with(|s| {
        assert!(s.tunnels.is_empty());
        assert!(s.apps.is_empty());
        assert!(s.policies.is_empty());
        assert!(s.service_tokens.is_empty());
        assert_eq!(s.dns.keys().collect::<Vec<_>>(), ["keep"]);
    });
    let un = t.host.with(|s| s.uninstalls.clone()).unwrap();
    assert_eq!(un.len(), 1);
    assert!(un[0].server && un[0].cloudflared);
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["configured"], false);
    assert_eq!(s["setup"]["started"], false);
    assert!(
        !t.secrets
            .all()
            .values()
            .any(|v| v == TEST_TOKEN || v == DESKTOP_TOKEN || v.starts_with("SERVICE-SECRET"))
    );
    // Existing tunnel: only our rule goes; the tunnel and cloudflared stay.
    let nas = json!({"hostname": "nas.example.com", "service": "http://localhost:5000"});
    let catch_all = json!({"service": "http_status:404"});
    let mut t = setup_book();
    t.cf.add_tunnel("tun-home", "home", json!([nas, catch_all]));
    t.host
        .with(|s| s.report.cloudflared_service = Some(ServiceState::Running));
    check(&mut t);
    apply(
        &mut t,
        json!({"emails": ["owner@example.com"], "tunnel": "tun-home"}),
    );
    t.b.run("capture.setup_remove", json!({}));
    t.cf.with(|s| {
        assert_eq!(s.tunnels.len(), 1);
        assert_eq!(
            s.tunnels.get("tun-home").unwrap().1["ingress"],
            json!([nas, catch_all])
        );
    });
    let calls = t.cf.cf_calls();
    assert!(
        !calls
            .iter()
            .any(|c| c.starts_with("DELETE /accounts/acc1/cfd_tunnel"))
    );
    let un = t.host.with(|s| s.uninstalls.clone()).unwrap();
    assert!(un[0].server && !un[0].cloudflared);
}

/// mobile-capture.md AT14 with revision 2: the commands that handle a token stay
/// off MCP; the plan and the status are offered.
#[test]
fn mcp_exposure() {
    for name in [
        "capture.setup_check_token",
        "capture.setup_apply",
        "capture.setup_remove",
        "capture.forget_cloudflare_token",
        "capture.start_server",
        "capture.setup_repair",
        "capture.update_server",
        "capture.check_access",
    ] {
        assert!(NOT_OVER_MCP.contains(&name), "{name}");
    }
    for name in ["capture.setup_plan", "capture.status"] {
        assert!(!NOT_OVER_MCP.contains(&name), "{name}");
        assert!(Registry::with_builtins().spec(name).is_some());
    }
}

/// app-desktop.md revision 30 test 183: an empty `GET /accounts` with a zone that
/// carries its account still finds the account; both empty gives the new message;
/// Details shows how many each call returned.
#[test]
fn app_desktop_at183_accounts_from_zones() {
    let mut t = setup_book();
    t.cf.with(|s| s.hide_accounts = true);
    let v = check(&mut t);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["account"]["id"], "acc1");
    assert_eq!(v["account"]["name"], "Owner's account");
    assert_eq!(v["zone"]["name"], "example.com");
    let s = t.b.run("capture.settings", json!({}));
    let details = s["setup"]["details"].as_array().unwrap();
    let find = |path: &str| {
        details
            .iter()
            .find(|d| d["path"] == path)
            .map(|d| d["result"].as_str().unwrap().to_owned())
            .unwrap()
    };
    assert_eq!(find("/accounts"), "ok: 0 accounts");
    assert_eq!(find("/zones"), "ok: 1 zone");
    // Setup goes on with that account.
    let out = apply(&mut t, new_tunnel_args());
    assert_eq!(out["complete"], true);
    // Both empty: the message names the zone and account permissions.
    let mut t = setup_book();
    t.cf.with(|s| {
        s.hide_accounts = true;
        s.granted.remove(PERMISSIONS[5]);
    });
    let e =
        t.b.err("capture.setup_check_token", json!({"token": TEST_TOKEN}));
    assert_eq!(e.to_string(), crate::capture_setup::NO_ACCOUNT);
    assert!(
        e.to_string()
            .contains("Zone · Zone · Read and Zone · DNS · Edit")
    );
    let s = t.b.run("capture.settings", json!({}));
    assert!(s["setup"]["details"].to_string().contains("ok: 0 zones"));
}

/// mobile-capture.md AT36: guided setup creates the Access application with a 30-day
/// session; Repair rewrites an existing application with it.
#[test]
fn at36_session_duration_and_repair() {
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    let app = &call_body(&t, "POST", "/access/apps")[0];
    assert_eq!(app["session_duration"], "720h");
    // An application made earlier with a shorter session.
    let id =
        t.cf.with(|s| s.apps.keys().next().cloned())
            .unwrap()
            .unwrap();
    t.cf.with(|s| {
        s.apps.get_mut(&id).unwrap()["session_duration"] = json!("24h");
    });
    let out = t.b.run("capture.setup_repair", json!({}));
    assert_eq!(out["repaired"], true);
    let put = call_body(&t, "PUT", "/access/apps/");
    assert_eq!(put.len(), 1);
    assert_eq!(put[0]["session_duration"], "720h");
    assert_eq!(put[0]["domain"], "capture.example.com");
    assert_eq!(put[0]["policies"].as_array().unwrap().len(), 2);
    t.cf.with(|s| {
        assert_eq!(s.apps[&id]["session_duration"], "720h");
        assert!(
            s.apps[&id]["aud"].as_str().unwrap().starts_with("aud-"),
            "the AUD tag stays"
        );
    });
    let s = t.b.run("capture.settings", json!({}));
    assert!(
        s["setup"]["details"]
            .to_string()
            .contains("/accounts/{account}/access/apps/{app}")
    );
    // Without a stored token, a token is asked for.
    t.b.run("capture.forget_cloudflare_token", json!({}));
    assert_eq!(
        t.b.err("capture.setup_repair", json!({})).code(),
        "token_needed"
    );
    let out =
        t.b.run("capture.setup_repair", json!({"token": TEST_TOKEN}));
    assert_eq!(out["repaired"], true);
    // Nothing to repair before setup.
    let mut fresh = setup_book();
    check(&mut fresh);
    assert_eq!(
        fresh.b.err("capture.setup_repair", json!({})).code(),
        "not_set_up"
    );
}

/// mobile-capture.md AT38 (desktop): an older server turns the light yellow with the
/// update text and Update server; Update server replaces it through the host and the
/// light is green again; a failed update is reported.
#[test]
fn at38_older_server_and_update() {
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    t.cf.with(|s| {
        s.server_version = "0.0.0+100.aaaaaaaaaaaa".into();
        s.server_capture_version = None;
    });
    let v = status(&mut t);
    assert_eq!(v["state"], "yellow");
    assert_eq!(v["text"], text::OLDER);
    // release-packaging.md: the update notice with Update now (the program beside
    // the app is this build's).
    assert_eq!(v["update_needed"], true);
    assert_eq!(v["update_ready"], true);
    assert_eq!(v["can_update"], false);
    let out = t.b.run("capture.update_server", json!({}));
    assert_eq!(out["updated"], true);
    assert_eq!(out["version"], mk_capture_api::BUILD_VERSION);
    assert_eq!(t.host.with(|s| s.updates.len()).unwrap(), 1);
    let s = t.b.run("capture.settings", json!({}));
    assert!(s["setup"]["details"].to_string().contains("UPDATE"));
    // The new server answers with this build's version.
    t.cf.with(|s| {
        s.server_version = mk_capture_api::BUILD_VERSION.into();
        s.server_capture_version = Some(mk_capture_api::CAPTURE_VERSION.into());
    });
    let v = status(&mut t);
    assert_eq!(v["state"], "green");
    assert_eq!(v["update_needed"], false);
    assert_eq!(v["update_ready"], false);
    // A failed update (the old program was restored by the host) is reported.
    t.cf.with(|s| {
        s.server_version = "0.0.0+100.aaaaaaaaaaaa".into();
        s.server_capture_version = None;
    });
    t.host.with(|s| {
        s.fail_update =
            Some("The new server did not answer its health check. The previous server program was restored.".into())
    });
    let e = t.b.err("capture.update_server", json!({}));
    assert_eq!(e.code(), "update_failed");
    assert!(
        e.to_string()
            .contains("previous server program was restored")
    );
}

/// mobile-capture.md AT39: Update server refuses a missing program and one without
/// the phone app, with the build message.
#[test]
fn at39_update_refuses_without_a_new_build() {
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    t.cf.with(|s| {
        s.server_version = "0.0.0+100.aaaaaaaaaaaa".into();
        s.server_capture_version = None;
    });
    type Change = Box<dyn Fn(&mut crate::capture_setup::HostReport)>;
    let changes: [(&str, Change); 2] = [
        ("missing", Box::new(|r| r.server_program = false)),
        ("no app", Box::new(|r| r.program_has_app = false)),
    ];
    for (what, f) in changes {
        let saved = t.host.with(|s| s.report.clone()).unwrap();
        t.host.with(|s| f(&mut s.report));
        let e = t.b.err("capture.update_server", json!({}));
        assert_eq!(e.code(), "build_first", "{what}");
        assert!(e.to_string().contains(text::BUILD_FIRST), "{what}: {e}");
        t.host.with(|s| s.report = saved);
    }
    assert!(t.host.with(|s| s.updates.is_empty()).unwrap());
}

/// mobile-capture.md test 41 (revision 4a): a build with build information is newer
/// than a server without it; equal is refused as "already this build"; an older
/// build is refused; an undecidable pair asks "Update anyway?" and goes ahead when
/// confirmed.
#[test]
fn at41_version_rules_for_update() {
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    let built = |t: &T, v: &str| t.host.with(|s| s.report.program_version = v.to_owned());
    let running = |t: &T, v: &str| {
        t.cf.with(|s| {
            s.server_version = v.to_owned();
            s.server_capture_version = None;
        })
    };
    // The owner's case: a server from before revision 4 reports "0.0.1".
    running(&t, "0.0.1");
    built(&t, "0.0.1+1793216209.59ebe798226f");
    assert_eq!(t.b.run("capture.update_server", json!({}))["updated"], true);
    // Equal: already this build.
    running(&t, "0.0.1+1793216209.59ebe798226f");
    let e = t.b.err("capture.update_server", json!({}));
    assert_eq!(e.code(), "already_this_build");
    assert_eq!(e.to_string(), text::ALREADY_THIS_BUILD);
    // Older build than the running server.
    built(&t, "0.0.1+1793000000.0123456789ab");
    let e = t.b.err("capture.update_server", json!({}));
    assert_eq!(e.code(), "running_is_newer");
    assert!(e.to_string().starts_with(text::RUNNING_IS_NEWER));
    // Undecidable (same time, other commit): confirmation, then forced.
    built(&t, "0.0.1+1793216209.ffffffffffff");
    let e = t.b.err("capture.update_server", json!({}));
    assert_eq!(e.code(), "confirm_needed");
    assert_eq!(e.to_string(), text::UPDATE_ANYWAY);
    assert_eq!(t.host.with(|s| s.updates.len()).unwrap(), 1);
    assert_eq!(
        t.b.run("capture.update_server", json!({"force": true}))["updated"],
        true
    );
    assert_eq!(t.host.with(|s| s.updates.len()).unwrap(), 2);
    // The status light calls a pre-revision-4 server older.
    if mk_capture_api::BUILD_VERSION.contains('+') {
        running(&t, "0.0.1");
        assert_eq!(status(&mut t)["text"], text::OLDER);
    }
}

/// branding.md step 6 (desktop): an installation under the old name turns the light
/// yellow with "The capture server uses the old name — press Update server" and
/// Update server, which moves it even when the running build equals the built one;
/// afterwards the light is green.
#[test]
fn branding_old_name_light_and_update() {
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    assert_eq!(status(&mut t)["state"], "green");
    t.host.with(|s| s.report.old_name = true);
    let v = status(&mut t);
    assert_eq!(v["state"], "yellow");
    assert_eq!(v["text"], text::OLD_NAME);
    assert_eq!(
        v["text"],
        "The capture server uses the old name — press Update server"
    );
    assert_eq!(v["problems"][0], text::OLD_NAME);
    assert_eq!(v["can_update"], true);
    assert_eq!(v["old_name"], true);
    // Same build running: still moved.
    let out = t.b.run("capture.update_server", json!({}));
    assert_eq!(out["updated"], true);
    assert_eq!(t.host.with(|s| s.updates.len()).unwrap(), 1);
    t.host.with(|s| s.report.old_name = false);
    let v = status(&mut t);
    assert_eq!(v["state"], "green");
    assert_eq!(v["old_name"], false);
    // Down and under the old name: red, with Update server offered.
    t.host.with(|s| s.report.old_name = true);
    t.cf.with(|s| s.local_health = false);
    let v = status(&mut t);
    assert_eq!(v["state"], "red");
    assert_eq!(v["problems"][0], text::OLD_NAME);
    assert_eq!(v["can_update"], true);
    assert_eq!(v["can_start"], false);
}

/// mobile-capture.md revision 8, test 58: the status names the server's latest
/// Access refusal; Check with a changed `aud` records it, rewrites `server.toml` and
/// restarts the server; a changed domain sends the `PUT` (other fields kept); a
/// missing application is reported and can be made again.
#[test]
fn at58_check_access_application() {
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    let id =
        t.cf.with(|s| s.apps.keys().next().cloned())
            .unwrap()
            .unwrap();
    let first_aud = format!("aud-{id}");
    let host = t.host.clone();
    let config = || host.with(|s| s.config_text.clone()).unwrap();
    assert!(config().contains(&format!("access_aud = \"{first_aud}\"")));

    // The status names the refusal first.
    t.cf.with(|s| {
        s.access_problem = Some(json!({
            "reason": "access token audience mismatch: expected old-aud, got new-aud",
            "at": 1_790_812_700,
        }));
    });
    let v = status(&mut t);
    assert_eq!(v["state"], "yellow");
    assert_eq!(
        v["problems"][0],
        "Phone requests are being refused by the capture server: access token audience mismatch: expected old-aud, got new-aud."
    );
    assert_eq!(v["text"], v["problems"][0]);
    assert_eq!(v["access_problem"]["at"], 1_790_812_700);

    // Nothing differs: no change, no restart.
    let out = t.b.run("capture.check_access", json!({}));
    assert_eq!(out["ok"], true, "{out}");
    assert_eq!(out["changes"], json!([]));
    assert_eq!(t.host.with(|s| s.aud_restarts).unwrap(), 0);
    assert!(
        t.cf.cf_calls()
            .contains(&format!("GET /accounts/acc1/access/apps/{id}"))
    );

    // Cloudflare's application has another AUD tag now.
    t.cf.with(|s| s.apps.get_mut(&id).unwrap()["aud"] = json!("rotated-aud"));
    let out = t.b.run("capture.check_access", json!({}));
    assert_eq!(out["ok"], true, "{out}");
    assert!(
        config().contains("access_aud = \"rotated-aud\""),
        "{}",
        config()
    );
    assert_eq!(t.host.with(|s| s.aud_restarts).unwrap(), 1, "restarted");
    // release-packaging.md: restarted, never replaced (no update without Update now).
    assert!(
        t.host.with(|s| s.updates.is_empty()).unwrap(),
        "not the update path"
    );
    let changes = out["changes"].to_string();
    assert!(
        changes.contains("AUD tag recorded: rotated-aud"),
        "{changes}"
    );
    assert!(changes.contains("server.toml access_aud set to rotated-aud"));
    let s = t.b.run("capture.settings", json!({}));
    let details = s["setup"]["details"].to_string();
    assert!(
        details.contains("RECORD") && details.contains("CONFIGURE"),
        "{details}"
    );
    // The recorded AUD matches but server.toml does not (the owner's report).
    t.host.with(|s| {
        s.config_text = s.config_text.replace("rotated-aud", &first_aud);
    });
    let out = t.b.run("capture.check_access", json!({}));
    assert!(config().contains("access_aud = \"rotated-aud\""));
    assert_eq!(t.host.with(|s| s.aud_restarts).unwrap(), 2);
    assert!(!out["changes"].to_string().contains("AUD tag recorded"));

    // The application's domain differs: PUT with the address, the rest unchanged.
    t.cf.with(|s| s.apps.get_mut(&id).unwrap()["domain"] = json!("capture.old.example.com"));
    let out = t.b.run("capture.check_access", json!({}));
    assert_eq!(out["ok"], true, "{out}");
    let put = call_body(&t, "PUT", &format!("/access/apps/{id}"));
    assert_eq!(put.len(), 1);
    assert_eq!(put[0]["domain"], "capture.example.com");
    assert_eq!(put[0]["session_duration"], "720h");
    assert_eq!(put[0]["policies"].as_array().unwrap().len(), 2);
    assert!(put[0].get("aud").is_none() && put[0].get("id").is_none());
    assert!(
        out["changes"]
            .to_string()
            .contains("address set to capture.example.com")
    );
    t.cf.with(|s| assert_eq!(s.apps[&id]["domain"], "capture.example.com"));

    // The application is gone: reported, then made again on request.
    t.cf.with(|s| s.apps.clear());
    let out = t.b.run("capture.check_access", json!({}));
    assert_eq!(out["ok"], false);
    assert_eq!(out["app_missing"], true);
    assert_eq!(
        out["text"],
        "The Access application for capture.example.com no longer exists at Cloudflare. Create it again?"
    );
    let out = t.b.run("capture.check_access", json!({"recreate": true}));
    assert_eq!(out["ok"], true, "{out}");
    let new_id =
        t.cf.with(|s| s.apps.keys().next().cloned())
            .unwrap()
            .unwrap();
    assert_ne!(new_id, id);
    assert!(config().contains(&format!("access_aud = \"aud-{new_id}\"")));
    assert_eq!(t.host.with(|s| s.aud_restarts).unwrap(), 3);
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["setup"]["complete"], true);

    // Before setup there is nothing to check.
    let mut fresh = setup_book();
    check(&mut fresh);
    assert_eq!(
        fresh.b.err("capture.check_access", json!({})).code(),
        "not_set_up"
    );
}

// ----- release-packaging.md --------------------------------------------------------

/// A server reporting this capture version (`cv`) and build version.
fn running_capture(t: &T, cv: &str, version: &str) {
    t.cf.with(|s| {
        s.server_capture_version = Some(cv.to_owned());
        s.server_version = version.to_owned();
    });
}

/// RP3: a running server with another capture version and this build's program
/// beside the app: the update notice with Update now; equal capture versions: none,
/// even when the build versions differ (a desktop-only change).
#[test]
fn rp3_capture_version_decides_the_notice() {
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    running_capture(&t, "0.0.1+000000000000", mk_capture_api::BUILD_VERSION);
    let v = status(&mut t);
    assert_eq!(v["state"], "yellow");
    assert_eq!(v["update_needed"], true);
    assert_eq!(v["update_ready"], true);
    assert_eq!(v["server_capture_version"], "0.0.1+000000000000");
    assert_eq!(v["app_capture_version"], mk_capture_api::CAPTURE_VERSION);
    // Equal capture versions, other build version: green, no notice.
    running_capture(
        &t,
        mk_capture_api::CAPTURE_VERSION,
        "0.0.0+100.aaaaaaaaaaaa",
    );
    let v = status(&mut t);
    assert_eq!(v["state"], "green");
    assert_eq!(v["update_needed"], false);
    assert_eq!(v["update_ready"], false);
}

/// RP4: the program beside the app is not this build's (another capture version,
/// none, or no phone app): the notice without Update now.
#[test]
fn rp4_program_not_from_this_build() {
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    running_capture(&t, "0.0.1+000000000000", mk_capture_api::BUILD_VERSION);
    type Change = Box<dyn Fn(&mut crate::capture_setup::HostReport)>;
    let changes: [(&str, Change); 4] = [
        (
            "older program",
            Box::new(|r| r.program_capture_version = "0.0.1+000000000000".into()),
        ),
        (
            "no capture version",
            Box::new(|r| r.program_capture_version.clear()),
        ),
        ("no app", Box::new(|r| r.program_has_app = false)),
        ("missing", Box::new(|r| r.server_program = false)),
    ];
    for (what, f) in changes {
        let saved = t.host.with(|s| s.report.clone()).unwrap();
        t.host.with(|s| f(&mut s.report));
        let v = status(&mut t);
        assert_eq!(v["update_needed"], true, "{what}");
        assert_eq!(v["update_ready"], false, "{what}");
        t.host.with(|s| s.report = saved);
    }
}

/// RP5: Update now with the permission declined says so and changes nothing;
/// accepted, it says "Capture server updated to <version>."; the same capture
/// version already running is refused.
#[test]
fn rp5_update_now_answers() {
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    running_capture(&t, "0.0.1+000000000000", mk_capture_api::BUILD_VERSION);
    t.host
        .with(|s| s.fail_update = Some(crate::capture_setup::PERMISSION_DECLINED.into()));
    let e = t.b.err("capture.update_server", json!({}));
    assert_eq!(e.code(), "permission_declined");
    assert_eq!(e.to_string(), text::NOT_UPDATED);
    assert_eq!(
        text::NOT_UPDATED,
        "Not updated — Windows permission was not given. Phones can keep sending to the current server; press Update now when ready."
    );
    assert!(t.host.with(|s| s.updates.is_empty()).unwrap());
    t.host.with(|s| s.fail_update = None);
    let out = t.b.run("capture.update_server", json!({}));
    assert_eq!(out["updated"], true);
    let short = mk_capture_api::package_version(mk_capture_api::BUILD_VERSION);
    assert_eq!(out["text"], format!("Capture server updated to {short}."));
    assert_eq!(t.host.with(|s| s.updates.len()).unwrap(), 1);
    running_capture(
        &t,
        mk_capture_api::CAPTURE_VERSION,
        mk_capture_api::BUILD_VERSION,
    );
    let e = t.b.err("capture.update_server", json!({}));
    assert_eq!(e.code(), "already_this_build");
}

/// The update never starts by itself: status checks, Check (a changed Access tag)
/// and collecting only restart the server, never replace it.
#[test]
fn no_update_without_update_now() {
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    running_capture(&t, "0.0.1+000000000000", mk_capture_api::BUILD_VERSION);
    for _ in 0..3 {
        let _ = status(&mut t);
    }
    let _ = t.b.try_run("capture.collect", json!({}));
    assert!(t.host.with(|s| s.updates.is_empty()).unwrap());
}

// ----- mobile-capture.md revision 16 ----------------------------------------------------------

/// The tunnel id the leftover cloudflared service runs (as its token shows it).
const OLD_TUNNEL: &str = "0d5c0a2e-6f1b-4c3d-9e8f-112233445566";

fn leftover_service(t: &T, tunnel: &str, account: Option<&str>) {
    t.host.with(|s| {
        s.report.cloudflared_installed = true;
        s.report.cloudflared_service = Some(ServiceState::Running);
        s.report.cloudflared_tunnel = Some(tunnel.into());
        s.report.cloudflared_account = account.map(str::to_owned);
    });
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_owned())
        .collect()
}

/// Test 88: a cloudflared service whose tunnel is not in the account → the replace
/// message, Replace preselected and Install available; Leave keeps Install
/// unavailable with the reason; Replace happens in the install step itself.
#[test]
fn r16_t88_service_of_a_deleted_tunnel_is_replaced() {
    let mut t = setup_book();
    leftover_service(&t, OLD_TUNNEL, Some("acc1"));
    let v = check(&mut t);
    assert_eq!(v["suggest"], "new", "{v}");
    let plan = t.b.run("capture.setup_plan", new_tunnel_args());
    assert_eq!(plan["problems"], json!([]), "{plan}");
    assert_eq!(plan["service"]["state"], "deleted");
    assert_eq!(plan["service"]["message"], text::REPLACE_MESSAGE);
    assert_eq!(plan["service"]["replace"], true);
    assert_eq!(
        text::REPLACE_MESSAGE,
        "A cloudflared service on this computer runs a tunnel that no longer exists in your Cloudflare account. MoneyKeep can replace it with the new tunnel."
    );
    assert!(strings(&plan["actions"]).contains(&text::REPLACE_STEP.to_owned()));
    assert!(
        t.cf.cf_calls()
            .contains(&format!("GET /accounts/acc1/cfd_tunnel/{OLD_TUNNEL}")),
        "the service's tunnel is looked up"
    );
    // Leave it: Install stays unavailable, with the reason.
    let mut leave = new_tunnel_args();
    leave["replace_service"] = json!(false);
    let plan = t.b.run("capture.setup_plan", leave.clone());
    assert_eq!(plan["problems"], json!([text::LEAVE_REASON]));
    assert_eq!(plan["service"]["replace"], false);
    assert!(!strings(&plan["actions"]).contains(&text::REPLACE_STEP.to_owned()));
    let e = t.b.err("capture.setup_apply", leave);
    assert_eq!(e.code(), "setup_blocked");
    assert!(t.host.with(|s| s.installs.is_empty()).unwrap());
    // Replace it (the default): one install step replaces the service.
    let out = apply(&mut t, new_tunnel_args());
    assert_eq!(out["complete"], true, "{out}");
    let installs = t.host.with(|s| s.installs.clone()).unwrap();
    assert_eq!(installs.len(), 1, "one elevated step");
    assert_eq!(
        installs[0].replace_cloudflared_tunnel.as_deref(),
        Some(OLD_TUNNEL)
    );
    assert!(installs[0].tunnel_token.is_some());
    let details = out["details"].as_array().unwrap();
    assert!(
        details.iter().any(|d| d["method"] == "REPLACE"
            && d["result"]
                .as_str()
                .unwrap()
                .starts_with(text::REPLACE_STEP)),
        "{details:#?}"
    );
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["setup"]["installed_cloudflared"], true);
}

/// Test 88 (the lookup): a 404, or a tunnel with `deleted_at`, is gone; a live
/// tunnel exists; a failed lookup is not known (never replaced).
#[test]
fn r16_t88_service_tunnel_lookup() {
    use crate::capture_setup::{HostReport, ServiceTunnel, service_tunnel_state};
    use crate::connect::HttpResponse;
    let host = HostReport {
        cloudflared_tunnel: Some(OLD_TUNNEL.into()),
        cloudflared_account: Some("acc1".into()),
        ..HostReport::default()
    };
    let answer = |status: u16, body: Value| {
        Ok(HttpResponse {
            status,
            body: body.to_string().into_bytes(),
            retry_after: None,
            deprecation: None,
        })
    };
    let deleted = answer(
        200,
        json!({"success": true, "errors": [], "result": {"id": OLD_TUNNEL, "name": "old", "deleted_at": "2026-11-01T10:00:00Z"}}),
    );
    assert_eq!(
        service_tunnel_state("acc1", &host, Some(&deleted)),
        ServiceTunnel::Gone(OLD_TUNNEL.into())
    );
    let missing = answer(
        404,
        json!({"success": false, "errors": [{"code": 1003, "message": "Tunnel not found"}], "result": null}),
    );
    assert_eq!(
        service_tunnel_state("acc1", &host, Some(&missing)),
        ServiceTunnel::Gone(OLD_TUNNEL.into())
    );
    let live = answer(
        200,
        json!({"success": true, "errors": [], "result": {"id": OLD_TUNNEL, "name": "home", "deleted_at": null}}),
    );
    assert_eq!(
        service_tunnel_state("acc1", &host, Some(&live)),
        ServiceTunnel::Exists("home".into())
    );
    let refused = answer(
        403,
        json!({"success": false, "errors": [{"code": 10000, "message": "Authentication error"}], "result": null}),
    );
    assert_eq!(
        service_tunnel_state("acc1", &host, Some(&refused)),
        ServiceTunnel::Unknown
    );
    let unknown = HostReport::default();
    assert_eq!(
        service_tunnel_state("acc1", &unknown, Some(&deleted)),
        ServiceTunnel::Unknown
    );
    // Revision 16a: another account's tunnel is not "deleted", whatever this
    // account answers.
    assert_eq!(
        service_tunnel_state("acc2", &host, Some(&missing)),
        ServiceTunnel::OtherAccount(OLD_TUNNEL.into())
    );
}

/// Test 89: a cloudflared service whose tunnel exists → "Add to my existing tunnel"
/// (that tunnel suggested), never replaced.
#[test]
fn r16_t89_service_of_a_live_tunnel() {
    let mut t = setup_book();
    let catch_all = json!({"service": "http_status:404"});
    t.cf.add_tunnel("tun-other", "other", json!([catch_all]));
    t.cf.add_tunnel("tun-home", "home", json!([catch_all]));
    leftover_service(&t, "tun-home", Some("acc1"));
    let v = check(&mut t);
    assert_eq!(v["suggest"], "existing");
    assert_eq!(v["suggest_tunnel"], "tun-home");
    let plan = t.b.run("capture.setup_plan", new_tunnel_args());
    let problems = plan["problems"].to_string();
    assert!(problems.contains("it was not replaced"), "{plan}");
    assert!(
        problems.contains("Add to my existing tunnel home"),
        "{plan}"
    );
    assert!(plan["service"].is_null());
    let e = t.b.err("capture.setup_apply", new_tunnel_args());
    assert_eq!(e.code(), "setup_blocked");
    assert!(t.host.with(|s| s.installs.is_empty()).unwrap());
    // Adding to it works as before.
    let out = apply(
        &mut t,
        json!({"emails": ["owner@example.com"], "tunnel": "tun-home"}),
    );
    assert_eq!(out["complete"], true);
    let installs = t.host.with(|s| s.installs.clone()).unwrap();
    assert!(installs[0].tunnel_token.is_none());
    assert!(installs[0].replace_cloudflared_tunnel.is_none());
}

/// Test 90: removing MoneyKeep's rule always leaves a configuration that ends with
/// the catch-all: alone on a tunnel without other rules, after the other rules
/// (in order) otherwise; a tunnel setup created is deleted, not edited.
#[test]
fn r16_t90_undo_keeps_a_valid_ingress_configuration() {
    let catch_all = json!({"service": "http_status:404"});
    // Unit: no rules at all, only ours, ours among others.
    let ours = json!({"hostname": "capture.example.com", "service": "http://127.0.0.1:8787"});
    let a = json!({"hostname": "a.example.com", "service": "http://localhost:1"});
    let b = json!({"hostname": "b.example.com", "service": "http://localhost:2"});
    let without = |ingress: Value| {
        crate::capture_setup::without_host(&json!({"ingress": ingress}), "capture.example.com")
            ["ingress"]
            .clone()
    };
    assert_eq!(without(json!([])), json!([catch_all]));
    assert_eq!(without(json!([ours])), json!([catch_all]));
    assert_eq!(
        without(json!([a, ours, b, {"service": "http_status:503"}])),
        json!([a, b, {"service": "http_status:503"}])
    );
    assert_eq!(
        crate::capture_setup::without_host(&json!({}), "capture.example.com")["ingress"],
        json!([catch_all])
    );
    // A tunnel with no other rules (the owner's report): the undo writes only the
    // catch-all, and Cloudflare accepts it.
    let mut t = setup_book();
    t.cf.add_tunnel("tun-bare", "bare", json!([]));
    check(&mut t);
    apply(
        &mut t,
        json!({"emails": ["owner@example.com"], "tunnel": "tun-bare"}),
    );
    let out = t.b.run("capture.setup_remove", json!({}));
    assert_eq!(out["removed"], true, "{out}");
    let puts = call_body(&t, "PUT", "/cfd_tunnel/tun-bare/configurations");
    assert_eq!(
        puts.last().unwrap()["config"]["ingress"],
        json!([{"service": "http_status:404"}])
    );
    // Other rules are kept in order before the catch-all.
    let mut t = setup_book();
    t.cf.add_tunnel("tun-home", "home", json!([a, b, catch_all]));
    check(&mut t);
    apply(
        &mut t,
        json!({"emails": ["owner@example.com"], "tunnel": "tun-home"}),
    );
    t.b.run("capture.setup_remove", json!({}));
    t.cf.with(|s| {
        assert_eq!(
            s.tunnels.get("tun-home").unwrap().1["ingress"],
            json!([a, b, catch_all])
        );
    });
    // A tunnel this setup created is deleted, its configuration never written.
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    let before = t.cf.cf_calls().len();
    t.b.run("capture.setup_remove", json!({}));
    let calls = t.cf.cf_calls()[before..].to_vec();
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("DELETE /accounts/acc1/cfd_tunnel/")),
        "{calls:#?}"
    );
    assert!(!calls.iter().any(|c| c.contains("/configurations")));
}

/// Test 91: a failing undo step is listed with its reason (and where to remove it
/// by hand); the other steps still run; setup is not shown as removed while
/// anything is left; Retry undo finishes it.
#[test]
fn r16_t91_failed_undo_step_listed_and_retried() {
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    t.cf.with(|s| {
        s.granted.remove(PERMISSIONS[4]);
    });
    let out = t.b.run("capture.setup_remove", json!({}));
    assert_eq!(out["removed"], false, "{out}");
    assert_eq!(out["text"], text::UNDO_INCOMPLETE);
    let failed = out["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1, "{failed:#?}");
    assert_eq!(failed[0]["what"], "The DNS record capture.example.com");
    assert!(
        failed[0]["reason"]
            .as_str()
            .unwrap()
            .contains("Authentication error"),
        "{failed:#?}"
    );
    assert!(failed[0]["where"].as_str().unwrap().contains("DNS"));
    assert_eq!(
        out["left"],
        json!([{"what": "The DNS record capture.example.com",
                "where": "Cloudflare dashboard: your domain > DNS > Records"}])
    );
    // The other steps ran.
    t.cf.with(|s| {
        assert!(s.apps.is_empty() && s.policies.is_empty() && s.service_tokens.is_empty());
        assert!(s.tunnels.is_empty());
        assert_eq!(s.dns.len(), 1);
    });
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["setup"]["started"], true, "not shown as removed");
    assert_eq!(
        s["setup"]["resources"],
        json!(["The DNS record capture.example.com"])
    );
    // Retry undo.
    t.cf.with(|s| s.granted = PERMISSIONS.iter().copied().collect());
    let out = t.b.run("capture.setup_remove", json!({}));
    assert_eq!(out["removed"], true, "{out}");
    t.cf.with(|s| assert!(s.dns.is_empty()));
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["setup"]["started"], false);
}

// ----- mobile-capture.md revision 16a and 17 --------------------------------------------------

/// Test 89a: a cloudflared service whose tunnel is in another Cloudflare account is
/// not treated as deleted: the other-account message, Leave it preselected (Install
/// unavailable with the reason), Replace it available with the warning and then
/// replacing that exact service in the install step; only a 404 / `deleted_at` in
/// the same account preselects Replace it (test 88).
#[test]
fn r16a_t89a_tunnel_in_another_account_is_left_alone() {
    let mut t = setup_book();
    leftover_service(&t, OLD_TUNNEL, Some("other-account"));
    let v = check(&mut t);
    assert_eq!(v["suggest"], "new");
    let plan = t.b.run("capture.setup_plan", new_tunnel_args());
    assert_eq!(plan["service"]["state"], "other_account", "{plan}");
    assert_eq!(plan["service"]["message"], text::OTHER_ACCOUNT_MESSAGE);
    assert_eq!(
        text::OTHER_ACCOUNT_MESSAGE,
        "A cloudflared service on this computer runs a tunnel in another Cloudflare account. MoneyKeep leaves it alone unless you choose to replace it."
    );
    assert_eq!(
        plan["service"]["warning"],
        "Anything that tunnel serves will stop working."
    );
    assert_eq!(plan["service"]["replace"], false, "Leave it preselected");
    assert_eq!(plan["problems"], json!([text::OTHER_ACCOUNT_LEAVE_REASON]));
    assert!(!strings(&plan["actions"]).contains(&text::REPLACE_STEP.to_owned()));
    let e = t.b.err("capture.setup_apply", new_tunnel_args());
    assert_eq!(e.code(), "setup_blocked");
    assert!(t.host.with(|s| s.installs.is_empty()).unwrap());
    // Replace it, chosen: available, and the install replaces exactly that service.
    let mut replace = new_tunnel_args();
    replace["replace_service"] = json!(true);
    let plan = t.b.run("capture.setup_plan", replace.clone());
    assert_eq!(plan["problems"], json!([]), "{plan}");
    assert_eq!(plan["service"]["replace"], true);
    let out = apply(&mut t, replace);
    assert_eq!(out["complete"], true, "{out}");
    let installs = t.host.with(|s| s.installs.clone()).unwrap();
    assert_eq!(
        installs[0].replace_cloudflared_tunnel.as_deref(),
        Some(OLD_TUNNEL)
    );
    // Same account, deleted: Replace it preselected (unchanged).
    let mut t = setup_book();
    leftover_service(&t, OLD_TUNNEL, Some("acc1"));
    check(&mut t);
    let plan = t.b.run("capture.setup_plan", new_tunnel_args());
    assert_eq!(plan["service"]["state"], "deleted");
    assert_eq!(plan["service"]["replace"], true);
}

use crate::capture_start_over::text as so_text;

/// Cloudflare (a new tunnel) and Wi-Fi capture with a PIN, both set up.
fn both_set_up() -> T {
    let mut t = setup_book();
    t.host.with(|s| {
        s.data_folder = true;
        s.addresses = vec![crate::capture_setup::LocalAddress {
            address: "192.168.1.20".into(),
            adapter: "Wi-Fi".into(),
            profile: "Private".into(),
            ..Default::default()
        }];
    });
    check(&mut t);
    let out = apply(&mut t, new_tunnel_args());
    assert_eq!(out["complete"], true, "{out}");
    t.b.run(
        "capture.wifi_apply",
        json!({"address": "192.168.1.20", "pin": "246813"}),
    );
    t
}

fn whats(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| x["what"].as_str().unwrap().to_owned())
        })
        .collect()
}

fn go(t: &mut T) -> Value {
    t.b.run("capture.start_over", json!({"confirm": "start over"}))
}

fn step<'a>(v: &'a Value, what_part: &str) -> &'a Value {
    v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["what"].as_str().unwrap().contains(what_part))
        .unwrap_or_else(|| panic!("{what_part} not in {v:#}"))
}

/// Test 92: the preview lists the three groups and Kept and changes nothing; the
/// run needs `START OVER` (any case).
#[test]
fn r17_t92_preview_changes_nothing_and_needs_the_words() {
    let mut t = both_set_up();
    let state = t.b.state();
    let audit = t.b.audit_len();
    let calls = t.cf.cf_calls().len();
    let p = t.b.run("capture.start_over_preview", json!({}));
    let cloudflare = whats(&p["cloudflare"]["items"]);
    for part in [
        "The Access application for capture.example.com",
        "The Access policy",
        "The Access service token",
        "The DNS record capture.example.com",
        "The tunnel \"moneykeep-capture\"",
    ] {
        assert!(cloudflare.iter().any(|c| c.contains(part)), "{part}: {p:#}");
    }
    assert!(p["cloudflare"]["problem"].is_null());
    let computer = whats(&p["computer"]);
    assert!(
        computer
            .iter()
            .any(|c| c.contains("capture server service")),
        "{p:#}"
    );
    assert!(
        computer
            .iter()
            .any(|c| c.contains("cloudflared service (MoneyKeep installed it)"))
    );
    assert!(computer.iter().any(|c| c.contains("Wi-Fi firewall rule")));
    assert!(computer.iter().any(|c| c.contains("data folder")));
    assert_eq!(p["permission"], so_text::PERMISSION);
    let mk = whats(&p["moneykeep"]);
    assert!(mk.contains(&"The Capture PIN".to_owned()), "{p:#}");
    assert!(mk.contains(&"The saved Cloudflare API token".to_owned()));
    assert!(mk.iter().any(|m| m.contains("setup state")));
    assert!(mk.contains(&"The list of paired phones".to_owned()));
    let kept = whats(&p["kept"]);
    assert!(kept.contains(&"Your books".to_owned()) && kept.contains(&"Your backups".to_owned()));
    assert!(kept.contains(&"The MoneyKeep program".to_owned()));
    assert_eq!(p["confirm_word"], "START OVER");
    // Nothing changed: no Cloudflare writes, no host change, no audit entry.
    assert_eq!(t.b.state(), state);
    assert_eq!(t.b.audit_len(), audit);
    assert!(
        t.cf.cf_calls()[calls..]
            .iter()
            .all(|c| c.starts_with("GET ")),
        "{:?}",
        &t.cf.cf_calls()[calls..]
    );
    assert!(t.host.with(|s| s.start_overs.is_empty()).unwrap());
    // The words are needed.
    for wrong in ["", "start", "STARTOVER", "RESET"] {
        let e = t.b.err("capture.start_over", json!({"confirm": wrong}));
        assert_eq!(e.code(), "confirm_needed", "{wrong}");
    }
    assert_eq!(t.b.state(), state);
}

/// Test 93: a setup-created tunnel, its Access app, policies, service token and DNS
/// record are deleted; with an existing tunnel only MoneyKeep's rule is removed and
/// the catch-all kept. Settings show nothing set up; an audit entry is written
/// without the token or the PIN.
#[test]
fn r17_t93_cloudflare_items_removed() {
    let mut t = both_set_up();
    let out = go(&mut t);
    assert_eq!(out["cleared"], true, "{out:#}");
    assert_eq!(out["text"], so_text::CLEARED);
    assert_eq!(
        so_text::CLEARED,
        "Phone capture is cleared. Set up phone capture to begin again."
    );
    t.cf.with(|s| {
        assert!(s.tunnels.is_empty() && s.dns.is_empty() && s.apps.is_empty());
        assert!(s.policies.is_empty() && s.service_tokens.is_empty());
    });
    assert_eq!(step(&out, "The DNS record")["result"], "removed");
    assert_eq!(step(&out, "data folder")["result"], "removed");
    assert_eq!(step(&out, "Capture PIN")["result"], "removed");
    let req = t.host.with(|s| s.start_overs.clone()).unwrap();
    assert_eq!(req.len(), 1, "one elevated step");
    assert!(req[0].server && req[0].cloudflared && req[0].data_folder);
    assert!(t.host.with(|s| s.firewall_rules.is_empty()).unwrap());
    assert_eq!(
        t.host.with(|s| s.report.server).unwrap(),
        ServiceState::NotInstalled
    );
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["configured"], false, "{s:#}");
    assert_eq!(s["setup"]["started"], false);
    assert_eq!(s["setup"]["has_cloudflare_token"], false);
    assert_eq!(s["wifi"]["started"], false);
    assert_eq!(s["wifi"]["pin_set"], false);
    let log = t.b.s.audit_entries(5).unwrap();
    let entry = log
        .iter()
        .find(|e| e.command == "capture.start_over")
        .unwrap();
    let text = format!("{} {}", entry.args, entry.result);
    assert!(
        !text.contains(TEST_TOKEN) && !text.contains("246813") && !text.contains(DESKTOP_TOKEN)
    );
    assert_eq!(entry.args["computer"], "removed");
    // An existing tunnel: only MoneyKeep's rule goes, the catch-all is kept.
    let mut t = setup_book();
    let a = json!({"hostname": "a.example.com", "service": "http://localhost:1"});
    let catch_all = json!({"service": "http_status:404"});
    t.cf.add_tunnel("tun-home", "home", json!([a, catch_all]));
    check(&mut t);
    apply(
        &mut t,
        json!({"emails": ["owner@example.com"], "tunnel": "tun-home"}),
    );
    let p = t.b.run("capture.start_over_preview", json!({}));
    assert!(
        whats(&p["kept"])
            .iter()
            .any(|k| k.contains("Your tunnel \"home\" (only MoneyKeep's rule")),
        "{p:#}"
    );
    let out = go(&mut t);
    assert_eq!(out["cleared"], true, "{out:#}");
    t.cf.with(|s| {
        assert_eq!(
            s.tunnels.get("tun-home").unwrap().1["ingress"],
            json!([a, catch_all])
        );
    });
}

/// Test 94: a cloudflared service for an existing tunnel MoneyKeep did not install
/// is kept; for a deleted tunnel it is removed.
#[test]
fn r17_t94_cloudflared_service_kept_or_removed() {
    // Wi-Fi only, with the owner's own cloudflared for a live tunnel.
    let mut t = setup_book();
    t.cf.add_tunnel("tun-home", "home", json!([{"service": "http_status:404"}]));
    check(&mut t);
    leftover_service(&t, "tun-home", Some("acc1"));
    let p = t.b.run("capture.start_over_preview", json!({}));
    assert!(
        whats(&p["kept"]).contains(&"The cloudflared service for your tunnel \"home\"".to_owned()),
        "{p:#}"
    );
    assert!(
        !whats(&p["computer"])
            .iter()
            .any(|c| c.contains("cloudflared"))
    );
    go(&mut t);
    assert!(
        t.host
            .with(|s| s.report.cloudflared_service.is_some())
            .unwrap()
    );
    assert!(!t.host.with(|s| s.start_overs[0].cloudflared).unwrap());
    assert!(t.cf.with(|s| s.tunnels.contains_key("tun-home")).unwrap());
    // A deleted tunnel's service is removed (exactly that one).
    let mut t = setup_book();
    check(&mut t);
    leftover_service(&t, OLD_TUNNEL, Some("acc1"));
    let p = t.b.run("capture.start_over_preview", json!({}));
    assert!(
        whats(&p["computer"])
            .iter()
            .any(|c| c.contains("its tunnel no longer exists")),
        "{p:#}"
    );
    go(&mut t);
    let req = t.host.with(|s| s.start_overs[0].clone()).unwrap();
    assert!(req.cloudflared);
    assert_eq!(req.cloudflared_tunnel.as_deref(), Some(OLD_TUNNEL));
    assert!(
        t.host
            .with(|s| s.report.cloudflared_service.is_none())
            .unwrap()
    );
    // Revision 16a: another account's tunnel is kept.
    let mut t = setup_book();
    check(&mut t);
    leftover_service(&t, OLD_TUNNEL, Some("other-account"));
    let p = t.b.run("capture.start_over_preview", json!({}));
    assert!(
        whats(&p["kept"])
            .iter()
            .any(|k| k.contains("another Cloudflare account"))
    );
    go(&mut t);
    assert!(
        t.host
            .with(|s| s.report.cloudflared_service.is_some())
            .unwrap()
    );
}

fn inbox_item(n: u8) -> Value {
    json!({
        "id": format!("{n:032x}"),
        "client_id": format!("c{n}"),
        "device_id": format!("{:032x}", 9),
        "device_name": "Pixel",
        "submitted_at": 1_790_800_000,
        "fields": {"date": "2026-10-01", "merchant": format!("Cafe {n}"), "amount": "4.50"},
        "receipts": [],
    })
}

/// Test 95: uncollected items are counted ("3 expenses from phones not yet
/// collected will be lost — collect them first?") and Collect now collects them
/// before clearing.
#[test]
fn r17_t95_uncollected_items_counted_and_collected() {
    let mut t = both_set_up();
    t.cf.with(|s| s.inbox = (1..=3).map(inbox_item).collect());
    let p = t.b.run("capture.start_over_preview", json!({}));
    assert_eq!(p["uncollected"], 3, "{p:#}");
    assert_eq!(
        p["uncollected_text"],
        "3 expenses from phones not yet collected will be lost — collect them first?"
    );
    assert!(
        whats(&p["moneykeep"]).contains(&"3 expenses from phones not yet collected".to_owned())
    );
    let count = |t: &mut T| {
        let v = t.b.run("expense.list", json!({}));
        v.as_array()
            .or_else(|| v["expenses"].as_array())
            .map_or(0, Vec::len)
    };
    let before = count(&mut t);
    let c = t.b.run("capture.collect", json!({}));
    assert_eq!(c["expenses"], 3, "{c:#}");
    assert_eq!(count(&mut t), before + 3);
    let p = t.b.run("capture.start_over_preview", json!({}));
    assert_eq!(p["uncollected"], 0);
    assert!(p["uncollected_text"].is_null());
    go(&mut t);
    assert_eq!(
        count(&mut t),
        before + 3,
        "collected expenses stay in the book"
    );
}

/// Test 96: Cloudflare unreachable: the computer and MoneyKeep parts run; the
/// Cloudflare items are listed as left with their dashboard places. The same when
/// no token is saved.
#[test]
fn r17_t96_cloudflare_unreachable() {
    let mut t = both_set_up();
    t.cf.with(|s| s.cloudflare_down = true);
    let p = t.b.run("capture.start_over_preview", json!({}));
    assert_eq!(p["cloudflare"]["problem"], so_text::UNREACHABLE, "{p:#}");
    assert_eq!(p["cloudflare"]["by_hand"], true);
    let out = go(&mut t);
    assert_eq!(out["cleared"], false, "{out:#}");
    assert_eq!(out["text"], so_text::LEFT);
    for (part, place) in [
        (
            "The Access application",
            "Zero Trust > Access > Applications",
        ),
        (
            "The Access service token",
            "Zero Trust > Access > Service auth",
        ),
        ("The DNS record", "DNS > Records"),
        ("The tunnel", "Zero Trust > Networks > Tunnels"),
    ] {
        let s = step(&out, part);
        assert_eq!(s["result"], "left", "{part}");
        assert!(s["where"].as_str().unwrap().contains(place), "{s}");
        assert!(
            s["reason"].as_str().unwrap().contains("can't be reached"),
            "{s}"
        );
    }
    assert_eq!(step(&out, "data folder")["result"], "removed");
    assert_eq!(step(&out, "Capture PIN")["result"], "removed");
    assert_eq!(t.host.with(|s| s.start_overs.len()).unwrap(), 1);
    t.cf.with(|s| assert_eq!(s.dns.len(), 1, "nothing deleted at Cloudflare"));
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["setup"]["started"], false);
    // No saved token: the same, with the reason.
    let mut t = both_set_up();
    t.b.run("capture.forget_cloudflare_token", json!({}));
    let p = t.b.run("capture.start_over_preview", json!({}));
    assert_eq!(p["cloudflare"]["problem"], so_text::NO_TOKEN);
    let out = go(&mut t);
    assert_eq!(step(&out, "The DNS record")["result"], "left");
    assert_eq!(step(&out, "data folder")["result"], "removed");
}

/// Test 97: the permission prompt declined: Cloudflare and MoneyKeep parts done;
/// the computer part listed as left with Try again, which finishes it.
#[test]
fn r17_t97_permission_declined_then_try_again() {
    let mut t = both_set_up();
    t.host.with(|s| s.decline_start_over = true);
    let out = go(&mut t);
    assert_eq!(out["cleared"], false, "{out:#}");
    assert_eq!(out["declined"], true);
    assert_eq!(out["computer_left"], true);
    let s = step(&out, "capture server service");
    assert_eq!(s["result"], "left");
    assert_eq!(s["reason"], so_text::DECLINED);
    assert!(s["where"].as_str().unwrap().contains("Try again"));
    assert_eq!(step(&out, "The DNS record")["result"], "removed");
    assert_eq!(step(&out, "Capture PIN")["result"], "removed");
    t.cf.with(|s| assert!(s.dns.is_empty() && s.tunnels.is_empty()));
    assert_eq!(
        t.host.with(|s| s.report.server).unwrap(),
        ServiceState::Running,
        "nothing changed on this computer"
    );
    // Try again: the computer part, including MoneyKeep's own cloudflared service.
    t.host.with(|s| s.decline_start_over = false);
    let p = t.b.run("capture.start_over_preview", json!({}));
    assert!(
        whats(&p["computer"])
            .iter()
            .any(|c| c.contains("cloudflared service (MoneyKeep installed it)")),
        "{p:#}"
    );
    let out = go(&mut t);
    assert_eq!(out["cleared"], true, "{out:#}");
    t.host.with(|s| {
        assert_eq!(s.report.server, ServiceState::NotInstalled);
        assert!(s.report.cloudflared_service.is_none());
        assert!(!s.data_folder);
    });
}

/// Test 98: the book's contents are unchanged by Start over (only phone capture's
/// own settings inside it are cleared); backups are files Start over never opens
/// (moneykeep-server's test 98 checks the data folder clearing leaves files beside
/// it byte for byte).
#[test]
fn r17_t98_book_contents_unchanged() {
    let mut t = both_set_up();
    t.b.transfer(
        "2026-10-02",
        "Expenses:Groceries",
        "Assets:Checking",
        "12.34",
    );
    let accounts = t.b.run("account.list", json!({"include_archived": true}));
    let txns = t.b.run("txn.list", json!({"include_voided": true}));
    let modules = t.b.run("module.list", json!({}));
    go(&mut t);
    assert_eq!(
        t.b.run("account.list", json!({"include_archived": true})),
        accounts
    );
    assert_eq!(t.b.run("txn.list", json!({"include_voided": true})), txns);
    assert_eq!(t.b.run("module.list", json!({})), modules);
}

#[test]
fn r17_start_over_not_over_mcp() {
    for name in ["capture.start_over", "capture.start_over_preview"] {
        assert!(NOT_OVER_MCP.contains(&name), "{name}");
        assert!(Registry::with_builtins().spec(name).is_some());
    }
}

// ----- mobile-capture.md revision 16c --------------------------------------------------------

fn desktop_calls(t: &T) -> Vec<(String, bool)> {
    t.cf.with(|s| {
        s.calls
            .iter()
            .filter(|c| c.path.starts_with("/api/v1/desktop/inbox"))
            .map(|c| {
                let id = c.headers.iter().any(|(k, _)| k == "CF-Access-Client-Id");
                let secret = c
                    .headers
                    .iter()
                    .any(|(k, _)| k == "CF-Access-Client-Secret");
                (c.authority.clone(), id && secret)
            })
            .collect()
    })
    .unwrap()
}

/// Test 89c: with the public address saved and the server on this computer,
/// collection goes through loopback ("Collected through this computer"); when
/// loopback can't be reached (or the server is elsewhere) it goes through
/// Cloudflare with both Access headers; an Access redirect gives the Access
/// message and a server 401 the token message.
#[test]
fn r16c_t89c_collect_through_loopback_or_cloudflare() {
    let mut t = setup_book();
    check(&mut t);
    let out = apply(&mut t, new_tunnel_args());
    assert_eq!(out["complete"], true, "{out}");
    t.b.run(
        "capture.set_server",
        json!({"url": "https://capture.example.com", "token": DESKTOP_TOKEN}),
    );
    let v = t.b.run("capture.collect", json!({}));
    assert_eq!(v["route"], "this_computer", "{v:#}");
    assert_eq!(v["route_text"], "Collected through this computer");
    let calls = desktop_calls(&t);
    assert_eq!(calls.last().unwrap(), &("127.0.0.1:8787".to_owned(), false));
    let s = t.b.run("capture.settings", json!({}));
    assert_eq!(s["last_route_text"], "Collected through this computer");
    // Loopback can't be reached: through Cloudflare, with both Access headers.
    t.cf.with(|s| s.loopback_down = true);
    let v = t.b.run("capture.collect", json!({}));
    assert_eq!(v["route"], "cloudflare", "{v:#}");
    assert_eq!(v["route_text"], "Collected through Cloudflare");
    assert_eq!(
        desktop_calls(&t).last().unwrap(),
        &("capture.example.com".to_owned(), true)
    );
    // The server elsewhere (not installed here): straight through Cloudflare.
    t.cf.with(|s| s.loopback_down = false);
    t.b.s
        .write(|tx| -> CommandResult<()> {
            let mut st = tx.get_setting("expenses", "capture.setup")?.unwrap();
            st["installed_server"] = json!(false);
            tx.set_setting("expenses", "capture.setup", &st, "test")?;
            Ok(())
        })
        .unwrap();
    let n = desktop_calls(&t).len();
    let v = t.b.run("capture.collect", json!({}));
    assert_eq!(v["route"], "cloudflare", "{v:#}");
    let calls = desktop_calls(&t);
    assert_eq!(calls.len(), n + 1, "no loopback attempt: {calls:?}");
    assert_eq!(calls[n], ("capture.example.com".to_owned(), true));
    // The service token is gone at Cloudflare's end: Access refuses (redirect).
    t.cf.with(|s| s.service_tokens.clear());
    let e = t.b.err("capture.collect", json!({}));
    assert_eq!(e.code(), "access_refused", "{e}");
    assert!(
        e.to_string().contains(crate::capture::ACCESS_REFUSED),
        "{e}"
    );
    // A server 401 (JSON): the desktop token message.
    let mut t = setup_book();
    check(&mut t);
    apply(&mut t, new_tunnel_args());
    t.cf.with(|s| s.refuse_desktop_token = true);
    let e = t.b.err("capture.collect", json!({}));
    assert_eq!(e.code(), "bad_token", "{e}");
    assert!(e.to_string().contains(crate::capture::TOKEN_REFUSED), "{e}");
    assert!(!e.to_string().contains("Access"));
}
