//! app-desktop.md acceptance tests, driven through the accessibility tree.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::ctl::App;
use mk_ui::{AppEvent, Screen, UserConfig};
use mk_views::testing::Key;
use serde_json::json;

#[test]
fn app_desktop_at1_create_book_from_start_screen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("household.moneykeep");
    let app = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        UserConfig::default(),
    );
    let mut h = harness(app);
    assert_eq!(h.state().screen(), Screen::Start);
    // Revision 26: the form is in the Create a new book dialog.
    click(&mut h, "Create a New Book");
    assert!(has(&h, "Create a new book"));
    h.get_by_label("Book name").focus();
    h.run();
    // Replace the default name.
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label("Book name").type_text("Household");
    h.run();
    click(&mut h, "Create book…");
    let evs = events(&mut h);
    assert_eq!(
        evs,
        [AppEvent::ChooseNewBookPath {
            name: "Household".into(),
            currency: "USD".into()
        }]
    );
    // The app shell shows the save dialog, then calls create_book with the path.
    h.state_mut()
        .create_book(path.to_str().unwrap(), "Household", "USD");
    h.run();
    assert_eq!(h.state().screen(), Screen::Main);
    assert!(has(&h, "Household"), "book name in the top bar");
    // Revision 34: the dialog creates the book from its chart (Personal & Household
    // by default), which holds Opening Balances among its accounts.
    let ob = h
        .state_mut()
        .test_invoke(
            "account.get",
            serde_json::json!({"account": "Opening Balances"}),
        )
        .unwrap();
    assert_eq!(ob["code"], "3900");
    let info = h
        .state_mut()
        .test_invoke("book.info", serde_json::json!({}))
        .unwrap();
    assert_eq!(info["chart"]["preset"], "personal");
    assert!(path.exists());
    let recent: Vec<String> = h
        .state()
        .config
        .recent
        .iter()
        .map(|r| r.path.clone())
        .collect();
    assert_eq!(recent, [path.to_str().unwrap()]);
}

#[test]
fn app_desktop_at2_new_account_with_opening_balance() {
    let (app, _book) = new_book_app();
    let mut h = harness(app);
    click(&mut h, "Accounts");
    click(&mut h, "New account…");
    type_into(&mut h, "Account name", "Checking");
    // Kind: bank account is the first (default) kind.
    type_into(&mut h, "Opening balance (optional)", "1,000.00");
    type_into(&mut h, "Opening balance date", "2026-10-01");
    click(&mut h, "Create account");
    assert!(!has(&h, "Create account"), "dialog closed");
    open_pane(&mut h, "All accounts");
    assert!(has(&h, "Account Checking"));
    assert!(!labels_containing(&h, "1,000.00").is_empty());
    let txns = cmd(&mut h, "txn.list", json!({"account": "Checking"}));
    assert_eq!(txns.as_array().unwrap().len(), 1);
    assert!(
        txns[0]["splits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["account_name"] == "Opening Balances")
    );
}

/// Enter a transaction from the keyboard only (revision 24: in the New transaction
/// pop-up): Ctrl+N, then the Date's text is selected, Tab between fields, Enter
/// saves.
fn enter_keyboard(
    h: &mut mk_views::testing::Harness,
    date: &str,
    payee: &str,
    cat_prefix: &str,
    spent: &str,
) {
    h.state_mut().blur_for_tests();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::N);
    h.run();
    h.run();
    assert!(
        h.get_by_label("New date").is_focused(),
        "focus starts in Date"
    );
    h.get_by_label("New date").type_text(date);
    h.run();
    key(h, Key::Tab); // No.
    key(h, Key::Tab); // Payee
    h.get_by_label("New payee").type_text(payee);
    h.run();
    key(h, Key::Tab); // Category
    h.get_by_label("New category").type_text(cat_prefix);
    h.run();
    h.run();
    key(h, Key::Tab); // accepts the first suggestion, moves to Memo
    key(h, Key::Tab); // Spent
    h.get_by_label("New spent").type_text(spent);
    h.run();
    key(h, Key::Enter);
}

fn opening(h: &mut mk_views::testing::Harness, amount: &str) {
    cmd(
        h,
        "txn.create",
        json!({"date": "2026-10-01", "payee": "Opening balance", "splits": [
        {"account": "Assets:Checking", "value": amount}, {"account": "Opening Balances", "value": format!("-{amount}")}]}),
    );
    h.run();
}

#[test]
fn app_desktop_at3_register_entry_from_keyboard() {
    let (mut h, _book) = harness_with_chart();
    opening(&mut h, "1000.00");
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    assert!(
        has(&h, "New spent") && has(&h, "New received"),
        "bank column titles"
    );
    key(&mut h, Key::Escape);
    assert!(has(&h, "Spent") && has(&h, "Received"));
    enter_keyboard(&mut h, "2026-10-05", "Corner Grocery", "groc", "50.00");
    assert!(
        has_contains(&h, "Corner Grocery Expenses:Groceries, Spent 50.00"),
        "{:?}",
        labels_containing(&h, "Row 2")
    );
    assert!(has_contains(&h, "status uncleared, balance 950.00"));
    assert!(has(&h, "Closing balance 950.00"));
    // The pop-up closed (fast entry off); the next one is blank, dated like the
    // last one saved, with focus in Date.
    assert!(!has(&h, "New transaction"));
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::N);
    h.run();
    h.run();
    assert_eq!(value(&h, "New payee"), "");
    assert_eq!(value(&h, "New date"), "2026-10-05");
    assert!(h.get_by_label("New date").is_focused());
}

#[test]
fn app_desktop_at4_payee_autocomplete_prefills() {
    let (mut h, _book) = harness_with_chart();
    opening(&mut h, "1000.00");
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-02", "payee": "KROGER #512", "splits": [
        {"account": "Expenses:Groceries", "value": "54.20"}, {"account": "Assets:Checking", "value": "-54.20"}]}),
    );
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    type_into(&mut h, "New payee", "kro");
    h.run();
    assert!(has(&h, "Payee suggestion KROGER #512"));
    click(&mut h, "Payee suggestion KROGER #512");
    assert_eq!(value(&h, "New category"), "Expenses:Groceries");
    assert_eq!(value(&h, "New spent"), "54.20");
    // The amount is selected when reached, so typing replaces it.
    h.get_by_label("New spent").focus();
    h.run();
    h.get_by_label("New spent").type_text("60.00");
    h.run();
    assert_eq!(value(&h, "New spent"), "60.00");
}

#[test]
fn app_desktop_at5_invalid_entry_shows_error_and_keeps_values() {
    let (mut h, _book) = harness_with_chart();
    open_register(&mut h, "Assets:Checking");
    add_txn(
        &mut h,
        &[
            ("payee", "Mystery"),
            ("category", "Nonexistent Category"),
            ("spent", "12.00"),
        ],
    );
    assert!(
        has_contains(&h, "no account with id, full name or code"),
        "command's error in the pop-up"
    );
    assert!(has(&h, "New transaction"), "the pop-up stays open");
    assert_eq!(value(&h, "New payee"), "Mystery");
    assert_eq!(value(&h, "New category"), "Nonexistent Category");
    assert_eq!(value(&h, "New spent"), "12.00");
    assert_eq!(
        cmd(&mut h, "txn.list", json!({})).as_array().unwrap().len(),
        0
    );
}

/// Select a row without editing it: a click opens the pop-up (revision 24) and
/// Esc closes it, leaving the row selected.
fn select_row(h: &mut mk_views::testing::Harness, part: &str) {
    h.get_by_label_contains(part).click();
    h.run();
    key(h, Key::Escape);
    h.run();
}

#[test]
fn app_desktop_at6_space_toggles_status() {
    let (mut h, _book) = harness_with_chart();
    opening(&mut h, "1000.00");
    open_register(&mut h, "Assets:Checking");
    select_row(&mut h, "Row 1:");
    key(&mut h, Key::Space);
    assert!(has_contains(&h, "status cleared"));
    key(&mut h, Key::Space);
    assert!(has_contains(&h, "status uncleared"));
    // A reconciled row is not toggled; a message explains why.
    let t = cmd(&mut h, "txn.list", json!({}))[0]["id"].clone();
    cmd(
        &mut h,
        "txn.set_reconcile",
        json!({"splits": [{"id": t, "split": 0, "state": "r"}]}),
    );
    h.run();
    select_row(&mut h, "Row 1:");
    key(&mut h, Key::Space);
    assert!(has_contains(&h, "status reconciled"));
    assert!(has_contains(
        &h,
        "Reconciled transactions keep their status"
    ));
}

#[test]
fn app_desktop_at7_split_editor() {
    let (mut h, _book) = harness_with_chart();
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Household", "type": "expense"}),
    );
    opening(&mut h, "1000.00");
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    fill_new(&mut h, &[("payee", "MegaMart"), ("spent", "100.00")]);
    click(&mut h, "Split…");
    assert!(has(&h, "Split transaction"));
    // Line 1 starts with the whole amount; change it and add a second line.
    h.get_by_label("Line 1 amount").focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label("Line 1 amount").type_text("60.00");
    h.run();
    type_into(&mut h, "Line 1 category", "Expenses:Groceries");
    assert!(has(&h, "Left to assign 40.00"));
    assert!(disabled(&h, "Save split"));
    type_into(&mut h, "Line 2 category", "Household");
    type_into(&mut h, "Line 2 amount", "30.00");
    assert!(has(&h, "Left to assign 10.00"));
    assert!(disabled(&h, "Save split"));
    h.get_by_label("Line 2 amount").focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label("Line 2 amount").type_text("40.00");
    h.run();
    assert!(has(&h, "Left to assign 0.00"));
    assert!(!disabled(&h, "Save split"));
    click(&mut h, "Save split");
    assert!(!has(&h, "Split transaction"));
    assert!(!has(&h, "New transaction"), "saved and closed");
    assert!(has_contains(&h, "MegaMart Split (2)"));
    let t = cmd(&mut h, "txn.list", json!({"account": "Household"}));
    assert_eq!(t[0]["splits"].as_array().unwrap().len(), 3);
}

#[test]
fn app_desktop_at8_ctrl_z_undo_and_redo() {
    let (mut h, _book) = harness_with_chart();
    opening(&mut h, "1000.00");
    open_register(&mut h, "Assets:Checking");
    enter_keyboard(&mut h, "2026-10-05", "Corner Grocery", "groc", "50.00");
    assert!(has(&h, "Closing balance 950.00"));
    h.state_mut().blur_for_tests();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::Z);
    h.run();
    assert!(
        has(&h, "Closing balance 1,000.00"),
        "{:?}",
        labels_containing(&h, "Closing")
    );
    h.key_press_modifiers(
        mk_views::testing::Modifiers::COMMAND | mk_views::testing::Modifiers::SHIFT,
        Key::Z,
    );
    h.run();
    assert!(has(&h, "Closing balance 950.00"));
}

#[test]
fn app_desktop_at9_void_row_struck_through_balance_unchanged() {
    // Row 2 is the later transaction: oldest first (revision 27).
    let (mut h, _book) = harness_with_chart_config(ascending(iso_config()));
    opening(&mut h, "1000.00");
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-05", "payee": "Corner Grocery", "splits": [
        {"account": "Expenses:Groceries", "value": "50.00"}, {"account": "Assets:Checking", "value": "-50.00"}]}),
    );
    open_register(&mut h, "Assets:Checking");
    assert!(has_contains(
        &h,
        "Corner Grocery Expenses:Groceries, Spent 50.00, status uncleared, balance 950.00"
    ));
    h.get_by_label_contains("Row 2:").click_secondary();
    h.run();
    click(&mut h, "Void");
    assert!(has_contains(
        &h,
        "Corner Grocery Expenses:Groceries, Spent 50.00, status void, balance 1000.00"
    ));
    assert!(has(&h, "Closing balance 1,000.00"));
}

#[test]
fn app_desktop_at10_import_review_and_reimport() {
    let (mut h, _book) = harness_with_chart();
    let bytes = fixture("ofx/checking-v1.ofx");
    h.state_mut().import_file("checking-v1.ofx", &bytes);
    h.run();
    assert!(
        has(&h, "Example Savings Bank CHECKING ending 6789"),
        "the bank's account description"
    );
    assert!(has(&h, "Account for section 1"), "account picker");
    for (i, payee) in ["Employer Payroll", "City Water", "Corner Grocery"]
        .iter()
        .enumerate()
    {
        assert!(has(
            &h,
            &format!("Row {} {payee}: Choose an account first", i + 1)
        ));
    }
    assert!(disabled(&h, "Add to books"), "disabled while unresolved");
    click(&mut h, "Account for section 1");
    click(&mut h, "Assets:Checking");
    assert!(has(&h, "Row 1 Employer Payroll: New"));
    assert!(has(&h, "New: 3"));
    assert!(!disabled(&h, "Add to books"));
    click(&mut h, "Add to books");
    assert!(has_contains(&h, "Added to books: 3 new"));
    assert_eq!(
        cmd(&mut h, "txn.list", json!({"account": "Assets:Checking"}))
            .as_array()
            .unwrap()
            .len(),
        3
    );
    // Re-import: the mapping was remembered and every row is already imported.
    h.state_mut().import_file("checking-v1.ofx", &bytes);
    h.run();
    for (i, payee) in ["Employer Payroll", "City Water", "Corner Grocery"]
        .iter()
        .enumerate()
    {
        assert!(has(&h, &format!("Row {} {payee}: Already imported", i + 1)));
    }
    assert!(has(&h, "Already imported: 3"));
}

#[test]
fn app_desktop_at11_always_use_this_creates_rule() {
    let (mut h, _book) = harness_with_chart();
    let qif = b"!Type:Bank\nD10/01/2026\nT-12.00\nPCorner Bakery\n^\n";
    cmd(&mut h, "import.list", json!({}));
    h.state_mut().import_file("bakery.qif", qif);
    h.run();
    click(&mut h, "Account for section 1");
    click(&mut h, "Assets:Checking");
    // Tick "Always" first, then pick: the choice applies at once (no Use button).
    click(&mut h, "Always use this for Corner Bakery");
    click(&mut h, "Category for row 1");
    type_into(&mut h, "Search categories for row 1", "groc");
    click(&mut h, "Expenses:Groceries");
    let rules = cmd(&mut h, "rule.list", json!({}));
    assert_eq!(rules.as_array().unwrap().len(), 1, "{rules}");
    assert_eq!(rules[0]["conditions"][0]["value"], "CORNER BAKERY");
    // The row now proposes the chosen category.
    assert!(!has(&h, "Search categories for row 1"), "picker closed");
    assert_eq!(value(&h, "Category for row 1"), "Expenses:Groceries");
}

#[test]
fn app_desktop_at12_reconcile_flow() {
    let (mut h, _book) = harness_with_chart();
    opening(&mut h, "1000.00");
    for (d, amt) in [("2026-10-02", "50.00"), ("2026-10-05", "25.00")] {
        cmd(
            &mut h,
            "txn.create",
            json!({"date": d, "payee": "Shop", "splits": [
            {"account": "Expenses:Groceries", "value": amt}, {"account": "Assets:Checking", "value": format!("-{amt}")}]}),
        );
    }
    open_register(&mut h, "Assets:Checking");
    click(&mut h, "Reconcile…");
    // The date defaults to today; replace it.
    h.get_by_label("Statement date").focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label("Statement date").type_text("2026-10-31");
    h.run();
    type_into(&mut h, "Statement balance", "925.00");
    click(&mut h, "Start reconciling");
    assert!(
        has(&h, "Difference 925.00"),
        "{:?}",
        labels_containing(&h, "Difference")
    );
    assert!(disabled(&h, "Finish"));
    click(&mut h, "2026-10-01 Opening balance 1,000.00");
    assert!(has(&h, "Difference -75.00"), "difference updates live");
    assert!(disabled(&h, "Finish"));
    click(&mut h, "2026-10-02 Shop -50.00");
    click(&mut h, "2026-10-05 Shop -25.00");
    assert!(has(&h, "Balanced"));
    assert!(!disabled(&h, "Finish"));
    click(&mut h, "Finish");
    assert!(has_contains(&h, "Reconciliation finished"));
    // Filled circles in the register.
    open_register(&mut h, "Assets:Checking");
    assert_eq!(labels_containing(&h, "status reconciled").len(), 3);
    assert_eq!(h.query_all_by_label("reconciled").count(), 3);
}

#[test]
fn app_desktop_at13_settings_features() {
    let (app, _book) = new_book_app();
    let mut h = harness(app);
    click(&mut h, "Settings");
    settings_section(&mut h, "Features");
    click(&mut h, "Estimates");
    assert_eq!(
        cmd(
            &mut h,
            "setting.get",
            json!({"module": "estimates", "key": "enabled"})
        )["value"],
        true
    );
    click(&mut h, "Estimates");
    assert_eq!(
        cmd(
            &mut h,
            "setting.get",
            json!({"module": "estimates", "key": "enabled"})
        )["value"],
        false
    );
    click(&mut h, "Progress invoicing");
    assert_eq!(
        cmd(
            &mut h,
            "setting.get",
            json!({"module": "estimates", "key": "enabled"})
        )["value"],
        true,
        "requirement enabled too"
    );
    click(&mut h, "Estimates");
    assert!(has_contains(
        &h,
        "estimates is required by enabled module(s): progress-invoicing"
    ));
    assert_eq!(
        cmd(
            &mut h,
            "setting.get",
            json!({"module": "estimates", "key": "enabled"})
        )["value"],
        true
    );
}

#[test]
fn app_desktop_at14_sidebar_groups_and_net_worth() {
    let (mut h, _book) = harness_with_chart();
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Savings", "type": "asset", "subtype": "bank", "parent": "Assets"}),
    );
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Car Loan", "type": "liability", "subtype": "loan"}),
    );
    opening(&mut h, "1000.00");
    for (debit, credit, amt) in [
        ("Assets:Savings", "Opening Balances", "500.00"),
        ("Expenses:Groceries", "Liabilities:Visa", "300.00"),
        ("Assets:Checking", "Car Loan", "2000.00"),
    ] {
        cmd(
            &mut h,
            "txn.create",
            json!({"date": "2026-10-03", "splits": [
            {"account": debit, "value": amt}, {"account": credit, "value": format!("-{amt}")}]}),
        );
    }
    h.state_mut().navigate(mk_ui::View::Home);
    h.run();
    open_pane(&mut h, "All accounts");
    assert!(
        has(&h, "Cash & bank  3,500.00"),
        "{:?}",
        labels_containing(&h, "  ")
    );
    assert!(has(&h, "Credit cards  300.00"));
    assert!(has(&h, "Loans & other debts  2,000.00"));
    // Assets 3,500.00 − liabilities 2,300.00.
    assert!(
        has(&h, "Net worth 1,200.00 USD"),
        "{:?}",
        labels_containing(&h, "Net worth")
    );
}

#[test]
fn app_desktop_at15_busy_book_message() {
    let (app, book) = new_book_app();
    let mut h = harness(app);
    let mut other = mk_commands::Session::open(&book.path, "other").unwrap();
    // Hold the write lock from another connection while the app writes.
    std::thread::scope(|scope| {
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        scope.spawn(move || {
            let _ = other.hold_write_lock_for_tests(|| {
                locked_tx.send(()).unwrap();
                done_rx.recv().unwrap();
            });
        });
        locked_rx.recv().unwrap();
        click(&mut h, "Settings");
        settings_section(&mut h, "Features");
        click(&mut h, "Estimates");
        done_tx.send(()).unwrap();
    });
    h.run();
    assert!(
        has_contains(&h, "The book is busy"),
        "{:?}",
        labels_containing(&h, "busy")
    );
}

#[test]
fn app_desktop_at16_no_direct_sqlite() {
    // The UI crates never depend on rusqlite or mk-store; xtask layering enforces
    // it and this test checks the manifests directly.
    for manifest in ["Cargo.toml", "../../apps/moneykeep-app/Cargo.toml"] {
        let path = format!("{}/{manifest}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&path).unwrap();
        let deps = text.split("[dev-dependencies]").next().unwrap();
        assert!(!deps.contains("rusqlite"), "{manifest} depends on rusqlite");
        assert!(!deps.contains("mk-store"), "{manifest} depends on mk-store");
    }
    let layering = std::fs::read_to_string(format!(
        "{}/../../xtask/src/layering.rs",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    // mobile-capture.md: mk-ui may also use the pure capture API crate; the capture
    // server's own inbox file is its only other SQLite user (never a book).
    // ui-slint.md revision 2: the controller has no toolkit dependency.
    assert!(layering.contains("(\"mk-ui\", &[\"mk-capture-api\", \"mk-core\"])"));
    assert!(layering.contains("(\"rusqlite\", &[\"mk-store\", \"moneykeep-server\"])"));
}

#[test]
fn app_desktop_at17_watch_cli_uses_mc_watch() {
    // The CLI's watch tests (apps/moneykeep-cli/tests/cli.rs:
    // import_pipeline_at10_watch_folder) run against mk-watch; here we check the CLI
    // uses the shared crate and the app's background loop is the same code.
    let cli = std::fs::read_to_string(format!(
        "{}/../../apps/moneykeep-cli/src/main.rs",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    assert!(cli.contains("use mk_watch as watch;"));
    let app = std::fs::read_to_string(format!(
        "{}/../../apps/moneykeep-app/src/main.rs",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    assert!(app.contains("mk_watch::watch_book"));
}

// ----- Revision 2 -----------------------------------------------------------------

fn spend(h: &mut mk_views::testing::Harness, date: &str, payee: &str, amt: &str) {
    cmd(
        h,
        "txn.create",
        json!({"date": date, "payee": payee, "splits": [
        {"account": "Expenses:Groceries", "value": amt}, {"account": "Assets:Checking", "value": format!("-{amt}")}]}),
    );
}

fn inside(h: &mk_views::testing::Harness, label: &str, size: mk_views::testing::Vec2) {
    let r = h.get_by_label(label).rect();
    let window = mk_views::testing::Rect::from_min_size(mk_views::testing::Pos2::ZERO, size);
    assert!(
        window.contains_rect(r),
        "{label} at {r:?} is outside the {size:?} window"
    );
}

fn stage_checking_import(h: &mut mk_views::testing::Harness) {
    let bytes = fixture("ofx/checking-v1.ofx");
    h.state_mut().import_file("checking-v1.ofx", &bytes);
    h.run();
}

#[test]
fn app_desktop_at18_everything_fits_from_1000x700() {
    for size in [
        mk_views::testing::vec2(1000.0, 700.0),
        mk_views::testing::vec2(1200.0, 800.0),
    ] {
        let (mut h, _book) = harness_with_chart();
        h.set_size(size);
        opening(&mut h, "1000.00");
        spend(
            &mut h,
            "2026-10-05",
            "A payee with a rather long name that must shrink",
            "50.00",
        );
        open_register(&mut h, "Assets:Checking");
        h.run();
        inside(&h, "Balance", size);
        inside(&h, "+ New", size);
        stage_checking_import(&mut h);
        click(&mut h, "Account for section 1");
        click(&mut h, "Assets:Checking");
        inside(&h, "Skip", size);
        inside(&h, "Skip row 1", size);
    }
}

/// The left/right edges of a node.
fn xs(h: &mk_views::testing::Harness, label: &str) -> mk_views::testing::Rect {
    h.get_by_label(label).rect()
}

fn overlaps(a: mk_views::testing::Rect, b: mk_views::testing::Rect) -> bool {
    a.min.x < b.max.x && b.min.x < a.max.x
}

#[test]
fn app_desktop_at19_headers_share_the_cell_columns() {
    let (mut h, _book) = harness_with_chart();
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-05", "number": "1001", "payee": "Hardware Store", "memo": "Nails", "splits": [
        {"account": "Expenses:Groceries", "value": "50.00"}, {"account": "Assets:Checking", "value": "-50.00"}]}),
    );
    open_register(&mut h, "Assets:Checking");
    let pairs = [
        ("Date", "2026-10-05"),
        ("No.", "1001"),
        ("Payee", "Hardware Store"),
        ("Category", "Expenses:Groceries"),
        ("Memo", "Nails"),
        ("Status", "uncleared"),
    ];
    for (header, cell) in pairs {
        assert!(
            overlaps(xs(&h, header), xs(&h, cell)),
            "{header} {:?} / {cell} {:?}",
            xs(&h, header),
            xs(&h, cell)
        );
    }
    // Spent reads 50.00 (unsigned, revision 8) and Balance -50.00: one cell sits
    // under each header, and amounts are right-aligned, ending where their
    // right-aligned header ends.
    for (header, text) in [("Spent", "50.00"), ("Balance", "-50.00")] {
        let amounts: Vec<mk_views::testing::Rect> =
            h.query_all_by_label(text).map(|n| n.rect()).collect();
        let hr = xs(&h, header);
        let cr = amounts
            .iter()
            .find(|c| overlaps(hr, **c))
            .unwrap_or_else(|| panic!("no amount under {header} {hr:?}: {amounts:?}"));
        assert!(
            (hr.max.x - cr.max.x).abs() < 1.5,
            "{header} {hr:?} / {cr:?}"
        );
    }
    // Text is left-aligned: it starts where its header starts.
    for (header, cell) in [("Date", "2026-10-05"), ("Payee", "Hardware Store")] {
        let (hr, cr) = (xs(&h, header), xs(&h, cell));
        assert!(
            (hr.min.x - cr.min.x).abs() < 1.5,
            "{header} {hr:?} / {cell} {cr:?}"
        );
    }

    // The import table follows the same rules.
    stage_checking_import(&mut h);
    click(&mut h, "Account for section 1");
    click(&mut h, "Assets:Checking");
    for (header, cell) in [
        ("Date", "2026-10-01"),
        ("Payee", "Employer Payroll"),
        ("Memo", "October salary"),
        ("Amount", "1,500.00"),
        ("Skip", "Skip row 1"),
        ("Category", "Category for row 1"),
    ] {
        assert!(
            overlaps(xs(&h, header), xs(&h, cell)),
            "{header} {:?} / {cell} {:?}",
            xs(&h, header),
            xs(&h, cell)
        );
    }
    let (hr, cr) = (xs(&h, "Amount"), xs(&h, "1,500.00"));
    assert!((hr.max.x - cr.max.x).abs() < 1.5, "Amount {hr:?} / {cr:?}");
}

#[test]
fn app_desktop_at20_sidebar_totals() {
    let (mut h, _book) = harness_with_chart();
    opening(&mut h, "1000.00");
    spend(&mut h, "2026-10-03", "Shop", "300.00");
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-04", "payee": "Pay", "splits": [
        {"account": "Assets:Checking", "value": "2000.00"}, {"account": "Salary", "value": "-2000.00"}]}),
    );
    h.run();
    open_pane(&mut h, "All accounts");
    assert!(
        labels_containing(&h, " total").is_empty(),
        "{:?}",
        labels_containing(&h, " total")
    );
    assert!(
        has(&h, "Cash & bank  2,700.00"),
        "{:?}",
        labels_containing(&h, "  ")
    );
    // The Categories heading (not its rail tab) has no total.
    let heading = h
        .query_all_by_label("Categories")
        .find(|n| n.accesskit_node().role() != mk_views::testing::Role::Tab);
    heading.expect("Categories heading without a total").click();
    h.run();
    assert!(
        has(&h, "Income  2,000.00"),
        "{:?}",
        labels_containing(&h, "Income")
    );
    assert!(
        has(&h, "Expenses  300.00"),
        "{:?}",
        labels_containing(&h, "Expenses")
    );
    assert!(labels_containing(&h, " total").is_empty());
}

#[test]
fn app_desktop_at21_suggestion_dropdown_keyboard() {
    let (mut h, _book) = harness_with_chart();
    // Most recent first: Corner Cafe, then Costco.
    spend(&mut h, "2026-10-02", "Costco", "80.00");
    spend(&mut h, "2026-10-03", "Corner Cafe", "4.50");
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    type_into(&mut h, "New payee", "Co");
    h.run();
    let shown = labels_containing(&h, "Payee suggestion ");
    assert_eq!(
        shown,
        ["Payee suggestion Corner Cafe", "Payee suggestion Costco"],
        "dropdown under the field"
    );
    let below = xs(&h, "Payee suggestion Corner Cafe").min.y >= xs(&h, "New payee").max.y - 1.0;
    assert!(below, "the list opens under the field");
    key(&mut h, Key::ArrowDown);
    key(&mut h, Key::Enter);
    assert_eq!(value(&h, "New payee"), "Costco");
    assert!(
        labels_containing(&h, "Payee suggestion ").is_empty(),
        "closed"
    );
    // A fresh "Co": Tab accepts the first (highlighted by default) suggestion.
    h.get_by_label("New payee").focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label("New payee").type_text("Co");
    h.run();
    h.run();
    assert!(has(&h, "Payee suggestion Corner Cafe"));
    key(&mut h, Key::Tab);
    assert_eq!(value(&h, "New payee"), "Corner Cafe");
    assert_eq!(value(&h, "New category"), "Expenses:Groceries");
    // Esc closes the list and keeps the text.
    h.get_by_label("New payee").focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label("New payee").type_text("Cos");
    h.run();
    h.run();
    assert!(has(&h, "Payee suggestion Costco"));
    key(&mut h, Key::Escape);
    assert!(labels_containing(&h, "Payee suggestion ").is_empty());
    assert_eq!(value(&h, "New payee"), "Cos");
    assert!(has(&h, "New transaction"), "Esc closed only the list");
}

#[test]
fn app_desktop_at22_matched_import_row_is_one_line() {
    let (mut h, _book) = harness_with_chart();
    spend(&mut h, "2026-10-03", "City Water", "42.10");
    stage_checking_import(&mut h);
    click(&mut h, "Account for section 1");
    click(&mut h, "Assets:Checking");
    assert!(
        has(&h, "Row 2 City Water: Matches existing"),
        "{:?}",
        labels_containing(&h, "Row 2")
    );
    assert!(has_contains(&h, "Matches 2026-10-03 City Water -42.10"));
    let y: Vec<f32> = ["2026-10-01", "2026-10-03", "2026-10-05"]
        .iter()
        .map(|d| xs(&h, d).center().y)
        .collect();
    let (pitch1, pitch2) = (y[1] - y[0], y[2] - y[1]);
    assert!(
        (pitch1 - pitch2).abs() < 0.5,
        "row pitches {pitch1} / {pitch2}"
    );
    let link = xs(&h, "Add as new instead");
    assert!(
        (link.center().y - y[1]).abs() < pitch1 / 2.0,
        "the link sits in its row"
    );
    click(&mut h, "Add as new instead");
    assert!(
        has(&h, "Row 2 City Water: New"),
        "{:?}",
        labels_containing(&h, "Row 2")
    );
}

fn focused_count(h: &mk_views::testing::Harness) -> usize {
    h.query_all_by(|n| n.is_focused()).count()
}

#[test]
fn app_desktop_at23_one_focus_after_save() {
    let (mut h, _book) = harness_with_chart();
    opening(&mut h, "1000.00");
    open_register(&mut h, "Assets:Checking");
    // Fast entry keeps the pop-up open after a save, with focus back in Date.
    h.state_mut().config.fast_entry = true;
    // Save with Enter.
    enter_keyboard(&mut h, "2026-10-05", "Corner Grocery", "groc", "50.00");
    h.run();
    assert_eq!(focused_count(&h), 1);
    assert!(h.get_by_label("New date").is_focused());
    // Save with the button.
    fill_new(
        &mut h,
        &[
            ("payee", "Bakery"),
            ("category", "Expenses:Groceries"),
            ("spent", "5.00"),
        ],
    );
    click(&mut h, "Save");
    h.run();
    assert!(has(&h, "Closing balance 945.00"));
    assert_eq!(focused_count(&h), 1);
    assert!(h.get_by_label("New date").is_focused());
    assert!(!h.get_by_label("Save").is_focused());
    // With fast entry off, a save closes the pop-up and leaves no focus ring on it.
    h.state_mut().config.fast_entry = false;
    fill_new(
        &mut h,
        &[
            ("payee", "Bakery"),
            ("category", "Expenses:Groceries"),
            ("spent", "5.00"),
        ],
    );
    key(&mut h, Key::Enter);
    h.run();
    assert!(!has(&h, "New transaction"));
    assert!(focused_count(&h) <= 1);
}

fn tab_selected(h: &mk_views::testing::Harness, label: &str) -> Option<bool> {
    h.query_all_by_label(label)
        .find_map(|n| n.accesskit_node().toggled())
        .map(|t| t == mk_views::testing::Toggled::True)
}

#[test]
fn app_desktop_at24_top_bar_highlights_current_view() {
    let (mut h, _book) = harness_with_chart();
    opening(&mut h, "1000.00");
    assert_eq!(tab_selected(&h, "Home"), Some(true));
    open_register(&mut h, "Assets:Checking");
    assert_eq!(tab_selected(&h, "Register"), Some(true));
    assert_eq!(tab_selected(&h, "Home"), Some(false));
    click(&mut h, "Reconcile…");
    assert_eq!(tab_selected(&h, "Reconcile"), Some(true));
    assert_eq!(tab_selected(&h, "Home"), Some(false));
    assert_eq!(
        tab_selected(&h, "Register"),
        None,
        "only the current view's item"
    );
    for item in ["Accounts", "History", "Settings"] {
        click(&mut h, item);
        assert_eq!(tab_selected(&h, item), Some(true), "{item}");
        assert_eq!(tab_selected(&h, "Home"), Some(false));
    }
    stage_checking_import(&mut h);
    assert_eq!(tab_selected(&h, "Import review"), Some(true));
    assert_eq!(tab_selected(&h, "Home"), Some(false));
}

/// Every text shape of the last frame, with its smallest font size.
fn smallest_text(h: &mk_views::testing::Harness) -> Vec<(String, f32)> {
    let mut out = Vec::new();
    for c in &h.output().shapes {
        if c.kind == "text" && !c.text.trim().is_empty() {
            out.push((c.text.clone(), c.size));
        }
    }
    out
}

fn assert_min_text(h: &mk_views::testing::Harness, view: &str) {
    let all = smallest_text(h);
    assert!(!all.is_empty(), "{view}: no text drawn");
    let small: Vec<_> = all
        .iter()
        .filter(|(_, s)| *s < mk_ui::theme::MIN_TEXT)
        .collect();
    assert!(small.is_empty(), "{view}: text under 12 pt: {small:?}");
}

#[test]
fn app_desktop_at25_no_text_under_12pt() {
    let start = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        UserConfig {
            recent: vec!["/home/someone/books/household.moneykeep".into()],
            ..UserConfig::default()
        },
    );
    let mut s = harness(start);
    s.run();
    assert_min_text(&s, "start");

    let (mut h, _book) = harness_with_chart();
    opening(&mut h, "1000.00");
    spend(&mut h, "2026-10-03", "Costco", "80.00");
    h.run();
    click(&mut h, "Categories");
    assert_min_text(&h, "home");
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    type_into(&mut h, "New payee", "Co");
    h.run();
    assert_min_text(&h, "register with suggestions");
    key(&mut h, Key::Escape);
    key(&mut h, Key::Escape);
    assert!(!has(&h, "New transaction"));
    click(&mut h, "Reconcile…");
    assert_min_text(&h, "reconcile start");
    type_into(&mut h, "Statement balance", "920.00");
    click(&mut h, "Start reconciling");
    assert_min_text(&h, "reconcile");
    for view in ["Accounts", "History", "Settings"] {
        click(&mut h, view);
        assert_min_text(&h, view);
    }
    click(&mut h, "Accounts");
    click(&mut h, "New account…");
    assert_min_text(&h, "new account dialog");
    key(&mut h, Key::Escape);
    stage_checking_import(&mut h);
    assert_min_text(&h, "import review (no account)");
    click(&mut h, "Account for section 1");
    click(&mut h, "Assets:Checking");
    assert_min_text(&h, "import review");
}

// ----- Revision 3 -----------------------------------------------------------------

#[test]
fn app_desktop_at27_import_category_cell_fits_its_column() {
    for size in [
        mk_views::testing::vec2(1000.0, 700.0),
        mk_views::testing::vec2(1200.0, 800.0),
    ] {
        let (mut h, _book) = harness_with_chart();
        h.set_size(size);
        spend(&mut h, "2026-10-03", "City Water", "42.10");
        stage_checking_import(&mut h);
        click(&mut h, "Account for section 1");
        click(&mut h, "Assets:Checking");
        assert!(has(&h, "Row 2 City Water: Matches existing"));
        // The Category column runs from its (left-aligned) header to the Skip column.
        let left = xs(&h, "Category").min.x - 1.0;
        let right = xs(&h, "Skip").min.x;
        let window = mk_views::testing::Rect::from_min_size(mk_views::testing::Pos2::ZERO, size);
        let mut widgets = vec![
            xs(&h, "Category for row 1"),
            xs(&h, "Always use this for Employer Payroll"),
            xs(&h, "Category for row 3"),
            xs(&h, "Always use this for Corner Grocery"),
            h.get_by_label_contains("Matches 2026-10-03 City Water")
                .rect(),
        ];
        let link = xs(&h, "Add as new instead");
        widgets.push(link);
        for w in &widgets {
            assert!(
                w.min.x >= left && w.max.x <= right,
                "{size:?}: {w:?} outside the Category column {left}..{right}"
            );
        }
        assert!(
            window.contains_rect(link),
            "{size:?}: link {link:?} not fully visible"
        );
    }
}

#[test]
fn app_desktop_at28_reconcile_start_without_statement_has_no_or_line() {
    let (mut h, _book) = harness_with_chart();
    opening(&mut h, "1000.00");
    open_register(&mut h, "Assets:Checking");
    click(&mut h, "Reconcile…");
    assert!(has(&h, "Statement date"));
    let ors: Vec<String> = labels_containing(&h, "Or ")
        .into_iter()
        .filter(|l| l.starts_with("Or "))
        .collect();
    assert!(ors.is_empty(), "{ors:?}");
    // The fields share one left edge (grid layout).
    let (d, b) = (
        h.get_by_role_and_label(mk_views::testing::Role::TextInput, "Statement date")
            .rect(),
        h.get_by_role_and_label(mk_views::testing::Role::TextInput, "Statement balance")
            .rect(),
    );
    assert!((d.min.x - b.min.x).abs() < 0.5, "{d:?} / {b:?}");
}

#[test]
fn app_desktop_at29_accounts_view_kinds_and_balances() {
    let (mut h, _book) = harness_with_chart();
    // Tall enough that every account group is on screen (queries see only what
    // is drawn).
    h.set_size(vec2(1400.0, 2400.0));
    opening(&mut h, "1000.00");
    spend(&mut h, "2026-10-03", "Shop", "42.50");
    click(&mut h, "Accounts");
    for slug in [
        "bank",
        "credit-card",
        "opening-balances",
        "owner-equity",
        "asset",
        "liability",
        "income",
        "expense",
    ] {
        assert_eq!(
            h.query_all_by_label(slug).count(),
            0,
            "internal slug {slug} shown"
        );
    }
    for kind in [
        "Bank",
        "Credit card",
        "Opening balances",
        "Expense",
        "Income",
    ] {
        assert!(h.query_all_by_label(kind).count() > 0, "kind label {kind}");
    }
    for group in [
        "Cash & bank",
        "Credit cards",
        "Equity",
        "Income",
        "Expenses",
    ] {
        assert!(h.query_all_by_label(group).count() > 0, "group {group}");
    }
    let header = h
        .query_all_by_label("Balance")
        .map(|n| n.rect())
        .max_by(|a, b| a.min.x.total_cmp(&b.min.x))
        .unwrap();
    // Balance cells carry the name "Balance of <account>: <amount>".
    let cells: Vec<(String, mk_views::testing::Rect)> = h
        .query_all_by(|n| n.label().is_some_and(|l| l.starts_with("Balance of ")))
        .map(|n| (n.accesskit_node().label().unwrap_or_default(), n.rect()))
        .collect();
    let names: Vec<&str> = cells.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(cells.len(), 8, "{names:?}");
    assert!(names.contains(&"Balance of Assets:Checking: 957.50"));
    assert!(
        names.contains(&"Balance of Liabilities:Visa: 0.00"),
        "currency scale"
    );
    for (c, r) in &cells {
        assert!(
            (r.max.x - header.max.x).abs() < 1.5,
            "{c} {r:?} / Balance {header:?}"
        );
    }
    // The actions buttons share one column.
    let acts: Vec<f32> = h
        .query_all_by_label_contains("Actions for ")
        .map(|n| n.rect().center().x)
        .collect();
    assert_eq!(acts.len(), 8);
    assert!(acts.iter().all(|x| (x - acts[0]).abs() < 0.5), "{acts:?}");
}

#[test]
fn app_desktop_at30_reconcile_view_without_account_shows_no_error() {
    for spec in [
        "reconcile",
        "reconcile:",
        "reconcile:No Such Account",
        "register",
    ] {
        let (mut h, _book) = harness_with_chart();
        h.state_mut().open_view_spec(spec);
        h.run();
        assert!(!has(&h, "Dismiss"), "{spec}: error banner");
        assert!(labels_containing(&h, "not found").is_empty(), "{spec}");
        let want = if spec.starts_with("reconcile") {
            "Choose an account to reconcile"
        } else {
            "Choose an account to open its register"
        };
        assert!(has(&h, want), "{spec}");
    }
    // Choosing an account opens the reconcile start.
    let (mut h, _book) = harness_with_chart();
    h.state_mut().open_view_spec("reconcile");
    h.run();
    click(&mut h, "Assets:Checking");
    assert!(has(&h, "Reconcile Assets:Checking"));
    assert!(has(&h, "Statement date"));
}

#[test]
fn app_desktop_at31_prefilled_category_opens_no_list() {
    let (mut h, _book) = harness_with_chart();
    spend(&mut h, "2026-10-02", "Costco", "80.00");
    spend(&mut h, "2026-10-03", "Corner Cafe", "4.50");
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    type_into(&mut h, "New payee", "Co");
    h.run();
    assert!(has(&h, "Payee suggestion Corner Cafe"));
    key(&mut h, Key::Enter);
    key(&mut h, Key::Tab);
    h.run();
    assert_eq!(value(&h, "New payee"), "Corner Cafe");
    assert_eq!(value(&h, "New category"), "Expenses:Groceries");
    assert_eq!(focused_count(&h), 1);
    assert!(
        labels_containing(&h, "Category suggestion ").is_empty(),
        "{:?}",
        labels_containing(&h, "Category suggestion ")
    );
    // Typing in Category opens the list again.
    h.get_by_label("New category").focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label("New category").type_text("Gro");
    h.run();
    h.run();
    assert!(has(&h, "Category suggestion Expenses:Groceries"));
}
