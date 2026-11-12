# Spec: Audit log and undo

- **Area / module:** ledger core (`core`)
- **Spec writer:** Claude session 2026-10-18 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-18
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Textbook | OpenStax, *Principles of Accounting, Volume 1* — https://open.umn.edu/opentextbooks/textbooks/principles-of-accounting-volume-1-financial-accounting | Audit trail, correcting entries |
| 2 | Design | MoneyKeep plan document ("History is append-only", "Undo is a compensating command") and `AGENTS.md` | |
| 3 | Existing code | Phase 0 `audit_log` table (append-only, enforced by triggers) | |

No target product was observed for this spec.

## Purpose

Every change to the books must be traceable (who, when, what changed), and users need
a reliable undo that never silently rewrites history.

## Behaviour

### Audit entries

Each mutating command writes exactly one audit entry in the same SQLite transaction
as its changes (already true in Phase 0). Phase 1 extends the entry with:

| Field | Meaning |
|---|---|
| `before` | JSON snapshot of every entity the command changed or deleted, as it was before (null for creations) |
| `after` | JSON snapshot of every entity created or changed, as it is after (null for deletions) |
| `undoes` | Sequence number of the entry this one undoes, if it is an undo |
| `undone_by` | Not stored on the old row (rows are immutable); derived by looking up entries whose `undoes` points at it |

Entities are identified by kind and id, e.g. `{"kind":"transaction","id":"…"}`.

### Undo

`history.undo` reverses the most recent audit entry that:

1. was written by the same actor,
2. is not itself an undo, and
3. has not already been undone.

It computes the inverse from `before` and `after`: re-create deleted entities, delete
created ones, restore changed ones to `before`. The undo is a new command with its own
audit entry (`undoes` = the original's sequence). It runs every normal validation;
if the inverse would now be invalid — the transaction has since been reconciled, the
period has been closed, an account it needs was archived, or the entity was changed
again by a later command — the undo is rejected with the reason, and nothing changes.

`history.redo` re-applies the most recent undone entry by the same actor, with the same
rules, if no other change by that actor has happened since the undo.

`history.list` returns audit entries (newest first) with an `undone` flag.

Undo never removes or edits audit rows.

### Which commands are undoable

All mutating commands in `ledger-accounts.md`, `ledger-transactions.md` and the
Phase 0 module/setting commands. `book.set_closing_date` is undoable only by the same
actor and only if no transaction has been created in the newly opened range since.

## Ledger effect

Undo changes the ledger exactly as the inverse operation would.

## Commands

| Command | Mutates |
|---|---|
| `history.undo` | yes |
| `history.redo` | yes |
| `history.list` | no (replaces Phase 0 `audit.list`, which remains as an alias) |

## Edge cases

- Undo with nothing to undo returns a clear "nothing to undo" rejection.
- Actor A cannot undo actor B's change with `history.undo` (B's change is not "most
  recent by A"). Correcting someone else's change is done by an ordinary edit.
- Undo of `txn.delete` re-creates the transaction with the same id.
- Undo of `account.create` fails if a transaction has since used the account.

## Acceptance tests

1. Create a transaction, undo → transaction gone, balances back, two audit entries.
2. Redo → transaction back with the same id.
3. Create T, update T, undo → T restored to its pre-update state; undo again → T gone.
4. Create T, mark a split `r` with `txn.set_reconcile` as actor B, then actor A undoes
   T's creation → rejected (a split is reconciled), nothing changes.
5. Set closing date after T's date, then undo T's creation → rejected (closed period).
6. Audit rows are never updated or deleted (the Phase 0 trigger test still passes).
7. Property test: for random command sequences, undoing all of them in reverse order
   returns the book to its initial state (accounts, transactions and settings equal).

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
