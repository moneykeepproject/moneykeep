# Spec: Dashboards (owner request 2026-10-28)

- **Area / module:** `core` (always available; blocks for module data appear only while
  their module is on)
- **Spec writer:** Claude session 2026-10-28 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-28
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Owner request | 2026-10-28: named, customisable dashboards with movable blocks (account transactions, scheduled transactions, charts, navigation buttons), a Dashboards button on the left, a startup dashboard. Owner decisions: stored in the book; snap-to-grid layout; Home becomes a dashboard; the button sits at the top of the left tab rail | |
| 2 | Library docs | egui documentation (drag and drop, `Sense::drag`, painter, layers) | |
| 3 | Standard | WCAG 2.2 SC 2.1.1 (keyboard), 2.5.7 (dragging movements: a non-drag alternative), 1.4.11 | Moving blocks must also work without dragging |
| 4 | Spec | `reports.md`, `scheduled-transactions.md`, `budgets.md`, `investments-core.md`, `expenses.md`, `mobile-capture.md`, `definition-files.md`, `app-desktop.md` (revisions 20, 23, 25) | Data comes from existing commands |

No target product was observed. Block types, layout rules and wording are MoneyKeep's
own.

## Purpose

Let the owner build screens that show what matters at a glance and jump straight to
the places used most.

## Behaviour

### Dashboards

| Field | Rules |
|---|---|
| `id` | generated |
| `name` | 1–60 characters, unique in the book (case-insensitive) |
| `icon` | optional symbol from the book-profile symbol list (`app-desktop` revision 26) |
| `columns` | 12 (fixed grid width) |
| `blocks` | 0–40 blocks |
| `order` | position in the Dashboards list |

- Stored **in the book** (audited, undoable, encrypted with the book).
- Book setting `dashboards.startup`: the dashboard opened when the book opens
  (default **Home**). Exactly one; choosing another replaces it.
- **Home** is a built-in dashboard created in every book (new and existing) with blocks
  equivalent to today's Home screen: Net worth, Account balances (by group), Attention.
  It can be edited, renamed and rearranged like any other, and can be **Reset to
  standard** but not deleted (so there is always one dashboard).

### Layout: snap-to-grid

- The dashboard area is a grid **12 columns** wide; row height ≈ 4 text lines (scaled
  with text size). A block occupies a rectangle `x, y, w, h` in whole cells
  (w 2–12, h 1–12); blocks never overlap.
- **Moving:** in **Edit layout** mode, drag a block by its title bar; a shadow shows
  where it will land, snapped to cells; other blocks move down to make room (compact
  upwards: no empty rows left above a block). **Resizing:** drag the block's
  bottom-right handle; sizes snap to cells and respect each block type's minimum.
- **Without dragging** (WCAG 2.5.7, keyboard): each block's menu has **Move up / down /
  left / right**, **Wider / Narrower**, **Taller / Shorter**; with a block focused in
  edit mode, arrow keys move it and Shift+arrows resize it.
- **Narrow windows:** when the area is narrower than 12 × the minimum column width
  (about 60 logical px), blocks are shown in a single column in reading order (by y,
  then x) at full width; the saved layout is unchanged.
- Outside edit mode nothing moves; blocks are read-only views with their own clickable
  items.

### Blocks

Every block has a **title** (default from its type and settings; editable, ≤ 60
characters), its settings, and a **⋯ menu**: Settings…, Refresh, Duplicate, Remove,
and the move/resize items in edit mode. Blocks whose module is off show "Turn on <module>
in Settings → Features to show this" instead of data; blocks whose account was deleted
show "This account no longer exists". Data refreshes when the book changes (generation)
and on Refresh. Dates and amounts follow the display settings (revision 20; display
sign).

| Type | Settings | Shows | Click |
|---|---|---|---|
| **Account transactions** | account (any, incl. category accounts); mode: *most recent N* (N 1–50, default 5) or *above an amount* (amount ≥ X, either direction / money in / money out, within the last D days, default 30, max N); include sub-accounts | Date, payee, amount (and category when wide enough); the account's balance in the title line | A row opens its register with that transaction selected; the title opens the register |
| **Scheduled transactions** | next N (1–50, default 5) or next D days; accounts filter (all or chosen) | Due date, payee, amount, account; overdue ones marked | Row opens the scheduled transaction; "Enter" button enters it (as the Scheduled screen) |
| **Account balances** | accounts (chosen list) or groups (sidebar groups) | Name and balance; group totals | Name opens the register |
| **Net worth** | — | Net worth with change since the start of the month and a small sparkline of the last 12 months | Opens the net worth report |
| **Chart** | chart kind: *spending by category* (bars, period), *income vs expenses* (paired bars by month), *net worth over time* (line), *account balance over time* (line; account), *budget vs actual* (budgets module), *portfolio value* (investments module); period: this month, last month, this quarter, this year, last 12 months, custom | The chart as the reports screen draws it (`charts.rs`), sized to the block | Opens the matching report with the same period |
| **Report summary** | report (any `report.*` with a single headline figure: P&L net income, cash flow net change, A/R and A/P totals, sales tax due, 1099 total), period | The figure and its comparison with the previous period | Opens the report |
| **Shortcut buttons** | 1–12 buttons; each: label (≤ 30), icon (symbol list), target: an account register, a report (with its saved options), a screen (Home, Scheduled, Reports, Accounts, Expenses, Sales, Purchases, Budget, Securities, Settings section), another dashboard, an action (New transaction in an account, Import file…, Reconcile an account, Calculator, New expense, Collect from phone) | Buttons in a wrapping grid | Goes there / runs the action (actions open their dialog, nothing posts without the user) |
| **Attention** | — | Today's Home attention list | Items open what they point to |
| **Bills and invoices due** | (bills / invoicing modules) next D days | Due bills and invoices with amounts, overdue marked | Opens the document |
| **Note** | text ≤ 2,000 characters (plain, cleaned) | The text | — |

New block types can be added later; unknown types (e.g. from a newer version) show
"This block needs a newer MoneyKeep" and are kept unchanged.

### Dashboards button and list

- The left tab rail (revision 23) gets a **Dashboards** tab at its **top**, above All
  accounts, with a drawn icon (four tiles). Hover shows the list of dashboard names;
  clicking opens the dashboard pane listing all dashboards (icon, name, a star on the
  startup one), with **+ New dashboard**, and per dashboard: Open, Rename, Duplicate,
  Set as startup, Move up/down, Delete (confirm; not for Home).
- Clicking a name opens that dashboard in the main area; the current dashboard is
  highlighted. The top bar's **Home** navigation item keeps its label and opens the
  startup dashboard.
- Each dashboard's header: name, **Edit layout** toggle (with **Done**), **+ Add
  block** (a picker of block types with one-line descriptions, then the block's
  settings), and the ⋯ menu (Rename, Duplicate, Set as startup, Delete).

### Commands (all audited and undoable except reads; MCP yes)

`dashboard.list`, `dashboard.get`, `dashboard.create {name, icon?, from?: dashboard}`,
`dashboard.update {dashboard, name?, icon?}`, `dashboard.delete`, `dashboard.reorder
{ids}`, `dashboard.set_startup`, `dashboard.reset_home`, `dashboard.add_block
{dashboard, type, settings, x?, y?, w?, h?}`, `dashboard.update_block {dashboard,
block, title?, settings?}`, `dashboard.move_blocks {dashboard, layout: [{block, x, y,
w, h}]}` (validated: inside the grid, no overlap, sizes within limits — the client
compacts and sends the whole layout), `dashboard.remove_block`, `dashboard.block_data
{dashboard, block}` (read: runs the block's query through existing commands and
returns display-ready rows).

A whole edit-layout session's moves are saved when the user presses Done as one
`dashboard.move_blocks` (one undo step); each block add/remove/settings change is its
own undo step.

### Sharing

Dashboards can be exported to and imported from definition files (`definition-files.md`)
as a new item kind `[[dashboard]]`: account and dashboard targets are stored by full
account name / dashboard name and resolved on import (unresolved → the block shows
"Choose an account" until set). No amounts or transactions are exported.

## Ledger effect

None.

## Edge cases

- A book opened by a version without dashboards ignores them; reopening with this
  version shows them unchanged.
- Very large data (an account with 100,000 transactions): blocks query only what they
  show (limit N / D days).
- The startup dashboard deleted (not possible for Home; deleting the startup one makes
  Home the startup).
- Two blocks configured identically are allowed.

## Acceptance tests

1. A new book has a Home dashboard with Net worth, Account balances and Attention
   blocks, set as startup; opening the book shows it.
2. Creating "Church" with an Account transactions block (Checking, most recent 5) shows
   the five newest Checking transactions newest first with the Checking balance;
   clicking a row opens the register with it selected.
3. Mode *above an amount* (money out ≥ 500.00, last 30 days) shows only those
   transactions.
4. A Scheduled transactions block (next 3) lists the next three by due date; overdue
   marked; Enter enters it (Undo removes it).
5. A Chart block (spending by category, this month) draws the same values as the
   Spending report for this month; clicking opens that report.
6. A Shortcut buttons block with "Checking" (register), "New expense" (action) and
   "Church" (dashboard) navigates/opens accordingly; the action posts nothing by
   itself.
7. Edit layout: dragging a 4 × 2 block onto another moves the other down (no overlap);
   Done saves one undo step; Undo restores the previous layout.
8. Keyboard: in edit mode a focused block moves with arrows and resizes with
   Shift+arrows; the block menu's Move/Wider items do the same.
9. `dashboard.move_blocks` rejects overlapping, out-of-grid and too-small layouts.
10. At a narrow width blocks stack in one column in reading order; the saved layout is
    unchanged after widening.
11. Set as startup on "Church": reopening the book opens Church; deleting Church makes
    Home the startup again.
12. The rail's Dashboards tab is first; hovering lists the names; the pane shows the
    star on the startup dashboard.
13. Turning a module off shows the "Turn on …" message in its blocks and keeps their
    settings.
14. Export "Church" to a definition file and import into another book: blocks resolve
    accounts by name; an unknown account shows "Choose an account".
15. Home can be Reset to standard and cannot be deleted.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-10-28) — implementation notes accepted

- Dashboards are stored as book settings (module `book`), not a new table, so older
  versions open the book and ignore them.
- Home's standard layout also has a "Coming up" scheduled-transactions block (as the
  old Home did).
- Follow-ups (not yet built): fields for a dashboard's icon and shortcut-button icons;
  bills/invoices rows opening the specific document; resizing in the narrow layout.

## Revision 3 (2026-10-28) — tidier header, button icons and sizes (owner request)

- **Header:** the dashboard header shows only the dashboard name on the left and the
  ⋯ menu button at the **far right** of the same line. **Edit layout** and **+ Add
  block** move into the ⋯ menu (first two items), followed by Rename, Duplicate, Set
  as startup, Delete. While editing, the header shows a **Done** button immediately
  left of the ⋯ button (so leaving edit mode is one click).
- **Shortcut buttons — icons:** every button shows an icon to the left of its label.
  The button editor gains an **Icon** picker: a grid of the symbol list (book-profile
  symbols plus new ones: register, report, calendar, receipt, camera, calculator,
  import, reconcile, dashboard, gear, invoice, bill, chart), drawn as shapes. Default
  icon by target: account → register (or the account group's rail icon), report →
  report, screen → that screen's icon, dashboard → dashboard, actions → their own
  (New transaction → plus, Import → import, Reconcile → reconcile, Calculator →
  calculator, New expense → receipt, Collect from phone → camera).
- **Shortcut buttons — sizes:** block setting **Button size**: Small (one text line
  high, icon and label side by side), Medium (default; about two lines, icon and label
  side by side), Large (tile: icon above the label, about four lines high, square-ish).
  Buttons wrap to the block width with even gaps; all buttons in a block share the
  size.
- **Dashboard icon:** the Rename dialog also offers the icon picker; the icon shows in
  the Dashboards list and before the name in the header.

Tests 16–19: the header has only the name and the ⋯ button (rightmost); Edit layout and
Add block are reachable in the menu and Done appears while editing; a button with the
default icon for its target and a chosen icon; Small/Medium/Large render at their
heights with Large placing the icon above the label.

## Revision 4 (2026-11-04) — chart legends always visible (owner report)

Owner report: on dashboards the legend under a chart is cut off at the bottom, and
making the block taller does not bring it back.

- **A chart fits its block.** The legend and any axis labels are laid out first; the
  drawing area gets the remaining height and width. Making a block taller or wider
  makes the drawing bigger; the legend never moves outside the block.
- **Legend wraps.** Entries flow left to right and wrap onto as many lines as the
  block width needs, with even gaps; an entry is never split, and a single entry
  wider than the block is shortened with "…" and shows its full text as a tooltip.
- **Minimum size.** A Chart block's minimum height is the height that leaves the
  drawing area at least about two text lines tall after the legend at the block's
  width. If a saved layout is smaller (or the block is made narrower so the legend
  needs more lines), the block grows to its minimum, moving blocks below it down as
  any resize does.
- The same rules apply to charts on the Reports screen and in the narrow (single
  column) layout.

Test 20: a Chart block with at least five legend entries (income vs expenses, or
spending by category) at its minimum size, at a medium size and at the narrow layout
shows every legend entry fully inside the block's bounds (no clipping) and a drawing
area at least two text lines tall; making the block one row taller makes the drawing
area taller and keeps the legend inside.

## Revision 5 (2026-11-04) — clickable chart data (owner request)

Owner request: on dashboard charts, clicking the data goes to the matching account,
category or report. Until now a click anywhere on a Chart block opened the matching
report.

**What each piece of a chart opens** (the period is the block's period unless the
piece is itself a period):

| Chart kind | Click on | Opens |
|---|---|---|
| Spending by category | a category's bar | that category's register, date range = the block's period |
| Income vs expenses | a month's income bar / expense bar | Income & expenses report for that month (expanded to its categories) |
| Net worth over time | a point (month) | Net worth report as of that month's end |
| Account balance over time | a point | the account's register, scrolled to and selecting the last transaction on or before that date |
| Budget vs actual | a category's bar (budget or actual) | that category's register for the period; the budget figure's bar opens the Budget screen at that category when the budgets module is on |
| Portfolio value | a point | Holdings report as of that date |
| any | a legend entry | the same as clicking that series (a category → its register for the period; Income / Expenses → the Income & expenses report for the period) |
| any | the title or empty chart area | the matching report with the block's period (as before) |

Pieces that stand for "Other" (a grouped remainder) open the report for the period
instead of a register. Reports-screen charts follow the same table.

**How it looks and works**

- Pointer over a clickable piece: the pointer becomes a hand, the piece is highlighted
  (slightly lighter fill or a ring on a point), and a tooltip shows its label and
  value ("Groceries — $412.50, Oct 2026"). Points on line charts have a hit area at
  least 24 × 24 px (WCAG 2.5.8) even though they are drawn smaller.
- One click opens it (no double-click), as everywhere else (app-desktop rev 32).
  In **Edit layout** mode nothing in a block opens; clicks select or drag the block.
- **Keyboard and screen readers:** Tab reaches the chart, then arrow keys move between
  its pieces in reading order (left to right, then the legend); each piece has an
  accessible name with its label and value and the role of a link; Enter or Space
  opens it. The chart's overall description stays as now.
- The phone and browser layouts get the same: tap opens; the tooltip shows on long
  press.
- Back (Alt+Left / the back button) returns to the dashboard.

Tests 21–24: clicking a category bar in a spending-by-category block opens that
category's register filtered to the block's period; clicking an income-vs-expenses
month bar opens Income & expenses for that month; keyboard: Tab to the chart, Right
arrow twice, Enter opens the third piece's target, and each piece exposes an accessible
name with its value; in edit mode a click on a bar opens nothing.
