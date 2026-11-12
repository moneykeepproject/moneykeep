# Spec: Expenses and receipts (Phase 6a)

- **Area / module:** `expenses` (new module)
- **Spec writer:** Claude session 2026-10-23 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-23
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Government | IRS Publication 463 (Travel, Gift, and Car Expenses), recordkeeping chapter — general principle that an expense record shows the amount, the date, the place and the business purpose, supported by receipts | Used only for which fields an expense keeps; MoneyKeep gives no tax advice |
| 2 | Standard | PNG (W3C), JPEG (ITU-T T.81), ISO 32000-1 (PDF image XObjects, standard fonts) | Same decoders and PDF writer as template logos and invoices (`business-dimensions.md` Part C, `business-sales.md`) |
| 3 | Spec | `ledger-transactions.md`, `import-pipeline.md` (matching), `input-safety.md`, `book-encryption.md`, `app-desktop.md` | |
| 4 | Owner request | 2026-10-23: expense manager with scanned receipts (pictures and manual entry), PDF expense report; one submitter (the owner); OCR later | |

No target product was observed. Design and wording are MoneyKeep's own.

## Purpose

Keep receipts with the spending they prove, record expenses that are not yet in the
books (cash, personal card, mileage-free items), and produce a PDF expense report.
Phone capture (`mobile-capture.md`) feeds the same records.

## Behaviour

### Module

Catalog entry `expenses`: "Expenses and receipts: record spending with photos of
receipts and print expense reports". No dependencies. When off, its screens and
commands are hidden; data is kept.

### Receipts (attachments)

- A **receipt** is a stored image or PDF: PNG/JPEG up to 10 MB and 6,000 px on the
  longer side, or PDF up to 10 MB. Images are decoded with the safe decoders,
  auto-rotated by their EXIF orientation, **all metadata removed (including GPS)**,
  scaled so the longer side is at most 2,400 px, and re-encoded (JPEG quality 85 for
  photos; PNG kept as PNG). A 256 px thumbnail is stored too. PDFs are stored as given
  after a structure check: must start with `%PDF-`, ≤ 200 pages, and are never
  rendered or executed by MoneyKeep beyond showing a "PDF" thumbnail; they open in the
  operating system's viewer on request (desktop only, after a confirmation naming the
  file).
- Stored inside the book (so encryption covers them), with `id`, `kind`, `bytes`,
  `sha256`, `width`/`height`, `added_at`, `source` (`desktop`, `phone`, `import`),
  original file name (cleaned).
- A receipt is linked to **one** expense or **one** transaction (or unlinked, "in the
  inbox"). The same SHA-256 added twice is detected: "This receipt is already
  attached to <what>." with Attach anyway / Cancel.

### Expenses

| Field | Rules |
|---|---|
| `date` | required |
| `merchant` | text ≤ 100, required (the payee) |
| `amount` | positive, base currency only in this phase |
| `category` | an expense account, required to post |
| `paid_with` | an account: bank, cash or credit card (paid by the book's own money), or the special **Personal funds (reimbursable)** |
| `purpose` | text ≤ 500 (business purpose / note) |
| `tags` | class / location / fund / project when those modules are on |
| `receipts` | 0–10 receipts |
| `status` | `draft` (missing required fields), `ready`, `posted`, `matched`, `reported` (flag kept with posted/matched) |
| `source` | `desktop` or `phone` |
| `transaction` | the linked transaction once posted or matched |

- **Personal funds (reimbursable):** posting debits the category and credits a
  liability account **Reimbursements Payable** (created on first use, subtype
  `payable`; setting `expenses.reimbursement_account`). Paying the owner back is an
  ordinary transfer from a bank account to that liability; the Expenses screen offers
  **Record reimbursement…** for selected posted reimbursable expenses (one transfer for
  their total, with memo listing them).
- **Posting** a `ready` expense with a book account in `paid_with`:
  1. **Match first:** look for an existing transaction in `paid_with` with the same
     amount (money out), date within ±5 days (setting `expenses.match_days`), not
     already linked to an expense. One candidate → proposed match; several → the user
     picks; none → new.
  2. **Match:** link the expense to that transaction, move its receipts onto the
     transaction, and — when the transaction's category is Uncategorized — set the
     category (and tags) from the expense. Status `matched`. No new transaction.
  3. **New:** create a transaction (date, payee = merchant, memo = purpose, split
     category debit / paid_with credit), receipts attached to it. Status `posted`.
- Later imports: when an imported row matches (pipeline matching) a transaction that
  came from an expense, the normal match rule applies (no duplicate), keeping receipts.
- Editing a posted/matched expense edits its transaction (date, merchant, amount,
  category) through the usual commands with their rules (reconciled values locked).
  Deleting an expense that is posted asks whether to delete the transaction too;
  matched expenses are only unlinked (the bank transaction stays).

### Receipts on transactions

Every register transaction can carry receipts (module on): the edit pop-up gains a
**Receipts** row with thumbnails, **Add receipt…** (file dialog; drag and drop onto the
pop-up works too), **View**, **Remove**. The register shows a small paperclip on rows
with receipts (accessible "Has receipts").

### Inbox

Receipts without an expense (from the phone, drag and drop onto the Expenses screen, or
**Add receipts…** with several files) land in the **Inbox** tab. Each can become a new
expense (form pre-filled with the receipt) or be attached to an existing transaction
(search by amount/date/payee).

### Expense report (PDF)

- `expense.report` {from, to, filter: all | reimbursable | paid_with account | category,
  include_receipts (default true), title (default "Expense report"), mark_reported
  (default false)} → PDF bytes.
- Page 1+: book name (profile name) and title, period, generated date; a table: Date,
  Merchant, Category, Paid with, Purpose (wrapped), Amount, Receipt (✓ or "missing" —
  drawn text "Yes"/"Missing" to avoid glyph issues), sorted by date; totals by category;
  grand total; reimbursable total (amount owed to the owner, minus reimbursements
  already recorded for those expenses).
- Then, with `include_receipts`, one section per expense: a heading line (date,
  merchant, amount) and each image receipt fitted to the page width (aspect kept,
  ≤ 1 per half page); PDF receipts are listed by file name with "attached separately"
  (not merged).
- Standard PDF fonts only; no JavaScript, links, forms or embedded files
  (`input-safety.md` R1). Dates use ISO in this phase (as other PDFs).
- `mark_reported` sets the `reported` flag on the included expenses (one undo step).
- Desktop: **Expense report…** on the Expenses screen opens options and Save as PDF.

### Commands (all audited; MCP yes unless noted)

`expense.create`, `expense.update`, `expense.delete`, `expense.get`, `expense.list`
{from, to, status, paid_with, category, text}, `expense.post` {expense, match?:
transaction | "new"}, `expense.match_candidates`, `expense.record_reimbursement`
{expenses, from_account, date}, `expense.report`, `receipt.add` {content_base64,
file_name, source} (MCP **no**: binary upload from local front ends only),
`receipt.get` (returns bytes; MCP no), `receipt.thumbnail`, `receipt.link` {receipt,
expense | transaction}, `receipt.unlink`, `receipt.delete`, `receipt.inbox`.

### Settings (module `expenses`)

| Key | Type | Default |
|---|---|---|
| `expenses.reimbursement_account` | liability account | Reimbursements Payable |
| `expenses.match_days` | int 0–30 | 5 |
| `expenses.default_paid_with` | account or personal | none |

## Ledger effect

Only through posting (a normal transaction) or matching (category set on an
uncategorized transaction); reimbursements are normal transfers.

## Edge cases

- A receipt with a huge declared size, a decompression bomb or corrupt data is
  refused with "This picture can't be read." before allocating more than the limits.
- HEIC files are refused with "Save the photo as JPEG and try again." (phones convert
  when sharing to a browser; see mobile capture).
- An expense in a closed period cannot be posted or edited.
- Turning the module off hides the paperclip and Receipts row but keeps data.

## Acceptance tests

1. Adding a 4,000 × 3,000 JPEG with EXIF orientation 6 and GPS tags stores a
   2,400 × 1,800 portrait-correct image (1,800 wide × 2,400 high) with no EXIF/GPS
   bytes, plus a 256 px thumbnail.
2. An expense (Office Depot, 45.20, Office Supplies, paid with Visa) posts as a new
   transaction with the receipt attached; trial balance zero.
3. With an imported Visa transaction of 45.20 dated 2 days later and Uncategorized,
   posting proposes a match; accepting links it, sets its category to Office Supplies,
   moves the receipt; no new transaction.
4. A Personal funds expense of 18.75 credits Reimbursements Payable; Record
   reimbursement from Checking clears it to zero.
5. The same image added twice warns "already attached".
6. Expense report for October, reimbursable filter: PDF contains the table, category
   totals, the reimbursable total, and one image page section per expense; it contains
   no `/JS`, `/JavaScript`, `/URI`, `/EmbeddedFile` or `/AcroForm`.
7. `mark_reported` flags the included expenses; Undo clears the flags.
8. A 25 MB file, a text file renamed .jpg and a PDF not starting with `%PDF-` are
   refused with their messages.
9. The register pop-up shows Receipts with a thumbnail; the row shows the paperclip.
10. MCP lists `expense.*` and `receipt.link`/`unlink`/`inbox`/`thumbnail` but not
    `receipt.add` or `receipt.get`.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-10-23) — review decisions

- `reported` is a flag kept with `posted`/`matched`, not a separate status.
- `expense.report` stays read-only; marking moves to a new command
  `expense.mark_reported {expenses}` (audited, undoable). The report dialog's "Mark
  as reported" calls it after the PDF is saved.
- Deleted receipts' bytes may stay inside the book for Undo (still encrypted with
  the book). `book.compact` (future) may purge unreferenced bytes.
- Opening a PDF receipt in the system viewer writes a decrypted temporary copy: the
  confirmation says "A temporary unencrypted copy will be opened."; the copy goes in
  a per-session folder with owner-only permissions and is deleted when the book
  closes or the app exits (and any leftovers from earlier sessions on start).
- AT1 means: the stored image is 1,800 wide × 2,400 high.
