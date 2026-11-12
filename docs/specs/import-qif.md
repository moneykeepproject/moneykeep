# Spec: QIF import

- **Area / module:** import (`core`)
- **Spec writer:** Claude session 2026-10-18 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-18
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Public documentation | "Quicken Interchange Format", Wikipedia — https://en.wikipedia.org/wiki/Quicken_Interchange_Format | Headers, field codes, splits, dates, amounts, record end, limitations |
| 2 | Public documentation | Intuit technical support FAQ "What is the Quicken interchange format (QIF)?" (1997), archived at https://www.w3.org/2000/10/swap/pim/qif-doc/QIF-doc.htm | Original field list, transfer notation, `!Option:AllXfr` |
| 3 | Spec | `import-pipeline.md` | Raw row, staging, resolution |

Both are public format descriptions. No product was run or observed, and no
implementation source was read.

## Purpose

QIF is an old plain-text export format still offered by some banks and used to move
data out of older personal-finance programs.

## Behaviour

### File and records

- Text file. Decode as UTF-8 if valid (with or without BOM); otherwise as
  Windows-1252. Line endings CRLF, LF or CR.
- A **header** line starts with `!`. `!Type:<name>` sets the record type for following
  records until the next header. Recognised types (case-insensitive):
  `Bank`, `Cash`, `CCard`, `Oth A`, `Oth L` (bank-style records) and `Invst`
  (investment records). `!Account` starts an account-list block. `!Type:Cat`,
  `!Type:Class`, `!Type:Memorized`, `!Type:Invoice` blocks are read and ignored.
  `!Option:...` lines are ignored. Any other header is an error naming the line.
- A **detail line** is one field-code character followed by its value. A record ends at
  a line containing only `^`. A final record without `^` is accepted with a warning.
- Blank lines are ignored. Leading/trailing whitespace in values is trimmed.
- Fields may appear in any order; if a non-split code repeats in one record, the last
  one wins.

### Bank-style record fields

| Code | Meaning | Mapping |
|---|---|---|
| `D` | Date | row `date` (see Dates) |
| `T` | Amount | row `amount` (see Amounts) |
| `U` | Amount (duplicate of `T` in newer exports) | used only if `T` is absent |
| `P` | Payee | `payee` |
| `M` | Memo | `memo` |
| `N` | Number or keyword (e.g. cheque number, `ATM`, `EFT`, `Deposit`) | `number` |
| `C` | Cleared: blank, `*` or `c` = cleared; `X` or `R` = reconciled | `cleared` |
| `L` | Category or transfer; `[Name]` is a transfer to account *Name*; `Cat:Sub` is a sub-category; `/Class` suffix is a class | `category_hint` (class part dropped in Phase 2a) |
| `A` | Address line (up to 6) | ignored |
| `F` | Reimbursable flag | ignored |
| `S` | Split category (same notation as `L`) | starts a split |
| `E` | Split memo | belongs to current split |
| `$` | Split amount | belongs to current split |
| `%` | Split percentage | ignored |

Splits: each `S` starts a new split; the following `E` and `$` lines belong to it.
A split with no `$` is an error. If split amounts do not sum to `T`, the row is staged
with `needs_review` (pipeline rule).

### Investment records (`!Type:Invst`)

Phase 2a does not import investment activity (that is Phase 3). Investment records are
counted and reported in the staging result as "skipped: investment records (n)", and
cause no error.

### Account blocks

An `!Account` block's `N` (name) and `T` (type) give the account the following
`!Type:` records belong to; each such account becomes its own section. Its name is the
section's account identity for mapping (`qif:` + name). Files without account blocks
have one section, resolved by the user or the account given to `import.stage`.

### Dates

QIF dates have no single fixed format. Accepted shapes, with optional leading zeros
and spaces:

- `M/D/YY`, `M/D/YYYY`, `M/D'YY` (apostrophe before a 2-digit year in 2000–2099), with
  `/`, `-` or `.` separators.
- `YYYY-MM-DD`.

Two-digit years 00–69 → 2000–2069, 70–99 → 1970–1999 (apostrophe form always 20YY).

Day/month order: default month-first (`date_order: "mdy"`). `import.stage` accepts
`date_order: "dmy"` for files from day-first locales. If any date in the file is
impossible in the chosen order (e.g. month 13) and valid in the other, staging fails
with an error suggesting the other order; it never silently swaps.

### Amounts

- Optional leading `+` or `-`; payments carry `-`.
- Thousands separators `,` are removed. The decimal separator is `.`; if
  `decimal_comma: true` is given to `import.stage`, `.` is the thousands separator and
  `,` the decimal.
- Currency symbols are not expected; a leading `$` is tolerated and removed.
- Amount precision must fit the account currency (pipeline/ledger rules).

### No unique ids

QIF has no transaction id, so rows use content fingerprints (pipeline).

## Ledger effect

None directly; see `import-pipeline.md`.

## Edge cases

- `!Type:CCard`: amounts are already from the card holder's view (charges negative).
- Opening-balance record: payee `Opening Balance` with `L[Own account name]` is mapped
  to the book's `Opening Balances` account instead of a transfer to itself.
- A file with only `!Type:Cat` blocks: zero rows, staged successfully with a note.

## Acceptance tests

Fixtures are created by the team and recorded in `examples/README.md`.

1. A `!Type:Bank` file with three records (deposit, cheque with `N1042`, card payment)
   stages three rows with correct dates, signed amounts, payees, numbers.
2. `D6/ 1'26` parses as 2026-06-01; `D25/12/2026` with default order fails with an
   error suggesting `dmy`; with `dmy` it is 2026-12-25.
3. `T-1,234.56` → −1234.56; with `decimal_comma`, `T-1.234,56` → −1234.56.
4. A record with `L[Savings]` gives category hint `[Savings]`, resolved to the
   `Savings` account as a transfer.
5. A split record (`T-100.00`, `SGroceries $-60.00`, `SHousehold $-40.00`) produces one
   transaction with three splits; a split set summing to −90.00 is flagged
   `needs_review`.
6. `CX` and `CR` stage as reconciled, `C*` and `Cc` as cleared.
7. A Windows-1252 file with `é` in a payee decodes correctly; a UTF-8 file with BOM
   decodes correctly.
8. A file with an `!Account` block for two accounts produces two sections.
9. `!Type:Invst` records are skipped with a count, not an error.
10. A missing final `^` is accepted with a warning.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
