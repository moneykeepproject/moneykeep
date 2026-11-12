# Spec: CSV import with institution profiles

- **Area / module:** import (`core`)
- **Spec writer:** Claude session 2026-10-18 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-18
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Standard | RFC 4180, Common Format and MIME Type for CSV Files — https://www.rfc-editor.org/rfc/rfc4180 | Quoting, embedded commas/quotes/newlines |
| 2 | Spec | `import-pipeline.md` | Raw row, fingerprints, staging |
| 3 | Design | MoneyKeep plan: "CSV mapping profiles per institution, shareable as plugins" | |

The profile design is MoneyKeep's own. No target product was observed.

## Purpose

Most banks offer CSV downloads, each with different columns. A **profile** describes
one layout once, so every later file from that institution imports with no questions.

## Behaviour

### Parsing

- Follow RFC 4180 quoting: fields may be enclosed in `"`; a `"` inside is written `""`;
  quoted fields may contain the delimiter and line breaks.
- Delimiter: `,` by default; profiles may set `;`, tab or `|`.
- Decoding as in `import-qif.md` (UTF-8 with or without BOM, else Windows-1252).
- Lines that are entirely empty are skipped.

### Profile

A profile is a named JSON object stored in the book (`csv_profile` table) and
exportable/importable as a file:

| Field | Type | Meaning |
|---|---|---|
| `name` | string | Unique in the book |
| `delimiter` | `,` `;` `\t` `|` | |
| `skip_rows` | integer ≥ 0 | Lines to skip before the header (bank preamble) |
| `has_header` | boolean | First non-skipped line is a header |
| `columns` | object | Maps roles to a column, by header name (case-insensitive, trimmed) or 1-based index |
| `date_format` | string | One of `YYYY-MM-DD`, `MM/DD/YYYY`, `DD/MM/YYYY`, `M/D/YYYY`, `D/M/YYYY`, `DD.MM.YYYY`, `YYYYMMDD` |
| `amount_mode` | `signed` / `split` / `type_column` | How the amount is expressed (below) |
| `negate` | boolean | Multiply amounts by −1 (for files that show charges as positive) |
| `decimal_comma` | boolean | As in `import-qif.md` |
| `debit_values` | list of strings | For `type_column` mode: values meaning money out (e.g. `DEBIT`, `DR`) |
| `account` | account ref or null | Default target account |
| `skip_if` | list of {column, equals} | Rows to ignore (e.g. pending lines) |

Column roles: `date` (required), `amount` or `debit`+`credit` (required per mode),
`payee`, `memo`, `number`, `external_id`, `category`, `type`, `balance`.

### Amount modes

- `signed`: one `amount` column; negative is money out (then `negate` if needed).
- `split`: `debit` and `credit` columns; amount = credit − debit; empty counts as 0;
  a row with both non-zero is an error.
- `type_column`: `amount` holds an unsigned value and `type` says direction; values in
  `debit_values` (case-insensitive) are money out.

Amount text: strip currency symbols (`$ € £ ¥`), spaces and thousands separators;
parentheses `(12.34)` mean negative.

### Running balance column

If `balance` is mapped, the last row's balance (by file order, or by date if the file
is newest-first) becomes the section's closing balance for statement capture, dated
the latest row date.

### Profile detection and creation

- `import.stage` with `format: "csv"` takes `profile` (name) or none.
- With no profile: if exactly one saved profile's header names all appear in the
  file's header, it is used. Otherwise staging returns `needs_profile` with the
  detected header, the first 5 rows, and a **suggested** profile (guessing roles from
  header words such as date, amount, debit, credit, description, payee, memo; and the
  date format from the values). Nothing is staged until a profile is chosen.
- `csv.profile_save`, `csv.profile_list`, `csv.profile_delete`, `csv.profile_export`,
  `csv.profile_import` manage profiles (all audited except list/export).

### Fingerprints

If `external_id` is mapped and non-empty, it is the row's `external_id`; otherwise
content fingerprints apply (pipeline).

## Ledger effect

None directly; see `import-pipeline.md`.

## Edge cases

- Newest-first files: order does not affect fingerprints (occurrence numbering is by
  file order, which is stable across re-downloads of the same file).
- Trailing summary line (e.g. "Total"): use `skip_if`, or it fails date parsing and
  staging reports the line number.
- A required column missing from a file: staging error naming the column.

## Acceptance tests

1. RFC 4180: a payee `"Smith, Jones & ""Co"""` parses to `Smith, Jones & "Co"`; a
   quoted memo containing a newline stays one row.
2. `signed` mode with `negate`: `12.50` becomes −12.50.
3. `split` mode: debit `40.00`, credit empty → −40.00; both filled → error with line
   number.
4. `type_column` mode with `debit_values: ["DR"]`: `DR,15.00` → −15.00, `CR,15.00` →
   +15.00.
5. `(1,234.56)` → −1234.56; `€1.234,56` with `decimal_comma` → 1234.56.
6. `skip_rows: 3` skips a three-line bank preamble.
7. A file with no profile returns `needs_profile` with a suggestion that maps `Date`,
   `Description` → payee, `Amount`, and detects `MM/DD/YYYY` from values like
   `10/05/2026`.
8. A saved profile is auto-selected for a later file with the same headers.
9. A `balance` column produces a captured statement balance.
10. Profile export then import into another book yields an identical profile.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-10-22) — transaction date column

Card and bank downloads often have both a transaction date and a posting date column
(e.g. "Transaction Date" and "Posted Date"). Imports use the transaction date
(`import-pipeline.md` revision 2, `app-desktop.md` revision 21).

- **Suggested profile:** among headers that look like dates, the date role goes to a
  header containing "trans" (transaction, trans., trans date) or equal to "date";
  headers containing "post" or "settle" are chosen only when no other date header
  exists. With `import.date_source` = `posted` the preference is reversed. This holds
  whatever order the columns are in.
- **Profiles** gain optional `date_kind` (`transaction` / `posted`). The suggestion
  sets it from the chosen header ("post"/"settle" → `posted`; "trans" →
  `transaction`; otherwise unset). Rows take it as their `date_kind` (unset →
  `unknown`). Older profiles without it load unchanged.
- **Warning on staging:** when a saved profile's date column header contains "post"
  or "settle" and the file also has a header containing "trans" that looks like a
  date, staging adds the warning "This profile uses the posting date (<header>). The
  file also has <other header>; edit the profile to use the transaction date."

Acceptance tests:

13. A header `Posted Date,Transaction Date,Card No.,Description,Category,Debit,Credit`
    suggests date = `Transaction Date` with `date_kind` `transaction`; with
    `import.date_source` = `posted`, `Posted Date`.
14. Staging that file with a saved profile mapped to `Posted Date` shows the warning.
