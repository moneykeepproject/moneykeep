# Spec: Import pipeline (all file formats and feeds)

- **Area / module:** import (`core`)
- **Spec writer:** Claude session 2026-10-18 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-18
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Design | MoneyKeep plan document: "Connectivity landscape → Import pipeline", data model (`import_batch`, `import_row`, `rule`, `statement`) | One dedupe and matching path for every source |
| 2 | Textbook | OpenStax, *Principles of Accounting, Volume 1* — https://open.umn.edu/opentextbooks/textbooks/principles-of-accounting-volume-1-financial-accounting | Bank reconciliation concepts |
| 3 | Spec | `ledger-transactions.md`, `ledger-accounts.md`, `ledger-audit-undo.md` | Transactions, reconcile flags, undo |

Format parsers are specified separately: `import-qif.md`, `import-csv.md`, `import-ofx.md`.
No target product was observed for this spec.

## Purpose

People download files from their banks and card issuers. MoneyKeep turns each file
into transactions in the right account, without creating duplicates when the same
transactions arrive twice, and lets the user review what will happen before it does.

## Behaviour

### Stages

1. **Parse** a file into a format-neutral *statement*: zero or more account sections,
   each with optional account identity, optional currency, optional statement period
   and closing balance, and a list of raw rows.
2. **Stage** the statement as one `import_batch` with its `import_row`s. Nothing in the
   ledger changes.
3. **Resolve** each section to a MoneyKeep account.
4. **Classify** each row: duplicate, matched to an existing transaction, or new.
5. **Review**: the user (or a client) inspects and adjusts the proposal.
6. **Commit** the batch: creates and updates transactions in one command. Undo of the
   commit reverses all of it.

Stages 1–4 happen in `import.stage`; 5 uses `import.*` editing commands; 6 is
`import.commit`.

### Raw row (format-neutral)

| Field | Rules |
|---|---|
| `external_id` | The source's stable transaction id when it has one (OFX FITID); otherwise null |
| `date` | Posting date |
| `amount` | Signed decimal from the account holder's view: + money in, − money out |
| `payee` | Text, may be empty |
| `memo` | Text, may be empty |
| `number` | Cheque or reference number, may be empty |
| `category_hint` | Category/transfer text from the source (QIF `L`), may be empty |
| `cleared` | Source's cleared/reconciled flag when present |
| `splits` | Optional source splits (QIF `S`/`$`), each with category hint, amount, memo |
| `raw` | Original record text, kept for display and diagnostics |

### Fingerprint

Every row gets a `fingerprint`, used to detect duplicates:

- With an `external_id`: `ext:` + resolved account id + `external_id`.
- Without: `hash:` + resolved account id + SHA-256 of
  `date | amount | normalised payee | number | occurrence`, where *occurrence* is
  1, 2, 3… for rows in the same file with identical date, amount, payee and number
  (so two identical $5.00 coffees on one day are two rows, and re-importing the same
  file still matches both).
- *Normalised payee*: upper-case, collapse whitespace, strip leading/trailing
  punctuation.

Committed rows store their fingerprint on the created or matched transaction's split
(`import_fingerprint`), so later imports see them.

### Account resolution

- A section with account identity (OFX bank id + account id, or a CSV/QIF profile)
  resolves through a stored mapping (`import_account_map`: source key → account id).
- If no mapping exists, the section is *unresolved*; the user picks an account with
  `import.map_account`, which can also save the mapping for future files.
- QIF and CSV files without account identity always need the user's pick (or the
  account named in the command).
- A section's currency, when present, must equal the account's commodity; otherwise
  the section is rejected with a clear error.

### Classification of each row

In this order:

1. **Duplicate**: a transaction split in the target account already carries the same
   fingerprint, or another row earlier in this batch has it. Duplicates are skipped on
   commit.
2. **Match**: an existing posted split in the target account with no fingerprint,
   the **same amount**, a date within the match window (setting
   `import.match_window_days`, default 3), and not already matched in this batch. If
   several candidates, prefer the closest date, then the most similar payee, then the
   earliest entered. A match does not create a transaction: commit attaches the
   fingerprint to the existing split and sets its reconcile flag to at least `c`.
3. **New**: everything else. Commit creates a two-split (or split) transaction.

### Proposed transaction for a new row

- Date, payee, number, memo from the row.
- Reconcile flag of the target-account split: rows from a bank or card download are
  transactions the institution has already posted, so they start **cleared** (`c`):
  every OFX/QFX/QBO row, and every CSV row. QIF rows use their `C` field (`cleared` or
  `reconciled` → `c`; blank → `n`), since QIF files are often exports from other
  personal-finance programs rather than bank downloads. Only reconciliation sets `r`.
  The other-side splits start at `n`.
- Split 1: the target account, value = row amount.
- Other side, in priority order:
  1. Source splits, each mapped to an account (below), if present and summing to the
     row amount; otherwise the row is flagged `needs_review` and uses option 4.
  2. A matching categorisation rule (`import-rules.md`).
  3. The `category_hint` resolved to an account: a `[Name]` hint (QIF transfer) or a
     name that resolves by full name to an existing account (case-insensitive).
  4. The book's **uncategorised** account: `Uncategorized Expense` for money out,
     `Uncategorized Income` for money in. Both are created on first use (top-level,
     types expense/income) and are ordinary accounts.
- **Transfers between two imported accounts**: if a new row's other side is an asset
  or liability account (a transfer) and that account already has (or this batch
  contains) a row of the opposite amount within the match window, the two are linked
  as one transaction rather than creating two. The second row is classified as a
  match to the first.

### Review commands

| Command | Mutates | Effect |
|---|---|---|
| `import.stage` | yes | Parse a file (path or inline content + `format`), create batch; returns batch id, sections, rows with classification and proposals |
| `import.list` | no | Batches with status `staged`, `committed`, `discarded` |
| `import.get` | no | One batch with rows and proposals |
| `import.map_account` | yes | Resolve a section to an account; optional `remember` |
| `import.set_row` | yes | Override one row: action (`create`, `match` with a transaction id, `skip`), other-side account, payee, memo |
| `import.commit` | yes | Apply all non-skipped rows; batch becomes `committed`. Rejected if any section is unresolved or any `create` row lacks an other side |
| `import.discard` | yes | Batch becomes `discarded`; nothing in the ledger changes |

All of these are audited. Undo of `import.commit` removes created transactions and
detaches fingerprints from matched splits (restoring their previous reconcile flags);
it is rejected if any created transaction has since been reconciled or edited.

### Statement capture

If a section carries a closing balance and date (OFX ledger balance), the batch
records it as a `statement` for that account (date, balance, source batch). It is
offered to the reconciliation workflow (`reconciliation.md`) but not applied
automatically.

### Watched folder

- Setting `import.watch_folder` (path) and `import.watch_archive` (path, default a
  subfolder `imported` inside the watch folder).
- `moneykeep watch <book>` runs until interrupted: every new or changed file with a
  supported extension (`.ofx`, `.qfx`, `.qbo`, `.qif`, `.csv`) that has been unchanged
  for 2 seconds is passed to `import.stage` with actor `watch`. Fully resolved
  batches whose rows are all duplicates, matches or rule-categorised are committed
  automatically **only** if setting `import.auto_commit` is true (default false);
  otherwise they wait for review.
- After staging, the file is moved to the archive folder (name kept; a numeric suffix
  is added on clash). A file that fails to parse is moved to `failed/` with a
  `.error.txt` next to it.
- The watch loop never touches files outside the watch folder and never deletes.
- This is desktop/server only, not part of the wasm build.

### Limits

- Files larger than 50 MB are rejected.
- A batch holds at most 100,000 rows.

## Ledger effect

Only `import.commit` (and its undo) changes the ledger.

## Settings

| Module | Key | Type | Default |
|---|---|---|---|
| core | `import.match_window_days` | integer 0–30 | 3 |
| core | `import.auto_commit` | boolean | false |
| core | `import.watch_folder` | string path or null | null |
| core | `import.watch_archive` | string path or null | null (→ `<watch>/imported`) |

## Edge cases

- Importing the same file twice: second batch is all duplicates; commit creates
  nothing.
- A file covering an overlapping period with a previous one: overlap rows are
  duplicates, the rest are new.
- A manually entered transaction later downloaded: matched, not duplicated.
- Two identical rows in one file (same date, amount, payee): two new transactions.
- An OFX correction (`import-ofx.md`) that replaces an earlier FITID: handled by the
  OFX spec's correction rule, then classified normally.
- A row of amount 0.00: staged, defaults to `skip`.

## Acceptance tests

1. Staging a file changes no ledger data (trial balance and transaction count
   unchanged).
2. Committing then re-staging the same file classifies every row as duplicate.
3. A manual −42.10 transaction on 2026-10-03 and an imported −42.10 row on 2026-10-05
   (window 3) is a match; commit attaches the fingerprint, sets the split to `c`, and
   creates no transaction. With window 1 it is new.
4. Two identical rows in one file become two transactions; re-importing finds both
   duplicates.
5. A row with no rule and no hint goes to `Uncategorized Expense` (money out) or
   `Uncategorized Income` (money in), creating them on first use.
6. A transfer: checking file row −500.00 and savings file row +500.00 on the same date,
   with the checking row's other side set to Savings, produce one transaction in total
   across the two batches.
7. `import.commit` with an unresolved section is rejected.
8. Undo of a commit restores the book exactly (property: commit then undo equals the
   pre-commit state, including matched splits' reconcile flags).
9. A section in EUR targeting a USD account is rejected.
10. Watch folder: a new `.qif` dropped in is staged and moved to `imported/`; a
    corrupt file is moved to `failed/` with an error file; with `auto_commit` false
    nothing is committed.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-10-22) — transaction dates and date corrections

Owner request: imported transactions must carry the date the bank's register shows,
which is the transaction date, not the posting date (`app-desktop.md` revision 21).

### Raw row

- `date` is the **transaction date** when the source gives one, else the posting date.
- New field `date_kind`: `transaction` (the source labelled it a transaction/user
  date), `posted` (the source labelled it a posting date) or `unknown` (a single,
  unlabelled date, e.g. QIF). Set by each format's reader.

### Setting

| Module | Key | Type | Default |
|---|---|---|---|
| core | `import.date_source` | `transaction` / `posted` | `transaction` |

`transaction` = use the transaction date when the source gives one; `posted` = always
use the posting date. Read by the OFX reader and the CSV profile suggestion; bank
feeds keep their own setting (`connectivity.md`).

### Date corrections

A row that would be classified **duplicate** by an `ext:` fingerprint, whose
`date_kind` is `transaction` and whose `date` differs from the existing transaction's
date, is classified **date correction** instead when all of these hold:

1. The existing transaction's date still equals the date it was given when it was
   imported (recorded on the batch row that created or matched it) — the user has not
   changed it.
2. None of its splits is reconciled.
3. Neither the old nor the new date is on or before the book's closing date.

Otherwise it stays a duplicate, and review shows the note "Date differs from the
bank (<date>); left unchanged because <it was edited | it is reconciled | the period
is closed>".

Commit of a date correction changes only the transaction's date, through the normal
audited update (Undo restores the old date), and records the new date as the
imported date. A batch containing only duplicates, matches, rule-categorised rows and
date corrections may still auto-commit. Content (`hash:`) fingerprints include the
date, so rows without an external id are never date corrections.

### Acceptance tests

13. An OFX file committed with posting dates, then re-staged with `DTUSER` two days
    earlier on each row (same FITIDs): every row is a date correction; commit moves
    the dates; the audit log has one update per transaction; Undo restores them.
14. The same, but one transaction's date was edited by the user after import and
    another has a reconciled split: both stay duplicates with the matching note.
15. A re-staged row with `date_kind` `posted` and a different date stays a plain
    duplicate.
