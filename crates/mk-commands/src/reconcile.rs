//! Statement reconciliation (spec: reconciliation.md).
//!
//! A session is a document in the `reconciliation` table (group = account id). Its
//! status moves `open` → `finished` or `cancelled`. Splits are identified as
//! `<transaction id>:<split index>`.

use mk_core::{
    AccountId, AccountType, Date, Decimal, Reconcile, Split, Transaction, TransactionId, TxnStatus,
};
use mk_ledger::sort_splits;
use mk_store::{Doc, DocTable, now_rfc3339};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::balances::raw_balances;
use crate::txns::validate_change;
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "reconcile.start",
        "Start reconciling an account against a statement (date and balance, or a captured statement)",
        true,
        start,
    );
    r.add(
        "reconcile.get",
        "Show the open reconciliation of an account: candidates, selection, difference",
        false,
        get,
    );
    r.add(
        "reconcile.select",
        "Tick splits in the open reconciliation",
        true,
        select,
    );
    r.add(
        "reconcile.unselect",
        "Untick splits in the open reconciliation",
        true,
        unselect,
    );
    r.add(
        "reconcile.finish",
        "Finish the reconciliation: mark selected splits reconciled (difference must be zero, or adjust)",
        true,
        finish,
    );
    r.add(
        "reconcile.cancel",
        "Discard the open reconciliation; reconcile flags are untouched",
        true,
        cancel,
    );
    r.add(
        "reconcile.history",
        "List finished reconciliations of an account, newest first",
        false,
        history,
    );
    r.add(
        "reconcile.undo_last",
        "Undo the most recent finished reconciliation of an account",
        true,
        undo_last,
    );
}

/// Commands whose effect `history.undo` refuses to reverse.
pub(crate) const NOT_UNDOABLE: &[&str] = &[
    "reconcile.start",
    "reconcile.select",
    "reconcile.unselect",
    "reconcile.cancel",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Session {
    pub id: String,
    pub account: AccountId,
    pub statement_date: Date,
    /// Display sign.
    pub statement_balance: Decimal,
    /// Display sign.
    pub opening_balance: Decimal,
    pub status: String,
    pub selected: Vec<String>,
    pub statement: Option<String>,
    pub started_at: String,
    pub actor: String,
    pub finished_at: Option<String>,
    pub finished_by: Option<String>,
    /// Flags the selected splits had before finishing, by split id.
    pub prior_flags: Vec<(String, Reconcile)>,
    pub adjustment: Option<TransactionId>,
    /// Order among this account's finished reconciliations (1, 2, …).
    pub sequence: Option<i64>,
    /// A later change (an opening balance's "Go ahead") made this period not add up:
    /// shown as "to check" (opening-balances.md).
    #[serde(default, skip_serializing_if = "mk_core::ledger::is_false")]
    pub to_check: bool,
}

fn split_id(t: TransactionId, i: usize) -> String {
    format!("{t}:{i}")
}

fn parse_split_id(s: &str) -> CommandResult<(TransactionId, usize)> {
    let bad = || {
        CommandError::InvalidArgs(format!(
            "invalid split id {s:?}; expected <transaction id>:<index>"
        ))
    };
    let (t, i) = s.rsplit_once(':').ok_or_else(bad)?;
    Ok((t.parse().map_err(|_| bad())?, i.parse().map_err(|_| bad())?))
}

pub(crate) fn sessions(ctx: &Ctx<'_>, account: AccountId) -> CommandResult<Vec<Session>> {
    ctx.tx
        .docs(DocTable::Reconciliation, Some(&account.to_string()))?
        .iter()
        .map(|d| {
            serde_json::from_value(d.json.clone())
                .map_err(|e| CommandError::Integrity(e.to_string()))
        })
        .collect()
}

pub(crate) fn save(ctx: &Ctx<'_>, s: &Session) -> CommandResult<()> {
    let ord = ctx
        .tx
        .get_doc(DocTable::Reconciliation, &s.id)?
        .map_or_else(
            || {
                i64::try_from(sessions(ctx, s.account).map(|v| v.len()).unwrap_or(0) + 1)
                    .unwrap_or(i64::MAX)
            },
            |d| d.ord,
        );
    let doc = Doc {
        id: s.id.clone(),
        grp: Some(s.account.to_string()),
        ord,
        json: serde_json::to_value(s)?,
    };
    ctx.save_doc(DocTable::Reconciliation, &s.id, Some(&doc))
}

fn open_session(ctx: &Ctx<'_>, account: AccountId) -> CommandResult<Session> {
    sessions(ctx, account)?
        .into_iter()
        .find(|s| s.status == "open")
        .ok_or_else(|| {
            CommandError::Rejected("there is no open reconciliation for this account".into())
        })
}

fn account_of(ctx: &Ctx<'_>, r: &str) -> CommandResult<(Chart, AccountId, AccountType)> {
    let chart = Chart::load(ctx)?;
    let acc = chart.resolve(r)?;
    if acc.placeholder
        || !matches!(
            acc.account_type,
            AccountType::Asset | AccountType::Liability
        )
    {
        return Err(CommandError::Rejected(format!(
            "{} cannot be reconciled: only non-placeholder asset or liability accounts can",
            chart.full_name(acc.id)
        )));
    }
    let (id, ty) = (acc.id, acc.account_type);
    Ok((chart, id, ty))
}

struct Candidate {
    id: String,
    date: Date,
    payee: Option<String>,
    quantity: Decimal,
    reconcile: Reconcile,
}

fn candidates(ctx: &Ctx<'_>, account: AccountId, through: Date) -> CommandResult<Vec<Candidate>> {
    Ok(ctx
        .tx
        .account_splits(account, None, Some(through))?
        .into_iter()
        .filter(|s| s.reconcile != Reconcile::Reconciled)
        .map(|s| Candidate {
            id: split_id(s.txn_id, s.index),
            date: s.date,
            payee: s.payee,
            quantity: s.quantity,
            reconcile: s.reconcile,
        })
        .collect())
}

fn difference(ctx: &Ctx<'_>, s: &Session, ty: AccountType) -> CommandResult<Decimal> {
    let overflow = || CommandError::Rejected("arithmetic overflow".into());
    let cands = candidates(ctx, s.account, s.statement_date)?;
    let mut sum = Decimal::ZERO;
    for c in cands.iter().filter(|c| s.selected.contains(&c.id)) {
        sum = sum
            .checked_add(ty.display(c.quantity))
            .ok_or_else(overflow)?;
    }
    s.statement_balance
        .checked_sub(s.opening_balance.checked_add(sum).ok_or_else(overflow)?)
        .ok_or_else(overflow)
}

fn session_json(ctx: &Ctx<'_>, s: &Session) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let acc = chart
        .get(s.account)
        .ok_or_else(|| CommandError::NotFound("account".into()))?;
    let ty = acc.account_type;
    let dp = Some(mk_core::currency::minor_units(acc.commodity.as_str()));
    let fmt = |d: Decimal| mk_core::currency::format_amount(d, dp);
    let mut v = serde_json::to_value(s)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("full_name".into(), json!(chart.full_name(s.account)));
        o.insert("statement_balance".into(), json!(fmt(s.statement_balance)));
        o.insert("opening_balance".into(), json!(fmt(s.opening_balance)));
        if s.status == "open" {
            let cands: Vec<Value> = candidates(ctx, s.account, s.statement_date)?
                .iter()
                .map(|c| {
                    json!({
                        "split": c.id, "date": c.date, "payee": c.payee,
                        "amount": fmt(ty.display(c.quantity)), "reconcile": c.reconcile,
                        "selected": s.selected.contains(&c.id),
                    })
                })
                .collect();
            o.insert("candidates".into(), Value::Array(cands));
            o.insert("difference".into(), json!(fmt(difference(ctx, s, ty)?)));
        }
    }
    Ok(v)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StartArgs {
    /// Account id, full name or code.
    account: String,
    #[serde(default)]
    statement_date: Option<Date>,
    /// Closing balance in display sign (a card with 812.40 owed is 812.40).
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    statement_balance: Option<Decimal>,
    /// A statement captured by import (fills date and balance).
    #[serde(default)]
    statement: Option<String>,
}

fn start(ctx: &Ctx<'_>, a: StartArgs) -> CommandResult<Value> {
    let (_, account, ty) = account_of(ctx, &a.account)?;
    if sessions(ctx, account)?.iter().any(|s| s.status == "open") {
        return Err(CommandError::Rejected(
            "this account already has an open reconciliation; finish or cancel it first".into(),
        ));
    }
    let (date, balance) = match (&a.statement, a.statement_date, a.statement_balance) {
        (Some(sid), None, None) => {
            let d = ctx
                .tx
                .get_doc(DocTable::Statement, sid)?
                .ok_or_else(|| CommandError::NotFound(format!("statement {sid}")))?;
            if d.grp.as_deref() != Some(account.to_string().as_str()) {
                return Err(CommandError::Rejected(
                    "that statement belongs to another account".into(),
                ));
            }
            let date: Date =
                serde_json::from_value(d.json.get("date").cloned().unwrap_or(Value::Null))?;
            let raw: Decimal =
                serde_json::from_value(d.json.get("balance").cloned().unwrap_or(Value::Null))?;
            // Captured balances are from the account holder's view, which for the
            // stored debit-positive convention equals the raw balance.
            (date, ty.display(raw))
        }
        (None, Some(d), Some(b)) => (d, b),
        _ => {
            return Err(CommandError::InvalidArgs(
                "give statement_date and statement_balance, or a captured statement id".into(),
            ));
        }
    };
    let reconciled = raw_balances(ctx, &[account], None, mk_core::Basis::Reconciled)?;
    let commodity = Chart::load(ctx)?.get(account).map(|a| a.commodity.clone());
    let opening_raw = commodity
        .and_then(|c| reconciled.get(&c).copied())
        .unwrap_or(Decimal::ZERO);
    let selected = candidates(ctx, account, date)?
        .into_iter()
        .filter(|c| c.reconcile == Reconcile::Cleared)
        .map(|c| c.id)
        .collect();
    let s = Session {
        id: mk_core::Id::<()>::new().to_string(),
        account,
        statement_date: date,
        statement_balance: balance,
        opening_balance: ty.display(opening_raw),
        status: "open".into(),
        selected,
        statement: a.statement,
        started_at: now_rfc3339()?,
        actor: ctx.actor.0.clone(),
        finished_at: None,
        finished_by: None,
        prior_flags: Vec::new(),
        adjustment: None,
        sequence: None,
        to_check: false,
    };
    save(ctx, &s)?;
    session_json(ctx, &s)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AccountArgs {
    /// Account id, full name or code.
    account: String,
}

fn get(ctx: &Ctx<'_>, a: AccountArgs) -> CommandResult<Value> {
    let (_, account, _) = account_of(ctx, &a.account)?;
    session_json(ctx, &open_session(ctx, account)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SelectArgs {
    account: String,
    /// Split ids (`<transaction id>:<index>`).
    splits: Vec<String>,
}

fn select(ctx: &Ctx<'_>, a: SelectArgs) -> CommandResult<Value> {
    let (_, account, _) = account_of(ctx, &a.account)?;
    let mut s = open_session(ctx, account)?;
    let ids: Vec<String> = candidates(ctx, account, s.statement_date)?
        .into_iter()
        .map(|c| c.id)
        .collect();
    for sp in &a.splits {
        parse_split_id(sp)?;
        if !ids.contains(sp) {
            return Err(CommandError::Rejected(format!(
                "split {sp} is not a candidate (it is reconciled, voided, in another account or dated after the statement)"
            )));
        }
        if !s.selected.contains(sp) {
            s.selected.push(sp.clone());
        }
    }
    save(ctx, &s)?;
    session_json(ctx, &s)
}

fn unselect(ctx: &Ctx<'_>, a: SelectArgs) -> CommandResult<Value> {
    let (_, account, _) = account_of(ctx, &a.account)?;
    let mut s = open_session(ctx, account)?;
    s.selected.retain(|x| !a.splits.contains(x));
    save(ctx, &s)?;
    session_json(ctx, &s)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FinishArgs {
    account: String,
    /// Create a balancing transaction for a non-zero difference.
    #[serde(default)]
    adjust: bool,
    /// The other side of the adjustment (required with adjust).
    #[serde(default)]
    adjustment_account: Option<String>,
}

fn finish(ctx: &Ctx<'_>, a: FinishArgs) -> CommandResult<Value> {
    let (chart, account, ty) = account_of(ctx, &a.account)?;
    let mut s = open_session(ctx, account)?;
    let closing = ctx.tx.closing_date()?;
    let mut by_txn: Vec<(TransactionId, Vec<usize>)> = Vec::new();
    for sp in &s.selected {
        let (t, i) = parse_split_id(sp)?;
        match by_txn.iter_mut().find(|(x, _)| *x == t) {
            Some((_, v)) => v.push(i),
            None => by_txn.push((t, vec![i])),
        }
    }
    let mut changes: Vec<(Transaction, Transaction)> = Vec::new();
    let mut prior = Vec::new();
    for (t, idxs) in &by_txn {
        let old = ctx.tx.transaction(*t)?.ok_or_else(|| {
            CommandError::Rejected(format!(
                "selected transaction {t} no longer exists; unselect it"
            ))
        })?;
        if old.date > s.statement_date {
            return Err(CommandError::Rejected(format!(
                "transaction {t} is dated after the statement date"
            )));
        }
        if closing.is_some_and(|c| old.date <= c) {
            return Err(CommandError::Rejected(format!(
                "transaction {t} is in the closed period"
            )));
        }
        if old.status != TxnStatus::Posted {
            return Err(CommandError::Rejected(format!(
                "transaction {t} is voided; unselect it"
            )));
        }
        let mut new = old.clone();
        for i in idxs {
            let sp = new
                .splits
                .get_mut(*i)
                .filter(|x| x.account == account)
                .ok_or_else(|| {
                    CommandError::Rejected(format!("split {t}:{i} changed; unselect it"))
                })?;
            prior.push((split_id(*t, *i), sp.reconcile));
            sp.reconcile = Reconcile::Reconciled;
        }
        changes.push((old, new));
    }
    let diff = difference(ctx, &s, ty)?;
    if !diff.is_zero() {
        if !a.adjust {
            let dp = chart
                .get(account)
                .map(|x| mk_core::currency::minor_units(x.commodity.as_str()));
            return Err(CommandError::Rejected(format!(
                "the difference is {}; it must be zero to finish (or pass adjust: true with an adjustment_account)",
                mk_core::currency::format_amount(diff, dp)
            )));
        }
        let other = a
            .adjustment_account
            .as_deref()
            .ok_or_else(|| CommandError::InvalidArgs("adjust needs adjustment_account".into()))?;
        let other = chart.resolve(other)?.id;
        if other == account {
            return Err(CommandError::InvalidArgs(
                "the adjustment account must differ from the account".into(),
            ));
        }
        let currency = chart
            .get(account)
            .map(|x| x.commodity.clone())
            .ok_or_else(|| CommandError::NotFound("account".into()))?;
        let raw = ty.display(diff);
        let mut splits = vec![
            Split {
                account,
                value: raw,
                quantity: raw,
                memo: None,
                reconcile: Reconcile::Reconciled,
                fingerprint: None,
                tags: Default::default(),
            },
            Split {
                account: other,
                value: -raw,
                quantity: -raw,
                memo: None,
                reconcile: Reconcile::New,
                fingerprint: None,
                tags: Default::default(),
            },
        ];
        sort_splits(&mut splits);
        let t = Transaction {
            id: TransactionId::new(),
            date: s.statement_date,
            currency,
            payee: None,
            number: None,
            memo: Some("Reconciliation adjustment".into()),
            status: TxnStatus::Posted,
            void_reason: None,
            reverses: None,
            opening: false,
            entered_at: now_rfc3339()?,
            splits,
        };
        validate_change(ctx, None, Some(&t))?;
        ctx.save_txn(None, &t)?;
        s.adjustment = Some(t.id);
    }
    for (old, new) in &changes {
        validate_change(ctx, Some(old), Some(new))?;
        ctx.save_txn(Some(old), new)?;
    }
    let finished = sessions(ctx, account)?
        .iter()
        .filter(|x| x.status == "finished")
        .count();
    s.status = "finished".into();
    s.prior_flags = prior;
    s.finished_at = Some(now_rfc3339()?);
    s.finished_by = Some(ctx.actor.0.clone());
    s.sequence = Some(i64::try_from(finished + 1).unwrap_or(i64::MAX));
    save(ctx, &s)?;
    let mut v = session_json(ctx, &s)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("reconciliation".into(), json!(s.id));
    }
    Ok(v)
}

fn cancel(ctx: &Ctx<'_>, a: AccountArgs) -> CommandResult<Value> {
    let (_, account, _) = account_of(ctx, &a.account)?;
    let mut s = open_session(ctx, account)?;
    s.status = "cancelled".into();
    save(ctx, &s)?;
    Ok(json!({ "cancelled": s.id }))
}

fn history(ctx: &Ctx<'_>, a: AccountArgs) -> CommandResult<Value> {
    let (_, account, _) = account_of(ctx, &a.account)?;
    let mut done: Vec<Session> = sessions(ctx, account)?
        .into_iter()
        .filter(|s| s.status == "finished")
        .collect();
    done.sort_by_key(|s| std::cmp::Reverse(s.sequence));
    let out = done
        .iter()
        .map(|s| session_json(ctx, s))
        .collect::<CommandResult<Vec<_>>>()?;
    Ok(Value::Array(out))
}

/// The newest finished reconciliation of `account`.
pub(crate) fn latest_finished(ctx: &Ctx<'_>, account: AccountId) -> CommandResult<Option<Session>> {
    Ok(sessions(ctx, account)?
        .into_iter()
        .filter(|s| s.status == "finished")
        .max_by_key(|s| s.sequence))
}

/// Check that the reconciliation finished by an audit entry is still the newest for
/// its account (older ones cannot be undone while a newer one exists).
pub(crate) fn check_undo_finish(ctx: &Ctx<'_>, result: &Value) -> CommandResult<()> {
    let id = result
        .get("reconciliation")
        .and_then(Value::as_str)
        .unwrap_or("");
    let account: AccountId = result
        .get("account")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| {
            CommandError::Integrity("reconcile.finish entry without an account".into())
        })?;
    match latest_finished(ctx, account)? {
        Some(l) if l.id == id => Ok(()),
        _ => Err(CommandError::Rejected(
            "a newer reconciliation of this account exists; undo that one first".into(),
        )),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UndoArgs {
    account: String,
    /// A specific reconciliation id; rejected unless it is the newest.
    #[serde(default)]
    reconciliation: Option<String>,
}

fn undo_last(ctx: &Ctx<'_>, a: UndoArgs) -> CommandResult<Value> {
    let (_, account, _) = account_of(ctx, &a.account)?;
    let latest = latest_finished(ctx, account)?.ok_or_else(|| {
        CommandError::Rejected("this account has no finished reconciliation to undo".into())
    })?;
    if let Some(want) = &a.reconciliation
        && *want != latest.id
    {
        return Err(CommandError::Rejected(
            "only the most recent reconciliation of an account can be undone".into(),
        ));
    }
    let entry = ctx
        .tx
        .audit_list(None, u32::MAX)?
        .into_iter()
        .find(|e| {
            e.command == "reconcile.finish"
                && e.result.get("reconciliation").and_then(Value::as_str)
                    == Some(latest.id.as_str())
                && !e.undone
        })
        .ok_or_else(|| CommandError::Integrity("no audit entry for that reconciliation".into()))?;
    crate::history::undo_entry(ctx, &entry)?;
    Ok(json!({ "undone": latest.id, "account": account }))
}
