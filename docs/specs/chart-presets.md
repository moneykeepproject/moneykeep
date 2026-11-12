# Spec: EZ Chart of Accounts — starter charts, tax lines and the Tax summary

- **Area / module:** ledger core (`core`): accounts, book creation, reports
- **Spec writer:** Claude session 2026-11-04 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-11-04
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Owner | `docs/specs/data/chart-seed-owner-2026-11-04.toml` — the owner's chart of accounts ("EZ Chart of Accounts"), supplied 2026-11-04, with its validation rules (owner's `coa.rs`, same date) | Presets, packs, accounts, codes, names, notes and tax-line hints. The file states it was built from general double-entry conventions and not from any other product's default list |
| 2 | Textbook | OpenStax, *Principles of Accounting, Volume 1* — https://open.umn.edu/opentextbooks/textbooks/principles-of-accounting-volume-1-financial-accounting | Numbered charts of accounts (1xxx assets … 5xxx–8xxx expenses), contra accounts, headers |
| 3 | Standard | IRS, tax year 2025 forms and instructions: Form 1040 and Schedules 1, 1-A, A, C, D; Forms 1120-S, 1065, 1120 with Schedules K, L, M-1, M-2; Form 1125-A; Forms 2441, 8863, 8889, 8829 — https://www.irs.gov/forms-instructions | Tax-line tags. Checked line by line on 2026-11-04; corrections are listed in "Seed changes" |
| 4 | Spec | `ledger-accounts.md`, `import-pipeline.md`, `business-sales.md`, `business-purchases.md`, `business-dimensions.md`, `expenses.md`, `investments-core.md`, `reports.md`, `app-desktop.md` | Existing account rules and the accounts that modules create on first use |
| 5 | Owner | Decisions 2026-11-04: tax-line field **and** a Tax summary report; the New Book dialog shows **one list** of all presets; existing books can **add accounts from a chart** | |

No target product was observed. Names, wording and layouts are MoneyKeep's own or the
owner's. IRS form and line names are public government terms.

## Purpose

A new book today starts with a single account, Opening Balances, so every user builds
a chart by hand. Most people want a sensible list to start from: household
categories, a small business list, a church's net assets and ministries, or accounts
laid out like the tax return they file so that tax time is a report. This spec ships
the owner's chart as built-in **presets**, lets a book start from one, lets an
existing book add accounts from one, and adds a **tax line** to accounts with a **Tax
summary** report that totals the year by tax line.

## Concepts

- **Seed** — one TOML document compiled into the program
  (`crates/mk-commands/charts/default_charts.toml`) holding presets, packs and
  accounts. It is MoneyKeep data, not a `.mkdef` definition (definitions can only
  describe income/expense categories; presets need every account type).
- **Pack** — a named group of accounts (`core`, `personal`, `business`, `form1040`,
  `schedc`, `f1120s`, `f1065`, `f1120`, `nonprofit`, `investments`, and the module
  packs). Packs that are never combined may reuse codes.
- **Preset** — a named list of packs with a picker blurb, a full description, an
  optional tax year, optional per-preset tax-line overrides, and optional modules to
  switch on.
- **Module pack** — a pack merged in when its module is on.
- **Role** — a fixed internal job an account does for the program (see "Roles").

## Seed format (schema 1)

Top level: `schema = 1`, `[[preset]]` tables, a `[module_packs]` table (module id →
pack id) and `[[account]]` tables.

**Preset fields:** `id` (lowercase letters, digits, `_`), `name`, `blurb` (one line),
`description` (at least 200 characters; shown when the preset is selected), `packs`
(list), optional `tax_year` (integer), optional `tax_overrides` (code → tag; `""`
means "no tag in this preset"), optional `modules` (module ids switched on when a
book is created from the preset).

**Account fields:** `code` (4 digits), `name`, `type` (`asset`, `liability`, `equity`,
`income`, `expense`), `subtype` (a MoneyKeep subtype from `ledger-accounts.md` that
belongs to `type`), `pack`, optional `parent` (code of a header in the same type),
optional `header` (bool → the account is a **placeholder**), optional `role`, optional
`tax_line`, optional `note` (becomes the account's description).

### Validation (a unit test runs it on the shipped seed; the program refuses a bad seed at start-up in debug builds)

1. `schema` is 1.
2. Every preset's description is at least 200 characters after trimming; every pack
   it names exists; every module pack names an existing pack and an existing module
   id from the module catalog.
3. Code ranges: asset 1000–1999, liability 2000–2999, equity 3000–3999, income
   4000–4999, expense 5000–8999; codes are exactly 4 digits.
4. `subtype` belongs to `type`. `cost-of-goods` only for codes 5000–5999.
5. A header has no parent and no tax line. A parent exists in the resolved chart, has
   the same type and is a header (so charts are at most two levels deep).
6. For every preset combined with **all** module packs: codes unique, names unique
   (case-insensitive), roles unique, every tax-override code present.
7. No name contains `:`; names are 1–100 characters; tax lines are 1–40 characters of
   printable text; notes are at most 1000 characters.
8. Every preset includes the `core` pack, which holds the required roles
   `opening_balances`, `uncategorized_income` and `uncategorized_expense`.
9. In `schedule_c`, `s_corp`, `partnership` and `c_corp`, each combined with all
   module packs, every postable income or expense account has a tax line (after
   overrides), except the two uncategorised accounts; in
   `personal_1040_schedule_c` the same holds for the accounts of the `schedc` pack.
   (Household presets deliberately leave everyday spending untagged.)

### Seed changes from the owner's file

The implementer produces the shipped seed from source 1 with exactly these changes,
and nothing else:

1. **`kind` becomes `subtype`:** bank → `bank`; cash → `cash`; credit_card →
   `credit-card`; investment → `brokerage`; receivable → `receivable`; payable →
   `payable`; fixed_asset → `fixed-asset`; other_asset → `other-asset`, except
   Undeposited Payments → `clearing`; loan → `loan`; other_liability →
   `other-liability`, except Sales Tax Payable, Income Taxes Payable and Payroll
   Liabilities → `tax-payable`; income → `income`; cost_of_sales → `cost-of-goods`;
   expense → `expense`; equity → `opening-balances` for 3900, `retained-earnings` for
   every Retained Earnings, `net-assets` for both nonprofit net-asset accounts,
   `owner-equity` for the rest.
2. **`system` and `contra` are dropped.** MoneyKeep displays every account by its
   type's normal side; a contra account (draws, accumulated depreciation, returns)
   shows as a negative amount, which is the intended reading. Roles replace `system`.
3. **Names aligned with the program's existing accounts:** 3900 "Opening Balance
   Equity" → **Opening Balances**; 1250 "Undeposited Payments" → **Payments to
   Deposit**; 2270 "Reimbursements Owed" → **Reimbursements Payable**; 4720
   "Realized Gains & Losses" → **Realized Gains**. Notes and preset descriptions that
   mention the old names are updated to match ("Opening Balance Equity should be
   cleared…" → "Opening Balances should be cleared…").
4. **Removed:** 4730 Unrealized Gains & Losses (MoneyKeep values holdings at market in
   reports and never posts unrealised gains).
5. **Module packs use real module ids:** `invoicing` → pack `sales` (1200 Accounts
   Receivable, 1250 Payments to Deposit); `sales-tax` → new pack `sales_tax` (2250
   Sales Tax Payable, moved out of `sales`); `bills` → `purchasing`; `expenses` →
   `expenses`; `investments` → `investments`.
6. **Roles added:** see the table below.
7. **New preset `personal_1040_schedule_c`** ("Personal — Form 1040 + Schedule C"),
   placed after `schedule_c`: packs `core`, `form1040`, `schedc`, `investments`;
   `tax_year = 2025`; modules `investments`; blurb "Household and sole-proprietor
   books in one, grouped by Form 1040 and Schedule C lines."; a description written
   in the style of the others (best for / what you get / good to know) saying it
   is the 1040 preset plus the Schedule C preset, that business codes never overlap
   household ones, and that the owner pays self-employment tax and personal health
   insurance from Owner's Draws. The Schedule C preset's description changes "(it can
   be combined with …)" to "Choose \"Personal — Form 1040 + Schedule C\" to keep
   household and business in one book."
8. **Preset modules:** `personal`, `sole_proprietor`, `personal_1040`,
   `personal_1040_schedule_c` → `investments`; `nonprofit` → `classes`, `funds`.
   Others none.
9. **Tax-line corrections** from the 2026-11-04 check of source 3, listed in
   "Tax-line corrections" at the end of this spec.

## Roles

A role ties an account to the program's own lookups, so a renamed or moved account
keeps working. Each role is held by at most one account per book.

| Role | Seed account | Used by |
|---|---|---|
| `opening_balances` | 3900 Opening Balances | Opening-balance entries (`ledger-accounts.md`) |
| `uncategorized_income` / `uncategorized_expense` | 4999 / 8999 | Import fallback (`import-pipeline.md`) |
| `receivable` | 1200 Accounts Receivable | `invoicing.receivable_account` default |
| `deposit_clearing` | 1250 Payments to Deposit | `invoicing.deposit_to` default |
| `sales_tax` | 2250 Sales Tax Payable | `sales-tax.liability_account` default |
| `payable` | 2000 Accounts Payable | `bills.payable_account` default |
| `reimbursements` | 2270 Reimbursements Payable | `expenses.reimbursement_account` default |
| `retained_earnings` | every Retained Earnings (3200) | Balance sheet "Retained earnings" line (`business-sales.md`) |
| `inv_dividends` | 4710 Dividends | Investments' auto account `Dividends` |
| `inv_interest` | 4700 Interest Income | Investments' auto account `Interest` |
| `inv_realized` | 4720 Realized Gains | Investments' auto account `Realized Gains` |
| `inv_fees` | 8850 Investment Fees | Investments' auto account `Investment Fees` |

**Lookup rule (changes the existing "created on first use" lookups):** wherever the
program finds or creates one of these accounts and no setting names one, it uses the
account holding the role; if none holds it, it falls back to today's behaviour (find
by name, else create) and gives the account it found or created the role. A role is
stored on the account (`role`, nullable, not editable by the user, not shown except
as "Used by MoneyKeep for …" in the account dialog). Existing books get roles the
first time a lookup runs. The Opening Balances account created by `book.create`
always gets `opening_balances`.

## Account fields added (`ledger-accounts.md`)

| Field | Rules |
|---|---|
| `tax_line` | Optional text, 1–40 printable characters after trimming, no line breaks; allowed on any non-placeholder account; free text (the user may type their own, e.g. "Sch E L3") |
| `role` | See Roles; set only by the program |

`account.create` and `account.update` accept `tax_line` (empty string clears it).
Making an account a placeholder clears nothing but `account.update` refuses
`placeholder = true` while `tax_line` is set ("Remove the tax line first: totals
can't be posted to a heading."). `account.get`/`account.list` return both fields.
Undo covers both. Definitions export and import (`definition-files.md`) carry
`tax_line` on categories as an optional field (new optional key `tax_line` on
`[[category]]`; readers of version 1 ignore it with the usual unknown-key warning).

## Book chart settings

Per book, set when a preset is applied: `chart.preset` (preset id or none),
`chart.tax_year` (integer or none). Shown read-only in Settings → Book as "Started
from: Personal — Form 1040 (tax year 2025)".

## Commands

| Command | Mutates | Notes |
|---|---|---|
| `chart.presets` | no | All presets: `id`, `name`, `blurb`, `description`, `tax_year`, `modules`, `account_count` (preset alone), and `accounts`: the resolved tree (code, name, type, subtype, header, parent, tax line after overrides, note) for previews |
| book creation | yes | Book creation (`Session::create`, `Session::create_encrypted`, the CLI's create command with a new `--preset <id>` option, and the desktop) gains an optional `preset`; called "`book.create`" below. Without it the book is created exactly as today (one account, Opening Balances; `ledger-accounts.md` AT 8 still holds). With it: Opening Balances gets code 3900 and its role; then, in the same creation (not a separate undo step), the preset's modules are switched on and every account of the preset's packs plus the packs of the modules now on is created in code order, headers before children, with code, subtype, placeholder, parent, description (note), role and tax line (after overrides); `chart.preset` and `chart.tax_year` are set |
| `chart.preview_add` | no | {`preset`}. For an open book: the accounts the preset (plus packs of modules currently on) would add — each with action `add`, `skip` (reason: same code / same name under the same parent / role already held) or `add_under` (its parent header is missing and will be added too) — and a summary ("Adds 23 accounts; skips 9 already in this book") |
| `chart.add` | yes | {`preset`, optional `exclude`: list of codes}. Re-runs the preview and creates the `add` accounts (and needed headers) in one audited, undoable command. A skipped header's children are placed under the existing account with that code if it is a placeholder of the same type, else at top level. Never renames, moves, retypes or retags an existing account. Sets `chart.preset` / `chart.tax_year` only if the book has none |
| `module.enable` | yes | Unchanged, plus: in a book with `chart.preset` set, also adds the module's pack accounts (skip rules as `chart.preview_add`) in the same undoable command. Books without a preset keep first-use creation |
| `report.tax_summary` | no | See below |

## `report.tax_summary`

What goes where on the return. Arguments: dates (default `preset last_year`; the
common report rules in `reports.md` apply), `include_untagged` (default true).

- **Activity section.** For every account whose tax line does **not** start with
  `Sch L`: the display-signed sum of its splits in the period. Rows grouped by tax
  line; each group lists its accounts (full name, code, amount) and a group total.
- **Balance section.** For every account whose tax line starts with `Sch L`: its
  display balance at the day before `from` ("Beginning of year") and at `to` ("End of
  year"), grouped the same way, with group totals for both columns.
- **Not tagged.** When `include_untagged`: every postable income and expense account
  with activity in the period and no tax line, with amounts and a total, so nothing
  is missed.
- **Order.** Groups sort first by the longest of these prefixes the tag starts with,
  in this order: `1040`, `Sch 1-A`, `Sch 1`, `Sch A`, `Sch C`, `Sch D`, `1120-S`,
  `1065`, `1120`, `1125-A`, `Sch K`, `Sch L`, `Sch M-1`, `Sch M-2`, `Form`; tags
  matching none come last. Within a prefix, tags sort in natural order (digit runs
  compared as numbers, so `L2b` < `L10` and `L16a` < `L16b`), case-insensitive.
- Header fields: dates, `chart.tax_year`, and the note "Tax lines are hints to help
  you or your preparer, not tax advice. Line numbers can change from year to year."
- Currency and exactness rules as other reports. Export (`report.export`) supports
  CSV and PDF for this report.
- Contra accounts appear with negative amounts and keep their own row; a group total
  is the plain sum.

## Desktop (`app-desktop.md` revision 34)

### Create a new book dialog

Adds **Chart of accounts**, after Description and before the password option:

- One list of every preset, in seed order, each row showing the preset name (strong)
  and its blurb (secondary). Default selection: **Personal & Household**.
- Beside the list (below it in narrow windows) a panel with the selected preset's
  full description (paragraphs and bullets as written), its account count, its tax
  year when present ("Tax lines for 2025"), and the modules it switches on ("Also
  turns on: Investments").
- **Show accounts** (toggle link) in the panel expands a read-only tree of the
  preset's accounts: code, name, and the tax line in secondary text.
- Create book creates the book with the selected preset.

### Accounts

- The accounts tree shows each account's code before its name (secondary text) when
  it has one, and the tax line as secondary text at the end of the row.
- The account dialog (new and edit) gains **Account number** (the existing `code`
  field, if not already shown) and **Tax line** (single-line text, placeholder e.g.
  "Sch C L18", with a tooltip "Where this account's total goes on your return. A
  hint for reports, not tax advice."). An account with a role shows a line of
  secondary text: "Used by MoneyKeep for <purpose>" (e.g. "for imported money
  that has no category"), and its Delete action is disabled with that reason as the
  tooltip if its role is `opening_balances`.
- A new action **Add accounts from a chart…** opens a dialog with the preset list and
  description panel as above; choosing one shows the `chart.preview_add` result as a
  list with a check box per `add` row (all ticked), skipped rows in secondary text
  with their reason, and the summary line. **Add accounts** runs `chart.add` with the
  unticked codes as `exclude`. One undo step.

### Reports

- **Tax summary** is added to the reports list (core, always present). It shows the
  three sections as tables with group headings (tag) and account rows, the tax-year
  line and the hint note, and the usual date controls (default Last year) and export.

## Ledger effect

Non-posting. Creating a book from a preset creates accounts only; no transaction.
Adding accounts from a chart and module packs create accounts only.

## Settings

| Key | Type | Default |
|---|---|---|
| `chart.preset` | text (preset id) | none |
| `chart.tax_year` | int | none |

(Book-level, set by the program; not module settings.)

## Edge cases

- `book.create` with an unknown preset is refused before any file is written
  ("Unknown chart preset 'x'").
- A preset whose module is unknown to this build: the seed validation prevents it.
- `chart.add` in a book that already has a top-level `Rent` and a preset that adds
  `Housing:Rent` (header 6000 Housing, child 6010 Rent): Rent is skipped as "same
  name" only if the clash is under the **same parent**; here the parent differs, so
  `Housing` and `Housing:Rent` are added.
- An account with code 6010 already exists but with a different name: skipped (same
  code); never renamed.
- A role already held by another account: the seed account with that role is skipped
  ("MoneyKeep already uses <account> for this").
- Turning a module off and on again adds nothing new (all its accounts exist).
- A user deletes Reimbursements Payable (no splits) in a preset book; the next
  expense that needs it recreates it by the lookup rule and gives it the role.
- Tax summary for a book with no tax lines: only the Not tagged section, plus the
  note "This book's accounts have no tax lines. Add them in Accounts, or add
  accounts from a tax chart."
- Tax summary when `from` is the book's first day: beginning balances are zero.

## Acceptance tests

Seed and presets:

CP1. The shipped seed passes validation; every preset resolves with and without all
     module packs.
CP2. Every preset contains Opening Balances (3900, role `opening_balances`),
     Uncategorized Income (4999) and Uncategorized Expense (8999).
CP3. `small_business` alone has no 1200; with modules `invoicing` and `bills` it has
     1200 Accounts Receivable and 2000 Accounts Payable; `sales-tax` adds 2250 Sales
     Tax Payable; module additions for a module already applied are empty.
CP4. Subtypes: 1650 Accumulated Depreciation is `fixed-asset`; 1250 is `clearing`;
     2250 is `tax-payable`; 3200 Retained Earnings is `retained-earnings`; nonprofit
     3000 and 3100 are `net-assets`; schedc 5010 is `cost-of-goods`.
CP5. `personal_1040` has tax year 2025; 4010 → "1040 L1a", 4700 → "1040 L2b",
     6320 → "Sch A L5b", 6510 → "Sch A L11" (after the corrections, if any change
     these, the corrected values).
CP6. `schedule_c`: 4500 → "Sch C L1", 7031 → "Sch C L27b", 7040 → "Sch C L30".
     `personal_1040_schedule_c` resolves with no code, name or role clash and holds
     both 4010 W-2 Wages and 4500 Gross Receipts.
CP7. `s_corp` with `invoicing`: 7007 → "1120-S L7", 7021 → "1120-S L20", 3300 →
     "Sch K L16d", 2650 → "Sch L L19", overrides 4700 → "Sch K L4" and 1200 → "Sch L
     L2a"; in `personal_1040` 4700 is still "1040 L2b".
CP8. `partnership` with `bills`: 7010 → "1065 L10", 7021 → "1065 L21", 3010 → "Sch L
     L21", 3110 → "Sch K L19a", 2650 → "Sch L L19a", 4700 → "Sch K L5", 2000 → "Sch L
     L15".
CP9. `c_corp` with `invoicing` and `bills`: 7012 → "1120 L12", 7019 → "1120 L19",
     7030 → "1120 L26", 8000 → "Sch M-1 L2", 3200 → "Sch L L25", 3300 → "Sch M-2
     L5a", 4700 → "1120 L5".
CP10. Validation rule 9 holds for the shipped seed; and with module `investments`
      on, `s_corp` tags 4710 "Sch K L5a", 4720 "Sch K L7/8a", 8850 "1120-S L20";
      `c_corp` tags 4710 "1120 L4"; `personal_1040` tags 4720
      "Sch D → 1040 L7a".
CP11. Mutated seeds are refused with the matching message: a header with a tax line;
      a child whose parent is not a header; an asset with code 2100; a duplicate code
      within a preset; a 150-character description; a tax override for a missing
      code; subtype `credit-card` on an expense.

Books:

CP12. `book.create` without a preset still gives exactly one account (Opening
      Balances), no code, no role change visible to existing tests.
CP13. `book.create` with `personal`: accounts equal the preset's resolved list plus
      the investments pack; `Housing` is a placeholder with child `Housing:Rent`
      (code 6010); module `investments` is on; `chart.preset` = `personal`.
CP14. `book.create` with `nonprofit` turns on `classes` and `funds`; 3000 is "Net
      Assets Without Donor Restrictions".
CP15. In a `small_business` book, `module.enable invoicing` adds 1200 and 1250 in the
      same undo step; undo removes them and turns the module off; an invoice then
      posts to 1200 (by role) even after it is renamed "Customer Balances".
CP16. In a book created without a preset, enabling `invoicing` adds no account; the
      first invoice creates Accounts Receivable as today and gives it role
      `receivable`.
CP17. Import into a `personal` book: a row with no rule posts to 8999 Uncategorized
      Expense (by role) even after it is renamed "To sort".
CP18. `chart.preview_add personal_1040` on a `personal` book: 1500 Home is `skip`
      (same code), 4010 W-2 Wages is `skip` (same code, different name), 1430 Health
      Savings Account is `add`; `chart.add` with `exclude = ["1430"]` adds the rest in
      one undo step and leaves every existing account unchanged.
CP19. `tax_line` of 41 characters is refused; a tax line on a placeholder is refused;
      setting `placeholder = true` on an account with a tax line is refused with the
      message.

Tax summary:

CP20. In a `personal_1040` book: W-2 Wages 3,000.00 and 2,000.00 in 2025, Real
      Estate Tax 1,200.00, Groceries 400.00, Uncategorized Expense 50.00.
      `report.tax_summary` for 2025: group "1040 L1a" 5,000.00; "Sch A L5b"
      1,200.00; Not tagged lists Groceries 400.00 and Uncategorized Expense 50.00,
      total 450.00; groups ordered 1040 before Sch A.
CP21. In an `s_corp` book with Checking (Sch L L1) opened at 10,000.00 on 2024-06-01
      and 2,500.00 more deposited in 2025: the balance section shows "Sch L L1"
      beginning 10,000.00, end 12,500.00; Shareholder Distributions of 4,000.00 in
      2025 show under "Sch K L16d" as −4,000.00 in the activity section.
CP22. Natural ordering: "Sch C L8" before "Sch C L16a" before "Sch C L16b" before
      "Sch C L27b".
CP23. A voided transaction never counts; an account in EUR is listed in `excluded`.
CP24. Export to CSV has columns section, tax line, account, code, amount (activity) or
      beginning/end (balance) and matches the report totals exactly.

Desktop:

CP25. The Create dialog lists all presets with Personal & Household selected and its
      description shown; selecting Small Business updates the panel; Show accounts
      lists 4500 Sales; creating the book gives the Small Business chart.
CP26. Accounts → Add accounts from a chart… → Personal — Form 1040 on a Personal book
      shows the preview with skip reasons; unticking one row and pressing Add accounts
      adds the rest; Undo removes them.
CP27. The account dialog shows Tax line; saving "Sch E L3" on an expense shows it in
      the tree row; the Opening Balances dialog shows "Used by MoneyKeep for opening
      balances" and Delete is disabled.
CP28. Reports → Tax summary shows the sections of CP20 with the hint note and Last
      year selected by default.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Tax-line corrections

From the 2026-11-04 line-by-line check of every tag, override and tax fact in the
owner's file against the final 2025 IRS forms and instructions (233 tags, 30
overrides, about 16 note facts; everything not listed here was confirmed correct,
including all three Schedule L layouts and the 1040 L13b, 70¢/mile, Form 1125-E
$500,000, >2 % shareholder health insurance, Form 8283 and $250 acknowledgment
facts). All tags stay within 40 characters.

| # | Code / where | Change | Why (2025 form) |
|---|---|---|---|
| T1 | 4720 Realized Gains (investments) | `Sch D → 1040 L7` → `Sch D → 1040 L7a` | Form 1040 line 7 is now 7a (amount) and 7b (check boxes) |
| T2 | `s_corp` overrides | add `4710` = `Sch K L5a`, `4720` = `Sch K L7/8a`, `8850` = `1120-S L20` | Investments can be on in a corporate book; without overrides the 1040 tags would show |
| T3 | `partnership` overrides | add `4710` = `Sch K L6a`, `4720` = `Sch K L8/9a`, `8850` = `1065 L21` | Same |
| T4 | `c_corp` overrides | add `4710` = `1120 L4`, `4720` = `1120 L8`, `8850` = `1120 L26` | Same |
| T5 | 4530 (f1120s) | `1120-S L4` → `1120-S L4 / Sch K L9`; note: "Form 4797. Ordinary gain (Part II) on L4; net section 1231 gain on Sch K L9." | Line 4 takes only Form 4797 Part II |
| T6 | 4530 (f1065) | `1065 L6` → `1065 L6 / Sch K L10`; note likewise with L6 / Sch K L10 | Line 6 takes only Form 4797 Part II |
| T7 | 4640 (f1120) | tag unchanged (`1120 L9`); note: "Form 4797. Ordinary gain (Part II) goes here; net section 1231 gain goes through Schedule D to L8 (Capital Gains)." | Line 9 takes only Part II |
| T8 | 7040 (f1120s) | `Sch K L12a` → `Sch K L12a/L12b`; note adds "Cash gifts on L12a, noncash on L12b." | L12a is cash only |
| T9 | 7040 (f1065) | `Sch K L13a` → `Sch K L13a/L13b`; note likewise | L13a is cash only |
| T10 | 4110 note | "Taxable only if you itemized last year." → "Taxable only to the extent last year's deduction lowered your tax (usually only if you itemized)." | Tax benefit rule |

Known for the next tax year (not changed now): the 2026 Schedule C splits line 16
into 16a (mortgage), 16b (vehicle loan interest) and 16c (other); 7017 Other
Business Interest becomes `Sch C L16c` when the presets move to tax year 2026.
