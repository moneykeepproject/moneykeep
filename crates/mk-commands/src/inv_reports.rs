//! Investment views and reports (spec: investments-core.md, "Holdings and valuation"
//! and "Reports"): holdings, lots, realised gains and investment income.

use std::collections::HashMap;

use mk_core::currency::format_amount;
use mk_core::{AccountId, AccountSubtype, Date, Decimal, RoundingStrategy};
use mk_ledger::lots::{self, is_long_term};
use mk_store::today_utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::inv_kinds::BondTax;
use crate::investments::{
    Action, DividendKind, INVESTMENTS, all_metas, find_security, price_text, replay,
    sec_account_of, securities, valuations,
};
use crate::reports::{Csv, Dates, Interval, interval_name, period_json, periods, resolve};
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    let m = INVESTMENTS;
    r.add_in(
        m,
        "inv.holdings",
        "Holdings per brokerage account: shares, basis, price, market value, gain",
        false,
        holdings,
    );
    r.add_in(
        m,
        "inv.lots",
        "Open lots of a security in an account",
        false,
        lots_cmd,
    );
    r.add_in(
        m,
        "report.realized_gains",
        "Realised gains per lot portion sold, short- and long-term",
        false,
        realized_gains,
    );
    r.add_in(
        m,
        "report.investment_income",
        "Dividends, interest, capital gain distributions, realised gains and fees per period",
        false,
        investment_income,
    );
    r.add_in(
        m,
        "report.holdings",
        "The holdings of brokerage accounts as a report",
        false,
        holdings_report,
    );
}

fn money(v: Decimal) -> String {
    format_amount(v, Some(2))
}

fn qty(v: Decimal) -> String {
    format_amount(v, None)
}

fn pct(part: Decimal, of: Decimal) -> Option<String> {
    if of.is_zero() {
        return None;
    }
    part.checked_mul(Decimal::ONE_HUNDRED)
        .and_then(|x| x.checked_div(of))
        .map(|x| {
            format_amount(
                x.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero),
                Some(2),
            )
        })
}

fn add(slot: &mut Decimal, v: Decimal) {
    *slot = slot.checked_add(v).unwrap_or(*slot);
}

/// Brokerage accounts: the given ones (id, full name or code), or every one.
pub(crate) fn brokerages(
    chart: &Chart,
    refs: Option<&Vec<String>>,
) -> CommandResult<Vec<AccountId>> {
    let mut out = Vec::new();
    match refs {
        Some(list) => {
            for r in list {
                let a = chart.resolve(r)?;
                if a.subtype != AccountSubtype::Brokerage {
                    return Err(CommandError::InvalidArgs(format!(
                        "{} is not a brokerage account",
                        chart.full_name(a.id)
                    )));
                }
                out.push(a.id);
            }
        }
        None => {
            for (a, _) in chart.tree_order() {
                if a.subtype == AccountSubtype::Brokerage {
                    out.push(a.id);
                }
            }
        }
    }
    Ok(out)
}

// ----- holdings --------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HoldingsArgs {
    /// Brokerage accounts (default: all).
    #[serde(default)]
    accounts: Option<Vec<String>>,
    /// Default today.
    #[serde(default)]
    as_of: Option<Date>,
}

pub(crate) fn holdings_data(
    ctx: &Ctx<'_>,
    refs: Option<&Vec<String>>,
    as_of: Date,
) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let vals = valuations(ctx, &chart, as_of)?;
    let secs = securities(ctx)?;
    // Cash: each brokerage account's own balance.
    let mut cash: HashMap<AccountId, Decimal> = HashMap::new();
    for row in ctx.tx.posted_splits(Some(as_of))? {
        add(cash.entry(row.account).or_default(), row.quantity);
    }
    let mut accounts = Vec::new();
    let mut all_market = Decimal::ZERO;
    let mut all_basis = Decimal::ZERO;
    let mut all_cash = Decimal::ZERO;
    let mut any_unpriced = false;
    let needing = crate::opening::basis_needed_holdings(ctx, None)?;
    for b in brokerages(&chart, refs)? {
        let c = cash.get(&b).copied().unwrap_or_default();
        let need_n = needing.iter().filter(|(x, _)| *x == b).count();
        let mut rows = Vec::new();
        let mut market = Decimal::ZERO;
        let mut basis = Decimal::ZERO;
        let mut unpriced = false;
        let mut kids: Vec<_> = chart
            .children(b)
            .filter(|k| k.subtype == AccountSubtype::Security)
            .collect();
        kids.sort_by(|x, y| x.commodity.cmp(&y.commodity));
        let mut priced_rows: Vec<(Value, Option<Decimal>)> = Vec::new();
        for k in kids {
            let Some(v) = vals.get(&k.id) else { continue };
            if v.quantity.is_zero() && v.basis.is_zero() {
                continue;
            }
            let sec = secs.iter().find(|s| s.symbol == k.commodity.as_str());
            let needed = needing.iter().any(|(_, s)| *s == k.id);
            add(&mut basis, v.basis);
            let (mv, gain, gain_pct) = if v.valued() {
                add(&mut market, v.value);
                let g = v.value.checked_sub(v.basis).unwrap_or_default();
                // A written option's basis is the premium received (negative).
                (Some(v.value), Some(g), pct(g, v.basis.abs()))
            } else {
                unpriced = true;
                (None, None, None)
            };
            let extra = sec.map(|s| {
                crate::inv_kinds::holding_fields(
                    s,
                    v.quantity,
                    v.basis,
                    v.price.map(|(_, p)| p),
                    as_of,
                )
            });
            let mut row = json!({
                "account": k.id,
                "security": sec.map(|s| s.id.clone()),
                "symbol": k.commodity,
                "name": sec.map(|s| s.name.clone()),
                "quantity": qty(v.quantity),
                "basis": money(v.basis),
                "price": match v.price {
                    Some((_, p)) => Some(price_text(p)),
                    None if v.at_face => Some(price_text(Decimal::ONE_HUNDRED)),
                    None => None,
                },
                "price_date": v.price.map(|(d, _)| d),
                "price_note": v.at_face.then_some("at face value"),
                "market_value": mv.map_or(json!("unknown"), |m| json!(money(m))),
                "gain": gain.map(money),
                "gain_percent": gain_pct,
                "basis_needed": needed,
                "gain_note": needed.then_some("basis not entered"),
            });
            if let (Some(o), Some(Value::Object(e))) = (row.as_object_mut(), extra) {
                o.extend(e);
            }
            priced_rows.push((row, mv));
        }
        let total_market = market.checked_add(c).unwrap_or(market);
        for (mut row, mv) in priced_rows {
            if let (Some(o), Some(m)) = (row.as_object_mut(), mv) {
                o.insert("weight".into(), json!(pct(m, total_market)));
            }
            rows.push(row);
        }
        any_unpriced |= unpriced;
        add(&mut all_market, market);
        add(&mut all_basis, basis);
        add(&mut all_cash, c);
        let mut acc = json!({
            "account": b,
            "full_name": chart.full_name(b),
            "holdings": rows,
            "cash": money(c),
            "market_value": money(market),
            "basis": money(basis),
            "total": money(total_market),
            "unpriced": unpriced,
        });
        if need_n > 0
            && let Some(o) = acc.as_object_mut()
        {
            o.insert("basis_needed".into(), json!(need_n));
            o.insert(
                "basis_text".into(),
                json!(crate::opening::holdings_text(need_n)),
            );
            o.insert("basis_where".into(), json!(crate::opening::BASIS_WHERE));
        }
        if unpriced && let Some(o) = acc.as_object_mut() {
            o.insert(
                "note".into(),
                json!("Holdings without a price are left out of the market totals."),
            );
        }
        accounts.push(acc);
    }
    let total = all_market.checked_add(all_cash).unwrap_or(all_market);
    Ok(json!({
        "as_of": as_of,
        "accounts": accounts,
        "totals": {
            "market_value": money(all_market),
            "basis": money(all_basis),
            "cash": money(all_cash),
            "total": money(total),
        },
        "unpriced": any_unpriced,
    }))
}

fn today_or(d: Option<Date>) -> CommandResult<Date> {
    match d {
        Some(d) => Ok(d),
        None => Ok(today_utc()?),
    }
}

fn holdings(ctx: &Ctx<'_>, a: HoldingsArgs) -> CommandResult<Value> {
    holdings_data(ctx, a.accounts.as_ref(), today_or(a.as_of)?)
}

fn holdings_report(ctx: &Ctx<'_>, a: HoldingsArgs) -> CommandResult<Value> {
    let as_of = today_or(a.as_of)?;
    let mut v = holdings_data(ctx, a.accounts.as_ref(), as_of)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("report".into(), json!("holdings"));
        o.insert("from".into(), json!(as_of));
        o.insert("to".into(), json!(as_of));
    }
    Ok(v)
}

// ----- lots ------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LotsArgs {
    /// Brokerage account.
    account: String,
    /// Security id or symbol.
    security: String,
    #[serde(default)]
    as_of: Option<Date>,
}

fn lots_cmd(ctx: &Ctx<'_>, a: LotsArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let b = chart.resolve(&a.account)?.id;
    let s = find_security(ctx, &a.security)?;
    let as_of = today_or(a.as_of)?;
    let price = crate::investments::latest_price(ctx, &s.id, as_of)?;
    let Some(acc) = sec_account_of(&chart, b, &s.symbol) else {
        return Ok(json!({"lots": [], "as_of": as_of}));
    };
    let r = replay(ctx, acc, Some(as_of))?;
    let flags = crate::opening::lot_flags(ctx)?;
    let rows: Vec<Value> = r
        .state
        .open()
        .iter()
        .map(|l| {
            let per = l.basis.checked_div(l.remaining).map(|x| lots::round(x, 6));
            let unreal = price.map(|(_, p)| {
                s.worth(l.remaining, p)
                    .unwrap_or_default()
                    .checked_sub(l.basis)
                    .unwrap_or_default()
            });
            json!({
                "lot": l.id,
                "acquired": l.acquired,
                "quantity": qty(l.remaining),
                "original_quantity": qty(l.original),
                "basis": money(l.basis),
                "basis_per_share": per.map(|x| format_amount(x.normalize(), Some(2))),
                "term": if is_long_term(l.acquired, as_of) { "long" } else { "short" },
                "unrealized_gain": unreal.map(money),
                "txn": l.txn,
                // opening-balances.md: a placeholder basis, and bought on several dates.
                "basis_needed": flags.get(&l.id).is_some_and(|f| f.0),
                "acquired_various": flags.get(&l.id).is_some_and(|f| f.1),
            })
        })
        .collect();
    let mut rows = rows;
    rows.extend(crate::inv_kinds::short_lots_json(&r));
    Ok(json!({
        "account": b,
        "security": s.id,
        "symbol": s.symbol,
        "as_of": as_of,
        "price": price.map(|(d, p)| json!({"date": d, "price": price_text(p)})),
        "lots": rows,
    }))
}

// ----- realised gains --------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct GainsArgs {
    #[serde(flatten)]
    dates: Dates,
    /// Brokerage accounts (default: all).
    #[serde(default)]
    accounts: Option<Vec<String>>,
}

const NOTE: &str = "MoneyKeep keeps records; it does not give tax advice.";

/// Disposals that realise a gain or loss: sales, cash in lieu, returns of capital
/// beyond basis, and mergers with a recognised gain (or taxable).
fn realizes(f: &crate::investments::Found) -> bool {
    match f.action {
        Action::Sell | Action::CashInLieu | Action::ReturnOfCapital => true,
        // investments-options-bonds-crypto.md: every disposal these actions record
        // realises (an exercise's or assignment's option lots and moved lots record
        // none).
        a if a.is_extra() => true,
        Action::Merger => f.disposal.portions.iter().any(|p| !p.gain().is_zero()),
        _ => false,
    }
}

fn realized_gains(ctx: &Ctx<'_>, a: GainsArgs) -> CommandResult<Value> {
    let (from, to) = resolve(ctx, &a.dates)?;
    let chart = Chart::load(ctx)?;
    let secs = securities(ctx)?;
    let wash = crate::corporate::wash_sales(ctx)?;
    let flags = crate::opening::lot_flags(ctx)?;
    let mut disallowed = Decimal::ZERO;
    let mut rows: Vec<(Date, String, Value, bool, Decimal, Decimal, Decimal)> = Vec::new();
    for b in brokerages(&chart, a.accounts.as_ref())? {
        for k in chart
            .children(b)
            .filter(|k| k.subtype == AccountSubtype::Security)
        {
            let sec = secs.iter().find(|s| s.symbol == k.commodity.as_str());
            let r = replay(ctx, k.id, None)?;
            for f in r.disposals.iter().filter(|f| realizes(f)) {
                if f.date < from || f.date > to {
                    continue;
                }
                for p in &f.disposal.portions {
                    let g = p.gain();
                    let symbol =
                        sec.map_or_else(|| k.commodity.to_string(), |s| s.symbol_on(f.date));
                    let w = wash.get(&(f.txn, p.lot.clone()));
                    if let Some(w) = w {
                        add(&mut disallowed, w.disallowed);
                    }
                    let note = match (f.action, w) {
                        (_, Some(w)) => Some(format!(
                            "Wash sale: {} of the loss would be added to the basis of the replacement purchase; MoneyKeep records this but does not change the basis.{}",
                            money(w.disallowed),
                            if w.ira {
                                " A replacement in an IRA may disallow the loss permanently."
                            } else {
                                ""
                            }
                        )),
                        (Action::ReturnOfCapital, _) => {
                            Some("Return of capital beyond basis".to_owned())
                        }
                        (Action::Merger, _) => Some("Merger".to_owned()),
                        (Action::CashInLieu, _) => Some("Cash in lieu".to_owned()),
                        (a, _) if a.is_extra() => Some(a.label().to_owned()),
                        _ => None,
                    };
                    let description = sec.map_or_else(
                        || format!("{} {}", qty(p.quantity), k.commodity),
                        |s| crate::inv_kinds::description(s, p.quantity),
                    );
                    rows.push((
                        f.date,
                        symbol.clone(),
                        json!({
                            "account": b,
                            "symbol": symbol,
                            "wash_sale": w.is_some(),
                            "adjustment_code": w.map(|_| "W"),
                            "adjustment": w.map(|w| money(w.disallowed)),
                            "replacements": w.map(|w| w.replacements.clone()),
                            "note": note,
                            "action": f.action,
                            "description": description,
                            "name": sec.map(|s| s.name.clone()),
                            "quantity": qty(p.quantity),
                            "acquired": if f.disposal.average { json!("Various") } else { json!(p.acquired) },
                            "sold": f.date,
                            "proceeds": money(p.proceeds),
                            "basis": money(p.basis),
                            "gain": money(g),
                            "term": if p.long_term { "long" } else { "short" },
                            "txn": f.txn,
                            "basis_note": flags
                                .get(&p.lot)
                                .is_some_and(|f| f.0)
                                .then_some("basis not entered"),
                        }),
                        p.long_term,
                        p.proceeds,
                        p.basis,
                        g,
                    ));
                }
            }
        }
    }
    rows.sort_by(|x, y| (x.0, &x.1).cmp(&(y.0, &y.1)));
    let mut sections = Vec::new();
    let mut total = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    for (key, title, long) in [("short", "Short-term", false), ("long", "Long-term", true)] {
        let mut sub = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
        let list: Vec<Value> = rows
            .iter()
            .filter(|r| r.3 == long)
            .map(|r| {
                add(&mut sub.0, r.4);
                add(&mut sub.1, r.5);
                add(&mut sub.2, r.6);
                r.2.clone()
            })
            .collect();
        add(&mut total.0, sub.0);
        add(&mut total.1, sub.1);
        add(&mut total.2, sub.2);
        sections.push(json!({
            "term": key,
            "title": title,
            "rows": list,
            "proceeds": money(sub.0),
            "basis": money(sub.1),
            "gain": money(sub.2),
        }));
    }
    Ok(json!({
        "report": "realized_gains",
        "from": from,
        "to": to,
        "sections": sections,
        "total": {"proceeds": money(total.0), "basis": money(total.1), "gain": money(total.2),
                  "disallowed": money(disallowed)},
        "note": NOTE,
    }))
}

// ----- investment income -----------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct IncomeArgs {
    #[serde(flatten)]
    dates: Dates,
    #[serde(default)]
    accounts: Option<Vec<String>>,
    #[serde(default)]
    interval: Interval,
}

const INCOME_ROWS: [(&str, &str); 13] = [
    ("dividends_ordinary", "Ordinary dividends"),
    ("dividends_qualified", "Qualified dividends"),
    ("dividends_tax_exempt", "Tax-exempt dividends"),
    (
        "cap_gain_dist_long",
        "Capital gain distributions, long-term",
    ),
    (
        "cap_gain_dist_short",
        "Capital gain distributions, short-term",
    ),
    // investments-options-bonds-crypto.md revision 2: rewards, and interest split by
    // the bond's tax (rows shown only when non-zero).
    ("crypto_rewards", "Crypto rewards"),
    ("interest", "Interest"),
    ("interest_tax_exempt", "Tax-exempt interest"),
    ("interest_treasury", "US Treasury interest"),
    ("realized_short", "Realised gains, short-term"),
    ("realized_long", "Realised gains, long-term"),
    ("fees", "Fees"),
    ("total", "Total"),
];

/// Rows of [`INCOME_ROWS`] left out of the report when they are zero in every period.
const INCOME_OPTIONAL: [&str; 3] = ["crypto_rewards", "interest_tax_exempt", "interest_treasury"];

/// The income row of a bond's interest (by its `tax`).
fn interest_row(
    bond_tax: &HashMap<String, BondTax>,
    m: &crate::investments::InvMeta,
) -> &'static str {
    match m.security.as_ref().and_then(|s| bond_tax.get(s)) {
        Some(BondTax::Municipal) => "interest_tax_exempt",
        Some(BondTax::Treasury) => "interest_treasury",
        _ => "interest",
    }
}

/// One amount of the investment income report: its row, date, the amount shown in
/// the row, what it adds to the total (fees subtract), and the account it is posted
/// to (`None` for amounts that come from lots: realized gains and accrued interest).
struct IncomeEntry {
    key: &'static str,
    date: Date,
    amount: Decimal,
    to_total: Decimal,
    source: Option<AccountId>,
}

/// Every posted amount of the investment income report for the `wanted` brokerages.
fn income_entries(
    ctx: &Ctx<'_>,
    chart: &Chart,
    wanted: &[AccountId],
) -> CommandResult<Vec<IncomeEntry>> {
    let mut out = Vec::new();
    let mut put = |key: &'static str, date: Date, amount: Decimal, to_total: Decimal, source| {
        out.push(IncomeEntry {
            key,
            date,
            amount,
            to_total,
            source,
        });
    };
    let metas = all_metas(ctx)?;
    let bond_tax: HashMap<String, BondTax> = securities(ctx)?
        .into_iter()
        .filter_map(|s| s.bond.as_ref().map(|b| (s.id.clone(), b.tax)))
        .collect();
    for m in metas.values() {
        if !m.account.is_some_and(|b| wanted.contains(&b)) {
            continue;
        }
        let Some(t) = ctx.tx.transaction(m.txn)? else {
            continue;
        };
        if t.status != mk_core::TxnStatus::Posted {
            continue;
        }
        let amt = m.amount.unwrap_or_default();
        let other = m.other_account;
        let fee_account = m.extra.as_ref().and_then(|e| e.fee_account);
        match m.action {
            Action::Dividend | Action::Reinvest => {
                let key = match m.kind.unwrap_or(DividendKind::Ordinary) {
                    DividendKind::Ordinary => "dividends_ordinary",
                    DividendKind::Qualified => "dividends_qualified",
                    DividendKind::TaxExempt => "dividends_tax_exempt",
                    DividendKind::CapitalGainLong => "cap_gain_dist_long",
                    DividendKind::CapitalGainShort => "cap_gain_dist_short",
                };
                put(key, t.date, amt, amt, other);
            }
            Action::Interest => put("interest", t.date, amt, amt, other),
            Action::BondInterest => put(interest_row(&bond_tax, m), t.date, amt, amt, other),
            Action::CryptoReward => put("crypto_rewards", t.date, amt, amt, other),
            // Accrued interest paid offsets the year's interest; received adds to it.
            Action::BondBuy | Action::BondSell => {
                let acc = m.extra.as_ref().and_then(|e| e.accrued).unwrap_or_default();
                let v = if m.action == Action::BondBuy {
                    -acc
                } else {
                    acc
                };
                put(interest_row(&bond_tax, m), t.date, v, v, None);
            }
            Action::Fee => put("fees", t.date, amt, -amt, other),
            // Revision 3: fees paid in coin post Investment Fees at their value (a swap's
            // cash fee is added to the received coin's basis, so it posts no fee).
            Action::CryptoFee => put("fees", t.date, amt, -amt, fee_account),
            Action::TransferLots => {
                if let Some(fv) = m.extra.as_ref().and_then(|e| e.fee_value) {
                    put("fees", t.date, fv, -fv, fee_account);
                }
            }
            _ => {}
        }
    }
    for b in wanted {
        for k in chart
            .children(*b)
            .filter(|k| k.subtype == AccountSubtype::Security)
        {
            for f in replay(ctx, k.id, None)?
                .disposals
                .iter()
                .filter(|f| realizes(f))
            {
                for p in &f.disposal.portions {
                    let key = if p.long_term {
                        "realized_long"
                    } else {
                        "realized_short"
                    };
                    put(key, f.date, p.gain(), p.gain(), None);
                }
            }
        }
    }
    Ok(out)
}

fn investment_income(ctx: &Ctx<'_>, a: IncomeArgs) -> CommandResult<Value> {
    let (from, to) = resolve(ctx, &a.dates)?;
    let ps = periods(from, to, a.interval)?;
    let chart = Chart::load(ctx)?;
    let wanted = brokerages(&chart, a.accounts.as_ref())?;
    let n = ps.len();
    let idx = |d: Date| ps.iter().position(|p| p.from <= d && d <= p.to);
    let mut grid: HashMap<&str, Vec<Decimal>> = INCOME_ROWS
        .iter()
        .map(|(k, _)| (*k, vec![Decimal::ZERO; n]))
        .collect();
    let mut put = |key: &str, d: Date, v: Decimal| {
        if let (Some(i), Some(row)) = (idx(d), grid.get_mut(key))
            && let Some(slot) = row.get_mut(i)
        {
            add(slot, v);
        }
    };
    for e in income_entries(ctx, &chart, &wanted)? {
        put(e.key, e.date, e.amount);
        put("total", e.date, e.to_total);
    }
    let rows: Vec<Value> = INCOME_ROWS
        .iter()
        .filter(|(k, _)| {
            !INCOME_OPTIONAL.contains(k)
                || grid.get(k).is_some_and(|v| v.iter().any(|x| !x.is_zero()))
        })
        .map(|(k, label)| {
            let vals = grid.get(k).cloned().unwrap_or_default();
            let mut t = Decimal::ZERO;
            for v in &vals {
                add(&mut t, *v);
            }
            json!({
                "key": k,
                "label": label,
                "amounts": vals.iter().map(|v| money(*v)).collect::<Vec<_>>(),
                "total": money(t),
            })
        })
        .collect();
    Ok(json!({
        "report": "investment_income",
        "from": from,
        "to": to,
        "interval": interval_name(a.interval),
        "periods": ps.iter().map(period_json).collect::<Vec<_>>(),
        "rows": rows,
    }))
}

// ----- CSV export ------------------------------------------------------------------

fn at<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&Value::Null)
}

fn s(v: &Value) -> String {
    match v {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn list(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

/// Run an investment report for `report.export`: (data, CSV content).
pub(crate) fn export(
    ctx: &Ctx<'_>,
    name: &str,
    args: Value,
) -> CommandResult<Option<(Value, String)>> {
    let names = ["holdings", "realized_gains", "investment_income"];
    if !names.contains(&name) {
        return Ok(None);
    }
    if !crate::investments::is_enabled(ctx)? {
        return Err(CommandError::ModuleDisabled {
            command: format!("report.{name}"),
            module: INVESTMENTS.into(),
        });
    }
    Ok(Some(match name {
        "holdings" => {
            let data = holdings_report(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(
                &[
                    "Account",
                    "Symbol",
                    "Name",
                    "Shares",
                    "Price",
                    "Price date",
                    "Market value",
                    "Cost basis",
                    "Gain",
                    "Gain %",
                    "Weight %",
                ]
                .map(str::to_owned),
            );
            for acc in list(at(&data, "accounts")) {
                for h in list(at(&acc, "holdings")) {
                    c.row(&[
                        s(at(&acc, "full_name")),
                        s(at(&h, "symbol")),
                        s(at(&h, "name")),
                        s(at(&h, "quantity")),
                        s(at(&h, "price")),
                        s(at(&h, "price_date")),
                        s(at(&h, "market_value")),
                        s(at(&h, "basis")),
                        s(at(&h, "gain")),
                        s(at(&h, "gain_percent")),
                        s(at(&h, "weight")),
                    ]);
                }
                c.row(&[
                    s(at(&acc, "full_name")),
                    "Cash".into(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    s(at(&acc, "cash")),
                ]);
                c.row(&[
                    s(at(&acc, "full_name")),
                    "Total".into(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    s(at(&acc, "total")),
                    s(at(&acc, "basis")),
                ]);
            }
            (data.clone(), c.out)
        }
        "realized_gains" => {
            let data = realized_gains(ctx, crate::parse_args(args)?)?;
            let mut c = Csv::new(
                &[
                    "Symbol",
                    "Name",
                    "Shares",
                    "Acquired",
                    "Sold",
                    "Proceeds",
                    "Cost basis",
                    "Gain",
                    "Term",
                    "Code",
                    "Adjustment",
                    "Description",
                ]
                .map(str::to_owned),
            );
            for sec in list(at(&data, "sections")) {
                c.heading(&s(at(&sec, "title")));
                for r in list(at(&sec, "rows")) {
                    c.row(&[
                        s(at(&r, "symbol")),
                        s(at(&r, "name")),
                        s(at(&r, "quantity")),
                        s(at(&r, "acquired")),
                        s(at(&r, "sold")),
                        s(at(&r, "proceeds")),
                        s(at(&r, "basis")),
                        s(at(&r, "gain")),
                        s(at(&r, "term")),
                        s(at(&r, "adjustment_code")),
                        s(at(&r, "adjustment")),
                        s(at(&r, "description")),
                    ]);
                }
                c.row(&[
                    format!("Total {}", s(at(&sec, "title")).to_lowercase()),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    s(at(&sec, "proceeds")),
                    s(at(&sec, "basis")),
                    s(at(&sec, "gain")),
                ]);
            }
            let t = at(&data, "total");
            c.row(&[
                "Total".into(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                s(at(t, "proceeds")),
                s(at(t, "basis")),
                s(at(t, "gain")),
            ]);
            c.row(&[
                "Disallowed (wash sales)".into(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                s(at(t, "disallowed")),
            ]);
            (data.clone(), c.out)
        }
        _ => {
            let data = investment_income(ctx, crate::parse_args(args)?)?;
            let mut header = vec!["Item".to_owned()];
            header.extend(list(at(&data, "periods")).iter().map(|p| s(at(p, "label"))));
            header.push("Total".into());
            let mut c = Csv::new(&header);
            for r in list(at(&data, "rows")) {
                let mut cells = vec![s(at(&r, "label"))];
                cells.extend(list(at(&r, "amounts")).iter().map(s));
                cells.push(s(at(&r, "total")));
                c.row(&cells);
            }
            (data.clone(), c.out)
        }
    }))
}

/// The note under the Tax summary's investment income heading (release-prep.md §1).
pub(crate) const TAX_SECTION_NOTE: &str = "Details of investment income. Amounts already listed under a tax line above are marked and not added again.";

/// The note in place of a separate figure for an amount already in a tax line.
pub(crate) fn included_above(tax_line: &str) -> String {
    format!("Included above under {tax_line}")
}

/// The Tax summary's investment income section (investments-options-bonds-crypto.md
/// revisions 2 and 3; release-prep.md §1): crypto rewards, interest by kind, realized
/// gains and fees, with the investment income report's row names, each only when
/// non-zero. An amount posted to an account with a tax line (one the Tax summary lists
/// under that line) is shown with `included_in` set to the line and left out of the
/// section's total; amounts from lots (realized gains, accrued interest) always count.
/// `None` when the investments module is off or every row is zero.
pub(crate) fn tax_section(ctx: &Ctx<'_>, from: Date, to: Date) -> CommandResult<Option<Value>> {
    if !crate::investments::is_enabled(ctx)? {
        return Ok(None);
    }
    let chart = Chart::load(ctx)?;
    let base = crate::investments::base_currency(ctx)?;
    let wanted = brokerages(&chart, None)?;
    let keys = [
        "crypto_rewards",
        "interest",
        "interest_tax_exempt",
        "interest_treasury",
        "realized_short",
        "realized_long",
        "fees",
    ];
    // The tax line the Tax summary lists an account's activity under, if any.
    let listed_under = |id: AccountId| -> Option<String> {
        let a = chart.get(id)?;
        let t = a.tax_line.as_ref()?;
        (a.commodity.as_str() == base && !t.to_lowercase().starts_with("sch l")).then(|| t.clone())
    };
    // Per row: the amounts by the tax line they are included under (None: counted).
    // (tax line, amount, what it adds to the total)
    type Part = (Option<String>, Decimal, Decimal);
    let mut parts: HashMap<&str, Vec<Part>> = HashMap::new();
    for e in income_entries(ctx, &chart, &wanted)? {
        if e.date < from || e.date > to || !keys.contains(&e.key) {
            continue;
        }
        let line = e.source.and_then(listed_under);
        let v = parts.entry(e.key).or_default();
        match v.iter_mut().find(|(l, _, _)| *l == line) {
            Some(slot) => {
                add(&mut slot.1, e.amount);
                add(&mut slot.2, e.to_total);
            }
            None => v.push((line, e.amount, e.to_total)),
        }
    }
    let mut rows = Vec::new();
    let mut total = Decimal::ZERO;
    for (key, label) in INCOME_ROWS.iter().filter(|(k, _)| keys.contains(k)) {
        let Some(v) = parts.get_mut(key) else {
            continue;
        };
        v.retain(|(_, amount, _)| !amount.is_zero());
        // Included parts first (by tax line), then the counted part.
        v.sort_by(|x, y| match (&x.0, &y.0) {
            (Some(a), Some(b)) => crate::chart::tax_summary::tag_cmp(a, b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        let split = v.len() > 1;
        for (line, amount, to_total) in v.iter() {
            match line {
                Some(l) => rows.push(json!({
                    "id": format!("{key}#{l}"),
                    "key": key,
                    "label": label,
                    "amount": money(*amount),
                    "included_in": l,
                    "note": included_above(l),
                })),
                None => {
                    add(&mut total, *to_total);
                    let label = if split {
                        format!("{label}, not included above")
                    } else {
                        (*label).to_owned()
                    };
                    rows.push(json!({
                        "id": key,
                        "key": key,
                        "label": label,
                        "amount": money(*amount),
                        "included_in": Value::Null,
                    }));
                }
            }
        }
    }
    if rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(json!({
        "title": "Investment income",
        "note": TAX_SECTION_NOTE,
        "rows": rows,
        "total_label": "Total, not counting amounts included above",
        "total": money(total),
    })))
}
