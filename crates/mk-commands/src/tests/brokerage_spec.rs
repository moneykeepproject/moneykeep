//! connectivity-brokerage.md acceptance tests 1–16 and 18 (17 is in mk-mcp). Answers
//! come from scripted OFX and SnapTrade servers shaped after the OFX 2.2
//! specification and SnapTrade's documentation; no network.

use super::*;
use crate::connect::{Connector, Fetcher, HttpRequest, HttpResponse, MemorySecrets};
use crate::ofx_conn::FakeOfx;
use crate::snap_conn::FakeSnaptrade;

/// 2026-10-04 00:00 UTC.
const NOW: i64 = 1_791_072_000;

fn connector(fetcher: std::sync::Arc<dyn Fetcher>, secrets: &MemorySecrets, now: i64) -> Connector {
    Connector {
        fetcher,
        secrets: Box::new(secrets.clone()),
        utc_offset_secs: 0,
        now: Some(now),
        host: None,
    }
}

struct Ofx {
    b: Book,
    fake: FakeOfx,
    secrets: MemorySecrets,
}

fn ofx_book(fake: FakeOfx) -> Ofx {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "bank-feeds"}));
    b.run("module.enable", json!({"module": "investments"}));
    b.account(
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    let secrets = MemorySecrets::new(true);
    b.r.set_connector(connector(std::sync::Arc::new(fake.clone()), &secrets, NOW));
    Ofx { b, fake, secrets }
}

fn set_now(b: &mut Book, now: i64) {
    if let Some(c) = b.r.connector_mut() {
        c.now = Some(now);
    }
}

fn institution(version: &str) -> Value {
    json!({"name": "Test Bank", "url": "https://ofx.testbank.example/ofx", "org": "TESTBANK",
           "fid": "1234", "broker_id": "broker.example", "ofx_version": version})
}

fn add(p: &mut Ofx, version: &str, extra: Value) -> Value {
    let mut args = json!({"provider": "ofx", "institution": institution(version), "user_id": "user1",
        "password": "pass1", "host_acknowledged": true, "label": "Test Bank"});
    if let (Some(o), Some(e)) = (args.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            o.insert(k.clone(), v.clone());
        }
    }
    p.b.run("connect.add", args)
}

fn remote<'a>(conn: &'a Value, kind: &str) -> &'a Value {
    conn["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["remote"]["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} in {conn}"))
}

fn link(p: &mut Ofx, conn: &Value, kind: &str, account: &str) -> Value {
    let id = remote(conn, kind)["remote_account_id"].clone();
    p.b.run(
        "connect.link_account",
        json!({"connection": conn["id"], "remote_account": id, "account": account}),
    )
}

fn sync(b: &mut Book) -> Value {
    b.run("connect.sync", json!({"now": true}))
}

fn connected(p: &mut Ofx) -> Value {
    p.fake.with(|s| s.cred1_label = None);
    add(p, "103", json!({}))
}

#[test]
fn at1_profile_asks_for_the_extra_credential() {
    let fake = FakeOfx::standard();
    fake.with(|s| s.cred1_label = Some("PIN:".into()));
    let mut p = ofx_book(fake);
    let first = add(&mut p, "103", json!({}));
    assert_eq!(first["needs_credentials"][0]["label"], "PIN:", "{first}");
    assert_eq!(first["needs_credentials"][0]["field"], "cred1");
    let uid = first["client_uid"].as_str().unwrap().to_owned();
    assert_eq!(uid.len(), 36);
    // The profile request signs on anonymously.
    let reqs = p.fake.requests();
    assert!(reqs[0].contains("<PROFRQ>") && reqs[0].contains(mk_import::ofx_dc::ANONYMOUS));
    assert!(!reqs[0].contains("pass1"));
    let c = add(&mut p, "103", json!({"cred1": "4321"}));
    assert_eq!(c["provider"], "ofx", "{c}");
    assert_eq!(c["host"], "ofx.testbank.example");
    assert_eq!(
        c["client_uid"],
        uid.as_str(),
        "the same id per book and computer"
    );
    let last = p.fake.requests().last().unwrap().clone();
    for want in [
        "<USERID>user1",
        "<USERPASS>pass1",
        "<USERCRED1>4321",
        &format!("<CLIENTUID>{uid}"),
        "<APPID>MCRFT",
        "<APPVER>0100",
        "<ACCTINFORQ>",
    ] {
        assert!(last.contains(want), "{want} missing in the signon");
    }
    assert_eq!(
        p.b.run("connect.client_uid", json!({}))["client_uid"],
        uid.as_str()
    );
}

#[test]
fn at2_pending_accounts_cannot_be_linked() {
    let mut p = ofx_book(FakeOfx::standard());
    let c = connected(&mut p);
    assert_eq!(c["accounts"].as_array().unwrap().len(), 3, "{c}");
    let inv = remote(&c, "investment");
    assert_eq!(inv["remote"]["status"], "PEND");
    assert_eq!(inv["remote"]["linkable"], false);
    assert_eq!(remote(&c, "bank")["remote"]["linkable"], true);
    assert_eq!(remote(&c, "card")["remote"]["masked"], "****5678");
    let e = p.b.err(
        "connect.link_account",
        json!({"connection": c["id"], "remote_account": inv["remote_account_id"], "account": "Brokerage"}),
    );
    assert!(e.to_string().contains("PEND"), "{e}");
}

fn rows_of(batch: &Value, section_name_part: &str) -> usize {
    let secs = batch["sections"].as_array().unwrap();
    let idx: Vec<u64> = secs
        .iter()
        .filter(|s| {
            s["account_name"]
                .as_str()
                .unwrap_or_default()
                .contains(section_name_part)
        })
        .map(|s| s["index"].as_u64().unwrap())
        .collect();
    batch["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| idx.contains(&r["section"].as_u64().unwrap()))
        .count()
}

#[test]
fn at3_statements_then_investments_then_duplicates() {
    let mut p = ofx_book(FakeOfx::standard());
    let c = connected(&mut p);
    link(&mut p, &c, "bank", "Assets:Checking");
    link(&mut p, &c, "card", "Liabilities:Visa");
    let out = sync(&mut p.b);
    let r = &out["connections"][0];
    let batch = p.b.run("import.get", json!({"batch": r["batch"]}));
    assert_eq!(rows_of(&batch, "Checking"), 3, "{batch}");
    assert_eq!(rows_of(&batch, "Card"), 2);
    p.b.commit(&batch);
    // The investment account becomes active; the next sync sees it.
    p.fake.with(|s| s.accounts[2].status = "ACTIVE".into());
    set_now(&mut p.b, NOW + 3600);
    sync(&mut p.b);
    let c = p.b.run("connect.get", json!({"connection": c["id"]}));
    assert_eq!(remote(&c, "investment")["remote"]["linkable"], true, "{c}");
    link(&mut p, &c, "investment", "Assets:Brokerage");
    set_now(&mut p.b, NOW + 7200);
    let out = sync(&mut p.b);
    let batch = p.b.run(
        "import.get",
        json!({"batch": out["connections"][0]["batch"]}),
    );
    let inv: Vec<&Value> = batch["inv_rows"].as_array().unwrap().iter().collect();
    let actions: Vec<&str> = inv
        .iter()
        .map(|r| {
            r["action_kind"]
                .as_str()
                .or(r["row"]["action"].as_str())
                .unwrap_or("")
        })
        .collect();
    assert!(
        inv.len() == 2,
        "one buy and one dividend: {actions:?} {batch}"
    );
    let sec = batch["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "investment")
        .unwrap()
        .clone();
    assert_eq!(sec["positions"].as_array().unwrap().len(), 1, "{sec}");
    p.b.commit(&batch);
    set_now(&mut p.b, NOW + 3 * 3600);
    let out = sync(&mut p.b);
    let r = &out["connections"][0];
    assert_eq!(r["new"], 0, "a re-sync stages only duplicates: {r}");
}

fn add_version(version: &str) -> (Ofx, Value) {
    let mut p = ofx_book(FakeOfx::standard());
    p.fake.with(|s| s.cred1_label = None);
    let c = add(&mut p, version, json!({}));
    (p, c)
}

#[test]
fn at4_sgml_and_xml_versions() {
    let (mut p, c) = add_version("102");
    let reqs = p.fake.requests();
    assert!(
        reqs.iter()
            .all(|r| r.starts_with("OFXHEADER:100\r\nDATA:OFXSGML\r\nVERSION:102"))
    );
    assert!(reqs.iter().all(|r| !r.contains("CLIENTUID")));
    link(&mut p, &c, "bank", "Assets:Checking");
    let out = sync(&mut p.b);
    assert!(out["connections"][0]["batch"].is_string(), "{out}");
    let (mut p, c) = add_version("220");
    let reqs = p.fake.requests();
    assert!(
        reqs.iter()
            .all(|r| r.contains("<?OFX OFXHEADER=\"200\" VERSION=\"220\""))
    );
    assert!(reqs.last().unwrap().contains("<CLIENTUID>"));
    link(&mut p, &c, "bank", "Assets:Checking");
    let out = sync(&mut p.b);
    assert!(out["connections"][0]["batch"].is_string(), "{out}");
}

#[test]
fn at5_challenge_questions() {
    let mut p = ofx_book(FakeOfx::standard());
    p.fake.with(|s| {
        s.cred1_label = None;
        s.challenge = vec![
            ("MFA13".into(), String::new()),
            ("123".into(), "Which branch?".into()),
        ];
        s.answers = vec![
            ("MFA13".into(), "6789".into()),
            ("123".into(), "Main".into()),
        ];
    });
    let first = add(&mut p, "103", json!({}));
    assert_eq!(first["challenge"].as_array().unwrap().len(), 2, "{first}");
    let answers = json!([{"id": "MFA13", "answer": "6789"}, {"id": "123", "answer": "Main"}]);
    let c = add(&mut p, "103", json!({"answers": answers}));
    assert_eq!(c["provider"], "ofx", "{c}");
    link(&mut p, &c, "bank", "Assets:Checking");
    // Sync now: the questions are fetched and shown.
    let out = sync(&mut p.b);
    let r = &out["connections"][0];
    let q = r["challenge"].as_array().unwrap();
    assert_eq!(q.len(), 2, "{r}");
    assert_eq!(
        q[0]["label"],
        "Last four digits of your social security number"
    );
    assert_eq!(q[1]["label"], "Which branch?");
    let att =
        p.b.run("connect.attention", json!({"today": "2026-10-04"}))
            .to_string();
    assert!(att.contains("Your bank asks extra questions"), "{att}");
    let out = p.b.run(
        "connect.answer_challenge",
        json!({"connection": c["id"], "answers": answers}),
    );
    let r = &out["connections"][0];
    assert!(r["batch"].is_string(), "{r}");
    assert!(
        p.fake
            .requests()
            .last()
            .unwrap()
            .contains("<MFAPHRASEA>6789")
    );
    // A background sync does not ask: it marks the connection.
    set_now(&mut p.b, NOW + 3 * 3600);
    let before = p.fake.requests().len();
    let out = p.b.run("connect.sync", json!({}));
    let r = &out["connections"][0];
    assert_eq!(r["status"], "needs_attention", "{r}");
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("open Sync to answer")
    );
    assert_eq!(
        p.fake.requests().len(),
        before + 1,
        "no challenge request in the background"
    );
}

#[test]
fn at6_rejected_signon_stops() {
    let mut p = ofx_book(FakeOfx::standard());
    let c = connected(&mut p);
    link(&mut p, &c, "bank", "Assets:Checking");
    link(&mut p, &c, "card", "Liabilities:Visa");
    p.fake.with(|s| {
        s.force_code = Some("15500".into());
        s.reject_multi = true;
    });
    let before = p.fake.requests().len();
    let out = sync(&mut p.b);
    let r = &out["connections"][0];
    assert_eq!(r["status"], "needs_attention");
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("User id or password not accepted")
    );
    assert_eq!(
        p.fake.requests().len(),
        before + 1,
        "no further request in that sync"
    );
    // A locked sign-in disables the connection.
    p.fake.with(|s| s.force_code = Some("15502".into()));
    sync(&mut p.b);
    let g = p.b.run("connect.get", json!({"connection": c["id"]}));
    assert_eq!(g["status"], "disabled", "{g}");
}

#[test]
fn at7_one_request_per_account_fallback() {
    let mut p = ofx_book(FakeOfx::standard());
    let c = connected(&mut p);
    link(&mut p, &c, "bank", "Assets:Checking");
    link(&mut p, &c, "card", "Liabilities:Visa");
    p.fake.with(|s| s.reject_multi = true);
    let before = p.fake.requests().len();
    let out = sync(&mut p.b);
    let r = &out["connections"][0];
    assert_eq!(r["fallback"], true, "{r}");
    let batch = p.b.run("import.get", json!({"batch": r["batch"]}));
    assert_eq!(batch["rows"].as_array().unwrap().len(), 5);
    assert_eq!(
        p.fake.requests().len(),
        before + 3,
        "one combined, then one per account"
    );
    let prof =
        p.b.run("connect.ofx_profile", json!({"connection": c["id"]}));
    assert_eq!(prof["single_requests"], true);
    set_now(&mut p.b, NOW + 3600);
    let before = p.fake.requests().len();
    sync(&mut p.b);
    let reqs = p.fake.requests();
    assert_eq!(reqs.len(), before + 2, "remembered: one per account");
    assert!(
        reqs.iter()
            .skip(before)
            .all(|r| r.matches("TRNRQ>").count() <= 4)
    );
}

fn def(version: &str, url: &str) -> String {
    format!(
        "moneykeep_definition = 1\nid = \"org.example.banks\"\nversion = \"{version}\"\nname = \"Banks\"\n\n[[ofx_institution]]\nkey = \"testbank\"\nname = \"Test Bank\"\nurl = \"{url}\"\norg = \"TESTBANK\"\nfid = \"1234\"\nofx_version = \"103\"\n"
    )
}

fn install(b: &mut Book, content: &str) -> Value {
    let i = b.run(
        "definition.inspect",
        json!({"content": content, "name": "banks.mkdef"}),
    );
    assert_eq!(i["valid"], true, "{i}");
    b.run(
        "definition.install",
        json!({"content": content, "sha256": i["sha256"], "name": "banks.mkdef"}),
    )
}

#[test]
fn at8_definition_profiles_and_host_guard() {
    let mut p = ofx_book(FakeOfx::standard());
    p.fake.with(|s| s.cred1_label = None);
    let v1 = def("1.0.0", "https://ofx.testbank.example/ofx");
    let i = p.b.run(
        "definition.inspect",
        json!({"content": v1, "name": "banks.mkdef"}),
    );
    assert_eq!(i["bank_hosts"][0]["host"], "ofx.testbank.example", "{i}");
    assert!(i["warnings"][0].as_str().unwrap().contains(
        "This file contains bank connection addresses. Only install it if you trust its author."
    ));
    install(&mut p.b, &v1);
    assert!(p.fake.requests().is_empty(), "installing connects nothing");
    let e = p.b.err(
        "connect.add",
        json!({"provider": "ofx", "institution_key": "Test Bank", "user_id": "user1", "password": "pass1"}),
    );
    assert_eq!(e.code(), "host_not_acknowledged");
    assert!(e.to_string().contains("ofx.testbank.example"), "{e}");
    assert!(p.fake.requests().is_empty());
    let c = p.b.run(
        "connect.add",
        json!({"provider": "ofx", "institution_key": "Test Bank", "user_id": "user1",
               "password": "pass1", "host_acknowledged": true}),
    );
    assert_eq!(c["host"], "ofx.testbank.example");
    // A later version points elsewhere: the connection stays, with a notice.
    install(
        &mut p.b,
        &def("1.1.0", "https://new-ofx.testbank.example/ofx"),
    );
    let g = p.b.run("connect.get", json!({"connection": c["id"]}));
    assert_eq!(g["host"], "ofx.testbank.example");
    assert_eq!(
        g["ofx"]["notice"], "The installed profile now points to new-ofx.testbank.example",
        "{g}"
    );
    // Switching needs the app (not MCP).
    let e =
        p.b.try_as(
            "mcp:test",
            "connect.set_ofx_profile",
            json!({"connection": c["id"], "accept_new_host": true}),
        )
        .unwrap_err();
    assert!(e.to_string().contains("only be changed in the app"), "{e}");
    let s = p.b.run(
        "connect.set_ofx_profile",
        json!({"connection": c["id"], "accept_new_host": true}),
    );
    assert_eq!(s["host"], "new-ofx.testbank.example");
}

#[test]
fn at9_password_kept_safe_and_ask_each_time() {
    let mut p = ofx_book(FakeOfx::standard());
    let c = connected(&mut p);
    let stored = p.secrets.all();
    assert!(
        stored.values().any(|v| v.contains("pass1")),
        "kept in the credential store"
    );
    let audit = serde_json::to_string(&p.b.s.audit_entries(100).unwrap()).unwrap();
    assert!(
        !audit.contains("pass1") && !audit.contains("user1\""),
        "{audit}"
    );
    let shown =
        p.b.run("connect.get", json!({"connection": c["id"]}))
            .to_string();
    assert!(!shown.contains("pass1"));
    // Ask for my password each time: nothing but the user id is kept.
    let mut q = ofx_book(FakeOfx::standard());
    q.fake.with(|s| s.cred1_label = None);
    let c = add(&mut q, "103", json!({"ask_password_each_time": true}));
    assert!(q.secrets.all().values().all(|v| !v.contains("pass1")));
    link(&mut q, &c, "bank", "Assets:Checking");
    let before = q.fake.requests().len();
    let out = q.b.run("connect.sync", json!({"scheduled": true}));
    assert_eq!(
        out["connections"][0]["skipped"], "Sync needs your password",
        "{out}"
    );
    assert_eq!(q.fake.requests().len(), before);
    let att =
        q.b.run("connect.attention", json!({"today": "2026-10-04"}))
            .to_string();
    assert!(att.contains("Sync needs your password"), "{att}");
    let out = q.b.run(
        "connect.answer_challenge",
        json!({"connection": c["id"], "password": "pass1"}),
    );
    assert!(out["connections"][0]["batch"].is_string(), "{out}");
    assert!(
        q.secrets.all().values().all(|v| !v.contains("pass1")),
        "still not stored"
    );
}

#[test]
fn at18_html_answer() {
    let mut p = ofx_book(FakeOfx::standard());
    let c = connected(&mut p);
    link(&mut p, &c, "bank", "Assets:Checking");
    p.fake.with(|s| s.html = true);
    let out = sync(&mut p.b);
    let r = &out["connections"][0];
    assert_eq!(r["status"], "needs_attention");
    assert_eq!(
        r["error"]["message"], "This address did not answer as an OFX server",
        "{r}"
    );
    assert!(!r.to_string().contains("We moved"), "the body is not shown");
}

/// A fetcher that refuses a redirect to another host, as the HTTPS fetcher does
/// (the transport itself is tested in mk-connect).
#[derive(Debug, Clone)]
struct Redirecting;

impl Fetcher for Redirecting {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, crate::connect::FetchError> {
        Err(crate::connect::FetchError::Refused(format!(
            "a request to elsewhere.example was refused: only {} is allowed",
            req.authority
        )))
    }
}

#[test]
fn at18_redirect_to_another_host_is_refused() {
    let mut p = ofx_book(FakeOfx::standard());
    let c = connected(&mut p);
    link(&mut p, &c, "bank", "Assets:Checking");
    let secrets = p.secrets.clone();
    p.b.r.set_connector(connector(
        std::sync::Arc::new(Redirecting),
        &secrets,
        NOW + 60,
    ));
    let out = sync(&mut p.b);
    let r = &out["connections"][0];
    assert_eq!(r["error"]["code"], "refused", "{r}");
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("only ofx.testbank.example is allowed")
    );
}

// ----- SnapTrade ------------------------------------------------------------------------

struct Snap {
    b: Book,
    fake: FakeSnaptrade,
}

fn snap_book() -> Snap {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "investments"}));
    b.account(
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    let fake = FakeSnaptrade::standard();
    let secrets = MemorySecrets::new(true);
    b.r.set_connector(connector(std::sync::Arc::new(fake.clone()), &secrets, NOW));
    Snap { b, fake }
}

fn snap_connect(p: &mut Snap) -> Value {
    let c = p.b.run(
        "connect.add",
        json!({"provider": "snaptrade", "client_id": "TESTCLIENT", "consumer_key": "test-consumer-key", "label": "Brokerages"}),
    );
    let id = c["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["remote_account_id"] == "acct-1")
        .unwrap()["remote_account_id"]
        .clone();
    p.b.run(
        "connect.link_account",
        json!({"connection": c["id"], "remote_account": id, "account": "Assets:Brokerage"}),
    );
    c
}

fn sym(ticker: &str) -> Value {
    json!({"id": "s-1", "symbol": ticker, "raw_symbol": ticker, "description": "Vanguard Total Stock Market ETF",
           "currency": {"id": "c-usd", "code": "USD", "name": "US Dollar"}, "figi_code": "BBG000HXBZB2",
           "type": {"code": "et", "description": "ETF"}})
}

fn act(
    id: &str,
    kind: &str,
    date: &str,
    units: &str,
    price: &str,
    amount: &str,
    fee: &str,
) -> Value {
    // Numbers are written as JSON number text, as the API sends them.
    let text = format!(
        r#"{{"id": "{id}", "type": "{kind}", "trade_date": "{date}T00:00:00.000Z", "settlement_date": "{date}T00:00:00.000Z",
            "units": {units}, "price": {price}, "amount": {amount}, "fee": {fee}, "fx_rate": null,
            "currency": {{"id": "c-usd", "code": "USD", "name": "US Dollar"}}, "description": "{kind} VTI",
            "institution": "Schwab", "option_symbol": null}}"#
    );
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["symbol"] = sym("VTI");
    v
}

fn snap_sync(p: &mut Snap) -> Value {
    p.b.run("connect.sync", json!({"now": true}))["connections"][0].clone()
}

fn snap_batch(p: &mut Snap, r: &Value) -> Value {
    p.b.run("import.get", json!({"batch": r["batch"]}))
}

#[test]
fn at10_requests_are_signed() {
    let mut p = snap_book();
    snap_connect(&mut p);
    let reqs = p.fake.requests();
    assert!(!reqs.is_empty());
    for (url, ok) in &reqs {
        assert!(*ok, "the fake server checked the Signature of {url}");
        // Revision 2: paths from the host root, without the legacy `/api/v1`.
        assert!(
            url.starts_with("https://api.snaptrade.com/accounts/"),
            "{url}"
        );
        assert!(
            url.contains("clientId=TESTCLIENT&timestamp=1791072000"),
            "{url}"
        );
        assert!(!url.contains("userSecret") && !url.contains("userId"));
    }
    // A wrong consumer key is refused by the server (401).
    let mut q = snap_book();
    let e = q.b.err(
        "connect.add",
        json!({"provider": "snaptrade", "client_id": "TESTCLIENT", "consumer_key": "wrong"}),
    );
    assert_eq!(e.code(), "credential_rejected", "{e}");
}

#[test]
fn at11_accounts_and_activity_pages() {
    let mut p = snap_book();
    let c = p.b.run(
        "connect.add",
        json!({"provider": "snaptrade", "client_id": "TESTCLIENT", "consumer_key": "test-consumer-key"}),
    );
    let accts = c["accounts"].as_array().unwrap();
    assert_eq!(accts.len(), 2);
    let inv = accts
        .iter()
        .find(|a| a["remote"]["kind"] == "investment")
        .unwrap();
    assert_eq!(inv["remote"]["masked"], "****8443");
    assert!(inv["name"].as_str().unwrap().ends_with("****8443"));
    let dep = accts
        .iter()
        .find(|a| a["remote"]["kind"] == "deposit")
        .unwrap();
    assert_eq!(dep["remote"]["masked"], "****1290");
    assert_eq!(dep["remote"]["status"], "initial sync not completed");
    assert_eq!(dep["remote"]["linkable"], false);
    p.fake.with(|s| {
        let list: Vec<Value> = (0..1200)
            .map(|i| {
                let mut a = act(
                    &format!("int-{i}"),
                    "INTEREST",
                    "2026-09-01",
                    "0",
                    "0",
                    "0.01",
                    "0",
                );
                a["symbol"] = Value::Null;
                a
            })
            .collect();
        s.activities.insert("acct-1".into(), list);
    });
    p.b.run(
        "connect.link_account",
        json!({"connection": c["id"], "remote_account": "acct-1", "account": "Assets:Brokerage"}),
    );
    let r = snap_sync(&mut p);
    let offsets: Vec<String> = p
        .fake
        .requests()
        .iter()
        .filter(|(u, _)| u.contains("/activities"))
        .map(|(u, _)| u.clone())
        .collect();
    assert_eq!(offsets.len(), 2, "{offsets:?}");
    assert!(offsets[0].contains("offset=0&limit=1000"));
    assert!(offsets[1].contains("offset=1000&limit=1000"));
    assert_eq!(r["pages"][0]["offsets"], json!([0, 1000]), "{r}");
    let batch = snap_batch(&mut p, &r);
    assert_eq!(batch["inv_rows"].as_array().unwrap().len(), 1200);
}

fn inv_rows(batch: &Value) -> Vec<Value> {
    batch["inv_rows"].as_array().unwrap().clone()
}

#[test]
fn at12_activity_mapping() {
    let mut p = snap_book();
    p.fake.with(|s| {
        let mut opt = act("o1", "OPTIONEXPIRATION", "2026-09-04", "1", "0", "0", "0");
        opt["option_symbol"] =
            json!({"id": "x", "ticker": "VTI  261016C00250000", "option_type": "CALL"});
        s.activities.insert(
            "acct-1".into(),
            vec![
                act("b1", "BUY", "2026-09-02", "10", "220.00", "-2200.00", "0"),
                act("d1", "DIVIDEND", "2026-09-03", "0", "0", "12.34", "0"),
                act("r1", "REI", "2026-09-03", "0.05", "246.80", "-12.34", "0"),
                opt,
                act("x1", "ADJUSTMENT", "2026-09-05", "0", "0", "1.00", "0"),
            ],
        );
    });
    snap_connect(&mut p);
    let r = snap_sync(&mut p);
    let rows = inv_rows(&snap_batch(&mut p, &r));
    assert_eq!(rows.len(), 4, "{rows:?}");
    let buy = rows.iter().find(|r| r["inv_action"] == "buy").unwrap();
    assert_eq!(buy["quantity"], "10");
    assert_eq!(buy["price"], "220");
    assert_eq!(buy["amount"], "2200.00");
    assert_eq!(buy["symbol"], "VTI");
    let rei = rows
        .iter()
        .filter(|r| r["inv_action"] == "reinvest")
        .count();
    assert_eq!(rei, 1, "dividend and REI are one reinvest");
    assert!(rows.iter().all(|r| r["inv_action"] != "dividend"));
    let unsupported: Vec<String> = rows
        .iter()
        .filter(|r| r["inv_action"] == "unsupported")
        .map(|r| r["row"]["unsupported"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        unsupported.contains(&"Options are not supported yet".to_owned()),
        "{unsupported:?}"
    );
    assert!(unsupported.contains(&"Unknown activity: ADJUSTMENT".to_owned()));
}

#[test]
fn at13_resent_activity_with_a_new_id() {
    let mut p = snap_book();
    p.fake.with(|s| {
        s.activities.insert(
            "acct-1".into(),
            vec![act(
                "b1",
                "BUY",
                "2026-09-02",
                "10",
                "220.00",
                "-2200.00",
                "0",
            )],
        );
    });
    snap_connect(&mut p);
    let r = snap_sync(&mut p);
    let batch = snap_batch(&mut p, &r);
    p.b.commit(&batch);
    p.fake.with(|s| {
        s.activities.insert(
            "acct-1".into(),
            vec![act(
                "b1-new",
                "BUY",
                "2026-09-02",
                "10",
                "220.00",
                "-2200.00",
                "0",
            )],
        );
    });
    set_now(&mut p.b, NOW + 86_400);
    let r = snap_sync(&mut p);
    let rows = inv_rows(&snap_batch(&mut p, &r));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["class"], "duplicate", "{:?}", rows[0]);
    assert!(
        rows[0]["notes"]
            .to_string()
            .contains("Same as an earlier transaction (the provider changed its id)"),
        "{:?}",
        rows[0]
    );
}

#[test]
fn at14_numbers_from_their_text() {
    let mut p = snap_book();
    p.fake.with(|s| {
        let mut b = act("b1", "BUY", "2026-09-02", "3", "0.1", "null", "0");
        b["amount"] = Value::Null;
        s.activities.insert("acct-1".into(), vec![b]);
        s.positions.insert(
            "acct-1".into(),
            vec![serde_json::from_str(r#"{"symbol": {"symbol": {"symbol": "VTI", "raw_symbol": "VTI", "description": "Vanguard Total Stock Market ETF"}}, "units": 1e-7, "price": 231.17}"#).unwrap()],
        );
    });
    snap_connect(&mut p);
    let r = snap_sync(&mut p);
    let batch = snap_batch(&mut p, &r);
    let rows = inv_rows(&batch);
    assert_eq!(rows[0]["amount"], "0.30", "{:?}", rows[0]);
    let sec = batch["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "investment")
        .unwrap()
        .clone();
    assert_eq!(sec["positions"][0]["quantity"], "0.0000001", "{sec}");
}

#[test]
fn at15_holdings_check_and_price() {
    let mut p = snap_book();
    p.fake.with(|s| {
        s.activities.insert(
            "acct-1".into(),
            vec![act(
                "b1",
                "BUY",
                "2026-09-02",
                "10",
                "220.00",
                "-2200.00",
                "0",
            )],
        );
    });
    let c = snap_connect(&mut p);
    let r = snap_sync(&mut p);
    let batch = snap_batch(&mut p, &r);
    p.b.commit(&batch);
    p.fake.with(|s| {
        s.positions.insert(
            "acct-1".into(),
            // The positions/all shape (connectivity-brokerage.md revision 2).
            vec![serde_json::from_str(r#"{"instrument": {"kind": "etf", "symbol": "VTI", "raw_symbol": "VTI", "description": "Vanguard Total Stock Market ETF", "currency": "USD", "figi_instrument": {"figi_code": "BBG000HXBZB2"}}, "units": 12.5, "price": 231.17, "cost_basis": 210.0, "currency": "USD", "cash_equivalent": false}"#).unwrap()],
        );
        s.balances.insert(
            "acct-1".into(),
            vec![serde_json::from_str(r#"{"currency": {"code": "USD"}, "cash": 0}"#).unwrap()],
        );
    });
    set_now(&mut p.b, NOW + 86_400);
    let r = snap_sync(&mut p);
    let h = &r["holdings"][0];
    assert_eq!(h["differences"].as_array().unwrap().len(), 1, "{r}");
    let d = &h["differences"][0];
    assert_eq!(d["provider"], "12.5");
    assert_eq!(d["book_after"], "10");
    assert_eq!(
        d["message"],
        "SnapTrade holds 12.5 VTI; MoneyKeep will hold 10 after this import"
    );
    let check = p.b.run(
        "connect.holdings_check",
        json!({"account": "Assets:Brokerage"}),
    );
    assert_eq!(check["differences"][0]["security"], "VTI", "{check}");
    let att =
        p.b.run("connect.attention", json!({"today": "2026-10-05"}))
            .to_string();
    assert!(
        att.contains("Assets:Brokerage: 1 holding differs from SnapTrade"),
        "{att}"
    );
    let prices = p.b.run("price.list", json!({"security": "VTI"}));
    let last = prices.as_array().unwrap().last().unwrap();
    assert_eq!(last["price"], "231.17", "{prices}");
    assert_eq!(last["source"], "SnapTrade");
    // The review offers a row for the difference; nothing is adjusted by itself.
    let b = p.b.run(
        "import.add_holding_row",
        json!({"batch": r["batch"], "security": "VTI"}),
    );
    let added = b["inv_rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["inv_action"] == "shares_in")
        .unwrap()
        .clone();
    assert_eq!(added["quantity"], "2.5", "{added}");
    assert_eq!(added["needs_review"], true);
    assert!(added["notes"].to_string().contains("enter the basis"));
    let _ = c;
}

#[test]
fn at16_rate_limit_waits_and_401_needs_attention() {
    let mut p = snap_book();
    snap_connect(&mut p);
    p.fake.with(|s| s.throttle = 1);
    let r = snap_sync(&mut p);
    assert!(
        p.fake.with(|s| s.waits.contains(&2000)).unwrap(),
        "waited Retry-After (fake clock)"
    );
    assert_eq!(r["status"], "active", "{r}");
    assert!(r.get("error").is_none_or(Value::is_null), "{r}");
    p.fake
        .with(|s| s.status = Some((401, r#"{"detail": "Invalid signature"}"#.into())));
    set_now(&mut p.b, NOW + 86_400);
    let r = snap_sync(&mut p);
    assert_eq!(r["status"], "needs_attention", "{r}");
    p.fake
        .with(|s| s.status = Some((401, r#"{"detail": "timestamp is too old"}"#.into())));
    set_now(&mut p.b, NOW + 2 * 86_400);
    let r = snap_sync(&mut p);
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Check your computer's clock"),
        "{r}"
    );
}

// ----- connectivity-brokerage.md revision 2 -------------------------------------------------

fn snap_with_trade() -> Snap {
    let p = snap_book();
    p.fake.with(|s| {
        s.activities.insert(
            "acct-1".into(),
            vec![act("b1", "BUY", "2026-09-02", "10", "220.00", "-2200.00", "0")],
        );
        s.balances.insert(
            "acct-1".into(),
            vec![serde_json::from_str(r#"{"currency": {"id": "c-usd", "code": "USD", "name": "US Dollar"}, "cash": 300.71, "buying_power": 410.71}"#).unwrap()],
        );
    });
    p
}

fn conn_of(p: &mut Snap) -> Value {
    p.b.run("connect.list", json!({}))[0].clone()
}

#[test]
fn rev2_test19_positions_from_positions_all() {
    let mut p = snap_with_trade();
    p.fake.with(|s| {
        s.balances.insert("acct-1".into(), Vec::new());
        s.positions.insert(
            "acct-1".into(),
            vec![
                serde_json::from_str(r#"{"instrument": {"kind": "etf", "id": "i-1", "symbol": "VTI", "raw_symbol": "VTI", "description": "Vanguard Total Stock Market ETF", "currency": "USD", "exchange": {}, "figi_instrument": {"figi_code": "BBG000HXBZB2", "figi_share_class": "BBG001"}}, "units": "10", "price": "231.17", "cost_basis": "220.00", "currency": "USD", "cash_equivalent": false, "tax_lots": []}"#).unwrap(),
                serde_json::from_str(r#"{"instrument": {"kind": "mutualfund", "symbol": "SWVXX", "description": "Schwab Prime Advantage Money Fund", "currency": "USD"}, "units": "125.25", "price": "1", "cost_basis": "1", "currency": "USD", "cash_equivalent": true}"#).unwrap(),
                serde_json::from_str(r#"{"instrument": {"kind": "option", "symbol": "VTI  261218C00300000", "currency": "USD"}, "units": "1", "price": "2.50", "currency": "USD", "cash_equivalent": false}"#).unwrap(),
                serde_json::from_str(r#"{"instrument": {"kind": "option", "symbol": "VTI  261218P00200000", "currency": "USD"}, "units": "2", "price": "1.10", "currency": "USD", "cash_equivalent": false}"#).unwrap(),
            ],
        );
    });
    snap_connect(&mut p);
    let r = snap_sync(&mut p);
    let urls: Vec<String> = p.fake.requests().into_iter().map(|(u, _)| u).collect();
    assert!(
        urls.iter()
            .any(|u| u.starts_with("https://api.snaptrade.com/accounts/acct-1/positions/all?")),
        "{urls:?}"
    );
    assert!(
        !urls.iter().any(|u| u.contains("/positions?")),
        "the deprecated per-account positions are not asked for"
    );
    let batch = snap_batch(&mut p, &r);
    let sec = batch["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "investment")
        .unwrap()
        .clone();
    let positions = sec["positions"].as_array().unwrap();
    assert_eq!(positions.len(), 1, "{sec}");
    assert_eq!(positions[0]["security"], "TICKER:VTI");
    assert_eq!(positions[0]["quantity"], "10");
    // The money market fund counts toward cash (no cash in the balances here).
    assert_eq!(sec["cash_balance"]["amount"], "125.25", "{sec}");
    assert!(
        batch["warnings"]
            .to_string()
            .contains("2 option positions not imported"),
        "{}",
        batch["warnings"]
    );
    assert!(r["partial"].is_null(), "{r}");
    // With cash in the balances, the fund is not counted twice.
    p.fake.with(|s| {
        s.balances.insert(
            "acct-1".into(),
            vec![serde_json::from_str(r#"{"currency": {"code": "USD"}, "cash": 425.96}"#).unwrap()],
        );
    });
    set_now(&mut p.b, NOW + 86_400);
    let r = snap_sync(&mut p);
    let batch = snap_batch(&mut p, &r);
    let sec = batch["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "investment")
        .unwrap()
        .clone();
    assert_eq!(sec["cash_balance"]["amount"], "425.96", "{sec}");
}

#[test]
fn rev2_test20_failing_positions_keep_the_transactions_and_cash() {
    let mut p = snap_with_trade();
    p.fake.with(|s| {
        s.fail.insert(
            "positions".into(),
            (410, r#"{"detail": "Gone", "status_code": 410}"#.into()),
        );
    });
    snap_connect(&mut p);
    let r = snap_sync(&mut p);
    assert_eq!(r["status"], "active", "{r}");
    assert!(
        r.get("error").is_none_or(Value::is_null),
        "no red line: {r}"
    );
    let batch = snap_batch(&mut p, &r);
    let rows = inv_rows(&batch);
    assert_eq!(rows.len(), 1, "the activities are imported");
    assert_eq!(rows[0]["inv_action"], "buy");
    let sec = batch["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "investment")
        .unwrap()
        .clone();
    assert_eq!(
        sec["cash_balance"]["amount"], "300.71",
        "the cash is imported"
    );
    let c = conn_of(&mut p);
    assert!(c["last_error"].is_null(), "{c}");
    let line = c["partial"].as_str().unwrap();
    assert_eq!(
        line,
        "Holdings couldn't be read (SnapTrade answered 410 for positions). SnapTrade no longer offers this request (positions). MoneyKeep needs an update. Transactions and the cash balance were imported."
    );
    // The holdings check does not claim the shares are gone.
    assert_eq!(
        r["holdings"][0]["differences"],
        json!([]),
        "{}",
        r["holdings"]
    );
    assert_eq!(
        r["holdings"][0]["positions"],
        json!([]),
        "{}",
        r["holdings"]
    );
    // A later complete sync clears the yellow line.
    p.fake.with(|s| s.fail.clear());
    set_now(&mut p.b, NOW + 86_400);
    snap_sync(&mut p);
    assert!(conn_of(&mut p)["partial"].is_null());
}

#[test]
fn rev2_test21_every_call_gone_is_the_red_line() {
    let mut p = snap_with_trade();
    snap_connect(&mut p);
    // Every per-account call answers 410 …
    p.fake.with(|s| {
        for k in ["activities", "positions", "balances"] {
            s.fail.insert(k.into(), (410, "{}".into()));
        }
    });
    let r = snap_sync(&mut p);
    let m = r["error"]["message"].as_str().unwrap();
    assert_eq!(
        m,
        "SnapTrade no longer offers this request (activities, positions and balances). MoneyKeep needs an update.",
        "{r}"
    );
    assert!(!m.contains("try again later"));
    assert!(conn_of(&mut p)["partial"].is_null(), "red, not yellow");
    // … and the accounts call too.
    p.fake.with(|s| s.status = Some((410, "{}".into())));
    set_now(&mut p.b, NOW + 86_400);
    let r = snap_sync(&mut p);
    assert_eq!(
        r["error"]["message"],
        "SnapTrade no longer offers this request (accounts). MoneyKeep needs an update.",
        "{r}"
    );
}

#[test]
fn rev2_test22_retry_after_and_spacing_per_account() {
    let mut p = snap_with_trade();
    snap_connect(&mut p);
    let before = p.fake.requests().len();
    p.fake.with(|s| {
        s.throttle = 1;
        s.retry_after = 2;
    });
    let r = snap_sync(&mut p);
    assert_eq!(r["status"], "active", "{r}");
    let (urls, times) = p
        .fake
        .with(|s| {
            (
                s.requests[before..]
                    .iter()
                    .map(|(u, _)| u.clone())
                    .collect::<Vec<_>>(),
                s.times[before..].to_vec(),
            )
        })
        .unwrap();
    // The 429 (first request) was retried no sooner than Retry-After.
    // The same request again, signed fresh (security-review-1 B3).
    let path = |u: &str| u.split('?').next().unwrap().to_owned();
    assert_eq!(path(&urls[0]), path(&urls[1]), "the same request again");
    assert_eq!(timestamp_of(&urls[1]), timestamp_of(&urls[0]) + 2);
    assert!(times[1] - times[0] >= 2000, "{times:?}");
    // Calls for the account are at least 6 seconds apart (under 10 a minute).
    let mine: Vec<u64> = urls
        .iter()
        .zip(&times)
        .filter(|(u, _)| u.contains("/accounts/acct-1/"))
        .map(|(_, t)| *t)
        .collect();
    assert_eq!(mine.len(), 3, "{urls:?}");
    for w in mine.windows(2) {
        assert!(w[1] - w[0] >= 6000, "{mine:?}");
    }
}

#[test]
fn rev2_deprecation_headers_are_logged_with_the_path() {
    let mut p = snap_with_trade();
    snap_connect(&mut p);
    snap_sync(&mut p);
    assert!(
        conn_of(&mut p)["deprecations"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "the current paths carry no Deprecation header"
    );
    p.fake.with(|s| s.deprecation = Some("@1790000000".into()));
    set_now(&mut p.b, NOW + 86_400);
    snap_sync(&mut p);
    let log = conn_of(&mut p)["deprecations"].to_string();
    assert!(log.contains("/accounts/all: @1790000000"), "{log}");
    assert!(
        log.contains("/accounts/acct-1/positions/all: @1790000000"),
        "{log}"
    );
    // The legacy prefix is answered with one, as SnapTrade does.
    let key = crate::snap_conn::key_from_input("TESTCLIENT", "test-consumer-key").unwrap();
    let mut req = crate::snap_conn::accounts_request(&key, NOW);
    req.url = req.url.replace("/accounts/all", "/api/v1/accounts/all");
    p.fake.with(|s| s.deprecation = None);
    let resp = crate::connect::Fetcher::send(&p.fake, &req).unwrap();
    assert_eq!(resp.deprecation.as_deref(), Some("@1781222400"));
}

// ----- security-review-1 B3: each request signed when sent ------------------------------

/// The `timestamp` query parameter of a request URL.
fn timestamp_of(url: &str) -> i64 {
    url.split(['?', '&'])
        .find_map(|kv| kv.strip_prefix("timestamp="))
        .and_then(|v| v.parse().ok())
        .unwrap()
}

#[test]
fn sr_b3_requests_are_signed_when_sent() {
    let mut p = snap_book();
    snap_connect(&mut p);
    let before = p.fake.requests().len();
    // Two 429s with Retry-After 30: the third try of the accounts request goes out
    // a fake minute after the first.
    p.fake.with(|s| {
        s.throttle = 2;
        s.retry_after = 30;
    });
    set_now(&mut p.b, NOW + 86_400);
    let r = snap_sync(&mut p);
    assert_eq!(r["status"], "active", "{r}");
    let (reqs, times) = p
        .fake
        .with(|s| (s.requests.clone(), s.times.clone()))
        .unwrap();
    let reqs = &reqs[before..];
    let times = &times[before..];
    assert!(reqs.iter().all(|(_, ok)| *ok), "every signature checked");
    let (first, third) = (&reqs[0].0, &reqs[2].0);
    assert!(first.contains("/accounts/all") && third.contains("/accounts/all"));
    assert_eq!(times[2] - times[0], 60_000, "a fake minute apart");
    assert_eq!(timestamp_of(first), NOW + 86_400);
    assert_eq!(timestamp_of(third), NOW + 86_400 + 60);
    // Each retry was re-signed: the timestamps differ, so do the URLs.
    assert_ne!(reqs[0].0, reqs[1].0);
    assert_ne!(reqs[1].0, reqs[2].0);
    // Later pages and parts carry the time they were sent (spacing advances the
    // clock between an account's calls).
    for ((url, _), at) in reqs.iter().zip(times) {
        let passed = i64::try_from((*at - times[0]) / 1000).unwrap();
        assert_eq!(timestamp_of(url), NOW + 86_400 + passed, "{url}");
    }
}

#[test]
fn sr_b3_a_401_is_a_rejected_key_only_when_refused_twice() {
    // Refused once: the same request signed fresh gets through.
    let mut p = snap_book();
    p.fake.with(|s| s.refuse = 1);
    let c = snap_connect(&mut p);
    assert!(c["id"].is_string(), "{c}");
    let reqs = p.fake.requests();
    assert!(reqs[0].0.contains("/accounts/all") && reqs[1].0.contains("/accounts/all"));
    p.fake.with(|s| s.refuse = 1);
    set_now(&mut p.b, NOW + 86_400);
    let r = snap_sync(&mut p);
    assert_eq!(r["status"], "active", "{r}");
    // Refused twice: the personal key was rejected.
    p.fake.with(|s| s.refuse = 2);
    set_now(&mut p.b, NOW + 2 * 86_400);
    let before = p.fake.requests().len();
    let r = snap_sync(&mut p);
    assert_eq!(r["status"], "needs_attention", "{r}");
    assert_eq!(
        p.fake.requests().len() - before,
        2,
        "tried twice, then stopped"
    );
    // On connecting too.
    let mut q = snap_book();
    q.fake.with(|s| s.refuse = 2);
    let e = q.b.err(
        "connect.add",
        json!({"provider": "snaptrade", "client_id": "TESTCLIENT", "consumer_key": "test-consumer-key"}),
    );
    assert_eq!(e.code(), "credential_rejected", "{e}");
    assert_eq!(q.fake.requests().len(), 2);
}
