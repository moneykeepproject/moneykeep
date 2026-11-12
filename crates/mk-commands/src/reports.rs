//! Personal report commands (spec: reports.md). All are read-only and recomputed
//! each time; amounts are exact decimal strings at the base currency's scale.

use std::collections::{BTreeMap, HashMap, HashSet};

use mk_core::currency::{format_amount, minor_units};
use mk_core::{
    Account, AccountId, AccountSubtype, AccountType, Date, Decimal, RoundingStrategy, Transaction,
};
use mk_import::normalise_payee;
use mk_store::{TxnFilter, today_utc};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::accounts::Chart;
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "report.spending",
        "Where money went: spending by category for the selected money accounts",
        false,
        spending,
    );
    r.add(
        "report.income_expense",
        "Income and expenses by category per period, with net income",
        false,
        income_expense,
    );
    r.add(
        "report.net_worth",
        "Asset and liability balances and net worth at each period end",
        false,
        net_worth,
    );
    r.add(
        "report.cash_flow",
        "Money into and out of the spending accounts, grouped by the other side",
        false,
        cash_flow,
    );
    r.add(
        "report.payees",
        "Who we paid: totals spent and received by payee",
        false,
        payees,
    );
    r.add(
        "report.export",
        "Export a report as RFC 4180 CSV (returns the file name and content)",
        false,
        export,
    );
}

// ----- common arguments ------------------------------------------------------------

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Preset {
    ThisMonth,
    LastMonth,
    ThisQuarter,
    ThisYear,
    LastYear,
    #[serde(rename = "last_12_months")]
    Last12Months,
    YearToDate,
    All,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Interval {
    #[default]
    Month,
    Quarter,
    Year,
}

/// Date selection shared by every report.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub(crate) struct Dates {
    /// First date (inclusive).
    #[serde(default)]
    from: Option<Date>,
    /// Last date (inclusive).
    #[serde(default)]
    to: Option<Date>,
    /// Instead of from/to.
    #[serde(default)]
    preset: Option<Preset>,
    /// The current date for presets (default: the local date of the machine).
    #[serde(default)]
    today: Option<Date>,
}

impl Dates {
    /// Use `preset last_year` when no dates were given (report.tax_summary).
    pub(crate) fn or_last_year(mut self) -> Self {
        if self.from.is_none() && self.to.is_none() && self.preset.is_none() {
            self.preset = Some(Preset::LastYear);
        }
        self
    }
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

fn ymd(y: i32, m: u8, d: u8) -> CommandResult<Date> {
    Ok(Date::from_ymd_clamped(y, m, d)?)
}

fn month_start(y: i32, m: u8) -> CommandResult<Date> {
    ymd(y, m, 1)
}

fn month_end(y: i32, m: u8) -> CommandResult<Date> {
    ymd(y, m, 31)
}

fn shift_month(y: i32, m: u8, n: i64) -> CommandResult<(i32, u8)> {
    Date::shift_month(y, m, n).ok_or_else(|| invalid("date out of range"))
}

/// Resolve dates against `today`; returns (from, to).
pub(crate) fn resolve(ctx: &Ctx<'_>, d: &Dates) -> CommandResult<(Date, Date)> {
    let today = match d.today {
        Some(t) => t,
        None => today_utc()?,
    };
    let (y, m) = (today.year(), today.month());
    let range = match d.preset {
        Some(Preset::ThisMonth) => (month_start(y, m)?, month_end(y, m)?),
        Some(Preset::LastMonth) => {
            let (py, pm) = shift_month(y, m, -1)?;
            (month_start(py, pm)?, month_end(py, pm)?)
        }
        Some(Preset::ThisQuarter) => {
            let q0 = (m - 1) / 3 * 3 + 1;
            (month_start(y, q0)?, month_end(y, q0 + 2)?)
        }
        Some(Preset::ThisYear) => (ymd(y, 1, 1)?, ymd(y, 12, 31)?),
        Some(Preset::LastYear) => (ymd(y - 1, 1, 1)?, ymd(y - 1, 12, 31)?),
        Some(Preset::Last12Months) => {
            let (fy, fm) = shift_month(y, m, -12)?;
            let (ty, tm) = shift_month(y, m, -1)?;
            (month_start(fy, fm)?, month_end(ty, tm)?)
        }
        Some(Preset::YearToDate) => (ymd(y, 1, 1)?, today),
        Some(Preset::All) => {
            let (first, last) = activity_span(ctx)?;
            (first.unwrap_or(today), last.map_or(today, |l| l.max(today)))
        }
        None => {
            let to = d.to.unwrap_or(today);
            let from = match d.from {
                Some(f) => f,
                None => activity_span(ctx)?.0.unwrap_or(to).min(to),
            };
            (from, to)
        }
    };
    if range.0 > range.1 {
        return Err(invalid("from is after to"));
    }
    Ok(range)
}

fn activity_span(ctx: &Ctx<'_>) -> CommandResult<(Option<Date>, Option<Date>)> {
    let rows = ctx.tx.posted_splits(None)?;
    Ok((
        rows.first().map(|r| r.date),
        rows.iter().map(|r| r.date).max(),
    ))
}

/// One reporting period.
#[derive(Debug, Clone)]
pub(crate) struct Period {
    pub from: Date,
    pub to: Date,
    pub label: String,
}

pub(crate) fn periods(from: Date, to: Date, interval: Interval) -> CommandResult<Vec<Period>> {
    let mut out = Vec::new();
    let mut start = from;
    while start <= to {
        let (y, m) = (start.year(), start.month());
        let (end, label) = match interval {
            Interval::Month => (month_end(y, m)?, format!("{y:04}-{m:02}")),
            Interval::Quarter => {
                let q = (m - 1) / 3 + 1;
                (month_end(y, q * 3)?, format!("{y:04}-Q{q}"))
            }
            Interval::Year => (ymd(y, 12, 31)?, format!("{y:04}")),
        };
        let end = end.min(to);
        out.push(Period {
            from: start,
            to: end,
            label,
        });
        match end.next_day() {
            Some(n) => start = n,
            None => break,
        }
        if out.len() > 10_000 {
            return Err(invalid("too many periods"));
        }
    }
    Ok(out)
}

pub(crate) fn period_json(p: &Period) -> Value {
    json!({"from": p.from, "to": p.to, "label": p.label})
}

// ----- shared helpers --------------------------------------------------------------

struct Ctx2<'a> {
    chart: Chart,
    base: String,
    dp: u32,
    depth: HashMap<AccountId, usize>,
    order: Vec<AccountId>,
    _p: std::marker::PhantomData<&'a ()>,
}

impl Ctx2<'_> {
    fn load(ctx: &Ctx<'_>) -> CommandResult<Self> {
        let chart = Chart::load(ctx)?;
        let base = ctx.tx.info()?.base_currency;
        let dp = minor_units(&base);
        let tree = chart.tree_order();
        let depth = tree.iter().map(|(a, d)| (a.id, *d)).collect();
        let order = tree.iter().map(|(a, _)| a.id).collect();
        Ok(Self {
            chart,
            base,
            dp,
            depth,
            order,
            _p: std::marker::PhantomData,
        })
    }

    fn money(&self, v: Decimal) -> String {
        format_amount(v, Some(self.dp))
    }

    fn acc(&self, id: AccountId) -> Option<&Account> {
        self.chart.get(id)
    }

    fn in_base(&self, id: AccountId) -> bool {
        self.acc(id)
            .is_some_and(|a| a.commodity.as_str() == self.base)
    }

    /// The account a split counts toward with `depth` levels kept (1 = top level).
    fn rollup(&self, id: AccountId, depth: Option<usize>) -> AccountId {
        let Some(keep) = depth else { return id };
        let mut cur = id;
        let mut steps = 0;
        while self.depth.get(&cur).copied().unwrap_or(0) + 1 > keep.max(1) {
            match self.acc(cur).and_then(|a| a.parent) {
                Some(p) => cur = p,
                None => break,
            }
            steps += 1;
            if steps > self.order.len() {
                break;
            }
        }
        cur
    }

    fn position(&self, id: AccountId) -> usize {
        self.order
            .iter()
            .position(|x| *x == id)
            .unwrap_or(usize::MAX)
    }

    fn account_json(&self, id: AccountId) -> Value {
        json!({
            "id": id,
            "full_name": self.chart.full_name(id),
            "depth": self.depth.get(&id).copied().unwrap_or(0),
        })
    }

    /// Resolve an account filter (each with its descendants); `None` → `default`.
    fn accounts(
        &self,
        refs: Option<&Vec<String>>,
        default: impl Fn(&Account) -> bool,
    ) -> CommandResult<HashSet<AccountId>> {
        Ok(match refs {
            Some(list) => {
                let mut set = HashSet::new();
                for r in list {
                    set.extend(self.chart.subtree(self.chart.resolve(r)?.id));
                }
                set
            }
            None => self
                .chart
                .accounts
                .iter()
                .filter(|a| default(a))
                .map(|a| a.id)
                .collect(),
        })
    }

    fn excluded(&self, ids: impl IntoIterator<Item = AccountId>) -> Vec<Value> {
        let mut seen: Vec<AccountId> = ids.into_iter().filter(|id| !self.in_base(*id)).collect();
        seen.sort_by_key(|id| self.position(*id));
        seen.dedup();
        seen.into_iter()
            .map(|id| json!({"account": id, "full_name": self.chart.full_name(id), "reason": "not in base currency"}))
            .collect()
    }

    fn is_type(&self, id: AccountId, types: &[AccountType]) -> bool {
        self.acc(id)
            .is_some_and(|a| types.contains(&a.account_type))
    }

    fn display(&self, id: AccountId, raw: Decimal) -> Decimal {
        self.acc(id).map_or(raw, |a| a.account_type.display(raw))
    }
}

fn sum(values: impl IntoIterator<Item = Decimal>) -> Decimal {
    values
        .into_iter()
        .fold(Decimal::ZERO, |t, v| t.checked_add(v).unwrap_or(t))
}

fn add(slot: &mut Decimal, v: Decimal) {
    *slot = slot.checked_add(v).unwrap_or(*slot);
}

fn percent(part: Decimal, of: Decimal) -> Option<String> {
    if of.is_zero() {
        return None;
    }
    part.checked_mul(Decimal::ONE_HUNDRED)
        .and_then(|x| x.checked_div(of))
        .map(|x| {
            let r = x.round_dp_with_strategy(1, RoundingStrategy::MidpointAwayFromZero);
            format_amount(r, Some(1))
        })
}

fn posted_txns(ctx: &Ctx<'_>, from: Date, to: Date) -> CommandResult<Vec<Transaction>> {
    Ok(ctx.tx.transactions(&TxnFilter {
        accounts: None,
        from: Some(from),
        to: Some(to),
        include_voided: false,
        limit: None,
        offset: 0,
    })?)
}

fn is_money(a: &Account) -> bool {
    matches!(a.account_type, AccountType::Asset | AccountType::Liability)
}

/// Raw base-currency balance of each account as of a date (posted, basis all).
fn balances_as_of(
    ctx: &Ctx<'_>,
    r: &Ctx2<'_>,
    as_of: Date,
) -> CommandResult<HashMap<AccountId, Decimal>> {
    let mut out: HashMap<AccountId, Decimal> = HashMap::new();
    for row in ctx.tx.posted_splits(Some(as_of))? {
        if r.in_base(row.account) {
            add(out.entry(row.account).or_default(), row.quantity);
        }
    }
    Ok(out)
}

// ----- report.spending -------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct SpendingArgs {
    #[serde(flatten)]
    dates: Dates,
    /// Money accounts to look at (default: every asset and liability account).
    #[serde(default)]
    accounts: Option<Vec<String>>,
    /// Keep this many levels of each account name (default: all).
    #[serde(default)]
    depth: Option<usize>,
    #[serde(default)]
    include_income: bool,
}

fn spending_data(ctx: &Ctx<'_>, a: &SpendingArgs) -> CommandResult<Value> {
    let (from, to) = resolve(ctx, &a.dates)?;
    let r = Ctx2::load(ctx)?;
    let selected = r.accounts(a.accounts.as_ref(), is_money)?;
    let cats: Vec<AccountType> = if a.include_income {
        vec![AccountType::Expense, AccountType::Income]
    } else {
        vec![AccountType::Expense]
    };
    let mut totals: HashMap<AccountId, Decimal> = HashMap::new();
    let mut touched = Vec::new();
    for t in posted_txns(ctx, from, to)? {
        if !t.splits.iter().any(|s| selected.contains(&s.account)) {
            continue;
        }
        touched.extend(t.splits.iter().map(|s| s.account));
        // The share of the money side paid through the selected accounts.
        let money: Vec<_> = t
            .splits
            .iter()
            .filter(|s| r.acc(s.account).is_some_and(is_money) && r.in_base(s.account))
            .collect();
        let all = sum(money.iter().map(|s| s.quantity.abs()));
        let mine = sum(money
            .iter()
            .filter(|s| selected.contains(&s.account))
            .map(|s| s.quantity.abs()));
        if all.is_zero() {
            continue;
        }
        for s in t
            .splits
            .iter()
            .filter(|s| r.is_type(s.account, &cats) && r.in_base(s.account))
        {
            let shown = r.display(s.account, s.quantity);
            let v = if mine == all {
                shown
            } else {
                shown
                    .checked_mul(mine)
                    .and_then(|x| x.checked_div(all))
                    .map(|x| x.round_dp_with_strategy(r.dp, RoundingStrategy::MidpointAwayFromZero))
                    .unwrap_or_default()
            };
            add(totals.entry(r.rollup(s.account, a.depth)).or_default(), v);
        }
    }
    let total = sum(totals.values().copied());
    let mut rows: Vec<(AccountId, Decimal)> =
        totals.into_iter().filter(|(_, v)| !v.is_zero()).collect();
    rows.sort_by(|x, y| {
        y.1.cmp(&x.1)
            .then_with(|| r.position(x.0).cmp(&r.position(y.0)))
    });
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|(id, v)| {
            let mut o = r.account_json(id);
            if let Some(m) = o.as_object_mut() {
                m.insert("amount".into(), json!(r.money(v)));
                m.insert("percent".into(), json!(percent(v, total)));
                m.insert("type".into(), json!(r.acc(id).map(|x| x.account_type)));
            }
            o
        })
        .collect();
    Ok(json!({
        "report": "spending",
        "from": from,
        "to": to,
        "rows": rows,
        "total": r.money(total),
        "excluded": r.excluded(touched),
    }))
}

fn spending(ctx: &Ctx<'_>, a: SpendingArgs) -> CommandResult<Value> {
    spending_data(ctx, &a)
}

// ----- report.income_expense -------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct IncomeExpenseArgs {
    #[serde(flatten)]
    dates: Dates,
    #[serde(default)]
    interval: Interval,
    #[serde(default)]
    depth: Option<usize>,
}

fn income_expense(ctx: &Ctx<'_>, a: IncomeExpenseArgs) -> CommandResult<Value> {
    let (from, to) = resolve(ctx, &a.dates)?;
    let r = Ctx2::load(ctx)?;
    let ps = periods(from, to, a.interval)?;
    let n = ps.len();
    let mut amounts: HashMap<AccountId, Vec<Decimal>> = HashMap::new();
    let mut touched = Vec::new();
    for t in posted_txns(ctx, from, to)? {
        let Some(pi) = ps.iter().position(|p| t.date >= p.from && t.date <= p.to) else {
            continue;
        };
        for s in &t.splits {
            if !r.is_type(s.account, &[AccountType::Income, AccountType::Expense]) {
                continue;
            }
            touched.push(s.account);
            if !r.in_base(s.account) {
                continue;
            }
            let key = r.rollup(s.account, a.depth);
            let row = amounts.entry(key).or_insert_with(|| vec![Decimal::ZERO; n]);
            if let Some(slot) = row.get_mut(pi) {
                add(slot, r.display(s.account, s.quantity));
            }
        }
    }
    let section = |ty: AccountType| {
        let mut ids: Vec<AccountId> = amounts
            .keys()
            .copied()
            .filter(|id| r.is_type(*id, &[ty]))
            .collect();
        ids.sort_by_key(|id| r.position(*id));
        let mut per = vec![Decimal::ZERO; n];
        let rows: Vec<Value> = ids
            .iter()
            .map(|id| {
                let vals = amounts.get(id).cloned().unwrap_or_default();
                for (slot, v) in per.iter_mut().zip(&vals) {
                    add(slot, *v);
                }
                let mut o = r.account_json(*id);
                if let Some(m) = o.as_object_mut() {
                    m.insert(
                        "amounts".into(),
                        json!(vals.iter().map(|v| r.money(*v)).collect::<Vec<_>>()),
                    );
                    m.insert("total".into(), json!(r.money(sum(vals.iter().copied()))));
                }
                o
            })
            .collect();
        (rows, per)
    };
    let (inc_rows, inc) = section(AccountType::Income);
    let (exp_rows, exp) = section(AccountType::Expense);
    let net: Vec<Decimal> = inc
        .iter()
        .zip(&exp)
        .map(|(i, e)| i.checked_sub(*e).unwrap_or(*i))
        .collect();
    let fmt_list = |v: &[Decimal]| v.iter().map(|x| r.money(*x)).collect::<Vec<_>>();
    Ok(json!({
        "report": "income_expense",
        "from": from,
        "to": to,
        "interval": a.interval_name(),
        "periods": ps.iter().map(period_json).collect::<Vec<_>>(),
        "income": {"rows": inc_rows, "totals": fmt_list(&inc), "total": r.money(sum(inc.iter().copied()))},
        "expenses": {"rows": exp_rows, "totals": fmt_list(&exp), "total": r.money(sum(exp.iter().copied()))},
        "net": {"amounts": fmt_list(&net), "total": r.money(sum(net.iter().copied()))},
        "excluded": r.excluded(touched),
    }))
}

impl IncomeExpenseArgs {
    fn interval_name(&self) -> &'static str {
        interval_name(self.interval)
    }
}

pub(crate) fn interval_name(i: Interval) -> &'static str {
    match i {
        Interval::Month => "month",
        Interval::Quarter => "quarter",
        Interval::Year => "year",
    }
}

// ----- report.net_worth ------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct NetWorthArgs {
    #[serde(flatten)]
    dates: Dates,
    #[serde(default)]
    interval: Interval,
    /// Levels of account names kept (default 1: top-level accounts).
    #[serde(default)]
    depth: Option<usize>,
}

fn net_worth(ctx: &Ctx<'_>, a: NetWorthArgs) -> CommandResult<Value> {
    let (from, to) = resolve(ctx, &a.dates)?;
    let r = Ctx2::load(ctx)?;
    let today = match a.dates.today {
        Some(t) => t,
        None => today_utc()?,
    };
    let ps = future_trimmed(periods(from, to, a.interval)?, from, to, today);
    let depth = Some(a.depth.unwrap_or(1));
    let n = ps.len();
    let mut rows_by: HashMap<AccountId, Vec<Decimal>> = HashMap::new();
    // Security accounts count at market value (the latest price on or before each
    // date), or at cost basis when they have no price (investments-core.md).
    let mut unpriced: HashSet<AccountId> = HashSet::new();
    for (pi, p) in ps.iter().enumerate() {
        let mut bal = balances_as_of(ctx, &r, p.to)?;
        for (id, v) in crate::investments::valuations(ctx, &r.chart, p.to)? {
            if !v.valued() && !(v.quantity.is_zero() && v.basis.is_zero()) {
                unpriced.insert(id);
            }
            bal.insert(id, v.value);
        }
        for (id, raw) in bal {
            if !r.acc(id).is_some_and(is_money) {
                continue;
            }
            let key = r.rollup(id, depth);
            let row = rows_by.entry(key).or_insert_with(|| vec![Decimal::ZERO; n]);
            if let Some(slot) = row.get_mut(pi) {
                add(slot, r.display(id, raw));
            }
        }
    }
    let section = |ty: AccountType| {
        let mut ids: Vec<AccountId> = rows_by
            .keys()
            .copied()
            .filter(|id| r.is_type(*id, &[ty]))
            .collect();
        ids.sort_by_key(|id| r.position(*id));
        let mut per = vec![Decimal::ZERO; n];
        let rows: Vec<Value> = ids
            .iter()
            .map(|id| {
                let vals = rows_by.get(id).cloned().unwrap_or_default();
                for (slot, v) in per.iter_mut().zip(&vals) {
                    add(slot, *v);
                }
                let mut o = r.account_json(*id);
                if let Some(m) = o.as_object_mut() {
                    m.insert(
                        "amounts".into(),
                        json!(vals.iter().map(|v| r.money(*v)).collect::<Vec<_>>()),
                    );
                }
                o
            })
            .collect();
        (rows, per)
    };
    let (asset_rows, assets) = section(AccountType::Asset);
    let (liab_rows, liabilities) = section(AccountType::Liability);
    let nw: Vec<Decimal> = assets
        .iter()
        .zip(&liabilities)
        .map(|(x, l)| x.checked_sub(*l).unwrap_or(*x))
        .collect();
    let change = match (nw.first(), nw.last()) {
        (Some(f), Some(l)) => l.checked_sub(*f).unwrap_or_default(),
        _ => Decimal::ZERO,
    };
    let fmt_list = |v: &[Decimal]| v.iter().map(|x| r.money(*x)).collect::<Vec<_>>();
    let foreign = r
        .chart
        .accounts
        .iter()
        .filter(|x| is_money(x) && x.subtype != AccountSubtype::Security)
        .map(|x| x.id);
    let mut excluded = r.excluded(foreign);
    let mut no_price: Vec<AccountId> = unpriced.into_iter().collect();
    no_price.sort_by_key(|id| r.position(*id));
    for id in no_price {
        excluded.push(json!({"account": id, "full_name": r.chart.full_name(id), "reason": "no price; counted at cost basis"}));
    }
    Ok(json!({
        "report": "net_worth",
        "from": from,
        "to": to,
        "interval": interval_name(a.interval),
        "periods": ps.iter().map(period_json).collect::<Vec<_>>(),
        "dates": ps.iter().map(|p| p.to).collect::<Vec<_>>(),
        "assets": {"rows": asset_rows, "totals": fmt_list(&assets)},
        "liabilities": {"rows": liab_rows, "totals": fmt_list(&liabilities)},
        "net_worth": fmt_list(&nw),
        "change": r.money(change),
        "excluded": excluded,
    }))
}

/// Net worth over a range reaching past today: period ends after today are dropped
/// and today becomes the last date (labelled with the date).
fn future_trimmed(mut ps: Vec<Period>, from: Date, to: Date, today: Date) -> Vec<Period> {
    if to <= today || today < from {
        return ps;
    }
    ps.retain(|p| p.to <= today);
    let start = ps.last().and_then(|p| p.to.next_day()).unwrap_or(from);
    if ps.last().is_none_or(|p| p.to < today) {
        ps.push(Period {
            from: start,
            to: today,
            label: today.to_string(),
        });
    }
    ps
}

// ----- report.cash_flow ------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct CashFlowArgs {
    #[serde(flatten)]
    dates: Dates,
    /// Default: bank and cash accounts and credit cards.
    #[serde(default)]
    accounts: Option<Vec<String>>,
    /// Optional: also return each period's figures.
    #[serde(default)]
    interval: Option<Interval>,
}

fn default_cash(a: &Account) -> bool {
    matches!(
        (a.account_type, a.subtype),
        (
            AccountType::Asset,
            AccountSubtype::Bank | AccountSubtype::Cash
        ) | (AccountType::Liability, AccountSubtype::CreditCard)
    )
}

fn cash_flow_period(
    ctx: &Ctx<'_>,
    r: &Ctx2<'_>,
    selected: &HashSet<AccountId>,
    from: Date,
    to: Date,
) -> CommandResult<Value> {
    let position = |as_of: Option<Date>| -> CommandResult<Decimal> {
        Ok(match as_of {
            Some(d) => sum(balances_as_of(ctx, r, d)?
                .into_iter()
                .filter(|(id, _)| selected.contains(id))
                .map(|(_, v)| v)),
            None => Decimal::ZERO,
        })
    };
    let opening = position(from.previous_day())?;
    let closing = position(Some(to))?;
    let mut inflows: BTreeMap<String, (usize, Decimal)> = BTreeMap::new();
    let mut outflows: BTreeMap<String, (usize, Decimal)> = BTreeMap::new();
    let mut internal = Decimal::ZERO;
    let two = Decimal::from(2);
    for t in posted_txns(ctx, from, to)? {
        let mine: Vec<_> = t
            .splits
            .iter()
            .filter(|s| selected.contains(&s.account) && r.in_base(s.account))
            .collect();
        if mine.is_empty() {
            continue;
        }
        let gross = sum(mine.iter().map(|s| s.quantity.abs()));
        let net = sum(mine.iter().map(|s| s.quantity)).abs();
        add(
            &mut internal,
            gross
                .checked_sub(net)
                .and_then(|x| x.checked_div(two))
                .unwrap_or_default(),
        );
        for s in t.splits.iter().filter(|s| !selected.contains(&s.account)) {
            let Some(acc) = r.acc(s.account) else {
                continue;
            };
            let name = r.chart.full_name(acc.id);
            let key = if matches!(acc.account_type, AccountType::Income | AccountType::Expense) {
                name
            } else {
                format!("Transfer: {name}")
            };
            let flow = -s.quantity;
            let (map, v) = if flow.is_sign_positive() {
                (&mut inflows, flow)
            } else {
                (&mut outflows, -flow)
            };
            if v.is_zero() {
                continue;
            }
            let e = map
                .entry(key)
                .or_insert((r.position(acc.id), Decimal::ZERO));
            add(&mut e.1, v);
        }
    }
    let rows = |m: &BTreeMap<String, (usize, Decimal)>| {
        let mut v: Vec<(&String, &(usize, Decimal))> = m.iter().collect();
        v.sort_by(|a, b| b.1.1.cmp(&a.1.1).then_with(|| a.1.0.cmp(&b.1.0)));
        v.into_iter()
            .map(|(k, (_, amt))| json!({"name": k, "amount": r.money(*amt)}))
            .collect::<Vec<_>>()
    };
    let total_in = sum(inflows.values().map(|x| x.1));
    let total_out = sum(outflows.values().map(|x| x.1));
    let difference = sum([opening, total_in, -total_out, -closing]);
    Ok(json!({
        "from": from,
        "to": to,
        "opening": r.money(opening),
        "inflows": {"rows": rows(&inflows), "total": r.money(total_in)},
        "outflows": {"rows": rows(&outflows), "total": r.money(total_out)},
        "internal": r.money(internal),
        "closing": r.money(closing),
        "difference": r.money(difference),
    }))
}

fn cash_flow(ctx: &Ctx<'_>, a: CashFlowArgs) -> CommandResult<Value> {
    let (from, to) = resolve(ctx, &a.dates)?;
    let r = Ctx2::load(ctx)?;
    let selected = r.accounts(a.accounts.as_ref(), default_cash)?;
    let mut v = cash_flow_period(ctx, &r, &selected, from, to)?;
    let periods_json = match a.interval {
        Some(i) => periods(from, to, i)?
            .iter()
            .map(|p| {
                let mut x = cash_flow_period(ctx, &r, &selected, p.from, p.to)?;
                if let Some(o) = x.as_object_mut() {
                    o.insert("label".into(), json!(p.label));
                }
                Ok(x)
            })
            .collect::<CommandResult<Vec<_>>>()?,
        None => Vec::new(),
    };
    let mut accounts: Vec<AccountId> = selected.iter().copied().collect();
    accounts.sort_by_key(|id| r.position(*id));
    if let Some(o) = v.as_object_mut() {
        o.insert("report".into(), json!("cash_flow"));
        o.insert("periods".into(), json!(periods_json));
        o.insert(
            "accounts".into(),
            json!(
                accounts
                    .iter()
                    .map(|id| r.chart.full_name(*id))
                    .collect::<Vec<_>>()
            ),
        );
        o.insert(
            "excluded".into(),
            json!(r.excluded(accounts.iter().copied())),
        );
    }
    Ok(v)
}

// ----- report.payees ---------------------------------------------------------------

fn default_limit() -> usize {
    50
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PayeesArgs {
    #[serde(flatten)]
    dates: Dates,
    /// Money accounts to look at (default: every asset and liability account).
    #[serde(default)]
    accounts: Option<Vec<String>>,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn payees(ctx: &Ctx<'_>, a: PayeesArgs) -> CommandResult<Value> {
    let (from, to) = resolve(ctx, &a.dates)?;
    let r = Ctx2::load(ctx)?;
    let selected = r.accounts(a.accounts.as_ref(), is_money)?;
    struct Acc {
        spellings: BTreeMap<String, usize>,
        count: usize,
        spent: Decimal,
        received: Decimal,
    }
    let mut by: HashMap<String, Acc> = HashMap::new();
    for t in posted_txns(ctx, from, to)? {
        let flow = sum(t
            .splits
            .iter()
            .filter(|s| selected.contains(&s.account) && r.in_base(s.account))
            .map(|s| s.quantity));
        if flow.is_zero() {
            continue;
        }
        let original = t.payee.clone().unwrap_or_default();
        let key = normalise_payee(&original);
        let e = by.entry(key).or_insert(Acc {
            spellings: BTreeMap::new(),
            count: 0,
            spent: Decimal::ZERO,
            received: Decimal::ZERO,
        });
        *e.spellings.entry(original).or_default() += 1;
        e.count += 1;
        if flow.is_sign_negative() {
            add(&mut e.spent, -flow);
        } else {
            add(&mut e.received, flow);
        }
    }
    let mut rows: Vec<(String, Acc)> = by.into_iter().collect();
    rows.sort_by(|x, y| y.1.spent.cmp(&x.1.spent).then_with(|| x.0.cmp(&y.0)));
    let rows: Vec<Value> = rows
        .into_iter()
        .take(a.limit)
        .map(|(key, acc)| {
            let display = acc
                .spellings
                .iter()
                .max_by(|x, y| x.1.cmp(y.1).then_with(|| y.0.cmp(x.0)))
                .map(|(s, _)| s.clone())
                .unwrap_or_default();
            let name = if display.is_empty() {
                "(no payee)".to_owned()
            } else {
                display
            };
            json!({
                "payee": name,
                "normalised": key,
                "count": acc.count,
                "spent": r.money(acc.spent),
                "received": r.money(acc.received),
            })
        })
        .collect();
    Ok(json!({"report": "payees", "from": from, "to": to, "rows": rows}))
}

// ----- report.export ---------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExportArgs {
    /// spending, income_expense, net_worth, cash_flow, payees, tax_summary, …
    report: String,
    /// The report's own arguments.
    #[serde(default)]
    args: Option<Value>,
    /// csv (every report) or pdf (tax_summary).
    #[serde(default = "csv_format")]
    format: String,
}

fn csv_format() -> String {
    "csv".into()
}

/// Whether a cell is a plain number or ISO date (written as is).
fn numeric_cell(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && (t.parse::<Decimal>().is_ok()
            || t.replace(',', "").parse::<Decimal>().is_ok()
            || t.parse::<mk_core::Date>().is_ok())
}

/// One CSV cell. A text cell starting with `=`, `+`, `-`, `@`, tab or carriage return
/// gets a leading apostrophe so spreadsheets show it as text (input-safety.md R6).
fn field(s: &str) -> String {
    let guarded;
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) && !numeric_cell(s) {
        guarded = format!("'{s}");
        guarded.as_str()
    } else {
        s
    };
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

pub(crate) struct Csv {
    pub out: String,
    width: usize,
}

impl Csv {
    pub(crate) fn new(header: &[String]) -> Self {
        let mut c = Self {
            out: String::new(),
            width: header.len(),
        };
        c.row(header);
        c
    }

    pub(crate) fn row(&mut self, cells: &[String]) {
        let mut cells: Vec<String> = cells.iter().map(|c| field(c)).collect();
        cells.resize(self.width.max(cells.len()), String::new());
        self.out.push_str(&cells.join(","));
        self.out.push_str("\r\n");
    }

    pub(crate) fn heading(&mut self, title: &str) {
        self.row(&[title.to_owned()]);
    }
}

fn at<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key).unwrap_or(&Value::Null)
}

fn s(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_default()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(s).collect())
        .unwrap_or_default()
}

fn rows_of(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn export(ctx: &Ctx<'_>, a: ExportArgs) -> CommandResult<Value> {
    let name = a.report.trim_start_matches("report.").to_owned();
    if name == "tax_summary" && a.format == "pdf" {
        return crate::chart::tax_summary::export_pdf(ctx, a.args.unwrap_or_else(|| json!({})));
    }
    if name == "project" && a.format == "pdf" {
        if !crate::is_module_enabled(ctx.tx, crate::projects::MODULE)? {
            return Err(CommandError::ModuleDisabled {
                command: "report.project".into(),
                module: crate::projects::MODULE.into(),
            });
        }
        return crate::projects::export_pdf(ctx, a.args.unwrap_or_else(|| json!({})));
    }
    if a.format != "csv" {
        return Err(invalid(format!("unknown format {:?}; use csv", a.format)));
    }
    let args = a.args.unwrap_or_else(|| json!({}));
    let other = match crate::inv_reports::export(ctx, &name, args.clone())? {
        Some(x) => Some(x),
        None => match crate::sales_reports::export(ctx, &name, args.clone())? {
            Some(x) => Some(x),
            None => match crate::purchase_reports::export(ctx, &name, args.clone())? {
                Some(x) => Some(x),
                None => match crate::dimension_reports::export(ctx, &name, args.clone())? {
                    Some(x) => Some(x),
                    None => crate::projects::export(ctx, &name, args.clone())?,
                },
            },
        },
    };
    if let Some((data, content)) = other {
        let file_name = if let Some(y) = at(&data, "year").as_i64() {
            format!("{}-{y}.csv", name.replace('_', "-"))
        } else if at(&data, "as_of").is_null() {
            format!(
                "{}-{}-to-{}.csv",
                name.replace('_', "-"),
                s(at(&data, "from")),
                s(at(&data, "to"))
            )
        } else {
            format!("{}-{}.csv", name.replace('_', "-"), s(at(&data, "as_of")))
        };
        let mut out = Map::new();
        out.insert("file_name".into(), json!(file_name));
        out.insert("content".into(), json!(content));
        return Ok(Value::Object(out));
    }
    let data = match name.as_str() {
        "spending" => spending(ctx, crate::parse_args(args)?)?,
        "income_expense" => income_expense(ctx, crate::parse_args(args)?)?,
        "net_worth" => net_worth(ctx, crate::parse_args(args)?)?,
        "cash_flow" => cash_flow(ctx, crate::parse_args(args)?)?,
        "payees" => payees(ctx, crate::parse_args(args)?)?,
        "tax_summary" => crate::chart::tax_summary::report_json(ctx, args.clone())?,
        other => return Err(invalid(format!("unknown report {other:?}"))),
    };
    let content = match name.as_str() {
        "spending" => {
            let mut c = Csv::new(&["Category".into(), "Amount".into(), "Percent".into()]);
            for r in rows_of(at(&data, "rows")) {
                c.row(&[
                    s(at(&r, "full_name")),
                    s(at(&r, "amount")),
                    s(at(&r, "percent")),
                ]);
            }
            c.row(&["Total".into(), s(at(&data, "total"))]);
            c.out
        }
        "income_expense" => {
            let labels: Vec<String> = rows_of(at(&data, "periods"))
                .iter()
                .map(|p| s(at(p, "label")))
                .collect();
            let mut header = vec!["Category".to_owned()];
            header.extend(labels.iter().cloned());
            header.push("Total".into());
            let mut c = Csv::new(&header);
            for (key, title) in [("income", "Income"), ("expenses", "Expenses")] {
                c.heading(title);
                for r in rows_of(at(at(&data, key), "rows")) {
                    let mut cells = vec![s(at(&r, "full_name"))];
                    cells.extend(strs(at(&r, "amounts")));
                    cells.push(s(at(&r, "total")));
                    c.row(&cells);
                }
                let mut cells = vec![format!("Total {}", title.to_lowercase())];
                cells.extend(strs(at(at(&data, key), "totals")));
                cells.push(s(at(at(&data, key), "total")));
                c.row(&cells);
            }
            let mut cells = vec!["Net".to_owned()];
            cells.extend(strs(at(at(&data, "net"), "amounts")));
            cells.push(s(at(at(&data, "net"), "total")));
            c.row(&cells);
            c.out
        }
        "net_worth" => {
            let mut header = vec!["Account".to_owned()];
            header.extend(strs(at(&data, "dates")));
            let mut c = Csv::new(&header);
            for (key, title) in [("assets", "Assets"), ("liabilities", "Liabilities")] {
                c.heading(title);
                for r in rows_of(at(at(&data, key), "rows")) {
                    let mut cells = vec![s(at(&r, "full_name"))];
                    cells.extend(strs(at(&r, "amounts")));
                    c.row(&cells);
                }
                let mut cells = vec![format!("Total {}", title.to_lowercase())];
                cells.extend(strs(at(at(&data, key), "totals")));
                c.row(&cells);
            }
            let mut cells = vec!["Net worth".to_owned()];
            cells.extend(strs(at(&data, "net_worth")));
            c.row(&cells);
            c.row(&["Change".into(), s(at(&data, "change"))]);
            c.out
        }
        "cash_flow" => {
            let mut c = Csv::new(&["Item".into(), "Amount".into()]);
            c.row(&["Opening".into(), s(at(&data, "opening"))]);
            for (key, title) in [("inflows", "Inflows"), ("outflows", "Outflows")] {
                c.heading(title);
                for r in rows_of(at(at(&data, key), "rows")) {
                    c.row(&[s(at(&r, "name")), s(at(&r, "amount"))]);
                }
                c.row(&[
                    format!("Total {}", title.to_lowercase()),
                    s(at(at(&data, key), "total")),
                ]);
            }
            c.row(&["Internal transfers".into(), s(at(&data, "internal"))]);
            c.row(&["Closing".into(), s(at(&data, "closing"))]);
            c.out
        }
        "tax_summary" => crate::chart::tax_summary::csv(&data),
        _ => {
            let mut c = Csv::new(&[
                "Payee".into(),
                "Transactions".into(),
                "Spent".into(),
                "Received".into(),
            ]);
            for r in rows_of(at(&data, "rows")) {
                c.row(&[
                    s(at(&r, "payee")),
                    at(&r, "count").to_string(),
                    s(at(&r, "spent")),
                    s(at(&r, "received")),
                ]);
            }
            c.out
        }
    };
    let file_name = format!(
        "{}-{}-to-{}.csv",
        name.replace('_', "-"),
        s(at(&data, "from")),
        s(at(&data, "to"))
    );
    let mut out = Map::new();
    out.insert("file_name".into(), json!(file_name));
    out.insert("content".into(), json!(content));
    Ok(Value::Object(out))
}
