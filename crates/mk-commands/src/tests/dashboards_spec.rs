//! dashboards.md acceptance tests at the command level (1–5, 7, 9, 11, 13, 15; 14 is
//! in definitions_spec).

use super::*;

const TODAY: &str = "2026-10-20";

fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Rent", "type": "expense"}));
    b
}

fn spend(b: &mut Book, date: &str, payee: &str, category: &str, amount: &str) -> String {
    b.run(
        "txn.create",
        json!({"date": date, "payee": payee, "splits": [
            {"account": category, "value": amount},
            {"account": "Assets:Checking", "value": format!("-{amount}")}]}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn church(b: &mut Book) -> String {
    b.run(
        "dashboard.create",
        json!({"name": "Church", "icon": "church"}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn add(b: &mut Book, dash: &str, kind: &str, settings: Value) -> String {
    b.run(
        "dashboard.add_block",
        json!({"dashboard": dash, "type": kind, "settings": settings}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn data(b: &mut Book, dash: &str, block: &str) -> Value {
    b.run(
        "dashboard.block_data",
        json!({"dashboard": dash, "block": block, "today": TODAY}),
    )
}

#[test]
fn dashboards_at1_new_book_has_home_as_startup() {
    let mut b = Book::new();
    let list = b.run("dashboard.list", json!({}));
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["name"], "Home");
    assert_eq!(list[0]["startup"], true);
    assert_eq!(list[0]["home"], true);
    let home = b.run("dashboard.get", json!({"dashboard": "Home"}));
    let kinds: Vec<&str> = home["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["type"].as_str().unwrap())
        .collect();
    for k in ["net_worth", "balances", "attention"] {
        assert!(kinds.contains(&k), "{k} in {kinds:?}");
    }
    // Reading creates nothing: no audit entry.
    assert_eq!(b.audit_len(), 0);
    let nw = data(&mut b, "home", "net-worth");
    assert_eq!(nw["status"], "ok");
    assert_eq!(nw["net_worth"], "0.00");
    assert_eq!(nw["sparkline"].as_array().unwrap().len(), 12);
}

#[test]
fn dashboards_at2_recent_transactions_newest_first_with_balance() {
    let mut b = book();
    b.transfer(
        "2026-10-01",
        "Assets:Checking",
        "Opening Balances",
        "1000.00",
    );
    let mut ids = Vec::new();
    for day in 2..=8 {
        ids.push(spend(
            &mut b,
            &format!("2026-10-{day:02}"),
            &format!("Shop {day}"),
            "Expenses:Groceries",
            "10.00",
        ));
    }
    let dash = church(&mut b);
    let blk = add(
        &mut b,
        &dash,
        "account_transactions",
        json!({"account": "Assets:Checking", "mode": "recent", "n": 5}),
    );
    let d = data(&mut b, &dash, &blk);
    assert_eq!(d["title"], "Checking");
    assert_eq!(d["balance"], "930.00");
    let rows = d["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 5);
    let payees: Vec<&str> = rows.iter().map(|r| r["payee"].as_str().unwrap()).collect();
    assert_eq!(payees, ["Shop 8", "Shop 7", "Shop 6", "Shop 5", "Shop 4"]);
    assert_eq!(
        rows[0]["txn"],
        json!(ids[6]),
        "rows carry their transaction"
    );
    assert_eq!(rows[0]["category"], "Expenses:Groceries");
}

#[test]
fn dashboards_at3_above_an_amount() {
    let mut b = book();
    b.transfer(
        "2026-09-01",
        "Assets:Checking",
        "Opening Balances",
        "5000.00",
    );
    spend(&mut b, "2026-09-10", "Old rent", "Rent", "900.00"); // older than 30 days
    spend(&mut b, "2026-10-01", "Rent", "Rent", "900.00");
    spend(
        &mut b,
        "2026-10-05",
        "Groceries",
        "Expenses:Groceries",
        "120.00",
    );
    spend(&mut b, "2026-10-06", "Exactly", "Rent", "500.00");
    b.transfer(
        "2026-10-07",
        "Assets:Checking",
        "Opening Balances",
        "700.00",
    ); // money in
    let dash = church(&mut b);
    let blk = add(
        &mut b,
        &dash,
        "account_transactions",
        json!({"account": "Assets:Checking", "mode": "above", "amount": "500.00",
               "direction": "out", "days": 30}),
    );
    let rows = data(&mut b, &dash, &blk)["rows"]
        .as_array()
        .unwrap()
        .clone();
    let payees: Vec<&str> = rows
        .iter()
        .map(|r| r["payee"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(payees, ["Exactly", "Rent"]);
    // Either direction includes the deposit.
    b.run(
        "dashboard.update_block",
        json!({"dashboard": dash, "block": blk, "settings": {"account": "Assets:Checking",
            "mode": "above", "amount": "500", "direction": "either", "days": 30}}),
    );
    assert_eq!(
        data(&mut b, &dash, &blk)["rows"].as_array().unwrap().len(),
        3
    );
}

#[test]
fn dashboards_at4_scheduled_next_three_enter_and_undo() {
    let mut b = book();
    for (name, start) in [
        ("Overdue rent", "2026-10-10"),
        ("Water", "2026-10-25"),
        ("Phone", "2026-11-02"),
        ("Gym", "2026-11-20"),
    ] {
        b.run(
            "schedule.create",
            json!({"name": name, "template": {"payee": name, "splits": [
                {"account": "Rent", "value": "10.00"},
                {"account": "Assets:Checking", "value": "-10.00"}]},
                "start": start, "recurrence": {"frequency": "once"}}),
        );
    }
    let dash = church(&mut b);
    let blk = add(&mut b, &dash, "scheduled", json!({"mode": "next", "n": 3}));
    let rows = data(&mut b, &dash, &blk)["rows"]
        .as_array()
        .unwrap()
        .clone();
    let names: Vec<&str> = rows
        .iter()
        .map(|r| r["shown_payee"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Overdue rent", "Water", "Phone"]);
    assert_eq!(rows[0]["overdue"], true);
    assert_eq!(rows[1]["overdue"], false);
    assert_eq!(rows[0]["shown_account"], "Assets:Checking");
    // Enter (as the Scheduled screen does), then Undo removes it.
    let before = b.txn_count();
    b.run(
        "schedule.enter",
        json!({"schedule": rows[0]["schedule"], "date": rows[0]["nominal"]}),
    );
    assert_eq!(b.txn_count(), before + 1);
    let rows = data(&mut b, &dash, &blk)["rows"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(rows[0]["shown_payee"], "Water");
    b.run("history.undo", json!({}));
    assert_eq!(b.txn_count(), before);
}

#[test]
fn dashboards_at5_spending_chart_matches_the_report() {
    let mut b = book();
    b.transfer(
        "2026-10-01",
        "Assets:Checking",
        "Opening Balances",
        "2000.00",
    );
    spend(&mut b, "2026-10-02", "A", "Rent", "900.00");
    spend(&mut b, "2026-10-03", "B", "Expenses:Groceries", "45.50");
    spend(&mut b, "2026-09-03", "C", "Expenses:Groceries", "99.00");
    let dash = church(&mut b);
    let blk = add(
        &mut b,
        &dash,
        "chart",
        json!({"chart": "spending", "period": "this_month"}),
    );
    let d = data(&mut b, &dash, &blk);
    let report = b.run(
        "report.spending",
        json!({"preset": "this_month", "today": TODAY, "depth": 99}),
    );
    let items: Vec<(String, String)> = d["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["label"].as_str().unwrap().to_owned(),
                i["amount"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = report["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["full_name"].as_str().unwrap().to_owned(),
                r["amount"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(items, expected);
    assert_eq!(items[0], ("Rent".to_owned(), "900.00".to_owned()));
    assert_eq!(
        d["open"],
        json!({"report": "spending", "preset": "this_month"})
    );
    assert_eq!(d["title"], "Spending by category");
}

#[test]
fn dashboards_at7_move_blocks_is_one_undo_step() {
    let mut b = book();
    let dash = church(&mut b);
    let a = add(&mut b, &dash, "note", json!({"text": "A"}));
    let c = add(&mut b, &dash, "note", json!({"text": "C"}));
    let before = b.run("dashboard.get", json!({"dashboard": dash}));
    let n = b.audit_len();
    b.run(
        "dashboard.move_blocks",
        json!({"dashboard": dash, "layout": [
            {"block": a, "x": 4, "y": 0, "w": 4, "h": 2},
            {"block": c, "x": 4, "y": 2, "w": 4, "h": 2}]}),
    );
    assert_eq!(b.audit_len(), n + 1);
    let moved = b.run("dashboard.get", json!({"dashboard": dash}));
    assert_eq!(moved["blocks"][0]["x"], 4);
    b.run("history.undo", json!({}));
    let back = b.run("dashboard.get", json!({"dashboard": dash}));
    assert_eq!(back["blocks"], before["blocks"]);
}

#[test]
fn dashboards_at9_move_blocks_rejects_bad_layouts() {
    let mut b = book();
    let dash = church(&mut b);
    let a = add(&mut b, &dash, "note", json!({"text": "A"}));
    let c = add(
        &mut b,
        &dash,
        "chart",
        json!({"chart": "net_worth", "period": "this_year"}),
    );
    let mv = |b: &mut Book, layout: Value| {
        b.err(
            "dashboard.move_blocks",
            json!({"dashboard": dash, "layout": layout}),
        )
        .to_string()
    };
    let e = mv(
        &mut b,
        json!([{"block": a, "x": 0, "y": 0, "w": 4, "h": 2},
                {"block": c, "x": 2, "y": 1, "w": 6, "h": 4}]),
    );
    assert!(e.contains("overlap"), "{e}");
    let e = mv(
        &mut b,
        json!([{"block": a, "x": 10, "y": 0, "w": 4, "h": 2},
                {"block": c, "x": 0, "y": 2, "w": 6, "h": 4}]),
    );
    assert!(e.contains("inside the 12-column grid"), "{e}");
    let e = mv(
        &mut b,
        json!([{"block": a, "x": 0, "y": 0, "w": 4, "h": 2},
                {"block": c, "x": 4, "y": 0, "w": 3, "h": 2}]),
    );
    assert!(e.contains("at least 4 columns wide and 3 rows high"), "{e}");
    let e = mv(
        &mut b,
        json!([{"block": a, "x": 0, "y": 0, "w": 4, "h": 2}]),
    );
    assert!(e.contains("every block"), "{e}");
    // A good layout is accepted.
    b.run(
        "dashboard.move_blocks",
        json!({"dashboard": dash, "layout": [
            {"block": a, "x": 0, "y": 0, "w": 4, "h": 2},
            {"block": c, "x": 4, "y": 0, "w": 8, "h": 4}]}),
    );
}

#[test]
fn dashboards_at11_startup_and_deleting_it() {
    let mut b = book();
    let dash = church(&mut b);
    b.run("dashboard.set_startup", json!({"dashboard": "Church"}));
    let list = b.run("dashboard.list", json!({}));
    let church_row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == json!(dash))
        .unwrap()
        .clone();
    assert_eq!(church_row["startup"], true);
    assert_eq!(list[0]["startup"], false, "exactly one startup dashboard");
    let out = b.run("dashboard.delete", json!({"dashboard": dash}));
    assert_eq!(out["startup"], "home");
    let list = b.run("dashboard.list", json!({}));
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["startup"], true);
    // Undo brings Church back as the startup dashboard.
    b.run("history.undo", json!({}));
    let list = b.run("dashboard.list", json!({}));
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|d| d["name"] == "Church" && d["startup"] == true)
    );
}

#[test]
fn dashboards_at13_module_off_shows_message_and_keeps_settings() {
    let mut b = book();
    b.run("module.enable", json!({"module": "budgets"}));
    let dash = church(&mut b);
    let blk = add(
        &mut b,
        &dash,
        "chart",
        json!({"chart": "budget", "period": "this_month"}),
    );
    b.run("module.disable", json!({"module": "budgets"}));
    let d = data(&mut b, &dash, &blk);
    assert_eq!(d["status"], "module_off");
    assert_eq!(
        d["message"],
        "Turn on Budgets in Settings → Features to show this"
    );
    let g = b.run("dashboard.get", json!({"dashboard": dash}));
    assert_eq!(g["blocks"][0]["settings"]["chart"], "budget");
    b.run("module.enable", json!({"module": "budgets"}));
    let d = data(&mut b, &dash, &blk);
    assert_ne!(d["status"], "module_off");
}

#[test]
fn dashboards_at15_home_resets_and_cannot_be_deleted() {
    let mut b = book();
    let e = b.err("dashboard.delete", json!({"dashboard": "home"}));
    assert!(e.to_string().contains("can't be deleted"));
    let home = b.run("dashboard.get", json!({"dashboard": "home"}));
    let first = home["blocks"][0]["id"].as_str().unwrap().to_owned();
    b.run(
        "dashboard.remove_block",
        json!({"dashboard": "home", "block": first}),
    );
    b.run(
        "dashboard.update",
        json!({"dashboard": "home", "name": "Start"}),
    );
    assert_eq!(
        b.run("dashboard.get", json!({"dashboard": "home"}))["blocks"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    b.run("dashboard.reset_home", json!({}));
    let reset = b.run("dashboard.get", json!({"dashboard": "home"}));
    assert_eq!(reset["blocks"], home["blocks"]);
    assert_eq!(reset["name"], "Start", "reset puts the blocks back");
}

#[test]
fn dashboard_names_icons_and_settings_are_checked() {
    let mut b = book();
    church(&mut b);
    let e = b.err("dashboard.create", json!({"name": "church"}));
    assert!(e.to_string().contains("already a dashboard"));
    let e = b.err("dashboard.create", json!({"name": "x".repeat(61)}));
    assert!(e.to_string().contains("at most 60"));
    let e = b.err("dashboard.create", json!({"name": "Y", "icon": "rocket"}));
    assert!(e.to_string().contains("Unknown icon"));
    let e = b.err(
        "dashboard.add_block",
        json!({"dashboard": "Church", "type": "account_transactions",
               "settings": {"account": "Nowhere"}}),
    );
    assert!(e.to_string().contains("no account"));
    let e = b.err(
        "dashboard.add_block",
        json!({"dashboard": "Church", "type": "teleport"}),
    );
    assert!(e.to_string().contains("Unknown block type"));
    let e = b.err(
        "dashboard.add_block",
        json!({"dashboard": "Church", "type": "note", "settings": {"text": "x".repeat(2001)}}),
    );
    assert!(e.to_string().contains("2000"));
    // A note keeps its line breaks.
    let n = add(&mut b, "Church", "note", json!({"text": "One\nTwo"}));
    assert_eq!(data(&mut b, "Church", &n)["text"], "One\nTwo");
    // Copies get new block ids.
    let copy = b.run(
        "dashboard.create",
        json!({"name": "Copy", "from": "Church"}),
    );
    assert_ne!(copy["blocks"][0]["id"], json!(n));
    assert_eq!(copy["blocks"][0]["settings"]["text"], "One\nTwo");
}

#[test]
fn dashboard_blocks_report_missing_accounts_and_unknown_types() {
    let mut b = book();
    let tmp = b.account(json!({"name": "Temp", "type": "asset", "subtype": "bank"}));
    let dash = church(&mut b);
    let blk = add(
        &mut b,
        &dash,
        "account_transactions",
        json!({"account": tmp}),
    );
    b.run("account.delete", json!({"account": "Temp"}));
    let d = data(&mut b, &dash, &blk);
    assert_eq!(d["status"], "missing_account");
    assert_eq!(d["message"], "This account no longer exists");
    // A block from a newer version is kept and says so.
    let key = format!("dashboard.{dash}");
    let mut stored = b.s.get_setting("book", &key).unwrap().unwrap();
    stored["blocks"].as_array_mut().unwrap().push(
        json!({"id": "future", "type": "hologram", "settings": {"z": 1},
                     "x": 0, "y": 9, "w": 2, "h": 1, "glow": true}),
    );
    b.s.write(|tx| tx.set_setting("book", &key, &stored, "test"))
        .unwrap();
    let d = data(&mut b, &dash, "future");
    assert_eq!(d["status"], "unknown_type");
    assert_eq!(d["message"], "This block needs a newer MoneyKeep");
    // Changing another block keeps the unknown one unchanged.
    b.run(
        "dashboard.update_block",
        json!({"dashboard": dash, "block": blk, "title": "Gone"}),
    );
    let after = b.s.get_setting("book", &key).unwrap().unwrap();
    let fut = after["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == "future")
        .unwrap()
        .clone();
    assert_eq!(fut["glow"], true);
    assert_eq!(fut["settings"], json!({"z": 1}));
}

#[test]
fn dashboard_balances_attention_and_summary_blocks() {
    let mut b = book();
    b.transfer(
        "2026-10-01",
        "Assets:Checking",
        "Opening Balances",
        "1000.00",
    );
    b.transfer(
        "2026-10-02",
        "Expenses:Groceries",
        "Liabilities:Visa",
        "40.00",
    );
    let bal = data(&mut b, "home", "balances");
    let groups = bal["groups"].as_array().unwrap();
    assert_eq!(groups[0]["name"], "Cash & bank");
    assert_eq!(groups[0]["total"], "1000.00");
    assert_eq!(groups[0]["accounts"][0]["full_name"], "Assets:Checking");
    assert_eq!(groups[1]["name"], "Credit cards");
    assert_eq!(groups[1]["total"], "40.00");
    let att = data(&mut b, "home", "attention");
    // opening-balances.md: Visa has a transaction and no opening balance (Checking's
    // first transaction is against Opening Balances, an opening entry).
    assert_eq!(
        att["items"],
        json!([{"label": "1 account needs an opening balance", "target": {"screen": "opening"}}])
    );
    let dash = church(&mut b);
    let blk = add(
        &mut b,
        &dash,
        "report_summary",
        json!({"report": "profit_loss", "period": "this_month"}),
    );
    let d = data(&mut b, &dash, &blk);
    assert_eq!(d["label"], "Net income");
    assert_eq!(d["figure"], "-40.00");
    assert_eq!(d["previous"], "0.00");
    assert_eq!(d["previous_from"], "2026-09-01");
    let blk = add(
        &mut b,
        &dash,
        "report_summary",
        json!({"report": "ar_total", "period": "this_month"}),
    );
    assert_eq!(data(&mut b, &dash, &blk)["status"], "module_off");
    let sc = add(
        &mut b,
        &dash,
        "shortcuts",
        json!({"buttons": [
            {"label": "Checking", "target": {"kind": "account", "account": "Assets:Checking"}},
            {"label": "New expense", "target": {"kind": "action", "action": "new_expense"}},
            {"label": "Church", "target": {"kind": "dashboard", "dashboard": "Church"}}]}),
    );
    let d = data(&mut b, &dash, &sc);
    assert_eq!(d["buttons"][0]["target"]["full_name"], "Assets:Checking");
    assert_eq!(d["buttons"][2]["target"]["name"], "Church");
    assert_eq!(d["buttons"][1]["missing"], false);
}

#[test]
fn dashboard_commands_are_offered_over_mcp() {
    let r = Registry::with_builtins();
    for name in [
        "dashboard.list",
        "dashboard.get",
        "dashboard.create",
        "dashboard.update",
        "dashboard.delete",
        "dashboard.reorder",
        "dashboard.set_startup",
        "dashboard.reset_home",
        "dashboard.add_block",
        "dashboard.update_block",
        "dashboard.move_blocks",
        "dashboard.remove_block",
        "dashboard.block_data",
    ] {
        let spec = r.spec(name).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(spec.module, "core");
        assert_eq!(
            spec.mutates,
            !matches!(
                name,
                "dashboard.list" | "dashboard.get" | "dashboard.block_data"
            )
        );
        assert!(!NOT_OVER_MCP.contains(&name));
    }
}

#[test]
fn dashboard_reorder_needs_every_id() {
    let mut b = book();
    let dash = church(&mut b);
    let e = b.err("dashboard.reorder", json!({"ids": [dash]}));
    assert!(e.to_string().contains("every dashboard"));
    b.run("dashboard.reorder", json!({"ids": [dash, "home"]}));
    let list = b.run("dashboard.list", json!({}));
    assert_eq!(list[0]["name"], "Church");
    assert_eq!(list[1]["name"], "Home");
}

/// Revision 3: button sizes and icons (the extended symbol list, defaults by target).
#[test]
fn dashboards_rev3_button_sizes_and_icons() {
    let mut b = book();
    let dash = church(&mut b);
    let btn = |icon: Option<&str>| {
        let mut x = json!({"label": "Go", "target": {"kind": "action", "action": "import_file"}});
        if let Some(i) = icon {
            x["icon"] = json!(i);
        }
        x
    };
    let e = b.err(
        "dashboard.add_block",
        json!({"dashboard": dash, "type": "shortcuts",
            "settings": {"button_size": "huge", "buttons": [btn(None)]}}),
    );
    assert!(e.to_string().contains("button size"), "{e}");
    let e = b.err(
        "dashboard.add_block",
        json!({"dashboard": dash, "type": "shortcuts",
            "settings": {"buttons": [btn(Some("rocket"))]}}),
    );
    assert!(e.to_string().contains("Unknown icon"), "{e}");
    let id = add(
        &mut b,
        &dash,
        "shortcuts",
        json!({"button_size": "large", "buttons": [btn(None), btn(Some("receipt"))]}),
    );
    let d = data(&mut b, &dash, &id);
    assert_eq!(d["button_size"], "large");
    assert_eq!(d["buttons"][0]["icon"], "import");
    assert_eq!(d["buttons"][1]["icon"], "receipt");
    // Dashboards take the extended symbols too.
    b.run(
        "dashboard.update",
        json!({"dashboard": dash, "icon": "calculator"}),
    );
    let g = b.run("dashboard.get", json!({"dashboard": dash}));
    assert_eq!(g["icon"], "calculator");
}
