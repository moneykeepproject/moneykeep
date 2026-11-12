# Spec: Transactions and splits

- **Area / module:** ledger core (`core`; foreign currencies need `multi-currency`)
- **Spec writer:** Claude session 2026-10-18 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-18
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Textbook | OpenStax, *Principles of Accounting, Volume 1* — https://open.umn.edu/opentextbooks/textbooks/principles-of-accounting-volume-1-financial-accounting | Journal entries, debits equal credits, compound entries, reversing and correcting entries, closing the books |
| 2 | Design | MoneyKeep plan document, "Core data model" (quantity + value per split) | |
| 3 | Spec | `ledger-accounts.md` | Sign convention, placeholder, archived |

No target product was observed for this spec.

## Purpose

A transaction records one economic event: a purchase, a paycheck, a transfer, a
share purchase. It is made of two or more splits, each moving an amount into or out of
one account. Debits always equal credits.

## Behaviour

### Transaction fields

| Field | Rules |
|---|---|
| `id` | Typed id, generated |
| `date` | Calendar date (no time, no time zone) the event happened |
| `currency` | Transaction currency; default book base currency; any other currency requires `multi-currency` |
| `payee` | Optional text, up to 200 characters (a party link replaces this in Phase 4) |
| `number` | Optional text up to 20 characters (cheque number, reference) |
| `memo` | Optional text up to 1000 characters |
| `status` | `posted` or `voided` |
| `entered_at` | Timestamp of creation; with `id`, gives a stable order for same-date transactions |
| `splits` | 2 to 1000 splits |

### Split fields

| Field | Rules |
|---|---|
| `account` | Must exist, not placeholder, not archived |
| `value` | Signed amount in the **transaction currency** (debit +, credit −) |
| `quantity` | Signed amount in the **account's commodity** |
| `memo` | Optional, up to 1000 characters |
| `reconcile` | `n` (new), `c` (cleared) or `r` (reconciled); default `n` |

### Balancing rule

- The sum of `value` over all splits must be **exactly zero**. There is no automatic
  "imbalance" account; an unbalanced transaction is rejected with the difference
  reported.
- If a split's account commodity equals the transaction currency, `quantity` must
  equal `value`. Clients may omit `quantity` in that case.
- If they differ (foreign currency or security), `quantity` must be given. When both
  are non-zero they must have the same sign. `value` may be zero with a non-zero
  `quantity` (e.g. a stock split adding shares) and vice versa only when the account
  commodity differs from the transaction currency.
- Amount precision: up to 9 decimal places for quantity, up to the currency's minor
  unit for value (2 for USD, 0 for JPY; unknown codes default to 2). Values with more
  decimals are rejected, not rounded.
- A transaction may not consist solely of zero-value, zero-quantity splits.

### Closing date (period lock)

The book has an optional closing date. Any command that would create, change, void or
delete a transaction dated on or before the closing date is rejected, as is changing
a transaction's date into the closed period. Setting or moving the closing date is its
own audited command (`book.set_closing_date`).

### Editing

- `txn.update` replaces a transaction's fields and full split list.
- Splits with `reconcile = r` may have only their `memo` changed; changing their
  account, value or quantity, or removing them, is rejected. (The user must
  unreconcile first, which Phase 2 defines.)
- The audit log records the full transaction before and after.

### Reconcile flags

`txn.set_reconcile` sets the `reconcile` state of one or more splits (by transaction id
and split index) to `n`, `c` or `r`. It is allowed even for reconciled splits (moving a
split away from `r` is "unreconciling"), but not inside the closed period. `txn.update`
does not change reconcile states: splits keep theirs, and new splits start at `n`.
Phase 2's reconciliation workflow is built on this command.

### Voiding and deleting

- `txn.void` keeps the transaction and its original splits, sets `status = voided`, and
  records an optional reason. Voided transactions are excluded from every balance and
  report total but remain visible in registers.
- `txn.unvoid` restores `posted`.
- `txn.delete` removes a transaction entirely. Allowed only if no split is reconciled
  and the date is after the closing date. The audit log keeps the full content.

### Reversing

`txn.reverse` creates a new posted transaction dated on a given date (default today)
with every split's value and quantity negated, memo "Reversal of <number or date>",
and a link to the original. This is how errors in a closed period are corrected.

### Register order

Within an account, transactions sort by `date`, then `entered_at`, then `id`.

## Ledger effect

These commands are the ledger: each posted, non-voided transaction changes account
balances by its splits' quantities.

## Commands

| Command | Mutates |
|---|---|
| `txn.create` | yes |
| `txn.update` | yes |
| `txn.void` / `txn.unvoid` | yes |
| `txn.delete` | yes |
| `txn.reverse` | yes |
| `txn.set_reconcile` | yes |
| `txn.get` | no |
| `txn.list` | no — filters: `account` (with or without children), `from`, `to`, `include_voided`, `limit`, `offset` |
| `book.set_closing_date` | yes — date or null |

## Settings

None in this spec.

## Edge cases

- A transfer between two accounts is a 2-split transaction (+100 / −100).
- A paycheck is a compound transaction: gross income credit, tax and deduction expense
  debits, net deposit debit.
- A split to the same account twice is allowed (e.g. two memo lines).
- Changing a transaction's date from inside to outside the closed period is rejected
  (it touches the closed period).
- A USD book with `multi-currency` off: a transaction in `EUR` is rejected; a security
  purchase (USD transaction, `VTI` account) is allowed because the transaction currency
  is the base currency.

## Acceptance tests

1. A 50.00 grocery purchase is `Groceries +50.00` (debit, expense up) and
   `Checking −50.00` (credit, asset down). Splits may be given in any order; the
   stored result is the same.
2. Splits `+100.00` and `−99.99` are rejected with difference `0.01`.
3. `10.005` USD is rejected (three decimals).
4. Buy 10 VTI for 2,500.00: `Brokerage:VTI value +2500.00 quantity +10`,
   `Brokerage:Cash value −2500.00` — accepted; VTI balance is 10 shares.
5. Value +2500 with quantity −10 is rejected (sign mismatch).
6. With closing date 2026-06-30, creating a 2026-06-30 transaction is rejected and a
   2026-07-01 one is accepted; updating the latter's date to 2026-06-15 is rejected.
7. A voided transaction no longer affects balances; unvoiding restores them.
8. Editing the value of a reconciled split is rejected; editing its memo works.
9. `txn.reverse` produces a transaction whose splits sum with the original to zero in
   every account.
10. Deleting a transaction leaves an audit entry containing its full content.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
