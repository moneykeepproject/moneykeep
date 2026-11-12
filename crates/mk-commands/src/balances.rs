//! Balance commands (spec: ledger-balances.md).

use std::collections::{BTreeMap, HashSet};

use mk_core::currency::{format_amount, minor_units};
use mk_core::{AccountId, AccountType, Basis, CommodityCode, Date, Decimal};
use mk_ledger::{Balances, add_to};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::accounts::Chart;
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "account.balance",
        "Balance of an account per commodity, raw and display, as of a date",
        false,
        balance,
    );
    r.add(
        "report.trial_balance",
        "Every account with activity, its raw balance per commodity, and totals",
        false,
        trial_balance,
    );
}

/// Sum of split quantities per commodity over `accounts`, for posted transactions
/// dated on or before `as_of`, counting only splits the basis includes.
pub(crate) fn raw_balances(
    ctx: &Ctx<'_>,
    accounts: &[AccountId],
    as_of: Option<Date>,
    basis: Basis,
) -> CommandResult<Balances> {
    let chart = Chart::load(ctx)?;
    let set: HashSet<AccountId> = accounts.iter().copied().collect();
    let mut totals = Balances::new();
    for row in ctx.tx.posted_splits(as_of)? {
        if !set.contains(&row.account) || !basis.includes(row.reconcile) {
            continue;
        }
        let commodity = chart
            .get(row.account)
            .map(|a| a.commodity.clone())
            .ok_or_else(|| CommandError::Integrity("split references a missing account".into()))?;
        add_to(&mut totals, &commodity, row.quantity)?;
    }
    totals.retain(|_, v| !v.is_zero());
    Ok(totals)
}

/// Amounts per commodity, each at its output scale (see [`Chart::scale_of`]).
fn amounts_json(chart: &Chart, totals: &Balances, account_type: Option<AccountType>) -> Value {
    let mut m = Map::new();
    for (c, v) in totals {
        let v = account_type.map_or(*v, |t| t.display(*v));
        m.insert(c.to_string(), json!(format_amount(v, chart.scale_of(c))));
    }
    Value::Object(m)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BalanceArgs {
    /// Account id, full name or code.
    account: String,
    /// Count transactions dated on or before this date (default: all).
    #[serde(default)]
    as_of: Option<Date>,
    /// Include all sub-accounts. Default false.
    #[serde(default)]
    include_children: bool,
    #[serde(default)]
    basis: Basis,
}

fn balance(ctx: &Ctx<'_>, a: BalanceArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let acc = chart.resolve(&a.account)?;
    let ids = if a.include_children {
        chart.subtree(acc.id)
    } else {
        vec![acc.id]
    };
    let totals = raw_balances(ctx, &ids, a.as_of, a.basis)?;
    // `raw` is the map the spec defines (empty when there is no activity); `display`
    // shows no activity as zero in the account's own commodity.
    let display = if totals.is_empty() {
        let zero = format_amount(Decimal::ZERO, chart.scale_of(&acc.commodity));
        json!({ acc.commodity.to_string(): zero })
    } else {
        amounts_json(&chart, &totals, Some(acc.account_type))
    };
    // A security account's quantity is its share count; its value is the remaining
    // cost basis (investments-core.md).
    let value = if acc.subtype == mk_core::AccountSubtype::Security && !a.include_children {
        let mut v = Decimal::ZERO;
        for row in ctx.tx.posted_splits(a.as_of)? {
            if row.account == acc.id && a.basis.includes(row.reconcile) {
                v = v.checked_add(row.value).unwrap_or(v);
            }
        }
        json!(format_amount(
            v,
            Some(minor_units(&ctx.tx.info()?.base_currency))
        ))
    } else {
        Value::Null
    };
    Ok(json!({
        "account": acc.id,
        "full_name": chart.full_name(acc.id),
        "value": value,
        "type": acc.account_type,
        "as_of": a.as_of,
        "include_children": a.include_children,
        "basis": a.basis,
        "raw": amounts_json(&chart, &totals, None),
        "display": display,
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TrialArgs {
    /// Count transactions dated on or before this date (default: all).
    #[serde(default)]
    as_of: Option<Date>,
}

/// The trial balance as data, used by the command and by tests.
pub(crate) struct Trial {
    /// Raw balance per account per commodity (accounts with activity only).
    pub accounts: BTreeMap<AccountId, Balances>,
    /// Sum of split values per transaction currency (must all be zero).
    pub value_totals: Balances,
}

pub(crate) fn compute_trial(ctx: &Ctx<'_>, as_of: Option<Date>) -> CommandResult<Trial> {
    let chart = Chart::load(ctx)?;
    let mut accounts: BTreeMap<AccountId, Balances> = BTreeMap::new();
    let mut value_totals = Balances::new();
    for row in ctx.tx.posted_splits(as_of)? {
        let commodity = chart
            .get(row.account)
            .map(|a| a.commodity.clone())
            .ok_or_else(|| CommandError::Integrity("split references a missing account".into()))?;
        add_to(
            accounts.entry(row.account).or_default(),
            &commodity,
            row.quantity,
        )?;
        add_to(&mut value_totals, &row.currency, row.value)?;
    }
    for b in accounts.values_mut() {
        b.retain(|_, v| !v.is_zero());
    }
    Ok(Trial {
        accounts,
        value_totals,
    })
}

fn trial_balance(ctx: &Ctx<'_>, a: TrialArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let trial = compute_trial(ctx, a.as_of)?;
    let unbalanced: Vec<String> = trial
        .value_totals
        .iter()
        .filter(|(_, v)| !v.is_zero())
        .map(|(c, v)| format!("{v} {c}"))
        .collect();
    if !unbalanced.is_empty() {
        return Err(CommandError::Integrity(format!(
            "the books do not balance: split values sum to {} (this is a defect)",
            unbalanced.join(", ")
        )));
    }
    let mut rows = Vec::new();
    for (acc, _) in chart.tree_order() {
        let Some(b) = trial.accounts.get(&acc.id) else {
            continue;
        };
        rows.push(json!({
            "account": acc.id,
            "full_name": chart.full_name(acc.id),
            "type": acc.account_type,
            "raw": amounts_json(&chart, b, None),
        }));
    }
    let totals: BTreeMap<String, Value> = trial
        .value_totals
        .keys()
        .map(|c| {
            let zero = format_amount(Decimal::ZERO, Some(minor_units(c.as_str())));
            (c.to_string(), json!(zero))
        })
        .collect();

    // Accounting-equation diagnostic, base-currency accounts only, display balances.
    let base: CommodityCode = ctx.tx.info()?.base_currency.parse()?;
    let mut by_type: BTreeMap<AccountType, Decimal> = BTreeMap::new();
    for (id, b) in &trial.accounts {
        let Some(acc) = chart.get(*id) else { continue };
        if let Some(v) = b.get(&base) {
            let e = by_type.entry(acc.account_type).or_insert(Decimal::ZERO);
            *e = e
                .checked_add(acc.account_type.display(*v))
                .ok_or_else(|| CommandError::Rejected("arithmetic overflow".into()))?;
        }
    }
    let get = |t| by_type.get(&t).copied().unwrap_or(Decimal::ZERO);
    let assets = get(AccountType::Asset);
    let overflow = || CommandError::Rejected("arithmetic overflow".into());
    let rhs = get(AccountType::Liability)
        .checked_add(get(AccountType::Equity))
        .and_then(|v| v.checked_add(get(AccountType::Income)))
        .and_then(|v| v.checked_sub(get(AccountType::Expense)))
        .ok_or_else(overflow)?;
    let fmt = |v: Decimal| format_amount(v, Some(minor_units(base.as_str())));
    Ok(json!({
        "as_of": a.as_of,
        "accounts": rows,
        "value_totals": totals,
        "balanced": true,
        "equation": {
            "currency": base,
            "assets": fmt(assets),
            "liabilities": fmt(get(AccountType::Liability)),
            "equity": fmt(get(AccountType::Equity)),
            "income": fmt(get(AccountType::Income)),
            "expenses": fmt(get(AccountType::Expense)),
            "holds": assets == rhs,
        },
    }))
}
