# Spec: Projects

- **Area / module:** `projects` (new optional module, off by default; Features list)
- **Spec writer:** spec-writer session (Claude), with the owner
- **Date:** 2026-11-07
- **Status:** reviewed — approved by the owner 2026-11-07 (answers to the open questions below)

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Textbook | OpenStax, *Principles of Accounting, Volume 2* (job order costing) | Costs gathered per job; job profit = revenue − costs |
| 2 | Spec | `business-dimensions.md` (Part A) | Tagging splits, documents, rules; P&L columns by dimension |
| 3 | Spec | `mileage.md` (rev 2), `expenses.md`, `mobile-capture.md` | `project` tag already named on trips and expenses, phone forms |
| 4 | Spec | `business-sales.md`, `business-purchases.md`, `budgets.md`, `dashboards.md` | Invoices/bills, budget comparison, Attention and Home blocks |
| 5 | Owner decisions | Conversation 2026-11-07 | Project records, tagging and a report |

## Purpose

Gather everything spent on and earned from one piece of work (a roof repair, a church
building fund drive, a client job, a mission trip) so the user can see what it cost,
what it brought in, how it compares with its budget, and the miles driven for it —
across accounts, cards and documents, without new accounts in the chart.

## Behaviour

### Project records

| Field | Notes |
|---|---|
| `name` | unique; `:` allowed for sub-projects (e.g. `House:Roof repair`); a parent's totals include its sub-projects |
| `status` | `planned`, `active` (default), `on_hold`, `completed`, `cancelled` |
| `customer` | optional party (business books: who the work is for) |
| `start`, `end` | optional dates (planned or actual); used as the report's default period |
| `budget_mode` | `total` (default) or `by_account` — selector in the project form |
| `budget` | `total`: one amount; `by_account`: budget lines {expense account, amount}, the total being their sum (switching from lines to total keeps the sum; from total to lines starts with one line "Other" holding the total) |
| `income_budget` | optional expected income (business jobs, fund drives) |
| `fund` | optional fund (module `funds` on) — see Funds below |
| `billable` | default for new tagged expense splits (false); business books only (module `invoicing` on) |
| `markup_percent` | optional, applied to billable costs when invoiced (default 0) |
| `notes` | free text ≤ 2,000 |

Commands: `project.create|update|list|get|set_status|delete` (delete only if never
used; otherwise set status completed or cancelled). Completed and cancelled projects are
hidden from tag pickers (a "Show closed" toggle brings them back) but remain in reports.

### Tagging

- `project` joins class, location and fund as a split tag (`business-dimensions.md` Part
  A rules apply unchanged): any split may carry one project; documents carry a
  document-level default with per-line overrides; posting copies them to the splits.
- Where tags are already shown, project is shown too when the module is on: the register
  split editor, transaction form, invoices, bills, expense reports, mileage trips, phone
  Expense and Mileage forms (`GET /lists` includes active projects), schedules, and
  definition files (projects named by name; a missing name installs the rule disabled).
- Rules gain the action `set_project`.
- **Bulk tag:** in a register, select transactions → ⋯ → **Set project…** (one undoable
  command). Useful after the fact, e.g. tagging three months of hardware-store charges.
- Setting `projects.required` (default false), as for classes.

### Funds (owner decision 3)

- A project may name a **fund**. When a split is tagged with that project and has no fund
  yet, the fund is set to the project's fund (in the form as a pre-filled value the user
  can change, and by rules and the phone when they set the project). Existing tagged
  splits are not changed when the project's fund is changed later; the project form
  offers "Also set the fund on N tagged splits without a fund" as one undoable command.
- The project report shows the fund in its heading; the fund activity report is unchanged.

### Billable costs (owner decision 1; module `invoicing` on)

- An expense split tagged with a project has a **Billable** checkbox (default from the
  project's `billable`). Mileage trips tagged with a project have it too (value = miles
  × the trip purpose's rate, or a per-project `mileage_bill_rate` when set).
- A billable item is **unbilled** until it is on an invoice line, then **billed** (linked
  to that line). Deleting the line or the invoice makes it unbilled again.
- Invoice editor, customer = the project's customer: a bar "N unbilled costs for Roof
  repair (1,240.00) — **Add…**" opens a list with checkboxes (date, payee, account, memo,
  amount, amount with markup). Adding creates one line per item (description = date,
  payee and memo; amount with markup; project tag set), or one summary line per expense
  account (choice in the dialog). Lines post to the income account in setting
  `projects.billable_income_account` (default **Billable Expense Income**, income type,
  created on first use).
- The project detail gets a **Billable** tab: unbilled and billed items with their
  invoice numbers; the project list shows "Unbilled" when the module is on.
- The expense itself is unchanged (still an expense of the project); the invoice adds the
  income, so the project's net shows the recovery.

### Project screen (Features → Projects)

- List: name, status, customer, budget, spent, % used (bar, coloured at 90 % and over
  100 %), income, net. Filter by status; totals row.
- Detail: fields; **Summary** (income, expenses by account, mileage, net, budget vs
  actual per budget line and total, remaining); **Transactions** (every tagged split,
  date order, open in register); **Mileage** (tagged trips with miles and the value at
  the trip's purpose rate); **Documents** (invoices, bills and estimates tagged);
  actions **Mark completed**, **Report…**, **Set project on transactions…** (opens a
  search across registers with checkboxes).

### Reports

- `report.project` {project (with sub-projects), dates (default: project start–end or
  all), include_mileage (default true)}: income by account, expenses by account,
  mileage (miles and value by purpose — shown as a separate section, not added to
  expenses unless the user ticks "Count mileage as a cost"), net, budget vs actual and
  remaining. CSV and PDF export.
- `report.projects_summary` {dates, status filter}: one row per project — budget,
  expenses, % used, income, net, miles.
- `report.profit_loss` gains `columns: project` (plus "Not set"); totals equal the
  ungrouped report.

### Alerts and Home

- Home → Attention: "Roof repair has used 92 % of its budget" (at 90 %, setting) and "…
  is over budget by 340.00", once per threshold crossing, dismissible.
- Dashboard block type **Projects** (not on Home by default): active projects with a
  budget bar each.

## Ledger effect

Non-posting. Project records, tags and settings are book documents, audited and
undoable. Tags change no amounts. Billable costs post only through the invoice they are
added to (normal invoice posting).

## Settings

Module `projects`: `required` (false), `warn_percent` (90), `billable_income_account`
(Billable Expense Income).

## Edge cases

- A split already tagged with another project: bulk **Set project** replaces it after a
  confirm listing how many will change.
- Transfers between the user's own accounts carry no project in reports (they are not
  costs); a tagged transfer is shown in the Transactions tab marked "transfer, not
  counted".
- A refund (income-side split on an expense account) reduces the project's expense.
- Turning the module off hides tags; values stay and return when it is on again.

## Acceptance tests

PJ1. Project "Roof repair", budget 5,000.00: Home Depot 1,200.00 (Visa), contractor
     3,100.00 (Checking), refund −150.00 on Visa, all tagged → spent 4,150.00, 83.0 %,
     remaining 850.00; no alert. A further 500.00 → 93.0 % → the 90 % Attention item.
PJ2. Sub-projects `House:Roof` and `House:Gutters` → `House` totals both.
PJ3. A trip of 40 miles (business, 76¢ from 2026-07-01) tagged to the project shows
     30.40 in Mileage; net unchanged until "Count mileage as a cost" is ticked.
PJ4. Bulk Set project on three selected transactions is one command; Undo clears all
     three.
PJ5. P&L by project: Total column equals the ungrouped net income.
PJ6. Rule `payee contains "LOWES" → set_project Roof repair` tags staged rows.
PJ7. Phone expense with project "Roof repair" arrives tagged; a completed project is not
     offered on the phone.
PJ8. Deleting a used project is rejected with the count of tagged splits.

PJ9. Budget by account: lines Materials 3,000.00 and Labour 2,000.00 → total 5,000.00;
     the report shows budget vs actual per line; switching to Total keeps 5,000.00.
PJ10. Project "Building drive" with fund Building Fund: a gift tagged with the project
      gets fund Building Fund; changing it to General by hand is kept.
PJ11. Billable: two tagged expenses 200.00 and 50.00 (billable) and one not billable;
      markup 10 % → the invoice bar offers 2 items, 275.00; adding them creates two lines
      to Billable Expense Income; both show billed with the invoice number; deleting the
      invoice makes them unbilled again. A billable 40-mile trip at 76¢ offers 30.40.

## Owner decisions (2026-11-07)

1. Billable costs: yes (section above).
2. Mileage: shown separately, counted as a cost only when ticked (as written).
3. A project can link to a fund (section above).
4. Budget: both, with a selector (`budget_mode`).

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
