//! app-desktop.md revision 34 (chart-presets.md CP25-CP28): starter charts in the
//! Create a new book dialog, Add accounts from a chart…, account numbers and tax
//! lines, and the Tax summary report.
//!
//! Clicks go through the window's own hit testing (the pointer moves onto the
//! control and the button goes down and up there), as in revision 32.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_arithmetic
)]

use crate::common::*;
use mk_ui::ctl::App;
use mk_ui::{AppEvent, Screen, UserConfig, View};
use mk_views::testing::Harness;
use serde_json::{Value, json};

type H = Harness;

/// One mouse click on the element named `label` (after it is scrolled into view).
fn mouse(h: &mut H, label: &str) {
    let c = h.get_by_label(label).rect().center();
    h.hover_at(pos2(2.0, 2.0));
    h.run();
    h.hover_at(c);
    h.press_at(c);
    h.release_at(c);
    h.run();
}

/// Whether at least one element is named `label`.
fn any(h: &H, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

/// Turn the mouse wheel over the point `at` until `label` is on screen (scrolling
/// lists only build the rows in view).
fn wheel_to(h: &mut H, at: Pos2, label: &str) {
    for _ in 0..40 {
        if any(h, label) {
            return;
        }
        h.hover_at(at);
        h.window()
            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(at.x, at.y),
                delta_x: 0.0,
                delta_y: -30.0,
            });
        h.run();
    }
    panic!("{label} never came into view");
}

/// A point inside the chart list (its first row as first shown).
fn chart_list_point(h: &H) -> Pos2 {
    h.get_by_label("Chart Personal & Household").rect().center()
}

fn start_app() -> App {
    App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        UserConfig { ..iso_config() },
    )
}

/// Create a book from `chart` through the Create a new book dialog.
fn book_from(chart: &str) -> (H, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::with_size(start_app(), 1280.0, 1000.0);
    mouse(&mut h, "Create a New Book");
    if chart != "Personal & Household" {
        let at = chart_list_point(&h);
        let label = format!("Chart {chart}");
        wheel_to(&mut h, at, &label);
        mouse(&mut h, &label);
    }
    mouse(&mut h, "Create book…");
    let events = h.state_mut().take_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AppEvent::ChooseNewBookPath { .. })),
        "{events:?}"
    );
    let path = dir.path().join("b.moneykeep");
    h.state_mut()
        .create_book(path.to_str().unwrap(), "Books", "USD");
    h.views().refresh();
    h.run();
    assert_eq!(h.state().screen(), Screen::Main);
    (h, dir)
}

fn account(h: &mut H, code: &str) -> Value {
    cmd(h, "account.get", json!({"account": code}))
}

fn count(h: &mut H) -> usize {
    cmd(h, "account.list", json!({})).as_array().unwrap().len()
}

#[test]
fn app_desktop_at_cp25_create_dialog_lists_presets() {
    let mut h = Harness::with_size(start_app(), 1280.0, 1000.0);
    mouse(&mut h, "Create a New Book");
    assert_eq!(
        h.get_by_label("Chart Personal & Household").is_selected(),
        Some(true)
    );
    assert!(has_contains(&h, "Best for: tracking your own"));
    assert!(has_contains(&h, "Also turns on: Investments"));
    // Every preset, in one scrolling list.
    let at = chart_list_point(&h);
    for name in [
        "Personal & Household",
        "Personal — Form 1040",
        "Sole Proprietor — Schedule C",
        "Personal — Form 1040 + Schedule C",
        "S Corporation — Form 1120-S",
        "Partnership / Multi-Member LLC — Form 1065",
        "C Corporation — Form 1120",
        "Small Business",
        "Nonprofit / Church",
        "Starter Minimum",
    ] {
        wheel_to(&mut h, at, &format!("Chart {name}"));
    }
    // Selecting Small Business updates the panel.
    wheel_to(&mut h, at, "Chart Small Business");
    mouse(&mut h, "Chart Small Business");
    assert_eq!(
        h.get_by_label("Chart Small Business").is_selected(),
        Some(true)
    );
    assert!(has_contains(&h, "Best for: a business that wants a clean"));
    assert!(!has_contains(&h, "Best for: tracking your own"));
    // Show accounts lists 4500 Sales.
    mouse(&mut h, "Show accounts of Small Business");
    assert!(has(&h, "4500"));
    assert!(has(&h, "Sales"));
    mouse(&mut h, "Create book…");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sb.moneykeep");
    h.state_mut()
        .create_book(path.to_str().unwrap(), "Shop", "USD");
    h.views().refresh();
    h.run();
    assert_eq!(account(&mut h, "4500")["name"], "Sales");
    let info = cmd(&mut h, "book.info", json!({}));
    assert_eq!(info["chart"]["preset"], "small_business");
}

#[test]
fn app_desktop_at_cp25_book_without_the_dialog_has_no_chart() {
    let (app, _book) = new_book_app();
    let mut h = harness(app);
    assert_eq!(count(&mut h), 1);
}

#[test]
fn app_desktop_at_cp26_add_accounts_from_a_chart() {
    let (mut h, _dir) = book_from("Personal & Household");
    let before = count(&mut h);
    h.state_mut().navigate(View::Accounts);
    h.run();
    mouse(&mut h, "Add accounts from a chart…");
    let at = chart_list_point(&h);
    wheel_to(&mut h, at, "Chart Personal — Form 1040");
    mouse(&mut h, "Chart Personal — Form 1040");
    // The preview: skip reasons and the summary.
    assert!(any(&h, "Skipped: same code"));
    let at = h
        .get_all_by_label("Skipped: same code")
        .next()
        .unwrap()
        .rect()
        .center();
    wheel_to(&mut h, at, "Skipped: same code as Bonus & Commission");
    assert!(any(&h, "Skipped: same code as Bonus & Commission"));
    assert!(has_contains(&h, "already in this book"));
    // Untick one row, add the rest.
    let preview = cmd(
        &mut h,
        "chart.preview_add",
        json!({"preset": "personal_1040"}),
    );
    let adds = preview["adds"].as_u64().unwrap() as usize;
    wheel_to(&mut h, at, "Add Health Savings Account");
    mouse(&mut h, "Add Health Savings Account");
    assert!(has_contains(&h, "(1 unticked)"));
    mouse(&mut h, "Add accounts");
    assert!(!has(&h, "Add accounts from a chart"), "the dialog closes");
    assert_eq!(count(&mut h), before + adds - 1);
    let has_hsa = cmd(&mut h, "account.list", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["code"] == "1430");
    assert!(!has_hsa);
    // One undo step removes them.
    mouse(&mut h, "Undo");
    assert_eq!(count(&mut h), before);
}

#[test]
fn app_desktop_at_cp27_account_dialog_tax_line_and_role() {
    let (mut h, _dir) = book_from("Personal & Household");
    h.state_mut().navigate(View::Accounts);
    h.run();
    // Saving a tax line on an expense shows it in the tree row.
    let rent = account(&mut h, "6010");
    let full = rent["full_name"].as_str().unwrap().to_owned();
    assert!(has(&h, &format!("Account number of {full}: 6010")));
    mouse(&mut h, &format!("Actions for {full}"));
    mouse(&mut h, "Edit…");
    assert!(has(&h, "Account number"));
    let tax = h.get_by_label("Tax line");
    assert_eq!(
        tax.description().as_deref(),
        Some("Where this account's total goes on your return. A hint for reports, not tax advice.")
    );
    type_into(&mut h, "Tax line", "Sch E L3");
    mouse(&mut h, "Save");
    assert_eq!(account(&mut h, "6010")["tax_line"], "Sch E L3");
    assert!(has(&h, &format!("Tax line of {full}: Sch E L3")));
    // The Opening Balances dialog: the role line and a disabled Delete.
    mouse(&mut h, "Actions for Opening Balances");
    mouse(&mut h, "Edit…");
    assert!(has(&h, "Used by MoneyKeep for opening balances"));
    assert!(disabled(&h, "Delete"));
    assert_eq!(
        h.get_by_label("Delete").description().as_deref(),
        Some("Used by MoneyKeep for opening balances")
    );
}

#[test]
fn app_desktop_at_cp28_tax_summary_report() {
    let (mut h, _dir) = book_from("Personal — Form 1040");
    h.state_mut().set_today("2026-11-04");
    let groceries = account(&mut h, "6825")["full_name"]
        .as_str()
        .unwrap()
        .to_owned();
    let tax = account(&mut h, "6320")["full_name"]
        .as_str()
        .unwrap()
        .to_owned();
    for (date, debit, credit, amount) in [
        ("2025-02-01", "Checking", "4010", "3000.00"),
        ("2025-03-01", "Checking", "4010", "2000.00"),
        ("2025-04-01", tax.as_str(), "Checking", "1200.00"),
        ("2025-05-01", groceries.as_str(), "Checking", "400.00"),
        ("2025-06-01", "8999", "Checking", "50.00"),
    ] {
        cmd(
            &mut h,
            "txn.create",
            json!({"date": date, "splits": [
                {"account": debit, "value": amount},
                {"account": credit, "value": format!("-{amount}")}]}),
        );
    }
    h.state_mut().navigate(View::Reports);
    h.run();
    mouse(&mut h, "Tax summary");
    assert_eq!(value(&h, "Period"), "Last year");
    assert!(has(&h, "Tax lines for 2025"));
    assert!(has(
        &h,
        "Tax lines are hints to help you or your preparer, not tax advice. Line numbers can change from year to year."
    ));
    assert!(has(&h, "1040 L1a"));
    assert!(has(&h, "Sch A L5b"));
    assert!(h.get_by_label("1040 L1a").rect().top() < h.get_by_label("Sch A L5b").rect().top());
    assert!(has(&h, "Total 1040 L1a"));
    assert!(any(&h, "5,000.00"));
    assert!(any(&h, "1,200.00"));
    assert!(has(&h, "Not tagged"));
    assert!(has(&h, &groceries));
    assert!(has(&h, "Uncategorized Expense"));
    assert!(any(&h, "450.00"));
    assert!(has(&h, "Export PDF…"));
}
