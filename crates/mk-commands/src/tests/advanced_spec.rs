//! investments-advanced.md acceptance tests 1–13.

use super::*;

fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "investments"}));
    b.account(
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    b
}

fn security(b: &mut Book, symbol: &str) {
    b.run(
        "security.create",
        json!({"symbol": symbol, "name": format!("{symbol} Inc"), "kind": "stock"}),
    );
}

fn fund(b: &mut Book, date: &str, amount: &str) {
    b.transfer(date, "Assets:Brokerage", "Assets:Checking", amount);
}

fn buy(b: &mut Book, sym: &str, date: &str, q: &str, p: &str) -> Value {
    b.run(
        "inv.buy",
        json!({"account": "Assets:Brokerage", "security": sym, "date": date, "quantity": q, "price": p}),
    )
}

fn sell(b: &mut Book, sym: &str, date: &str, q: &str, p: &str) -> Value {
    b.run(
        "inv.sell",
        json!({"account": "Assets:Brokerage", "security": sym, "date": date, "quantity": q, "price": p}),
    )
}

/// (acquired, quantity, basis) of each open lot.
fn lots(b: &mut Book, sym: &str) -> Vec<(String, String, String)> {
    b.run(
        "inv.lots",
        json!({"account": "Assets:Brokerage", "security": sym, "as_of": "2027-12-31"}),
    )["lots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["acquired"].as_str().unwrap().to_owned(),
                l["quantity"].as_str().unwrap().to_owned(),
                l["basis"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn gains(b: &mut Book, from: &str, to: &str) -> Value {
    b.run("report.realized_gains", json!({"from": from, "to": to}))
}

fn all_rows(g: &Value) -> Vec<Value> {
    g["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["rows"].as_array().unwrap().clone())
        .collect()
}

fn l(a: &str, q: &str, basis: &str) -> (String, String, String) {
    (a.to_owned(), q.to_owned(), basis.to_owned())
}

// ----- wash sales -----------------------------------------------------------------------

fn wash_book(second_buy: &str, second_sym: &str) -> Book {
    let mut b = book();
    security(&mut b, "XYZ");
    if second_sym != "XYZ" {
        security(&mut b, second_sym);
    }
    fund(&mut b, "2026-01-02", "5000.00");
    buy(&mut b, "XYZ", "2026-01-05", "10", "100.00");
    sell(&mut b, "XYZ", "2026-03-02", "10", "80.00");
    buy(&mut b, second_sym, second_buy, "6", "82.00");
    b
}

#[test]
fn advanced_at5_wash_sale_flag_and_window() {
    let mut b = wash_book("2026-03-20", "XYZ");
    let g = gains(&mut b, "2026-01-01", "2026-12-31");
    let rows = all_rows(&g);
    assert_eq!(rows.len(), 1, "{g}");
    let r = &rows[0];
    assert_eq!(r["gain"], "-200.00");
    assert_eq!(r["adjustment_code"], "W");
    assert_eq!(r["adjustment"], "120.00");
    assert_eq!(r["wash_sale"], true);
    assert_eq!(r["replacements"][0]["date"], "2026-03-20");
    assert_eq!(r["replacements"][0]["quantity"], "6");
    assert_eq!(g["total"]["gain"], "-200.00");
    assert_eq!(g["total"]["disallowed"], "120.00");
    // The window is 30 days after the sale, inclusive.
    let mut b = wash_book("2026-04-01", "XYZ");
    assert_eq!(
        all_rows(&gains(&mut b, "2026-01-01", "2026-12-31"))[0]["adjustment"],
        "120.00"
    );
    let mut b = wash_book("2026-04-02", "XYZ");
    let r = &all_rows(&gains(&mut b, "2026-01-01", "2026-12-31"))[0];
    assert_eq!(r["wash_sale"], false);
    assert!(r["adjustment_code"].is_null());
    // Bases are not changed in 3c.
    let mut b = wash_book("2026-03-20", "XYZ");
    assert_eq!(lots(&mut b, "XYZ"), vec![l("2026-03-20", "6", "492.00")]);
}

#[test]
fn advanced_at6_wash_group() {
    let mut b = wash_book("2026-03-20", "XYZB");
    assert_eq!(
        all_rows(&gains(&mut b, "2026-01-01", "2026-12-31"))[0]["wash_sale"],
        false
    );
    b.run(
        "security.link_wash_group",
        json!({"securities": ["XYZ", "XYZB"], "name": "XYZ classes"}),
    );
    let r = &all_rows(&gains(&mut b, "2026-01-01", "2026-12-31"))[0];
    assert_eq!(r["adjustment"], "120.00");
    assert_eq!(r["replacements"][0]["symbol"], "XYZB");
    let groups = b.run("security.wash_groups", json!({}));
    assert_eq!(groups[0]["members"].as_array().unwrap().len(), 2);
}

#[test]
fn advanced_wash_check_lists_recent_buys() {
    let mut b = book();
    security(&mut b, "XYZ");
    fund(&mut b, "2026-01-02", "5000.00");
    buy(&mut b, "XYZ", "2026-02-20", "5", "100.00");
    let w = b.run(
        "inv.wash_check",
        json!({"security": "XYZ", "date": "2026-03-02", "quantity": "5"}),
    );
    assert_eq!(w["recent"][0]["date"], "2026-02-20");
    assert!(w["warning"].as_str().unwrap().contains("2026-04-01"));
}

// ----- corporate actions ----------------------------------------------------------------

fn parent_book() -> Book {
    let mut b = book();
    security(&mut b, "PAR");
    security(&mut b, "NEW");
    fund(&mut b, "2024-04-01", "5000.00");
    buy(&mut b, "PAR", "2024-05-01", "100", "50.00");
    b
}

#[test]
fn advanced_at7_spinoff_and_at13_undo() {
    let mut b = parent_book();
    b.run(
        "inv.spinoff",
        json!({"parent": "PAR", "new_security": "NEW", "date": "2026-06-01",
               "new_per_parent": "0.5", "allocation_percent": "12.5"}),
    );
    assert_eq!(lots(&mut b, "NEW"), vec![l("2024-05-01", "50", "625.00")]);
    assert_eq!(lots(&mut b, "PAR"), vec![l("2024-05-01", "100", "4375.00")]);
    assert_eq!(b.shown("Assets:Brokerage:NEW", "NEW"), "50");
    // Out of range percentages are rejected.
    for pct in ["0", "100", "120"] {
        b.err(
            "inv.spinoff",
            json!({"parent": "PAR", "new_security": "NEW", "date": "2026-07-01",
                   "new_per_parent": "0.5", "allocation_percent": pct}),
        );
    }
    b.run("history.undo", json!({}));
    assert_eq!(lots(&mut b, "PAR"), vec![l("2024-05-01", "100", "5000.00")]);
    assert!(lots(&mut b, "NEW").is_empty());
}

#[test]
fn advanced_spinoff_cash_in_lieu() {
    let mut b = parent_book();
    b.run(
        "inv.spinoff",
        json!({"parent": "PAR", "new_security": "NEW", "date": "2026-06-01",
               "new_per_parent": "0.335", "allocation_percent": "10",
               "cash_in_lieu": {"quantity": "0.5", "amount": "6.00"}}),
    );
    // 33.5 new shares, 0.5 sold for cash: 33 left.
    let new = lots(&mut b, "NEW");
    assert_eq!(new[0].1, "33");
    let rows = all_rows(&gains(&mut b, "2026-01-01", "2026-12-31"));
    assert_eq!(rows[0]["quantity"], "0.5");
    assert_eq!(rows[0]["proceeds"], "6.00");
}

fn merger_book(basis_price: &str) -> Book {
    let mut b = book();
    security(&mut b, "OLD");
    security(&mut b, "ACQ");
    fund(&mut b, "2024-01-02", "10000.00");
    buy(&mut b, "OLD", "2024-02-01", "100", basis_price);
    b
}

#[test]
fn advanced_at8_tax_free_merger() {
    let mut b = merger_book("30.00");
    b.run(
        "inv.merger",
        json!({"old_security": "OLD", "new_security": "ACQ", "date": "2026-06-01",
               "ratio_new": "1", "ratio_old": "2", "mode": "tax_free"}),
    );
    assert_eq!(lots(&mut b, "ACQ"), vec![l("2024-02-01", "50", "3000.00")]);
    assert!(lots(&mut b, "OLD").is_empty());
    assert!(all_rows(&gains(&mut b, "2026-01-01", "2026-12-31")).is_empty());
}

#[test]
fn advanced_at9_merger_with_boot() {
    let terms = json!({"old_security": "OLD", "new_security": "ACQ", "date": "2026-06-01",
                       "ratio_new": "1", "ratio_old": "2", "cash_per_share": "10.00",
                       "new_share_value": "70.00", "mode": "tax_free"});
    let mut b = merger_book("30.00");
    b.run("inv.merger", terms.clone());
    assert_eq!(lots(&mut b, "ACQ"), vec![l("2024-02-01", "50", "3000.00")]);
    let rows = all_rows(&gains(&mut b, "2026-01-01", "2026-12-31"));
    assert_eq!(rows[0]["gain"], "1000.00");
    assert_eq!(rows[0]["term"], "long");
    assert_eq!(b.shown("Realized Gains", "USD"), "1000.00");
    let mut b = merger_book("50.00");
    b.run("inv.merger", terms);
    assert_eq!(lots(&mut b, "ACQ"), vec![l("2024-02-01", "50", "4000.00")]);
    assert!(all_rows(&gains(&mut b, "2026-01-01", "2026-12-31")).is_empty());
}

#[test]
fn advanced_at10_taxable_merger() {
    let mut b = merger_book("30.00");
    b.run(
        "inv.merger",
        json!({"old_security": "OLD", "new_security": "ACQ", "date": "2026-06-01",
               "ratio_new": "1", "ratio_old": "2", "cash_per_share": "10.00",
               "new_share_value": "70.00", "mode": "taxable"}),
    );
    let rows = all_rows(&gains(&mut b, "2026-01-01", "2026-12-31"));
    assert_eq!(rows[0]["proceeds"], "4500.00");
    assert_eq!(rows[0]["gain"], "1500.00");
    assert_eq!(lots(&mut b, "ACQ"), vec![l("2026-06-01", "50", "3500.00")]);
}

#[test]
fn advanced_at11_return_of_capital_beyond_basis() {
    let mut b = book();
    security(&mut b, "ROC");
    fund(&mut b, "2025-01-02", "1000.00");
    buy(&mut b, "ROC", "2025-01-10", "10", "10.00");
    b.run(
        "inv.return_of_capital",
        json!({"account": "Assets:Brokerage", "security": "ROC", "date": "2025-06-01", "amount": "150.00"}),
    );
    assert_eq!(lots(&mut b, "ROC"), vec![l("2025-01-10", "10", "0.00")]);
    let rows = all_rows(&gains(&mut b, "2025-01-01", "2025-12-31"));
    assert_eq!(rows[0]["gain"], "50.00");
    assert_eq!(rows[0]["term"], "short");
    assert_eq!(b.shown("Realized Gains", "USD"), "50.00");
}

#[test]
fn advanced_at12_symbol_change() {
    let mut b = book();
    security(&mut b, "ABC");
    fund(&mut b, "2026-01-02", "5000.00");
    buy(&mut b, "ABC", "2026-01-10", "10", "100.00");
    sell(&mut b, "ABC", "2026-05-15", "4", "110.00");
    b.run(
        "inv.symbol_change",
        json!({"security": "ABC", "date": "2026-06-01", "new_symbol": "XYZ"}),
    );
    let h = b.run("inv.holdings", json!({"as_of": "2026-06-30"}));
    assert_eq!(h["accounts"][0]["holdings"][0]["symbol"], "XYZ", "{h}");
    let rows = all_rows(&gains(&mut b, "2026-01-01", "2026-12-31"));
    assert_eq!(rows[0]["symbol"], "ABC (now XYZ)");
    // Lookups by the old symbol resolve to the security.
    assert_eq!(
        b.run("security.get", json!({"security": "ABC"}))["symbol"],
        "XYZ"
    );
    let csv = "Date,Action,Symbol,Quantity,Price,Amount\n2026-06-10,Bought,ABC,1,100,-100\n";
    b.run(
        "inv_profile.save",
        json!({"profile": {"name": "Plain broker", "format": "csv",
               "signature": ["Date", "Action", "Symbol", "Quantity", "Price", "Amount"],
               "fields": {"date": "Date", "action": "Action", "symbol": "Symbol",
                          "quantity": "Quantity", "price": "Price", "amount": "Amount"},
               "date_format": "YYYY-MM-DD",
               "actions": [{"match_type": "equals", "text": "Bought", "action": "buy"}]}}),
    );
    let batch = b.run(
        "import.stage",
        json!({"name": "trades.csv", "content": csv, "account": "Assets:Brokerage"}),
    );
    let sec = &batch["sections"][0]["securities"][0];
    assert_eq!(sec["how"], "ticker", "{batch}");
    assert_eq!(sec["symbol"], "XYZ");
}

#[test]
fn advanced_imported_corporate_rows_need_review() {
    let mut b = book();
    security(&mut b, "OLD");
    security(&mut b, "ACQ");
    fund(&mut b, "2024-01-02", "10000.00");
    buy(&mut b, "OLD", "2024-02-01", "100", "30.00");
    let csv = "Date,Action,Symbol,Quantity,Price,Amount\n2026-06-01,Merger,OLD,100,,\n";
    b.run(
        "inv_profile.save",
        json!({"profile": {"name": "Plain broker", "format": "csv",
               "signature": ["Date", "Action", "Symbol", "Quantity", "Price", "Amount"],
               "fields": {"date": "Date", "action": "Action", "symbol": "Symbol",
                          "quantity": "Quantity", "price": "Price", "amount": "Amount"},
               "date_format": "YYYY-MM-DD",
               "actions": [{"match_type": "equals", "text": "Merger", "action": "merger"}]}}),
    );
    let batch = b.run(
        "import.stage",
        json!({"name": "corp.csv", "content": csv, "account": "Assets:Brokerage"}),
    );
    let row = batch["inv_rows"][0].clone();
    assert_eq!(row["needs_review"], true, "{batch}");
    assert!(row["row"]["notes"].to_string().contains("merger terms"));
    b.run(
        "import.set_inv_row",
        json!({"batch": batch["id"], "row": row["id"],
               "terms": {"new_security": "ACQ", "ratio_new": "1", "ratio_old": "2", "mode": "tax_free"}}),
    );
    b.run("import.commit", json!({"batch": batch["id"]}));
    assert_eq!(lots(&mut b, "ACQ"), vec![l("2024-02-01", "50", "3000.00")]);
}

// ----- performance ----------------------------------------------------------------------

fn perf(b: &mut Book, from: &str, to: &str) -> Value {
    let r = b.run(
        "report.performance",
        json!({"from": from, "to": to, "group": "account"}),
    );
    r["rows"][0].clone()
}

fn six(v: &Value) -> String {
    let d: mk_core::Decimal = v.as_str().unwrap().parse().unwrap();
    format!("{:.6}", d.round_dp(6))
}

fn price(b: &mut Book, sym: &str, date: &str, p: &str) {
    b.run(
        "price.set",
        json!({"security": sym, "date": date, "price": p}),
    );
}

#[test]
fn advanced_at1_mwr_and_twr_single_flow() {
    let mut b = book();
    security(&mut b, "IDX");
    fund(&mut b, "2025-01-01", "1000.00");
    buy(&mut b, "IDX", "2025-01-01", "100", "10.00");
    price(&mut b, "IDX", "2025-12-31", "11.00");
    let r = perf(&mut b, "2025-01-01", "2025-12-31");
    assert_eq!(r["end_value"], "1100.00");
    assert_eq!(r["net_flows"], "1000.00");
    assert_eq!(r["gain"], "100.00");
    assert_eq!(r["mwr"]["percent"], "10.00", "{r}");
    assert_eq!(r["twr"]["percent"], "10.00", "{r}");
    assert_eq!(r["mwr"]["annualised"], true);
}

#[test]
fn advanced_at2_mwr_two_flows() {
    let mut b = book();
    security(&mut b, "IDX");
    fund(&mut b, "2025-01-01", "1000.00");
    fund(&mut b, "2025-07-02", "1000.00");
    buy(&mut b, "IDX", "2025-01-01", "100", "10.00");
    buy(&mut b, "IDX", "2025-07-02", "100", "10.00");
    price(&mut b, "IDX", "2025-12-31", "11.00");
    let r = perf(&mut b, "2025-01-01", "2025-12-31");
    assert_eq!(r["end_value"], "2200.00");
    assert_eq!(r["mwr"]["percent"], "13.46", "{r}");
    assert_eq!(six(&r["mwr"]["value"]), "0.134627");
}

fn at3_book() -> Book {
    let mut b = book();
    security(&mut b, "IDX");
    fund(&mut b, "2025-01-01", "1000.00");
    buy(&mut b, "IDX", "2025-01-01", "100", "10.00");
    price(&mut b, "IDX", "2025-06-30", "11.00");
    fund(&mut b, "2025-07-01", "1100.00");
    buy(&mut b, "IDX", "2025-07-01", "100", "11.00");
    price(&mut b, "IDX", "2025-12-31", "12.00");
    b
}

#[test]
fn advanced_at3_twr_vs_mwr() {
    let mut b = at3_book();
    let r = perf(&mut b, "2025-01-01", "2025-12-31");
    assert_eq!(r["end_value"], "2400.00");
    assert_eq!(r["gain"], "300.00");
    assert_eq!(r["twr"]["percent"], "20.00", "{r}");
    assert_eq!(six(&r["twr"]["value"]), "0.200000");
    assert_eq!(r["mwr"]["percent"], "19.61", "{r}");
    assert_eq!(six(&r["mwr"]["value"]), "0.196090");
}

#[test]
fn advanced_at4_short_period_not_annualised() {
    let mut b = at3_book();
    let r = perf(&mut b, "2025-01-01", "2025-06-30");
    assert_eq!(r["twr"]["percent"], "10.00", "{r}");
    assert_eq!(r["mwr"]["percent"], "10.00", "{r}");
    assert_eq!(r["twr"]["label"], "not annualised");
    assert_eq!(r["mwr"]["label"], "not annualised");
    assert_eq!(six(&r["mwr"]["annual_rate"]), "0.211913");
}

#[test]
fn advanced_performance_no_holdings_is_na() {
    let mut b = book();
    let r = perf(&mut b, "2025-01-01", "2025-12-31");
    assert_eq!(r["mwr"]["percent"], "n/a");
    assert_eq!(r["twr"]["percent"], "n/a");
}
