//! Audit history, undo and redo (spec: ledger-audit-undo.md).
//!
//! Undo is a compensating command: it computes the inverse of an audit entry from its
//! `before`/`after` snapshots, runs the normal validations on it, applies it, and
//! writes its own audit entry with `undoes` set. Audit rows are never changed.

use std::collections::{HashMap, HashSet};

use mk_core::{Account, AccountId, Date, Transaction, TransactionId};
use mk_store::{AuditEntry, Doc};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::check_account_transition;
use crate::builtin::{NoArgs, check_module_consistency, known_module};
use crate::changes::{Kind, Snapshot, parse_snapshots, setting_id};
use crate::modules::CORE;
use crate::txns::validate_change;
use crate::{CommandError, CommandResult, Ctx, ENABLED_KEY, Links, Registry, is_module_enabled};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "history.undo",
        "Undo your most recent change that has not been undone",
        true,
        undo,
    );
    r.add(
        "history.redo",
        "Redo your most recently undone change",
        true,
        redo,
    );
    r.add(
        "history.list",
        "List audit-log entries, newest first, with before/after snapshots and an undone flag",
        false,
        list,
    );
    r.add(
        "audit.list",
        "List recent audit-log entries, newest first (alias of history.list)",
        false,
        list,
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    /// Maximum entries (default 50, max 1000).
    #[serde(default = "default_limit")]
    limit: u32,
    /// Only entries by this actor.
    #[serde(default)]
    actor: Option<String>,
}

fn default_limit() -> u32 {
    50
}

fn list(ctx: &Ctx<'_>, a: ListArgs) -> CommandResult<Value> {
    Ok(serde_json::to_value(
        ctx.tx.audit_list(a.actor.as_deref(), a.limit.min(1000))?,
    )?)
}

/// The entry `history.undo` would reverse for this actor.
/// Session records (`book.encrypted`, `book.password_changed`, …) are not
/// undoable changes; undo and redo pass over them.
fn is_session_record(e: &AuditEntry) -> bool {
    matches!(
        e.command.as_str(),
        "book.create"
            | "book.encrypted"
            | "book.password_changed"
            | "book.recovery_key_changed"
            | "book.decrypted"
            | crate::ai_access::CHANGED_RECORD
            | crate::ai_access::SESSION_ALLOWED_RECORD
    )
}

fn undo_target(entries: &[AuditEntry]) -> Option<&AuditEntry> {
    entries
        .iter()
        .filter(|e| !is_session_record(e))
        .find(|e| e.undoes.is_none() && e.undone_by.is_none())
}

/// The undo entry `history.redo` would re-apply for this actor.
fn redo_target(entries: &[AuditEntry]) -> Option<&AuditEntry> {
    let mut redone: HashSet<i64> = HashSet::new();
    for e in entries.iter().filter(|e| !is_session_record(e)) {
        if let Some(u) = e.redoes {
            redone.insert(u);
            continue;
        }
        if e.undoes.is_some() {
            if redone.contains(&e.seq) {
                continue;
            }
            return Some(e);
        }
        // An ordinary change by this actor since the undo: nothing to redo.
        return None;
    }
    None
}

fn undo(ctx: &Ctx<'_>, _: NoArgs) -> CommandResult<Value> {
    let entries = ctx.tx.audit_entries_by(&ctx.actor.0)?;
    let target = undo_target(&entries)
        .ok_or_else(|| CommandError::Rejected("nothing to undo".into()))?
        .clone();
    if target.before.is_none() && target.after.is_none() {
        return Err(CommandError::Rejected(format!(
            "entry {} ({}) was recorded before undo support and cannot be undone",
            target.seq, target.command
        )));
    }
    if crate::reconcile::NOT_UNDOABLE.contains(&target.command.as_str()) {
        return Err(CommandError::Rejected(format!(
            "{} cannot be undone with history.undo; use reconcile.unselect or reconcile.cancel instead",
            target.command
        )));
    }
    undo_entry(ctx, &target)?;
    Ok(json!({ "undone": target.seq, "command": target.command }))
}

/// Reverse one audit entry (shared by history.undo and reconcile.undo_last).
pub(crate) fn undo_entry(ctx: &Ctx<'_>, target: &AuditEntry) -> CommandResult<()> {
    if target.command == "reconcile.finish" {
        crate::reconcile::check_undo_finish(ctx, &target.result)?;
        ctx.unreconciling.set(true);
    }
    let after = parse_snapshots(target.after.as_ref())?;
    let before = parse_snapshots(target.before.as_ref())?;
    // Expected now: the entry's `after`. Target: its `before`.
    ctx.suppress_usage.set(true);
    apply(ctx, target, &after, &before)?;
    ctx.tx.forget_usage(target.seq)?;
    ctx.links.set(Links {
        undoes: Some(target.seq),
        redoes: None,
    });
    Ok(())
}

fn redo(ctx: &Ctx<'_>, _: NoArgs) -> CommandResult<Value> {
    let entries = ctx.tx.audit_entries_by(&ctx.actor.0)?;
    let undo_entry = redo_target(&entries)
        .ok_or_else(|| CommandError::Rejected("nothing to redo".into()))?
        .clone();
    let original_seq = undo_entry
        .undoes
        .ok_or_else(|| CommandError::Integrity("redo target is not an undo".into()))?;
    let original = ctx
        .tx
        .audit_entry(original_seq)?
        .ok_or_else(|| CommandError::Integrity(format!("audit entry {original_seq} missing")))?;
    let before = parse_snapshots(original.before.as_ref())?;
    let after = parse_snapshots(original.after.as_ref())?;
    // Expected now: the original's `before`. Target: its `after`.
    apply(ctx, &original, &before, &after)?;
    ctx.links.set(Links {
        undoes: None,
        redoes: Some(undo_entry.seq),
    });
    Ok(json!({ "redone": original.seq, "command": original.command }))
}

fn find<'a>(list: &'a [Snapshot], kind: Kind, id: &str) -> Option<&'a Value> {
    list.iter()
        .find(|s| s.kind == kind && s.id == id)
        .map(|s| &s.data)
}

fn decode<T: serde::de::DeserializeOwned>(v: Option<&Value>) -> CommandResult<Option<T>> {
    v.map(|v| {
        serde_json::from_value(v.clone())
            .map_err(|e| CommandError::Integrity(format!("bad audit snapshot: {e}")))
    })
    .transpose()
}

fn stale(entry: &AuditEntry, what: &str) -> CommandError {
    CommandError::Rejected(format!(
        "{what} has been changed by a later command, so entry {} ({}) cannot be reversed; \
         make an ordinary edit instead",
        entry.seq, entry.command
    ))
}

/// Move every entity the entry touched from `expected` to `target`, validating first.
fn apply(
    ctx: &Ctx<'_>,
    entry: &AuditEntry,
    expected: &[Snapshot],
    target: &[Snapshot],
) -> CommandResult<()> {
    ctx.restoring.set(true);
    let mut keys: Vec<(Kind, String)> = Vec::new();
    for s in expected.iter().chain(target) {
        if !keys.iter().any(|(k, i)| *k == s.kind && *i == s.id) {
            keys.push((s.kind, s.id.clone()));
        }
    }
    // Settings and the book first, then account creations/changes, transactions, and
    // account deletions last (so splits always point at existing accounts).
    let rank = |(k, id): &(Kind, String)| match k {
        Kind::Setting | Kind::Book | Kind::BookName => 0,
        // Import/rule/reconciliation documents never constrain ledger validation.
        Kind::ImportBatch
        | Kind::ImportRow
        | Kind::ImportAccountMap
        | Kind::Statement
        | Kind::CsvProfile
        | Kind::Rule
        | Kind::Reconciliation
        | Kind::Schedule
        | Kind::ScheduleOccurrence
        | Kind::Budget
        | Kind::Security
        | Kind::Price
        | Kind::InvTxn
        | Kind::InvImportRow
        | Kind::ImportSecurityMap
        | Kind::InvProfile
        | Kind::Definition
        | Kind::Party
        | Kind::Item
        | Kind::TaxRate
        | Kind::SalesDoc
        | Kind::Payment
        | Kind::Deposit
        | Kind::BillDoc
        | Kind::BillPayment
        | Kind::TxnLink
        | Kind::Dimension
        | Kind::Template
        | Kind::Connection
        | Kind::AccountLink
        | Kind::OfxInstitution
        | Kind::Expense
        | Kind::Receipt
        | Kind::F1099Payer
        | Kind::F1099Recipient
        | Kind::F1099Form
        | Kind::Vehicle
        | Kind::Trip
        | Kind::SavedTrip
        | Kind::MileageRate
        | Kind::ImportForgotten
        | Kind::Subscription
        | Kind::SubscriptionAnswer
        | Kind::Project => 0,
        Kind::Account if find(target, Kind::Account, id).is_some() => 1,
        Kind::Transaction => 2,
        Kind::Account => 3,
    };
    // Within account creations parents come first; within deletions children first.
    let mut parents: HashMap<String, Option<String>> = ctx
        .tx
        .accounts()?
        .into_iter()
        .map(|a| (a.id.to_string(), a.parent.map(|p| p.to_string())))
        .collect();
    for snap in expected
        .iter()
        .chain(target)
        .filter(|s| s.kind == Kind::Account)
    {
        if let Ok(Some(a)) = decode::<Account>(Some(&snap.data)) {
            parents
                .entry(a.id.to_string())
                .or_insert(a.parent.map(|p| p.to_string()));
        }
    }
    let depth = |id: &str| {
        let mut d = 0i64;
        let mut cur = parents.get(id).cloned().flatten();
        while let Some(p) = cur {
            d += 1;
            if d > 64 {
                break;
            }
            cur = parents.get(&p).cloned().flatten();
        }
        d
    };
    keys.sort_by_key(|k| {
        let r = rank(k);
        let d = if k.0 == Kind::Account { depth(&k.1) } else { 0 };
        (r, if r == 3 { -d } else { d })
    });

    for (kind, id) in &keys {
        let exp = find(expected, *kind, id);
        let tgt = find(target, *kind, id);
        match kind {
            Kind::Account => {
                let aid: AccountId = id
                    .parse()
                    .map_err(|_| CommandError::Integrity(format!("bad account id {id}")))?;
                let exp: Option<Account> = decode(exp)?;
                let tgt: Option<Account> = decode(tgt)?;
                let cur = ctx.tx.account(aid)?;
                check_account_transition(ctx, cur.as_ref(), tgt.as_ref())?;
                if cur != exp {
                    return Err(stale(entry, "the account"));
                }
                match (&cur, &tgt) {
                    (_, Some(t)) => ctx.save_account(cur.as_ref(), t)?,
                    (Some(c), None) => ctx.remove_account(c)?,
                    (None, None) => {}
                }
            }
            Kind::Transaction => {
                let tid: TransactionId = id
                    .parse()
                    .map_err(|_| CommandError::Integrity(format!("bad transaction id {id}")))?;
                let exp: Option<Transaction> = decode(exp)?;
                let tgt: Option<Transaction> = decode(tgt)?;
                let cur = ctx.tx.transaction(tid)?;
                validate_change(ctx, cur.as_ref(), tgt.as_ref())?;
                if cur != exp {
                    return Err(stale(entry, "the transaction"));
                }
                match (&cur, &tgt) {
                    (_, Some(t)) => ctx.save_txn(cur.as_ref(), t)?,
                    (Some(c), None) => ctx.remove_txn(c)?,
                    (None, None) => {}
                }
            }
            Kind::Setting => {
                let value_of = |v: Option<&Value>| v.and_then(|v| v.get("value")).cloned();
                let snap = exp.or(tgt).cloned().unwrap_or(Value::Null);
                let module = snap.get("module").and_then(Value::as_str).unwrap_or("");
                let key = snap.get("key").and_then(Value::as_str).unwrap_or("");
                if setting_id(module, key) != *id {
                    return Err(CommandError::Integrity(format!(
                        "bad setting snapshot {id}"
                    )));
                }
                // The book's own settings (fiscal year, profile) need no module.
                if key != ENABLED_KEY && module != CORE && module != crate::book_profile::MODULE {
                    known_module(module)?;
                    if !is_module_enabled(ctx.tx, module)? {
                        return Err(CommandError::Rejected(format!(
                            "module {module:?} is disabled; enable it to change its settings"
                        )));
                    }
                }
                if ctx.tx.get_setting(module, key)? != value_of(exp) {
                    return Err(stale(entry, "the setting"));
                }
                ctx.save_setting(module, key, value_of(tgt).as_ref())?;
            }
            kind @ (Kind::ImportBatch
            | Kind::ImportRow
            | Kind::ImportAccountMap
            | Kind::Statement
            | Kind::CsvProfile
            | Kind::Rule
            | Kind::Reconciliation
            | Kind::Schedule
            | Kind::ScheduleOccurrence
            | Kind::Budget
            | Kind::Security
            | Kind::Price
            | Kind::InvTxn
            | Kind::InvImportRow
            | Kind::ImportSecurityMap
            | Kind::InvProfile
            | Kind::Definition
            | Kind::Party
            | Kind::Item
            | Kind::TaxRate
            | Kind::SalesDoc
            | Kind::Payment
            | Kind::Deposit
            | Kind::BillDoc
            | Kind::BillPayment
            | Kind::TxnLink
            | Kind::Dimension
            | Kind::Template
            | Kind::Connection
            | Kind::AccountLink
            | Kind::OfxInstitution
            | Kind::Expense
            | Kind::Receipt
            | Kind::F1099Payer
            | Kind::F1099Recipient
            | Kind::F1099Form
            | Kind::Vehicle
            | Kind::Trip
            | Kind::SavedTrip
            | Kind::MileageRate
            | Kind::ImportForgotten
            | Kind::Subscription
            | Kind::SubscriptionAnswer
            | Kind::Project) => {
                let Some(table) = kind.doc_table() else {
                    continue;
                };
                let exp: Option<Doc> = decode(exp)?;
                let tgt: Option<Doc> = decode(tgt)?;
                if ctx.tx.get_doc(table, id)? != exp {
                    return Err(stale(entry, "a stored record"));
                }
                ctx.save_doc(table, id, tgt.as_ref())?;
            }
            Kind::BookName => {
                let name_of = |v: Option<&Value>| -> Option<String> {
                    v.and_then(|v| v.get("name"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                };
                let (Some(exp), Some(tgt)) = (name_of(exp), name_of(tgt)) else {
                    return Err(CommandError::Integrity("bad book name snapshot".into()));
                };
                if ctx.tx.info()?.name != exp {
                    return Err(stale(entry, "the book name"));
                }
                ctx.save_book_name(&tgt)?;
            }
            Kind::Book => {
                let date_of = |v: Option<&Value>| -> CommandResult<Option<Date>> {
                    decode::<Option<Date>>(v.and_then(|v| v.get("closing_date")))
                        .map(Option::flatten)
                };
                let exp = date_of(exp)?;
                let tgt = date_of(tgt)?;
                let cur = ctx.tx.closing_date()?;
                if cur != exp {
                    return Err(stale(entry, "the closing date"));
                }
                check_reclose(ctx, entry, cur, tgt)?;
                ctx.save_closing_date(tgt)?;
            }
        }
    }
    check_module_consistency(ctx)
}

/// Moving the closing date later re-closes the range `(current, target]`. That is
/// refused if a transaction created after `entry` (by a command still in effect, not
/// itself an undo) now lies in that range.
fn check_reclose(
    ctx: &Ctx<'_>,
    entry: &AuditEntry,
    current: Option<Date>,
    target: Option<Date>,
) -> CommandResult<()> {
    let Some(target) = target else { return Ok(()) };
    if current.is_some_and(|c| c >= target) {
        return Ok(());
    }
    let in_range = |d: Date| current.is_none_or(|c| d > c) && d <= target;
    for later in ctx.tx.audit_entries_after(entry.seq)? {
        if later.undone || later.undoes.is_some() {
            continue;
        }
        let before = parse_snapshots(later.before.as_ref())?;
        for snap in parse_snapshots(later.after.as_ref())? {
            if snap.kind != Kind::Transaction
                || find(&before, Kind::Transaction, &snap.id).is_some()
            {
                continue;
            }
            let Ok(id) = snap.id.parse::<TransactionId>() else {
                continue;
            };
            let now = ctx.tx.transaction(id)?;
            if now.is_some_and(|t| in_range(t.date)) {
                return Err(CommandError::Rejected(format!(
                    "a transaction was created in the range the closing date would close again \
                     after entry {} ({}), so it cannot be reversed",
                    entry.seq, entry.command
                )));
            }
        }
    }
    Ok(())
}
