//! scheduled-transactions.md acceptance tests (the date-math ones also have unit
//! tests in mk-ledger).

use super::*;

fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Salary", "type": "income"}));
    b.account(json!({"name": "Rent", "type": "expense"}));
    b.account(json!({"name": "Utilities", "type": "expense"}));
    b
}

/// A template paying `amount` from Checking into `category`.
fn bill(category: &str, amount: &str) -> Value {
    json!({"payee": category, "splits": [
        {"account": category, "value": amount},
        {"account": "Assets:Checking", "value": format!("-{amount}")}]})
}

fn create(b: &mut Book, args: Value) -> Value {
    b.run("schedule.create", args)
}

fn upcoming(b: &mut Book, args: Value) -> Vec<Value> {
    b.run("schedule.upcoming", args)["occurrences"]
        .as_array()
        .unwrap()
        .clone()
}

fn dues(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|r| r["due"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn scheduled_transactions_at1_to_5_through_commands() {
    let mut b = book();
    // AT1: monthly on day 31.
    let s = create(
        &mut b,
        json!({"template": bill("Rent", "100.00"), "start": "2026-01-31",
               "recurrence": {"frequency": "monthly", "day": 31}}),
    );
    assert_eq!(s["next_due"]["nominal"], "2026-01-31");
    let rows = upcoming(
        &mut b,
        json!({"from": "2026-01-01", "to": "2026-05-31", "today": "2026-01-01"}),
    );
    assert_eq!(
        dues(&rows),
        [
            "2026-01-31",
            "2026-02-28",
            "2026-03-31",
            "2026-04-30",
            "2026-05-31"
        ]
    );
    let rows = upcoming(
        &mut b,
        json!({"from": "2028-02-01", "to": "2028-02-29", "today": "2028-02-01"}),
    );
    assert!(dues(&rows).contains(&"2028-02-29".to_owned()));

    // AT5: weekend before, monthly on day 1.
    let mut b = book();
    create(
        &mut b,
        json!({"template": bill("Rent", "100.00"), "start": "2026-11-01", "weekend": "before",
               "recurrence": {"frequency": "monthly", "day": 1}}),
    );
    let rows = upcoming(
        &mut b,
        json!({"from": "2026-10-01", "to": "2026-12-31", "today": "2026-10-01"}),
    );
    assert_eq!(dues(&rows), ["2026-10-30", "2026-12-01"]);
    assert_eq!(rows[0]["nominal"], "2026-11-01");
}

#[test]
fn scheduled_transactions_at6_end_after_three() {
    let mut b = book();
    let s = create(
        &mut b,
        json!({"template": bill("Utilities", "40.00"), "start": "2026-08-01",
               "recurrence": {"frequency": "monthly"}, "end": {"after": 3}}),
    );
    let id = s["id"].clone();
    b.run("schedule.enter", json!({"schedule": id}));
    b.run("schedule.skip", json!({"schedule": id}));
    b.run("schedule.enter", json!({"schedule": id}));
    let s = b.run("schedule.get", json!({"schedule": id}));
    assert_eq!(s["next_due"], Value::Null);
    assert_eq!(s["ended"], true);
    assert!(
        upcoming(
            &mut b,
            json!({"from": "2026-01-01", "to": "2027-12-31", "today": "2026-10-20"})
        )
        .is_empty()
    );
    assert_eq!(
        b.err("schedule.enter", json!({"schedule": id})).code(),
        "rejected"
    );
}

fn estimate(b: &mut Book) -> Value {
    create(
        b,
        json!({"name": "Electric", "template": bill("Utilities", "60.00"), "start": "2026-10-10",
               "amount_kind": "estimate", "recurrence": {"frequency": "monthly"}}),
    )
}

#[test]
fn scheduled_transactions_at7_enter_estimate_with_actual_amount() {
    let mut b = book();
    let s = estimate(&mut b);
    let t = b.run(
        "schedule.enter",
        json!({"schedule": "Electric", "date": "2026-10-10", "splits": [
            {"account": "Utilities", "value": "63.18"},
            {"account": "Assets:Checking", "value": "-63.18"}]}),
    );
    let got = b.run("txn.get", json!({"id": t["id"]}));
    assert_eq!(got["schedule"], s["id"]);
    assert_eq!(got["occurrence"], "2026-10-10");
    assert_eq!(got["date"], "2026-10-10");
    assert!(
        got["splits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["value"] == "63.18")
    );
    let s = b.run("schedule.get", json!({"schedule": "Electric"}));
    assert_eq!(s["next_due"]["nominal"], "2026-11-10");
    let e = b.err(
        "schedule.enter",
        json!({"schedule": "Electric", "date": "2026-10-10"}),
    );
    assert_eq!(e.code(), "conflict");
    // Replacement splits that do not balance are rejected; the occurrence stays pending.
    let e = b.err(
        "schedule.enter",
        json!({"schedule": "Electric", "splits": [
            {"account": "Utilities", "value": "63.18"},
            {"account": "Assets:Checking", "value": "-60.00"}]}),
    );
    assert_eq!(e.code(), "invalid_args");
    let s = b.run("schedule.get", json!({"schedule": "Electric"}));
    assert_eq!(s["next_due"]["nominal"], "2026-11-10");
    // Not an occurrence date.
    assert_eq!(
        b.err(
            "schedule.enter",
            json!({"schedule": "Electric", "date": "2026-10-11"})
        )
        .code(),
        "invalid_args"
    );
}

#[test]
fn scheduled_transactions_at8_undo_and_redo_enter() {
    let mut b = book();
    estimate(&mut b);
    b.run(
        "schedule.enter",
        json!({"schedule": "Electric", "splits": [
            {"account": "Utilities", "value": "63.18"},
            {"account": "Assets:Checking", "value": "-63.18"}]}),
    );
    assert_eq!(b.txn_count(), 1);
    b.run("history.undo", json!({}));
    assert_eq!(b.txn_count(), 0);
    let s = b.run("schedule.get", json!({"schedule": "Electric"}));
    assert_eq!(s["next_due"]["nominal"], "2026-10-10");
    b.run("history.redo", json!({}));
    let txns = b.run("txn.list", json!({}));
    assert_eq!(txns.as_array().unwrap().len(), 1);
    assert!(
        txns[0]["splits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["value"] == "63.18")
    );
    let s = b.run("schedule.get", json!({"schedule": "Electric"}));
    assert_eq!(s["next_due"]["nominal"], "2026-11-10");
    // Skip is undoable too.
    b.run("schedule.skip", json!({"schedule": "Electric"}));
    assert_eq!(
        b.run("schedule.get", json!({"schedule": "Electric"}))["next_due"]["nominal"],
        "2026-12-10"
    );
    b.run("history.undo", json!({}));
    assert_eq!(
        b.run("schedule.get", json!({"schedule": "Electric"}))["next_due"]["nominal"],
        "2026-11-10"
    );
}

#[test]
fn scheduled_transactions_at9_run_due_enters_as_schedule_once() {
    let mut b = book();
    create(
        &mut b,
        json!({"template": bill("Rent", "1200.00"), "start": "2026-08-15", "auto_enter": true,
               "recurrence": {"frequency": "monthly"}}),
    );
    // Not auto-enter: never entered automatically.
    create(
        &mut b,
        json!({"template": bill("Utilities", "50.00"), "start": "2026-08-01",
               "recurrence": {"frequency": "monthly"}}),
    );
    let r = b.run("schedule.run_due", json!({"today": "2026-10-20"}));
    let entered: Vec<&str> = r["entered"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["nominal"].as_str().unwrap())
        .collect();
    assert_eq!(entered, ["2026-08-15", "2026-09-15", "2026-10-15"]);
    assert!(r["failed"].as_array().unwrap().is_empty());
    assert_eq!(b.txn_count(), 3);
    let h = b.run("history.list", json!({"limit": 1}));
    assert_eq!(h[0]["actor"], "schedule");
    assert_eq!(h[0]["command"], "schedule.run_due");
    let before = b.run("history.list", json!({})).as_array().unwrap().len();
    let r = b.run("schedule.run_due", json!({"today": "2026-10-20"}));
    assert!(r["entered"].as_array().unwrap().is_empty());
    assert_eq!(b.txn_count(), 3);
    // A run that found nothing leaves no audit entry.
    assert_eq!(
        b.run("history.list", json!({})).as_array().unwrap().len(),
        before
    );
}

#[test]
fn run_due_reports_failures_and_continues() {
    let mut b = book();
    b.account(json!({"name": "Old", "type": "expense"}));
    create(
        &mut b,
        json!({"name": "A", "template": bill("Old", "5.00"), "start": "2026-10-01", "auto_enter": true,
               "recurrence": {"frequency": "once"}}),
    );
    create(
        &mut b,
        json!({"name": "B", "template": bill("Rent", "5.00"), "start": "2026-10-02", "auto_enter": true,
               "recurrence": {"frequency": "once"}}),
    );
    b.run("account.archive", json!({"account": "Old"}));
    let r = b.run("schedule.run_due", json!({"today": "2026-10-20"}));
    assert_eq!(r["failed"].as_array().unwrap().len(), 1);
    assert_eq!(r["failed"][0]["name"], "A");
    assert_eq!(r["entered"].as_array().unwrap().len(), 1);
    let a = b.run("schedule.get", json!({"schedule": "A"}));
    assert_eq!(a["next_due"]["nominal"], "2026-10-01", "stays due");
}

#[test]
fn scheduled_transactions_at10_upcoming_when() {
    let mut b = book();
    for (name, start) in [
        ("Water", "2026-10-15"),
        ("Phone", "2026-10-25"),
        ("Gas", "2026-11-15"),
    ] {
        create(
            &mut b,
            json!({"name": name, "template": bill("Utilities", "30.00"), "start": start,
                   "recurrence": {"frequency": "once"}, "remind_days": 7}),
        );
    }
    let rows = upcoming(&mut b, json!({"today": "2026-10-20"}));
    let when: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r["name"].as_str().unwrap(), r["when"].as_str().unwrap()))
        .collect();
    assert_eq!(
        when,
        [
            ("Water", "overdue"),
            ("Phone", "due_soon"),
            ("Gas", "later")
        ]
    );
    // Without `account`: as seen from the first split's account (Utilities).
    assert_eq!(rows[0]["amount"], "30.00");
    assert_eq!(rows[0]["category"], "Assets:Checking");
    // The money-account view for front ends.
    assert_eq!(rows[0]["money"]["amount"], "-30.00");
    assert_eq!(rows[0]["money"]["account"]["full_name"], "Assets:Checking");
    assert_eq!(rows[0]["money"]["category"], "Utilities");
    // An overdue item before `from` is still returned, flagged.
    let rows = upcoming(&mut b, json!({"today": "2026-10-20", "from": "2026-10-20"}));
    assert_eq!(rows[0]["name"], "Water");
    assert_eq!(rows[0]["before_from"], true);
    // Paused schedules are left out unless asked.
    b.run("schedule.pause", json!({"schedule": "Gas"}));
    assert_eq!(upcoming(&mut b, json!({"today": "2026-10-20"})).len(), 2);
    assert_eq!(
        upcoming(
            &mut b,
            json!({"today": "2026-10-20", "include_paused": true})
        )
        .len(),
        3
    );
    // Seen from Checking, the amount is negative.
    let rows = upcoming(
        &mut b,
        json!({"today": "2026-10-20", "account": "Assets:Checking"}),
    );
    assert_eq!(rows[0]["amount"], "-30.00");
}

#[test]
fn scheduled_transactions_at11_forecast() {
    let mut b = book();
    b.transfer(
        "2026-10-01",
        "Assets:Checking",
        "Opening Balances",
        "1000.00",
    );
    create(
        &mut b,
        json!({"name": "Pay", "start": "2026-10-30", "recurrence": {"frequency": "monthly", "day": 30},
               "template": {"payee": "Employer", "splits": [
                   {"account": "Assets:Checking", "value": "2000.00"},
                   {"account": "Salary", "value": "-2000.00"}]}}),
    );
    create(
        &mut b,
        json!({"name": "Rent", "template": bill("Rent", "3500.00"), "start": "2026-11-01",
               "recurrence": {"frequency": "monthly", "day": 1}}),
    );
    let f = b.run(
        "schedule.forecast",
        json!({"account": "Assets:Checking", "to": "2026-12-05", "today": "2026-10-20"}),
    );
    assert_eq!(f["balance"], "1000.00");
    let rows: Vec<(String, String, String)> = f["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["date"].as_str().unwrap().to_owned(),
                r["name"].as_str().unwrap().to_owned(),
                r["balance"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let want = [
        ("2026-10-30", "Pay", "3000.00"),
        ("2026-11-01", "Rent", "-500.00"),
        ("2026-11-30", "Pay", "1500.00"),
        ("2026-12-01", "Rent", "-2000.00"),
    ];
    assert_eq!(
        rows,
        want.map(|(a, b, c)| (a.to_owned(), b.to_owned(), c.to_owned()))
    );
    assert_eq!(
        f["lowest"],
        json!({"balance": "-2000.00", "date": "2026-12-01"})
    );
    assert_eq!(
        b.err(
            "schedule.forecast",
            json!({"account": "Assets:Checking", "to": "2030-01-01", "today": "2026-10-20"})
        )
        .code(),
        "invalid_args"
    );
}

#[test]
fn scheduled_transactions_at12_delete_returns_to_pending_void_does_not() {
    let mut b = book();
    create(
        &mut b,
        json!({"name": "Rent", "template": bill("Rent", "100.00"), "start": "2026-10-01",
               "recurrence": {"frequency": "monthly"}}),
    );
    let t = b.run("schedule.enter", json!({"schedule": "Rent"}));
    b.run("txn.void", json!({"id": t["id"]}));
    let s = b.run("schedule.get", json!({"schedule": "Rent"}));
    assert_eq!(
        s["next_due"]["nominal"], "2026-11-01",
        "voiding keeps it entered"
    );
    b.run("txn.unvoid", json!({"id": t["id"]}));
    b.run("txn.delete", json!({"id": t["id"]}));
    let s = b.run("schedule.get", json!({"schedule": "Rent"}));
    assert_eq!(
        s["next_due"]["nominal"], "2026-10-01",
        "deleting makes it pending"
    );
    // Undoing the delete links it again.
    b.run("history.undo", json!({}));
    let s = b.run("schedule.get", json!({"schedule": "Rent"}));
    assert_eq!(s["next_due"]["nominal"], "2026-11-01");
    assert_eq!(
        b.run("txn.get", json!({"id": t["id"]}))["schedule"],
        s["id"]
    );
}

#[test]
fn scheduled_transactions_at13_summary() {
    let mut b = book();
    let s = create(
        &mut b,
        json!({"template": bill("Rent", "10.00"), "start": "2026-10-02",
               "recurrence": {"frequency": "weekly", "interval": 2}}),
    );
    assert_eq!(s["summary"], "Every 2 weeks on Friday");
    let s = create(
        &mut b,
        json!({"name": "Card", "template": bill("Utilities", "10.00"), "start": "2026-10-31",
               "recurrence": {"frequency": "monthly", "day": "last"}, "end": {"after": 12}}),
    );
    assert_eq!(s["summary"], "Monthly on the last day, 12 times");
    let l = b.run("schedule.list", json!({}));
    assert_eq!(l.as_array().unwrap().len(), 2);
    assert!(l[0]["summary"].is_string());
}

#[test]
fn schedule_management() {
    let mut b = book();
    // Name defaults to the payee; template accounts are checked.
    let s = create(
        &mut b,
        json!({"template": bill("Rent", "10.00"), "start": "2026-10-02",
               "recurrence": {"frequency": "monthly"}}),
    );
    assert_eq!(s["name"], "Rent");
    assert_eq!(s["template"]["splits"][0]["account_name"], "Rent");
    let e = b.err(
        "schedule.create",
        json!({"template": bill("Expenses", "10.00"), "start": "2026-10-02",
               "recurrence": {"frequency": "monthly"}}),
    );
    assert_eq!(e.code(), "rejected", "placeholder account");
    let e = b.err(
        "schedule.create",
        json!({"template": {"payee": "X", "splits": [
            {"account": "Rent", "value": "10.00"},
            {"account": "Assets:Checking", "value": "-9.00"}]},
            "start": "2026-10-02", "recurrence": {"frequency": "monthly"}}),
    );
    assert_eq!(e.code(), "invalid_args", "unbalanced template");
    // Update replaces fields; pending occurrences are recomputed.
    let u = b.run(
        "schedule.update",
        json!({"schedule": "Rent", "recurrence": {"frequency": "monthly", "day": 15}}),
    );
    assert_eq!(u["next_due"]["nominal"], "2026-10-15");
    // Pause and resume.
    b.run("schedule.pause", json!({"schedule": "Rent"}));
    assert_eq!(
        b.err("schedule.pause", json!({"schedule": "Rent"})).code(),
        "rejected"
    );
    assert_eq!(
        b.run("schedule.list", json!({"status": "paused"}))
            .as_array()
            .unwrap()
            .len(),
        1
    );
    b.run("schedule.resume", json!({"schedule": "Rent"}));
    // from_txn: start one period after the transaction.
    let t = b.transfer("2026-10-05", "Utilities", "Assets:Checking", "42.00");
    let s = b.run(
        "schedule.from_txn",
        json!({"txn": t, "recurrence": {"frequency": "monthly"}, "name": "Water"}),
    );
    assert_eq!(s["start"], "2026-11-05");
    assert_eq!(
        s["amount"], "42.00",
        "seen from the first split (Utilities)"
    );
    // Delete keeps entered transactions and drops their link.
    let e = b.run("schedule.enter", json!({"schedule": "Water"}));
    b.run("schedule.delete", json!({"schedule": "Water"}));
    let got = b.run("txn.get", json!({"id": e["id"]}));
    assert!(got.get("schedule").is_none());
    assert_eq!(
        b.err("schedule.get", json!({"schedule": "Water"})).code(),
        "not_found"
    );
    b.run("history.undo", json!({}));
    assert_eq!(
        b.run("txn.get", json!({"id": e["id"]}))["occurrence"],
        "2026-11-05"
    );
}

#[test]
fn schedule_semi_monthly_and_weekday_through_commands() {
    let mut b = book();
    let s = create(
        &mut b,
        json!({"template": bill("Rent", "10.00"), "start": "2026-02-01",
               "recurrence": {"frequency": "semi_monthly", "days": [15, "last"]}}),
    );
    assert_eq!(s["summary"], "Twice a month on the 15th and last day");
    let rows = upcoming(
        &mut b,
        json!({"from": "2026-02-01", "to": "2026-03-31", "today": "2026-02-01"}),
    );
    assert_eq!(
        dues(&rows),
        ["2026-02-15", "2026-02-28", "2026-03-15", "2026-03-31"]
    );
    let mut b = book();
    create(
        &mut b,
        json!({"template": bill("Rent", "10.00"), "start": "2026-10-01",
               "recurrence": {"frequency": "monthly", "weekday": "tuesday", "week": 2}}),
    );
    let rows = upcoming(
        &mut b,
        json!({"from": "2026-10-01", "to": "2026-12-31", "today": "2026-10-01"}),
    );
    assert_eq!(dues(&rows), ["2026-10-13", "2026-11-10", "2026-12-08"]);
    let e = b.err(
        "schedule.create",
        json!({"template": bill("Rent", "10.00"), "start": "2026-10-01",
               "recurrence": {"frequency": "monthly", "weekday": "tuesday"}}),
    );
    assert_eq!(e.code(), "invalid_args");
}

#[test]
fn schedule_preview_for_forms() {
    let mut b = book();
    let p = b.run(
        "schedule.preview",
        json!({"recurrence": {"frequency": "weekly", "interval": 2}, "start": "2026-10-02"}),
    );
    assert_eq!(p["summary"], "Every 2 weeks on Friday");
    let next: Vec<&str> = p["next"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["due"].as_str().unwrap())
        .collect();
    assert_eq!(next, ["2026-10-02", "2026-10-16", "2026-10-30"]);
    let p = b.run(
        "schedule.preview",
        json!({"recurrence": {"frequency": "monthly"}, "after": "2026-10-05"}),
    );
    assert_eq!(p["start"], "2026-11-05");
    assert_eq!(b.txn_count(), 0);
}
