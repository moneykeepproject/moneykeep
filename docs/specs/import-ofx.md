# Spec: OFX, QFX and QBO import

- **Area / module:** import (`core`)
- **Spec writer:** Claude session 2026-10-18 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-18
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Standard | *Open Financial Exchange Specification 2.2* (dated 11/26/2017), published by Financial Data Exchange — https://financialdataexchange.org/wp-content/uploads/2025/12/OFX-2.2.pdf (copy supplied by the project owner) | §2.8.1–2.8.3 header and end tags, §3.2.5 TRNAMT, §3.2.8 dates, §3.2.9 amounts and signs, FITID rules (§3.2, "A-255", unique per account), §11.3.1–11.3.2 account aggregates, §11.4.2.2 `<STMTRS>`, §11.4.3 credit card, §11.4.4.1 `<STMTTRN>`, §11.4.4.2 `<STMTTRNP>`, §11.4.4.3 `<TRNTYPE>`, §16.5 sample 1.x file |
| 2 | Public documentation | "Open Financial Exchange", Wikipedia — https://en.wikipedia.org/wiki/Open_Financial_Exchange | QFX/QBO are vendor variants of OFX used for Web Connect downloads |
| 3 | Spec | `import-pipeline.md` | Raw row, sections, fingerprints, statement capture |

No product was run or observed, and no implementation source was read.

## Purpose

OFX is the most common bank and card download format in North America. QFX
(personal-finance downloads) and QBO (small-business downloads) are the same format
with a few vendor-specific additions. All three go through this one importer.

## Behaviour

### Recognising the file

A file is OFX if, after an optional UTF-8 BOM and whitespace, it starts with either:

- an **OFX 1.x header**: lines of `NAME:VALUE` beginning with `OFXHEADER:100`
  (§2.8.2), followed by a blank line and the `<OFX>` body; or
- an **OFX 2.x header**: an optional XML declaration, then a processing instruction
  `<?OFX OFXHEADER="200" … ?>`, then the `<OFX>` element.

Extensions `.ofx`, `.qfx`, `.qbo` select this importer; content detection decides the
version. Anything else is an error ("not an OFX file").

### Character encoding

- 1.x: `ENCODING:USASCII` or `ENCODING:UTF-8`, combined with `CHARSET`:
  `1252` → Windows-1252; `ISO-8859-1` / `8859-1` → Latin-1; `NONE` or missing → treat
  as Windows-1252 (a superset of US-ASCII that real files rely on). `ENCODING:UTF-8`
  wins over `CHARSET`.
- 2.x: the XML declaration's `encoding`, defaulting to UTF-8 (§2.8.3 limits to
  US-ASCII and UTF-8; Windows-1252 and Latin-1 are also accepted).
- Character entities `&amp; &lt; &gt; &quot; &apos; &nbsp;` and numeric references are
  decoded in both versions.

### Parsing the body

- Tag names are matched case-insensitively (§2.4.2 requires upper case; real files
  sometimes deviate).
- **1.x (SGML style):** an *element* (a tag followed by text) may omit its end tag; it
  ends at the next tag. An *aggregate* (a tag followed by other tags) ends at its
  matching end tag, or at the end tag of an enclosing aggregate (tolerating missing end
  tags). End tags present on elements are accepted (§2.8: parsers had to accept them).
- **2.x (XML):** parsed as XML. DOCTYPE declarations, external entities and entity
  definitions are **rejected** (no external resolution ever).
- Unknown elements and aggregates, including vendor extensions such as tags beginning
  with `INTU.` in QFX/QBO files, are ignored.
- Element text is trimmed of leading and trailing whitespace (§3.2.9.1).

### Sections

Each statement response becomes one section:

| Aggregate | Path | Section kind |
|---|---|---|
| `<STMTRS>` | `OFX/BANKMSGSRSV1/STMTTRNRS/STMTRS` | bank |
| `<CCSTMTRS>` | `OFX/CREDITCARDMSGSRSV1/CCSTMTTRNRS/CCSTMTRS` | credit card |

A file may contain several. If the wrapping transaction response's
`<STATUS><CODE>` is not `0`, that section is reported as an error with the code and
any `<MESSAGE>`, and contributes no rows. Investment statements
(`INVSTMTMSGSRSV1`) are skipped in Phase 2a, reported as "skipped: investment
statements (n)". Loan statements are skipped the same way.

### Account identity

- Bank: `BANKID`, `BRANCHID` (optional), `ACCTID`, `ACCTTYPE` from `<BANKACCTFROM>`
  (§11.3.1). Credit card: `ACCTID` from `<CCACCTFROM>` (§11.3.2).
- Institution: `<SONRS><FI><ORG>` and `<FID>` when present.
- Mapping key (for `import_account_map`): `ofx:` + SHA-256 hex of
  `ORG|FID|BANKID|BRANCHID|ACCTID|kind`, with absent parts empty. The book never stores
  the full account number: staging results show only the **last four** characters of
  `ACCTID`, plus `ACCTTYPE` and `ORG`, to help the user pick the account.
- §3.2 notes FITIDs are unique only within an account at one institution; because
  fingerprints already include the resolved MoneyKeep account (pipeline), they are
  globally unique.

### Section currency and statement period

- `CURDEF` is the section currency (pipeline rule: must equal the account commodity).
- `<BANKTRANLIST><DTSTART>` / `<DTEND>` are recorded as the statement period.

### Transactions (`<STMTTRN>`, §11.4.4.1)

| OFX element | Raw row field |
|---|---|
| `FITID` (up to 255 chars) | `external_id` |
| `DTPOSTED` | `date` (see Dates) |
| `TRNAMT` | `amount` (see Amounts). §11.4.4.1: signed from the customer's view, e.g. a card purchase is negative, which is exactly the pipeline convention; never re-signed by `TRNTYPE` |
| `NAME`, or `PAYEE/NAME` | `payee`; if `EXTDNAME` is present it is used instead (it is the longer form) |
| `MEMO` | `memo` |
| `CHECKNUM`, else `REFNUM` | `number` |
| `TRNTYPE` (§11.4.4.3: CREDIT, DEBIT, INT, DIV, FEE, SRVCHG, DEP, ATM, POS, XFER, CHECK, PAYMENT, CASH, DIRECTDEP, DIRECTDEBIT, REPEATPMT, OTHER) | stored on the row as `trntype` and available to rules as a condition field (`trntype equals …`) |
| `SIC` | ignored |
| `CURRENCY` or `ORIGCURRENCY` differing from `CURDEF` | row flagged `needs_review` with reason "foreign-currency transaction"; never auto-committed |

A `<STMTTRN>` without `FITID`, `DTPOSTED` or `TRNAMT` is skipped and listed as a
warning with its position.

### Pending transactions (`<STMTTRNP>`, §11.4.4.2)

Never staged. §11.4.4.2 states pending items have no FITID, may change or vanish, and
must not be stored or used for reconciliation. The staging result reports
"pending transactions not imported (n)".

### Corrections (`CORRECTFITID`, `CORRECTACTION`)

A `<STMTTRN>` with `CORRECTFITID` replaces (`REPLACE`) or deletes (`DELETE`) the
earlier transaction with that FITID in the same account:

- If that FITID is already committed in the target account (fingerprint found):
  - `REPLACE`: the row is classified **correction**. Commit updates the existing
    transaction's date, payee, memo and number, and its amount if it is a two-split
    transaction (both splits change; the other side keeps its account). If the amount
    changes and the transaction has more than two splits, or any of its splits is
    reconciled, the row is `needs_review` and nothing changes automatically. The
    fingerprint moves to the new FITID.
  - `DELETE`: the row is classified **correction-delete**. Commit voids the existing
    transaction with reason "correction from bank". If a split is reconciled, the row
    is `needs_review` instead.
- If that FITID appears earlier in the same file, the earlier row is dropped (for
  `DELETE`) or replaced by this one (for `REPLACE`).
- If it is not found at all: `REPLACE` is treated as a normal new row; `DELETE` is
  ignored with a note.

### Balances (statement capture)

- `<LEDGERBAL><BALAMT>` with its `<DTASOF>` becomes the captured statement balance and
  date (§11.4.2.2). Per §3.2.9.2, statement balances are signed by effect on net worth
  (a card balance owed is negative), which matches MoneyKeep's raw debit-positive
  convention, so it is stored as given.
- `<AVAILBAL>` is shown in the staging result for information but never used for
  reconciliation (it includes pending items, §11.4.4).

### Dates (§3.2.8)

- Format `YYYYMMDD[HHMMSS[.XXX]][ [offset[:tz name]]]`; any right-hand part may be
  missing (§3.2.8.2). The offset may be whole or fractional, with or without sign.
- The row date is the calendar date **as written** (the first eight digits), without
  time-zone conversion. This keeps a transaction on the day the bank shows it.
- Invalid dates (month 13, day 32) reject that transaction with a warning.

### Amounts (§3.2.9.1)

- Leading `+` or `-`; no sign means positive.
- The decimal separator is `.` or `,` (§3.2.9.1 allows either); there are no
  thousands separators.
- No decimal point means an implied point at the end (`550` = 550).
- Up to 32 characters. Precision must still fit the account currency (pipeline).

### QFX and QBO

Identical handling. Vendor-specific elements are ignored. No institution registration
or login data in these files is used or stored.

## Ledger effect

None directly; see `import-pipeline.md`. Corrections update or void existing
transactions only at commit, with audit and undo like any other commit.

## Edge cases

- A 1.x file whose elements carry end tags anyway parses the same as one without.
- A file with both a bank and a credit-card statement yields two sections.
- `TRNTYPE` `DEBIT` with a positive `TRNAMT` (a malformed file): the amount sign is
  trusted, and a warning is recorded.
- Very long FITIDs (up to 255 characters) are accepted.
- An XML 2.x file containing `<!DOCTYPE` with entity declarations is rejected.

## Acceptance tests

Fixtures are synthetic, written by the team, and recorded in `examples/README.md`.

1. A 1.x SGML checking file (header as in §16.5, elements without end tags) with three
   `STMTTRN`s stages one bank section with three rows: correct dates, signed amounts,
   payees, FITIDs; `CURDEF` USD; ledger balance captured.
2. The same content as a 2.x XML file stages identically.
3. A credit-card `CCSTMTRS` file: a purchase `-45.10` and a payment `+200.00` stage as
   money out and money in; the captured balance `-812.40` reconciles against a
   liability account showing 812.40 owed.
4. Re-staging after commit: all rows duplicates (FITID fingerprints).
5. Dates: `20261005`, `20261005120000`, `20261005235959.000[-5:EST]` all give
   2026-10-05; `20261305` rejects that transaction with a warning.
6. Amounts: `550` = 550.00; `-12,34` = −12.34; `+7.5` = 7.50.
7. `CHARSET:1252` with byte 0xE9 in a payee decodes as `é`; a 2.x UTF-8 file decodes
   correctly; `&amp;` in a payee becomes `&`.
8. `STMTTRNP` pending items are not staged and are counted in the result.
9. Corrections: a committed FITID `A1` then a file with `CORRECTFITID A1`,
   `CORRECTACTION REPLACE`, new amount on a two-split transaction updates both splits;
   `DELETE` voids it; with a reconciled split, both become `needs_review` and nothing
   changes.
10. A section whose `STATUS` code is `2003` reports "account not found" and stages no
    rows.
11. Account identity: staging shows only the last four digits of `ACCTID`; the stored
    mapping key is a hash and the full account number appears nowhere in the book file
    (test scans the SQLite file bytes for it).
12. A QFX file with `INTU.BID` and other unknown elements stages like plain OFX.
13. A 2.x file with `<!DOCTYPE ofx [<!ENTITY x SYSTEM "file:///etc/passwd">]>` is
    rejected.
14. A foreign-currency `STMTTRN` (`CURRENCY` EUR in a USD statement) is
    `needs_review` and not committed by auto-commit.
15. `TRNTYPE` is available to rules: a rule `trntype equals FEE` → `Bank Fees`
    categorises a fee row.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-10-19) — memo equal to name

Some institutions send the same text in `<NAME>` and `<MEMO>` (seen in an owner's
real credit-card download). When the memo, trimmed, equals the name ignoring case,
the row's memo is left empty so registers do not show the text twice.

Acceptance test: a `STMTTRN` with `NAME` "ALDI INC" and `MEMO` "ALDI INC" stages
payee "ALDI INC" and an empty memo; `MEMO` "Store 42" is kept.

## Revision 3 (2026-10-22) — transaction date (`DTUSER`)

OFX 2.2 §11.4.4.1 defines `DTUSER` in `<STMTTRN>` as the date the user initiated the
transaction, if known, alongside the required `DTPOSTED` (the date posted). The
owner's credit-card register lists the former, so:

- With core setting `import.date_source` = `transaction` (default): when `DTUSER` is
  present and valid, the row `date` is `DTUSER` and `date_kind` is `transaction`;
  otherwise `date` is `DTPOSTED` and `date_kind` is `posted`. A present but invalid
  `DTUSER` falls back to `DTPOSTED` with a warning naming the transaction.
- With `posted`: `date` is always `DTPOSTED`, `date_kind` `posted`.
- `DTPOSTED` remains required (a `STMTTRN` without it is still skipped).
- Both dates are read as written (Dates section, first eight digits).
- The same applies to `<STMTTRN>` in OFX Direct Connect replies
  (`connectivity-brokerage.md`), credit-card statements (`CCSTMTRS`) and bank
  statements alike. Investment transactions (`INVTRAN`, `DTTRADE`) are unchanged.
- Corrections (`CORRECTACTION REPLACE`) use the same date choice.
- Re-importing an earlier file after this change produces date corrections
  (`import-pipeline.md` revision 2).

Acceptance tests:

16. A `CCSTMTRS` `STMTTRN` with `DTPOSTED 20261020` and `DTUSER 20261018` stages
    date 2026-10-18; with `import.date_source` = `posted`, 2026-10-20.
17. Without `DTUSER`, the row date is `DTPOSTED` and `date_kind` is `posted`.
18. `DTUSER 20261340` falls back to `DTPOSTED` with a warning.
