# Spec: Balances and trial balance

- **Area / module:** ledger core (`core`)
- **Spec writer:** Claude session 2026-10-18 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-18
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Textbook | OpenStax, *Principles of Accounting, Volume 1* — https://open.umn.edu/opentextbooks/textbooks/principles-of-accounting-volume-1-financial-accounting | Account balances, trial balance, accounting equation |
| 2 | Spec | `ledger-accounts.md`, `ledger-transactions.md` | Sign convention, voided transactions |

No target product was observed for this spec.

## Purpose

Users need to know how much is in each account, as of any date, with or without its
sub-accounts, and need a check that the books balance.

## Behaviour

### Account balance

`balance(account, as_of, include_children, basis)`:

- Sums the `quantity` of every split in the account (and, if `include_children`, in
  all descendants) where the transaction is `posted` (not voided) and `date <= as_of`
  (default: no date limit).
- `basis`: `all` (default), `cleared` (splits with reconcile `c` or `r`), or
  `reconciled` (reconcile `r` only).
- The result is a map from commodity to amount, because children may hold different
  commodities (a brokerage parent with cash and several securities). Commodities with
  a zero total are omitted; an account with no activity returns an empty map, shown as
  zero in its own commodity.
- Each amount is returned both raw (debit-positive) and in display form (negated for
  credit-normal types, per `ledger-accounts.md`).

### Trial balance

`trial_balance(as_of)` lists every account with activity, its raw balance in each
commodity, and totals. For every transaction currency, the sum of split **values**
over all posted transactions up to `as_of` is zero. This invariant must hold for every
book at every date; a non-zero total is a defect, reported as an error, never hidden.

### Accounting equation check

For accounts in the base currency only: assets = liabilities + equity + (income −
expenses), using display balances. This follows from the trial balance invariant and
is exposed as a diagnostic.

## Ledger effect

Read-only.

## Commands

| Command | Mutates |
|---|---|
| `account.balance` | no — `account`, `as_of`, `include_children`, `basis` |
| `report.trial_balance` | no — `as_of` |

## Edge cases

- Balance as of a date before any activity is empty (zero).
- Archived accounts still contribute to historical balances.
- A brokerage parent with children `Cash` (USD) and `VTI` returns `{USD: …, VTI: …}`
  with `include_children`.

## Acceptance tests

1. After opening balance Checking 1,000.00 (Opening Balances −1,000.00) and a 50.00
   grocery purchase, Checking shows 950.00, Groceries 50.00, Opening Balances displays
   1,000.00, and the trial balance totals zero.
2. `as_of` the day before the purchase shows Checking 1,000.00.
3. Voiding the purchase restores Checking to 1,000.00.
4. With one split cleared and one not, `cleared` basis counts only the cleared one.
5. A property test: for any randomly generated sequence of valid transaction creates,
   updates, voids and deletes, the trial balance totals zero in every currency.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
