//! import-rules.md acceptance tests.

use super::*;

fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Household", "type": "expense", "parent": "Expenses"}));
    b
}

fn rule(b: &mut Book, args: Value) -> Value {
    b.run("rule.create", args)
}

fn first_row(b: &mut Book, payee: &str, amount: &str) -> Value {
    let file = format!("!Type:Bank\nD10/01/2026\nT{amount}\nP{payee}\n^\n");
    b.stage_qif(&file, "Assets:Checking")["rows"][0].clone()
}

#[test]
fn import_rules_at1_contains_categorises() {
    let mut b = book();
    let r = rule(
        &mut b,
        json!({"name": "Kroger", "conditions": [{"field": "payee", "op": "contains", "value": "KROGER"}],
        "actions": [{"action": "set_account", "account": "Expenses:Groceries"}]}),
    );
    let row = first_row(&mut b, "KROGER #123 LIMA OH", "-54.20");
    assert_eq!(row["other"][0]["account_name"], "Expenses:Groceries");
    assert_eq!(row["other_source"], "rule");
    assert_eq!(row["rule_ids"], json!([r["id"]]));
}

#[test]
fn import_rules_at2_priority_and_stop() {
    let mut b = book();
    rule(
        &mut b,
        json!({"name": "Low", "priority": 10, "conditions": [{"field": "payee", "op": "contains", "value": "mart"}],
        "actions": [{"action": "set_account", "account": "Expenses:Household"}]}),
    );
    let high = rule(
        &mut b,
        json!({"name": "High", "priority": 1, "stop": true, "conditions": [{"field": "payee", "op": "starts_with", "value": "mega"}],
        "actions": [{"action": "set_account", "account": "Expenses:Groceries"}, {"action": "set_payee", "payee": "MegaMart"}]}),
    );
    let row = first_row(&mut b, "MEGAMART 77", "-10.00");
    assert_eq!(row["other"][0]["account_name"], "Expenses:Groceries");
    assert_eq!(row["payee"], "MegaMart");
    assert_eq!(row["rule_ids"], json!([high["id"]]));
}

#[test]
fn import_rules_at3_stop_false_chain() {
    let mut b = book();
    let a = rule(
        &mut b,
        json!({"name": "A", "stop": false, "conditions": [{"field": "payee", "op": "contains", "value": "kroger"}],
        "actions": [{"action": "set_payee", "payee": "Kroger"}]}),
    );
    let c = rule(
        &mut b,
        json!({"name": "B", "conditions": [{"field": "direction", "op": "eq", "value": "out"}],
        "actions": [{"action": "set_account", "account": "Expenses:Groceries"}]}),
    );
    let row = first_row(&mut b, "KROGER #9", "-3.00");
    assert_eq!(row["payee"], "Kroger");
    assert_eq!(row["other"][0]["account_name"], "Expenses:Groceries");
    assert_eq!(row["rule_ids"], json!([a["id"], c["id"]]));
}

#[test]
fn import_rules_at4_amount_between() {
    let mut b = book();
    rule(
        &mut b,
        json!({"name": "Mid", "conditions": [{"field": "amount", "op": "between", "value": ["-100.00", "-50.00"]}],
        "actions": [{"action": "set_account", "account": "Expenses:Household"}]}),
    );
    let test = |b: &mut Book, amount: &str| {
        b.run(
            "rule.test",
            json!({"account": "Assets:Checking", "amount": amount, "payee": "X"}),
        )
    };
    assert_eq!(
        test(&mut b, "-75.00")["rule_ids"].as_array().unwrap().len(),
        1
    );
    assert_eq!(test(&mut b, "-25.00")["rule_ids"], json!([]));
}

#[test]
fn import_rules_at5_split_remainder_to_first() {
    let mut b = book();
    rule(
        &mut b,
        json!({"name": "Half", "conditions": [{"field": "payee", "op": "equals", "value": "shared"}],
        "actions": [{"action": "split", "shares": [{"account": "Expenses:Groceries", "percent": "50"}, {"account": "Expenses:Household", "percent": "50"}]}]}),
    );
    let row = first_row(&mut b, "Shared", "-10.01");
    let values: Vec<&str> = row["other"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["value"].as_str().unwrap())
        .collect();
    // Other-side values are the negation of the row's shares (-5.01, -5.00).
    assert_eq!(values, ["5.01", "5.00"]);
    let t = b.run(
        "rule.test",
        json!({"account": "Assets:Checking", "amount": "-10.01", "payee": "shared"}),
    );
    assert_eq!(t["other"][0]["value"], "5.01");
    // 33.33/33.33/33.34 is valid; 50/40 is not.
    rule(
        &mut b,
        json!({"name": "Thirds", "conditions": [{"field": "payee", "op": "equals", "value": "t"}],
        "actions": [{"action": "split", "shares": [{"account": "Expenses:Groceries", "percent": "33.33"}, {"account": "Expenses:Household", "percent": "33.33"}, {"account": "Liabilities:Visa", "percent": "33.34"}]}]}),
    );
    assert_eq!(b.err("rule.create", json!({"name": "Bad", "conditions": [{"field": "payee", "op": "equals", "value": "t"}],
        "actions": [{"action": "split", "shares": [{"account": "Expenses:Groceries", "percent": "50"}, {"account": "Expenses:Household", "percent": "40"}]}]})).code(), "invalid_args");
}

#[test]
fn import_rules_at6_user_override_beats_rule() {
    let mut b = book();
    rule(
        &mut b,
        json!({"name": "Kroger", "conditions": [{"field": "payee", "op": "contains", "value": "KROGER"}],
        "actions": [{"action": "set_account", "account": "Expenses:Groceries"}]}),
    );
    let batch = b.stage_qif(
        "!Type:Bank\nD10/01/2026\nT-5.00\nPKROGER 1\n^\n",
        "Assets:Checking",
    );
    let out = b.run("import.set_row", json!({"batch": batch["id"], "row": batch["rows"][0]["id"], "other_account": "Expenses:Household"}));
    assert_eq!(
        out["rows"][0]["other"][0]["account_name"],
        "Expenses:Household"
    );
    assert_eq!(out["rows"][0]["other_source"], "user");
}

#[test]
fn import_rules_at7_make_rule_learns() {
    let mut b = book();
    let batch = b.stage_qif(
        "!Type:Bank\nD10/01/2026\nT-5.00\nPCorner Bakery\n^\n",
        "Assets:Checking",
    );
    b.run("import.set_row", json!({"batch": batch["id"], "row": batch["rows"][0]["id"], "other_account": "Expenses:Groceries", "make_rule": true}));
    let rules = b.run("rule.list", json!({}));
    assert_eq!(rules.as_array().unwrap().len(), 1);
    assert_eq!(rules[0]["conditions"][0]["value"], "CORNER BAKERY");
    b.commit(&batch);
    let row = first_row(&mut b, "corner bakery", "-6.00");
    assert_eq!(row["other"][0]["account_name"], "Expenses:Groceries");
    assert_eq!(row["other_source"], "rule");
    // An identical rule is not created twice.
    let batch = b.stage_qif(
        "!Type:Bank\nD10/02/2026\nT-7.00\nPCorner Bakery\n^\n",
        "Assets:Checking",
    );
    b.run("import.set_row", json!({"batch": batch["id"], "row": batch["rows"][0]["id"], "other_account": "Expenses:Groceries", "make_rule": true}));
    assert_eq!(b.run("rule.list", json!({})).as_array().unwrap().len(), 1);
}

#[test]
fn import_rules_at8_backreference_rejected() {
    let mut b = book();
    let e = b.err(
        "rule.create",
        json!({"name": "Bad", "conditions": [{"field": "payee", "op": "regex", "value": r"(ab)\1"}],
        "actions": [{"action": "set_account", "account": "Expenses:Groceries"}]}),
    );
    assert_eq!(e.code(), "invalid_args");
    assert!(e.to_string().contains("regex"), "{e}");
    let ok = rule(
        &mut b,
        json!({"name": "Re", "conditions": [{"field": "payee", "op": "regex", "value": r"^SHELL\s+\d+"}],
        "actions": [{"action": "set_account", "account": "Expenses:Groceries"}]}),
    );
    assert_eq!(b.err("rule.update", json!({"id": ok["id"], "conditions": [{"field": "memo", "op": "regex", "value": "(?=x)"}]})).code(), "invalid_args");
    let t = b.run(
        "rule.test",
        json!({"account": "Assets:Checking", "amount": "-1", "payee": "shell 42 main st"}),
    );
    assert_eq!(t["rule_ids"], json!([ok["id"]]));
}

#[test]
fn import_rules_at9_rule_test_changes_nothing() {
    let mut b = book();
    let r = rule(
        &mut b,
        json!({"name": "Kroger", "conditions": [{"field": "payee", "op": "contains", "value": "KROGER"}],
        "actions": [{"action": "set_account", "account": "Expenses:Groceries"}, {"action": "set_memo", "memo": "food"}]}),
    );
    let n = b.audit_len();
    let before = b.state();
    let t = b.run(
        "rule.test",
        json!({"account": "Assets:Checking", "amount": "-9.99", "payee": "Kroger 5"}),
    );
    assert_eq!(t["rule_ids"], json!([r["id"]]));
    assert_eq!(t["other"][0]["account_name"], "Expenses:Groceries");
    assert_eq!(t["memo"], "food");
    assert_eq!(b.audit_len(), n);
    assert_eq!(b.state(), before);
}

#[test]
fn rules_management_and_archived_account_warning() {
    let mut b = book();
    let a = rule(
        &mut b,
        json!({"name": "A", "conditions": [{"field": "payee", "op": "contains", "value": "x"}], "actions": [{"action": "skip"}]}),
    );
    let c = rule(
        &mut b,
        json!({"name": "C", "conditions": [{"field": "payee", "op": "contains", "value": "y"}],
        "actions": [{"action": "set_account", "account": "Expenses:Household"}]}),
    );
    let list = b.run("rule.reorder", json!({"ids": [c["id"], a["id"]]}));
    assert_eq!(list[0]["id"], c["id"]);
    assert_eq!(
        b.err("rule.reorder", json!({"ids": [c["id"]]})).code(),
        "invalid_args"
    );
    b.run("history.undo", json!({}));
    assert_eq!(
        b.run("rule.list", json!({}))[0]["id"],
        a["id"],
        "reorder is undoable"
    );
    // A rule whose account is archived is skipped with a warning.
    b.run("account.archive", json!({"account": "Expenses:Household"}));
    let row = first_row(&mut b, "y", "-1.00");
    assert_eq!(row["other"][0]["account_name"], "Uncategorized Expense");
    assert!(row["warnings"][0].as_str().unwrap().contains("skipped"));
    b.run("rule.delete", json!({"id": a["id"]}));
    assert_eq!(b.run("rule.list", json!({})).as_array().unwrap().len(), 1);
    // A skip rule proposes skip.
    let mut d = book();
    rule(
        &mut d,
        json!({"name": "S", "conditions": [{"field": "memo", "op": "equals", "value": "pending"}], "actions": [{"action": "skip"}]}),
    );
    let row = d.stage_qif(
        "!Type:Bank\nD10/01/2026\nT-1.00\nPA\nMPending\n^\n",
        "Assets:Checking",
    )["rows"][0]
        .clone();
    assert_eq!(row["action"], "skip");
}
