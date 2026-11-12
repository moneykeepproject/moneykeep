//! app-desktop.md revision 16 acceptance tests (100–104): dimensions, nonprofit
//! reports, late fees, templates and IIF import.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_views::testing::Role;
use serde_json::{Value, json};

type H = mk_views::testing::Harness;

fn choose(h: &mut H, combo: &str, option: &str) {
    click(h, combo);
    h.get_by_role_and_label(Role::Button, option).click();
    h.run();
}

fn retype(h: &mut H, label: &str, text: &str) {
    h.get_by_label(label).focus();
    h.run();
    h.key_press_modifiers(
        mk_views::testing::Modifiers::COMMAND,
        mk_views::testing::Key::A,
    );
    h.run();
    h.get_by_label(label).type_text(text);
    h.run();
}

/// business-dimensions.md AT1's book: classes and funds on.
fn nonprofit(today: &str) -> (H, TestBook) {
    let (mut h, b) = harness_with_chart();
    for m in ["classes", "funds"] {
        cmd(&mut h, "module.enable", json!({"module": m}));
    }
    cmd(
        &mut h,
        "fund.create",
        json!({"name": "Youth Mission", "restriction": "with_donor_restrictions"}),
    );
    for (n, f) in [
        ("Worship", "program"),
        ("Youth", "program"),
        ("Admin", "management_general"),
        ("Fundraising", "fundraising"),
    ] {
        cmd(&mut h, "class.create", json!({"name": n, "function": f}));
    }
    for n in ["Tithes", "Gifts"] {
        cmd(
            &mut h,
            "account.create",
            json!({"name": n, "type": "income"}),
        );
    }
    for n in ["Salaries", "Youth Trips", "Events"] {
        cmd(
            &mut h,
            "account.create",
            json!({"name": n, "type": "expense", "parent": "Expenses"}),
        );
    }
    h.state_mut().set_today(today);
    h.run();
    (h, b)
}

fn tagged(
    h: &mut H,
    date: &str,
    payee: &str,
    debit: (&str, Value),
    credit: (&str, Value),
    amount: &str,
) -> String {
    let mut d = json!({"account": debit.0, "value": amount});
    let mut c = json!({"account": credit.0, "value": format!("-{amount}")});
    for (k, v) in debit.1.as_object().unwrap() {
        d[k] = v.clone();
    }
    for (k, v) in credit.1.as_object().unwrap() {
        c[k] = v.clone();
    }
    cmd(
        h,
        "txn.create",
        json!({"date": date, "payee": payee, "splits": [d, c]}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn at1_data(h: &mut H, trip_fund: Option<&str>) {
    tagged(
        h,
        "2026-01-10",
        "Tithes",
        ("Assets:Checking", json!({"fund": "General"})),
        ("Tithes", json!({"fund": "General"})),
        "10000.00",
    );
    tagged(
        h,
        "2026-02-01",
        "Gift",
        ("Assets:Checking", json!({"fund": "Youth Mission"})),
        ("Gifts", json!({"fund": "Youth Mission"})),
        "2000.00",
    );
    tagged(
        h,
        "2026-03-01",
        "Payroll",
        (
            "Expenses:Salaries",
            json!({"class": "Admin", "fund": "General"}),
        ),
        ("Assets:Checking", json!({})),
        "4000.00",
    );
    let trip = match trip_fund {
        Some(f) => json!({"class": "Youth", "fund": f}),
        None => json!({"class": "Youth"}),
    };
    tagged(
        h,
        "2026-04-01",
        "Camp Trip",
        ("Expenses:Youth Trips", trip),
        ("Assets:Checking", json!({})),
        "1500.00",
    );
    tagged(
        h,
        "2026-05-01",
        "Gala",
        (
            "Expenses:Events",
            json!({"class": "Fundraising", "fund": "General"}),
        ),
        ("Assets:Checking", json!({})),
        "300.00",
    );
    h.run();
}

fn open_report(h: &mut H, title: &str) {
    h.state_mut().open_view_spec("reports");
    h.run();
    h.get_by_role_and_label(Role::Tab, title).click();
    h.run();
}

#[test]
fn app_desktop_at100_statement_of_activities() {
    let (mut h, _b) = nonprofit("2026-12-31");
    at1_data(&mut h, Some("Youth Mission"));
    open_report(&mut h, "Statement of activities");
    choose(&mut h, "Period", "This year");
    assert!(has(&h, "Without donor restrictions"));
    assert!(has(&h, "With donor restrictions"));
    assert!(has(&h, "Change in net assets"));
    assert!(
        has(&h, "Change in net assets 5,700.00 / 500.00 / 6,200.00"),
        "{:?}",
        labels_containing(&h, "Change")
    );
    for v in ["5,700.00", "500.00", "6,200.00"] {
        assert!(h.query_all_by_label(v).count() >= 1, "{v}");
    }
    // The other nonprofit reports and Profit and loss by fund are listed.
    for t in [
        "Profit and loss",
        "Balance sheet by fund",
        "Statement of financial position",
        "Functional expenses",
        "Fund activity",
    ] {
        assert!(h.query_by_role_and_label(Role::Tab, t).is_some(), "{t}");
    }
    open_report(&mut h, "Profit and loss");
    choose(&mut h, "Period", "This year");
    choose(&mut h, "Columns", "Fund");
    assert!(has(&h, "General") && has(&h, "Youth Mission"));
    assert!(has(&h, "Net income"));
    open_report(&mut h, "Statement of financial position");
    assert!(has(&h, "Total net assets"));
}

#[test]
fn app_desktop_at101_fund_in_edit_popup_creates_release() {
    let (mut h, _b) = nonprofit("2026-12-31");
    at1_data(&mut h, None);
    let releases = |h: &mut H| -> usize {
        cmd(h, "txn.list", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["payee"] == "Released from restrictions")
            .count()
    };
    assert_eq!(releases(&mut h), 0);
    open_register(&mut h, "Assets:Checking");
    h.get_by_label("Camp Trip").click();
    h.run();
    assert!(has(&h, "Edit transaction"));
    choose(&mut h, "Edit fund", "Youth Mission");
    click(&mut h, "Save");
    assert!(!has(&h, "Edit transaction"));
    assert_eq!(releases(&mut h), 1);
    // The release shows in its register, marked as made for an expense.
    h.state_mut()
        .open_view_spec("register:Net Assets Released from Restrictions");
    h.run();
    assert!(
        h.query_all_by_label_contains("Released from restrictions  · for an expense")
            .count()
            >= 1,
        "{:?}",
        labels_containing(&h, "Released")
    );
    // Undo removes both the tag and the release.
    h.get_by_role_and_label(Role::Button, "Undo").click();
    h.run();
    assert_eq!(releases(&mut h), 0);
    let txns = cmd(&mut h, "txn.list", json!({}));
    let trip = txns
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["payee"] == "Camp Trip")
        .unwrap()
        .clone();
    let t = cmd(&mut h, "txn.get", json!({"id": trip["id"]}));
    assert!(
        t["splits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["tag_names"]["fund"].is_null()),
        "{t}"
    );
}

#[test]
fn app_desktop_r16_funds_tab_release_and_overspent() {
    let (mut h, _b) = nonprofit("2026-12-31");
    at1_data(&mut h, Some("Youth Mission"));
    tagged(
        &mut h,
        "2026-06-01",
        "Bus",
        (
            "Expenses:Youth Trips",
            json!({"class": "Youth", "fund": "Youth Mission"}),
        ),
        ("Assets:Checking", json!({})),
        "900.00",
    );
    h.state_mut().open_view_spec("home");
    h.run();
    assert!(
        has(&h, "Youth Mission fund is overspent by 400.00"),
        "{:?}",
        labels_containing(&h, "overspent")
    );
    click(&mut h, "Youth Mission fund is overspent by 400.00");
    assert!(has(&h, "New fund…"), "the link opens the Funds tab");
    h.state_mut().open_view_spec("accounts");
    h.run();
    for t in ["Chart of accounts", "Classes", "Funds"] {
        assert!(h.query_by_role_and_label(Role::Tab, t).is_some(), "{t}");
    }
    assert!(h.query_by_role_and_label(Role::Tab, "Locations").is_none());
    h.get_by_role_and_label(Role::Tab, "Funds").click();
    h.run();
    assert!(
        has(&h, "Overspent by 400.00"),
        "{:?}",
        labels_containing(&h, "Overspent")
    );
    click(&mut h, "New fund…");
    type_into(&mut h, "Name", "Building");
    choose(&mut h, "Restriction", "With donor restrictions");
    click(&mut h, "Save");
    let funds = cmd(&mut h, "fund.list", json!({}));
    assert!(
        funds
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "Building" && f["restriction"] == "with_donor_restrictions")
    );
    click(&mut h, "Transfer between funds…");
    choose(&mut h, "From fund", "General");
    choose(&mut h, "To fund", "Youth Mission");
    type_into(&mut h, "Amount", "400.00");
    retype(&mut h, "Date", "2026-07-01");
    click(&mut h, "Transfer");
    assert!(!has_contains(&h, "Overspent by"));
    // The Classes tab shows functions and the active toggle.
    h.get_by_role_and_label(Role::Tab, "Classes").click();
    h.run();
    assert!(has(&h, "Management and general"));
    click(&mut h, "Active Worship");
    let classes = cmd(&mut h, "class.list", json!({}));
    assert!(
        classes
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "Worship" && c["active"] == false)
    );
}

/// business-sales.md's setup with statements on, INV-1001 and a 1,000.00 payment.
fn sales_book(today: &str) -> (H, TestBook) {
    let (mut h, b) = harness_with_chart();
    for m in ["invoicing", "items", "sales-tax", "statements"] {
        cmd(&mut h, "module.enable", json!({"module": m}));
    }
    for n in ["Consulting Income", "Product Sales"] {
        cmd(
            &mut h,
            "account.create",
            json!({"name": n, "type": "income"}),
        );
    }
    cmd(
        &mut h,
        "item.create",
        json!({"name": "Consulting", "kind": "service", "price": "125.00", "income_account": "Consulting Income"}),
    );
    cmd(
        &mut h,
        "item.create",
        json!({"name": "Widget", "kind": "product", "price": "19.99", "income_account": "Product Sales"}),
    );
    cmd(
        &mut h,
        "tax_rate.create",
        json!({"name": "State", "rate": "6.00"}),
    );
    cmd(
        &mut h,
        "tax_rate.create",
        json!({"name": "County Rate", "rate": "1.25"}),
    );
    cmd(
        &mut h,
        "tax_rate.create",
        json!({"kind": "group", "name": "County", "members": ["State", "County Rate"]}),
    );
    cmd(
        &mut h,
        "party.create",
        json!({"name": "Acme LLC", "terms": {"type": "net", "days": 30}, "tax_rate": "County"}),
    );
    cmd(
        &mut h,
        "invoice.create",
        json!({"customer": "Acme LLC", "date": "2026-09-01", "lines": [
            {"item": "Consulting", "quantity": "10"}, {"item": "Widget", "quantity": "3"}]}),
    );
    cmd(
        &mut h,
        "payment.receive",
        json!({"customer": "Acme LLC", "date": "2026-09-20", "amount": "1000.00"}),
    );
    h.state_mut().set_today(today);
    h.run();
    (h, b)
}

fn open_sales(h: &mut H, tab: &str) {
    open_feature(h, "Sales");
    h.get_by_role_and_label(Role::Tab, tab).click();
    h.run();
}

#[test]
fn app_desktop_at102_late_fees() {
    let (mut h, _b) = sales_book("2026-10-10");
    for (k, v) in [
        ("statements.late_fee_kind", json!("percent")),
        ("statements.late_fee_percent", json!("1.5")),
        ("statements.late_fee_min", json!("2.00")),
        ("statements.grace_days", json!(5)),
    ] {
        cmd(&mut h, "module.set_setting", json!({"key": k, "value": v}));
    }
    h.run();
    open_sales(&mut h, "Invoices");
    click(&mut h, "Late fees…");
    assert_eq!(value(&h, "As of date"), "2026-10-10");
    assert!(
        has(&h, "Charge a late fee on INV-1001"),
        "{:?}",
        labels_containing(&h, "INV")
    );
    assert!(
        h.query_all_by_label("4.71").count() >= 1,
        "{:?}",
        labels_containing(&h, "4.")
    );
    assert!(has(&h, "Total fees 4.71"));
    // Unticking the only row disables Assess; ticking it again enables it.
    click(&mut h, "Charge a late fee on INV-1001");
    assert!(disabled(&h, "Assess selected"));
    click(&mut h, "Charge a late fee on INV-1001");
    click(&mut h, "Assess selected");
    assert!(!has(&h, "Late fees"));
    let list = cmd(&mut h, "invoice.list", json!({}));
    let fee = list
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["total"] == "4.71")
        .expect("the late-fee invoice");
    assert_eq!(fee["customer_name"], "Acme LLC");
    // Nothing more is due afterwards.
    click(&mut h, "Late fees…");
    assert!(has(&h, "No late fees are due on that date."));
}

#[test]
fn app_desktop_r16_customer_statement() {
    let (mut h, _b) = sales_book("2026-10-31");
    open_sales(&mut h, "Customers");
    click(&mut h, "Acme LLC");
    click(&mut h, "Statement…");
    assert!(has(&h, "Statement for Acme LLC"));
    retype(&mut h, "From date", "2026-09-01");
    assert!(
        has(&h, "Amount due 314.32"),
        "{:?}",
        labels_containing(&h, "Amount due")
    );
    click(&mut h, "PDF…");
    let ev = events(&mut h);
    assert!(
        ev.iter().any(
            |e| matches!(e, mk_ui::AppEvent::SaveBytes { file_name, data }
            if file_name.starts_with("Statement Acme LLC") && data.starts_with(b"%PDF"))
        ),
        "{ev:?}"
    );
    click(&mut h, "Close");
    open_sales(&mut h, "Invoices");
    click(&mut h, "Statements…");
    assert!(has(&h, "Acme LLC 314.32"));
    click(&mut h, "Create PDFs…");
    let ev = events(&mut h);
    assert!(
        ev.iter()
            .any(|e| matches!(e, mk_ui::AppEvent::SaveFilesToFolder { files } if files.len() == 1)),
        "{ev:?}"
    );
}

#[test]
fn app_desktop_at103_template_logo() {
    let (mut h, _b) = sales_book("2026-10-10");
    cmd(
        &mut h,
        "template.create",
        json!({"name": "Brand", "accent_color": "#1F6F78"}),
    );
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Templates").click();
    h.run();
    click(&mut h, "Template Brand");
    assert!(has_contains(&h, "at most 512 KB and 2,000 × 2,000 pixels"));
    click(&mut h, "Upload logo…");
    let ev = events(&mut h);
    assert!(
        ev.iter()
            .any(|e| matches!(e, mk_ui::AppEvent::ChooseLogoFile { .. })),
        "{ev:?}"
    );
    let big = mk_import::image::solid_png(3000, 3000, [0, 0, 0]);
    h.state_mut().set_template_logo("Brand", &big);
    h.run();
    assert!(
        has_contains(&h, "2000"),
        "{:?}",
        labels_containing(&h, "pixel")
    );
    assert!(h.query_by_label_contains("Logo preview").is_none());
    let logo = mk_import::image::solid_png(300, 100, [0x1F, 0x6F, 0x78]);
    h.state_mut().set_template_logo("Brand", &logo);
    h.run();
    assert!(
        has(&h, "Logo preview 300 × 100"),
        "{:?}",
        labels_containing(&h, "Logo")
    );
    assert!(!has_contains(&h, "2000"));
    // Editing and the default template.
    retype(&mut h, "Footer", "Thank you");
    click(&mut h, "Save template");
    let t = cmd(&mut h, "template.get", json!({"template": "Brand"}));
    assert_eq!(t["footer"], "Thank you");
    choose(&mut h, "Default template", "Brand");
    let s = cmd(&mut h, "module.settings", json!({"module": "invoicing"}));
    let d = s["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == "invoicing.default_template")
        .unwrap()
        .clone();
    assert_eq!(d["value"], t["id"]);
    click(&mut h, "Remove logo");
    assert!(h.query_by_label_contains("Logo preview").is_none());
}

const AT10_IIF: &str = "!TRNS\tTRNSTYPE\tDATE\tACCNT\tNAME\tCLASS\tAMOUNT\tDOCNUM\tMEMO\r\n!SPL\tTRNSTYPE\tDATE\tACCNT\tNAME\tCLASS\tAMOUNT\tDOCNUM\tMEMO\r\n!ENDTRNS\r\nTRNS\tCHECK\t9/5/2026\tBank Checking\tPost Office\t\t-70.00\t1001\tStamps\r\nSPL\tCHECK\t9/5/2026\tOffice Supplies\t\t\t10.00\t\t\r\nSPL\tCHECK\t9/5/2026\tPostage\t\t\t60.00\t\t\r\nENDTRNS\r\n";

#[test]
fn app_desktop_at104_iif_accounts_mapping() {
    let (mut h, _b) = harness_with_chart();
    h.state_mut().set_today("2026-10-10");
    h.state_mut().import_file("books.iif", AT10_IIF.as_bytes());
    h.run();
    assert!(has(&h, "Accounts in this file"));
    // The table's column headers are named now ("Map to"); the pickers are counted.
    let maps: Vec<String> = labels_containing(&h, "Map ")
        .into_iter()
        .filter(|l| l != "Map to")
        .collect();
    assert_eq!(
        maps,
        ["Map Bank Checking", "Map Office Supplies", "Map Postage"],
        "{maps:?}"
    );
    assert_eq!(h.query_all_by_label("Not mapped").count(), 3);
    choose(&mut h, "Map Bank Checking", "Assets:Checking");
    choose(&mut h, "Create Office Supplies as", "Expense");
    choose(&mut h, "Create Postage as", "Expense");
    assert_eq!(h.query_all_by_label("Not mapped").count(), 0);
    click(&mut h, "Add to books");
    let txns = cmd(&mut h, "txn.list", json!({}));
    assert_eq!(txns.as_array().unwrap().len(), 1);
    let t = cmd(&mut h, "txn.get", json!({"id": txns[0]["id"]}));
    assert_eq!(t["splits"].as_array().unwrap().len(), 3);
    // Export IIF… (on Accounts) states its limits before saving.
    h.state_mut().open_view_spec("accounts");
    h.run();
    click(&mut h, "Export IIF…");
    retype(&mut h, "From date", "2026-01-01");
    assert!(has_contains(
        &h,
        "no memos, check numbers or document details"
    ));
    assert!(has_contains(&h, "written as '?'"));
    click(&mut h, "Export…");
    let ev = events(&mut h);
    assert!(
        ev.iter().any(
            |e| matches!(e, mk_ui::AppEvent::SaveFile { file_name, content }
            if file_name.ends_with(".iif") && content.contains("Postage"))
        ),
        "{ev:?}"
    );
}

#[test]
fn app_desktop_r16_tag_pickers_new_popup_and_required() {
    let (mut h, _b) = nonprofit("2026-10-05");
    cmd(
        &mut h,
        "module.set_setting",
        json!({"key": "classes.required", "value": true}),
    );
    h.run();
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    assert!(has(&h, "New class") && has(&h, "New fund"));
    fill_new(
        &mut h,
        &[
            ("date", "2026-10-05"),
            ("payee", "Bus Co"),
            ("category", "Expenses:Youth Trips"),
            ("spent", "80.00"),
        ],
    );
    // A required class is asked for before saving.
    click(&mut h, "Save");
    assert!(
        has(&h, "Choose a class."),
        "{:?}",
        labels_containing(&h, "class")
    );
    choose(&mut h, "New class", "Youth");
    choose(&mut h, "New fund", "Youth Mission");
    click(&mut h, "Save");
    let txns = cmd(&mut h, "txn.list", json!({}));
    let bus = txns
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["payee"] == "Bus Co")
        .expect("saved")
        .clone();
    let t = cmd(&mut h, "txn.get", json!({"id": bus["id"]}));
    let cat = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["account_name"] == "Expenses:Youth Trips")
        .unwrap()
        .clone();
    assert_eq!(cat["tag_names"]["class"], "Youth");
    assert_eq!(cat["tag_names"]["fund"], "Youth Mission");
}

#[test]
fn app_desktop_r16_invoice_class_picker() {
    let (mut h, _b) = sales_book("2026-10-10");
    cmd(&mut h, "module.enable", json!({"module": "classes"}));
    cmd(&mut h, "class.create", json!({"name": "Consulting dept"}));
    h.run();
    open_sales(&mut h, "Invoices");
    click(&mut h, "INV-1001");
    choose(&mut h, "Class", "Consulting dept");
    click(&mut h, "Save");
    let inv = cmd(&mut h, "invoice.get", json!({"invoice": "INV-1001"}));
    assert_eq!(inv["tag_names"]["class"], "Consulting dept", "{inv}");
}
