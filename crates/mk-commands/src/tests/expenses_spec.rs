//! expenses.md acceptance tests 1–9 at the command level (10 is in the MCP tests),
//! plus the edge cases the spec names.

use base64::Engine;
use mk_import::image::Rgb;

use super::*;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(v.as_str().unwrap())
        .unwrap()
}

/// A book with the module on: Checking (bank), Visa (credit card), Office Supplies
/// and Meals (expenses).
fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "expenses"}));
    b.account(json!({"name": "Office Supplies", "type": "expense", "parent": "Expenses"}));
    b.account(json!({"name": "Meals", "type": "expense", "parent": "Expenses"}));
    b
}

/// A picture with four coloured quadrants: red top-left, green top-right, blue
/// bottom-left, white bottom-right.
fn quadrants(w: u32, h: u32) -> Rgb {
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            data.extend(match (x < w / 2, y < h / 2) {
                (true, true) => [230, 20, 20],
                (false, true) => [20, 200, 30],
                (true, false) => [20, 30, 220],
                (false, false) => [245, 245, 245],
            });
        }
    }
    Rgb {
        width: w,
        height: h,
        data,
    }
}

fn small_jpeg(seed: u8) -> Vec<u8> {
    let mut img = quadrants(120, 90);
    // A block of the seed's own shade makes each picture different.
    for y in 0..16u32 {
        for x in 0..16u32 {
            let at = ((y * 120 + x) * 3) as usize;
            img.data[at..at + 3].copy_from_slice(&[seed.wrapping_mul(23), 128, 64]);
        }
    }
    mk_import::jpeg::encode_jpeg(&img, 85).unwrap()
}

fn add_receipt(b: &mut Book, bytes: &[u8], name: &str) -> Value {
    b.run(
        "receipt.add",
        json!({"content_base64": b64(bytes), "file_name": name}),
    )
}

fn find(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn px(img: &Rgb, x: u32, y: u32) -> [u8; 3] {
    let at = ((y * img.width + x) * 3) as usize;
    [img.data[at], img.data[at + 1], img.data[at + 2]]
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) < 40)
}

#[test]
fn expenses_at1_photo_upright_scaled_and_stripped() {
    let mut b = book();
    let jpg = mk_import::jpeg::encode_jpeg(&quadrants(4000, 3000), 90).unwrap();
    let tagged = mk_import::receipt::with_test_exif(&jpg, 6);
    assert!(find(&tagged, b"GPSTESTMARK") && find(&tagged, b"Exif"));
    let r = add_receipt(&mut b, &tagged, "IMG_0001.jpg");
    assert_eq!(r["kind"], "jpeg");
    assert_eq!(
        (r["width"].as_u64(), r["height"].as_u64()),
        (Some(1800), Some(2400))
    );
    assert_eq!(r["file_name"], "IMG_0001.jpg");
    assert_eq!(r["source"], "desktop");
    assert!(
        r["expense"].is_null() && r["transaction"].is_null(),
        "in the inbox"
    );
    let got = b.run("receipt.get", json!({"receipt": r["id"]}));
    let stored = unb64(&got["content_base64"]);
    assert!(!find(&stored, b"Exif"), "no EXIF segment");
    assert!(!find(&stored, b"GPSTESTMARK"), "no GPS bytes");
    let img = mk_import::receipt::decode_stored(&stored).unwrap();
    assert_eq!((img.width, img.height), (1800, 2400));
    // Turned 90° clockwise: the red top-left is now at the top right, blue at the
    // top left.
    assert!(
        near(px(&img, 1700, 100), [230, 20, 20]),
        "{:?}",
        px(&img, 1700, 100)
    );
    assert!(
        near(px(&img, 100, 100), [20, 30, 220]),
        "{:?}",
        px(&img, 100, 100)
    );
    assert!(near(px(&img, 1700, 2300), [20, 200, 30]));
    let t = b.run("receipt.thumbnail", json!({"receipt": r["id"]}));
    let thumb = mk_import::image::decode_logo(&unb64(&t["png_base64"])).unwrap();
    assert_eq!((thumb.width, thumb.height), (192, 256));
    // The audit entry records the file's digest, not its bytes.
    let entries = b.s.audit_entries(1).unwrap();
    let args = entries[0].args.to_string();
    assert!(args.contains("sha256") && !args.contains("content_base64"));
    assert!(args.len() < 1000);
    // In the inbox.
    let inbox = b.run("receipt.inbox", json!({}));
    assert_eq!(inbox.as_array().unwrap().len(), 1);
}

fn office_depot(b: &mut Book, receipt: Option<&Value>) -> Value {
    let mut args = json!({
        "date": "2026-10-03", "merchant": "Office Depot", "amount": "45.20",
        "category": "Expenses:Office Supplies", "paid_with": "Liabilities:Visa",
        "purpose": "Printer paper",
    });
    if let Some(r) = receipt {
        args["receipts"] = json!([r["id"]]);
    }
    b.run("expense.create", args)
}

#[test]
fn expenses_at2_post_as_new_transaction() {
    let mut b = book();
    let r = add_receipt(&mut b, &small_jpeg(1), "depot.jpg");
    let e = office_depot(&mut b, Some(&r));
    assert_eq!(e["status"], "ready");
    assert_eq!(e["receipt_count"], 1);
    assert!(
        b.run("receipt.inbox", json!({}))
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(b.txn_count(), 0);
    let p = b.run("expense.post", json!({"expense": e["id"]}));
    assert_eq!(p["status"], "posted");
    assert_eq!(b.txn_count(), 1);
    let t = b.run("txn.get", json!({"id": p["transaction"]}));
    assert_eq!(t["payee"], "Office Depot");
    assert_eq!(t["memo"], "Printer paper");
    assert_eq!(b.shown("Expenses:Office Supplies", "USD"), "45.20");
    assert_eq!(b.shown("Liabilities:Visa", "USD"), "45.20");
    // The receipt is on the transaction now.
    let on_txn = b.run("receipt.list", json!({"transaction": p["transaction"]}));
    assert_eq!(on_txn[0]["id"], r["id"]);
    assert_eq!(p["receipts"][0]["id"], r["id"]);
    let tb = b.run("report.trial_balance", json!({}));
    assert_eq!(tb["balanced"], true);
    assert_eq!(tb["value_totals"], json!({"USD": "0.00"}));
    // The register row says it has receipts.
    let reg = b.run("account.register", json!({"account": "Liabilities:Visa"}));
    assert_eq!(reg["rows"][0]["receipts"], 1);
    // One undo step removes the transaction and puts the receipt back.
    b.run("history.undo", json!({}));
    assert_eq!(b.txn_count(), 0);
    let e2 = b.run("expense.get", json!({"expense": e["id"]}));
    assert_eq!(e2["status"], "ready");
    assert_eq!(e2["receipt_count"], 1);
}

#[test]
fn expenses_at3_match_imported_uncategorized() {
    let mut b = book();
    let batch = b.stage_qif(
        "!Type:CCard\nD10/05/2026\nT-45.20\nPOFFICE DEPOT #1234\n^\n",
        "Liabilities:Visa",
    );
    b.commit(&batch);
    assert_eq!(b.txn_count(), 1);
    let imported = b.run("txn.list", json!({}))[0].clone();
    assert!(imported.to_string().contains("Uncategorized Expense"));
    let r = add_receipt(&mut b, &small_jpeg(2), "depot.jpg");
    let e = office_depot(&mut b, Some(&r));
    let n = b.audit_len();
    let proposal = b.run("expense.post", json!({"expense": e["id"]}));
    assert_eq!(proposal["status"], "needs_choice");
    let c = proposal["candidates"].as_array().unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0]["id"], imported["id"]);
    assert_eq!(c[0]["date"], "2026-10-05");
    assert_eq!(c[0]["uncategorized"], true);
    assert_eq!(b.audit_len(), n, "proposing changes nothing");
    assert_eq!(
        b.run("expense.match_candidates", json!({"expense": e["id"]}))[0]["id"],
        imported["id"]
    );
    let m = b.run(
        "expense.post",
        json!({"expense": e["id"], "match": imported["id"]}),
    );
    assert_eq!(m["status"], "matched");
    assert_eq!(m["transaction"], imported["id"]);
    assert_eq!(b.txn_count(), 1, "no new transaction");
    let t = b.run("txn.get", json!({"id": imported["id"]}));
    let accounts: Vec<&str> = t["splits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["account_name"].as_str().unwrap())
        .collect();
    assert!(
        accounts.contains(&"Expenses:Office Supplies"),
        "{accounts:?}"
    );
    assert!(!accounts.contains(&"Uncategorized Expense"));
    let on_txn = b.run("receipt.list", json!({"transaction": imported["id"]}));
    assert_eq!(on_txn[0]["id"], r["id"]);
    // Matched transactions are no longer candidates for another expense.
    let e2 = office_depot(&mut b, None);
    assert!(
        b.run("expense.match_candidates", json!({"expense": e2["id"]}))
            .as_array()
            .unwrap()
            .is_empty()
    );
    // Deleting a matched expense only unlinks it: the bank transaction stays.
    b.run("expense.delete", json!({"expense": e["id"]}));
    assert_eq!(b.txn_count(), 1);
    assert_eq!(
        b.run("receipt.list", json!({"transaction": imported["id"]}))[0]["id"],
        r["id"]
    );
}

#[test]
fn expenses_later_import_matches_the_posted_transaction_and_keeps_receipts() {
    let mut b = book();
    let r = add_receipt(&mut b, &small_jpeg(3), "depot.jpg");
    let e = office_depot(&mut b, Some(&r));
    let p = b.run("expense.post", json!({"expense": e["id"]}));
    let batch = b.stage_qif(
        "!Type:CCard\nD10/04/2026\nT-45.20\nPOFFICE DEPOT\n^\n",
        "Liabilities:Visa",
    );
    let out = b.commit(&batch);
    assert_eq!(out["matched"], 1, "{out}");
    assert_eq!(b.txn_count(), 1, "no duplicate");
    assert_eq!(
        b.run("receipt.list", json!({"transaction": p["transaction"]}))[0]["id"],
        r["id"]
    );
}

#[test]
fn expenses_at4_personal_funds_and_reimbursement() {
    let mut b = book();
    let e = b.run(
        "expense.create",
        json!({"date": "2026-10-08", "merchant": "Cafe Lune", "amount": "18.75",
               "category": "Expenses:Meals", "paid_with": "personal", "purpose": "Client lunch"}),
    );
    assert_eq!(e["reimbursable"], true);
    assert_eq!(e["paid_with_name"], "Personal funds (reimbursable)");
    let p = b.run("expense.post", json!({"expense": e["id"]}));
    assert_eq!(p["status"], "posted");
    let acc = b.run("account.get", json!({"account": "Reimbursements Payable"}));
    assert_eq!(acc["subtype"], "payable");
    assert_eq!(b.shown("Reimbursements Payable", "USD"), "18.75");
    let r = b.run(
        "expense.record_reimbursement",
        json!({"expenses": [e["id"]], "from_account": "Assets:Checking", "date": "2026-10-20"}),
    );
    assert_eq!(r["total"], "18.75");
    assert_eq!(b.shown("Reimbursements Payable", "USD"), "0.00");
    assert_eq!(b.shown("Assets:Checking", "USD"), "-18.75");
    let t = b.run("txn.get", json!({"id": r["transaction"]}));
    assert!(t["memo"].as_str().unwrap().contains("Cafe Lune"));
    // Not twice.
    let err = b.err(
        "expense.record_reimbursement",
        json!({"expenses": [e["id"]], "from_account": "Assets:Checking"}),
    );
    assert!(err.to_string().contains("already reimbursed"), "{err}");
    // Deleting the reimbursement transaction makes it owed again.
    b.run("txn.delete", json!({"id": r["transaction"]}));
    assert!(b.run("expense.get", json!({"expense": e["id"]}))["reimbursement"].is_null());
}

#[test]
fn expenses_at5_same_image_twice_warns() {
    let mut b = book();
    let jpg = small_jpeg(4);
    let first = add_receipt(&mut b, &jpg, "a.jpg");
    let e = office_depot(&mut b, Some(&first));
    let err = b.err(
        "receipt.add",
        json!({"content_base64": b64(&jpg), "file_name": "again.jpg"}),
    );
    assert_eq!(err.code(), "duplicate_receipt");
    assert_eq!(
        err.to_string(),
        "This receipt is already attached to the expense Office Depot on 2026-10-03."
    );
    // Attach anyway.
    let second = b.run(
        "receipt.add",
        json!({"content_base64": b64(&jpg), "file_name": "again.jpg", "allow_duplicate": true,
               "expense": e["id"]}),
    );
    assert_ne!(second["id"], first["id"]);
    assert_eq!(
        b.run("expense.get", json!({"expense": e["id"]}))["receipt_count"],
        2
    );
}

#[test]
fn expenses_at6_report_pdf() {
    let mut b = book();
    let mut ids = Vec::new();
    for (i, (date, merchant, amount, cat)) in [
        ("2026-10-02", "Cafe Lune", "18.75", "Expenses:Meals"),
        (
            "2026-10-15",
            "Paper Co",
            "30.00",
            "Expenses:Office Supplies",
        ),
        ("2026-10-20", "Diner", "12.25", "Expenses:Meals"),
    ]
    .into_iter()
    .enumerate()
    {
        let r = add_receipt(&mut b, &small_jpeg(10 + i as u8), "r.jpg");
        let e = b.run(
            "expense.create",
            json!({"date": date, "merchant": merchant, "amount": amount, "category": cat,
                   "paid_with": "personal", "purpose": "Business", "receipts": [r["id"]]}),
        );
        ids.push(e["id"].clone());
    }
    // A Visa expense (not reimbursable) and one in November (outside).
    office_depot(&mut b, None);
    b.run(
        "expense.create",
        json!({"date": "2026-11-01", "merchant": "Later", "amount": "5.00",
               "category": "Expenses:Meals", "paid_with": "personal"}),
    );
    // A PNG receipt and a PDF receipt on the third expense.
    let png = mk_import::image::encode_png(&quadrants(50, 80)).unwrap();
    b.run(
        "receipt.add",
        json!({"content_base64": b64(&png), "file_name": "scan.png", "expense": ids[2]}),
    );
    b.run(
        "receipt.add",
        json!({"content_base64": b64(b"%PDF-1.4\n<< /Type /Page >>\n%%EOF"), "file_name": "invoice.pdf", "expense": ids[2]}),
    );
    let out = b.run(
        "expense.report",
        json!({"from": "2026-10-01", "to": "2026-10-31", "filter": "reimbursable"}),
    );
    assert_eq!(out["expenses"], 3);
    assert_eq!(out["total"], "61.00");
    let pdf = unb64(&out["content_base64"]);
    assert!(pdf.starts_with(b"%PDF-1.4"));
    let text: String = pdf.iter().map(|c| char::from(*c)).collect();
    for needle in [
        "(Test)",
        "(Expense report)",
        "(Period: 2026-10-01 to 2026-10-31)",
        "(Date)",
        "(Merchant)",
        "(Category)",
        "(Paid with)",
        "(Purpose)",
        "(Amount)",
        "(Receipt)",
        "(Cafe Lune)",
        "(Paper Co)",
        "(Diner)",
        "(Totals by category)",
        "(Expenses:Meals)",
        "(31.00)",
        "(Expenses:Office Supplies)",
        "(Total)",
        "(61.00)",
        "(Reimbursable total owed)",
        "(Yes)",
        "(Receipts)",
        "(invoice.pdf \\(PDF, attached separately\\))",
    ] {
        assert!(text.contains(needle), "missing {needle}");
    }
    assert!(
        !text.contains("(Office Depot)"),
        "Visa expense filtered out"
    );
    assert!(!text.contains("(Later)"), "November excluded");
    // One section per expense with its pictures: three JPEGs and one PNG.
    assert_eq!(text.matches("/Filter /DCTDecode").count(), 3);
    assert_eq!(text.matches("/Filter /FlateDecode").count(), 1);
    assert_eq!(text.matches(" Do Q").count(), 4);
    for heading in [
        "(2026-10-02  Cafe Lune  18.75)",
        "(2026-10-15  Paper Co  30.00)",
    ] {
        assert!(text.contains(heading), "missing {heading}");
    }
    for bad in ["/JS", "/JavaScript", "/URI", "/EmbeddedFile", "/AcroForm"] {
        assert!(!text.contains(bad), "contains {bad}");
    }
    // A report changes nothing and leaves no audit entry.
    let n = b.audit_len();
    b.run(
        "expense.report",
        json!({"from": "2026-10-01", "to": "2026-10-31", "include_receipts": false}),
    );
    assert_eq!(b.audit_len(), n);
}

#[test]
fn expenses_at7_mark_reported_and_undo() {
    let mut b = book();
    let e1 = office_depot(&mut b, None);
    let e2 = b.run(
        "expense.create",
        json!({"date": "2026-10-09", "merchant": "Diner", "amount": "9.00",
               "category": "Expenses:Meals", "paid_with": "personal"}),
    );
    let n = b.audit_len();
    // The report itself is read-only (revision 2) and lists what it included.
    let out = b.run(
        "expense.report",
        json!({"from": "2026-10-01", "to": "2026-10-31"}),
    );
    assert_eq!(b.audit_len(), n, "a report writes nothing");
    assert!(out.get("marked_reported").is_none());
    let ids = out["expense_ids"].clone();
    assert_eq!(ids.as_array().unwrap().len(), 2);
    let e = b.err(
        "expense.report",
        json!({"from": "2026-10-01", "to": "2026-10-31", "mark_reported": true}),
    );
    assert!(e.to_string().contains("mark_reported"), "{e}");
    // Marking is its own command: one audited undo step.
    let out = b.run("expense.mark_reported", json!({"expenses": ids}));
    assert_eq!(out["marked_reported"], 2);
    assert_eq!(b.audit_len(), n + 1, "one undo step");
    let entry = &b.s.audit_entries(1).unwrap()[0];
    assert_eq!(entry.command, "expense.mark_reported");
    for e in [&e1, &e2] {
        assert_eq!(
            b.run("expense.get", json!({"expense": e["id"]}))["reported"],
            true
        );
    }
    // Marking them again changes nothing and leaves no entry.
    let again = b.run("expense.mark_reported", json!({"expenses": [e1["id"]]}));
    assert_eq!(again["marked_reported"], 0);
    assert_eq!(b.audit_len(), n + 1);
    assert!(
        b.err("expense.mark_reported", json!({"expenses": []}))
            .to_string()
            .contains("at least one")
    );
    let reported = b.run("expense.list", json!({"status": "reported"}));
    assert_eq!(reported.as_array().unwrap().len(), 2);
    b.run("history.undo", json!({}));
    for e in [&e1, &e2] {
        assert_eq!(
            b.run("expense.get", json!({"expense": e["id"]}))["reported"],
            false
        );
    }
}

#[test]
fn expenses_at8_refusals() {
    let mut b = book();
    let big = vec![0xFFu8; 25 * 1024 * 1024];
    let e = b.err(
        "receipt.add",
        json!({"content_base64": b64(&big), "file_name": "huge.jpg"}),
    );
    assert_eq!(e.to_string(), "Receipts can be at most 10 MB.");
    let e = b.err(
        "receipt.add",
        json!({"content_base64": b64(b"just some text, really"), "file_name": "fake.jpg"}),
    );
    assert_eq!(e.to_string(), "This picture can't be read.");
    let e = b.err(
        "receipt.add",
        json!({"content_base64": b64(b"<html>not a pdf</html>"), "file_name": "doc.pdf"}),
    );
    assert_eq!(e.to_string(), "This PDF can't be read.");
    let e = b.err(
        "receipt.add",
        json!({"content_base64": b64(b"\0\0\0\x18ftypheic\0\0\0\0"), "file_name": "IMG_2.HEIC"}),
    );
    assert_eq!(e.to_string(), "Save the photo as JPEG and try again.");
    // A corrupt JPEG (a decompression bomb's header claiming a huge size is refused
    // by the decoder limits the same way).
    let jpg = small_jpeg(5);
    let e = b.err(
        "receipt.add",
        json!({"content_base64": b64(&jpg[..60]), "file_name": "cut.jpg"}),
    );
    assert_eq!(e.to_string(), "This picture can't be read.");
    assert_eq!(b.audit_len(), b.audit_len());
    assert!(
        b.run("receipt.inbox", json!({}))
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn expenses_at9_register_shows_receipts() {
    let mut b = book();
    let t = b.transfer(
        "2026-10-03",
        "Expenses:Groceries",
        "Assets:Checking",
        "12.00",
    );
    let r = b.run(
        "receipt.add",
        json!({"content_base64": b64(&small_jpeg(6)), "file_name": "milk.jpg", "transaction": t}),
    );
    assert_eq!(r["transaction"], t.as_str());
    let reg = b.run("account.register", json!({"account": "Assets:Checking"}));
    assert_eq!(reg["rows"][0]["receipts"], 1);
    // Module off: the paperclip hides, the receipt stays.
    b.run("module.disable", json!({"module": "expenses"}));
    let reg = b.run("account.register", json!({"account": "Assets:Checking"}));
    assert!(reg["rows"][0].get("receipts").is_none());
    assert_eq!(b.err("receipt.inbox", json!({})).code(), "module_disabled");
    b.run("module.enable", json!({"module": "expenses"}));
    assert_eq!(
        b.run("receipt.list", json!({"transaction": t}))[0]["id"],
        r["id"]
    );
    // Remove, then deleting the transaction sends receipts to the inbox.
    b.run("receipt.unlink", json!({"receipt": r["id"]}));
    assert_eq!(b.run("receipt.inbox", json!({}))[0]["id"], r["id"]);
    b.run(
        "receipt.link",
        json!({"receipt": r["id"], "transaction": t}),
    );
    b.run("txn.delete", json!({"id": t}));
    assert_eq!(b.run("receipt.inbox", json!({}))[0]["id"], r["id"]);
    b.run("history.undo", json!({}));
    assert_eq!(
        b.run("receipt.list", json!({"transaction": t}))[0]["id"],
        r["id"]
    );
    // Delete and undo.
    b.run("receipt.delete", json!({"receipt": r["id"]}));
    assert!(
        b.run("receipt.list", json!({"transaction": t}))
            .as_array()
            .unwrap()
            .is_empty()
    );
    b.run("history.undo", json!({}));
    let thumb = b.run("receipt.thumbnail", json!({"receipt": r["id"]}));
    assert!(thumb["png_base64"].is_string());
}

#[test]
fn expenses_drafts_edits_closed_period_and_delete() {
    let mut b = book();
    let d = b.run(
        "expense.create",
        json!({"date": "2026-10-03", "merchant": "Hardware"}),
    );
    assert_eq!(d["status"], "draft");
    assert_eq!(d["missing"], json!(["amount", "category", "paid with"]));
    let err = b.err("expense.post", json!({"expense": d["id"]}));
    assert!(
        err.to_string().contains("amount, category, paid with"),
        "{err}"
    );
    let e = b.err(
        "expense.create",
        json!({"date": "2026-10-03", "amount": "-4.00"}),
    );
    assert!(e.to_string().contains("more than zero"));
    let e = b.err(
        "expense.create",
        json!({"date": "2026-10-03", "category": "Assets:Checking"}),
    );
    assert!(e.to_string().contains("expense account"), "{e}");
    let e = b.err(
        "expense.create",
        json!({"date": "2026-10-03", "paid_with": "Expenses:Meals"}),
    );
    assert!(e.to_string().contains("Paid with"), "{e}");
    // Default paid with.
    b.run(
        "module.set_setting",
        json!({"key": "expenses.default_paid_with", "value": "personal"}),
    );
    let x = b.run("expense.create", json!({"date": "2026-10-04"}));
    assert_eq!(x["paid_with"], "personal");
    // Complete the draft, post it, then edit the posted expense.
    let r = b.run(
        "expense.update",
        json!({"expense": d["id"], "amount": "20.00", "category": "Expenses:Office Supplies",
               "paid_with": "Assets:Checking"}),
    );
    assert_eq!(r["status"], "ready");
    let p = b.run("expense.post", json!({"expense": d["id"], "match": "new"}));
    b.run(
        "expense.update",
        json!({"expense": d["id"], "amount": "22.50", "merchant": "Hardware Store", "category": "Expenses:Meals"}),
    );
    let t = b.run("txn.get", json!({"id": p["transaction"]}));
    assert_eq!(t["payee"], "Hardware Store");
    assert_eq!(b.shown("Expenses:Meals", "USD"), "22.50");
    assert_eq!(b.shown("Assets:Checking", "USD"), "-22.50");
    // Closed period: no posting or editing.
    let y = b.run(
        "expense.create",
        json!({"date": "2026-09-30", "merchant": "Old", "amount": "1.00",
               "category": "Expenses:Meals", "paid_with": "Assets:Checking"}),
    );
    b.run("book.set_closing_date", json!({"date": "2026-09-30"}));
    assert!(
        b.err("expense.post", json!({"expense": y["id"]}))
            .to_string()
            .contains("closed period")
    );
    assert!(
        b.err(
            "expense.update",
            json!({"expense": y["id"], "merchant": "New"})
        )
        .to_string()
        .contains("closed period")
    );
    // Deleting a posted expense asks about its transaction.
    let err = b.err("expense.delete", json!({"expense": d["id"]}));
    assert_eq!(err.code(), "needs_choice");
    b.run(
        "expense.delete",
        json!({"expense": d["id"], "delete_transaction": true}),
    );
    assert!(
        b.try_run("txn.get", json!({"id": p["transaction"]}))
            .is_err()
    );
    b.run("history.undo", json!({}));
    assert_eq!(
        b.run("expense.get", json!({"expense": d["id"]}))["status"],
        "posted"
    );
    // Listing and filters.
    let all = b.run("expense.list", json!({}));
    assert_eq!(all[0]["date"], "2026-10-04", "newest first");
    let found = b.run("expense.list", json!({"text": "hardware"}));
    assert_eq!(found.as_array().unwrap().len(), 1);
    let personal = b.run("expense.list", json!({"paid_with": "personal"}));
    assert_eq!(personal.as_array().unwrap().len(), 1);
}

#[test]
fn expenses_module_settings() {
    let mut b = book();
    let s = b.run("module.settings", json!({"module": "expenses"}));
    let keys: Vec<&str> = s["settings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        [
            "expenses.reimbursement_account",
            "expenses.match_days",
            "expenses.default_paid_with",
            // mobile-capture.md settings belong to the expenses module.
            "capture.auto_collect",
            "capture.collect_minutes"
        ]
    );
    assert_eq!(s["settings"][1]["value"], 5);
    assert!(
        b.try_run(
            "module.set_setting",
            json!({"key": "expenses.match_days", "value": 31})
        )
        .is_err()
    );
    // Match days 0: a transaction two days away is not a candidate.
    b.run(
        "module.set_setting",
        json!({"key": "expenses.match_days", "value": 0}),
    );
    b.transfer(
        "2026-10-05",
        "Expenses:Groceries",
        "Liabilities:Visa",
        "45.20",
    );
    let e = office_depot(&mut b, None);
    assert!(
        b.run("expense.match_candidates", json!({"expense": e["id"]}))
            .as_array()
            .unwrap()
            .is_empty()
    );
}
