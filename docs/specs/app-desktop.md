# Spec: Desktop app (Phase 2c)

- **Area / module:** front end (`core`)
- **Spec writer:** Claude session 2026-10-18 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-18
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Library docs | egui / eframe — https://docs.rs/egui , https://docs.rs/eframe | Immediate-mode GUI library |
| 2 | Library docs | egui_kittest — https://docs.rs/egui_kittest ; kittest — https://docs.rs/kittest | Headless UI tests driven through the accessibility tree |
| 3 | Design | MoneyKeep plan ("thin, swappable egui UI", "keyboard-first register"), AGENTS.md (front ends only call registry commands) | |
| 4 | Specs | `ledger-*.md`, `ledger-register.md`, `import-*.md`, `reconciliation.md`, `mcp-server.md` | Every screen action maps to an existing command |

The layout, visual design and wording below are MoneyKeep's own. No target product's
screens, screenshots, icons or terminology lists were consulted, and none may be.

## Purpose

A native desktop app (Windows, macOS, Linux) for everyday use: see balances, enter and
edit transactions quickly from the keyboard, review imports, and reconcile.

## Architecture

- New crate `crates/mk-ui`: egui widgets and view logic. Depends on `mk-core` and
  `mk-commands` only. No file or network IO.
- New crate `crates/mk-watch`: the watched-folder loop moved out of the CLI so the app
  and CLI share it. File IO allowed; no UI. The CLI's `watch` command uses it with
  unchanged behaviour.
- New app `apps/moneykeep-app`: the `eframe` binary `moneykeep-app`. Owns file
  dialogs, drag-and-drop, the per-user config file and threads. On Windows release
  builds it opens no console window.
- **Every change to a book goes through `Registry::invoke`** with actor `app`. The UI
  never writes SQLite, never computes balances, and never validates business rules
  itself: it shows what commands return. Undo/redo are `history.undo`/`history.redo`.
- Commands that may take long (`import.stage`, `import.commit`, registers over 5,000
  rows) run on a worker thread; the UI shows progress and stays responsive. Only one
  command runs against the book at a time.
- Per-user app config (recent books, window size, theme, density) lives in the
  platform config directory as JSON, never inside the book and never containing
  financial data.
- Layering rules in `xtask` are extended for the new crates; `rusqlite` stays in
  `mk-store` only.

## Visual design

- **Character:** calm, quiet, dense where it matters. Neutral warm-grey surfaces, one
  accent colour (deep teal, used only for focus, selection and primary buttons), and
  no gradients, stripes or decorative icons.
- **Numbers:** right-aligned in a monospaced face so digits line up; thousands
  separators; amounts that lower a displayed balance shown with a leading minus in a
  muted red; others in the normal text colour. Never colour alone: the sign is always
  shown.
- **Themes:** follows the system light/dark setting by default; user can force light
  or dark. Both meet WCAG AA contrast for text.
- **Density:** comfortable (default, 28 px register rows) or compact (22 px).
- **Fonts:** egui's built-in fonts (no extra font assets in Phase 2c).
- **Wording:** plain, short labels. Column and button names are MoneyKeep's own (see
  screens below).

## Screens

### Start screen (no book open)

"Create a new book" (name, base currency with USD default, save location via file
dialog), "Open a book…", and a list of recent books with their paths. A book that
fails to open shows the error and stays in the list with a "Remove from list" action.

### Main window

- **Top bar:** book name; Undo and Redo buttons (tooltips name the change, from
  `history.list`); "Import…" button; "Reconcile…" when a register is open.
- **Sidebar:** accounts grouped as *Cash & bank* (bank, cash), *Credit cards*,
  *Investments* (brokerage, security), *Other assets*, *Loans & other debts*, then a
  collapsed *Categories* group (income and expense), each account with its display
  balance. Groups show their total. Placeholder accounts show as headings; archived
  accounts are hidden unless "Show archived" is ticked.
- **Main area:** one of the views below. Navigation keeps a back/forward history.

### Home

Net worth (assets minus liabilities, display sign, base currency), a short list of
account balances by group, and an **Attention** list: staged import batches waiting
for review, open reconciliation sessions, and imported rows marked `needs_review`.
Each item opens the relevant view.

### Register (one account)

Columns: **Date**, **No.**, **Payee**, **Category** (the other account's full name,
"Split (n)" for split transactions, "Transfer: <account>" for asset/liability
counterparts), **Memo**, **Status** (blank, `c`, `r` shown as an empty circle,
half-filled circle and filled circle with an accessible label "uncleared",
"cleared", "reconciled"), **Decrease**, **Increase**, **Balance**. Column titles for
Decrease/Increase adapt to the account kind: bank and cash "Spent"/"Received";
credit card "Charge"/"Payment"; other kinds "Decrease"/"Increase".

- Data comes from `account.register` in pages; scrolling is virtualised so 100,000
  rows scroll smoothly.
- Voided rows are shown struck through and dimmed, with "Void" in the status column.
- Imported rows show a small "imported" marker with a tooltip.
- A filter field (Ctrl+F) narrows rows by payee/memo/category text, and a date-range
  picker narrows by date (both just change `account.register` arguments or filter the
  returned page; balances always come from the command).
- Footer: closing balance, cleared balance, reconciled balance.

**Entry row** (always last, or first in newest-first order):

| Field | Behaviour |
|---|---|
| Date | Defaults to the last date entered in this session, else today. `+`/`-` change by a day while focused. |
| No. | Free text |
| Payee | Autocomplete from `payee.suggest`; choosing a suggestion pre-fills Category and the amount from its last use (amount stays selected so typing replaces it) |
| Category | Autocomplete from `account.suggest`; "Split…" opens the split editor |
| Memo | Free text |
| Decrease / Increase | One of the two; typing in one clears the other |

Keys: Tab / Shift+Tab move between fields; **Enter** saves (`txn.create` or
`txn.update`); **Esc** cancels the edit; **Ctrl+N** focuses a new entry row; Up/Down
move the selection when not editing; **Enter** on a selected row edits it; **Space** on
a selected row toggles uncleared↔cleared (`txn.set_reconcile`; reconciled rows are not
toggled, a message explains why); **Delete** asks to confirm, then `txn.delete`.
**Ctrl+Z** / **Ctrl+Shift+Z** (and Ctrl+Y) undo/redo.

Errors returned by a command appear under the entry row in plain words (the command's
message), with the field it concerns highlighted when known. The row keeps its values.

**Row menu** (right-click or the "…" button): Edit, Split…, Mark cleared / uncleared,
Void / Unvoid, Reverse…, Delete, Copy as text, Open other account.

**Split editor:** a modal list of lines (category, memo, amount); shows "Left to
assign" live; "Save" is enabled only when the remainder is exactly zero; saving calls
`txn.create`/`txn.update` with all splits.

### Import review

Opened by "Import…" (file dialog filtered to `.ofx .qfx .qbo .qif .csv`), by dropping
a file on the window, or from Home's Attention list. The app reads the file and calls
`import.stage` with `content_base64` and the file name.

- Each section shows the bank's account description (e.g. "REVIEWBANK CHECKING ending
  6789"), an account picker, and "Remember for future files" (default on), calling
  `import.map_account`.
- Rows table: Date, Payee, Memo, Amount, **What will happen** (chip: *New*, *Matches
  existing*, *Already imported*, *Bank correction*, *Needs review*), Category (picker,
  shows "by rule: <name>" when a rule chose it), Skip toggle. Changing a category
  offers "Always use this for <payee>" (`make_rule`).
- A match row shows the existing transaction it matched; the user can choose a
  different candidate or "Add as new" (`import.set_row`).
- Footer: counts per chip; **Add to books** (`import.commit`, disabled while any
  section is unresolved) and **Discard** (`import.discard`). After commit, the view
  offers "Reconcile now" if a statement balance was captured.
- CSV without a profile: the profile editor opens with the suggested profile filled in
  and a live preview of the first five parsed rows; saving calls `csv.profile_save`
  and re-stages.

### Reconcile

- Start dialog: account, statement date, statement balance; or pick a captured
  statement (`reconcile.start`).
- Two lists side by side (narrow windows: stacked): money out and money in, each
  candidate with a checkbox (`reconcile.select`/`unselect`), cleared items pre-ticked.
- A footer that stays visible: statement balance, starting balance, ticked total,
  **Difference** (large; zero shown as "Balanced").
- **Finish** enabled only when balanced (`reconcile.finish`). "Finish with an
  adjustment…" asks for the adjustment account and confirms the amount. "Cancel
  reconciliation" discards the session.
- Reconciliation history per account with "Undo last reconciliation".

### Accounts

A tree of all accounts with create, edit, move (drag or "Move to…"), archive,
unarchive and delete actions (`account.*`). The new-account dialog asks for name,
kind (a friendly list mapping to type + subtype), parent, and an optional **opening
balance and date**, which creates a transaction against the Opening Balances account.
Errors from commands are shown in the dialog.

### Settings

- **Features**: every module from `module.list` with its description and an on/off
  switch (`module.enable`/`module.disable`); refusals (e.g. a required module) are
  shown next to the switch. Turning a module off says it hides features without
  deleting data.
- **Import**: match window (days), auto-commit, watched folder (folder picker) and
  "Watch while MoneyKeep is open" (runs `mk-watch` on a background thread with actor
  `watch`; the Attention list updates when files arrive).
- **Display** (per user, stored in app config): theme (system/light/dark), density.

### History

`history.list` as a readable list (time, who, what), with Undo and Redo buttons.

## Accessibility and keyboard

- Every interactive element has an accessible name (AccessKit via egui), which the UI
  tests also use.
- Everything in the register, import review and reconcile views is reachable without a
  mouse.
- Focus is always visible.

## Out of scope for 2c

Budgets, reports beyond balances, scheduled transactions, investments, business
modules' screens, encryption, the web (wasm) build, and a JSON control channel
(commands as JSON over a local socket) (MCP already gives agents full control).

## Acceptance tests

Automated with `egui_kittest` against the real views and an in-memory or temporary
book, finding widgets by accessible name. Screenshot snapshots, if added, are optional
and not required in CI.

1. Start screen: create a book with name and currency; the main window shows the book
   name and the Opening Balances account.
2. New account dialog with opening balance 1,000.00 creates the account and the
   opening transaction; the sidebar shows 1,000.00.
3. Register entry from the keyboard only: type date, payee, category (via
   autocomplete), amount in Spent, press Enter; the row appears with balance 950.00 and
   the entry row is ready for the next one.
4. Payee autocomplete pre-fills category and amount from the last use.
5. An invalid entry (unknown category) shows the command's error under the row and
   keeps the typed values.
6. Space toggles a selected row's status uncleared→cleared and back; a reconciled row
   is not toggled and a message explains why.
7. Split editor: Save stays disabled until "Left to assign" is 0.00; saving creates a
   three-split transaction shown as "Split (2)".
8. Ctrl+Z undoes the last entry and the balance returns; Ctrl+Shift+Z redoes it.
9. Void shows the row struck through with an unchanged running balance.
10. Import review: staging `examples/fixtures/ofx/checking-v1.ofx` shows the account
    description, the account picker, chips for each row and counts; mapping the account
    and pressing "Add to books" commits; re-importing shows every row as "Already
    imported".
11. Changing a row's category during review with "Always use this" creates a rule
    (visible through `rule.list`).
12. Reconcile: ticking items updates Difference live; Finish is disabled until
    Balanced; finishing marks the splits reconciled (filled circles in the register).
13. Settings: switching Estimates on then off works; switching off Estimates while
    Progress invoicing is on shows the refusal message.
14. The sidebar groups accounts correctly and shows group totals; Home's net worth
    equals assets minus liabilities.
15. A busy book (another writer holds it) shows a clear "book is busy" message rather
    than failing silently.
16. The app never calls SQLite directly: a layering test confirms `moneykeep-app` and
    `mk-ui` do not depend on `rusqlite` (already enforced by xtask; add the rule).
17. `moneykeep watch` (CLI) still passes its existing tests after moving to `mk-watch`.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 — review findings (2026-10-19)

The spec writer ran the release build on a virtual display (Xvfb, software OpenGL)
against a demo book and found layout problems the behaviour tests did not catch. These
rules are added; where they differ from the text above, this section wins.

### Layout rules

1. **Everything fits.** At every window size from 1000×700 logical pixels up, the
   register's Balance column, the entry row's save button and every import-review
   column are fully inside the window without horizontal scrolling. Payee, Category and
   Memo columns shrink first and truncate with an ellipsis; the full text is in a
   tooltip and in the row's accessible name. Date, No., Status and the amount columns
   never shrink below their content.
2. **Alignment.** Text cells are left-aligned, amounts and balances right-aligned,
   status centred. Each column header is aligned the same way as its cells and spans
   exactly its column (one shared column layout for header, rows and entry row).
3. **Readable sizes.** No text smaller than 12 logical points anywhere (chips, setting
   descriptions, sidebar subtotals included).
4. **One focus ring.** Exactly one widget shows keyboard focus at a time.
5. **Navigation highlight.** The top bar highlights the current view (Register,
   Reconcile, Import review, Accounts, History, Settings, Home); views not in the top
   bar highlight nothing.

### Screen changes

- **Start screen:** a centred column (max 560 px wide): the app name, "Create a new
  book", "Open a book…", then recent books shown as book name (from the file name,
  without extension) with the folder in smaller text beneath, not the raw full path.
- **Sidebar:** remove the second, smaller "… total" line under each group heading (the
  heading already shows the total). The *Categories* group shows **no** total (adding
  income and expenses together is meaningless); it lists *Income* and *Expenses*
  sub-headings, each with its own total.
- **Register entry, payee suggestions:** suggestions appear as a dropdown list directly
  under the Payee field (up to 8). Down/Up move the highlight, Enter or Tab accepts the
  highlighted suggestion (the first one by default), Esc closes the list. Same pattern
  for Category suggestions.
- **Import review:**
  - One table with columns Date, Payee, Memo, Amount, **What will happen**,
    **Category**, **Skip**. Every row is one line high.
  - A *Matches existing* row shows, in the Category column, "Matches <date> <payee>
    <amount>" (truncated as needed) and a small "Add as new instead" link. Nothing is
    stacked.
  - The Category cell is a single searchable account picker; the separate "Use"
    button is removed. Choosing a category is applied immediately (`import.set_row`)
    and offers "Always use this for <payee>" as a small checkbox next to it.
  - Before the section's account is chosen, rows show "Choose an account first" in
    *What will happen*, and the Skip boxes are shown unticked and disabled.
  - Chips use the normal text size with a subtle background, one colour per kind.
- **Reconcile start:** statement date defaults to today; fields show their format as
  hint text ("YYYY-MM-DD", "e.g. 1,234.56"); if captured statements exist for the
  account, a "Use statement from <file> (<date>, <balance>)" choice appears above the
  fields.

### Additional acceptance tests

18. At 1000×700 and 1200×800, the register's "Balance" header, the entry row's save
    button and the import review's "Skip" header have bounding boxes fully inside the
    window (kittest node bounds).
19. For each register and import-review column, the header's horizontal span overlaps
    its first data cell's span, and amount cells are right-aligned within their column.
20. The sidebar has no "… total" sub-line, the Categories group has no total, and it
    shows Income and Expenses sub-totals.
21. Typing "Co" in the entry payee shows a suggestion list; Down then Enter fills
    "Costco"; Tab on a fresh "Co" accepts the first suggestion.
22. A matched import row is the same height as other rows and offers "Add as new
    instead".
23. After saving an entry, exactly one widget has focus.
24. With Reconcile open, the top bar's Reconcile item (or no item) is highlighted, not
    Home.
25. No rendered text in any view is smaller than 12 points (walk all text shapes in a
    frame for each view).
26. Optional, not in CI: a screenshot of each main view at 1200×800 in light and dark
    themes, stored as test artefacts for human review.

## Revision 3 — second review (2026-10-19)

The spec writer re-ran the release build (Xvfb, 1200×800, demo book) after revision 2.
Start screen, register layout, sidebar and payee suggestions now meet revision 2. The
items below remain. Where they differ from the text above, this section wins.

- **Import review, Category column.** The cell's contents must fit its column: today
  the "Always use this" checkbox label is clipped ("Alway") and runs under the Skip
  column, and a *Matches existing* row's "Add as new instead" link is pushed out of
  view by the match text. Give the column a fixed share of the table width; inside it,
  the picker shrinks first, the checkbox label reads "Always" with the full "Always use
  this for <payee>" as tooltip and accessible name, and the match text truncates so
  the link is always fully visible.
- **Reconcile start.** Show "Or enter the statement's closing date and balance:" only
  when a captured-statement choice is shown above it; otherwise the fields stand alone
  under the heading. Label/field pairs are laid out in a grid so the fields share one
  left edge.
- **Accounts view.** A tree grouped like the sidebar (Cash & bank, Credit cards, …,
  Income, Expenses), indented by parent. Each row: name (left), friendly kind label
  ("Bank", "Credit card", "Opening balances" — never the internal type slug), balance
  (right-aligned, currency scale), and the "…" actions button in one aligned column.
- **Unresolvable start view.** If `MONEYKEEP_VIEW` (or any navigation) names a view
  that needs an account and none is given or it does not resolve, open that view with
  an account picker, or show a plain-words message ("Choose an account to reconcile");
  never a raw command error such as `not found: … ""`.
- **Category suggestions after pre-fill.** When choosing a payee pre-fills the Category,
  do not reopen the Category suggestion list until the user types in that field.

### Additional acceptance tests

27. Import review at 1000×700 and 1200×800: for every row, each widget in the Category
    cell has a bounding box inside the Category column, and a matched row's "Add as new
    instead" link is fully visible.
28. Reconcile start for an account with no captured statements contains no text
    beginning "Or ".
29. The Accounts view shows no internal type slug (`credit-card`, `opening-balances`,
    …) and each account's balance cell is right-aligned.
30. Starting with `MONEYKEEP_VIEW=reconcile` shows no error banner.
31. Typing "Co" in the entry payee, Enter, then Tab leaves exactly one widget focused,
    and a pre-filled Category shows no open suggestion list.

## Revision 4 — Phase 2b screens (2026-10-19)

Adds the screens for `scheduled-transactions.md`, `budgets.md` and `reports.md`, and
one fix found on Windows. The layout rules of revision 2 apply to every new screen
(everything fits from 1000×700, one shared column layout per table, amounts
right-aligned, no text under 12 points, one focus ring, top-bar highlight).

### Top bar

Order: Home, **Scheduled**, **Budget** (only while the `budgets` module is on),
**Reports**, Accounts, History, Settings; Import review / Reconcile / Register appear
when open, as now.

### Scheduled

- A list from `schedule.upcoming` (from: none, so overdue items always show; to: today
  + 30 days) in three groups with headings: *Overdue*, *Due in the next N days* (per
  schedule `remind_days`, shown as "Due soon" when they differ) and *Later this month*.
  Columns: Due, Payee, Account, Category, Amount (as seen from the account), Repeats
  (the schedule's `summary`), and actions **Enter**, **Skip**, **Edit**. Estimated
  amounts show "about" before the number.
- **Enter** on a fixed-amount occurrence calls `schedule.enter` directly; on an
  estimate it first opens a small form with date, amount (pre-filled, selected) and
  memo. **Skip** asks nothing and is undoable (the message after it says "Skipped —
  Undo").
- A second tab or toggle, **All schedules**, lists every schedule (`schedule.list`)
  with name, account, amount, Repeats, Next due, status, and Pause/Resume, Edit,
  Delete (Delete confirms).
- **New schedule…** and Edit open one form: payee (with `payee.suggest`), account
  (money account), category or Split…, amount with Spent/Received choice (or
  Charge/Payment etc. per account kind), memo; **Repeats** picker offering Once, Every
  week, Every 2 weeks, Twice a month, Every month, Every 3 months, Every year, Custom…
  (custom exposes frequency, interval, day/weekday/week); start date; ends Never / After
  N times / On date; "If it falls on a weekend" None / Friday before / Monday after;
  "Amount changes each time"; "Enter automatically"; "Remind me N days before". The
  form shows the live `summary` text and the next three due dates as the user edits.
  Save calls `schedule.create`/`schedule.update`; command errors appear in the form.
- Register row menu gains **Repeat this…**, opening the same form pre-filled via
  `schedule.from_txn` semantics (the form saves with `schedule.from_txn` or
  `schedule.create`).
- Register gains a toggle **Show scheduled** (default on): pending occurrences for the
  account in the next 30 days (and overdue ones) appear after the last real row, in
  italic muted text with a clock marker, with projected balances from
  `schedule.forecast`; they cannot be edited in place, and selecting one offers Enter
  and Skip.

### Home additions

- On opening a book the app calls `schedule.run_due` (actor `schedule`). If anything
  was entered, the message line says "Entered N scheduled transactions" with a
  "Show" link to History; failures are listed in the Attention list with the error.
- Attention gains "N scheduled transactions overdue" and "N due in the next 7 days",
  opening the Scheduled view.
- Home gains a small **Coming up** list: the next five occurrences (date, payee,
  amount).

### Budget

- With the module off, the Budget item is hidden and nothing else changes. Settings is
  where it is turned on. (No other screen nags about it.)
- With no budget yet: a short explanation and two buttons, **Start from my spending**
  (opens the suggestion dialog) and **Start empty** (creates "Budget" with the current
  month as start).
- The view: month switcher (‹ October 2026 ›, plus "This month"), a Month / Year
  toggle, and a budget picker when more than one exists. Table sections *Income* and
  *Expenses*, rows in tree order (indented), columns Category, Budget, Spent (Received
  for income), Remaining, and a progress bar (actual ÷ available; over 100 % drawn in
  the muted red with the overspent amount; never colour alone — Remaining shows the
  sign). Rollover lines show "incl. carried" with the carried amount in a tooltip.
  Section totals, then **Net** (income − expenses: budget and actual).
- **Budget** cells are editable in Month view: typing a number and pressing Enter or
  Tab calls `budget.set_month` when the row already has a different default, otherwise
  `budget.set_line`. A row menu offers "Use this amount every month", "Roll over
  unspent", "Remove from budget".
- *Not in budget* section below: accounts from `unbudgeted` with their actual and an
  **Add** button (adds a line with amount = the suggestion for that account).
- **Add category…** button: an account picker restricted to income and expense.
- **Fill from history…** opens the suggestion dialog: range (last 3, 6 or 12 months),
  method (average or last month), and a table of proposed amounts with a checkbox and
  editable amount per row; **Apply** calls `budget.apply_suggestion` once.
- Year view: one column per month (Jan…Dec, horizontally scrolling is allowed here and
  only here, with Category frozen on the left) showing actual over budget; read-only.

### Reports

- Left column: the report list — **Spending**, **Income & expenses**, **Net worth**,
  **Cash flow**, **Payees**. Right: the selected report.
- Controls above every report: period preset (This month, Last month, This quarter,
  This year, Year to date, Last 12 months, Last year, All, Custom… with two date
  fields), interval where it applies (Month / Quarter / Year), a depth control
  ("Top-level only" / "All levels"), accounts filter where it applies, and **Export
  CSV…** (calls `report.export`, then a save dialog with the suggested file name).
- A chart above the table, drawn with egui painting or an MIT/Apache-licensed plot
  crate (checked by `cargo deny`):
  - Spending: horizontal bars, largest first, top 10 with the rest as "Other".
  - Income & expenses: paired bars per period (income, expenses) and a net line.
  - Net worth: a line of net worth per period end, with assets and liabilities as
    lighter lines.
  - Cash flow and Payees: no chart.
  Charts use the accent colour and neutral greys, label axes with amounts at whole
  currency units, show the exact value in a tooltip on hover, and have an accessible
  description summarising the series. The table below always holds the exact numbers.
- Tables follow the command's rows exactly (totals rows bold). Clicking a category or
  payee row opens a filtered transaction list (`txn.list` with that account and the
  report's dates) in a modal with Date, Account, Payee, Memo, Amount.

### Book file errors (found on Windows, 2026-10-19)

When creating or opening a book fails because the file or folder cannot be written or
read, the message names the path and the likely cause in plain words, e.g. "Couldn't
create the book at C:\Users\…\Documents\Home.moneykeep. Windows or another program may
be blocking MoneyKeep from writing to this folder (for example Controlled folder
access). Choose another folder, or allow MoneyKeep in your security settings." On
other platforms the second sentence is "Check that you can write to this folder." The
underlying error text is available under a "Details" disclosure. The store layer
reports such failures as a distinct `io` error code carrying the path, rather than a
generic SQLite or not-found error.

### Additional acceptance tests

32. Scheduled view with `today` = 2026-10-20 (injected) shows overdue, due-soon and
    later groups as in `scheduled-transactions.md` AT10; Enter on a fixed occurrence
    adds the transaction (visible in the register) and removes the row; Undo restores
    it.
33. Enter on an estimate opens the amount form; confirming 63.18 creates that amount.
34. The schedule form shows "Every 2 weeks on Friday" and the next three dates for an
    every-2-weeks schedule starting 2026-10-02.
35. Opening a book with an auto-enter schedule due calls `run_due` once and shows
    "Entered 1 scheduled transaction"; reopening the same day enters nothing.
36. Register "Show scheduled" lists an upcoming occurrence after the last real row with
    a projected balance equal to the forecast's.
37. With `budgets` off, no Budget item in the top bar; turning it on in Settings shows
    it; "Start empty" creates a budget.
38. Typing 450 in Groceries' Budget cell and pressing Enter updates Budget, Remaining
    and the progress bar from `budget.report`.
39. "Fill from history…" → Apply creates one audit entry (one `history.list` item).
40. Reports: Income & expenses for the `reports.md` test book shows the numbers of its
    AT2 in the table; Export CSV produces the file content from `report.export`.
41. Every new view meets AT18/AT19/AT25 rules (Skip-style right-edge check on the last
    column of each new table, header/cell overlap, text size).
42. Creating a book in a read-only folder (tests use a temp dir without write
    permission on Unix) shows a message containing the path and "Choose another
    folder", and no error banner with raw SQLite text.

## Revision 5 — Phase 2b review (2026-10-19)

The spec writer checked the release build on a virtual display (1200×800) against a
book built independently through the CLI; every report figure matched the
hand-computed values in `reports.md`. These presentation problems remain. Where they
differ from the text above, this section wins.

- **Period tables (Income & expenses, Net worth, Budget year view).** With 12 periods,
  period headers show as "202…" and amounts such as 2,500.00 are cut off, which breaks
  layout rule 1 (amount columns never shrink below their content). Instead:
  - Period headers are short: months as "Jan", "Feb", … with the year shown once
    above the first month of each year ("2026"); quarters "Q1 2026"; years "2026".
  - Zero amounts in period cells are shown as a dimmed "–" (the CSV export still
    writes 0.00).
  - When the periods do not fit, the period columns scroll horizontally while the
    Category column (left) and the Total column (right) stay fixed. This is the
    second allowed exception to "no horizontal scrolling"; amount cells are never
    truncated.
- **Chart axes.** Use rounded tick values (1, 2 or 5 × a power of ten, e.g. 0,
  1,000, 2,000) including zero when the data spans it, never data-derived values
  like 1,229 or −41. Axis and bar labels follow layout rule 3 (at least 12 points);
  the date axis uses the same short labels as the table.
- **Net worth over a range reaching into the future** (e.g. "This year" in October):
  period ends after today are not shown; the last point is today (labelled with the
  date). `report.net_worth` gains this rule: when `to` is after `today`, period ends
  after today are dropped and `today` is added as the final date.
- **Home "Coming up"** shows the next five pending occurrences (not overdue ones,
  which are in Attention), regardless of how far ahead they are; it currently shows
  only those within 30 days.
- **Scheduled table:** the Repeats column is truncated while Account and Category
  have spare width. Repeats gets priority: Payee, Account and Category shrink first.

### Additional acceptance tests

43. Income & expenses for "This year" at 1200×800: every amount cell's text is
    complete (no "…") and the Total column is fully inside the window; scrolling the
    periods keeps Category and Total in place.
44. Chart tick labels are all round values as defined above and no chart text is
    under 12 points.
45. `report.net_worth` with `from` 2026-01-01, `to` 2026-12-31, `today` 2026-10-19,
    interval month: the dates end with 2026-09-30, 2026-10-19.
46. With three schedules whose next occurrences are 5, 40 and 70 days away, Home's
    Coming up lists all three.

## Revision 6 — password-protected books (2026-10-19)

Screens for `book-encryption.md`, plus two leftovers from the Phase 2b review.

### Start screen and opening

- "Create a new book" gains **Protect with a password** (unchecked by default). When
  ticked: password and confirm fields, the passphrase note, and "Create a recovery
  key" (ticked). The book is created encrypted.
- Recent books show "Password protected" in the smaller folder line for encrypted
  books (detected from the file's first bytes, not stored in config).
- Opening an encrypted book (from the list, the Open dialog, drag-and-drop or
  `MONEYKEEP_OPEN`) shows a password prompt naming the book, with **Open**, a "Use
  recovery key instead" link, and Cancel. `wrong_password` keeps the prompt open with
  "That password didn't work." and clears the field; `book_in_use` says the book is
  open in another MoneyKeep window; `book_damaged` shows the message from the spec
  and, if `<file>.prev` exists, offers **Open previous version** (opens it read-write
  after copying it to `<file>` — the damaged file is kept as `<file>.damaged`).
- Opening runs on the worker thread with a "Unlocking…" indicator (the KDF takes up
  to a second).

### Recovery key display

After creating or encrypting with a recovery key, and after "New recovery key", a
modal shows the key in a large monospaced font (groups of four), with **Copy**,
**Save to file…** (plain text file the user chooses; the dialog suggests a folder
other than the book's) and a checkbox "I've stored this key somewhere safe"; **Done**
is enabled only after the checkbox. Text: without the password and without this key,
nobody — including MoneyKeep — can open the book.

### Settings → Security (per book)

- Plain book: "This book is not password protected." and **Protect with a
  password…** (password, confirm, recovery key option, and the one-line note that
  earlier unencrypted copies of the file may remain on disk).
- Encrypted book: **Change password…**, **New recovery key…**, **Remove recovery
  key…** (only when one exists), **Save an unencrypted copy…** (save dialog), and
  **Remove password protection…** (confirm dialog). Each asks for the current
  password or recovery key.
- Per-user: **Lock after** Never / 5 / 15 / 30 / 60 minutes without input. Locking
  closes the book (after any running command finishes) and shows the password
  prompt for the same book; Cancel returns to the start screen.

### Phase 2b leftovers

- Period tables: when the period columns scroll, show a visible horizontal scroll bar
  under them, and never draw a partially visible amount at the clipped edge of the
  scroll area without a fade or edge shadow that signals more content (amounts at the
  edge may be cut by the viewport, but the user must see that it scrolls). Net worth
  gets a fixed right column too: the last date's amounts stay visible while the
  earlier periods scroll.
- Net worth chart date axis: the same short labels as the table ("Jan", year once;
  the final "today" point labelled e.g. "Oct 19"), thinned so labels never overlap and
  never clip at the chart edges.

### Additional acceptance tests

47. Creating a password-protected book from the start screen produces a file starting
    with `MCBOOKE1`; the recovery-key modal's Done button is disabled until the
    checkbox is ticked.
48. Opening an encrypted recent book shows the prompt; a wrong password shows "That
    password didn't work." and the book stays closed; the right one opens it.
49. Settings → Security on a plain book → Protect with a password… converts it; the
    Security section then shows the encrypted-book actions.
50. With Lock after 5 minutes and the clock advanced (injected) by 5 minutes of no
    input, the book closes and the prompt appears.
51. Net worth "This year": the Oct 19 column is fully visible at 1200×800 without
    scrolling, a horizontal scroll bar is present, and chart axis labels do not
    overlap (bounding boxes disjoint).

## Revision 7 — text size and fonts (2026-10-19, owner request)

Owner request: keep the text size between sessions, add a control for it, and let
users choose fonts. Sources: egui/eframe documentation (zoom factor, font
definitions; https://docs.rs/egui), `fontdb` crate documentation
(https://docs.rs/fontdb) for finding installed fonts, WCAG 2.2 success criterion
1.4.4 "Resize Text" (text resizable to 200 % without loss of content or function).

All settings here are **per user** (app config), never stored in a book.

### Text size

- Settings → Display gains **Text size**: a "−" button, the current size as a
  percentage, a "+" button and **Reset**, with steps 90, 100, 115, 130, 150, 175,
  200 %. The tooltip names the shortcuts.
- Keyboard: Ctrl + "+" / Ctrl + "=" larger, Ctrl + "−" smaller, Ctrl + 0 reset (Cmd on
  macOS), moving through the same steps. Whatever changes the size (keys or buttons),
  the Settings control and the saved value follow it.
- The size is saved to the app config when it changes and restored at start-up
  before the first frame (no visible jump). Default 100 %. Values outside 90–200 %
  in the config are clamped.
- Size scales the whole interface (egui zoom), so layout rules keep working.

### Fonts

- Settings → Display gains **Interface font** and **Number font**, each a searchable
  list: "Built-in" first, then the font families installed on this computer (found
  at runtime with `fontdb` from the system font folders), alphabetically, each name
  drawn in its own font when it loads.
  - Number font lists only monospaced families (plus "Built-in monospace").
  - Only regular weight/upright faces are loaded (one face per family; TrueType or
    OpenType, including collections).
- A choice applies immediately and is saved as the family name. At start-up the
  saved family is looked up; if it is missing or fails to load, the built-in font
  is used and the message line says "The font <name> isn't available, so the
  built-in font is used." once per start-up. The setting is kept, so the font comes
  back if it is reinstalled.
- The built-in fonts stay as fallback for characters the chosen font lacks (e.g.
  symbols), so no text turns into boxes.
- Fonts are read from the user's machine and never bundled or copied, so no new font
  licences enter the project.
- **Reset display settings** restores theme, density, text size and both fonts to
  their defaults.

### Settings sections

Settings is now long, and Security ended up below the full module list. Settings
gets a section list on the left: **Display**, **Security**, **Import**,
**Features** (the modules). Display is first; the last section shown is remembered
per user.

### Layout at large sizes

Layout rule 1 ("everything fits") now applies at 100 % and 150 % from 1000×700, and
at 200 % from 1400×900. At larger sizes the sidebar may collapse to a toggle button,
and tables may scroll horizontally, but no text overlaps, no control is
unreachable, and amounts are never truncated (WCAG 1.4.4).

### Additional acceptance tests

52. Setting 150 % in Settings, closing and reopening the app (config round trip)
    starts at 150 %; Ctrl + "+" then moves to 175 % and the Settings control shows
    175 %; Ctrl + 0 returns to 100 %.
53. Choosing a font available in the test environment (tests provide a font file
    via an injected font list) applies it and saves its family name; with the
    saved family removed from the list, start-up uses the built-in font and shows
    the message once.
54. The Number font list contains only monospaced families.
55. At 200 % and 1400×900, the register's Balance header, the entry row's save button
    and the Reports table's Total column are inside the window and no amount ends in
    "…"; at 150 % the same holds at 1000×700.
56. Settings shows the four sections; selecting Security shows the Security controls
    without scrolling.
57. Reset display settings restores all five defaults.

### Revision 7a — review finding (2026-10-19)

At 130 % in a 1200×800 window the Scheduled table shrinks Payee, Account and
Category to a bare "…" (headers included) so that Repeats stays whole. Revision 5's
"Repeats gets priority" is limited: every text column keeps a minimum width of 8
average characters (its header always readable); once Payee, Account and Category
reach that minimum, Repeats truncates with an ellipsis (full text in the tooltip).
This minimum applies to every table's text columns.

58. Scheduled table at 130 % and 1200×800: the Payee, Account and Category headers
    are drawn in full and each cell shows at least its first 6 characters; Repeats
    may truncate.

## Revision 8 — owner-reported defects (2026-10-19)

Reported by the owner on Windows with a real (anonymised) credit-card QFX download.

### Credit-card register columns were reversed

The 2c text said Decrease/Increase titles are "Charge"/"Payment" for credit cards.
That is wrong: on a credit card a **charge increases** the amount owed and a
**payment decreases** it. Corrected rule, which supersedes the 2c text:

| Account kind | First amount column | Second amount column |
|---|---|---|
| bank, cash | **Spent** (decreases the balance) | **Received** (increases it) |
| credit card | **Charge** (increases the amount owed) | **Payment** (decreases it) |
| other liabilities (loan, …) | **Increase** | **Payment** |
| other assets | **Decrease** | **Increase** |

- The entry row's two amount fields follow the same order and titles, and typing in
  one still clears the other.
- In these two columns amounts are shown **without a sign**: the column already says
  which way the money moved. (A charge is never shown as "−", a payment never as a
  negative number.) Colour is not used for these two columns. The Balance column keeps
  its sign, and a negative balance keeps the muted red.
- Schedule forms, the import review Amount column and "Show scheduled" rows use the
  same per-kind wording where they show a direction.

### Window size and panel widths were not remembered

The app config held a window size but nothing wrote it. Now:

- The window's size, position and maximised state are saved when they change
  (debounced to at most once a second) and on exit, and restored at start-up. If the
  saved position would put the window mostly off every connected screen (monitor
  removed), the window opens centred at the saved size, shrunk to fit the screen.
- Every panel the user can resize (the sidebar, the Reports list, the Settings
  section list) remembers its width the same way.
- Like other display settings these are per user, never in a book; Reset display
  settings also resets panel widths (not the window).

### Additional acceptance tests

59. A credit-card register with a 35.94 purchase and a 1,003.15 payment: "35.94"
    appears under **Charge**, "1,003.15" under **Payment** (no minus sign); the
    entry row's fields are titled Charge then Payment.
60. A bank register still shows Spent then Received, unsigned.
61. Config round trip: a window state of 1400×900 at (100, 80), not maximised, and a
    sidebar width of 320 are restored; a saved position of (−5000, −5000) opens
    centred.

### Revision 8a — minimum window size (2026-10-19)

The layout rules hold from 1000×700 logical pixels, but the window could be made (and
saved, and restored) smaller, which hides the register's Balance column. The window
gets a minimum inner size of 1000×700 logical pixels — or the screen's usable size if
that is smaller. A saved size below the minimum is raised to it on restore. At text
sizes above 100 % the minimum stays 1000×700 (the larger-size rules of revision 7
describe how screens adapt).

62. A config with a saved window of 900×600 restores as 1000×700; the window cannot be
    resized below 1000×700.

## Revision 9 — "Edit with Pop-Up" (2026-10-19, owner request)

### Setting

Settings gains an **Operation** section (after Display) with one checkbox:
**Edit with Pop-Up**, ticked by default, described as "Click an uncleared
transaction to edit it in a pop-up window." Per user (app config), not per book.

### Behaviour when on

- A **single click anywhere on a register row** whose reconcile status is
  **uncleared** (`n`) and whose status is posted opens the transaction in a modal
  edit window. This includes clicks on the status cell (the modal has its own status
  control). The clicked row also becomes the selected row.
- Rows that are cleared (`c`), reconciled (`r`) or voided, the entry row, and
  "Show scheduled" rows keep their existing click behaviour (select only; Enter or the
  row menu still edits inline).
- Keyboard: Enter on a selected uncleared row opens the pop-up too (instead of inline
  editing). The row menu's **Edit** opens the pop-up for uncleared rows.

### The pop-up

- A modal window titled "Edit transaction", centred over the register, about
  480 logical pixels wide, with one field per line, label on the left and field on
  the right, all fields left-aligned on one edge, in this order: **Date**, **No.**,
  **Payee** (with suggestions), **Category** (with suggestions; a split transaction
  shows "Split (n)" with an **Edit splits…** button that opens the split editor),
  **Memo** (multi-line, 3 lines high), the two amount fields with the account's own
  titles and order (revision 8 table; typing in one clears the other), and
  **Status** (Uncleared / Cleared).
- Buttons along the bottom: **Save** (primary), **Cancel**, and on the left
  **Delete…** (asks to confirm) and **Void**.
- Save calls `txn.update` with the edited fields (and `txn.set_reconcile` if Status
  changed); Delete calls `txn.delete`, Void `txn.void`. Command errors appear inside
  the pop-up under the field they concern, and the pop-up stays open with the user's
  values.
- Keys: Tab / Shift+Tab move between fields in order; Enter in a single-line field
  saves (Enter in Memo adds a line; Ctrl+Enter saves from anywhere); Esc cancels.
  Focus starts in the field of the column that was clicked (Payee if the click was
  on Date, No. or Status), with its text selected. Exactly one focus ring (rule 4).
- Closing returns focus to the register with the same row selected; the register and
  balances refresh from the commands.
- Fits and stays readable at every supported text size (revision 7); at 200 % it may
  grow taller and scroll inside.

### Behaviour when off

Unchanged from earlier revisions: a click selects, Enter edits inline.

### Also in this round (from the revision 8 review)

At 1000×700 the register's vertical scroll bar covers the last digit of the Balance
column. The scroll bar gets its own gutter so no column is drawn under it.

### Additional acceptance tests

63. With the setting on (default), clicking the Memo cell of an uncleared row opens
    the pop-up with Memo focused; fields appear in the listed order, vertically, with
    a shared left edge.
64. Changing the amount in the pop-up and pressing Enter saves via `txn.update`; the
    register shows the new amount and balance; Undo reverts it.
65. Clicking a cleared row does not open the pop-up; it selects the row.
66. With the setting off, clicking an uncleared row only selects it.
67. A credit-card transaction's pop-up shows Charge then Payment.
68. A command error (e.g. date in the closed period) shows in the pop-up, which stays
    open with the typed values.
69. At 1000×700 the Balance column's right edge is left of the vertical scroll bar.

## Revision 10 — investments screens (Phase 3a, 2026-10-20)

Screens for `investments-core.md`. Shown only while the `investments` module is on
(Settings → Features). Layout rules of revisions 2, 7 and 7a apply.

### Sidebar and Home

- The *Investments* group lists brokerage accounts with their **total market value**
  (cash + holdings at latest prices); security child accounts are not listed
  separately in the sidebar. A "no price" marker appears when any holding lacks a
  price.
- Home's net worth uses market values.

### Investment account view

Opening a brokerage account shows two tabs, **Holdings** (default) and **Activity**.

- **Holdings:** "as of" date (default today) and a table: Security (symbol and name),
  Shares, Price, Price date, Market value, Cost basis, Gain, Gain %, and Weight;
  then a **Cash** row and a **Total** row. Amounts right-aligned; Gain shows its sign;
  rows with no price show "no price" in Price and are excluded from the totals with
  a footnote. Clicking a security opens its **Lots** (a modal with the `inv.lots`
  table).
- **Activity** (the account's register): Date, Action (Buy, Sell, Dividend,
  Reinvest, Interest, Fee, Shares in, Shares out, Split, Return of capital, Transfer),
  Security, Shares, Price, Commission, Cash amount (signed: + into the account's cash,
  − out), Cash balance. Non-investment transfers show "Transfer" with the other
  account.
- Action buttons above both tabs: **Buy**, **Sell**, **Dividend**, **Reinvest**, and
  **More…** (Interest, Fee, Shares in, Shares out, Split, Return of capital).
- Each action opens a form laid out like the edit pop-up (revision 9: one field per
  line, shared left edge): date, security (searchable; "New security…" inline),
  quantity, price, commission, and a live **Total** computed by the command's rules
  (shown from `inv.preview_sell` for sales). The total can be overwritten to match the
  broker's statement (sent as `amount`).
- **Sell form:** shows the method in use and, below, the lots that will be consumed
  (acquired, shares, basis, term, gain) from `inv.preview_sell`, updating as the user
  types. A **Choose lots…** toggle switches to a lot table with a shares-to-sell field
  per lot (specific identification); the total must equal the quantity before Save is
  enabled. The estimated short-term and long-term gains are shown, with a small note
  that MoneyKeep keeps records and does not give tax advice.
- Errors (e.g. `insufficient_shares`, `would_change_closed`) appear in the form.
- Clicking an Activity row with "Edit with Pop-Up" on opens the matching action form
  pre-filled (saved with `inv.update`); Delete and Void are available as in revision 9.

### Securities and prices

A **Securities** view (reached from Accounts and from the investment account view):
the security list with symbol, name, kind, basis method, latest price and date, and
total shares held across accounts; **New security…**, **Edit…**, **Update prices…**
(a grid of every held security with today's date and a price field each; Save calls
`price.set` per filled row, in one batch), **Import prices…** (CSV file dialog →
`price.import`, then a summary with any warnings), and a per-security price history
list.

### Reports

The Reports list gains **Holdings**, **Realized gains** (short-term and long-term
sections with subtotals, and the not-tax-advice note) and **Investment income**, each
with the usual controls and Export CSV. Net worth uses market values.

### Additional acceptance tests

70. With the module on and the `investments-core.md` test-5 book, the Holdings tab on
    2026-10-19 shows VTI 6.1 shares, price 125.00, market value 762.50, cost basis
    682.00, gain 80.50, and Cash 4,905.00; the sidebar shows the account at 5,667.50.
71. The Sell form for 12 shares @ 250.00 with 5.00 commission (test-1 state before the
    sale) shows the FIFO lots and gains 490.83 long-term and 59.17 short-term before
    saving; choosing lots B 5 and A 7 updates them to 343.58 and 147.92.
72. Update prices… with one price entered saves it and refreshes Holdings.
73. Realized gains report for 2026 shows the test-1 rows and subtotals.
74. With the module off: no Investments group actions, no Securities view, and no
    investment reports in the list.

### Revision 10a — investment form polish (2026-10-20 review)

Found on the release build (1200×800). The figures were all correct; these are
presentation fixes.

1. **Field hints.** Action forms show internal-looking placeholder text
   ("Action security", "Action quant…", "Action com…"). Use plain hints: Security
   "Symbol or name", Shares "Number of shares", Price "Per share", Commission
   "0.00", Total "Calculated" (amount field), Amount "0.00".
2. **No premature errors.** The preview's argument errors ("invalid arguments: give
   the security") must not appear while required fields are still empty; the
   preview runs only once security, shares and price (or amount) are filled. When a
   real error remains, it is shown in plain words next to the field ("Enter the
   number of shares", "You hold 6.1 shares of VTI in this account on that date").
3. **Titles:** "Buy shares", "Sell shares", "Record a dividend", "Reinvest a
   dividend", "Record interest", "Record a fee", "Move shares in", "Move shares out",
   "Record a stock split", "Record a return of capital" (and "Edit …" when editing).
4. **Sell and Move shares out** offer only securities held in this account on the
   form's date, each with the shares held (e.g. "VTI — 6.1 held"); no "New security…"
   there.
5. **Field widths:** inputs fill the form's field column (at least 240 logical
   pixels); amount and quantity inputs right-align their text; the lot table's
   numeric columns are right-aligned like other tables.
6. **Stable position:** a form keeps its top edge where it opened while its content
   grows or shrinks (the lot preview appearing must not move the fields under the
   pointer).
7. **Number font everywhere:** all amounts and quantities in the Holdings, Activity,
   Lots, Securities and the three investment reports use the number font, as the
   register does (today only Gain does).

75. Opening "Sell shares" shows no error text; the security list offers "VTI — 6.1
    held" and no "New security…"; filling shares and price shows the lot preview and
    the Shares field stays at the same screen position before and after the preview
    appears.
76. No hint text in any action form contains the word "Action".

### Revision 10b — security choice (2026-10-20 review)

On "Sell shares" the held security is shown as an unselected chip ("VTI — 6.1 held")
that looks like a filled-in value, so the preview never runs until the user happens
to click it. Rules for the Security field in every action form:

- When exactly one security qualifies (held in the account for Sell / Move shares
  out, or the form was opened from a security's row), it is **preselected**.
- Otherwise the field is a dropdown with the placeholder "Choose a security" and
  nothing preselected; the chosen value shows as the dropdown's text.
- The lot preview table gives each column at least the width of its header plus one
  space, so numbers never touch the next column.

77. With one security held, opening Sell shares, typing 2 shares and 130 price shows
    the lot preview without touching the Security field.
78. With two securities held, the Security field shows "Choose a security" until one
    is chosen.

## Revision 11 — investment import screens (Phase 3b, 2026-10-20)

Screens for `import-investments.md`. Import… and drag-and-drop now also accept
`.json` and `.xml` (and `.csv` / `.ofx` / `.qfx` investment files).

### Import review for an investment section

- Section header as for bank sections (description with last four digits, account
  picker limited to brokerage accounts, Remember for future files).
- **Securities in this file** panel (shown when any security is new or unmatched):
  one line per source security — the file's ticker/name/id, then either "Matches VTI"
  (with Change…) or "New security" with editable Symbol, Name, Kind and Basis method;
  or "Use existing…" to map it. Commit is disabled while a proposed symbol is empty or
  clashes with a different existing security.
- Rows table (one line per row, layout rules apply): Date, Action (word), Security,
  Shares, Price, Amount, What will happen (chips: New, Matches existing, Already
  imported, Will void, Not supported, Needs review), Skip. Cash rows show their
  category picker in place of Security/Shares/Price, as bank rows do. Hovering a row
  shows its notes and the original record.
- Rows needing input (e.g. a transfer in without cost basis) show an inline **Enter
  basis…** link opening a small form (basis, acquired date).
- **Holdings check** panel under the table: "Holdings agree with the statement" or a
  list of differences (security, in your book, on the statement), plus cash.
- Footer counts and **Add to books** / **Discard** as for bank imports.

### Profile editor (CSV, JSON, XML broker files)

Opened when a CSV/JSON/XML investment file matches no profile, or from Settings →
Import → Investment profiles. It shows:

- the detected format and record path (editable), and the first record's fields;
- for each MoneyKeep field (Date, Action, Symbol, CUSIP, Security name, Shares,
  Price, Commission, Fees, Amount, Description, Transaction id, Account) a picker of
  the file's columns / keys / elements, pre-filled from `inv_profile.suggest`;
- date and number formats;
- **Actions**: every distinct action text found in the file, each with a picker
  (Buy, Sell, Dividend — ordinary / qualified / capital gain long / short / tax
  exempt, Reinvest, Interest, Fee, Shares in, Shares out, Split, Return of capital,
  Cash, Ignore) and a match type (equals / starts with / contains);
- a live preview of the first 20 rows as MoneyKeep will read them, with errors in
  plain words;
- **Save profile** (name) then continue to the import review.

### Additional acceptance tests

79. Importing the OFX fixture opens a review with an investment section, the
    Securities panel showing VTI as "New security" with Symbol VTI, the option row
    chip "Not supported", and the Holdings check panel.
80. Committing shows the new transactions in the Activity tab and the holdings in the
    Holdings tab; Undo removes them.
81. Importing the JSON fixture with no profile opens the profile editor with Date,
    Symbol, Shares, Price, Fees and Amount pre-filled and "Bought" listed under
    Actions; choosing Buy, saving and continuing shows the buy in the review.
82. A second JSON file with the same keys uses the saved profile without opening the
    editor.

## Revision 12 — Definitions screens (2026-10-20)

Screens for `definition-files.md` and the visible parts of `input-safety.md`.

### Settings → Definitions (new section, after Import)

- A list of installed packages: name, version, id, installed date, item counts, and
  "N items edited since install". Row actions: **Details** (items, provenance, file
  SHA-256), **Export again…**, **Uninstall…** (lists what will be removed and what is
  kept because it was edited or holds transactions; confirm).
- **Install from file…** (file dialog filtered to `.mkdef`; also by dropping a
  `.mkdef` file on the window) opens the review below.
- **Create definition file…** opens the export form below.

### Install review (modal)

- Header: name, version, id, authors, license, created, tags; the description as
  plain text; homepage as plain selectable text with a Copy button (never a link);
  the file's SHA-256 (shortened, full in a tooltip).
- A plain banner: "Definition files contain import layouts, rules, categories and
  securities. They cannot run programs or change your transactions. Review what will
  be added before installing."
- Warnings and errors list (errors disable Install), each naming the key path.
- Items grouped by kind, each with a checkbox (default on) and its proposed action:
  Add, Update, Unchanged (unchecked, disabled), Conflict (with Replace / Rename… /
  Skip). Expanding an item shows its fields read-only.
- **Install selected** / **Cancel**. After install, a message "Installed <name>:
  N items" with Undo.

### Create definition file (modal)

- Header fields (id, version, name, description, authors, license, tags, homepage).
- Pick items: bank profiles, investment profiles, rules, category subtrees,
  securities — searchable checklists. Rules that point at non-category accounts are
  shown disabled with the reason.
- **Validate** (shows any warnings) and **Save as…** (save dialog, suggested name
  `<id>-<version>.mkdef`).

### Import review notes

Rows whose text was cleaned (`input-safety.md` R3) show a small "cleaned" marker with
a tooltip listing what was removed.

### Additional acceptance tests

83. Dropping a valid `.mkdef` file opens the install review with the banner, items and
    actions; installing shows the package under Settings → Definitions; Undo removes it.
84. A `.mkdef` with an error shows the error with its key path and Install disabled.
85. The homepage is not clickable (no link widget in the accessibility tree).
86. Create definition file… with two rules and a category subtree saves a file that
    the install review then shows as all Unchanged for the same book.

## Revision 13 — performance, wash sales, corporate actions (Phase 3c, 2026-10-20)

- **Reports → Performance** (with the investment reports): period presets, group
  (By account / By security / Total), and a table: Name, Start value, Net flows,
  Income, Fees, End value, Gain, **Money-weighted**, **Time-weighted** (percent, with
  "not annualised" shown as a small suffix when applicable, and "n/a" with a tooltip
  reason). A line chart of account value over time (one line per account, or the
  total). A short explanation under the table: money-weighted reflects your timing of
  deposits and withdrawals; time-weighted removes it, to compare with an index. The
  not-tax-or-investment-advice note.
- **Realized gains report** gains **Code** and **Adjustment** columns; rows with a
  wash sale show "W" and the amount, with a tooltip naming the replacement purchase
  and the note that MoneyKeep records but does not apply the adjustment.
- **Sell form:** when the preview shows a loss, a notice lists purchases of the same
  security (or its wash group) in the last 30 days and reminds that buying again
  within 30 days would make it a wash sale (`inv.wash_check`).
- **More… menu** in the investment account view gains **Merger…**, **Spin-off…**,
  **Symbol change…**, **Cash in lieu…**; each opens a form in the revision 9 style
  with a live preview of the resulting lots (quantities, bases, acquired dates) and
  any recognised gain, before saving. The Spin-off form explains in one line that the
  basis percentage comes from the company's published basis statement (IRS Form
  8937).
- **Securities view:** **Wash groups…** to link substantially identical securities
  (a list of groups with members; add/remove).

### Additional acceptance tests

87. The Performance report for the `investments-advanced.md` AT3 book shows TWR
    20.00 % and MWR 19.61 % and a value line ending at 2,400.00.
88. The realized-gains report for AT5 shows code W and adjustment 120.00 on the sale
    row, and "Disallowed (wash sales) 120.00" in the totals.
89. The Spin-off form for AT7 previews a 50-share lot with basis 625.00 before saving;
    saving shows both lots in Lots.
90. The Sell form for a loss shows the 30-day notice listing a purchase made 10 days
    earlier.

## Revision 14 — business sales screens (Phase 4a, 2026-10-20)

Screens for `business-sales.md`; visible only while the related modules are on. Layout
rules of revisions 2, 7, 7a and the revision 9 form style apply.

- **Settings → Features:** each module row expands, when switched on, to show its
  settings (from `module.settings`) as labelled fields with their descriptions;
  changes save immediately (`module.set_setting`) with field-level errors.
- **Top bar:** **Sales** (when `invoicing` is on). Sales has tabs: **Invoices**,
  **Payments**, **Estimates** (module on), **Customers**, **Items** (module on), and
  **Sales tax** (module on).
- **Lists:** Invoices (number, customer, date, due, total, balance, status chip;
  filters by status/customer/overdue); Payments (date, customer, method, amount,
  unapplied, deposited?); Estimates (number, customer, date, expires, total,
  status); Customers (name, open balance, overdue, credit); Items; Sales tax (rates,
  groups, liability per rate, **Pay sales tax…**).
- **Invoice editor:** a document view: customer picker (with "New customer…"),
  terms, date, due date, number; a lines grid (item/account, description, qty, rate,
  amount, taxable) with add/remove/reorder, subtotal and discount lines; tax picker;
  live totals from `invoice.preview`; message and memo. Buttons: **Save draft**,
  **Save and finalise**, **PDF…** (save dialog), **Record payment…**, **Duplicate**,
  **Void…**, **Delete…**. Estimates use the same editor with **Convert to invoice**.
- **Record payment:** customer, date, amount, method, reference, deposit to; a list
  of the customer's open invoices with "apply" amounts pre-filled oldest first; shows
  any credit left.
- **Make deposit:** lists undeposited payments with checkboxes, the bank account and
  date, running total; saves one deposit.
- **Customer view:** details form, open invoices, payments, credits, and **Refund…**.
- **Reports:** A/R aging (summary and detail), Customer balances, Open invoices, Sales
  by customer, Sales by item, Sales tax liability, Balance sheet.

### Additional acceptance tests

91. Turning on Estimates in Settings shows its settings list; changing "Default days
    valid" to 45 persists.
92. Creating the `business-sales.md` AT2 invoice in the editor shows total 1,314.32
    from the preview; Save and finalise posts it; PDF… writes a file.
93. Record payment of 1,000.00 pre-applies to INV-1001 and the list then shows balance
    314.32 and status Partly paid.
94. Make deposit with two payments creates one 1,200.00 transaction in Checking's
    register.
95. With `invoicing` off, no Sales item in the top bar.

## Revision 15 — purchasing screens (Phase 4b, 2026-10-21)

Screens for `business-purchases.md`, shown while `bills` is on.

- **Top bar:** **Purchases**, with tabs **Bills**, **Payments**, **Purchase orders**
  (module on), **Vendors**, and **1099** (module on).
- **Bills list:** vendor, number, date, due, total, balance, status chip; filters
  (open, overdue, due in 7 days, vendor). **Pay bills…** opens the payment run.
- **Bill editor** (revision 9 form style, document layout like the invoice editor):
  vendor (with New vendor…), number, date, terms, due date, expense lines and item
  lines, memo, live total; buttons Save, Pay…, Duplicate, Void…, Delete….
- **Pay bills (payment run):** pick bank/card account, date, method, starting check
  number; a table of open bills (vendor, number, due, balance, pay amount with
  checkbox; quick filters "Due by …", "Overdue"); total; Pay selected → one payment
  per vendor.
- **Payments tab:** date, vendor, method, check number, amount, unapplied; void.
- **Purchase order editor** with **Receive…** (quantities per line, creating a bill)
  and **PDF…**.
- **Vendor view:** details incl. 1099 fields (TIN on file as a checkbox — no TIN
  field), open bills, payments, credits, Refund received….
- **1099 tab:** year picker, the `report.1099` table with flags, threshold table
  editor (per year), note that MoneyKeep does not file forms, Export CSV….
- **Register:** a **Vendor** column picker on expense transactions (optional link for
  1099), visible when the `1099` module is on.
- **Home Attention:** bills overdue / due in the next 7 days.
- **Reports:** A/P aging, Vendor balances, Unpaid bills, Expenses by vendor, Open
  purchase orders, Check register, 1099 summary.

### Additional acceptance tests

96. Creating the AT1 bill in the editor posts it; Pay… with a check from Checking
    shows check 1001 in the Checking register.
97. Pay bills with three bills from two vendors creates two payments with consecutive
    check numbers.
98. The 1099 tab for 2026 shows J. Smith Design flagged with 2,400.00 and 400.00
    excluded card payments.
99. With `bills` off, no Purchases item in the top bar.

## Revision 16 — dimensions, nonprofit reports, statements, templates, IIF (Phase 4c, 2026-10-21)

- **Lists:** Accounts gains tabs **Classes**, **Locations**, **Funds** (each shown
  while its module is on): name tree, function (classes), restriction (funds), active
  toggle, and usage count; New…/Edit…/Deactivate.
- **Tag pickers:** the register entry row, the edit pop-up, split editor, document
  editors (header and lines), schedule form and rule editor gain Class / Location /
  Fund pickers for modules that are on (compact, searchable). When `*.required` is
  on, missing values are marked in the form before saving.
- **Funds:** a **Release from restriction…** action (fund, amount, date, memo) and
  **Transfer between funds…** on the Funds tab; overspent funds show a red marker
  with the amount; auto-created releases appear in registers with a link to their
  expense.
- **Reports:** Profit and loss (with Columns: Total / Class / Location / Fund / Month
  / Quarter / Year), Balance sheet by fund, and — when Funds is on — Statement of
  activities, Statement of financial position, Functional expenses, Fund activity.
  Wide column sets scroll horizontally with the row labels and Total fixed
  (revision 5 rules).
- **Customer statements:** on the customer view, **Statement…** (period, kind) →
  preview + PDF…; Sales → **Statements…** batch (date, customers with balances,
  one PDF or one per customer, folder picker).
- **Late fees:** Sales → **Late fees…**: as-of date, the preview table (customer,
  invoice, days late, balance, fee) with checkboxes, Assess selected.
- **Templates:** Settings → Invoicing gains **Templates** (list, New/Edit: accent
  colour, font, show options, payment instructions, footer, logo upload with a
  preview and the size limits stated, Remove logo) and the default template.
- **IIF:** Import… accepts `.iif`; the review shows an **Accounts in this file**
  mapping table (map or create with type), classes and names mapping, then the usual
  rows table. File → **Export IIF…** (dates, accounts) shows the limitations (no
  memos, numbers or document details; non-ASCII written as `?`) before saving.

### Additional acceptance tests

100. The Statement of activities for `business-dimensions.md` AT1 shows the three
     columns with change in net assets 5,700.00 / 500.00 / 6,200.00.
101. Tagging an expense with fund Youth Mission in the edit pop-up creates the linked
     release (visible in the register) and Undo removes both.
102. Late fees… for AT8 lists INV-1001 with fee 4.71; Assess creates the late-fee
     invoice.
103. Uploading a 3,000 × 3,000 PNG logo shows the size limit error; a valid logo shows
     in the template preview.
104. Importing the AT10 IIF file shows the accounts mapping table with three names.

## Revision 17 — bank feeds (Phase 5a, 2026-10-21)

Shown while `bank-feeds` is on.

- **Settings → Connections** (new section): list of connections (label, provider,
  host, status chip — Active / Needs attention / Revoked / Disabled, last sync,
  last error in plain words) with **Sync now**, **Rename**, **Disable/Enable**,
  **Disconnect…** (explains that SimpleFIN access should also be revoked on the
  provider's site).
- **Add connection…**: explains in two lines what SimpleFIN is and that MoneyKeep
  never sees the bank password; a field for the **setup token** (masked, with Show);
  after decoding, a confirmation "MoneyKeep will contact bridge.simplefin.org to
  claim this token. Continue?"; then the remote accounts list, each with a picker to
  link to an existing account, create a new one, or ignore; "Start from" date; include
  pending; add new transactions automatically when nothing needs review.
- **Book not password protected and no OS credential store:** the dialog says the
  connection can't be saved securely and offers to open Settings → Security.
- **Accounts:** linked accounts show a small "feed" marker with the last sync time in
  a tooltip; the register toolbar gains **Sync now** for linked accounts.
- **Home Attention:** staged feed batches and connections needing attention; clicking
  opens the import review or the connection.
- Feed batches open in the normal import review, titled "SimpleFIN — <label>".

### Additional acceptance tests

105. Add connection with a test token (fake server) lists two remote accounts; linking
     one to Checking and syncing shows "SimpleFIN: N new transactions to review" on
     Home.
106. The setup token field is masked and the token does not appear in any label after
     the dialog closes.
107. A revoked connection shows the Revoked chip and the reconnect message.

### Revision 17a — background sync (2026-10-21)

Syncs run in the background (`connectivity.md` revision 2): a "Syncing…" status with
a spinner and **Cancel** on the connection row and on the register's Sync now button;
the window and other commands stay usable. A "Not finished — resume" row appears for
interrupted connections, and "Unused credential" rows offer Delete.

108. During a 5 s fake-server delay, clicking another view responds immediately and
     the sync then finishes with its Home attention line.

## Revision 18 — payment processors (Phase 5b, 2026-10-21)

Shown while `payment-processing` is on.

- **Add connection…** offers Stripe, Square and PayPal alongside SimpleFIN, each with
  short instructions for creating the read-only credential (Stripe restricted key with
  read access only; Square personal access token with payout read; PayPal REST app
  client id and secret), masked fields, and the confirm step.
- **Processor settings** (from the connection row): mode (Summary / Detailed), the
  clearing account, and an account picker per category (Sales, Refunds, Fees,
  Disputes, Adjustments, Reserves, Tax, Other), plus "Match invoice numbers".
- **Import review for processor batches:** one row per payout (date, payout id
  shortened, gross, refunds, fees, other, **net**), expandable to its entries; rows
  proposing "Apply to INV-…" show that action with Accept / Use sales instead.
- **Home Attention:** clearing balances not matched to a bank deposit after the match
  window.

### Additional acceptance tests

109. Adding a Stripe connection against the fake server and syncing shows one payout
     row with net 945.30 expandable to its five entries.
110. Processor settings change the fees account; the next staged batch uses it.
111. A clearing balance older than the match window shows on Home.

## Revision 19 — OFX Direct Connect and SnapTrade (Phase 5c, 2026-10-21)

Spec: `connectivity-brokerage.md`.

- **Add connection…** gains **Bank (OFX Direct Connect)** and **Brokerage
  (SnapTrade)**.
  - OFX: pick an installed institution profile or "Enter details" (name, URL, ORG,
    FID, broker id, OFX version, application id/version under "Advanced"). The next
    step shows the **host** in large type with "Your user id and password will be
    sent to <host>." and the tick-box "I recognise this address as my bank's";
    Connect stays disabled until it is ticked. Then user id, password (masked), extra
    credential fields when the profile asks for them, "Ask for my password each
    time", and the client id with a Copy button. After connecting: the account list
    from the bank (inactive ones greyed with their status) and "Add an account by
    hand…".
  - SnapTrade: three short steps (create a personal key in the SnapTrade dashboard;
    connect your brokerages there; paste the client id and consumer key), two masked
    fields, then the account list with numbers masked to the last 4 and each
    account's sync status.
- **Extra questions dialog** (OFX status 3000/15512): the bank's questions, one answer
  field each (masked), or a one-time code field; Cancel leaves the connection needing
  attention.
- **Password prompt** for connections set to "Ask for my password each time", shown on
  Sync now.
- **Connection row** (OFX): host, OFX version, "Profile changed" notice with "Switch to
  <new host>…" (same tick-box).
- **Import review for brokerage batches:** a **Holdings check** panel above the rows:
  per security, provider quantity vs MoneyKeep quantity after this import, and cash;
  differences highlighted with "Add a shares-in/out row for the difference".
- **Home Attention:** "<account>: N holdings differ from <provider>", "Sync needs your
  password", "Your bank asks extra questions".

### Additional acceptance tests

112. Adding an OFX connection against the fake server: Connect stays disabled until the
     host tick-box is ticked; after connecting, the pending investment account is
     greyed.
113. A status 3000 during Sync now opens the extra questions dialog; answering
     completes the sync.
114. A SnapTrade sync with a quantity difference shows the Holdings check panel with
     the difference highlighted and a Home Attention entry.

## Revision 20 — date format (2026-10-22, owner request)

Sources added: ISO 8601 (calendar dates), Unicode CLDR English month names and
abbreviations (public data), owner request. The format list and the wording below are
MoneyKeep's own; no target product was observed.

### Setting

Settings → **Operation** gains **Date format**, a drop-down under "Edit with Pop-Up".
Per user (app config, like revision 9's setting), not per book. Default
**MM-DD-YYYY**. Each choice is listed as its pattern followed by today's date written
in it in secondary text, e.g. "MM-DD-YYYY  10-22-2026". Description under it: "How
dates are shown and typed. Dates are always stored the same way, so changing this
never changes your data."

The choices, in this order:

| Group | Patterns |
|---|---|
| Month first | `MM-DD-YYYY` (default), `MM/DD/YYYY`, `MM.DD.YYYY`, `M/D/YYYY`, `MM-DD-YY`, `MM/DD/YY`, `M/D/YY` |
| Day first | `DD-MM-YYYY`, `DD/MM/YYYY`, `DD.MM.YYYY`, `D/M/YYYY`, `D.M.YYYY`, `DD-MM-YY`, `DD/MM/YY`, `DD.MM.YY` |
| Year first | `YYYY-MM-DD`, `YYYY/MM/DD`, `YYYY.MM.DD`, `YYYYMMDD` |
| Month name | `DD-Mon-YYYY` (22-Oct-2026), `DD Mon YYYY` (22 Oct 2026), `Mon DD, YYYY` (Oct 22, 2026), `Month DD, YYYY` (October 22, 2026), `DD Month YYYY` (22 October 2026) |

Token meanings: `YYYY` four-digit year; `YY` two-digit year; `MM`/`DD` two digits with
leading zero; `M`/`D` no leading zero; `Mon` three-letter English month abbreviation
(Jan … Dec, "Sep" for September); `Month` the full English month name.

### What it changes

- **Display:** every date the desktop app shows as a value — registers, the edit
  pop-up, lists (invoices, bills, payments, scheduled, budgets, statements,
  connections' last sync date), import review rows, reconciliation, investment
  screens, on-screen reports, Home — is written in the chosen format. Date-and-time
  values show the date part in the chosen format; the time part is unchanged.
- **Entry:** every field where a date is typed reads the chosen format (rules below)
  and, when the field loses focus or the form saves, rewrites a valid entry in the
  chosen format.
- **Not changed:** storage, commands, the JSON control channel, MCP, the CLI and
  audit text keep ISO `YYYY-MM-DD`. Messages composed by commands that contain dates
  are shown as sent. CSV exports keep ISO dates (they are data). PDFs (invoices,
  statements, printed reports) are unchanged in this revision. Import profiles'
  own date formats (CSV, investment profiles) are separate and unchanged.
- Changing the setting re-renders immediately; no book write.
- Date columns are wide enough for the longest value the chosen format can produce
  (e.g. "September 30, 2026") at every supported text size (revision 7), with no
  clipping.

### Reading a typed date

Given the chosen format F, a typed text (trimmed) is accepted when it matches one of:

1. **F itself, leniently:** for numeric formats any of `-`, `/`, `.` or a space is
   accepted as the separator, and leading zeros are optional (`1-5-2026` is fine
   under `MM-DD-YYYY`). Field order always follows F. `YYYYMMDD` requires exactly
   eight digits.
2. **F's order without a year** (numeric formats with separators, and name formats):
   the year is the current year (e.g. `10-22` under `MM-DD-YYYY`).
3. **Two-digit years** are accepted where F has `YYYY`, and four-digit years where F
   has `YY`. A two-digit year means the year ending in those digits within the range
   current year − 80 … current year + 19.
4. **ISO `YYYY-MM-DD`** is always accepted, whatever F is (a four-digit first field is
   never ambiguous).
5. **Name formats:** month names and abbreviations match case-insensitively, in any of
   the name forms (so "oct 22 2026" and "October 22, 2026" are both fine under
   `Mon DD, YYYY`); the comma is optional; the day/month order follows F.

Anything else, or an impossible date (02-30-2026), is refused: the field shows the
error "Enter the date as <F>" (e.g. "Enter the date as MM-DD-YYYY") under it, as for
other field errors, and nothing is saved. Existing shortcuts keep working: in the
register's Date field `+`/`-` (and the existing arrow keys) move the date by a day,
reading and writing it in F.

### Format guide in the field

- An **empty** date field that is focused or being edited shows F as greyed
  placeholder text (e.g. `MM-DD-YYYY`). Empty unfocused date fields show it too.
- While the user types, the **rest of the pattern** is shown greyed immediately after
  the typed text, as long as what is typed so far fits F character by character
  (digits where F has digit tokens, F's separator — or one of the lenient
  separators — where F has one, letters where F has a month name). E.g. under
  `MM-DD-YYYY`, typing `10-2` shows `10-2` followed by the greyed rest of the pattern
  from position 5 on, `D-YYYY`. When the
  typed text stops fitting (e.g. leading zeros left out, or a name typed in full),
  the guide is hidden rather than shown misaligned. The guide is never part of the
  field's text, is not selectable, and is not read by screen readers as the value
  (the accessible description of the field is "Date, format <F>").
- This applies to every date field: register entry row and edit pop-up, investment
  forms, invoice/bill/payment/refund/deposit forms, statement and report date ranges,
  scheduled transactions, reconciliation statement date, import and connection
  "sync from" dates.

### Implementation notes (behaviour, not code)

- One shared formatter/reader for all screens, so the rules above are identical
  everywhere; pure date logic belongs below the UI (no IO), the widget in `mk-ui`.
- Fields keep the text the user typed until it is valid; commands always receive
  ISO dates.

### Additional acceptance tests

115. With a fresh config, Settings → Operation shows Date format = MM-DD-YYYY and the
     register shows a transaction stored as 2026-10-22 as `10-22-2026`.
116. Choosing `DD Mon YYYY` re-renders the register as `22 Oct 2026` without a book
     write (audit log unchanged); restarting the app keeps the choice.
117. Under MM-DD-YYYY, typing `1/5/26` in the register Date field and saving stores
     2026-01-05 and shows `01-05-2026`; `2026-01-05` is also accepted; `13-01-2026`
     shows "Enter the date as MM-DD-YYYY" and saves nothing.
118. Under DD/MM/YYYY, `05/01/2026` stores 2026-01-05.
119. Under `Mon DD, YYYY`, `oct 22 2026` and `October 22, 2026` both store
     2026-10-22.
120. Typing `10-22` with no year stores the current year.
121. An empty focused date field under MM-DD-YYYY shows the greyed `MM-DD-YYYY`;
     after typing `10-2` the greyed text after the typed characters is `D-YYYY`;
     the field's value is `10-2` (the guide is not part of it).
122. `+` in the register Date field under `DD.MM.YYYY` moves `31.12.2026` to
     `01.01.2027`.
123. At 200 % text size with `Month DD, YYYY`, "September 30, 2026" is not clipped in
     the register Date column.
124. The CLI's `txn list` output and MCP results still show `2026-10-22` whatever the
     desktop setting is.

## Revision 21 — imported dates are transaction dates (2026-10-22, owner request)

The owner found that transactions imported from a credit-card download carried the
**posting** date, while the bank's own register lists the **transaction** date, so
the two never lined up. From now on imports use the transaction date whenever the
source gives one: `import-ofx.md` revision 3, `import-csv.md` revision 2,
`connectivity.md` revision 3, and the re-dating rule in `import-pipeline.md`
revision 2.

Screens:

- Settings → **Import** gains **Date to use**: "Transaction date when given" (default)
  / "Posting date" (core setting `import.date_source`). It applies to OFX/QFX files and
  to suggested CSV profiles. Settings → Features → Bank feeds' existing date setting
  now defaults to the transaction date (connectivity revision 3).
- Import review shows rows whose only change is a corrected date as **Date
  corrected** with "<old> → <new>" (dates in the revision 20 format); they are
  committed like duplicates (no new transaction).
- Connection rows (bank feeds) gain **Fix dates of imported transactions…**: a
  confirmation ("Fetch transactions again from <earliest date> and correct dates of
  ones already imported? Reconciled transactions and ones whose date you changed are
  left alone.") then a sync from that date (`connect.sync` with `from`).

Acceptance tests:

125. Settings → Import shows Date to use = "Transaction date when given".
126. Re-importing an OFX file whose rows now carry `DTUSER` earlier than the committed
     dates shows those rows as Date corrected with old → new, and committing changes
     the transactions' dates (audited; Undo restores them).

## Revision 22 — dates in report and chart headings (2026-10-22, owner request)

Revision 20 left report period headings alone. Now:

- A period heading that is a **single date** (e.g. the net worth report's "today"
  column, and the same point on charts) is written as the day and month of the
  revision 20 date format, without the year (the year stays in the line above, as
  now): `MM-DD-YYYY` → `10-19`; `MM/DD/YY` → `10/19`; `M/D/YYYY` → `10/19` without
  leading zeros (`1/5`); `DD.MM.YYYY` → `19.10`; `D/M/YYYY` → `19/10`; year-first
  formats → `10-19` with their own separator (`YYYYMMDD` → `10-19`); `DD-Mon-YYYY` →
  `19-Oct`; `DD Mon YYYY` and `DD Month YYYY` → `19 Oct`; `Mon DD, YYYY` and
  `Month DD, YYYY` → `Oct 19`.
- Month headings ("Oct" with the year above), quarters ("Q4 2026") and years stay as
  they are: a month name is not ambiguous in any format.
- Report titles and subtitles that show a date range ("Oct 1, 2026 – Oct 22, 2026" or
  ISO), and "as of" dates, use the full date format.
- Budget screen month titles ("October 2026") are unchanged.
- On-screen only: CSV and PDF output keep their present dates.

Acceptance tests:

127. The net worth report's single-date column under `MM-DD-YYYY` is headed `10-22`
     with `2026` above; under `DD Mon YYYY`, `22 Oct`; a monthly report still shows
     `Oct`.
128. A report's date-range subtitle under `DD/MM/YYYY` reads `01/10/2026 – 22/10/2026`.

## Revision 23 — accounts slide-out pane with group tabs (2026-10-22, owner request)

Sources: egui documentation (panels, `Area`, animation helpers, tooltips), WCAG 2.2
SC 1.4.13 (content on hover) and 2.1.1 (keyboard); owner request. Icons are
MoneyKeep's own simple drawings.

The left accounts sidebar becomes a narrow **tab rail** plus a **pane** that slides
out from it.

### Tab rail

- Always visible at the window's left edge, about 40 logical pixels wide (scaled by
  text size), full height below the top bar.
- One tab per sidebar group that has at least one non-placeholder account, in sidebar
  order, preceded by an **All accounts** tab:

  | Tab | Icon (drawn as simple vector shapes in the text colour, not font glyphs) |
  |---|---|
  | All accounts | three stacked horizontal lines |
  | Cash & bank | a building with columns |
  | Credit cards | a card with a stripe |
  | Investments | a rising line chart |
  | Other assets | a house |
  | Loans & other debts | a document with a corner fold |
  | Equity | a balance scale |
  | Categories | a tag |

- **Hover** shows a tooltip with the group name and its total (as the group heading
  shows today; Categories shows only its name). The tooltip appears with egui's usual
  tooltip delay.
- **Hover for 3 seconds** (pointer resting on the same tab, not moving off it) opens
  the pane on that tab's group. A small progress cue is not required.
- **Click** opens the pane on that group immediately; clicking the tab of the group
  already shown closes the pane.
- The active tab (the group the open pane shows) is highlighted; when the pane is
  closed, no tab is highlighted.
- Keyboard: the tabs are in the focus order before the main area; Enter or Space acts
  like a click; Ctrl+B toggles the pane on the last group shown. Each tab's accessible
  name is the group name (and total), role tab, with its selected state.
- Tooltip and pane contents meet WCAG 1.4.13: the tooltip is dismissable with Esc and
  stays while the pointer is over it.

### Pane

- Shows the accounts of the selected group exactly as the sidebar does today (heading
  with total, account lines with balances, click opens the register, Categories split
  into Income and Expenses, the "no price" marker); **All accounts** shows every group
  as today's sidebar does. "Show archived" stays at the bottom of the pane.
- Header line: the group name (strong) and two small icon buttons on the right:
  **Pin** (pushpin; tooltip "Keep open" / "Let it slide away") and **Close** (×,
  tooltip "Close"). Both drawn shapes.
- **Slides** in and out horizontally from the rail's right edge over about 150 ms.
- **Resizable** by dragging its right edge; the width is remembered per user (the
  existing remembered sidebar width, revision 8) and restored when the pane next
  opens, pinned or not.
- **Unpinned (default):** the pane floats over the main area (does not move it), with
  a shadow. It closes when the pointer has been outside both the rail and the pane
  for about 1 second, on Esc, on Close, or after an account is clicked (the register
  opens). It does not close while the user is dragging its edge, or while a text
  field or menu inside it has focus.
- **Pinned:** the pane is docked beside the rail and the main area is laid out to its
  right (like today's sidebar). It stays open until Close, its tab is clicked again,
  or Ctrl+B. Hover-to-open still switches groups when it is open.
- Remembered per user (app config, debounced like other display settings): width,
  pinned, last group shown, and — when pinned — whether it was open, so a pinned open
  pane reopens at start-up. An unpinned pane always starts closed. **Reset display
  settings** resets these too (unpinned, closed, All accounts, default width).
- At the layout sizes of revision 7, the pinned pane follows layout rule 1; the
  floating pane may cover the main area but never extends past the window, and its
  width is capped at half the window width.
- The Reports and Settings section lists are unchanged.

### Acceptance tests

129. The rail shows All accounts plus one tab per group with accounts; a book with no
     credit-card account has no Credit cards tab.
130. Hovering the Cash & bank tab shows a tooltip containing "Cash & bank" and its
     total; after 3 seconds of hovering the pane is open on Cash & bank showing only
     that group; moving off before 3 seconds leaves it closed.
131. Clicking the Credit cards tab opens the pane on Credit cards at once; clicking it
     again closes it.
132. Unpinned: the pane overlays the main area (the main area's left edge does not
     move); moving the pointer away for over a second closes it; clicking an account
     opens its register and closes the pane.
133. Pinned: the main area starts right of the pane; moving the pointer away leaves it
     open; after a config round trip it reopens pinned, open, on the same group, at
     the same width.
134. Dragging the pane's edge to 300 then closing and reopening (and after a config
     round trip) shows it at 300.
135. Keyboard: tabbing reaches the rail tabs; Enter opens the pane; Esc closes an
     unpinned pane; Ctrl+B toggles it.
136. Reset display settings returns to unpinned, closed, All accounts, default width.
137. At 200 % text size from 1400×900, rail icons and the pinned pane meet layout
     rule 1 (no clipping or overlap).

## Revision 24 — every edit in a pop-up, fast entry, New button (2026-10-22, owner request)

### Why

The owner reported that the edit pop-up "is not working". Cause found: revision 9
opens the pop-up only for **uncleared** rows, but downloaded bank and card rows are
imported **cleared** (`import-pipeline`: OFX/QFX/QBO and CSV rows start `c`), so
clicking almost any real row did nothing. The owner also wants the register's inline
entry line removed and every edit done in a pop-up.

### Changes to revisions 2, 8 and 9

- The register's **entry row** (the always-present line for a new transaction) and
  **inline editing** of rows are removed. The register shows only transaction rows.
- Settings → Operation's **Edit with Pop-Up** checkbox is removed; the pop-up is
  always used. The old config field is ignored when read.
- **Opening the pop-up:** a single click on any posted row opens "Edit transaction" —
  uncleared, cleared and reconciled alike (the revision 9 focus rule for the clicked
  column stays). Enter on a selected row, the row menu's **Edit**, and a double-click
  do the same. Voided rows open it too, read-only (see below).
- **Status** control in the pop-up: Uncleared / Cleared, plus "Reconciled" shown (not
  selectable) for a reconciled row. Changing Uncleared↔Cleared calls
  `txn.set_reconcile` on save as before.
- **Reconciled rows:** the pop-up shows the line "This transaction is reconciled.
  Its date and amounts can't be changed here." under the title; Date, the amount
  fields and Split editing are read-only (shown, not editable); Payee, Category (for a
  two-split transaction), No., Memo and tags stay editable (`ledger-transactions`:
  editing a reconciled split's value is rejected, its memo is allowed). Delete is not
  offered; Void is.
- **Voided rows:** every field read-only; buttons **Unvoid** and **Close**.
- Space on a selected row (toggle cleared) and Delete (confirm, then delete) keep
  working from the register without opening the pop-up.
- The investment register (revision 10) opens its edit forms on click as if the old
  setting were on.

### New transaction

- The register's header line (account name, balances) gains a **+ New** button at its
  far right, primary style, tooltip "Add a transaction (Ctrl+N)". Ctrl+N (and Insert)
  in the register do the same.
- It opens the same pop-up titled **New transaction**, with the revision 9 field order
  and these defaults: Date = the last date saved in this session, else today; other
  fields empty; Status Uncleared. Payee suggestions pre-fill Category and amount from
  last use, with the amount selected (the old entry row's behaviour). Focus starts in
  Date with its text selected.
- Buttons: **Save** (primary) and **Cancel**. Save calls `txn.create`; errors show in
  the pop-up under the field they concern, keeping the values.
- Keys as revision 9: Tab / Shift+Tab, Enter in a single-line field saves, Ctrl+Enter
  saves from anywhere, Esc cancels.

### Fast entry

- A small toggle button in the pop-up's top-right corner, in both "New transaction"
  and "Edit transaction". Its icon is MoneyKeep's own drawing of a **foot with small
  wings at the heel**, painted as simple shapes in the text colour (not a font glyph),
  about one text line high. When on, the button is drawn in the selected/accent style
  and the icon is filled; when off, outlined.
- Tooltip (always, on hover): when off, "Fast entry is off. Turn it on to add several
  transactions in a row: Enter saves each one and a new blank one opens, without
  closing this window."; when on, "Fast entry is on: Enter saves and starts the next
  transaction. Click to turn it off." Accessible name "Fast entry", role toggle, with
  its on/off state. Shortcut Ctrl+Shift+F while the pop-up is open.
- On/off is remembered per user (app config). Default off.
- **When on, in New transaction:** Enter (in any single-line field), Ctrl+Enter or
  Save saves the transaction and, on success, the pop-up stays open and clears for the
  next one: Date keeps the date just used, all other fields empty, Status Uncleared,
  focus in Date with its text selected (so typing replaces it, Tab keeps it). The
  register updates behind the pop-up. A line under the title counts what was saved in
  this pop-up ("3 transactions added"). Esc or Cancel closes; a partly filled form is
  discarded only after "Discard this transaction?" (Esc again or Discard confirms).
  Each saved transaction is its own undo step.
- **When on, in Edit transaction:** saving an edit, on success, turns the pop-up into a
  New transaction form (same defaults, date = the edited transaction's date).
- **When off:** Save closes the pop-up, as before.
- A save that fails keeps the pop-up and the typed values, whatever the mode.

### Acceptance tests (replacing tests 63, 65 and 66 and every entry-row test)

138. A register shows no entry row; Settings → Operation has no "Edit with Pop-Up".
139. Clicking the Payee cell of an imported (cleared) row opens "Edit transaction"
     with Payee focused; changing the memo and pressing Enter saves via `txn.update`
     and closes the pop-up (fast entry off).
140. Clicking a reconciled row opens the pop-up with the reconciled line, Date and
     amounts read-only, Memo editable; saving a memo change works; no Delete button.
141. Clicking a voided row opens a read-only pop-up with Unvoid.
142. **+ New** is the rightmost control on the register header; clicking it (or
     Ctrl+N) opens "New transaction" with Date = today and focus in Date.
143. Fast entry off: entering payee, category and amount then Enter creates the
     transaction and closes the pop-up.
144. Fast entry on: three transactions entered with Enter create three transactions,
     the pop-up stays open showing "3 transactions added", Date keeps the last date,
     other fields are empty; three Undo steps remove them one by one.
145. Hovering the fast entry button shows the tooltip for its current state; clicking
     toggles it; the state survives a config round trip; its accessible role is a
     toggle with the right state.
146. Fast entry on, a command error (closed period date) keeps the pop-up and values
     and adds nothing to the count.
147. Fast entry on in Edit transaction: saving an edit leaves a blank New transaction
     form dated like the edited one.
148. Space on a selected uncleared row still toggles it cleared without opening the
     pop-up; Delete still asks to confirm.

## Revision 25 — compact top bar, calculator, Features menu (2026-10-22, owner request)

Sources: egui documentation (`Window`, `Area`, menus, tooltips), eframe
`ViewportBuilder::with_icon`, rust_decimal documentation, WCAG 2.2 SC 1.1.1 (text
alternatives) and 2.1.1; owner request. All icons are MoneyKeep's own drawings,
painted with simple shapes (no font glyphs, no third-party artwork).

### Icon buttons

Undo, Redo, Settings and Close book become small icon buttons (about one text line
high, same size as each other), each with a hover tooltip and an accessible name equal
to its tooltip's first words:

| Button | Icon | Tooltip / accessible name |
|---|---|---|
| Undo | a circular arrow turning counter-clockwise | "Undo <change>" (from `history.list`, as now) or "Nothing to undo"; name "Undo" |
| Redo | a circular arrow turning clockwise | "Redo <change>" or "Nothing to redo"; name "Redo" |
| Calculator (new) | a small calculator (body, display strip, 3×3 keys) | "Calculator"; name "Calculator" |
| Settings | a gear | "Settings"; name "Settings" |
| Close book | a closed book with an arrow pointing out of it to the right | "Close book"; name "Close book" |

- Undo and Redo stay where they are (after the book name) and are disabled (dimmed,
  tooltip still shown) when there is nothing to undo/redo.
- Calculator, Settings and Close book move to the **far right** of the top bar, in that
  order. Settings is no longer a text item in the navigation; the gear is highlighted
  (selected style) while the Settings view is open, and its role is tab with the
  selected state, like the other navigation items.
- Keyboard: all are in the tab order; Enter/Space activate; Ctrl+Z / Ctrl+Shift+Z keep
  working; Ctrl+, opens Settings (new).

### Features menu

- The navigation items **Budget**, **Sales** and **Purchases** are replaced by one
  drop-down **Features** (text, with a small down-chevron).
- Its menu lists, **alphabetically**, the screens of modules that are turned on:

  | Menu item | Shown when module on | Opens |
  |---|---|---|
  | Budget | `budgets` | Budget view |
  | Purchases | `bills` | Purchases view (bills, vendors, …) |
  | Sales | `invoicing` | Sales view (invoices, estimates, …) |
  | Securities | `investments` | Securities view |

  Future module screens join this list in alphabetical order.
- **Features** is hidden when none of these modules is on.
- **Features** shows the selected style while one of its screens is open, and the open
  one is checked in the menu. Accessible: Features is a menu button; items are menu
  items with their checked state.
- Settings → Features (the module switches) is unchanged.

### Calculator

- The Calculator button opens a small floating calculator window titled
  "Calculator", above all other content, about 240 logical pixels wide (scaled with
  text size). Clicking the button again (or Esc while it has focus, or its × button)
  closes it.
- **Move:** drag it by its title bar anywhere inside the app window; it is kept fully
  inside the window (also after a window resize).
- **Pin** (pushpin icon button in its title bar, tooltip "Pin in place" / "Unpin",
  accessible toggle): when pinned, the calculator cannot be dragged, stays open while
  the user moves between screens and stays open after Esc (only × or the Calculator
  button closes it), and it reopens pinned at start-up if it was open. Unpinned it
  can be dragged and closes on Esc; it also stays open across screen changes until
  closed.
- Position, pinned state and (when pinned) open state are remembered per user (app
  config), and Reset display settings resets them.
- **Display:** the current entry or result in large monospaced digits, right-aligned,
  grouped with commas (the user's number style), and above it in secondary text the
  pending operation ("1,250.00 +"). Errors show in the display: "Can't divide by zero".
- **Keys (buttons):** 7 8 9 ÷ / 4 5 6 × / 1 2 3 − / 0 . = + and a top row
  **C** (clear all), **⌫** (delete last digit), **±**, **%**. The labels ÷ × − ⌫ ±
  must render in the app's fonts; if a glyph is missing, draw it as a shape.
  Plus **Copy** (copies the displayed value, ungrouped, e.g. `1250.5`, to the
  clipboard; tooltip "Copy the result").
- **Arithmetic:** exact decimal (no binary floating point). Operations evaluate left
  to right as entered (a simple calculator, no precedence): `2 + 3 × 4 =` → 20.
  Repeated `=` repeats the last operation. `%` turns the entry into a percentage of the
  pending left operand for + and − (`200 + 10 %` → `200 + 20`) and into entry ÷ 100
  otherwise. Division results are rounded to 10 decimal places (half to even) and
  trailing zeros dropped. Values beyond 28 significant digits show "Too large".
- **Keyboard** while the calculator has focus: digits, `.`, `+ - * /`, Enter or `=`,
  Backspace, Esc (clear the entry first; Esc on an empty entry closes when unpinned),
  Delete (clear all), Ctrl+C (copy). Typing in other fields is unaffected when it
  does not have focus.
- The calculator never changes the book (no command, no audit entry).
- Accessible names: each key's name is its meaning ("seven", "divide", "equals",
  "clear", "delete last digit", "change sign", "percent"); the display is a live
  read-only value "Result 1,250.00".

### App icon

- The window and taskbar icon is MoneyKeep's own **small calculator** drawing (same
  design as the toolbar icon, in colour: a dark rounded body, a light display strip,
  accent-coloured keys), generated by the app at start-up (an RGBA image, at least
  256×256) and passed to the window builder.
- On Windows, where practical, the same image is embedded as the executable's file
  icon at build time; if that needs a build tool that is not available, it is left
  out and noted.

### Acceptance tests

149. The top bar shows Undo, Redo, Calculator, Settings and Close book as icon
     buttons with accessible names "Undo", "Redo", "Calculator", "Settings", "Close
     book"; hovering Settings shows "Settings"; hovering Undo after a change shows
     "Undo <change>".
150. There is no "Settings" text navigation item; clicking the gear opens Settings
     and the gear shows selected; Ctrl+, opens Settings.
151. Calculator, Settings and Close book are the three rightmost controls of the top
     bar, in that order.
152. With invoicing, bills and budgets on and investments off, Features lists Budget,
     Purchases, Sales in that order; with all four off, Features is absent; with
     only investments on, it lists Securities.
153. Opening Sales from Features shows Features selected and Sales checked.
154. The calculator: clicking 1 2 . 5 + 7 . 5 = shows 20; `2 + 3 × 4 =` shows 20;
     `10 ÷ 3 =` shows 3.3333333333; `1 ÷ 0 =` shows "Can't divide by zero";
     `200 + 10 % =` shows 220; `=` again shows 240 (repeats + 20).
155. Keyboard: with the calculator focused, typing `12*3` Enter shows 36; Ctrl+C puts
     `36` on the clipboard; typing in a register pop-up field while the calculator is
     open but not focused goes to that field.
156. Unpinned: dragging the title bar moves it; Esc on an empty entry closes it.
     Pinned: dragging does not move it; Esc does not close it; navigating to Reports
     keeps it open; after a config round trip it reopens pinned at the same position.
157. A calculator dragged past the window edge stays fully inside the window.
158. The app's window icon is set (the builder carries an icon of at least 256×256).
159. The audit log is unchanged after using the calculator.

## Revision 26 — book tiles on the start screen, book profile (2026-10-22, owner request)

Sources: egui documentation (layout, `Grid`/wrapping, painter), PNG specification
(W3C) and JPEG (ITU-T T.81) as already used for template logos
(`business-dimensions.md` Part C), WCAG 2.2 SC 1.1.1, 1.4.3, 2.1.1; owner request.
Tile design, symbols and wording are MoneyKeep's own (a tile grid like a phone's
home screen, as the owner described); no target product observed.

### Book profile (stored in the book)

Each book gains a profile:

| Field | Rules | Default |
|---|---|---|
| `name` | the existing book name; 1–80 characters, trimmed | as created |
| `description` | plain text, 0–200 characters, single paragraph (line breaks become spaces); cleaned like other user text (`input-safety.md`) | empty |
| `icon` | one of: `letter` with `text` (1–2 characters, shown upper-cased); `symbol` with a name from the list below; `image` (a stored picture) | `letter` with the first letter (character) of the name; for a name starting with a non-letter, its first character |
| `color` | one of 10 named tile colours (list below) | chosen from the name: the colour at index (sum of the name's character codes) mod 10 |

Symbols (MoneyKeep's own simple drawings, painted as shapes): `house`, `church`,
`briefcase`, `shop`, `building`, `piggy-bank`, `wallet`, `chart`, `heart`, `star`,
`leaf`, `car`, `tools`, `graduation-cap`, `people`, `paw`.

Tile colours (each with white or near-black foreground chosen for at least 4.5:1
contrast, WCAG 1.4.3): `teal`, `blue`, `indigo`, `purple`, `rose`, `red`, `orange`,
`amber`, `green`, `slate`.

**Image icons:** PNG or JPEG up to 2 MB and up to 6,000 pixels on the longer side (so
phone photos work), decoded with the same safe decoders as template logos (memory
bounded: refused from the header before decoding when larger); centre-cropped to a square and scaled to 128 × 128, re-encoded as
PNG and stored in the book. Any other file or a decode failure is refused with "Choose
a PNG or JPEG picture up to 2 MB."

**Commands** (all audited, undoable as compensating commands, available to CLI, JSON
channel and MCP):

- `book.info` also returns `description`, `icon` (`{kind, text|symbol}`; for `image`
  also `image_png_base64`) and `color`.
- `book.update_profile {name?, description?, icon? {kind: letter|symbol, text?,
  symbol?}, color?}` — validates as above; changes only the given fields; renaming
  never renames or moves the file.
- `book.set_icon_image {content_base64}` — sets `icon` to `image`.
- `book.clear_icon_image` — returns `icon` to the default letter.

### Start screen

- Title "MoneyKeep" stays at the top, then a **grid of tiles** that wraps to the
  window width, centred, with even gaps (about 16 logical pixels). Tiles are about
  150 logical pixels wide and 180 high (scaled with text size).
- **One tile per recent book**, most recent first:
  - a rounded rectangle in the book's colour holding the icon large and centred
    (letter in bold about 48 px; symbol or image filling about 60 % of the square);
  - under the coloured square: the book name (strong, up to 2 lines, then "…"), and
    the description in secondary text (up to 2 lines, then "…");
  - hover: the tile lifts slightly (outline or shadow) and a tooltip shows the full
    name, full description and the file path;
  - a small lock badge in the square's corner for a password-protected book
    (accessible text "Password protected");
  - click or Enter opens the book (as the recent-list link did); right-click opens a
    menu: **Open**, **Remove from list**;
  - a book that failed to open shows its tile dimmed with the error below it and a
    **Remove from list** button (as before).
  - Accessible name: "Open <name>", description "<description>".
- **Create tile** after the books: a tile of the same size with a neutral outlined
  square holding a large **+** (drawn), labelled underneath **Create a New Book**.
  It opens a modal **Create a new book** dialog holding today's create form (Book name,
  Base currency, Description (optional, new), Protect with a password with its
  fields and recovery option, then Create book…). The new book's profile takes the
  typed description and the default icon and colour.
- Below the grid: **Open a book…** (secondary button), for books not in the list.
- With no recent books, the grid holds only the Create tile.
- Keyboard: tiles are in the tab order, left to right then down; arrow keys move
  between tiles; Enter opens.

### What the tiles show (cache)

The start screen must not open books to draw tiles (protected books cannot be read
without the password). The app keeps a **tile cache** per recent book in the user's
config: name, description, icon (kind, text/symbol), colour, and for image icons the
128 × 128 PNG saved as a file in the app's config folder (`book-icons/`, one file per
book, named by a hash of the path). The cache is refreshed whenever the book is
opened and whenever its profile changes. A recent book with no cache yet shows the
file name, no description, the default letter and colour. Removing a book from the
list deletes its cache entry and icon file.

### Editing the profile (Settings → Book)

- Settings gains a **Book** section, listed first (before Display), present while a
  book is open.
- Fields: **Name**, **Description** (multi-line box, 200-character limit with a
  live "n / 200" count), **Icon** (choice: Letter / Symbol / Picture):
  - Letter: a 2-character field, prefilled with the current letter;
  - Symbol: a grid of the 16 symbols as toggle buttons (accessible names = the
    symbol's plain name, e.g. "piggy bank");
  - Picture: **Choose picture…** (file dialog, PNG/JPEG) and **Remove picture**;
  **Colour**: 10 swatches as toggle buttons (accessible names = colour names).
- A live **preview tile** shows the result.
- Name and Description save when the field loses focus or Enter is pressed (as other
  settings fields); icon and colour choices save immediately; errors show under the
  field. Each save is one undo step.
- The top bar shows the book's profile name (falling back to the file name).

### Acceptance tests

160. A new book "Household" shows on the start screen as a tile with "H" on its
     default colour, the name under it and no description; the last tile is "Create a
     New Book" with a plus.
161. Clicking "Create a New Book" opens the dialog with Book name, Base currency,
     Description and the password option; creating "Church Office" with description
     "Weekly offerings" adds its tile (after reopening the start screen) with "C" and
     that description.
162. Settings → Book: changing the description and choosing symbol `church` and colour
     `purple` calls `book.update_profile`; the preview updates; after closing the book
     the tile shows the church symbol on purple with the new description; Undo
     restores the previous profile.
163. Choosing a 400 × 300 PNG as picture stores a 128 × 128 image and the tile shows it;
     a 3 MB file or a text file is refused with the message.
164. A password-protected book's tile shows its cached name, description and icon and
     the lock badge without asking for the password.
165. Renaming the book in Settings → Book changes the top bar and tile name but not
     the file name.
166. A tile whose file is missing shows dimmed with the error and Remove from list;
     removing it deletes its cache entry.
167. `book.update_profile` refuses a 201-character description, a 3-character letter,
     an unknown symbol and an unknown colour, each with a clear message.
168. Tiles are reachable by Tab and arrow keys, and Enter opens the focused book; each
     tile's accessible name is "Open <name>".
169. At 200 % text size from 1400×900 the tiles wrap without overlap or clipped names.

## Revision 27 — sort order, scroll bar width and visibility, pop-up buttons (2026-10-22, owner request)

Sources: egui documentation (`style::ScrollStyle`: `bar_width`, `floating`,
`foreground_color`, handle colours), WCAG 2.2 SC 1.4.11 (non-text contrast, 3:1) and
2.5.8 (target size); owner request. No target product observed.

### Sort transactions by date (Settings → Operation)

- New setting **Sort transactions by date**: **Descending** (newest first, default)
  / **Ascending** (oldest first). Per user (app config).
- Applies to every account register (`account.register` `order`) and the investment
  register. Running balances stay correct either way (they are computed in date
  order by the command).
- The register's existing **Newest first** checkbox is removed; the setting replaces
  it. Changing the setting re-renders open registers immediately; the selected row
  stays selected and scrolled into view.
- On opening a register, it is scrolled to the newest transaction: the top when
  Descending, the bottom when Ascending.

### Scroll bar width (Settings → Display)

- New setting **Scroll bar width** in pixels: a slider with a number box, 6–30,
  default **14**. Per user (app config); Reset display settings resets it.
- Applies to every scroll area in the app (registers, lists, reports, settings,
  pop-ups, the accounts pane).
- Hit target: at least the bar width; the handle is at least 24 pixels long.

### Scroll bars always visible

- Scroll bars are **solid**, not floating: shown at all times whenever content
  overflows, never hidden until hovered, and they take their own space (no content
  under them — revision 9's gutter rule holds for all of them).
- Colours, in both light and dark themes: a visible **track** (a tone distinct from
  the panel background) and a **handle** with at least 3:1 contrast against the track
  (WCAG 1.4.11), darker/brighter on hover and while dragging. The handle is never
  white on a white/light background.
- Clicking the track pages by one view; dragging the handle scrolls; mouse wheel
  unchanged.

### Pop-up buttons (revisions 9 and 24)

- The **fast entry** toggle moves from the pop-up's top-right corner to the bottom
  button row, immediately to the **left of Save**, same size as the other buttons'
  height (icon only; tooltip, accessible name, shortcut and states unchanged).
- A **close button (×)**, drawn as a shape, is placed in the pop-up's top-right
  corner (title bar), tooltip "Close", accessible name "Close". It acts exactly like
  Cancel (including the fast-entry "Discard this transaction?" question).
- Applies to "New transaction", "Edit transaction" and the split editor (× = its
  Cancel).

### Acceptance tests

170. Fresh config: Settings → Operation shows Sort transactions by date = Descending;
     a register with transactions on 10-01, 10-05 and 10-03 lists 10-05, 10-03,
     10-01 top to bottom, with correct running balances; the register has no "Newest
     first" checkbox.
171. Choosing Ascending lists 10-01, 10-03, 10-05; the choice survives a config round
     trip.
172. Settings → Display shows Scroll bar width = 14; setting 24 makes the register's
     vertical scroll bar 24 points wide (measured on the bar's rect); a config round
     trip keeps it; values outside 6–30 are clamped.
173. With more rows than fit, the register's scroll bar is drawn without hovering
     (present in the first frame, pointer elsewhere) and its handle colour has at
     least 3:1 contrast with the track in both light and dark themes.
174. The pop-up's bottom row shows the fast entry toggle immediately left of Save; the
     top-right × closes the pop-up like Cancel (with the discard question when fast
     entry has a partly filled form).

## Revision 28 — Expenses screen and phone capture settings (Phase 6a/6b, 2026-10-23)

Specs: `expenses.md`, `mobile-capture.md`.

- **Features menu** (revision 25) gains **Expenses** (module `expenses`), in
  alphabetical order.
- **Expenses screen:** tabs **Expenses** and **Inbox**.
  - Expenses: a list (Date, Merchant, Category, Paid with, Amount, Receipt
    paperclip/"Missing", Status, "From phone" badge) with filters (period, status,
    paid with, text) and buttons **+ New expense**, **Add receipts…**, **Expense
    report…**, **Record reimbursement…** (for selected reimbursable posted expenses).
    Dropping image/PDF files on the screen adds them to the Inbox.
  - The expense form is a pop-up in the revision 9/24 style: receipts strip at the top
    (thumbnails; click to view large in a viewer with zoom and rotate-for-viewing),
    then Date, Merchant, Amount, Category, Paid with (accounts plus "Personal funds
    (reimbursable)"), Purpose, tags; buttons **Save**, **Save and post**, Cancel, ×,
    and the fast entry toggle (revision 24). Posting with a match candidate shows the
    candidate(s) (date, payee, amount) with **Match**, or **Create new**.
  - Inbox: receipt thumbnails with added date and source; each has **New expense**,
    **Attach to transaction…** (search by amount/date/payee), **Delete**.
  - Expense report dialog: period, filter, include receipts, mark as reported, title;
    **Save PDF…**.
- **Register edit pop-up:** a **Receipts** row (thumbnails, Add receipt…, View,
  Remove); drag and drop onto the pop-up; paperclip on register rows with receipts.
- **Settings → Phone capture** as `mobile-capture.md` (server address, token, Test
  connection, Pair a phone… with the large code and countdown, Paired phones with
  Revoke, Collect automatically, **Collect now**).
- **Home Attention:** "N expenses from your phone to review", "N receipts in the
  inbox".

### Acceptance tests

175. With `expenses` on, Features lists Expenses; the screen shows both tabs.
176. Creating an expense with a dropped JPEG, then Save and post with no candidate,
     creates the transaction; the register row shows the paperclip and the pop-up's
     Receipts row shows the thumbnail.
177. Posting with one candidate shows it with Match; Match links it (no new row).
178. Expense report dialog saves a PDF file through the save dialog.
179. Settings → Phone capture: Test connection against the test server succeeds; Pair
     a phone shows an 8-character code; Collect now brings a phone submission into
     Expenses with the From phone badge and a Home Attention entry.

## Revision 29 — phone capture setup dialog and status light (2026-10-26)

Spec: `mobile-capture.md` revision 2, sections B and C. The setup dialog is a modal
with a step list on the left (numbered, with check marks), the current step on the
right, Back / Next, and a collapsible **Details** list; it can be closed and resumed
where it stopped. The status light sits left of the Calculator button. Tests: 180 —
the light shows each state with its accessible name and tooltip; 181 — the setup
dialog walks all steps against the faked Cloudflare API and ends with the QR code and
the Delete Cloudflare token box ticked.

## Revision 30 — clipboard and Cloudflare account discovery (2026-10-27, owner report)

Owner report: pasting into the Cloudflare token field does nothing (the text pastes
fine into other programs), and a correctly typed token with the step 1 permissions
gets "This token can't see any Cloudflare account (HTTP 200)".

1. **Clipboard.** The desktop app is built without the windowing layer's system
   clipboard support, so Ctrl+V, Ctrl+C, Ctrl+X and the context menu's paste/copy
   never reach the operating system clipboard anywhere in the app (also the
   calculator's Copy). The desktop build must enable system clipboard support in
   eframe (its `clipboard` feature, egui-winit with the platform clipboard). Password
   fields accept paste (copy/cut from them stay disabled, as egui does).
   Test 182: with the app's real integration layer (or, where a headless test can't
   reach the OS clipboard, a check on the built feature set), paste into a password
   field and a normal field inserts the text; the calculator's Copy sets the clipboard.
2. **Account discovery.** Cloudflare's account list (`GET /accounts`) can come back
   empty (HTTP 200, no results) for a token that lacks account-settings read access,
   even when the token has every permission step 1 asks for. `capture.setup_check_token`
   must therefore also take the accounts from the zone list (`GET /zones` results
   carry `account.id` and `account.name`), merge both (by id), and only report "can't
   see any account" when both are empty — then saying "This token can't see your
   domain. Check that it includes Zone · Zone · Read and Zone · DNS · Edit for your
   domain, and the Account permissions for your account." The Details list shows how
   many accounts and zones each call returned.
   Test 183: `/accounts` returns an empty list and `/zones` returns one zone with its
   account → the check succeeds with that account and zone; both empty → the new
   message.

## Revision 31 — dashboards (2026-10-28)

Spec: `dashboards.md`. The Home screen becomes the built-in Home dashboard (blocks Net
worth, Account balances, Attention); the left rail gains the Dashboards tab at its top;
the dashboard header has Edit layout / Done, + Add block and its ⋯ menu; blocks draw
with a title bar (drag handle in edit mode), ⋯ menu and a resize handle in edit mode,
and a dashed outline marks the drop cell while dragging. Tests 184–186: the Home
dashboard replaces the old Home screen with the same information; dragging and the
keyboard alternative move a block; the Dashboards tab opens the list.

## Revision 32 — one click, and a minimal password prompt (2026-11-04, owner report)

Owner report on the Slint build: many controls need **two clicks** — opening a book
from its tile, and the icon buttons at the right of the top bar (Settings, Close book,
etc.) — and the password prompt looks old and wordy.

1. **One click everywhere.** Every button, icon button, tile, tab, chip, link and menu
   item acts on the first click (press and release over it), whatever happened just
   before: the pointer has just moved onto it, a tooltip is showing or pending, the
   window has just gained focus, another control holds keyboard focus, or the view
   model has just been rebuilt. A tooltip never takes a click, and rebuilding a model
   must not replace the element under the pointer between press and release. The
   implementer finds the actual cause on the real window (winit backend), fixes it at
   the source rather than per screen, and adds tests that reproduce it.
   Test 187: for a book tile, a top-bar icon button (Settings), a left-rail tab and a
   dialog button — pointer moves onto the control and clicks at once; and pointer rests
   until the tooltip shows, then clicks — each acts after exactly one click.
2. **Password prompt** (replaces the prompt part of revision 17's "Start screen and
   opening"; behaviour and error codes unchanged):
   - A compact card dialog: a lock icon and the book's name as the only heading; one
     password field, centred horizontally and vertically in the dialog, with the
     placeholder "Password" and focus already in it; nothing else in the body.
   - Buttons are icons with tooltips and accessible names: **Open** (primary, unlock
     icon, also Enter), **Use recovery key instead** (key icon, small, beside the
     field or at the dialog's bottom left), and **Cancel** as the × in the top-right
     corner (also Esc). No text labels on these buttons.
   - The key button switches the field to a recovery key field (placeholder "Recovery
     key"); its tooltip and name then read **Use password instead**.
   - Errors show as one short line under the field (`wrong_password`: "Wrong
     password." / "Wrong recovery key."); the field is cleared and keeps focus.
     `book_in_use` and `book_damaged` keep their messages and the **Open previous
     version** button (revision 17).
   - While unlocking, the Open button shows a spinner instead of its icon; there is no
     separate "Unlocking…" text.
   Test 188: the prompt has exactly one text field, centred in the dialog; Open,
   recovery key and Cancel are reachable by their accessible names and have tooltips;
   the key button switches modes; a wrong password shows the one-line error and
   clears the field.

## Revision 33 — AI assistant access (2026-11-04, owner decision)

Settings → Security gains the **AI assistant access** group, the approval dialog, the
top-bar "AI assistant connected" indicator and History's **Changes by AI assistants**
filter. They are specified in `mcp-server.md` revision 3 (sections "App: Settings →
Security → AI assistant access" and "App: approval dialog, indicator, History"),
together with the local control channel the app runs while a book with AI access is
open. Acceptance test: `mcp-server.md` AT28.

## Revision 34 — chart of accounts presets and Tax summary (2026-11-04, owner request)

Specified in `chart-presets.md`, section "Desktop (`app-desktop.md` revision 34)":
the Create a new book dialog gains a **Chart of accounts** list of all presets with a
description panel and Show accounts; Accounts shows codes and tax lines, the account
dialog gains Account number and Tax line, and a new **Add accounts from a chart…**
action; Reports gains **Tax summary**. Acceptance tests CP25–CP28 there; all layout
rules of `ui-slint.md` rev 2 apply (8 px grid, cards, icons, one click).

## Revision 35 — account kind in Edit account (2026-11-05, owner request)

- The Edit account dialog of a money account (ledger-accounts.md revision 3) shows
  **Account type**: Bank, Cash, Other asset, Credit card, Loan, Other liability, with
  the current one selected. Choosing one from the other side shows **Group** (accounts
  of the new type that may hold it, preselected as the command would choose; "Top
  level" allowed) and a note: "Balances and reports will show this account as a
  liability (asset). Its transactions do not change." Save runs `account.update`.
- Other accounts show their type as text, as today.
- Test: in a book with Checking (bank, with transactions), Edit → Account type Credit
  card → Save; the Accounts list shows it under Liabilities and the register shows the
  same transactions with the balance as owed.

## Revision 36 — Mileage screen (2026-11-05, owner request)

Specified in `mileage.md`: the **Mileage** module screen (Trips and Vehicles tabs),
trip and vehicle dialogs, Settings → Mileage (Rates, Saved trips, defaults), Mileage
report…, reimbursable trips in Expenses (Record reimbursement…, expense report) and the
Mileage section of the Tax summary. All layout rules of `ui-slint.md` rev 2 apply.

## Revision 37 — status messages close by themselves (2026-11-06, owner request)

Owner request: stop having to press Dismiss after every action.

- **Settings → Operation** gains **Close status messages automatically** (on by
  default) with **After** seconds: 2, 3, 5, 8, 10, 15 (default 3). Stored per user
  like the other Operation settings (`ui.message_autoclose`, `ui.message_seconds`).
- When on, a status message (the band with Dismiss) closes by itself that many
  seconds after it appears. The time restarts when a new message replaces it. While
  the pointer is over the band, or keyboard focus is in it, or its Details are open,
  it waits and closes the full time after the pointer or focus leaves.
- **Error messages do not close by themselves** (they stay until Dismiss), so a
  failure is never missed. Messages with an **Undo** action close like the others;
  Undo stays available from the toolbar and Edit menu.
- Off: messages stay until Dismiss, as before. Screen readers still get each message
  when it appears (live region), whatever the setting.

Tests:
- With the default, an informational message disappears after 3 s of (mocked) time
  and an error message is still shown after 10 s.
- With 8 s chosen, the message is shown at 7 s and gone at 8 s; hovering at 7 s keeps
  it past 8 s and it closes 8 s after the pointer leaves.
- Off: the message is shown after 60 s.

## Revision 38 — dismissing Attention notices; one way to pair (2026-11-06, owner request)

- **Attention block.** An item the command marks dismissible (connectivity.md revision 5)
  shows a small **×** button at its right end (tooltip and accessible name "Dismiss");
  pressing it runs `connect.dismiss`, removes the line at once and shows "Notice
  dismissed." with **Undo**. Clicking the text still opens the item as today. Lines that
  are not dismissible have no ×. The same × appears on these items in Settings →
  Connections.
- Home's standard blocks keep Attention (already the case); Reset Home layout restores it.
- **Pairing.** Settings → Phone capture → Paired phones no longer shows the old
  **Pair a phone…** button that shows only a typed code. It is replaced by **Pair a
  phone…** opening the QR pairing flow of the active setup (Wi-Fi when Wi-Fi capture is
  set up, else Cloudflare); the typed code remains inside that flow under "Can't scan?".
  The Cloudflare section's "Pair a phone with a QR code…" button is the same flow and
  is removed as a duplicate.

Tests:
- Home with a dismissible connection notice: × removes it, Undo brings it back; a
  "SimpleFIN: 3 new transactions to review" line has no ×.
- Settings → Phone capture with Wi-Fi set up: there is exactly one "Pair a phone…"
  button and it opens the QR flow showing the QR code and the code under "Can't scan?".

## Revision 39 — pairing: one button, a clear choice (2026-11-07, owner decision)

- The Wi-Fi card's own **Pair a phone over Wi-Fi…** button is removed; Paired phones'
  **Pair a phone…** (revision 38) is the only pairing button.
- When both Wi-Fi capture and Cloudflare capture are set up, **Pair a phone…** first asks
  "How will this phone send expenses?" with two choices, each with one line explaining
  it:
  - **Wi-Fi at home (recommended)** — "Sends when the phone is on the same network as
    this computer. Faster, and works without the internet."
  - **From anywhere (Cloudflare)** — "Sends from any network through your Cloudflare
    address. Use this if the phone is often away from home."
  and a note: "A phone pairs with one of them. You can pair it again later to switch."
  The choice opens that setup's QR flow. With only one setup, the question is skipped.
- Test: with both set up, Pair a phone… shows the choice with both explanations; each
  opens its own QR flow; with Wi-Fi only, the QR flow opens directly.

## Revision 40 — book cards and book pictures (2026-11-07, owner request)

Sources: the owner's own keep-launcher (`C:\dev\keep-launcher`, app cards in
`docs/screenshots/keep-launcher-grid.png`), whose card design MoneyKeep's family shares;
the visual style of the template project's launcher icons, the owner's reference
(engraved black-and-white subjects on a flat colour), observed as pictures only; photos from Pexels, Pixabay and Unsplash
(their licences allow use and changes in an app) for the preset pictures, given the
treatment by the spec writer (each file's source page in `ATTRIBUTION.md` and
`docs/specs/data/book-pictures.csv`); the PNG/JPEG decoders already used for book icons
(revision 26); WCAG 2.2 SC 1.4.3, 1.1.1. The engraving effect below is MoneyKeep's own
description; no other program's code was read.

Files supplied by the spec writer: the launcher's card screenshot as a visual reference
(`docs/reference/keep-launcher-cards.png`), the three fonts with their OFL texts
(`crates/mk-views/assets/fonts/`, copied from the owner's keep-launcher), and the 24 preset
pictures (`crates/mk-views/assets/book-pictures/`, 256 × 256 PNGs with transparency, sources
in `ATTRIBUTION.md`).

Owner decisions: the start screen takes the launcher's full look; books gain a tagline;
24 preset pictures of people and work made from free-licence photos; MoneyKeep only.
Replaces the tile design of revision 26 (profile, cache and commands stay, extended below).

### Start screen

- **Top bar** (as the launcher): the MoneyKeep mark and wordmark at the left, a divider,
  the label **MONEYKEEP BOOKS** (mono, uppercase, letter-spaced), and the theme button at
  the right. Square corners, 1 px rules.
- **Toolbar row:** **Open a book…** (outlined), then at the right **SORT** with
  *Last opened* (default) / *Name (A–Z)*, and **Search books** (filters by name, tagline
  and description). The choice of sort is remembered.
- **Cards** in a grid, same size as the launcher's cards (374 × 312 logical pixels at
  100 % text size, scaled with it), sharing 1 px borders, as many columns as fit (at
  least one; the card narrows to the window on a narrow window), centred.
- **Fonts** (bundled, SIL Open Font License 1.1, already the licence of Inter in
  `ATTRIBUTION.md`): Archivo for book names, Inter for body text, Geist Mono for labels,
  chips and buttons. Used on the start screen only; the rest of the app is unchanged.
- Light and dark follow the app's theme; colours as the launcher's (near-black page
  #111214 and card rules #2A2C30 in dark; white page and #D9D9D6 rules in light).

### A book card

From top to bottom:
1. **Top strip** (40 px, rule below): a square **number badge** (`01`, `02`… in list
   order) in the book's colour with its contrasting text; at the right **SHOW IN
   FOLDER ↗** (opens the file's folder).
2. **Picture** 56 × 56, rounded 10 px: the book's colour behind the letter, symbol or
   picture. Beside it the **name** (Archivo semibold 22, one line, "…") with a small
   outlined chip showing the base currency (`USD`), and under it the **tagline**
   (secondary text, one line, "…").
3. A **status chip** (mono, uppercase, outlined) only when there is something to say:
   **PASSWORD PROTECTED** (blue), **COULDN'T OPEN** (red, with the error as the
   description), **FILE NOT FOUND** (red).
4. The **description** (up to 3 lines, "…"), then **Last opened: Nov 7, 2026**
   (secondary).
5. Bottom row: **OPEN** (primary: filled, mono uppercase; full width less the menu)
   and **…** (menu: Open, Show in folder, Remove from list). A failed book shows
   **REMOVE FROM LIST** instead of OPEN.
- Hover lifts the card's rule to the book's colour; clicking anywhere on the card
  except the links opens the book; tooltip shows the full name, tagline, description and
  path. Keyboard and accessible names as revision 26 ("Open <name>").
- **Create card** (last): the same size, the top strip reads **NEW BOOK**, a 56 px
  outlined square with a drawn **+**, the name **Create a New Book**, the tagline
  "Household, business, church or club", and the primary button **CREATE**, which opens
  the existing Create a new book dialog (now with a Tagline field too).

### Profile additions

| Field | Rules | Default |
|---|---|---|
| `tagline` | plain text, 0–60 characters, one line; cleaned like other user text | empty |
| `icon` | adds `preset` with `preset` = one of the 24 ids below, and `image` (now 256 × 256) | as before |

- `book.update_profile` accepts `tagline` and `icon {kind: "preset", preset}`;
  `book.info` returns them; the tile cache stores the tagline and the preset id (preset
  pictures are drawn from the app, so no file is cached for them).
- **Preset ids** (owner list): `farmer`, `pastor`, `teacher`, `nurse`, `doctor`,
  `carpenter`, `builder`, `mechanic`, `shopkeeper`, `baker`, `cook`, `bookkeeper`,
  `lawyer`, `artist`, `musician`, `gardener`, `fisherman`, `seamstress`, `scholar`,
  `student`, `family`, `couple`, `grandparents`, `piggy-bank`. Plain names for screen
  readers ("piggy bank", "grandparents").
- Preset pictures ship as 256 × 256 PNGs with transparency
  (`crates/mk-views/assets/book-pictures/<id>.png`, supplied by the spec writer, already
  given the treatment below): white paper and dark ink for the subject, transparent
  background, so the book's colour shows behind them.

### Settings → Book

- Fields: **Name**, **Tagline** (live "n / 60"), **Description** (as before), then
  **Picture** with four choices: **Letter**, **Symbol**, **People and work** (a grid of
  the 24 preset pictures, each on the book's colour, as toggle buttons) and **Your
  picture**; **Colour** swatches as before. The live preview is now a full **book card**.

### Your picture — the engraving treatment

**Choose picture…** (PNG or JPEG, limits as revision 26) opens the **Picture editor**
dialog:
- **Crop:** the picture in a square frame; drag to move, **Zoom** slider (100–400 %).
- **Preview:** large (256 px) and at card size (56 px), on the book's colour; **Before /
  After** switch.
- **Sliders** (each updates the preview while dragging; under 150 ms at preview size):

| Slider | Range (default) | Effect |
|---|---|---|
| **Detail** | 0–100 (50) | line spacing: 0 = wide (about 9 px at 512), 100 = fine (about 3 px) |
| **Line weight** | 0–100 (50) | how thick lines grow in dark areas |
| **Contrast** | −50–50 (10) | tone contrast before engraving |
| **Brightness** | −50–50 (0) | tone brightness before engraving |
| **Outline** | 0–100 (40) | strength of drawn edges (contours of the face, clothes) |
| **Curve** | 0–100 (30) | how much lines bend with the picture's tones (0 = straight hatching; higher = flowing, banknote-like lines) |
| **Background** | 0–100 (0) | removes the background: 0 keeps it; higher removes more of the area that joins the picture's edges and is close in colour to them; removed area shows the book's colour |

- **Touch-up brushes** (owner decision): **Keep** and **Remove** brush buttons over the
  large preview, with a **Brush size** slider (2–20 % of the picture). Painting with Keep
  marks area that is always part of the subject; Remove marks area that is always
  background; both override the Background slider. **Undo stroke** and **Clear
  brushes**. Strokes are shown as a light tint while a brush is selected and stored with
  the slider values (as a 256 × 256 mask: keep / remove / automatic), so Edit picture…
  brings them back.
- **Tips for a good picture** (owner request): shown under **Choose picture…** in Settings
  → Book and as a collapsible panel in the Picture editor (open the first time, then
  remembered). Text:
  > **Tips for a good picture**
  > - **Who:** one person, or a small group, from the chest or waist up. Something that
  >   shows what the book is about helps: a tool, an instrument, a building.
  > - **Background:** plain, and different from the clothes: a light wall behind dark
  >   clothes, or a darker wall behind light clothes. Busy rooms, trees and patterns are
  >   hard to remove.
  > - **Light:** soft and even, from the front: daylight from a window beside the camera,
  >   or outdoors in shade or on a cloudy day. Avoid light from behind the person (a
  >   window or the sun behind them) and harsh midday sun, which makes deep shadows.
  > - **Framing:** the person fills most of the square, with a little space above the head.
  > - **Quality:** sharp, no filters, at least 800 pixels on the short side.
  > - **Permission:** use photos you took or have the right to use, and ask people before
  >   using a picture of them.
  > If the background isn't fully removed, lower or raise **Background**, then paint with
  > **Keep** or **Remove**.
- **Ink:** Black (#16181B) / Navy (#1B2340) / Sepia (#3B2A1E). **Paper:** White /
  Cream (#F6F1E4).
- **Styles** (set the sliders at once): **Fine**, **Classic** (defaults), **Bold**;
  **Reset**.
- **Use picture** saves; **Cancel** leaves the book unchanged.

**How the treatment works** (the implementer may refine values, keeping the behaviour):
1. Crop to the square and scale to 512 × 512 (working size; 256 for the live preview).
2. Tone = luminance, adjusted by Contrast and Brightness, lightly sharpened.
3. **Hatching:** up to three line layers at 45°, −45° and 0°. Each layer's lines have
   the Detail spacing; a pixel is ink when it lies within half a line width of a line,
   the width growing with darkness (scaled by Line weight). The second layer appears
   only where the tone is darker than about 55 %, the third darker than about 30 %
   (cross-hatching in the shadows). Light areas stay paper. With Curve above 0, each
   line is shifted across its direction by an amount that follows the local tone, so
   lines flow around shapes.
4. **Outline:** strong tone edges (a gradient filter) are added as ink, by Outline.
5. **Background:** a fill from every edge pixel through neighbours whose colour is within
   a tolerance set by the slider of the edge colours; the filled area becomes
   transparent; the border is smoothed by one pixel. Small holes inside the subject are
   not filled.
6. Render ink and paper at 512 with smooth (anti-aliased) edges, scale to 256 × 256, and
   store as PNG with transparency. Pure Rust, exact integer or fixed-point arithmetic
   where `float_arithmetic` is denied (or a module-level allowance with a comment, as
   for image scaling).

**Storage:** the book stores the result (256 × 256 PNG) for the cards and, so the
sliders can be changed later, the cropped source (512 × 512 PNG, colour, no metadata) and
the slider values. **Edit picture…** reopens the editor with them. **Remove picture**
deletes all three. The cache keeps only the result.

New commands: `book.set_picture {source_png_base64, crop {x, y, size}, settings}` (runs
the treatment in the command so every front end gets the same result) and
`book.preview_picture {…}` → `{png_base64}` (not audited, no change).
`book.set_icon_image` stays for importing a finished picture without treatment.

### Acceptance tests

219. Start screen with two books (one protected) shows the top bar label MONEYKEEP
     BOOKS, two cards and the Create card, each 374 × 312 at 100 % text size; the
     protected card shows PASSWORD PROTECTED; badges read 01 and 02.
220. A card shows name, currency chip, tagline, description (cut after 3 lines with
     "…") and "Last opened: …"; OPEN opens the book; … → Remove from list removes it.
221. Search "church" shows only books whose name, tagline or description contains it;
     Sort by Name orders A–Z and is remembered after restart.
222. Settings → Book: tagline over 60 characters is refused with the count shown; the
     preview card updates as fields change; each change is one undo step.
223. People and work shows 24 pictures with accessible names; choosing "baker" shows it
     on the book's colour in the preview and, after reopening the start screen, on the
     card; changing the colour changes the colour behind it.
224. Picture editor on a test photo (a dark figure on a plain light background): with
     Background 0 the result has no transparent pixels; at 60 the corners are
     transparent and the figure is not; Detail 100 gives more ink lines across a fixed
     row than Detail 0; Before/After switches the preview; Cancel changes nothing.
225. Use picture stores result, source and settings; Edit picture… reopens with the same
     slider values; Remove picture deletes them and returns to the letter.
226. `book.set_picture` gives the same PNG bytes for the same input and settings
     (deterministic).
227. Fonts: the start screen uses Archivo, Inter and Geist Mono (bundled); the About
     dialog lists them under Third-party licences.
228. Brushes: on the test photo with Background 60, a Remove stroke across part of the
     figure makes that part transparent; a Keep stroke over a removed corner restores
     it; Undo stroke reverts the last one; after Use picture and Edit picture… the
     strokes are still there.
229. Screenshots, light and dark: `start-cards`, `book-settings-presets`,
     `picture-editor`.

## Revision 41 — Reset MoneyKeep settings (2026-11-09, owner request)

- **Settings → Advanced → Reset MoneyKeep settings…** clears the app's own settings on
  this computer: the recent books list and its card cache (the books themselves stay
  where they are and can be opened again with Open a book…), window size and layout,
  theme, start-screen sort, display settings, the tips-panel state, and the backup
  settings (folder, second place, rotation numbers — existing backup files are kept and
  the default folder applies again).
- **Not touched:** books, backup files, anything stored inside books (book settings,
  pictures, audit), phone capture (it has its own Start over, mobile-capture revision 17).
- A preview lists both groups; the button needs typing `RESET`. Before resetting, the
  current settings file is saved as `settings-before-reset-<date>.json` next to it (the
  dialog shows where), so it can be restored by hand.
- After the reset MoneyKeep returns to the start screen as on first run.
- Tests: 230. Reset clears the recent list and display settings, keeps books and backup
  files, and writes the saved copy; the button needs `RESET`. 231. Screenshots light and
  dark: `reset-settings`.

## Revision 42 — status messages in a corner pop-up (2026-11-11, owner request)

Owner report: when a status message appears or closes, the band under the top bar
pushes the rail, menu and page down and back up. Owner decision: show every status
message in a pop-up in the bottom-right corner that floats over the page, so nothing
moves.

- **Where.** The message band under the top bar is removed. The current status message
  (the same single message as today: text, its tone, and its Show / Review / Undo /
  Details actions) is drawn in a pop-up anchored to the bottom-right of the window,
  `Theme.s3` from the right and bottom edges. Width: the text's natural width, at most
  420 px and never more than the window width less `2 × Theme.s3`; text wraps inside
  it. It is drawn above the page, docked panes and the Edit pop-up, and below menus,
  drop-down lists and modal dialogs' scrims (a message raised by a modal dialog shows
  above that dialog's scrim so it can be read). Laying it out never changes the size or
  position of anything else on screen.
- **Look.** A card with the tone's banner background and border (accent for normal
  messages, warning amber, error red), rounded corners and the theme's popup shadow.
  An icon at the left shows the tone (✓ / ! / ×). It always has a small **×** button
  at its right (tooltip and accessible name "Dismiss"). Light and dark themes as other
  cards.
- **Details.** Pressing Details opens the details inside the pop-up, which grows
  upwards (its bottom edge stays put) to at most half the window height, scrolling
  beyond that.
- **Closing.** Revision 37 is unchanged in substance: with **Close status messages
  automatically** on, a non-error message closes after the chosen seconds, held while
  the pointer is over the pop-up, focus is in it, or its Details are open. Errors stay
  until × is pressed. Esc while focus is in the pop-up closes it. A new message replaces
  the current one in place.
- **Motion.** It fades in over 150 ms and out over 150 ms; with the system's reduce
  motion setting on, it appears and disappears without fading.
- **Keyboard and screen readers.** The live-region announcement is unchanged. F6 (which
  cycles focus between regions) includes the pop-up while one is shown. Opening a
  pop-up never takes focus from what the user is typing in.
- **Covering.** While the pointer is not over it, nothing under the pop-up is needed
  for long: a non-error message closes by itself; the user can close any message with
  × or Esc.

Tests:
- 232. Showing and closing an informational message leaves the top-left position and
  size of the rail, top bar and page unchanged (compare their geometry before, during
  and after).
- 233. The pop-up's right and bottom edges are `Theme.s3` from the window's; with a
  long message its width is 420 px and the text wraps; at the minimum window size it
  fits inside the window.
- 234. Revision 37's tests pass with the pop-up: an info message closes after 3 s,
  hovering holds it, an error stays after 10 s, × and Esc close it.
- 235. Details grow the pop-up upwards with its bottom edge fixed, capped at half the
  window height.
- 236. Screenshots light and dark: `message-info`, `message-error-details`,
  `message-undo` (a register with the pop-up over its bottom-right corner).
