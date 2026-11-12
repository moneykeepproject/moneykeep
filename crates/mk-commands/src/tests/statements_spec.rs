//! business-dimensions.md acceptance tests 7–9 (statements, late fees, templates).

use super::sales_spec::{book, invoice, pay};
use super::*;
use base64::Engine;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn pdf_text(v: &Value) -> String {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(v["content_base64"].as_str().unwrap())
        .unwrap();
    bytes.iter().map(|b| char::from(*b)).collect()
}

const ACTIVE: [&str; 8] = [
    "/JavaScript",
    "/JS",
    "/AA",
    "/OpenAction",
    "/Launch",
    "/URI",
    "/EmbeddedFile",
    "/AcroForm",
];

fn sales_with_statements() -> Book {
    let mut b = book();
    b.run("module.enable", json!({"module": "statements"}));
    invoice(&mut b);
    pay(&mut b, "Acme LLC", "2026-09-20", "1000.00");
    b
}

#[test]
fn at7_customer_statement() {
    let mut b = sales_with_statements();
    let s = b.run(
        "statement.render",
        json!({"customer": "Acme LLC", "from": "2026-09-01", "to": "2026-10-31"}),
    );
    assert_eq!(s["opening"], "0.00");
    let rows = s["rows"].as_array().unwrap();
    assert_eq!(rows[0]["kind"], "Invoice");
    assert_eq!(rows[0]["amount"], "1314.32");
    assert_eq!(rows[1]["kind"], "Payment");
    assert_eq!(rows[1]["amount"], "-1000.00");
    assert_eq!(s["closing"], "314.32");
    assert_eq!(
        s["aging"]["buckets"],
        json!(["0.00", "314.32", "0.00", "0.00", "0.00"])
    );
    let cb = b.run("report.customer_balances", json!({"as_of": "2026-10-31"}));
    assert_eq!(cb["total"], s["closing"]);
    let text = pdf_text(&s);
    assert!(text.starts_with("%PDF-1.4"));
    assert!(text.contains("(Statement)") && text.contains("314.32"));
    for k in ACTIVE {
        assert!(!text.contains(k), "{k}");
    }
    let open = b.run(
        "statement.render",
        json!({"customer": "Acme LLC", "from": "2026-09-01", "to": "2026-10-31", "kind": "open_items"}),
    );
    assert_eq!(open["closing"], "314.32");
    // Batch: one per customer with a balance, or one combined file.
    let batch = b.run(
        "statement.batch",
        json!({"to": "2026-10-31", "from": "2026-09-01"}),
    );
    assert_eq!(batch["files"].as_array().unwrap().len(), 1);
    let combined = b.run(
        "statement.batch",
        json!({"to": "2026-10-31", "combined": true}),
    );
    assert!(pdf_text(&combined["pdf"]).contains("(Statement)"));
}

#[test]
fn at8_late_fees() {
    let mut b = sales_with_statements();
    for (k, v) in [
        ("statements.late_fee_kind", json!("percent")),
        ("statements.late_fee_percent", json!("1.5")),
        ("statements.late_fee_min", json!("2.00")),
        ("statements.grace_days", json!(5)),
    ] {
        b.run("module.set_setting", json!({"key": k, "value": v}));
    }
    let p = b.run("late_fee.preview", json!({"as_of": "2026-10-05"}));
    assert!(p["fees"].as_array().unwrap().is_empty());
    let p = b.run("late_fee.preview", json!({"as_of": "2026-10-10"}));
    let fees = p["fees"].as_array().unwrap();
    assert_eq!(fees.len(), 1);
    assert_eq!(fees[0]["number"], "INV-1001");
    assert_eq!(fees[0]["balance"], "314.32");
    assert_eq!(fees[0]["fee"], "4.71");
    assert_eq!(fees[0]["days_late"], 9);
    let out = b.run("late_fee.assess", json!({"as_of": "2026-10-10"}));
    let inv = &out["invoices"][0];
    assert_eq!(inv["totals"]["total"], "4.71");
    assert_eq!(
        inv["totals"]["lines"][0]["description"],
        "Late fee — INV-1001"
    );
    assert_eq!(b.shown("Late Fee Income", "USD"), "4.71");
    let p = b.run("late_fee.preview", json!({"as_of": "2026-10-25"}));
    assert!(p["fees"].as_array().unwrap().is_empty(), "{p}");
    // No fee on the late-fee invoice itself, even much later.
    let p = b.run("late_fee.preview", json!({"as_of": "2026-12-31"}));
    assert!(
        p["fees"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["number"] == "INV-1001")
    );
    // The statement shows the late fee.
    let s = b.run(
        "statement.render",
        json!({"customer": "Acme LLC", "from": "2026-09-01", "to": "2026-10-31"}),
    );
    assert_eq!(
        s["rows"].as_array().unwrap().last().unwrap()["kind"],
        "Late fee"
    );
    assert_eq!(s["closing"], "319.03");
    // Undo removes the fee invoice.
    b.run("history.undo", json!({}));
    assert_eq!(
        b.run("late_fee.preview", json!({"as_of": "2026-10-10"}))["fees"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn at9_template_with_logo() {
    let mut b = book();
    invoice(&mut b);
    b.run(
        "template.create",
        json!({"name": "Brand", "accent_color": "#1F6F78", "footer": "Thank you for your business",
               "font": "times", "payment_instructions": "Pay by bank transfer",
               "title_overrides": {"invoice": "Tax invoice"}}),
    );
    let logo = mk_import::image::solid_png(300, 100, [0x1F, 0x6F, 0x78]);
    let t = b.run(
        "template.set_logo",
        json!({"template": "Brand", "content_base64": b64(&logo)}),
    );
    assert_eq!(t["logo"], json!({"width": 300, "height": 100}));
    // The editor's preview is scaled down to fit 240 × 120.
    let g = b.run("template.get", json!({"template": "Brand"}));
    assert_eq!(g["logo_preview"]["width"], 150);
    assert_eq!(g["logo_preview"]["height"], 50);
    let px = base64::engine::general_purpose::STANDARD
        .decode(g["logo_preview"]["rgb_base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(px.len(), 150 * 50 * 3);
    assert_eq!(&px[..3], &[0x1F, 0x6F, 0x78]);
    // The audit entry keeps the size, not the file.
    let log = b.s.audit_entries(1).unwrap();
    assert!(log[0].args.to_string().contains("bytes"));
    b.run(
        "module.set_setting",
        json!({"key": "invoicing.default_template", "value": "Brand"}),
    );
    let pdf = b.run("invoice.render_pdf", json!({"invoice": "INV-1001"}));
    let text = pdf_text(&pdf);
    assert_eq!(text.matches("/Subtype /Image").count(), 1);
    assert!(text.contains("/Width 300 /Height 100"));
    assert!(text.contains("/Filter /FlateDecode"));
    assert!(text.contains("(Thank you for your business)"));
    assert!(text.contains("(Tax invoice)"));
    assert!(text.contains("/BaseFont /Times-Roman"));
    assert!(text.contains("0.121 0.435 0.470 rg"));
    assert!(text.contains("(Pay by bank transfer)"));
    for k in ACTIVE {
        assert!(!text.contains(k), "{k}");
    }
    // Rejected logos.
    let big = mk_import::image::solid_png(3000, 3000, [0, 0, 0]);
    let e = b.err(
        "template.set_logo",
        json!({"template": "Brand", "content_base64": b64(&big)}),
    );
    assert!(e.to_string().contains("2000"), "{e}");
    let huge = vec![0x89_u8; 600 * 1024];
    let e = b.err(
        "template.set_logo",
        json!({"template": "Brand", "content_base64": b64(&huge)}),
    );
    assert!(e.to_string().contains("512 KB"), "{e}");
    let e = b.err(
        "template.set_logo",
        json!({"template": "Brand", "content_base64": b64(b"GIF89a\x01\x00\x01\x00")}),
    );
    assert!(e.to_string().contains("PNG or JPEG"), "{e}");
    let mut corrupt = logo.clone();
    let idat = corrupt.windows(4).position(|w| w == b"IDAT").unwrap();
    for x in corrupt.iter_mut().skip(idat + 6).take(12) {
        *x ^= 0x5A;
    }
    assert!(
        b.try_run(
            "template.set_logo",
            json!({"template": "Brand", "content_base64": b64(&corrupt)})
        )
        .is_err()
    );
    // A template travels in definition files without its logo.
    let def = b.run(
        "definition.export",
        json!({"id": "com.example.brand", "version": "1.0.0", "name": "Brand", "document_templates": ["Brand"]}),
    );
    let content = def["content"].as_str().unwrap();
    assert!(content.contains("[[document_template]]") && content.contains("#1F6F78"));
    assert!(!content.contains("logo"));
    b.run("template.clear_logo", json!({"template": "Brand"}));
    let pdf = b.run("invoice.render_pdf", json!({"invoice": "INV-1001"}));
    assert!(!pdf_text(&pdf).contains("/Subtype /Image"));
    // Statements and purchase orders use it too.
    b.run("module.enable", json!({"module": "statements"}));
    let s = b.run(
        "statement.render",
        json!({"customer": "Acme LLC", "from": "2026-09-01", "to": "2026-10-31"}),
    );
    assert!(pdf_text(&s).contains("(Thank you for your business)"));
}
