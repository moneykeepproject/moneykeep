//! OFX investment statements (`INVSTMTRS`, spec: import-investments.md, OFX 2.2
//! chapter 13). Sections come out with kind `investment`: securities from `SECLIST`,
//! investment rows per the mapping table, cash rows from `INVBANKTRAN`, positions
//! from `INVPOSLIST` and cash from `INVBAL`. Quantities and amounts are absolute
//! values; the action carries the direction (§13.9.1 sign rules).

use mk_core::{CommodityCode, Date, Decimal};

use crate::ofx::{Node, last_four, parse_ofx_amount, parse_ofx_date, status_error, transaction};
use crate::statement::{
    ClosingBalance, InvAction, InvRow, Position, Section, SectionKind, SourceSecurity, Statement,
};
use crate::{ImportError, ImportResult};

/// The source security key of a `SECID`.
fn sec_key(secid: Option<&Node>) -> Option<String> {
    let s = secid?;
    let id = s.text_of("UNIQUEID")?;
    let ty = s
        .text_of("UNIQUEIDTYPE")
        .map_or_else(|| "ID".to_owned(), |t| t.to_ascii_uppercase());
    Some(format!("{ty}:{id}"))
}

fn securities(ofx: &Node) -> Vec<SourceSecurity> {
    let mut out = Vec::new();
    for list in ofx
        .all("SECLISTMSGSRSV1")
        .flat_map(|m| m.all("SECLIST").collect::<Vec<_>>())
    {
        for (agg, kind) in [
            ("STOCKINFO", "stock"),
            ("MFINFO", "mutual_fund"),
            ("DEBTINFO", "bond"),
            ("OTHERINFO", "other"),
            ("OPTINFO", "option"),
        ] {
            for info in list.all(agg) {
                let Some(si) = info.child("SECINFO") else {
                    continue;
                };
                let Some(key) = sec_key(si.child("SECID")) else {
                    continue;
                };
                let secid = si.child("SECID");
                let cusip = secid
                    .filter(|s| {
                        s.text_of("UNIQUEIDTYPE")
                            .is_some_and(|t| t.eq_ignore_ascii_case("CUSIP"))
                    })
                    .and_then(|s| s.text_of("UNIQUEID"));
                out.push(SourceSecurity {
                    key,
                    cusip,
                    ticker: si.text_of("TICKER").map(|t| t.to_ascii_uppercase()),
                    name: si.text_of("SECNAME"),
                    kind: Some(kind.to_owned()),
                    price: si.text_of("UNITPRICE").and_then(|p| parse_ofx_amount(&p)),
                    price_date: si
                        .text_of("DTASOF")
                        .and_then(|d| parse_ofx_date(&d))
                        .filter(|d| d.year() > 1990),
                    figi: None,
                });
            }
        }
    }
    out
}

fn amount(n: &Node, tag: &str) -> Option<Decimal> {
    n.text_of(tag).and_then(|t| parse_ofx_amount(&t))
}

fn sum(n: &Node, tags: &[&str]) -> Decimal {
    tags.iter()
        .filter_map(|t| amount(n, t))
        .fold(Decimal::ZERO, |a, v| a.checked_add(v.abs()).unwrap_or(a))
}

fn kind_of(
    income_type: Option<&str>,
    tax_exempt: bool,
    div_kind: &str,
) -> (InvAction, String, bool) {
    if tax_exempt {
        return (InvAction::Dividend, "tax_exempt".into(), false);
    }
    match income_type.unwrap_or("DIV") {
        "CGLONG" => (InvAction::Dividend, "capital_gain_long".into(), false),
        "CGSHORT" => (InvAction::Dividend, "capital_gain_short".into(), false),
        "INTEREST" => (InvAction::Interest, String::new(), false),
        "MISC" => (InvAction::Dividend, "ordinary".into(), true),
        _ => (InvAction::Dividend, div_kind.to_owned(), false),
    }
}

/// One investment transaction aggregate → a row (or `None` for an unusable one,
/// with a warning).
fn inv_row(
    agg: &Node,
    curdef: Option<&str>,
    div_kind: &str,
    warnings: &mut Vec<String>,
) -> Option<InvRow> {
    let name = agg.name.as_str();
    // INVTRAN sits in the aggregate, or in its INVBUY / INVSELL.
    let inner = agg.child("INVBUY").or_else(|| agg.child("INVSELL"));
    let body = inner.unwrap_or(agg);
    let invtran = body.child("INVTRAN");
    let at = format!("{name} (line {})", agg.line);
    let Some(t) = invtran else {
        warnings.push(format!("{at} skipped: no INVTRAN"));
        return None;
    };
    let Some(date) = t.text_of("DTTRADE").and_then(|d| parse_ofx_date(&d)) else {
        warnings.push(format!("{at} skipped: missing or invalid DTTRADE"));
        return None;
    };
    let security = sec_key(body.child("SECID"));
    let units = amount(body, "UNITS").map(|u| u.abs());
    let price = amount(body, "UNITPRICE").map(|p| p.abs());
    let total = amount(body, "TOTAL");
    let mut row = InvRow {
        cleaned: Vec::new(),
        external_id: t.text_of("FITID"),
        date,
        action: InvAction::Unsupported,
        security,
        quantity: units,
        price,
        commission: Decimal::ZERO,
        amount: total.map(|v| v.abs()),
        kind: None,
        ratio: None,
        basis: None,
        acquired: None,
        expense_account: None,
        memo: t.text_of("MEMO").unwrap_or_default(),
        reversal_of: t.text_of("REVERSALFITID"),
        unsupported: None,
        needs_review: false,
        notes: Vec::new(),
        line: agg.line,
        raw: format!(
            "{name} {} {} {}",
            t.text_of("FITID").unwrap_or_default(),
            t.text_of("DTTRADE").unwrap_or_default(),
            total.map(|v| v.to_string()).unwrap_or_default()
        ),
    };
    let unsupported = |row: &mut InvRow, why: &str| {
        row.action = InvAction::Unsupported;
        row.unsupported = Some(why.to_owned());
    };
    let costs = ["COMMISSION", "FEES", "TAXES", "LOAD"];
    match name {
        "BUYSTOCK" | "BUYMF" | "BUYOTHER" | "BUYDEBT" => {
            row.action = InvAction::Buy;
            row.commission = sum(body, &costs);
            if name == "BUYDEBT" && amount(agg, "ACCRDINT").is_some() {
                row.notes
                    .push("accrued interest (ACCRDINT) is not included".into());
            }
            if agg
                .text_of("BUYTYPE")
                .is_some_and(|b| b.eq_ignore_ascii_case("BUYTOCOVER"))
            {
                unsupported(&mut row, "buys to cover short sales are not supported");
            }
        }
        "SELLSTOCK" | "SELLMF" | "SELLOTHER" | "SELLDEBT" => {
            row.action = InvAction::Sell;
            row.commission = sum(
                body,
                &[
                    "COMMISSION",
                    "FEES",
                    "TAXES",
                    "LOAD",
                    "WITHHOLDING",
                    "PENALTY",
                ],
            );
            if agg
                .text_of("SELLTYPE")
                .is_some_and(|b| b.eq_ignore_ascii_case("SELLSHORT"))
            {
                unsupported(&mut row, "short sales are not supported");
            }
        }
        "INCOME" => {
            let tax_exempt = agg.text_of("TAXEXEMPT").is_some_and(|v| v == "Y");
            let it = agg.text_of("INCOMETYPE").map(|t| t.to_ascii_uppercase());
            let (action, kind, review) = kind_of(it.as_deref(), tax_exempt, div_kind);
            row.action = action;
            row.kind = (!kind.is_empty()).then_some(kind);
            row.quantity = None;
            row.price = None;
            if review {
                row.needs_review = true;
                row.notes
                    .push("income type MISC: check that this is a dividend".into());
            }
            if total.is_some_and(|v| v.is_sign_negative()) {
                row.needs_review = true;
                row.notes
                    .push("a negative income total is an adjustment".into());
            }
        }
        "REINVEST" => {
            let tax_exempt = agg.text_of("TAXEXEMPT").is_some_and(|v| v == "Y");
            let it = agg.text_of("INCOMETYPE").map(|t| t.to_ascii_uppercase());
            let (_, kind, review) = kind_of(it.as_deref(), tax_exempt, div_kind);
            row.action = InvAction::Reinvest;
            row.kind = Some(if kind.is_empty() {
                "ordinary".into()
            } else {
                kind
            });
            if review {
                row.needs_review = true;
                row.notes
                    .push("income type MISC: check that this is a dividend".into());
            }
            if !sum(agg, &costs).is_zero() {
                row.needs_review = true;
                row.notes
                    .push("the reinvestment carries a commission or fee".into());
            }
        }
        "INVEXPENSE" => {
            row.action = InvAction::Fee;
            row.quantity = None;
        }
        "MARGININTEREST" => {
            row.action = InvAction::Fee;
            row.security = None;
            row.quantity = None;
            row.expense_account = Some("Margin Interest".into());
        }
        "RETOFCAP" => {
            row.action = InvAction::ReturnOfCapital;
            row.quantity = None;
        }
        "SPLIT" => {
            row.action = InvAction::Split;
            row.quantity = None;
            let num = amount(agg, "NUMERATOR").zip(amount(agg, "DENOMINATOR"));
            let units = amount(agg, "NEWUNITS").zip(amount(agg, "OLDUNITS"));
            row.ratio = num.or(units).map(|(a, b)| (a.abs(), b.abs()));
            if row.ratio.is_none() {
                unsupported(&mut row, "the split has no ratio");
            }
            if amount(agg, "FRACCASH").is_some_and(|c| c > Decimal::ZERO) {
                row.notes
                    .push("cash in lieu of fractional shares needs a manual sale entry".into());
            }
        }
        "TRANSFER" => {
            let action = agg.text_of("TFERACTION").map(|a| a.to_ascii_uppercase());
            if agg
                .text_of("POSTYPE")
                .is_some_and(|p| p.eq_ignore_ascii_case("SHORT"))
            {
                unsupported(&mut row, "short positions are not supported");
            } else if action.as_deref() == Some("OUT") {
                row.action = InvAction::SharesOut;
            } else {
                row.action = InvAction::SharesIn;
                row.basis = amount(agg, "AVGCOSTBASIS").map(|b| b.abs());
                row.acquired = agg
                    .text_of("DTPURCHASE")
                    .and_then(|d| parse_ofx_date(&d))
                    .or(Some(date));
                if row.basis.is_none() {
                    row.needs_review = true;
                    row.notes
                        .push("the transfer has no cost basis: enter it".into());
                }
            }
        }
        "JRNLFUND" | "JRNLSEC" => {
            unsupported(&mut row, "journals between sub-accounts are not supported");
        }
        "CLOSUREOPT" | "BUYOPT" | "SELLOPT" => {
            unsupported(&mut row, "options are not supported");
        }
        other => {
            unsupported(&mut row, &format!("{other} is not supported"));
        }
    }
    let foreign = ["CURRENCY", "ORIGCURRENCY"].iter().find_map(|a| {
        let sym = body
            .child(a)
            .or_else(|| agg.child(a))?
            .text_of("CURSYM")?
            .to_ascii_uppercase();
        (Some(sym.as_str()) != curdef).then_some(sym)
    });
    if let Some(c) = foreign {
        unsupported(&mut row, &format!("foreign-currency transaction ({c})"));
    }
    Some(row)
}

const INV_AGGREGATES: &[&str] = &[
    "BUYDEBT",
    "BUYMF",
    "BUYOPT",
    "BUYOTHER",
    "BUYSTOCK",
    "CLOSUREOPT",
    "INCOME",
    "INVEXPENSE",
    "JRNLFUND",
    "JRNLSEC",
    "MARGININTEREST",
    "REINVEST",
    "RETOFCAP",
    "SELLDEBT",
    "SELLMF",
    "SELLOPT",
    "SELLOTHER",
    "SELLSTOCK",
    "SPLIT",
    "TRANSFER",
];

/// Investment sections of an OFX document, appended to the statement.
pub(crate) fn extract_investments(
    ofx: &Node,
    org: &str,
    st: &mut Statement,
    posted_dates: bool,
) -> ImportResult<()> {
    let secs = securities(ofx);
    let div_kind = "ordinary";
    let mut noted_401k = false;
    let trnrs: Vec<&Node> = ofx
        .all("INVSTMTMSGSRSV1")
        .flat_map(|m| m.all("INVSTMTTRNRS").collect::<Vec<_>>())
        .collect();
    for (n, t) in trnrs.into_iter().enumerate() {
        if let Some(err) = status_error(t) {
            st.warnings
                .push(format!("investment statement {}: {err}", n + 1));
            continue;
        }
        let Some(rs) = t.child("INVSTMTRS") else {
            continue;
        };
        let from = rs.child("INVACCTFROM");
        let broker = from.and_then(|f| f.text_of("BROKERID")).unwrap_or_default();
        let acctid = from.and_then(|f| f.text_of("ACCTID")).unwrap_or_default();
        let key = crate::ofx::inv_key(org, &broker, &acctid);
        let hint = last_four(&acctid);
        let curdef = rs.text_of("CURDEF").map(|c| c.to_ascii_uppercase());
        let currency = curdef
            .as_deref()
            .map(CommodityCode::new)
            .transpose()
            .map_err(|e| ImportError::Invalid(e.to_string()))?;
        let as_of = rs.text_of("DTASOF").and_then(|d| parse_ofx_date(&d));
        let mut sec = Section {
            account_key: Some(key),
            account_name: Some(
                format!(
                    "{} investment{}",
                    if org.is_empty() { "OFX" } else { org },
                    if hint.is_empty() {
                        String::new()
                    } else {
                        format!(" ending {hint}")
                    }
                )
                .trim()
                .to_owned(),
            ),
            account_hint: (!hint.is_empty()).then_some(hint),
            currency,
            kind: SectionKind::Investment,
            securities: secs.clone(),
            as_of,
            ..Section::default()
        };
        if let Some(list) = rs.child("INVTRANLIST") {
            sec.period = list
                .text_of("DTSTART")
                .and_then(|d| parse_ofx_date(&d))
                .zip(list.text_of("DTEND").and_then(|d| parse_ofx_date(&d)));
            for (i, child) in list.children.iter().enumerate() {
                if child.name == "INVBANKTRAN" {
                    if let Some(stmt) = child.child("STMTTRN") {
                        let at = (n + 1, i + 1);
                        if let Some(row) =
                            transaction(stmt, curdef.as_deref(), &mut st.warnings, at, posted_dates)
                        {
                            sec.rows.push(row);
                        }
                    }
                } else if INV_AGGREGATES.contains(&child.name.as_str())
                    && let Some(row) = inv_row(child, curdef.as_deref(), div_kind, &mut st.warnings)
                {
                    sec.inv_rows.push(row);
                }
            }
        }
        if let Some(list) = rs.child("INVPOSLIST") {
            for p in &list.children {
                if p.name == "POSOPT" {
                    continue;
                }
                let Some(pos) = p.child("INVPOS") else {
                    continue;
                };
                let (Some(key), Some(q)) = (sec_key(pos.child("SECID")), amount(pos, "UNITS"))
                else {
                    continue;
                };
                let date = pos
                    .text_of("DTPRICEASOF")
                    .and_then(|d| parse_ofx_date(&d))
                    .filter(|d: &Date| d.year() > 1990);
                sec.positions.push(Position {
                    security: key,
                    quantity: q,
                    price: date.and(amount(pos, "UNITPRICE")),
                    market_value: amount(pos, "MKTVAL"),
                    as_of: date,
                });
            }
        }
        if let Some(bal) = rs.child("INVBAL")
            && let Some(cash) = amount(bal, "AVAILCASH")
        {
            sec.cash_balance = Some(ClosingBalance {
                date: as_of.or(sec.period.map(|p| p.1)).unwrap_or(
                    Date::from_ymd(1970, 1, 1).map_err(|e| ImportError::Invalid(e.to_string()))?,
                ),
                amount: cash,
            });
        }
        if !noted_401k && (rs.child("INV401K").is_some() || rs.child("INV401KBAL").is_some()) {
            noted_401k = true;
            st.warnings
                .push("401(k) sub-account and loan details are ignored".into());
        }
        st.sections.push(sec);
    }
    Ok(())
}
