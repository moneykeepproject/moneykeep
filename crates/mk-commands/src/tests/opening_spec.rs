//! opening-balances.md acceptance tests OB1–OB10 and IB1–IB6. Bank feeds use the
//! registry's scripted SimpleFIN server; nothing touches the network.

use super::*;
use crate::connect::{Connector, FakeSimplefin, MemorySecrets};
use mk_core::Date;

/// 2026-10-01 00:00 UTC.
const OCT1: i64 = 1_790_812_800;

fn epoch(date: &str) -> i64 {
    let d: Date = date.parse().unwrap();
    let base: Date = "2026-10-01".parse().unwrap();
    OCT1 + base.days_until(d) * 86_400
}

/// A SimpleFIN answer with one account: (id, posted date, amount, description,
/// pending).
fn body(account: &str, balance: &str, on: &str, txns: &[(&str, &str, &str, &str, bool)]) -> String {
    let rows: Vec<Value> = txns
        .iter()
        .map(|(id, date, amount, desc, pending)| {
            let mut v = json!({"id": id, "posted": if *pending { 0 } else { epoch(date) },
                "amount": amount, "description": desc});
            if *pending {
                v["pending"] = json!(true);
                v["transacted_at"] = json!(epoch(date));
            }
            v
        })
        .collect();
    json!({
        "errlist": [],
        "connections": [{"conn_id": "c1", "name": "First Bank", "org_id": "o1"}],
        "accounts": [{"id": account, "name": "Everyday", "conn_id": "c1", "currency": "USD",
            "balance": balance, "balance-date": epoch(on), "transactions": rows}],
    })
    .to_string()
}

struct Feed {
    b: Book,
    fake: FakeSimplefin,
    conn: String,
}

/// A book with Checking (and Visa) and a SimpleFIN connection linked to `account`.
fn feed(body: &str, now: &str, account: &str, extra: Value) -> Feed {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "bank-feeds"}));
    let fake = FakeSimplefin::new(body);
    b.r.set_connector(Connector {
        fetcher: std::sync::Arc::new(fake.clone()),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(epoch(now)),
        host: None,
    });
    let c = b.run(
        "connect.add",
        json!({"setup_input": FakeSimplefin::token("ob"), "label": "My bank", "confirm": true}),
    );
    let conn = c["id"].as_str().unwrap().to_owned();
    let mut a = json!({"connection": conn, "remote_account": "a1", "account": account,
        "sync_from": "2026-08-01"});
    for (k, v) in extra.as_object().unwrap() {
        a[k] = v.clone();
    }
    b.run("connect.link_account", a);
    Feed { b, fake, conn }
}

fn sync_commit(f: &mut Feed) -> Value {
    let out = f.b.run("connect.sync", json!({"now": true}));
    let id = out["connections"][0]["batch"].clone();
    if !id.is_null() {
        let batch = f.b.run("import.get", json!({"batch": id}));
        f.b.commit(&batch);
    }
    out
}

fn opening_txns(b: &mut Book, account: &str) -> Vec<Value> {
    b.run("txn.list", json!({"account": account}))
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["opening"] == true)
        .cloned()
        .collect()
}

fn balance_on(b: &mut Book, account: &str, date: &str) -> String {
    let v = b.run(
        "account.balance",
        json!({"account": account, "as_of": date}),
    );
    v["display"]["USD"].as_str().unwrap_or("absent").to_owned()
}

const OB1_ROWS: [(&str, &str, &str, &str, bool); 3] = [
    ("t1", "2026-08-04", "2000.00", "Payroll", false),
    ("t2", "2026-09-10", "-1035.35", "Rent", false),
    ("t3", "2026-11-05", "-500.00", "Savings transfer", false),
];

#[test]
fn ob1_feed_checking_keep_everything() {
    let mut f = feed(
        &body("a1", "2341.10", "2026-11-06", &OB1_ROWS),
        "2026-11-06",
        "Assets:Checking",
        json!({}),
    );
    sync_commit(&mut f);
    let p = f.b.run(
        "opening.plan",
        json!({"account": "Assets:Checking", "today": "2026-11-06"}),
    );
    assert_eq!(p["feed"]["balance"], "2341.10", "{p}");
    assert_eq!(p["feed"]["date"], "2026-11-06");
    assert_eq!(p["source"], "feed");
    assert_eq!(p["start"]["mode"], "keep");
    assert_eq!(p["start"]["imported"], 3);
    assert_eq!(p["opening"]["amount"], "1876.45");
    assert_eq!(p["opening"]["date"], "2026-08-03");
    assert_eq!(p["offset"]["name"], "Opening Balances");
    assert_eq!(p["check"]["matches"], true);
    assert_eq!(p["check"]["balance"], "2341.10");
    assert_eq!(p["causes"], json!([]));
    let out = f.b.run(
        "opening.apply",
        json!({"account": "Assets:Checking", "today": "2026-11-06"}),
    );
    assert_eq!(out["amount"], "1876.45");
    let t = &out["transaction"];
    assert_eq!(t["opening"], true);
    assert_eq!(t["payee"], "Opening balance");
    assert_eq!(t["date"], "2026-08-03");
    assert!(
        t["splits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["account_name"] == "Opening Balances" && s["value"] == "-1876.45"),
        "{t}"
    );
    assert_eq!(
        balance_on(&mut f.b, "Assets:Checking", "2026-11-06"),
        "2341.10"
    );
    // The Check now shows the match with the entry in place.
    let p = f.b.run(
        "opening.plan",
        json!({"account": "Assets:Checking", "today": "2026-11-06"}),
    );
    assert_eq!(p["existing"]["amount"], "1876.45");
    assert_eq!(p["check"]["matches"], true);
}

#[test]
fn ob2_statement_start_removes_and_remembers() {
    let mut rows: Vec<(String, String, String)> = (1..=12)
        .map(|i| {
            (
                format!("p{i}"),
                format!("2026-09-{:02}", i * 2),
                format!("-{i}.00"),
            )
        })
        .collect();
    rows.push(("q1".into(), "2026-10-05".into(), "-40.00".into()));
    rows.push(("q2".into(), "2026-10-20".into(), "300.00".into()));
    let rows: Vec<(&str, &str, &str, &str, bool)> = rows
        .iter()
        .map(|(i, d, a)| (i.as_str(), d.as_str(), a.as_str(), "Shop", false))
        .collect();
    let text = body("a1", "2260.00", "2026-11-06", &rows);
    let mut f = feed(&text, "2026-11-06", "Assets:Checking", json!({}));
    sync_commit(&mut f);
    assert_eq!(f.b.txn_count(), 14);
    let args = json!({"account": "Assets:Checking", "start": "statement",
        "statement_date": "2026-09-30", "statement_balance": "2000.00"});
    let p = f.b.run("opening.plan", args.clone());
    assert_eq!(p["pre_start"].as_array().unwrap().len(), 12, "{p}");
    assert_eq!(p["removed"], 12);
    assert_eq!(p["opening"]["amount"], "2000.00");
    assert_eq!(p["opening"]["date"], "2026-09-30");
    assert_eq!(p["check"]["matches"], true);
    let out = f.b.run("opening.apply", args);
    assert_eq!(out["removed"], 12);
    assert_eq!(
        f.b.txn_count(),
        3,
        "two after the start plus the opening entry"
    );
    assert_eq!(
        balance_on(&mut f.b, "Assets:Checking", "2026-09-30"),
        "2000.00"
    );
    // A later sync does not bring the removed transactions back.
    f.fake.set_accounts(&text);
    if let Some(c) = f.b.r.connector_mut() {
        c.now = Some(epoch("2026-11-07"));
    }
    let out = f.b.run("connect.sync", json!({"now": true}));
    let r = &out["connections"][0];
    assert_eq!(r["counts"]["new"].as_u64().unwrap_or(0), 0, "{r}");
    if !r["batch"].is_null() {
        let batch = f.b.run("import.get", json!({"batch": r["batch"]}));
        f.b.commit(&batch);
    }
    assert_eq!(f.b.txn_count(), 3);
    let _ = &f.conn;
}

#[test]
fn ob3_credit_card_owed_and_in_credit() {
    let mut b = Book::new();
    b.standard();
    let out = b.run(
        "opening.apply",
        json!({"account": "Liabilities:Visa", "balance": "512.30", "as_of": "2026-11-06"}),
    );
    assert_eq!(out["date"], "2026-11-06");
    assert_eq!(b.shown("Liabilities:Visa", "USD"), "512.30");
    let card = out["transaction"]["splits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["account_name"] == "Liabilities:Visa")
        .unwrap()
        .clone();
    assert_eq!(card["value"], "-512.30", "the entry credits the card");
    let p = b.run("opening.plan", json!({"account": "Liabilities:Visa"}));
    assert_eq!(p["account"]["question"], "How much do you owe?");
    b.run(
        "opening.apply",
        json!({"account": "Liabilities:Visa", "balance": "20.00", "owes_me": true,
            "as_of": "2026-11-06"}),
    );
    assert_eq!(
        b.shown("Liabilities:Visa", "USD"),
        "-20.00",
        "a 20.00 credit"
    );
    assert_eq!(opening_txns(&mut b, "Liabilities:Visa").len(), 1);
}

#[test]
fn ob4_rerun_changes_the_one_entry_and_undo() {
    let mut b = Book::new();
    b.standard();
    b.transfer(
        "2026-09-10",
        "Expenses:Groceries",
        "Assets:Checking",
        "50.00",
    );
    b.run(
        "opening.apply",
        json!({"account": "Assets:Checking", "balance": "1000.00", "as_of": "2026-11-06"}),
    );
    let first = opening_txns(&mut b, "Assets:Checking");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0]["date"], "2026-09-09");
    let p = b.run(
        "opening.plan",
        json!({"account": "Assets:Checking", "balance": "1200.00", "as_of": "2026-11-06"}),
    );
    assert!(p["existing"]["txn"].is_string());
    assert_eq!(p["existing"]["amount"], "1050.00");
    b.run(
        "opening.apply",
        json!({"account": "Assets:Checking", "balance": "1200.00", "as_of": "2026-11-06"}),
    );
    let second = opening_txns(&mut b, "Assets:Checking");
    assert_eq!(second.len(), 1, "still exactly one");
    assert_eq!(second[0]["id"], first[0]["id"]);
    assert_eq!(b.shown("Assets:Checking", "USD"), "1200.00");
    b.run("history.undo", json!({}));
    let back = opening_txns(&mut b, "Assets:Checking");
    assert_eq!(back.len(), 1);
    assert_eq!(b.shown("Assets:Checking", "USD"), "1000.00");
    // An edit in the register keeps the mark.
    let id = back[0]["id"].clone();
    let t = b.run("txn.get", json!({"id": id}));
    let splits: Vec<Value> = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| json!({"account": s["account"], "value": s["value"]}))
        .collect();
    b.run(
        "txn.update",
        json!({"id": id, "date": "2026-09-01", "payee": "Opening balance", "splits": splits}),
    );
    assert_eq!(opening_txns(&mut b, "Assets:Checking").len(), 1);
    // A legacy opening entry (before this spec) is recognised and changed in place.
    let mut b = Book::new();
    b.standard();
    let legacy = b.transfer(
        "2026-08-01",
        "Assets:Checking",
        "Opening Balances",
        "300.00",
    );
    b.transfer(
        "2026-08-05",
        "Expenses:Groceries",
        "Assets:Checking",
        "20.00",
    );
    let p = b.run(
        "opening.plan",
        json!({"account": "Assets:Checking", "balance": "500.00", "as_of": "2026-11-06"}),
    );
    assert_eq!(p["existing"]["legacy"], true);
    assert_eq!(p["existing"]["txn"], json!(legacy));
    b.run(
        "opening.apply",
        json!({"account": "Assets:Checking", "balance": "500.00", "as_of": "2026-11-06"}),
    );
    let o = opening_txns(&mut b, "Assets:Checking");
    assert_eq!(o.len(), 1);
    assert_eq!(o[0]["id"], json!(legacy));
    assert_eq!(b.shown("Assets:Checking", "USD"), "500.00");
}

#[test]
fn ob5_pending_follows_the_account_setting() {
    let rows = [
        ("t1", "2026-10-02", "-19.99", "Stream Co", false),
        ("t4", "2026-11-05", "-5.00", "Coffee", true),
    ];
    let mut f = feed(
        &body("a1", "-300.00", "2026-11-06", &rows),
        "2026-11-06",
        "Liabilities:Visa",
        json!({"include_pending": true}),
    );
    sync_commit(&mut f);
    assert_eq!(f.b.txn_count(), 2, "the pending coffee was imported");
    let p =
        f.b.run("opening.plan", json!({"account": "Liabilities:Visa"}));
    assert_eq!(
        p["feed"]["pending_text"],
        "Pending transactions are included, as set for this account."
    );
    // Owed 300.00 today; 24.99 of it came after the start: 275.01 owed at the start.
    assert_eq!(p["opening"]["amount"], "275.01", "{p}");
    f.b.run(
        "connect.link_account",
        json!({"connection": f.conn, "remote_account": "a1", "account": "Liabilities:Visa",
            "include_pending": false}),
    );
    let p =
        f.b.run("opening.plan", json!({"account": "Liabilities:Visa"}));
    assert_eq!(
        p["feed"]["pending_text"],
        "Pending transactions are not included, as set for this account."
    );
    assert_eq!(
        p["opening"]["amount"], "280.01",
        "the pending coffee is not counted"
    );
    // It is still in the register, so the check differs and names the setting.
    assert_eq!(p["check"]["matches"], false);
    assert!(
        p["causes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["kind"] == "pending" && c["included"] == false)
    );
}

#[test]
fn ob6_business_book_owners_equity() {
    let mut b = Book::new();
    b.standard();
    let p = b.run(
        "opening.plan",
        json!({"account": "Assets:Checking", "balance": "100.00", "as_of": "2026-11-06"}),
    );
    assert_eq!(p["offset"]["choices"].as_array().unwrap().len(), 1);
    b.run("module.enable", json!({"module": "invoicing"}));
    let p = b.run(
        "opening.plan",
        json!({"account": "Assets:Checking", "balance": "100.00", "as_of": "2026-11-06"}),
    );
    let choices = p["offset"]["choices"].as_array().unwrap();
    assert_eq!(choices[0]["name"], "Opening Balances", "preselected");
    assert_eq!(p["offset"]["name"], "Opening Balances");
    assert_eq!(choices[1]["key"], "owners-equity");
    assert_eq!(choices[1]["create"], true);
    let out = b.run(
        "opening.apply",
        json!({"account": "Assets:Checking", "balance": "100.00", "as_of": "2026-11-06",
            "offset": "owners-equity"}),
    );
    assert!(
        out["transaction"]["splits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["account_name"] == "Owner's Equity" && s["value"] == "-100.00"),
        "{out}"
    );
    let eq = b.run("account.get", json!({"account": "Owner's Equity"}));
    assert_eq!(eq["type"], "equity");
    assert_eq!(eq["subtype"], "owner-equity");
    assert!(eq["parent"].is_null(), "top level");
}

#[test]
fn ob7_loan_for_something_you_own() {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Mortgage", "type": "liability", "subtype": "loan", "parent": "Liabilities"}));
    let p = b.run(
        "opening.plan",
        json!({"account": "Liabilities:Mortgage", "balance": "240000.00", "as_of": "2026-11-01"}),
    );
    assert_eq!(p["loan"], true);
    let out = b.run(
        "opening.apply",
        json!({"account": "Liabilities:Mortgage", "balance": "240000.00", "as_of": "2026-11-01",
            "asset": {"name": "House", "value": "300000"}}),
    );
    let house = out["asset"]["name"].as_str().unwrap().to_owned();
    assert_eq!(house, "Assets:House");
    assert_eq!(b.shown("Assets:House", "USD"), "300000.00");
    assert_eq!(b.shown("Liabilities:Mortgage", "USD"), "240000.00");
    assert_eq!(opening_txns(&mut b, "Assets:House").len(), 1);
    assert_eq!(opening_txns(&mut b, "Liabilities:Mortgage").len(), 1);
    // Net worth changes by 300,000 − 240,000.
    assert_eq!(b.shown("Opening Balances", "USD"), "60000.00");
}

/// Checking with two reconciled transactions (Sep 30) and later unreconciled ones.
fn reconciled_book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.transfer(
        "2026-09-05",
        "Assets:Checking",
        "Expenses:Groceries",
        "100.00",
    );
    b.transfer(
        "2026-09-10",
        "Assets:Checking",
        "Expenses:Groceries",
        "200.00",
    );
    b.run(
        "reconcile.start",
        json!({"account": "Assets:Checking", "statement_date": "2026-09-30", "statement_balance": "300.00"}),
    );
    let s = b.run("reconcile.get", json!({"account": "Assets:Checking"}));
    let all: Vec<Value> = s["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["split"].clone())
        .collect();
    b.run(
        "reconcile.select",
        json!({"account": "Assets:Checking", "splits": all}),
    );
    b.run("reconcile.finish", json!({"account": "Assets:Checking"}));
    // Entered after the reconciliation: one dated inside its period, one after it.
    b.transfer(
        "2026-09-20",
        "Assets:Checking",
        "Expenses:Groceries",
        "25.00",
    );
    b.transfer(
        "2026-10-05",
        "Assets:Checking",
        "Expenses:Groceries",
        "50.00",
    );
    b
}

#[test]
fn ob8_reconciled_period_note_go_ahead() {
    let mut b = reconciled_book();
    let args = json!({"account": "Assets:Checking", "start": "statement",
        "statement_date": "2026-10-31", "statement_balance": "375.00"});
    let p = b.run("opening.plan", args.clone());
    let notes = p["reconciled"].as_array().unwrap();
    assert_eq!(notes.len(), 1, "{p}");
    assert_eq!(notes[0]["date"], "2026-09-30");
    assert_eq!(notes[0]["ending"], "300.00");
    // The reconciled transactions are not offered for removal.
    assert_eq!(p["pre_start"].as_array().unwrap().len(), 2);
    let e = b.err("opening.apply", args.clone());
    assert_eq!(e.code(), "reconciled_period");
    assert!(e.to_string().contains("ended at 300.00"), "{e}");
    let mut go = args;
    go["go_ahead"] = json!(true);
    let out = b.run("opening.apply", go);
    assert_eq!(out["reconciliations_to_check"], 1);
    let h = b.run("reconcile.history", json!({"account": "Assets:Checking"}));
    assert_eq!(h[0]["to_check"], true, "{h}");
    // Keeping everything (the opening entry before all transactions) has no note.
    let mut b = reconciled_book();
    let p = b.run(
        "opening.plan",
        json!({"account": "Assets:Checking", "balance": "375.00", "as_of": "2026-11-06"}),
    );
    assert_eq!(p["opening"]["amount"], "0.00");
    assert_eq!(p["reconciled"], json!([]), "{p}");
}

#[test]
fn ob9_unticking_shows_the_difference_and_helper() {
    let mut b = Book::new();
    b.standard();
    let keep = b.transfer(
        "2026-09-20",
        "Assets:Checking",
        "Expenses:Groceries",
        "25.00",
    );
    b.transfer(
        "2026-09-25",
        "Expenses:Groceries",
        "Assets:Checking",
        "10.00",
    );
    b.transfer(
        "2026-10-05",
        "Assets:Checking",
        "Expenses:Groceries",
        "50.00",
    );
    let mut args = json!({"account": "Assets:Checking", "start": "statement",
        "statement_date": "2026-09-30", "statement_balance": "400.00"});
    let p = b.run("opening.plan", args.clone());
    assert_eq!(p["check"]["matches"], true);
    args["keep"] = json!([keep]);
    let p = b.run("opening.plan", args);
    assert_eq!(p["removed"], 1);
    assert_eq!(p["check"]["matches"], false);
    assert_eq!(p["check"]["difference"], "-25.00", "{p}");
    let kinds: Vec<&str> = p["causes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"kept"), "{kinds:?}");
    assert!(kinds.contains(&"as_of"), "{kinds:?}");
}

#[test]
fn ob10_checklist_progress_and_attention() {
    let mut b = Book::new();
    b.standard();
    for (name, ty, sub, parent) in [
        ("Savings", "asset", "bank", "Assets"),
        ("Wallet", "asset", "cash", "Assets"),
        ("Car loan", "liability", "loan", "Liabilities"),
        ("Store card", "liability", "credit-card", "Liabilities"),
        ("Owed by Sam", "asset", "other-asset", "Assets"),
    ] {
        b.account(json!({"name": name, "type": ty, "subtype": sub, "parent": parent}));
    }
    let all = [
        "Assets:Checking",
        "Liabilities:Visa",
        "Assets:Savings",
        "Assets:Wallet",
        "Liabilities:Car loan",
        "Liabilities:Store card",
        "Assets:Owed by Sam",
    ];
    for a in all {
        b.transfer("2026-10-10", "Expenses:Groceries", a, "5.00");
    }
    for a in &all[..3] {
        b.run(
            "opening.apply",
            json!({"account": a, "balance": "100.00", "as_of": "2026-11-06"}),
        );
    }
    let c = b.run("opening.checklist", json!({}));
    assert_eq!(c["progress"], "3 of 7 accounts set up", "{c}");
    assert_eq!(c["need_opening"], 4);
    assert!(c["encouragement"].is_null());
    let row = |c: &Value, n: &str| -> Value {
        c["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["full_name"] == n)
            .unwrap()
            .clone()
    };
    assert_eq!(row(&c, "Assets:Checking")["status"], "done");
    assert_eq!(row(&c, "Assets:Checking")["amount"], "105.00");
    assert_eq!(row(&c, "Assets:Wallet")["status"], "not_set");
    let att = b.run(
        "dashboard.block_data",
        json!({"dashboard": "home", "block": "attention", "today": "2026-11-06"}),
    );
    let item = att["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["label"] == "4 accounts need an opening balance")
        .unwrap()
        .clone();
    assert_eq!(item["target"]["screen"], "opening");
    b.run(
        "opening.started_from_zero",
        json!({"account": "Assets:Owed by Sam"}),
    );
    let c = b.run("opening.checklist", json!({}));
    assert_eq!(c["progress"], "3 of 6 accounts set up");
    assert_eq!(c["encouragement"], "Halfway there.");
    assert_eq!(row(&c, "Assets:Owed by Sam")["status"], "zero");
    b.run("history.undo", json!({}));
    let c = b.run("opening.checklist", json!({}));
    assert_eq!(c["progress"], "3 of 7 accounts set up", "undoable");
    for a in &all[3..] {
        b.run(
            "opening.apply",
            json!({"account": a, "balance": "1.00", "as_of": "2026-11-06"}),
        );
    }
    let c = b.run("opening.checklist", json!({}));
    assert_eq!(
        c["encouragement"],
        "All accounts are set up. Your net worth now reflects everything you own and owe."
    );
    // Limited to some accounts (after linking a feed).
    let c = b.run(
        "opening.checklist",
        json!({"accounts": ["Assets:Checking"]}),
    );
    assert_eq!(c["accounts"].as_array().unwrap().len(), 1);
}

// ----- investments -------------------------------------------------------------------------

fn inv_book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "investments"}));
    b.account(
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    for (sym, name, kind) in [
        ("VTI", "Total Stock Market ETF", "etf"),
        ("FXAIX", "500 Index Fund", "mutual_fund"),
    ] {
        b.run(
            "security.create",
            json!({"symbol": sym, "name": name, "kind": kind}),
        );
    }
    b
}

/// A SnapTrade-style link with a holdings check, written as the sync writes it.
fn snaptrade_holdings(b: &mut Book, positions: Value, cash: &str) {
    let acc = b.run("account.get", json!({"account": "Assets:Brokerage"}))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let link = json!({
        "id": "snap|r1", "connection": "snap", "remote_account_id": "r1", "name": "Brokerage",
        "conn_id": "", "account": acc, "account_key": "snap:r1", "active": true,
        "remote": {"holdings": {"as_of": "2026-11-06", "positions": positions,
            "cash": {"provider": cash}}},
    });
    b.s.write(|tx| -> CommandResult<()> {
        tx.put_doc(
            mk_store::DocTable::AccountLink,
            &mk_store::Doc {
                id: "snap|r1".into(),
                grp: Some("snap".into()),
                ord: 0,
                json: link,
            },
        )?;
        Ok(())
    })
    .unwrap();
}

fn lots(b: &mut Book, sym: &str) -> Vec<Value> {
    b.run(
        "inv.lots",
        json!({"account": "Assets:Brokerage", "security": sym, "as_of": "2026-11-06"}),
    )["lots"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn ib1_snaptrade_holdings_exact_lots_and_average_cost() {
    let mut b = inv_book();
    snaptrade_holdings(
        &mut b,
        json!([{"security": "VTI", "provider": "100"}, {"security": "FXAIX", "provider": "52.3"}]),
        "2341.10",
    );
    for (d, amt) in [
        ("2026-08-04", "2000.00"),
        ("2026-09-10", "-1035.35"),
        ("2026-11-05", "-500.00"),
    ] {
        match amt.strip_prefix('-') {
            Some(out) => b.transfer(d, "Assets:Checking", "Assets:Brokerage", out),
            None => b.transfer(d, "Assets:Brokerage", "Assets:Checking", amt),
        };
    }
    let p = b.run(
        "opening.inv_plan",
        json!({"account": "Assets:Brokerage", "today": "2026-11-06"}),
    );
    let pre: Vec<(String, String)> = p["feed_holdings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            (
                h["symbol"].as_str().unwrap().to_owned(),
                h["shares"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        pre,
        [
            ("VTI".into(), "100".into()),
            ("FXAIX".into(), "52.3".into())
        ]
    );
    assert_eq!(p["feed_cash"], "2341.10");
    assert_eq!(
        p["opening"]["amount"], "1876.45",
        "cash worked backwards: {p}"
    );
    assert_eq!(p["opening"]["date"], "2026-08-03");
    let args = json!({"account": "Assets:Brokerage", "today": "2026-11-06", "holdings": [
        {"symbol": "VTI", "shares": "100", "basis": {"level": "lots", "lots": [
            {"acquired": "2020-01-10", "shares": "60", "cost": "9000.00"},
            {"acquired": "2021-05-03", "shares": "40", "cost": "8000.00"}]}},
        {"symbol": "FXAIX", "shares": "52.3", "basis": {"level": "total", "per_share": "145.20"}},
    ]});
    let p = b.run("opening.inv_plan", args.clone());
    assert_eq!(p["holdings"][1]["basis"], "7593.96", "{p}");
    let out = b.run("opening.inv_apply", args);
    assert_eq!(out["holdings"].as_array().unwrap().len(), 2);
    let vti = lots(&mut b, "VTI");
    assert_eq!(vti.len(), 2);
    assert_eq!(vti[0]["basis"], "9000.00");
    assert_eq!(vti[1]["acquired"], "2021-05-03");
    let fx = lots(&mut b, "FXAIX");
    assert_eq!(fx[0]["basis"], "7593.96");
    assert_eq!(fx[0]["acquired_various"], true);
    let shares_in: Vec<Value> = b
        .run("txn.list", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["opening"] == true)
        .cloned()
        .collect();
    assert_eq!(shares_in.len(), 3, "cash plus one shares in per holding");
    assert_eq!(b.shown("Assets:Brokerage", "USD"), "2341.10");
    // Rerunning changes the same transactions.
    let again = json!({"account": "Assets:Brokerage", "today": "2026-11-06", "holdings": [
        {"symbol": "VTI", "shares": "100", "basis": {"level": "total", "total": "17500"}},
        {"symbol": "FXAIX", "shares": "52.3", "basis": {"level": "total", "per_share": "145.20"}},
    ]});
    b.run("opening.inv_apply", again);
    let n = b
        .run("txn.list", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["opening"] == true)
        .count();
    assert_eq!(n, 3);
    assert_eq!(lots(&mut b, "VTI")[0]["basis"], "17500.00");
}

#[test]
fn ib2_holdings_today_and_on_start() {
    let mut b = inv_book();
    b.transfer(
        "2026-08-04",
        "Assets:Brokerage",
        "Assets:Checking",
        "5000.00",
    );
    b.run(
        "inv.buy",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-10-01",
            "quantity": "10", "price": "250"}),
    );
    let p = b.run(
        "opening.inv_plan",
        json!({"account": "Assets:Brokerage", "today": "2026-11-06",
            "cash": {"balance": "2500.00", "as_of": "2026-11-06"},
            "holdings": [{"symbol": "VTI", "shares": "110"}]}),
    );
    let h = &p["holdings"][0];
    assert_eq!(h["today"], "110");
    assert_eq!(h["on_start"], "100", "{p}");
    assert_eq!(h["differs"], true);
    assert_eq!(p["opening"]["date"], "2026-08-03");
}

#[test]
fn ib3_basis_needed_then_supplied() {
    let mut b = inv_book();
    b.run(
        "price.set",
        json!({"security": "VTI", "date": "2026-08-01", "price": "200"}),
    );
    b.transfer(
        "2026-08-04",
        "Assets:Brokerage",
        "Assets:Checking",
        "100.00",
    );
    let args = json!({"account": "Assets:Brokerage", "today": "2026-11-06",
        "cash": {"balance": "100.00", "as_of": "2026-11-06"},
        "holdings": [{"symbol": "VTI", "shares": "50", "basis": {"level": "unknown"}}]});
    let p = b.run("opening.inv_plan", args.clone());
    assert_eq!(p["holdings"][0]["basis"], "10000.00", "market value on D");
    assert_eq!(p["holdings"][0]["basis_needed"], true);
    b.run("opening.inv_apply", args);
    let l = lots(&mut b, "VTI");
    assert_eq!(l[0]["basis"], "10000.00");
    assert_eq!(l[0]["basis_needed"], true);
    let c = b.run("opening.checklist", json!({}));
    assert_eq!(c["basis_text"], "1 holding needs cost basis");
    let row = c["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["full_name"] == "Assets:Brokerage")
        .unwrap()
        .clone();
    assert_eq!(row["status"], "basis_needed");
    let h = b.run("inv.holdings", json!({"as_of": "2026-11-06"}));
    assert_eq!(h["accounts"][0]["basis_text"], "1 holding needs cost basis");
    let att = b.run(
        "dashboard.block_data",
        json!({"dashboard": "home", "block": "attention", "today": "2026-11-06"}),
    );
    assert!(
        att.to_string().contains("1 holding needs cost basis"),
        "{att}"
    );
    let ob_before = b.shown("Opening Balances", "USD");
    assert_eq!(ob_before, "10000.00", "the cash opening is zero");
    b.run(
        "opening.set_basis",
        json!({"account": "Assets:Brokerage", "security": "VTI",
            "basis": {"level": "total", "total": "9000"}}),
    );
    let l = lots(&mut b, "VTI");
    assert_eq!(l.len(), 1);
    assert_eq!(l[0]["basis"], "9000.00");
    assert_eq!(l[0]["basis_needed"], false);
    assert_eq!(
        b.shown("Opening Balances", "USD"),
        "9000.00",
        "difference moved"
    );
    let c = b.run("opening.checklist", json!({}));
    assert!(c["basis_text"].is_null());
    b.run("history.undo", json!({}));
    let l = lots(&mut b, "VTI");
    assert_eq!(l[0]["basis"], "10000.00");
    assert_eq!(l[0]["basis_needed"], true);
}

#[test]
fn ib4_retirement_account_skips_basis() {
    let mut b = inv_book();
    b.run(
        "price.set",
        json!({"security": "VTI", "date": "2026-11-01", "price": "300"}),
    );
    let args = json!({"account": "Assets:Brokerage", "today": "2026-11-06", "retirement": true,
        "cash": {"balance": "0", "as_of": "2026-11-06"},
        "holdings": [{"symbol": "VTI", "shares": "10"}]});
    let p = b.run("opening.inv_plan", args.clone());
    assert_eq!(p["basis_step"], false);
    assert_eq!(p["holdings"][0]["basis"], "3000.00");
    assert_eq!(p["holdings"][0]["basis_needed"], false);
    b.run("opening.inv_apply", args.clone());
    let acc = b.run("account.get", json!({"account": "Assets:Brokerage"}));
    assert_eq!(acc["retirement"], true);
    let l = lots(&mut b, "VTI");
    assert_eq!(l[0]["basis"], "3000.00");
    assert_eq!(l[0]["basis_needed"], false, "not flagged");
    let mut anyway = args;
    anyway["enter_basis"] = json!(true);
    let p = b.run("opening.inv_plan", anyway.clone());
    assert_eq!(p["basis_step"], true);
    assert!(p["holdings"][0]["basis"].is_null(), "a level to choose");
    let e = b.err("opening.inv_apply", anyway);
    assert!(e.to_string().contains("choose how to give"), "{e}");
    // Editable in Edit account.
    b.run(
        "account.update",
        json!({"account": "Assets:Brokerage", "retirement": false}),
    );
    let e = b.err(
        "account.update",
        json!({"account": "Assets:Checking", "retirement": true}),
    );
    assert!(e.to_string().contains("brokerage"));
}

#[test]
fn ib5_check_total_and_stale_price() {
    let mut b = inv_book();
    b.run(
        "price.set",
        json!({"security": "VTI", "date": "2026-11-05", "price": "300"}),
    );
    b.run(
        "price.set",
        json!({"security": "FXAIX", "date": "2026-09-01", "price": "150"}),
    );
    let args = json!({"account": "Assets:Brokerage", "today": "2026-11-06",
        "cash": {"balance": "500.00", "as_of": "2026-11-06"},
        "holdings": [{"symbol": "VTI", "shares": "10", "basis": {"level": "unknown"}},
            {"symbol": "FXAIX", "shares": "20", "basis": {"level": "unknown"}}],
        "broker_total": "6500.00"});
    let p = b.run("opening.inv_plan", args.clone());
    // 500 + 10 × 300 + 20 × 150.
    assert_eq!(p["total"]["total"], "6500.00", "{p}");
    assert_eq!(p["total"]["matches"], true);
    assert_eq!(p["holdings"][0]["value"], "3000.00");
    assert_eq!(p["holdings"][1]["stale"], true);
    let mut off = args;
    off["broker_total"] = json!("6900.00");
    let p = b.run("opening.inv_plan", off);
    assert_eq!(p["total"]["difference"], "400.00");
    let c = p["inv_causes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "price")
        .unwrap()
        .clone();
    assert_eq!(c["symbol"], "FXAIX");
    assert!(c["text"].as_str().unwrap().contains("out of date"));
}

#[test]
fn ib6_paste_known_and_unknown() {
    let mut b = inv_book();
    let p = b.run(
        "opening.inv_paste",
        json!({"text": "Symbol, Quantity, Cost\nVTI, 100, 21000\nXYZ, 5\n"}),
    );
    let rows = p["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(p["skipped"], 1, "the header line");
    assert_eq!(rows[0]["symbol"], "VTI");
    assert_eq!(rows[0]["cost"], "21000.00");
    assert_eq!(rows[0]["level"], "total");
    assert_eq!(rows[1]["known"], false);
    assert!(rows[1]["level"].is_null());
    assert_eq!(p["unknown"], json!(["XYZ"]));
    let args = json!({"account": "Assets:Brokerage", "today": "2026-11-06",
        "cash": {"balance": "0", "as_of": "2026-11-06"},
        "holdings": [{"symbol": "VTI", "shares": "100", "basis": {"level": "total", "total": "21000"}},
            {"symbol": "XYZ", "shares": "5"}]});
    let e = b.err("opening.inv_apply", args.clone());
    assert_eq!(e.code(), "unknown_securities");
    // Confirm: the security is created; its basis level is still to choose.
    b.run(
        "security.create",
        json!({"symbol": "XYZ", "name": "XYZ", "kind": "other"}),
    );
    let p = b.run("opening.inv_plan", args);
    assert_eq!(p["holdings"][1]["known"], true);
    assert!(p["holdings"][1]["level"].is_null());
    assert_eq!(p["holdings"][0]["basis"], "21000.00");
}
