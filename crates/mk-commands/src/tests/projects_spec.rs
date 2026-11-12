//! projects.md acceptance tests PJ1–PJ11 at the command level, plus the edge cases
//! the spec names (transfers not counted, bulk replace confirm, module off hides,
//! budget mode switching, CSV/PDF export, the dashboard block).

use super::*;

/// A book with the projects module on, a checking account, a card and project
/// expense accounts.
fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "projects"}));
    for n in ["Materials", "Labour", "Supplies"] {
        b.account(json!({"name": n, "type": "expense", "parent": "Expenses"}));
    }
    b.account(json!({"name": "Income", "type": "income", "placeholder": true}));
    b.account(json!({"name": "Gifts", "type": "income", "parent": "Income"}));
    b
}

fn full(account: &str) -> String {
    match account {
        "Checking" => "Assets:Checking".into(),
        "Visa" => "Liabilities:Visa".into(),
        x => format!("Expenses:{x}"),
    }
}

/// An expense of `amount` (negative: a refund) from `money` to `cat`, tagged.
fn spend(
    b: &mut Book,
    date: &str,
    payee: &str,
    amount: &str,
    money: &str,
    cat: &str,
    project: Option<&str>,
) -> String {
    let neg = if let Some(x) = amount.strip_prefix('-') {
        x.to_owned()
    } else {
        format!("-{amount}")
    };
    let mut split = json!({"account": full(cat), "value": amount});
    if let Some(p) = project {
        split["project"] = json!(p);
    }
    b.run(
        "txn.create",
        json!({"date": date, "payee": payee, "splits": [split, {"account": full(money), "value": neg}]}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn row(b: &mut Book, name: &str) -> Value {
    b.run("project.list", json!({}))["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("no {name}"))
        .clone()
}

fn project_alerts(b: &mut Book) -> Vec<Value> {
    b.run("project.alerts", json!({}))["alerts"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn projects_off_by_default_and_on_hides_nothing() {
    let mut b = Book::new();
    b.standard();
    let m = b.run("module.list", json!({}));
    let p = m
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "projects")
        .unwrap()
        .clone();
    assert_eq!(p["enabled"], false);
    assert_eq!(b.err("project.list", json!({})).code(), "module_disabled");
    // A project tag needs the module.
    let e = b.err(
        "txn.create",
        json!({"date": "2026-05-01", "splits": [
            {"account": "Expenses:Groceries", "value": "5.00", "project": "Roof"},
            {"account": "Assets:Checking", "value": "-5.00"}]}),
    );
    assert_eq!(e.code(), "invalid_args");
}

#[test]
fn projects_pj1_budget_spent_and_alert() {
    let mut b = book();
    let p = b.run(
        "project.create",
        json!({"name": "Roof repair", "budget": "5,000.00"}),
    );
    assert_eq!(p["status"], "active");
    assert_eq!(p["budget"], "5000.00");
    spend(
        &mut b,
        "2026-05-02",
        "Home Depot",
        "1200.00",
        "Visa",
        "Materials",
        Some("Roof repair"),
    );
    spend(
        &mut b,
        "2026-05-10",
        "A1 Roofing",
        "3100.00",
        "Checking",
        "Labour",
        Some("Roof repair"),
    );
    spend(
        &mut b,
        "2026-05-12",
        "Home Depot",
        "-150.00",
        "Visa",
        "Materials",
        Some("Roof repair"),
    );
    // An untagged expense does not count.
    spend(
        &mut b,
        "2026-05-12",
        "Grocer",
        "80.00",
        "Checking",
        "Supplies",
        None,
    );
    let r = row(&mut b, "Roof repair");
    assert_eq!(r["spent"], "4150.00");
    assert_eq!(r["percent"], "83.0");
    assert_eq!(r["remaining"], "850.00");
    assert_eq!(r["level_of_use"], "ok");
    assert!(project_alerts(&mut b).is_empty());
    spend(
        &mut b,
        "2026-05-20",
        "Lowes",
        "500.00",
        "Visa",
        "Materials",
        Some("Roof repair"),
    );
    let r = row(&mut b, "Roof repair");
    assert_eq!(r["percent"], "93.0");
    assert_eq!(r["level_of_use"], "warn");
    let a = project_alerts(&mut b);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0]["label"], "Roof repair has used 93 % of its budget");
    // It is on Home → Attention, dismissible, and undoable.
    let att = b.run(
        "dashboard.block_data",
        json!({"dashboard": "home", "block": "attention"}),
    );
    let item = att["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "project_budget_warning")
        .unwrap()
        .clone();
    assert_eq!(item["target"]["screen"], "projects");
    b.run(
        "project.dismiss_alert",
        json!({"key": item["dismiss"]["key"]}),
    );
    assert!(project_alerts(&mut b).is_empty());
    // Over budget: a new alert (once per crossing).
    spend(
        &mut b,
        "2026-05-25",
        "A1 Roofing",
        "690.00",
        "Checking",
        "Labour",
        Some("Roof repair"),
    );
    let a = project_alerts(&mut b);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0]["label"], "Roof repair is over budget by 340.00");
    b.run("history.undo", json!({}));
    b.run("history.undo", json!({}));
    assert_eq!(project_alerts(&mut b).len(), 1);
    // The settings: warning percentage.
    b.run(
        "module.set_setting",
        json!({"key": "projects.warn_percent", "value": 95}),
    );
    assert!(project_alerts(&mut b).is_empty());
}

#[test]
fn projects_pj2_sub_projects() {
    let mut b = book();
    b.run("project.create", json!({"name": "House"}));
    b.run(
        "project.create",
        json!({"name": "House:Roof", "budget": "1000"}),
    );
    b.run(
        "project.create",
        json!({"name": "House:Gutters", "budget": "500"}),
    );
    spend(
        &mut b,
        "2026-06-01",
        "Roofer",
        "700.00",
        "Checking",
        "Labour",
        Some("House:Roof"),
    );
    spend(
        &mut b,
        "2026-06-02",
        "Gutter Co",
        "200.00",
        "Checking",
        "Labour",
        Some("House:Gutters"),
    );
    let h = row(&mut b, "House");
    assert_eq!(h["spent"], "900.00");
    // A parent without a budget of its own compares with its sub-projects'.
    assert_eq!(h["budget"], "1500.00");
    assert_eq!(row(&mut b, "House:Roof")["spent"], "700.00");
    let r = b.run("report.project", json!({"project": "House"}));
    assert_eq!(r["expenses"]["total"], "900.00");
    assert_eq!(r["sub_projects"], 2);
    // Totals add top-level projects only.
    let l = b.run("project.list", json!({}));
    assert_eq!(l["totals"]["spent"], "900.00");
    // A project with sub-projects cannot be deleted; a name is unique.
    assert_eq!(
        b.err("project.delete", json!({"project": "House"})).code(),
        "rejected"
    );
    assert_eq!(
        b.err("project.create", json!({"name": "house:roof"}))
            .code(),
        "rejected"
    );
}

#[test]
fn projects_pj3_mileage_separate_until_counted() {
    let mut b = book();
    b.run("module.enable", json!({"module": "mileage"}));
    b.run("vehicle.create", json!({"name": "2019 Honda CR-V"}));
    b.run("project.create", json!({"name": "Roof repair"}));
    b.run(
        "txn.create",
        json!({"date": "2026-07-03", "payee": "Parish", "splits": [
            {"account": "Income:Gifts", "value": "-1000.00", "project": "Roof repair"},
            {"account": "Assets:Checking", "value": "1000.00"}]}),
    );
    spend(
        &mut b,
        "2026-07-03",
        "Home Depot",
        "200.00",
        "Visa",
        "Materials",
        Some("Roof repair"),
    );
    let t = b.run(
        "trip.create",
        json!({"date": "2026-07-05", "purpose_kind": "business", "purpose": "Roof supplies",
               "from_place": "Office", "to_place": "Home Depot", "miles": "40",
               "project": "Roof repair"}),
    );
    assert_eq!(t["tag_names"]["project"], "Roof repair");
    let g = b.run("project.get", json!({"project": "Roof repair"}));
    let s = &g["summary"];
    assert_eq!(s["mileage"]["value"], "30.40");
    assert_eq!(s["mileage"]["miles"], "40");
    assert_eq!(s["net"], "800.00");
    assert_eq!(s["income"]["total"], "1000.00");
    let r = b.run(
        "report.project",
        json!({"project": "Roof repair", "count_mileage": true}),
    );
    assert_eq!(r["net"], "769.60");
    assert_eq!(r["cost"], "230.40");
    let r = b.run(
        "report.project",
        json!({"project": "Roof repair", "include_mileage": false}),
    );
    assert!(r["mileage"].is_null());
    let sum = b.run("report.projects_summary", json!({}));
    assert_eq!(sum["rows"][0]["miles"], "40");
}

#[test]
fn projects_pj4_bulk_set_project_one_command() {
    let mut b = book();
    b.run("project.create", json!({"name": "Roof repair"}));
    b.run("project.create", json!({"name": "Kitchen"}));
    let ids: Vec<String> = (1..=3)
        .map(|i| {
            spend(
                &mut b,
                &format!("2026-03-0{i}"),
                "Hardware Store",
                "40.00",
                "Visa",
                "Materials",
                None,
            )
        })
        .collect();
    let n = b.audit_len();
    let out = b.run(
        "txn.set_project",
        json!({"transactions": ids, "project": "Roof repair"}),
    );
    assert_eq!(out["transactions"], 3);
    assert_eq!(b.audit_len(), n + 1);
    assert_eq!(row(&mut b, "Roof repair")["spent"], "120.00");
    // Only the expense lines are tagged.
    let t = b.run("txn.get", json!({"id": ids[0]}));
    for s in t["splits"].as_array().unwrap() {
        let tagged = s["tag_names"]["project"] == "Roof repair";
        assert_eq!(tagged, s["account_name"] == "Expenses:Materials", "{s}");
    }
    // Replacing another project asks first (preview counts, then a conflict).
    let p = b.run(
        "txn.set_project",
        json!({"transactions": [ids[0]], "project": "Kitchen", "preview": true}),
    );
    assert_eq!(p["replaces"], 1);
    assert_eq!(b.audit_len(), n + 1);
    assert_eq!(
        b.err(
            "txn.set_project",
            json!({"transactions": [ids[0]], "project": "Kitchen"})
        )
        .code(),
        "conflict"
    );
    b.run(
        "txn.set_project",
        json!({"transactions": [ids[0]], "project": "Kitchen", "replace": true}),
    );
    assert_eq!(row(&mut b, "Kitchen")["spent"], "40.00");
    b.run("history.undo", json!({}));
    b.run("history.undo", json!({}));
    assert_eq!(row(&mut b, "Roof repair")["spent"], "0.00");
    assert_eq!(row(&mut b, "Kitchen")["spent"], "0.00");
}

#[test]
fn projects_pj5_profit_loss_by_project() {
    let mut b = book();
    b.run("project.create", json!({"name": "Roof repair"}));
    b.run("project.create", json!({"name": "Kitchen"}));
    spend(
        &mut b,
        "2026-04-01",
        "Lumber",
        "300.00",
        "Checking",
        "Materials",
        Some("Roof repair"),
    );
    spend(
        &mut b,
        "2026-04-02",
        "Tiles",
        "120.00",
        "Checking",
        "Materials",
        Some("Kitchen"),
    );
    spend(
        &mut b,
        "2026-04-03",
        "Grocer",
        "55.00",
        "Checking",
        "Supplies",
        None,
    );
    b.run(
        "txn.create",
        json!({"date": "2026-04-04", "splits": [
            {"account": "Income:Gifts", "value": "-500.00", "project": "Roof repair"},
            {"account": "Assets:Checking", "value": "500.00"}]}),
    );
    let dates = json!({"from": "2026-01-01", "to": "2026-12-31"});
    let mut args = dates.clone();
    let plain = b.run("report.profit_loss", args.clone());
    args["columns"] = json!("project");
    let by = b.run("report.profit_loss", args);
    let labels: Vec<&str> = by["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["Kitchen", "Roof repair", "Not set"]);
    assert_eq!(by["net_income"]["total"], plain["net_income"]["total"]);
    assert_eq!(
        by["net_income"]["amounts"],
        json!(["-120.00", "200.00", "-55.00"])
    );
    // CSV export of the project report and the summary.
    let csv = b.run(
        "report.export",
        json!({"report": "report.project", "format": "csv", "args": {"project": "Roof repair"}}),
    );
    assert!(
        csv["content"]
            .as_str()
            .unwrap()
            .contains("Expenses:Materials,300.00")
    );
    let csv = b.run(
        "report.export",
        json!({"report": "report.projects_summary", "format": "csv", "args": {}}),
    );
    assert!(
        csv["content"]
            .as_str()
            .unwrap()
            .starts_with("Project,Status,Budget")
    );
    let pdf = b.run(
        "report.export",
        json!({"report": "report.project", "format": "pdf", "args": {"project": "Roof repair"}}),
    );
    assert!(
        pdf["file_name"].as_str().unwrap().ends_with(".pdf"),
        "{pdf}"
    );
}

#[test]
fn projects_pj6_rule_sets_project() {
    let mut b = book();
    b.run("project.create", json!({"name": "Roof repair"}));
    b.run(
        "rule.create",
        json!({"name": "Lowes", "conditions": [{"field": "payee", "op": "contains", "value": "LOWES"}],
               "actions": [{"action": "set_account", "account": "Expenses:Materials"},
                           {"action": "set_project", "project": "Roof repair"}]}),
    );
    let qif =
        "!Type:Bank\nD03/15/2026\nT-84.20\nPLOWES #1234\n^\nD03/16/2026\nT-12.00\nPCOFFEE\n^\n";
    let batch = b.stage_qif(qif, "Assets:Checking");
    let rows = batch["rows"].as_array().unwrap();
    let lowes = rows
        .iter()
        .find(|r| r["payee"].as_str().unwrap_or_default().contains("LOWES"))
        .unwrap();
    assert!(lowes["tags"]["project"].as_str().is_some(), "{lowes}");
    b.commit(&batch);
    assert_eq!(row(&mut b, "Roof repair")["spent"], "84.20");
    // A rule naming a missing project is refused.
    let e = b.err(
        "rule.create",
        json!({"name": "X", "conditions": [{"field": "payee", "op": "contains", "value": "X"}],
               "actions": [{"action": "set_project", "project": "Nope"}]}),
    );
    assert_eq!(e.code(), "not_found");
}

#[test]
fn projects_pj7_phone_expense_and_closed_projects() {
    let mut b = book();
    b.run("module.enable", json!({"module": "expenses"}));
    b.run("project.create", json!({"name": "Roof repair"}));
    b.run("project.create", json!({"name": "Old job"}));
    b.run(
        "project.set_status",
        json!({"project": "Old job", "status": "completed"}),
    );
    let actor = Actor("test".into());
    let (lists, id) =
        b.s.write(|tx| -> CommandResult<_> {
            let ctx = Ctx::new(tx, &actor);
            let lists = crate::capture::lists_doc(&ctx)?;
            let f = mk_capture_api::ExpenseFields {
                date: Some("2026-09-01".into()),
                merchant: Some("Lowes".into()),
                amount: Some("45.20".into()),
                category: Some("Expenses:Materials".into()),
                paid_with: Some("Liabilities:Visa".into()),
                tags: mk_capture_api::Tags {
                    project: Some("Roof repair".into()),
                    ..Default::default()
                },
                ..Default::default()
            };
            let e = crate::expenses::create_from_phone(&ctx, &f, "2026-09-02".parse().unwrap())?;
            Ok((lists, e.id))
        })
        .unwrap();
    assert_eq!(lists.tags.project, ["Roof repair"]);
    let e = b.run("expense.get", json!({"expense": id}));
    assert_eq!(e["tag_names"]["project"], "Roof repair", "{e}");
}

#[test]
fn projects_pj8_delete_used_project_rejected() {
    let mut b = book();
    b.run("project.create", json!({"name": "Roof repair"}));
    b.run("project.create", json!({"name": "Spare"}));
    spend(
        &mut b,
        "2026-05-02",
        "Home Depot",
        "10.00",
        "Visa",
        "Materials",
        Some("Roof repair"),
    );
    spend(
        &mut b,
        "2026-05-03",
        "Home Depot",
        "12.00",
        "Visa",
        "Materials",
        Some("Roof repair"),
    );
    let e = b.err("project.delete", json!({"project": "Roof repair"}));
    assert_eq!(e.code(), "rejected");
    assert!(e.to_string().contains("2 tagged splits"), "{e}");
    b.run("project.delete", json!({"project": "Spare"}));
    assert_eq!(
        b.run("project.list", json!({}))["projects"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn projects_pj9_budget_by_account() {
    let mut b = book();
    b.run(
        "project.create",
        json!({"name": "Roof repair", "budget_mode": "by_account", "budget_lines": [
            {"account": "Expenses:Materials", "amount": "3000.00"},
            {"account": "Expenses:Labour", "amount": "2000.00"}]}),
    );
    spend(
        &mut b,
        "2026-05-02",
        "Home Depot",
        "1200.00",
        "Visa",
        "Materials",
        Some("Roof repair"),
    );
    spend(
        &mut b,
        "2026-05-10",
        "A1 Roofing",
        "2100.00",
        "Checking",
        "Labour",
        Some("Roof repair"),
    );
    spend(
        &mut b,
        "2026-05-11",
        "Coffee",
        "20.00",
        "Checking",
        "Supplies",
        Some("Roof repair"),
    );
    let g = b.run("project.get", json!({"project": "Roof repair"}));
    assert_eq!(g["budget_total"], "5000.00");
    let r = b.run("report.project", json!({"project": "Roof repair"}));
    let lines = r["budget_lines"].as_array().unwrap();
    let names: Vec<&str> = lines.iter().map(|l| l["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "Expenses:Materials",
            "Expenses:Labour",
            "Not budgeted",
            "Total"
        ]
    );
    assert_eq!(lines[0]["actual"], "1200.00");
    assert_eq!(lines[0]["remaining"], "1800.00");
    assert_eq!(lines[1]["remaining"], "-100.00");
    assert_eq!(lines[3]["actual"], "3320.00");
    // Switching to Total keeps 5,000.00; back to lines starts with Other.
    let p = b.run(
        "project.update",
        json!({"project": "Roof repair", "budget_mode": "total"}),
    );
    assert_eq!(p["budget"], "5000.00");
    assert!(p["budget_lines"].as_array().unwrap().is_empty());
    let p = b.run(
        "project.update",
        json!({"project": "Roof repair", "budget_mode": "by_account"}),
    );
    assert_eq!(p["budget_lines"][0]["account_name"], "Other");
    assert_eq!(p["budget_lines"][0]["amount"], "5000.00");
    let r = b.run("report.project", json!({"project": "Roof repair"}));
    assert_eq!(r["budget_lines"][0]["actual"], "3320.00");
    // A line needs an expense account.
    let e = b.err(
        "project.update",
        json!({"project": "Roof repair", "budget_lines": [{"account": "Assets:Checking", "amount": "1"}]}),
    );
    assert_eq!(e.code(), "invalid_args");
}

#[test]
fn projects_pj10_fund_link() {
    let mut b = book();
    b.run("module.enable", json!({"module": "funds"}));
    b.run(
        "fund.create",
        json!({"name": "Building Fund", "restriction": "with_donor_restrictions"}),
    );
    b.run(
        "project.create",
        json!({"name": "Building drive", "fund": "Building Fund"}),
    );
    let gift = |b: &mut Book, fund: Option<&str>| -> Value {
        let mut s =
            json!({"account": "Income:Gifts", "value": "-250.00", "project": "Building drive"});
        if let Some(f) = fund {
            s["fund"] = json!(f);
        }
        let id = b.run(
            "txn.create",
            json!({"date": "2026-08-01", "payee": "Donor", "splits": [s,
                {"account": "Assets:Checking", "value": "250.00"}]}),
        )["id"]
            .clone();
        let t = b.run("txn.get", json!({"id": id}));
        t["splits"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["account_name"] == "Income:Gifts")
            .unwrap()
            .clone()
    };
    assert_eq!(gift(&mut b, None)["tag_names"]["fund"], "Building Fund");
    assert_eq!(
        gift(&mut b, Some("General"))["tag_names"]["fund"],
        "General"
    );
    // Changing the project's fund later leaves tagged splits alone; the offer sets
    // the fund on tagged splits without one.
    b.run("module.disable", json!({"module": "funds"}));
    let untagged = gift(&mut b, None);
    assert!(untagged["tag_names"]["fund"].is_null());
    b.run("module.enable", json!({"module": "funds"}));
    let g = b.run("project.get", json!({"project": "Building drive"}));
    assert_eq!(g["untagged_fund_splits"], 1);
    let r = b.run(
        "project.set_fund_on_tagged",
        json!({"project": "Building drive"}),
    );
    assert_eq!(r["changed"], 1);
    let r = b.run("report.project", json!({"project": "Building drive"}));
    assert_eq!(r["fund_name"], "Building Fund");
}

#[test]
fn projects_pj11_billable_costs_on_invoices() {
    let mut b = book();
    b.run("module.enable", json!({"module": "invoicing"}));
    b.run("module.enable", json!({"module": "mileage"}));
    b.run("vehicle.create", json!({"name": "Van"}));
    b.run(
        "party.create",
        json!({"name": "Smith Family", "roles": ["customer"]}),
    );
    b.run(
        "project.create",
        json!({"name": "Roof repair", "customer": "Smith Family", "billable": true, "markup_percent": "10"}),
    );
    spend(
        &mut b,
        "2026-07-02",
        "Home Depot",
        "200.00",
        "Visa",
        "Materials",
        Some("Roof repair"),
    );
    spend(
        &mut b,
        "2026-07-03",
        "Lowes",
        "50.00",
        "Visa",
        "Materials",
        Some("Roof repair"),
    );
    // Not billable.
    b.run(
        "txn.create",
        json!({"date": "2026-07-04", "payee": "Coffee", "splits": [
            {"account": "Expenses:Supplies", "value": "9.00", "project": "Roof repair", "billable": false},
            {"account": "Assets:Checking", "value": "-9.00"}]}),
    );
    let u = b.run("project.unbilled", json!({"customer": "Smith Family"}));
    let p = &u["projects"][0];
    assert_eq!(p["count"], 2);
    assert_eq!(p["total"], "250.00");
    assert_eq!(p["with_markup"], "275.00");
    let keys: Vec<Value> = p["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].clone())
        .collect();
    let lines = b.run("project.billable_lines", json!({"items": keys}));
    assert_eq!(lines["account_name"], "Billable Expense Income");
    let ls = lines["lines"].as_array().unwrap().clone();
    assert_eq!(ls.len(), 2);
    assert_eq!(ls[0]["amount"], "220.00");
    assert_eq!(ls[1]["amount"], "55.00");
    let inv = b.run(
        "invoice.create",
        json!({"customer": "Smith Family", "date": "2026-07-31", "lines": ls}),
    );
    assert_eq!(inv["totals"]["total"], "275.00");
    let g = b.run("project.get", json!({"project": "Roof repair"}));
    let billed = g["billable_items"]["billed"].as_array().unwrap();
    assert_eq!(billed.len(), 2);
    assert_eq!(billed[0]["invoice_number"], inv["number"]);
    assert!(
        g["billable_items"]["unbilled"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    // The income shows in the project: net = 275.00 − 259.00.
    assert_eq!(g["summary"]["net"], "16.00");
    // The same costs cannot go on a second invoice.
    let again = b.err(
        "invoice.create",
        json!({"customer": "Smith Family", "date": "2026-08-01", "lines": ls}),
    );
    assert_eq!(again.code(), "rejected");
    // Summary lines: one per expense account.
    b.run("invoice.delete", json!({"invoice": inv["id"]}));
    let u = b.run("project.unbilled", json!({}));
    assert_eq!(u["projects"][0]["count"], 2);
    let s = b.run(
        "project.billable_lines",
        json!({"items": keys, "summary": true}),
    );
    assert_eq!(s["lines"].as_array().unwrap().len(), 1);
    assert_eq!(s["lines"][0]["amount"], "275.00");
    assert_eq!(s["lines"][0]["description"], "Expenses:Materials (2 items)");
    // A billable 40-mile trip at 76¢ offers 30.40 (33.44 with the markup).
    b.run(
        "trip.create",
        json!({"date": "2026-07-05", "purpose_kind": "business", "purpose": "Supplies",
               "from_place": "Office", "to_place": "Home Depot", "miles": "40",
               "project": "Roof repair"}),
    );
    let u = b.run("project.unbilled", json!({"project": "Roof repair"}));
    let trip = u["projects"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "mileage")
        .unwrap()
        .clone();
    assert_eq!(trip["amount"], "30.40");
    assert_eq!(trip["with_markup"], "33.44");
    // The project list shows Unbilled.
    let r = row(&mut b, "Roof repair");
    assert_eq!(r["unbilled"]["count"], 3);
}

#[test]
fn projects_transfers_not_counted_and_block() {
    let mut b = book();
    b.run(
        "project.create",
        json!({"name": "Roof repair", "budget": "100"}),
    );
    // A transfer between own accounts, tagged: listed, not counted.
    b.run(
        "txn.create",
        json!({"date": "2026-05-01", "splits": [
            {"account": "Liabilities:Visa", "value": "50.00", "project": "Roof repair"},
            {"account": "Assets:Checking", "value": "-50.00", "project": "Roof repair"}]}),
    );
    let g = b.run("project.get", json!({"project": "Roof repair"}));
    let t = g["transactions"].as_array().unwrap();
    assert_eq!(t.len(), 2);
    assert!(
        t.iter()
            .all(|x| x["kind"] == "transfer" && x["counted"] == false)
    );
    assert_eq!(g["summary"]["cost"], "0.00");
    // The Projects block: not on Home by default; added, it lists active projects.
    let home = b.run("dashboard.get", json!({"dashboard": "home"}));
    assert!(!home.to_string().contains("\"projects\""));
    let added = b.run(
        "dashboard.add_block",
        json!({"dashboard": "home", "type": "projects", "x": 0, "y": 0, "w": 4, "h": 3}),
    );
    let block = added["id"].as_str().unwrap_or_default().to_owned();
    let d = b.run(
        "dashboard.block_data",
        json!({"dashboard": "home", "block": block}),
    );
    assert_eq!(d["projects"][0]["name"], "Roof repair", "{d}");
    // Turning the module off hides the tags; values come back when it is on again.
    spend(
        &mut b,
        "2026-05-02",
        "Lumber",
        "30.00",
        "Checking",
        "Materials",
        Some("Roof repair"),
    );
    b.run("module.disable", json!({"module": "projects"}));
    b.run("module.enable", json!({"module": "projects"}));
    assert_eq!(row(&mut b, "Roof repair")["spent"], "30.00");
    // Closed projects stay in reports and the list, flagged closed.
    b.run(
        "project.set_status",
        json!({"project": "Roof repair", "status": "completed"}),
    );
    let r = row(&mut b, "Roof repair");
    assert_eq!(r["open"], false);
    assert_eq!(r["spent"], "30.00");
    let open = b.run("project.list", json!({"status": "open"}));
    assert!(open["projects"].as_array().unwrap().is_empty());
}

#[test]
fn projects_required_and_documents() {
    let mut b = book();
    b.run("module.enable", json!({"module": "bills"}));
    b.run("project.create", json!({"name": "Roof repair"}));
    b.run(
        "party.create",
        json!({"name": "Lumber Yard", "roles": ["vendor"]}),
    );
    // A bill with a document-level project: its expense lines are tagged, the
    // payable is not.
    b.run(
        "bill.create",
        json!({"vendor": "Lumber Yard", "date": "2026-06-01", "project": "Roof repair",
               "lines": [{"account": "Expenses:Materials", "amount": "400.00"}]}),
    );
    let g = b.run("project.get", json!({"project": "Roof repair"}));
    assert_eq!(g["summary"]["expenses"]["total"], "400.00");
    assert_eq!(g["documents"][0]["kind"], "bill");
    assert_eq!(g["transactions"].as_array().unwrap().len(), 1);
    // projects.required: an expense line needs a project.
    b.run(
        "module.set_setting",
        json!({"key": "projects.required", "value": true}),
    );
    let e = b.err(
        "txn.create",
        json!({"date": "2026-06-02", "splits": [
            {"account": "Expenses:Supplies", "value": "5.00"},
            {"account": "Assets:Checking", "value": "-5.00"}]}),
    );
    assert_eq!(e.code(), "rejected");
}
