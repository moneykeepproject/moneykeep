# Spec: Investments, part 3 — performance, wash sales, corporate actions (Phase 3c)

- **Area / module:** `investments`
- **Spec writer:** Claude session 2026-10-20 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-20
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Government | IRS Publication 550, *Investment Income and Expenses* | Wash sales (substantially identical securities bought within 30 days before or after a loss sale; loss disallowed and added to the replacement's basis; holding period carries over); nontaxable stock dividends and spin-offs (basis divided between old and new stock); return of capital beyond basis is capital gain; reorganisations: gain recognised up to the cash received, no loss recognised |
| 2 | Government | IRS Instructions for Form 8949 | Adjustment code **W** (wash sale) and the adjustment amount column |
| 3 | Government | IRS Form 8937, *Report of Organizational Actions Affecting Basis of Securities* | Issuers publish the basis allocation percentage for spin-offs and similar actions; the user copies it in |
| 4 | Standard | CFA Institute, Global Investment Performance Standards (GIPS) — time-weighted return; Modified Dietz approximation with geometric linking of sub-periods | TWR method |
| 5 | Textbook | OpenStax, *Principles of Finance* — internal rate of return, money-weighted vs time-weighted returns | MWR / XIRR definition |
| 6 | Vendor doc | `rust_decimal` documentation, `maths` feature (powd, ln, exp) | Decimal-only computation (floating point is denied in this codebase) |
| 7 | Spec | `investments-core.md`, `import-investments.md`, `reports.md` | Actions, lots, replay, holdings |

No target product was observed. Record-keeping only; MoneyKeep does not give tax or
investment advice, and the screens say so.

## Purpose

Answer "how are my investments really doing?" with standard return measures, warn
about wash sales before tax time, and record the corporate events (mergers,
spin-offs, symbol changes) that otherwise leave holdings and cost basis wrong.

## Behaviour

All commands require the `investments` module.

### 1. Performance

`report.performance` {dates (`reports.md` common rules), `accounts` (default all
brokerage accounts), `group` (`account` default, `security`, or `total`), `interval`
for the value series (`month` default)}.

**Valuation.** The value of a holding on a date is quantity × the latest price on or
before that date (CDs without a price at face, `import-investments.md` rev 4); with
no price, its remaining basis, and the row is flagged "valued at cost". An account's
value is its cash plus its holdings.

**External cash flows** (for `account` and `total` groups): money and securities
entering or leaving the investment account(s) from outside them — transfers to or from
non-investment accounts (bank, card, equity), `shares_in` / `shares_out` (valued at
market value on the date, else basis). Transfers between two included investment
accounts are internal and ignored. Income, fees, trades and corporate actions are
not external flows; their effect shows in the value.

For the `security` group, flows are the security's own cash: buys are money in,
sells and income received in cash are money out to the investor (reinvested income
is neither: it stays in the position).

**Measures per row:**

- `start_value` (on `from` − 1 day), `end_value` (on `to`), `net_flows` (in − out),
  `income` (dividends, interest, distributions in the period), `fees`,
  `gain` = end − start − net flows.
- **Money-weighted return (MWR, XIRR):** the rate r solving
  Σ CFᵢ · (1 + r)^(−dᵢ/365) = 0, where the flows are −start value, −each inflow,
  +each outflow, +end value. **Day positions:** a flow counts at the start of its
  date; the start value at the start of `from`; the end value at the end of `to`
  (i.e. the start of `to` + 1). dᵢ is days from the start of `from`. The period length
  for annualising (both measures) is `to` − `from` + 1 days.
  Shown **annualised**; for periods shorter than 365 days, the **cumulative** figure
  (1 + r)^(days/365) − 1 is shown instead, labelled "not annualised". Solved by
  bracketing and bisection in decimal arithmetic between −99.99 % and +10,000 % to an
  absolute tolerance of 1e-10 on r; no solution in range → "n/a" with the reason.
- **Time-weighted return (TWR):** split the period at month ends (and at `from`/`to`);
  for each sub-period, Modified Dietz:
  R = (EMV − BMV − CF) ÷ (BMV + Σ wᵢ·CFᵢ), wᵢ = (D − dᵢ + 1) ÷ D, with D the days in
  the sub-period and dᵢ the 1-based day of the flow (flows count from the start of
  their day); then TWR = Π(1 + Rₖ) − 1. Annualised as (1 + TWR)^(365/days) − 1 when
  the period is at least 365 days, else cumulative. A sub-period whose denominator is
  zero (nothing invested) contributes 0 %.
- All arithmetic in `rust_decimal` (the `maths` feature for powers); results rounded
  half away from zero to two decimals of a percent for display, with the unrounded
  value in the command result.

**Value series:** the account value at each interval end (and `to`), for the chart.

### 2. Wash sales (flags and Form 8949 adjustment amounts)

- **Substantially identical:** the same security, plus any securities the user has
  linked into a **wash group** (`security.link_wash_group` {securities, name}; no
  automatic judgement is made).
- For every realised **loss** portion (a consumed lot portion with gain < 0) in any
  account, find **replacement acquisitions**: buys, reinvestments and shares-in of a
  security in the same wash group, in **any** account, dated from 30 days before to
  30 days after the sale date inclusive, excluding the very shares sold, and not
  already used as replacements for an earlier loss sale (earliest sales first,
  earliest replacements first).
- Disallowed loss for a portion = |loss| × min(replacement quantity matched, portion
  quantity) ÷ portion quantity, rounded to cents; partial matches split as needed.
- **3c records but does not apply:** realised-gain rows carry `wash_sale: true`,
  `adjustment_code: "W"`, `adjustment` (the disallowed amount, positive), the
  replacement lot(s), and a note that the disallowed loss would be added to the
  replacement's basis. `report.realized_gains` shows a **Code** and **Adjustment**
  column and totals "Disallowed (wash sales)". Lot bases are **not** changed in 3c.
  (Applying the adjustment to bases and holding periods is planned; the setting key
  `investments.wash_sale_mode` is reserved with only value `flag`.)
- `inv.wash_check` {security, date, quantity}: before selling at a loss, lists buys in
  the previous 30 days and warns that buying again within 30 days after would make it
  a wash sale. The Sell form shows this when the preview's gain is negative.

### 3. Corporate actions

Each is a command creating ordinary transactions plus metadata, replayed by the lot
engine like other actions (`investments-core.md`), audited and undoable, rejected by
the closed-period rule as usual.

| Command | Effect |
|---|---|
| `inv.symbol_change` {security, date, new_symbol, new_name?} | Renames the security from that date; earlier reports show the symbol as it was then ("ABC (now XYZ)"). No transactions; the old symbol stays a lookup alias for imports |
| `inv.merger` {old_security, new_security, date, ratio_new, ratio_old, cash_per_share (default 0), new_share_value (per new share, required when cash > 0), mode: `tax_free` or `taxable`} | In every account holding the old security, per open lot: new quantity = old × ratio_new ÷ ratio_old. **`tax_free`, no cash:** each new lot keeps the old lot's basis and acquired date. **`tax_free` with cash (boot):** per lot, realised gain = cash + new shares × `new_share_value` − basis; recognised gain = min(cash, realised gain) when positive, else 0 (no loss recognised); new lot basis = basis − cash + recognised gain; recognised gain posts to Realized Gains with the lot's term. **`taxable`:** the old shares are sold at cash + new shares × value (a normal sale, gains and losses recognised) and the new shares are bought at their value with the merger date as acquired date |
| `inv.spinoff` {parent, new_security, date, new_per_parent (shares of new per parent share), allocation_percent (basis share moving to the new security, from the issuer's Form 8937), cash_in_lieu (optional {quantity, amount})} | Per parent lot: a new lot of quantity × new_per_parent with basis = round(lot basis × allocation %, 2) and the parent lot's acquired date; the parent lot keeps the rest. Cash in lieu sells that fractional quantity from the new lots (FIFO) for the amount |
| `inv.cash_in_lieu` {security, date, quantity, amount} | Sells a fractional quantity (e.g. after a split) for the cash, FIFO, gains recognised |
| `inv.return_of_capital` (amended) | When the amount exceeds the open basis, basis goes to zero lot by lot (pro rata as before) and the excess is a capital gain per lot (term by each lot's holding period), posted to Realized Gains and shown in realised-gains reports |

Values are in the base currency; transactions balance as usual: share movements in
security accounts carry value equal to the basis moved; recognised gains post to
Realized Gains; cash to the brokerage cash.

`import-investments.md` profiles may map broker texts to these actions (`merger`,
`spinoff`, `symbol_change`, `cash_in_lieu`); because the files rarely carry ratios
and percentages, imported rows of these kinds are always **needs review**, and the
review asks for the missing figures.

## Ledger effect

As described per action.

## Settings

`investments.wash_sale_mode` (reserved; only `flag`).

## Edge cases

- A loss sale whose replacement is in an IRA account is still flagged (the rule spans
  accounts); the note says the loss may be permanently disallowed for IRA purchases.
- A merger into a security the account already holds adds new lots alongside.
- A spin-off percentage of 0 or 100 is rejected; > 100 rejected.
- Performance over a period with no holdings: all figures "n/a".
- Undoing a corporate action replays later sales as usual (and is rejected if that
  would change a reconciled or closed transaction).

## Acceptance tests

1. **MWR, single flow:** 1,000.00 transferred in on 2025-01-01 and bought into a
   security; value 1,100.00 at the end of 2025-12-31; report 2025-01-01..2025-12-31
   → MWR 10.00 %, TWR 10.00 % (365 days, annualised = cumulative).
2. **MWR, two flows:** inflows 1,000.00 on 2025-01-01 and 1,000.00 on 2025-07-02, end
   value 2,200.00 at the end of 2025-12-31 → MWR 13.46 % (r = 0.134627 to six
   places).
3. **TWR vs MWR:** 2025-01-01 transfer 1,000.00, buy 100 sh @ 10.00; price 11.00 on
   2025-06-30; 2025-07-01 transfer 1,100.00, buy 100 sh @ 11.00; price 12.00 on
   2025-12-31; report 2025-01-01..2025-12-31 → end value 2,400.00, gain 300.00,
   TWR 20.00 % (June +10 %, December +9.0909 %, other months 0 %), MWR 19.61 %
   (r = 0.196090).
4. **Short period:** report 2025-01-01..2025-06-30 on test 3 (181 days) → TWR
   10.00 % and MWR 10.00 %, both labelled "not annualised" (annual r = 0.211913).
5. **Wash sale:** buy 10 XYZ @ 100.00 on 2026-01-05; sell 10 @ 80.00 on 2026-03-02
   (loss 200.00); buy 6 @ 82.00 on 2026-03-20 → the sale row has code W, adjustment
   120.00, replacement lot 2026-03-20 (6 sh); totals: loss 200.00, disallowed 120.00.
   A buy on 2026-04-01 would also count; a buy on 2026-04-02 would not.
6. **Wash group:** the replacement is a different security linked in a wash group →
   flagged; unlinked → not flagged.
7. **Spin-off:** parent lot 100 sh, basis 5,000.00, acquired 2024-05-01; 1 new per 2
   parent, allocation 12.5 % → new lot 50 sh, basis 625.00, acquired 2024-05-01;
   parent basis 4,375.00.
8. **Merger, tax-free stock-for-stock:** lot 100 sh basis 3,000.00, ratio 1 new per 2
   old → 50 new sh, basis 3,000.00, same acquired date; no gain.
9. **Merger with boot:** same lot, 50 new sh valued 70.00 each plus cash 10.00 per old
   share (1,000.00): realised gain 1,500.00, recognised 1,000.00, new basis 3,000.00.
   With basis 5,000.00 instead: realised −500.00, recognised 0.00, new basis 4,000.00.
10. **Taxable merger:** the same terms in `taxable` mode → sale proceeds 4,500.00
    (gain 1,500.00 recognised), new lot 50 sh basis 3,500.00 acquired on the merger
    date.
11. **Return of capital beyond basis:** lot basis 100.00, return of capital 150.00 →
    basis 0.00, capital gain 50.00 with the lot's term.
12. **Symbol change:** ABC → XYZ on 2026-06-01; holdings show XYZ; a realised-gains
    row for a 2026-05 sale shows "ABC (now XYZ)"; an import naming ABC after the
    change resolves to XYZ.
13. Undo of the spin-off in test 7 restores the parent lot to 100 sh, basis 5,000.00.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
