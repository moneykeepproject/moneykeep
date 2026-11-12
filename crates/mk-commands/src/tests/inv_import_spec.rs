//! import-investments.md acceptance tests 1–24.

use base64::Engine;

use super::*;

fn fixture(path: &str) -> Vec<u8> {
    let p = format!(
        "{}/../../examples/fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn book() -> Book {
    let mut b = Book::new();
    b.standard();
    b.run("module.enable", json!({"module": "investments"}));
    b.account(
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    b
}

fn stage(b: &mut Book, name: &str, bytes: &[u8], account: Option<&str>) -> Value {
    let mut args = json!({"name": name, "content_base64": b64(bytes)});
    if let Some(a) = account {
        args["account"] = json!(a);
    }
    b.run("import.stage", args)
}

fn ofx(b: &mut Book) -> Value {
    stage(
        b,
        "investment-v2.ofx",
        &fixture("ofx/investment-v2.ofx"),
        Some("Assets:Brokerage"),
    )
}

fn inv_rows(batch: &Value) -> Vec<Value> {
    batch["inv_rows"].as_array().unwrap().clone()
}

fn row<'a>(rows: &'a [Value], action: &str) -> &'a Value {
    rows.iter()
        .find(|r| r["inv_action"] == action)
        .unwrap_or_else(|| panic!("no {action} row in {rows:?}"))
}

#[test]
fn inv_import_at1_ofx_section_securities_option_and_cash() {
    let mut b = book();
    let batch = ofx(&mut b);
    let secs = batch["sections"].as_array().unwrap();
    assert_eq!(secs.len(), 1, "{batch}");
    assert_eq!(secs[0]["kind"], "investment");
    assert_eq!(secs[0]["account_hint"], "2333");
    let vti = secs[0]["securities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "CUSIP:922908769")
        .unwrap()
        .clone();
    assert_eq!(vti["how"], "new");
    assert_eq!(vti["proposed"]["symbol"], "VTI");
    assert_eq!(vti["proposed"]["name"], "Total Stock Market ETF");
    assert_eq!(vti["cusip"], "922908769");
    let rows = inv_rows(&batch);
    let opt = rows
        .iter()
        .find(|r| r["row"]["raw"].as_str().unwrap().starts_with("BUYOPT"))
        .unwrap();
    assert_eq!(opt["class"], "unsupported");
    assert!(
        opt["notes"][0]
            .as_str()
            .unwrap()
            .contains("options are not supported")
    );
    // The deposit is a cash row (an ordinary bank row of the section).
    let cash = batch["rows"].as_array().unwrap();
    assert_eq!(cash.len(), 1);
    assert_eq!(cash[0]["amount"], "5000.00");
}

#[test]
fn inv_import_at2_buy_and_sell_rows() {
    let mut b = book();
    let rows = inv_rows(&ofx(&mut b));
    let buy = row(&rows, "buy");
    assert_eq!(buy["quantity"], "10");
    assert_eq!(buy["price"], "200");
    assert_eq!(buy["commission"], "5.00");
    assert_eq!(buy["amount"], "2005.00");
    let sell = row(&rows, "sell");
    assert_eq!(sell["quantity"], "12");
    assert_eq!(sell["amount"], "2995.00");
}

fn commit(b: &mut Book, batch: &Value) -> Value {
    b.run("import.commit", json!({"batch": batch["id"]}))
}

#[test]
fn inv_import_at3_commit_through_actions_one_entry_undo_everything() {
    let mut b = book();
    let before = b.state();
    let batch = ofx(&mut b);
    let audit = b.audit_len();
    let done = commit(&mut b, &batch);
    assert_eq!(b.audit_len(), audit + 1, "one audit entry");
    assert_eq!(done["created"], 7, "{done}");
    let rg = b.run(
        "report.realized_gains",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    assert_eq!(rg["total"]["gain"], "550.00");
    assert_eq!(rg["sections"][0]["gain"], "59.17", "short-term");
    assert_eq!(rg["sections"][1]["gain"], "490.83", "long-term");
    let h = b.run("inv.holdings", json!({"as_of": "2026-10-19"}));
    let vti = &h["accounts"][0]["holdings"][0];
    assert_eq!(vti["symbol"], "VTI");
    assert_eq!(vti["quantity"], "6.1");
    assert_eq!(vti["basis"], "682.00");
    assert_eq!(vti["market_value"], "762.50");
    let s = b.run("security.get", json!({"security": "VTI"}));
    assert_eq!(s["cusip"], "922908769");
    // Rows from a download start cleared.
    let t = b.run("txn.list", json!({"account": "Assets:Brokerage"}));
    assert!(t.to_string().contains("\"reconcile\":\"c\""));
    b.run("history.undo", json!({}));
    // Undo of the commit; then undo of the staging.
    b.run("history.undo", json!({}));
    assert_eq!(
        b.state(),
        before,
        "undo removes everything including the security"
    );
    assert_eq!(b.run("security.list", json!({})), json!([]));
}

#[test]
fn inv_import_at4_restaging_gives_duplicates() {
    let mut b = book();
    let batch = ofx(&mut b);
    commit(&mut b, &batch);
    let again = ofx(&mut b);
    for r in inv_rows(&again) {
        let class = r["class"].as_str().unwrap();
        assert!(class == "duplicate" || class == "unsupported", "{r}");
    }
    assert_eq!(again["rows"][0]["class"], "duplicate");
}

fn ofx_with(body: &str) -> Vec<u8> {
    let base = String::from_utf8(fixture("ofx/investment-v2.ofx")).unwrap();
    let start = base.find("<BUYSTOCK>").unwrap();
    let end = base.find("</INVTRANLIST>").unwrap();
    format!("{}{body}{}", &base[..start], &base[end..])
        .replace("<INVBANKTRAN>", "<!--x-->")
        .into_bytes()
}

#[test]
fn inv_import_at5_reversal_proposes_voiding_the_buy() {
    let mut b = book();
    let batch = ofx(&mut b);
    commit(&mut b, &batch);
    let rev = ofx_with(
        "<BUYSTOCK><INVBUY><INVTRAN><FITID>INV-0099</FITID><DTTRADE>20250312</DTTRADE><REVERSALFITID>INV-0002</REVERSALFITID></INVTRAN>\
         <SECID><UNIQUEID>922908769</UNIQUEID><UNIQUEIDTYPE>CUSIP</UNIQUEIDTYPE></SECID><UNITS>10</UNITS><UNITPRICE>200.00</UNITPRICE>\
         <COMMISSION>5.00</COMMISSION><TOTAL>-2005.00</TOTAL></INVBUY><BUYTYPE>BUY</BUYTYPE></BUYSTOCK>",
    );
    let batch = stage(&mut b, "rev.ofx", &rev, Some("Assets:Brokerage"));
    let rows = inv_rows(&batch);
    let r = rows.iter().find(|r| r["class"] == "reverse").unwrap();
    assert_eq!(r["action"], "void");
    let buys = b.run("txn.list", json!({"account": "Assets:Brokerage:VTI"}));
    let buy_id = buys
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["date"] == "2025-03-10")
        .unwrap()["id"]
        .clone();
    assert_eq!(r["link"], buy_id);
}

#[test]
fn inv_import_at6_prices_from_the_file() {
    let mut b = book();
    let batch = ofx(&mut b);
    commit(&mut b, &batch);
    let p = b.run("price.list", json!({"security": "VTI"}));
    // The SECLIST price, plus each buy, sell and reinvest trade price (revision 4).
    assert_eq!(
        p,
        json!([
            {"date": "2025-03-10", "price": "200.00"},
            {"date": "2026-04-15", "price": "250.00"},
            {"date": "2026-09-30", "price": "220.00"},
            {"date": "2026-10-16", "price": "125.00"}
        ])
    );
    let mut b = book();
    b.run(
        "setting.set",
        json!({"module": "core", "key": "import.store_prices", "value": false}),
    );
    let batch = ofx(&mut b);
    commit(&mut b, &batch);
    assert_eq!(b.run("price.list", json!({"security": "VTI"})), json!([]));
}

#[test]
fn inv_import_at7_positions_check() {
    let mut b = book();
    let batch = ofx(&mut b);
    commit(&mut b, &batch);
    let got = b.run("import.get", json!({"batch": batch["id"]}));
    let pc = &got["sections"][0]["positions_check"];
    let vti = pc["positions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["security"] == "VTI")
        .unwrap()
        .clone();
    assert_eq!(vti["agree"], true, "{pc}");
    // One share missing from the book.
    let text = String::from_utf8(fixture("ofx/investment-v2.ofx"))
        .unwrap()
        .replace("<UNITS>6.1</UNITS>", "<UNITS>7.1</UNITS>");
    let again = stage(&mut b, "x.ofx", text.as_bytes(), Some("Assets:Brokerage"));
    let pc = &again["sections"][0]["positions_check"];
    assert_eq!(pc["agree"], false);
    let d = &pc["differences"][0];
    assert_eq!(
        (
            d["security"].as_str(),
            d["book"].as_str(),
            d["statement"].as_str()
        ),
        (Some("VTI"), Some("6.1"), Some("7.1"))
    );
}

#[test]
fn inv_import_at8_misc_income_and_cglong() {
    let mut b = book();
    let body = "<INCOME><INVTRAN><FITID>M1</FITID><DTTRADE>20260101</DTTRADE></INVTRAN>\
        <SECID><UNIQUEID>922908769</UNIQUEID><UNIQUEIDTYPE>CUSIP</UNIQUEIDTYPE></SECID><INCOMETYPE>MISC</INCOMETYPE><TOTAL>3.00</TOTAL></INCOME>\
        <INCOME><INVTRAN><FITID>M2</FITID><DTTRADE>20260102</DTTRADE></INVTRAN>\
        <SECID><UNIQUEID>922908769</UNIQUEID><UNIQUEIDTYPE>CUSIP</UNIQUEIDTYPE></SECID><INCOMETYPE>CGLONG</INCOMETYPE><TOTAL>7.00</TOTAL></INCOME>";
    let batch = stage(&mut b, "inc.ofx", &ofx_with(body), Some("Assets:Brokerage"));
    let rows = inv_rows(&batch);
    let misc = rows
        .iter()
        .find(|r| r["row"]["external_id"] == "M1")
        .unwrap();
    assert_eq!(misc["needs_review"], true);
    let cg = rows
        .iter()
        .find(|r| r["row"]["external_id"] == "M2")
        .unwrap();
    assert_eq!(cg["row"]["kind"], "capital_gain_long");
    assert_eq!(cg["needs_review"], false);
}

const JSON_FILE: &str = r#"{"data":{"transactions":[{"tradeDate":"10/01/2026","activity":"Bought","symbol":"VTI","shares":"10","price":"200","fees":"5","netAmount":"-2005.00","id":"T1"}]}}"#;

fn assert_same_buy(rows: &[Value]) {
    let buy = row(rows, "buy");
    assert_eq!(buy["quantity"], "10");
    assert_eq!(buy["price"], "200");
    assert_eq!(buy["commission"], "5.00");
    assert_eq!(buy["amount"], "2005.00");
    assert_eq!(buy["row"]["external_id"], "T1");
}

fn profile(format: &str, records: &str, fields: Value) -> Value {
    json!({"profile": {
        "name": format!("Test {format}"), "format": format, "signature": [], "records": records,
        "fields": fields, "date_format": "MM/DD/YYYY",
        "actions": [{"match_type": "equals", "text": "Bought", "action": "buy"}]
    }})
}

#[test]
fn inv_import_at9_json_profile_and_suggest() {
    let mut b = book();
    let s = b.run("inv_profile.suggest", json!({"content": JSON_FILE}));
    assert_eq!(s["format"], "json");
    assert_eq!(s["records"], "data.transactions");
    let f = &s["fields"];
    for (k, v) in [
        ("date", "tradeDate"),
        ("action", "activity"),
        ("symbol", "symbol"),
        ("quantity", "shares"),
        ("price", "price"),
        ("fees", "fees"),
        ("amount", "netAmount"),
        ("external_id", "id"),
    ] {
        assert_eq!(f[k], v, "{k}: {s}");
    }
    assert_eq!(s["action_texts"], json!(["Bought"]));
    assert_eq!(s["date_format"], "MM/DD/YYYY");
    // Without a profile the file needs one.
    let need = b.run(
        "import.stage",
        json!({"name": "a.json", "content": JSON_FILE}),
    );
    assert!(need.get("needs_inv_profile").is_some());
    let mut p = profile("json", "data.transactions", f.clone());
    p["profile"]["signature"] = json!(["data"]);
    b.run("inv_profile.save", p);
    let batch = b.run(
        "import.stage",
        json!({"name": "a.json", "content": JSON_FILE, "account": "Assets:Brokerage"}),
    );
    assert_same_buy(&inv_rows(&batch));
}

#[test]
fn inv_import_at10_xml_profile_and_dtd_rejected() {
    let mut b = book();
    let xml = r#"<?xml version="1.0"?><Statement><Transactions><Transaction id="T1"><TradeDate>10/01/2026</TradeDate><Activity>Bought</Activity><Symbol>VTI</Symbol><Shares>10</Shares><Price>200</Price><Fees>5</Fees><NetAmount>-2005.00</NetAmount></Transaction></Transactions></Statement>"#;
    let mut p = profile(
        "xml",
        "Statement/Transactions/Transaction",
        json!({"date": "TradeDate", "action": "Activity", "symbol": "Symbol", "quantity": "Shares",
               "price": "Price", "fees": "Fees", "amount": "NetAmount", "external_id": "@id"}),
    );
    p["profile"]["signature"] = json!(["Statement"]);
    b.run("inv_profile.save", p);
    let batch = b.run(
        "import.stage",
        json!({"name": "a.xml", "content": xml, "account": "Assets:Brokerage"}),
    );
    assert_same_buy(&inv_rows(&batch));
    let evil = r#"<?xml version="1.0"?><!DOCTYPE Statement [<!ENTITY x SYSTEM "file:///etc/passwd">]><Statement><Transactions><Transaction id="&x;"/></Transactions></Statement>"#;
    let e = b.err(
        "import.stage",
        json!({"name": "a.xml", "content": evil, "account": "Assets:Brokerage"}),
    );
    assert!(e.to_string().contains("DTD"), "{e}");
}

#[test]
fn inv_import_at11_csv_profile_and_unknown_action() {
    let mut b = book();
    let csv = "Trade Date,Activity,Symbol,Shares,Price,Fees,Net Amount,Id\n10/01/2026,Bought,VTI,10,200,5,-2005.00,T1\n10/02/2026,Gifted,VTI,1,,,,T2\n";
    let s = b.run("inv_profile.suggest", json!({"content": csv}));
    assert_eq!(s["format"], "csv");
    let mut p = profile("csv", "", s["fields"].clone());
    p["profile"]["signature"] = json!(["Trade Date", "Activity", "Net Amount"]);
    b.run("inv_profile.save", p);
    let batch = b.run(
        "import.stage",
        json!({"name": "a.csv", "content": csv, "account": "Assets:Brokerage"}),
    );
    let rows = inv_rows(&batch);
    assert_same_buy(&rows);
    let u = rows
        .iter()
        .find(|r| r["row"]["external_id"] == "T2")
        .unwrap();
    assert_eq!(u["class"], "unsupported");
    assert_eq!(u["notes"][0], "Unknown action: Gifted");
    // The preview shows the same rows.
    let prof = b.run("inv_profile.get", json!({"name": "Test csv"}));
    let pv = b.run(
        "inv_profile.preview",
        json!({"profile": prof, "content": csv}),
    );
    assert_eq!(pv["rows"][0]["action"], "buy");
    assert_eq!(pv["rows"][1]["action"], "unsupported");
}

// ----- revision 2: the broker's files -------------------------------------------------

const NAME: &str = "Brokerage_XXX039_Transactions_20261020-120000";

fn schwab(b: &mut Book, ext: &str, account: Option<&str>) -> Value {
    let bytes = fixture(&format!("broker/schwab-like.{ext}"));
    stage(b, &format!("{NAME}.{ext}"), &bytes, account)
}

fn summary(batch: &Value) -> Vec<(String, String, String, String, String)> {
    inv_rows(batch)
        .iter()
        .map(|r| {
            (
                r["date"].as_str().unwrap_or("").to_owned(),
                r["inv_action"].as_str().unwrap_or("").to_owned(),
                r["row"]["security"].as_str().unwrap_or("").to_owned(),
                r["quantity"].as_str().unwrap_or("").to_owned(),
                r["amount"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect()
}

#[test]
fn inv_import_at12_builtin_profiles_recognise_all_three() {
    let mut b = book();
    let csv = schwab(&mut b, "csv", Some("Assets:Brokerage"));
    let json_ = schwab(&mut b, "json", Some("Assets:Brokerage"));
    let xml = schwab(&mut b, "xml", Some("Assets:Brokerage"));
    for (batch, name) in [
        (&csv, "Charles Schwab — CSV"),
        (&json_, "Charles Schwab — JSON"),
        (&xml, "Charles Schwab — XML"),
    ] {
        assert_eq!(batch["sections"][0]["profile"], name, "{batch}");
    }
    assert!(!summary(&csv).is_empty());
    assert_eq!(summary(&csv), summary(&json_));
    assert_eq!(summary(&csv), summary(&xml));
    assert_eq!(csv["rows"].as_array().unwrap().len(), 0, "no cash rows");
}

fn cd_book() -> (Book, Value) {
    let mut b = book();
    let batch = schwab(&mut b, "json", Some("Assets:Brokerage"));
    (b, batch)
}

#[test]
fn inv_import_at13_cd_buy_priced_per_100() {
    let (mut b, batch) = cd_book();
    let rows = inv_rows(&batch);
    let buy = rows
        .iter()
        .find(|r| r["inv_action"] == "buy" && r["row"]["security"] == "CUSIP:11223EXA4")
        .unwrap();
    assert_eq!(buy["quantity"], "25000");
    assert_eq!(buy["price"], "100");
    assert_eq!(buy["amount"], "25000.00");
    let sec = batch["sections"][0]["securities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == "CUSIP:11223EXA4")
        .unwrap()
        .clone();
    assert_eq!(sec["proposed"]["kind"], "cd");
    assert_eq!(sec["proposed"]["symbol"], "11223EXA4");
    assert_eq!(
        sec["proposed"]["name"],
        "EXAMPLE SAVINGS 4.10%27CD FDIC INS DUE 04/01/27"
    );
    // Skip the maturity without a buy, then commit.
    let lone = rows
        .iter()
        .find(|r| r["row"]["security"] == "CUSIP:99887EXC3")
        .unwrap()["id"]
        .clone();
    b.run(
        "import.set_inv_row",
        json!({"batch": batch["id"], "row": lone, "skip": true}),
    );
    commit(&mut b, &batch);
    b.run(
        "price.set",
        json!({"security": "11223EXA4", "date": "2026-10-01", "price": "100"}),
    );
    let h = b.run("inv.holdings", json!({"as_of": "2026-10-19"}));
    let cd = h["accounts"][0]["holdings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["symbol"] == "11223EXA4")
        .unwrap()
        .clone();
    assert_eq!(cd["quantity"], "25000");
    assert_eq!(cd["price"], "100.00");
    assert_eq!(cd["market_value"], "25000.00");
    assert_eq!(cd["basis"], "25000.00");
}

#[test]
fn inv_import_at14_maturity_pairs_into_one_sell_and_cd_interest() {
    let (mut b, batch) = cd_book();
    let rows = inv_rows(&batch);
    let sells: Vec<&Value> = rows
        .iter()
        .filter(|r| r["row"]["security"] == "CUSIP:55667EXB2" && r["inv_action"] == "sell")
        .collect();
    assert_eq!(sells.len(), 1, "one sell for the pair");
    assert_eq!(sells[0]["quantity"], "25000");
    assert_eq!(sells[0]["amount"], "25000.00");
    assert_eq!(sells[0]["price"], "100");
    let interest = rows
        .iter()
        .find(|r| r["inv_action"] == "interest" && r["row"]["security"] == "CUSIP:55667EXB2")
        .unwrap();
    assert_eq!(interest["amount"], "247.88");
    let lone = rows
        .iter()
        .find(|r| r["row"]["security"] == "CUSIP:99887EXC3")
        .unwrap()["id"]
        .clone();
    b.run(
        "import.set_inv_row",
        json!({"batch": batch["id"], "row": lone, "skip": true}),
    );
    commit(&mut b, &batch);
    let rg = b.run(
        "report.realized_gains",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    assert_eq!(rg["total"]["proceeds"], "25000.00");
    assert_eq!(rg["total"]["gain"], "0.00");
    assert_eq!(b.shown("Interest", "USD"), "248.20", "247.88 + 0.32");
}

#[test]
fn inv_import_at15_maturity_without_buy_proposes_move_shares_in() {
    let (mut b, batch) = cd_book();
    let rows = inv_rows(&batch);
    let lone = rows
        .iter()
        .find(|r| r["row"]["security"] == "CUSIP:99887EXC3")
        .unwrap()
        .clone();
    assert_eq!(lone["inv_action"], "sell");
    assert_eq!(lone["needs_review"], true);
    assert_eq!(lone["proposal"]["quantity"], "10000");
    assert_eq!(lone["proposal"]["basis"], "10000");
    // Accepting it lets the commit through: shares moved in, then sold at face.
    b.run(
        "import.set_inv_row",
        json!({"batch": batch["id"], "row": lone["id"], "move_in": true}),
    );
    commit(&mut b, &batch);
    let rg = b.run(
        "report.realized_gains",
        json!({"from": "2026-01-01", "to": "2026-12-31"}),
    );
    assert_eq!(rg["total"]["proceeds"], "35000.00");
    assert_eq!(rg["total"]["gain"], "0.00");
}

#[test]
fn inv_import_at16_option_unsupported_and_at17_credit_interest() {
    let (_, batch) = cd_book();
    let rows = inv_rows(&batch);
    let opt = rows
        .iter()
        .find(|r| r["row"]["memo"].as_str().unwrap().starts_with("PUT"))
        .unwrap();
    assert_eq!(opt["class"], "unsupported");
    assert_eq!(opt["notes"][0], "options are not supported yet");
    let ci = rows
        .iter()
        .find(|r| r["inv_action"] == "interest" && r["amount"] == "0.32")
        .unwrap();
    assert_eq!(ci["row"]["security"], Value::Null);
}

#[test]
fn inv_import_at18_file_total_check() {
    let mut b = book();
    let ok = schwab(&mut b, "json", Some("Assets:Brokerage"));
    assert!(
        !ok["warnings"].to_string().contains("total"),
        "{}",
        ok["warnings"]
    );
    for ext in ["json", "xml"] {
        let text = String::from_utf8(fixture(&format!("broker/schwab-like.{ext}")))
            .unwrap()
            .replace("$247.88", "$248.88");
        let bad = stage(
            &mut b,
            &format!("{NAME}.{ext}"),
            text.as_bytes(),
            Some("Assets:Brokerage"),
        );
        assert!(
            bad["warnings"].to_string().contains("differs from the sum"),
            "{ext}: {}",
            bad["warnings"]
        );
    }
}

#[test]
fn inv_import_at19_account_from_file_name_is_remembered() {
    let mut b = book();
    let batch = schwab(&mut b, "json", None);
    assert_eq!(batch["sections"][0]["account_key"], "039");
    assert_eq!(batch["sections"][0]["resolved"], false);
    b.run(
        "import.map_account",
        json!({"batch": batch["id"], "section": 0, "account": "Assets:Brokerage", "remember": true}),
    );
    let next = schwab(&mut b, "csv", None);
    assert_eq!(next["sections"][0]["resolved"], true);
}

#[test]
fn inv_import_unverified_rule_needs_review_once() {
    let mut b = book();
    let csv = "\"Date\",\"Action\",\"Symbol\",\"Description\",\"Quantity\",\"Price\",\"Fees & Comm\",\"Amount\"\n\
               \"10/05/2026\",\"Qualified Dividend\",\"VTI\",\"TOTAL STOCK MARKET ETF\",\"\",\"\",\"\",\"$12.00\"\n";
    let batch = stage(
        &mut b,
        "x_XXX039_y.csv",
        csv.as_bytes(),
        Some("Assets:Brokerage"),
    );
    let r = &inv_rows(&batch)[0];
    assert_eq!(r["inv_action"], "dividend");
    assert_eq!(r["row"]["kind"], "qualified");
    assert_eq!(r["needs_review"], true, "unverified text, first time");
    commit(&mut b, &batch);
    let csv2 = csv.replace("10/05/2026", "11/05/2026");
    let again = stage(
        &mut b,
        "x_XXX039_y.csv",
        csv2.as_bytes(),
        Some("Assets:Brokerage"),
    );
    assert_eq!(
        inv_rows(&again)[0]["needs_review"],
        false,
        "confirmed by the commit"
    );
}

#[test]
fn inv_import_module_off_skips_investment_sections() {
    let mut b = Book::new();
    b.standard();
    let batch = b.run(
        "import.stage",
        json!({"name": "investment-v2.ofx", "content_base64": b64(&fixture("ofx/investment-v2.ofx"))}),
    );
    assert_eq!(batch["sections"], json!([]));
    assert!(
        batch["warnings"]
            .to_string()
            .contains("investment statements (1)")
    );
}

/// A file reader that serves one file at any path.
struct OneFile(Vec<u8>);

impl FileReader for OneFile {
    fn read(&self, _path: &str) -> Result<Vec<u8>, String> {
        Ok(self.0.clone())
    }
}

#[test]
fn inv_import_at21_path_gives_the_file_name_to_the_parser() {
    let mut b = book();
    let reader = OneFile(fixture("broker/schwab-like.json"));
    let batch = b
        .r
        .invoke_with(
            &mut b.s,
            &b.a,
            "import.stage",
            json!({"path": "/home/owner/Downloads/Designated_Bene_Individual_XXX039_Transactions_x.json"}),
            Some(&reader),
        )
        .unwrap();
    let batch = b.run("import.get", json!({"batch": batch["id"]}));
    assert_eq!(batch["sections"][0]["account_key"], "039", "{batch}");
    b.run(
        "import.map_account",
        json!({"batch": batch["id"], "section": 0, "account": "Assets:Brokerage", "remember": true}),
    );
    let next = schwab(&mut b, "csv", None);
    assert_eq!(next["sections"][0]["resolved"], true);
}

#[test]
fn inv_import_at22_matured_marker_dropped_from_proposed_names() {
    let mut b = book();
    let json_file = r#"{"FromDate":"10/01/2026","ToDate":"10/20/2026","TotalTransactionsAmount":"$0.00","TotalFeesAndCommAmount":"$0.00","BrokerageTransactions":[
        {"Date":"10/02/2026","Action":"CD Deposit Adj","Symbol":"44556EXD7","Description":"CHARLES SCHWAB B 3.75XXX**MATURED** US","Quantity":"-5,000","Price":"","Fees & Comm":"","Amount":"","AcctgRuleCd":"1"}]}"#;
    let batch = stage(
        &mut b,
        &format!("{NAME}.json"),
        json_file.as_bytes(),
        Some("Assets:Brokerage"),
    );
    let secs = batch["sections"][0]["securities"].as_array().unwrap();
    let s = secs
        .iter()
        .find(|s| s["key"] == "CUSIP:44556EXD7")
        .unwrap_or_else(|| panic!("{batch}"));
    assert_eq!(s["proposed"]["name"], "CHARLES SCHWAB B 3.75", "{s}");
}

fn holding(h: &Value, symbol: &str) -> Value {
    h["accounts"][0]["holdings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["symbol"] == symbol)
        .unwrap_or_else(|| panic!("{h}"))
        .clone()
}

#[test]
fn inv_import_at23_trade_prices_stored_and_value_the_cd() {
    // The fixture's open CD is 25,000 face bought at 100 on 2026-10-01.
    let (mut b, batch) = cd_book();
    let lone = inv_rows(&batch)
        .iter()
        .find(|r| r["row"]["security"] == "CUSIP:99887EXC3")
        .unwrap()["id"]
        .clone();
    b.run(
        "import.set_inv_row",
        json!({"batch": batch["id"], "row": lone, "skip": true}),
    );
    commit(&mut b, &batch);
    let p = b.run(
        "price.latest",
        json!({"security": "11223EXA4", "as_of": "2026-10-19"}),
    );
    assert_eq!(p["date"], "2026-10-01");
    let h = b.run("inv.holdings", json!({"as_of": "2026-10-19"}));
    let cd = holding(&h, "11223EXA4");
    assert_eq!(cd["price"], "100.00");
    assert_eq!(cd["price_date"], "2026-10-01");
    assert!(cd["price_note"].is_null());
    assert_eq!(cd["market_value"], "25000.00");
    let acc = &h["accounts"][0];
    let cash: mk_core::Decimal = acc["cash"].as_str().unwrap().parse().unwrap();
    let total: mk_core::Decimal = acc["total"].as_str().unwrap().parse().unwrap();
    assert_eq!(total - cash, mk_core::Decimal::new(25000, 0), "{acc}");
    assert_eq!(acc["unpriced"], false, "{acc}");
}

#[test]
fn inv_import_at23_existing_price_for_the_date_is_kept() {
    let mut b = book();
    b.run(
        "security.create",
        json!({"symbol": "VTI", "name": "Total Stock Market ETF", "kind": "etf"}),
    );
    b.run(
        "price.set",
        json!({"security": "VTI", "date": "2026-10-01", "price": "199"}),
    );
    b.run(
        "inv_profile.save",
        profile(
            "json",
            "data.transactions",
            json!({"date": "tradeDate", "action": "activity", "symbol": "symbol",
                   "quantity": "shares", "price": "price", "fees": "fees",
                   "amount": "netAmount", "external_id": "id"}),
        ),
    );
    let args = json!({"name": "a.json", "content_base64": b64(JSON_FILE.as_bytes()),
                      "account": "Assets:Brokerage", "profile": "Test json"});
    let batch = b.run("import.stage", args);
    commit(&mut b, &batch);
    let p = b.run(
        "price.latest",
        json!({"security": "VTI", "as_of": "2026-10-01"}),
    );
    assert_eq!(p["price"], "199.00");
}

#[test]
fn inv_import_at24_cd_without_price_valued_at_face() {
    let mut b = book();
    b.run(
        "security.create",
        json!({"symbol": "CD123", "name": "Example Bank CD", "kind": "cd"}),
    );
    b.run(
        "inv.shares_in",
        json!({"account": "Assets:Brokerage", "security": "CD123", "date": "2026-09-01",
               "quantity": "10000", "lots": [{"acquired": "2026-09-01", "quantity": "10000", "basis": "9950.00"}]}),
    );
    let h = b.run("inv.holdings", json!({"as_of": "2026-10-19"}));
    let cd = holding(&h, "CD123");
    assert_eq!(cd["price"], "100.00");
    assert!(cd["price_date"].is_null());
    assert_eq!(cd["price_note"], "at face value");
    assert_eq!(cd["market_value"], "10000.00");
    assert_eq!(cd["gain"], "50.00");
    assert_eq!(h["accounts"][0]["market_value"], "10000.00");
    assert_eq!(h["accounts"][0]["unpriced"], false);
    // A bond without a price stays "no price".
    b.run(
        "security.create",
        json!({"symbol": "BOND1", "name": "Example Bond", "kind": "bond"}),
    );
    b.run(
        "inv.shares_in",
        json!({"account": "Assets:Brokerage", "security": "BOND1", "date": "2026-09-01",
               "quantity": "5000", "lots": [{"acquired": "2026-09-01", "quantity": "5000", "basis": "5000.00"}]}),
    );
    let h = b.run("inv.holdings", json!({"as_of": "2026-10-19"}));
    assert!(holding(&h, "BOND1")["price"].is_null());
    assert_eq!(h["accounts"][0]["unpriced"], true);
}
