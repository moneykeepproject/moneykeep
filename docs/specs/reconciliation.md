# Spec: Statement reconciliation

- **Area / module:** ledger (`core`)
- **Spec writer:** Claude session 2026-10-18 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-18
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Textbook | OpenStax, *Principles of Accounting, Volume 1*, bank reconciliation — https://open.umn.edu/opentextbooks/textbooks/principles-of-accounting-volume-1-financial-accounting | Statement vs book balance, outstanding items |
| 2 | Spec | `ledger-transactions.md` (reconcile flags, `txn.set_reconcile`), `ledger-balances.md` (`reconciled` basis), `import-pipeline.md` (statement capture) | |

No target product was observed.

## Purpose

Each month the user confirms that the book agrees with the bank or card statement:
every item on the statement is in the book, and the balances match.

## Behaviour

### Session

A reconciliation is a session on one account (any non-placeholder asset or liability
account) with:

| Field | Rules |
|---|---|
| `account` | The account |
| `statement_date` | Statement closing date |
| `statement_balance` | Closing balance as shown on the statement, in display sign (a card with 812.40 owed is entered as 812.40) |
| `opening_balance` | Computed: the account's `reconciled`-basis balance (display sign) when the session starts |
| `status` | `open`, `finished`, `cancelled` |
| `selected` | Set of split ids the user has ticked in this session |

At most one open session per account.

### Candidates

Splits in the account, in posted transactions dated on or before `statement_date`,
whose reconcile flag is `n` or `c`. Cleared (`c`) splits are pre-selected.

### Difference

`difference = statement_balance − (opening_balance + display-signed sum of selected
splits' quantities)`. The session can be finished only when the difference is exactly
zero.

### Finishing

`reconcile.finish`:

- Sets every selected split to `r` (via the same rules as `txn.set_reconcile`) and
  records the session (date, balances, split ids, actor) as a `reconciliation` record.
- Unselected candidates keep their flags.
- Rejected if the difference is not zero, or if any selected split's transaction is
  dated after `statement_date` or inside the closed period.

`reconcile.finish` with `adjust: true` and a non-zero difference creates a balancing
transaction dated `statement_date` between the account and the account given as
`adjustment_account` (required with `adjust`), marks its split `r`, then finishes.
This is explicit; it never happens without `adjust: true`.

### Undo

`reconcile.undo_last` (or `history.undo` on the finish) reverts the most recent
finished reconciliation of that account: its splits return to the flags they had
before finishing, and any adjustment transaction is removed. Older reconciliations
cannot be undone while a newer one exists.

### Statement from import

`reconcile.start` accepts a captured statement id (`import-pipeline.md`), filling in
date and balance (converted to display sign).

### Commands

| Command | Mutates |
|---|---|
| `reconcile.start` | yes — account, statement date, statement balance or statement id |
| `reconcile.get` | no — session with candidates, selected, difference |
| `reconcile.select` / `reconcile.unselect` | yes — split ids |
| `reconcile.finish` | yes |
| `reconcile.cancel` | yes — session discarded, flags untouched |
| `reconcile.history` | no — finished reconciliations of an account |
| `reconcile.undo_last` | yes |

All these commands are audited. `history.undo` of a `reconcile.select`,
`reconcile.unselect`, `reconcile.start` or `reconcile.cancel` is rejected with a
message to use `reconcile.unselect` or `reconcile.cancel` instead; `history.undo` of
`reconcile.finish` behaves like `reconcile.undo_last`.

## Ledger effect

Only reconcile flags change, plus an explicit adjustment transaction when requested.

## Edge cases

- First-ever reconciliation: opening balance is 0.00 (nothing reconciled yet).
- A split reconciled in an earlier session is not a candidate.
- Editing a reconciled split's amount stays blocked (`ledger-transactions.md`).
- A credit-card statement balance is entered as the amount owed; display sign handles
  it.

## Acceptance tests

1. Checking with opening balance 1,000.00 (reconciled), cleared −50.00 and −25.00,
   uncleared −10.00; statement balance 925.00: the two cleared items are pre-selected,
   difference is 0.00, finish marks them `r`, the −10.00 stays `n`.
2. Same with statement balance 920.00: finish is rejected (difference −5.00); with
   `adjust: true` and an adjustment account it creates a −5.00 transaction marked `r`
   and finishes.
3. A credit card with charges 300.00 and 512.40 and statement balance 812.40 owed
   reconciles to zero difference.
4. A split dated after the statement date is not a candidate.
5. Starting a second session on the same account while one is open is rejected.
6. `reconcile.undo_last` returns the splits to their previous flags and removes the
   adjustment transaction; undoing an older one while a newer exists is rejected.
7. After finishing, `account.balance` with `basis: "reconciled"` equals the statement
   balance (display sign).
8. Starting from a captured OFX statement fills in date and balance.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
