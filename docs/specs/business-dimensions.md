# Spec: Business, part 3 — classes, locations, funds, statements and late fees, invoice templates, IIF (Phase 4c)

- **Area / module:** `classes`, `locations`, `funds`, `statements`, `invoicing` (templates), core (IIF)
- **Spec writer:** Claude session 2026-10-21 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-21
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Textbook | OpenStax, *Principles of Accounting, Volume 1* (segment reporting, receivables follow-up) | |
| 2 | Standard | FASB ASU 2016-14, *Presentation of Financial Statements of Not-for-Profit Entities* — summaries: Deloitte Heads Up 2016 issue 24 (https://dart.deloitte.com/USDART/home/publications/archive/deloitte-publications/heads-up/2016/fasb-overhauls-guidance-presentation-financial-statements), CPA Journal 2018-04-25 | Two net asset classes (with and without donor restrictions); statement of activities with releases from restriction; expenses by function (program, management and general, fundraising) and by nature |
| 3 | Vendor doc | Intuit support article "IIF Overview: import kit, sample files, and headers" — https://quickbooks.intuit.com/learn-support/en-us/help-articles/iif-overview-import-kit-sample-files-and-headers/00/262115 | IIF is tab-separated ASCII text; header rows start with `!`; transactions use `!TRNS` / `!SPL` / `!ENDTRNS` headers and `TRNS` / `SPL` / `ENDTRNS` rows; example columns TRNSTYPE, DATE, ACCNT, NAME, CLASS, AMOUNT; list headers such as `!CUST`; exports begin with `!HDR`; import error checking in the other program is limited |
| 4 | Standard | ISO 32000-1 (PDF) image XObjects (DCTDecode JPEG, FlateDecode raster) | Logo on invoice PDFs |
| 5 | Spec | `business-sales.md`, `business-purchases.md`, `ledger-*.md`, `reports.md`, `input-safety.md`, `definition-files.md` | |
| 6 | Owner | Plan request: classes, locations and funds; owner supports a nonprofit church (2026-10-21 context) | Fund reports prioritised |

No target product was observed. Names, layouts and wording are MoneyKeep's own. IIF
is named only to identify the file format other software reads; MoneyKeep is not
affiliated with any other software vendor.

## Part A — Dimensions: classes, locations, funds

### Records

| Dimension | Module | Fields |
|---|---|---|
| Class | `classes` | `name` (unique per dimension, `:` for sub-classes, e.g. `Ministries:Youth`), `active`, optional `function` (`program`, `management_general`, `fundraising`) used by nonprofit reports |
| Location | `locations` | `name` (with `:` hierarchy), `active` |
| Fund | `funds` | `name`, `restriction` (`without_donor_restrictions` or `with_donor_restrictions`), `purpose` text ≤ 1,000, `active`. A fund named **General** (without restrictions) exists once the module is on and is the default |

Commands per dimension: `class.create|update|list|set_active|delete` (delete only if
never used; otherwise deactivate), same for `location.*` and `fund.*`.

### Tagging

- Every split may carry `class`, `location` and `fund` (each optional; only for
  modules that are on). Income, expense and (for funds) all splits can be tagged.
- Documents (invoices, bills, credit memos, vendor credits, estimates, POs, deposits,
  payments) carry a document-level default and per-line overrides; posting copies
  them to the splits. Bank/AR/AP splits take the document-level value.
- Rules (`import-rules.md`) gain actions `set_class`, `set_location`, `set_fund`.
  Schedules and definitions carry them too (definition kinds may name dimension
  values by name; missing ones → the rule is installed disabled, as for categories).
- Settings: `classes.required` (bool, default false): when true, saving a transaction
  with an income or expense split without a class is rejected with the split named;
  same for `locations.required`. For funds: splits without a fund belong to General.

### Funds and net assets

- **Expenses and restrictions (source 2):** expenses are always reported as
  decreases of net assets **without** donor restrictions. Spending that satisfies a
  restriction is shown by **releasing** that amount from the restricted fund. An
  expense split tagged to a fund with donor restrictions therefore means "paid for
  this purpose": with setting `funds.auto_release` (default true) saving it creates a
  linked release of the same amount from that fund to General (updated and undone
  with the expense); with it off, the fund reports list such expenses as "spent but
  not released".
- **Net assets by fund:**
  - fund without restrictions = its equity splits + income tagged to it + releases in
    − expenses tagged to it (expenses tagged to restricted funds count against
    General, the fund that receives their release);
  - fund with restrictions = its equity splits + income tagged to it − releases out
    ± interfund transfers (its tagged expenses do not reduce it directly; their
    releases do).
  The two net-asset classes are the sums over funds of each restriction type.
- **Release from restriction:** `fund.release` {from_fund (with restrictions),
  to_fund (default General), amount, date, memo}: one transaction with two splits on
  the account **Net Assets Released from Restrictions** (income type, created on
  first use, subtype `income`): a debit tagged to the restricted fund and an equal
  credit tagged to the receiving fund. Overall it nets to zero; by fund it moves net
  assets. Reports show it as the "released from restrictions" line.
- **Interfund transfer** (not a release): `fund.transfer` {from, to, amount, date} on
  the equity account **Interfund Transfers** (equity), same two-split shape.
- **Overspent restricted fund:** a fund with donor restrictions whose net assets go
  below zero on any report date is flagged in the fund reports and on Home Attention
  ("Youth Mission fund is overspent by 120.00").
- Setting `funds.balance_by_fund` is reserved (self-balancing fund ledgers) and only
  accepts `false` in 4c.

### Reports

All accept dates and the usual filters, plus `dimension` and `values` where relevant,
and export to CSV.

- `report.profit_loss` {dates, `columns`: `total` (default) | `class` | `location` |
  `fund` | `month` | `quarter` | `year`, `depth`}: income, cost of goods (expense
  accounts with subtype `cost_of_goods`, new subtype), expenses, net income; one
  column per dimension value (plus "Not set") when columns is a dimension; totals
  equal the ungrouped report exactly.
- `report.balance_sheet` gains `columns: fund` (assets and liabilities by fund where
  tagged, "Not set" otherwise; net assets by fund computed as above).
- **Nonprofit set** (module `funds` on):
  - `report.statement_of_activities` {dates}: columns **Without donor restrictions**,
    **With donor restrictions**, **Total**; rows: revenue and support by account,
    "Net assets released from restrictions" (negative in the restricted column,
    positive in the unrestricted one, zero in total), expenses by account (all in
    the unrestricted column), change in net assets, net assets at beginning, net
    assets at end.
  - `report.statement_of_financial_position` {as_of}: the balance sheet with net
    assets split into the two classes and listed by fund.
  - `report.functional_expenses` {dates}: a matrix of expense accounts (rows, "by
    nature") × functions (Program, Management and general, Fundraising, Not
    assigned) using each split's class's `function`; totals by row and column.
  - `report.fund_activity` {dates, funds}: per fund: beginning net assets, revenue,
    expenses tagged (for information), releases in/out, transfers in/out, ending net
    assets; for restricted funds also "spent but not released" (auto release off);
    overspent flag.

## Part B — Customer statements and late fees (module `statements`, requires `invoicing`)

- `statement.render` {customer, `from`, `to`, `kind`: `activity` (all activity in the
  period with a running balance) or `open_items` (open invoices as of `to`)} → data
  and a PDF (same safety rules as invoice PDFs): opening balance, each invoice,
  payment, credit memo, refund and late fee in date order, running balance, closing
  balance, and an aging line (Current / 1–30 / 31–60 / 61–90 / Over 90) as of `to`.
  The closing balance equals the customer's balance in `report.customer_balances`.
- `statement.batch` {to, customers: all with a balance (default) or a list} → one PDF
  per customer (returned as a list) or one combined PDF (option).
- **Late fees:** settings `statements.late_fee_kind` (`none` default, `percent`,
  `flat`), `statements.late_fee_percent` (monthly %, e.g. 1.5), `statements.late_fee_flat`,
  `statements.late_fee_min` (minimum fee), `statements.grace_days` (default 0),
  `statements.late_fee_account` (income; default **Late Fee Income**).
  `late_fee.preview` {as_of} lists, per overdue invoice (past due + grace days and
  not charged since its last fee or within the last 30 days), the proposed fee:
  percent kind = round(balance × percent ÷ 100, 2), at least the minimum; flat kind =
  the flat amount. `late_fee.assess` {as_of, invoices} creates one late-fee invoice
  per customer (item/line "Late fee" with the invoice numbers in the description), in
  one command (undoable). Fees are never charged on late-fee invoices themselves.

## Part C — Invoice templates

- A **template** is a named set of presentation options used by invoice, estimate,
  credit memo, statement and purchase order PDFs: `accent_color` (#RRGGBB, used for
  the title and rules), `font` (`helvetica` or `times` — standard PDF fonts),
  `show` flags (terms, due date, item SKU, quantity/rate columns, customer email,
  payment instructions), `payment_instructions` (≤ 1,000), `footer` (≤ 500),
  `title_overrides` (e.g. "Tax invoice"), and an optional **logo**.
- **Logo safety:** PNG or JPEG only, ≤ 512 KB, ≤ 2,000 × 2,000 px. On upload the image
  is fully **decoded** with a pure-Rust decoder (with those limits enforced before and
  during decoding) and **re-encoded** as an 8-bit RGB raster stored in the book (no
  metadata, no ICC profiles, no comments carried over). Animated PNGs, progressive
  oddities beyond the decoder's support, or any decode error → rejected. The PDF
  embeds the re-encoded raster (FlateDecode). The original file is never stored.
- Commands: `template.create|update|delete|list|get`, `template.set_logo`
  {template, content_base64}, `template.clear_logo`; setting
  `invoicing.default_template`; documents may name a template.
- Definitions (`definition-files.md`): new kind `[[document_template]]` with every
  option **except the logo** (logos are not shared through definition files in this
  version).

## Part D — IIF import and export (core)

IIF support is deliberately limited to what source 3 documents, with everything
else handled through review.

### Import

- `import.stage` with `format: "iif"` (`.iif` files): tab-separated, ASCII or
  Windows-1252 text (decoded per `input-safety.md`), CRLF or LF.
- Header rows (`!`-prefixed first column) define the column names for the following
  rows of that keyword. Rows are read for keywords `TRNS`, `SPL`, `ENDTRNS`, `ACCNT`,
  `CUST`, `VEND`, `CLASS`; other keywords are skipped with one note each.
- **Transactions:** a `TRNS` row and its following `SPL` rows up to `ENDTRNS` form one
  transaction. Columns used when present: `TRNSTYPE` (kept as a note and mapped to
  MoneyKeep wording, e.g. CHECK → "Check", DEPOSIT → "Deposit", GENERAL JOURNAL →
  "Journal"; others shown as given), `DATE` (`M/D/YYYY`, `MM/DD/YYYY` or `M/D/YY` per
  a date-order choice offered in review), `ACCNT`, `NAME`, `CLASS`, `AMOUNT`, and —
  if present — `DOCNUM` (→ number), `MEMO` (→ memo). Unknown columns are kept in
  `raw`. Amounts are signed; the `TRNS` and `SPL` amounts of one transaction must sum
  to zero, else the transaction is flagged needs review with the difference.
- **Accounts:** each distinct `ACCNT` name is shown in a mapping table in review (map
  to an existing account, or create a new account with a chosen type); IIF
  sub-account names using `:` map to MoneyKeep full names when they match. Nothing
  is created without the user's choice. `CLASS` maps to classes the same way (module
  `classes` on), `NAME` to parties (optional) or plain payee text.
- **Lists:** `ACCNT`, `CUST`, `VEND`, `CLASS` rows are offered as items to create
  (names only, plus address lines for CUST/VEND when columns named like
  `BADDR1`–`BADDR5` exist), each with a checkbox in review.
- Transactions go through the normal pipeline (fingerprints → duplicates, matching,
  review, one-command commit with undo). Each committed transaction becomes an
  ordinary MoneyKeep transaction (not an invoice/bill document).

### Export

- `export.iif` {dates, accounts (optional filter)} → `{file_name, content}`: a file
  starting with a `!HDR` header row and an `HDR` row (fields PROD = `MoneyKeep`,
  VER, REL, IIFVER = `1`, DATE, TIME), then `!TRNS`, `!SPL`, `!ENDTRNS` header rows
  with columns `TRNSTYPE`, `DATE` (`MM/DD/YYYY`), `ACCNT` (full name with `:`),
  `NAME`, `CLASS`, `AMOUNT`, then each transaction as one `TRNS` row (its first
  split), `SPL` rows (the rest), and `ENDTRNS`. `TRNSTYPE` is `GENERAL JOURNAL` for
  all exported transactions. Tabs, CR and LF inside values are replaced with spaces;
  non-ASCII characters are written as `?` with a warning count (the format is ASCII).
- **Not exported:** memo, number and document details (not yet verified against the
  format's reference; to be added when an owner-supplied sample confirms the columns),
  investment metadata, attachments.
- IIF has no comment syntax, so the app shows these limitations before saving.

## Edge cases

- Turning a dimension module off hides tags; values stay on splits and reappear when
  it is turned back on.
- Deleting a class used on splits: rejected; deactivate instead.
- A release larger than the restricted fund's net assets: allowed with a warning (the
  fund becomes overspent).
- An IIF transaction with one row only (TRNS without SPL): needs review ("unbalanced").
- Late fee preview on a customer marked tax exempt: fee lines are not taxable anyway.

## Acceptance tests

Nonprofit book (modules classes, funds on): funds General (without restrictions) and
"Youth Mission" (with donor restrictions); classes "Worship" (program), "Youth"
(program), "Admin" (management_general), "Fundraising" (fundraising).

1. Tithes 10,000.00 into Checking tagged General; a restricted gift 2,000.00 tagged
   Youth Mission; expenses: Salaries 4,000.00 (class Admin, General), Youth trip
   1,500.00 (class Youth, fund Youth Mission — auto release 1,500.00 created), Event
   costs 300.00 (class Fundraising, General). Statement of activities, 2026:
   without restrictions — revenue 10,000.00, released 1,500.00, expenses 5,800.00,
   change 5,700.00; with restrictions — revenue 2,000.00, released −1,500.00, change
   500.00; total — revenue 12,000.00, released 0.00, expenses 5,800.00, change
   6,200.00. Undoing the trip expense removes its release.
2. Functional expenses: Program 1,500.00 (Youth), Management and general 4,000.00,
   Fundraising 300.00; total 5,800.00.
3. Fund activity: Youth Mission begins 0.00, revenue 2,000.00, expenses tagged
   1,500.00, released out 1,500.00, ends 500.00. A second trip expense of 800.00 tagged
   Youth Mission releases 800.00 → ends −300.00 → **overspent by 300.00** flag and a
   Home attention line. With `funds.auto_release` off, the 800.00 instead appears as
   "spent but not released" and the fund ends 500.00.
4. P&L by class shows Worship 0.00, Youth −1,500.00, Admin −4,000.00, Fundraising
   −300.00, Not set +12,000.00 (income untagged by class); the Total column equals the
   ungrouped net income 6,200.00.
5. `classes.required` true: saving an expense without a class is rejected naming the
   split; income splits too.
6. A rule `payee contains "PAYROLL" → set_class Admin, set_fund General` tags staged
   rows accordingly.
7. Statement for Acme LLC (business-sales AT book after tests 2 and 3), activity
   2026-09-01..2026-10-31: opening 0.00, invoice 1,314.32, payment −1,000.00, closing
   314.32, aging as of 2026-10-31: 1–30 → 314.32; PDF has no active content.
8. Late fees: percent 1.5, minimum 2.00, grace 5 days, as of 2026-10-10 on INV-1001
   (due 2026-10-01, balance 314.32): proposed fee 4.71 (314.32 × 1.5 % = 4.7148 →
   4.71); as of 2026-10-05 nothing is due (within grace). Assessing creates an
   invoice "Late fee — INV-1001" for 4.71; running again the same month proposes
   nothing.
9. Template with accent #1F6F78, footer text and a 300 × 100 PNG logo: the invoice PDF
   contains one image XObject of 300 × 100, the footer text, and still no active
   content; a 3,000 × 3,000 PNG, a 600 KB file, a GIF, and a PNG with a corrupt IDAT
   are each rejected.
10. IIF import of a file with `!TRNS`/`!SPL`/`!ENDTRNS` headers and one CHECK (TRNS
    Checking −70.00, SPL Office Supplies 10.00, SPL Postage 60.00): review lists
    three account names to map; after mapping, commit creates one transaction with
    three splits; re-importing the file finds it as a duplicate.
11. An IIF transaction whose amounts sum to −5.00 is flagged unbalanced; an unknown
    keyword `!INVITEM` row block is skipped with one note.
12. `export.iif` for a month writes `!HDR`, `HDR` (PROD MoneyKeep), the three
    transaction headers, and for each transaction a TRNS row, its SPL rows and
    ENDTRNS, tab-separated with CRLF line endings; re-importing it into a new book
    (with accounts mapped) reproduces the same transactions and amounts.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
