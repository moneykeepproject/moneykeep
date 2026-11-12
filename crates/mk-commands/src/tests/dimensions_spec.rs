//! business-dimensions.md acceptance tests 1–6 and 10–12 (Parts A and D).

use super::*;

fn nonprofit() -> Book {
    let mut b = Book::new();
    b.standard();
    for m in ["classes", "funds"] {
        b.run("module.enable", json!({"module": m}));
    }
    b.run(
        "fund.create",
        json!({"name": "Youth Mission", "restriction": "with_donor_restrictions"}),
    );
    for (n, f) in [
        ("Worship", "program"),
        ("Youth", "program"),
        ("Admin", "management_general"),
        ("Fundraising", "fundraising"),
    ] {
        b.run("class.create", json!({"name": n, "function": f}));
    }
    b.account(json!({"name": "Tithes", "type": "income"}));
    b.account(json!({"name": "Gifts", "type": "income"}));
    for n in ["Salaries", "Youth Trips", "Events"] {
        b.account(json!({"name": n, "type": "expense", "parent": "Expenses"}));
    }
    b
}

fn tagged(
    b: &mut Book,
    date: &str,
    debit: (&str, Value),
    credit: (&str, Value),
    amount: &str,
) -> Value {
    let mut d = json!({"account": debit.0, "value": amount});
    let mut c = json!({"account": credit.0, "value": format!("-{amount}")});
    for (k, v) in debit.1.as_object().unwrap() {
        d[k] = v.clone();
    }
    for (k, v) in credit.1.as_object().unwrap() {
        c[k] = v.clone();
    }
    b.run(
        "txn.create",
        json!({"date": date, "payee": "x", "splits": [d, c]}),
    )
}

/// AT1 data; returns the trip expense transaction.
fn at1_data(b: &mut Book) -> Value {
    tagged(
        b,
        "2026-01-10",
        ("Assets:Checking", json!({"fund": "General"})),
        ("Tithes", json!({"fund": "General"})),
        "10000.00",
    );
    tagged(
        b,
        "2026-02-01",
        ("Assets:Checking", json!({"fund": "Youth Mission"})),
        ("Gifts", json!({"fund": "Youth Mission"})),
        "2000.00",
    );
    tagged(
        b,
        "2026-03-01",
        (
            "Expenses:Salaries",
            json!({"class": "Admin", "fund": "General"}),
        ),
        ("Assets:Checking", json!({})),
        "4000.00",
    );
    let trip = tagged(
        b,
        "2026-04-01",
        (
            "Expenses:Youth Trips",
            json!({"class": "Youth", "fund": "Youth Mission"}),
        ),
        ("Assets:Checking", json!({})),
        "1500.00",
    );
    tagged(
        b,
        "2026-05-01",
        (
            "Expenses:Events",
            json!({"class": "Fundraising", "fund": "General"}),
        ),
        ("Assets:Checking", json!({})),
        "300.00",
    );
    trip
}

fn year() -> Value {
    json!({"from": "2026-01-01", "to": "2026-12-31"})
}

#[test]
fn at1_statement_of_activities_and_release() {
    let mut b = nonprofit();
    let trip = at1_data(&mut b);
    let txns = b.run("txn.list", json!({}));
    let releases: Vec<&Value> = txns
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["payee"] == "Released from restrictions")
        .collect();
    assert_eq!(releases.len(), 1);
    let rel = b.run("txn.get", json!({"id": releases[0]["id"]}));
    assert_eq!(rel["release_for"], trip["id"]);
    let s = b.run("report.statement_of_activities", year());
    assert_eq!(
        s["total_revenue"],
        json!(["10000.00", "2000.00", "12000.00"])
    );
    assert_eq!(s["released"], json!(["1500.00", "-1500.00", "0.00"]));
    assert_eq!(s["total_expenses"], json!(["5800.00", "0.00", "5800.00"]));
    assert_eq!(
        s["change_in_net_assets"],
        json!(["5700.00", "500.00", "6200.00"])
    );
    assert_eq!(s["net_assets_end"], json!(["5700.00", "500.00", "6200.00"]));
    let fp = b.run(
        "report.statement_of_financial_position",
        json!({"as_of": "2026-12-31"}),
    );
    assert_eq!(fp["total_net_assets"], "6200.00");
    assert_eq!(fp["total_with"], "500.00");
    assert_eq!(fp["balanced"], true);
    // Editing the expense updates its release; undoing removes both.
    b.run(
        "txn.update",
        json!({"id": trip["id"], "date": "2026-04-01", "payee": "x", "splits": [
            {"account": "Expenses:Youth Trips", "value": "1200.00"},
            {"account": "Assets:Checking", "value": "-1200.00"}]}),
    );
    let s = b.run("report.statement_of_activities", year());
    assert_eq!(s["released"], json!(["1200.00", "-1200.00", "0.00"]));
    for _ in 0..3 {
        b.run("history.undo", json!({}));
    }
    let s = b.run("report.statement_of_activities", year());
    assert_eq!(s["released"], json!(["0.00", "0.00", "0.00"]));
    let n = b
        .run("txn.list", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["payee"] == "Released from restrictions")
        .count();
    assert_eq!(n, 0);
    let csv = b.run(
        "report.export",
        json!({"report": "report.statement_of_activities", "args": year()}),
    );
    assert!(
        csv["content"]
            .as_str()
            .unwrap()
            .contains("Change in net assets,6000.00,2000.00,8000.00")
    );
}

#[test]
fn at2_functional_expenses() {
    let mut b = nonprofit();
    at1_data(&mut b);
    let f = b.run("report.functional_expenses", year());
    assert_eq!(
        f["columns"],
        json!([
            "Program",
            "Management and general",
            "Fundraising",
            "Not assigned"
        ])
    );
    assert_eq!(f["totals"], json!(["1500.00", "4000.00", "300.00", "0.00"]));
    assert_eq!(f["total"], "5800.00");
}

#[test]
fn at3_fund_activity_and_overspent() {
    let mut b = nonprofit();
    at1_data(&mut b);
    let fa = b.run("report.fund_activity", year());
    let youth = fa["funds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "Youth Mission")
        .unwrap()
        .clone();
    assert_eq!(youth["beginning"], "0.00");
    assert_eq!(youth["revenue"], "2000.00");
    assert_eq!(youth["expenses_tagged"], "1500.00");
    assert_eq!(youth["released_out"], "1500.00");
    assert_eq!(youth["ending"], "500.00");
    assert_eq!(youth["overspent"], false);
    let second = tagged(
        &mut b,
        "2026-06-01",
        (
            "Expenses:Youth Trips",
            json!({"class": "Youth", "fund": "Youth Mission"}),
        ),
        ("Assets:Checking", json!({})),
        "800.00",
    );
    let fa = b.run("report.fund_activity", year());
    let youth = fa["funds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "Youth Mission")
        .unwrap()
        .clone();
    assert_eq!(youth["ending"], "-300.00");
    assert_eq!(youth["overspent_by"], "300.00");
    let att = b.run("fund.attention", json!({"today": "2026-12-31"}));
    assert_eq!(
        att[0]["message"],
        "Youth Mission fund is overspent by 300.00"
    );
    // Auto release off: the 800.00 is spent but not released.
    b.run("history.undo", json!({}));
    let _ = second;
    b.run(
        "module.set_setting",
        json!({"key": "funds.auto_release", "value": false}),
    );
    tagged(
        &mut b,
        "2026-06-01",
        (
            "Expenses:Youth Trips",
            json!({"class": "Youth", "fund": "Youth Mission"}),
        ),
        ("Assets:Checking", json!({})),
        "800.00",
    );
    let fa = b.run("report.fund_activity", year());
    let youth = fa["funds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "Youth Mission")
        .unwrap()
        .clone();
    assert_eq!(youth["spent_not_released"], "800.00");
    assert_eq!(youth["ending"], "500.00");
    assert!(
        b.run("fund.attention", json!({"today": "2026-12-31"}))
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Manual release and transfer.
    let r = b.run(
        "fund.release",
        json!({"from_fund": "Youth Mission", "amount": "800.00", "date": "2026-06-02"}),
    );
    assert_eq!(r["warnings"].as_array().unwrap().len(), 1);
    b.run("fund.create", json!({"name": "Building"}));
    b.run(
        "fund.transfer",
        json!({"from": "General", "to": "Building", "amount": "100.00", "date": "2026-07-01"}),
    );
    let list = b.run("fund.list", json!({}));
    let building = list
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "Building")
        .unwrap()
        .clone();
    assert_eq!(building["uses"], 1);
    let fa = b.run("report.fund_activity", year());
    let bld = fa["funds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "Building")
        .unwrap()
        .clone();
    assert_eq!(bld["transfers_in"], "100.00");
    assert_eq!(bld["ending"], "100.00");
    assert!(
        b.try_run("fund.delete", json!({"fund": "Building"}))
            .is_err()
    );
    assert!(
        b.try_run("fund.delete", json!({"fund": "General"}))
            .is_err()
    );
}

#[test]
fn at4_profit_and_loss_by_class() {
    let mut b = nonprofit();
    at1_data(&mut b);
    let p = b.run(
        "report.profit_loss",
        json!({"from": "2026-01-01", "to": "2026-12-31", "columns": "class"}),
    );
    let labels: Vec<&str> = p["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        ["Admin", "Fundraising", "Worship", "Youth", "Not set"]
    );
    assert_eq!(
        p["net_income"]["amounts"],
        json!(["-4000.00", "-300.00", "0.00", "-1500.00", "12000.00"])
    );
    assert_eq!(p["net_income"]["total"], "6200.00");
    let total = b.run(
        "report.profit_loss",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    assert_eq!(total["net_income"]["total"], "6200.00");
    let q = b.run(
        "report.profit_loss",
        json!({"from": "2026-01-01", "to": "2026-12-31", "columns": "quarter"}),
    );
    assert_eq!(
        q["net_income"]["amounts"],
        json!(["8000.00", "-1800.00", "0.00", "0.00"])
    );
    let f = b.run(
        "report.profit_loss",
        json!({"from": "2026-01-01", "to": "2026-12-31", "columns": "fund"}),
    );
    assert_eq!(f["net_income"]["total"], "6200.00");
    let bs = b.run(
        "report.balance_sheet",
        json!({"as_of": "2026-12-31", "columns": "fund"}),
    );
    assert_eq!(bs["total_net_assets"], "6200.00");
    assert_eq!(bs["balanced"], true);
    // Cost of goods sold is its own section.
    b.account(json!({"name": "Books Sold", "type": "expense", "subtype": "cost-of-goods"}));
    b.transfer("2026-08-01", "Books Sold", "Assets:Checking", "50.00");
    let total = b.run(
        "report.profit_loss",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    assert_eq!(total["sections"][1]["total"], "50.00");
    assert_eq!(total["net_income"]["total"], "6150.00");
}

#[test]
fn at5_required_class() {
    let mut b = nonprofit();
    b.run(
        "module.set_setting",
        json!({"key": "classes.required", "value": true}),
    );
    let e = b.err("txn.create", json!({"date": "2026-03-01", "splits": [
        {"account": "Expenses:Salaries", "value": "10.00"}, {"account": "Assets:Checking", "value": "-10.00"}]}));
    assert!(
        e.to_string().contains("Expenses:Salaries") && e.to_string().contains("needs a class"),
        "{e}"
    );
    let e = b.err("txn.create", json!({"date": "2026-03-01", "splits": [
        {"account": "Assets:Checking", "value": "10.00"}, {"account": "Tithes", "value": "-10.00"}]}));
    assert!(e.to_string().contains("Tithes"), "{e}");
    // With a class it saves, and a restricted expense's release is exempt.
    tagged(
        &mut b,
        "2026-03-01",
        (
            "Expenses:Youth Trips",
            json!({"class": "Youth", "fund": "Youth Mission"}),
        ),
        ("Assets:Checking", json!({})),
        "10.00",
    );
}

#[test]
fn at6_rule_sets_tags() {
    let mut b = nonprofit();
    b.run(
        "rule.create",
        json!({"name": "Payroll", "conditions": [{"field": "payee", "op": "contains", "value": "PAYROLL"}],
               "actions": [{"action": "set_account", "account": "Expenses:Salaries"}, {"action": "set_class", "class": "Admin"}, {"action": "set_fund", "fund": "General"}]}),
    );
    let qif = "!Type:Bank\nD03/15/2026\nT-500.00\nPACME PAYROLL\n^\n";
    let batch = b.stage_qif(qif, "Assets:Checking");
    let rows = batch["rows"].as_array().unwrap();
    assert!(rows[0]["tags"]["class"].as_str().is_some(), "{}", rows[0]);
    b.commit(&batch);
    let txns = b.run("txn.list", json!({}));
    let t = b.run("txn.get", json!({"id": txns[0]["id"]}));
    let sal = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["account_name"] == "Expenses:Salaries")
        .unwrap()
        .clone();
    assert_eq!(sal["tag_names"]["class"], "Admin");
    assert_eq!(sal["tag_names"]["fund"], "General");
}

const AT10: &str = "!TRNS\tTRNSTYPE\tDATE\tACCNT\tNAME\tCLASS\tAMOUNT\tDOCNUM\tMEMO\r\n!SPL\tTRNSTYPE\tDATE\tACCNT\tNAME\tCLASS\tAMOUNT\tDOCNUM\tMEMO\r\n!ENDTRNS\r\nTRNS\tCHECK\t9/5/2026\tBank Checking\tPost Office\t\t-70.00\t1001\tStamps\r\nSPL\tCHECK\t9/5/2026\tOffice Supplies\t\t\t10.00\t\t\r\nSPL\tCHECK\t9/5/2026\tPostage\t\t\t60.00\t\t\r\nENDTRNS\r\n";

fn stage_iif(b: &mut Book, content: &str) -> Value {
    b.run(
        "import.stage",
        json!({"format": "iif", "content": content, "name": "file.iif"}),
    )
}

#[test]
fn at10_iif_import_with_mapping() {
    let mut b = Book::new();
    b.standard();
    let batch = stage_iif(&mut b, AT10);
    let names: Vec<&str> = batch["names"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["kind"] == "account")
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Bank Checking", "Office Supplies", "Postage"]);
    let id = batch["id"].clone();
    b.run(
        "import.map_name",
        json!({"batch": id, "name": "Bank Checking", "to": "Assets:Checking"}),
    );
    b.run(
        "import.map_name",
        json!({"batch": id, "name": "Office Supplies", "create": "expense"}),
    );
    let v = b.run(
        "import.map_name",
        json!({"batch": id, "name": "Postage", "create": "expense"}),
    );
    assert_eq!(v["rows"][0]["number"].as_str().unwrap_or("1001"), "1001");
    b.commit(&v);
    let txns = b.run("txn.list", json!({}));
    assert_eq!(txns.as_array().unwrap().len(), 1);
    let t = b.run("txn.get", json!({"id": txns[0]["id"]}));
    assert_eq!(t["splits"].as_array().unwrap().len(), 3);
    assert_eq!(t["number"], "1001");
    assert_eq!(b.shown("Postage", "USD"), "60.00");
    // Re-importing finds it as a duplicate.
    let again = stage_iif(&mut b, AT10);
    assert_eq!(again["counts"]["duplicate"], 1, "{}", again["counts"]);
}

#[test]
fn at11_iif_unbalanced_and_unknown_keyword() {
    let mut b = Book::new();
    b.standard();
    let bad = "!TRNS\tTRNSTYPE\tDATE\tACCNT\tAMOUNT\r\n!SPL\tTRNSTYPE\tDATE\tACCNT\tAMOUNT\r\n!ENDTRNS\r\nTRNS\tCHECK\t9/5/2026\tAssets:Checking\t-70.00\r\nSPL\tCHECK\t9/5/2026\tExpenses:Groceries\t65.00\r\nENDTRNS\r\n!INVITEM\tNAME\r\nINVITEM\tWidget\r\nINVITEM\tGadget\r\n";
    let batch = stage_iif(&mut b, bad);
    let row = &batch["rows"][0];
    assert_eq!(row["needs_review"], true);
    assert!(
        row["warnings"]
            .to_string()
            .contains("unbalanced: the amounts sum to -5.00"),
        "{row}"
    );
    let w = batch["warnings"].as_array().unwrap();
    assert_eq!(
        w.iter()
            .filter(|x| x.as_str().unwrap().contains("INVITEM"))
            .count(),
        1
    );
    // Lists are offered and created on request.
    let lists = "!ACCNT\tNAME\tACCNTTYPE\r\nACCNT\tSavings\tBANK\r\n!VEND\tNAME\tBADDR1\tBADDR2\r\nVEND\tPaper Co\t1 Elm\tTown\r\n";
    let batch = stage_iif(&mut b, lists);
    assert_eq!(batch["lists"].as_array().unwrap().len(), 2);
    b.run("module.enable", json!({"module": "bills"}));
    b.run(
        "import.create_lists",
        json!({"batch": batch["id"], "items": [0, 1]}),
    );
    assert_eq!(
        b.run("account.get", json!({"account": "Savings"}))["subtype"],
        "bank"
    );
    assert_eq!(
        b.run("party.get", json!({"party": "Paper Co"}))["billing_address"],
        json!(["1 Elm", "Town"])
    );
}

#[test]
fn at12_iif_export_round_trip() {
    let mut b = Book::new();
    b.standard();
    b.transfer(
        "2026-09-05",
        "Expenses:Groceries",
        "Assets:Checking",
        "12.34",
    );
    b.run("txn.create", json!({"date": "2026-09-06", "payee": "Café", "splits": [
        {"account": "Expenses:Groceries", "value": "5.00"}, {"account": "Liabilities:Visa", "value": "-5.00"}]}));
    let e = b.run(
        "export.iif",
        json!({"from": "2026-09-01", "to": "2026-09-30"}),
    );
    let text = e["content"].as_str().unwrap();
    let lines: Vec<&str> = text.split("\r\n").collect();
    assert!(lines[0].starts_with("!HDR\tPROD"));
    assert!(lines[1].starts_with("HDR\tMoneyKeep"));
    assert!(lines[2].starts_with("!TRNS\tTRNSTYPE\tDATE\tACCNT\tNAME\tCLASS\tAMOUNT"));
    assert!(lines[3].starts_with("!SPL\t"));
    assert_eq!(lines[4], "!ENDTRNS");
    assert!(lines[5].starts_with("TRNS\tGENERAL JOURNAL\t09/05/2026\t"));
    assert!(lines[6].starts_with("SPL\t"));
    assert_eq!(lines[7], "ENDTRNS");
    assert!(!text.replace("\r\n", "").contains('\n'));
    assert_eq!(e["warnings"].as_array().unwrap().len(), 1);
    // Into a new book with the same accounts.
    let mut c = Book::new();
    c.standard();
    let batch = stage_iif(&mut c, text);
    assert!(
        batch["names"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["kind"] == "account")
            .all(|n| !n["mapped"].is_null())
    );
    c.commit(&batch);
    assert_eq!(c.shown("Expenses:Groceries", "USD"), "17.34");
    assert_eq!(c.shown("Liabilities:Visa", "USD"), "5.00");
    assert_eq!(c.shown("Assets:Checking", "USD"), "-12.34");
}
