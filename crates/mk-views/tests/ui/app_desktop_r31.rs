//! app-desktop.md revision 31 (tests 184–186) and dashboards.md acceptance tests
//! 1–15 in the desktop app.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_arithmetic
)]

use crate::common::*;
use mk_ui::View;
use mk_views::testing::Role;
use mk_views::testing::{Key, Modifiers};
use serde_json::{Value, json};

type H = mk_views::testing::Harness;

const TODAY: &str = "2026-10-20";

fn book() -> (H, TestBook) {
    let (mut h, b) = harness_with_chart();
    h.state_mut().set_today(TODAY);
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-01", "payee": "Opening", "splits": [
            {"account": "Assets:Checking", "value": "1000.00"},
            {"account": "Opening Balances", "value": "-1000.00"}]}),
    );
    h.run();
    (h, b)
}

fn spend(h: &mut H, date: &str, payee: &str, amount: &str) {
    cmd(
        h,
        "txn.create",
        json!({"date": date, "payee": payee, "splits": [
            {"account": "Expenses:Groceries", "value": amount},
            {"account": "Assets:Checking", "value": format!("-{amount}")}]}),
    );
}

fn dashboard(h: &mut H, name: &str) -> String {
    cmd(h, "dashboard.create", json!({"name": name}))["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn add(h: &mut H, dash: &str, kind: &str, title: Option<&str>, settings: Value) -> String {
    let mut args = json!({"dashboard": dash, "type": kind, "settings": settings});
    if let Some(t) = title {
        args["title"] = json!(t);
    }
    cmd(h, "dashboard.add_block", args)["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Edit layout from the dashboard's ⋯ menu (dashboards.md revision 3).
fn edit_layout(h: &mut H) {
    click(h, "Dashboard menu");
    click(h, "Edit layout");
}

/// The Dashboards pane, opened with its rail tab unless it is already showing.
fn show_dashboards(h: &mut H) {
    if !has(h, "+ New dashboard") {
        open_pane(h, "Dashboards");
    }
}

fn show(h: &mut H, name: &str) {
    h.state_mut().open_view_spec(&format!("dashboard:{name}"));
    h.run();
}

fn layout(h: &mut H, dash: &str) -> Vec<(String, u64, u64, u64, u64)> {
    let d = cmd(h, "dashboard.get", json!({"dashboard": dash}));
    d["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["title"]
                    .as_str()
                    .unwrap_or(b["type"].as_str().unwrap())
                    .to_owned(),
                b["x"].as_u64().unwrap(),
                b["y"].as_u64().unwrap(),
                b["w"].as_u64().unwrap(),
                b["h"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn audit_len(h: &mut H) -> usize {
    cmd(h, "audit.list", json!({"limit": 10000}))
        .as_array()
        .unwrap()
        .len()
}

/// Two note blocks A (0,0,4,2) and B (4,0,4,2) on "Church".
fn two_notes(h: &mut H) -> String {
    let dash = dashboard(h, "Church");
    add(h, &dash, "note", Some("A"), json!({"text": "first"}));
    add(h, &dash, "note", Some("B"), json!({"text": "second"}));
    let d = cmd(h, "dashboard.get", json!({"dashboard": dash}));
    let ids: Vec<String> = d["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap().to_owned())
        .collect();
    cmd(
        h,
        "dashboard.move_blocks",
        json!({"dashboard": dash, "layout": [
            {"block": ids[0], "x": 0, "y": 0, "w": 4, "h": 2},
            {"block": ids[1], "x": 4, "y": 0, "w": 4, "h": 2}]}),
    );
    show(h, "Church");
    dash
}

#[test]
fn app_desktop_at184_home_dashboard_shows_the_home_information() {
    let (mut h, _b) = book();
    spend(&mut h, "2026-10-03", "Grocer", "100.00");
    cmd(
        &mut h,
        "schedule.create",
        json!({"name": "Water", "template": {"payee": "Water", "splits": [
            {"account": "Expenses:Groceries", "value": "20.00"},
            {"account": "Assets:Checking", "value": "-20.00"}]},
            "start": "2026-11-25", "recurrence": {"frequency": "once"}}),
    );
    h.state_mut().navigate(View::Home);
    h.run();
    // Net worth, the attention list, the account balances by group and Coming up.
    assert!(
        has(&h, "Net worth 900.00 USD"),
        "{:?}",
        labels_containing(&h, "Net worth")
    );
    assert!(has(&h, "Assets 900.00  ·  Liabilities 0.00"));
    assert!(has(&h, "Attention"));
    assert!(has(&h, "Nothing needs your attention."));
    assert!(has(&h, "Cash & bank group"));
    assert!(has(&h, "Cash & bank balance 900.00"));
    assert!(has(&h, "Assets:Checking register"));
    assert!(has(&h, "Coming up"));
    assert!(has_contains(&h, "Scheduled Water due 2026-11-25"));
    // It is a dashboard: its header has the ⋯ menu with Edit layout and + Add
    // block (revision 3).
    click(&mut h, "Dashboard menu");
    assert!(has(&h, "Edit layout"));
    assert!(has(&h, "+ Add block"));
    key(&mut h, Key::Escape);
    // An account name opens its register.
    click(&mut h, "Assets:Checking register");
    assert!(matches!(h.state().view(), View::Register { .. }));
}

#[test]
fn dashboards_at1_new_book_opens_on_home() {
    let (mut h, _b) = book();
    assert_eq!(h.state().view(), &View::Home);
    let list = cmd(&mut h, "dashboard.list", json!({}));
    assert_eq!(list[0]["name"], "Home");
    assert_eq!(list[0]["startup"], true);
    for title in ["Net worth", "Attention", "Account balances", "Coming up"] {
        assert!(has(&h, title), "{title}");
    }
}

#[test]
fn dashboards_at2_account_transactions_open_the_register_with_the_row_selected() {
    let (mut h, _b) = book();
    for day in 2..=8 {
        spend(
            &mut h,
            &format!("2026-10-{day:02}"),
            &format!("Shop {day}"),
            "10.00",
        );
    }
    let dash = dashboard(&mut h, "Church");
    add(
        &mut h,
        &dash,
        "account_transactions",
        None,
        json!({"account": "Assets:Checking", "mode": "recent", "n": 5}),
    );
    show(&mut h, "Church");
    assert!(has(&h, "Checking balance 930.00"));
    let rows: Vec<String> = h
        .query_all_by_role(Role::Button)
        .filter_map(|n| n.accesskit_node().label())
        .filter(|l| l.starts_with("2026-10-0"))
        .collect();
    assert_eq!(
        rows,
        [
            "2026-10-08 Shop 8 -10.00",
            "2026-10-07 Shop 7 -10.00",
            "2026-10-06 Shop 6 -10.00",
            "2026-10-05 Shop 5 -10.00",
            "2026-10-04 Shop 4 -10.00"
        ]
    );
    click(&mut h, "2026-10-06 Shop 6 -10.00");
    assert!(matches!(h.state().view(), View::Register { .. }));
    h.run();
    let selected: Vec<String> = h
        .query_all_by_role(Role::Button)
        .filter(|n| n.accesskit_node().toggled() == Some(mk_views::testing::Toggled::True))
        .filter_map(|n| n.accesskit_node().label())
        .filter(|l| l.starts_with("Row "))
        .collect();
    assert_eq!(selected.len(), 1, "{selected:?}");
    assert!(selected[0].contains("Shop 6"), "{selected:?}");
}

#[test]
fn dashboards_at3_above_an_amount_in_the_app() {
    let (mut h, _b) = book();
    spend(&mut h, "2026-10-05", "Small", "20.00");
    spend(&mut h, "2026-10-06", "Big", "600.00");
    let dash = dashboard(&mut h, "Church");
    add(
        &mut h,
        &dash,
        "account_transactions",
        None,
        json!({"account": "Assets:Checking", "mode": "above", "amount": "500.00",
               "direction": "out", "days": 30}),
    );
    show(&mut h, "Church");
    assert!(has(&h, "2026-10-06 Big -600.00"));
    assert!(!has_contains(&h, "Small"));
    assert!(
        !has_contains(&h, "2026-10-01 Opening"),
        "money in is left out"
    );
}

#[test]
fn dashboards_at4_scheduled_enter_and_undo() {
    let (mut h, _b) = book();
    for (name, start) in [
        ("Late", "2026-10-15"),
        ("Water", "2026-10-25"),
        ("Phone", "2026-11-02"),
        ("Gym", "2026-11-09"),
    ] {
        cmd(
            &mut h,
            "schedule.create",
            json!({"name": name, "template": {"payee": name, "splits": [
                {"account": "Expenses:Groceries", "value": "20.00"},
                {"account": "Assets:Checking", "value": "-20.00"}]},
                "start": start, "recurrence": {"frequency": "once"}}),
        );
    }
    let dash = dashboard(&mut h, "Church");
    add(
        &mut h,
        &dash,
        "scheduled",
        None,
        json!({"mode": "next", "n": 3}),
    );
    show(&mut h, "Church");
    assert!(has(&h, "Scheduled Late due 2026-10-15, -20.00, overdue"));
    assert!(has_contains(&h, "Scheduled Water due 2026-10-25"));
    assert!(has_contains(&h, "Scheduled Phone due 2026-11-02"));
    assert!(!has_contains(&h, "Scheduled Gym"), "only the next three");
    let before = cmd(&mut h, "txn.list", json!({})).as_array().unwrap().len();
    click(&mut h, "Enter Late due 2026-10-15");
    assert_eq!(
        cmd(&mut h, "txn.list", json!({})).as_array().unwrap().len(),
        before + 1
    );
    assert!(has_contains(&h, "Scheduled Gym"), "the next one moves up");
    h.get_by_role_and_label(Role::Button, "Undo").click();
    h.run();
    assert_eq!(
        cmd(&mut h, "txn.list", json!({})).as_array().unwrap().len(),
        before
    );
}

#[test]
fn dashboards_at5_chart_draws_the_spending_report_and_opens_it() {
    let (mut h, _b) = book();
    spend(&mut h, "2026-10-03", "Grocer", "45.50");
    spend(&mut h, "2026-09-03", "Old", "99.00");
    let dash = dashboard(&mut h, "Church");
    add(
        &mut h,
        &dash,
        "chart",
        None,
        json!({"chart": "spending", "period": "this_month"}),
    );
    show(&mut h, "Church");
    let block_chart = labels_containing(&h, "Spending chart:");
    assert_eq!(block_chart, ["Spending chart: Expenses:Groceries 45.50"]);
    click(&mut h, "Spending by category");
    assert_eq!(h.state().view(), &View::Reports);
    h.run();
    assert_eq!(
        h.get_by_role_and_label(Role::Tab, "Spending")
            .accesskit_node()
            .toggled(),
        Some(mk_views::testing::Toggled::True)
    );
    // The report draws the same values for the same period.
    assert_eq!(labels_containing(&h, "Spending chart:"), block_chart);
}

#[test]
fn dashboards_at6_shortcut_buttons() {
    let (mut h, _b) = book();
    cmd(&mut h, "module.enable", json!({"module": "expenses"}));
    let church = dashboard(&mut h, "Church");
    let office = dashboard(&mut h, "Office");
    add(
        &mut h,
        &office,
        "shortcuts",
        None,
        json!({"buttons": [
            {"label": "Checking", "target": {"kind": "account", "account": "Assets:Checking"}},
            {"label": "New expense", "target": {"kind": "action", "action": "new_expense"}},
            {"label": "Church", "target": {"kind": "dashboard", "dashboard": "Church"}}]}),
    );
    let txns = cmd(&mut h, "txn.list", json!({})).as_array().unwrap().len();
    let audit = audit_len(&mut h);
    show(&mut h, "Office");
    click(&mut h, "Checking");
    assert!(matches!(h.state().view(), View::Register { .. }));
    show(&mut h, "Office");
    click(&mut h, "New expense");
    assert_eq!(h.state().view(), &View::Expenses);
    assert!(has(&h, "Merchant"), "the New expense form is open");
    click(&mut h, "Cancel");
    show(&mut h, "Office");
    click(&mut h, "Church");
    assert_eq!(h.state().view(), &View::Dashboard { id: church });
    // Nothing was posted or changed by the shortcuts themselves.
    assert_eq!(
        cmd(&mut h, "txn.list", json!({})).as_array().unwrap().len(),
        txns
    );
    assert_eq!(audit_len(&mut h), audit);
}

/// Revision 31 test 185 / dashboards.md 7: dragging a 4 × 2 block onto another.
#[test]
fn app_desktop_at185_dragging_a_block_moves_the_other_down_and_done_is_one_undo_step() {
    let (mut h, _b) = book();
    let dash = two_notes(&mut h);
    let before = layout(&mut h, &dash);
    edit_layout(&mut h);
    let a = h.get_by_label("Block A").rect();
    let b = h.get_by_label("Block B").rect();
    let from = a.center();
    let to = b.center();
    h.drag_at(from);
    h.run();
    h.hover_at(from + mk_views::testing::vec2(10.0, 0.0));
    h.run();
    h.hover_at(to);
    h.run();
    h.drop_at(to);
    h.run();
    let audit = audit_len(&mut h);
    click(&mut h, "Done");
    assert_eq!(audit_len(&mut h), audit + 1, "Done saves one change");
    let after = layout(&mut h, &dash);
    assert_eq!(after[0], ("A".to_owned(), 4, 0, 4, 2));
    assert_eq!(after[1], ("B".to_owned(), 4, 2, 4, 2), "B moved down");
    h.get_by_role_and_label(Role::Button, "Undo").click();
    h.run();
    assert_eq!(layout(&mut h, &dash), before);
}

#[test]
fn dashboards_at8_keyboard_and_menu_move_and_resize() {
    let (mut h, _b) = book();
    let dash = two_notes(&mut h);
    edit_layout(&mut h);
    h.get_by_label("Block A").focus();
    h.run();
    key(&mut h, Key::ArrowDown); // nothing below A: stays
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowDown);
    h.run();
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowLeft);
    h.run();
    // The block menu does the same.
    click(&mut h, "Menu for A");
    click(&mut h, "Wider");
    click(&mut h, "Done");
    let l = layout(&mut h, &dash);
    assert_eq!(
        l[0],
        ("A".to_owned(), 0, 0, 4, 3),
        "taller, narrower then wider"
    );
    // Arrow keys move it.
    edit_layout(&mut h);
    h.get_by_label("Block B").focus();
    h.run();
    key(&mut h, Key::ArrowRight);
    click(&mut h, "Menu for B");
    click(&mut h, "Move right");
    click(&mut h, "Done");
    let l = layout(&mut h, &dash);
    assert_eq!((l[1].1, l[1].2), (6, 0), "{l:?}");
}

#[test]
fn dashboards_at10_narrow_windows_stack_blocks_without_changing_the_layout() {
    let (mut h, _b) = book();
    let dash = two_notes(&mut h);
    let saved = layout(&mut h, &dash);
    h.set_size(mk_views::testing::vec2(700.0, 900.0));
    h.run();
    let a = h.get_by_label("A").rect();
    let b = h.get_by_label("B").rect();
    assert!(b.top() > a.bottom(), "one column: {a:?} {b:?}");
    assert!((a.left() - b.left()).abs() < 1.0);
    h.set_size(mk_views::testing::vec2(1400.0, 900.0));
    h.run();
    let a = h.get_by_label("A").rect();
    let b = h.get_by_label("B").rect();
    assert!(
        (a.top() - b.top()).abs() < 1.0 && b.left() > a.right(),
        "side by side again"
    );
    assert_eq!(layout(&mut h, &dash), saved);
}

#[test]
fn dashboards_at11_set_as_startup_reopen_and_delete() {
    let (mut h, b) = book();
    dashboard(&mut h, "Church");
    open_pane(&mut h, "Dashboards");
    click(&mut h, "Menu for dashboard Church");
    click(&mut h, "Set as startup");
    let path = b.path.to_str().unwrap().to_owned();
    h.state_mut().close_book();
    h.run();
    h.state_mut().open_book(&path);
    h.run();
    assert_eq!(h.state().view(), &View::Home);
    assert!(has(&h, "Church"), "the book opens on Church");
    show_dashboards(&mut h);
    click(&mut h, "Menu for dashboard Church");
    click(&mut h, "Delete…");
    click(&mut h, "Delete");
    let list = cmd(&mut h, "dashboard.list", json!({}));
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["startup"], true, "Home is the startup again");
}

#[test]
fn dashboards_at12_rail_tab_first_hover_lists_names_pane_stars_startup() {
    let (mut h, _b) = book();
    dashboard(&mut h, "Church");
    h.run();
    let mut tabs: Vec<(f32, String)> = h
        .query_all_by_role(Role::Tab)
        .filter(|n| n.rect().right() < 60.0)
        .map(|n| {
            (
                n.rect().top(),
                n.accesskit_node().label().unwrap_or_default(),
            )
        })
        .collect();
    tabs.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(tabs[0].1, "Dashboards");
    assert_eq!(tabs[1].1, "All accounts");
    rail_tab(&h, "Dashboards").hover();
    h.run_steps(30);
    assert!(
        labels_containing(&h, "Dashboards\n")
            .iter()
            .any(|l| l.contains("Home") && l.contains("Church")),
        "{:?}",
        labels_containing(&h, "Church")
    );
    if !has(&h, "+ New dashboard") {
        open_pane(&mut h, "Dashboards");
    }
    let stars = h.query_all_by_label("Startup dashboard").count();
    assert_eq!(stars, 1);
    let star = h.get_by_label("Startup dashboard").rect();
    let home = h.get_by_label("Dashboard Home").rect();
    assert!(
        (star.center().y - home.center().y).abs() < 4.0,
        "the star is on Home's line"
    );
}

/// Revision 31 test 186: the Dashboards tab opens the list.
#[test]
fn app_desktop_at186_dashboards_tab_opens_the_list() {
    let (mut h, _b) = book();
    open_pane(&mut h, "Dashboards");
    assert!(has(&h, "Dashboard Home"));
    assert!(has(&h, "+ New dashboard"));
    click(&mut h, "+ New dashboard");
    type_into(&mut h, "Dashboard name", "Office");
    click(&mut h, "Create");
    let list = cmd(&mut h, "dashboard.list", json!({}));
    let office = list
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "Office")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(h.state().view(), &View::Dashboard { id: office });
    // Clicking a name opens it; Home is the startup dashboard (the Home view).
    // (Whether the unpinned pane has slid away by now depends on the harness's
    // timing; the Dashboards tab opens it when it has.)
    show_dashboards(&mut h);
    click(&mut h, "Dashboard Home");
    assert_eq!(h.state().view(), &View::Home);
}

#[test]
fn dashboards_add_block_through_the_picker() {
    let (mut h, _b) = book();
    let dash = dashboard(&mut h, "Church");
    show(&mut h, "Church");
    click(&mut h, "Dashboard menu");
    click(&mut h, "+ Add block");
    assert!(has_contains(&h, "A short text of your own"));
    click(&mut h, "Note");
    type_into(&mut h, "Note text", "Offerings on Monday");
    click(&mut h, "Save");
    assert!(has(&h, "Offerings on Monday"));
    let d = cmd(&mut h, "dashboard.get", json!({"dashboard": dash}));
    assert_eq!(d["blocks"][0]["type"], "note");
    // Remove from the block's menu.
    click(&mut h, "Menu for Note");
    click(&mut h, "Remove");
    let d = cmd(&mut h, "dashboard.get", json!({"dashboard": dash}));
    assert_eq!(d["blocks"].as_array().unwrap().len(), 0);
}

#[test]
fn dashboards_at13_module_off_message_in_the_block() {
    let (mut h, _b) = book();
    cmd(&mut h, "module.enable", json!({"module": "budgets"}));
    let dash = dashboard(&mut h, "Church");
    add(
        &mut h,
        &dash,
        "chart",
        None,
        json!({"chart": "budget", "period": "this_month"}),
    );
    cmd(&mut h, "module.disable", json!({"module": "budgets"}));
    show(&mut h, "Church");
    // (The arrow is written ">" when the interface font has no arrow glyph.)
    assert!(
        has(&h, "Turn on Budgets in Settings → Features to show this")
            || has(&h, "Turn on Budgets in Settings > Features to show this"),
        "{:?}",
        labels_containing(&h, "Turn on")
    );
    let d = cmd(&mut h, "dashboard.get", json!({"dashboard": dash}));
    assert_eq!(d["blocks"][0]["settings"]["chart"], "budget");
}

#[test]
fn dashboards_at15_home_menu_resets_and_has_no_delete() {
    let (mut h, _b) = book();
    h.state_mut().navigate(View::Home);
    h.run();
    click(&mut h, "Menu for Attention");
    click(&mut h, "Remove");
    assert!(!has(&h, "Attention"));
    click(&mut h, "Dashboard menu");
    assert!(!has(&h, "Delete…"));
    click(&mut h, "Reset to standard");
    assert!(has(&h, "Attention"));
}

// ----- dashboards.md revision 3 (tests 16–19) ------------------------------------------

/// The header row: buttons whose vertical middle lies within the heading's line.
fn header_buttons(h: &H, heading: &str) -> Vec<(String, mk_views::testing::Rect)> {
    let head = h.get_by_label(heading).rect();
    let mut v: Vec<(String, mk_views::testing::Rect)> = h
        .query_all_by_role(Role::Button)
        .filter(|n| {
            let r = n.rect();
            r.left() >= head.left() - 1.0
                && r.center().y >= head.top() - 2.0
                && r.center().y <= head.bottom() + 2.0
        })
        .map(|n| (n.accesskit_node().label().unwrap_or_default(), n.rect()))
        .collect();
    v.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
    v
}

#[test]
fn dashboards_at16_header_has_only_the_name_and_the_menu_button_at_the_right() {
    let (mut h, _b) = book();
    let dash = dashboard(&mut h, "Church");
    add(&mut h, &dash, "note", Some("A"), json!({"text": "Hello"}));
    show(&mut h, "Church");
    let buttons = header_buttons(&h, "Church");
    let names: Vec<&str> = buttons.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["Dashboard menu"], "{buttons:?}");
    assert!(!has(&h, "Edit layout") && !has(&h, "+ Add block"));
    // The menu button is the rightmost thing on the dashboard (blocks included).
    let menu = buttons[0].1;
    let block = h.get_by_label("Menu for A").rect();
    assert!(menu.right() >= block.right() - 2.0, "{menu:?} {block:?}");
}

#[test]
fn dashboards_at17_edit_layout_and_add_block_in_the_menu_done_beside_it() {
    let (mut h, _b) = book();
    let dash = dashboard(&mut h, "Church");
    add(&mut h, &dash, "note", Some("A"), json!({"text": "Hello"}));
    show(&mut h, "Church");
    click(&mut h, "Dashboard menu");
    // Edit layout and + Add block come first (menu items on Slint, buttons on the
    // old toolkit).
    let mut items: Vec<(f32, String)> = h
        .query_all_by_role(Role::MenuItem)
        .filter_map(|n| {
            let l = n.accesskit_node().label().unwrap_or_default();
            [
                "Edit layout",
                "+ Add block",
                "Rename…",
                "Duplicate…",
                "Set as startup",
                "Delete…",
            ]
            .contains(&l.as_str())
            .then(|| (n.rect().top(), l))
        })
        .collect();
    items.sort_by(|a, b| a.0.total_cmp(&b.0));
    let order: Vec<&str> = items.iter().map(|(_, l)| l.as_str()).collect();
    assert_eq!(
        order,
        [
            "Edit layout",
            "+ Add block",
            "Rename…",
            "Duplicate…",
            "Set as startup",
            "Delete…"
        ]
    );
    click(&mut h, "Edit layout");
    let buttons = header_buttons(&h, "Church");
    let names: Vec<&str> = buttons.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["Done", "Dashboard menu"], "{buttons:?}");
    let gap = buttons[1].1.left() - buttons[0].1.right();
    assert!(
        (0.0..=16.0).contains(&gap),
        "Done sits just left of the menu: {gap}"
    );
    click(&mut h, "Done");
    assert!(!has(&h, "Done"));
}

#[test]
fn dashboards_at18_button_icons_default_by_target_and_chosen() {
    let (mut h, _b) = book();
    let dash = dashboard(&mut h, "Office");
    let block = add(
        &mut h,
        &dash,
        "shortcuts",
        Some("Go"),
        json!({"buttons": [
            {"label": "Spending", "target": {"kind": "report", "report": "spending"}},
            {"label": "Phone", "target": {"kind": "action", "action": "collect_from_phone"}},
            {"label": "Mine", "icon": "church", "target": {"kind": "screen", "screen": "home"}}]}),
    );
    let data = cmd(
        &mut h,
        "dashboard.block_data",
        json!({"dashboard": dash, "block": block, "today": TODAY}),
    );
    let icons: Vec<&str> = data["buttons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["icon"].as_str().unwrap())
        .collect();
    assert_eq!(icons, ["report", "camera", "church"]);
    // Choosing an icon in the button editor.
    show(&mut h, "Office");
    click(&mut h, "Menu for Go");
    click(&mut h, "Settings…");
    click(&mut h, "Button 1 icon: Default");
    assert!(has(&h, "Icon calendar") && has(&h, "Icon receipt") && has(&h, "Icon house"));
    click(&mut h, "Icon calendar");
    assert!(has(&h, "Button 1 icon: calendar"));
    click(&mut h, "Save");
    let d = cmd(&mut h, "dashboard.get", json!({"dashboard": dash}));
    let buttons = &d["blocks"][0]["settings"]["buttons"];
    assert_eq!(buttons[0]["icon"], "calendar");
    assert_eq!(buttons[2]["icon"], "church");
    assert!(buttons[1].get("icon").is_none(), "{buttons}");
    // The dashboard's own icon from the Rename dialog.
    click(&mut h, "Dashboard menu");
    click(&mut h, "Rename…");
    click(&mut h, "Dashboard icon: None");
    click(&mut h, "Icon briefcase");
    click(&mut h, "Save");
    let d = cmd(&mut h, "dashboard.get", json!({"dashboard": dash}));
    assert_eq!(d["icon"], "briefcase");
}

#[test]
fn dashboards_at19_button_sizes() {
    let (mut h, _b) = book();
    let dash = dashboard(&mut h, "Office");
    let mut heights = Vec::new();
    for (size, label) in [
        ("small", "Small one"),
        ("medium", "Medium one"),
        ("large", "Large one"),
    ] {
        add(
            &mut h,
            &dash,
            "shortcuts",
            Some(size),
            json!({"button_size": size, "buttons": [
                {"label": label, "target": {"kind": "screen", "screen": "home"}}]}),
        );
        show(&mut h, "Office");
        let r = h.get_by_role_and_label(Role::Button, label).rect();
        heights.push((size, r));
    }
    let line = 16.0_f32; // the body text height in points at the default style
    let (s, m, l) = (heights[0].1, heights[1].1, heights[2].1);
    // Small is a standard control: 32 points on the Slint 8-pixel grid (the old
    // toolkit's buttons were a text line tall).
    assert!(s.height() <= 32.5, "small: {s:?}");
    assert!(
        m.height() > s.height() + 8.0 && m.height() < line * 3.0,
        "medium: {m:?}"
    );
    assert!(l.height() > line * 3.5, "large: {l:?}");
    // Large is a square-ish tile.
    assert!(l.width() / l.height() < 1.6, "{l:?}");
    // The default size is Medium.
    let block = add(
        &mut h,
        &dash,
        "shortcuts",
        None,
        json!({"buttons": [{"label": "Plain", "target": {"kind": "screen", "screen": "home"}}]}),
    );
    let d = cmd(&mut h, "dashboard.get", json!({"dashboard": dash}));
    let b = d["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["id"] == block)
        .unwrap()
        .clone();
    assert_eq!(b["settings"]["button_size"], "medium");
}
