//! app-desktop.md revision 4 and 5 acceptance tests (32–46): Scheduled, Budget,
//! Reports, Home additions, book-file errors, period tables and chart axes.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_arithmetic
)]

use crate::common::*;
use mk_ui::ctl::App;
use mk_ui::{AppEvent, UserConfig};
use mk_views::testing::Harness;
use mk_views::testing::Key;
use serde_json::{Value, json};

type H = Harness;

fn bill(name: &str, start: &str, amount: &str, extra: Value) -> Value {
    let mut v = json!({"name": name, "start": start, "remind_days": 7,
        "recurrence": {"frequency": "once"},
        "template": {"payee": name, "splits": [
            {"account": "Expenses:Groceries", "value": amount},
            {"account": "Assets:Checking", "value": format!("-{amount}")}]}});
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in e {
            o.insert(k.clone(), x.clone());
        }
    }
    v
}

fn opening(h: &mut H, amount: &str) {
    cmd(
        h,
        "txn.create",
        json!({"date": "2026-10-01", "payee": "Opening balance", "splits": [
        {"account": "Assets:Checking", "value": amount}, {"account": "Opening Balances", "value": format!("-{amount}")}]}),
    );
}

fn spend(h: &mut H, date: &str, category: &str, amount: &str) {
    cmd(
        h,
        "txn.create",
        json!({"date": date, "payee": "Shop", "splits": [
        {"account": category, "value": amount}, {"account": "Assets:Checking", "value": format!("-{amount}")}]}),
    );
}

fn rect(h: &H, label: &str) -> mk_views::testing::Rect {
    h.get_by_label(label).rect()
}

fn overlaps(a: mk_views::testing::Rect, b: mk_views::testing::Rect) -> bool {
    a.min.x < b.max.x && b.min.x < a.max.x
}

/// The top bar's Undo button.
fn undo(h: &mut H) {
    h.get_by_role_and_label(mk_views::testing::Role::Button, "Undo")
        .click();
    h.run();
}

fn present(h: &H, label: &str) -> bool {
    h.query_all_by_label(label).count() > 0
}

fn txn_count(h: &mut H) -> usize {
    cmd(h, "txn.list", json!({})).as_array().unwrap().len()
}

fn three_bills(h: &mut H) {
    cmd(
        h,
        "schedule.create",
        bill("Water", "2026-10-15", "30.00", json!({})),
    );
    cmd(
        h,
        "schedule.create",
        bill("Phone", "2026-10-25", "45.00", json!({})),
    );
    cmd(
        h,
        "schedule.create",
        bill("Gas", "2026-11-15", "60.00", json!({})),
    );
}

#[test]
fn app_desktop_at32_scheduled_groups_enter_and_undo() {
    let (mut h, _book) = harness_with_chart();
    h.state_mut().set_today("2026-10-20");
    opening(&mut h, "1000.00");
    three_bills(&mut h);
    click(&mut h, "Scheduled");
    for heading in ["Overdue", "Due in the next 7 days", "Later this month"] {
        assert!(
            has(&h, heading),
            "{heading}: {:?}",
            labels_containing(&h, "Due")
        );
    }
    assert!(has_contains(
        &h,
        "Scheduled Water due 2026-10-15, -30.00, overdue"
    ));
    assert!(has_contains(
        &h,
        "Scheduled Phone due 2026-10-25, -45.00, due_soon"
    ));
    assert!(has_contains(
        &h,
        "Scheduled Gas due 2026-11-15, -60.00, later"
    ));
    // Groups in order: Overdue above Due soon above Later.
    assert!(rect(&h, "Overdue").min.y < rect(&h, "Due in the next 7 days").min.y);
    assert!(rect(&h, "Due in the next 7 days").min.y < rect(&h, "Later this month").min.y);
    click(&mut h, "Enter Phone due 2026-10-25");
    assert!(!has_contains(&h, "Scheduled Phone due"), "row removed");
    assert_eq!(txn_count(&mut h), 2);
    open_register(&mut h, "Assets:Checking");
    assert!(
        has_contains(&h, "2026-10-25 Phone Expenses:Groceries, Spent 45.00"),
        "{:?}",
        labels_containing(&h, "Row")
    );
    click(&mut h, "Scheduled");
    undo(&mut h);
    assert!(
        has_contains(&h, "Scheduled Phone due 2026-10-25"),
        "back after undo"
    );
    assert_eq!(txn_count(&mut h), 1);
    // Skip says so and offers Undo.
    click(&mut h, "Skip Gas due 2026-11-15");
    assert!(has(&h, "Skipped —"));
    assert_eq!(
        h.query_all_by_label("Undo").count(),
        2,
        "the message offers Undo"
    );
    assert!(!has_contains(&h, "Scheduled Gas due"));
}

#[test]
fn app_desktop_at33_enter_estimate_with_amount_form() {
    let (mut h, _book) = harness_with_chart();
    h.state_mut().set_today("2026-10-20");
    cmd(
        &mut h,
        "schedule.create",
        bill(
            "Electric",
            "2026-10-18",
            "60.00",
            json!({"amount_kind": "estimate"}),
        ),
    );
    click(&mut h, "Scheduled");
    assert!(has_contains(
        &h,
        "Scheduled Electric due 2026-10-18, about -60.00"
    ));
    click(&mut h, "Enter Electric due 2026-10-18");
    assert!(has(&h, "Enter Electric"), "the amount form opened");
    assert_eq!(value(&h, "Actual amount"), "60.00");
    assert!(h.get_by_label("Actual amount").is_focused());
    // Pre-filled and selected: typing replaces it.
    h.get_by_label("Actual amount").type_text("63.18");
    h.run();
    assert_eq!(value(&h, "Actual amount"), "63.18");
    click(&mut h, "Enter transaction");
    assert!(!has(&h, "Enter transaction"), "form closed");
    let t = cmd(&mut h, "txn.list", json!({}));
    assert_eq!(t.as_array().unwrap().len(), 1);
    assert!(
        t[0]["splits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["value"] == "63.18")
    );
    assert_eq!(t[0]["date"], "2026-10-18");
}

#[test]
fn app_desktop_at34_schedule_form_summary_and_next_dates() {
    let (mut h, _book) = harness_with_chart();
    h.state_mut().set_today("2026-10-01");
    click(&mut h, "Scheduled");
    click(&mut h, "New schedule…");
    assert!(has(&h, "New schedule"));
    click(&mut h, "Repeats");
    click(&mut h, "Every 2 weeks");
    h.get_by_label("Start date").focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label("Start date").type_text("2026-10-02");
    h.run();
    h.run();
    assert!(
        has(&h, "Every 2 weeks on Friday"),
        "{:?}",
        labels_containing(&h, "Every")
    );
    for d in ["2026-10-02", "2026-10-16", "2026-10-30"] {
        assert!(has(&h, d), "next date {d}");
    }
    // Save creates the schedule through schedule.create.
    type_into(&mut h, "Schedule payee", "Babysitter");
    click(&mut h, "Schedule account");
    click(&mut h, "Assets:Checking");
    click(&mut h, "Schedule category");
    click(&mut h, "Expenses:Groceries");
    type_into(&mut h, "Schedule amount", "80.00");
    click(&mut h, "Save schedule");
    assert!(!has(&h, "Save schedule"), "form closed");
    let list = cmd(&mut h, "schedule.list", json!({}));
    assert_eq!(list[0]["summary"], "Every 2 weeks on Friday");
    assert_eq!(list[0]["amount"], "80.00");
}

#[test]
fn app_desktop_at35_open_book_runs_due_schedules_once() {
    let (mut app, book) = new_book_app();
    app.test_invoke(
        "account.create",
        json!({"name": "Checking", "type": "asset", "subtype": "bank"}),
    )
    .unwrap();
    app.test_invoke("account.create", json!({"name": "Rent", "type": "expense"}))
        .unwrap();
    app.test_invoke(
        "schedule.create",
        json!({"name": "Rent", "start": "2026-10-15", "auto_enter": true,
               "recurrence": {"frequency": "monthly"},
               "template": {"payee": "Landlord", "splits": [
                   {"account": "Rent", "value": "900.00"},
                   {"account": "Checking", "value": "-900.00"}]}}),
    )
    .unwrap();
    app.set_today("2026-10-20");
    app.close_book();
    let path = book.path.to_str().unwrap().to_owned();
    app.open_book(&path);
    let mut h = harness(app);
    h.run();
    assert!(
        has(&h, "Entered 1 scheduled transaction"),
        "{:?}",
        labels_containing(&h, "Entered")
    );
    assert!(has(&h, "Show"));
    assert_eq!(txn_count(&mut h), 1);
    let last = cmd(&mut h, "history.list", json!({"limit": 1}));
    assert_eq!(last[0]["actor"], "schedule");
    // Show opens History.
    click(&mut h, "Show");
    assert_eq!(h.state().view(), &mk_ui::View::History);
    // Reopening the same day enters nothing.
    h.state_mut().close_book();
    h.state_mut().open_book(&path);
    h.run();
    assert!(!has_contains(&h, "Entered"));
    assert_eq!(txn_count(&mut h), 1);
}

#[test]
fn app_desktop_at36_register_shows_scheduled_with_projected_balance() {
    // Scheduled rows follow the last real row when oldest first (revision 27).
    let (mut h, _book) = harness_with_chart_config(ascending(iso_config()));
    h.state_mut().set_today("2026-10-20");
    opening(&mut h, "1000.00");
    cmd(
        &mut h,
        "schedule.create",
        json!({"name": "Pay", "start": "2026-10-30", "recurrence": {"frequency": "monthly"},
               "template": {"payee": "Employer", "splits": [
                   {"account": "Assets:Checking", "value": "2000.00"},
                   {"account": "Salary", "value": "-2000.00"}]}}),
    );
    let f = cmd(
        &mut h,
        "schedule.forecast",
        json!({"account": "Assets:Checking", "to": "2026-11-19", "today": "2026-10-20"}),
    );
    let projected = f["rows"][0]["balance"].as_str().unwrap().to_owned();
    assert_eq!(projected, "3000.00");
    open_register(&mut h, "Assets:Checking");
    let label = format!(
        "Scheduled row: due 2026-10-30 Employer, Received 2000.00, projected balance {projected}"
    );
    assert!(
        has(&h, &label),
        "{:?}",
        labels_containing(&h, "Scheduled row")
    );
    // After the last real row.
    let real = h.get_by_label_contains("Row 1:").rect();
    assert!(rect(&h, &label).min.y > real.min.y);
    // Selecting it offers Enter and Skip.
    click(&mut h, &label);
    assert!(has(&h, "Enter scheduled Employer") && has(&h, "Skip scheduled Employer"));
    click(&mut h, "Enter scheduled Employer");
    assert!(
        has(&h, "Closing balance 3,000.00"),
        "{:?}",
        labels_containing(&h, "Closing")
    );
    // The toggle hides them.
    click(&mut h, "Show scheduled");
    assert!(labels_containing(&h, "Scheduled row").is_empty());
}

#[test]
fn app_desktop_at37_budget_item_follows_module_and_start_empty() {
    let (mut h, _book) = harness_with_chart();
    h.state_mut().set_today("2026-10-20");
    assert!(
        !feature_items(&mut h).contains(&"Budget".to_owned()),
        "no Budget item while the module is off"
    );
    click(&mut h, "Settings");
    settings_section(&mut h, "Features");
    click(&mut h, "Budgets");
    assert_eq!(feature_items(&mut h), ["Budget"], "Budget item appears");
    open_feature(&mut h, "Budget");
    assert!(has(&h, "Start empty") && has(&h, "Start from my spending"));
    click(&mut h, "Start empty");
    let list = cmd(&mut h, "budget.list", json!({}));
    assert_eq!(list[0]["name"], "Budget");
    assert_eq!(list[0]["start_month"], "2026-10");
    assert!(has(&h, "October 2026"));
}

fn budget_book() -> H {
    let (mut h, book) = harness_with_chart();
    std::mem::forget(book);
    h.state_mut().set_today("2026-10-20");
    cmd(&mut h, "module.enable", json!({"module": "budgets"}));
    cmd(
        &mut h,
        "budget.create",
        json!({"name": "Household", "start_month": "2026-01"}),
    );
    cmd(
        &mut h,
        "budget.set_line",
        json!({"account": "Expenses:Groceries", "amount": "400.00"}),
    );
    spend(&mut h, "2026-10-05", "Expenses:Groceries", "350.00");
    h.run();
    h
}

#[test]
fn app_desktop_at38_edit_budget_cell() {
    let mut h = budget_book();
    open_feature(&mut h, "Budget");
    assert!(has(
        &h,
        "Expenses:Groceries: budget 400.00, spent 350.00, remaining 50.00, 87.5%"
    ));
    let field = "Budget for Expenses:Groceries";
    h.get_by_label(field).focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label(field).type_text("450");
    h.run();
    key(&mut h, Key::Enter);
    h.run();
    let r = cmd(&mut h, "budget.report", json!({"month": "2026-10"}));
    assert_eq!(r["expenses"]["rows"][0]["budget"], "450.00");
    assert!(
        has(
            &h,
            "Expenses:Groceries: budget 450.00, spent 350.00, remaining 100.00, 77.8%"
        ),
        "{:?}",
        labels_containing(&h, "Expenses:Groceries:")
    );
    assert!(has(&h, "Progress Expenses:Groceries 77.8%"));
    assert_eq!(value(&h, field), "450.00");
}

#[test]
fn app_desktop_at39_fill_from_history_is_one_change() {
    let mut h = budget_book();
    spend(&mut h, "2026-09-05", "Expenses:Groceries", "120.00");
    open_feature(&mut h, "Budget");
    let before = cmd(&mut h, "history.list", json!({"limit": 500}))
        .as_array()
        .unwrap()
        .len();
    click(&mut h, "Fill from history…");
    assert!(has(&h, "Fill from history"));
    assert!(has(&h, "Amount for Expenses:Groceries"));
    click(&mut h, "Apply");
    let after = cmd(&mut h, "history.list", json!({"limit": 500}));
    assert_eq!(after.as_array().unwrap().len(), before + 1);
    assert_eq!(after[0]["command"], "budget.apply_suggestion");
}

/// The reports.md test book.
fn reports_book() -> H {
    let (app, book) = new_book_app();
    std::mem::forget(book);
    let mut h = harness(app);
    h.state_mut().set_today("2026-11-05");
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Checking", "type": "asset", "subtype": "bank"}),
    );
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Visa", "type": "liability", "subtype": "credit-card"}),
    );
    for n in ["Groceries", "Dining", "Household", "Fuel"] {
        cmd(
            &mut h,
            "account.create",
            json!({"name": n, "type": "expense"}),
        );
    }
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Salary", "type": "income"}),
    );
    let t = |h: &mut H, date: &str, debit: &str, credit: &str, amount: &str| {
        cmd(
            h,
            "txn.create",
            json!({"date": date, "splits": [
                {"account": debit, "value": amount},
                {"account": credit, "value": format!("-{amount}")}]}),
        );
    };
    t(
        &mut h,
        "2026-09-30",
        "Checking",
        "Opening Balances",
        "1000.00",
    );
    t(&mut h, "2026-09-15", "Fuel", "Checking", "41.07");
    t(&mut h, "2026-10-01", "Checking", "Salary", "2500.00");
    t(&mut h, "2026-10-03", "Groceries", "Checking", "63.41");
    t(&mut h, "2026-10-05", "Dining", "Visa", "23.65");
    t(&mut h, "2026-10-20", "Visa", "Checking", "23.65");
    t(&mut h, "2026-10-22", "Visa", "Groceries", "10.00");
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-25", "splits": [
            {"account": "Groceries", "value": "80.00"},
            {"account": "Household", "value": "20.00"},
            {"account": "Visa", "value": "-100.00"}]}),
    );
    h.run();
    h
}

fn custom_period(h: &mut H, from: &str, to: &str) {
    click(h, "Period");
    click(h, "Custom…");
    type_into(h, "From date", from);
    type_into(h, "To date", to);
    h.run();
}

#[test]
fn app_desktop_at40_income_expense_report_and_export() {
    let mut h = reports_book();
    click(&mut h, "Reports");
    click(&mut h, "Income & expenses");
    custom_period(&mut h, "2026-09-01", "2026-10-31");
    for text in [
        "2026", "Sep", "Oct", "–", "Salary", "2,500.00", "41.07", "177.06", "-41.07", "2,322.94",
        "2,281.87",
    ] {
        assert!(
            present(&h, text),
            "{text}: {:?}",
            labels_containing(&h, ".")
        );
    }
    assert!(has_contains(
        &h,
        "Income and expenses chart by period: 2026-09 income 0.00"
    ));
    events(&mut h);
    click(&mut h, "Export CSV…");
    let evs = events(&mut h);
    let expected = cmd(
        &mut h,
        "report.export",
        json!({"report": "income_expense", "format": "csv", "args": {
            "from": "2026-09-01", "to": "2026-10-31", "interval": "month", "depth": 99, "today": "2026-11-05"}}),
    );
    assert_eq!(
        evs,
        [AppEvent::SaveFile {
            file_name: expected["file_name"].as_str().unwrap().to_owned(),
            content: expected["content"].as_str().unwrap().to_owned(),
        }]
    );
    assert!(
        expected["content"]
            .as_str()
            .unwrap()
            .contains("Salary,0.00,2500.00,2500.00")
    );
    // Clicking a category row opens its transactions.
    click(&mut h, "Show transactions for Groceries");
    assert!(has(&h, "Transactions: Groceries"));
    assert!(present(&h, "2026-10-03"));
}

// ----- AT41: layout and text-size rules on the new views ---------------------------

fn smallest_text(h: &H) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    for c in &h.output().shapes {
        if c.kind == "text" && !c.text.trim().is_empty() {
            out.push((c.text.clone(), c.size));
        }
    }
    out
}

fn assert_min_text(h: &H, view: &str) {
    let all = smallest_text(h);
    assert!(!all.is_empty(), "{view}: no text drawn");
    let small: Vec<_> = all
        .iter()
        .filter(|(_, s)| *s < mk_ui::theme::MIN_TEXT)
        .collect();
    assert!(small.is_empty(), "{view}: text under 12 pt: {small:?}");
}

fn inside(h: &H, label: &str, size: mk_views::testing::Vec2) {
    let r = h
        .query_all_by_label(label)
        .map(|n| n.rect())
        .max_by(|a, b| a.max.x.total_cmp(&b.max.x))
        .unwrap_or_else(|| panic!("no {label}"));
    let window = mk_views::testing::Rect::from_min_size(mk_views::testing::Pos2::ZERO, size);
    assert!(
        window.contains_rect(r),
        "{label} at {r:?} is outside {size:?}"
    );
}

#[test]
fn app_desktop_at41_new_views_follow_layout_rules() {
    for size in [
        mk_views::testing::vec2(1000.0, 700.0),
        mk_views::testing::vec2(1200.0, 800.0),
    ] {
        // Scheduled.
        let (mut h, _book) = harness_with_chart();
        h.set_size(size);
        h.state_mut().set_today("2026-10-20");
        three_bills(&mut h);
        click(&mut h, "Scheduled");
        inside(&h, "Actions", size);
        inside(&h, "Edit Gas due 2026-11-15", size);
        assert!(overlaps(rect(&h, "Due"), rect(&h, "2026-10-15")));
        let amount_header = rect(&h, "Amount");
        assert!(overlaps(amount_header, rect(&h, "-30.00")));
        assert!(
            (amount_header.max.x - rect(&h, "-30.00").max.x).abs() < 1.5,
            "amounts right-aligned"
        );
        assert_min_text(&h, "scheduled");
        click(&mut h, "All schedules");
        inside(&h, "Delete Gas", size);
        assert_min_text(&h, "all schedules");
        click(&mut h, "New schedule…");
        assert_min_text(&h, "schedule form");

        // Budget.
        let mut h = budget_book();
        h.set_size(size);
        open_feature(&mut h, "Budget");
        inside(&h, "Progress", size);
        let headers: Vec<mk_views::testing::Rect> =
            h.query_all_by_label("Budget").map(|n| n.rect()).collect();
        let cell = rect(&h, "Budget for Expenses:Groceries");
        assert!(
            headers.iter().any(|hr| overlaps(*hr, cell)),
            "Budget header over its cells"
        );
        assert_min_text(&h, "budget");
        click(&mut h, "Year");
        assert_min_text(&h, "budget year");

        // Reports.
        let mut h = reports_book();
        h.set_size(size);
        click(&mut h, "Reports");
        custom_period(&mut h, "2026-10-01", "2026-10-31");
        inside(&h, "Percent", size);
        assert!(overlaps(rect(&h, "Category"), rect(&h, "Groceries")));
        assert_min_text(&h, "spending report");
        for report in ["Income & expenses", "Net worth", "Cash flow", "Payees"] {
            click(&mut h, report);
            assert_min_text(&h, report);
        }
        inside(&h, "Received", size);
    }
}

#[test]
fn app_desktop_at42_unwritable_folder_message() {
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    }
    // A superuser can write anywhere; then use a folder that cannot exist (a file
    // stands in its place) to get the same kind of failure.
    let probe = locked.join("probe");
    let target_dir = if std::fs::write(&probe, b"x").is_ok() {
        let _ = std::fs::remove_file(&probe);
        let blocker = dir.path().join("not-a-folder");
        std::fs::write(&blocker, b"x").unwrap();
        blocker
    } else {
        locked.clone()
    };
    let path = target_dir.join("Home.moneykeep");
    let app = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        UserConfig::default(),
    );
    let mut h = harness(app);
    h.state_mut()
        .create_book(path.to_str().unwrap(), "Home", "USD");
    h.run();
    let shown = labels_containing(&h, "Couldn't create the book");
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(shown[0].contains(path.to_str().unwrap()), "{shown:?}");
    assert!(shown[0].contains("Choose another folder"), "{shown:?}");
    for raw in ["database error", "unable to open", "sqlite", "SQLITE"] {
        assert!(
            labels_containing(&h, raw).is_empty(),
            "raw text {raw} shown"
        );
    }
    assert!(has(&h, "Details"), "underlying error under a disclosure");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
    }
}

// ----- Revision 5 ------------------------------------------------------------------

fn preset(h: &mut H, name: &str) {
    click(h, "Period");
    click(h, name);
    h.run();
}

#[test]
fn app_desktop_at43_period_columns_scroll_with_fixed_category_and_total() {
    let mut h = reports_book();
    let size = mk_views::testing::vec2(1200.0, 800.0);
    h.set_size(size);
    // The account list pinned beside the main area (as when this test was written),
    // so the periods do not fit.
    open_pane(&mut h, "All accounts");
    click(&mut h, "Keep open");
    click(&mut h, "Reports");
    click(&mut h, "Income & expenses");
    preset(&mut h, "This year");
    // Short month headers, the year once above January.
    for m in ["Jan", "Sep", "Oct", "Dec", "2026"] {
        assert!(present(&h, m), "{m}");
    }
    // No amount is cut off: no drawn text with digits ends in "…".
    let cut: Vec<String> = smallest_text(&h)
        .into_iter()
        .map(|(t, _)| t)
        .filter(|t| t.contains('…') && t.chars().any(|c| c.is_ascii_digit()))
        .collect();
    assert!(cut.is_empty(), "truncated amounts: {cut:?}");
    // Zeros are a dimmed dash; the exact numbers are there.
    assert!(present(&h, "–"));
    assert!(present(&h, "2,500.00") && present(&h, "2,281.87"));
    let total = rect(&h, "Total");
    let window = mk_views::testing::Rect::from_min_size(mk_views::testing::Pos2::ZERO, size);
    assert!(window.contains_rect(total), "Total at {total:?}");
    let category = rect(&h, "Category");
    // The table's December header (the chart above has a "Dec" axis label too).
    let header_dec = |h: &H| {
        h.query_all_by(|n| n.label().as_deref() == Some("Dec"))
            .map(|n| n.rect())
            .find(|r| r.min.y >= category.min.y - 40.0)
    };
    // December starts out of view: the Slint views do not report what is
    // scrolled out of a clipped area, so it is not found at all yet.
    assert!(
        header_dec(&h).is_none_or(|r| r.max.x > total.min.x),
        "December starts out of view"
    );
    // Scroll the periods sideways to their end with their own scroll bar.
    let bar = h.get_by_label("Periods scroll sideways");
    bar.set_text("100000");
    h.run();
    h.run();
    assert_eq!(rect(&h, "Total"), total, "Total stays in place");
    assert_eq!(rect(&h, "Category"), category, "Category stays in place");
    let dec = header_dec(&h).expect("December in view");
    assert!(
        dec.max.x <= total.min.x + 1.0 && dec.min.x > category.max.x,
        "December scrolled into view: {dec:?}"
    );
}

/// Parse "1,000" style tick labels.
fn tick_values(label: &str) -> Vec<f64> {
    label
        .trim_start_matches("Axis ticks: ")
        .split("; ")
        .map(|t| t.replace(',', "").parse::<f64>().unwrap())
        .collect()
}

fn is_round_step(step: f64) -> bool {
    let pow = 10f64.powf(step.log10().floor());
    let unit = (step / pow * 1000.0).round() / 1000.0;
    [1.0, 2.0, 5.0, 10.0].contains(&unit)
}

#[test]
fn app_desktop_at44_round_chart_ticks_and_text_size() {
    let mut h = reports_book();
    click(&mut h, "Reports");
    for report in ["Income & expenses", "Net worth"] {
        click(&mut h, report);
        preset(&mut h, "This year");
        let labels = labels_containing(&h, "Axis ticks: ");
        assert_eq!(labels.len(), 1, "{report}: {labels:?}");
        let ticks = tick_values(&labels[0]);
        assert!(ticks.len() >= 2, "{report}: {ticks:?}");
        assert!(ticks.contains(&0.0), "{report}: zero is a tick: {ticks:?}");
        let step = ticks[1] - ticks[0];
        assert!(is_round_step(step), "{report}: step {step} in {ticks:?}");
        for w in ticks.windows(2) {
            assert!(
                (w[1] - w[0] - step).abs() < 1e-6,
                "{report}: evenly spaced {ticks:?}"
            );
        }
        assert_min_text(&h, report);
    }
}

#[test]
fn app_desktop_at45_net_worth_ends_today() {
    let mut h = reports_book();
    let r = cmd(
        &mut h,
        "report.net_worth",
        json!({"from": "2026-01-01", "to": "2026-12-31", "today": "2026-10-19", "interval": "month"}),
    );
    let dates: Vec<&str> = r["dates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert_eq!(&dates[dates.len() - 2..], ["2026-09-30", "2026-10-19"]);
    h.state_mut().set_today("2026-10-19");
    click(&mut h, "Reports");
    click(&mut h, "Net worth");
    preset(&mut h, "This year");
    assert!(
        present(&h, "10-19"),
        "today is the last column (day and month in the date format)"
    );
    assert!(
        !present(&h, "Nov") && !present(&h, "Dec"),
        "no future month ends"
    );
}

#[test]
fn app_desktop_at46_coming_up_reaches_beyond_30_days() {
    let (mut h, _book) = harness_with_chart();
    h.state_mut().set_today("2026-10-20");
    cmd(
        &mut h,
        "schedule.create",
        bill("Soon", "2026-10-25", "10.00", json!({})),
    );
    cmd(
        &mut h,
        "schedule.create",
        bill("Later", "2026-11-29", "20.00", json!({})),
    );
    cmd(
        &mut h,
        "schedule.create",
        bill("Much later", "2026-12-29", "30.00", json!({})),
    );
    h.state_mut().navigate(mk_ui::View::Home);
    h.run();
    assert!(has(&h, "Coming up"));
    // Home's Coming up block (a dashboard since revision 31) names each row by its
    // payee, due date and amount.
    for (date, payee) in [
        ("2026-10-25", "Soon"),
        ("2026-11-29", "Later"),
        ("2026-12-29", "Much later"),
    ] {
        assert!(
            has_contains(&h, &format!("Scheduled {payee} due {date}")),
            "{payee}: {:?}",
            labels_containing(&h, "Scheduled")
        );
    }
}
