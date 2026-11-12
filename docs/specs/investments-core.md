# Spec: Investments, part 1 (Phase 3a)

- **Area / module:** `investments` (optional module, already in the catalog)
- **Spec writer:** Claude session 2026-10-20 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-20
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Government | IRS Publication 551, *Basis of Assets* (12/2025) — https://www.irs.gov/publications/p551 | Basis = price plus purchase costs; identified shares; otherwise shares acquired first; average basis for mutual fund shares |
| 2 | Government | IRS Publication 550, *Investment Income and Expenses* | Holding period (more than one year is long-term, counted from the day after acquisition); stock splits spread basis over the new shares; reinvested dividends are income and buy new shares; return of capital reduces basis |
| 3 | Government | IRS Instructions for Form 8949 | Fields reported per disposition: description, date acquired ("Various" allowed), date sold, proceeds, cost basis, gain or loss; short- vs long-term |
| 4 | Textbook | OpenStax, *Principles of Accounting, Volume 1* | Recording asset purchases and sales, gains as income |
| 5 | Spec | `ledger-accounts.md`, `ledger-transactions.md`, `ledger-balances.md`, `reports.md`, `budgets.md` | Security subtype, quantity vs value, module gating |

No target product was observed. Names, actions and wording are MoneyKeep's own. This
is record-keeping, not tax advice; the app says so where it shows gains.

## Purpose

Track brokerage and retirement accounts: what is held, what it cost, what it is worth,
what was earned, and the realised gains of each sale split into short- and long-term,
with correct cost basis.

## Behaviour

All commands here require the `investments` module (`module_disabled` otherwise). Data
is kept when the module is off.

### Securities

| Field | Rules |
|---|---|
| `symbol` | 1–12 characters, letters, digits, `.` and `-`, stored upper case; unique; must not be an ISO 4217 currency code. Used as the commodity code of security accounts |
| `name` | 1–100 characters |
| `kind` | `stock`, `etf`, `mutual_fund`, `bond`, `money_market`, `other` |
| `currency` | Pricing currency; base currency in 3a (others need `multi-currency` and are out of scope) |
| `basis_method` | `fifo` (default), `average` (only for kinds `mutual_fund` and `etf`, see below), or `specific` |
| `cusip` | Optional, 9 characters |

`average` is allowed for kinds `mutual_fund` and `etf` (source 1 mentions mutual fund
shares; ETFs structured as funds are commonly treated alike — the user chooses). A
method change applies only to sales dated after the change and is audited.

Commands: `security.create`, `security.update`, `security.get`, `security.list`
(with latest price), `security.delete` (only when never used).

### Prices

`price.set` {`security`, `date`, `price`} stores one closing price per security per day
(replacing that day's price). Prices have up to 6 decimal places, must be > 0.
`price.import` {`content_base64`, `file_name`} reads CSV with a header row containing
`date`, `symbol`, `price` columns (any order, case-insensitive; dates `YYYY-MM-DD` or
`MM/DD/YYYY`), skipping unknown symbols with a warning per symbol, and reports counts.
`price.list` {`security`, `from`, `to`}. `price.latest` {`security`, `as_of`} returns
the latest price on or before `as_of` with its date, or none.

### Investment accounts

- An investment account is an asset account with subtype `brokerage`. It holds cash
  in the base currency itself.
- Each security held in it lives in a **child account** with subtype `security`,
  commodity = the symbol, named after the symbol (e.g. `Brokerage:VTI`), created
  automatically by the first action that needs it. Quantity is the share count; the
  account's **value** total is the remaining cost basis.
- Automatically created income and expense accounts (top level, created on first
  use, renameable): `Dividends`, `Interest`, `Capital Gain Distributions`,
  `Realized Gains` (income), `Investment Fees` (expense). An action may name a
  different income/expense account instead.

### Actions

Each action is a command that creates **one ordinary transaction** (so registers,
balances, reports, audit and undo work as for any transaction) plus investment
metadata (action kind, security, quantity, price, commission, lot selections). All
amounts are in the base currency; quantities up to 9 decimals.

| Command | Arguments | Splits posted |
|---|---|---|
| `inv.buy` | `account`, `security`, `date`, `quantity`, `price`, `commission` (default 0), optional `amount` (total cost; when given it is used as is and `price` is informational) | security account +quantity, value = cost; cash (the brokerage account) −cost. Cost = round(quantity × price, 2) + commission unless `amount` is given. Creates a **lot** |
| `inv.sell` | `account`, `security`, `date`, `quantity`, `price`, `commission`, optional `amount` (net proceeds), optional `lots` [{`lot`, `quantity`}] | cash +proceeds; security −quantity, value −basis; `Realized Gains` −(proceeds − basis) (a loss posts a debit). Proceeds = round(quantity × price, 2) − commission unless `amount` is given |
| `inv.dividend` | `account`, `security`, `date`, `amount`, `kind` (`ordinary`, `qualified`, `capital_gain_long`, `capital_gain_short`, `tax_exempt`), optional `income_account` | cash +amount; income account −amount (Dividends, or Capital Gain Distributions for the capital-gain kinds) |
| `inv.reinvest` | as dividend plus `quantity` and `price` (or `quantity` alone: price = amount ÷ quantity) | security +quantity, value +amount; income account −amount. Creates a lot. No cash split |
| `inv.interest` | `account`, `date`, `amount`, optional `security` | cash +amount; `Interest` −amount |
| `inv.fee` | `account`, `date`, `amount`, optional `security`, optional `expense_account` | `Investment Fees` +amount; cash −amount |
| `inv.shares_in` | `account`, `security`, `date`, `quantity`, `lots` [{`acquired`, `quantity`, `basis`}] summing to `quantity` | security +quantity, value +total basis; `Opening Balances` −total basis. Lots keep their original acquired dates |
| `inv.shares_out` | `account`, `security`, `date`, `quantity`, optional `lots` | security −quantity, value −basis; `Opening Balances` +basis. Not a sale: no gain |
| `inv.split` | `security`, `date`, `ratio_new`, `ratio_old` (e.g. 2 and 1) | In every account holding the security: security quantity × (new/old − 1) added with value 0. Each open lot's quantity is multiplied by new/old; its basis is unchanged |
| `inv.return_of_capital` | `account`, `security`, `date`, `amount` | cash +amount; security value −amount (quantity 0). Each open lot's basis is reduced in proportion to its basis (rounded, remainder on the last lot). Rejected if `amount` exceeds the open basis (excess-as-gain is deferred to 3b) |

Cash moves between a bank and a brokerage account are ordinary transfers. Short sales,
options, margin interest specifics and bonds' accrued interest are out of scope.

Validation: selling or moving out more shares than held on that date in that account
is rejected (`insufficient_shares`, with the quantity held). Dates obey the closing
date.

### Lots and cost basis

- A lot records: security account, acquired date, original quantity, remaining
  quantity, remaining basis, and the transaction that created it.
- **Lots are derived data.** For each security account, open lots and every sale's
  basis are recomputed by replaying its investment transactions in register order
  (date, entered time, id), using each sale's stored method and lot selections.
  After any create, edit or delete of an investment transaction dated D, the replay
  runs for that security account from D on; if a later sale's basis changes, that
  sale transaction's security-value and `Realized Gains` splits are rewritten in the
  same command (one audit entry). If any rewritten sale is dated in the closed period
  or has a reconciled split, the whole command is rejected (`would_change_closed`).
- **Methods** (the security's method at the sale date, unless the sale gives `lots`):
  - `fifo`: consume open lots oldest acquired first (ties by creation order).
  - `specific`: `lots` is required and lists lot ids with quantities summing to the
    sale quantity.
  - `average`: basis per share = total remaining basis ÷ total remaining quantity
    of the security account immediately before the sale; sale basis =
    round(that × quantity, 2). For holding periods the shares are treated as sold
    oldest lot first; each consumed portion's basis is round(average × portion
    quantity, 2), the last portion taking the remainder of the sale basis. After the
    sale, every remaining lot is re-based to the average: lot basis = round(average ×
    remaining quantity, 2), the last lot taking the remainder, so the lots total the
    account's remaining basis exactly.
  - Giving `lots` on a sale overrides the method for that sale (specific
    identification).
- **Partial lots and rounding.** When q of a lot's remaining Q shares are consumed,
  the consumed basis is round(remaining basis × q ÷ Q, 2) and the lot keeps the rest
  (so a lot sold in pieces always totals its original basis exactly). A sale's
  proceeds are allocated to its consumed lot portions in consumption order as
  round(proceeds × q_i ÷ quantity, 2), the last portion taking the remainder.
  Rounding is half away from zero.
- **Holding period.** A portion is long-term when the sale date is later than the
  acquired date's anniversary one year on (Feb 29 → Feb 28 of the next year);
  otherwise short-term. Lots received by `shares_in` use their given acquired date;
  reinvestment lots use the reinvest date.
- **Wash sales** are not adjusted in 3a (flagging is in 3b).

### Holdings and valuation

- `inv.holdings` {`accounts` (default all brokerage accounts), `as_of` (default
  today)}: per account and security: quantity, cost basis (remaining value), price and
  its date (latest on or before `as_of`), market value = round(quantity × price, 2),
  unrealised gain and percent, weight in the account; the account's cash; totals per
  account and overall. A security with no price shows market value "unknown" and is
  left out of market totals, with a note.
- `inv.lots` {`account`, `security`, `as_of`}: open lots with acquired date,
  remaining quantity and basis, basis per share, term if sold on `as_of`, and
  unrealised gain at the price on `as_of`.
- `inv.preview_sell`: the same arguments as `inv.sell`; returns the consumed lot
  portions, basis, proceeds, and short/long-term gains without posting.
- **Balances and reports.** `account.balance` of a security account returns its
  quantity (commodity) and its value (basis). Wherever reports or the sidebar show a
  money value for a security account they now use **market value** at the latest
  price on or before the date, falling back to basis with a "no price" marker when
  there is none. `reports.md` is amended: `report.net_worth` counts security
  accounts at market value (no longer under `excluded` when a price exists).

### Reports

- `report.realized_gains` {dates, `accounts`}: one row per consumed lot portion:
  security symbol and name, quantity, acquired date ("Various" for average-method
  sales), sold date, proceeds, basis, gain, term; subtotals short-term and long-term;
  total. CSV export as other reports.
- `report.investment_income` {dates, `accounts`, `interval`}: dividends by kind,
  interest, capital gain distributions, realised gains short/long, fees; per period
  and total.
- `report.holdings` {`as_of`, `accounts`}: the `inv.holdings` data as a report (CSV
  export).

## Ledger effect

As in the actions table. Every investment transaction balances like any other.

## Settings

None beyond the module switch and each security's `basis_method`.

## Edge cases

- Buying the same security in two brokerage accounts: separate security accounts and
  separate lots.
- A split recorded after a sale: earlier sales are unaffected; later sales see the
  multiplied lots.
- Deleting a buy whose lot was consumed by a later sale: rejected with
  `insufficient_shares` (the replay would oversell) unless the sale is deleted first.
- Voided investment transactions are excluded from the replay.
- Editing an investment transaction with `txn.update` directly is rejected for its
  security splits (`use_investment_action`); payee/memo edits are allowed.
  `inv.update` {`txn`, new action arguments} replaces an action in one command.

## Acceptance tests

Common setup: security VTI (etf). Brokerage account funded by a 5,000.00 transfer from
Checking on 2025-03-01. Buy 10 VTI @ 200.00 + 5.00 commission on 2025-03-10 (lot A,
basis 2,005.00). Buy 5 @ 220.00 on 2025-11-20 (lot B, basis 1,100.00). Sell 12 @ 250.00
− 5.00 commission on 2026-04-15 (proceeds 2,995.00).

1. **FIFO:** the sale consumes A 10 (basis 2,005.00, proceeds 2,495.83, long-term,
   gain 490.83) and B 2 (basis 440.00, proceeds 499.17, short-term, gain 59.17);
   Realized Gains shows 550.00; lot B has 3 shares, basis 660.00.
2. **Average:** with basis method `average`, basis per share 207.00, sale basis
   2,484.00, gain 511.00: long-term (10 shares) basis 2,070.00, proceeds 2,495.83,
   gain 425.83; short-term (2) basis 414.00, proceeds 499.17, gain 85.17; remaining
   3 shares basis 621.00; the realised-gains row shows acquired "Various".
3. **Specific:** `lots` = B 5, A 7: B basis 1,100.00, proceeds 1,247.92, short-term
   gain 147.92; A basis 1,403.50, proceeds 1,747.08, long-term gain 343.58; total
   491.50; lot A keeps 3 shares, basis 601.50.
4. **Holding period:** a sale of 1 share of lot A on 2026-03-10 is short-term; on
   2026-03-11 long-term.
5. **Split and income (after test 1):** 2-for-1 split on 2026-06-01 → lot B 6 shares,
   basis 660.00. Ordinary dividend 15.00 on 2026-06-30 → cash +15.00, Dividends
   15.00. Reinvest qualified dividend 22.00 on 2026-09-30 as 0.1 share @ 220.00 →
   lot C 0.1 share, basis 22.00. Price 125.00 on 2026-10-16 → holdings on
   2026-10-19: 6.1 shares, basis 682.00, market value 762.50, unrealised 80.50; cash
   4,905.00.
6. **Backdated buy:** adding a buy of 2 @ 150.00 on 2025-01-05 to the test-1 book
   rewrites the sale in the same command: consumed X 2 (basis 300.00, proceeds 499.17,
   long-term) and A 10 (basis 2,005.00, proceeds 2,495.83, long-term); Realized Gains
   690.00, short-term 0.00; lot B keeps all 5 shares (1,100.00); one audit entry;
   undo restores the test-1 state exactly.
7. Selling 20 shares when 15 are held → `insufficient_shares` (held 15).
8. Return of capital 30.00 on lots B (3 sh, 660.00) and C (0.1 sh, 22.00) → B basis
   reduced by 29.03 to 630.97, C by 0.97 to 21.03.
9. Net worth on 2026-10-19 (test 5 book) counts Brokerage:VTI at 762.50, and the
   brokerage cash at 4,905.00.
10. With the module off, `inv.buy` fails with `module_disabled`; the security
    accounts and their transactions still show in registers and balances.
11. `price.import` with rows for VTI and an unknown symbol stores the VTI rows and
    warns once for the unknown symbol.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
