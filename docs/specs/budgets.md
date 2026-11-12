# Spec: Budgets

- **Area / module:** `budgets` (optional module, already in the catalog)
- **Spec writer:** Claude session 2026-10-19 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-19
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Textbook | OpenStax, *Principles of Managerial Accounting*, ch. 7 "Budgeting" — https://openstax.org/details/books/principles-managerial-accounting | Budgeted vs actual, variance |
| 2 | Spec | `ledger-accounts.md`, `ledger-balances.md` | Display sign, account tree, voided transactions |
| 3 | Spec | `app-desktop.md` | Module gating in the UI |
| 4 | Design | MoneyKeep plan document, Phase 2 | Monthly household budget with rollover |

No target product was observed. Names, wording and screens are MoneyKeep's own.

## Purpose

A household decides how much to spend (and expects to earn) per category each month,
then watches actual spending against that plan.

## Behaviour

All budget commands require the `budgets` module; with it off they return the normal
`module_disabled` error and budget data is kept untouched.

### Budget fields

| Field | Rules |
|---|---|
| `id` | Typed id (`bud_…`) |
| `name` | 1–100 characters, unique in the book (case-insensitive) |
| `start_month` | `YYYY-MM`; months before it have no budget |
| `default` | Boolean; exactly one budget is the default once any exist (the first one created; setting another clears the previous) |
| `lines` | See below |

Periods are calendar months. A year view is the sum of its months.

### Lines

A line budgets one **income or expense** account (any level of the tree, not
placeholder-restricted: budgeting a parent is the usual way to budget a group).

| Field | Rules |
|---|---|
| `account` | Income or expense account; at most one line per account per budget |
| `amount` | Default monthly amount, ≥ 0, at currency scale, in the base currency |
| `overrides` | Optional map `YYYY-MM → amount` replacing `amount` for that month |
| `rollover` | Boolean, default false (see "Rollover") |

Amounts are entered as positive numbers for both income and expenses.

### Actuals

For a line on account A and a month M, **actual** is the display-signed sum (per
`ledger-accounts.md`: expenses positive when money is spent, income positive when
earned) of the base-currency splits of posted, non-voided transactions dated in M,
in A and in every descendant of A that does **not** have its own line in the same
budget. A refund in an expense account reduces the actual and may make it negative.

Splits in other commodities are not counted; the report lists the affected accounts
under `excluded` with the reason "not in base currency".

### Rollover

For a line with `rollover`, the month's **available** amount is its budget plus the
previous month's remaining amount (which may be negative), starting from
`start_month` with nothing carried in. Without rollover, available = budget.

remaining = available − actual. For expenses, a negative remaining means overspent.
For income, a positive remaining means income still expected and a negative one means
more than expected was received.

### Commands

| Command | Mutates | Arguments / result |
|---|---|---|
| `budget.create` | yes | `name`, `start_month` (default current month), optional `lines`, `default` |
| `budget.update` | yes | `budget`, `name`, `start_month`, `default` |
| `budget.delete` | yes | `budget` |
| `budget.list` / `budget.get` | no | |
| `budget.set_line` | yes | `budget`, `account`, `amount`, `overrides` (replaced entirely when given), `rollover`. Creates or replaces the line |
| `budget.set_month` | yes | `budget`, `account`, `month`, `amount` — sets one override (an amount equal to the line's default removes the override) |
| `budget.remove_line` | yes | `budget`, `account` |
| `budget.suggest` | no | `budget` (or none), `from_month`, `to_month` (default the 12 months before the current month), `method` (`average` default, or `last`), optional `accounts`. For each income/expense leaf account with activity in the range (or each given account): the suggested monthly amount — the average of monthly actuals over all months in the range including zero months, or the last month's actual — rounded **up** to a whole currency unit; negative results become 0 |
| `budget.apply_suggestion` | yes | `budget`, `lines`: a list from `budget.suggest` (possibly edited) to set as lines in one command (one audit entry) |
| `budget.report` | no | See below |

`budget` arguments accept an id or name; when omitted, the default budget is used.

### `budget.report`

Arguments: `budget`, `month` (`YYYY-MM`, default current) or `from_month` +
`to_month` (inclusive range; the report then covers the whole range: budget and actual
are sums over the months; for rollover lines, available = sum of the months' budgets
plus the amount carried into the first month).

Returns:

- `income` and `expenses` sections, each with rows in account tree order: `account`
  (id, full name, depth), `budget`, `available`, `actual`, `remaining`, `percent`
  (actual ÷ available × 100, one decimal, absent when available is zero), `rollover`.
- Each section's totals of budget, available, actual, remaining.
- `net`: income totals minus expense totals, for budget, available and actual.
- `unbudgeted`: income and expense accounts with a non-zero actual in the period that
  are neither budgeted nor descendants of a budgeted account — full name and actual.
  Their sum is reported separately and is **not** part of the section totals.
- `excluded`: as above.
- All amounts are strings at currency scale (like other commands).

## Ledger effect

Non-posting.

## Settings

None beyond the module switch.

## Edge cases

- A line on `Expenses:Food` plus its own line on `Expenses:Food:Dining`: Dining
  spending counts only on the Dining line; Groceries (no line) counts on Food.
- Removing the Dining line makes its actuals count on Food from then on (reports are
  always computed, never stored).
- A month before `start_month`: report rows show budget 0 and a note that the budget
  starts later; actuals are still shown.
- Moving an account in the tree changes which line it counts toward, in every month.
- An account changed to archived keeps its line; actual history still counts.
- Turning the module off and on again shows the same budgets.

## Acceptance tests

1. Budget Groceries 400.00/month; October has purchases 120.00 and 250.00 and a
   refund of 20.00: actual 350.00, remaining 50.00, percent 87.5.
2. Rollover on Groceries 400.00: September actual 450.00 → October available 350.00;
   with October actual 350.00, remaining 0.00.
3. Override: Gifts 50.00 default, 2026-12 → 300.00: December budget 300.00,
   November 50.00; `set_month` 2026-12 50.00 removes the override.
4. Parent/child attribution as in the first edge case: Food line actual excludes
   Dining, includes Groceries.
5. Income line Salary 5,000.00 with 2,500.00 received: remaining 2,500.00; net budget
   = income budget − expense budget.
6. `budget.suggest` over 2026-07..2026-09 with Fuel actuals 40.00, 0, 61.50:
   average 33.833… → 34; `last` → 62.
7. Spending in Utilities with no line (and no budgeted ancestor) appears in
   `unbudgeted` and not in expense totals.
8. With the module off, `budget.report` fails with `module_disabled`; after enabling
   it, the budget is unchanged.
9. Range report 2026-01..2026-03 for a 100.00/month line sums budget to 300.00.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
