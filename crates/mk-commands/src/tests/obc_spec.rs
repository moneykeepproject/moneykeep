//! investments-options-bonds-crypto.md acceptance tests OP1–OP7, BD1–BD4, CR1–CR5, CR8 and
//! TX1, plus the rules around them (symbols, short positions, wash sales, holdings
//! and the opening balance wizard).

use super::*;

fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "investments"}));
    b.account(
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    b.account(
        json!({"name": "Exchange", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    b.account(
        json!({"name": "Exchange2", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    b.transfer(
        "2025-01-02",
        "Assets:Brokerage",
        "Assets:Checking",
        "50000.00",
    );
    b.transfer(
        "2025-01-02",
        "Assets:Exchange",
        "Assets:Checking",
        "20000.00",
    );
    b
}

fn option(b: &mut Book, under: &str, right: &str, strike: &str, exp: &str) -> String {
    let s = b.run(
        "security.create",
        json!({"kind": "option", "underlying": under, "right": right, "strike": strike,
               "expiration": exp}),
    );
    s["symbol"].as_str().unwrap().to_owned()
}

fn act(b: &mut Book, cmd: &str, args: Value) -> Value {
    let mut a = json!({"account": "Assets:Brokerage"});
    for (k, v) in args.as_object().unwrap() {
        a[k] = v.clone();
    }
    b.run(cmd, a)
}

/// Value (basis) of an account.
fn value(b: &mut Book, account: &str) -> String {
    b.run("account.balance", json!({"account": account}))["value"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn gains_rows(b: &mut Book, from: &str, to: &str) -> Vec<Value> {
    let r = b.run("report.realized_gains", json!({"from": from, "to": to}));
    r["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["rows"].as_array().unwrap().clone())
        .collect()
}

#[test]
fn option_symbol_and_name_are_generated() {
    let mut b = book();
    let sym = option(&mut b, "aapl", "call", "200", "2026-12-18");
    assert_eq!(sym, "AAPL 261218 C200");
    let s = b.run("security.get", json!({"security": sym}));
    assert_eq!(s["name"], "AAPL Dec 18 2026 200 Call");
    assert_eq!(s["option"]["right"], "call");
    assert_eq!(s["option"]["multiplier"], "100");
    // The underlying was created as a stock.
    assert_eq!(
        b.run("security.get", json!({"security": "AAPL"}))["kind"],
        "stock"
    );
    // A typed option symbol gives the terms.
    let s = b.run(
        "security.create",
        json!({"kind": "option", "symbol": "spy 270115 p452.5"}),
    );
    assert_eq!(s["symbol"], "SPY 270115 P452.5");
    assert_eq!(s["option"]["strike"], "452.5");
    assert_eq!(s["option"]["expiration"], "2027-01-15");
    // Spaces are for options only; crypto is letters and digits.
    assert!(matches!(
        b.err(
            "security.create",
            json!({"symbol": "AB C", "name": "x", "kind": "stock"})
        ),
        CommandError::InvalidArgs(_)
    ));
    assert!(matches!(
        b.err(
            "security.create",
            json!({"symbol": "BT-C", "name": "x", "kind": "crypto"})
        ),
        CommandError::InvalidArgs(_)
    ));
    b.run(
        "security.create",
        json!({"symbol": "btc", "name": "Bitcoin", "kind": "crypto"}),
    );
}

/// OP1. Buy 2 AAPL 200 calls at 5.10 + 1.30 → 1,021.30; sell both at 7.00 − 1.30 →
/// 1,398.70, gain 377.40, short-term.
#[test]
fn op1_buy_and_sell_calls() {
    let mut b = book();
    let sym = option(&mut b, "AAPL", "call", "200", "2026-12-18");
    let buy = act(
        &mut b,
        "inv.option_buy",
        json!({"security": sym, "date": "2026-03-02", "quantity": "2", "price": "5.10",
               "commission": "1.30"}),
    );
    assert_eq!(buy["investment"]["amount"], "1021.30");
    assert_eq!(
        value(&mut b, "Assets:Brokerage:AAPL 261218 C200"),
        "1021.30"
    );
    // Whole contracts only.
    assert!(matches!(
        b.err(
            "inv.option_sell",
            json!({"account": "Assets:Brokerage", "security": sym, "date": "2026-06-01",
                   "quantity": "1.5", "price": "7"})
        ),
        CommandError::InvalidArgs(_)
    ));
    let sale = act(
        &mut b,
        "inv.option_sell",
        json!({"security": sym, "date": "2026-06-01", "quantity": "2", "price": "7.00",
               "commission": "1.30"}),
    );
    let d = &sale["disposal"];
    assert_eq!(d["proceeds"], "1398.70");
    assert_eq!(d["basis"], "1021.30");
    assert_eq!(d["gain"], "377.40");
    assert_eq!(d["short_term_gain"], "377.40");
    assert_eq!(b.shown("Realized Gains", "USD"), "377.40");
    assert_eq!(value(&mut b, "Assets:Brokerage:AAPL 261218 C200"), "0.00");
}

/// OP2. A bought call expires worthless: loss = basis, sale date = expiration, long-term
/// when held over a year.
#[test]
fn op2_long_call_expires_worthless() {
    let mut b = book();
    let sym = option(&mut b, "AAPL", "call", "250", "2027-01-15");
    act(
        &mut b,
        "inv.option_buy",
        json!({"security": sym, "date": "2025-12-01", "quantity": "1", "price": "3.00"}),
    );
    let out = act(&mut b, "inv.option_expire", json!({"security": sym}));
    assert_eq!(out["transaction"]["date"], "2027-01-15");
    assert_eq!(out["disposal"]["gain"], "-300.00");
    assert_eq!(out["disposal"]["long_term_gain"], "-300.00");
    let rows = gains_rows(&mut b, "2027-01-01", "2027-12-31");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["sold"], "2027-01-15");
    assert_eq!(rows[0]["acquired"], "2025-12-01");
    assert_eq!(rows[0]["term"], "long");
    assert_eq!(rows[0]["proceeds"], "0.00");
    assert_eq!(rows[0]["basis"], "300.00");
    assert_eq!(b.shown("Realized Gains", "USD"), "-300.00");
    assert_eq!(
        b.shown("Assets:Brokerage:AAPL 270115 C250", "AAPL 270115 C250"),
        "0"
    );
}

/// OP3. Write 1 put at 2.00 (0.65) → cash +199.35, quantity −1; buy to close at 0.50
/// (0.65) → 50.65; gain 148.70 short-term.
#[test]
fn op3_write_and_buy_back_a_put() {
    let mut b = book();
    let sym = option(&mut b, "XYZ", "put", "50", "2026-12-18");
    let acct = format!("Assets:Brokerage:{sym}");
    let before = b.shown("Assets:Brokerage", "USD");
    assert_eq!(before, "50000.00");
    act(
        &mut b,
        "inv.option_write",
        json!({"security": sym, "date": "2025-02-03", "quantity": "1", "price": "2.00",
               "commission": "0.65"}),
    );
    assert_eq!(b.shown("Assets:Brokerage", "USD"), "50199.35");
    assert_eq!(b.shown(&acct, &sym), "-1");
    assert_eq!(value(&mut b, &acct), "-199.35");
    // Short lots are listed with the premium.
    let lots = b.run(
        "inv.lots",
        json!({"account": "Assets:Brokerage", "security": sym, "as_of": "2025-03-01"}),
    );
    assert_eq!(lots["lots"][0]["short"], true);
    assert_eq!(lots["lots"][0]["premium"], "199.35");
    // Selling to close needs held contracts; written ones are bought back instead.
    let e = b.err(
        "inv.option_sell",
        json!({"account": "Assets:Brokerage", "security": sym, "date": "2025-03-01",
               "quantity": "1", "price": "1"}),
    );
    assert_eq!(e.code(), "insufficient_shares");
    // Held more than a year: still short-term (written options).
    let out = act(
        &mut b,
        "inv.option_cover",
        json!({"security": sym, "date": "2026-03-02", "quantity": "1", "price": "0.50",
               "commission": "0.65"}),
    );
    let d = &out["disposal"];
    assert_eq!(d["basis"], "50.65");
    assert_eq!(d["proceeds"], "199.35");
    assert_eq!(d["gain"], "148.70");
    assert_eq!(d["short_term_gain"], "148.70");
    assert_eq!(d["long_term_gain"], "0.00");
    assert_eq!(b.shown(&acct, &sym), "0");
    assert_eq!(value(&mut b, &acct), "0.00");
    assert_eq!(b.shown("Realized Gains", "USD"), "148.70");
    assert_eq!(b.shown("Assets:Brokerage", "USD"), "50148.70");
}

/// OP4. A written put assigned at strike 50: 100 shares bought, basis
/// 5,000 − 199.35 + 0 = 4,800.65; the option position closes with no gain.
#[test]
fn op4_written_put_assigned() {
    let mut b = book();
    let sym = option(&mut b, "XYZ", "put", "50", "2026-12-18");
    act(
        &mut b,
        "inv.option_write",
        json!({"security": sym, "date": "2026-02-03", "quantity": "1", "price": "2.00",
               "commission": "0.65"}),
    );
    let out = act(&mut b, "inv.option_assigned", json!({"security": sym}));
    assert!(out.get("disposal").is_none(), "{out}");
    assert_eq!(b.shown("Assets:Brokerage:XYZ", "XYZ"), "100");
    assert_eq!(value(&mut b, "Assets:Brokerage:XYZ"), "4800.65");
    let lots = b.run(
        "inv.lots",
        json!({"account": "Assets:Brokerage", "security": "XYZ", "as_of": "2026-12-31"}),
    );
    assert_eq!(lots["lots"][0]["acquired"], "2026-12-18");
    assert_eq!(b.shown(&format!("Assets:Brokerage:{sym}"), &sym), "0");
    assert_eq!(value(&mut b, &format!("Assets:Brokerage:{sym}")), "0.00");
    assert_eq!(b.shown("Realized Gains", "USD"), "0.00");
    // Cash: +199.35 premium, −5,000 for the shares.
    assert_eq!(b.shown("Assets:Brokerage", "USD"), "45199.35");
    assert!(gains_rows(&mut b, "2026-01-01", "2026-12-31").is_empty());
}

/// OP5. Exercise 1 long call (basis 300, strike 100): 100 shares, basis 10,300.
#[test]
fn op5_exercise_a_call() {
    let mut b = book();
    let sym = option(&mut b, "ABC", "call", "100", "2026-09-18");
    act(
        &mut b,
        "inv.option_buy",
        json!({"security": sym, "date": "2026-02-02", "quantity": "1", "price": "3.00"}),
    );
    act(
        &mut b,
        "inv.option_exercise",
        json!({"security": sym, "date": "2026-09-10"}),
    );
    assert_eq!(b.shown("Assets:Brokerage:ABC", "ABC"), "100");
    assert_eq!(value(&mut b, "Assets:Brokerage:ABC"), "10300.00");
    assert_eq!(value(&mut b, &format!("Assets:Brokerage:{sym}")), "0.00");
    assert_eq!(b.shown("Assets:Brokerage", "USD"), "39700.00");
    let lots = b.run(
        "inv.lots",
        json!({"account": "Assets:Brokerage", "security": "ABC", "as_of": "2026-12-31"}),
    );
    assert_eq!(lots["lots"][0]["acquired"], "2026-09-10");
    assert!(gains_rows(&mut b, "2026-01-01", "2026-12-31").is_empty());
}

/// OP6. Exercise 1 long put (basis 200, strike 40) against 100 shares bought at 50:
/// proceeds 4,000 − 200 = 3,800; loss 1,200 on the shares.
#[test]
fn op6_exercise_a_put() {
    let mut b = book();
    b.run(
        "security.create",
        json!({"symbol": "DEF", "name": "Def Corp", "kind": "stock"}),
    );
    act(
        &mut b,
        "inv.buy",
        json!({"security": "DEF", "date": "2026-01-05", "quantity": "100", "price": "50"}),
    );
    let sym = option(&mut b, "DEF", "put", "40", "2026-12-18");
    act(
        &mut b,
        "inv.option_buy",
        json!({"security": sym, "date": "2026-01-05", "quantity": "1", "price": "2.00"}),
    );
    let out = act(
        &mut b,
        "inv.option_exercise",
        json!({"security": sym, "date": "2026-11-02"}),
    );
    let d = &out["disposal"];
    assert_eq!(d["proceeds"], "3800.00");
    assert_eq!(d["basis"], "5000.00");
    assert_eq!(d["gain"], "-1200.00");
    assert_eq!(b.shown("Realized Gains", "USD"), "-1200.00");
    assert_eq!(b.shown("Assets:Brokerage:DEF", "DEF"), "0");
    assert_eq!(value(&mut b, &format!("Assets:Brokerage:{sym}")), "0.00");
    // Cash: −5,000 − 200 + 4,000.
    assert_eq!(b.shown("Assets:Brokerage", "USD"), "48800.00");
    // A backdated buy of DEF changes the put's sale (the replay crosses accounts).
    act(
        &mut b,
        "inv.buy",
        json!({"security": "DEF", "date": "2025-12-01", "quantity": "100", "price": "45"}),
    );
    let rows = gains_rows(&mut b, "2026-01-01", "2026-12-31");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["basis"], "4500.00");
    assert_eq!(rows[0]["gain"], "-700.00");
    assert_eq!(b.shown("Realized Gains", "USD"), "-700.00");
}

/// OP7. An open option past expiration shows the Attention item; Expired worthless
/// runs `inv.option_expire`.
#[test]
fn op7_expired_option_attention() {
    let mut b = book();
    let sym = option(&mut b, "AAPL", "call", "200", "2026-10-16");
    act(
        &mut b,
        "inv.option_buy",
        json!({"security": sym, "date": "2026-03-02", "quantity": "2", "price": "5.10"}),
    );
    let label = "AAPL Oct 16 2026 200 Call expired — record what happened";
    let items = |b: &mut Book, today: &str| -> Vec<Value> {
        b.run(
            "dashboard.block_data",
            json!({"dashboard": "home", "block": "attention", "today": today}),
        )["items"]
            .as_array()
            .unwrap()
            .clone()
    };
    assert!(
        !items(&mut b, "2026-10-16")
            .iter()
            .any(|i| i["label"] == label)
    );
    let found = items(&mut b, "2026-11-06")
        .into_iter()
        .find(|i| i["label"] == label)
        .unwrap();
    assert_eq!(found["target"]["screen"], "option_expiry");
    let sec = b.run("security.get", json!({"security": sym}));
    assert_eq!(found["target"]["security"], sec["id"]);
    b.run(
        "inv.option_expire",
        json!({"account": found["target"]["account"], "security": found["target"]["security"]}),
    );
    assert!(
        !items(&mut b, "2026-11-06")
            .iter()
            .any(|i| i["label"] == label)
    );
    assert_eq!(b.shown("Realized Gains", "USD"), "-1020.00");
}

/// A buy of contracts already written closes them first (buy to close), the rest
/// opens a long position; undo restores the short position.
#[test]
fn option_buy_closes_written_contracts_first() {
    let mut b = book();
    let sym = option(&mut b, "XYZ", "call", "60", "2026-12-18");
    let acct = format!("Assets:Brokerage:{sym}");
    act(
        &mut b,
        "inv.option_write",
        json!({"security": sym, "date": "2026-02-03", "quantity": "1", "price": "3.00"}),
    );
    let before = b.state();
    let out = act(
        &mut b,
        "inv.option_buy",
        json!({"security": sym, "date": "2026-03-02", "quantity": "3", "price": "1.00"}),
    );
    // One written contract bought back for 100.00 (gain 200.00), two held at 200.00.
    assert_eq!(out["disposal"]["gain"], "200.00");
    assert_eq!(b.shown(&acct, &sym), "2");
    assert_eq!(value(&mut b, &acct), "200.00");
    assert_eq!(b.shown("Realized Gains", "USD"), "200.00");
    b.run("history.undo", json!({}));
    assert_eq!(b.state(), before);
    assert_eq!(b.shown(&acct, &sym), "-1");
    // Writing while long contracts are held is refused.
    act(
        &mut b,
        "inv.option_cover",
        json!({"security": sym, "date": "2026-03-02", "quantity": "1", "price": "1"}),
    );
    act(
        &mut b,
        "inv.option_buy",
        json!({"security": sym, "date": "2026-03-03", "quantity": "1", "price": "1"}),
    );
    let e = b.err(
        "inv.option_write",
        json!({"account": "Assets:Brokerage", "security": sym, "date": "2026-03-04",
               "quantity": "1", "price": "1"}),
    );
    assert!(matches!(e, CommandError::Rejected(_)), "{e}");
}

/// Short positions are for options only.
#[test]
fn short_positions_are_options_only() {
    let mut b = book();
    b.run(
        "security.create",
        json!({"symbol": "VTI", "name": "Total Market", "kind": "etf"}),
    );
    let e = b.err(
        "inv.sell",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-03-04",
               "quantity": "1", "price": "1"}),
    );
    assert_eq!(e.code(), "insufficient_shares");
    let e = b.err(
        "inv.option_write",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-03-04",
               "quantity": "1", "price": "1"}),
    );
    assert!(matches!(e, CommandError::InvalidArgs(_)), "{e}");
    let e = b.err(
        "inv.shares_in",
        json!({"account": "Assets:Brokerage", "security": "VTI", "date": "2026-03-04",
               "quantity": "1", "short": true,
               "lots": [{"acquired": "2026-01-01", "quantity": "1", "basis": "10"}]}),
    );
    assert!(matches!(e, CommandError::InvalidArgs(_)), "{e}");
}

fn bond(b: &mut Book, symbol: &str, name: &str, tax: &str) {
    b.run(
        "security.create",
        json!({"symbol": symbol, "name": name, "kind": "bond", "coupon_rate": "4.25",
               "maturity": "2027-08-15", "tax": tax}),
    );
}

/// BD1. Buy 5 bonds at 98.50 with 42.10 accrued interest and 5.00 commission: cash
/// −4,972.10; basis 4,930.00; the interest account shows −42.10; the first coupon of
/// 106.25 nets interest for the year to 64.15.
/// BD2. Redeem at maturity at 100: proceeds 5,000; gain 70.00, long-term.
#[test]
fn bd1_bd2_bond_buy_coupon_and_redemption() {
    let mut b = book();
    bond(&mut b, "T27", "Treasury 4.25% 2027", "taxable");
    act(
        &mut b,
        "inv.bond_buy",
        json!({"security": "T27", "date": "2026-03-02", "units": "5", "price": "98.50",
               "accrued_interest": "42.10", "commission": "5.00"}),
    );
    assert_eq!(b.shown("Assets:Brokerage", "USD"), "45027.90");
    assert_eq!(value(&mut b, "Assets:Brokerage:T27"), "4930.00");
    assert_eq!(b.shown("Interest", "USD"), "-42.10");
    // Holdings: face, coupon, maturity, price in percent of face; bought below face.
    b.run(
        "price.set",
        json!({"security": "T27", "date": "2026-03-02", "price": "98.50"}),
    );
    let h = b.run(
        "inv.holdings",
        json!({"accounts": ["Assets:Brokerage"], "as_of": "2026-03-02"}),
    );
    let row = &h["accounts"][0]["holdings"][0];
    assert_eq!(row["market_value"], "4925.00");
    assert_eq!(row["face"], "5000.00");
    assert_eq!(row["coupon_rate"], "4.25");
    assert_eq!(row["maturity"], "2027-08-15");
    assert_eq!(row["price_percent"], "98.5");
    assert_eq!(row["face_note"], "Bought below face by 70.00");
    act(
        &mut b,
        "inv.bond_interest",
        json!({"security": "T27", "date": "2026-08-15", "amount": "106.25"}),
    );
    assert_eq!(b.shown("Interest", "USD"), "64.15");
    let inc = b.run(
        "report.investment_income",
        json!({"from": "2026-01-01", "to": "2026-12-31", "interval": "year"}),
    );
    let interest = inc["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == "interest")
        .unwrap()["total"]
        .clone();
    assert_eq!(interest, "64.15");
    // Maturity: an Attention item on the date.
    let att = b.run(
        "dashboard.block_data",
        json!({"dashboard": "home", "block": "attention", "today": "2027-08-15"}),
    );
    let item = att["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["label"] == "Treasury 4.25% 2027 matured — record the redemption")
        .unwrap()
        .clone();
    assert_eq!(item["target"]["screen"], "bond_redeem");
    let out = act(&mut b, "inv.bond_redeem", json!({"security": "T27"}));
    let d = &out["disposal"];
    assert_eq!(out["transaction"]["date"], "2027-08-15");
    assert_eq!(d["proceeds"], "5000.00");
    assert_eq!(d["gain"], "70.00");
    assert_eq!(d["long_term_gain"], "70.00");
    assert_eq!(b.shown("Assets:Brokerage:T27", "T27"), "0");
    let att = b.run(
        "dashboard.block_data",
        json!({"dashboard": "home", "block": "attention", "today": "2027-08-16"}),
    );
    assert!(!att.to_string().contains("matured"));
}

/// BD3. Municipal bond coupons go to Tax-exempt Interest; Treasury to US Treasury
/// Interest.
#[test]
fn bd3_interest_accounts_by_tax() {
    let mut b = book();
    bond(&mut b, "MUNI27", "City GO 4.25% 2027", "municipal");
    bond(&mut b, "UST27", "US Treasury 4.25% 2027", "treasury");
    for s in ["MUNI27", "UST27"] {
        act(
            &mut b,
            "inv.bond_buy",
            json!({"security": s, "date": "2026-03-02", "units": "1", "price": "100"}),
        );
    }
    act(
        &mut b,
        "inv.bond_interest",
        json!({"security": "MUNI27", "date": "2026-08-15", "amount": "21.25"}),
    );
    act(
        &mut b,
        "inv.bond_interest",
        json!({"security": "UST27", "date": "2026-08-15", "amount": "21.30"}),
    );
    assert_eq!(b.shown("Tax-exempt Interest", "USD"), "21.25");
    assert_eq!(b.shown("US Treasury Interest", "USD"), "21.30");
    let list = b.run("account.list", json!({}));
    assert!(
        !list
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["name"] == "Interest")
    );
}

/// BD4. Add coupon reminders creates one reminder per payment date to maturity with the
/// right amount; entering one records the bond interest.
#[test]
fn bd4_coupon_reminders() {
    let mut b = book();
    bond(&mut b, "T27", "Treasury 4.25% 2027", "taxable");
    act(
        &mut b,
        "inv.bond_buy",
        json!({"security": "T27", "date": "2026-03-02", "units": "5", "price": "100"}),
    );
    let out = b.run(
        "inv.bond_reminders",
        json!({"account": "Assets:Brokerage", "security": "T27", "today": "2026-03-02"}),
    );
    assert_eq!(out["amount"], "106.25");
    assert_eq!(
        out["dates"],
        json!(["2026-08-15", "2027-02-15", "2027-08-15"])
    );
    let id = out["schedule"]["id"].as_str().unwrap().to_owned();
    assert_eq!(out["schedule"]["auto_enter"], false);
    let up = b.run(
        "schedule.upcoming",
        json!({"today": "2026-03-02", "from": "2026-03-02", "to": "2030-12-31"}),
    );
    let occ: Vec<&Value> = up["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["schedule"] == id)
        .collect();
    assert_eq!(occ.len(), 3);
    for o in &occ {
        assert_eq!(o["amount"], "106.25", "{o}");
    }
    // Entering one is the bond interest action.
    let t = b.run("schedule.enter", json!({"schedule": id}));
    assert_eq!(t["date"], "2026-08-15");
    let m = b.run("inv.get", json!({"txn": t["id"]}));
    assert_eq!(m["action"], "bond_interest");
    assert_eq!(b.shown("Interest", "USD"), "106.25");
    // A zero-coupon bond has nothing to remind of.
    b.run(
        "security.create",
        json!({"symbol": "Z30", "name": "Zero 2030", "kind": "bond", "coupon_rate": "0",
               "maturity": "2030-05-15"}),
    );
    act(
        &mut b,
        "inv.bond_buy",
        json!({"security": "Z30", "date": "2026-03-02", "units": "1", "price": "80"}),
    );
    assert!(matches!(
        b.err(
            "inv.bond_reminders",
            json!({"account": "Assets:Brokerage", "security": "Z30", "today": "2026-03-02"})
        ),
        CommandError::InvalidArgs(_)
    ));
}

fn coins(b: &mut Book) {
    for (s, n) in [("BTC", "Bitcoin"), ("ETH", "Ether"), ("SOL", "Solana")] {
        b.run(
            "security.create",
            json!({"symbol": s, "name": n, "kind": "crypto"}),
        );
    }
}

fn ex(b: &mut Book, cmd: &str, args: Value) -> Value {
    let mut a = json!({"account": "Assets:Exchange"});
    for (k, v) in args.as_object().unwrap() {
        a[k] = v.clone();
    }
    b.run(cmd, a)
}

/// CR1. Buy 0.12345678 BTC for 8,000 then sell 0.05 for 4,000: FIFO basis 3,240.00;
/// gain 760.00.
#[test]
fn cr1_crypto_buy_and_sell() {
    let mut b = book();
    coins(&mut b);
    ex(
        &mut b,
        "inv.buy",
        json!({"security": "BTC", "date": "2026-01-10", "quantity": "0.12345678",
               "price": "64800", "amount": "8000.00"}),
    );
    // At most 8 decimals.
    assert!(matches!(
        b.err(
            "inv.sell",
            json!({"account": "Assets:Exchange", "security": "BTC", "date": "2026-05-04",
                   "quantity": "0.000000001", "amount": "1"})
        ),
        CommandError::InvalidArgs(_)
    ));
    let out = ex(
        &mut b,
        "inv.sell",
        json!({"security": "BTC", "date": "2026-05-04", "quantity": "0.05", "price": "80000",
               "amount": "4000.00"}),
    );
    assert_eq!(out["disposal"]["basis"], "3240.00");
    assert_eq!(out["disposal"]["gain"], "760.00");
    // Holdings show 8 decimals.
    let h = b.run(
        "inv.holdings",
        json!({"accounts": ["Assets:Exchange"], "as_of": "2026-05-04"}),
    );
    assert_eq!(
        h["accounts"][0]["holdings"][0]["quantity_text"],
        "0.07345678"
    );
}

/// CR2. Swap 1 ETH (basis 2,000) for 50 SOL worth 3,000: gain 1,000 on ETH; SOL lot
/// basis 3,000.
#[test]
fn cr2_crypto_swap() {
    let mut b = book();
    coins(&mut b);
    ex(
        &mut b,
        "inv.buy",
        json!({"security": "ETH", "date": "2026-01-10", "quantity": "1", "amount": "2000.00"}),
    );
    let out = ex(
        &mut b,
        "inv.crypto_swap",
        json!({"from": "ETH", "from_quantity": "1", "to": "SOL", "to_quantity": "50",
               "value": "3000.00", "date": "2026-04-01"}),
    );
    assert_eq!(out["disposal"]["gain"], "1000.00");
    assert_eq!(b.shown("Realized Gains", "USD"), "1000.00");
    assert_eq!(value(&mut b, "Assets:Exchange:SOL"), "3000.00");
    assert_eq!(b.shown("Assets:Exchange:SOL", "SOL"), "50");
    assert_eq!(b.shown("Assets:Exchange:ETH", "ETH"), "0");
    let lots = b.run(
        "inv.lots",
        json!({"account": "Assets:Exchange", "security": "SOL", "as_of": "2026-04-01"}),
    );
    assert_eq!(lots["lots"][0]["basis"], "3000.00");
    assert_eq!(lots["lots"][0]["acquired"], "2026-04-01");
}

/// CR3. Reward 0.1 ETH worth 300: Crypto Rewards +300; a lot with basis 300.
#[test]
fn cr3_crypto_reward() {
    let mut b = book();
    coins(&mut b);
    ex(
        &mut b,
        "inv.crypto_reward",
        json!({"security": "ETH", "date": "2026-02-01", "quantity": "0.1", "value": "300.00"}),
    );
    assert_eq!(b.shown("Crypto Rewards", "USD"), "300.00");
    let lots = b.run(
        "inv.lots",
        json!({"account": "Assets:Exchange", "security": "ETH", "as_of": "2026-02-01"}),
    );
    assert_eq!(lots["lots"][0]["basis"], "300.00");
    assert_eq!(lots["lots"][0]["quantity"], "0.1");
}

/// CR4. Transfer 0.05 BTC between exchanges: lots keep acquired dates and basis; no
/// gain.
#[test]
fn cr4_transfer_lots() {
    let mut b = book();
    coins(&mut b);
    ex(
        &mut b,
        "inv.buy",
        json!({"security": "BTC", "date": "2025-06-10", "quantity": "0.12345678",
               "amount": "8000.00"}),
    );
    let out = ex(
        &mut b,
        "inv.transfer_lots",
        json!({"security": "BTC", "date": "2026-04-01", "quantity": "0.05",
               "to_account": "Assets:Exchange2"}),
    );
    assert!(out.get("disposal").is_none(), "{out}");
    let lots = b.run(
        "inv.lots",
        json!({"account": "Assets:Exchange2", "security": "BTC", "as_of": "2026-04-01"}),
    );
    assert_eq!(lots["lots"][0]["acquired"], "2025-06-10");
    assert_eq!(lots["lots"][0]["basis"], "3240.00");
    assert_eq!(lots["lots"][0]["quantity"], "0.05");
    assert_eq!(value(&mut b, "Assets:Exchange:BTC"), "4760.00");
    assert_eq!(value(&mut b, "Assets:Exchange2:BTC"), "3240.00");
    assert_eq!(b.shown("Realized Gains", "USD"), "0.00");
    assert!(gains_rows(&mut b, "2026-01-01", "2026-12-31").is_empty());
    // Selling there later keeps the long-term holding period.
    let s = b.run(
        "inv.sell",
        json!({"account": "Assets:Exchange2", "security": "BTC", "date": "2026-07-01",
               "quantity": "0.05", "amount": "4000.00"}),
    );
    assert_eq!(s["disposal"]["long_term_gain"], "760.00");
    // With a network fee in coin: a disposal at its value.
    b.run(
        "price.set",
        json!({"security": "BTC", "date": "2026-08-01", "price": "60000"}),
    );
    let out = ex(
        &mut b,
        "inv.transfer_lots",
        json!({"security": "BTC", "date": "2026-08-01", "quantity": "0.01",
               "fee_quantity": "0.0001", "to_account": "Assets:Exchange2"}),
    );
    assert_eq!(out["disposal"]["proceeds"], "6.00");
    assert_eq!(b.shown("Investment Fees", "USD"), "6.00");
    assert_eq!(b.shown("Assets:Exchange:BTC", "BTC"), "0.06335678");
}

/// CR5. A fee of 0.0001 BTC worth 6.00 with basis 4.00: gain 2.00; Investment Fees
/// +6.00.
#[test]
fn cr5_crypto_fee() {
    let mut b = book();
    coins(&mut b);
    ex(
        &mut b,
        "inv.buy",
        json!({"security": "BTC", "date": "2026-01-10", "quantity": "0.001", "amount": "40.00"}),
    );
    let out = ex(
        &mut b,
        "inv.crypto_fee",
        json!({"security": "BTC", "date": "2026-02-10", "quantity": "0.0001", "value": "6.00"}),
    );
    assert_eq!(out["disposal"]["basis"], "4.00");
    assert_eq!(out["disposal"]["gain"], "2.00");
    assert_eq!(b.shown("Investment Fees", "USD"), "6.00");
    assert_eq!(b.shown("Realized Gains", "USD"), "2.00");
}

/// Wash-sale flags leave options and crypto out.
#[test]
fn wash_sales_leave_options_and_crypto_out() {
    let mut b = book();
    coins(&mut b);
    ex(
        &mut b,
        "inv.buy",
        json!({"security": "BTC", "date": "2026-01-10", "quantity": "0.1", "amount": "8000.00"}),
    );
    ex(
        &mut b,
        "inv.sell",
        json!({"security": "BTC", "date": "2026-03-01", "quantity": "0.1", "amount": "6000.00"}),
    );
    ex(
        &mut b,
        "inv.buy",
        json!({"security": "BTC", "date": "2026-03-05", "quantity": "0.1", "amount": "6100.00"}),
    );
    let sym = option(&mut b, "AAPL", "call", "200", "2026-12-18");
    act(
        &mut b,
        "inv.option_buy",
        json!({"security": sym, "date": "2026-03-02", "quantity": "1", "price": "5"}),
    );
    act(
        &mut b,
        "inv.option_sell",
        json!({"security": sym, "date": "2026-03-10", "quantity": "1", "price": "2"}),
    );
    act(
        &mut b,
        "inv.option_buy",
        json!({"security": sym, "date": "2026-03-12", "quantity": "1", "price": "2"}),
    );
    let rows = gains_rows(&mut b, "2026-01-01", "2026-12-31");
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert_eq!(r["wash_sale"], false, "{r}");
    }
    let w = b.run(
        "inv.wash_check",
        json!({"security": "BTC", "date": "2026-03-20", "quantity": "0.1"}),
    );
    assert_eq!(w["applies"], false);
}

/// TX1. The Form 8949 export lists OP1, OP2, OP3, BD2 and CR1 with correct dates,
/// proceeds, basis and term.
#[test]
fn tx1_form_8949_export() {
    let mut b = book();
    coins(&mut b);
    // OP1
    let c200 = option(&mut b, "AAPL", "call", "200", "2026-12-18");
    act(
        &mut b,
        "inv.option_buy",
        json!({"security": c200, "date": "2026-03-02", "quantity": "2", "price": "5.10",
               "commission": "1.30"}),
    );
    act(
        &mut b,
        "inv.option_sell",
        json!({"security": c200, "date": "2026-06-01", "quantity": "2", "price": "7.00",
               "commission": "1.30"}),
    );
    // OP2 (expires 2026-12-18, bought 2025-11-03: long-term)
    let c250 = option(&mut b, "AAPL", "call", "250", "2026-12-18");
    act(
        &mut b,
        "inv.option_buy",
        json!({"security": c250, "date": "2025-11-03", "quantity": "1", "price": "3.00"}),
    );
    act(&mut b, "inv.option_expire", json!({"security": c250}));
    // OP3
    let p50 = option(&mut b, "XYZ", "put", "50", "2026-12-18");
    act(
        &mut b,
        "inv.option_write",
        json!({"security": p50, "date": "2026-02-03", "quantity": "1", "price": "2.00",
               "commission": "0.65"}),
    );
    act(
        &mut b,
        "inv.option_cover",
        json!({"security": p50, "date": "2026-04-01", "quantity": "1", "price": "0.50",
               "commission": "0.65"}),
    );
    // BD2 (bought 2025-03-03, redeemed 2026-08-15 at maturity)
    b.run(
        "security.create",
        json!({"symbol": "T26", "name": "Treasury 4.25% 2026", "kind": "bond",
               "coupon_rate": "4.25", "maturity": "2026-08-15", "tax": "treasury"}),
    );
    act(
        &mut b,
        "inv.bond_buy",
        json!({"security": "T26", "date": "2025-03-03", "units": "5", "price": "98.50",
               "accrued_interest": "42.10", "commission": "5.00"}),
    );
    act(&mut b, "inv.bond_redeem", json!({"security": "T26"}));
    // CR1
    ex(
        &mut b,
        "inv.buy",
        json!({"security": "BTC", "date": "2026-01-10", "quantity": "0.12345678",
               "amount": "8000.00"}),
    );
    ex(
        &mut b,
        "inv.sell",
        json!({"security": "BTC", "date": "2026-05-04", "quantity": "0.05", "amount": "4000.00"}),
    );
    let out = b.run(
        "report.export",
        json!({"report": "report.realized_gains",
               "args": {"from": "2026-01-01", "to": "2026-12-31"}}),
    );
    let csv = out["content"].as_str().unwrap();
    let want = [
        // Symbol, …, acquired, sold, proceeds, basis, gain, term, …, description
        "AAPL 261218 C200,AAPL Dec 18 2026 200 Call,2,2026-03-02,2026-06-01,1398.70,1021.30,377.40,short,,,2 AAPL Dec 18 2026 200 Call",
        "XYZ 261218 P50,XYZ Dec 18 2026 50 Put,1,2026-02-03,2026-04-01,199.35,50.65,148.70,short,,,1 XYZ Dec 18 2026 50 Put",
        "BTC,Bitcoin,0.05,2026-01-10,2026-05-04,4000.00,3240.00,760.00,short,,,0.05 BTC",
        "T26,Treasury 4.25% 2026,5,2025-03-03,2026-08-15,5000.00,4930.00,70.00,long,,,5000.00 face Treasury 4.25% 2026",
        "AAPL 261218 C250,AAPL Dec 18 2026 250 Call,1,2025-11-03,2026-12-18,0.00,300.00,-300.00,long,,,1 AAPL Dec 18 2026 250 Call",
    ];
    for w in want {
        assert!(csv.contains(w), "missing {w}\n{csv}");
    }
}

/// The opening balance wizard's holdings step accepts options (long and written),
/// bonds (units and price) and crypto (8 decimals).
#[test]
fn opening_holdings_take_the_new_kinds() {
    let mut b = book();
    let args = json!({
        "account": "Assets:Exchange2",
        "cash": {"balance": "0", "as_of": "2026-11-06"},
        "today": "2026-11-06",
        "create_securities": true,
        "holdings": [
            {"symbol": "AAPL 261218 C200", "kind": "option", "shares": "2", "price": "5.00",
             "basis": {"level": "total", "total": "800.00"}},
            {"symbol": "XYZ 261218 P50", "kind": "option", "shares": "-1", "price": "1.00",
             "basis": {"level": "total", "total": "199.35"}},
            {"symbol": "T27", "name": "Treasury 4.25% 2027", "kind": "bond", "shares": "5",
             "price": "99.00", "maturity": "2027-08-15", "coupon_rate": "4.25",
             "basis": {"level": "total", "total": "4930.00"}},
            {"symbol": "BTC", "name": "Bitcoin", "kind": "crypto", "shares": "0.12345678",
             "price": "60000", "basis": {"level": "unknown"}}
        ]
    });
    let plan = b.run("opening.inv_plan", args.clone());
    let rows = plan["holdings"].as_array().unwrap();
    assert!(rows.iter().all(|r| r["problem"].is_null()), "{plan}");
    // 2 × 100 × 5 + (−1 × 100 × 1) + 5 × 1,000 × 0.99 + 0.12345678 × 60,000.
    assert_eq!(plan["total"]["total"], "13257.41");
    b.run("opening.inv_apply", args);
    assert_eq!(
        b.shown("Assets:Exchange2:AAPL 261218 C200", "AAPL 261218 C200"),
        "2"
    );
    assert_eq!(value(&mut b, "Assets:Exchange2:AAPL 261218 C200"), "800.00");
    assert_eq!(
        b.shown("Assets:Exchange2:XYZ 261218 P50", "XYZ 261218 P50"),
        "-1"
    );
    assert_eq!(value(&mut b, "Assets:Exchange2:XYZ 261218 P50"), "-199.35");
    assert_eq!(value(&mut b, "Assets:Exchange2:T27"), "4930.00");
    assert_eq!(b.shown("Assets:Exchange2:BTC", "BTC"), "0.12345678");
    let s = b.run("security.get", json!({"security": "T27"}));
    assert_eq!(s["bond"]["maturity"], "2027-08-15");
    // The written put can be bought back from its opening short lot.
    let out = b.run(
        "inv.option_cover",
        json!({"account": "Assets:Exchange2", "security": "XYZ 261218 P50",
               "date": "2026-11-20", "quantity": "1", "price": "0.50"}),
    );
    assert_eq!(out["disposal"]["gain"], "149.35");
    // Holdings: the written option is a liability within the account's total.
    let h = b.run(
        "inv.holdings",
        json!({"accounts": ["Assets:Exchange2"], "as_of": "2026-11-06"}),
    );
    let put = h["accounts"][0]["holdings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["symbol"] == "XYZ 261218 P50")
        .unwrap()
        .clone();
    assert_eq!(put["quantity"], "-1");
    assert_eq!(put["market_value"], "-100.00");
    assert_eq!(put["short"], true);
    assert_eq!(put["right"], "put");
    assert_eq!(put["days_left"], 42);
}

// ----- revision 2: editing the new actions; rewards and bond interest in income -----

fn txn_of(out: &Value) -> String {
    out["transaction"]["id"].as_str().unwrap().to_owned()
}

/// OP8. Write a put (OP3), edit it: the stored arguments are those of the form (1
/// contract at 2.00, commission 0.65); a price of 2.50 → cash +249.35; Undo restores
/// 199.35.
#[test]
fn op8_edit_a_written_put() {
    let mut b = book();
    let sym = option(&mut b, "XYZ", "put", "50", "2026-12-18");
    let out = act(
        &mut b,
        "inv.option_write",
        json!({"security": sym, "date": "2025-02-03", "quantity": "1", "price": "2.00",
               "commission": "0.65"}),
    );
    let txn = txn_of(&out);
    let m = b.run("inv.get", json!({"txn": txn}));
    assert_eq!(m["action"], "option_write");
    assert_eq!(m["args"]["quantity"], "1");
    assert_eq!(m["args"]["price"], "2.00");
    assert_eq!(m["args"]["commission"], "0.65");
    // The preview of the edit shows the transaction as it will be.
    let p = b.run(
        "inv.preview",
        json!({"txn": txn, "action": "option_write", "account": "Assets:Brokerage",
               "security": sym, "date": "2025-02-03", "quantity": "1", "price": "2.50",
               "commission": "0.65"}),
    );
    assert!(p.to_string().contains("249.35"), "{p}");
    assert_eq!(b.shown("Assets:Brokerage", "USD"), "50199.35");
    let n = b.txn_count();
    b.run(
        "inv.update",
        json!({"txn": txn, "action": "option_write", "account": "Assets:Brokerage",
               "security": sym, "date": "2025-02-03", "quantity": "1", "price": "2.50",
               "commission": "0.65"}),
    );
    assert_eq!(b.txn_count(), n, "replaced, not added");
    assert_eq!(b.shown("Assets:Brokerage", "USD"), "50249.35");
    let lots = b.run(
        "inv.lots",
        json!({"account": "Assets:Brokerage", "security": sym, "as_of": "2025-03-01"}),
    );
    assert_eq!(lots["lots"][0]["premium"], "249.35");
    b.run("history.undo", json!({}));
    assert_eq!(b.shown("Assets:Brokerage", "USD"), "50199.35");
    let lots = b.run(
        "inv.lots",
        json!({"account": "Assets:Brokerage", "security": sym, "as_of": "2025-03-01"}),
    );
    assert_eq!(lots["lots"][0]["premium"], "199.35");
}

/// Revision 2: an edit may change the action (Opening ↔ Closing, Buy ↔ Sell) in the
/// same replace, and lots and gains are recomputed.
#[test]
fn edit_changes_the_option_action() {
    let mut b = book();
    let sym = option(&mut b, "XYZ", "put", "50", "2026-12-18");
    let acct = format!("Assets:Brokerage:{sym}");
    act(
        &mut b,
        "inv.option_write",
        json!({"security": sym, "date": "2025-02-03", "quantity": "1", "price": "2.00",
               "commission": "0.65"}),
    );
    // Recorded as a buy to open by mistake; it closed the written contract.
    let out = act(
        &mut b,
        "inv.option_buy",
        json!({"security": sym, "date": "2025-03-03", "quantity": "1", "price": "0.50",
               "commission": "0.65"}),
    );
    let txn = txn_of(&out);
    assert_eq!(b.shown(&acct, &sym), "0");
    // Sell to open instead (Buy → Sell, still opening): two written contracts.
    b.run(
        "inv.update",
        json!({"txn": txn, "action": "option_write", "account": "Assets:Brokerage",
               "security": sym, "date": "2025-03-03", "quantity": "1", "price": "0.50",
               "commission": "0.65"}),
    );
    assert_eq!(b.shown(&acct, &sym), "-2");
    assert_eq!(b.shown("Realized Gains", "USD"), "0.00");
    assert_eq!(
        b.run("inv.get", json!({"txn": txn}))["action"],
        "option_write"
    );
    // And back to a closing buy (Opening → Closing): gain 148.70 short-term.
    let out = b.run(
        "inv.update",
        json!({"txn": txn, "action": "option_cover", "account": "Assets:Brokerage",
               "security": sym, "date": "2025-03-03", "quantity": "1", "price": "0.50",
               "commission": "0.65"}),
    );
    assert_eq!(out["disposal"]["short_term_gain"], "148.70");
    assert_eq!(b.shown(&acct, &sym), "0");
    assert_eq!(b.shown("Realized Gains", "USD"), "148.70");
}

/// BD5. Edit BD1's buy so accrued interest is 40.00 → the interest account shows
/// −40.00 and the coupon nets 66.25.
#[test]
fn bd5_edit_accrued_interest_of_a_bond_buy() {
    let mut b = book();
    bond(&mut b, "T27", "Treasury 4.25% 2027", "taxable");
    let out = act(
        &mut b,
        "inv.bond_buy",
        json!({"security": "T27", "date": "2026-03-02", "units": "5", "price": "98.50",
               "accrued_interest": "42.10", "commission": "5.00"}),
    );
    let txn = txn_of(&out);
    let m = b.run("inv.get", json!({"txn": txn}));
    assert_eq!(m["args"]["units"], "5");
    assert_eq!(m["args"]["accrued_interest"], "42.10");
    b.run(
        "inv.update",
        json!({"txn": txn, "action": "bond_buy", "account": "Assets:Brokerage",
               "security": "T27", "date": "2026-03-02", "units": "5", "price": "98.50",
               "accrued_interest": "40.00", "commission": "5.00"}),
    );
    assert_eq!(b.shown("Interest", "USD"), "-40.00");
    assert_eq!(value(&mut b, "Assets:Brokerage:T27"), "4930.00");
    assert_eq!(b.shown("Assets:Brokerage", "USD"), "45030.00");
    act(
        &mut b,
        "inv.bond_interest",
        json!({"security": "T27", "date": "2026-08-15", "amount": "106.25"}),
    );
    assert_eq!(b.shown("Interest", "USD"), "66.25");
}

/// CR6. Edit CR2's swap so the SOL received is worth 3,200 → gain on ETH 1,200; SOL
/// basis 3,200.
#[test]
fn cr6_edit_a_swap() {
    let mut b = book();
    coins(&mut b);
    ex(
        &mut b,
        "inv.buy",
        json!({"security": "ETH", "date": "2026-01-10", "quantity": "1", "amount": "2000.00"}),
    );
    let out = ex(
        &mut b,
        "inv.crypto_swap",
        json!({"from": "ETH", "from_quantity": "1", "to": "SOL", "to_quantity": "50",
               "value": "3000.00", "date": "2026-04-01"}),
    );
    let txn = txn_of(&out);
    let out = b.run(
        "inv.update",
        json!({"txn": txn, "action": "crypto_swap", "account": "Assets:Exchange",
               "from": "ETH", "from_quantity": "1", "to": "SOL", "to_quantity": "50",
               "value": "3200.00", "date": "2026-04-01"}),
    );
    assert_eq!(out["disposal"]["gain"], "1200.00");
    assert_eq!(b.shown("Realized Gains", "USD"), "1200.00");
    assert_eq!(value(&mut b, "Assets:Exchange:SOL"), "3200.00");
    let lots = b.run(
        "inv.lots",
        json!({"account": "Assets:Exchange", "security": "SOL", "as_of": "2026-04-01"}),
    );
    assert_eq!(lots["lots"][0]["basis"], "3200.00");
}

fn income_row(r: &Value, key: &str) -> Option<String> {
    r["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == key)
        .map(|x| x["total"].as_str().unwrap().to_owned())
}

/// CR7. Investment income for the year of CR3 and BD1 shows Crypto rewards 300.00
/// and Interest 64.15 (106.25 coupon − 42.10 accrued), included in the total; the
/// CSV and the Tax summary use the same row names; rows that are zero are left out.
#[test]
fn cr7_investment_income_rewards_and_interest() {
    let mut b = book();
    coins(&mut b);
    ex(
        &mut b,
        "inv.crypto_reward",
        json!({"security": "ETH", "date": "2026-02-01", "quantity": "0.1", "value": "300.00"}),
    );
    bond(&mut b, "T27", "Treasury 4.25% 2027", "taxable");
    act(
        &mut b,
        "inv.bond_buy",
        json!({"security": "T27", "date": "2026-03-02", "units": "5", "price": "98.50",
               "accrued_interest": "42.10", "commission": "5.00"}),
    );
    act(
        &mut b,
        "inv.bond_interest",
        json!({"security": "T27", "date": "2026-08-15", "amount": "106.25"}),
    );
    let args = json!({"from": "2026-01-01", "to": "2026-12-31", "interval": "year"});
    let r = b.run("report.investment_income", args.clone());
    assert_eq!(income_row(&r, "crypto_rewards").as_deref(), Some("300.00"));
    assert_eq!(income_row(&r, "interest").as_deref(), Some("64.15"));
    assert_eq!(income_row(&r, "total").as_deref(), Some("364.15"));
    assert_eq!(
        income_row(&r, "interest_tax_exempt"),
        None,
        "zero rows left out"
    );
    assert_eq!(income_row(&r, "interest_treasury"), None);
    let labels: Vec<&str> = r["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["label"].as_str().unwrap())
        .collect();
    let pos = |l: &str| labels.iter().position(|x| *x == l).unwrap();
    assert!(pos("Crypto rewards") < pos("Interest"));
    let csv = b.run(
        "report.export",
        json!({"report": "report.investment_income", "args": args}),
    );
    let csv = csv["content"].as_str().unwrap();
    assert!(csv.contains("Crypto rewards,300.00,300.00"), "{csv}");
    assert!(csv.contains("Interest,64.15,64.15"), "{csv}");
    assert!(!csv.contains("Tax-exempt interest"), "{csv}");
    let t = b.run(
        "report.tax_summary",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    let inc = &t["investment_income"];
    assert_eq!(inc["title"], "Investment income");
    let rows: Vec<(String, String)> = inc["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            (
                x["label"].as_str().unwrap().to_owned(),
                x["amount"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            ("Crypto rewards".to_owned(), "300.00".to_owned()),
            ("Interest".to_owned(), "64.15".to_owned()),
        ]
    );
    let csv = b.run(
        "report.export",
        json!({"report": "report.tax_summary",
               "args": {"from": "2026-01-01", "to": "2026-12-31"}}),
    );
    let csv = csv["content"].as_str().unwrap();
    assert!(
        csv.contains("Investment income,,Crypto rewards,,300.00"),
        "{csv}"
    );
    // Nothing in another year: no section.
    let t = b.run(
        "report.tax_summary",
        json!({"from": "2024-01-01", "to": "2024-12-31"}),
    );
    assert!(t["investment_income"].is_null());
}

/// CR8 (revision 3). CR5's fee of 6.00 in coin shows in the Fees row for its month
/// (and the total); a transfer's network fee in coin does too; the Tax summary's
/// investment income section has the Fees row.
#[test]
fn cr8_coin_fees_in_investment_income() {
    let mut b = book();
    coins(&mut b);
    ex(
        &mut b,
        "inv.buy",
        json!({"security": "BTC", "date": "2026-01-10", "quantity": "0.1", "amount": "4000.00"}),
    );
    ex(
        &mut b,
        "inv.crypto_fee",
        json!({"security": "BTC", "date": "2026-02-10", "quantity": "0.0001", "value": "6.00"}),
    );
    let r = b.run(
        "report.investment_income",
        json!({"from": "2026-01-01", "to": "2026-03-31", "interval": "month"}),
    );
    let fees = r["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == "fees")
        .unwrap()
        .clone();
    assert_eq!(fees["label"], "Fees");
    assert_eq!(fees["amounts"], json!(["0.00", "6.00", "0.00"]), "{r}");
    assert_eq!(fees["total"], "6.00");
    // The fee's disposal gain (6.00 − 4.00 basis = 2.00) is a short-term gain.
    assert_eq!(income_row(&r, "realized_short").as_deref(), Some("2.00"));
    assert_eq!(income_row(&r, "total").as_deref(), Some("-4.00"));
    // A transfer's network fee in coin.
    ex(
        &mut b,
        "inv.transfer_lots",
        json!({"security": "BTC", "date": "2026-03-05", "quantity": "0.01",
               "fee_quantity": "0.0001", "fee_value": "7.00", "to_account": "Assets:Exchange2"}),
    );
    let r = b.run(
        "report.investment_income",
        json!({"from": "2026-01-01", "to": "2026-03-31", "interval": "month"}),
    );
    let fees = r["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == "fees")
        .unwrap()
        .clone();
    assert_eq!(fees["amounts"], json!(["0.00", "6.00", "7.00"]), "{r}");
    let t = b.run(
        "report.tax_summary",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    let rows = t["investment_income"]["rows"].as_array().unwrap();
    let f = rows.iter().find(|x| x["key"] == "fees").unwrap();
    assert_eq!(f["label"], "Fees");
    assert_eq!(f["amount"], "13.00");
}

/// Revision 2: municipal and Treasury bond interest (and accrued interest paid, as a
/// negative within its row) have rows of their own.
#[test]
fn income_rows_split_bond_interest_by_tax() {
    let mut b = book();
    bond(&mut b, "M30", "City 3% 2030", "municipal");
    bond(&mut b, "UST", "Treasury 4% 2029", "treasury");
    act(
        &mut b,
        "inv.bond_buy",
        json!({"security": "M30", "date": "2026-03-02", "units": "1", "price": "100",
               "accrued_interest": "5.00"}),
    );
    act(
        &mut b,
        "inv.bond_interest",
        json!({"security": "M30", "date": "2026-08-15", "amount": "15.00"}),
    );
    act(
        &mut b,
        "inv.bond_interest",
        json!({"security": "UST", "date": "2026-08-15", "amount": "20.00"}),
    );
    let r = b.run(
        "report.investment_income",
        json!({"from": "2026-01-01", "to": "2026-12-31", "interval": "year"}),
    );
    assert_eq!(
        income_row(&r, "interest_tax_exempt").as_deref(),
        Some("10.00")
    );
    assert_eq!(
        income_row(&r, "interest_treasury").as_deref(),
        Some("20.00")
    );
    assert_eq!(income_row(&r, "interest").as_deref(), Some("0.00"));
    assert_eq!(income_row(&r, "crypto_rewards"), None);
    assert_eq!(income_row(&r, "total").as_deref(), Some("30.00"));
    let labels = r.to_string();
    assert!(labels.contains("Tax-exempt interest") && labels.contains("US Treasury interest"));
}

/// Revision 2: an assignment (and an expiry) can be edited in place; the contracts it
/// closed count as held for the edit, and the stock side follows.
#[test]
fn edit_an_assignment_and_an_expiry() {
    let mut b = book();
    let sym = option(&mut b, "XYZ", "put", "50", "2026-12-18");
    act(
        &mut b,
        "inv.option_write",
        json!({"security": sym, "date": "2026-02-03", "quantity": "1", "price": "2.00",
               "commission": "0.65"}),
    );
    let out = act(&mut b, "inv.option_assigned", json!({"security": sym}));
    let txn = txn_of(&out);
    b.run(
        "inv.update",
        json!({"txn": txn, "action": "option_assigned", "account": "Assets:Brokerage",
               "security": sym, "date": "2026-12-18", "commission": "5.00"}),
    );
    assert_eq!(value(&mut b, "Assets:Brokerage:XYZ"), "4805.65");
    assert_eq!(b.shown("Assets:Brokerage:XYZ", "XYZ"), "100");
    // It expired worthless after all: the premium is a short-term gain.
    b.run(
        "inv.update",
        json!({"txn": txn, "action": "option_expire", "account": "Assets:Brokerage",
               "security": sym, "date": "2026-12-18"}),
    );
    assert_eq!(b.shown("Assets:Brokerage:XYZ", "XYZ"), "0");
    assert_eq!(b.shown("Realized Gains", "USD"), "199.35");
    b.run(
        "inv.update",
        json!({"txn": txn, "action": "option_expire", "account": "Assets:Brokerage",
               "security": sym, "date": "2026-12-19"}),
    );
    assert_eq!(b.shown("Realized Gains", "USD"), "199.35");
}
