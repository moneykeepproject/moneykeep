//! `report.performance` (investments-advanced.md §1): start and end values, external
//! flows, income, fees, gain, money-weighted return (XIRR by decimal bisection) and
//! time-weighted return (monthly Modified Dietz, linked geometrically), plus a value
//! series for the chart. All arithmetic is in `rust_decimal`; no floating point.

use std::collections::{BTreeMap, HashMap, HashSet};

use mk_core::{
    AccountId, AccountSubtype, AccountType, Date, Decimal, MathematicalOps, RoundingStrategy,
    TxnStatus,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::investments::{
    Action, INVESTMENTS, InvMeta, Valued, all_metas, latest_price, neg, securities, valuations,
};
use crate::reports::{Dates, Interval, periods, resolve};
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    r.add_in(
        INVESTMENTS,
        "report.performance",
        "Investment performance: values, flows, income, fees, gain, money- and time-weighted returns",
        false,
        performance,
    );
}

const NOTE: &str = "MoneyKeep keeps records; it does not give tax or investment advice.";

#[derive(Debug, Deserialize, JsonSchema)]
struct PerfArgs {
    #[serde(flatten)]
    dates: Dates,
    /// Brokerage accounts (default: all).
    #[serde(default)]
    accounts: Option<Vec<String>>,
    /// `account` (default), `security` or `total`.
    #[serde(default)]
    group: Option<String>,
    /// Interval of the value series (default month).
    #[serde(default)]
    interval: Interval,
}

fn bad(m: impl Into<String>) -> CommandError {
    CommandError::Rejected(m.into())
}

fn add(a: Decimal, b: Decimal) -> CommandResult<Decimal> {
    a.checked_add(b).ok_or_else(|| bad("arithmetic overflow"))
}

fn sub(a: Decimal, b: Decimal) -> CommandResult<Decimal> {
    a.checked_sub(b).ok_or_else(|| bad("arithmetic overflow"))
}

fn money(v: Decimal) -> String {
    mk_core::currency::format_amount(
        v.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero),
        Some(2),
    )
}

fn days(from: Date, to: Date) -> i64 {
    from.days_until(to)
}

/// External flows (date, amount in), income and fees of a row.
type Flows = (Vec<(Date, Decimal)>, Decimal, Decimal);

/// A row of the report: which accounts, or which security.
#[derive(Debug, Clone)]
enum RowKey {
    Accounts(Vec<AccountId>),
    Security(String),
}

#[derive(Debug, Clone)]
struct Row {
    key: RowKey,
    name: String,
    /// Brokerage accounts whose flows and values count.
    brokers: Vec<AccountId>,
}

/// Values on one date: security valuations and brokerage cash.
struct Snapshot {
    vals: HashMap<AccountId, Valued>,
    cash: HashMap<AccountId, Decimal>,
}

struct Ctx2<'a, 'c> {
    ctx: &'a Ctx<'c>,
    chart: Chart,
    secs: Vec<crate::investments::Security>,
    metas: HashMap<mk_core::TransactionId, InvMeta>,
    snaps: BTreeMap<Date, Snapshot>,
}

impl Ctx2<'_, '_> {
    fn snapshot(&mut self, date: Date) -> CommandResult<&Snapshot> {
        if !self.snaps.contains_key(&date) {
            let vals = valuations(self.ctx, &self.chart, date)?;
            let mut cash: HashMap<AccountId, Decimal> = HashMap::new();
            for row in self.ctx.tx.posted_splits(Some(date))? {
                if self
                    .chart
                    .get(row.account)
                    .is_some_and(|a| a.subtype == AccountSubtype::Brokerage)
                {
                    let e = cash.entry(row.account).or_default();
                    *e = e.checked_add(row.quantity).unwrap_or(*e);
                }
            }
            self.snaps.insert(date, Snapshot { vals, cash });
        }
        self.snaps
            .get(&date)
            .ok_or_else(|| CommandError::Integrity("missing snapshot".into()))
    }

    /// The row's value at the end of `date`, and whether any holding was valued at cost.
    fn value(&mut self, row: &Row, date: Date) -> CommandResult<(Decimal, bool)> {
        let chart_children: Vec<(AccountId, AccountId, String)> = self
            .chart
            .accounts
            .iter()
            .filter(|a| a.subtype == AccountSubtype::Security)
            .filter_map(|a| a.parent.map(|p| (a.id, p, a.commodity.to_string())))
            .collect();
        let brokers = row.brokers.clone();
        let key = row.key.clone();
        let snap = self.snapshot(date)?;
        let mut cash = Decimal::ZERO;
        if let RowKey::Accounts(list) = &key {
            for b in list {
                cash = add(cash, snap.cash.get(b).copied().unwrap_or_default())?;
            }
        }
        let mut total = cash;
        let mut at_cost = false;
        let mut take = |v: &Valued| -> CommandResult<()> {
            if !(v.quantity.is_zero() && v.basis.is_zero()) {
                total = add(total, v.value)?;
                at_cost |= !v.valued();
            }
            Ok(())
        };
        match &key {
            RowKey::Accounts(list) => {
                for (acc, parent, _) in &chart_children {
                    if list.contains(parent)
                        && let Some(v) = snap.vals.get(acc)
                    {
                        take(v)?;
                    }
                }
            }
            RowKey::Security(symbol) => {
                for (acc, parent, sym) in &chart_children {
                    if brokers.contains(parent)
                        && sym == symbol
                        && let Some(v) = snap.vals.get(acc)
                    {
                        take(v)?;
                    }
                }
            }
        }
        Ok((total, at_cost))
    }

    /// Market value of `quantity` shares of a security on a date, else `basis`.
    fn market(
        &self,
        security: Option<&str>,
        quantity: Decimal,
        basis: Decimal,
        date: Date,
    ) -> CommandResult<Decimal> {
        let Some(s) = security.and_then(|id| self.secs.iter().find(|s| s.id == id)) else {
            return Ok(basis);
        };
        match latest_price(self.ctx, &s.id, date)? {
            Some((_, p)) => s.worth(quantity, p),
            None => Ok(basis),
        }
    }

    /// External flows (date, amount in) of a row within [from, to], plus income and
    /// fees in the period.
    fn flows(&self, row: &Row, from: Date, to: Date) -> CommandResult<Flows> {
        let mut flows = Vec::new();
        let mut income = Decimal::ZERO;
        let mut fees = Decimal::ZERO;
        let subtree: HashSet<AccountId> = match &row.key {
            RowKey::Accounts(list) => list.iter().flat_map(|b| self.chart.subtree(*b)).collect(),
            RowKey::Security(_) => row
                .brokers
                .iter()
                .flat_map(|b| self.chart.subtree(*b))
                .collect(),
        };
        let txns = self.ctx.tx.transactions(&mk_store::TxnFilter {
            accounts: Some(subtree.iter().copied().collect()),
            from: Some(from),
            to: Some(to),
            include_voided: false,
            limit: None,
            offset: 0,
        })?;
        let value_in = |t: &mk_core::Transaction, acc: Option<AccountId>| {
            t.splits
                .iter()
                .filter(|s| Some(s.account) == acc)
                .fold(Decimal::ZERO, |a, s| a.checked_add(s.value).unwrap_or(a))
        };
        for t in txns.iter().filter(|t| t.status == TxnStatus::Posted) {
            let meta = self.metas.get(&t.id);
            // Income and fees count for the row the action belongs to.
            let belongs = |m: &InvMeta| match &row.key {
                RowKey::Accounts(list) => m.account.is_some_and(|a| list.contains(&a)),
                RowKey::Security(sym) => {
                    let mine = |id: &Option<String>| {
                        id.as_ref()
                            .and_then(|id| self.secs.iter().find(|s| s.id == *id))
                            .is_some_and(|s| s.symbol == *sym)
                    };
                    m.account.is_some_and(|a| row.brokers.contains(&a))
                        && (mine(&m.security) || mine(&m.security2))
                }
            };
            if let Some(m) = meta.filter(|m| belongs(m)) {
                match m.action {
                    Action::Dividend | Action::Reinvest | Action::Interest => {
                        income = add(income, m.amount.unwrap_or_default())?;
                    }
                    Action::Fee => fees = add(fees, m.amount.unwrap_or_default())?,
                    _ => {}
                }
                fees = add(fees, m.commission)?;
            }
            match (&row.key, meta) {
                (RowKey::Accounts(_), Some(m)) => {
                    if !belongs(m) {
                        continue;
                    }
                    match m.action {
                        Action::SharesIn | Action::SharesOut => {
                            let q = m.quantity.unwrap_or_default();
                            let basis = value_in(t, m.sec_account).abs();
                            let v = self.market(m.security.as_deref(), q, basis, t.date)?;
                            let v = if m.action == Action::SharesOut {
                                neg(v)
                            } else {
                                v
                            };
                            flows.push((t.date, v));
                        }
                        _ => {}
                    }
                }
                (RowKey::Accounts(_), None) => {
                    // A plain transfer: money from accounts outside the row's accounts.
                    let mut f = Decimal::ZERO;
                    for s in &t.splits {
                        if subtree.contains(&s.account) {
                            continue;
                        }
                        let outside = self.chart.get(s.account).is_some_and(|a| {
                            matches!(
                                a.account_type,
                                AccountType::Asset | AccountType::Liability | AccountType::Equity
                            )
                        });
                        if outside {
                            f = sub(f, s.value)?;
                        }
                    }
                    if !f.is_zero() {
                        flows.push((t.date, f));
                    }
                }
                (RowKey::Security(_), Some(m)) if belongs(m) => {
                    let sym_of = |id: &Option<String>| {
                        id.as_ref()
                            .and_then(|id| self.secs.iter().find(|s| s.id == *id))
                            .map(|s| s.symbol.clone())
                    };
                    let RowKey::Security(sym) = &row.key else {
                        continue;
                    };
                    let is_old = sym_of(&m.security).as_deref() == Some(sym.as_str());
                    let amt = m.amount.unwrap_or_default();
                    let f = match m.action {
                        Action::Buy => Some(amt),
                        Action::Sell
                        | Action::CashInLieu
                        | Action::Dividend
                        | Action::Interest
                        | Action::ReturnOfCapital => Some(neg(amt)),
                        Action::SharesIn | Action::SharesOut => {
                            let q = m.quantity.unwrap_or_default();
                            let basis = value_in(t, m.sec_account).abs();
                            let v = self.market(m.security.as_deref(), q, basis, t.date)?;
                            Some(if m.action == Action::SharesOut {
                                neg(v)
                            } else {
                                v
                            })
                        }
                        // Corporate actions move value between securities at basis.
                        Action::Merger | Action::Spinoff => {
                            let moved = value_in(t, m.sec_account2);
                            let cash = value_in(t, m.account);
                            Some(if is_old {
                                neg(add(moved, cash)?)
                            } else {
                                moved
                            })
                        }
                        _ => None,
                    };
                    if let Some(f) = f.filter(|f| !f.is_zero()) {
                        flows.push((t.date, f));
                    }
                }
                _ => {}
            }
        }
        flows.sort_by_key(|(d, _)| *d);
        Ok((flows, income, fees))
    }
}

/// Σ CFᵢ · (1 + r)^((D − dᵢ)/365): the XIRR equation multiplied through by
/// (1 + r)^(D/365) so every exponent is non-negative. `None` on overflow.
fn npv(cfs: &[(i64, Decimal)], end: i64, r: Decimal) -> Option<Decimal> {
    let base = Decimal::ONE.checked_add(r)?;
    let mut total = Decimal::ZERO;
    for (d, cf) in cfs {
        if cf.is_zero() {
            continue;
        }
        let e = Decimal::from(end.checked_sub(*d)?).checked_div(Decimal::from(365))?;
        let f = base.checked_powd(e)?;
        total = total.checked_add(cf.checked_mul(f)?)?;
    }
    Some(total)
}

/// The sign of the equation at `r`; on overflow, the sign of the term with the
/// largest exponent (it dominates).
fn sign(cfs: &[(i64, Decimal)], end: i64, r: Decimal) -> i32 {
    match npv(cfs, end, r) {
        Some(v) if v > Decimal::ZERO => 1,
        Some(v) if v < Decimal::ZERO => -1,
        Some(_) => 0,
        None => cfs
            .iter()
            .filter(|(_, c)| !c.is_zero())
            .min_by_key(|(d, _)| *d)
            .map_or(0, |(_, c)| if c.is_sign_negative() { -1 } else { 1 }),
    }
}

/// Solve for r between −99.99 % and +10,000 % by bisection to 1e-10.
fn xirr(cfs: &[(i64, Decimal)], end: i64) -> Result<Decimal, String> {
    if cfs.iter().all(|(_, c)| c.is_zero()) {
        return Err("nothing invested in the period".into());
    }
    let mut lo = Decimal::new(-9999, 4);
    let mut hi = Decimal::from(100);
    let (slo, shi) = (sign(cfs, end, lo), sign(cfs, end, hi));
    if slo == 0 {
        return Ok(lo);
    }
    if shi == 0 {
        return Ok(hi);
    }
    if slo == shi {
        return Err("no rate between −99.99 % and +10,000 % fits these flows".into());
    }
    let tol = Decimal::new(1, 10);
    let two = Decimal::TWO;
    let mut guard = 0;
    while hi.checked_sub(lo).is_some_and(|w| w > tol) && guard < 200 {
        guard += 1;
        let mid = lo
            .checked_add(hi)
            .and_then(|x| x.checked_div(two))
            .unwrap_or(lo);
        let s = sign(cfs, end, mid);
        if s == 0 {
            return Ok(mid);
        }
        if s == slo {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(lo
        .checked_add(hi)
        .and_then(|x| x.checked_div(two))
        .unwrap_or(lo))
}

/// (1 + r)^(num/365) − 1.
fn rescale(r: Decimal, num: i64) -> Option<Decimal> {
    let e = Decimal::from(num).checked_div(Decimal::from(365))?;
    Decimal::ONE
        .checked_add(r)?
        .checked_powd(e)?
        .checked_sub(Decimal::ONE)
}

fn measure(value: Result<Decimal, String>, annualised: bool) -> Value {
    match value {
        Ok(v) => {
            let pct = v
                .checked_mul(Decimal::ONE_HUNDRED)
                .map(|p| p.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero));
            json!({
                "value": v.round_dp(12).normalize().to_string(),
                "percent": pct.map(|p| mk_core::currency::format_amount(p, Some(2))),
                "annualised": annualised,
                "label": if annualised { "annualised" } else { "not annualised" },
                "reason": null,
            })
        }
        Err(why) => json!({"value": null, "percent": "n/a", "annualised": annualised,
                           "label": null, "reason": why}),
    }
}

fn performance(ctx: &Ctx<'_>, a: PerfArgs) -> CommandResult<Value> {
    let (from, to) = resolve(ctx, &a.dates)?;
    let chart = Chart::load(ctx)?;
    let brokers = crate::inv_reports::brokerages(&chart, a.accounts.as_ref())?;
    let group = a.group.clone().unwrap_or_else(|| "account".into());
    let secs = securities(ctx)?;
    let mut rows: Vec<Row> = Vec::new();
    match group.as_str() {
        "account" => {
            for b in &brokers {
                rows.push(Row {
                    key: RowKey::Accounts(vec![*b]),
                    name: chart.full_name(*b),
                    brokers: vec![*b],
                });
            }
        }
        "total" => rows.push(Row {
            key: RowKey::Accounts(brokers.clone()),
            name: "Total".into(),
            brokers: brokers.clone(),
        }),
        "security" => {
            let mut syms: Vec<String> = chart
                .accounts
                .iter()
                .filter(|x| {
                    x.subtype == AccountSubtype::Security
                        && x.parent.is_some_and(|p| brokers.contains(&p))
                })
                .map(|x| x.commodity.to_string())
                .collect();
            syms.sort();
            syms.dedup();
            for s in syms {
                let name = secs
                    .iter()
                    .find(|x| x.symbol == s)
                    .map_or(s.clone(), |x| format!("{}  {}", x.symbol, x.name));
                rows.push(Row {
                    key: RowKey::Security(s),
                    name,
                    brokers: brokers.clone(),
                });
            }
        }
        other => {
            return Err(CommandError::InvalidArgs(format!(
                "group is account, security or total, not {other:?}"
            )));
        }
    }
    let mut c = Ctx2 {
        ctx,
        chart,
        secs,
        metas: all_metas(ctx)?,
        snaps: BTreeMap::new(),
    };
    let period_days = days(from, to).saturating_add(1);
    let annualise = period_days >= 365;
    let before = from.previous_day().unwrap_or(from);
    let subs = periods(from, to, Interval::Month)?;
    let series_ends: Vec<Date> = periods(from, to, a.interval)?
        .iter()
        .map(|p| p.to)
        .collect();
    let mut out_rows = Vec::new();
    let mut series = Vec::new();
    for row in &rows {
        let (start, _) = c.value(row, before)?;
        let (end, at_cost) = c.value(row, to)?;
        let (flows, income, fees) = c.flows(row, from, to)?;
        let net = flows
            .iter()
            .try_fold(Decimal::ZERO, |t, (_, f)| add(t, *f))?;
        let gain = sub(sub(end, start)?, net)?;
        let empty = start.is_zero() && end.is_zero() && flows.is_empty();
        // Money-weighted: investor's cash flows (money in is negative).
        let mut cfs: Vec<(i64, Decimal)> = vec![(0, neg(start))];
        for (d, f) in &flows {
            cfs.push((days(from, *d), neg(*f)));
        }
        cfs.push((period_days, end));
        let annual = if empty {
            Err("no holdings in the period".to_owned())
        } else {
            xirr(&cfs, period_days)
        };
        let mwr = annual.clone().and_then(|r| {
            if annualise {
                Ok(r)
            } else {
                rescale(r, period_days).ok_or_else(|| "arithmetic overflow".to_owned())
            }
        });
        // Time-weighted: monthly Modified Dietz, linked.
        let twr = if empty {
            Err("no holdings in the period".to_owned())
        } else {
            let mut link = Decimal::ONE;
            let mut ok = true;
            for p in &subs {
                let b = p.from.previous_day().unwrap_or(p.from);
                let (bmv, _) = c.value(row, b)?;
                let (emv, _) = c.value(row, p.to)?;
                let d_total = Decimal::from(days(p.from, p.to).saturating_add(1));
                let mut cf = Decimal::ZERO;
                let mut weighted = Decimal::ZERO;
                for (d, f) in flows.iter().filter(|(d, _)| *d >= p.from && *d <= p.to) {
                    let day = Decimal::from(days(p.from, *d).saturating_add(1));
                    let w = d_total
                        .checked_sub(day)
                        .and_then(|x| x.checked_add(Decimal::ONE))
                        .and_then(|x| x.checked_div(d_total))
                        .unwrap_or_default();
                    cf = add(cf, *f)?;
                    weighted = add(weighted, w.checked_mul(*f).unwrap_or_default())?;
                }
                let denom = add(bmv, weighted)?;
                let r = if denom.is_zero() {
                    Decimal::ZERO
                } else {
                    match sub(sub(emv, bmv)?, cf)?.checked_div(denom) {
                        Some(r) => r,
                        None => {
                            ok = false;
                            break;
                        }
                    }
                };
                match Decimal::ONE
                    .checked_add(r)
                    .and_then(|x| link.checked_mul(x))
                {
                    Some(l) => link = l,
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            let cumulative = link.checked_sub(Decimal::ONE);
            match (ok, cumulative) {
                (true, Some(t)) if annualise => {
                    let e = Decimal::from(365).checked_div(Decimal::from(period_days));
                    e.and_then(|e| link.checked_powd(e))
                        .and_then(|x| x.checked_sub(Decimal::ONE))
                        .ok_or_else(|| "arithmetic overflow".to_owned())
                        .or(Ok(t))
                }
                (true, Some(t)) => Ok(t),
                _ => Err("arithmetic overflow".to_owned()),
            }
        };
        let mut points = Vec::new();
        for d in &series_ends {
            let (v, _) = c.value(row, *d)?;
            points.push(json!({"date": d, "value": money(v)}));
        }
        series.push(json!({"name": row.name, "points": points}));
        let mut mwr_json = measure(mwr, annualise);
        if let (Some(o), Ok(r)) = (mwr_json.as_object_mut(), &annual) {
            o.insert(
                "annual_rate".into(),
                json!(r.round_dp(12).normalize().to_string()),
            );
        }
        out_rows.push(json!({
            "name": row.name,
            "start_value": money(start),
            "end_value": money(end),
            "net_flows": money(net),
            "income": money(income),
            "fees": money(fees),
            "gain": money(gain),
            "valued_at_cost": at_cost,
            "mwr": mwr_json,
            "twr": measure(twr, annualise),
            "flows": flows.iter().map(|(d, f)| json!({"date": d, "amount": money(*f)})).collect::<Vec<_>>(),
        }));
    }
    Ok(json!({
        "report": "performance",
        "from": from,
        "to": to,
        "days": period_days,
        "group": group,
        "rows": out_rows,
        "series": series,
        "note": NOTE,
        "explanation": "Money-weighted return reflects the timing of your deposits and withdrawals; time-weighted return removes it, to compare with an index.",
    }))
}
