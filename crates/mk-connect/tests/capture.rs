//! mobile-capture.md desktop acceptance tests 11–13: the `capture.*` commands talk
//! to the real capture server running on 127.0.0.1 (Access checks off, allowed only
//! on loopback) through this crate's fetcher. No internet.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::sync::Arc;

use mk_capture_api::{ExpenseFields, NewExpense};
use mk_commands::Session;
use mk_commands::connect::{Connector, MemorySecrets};
use mk_connect::HttpsFetcher;
use moneykeep_server::testing::{LocalServer, call};
use serde_json::{Value, json};

struct T {
    s: Session,
    server: LocalServer,
    secrets: MemorySecrets,
    _book: tempfile::TempDir,
    _data: tempfile::TempDir,
}

fn photo(seed: u32) -> Vec<u8> {
    let (w, h) = (320u32, 240u32);
    let mut x = seed;
    let data = (0..w * h * 3)
        .map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (x >> 16) as u8
        })
        .collect();
    mk_import::jpeg::encode_jpeg(
        &mk_import::image::Rgb {
            width: w,
            height: h,
            data,
        },
        85,
    )
    .unwrap()
}

fn setup() -> T {
    let book = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let server = LocalServer::start(data.path()).unwrap();
    let mut s = Session::create(
        &book.path().join("b.moneykeep"),
        "Phone Books",
        "USD",
        "app",
    )
    .unwrap();
    let secrets = MemorySecrets::new(true);
    s.set_connector(Connector {
        fetcher: Arc::new(HttpsFetcher::trusting(Vec::new())),
        secrets: Box::new(secrets.clone()),
        utc_offset_secs: 0,
        now: None,
        host: None,
    });
    s.invoke("module.enable", json!({"module": "expenses"}))
        .unwrap();
    for args in [
        json!({"name": "Expenses", "type": "expense", "placeholder": true}),
        json!({"name": "Office Supplies", "type": "expense", "parent": "Expenses", "code": "6100"}),
        json!({"name": "Liabilities", "type": "liability", "placeholder": true}),
        json!({"name": "Visa", "type": "liability", "subtype": "credit-card", "parent": "Liabilities", "code": "2100"}),
        json!({"name": "Checking", "type": "asset", "subtype": "bank"}),
    ] {
        s.invoke("account.create", args).unwrap();
    }
    s.invoke(
        "capture.set_server",
        json!({"url": server.url(), "token": server.desktop_token}),
    )
    .unwrap();
    T {
        s,
        server,
        secrets,
        _book: book,
        _data: data,
    }
}

fn today() -> String {
    mk_capture_api::format_date(mk_capture_api::today_from_unix(
        moneykeep_server::Clock::default().now(),
        0,
    ))
}

fn submission(client: &str, category: &str, receipts: Vec<String>) -> NewExpense {
    NewExpense {
        client_id: client.into(),
        fields: ExpenseFields {
            date: Some(today()),
            merchant: Some("Office Depot".into()),
            amount: Some("45.20".into()),
            category: Some(category.into()),
            paid_with: Some("Liabilities:Visa".into()),
            ..ExpenseFields::default()
        },
        receipt_ids: receipts,
    }
}

fn waiting_on_server(t: &T) -> usize {
    let (s, b) = call(
        &t.server.url(),
        "GET",
        "/api/v1/desktop/inbox",
        &[(
            "Authorization",
            &format!("Bearer {}", t.server.desktop_token),
        )],
        None,
    )
    .unwrap();
    assert_eq!(s, 200);
    let v: Value = serde_json::from_slice(&b).unwrap();
    v["items"].as_array().unwrap().len()
}

/// AT11: one ready expense with its receipt and one inbox receipt; both acknowledged;
/// a second collect creates nothing.
#[test]
fn at11_collect() {
    let mut t = setup();
    let phone = t.server.pair_phone("Owner's phone").unwrap();
    let r1 = t.server.upload(&phone, &photo(1)).unwrap();
    let s = t.server.submit(
        &phone,
        &submission(
            "6f1c2d3e-0000-4000-8000-000000000011",
            "Expenses:Office Supplies",
            vec![r1],
        ),
    );
    assert_eq!(s.unwrap(), 201);
    let r2 = t.server.upload(&phone, &photo(2)).unwrap();
    let only = NewExpense {
        client_id: "6f1c2d3e-0000-4000-8000-000000000012".into(),
        fields: ExpenseFields::default(),
        receipt_ids: vec![r2],
    };
    assert_eq!(t.server.submit(&phone, &only).unwrap(), 201);
    assert_eq!(waiting_on_server(&t), 2);

    let out = t.s.invoke("capture.collect", json!({})).unwrap();
    assert_eq!(out["expenses"], 1, "{out}");
    assert_eq!(out["ready"], 1, "{out}");
    assert_eq!(out["inbox_receipts"], 1, "{out}");
    assert_eq!(out["acknowledged"], 2, "{out}");
    assert_eq!(out["lists_published"], true, "{out}");

    let list = t.s.invoke("expense.list", json!({})).unwrap();
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 1);
    let e = &list[0];
    assert_eq!(e["source"], "phone");
    assert_eq!(e["status"], "ready");
    assert_eq!(e["merchant"], "Office Depot");
    assert_eq!(e["amount"], "45.20");
    assert_eq!(e["category_name"], "Expenses:Office Supplies");
    assert_eq!(e["receipt_count"], 1);
    assert_eq!(e["receipts"][0]["source"], "phone");
    let inbox = t.s.invoke("receipt.inbox", json!({})).unwrap();
    assert_eq!(inbox.as_array().unwrap().len(), 1, "{inbox}");
    assert_eq!(inbox[0]["source"], "phone");
    assert_eq!(waiting_on_server(&t), 0);

    let again = t.s.invoke("capture.collect", json!({})).unwrap();
    assert_eq!(again["expenses"], 0, "{again}");
    assert_eq!(again["inbox_receipts"], 0);
    assert_eq!(
        t.s.invoke("expense.list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        t.s.invoke("receipt.inbox", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // The token never reaches the book's audit log or the settings output.
    let settings = t.s.invoke("capture.settings", json!({})).unwrap();
    assert_eq!(settings["configured"], true);
    assert!(!settings.to_string().contains(&t.server.desktop_token));
    for cmd in ["history.list", "audit.list"] {
        let log = t.s.invoke(cmd, json!({})).unwrap();
        assert!(!log.to_string().contains(&t.server.desktop_token), "{cmd}");
    }
    assert!(
        t.secrets
            .all()
            .values()
            .any(|v| *v == t.server.desktop_token)
    );
}

/// AT12: a category the book does not have leaves a draft with a note.
#[test]
fn at12_unknown_category_makes_a_draft() {
    let mut t = setup();
    let phone = t.server.pair_phone("Phone").unwrap();
    t.server
        .submit(
            &phone,
            &submission(
                "6f1c2d3e-0000-4000-8000-000000000021",
                "Travel:Taxis",
                vec![],
            ),
        )
        .unwrap();
    let out = t.s.invoke("capture.collect", json!({})).unwrap();
    assert_eq!(out["drafts"], 1, "{out}");
    let e = &t.s.invoke("expense.list", json!({})).unwrap()[0];
    assert_eq!(e["status"], "draft");
    assert!(e["category"].is_null());
    assert_eq!(e["notes"][0], "Category from phone not found: Travel:Taxis");
    assert_eq!(e["paid_with_name"], "Liabilities:Visa");
}

/// AT13: the published lists hold category and paid-with names only.
#[test]
fn at13_lists_hold_names_only() {
    let mut t = setup();
    t.s.invoke(
        "txn.create",
        json!({"date": today(), "payee": "Secret Payee Inc", "splits": [
            {"account": "Expenses:Office Supplies", "value": "123.45"},
            {"account": "Checking", "value": "-123.45"}]}),
    )
    .unwrap();
    let out = t.s.invoke("capture.publish_lists", json!({})).unwrap();
    assert_eq!(out["published"], true, "{out}");
    let hash = out["hash"].as_str().unwrap().to_owned();
    let same =
        t.s.invoke("capture.publish_lists", json!({"known_hash": hash}))
            .unwrap();
    assert_eq!(same["unchanged"], true);
    let phone = t.server.pair_phone("Phone").unwrap();
    let (s, b) = call(
        &t.server.url(),
        "GET",
        "/api/v1/lists",
        &[("Authorization", &format!("Bearer {phone}"))],
        None,
    )
    .unwrap();
    assert_eq!(s, 200);
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["categories"], json!(["Expenses:Office Supplies"]));
    assert_eq!(
        v["paid_with"],
        json!([
            "Checking",
            "Liabilities:Visa",
            "Personal funds (reimbursable)"
        ])
    );
    assert_eq!(v["book_name"], "Phone Books");
    assert_eq!(v["base_currency"], "USD");
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "base_currency",
            "book_id",
            "book_name",
            "categories",
            "merchants",
            "paid_with",
            "published_at",
            "tags"
        ]
    );
    let text = v.to_string();
    for forbidden in ["123.45", "6100", "2100", "Secret Payee", "balance"] {
        assert!(!text.contains(forbidden), "{forbidden} in {text}");
    }
}

/// Test connection, pairing codes, devices and revoking through the commands.
#[test]
fn test_pair_and_devices() {
    let mut t = setup();
    let ok = t.s.invoke("capture.test", json!({})).unwrap();
    assert_eq!(ok["ok"], true);
    let code = t.s.invoke("capture.pair_code", json!({})).unwrap();
    assert_eq!(code["code"].as_str().unwrap().len(), 8);
    assert_eq!(code["expires_in"], 600);
    assert_eq!(code["client_url"], "https://capture.example.test");
    t.server.pair_phone("Second phone").unwrap();
    let d = t.s.invoke("capture.devices", json!({})).unwrap();
    let id = d["devices"][0]["id"].as_str().unwrap().to_owned();
    t.s.invoke("capture.revoke_device", json!({"device": id}))
        .unwrap();
    let d = t.s.invoke("capture.devices", json!({})).unwrap();
    assert!(d["devices"].as_array().unwrap().is_empty());
    // A wrong token is reported plainly.
    t.s.invoke(
        "capture.set_server",
        json!({"url": t.server.url(), "token": "A".repeat(43)}),
    )
    .unwrap();
    let e = t.s.invoke("capture.test", json!({})).unwrap_err();
    assert_eq!(e.code(), "bad_token");
    // http:// to a LAN address needs the trusted tick; https is required elsewhere.
    let e =
        t.s.invoke(
            "capture.set_server",
            json!({"url": "http://192.168.1.5:8787"}),
        )
        .unwrap_err();
    assert!(e.to_string().contains("trusted"), "{e}");
}
