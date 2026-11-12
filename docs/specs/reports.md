# Spec: Personal reports

- **Area / module:** reports (`core`)
- **Spec writer:** Claude session 2026-10-19 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-19
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Textbook | OpenStax, *Principles of Accounting, Volume 1*, ch. 2 and ch. 16 — https://open.umn.edu/opentextbooks/textbooks/principles-of-accounting-volume-1-financial-accounting | Income statement, balance sheet, statement of cash flows (concepts) |
| 2 | Standard | RFC 4180, *Common Format and MIME Type for CSV Files* | Export format |
| 3 | Spec | `ledger-accounts.md`, `ledger-balances.md`, `ledger-transactions.md` | Sign convention, bases, voided transactions |
| 4 | Design | MoneyKeep plan document, Phase 2 | The personal report set |

No target product was observed. Report names, layouts and wording are MoneyKeep's
own.

## Purpose

Answer the household's standing questions: where did the money go, did we earn more
than we spent, what are we worth and how is that changing, and what came in and went
out of our spending accounts.

## Common rules

These apply to every report below.

- **Dates.** `from` and `to` are inclusive calendar dates. A report also accepts
  `preset`: `this_month`, `last_month`, `this_quarter`, `this_year`, `last_year`,
  `last_12_months` (the 12 full calendar months before the current month; the
  current month is not included), `year_to_date`, or `all`. Presets are
  resolved against an optional `today` argument (default the local date) and the
  resolved dates are returned in the result.
- **Intervals.** Where a report has `interval` (`month`, `quarter`, `year`), periods
  are calendar periods; the first and last are cut to `from`/`to`. Each period
  carries its own `from`/`to` and a label: `2026-10`, `2026-Q4`, `2026`.
- Only posted transactions count; voided ones never do.
- **Currency.** Only splits in accounts whose commodity is the book's base currency
  are counted. Other accounts are listed in `excluded` with a reason; nothing is
  converted (prices arrive in Phase 3).
- **Signs.** Amounts are display-signed (`ledger-accounts.md`): income and expense
  positive in their normal direction, so a refund shows as a negative expense.
- **Splits.** Each split counts in its own account; a split transaction's categories
  are reported separately.
- **Exactness.** All amounts are strings at currency scale. Every total equals the
  exact sum of the rows it totals; no rounding happens anywhere.
- **Account filter.** `accounts`, where accepted, is a list of account refs; a parent
  includes its descendants.
- **Grouping.** `depth` (default unlimited) keeps the first n levels of each account's
  full name; deeper accounts roll up into their ancestor at level n. With depth 2,
  `Food:Dining:Lunch` counts toward `Food:Dining`; with depth 1, toward `Food`.
- **Percentages** have one decimal, rounded half away from zero.
- Every report is a non-mutating command; results are recomputed each time.

## Reports

### `report.spending`

Where money went, by category. Arguments: dates, `accounts` (the money accounts to
look at — default every asset and liability account), `depth`, `include_income`
(default false).

For each expense account (and income account when `include_income`): the sum of its
splits in transactions that also have a split in one of the selected money accounts,
**limited to the share** of the transaction those money accounts paid (for a typical
two-split purchase that is the whole amount). Rows are sorted by amount, largest
first, with `percent` of the total (one decimal). Totals: `total`.

When a transaction touches several selected money accounts and categories, each
category split is counted once.

### `report.income_expense`

Did we earn more than we spent. Arguments: dates, `interval` (default `month`),
`depth`.

Returns `periods` (each with `from`, `to`, `label`), and two sections, `income` and
`expenses`, each with rows (account full name, depth, one amount per period, row
total) in tree order, and per-period and grand totals. Also `net` per period and in
total (income − expenses).

Transactions that touch only asset/liability/equity accounts (transfers, card
payments, opening balances) contribute nothing.

### `report.net_worth`

What we own and owe over time. Arguments: dates, `interval` (default `month`),
`depth` (default 1 within assets and liabilities).

For each period end date (each period's `to`), the display balance (basis `all`) of
every asset and liability account up to that date, in tree order; totals `assets`,
`liabilities` and `net_worth` = assets − liabilities per date; plus `change` between
the first and last date. Equity, income and expense accounts are not listed.
A single date is a `from` = `to` report.

### `report.cash_flow`

What came into and went out of the spending accounts. Arguments: dates, `accounts`
(default every asset with subtype `bank` or `cash` and every liability with subtype
`credit-card`), `interval` (optional; without it one period).

For the selected money accounts taken together:

- `opening` (day before `from`) and `closing` (at `to`): the net position of the
  selected accounts — the sum of the selected assets' display balances minus the sum
  of the selected liabilities' display balances.
- `inflows` and `outflows`: rows grouped by the **other side** of each transaction —
  the category (income/expense account) or, for transfers, the non-selected account
  (shown as "Transfer: <full name>"). Inflows are positive amounts into the selected
  accounts, outflows positive amounts out of them.
- Transfers **between two selected accounts** (e.g. paying the credit card from
  checking) are not inflows or outflows; they are reported once as `internal`, total
  only.
- Check: opening + inflows − outflows = closing, exactly. The result includes
  `difference` (always "0.00" for a correct implementation; a test asserts it).

### `report.payees`

Who we paid. Arguments: dates, `accounts` (as `report.spending`), `limit` (default
50). Rows by normalised payee (`import-pipeline.md`), each with the most common
original spelling as its display name, count of transactions, total spent, total
received; sorted by total spent.

### `report.export`

Arguments: `report` (one of the names above), its arguments, and `format` `csv`.
Returns `{file_name, content}` where `file_name` is e.g.
`income-expense-2026-01-01-to-2026-10-31.csv` and `content` is RFC 4180 CSV:

- UTF-8, comma separator, CRLF line endings, a header row, fields with commas, quotes
  or line breaks quoted with doubled quotes.
- One CSV row per report row in the order shown; section headings become a row with
  only the first column; totals rows start with "Total".
- Amounts in plain form: `-1234.56` (no thousands separators, no currency symbols,
  `.` decimal point); dates `YYYY-MM-DD`.

The app writes the content to a file the user chooses; the command does no file IO.

## Ledger effect

Read-only.

## Settings

None.

## Edge cases

- An empty book: every report returns zero totals and empty rows, not an error.
- A credit card payment from checking: absent from `report.spending` and
  `report.income_expense`; in `report.cash_flow` with default accounts it is
  `internal`.
- A purchase split between Groceries 80.00 and Household 20.00 paid from Visa: both
  categories appear in `report.spending` with their own amounts.
- A paycheck deposited as gross Salary 3,000.00 less Taxes 600.00 into checking
  2,400.00: income 3,000.00, expenses 600.00, cash-flow inflow Salary 3,000.00 and
  outflow Taxes 600.00.
- A brokerage account holding VTI appears under `excluded` in net worth.
- A transaction dated exactly on `to` is included; one on `to` + 1 is not.

## Acceptance tests

Book: Checking opening 1,000.00 on 2026-09-30 (from Opening Balances); Visa (credit
card). October 2026: Salary 2,500.00 into Checking (10-01); Groceries 63.41 from
Checking (10-03); Dining 23.65 from Visa (10-05); Visa payment 23.65 from Checking
(10-20); Groceries refund 10.00 into Visa (10-22); split purchase from Visa 10-25:
Groceries 80.00 + Household 20.00. September 2026: Fuel 41.07 from Checking (09-15).

1. `report.spending` for October: Groceries 133.41, Dining 23.65, Household 20.00;
   total 177.06; Groceries percent 75.3.
2. `report.income_expense` 2026-09-01..2026-10-31, month: Salary 0.00 / 2,500.00;
   expenses September 41.07, October 177.06; net September −41.07, October
   2,322.94, total 2,281.87.
3. `report.net_worth` 2026-09-01..2026-10-31, month (dates 09-30 and 10-31): assets
   958.93 (Checking 1,000.00 − 41.07) and 3,371.87; liabilities 0.00 and 90.00 (Visa:
   23.65 − 23.65 − 10.00 + 100.00); net worth 958.93 and 3,281.87; change 2,322.94
   (equal to October's net income, as it must be here).
4. `report.cash_flow` for October, default accounts: opening 958.93; inflows Salary
   2,500.00 and Groceries 10.00; outflows Groceries 143.41, Dining 23.65, Household
   20.00 (outflows total 187.06); internal 23.65; closing 3,281.87 (Checking
   3,371.87 minus Visa 90.00); 958.93 + 2,510.00 − 187.06 = 3,281.87, difference 0.00.
5. `report.export` of test 2 produces a header row
   `Category,2026-09,2026-10,Total`, rows `Salary,0.00,2500.00,2500.00`, and lines
   ending in CRLF.
6. Voiding the Dining purchase removes it from every report.
7. `preset last_month` with `today` 2026-11-05 resolves to 2026-10-01..2026-10-31.

Note on test 4: cash-flow groups by the other side **per direction**; the October
Groceries refund (10.00 into Visa) is an inflow, and Groceries outflows are 63.41 +
80.00 = 143.41. Net spending on groceries (133.41) is what `report.spending` shows.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-10-19) — decisions confirmed after implementation

- `report.spending` counts each category in proportion to the share of the
  transaction's money-account side that went through the selected accounts.
- `report.net_worth` default depth 1 means top-level accounts.
- CSV section-heading rows keep the full field count (empty fields after the name).
- When `to` is after `today`, `report.net_worth` drops period ends after today and
  adds `today` as the final date (see `app-desktop.md` revision 5).

## Revision 3 (2026-10-20) — security accounts at market value (Phase 3a)

Amended by `investments-core.md` ("Balances and reports"):

- `report.net_worth` counts each security account at **market value**: shares ×
  the latest price on or before each period end, rounded to cents. Such accounts are
  no longer listed under `excluded`.
- A security account with no price on or before the date counts at its cost basis
  and is listed under `excluded` with the reason "no price; counted at cost basis".
- `report.export` also exports `report.holdings`, `report.realized_gains` and
  `report.investment_income` (they need the `investments` module).

## Revision 4 (2026-11-04) — Tax summary

`report.tax_summary` (totals by account tax line, with a beginning/end balance
section for `Sch L` lines and a Not tagged section) is specified in
`chart-presets.md`. The common rules above apply to it.
