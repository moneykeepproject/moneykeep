# Spec: Form1099Keep tables in the book (migration v15)

- **Area / module:** storage (`core`)
- **Spec writer:** Claude session 2026-11-04 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-11-04
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Spec | `C:\dev\form1099keep\docs\specs\form1099keep.md` revision 1 | The separate Form1099Keep program |
| 2 | Spec | `book-encryption.md`, `ledger-audit-undo.md` | Encryption, audit log |

## Purpose

Form1099Keep (a separate program in the MoneyKeep family) keeps payers, recipients
and Forms 1099-NEC/MISC in an ordinary MoneyKeep book, so both programs share one
file format, its encryption and its audit log.

## Behaviour

- Migration v15 adds three document tables of the usual shape (`id`, `grp`, `ord`,
  `json`): `f1099_payer`, `f1099_recipient`, `f1099_form` (grp = tax year, indexed).
  `DocTable` gains `F1099Payer`, `F1099Recipient`, `F1099Form`; the audit snapshot
  kinds gain `f1099_payer`, `f1099_recipient`, `f1099_form`.
- MoneyKeep registers no commands on these tables, so they are not reachable through
  the CLI, the JSON control channel or `keep-mcp`. Payers and recipients contain
  taxpayer id numbers; Form1099Keep writes them only while the book is password
  protected, and its audit entries carry no TINs and no snapshots.
- A book at v14 opened by a build with v15 gains the empty tables; nothing else
  changes. A v15 book opened by an older build fails with `TooNew`, as for any
  migration.
