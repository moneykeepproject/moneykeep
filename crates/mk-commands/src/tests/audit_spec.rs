//! ledger-audit-undo.md acceptance tests (AT-n). AT-7 is in `props.rs`.

use super::*;

fn grocery_args(amount: &str) -> Value {
    let neg = format!("-{amount}");
    json!({"date": "2026-07-01", "splits": [
        {"account": "Expenses:Groceries", "value": amount},
        {"account": "Assets:Checking", "value": neg}]})
}

#[test]
fn at1_undo_create() {
    let mut b = Book::new();
    b.standard();
    let n0 = b.audit_len();
    let t = b.run("txn.create", grocery_args("50.00"));
    b.run("history.undo", json!({}));
    assert_eq!(b.err("txn.get", json!({"id": t["id"]})).code(), "not_found");
    assert_eq!(b.shown("Assets:Checking", "USD"), "0.00");
    let log = b.s.audit_entries(10).unwrap();
    assert_eq!(b.audit_len(), n0 + 2);
    assert_eq!(log[0].command, "history.undo");
    assert_eq!(log[0].undoes, Some(log[1].seq));
    let hist = b.run("history.list", json!({"limit": 2}));
    assert_eq!(hist[1]["undone"], true);
    assert_eq!(hist[1]["undone_by"], json!(log[0].seq));
    // The account can be deleted again: the undone split never "happened".
    b.run("account.delete", json!({"account": "Expenses:Groceries"}));
}

#[test]
fn at2_redo_restores_same_id() {
    let mut b = Book::new();
    b.standard();
    let t = b.run("txn.create", grocery_args("50.00"));
    b.run("history.undo", json!({}));
    b.run("history.redo", json!({}));
    let back = b.run("txn.get", json!({"id": t["id"]}));
    assert_eq!(back, t);
    assert_eq!(b.shown("Assets:Checking", "USD"), "-50.00");
    assert_eq!(
        b.err("history.redo", json!({})).to_string(),
        "nothing to redo"
    );
    // Undo after redo undoes the redo's effect.
    b.run("history.undo", json!({}));
    assert_eq!(b.err("txn.get", json!({"id": t["id"]})).code(), "not_found");
}

#[test]
fn at3_undo_update_then_create() {
    let mut b = Book::new();
    b.standard();
    let t = b.run("txn.create", grocery_args("50.00"));
    let id = t["id"].clone();
    let mut upd = grocery_args("75.00");
    upd["id"] = id.clone();
    b.run("txn.update", upd);
    b.run("history.undo", json!({}));
    assert_eq!(b.run("txn.get", json!({"id": id})), t);
    b.run("history.undo", json!({}));
    assert_eq!(b.err("txn.get", json!({"id": id})).code(), "not_found");
}

#[test]
fn at4_undo_rejected_after_other_actor_reconciles() {
    let mut b = Book::new();
    b.standard();
    let t = b.run("txn.create", grocery_args("50.00"));
    b.as_actor(
        "bob",
        "txn.set_reconcile",
        json!({"splits": [{"id": t["id"], "split": 1, "state": "r"}]}),
    );
    let n = b.audit_len();
    let e = b.err("history.undo", json!({}));
    assert_eq!(e.code(), "rejected");
    assert!(e.to_string().contains("reconciled"), "{e}");
    assert_eq!(b.audit_len(), n, "nothing changes");
    assert_eq!(
        b.run("txn.get", json!({"id": t["id"]}))["splits"][1]["reconcile"],
        "r"
    );
}

#[test]
fn at5_undo_rejected_in_closed_period() {
    let mut b = Book::new();
    b.standard();
    b.run("txn.create", grocery_args("50.00"));
    b.as_actor(
        "bob",
        "book.set_closing_date",
        json!({"date": "2026-07-31"}),
    );
    let e = b.err("history.undo", json!({}));
    assert_eq!(e.code(), "rejected");
    assert!(e.to_string().contains("closed"), "{e}");
}

#[test]
fn at6_audit_rows_are_append_only() {
    // The Phase 0 trigger test lives in mk-store (audit_log_is_append_only) and still
    // passes; here we check undo adds rows rather than changing them.
    let mut b = Book::new();
    b.standard();
    b.run("txn.create", grocery_args("50.00"));
    let before: Vec<_> =
        b.s.audit_entries(100)
            .unwrap()
            .into_iter()
            .map(|e| (e.seq, e.command, e.args))
            .collect();
    b.run("history.undo", json!({}));
    let after: Vec<_> =
        b.s.audit_entries(100)
            .unwrap()
            .into_iter()
            .skip(1)
            .map(|e| (e.seq, e.command, e.args))
            .collect();
    assert_eq!(before, after);
}

#[test]
fn nothing_to_undo_and_actor_isolation() {
    let mut b = Book::new();
    assert_eq!(
        b.err("history.undo", json!({})).to_string(),
        "nothing to undo"
    );
    b.standard();
    b.as_actor("bob", "txn.create", grocery_args("5.00"));
    // A's most recent change is the Visa account creation, not Bob's transaction.
    let out = b.run("history.undo", json!({}));
    assert_eq!(out["command"], "account.create");
    assert_eq!(
        b.err("account.get", json!({"account": "Liabilities:Visa"}))
            .code(),
        "not_found"
    );
    assert_eq!(b.run("txn.list", json!({})).as_array().unwrap().len(), 1);
}

#[test]
fn undo_delete_recreates_same_id() {
    let mut b = Book::new();
    b.standard();
    let t = b.run("txn.create", grocery_args("50.00"));
    b.run("txn.delete", json!({"id": t["id"]}));
    b.run("history.undo", json!({}));
    assert_eq!(b.run("txn.get", json!({"id": t["id"]})), t);
}

#[test]
fn undo_account_create_fails_once_used() {
    let mut b = Book::new();
    b.account(json!({"name": "Cash", "type": "asset"}));
    b.as_actor(
        "bob",
        "txn.create",
        json!({"date": "2026-07-01", "splits": [
        {"account": "Cash", "value": "5.00"},
        {"account": "Opening Balances", "value": "-5.00"}]}),
    );
    let e = b.err("history.undo", json!({}));
    assert_eq!(e.code(), "rejected");
    // Even after Bob deletes the transaction, the account has had a split.
    let id = b.run("txn.list", json!({}))[0]["id"].clone();
    b.as_actor("bob", "txn.delete", json!({"id": id}));
    assert_eq!(b.err("history.undo", json!({})).code(), "rejected");
}

#[test]
fn undo_rejected_when_entity_changed_later() {
    let mut b = Book::new();
    b.standard();
    let t = b.run("txn.create", grocery_args("50.00"));
    let mut upd = grocery_args("60.00");
    upd["id"] = t["id"].clone();
    b.as_actor("bob", "txn.update", upd);
    let e = b.err("history.undo", json!({}));
    assert!(e.to_string().contains("later command"), "{e}");
}

#[test]
fn redo_blocked_by_new_change() {
    let mut b = Book::new();
    b.standard();
    b.run("txn.create", grocery_args("50.00"));
    b.run("history.undo", json!({}));
    b.run("txn.create", grocery_args("10.00"));
    assert_eq!(
        b.err("history.redo", json!({})).to_string(),
        "nothing to redo"
    );
}

#[test]
fn undo_module_and_setting_commands() {
    let mut b = Book::new();
    let s0 = b.state();
    b.run("module.enable", json!({"module": "progress-invoicing"}));
    b.run(
        "setting.set",
        json!({"module": "estimates", "key": "valid_days", "value": 30}),
    );
    b.run("module.disable", json!({"module": "progress-invoicing"}));
    b.run("history.undo", json!({}));
    assert_eq!(
        b.s.get_setting("progress-invoicing", "enabled").unwrap(),
        Some(json!(true))
    );
    b.run("history.undo", json!({}));
    assert_eq!(b.s.get_setting("estimates", "valid_days").unwrap(), None);
    b.run("history.undo", json!({}));
    assert_eq!(b.state(), s0);
}

#[test]
fn undo_closing_date_rules() {
    let mut b = Book::new();
    b.standard();
    b.run("book.set_closing_date", json!({"date": "2026-06-30"}));
    b.run("book.set_closing_date", json!({"date": "2026-05-31"}));
    // Someone records a transaction in the newly opened range.
    b.as_actor(
        "bob",
        "txn.create",
        json!({"date": "2026-06-15", "splits": [
        {"account": "Expenses:Groceries", "value": "1.00"},
        {"account": "Assets:Checking", "value": "-1.00"}]}),
    );
    let e = b.err("history.undo", json!({}));
    assert_eq!(e.code(), "rejected");
    // Without such a transaction, undo of a move works.
    let mut c = Book::new();
    c.run("book.set_closing_date", json!({"date": "2026-06-30"}));
    c.run("book.set_closing_date", json!({"date": "2026-05-31"}));
    c.run("history.undo", json!({}));
    assert_eq!(c.run("book.info", json!({}))["closing_date"], "2026-06-30");
}

#[test]
fn snapshots_record_before_and_after() {
    let mut b = Book::new();
    let id = b.account(json!({"name": "Cash", "type": "asset"}));
    b.run("account.update", json!({"account": id, "name": "Wallet"}));
    let e = &b.s.audit_entries(1).unwrap()[0];
    assert_eq!(e.before.as_ref().unwrap()[0]["data"]["name"], "Cash");
    assert_eq!(e.after.as_ref().unwrap()[0]["data"]["name"], "Wallet");
    assert_eq!(e.after.as_ref().unwrap()[0]["kind"], "account");
}
