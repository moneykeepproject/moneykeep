//! Property test: for random valid QIF and CSV inputs, stage+commit then re-stage
//! yields only duplicates; commit then undo restores the book exactly.

use proptest::prelude::*;

use super::*;

#[derive(Debug, Clone)]
struct GenRow {
    day: u8,
    cents: i64,
    payee: usize,
}

const PAYEES: &[&str] = &["Corner Grocery", "City Water", "Cafe", "Payroll", "Kiosk"];

fn rows() -> impl Strategy<Value = Vec<GenRow>> {
    proptest::collection::vec(
        (
            1u8..28,
            prop_oneof![-50_000i64..-1, 1i64..50_000, Just(-500i64)],
            0..PAYEES.len(),
        )
            .prop_map(|(day, cents, payee)| GenRow { day, cents, payee }),
        1..12,
    )
}

fn amount(cents: i64) -> String {
    mk_core::Decimal::new(cents, 2).to_string()
}

fn qif_file(rows: &[GenRow]) -> String {
    let mut s = String::from("!Type:Bank\n");
    for r in rows {
        s.push_str(&format!(
            "D10/{:02}/2026\nT{}\nP{}\n^\n",
            r.day,
            amount(r.cents),
            PAYEES[r.payee]
        ));
    }
    s
}

fn csv_file(rows: &[GenRow]) -> String {
    let mut s = String::from("Date,Description,Amount\n");
    for r in rows {
        s.push_str(&format!(
            "10/{:02}/2026,\"{}\",{}\n",
            r.day,
            PAYEES[r.payee],
            amount(r.cents)
        ));
    }
    s
}

fn book(manual: &[GenRow]) -> Book {
    let mut b = Book::new();
    b.standard();
    b.run("csv.profile_save", json!({"profile": {"name": "P", "columns": {"date": "Date", "payee": "Description", "amount": "Amount"},
        "date_format": "MM/DD/YYYY", "account": "Assets:Checking"}}));
    b.run("rule.create", json!({"name": "Water", "conditions": [{"field": "payee", "op": "contains", "value": "water"}],
        "actions": [{"action": "set_account", "account": "Expenses:Groceries"}]}));
    // Some manual transactions so matches happen.
    for r in manual {
        let v = amount(r.cents);
        let neg = amount(-r.cents);
        b.run("txn.create", json!({"date": format!("2026-10-{:02}", r.day), "splits": [
            {"account": "Assets:Checking", "value": v}, {"account": "Expenses:Groceries", "value": neg}]}));
    }
    b
}

fn stage(b: &mut Book, csv: bool, rows: &[GenRow]) -> Value {
    if csv {
        b.run(
            "import.stage",
            json!({"format": "csv", "content": csv_file(rows)}),
        )
    } else {
        b.stage_qif(&qif_file(rows), "Assets:Checking")
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 40, ..ProptestConfig::default() })]

    #[test]
    fn import_property_restage_duplicates_and_undo_restores(
        file in rows(), manual in proptest::collection::vec(any::<prop::sample::Index>(), 0..3), csv in any::<bool>()
    ) {
        let manual: Vec<GenRow> = manual.iter().map(|i| file[i.index(file.len())].clone()).collect();
        let mut b = book(&manual);
        let before = b.state();
        let batch = stage(&mut b, csv, &file);
        let staged = b.state();
        prop_assert_eq!(&staged["txns"], &before["txns"]);
        b.commit(&batch);
        let committed = b.state();
        let again = stage(&mut b, csv, &file);
        for r in again["rows"].as_array().unwrap() {
            prop_assert_eq!(r["class"].as_str(), Some("duplicate"), "{}", r);
        }
        // Undo the re-stage, then the commit: the book is as before the commit.
        b.run("history.undo", json!({}));
        prop_assert_eq!(b.state(), committed);
        b.run("history.undo", json!({}));
        prop_assert_eq!(b.state(), staged);
        prop_assert_eq!(&b.state()["accounts"], &before["accounts"]);
    }
}
