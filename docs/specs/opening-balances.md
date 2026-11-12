# Spec: Opening balance wizards

- **Area / module:** ledger setup (core; investment part needs the investments module)
- **Spec writer:** spec-writer session (Claude), with the owner
- **Date:** 2026-11-06
- **Status:** reviewed — approved by the owner 2026-11-06

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Spec | `ledger-accounts.md` (Opening Balances account, account kinds, revision 3) | The default other side of an opening entry |
| 2 | Spec | `connectivity.md` (account link: last balance and `balance-date`, `include_pending`, `sync_from`) | The trusted balance for feed accounts |
| 3 | Spec | `investments-core.md` (`inv.shares_in`, lots, `basis_method` average) | Holdings with basis against Opening Balances |
| 4 | Spec | `reconciliation.md` | Reconciled transactions and periods |
| 5 | Government | IRS Publication 551, *Basis of Assets* (12/2025) | Basis = cost; average basis for mutual fund shares (already cited by investments-core) |
| 6 | Owner decisions | Conversation 2026-11-06 | Defaults, offsets, entry points, investment choices below |

## Purpose

People starting a book get stuck on opening balances: a bank feed gives only 90 days, a
statement ends mid-month, a card balance is "owed" not "had", and investment cost basis
is hard to find. Many give up because they fear an early choice can't be changed. These
wizards turn one number the user trusts into a correct opening entry, explain each step
in plain words, show progress, and make clear that everything can be changed later.

Two wizards share one look and one checklist:

- **Opening balance** — bank, cash, credit card, loan and other money accounts.
- **Investment account opening balance** — brokerage and retirement accounts: cash,
  holdings and cost basis.

## Behaviour

### The working-backwards rule

The user gives a **trusted balance** and its **as-of date** (from the bank feed, a
statement, or the bank's website). For an opening date D:

    opening amount = trusted balance − (sum of the account's transactions dated after D
                                        and on or before the as-of date)

Transactions after the as-of date are not counted. Pending feed transactions are counted
only if the account link's `include_pending` is on (owner decision: follow the account's
setting; the wizard shows which applies: "Pending transactions are included, as set for
this account"). The opening entry is one ordinary transaction dated D (description
"Opening balance"), between the account and the **offset account**, marked as the
account's opening entry (`opening: true` on the transaction).

An account has at most one opening entry. Running the wizard again changes that entry
(amount, date, offset); it never adds a second one. An account set up before this spec
whose earliest transaction is against Opening Balances is treated as having that
transaction as its opening entry.

### Opening Balance wizard (money accounts)

For accounts of kind bank, cash, other asset, credit card, loan, other liability.

**Steps** (a step bar at the top: 1 Account · 2 Balance · 3 Start date · 4 Check · Done):

1. **Account.** Pick an account (preselected when started from an account). Shows its
   kind and whether it already has an opening entry ("Checking already has an opening
   balance of 1,200.00 on 2026-08-04. You can change it here.").
2. **Balance you trust.**
   - Feed account: "Your bank reports **2,341.10** on Nov 6, 2026. Is that what your
     bank shows?" **Yes, use it** / **Use a different balance**.
   - Otherwise (or a different balance): amount and as-of date, with hints: "From your
     latest statement or your bank's website."
   - Asset accounts ask "How much is in this account?"; credit cards, loans and other
     liabilities ask "How much do you owe?" — entered as a positive number; the wizard
     applies the sign. A card in credit is entered with the switch "The card owes me".
3. **Start date.** Two choices, the first preselected (owner decision 1):
   - **Keep everything imported** — D = the day before the account's earliest
     transaction (other than the opening entry). Text: "All N imported transactions
     stay. Your books start on Aug 3."
   - **Start at a statement** — the user gives a statement ending date and ending
     balance; D = that date and the statement balance is the trusted balance for the
     calculation. Transactions dated on or before D that are not reconciled are listed
     as "These N transactions are before your start date and will be removed (they are
     already in the starting balance)" with checkboxes (all on); unticking one keeps it
     and the amount is recalculated. Removed feed transactions are remembered so the
     feed does not bring them back (the import's existing duplicate memory).
   - With no transactions at all, D defaults to the as-of date and the step says
     "Nothing to work back from — your opening balance is simply your balance today."
4. **Offset and check.** Shows the computed entry in words: "Opening balance on Aug 3,
   2026: **1,876.45** in Checking, from Opening Balances." Then:
   - **Other side** (collapsed "Change" link): the suggestion with one sentence why.
     - All money accounts: **Opening Balances** — "This is what you had before you
       started using MoneyKeep. It is kept separate so your income and spending are
       not affected."
     - Business books (book has the business feature on): Opening Balances
       preselected, and **Owner's Equity** offered as the first alternative (owner
       decision 3) — "Choose Owner's Equity if this is money you put into the
       business." Owner's Equity is created on first use (equity, top level) if the
       book has none.
     - Loans: an extra question "Is this loan for something you own, such as a house
       or car?" **Yes** opens a small form (name, value) that also creates that asset
       account with its own opening entry, so net worth counts both.
     - Money someone owes you (other asset): Opening Balances, with a receivable
       account offered when the receivables feature is on.
     - Any account may be chosen from the full list.
   - **Check**: the account's balance as of the as-of date after this entry, compared
     with the trusted balance. Match → green "Matches your bank. ✓". The wizard cannot
     produce a mismatch by its own arithmetic; a mismatch arises only when the user
     unticked transactions or changed dates, and then the difference is shown with the
     **mismatch helper**: likely causes, each a link: pending transactions included or
     not (opens the account's feed setting), possible duplicates (same amount within 3
     days), transactions dated after the as-of date, a different as-of date.
   - **Reconciled check**: if the change moves a reconciled transaction or alters the
     balance of a finished reconciliation, a plain note: "Your reconciliation of Sep 30
     ended at 2,000.00. This change would make that period not add up. You can choose
     an earlier start date, or go ahead and the reconciliation will be marked to
     check." **Go ahead** / **Pick another date**. Never a dead end.
   - **Save** posts or updates the opening entry (one undoable action, audited).
5. **Done.** "Checking is set up. ✓" and, when the checklist has more, "Next:
   Visa — 2 of 5 accounts done" with **Set up Visa** / **Later**.

**Tone.** Every step has one line of reassurance where it helps, e.g. "You can change
this any time — the opening balance is an ordinary transaction." No accounting terms
without a plain explanation ("Other side" not "offset account"; "Opening Balances is
where the starting money comes from").

### Investment Account Opening Balance wizard

For accounts of subtype `brokerage`. Steps: 1 Account · 2 Cash · 3 Holdings · 4 Cost
basis · 5 Check · Done.

1. **Account.** As above, plus **"This is a retirement account (IRA, 401(k), Roth and
   similar)"** — stored on the account as `retirement: true` (editable in Edit account).
2. **Cash.** The working-backwards rule for the account's cash only (the account itself,
   not its security children), with the same start-date choice. Feed balances: the
   provider's cash figure when it reports one; otherwise the user types it.
3. **Holdings.** A table: symbol, name, shares on the start date.
   - SnapTrade accounts: prefilled from the provider's holdings; the user confirms.
   - Otherwise: typed rows, or **Paste** (lines "symbol, shares[, cost]" from a
     broker's positions page or a CSV; unknown symbols create securities after a
     confirm, as the price import does).
   - Working backwards for shares: shares on D = shares today − bought + sold (and
     split-adjusted) by transactions after D. The table shows both "today" and "on
     your start date" when they differ.
   - Scope (owner decision 4): stocks, ETFs, mutual funds, money market and other kinds
     the book already holds, plus options (held or written), bonds and crypto as defined
     in `investments-options-bonds-crypto.md` (updated 2026-11-06 when that feature
     shipped).
4. **Cost basis.** Per holding, one of three levels — nobody is forced to stop:
   - **Exact lots** — purchase date, shares, cost per lot (rows must add up to the
     shares).
   - **One total** — total cost, with **or average cost per share** (owner decision 3:
     the wizard multiplies); acquisition date chosen or **Various** (stored as the
     start date with `acquired_various: true`, shown as "Various").
   - **I don't know yet** — the holding is recorded at its market value on D (latest
     price on or before D; the user types a price if none) and the lot is flagged
     **Basis needed** (owner decision 2). The checklist and the holdings screen show
     "2 holdings need cost basis" with where to find it: "Your broker's cost basis or
     unrealized gain page, or last year's 1099-B." Supplying it later replaces the
     placeholder lots and moves the difference to Opening Balances (one undoable
     action). Reports that compute gains mark any figure involving a Basis needed lot
     with "basis not entered".
   - Retirement accounts (owner decision 1): this step is skipped and holdings are
     recorded at market value on D, flag not set, with a switch **"Enter cost basis
     anyway"** (useful for after-tax contributions).
   Posting uses `inv.shares_in` with the lots (offset Opening Balances, or Owner's
   Equity as for money accounts in business books).
5. **Check.** Cash + holdings at prices on the as-of date, compared with the total the
   user sees at the broker (prefilled from the feed when given). Shows per-holding value
   so a difference can be traced; same mismatch helper plus "a price is out of date"
   (opens the price for that security).
6. **Done.** As above.

Rerunning the investment wizard edits the opening cash entry and the opening
`inv.shares_in` transactions (one per holding, all marked `opening: true`).

### Setup checklist

- **Home → Attention** item "N accounts need an opening balance" (an account needs one
  when it has transactions or a feed link and no opening entry, and is not marked
  "Started from zero") and "N holdings need cost basis". Clicking opens the checklist.
- **Checklist** (a dialog, also from Accounts → ⋯ → Opening balances…): every
  balance-sheet account with status — Done (amount, date), Not set, Basis needed, or
  Started from zero. A progress bar "3 of 7 accounts set up". Each row: **Set up** or
  **Change**. A row's ⋯ offers **This account started from zero** (removes it from the
  list; undoable).
- Encouragement on progress: at 50% "Halfway there." and at 100% "All accounts are set
  up. Your net worth now reflects everything you own and owe."

### Where the wizards start (owner decision 5)

- **After linking a feed**: when the link step creates or links accounts, the last step
  offers "Set opening balances now (recommended)" / "Later"; Now opens the checklist
  limited to those accounts.
- **New account dialog**: the opening balance field stays for quick entry; beside it
  **Help me work it out…** opens the wizard for the new account after it is created.
  Brokerage accounts get the investment wizard instead.
- **Home checklist** as above.
- **Account ⋯ menu** (Accounts list and register): **Set opening balance…** (money
  accounts) or **Set opening holdings…** (brokerage).

### Nothing is locked

- The opening entry is an ordinary transaction: it appears in the register (labelled
  "Opening balance"), can be edited there, and edits keep the `opening` mark.
- Every wizard save is one undoable action.
- Help page "Opening balances" explains working backwards, the start-date choice, the
  other side, and that any of it can be changed later.

## Ledger effect

- Money accounts: one transaction per account, two splits (account, offset), marked
  `opening: true`. Removing pre-start transactions (statement start) deletes them as
  ordinary deletions in the same undoable action.
- Investment accounts: the cash opening transaction as above; one `inv.shares_in`
  transaction per holding marked `opening: true`; Basis needed lots carry
  `basis_needed: true`.
- Loan-for-an-asset creates the asset account and its opening entry.

## Settings

- Account fields: `retirement` (bool, brokerage only), `opening_none` (bool: started from
  zero). Lot fields: `basis_needed`, `acquired_various`. Transaction field: `opening`.
- No book settings.

## Acceptance tests

OB1. Feed checking: balance 2,341.10 as of Nov 6; imported transactions Aug 4–Nov 5
     summing +464.65; Keep everything → opening entry 1,876.45 dated Aug 3 from Opening
     Balances; the account's balance on Nov 6 is 2,341.10; Check shows the match.
OB2. Statement start: statement Sep 30, 2,000.00; 12 unreconciled transactions on or
     before Sep 30 are removed; opening entry 2,000.00 on Sep 30; a later sync does not
     bring the removed transactions back.
OB3. Credit card, owed 512.30 entered as positive: the card shows 512.30 owed; the entry
     credits the card. "The card owes me" 20.00 → the card shows a 20.00 credit.
OB4. Rerunning the wizard on an account with an opening entry changes that entry; the
     account still has exactly one; Undo restores the previous one.
OB5. Pending: with `include_pending` off, a pending feed transaction is not counted; on,
     it is; the step names which applies.
OB6. Business book: Other side offers Owner's Equity; choosing it creates Owner's Equity
     if missing and posts against it.
OB7. Loan "for something you own" with house value 300,000: the loan and a new asset
     "House" both get opening entries; net worth changes by 300,000 − loan.
OB8. A reconciled transaction before the chosen start date: the note appears with Go
     ahead / Pick another date; Go ahead marks the reconciliation to check.
OB9. Unticking a pre-start transaction shows a difference and the mismatch helper.
OB10. Checklist: 7 balance-sheet accounts with transactions, 3 with opening entries →
     "3 of 7"; marking one "started from zero" → "3 of 6"; Home Attention shows the
     count and opens the checklist.
IB1. Investment, SnapTrade holdings VTI 100 and FXAIX 52.3: prefilled; VTI exact lots,
     FXAIX average cost 145.20 → two `inv.shares_in` with the right basis; cash worked
     backwards as OB1.
IB2. A buy of 10 VTI after the start date: holdings step shows 110 today, 100 on start.
IB3. "I don't know yet" for a holding: recorded at market value on D, flagged Basis
     needed; the checklist shows "1 holding needs cost basis"; entering 9,000 later
     replaces the lot and the difference goes to Opening Balances; Undo restores.
IB4. Retirement account: basis step skipped; holdings at market value, not flagged;
     "Enter cost basis anyway" shows the step.
IB5. Check step total = cash + Σ shares × price on the as-of date; a stale price shows
     the "price is out of date" cause.
IB6. Paste "VTI, 100, 21000" and "XYZ, 5": VTI with total cost; XYZ unknown → confirm
     creates the security, basis level left to choose.

## Clean-room check

Written from MoneyKeep's own specs, IRS Publication 551 and the owner's decisions. No
product's screens, wording or code were consulted.
