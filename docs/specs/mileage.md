# Spec: Mileage tracking

- **Area / module:** `mileage` (new module)
- **Spec writer:** Claude session 2026-11-05 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-11-05
- **Status:** approved by the owner as described (2026-11-05)

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Government | IRS newsroom, "IRS sets 2026 business standard mileage rate at 72.5 cents per mile, up 2.5 cents" (IR-2025-128, Notice 2026-10) | 2026 rates from 1 January: business 72.5¢, medical and moving 20.5¢, charitable 14¢ |
| 2 | Government | Internal Revenue Bulletin 2026-29 (modifying Notice 2026-10), as reported by Accounting Today, "IRS quietly raises mileage rates due to inflation" | From 1 July 2026: business 76¢, medical and moving 23.5¢, charitable 14¢ (fixed by statute, 26 U.S.C. §170(i)) |
| 3 | Government | IRS Publication 463 (Travel, Gift, and Car Expenses), recordkeeping chapter | A mileage record shows the date, the miles, the destination or route and the purpose; total miles for the year per vehicle (odometer readings) | Used only to choose which fields a trip keeps; MoneyKeep gives no tax advice |
| 4 | Spec | `expenses.md` (reimbursements, expense report), `mobile-capture.md` (phone app, server, outbox), `chart-presets.md` (Tax summary), `ledger-transactions.md`, `app-desktop.md`, `ui-slint.md` | |
| 5 | Owner decisions | 2026-11-05: purposes business, medical, charity (church) with their rates; a log for taxes and reimbursement through expense reports; miles typed or odometer start/end, round trip, saved trips, no GPS; several vehicles; on the phone the Expense tab toggles to Mileage | |

No target product was observed. Design and wording are MoneyKeep's own.

## Purpose

Keep a mileage log good enough for a tax return and for paying people back: every
trip with its date, vehicle, route, purpose and miles; the standard rate that applies;
totals per vehicle and purpose for the year; and, for trips someone should be repaid
for, the same reimbursement path as out-of-pocket expenses. Trips can be entered on
the desktop or on the phone (through the capture server, over Wi-Fi or Cloudflare).

## Behaviour

### Module

Catalog entry `mileage`: "Mileage: log trips by vehicle and purpose, apply standard
mileage rates, and include reimbursable trips in expense reports". No dependencies;
reimbursement uses `expenses` when that module is on (otherwise trips are log-only and
the Reimbursable switch is hidden). When off, its screens and commands are hidden;
data is kept.

### Vehicles

| Field | Rules |
|---|---|
| `name` | 1–60 characters, unique (case-insensitive), e.g. "2019 Honda CR-V" |
| `plate` / `note` | optional text ≤ 100 |
| `active` | default true; inactive vehicles are hidden from pickers but kept for history |
| `odometer` | per tax year: `start` (reading on 1 January or when placed in service) and `end` (31 December or when retired), whole or one-decimal miles, `end` ≥ `start` |

At least one vehicle is needed before a trip can be saved; the first trip offers
**Add vehicle** inline. With one active vehicle it is preselected.

### Purposes and rates

- A trip's `purpose_kind` is one of **business**, **medical**, **moving** (shown only
  when turned on in settings; it applies only to certain military and intelligence
  community moves), **charity**, or **personal** (logged for the year's total miles;
  no rate, never reimbursed by default).
- A **rate table** holds rows {`kind`, `from` date, cents per mile with up to 1
  decimal}. The rate for a trip is the row of its kind with the latest `from` on or
  before the trip date. Shipped rows (from Sources 1–2, plus 2025 for older trips):

| From | Business | Medical | Moving | Charity |
|---|---|---|---|---|
| 2025-01-01 | 70.0 | 21.0 | 21.0 | 14.0 |
| 2026-01-01 | 72.5 | 20.5 | 20.5 | 14.0 |
| 2026-07-01 | 76.0 | 23.5 | 23.5 | 14.0 |

- Settings → Mileage → **Rates** lists the table; the user can add, edit and delete
  rows (audited, undoable). A trip stores the rate it used when saved
  (`rate_cents`); changing the table later does not change saved trips unless the user
  runs **Re-apply rates…** for a date range (shows how many trips change and the
  total difference, then applies; one undo step).
- MoneyKeep does not fetch rates from the internet; the notes under the table say
  where rates are published and that the user should add new rows when the IRS
  announces them.

### Trips

| Field | Rules |
|---|---|
| `date` | required |
| `vehicle` | a vehicle, required |
| `purpose_kind` | business / medical / moving / charity / personal; default from setting `mileage.default_purpose` (business) |
| `purpose` | text ≤ 500, required unless personal (the business or charitable purpose, e.g. "Food pantry delivery for St. Mark's") |
| `from_place`, `to_place` | text ≤ 100 each; `to_place` required unless personal |
| `miles` | positive, up to 1 decimal, ≤ 2,000 per trip |
| `odometer_start`, `odometer_end` | optional; when both are given, `miles` = end − start (end > start); when only miles is given they stay empty |
| `round_trip` | bool; when on, the stored `miles` is twice the one-way distance entered (the form shows "One way 12.4 mi → 24.8 mi") and the route reads "From → To → From". With odometer readings the round trip is already in the readings, so the switch is off and disabled |
| `rate_cents` | from the rate table (above); personal 0 |
| `amount` | round(miles × rate_cents / 100, 2); shown as "Deduction value" for log-only trips and "Amount" for reimbursable ones |
| `reimbursable` | bool, default false (expenses module on); `driver` text ≤ 100 (default the book's profile name) |
| `reimburse_rate_cents` | for reimbursable trips: default `mileage.reimbursement_rate` (setting; empty means the standard rate for the kind); `reimburse_amount` = round(miles × that rate / 100, 2) |
| `category` | for reimbursable trips: an expense account, default `mileage.expense_account` (created on first use as "Auto: Mileage reimbursement", type expense) |
| `tags` | class / location / fund / project when those modules are on |
| `status` | `logged`, or for reimbursable trips `ready` → `posted` (→ flag `reported` as for expenses) |
| `source` | `desktop` or `phone` |

- **Log-only trips** (not reimbursable) never create ledger entries. They are the tax
  log: the deduction value is information only.
- **Reimbursable trips** post like personal-funds expenses (`expenses.md`): debit the
  category, credit **Reimbursements Payable**, payee = driver, memo "Mileage: <route>
  (<miles> mi × <rate>¢) — <purpose>". **Record reimbursement…** on the Expenses
  screen includes posted reimbursable trips beside expenses; repayment is the same
  transfer. Editing a posted trip edits its transaction (reconciled values locked);
  deleting asks whether to delete the transaction too.
- **Saved trips:** a named route {name, from_place, to_place, one-way miles,
  round_trip default, purpose_kind, purpose, vehicle?} chosen from a list to fill the
  form. **Save as a saved trip** on any trip form. Up to 200; edit/delete in Settings
  → Mileage → Saved trips.
- **Duplicate check:** saving a trip with the same date, vehicle and miles as an
  existing one asks "A trip with these details already exists. Save anyway?".
- **Odometer sanity:** when a trip has odometer readings and another trip of the same
  vehicle on a later date has a lower start reading (or an earlier date has a higher
  end reading), the trip shows a warning "Odometer readings out of order for this
  vehicle" (saved anyway).

### Mileage log and report

- Desktop rail/module screen **Mileage** with tabs **Trips** (table: Date, Vehicle,
  Route, Purpose kind, Purpose, Miles, Rate, Amount, Reimbursable/status; filters by
  year or date range, vehicle, purpose kind; totals row) and **Vehicles**.
- **Mileage report…** (`mileage.report` {from, to, vehicle?, kinds?, format: pdf |
  csv}) → per vehicle: odometer start and end for the year when recorded, total miles
  (end − start), and miles by purpose kind with percentages; then the trip list
  (date, route, purpose, miles, rate, amount) grouped by purpose kind with subtotals;
  grand totals of deduction value by kind. PDF rules as other MoneyKeep PDFs (standard
  fonts, no JavaScript, links, forms or embedded files); CSV is plain UTF-8 with a
  header row and formula-leading characters neutralised (`input-safety.md`).
- **Expense report:** posted reimbursable trips appear in `expense.report` as rows with
  Merchant = "Mileage — <vehicle>", Category, Purpose (route and purpose), Amount
  (reimburse amount) and Receipt "N/A"; the trip list section prints the route, miles
  and rate instead of a receipt image.
- **Tax summary** (`chart-presets.md`): when trips exist for the year, an extra
  section "Mileage (standard rates)" lists business miles and value, medical miles and
  value, charity miles and value, per vehicle, with the note "Information for your tax
  preparer; not entered in the books."

### Phone (mobile-capture.md revision 11)

- **Mileage page.** On the Expense tab, tapping the **Expense** tab while it is
  already selected switches the page to **Mileage**; tapping it again switches back.
  The tab's icon and label follow the page shown (receipt icon "Expense" / car icon
  "Mileage"), its accessible name is "Expense, tap again for Mileage" (or the reverse),
  and the top bar title reads Expense or Mileage. The first time the app is opened
  after this update a one-line tip above the tab bar says "Tap Expense again to log
  mileage" (closes on tap, shown once). The choice is remembered on the phone.
- **Mileage form** (same touch sizes and theme as revision 7–9): **Saved trip** picker
  (fills the rest), Date (calendar sheet; today), Vehicle (picker; preselected when one
  is active), Purpose kind (Business / Medical / Charity / Personal; Moving when on),
  **Miles** card (like the Amount card: opens the keypad; "mi"), or **Odometer** (a
  switch "Enter odometer readings" showing Start and End fields with the numeric
  keypad; miles computed and shown), **Round trip** switch, From and To (text, with
  the last 20 places used as suggestions), Purpose (text), Reimbursable switch (when
  the book's expenses module is on), and a computed line "24.8 mi × 72.5¢ = $17.98".
- **Send** and **Save for later** work as for expenses: the trip goes to the same
  **Outbox** (row: "Mileage · 24.8 mi · Office → Client") and **Sent** lists; edit and
  delete in the Outbox as for expenses. Unfinished trips are kept across tab switches
  like expenses (revision 9); after Send the form resets with today's date, the same
  vehicle and the same purpose kind.
- **Lists from the book.** The phone receives vehicles (active only), saved trips,
  purpose kinds in use, the rate table, whether Reimbursable is available and the
  default purpose with the other lists it already receives. A phone that is offline
  uses the last lists received.

### Capture server and API

- `mk-capture-api` gains a submission kind **trip**: {client_id (UUID from the phone,
  for idempotent retry), date, vehicle_id, purpose_kind, purpose, from_place,
  to_place, miles, odometer_start?, odometer_end?, round_trip, reimbursable,
  saved_trip_id?} with the same size limits as the fields above, validated on the
  server before storing (unknown vehicle ids are kept as given and resolved on the
  desktop: an unknown or inactive vehicle sends the trip to the desktop Inbox with
  "Choose a vehicle").
- The server stores trips encrypted like expense submissions, lists them in the inbox
  the desktop pulls, accepts update/delete while not yet taken, and reports status
  (`waiting`, `taken`, `refused` with a reason) to the phone exactly like expenses.
  Both ways in (Wi-Fi and Cloudflare) serve the same endpoints with the same pairing,
  PIN and token rules.
- The lists endpoint gains `vehicles`, `saved_trips`, `mileage_rates`,
  `mileage_enabled`, `reimbursable_enabled`.
- An older phone app that does not know trips keeps working (new fields are optional
  additions); an older server refuses a trip with 400 "This server does not accept
  mileage yet; update the server" and the phone keeps it in the Outbox.
- The desktop takes waiting trips into the book as **trips** (status `logged`, or
  `ready` when reimbursable and complete), visible on the Mileage screen with a "From
  phone" marker; Home Attention: "N trips from your phone" when any needs a choice.

### Commands (all audited; MCP yes unless noted)

`vehicle.create`, `vehicle.update`, `vehicle.list`, `vehicle.set_odometer` {vehicle,
year, start?, end?}, `trip.create`, `trip.update`, `trip.delete`, `trip.get`,
`trip.list` {from, to, vehicle, kind, status, text}, `trip.post` {trip},
`saved_trip.create` / `update` / `delete` / `list`, `mileage_rate.list` / `set` {kind,
from, cents} / `delete`, `mileage.reapply_rates` {from, to, preview: bool},
`mileage.report` {…} (PDF/CSV bytes), `mileage.summary` {year} (totals by vehicle and
kind, used by the Tax summary). `expense.record_reimbursement` accepts trips as well
as expenses.

### Settings (module `mileage`)

| Key | Type | Default |
|---|---|---|
| `mileage.default_purpose` | kind | business |
| `mileage.show_moving` | bool | false |
| `mileage.reimbursement_rate` | cents or empty (standard rate) | empty |
| `mileage.expense_account` | expense account | Auto: Mileage reimbursement |
| `mileage.distance_unit` | `mi` (only miles in this phase) | mi |

## Ledger effect

Only reimbursable trips, when posted: one transaction debit category / credit
Reimbursements Payable; reimbursement is the normal transfer. Log-only trips: none.

## Edge cases

- A trip date before the first rate row of its kind: rate 0 with the warning "No
  standard rate for this date; add one in Settings → Mileage → Rates".
- Rate rows with the same kind and `from` are refused ("A rate for this kind already
  starts on that date").
- Deleting a vehicle with trips is refused; it can be made inactive.
- Round trip with miles over 1,000 one way is refused (2,000 per trip limit).
- Personal trips are excluded from reimbursement and from deduction totals but count
  toward the vehicle's total miles.
- Moving off in settings hides the kind in pickers; existing moving trips still show.
- Phone with a cached rate table older than a new rate row: the phone shows its own
  computed value, but the desktop recomputes on taking the trip (the book's table
  wins) and the Sent row shows the book's amount once taken.

## Acceptance tests

M1. Rate lookup: business trips on 2026-06-30 and 2026-07-01 get 72.5¢ and 76¢;
    charity on either gets 14¢; a 2024 business trip gets 0 with the warning.
M2. Miles 12.4 one way with Round trip gives 24.8 mi; at 72.5¢ amount 17.98.
M3. Odometer 45,210.3 → 45,236.0 gives 25.7 mi; Round trip is disabled.
M4. A reimbursable business trip of 24.8 mi posted with reimbursement rate empty
    creates debit Auto: Mileage reimbursement 17.98 / credit Reimbursements Payable
    17.98, payee the driver; Record reimbursement… for it and an expense of 40.00
    makes one transfer of 57.98.
M5. Log-only trips create no transactions; deleting one leaves the ledger unchanged.
M6. Mileage report for 2026 with vehicle odometer 30,000 → 42,000 and trips business
    800 mi, charity 120 mi, personal 0 logged: totals 12,000 mi, business 6.7 %,
    charity 1.0 %; PDF parses as PDF 1.4+ without forbidden keys; CSV first row is
    the header and a purpose starting with "=" is written with a leading apostrophe.
M7. Expense report includes a posted reimbursable trip with Receipt "N/A".
M8. Tax summary for 2026 shows the Mileage section with business and charity values.
M9. Re-apply rates over 2026-07-01..2026-12-31 after adding a row reports the count
    and difference, then changes those trips; Undo restores them.
M10. Phone: on the Expense tab, tapping Expense shows the Mileage form (title, tab
     label and icon change); tapping again returns with the unfinished expense kept.
M11. Phone: a trip saved for later appears in the Outbox; Send delivers it through the
     server (Wi-Fi and Cloudflare paths in the test server) and the desktop takes it as
     a trip with source phone; resending the same client_id does not duplicate it.
M12. Server: a trip with miles 0, an unknown purpose kind or a 600-character purpose
     is refused with 400 and the reason; an older-API phone keeps working.
M13. Lists: the phone receives only active vehicles and the saved trips; choosing a
     saved trip fills route, miles, round trip, purpose and vehicle.
M14. Screenshots at 390×844 light and dark: `phone11-mileage`, `phone11-odometer`;
     desktop `mileage-trips`, `mileage-report-dialog`.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-11-07) — tags on the phone (owner request)

The phone's Mileage form shows the trip **tags** (class, location, fund, project — each
only when its module is on and it has entries), as the phone's Expense form does, below
Purpose. The lists come with the mileage lists (`GET /lists?mileage=1`); the trip carries
the chosen tags to the book. Test: with projects on, a trip sent with project "Roof
repair" arrives as a draft trip tagged with that project.
