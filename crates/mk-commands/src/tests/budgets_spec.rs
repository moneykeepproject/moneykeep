//! budgets.md acceptance tests.

use super::*;

fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "budgets"}));
    for (name, ty, parent) in [
        ("Salary", "income", None),
        ("Food", "expense", Some("Expenses")),
        ("Dining", "expense", Some("Expenses:Food")),
        ("Gifts", "expense", Some("Expenses")),
        ("Fuel", "expense", Some("Expenses")),
        ("Utilities", "expense", Some("Expenses")),
    ] {
        let mut args = json!({"name": name, "type": ty});
        if let Some(p) = parent {
            args["parent"] = json!(p);
        }
        b.account(args);
    }
    b.run(
        "budget.create",
        json!({"name": "Household", "start_month": "2026-01"}),
    );
    b
}

fn spend(b: &mut Book, date: &str, category: &str, amount: &str) {
    b.transfer(date, category, "Assets:Checking", amount);
}

fn refund(b: &mut Book, date: &str, category: &str, amount: &str) {
    b.transfer(date, "Assets:Checking", category, amount);
}

fn row<'a>(r: &'a Value, section: &str, name: &str) -> &'a Value {
    r[section]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["account"]["full_name"] == name)
        .unwrap_or_else(|| panic!("no row {name} in {r}"))
}

#[test]
fn budgets_at1_actual_remaining_percent() {
    let mut b = book();
    b.run(
        "budget.set_line",
        json!({"account": "Expenses:Groceries", "amount": "400.00"}),
    );
    spend(&mut b, "2026-10-03", "Expenses:Groceries", "120.00");
    spend(&mut b, "2026-10-12", "Expenses:Groceries", "250.00");
    refund(&mut b, "2026-10-15", "Expenses:Groceries", "20.00");
    let r = b.run("budget.report", json!({"month": "2026-10"}));
    let g = row(&r, "expenses", "Expenses:Groceries");
    assert_eq!(g["budget"], "400.00");
    assert_eq!(g["actual"], "350.00");
    assert_eq!(g["remaining"], "50.00");
    assert_eq!(g["percent"], "87.5");
    // A voided purchase does not count.
    let t = b.transfer(
        "2026-10-20",
        "Expenses:Groceries",
        "Assets:Checking",
        "10.00",
    );
    b.run("txn.void", json!({"id": t}));
    let r = b.run("budget.report", json!({"month": "2026-10"}));
    assert_eq!(
        row(&r, "expenses", "Expenses:Groceries")["actual"],
        "350.00"
    );
}

#[test]
fn budgets_at2_rollover() {
    let mut b = book();
    b.run("budget.update", json!({"start_month": "2026-09"}));
    b.run(
        "budget.set_line",
        json!({"account": "Expenses:Groceries", "amount": "400.00", "rollover": true}),
    );
    spend(&mut b, "2026-09-10", "Expenses:Groceries", "450.00");
    spend(&mut b, "2026-10-10", "Expenses:Groceries", "350.00");
    let r = b.run("budget.report", json!({"month": "2026-10"}));
    let g = row(&r, "expenses", "Expenses:Groceries");
    assert_eq!(g["available"], "350.00");
    assert_eq!(g["remaining"], "0.00");
    assert_eq!(g["carried"], "-50.00");
    assert_eq!(g["rollover"], true);
    // September itself starts with nothing carried in.
    let r = b.run("budget.report", json!({"month": "2026-09"}));
    assert_eq!(
        row(&r, "expenses", "Expenses:Groceries")["available"],
        "400.00"
    );
}

#[test]
fn budgets_at3_overrides() {
    let mut b = book();
    b.run(
        "budget.set_line",
        json!({"account": "Expenses:Gifts", "amount": "50.00"}),
    );
    b.run(
        "budget.set_month",
        json!({"account": "Expenses:Gifts", "month": "2026-12", "amount": "300.00"}),
    );
    let dec = b.run("budget.report", json!({"month": "2026-12"}));
    assert_eq!(row(&dec, "expenses", "Expenses:Gifts")["budget"], "300.00");
    let nov = b.run("budget.report", json!({"month": "2026-11"}));
    assert_eq!(row(&nov, "expenses", "Expenses:Gifts")["budget"], "50.00");
    let g = b.run(
        "budget.set_month",
        json!({"account": "Expenses:Gifts", "month": "2026-12", "amount": "50.00"}),
    );
    assert_eq!(g["lines"][0]["overrides"], json!({}));
}

#[test]
fn budgets_at4_parent_child_attribution() {
    let mut b = book();
    b.run(
        "budget.set_line",
        json!({"account": "Expenses:Food", "amount": "500.00"}),
    );
    b.run(
        "budget.set_line",
        json!({"account": "Expenses:Food:Dining", "amount": "100.00"}),
    );
    // Groceries under Food for this test.
    b.run(
        "account.move",
        json!({"account": "Expenses:Groceries", "parent": "Expenses:Food"}),
    );
    spend(&mut b, "2026-10-03", "Expenses:Food:Groceries", "200.00");
    spend(&mut b, "2026-10-04", "Expenses:Food:Dining", "40.00");
    let r = b.run("budget.report", json!({"month": "2026-10"}));
    assert_eq!(row(&r, "expenses", "Expenses:Food")["actual"], "200.00");
    assert_eq!(
        row(&r, "expenses", "Expenses:Food:Dining")["actual"],
        "40.00"
    );
    // Removing the Dining line makes its actuals count on Food.
    b.run(
        "budget.remove_line",
        json!({"account": "Expenses:Food:Dining"}),
    );
    let r = b.run("budget.report", json!({"month": "2026-10"}));
    assert_eq!(row(&r, "expenses", "Expenses:Food")["actual"], "240.00");
    assert!(r["unbudgeted"].as_array().unwrap().is_empty());
}

#[test]
fn budgets_at5_income_and_net() {
    let mut b = book();
    b.run(
        "budget.set_line",
        json!({"account": "Salary", "amount": "5000.00"}),
    );
    b.run(
        "budget.set_line",
        json!({"account": "Expenses:Groceries", "amount": "400.00"}),
    );
    b.transfer("2026-10-01", "Assets:Checking", "Salary", "2500.00");
    let r = b.run("budget.report", json!({"month": "2026-10"}));
    let s = row(&r, "income", "Salary");
    assert_eq!(s["actual"], "2500.00");
    assert_eq!(s["remaining"], "2500.00");
    assert_eq!(r["net"]["budget"], "4600.00");
    assert_eq!(r["income"]["totals"]["budget"], "5000.00");
    assert_eq!(r["expenses"]["totals"]["budget"], "400.00");
}

#[test]
fn budgets_at6_suggest() {
    let mut b = book();
    spend(&mut b, "2026-07-05", "Expenses:Fuel", "40.00");
    spend(&mut b, "2026-09-05", "Expenses:Fuel", "61.50");
    let avg = b.run(
        "budget.suggest",
        json!({"from_month": "2026-07", "to_month": "2026-09"}),
    );
    let fuel = avg["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["full_name"] == "Expenses:Fuel")
        .unwrap()
        .clone();
    assert_eq!(fuel["amount"], "34.00");
    let last = b.run(
        "budget.suggest",
        json!({"from_month": "2026-07", "to_month": "2026-09", "method": "last"}),
    );
    assert_eq!(last["lines"][0]["amount"], "62.00");
    // Default range: the 12 months before the current month.
    let d = b.run("budget.suggest", json!({"today": "2026-10-19"}));
    assert_eq!(d["from_month"], "2025-10");
    assert_eq!(d["to_month"], "2026-09");
}

#[test]
fn budgets_at7_unbudgeted() {
    let mut b = book();
    b.run(
        "budget.set_line",
        json!({"account": "Expenses:Groceries", "amount": "400.00"}),
    );
    spend(&mut b, "2026-10-03", "Expenses:Groceries", "100.00");
    spend(&mut b, "2026-10-04", "Expenses:Utilities", "80.00");
    let r = b.run("budget.report", json!({"month": "2026-10"}));
    assert_eq!(
        r["unbudgeted"],
        json!([{"account": r["unbudgeted"][0]["account"], "full_name": "Expenses:Utilities", "type": "expense", "actual": "80.00"}])
    );
    assert_eq!(r["unbudgeted_total"]["expenses"], "80.00");
    assert_eq!(r["expenses"]["totals"]["actual"], "100.00");
}

#[test]
fn budgets_at8_module_gate_keeps_data() {
    let mut b = book();
    b.run(
        "budget.set_line",
        json!({"account": "Expenses:Groceries", "amount": "400.00"}),
    );
    let before = b.run("budget.get", json!({}));
    b.run("module.disable", json!({"module": "budgets"}));
    assert_eq!(
        b.err("budget.report", json!({"month": "2026-10"})).code(),
        "module_disabled"
    );
    b.run("module.enable", json!({"module": "budgets"}));
    assert_eq!(b.run("budget.get", json!({})), before);
}

#[test]
fn budgets_at9_range_sums() {
    let mut b = book();
    b.run(
        "budget.set_line",
        json!({"account": "Expenses:Groceries", "amount": "100.00"}),
    );
    let r = b.run(
        "budget.report",
        json!({"from_month": "2026-01", "to_month": "2026-03"}),
    );
    assert_eq!(
        row(&r, "expenses", "Expenses:Groceries")["budget"],
        "300.00"
    );
}

#[test]
fn budget_management() {
    let mut b = book();
    // Names are unique, case-insensitively.
    assert_eq!(
        b.err("budget.create", json!({"name": "household"})).code(),
        "rejected"
    );
    // The first budget is the default; making another the default clears it.
    let second = b.run(
        "budget.create",
        json!({"name": "Lean", "start_month": "2026-01"}),
    );
    assert_eq!(second["default"], false);
    b.run("budget.update", json!({"budget": "Lean", "default": true}));
    let list = b.run("budget.list", json!({}));
    let defaults: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["default"] == true)
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(defaults, ["Lean"]);
    // Lines only for income and expense accounts.
    assert_eq!(
        b.err(
            "budget.set_line",
            json!({"account": "Assets:Checking", "amount": "1.00"})
        )
        .code(),
        "rejected"
    );
    assert_eq!(
        b.err(
            "budget.set_line",
            json!({"account": "Expenses:Fuel", "amount": "-1.00"})
        )
        .code(),
        "invalid_args"
    );
    // apply_suggestion: one audit entry for several lines.
    let before = b.run("history.list", json!({})).as_array().unwrap().len();
    b.run(
        "budget.apply_suggestion",
        json!({"lines": [
            {"account": "Expenses:Fuel", "amount": "34.00", "full_name": "Expenses:Fuel"},
            {"account": "Expenses:Gifts", "amount": "20.00"}]}),
    );
    assert_eq!(
        b.run("history.list", json!({})).as_array().unwrap().len(),
        before + 1
    );
    assert_eq!(
        b.run("budget.get", json!({}))["lines"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    b.run("history.undo", json!({}));
    assert_eq!(
        b.run("budget.get", json!({}))["lines"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    // Deleting the default makes the remaining budget the default.
    b.run("budget.delete", json!({"budget": "Lean"}));
    assert_eq!(b.run("budget.get", json!({}))["name"], "Household");
    // A month before the start: budget 0 with a note; actuals still shown.
    b.run(
        "budget.set_line",
        json!({"budget": "Household", "account": "Expenses:Groceries", "amount": "100.00"}),
    );
    spend(&mut b, "2025-12-03", "Expenses:Groceries", "9.00");
    let r = b.run("budget.report", json!({"month": "2025-12"}));
    let g = row(&r, "expenses", "Expenses:Groceries");
    assert_eq!(g["budget"], "0.00");
    assert_eq!(g["actual"], "9.00");
    assert_eq!(g["note"], "This budget starts in 2026-01");
}
