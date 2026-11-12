//! reports.md acceptance tests, on the shared test book described there.

use super::*;

/// The reports.md test book. Returns it with the Dining purchase's id.
fn test_book() -> (Book, String) {
    let mut b = Book::new();
    b.account(json!({"name": "Checking", "type": "asset", "subtype": "bank"}));
    b.account(json!({"name": "Visa", "type": "liability", "subtype": "credit-card"}));
    for name in ["Groceries", "Dining", "Household", "Fuel"] {
        b.account(json!({"name": name, "type": "expense"}));
    }
    b.account(json!({"name": "Salary", "type": "income"}));
    b.transfer("2026-09-30", "Checking", "Opening Balances", "1000.00");
    b.transfer("2026-09-15", "Fuel", "Checking", "41.07");
    b.transfer("2026-10-01", "Checking", "Salary", "2500.00");
    b.transfer("2026-10-03", "Groceries", "Checking", "63.41");
    let dining = b.transfer("2026-10-05", "Dining", "Visa", "23.65");
    b.transfer("2026-10-20", "Visa", "Checking", "23.65");
    b.transfer("2026-10-22", "Visa", "Groceries", "10.00");
    b.run(
        "txn.create",
        json!({"date": "2026-10-25", "payee": "MegaMart", "splits": [
            {"account": "Groceries", "value": "80.00"},
            {"account": "Household", "value": "20.00"},
            {"account": "Visa", "value": "-100.00"}]}),
    );
    (b, dining)
}

fn october() -> Value {
    json!({"from": "2026-10-01", "to": "2026-10-31"})
}

fn amounts(rows: &Value) -> Vec<(String, String)> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["full_name"]
                    .as_str()
                    .or(r["name"].as_str())
                    .unwrap()
                    .to_owned(),
                r["amount"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

#[test]
fn reports_at1_spending() {
    let (mut b, _) = test_book();
    let r = b.run("report.spending", october());
    assert_eq!(
        amounts(&r["rows"]),
        pairs(&[
            ("Groceries", "133.41"),
            ("Dining", "23.65"),
            ("Household", "20.00")
        ])
    );
    assert_eq!(r["total"], "177.06");
    assert_eq!(r["rows"][0]["percent"], "75.3");
    // Only the share paid from the selected money accounts.
    let r = b.run(
        "report.spending",
        json!({"from": "2026-10-01", "to": "2026-10-31", "accounts": ["Checking"]}),
    );
    assert_eq!(amounts(&r["rows"]), pairs(&[("Groceries", "63.41")]));
}

#[test]
fn reports_at2_income_expense() {
    let (mut b, _) = test_book();
    let r = b.run(
        "report.income_expense",
        json!({"from": "2026-09-01", "to": "2026-10-31", "interval": "month"}),
    );
    let labels: Vec<&str> = r["periods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["2026-09", "2026-10"]);
    let salary = &r["income"]["rows"][0];
    assert_eq!(salary["full_name"], "Salary");
    assert_eq!(salary["amounts"], json!(["0.00", "2500.00"]));
    assert_eq!(r["expenses"]["totals"], json!(["41.07", "177.06"]));
    assert_eq!(r["net"]["amounts"], json!(["-41.07", "2322.94"]));
    assert_eq!(r["net"]["total"], "2281.87");
    // Transfers and card payments contribute nothing: every expense row is a category.
    let names: Vec<&str> = r["expenses"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["full_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Dining", "Fuel", "Groceries", "Household"]);
}

#[test]
fn reports_at3_net_worth() {
    let (mut b, _) = test_book();
    let r = b.run(
        "report.net_worth",
        json!({"from": "2026-09-01", "to": "2026-10-31", "interval": "month", "today": "2026-11-05"}),
    );
    assert_eq!(r["dates"], json!(["2026-09-30", "2026-10-31"]));
    assert_eq!(r["assets"]["totals"], json!(["958.93", "3371.87"]));
    assert_eq!(r["liabilities"]["totals"], json!(["0.00", "90.00"]));
    assert_eq!(r["net_worth"], json!(["958.93", "3281.87"]));
    assert_eq!(r["change"], "2322.94");
    assert_eq!(r["assets"]["rows"][0]["full_name"], "Checking");
    // Equity, income and expense accounts are not listed.
    let all: Vec<&str> = r["assets"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .chain(r["liabilities"]["rows"].as_array().unwrap())
        .map(|x| x["full_name"].as_str().unwrap())
        .collect();
    assert_eq!(all, ["Checking", "Visa"]);
}

#[test]
fn reports_at4_cash_flow() {
    let (mut b, _) = test_book();
    let r = b.run("report.cash_flow", october());
    assert_eq!(r["opening"], "958.93");
    assert_eq!(
        amounts(&r["inflows"]["rows"]),
        pairs(&[("Salary", "2500.00"), ("Groceries", "10.00")])
    );
    assert_eq!(r["inflows"]["total"], "2510.00");
    assert_eq!(
        amounts(&r["outflows"]["rows"]),
        pairs(&[
            ("Groceries", "143.41"),
            ("Dining", "23.65"),
            ("Household", "20.00")
        ])
    );
    assert_eq!(r["outflows"]["total"], "187.06");
    assert_eq!(r["internal"], "23.65");
    assert_eq!(r["closing"], "3281.87");
    assert_eq!(r["difference"], "0.00");
    // Transfers to a non-selected account are named as such.
    let r = b.run(
        "report.cash_flow",
        json!({"from": "2026-10-01", "to": "2026-10-31", "accounts": ["Checking"]}),
    );
    assert!(
        amounts(&r["outflows"]["rows"])
            .contains(&("Transfer: Visa".to_owned(), "23.65".to_owned()))
    );
    assert_eq!(r["difference"], "0.00");
    // With an interval, each period also balances.
    let r = b.run(
        "report.cash_flow",
        json!({"from": "2026-09-01", "to": "2026-10-31", "interval": "month"}),
    );
    for p in r["periods"].as_array().unwrap() {
        assert_eq!(p["difference"], "0.00");
    }
}

#[test]
fn reports_at5_export_csv() {
    let (mut b, _) = test_book();
    let e = b.run(
        "report.export",
        json!({"report": "income_expense", "format": "csv",
               "args": {"from": "2026-09-01", "to": "2026-10-31", "interval": "month"}}),
    );
    assert_eq!(
        e["file_name"],
        "income-expense-2026-09-01-to-2026-10-31.csv"
    );
    let content = e["content"].as_str().unwrap();
    let lines: Vec<&str> = content.split("\r\n").collect();
    assert_eq!(lines[0], "Category,2026-09,2026-10,Total");
    assert!(lines.contains(&"Salary,0.00,2500.00,2500.00"), "{content}");
    assert!(lines.contains(&"Income,,,"));
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Total expenses,41.07,177.06,218.13"))
    );
    assert!(content.ends_with("\r\n"));
    assert!(!content.replace("\r\n", "").contains('\n'));
    // Every report exports.
    for (name, args) in [
        ("spending", october()),
        ("net_worth", october()),
        ("cash_flow", october()),
        ("payees", october()),
    ] {
        let e = b.run("report.export", json!({"report": name, "args": args}));
        assert!(e["content"].as_str().unwrap().ends_with("\r\n"), "{name}");
    }
    // Quoting.
    b.run(
        "txn.create",
        json!({"date": "2026-10-26", "payee": "Smith, \"Bob\"", "splits": [
            {"account": "Household", "value": "5.00"}, {"account": "Checking", "value": "-5.00"}]}),
    );
    let e = b.run(
        "report.export",
        json!({"report": "payees", "args": october()}),
    );
    assert!(
        e["content"]
            .as_str()
            .unwrap()
            .contains("\"Smith, \"\"Bob\"\"\",1,5.00,0.00")
    );
}

#[test]
fn reports_at6_voided_leaves_every_report() {
    let (mut b, dining) = test_book();
    b.run("txn.void", json!({"id": dining}));
    let s = b.run("report.spending", october());
    assert!(!amounts(&s["rows"]).iter().any(|(n, _)| n == "Dining"));
    assert_eq!(s["total"], "153.41");
    let ie = b.run("report.income_expense", october());
    assert_eq!(ie["expenses"]["totals"], json!(["153.41"]));
    let nw = b.run(
        "report.net_worth",
        json!({"from": "2026-10-31", "to": "2026-10-31", "today": "2026-11-05"}),
    );
    assert_eq!(nw["liabilities"]["totals"], json!(["66.35"]));
    let cf = b.run("report.cash_flow", october());
    assert!(
        !amounts(&cf["outflows"]["rows"])
            .iter()
            .any(|(n, _)| n == "Dining")
    );
    assert_eq!(cf["difference"], "0.00");
    let p = b.run("report.payees", october());
    assert!(
        p["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["spent"] != "23.65" || r["count"] != 1)
    );
}

#[test]
fn reports_at7_presets() {
    let (mut b, _) = test_book();
    let r = b.run(
        "report.spending",
        json!({"preset": "last_month", "today": "2026-11-05"}),
    );
    assert_eq!(
        (r["from"].as_str(), r["to"].as_str()),
        (Some("2026-10-01"), Some("2026-10-31"))
    );
    for (preset, from, to) in [
        ("this_month", "2026-11-01", "2026-11-30"),
        ("this_quarter", "2026-10-01", "2026-12-31"),
        ("this_year", "2026-01-01", "2026-12-31"),
        ("last_year", "2025-01-01", "2025-12-31"),
        ("last_12_months", "2025-11-01", "2026-10-31"),
        ("year_to_date", "2026-01-01", "2026-11-05"),
        ("all", "2026-09-15", "2026-11-05"),
    ] {
        let r = b.run(
            "report.spending",
            json!({"preset": preset, "today": "2026-11-05"}),
        );
        assert_eq!(
            (r["from"].as_str(), r["to"].as_str()),
            (Some(from), Some(to)),
            "{preset}"
        );
    }
}

#[test]
fn reports_edge_cases() {
    // Empty book: zero totals, no error.
    let mut e = Book::new();
    let r = e.run("report.spending", october());
    assert_eq!(r["total"], "0.00");
    let r = e.run("report.cash_flow", october());
    assert_eq!(r["difference"], "0.00");
    let r = e.run("report.income_expense", october());
    assert_eq!(r["net"]["total"], "0.00");
    // A paycheck with taxes withheld.
    let mut b = Book::new();
    b.account(json!({"name": "Checking", "type": "asset", "subtype": "bank"}));
    b.account(json!({"name": "Salary", "type": "income"}));
    b.account(json!({"name": "Taxes", "type": "expense"}));
    b.run(
        "txn.create",
        json!({"date": "2026-10-01", "splits": [
            {"account": "Checking", "value": "2400.00"},
            {"account": "Taxes", "value": "600.00"},
            {"account": "Salary", "value": "-3000.00"}]}),
    );
    let ie = b.run("report.income_expense", october());
    assert_eq!(ie["income"]["total"], "3000.00");
    assert_eq!(ie["expenses"]["total"], "600.00");
    let cf = b.run("report.cash_flow", october());
    assert_eq!(
        amounts(&cf["inflows"]["rows"]),
        pairs(&[("Salary", "3000.00")])
    );
    assert_eq!(
        amounts(&cf["outflows"]["rows"]),
        pairs(&[("Taxes", "600.00")])
    );
    // Depth rolls up to the ancestor.
    let mut d = Book::new();
    d.account(json!({"name": "Checking", "type": "asset", "subtype": "bank"}));
    d.account(json!({"name": "Food", "type": "expense"}));
    d.account(json!({"name": "Dining", "type": "expense", "parent": "Food"}));
    d.account(json!({"name": "Lunch", "type": "expense", "parent": "Food:Dining"}));
    d.transfer("2026-10-02", "Food:Dining:Lunch", "Checking", "12.00");
    let r = d.run(
        "report.spending",
        json!({"from": "2026-10-01", "to": "2026-10-31", "depth": 2}),
    );
    assert_eq!(amounts(&r["rows"]), pairs(&[("Food:Dining", "12.00")]));
    let r = d.run(
        "report.spending",
        json!({"from": "2026-10-01", "to": "2026-10-31", "depth": 1}),
    );
    assert_eq!(amounts(&r["rows"]), pairs(&[("Food", "12.00")]));
    // A transaction on `to` is included; one on to + 1 is not.
    d.transfer("2026-10-31", "Food", "Checking", "1.00");
    d.transfer("2026-11-01", "Food", "Checking", "2.00");
    let r = d.run(
        "report.spending",
        json!({"from": "2026-10-01", "to": "2026-10-31", "depth": 1}),
    );
    assert_eq!(r["total"], "13.00");
}

#[test]
fn reports_net_worth_stops_at_today() {
    // app-desktop.md AT45 / reports.md revision 2.
    let (mut b, _) = test_book();
    let r = b.run(
        "report.net_worth",
        json!({"from": "2026-01-01", "to": "2026-12-31", "today": "2026-10-19", "interval": "month"}),
    );
    let dates: Vec<&str> = r["dates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert_eq!(dates.len(), 10);
    assert_eq!(&dates[8..], ["2026-09-30", "2026-10-19"]);
    assert_eq!(r["periods"][9]["label"], "2026-10-19");
    assert_eq!(r["net_worth"].as_array().unwrap().len(), 10);
    // A range entirely in the past is unchanged.
    let r = b.run(
        "report.net_worth",
        json!({"from": "2026-09-01", "to": "2026-10-31", "today": "2026-11-05"}),
    );
    assert_eq!(r["dates"], json!(["2026-09-30", "2026-10-31"]));
    // Today on a period end: no duplicate.
    let r = b.run(
        "report.net_worth",
        json!({"from": "2026-09-01", "to": "2026-12-31", "today": "2026-10-31"}),
    );
    assert_eq!(r["dates"], json!(["2026-09-30", "2026-10-31"]));
}
