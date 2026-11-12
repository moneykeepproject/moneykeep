# Spec: Business, part 1 — module settings, customers, items, sales tax, estimates, invoices, payments (Phase 4a)

- **Area / module:** module framework (`core`); `invoicing` (new), `estimates`, `items`, `sales-tax`
- **Spec writer:** Claude session 2026-10-20 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-20
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Textbook | OpenStax, *Principles of Accounting, Volume 1*, ch. 3–4 (recording sales on account, collections), ch. 9 (accounts receivable, aging of receivables), ch. 12 (sales tax as a current liability) | Journal entries, aging buckets |
| 2 | Standard | ISO 32000-1 (PDF 1.7), the standard 14 fonts and WinAnsiEncoding | Invoice PDF without embedded fonts |
| 3 | Spec | `ledger-*.md`, `reports.md`, `app-desktop.md`, `definition-files.md`, `input-safety.md` | |
| 4 | Owner | Plan request (2026-10-18): business features as toggleable modules with a Settings page; when on, a list of that feature's settings (e.g. Estimates) | |

No target product was observed. All names, wording, layouts and the PDF design are
MoneyKeep's own. Generic accounting terms (invoice, estimate, credit memo, accounts
receivable, terms such as "Net 30") are used as in the textbook.

## Part A — Module settings framework

The owner's original request: each business feature can be switched on in Settings,
and when on, shows its own list of settings.

- Each module in the catalog may declare a **settings schema**: a list of
  `{key, label, description, type, default, choices?, min?, max?}` where `type` is
  `bool`, `int`, `decimal`, `text`, `choice`, `account` (with allowed account types),
  `party` or `date`. Keys are namespaced by module (`estimates.default_valid_days`).
- Values are stored **per book**. Unset keys read as the default.
- Commands: `module.settings` {`module`} → schema with current values;
  `module.set_setting` {`key`, `value`} (validated against the schema; audited,
  undoable). Turning a module off hides its settings and keeps their values.
- `module.list` gains, per module, `has_settings` and `requires`.
- The catalog gains module **`invoicing`** ("Customers, invoices and payments
  received"); `estimates`, `progress-invoicing`, `sales-orders`,
  `recurring-invoices` and `statements` now require it. `items` and `sales-tax`
  stand alone but are used by invoices when on.

## Part B — Customers (module `invoicing`)

A **party** record serves customers now and vendors in 4b.

| Field | Rules |
|---|---|
| `id` | Typed id |
| `name` | 1–100 chars, unique among parties (case-insensitive) |
| `roles` | Set of `customer`, `vendor` (4b) |
| `company`, `contact` | Optional, ≤ 100 |
| `email` | Optional, ≤ 254, must contain one `@` |
| `phone` | Optional, ≤ 40 |
| `billing_address`, `shipping_address` | Optional, up to 5 lines of ≤ 100 chars |
| `terms` | Default payment terms (below); else the module default |
| `tax_exempt` | Bool, with optional `exemption_note` (≤ 200) |
| `tax_rate` | Optional default tax rate or group (sales-tax module) |
| `notes` | ≤ 4,000 |
| `active` | Bool; inactive parties are hidden from pickers, kept in history |

Commands: `party.create`, `party.update`, `party.get`, `party.list` {`role`,
`active`, `search`}, `party.set_active`, `party.delete` (only if never used).

**Terms:** `due_on_receipt`, `net` {days 1–365} — e.g. Net 15/30/60 — with labels
"Due on receipt", "Net 30". Due date = document date + days.

## Part C — Items (module `items`)

| Field | Rules |
|---|---|
| `name` | 1–100, unique |
| `kind` | `service`, `product` (non-inventory), `other_charge`, `discount` |
| `description` | Default line description, ≤ 1,000 |
| `sku` | Optional, ≤ 40, unique when set |
| `price` | Default rate, ≥ 0 (discount items: a percentage 0–100 or a fixed amount) |
| `income_account` | Income account credited when sold (required; discount items: an income or expense account — a contra-income account is typical) |
| `taxable` | Bool (default true for products, false for services) |
| `active` | Bool |

Commands: `item.create`, `item.update`, `item.get`, `item.list`, `item.set_active`,
`item.delete` (only if never used). Without the `items` module, invoice lines name an
income account directly.

## Part D — Sales tax (module `sales-tax`)

- **Tax rate:** `name` (unique), `rate` percent (0–100, up to 4 decimals),
  `agency` (a party, vendor role in 4b; free text until then), `active`.
- **Tax group:** `name`, member rates; the group's rate is shown as the sum.
- Liability account: `Sales Tax Payable` (liability, subtype `tax-payable`), created
  on first use; one per book (setting `sales-tax.liability_account`).
- **Calculation per document:** taxable base = sum of line amounts of taxable lines
  (a line is taxable when its item is taxable — or the line's own flag without
  items — and the customer is not exempt), after applying discount lines to the lines
  they follow (discounts apply proportionally to preceding taxable and non-taxable
  lines since the previous subtotal). For each component rate: tax = round(base ×
  rate ÷ 100, 2), half away from zero; the document's tax = the sum of components.
  Each component is posted separately (so the liability report can split by rate).
- **Liability report:** `report.sales_tax_liability` {dates, basis `accrual`} — per
  rate: taxable sales, non-taxable sales, tax collected, tax paid, balance.
- **Pay sales tax:** `sales_tax.pay` {from_account, date, amounts per rate (default:
  the balance as of an end date)} posts liability debit / bank credit; one
  transaction; audited.

## Part E — Invoices (module `invoicing`)

### Document

| Field | Rules |
|---|---|
| `number` | Text ≤ 20, unique among invoices; default `invoicing.number_prefix` + next number (settings), editable |
| `customer` | A customer party |
| `date`, `terms`, `due_date` | Due date from terms unless typed |
| `lines` | 1–500: `item` (when items on) or `account` (income), `description` ≤ 1,000, `quantity` (up to 4 decimals, may be negative for returns), `rate` (up to 4 decimals), `amount` = round(quantity × rate, 2) unless typed (then rate is informational), `taxable` (default from item), or a **subtotal** line, or a **discount** line (item kind discount: percent of the lines since the previous subtotal, or a fixed amount) |
| `tax_rate` | Rate or group (default from customer, else `sales-tax.default_rate`); none when the module is off |
| `memo` | Internal, ≤ 1,000 |
| `message` | Printed on the invoice, ≤ 2,000 |
| `status` | `draft` (no ledger effect), `open`, `partial`, `paid`, `void` |

Totals: subtotal (sum of line amounts including discounts), tax, total; balance due =
total − applied payments and credits.

### Ledger effect of an invoice (status open or later)

One transaction dated the invoice date, payee = customer name:

- `Accounts Receivable` (asset, subtype `receivable`, created on first use; setting
  `invoicing.receivable_account`) debit = total.
- Each income account credit = the sum of its lines' amounts (discount lines credit or
  debit their own account).
- Each tax component's liability credit.

The AR split carries the customer party and invoice id. Editing an open invoice
rewrites its transaction (one command; rejected if in the closed period); voiding
voids the transaction and marks the invoice `void` (rejected while payments are
applied — remove them first).

### Commands

`invoice.create` (status draft or open), `invoice.update`, `invoice.finalize` (draft →
open), `invoice.void`, `invoice.delete` (drafts only, or open with no payments —
deletes its transaction), `invoice.get`, `invoice.list` {customer, status, dates,
overdue}, `invoice.duplicate`, `invoice.render_pdf` (below), `invoice.preview`
(totals and tax for unsaved data; no posting).

### Invoice PDF

`invoice.render_pdf` {`invoice`} → `{file_name, content_base64}` (the app saves it).
A one- or multi-page A4/Letter (setting `invoicing.paper`, default Letter) PDF using
only the standard Helvetica fonts (no embedding), MoneyKeep's own simple layout:
the book's business name and address (settings `invoicing.business_name`,
`invoicing.business_address`, `invoicing.business_email`, `invoicing.business_phone`),
"Invoice", number, date, due date, terms, bill-to block, a lines table (description,
quantity, rate, amount), subtotal, each tax line, total, payments applied, balance due,
and the message. Characters outside WinAnsiEncoding are replaced with `?` and the
command returns a warning listing them. The PDF contains no JavaScript, no forms, no
links, no embedded files and no external references (only text, lines and the PDF
structures needed for them).

## Part F — Payments received, credits and deposits

- **Payment:** `payment.receive` {customer, date, amount, method (`cash`, `check`,
  `card`, `bank_transfer`, `other`), reference ≤ 40, `deposit_to` (a bank/cash account,
  or the clearing account below; default setting `invoicing.deposit_to`, default the
  clearing account), `apply` [{invoice, amount}] — default: oldest open invoices first
  until the amount is used}. Posts: deposit account debit / AR credit (with the party).
  Amount not applied stays as **customer credit** (an unapplied payment), usable later
  by `payment.apply` {payment, apply}.
- **Clearing account:** `Payments to Deposit` (asset, subtype `clearing`, created on
  first use). `deposit.create` {bank_account, date, payments [ids], other lines
  [{account, amount, memo}] optional} moves the chosen payments from the clearing
  account into the bank as **one** transaction, matching the single deposit on the
  bank statement (and import matching).
- **Credit memo:** `credit_memo.create` {customer, date, lines as invoice, tax} posts
  the reverse of an invoice (AR credit, income debit, tax liability debit); apply to
  invoices with `credit_memo.apply`; unapplied credit remains.
- **Refund:** `customer.refund` {customer, from_account, amount, date, from credit} pays
  out unapplied credit.
- Applying and unapplying change only invoice balances and statuses (no ledger effect
  beyond the payment's own transaction, which already credited AR).
- Every command audited and undoable.

## Part G — Estimates (module `estimates`)

- Same document shape as invoices (number with `estimates.number_prefix`, customer,
  date, `expires` = date + `estimates.default_valid_days`, lines, tax, message).
  Non-posting.
- Status: `draft`, `sent`, `accepted`, `declined`, `expired` (computed when past
  `expires` and still draft/sent), `converted`.
- `estimate.convert` {estimate, date}: creates an invoice (draft or open per setting
  `estimates.convert_as`) with the same lines and tax; links both; estimate becomes
  `converted`. With `estimates.require_acceptance` true, only accepted estimates
  convert.
- `estimate.render_pdf` as invoices, titled "Estimate", showing "Valid until".
- Commands: `estimate.create`, `update`, `set_status`, `convert`, `delete`, `get`,
  `list`, `duplicate`, `render_pdf`.

## Part H — Module settings (the lists shown in Settings when a module is on)

| Key | Type | Default |
|---|---|---|
| `invoicing.number_prefix` | text ≤ 10 | `INV-` |
| `invoicing.next_number` | int ≥ 1 | 1001 |
| `invoicing.default_terms` | terms | Net 30 |
| `invoicing.deposit_to` | account (asset bank/cash/clearing) | Payments to Deposit |
| `invoicing.receivable_account` | account (asset receivable) | Accounts Receivable |
| `invoicing.business_name`, `business_address`, `business_email`, `business_phone` | text | empty |
| `invoicing.default_message` | text ≤ 2,000 | empty |
| `invoicing.paper` | choice Letter / A4 | Letter |
| `estimates.number_prefix` | text | `EST-` |
| `estimates.next_number` | int | 1001 |
| `estimates.default_valid_days` | int 1–365 | 30 |
| `estimates.require_acceptance` | bool | false |
| `estimates.convert_as` | choice draft / open | draft |
| `estimates.default_message` | text | empty |
| `items.default_service_taxable` | bool | false |
| `items.default_product_taxable` | bool | true |
| `sales-tax.default_rate` | rate or group | none |
| `sales-tax.liability_account` | account (liability tax-payable) | Sales Tax Payable |

Number sequences: the next number advances when a document takes it; a typed number
that collides is rejected; numbers are never reused automatically.

## Part I — Reports

- `report.ar_aging` {`as_of`, `basis`: `due_date` (default) or `invoice_date`,
  `detail` bool}: per customer, open balances bucketed **Current** (not yet due),
  **1–30**, **31–60**, **61–90**, **Over 90** days past due as of the date, and total;
  unapplied credits shown as a negative "Credits" column; detail lists each invoice.
  Total equals the AR account balance as of the date (checked; a difference is
  reported, never hidden).
- `report.customer_balances` {as_of}; `report.open_invoices` {as_of}.
- `report.sales_by_customer` and `report.sales_by_item` {dates}: quantity (items),
  amount, percent; excludes tax.
- `report.balance_sheet` {as_of, depth}: assets, liabilities, equity, with **Net
  income** (current year to date) and **Retained earnings** (prior years' net income)
  computed lines in equity (the fiscal year starts on `book.fiscal_year_start`,
  default 01-01); assets = liabilities + equity is checked.
- All with CSV export and `input-safety.md` R6.

## Edge cases

- Customer marked exempt after invoices were created: existing invoices unchanged.
- A payment larger than all open invoices: the rest is credit.
- Deleting a payment that was deposited: rejected; remove it from the deposit first.
- An invoice with a negative total (all returns): allowed; behaves as a credit.
- Turning `invoicing` off hides screens; AR and other accounts remain in the ledger.

## Acceptance tests

Setup: modules invoicing, items, sales-tax, estimates on. Items: "Consulting"
(service, 125.00, income Consulting Income, not taxable), "Widget" (product, 19.99,
income Product Sales, taxable). Tax group "County" = State 6.00 % + County 1.25 %.
Customer "Acme LLC", Net 30, tax County.

1. Module settings: `module.settings estimates` lists the six keys with defaults;
   setting `estimates.default_valid_days` to 45 is validated (0 rejected), audited and
   undoable; with the module off the settings are hidden and kept.
2. Invoice INV-1001 dated 2026-09-01: Consulting 10 × 125.00 = 1,250.00; Widget 3 ×
   19.99 = 59.97. Tax: State 3.60 + County 0.75 = 4.35. Total 1,314.32, due
   2026-10-01. Posting: AR 1,314.32 Dr; Consulting Income 1,250.00 Cr; Product Sales
   59.97 Cr; Sales Tax Payable 3.60 Cr and 0.75 Cr (two splits). Trial balance zero.
3. Payment 1,000.00 on 2026-09-20 to Payments to Deposit, auto-applied → INV-1001
   `partial`, balance 314.32.
4. Second payment 200.00 from another customer; `deposit.create` into Checking with
   both → one Checking transaction of 1,200.00; Payments to Deposit back to 0.00.
5. Payment 400.00 from Acme on 2026-10-10 → INV-1001 paid (314.32), customer credit
   85.68.
6. AR aging as of 2026-11-15 after test 3 only: Acme 314.32 in **31–60** (45 days past
   due); total equals the AR balance.
7. Credit memo for 1 Widget (19.99, tax 1.20 + 0.25 = 1.45, total 21.44) applied to
   INV-1001 after test 3 → balance 292.88.
8. Estimate EST-1001 with the same lines converts to an invoice with identical totals
   and the next invoice number; the estimate is `converted` and linked. With
   `require_acceptance` true, converting a `sent` estimate is rejected.
9. A discount line "10 % off" after the Widget line in a new invoice (Widget 3 × 19.99)
   gives −6.00 (rounded 5.997 → 6.00); taxable base 53.97; tax 3.24 + 0.67 = 3.91.
10. A tax-exempt customer's invoice has tax 0.00 and the exemption note on the PDF.
11. `invoice.render_pdf` for test 2 produces a PDF that parses as PDF 1.4+, contains
    the strings "INV-1001", "1,314.32" and "Balance due", has no `/JavaScript`,
    `/JS`, `/AA`, `/OpenAction`, `/Launch`, `/URI`, `/EmbeddedFile` or `/AcroForm`
    keys, and a customer named "Café Ünïcode 東京" renders "Café Ünïcode ??" with a
    warning listing the replaced characters.
12. `sales_tax.pay` of the balance after test 2 zeroes Sales Tax Payable; the
    liability report shows State 3.60 collected, 3.60 paid.
13. A separate book with income Sales 500.00 in 2025 and, in 2026, Sales 800.00 and an
    expense 300.00 (all through Checking): the balance sheet on 2026-12-31 shows
    Checking 1,000.00, Retained earnings 500.00, Net income 500.00, and assets =
    liabilities + equity.
14. Voiding INV-1001 with a payment applied is rejected; after unapplying it succeeds
    and the AR transaction is voided.
15. Number sequences: two invoices take INV-1001 and INV-1002; typing INV-1002 again
    is rejected.

## Revision 2 (2026-11-05) — fixed discounts are quantity × rate (owner report)

Owner report: an invoice line for a fixed-amount discount item with Qty 3 and Rate
40 gave −40.00; the owner expects −120.00, as for any other line.

- A **fixed-amount** discount line's amount is −round(quantity × rate, 2), with
  quantity defaulting to 1 (so a line without a quantity gives −rate as before) and
  the rate defaulting to the item's price. A typed amount still wins (−|amount|).
  The line keeps its quantity.
- A **percentage** discount ignores quantity (percent of the lines since the
  previous subtotal, unchanged) and stores none.
- Test 9b: Consulting 3 × 80, Discount ×3 at 40, Consulting 4 × 50, Discount 4 × 15,
  and a Discount line without quantity give −120.00, −60.00 and −40.00; subtotal
  220.00.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
