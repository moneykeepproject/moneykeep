//! investments-core.md acceptance tests 1–11, plus the edge cases it lists.

use super::*;

struct Inv {
    b: Book,
    lot_a: String,
    lot_b: String,
}

fn new_book(method: &str) -> Inv {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "investments"}));
    b.run(
        "security.create",
        json!({"symbol": "vti", "name": "Total Stock Market ETF", "kind": "etf", "basis_method": method}),
    );
    b.account(
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    b.transfer(
        "2025-03-01",
        "Assets:Brokerage",
        "Assets:Checking",
        "5000.00",
    );
    let lot_a = buy(&mut b, "2025-03-10", "10", "200.00", "5.00");
    let lot_b = buy(&mut b, "2025-11-20", "5", "220.00", "0");
    Inv { b, lot_a, lot_b }
}

fn buy(b: &mut Book, date: &str, q: &str, p: &str, c: &str) -> String {
    let v = b.run(
        "inv.buy",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": date,
               "quantity": q, "price": p, "commission": c}),
    );
    v["transaction"]["id"].as_str().unwrap().to_owned()
}

fn sell_args(extra: Value) -> Value {
    let mut a = json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-04-15",
               "quantity": "12", "price": "250.00", "commission": "5.00"});
    for (k, v) in extra.as_object().unwrap() {
        a[k] = v.clone();
    }
    a
}

/// (lot, quantity, basis, proceeds, gain, term) of each consumed portion.
fn portions(d: &Value) -> Vec<(String, String, String, String, String, String)> {
    d["portions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["lot"].as_str().unwrap().to_owned(),
                p["quantity"].as_str().unwrap().to_owned(),
                p["basis"].as_str().unwrap().to_owned(),
                p["proceeds"].as_str().unwrap().to_owned(),
                p["gain"].as_str().unwrap().to_owned(),
                p["term"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn t6(
    l: &str,
    q: &str,
    b: &str,
    p: &str,
    g: &str,
    t: &str,
) -> (String, String, String, String, String, String) {
    (l.into(), q.into(), b.into(), p.into(), g.into(), t.into())
}

/// Open lots as (lot, quantity, basis).
fn lots(b: &mut Book, as_of: &str) -> Vec<(String, String, String)> {
    b.run(
        "inv.lots",
        json!({"account": "Assets:Brokerage", "security": "VTI", "as_of": as_of}),
    )["lots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["lot"].as_str().unwrap().to_owned(),
                l["quantity"].as_str().unwrap().to_owned(),
                l["basis"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn test1() -> (Inv, Value) {
    let mut i = new_book("fifo");
    let sale = i.b.run("inv.sell", sell_args(json!({})));
    (i, sale)
}

#[test]
fn investments_at1_fifo() {
    let (mut i, sale) = test1();
    assert_eq!(
        portions(&sale["disposal"]),
        vec![
            t6(&i.lot_a, "10", "2005.00", "2495.83", "490.83", "long"),
            t6(&i.lot_b, "2", "440.00", "499.17", "59.17", "short"),
        ]
    );
    assert_eq!(i.b.shown("Realized Gains", "USD"), "550.00");
    let lb = i.lot_b.clone();
    assert_eq!(
        lots(&mut i.b, "2026-04-15"),
        vec![(lb, "3".into(), "660.00".into())]
    );
    // The sale's transaction: cash +2,995.00, VTI −2,445.00, Realized Gains −550.00.
    let t = &sale["transaction"];
    let vals: Vec<(String, String)> = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["account_name"].as_str().unwrap().to_owned(),
                s["value"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    for want in [
        ("Assets:Brokerage", "2995.00"),
        ("Assets:Brokerage:VTI", "-2445.00"),
        ("Realized Gains", "-550.00"),
    ] {
        assert!(
            vals.contains(&(want.0.to_owned(), want.1.to_owned())),
            "{want:?} in {vals:?}"
        );
    }
    // The security account: 3 shares, basis 660.00.
    let bal = i.b.run(
        "account.balance",
        json!({"account": "Assets:Brokerage:VTI"}),
    );
    assert_eq!(bal["display"]["VTI"], "3");
    assert_eq!(bal["value"], "660.00");
}

#[test]
fn investments_at2_average() {
    let mut i = new_book("average");
    let sale = i.b.run("inv.sell", sell_args(json!({})));
    let d = &sale["disposal"];
    assert_eq!(d["basis"], "2484.00");
    assert_eq!(d["gain"], "511.00");
    let p: Vec<_> = portions(d)
        .into_iter()
        .map(|(_, q, b, p, g, t)| (q, b, p, g, t))
        .collect();
    let s = |x: &str| x.to_owned();
    assert_eq!(
        p,
        vec![
            (s("10"), s("2070.00"), s("2495.83"), s("425.83"), s("long")),
            (s("2"), s("414.00"), s("499.17"), s("85.17"), s("short")),
        ]
    );
    let left = lots(&mut i.b, "2026-04-15");
    assert_eq!(left.len(), 1);
    assert_eq!((left[0].1.as_str(), left[0].2.as_str()), ("3", "621.00"));
    let rg = i.b.run(
        "report.realized_gains",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    for sec in rg["sections"].as_array().unwrap() {
        for r in sec["rows"].as_array().unwrap() {
            assert_eq!(r["acquired"], "Various");
        }
    }
}

#[test]
fn investments_at3_specific() {
    let mut i = new_book("fifo");
    let sale = i.b.run(
        "inv.sell",
        sell_args(json!({"lots": [
            {"lot": i.lot_b, "quantity": "5"},
            {"lot": i.lot_a, "quantity": "7"}]})),
    );
    let d = &sale["disposal"];
    assert_eq!(
        portions(d),
        vec![
            t6(&i.lot_b, "5", "1100.00", "1247.92", "147.92", "short"),
            t6(&i.lot_a, "7", "1403.50", "1747.08", "343.58", "long"),
        ]
    );
    assert_eq!(d["gain"], "491.50");
    let la = i.lot_a.clone();
    assert_eq!(
        lots(&mut i.b, "2026-04-15"),
        vec![(la, "3".into(), "601.50".into())]
    );
}

#[test]
fn investments_at4_holding_period() {
    let mut i = new_book("fifo");
    for (date, term) in [("2026-03-10", "short"), ("2026-03-11", "long")] {
        let d = i.b.run(
            "inv.preview_sell",
            json!({"account": "Assets:Brokerage", "security": "VTI", "date": date,
                   "quantity": "1", "price": "250.00", "lots": [{"lot": i.lot_a, "quantity": "1"}]}),
        );
        assert_eq!(d["portions"][0]["term"], term, "{date}");
    }
    // A preview posts nothing.
    assert_eq!(i.b.shown("Assets:Brokerage:VTI", "VTI"), "15");
}

fn test5() -> Inv {
    let (mut i, _) = test1();
    i.b.run(
        "inv.split",
        json!({"security": "VTI", "date": "2026-06-01", "ratio_new": "2", "ratio_old": "1"}),
    );
    i.b.run(
        "inv.dividend",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-06-30",
               "amount": "15.00", "kind": "ordinary"}),
    );
    i.b.run(
        "inv.reinvest",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-09-30",
               "amount": "22.00", "kind": "qualified", "quantity": "0.1", "price": "220.00"}),
    );
    i.b.run(
        "price.set",
        json!({"security": "VTI", "date": "2026-10-16", "price": "125.00"}),
    );
    i
}

#[test]
fn investments_at5_split_income_and_holdings() {
    let mut i = test5();
    let l = lots(&mut i.b, "2026-10-19");
    assert_eq!(l.len(), 2);
    assert_eq!(
        (l[0].0.as_str(), l[0].1.as_str(), l[0].2.as_str()),
        (i.lot_b.as_str(), "6", "660.00")
    );
    assert_eq!((l[1].1.as_str(), l[1].2.as_str()), ("0.1", "22.00"));
    assert_eq!(
        i.b.shown("Dividends", "USD"),
        "37.00",
        "15.00 ordinary + 22.00 reinvested"
    );
    let h = i.b.run("inv.holdings", json!({"as_of": "2026-10-19"}));
    let acc = &h["accounts"][0];
    let vti = &acc["holdings"][0];
    assert_eq!(vti["symbol"], "VTI");
    assert_eq!(vti["quantity"], "6.1");
    assert_eq!(vti["basis"], "682.00");
    assert_eq!(vti["price"], "125.00");
    assert_eq!(vti["price_date"], "2026-10-16");
    assert_eq!(vti["market_value"], "762.50");
    assert_eq!(vti["gain"], "80.50");
    assert_eq!(acc["cash"], "4905.00");
    assert_eq!(acc["total"], "5667.50");
}

#[test]
fn investments_at6_backdated_buy_rewrites_the_sale_and_undo_restores() {
    let (mut i, sale) = test1();
    let sale_id = sale["transaction"]["id"].as_str().unwrap().to_owned();
    let before = i.b.state();
    let audit = i.b.audit_len();
    let x = buy(&mut i.b, "2025-01-05", "2", "150.00", "0");
    assert_eq!(i.b.audit_len(), audit + 1, "one audit entry");
    let rg = i.b.run(
        "report.realized_gains",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    let rows: Vec<(String, String, String, String)> = rg["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["rows"].as_array().unwrap().clone())
        .map(|r| {
            (
                r["acquired"].as_str().unwrap().to_owned(),
                r["basis"].as_str().unwrap().to_owned(),
                r["proceeds"].as_str().unwrap().to_owned(),
                r["term"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let s = |x: &str| x.to_owned();
    assert_eq!(
        rows,
        vec![
            (s("2025-01-05"), s("300.00"), s("499.17"), s("long")),
            (s("2025-03-10"), s("2005.00"), s("2495.83"), s("long")),
        ]
    );
    assert_eq!(rg["sections"][0]["gain"], "0.00", "short-term");
    assert_eq!(rg["total"]["gain"], "690.00");
    assert_eq!(i.b.shown("Realized Gains", "USD"), "690.00");
    let t = i.b.run("txn.get", json!({"id": sale_id}));
    assert!(t["splits"].to_string().contains("-2305.00"), "{t}");
    let lb = i.lot_b.clone();
    let open = lots(&mut i.b, "2026-04-15");
    assert_eq!(open, vec![(lb, "5".into(), "1100.00".into())]);
    let _ = x;
    i.b.run("history.undo", json!({}));
    assert_eq!(
        i.b.state(),
        before,
        "undo restores the test-1 state exactly"
    );
    assert_eq!(i.b.shown("Realized Gains", "USD"), "550.00");
}

#[test]
fn investments_at7_insufficient_shares() {
    let mut i = new_book("fifo");
    let e = i.b.err("inv.sell", sell_args(json!({"quantity": "20"})));
    assert_eq!(e.code(), "insufficient_shares");
    assert!(e.to_string().contains("15 held"), "{e}");
}

#[test]
fn investments_at8_return_of_capital() {
    let (mut i, _) = test1();
    i.b.run(
        "inv.reinvest",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-09-30",
               "amount": "22.00", "kind": "qualified", "quantity": "0.1", "price": "220.00"}),
    );
    i.b.run(
        "inv.return_of_capital",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-10-01", "amount": "30.00"}),
    );
    let l = lots(&mut i.b, "2026-10-19");
    let basis: Vec<&str> = l.iter().map(|x| x.2.as_str()).collect();
    assert_eq!(basis, vec!["630.97", "21.03"]);
    assert_eq!(
        i.b.shown("Assets:Brokerage:VTI", "VTI"),
        "3.1",
        "quantity unchanged"
    );
    // Amended by investments-advanced.md: beyond the open basis the excess is a gain.
    let out = i.b.run(
        "inv.return_of_capital",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-10-02", "amount": "700.00"}),
    );
    assert!(
        out["transaction"].to_string().contains("Realized Gains"),
        "{out}"
    );
}

#[test]
fn investments_at9_net_worth_at_market_value() {
    let mut i = test5();
    let nw = i.b.run(
        "report.net_worth",
        json!({"from": "2026-10-19", "to": "2026-10-19", "depth": 3, "today": "2026-10-19"}),
    );
    let rows: Vec<(String, String)> = nw["assets"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["full_name"].as_str().unwrap().to_owned(),
                r["amounts"][0].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(
        rows.contains(&("Assets:Brokerage:VTI".into(), "762.50".into())),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("Assets:Brokerage".into(), "4905.00".into())),
        "{rows:?}"
    );
    assert!(
        !nw["excluded"].to_string().contains("VTI"),
        "a priced security is not excluded: {}",
        nw["excluded"]
    );
}

#[test]
fn investments_at10_module_off_keeps_data_visible() {
    let (mut i, _) = test1();
    i.b.run("module.disable", json!({"module": "investments"}));
    let e = i.b.err(
        "inv.buy",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-05-01",
               "quantity": "1", "price": "1.00"}),
    );
    assert_eq!(e.code(), "module_disabled");
    assert_eq!(i.b.shown("Assets:Brokerage:VTI", "VTI"), "3");
    assert_eq!(i.b.shown("Assets:Brokerage", "USD"), "4890.00");
    let reg = i.b.run(
        "account.register",
        json!({"account": "Assets:Brokerage:VTI", "offset": 0, "limit": 50}),
    );
    assert_eq!(reg["rows"].as_array().unwrap().len(), 3);
}

#[test]
fn investments_at11_price_import() {
    let mut i = new_book("fifo");
    let csv = "Symbol,Date,Price\nVTI,2026-10-15,250.10\nFOO,2026-10-15,1.00\nvti,10/16/2026,251.5\nFOO,2026-10-16,2\n";
    use base64::Engine;
    let v = i.b.run(
        "price.import",
        json!({"file_name": "prices.csv",
               "content_base64": base64::engine::general_purpose::STANDARD.encode(csv)}),
    );
    assert_eq!(v["stored"], 2);
    assert_eq!(v["warnings"].as_array().unwrap().len(), 1, "{v}");
    assert!(v["warnings"][0].as_str().unwrap().contains("FOO"));
    let list = i.b.run("price.list", json!({"security": "VTI"}));
    assert_eq!(
        list,
        json!([
            {"date": "2026-10-15", "price": "250.10"},
            {"date": "2026-10-16", "price": "251.50"}
        ])
    );
    let latest = i.b.run(
        "price.latest",
        json!({"security": "VTI", "as_of": "2026-10-15"}),
    );
    assert_eq!(latest["price"], "250.10");
}

#[test]
fn investments_edges() {
    let (mut i, sale) = test1();
    let sale_id = sale["transaction"]["id"].as_str().unwrap().to_owned();
    // txn.update may change the payee but not amounts.
    let t = i.b.run("txn.get", json!({"id": sale_id}));
    let splits: Vec<Value> = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| json!({"account": s["account"], "value": s["value"], "quantity": s["quantity"]}))
        .collect();
    i.b.run(
        "txn.update",
        json!({"id": sale_id, "date": "2026-04-15", "payee": "Broker", "splits": splits}),
    );
    let mut changed = splits.clone();
    changed[0]["value"] = json!("1.00");
    changed[1]["value"] = json!("-1.00");
    let e = i.b.err(
        "txn.update",
        json!({"id": sale_id, "date": "2026-04-16", "splits": changed}),
    );
    assert_eq!(e.code(), "use_investment_action");
    assert_eq!(
        i.b.err("txn.reverse", json!({"id": sale_id})).code(),
        "use_investment_action"
    );
    // Deleting a buy consumed by the sale oversells.
    let la = i.lot_a.clone();
    assert_eq!(
        i.b.err("txn.delete", json!({"id": la})).code(),
        "insufficient_shares"
    );
    // inv.update replaces the sale (13 → price change) in one command.
    let up = i.b.run(
        "inv.update",
        json!({"txn": sale_id, "account": "Assets:Brokerage", "security": "VTI",
               "date": "2026-04-15", "quantity": "12", "price": "260.00", "commission": "5.00"}),
    );
    assert_eq!(up["disposal"]["proceeds"], "3115.00");
    assert_eq!(i.b.shown("Realized Gains", "USD"), "670.00");
    // A reconciled sale cannot be rewritten by a backdated buy.
    let t = i.b.run("txn.get", json!({"id": sale_id}));
    let cash = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .position(|s| s["account_name"] == "Assets:Brokerage")
        .unwrap();
    i.b.run(
        "txn.set_reconcile",
        json!({"splits": [{"id": sale_id, "split": cash, "state": "r"}]}),
    );
    let e = i.b.err(
        "inv.buy",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2025-01-05",
               "quantity": "2", "price": "150.00"}),
    );
    assert_eq!(e.code(), "would_change_closed");
    // Voiding a buy that the (reconciled) sale depends on is refused too.
    assert_eq!(
        i.b.err("txn.void", json!({"id": i.lot_b})).code(),
        "insufficient_shares"
    );
}

#[test]
fn investments_securities_and_other_actions() {
    let mut i = new_book("fifo");
    // Symbols: unique, not a currency, average only for funds.
    assert_eq!(
        i.b.err(
            "security.create",
            json!({"symbol": "VTI", "name": "x", "kind": "stock"})
        )
        .code(),
        "rejected"
    );
    assert_eq!(
        i.b.err(
            "security.create",
            json!({"symbol": "EUR", "name": "x", "kind": "stock"})
        )
        .code(),
        "invalid_args"
    );
    assert_eq!(
        i.b.err(
            "security.create",
            json!({"symbol": "AAPL", "name": "Apple", "kind": "stock", "basis_method": "average"})
        )
        .code(),
        "invalid_args"
    );
    i.b.run(
        "security.create",
        json!({"symbol": "AAPL", "name": "Apple", "kind": "stock"}),
    );
    i.b.run(
        "inv.interest",
        json!({"account": "Assets:Brokerage", "date": "2026-01-31", "amount": "1.25"}),
    );
    i.b.run(
        "inv.fee",
        json!({"account": "Assets:Brokerage", "date": "2026-01-31", "amount": "2.00"}),
    );
    assert_eq!(i.b.shown("Interest", "USD"), "1.25");
    assert_eq!(i.b.shown("Investment Fees", "USD"), "2.00");
    i.b.run(
        "inv.shares_in",
        json!({"account": "Assets:Brokerage", "security": "AAPL", "date": "2026-02-01",
               "quantity": "3", "lots": [
                   {"acquired": "2020-05-01", "quantity": "1", "basis": "50.00"},
                   {"acquired": "2025-12-01", "quantity": "2", "basis": "400.00"}]}),
    );
    let out = i.b.run(
        "inv.shares_out",
        json!({"account": "Assets:Brokerage", "security": "AAPL", "date": "2026-02-02", "quantity": "1"}),
    );
    assert_eq!(out["disposal"]["basis"], "50.00", "FIFO takes the 2020 lot");
    assert_eq!(i.b.shown("Assets:Brokerage:AAPL", "AAPL"), "2");
    let list = i.b.run("security.list", json!({}));
    let aapl = list
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["symbol"] == "AAPL")
        .unwrap()
        .clone();
    assert_eq!(aapl["shares"], "2");
    assert_eq!(
        i.b.err("security.delete", json!({"security": "AAPL"}))
            .code(),
        "rejected"
    );
    // Investment income report.
    let inc = i.b.run(
        "report.investment_income",
        json!({"from": "2026-01-01", "to": "2026-12-31", "interval": "year"}),
    );
    let row = |k: &str| {
        inc["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["key"] == k)
            .unwrap()["total"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(row("interest"), "1.25");
    assert_eq!(row("fees"), "2.00");
    let csv = i.b.run(
        "report.export",
        json!({"report": "report.investment_income",
               "args": {"from": "2026-01-01", "to": "2026-12-31", "interval": "year"}}),
    );
    assert!(
        csv["content"]
            .as_str()
            .unwrap()
            .contains("Interest,1.25,1.25")
    );
}
