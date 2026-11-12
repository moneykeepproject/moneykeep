//! projects.md on the desktop: Features → Projects (off by default), the new
//! project form, the list with its % used bar, the detail tabs, Set project on
//! transactions… (one undoable change), Settings → Projects, the Home → Attention
//! budget alert, and the invoice editor's unbilled costs bar with its Add dialog.
//! subscriptions.md revision 2: Settings → Subscriptions → Make a bill creates, and
//! a Bill schedule entered through the bill editor.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::View;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

fn spend(h: &mut H, date: &str, payee: &str, amount: &str, project: Option<&str>) -> String {
    let neg = match amount.strip_prefix('-') {
        Some(x) => x.to_owned(),
        None => format!("-{amount}"),
    };
    let mut split = json!({"account": "Expenses:Groceries", "value": amount});
    if let Some(p) = project {
        split["project"] = json!(p);
    }
    cmd(
        h,
        "txn.create",
        json!({"date": date, "payee": payee, "splits": [split,
            {"account": "Liabilities:Visa", "value": neg}]}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn choose(h: &mut H, combo: &str, option: &str) {
    click(h, combo);
    h.get_by_role_and_label(Role::Button, option).click();
    h.run();
}

fn spent(h: &mut H, name: &str) -> String {
    let l = cmd(h, "project.list", json!({}));
    l["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap()["spent"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn projects_screen_form_list_and_detail() {
    let (mut h, _b) = harness_with_chart();
    h.state_mut().set_today("2026-06-30");
    assert!(!feature_items(&mut h).contains(&"Projects".to_owned()));
    cmd(&mut h, "module.enable", json!({"module": "projects"}));
    h.run();
    assert!(feature_items(&mut h).contains(&"Projects".to_owned()));
    open_feature(&mut h, "Projects");
    assert_eq!(h.state().view(), &View::Projects);
    assert!(has_contains(&h, "No projects yet."));
    click(&mut h, "+ New project");
    type_into(&mut h, "Name", "Roof repair");
    type_into(&mut h, "Budget amount", "5000");
    click(&mut h, "Save");
    let l = cmd(&mut h, "project.list", json!({}));
    assert_eq!(l["projects"][0]["budget"], "5000.00");
    spend(
        &mut h,
        "2026-05-02",
        "Home Depot",
        "1200.00",
        Some("Roof repair"),
    );
    spend(
        &mut h,
        "2026-05-10",
        "A1 Roofing",
        "3100.00",
        Some("Roof repair"),
    );
    spend(
        &mut h,
        "2026-05-12",
        "Home Depot",
        "-150.00",
        Some("Roof repair"),
    );
    h.run();
    // The detail opened on Summary.
    assert!(has(&h, "Budget vs actual") || has_contains(&h, "Expenses by account"));
    assert!(has(&h, "Share of the budget used"));
    click(&mut h, "← All projects");
    assert!(has(&h, "Open Roof repair"));
    assert!(has(&h, "Roof repair budget used"));
    assert!(has(&h, "83.0 %"), "{:?}", labels_containing(&h, "%"));
    // The tabs.
    click(&mut h, "Open Roof repair");
    h.get_by_role_and_label(Role::Tab, "Transactions").click();
    h.run();
    assert_eq!(
        labels_containing(&h, "Open in register: Home Depot").len(),
        2
    );
    h.get_by_role_and_label(Role::Tab, "Mileage").click();
    h.run();
    assert!(has_contains(&h, "No trips are tagged"));
    // Mark completed: it leaves the pickers, stays in the list.
    h.get_by_role_and_label(Role::Tab, "Summary").click();
    h.run();
    click(&mut h, "Mark completed");
    let p = cmd(&mut h, "project.get", json!({"project": "Roof repair"}));
    assert_eq!(p["status"], "completed");
    assert!(has(&h, "Reopen"));
}

#[test]
fn projects_set_project_settings_and_home_alert() {
    let (mut h, _b) = harness_with_chart();
    h.state_mut().set_today("2026-06-30");
    cmd(&mut h, "module.enable", json!({"module": "projects"}));
    cmd(
        &mut h,
        "project.create",
        json!({"name": "Roof repair", "budget": "1000"}),
    );
    for d in ["2026-06-01", "2026-06-02", "2026-06-03"] {
        spend(&mut h, d, "Hardware Store", "310.00", None);
    }
    h.run();
    open_feature(&mut h, "Projects");
    click(&mut h, "Open Roof repair");
    click(&mut h, "Set project on transactions…");
    assert!(
        has_contains(&h, "3 found"),
        "{:?}",
        labels_containing(&h, "found")
    );
    click(&mut h, "Select all");
    click(&mut h, "Set project on 3");
    assert_eq!(spent(&mut h, "Roof repair"), "930.00");
    // Home → Attention: 93 % of the budget.
    h.state_mut().navigate(View::Home);
    h.run();
    assert!(
        has(&h, "Roof repair has used 93 % of its budget"),
        "{:?}",
        labels_containing(&h, "Roof")
    );
    // One undoable change.
    cmd(&mut h, "history.undo", json!({}));
    assert_eq!(spent(&mut h, "Roof repair"), "0.00");
    // Settings → Projects.
    h.state_mut().navigate(View::Settings);
    h.run();
    settings_section(&mut h, "Projects");
    assert!(has(&h, "Budget warning at (%)"));
    assert!(has(&h, "Project required"));
}

#[test]
fn projects_invoice_add_unbilled_costs() {
    let (mut h, _b) = harness_with_chart();
    h.state_mut().set_today("2026-07-31");
    for m in ["projects", "invoicing"] {
        cmd(&mut h, "module.enable", json!({"module": m}));
    }
    cmd(
        &mut h,
        "party.create",
        json!({"name": "Smith Family", "roles": ["customer"]}),
    );
    cmd(
        &mut h,
        "project.create",
        json!({"name": "Roof repair", "customer": "Smith Family", "billable": true, "markup_percent": "10"}),
    );
    spend(
        &mut h,
        "2026-07-02",
        "Home Depot",
        "200.00",
        Some("Roof repair"),
    );
    spend(&mut h, "2026-07-03", "Lowes", "50.00", Some("Roof repair"));
    h.run();
    open_feature(&mut h, "Sales");
    h.get_by_role_and_label(Role::Tab, "Invoices").click();
    h.run();
    click(&mut h, "New invoice");
    choose(&mut h, "Customer", "Smith Family");
    assert!(
        has_contains(&h, "2 unbilled costs for Roof repair (250.00)"),
        "{:?}",
        labels_containing(&h, "unbilled")
    );
    click(&mut h, "Add unbilled costs for Roof repair");
    assert!(has(&h, "Add 2 (275.00)"));
    click(&mut h, "Add 2 (275.00)");
    assert!(!has_contains(&h, "unbilled costs for Roof repair"));
    click(&mut h, "Save and finalise");
    let list = cmd(&mut h, "invoice.list", json!({}));
    assert_eq!(list[0]["total"], "275.00");
    let g = cmd(&mut h, "project.get", json!({"project": "Roof repair"}));
    assert_eq!(g["billable_items"]["billed"].as_array().unwrap().len(), 2);
}

#[test]
fn subscriptions_make_bill_setting_and_bill_schedule() {
    let (mut h, _b) = harness_with_chart();
    h.state_mut().set_today("2026-09-01");
    for m in ["subscriptions", "bills"] {
        cmd(&mut h, "module.enable", json!({"module": m}));
    }
    for m in 3..=8 {
        let a = if m < 7 { "15.49" } else { "17.99" };
        cmd(
            &mut h,
            "txn.create",
            json!({"date": format!("2026-{m:02}-05"), "payee": "Netflix", "splits": [
                {"account": "Expenses:Groceries", "value": a},
                {"account": "Assets:Checking", "value": format!("-{a}")}]}),
        );
    }
    let s = cmd(&mut h, "subscription.suggestions", json!({}));
    cmd(
        &mut h,
        "subscription.answer",
        json!({"key": s[0]["key"], "answer": "subscription"}),
    );
    // Settings → Subscriptions shows the choice, default a repeating bill.
    h.state_mut().navigate(View::Settings);
    h.run();
    settings_section(&mut h, "Subscriptions");
    assert!(has(
        &h,
        "A repeating bill in Scheduled (posts when you enter it)"
    ));
    assert!(has(&h, "A bill now (posts to Accounts Payable)"));
    // Make a bill: a Bill schedule, nothing posted.
    let l = cmd(&mut h, "subscription.list", json!({"today": "2026-09-01"}));
    let id = l["subscriptions"][0]["id"].as_str().unwrap().to_owned();
    h.state_mut().open_view_spec(&format!("subscriptions:{id}"));
    h.run();
    click(&mut h, "Make a bill");
    assert!(has_contains(&h, "is a repeating bill in Scheduled"));
    assert!(
        cmd(&mut h, "bill.list", json!({}))
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Scheduled shows it marked Bill; Enter opens the bill editor.
    h.state_mut().navigate(View::Scheduled);
    h.run();
    assert!(has(&h, "Bill"), "{:?}", labels_containing(&h, "Bill"));
    click(&mut h, "Enter Netflix due 2026-09-05");
    assert_eq!(h.state().view(), &View::Purchases);
    assert!(has_contains(&h, "Entering a scheduled bill"));
    click(&mut h, "Save");
    let bills = cmd(&mut h, "bill.list", json!({}));
    assert_eq!(bills.as_array().unwrap().len(), 1);
    let sch = cmd(&mut h, "schedule.list", json!({}));
    assert_eq!(sch[0]["entered"], 1);
}

/// release-prep.md §4 (projects.md): the tag pickers' "Show closed" toggle, off by
/// default; when on, completed and cancelled projects are listed, marked.
#[test]
fn projects_tag_picker_show_closed() {
    let (mut h, _b) = harness_with_chart();
    for m in ["projects", "invoicing"] {
        cmd(&mut h, "module.enable", json!({"module": m}));
    }
    cmd(
        &mut h,
        "party.create",
        json!({"name": "Smith Family", "roles": ["customer"]}),
    );
    for p in ["Roof repair", "Old job", "Dropped job"] {
        cmd(&mut h, "project.create", json!({"name": p}));
    }
    cmd(
        &mut h,
        "project.set_status",
        json!({"project": "Old job", "status": "completed"}),
    );
    cmd(
        &mut h,
        "project.set_status",
        json!({"project": "Dropped job", "status": "cancelled"}),
    );
    h.run();
    open_feature(&mut h, "Sales");
    h.get_by_role_and_label(Role::Tab, "Invoices").click();
    h.run();
    click(&mut h, "New invoice");
    let toggle = |h: &H| {
        h.query_by_role_and_label(Role::CheckBox, "Show closed projects")
            .or_else(|| h.query_by_role_and_label(Role::CheckBox, "Show closed"))
            .is_some()
    };
    assert!(toggle(&h), "{:?}", labels_containing(&h, "closed"));
    // Off: open projects only.
    click(&mut h, "Project");
    assert!(
        h.query_by_role_and_label(Role::Button, "Roof repair")
            .is_some()
    );
    assert!(!has_contains(&h, "Old job"));
    h.get_by_role_and_label(Role::Button, "Roof repair").click();
    h.run();
    // On: the closed ones too, marked.
    match h.query_by_role_and_label(Role::CheckBox, "Show closed projects") {
        Some(c) => c.click(),
        None => h
            .get_by_role_and_label(Role::CheckBox, "Show closed")
            .click(),
    }
    h.run();
    assert_eq!(value(&h, "Project"), "Roof repair");
    choose(&mut h, "Project", "Old job (completed)");
    assert_eq!(value(&h, "Project"), "Old job (completed)");
    click(&mut h, "Project");
    assert!(
        h.query_by_role_and_label(Role::Button, "Dropped job (cancelled)")
            .is_some()
    );
}
