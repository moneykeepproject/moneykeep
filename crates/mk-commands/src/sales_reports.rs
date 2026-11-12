//! Sales reports and the balance sheet (business-sales.md Part I).

use std::collections::{BTreeMap, HashMap};

use mk_core::{AccountId, AccountType, Date, Decimal};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::reports::{Csv, Dates};
use crate::sales::{
    self, INVOICING, Ledgerish, SALES_TAX, docs, invoice_state, module_on, money, parties,
};
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    r.add_in(
        INVOICING,
        "report.ar_aging",
        "Accounts receivable aging by customer",
        false,
        ar_aging,
    );
    r.add_in(
        INVOICING,
        "report.customer_balances",
        "What each customer owes",
        false,
        customer_balances,
    );
    r.add_in(
        INVOICING,
        "report.open_invoices",
        "Invoices with a balance due",
        false,
        open_invoices,
    );
    r.add_in(
        INVOICING,
        "report.sales_by_customer",
        "Sales (before tax) by customer",
        false,
        sales_by_customer,
    );
    r.add_in(
        INVOICING,
        "report.sales_by_item",
        "Sales (before tax) by item",
        false,
        sales_by_item,
    );
    r.add_in(
        SALES_TAX,
        "report.sales_tax_liability",
        "Sales tax collected, paid and owed per rate",
        false,
        tax_liability,
    );
    r.add(
        "report.balance_sheet",
        "Assets, liabilities and equity as of a date",
        false,
        balance_sheet,
    );
}

pub(crate) fn plus(a: Decimal, b: Decimal) -> Decimal {
    a.checked_add(b).unwrap_or(a)
}

pub(crate) fn minus(a: Decimal, b: Decimal) -> Decimal {
    a.checked_sub(b).unwrap_or(a)
}

pub(crate) fn pct(part: Decimal, of: Decimal) -> String {
    if of.is_zero() {
        return String::new();
    }
    part.checked_mul(Decimal::ONE_HUNDRED)
        .and_then(|x| x.checked_div(of))
        .map(|x| mk_core::currency::format_amount(sales::round2(x).round_dp(1), Some(1)))
        .unwrap_or_default()
}

pub(crate) fn today_or(d: Option<Date>) -> CommandResult<Date> {
    d.map_or_else(|| Ok(mk_store::today_utc()?), Ok)
}

/// The ledger balance of the receivable account as of a date.
fn ar_ledger(ctx: &Ctx<'_>, as_of: Date) -> CommandResult<Option<Decimal>> {
    let Some(id) = receivable_id(ctx)? else {
        return Ok(None);
    };
    let mut total = Decimal::ZERO;
    for row in ctx.tx.posted_splits(Some(as_of))? {
        if row.account == id {
            total = plus(total, row.value);
        }
    }
    Ok(Some(total))
}

/// The receivable account if it exists (never created by a report).
fn receivable_id(ctx: &Ctx<'_>) -> CommandResult<Option<AccountId>> {
    let chart = Chart::load(ctx)?;
    if let Some(id) = crate::module_settings::get(ctx, "invoicing.receivable_account")?.as_str()
        && let Ok(a) = chart.resolve(id)
    {
        return Ok(Some(a.id));
    }
    if let Some(a) = crate::chart::roles::holder(&chart, crate::chart::roles::RECEIVABLE) {
        return Ok(Some(a.id));
    }
    Ok(chart
        .accounts
        .iter()
        .find(|a| {
            a.parent.is_none()
                && a.subtype == mk_core::AccountSubtype::Receivable
                && a.name.eq_ignore_ascii_case("Accounts Receivable")
        })
        .map(|a| a.id))
}

/// Open invoices as of a date: (invoice, balance) with a non-zero balance.
fn open_as_of(
    ctx: &Ctx<'_>,
    l: &Ledgerish,
    as_of: Date,
) -> CommandResult<Vec<(sales::SalesDoc, Decimal)>> {
    Ok(docs(ctx, "invoice")?
        .into_iter()
        .filter(|d| !matches!(d.status.as_str(), "draft" | "void") && d.date <= as_of)
        .map(|d| {
            let (_, b, _) = invoice_state(&d, l, Some(as_of));
            (d, b)
        })
        .filter(|(_, b)| !b.is_zero())
        .collect())
}

/// Unapplied credit per customer as of a date (positive amounts).
fn credits_as_of(l: &Ledgerish, as_of: Date) -> BTreeMap<String, Decimal> {
    let mut out: BTreeMap<String, Decimal> = BTreeMap::new();
    for p in &l.payments {
        let u = l.unapplied_payment(p, Some(as_of));
        if !u.is_zero() {
            let e = out.entry(p.customer.clone()).or_default();
            *e = plus(*e, u);
        }
    }
    for c in &l.credits {
        let u = l.unapplied_credit(c, Some(as_of));
        if !u.is_zero() {
            let e = out.entry(c.customer.clone()).or_default();
            *e = plus(*e, u);
        }
    }
    out
}

/// Per customer: bucket balances, unapplied credit, invoice detail rows.
type AgingRow = ([Decimal; 5], Decimal, Vec<Value>);

pub(crate) const BUCKETS: [&str; 5] = ["Current", "1-30", "31-60", "61-90", "Over 90"];

pub(crate) fn bucket(days: i64) -> usize {
    match days {
        i64::MIN..=0 => 0,
        1..=30 => 1,
        31..=60 => 2,
        61..=90 => 3,
        _ => 4,
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AgingArgs {
    #[serde(default)]
    as_of: Option<Date>,
    /// `due_date` (default) or `invoice_date`.
    #[serde(default)]
    basis: Option<String>,
    #[serde(default)]
    detail: bool,
}

fn ar_aging(ctx: &Ctx<'_>, a: AgingArgs) -> CommandResult<Value> {
    let as_of = today_or(a.as_of)?;
    let basis = a.basis.unwrap_or_else(|| "due_date".into());
    if !matches!(basis.as_str(), "due_date" | "invoice_date") {
        return Err(CommandError::InvalidArgs(
            "basis is due_date or invoice_date".into(),
        ));
    }
    let l = Ledgerish::load(ctx)?;
    let names: HashMap<String, String> =
        parties(ctx)?.into_iter().map(|p| (p.id, p.name)).collect();
    // customer → (buckets, credits, detail rows)
    let mut rows: BTreeMap<String, AgingRow> = BTreeMap::new();
    for (d, bal) in open_as_of(ctx, &l, as_of)? {
        let e = rows.entry(d.customer.clone()).or_default();
        if bal.is_sign_negative() {
            // An invoice with a negative balance behaves as a credit.
            e.1 = plus(e.1, bal.abs());
            continue;
        }
        let from = if basis == "due_date" {
            d.due_date.unwrap_or(d.date)
        } else {
            d.date
        };
        let days = from.days_until(as_of);
        let b = bucket(days);
        if let Some(slot) = e.0.get_mut(b) {
            *slot = plus(*slot, bal);
        }
        e.2.push(json!({
            "invoice": d.id, "number": d.number, "date": d.date, "due_date": d.due_date,
            "days_past_due": days.max(0), "bucket": BUCKETS.get(b), "balance": money(bal),
        }));
    }
    for (c, amt) in credits_as_of(&l, as_of) {
        let e = rows.entry(c).or_default();
        e.1 = plus(e.1, amt);
    }
    let mut totals = [Decimal::ZERO; 5];
    let mut credit_total = Decimal::ZERO;
    let mut out = Vec::new();
    let mut ordered: Vec<(String, AgingRow)> = rows.into_iter().collect();
    ordered.sort_by_key(|(c, _)| names.get(c).map(|n| n.to_lowercase()).unwrap_or_default());
    for (c, (b, credit, detail)) in ordered {
        let total = minus(b.iter().fold(Decimal::ZERO, |s, x| plus(s, *x)), credit);
        if total.is_zero() && b.iter().all(|x| x.is_zero()) && credit.is_zero() {
            continue;
        }
        for (t, x) in totals.iter_mut().zip(b.iter()) {
            *t = plus(*t, *x);
        }
        credit_total = plus(credit_total, credit);
        let mut row = json!({
            "customer": c, "customer_name": names.get(&c),
            "buckets": b.iter().map(|x| money(*x)).collect::<Vec<_>>(),
            "credits": money(-credit),
            "total": money(total),
        });
        if a.detail
            && let Some(o) = row.as_object_mut()
        {
            o.insert("invoices".into(), Value::Array(detail));
        }
        out.push(row);
    }
    let grand = minus(
        totals.iter().fold(Decimal::ZERO, |s, x| plus(s, *x)),
        credit_total,
    );
    let ledger = ar_ledger(ctx, as_of)?.unwrap_or_default();
    let difference = minus(ledger, grand);
    Ok(json!({
        "as_of": as_of, "basis": basis,
        "columns": BUCKETS,
        "customers": out,
        "totals": {"buckets": totals.iter().map(|x| money(*x)).collect::<Vec<_>>(), "credits": money(-credit_total), "total": money(grand)},
        "ar_balance": money(ledger),
        "difference": money(difference),
        "balanced": difference.is_zero(),
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AsOfArgs {
    #[serde(default)]
    as_of: Option<Date>,
}

fn customer_balances(ctx: &Ctx<'_>, a: AsOfArgs) -> CommandResult<Value> {
    let as_of = today_or(a.as_of)?;
    let l = Ledgerish::load(ctx)?;
    // customer → (open, credit, invoices, overdue)
    let mut by: BTreeMap<String, (Decimal, Decimal, usize, Decimal)> = BTreeMap::new();
    for (d, bal) in open_as_of(ctx, &l, as_of)? {
        let e = by.entry(d.customer.clone()).or_default();
        e.0 = plus(e.0, bal);
        e.2 += 1;
        if bal > Decimal::ZERO && d.due_date.is_some_and(|x| as_of > x) {
            e.3 = plus(e.3, bal);
        }
    }
    for (c, amt) in credits_as_of(&l, as_of) {
        let e = by.entry(c).or_default();
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
            json!({"customer": p.id, "customer_name": p.name, "open_invoices": n,
            "open": money(*open), "overdue": money(*overdue), "credits": money(-*credit),
            "balance": money(bal)}),
        );
    }
    Ok(json!({"as_of": as_of, "customers": out, "total": money(total)}))
}

fn open_invoices(ctx: &Ctx<'_>, a: AsOfArgs) -> CommandResult<Value> {
    let as_of = today_or(a.as_of)?;
    let l = Ledgerish::load(ctx)?;
    let names: HashMap<String, String> =
        parties(ctx)?.into_iter().map(|p| (p.id, p.name)).collect();
    let mut out = Vec::new();
    let mut total = Decimal::ZERO;
    for (d, bal) in open_as_of(ctx, &l, as_of)? {
        total = plus(total, bal);
        let overdue = d.due_date.map_or(0, |x| x.days_until(as_of).max(0));
        out.push(
            json!({"invoice": d.id, "number": d.number, "customer": d.customer,
            "customer_name": names.get(&d.customer), "date": d.date, "due_date": d.due_date,
            "days_overdue": overdue, "total": money(d.totals.total), "balance": money(bal)}),
        );
    }
    Ok(json!({"as_of": as_of, "invoices": out, "total": money(total)}))
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SalesArgs {
    #[serde(flatten)]
    dates: Dates,
}

/// Posted invoice and credit memo lines in a date range, with their sign.
fn sold_lines(
    ctx: &Ctx<'_>,
    from: Date,
    to: Date,
) -> CommandResult<Vec<(String, Decimal, sales::LineOut)>> {
    let mut out = Vec::new();
    for kind in ["invoice", "credit_memo"] {
        let sign = if kind == "credit_memo" {
            Decimal::NEGATIVE_ONE
        } else {
            Decimal::ONE
        };
        for d in docs(ctx, kind)? {
            if matches!(d.status.as_str(), "draft" | "void") || d.date < from || d.date > to {
                continue;
            }
            for line in d.totals.lines.iter().filter(|x| x.kind != "subtotal") {
                out.push((d.customer.clone(), sign, line.clone()));
            }
        }
    }
    Ok(out)
}

fn sales_by_customer(ctx: &Ctx<'_>, a: SalesArgs) -> CommandResult<Value> {
    let (from, to) = crate::reports::resolve(ctx, &a.dates)?;
    let mut by: BTreeMap<String, Decimal> = BTreeMap::new();
    let mut total = Decimal::ZERO;
    for (c, sign, l) in sold_lines(ctx, from, to)? {
        let v = l.amount.checked_mul(sign).unwrap_or_default();
        let e = by.entry(c).or_default();
        *e = plus(*e, v);
        total = plus(total, v);
    }
    let mut rows = Vec::new();
    for p in parties(ctx)? {
        if let Some(v) = by.get(&p.id) {
            rows.push(json!({"customer": p.id, "customer_name": p.name, "amount": money(*v), "percent": pct(*v, total)}));
        }
    }
    Ok(json!({"from": from, "to": to, "customers": rows, "total": money(total)}))
}

fn sales_by_item(ctx: &Ctx<'_>, a: SalesArgs) -> CommandResult<Value> {
    let (from, to) = crate::reports::resolve(ctx, &a.dates)?;
    let chart = Chart::load(ctx)?;
    // name → (quantity, amount)
    let mut by: BTreeMap<String, (Option<Decimal>, Decimal)> = BTreeMap::new();
    let mut total = Decimal::ZERO;
    for (_, sign, l) in sold_lines(ctx, from, to)? {
        let name = match (&l.item_name, l.account) {
            (Some(n), _) => n.clone(),
            (None, Some(acc)) => format!("(No item) {}", chart.full_name(acc)),
            _ => "(No item)".into(),
        };
        let v = l.amount.checked_mul(sign).unwrap_or_default();
        let e = by.entry(name).or_default();
        if l.kind == "item" {
            let q = l
                .quantity
                .unwrap_or_default()
                .checked_mul(sign)
                .unwrap_or_default();
            e.0 = Some(plus(e.0.unwrap_or_default(), q));
        }
        e.1 = plus(e.1, v);
        total = plus(total, v);
    }
    let mut rows: Vec<(String, (Option<Decimal>, Decimal))> = by.into_iter().collect();
    rows.sort_by_key(|(n, _)| n.to_lowercase());
    Ok(json!({
        "from": from, "to": to,
        "items": rows.iter().map(|(n, (q, v))| json!({"item": n, "quantity": q.map(|q| q.normalize().to_string()), "amount": money(*v), "percent": pct(*v, total)})).collect::<Vec<_>>(),
        "total": money(total),
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LiabilityArgs {
    #[serde(flatten)]
    dates: Dates,
    /// `accrual` (the only basis in this version).
    #[serde(default)]
    basis: Option<String>,
}

fn tax_liability(ctx: &Ctx<'_>, a: LiabilityArgs) -> CommandResult<Value> {
    if a.basis.as_deref().is_some_and(|b| b != "accrual") {
        return Err(CommandError::InvalidArgs("basis is accrual".into()));
    }
    let (from, to) = crate::reports::resolve(ctx, &a.dates)?;
    let period = sales::tax_balances(ctx, Some(from), to)?;
    let cumulative = sales::tax_balances(ctx, None, to)?;
    let mut rows = Vec::new();
    let mut tot = [Decimal::ZERO; 5];
    for (id, (name, _, _, c, p)) in &cumulative {
        let (taxable, nontaxable, collected, paid) = period.get(id).map_or(
            (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO, Decimal::ZERO),
            |x| (x.1, x.2, x.3, x.4),
        );
        let balance = minus(*c, *p);
        if [taxable, collected, paid, balance]
            .iter()
            .all(|x| x.is_zero())
        {
            continue;
        }
        for (t, v) in tot
            .iter_mut()
            .zip([taxable, nontaxable, collected, paid, balance])
        {
            *t = plus(*t, v);
        }
        rows.push(
            json!({"rate": id, "name": name, "taxable_sales": money(taxable),
            "nontaxable_sales": money(nontaxable), "collected": money(collected),
            "paid": money(paid), "balance": money(balance)}),
        );
    }
    let [t0, t1, t2, t3, t4] = tot;
    Ok(
        json!({"from": from, "to": to, "basis": "accrual", "rates": rows,
        "totals": {"taxable_sales": money(t0), "nontaxable_sales": money(t1), "collected": money(t2), "paid": money(t3), "balance": money(t4)}}),
    )
}

// ----- balance sheet ------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BalanceSheetArgs {
    #[serde(default)]
    as_of: Option<Date>,
    /// Keep this many levels of each account name (default: all).
    #[serde(default)]
    depth: Option<usize>,
    /// `fund`: a column per fund (module funds).
    #[serde(default)]
    columns: Option<String>,
}

/// The first day of the fiscal year containing `as_of`.
pub(crate) fn fiscal_year_start(ctx: &Ctx<'_>, as_of: Date) -> CommandResult<Date> {
    let s = crate::module_settings::get_str(ctx, "book.fiscal_year_start")?;
    let (m, d) = crate::module_settings::month_day(&s).unwrap_or((1, 1));
    let this = Date::from_ymd(as_of.year(), m, d)?;
    Ok(if this <= as_of {
        this
    } else {
        Date::from_ymd(as_of.year().saturating_sub(1), m, d)?
    })
}

fn balance_sheet(ctx: &Ctx<'_>, a: BalanceSheetArgs) -> CommandResult<Value> {
    let as_of = today_or(a.as_of)?;
    match a.columns.as_deref() {
        None | Some("total") => {}
        Some("fund") => {
            if !module_on(ctx, crate::dimensions::FUNDS)? {
                return Err(CommandError::ModuleDisabled {
                    command: "report.balance_sheet".into(),
                    module: crate::dimensions::FUNDS.into(),
                });
            }
            return crate::dimension_reports::balance_by_fund_at(ctx, as_of);
        }
        Some(_) => return Err(CommandError::InvalidArgs("columns is total or fund".into())),
    }
    let fy = fiscal_year_start(ctx, as_of)?;
    let chart = Chart::load(ctx)?;
    let base: mk_core::CommodityCode = ctx.tx.info()?.base_currency.parse()?;
    let mut own: HashMap<AccountId, Decimal> = HashMap::new();
    let mut retained = Decimal::ZERO;
    let mut net = Decimal::ZERO;
    let mut skipped = 0_usize;
    for row in ctx.tx.posted_splits(Some(as_of))? {
        if row.currency != base {
            skipped += 1;
            continue;
        }
        let Some(acc) = chart.get(row.account) else {
            continue;
        };
        match acc.account_type {
            AccountType::Income | AccountType::Expense => {
                if row.date < fy {
                    retained = minus(retained, row.value);
                } else {
                    net = minus(net, row.value);
                }
            }
            _ => {
                let e = own.entry(row.account).or_default();
                *e = plus(*e, row.value);
            }
        }
    }
    // chart-presets.md: an account holding the retained_earnings role shows the
    // computed retained earnings as its own balance instead of a separate line.
    let retained_holder =
        crate::chart::roles::holder(&chart, crate::chart::roles::RETAINED_EARNINGS)
            .filter(|h| h.account_type == AccountType::Equity && !h.placeholder)
            .map(|h| h.id);
    let mut retained_line = retained;
    if let Some(h) = retained_holder
        && !retained.is_zero()
    {
        let e = own.entry(h).or_default();
        *e = minus(*e, retained);
        retained_line = Decimal::ZERO;
    }
    // Totals including descendants, shown with the section's natural sign.
    let section = |ty: AccountType| -> (Vec<Value>, Decimal) {
        let sign = if ty == AccountType::Asset {
            Decimal::ONE
        } else {
            Decimal::NEGATIVE_ONE
        };
        let mut rows = Vec::new();
        let mut total = Decimal::ZERO;
        let tops: Vec<&mk_core::Account> = chart
            .accounts
            .iter()
            .filter(|x| x.account_type == ty && x.parent.is_none())
            .collect();
        fn walk(
            chart: &Chart,
            own: &HashMap<AccountId, Decimal>,
            id: AccountId,
            level: usize,
            depth: Option<usize>,
            sign: Decimal,
            rows: &mut Vec<Value>,
        ) -> Decimal {
            let sub: Decimal = chart
                .subtree(id)
                .iter()
                .filter_map(|x| own.get(x))
                .fold(Decimal::ZERO, |s, x| plus(s, *x));
            let shown = sub.checked_mul(sign).unwrap_or_default();
            if !sub.is_zero() && depth.is_none_or(|d| level < d) {
                rows.push(
                    json!({"account": id, "name": chart.get(id).map(|a| a.name.clone()),
                    "full_name": chart.full_name(id), "level": level, "balance": money(shown)}),
                );
                let kids: Vec<AccountId> = chart.children(id).map(|c| c.id).collect();
                for k in kids {
                    walk(chart, own, k, level + 1, depth, sign, rows);
                }
            }
            shown
        }
        for t in tops {
            total = plus(total, walk(&chart, &own, t.id, 0, a.depth, sign, &mut rows));
        }
        (rows, total)
    };
    let (assets, ta) = section(AccountType::Asset);
    let (liabilities, tl) = section(AccountType::Liability);
    let (mut equity, te_accounts) = section(AccountType::Equity);
    if !retained_line.is_zero() {
        equity.push(json!({"account": null, "name": "Retained earnings", "full_name": "Retained earnings", "level": 0, "balance": money(retained_line), "computed": true}));
    }
    if !net.is_zero() {
        equity.push(json!({"account": null, "name": "Net income", "full_name": "Net income", "level": 0, "balance": money(net), "computed": true}));
    }
    let te = plus(plus(te_accounts, retained_line), net);
    let le = plus(tl, te);
    let difference = minus(ta, le);
    Ok(json!({
        "as_of": as_of, "fiscal_year_start": fy,
        "assets": assets, "total_assets": money(ta),
        "liabilities": liabilities, "total_liabilities": money(tl),
        "equity": equity, "total_equity": money(te),
        "retained_earnings": money(retained), "net_income": money(net),
        "total_liabilities_and_equity": money(le),
        "difference": money(difference), "balanced": difference.is_zero(),
        "foreign_currency_splits_excluded": skipped,
    }))
}

// ----- CSV ----------------------------------------------------------------------------

pub(crate) fn s(v: &Value) -> String {
    match v {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub(crate) fn at<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&Value::Null)
}

pub(crate) fn list(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

pub(crate) fn h(cols: &[&str]) -> Vec<String> {
    cols.iter().map(|c| (*c).to_owned()).collect()
}

/// CSV for the reports in this module: `Some((data, csv))`, or `None` for others.
pub(crate) fn export(
    ctx: &Ctx<'_>,
    name: &str,
    args: Value,
) -> CommandResult<Option<(Value, String)>> {
    let module = match name {
        "ar_aging" | "customer_balances" | "open_invoices" | "sales_by_customer"
        | "sales_by_item" => Some(INVOICING),
        "sales_tax_liability" => Some(SALES_TAX),
        "balance_sheet" => None,
        _ => return Ok(None),
    };
    if let Some(m) = module
        && !module_on(ctx, m)?
    {
        return Err(CommandError::ModuleDisabled {
            command: format!("report.{name}"),
            module: m.into(),
        });
    }
    Ok(Some(match name {
        "ar_aging" => {
            let d = ar_aging(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&[
                "Customer", "Current", "1-30", "31-60", "61-90", "Over 90", "Credits", "Total",
            ]));
            for r in list(at(&d, "customers")) {
                let mut row = vec![s(at(&r, "customer_name"))];
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
        "customer_balances" => {
            let d = customer_balances(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&[
                "Customer",
                "Open invoices",
                "Open",
                "Credits",
                "Balance",
            ]));
            for r in list(at(&d, "customers")) {
                c.row(&[
                    s(at(&r, "customer_name")),
                    s(at(&r, "open_invoices")),
                    s(at(&r, "open")),
                    s(at(&r, "credits")),
                    s(at(&r, "balance")),
                ]);
            }
            c.row(&[
                "Total".into(),
                String::new(),
                String::new(),
                String::new(),
                s(at(&d, "total")),
            ]);
            (d, c.out)
        }
        "open_invoices" => {
            let d = open_invoices(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&[
                "Number",
                "Customer",
                "Date",
                "Due date",
                "Days overdue",
                "Total",
                "Balance",
            ]));
            for r in list(at(&d, "invoices")) {
                c.row(&[
                    s(at(&r, "number")),
                    s(at(&r, "customer_name")),
                    s(at(&r, "date")),
                    s(at(&r, "due_date")),
                    s(at(&r, "days_overdue")),
                    s(at(&r, "total")),
                    s(at(&r, "balance")),
                ]);
            }
            (d, c.out)
        }
        "sales_by_customer" => {
            let d = sales_by_customer(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&["Customer", "Amount", "Percent"]));
            for r in list(at(&d, "customers")) {
                c.row(&[
                    s(at(&r, "customer_name")),
                    s(at(&r, "amount")),
                    s(at(&r, "percent")),
                ]);
            }
            c.row(&["Total".into(), s(at(&d, "total"))]);
            (d, c.out)
        }
        "sales_by_item" => {
            let d = sales_by_item(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&["Item", "Quantity", "Amount", "Percent"]));
            for r in list(at(&d, "items")) {
                c.row(&[
                    s(at(&r, "item")),
                    s(at(&r, "quantity")),
                    s(at(&r, "amount")),
                    s(at(&r, "percent")),
                ]);
            }
            c.row(&["Total".into(), String::new(), s(at(&d, "total"))]);
            (d, c.out)
        }
        "sales_tax_liability" => {
            let d = tax_liability(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&[
                "Rate",
                "Taxable sales",
                "Non-taxable sales",
                "Tax collected",
                "Tax paid",
                "Balance",
            ]));
            for r in list(at(&d, "rates")) {
                c.row(&[
                    s(at(&r, "name")),
                    s(at(&r, "taxable_sales")),
                    s(at(&r, "nontaxable_sales")),
                    s(at(&r, "collected")),
                    s(at(&r, "paid")),
                    s(at(&r, "balance")),
                ]);
            }
            (d, c.out)
        }
        _ => {
            let d = balance_sheet(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&["Section", "Account", "Balance"]));
            for (sec, key, total, label) in [
                ("Assets", "assets", "total_assets", "Total assets"),
                (
                    "Liabilities",
                    "liabilities",
                    "total_liabilities",
                    "Total liabilities",
                ),
                ("Equity", "equity", "total_equity", "Total equity"),
            ] {
                for r in list(at(&d, key)) {
                    c.row(&[sec.into(), s(at(&r, "full_name")), s(at(&r, "balance"))]);
                }
                c.row(&[sec.into(), label.into(), s(at(&d, total))]);
            }
            c.row(&[
                String::new(),
                "Total liabilities and equity".into(),
                s(at(&d, "total_liabilities_and_equity")),
            ]);
            (d, c.out)
        }
    }))
}
