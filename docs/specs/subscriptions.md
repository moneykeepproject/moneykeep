# Spec: Subscription audit

- **Area / module:** `subscriptions` (new optional module, off by default; Features list)
- **Spec writer:** spec-writer session (Claude), with the owner
- **Date:** 2026-11-07
- **Status:** reviewed — approved by the owner 2026-11-07

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Spec | `import-rules.md`, `import-pipeline.md` | Payee normalisation used for grouping |
| 2 | Spec | `scheduled-transactions.md`, `business-purchases.md` | Turning a subscription into a reminder or bill |
| 3 | Spec | `dashboards.md` | Attention items and a new Home block |
| 4 | Owner decisions | Conversation 2026-11-07 | Own screen + Home block; suggestions confirmed by the user; alert threshold; recurring bills section; colour thresholds |

## Purpose

Find the payments that repeat on a schedule, keep a price history for each, explain
changes with notes, and catch the money leaks: price rises, charges after cancelling,
renewals that slip by, forgotten trials and duplicates. Recurring bills whose amount
varies (electric, phone, insurance) are listed separately without price alerts.

## Behaviour

### Detection

- Runs over every bank, cash, credit-card and other-liability account, on all history,
  when the screen opens and after each import or feed sync (results cached; a change in
  transactions marks the cache stale).
- **Grouping:** outgoing payments by normalised payee (the pipeline's normalisation;
  the payee rule target when a rule renamed it) and account.
- **Cadence:** intervals between consecutive charges are tested against weekly (7 ± 2
  days), every two weeks (14 ± 3), monthly (same day of month ± 4 days, month-end
  allowed), quarterly (≈91 ± 7), every six months (≈182 ± 10) and yearly (≈365 ± 14). A
  group qualifies when its last **3** charges (monthly and shorter), or last **2**
  (quarterly and longer), fit one cadence. A missed single period inside the history does
  not break a match.
- **Kind:** a group is a **subscription candidate** when the amounts of its qualifying
  charges are equal or differ only by price changes (each amount repeats at least twice,
  or the last charge differs from the one before by a single step); otherwise it is a
  **recurring bill candidate** (amounts vary from charge to charge).
- **Suggested** list: candidates not yet answered, each with payee, account, cadence,
  last amount, number of charges and first date. Buttons **It's a subscription**, **It's a
  recurring bill**, **Not recurring**. Answers are remembered per payee and account; a
  "Not recurring" payee is never suggested again (the answer is undoable and listed in
  the screen's ⋯ → Dismissed suggestions).
- **Add by hand:** name, payee to match (equals / contains on the normalised payee),
  account (or any), cadence, price, next charge date. Useful before three charges exist
  (new subscription, free trial).

### A subscription (or recurring bill)

| Field | Notes |
|---|---|
| `name` | Defaults to the payee |
| `kind` | `subscription` or `bill` |
| `match` | payee rule (equals / contains) and account (optional) |
| `cadence` | as above |
| `price` | current expected amount (subscriptions); bills show average and last |
| `next_charge` | computed from the last charge and cadence; editable |
| `category` | an expense account (defaults from the transactions) |
| `status` | `active`, `paused`, `cancelled` (with `cancelled_on`), `trial` (with `trial_ends`) |
| `used_by` | free text (e.g. a family member) |
| `link` | optional URL (account or cancellation page); opened in the browser only after a confirm showing the address |
| `notes` | free text |
| `reviewed_on` | date of the last review |

Charges are the matching transactions (linked, not copied). A transaction can belong to
one subscription only; the detail view lists them and allows removing a wrong one or
adding a missed one.

**Price history** (subscriptions): every change in the charged amount between
consecutive charges is a history row: date, old price, new price, change (amount and %),
**reason** note (editable, empty at first). Changes below the alert threshold are still
recorded but marked "small". Bills keep no price history; they show a chart of their
amounts over time and the yearly total versus last year.

### Change colours (owner decision)

Settings → Subscriptions (module settings):
- **Alert on price changes of at least** `1` % **or** `0.50` (both editable; a change
  alerts when it is at or over either). Smaller changes are recorded as "small".
- **Colour bands** for price-change percentages: a list of bands, each with a threshold
  percentage and a colour, by default **20 % → yellow (#F2C94C)** and **40 % → red
  (#D64545)**. Changes below the first band use the normal text colour. The user can
  edit thresholds and colours (colour chooser with a few presets and a hex field), add a
  band or remove one (at least zero bands). A change takes the colour of the highest band
  it reaches. Decreases are shown in the positive (green) colour of the theme and are not
  banded.
- Colour is never the only signal: banded changes also show an up arrow and the
  percentage, and the chosen colour is shown with enough contrast (the text uses the band
  colour as a background chip with dark or light text chosen automatically for 4.5:1).

### Alerts (Home → Attention, dismissible like other notices)

1. **Price went up:** "Netflix: 15.49 → 17.99 (+16.1%). Add a reason?" — opens the history
   row's note. Shown once per change, at or above the alert threshold; the chip uses the
   band colour.
2. **Charged after cancelling:** a matching charge dated after `cancelled_on` — "Hulu
   charged 7.99 on Nov 3 after you cancelled it on Oct 20." Highest priority (listed
   first, red chip regardless of bands).
3. **Expected charge missing:** no matching charge within the cadence tolerance after
   `next_charge` for an active subscription — "Spotify's charge due Nov 1 hasn't
   appeared. Cancelled, or did the card change?" Actions: **Mark cancelled**, **Still
   active**.
4. **Renewal coming up:** yearly (and six-monthly) subscriptions N days before
   `next_charge` (setting, default 14) — "Amazon Prime renews on Dec 2 for 139.00."
5. **Trial ending:** 3 days before `trial_ends` (setting, default 3).
6. **Possible duplicate:** two active subscriptions with the same normalised payee on
   different accounts, or three or more active in the same category — informational,
   dismissible.

### Screen (Features → Subscriptions)

- Tabs **Subscriptions**, **Recurring bills**, **Suggested (N)**.
- Subscriptions list: name, cadence, price, monthly equivalent, next charge, status,
  last change (coloured chip). Totals: per month and per year (active only, monthly
  equivalent = price × charges per year ÷ 12), by category, and versus the same date a
  year ago. **Biggest increases** (last 12 months) panel.
- Detail: fields, charges, price history with reason notes, actions **Mark cancelled**,
  **Pause**, **Make a reminder** (a scheduled transaction, reminder only) or **Make a
  bill** (when the bills module is on).
- **Review** button: steps through active subscriptions one at a time, oldest
  `reviewed_on` first, with **Keep**, **Cancel** (sets status and records the yearly
  amount as saved), **Downgrade…** (enter the new price; recorded with reason
  "Downgraded"), **Skip**; a progress bar and a running "You've saved X a year" total
  (sum of yearly amounts of subscriptions cancelled or downgraded in reviews, last 12
  months).

### Home block

Dashboard block type **Subscriptions** (not on Home by default; added from the block
gallery): monthly total, next three charges, and the count of open alerts.

## Ledger effect

Non-posting. Subscriptions, answers, history notes and settings are book documents,
audited and undoable. "Make a reminder/bill" uses the existing commands.

## Settings

Module `subscriptions`: `alert_percent` (1), `alert_amount` (0.50), `bands`
([{percent: 20, color: "#F2C94C"}, {percent: 40, color: "#D64545"}]), `renewal_days`
(14), `trial_days` (3), `make_bill_as` (`scheduled_bill`; revision 2).

## Acceptance tests

SA1. Netflix charged 15.49 on the 5th for 4 months, then 17.99 for 2 → suggested as a
     monthly subscription; after confirming, price 17.99, one history row +2.50 (+16.1%),
     no band colour; an Attention item asks for a reason; adding a reason shows it in
     the history.
SA2. A change of +25% shows the yellow chip; +45% red; with the bands edited to 10 %
     green and 30 % purple, +25% shows green and +45% purple.
SA3. A change of 0.30 on 15.49 (1.9%) alerts; 0.10 (0.6%, under 0.50) is recorded as
     small without an alert.
SA4. Electric 82.10, 95.40, 120.55 monthly → suggested as a recurring bill; confirming it
     puts it under Recurring bills with no price history and no price alerts.
SA5. Two charges a year apart of 139.00 → suggested yearly; renewal alert 14 days before
     the next date.
SA6. A subscription marked cancelled on Oct 20 with a matching charge Nov 3 → the
     "charged after cancelling" item, listed first.
SA7. An active monthly subscription with no charge 5 days after its next date → the
     missing-charge item; Mark cancelled sets status cancelled on that date.
SA8. "Not recurring" on a suggestion hides it permanently; Undo brings it back.
SA9. Review: three subscriptions; Cancel one of 9.99 monthly → "You've saved 119.88 a
     year"; Downgrade another from 22.99 to 11.99 → saved total 251.88; history row
     reason "Downgraded".
SA10. Monthly equivalent: 139.00 yearly → 11.58; weekly 5.00 → 21.67.
SA11. Weekly charges of 5.00 on the same weekday for 3 weeks are suggested weekly;
      charges 9, 40 and 75 days apart are not suggested.

## Clean-room check

Written from MoneyKeep's own specs and the owner's decisions. No product's screens,
wording or code were consulted.

## Revision 2 (2026-11-07) — owner decisions on the implementer's questions

### Make a bill: a setting

- Module setting `make_bill_as`:
  - **`scheduled_bill`** (default) — **Make a bill** creates a schedule
    (`scheduled-transactions.md`) that, when entered, creates the bill. Nothing posts
    until then. The schedule repeats on the subscription's cadence, `auto_enter` is off
    and the amount is the subscription's price (bills: `amount_kind` `estimate` with the
    average). It needs the bills module; without it the action is hidden, as before.
  - **`bill_now`** — today's behaviour: opens the bill editor pre-filled; saving posts a
    bill to Accounts Payable.
- Settings → Subscriptions shows it as "Make a bill creates: **A repeating bill in
  Scheduled (posts when you enter it)** / **A bill now (posts to Accounts Payable)**".
- **Schedules that create a bill** (addition to `scheduled-transactions.md`): a schedule
  may have `creates: "bill"` with vendor, terms and bill lines instead of splits. It
  appears in Scheduled and Coming up like any schedule, marked "Bill". Entering it opens
  the bill editor pre-filled for that due date, so an estimated amount can be corrected;
  saving goes through `bill.create`. Skip and the reminder days work as for other
  schedules. Definition files may not carry `creates: "bill"` in this version.

### Single-step price changes count as a subscription

- Kind (replaces the second sentence of the Kind rule): a group is a **subscription
  candidate** when its qualifying charges form runs of equal amounts in which every run
  except the first and the last has at least 2 charges. Two charges with different
  amounts (139.00 then 149.00 yearly) are therefore a subscription with one price change.
  Otherwise (amounts vary from charge to charge) it is a **recurring bill candidate**.

### Tests

SA12. 139.00 then 149.00 a year apart → suggested as a yearly subscription with one
      history row +10.00 (+7.2%).
SA13. `make_bill_as` default: Make a bill on Netflix creates a monthly schedule marked
      Bill for 17.99, nothing posted; entering it opens the bill editor and saving
      creates the bill. With `bill_now`, Make a bill opens the bill editor directly.
