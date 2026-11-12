//! subscriptions.md acceptance tests SA1–SA11 at the command level, plus the edge
//! cases the spec names (one subscription per transaction, add by hand, settings,
//! the dashboard block and undo).

use super::*;

/// A book with the subscriptions module on, a checking account, a card and some
/// expense accounts.
fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "subscriptions"}));
    for n in ["Streaming", "Utilities", "Software", "Music"] {
        b.account(json!({"name": n, "type": "expense", "parent": "Expenses"}));
    }
    b
}

/// A payment of `amount` to `payee` from `account` on `date`.
fn pay(b: &mut Book, date: &str, payee: &str, amount: &str, account: &str, cat: &str) -> String {
    let neg = format!("-{amount}");
    let account = full(account);
    let cat = format!("Expenses:{cat}");
    b.run(
        "txn.create",
        json!({"date": date, "payee": payee, "splits": [
            {"account": cat, "value": amount},
            {"account": account, "value": neg},
        ]}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The standard chart's full names.
fn full(account: &str) -> String {
    match account {
        "Checking" => "Assets:Checking".into(),
        "Visa" => "Liabilities:Visa".into(),
        x => x.to_owned(),
    }
}

fn suggestions(b: &mut Book) -> Vec<Value> {
    b.run("subscription.suggestions", json!({}))
        .as_array()
        .unwrap()
        .clone()
}

fn suggestion(b: &mut Book, payee: &str) -> Option<Value> {
    suggestions(b)
        .into_iter()
        .find(|s| s["payee"].as_str() == Some(payee))
}

fn confirm(b: &mut Book, payee: &str, answer: &str) -> String {
    let s = suggestion(b, payee).unwrap_or_else(|| panic!("{payee} not suggested"));
    b.run(
        "subscription.answer",
        json!({"key": s["key"], "answer": answer, "today": "2026-11-01"}),
    )["subscription"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn alerts(b: &mut Book, today: &str) -> Vec<Value> {
    b.run("subscription.alerts", json!({"today": today}))["alerts"]
        .as_array()
        .unwrap()
        .clone()
}

/// A monthly subscription charged on the 5th with these amounts, from January 2026.
fn monthly(b: &mut Book, payee: &str, amounts: &[&str]) -> String {
    for (i, a) in amounts.iter().enumerate() {
        pay(
            b,
            &format!("2026-{:02}-05", i + 1),
            payee,
            a,
            "Checking",
            "Streaming",
        );
    }
    confirm(b, payee, "subscription")
}

#[test]
fn subscriptions_sa1_netflix_price_rise() {
    let mut b = book();
    for m in 3..=6 {
        pay(
            &mut b,
            &format!("2026-{m:02}-05"),
            "Netflix",
            "15.49",
            "Checking",
            "Streaming",
        );
    }
    for m in 7..=8 {
        pay(
            &mut b,
            &format!("2026-{m:02}-05"),
            "Netflix",
            "17.99",
            "Checking",
            "Streaming",
        );
    }
    let s = suggestion(&mut b, "Netflix").unwrap();
    assert_eq!(s["cadence"], "monthly");
    assert_eq!(s["kind"], "subscription");
    assert_eq!(s["charges"], 6);
    assert_eq!(s["last_amount"], "17.99");
    assert_eq!(s["first_date"], "2026-03-05");
    assert_eq!(s["account_name"], "Assets:Checking");
    let id = confirm(&mut b, "Netflix", "subscription");
    assert!(suggestion(&mut b, "Netflix").is_none());
    let g = b.run("subscription.get", json!({"subscription": id}));
    assert_eq!(g["price"], "17.99");
    assert_eq!(g["category_name"], "Expenses:Streaming");
    assert_eq!(g["next_charge"], "2026-09-05");
    let h = g["history"].as_array().unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0]["change"], "2.50");
    assert_eq!(h[0]["percent_text"], "+16.1%");
    assert_eq!(h[0]["color"], Value::Null);
    assert_eq!(h[0]["small"], false);
    // An Attention item asks for a reason.
    let a = alerts(&mut b, "2026-08-10");
    assert_eq!(a.len(), 1);
    assert_eq!(
        a[0]["label"],
        "Netflix: 15.49 → 17.99 (+16.1%). Add a reason?"
    );
    assert_eq!(a[0]["target"]["reason"], h[0]["key"]);
    assert!(a[0].get("chip").is_none());
    let att = b.run(
        "dashboard.block_data",
        json!({"dashboard": "home", "block": "attention", "today": "2026-08-10"}),
    );
    assert!(
        att["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["kind"] == "price_rise"
                && i["dismiss"]["command"] == "subscription.dismiss_alert")
    );
    // A reason shows in the history and answers the alert.
    b.run(
        "subscription.set_reason",
        json!({"subscription": id, "row": h[0]["key"], "reason": "Standard plan went up"}),
    );
    let g = b.run("subscription.get", json!({"subscription": id}));
    assert_eq!(g["history"][0]["reason"], "Standard plan went up");
    assert!(alerts(&mut b, "2026-08-10").is_empty());
    // Undo brings the alert back.
    b.run("history.undo", json!({}));
    assert_eq!(alerts(&mut b, "2026-08-10").len(), 1);
}

#[test]
fn subscriptions_sa2_band_colours() {
    let mut b = book();
    let a = monthly(&mut b, "Gym", &["40.00", "40.00", "50.00"]);
    let c = monthly(&mut b, "Cloud", &["20.00", "20.00", "29.00"]);
    let colour = |b: &mut Book, id: &str| {
        b.run("subscription.get", json!({"subscription": id}))["history"][0].clone()
    };
    let g = colour(&mut b, &a);
    assert_eq!(g["percent_text"], "+25.0%");
    assert_eq!(g["color"], "#F2C94C");
    assert_eq!(g["dark_text"], true);
    let g = colour(&mut b, &c);
    assert_eq!(g["percent_text"], "+45.0%");
    assert_eq!(g["color"], "#D64545");
    // The alert chip carries the band colour, the arrow and the percentage.
    let list = alerts(&mut b, "2026-03-10");
    let gym = list
        .iter()
        .find(|x| x["subscription"] == a.as_str())
        .unwrap();
    assert_eq!(gym["chip"]["color"], "#F2C94C");
    assert_eq!(gym["chip"]["text"], "↑ 25.0%");
    // Bands edited to 10 % green and 30 % purple.
    b.run(
        "module.set_setting",
        json!({"key": "subscriptions.bands", "value": [
            {"percent": "30", "color": "#8e44ad"}, {"percent": 10, "color": "#27AE60"}]}),
    );
    let s = b.run("module.settings", json!({"module": "subscriptions"}));
    let bands = s["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == "subscriptions.bands")
        .unwrap()["value"]
        .clone();
    assert_eq!(
        bands,
        json!([{"percent": "10", "color": "#27AE60"}, {"percent": "30", "color": "#8E44AD"}])
    );
    assert_eq!(colour(&mut b, &a)["color"], "#27AE60");
    let p = colour(&mut b, &c);
    assert_eq!(p["color"], "#8E44AD");
    // Purple takes light text for 4.5:1.
    assert_eq!(p["dark_text"], false);
    // Zero bands: no colour.
    b.run(
        "module.set_setting",
        json!({"key": "subscriptions.bands", "value": []}),
    );
    assert_eq!(colour(&mut b, &c)["color"], Value::Null);
    // Bad bands are refused.
    for bad in [
        json!([{"percent": "x", "color": "#fff"}]),
        json!([{"percent": "20", "color": "yellow"}]),
        json!([{"percent": "20", "color": "#fff"}, {"percent": "20", "color": "#000"}]),
    ] {
        let e = b.err(
            "module.set_setting",
            json!({"key": "subscriptions.bands", "value": bad}),
        );
        assert_eq!(e.code(), "invalid_args");
    }
    // A decrease is not banded.
    let d = monthly(&mut b, "Phone plan", &["60.00", "60.00", "30.00"]);
    let r = colour(&mut b, &d);
    assert_eq!(r["increase"], false);
    assert_eq!(r["color"], Value::Null);
}

#[test]
fn subscriptions_sa3_small_changes() {
    let mut b = book();
    let up = monthly(&mut b, "Music Plus", &["15.49", "15.49", "15.79"]);
    let tiny = monthly(&mut b, "News Daily", &["15.49", "15.49", "15.59"]);
    let h = |b: &mut Book, id: &str| {
        b.run("subscription.get", json!({"subscription": id}))["history"][0].clone()
    };
    assert_eq!(h(&mut b, &up)["small"], false);
    assert_eq!(h(&mut b, &tiny)["small"], true);
    let list = alerts(&mut b, "2026-03-10");
    assert!(list.iter().any(|x| x["subscription"] == up.as_str()));
    assert!(!list.iter().any(|x| x["subscription"] == tiny.as_str()));
}

#[test]
fn subscriptions_sa4_recurring_bill() {
    let mut b = book();
    for (d, a) in [
        ("2026-08-12", "82.10"),
        ("2026-09-12", "95.40"),
        ("2026-10-12", "120.55"),
    ] {
        pay(&mut b, d, "City Electric", a, "Checking", "Utilities");
    }
    let s = suggestion(&mut b, "City Electric").unwrap();
    assert_eq!(s["kind"], "bill");
    assert_eq!(s["cadence"], "monthly");
    let id = confirm(&mut b, "City Electric", "bill");
    let l = b.run("subscription.list", json!({"today": "2026-10-20"}));
    assert!(l["subscriptions"].as_array().unwrap().is_empty());
    let bills = l["bills"].as_array().unwrap();
    assert_eq!(bills.len(), 1);
    assert_eq!(bills[0]["id"], id.as_str());
    assert_eq!(bills[0]["last_amount"], "120.55");
    assert_eq!(bills[0]["average"], "99.35");
    assert_eq!(bills[0]["year_total"], "298.05");
    assert_eq!(bills[0]["amounts"].as_array().unwrap().len(), 3);
    let g = b.run("subscription.get", json!({"subscription": id}));
    assert!(g["history"].as_array().unwrap().is_empty());
    assert!(alerts(&mut b, "2026-10-20").is_empty());
}

#[test]
fn subscriptions_sa5_yearly_renewal() {
    let mut b = book();
    pay(
        &mut b,
        "2024-12-02",
        "Amazon Prime",
        "139.00",
        "Visa",
        "Streaming",
    );
    pay(
        &mut b,
        "2025-12-02",
        "Amazon Prime",
        "139.00",
        "Visa",
        "Streaming",
    );
    let s = suggestion(&mut b, "Amazon Prime").unwrap();
    assert_eq!(s["cadence"], "yearly");
    assert_eq!(s["kind"], "subscription");
    confirm(&mut b, "Amazon Prime", "subscription");
    assert!(alerts(&mut b, "2026-11-17").is_empty());
    let a = alerts(&mut b, "2026-11-18");
    assert_eq!(a.len(), 1);
    assert_eq!(a[0]["kind"], "renewal");
    assert_eq!(a[0]["label"], "Amazon Prime renews on Dec 2 for 139.00.");
    // Dismissible like other notices, and undoable.
    b.run("subscription.dismiss_alert", json!({"key": a[0]["key"]}));
    assert!(alerts(&mut b, "2026-11-18").is_empty());
    b.run("history.undo", json!({}));
    assert_eq!(alerts(&mut b, "2026-11-18").len(), 1);
}

#[test]
fn subscriptions_sa6_charged_after_cancel_first() {
    let mut b = book();
    let hulu = monthly(&mut b, "Hulu", &["7.99"; 9]);
    b.run(
        "subscription.set_status",
        json!({"subscription": hulu, "status": "cancelled", "on": "2026-10-20"}),
    );
    pay(
        &mut b,
        "2026-11-03",
        "Hulu",
        "7.99",
        "Checking",
        "Streaming",
    );
    // A price rise elsewhere, so there is something to come after.
    monthly(&mut b, "Spotify", &["9.99", "9.99", "11.99"]);
    let a = alerts(&mut b, "2026-11-05");
    assert_eq!(a[0]["kind"], "charged_after_cancel");
    assert_eq!(
        a[0]["label"],
        "Hulu charged 7.99 on Nov 3 after you cancelled it on Oct 20."
    );
    assert_eq!(a[0]["chip"]["color"], "#D64545");
    // Listed first on Home too.
    let att = b.run(
        "dashboard.block_data",
        json!({"dashboard": "home", "block": "attention", "today": "2026-11-05"}),
    );
    let items = att["items"].as_array().unwrap();
    assert_eq!(items[0]["kind"], "charged_after_cancel");
    assert!(items.len() >= 3);
}

#[test]
fn subscriptions_sa7_missing_charge() {
    let mut b = book();
    let id = monthly(&mut b, "Spotify", &["10.99"; 10]);
    let g = b.run("subscription.get", json!({"subscription": id}));
    assert_eq!(g["next_charge"], "2026-11-05");
    assert!(alerts(&mut b, "2026-11-09").is_empty());
    let a = alerts(&mut b, "2026-11-10");
    assert_eq!(a.len(), 1);
    assert_eq!(
        a[0]["label"],
        "Spotify's charge due Nov 5 hasn't appeared. Cancelled, or did the card change?"
    );
    let acts = a[0]["actions"].as_array().unwrap();
    assert_eq!(acts[0]["label"], "Mark cancelled");
    assert_eq!(acts[1]["label"], "Still active");
    b.run(
        acts[0]["command"].as_str().unwrap(),
        acts[0]["args"].clone(),
    );
    let g = b.run("subscription.get", json!({"subscription": id}));
    assert_eq!(g["status"], "cancelled");
    assert_eq!(g["cancelled_on"], "2026-11-05");
    assert!(alerts(&mut b, "2026-11-10").is_empty());
    // Still active moves the expected charge on one period.
    b.run("history.undo", json!({}));
    b.run(
        acts[1]["command"].as_str().unwrap(),
        acts[1]["args"].clone(),
    );
    let g = b.run("subscription.get", json!({"subscription": id}));
    assert_eq!(g["next_charge"], "2026-12-05");
    assert!(alerts(&mut b, "2026-11-10").is_empty());
}

#[test]
fn subscriptions_sa8_not_recurring_and_undo() {
    let mut b = book();
    for m in 1..=3 {
        pay(
            &mut b,
            &format!("2026-{m:02}-15"),
            "Corner Bakery",
            "12.00",
            "Visa",
            "Groceries",
        );
    }
    let s = suggestion(&mut b, "Corner Bakery").unwrap();
    b.run(
        "subscription.answer",
        json!({"key": s["key"], "answer": "not_recurring"}),
    );
    assert!(suggestion(&mut b, "Corner Bakery").is_none());
    // More charges do not bring it back.
    pay(
        &mut b,
        "2026-04-15",
        "Corner Bakery",
        "12.00",
        "Visa",
        "Groceries",
    );
    assert!(suggestion(&mut b, "Corner Bakery").is_none());
    let d = b.run("subscription.dismissed", json!({}));
    assert_eq!(d.as_array().unwrap().len(), 1);
    assert_eq!(d[0]["payee"], "Corner Bakery");
    // Undo (from Dismissed suggestions) brings it back.
    b.run(
        "subscription.restore_suggestion",
        json!({"key": d[0]["key"]}),
    );
    assert!(suggestion(&mut b, "Corner Bakery").is_some());
    // The answer by payee and account, and history undo, work too.
    b.run(
        "subscription.answer",
        json!({"payee": "corner bakery", "account": "Liabilities:Visa", "answer": "not_recurring"}),
    );
    assert!(suggestion(&mut b, "Corner Bakery").is_none());
    b.run("history.undo", json!({}));
    assert!(suggestion(&mut b, "Corner Bakery").is_some());
}

#[test]
fn subscriptions_sa9_review() {
    let mut b = book();
    let cheap = monthly(&mut b, "Peacock", &["9.99", "9.99", "9.99"]);
    let big = monthly(&mut b, "Adobe", &["22.99", "22.99", "22.99"]);
    let keep = monthly(&mut b, "iCloud", &["2.99", "2.99", "2.99"]);
    b.run(
        "subscription.update",
        json!({"subscription": keep, "reviewed_on": "2026-02-01"}),
    );
    let q = b.run("subscription.review", json!({"today": "2026-03-20"}));
    let order: Vec<&str> = q["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(order, ["Adobe", "Peacock", "iCloud"]);
    let r = b.run(
        "subscription.review_answer",
        json!({"subscription": cheap, "answer": "cancel", "today": "2026-03-20"}),
    );
    assert_eq!(r["saved_year"], "119.88");
    let r = b.run(
        "subscription.review_answer",
        json!({"subscription": big, "answer": "downgrade", "price": "11.99", "today": "2026-03-20"}),
    );
    assert_eq!(r["saved_year"], "251.88");
    b.run(
        "subscription.review_answer",
        json!({"subscription": keep, "answer": "keep", "today": "2026-03-20"}),
    );
    let g = b.run("subscription.get", json!({"subscription": big}));
    assert_eq!(g["price"], "11.99");
    assert_eq!(g["history"][0]["reason"], "Downgraded");
    assert_eq!(g["history"][0]["new"], "11.99");
    let g = b.run("subscription.get", json!({"subscription": cheap}));
    assert_eq!(g["status"], "cancelled");
    let l = b.run("subscription.list", json!({"today": "2026-03-21"}));
    assert_eq!(l["saved_year"], "251.88");
    // The next charge at the new price takes the review's row (one row, its reason).
    pay(
        &mut b,
        "2026-04-05",
        "Adobe",
        "11.99",
        "Checking",
        "Streaming",
    );
    let g = b.run("subscription.get", json!({"subscription": big}));
    let h = g["history"].as_array().unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0]["reason"], "Downgraded");
    assert_eq!(h[0]["manual"], false);
    // A downgrade must lower the price.
    let e = b.err(
        "subscription.review_answer",
        json!({"subscription": keep, "answer": "downgrade", "price": "5.00"}),
    );
    assert_eq!(e.code(), "invalid_args");
}

#[test]
fn subscriptions_sa10_monthly_equivalent_and_totals() {
    let mut b = book();
    b.run(
        "subscription.create",
        json!({"name": "Prime", "payee": "Amazon Prime", "cadence": "yearly", "price": "139.00",
               "next_charge": "2026-12-02", "today": "2026-11-01"}),
    );
    b.run(
        "subscription.create",
        json!({"name": "Paper", "payee": "Daily Paper", "cadence": "weekly", "price": "5.00",
               "today": "2026-11-01"}),
    );
    let l = b.run("subscription.list", json!({"today": "2026-11-01"}));
    let rows = l["subscriptions"].as_array().unwrap();
    let m = |name: &str| {
        rows.iter().find(|r| r["name"] == name).unwrap()["monthly"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(m("Prime"), "11.58");
    assert_eq!(m("Paper"), "21.67");
    assert_eq!(l["totals"]["monthly"], "33.25");
    assert_eq!(l["totals"]["yearly"], "399.00");
    assert_eq!(l["totals"]["year_ago_monthly"], "0.00");
}

#[test]
fn subscriptions_sa11_weekly_and_irregular() {
    let mut b = book();
    for d in ["2026-10-05", "2026-10-12", "2026-10-19"] {
        pay(&mut b, d, "Car Wash Club", "5.00", "Checking", "Groceries");
    }
    let s = suggestion(&mut b, "Car Wash Club").unwrap();
    assert_eq!(s["cadence"], "weekly");
    // 9, 40 and 75 days apart.
    let mut d: mk_core::Date = "2026-01-01".parse().unwrap();
    pay(
        &mut b,
        &d.to_string(),
        "Hardware Store",
        "20.00",
        "Checking",
        "Groceries",
    );
    for gap in [9, 40, 75] {
        d = d.add_days(gap).unwrap();
        pay(
            &mut b,
            &d.to_string(),
            "Hardware Store",
            "20.00",
            "Checking",
            "Groceries",
        );
    }
    assert!(suggestion(&mut b, "Hardware Store").is_none());
}

#[test]
fn subscriptions_charges_and_matching() {
    let mut b = book();
    let id = monthly(&mut b, "Netflix", &["15.49", "15.49", "15.49"]);
    // A transfer to the card is not a payment.
    let transfer = b.transfer(
        "2026-04-01",
        "Liabilities:Visa",
        "Assets:Checking",
        "100.00",
    );
    // One subscription per transaction: a second record matching the same payee
    // does not take the first one's charges.
    let other = b.run(
        "subscription.create",
        json!({"name": "Netflix again", "payee": "netf", "how": "contains", "cadence": "monthly",
               "price": "15.49", "today": "2026-11-02"}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let n = |b: &mut Book, id: &str| {
        b.run("subscription.get", json!({"subscription": id}))["charges"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(n(&mut b, &id), 3);
    assert_eq!(n(&mut b, &other), 0);
    // Remove a wrong one; add a missed one (it moves).
    let g = b.run("subscription.get", json!({"subscription": id}));
    let first = g["charge_list"][0]["txn"].as_str().unwrap().to_owned();
    b.run(
        "subscription.remove_charge",
        json!({"subscription": id, "txn": first}),
    );
    assert_eq!(n(&mut b, &id), 2);
    assert_eq!(n(&mut b, &other), 1);
    let missed = pay(
        &mut b,
        "2026-04-06",
        "NFLX DIGITAL",
        "15.49",
        "Checking",
        "Streaming",
    );
    b.run(
        "subscription.add_charge",
        json!({"subscription": id, "txn": missed}),
    );
    assert_eq!(n(&mut b, &id), 3);
    b.run(
        "subscription.add_charge",
        json!({"subscription": other, "txn": missed}),
    );
    assert_eq!(n(&mut b, &id), 2);
    // Only payments can be linked.
    let e = b.err(
        "subscription.add_charge",
        json!({"subscription": id, "txn": transfer}),
    );
    assert_eq!(e.code(), "rejected");
    // Delete, and the suggestion comes back.
    b.run("subscription.delete", json!({"subscription": other}));
    b.run("subscription.delete", json!({"subscription": id}));
    assert!(suggestion(&mut b, "Netflix").is_some());
}

#[test]
fn subscriptions_trial_duplicate_and_block() {
    let mut b = book();
    b.run(
        "subscription.create",
        json!({"name": "Music Trial", "payee": "Tidal", "cadence": "monthly", "price": "10.99",
               "status": "trial", "trial_ends": "2026-11-10", "link": "https://example.com/cancel",
               "today": "2026-11-01"}),
    );
    let a = alerts(&mut b, "2026-11-07");
    assert_eq!(a.len(), 1);
    assert_eq!(a[0]["kind"], "trial");
    assert!(alerts(&mut b, "2026-11-06").is_empty());
    // The same payee on two accounts.
    b.run(
        "subscription.create",
        json!({"name": "Disney", "payee": "Disney Plus", "account": "Assets:Checking", "cadence": "monthly",
               "price": "7.99", "next_charge": "2026-11-20", "today": "2026-11-01"}),
    );
    b.run(
        "subscription.create",
        json!({"name": "Disney 2", "payee": "Disney Plus", "account": "Liabilities:Visa", "cadence": "monthly",
               "price": "7.99", "next_charge": "2026-11-21", "today": "2026-11-01"}),
    );
    let a = alerts(&mut b, "2026-11-07");
    assert!(a.iter().any(|x| x["kind"] == "duplicate"));
    // Bad links and kinds are refused.
    let e = b.err(
        "subscription.create",
        json!({"payee": "X", "cadence": "monthly", "price": "1", "link": "javascript:alert(1)"}),
    );
    assert_eq!(e.code(), "invalid_args");
    // The dashboard block: not on Home by default; added from the gallery.
    let home = b.run("dashboard.get", json!({"dashboard": "home"}));
    assert!(
        !home["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["type"] == "subscriptions")
    );
    let blk = b.run(
        "dashboard.add_block",
        json!({"dashboard": "home", "type": "subscriptions"}),
    );
    assert_eq!(blk["type"], "subscriptions");
    let bid = blk["id"].clone();
    let d = b.run(
        "dashboard.block_data",
        json!({"dashboard": "home", "block": bid, "today": "2026-11-07"}),
    );
    assert_eq!(d["status"], "ok");
    assert_eq!(d["monthly"], "15.98");
    assert_eq!(d["next"].as_array().unwrap().len(), 2);
    assert_eq!(d["next"][0]["name"], "Disney");
    assert_eq!(d["alerts"], 2);
    // Module off: the block says so, the alerts leave Home.
    b.run("module.disable", json!({"module": "subscriptions"}));
    let d = b.run(
        "dashboard.block_data",
        json!({"dashboard": "home", "block": bid, "today": "2026-11-07"}),
    );
    assert_eq!(d["status"], "module_off");
}

#[test]
fn subscriptions_off_by_default() {
    let mut b = Book::new();
    let e = b.err("subscription.list", json!({}));
    assert_eq!(e.code(), "module_disabled");
    let m = b.run("module.list", json!({}));
    let s = m
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "subscriptions")
        .unwrap()
        .clone();
    assert_eq!(s["enabled"], false);
}

// ----- revision 2 ---------------------------------------------------------------------

#[test]
fn subscriptions_sa12_single_step_yearly() {
    let mut b = book();
    pay(
        &mut b,
        "2025-03-01",
        "Club Pass",
        "139.00",
        "Visa",
        "Software",
    );
    pay(
        &mut b,
        "2026-03-01",
        "Club Pass",
        "149.00",
        "Visa",
        "Software",
    );
    let s = suggestion(&mut b, "Club Pass").unwrap();
    assert_eq!(s["cadence"], "yearly");
    assert_eq!(s["kind"], "subscription");
    let id = confirm(&mut b, "Club Pass", "subscription");
    let g = b.run("subscription.get", json!({"subscription": id}));
    assert_eq!(g["price"], "149.00");
    let h = g["history"].as_array().unwrap();
    assert_eq!(h.len(), 1);
    assert_eq!(h[0]["change"], "10.00");
    assert_eq!(h[0]["percent_text"], "+7.2%");
}

/// Netflix as in SA1 (17.99 since July), confirmed as a subscription.
fn netflix(b: &mut Book) -> String {
    for (m, a) in [
        (3, "15.49"),
        (4, "15.49"),
        (5, "15.49"),
        (6, "15.49"),
        (7, "17.99"),
        (8, "17.99"),
    ] {
        pay(
            b,
            &format!("2026-{m:02}-05"),
            "Netflix",
            a,
            "Checking",
            "Streaming",
        );
    }
    confirm(b, "Netflix", "subscription")
}

#[test]
fn subscriptions_sa13_make_bill_scheduled() {
    let mut b = book();
    let id = netflix(&mut b);
    // Without the bills module the action is refused.
    assert_eq!(
        b.err("subscription.make_bill", json!({"subscription": id}))
            .code(),
        "rejected"
    );
    b.run("module.enable", json!({"module": "bills"}));
    let s = b.run("module.settings", json!({"module": "subscriptions"}));
    let mb = s["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["key"] == "subscriptions.make_bill_as")
        .unwrap()
        .clone();
    assert_eq!(mb["value"], "scheduled_bill");
    let txns = b.txn_count();
    let r = b.run("subscription.make_bill", json!({"subscription": id}));
    assert_eq!(r["mode"], "scheduled_bill");
    let sch = r["schedule"].clone();
    assert_eq!(sch["creates"], "bill");
    assert_eq!(sch["auto_enter"], false);
    assert_eq!(sch["recurrence"]["frequency"], "monthly");
    assert_eq!(sch["amount"], "-17.99");
    assert_eq!(sch["bill_view"]["total"], "17.99");
    assert_eq!(sch["bill_view"]["vendor_name"], "Netflix");
    assert_eq!(sch["next_due"]["nominal"], "2026-09-05");
    // Nothing posted, no bill.
    assert_eq!(b.txn_count(), txns);
    assert!(b.run("bill.list", json!({})).as_array().unwrap().is_empty());
    // It is listed in Scheduled marked as a bill.
    let up = b.run(
        "schedule.upcoming",
        json!({"today": "2026-09-01", "to": "2026-09-30"}),
    );
    let occ = up["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["schedule"] == sch["id"])
        .unwrap()
        .clone();
    assert_eq!(occ["creates"], "bill");
    assert_eq!(occ["amount"], "-17.99");
    assert_eq!(occ["money"]["category"], "Expenses:Streaming");
    // Entering: the editor gets the bill arguments; saving goes through bill.create
    // with from_schedule (an estimate corrected to 18.49).
    let args = json!({
        "vendor": sch["bill_view"]["vendor"],
        "date": "2026-09-05",
        "lines": [{"account": "Expenses:Streaming", "description": "Netflix", "amount": "18.49"}],
        "from_schedule": {"schedule": sch["id"], "date": "2026-09-05"},
    });
    let bill = b.run("bill.create", args.clone());
    assert_eq!(bill["total"], "18.49");
    assert_eq!(b.txn_count(), txns + 1);
    let g = b.run("schedule.get", json!({"schedule": sch["id"]}));
    assert_eq!(g["entered"], 1);
    assert_eq!(g["next_due"]["nominal"], "2026-10-05");
    // The same occurrence cannot be entered twice.
    assert_eq!(b.err("bill.create", args.clone()).code(), "conflict");
    // Deleting the bill makes the occurrence pending again.
    b.run("bill.delete", json!({"bill": bill["id"]}));
    let g = b.run("schedule.get", json!({"schedule": sch["id"]}));
    assert_eq!(g["entered"], 0);
    // Entering directly (CLI) creates the bill as scheduled; Skip works as usual.
    let e = b.run("schedule.enter", json!({"schedule": sch["id"]}));
    assert_eq!(e["total"], "17.99");
    assert_eq!(e["occurrence"], "2026-09-05");
    b.run("schedule.skip", json!({"schedule": sch["id"]}));
    let g = b.run("schedule.get", json!({"schedule": sch["id"]}));
    assert_eq!(g["skipped"], 1);
    assert_eq!(g["next_due"]["nominal"], "2026-11-05");
    // A bill schedule is never auto-entered.
    let r = b.run("schedule.run_due", json!({"today": "2027-01-10"}));
    assert!(r["entered"].as_array().unwrap().is_empty());
    assert_eq!(
        b.err(
            "schedule.update",
            json!({"schedule": sch["id"], "auto_enter": true})
        )
        .code(),
        "invalid_args"
    );
    assert_eq!(
        b.run("schedule.list", json!({})).as_array().unwrap().len(),
        1
    );
}

#[test]
fn subscriptions_sa13_make_bill_now() {
    let mut b = book();
    let id = netflix(&mut b);
    b.run("module.enable", json!({"module": "bills"}));
    b.run(
        "module.set_setting",
        json!({"key": "subscriptions.make_bill_as", "value": "bill_now"}),
    );
    let r = b.run("subscription.make_bill", json!({"subscription": id}));
    assert_eq!(r["mode"], "bill_now");
    assert_eq!(r["draft"]["vendor"], "Netflix");
    assert_eq!(r["draft"]["lines"][0]["amount"], "17.99");
    assert_eq!(r["category_name"], "Expenses:Streaming");
    // Nothing is created until the editor saves.
    assert!(
        b.run("schedule.list", json!({}))
            .as_array()
            .unwrap()
            .is_empty()
    );
    // A recurring bill makes an estimate with its average.
    for (d, a) in [
        ("2026-08-12", "82.10"),
        ("2026-09-12", "95.40"),
        ("2026-10-12", "120.55"),
    ] {
        pay(&mut b, d, "City Electric", a, "Checking", "Utilities");
    }
    let e = confirm(&mut b, "City Electric", "bill");
    b.run(
        "module.set_setting",
        json!({"key": "subscriptions.make_bill_as", "value": "scheduled_bill"}),
    );
    let r = b.run("subscription.make_bill", json!({"subscription": e}));
    assert_eq!(r["schedule"]["amount_kind"], "estimate");
    assert_eq!(r["schedule"]["bill_view"]["total"], "99.35");
    b.run("history.undo", json!({}));
    assert!(
        b.run("schedule.list", json!({}))
            .as_array()
            .unwrap()
            .is_empty()
    );
}
