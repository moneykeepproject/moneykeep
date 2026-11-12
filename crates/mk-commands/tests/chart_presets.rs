//! chart-presets.md acceptance tests CP12-CP21, CP23 and CP24 (books, roles and
//! the Tax summary), through the public `Session` API.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use mk_commands::Session;
use serde_json::{Value, json};

fn preset_book(preset: &str) -> Session {
    Session::in_memory_with_preset("Test", "USD", "test", preset).unwrap()
}

fn run(s: &mut Session, cmd: &str, args: Value) -> Value {
    s.invoke(cmd, args.clone())
        .unwrap_or_else(|e| panic!("{cmd} {args} failed: {e}"))
}

fn accounts(s: &mut Session) -> Vec<Value> {
    run(s, "account.list", json!({"include_archived": true}))
        .as_array()
        .unwrap()
        .clone()
}

fn by_code(s: &mut Session, code: &str) -> Value {
    accounts(s)
        .into_iter()
        .find(|a| a["code"] == code)
        .unwrap_or_else(|| panic!("no account with code {code}"))
}

fn codes(s: &mut Session) -> Vec<String> {
    let mut c: Vec<String> = accounts(s)
        .iter()
        .filter_map(|a| a["code"].as_str().map(str::to_owned))
        .collect();
    c.sort();
    c
}

fn module_on(s: &mut Session, m: &str) -> bool {
    run(s, "module.list", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["name"] == m && x["enabled"] == true)
}

fn txn(s: &mut Session, date: &str, debit: &str, credit: &str, amount: &str) -> String {
    let v = run(
        s,
        "txn.create",
        json!({"date": date, "splits": [
            {"account": debit, "value": amount},
            {"account": credit, "value": format!("-{amount}")}]}),
    );
    v["id"].as_str().unwrap().to_owned()
}

fn preset_accounts(id: &str) -> Vec<Value> {
    let all = mk_commands::chart::presets_json().unwrap();
    all.as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .unwrap()["accounts"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn cp12_book_without_preset_has_only_opening_balances() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::create(&dir.path().join("b.moneykeep"), "B", "USD", "test").unwrap();
    let list = accounts(&mut s);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["name"], "Opening Balances");
    assert!(list[0]["code"].is_null());
    assert!(list[0]["tax_line"].is_null());
    let info = run(&mut s, "book.info", json!({}));
    assert!(info["chart"]["preset"].is_null());
}

#[test]
fn cp12_unknown_preset_is_refused_before_any_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.moneykeep");
    let e = Session::create_with_preset(&path, "X", "USD", "test", Some("x")).unwrap_err();
    assert_eq!(e.to_string(), "invalid arguments: Unknown chart preset 'x'");
    assert!(!path.exists());
}

#[test]
fn cp13_personal_book() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.moneykeep");
    let mut s = Session::create_with_preset(&path, "P", "USD", "test", Some("personal")).unwrap();
    let mut want: Vec<String> = preset_accounts("personal")
        .iter()
        .map(|a| a["code"].as_str().unwrap().to_owned())
        .collect();
    want.sort();
    assert_eq!(codes(&mut s), want);
    // The investments pack is part of it.
    assert!(want.contains(&"1400".to_owned()));
    let housing = by_code(&mut s, "6000");
    assert_eq!(housing["name"], "Housing");
    assert_eq!(housing["placeholder"], true);
    let rent = by_code(&mut s, "6010");
    assert_eq!(rent["full_name"], "Housing:Rent");
    assert!(module_on(&mut s, "investments"));
    let info = run(&mut s, "book.info", json!({}));
    assert_eq!(info["chart"]["preset"], "personal");
    assert_eq!(info["chart"]["started_from"], "Personal & Household");
    let ob = by_code(&mut s, "3900");
    assert_eq!(ob["name"], "Opening Balances");
    assert_eq!(ob["role"], "opening_balances");
    assert_eq!(ob["id"], info["opening_balances_account"]);
    // Creation is not an undo step of its own.
    assert!(s.invoke("history.undo", json!({})).is_err());
    assert_eq!(codes(&mut s).len(), want.len());
}

#[test]
fn cp13_tax_year_in_settings_line() {
    let mut s = preset_book("personal_1040");
    let info = run(&mut s, "book.info", json!({}));
    assert_eq!(info["chart"]["tax_year"], 2025);
    assert_eq!(
        info["chart"]["started_from"],
        "Personal — Form 1040 (tax year 2025)"
    );
}

#[test]
fn cp14_nonprofit_turns_on_classes_and_funds() {
    let mut s = preset_book("nonprofit");
    assert!(module_on(&mut s, "classes"));
    assert!(module_on(&mut s, "funds"));
    assert_eq!(
        by_code(&mut s, "3000")["name"],
        "Net Assets Without Donor Restrictions"
    );
}

fn invoice(s: &mut Session) -> Value {
    run(s, "party.create", json!({"name": "Acme LLC"}));
    run(
        s,
        "invoice.create",
        json!({"customer": "Acme LLC", "date": "2026-09-01",
               "lines": [{"account": "Sales", "amount": "100.00"}]}),
    )
}

fn invoice_debit_account(s: &mut Session, inv: &Value) -> String {
    let t = run(s, "txn.get", json!({"id": inv["txn"]}));
    let split = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["value"] == "100.00" || x["value"] == "100")
        .unwrap()
        .clone();
    split["account"].as_str().unwrap().to_owned()
}

#[test]
fn cp15_module_pack_in_one_undo_step_and_role_after_rename() {
    let mut s = preset_book("small_business");
    let before = codes(&mut s);
    let v = run(&mut s, "module.enable", json!({"module": "invoicing"}));
    assert_eq!(v["accounts_added"], 2);
    assert_eq!(by_code(&mut s, "1200")["name"], "Accounts Receivable");
    assert_eq!(by_code(&mut s, "1250")["name"], "Payments to Deposit");
    run(&mut s, "history.undo", json!({}));
    assert_eq!(codes(&mut s), before);
    assert!(!module_on(&mut s, "invoicing"));
    run(&mut s, "module.enable", json!({"module": "invoicing"}));
    let ar = by_code(&mut s, "1200");
    run(
        &mut s,
        "account.update",
        json!({"account": "1200", "name": "Customer Balances"}),
    );
    let inv = invoice(&mut s);
    assert_eq!(
        invoice_debit_account(&mut s, &inv),
        ar["id"].as_str().unwrap()
    );
    // Turning the module off and on again adds nothing.
    run(&mut s, "module.disable", json!({"module": "invoicing"}));
    let v = run(&mut s, "module.enable", json!({"module": "invoicing"}));
    assert_eq!(v["accounts_added"], 0);
}

#[test]
fn cp16_book_without_preset_keeps_first_use_creation() {
    let mut s = Session::in_memory("T", "USD", "test").unwrap();
    run(
        &mut s,
        "account.create",
        json!({"name": "Sales", "type": "income"}),
    );
    let v = run(&mut s, "module.enable", json!({"module": "invoicing"}));
    assert_eq!(v["accounts_added"], 0);
    assert_eq!(accounts(&mut s).len(), 2);
    let inv = invoice(&mut s);
    let id = invoice_debit_account(&mut s, &inv);
    let ar = run(&mut s, "account.get", json!({"account": id}));
    assert_eq!(ar["name"], "Accounts Receivable");
    assert_eq!(ar["role"], "receivable");
    assert_eq!(ar["role_purpose"], "money customers owe you");
}

#[test]
fn cp17_import_posts_to_the_uncategorised_role_account() {
    let mut s = preset_book("personal");
    let id = by_code(&mut s, "8999")["id"].clone();
    run(
        &mut s,
        "account.update",
        json!({"account": "8999", "name": "To sort"}),
    );
    let batch = run(
        &mut s,
        "import.stage",
        json!({"format": "qif", "account": "Checking",
               "content": "!Type:Bank\nD10/01/2026\nT-42.00\nPCorner Shop\n^\n"}),
    );
    run(&mut s, "import.commit", json!({"batch": batch["id"]}));
    let b = run(&mut s, "account.balance", json!({"account": id}));
    assert_eq!(b["display"]["USD"], "42.00");
    // No new "Uncategorized Expense" account appeared.
    assert!(
        !accounts(&mut s)
            .iter()
            .any(|a| a["name"] == "Uncategorized Expense")
    );
}

#[test]
fn cp18_preview_and_add_accounts_from_a_chart() {
    let mut s = preset_book("personal");
    let before = accounts(&mut s);
    let p = run(
        &mut s,
        "chart.preview_add",
        json!({"preset": "personal_1040"}),
    );
    let row = |code: &str| {
        p["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["code"] == code)
            .unwrap()
            .clone()
    };
    assert_eq!(row("1500")["action"], "skip");
    assert_eq!(row("1500")["reason"], "same code");
    assert_eq!(row("4010")["action"], "skip");
    assert_eq!(row("4010")["reason"], "same code as Bonus & Commission");
    assert_eq!(row("1430")["action"], "add");
    let adds = p["adds"].as_u64().unwrap();
    let skips = p["skips"].as_u64().unwrap();
    assert_eq!(
        p["summary"],
        format!("Adds {adds} accounts; skips {skips} already in this book")
    );
    let v = run(
        &mut s,
        "chart.add",
        json!({"preset": "personal_1040", "exclude": ["1430"]}),
    );
    let added = v["added"].as_array().unwrap().len();
    assert_eq!(added as u64, adds - 1);
    let after = accounts(&mut s);
    assert_eq!(after.len(), before.len() + added);
    assert!(!after.iter().any(|a| a["code"] == "1430"));
    // Existing accounts are unchanged.
    for a in &before {
        let now = after.iter().find(|x| x["id"] == a["id"]).unwrap();
        assert_eq!(now, a);
    }
    // The book keeps its preset.
    let info = run(&mut s, "book.info", json!({}));
    assert_eq!(info["chart"]["preset"], "personal");
    // One undo step removes them all.
    run(&mut s, "history.undo", json!({}));
    assert_eq!(accounts(&mut s), before);
}

#[test]
fn cp18_same_name_only_clashes_under_the_same_parent() {
    let mut s = Session::in_memory("T", "USD", "test").unwrap();
    run(
        &mut s,
        "account.create",
        json!({"name": "Rent", "type": "expense"}),
    );
    let p = run(&mut s, "chart.preview_add", json!({"preset": "personal"}));
    let row = |code: &str| {
        p["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["code"] == code)
            .unwrap()
            .clone()
    };
    assert_eq!(row("6000")["action"], "add");
    assert_eq!(row("6010")["action"], "add_under");
    assert_eq!(row("6010")["full_name"], "Housing:Rent");
    // Opening Balances already holds its role.
    assert_eq!(row("3900")["action"], "skip");
    assert_eq!(
        row("3900")["reason"],
        "MoneyKeep already uses Opening Balances for this"
    );
    // Excluding a header still adds it when a child needs it.
    run(
        &mut s,
        "chart.add",
        json!({"preset": "personal", "exclude": ["6000"]}),
    );
    assert_eq!(by_code(&mut s, "6010")["full_name"], "Housing:Rent");
    let info = run(&mut s, "book.info", json!({}));
    assert_eq!(info["chart"]["preset"], "personal");
}

#[test]
fn cp19_tax_line_rules() {
    let mut s = Session::in_memory("T", "USD", "test").unwrap();
    let e = s
        .invoke(
            "account.create",
            json!({"name": "Rent", "type": "expense", "tax_line": "x".repeat(41)}),
        )
        .unwrap_err();
    assert_eq!(e.code(), "invalid_args");
    let e = s
        .invoke(
            "account.create",
            json!({"name": "Housing", "type": "expense", "placeholder": true, "tax_line": "Sch A"}),
        )
        .unwrap_err();
    assert_eq!(e.code(), "rejected");
    run(
        &mut s,
        "account.create",
        json!({"name": "Rent", "type": "expense", "tax_line": "  Sch E L3  "}),
    );
    let a = run(&mut s, "account.get", json!({"account": "Rent"}));
    assert_eq!(a["tax_line"], "Sch E L3");
    let e = s
        .invoke(
            "account.update",
            json!({"account": "Rent", "placeholder": true}),
        )
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        "Remove the tax line first: totals can't be posted to a heading."
    );
    // Empty clears; undo restores.
    run(
        &mut s,
        "account.update",
        json!({"account": "Rent", "tax_line": ""}),
    );
    assert!(run(&mut s, "account.get", json!({"account": "Rent"}))["tax_line"].is_null());
    run(&mut s, "history.undo", json!({}));
    assert_eq!(
        run(&mut s, "account.get", json!({"account": "Rent"}))["tax_line"],
        "Sch E L3"
    );
}

fn group<'a>(r: &'a Value, section: &str, tag: &str) -> &'a Value {
    r[section]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["tax_line"] == tag)
        .unwrap_or_else(|| panic!("no group {tag} in {section}: {r}"))
}

fn cp20_book() -> Session {
    let mut s = preset_book("personal_1040");
    txn(&mut s, "2025-02-01", "Checking", "4010", "3000.00");
    txn(&mut s, "2025-03-01", "Checking", "4010", "2000.00");
    let ret = by_code(&mut s, "6320")["full_name"]
        .as_str()
        .unwrap()
        .to_owned();
    txn(&mut s, "2025-04-01", &ret, "Checking", "1200.00");
    let groc = by_code(&mut s, "6825")["full_name"]
        .as_str()
        .unwrap()
        .to_owned();
    txn(&mut s, "2025-05-01", &groc, "Checking", "400.00");
    txn(
        &mut s,
        "2025-06-01",
        "Uncategorized Expense",
        "Checking",
        "50.00",
    );
    s
}

#[test]
fn cp20_tax_summary_groups_and_not_tagged() {
    let mut s = cp20_book();
    let r = run(&mut s, "report.tax_summary", json!({"today": "2026-11-04"}));
    assert_eq!(r["from"], "2025-01-01");
    assert_eq!(r["to"], "2025-12-31");
    assert_eq!(r["tax_year"], 2025);
    assert!(
        r["note"]
            .as_str()
            .unwrap()
            .starts_with("Tax lines are hints")
    );
    assert_eq!(group(&r, "activity", "1040 L1a")["total"], "5000.00");
    assert_eq!(group(&r, "activity", "Sch A L5b")["total"], "1200.00");
    let tags: Vec<&str> = r["activity"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["tax_line"].as_str().unwrap())
        .collect();
    let i1040 = tags.iter().position(|t| *t == "1040 L1a").unwrap();
    let ia = tags.iter().position(|t| *t == "Sch A L5b").unwrap();
    assert!(i1040 < ia);
    let un = &r["untagged"];
    let names: Vec<&str> = un["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["full_name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.iter().any(|n| n.ends_with("Groceries")));
    assert!(names.contains(&"Uncategorized Expense"));
    assert_eq!(un["total"], "450.00");
    assert!(r["no_tax_lines"].is_null());
}

#[test]
fn cp21_balance_section_and_negative_distributions() {
    let mut s = preset_book("s_corp");
    txn(
        &mut s,
        "2024-06-01",
        "Checking",
        "Opening Balances",
        "10000.00",
    );
    txn(
        &mut s,
        "2025-03-01",
        "Checking",
        "Gross Receipts",
        "2500.00",
    );
    txn(
        &mut s,
        "2025-04-01",
        "Shareholder Distributions",
        "Credit Card",
        "4000.00",
    );
    let r = run(
        &mut s,
        "report.tax_summary",
        json!({"from": "2025-01-01", "to": "2025-12-31"}),
    );
    let l1 = group(&r, "balance", "Sch L L1");
    assert_eq!(l1["beginning"], "10000.00");
    assert_eq!(l1["end"], "12500.00");
    assert_eq!(r["beginning_date"], "2024-12-31");
    assert_eq!(group(&r, "activity", "Sch K L16d")["total"], "-4000.00");
    // From the book's first day, beginning balances are zero.
    let r = run(
        &mut s,
        "report.tax_summary",
        json!({"from": "2024-06-01", "to": "2025-12-31"}),
    );
    assert_eq!(group(&r, "balance", "Sch L L1")["beginning"], "0.00");
}

#[test]
fn cp23_voided_never_counts_and_foreign_accounts_are_excluded() {
    let mut s = cp20_book();
    let id = txn(&mut s, "2025-07-01", "Checking", "4010", "999.00");
    run(&mut s, "txn.void", json!({"id": id, "reason": "mistake"}));
    run(&mut s, "module.enable", json!({"module": "multi-currency"}));
    run(
        &mut s,
        "account.create",
        json!({"name": "Paris Rent", "type": "expense", "commodity": "EUR", "tax_line": "Sch E L3"}),
    );
    let r = run(
        &mut s,
        "report.tax_summary",
        json!({"from": "2025-01-01", "to": "2025-12-31"}),
    );
    assert_eq!(group(&r, "activity", "1040 L1a")["total"], "5000.00");
    let ex = r["excluded"].as_array().unwrap();
    assert_eq!(ex.len(), 1);
    assert_eq!(ex[0]["full_name"], "Paris Rent");
}

#[test]
fn cp24_csv_export_matches_report() {
    let mut s = cp20_book();
    let args = json!({"from": "2025-01-01", "to": "2025-12-31"});
    let r = run(&mut s, "report.tax_summary", args.clone());
    let e = run(
        &mut s,
        "report.export",
        json!({"report": "tax_summary", "args": args}),
    );
    assert_eq!(e["file_name"], "tax-summary-2025-01-01-to-2025-12-31.csv");
    let csv = e["content"].as_str().unwrap();
    let mut lines = csv.lines();
    assert_eq!(
        lines.next().unwrap(),
        "Section,Tax line,Account,Code,Amount,Beginning,End,Included in"
    );
    assert!(csv.contains("Activity,1040 L1a,Total,,5000.00,,"));
    assert!(csv.contains("Activity,Sch A L5b,Total,,1200.00,,"));
    assert!(csv.contains("Not tagged,,Total,,450.00,,"));
    // Every activity group total in the CSV equals the report's.
    for g in r["activity"].as_array().unwrap() {
        let line = format!(
            "Activity,{},Total,,{},,",
            g["tax_line"].as_str().unwrap(),
            g["total"].as_str().unwrap()
        );
        assert!(csv.contains(&line), "{line}");
    }
    let pdf = run(
        &mut s,
        "report.export",
        json!({"report": "tax_summary", "args": args, "format": "pdf"}),
    );
    assert_eq!(pdf["file_name"], "tax-summary-2025-01-01-to-2025-12-31.pdf");
    assert!(pdf["content_base64"].as_str().unwrap().len() > 100);
}

#[test]
fn tax_summary_without_tax_lines_has_the_note() {
    let mut s = Session::in_memory("T", "USD", "test").unwrap();
    let r = run(&mut s, "report.tax_summary", json!({}));
    assert_eq!(
        r["no_tax_lines"],
        "This book's accounts have no tax lines. Add them in Accounts, or add accounts from a tax chart."
    );
    assert!(r["activity"].as_array().unwrap().is_empty());
}

#[test]
fn every_preset_creates_a_book() {
    let all = mk_commands::chart::presets_json().unwrap();
    for p in all.as_array().unwrap() {
        let id = p["id"].as_str().unwrap();
        let mut s = preset_book(id);
        let n = accounts(&mut s).len();
        assert_eq!(n, p["accounts"].as_array().unwrap().len(), "{id}");
        let d = p["description"].as_str().unwrap();
        assert!(d.chars().count() >= 200, "{id}");
    }
}

#[test]
fn encrypted_book_with_preset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("e.moneykeep");
    let (mut s, _) = Session::create_encrypted_with_preset(
        &path,
        "E",
        "USD",
        "test",
        "correct horse battery",
        false,
        mk_commands::KdfParams::TEST,
        Some("minimal"),
    )
    .unwrap();
    assert_eq!(by_code(&mut s, "8999")["role"], "uncategorized_expense");
}

#[test]
fn reimbursements_role_recreated_after_delete() {
    let mut s = preset_book("small_business");
    run(&mut s, "module.enable", json!({"module": "expenses"}));
    let r = by_code(&mut s, "2270");
    assert_eq!(r["role"], "reimbursements");
    run(&mut s, "account.delete", json!({"account": "2270"}));
    assert!(
        !accounts(&mut s)
            .iter()
            .any(|a| a["role"] == "reimbursements")
    );
}
