//! ledger-register.md acceptance tests.

use super::*;

fn reg(b: &mut Book, args: Value) -> Value {
    b.run("account.register", args)
}

fn balances(r: &Value) -> Vec<String> {
    r["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["balance"].as_str().unwrap().to_owned())
        .collect()
}

fn checking() -> (Book, String) {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Salary", "type": "income"}));
    b.transfer(
        "2026-10-01",
        "Assets:Checking",
        "Opening Balances",
        "1000.00",
    );
    let purchase = b.transfer(
        "2026-10-05",
        "Expenses:Groceries",
        "Assets:Checking",
        "50.00",
    );
    b.transfer("2026-10-10", "Assets:Checking", "Salary", "200.00");
    (b, purchase)
}

#[test]
fn ledger_register_at1_running_balances_and_increase() {
    let (mut b, _) = checking();
    let r = reg(&mut b, json!({"account": "Assets:Checking"}));
    assert_eq!(balances(&r), ["1000.00", "950.00", "1150.00"]);
    let rows = r["rows"].as_array().unwrap();
    assert_eq!(rows[1]["amount"], "-50.00");
    assert_eq!(rows[1]["increase"], false);
    assert_eq!(rows[2]["increase"], true);
    assert_eq!(
        rows[1]["other"],
        json!({"kind": "account", "account": rows[1]["other"]["account"], "full_name": "Expenses:Groceries"})
    );
    assert!(
        rows[1]["split"]
            .as_str()
            .unwrap()
            .starts_with(rows[1]["txn"].as_str().unwrap())
    );
    assert_eq!(r["total_rows"], 3);
    assert_eq!(r["closing_balance"], "1150.00");
    assert_eq!(r["opening_balance"], "0.00");
    assert_eq!(r["account"]["full_name"], "Assets:Checking");
    assert_eq!(r["cleared_balance"], "0.00");
}

#[test]
fn ledger_register_at2_credit_card_charge_increases() {
    let mut b = Book::new();
    b.standard();
    b.transfer(
        "2026-10-02",
        "Expenses:Groceries",
        "Liabilities:Visa",
        "300.00",
    );
    let r = reg(&mut b, json!({"account": "Liabilities:Visa"}));
    assert_eq!(r["rows"][0]["increase"], true);
    assert_eq!(r["rows"][0]["amount"], "300.00");
    assert_eq!(r["rows"][0]["balance"], "300.00");
}

#[test]
fn ledger_register_at3_from_sets_opening_balance() {
    let (mut b, _) = checking();
    let r = reg(
        &mut b,
        json!({"account": "Assets:Checking", "from": "2026-10-02"}),
    );
    assert_eq!(r["opening_balance"], "1000.00");
    assert_eq!(balances(&r), ["950.00", "1150.00"]);
    let r = reg(
        &mut b,
        json!({"account": "Assets:Checking", "to": "2026-10-06"}),
    );
    assert_eq!(r["closing_balance"], "950.00");
    assert_eq!(r["total_rows"], 2);
}

#[test]
fn ledger_register_at4_paging_keeps_balances() {
    let (mut b, _) = checking();
    let full = reg(&mut b, json!({"account": "Assets:Checking"}));
    let page = reg(
        &mut b,
        json!({"account": "Assets:Checking", "offset": 1, "limit": 1}),
    );
    assert_eq!(page["rows"].as_array().unwrap().len(), 1);
    assert_eq!(page["rows"][0], full["rows"][1]);
    assert_eq!(page["total_rows"], 3);
    assert_eq!(
        b.err(
            "account.register",
            json!({"account": "Assets:Checking", "limit": 5001})
        )
        .code(),
        "invalid_args"
    );
}

#[test]
fn ledger_register_at5_voided_rows() {
    let (mut b, purchase) = checking();
    b.run("txn.void", json!({"id": purchase}));
    let r = reg(&mut b, json!({"account": "Assets:Checking"}));
    assert_eq!(balances(&r), ["1000.00", "1000.00", "1200.00"]);
    assert_eq!(r["rows"][1]["status"], "voided");
    let r = reg(
        &mut b,
        json!({"account": "Assets:Checking", "include_voided": false}),
    );
    assert_eq!(balances(&r), ["1000.00", "1200.00"]);
}

#[test]
fn ledger_register_at6_split_transaction() {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Household", "type": "expense"}));
    b.run(
        "txn.create",
        json!({"date": "2026-10-03", "splits": [
        {"account": "Expenses:Groceries", "value": "60.00"},
        {"account": "Household", "value": "40.00"},
        {"account": "Assets:Checking", "value": "-100.00"}]}),
    );
    let r = reg(&mut b, json!({"account": "Assets:Checking"}));
    assert_eq!(r["rows"][0]["other"], json!({"kind": "split", "count": 2}));
    // Two splits in the same account appear as two rows; a self-only transaction is "none".
    b.run("txn.create", json!({"date": "2026-10-04", "splits": [
        {"account": "Assets:Checking", "value": "5.00"}, {"account": "Assets:Checking", "value": "-5.00"}]}));
    let r = reg(&mut b, json!({"account": "Assets:Checking"}));
    assert_eq!(r["total_rows"], 3);
    assert_eq!(r["rows"][1]["other"], json!({"kind": "none"}));
}

#[test]
fn ledger_register_at7_desc_order() {
    let (mut b, _) = checking();
    let asc = reg(&mut b, json!({"account": "Assets:Checking"}));
    let desc = reg(
        &mut b,
        json!({"account": "Assets:Checking", "order": "desc"}),
    );
    let mut rev = asc["rows"].as_array().unwrap().clone();
    rev.reverse();
    assert_eq!(desc["rows"], Value::Array(rev));
    let first = reg(
        &mut b,
        json!({"account": "Assets:Checking", "order": "desc", "limit": 1}),
    );
    assert_eq!(first["rows"][0]["balance"], "1150.00");
}

#[test]
fn ledger_register_at8_suggestions() {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Groceries", "type": "expense"}));
    b.account(json!({"name": "Groceries Online", "type": "expense", "parent": "Expenses"}));
    b.run("txn.create", json!({"date": "2026-10-01", "payee": "KROGER #512 LIMA OH", "splits": [
        {"account": "Groceries", "value": "54.20"}, {"account": "Assets:Checking", "value": "-54.20"}]}));
    b.run("txn.create", json!({"date": "2026-10-02", "payee": "Super Kroger", "splits": [
        {"account": "Expenses:Groceries Online", "value": "9.00"}, {"account": "Assets:Checking", "value": "-9.00"}]}));
    b.run("txn.create", json!({"date": "2026-10-03", "payee": "Bakroom", "splits": [
        {"account": "Expenses:Groceries", "value": "1.00"}, {"account": "Assets:Checking", "value": "-1.00"}]}));
    let s = b.run(
        "payee.suggest",
        json!({"prefix": "kro", "account": "Assets:Checking"}),
    );
    let names: Vec<&str> = s
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["payee"].as_str().unwrap())
        .collect();
    // Most recent first; "Super Kroger" matches at a word start; "Bakroom" does not.
    assert_eq!(names, ["Super Kroger", "KROGER #512 LIMA OH"]);
    assert_eq!(s[1]["other"]["full_name"], "Groceries");
    assert_eq!(s[1]["amount"], "-54.20", "from the checking account's side");
    let a = b.run("account.suggest", json!({"prefix": "gro"}));
    let names: Vec<&str> = a
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["full_name"].as_str().unwrap())
        .collect();
    // Every account with a name or segment starting "gro" is offered.
    assert_eq!(names.len(), 3);
    assert!(names.contains(&"Groceries") && names.contains(&"Expenses:Groceries Online"));
    // When "Groceries" is an exact name match it comes before "Expenses:Groceries
    // Online", even though the latter was used more recently. (Exact matches, then
    // most recently used, then full name.)
    let ex = b.run("account.suggest", json!({"prefix": "groceries"}));
    let names: Vec<&str> = ex
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["full_name"].as_str().unwrap())
        .collect();
    let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
    assert!(
        pos("Groceries") < pos("Expenses:Groceries Online"),
        "{names:?}"
    );
    assert!(
        pos("Expenses:Groceries") < pos("Expenses:Groceries Online"),
        "{names:?}"
    );
    // Placeholders and type filters.
    let e = b.run("account.suggest", json!({"prefix": "", "types": ["asset"]}));
    let names: Vec<&str> = e
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["full_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Assets:Checking"]);
}

/// Revision 2: with `account`, payees from other accounts fill the remaining slots.
#[test]
fn ledger_register_payee_suggest_falls_back_to_other_accounts() {
    let mut b = Book::new();
    b.standard();
    b.run("txn.create", json!({"date": "2026-10-01", "payee": "Corner Cafe", "splits": [
        {"account": "Expenses:Groceries", "value": "4.50"}, {"account": "Assets:Checking", "value": "-4.50"}]}));
    // Costco used only on the card.
    b.run("txn.create", json!({"date": "2026-10-03", "payee": "Costco", "splits": [
        {"account": "Expenses:Groceries", "value": "80.00"}, {"account": "Liabilities:Visa", "value": "-80.00"}]}));
    let s = b.run(
        "payee.suggest",
        json!({"prefix": "co", "account": "Assets:Checking"}),
    );
    let names: Vec<&str> = s
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["payee"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Corner Cafe", "Costco"],
        "own account's payees first"
    );
    assert_eq!(s[0]["from_account"], true);
    assert_eq!(s[1]["from_account"], false);
    assert_eq!(s[1]["other"]["full_name"], "Expenses:Groceries");
    assert_eq!(s[1]["amount"], "-80.00", "pre-filled as money out");
    // The limit still applies across both passes.
    let one = b.run(
        "payee.suggest",
        json!({"prefix": "co", "account": "Assets:Checking", "limit": 1}),
    );
    assert_eq!(one.as_array().unwrap().len(), 1);
}

#[test]
fn ledger_register_at9_mixed_commodity_children_rejected() {
    let mut b = Book::new();
    b.account(
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "placeholder": true}),
    );
    b.account(json!({"name": "Cash", "type": "asset", "parent": "Brokerage"}));
    b.account(json!({"name": "VTI", "type": "asset", "subtype": "security", "commodity": "VTI", "parent": "Brokerage"}));
    let e = b.err(
        "account.register",
        json!({"account": "Brokerage", "include_children": true}),
    );
    assert_eq!(e.code(), "rejected");
    // An account with no activity: empty rows and zero balances.
    let r = reg(&mut b, json!({"account": "Brokerage:Cash"}));
    assert_eq!(r["rows"], json!([]));
    assert_eq!(r["closing_balance"], "0.00");
    assert_eq!(r["reconciled_balance"], "0.00");
}

/// Performance (spec edge case): a 200-row page from a 100,000-row register in well
/// under a second. Release build only; run with
/// `cargo test --release -p mk-commands -- --ignored ledger_register_perf`.
#[test]
#[ignore = "release-build performance test; not run in CI debug builds"]
fn ledger_register_perf_100k_rows() {
    let mut b = Book::new();
    b.standard();
    b.s.write(|tx| -> CommandResult<()> {
        let a = Actor("perf".into());
        let ctx = Ctx::new(tx, &a);
        let chart = crate::accounts::Chart::load(&ctx)?;
        let chk = chart.resolve("Assets:Checking")?.id;
        let gro = chart.resolve("Expenses:Groceries")?.id;
        for i in 0..100_000u32 {
            let v = mk_core::Decimal::new(i64::from(i % 997) + 1, 2);
            let t = mk_core::Transaction {
                id: mk_core::TransactionId::new(),
                date: "2026-01-01"
                    .parse::<mk_core::Date>()?
                    .add_days(i32::try_from(i / 300).unwrap_or(0))
                    .unwrap(),
                currency: CommodityCode::new("USD")?,
                payee: Some(format!("Payee {}", i % 50)),
                number: None,
                memo: None,
                status: mk_core::TxnStatus::Posted,
                void_reason: None,
                reverses: None,
                opening: false,
                entered_at: format!("{i:010}"),
                splits: vec![
                    mk_core::Split {
                        account: gro,
                        value: v,
                        quantity: v,
                        memo: None,
                        reconcile: mk_core::Reconcile::New,
                        fingerprint: None,
                        tags: Default::default(),
                    },
                    mk_core::Split {
                        account: chk,
                        value: -v,
                        quantity: -v,
                        memo: None,
                        reconcile: mk_core::Reconcile::New,
                        fingerprint: None,
                        tags: Default::default(),
                    },
                ],
            };
            tx.put_transaction(&t)?;
        }
        Ok(())
    })
    .unwrap();
    let start = std::time::Instant::now();
    let r = reg(
        &mut b,
        json!({"account": "Assets:Checking", "offset": 50_000, "limit": 200}),
    );
    let took = start.elapsed();
    assert_eq!(r["rows"].as_array().unwrap().len(), 200);
    assert_eq!(r["total_rows"], 100_000);
    assert!(took < std::time::Duration::from_secs(1), "took {took:?}");
}
