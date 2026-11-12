# Spec: Scheduled transactions

- **Area / module:** scheduling (`core`)
- **Spec writer:** Claude session 2026-10-19 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-19
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Standard | ISO 8601 calendar dates; proleptic Gregorian calendar | Month lengths, leap years, weekdays |
| 2 | Spec | `ledger-transactions.md` | Transactions are created through the normal rules (`txn.create`) |
| 3 | Spec | `ledger-audit-undo.md` | Every mutation is audited; undo is compensating |
| 4 | Design | MoneyKeep plan document, Phase 2 "Personal parity" | Recurring bills and income, reminders, forecast |
| 5 | Common knowledge | Household bills recur monthly, twice a month, every two weeks (pay periods), quarterly or yearly; banks do not post on weekends | Motivates the frequency list and weekend rule |

No target product was observed. Names, wording and screens are MoneyKeep's own.

## Purpose

People pay the same bills and receive the same pay on a pattern. A schedule describes
a transaction that repeats, so MoneyKeep can show what is coming, remind the user,
enter it when it happens (with or without confirmation), and project future balances.

## Behaviour

### Schedule fields

| Field | Rules |
|---|---|
| `id` | Typed id (`sch_…`), generated |
| `name` | 1–100 characters; defaults to the template payee |
| `template` | A transaction without id/date/status: `payee`, `number`, `memo`, `currency`, `splits` (as `txn.create`; same balancing rules, checked when the schedule is saved) |
| `recurrence` | See "Recurrence" |
| `start` | First nominal date (inclusive) |
| `end` | `never` (default), `{after: N}` occurrences (N ≥ 1, counting entered and skipped occurrences), or `{on: date}` (inclusive) |
| `weekend` | `none` (default), `before` (move a Saturday/Sunday date to the Friday before), `after` (to the Monday after) |
| `amount_kind` | `fixed` (default) or `estimate` (the user is expected to confirm the real amount when entering) |
| `auto_enter` | Boolean, default false |
| `remind_days` | 0–60, default 7: occurrences due within this many days from today are "due soon" |
| `status` | `active` or `paused` |
| `next_due` | Derived: the earliest occurrence not yet entered or skipped, or none when the schedule has ended |
| `created_at` | Timestamp |

Splits in the template must reference existing, non-placeholder, non-archived accounts
at save time. If an account is later archived, entering an occurrence fails with the
normal `txn.create` error and the schedule stays due.

### Recurrence

| `frequency` | Parameters | Nominal dates |
|---|---|---|
| `once` | — | `start` only |
| `daily` | `interval` n ≥ 1 | start, start + n days, … |
| `weekly` | `interval` n ≥ 1 | start, start + 7n days, … (every two weeks is `weekly` with n = 2) |
| `semi_monthly` | `days`: two distinct values from 1–31 or `last`, e.g. `[1, 15]`, `[15, "last"]` | Both days in every month, from `start` on |
| `monthly` | `interval` n ≥ 1; and either `day` (1–31 or `last`) or `weekday` + `week` (`week` 1–4 or `last`; e.g. second Tuesday, last Friday) | Every n-th month counting from `start`'s month |
| `yearly` | `interval` n ≥ 1; `month` 1–12 and `day` (1–31 or `last`) | Every n-th year from `start`'s year |

Quarterly is `monthly` with n = 3. Rules for computing dates:

1. **No drift.** The k-th occurrence is computed from `start` and k, never from the
   previous (possibly clamped or weekend-moved) occurrence. A monthly schedule on day 31
   gives Jan 31, Feb 28 (29 in leap years), Mar 31, Apr 30, …
2. **Clamping.** A `day` beyond the month's length means the month's last day.
   `last` is always the last day. Feb 29 yearly gives Feb 28 in common years.
3. **Monthly/yearly defaults.** If `day` and `weekday` are both absent, `day` is
   `start`'s day of month (and for yearly, `month` is `start`'s month).
4. **Start alignment.** Occurrences before `start` are never generated. For
   `semi_monthly` and `monthly` with `weekday`, the first occurrence is the first
   matching date on or after `start`.
5. **Weekend rule** applies last, to the clamped nominal date. The moved date is the
   occurrence's **due date**; its identity remains the nominal date (see below). If
   moving would place two occurrences on the same due date, both are kept.
6. Dates are calendar dates with no time zone. "Today" is the local date of the
   machine running the command, passed by front ends as `today` where relevant
   (commands accept an optional `today` argument so tests are deterministic).

### Occurrences

An occurrence is identified by `(schedule, nominal_date)`. Each has a state:
`upcoming`, `entered` (with the created transaction id), or `skipped`. Only the
entered/skipped states are stored; upcoming ones are computed.

- `schedule.upcoming` (non-mutating): arguments `from` (default today − 365 days),
  `to` (default today + 30), optional `account` (schedules with a split in that
  account), `include_paused` (default false). Returns, ordered by due date then
  schedule name, every occurrence not entered or skipped with due date in range (and
  every not-yet-handled occurrence before `from` is also returned, flagged, so an
  overdue item never silently disappears): schedule id and name, nominal and due
  date, payee, amount as seen from the account in question (or the first split's
  account when none is given), and a `when` of `overdue` (due before today),
  `due_soon` (today ≤ due ≤ today + `remind_days`) or `later`.
- `schedule.enter` (mutating): arguments `schedule`, `date` (nominal date; default the
  earliest pending one), optional overrides `entry_date` (default the due date),
  `payee`, `memo`, `number`, `splits` (a full replacement split list, e.g. the actual
  amount of an estimated bill). Creates the transaction through the same validation
  as `txn.create`, records the occurrence as entered, and links the transaction to
  the schedule (`txn.get` shows `schedule` and `occurrence`). Entering an occurrence
  that is already entered or skipped is rejected (`conflict`). One command, one audit
  entry; `history.undo` removes the transaction and returns the occurrence to
  pending.
- `schedule.skip` (mutating): `schedule`, `date` (nominal). Marks it skipped. Undo
  restores it.
- `schedule.run_due` (mutating): argument `today`. For every **active** schedule with
  `auto_enter`, enters each pending occurrence whose due date ≤ today, oldest first,
  with actor `schedule`. Returns what was entered and what failed (with the error),
  and never stops at the first failure. Running it twice the same day enters nothing
  the second time. Schedules without `auto_enter` are never entered automatically.
- Deleting a transaction that was entered from a schedule returns that occurrence to
  pending; voiding it does not.

### Managing schedules

| Command | Mutates | Notes |
|---|---|---|
| `schedule.create` | yes | All fields except id/derived; returns the schedule with `next_due` |
| `schedule.update` | yes | Replaces fields. Changing `recurrence` or `start` does not touch entered or skipped occurrences; pending occurrences are recomputed |
| `schedule.pause` / `schedule.resume` | yes | Paused schedules are excluded from `upcoming` (unless asked), `run_due` and forecasts |
| `schedule.delete` | yes | Removes the schedule; transactions already entered stay and lose their link |
| `schedule.get` / `schedule.list` | no | `list` filters: `account`, `status`; each item carries `next_due` and a plain-words `summary` |
| `schedule.from_txn` | yes | Creates a schedule whose template copies an existing transaction (payee, memo, splits), `start` = that date + one period of the given recurrence |
| `schedule.upcoming`, `schedule.enter`, `schedule.skip`, `schedule.run_due` | see above | |
| `schedule.forecast` | no | See below |

`summary` is a short English description built from the recurrence, e.g. "Once on
2026-11-01", "Every day", "Every 2 weeks on Friday", "Twice a month on the 1st and
15th", "Monthly on the 15th", "Monthly on the last day", "Every 3 months on the 1st",
"Monthly on the second Tuesday", "Yearly on March 31", followed by ", until
2027-06-30" or ", 12 times" when an end is set.

### Forecast

`schedule.forecast` (non-mutating): arguments `account`, `to` (date, max today + 3
years), `today`. Returns the account's current display balance (all posted splits up
to today, `ledger-balances.md`), then one row per pending occurrence of active
schedules touching the account, with due date between today+1 and `to` — plus overdue
ones, dated today — in due-date order, each with the amount in the account's
commodity and the projected display balance after it. Also returns `lowest` (the
minimum projected balance and its date).

## Ledger effect

Schedules are non-posting. Entering an occurrence posts an ordinary transaction.

## Settings

None. (The app may store per-user display preferences, not in the book.)

## Edge cases

- `end {after: 3}` with one occurrence skipped: two more are entered and the schedule
  has ended; `next_due` is none.
- `once` schedules end after their single occurrence is entered or skipped.
- An estimate entered with replacement splits that do not balance is rejected with
  the normal error; the occurrence stays pending.
- A schedule whose template's currency is not the base currency requires the
  `multi-currency` module, like any transaction.
- Paused for months, then resumed: past pending occurrences show as overdue; the user
  may skip them one by one (a bulk skip is a front-end loop over `schedule.skip`).
- The closing date: entering an occurrence with an entry date in the closed period is
  rejected like any `txn.create`.

## Acceptance tests

1. Monthly on day 31 from 2026-01-31: the first five nominal dates are 2026-01-31,
   02-28, 03-31, 04-30, 05-31; in 2028 February gives 2028-02-29.
2. Every 2 weeks from Friday 2026-10-02: 10-02, 10-16, 10-30, 11-13.
3. Semi-monthly `[15, "last"]` from 2026-02-01: 02-15, 02-28, 03-15, 03-31.
4. Monthly second Tuesday from 2026-10-01: 10-13, 11-10, 12-08.
5. Weekend `before`, monthly on day 1: 2026-11-01 is a Sunday, so it is due 10-30;
   the December occurrence is due 2026-12-01 (Tuesday), not 11-30.
6. `end {after: 3}`: after entering, skipping and entering, `next_due` is none and
   `upcoming` returns nothing.
7. `schedule.enter` on an estimate with replacement splits of 63.18 creates that
   transaction, `txn.get` shows the schedule link, and `next_due` advances one period.
   Entering the same nominal date again fails with `conflict`.
8. `history.undo` after `schedule.enter` removes the transaction and the occurrence
   is pending again; `history.redo` re-enters it with the same amount.
9. `run_due` with `today` = 2026-10-20 on an auto-enter monthly schedule starting
   2026-08-15 enters three transactions (08-15, 09-15, 10-15) with actor `schedule`;
   a second run the same day enters none.
10. Upcoming with `today` = 2026-10-20 and `remind_days` 7 for a bill due 10-15
    (not entered), 10-25 and 11-15: `overdue`, `due_soon`, `later`.
11. Forecast, today 2026-10-20, to 2026-12-05: Checking at 1,000.00; a monthly pay
    schedule of +2,000.00 on day 30 and a monthly rent schedule of −3,500.00 on day 1
    (weekend `none`). Rows: 10-30 pay → 3,000.00; 11-01 rent → −500.00; 11-30 pay →
    1,500.00; 12-01 rent → −2,000.00. `lowest` is −2,000.00 on 2026-12-01.
12. Deleting a transaction entered from a schedule makes that occurrence pending
    again; voiding it does not.
13. `summary` gives "Every 2 weeks on Friday" for test 2 and "Monthly on the last day,
    12 times" for a monthly `last` schedule with `end {after: 12}`.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-10-19) — decisions confirmed after implementation

- `schedule.preview` (non-mutating): given a recurrence, `start`, `end` and `weekend`,
  returns the `summary` and the next N due dates (default 3). Used by the schedule
  form so no date logic lives in the UI.
- An occurrence due exactly today is included in forecasts, dated today.
- `schedule.upcoming` rows also carry `money`: the money-account split (account,
  amount, category) for display.
- `schedule.run_due` writes one audit entry when it enters anything, and none when
  nothing was due.
