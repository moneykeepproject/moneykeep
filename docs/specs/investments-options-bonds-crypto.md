# Spec: Options, bonds and crypto

- **Area / module:** investments (extends `investments-core.md`)
- **Spec writer:** spec-writer session (Claude), with the owner
- **Date:** 2026-11-06
- **Status:** reviewed — approved by the owner 2026-11-06

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Spec | `investments-core.md` | Securities, prices, actions, lots, holdings, Realized Gains, Form 8949 data |
| 2 | Government | IRS Publication 550, *Investment Income and Expenses* | Options: purchased option cost added to the stock's basis on exercise of a call, subtracted from the amount realised on exercise of a put; premium on a written call added to the amount realised when assigned, on a written put subtracted from the basis of the stock bought; an option that lapses is treated as sold on the expiration date (holder: loss; writer: short-term gain). Bonds: accrued interest paid at purchase is not income to the buyer and offsets the next interest received; gain or loss at redemption |
| 3 | Government | IRS Notice 2014-21 and the FAQs on virtual currency transactions (irs.gov) | Digital assets are property; basis and holding period rules as for other property; rewards are income at fair market value when received; paying a fee with a digital asset is a disposal |
| 4 | Government | IRS Instructions for Form 1099-DA and Rev. Proc. 2024-28 | Brokers report digital asset sales from 2025; basis is tracked per wallet or account |
| 5 | Owner decisions | Conversation 2026-11-06 | Options mostly bought, occasionally sold, gains for taxes; individual bonds with interest and maturity; crypto on exchanges with gains for taxes, as holdings in investment accounts; gain at sale or maturity only (no premium/discount amortisation); wash sales not extended; prices typed |

## Purpose

Hold options, individual bonds and crypto in investment accounts with correct cost basis
and gains for taxes, using the same accounts, lots, holdings screen and reports as
stocks. Prices are typed (or imported with the existing price CSV import).

## Behaviour

### New security kinds and fields

`kind` gains `option` and `crypto` (`bond` exists). New fields by kind:

| Kind | Fields |
|---|---|
| `option` | `underlying` (a security of kind stock or etf; created if missing), `right` (`call` or `put`), `strike` (> 0, up to 4 decimals), `expiration` (date), `multiplier` (default 100). Symbol is generated: `<UNDERLYING> <YYMMDD> <C|P><strike>` e.g. `AAPL 261218 C200` (the symbol rule allows spaces for this kind only; up to 24 characters); name "AAPL Dec 18 2026 200 Call" |
| `bond` | `face_per_unit` (default 1,000), `coupon_rate` (percent, up to 4 decimals; 0 for zero-coupon), `payments_per_year` (1, 2, 4 or 12; default 2), `maturity` (date), `first_payment` (optional date; default derived back from maturity), `tax` (`taxable`, `municipal`, `treasury`), optional `cusip` |
| `crypto` | none beyond core; symbol letters/digits up to 12 (e.g. BTC, ETH, SOL) |

Quantities: options in contracts (whole numbers), bonds in units of `face_per_unit`
(whole numbers), crypto up to 8 decimals (core allows 9).

Prices: option price per share of underlying (value = contracts × multiplier × price);
bond price as percent of face (value = units × face_per_unit × price ÷ 100); crypto per
coin. Typed in the Prices dialog or imported with the existing price CSV.

### Short positions (written options only)

An option holding may have a **negative** quantity (written, "sold to open"). Its lots
are **short lots**: opened date, contracts, premium received (net of commission). Short
positions are allowed for kind `option` only; other kinds keep the core rule (no short
sales).

### Option actions

Each is a command creating one ordinary transaction, like core actions, undoable.

| Command | Effect |
|---|---|
| `inv.option_buy` (buy to open) | as `inv.buy`: cash −(contracts × multiplier × price + commission); creates a lot. If the account holds short lots of that option, it closes them first (buy to close, below) |
| `inv.option_sell` (sell to close) | as `inv.sell` with the option's multiplier; gain/loss to Realized Gains; holding period from the lot |
| `inv.option_write` (sell to open) | cash +(premium − commission); option quantity −contracts; creates a short lot with that premium |
| `inv.option_cover` (buy to close) | cash −(cost + commission); closes short lots FIFO; gain = premium − cost, **always short-term** (source 2) |
| `inv.option_expire` | for a long position: lots close with proceeds 0 → loss of the basis, dated expiration, holding period from the lot; for a short position: lots close with cost 0 → gain of the premium, short-term |
| `inv.option_exercise` (long) | call: buys `contracts × multiplier` shares of the underlying at the strike; the new stock lot's basis = strike × shares + option basis + commission; acquired date = exercise date. Put: sells those shares of the underlying at the strike (core lot selection applies); proceeds = strike × shares − option basis − commission. The option lots close with no gain of their own |
| `inv.option_assigned` (short) | call: sells the underlying shares at the strike; proceeds = strike × shares + premium − commission. Put: buys the underlying at the strike; basis = strike × shares − premium + commission. The short lots close with no gain of their own |

Expiry reminder: an option holding still open after its expiration date shows in Home →
Attention "AAPL Dec 18 2026 200 Call expired — record what happened" opening a small
dialog: **Expired worthless** / **Exercised** / **Assigned** / **Closed earlier (sold or
bought back)**. Each runs the matching action.

### Bond actions

| Command | Effect |
|---|---|
| `inv.bond_buy` | `units`, `price` (% of face), `accrued_interest` (default 0), `commission`. Cash −(units × face × price/100 + accrued + commission); lot basis = principal + commission (not the accrued interest). The accrued interest is posted as a debit to the bond's interest account (below), labelled "Accrued interest paid", so it offsets the next coupon in that year's interest (source 2) |
| `inv.bond_sell` | as `inv.sell` with price as % of face; optional `accrued_interest` received → credit to the interest account |
| `inv.bond_interest` | coupon received: cash +amount; credit to the interest account |
| `inv.bond_redeem` | at maturity or when called: `units` (default all), `price` (default 100). Cash +units × face × price/100; lots close; gain/loss = proceeds − basis, holding period from the lot |

Interest accounts by `tax`: `Interest` (taxable), `Tax-exempt Interest` (municipal),
`US Treasury Interest` (treasury) — created on first use like core income accounts, so
the tax summary can show them apart.

Coupon schedule: creating a bond offers **Add coupon reminders**: a scheduled
transaction (reminder, not auto-enter) for each payment date until maturity, amount
units × face × coupon_rate ÷ 100 ÷ payments_per_year, entering `inv.bond_interest`.
Maturity adds an Attention item on the date: "Treasury 4.25% 2027 matured — record the
redemption".

Premium and discount: **not amortised** (owner decision). Basis is the purchase cost;
the difference is gain or loss at sale or redemption. Shown on the holding as "Bought
above (below) face by X".

### Crypto

Crypto holdings use the core actions (`inv.buy`, `inv.sell`, `inv.shares_in`,
`inv.shares_out`) with quantities up to 8 decimals, plus:

| Command | Effect |
|---|---|
| `inv.crypto_fee` | a fee paid in coin: `security`, `quantity`, `value` (market value; default quantity × latest price). Disposes that quantity (gain/loss = value − basis of the lots used) and posts `Investment Fees` +value |
| `inv.crypto_swap` | one coin for another in the same account: `from`, `from_quantity`, `to`, `to_quantity`, `value` (market value of what was received), optional fee. Posts a sale of `from` with proceeds = value and a buy of `to` with cost = value, in one transaction |
| `inv.crypto_reward` | staking or other rewards: `security`, `quantity`, `value`; income account `Crypto Rewards` (created on first use) credited with value; creates a lot with basis = value (source 3) |
| `inv.transfer_lots` | moving coins (or any security) between two of the user's investment accounts, e.g. an exchange to another exchange: lots move with their acquired dates and basis; no gain. Optional network fee in coin handled as `inv.crypto_fee` in the same transaction |

Each exchange is its own investment account, so basis is tracked per account (source 4).

### Holdings, reports and taxes

- Holdings screen: options show underlying, right, strike, expiration and days left;
  short positions show negative contracts and their value as a liability within the
  account's total. Bonds show face, coupon, maturity and price as % of face. Crypto
  shows 8 decimals.
- Realized gains and the Form 8949 export include the new kinds with correct
  short/long-term; written-option gains are always short-term; expired options use the
  expiration date as the sale date. Description: option name, bond name with face, or
  "0.25 BTC".
- Wash-sale flags (`investments-advanced.md` §2) are unchanged and do not consider the
  new kinds (owner decision).

### Entering them

The investment register's action menu gains **Option…**, **Bond…** and **Crypto…** groups
with the actions above; forms ask in plain words ("Did you buy or sell?", "Opening a new
position or closing one?") and preview the transaction and any gain before saving. The
opening balance wizard (`opening-balances.md`) accepts the new kinds in its holdings
step (options: long or short; bonds: units and price; crypto: 8 decimals).

## Ledger effect

As in each table: ordinary transactions between the brokerage cash, security child
accounts, Realized Gains, the interest accounts, Investment Fees and Crypto Rewards.
Short option positions are a security child account with negative quantity and a
credit value (premium received).

## Settings

None. Security fields as above.

## Acceptance tests

OP1. Buy 2 AAPL 200 calls at 5.10, commission 1.30 → cost 1,021.30; sell both at 7.00,
     commission 1.30 → proceeds 1,398.70, gain 377.40, short-term.
OP2. Bought call expires worthless: loss = basis, sale date = expiration; holding period
     over a year → long-term.
OP3. Write 1 put at 2.00 (commission 0.65) → cash +199.35, quantity −1; buy to close at
     0.50 (0.65) → cost 50.65; gain 148.70 short-term.
OP4. Written put assigned at strike 50: 100 shares bought, basis 5,000 − 199.35 + 0 =
     4,800.65; the option position closes with no gain.
OP5. Exercise 1 long call (basis 300, strike 100): 100 shares, basis 10,300.
OP6. Exercise 1 long put (basis 200, strike 40) against 100 shares bought at 50:
     proceeds 4,000 − 200 = 3,800; loss 1,200 on the shares.
OP7. An open option past expiration shows the Attention item; Expired worthless runs
     `inv.option_expire`.
BD1. Buy 5 bonds at 98.50 with 42.10 accrued interest and 5.00 commission: cash
     −4,972.10; basis 4,930.00; the interest account shows −42.10; the first coupon of
     106.25 nets interest for the year to 64.15.
BD2. Redeem at maturity at 100: proceeds 5,000; gain 70.00, long-term if held over a year.
BD3. Municipal bond coupons go to Tax-exempt Interest; Treasury to US Treasury Interest.
BD4. Add coupon reminders creates one reminder per payment date to maturity with the
     right amount.
CR1. Buy 0.12345678 BTC for 8,000 then sell 0.05 for 4,000: FIFO basis 3,240.00
     (rounded from 8,000 × 0.05 ÷ 0.12345678); gain 760.00.
CR2. Swap 1 ETH (basis 2,000) for 50 SOL worth 3,000: gain 1,000 on ETH; SOL lot basis
     3,000.
CR3. Reward 0.1 ETH worth 300: Crypto Rewards +300; a lot with basis 300.
CR4. Transfer 0.05 BTC between exchanges: lots keep acquired dates and basis; no gain.
CR5. Fee of 0.0001 BTC worth 6.00 with basis 4.00: gain 2.00; Investment Fees +6.00.
TX1. Form 8949 export lists OP1, OP2, OP3, BD2 and CR1 with correct dates, proceeds,
     basis and term.

## Clean-room check

Written from MoneyKeep's own specs, IRS publications and the owner's decisions. No
product's screens, wording or code were consulted.

## Revision 2 (2026-11-06) — editing the new actions; rewards and bond interest in income (owner request)

1. **Editing from the Activity tab.** Clicking (or Enter on) a transaction recorded by any
   action in this spec opens the same form that recorded it (Option…, Bond… or Crypto…),
   with the action chosen and every field filled from the transaction, titled "Edit …".
   The preview shows the transaction as it will be after the change, and any gain.
   **Save** replaces the action through `inv.update` (one undoable action), so lots and
   gains are recomputed. Choices that would turn it into a different action (e.g.
   Opening ↔ Closing, Buy ↔ Sell) are allowed and handled by the same replace. Options
   created by exercise or assignment open the exercise/assignment form; the stock side
   is not edited separately. A transaction the form cannot represent (edited by hand in
   the register) opens the register's split view as today, with a note "This
   transaction was changed by hand, so it opens as splits."
2. **Investment income report.** `report.investment_income` adds rows, each only when
   non-zero in the period: **Crypto rewards** (the Crypto Rewards account or the account
   named on the action), and interest split as **Interest**, **Tax-exempt interest** and
   **US Treasury interest** (the bond interest accounts; accrued interest paid shows as a
   negative within its row). Totals include them. The report's CSV and the tax summary
   use the same row names.

Acceptance tests:
OP8. Write a put (OP3), open it from Activity: the form shows Sold contracts / Opening,
     1 contract at 2.00, commission 0.65; change the price to 2.50 and save → cash
     +249.35; Undo restores 199.35.
BD5. Open BD1's buy from Activity, change accrued interest to 40.00 → the interest
     account shows −40.00 and the coupon nets 66.25.
CR6. Edit CR2's swap so SOL received is worth 3,200 → gain on ETH 1,200; SOL basis 3,200.
CR7. Investment income for the year of CR3 and BD1 shows Crypto rewards 300.00 and
     Interest 64.15 (106.25 coupon − 42.10 accrued), included in the total.

## Revision 3 (2026-11-07) — coin fees in the Fees row

Investment Fees posted by `inv.crypto_fee`, by a swap's fee and by a transfer's network
fee are included in `report.investment_income`'s **Fees** row (and the Tax summary
section). Test CR8: CR5's fee of 6.00 shows in Fees for its month.
