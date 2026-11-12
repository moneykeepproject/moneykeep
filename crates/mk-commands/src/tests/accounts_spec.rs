//! ledger-accounts.md acceptance tests (AT-n) and edge cases.

use super::*;

#[test]
fn at1_full_name_of_nested_account() {
    let mut b = Book::new();
    b.account(json!({"name": "Assets", "type": "asset", "placeholder": true}));
    let id = b.account(
        json!({"name": "Checking", "type": "asset", "subtype": "bank", "parent": "Assets"}),
    );
    let got = b.run("account.get", json!({"account": id}));
    assert_eq!(got["full_name"], "Assets:Checking");
    // Lookup by full name (any case) and by code also works.
    let by_name = b.run("account.get", json!({"account": "assets:checking"}));
    assert_eq!(by_name["id"], json!(id));
}

#[test]
fn at2_parent_type_mismatch_rejected() {
    let mut b = Book::new();
    b.account(json!({"name": "Assets", "type": "asset"}));
    let e = b.err(
        "account.create",
        json!({"name": "Food", "type": "expense", "parent": "Assets"}),
    );
    assert_eq!(e.code(), "invalid_args");
    assert!(e.to_string().contains("same type"), "{e}");
}

#[test]
fn at3_case_insensitive_sibling_clash() {
    let mut b = Book::new();
    b.standard();
    let e = b.err(
        "account.create",
        json!({"name": "checking", "type": "asset", "parent": "Assets"}),
    );
    assert_eq!(e.code(), "rejected");
    // Same name under a different parent is fine.
    b.account(json!({"name": "Checking", "type": "asset"}));
}

#[test]
fn at4_posting_to_placeholder_rejected() {
    let mut b = Book::new();
    b.standard();
    let e = b.err(
        "txn.create",
        json!({"date": "2026-07-01", "splits": [
            {"account": "Assets", "value": "5.00"},
            {"account": "Opening Balances", "value": "-5.00"}]}),
    );
    assert_eq!(e.code(), "rejected");
    assert!(e.to_string().contains("placeholder"), "{e}");
}

#[test]
fn at5_credit_card_displays_owed_amount() {
    let mut b = Book::new();
    b.standard();
    b.transfer(
        "2026-07-01",
        "Expenses:Groceries",
        "Liabilities:Visa",
        "200.00",
    );
    let bal = b.run("account.balance", json!({"account": "Liabilities:Visa"}));
    assert_eq!(bal["raw"]["USD"], "-200.00");
    assert_eq!(bal["display"]["USD"], "200.00");
}

#[test]
fn at6_archive_requires_zero_balance_then_blocks_posting() {
    let mut b = Book::new();
    b.standard();
    b.transfer("2026-07-01", "Assets:Checking", "Opening Balances", "10.00");
    let e = b.err("account.archive", json!({"account": "Assets:Checking"}));
    assert_eq!(e.code(), "rejected");
    b.transfer(
        "2026-07-02",
        "Expenses:Groceries",
        "Assets:Checking",
        "10.00",
    );
    b.run("account.archive", json!({"account": "Assets:Checking"}));
    let e = b.err(
        "txn.create",
        json!({"date": "2026-07-03", "splits": [
            {"account": "Expenses:Groceries", "value": "1.00"},
            {"account": "Assets:Checking", "value": "-1.00"}]}),
    );
    assert_eq!(e.code(), "rejected");
    assert!(e.to_string().contains("archived"), "{e}");
    // Hidden from default lists, shown with include_archived; still in history.
    let names = |v: &Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|a| a["full_name"].as_str().unwrap().to_owned())
            .collect()
    };
    let default = b.run("account.list", json!({}));
    assert!(!names(&default).contains(&"Assets:Checking".to_owned()));
    let all = b.run("account.list", json!({"include_archived": true}));
    assert!(names(&all).contains(&"Assets:Checking".to_owned()));
    let past = b.run(
        "account.balance",
        json!({"account": "Assets:Checking", "as_of": "2026-07-01"}),
    );
    assert_eq!(past["display"]["USD"], "10.00");
    // Unarchiving is always allowed.
    b.run("account.unarchive", json!({"account": "Assets:Checking"}));
}

#[test]
fn at7_delete_only_if_never_used() {
    let mut b = Book::new();
    b.standard();
    let t = b.transfer("2026-07-01", "Assets:Checking", "Opening Balances", "10.00");
    b.run("txn.delete", json!({"id": t}));
    // The split is gone, but the account has had one.
    let e = b.err("account.delete", json!({"account": "Assets:Checking"}));
    assert_eq!(e.code(), "rejected");
    assert!(e.to_string().contains("archive"), "{e}");
    let fresh = b.account(json!({"name": "Savings", "type": "asset", "parent": "Assets"}));
    b.run("account.delete", json!({"account": fresh}));
    assert_eq!(
        b.err("account.get", json!({"account": fresh})).code(),
        "not_found"
    );
    // An account with children cannot be deleted.
    assert_eq!(
        b.err("account.delete", json!({"account": "Expenses"}))
            .code(),
        "rejected"
    );
}

#[test]
fn at8_new_book_has_only_opening_balances() {
    let mut b = Book::new();
    let list = b.run("account.list", json!({"include_archived": true}));
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["name"], "Opening Balances");
    assert_eq!(list[0]["type"], "equity");
    assert_eq!(list[0]["subtype"], "opening-balances");
    assert_eq!(list[0]["commodity"], "USD");
    let id = list[0]["id"].clone();
    assert_eq!(
        b.run("book.info", json!({}))["opening_balances_account"],
        id
    );
    // Renamable, not deletable.
    b.run(
        "account.update",
        json!({"account": id, "name": "Starting Balances"}),
    );
    assert_eq!(
        b.err("account.delete", json!({"account": id})).code(),
        "rejected"
    );
}

#[test]
fn at9_foreign_currency_account_gated_by_module() {
    let mut b = Book::new();
    let args = json!({"name": "Euro Cash", "type": "asset", "subtype": "cash", "commodity": "EUR"});
    let e = b.err("account.create", args.clone());
    assert_eq!(e.code(), "rejected");
    assert!(e.to_string().contains("multi-currency"), "{e}");
    b.run("module.enable", json!({"module": "multi-currency"}));
    b.account(args);
}

#[test]
fn opening_balances_cannot_be_archived_while_holding_splits() {
    let mut b = Book::new();
    b.standard();
    b.transfer("2026-07-01", "Assets:Checking", "Opening Balances", "10.00");
    b.transfer("2026-07-02", "Opening Balances", "Assets:Checking", "10.00");
    let e = b.err("account.archive", json!({"account": "Opening Balances"}));
    assert!(e.to_string().contains("Opening Balances"), "{e}");
}

#[test]
fn security_accounts_hold_a_non_base_commodity() {
    let mut b = Book::new();
    let e = b.err(
        "account.create",
        json!({"name": "VTI", "type": "asset", "subtype": "security", "commodity": "USD"}),
    );
    assert_eq!(e.code(), "invalid_args");
    assert_eq!(
        b.err(
            "account.create",
            json!({"name": "VTI", "type": "asset", "subtype": "security"})
        )
        .code(),
        "invalid_args"
    );
    // Allowed without multi-currency: it is a security, not a currency.
    b.account(json!({"name": "VTI", "type": "asset", "subtype": "security", "commodity": "VTI"}));
}

#[test]
fn name_rules() {
    let mut b = Book::new();
    let id = b.account(json!({"name": "  Food  ", "type": "expense"}));
    assert_eq!(b.run("account.get", json!({"account": id}))["name"], "Food");
    for bad in ["   ", "A:B"] {
        assert_eq!(
            b.err("account.create", json!({"name": bad, "type": "expense"}))
                .code(),
            "invalid_args",
            "{bad:?}"
        );
    }
    assert_eq!(
        b.err(
            "account.create",
            json!({"name": "Rent", "type": "expense", "subtype": "bank"})
        )
        .code(),
        "invalid_args"
    );
}

#[test]
fn codes_are_unique_and_resolve() {
    let mut b = Book::new();
    let id = b.account(json!({"name": "Cash", "type": "asset", "code": "1000"}));
    assert_eq!(
        b.run("account.get", json!({"account": "1000"}))["id"],
        json!(id)
    );
    assert_eq!(
        b.err(
            "account.create",
            json!({"name": "Bank", "type": "asset", "code": "1000"})
        )
        .code(),
        "rejected"
    );
    b.run("account.update", json!({"account": id, "code": null}));
    b.account(json!({"name": "Bank", "type": "asset", "code": "1000"}));
}

#[test]
fn moving_rules() {
    let mut b = Book::new();
    b.standard();
    let sub = b.account(json!({"name": "Joint", "type": "asset", "parent": "Assets:Checking"}));
    // Under its own descendant: rejected.
    assert_eq!(
        b.err("account.move", json!({"account": "Assets", "parent": sub}))
            .code(),
        "rejected"
    );
    // Type mismatch.
    assert_eq!(
        b.err(
            "account.move",
            json!({"account": sub, "parent": "Expenses"})
        )
        .code(),
        "invalid_args"
    );
    let moved = b.run("account.move", json!({"account": sub, "parent": null}));
    assert_eq!(moved["full_name"], "Joint");
    b.account(json!({"name": "Joint", "type": "asset", "parent": "Assets"}));
    // Moving would clash with a sibling.
    assert_eq!(
        b.err(
            "account.move",
            json!({"account": "Joint", "parent": "Assets"})
        )
        .code(),
        "rejected"
    );
}

#[test]
fn placeholder_and_subtype_rules() {
    let mut b = Book::new();
    b.standard();
    // A placeholder may become a normal account at any time.
    b.run(
        "account.update",
        json!({"account": "Assets", "placeholder": false}),
    );
    b.transfer("2026-07-01", "Assets:Checking", "Opening Balances", "10.00");
    assert_eq!(
        b.err(
            "account.update",
            json!({"account": "Assets:Checking", "placeholder": true})
        )
        .code(),
        "rejected"
    );
    // Revision 3: among the money subtypes the change is allowed with splits; other
    // subtypes still change only while unused.
    assert_eq!(
        b.err(
            "account.update",
            json!({"account": "Assets:Checking", "subtype": "receivable"})
        )
        .code(),
        "rejected"
    );
    let cash = b.run(
        "account.update",
        json!({"account": "Assets:Checking", "subtype": "cash"}),
    );
    assert_eq!(cash["subtype"], "cash");
    b.run(
        "account.update",
        json!({"account": "Assets:Checking", "description": "Main account"}),
    );
}

#[test]
fn list_is_in_tree_order_with_type_filter() {
    let mut b = Book::new();
    b.standard();
    let list = b.run("account.list", json!({"type": "asset"}));
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["full_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Assets", "Assets:Checking"]);
    let all = b.run("account.list", json!({}));
    let names: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["full_name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Assets",
            "Assets:Checking",
            "Expenses",
            "Expenses:Groceries",
            "Liabilities",
            "Liabilities:Visa",
            "Opening Balances"
        ]
    );
}

// ----- Revision 3: changing a money account's kind ---------------------------------

#[test]
fn r3_1_bank_becomes_credit_card_keeping_splits() {
    let mut b = Book::new();
    b.standard();
    b.transfer("2026-07-01", "Assets:Checking", "Opening Balances", "50.00");
    b.transfer(
        "2026-07-02",
        "Expenses:Groceries",
        "Assets:Checking",
        "20.00",
    );
    let nw_before = b.run(
        "report.net_worth",
        json!({"from": "2026-07-01", "to": "2026-07-31", "interval": "month", "today": "2026-08-01"}),
    )["net_worth"]
        .clone();
    let before = b.run("account.get", json!({"account": "Assets:Checking"}));
    let acc = b.run(
        "account.update",
        json!({"account": "Assets:Checking", "type": "liability", "subtype": "credit-card"}),
    );
    assert_eq!(acc["type"], "liability");
    assert_eq!(acc["subtype"], "credit-card");
    assert_eq!(acc["id"], before["id"]);
    // Liabilities holds Visa (a credit card), so the account moves under it.
    assert_eq!(acc["full_name"], "Liabilities:Checking");
    let bal = b.run("account.balance", json!({"account": acc["id"]}));
    assert_eq!(bal["raw"]["USD"], "30.00");
    assert_eq!(bal["display"]["USD"], "-30.00");
    let reg = b.run("account.register", json!({"account": acc["id"]}));
    assert_eq!(reg["rows"].as_array().map(Vec::len), Some(2), "{reg}");
    let bs = b.run("report.balance_sheet", json!({"as_of": "2026-12-31"}));
    let in_liab = bs["liabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["full_name"] == "Liabilities:Checking");
    assert!(in_liab, "{bs}");
    let nw_after = b.run(
        "report.net_worth",
        json!({"from": "2026-07-01", "to": "2026-07-31", "interval": "month", "today": "2026-08-01"}),
    )["net_worth"]
        .clone();
    assert_eq!(nw_before, nw_after);
}

#[test]
fn r3_2_other_kinds_and_groups_cannot_change_type() {
    let mut b = Book::new();
    b.standard();
    b.account(json!({"name": "Broker", "type": "asset", "subtype": "brokerage"}));
    b.account(json!({"name": "Owed to us", "type": "asset", "subtype": "receivable"}));
    b.account(json!({"name": "Bills", "type": "liability", "subtype": "payable"}));
    for (acc, ty, sub) in [
        ("Broker", "liability", "loan"),
        ("Owed to us", "liability", "credit-card"),
        ("Bills", "asset", "bank"),
        ("Assets:Checking", "liability", "payable"),
        ("Assets:Checking", "equity", "owner-equity"),
    ] {
        let e = b.err(
            "account.update",
            json!({"account": acc, "type": ty, "subtype": sub}),
        );
        assert!(
            matches!(e.code(), "rejected" | "invalid_args"),
            "{acc}: {e}"
        );
    }
    // A placeholder with children (Assets holds Checking).
    b.run(
        "account.update",
        json!({"account": "Assets", "subtype": "bank"}),
    );
    let e = b.err(
        "account.update",
        json!({"account": "Assets", "type": "liability", "subtype": "loan"}),
    );
    assert_eq!(e.code(), "rejected");
    assert!(e.to_string().contains("type cannot change"), "{e}");
    // A type needs its subtype.
    let e = b.err(
        "account.update",
        json!({"account": "Assets:Checking", "type": "liability"}),
    );
    assert_eq!(e.code(), "invalid_args");
}

#[test]
fn r3_3_default_group_and_name_clash() {
    let mut b = Book::new();
    b.account(json!({"name": "Credit Cards", "type": "liability", "placeholder": true}));
    b.account(json!({"name": "Amex", "type": "liability", "subtype": "credit-card", "parent": "Credit Cards"}));
    b.account(json!({"name": "Loans", "type": "liability", "placeholder": true}));
    b.account(json!({"name": "Visa", "type": "asset", "subtype": "bank"}));
    b.account(json!({"name": "Card", "type": "asset", "subtype": "bank"}));
    b.transfer("2026-07-01", "Visa", "Opening Balances", "5.00");
    let acc = b.run(
        "account.update",
        json!({"account": "Visa", "type": "liability", "subtype": "credit-card"}),
    );
    assert_eq!(acc["full_name"], "Credit Cards:Visa");
    // An explicit top level wins.
    let top = b.run(
        "account.update",
        json!({"account": "Card", "type": "liability", "subtype": "credit-card", "parent": null}),
    );
    assert_eq!(top["full_name"], "Card");
    // A clash under the group is refused, naming it.
    b.account(json!({"name": "Checking", "type": "liability", "subtype": "credit-card", "parent": "Credit Cards"}));
    b.account(json!({"name": "Checking", "type": "asset", "subtype": "bank"}));
    let e = b.err(
        "account.update",
        json!({"account": "Checking", "type": "liability", "subtype": "credit-card"}),
    );
    assert_eq!(e.code(), "rejected");
    let msg = e.to_string();
    assert!(
        msg.contains("\"Checking\"") && msg.contains("Credit Cards"),
        "{msg}"
    );
    // A parent of the wrong type is refused.
    let e = b.err(
        "account.update",
        json!({"account": "Checking", "type": "liability", "subtype": "loan", "parent": "Opening Balances"}),
    );
    assert_eq!(e.code(), "invalid_args", "{e}");
    let loan = b.run(
        "account.update",
        json!({"account": "Checking", "type": "liability", "subtype": "loan", "parent": "Loans"}),
    );
    assert_eq!(loan["full_name"], "Loans:Checking");
}

#[test]
fn r3_4_undo_restores_type_subtype_and_parent() {
    let mut b = Book::new();
    b.standard();
    b.transfer("2026-07-01", "Assets:Checking", "Opening Balances", "50.00");
    let id = b.run("account.get", json!({"account": "Assets:Checking"}))["id"].clone();
    b.run(
        "account.update",
        json!({"account": "Assets:Checking", "type": "liability", "subtype": "credit-card"}),
    );
    b.run("history.undo", json!({}));
    let back = b.run("account.get", json!({"account": id}));
    assert_eq!(back["type"], "asset");
    assert_eq!(back["subtype"], "bank");
    assert_eq!(back["full_name"], "Assets:Checking");
    b.run("history.redo", json!({}));
    let again = b.run("account.get", json!({"account": id}));
    assert_eq!(again["full_name"], "Liabilities:Checking");
    let log = b.run("audit.list", json!({}));
    let txt = log.to_string();
    assert!(txt.contains("account.update"), "{txt}");
}
