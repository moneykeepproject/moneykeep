# Spec: Business, part 2 — vendors, bills, bill payments, purchase orders, 1099 tracking (Phase 4b)

- **Area / module:** `bills` (vendors, bills, payments, credits), `purchase-orders`, `1099`
- **Spec writer:** Claude session 2026-10-21 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-21
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Textbook | OpenStax, *Principles of Accounting, Volume 1*, ch. 3 and ch. 12 (purchases on account, accounts payable, current liabilities) | Journal entries |
| 2 | Government | IRS Instructions for Forms 1099-MISC and 1099-NEC | Who gets a 1099-NEC (nonemployee compensation) or 1099-MISC (rents, royalties, other); payments made by credit card or payment card / third-party network are reported by the payment settlement entity on Form 1099-K, not by the payer |
| 3 | Guidance | NATP, "Form 1099 filing changes under the new $2,000 threshold" — https://www.natptax.com/news-insights/blog/form-1099-filing-changes-under-the-new-2-000-threshold/ | For payments made after 2025-12-31 the $600 threshold becomes $2,000 for 1099-NEC and most 1099-MISC boxes; royalties stay at $10 |
| 4 | Spec | `business-sales.md` (parties, terms, items, sales tax, numbering, module settings), `ledger-*.md`, `reports.md` | |

No target product was observed. Record-keeping only; MoneyKeep does not file forms
or give tax advice.

## Purpose

Track what the business owes and has paid to suppliers, pay bills on time, see
what is coming due, and know at year end which vendors need a 1099.

## Behaviour

### Module changes

- Catalog: `bills` ("Vendors, bills, bill payments and vendor credits (accounts
  payable)"); `purchase-orders` requires `bills`; `1099` requires `bills`.
- `sales-tax`: a tax rate's `agency` may now be a vendor party (free text kept for
  older rates).

### Vendors

The party record of `business-sales.md` with role `vendor`, plus:

| Field | Rules |
|---|---|
| `vendor_terms` | Default terms for bills (else `bills.default_terms`) |
| `default_expense_account` | Optional account pre-filled on bill lines |
| `account_number_at_vendor` | Optional, ≤ 40 (the customer number the vendor gave you); shown only on screens and bills, never exported in definitions |
| `tracks_1099` | Bool (module `1099`) |
| `form_1099` | `nec` (default) or `misc` |
| `misc_box` | For MISC: `rents` (box 1), `royalties` (box 2), `other_income` (box 3), `medical` (box 6), `attorney_gross_proceeds` (box 10) |
| `tin_on_file` | Bool only — MoneyKeep never stores the taxpayer id number itself |

### Bills

| Field | Rules |
|---|---|
| `number` | Vendor's invoice number (text ≤ 40, optional); unique per vendor when set |
| `vendor`, `date`, `terms`, `due_date` | As invoices |
| `lines` | 1–500: **expense lines** {account (expense, asset or liability), description, amount} and, with `items` on, **item lines** {item, description, quantity, cost, amount = round(q × cost, 2)} — an item's purchase side uses `item.expense_account` (new optional item field, required when an item is bought) |
| `memo` | ≤ 1,000 |
| `status` | `open`, `partial`, `paid`, `void` (bills post when saved; there is no draft) |

**Ledger effect:** `Accounts Payable` (liability, subtype `payable`, created on first
use; setting `bills.payable_account`) credit = total; each line's account debit. The
AP split carries the vendor. Sales tax paid on purchases is simply a line to an
expense or asset account (no input-tax credits in this phase).

Commands: `bill.create`, `bill.update` (rewrites its transaction; rejected if in the
closed period or if the new total is below what is applied), `bill.void` (rejected
while payments are applied), `bill.delete` (no payments), `bill.get`, `bill.list`
{vendor, status, dates, due_before, overdue}, `bill.duplicate`.

### Bill payments

`bill.pay` {vendor, date, `from_account` (bank, cash, or credit card), method
(`check`, `bank_transfer`, `card`, `cash`, `other`), `check_number` (for checks;
default `bills.next_check_number` per bank account), reference, `apply` [{bill,
amount}] (default oldest due first), amount (defaults to the sum applied)}.

- **Ledger effect:** AP debit (with vendor) / from_account credit. Paying with a
  credit card account makes the card liability grow (a charge).
- Amount beyond the applied bills is a **vendor prepayment** (debit balance with the
  vendor), applied later with `bill_payment.apply`.
- `bill.pay_many` {date, from_account, method, bills [{bill, amount}]} creates one
  payment per vendor in one command (the "pay bills" run), numbering checks
  sequentially.
- Check numbers are stored on the payment and on the bank transaction (`number`
  field), so the register shows them and bank import matching can use them.

### Vendor credits

`vendor_credit.create` {vendor, date, number?, lines as bills}: AP debit / line
accounts credit. Applied to bills with `vendor_credit.apply`; unapplied credit stays
with the vendor. `vendor.refund_received` {vendor, to_account, amount, date} records
cash received against unapplied credit or prepayment.

### Purchase orders (module `purchase-orders`)

- Same document shape as a bill with items or expense lines, plus `ship_to`,
  `expected_date`, number (`purchase-orders.number_prefix` + next number). **Non-posting.**
- Status: `open`, `partly_received`, `received`, `closed` (manually), `cancelled`.
- `po.receive` {po, date, lines [{po_line, quantity or amount}], bill number?}:
  creates a **bill** for the received quantities/amounts, links it to the PO and
  updates each line's received quantity; status follows. Over-receiving a line is
  allowed with a warning.
- Commands: `po.create`, `po.update`, `po.set_status`, `po.receive`, `po.get`,
  `po.list`, `po.render_pdf` (as invoices: titled "Purchase order", no active
  content), `po.delete` (no receipts).

### 1099 tracking (module `1099`)

- **What counts:** for each vendor with `tracks_1099`, payments made in the calendar
  year by `check`, `bank_transfer`, `cash` or `other` — **excluding** payments made
  with a credit-card account or method `card` (reported by the card network on Form
  1099-K, source 2). Amounts are the payment amounts applied to bills plus direct
  expense transactions whose payee is linked to the vendor (an optional `vendor` link
  on ordinary transactions, set in the register), counting only lines posted to
  accounts mapped to 1099 (below).
- **Account mapping:** setting `1099.accounts` — the expense accounts whose payments
  count (default: all expense accounts). Payments for goods (e.g. an account marked
  `inventory`/materials) can be excluded by leaving them out.
- **Thresholds** (setting `1099.thresholds`, a table by year and form/box, editable):
  default for years ≤ 2025: 600.00 for NEC and MISC boxes 1, 3, 6, 10; for 2026 and
  later: 2,000.00 for those; royalties (MISC box 2): 10.00 every year. Later years
  copy the latest year until the user updates them (the law indexes the threshold for
  inflation after 2026; MoneyKeep shows a note asking the user to check each year).
- `report.1099` {year, `include_below_threshold` (default false)}: per vendor: name,
  form and box, total paid in the year (qualifying), excluded card payments (shown for
  information), whether `tin_on_file`, address completeness, and a flag when at or
  above the threshold. Missing address or TIN-on-file for a vendor above threshold is
  flagged. CSV export (with `input-safety.md` R6).
- MoneyKeep does not print or e-file forms in this phase.

### Reports

- `report.ap_aging` {as_of, basis due_date|bill_date, detail}: per vendor, open bill
  balances in Current, 1–30, 31–60, 61–90, Over 90; prepayments and unapplied credits
  as a negative "Credits" column; total ties to the Accounts Payable balance (a
  difference is reported).
- `report.vendor_balances`, `report.unpaid_bills` {as_of, due_before},
  `report.expenses_by_vendor` {dates}, `report.open_purchase_orders`,
  `report.check_register` {bank account, dates} (checks with numbers, payees,
  amounts, voided flag).
- `report.bills_due` {from, to}: bills due in the window, for planning payment runs;
  Home's Attention list gains "N bills overdue" and "N due in the next 7 days".

### Settings (module lists)

| Key | Type | Default |
|---|---|---|
| `bills.default_terms` | terms | Net 30 |
| `bills.payable_account` | account (liability payable) | Accounts Payable |
| `bills.default_payment_account` | account (bank/cash/card) | none |
| `bills.next_check_number` | per-bank map of int | empty (asks on first check) |
| `purchase-orders.number_prefix` | text | `PO-` |
| `purchase-orders.next_number` | int | 1001 |
| `purchase-orders.default_message` | text | empty |
| `1099.accounts` | list of expense accounts | all expense accounts |
| `1099.thresholds` | table | as above |

All commands audited and undoable.

## Ledger effect

As described: bills and vendor credits post to AP and line accounts; bill payments
post AP against the paying account; POs are non-posting.

## Edge cases

- Paying a bill with a credit card account, then paying the card from checking: the
  1099 report excludes the bill payment (card) — correct per source 2.
- A vendor that is also a customer: one party with both roles; A/R and A/P stay
  separate (no automatic netting).
- Voiding a check (bill payment): the payment's transaction is voided, its
  applications removed, the bills reopen; the check number stays used and shows as
  void in the check register.
- A bill dated in a closed period cannot be paid with a payment dated in it.

## Acceptance tests

Setup: modules bills, purchase-orders, 1099, items on. Vendors "Office Supply Co"
(Net 30), "J. Smith Design" (tracks 1099, NEC, TIN on file, address complete),
"Landlord LLC" (tracks 1099, MISC rents). Accounts: Checking (bank), Visa (credit
card), Office Supplies, Contract Labor, Rent (expenses).

1. Bill from Office Supply Co dated 2026-09-05, lines Office Supplies 240.00 and
   Office Supplies 60.50 → total 300.50, due 2026-10-05; AP credit 300.50, Office
   Supplies debit 300.50; trial balance zero.
2. `bill.pay` 200.00 by check from Checking on 2026-09-20 with check number 1001 →
   bill `partial`, balance 100.50; Checking register shows number 1001 and payee
   Office Supply Co; `bills.next_check_number` for Checking is 1002.
3. AP aging as of 2026-11-10 → Office Supply Co 100.50 in **31–60** (36 days past due);
   total equals the AP balance.
4. Overpayment: paying 150.00 against the 100.50 balance leaves a 49.50 vendor
   prepayment shown as a credit in AP aging.
5. Vendor credit 25.00 applied to a new 80.00 bill → balance 55.00.
6. `bill.pay_many` paying three bills from two vendors creates two payments with check
   numbers 1002 and 1003 in one command; undo removes both and restores the next
   check number.
7. Voiding check 1002 reopens its bills; the check register lists 1002 as void.
8. PO-1001 for 10 × "Paper" (item, cost 4.25) = 42.50; `po.receive` 6 → a bill of
   25.50 linked to the PO, PO `partly_received`; receiving 4 more → second bill
   17.00, PO `received`.
9. 1099 for 2026: J. Smith Design paid 1,500.00 by check and 900.00 by bank transfer
   (Contract Labor) and 400.00 with the Visa card → qualifying 2,400.00 (≥ 2,000.00,
   flagged to file), card 400.00 shown as excluded. For 2025 data with 700.00 paid by
   check → flagged (≥ 600.00 threshold for 2025).
10. Landlord LLC paid 1,800.00 rent by check in 2026 → MISC box 1, below 2,000.00, not
    flagged (listed only with `include_below_threshold`).
11. A tax rate whose agency is a vendor: `sales_tax.pay` creates the payment to that
    vendor (payee set) — appears under the vendor's activity, not in the 1099 report
    unless the vendor tracks 1099.
12. Undo of `bill.pay` restores the bill to open and removes the check.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
