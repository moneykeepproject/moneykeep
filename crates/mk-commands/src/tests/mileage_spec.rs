//! mileage.md acceptance tests M1–M9 at the command level, plus the edge cases the
//! spec names (M10–M14 are the phone, the server and screenshots).

use base64::Engine;

use super::*;

fn unb64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(v.as_str().unwrap())
        .unwrap()
}

/// A book with mileage and expenses on and one vehicle.
fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "mileage"}));
    b.run("module.enable", json!({"module": "expenses"}));
    b.run("vehicle.create", json!({"name": "2019 Honda CR-V"}));
    b
}

fn trip(b: &mut Book, args: Value) -> Value {
    let mut a = json!({"purpose": "Client visit", "from_place": "Office", "to_place": "Client"});
    for (k, v) in args.as_object().unwrap() {
        a[k] = v.clone();
    }
    b.run("trip.create", a)
}

#[test]
fn mileage_shipped_rate_table() {
    let mut b = book();
    let rows = b.run("mileage_rate.list", json!({}));
    let got: Vec<(String, String, String)> = rows["rates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["from"].as_str().unwrap().to_owned(),
                r["kind"].as_str().unwrap().to_owned(),
                r["cents"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let want = [
        ("2025-01-01", "business", "70.0"),
        ("2025-01-01", "medical", "21.0"),
        ("2025-01-01", "moving", "21.0"),
        ("2025-01-01", "charity", "14.0"),
        ("2026-01-01", "business", "72.5"),
        ("2026-01-01", "medical", "20.5"),
        ("2026-01-01", "moving", "20.5"),
        ("2026-01-01", "charity", "14.0"),
        ("2026-07-01", "business", "76.0"),
        ("2026-07-01", "medical", "23.5"),
        ("2026-07-01", "moving", "23.5"),
        ("2026-07-01", "charity", "14.0"),
    ];
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(want) {
        assert_eq!((g.0.as_str(), g.1.as_str(), g.2.as_str()), w);
    }
    assert!(rows["note"].as_str().unwrap().contains("IRS"));
}

#[test]
fn mileage_m1_rate_lookup() {
    let mut b = book();
    let t1 = trip(&mut b, json!({"date": "2026-06-30", "miles": "10"}));
    let t2 = trip(&mut b, json!({"date": "2026-07-01", "miles": "11"}));
    assert_eq!(t1["rate_cents"], "72.5");
    assert_eq!(t2["rate_cents"], "76.0");
    for d in ["2026-06-30", "2026-07-01"] {
        let c = trip(
            &mut b,
            json!({"date": d, "miles": "5", "purpose_kind": "charity",
                   "purpose": "Food pantry delivery for St. Mark's"}),
        );
        assert_eq!(c["rate_cents"], "14.0");
        assert!(c["warnings"].as_array().unwrap().is_empty());
    }
    let old = trip(&mut b, json!({"date": "2024-05-01", "miles": "10"}));
    assert_eq!(old["rate_cents"], "0");
    assert_eq!(old["amount"], "0.00");
    assert_eq!(
        old["warnings"],
        json!(["No standard rate for this date; add one in Settings → Mileage → Rates"])
    );
    // Personal: no rate and no warning.
    let p = b.run(
        "trip.create",
        json!({"date": "2026-08-01", "miles": "3", "purpose_kind": "personal"}),
    );
    assert_eq!(p["rate_cents"], "0");
    assert!(p["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn mileage_m2_round_trip() {
    let mut b = book();
    let t = trip(
        &mut b,
        json!({"date": "2026-03-02", "miles": "12.4", "round_trip": true}),
    );
    assert_eq!(t["miles"], "24.8");
    assert_eq!(t["entered_miles"], "12.4");
    assert_eq!(t["amount"], "17.98");
    assert_eq!(t["route"], "Office → Client → Office");
    // Over 1,000 one way is refused.
    let e = b.err(
        "trip.create",
        json!({"date": "2026-03-03", "miles": "1000.5", "round_trip": true,
               "purpose": "x", "to_place": "y"}),
    );
    assert_eq!(e.code(), "invalid_args");
    let e = b.err(
        "trip.create",
        json!({"date": "2026-03-03", "miles": "2000.1", "purpose": "x", "to_place": "y"}),
    );
    assert_eq!(e.code(), "invalid_args");
    let e = b.err(
        "trip.create",
        json!({"date": "2026-03-03", "miles": "1.25", "purpose": "x", "to_place": "y"}),
    );
    assert!(e.to_string().contains("1 decimal"), "{e}");
}

#[test]
fn mileage_m3_odometer() {
    let mut b = book();
    let t = trip(
        &mut b,
        json!({"date": "2026-03-02", "odometer_start": "45210.3", "odometer_end": "45236.0",
               "round_trip": true}),
    );
    assert_eq!(t["miles"], "25.7");
    assert_eq!(t["round_trip"], false);
    // A later trip starting below this one's end shows the warning on both.
    let t2 = trip(
        &mut b,
        json!({"date": "2026-03-05", "odometer_start": "45230.0", "odometer_end": "45240.0"}),
    );
    assert!(
        t2["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("Odometer readings out of order for this vehicle"))
    );
    let again = b.run("trip.get", json!({"trip": t["id"]}));
    assert!(
        again["warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("Odometer readings out of order for this vehicle"))
    );
    let e = b.err(
        "trip.create",
        json!({"date": "2026-03-06", "odometer_start": "100", "odometer_end": "90",
               "purpose": "x", "to_place": "y"}),
    );
    assert_eq!(e.code(), "invalid_args");
}

#[test]
fn mileage_m4_reimbursable_trip_posts_and_is_reimbursed_with_an_expense() {
    let mut b = book();
    let before = b.txn_count();
    let t = trip(
        &mut b,
        json!({"date": "2026-06-15", "miles": "12.4", "round_trip": true,
               "reimbursable": true, "driver": "Pat Lee"}),
    );
    assert_eq!(t["status"], "ready");
    assert_eq!(t["reimburse_amount"], "17.98");
    assert_eq!(t["category_name"], "Auto:Mileage reimbursement");
    assert_eq!(
        b.txn_count(),
        before,
        "ready trips are not in the books yet"
    );
    let posted = b.run("trip.post", json!({"trip": t["id"]}));
    assert_eq!(posted["status"], "posted");
    let txn = b.run("txn.get", json!({"id": posted["transaction"]}));
    assert_eq!(txn["payee"], "Pat Lee");
    assert!(
        txn["memo"]
            .as_str()
            .unwrap()
            .starts_with("Mileage: Office → Client → Office (24.8 mi × 72.5¢) — Client visit"),
        "{}",
        txn["memo"]
    );
    let splits = txn["splits"].as_array().unwrap();
    let line = |name: &str| {
        splits
            .iter()
            .find(|s| s["account_name"] == name)
            .unwrap_or_else(|| panic!("no {name} in {splits:?}"))["value"]
            .clone()
    };
    assert_eq!(line("Auto:Mileage reimbursement"), "17.98");
    assert_eq!(line("Reimbursements Payable"), "-17.98");
    // An expense of 40.00 from personal funds, then one reimbursement for both.
    let e = b.run(
        "expense.create",
        json!({"date": "2026-06-16", "merchant": "Paper Co", "amount": "40.00",
               "category": "Expenses:Groceries", "paid_with": "personal"}),
    );
    b.run("expense.post", json!({"expense": e["id"]}));
    let r = b.run(
        "expense.record_reimbursement",
        json!({"expenses": [e["id"]], "trips": [t["id"]], "from_account": "Assets:Checking",
               "date": "2026-06-30"}),
    );
    assert_eq!(r["total"], "57.98");
    let transfer = b.run("txn.get", json!({"id": r["transaction"]}));
    let splits = transfer["splits"].as_array().unwrap();
    assert_eq!(splits.len(), 2, "{splits:?}");
    assert!(splits.iter().any(|s| s["value"] == "-57.98"));
    assert!(splits.iter().any(|s| s["value"] == "57.98"));
    let t = b.run("trip.get", json!({"trip": t["id"]}));
    assert_eq!(t["reimbursement"], r["transaction"]);
    // Reimbursing again is refused.
    let err = b.err(
        "expense.record_reimbursement",
        json!({"trips": [t["id"]], "from_account": "Assets:Checking"}),
    );
    assert_eq!(err.code(), "rejected");
    // Editing the posted trip changes its transaction.
    b.run(
        "trip.update",
        json!({"trip": t["id"], "miles": "15", "round_trip": false}),
    );
    let txn = b.run("txn.get", json!({"id": posted["transaction"]}));
    assert!(
        txn["splits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["value"] == "10.88")
    );
    // Deleting it asks about the transaction.
    let err = b.err("trip.delete", json!({"trip": t["id"]}));
    assert_eq!(err.code(), "needs_choice");
}

#[test]
fn mileage_m5_log_only_trips_never_touch_the_ledger() {
    let mut b = book();
    let state = b.state();
    let t = trip(&mut b, json!({"date": "2026-04-01", "miles": "8"}));
    assert_eq!(t["status"], "logged");
    assert_eq!(t["amount"], "5.80");
    assert_eq!(b.state(), state);
    let err = b.err("trip.post", json!({"trip": t["id"]}));
    assert_eq!(err.code(), "rejected");
    b.run("trip.delete", json!({"trip": t["id"]}));
    assert_eq!(b.state(), state);
    // Undo brings it back.
    b.run("history.undo", json!({}));
    assert_eq!(b.run("trip.list", json!({})).as_array().unwrap().len(), 1);
}

fn m6_book() -> Book {
    let mut b = book();
    b.run(
        "vehicle.set_odometer",
        json!({"vehicle": "2019 Honda CR-V", "year": 2026, "start": "30000", "end": "42000"}),
    );
    trip(&mut b, json!({"date": "2026-02-10", "miles": "500"}));
    trip(&mut b, json!({"date": "2026-08-10", "miles": "300"}));
    trip(
        &mut b,
        json!({"date": "2026-05-01", "miles": "120", "purpose_kind": "charity",
               "purpose": "=HYPERLINK(\"http://x\",\"click\")"}),
    );
    b
}

#[test]
fn mileage_m6_report_pdf_and_csv() {
    let mut b = m6_book();
    let s = b.run("mileage.summary", json!({"year": 2026}));
    let v = &s["vehicles"][0];
    assert_eq!(v["total_miles"], "12000.0");
    let pct = |k: &str| {
        v["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["kind"] == k)
            .unwrap()["percent"]
            .clone()
    };
    assert_eq!(pct("business"), "6.7");
    assert_eq!(pct("charity"), "1.0");
    let n = b.audit_len();
    let out = b.run(
        "mileage.report",
        json!({"from": "2026-01-01", "to": "2026-12-31", "format": "pdf"}),
    );
    assert_eq!(b.audit_len(), n, "a report changes nothing");
    let pdf = unb64(&out["content_base64"]);
    assert!(pdf.starts_with(b"%PDF-1.4"));
    assert!(pdf.ends_with(b"%%EOF\n"));
    let text: String = pdf.iter().map(|c| char::from(*c)).collect();
    for needle in [
        "(Mileage report)",
        "(2019 Honda CR-V)",
        "(Odometer 30,000.0 to 42,000.0)",
        "(Total miles 12,000.0)",
        "(6.7 %)",
        "(1.0 %)",
        "(Business)",
        "(Charity)",
        "(Subtotal business)",
        "(800.0)",
        "(Office -> Client)",
    ] {
        assert!(text.contains(needle), "missing {needle}");
    }
    for bad in ["/JS", "/JavaScript", "/URI", "/EmbeddedFile", "/AcroForm"] {
        assert!(!text.contains(bad), "contains {bad}");
    }
    let csv = b.run(
        "mileage.report",
        json!({"from": "2026-01-01", "to": "2026-12-31", "format": "csv"}),
    );
    let content = csv["content"].as_str().unwrap();
    assert!(
        content.starts_with(
            "Section,Vehicle,Purpose kind,Date,Route,Purpose,Miles,Rate (cents),Amount,Percent,Odometer start,Odometer end\r\n"
        ),
        "{content}"
    );
    assert!(
        content.contains("\"'=HYPERLINK(\"\"http://x\"\",\"\"click\"\")\""),
        "{content}"
    );
    assert!(content.contains(
        "Trip,2019 Honda CR-V,Business,2026-02-10,Office → Client,Client visit,500.0,72.5,362.50"
    ));
    assert!(
        content.contains("Total,,Business,,,,800.0,,590.50"),
        "{content}"
    );
    // Only some kinds, and one vehicle.
    let only = b.run(
        "mileage.report",
        json!({"from": "2026-01-01", "to": "2026-12-31", "format": "csv",
               "kinds": ["charity"], "vehicle": "2019 Honda CR-V"}),
    );
    assert_eq!(only["trips"], 1);
}

#[test]
fn mileage_m7_expense_report_includes_posted_trip() {
    let mut b = book();
    let t = trip(
        &mut b,
        json!({"date": "2026-10-05", "miles": "20", "reimbursable": true}),
    );
    b.run("trip.post", json!({"trip": t["id"]}));
    // A log-only trip stays out.
    trip(&mut b, json!({"date": "2026-10-06", "miles": "7"}));
    let out = b.run(
        "expense.report",
        json!({"from": "2026-10-01", "to": "2026-10-31"}),
    );
    assert_eq!(out["trips"], 1);
    assert_eq!(out["total"], "15.20");
    let pdf = unb64(&out["content_base64"]);
    let text: String = pdf.iter().map(|c| char::from(*c)).collect();
    for needle in [
        "(2026-10-05  Mileage \u{97} 2019 Honda CR-V  15.20)",
        "(N/A)",
        "(Auto:Mileage reimbursement)",
        "(Office -> Client: 20.0 mi \u{d7} 76\u{a2} = 15.20)",
    ] {
        assert!(
            text.contains(needle),
            "missing {needle} in {:?}",
            text.match_indices("Mileage")
                .map(|(i, _)| &text[i.saturating_sub(2)..i + 30])
                .collect::<Vec<_>>()
        );
    }
    // Marking reported flags the trip.
    b.run(
        "expense.mark_reported",
        json!({"trips": out["trip_ids"].clone()}),
    );
    assert_eq!(
        b.run("trip.get", json!({"trip": t["id"]}))["reported"],
        true
    );
}

#[test]
fn mileage_m8_tax_summary_section() {
    let mut b = m6_book();
    let r = b.run(
        "report.tax_summary",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    let m = &r["mileage"];
    assert_eq!(m["title"], "Mileage (standard rates)");
    assert_eq!(
        m["note"],
        "Information for your tax preparer; not entered in the books."
    );
    let rows = m["vehicles"][0]["rows"].as_array().unwrap();
    let business = rows.iter().find(|x| x["kind"] == "business").unwrap();
    assert_eq!(business["miles"], "800.0");
    assert_eq!(business["value"], "590.50");
    let charity = rows.iter().find(|x| x["kind"] == "charity").unwrap();
    assert_eq!(charity["value"], "16.80");
    let pdf = b.run(
        "report.export",
        json!({"report": "tax_summary", "format": "pdf",
               "args": {"from": "2026-01-01", "to": "2026-12-31"}}),
    );
    let text: String = unb64(&pdf["content_base64"])
        .iter()
        .map(|c| char::from(*c))
        .collect();
    assert!(text.contains("(Mileage \\(standard rates\\))"));
    assert!(text.contains("(590.50)"));
    let csv = b.run(
        "report.export",
        json!({"report": "tax_summary", "args": {"from": "2026-01-01", "to": "2026-12-31"}}),
    );
    assert!(csv["content"].as_str().unwrap().contains("590.50"));
    // No trips in 2025: no section.
    let r = b.run(
        "report.tax_summary",
        json!({"from": "2025-01-01", "to": "2025-12-31"}),
    );
    assert!(r["mileage"].is_null());
}

#[test]
fn mileage_m9_reapply_rates_and_undo() {
    let mut b = book();
    let t1 = trip(&mut b, json!({"date": "2026-07-10", "miles": "100"}));
    let t2 = trip(&mut b, json!({"date": "2026-09-10", "miles": "50"}));
    trip(&mut b, json!({"date": "2026-06-10", "miles": "10"}));
    assert_eq!(t1["amount"], "76.00");
    b.run(
        "mileage_rate.set",
        json!({"kind": "business", "from": "2026-09-01", "cents": "80"}),
    );
    // Saved trips keep their rate until rates are applied again.
    assert_eq!(
        b.run("trip.get", json!({"trip": t2["id"]}))["rate_cents"],
        "76.0"
    );
    let n = b.audit_len();
    let p = b.run(
        "mileage.reapply_rates",
        json!({"from": "2026-07-01", "to": "2026-12-31", "preview": true}),
    );
    assert_eq!(p["trips"], 1);
    assert_eq!(p["difference"], "2.00");
    assert_eq!(b.audit_len(), n, "a preview writes nothing");
    let done = b.run(
        "mileage.reapply_rates",
        json!({"from": "2026-07-01", "to": "2026-12-31"}),
    );
    assert_eq!(done["trips"], 1);
    let t = b.run("trip.get", json!({"trip": t2["id"]}));
    assert_eq!(t["rate_cents"], "80.0");
    assert_eq!(t["amount"], "40.00");
    b.run("history.undo", json!({}));
    let t = b.run("trip.get", json!({"trip": t2["id"]}));
    assert_eq!(t["rate_cents"], "76.0");
    assert_eq!(t["amount"], "38.00");
}

#[test]
fn mileage_edge_cases() {
    let mut b = book();
    // Same kind and start date refused.
    let e = b.err(
        "mileage_rate.set",
        json!({"kind": "business", "from": "2026-07-01", "cents": "77"}),
    );
    assert_eq!(
        e.to_string(),
        "A rate for this kind already starts on that date"
    );
    // Duplicate trips ask first.
    trip(&mut b, json!({"date": "2026-05-05", "miles": "9"}));
    let e = b.err(
        "trip.create",
        json!({"date": "2026-05-05", "miles": "9", "purpose": "Again", "to_place": "Client"}),
    );
    assert_eq!(e.code(), "duplicate_trip");
    assert_eq!(
        e.to_string(),
        "A trip with these details already exists. Save anyway?"
    );
    trip(
        &mut b,
        json!({"date": "2026-05-05", "miles": "9", "allow_duplicate": true}),
    );
    // A vehicle with trips can't be deleted; it can be made inactive.
    let e = b.err("vehicle.delete", json!({"vehicle": "2019 Honda CR-V"}));
    assert_eq!(e.code(), "rejected");
    b.run(
        "vehicle.update",
        json!({"vehicle": "2019 Honda CR-V", "active": false}),
    );
    let active = b.run("vehicle.list", json!({"include_inactive": false}));
    assert!(active.as_array().unwrap().is_empty());
    // Vehicle names are unique ignoring case.
    let e = b.err("vehicle.create", json!({"name": "2019 HONDA CR-V"}));
    assert_eq!(e.code(), "rejected");
    // Purpose and destination are required unless personal.
    let e = b.err(
        "trip.create",
        json!({"date": "2026-05-06", "miles": "3", "vehicle": "2019 Honda CR-V"}),
    );
    assert_eq!(e.code(), "invalid_args");
    // Saved trips fill the form.
    let s = b.run(
        "saved_trip.create",
        json!({"name": "Church run", "from_place": "Home", "to_place": "St. Mark's",
               "miles": "6.2", "round_trip": true, "purpose_kind": "charity",
               "purpose": "Food pantry delivery", "vehicle": "2019 Honda CR-V"}),
    );
    let t = b.run(
        "trip.create",
        json!({"date": "2026-05-07", "saved_trip": s["id"]}),
    );
    assert_eq!(t["miles"], "12.4");
    assert_eq!(t["purpose_kind"], "charity");
    assert_eq!(t["route"], "Home → St. Mark's → Home");
    // Reimbursement needs the expenses module.
    b.run("module.disable", json!({"module": "expenses"}));
    let e = b.err(
        "trip.create",
        json!({"date": "2026-05-08", "miles": "4", "vehicle": "2019 Honda CR-V",
               "purpose": "x", "to_place": "y", "reimbursable": true}),
    );
    assert_eq!(e.code(), "rejected");
    // Module off: commands hidden, data kept.
    b.run("module.disable", json!({"module": "mileage"}));
    assert_eq!(b.err("trip.list", json!({})).code(), "module_disabled");
    b.run("module.enable", json!({"module": "mileage"}));
    assert_eq!(b.run("trip.list", json!({})).as_array().unwrap().len(), 3);
}

#[test]
fn mileage_settings_and_client_id() {
    let mut b = book();
    let s = b.run("module.settings", json!({"module": "mileage"}));
    let keys: Vec<&str> = s["settings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        [
            "mileage.default_purpose",
            "mileage.show_moving",
            "mileage.reimbursement_rate",
            "mileage.expense_account",
            "mileage.distance_unit"
        ]
    );
    b.run(
        "module.set_setting",
        json!({"key": "mileage.reimbursement_rate", "value": "60"}),
    );
    let t = trip(
        &mut b,
        json!({"date": "2026-07-02", "miles": "10", "reimbursable": true}),
    );
    assert_eq!(t["reimburse_rate_cents"], "60.0");
    assert_eq!(t["reimburse_amount"], "6.00");
    assert_eq!(t["amount"], "7.60");
    b.run(
        "module.set_setting",
        json!({"key": "mileage.reimbursement_rate", "value": ""}),
    );
    let e = b.err(
        "module.set_setting",
        json!({"key": "mileage.reimbursement_rate", "value": "12.34"}),
    );
    assert_eq!(e.code(), "invalid_args");
    b.run(
        "module.set_setting",
        json!({"key": "mileage.default_purpose", "value": "charity"}),
    );
    // A trip sent twice with the same client id is made once.
    let args = json!({"date": "2026-07-03", "miles": "2", "purpose": "Visit",
                      "to_place": "Hospital", "source": "phone", "client_id": "abc-1"});
    let first = b.run("trip.create", args.clone());
    assert_eq!(first["purpose_kind"], "charity");
    assert_eq!(first["source"], "phone");
    let n = b.audit_len();
    let again = b.run("trip.create", args);
    assert_eq!(again["id"], first["id"]);
    assert_eq!(again["already_taken"], true);
    assert_eq!(b.audit_len(), n);
}

#[test]
fn mileage_phone_trip_with_unknown_vehicle_waits_for_a_choice() {
    let mut b = book();
    let actor = Actor("test".into());
    let out =
        b.s.write(|tx| -> CommandResult<_> {
            let ctx = Ctx::new(tx, &actor);
            let p = crate::mileage::PhoneTrip {
                client_id: "phone-1".into(),
                date: Some("2026-09-01".into()),
                vehicle_id: Some("no-such-vehicle".into()),
                purpose_kind: Some("business".into()),
                purpose: Some("Supplies".into()),
                to_place: Some("Store".into()),
                miles: Some("4.5".into()),
                round_trip: true,
                ..Default::default()
            };
            let today = "2026-09-02".parse().unwrap();
            let first = crate::mileage::create_from_phone(&ctx, &p, today)?;
            let second = crate::mileage::create_from_phone(&ctx, &p, today)?;
            Ok((first, second))
        })
        .unwrap();
    assert_eq!(out.0.status, "draft");
    assert!(out.0.notes.contains(&"Choose a vehicle".to_owned()));
    assert!(out.1.already_taken);
    assert_eq!(out.1.id, out.0.id);
    let t = b.run("trip.get", json!({"trip": out.0.id}));
    assert_eq!(t["miles"], "9.0");
    // Choosing the vehicle completes it.
    let t = b.run(
        "trip.update",
        json!({"trip": out.0.id, "vehicle": "2019 Honda CR-V"}),
    );
    assert_eq!(t["status"], "logged");
    assert_eq!(t["rate_cents"], "76.0");
}
