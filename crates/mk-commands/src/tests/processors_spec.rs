//! connectivity-processors.md acceptance tests 1–11 and 13–18 (12 is in mk-mcp). Answers come
//! from the scripted processor server (shapes from the vendors' documentation); no
//! network.

use super::*;
use crate::connect::{Connector, HttpMethod, MemorySecrets};
use crate::processors::FakeProcessors;

/// 2026-10-04 00:00 UTC.
const NOW: i64 = 1_791_072_000;
const DAY: i64 = 86_400;
/// 2026-10-03.
const OCT3: i64 = NOW - DAY;

struct Proc {
    b: Book,
    fake: FakeProcessors,
    secrets: MemorySecrets,
}

fn connector(fake: &FakeProcessors, secrets: &MemorySecrets, now: i64) -> Connector {
    Connector {
        fetcher: std::sync::Arc::new(fake.clone()),
        secrets: Box::new(secrets.clone()),
        utc_offset_secs: 0,
        now: Some(now),
        host: None,
    }
}

fn proc_book() -> Proc {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "payment-processing"}));
    let fake = FakeProcessors::default();
    let secrets = MemorySecrets::new(true);
    b.r.set_connector(connector(&fake, &secrets, NOW));
    Proc { b, fake, secrets }
}

fn set_now(p: &mut Proc, now: i64) {
    if let Some(c) = p.b.r.connector_mut() {
        c.now = Some(now);
    }
}

fn bt(id: &str, kind: &str, cat: &str, amount: i64, fee: i64, desc: &str, created: i64) -> Value {
    json!({"id": id, "object": "balance_transaction", "amount": amount, "available_on": created,
           "created": created, "currency": "usd", "description": desc, "exchange_rate": null,
           "fee": fee, "fee_details": [], "net": amount - fee, "reporting_category": cat,
           "source": format!("src_{id}"), "status": "available", "type": kind})
}

/// A Stripe Payout object (<https://docs.stripe.com/api/payouts/object>).
fn payout_obj(id: &str, amount: i64, status: &str, arrival: i64) -> Value {
    json!({"id": id, "object": "payout", "amount": amount, "arrival_date": arrival,
           "automatic": true, "created": arrival - DAY, "currency": "usd",
           "description": "STRIPE PAYOUT", "failure_code": null, "failure_message": null,
           "method": "standard", "source_type": "card", "status": status,
           "statement_descriptor": null, "type": "bank_account"})
}

/// The AT1 payout: three charges (1,000.00 with 29.90 in fees), a 0.60 Stripe fee and a
/// 24.20 refund; net 945.30, arriving Oct 3.
fn stripe_payout(fake: &FakeProcessors, charge_desc: &str) {
    fake.with(|s| {
        let mut payout = bt(
            "txn_po1",
            "payout",
            "payout",
            -94_530,
            0,
            "STRIPE PAYOUT",
            OCT3,
        );
        payout["source"] = json!("po_1");
        s.stripe_payouts = vec![payout_obj("po_1", 94_530, "paid", OCT3)];
        s.stripe_entries.insert(
            "po_1".into(),
            vec![
                bt(
                    "txn_c1",
                    "charge",
                    "charge",
                    40_000,
                    1_190,
                    "Order 1001",
                    OCT3 - 3 * DAY,
                ),
                bt(
                    "txn_c2",
                    "charge",
                    "charge",
                    35_000,
                    1_045,
                    "Order 1002",
                    OCT3 - 3 * DAY,
                ),
                bt(
                    "txn_c3",
                    "charge",
                    "charge",
                    25_000,
                    755,
                    charge_desc,
                    OCT3 - 2 * DAY,
                ),
                bt(
                    "txn_f1",
                    "stripe_fee",
                    "fee",
                    -60,
                    0,
                    "Radar",
                    OCT3 - 2 * DAY,
                ),
                bt(
                    "txn_r1",
                    "refund",
                    "refund",
                    -2_420,
                    0,
                    "Refund for Order 1001",
                    OCT3 - DAY,
                ),
                payout,
            ],
        );
    });
}

fn add_stripe(p: &mut Proc) -> Value {
    p.b.run(
        "connect.add",
        json!({"provider": "stripe", "setup_input": "rk_live_51TestRestrictedKey", "label": "Shop", "confirm": true}),
    )
}

fn sync(p: &mut Proc) -> Value {
    p.b.run("connect.sync", json!({"now": true}))
}

fn batch_of(p: &mut Proc, out: &Value) -> Value {
    p.b.run(
        "import.get",
        json!({"batch": out["connections"][0]["batch"]}),
    )
}

fn txn_with(p: &mut Proc, payee: &str) -> Value {
    let list = p.b.run("txn.list", json!({}));
    let t = list
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["payee"] == payee)
        .unwrap_or_else(|| panic!("no {payee} in {list}"))
        .clone();
    p.b.run("txn.get", json!({"id": t["id"]}))
}

fn split_value(t: &Value, account: &str) -> Vec<String> {
    t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["account_name"] == account)
        .map(|s| s["value"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn at1_stripe_summary_payout_and_bank_match() {
    let mut p = proc_book();
    stripe_payout(&p.fake, "Order 1003");
    let c = add_stripe(&mut p);
    assert_eq!(c["host"], "api.stripe.com");
    assert_eq!(c["processor"]["clearing"]["name"], "Stripe Clearing");
    p.b.account(json!({"name": "Refunds", "type": "income"}));
    p.b.run(
        "processor.set",
        json!({"connection": c["id"], "accounts": {"refund": "Refunds"}}),
    );
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    assert_eq!(batch["source"], "Stripe — Shop");
    let rows = batch["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["amount"], "945.30");
    assert_eq!(rows[0]["date"], "2026-10-03");
    let raw: Value = serde_json::from_str(rows[0]["raw"]["raw"].as_str().unwrap()).unwrap();
    assert_eq!(raw["entries"].as_array().unwrap().len(), 5);
    assert_eq!(raw["fees"], "-30.50");
    p.b.commit(&batch);
    let t = txn_with(&mut p, "Stripe payout");
    assert_eq!(split_value(&t, "Sales"), ["-1000.00"]);
    assert_eq!(split_value(&t, "Refunds"), ["24.20"]);
    assert_eq!(split_value(&t, "Merchant Fees"), ["30.50"]);
    assert_eq!(split_value(&t, "Stripe Clearing"), ["945.30"]);
    // The bank deposit two days later is a transfer from the clearing account.
    let qif = "!Type:Bank\nD10/05/2026\nT945.30\nPSTRIPE TRANSFER\n^\n";
    let bank = p.b.stage_qif(qif, "Assets:Checking");
    let row = &bank["rows"][0];
    assert_eq!(row["other"][0]["account_name"], "Stripe Clearing", "{row}");
    p.b.commit(&bank);
    assert_eq!(p.b.shown("Stripe Clearing", "USD"), "0.00");
    // Requests: Basic auth with the key, only to the API host.
    for (_, url, _, basic, _) in p.fake.requests() {
        assert!(url.starts_with("https://api.stripe.com/"), "{url}");
        assert!(basic);
    }
}

#[test]
fn at2_resync_is_a_duplicate() {
    let mut p = proc_book();
    stripe_payout(&p.fake, "Order 1003");
    add_stripe(&mut p);
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    p.b.commit(&batch);
    set_now(&mut p, NOW + 2 * 3600);
    let out = sync(&mut p);
    assert_eq!(out["connections"][0]["counts"]["duplicate"], 1, "{out}");
    assert_eq!(out["connections"][0]["new"], 0);
    // A payout reported with a new entry after posting is flagged with the difference.
    p.fake.with(|s| {
        let e = s.stripe_entries.get_mut("po_1").unwrap();
        e.push(bt("txn_f2", "stripe_fee", "fee", -100, 0, "Late fee", OCT3));
        s.stripe_payouts[0]["amount"] = json!(94_430);
    });
    set_now(&mut p, NOW + 4 * 3600);
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    let changed = batch["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["amount"] == "-1.00")
        .unwrap()
        .clone();
    assert_eq!(changed["needs_review"], true);
    assert!(
        changed["warnings"]
            .to_string()
            .contains("changed after it was posted")
    );
}

#[test]
fn at3_detailed_mode() {
    let mut p = proc_book();
    stripe_payout(&p.fake, "Order 1003");
    let c = add_stripe(&mut p);
    p.b.run(
        "processor.set",
        json!({"connection": c["id"], "mode": "detailed"}),
    );
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    let rows = batch["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 6, "five entries and the payout");
    assert_eq!(rows.iter().filter(|r| r["amount"] == "-945.30").count(), 1);
    p.b.commit(&batch);
    assert_eq!(p.b.shown("Stripe Clearing", "USD"), "0.00");
    assert_eq!(p.b.shown("Stripe Payouts in Transit", "USD"), "945.30");
    assert_eq!(p.b.shown("Merchant Fees", "USD"), "30.50");
    assert_eq!(p.b.shown("Sales", "USD"), "975.80");
    let qif = "!Type:Bank\nD10/05/2026\nT945.30\nPSTRIPE\n^\n";
    let bank = p.b.stage_qif(qif, "Assets:Checking");
    assert_eq!(
        bank["rows"][0]["other"][0]["account_name"],
        "Stripe Payouts in Transit"
    );
    p.b.commit(&bank);
    assert_eq!(p.b.shown("Stripe Payouts in Transit", "USD"), "0.00");
}

#[test]
fn at4_invoice_number_proposes_a_payment() {
    let mut p = proc_book();
    p.b.run("module.enable", json!({"module": "invoicing"}));
    p.b.account(json!({"name": "Consulting Income", "type": "income"}));
    p.b.run("party.create", json!({"name": "Acme LLC"}));
    p.b.run(
        "module.set_setting",
        json!({"key": "invoicing.next_number", "value": 1002}),
    );
    let inv = p.b.run(
        "invoice.create",
        json!({"customer": "Acme LLC", "date": "2026-09-20", "lines": [
            {"account": "Consulting Income", "description": "Work", "amount": "250.00"}]}),
    );
    assert_eq!(inv["number"], "INV-1002");
    stripe_payout(&p.fake, "Payment for INV-1002");
    add_stripe(&mut p);
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    let row = batch["rows"][0].clone();
    assert!(
        row["warnings"].to_string().contains("Apply to INV-1002"),
        "{row}"
    );
    assert_eq!(row["needs_review"], true);
    let batch = p.b.run(
        "import.set_row",
        json!({"batch": batch["id"], "row": row["id"], "apply_invoices": true}),
    );
    p.b.commit(&batch);
    let i = p.b.run("invoice.get", json!({"invoice": "INV-1002"}));
    assert_eq!(i["status"], "paid", "{i}");
    let pays = p.b.run("payment.list", json!({}));
    assert_eq!(pays[0]["deposit_to_name"], "Stripe Clearing");
    assert_eq!(p.b.shown("Stripe Clearing", "USD"), "945.30");
    assert_eq!(p.b.shown("Sales", "USD"), "725.80");
}

#[test]
fn at5_secret_key_refused_before_any_request() {
    let mut p = proc_book();
    let e = p.b.err(
        "connect.add",
        json!({"provider": "stripe", "setup_input": "sk_live_51SecretKeyHere", "confirm": true}),
    );
    assert_eq!(e.code(), "invalid_args");
    assert!(e.to_string().contains("restricted key"), "{e}");
    assert!(p.fake.requests().is_empty());
}

fn square_entry(id: &str, kind: &str, gross: i64, net: i64, when: &str) -> Value {
    json!({"id": id, "payout_id": "x", "effective_at": when, "type": kind,
           "gross_amount_money": {"amount": gross, "currency_code": "USD"},
           "fee_amount_money": {"amount": gross - net, "currency_code": "USD"},
           "net_amount_money": {"amount": net, "currency_code": "USD"},
           "type_charge_details": {"payment_id": format!("pay_{id}")}})
}

#[test]
fn at6_square_two_locations() {
    let mut p = proc_book();
    p.fake.with(|s| {
        s.square_locations = vec![
            json!({"id": "L1", "name": "Main Street", "currency": "USD"}),
            json!({"id": "L2", "name": "Market Stall", "currency": "USD"}),
        ];
        for (loc, po) in [("L1", "po_a"), ("L2", "po_b")] {
            s.square_payouts.insert(
                loc.into(),
                vec![json!({"id": po, "status": "PAID", "location_id": loc,
                    "created_at": "2026-10-02T10:00:00Z", "arrival_date": "2026-10-03",
                    "amount_money": {"amount": 9_710, "currency_code": "USD"}, "type": "BATCH"})],
            );
            s.square_entries.insert(
                po.into(),
                vec![
                    square_entry(
                        &format!("{po}_1"),
                        "CHARGE",
                        15_000,
                        14_535,
                        "2026-10-01T03:07:09Z",
                    ),
                    square_entry(
                        &format!("{po}_2"),
                        "REFUND",
                        -5_000,
                        -5_000,
                        "2026-10-01T05:00:00Z",
                    ),
                    square_entry(&format!("{po}_3"), "FEE", 0, 175, "2026-10-01T06:00:00Z"),
                ],
            );
        }
    });
    let c = p.b.run(
        "connect.add",
        json!({"provider": "square", "setup_input": "EAAAEtestsquaretoken1234", "confirm": true}),
    );
    assert_eq!(c["host"], "connect.squareup.com");
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    let sections = batch["sections"].as_array().unwrap();
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0]["account_name"], "Square — Main Street");
    let rows = batch["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["amount"], "97.10");
    p.b.commit(&batch);
    let t = txn_with(&mut p, "Square payout");
    assert_eq!(split_value(&t, "Sales").len(), 2, "sales and refunds");
    assert!(split_value(&t, "Merchant Fees") == ["2.90"], "{t}");
    for (_, url, headers, _, _) in p.fake.requests() {
        assert!(url.starts_with("https://connect.squareup.com/v2/"), "{url}");
        assert!(headers.iter().any(|h| h == "Authorization"));
    }
}

fn paypal_txn(
    id: &str,
    code: &str,
    when: &str,
    amount: &str,
    fee: Option<&str>,
    subject: &str,
) -> Value {
    let mut t = json!({"transaction_info": {"transaction_id": id, "transaction_event_code": code,
        "transaction_initiation_date": when, "transaction_updated_date": when,
        "transaction_amount": {"currency_code": "USD", "value": amount},
        "transaction_status": "S", "transaction_subject": subject}});
    if let Some(f) = fee {
        t["transaction_info"]["fee_amount"] = json!({"currency_code": "USD", "value": f});
    }
    t
}

fn add_paypal(p: &mut Proc) -> Value {
    p.b.run(
        "connect.add",
        json!({"provider": "paypal", "setup_input": "AbCdClientId:EfGhClientSecret", "label": "PayPal", "confirm": true}),
    )
}

#[test]
fn at7_paypal_windows_and_payout_groups() {
    let mut p = proc_book();
    p.b.run(
        "module.set_setting",
        json!({"key": "processors.history_days_first_sync", "value": 40}),
    );
    p.fake.with(|s| {
        s.paypal = vec![
            paypal_txn(
                "S1",
                "T0006",
                "2026-08-28T10:00:00Z",
                "100.00",
                Some("-3.20"),
                "Sale 1",
            ),
            paypal_txn(
                "S2",
                "T0006",
                "2026-09-10T10:00:00Z",
                "100.00",
                Some("-3.20"),
                "Sale 2",
            ),
            paypal_txn(
                "A1",
                "T0106",
                "2026-09-12T10:00:00Z",
                "-1.00",
                None,
                "Fee adjustment",
            ),
            paypal_txn(
                "S3",
                "T0006",
                "2026-09-20T10:00:00Z",
                "100.00",
                Some("-3.20"),
                "Sale 3",
            ),
            paypal_txn(
                "W1",
                "T0400",
                "2026-09-25T10:00:00Z",
                "-289.40",
                None,
                "Withdrawal",
            ),
            paypal_txn(
                "S4",
                "T0006",
                "2026-10-01T10:00:00Z",
                "50.00",
                Some("-1.75"),
                "Sale 4",
            ),
        ];
    });
    add_paypal(&mut p);
    let out = sync(&mut p);
    let windows: Vec<String> = p
        .fake
        .requests()
        .iter()
        .filter(|r| r.1.contains("/v1/reporting/transactions"))
        .map(|r| r.1.clone())
        .collect();
    assert_eq!(windows.len(), 2, "{windows:?}");
    let batch = batch_of(&mut p, &out);
    let rows = batch["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["amount"], "289.40");
    assert_eq!(rows[0]["date"], "2026-09-25");
    let raw: Value = serde_json::from_str(rows[0]["raw"]["raw"].as_str().unwrap()).unwrap();
    assert_eq!(raw["entries"].as_array().unwrap().len(), 4);
    assert!(
        batch["warnings"].to_string().contains("pending payout"),
        "{}",
        batch["warnings"]
    );
    let s =
        p.b.run("processor.settings", json!({"connection": "PayPal"}));
    assert_eq!(s["pending_payout"], 1);
}

#[test]
fn at8_paypal_token_never_stored() {
    let mut p = proc_book();
    p.fake.with(|s| {
        s.paypal = vec![paypal_txn(
            "W1",
            "T0400",
            "2026-09-25T10:00:00Z",
            "-10.00",
            None,
            "Withdrawal",
        )];
    });
    let c = add_paypal(&mut p);
    sync(&mut p);
    let reqs = p.fake.requests();
    let token: Vec<_> = reqs
        .iter()
        .filter(|r| r.1.ends_with("/v1/oauth2/token"))
        .collect();
    assert_eq!(token.len(), 2, "verify and sync");
    assert!(token.iter().all(|r| r.0 == HttpMethod::Post && r.3));
    assert_eq!(token[0].4.as_deref(), Some("grant_type=client_credentials"));
    assert!(
        reqs.iter()
            .filter(|r| r.1.contains("/reporting/"))
            .all(|r| r.2.iter().any(|h| h == "Authorization") && !r.3)
    );
    let stored = p.secrets.all();
    assert_eq!(stored.len(), 1);
    let v = stored.values().next().unwrap();
    assert!(v.contains("EfGhClientSecret") && !v.contains("A21AA"));
    let audit = serde_json::to_string(&p.b.s.audit_entries(50).unwrap()).unwrap();
    assert!(!audit.contains("A21AA") && !audit.contains("EfGhClientSecret"));
    let shown =
        p.b.run("connect.get", json!({"connection": c["id"]}))
            .to_string();
    assert!(!shown.contains("A21AA") && !shown.contains("EfGh"));
}

#[test]
fn at9_rate_limit_and_rejected_key() {
    let mut p = proc_book();
    stripe_payout(&p.fake, "Order");
    let c = add_stripe(&mut p);
    p.fake.with(|s| s.status = Some(429));
    let out = sync(&mut p);
    let r = &out["connections"][0];
    assert_eq!(r["status"], "active");
    assert_eq!(r["error"]["code"], "rate_limited");
    let g = p.b.run("connect.get", json!({"connection": c["id"]}));
    assert_eq!(g["next_retry_at"], NOW + 3600);
    p.fake.with(|s| s.status = Some(401));
    let out = sync(&mut p);
    assert_eq!(out["connections"][0]["status"], "needs_attention");
    assert!(
        out["connections"][0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("reconnect with a new key")
    );
}

#[test]
fn at10_foreign_payout_listed_not_posted() {
    let mut p = proc_book();
    p.fake.with(|s| {
        let mut po = payout_obj("po_9", 5_000, "paid", OCT3);
        po["currency"] = json!("eur");
        s.stripe_payouts = vec![po];
        s.stripe_entries.insert(
            "po_9".into(),
            vec![{
                let mut c = bt("txn_e1", "charge", "charge", 5_000, 0, "Euro sale", OCT3);
                c["currency"] = json!("eur");
                c
            }],
        );
    });
    add_stripe(&mut p);
    let out = sync(&mut p);
    let r = &out["connections"][0];
    assert!(r["batch"].is_null(), "{r}");
    assert_eq!(r["listed"][0]["reason"], "multi-currency needed");
    assert!(r["warnings"].to_string().contains("multi-currency needed"));
}

#[test]
fn at11_cleaned_descriptions_and_oversize_refused() {
    let mut p = proc_book();
    stripe_payout(&p.fake, "Order\u{7} \u{202e}1003");
    let c = add_stripe(&mut p);
    p.b.run(
        "processor.set",
        json!({"connection": c["id"], "mode": "detailed"}),
    );
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    let text = batch.to_string();
    assert!(!text.contains('\u{202e}') && !text.contains('\u{7}'));
    assert!(
        text.contains("Order 1003") || text.contains("Order1003"),
        "{text}"
    );
    p.fake.with(|s| s.pad_to = 25 * 1024 * 1024);
    let out = sync(&mut p);
    let r = &out["connections"][0];
    assert!(
        r["error"]["message"].as_str().unwrap().contains("20 MB"),
        "{r}"
    );
}

// ----- revision 2 (verified vendor details): ATs 13–18 -------------------------------

fn row_other<'a>(row: &'a Value, account: &str) -> Option<&'a Value> {
    row["other"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["account_name"] == account)
}

#[test]
fn at13_stripe_payout_on_the_way_then_paid() {
    let mut p = proc_book();
    stripe_payout(&p.fake, "Order 1003");
    p.fake
        .with(|s| s.stripe_payouts[0]["status"] = json!("in_transit"));
    add_stripe(&mut p);
    // The connection test lists one payout.
    let verify = p.fake.requests()[0].1.clone();
    assert_eq!(verify, "https://api.stripe.com/v1/payouts?limit=1");
    let out = sync(&mut p);
    let r = &out["connections"][0];
    assert!(r["batch"].is_null(), "{r}");
    assert_eq!(r["listed"][0]["reason"], "on the way");
    assert!(
        r["warnings"]
            .to_string()
            .contains("payout po_1 of 945.30 is on the way (in_transit)"),
        "{r}"
    );
    // Payouts come from the Payouts endpoint, with created filters.
    assert!(p.fake.requests().iter().any(|q| {
        q.1.starts_with("https://api.stripe.com/v1/payouts?created%5Bgte%5D=")
    }));
    p.fake
        .with(|s| s.stripe_payouts[0]["status"] = json!("paid"));
    set_now(&mut p, NOW + 2 * 3600);
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    assert_eq!(batch["rows"].as_array().unwrap().len(), 1);
    assert_eq!(batch["rows"][0]["amount"], "945.30");
}

#[test]
fn at14_failed_payout_after_posting_is_reversed() {
    let mut p = proc_book();
    stripe_payout(&p.fake, "Order 1003");
    add_stripe(&mut p);
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    p.b.commit(&batch);
    let bank = p.b.stage_qif(
        "!Type:Bank\nD10/05/2026\nT945.30\nPSTRIPE TRANSFER\n^\n",
        "Assets:Checking",
    );
    p.b.commit(&bank);
    assert_eq!(p.b.shown("Stripe Clearing", "USD"), "0.00");
    p.fake.with(|s| {
        s.stripe_payouts[0]["status"] = json!("failed");
        s.stripe_payouts[0]["failure_code"] = json!("account_closed");
    });
    set_now(&mut p, NOW + 3 * DAY);
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    let rows = batch["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{batch}");
    let row = &rows[0];
    assert_eq!(row["amount"], "945.30");
    assert_eq!(row["needs_review"], true);
    assert!(
        row["warnings"].to_string().contains("account_closed"),
        "{row}"
    );
    assert!(
        row_other(row, "Assets:Checking").is_some() || row_other(row, "Checking").is_some(),
        "{row}"
    );
    p.b.commit(&batch);
    assert_eq!(p.b.shown("Stripe Clearing", "USD"), "945.30");
    assert_eq!(p.b.shown("Assets:Checking", "USD"), "0.00");
    // Seen again: nothing new.
    set_now(&mut p, NOW + 4 * DAY);
    let out = sync(&mut p);
    assert_eq!(out["connections"][0]["new"], 0, "{out}");
    // A payout that fails before it is posted is listed, not staged.
    let mut q = proc_book();
    stripe_payout(&q.fake, "Order 1003");
    q.fake.with(|s| {
        s.stripe_payouts[0]["status"] = json!("canceled");
    });
    add_stripe(&mut q);
    let out = sync(&mut q);
    let r = &out["connections"][0];
    assert!(r["batch"].is_null(), "{r}");
    assert_eq!(r["listed"][0]["reason"], "failed");
}

fn square_book(p: &mut Proc, status: &str) -> Value {
    p.fake.with(|s| {
        s.square_locations = vec![json!({"id": "L1", "name": "Main Street", "currency": "USD"})];
        s.square_payouts.insert(
            "L1".into(),
            vec![json!({"id": "po_a", "status": status, "location_id": "L1",
                "created_at": "2026-10-02T10:00:00Z", "arrival_date": "2026-10-03",
                "amount_money": {"amount": 14_535, "currency_code": "USD"}, "type": "BATCH"})],
        );
        s.square_entries.insert(
            "po_a".into(),
            vec![square_entry(
                "e1",
                "CHARGE",
                15_000,
                14_535,
                "2026-10-01T03:07:09Z",
            )],
        );
    });
    p.b.run(
        "connect.add",
        json!({"provider": "square", "setup_input": "EAAAEtestsquaretoken1234", "label": "Till", "confirm": true}),
    )
}

#[test]
fn at15_square_version_header_and_permission_message() {
    let mut p = proc_book();
    square_book(&mut p, "PAID");
    let out = sync(&mut p);
    assert!(out["connections"][0]["batch"].is_string(), "{out}");
    let reqs = p.fake.requests();
    assert!(reqs.len() >= 3);
    for (_, url, headers, _, _) in &reqs {
        assert!(
            headers.iter().any(|h| h == "Square-Version"),
            "{url} without Square-Version"
        );
    }
    assert!(
        reqs.iter()
            .any(|r| r.1.contains("/v2/payouts?location_id=L1") && r.1.contains("limit=100"))
    );
    // The fake server refuses a request without the version (400), so a good sync
    // proves every request named 2026-09-16.
    assert_eq!(crate::processors::SQUARE_VERSION, "2026-09-16");
    p.fake
        .with(|s| s.status_for = vec![("/v2/payouts".into(), 403)]);
    set_now(&mut p, NOW + 2 * 3600);
    let out = sync(&mut p);
    let r = &out["connections"][0];
    assert_eq!(r["status"], "needs_attention");
    let msg = r["error"]["message"].as_str().unwrap();
    assert!(msg.contains("PAYOUTS_READ"), "{msg}");
    // A token refused at connect names the permissions too.
    let mut q = proc_book();
    q.fake
        .with(|s| s.status_for = vec![("/v2/locations".into(), 401)]);
    let e = q.b.err(
        "connect.add",
        json!({"provider": "square", "setup_input": "EAAAEtestsquaretoken1234", "confirm": true}),
    );
    assert!(e.to_string().contains("MERCHANT_PROFILE_READ"), "{e}");
}

#[test]
fn r2_square_sent_payout_waits_and_entry_amounts_checked() {
    let mut p = proc_book();
    square_book(&mut p, "SENT");
    let out = sync(&mut p);
    let r = &out["connections"][0];
    assert!(r["batch"].is_null(), "{r}");
    assert_eq!(r["listed"][0]["reason"], "on the way");
    // Paid, with an entry whose gross − fee is not its net: the payout needs review.
    p.fake.with(|s| {
        s.square_payouts.get_mut("L1").unwrap()[0]["status"] = json!("PAID");
        let e = &mut s.square_entries.get_mut("po_a").unwrap()[0];
        e["fee_amount_money"]["amount"] = json!(400);
    });
    set_now(&mut p, NOW + 2 * 3600);
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    let row = &batch["rows"][0];
    assert_eq!(row["needs_review"], true);
    assert!(row["warnings"].to_string().contains("not the net"), "{row}");
}

#[test]
fn at16_paypal_event_codes() {
    let mut p = proc_book();
    p.fake.with(|s| {
        s.paypal = vec![
            paypal_txn(
                "S1",
                "T0006",
                "2026-09-20T10:00:00Z",
                "100.00",
                Some("-3.20"),
                "Sale 1",
            ),
            paypal_txn(
                "F1",
                "T1108",
                "2026-09-21T10:00:00Z",
                "2.10",
                None,
                "Fee reversal",
            ),
            paypal_txn(
                "D1",
                "T1201",
                "2026-09-22T10:00:00Z",
                "-50.00",
                None,
                "Chargeback",
            ),
            paypal_txn(
                "X1",
                "T0402",
                "2026-09-23T10:00:00Z",
                "-20.00",
                None,
                "Transfer",
            ),
            paypal_txn(
                "D2",
                "T1202",
                "2026-09-24T10:00:00Z",
                "50.00",
                None,
                "Chargeback reversal",
            ),
            paypal_txn(
                "W1",
                "T0400",
                "2026-09-25T10:00:00Z",
                "-198.90",
                None,
                "Withdrawal",
            ),
        ];
    });
    add_paypal(&mut p);
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    let rows = batch["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{batch}");
    let payout = rows.iter().find(|r| r["amount"] == "198.90").unwrap();
    assert_eq!(payout["date"], "2026-09-25", "the group ends at the T0400");
    let raw: Value = serde_json::from_str(payout["raw"]["raw"].as_str().unwrap()).unwrap();
    let entries = raw["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 4, "{raw}");
    let cat = |id: &str| {
        entries.iter().find(|e| e["id"] == id).unwrap()["category"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(cat("F1"), "fee");
    assert_eq!(cat("D2"), "dispute_reversal");
    assert_eq!(raw["fees"], "-1.10", "the T1108 reduces the fees");
    assert_eq!(
        row_other(payout, "Merchant Fees").unwrap()["value"],
        "1.10",
        "{payout}"
    );
    // The T0402 is staged on its own, for review.
    let alone = rows.iter().find(|r| r["amount"] == "-20.00").unwrap();
    assert_eq!(alone["needs_review"], true);
    assert!(
        alone["warnings"]
            .to_string()
            .contains("not a withdrawal to the bank"),
        "{alone}"
    );
}

#[test]
fn at17_stripe_fee_outside_payouts() {
    let mut p = proc_book();
    stripe_payout(&p.fake, "Order 1003");
    p.fake.with(|s| {
        s.stripe_fees = vec![
            bt(
                "txn_f1",
                "stripe_fee",
                "fee",
                -60,
                0,
                "Radar",
                OCT3 - 2 * DAY,
            ),
            bt("txn_lf", "stripe_fee", "fee", -500, 0, "Billing fee", OCT3),
        ];
    });
    add_stripe(&mut p);
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    let rows = batch["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{batch}");
    let fee = rows.iter().find(|r| r["amount"] == "-5.00").unwrap();
    assert_eq!(fee["payee"], "Billing fee");
    assert!(row_other(fee, "Merchant Fees").is_some(), "{fee}");
    assert_eq!(batch["sections"][0]["account_name"], "Stripe");
    p.b.commit(&batch);
    assert_eq!(p.b.shown("Stripe Clearing", "USD"), "940.30");
    // A later payout holding that fee does not post it again.
    p.fake.with(|s| {
        s.stripe_payouts
            .push(payout_obj("po_2", 9_500, "paid", NOW + 2 * DAY));
        s.stripe_entries.insert(
            "po_2".into(),
            vec![
                bt("txn_c9", "charge", "charge", 10_000, 0, "Order 2001", NOW),
                bt("txn_lf", "stripe_fee", "fee", -500, 0, "Billing fee", OCT3),
            ],
        );
    });
    set_now(&mut p, NOW + 3 * DAY);
    let out = sync(&mut p);
    let batch = batch_of(&mut p, &out);
    let row = batch["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["memo"] == "po_2")
        .unwrap()
        .clone();
    assert_eq!(
        row["raw"]["amount"], "100.00",
        "only what was not posted yet"
    );
    p.b.commit(&batch);
    assert_eq!(p.b.shown("Merchant Fees", "USD"), "35.50", "{row}");
    assert_eq!(p.b.shown("Stripe Clearing", "USD"), "1040.30", "{row}");
    // Both bank deposits still match their payouts.
    let bank = p.b.stage_qif(
        "!Type:Bank\nD10/05/2026\nT945.30\nPSTRIPE\n^\nD10/07/2026\nT95.00\nPSTRIPE\n^\n",
        "Assets:Checking",
    );
    for r in bank["rows"].as_array().unwrap() {
        assert_eq!(r["other"][0]["account_name"], "Stripe Clearing", "{r}");
    }
    p.b.commit(&bank);
    assert_eq!(p.b.shown("Stripe Clearing", "USD"), "0.00");
}

#[test]
fn at18_held_at_paypal_note() {
    let mut p = proc_book();
    add_paypal(&mut p);
    p.b.transfer("2026-09-01", "PayPal Clearing", "Assets:Checking", "412.00");
    let note = |p: &mut Proc, today: &str| {
        let list = p.b.run("account.list", json!({"today": today}));
        list.as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == "PayPal Clearing")
            .unwrap()["note"]
            .clone()
    };
    assert_eq!(note(&mut p, "2026-10-04"), "held at PayPal");
    assert!(
        p.b.run("account.list", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["name"] != "PayPal Clearing")
            .all(|a| a["note"].is_null())
    );
    // A withdrawal in the last 30 days: no note.
    p.fake.with(|s| {
        s.paypal = vec![paypal_txn(
            "W1",
            "T0400",
            "2026-09-25T10:00:00Z",
            "-10.00",
            None,
            "Withdrawal",
        )];
    });
    sync(&mut p);
    assert!(note(&mut p, "2026-10-04").is_null());
    assert_eq!(note(&mut p, "2026-10-26"), "held at PayPal");
}
