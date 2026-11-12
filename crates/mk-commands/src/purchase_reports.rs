//! Purchase reports and 1099 tracking (business-purchases.md): A/P aging tied to the
//! Accounts Payable balance, vendor balances, unpaid bills, expenses by vendor, open
//! purchase orders, the check register, bills due and the 1099 summary.

use std::collections::{BTreeMap, BTreeSet};

use mk_core::{AccountId, AccountSubtype, AccountType, Date, Decimal, TxnStatus};
use mk_store::TxnFilter;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::module_settings as ms;
use crate::purchases::{
    BILLS, BillDoc, Book, POS, bill_docs, bill_state, links, payable_id, po_received, po_status,
};
use crate::reports::{Csv, Dates};
use crate::sales::{Party, money, parties, round2};
use crate::sales_reports::{BUCKETS, at, bucket, h, list, minus, pct, plus, s, today_or};
use crate::{CommandError, CommandResult, Ctx, Registry};

const TEN99: &str = "1099";

pub(crate) fn register(r: &mut Registry) {
    r.add_in(
        BILLS,
        "report.ap_aging",
        "Accounts payable aging by vendor",
        false,
        ap_aging,
    );
    r.add_in(
        BILLS,
        "report.vendor_balances",
        "What the business owes each vendor",
        false,
        vendor_balances,
    );
    r.add_in(
        BILLS,
        "report.unpaid_bills",
        "Bills with a balance",
        false,
        unpaid_bills,
    );
    r.add_in(
        BILLS,
        "report.expenses_by_vendor",
        "Purchases by vendor",
        false,
        expenses_by_vendor,
    );
    r.add_in(
        POS,
        "report.open_purchase_orders",
        "Purchase orders not fully received",
        false,
        open_pos,
    );
    r.add_in(
        BILLS,
        "report.check_register",
        "Checks written from a bank account",
        false,
        check_register,
    );
    r.add_in(
        BILLS,
        "report.bills_due",
        "Bills due in a window (for payment runs)",
        false,
        bills_due,
    );
    r.add_in(
        TEN99,
        "report.1099",
        "Vendors' 1099 totals for a year",
        false,
        report_1099,
    );
}

fn names(ctx: &Ctx<'_>) -> CommandResult<BTreeMap<String, String>> {
    Ok(parties(ctx)?.into_iter().map(|p| (p.id, p.name)).collect())
}

/// Bills with a non-zero balance as of a date.
fn open_as_of(ctx: &Ctx<'_>, b: &Book, as_of: Date) -> CommandResult<Vec<(BillDoc, Decimal)>> {
    Ok(bill_docs(ctx, "bill")?
        .into_iter()
        .filter(|d| d.status != "void" && d.date <= as_of)
        .map(|d| {
            let (_, bal, _) = bill_state(&d, b, Some(as_of));
            (d, bal)
        })
        .filter(|(_, bal)| !bal.is_zero())
        .collect())
}

/// Prepayments and unapplied credits per vendor as of a date.
fn credits_as_of(b: &Book, as_of: Date) -> BTreeMap<String, Decimal> {
    let mut out: BTreeMap<String, Decimal> = BTreeMap::new();
    for p in &b.payments {
        let u = b.unapplied_payment(p, Some(as_of));
        if !u.is_zero() {
            let e = out.entry(p.vendor.clone()).or_default();
            *e = plus(*e, u);
        }
    }
    for c in &b.credits {
        let u = b.unapplied_credit(c, Some(as_of));
        if !u.is_zero() {
            let e = out.entry(c.vendor.clone()).or_default();
            *e = plus(*e, u);
        }
    }
    out
}

/// The Accounts Payable balance (credit positive) as of a date.
fn ap_ledger(ctx: &Ctx<'_>, as_of: Date) -> CommandResult<Decimal> {
    let Some(id) = payable_id(ctx)? else {
        return Ok(Decimal::ZERO);
    };
    let mut total = Decimal::ZERO;
    for row in ctx.tx.posted_splits(Some(as_of))? {
        if row.account == id {
            total = minus(total, row.value);
        }
    }
    Ok(total)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AgingArgs {
    #[serde(default)]
    as_of: Option<Date>,
    /// `due_date` (default) or `bill_date`.
    #[serde(default)]
    basis: Option<String>,
    #[serde(default)]
    detail: bool,
}

/// Per vendor: bucket balances, credits, detail rows.
type AgingRow = ([Decimal; 5], Decimal, Vec<Value>);

fn ap_aging(ctx: &Ctx<'_>, a: AgingArgs) -> CommandResult<Value> {
    let as_of = today_or(a.as_of)?;
    let basis = a.basis.unwrap_or_else(|| "due_date".into());
    if !matches!(basis.as_str(), "due_date" | "bill_date") {
        return Err(CommandError::InvalidArgs(
            "basis is due_date or bill_date".into(),
        ));
    }
    let b = Book::load(ctx)?;
    let names = names(ctx)?;
    let mut rows: BTreeMap<String, AgingRow> = BTreeMap::new();
    for (d, bal) in open_as_of(ctx, &b, as_of)? {
        let e = rows.entry(d.vendor.clone()).or_default();
        let from = if basis == "due_date" {
            d.due_date.unwrap_or(d.date)
        } else {
            d.date
        };
        let days = from.days_until(as_of);
        let k = bucket(days);
        if let Some(slot) = e.0.get_mut(k) {
            *slot = plus(*slot, bal);
        }
        e.2.push(json!({
            "bill": d.id, "number": d.number, "date": d.date, "due_date": d.due_date,
            "days_past_due": days.max(0), "bucket": BUCKETS.get(k), "balance": money(bal),
        }));
    }
    for (v, amt) in credits_as_of(&b, as_of) {
        let e = rows.entry(v).or_default();
        e.1 = plus(e.1, amt);
    }
    let mut ordered: Vec<(String, AgingRow)> = rows.into_iter().collect();
    ordered.sort_by_key(|(v, _)| names.get(v).map(|n| n.to_lowercase()).unwrap_or_default());
    let mut totals = [Decimal::ZERO; 5];
    let mut credit_total = Decimal::ZERO;
    let mut out = Vec::new();
    for (v, (bk, credit, detail)) in ordered {
        let total = minus(bk.iter().fold(Decimal::ZERO, |s, x| plus(s, *x)), credit);
        for (t, x) in totals.iter_mut().zip(bk.iter()) {
            *t = plus(*t, *x);
        }
        credit_total = plus(credit_total, credit);
        let mut row = json!({
            "vendor": v, "vendor_name": names.get(&v),
            "buckets": bk.iter().map(|x| money(*x)).collect::<Vec<_>>(),
            "credits": money(-credit), "total": money(total),
        });
        if a.detail
            && let Some(o) = row.as_object_mut()
        {
            o.insert("bills".into(), Value::Array(detail));
        }
        out.push(row);
    }
    let grand = minus(
        totals.iter().fold(Decimal::ZERO, |s, x| plus(s, *x)),
        credit_total,
    );
    let ledger = ap_ledger(ctx, as_of)?;
    let difference = minus(ledger, grand);
    Ok(json!({
        "as_of": as_of, "basis": basis, "columns": BUCKETS, "vendors": out,
        "totals": {"buckets": totals.iter().map(|x| money(*x)).collect::<Vec<_>>(), "credits": money(-credit_total), "total": money(grand)},
        "ap_balance": money(ledger), "difference": money(difference), "balanced": difference.is_zero(),
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AsOfArgs {
    #[serde(default)]
    as_of: Option<Date>,
}

fn vendor_balances(ctx: &Ctx<'_>, a: AsOfArgs) -> CommandResult<Value> {
    let as_of = today_or(a.as_of)?;
    let b = Book::load(ctx)?;
    // vendor → (open, credit, bills, overdue)
    let mut by: BTreeMap<String, (Decimal, Decimal, usize, Decimal)> = BTreeMap::new();
    for (d, bal) in open_as_of(ctx, &b, as_of)? {
        let e = by.entry(d.vendor.clone()).or_default();
        e.0 = plus(e.0, bal);
        e.2 += 1;
        if d.due_date.is_some_and(|x| as_of > x) {
            e.3 = plus(e.3, bal);
        }
    }
    for (v, amt) in credits_as_of(&b, as_of) {
        let e = by.entry(v).or_default();
        e.1 = plus(e.1, amt);
    }
    let mut out = Vec::new();
    let mut total = Decimal::ZERO;
    for p in parties(ctx)? {
        let Some((open, credit, n, overdue)) = by.get(&p.id) else {
            continue;
        };
        let bal = minus(*open, *credit);
        total = plus(total, bal);
        out.push(
            json!({"vendor": p.id, "vendor_name": p.name, "open_bills": n,
            "open": money(*open), "overdue": money(*overdue), "credits": money(-*credit),
            "balance": money(bal)}),
        );
    }
    Ok(json!({"as_of": as_of, "vendors": out, "total": money(total)}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UnpaidArgs {
    #[serde(default)]
    as_of: Option<Date>,
    #[serde(default)]
    due_before: Option<Date>,
}

fn unpaid_bills(ctx: &Ctx<'_>, a: UnpaidArgs) -> CommandResult<Value> {
    let as_of = today_or(a.as_of)?;
    let b = Book::load(ctx)?;
    let names = names(ctx)?;
    let mut rows = Vec::new();
    let mut total = Decimal::ZERO;
    let mut open = open_as_of(ctx, &b, as_of)?;
    open.sort_by_key(|(d, _)| d.due_date.unwrap_or(d.date));
    for (d, bal) in open {
        if a.due_before
            .is_some_and(|x| d.due_date.is_none_or(|due| due > x))
        {
            continue;
        }
        total = plus(total, bal);
        rows.push(json!({"bill": d.id, "number": d.number, "vendor": d.vendor,
            "vendor_name": names.get(&d.vendor), "date": d.date, "due_date": d.due_date,
            "days_overdue": d.due_date.map_or(0, |x| x.days_until(as_of).max(0)),
            "total": money(d.total), "balance": money(bal)}));
    }
    Ok(json!({"as_of": as_of, "bills": rows, "total": money(total)}))
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DatesArgs {
    #[serde(flatten)]
    dates: Dates,
}

fn expenses_by_vendor(ctx: &Ctx<'_>, a: DatesArgs) -> CommandResult<Value> {
    let (from, to) = crate::reports::resolve(ctx, &a.dates)?;
    let mut by: BTreeMap<String, Decimal> = BTreeMap::new();
    for (kind, sign) in [
        ("bill", Decimal::ONE),
        ("vendor_credit", Decimal::NEGATIVE_ONE),
    ] {
        for d in bill_docs(ctx, kind)? {
            if d.status == "void" || d.date < from || d.date > to {
                continue;
            }
            let e = by.entry(d.vendor.clone()).or_default();
            *e = plus(*e, d.total.checked_mul(sign).unwrap_or_default());
        }
    }
    let chart = Chart::load(ctx)?;
    let link = links(ctx)?;
    for t in ctx.tx.transactions(&TxnFilter {
        accounts: None,
        from: Some(from),
        to: Some(to),
        include_voided: false,
        limit: None,
        offset: 0,
    })? {
        let Some(v) = link.get(&t.id.to_string()) else {
            continue;
        };
        let spent = t
            .splits
            .iter()
            .filter(|sp| {
                chart
                    .get(sp.account)
                    .is_some_and(|x| x.account_type == AccountType::Expense)
            })
            .fold(Decimal::ZERO, |s, sp| plus(s, sp.value));
        let e = by.entry(v.clone()).or_default();
        *e = plus(*e, spent);
    }
    let total = by.values().fold(Decimal::ZERO, |s, x| plus(s, *x));
    let mut rows = Vec::new();
    for p in parties(ctx)? {
        if let Some(v) = by.get(&p.id) {
            rows.push(json!({"vendor": p.id, "vendor_name": p.name, "amount": money(*v), "percent": pct(*v, total)}));
        }
    }
    Ok(json!({"from": from, "to": to, "vendors": rows, "total": money(total)}))
}

fn open_pos(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let names = names(ctx)?;
    let mut rows = Vec::new();
    let mut total = Decimal::ZERO;
    for d in bill_docs(ctx, "po")? {
        let received = po_received(ctx, &d)?;
        let status = po_status(&d, &received);
        if !matches!(status.as_str(), "open" | "partly_received") {
            continue;
        }
        let mut left = Decimal::ZERO;
        for (l, r) in d.computed.iter().zip(&received) {
            let open = if l.item.is_some() {
                let q = minus(l.quantity.unwrap_or_default(), *r).max(Decimal::ZERO);
                round2(
                    q.checked_mul(l.cost.unwrap_or_default())
                        .unwrap_or_default(),
                )
            } else {
                minus(l.amount, *r).max(Decimal::ZERO)
            };
            left = plus(left, open);
        }
        total = plus(total, left);
        rows.push(json!({"po": d.id, "number": d.number, "vendor": d.vendor,
            "vendor_name": names.get(&d.vendor), "date": d.date, "expected_date": d.expected_date,
            "total": money(d.total), "open": money(left), "status": status}));
    }
    Ok(json!({"purchase_orders": rows, "total": money(total)}))
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CheckArgs {
    /// The bank account.
    account: String,
    #[serde(flatten)]
    dates: Dates,
}

fn check_register(ctx: &Ctx<'_>, a: CheckArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let acc = chart.resolve(&a.account)?.id;
    let (from, to) = crate::reports::resolve(ctx, &a.dates)?;
    let mut rows = Vec::new();
    let mut total = Decimal::ZERO;
    let mut txns = ctx.tx.transactions(&TxnFilter {
        accounts: Some(vec![acc]),
        from: Some(from),
        to: Some(to),
        include_voided: true,
        limit: None,
        offset: 0,
    })?;
    txns.sort_by_key(|t| {
        (
            t.number
                .as_deref()
                .and_then(|n| n.trim().parse::<i64>().ok()),
            t.number.clone(),
            t.date,
        )
    });
    for t in txns {
        let Some(number) = t.number.clone().filter(|n| !n.trim().is_empty()) else {
            continue;
        };
        let amount = t
            .splits
            .iter()
            .filter(|sp| sp.account == acc)
            .fold(Decimal::ZERO, |s, sp| minus(s, sp.value));
        let voided = t.status == TxnStatus::Voided;
        if !voided {
            total = plus(total, amount);
        }
        rows.push(
            json!({"txn": t.id, "date": t.date, "number": number, "payee": t.payee,
            "amount": money(amount), "voided": voided}),
        );
    }
    Ok(
        json!({"account": acc, "account_name": chart.full_name(acc), "from": from, "to": to,
        "checks": rows, "total": money(total)}),
    )
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DueArgs {
    from: Date,
    to: Date,
}

fn bills_due(ctx: &Ctx<'_>, a: DueArgs) -> CommandResult<Value> {
    let b = Book::load(ctx)?;
    let names = names(ctx)?;
    let mut rows = Vec::new();
    let mut total = Decimal::ZERO;
    let mut all = bill_docs(ctx, "bill")?;
    all.sort_by_key(|d| d.due_date.unwrap_or(d.date));
    for d in all {
        let (_, bal, status) = bill_state(&d, &b, None);
        if !matches!(status.as_str(), "open" | "partial") {
            continue;
        }
        let due = d.due_date.unwrap_or(d.date);
        if due < a.from || due > a.to {
            continue;
        }
        total = plus(total, bal);
        rows.push(json!({"bill": d.id, "number": d.number, "vendor": d.vendor,
            "vendor_name": names.get(&d.vendor), "due_date": due, "balance": money(bal), "status": status}));
    }
    Ok(json!({"from": a.from, "to": a.to, "bills": rows, "total": money(total)}))
}

// ----- 1099 ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Ten99Args {
    year: i32,
    #[serde(default)]
    include_below_threshold: bool,
}

/// The accounts whose payments count (setting `1099.accounts` with sub-accounts, else
/// every expense account).
fn mapped(ctx: &Ctx<'_>, chart: &Chart) -> CommandResult<BTreeSet<AccountId>> {
    let v = ms::get(ctx, "1099.accounts")?;
    Ok(match v.as_array() {
        Some(list) => list
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|r| chart.resolve(r).ok())
            .flat_map(|a| chart.subtree(a.id))
            .collect(),
        None => chart
            .accounts
            .iter()
            .filter(|a| a.account_type == AccountType::Expense)
            .map(|a| a.id)
            .collect(),
    })
}

/// (form label, box number, threshold key) of a vendor.
fn form_of(p: &Party) -> (&'static str, &'static str, String) {
    if p.form_1099 == "misc" {
        let b = p.misc_box.clone().unwrap_or_else(|| "other_income".into());
        let n = match b.as_str() {
            "rents" => "1",
            "royalties" => "2",
            "medical" => "6",
            "attorney_gross_proceeds" => "10",
            _ => "3",
        };
        ("1099-MISC", n, b)
    } else {
        ("1099-NEC", "1", "nec".into())
    }
}

fn report_1099(ctx: &Ctx<'_>, a: Ten99Args) -> CommandResult<Value> {
    let (start, end) = (
        Date::from_ymd(a.year, 1, 1)?,
        Date::from_ymd(a.year, 12, 31)?,
    );
    let inside = |d: Date| d >= start && d <= end;
    let chart = Chart::load(ctx)?;
    let accounts = mapped(ctx, &chart)?;
    let is_card = |id: AccountId| {
        chart
            .get(id)
            .is_some_and(|x| x.subtype == AccountSubtype::CreditCard)
    };
    let b = Book::load(ctx)?;
    let bills: BTreeMap<String, BillDoc> = bill_docs(ctx, "bill")?
        .into_iter()
        .map(|d| (d.id.clone(), d))
        .collect();
    // vendor → (qualifying, excluded card)
    let mut by: BTreeMap<String, (Decimal, Decimal)> = BTreeMap::new();
    for p in b.payments.iter().filter(|p| !p.void) {
        let card = p.method == "card" || is_card(p.from_account);
        for app in p.applied.iter().filter(|x| inside(x.date)) {
            let Some(bill) = bills.get(&app.bill) else {
                continue;
            };
            let counted = bill
                .computed
                .iter()
                .filter(|l| accounts.contains(&l.account))
                .fold(Decimal::ZERO, |s, l| plus(s, l.amount));
            if bill.total.is_zero() || counted.is_zero() {
                continue;
            }
            let share = round2(
                app.amount
                    .checked_mul(counted)
                    .and_then(|x| x.checked_div(bill.total))
                    .unwrap_or_default(),
            );
            let e = by.entry(p.vendor.clone()).or_default();
            if card {
                e.1 = plus(e.1, share);
            } else {
                e.0 = plus(e.0, share);
            }
        }
    }
    let link = links(ctx)?;
    for t in ctx.tx.transactions(&TxnFilter {
        accounts: None,
        from: Some(start),
        to: Some(end),
        include_voided: false,
        limit: None,
        offset: 0,
    })? {
        let Some(v) = link.get(&t.id.to_string()) else {
            continue;
        };
        let counted = t
            .splits
            .iter()
            .filter(|sp| accounts.contains(&sp.account))
            .fold(Decimal::ZERO, |s, sp| plus(s, sp.value));
        if counted <= Decimal::ZERO {
            continue;
        }
        let card = t.splits.iter().any(|sp| is_card(sp.account));
        let e = by.entry(v.clone()).or_default();
        if card {
            e.1 = plus(e.1, counted);
        } else {
            e.0 = plus(e.0, counted);
        }
    }
    let mut rows = Vec::new();
    for p in parties(ctx)?.iter().filter(|p| p.tracks_1099) {
        let (total, card) = by.get(&p.id).copied().unwrap_or_default();
        let (form, box_no, key) = form_of(p);
        let threshold = ms::threshold(ctx, a.year, &key)?;
        let file = total >= threshold && total > Decimal::ZERO;
        if !file && !a.include_below_threshold {
            continue;
        }
        let address_complete = p.billing_address.len() >= 2;
        let mut flags = Vec::new();
        if file {
            flags.push("file".to_owned());
            if !p.tin_on_file {
                flags.push("missing TIN on file".to_owned());
            }
            if !address_complete {
                flags.push("missing address".to_owned());
            }
        }
        rows.push(json!({
            "vendor": p.id, "vendor_name": p.name, "form": form, "box": box_no, "box_key": key,
            "total": money(total), "excluded_card": money(card), "threshold": money(threshold),
            "file": file, "tin_on_file": p.tin_on_file, "address_complete": address_complete,
            "flags": flags,
        }));
    }
    let thresholds: serde_json::Map<String, Value> = ms::THRESHOLD_KEYS
        .iter()
        .map(|k| {
            Ok((
                (*k).to_owned(),
                json!(money(ms::threshold(ctx, a.year, k)?)),
            ))
        })
        .collect::<CommandResult<_>>()?;
    Ok(json!({
        "year": a.year, "vendors": rows, "thresholds": thresholds,
        "note": "MoneyKeep keeps records only; it does not file 1099 forms. Check the thresholds each year: from 2027 they are indexed for inflation.",
    }))
}

// ----- CSV ----------------------------------------------------------------------------

/// CSV for the reports in this module: `Some((data, csv))`, or `None` for others.
pub(crate) fn export(
    ctx: &Ctx<'_>,
    name: &str,
    args: Value,
) -> CommandResult<Option<(Value, String)>> {
    let module = match name {
        "ap_aging" | "vendor_balances" | "unpaid_bills" | "expenses_by_vendor"
        | "check_register" | "bills_due" => BILLS,
        "open_purchase_orders" => POS,
        "1099" => TEN99,
        _ => return Ok(None),
    };
    if !crate::sales::module_on(ctx, module)? {
        return Err(CommandError::ModuleDisabled {
            command: format!("report.{name}"),
            module: module.into(),
        });
    }
    Ok(Some(match name {
        "ap_aging" => {
            let d = ap_aging(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&[
                "Vendor", "Current", "1-30", "31-60", "61-90", "Over 90", "Credits", "Total",
            ]));
            for r in list(at(&d, "vendors")) {
                let mut row = vec![s(at(&r, "vendor_name"))];
                row.extend(list(at(&r, "buckets")).iter().map(s));
                row.push(s(at(&r, "credits")));
                row.push(s(at(&r, "total")));
                c.row(&row);
            }
            let t = at(&d, "totals");
            let mut row = vec!["Total".to_owned()];
            row.extend(list(at(t, "buckets")).iter().map(s));
            row.push(s(at(t, "credits")));
            row.push(s(at(t, "total")));
            c.row(&row);
            (d, c.out)
        }
        "vendor_balances" => {
            let d = vendor_balances(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&[
                "Vendor",
                "Open bills",
                "Open",
                "Overdue",
                "Credits",
                "Balance",
            ]));
            for r in list(at(&d, "vendors")) {
                c.row(&[
                    s(at(&r, "vendor_name")),
                    s(at(&r, "open_bills")),
                    s(at(&r, "open")),
                    s(at(&r, "overdue")),
                    s(at(&r, "credits")),
                    s(at(&r, "balance")),
                ]);
            }
            (d, c.out)
        }
        "unpaid_bills" => {
            let d = unpaid_bills(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&[
                "Vendor",
                "Number",
                "Date",
                "Due date",
                "Days overdue",
                "Total",
                "Balance",
            ]));
            for r in list(at(&d, "bills")) {
                c.row(&[
                    s(at(&r, "vendor_name")),
                    s(at(&r, "number")),
                    s(at(&r, "date")),
                    s(at(&r, "due_date")),
                    s(at(&r, "days_overdue")),
                    s(at(&r, "total")),
                    s(at(&r, "balance")),
                ]);
            }
            (d, c.out)
        }
        "expenses_by_vendor" => {
            let d = expenses_by_vendor(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&["Vendor", "Amount", "Percent"]));
            for r in list(at(&d, "vendors")) {
                c.row(&[
                    s(at(&r, "vendor_name")),
                    s(at(&r, "amount")),
                    s(at(&r, "percent")),
                ]);
            }
            (d, c.out)
        }
        "open_purchase_orders" => {
            let d = open_pos(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&[
                "Number", "Vendor", "Date", "Expected", "Total", "Open", "Status",
            ]));
            for r in list(at(&d, "purchase_orders")) {
                c.row(&[
                    s(at(&r, "number")),
                    s(at(&r, "vendor_name")),
                    s(at(&r, "date")),
                    s(at(&r, "expected_date")),
                    s(at(&r, "total")),
                    s(at(&r, "open")),
                    s(at(&r, "status")),
                ]);
            }
            (d, c.out)
        }
        "check_register" => {
            let d = check_register(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&["Number", "Date", "Payee", "Amount", "Void"]));
            for r in list(at(&d, "checks")) {
                let void = if at(&r, "voided").as_bool() == Some(true) {
                    "void"
                } else {
                    ""
                };
                c.row(&[
                    s(at(&r, "number")),
                    s(at(&r, "date")),
                    s(at(&r, "payee")),
                    s(at(&r, "amount")),
                    void.into(),
                ]);
            }
            (d, c.out)
        }
        "bills_due" => {
            let d = bills_due(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&["Vendor", "Number", "Due date", "Balance"]));
            for r in list(at(&d, "bills")) {
                c.row(&[
                    s(at(&r, "vendor_name")),
                    s(at(&r, "number")),
                    s(at(&r, "due_date")),
                    s(at(&r, "balance")),
                ]);
            }
            (d, c.out)
        }
        _ => {
            let d = report_1099(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&[
                "Vendor",
                "Form",
                "Box",
                "Total",
                "Excluded card payments",
                "Threshold",
                "File",
                "TIN on file",
                "Address complete",
                "Flags",
            ]));
            for r in list(at(&d, "vendors")) {
                let yes = |k: &str| {
                    if at(&r, k).as_bool() == Some(true) {
                        "yes"
                    } else {
                        "no"
                    }
                    .to_owned()
                };
                let flags: Vec<String> = list(at(&r, "flags")).iter().map(s).collect();
                c.row(&[
                    s(at(&r, "vendor_name")),
                    s(at(&r, "form")),
                    s(at(&r, "box")),
                    s(at(&r, "total")),
                    s(at(&r, "excluded_card")),
                    s(at(&r, "threshold")),
                    yes("file"),
                    yes("tin_on_file"),
                    yes("address_complete"),
                    flags.join("; "),
                ]);
            }
            (d, c.out)
        }
    }))
}
