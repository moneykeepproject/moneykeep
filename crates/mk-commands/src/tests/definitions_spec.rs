//! definition-files.md acceptance tests 1–10 and input-safety.md AT6.

use std::time::{Duration, Instant};

use super::*;

const AT1: &str = r#"
# A hand-written definition.
moneykeep_definition = 1
id = "org.example.starter"
version = "1.0.0"
name = "Starter kit"
description = """
Broker layout, two categories and a rule.
"""
authors = ["Jane Doe"]
license = "CC0-1.0"
homepage = "https://example.org/mkdef"
created = 2026-10-20
tags = ["broker", "usa"]

[[investment_profile]]
key = "my-broker"
name = "My broker JSON"
format = "json"
signature = ["tradeDate", "activity"]
records = "data.transactions"
date_format = "MM/DD/YYYY"
fields = { date = "tradeDate", action = "activity", symbol = "symbol", quantity = "shares", price = "price", amount = "netAmount" }
[[investment_profile.actions]]
match_type = "equals"
text = "Bought"
action = "buy"

[[category]]
key = "food"
path = "Expenses:Food"
type = "expense"

[[category]]
key = "coffee"
path = "Expenses:Food:Coffee"
type = "expense"
description = "Cafés and coffee beans"

[[rule]]
key = "coffee"
name = "Coffee shops"
[[rule.conditions]]
field = "payee"
op = "contains"
value = "coffee"
[[rule.actions]]
action = "set_account"
account = "Expenses:Food:Coffee"
"#;

fn header(id: &str) -> String {
    format!("moneykeep_definition = 1\nid = \"{id}\"\nversion = \"1.0.0\"\nname = \"Test\"\n")
}

fn rule(key: &str, value: &str, op: &str) -> String {
    format!(
        "\n[[rule]]\nkey = \"{key}\"\nname = \"Rule {key}\"\n[[rule.conditions]]\nfield = \"payee\"\nop = \"{op}\"\nvalue = \"{value}\"\n[[rule.actions]]\naction = \"set_account\"\naccount = \"Expenses:Groceries\"\n"
    )
}

fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b
}

fn inspect(b: &mut Book, content: &str) -> Value {
    b.run(
        "definition.inspect",
        json!({"content": content, "name": "test.mkdef"}),
    )
}

fn install(b: &mut Book, content: &str) -> Value {
    let i = inspect(b, content);
    assert_eq!(i["valid"], true, "{i}");
    b.run(
        "definition.install",
        json!({"content": content, "sha256": i["sha256"], "name": "test.mkdef"}),
    )
}

fn errors(v: &Value) -> String {
    assert_eq!(v["valid"], false, "{v}");
    v["errors"].to_string()
}

fn actions(v: &Value) -> Vec<(String, String)> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["key"].as_str().unwrap().to_owned(),
                i["action"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn definition_at1_inspect_install_undo() {
    let mut b = book();
    let i = inspect(&mut b, AT1);
    assert_eq!(
        i["summary"],
        "Adds 1 broker profile, 2 categories and 1 rule"
    );
    assert_eq!(i["header"]["homepage"], "https://example.org/mkdef");
    let out = install(&mut b, AT1);
    assert_eq!(out["installed"], 4, "{out}");
    let coffee = b.run("account.get", json!({"account": "Expenses:Food:Coffee"}));
    assert_eq!(coffee["description"], "Cafés and coffee beans");
    let rules = b.run("rule.list", json!({}));
    let r = rules
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "Coffee shops")
        .unwrap()
        .clone();
    assert_eq!(r["enabled"], true);
    assert!(
        r.to_string().contains(coffee["id"].as_str().unwrap()),
        "{r}"
    );
    assert!(
        b.try_run("inv_profile.get", json!({"name": "My broker JSON"}))
            .is_ok()
    );
    let listed = b.run("definition.list", json!({}));
    assert_eq!(listed[0]["id"], "org.example.starter");
    // One audited command; undo removes everything.
    b.run("history.undo", json!({}));
    assert!(
        b.try_run("account.get", json!({"account": "Expenses:Food"}))
            .is_err()
    );
    assert!(
        b.try_run("inv_profile.get", json!({"name": "My broker JSON"}))
            .is_err()
    );
    assert!(
        !b.run("rule.list", json!({}))
            .to_string()
            .contains("Coffee shops")
    );
    assert_eq!(b.run("definition.list", json!({})), json!([]));
    // Installing twice: everything unchanged, install does nothing.
    install(&mut b, AT1);
    let again = inspect(&mut b, AT1);
    assert!(
        actions(&again).iter().all(|(_, a)| a == "unchanged"),
        "{again}"
    );
    assert_eq!(install(&mut b, AT1)["installed"], 0);
}

#[test]
fn definition_at2_export_validates_and_reinspects_unchanged() {
    let mut b = book();
    b.run(
        "rule.create",
        json!({"name": "Market", "conditions": [{"field": "payee", "op": "contains", "value": "market"}],
               "actions": [{"action": "set_account", "account": "Expenses:Groceries"}]}),
    );
    let out = b.run(
        "definition.export",
        json!({"id": "org.example.schwab", "version": "1.0.0", "name": "Schwab layouts",
               "investment_profiles": ["Charles Schwab — CSV", "Charles Schwab — JSON", "Charles Schwab — XML"],
               "rules": ["Market"]}),
    );
    assert_eq!(out["file_name"], "org.example.schwab-1.0.0.mkdef");
    let content = out["content"].as_str().unwrap().to_owned();
    assert!(content.starts_with("# MoneyKeep Definition file"));
    let v = b.run("definition.validate", json!({"content": content}));
    assert_eq!(v["valid"], true, "{v}");
    let i = inspect(&mut b, &content);
    let acts = actions(&i);
    assert_eq!(acts.len(), 4);
    assert!(acts.iter().all(|(_, a)| a == "unchanged"), "{i}");
    // Deterministic.
    let again = b.run(
        "definition.export",
        json!({"id": "org.example.schwab", "version": "1.0.0", "name": "Schwab layouts",
               "investment_profiles": ["Charles Schwab — CSV", "Charles Schwab — JSON", "Charles Schwab — XML"],
               "rules": ["Market"]}),
    );
    assert_eq!(again["content"], out["content"]);
}

#[test]
fn definition_at3_text_rules() {
    let mut b = book();
    let bidi = format!("{}description = \"a\u{202E}b\"\n", header("org.example.t"));
    let e = errors(&inspect(&mut b, &bidi));
    assert!(e.contains("description") && e.contains("U+202E"), "{e}");
    let zw = format!(
        "{}{}",
        header("org.example.t"),
        rule("r1", "cof\u{200B}fee", "contains")
    );
    let e = errors(&inspect(&mut b, &zw));
    assert!(
        e.contains("rule[r1].conditions[0].value") && e.contains("U+200B"),
        "{e}"
    );
    let cyr = format!(
        "{}{}",
        header("org.example.t"),
        rule("r1", "c\u{0430}fe", "contains")
    );
    let i = inspect(&mut b, &cyr);
    assert_eq!(i["valid"], true, "{i}");
    assert!(
        i["warnings"]
            .to_string()
            .contains("rule[r1].conditions[0].value"),
        "{i}"
    );
}

#[test]
fn definition_at4_size_depth_and_count_limits() {
    let mut b = book();
    let big = format!("{}# {}\n", header("org.example.t"), "x".repeat(1024 * 1024));
    assert!(errors(&inspect(&mut b, &big)).contains("1 MiB"));
    let deep = format!(
        "{}x = {}1{}\n",
        header("org.example.t"),
        "[".repeat(9),
        "]".repeat(9)
    );
    assert!(errors(&inspect(&mut b, &deep)).contains("deeper than 8"));
    let mut many = header("org.example.t");
    for i in 0..2001 {
        many.push_str(&rule(&format!("r{i}"), "x", "contains"));
    }
    assert!(errors(&inspect(&mut b, &many)).contains("the limit is 2000 rules"));
}

#[test]
fn definition_at5_linear_regex_and_size_limit() {
    let mut b = book();
    let ok = format!(
        "{}{}",
        header("org.example.t"),
        rule("r1", "(a+)+$", "regex")
    );
    install(&mut b, &ok);
    let payee = format!("{}!", "a".repeat(100_000));
    let t = Instant::now();
    let out = b.run(
        "rule.test",
        json!({"payee": payee, "amount": "-1.00", "account": "Assets:Checking"}),
    );
    assert!(
        t.elapsed() < Duration::from_millis(100),
        "{:?}",
        t.elapsed()
    );
    let _ = out;
    let huge = format!(
        "{}{}",
        header("org.example.t"),
        rule("r1", "\\\\w{1000}\\\\w{1000}", "regex")
    );
    let e = errors(&inspect(&mut b, &huge));
    assert!(e.contains("regex") || e.contains("size"), "{e}");
}

#[test]
fn definition_at6_unknown_keys_kinds_and_requires() {
    let mut b = book();
    let mut f = header("org.example.t");
    f.push_str(&rule("r1", "x", "contains").replace(
        "name = \"Rule r1\"\n",
        "name = \"Rule r1\"\ncolour = \"red\"\n",
    ));
    f.push_str("\n[[report]]\nkey = \"r\"\nname = \"Report\"\n\n[x_notes]\nnote = \"ignored\"\n");
    let i = inspect(&mut b, &f);
    let w = i["warnings"].as_array().unwrap();
    assert_eq!(w.len(), 2, "{i}");
    assert!(
        i["warnings"]
            .to_string()
            .contains("unknown key `colour` in rule `r1`")
    );
    assert!(i["warnings"].to_string().contains("unknown kind `report`"));
    assert_eq!(install(&mut b, &f)["installed"], 1);
    let req = format!(
        "{}requires = [\"rule-condition-any\"]\n",
        header("org.example.u")
    );
    assert!(errors(&inspect(&mut b, &req)).contains("rule-condition-any"));
}

#[test]
fn definition_at7_homepage_is_https_text() {
    let mut b = book();
    let bad = format!(
        "{}homepage = \"javascript:alert(1)\"\n",
        header("org.example.t")
    );
    assert!(errors(&inspect(&mut b, &bad)).contains("homepage"));
    let good = format!(
        "{}homepage = \"https://example.org/x\"\n",
        header("org.example.t")
    );
    let i = inspect(&mut b, &good);
    assert_eq!(i["header"]["homepage"], "https://example.org/x");
}

#[test]
fn definition_at8_uninstall_keeps_edited_items() {
    let mut b = book();
    let mut f = header("org.example.t");
    f.push_str(&rule("r1", "one", "contains"));
    f.push_str(&rule("r2", "two", "contains"));
    f.push_str("\n[[category]]\nkey = \"gifts\"\npath = \"Expenses:Gifts\"\ntype = \"expense\"\n");
    install(&mut b, &f);
    let rules = b.run("rule.list", json!({}));
    let r1 = rules
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "Rule r1")
        .unwrap()["id"]
        .clone();
    b.run("rule.update", json!({"id": r1, "stop": false}));
    let listed = b.run("definition.list", json!({}));
    assert_eq!(listed[0]["edited"][0]["key"], "r1");
    let out = b.run("definition.uninstall", json!({"id": "org.example.t"}));
    assert_eq!(out["kept"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(out["kept"][0]["key"], "r1");
    assert_eq!(out["removed"].as_array().unwrap().len(), 2, "{out}");
    let names = b.run("rule.list", json!({})).to_string();
    assert!(names.contains("Rule r1") && !names.contains("Rule r2"));
    assert!(
        b.try_run("account.get", json!({"account": "Expenses:Gifts"}))
            .is_err()
    );
}

#[test]
fn definition_at9_install_refuses_changed_bytes() {
    let mut b = book();
    let i = inspect(&mut b, AT1);
    let changed = AT1.replace("Starter kit", "Starter kit 2");
    let err = b
        .try_run(
            "definition.install",
            json!({"content": changed, "sha256": i["sha256"]}),
        )
        .unwrap_err();
    assert!(err.to_string().contains("SHA-256"), "{err}");
}

#[test]
fn definition_at10_export_has_no_account_numbers_balances_or_records() {
    let mut b = book();
    b.run(
        "account.update",
        json!({"account": "Expenses:Groceries", "code": "5100"}),
    );
    b.run(
        "account.update",
        json!({"account": "Assets:Checking", "code": "1234567890"}),
    );
    b.run(
        "txn.create",
        json!({"date": "2026-10-01", "payee": "Secret Payee", "splits": [
            {"account": "Expenses:Groceries", "value": "987.65"},
            {"account": "Assets:Checking", "value": "-987.65"}]}),
    );
    b.run(
        "csv.profile_save",
        json!({"profile": {"name": "My Bank", "columns": {"date": "Date", "payee": "Description", "amount": "Amount"},
               "date_format": "YYYY-MM-DD", "account": "Assets:Checking"}}),
    );
    b.run(
        "rule.create",
        json!({"name": "Market", "conditions": [{"field": "payee", "op": "contains", "value": "market"}],
               "actions": [{"action": "set_account", "account": "Expenses:Groceries"}]}),
    );
    // A file-name account key mapping exists in the book.
    b.run("module.enable", json!({"module": "investments"}));
    let out = b.run(
        "definition.export",
        json!({"id": "org.example.mine", "version": "1.0.0", "name": "Mine",
               "bank_profiles": ["My Bank"], "rules": ["Market"], "categories": ["Expenses"]}),
    );
    let c = out["content"].as_str().unwrap();
    for secret in [
        "5100",
        "1234567890",
        "987.65",
        "Secret Payee",
        "Assets:Checking",
        "csv:My Bank",
    ] {
        assert!(!c.contains(secret), "{secret} leaked:\n{c}");
    }
    assert!(c.contains("Expenses:Groceries"));
    // A rule naming a bank account is refused.
    b.run(
        "rule.create",
        json!({"name": "To checking", "conditions": [{"field": "payee", "op": "contains", "value": "x"}],
               "actions": [{"action": "set_account", "account": "Assets:Checking"}]}),
    );
    let err = b
        .try_run(
            "definition.export",
            json!({"id": "org.example.mine", "version": "1.0.0", "name": "Mine", "rules": ["To checking"]}),
        )
        .unwrap_err();
    assert!(err.to_string().contains("To checking"), "{err}");
}

/// input-safety.md AT6: a hostile definition plus a crafted data file.
#[test]
fn input_safety_at6_hostile_definition_and_data_file() {
    let mut b = book();
    b.run("module.enable", json!({"module": "investments"}));
    b.account(
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    let long = "p".repeat(5000);
    let bad = format!(
        "{}{}\n[[rule.actions]]\naction = \"set_payee\"\npayee = \"{long}\"\n",
        header("org.example.hostile"),
        rule("r1", "x", "contains")
    );
    let e = errors(&inspect(&mut b, &bad));
    assert!(e.contains("5000 characters"), "{e}");
    let hostile = format!(
        "{}\n[[investment_profile]]\nkey = \"evil\"\nname = \"Evil\"\nformat = \"json\"\nsignature = [\"ref\", \"act\"]\nrecords = \"rows\"\ndate_format = \"YYYY-MM-DD\"\nfields = {{ date = \"d\", action = \"act\", external_id = \"ref\", symbol = \"s\", quantity = \"q\", price = \"p\", amount = \"a\" }}\n[[investment_profile.actions]]\nmatch_type = \"contains\"\ntext = \"\"\naction = \"buy\"\n",
        header("org.example.hostile")
    );
    let i = inspect(&mut b, &hostile);
    if i["valid"] == true {
        b.run(
            "definition.install",
            json!({"content": hostile, "sha256": i["sha256"]}),
        );
        let data = r#"{"rows":[{"d":"2026-10-01","act":"anything at all","ref":"../../etc/passwd","s":"VTI","q":"1","p":"10","a":"-10"}]}"#;
        let batch = b.run(
            "import.stage",
            json!({"name": "data.json", "content": data, "account": "Assets:Brokerage"}),
        );
        // Everything the file does is a staged, reviewable row; nothing was posted.
        let rows = batch["inv_rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{batch}");
        assert_eq!(rows[0]["row"]["external_id"], "../../etc/passwd");
        assert!(
            b.run("txn.list", json!({"account": "Assets:Brokerage"}))
                .as_array()
                .unwrap()
                .is_empty()
        );
    } else {
        // An empty match text may itself be refused by validation.
        assert!(i["errors"].to_string().contains("evil"), "{i}");
    }
}

/// dashboards.md acceptance test 14: export "Church" and import it into another book;
/// blocks resolve accounts by name and an unknown account asks to choose one.
#[test]
fn dashboards_at14_export_and_import_by_name() {
    let mut a = book();
    a.account(json!({"name": "Savings", "type": "asset", "subtype": "bank", "parent": "Assets"}));
    a.transfer(
        "2026-10-01",
        "Assets:Checking",
        "Opening Balances",
        "1234.56",
    );
    let church = a.run(
        "dashboard.create",
        json!({"name": "Church", "icon": "church"}),
    );
    let dash = church["id"].as_str().unwrap().to_owned();
    for (kind, settings) in [
        (
            "account_transactions",
            json!({"account": "Assets:Checking", "n": 5}),
        ),
        (
            "account_transactions",
            json!({"account": "Assets:Savings", "n": 3}),
        ),
        ("note", json!({"text": "Offerings\ncounted on Monday"})),
        (
            "shortcuts",
            json!({"buttons": [
                {"label": "Checking", "target": {"kind": "account", "account": "Assets:Checking"}},
                {"label": "Church", "target": {"kind": "dashboard", "dashboard": "Church"}},
                {"label": "New expense", "target": {"kind": "action", "action": "new_expense"}}]}),
        ),
    ] {
        a.run(
            "dashboard.add_block",
            json!({"dashboard": dash, "type": kind, "settings": settings}),
        );
    }
    let out = a.run(
        "definition.export",
        json!({"id": "org.example.church", "version": "1.0.0", "name": "Church screens",
               "dashboards": ["Church"]}),
    );
    let content = out["content"].as_str().unwrap().to_owned();
    assert!(content.contains("[[dashboard]]"), "{content}");
    assert!(content.contains("[[dashboard.blocks]]"), "{content}");
    assert!(content.contains("Assets:Checking"), "{content}");
    // No ids, no amounts.
    assert!(!content.contains(&dash), "{content}");
    let checking = a.run("account.get", json!({"account": "Assets:Checking"}));
    assert!(!content.contains(checking["id"].as_str().unwrap()));
    assert!(!content.contains("1234.56") && !content.contains("1,234.56"));
    // It validates and re-inspects as unchanged in the book it came from.
    let v = a.run("definition.validate", json!({"content": content}));
    assert_eq!(v["valid"], true, "{v}");
    assert_eq!(actions(&inspect(&mut a, &content))[0].1, "unchanged");

    // Another book has Checking but no Savings.
    let mut b = book();
    let i = inspect(&mut b, &content);
    assert_eq!(i["summary"], "Adds 1 dashboard", "{i}");
    let r = install(&mut b, &content);
    assert_eq!(r["installed"], 1, "{r}");
    let got = b.run("dashboard.get", json!({"dashboard": "Church"}));
    assert_eq!(got["icon"], "church");
    let blocks = got["blocks"].as_array().unwrap().clone();
    assert_eq!(blocks.len(), 4);
    let id = got["id"].as_str().unwrap().to_owned();
    let data = |b: &mut Book, block: &Value| {
        b.run(
            "dashboard.block_data",
            json!({"dashboard": id, "block": block["id"], "today": "2026-10-20"}),
        )
    };
    let checking_b = b.run("account.get", json!({"account": "Assets:Checking"}));
    let first = blocks
        .iter()
        .find(|x| x["settings"]["account"] == checking_b["id"])
        .expect("the Checking block resolves to this book's account");
    assert_eq!(data(&mut b, first)["status"], "ok");
    let unknown = blocks
        .iter()
        .find(|x| x["type"] == "account_transactions" && x["settings"]["account"] == "")
        .unwrap();
    let d = data(&mut b, unknown);
    assert_eq!(d["status"], "choose_account");
    assert_eq!(d["message"], "Choose an account");
    let note = blocks.iter().find(|x| x["type"] == "note").unwrap();
    assert_eq!(note["settings"]["text"], "Offerings\ncounted on Monday");
    let sc = blocks.iter().find(|x| x["type"] == "shortcuts").unwrap();
    let d = data(&mut b, sc);
    assert_eq!(d["buttons"][0]["target"]["full_name"], "Assets:Checking");
    assert_eq!(
        d["buttons"][1]["target"]["name"], "Church",
        "the dashboard resolves by name"
    );
    // One undo removes it again; uninstalling removes an unedited dashboard.
    b.run("history.undo", json!({}));
    assert_eq!(
        b.run("dashboard.list", json!({})).as_array().unwrap().len(),
        1
    );
    b.run("history.redo", json!({}));
    let u = b.run("definition.uninstall", json!({"id": "org.example.church"}));
    assert_eq!(u["removed"].as_array().unwrap().len(), 1, "{u}");
    assert_eq!(
        b.run("dashboard.list", json!({})).as_array().unwrap().len(),
        1
    );
}

#[test]
fn dashboard_items_are_validated() {
    let mut b = book();
    let bad = format!(
        "{}\n[[dashboard]]\nkey = \"d\"\nname = \"D\"\n[[dashboard.blocks]]\ntype = \"note\"\nx = 0\ny = 0\nw = 4\nh = 2\n[[dashboard.blocks]]\ntype = \"note\"\nx = 2\ny = 1\nw = 4\nh = 2\n",
        header("org.example.bad")
    );
    let v = inspect(&mut b, &bad);
    assert!(errors(&v).contains("overlap"), "{v}");
    let future = format!(
        "{}\n[[dashboard]]\nkey = \"d\"\nname = \"Future\"\n[[dashboard.blocks]]\ntype = \"hologram\"\nx = 0\ny = 0\nw = 4\nh = 2\nsettings = {{ depth = 3 }}\n",
        header("org.example.future")
    );
    let v = inspect(&mut b, &future);
    assert!(v["warnings"].to_string().contains("hologram"), "{v}");
    install(&mut b, &future);
    let got = b.run("dashboard.get", json!({"dashboard": "Future"}));
    assert_eq!(got["blocks"][0]["settings"], json!({"depth": 3}));
    assert_eq!(got["blocks"][0]["known"], false);
}

/// chart-presets.md: categories carry an optional tax line in definition files.
#[test]
fn category_tax_line_round_trips() {
    let mut b = Book::new();
    let content = format!(
        "{}\n[[category]]\nkey = \"rent\"\npath = \"Rental:Repairs\"\ntype = \"expense\"\ntax_line = \"Sch E L14\"\n",
        header("org.example.taxes")
    );
    install(&mut b, &content);
    let a = b.run("account.get", json!({"account": "Rental:Repairs"}));
    assert_eq!(a["tax_line"], "Sch E L14");
    // The parent made for the path is not tagged.
    let p = b.run("account.get", json!({"account": "Rental"}));
    assert!(p["tax_line"].is_null());
    let out = b.run(
        "definition.export",
        json!({"id": "org.example.taxes", "version": "1.0.0", "name": "Taxes",
               "categories": ["Rental:Repairs"]}),
    );
    let text = out["content"].as_str().unwrap();
    assert!(text.contains("tax_line = \"Sch E L14\""), "{text}");
    // Too long a tax line is an error.
    let bad = format!(
        "{}\n[[category]]\nkey = \"x\"\npath = \"X\"\ntype = \"expense\"\ntax_line = \"{}\"\n",
        header("org.example.bad"),
        "y".repeat(41)
    );
    let v = inspect(&mut b, &bad);
    assert!(errors(&v).contains("tax_line"), "{v}");
}
