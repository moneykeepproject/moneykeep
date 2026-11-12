//! app-desktop.md revision 28 acceptance tests 175–178 (Phase 6a): the Expenses
//! screen, receipts dropped on the expense pop-up, posting and matching, the
//! register's paperclip and Receipts row, and the expense report's save dialog.
//! (179, phone capture, belongs to Phase 6b.)
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_arithmetic
)]

use crate::common::*;
use mk_ui::{AppEvent, PickedFile, ReceiptTarget, View};
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

/// A file dropped on the window.
fn drop_file(h: &mut H, name: &str, bytes: Vec<u8>) {
    h.drop_files(vec![PickedFile {
        name: name.to_owned(),
        bytes: Ok(bytes),
    }]);
    h.run();
}

/// A small JPEG receipt; `shade` makes each one different.
fn jpeg(shade: u8) -> Vec<u8> {
    let (w, h) = (90u32, 120u32);
    let mut data = Vec::new();
    for y in 0..h {
        for x in 0..w {
            data.extend(if y < 20 {
                [shade, 40, 40]
            } else if (x / 10 + y / 10) % 2 == 0 {
                [250, 250, 250]
            } else {
                [30, 30, 30]
            });
        }
    }
    mk_import::jpeg::encode_jpeg(
        &mk_import::image::Rgb {
            width: w,
            height: h,
            data,
        },
        85,
    )
    .unwrap()
}

/// The standard chart plus Office Supplies, with the expenses module on.
fn book() -> (H, TestBook) {
    let (mut h, b) = harness_with_chart();
    h.state_mut().set_today("2026-10-23");
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Office Supplies", "type": "expense", "parent": "Expenses"}),
    );
    cmd(&mut h, "module.enable", json!({"module": "expenses"}));
    h.run();
    (h, b)
}

fn by_role<'h>(h: &'h H, role: Role, label: &'h str) -> mk_views::testing::Node<'h> {
    h.get_by_role_and_label(role, label)
}

fn type_field(h: &mut H, label: &str, text: &str) {
    by_role(h, Role::TextInput, label).focus();
    h.run();
    h.key_press_modifiers(
        mk_views::testing::Modifiers::COMMAND,
        mk_views::testing::Key::A,
    );
    by_role(h, Role::TextInput, label).type_text(text);
    h.run();
}

fn choose(h: &mut H, combo: &str, option: &str) {
    by_role(h, Role::ComboBox, combo).click();
    h.run();
    h.get_by_role_and_label(Role::Button, option).click();
    h.run();
}

/// Open the Expenses screen and a new expense with a dropped JPEG, filled in as
/// Office Depot, 45.20, Office Supplies, paid with Visa.
fn new_office_depot(h: &mut H) {
    open_feature(h, "Expenses");
    click(h, "+ New expense");
    assert!(has(h, "New expense"));
    drop_file(h, "depot.jpg", jpeg(200));
    assert!(
        has(h, "Receipt thumbnail depot.jpg"),
        "{:?}",
        labels_containing(h, "Receipt")
    );
    type_field(h, "Merchant", "Office Depot");
    type_field(h, "Amount", "45.20");
    choose(h, "Category", "Expenses:Office Supplies");
    choose(h, "Paid with", "Liabilities:Visa");
}

fn txns(h: &mut H) -> Vec<serde_json::Value> {
    cmd(h, "txn.list", json!({})).as_array().unwrap().clone()
}

#[test]
fn app_desktop_at175_features_lists_expenses_with_both_tabs() {
    let (mut h, _b) = harness_with_chart();
    assert!(!feature_items(&mut h).contains(&"Expenses".to_owned()));
    cmd(&mut h, "module.enable", json!({"module": "expenses"}));
    h.run();
    let items = feature_items(&mut h);
    assert!(items.contains(&"Expenses".to_owned()), "{items:?}");
    let mut sorted = items.clone();
    sorted.sort();
    assert_eq!(items, sorted, "alphabetical");
    open_feature(&mut h, "Expenses");
    assert_eq!(h.state().view(), &View::Expenses);
    by_role(&h, Role::Tab, "Expenses");
    by_role(&h, Role::Tab, "Inbox");
    for b in [
        "+ New expense",
        "Add receipts…",
        "Expense report…",
        "Record reimbursement…",
    ] {
        assert!(has(&h, b), "{b}");
    }
    assert!(disabled(&h, "Record reimbursement…"));
    by_role(&h, Role::Tab, "Inbox").click();
    h.run();
    assert!(has(&h, "The inbox is empty."));
    // Add receipts… asks the app for files for the inbox.
    click(&mut h, "Add receipts…");
    assert!(events(&mut h).contains(&AppEvent::ChooseReceiptFiles {
        target: ReceiptTarget::Inbox
    }));
}

#[test]
fn app_desktop_at176_dropped_jpeg_save_and_post_paperclip_and_receipts_row() {
    let (mut h, _b) = book();
    new_office_depot(&mut h);
    click(&mut h, "Save and post");
    assert!(!has(&h, "New expense"), "the pop-up closed");
    let list = txns(&mut h);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["payee"], "Office Depot");
    let exp = cmd(&mut h, "expense.list", json!({}));
    assert_eq!(exp[0]["status"], "posted");
    assert!(
        has(&h, "Has receipts"),
        "the expense list shows the paperclip"
    );
    open_register(&mut h, "Liabilities:Visa");
    assert!(has(&h, "Has receipts"));
    let row = h
        .query_all_by_label_contains("Row 1:")
        // Register rows are table rows (ui-slint.md revision 2).
        .find(|n| matches!(n.accesskit_node().role(), Role::Button | Role::Row))
        .unwrap();
    assert!(
        row.accesskit_node()
            .label()
            .unwrap_or_default()
            .ends_with(", has receipts")
    );
    row.click();
    h.run();
    assert!(has(&h, "Edit transaction"));
    assert!(has(&h, "Receipts"));
    assert!(has(&h, "Receipt thumbnail depot.jpg"));
    assert!(has(&h, "Add receipt…"));
    // View opens the viewer with zoom and rotate.
    click(&mut h, "View receipt 1");
    assert!(has(&h, "Receipt: depot.jpg"));
    assert!(has(&h, "Receipt picture"));
    click(&mut h, "Rotate");
    click(&mut h, "Zoom in");
    assert!(has(&h, "125 %"));
}

#[test]
fn app_desktop_at177_one_candidate_shows_match() {
    let (mut h, _b) = book();
    let t = cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-25", "payee": "OFFICE DEPOT #1", "splits": [
            {"account": "Expenses:Groceries", "value": "45.20"},
            {"account": "Liabilities:Visa", "value": "-45.20"}]}),
    );
    new_office_depot(&mut h);
    click(&mut h, "Save and post");
    assert!(has(&h, "This expense may already be in your books"));
    assert!(has(&h, "OFFICE DEPOT #1"));
    assert!(has(&h, "Create new"));
    click(&mut h, "Match");
    assert!(!has(&h, "Edit expense"), "the pop-up closed");
    let list = txns(&mut h);
    assert_eq!(list.len(), 1, "no new row");
    let exp = cmd(&mut h, "expense.list", json!({}));
    assert_eq!(exp[0]["status"], "matched");
    assert_eq!(exp[0]["transaction"], t["id"]);
    let receipts = cmd(&mut h, "receipt.list", json!({"transaction": t["id"]}));
    assert_eq!(receipts.as_array().unwrap().len(), 1);
}

#[test]
fn app_desktop_at178_report_saved_through_the_save_dialog() {
    let (mut h, _b) = book();
    let r = cmd(
        &mut h,
        "receipt.add",
        json!({"content_base64": base64_of(&jpeg(10)), "file_name": "lunch.jpg"}),
    );
    cmd(
        &mut h,
        "expense.create",
        json!({"date": "2026-10-08", "merchant": "Cafe Lune", "amount": "18.75",
               "category": "Expenses:Office Supplies", "paid_with": "personal",
               "receipts": [r["id"]]}),
    );
    open_feature(&mut h, "Expenses");
    click(&mut h, "Expense report…");
    assert!(has(&h, "Expense report"));
    assert_eq!(value(&h, "Report from"), "2026-10-01");
    assert_eq!(value(&h, "Report to"), "2026-10-31");
    assert!(has(&h, "Include receipt pictures"));
    assert!(has(&h, "Mark these expenses as reported"));
    events(&mut h);
    click(&mut h, "Save PDF…");
    let saved = events(&mut h)
        .into_iter()
        .find_map(|e| match e {
            AppEvent::SaveBytes { file_name, data } => Some((file_name, data)),
            _ => None,
        })
        .expect("a save dialog");
    assert!(saved.0.ends_with(".pdf"), "{}", saved.0);
    assert!(saved.1.starts_with(b"%PDF-1.4"));
    let text: String = saved.1.iter().map(|b| char::from(*b)).collect();
    assert!(text.contains("(Cafe Lune)"));
    assert!(text.contains("/DCTDecode"));
    assert!(!has(&h, "Save PDF…"), "the dialog closed");
}

/// expenses.md revision 2: "Mark these expenses as reported" marks them only after
/// the PDF is written, through `expense.mark_reported` (the report itself is
/// read-only).
#[test]
fn report_marks_expenses_reported_only_once_saved() {
    let (mut h, _b) = book();
    let e = cmd(
        &mut h,
        "expense.create",
        json!({"date": "2026-10-08", "merchant": "Cafe Lune", "amount": "18.75",
               "category": "Expenses:Office Supplies", "paid_with": "personal"}),
    );
    open_feature(&mut h, "Expenses");
    click(&mut h, "Expense report…");
    click(&mut h, "Mark these expenses as reported");
    events(&mut h);
    click(&mut h, "Save PDF…");
    let (data, expenses) = events(&mut h)
        .into_iter()
        .find_map(|ev| match ev {
            AppEvent::SaveExpenseReport { data, expenses, .. } => Some((data, expenses)),
            _ => None,
        })
        .expect("a save dialog that marks after saving");
    assert!(data.starts_with(b"%PDF-1.4"));
    assert_eq!(expenses, vec![e["id"].as_str().unwrap().to_owned()]);
    // Not marked while the save dialog is open (or if it is cancelled).
    let got = cmd(&mut h, "expense.get", json!({"expense": e["id"]}));
    assert_eq!(got["reported"], false);
    h.state_mut().expense_report_saved(&expenses);
    h.run();
    assert!(has(&h, "Marked 1 expense as reported."));
    let got = cmd(&mut h, "expense.get", json!({"expense": e["id"]}));
    assert_eq!(got["reported"], true);
}

/// expenses.md revision 2: opening a PDF receipt says a temporary unencrypted copy
/// will be opened, then hands the file to the app shell.
#[test]
fn pdf_receipt_confirmation_mentions_the_temporary_copy() {
    let (mut h, _b) = book();
    open_feature(&mut h, "Expenses");
    let pdf = b"%PDF-1.4\n<< /Type /Page >>\n%%EOF".to_vec();
    drop_file(&mut h, "invoice.pdf", pdf.clone());
    by_role(&h, Role::Tab, "Inbox").click();
    h.run();
    click(&mut h, "PDF receipt invoice.pdf");
    assert!(has(&h, "Open PDF receipt"));
    assert!(has(&h, "A temporary unencrypted copy will be opened."));
    events(&mut h);
    click(&mut h, "Open");
    let opened = events(&mut h)
        .into_iter()
        .find_map(|ev| match ev {
            AppEvent::OpenReceiptPdf { file_name, data } => Some((file_name, data)),
            _ => None,
        })
        .expect("the PDF goes to the system viewer");
    assert_eq!(opened.0, "invoice.pdf");
    assert_eq!(opened.1, pdf);
}

/// Closing the book tells the app shell, which deletes the temporary PDF copies.
#[test]
fn closing_the_book_is_announced_to_the_app() {
    let (mut h, _b) = book();
    events(&mut h);
    h.state_mut().close_book();
    assert!(
        events(&mut h)
            .iter()
            .any(|ev| matches!(ev, AppEvent::BookClosed))
    );
}

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[test]
fn receipts_dropped_on_the_screen_go_to_the_inbox_and_home_attention() {
    let (mut h, _b) = book();
    open_feature(&mut h, "Expenses");
    drop_file(&mut h, "a.jpg", jpeg(1));
    drop_file(&mut h, "b.jpg", jpeg(2));
    assert!(has(&h, "Added 1 receipt to the inbox."));
    by_role(&h, Role::Tab, "Inbox").click();
    h.run();
    assert!(has(&h, "Receipt thumbnail a.jpg"));
    assert!(has(&h, "New expense from b.jpg"));
    // The same picture again asks first.
    drop_file(&mut h, "a-again.jpg", jpeg(1));
    assert!(has(&h, "Receipt already added"));
    assert!(has_contains(
        &h,
        "This receipt is already attached to a receipt in the inbox."
    ));
    click(&mut h, "Cancel");
    assert_eq!(
        cmd(&mut h, "receipt.inbox", json!({}))
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // Home lists the inbox under Attention and opens the Inbox tab.
    click(&mut h, "Home");
    assert!(has(&h, "2 receipts in the inbox"));
    click(&mut h, "2 receipts in the inbox");
    assert_eq!(h.state().view(), &View::Expenses);
    assert!(has(&h, "Receipt thumbnail b.jpg"));
    // New expense from an inbox receipt carries it into the pop-up.
    click(&mut h, "New expense from b.jpg");
    assert!(has(&h, "New expense"));
    // In the pop-up's strip (the inbox behind the modal pop-up is out of reach
    // for assistive technology; it is checked again after the pop-up closes).
    assert_eq!(h.query_all_by_label("Receipt thumbnail b.jpg").count(), 1);
    type_field(&mut h, "Merchant", "Hardware");
    click(&mut h, "Save");
    let e = cmd(&mut h, "expense.list", json!({}));
    assert_eq!(e[0]["status"], "draft");
    assert_eq!(e[0]["receipt_count"], 1);
    assert_eq!(
        cmd(&mut h, "receipt.inbox", json!({}))
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Phone expenses (Phase 6b feeds them) are counted under Attention.
    cmd(
        &mut h,
        "expense.create",
        json!({"date": "2026-10-20", "merchant": "Taxi", "source": "phone"}),
    );
    click(&mut h, "Home");
    assert!(has(&h, "1 expense from your phone to review"));
    click(&mut h, "1 expense from your phone to review");
    assert!(has(&h, "From phone"));
}

#[test]
fn register_popup_adds_receipts_from_the_dialog_and_hides_without_the_module() {
    let (mut h, _b) = book();
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-10-03", "payee": "Corner Grocery", "splits": [
            {"account": "Expenses:Groceries", "value": "12.00"},
            {"account": "Assets:Checking", "value": "-12.00"}]}),
    );
    open_register(&mut h, "Assets:Checking");
    assert!(!has(&h, "Has receipts"));
    h.query_all_by_label_contains("Row 1:")
        // Register rows are table rows (ui-slint.md revision 2).
        .find(|n| matches!(n.accesskit_node().role(), Role::Button | Role::Row))
        .unwrap()
        .click();
    h.run();
    click(&mut h, "Add receipt…");
    assert!(events(&mut h).contains(&AppEvent::ChooseReceiptFiles {
        target: ReceiptTarget::TransactionPopup
    }));
    h.state_mut().add_receipt_files(
        ReceiptTarget::TransactionPopup,
        vec![
            PickedFile {
                name: "milk.jpg".into(),
                bytes: Ok(jpeg(50)),
            },
            PickedFile {
                name: "big.jpg".into(),
                bytes: Err(mk_ui::TOO_BIG.into()),
            },
        ],
    );
    h.run();
    assert!(has(&h, "Receipt thumbnail milk.jpg"));
    assert!(has_contains(&h, "big.jpg: Receipts can be at most 10 MB."));
    // Dropping onto the pop-up adds another.
    drop_file(&mut h, "milk2.jpg", jpeg(60));
    assert!(has(&h, "Receipt thumbnail milk2.jpg"));
    click(&mut h, "Remove receipt 2");
    assert!(!has(&h, "Receipt thumbnail milk2.jpg"));
    click(&mut h, "Cancel");
    assert!(has(&h, "Has receipts"));
    // Turning the module off hides the paperclip and the Receipts row.
    cmd(&mut h, "module.disable", json!({"module": "expenses"}));
    h.run();
    assert!(!has(&h, "Has receipts"));
    h.query_all_by_label_contains("Row 1:")
        // Register rows are table rows (ui-slint.md revision 2).
        .find(|n| matches!(n.accesskit_node().role(), Role::Button | Role::Row))
        .unwrap()
        .click();
    h.run();
    assert!(has(&h, "Edit transaction"));
    assert!(!has(&h, "Add receipt…"));
}

#[test]
fn new_transaction_receipts_attach_on_save_and_reimbursement_dialog() {
    let (mut h, _b) = book();
    open_register(&mut h, "Assets:Checking");
    open_new(&mut h);
    drop_file(&mut h, "fuel.jpg", jpeg(70));
    assert!(has(&h, "Receipt thumbnail fuel.jpg"));
    fill_new(
        &mut h,
        &[
            ("payee", "Fuel Stop"),
            ("category", "Expenses:Groceries"),
            ("spent", "30.00"),
        ],
    );
    click(&mut h, "Save");
    let t = txns(&mut h);
    let on = cmd(&mut h, "receipt.list", json!({"transaction": t[0]["id"]}));
    assert_eq!(on.as_array().unwrap().len(), 1);
    // Record reimbursement for a selected personal-funds expense.
    let e = cmd(
        &mut h,
        "expense.create",
        json!({"date": "2026-10-09", "merchant": "Diner", "amount": "9.00",
               "category": "Expenses:Office Supplies", "paid_with": "personal"}),
    );
    cmd(&mut h, "expense.post", json!({"expense": e["id"]}));
    open_feature(&mut h, "Expenses");
    click(&mut h, "Select Diner 2026-10-09");
    assert!(!disabled(&h, "Record reimbursement…"));
    click(&mut h, "Record reimbursement…");
    choose(&mut h, "Reimburse from", "Assets:Checking");
    click(&mut h, "Record");
    assert!(has_contains(&h, "Recorded a reimbursement of 9.00."));
    let bal = cmd(
        &mut h,
        "account.balance",
        json!({"account": "Reimbursements Payable"}),
    );
    assert_eq!(bal["display"]["USD"], "0.00");
}
