# Spec: Register view and entry helpers

- **Area / module:** ledger (`core`)
- **Spec writer:** Claude session 2026-10-18 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-18
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Textbook | OpenStax, *Principles of Accounting, Volume 1* — https://open.umn.edu/opentextbooks/textbooks/principles-of-accounting-volume-1-financial-accounting | Ledger accounts and running balances |
| 2 | Spec | `ledger-accounts.md`, `ledger-transactions.md`, `ledger-balances.md` | Register order, display sign, voided transactions |
| 3 | Design | AGENTS.md: "front ends never compute business results" | Why this is a command, not UI code |

No target product was observed. Layout and wording are MoneyKeep's own.

## Purpose

Every front end shows an account's transactions in order with a running balance, and
offers suggestions while typing. These are computed by commands so the GUI, CLI and
MCP agree exactly.

## Behaviour

### `account.register` (non-mutating)

Arguments: `account` (ref), `from` and `to` (optional dates, inclusive),
`include_voided` (default true), `include_children` (default false), `offset`
(default 0), `limit` (default 200, max 5000), `order` (`asc` default, or `desc`).

Returns:

- `account`: id, full name, type, commodity.
- `opening_balance`: display-signed balance of all non-voided splits **before**
  `from` (zero if `from` is absent), in the account's commodity.
- `rows`: one row per split of the account (or of the account and its descendants
  with `include_children`) in register order (`ledger-transactions.md`: date, then
  entered time, then id; then split index). Each row:

  | Field | Meaning |
  |---|---|
  | `txn` | Transaction id |
  | `split` | Split id `<txn>:<index>` (same form as reconciliation) |
  | `date`, `number`, `payee`, `memo` | From the transaction (split memo overrides transaction memo if non-empty) |
  | `other` | `{kind: "account", account, full_name}` when the transaction has exactly one other split; `{kind: "split", count}` when more; `{kind: "none"}` when every split is in this account |
  | `amount` | Display-signed amount of this split in the account commodity, formatted at currency scale |
  | `increase` | true if the amount raises the displayed balance (deposit to a bank account, charge to a card) |
  | `reconcile` | `n`, `c` or `r` |
  | `status` | `posted` or `voided` |
  | `balance` | Display-signed running balance **after** this row; voided rows show the unchanged balance |
  | `imported` | true if the split carries an import fingerprint |

- `total_rows` (ignoring offset/limit), `closing_balance` (after the last row in the
  date range), `cleared_balance` and `reconciled_balance` (bases from
  `ledger-balances.md`, as of `to`).

Running balances are computed over the full date range regardless of `offset`/
`limit`, so a page's first row already carries the correct balance. With `desc`, rows
are returned newest first but each row's `balance` is still the balance after that row
in chronological order.

`include_children` with children in different commodities is rejected (one running
balance needs one commodity).

### `payee.suggest` (non-mutating)

Arguments: `prefix` (string, may be empty), `account` (optional ref), `limit`
(default 10, max 50).

Returns distinct payees from posted transactions whose normalised payee
(`import-pipeline.md` normalisation) starts with the normalised prefix, or contains
it as a word start, ordered by most recent use. Each suggestion carries the most
recent transaction's other-side account (only when it had exactly one other split) and
amount, so a front end can pre-fill them. With `account`, transactions touching
that account come first; if fewer than `limit` match, payees from other accounts fill the
rest (revision 2: a payee used only on a card should still be suggested in checking).

### `account.suggest` (non-mutating)

Arguments: `prefix`, `types` (optional list), `limit` (default 10). Returns
non-placeholder, non-archived accounts whose full name or any name segment starts with
the prefix (case-insensitive), ordered by: exact name match, then most recently used,
then full name.

## Ledger effect

Read-only.

## Edge cases

- An account with no activity: empty rows, all balances zero in its commodity.
- A transaction with two splits in the same account appears as two rows.
- A transfer shows the other account by full name.
- Very large registers: the command must return a page of 200 rows from 100,000 in
  well under a second on a laptop (performance test, release build, not run in CI
  debug builds).

## Acceptance tests

1. Checking with opening 1,000.00, a −50.00 purchase and a +200.00 deposit: rows show
   balances 1,000.00, 950.00, 1,150.00; `increase` false for the purchase, true for
   the deposit.
2. A credit card: a 300.00 charge shows `increase` true and balance 300.00 owed.
3. `from` after the first row: `opening_balance` is the balance before it and the
   first returned row's balance continues from it.
4. `offset 1, limit 1` returns the second row with the same balance as in the full
   list.
5. A voided transaction appears with `status voided` and does not change the running
   balance; `include_voided false` hides it.
6. A three-split transaction shows `other.kind split, count 2`.
7. `desc` order returns rows reversed with the same per-row balances.
8. `payee.suggest "kro"` returns "KROGER #512 LIMA OH" with its last other account
   Groceries; `account.suggest "gro"` returns `Groceries` before `Expenses:Groceries
   Online` if the former is an exact name match.
9. `include_children` on a parent whose children hold USD and VTI is rejected.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
