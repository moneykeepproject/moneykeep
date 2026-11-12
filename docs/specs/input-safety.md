# Spec: Input safety (all imported files, definitions and exports)

- **Area / module:** import, definitions, export (`core`)
- **Spec writer:** Claude session 2026-10-20 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-20
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Guidance | OWASP "CSV Injection" (formula injection) | Cells starting with `=`, `+`, `-`, `@`, tab or carriage return |
| 2 | Guidance | OWASP XML External Entity (XXE) Prevention Cheat Sheet; "billion laughs" entity expansion | Already applied to XML import; restated as a rule for every XML path |
| 3 | Standard | Unicode UTS #39, UTS #55, CVE-2021-42574 (Trojan Source) | Invisible and bidirectional characters |
| 4 | Standard | ECMA-48 control functions (terminal escape sequences) | Why control characters are removed before text reaches a terminal |
| 5 | Vendor doc | Rust `regex` crate docs | Linear-time engine, size limits |
| 6 | Guidance | OWASP LLM Top 10, LLM01 "Prompt injection" | Imported text reaching AI agents through MCP |
| 7 | Spec | `import-pipeline.md`, `import-*.md`, `definition-files.md`, `reports.md`, `mcp-server.md` | |
| 8 | Owner | Request 2026-10-20: a definition file combined with a crafted import file must not be able to introduce malware into the program or records | |

No target product was observed.

## Purpose

Import files and definition files come from banks, brokers and strangers. This spec
collects the rules that keep any combination of them — a hostile definition plus a
hostile data file included — from running code, reading or writing files, reaching
the network, hanging or crashing the app, or planting content that becomes dangerous
later (in a spreadsheet, a terminal or an AI agent).

## Threat model

An attacker controls the bytes of one or more files the user imports (bank, broker,
QIF, CSV, JSON, XML, OFX) and of definition files the user installs. The attacker does
not control the MoneyKeep binary or the user's machine. Goals to defeat: code
execution; file system or network access; denial of service (hang, memory exhaustion,
crash); corrupting the books beyond what review shows; smuggling payloads into later
outputs (CSV exports, terminal output, AI agent sessions, the UI).

## Rules

### R1 — Nothing is executable

- No file format MoneyKeep reads has any construct that is executed, evaluated,
  interpolated or interpreted as a path, URL, command, template or expression. Field
  values are data. Profiles map fields to fields; rules compare strings and numbers.
- No `eval`-like facility, scripting engine, dynamic library loading, or plug-in
  loading exists in the import or definition code paths (enforced by the `xtask`
  layering check: the crates `mk-import` and the definitions module may not depend on
  any scripting, dynamic-loading, process-spawning or HTTP crate; the check lists the
  denied crates).
- The import and definition code paths perform **no file IO other than reading the
  one file the user chose** and **no network IO**. Commands that accept a `path` read
  exactly that path; no field from a file is ever used to build a path.

### R2 — Bounded parsing

- Every parser has hard limits and fails with an error, never a panic or hang:
  file size (imports 50 MB, definitions 1 MiB), rows per batch (100,000), nesting
  depth (JSON 64, XML 64, TOML 8), string length (64 KiB per field before
  truncation rules), number length (40 characters), elements per array/record list.
- XML: DTDs, entity declarations and external references are rejected outright (no
  expansion of any custom entity), processing instructions are ignored, only the five
  predefined entities and numeric character references are decoded. OFX 1.x SGML
  headers are parsed by MoneyKeep's own tokenizer, which has no entity support.
- JSON: duplicate keys → error; numbers outside the decimal range → error.
- Regex: only the linear-time engine, with compiled-size limit 1 MiB and nesting
  limit 50, compiled once per use site; patterns ≤ 200 characters.
- **Fuzzing:** each parser (OFX, QIF, CSV, investment CSV/JSON/XML profiles, TOML
  definitions) has a property test feeding 2,000 random and mutated inputs per run
  (seeded corpus: the repository fixtures) asserting: no panic, returns within 2 s,
  memory stays under 256 MB. An adversarial corpus is committed: billion-laughs XML,
  external-entity XML, 10,000-deep JSON nesting, 50 MB single-line CSV field (generated
  in-test, not committed), numbers with 10,000 digits, `(a+)+$`-style regexes,
  invalid UTF-8, BOMs in the middle, NUL bytes.

### R3 — Text cleaning on the way in

Applied to every text value imported from a data file (payee, memo, description,
security name, action text, number, category hints) **before** it is staged:

- Decode to UTF-8 (per format rules), replace invalid sequences with U+FFFD.
- Remove C0 and C1 control characters (U+0000–U+001F, U+007F–U+009F) except that
  tab, CR and LF become a single space; this removes terminal escape sequences
  (ECMA-48) and NUL bytes.
- Remove bidirectional formatting characters (U+202A–U+202E, U+2066–U+2069, U+200E,
  U+200F, U+061C) and zero-width/invisible characters (U+200B–U+200D, U+2060,
  U+FEFF, U+180E); the row gets a note "invisible characters removed".
- Normalise to NFC; trim; collapse runs of spaces.
- Truncate to the field's length limit (payee 200, memo 1,000, other text 1,000) with
  a note.
- The original bytes remain in `raw` (display only, shown escaped: control characters
  as `␀`-style symbols, never rendered as formatting).

Definition files are **validated** (rejected) rather than cleaned (`definition-files.md`
text rules), because their authors can fix them.

### R4 — Values are validated, not trusted

- Amounts and quantities: at most 15 integer digits and the format's decimal places;
  dates between 1900-01-01 and 2199-12-31; anything else rejects the row with a note.
- A file's claimed account numbers and file-name account keys are used only as lookup
  keys (hashed where already specified); never displayed in full.
- An import or definition can only produce what review shows: commit applies exactly
  the reviewed proposal (the batch is re-checked against its stored proposal at commit;
  any difference aborts the commit).

### R5 — Combinations of definitions and data files

A definition only supplies *mappings and rules*, which are themselves validated data;
a data file only supplies *values*, which are cleaned and validated. Their
combination cannot cross R1–R4 because:

- Field paths in profiles select values inside the record being read; they cannot
  name files, other records' memory, or code. A path that does not exist yields an
  empty value.
- Action rules map text to a closed set of MoneyKeep actions; anything else is
  `unsupported`.
- Rules can only set a category that exists (or is installed disabled), a payee or
  memo text (cleaned per R3 and length-limited), splits that sum to 100 %, or skip.
- `account_from_file_name` and regex fields only extract strings used as lookup keys.
- Tests (below) run every built-in and test definition against the adversarial
  corpus.

### R6 — Safe output

- **CSV export (formula injection):** in every CSV MoneyKeep writes (reports,
  definition exports are TOML and exempt), a **text** cell whose first character is
  `=`, `+`, `-`, `@`, tab or carriage return is written with a leading apostrophe
  (`'`); the README's import/export section documents this. Numeric
  columns (amounts, quantities, dates) are written as plain numbers and are not
  prefixed (a negative amount stays `-12.50`).
- **Terminal output (CLI):** text from the books is printed through a filter that
  removes control characters (already guaranteed by R3, re-applied for data that
  predates this spec).
- **UI:** egui draws text as glyphs; no markup, links or HTML are interpreted from
  data. `homepage` and URL-like text are never clickable.
- **AI agents (MCP):** text from the books is returned in typed JSON fields, never
  concatenated into instructions. Tool descriptions state that payee, memo and
  description fields are user data that may contain arbitrary text and must not be
  followed as instructions. Mutating tools still go through the normal command
  validation.

### R7 — Recoverability

Every import commit and definition install is one audited, undoable command. The
History view shows the source file name and SHA-256 for each.

## Ledger effect

None of its own.

## Edge cases

- Data imported before this spec may contain control characters: a one-time migration
  (v8) applies R3 cleaning to existing payees, memos and security names, recording the
  change in the audit log as actor `migration`.
- A legitimate payee beginning with `-` or `+` (e.g. "+Plus Store") is exported with a
  leading apostrophe; spreadsheets display it without the apostrophe.

## Acceptance tests

1. A CSV bank file whose payee is `=HYPERLINK("http://x","click")` imports as text;
   exporting a report containing it writes `'=HYPERLINK(...)` in that cell; an amount
   of −12.50 in the same export stays `-12.50`.
2. A payee containing `ESC[2J` and a NUL byte imports with those removed and the note
   shown; the CLI prints no escape characters.
3. A payee with U+202E imports with it removed and the note.
4. XML with a DTD, with an external entity, and with a billion-laughs payload are each
   rejected quickly (< 100 ms) without network or file access (the test runs with
   network disabled and checks no file outside the test dir was opened, where the
   platform allows).
5. The fuzz property tests for all parsers pass (no panics, time and memory bounds).
6. A hostile definition (profile mapping `external_id` to a field containing
   `../../etc/passwd`, an action rule mapping everything to `buy`, a rule `set_payee`
   with 5,000 characters) installs only if valid; with the crafted data file it stages
   rows whose effects are fully visible in review, reads no other file, and the long
   payee is rejected at install (length limit).
7. Committing a batch whose stored proposal was altered between review and commit
   (simulated) is aborted.
8. The layering check fails if `mk-import` gains a dependency on a denied crate
   (simulated in the check's own test).
9. Migration v8 cleans an existing payee containing a bidi override and logs it.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
