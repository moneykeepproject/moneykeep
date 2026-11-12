//! app-desktop.md revision 10 acceptance tests (70–74): investment screens, on the
//! investments-core.md test books.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_views::testing::Key;
use serde_json::json;

type H = mk_views::testing::Harness;

/// The common setup (VTI, brokerage funded, lots A and B); with `sale`, test 1's
/// sale as well.
fn book(sale: bool) -> (H, TestBook) {
    let (mut h, b) = harness_with_chart();
    cmd(&mut h, "module.enable", json!({"module": "investments"}));
    cmd(
        &mut h,
        "security.create",
        json!({"symbol": "VTI", "name": "Total Stock Market ETF", "kind": "etf"}),
    );
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    cmd(
        &mut h,
        "txn.create",
        json!({"date": "2025-03-01", "splits": [
            {"account": "Assets:Brokerage", "value": "5000.00"},
            {"account": "Assets:Checking", "value": "-5000.00"}]}),
    );
    for (d, q, p, c) in [
        ("2025-03-10", "10", "200.00", "5.00"),
        ("2025-11-20", "5", "220.00", "0"),
    ] {
        cmd(
            &mut h,
            "inv.buy",
            json!({"account": "Assets:Brokerage", "security": "VTI", "date": d,
                   "quantity": q, "price": p, "commission": c}),
        );
    }
    if sale {
        cmd(
            &mut h,
            "inv.sell",
            json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-04-15",
                   "quantity": "12", "price": "250.00", "commission": "5.00"}),
        );
    }
    h.state_mut().set_today("2026-10-19");
    h.run();
    (h, b)
}

fn test5() -> (H, TestBook) {
    let (mut h, b) = book(true);
    cmd(
        &mut h,
        "inv.split",
        json!({"security": "VTI", "date": "2026-06-01", "ratio_new": "2", "ratio_old": "1"}),
    );
    cmd(
        &mut h,
        "inv.dividend",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-06-30", "amount": "15.00"}),
    );
    cmd(
        &mut h,
        "inv.reinvest",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-09-30",
               "amount": "22.00", "kind": "qualified", "quantity": "0.1", "price": "220.00"}),
    );
    cmd(
        &mut h,
        "price.set",
        json!({"security": "VTI", "date": "2026-10-16", "price": "125.00"}),
    );
    h.run();
    (h, b)
}

fn replace(h: &mut H, label: &str, text: &str) {
    h.get_by_label(label).focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label(label).type_text(text);
    h.run();
    h.run();
}

#[test]
fn app_desktop_at70_holdings_tab_and_sidebar() {
    let (mut h, _b) = test5();
    open_register(&mut h, "Assets:Brokerage");
    assert!(has(&h, "Holdings") && has(&h, "Activity"));
    for text in [
        "VTI  Total Stock Market ETF",
        "6.1",
        "125.00",
        "762.50",
        "682.00",
        "80.50",
        "Cash",
        "4,905.00",
    ] {
        assert!(
            h.query_all_by_label(text).count() >= 1,
            "{text}: {:?}",
            labels_containing(&h, "VTI")
        );
    }
    // The accounts pane (revision 23) lists the brokerage account at market value.
    open_pane(&mut h, "All accounts");
    assert!(
        has(&h, "Investments  5,667.50"),
        "{:?}",
        labels_containing(&h, "Investments")
    );
    assert!(has(&h, "Account Assets:Brokerage"));
    // Security accounts are not listed in the account list.
    assert!(!has(&h, "Account Assets:Brokerage:VTI"));
    open_pane(&mut h, "All accounts");
    assert!(!has(&h, "Account Assets:Brokerage"), "the pane closed");
    // Clicking a holding opens its lots.
    h.get_by_label_contains("Holding VTI").click();
    h.run();
    assert!(has(&h, "Lots of VTI"));
    assert!(h.query_all_by_label("660.00").count() >= 1);
    click(&mut h, "Close");
    // Activity lists the actions with a running cash balance.
    click(&mut h, "Activity");
    assert!(has_contains(&h, "Activity 2026-04-15 Sell VTI -12"));
    assert!(has_contains(&h, "Activity 2026-06-01 Split VTI 3"));
}

#[test]
fn app_desktop_at71_sell_form_lot_preview_and_choose_lots() {
    let (mut h, _b) = book(false);
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "Sell");
    replace(&mut h, "Date field", "2026-04-15");
    type_into(&mut h, "Shares field", "12");
    type_into(&mut h, "Price field", "250.00");
    type_into(&mut h, "Commission field", "5.00");
    h.run();
    assert!(has(&h, "Method: First in, first out"));
    assert!(
        has(&h, "Long-term gain 490.83"),
        "{:?}",
        labels_containing(&h, "gain")
    );
    assert!(has(&h, "Short-term gain 59.17"));
    assert_eq!(value(&h, "Total field"), "2995.00");
    click(&mut h, "Choose lots…");
    h.run();
    type_into(&mut h, "Sell from lot 2025-11-20", "5");
    type_into(&mut h, "Sell from lot 2025-03-10", "7");
    h.run();
    assert!(
        has(&h, "Long-term gain 343.58"),
        "{:?}",
        labels_containing(&h, "gain")
    );
    assert!(has(&h, "Short-term gain 147.92"));
    assert!(has(
        &h,
        "MoneyKeep keeps records; it does not give tax advice."
    ));
    click(&mut h, "Save");
    assert!(!has(&h, "Sell from lot 2025-03-10"), "the form closed");
    let gains = cmd(
        &mut h,
        "report.realized_gains",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    assert_eq!(gains["total"]["gain"], "491.50");
}

#[test]
fn app_desktop_at72_update_prices_refreshes_holdings() {
    let (mut h, _b) = test5();
    click(&mut h, "Accounts");
    click(&mut h, "Securities");
    assert!(has(&h, "Security VTI Total Stock Market ETF"));
    click(&mut h, "Update prices…");
    assert_eq!(value(&h, "Price date VTI"), "2026-10-19");
    type_into(&mut h, "Price VTI", "130.00");
    click(&mut h, "Save prices");
    assert!(!has(&h, "Price VTI"), "the grid closed");
    open_register(&mut h, "Assets:Brokerage");
    assert!(h.query_all_by_label("793.00").count() >= 1, "6.1 × 130.00");
    open_pane(&mut h, "Investments");
    assert!(has(&h, "Investments  5,698.00"));
}

#[test]
fn app_desktop_at73_realized_gains_report() {
    let (mut h, _b) = book(true);
    click(&mut h, "Reports");
    click(&mut h, "Realized gains");
    click(&mut h, "Period");
    click(&mut h, "This year");
    for text in [
        "Short-term",
        "Long-term",
        "2,495.83",
        "499.17",
        "Short-term total: proceeds 499.17, cost basis 440.00, gain 59.17",
        "Long-term total: proceeds 2,495.83, cost basis 2,005.00, gain 490.83",
        "Total gain 550.00",
        "MoneyKeep keeps records; it does not give tax advice.",
    ] {
        assert!(has(&h, text), "{text}");
    }
    for r in ["Holdings", "Investment income"] {
        assert!(has(&h, r), "{r} listed");
    }
}

#[test]
fn app_desktop_at74_module_off_hides_investment_screens() {
    let (mut h, _b) = book(true);
    cmd(&mut h, "module.disable", json!({"module": "investments"}));
    h.run();
    open_register(&mut h, "Assets:Brokerage");
    assert!(!has(&h, "Holdings"));
    assert!(!has(&h, "Buy"));
    assert!(has(&h, "+ New"), "the plain register");
    click(&mut h, "Accounts");
    assert!(!has(&h, "Securities"));
    click(&mut h, "Reports");
    for r in ["Holdings", "Realized gains", "Investment income"] {
        assert!(!has(&h, r), "{r} not listed");
    }
    h.state_mut().open_view_spec("securities");
    h.run();
    assert!(!has(&h, "Update prices…"));
}

#[test]
fn app_desktop_r10_activity_row_opens_prefilled_form_saved_with_update() {
    let (mut h, _b) = book(false);
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "Activity");
    h.get_by_label_contains("Activity 2025-11-20 Buy VTI 5")
        .click();
    h.run();
    assert!(has(&h, "Edit — Buy shares"));
    assert_eq!(value(&h, "Shares field"), "5");
    assert_eq!(value(&h, "Price field"), "220.00");
    replace(&mut h, "Price field", "210.00");
    h.run();
    assert_eq!(
        value(&h, "Total field"),
        "1050.00",
        "live total from the command"
    );
    click(&mut h, "Save");
    assert!(!has(&h, "Edit — Buy shares"));
    let lots = cmd(
        &mut h,
        "inv.lots",
        json!({"account": "Assets:Brokerage", "security": "VTI", "as_of": "2026-01-01"}),
    );
    assert_eq!(lots["lots"][1]["basis"], "1050.00");
    // Void from the form.
    h.get_by_label_contains("Activity 2025-11-20 Buy VTI 5")
        .click();
    h.run();
    click(&mut h, "Void");
    assert_eq!(
        cmd(
            &mut h,
            "inv.lots",
            json!({"account": "Assets:Brokerage", "security": "VTI", "as_of": "2026-01-01"})
        )["lots"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn app_desktop_at75_sell_form_offers_held_securities_and_stays_put() {
    let (mut h, _b) = test5();
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "Sell");
    assert!(has(&h, "Sell shares"));
    for bad in ["invalid", "give ", "Enter ", "Choose a security"] {
        assert!(
            labels_containing(&h, bad).is_empty(),
            "no error text on opening: {:?}",
            labels_containing(&h, bad)
        );
    }
    assert_eq!(value(&h, "Security field"), "VTI — 6.1 held", "preselected");
    assert!(!has(&h, "New security…"));
    type_into(&mut h, "Shares field", "1");
    assert!(
        labels_containing(&h, "Enter ").is_empty(),
        "price not yet filled: no error"
    );
    let before = h.get_by_label("Shares field").rect();
    assert!(!has(&h, "Short-term gain 0.00") && labels_containing(&h, "gain ").is_empty());
    type_into(&mut h, "Price field", "130.00");
    h.run();
    assert!(
        !labels_containing(&h, "Long-term gain").is_empty(),
        "the lot preview appeared"
    );
    let after = h.get_by_label("Shares field").rect();
    assert!(
        (before.min.y - after.min.y).abs() < 0.5 && (before.min.x - after.min.x).abs() < 0.5,
        "Shares field stayed put: {before:?} → {after:?}"
    );
    // Too many shares: plain words next to the field.
    h.get_by_label("Shares field").focus();
    h.run();
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::A);
    h.get_by_label("Shares field").type_text("10");
    h.run();
    h.run();
    assert!(
        has(
            &h,
            "You hold 6.1 shares of VTI in this account on that date"
        ),
        "{:?}",
        labels_containing(&h, "hold")
    );
}

#[test]
fn app_desktop_at76_no_hint_mentions_action() {
    let (mut h, _b) = book(false);
    open_register(&mut h, "Assets:Brokerage");
    for (label, title) in [
        ("Buy", "Buy shares"),
        ("Sell", "Sell shares"),
        ("Dividend", "Record a dividend"),
        ("Reinvest", "Reinvest a dividend"),
    ] {
        click(&mut h, label);
        assert!(has(&h, title), "{title}");
        assert!(
            labels_containing(&h, "Action").is_empty(),
            "{label}: {:?}",
            labels_containing(&h, "Action")
        );
        click(&mut h, "Cancel");
    }
    click(&mut h, "More…");
    for (label, title) in [
        ("Interest", "Record interest"),
        ("Fee", "Record a fee"),
        ("Shares in", "Move shares in"),
        ("Shares out", "Move shares out"),
        ("Split", "Record a stock split"),
        ("Return of capital", "Record a return of capital"),
    ] {
        click(&mut h, label);
        assert!(has(&h, title), "{title}");
        assert!(labels_containing(&h, "Action").is_empty(), "{label}");
        click(&mut h, "Cancel");
        click(&mut h, "More…");
    }
}

#[test]
fn app_desktop_at77_single_held_security_is_preselected() {
    let (mut h, _b) = test5();
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "Sell");
    type_into(&mut h, "Shares field", "2");
    type_into(&mut h, "Price field", "130.00");
    h.run();
    assert!(
        !labels_containing(&h, "Long-term gain").is_empty(),
        "the preview ran without touching Security"
    );
    // The lot table's numbers stand in their own columns.
    let shares = h.get_by_label("2").rect();
    let basis = h.get_by_label("220.00").rect();
    assert!(
        basis.min.x - shares.max.x > 10.0,
        "columns apart: {shares:?} {basis:?}"
    );
}

#[test]
fn app_desktop_at78_two_held_securities_need_a_choice() {
    let (mut h, _b) = test5();
    cmd(
        &mut h,
        "security.create",
        json!({"symbol": "AAPL", "name": "Apple", "kind": "stock"}),
    );
    cmd(
        &mut h,
        "inv.buy",
        json!({"account": "Assets:Brokerage", "security": "AAPL", "date": "2026-01-05",
               "quantity": "3", "price": "100.00"}),
    );
    h.run();
    open_register(&mut h, "Assets:Brokerage");
    click(&mut h, "Sell");
    assert_eq!(value(&h, "Security field"), "Choose a security");
    type_into(&mut h, "Shares field", "1");
    type_into(&mut h, "Price field", "130.00");
    assert!(
        labels_containing(&h, "Long-term gain").is_empty(),
        "no preview yet"
    );
    click(&mut h, "Security field");
    click(&mut h, "AAPL — 3 held");
    h.run();
    assert_eq!(value(&h, "Security field"), "AAPL — 3 held");
    assert!(
        !labels_containing(&h, "Short-term gain").is_empty(),
        "preview after choosing"
    );
}
