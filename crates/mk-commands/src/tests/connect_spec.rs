//! connectivity.md acceptance tests 1–8, 10 and 13 (11 is in mk-mcp, 9 and the
//! transport rules in mk-connect, 12 in xtask). Recorded answers through the injected
//! fetcher; nothing touches the network.

use super::*;
use crate::connect::{Connector, FakeSimplefin, HttpMethod, MemorySecrets};

/// 2026-10-01 00:00 UTC.
const OCT1: i64 = 1_790_812_800;

const V2: &str = r#"{
  "errlist": [],
  "connections": [
    {"conn_id": "c1", "name": "First Bank", "org_id": "o1", "org_url": "https://first.test"},
    {"conn_id": "c2", "name": "Euro Bank", "org_id": "o2"}
  ],
  "accounts": [
    {"id": "a1", "name": "Everyday", "conn_id": "c1", "currency": "USD",
     "balance": "1200.50", "available-balance": "1100.00", "balance-date": 1790812800,
     "transactions": [
       {"id": "t1", "posted": 1790553600, "amount": "-45.10", "description": "Corner Grocery"},
       {"id": "t2", "posted": 1790640000, "amount": "2000.00", "description": "Payroll"}
     ]},
    {"id": "a2", "name": "Card", "conn_id": "c1", "currency": "USD",
     "balance": "-300.00", "balance-date": 1790812800,
     "transactions": [
       {"id": "t3", "posted": 1790726400, "amount": "-19.99", "description": "Stream Co"},
       {"id": "t4", "posted": 0, "amount": "-5.00", "description": "Coffee", "pending": true}
     ]},
    {"id": "a3", "name": "Euro", "conn_id": "c2", "currency": "EUR",
     "balance": "80.00", "balance-date": 1790812800,
     "transactions": [
       {"id": "t5", "posted": 1790726400, "amount": "-12.00", "description": "Bakery"}
     ]}
  ]
}"#;

struct Feed {
    b: Book,
    fake: FakeSimplefin,
    secrets: MemorySecrets,
}

fn connector(fake: &FakeSimplefin, secrets: &MemorySecrets, now: i64) -> Connector {
    Connector {
        fetcher: std::sync::Arc::new(fake.clone()),
        secrets: Box::new(secrets.clone()),
        utc_offset_secs: 0,
        now: Some(now),
        host: None,
    }
}

fn feed_book(body: &str) -> Feed {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Card", "type": "liability", "subtype": "credit-card", "parent": "Liabilities"}));
    b.run("module.enable", json!({"module": "bank-feeds"}));
    let fake = FakeSimplefin::new(body);
    let secrets = MemorySecrets::new(true);
    b.r.set_connector(connector(&fake, &secrets, OCT1));
    Feed { b, fake, secrets }
}

fn set_now(f: &mut Feed, now: i64) {
    if let Some(c) = f.b.r.connector_mut() {
        c.now = Some(now);
    }
}

fn add(f: &mut Feed, token: &str) -> Value {
    f.b.run(
        "connect.add",
        json!({"setup_input": token, "label": "My bank", "confirm": true}),
    )
}

fn connected(body: &str) -> (Feed, String) {
    let mut f = feed_book(body);
    let c = add(&mut f, &FakeSimplefin::token("abc"));
    let id = c["id"].as_str().unwrap().to_owned();
    (f, id)
}

fn link(f: &mut Feed, conn: &str, remote: &str, account: &str, extra: Value) -> Value {
    let mut a = json!({"connection": conn, "remote_account": remote, "account": account,
                       "sync_from": "2026-09-01"});
    for (k, v) in extra.as_object().unwrap() {
        a[k] = v.clone();
    }
    f.b.run("connect.link_account", a)
}

fn sync(f: &mut Feed, now: bool) -> Value {
    f.b.run("connect.sync", json!({"now": now}))
}

#[test]
fn at1_claim_stores_the_credential_outside_the_book() {
    let mut f = feed_book(V2);
    // The first step only shows the host.
    let ask = f.b.run(
        "connect.add",
        json!({"setup_input": FakeSimplefin::token("abc"), "label": "My bank"}),
    );
    assert_eq!(ask["confirm"]["host"], "bridge.test");
    assert!(
        f.fake.requests().is_empty(),
        "nothing sent before confirming"
    );
    let c = add(&mut f, &FakeSimplefin::token("abc"));
    let reqs = f.fake.requests();
    assert_eq!(reqs[0].0, HttpMethod::Post);
    assert_eq!(reqs[0].1, "https://bridge.test/simplefin/claim/abc");
    assert!(!reqs[0].2, "the claim carries no credentials");
    assert_eq!(c["host"], "bridge.test");
    assert_eq!(c["credential_store"], "os");
    let stored = f.secrets.all();
    assert_eq!(stored.len(), 1);
    assert!(stored.values().next().unwrap().contains("s3cret-pass"));
    let shown = f.b.run("connect.get", json!({"connection": c["id"]}));
    for text in [
        shown.to_string(),
        f.b.run("connect.list", json!({})).to_string(),
    ] {
        assert!(
            !text.contains("s3cret") && !text.contains("demo:"),
            "{text}"
        );
    }
    let audit = serde_json::to_string(&f.b.s.audit_entries(50).unwrap()).unwrap();
    assert!(!audit.contains("s3cret") && !audit.contains("demo:"));
    assert!(
        !audit.contains(&FakeSimplefin::token("abc")),
        "no token in the audit log"
    );
    // Two remote accounts of the first bank and one of the second were listed.
    assert_eq!(shown["accounts"].as_array().unwrap().len(), 3);
    assert_eq!(shown["version"], 2);
}

#[test]
fn at1_encrypted_book_keeps_the_credential_inside() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("enc.moneykeep");
    let (s, _) = Store::create_encrypted(
        &path,
        "Enc",
        &CommodityCode::new("USD").unwrap(),
        "a long password here",
        false,
        mk_store::KdfParams::TEST,
    )
    .unwrap();
    let mut b = Book {
        r: Registry::with_builtins(),
        s,
        a: Actor("test".into()),
    };
    b.run("module.enable", json!({"module": "bank-feeds"}));
    let fake = FakeSimplefin::new(V2);
    let secrets = MemorySecrets::new(true);
    b.r.set_connector(connector(&fake, &secrets, OCT1));
    let c = b.run(
        "connect.add",
        json!({"setup_input": FakeSimplefin::token("enc"), "confirm": true}),
    );
    assert_eq!(c["credential_store"], "book");
    assert!(secrets.all().is_empty(), "nothing in the OS store");
    // Removing the password removes the credential with it.
    let copy = dir.path().join("copy.moneykeep");
    b.s.save_unencrypted_copy(&copy).unwrap();
    let plain = Store::open(&copy).unwrap();
    drop(plain);
    let bytes = std::fs::read(&copy).unwrap();
    assert!(
        !bytes.windows(11).any(|w| w == b"s3cret-pass"),
        "the unprotected copy has no credential"
    );
}

#[test]
fn at1_refused_without_secure_storage() {
    let mut b = Book::new();
    b.run("module.enable", json!({"module": "bank-feeds"}));
    let fake = FakeSimplefin::new(V2);
    b.r.set_connector(connector(&fake, &MemorySecrets::new(false), OCT1));
    let p = b.run("connect.providers", json!({}));
    assert_eq!(p[0]["secure_storage"], false);
    let e = b.err(
        "connect.add",
        json!({"setup_input": FakeSimplefin::token("x"), "confirm": true}),
    );
    assert_eq!(e.code(), "no_secure_storage");
    assert!(e.to_string().contains("password"));
    assert!(fake.requests().is_empty(), "the token is not used up");
}

#[test]
fn at2_bad_tokens_and_used_tokens() {
    let mut f = feed_book(V2);
    use base64::Engine;
    let enc = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    for bad in [
        enc("http://bridge.test/simplefin/claim/abc"),
        enc("https://bridge.test/simplefin/other/abc"),
        enc("just some words"),
    ] {
        let e =
            f.b.err("connect.add", json!({"setup_input": bad, "confirm": true}));
        assert_eq!(e.code(), "invalid_args", "{e}");
    }
    assert!(f.fake.requests().is_empty());
    add(&mut f, &FakeSimplefin::token("once"));
    let e = f.b.err(
        "connect.add",
        json!({"setup_input": FakeSimplefin::token("once"), "confirm": true}),
    );
    assert!(e.to_string().contains("already used"), "{e}");
}

#[test]
fn at3_version_two_chosen() {
    let (mut f, id) = connected(V2);
    let c = f.b.run("connect.get", json!({"connection": id}));
    assert_eq!(c["version"], 2);
    if let Ok(mut s) = f.fake.state.lock() {
        s.versions = vec!["1".into()];
    }
    let c = add(&mut f, &FakeSimplefin::token("v1"));
    assert_eq!(c["version"], 1);
}

fn link_all(f: &mut Feed, id: &str) {
    link(f, id, "a1", "Assets:Checking", json!({}));
    link(f, id, "a2", "Liabilities:Card", json!({}));
}

#[test]
fn at4_account_set_stages_sections_and_rows() {
    let (mut f, id) = connected(V2);
    // A EUR remote account cannot be linked to a USD account.
    let e = f.b.err(
        "connect.link_account",
        json!({"connection": id, "remote_account": "a3", "account": "Assets:Checking"}),
    );
    assert!(e.to_string().contains("EUR"), "{e}");
    f.b.run("module.enable", json!({"module": "multi-currency"}));
    f.b.account(json!({"name": "Euro Cash", "type": "asset", "subtype": "bank", "parent": "Assets", "commodity": "EUR"}));
    link_all(&mut f, &id);
    link(&mut f, &id, "a3", "Assets:Euro Cash", json!({}));
    let out = sync(&mut f, true);
    let r = &out["connections"][0];
    let batch = f.b.run("import.get", json!({"batch": r["batch"]}));
    assert_eq!(batch["source"], "SimpleFIN — My bank");
    let sections = batch["sections"].as_array().unwrap();
    assert_eq!(sections.len(), 3);
    assert_eq!(
        sections.iter().filter(|s| s["currency"] == "USD").count(),
        2
    );
    assert_eq!(sections[0]["account_name"], "First Bank — Everyday");
    assert_eq!(sections[0]["closing"]["amount"], "1200.50");
    assert_eq!(sections[0]["closing"]["date"], "2026-10-01");
    let rows = batch["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 4, "the pending row is left out");
    assert!(
        rows.iter()
            .any(|r| r["payee"] == "Corner Grocery" && r["amount"] == "-45.10")
    );
    assert!(rows.iter().all(|r| r["payee"] != "Coffee"));
    // The request asked from the link's start date, with Basic credentials.
    let reqs = f.fake.requests();
    let (_, url, basic) = reqs.last().unwrap().clone();
    assert!(basic);
    assert!(
        url.starts_with("https://bridge.test/simplefin/accounts?version=2&start-date=1788220800"),
        "{url}"
    );
    assert!(!url.contains("s3cret"));
    assert!(url.contains("account=a1") && url.contains("account=a3"));
    // Balances recorded on the links.
    let c = f.b.run("connect.get", json!({"connection": id}));
    let a1 = c["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["remote_account_id"] == "a1")
        .unwrap()
        .clone();
    assert_eq!(a1["balance"], "1200.50");
    assert_eq!(a1["balance_date"], 1_790_812_800);
}

#[test]
fn at5_overlap_window_finds_duplicates() {
    let (mut f, id) = connected(V2);
    link_all(&mut f, &id);
    let out = sync(&mut f, true);
    let batch = f.b.run(
        "import.get",
        json!({"batch": out["connections"][0]["batch"]}),
    );
    f.b.commit(&batch);
    // Two days later the provider adds one transaction.
    let more = V2.replace(
        r#"{"id": "t2", "posted": 1790640000, "amount": "2000.00", "description": "Payroll"}"#,
        r#"{"id": "t2", "posted": 1790640000, "amount": "2000.00", "description": "Payroll"},
       {"id": "t6", "posted": 1790899200, "amount": "-60.00", "description": "Hardware"}"#,
    );
    f.fake.set_accounts(&more);
    set_now(&mut f, OCT1 + 2 * 86_400);
    let out = sync(&mut f, false);
    let r = &out["connections"][0];
    assert_eq!(r["counts"]["duplicate"], 3, "{r}");
    assert_eq!(r["counts"]["new"], 1);
    let (_, url, _) = f.fake.requests().last().unwrap().clone();
    // Last success Oct 1 minus 10 days = Sep 21.
    assert!(url.contains("start-date=1789948800"), "{url}");
    // Within the hour, an automatic sync waits; Sync now does not.
    let out = sync(&mut f, false);
    assert!(
        out["connections"][0]["skipped"]
            .as_str()
            .unwrap()
            .contains("hour")
    );
}

#[test]
fn at6_pending_replaced_by_posted() {
    let (mut f, id) = connected(V2);
    link(
        &mut f,
        &id,
        "a2",
        "Liabilities:Card",
        json!({"include_pending": true}),
    );
    let out = sync(&mut f, true);
    let first = out["connections"][0]["batch"].clone();
    let batch = f.b.run("import.get", json!({"batch": first}));
    let pending = batch["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["payee"] == "Coffee")
        .unwrap()
        .clone();
    assert_eq!(pending["needs_review"], true);
    let (_, url, _) = f.fake.requests().last().unwrap().clone();
    assert!(url.contains("pending=1"));
    // The coffee posts.
    f.fake.set_accounts(&V2.replace(
        r#""posted": 0, "amount": "-5.00", "description": "Coffee", "pending": true"#,
        r#""posted": 1790812800, "amount": "-5.25", "description": "Coffee""#,
    ));
    set_now(&mut f, OCT1 + 3 * 3600);
    let out = sync(&mut f, false);
    let second = out["connections"][0]["batch"].clone();
    let old = f.b.run("import.get", json!({"batch": first}));
    let old_coffee = old["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["payee"] == "Coffee")
        .unwrap()
        .clone();
    assert_eq!(old_coffee["action"], "skip", "{old_coffee}");
    f.b.commit(&old);
    let new = f.b.run("import.get", json!({"batch": second}));
    f.b.commit(&new);
    let coffee: Vec<Value> =
        f.b.run("txn.list", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["payee"] == "Coffee")
            .cloned()
            .collect();
    assert_eq!(coffee.len(), 1, "not duplicated");
    let t = f.b.run("txn.get", json!({"id": coffee[0]["id"]}));
    assert!(t.to_string().contains("-5.25"));
}

#[test]
fn at7_payment_required_and_revoked() {
    let (mut f, id) = connected(V2);
    link_all(&mut f, &id);
    f.fake.set_status(Some(402));
    let out = sync(&mut f, true);
    assert_eq!(out["connections"][0]["status"], "needs_attention");
    let c = f.b.run("connect.get", json!({"connection": id}));
    assert_eq!(
        c["last_error"]["message"],
        "Your SimpleFIN subscription needs payment."
    );
    let att = f.b.run("connect.attention", json!({}));
    assert!(att.to_string().contains("needs payment"));
    f.fake.set_status(Some(403));
    let out = sync(&mut f, true);
    assert_eq!(out["connections"][0]["status"], "revoked");
    let c = f.b.run("connect.get", json!({"connection": id}));
    assert!(
        c["last_error"]["message"]
            .as_str()
            .unwrap()
            .contains("reconnect with a new setup token")
    );
    assert_eq!(c["status_label"], "Revoked");
    // A server error keeps the connection active and waits an hour, then two.
    let (mut g, gid) = connected(V2);
    link_all(&mut g, &gid);
    g.fake.set_status(Some(503));
    let out = sync(&mut g, true);
    assert_eq!(out["connections"][0]["status"], "active");
    let c = g.b.run("connect.get", json!({"connection": gid}));
    assert_eq!(c["next_retry_at"], OCT1 + 3600);
    set_now(&mut g, OCT1 + 3601);
    sync(&mut g, false);
    let c = g.b.run("connect.get", json!({"connection": gid}));
    assert_eq!(c["next_retry_at"], OCT1 + 3601 + 7200);
}

#[test]
fn at8_errlist_marks_accounts_with_cleaned_messages() {
    let body = V2.replace(
        r#""errlist": []"#,
        r#""errlist": [{"code": "con.auth", "msg": "Sign in again\u0007 at \u202eFirst Bank", "conn_id": "c1"}]"#,
    );
    let (mut f, id) = connected(&body);
    link_all(&mut f, &id);
    sync(&mut f, true);
    let c = f.b.run("connect.get", json!({"connection": id}));
    let accts = c["accounts"].as_array().unwrap();
    for a in accts {
        let att = a["attention"].as_str().unwrap_or("");
        if a["conn_id"] == "c1" {
            assert!(att.contains("Sign in again"), "{a}");
            assert!(
                !att.contains('\u{7}') && !att.contains('\u{202e}'),
                "{att:?}"
            );
        } else {
            assert!(att.is_empty());
        }
    }
    assert!(
        f.b.run("connect.attention", json!({}))
            .to_string()
            .contains("Sign in again")
    );
}

#[test]
fn at10_auto_commit_like_watched_folders() {
    let (mut f, id) = connected(V2);
    link(
        &mut f,
        &id,
        "a1",
        "Assets:Checking",
        json!({"auto_commit": true}),
    );
    // A rule categorises the grocery row but nothing categorises Payroll: the batch
    // waits for review.
    f.b.run(
        "rule.create",
        json!({"name": "Groceries", "conditions": [{"field": "payee", "op": "contains", "value": "Grocery"}],
               "actions": [{"action": "set_account", "account": "Expenses:Groceries"}]}),
    );
    let out = sync(&mut f, true);
    assert!(out["connections"][0]["committed"].is_null());
    let att = f.b.run("connect.attention", json!({}));
    assert!(
        att.to_string()
            .contains("SimpleFIN: 2 new transactions to review"),
        "{att}"
    );
    // With every row categorised, the next batch commits by itself.
    let (mut g, gid) = connected(V2);
    link(
        &mut g,
        &gid,
        "a1",
        "Assets:Checking",
        json!({"auto_commit": true}),
    );
    g.b.account(json!({"name": "Salary", "type": "income"}));
    for (n, p, acc) in [
        ("G", "Grocery", "Expenses:Groceries"),
        ("P", "Payroll", "Salary"),
    ] {
        g.b.run(
            "rule.create",
            json!({"name": n, "conditions": [{"field": "payee", "op": "contains", "value": p}],
                   "actions": [{"action": "set_account", "account": acc}]}),
        );
    }
    let out = sync(&mut g, true);
    assert_eq!(out["connections"][0]["committed"]["created"], 2, "{out}");
    assert_eq!(g.b.shown("Assets:Checking", "USD"), "1954.90");
}

#[test]
fn at13_v1_accounts_with_org() {
    let v1 = r#"{
      "errors": ["Bank is slow\u0000 today"],
      "accounts": [
        {"id": "a1", "name": "Everyday", "currency": "USD", "balance": "10.00", "balance-date": 1790812800,
         "org": {"domain": "first.test", "name": "First Bank", "sfin-url": "https://bridge.test/simplefin"},
         "transactions": [{"id": "t1", "posted": 1790553600, "amount": "-45.10", "description": "Corner Grocery"}]}
      ]
    }"#;
    let mut f = feed_book(v1);
    if let Ok(mut s) = f.fake.state.lock() {
        s.versions = vec!["1".into()];
    }
    let c = add(&mut f, &FakeSimplefin::token("v1"));
    let id = c["id"].as_str().unwrap().to_owned();
    assert_eq!(c["version"], 1);
    assert_eq!(c["connections"][0]["name"], "First Bank");
    link(&mut f, &id, "a1", "Assets:Checking", json!({}));
    let out = sync(&mut f, true);
    let batch = f.b.run(
        "import.get",
        json!({"batch": out["connections"][0]["batch"]}),
    );
    assert_eq!(
        batch["sections"][0]["account_name"],
        "First Bank — Everyday"
    );
    assert_eq!(batch["rows"][0]["amount"], "-45.10");
    let c = f.b.run("connect.get", json!({"connection": id}));
    assert_eq!(c["errors"][0]["msg"], "Bank is slow today");
    let (_, url, _) = f.fake.requests().last().unwrap().clone();
    assert!(url.contains("version=1"));
}

#[test]
fn credential_missing_and_disconnect() {
    let (mut f, id) = connected(V2);
    link_all(&mut f, &id);
    // The book moved to a computer without the credential.
    f.secrets.values.lock().unwrap().clear();
    let out = sync(&mut f, true);
    assert!(
        out["connections"][0]["skipped"]
            .as_str()
            .unwrap()
            .contains("credential not found")
    );
    let (mut g, gid) = connected(V2);
    let d = g.b.run("connect.disconnect", json!({"connection": gid}));
    assert!(d["message"].as_str().unwrap().contains("revoke"));
    assert!(g.secrets.all().is_empty());
    assert_eq!(d["status"], "revoked");
    // Undo of the link restores the previous state.
    g.b.run(
        "connect.set_label",
        json!({"connection": gid, "label": "Renamed"}),
    );
    g.b.run("history.undo", json!({}));
    assert_eq!(
        g.b.run("connect.get", json!({"connection": gid}))["label"],
        "My bank"
    );
    // No network in this front end: a plain refusal.
    let mut h = Book::new();
    h.run("module.enable", json!({"module": "bank-feeds"}));
    let e = h.err(
        "connect.add",
        json!({"setup_input": FakeSimplefin::token("z"), "confirm": true}),
    );
    assert!(e.to_string().contains("network"));
}

fn session_with(fake: &FakeSimplefin, secrets: &MemorySecrets) -> (crate::Session, String) {
    let mut s = crate::Session::in_memory("T", "USD", "app").unwrap();
    s.set_connector(connector(fake, secrets, OCT1));
    for (c, a) in [
        ("module.enable", json!({"module": "bank-feeds"})),
        (
            "account.create",
            json!({"name": "Checking", "type": "asset", "subtype": "bank"}),
        ),
    ] {
        s.invoke(c, a).unwrap();
    }
    let c = s
        .invoke(
            "connect.add",
            json!({"setup_input": FakeSimplefin::token("bg"), "label": "My bank", "confirm": true}),
        )
        .unwrap();
    let id = c["id"].as_str().unwrap().to_owned();
    s.invoke(
        "connect.link_account",
        json!({"connection": id, "remote_account": "a1", "account": "Checking", "sync_from": "2026-09-01"}),
    )
    .unwrap();
    (s, id)
}

fn wait(r: &mut crate::connect::NetRunner, s: &mut crate::Session) -> CommandResult<Value> {
    for _ in 0..400 {
        if let Some((_, out)) = r.poll(s).pop() {
            return out;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("the job did not finish");
}

#[test]
fn at14_other_commands_run_during_a_sync() {
    let fake = FakeSimplefin::new(V2);
    let secrets = MemorySecrets::new(true);
    let (mut s, _) = session_with(&fake, &secrets);
    fake.state.lock().unwrap().delay_ms = 5000;
    let mut r = crate::connect::NetRunner::default();
    let started = std::time::Instant::now();
    r.start(&mut s, "connect.sync", json!({"now": true}));
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    let t = std::time::Instant::now();
    s.invoke("account.register", json!({"account": "Checking"}))
        .unwrap();
    s.invoke(
        "txn.create",
        json!({"date": "2026-10-01", "payee": "Cash", "splits": [
        {"account": "Checking", "value": "1.00"}, {"account": "Checking", "value": "-1.00"}]}),
    )
    .unwrap();
    assert!(
        t.elapsed() < std::time::Duration::from_secs(1),
        "{:?}",
        t.elapsed()
    );
    assert_eq!(r.running().len(), 1);
    let out = wait(&mut r, &mut s).unwrap();
    assert_eq!(out["connections"][0]["new"], 2, "{out}");
}

#[test]
fn at15_cancel_records_nothing() {
    let fake = FakeSimplefin::new(V2);
    let secrets = MemorySecrets::new(true);
    let (mut s, id) = session_with(&fake, &secrets);
    fake.state.lock().unwrap().delay_ms = 5000;
    let audit_before = s
        .invoke("audit.list", json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .len();
    let before = s.invoke("connect.get", json!({"connection": id})).unwrap();
    let mut r = crate::connect::NetRunner::default();
    let job = r.start(&mut s, "connect.sync", json!({"now": true}));
    std::thread::sleep(std::time::Duration::from_millis(200));
    r.cancel(job);
    let t = std::time::Instant::now();
    let e = wait(&mut r, &mut s).unwrap_err();
    assert!(t.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(e.code(), "cancelled");
    assert_eq!(
        s.invoke("audit.list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        audit_before
    );
    assert!(
        s.invoke("import.list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        s.invoke("connect.get", json!({"connection": id})).unwrap(),
        before
    );
}

#[test]
fn at16_resumable_after_a_failure_following_the_claim() {
    let fake = FakeSimplefin::new(V2);
    let secrets = MemorySecrets::new(true);
    let mut f = feed_book(V2);
    f.fake = fake.clone();
    f.secrets = secrets.clone();
    f.b.r.set_connector(connector(&fake, &secrets, OCT1));
    fake.state.lock().unwrap().list_status = Some(500);
    let c = add(&mut f, &FakeSimplefin::token("r16"));
    assert_eq!(c["status"], "unfinished");
    assert_eq!(c["status_label"], "Not finished");
    assert_eq!(c["resumable"], true);
    assert_eq!(secrets.all().len(), 1, "the credential is in the store");
    let claims = |f: &FakeSimplefin| {
        f.requests()
            .iter()
            .filter(|r| r.0 == HttpMethod::Post)
            .count()
    };
    assert_eq!(claims(&fake), 1);
    // A sync leaves it alone until it is finished.
    let out = sync(&mut f, true);
    assert!(out["connections"].as_array().unwrap().is_empty());
    fake.state.lock().unwrap().list_status = None;
    let done = f.b.run("connect.resume", json!({"connection": c["id"]}));
    assert_eq!(done["status"], "active");
    assert_eq!(done["accounts"].as_array().unwrap().len(), 3);
    assert_eq!(claims(&fake), 1, "no new claim");
    // A credential whose connection record was never written is listed after a day.
    let mut g = feed_book(V2);
    let book = g.b.s.info().unwrap().id;
    use crate::connect::SecretStore;
    g.secrets
        .put(
            &format!("{book}:orphan"),
            "https://u:p@bridge.test/simplefin",
        )
        .unwrap();
    g.secrets
        .put(
            &format!("{book}:index"),
            &json!([{"id": format!("{book}:orphan"), "at": OCT1 - 3600}]).to_string(),
        )
        .unwrap();
    assert!(
        g.b.run("connect.unused", json!({}))
            .as_array()
            .unwrap()
            .is_empty(),
        "not yet a day old"
    );
    set_now(&mut g, OCT1 + 86_400);
    let unused = g.b.run("connect.unused", json!({}));
    assert_eq!(unused[0]["credential"], format!("{book}:orphan"));
    g.b.run(
        "connect.delete_credential",
        json!({"credential": unused[0]["credential"]}),
    );
    assert!(g.secrets.all().is_empty());
    assert!(
        g.b.run("connect.unused", json!({}))
            .as_array()
            .unwrap()
            .is_empty()
    );
}

// ----- connectivity.md revision 3: transaction dates by default --------------------

/// t1 and t3 with `transacted_at` two days before `posted`.
fn with_transacted() -> String {
    V2.replace(
        r#""id": "t1", "posted": 1790553600,"#,
        r#""id": "t1", "posted": 1790553600, "transacted_at": 1790380800,"#,
    )
    .replace(
        r#""id": "t3", "posted": 1790726400,"#,
        r#""id": "t3", "posted": 1790726400, "transacted_at": 1790553600,"#,
    )
}

fn row_by_payee<'a>(batch: &'a Value, payee: &str) -> &'a Value {
    batch["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["payee"] == payee)
        .unwrap()
}

#[test]
fn connectivity_rev3_at17_transacted_at_by_default() {
    let (mut f, id) = connected(&with_transacted());
    let s = f.b.run("module.settings", json!({"module": "bank-feeds"}));
    let ds = s["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["key"] == "bank-feeds.date_source")
        .unwrap()
        .clone();
    assert_eq!(ds["value"], "transacted");
    link_all(&mut f, &id);
    let out = sync(&mut f, true);
    let batch = f.b.run(
        "import.get",
        json!({"batch": out["connections"][0]["batch"]}),
    );
    let r = row_by_payee(&batch, "Corner Grocery");
    assert_eq!(r["date"], "2026-09-26", "transacted_at, not posted (09-28)");
    assert_eq!(r["raw"]["date_kind"], "transaction");
    let r = row_by_payee(&batch, "Payroll");
    assert_eq!(r["date"], "2026-09-29");
    assert_eq!(r["raw"]["date_kind"], "posted");
}

#[test]
fn connectivity_rev3_at18_sync_from_corrects_posted_dates() {
    let (mut f, id) = connected(&with_transacted());
    f.b.run(
        "module.set_setting",
        json!({"key": "bank-feeds.date_source", "value": "posted"}),
    );
    link_all(&mut f, &id);
    let out = sync(&mut f, true);
    let batch = f.b.run(
        "import.get",
        json!({"batch": out["connections"][0]["batch"]}),
    );
    assert_eq!(row_by_payee(&batch, "Corner Grocery")["date"], "2026-09-28");
    f.b.commit(&batch);
    // Reconcile the card's Stream Co split.
    let t =
        f.b.run("txn.list", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["payee"] == "Stream Co")
            .unwrap()
            .clone();
    let idx = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .position(|s| s["account_name"] == "Liabilities:Card")
        .unwrap();
    f.b.run(
        "txn.set_reconcile",
        json!({"splits": [{"id": t["id"], "split": idx, "state": "r"}]}),
    );
    f.b.run(
        "module.set_setting",
        json!({"key": "bank-feeds.date_source", "value": "transacted"}),
    );
    let c = f.b.run("connect.get", json!({"connection": id}));
    assert_eq!(c["earliest_imported"], "2026-09-28");
    let out = f.b.run(
        "connect.sync",
        json!({"connection": id, "now": true, "from": c["earliest_imported"]}),
    );
    let (_, url, _) = f.fake.requests().last().unwrap().clone();
    // 2026-09-28 00:00 UTC.
    assert!(url.contains("start-date=1790553600"), "{url}");
    let batch = f.b.run(
        "import.get",
        json!({"batch": out["connections"][0]["batch"]}),
    );
    let grocery = row_by_payee(&batch, "Corner Grocery");
    assert_eq!(grocery["class"], "date_correction");
    assert_eq!(grocery["date_from"], "2026-09-28");
    let stream = row_by_payee(&batch, "Stream Co");
    assert_eq!(stream["class"], "duplicate");
    assert_eq!(
        stream["warnings"][0],
        "Date differs from the bank (2026-09-28); left unchanged because it is reconciled"
    );
    assert_eq!(row_by_payee(&batch, "Payroll")["class"], "duplicate");
    f.b.commit(&batch);
    let dates: Vec<(String, String)> =
        f.b.run("txn.list", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                (
                    t["payee"].as_str().unwrap().to_owned(),
                    t["date"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
    assert!(
        dates.contains(&("Corner Grocery".into(), "2026-09-26".into())),
        "{dates:?}"
    );
    assert!(
        dates.contains(&("Stream Co".into(), "2026-09-30".into())),
        "{dates:?}"
    );
}

// ----- revision 4: account kind when linking; staged rows per account ------------------

#[test]
fn connectivity_rev4_at18_kind_hint_from_the_account_name() {
    let body = V2
        .replace(r#""name": "Card""#, r#""name": "Chase Sapphire Visa""#)
        .replace(r#""name": "Euro""#, r#""name": "Home Mortgage""#);
    let (mut f, id) = connected(&body);
    let c = f.b.run("connect.get", json!({"connection": id}));
    let hint = |remote: &str| {
        c["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["remote_account_id"] == remote)
            .map(|a| a["kind_hint"].clone())
            .unwrap()
    };
    assert_eq!(hint("a1"), "bank");
    assert_eq!(hint("a2"), "credit-card");
    assert_eq!(hint("a3"), "loan");
    // The link step creates the account with that type and subtype, then links it.
    let acc = f.b.run(
        "account.create",
        json!({"name": "Chase Sapphire Visa", "type": "liability", "subtype": "credit-card"}),
    );
    let l = link(
        &mut f,
        &id,
        "a2",
        acc["full_name"].as_str().unwrap(),
        json!({}),
    );
    assert_eq!(l["account_name"], "Chase Sapphire Visa");
    assert_eq!(l["kind_hint"], "credit-card");
}

#[test]
fn connectivity_rev4_kind_hint_rules() {
    let l = |name: &str| crate::connect::LinkDoc {
        id: String::new(),
        connection: String::new(),
        remote_account_id: String::new(),
        name: name.to_owned(),
        conn_id: String::new(),
        currency: None,
        balance: None,
        available: None,
        balance_date: None,
        account: None,
        sync_from: None,
        include_pending: false,
        auto_commit: false,
        last_seen: None,
        last_success: None,
        attention: None,
        active: true,
        account_key: String::new(),
        remote: None,
        dismissed: None,
    };
    use crate::connect::kind_hint;
    assert_eq!(kind_hint(&l("AMEX Gold"), ""), "credit-card");
    assert_eq!(kind_hint(&l("MasterCard"), ""), "credit-card");
    assert_eq!(kind_hint(&l("Auto Loan"), ""), "loan");
    assert_eq!(kind_hint(&l("Savings"), "Navy Credit Union"), "bank");
    assert_eq!(kind_hint(&l("Account 1234"), "Discover"), "credit-card");
    let mut ofx = l("Line");
    ofx.remote = Some(crate::connect::RemoteInfo {
        kind: "bank".into(),
        acct_type: "CHECKING".into(),
        ..Default::default()
    });
    assert_eq!(kind_hint(&ofx, ""), "bank");
    ofx.name = "Visa".into();
    assert_eq!(kind_hint(&ofx, ""), "bank", "OFX says what it is");
    ofx.remote.as_mut().unwrap().kind = "card".into();
    assert_eq!(kind_hint(&ofx, ""), "credit-card");
}

#[test]
fn connectivity_rev4_at19_attention_counts_rows_per_account() {
    let (mut f, id) = connected(V2);
    link_all(&mut f, &id);
    let out = sync(&mut f, true);
    let batch = out["connections"][0]["batch"].clone();
    let checking =
        f.b.run("account.get", json!({"account": "Assets:Checking"}))["id"].clone();
    let card =
        f.b.run("account.get", json!({"account": "Liabilities:Card"}))["id"].clone();
    let att = f.b.run("connect.attention", json!({}));
    let item = att
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "batch")
        .unwrap()
        .clone();
    assert_eq!(item["batch"], batch);
    assert_eq!(item["connection"], json!(id));
    assert_eq!(item["provider"], "SimpleFIN");
    assert_eq!(item["count"], 3);
    let per = item["accounts"].as_array().unwrap();
    let of = |a: &Value| {
        per.iter()
            .find(|p| p["account"] == *a)
            .map(|p| p["count"].clone())
    };
    assert_eq!(of(&checking), Some(json!(2)));
    assert_eq!(of(&card), Some(json!(1)));
}

// ----- revision 5: SimpleFIN 90-day cap; informational notices; Dismiss ----------------

#[test]
fn connectivity_rev5_at20_start_date_clamped_to_90_days() {
    let (mut f, id) = connected(V2);
    // 200 days before 2026-10-01.
    link(
        &mut f,
        &id,
        "a1",
        "Assets:Checking",
        json!({"sync_from": "2026-03-15"}),
    );
    let out = sync(&mut f, true);
    let (_, url, _) = f.fake.requests().last().unwrap().clone();
    // 2026-10-01 − 90 days = 2026-07-03 00:00 UTC.
    let earliest = OCT1 - 90 * 86_400;
    assert!(url.contains(&format!("start-date={earliest}")), "{url}");
    let notes = out["connections"][0]["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1, "said once: {notes:?}");
    assert!(notes[0].as_str().unwrap().contains("at most 90 days"));
    // An explicit `from` is clamped too.
    f.b.run(
        "connect.sync",
        json!({"connection": id, "now": true, "from": "2026-01-01"}),
    );
    let (_, url, _) = f.fake.requests().last().unwrap().clone();
    assert!(url.contains(&format!("start-date={earliest}")), "{url}");
    // Within 90 days: unchanged, no note.
    let (mut g, gid) = connected(V2);
    link_all(&mut g, &gid);
    let out = sync(&mut g, true);
    assert!(
        out["connections"][0]["notes"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "{out}"
    );
}

#[test]
fn connectivity_rev5_at21_capped_errlist_is_informational() {
    let body = V2.replace(
        r#""errlist": []"#,
        r#""errlist": [{"code": "gen.range", "msg": "Requested date range exceeds limit of 90 days and was capped."}]"#,
    );
    let (mut f, id) = connected(&body);
    link_all(&mut f, &id);
    let out = sync(&mut f, true);
    let r = &out["connections"][0];
    assert_eq!(r["status"], "active", "{r}");
    assert!(r["notes"].to_string().contains("was capped"), "{r}");
    let c = f.b.run("connect.get", json!({"connection": id}));
    assert_eq!(c["status"], "active");
    assert!(c["last_error"].is_null());
    let att = f.b.run("connect.attention", json!({}));
    assert!(
        !att.as_array()
            .unwrap()
            .iter()
            .any(|i| i["kind"] == "connection"),
        "{att}"
    );
    // A real general failure still flags the connection.
    assert!(!crate::connect::informational(
        "gen.auth",
        "Authentication failed"
    ));
    assert!(!crate::connect::informational("con.auth", "limit reached"));
}

#[test]
fn connectivity_rev5_at22_dismiss_hides_until_a_success_in_between() {
    let failing = V2.replace(
        r#""errlist": []"#,
        r#""errlist": [{"code": "gen.auth", "msg": "Authentication failed"}]"#,
    );
    let (mut f, id) = connected(&failing);
    link_all(&mut f, &id);
    let conn_items = |f: &mut Feed| -> Vec<Value> {
        f.b.run("connect.attention", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["kind"] == "connection")
            .cloned()
            .collect()
    };
    sync(&mut f, true);
    let items = conn_items(&mut f);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["message"], "My bank: Authentication failed");
    assert_eq!(items[0]["dismissible"], true);
    // Batches to review are not dismissible.
    let att = f.b.run("connect.attention", json!({}));
    for i in att
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == "batch")
    {
        assert!(i.get("dismissible").is_none(), "{i}");
    }
    let out = f.b.run("connect.dismiss", items[0]["dismiss"].clone());
    assert_eq!(out["status"], "active");
    assert!(conn_items(&mut f).is_empty());
    let c = f.b.run("connect.get", json!({"connection": id}));
    assert_eq!(
        c["status"], "active",
        "needs attention only from this message"
    );
    assert_eq!(c["notice_dismissed"], true);
    // Undo restores it.
    f.b.run("history.undo", json!({}));
    assert_eq!(conn_items(&mut f).len(), 1);
    f.b.run("history.redo", json!({}));
    assert!(conn_items(&mut f).is_empty());
    // The same failure again, no success in between: still hidden.
    set_now(&mut f, OCT1 + 7200);
    sync(&mut f, true);
    assert!(conn_items(&mut f).is_empty());
    // A success, then the failure again: shown.
    f.fake.set_accounts(V2);
    set_now(&mut f, OCT1 + 4 * 3600);
    sync(&mut f, true);
    assert!(conn_items(&mut f).is_empty());
    f.fake.set_accounts(&failing);
    set_now(&mut f, OCT1 + 6 * 3600);
    sync(&mut f, true);
    assert_eq!(conn_items(&mut f).len(), 1);
    // A stale message is refused.
    let e = f.b.try_run(
        "connect.dismiss",
        json!({"connection": id, "message": "Something else"}),
    );
    assert!(e.is_err());
}

#[test]
fn connectivity_rev5_dismiss_an_account_notice() {
    let body = V2.replace(
        r#""errlist": []"#,
        r#""errlist": [{"code": "con.auth", "msg": "Sign in again", "conn_id": "c1"}]"#,
    );
    let (mut f, id) = connected(&body);
    link_all(&mut f, &id);
    sync(&mut f, true);
    let att = f.b.run("connect.attention", json!({}));
    let items: Vec<&Value> = att
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["kind"] == "connection")
        .collect();
    assert!(!items.is_empty());
    for i in &items {
        assert!(i["dismiss"]["link"].is_string(), "{i}");
        f.b.run("connect.dismiss", i["dismiss"].clone());
    }
    let att = f.b.run("connect.attention", json!({}));
    assert!(!att.to_string().contains("Sign in again"), "{att}");
}
