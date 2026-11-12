//! Profit and loss with dimension or period columns, the balance sheet by fund, and
//! the nonprofit reports (business-dimensions.md): statement of activities, statement
//! of financial position, functional expenses and fund activity.

use std::collections::{BTreeMap, BTreeSet};

use mk_core::{AccountId, AccountSubtype, AccountType, Date, Decimal};
use mk_store::{DocTable, SplitRow};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::dimensions::{
    DimRecord, FUNDS, GENERAL, RELEASE_ACCOUNT, TRANSFER_ACCOUNT, fund_of, net_assets, records,
    tag_of,
};
use crate::reports::{Csv, Dates, Interval};
use crate::sales::money;
use crate::sales_reports::{at, h, list, s};
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "report.profit_loss",
        "Profit and loss, optionally by class, location, fund, project or period",
        false,
        profit_loss,
    );
    r.add_in(
        FUNDS,
        "report.balance_sheet_by_fund",
        "Balance sheet with a column per fund",
        false,
        balance_by_fund,
    );
    r.add_in(
        FUNDS,
        "report.statement_of_activities",
        "Revenue, releases and expenses by net asset class",
        false,
        activities,
    );
    r.add_in(
        FUNDS,
        "report.statement_of_financial_position",
        "Assets, liabilities and net assets by class and fund",
        false,
        financial_position,
    );
    r.add_in(
        FUNDS,
        "report.functional_expenses",
        "Expenses by nature and function",
        false,
        functional,
    );
    r.add_in(
        FUNDS,
        "report.fund_activity",
        "Each fund's net assets and their changes",
        false,
        fund_activity,
    );
}

fn plus(a: Decimal, b: Decimal) -> Decimal {
    a.checked_add(b).unwrap_or(a)
}

fn minus(a: Decimal, b: Decimal) -> Decimal {
    a.checked_sub(b).unwrap_or(a)
}

fn sum<'a>(v: impl IntoIterator<Item = &'a Decimal>) -> Decimal {
    v.into_iter().fold(Decimal::ZERO, |s, x| plus(s, *x))
}

/// Posted base-currency splits dated in [from, to].
fn splits_in(ctx: &Ctx<'_>, from: Option<Date>, to: Date) -> CommandResult<Vec<SplitRow>> {
    let base: mk_core::CommodityCode = ctx.tx.info()?.base_currency.parse()?;
    Ok(ctx
        .tx
        .posted_splits(Some(to))?
        .into_iter()
        .filter(|r| r.currency == base && from.is_none_or(|f| r.date >= f))
        .collect())
}

fn top_named(chart: &Chart, id: AccountId, name: &str) -> bool {
    chart
        .get(id)
        .is_some_and(|a| a.parent.is_none() && a.name == name)
}

/// The account a row rolls up to at `depth` levels (all levels when `None`).
fn rollup(chart: &Chart, id: AccountId, depth: Option<usize>) -> AccountId {
    let Some(d) = depth else { return id };
    let mut chain = vec![id];
    let mut cur = id;
    while let Some(p) = chart.get(cur).and_then(|a| a.parent) {
        chain.push(p);
        cur = p;
        if chain.len() > chart.accounts.len() {
            break;
        }
    }
    chain.reverse();
    chain
        .get(d.saturating_sub(1).min(chain.len().saturating_sub(1)))
        .copied()
        .unwrap_or(id)
}

// ----- profit and loss ----------------------------------------------------------------

/// (section, account) → (per column, total).
type PlEntry<'a> = (&'a (u8, AccountId), &'a (Vec<Decimal>, Decimal));

#[derive(Debug, Deserialize, JsonSchema)]
struct PlArgs {
    #[serde(flatten)]
    dates: Dates,
    /// total (default), class, location, fund, project, month, quarter or year.
    #[serde(default)]
    columns: Option<String>,
    /// Keep this many levels of each account name (default: all).
    #[serde(default)]
    depth: Option<usize>,
}

fn profit_loss(ctx: &Ctx<'_>, a: PlArgs) -> CommandResult<Value> {
    let (from, to) = crate::reports::resolve(ctx, &a.dates)?;
    let mode = a.columns.unwrap_or_else(|| "total".into());
    let chart = Chart::load(ctx)?;
    // (key, label) columns and the column of a split.
    let (cols, periods): (Vec<(String, String)>, Vec<crate::reports::Period>) = match mode.as_str()
    {
        "total" => (Vec::new(), Vec::new()),
        "class" | "location" | "fund" => {
            let module = match mode.as_str() {
                "class" => crate::dimensions::CLASSES,
                "location" => crate::dimensions::LOCATIONS,
                _ => FUNDS,
            };
            if !crate::sales::module_on(ctx, module)? {
                return Err(CommandError::ModuleDisabled {
                    command: "report.profit_loss".into(),
                    module: module.into(),
                });
            }
            let mut c: Vec<(String, String)> = records(ctx, &mode)?
                .into_iter()
                .map(|r| (r.id, r.name))
                .collect();
            if mode != "fund" {
                c.push((String::new(), "Not set".into()));
            }
            (c, Vec::new())
        }
        "project" => {
            if !crate::sales::module_on(ctx, crate::projects::MODULE)? {
                return Err(CommandError::ModuleDisabled {
                    command: "report.profit_loss".into(),
                    module: crate::projects::MODULE.into(),
                });
            }
            let mut c: Vec<(String, String)> = crate::projects::all(ctx)?
                .into_iter()
                .map(|p| (p.id, p.name))
                .collect();
            c.push((String::new(), "Not set".into()));
            (c, Vec::new())
        }
        "month" | "quarter" | "year" => {
            let interval = match mode.as_str() {
                "month" => Interval::Month,
                "quarter" => Interval::Quarter,
                _ => Interval::Year,
            };
            let ps = crate::reports::periods(from, to, interval)?;
            (
                ps.iter()
                    .map(|p| (p.label.clone(), p.label.clone()))
                    .collect(),
                ps,
            )
        }
        _ => {
            return Err(CommandError::InvalidArgs(
                "columns is total, class, location, fund, project, month, quarter or year".into(),
            ));
        }
    };
    let col_of = |r: &SplitRow| -> Option<usize> {
        match mode.as_str() {
            "total" => None,
            "class" | "location" | "project" => {
                let k = tag_of(&r.tags, &mode).unwrap_or("");
                cols.iter().position(|c| c.0 == k)
            }
            "fund" => cols.iter().position(|c| c.0 == fund_of(&r.tags)),
            _ => periods
                .iter()
                .position(|p| r.date >= p.from && r.date <= p.to),
        }
    };
    let n = cols.len();
    // section → account → (per column, total)
    let mut by: BTreeMap<(u8, AccountId), (Vec<Decimal>, Decimal)> = BTreeMap::new();
    for r in splits_in(ctx, Some(from), to)? {
        let Some(acc) = chart.get(r.account) else {
            continue;
        };
        let (section, amount) = match acc.account_type {
            AccountType::Income => (0_u8, -r.value),
            AccountType::Expense if acc.subtype == AccountSubtype::CostOfGoods => (1, r.value),
            AccountType::Expense => (2, r.value),
            _ => continue,
        };
        let key = (section, rollup(&chart, r.account, a.depth));
        let e = by
            .entry(key)
            .or_insert_with(|| (vec![Decimal::ZERO; n], Decimal::ZERO));
        if let Some(c) = col_of(&r).and_then(|i| e.0.get_mut(i)) {
            *c = plus(*c, amount);
        }
        e.1 = plus(e.1, amount);
    }
    let mut sections = Vec::new();
    let mut section_totals: Vec<(Vec<Decimal>, Decimal)> = Vec::new();
    for (k, title) in [(0_u8, "Income"), (1, "Cost of goods sold"), (2, "Expenses")] {
        let mut rows = Vec::new();
        let mut tc = vec![Decimal::ZERO; n];
        let mut tt = Decimal::ZERO;
        let mut accounts: Vec<PlEntry<'_>> = by.iter().filter(|((sec, _), _)| *sec == k).collect();
        accounts.sort_by_key(|((_, id), _)| chart.full_name(*id).to_lowercase());
        for ((_, id), (vals, total)) in accounts {
            if total.is_zero() && vals.iter().all(Decimal::is_zero) {
                continue;
            }
            for (t, v) in tc.iter_mut().zip(vals) {
                *t = plus(*t, *v);
            }
            tt = plus(tt, *total);
            rows.push(json!({"account": id, "name": chart.full_name(*id),
                "amounts": vals.iter().map(|v| money(*v)).collect::<Vec<_>>(), "total": money(*total)}));
        }
        sections.push(json!({"name": title, "rows": rows,
            "totals": tc.iter().map(|v| money(*v)).collect::<Vec<_>>(), "total": money(tt)}));
        section_totals.push((tc, tt));
    }
    let net: Vec<Decimal> = (0..n)
        .map(|i| {
            let g = |s: usize| {
                section_totals
                    .get(s)
                    .and_then(|x| x.0.get(i))
                    .copied()
                    .unwrap_or_default()
            };
            minus(minus(g(0), g(1)), g(2))
        })
        .collect();
    let t = |s: usize| section_totals.get(s).map(|x| x.1).unwrap_or_default();
    let net_total = minus(minus(t(0), t(1)), t(2));
    Ok(json!({
        "from": from, "to": to, "columns_mode": mode,
        "columns": cols.iter().map(|(k, l)| json!({"key": k, "label": l})).collect::<Vec<_>>(),
        "sections": sections,
        "net_income": {"amounts": net.iter().map(|v| money(*v)).collect::<Vec<_>>(), "total": money(net_total)},
        "gross_profit": money(minus(t(0), t(1))),
    }))
}

// ----- funds --------------------------------------------------------------------------

fn day_before(d: Date) -> Date {
    d.add_days(-1).unwrap_or(d)
}

/// Net assets of each restriction class as of a date: (without, with).
fn classes_of(funds: &[DimRecord], net: &BTreeMap<String, Decimal>) -> (Decimal, Decimal) {
    let mut w = Decimal::ZERO;
    let mut r = Decimal::ZERO;
    for (id, v) in net {
        if funds.iter().any(|f| f.id == *id && f.restricted()) {
            r = plus(r, *v);
        } else {
            w = plus(w, *v);
        }
    }
    (w, r)
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DatesArgs {
    #[serde(flatten)]
    dates: Dates,
}

fn activities(ctx: &Ctx<'_>, a: DatesArgs) -> CommandResult<Value> {
    let (from, to) = crate::reports::resolve(ctx, &a.dates)?;
    let chart = Chart::load(ctx)?;
    let funds = records(ctx, "fund")?;
    let restricted = |id: &str| funds.iter().any(|f| f.id == id && f.restricted());
    // account → (without, with)
    let mut revenue: BTreeMap<AccountId, (Decimal, Decimal)> = BTreeMap::new();
    let mut expenses: BTreeMap<AccountId, Decimal> = BTreeMap::new();
    let (mut rel_w, mut rel_r) = (Decimal::ZERO, Decimal::ZERO);
    let (mut other_w, mut other_r) = (Decimal::ZERO, Decimal::ZERO);
    for r in splits_in(ctx, Some(from), to)? {
        let Some(acc) = chart.get(r.account) else {
            continue;
        };
        let f = fund_of(&r.tags);
        let with = restricted(f);
        match acc.account_type {
            AccountType::Income if top_named(&chart, r.account, RELEASE_ACCOUNT) => {
                if with {
                    rel_r = minus(rel_r, r.value);
                } else {
                    rel_w = minus(rel_w, r.value);
                }
            }
            AccountType::Income => {
                let e = revenue.entry(r.account).or_default();
                if with {
                    e.1 = minus(e.1, r.value);
                } else {
                    e.0 = minus(e.0, r.value);
                }
            }
            AccountType::Expense => {
                let e = expenses.entry(r.account).or_default();
                *e = plus(*e, r.value);
            }
            AccountType::Equity => {
                if with {
                    other_r = minus(other_r, r.value);
                } else {
                    other_w = minus(other_w, r.value);
                }
            }
            _ => {}
        }
    }
    let rev_w = sum(revenue.values().map(|x| &x.0));
    let rev_r = sum(revenue.values().map(|x| &x.1));
    let exp = sum(expenses.values());
    let change_w = minus(plus(rev_w, rel_w), exp);
    let change_r = plus(rev_r, rel_r);
    let (begin_w, begin_r) = classes_of(&funds, &net_assets(ctx, day_before(from))?);
    let (end_w, end_r) = classes_of(&funds, &net_assets(ctx, to)?);
    let three = |w: Decimal, r: Decimal| json!([money(w), money(r), money(plus(w, r))]);
    let mut rev_rows: Vec<Value> = revenue
        .iter()
        .map(|(id, (w, r))| json!({"account": id, "name": chart.full_name(*id), "amounts": three(*w, *r)}))
        .collect();
    rev_rows.sort_by_key(|v| v["name"].as_str().unwrap_or_default().to_lowercase());
    let mut exp_rows: Vec<Value> = expenses
        .iter()
        .map(|(id, v)| json!({"account": id, "name": chart.full_name(*id), "amounts": three(*v, Decimal::ZERO)}))
        .collect();
    exp_rows.sort_by_key(|v| v["name"].as_str().unwrap_or_default().to_lowercase());
    Ok(json!({
        "from": from, "to": to,
        "columns": ["Without donor restrictions", "With donor restrictions", "Total"],
        "revenue": rev_rows, "total_revenue": three(rev_w, rev_r),
        "released": three(rel_w, rel_r),
        "expenses": exp_rows, "total_expenses": three(exp, Decimal::ZERO),
        "other_changes": three(other_w, other_r),
        "change_in_net_assets": three(change_w, change_r),
        "net_assets_beginning": three(begin_w, begin_r),
        "net_assets_end": three(end_w, end_r),
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AsOfArgs {
    #[serde(default)]
    as_of: Option<Date>,
}

fn as_of(d: Option<Date>) -> CommandResult<Date> {
    d.map_or_else(|| Ok(mk_store::today_utc()?), Ok)
}

fn financial_position(ctx: &Ctx<'_>, a: AsOfArgs) -> CommandResult<Value> {
    let day = as_of(a.as_of)?;
    let chart = Chart::load(ctx)?;
    let funds = records(ctx, "fund")?;
    let mut bal: BTreeMap<AccountId, Decimal> = BTreeMap::new();
    for r in splits_in(ctx, None, day)? {
        let e = bal.entry(r.account).or_default();
        *e = plus(*e, r.value);
    }
    let section = |ty: AccountType, sign: Decimal| -> (Vec<Value>, Decimal) {
        let mut rows: Vec<(String, Decimal)> = bal
            .iter()
            .filter(|(id, v)| !v.is_zero() && chart.get(**id).is_some_and(|x| x.account_type == ty))
            .map(|(id, v)| {
                (
                    chart.full_name(*id),
                    v.checked_mul(sign).unwrap_or_default(),
                )
            })
            .collect();
        rows.sort_by_key(|r| r.0.to_lowercase());
        let total = rows.iter().fold(Decimal::ZERO, |s, r| plus(s, r.1));
        (
            rows.into_iter()
                .map(|(n, v)| json!({"name": n, "balance": money(v)}))
                .collect(),
            total,
        )
    };
    let (assets, ta) = section(AccountType::Asset, Decimal::ONE);
    let (liabilities, tl) = section(AccountType::Liability, Decimal::NEGATIVE_ONE);
    let net = net_assets(ctx, day)?;
    let group = |restricted: bool| -> (Vec<Value>, Decimal) {
        let mut t = Decimal::ZERO;
        let rows = funds
            .iter()
            .filter(|f| f.restricted() == restricted)
            .map(|f| {
                let v = net.get(&f.id).copied().unwrap_or_default();
                t = plus(t, v);
                json!({"fund": f.id, "name": f.name, "net_assets": money(v),
                       "overspent": restricted && v < Decimal::ZERO})
            })
            .collect();
        (rows, t)
    };
    let (without, tw) = group(false);
    let (with, tr) = group(true);
    let total_na = plus(tw, tr);
    let difference = minus(minus(ta, tl), total_na);
    Ok(json!({
        "as_of": day, "assets": assets, "total_assets": money(ta),
        "liabilities": liabilities, "total_liabilities": money(tl),
        "without_donor_restrictions": without, "total_without": money(tw),
        "with_donor_restrictions": with, "total_with": money(tr),
        "total_net_assets": money(total_na),
        "total_liabilities_and_net_assets": money(plus(tl, total_na)),
        "difference": money(difference), "balanced": difference.is_zero(),
    }))
}

fn balance_by_fund(ctx: &Ctx<'_>, a: AsOfArgs) -> CommandResult<Value> {
    balance_by_fund_at(ctx, as_of(a.as_of)?)
}

/// The balance sheet with a column per fund (`report.balance_sheet` `columns: fund`).
pub(crate) fn balance_by_fund_at(ctx: &Ctx<'_>, day: Date) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let funds = records(ctx, "fund")?;
    let mut cols: Vec<(String, String)> = funds
        .iter()
        .map(|f| (f.id.clone(), f.name.clone()))
        .collect();
    cols.push((String::new(), "Not set".into()));
    let n = cols.len();
    let mut by: BTreeMap<AccountId, Vec<Decimal>> = BTreeMap::new();
    for r in splits_in(ctx, None, day)? {
        let Some(acc) = chart.get(r.account) else {
            continue;
        };
        if !matches!(
            acc.account_type,
            AccountType::Asset | AccountType::Liability
        ) {
            continue;
        }
        let k = r.tags.fund.clone().unwrap_or_default();
        let i = cols.iter().position(|c| c.0 == k).unwrap_or(n - 1);
        let v = if acc.account_type == AccountType::Liability {
            -r.value
        } else {
            r.value
        };
        let e = by
            .entry(r.account)
            .or_insert_with(|| vec![Decimal::ZERO; n]);
        if let Some(slot) = e.get_mut(i) {
            *slot = plus(*slot, v);
        }
    }
    let rows_of = |ty: AccountType| -> (Vec<Value>, Vec<Decimal>) {
        let mut t = vec![Decimal::ZERO; n];
        let mut rows: Vec<(String, Vec<Decimal>)> = by
            .iter()
            .filter(|(id, _)| chart.get(**id).is_some_and(|x| x.account_type == ty))
            .map(|(id, v)| (chart.full_name(*id), v.clone()))
            .filter(|(_, v)| v.iter().any(|x| !x.is_zero()))
            .collect();
        rows.sort_by_key(|r| r.0.to_lowercase());
        for (_, v) in &rows {
            for (a, b) in t.iter_mut().zip(v) {
                *a = plus(*a, *b);
            }
        }
        (
            rows.into_iter()
                .map(|(name, v)| json!({"name": name, "amounts": v.iter().map(|x| money(*x)).collect::<Vec<_>>(), "total": money(sum(&v))}))
                .collect(),
            t,
        )
    };
    let (assets, ta) = rows_of(AccountType::Asset);
    let (liabilities, tl) = rows_of(AccountType::Liability);
    let net = net_assets(ctx, day)?;
    let na: Vec<Decimal> = cols
        .iter()
        .map(|(k, _)| {
            if k.is_empty() {
                Decimal::ZERO
            } else {
                net.get(k).copied().unwrap_or_default()
            }
        })
        .collect();
    let m = |v: &[Decimal]| v.iter().map(|x| money(*x)).collect::<Vec<_>>();
    Ok(json!({
        "as_of": day,
        "columns": cols.iter().map(|(k, l)| json!({"key": k, "label": l})).collect::<Vec<_>>(),
        "assets": assets, "total_assets": m(&ta), "total_assets_all": money(sum(&ta)),
        "liabilities": liabilities, "total_liabilities": m(&tl), "total_liabilities_all": money(sum(&tl)),
        "net_assets": m(&na), "total_net_assets": money(sum(&na)),
        "balanced": minus(sum(&ta), sum(&tl)) == sum(&na),
    }))
}

const FUNCTIONS: [(&str, &str); 4] = [
    ("program", "Program"),
    ("management_general", "Management and general"),
    ("fundraising", "Fundraising"),
    ("", "Not assigned"),
];

fn functional(ctx: &Ctx<'_>, a: DatesArgs) -> CommandResult<Value> {
    let (from, to) = crate::reports::resolve(ctx, &a.dates)?;
    let chart = Chart::load(ctx)?;
    let classes = records(ctx, "class")?;
    // A class's function, else its nearest parent's.
    let function_of = |id: Option<&str>| -> &str {
        let Some(c) = id.and_then(|id| classes.iter().find(|c| c.id == id)) else {
            return "";
        };
        let mut name = c.name.as_str();
        loop {
            if let Some(f) = classes
                .iter()
                .find(|x| x.name == name)
                .and_then(|x| x.function.as_deref())
            {
                return f;
            }
            match name.rsplit_once(':') {
                Some((p, _)) => name = p,
                None => return "",
            }
        }
    };
    let mut by: BTreeMap<AccountId, [Decimal; 4]> = BTreeMap::new();
    for r in splits_in(ctx, Some(from), to)? {
        if !chart
            .get(r.account)
            .is_some_and(|x| x.account_type == AccountType::Expense)
        {
            continue;
        }
        let f = function_of(r.tags.class.as_deref());
        let i = FUNCTIONS.iter().position(|x| x.0 == f).unwrap_or(3);
        let e = by.entry(r.account).or_default();
        if let Some(slot) = e.get_mut(i) {
            *slot = plus(*slot, r.value);
        }
    }
    let mut totals = [Decimal::ZERO; 4];
    let mut rows: Vec<Value> = Vec::new();
    for (id, v) in &by {
        for (t, x) in totals.iter_mut().zip(v) {
            *t = plus(*t, *x);
        }
        rows.push(json!({"account": id, "name": chart.full_name(*id),
            "amounts": v.iter().map(|x| money(*x)).collect::<Vec<_>>(), "total": money(sum(v))}));
    }
    rows.sort_by_key(|v| v["name"].as_str().unwrap_or_default().to_lowercase());
    Ok(json!({
        "from": from, "to": to,
        "columns": FUNCTIONS.iter().map(|f| f.1).collect::<Vec<_>>(),
        "rows": rows, "totals": totals.iter().map(|x| money(*x)).collect::<Vec<_>>(),
        "total": money(sum(&totals)),
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
struct FundActivityArgs {
    #[serde(flatten)]
    dates: Dates,
    /// Funds to show (ids or names; default all).
    #[serde(default)]
    funds: Option<Vec<String>>,
}

#[derive(Default, Clone, Copy)]
struct Movement {
    revenue: Decimal,
    expenses: Decimal,
    released_in: Decimal,
    released_out: Decimal,
    transfers_in: Decimal,
    transfers_out: Decimal,
    other: Decimal,
    not_released: Decimal,
}

fn fund_activity(ctx: &Ctx<'_>, a: FundActivityArgs) -> CommandResult<Value> {
    let (from, to) = crate::reports::resolve(ctx, &a.dates)?;
    let chart = Chart::load(ctx)?;
    let mut funds = records(ctx, "fund")?;
    if let Some(want) = &a.funds {
        funds.retain(|f| {
            want.iter()
                .any(|w| w == &f.id || w.eq_ignore_ascii_case(&f.name))
        });
    }
    let released: BTreeSet<String> = ctx
        .tx
        .docs(DocTable::TxnLink, Some("release"))?
        .into_iter()
        .filter_map(|d| {
            d.json
                .get("expense")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    let mut by: BTreeMap<String, Movement> = BTreeMap::new();
    for r in splits_in(ctx, Some(from), to)? {
        let Some(acc) = chart.get(r.account) else {
            continue;
        };
        let f = fund_of(&r.tags).to_owned();
        let restricted = records_restricted(&funds, &f);
        let m = by.entry(f).or_default();
        match acc.account_type {
            AccountType::Income if top_named(&chart, r.account, RELEASE_ACCOUNT) => {
                if r.value < Decimal::ZERO {
                    m.released_in = minus(m.released_in, r.value);
                } else {
                    m.released_out = plus(m.released_out, r.value);
                }
            }
            AccountType::Income => m.revenue = minus(m.revenue, r.value),
            AccountType::Expense => {
                m.expenses = plus(m.expenses, r.value);
                if restricted && !released.contains(&r.txn_id.to_string()) {
                    m.not_released = plus(m.not_released, r.value);
                }
            }
            AccountType::Equity if top_named(&chart, r.account, TRANSFER_ACCOUNT) => {
                if r.value < Decimal::ZERO {
                    m.transfers_in = minus(m.transfers_in, r.value);
                } else {
                    m.transfers_out = plus(m.transfers_out, r.value);
                }
            }
            AccountType::Equity => m.other = minus(m.other, r.value),
            _ => {}
        }
    }
    let begin = net_assets(ctx, day_before(from))?;
    let end = net_assets(ctx, to)?;
    let rows: Vec<Value> = funds
        .iter()
        .map(|f| {
            let m = by.get(&f.id).copied().unwrap_or_default();
            let ending = end.get(&f.id).copied().unwrap_or_default();
            let overspent = f.restricted() && ending < Decimal::ZERO;
            json!({
                "fund": f.id, "name": f.name, "restriction": f.restriction,
                "beginning": money(begin.get(&f.id).copied().unwrap_or_default()),
                "revenue": money(m.revenue), "expenses_tagged": money(m.expenses),
                "released_in": money(m.released_in), "released_out": money(m.released_out),
                "transfers_in": money(m.transfers_in), "transfers_out": money(m.transfers_out),
                "other_changes": money(m.other),
                "spent_not_released": f.restricted().then(|| money(m.not_released)),
                "ending": money(ending), "overspent": overspent,
                "overspent_by": overspent.then(|| money(-ending)),
            })
        })
        .collect();
    Ok(json!({"from": from, "to": to, "funds": rows}))
}

fn records_restricted(funds: &[DimRecord], id: &str) -> bool {
    funds.iter().any(|f| f.id == id && f.restricted()) && id != GENERAL
}

// ----- CSV ----------------------------------------------------------------------------

/// CSV for the reports in this module.
pub(crate) fn export(
    ctx: &Ctx<'_>,
    name: &str,
    args: Value,
) -> CommandResult<Option<(Value, String)>> {
    let needs_funds = matches!(
        name,
        "statement_of_activities"
            | "statement_of_financial_position"
            | "functional_expenses"
            | "fund_activity"
            | "balance_sheet_by_fund"
    );
    if name != "profit_loss" && !needs_funds {
        return Ok(None);
    }
    if needs_funds && !crate::sales::module_on(ctx, FUNDS)? {
        return Err(CommandError::ModuleDisabled {
            command: format!("report.{name}"),
            module: FUNDS.into(),
        });
    }
    Ok(Some(match name {
        "profit_loss" => {
            let d = profit_loss(ctx, crate::parse_args(args)?)?;
            let labels: Vec<String> = list(at(&d, "columns"))
                .iter()
                .map(|c| s(at(c, "label")))
                .collect();
            let mut header = vec!["Account".to_owned()];
            header.extend(labels);
            header.push("Total".into());
            let mut c = Csv::new(&header);
            for sec in list(at(&d, "sections")) {
                c.heading(&s(at(&sec, "name")));
                for r in list(at(&sec, "rows")) {
                    let mut row = vec![s(at(&r, "name"))];
                    row.extend(list(at(&r, "amounts")).iter().map(s));
                    row.push(s(at(&r, "total")));
                    c.row(&row);
                }
                let mut row = vec![format!("Total {}", s(at(&sec, "name")).to_lowercase())];
                row.extend(list(at(&sec, "totals")).iter().map(s));
                row.push(s(at(&sec, "total")));
                c.row(&row);
            }
            let ni = at(&d, "net_income");
            let mut row = vec!["Net income".to_owned()];
            row.extend(list(at(ni, "amounts")).iter().map(s));
            row.push(s(at(ni, "total")));
            c.row(&row);
            (d, c.out)
        }
        "statement_of_activities" => {
            let d = activities(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&[
                "Line",
                "Without donor restrictions",
                "With donor restrictions",
                "Total",
            ]));
            let three = |c: &mut Csv, label: String, v: &Value| {
                let mut row = vec![label];
                row.extend(list(v).iter().map(s));
                c.row(&row);
            };
            c.heading("Revenue and support");
            for r in list(at(&d, "revenue")) {
                three(&mut c, s(at(&r, "name")), at(&r, "amounts"));
            }
            three(
                &mut c,
                "Net assets released from restrictions".into(),
                at(&d, "released"),
            );
            c.heading("Expenses");
            for r in list(at(&d, "expenses")) {
                three(&mut c, s(at(&r, "name")), at(&r, "amounts"));
            }
            three(
                &mut c,
                "Change in net assets".into(),
                at(&d, "change_in_net_assets"),
            );
            three(
                &mut c,
                "Net assets at beginning".into(),
                at(&d, "net_assets_beginning"),
            );
            three(&mut c, "Net assets at end".into(), at(&d, "net_assets_end"));
            (d, c.out)
        }
        "statement_of_financial_position" => {
            let d = financial_position(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&["Section", "Line", "Amount"]));
            for r in list(at(&d, "assets")) {
                c.row(&["Assets".into(), s(at(&r, "name")), s(at(&r, "balance"))]);
            }
            c.row(&[
                "Assets".into(),
                "Total assets".into(),
                s(at(&d, "total_assets")),
            ]);
            for r in list(at(&d, "liabilities")) {
                c.row(&[
                    "Liabilities".into(),
                    s(at(&r, "name")),
                    s(at(&r, "balance")),
                ]);
            }
            for r in list(at(&d, "without_donor_restrictions")) {
                c.row(&[
                    "Without donor restrictions".into(),
                    s(at(&r, "name")),
                    s(at(&r, "net_assets")),
                ]);
            }
            for r in list(at(&d, "with_donor_restrictions")) {
                c.row(&[
                    "With donor restrictions".into(),
                    s(at(&r, "name")),
                    s(at(&r, "net_assets")),
                ]);
            }
            c.row(&[
                "Net assets".into(),
                "Total net assets".into(),
                s(at(&d, "total_net_assets")),
            ]);
            (d, c.out)
        }
        "functional_expenses" => {
            let d = functional(ctx, crate::parse_args(args)?)?;
            let mut header = vec!["Expense".to_owned()];
            header.extend(list(at(&d, "columns")).iter().map(s));
            header.push("Total".into());
            let mut c = Csv::new(&header);
            for r in list(at(&d, "rows")) {
                let mut row = vec![s(at(&r, "name"))];
                row.extend(list(at(&r, "amounts")).iter().map(s));
                row.push(s(at(&r, "total")));
                c.row(&row);
            }
            let mut row = vec!["Total".to_owned()];
            row.extend(list(at(&d, "totals")).iter().map(s));
            row.push(s(at(&d, "total")));
            c.row(&row);
            (d, c.out)
        }
        "fund_activity" => {
            let d = fund_activity(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(&h(&[
                "Fund",
                "Beginning",
                "Revenue",
                "Expenses tagged",
                "Released in",
                "Released out",
                "Transfers in",
                "Transfers out",
                "Spent not released",
                "Ending",
                "Overspent",
            ]));
            for r in list(at(&d, "funds")) {
                c.row(&[
                    s(at(&r, "name")),
                    s(at(&r, "beginning")),
                    s(at(&r, "revenue")),
                    s(at(&r, "expenses_tagged")),
                    s(at(&r, "released_in")),
                    s(at(&r, "released_out")),
                    s(at(&r, "transfers_in")),
                    s(at(&r, "transfers_out")),
                    s(at(&r, "spent_not_released")),
                    s(at(&r, "ending")),
                    s(at(&r, "overspent_by")),
                ]);
            }
            (d, c.out)
        }
        _ => {
            let d = balance_by_fund(ctx, crate::parse_args(args)?)?;
            let mut header = vec!["Account".to_owned()];
            header.extend(list(at(&d, "columns")).iter().map(|c| s(at(c, "label"))));
            header.push("Total".into());
            let mut c = Csv::new(&header);
            for key in ["assets", "liabilities"] {
                for r in list(at(&d, key)) {
                    let mut row = vec![s(at(&r, "name"))];
                    row.extend(list(at(&r, "amounts")).iter().map(s));
                    row.push(s(at(&r, "total")));
                    c.row(&row);
                }
            }
            let mut row = vec!["Net assets".to_owned()];
            row.extend(list(at(&d, "net_assets")).iter().map(s));
            row.push(s(at(&d, "total_net_assets")));
            c.row(&row);
            (d, c.out)
        }
    }))
}
