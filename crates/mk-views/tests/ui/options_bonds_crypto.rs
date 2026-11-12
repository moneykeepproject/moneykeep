//! investments-options-bonds-crypto.md in the desktop window: the brokerage view's
//! Option…, Bond… and Crypto… forms (plain questions, preview of the transaction and
//! gain), the expired-option dialog from Home → Attention and the holdings Details.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

fn book() -> (H, TestBook) {
    let (mut h, b) = harness_with_chart();
    h.state_mut().set_today("2026-11-06");
    cmd(&mut h, "module.enable", json!({"module": "investments"}));
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2026-01-02", "splits": [
            {"account": "Assets:Brokerage", "value": "10000.00"},
            {"account": "Assets:Checking", "value": "-10000.00"}]}),
    );
    cmd(
        &mut h,
        "security.create",
        json!({"kind": "option", "underlying": "XYZ", "right": "put", "strike": "50",
               "expiration": "2026-12-18"}),
    );
    h.run();
    (h, b)
}

/// The Option… form writes a put in plain words, with the preview before saving.
#[test]
fn option_form_writes_a_put_with_preview() {
    let (mut h, _b) = book();
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "More…");
    click(&mut h, "Option…");
    click(&mut h, "What happened");
    click(&mut h, "I sold contracts");
    assert!(has(&h, "Opening: I wrote them (sell to open)"));
    click(&mut h, "Opening: I wrote them (sell to open)");
    click(&mut h, "Option field");
    click(&mut h, "XYZ Dec 18 2026 50 Put");
    type_into(&mut h, "Contracts field", "1");
    type_into(&mut h, "Price per share field", "2.00");
    type_into(&mut h, "Commission field", "0.65");
    h.run();
    assert!(
        has(&h, "This will record"),
        "{:?}",
        labels_containing(&h, "")
    );
    assert!(has_contains(&h, "Preview: Assets:Brokerage 199.35"));
    assert!(has(&h, "-199.35"));
    h.get_by_role_and_label(Role::Button, "Save").click();
    h.run_steps(2);
    let lots = cmd(
        &mut h,
        "inv.lots",
        json!({"account": "Assets:Brokerage", "security": "XYZ 261218 P50",
               "as_of": "2026-11-06"}),
    );
    assert_eq!(lots["lots"][0]["quantity"], "-1");
    assert_eq!(lots["lots"][0]["premium"], "199.35");
    // Holdings show the terms, a written position and its value as a liability.
    h.run_steps(2);
    assert!(has(&h, "Details"), "{:?}", labels_containing(&h, ""));
    assert!(has_contains(&h, "42 days left · written"));
}

/// Home → Attention: an open option past its expiration opens the dialog; Expired
/// worthless records `inv.option_expire` (OP7).
#[test]
fn expired_option_dialog_from_attention() {
    let (mut h, _b) = book();
    cmd(
        &mut h,
        "security.create",
        json!({"kind": "option", "underlying": "XYZ", "right": "call", "strike": "60",
               "expiration": "2026-10-16"}),
    );
    cmd(
        &mut h,
        "inv.option_buy",
        json!({"account": "Assets:Brokerage", "security": "XYZ 261016 C60",
               "date": "2026-03-02", "quantity": "1", "price": "1.50"}),
    );
    h.state_mut().open_view_spec("home");
    h.run_steps(3);
    let label = "XYZ Oct 16 2026 60 Call expired — record what happened";
    assert!(has_contains(&h, label), "{:?}", labels_containing(&h, ""));
    click(&mut h, label);
    h.run_steps(2);
    assert!(has(&h, "An option has expired"));
    assert!(has(&h, "Expired worthless"));
    assert!(has(&h, "Closed earlier (sold or bought back)"));
    assert!(disabled(&h, "Assigned"), "a held option is not assigned");
    assert!(
        has_contains(&h, "loss 150.00"),
        "{:?}",
        labels_containing(&h, "")
    );
    click(&mut h, "Record");
    h.run_steps(2);
    let bal = cmd(
        &mut h,
        "account.balance",
        json!({"account": "Assets:Brokerage:XYZ 261016 C60"}),
    );
    assert_eq!(bal["value"], "0.00");
    h.run_steps(2);
    assert!(!has_contains(&h, label));
}

/// The Bond… form buys with accrued interest and adds coupon reminders.
#[test]
fn bond_form_buys_with_accrued_interest() {
    let (mut h, _b) = book();
    cmd(
        &mut h,
        "security.create",
        json!({"symbol": "T27", "name": "Treasury 4.25% 2027", "kind": "bond",
               "coupon_rate": "4.25", "maturity": "2027-08-15"}),
    );
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "More…");
    click(&mut h, "Bond…");
    click(&mut h, "Bond field");
    click(&mut h, "Treasury 4.25% 2027");
    type_into(&mut h, "Bonds field", "5");
    type_into(&mut h, "Price (% of face) field", "98.50");
    type_into(&mut h, "Accrued interest paid field", "42.10");
    type_into(&mut h, "Commission field", "5.00");
    h.run();
    assert!(has(&h, "-4,972.10"), "{:?}", labels_containing(&h, ""));
    assert!(has(&h, "4,930.00"));
    assert!(has(&h, "42.10"));
    h.get_by_role_and_label(Role::Button, "Save").click();
    h.run_steps(2);
    assert!(
        has_contains(&h, "with 2 coupon reminders"),
        "{:?}",
        labels_containing(&h, "")
    );
    let up = cmd(
        &mut h,
        "schedule.upcoming",
        json!({"today": "2026-11-06", "to": "2028-01-01"}),
    );
    assert_eq!(up["occurrences"].as_array().unwrap().len(), 2);
}

// ----- revision 2: editing from the Activity tab; investment income rows -------------

fn replace(h: &mut H, label: &str, text: &str) {
    h.get_by_label(label).focus();
    h.run();
    h.key_press_modifiers(
        mk_views::testing::Modifiers::COMMAND,
        mk_views::testing::Key::A,
    );
    h.get_by_label(label).type_text(text);
    h.run();
    h.run();
}

fn cash(h: &mut H) -> String {
    cmd(h, "account.balance", json!({"account": "Assets:Brokerage"}))["display"]["USD"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn checked(h: &H, label: &str) -> bool {
    h.query_all_by_label(label)
        .find_map(|n| n.accesskit_node().toggled())
        .is_some_and(|t| t == mk_views::testing::Toggled::True)
}

/// OP8. A written put opens from Activity in the Option… form titled "Edit — Option"
/// (Sold contracts / Opening, 1 contract at 2.00, commission 0.65); a price of 2.50
/// previews and saves cash +249.35 through `inv.update`; Undo restores 199.35.
#[test]
fn op8_written_put_opens_from_activity_and_saves_with_update() {
    let (mut h, _b) = book();
    cmd(
        &mut h,
        "inv.option_write",
        json!({"account": "Assets:Brokerage", "security": "XYZ 261218 P50",
               "date": "2026-09-01", "quantity": "1", "price": "2.00", "commission": "0.65"}),
    );
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "Activity");
    h.get_by_label_contains("Sell to open XYZ 261218 P50")
        .click();
    h.run_steps(2);
    assert!(
        has(&h, "Edit — Option"),
        "{:?}",
        labels_containing(&h, "Edit")
    );
    assert_eq!(value(&h, "What happened"), "I sold contracts");
    assert!(checked(&h, "Opening: I wrote them (sell to open)"));
    assert_eq!(value(&h, "Contracts field"), "1");
    assert_eq!(value(&h, "Price per share field"), "2.00");
    assert_eq!(value(&h, "Commission field"), "0.65");
    assert!(!has(&h, "Add coupon reminders"));
    replace(&mut h, "Price per share field", "2.50");
    h.run();
    assert!(
        has_contains(&h, "Preview: Assets:Brokerage 249.35"),
        "{:?}",
        labels_containing(&h, "Preview")
    );
    h.get_by_role_and_label(Role::Button, "Save").click();
    h.run_steps(2);
    assert!(!has(&h, "Edit — Option"), "the form closed");
    assert_eq!(cash(&mut h), "10249.35");
    let count = cmd(&mut h, "txn.list", json!({}))
        .as_array()
        .map_or(0, Vec::len);
    h.get_by_role_and_label(Role::Button, "Undo").click();
    h.run_steps(2);
    assert_eq!(cash(&mut h), "10199.35");
    assert_eq!(
        cmd(&mut h, "txn.list", json!({}))
            .as_array()
            .map_or(0, Vec::len),
        count,
        "one undoable replace"
    );
}

/// Revision 2: an assigned option opens the exercise/assignment form; changing
/// Opening to Closing in the Option… form saves as a different action.
#[test]
fn assignment_and_action_change_from_activity() {
    let (mut h, _b) = book();
    let w = cmd(
        &mut h,
        "inv.option_write",
        json!({"account": "Assets:Brokerage", "security": "XYZ 261218 P50",
               "date": "2026-09-01", "quantity": "1", "price": "2.00", "commission": "0.65"}),
    );
    cmd(
        &mut h,
        "inv.option_assigned",
        json!({"account": "Assets:Brokerage", "security": "XYZ 261218 P50",
               "date": "2026-10-01"}),
    );
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "Activity");
    h.get_by_label_contains("Option assigned XYZ 261218 P50")
        .click();
    h.run_steps(2);
    assert!(has(&h, "Edit — Option"));
    assert_eq!(value(&h, "What happened"), "They were assigned to me");
    assert!(
        has_contains(&h, "Preview:"),
        "{:?}",
        labels_containing(&h, "")
    );
    click(&mut h, "Cancel");
    // Delete the assignment from its form, then change the write to a buy to open.
    h.get_by_label_contains("Option assigned XYZ 261218 P50")
        .click();
    h.run_steps(2);
    click(&mut h, "Delete…");
    click(&mut h, "Delete transaction");
    h.run_steps(2);
    h.get_by_label_contains("Sell to open XYZ 261218 P50")
        .click();
    h.run_steps(2);
    click(&mut h, "What happened");
    click(&mut h, "I bought contracts");
    click(&mut h, "Opening (buy to open)");
    h.run();
    assert!(
        has_contains(&h, "; Assets:Brokerage -200.65"),
        "{:?}",
        labels_containing(&h, "Preview")
    );
    h.get_by_role_and_label(Role::Button, "Save").click();
    h.run_steps(2);
    let txn = w["transaction"]["id"].as_str().unwrap();
    assert_eq!(
        cmd(&mut h, "inv.get", json!({"txn": txn}))["action"],
        "option_buy"
    );
    assert_eq!(cash(&mut h), "9799.35");
}

/// BD5 in the window: the bond buy opens in the Bond… form; accrued interest 40.00
/// saves and the interest account shows −40.00.
#[test]
fn bd5_bond_buy_opens_from_activity() {
    let (mut h, _b) = book();
    cmd(
        &mut h,
        "security.create",
        json!({"symbol": "T27", "name": "Treasury 4.25% 2027", "kind": "bond",
               "coupon_rate": "4.25", "maturity": "2027-08-15"}),
    );
    cmd(
        &mut h,
        "inv.bond_buy",
        json!({"account": "Assets:Brokerage", "security": "T27", "date": "2026-03-02",
               "units": "5", "price": "98.50", "accrued_interest": "42.10", "commission": "5.00"}),
    );
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "Activity");
    h.get_by_label_contains("Buy bond T27").click();
    h.run_steps(2);
    assert!(has(&h, "Edit — Bond"));
    assert_eq!(value(&h, "Bonds field"), "5");
    assert_eq!(value(&h, "Accrued interest paid field"), "42.10");
    replace(&mut h, "Accrued interest paid field", "40.00");
    h.run();
    h.get_by_role_and_label(Role::Button, "Save").click();
    h.run_steps(2);
    let i = cmd(&mut h, "account.balance", json!({"account": "Interest"}));
    assert_eq!(i["display"]["USD"], "-40.00", "{i}");
}

/// A coin bought with the core action opens in the Crypto… form.
#[test]
fn crypto_buy_opens_in_the_crypto_form() {
    let (mut h, _b) = book();
    cmd(
        &mut h,
        "security.create",
        json!({"symbol": "BTC", "name": "Bitcoin", "kind": "crypto"}),
    );
    cmd(
        &mut h,
        "inv.buy",
        json!({"account": "Assets:Brokerage", "security": "BTC", "date": "2026-01-10",
               "quantity": "0.12345678", "price": "64800"}),
    );
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "Activity");
    h.get_by_label_contains("Buy BTC").click();
    h.run_steps(2);
    assert!(
        has(&h, "Edit — Crypto"),
        "{:?}",
        labels_containing(&h, "Edit")
    );
    assert_eq!(value(&h, "What happened"), "I bought coins");
    assert_eq!(value(&h, "Quantity field"), "0.12345678");
    assert_eq!(value(&h, "Total paid field"), "8000.00");
    assert!(has_contains(&h, "Preview:"));
}

/// A transaction the form cannot show as it is (a put exercised against chosen lots)
/// opens the register's split view with the note.
#[test]
fn unrepresentable_transaction_opens_as_splits() {
    let (mut h, _b) = book();
    for (d, p) in [("2026-02-02", "40.00"), ("2026-03-02", "60.00")] {
        cmd(
            &mut h,
            "inv.buy",
            json!({"account": "Assets:Brokerage", "security": "XYZ", "date": d,
                   "quantity": "100", "price": p}),
        );
    }
    cmd(
        &mut h,
        "inv.option_buy",
        json!({"account": "Assets:Brokerage", "security": "XYZ 261218 P50",
               "date": "2026-04-01", "quantity": "1", "price": "2.00"}),
    );
    let lots = cmd(
        &mut h,
        "inv.lots",
        json!({"account": "Assets:Brokerage", "security": "XYZ", "as_of": "2026-05-01"}),
    );
    let second = lots["lots"][1]["lot"].clone();
    cmd(
        &mut h,
        "inv.option_exercise",
        json!({"account": "Assets:Brokerage", "security": "XYZ 261218 P50",
               "date": "2026-05-01", "underlying_lots": [{"lot": second, "quantity": "100"}]}),
    );
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "Activity");
    h.get_by_label_contains("Option exercised XYZ 261218 P50")
        .click();
    h.run_steps(3);
    assert!(!has(&h, "Edit — Option"));
    assert!(
        has(
            &h,
            "This transaction was changed by hand, so it opens as splits."
        ),
        "{:?}",
        labels_containing(&h, "")
    );
    assert!(
        has(&h, "Split transaction"),
        "{:?}",
        labels_containing(&h, "plit")
    );
}

/// CR7 in the window: the investment income report shows Crypto rewards and the
/// interest rows that are not zero.
#[test]
fn cr7_investment_income_report_rows() {
    let (mut h, _b) = book();
    cmd(
        &mut h,
        "security.create",
        json!({"symbol": "ETH", "name": "Ether", "kind": "crypto"}),
    );
    cmd(
        &mut h,
        "security.create",
        json!({"symbol": "M30", "name": "City 3% 2030", "kind": "bond", "coupon_rate": "3",
               "maturity": "2030-06-01", "tax": "municipal"}),
    );
    cmd(
        &mut h,
        "inv.crypto_reward",
        json!({"account": "Assets:Brokerage", "security": "ETH", "date": "2026-02-01",
               "quantity": "0.1", "value": "300.00"}),
    );
    cmd(
        &mut h,
        "inv.bond_interest",
        json!({"account": "Assets:Brokerage", "security": "M30", "date": "2026-06-01",
               "amount": "45.00"}),
    );
    h.state_mut()
        .open_obc_review("report|investment_income|2026-01-01|2026-12-31");
    h.run_steps(3);
    assert!(has(&h, "Crypto rewards"), "{:?}", labels_containing(&h, ""));
    assert!(has(&h, "Tax-exempt interest"));
    assert!(!has(&h, "US Treasury interest"), "zero rows are left out");
    assert!(has(&h, "345.00"), "{:?}", labels_containing(&h, "345"));
}
