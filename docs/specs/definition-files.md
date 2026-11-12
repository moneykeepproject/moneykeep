# Spec: MoneyKeep Definition files (`.mkdef`)

- **Area / module:** definitions (`core`)
- **Spec writer:** Claude session 2026-10-20 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-20
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Standard | TOML v1.0.0 — https://toml.io/en/v1.0.0 | Syntax; comments; typed values; no references, tags or code |
| 2 | Standard | Semantic Versioning 2.0.0 — https://semver.org | Package versions |
| 3 | Standard | SPDX License List — https://spdx.org/licenses/ | `license` field |
| 4 | Standard | Unicode UAX #15 (normalisation), UTS #39 (security mechanisms), UTS #55 / CVE-2021-42574 "Trojan Source" (bidirectional control characters) | Text safety rules |
| 5 | Vendor doc | Rust `regex` crate documentation — https://docs.rs/regex | Linear-time matching, no backreferences or look-around, compile-size limits |
| 6 | Spec | `import-csv.md`, `import-investments.md`, `import-rules.md`, `ledger-accounts.md`, `investments-core.md`, `input-safety.md` | The things a definition can describe; shared safety rules |
| 7 | Owner | Request 2026-10-20: a flexible, future-proof, hand-editable format for definitions that people post and trade, with safety against scripts and malware | |

No target product was observed. The format is MoneyKeep's own.

## Purpose

People want to share the *recipes* that make MoneyKeep work with their institutions
and habits — how to read a broker's export, which payees go to which categories, a
starter category tree, a list of securities — without sharing any financial records.
A definition file packages those recipes so they can be posted, traded, edited by
hand, installed, updated and removed safely.

## Design decisions

- **TOML 1.0, UTF-8, extension `.mkdef`.** Chosen for hand editing (comments, no
  brace or comma bookkeeping, clear types for dates, integers and strings), a strict
  and small specification, and the absence of anything executable: TOML has no
  references, anchors, tags, includes or expression syntax (unlike YAML), and allows
  comments (unlike JSON). The parser is a pure-Rust TOML crate (MIT/Apache).
- **Declarative only.** A definition describes data. Nothing in it is ever executed,
  evaluated, templated, fetched, opened as a path, or passed to a shell. The only
  "pattern language" is the linear-time regex engine already used by rules, with
  size limits. There is no plug-in, macro or formula mechanism, and the format
  reserves no place for one.
- **Definitions never contain or create financial records.** Installing one cannot
  create, change or delete accounts' balances, transactions, prices, budgets or
  settings outside the item kinds below. Category *accounts* created by a category
  definition start empty.
- **Explicit versioning with forward compatibility.** Files state their format
  version; readers ignore unknown keys with a warning, except when the file declares
  it *requires* a feature the reader lacks, in which case it is refused with a clear
  message.
- **Nothing installs silently.** Every install goes through inspect → review →
  install, is audited, and is undoable as one change.

## File structure

```toml
# MoneyKeep Definition — comments are allowed anywhere.
moneykeep_definition = 1            # format version (integer, required)
id = "org.example.schwab-imports"     # package id (required)
version = "1.2.0"                     # package version, SemVer (required)
name = "Charles Schwab import layouts"   # required, 1–100 chars
description = """
Reads Schwab CSV, JSON and XML transaction downloads.
"""                                   # optional, ≤ 4,000 chars
authors = ["Jane Doe"]                # optional, ≤ 10 entries of ≤ 100 chars
license = "CC0-1.0"                   # optional SPDX id or expression
homepage = "https://example.org/mkdef" # optional, display-only text
created = 2026-10-20                  # optional TOML date
requires = []                         # optional feature names the reader must know
tags = ["broker", "usa"]              # optional, ≤ 20 tags of ≤ 30 chars

[[investment_profile]]                # zero or more items of each kind
key = "schwab-json"                   # item key, unique within the package
# … fields as in import-investments.md …

[[rule]]
key = "coffee"
# …

[x_example]                           # extension tables: any key starting "x_"
note = "ignored by MoneyKeep"
```

### Header fields

| Key | Rules |
|---|---|
| `moneykeep_definition` | Integer. This spec defines **1**. A higher number is refused ("made for a newer MoneyKeep") unless only additive (see Compatibility) |
| `id` | 3–100 chars: lowercase letters, digits, `.`, `-`, `_`; at least one `.`; starts with a letter. Identifies the package across versions (reverse domain style recommended, not checked) |
| `version` | SemVer 2.0.0 string |
| `name`, `description`, `authors`, `tags` | Plain text, subject to the text rules below |
| `license` | SPDX license id or expression, checked for syntax only |
| `homepage` | Text that must look like an `https://` URL of ≤ 200 chars. Shown as **plain, selectable text** with a copy button; MoneyKeep never opens, fetches or makes it clickable |
| `created` | TOML local date |
| `requires` | List of feature names (see Compatibility) |

### Item kinds (format version 1)

Each kind is an array of tables. Every item has a `key` (1–64 chars: letters,
digits, `-`, `_`), unique per kind within the package. Fields are exactly those of the
existing MoneyKeep object, written as TOML values, except that **references to other
items or to accounts use names, never internal ids**:

| Table | Describes | Fields (summary) | References |
|---|---|---|---|
| `[[bank_profile]]` | A bank CSV profile (`import-csv.md`) | name, delimiter, header row, column roles, date format, amount mode, decimal comma, signature | — |
| `[[investment_profile]]` | A broker profile (`import-investments.md`) | name, format, signature, records, fields, date/number formats, action rules (with `unverified` flags), account-from-file-name, security rules, total field | — |
| `[[category]]` | A category account in a tree (`ledger-accounts.md`) | `path` (full name with `:` separators, e.g. `"Expenses:Food:Groceries"`), `type` (`income`/`expense`), optional `description`, `code` | Parents are implied by the path |
| `[[rule]]` | A categorisation rule (`import-rules.md`) | name, priority (relative within the package), conditions, actions, stop | `set_account` / `split` name a category `path` (from this package or the book) |
| `[[security]]` | A security definition (`investments-core.md`) | symbol, name, kind, basis method, CUSIP | — |

Kinds that are not listed are unknown to version 1 (see Compatibility). Future kinds
already anticipated (not part of this spec): `report`, `budget_template`,
`schedule_template`, `module_preset`.

### Text rules (all string values)

These apply on top of TOML's own rules; violations make the file invalid (with the
key path in the error) unless marked "warning":

- Must be valid UTF-8 (TOML requires it); normalised to NFC on reading.
- No C0/C1 control characters except tab and line feed, and those only in
  `description` and `notes` fields.
- No bidirectional formatting characters (U+202A–U+202E, U+2066–U+2069, U+200E,
  U+200F, U+061C) anywhere; no zero-width or invisible characters (U+200B–U+200D,
  U+2060, U+FEFF, U+180E) in names, keys, paths, symbols and match values (source 4).
- Names, symbols and paths that mix scripts in a way UTS #39 calls "mixed-script
  confusable" (e.g. Latin with Cyrillic look-alikes) give a **warning** shown in the
  review.
- Length limits: names 100, descriptions 4,000, match values and patterns 200 (regex)
  or 500 (plain), any other string 1,000.

### Size and structure limits

- File ≤ 1 MiB; ≤ 5,000 items in total; ≤ 500 of any one kind except `category` and
  `rule` (≤ 2,000 each).
- TOML nesting depth ≤ 8; arrays ≤ 1,000 elements; no key longer than 64 characters.
- Every regex is compiled with a compiled-size limit of 1 MiB and a nesting limit of
  50 before the file is accepted; failures make the file invalid.
- Parsing and validation are bounded in time by these limits (no backtracking
  anywhere); a test asserts a 1 MiB adversarial file validates in under one second.

### Compatibility

- **Unknown keys** in known tables, unknown top-level keys, and unknown item kinds:
  ignored with one warning each ("ignored: unknown key `foo` in rule `coffee`"), so
  newer files still install their known parts in older versions.
- **Extension tables** whose names start with `x_` are ignored silently and kept when
  the package is re-exported unchanged.
- **`requires`** lists feature names that change meaning rather than add (e.g.
  `"rule-condition-any"`). A reader refuses a file listing a feature it does not
  implement, naming it. Version 1 readers implement no optional features; the list
  exists for the future.
- A future format version 2 must be readable as "version 1 plus additions" or must
  bump `moneykeep_definition`; readers refuse higher versions they do not know.

## Behaviour

### Commands

| Command | Mutates | Notes |
|---|---|---|
| `definition.inspect` | no | {`content_base64` or `path`, `name`}. Parses and validates. Returns the header, every item with its proposed action (`add`, `update` — same package and key installed before, `conflict` — an object of the same kind and name exists from elsewhere, `unchanged`), warnings, errors, the file's SHA-256, and a plain summary ("Adds 3 broker profiles and 12 rules; updates 1 rule; 2 warnings"). Invalid files return every error found (up to 100) and nothing else |
| `definition.install` | yes | {inspect inputs, `choices`: per item `install` / `skip`, and for conflicts `replace` / `rename` (new name) / `skip`}. Re-validates (the file must have the same SHA-256 as inspected), then installs the chosen items in **one** audited command: categories first (parents created as needed, placeholder parents allowed), then securities, profiles, rules. Undo removes or restores everything it changed |
| `definition.list` | no | Installed packages: id, version, name, file SHA-256, install date, item counts, and items changed locally since install |
| `definition.uninstall` | yes | {`id`}. Removes items installed by the package that were not modified locally; modified ones are kept and listed. Categories with transactions are never removed (they are kept and listed). One audited command |
| `definition.export` | no | {`id`, `version`, `name`, header fields, and selections: lists of bank profiles, investment profiles, rules, categories (subtrees), securities by name}. Returns `{file_name, content}`: a TOML document with a generated comment header, keys in a fixed order, items sorted by key, plain multi-line strings for long text. Account references become category paths; rules naming non-category accounts (e.g. a specific bank account) are refused with a list of the offending rules. Exports never include balances, transactions, payee history, prices, or any account number or file-name account key |
| `definition.validate` | no | Validation only, for authors; same errors and warnings as inspect |

### Provenance

Installed items record the package id, version and file SHA-256 they came from, and
whether they were edited since. Item lists in the UI show a small "from <package
name>" note. Updating a package (same id, newer version) offers `update` for changed
items and leaves locally edited items alone unless the user chooses to replace them.

### Trust model

- Every installed package is treated as **untrusted input**, permanently: its
  profiles and rules run only through the same validated, linear-time, side-effect-free
  code paths as user-made ones, with all limits of `input-safety.md` applied on every
  use, not only at install.
- The worst a malicious definition can do is mis-categorise or mis-read imported
  rows. Every effect still passes through import **review** before reaching the books,
  and every install and import is undoable.
- Signatures are not part of version 1. The header reserves the table name
  `[signature]` for a future version (an Ed25519 signature over the canonical file
  content and the signer's public key); version 1 readers ignore it with a warning
  "signature not checked".

## Ledger effect

Category definitions create empty accounts. Nothing else touches the ledger.

## Settings

None.

## Edge cases

- A rule referencing a category path that neither the package nor the book has:
  the rule is installed **disabled**, with a note; the review lists it.
- A category path whose type conflicts with an existing account at that path (e.g.
  `Expenses:Gifts` exists as income): conflict, default skip.
- Two packages define rules with the same name: allowed; names are not unique for
  rules. Profiles and securities must have unique names (conflict handling applies).
- Installing the same file twice: every item `unchanged`; install does nothing.
- A file with Windows line endings, a UTF-8 BOM, or tabs: accepted.

## Acceptance tests

1. A hand-written file with one investment profile, two categories and one rule
   referencing a category path inspects with "Adds 1 broker profile, 2 categories and
   1 rule"; installing creates them; the rule's account is the created category; undo
   removes all three.
2. Exporting the Schwab built-in profiles plus one rule produces a file that
   `definition.validate` accepts and that re-inspects as `unchanged` against the book
   it came from.
3. A file containing a string with U+202E is invalid, naming the key; one with a
   zero-width space in a rule match value is invalid; one with a Cyrillic "а" in a
   payee match gives a warning only.
4. A file larger than 1 MiB, a file nesting deeper than 8, and a file with 2,001
   rules are each rejected with the limit named.
5. A regex `(a+)+$` is accepted (the engine is linear-time) and matching a
   100,000-character adversarial payee completes in under 100 ms; a regex exceeding the
   compiled-size limit is rejected.
6. Unknown key `colour` in a rule and an unknown kind `[[report]]` produce two
   warnings and install the rest; `requires = ["rule-condition-any"]` is refused naming
   the feature.
7. `homepage = "javascript:alert(1)"` is invalid (not https); a valid https homepage is
   returned as text only.
8. Installing a package, editing one of its rules, then uninstalling removes the
   unedited items and keeps (and lists) the edited rule.
9. `definition.install` with a file whose bytes differ from the inspected SHA-256 is
   refused.
10. An export never contains an account number, a balance, a transaction, or a
    file-name account key (checked by scanning the output for the test book's values).

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision (2026-10-28) — dashboard item kind

`[[dashboard]]` joins the item kinds (`dashboards.md`, Sharing): name, optional icon,
blocks with type, title, settings and layout; account and dashboard references by full
name; a block's `text` may be multi-line up to 2,000 characters; shortcut button
targets are flat fields of the button (nesting limit). No amounts or transactions.
