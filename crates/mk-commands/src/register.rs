//! Register view and entry helpers (spec: ledger-register.md).

use std::collections::HashSet;

use mk_core::currency::format_amount;
use mk_core::{AccountId, AccountType, Basis, Date, Decimal, TxnStatus};
use mk_import::normalise_payee;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "account.register",
        "An account's transactions in register order with running balances",
        false,
        account_register,
    );
    r.add(
        "payee.suggest",
        "Suggest payees by prefix, most recently used first, with their last category and amount",
        false,
        payee_suggest,
    );
    r.add(
        "account.suggest",
        "Suggest accounts by name prefix for entry fields",
        false,
        account_suggest,
    );
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Order {
    #[default]
    Asc,
    Desc,
}

fn yes() -> bool {
    true
}
fn default_limit() -> usize {
    200
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RegisterArgs {
    /// Account id, full name or code.
    account: String,
    #[serde(default)]
    from: Option<Date>,
    #[serde(default)]
    to: Option<Date>,
    #[serde(default = "yes")]
    include_voided: bool,
    #[serde(default)]
    include_children: bool,
    #[serde(default)]
    offset: usize,
    /// Default 200, max 5000.
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    order: Order,
}

fn account_register(ctx: &Ctx<'_>, a: RegisterArgs) -> CommandResult<Value> {
    if a.limit > 5000 {
        return Err(CommandError::InvalidArgs("limit is at most 5000".into()));
    }
    let chart = Chart::load(ctx)?;
    let acc = chart.resolve(&a.account)?.clone();
    let ids = if a.include_children {
        chart.subtree(acc.id)
    } else {
        vec![acc.id]
    };
    let commodities: HashSet<String> = ids
        .iter()
        .filter_map(|i| chart.get(*i))
        .map(|x| x.commodity.to_string())
        .collect();
    if commodities.len() > 1 {
        return Err(CommandError::Rejected(format!(
            "{} and its sub-accounts hold several commodities ({}); a register needs one",
            chart.full_name(acc.id),
            commodities.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    let ty = acc.account_type;
    let scale = chart.scale_of(&acc.commodity);
    let fmt = |d: Decimal| format_amount(d, scale);
    let overflow = || CommandError::Rejected("arithmetic overflow".into());
    let splits = ctx.tx.register_splits(&ids, a.to)?;
    // One pass computes every balance; rows keep only an index and their balance, and
    // JSON is built for the returned page only.
    let mut opening = Decimal::ZERO;
    let mut running = Decimal::ZERO;
    let mut cleared = Decimal::ZERO;
    let mut reconciled = Decimal::ZERO;
    let mut visible: Vec<(usize, Decimal, Decimal)> = Vec::new();
    for (i, s) in splits.iter().enumerate() {
        let posted = s.status == TxnStatus::Posted;
        let display = ty.display(s.quantity);
        if posted {
            running = running.checked_add(display).ok_or_else(overflow)?;
            if Basis::Cleared.includes(s.reconcile) {
                cleared = cleared.checked_add(display).ok_or_else(overflow)?;
            }
            if Basis::Reconciled.includes(s.reconcile) {
                reconciled = reconciled.checked_add(display).ok_or_else(overflow)?;
            }
        }
        if a.from.is_some_and(|f| s.date < f) {
            if posted {
                opening = opening.checked_add(display).ok_or_else(overflow)?;
            }
            continue;
        }
        if !posted && !a.include_voided {
            continue;
        }
        visible.push((i, display, running));
    }
    let total_rows = visible.len();
    if a.order == Order::Desc {
        visible.reverse();
    }
    let releases = crate::dimensions::release_links(ctx)?;
    let receipts = crate::expenses::receipt_counts(ctx)?;
    let mut page = Vec::new();
    for (i, display, balance) in visible.into_iter().skip(a.offset).take(a.limit) {
        let Some(s) = splits.get(i) else { continue };
        let other = match (s.other_count, s.other_account) {
            (0, _) => json!({ "kind": "none" }),
            (1, Some(o)) => {
                json!({ "kind": "account", "account": o, "full_name": chart.full_name(o) })
            }
            (n, _) => json!({ "kind": "split", "count": n }),
        };
        let memo = s
            .split_memo
            .clone()
            .filter(|m| !m.trim().is_empty())
            .or_else(|| s.txn_memo.clone());
        page.push(json!({
            "txn": s.txn_id,
            "split": format!("{}:{}", s.txn_id, s.index),
            "account": s.account,
            "date": s.date,
            "number": s.number,
            "payee": s.payee,
            "memo": memo,
            "other": other,
            "amount": fmt(display),
            "increase": display > Decimal::ZERO,
            "reconcile": s.reconcile,
            "status": s.status,
            "balance": fmt(balance),
            "imported": s.imported,
        }));
        if let (Some(e), Some(Value::Object(o))) =
            (releases.get(&s.txn_id.to_string()), page.last_mut())
        {
            o.insert("release_for".into(), json!(e));
        }
        if let (Some(n), Some(Value::Object(o))) =
            (receipts.get(&s.txn_id.to_string()), page.last_mut())
        {
            o.insert("receipts".into(), json!(n));
        }
    }
    Ok(json!({
        "account": {
            "id": acc.id, "full_name": chart.full_name(acc.id), "type": acc.account_type,
            "subtype": acc.subtype, "commodity": acc.commodity,
        },
        "opening_balance": fmt(opening),
        "rows": page,
        "total_rows": total_rows,
        "closing_balance": fmt(running),
        "cleared_balance": fmt(cleared),
        "reconciled_balance": fmt(reconciled),
    }))
}

// ----- payee.suggest -----------------------------------------------------------

fn default_suggest() -> usize {
    10
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PayeeArgs {
    #[serde(default)]
    prefix: String,
    /// Only transactions touching this account (id, full name or code).
    #[serde(default)]
    account: Option<String>,
    /// Default 10, max 50.
    #[serde(default = "default_suggest")]
    limit: usize,
}

fn payee_matches(normalised: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || normalised.starts_with(prefix)
        || normalised.match_indices(prefix).any(|(i, _)| {
            normalised
                .get(..i)
                .and_then(|s| s.chars().last())
                .is_some_and(|c| !c.is_alphanumeric())
        })
}

fn payee_suggest(ctx: &Ctx<'_>, a: PayeeArgs) -> CommandResult<Value> {
    if a.limit > 50 {
        return Err(CommandError::InvalidArgs("limit is at most 50".into()));
    }
    let chart = Chart::load(ctx)?;
    let filter = a
        .account
        .as_deref()
        .map(|r| chart.resolve(r).map(|x| x.id))
        .transpose()?;
    let prefix = normalise_payee(&a.prefix);
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    // With `account`: its own payees first, then payees from other accounts fill the
    // rest (a payee used only on a card is still suggested in checking).
    let mut passes = vec![filter];
    if filter.is_some() {
        passes.push(None);
    }
    for pass in passes {
        for u in ctx.tx.payee_uses(pass)? {
            if out.len() >= a.limit {
                break;
            }
            let norm = normalise_payee(&u.payee);
            if !payee_matches(&norm, &prefix) || !seen.insert(norm) {
                continue;
            }
            let t = ctx
                .tx
                .transaction(u.txn_id)?
                .ok_or_else(|| CommandError::Integrity("payee transaction missing".into()))?;
            out.push(suggestion(&chart, &t, filter, pass.is_some(), &u.payee));
        }
    }
    Ok(Value::Array(out))
}

/// One payee suggestion: its last other-side account and amount, from `account`'s
/// view when the transaction touched it. A transaction from another account (the
/// fallback) is shown as if paid from an asset: category debited means money out.
fn suggestion(
    chart: &Chart,
    t: &mk_core::Transaction,
    filter: Option<AccountId>,
    own: bool,
    payee: &str,
) -> Value {
    let (mut other, mut amount) = (Value::Null, Value::Null);
    if t.splits.len() == 2 {
        let base = if own {
            filter
                .and_then(|f| t.splits.iter().position(|s| s.account == f))
                .unwrap_or(0)
        } else {
            // The money side is the asset/liability split; the other is the category.
            t.splits
                .iter()
                .position(|s| {
                    chart.get(s.account).is_some_and(|a| {
                        matches!(a.account_type, AccountType::Asset | AccountType::Liability)
                    })
                })
                .unwrap_or(0)
        };
        if let (Some(b), Some(o)) = (t.splits.get(base), t.splits.get(1 - base.min(1))) {
            other = json!({ "account": o.account, "full_name": chart.full_name(o.account) });
            if let Some(acc) = chart.get(b.account) {
                let shown = if own {
                    acc.account_type.display(b.quantity)
                } else {
                    -o.value
                };
                amount = json!(format_amount(shown, chart.scale_of(&acc.commodity)));
            }
        }
    }
    json!({
        "payee": payee, "last_txn": t.id, "last_date": t.date, "other": other,
        "amount": amount, "from_account": own || filter.is_none(),
    })
}

// ----- account.suggest ---------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AccountSuggestArgs {
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    types: Option<Vec<AccountType>>,
    #[serde(default = "default_suggest")]
    limit: usize,
}

fn account_suggest(ctx: &Ctx<'_>, a: AccountSuggestArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let last = ctx.tx.account_last_used()?;
    let p = a.prefix.trim().to_lowercase();
    let mut hits: Vec<(bool, Option<&String>, String, AccountId)> = chart
        .accounts
        .iter()
        .filter(|x| !x.placeholder && !x.archived)
        .filter(|x| a.types.as_ref().is_none_or(|t| t.contains(&x.account_type)))
        .filter_map(|x| {
            let full = chart.full_name(x.id);
            let lower = full.to_lowercase();
            let ok = p.is_empty()
                || lower.starts_with(&p)
                || lower.split(':').any(|seg| seg.starts_with(&p));
            ok.then(|| (x.name.to_lowercase() == p, last.get(&x.id), full, x.id))
        })
        .collect();
    hits.sort_by(|x, y| {
        y.0.cmp(&x.0)
            .then_with(|| y.1.cmp(&x.1))
            .then_with(|| x.2.to_lowercase().cmp(&y.2.to_lowercase()))
    });
    let out: Vec<Value> = hits
        .into_iter()
        .take(a.limit.min(50))
        .filter_map(|(_, _, full, id)| {
            chart.get(id).map(|x| json!({ "id": id, "full_name": full, "name": x.name, "type": x.account_type, "subtype": x.subtype }))
        })
        .collect();
    Ok(Value::Array(out))
}
