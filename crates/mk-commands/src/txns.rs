//! Transaction commands (spec: ledger-transactions.md).

use std::collections::HashMap;

use mk_core::currency::minor_units;
use mk_core::{
    AccountId, AccountSubtype, CommodityCode, Date, Decimal, Reconcile, Split, Transaction,
    TransactionId, TxnStatus,
};
use mk_ledger::{
    AccountFacts, TxnRules, canonical_amount, check_archived_untouched, check_closing,
    check_reconciled_preserved, check_transaction, inherit_reconcile, sort_splits,
};
use mk_store::{TxnFilter, now_rfc3339, today_utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::{Chart, MULTI_CURRENCY};
use crate::{CommandError, CommandResult, Ctx, Registry, is_module_enabled};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "txn.create",
        "Record a balanced transaction of two or more splits",
        true,
        create,
    );
    r.add(
        "txn.update",
        "Replace a transaction's fields and full split list",
        true,
        update,
    );
    r.add(
        "txn.void",
        "Void a transaction: kept and visible, excluded from balances",
        true,
        void,
    );
    r.add("txn.unvoid", "Restore a voided transaction", true, unvoid);
    r.add(
        "txn.delete",
        "Delete a transaction (no reconciled splits, not in the closed period)",
        true,
        delete,
    );
    r.add(
        "txn.reverse",
        "Create a reversing transaction that negates every split of another",
        true,
        reverse,
    );
    r.add(
        "txn.set_reconcile",
        "Set the reconcile state (n, c, r) of splits by transaction and split index",
        true,
        set_reconcile,
    );
    r.add("txn.get", "Get one transaction by id", false, get);
    r.add(
        "txn.list",
        "List transactions in register order, with filters",
        false,
        list,
    );
    r.add(
        "book.set_closing_date",
        "Set or clear the closing date (no changes on or before it)",
        true,
        set_closing_date,
    );
}

// ----- input ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SplitInput {
    /// Account id, full name or code.
    pub account: String,
    /// Signed amount in the transaction currency: debit positive, credit negative.
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    pub value: Decimal,
    /// Signed amount in the account's commodity. May be omitted when the account's
    /// commodity is the transaction currency (it then equals value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub quantity: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// Class (id or name; "" for none). Modules classes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// Location (id or name; "" for none). Module locations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Fund (id or name; "" for none = General). Module funds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fund: Option<String>,
    /// Project (id or name; "" for none). Module projects. Without a fund the line
    /// takes the project's fund.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// A billable project cost (default: the project's billable setting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billable: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TxnInput {
    pub date: Date,
    /// Defaults to the book's base currency.
    #[serde(default)]
    pub currency: Option<CommodityCode>,
    #[serde(default)]
    pub payee: Option<String>,
    #[serde(default)]
    pub number: Option<String>,
    #[serde(default)]
    pub memo: Option<String>,
    /// 2 to 1000 splits whose values sum to exactly zero.
    pub splits: Vec<SplitInput>,
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Build a transaction from client input: resolve accounts, fill omitted quantities,
/// canonicalise amounts and split order.
pub(crate) fn build(
    ctx: &Ctx<'_>,
    chart: &Chart,
    input: TxnInput,
    id: TransactionId,
    entered_at: String,
) -> CommandResult<Transaction> {
    let currency = match input.currency {
        Some(c) => c,
        None => ctx.tx.info()?.base_currency.parse()?,
    };
    let value_dp = minor_units(currency.as_str());
    let mut splits = Vec::with_capacity(input.splits.len());
    for (i, s) in input.splits.into_iter().enumerate() {
        let acc = chart
            .resolve(&s.account)
            .map_err(|e| CommandError::InvalidArgs(format!("split {i}: {e}")))?;
        let quantity = match s.quantity {
            Some(q) => q,
            None if acc.commodity == currency => s.value,
            None => {
                return Err(CommandError::InvalidArgs(format!(
                    "split {i}: {} is in {}, not {currency}; give a quantity",
                    chart.full_name(acc.id),
                    acc.commodity
                )));
            }
        };
        // Values are in the transaction currency. Quantities of a currency account are
        // at that currency's minor units; security quantities are not padded.
        let qty_dp = if acc.commodity == currency {
            value_dp
        } else if acc.subtype == AccountSubtype::Security {
            0
        } else {
            minor_units(acc.commodity.as_str())
        };
        splits.push(Split {
            account: acc.id,
            value: canonical_amount(s.value, value_dp),
            quantity: canonical_amount(quantity, qty_dp),
            memo: clean(s.memo),
            reconcile: Reconcile::New,
            fingerprint: None,
            tags: crate::dimensions::resolve_tags_p(
                ctx,
                s.class.as_deref(),
                s.location.as_deref(),
                s.fund.as_deref(),
                s.project.as_deref(),
                s.billable,
            )
            .map_err(|e| CommandError::InvalidArgs(format!("split {i}: {e}")))?,
        });
    }
    sort_splits(&mut splits);
    Ok(Transaction {
        id,
        date: input.date,
        currency,
        payee: clean(input.payee),
        number: clean(input.number),
        memo: clean(input.memo),
        status: TxnStatus::Posted,
        void_reason: None,
        reverses: None,
        opening: false,
        entered_at,
        splits,
    })
}

/// Facts about every account, for the ledger rules.
fn facts(chart: &Chart) -> HashMap<AccountId, AccountFacts> {
    chart
        .accounts
        .iter()
        .map(|a| {
            (
                a.id,
                AccountFacts {
                    full_name: chart.full_name(a.id),
                    commodity: a.commodity.clone(),
                    placeholder: a.placeholder,
                    archived: a.archived,
                    is_security: a.subtype == AccountSubtype::Security,
                },
            )
        })
        .collect()
}

pub(crate) fn rules(ctx: &Ctx<'_>) -> CommandResult<TxnRules> {
    Ok(TxnRules {
        base_currency: ctx.tx.info()?.base_currency.parse()?,
        multi_currency: is_module_enabled(ctx.tx, MULTI_CURRENCY)?,
    })
}

/// Every rule a change from `old` to `new` must satisfy (either may be absent:
/// creation or deletion). Shared by all transaction commands and by undo/redo.
pub(crate) fn validate_change(
    ctx: &Ctx<'_>,
    old: Option<&Transaction>,
    new: Option<&Transaction>,
) -> CommandResult<()> {
    check_closing(ctx.tx.closing_date()?, old, new)?;
    if let (Some(o), Some(n)) = (old, new)
        && reconcile_only(o, n)
    {
        // Same rules as txn.set_reconcile: allowed outside the closed period.
        return Ok(());
    }
    let chart = Chart::load(ctx)?;
    let facts = facts(&chart);
    if let Some(n) = new {
        // Void/unvoid do not touch currency or splits, so turning multi-currency off
        // hides rather than freezes existing foreign entries.
        let gate = !ctx.restoring.get()
            && old.is_none_or(|o| o.currency != n.currency || o.splits != n.splits);
        check_transaction(n, &facts, &rules(ctx)?, gate)?;
        if !ctx.restoring.get() && old.is_none_or(|o| o.splits != n.splits || o.status != n.status)
        {
            crate::dimensions::check_required(ctx, n)?;
        }
    }
    check_archived_untouched(old, new, &facts)?;
    if !ctx.unreconciling.get() {
        check_reconciled_preserved(old, new)?;
    }
    Ok(())
}

fn reconcile_only(o: &Transaction, n: &Transaction) -> bool {
    mk_ledger::only_reconcile_differs(o, n)
}

fn load(ctx: &Ctx<'_>, id: TransactionId) -> CommandResult<Transaction> {
    ctx.tx
        .transaction(id)?
        .ok_or_else(|| CommandError::NotFound(format!("transaction {id}")))
}

/// Transaction as returned to clients, with account full names on splits.
pub(crate) fn txn_json(chart: &Chart, t: &Transaction) -> CommandResult<Value> {
    let mut v = serde_json::to_value(t)?;
    if let Some(splits) = v.get_mut("splits").and_then(Value::as_array_mut) {
        for (s, orig) in splits.iter_mut().zip(&t.splits) {
            if let Some(o) = s.as_object_mut() {
                o.insert("account_name".into(), json!(chart.full_name(orig.account)));
            }
        }
    }
    Ok(v)
}

fn respond(ctx: &Ctx<'_>, t: &Transaction) -> CommandResult<Value> {
    let mut v = txn_json(&Chart::load(ctx)?, t)?;
    if let Some(splits) = v.get_mut("splits").and_then(Value::as_array_mut) {
        for (s, orig) in splits.iter_mut().zip(&t.splits) {
            if orig.tags.is_empty() {
                continue;
            }
            let names = crate::dimensions::tag_names(ctx, &orig.tags)?;
            if let Some(o) = s.as_object_mut() {
                o.insert("tag_names".into(), names);
            }
        }
    }
    if let Some(expense) = crate::dimensions::release_of(ctx, t.id)?
        && let Some(o) = v.as_object_mut()
    {
        o.insert("release_for".into(), json!(expense));
    }
    Ok(v)
}

// ----- commands ------------------------------------------------------------

fn create(ctx: &Ctx<'_>, input: TxnInput) -> CommandResult<Value> {
    let t = create_txn(ctx, input)?;
    respond(ctx, &t)
}

/// Build, validate and save a new transaction (shared with `schedule.enter`).
pub(crate) fn create_txn(ctx: &Ctx<'_>, input: TxnInput) -> CommandResult<Transaction> {
    let chart = Chart::load(ctx)?;
    let t = build(ctx, &chart, input, TransactionId::new(), now_rfc3339()?)?;
    validate_change(ctx, None, Some(&t))?;
    ctx.save_txn(None, &t)?;
    Ok(t)
}

/// The rules a schedule template must satisfy when saved: everything a new
/// transaction must satisfy except the closing date (it has no real date yet).
pub(crate) fn check_template(ctx: &Ctx<'_>, input: TxnInput) -> CommandResult<Transaction> {
    let chart = Chart::load(ctx)?;
    let t = build(ctx, &chart, input, TransactionId::new(), String::new())?;
    let facts = facts(&chart);
    check_transaction(&t, &facts, &rules(ctx)?, true)?;
    check_archived_untouched(None, Some(&t), &facts)?;
    Ok(t)
}

/// `txn.update` arguments: the transaction id plus the full new content.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    id: TransactionId,
    date: Date,
    /// Defaults to the book's base currency.
    #[serde(default)]
    currency: Option<CommodityCode>,
    #[serde(default)]
    payee: Option<String>,
    #[serde(default)]
    number: Option<String>,
    #[serde(default)]
    memo: Option<String>,
    /// The complete new split list (2 to 1000 splits summing to zero). Reconcile states
    /// carry over to matching splits; new splits start at `n`.
    splits: Vec<SplitInput>,
}

fn update(ctx: &Ctx<'_>, a: UpdateArgs) -> CommandResult<Value> {
    let old = load(ctx, a.id)?;
    let chart = Chart::load(ctx)?;
    // Tags not given keep the old split's tags (same account, in order).
    let mut used = vec![false; old.splits.len()];
    let mut splits = a.splits;
    for s in &mut splits {
        let Ok(acc) = chart.resolve(&s.account) else {
            continue;
        };
        let Some(k) = old
            .splits
            .iter()
            .enumerate()
            .position(|(k, o)| o.account == acc.id && !used.get(k).copied().unwrap_or(true))
        else {
            continue;
        };
        if let (Some(slot), Some(o)) = (used.get_mut(k), old.splits.get(k)) {
            *slot = true;
            if s.class.is_none() {
                s.class.clone_from(&o.tags.class);
            }
            if s.location.is_none() {
                s.location.clone_from(&o.tags.location);
            }
            if s.fund.is_none() {
                s.fund.clone_from(&o.tags.fund);
            }
            if s.project.is_none() {
                s.project.clone_from(&o.tags.project);
                if s.billable.is_none() && o.tags.project.is_some() {
                    s.billable = Some(o.tags.billable);
                }
            }
        }
    }
    let input = TxnInput {
        date: a.date,
        currency: a.currency,
        payee: a.payee,
        number: a.number,
        memo: a.memo,
        splits,
    };
    let mut new = build(ctx, &chart, input, old.id, old.entered_at.clone())?;
    new.status = old.status;
    new.void_reason = old.void_reason.clone();
    new.reverses = old.reverses;
    // opening-balances.md: edits keep the opening mark.
    new.opening = old.opening;
    inherit_reconcile(&old.splits, &mut new.splits);
    crate::investments::check_plain_update(ctx, &old, &new)?;
    validate_change(ctx, Some(&old), Some(&new))?;
    if new != old {
        ctx.save_txn(Some(&old), &new)?;
    }
    respond(ctx, &new)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct VoidArgs {
    id: TransactionId,
    #[serde(default)]
    reason: Option<String>,
}

fn void(ctx: &Ctx<'_>, a: VoidArgs) -> CommandResult<Value> {
    let old = load(ctx, a.id)?;
    if old.status == TxnStatus::Voided {
        return Err(CommandError::Rejected(
            "the transaction is already voided".into(),
        ));
    }
    let mut new = old.clone();
    new.status = TxnStatus::Voided;
    new.void_reason = clean(a.reason);
    validate_change(ctx, Some(&old), Some(&new))?;
    ctx.save_txn(Some(&old), &new)?;
    crate::investments::after_plain_change(ctx, &old, false)?;
    respond(ctx, &new)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IdArgs {
    id: TransactionId,
}

fn unvoid(ctx: &Ctx<'_>, a: IdArgs) -> CommandResult<Value> {
    let old = load(ctx, a.id)?;
    if old.status == TxnStatus::Posted {
        return Err(CommandError::Rejected(
            "the transaction is not voided".into(),
        ));
    }
    let mut new = old.clone();
    new.status = TxnStatus::Posted;
    new.void_reason = None;
    validate_change(ctx, Some(&old), Some(&new))?;
    ctx.save_txn(Some(&old), &new)?;
    crate::investments::after_plain_change(ctx, &old, false)?;
    respond(ctx, &new)
}

fn delete(ctx: &Ctx<'_>, a: IdArgs) -> CommandResult<Value> {
    let old = load(ctx, a.id)?;
    if old
        .splits
        .iter()
        .any(|s| s.reconcile == Reconcile::Reconciled)
    {
        return Err(CommandError::Rejected(
            "the transaction has reconciled splits and cannot be deleted".into(),
        ));
    }
    validate_change(ctx, Some(&old), None)?;
    let chart = Chart::load(ctx)?;
    let content = txn_json(&chart, &old)?;
    remove_with_effects(ctx, &old)?;
    Ok(json!({ "deleted": old.id, "transaction": content }))
}

/// Remove a transaction (already validated) with everything a deletion does to
/// what refers to it. Shared with the opening balance wizard's statement start.
pub(crate) fn remove_with_effects(ctx: &Ctx<'_>, old: &Transaction) -> CommandResult<()> {
    ctx.remove_txn(old)?;
    crate::investments::after_plain_change(ctx, old, true)?;
    // An occurrence entered as this transaction becomes pending again.
    crate::schedules::unlink_txn(ctx, old.id)?;
    // Its receipts go to the inbox; an expense posted or matched to it is unlinked.
    crate::expenses::after_txn_delete(ctx, old.id)?;
    // A trip posted to it becomes ready again (mileage.md).
    crate::mileage::after_txn_delete(ctx, old.id)?;
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReverseArgs {
    id: TransactionId,
    /// Date of the reversing transaction; defaults to today.
    #[serde(default)]
    date: Option<Date>,
}

fn reverse(ctx: &Ctx<'_>, a: ReverseArgs) -> CommandResult<Value> {
    let orig = load(ctx, a.id)?;
    crate::investments::refuse_plain(ctx, orig.id)?;
    let date = match a.date {
        Some(d) => d,
        None => today_utc()?,
    };
    let label = orig.number.clone().unwrap_or_else(|| orig.date.to_string());
    let mut splits: Vec<Split> = orig
        .splits
        .iter()
        .map(|s| Split {
            account: s.account,
            value: -s.value,
            quantity: -s.quantity,
            memo: s.memo.clone(),
            reconcile: Reconcile::New,
            fingerprint: None,
            tags: s.tags.clone(),
        })
        .collect();
    sort_splits(&mut splits);
    let t = Transaction {
        id: TransactionId::new(),
        date,
        currency: orig.currency.clone(),
        payee: orig.payee.clone(),
        number: None,
        memo: Some(format!("Reversal of {label}")),
        status: TxnStatus::Posted,
        void_reason: None,
        reverses: Some(orig.id),
        opening: false,
        entered_at: now_rfc3339()?,
        splits,
    };
    validate_change(ctx, None, Some(&t))?;
    ctx.save_txn(None, &t)?;
    respond(ctx, &t)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReconcileItem {
    /// Transaction id.
    id: TransactionId,
    /// Index of the split in the transaction's `splits` list (0-based).
    split: usize,
    state: Reconcile,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetReconcileArgs {
    /// One or more splits to set.
    splits: Vec<ReconcileItem>,
}

fn set_reconcile(ctx: &Ctx<'_>, a: SetReconcileArgs) -> CommandResult<Value> {
    if a.splits.is_empty() {
        return Err(CommandError::InvalidArgs("give at least one split".into()));
    }
    let mut work: Vec<(Transaction, Transaction)> = Vec::new();
    for item in a.splits {
        let pos = match work.iter().position(|(o, _)| o.id == item.id) {
            Some(p) => p,
            None => {
                let t = load(ctx, item.id)?;
                work.push((t.clone(), t));
                work.len() - 1
            }
        };
        let Some((_, new)) = work.get_mut(pos) else {
            continue;
        };
        let n = new.splits.len();
        let split = new.splits.get_mut(item.split).ok_or_else(|| {
            CommandError::InvalidArgs(format!(
                "transaction {} has {n} splits; there is no split {}",
                item.id, item.split
            ))
        })?;
        split.reconcile = item.state;
    }
    let mut out = Vec::new();
    for (old, new) in &work {
        validate_change(ctx, Some(old), Some(new))?;
        if new != old {
            ctx.save_txn(Some(old), new)?;
        }
        out.push(respond(ctx, new)?);
    }
    Ok(Value::Array(out))
}

fn get(ctx: &Ctx<'_>, a: IdArgs) -> CommandResult<Value> {
    let mut v = respond(ctx, &load(ctx, a.id)?)?;
    if let (Some((schedule, occurrence)), Some(o)) =
        (crate::schedules::link_of(ctx, a.id)?, v.as_object_mut())
    {
        o.insert("schedule".into(), json!(schedule));
        o.insert("occurrence".into(), json!(occurrence));
    }
    Ok(v)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    /// Only transactions touching this account (id, full name or code).
    #[serde(default)]
    account: Option<String>,
    /// With `account`, also include its sub-accounts. Default true.
    #[serde(default = "yes")]
    include_children: bool,
    #[serde(default)]
    from: Option<Date>,
    #[serde(default)]
    to: Option<Date>,
    #[serde(default)]
    include_voided: bool,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: u32,
}

fn yes() -> bool {
    true
}

fn list(ctx: &Ctx<'_>, a: ListArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let accounts = match &a.account {
        None => None,
        Some(r) => {
            let acc = chart.resolve(r)?;
            Some(if a.include_children {
                chart.subtree(acc.id)
            } else {
                vec![acc.id]
            })
        }
    };
    let txns = ctx.tx.transactions(&TxnFilter {
        accounts,
        from: a.from,
        to: a.to,
        include_voided: a.include_voided,
        limit: a.limit,
        offset: a.offset,
    })?;
    let out = txns
        .iter()
        .map(|t| txn_json(&chart, t))
        .collect::<CommandResult<Vec<_>>>()?;
    Ok(Value::Array(out))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClosingArgs {
    /// New closing date, or null to clear it.
    pub date: Option<Date>,
}

fn set_closing_date(ctx: &Ctx<'_>, a: ClosingArgs) -> CommandResult<Value> {
    let before = ctx.tx.closing_date()?;
    ctx.save_closing_date(a.date)?;
    Ok(json!({ "closing_date": a.date, "previous": before }))
}
