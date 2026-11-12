# Clean-room protocol

This protocol governs the entire MoneyKeep project: every phase, every contributor,
human or AI agent. A change that conflicts with it is rejected in review, whatever its
technical merit.

## Roles

**Spec writers** may study the target products (commercial finance products) and prior-art
projects by using them, reading their public documentation, and reading published
format specifications. They write behaviour-only specs in `docs/specs/`: inputs,
outputs, rules, edge cases. Specs never contain code, pseudo-code transcribed from
source, internal identifiers, UI text, screenshots or layouts copied from a target.

**Implementers** write MoneyKeep code from `docs/specs/`, public format standards and
general accounting knowledge only. Implementers never read the source code of the
target products or of any prior-art project listed below, and never decompile,
disassemble or inspect target binaries or data files beyond their documented, public
formats.

One person or agent session does not act in both roles for the same feature area. A
contributor who has read a prior-art project's source is a spec writer for that area
and does not implement it.

## Permitted inputs

- Public standards and specs: OFX, QIF, IIF, ISO 20022, BAI2, MT940, NACHA, FDX,
  ISO 4217, XBRL, Beancount and hledger file-format documentation.
- Vendor API documentation for integrations (SimpleFIN, SnapTrade, Plaid, Stripe, …).
- Accounting textbooks, GAAP and IRS publications, public tax-rate data.
- Observed behaviour of target products used under their normal licence terms,
  recorded as behaviour, not as code or copied UI.
- Sample data files the team created, or files whose licence permits use as fixtures.

## Forbidden inputs

- Source code of commercial finance products or any proprietary product, including leaked code.
- Source code of any prior-art project below, whatever its licence. Prior art is studied
  only through documentation, published formats and behaviour. No code is copied,
  translated or ported.
- Decompilation, disassembly or reverse engineering of target binaries or their
  proprietary file formats. Migration goes only through the vendors' export formats.
- Trademarks of commercial finance products, or their icons, artwork, report layouts,
  help text or UI wording.
- Browser automation or scraping of bank websites.
- Any file a contributor cannot trace to a permitted source.

## Prior art (behaviour reference only — never read the source)

| Project | Licence (verified 2026-10-18) |
|---|---|
| rustledger/rustledger | GPL-3.0 |
| DrCr | not verified |
| ebcrowder/rust_ledger | GPL-3.0 |
| VonHoltenCodes/SlowBooks-Pro-2026 | Source Available License 2.0 |
| frappe/books | AGPL-3.0 |
| bigcapitalhq/bigcapital | AGPL-3.0 |
| akaunting/akaunting | Business Source License |
| ledgersmb/LedgerSMB | GPL-2.0 |
| frappe/erpnext | GPL-3.0 |
| odoo/odoo | LGPL-3.0 |
| Gnucash/gnucash | GPL-2.0 or GPL-3.0 |
| KDE/kmymoney | GPL family (per-file SPDX) |
| actualbudget/actual | MIT |
| firefly-iii/firefly-iii | AGPL-3.0 |
| ghostfolio/ghostfolio | AGPL-3.0 |
| maybe-finance/maybe | AGPL-3.0 (archived) |
| beancount/beancount | GPL-2.0 |
| beancount/fava | MIT |
| simonmichael/hledger | GPL-3.0 |
| ledger/ledger | BSD-3-Clause |
| woob, finance-dl (bank scrapers) | out of scope |

## Data sources and formats considered (2026-10-20)

Reviewed so MoneyKeep covers the reasonable formats for financial transactions.
Decisions are by licence first, then by usefulness.

| Source | Licence / terms | Decision |
|---|---|---|
| OFX 2.2 (FDX) | Published specification, free download | **Used** (bank, card, investment import) |
| QIF | Public descriptions only (no owner) | **Used** |
| CSV (RFC 4180) | Open standard | **Used** with institution profiles |
| Broker CSV/JSON/XML (Schwab samples) | Owner's own anonymised files, format reference only, not committed | **Used** for built-in profiles; fixtures are invented |
| ISO 20022 `camt.053` / `camt.052` (bank-to-customer statements, XML) | Message definitions freely published by ISO 20022 RA | **Planned** importer (Phase 7); common outside the US |
| SWIFT MT940 / MT942 | Format owned by SWIFT; full handbook is member-only, public descriptions exist | **Planned** importer (Phase 7) from public descriptions only; no member material |
| FDX API (JSON) | Membership terms for full spec | **Deferred** to Phase 5 connectivity, subject to licence review |
| Kaggle "Financial Transactions" / "USA Banking Transactions" datasets | No licence stated | **Not used** (owner decision 2026-10-20): no licence means no right to copy; formats are generic CSV already covered |
| Other public datasets | Only if CC0 or CC-BY, credited in `ATTRIBUTION.md` | Case by case; none adopted yet |

## Third-party crates

Crates are used as dependencies through Cargo only, and only when their licence passes
the allowlist in `deny.toml` (MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0,
MPL-2.0 unmodified). `cargo xtask ci` enforces this when `cargo-deny` is installed.
Crates are never vendored with changes copied from forbidden sources.

## AI agents

Agents take the same roles. `AGENTS.md` and `CLAUDE.md` state the protocol first.
Implementer agents receive only `docs/specs/`, public standards and this repository as
context; they are never pointed at, given links to, or asked to browse prior-art or
target source code. A spec-writer agent session that reads prior-art material commits
only to `docs/specs/`.

## Provenance records

- Every spec lists its sources (template: `docs/specs/TEMPLATE.md`).
- Every pull request completes `.github/pull_request_template.md`; a "yes" to the
  forbidden-input question blocks merge.
- Test fixtures record their origin in `examples/README.md`.

## Role roster

| Area | Spec writer(s) | Implementer(s) |
|---|---|---|
| Phase 0 scaffold | n/a (no target behaviour) | Claude session, 2026-10-18 |
| Phase 1 ledger core (`ledger-accounts`, `ledger-transactions`, `ledger-balances`, `ledger-audit-undo`, `mcp-server`) | Claude session 2026-10-18 (spec-writer; sources: open accounting textbook, ISO 4217, MCP spec; no target observation, no prior-art source) | Claude implementer subagent session, 2026-10-18 (inputs: docs/specs only + public standards) |
| Phase 2a import (`import-pipeline`, `import-qif`, `import-csv`, `import-rules`, `reconciliation`, `import-ofx`) | Claude session 2026-10-18 (spec-writer; sources: QIF public descriptions (Wikipedia, the vendor's 1997 FAQ; cited by title in `import-qif.md`), RFC 4180, OFX 2.2 specification (FDX), open accounting textbook; no product observed, no implementation source read) | Claude implementer subagent session (inputs: docs/specs only + public standards) |
| Phase 2c desktop app (`ledger-register`, `app-desktop`) | Claude session 2026-10-18 (spec-writer; sources: egui/eframe/egui_kittest documentation, open accounting textbook, MoneyKeep's own design; no target product screens, screenshots or terminology consulted) | Claude implementer subagent session (inputs: docs/specs only + public library docs) |
| Phase 2b scheduling, budgets, reports (`scheduled-transactions`, `budgets`, `reports`, `app-desktop` rev. 4) | Claude session 2026-10-19 (spec-writer; sources: ISO 8601 calendar, OpenStax accounting and managerial accounting textbooks, RFC 4180, MoneyKeep's own design; no target product observed) | Claude implementer subagent session (inputs: docs/specs only + public library docs) |
| Phase 2d encryption at rest (`book-encryption`, `app-desktop` rev. 6) | Claude session 2026-10-19 (spec-writer; sources: RFC 8439, XChaCha CFRG draft, RFC 9106, OWASP Password Storage Cheat Sheet, SQLite serialize docs, libsqlite3-sys docs; no target product observed) | Claude implementer subagent session (inputs: docs/specs only + public library docs) |
| Display customisation (`app-desktop` rev. 7) | Claude session 2026-10-19 (spec-writer; sources: egui/eframe docs, fontdb docs, WCAG 2.2 SC 1.4.4; owner request) | Claude implementer subagent session (inputs: docs/specs only + public library docs) |
| Phase 3a investments (`investments-core`, `app-desktop` rev. 10) | Claude session 2026-10-20 (spec-writer; sources: IRS Publications 550 and 551, IRS Form 8949 instructions, OpenStax accounting textbook; no target product observed) | Claude implementer subagent session (inputs: docs/specs only + public library docs) |
| Phase 3b investment import (`import-investments`, `app-desktop` rev. 11) | Claude session 2026-10-20 (spec-writer; sources: OFX 2.2 specification ch. 13, RFC 8259, XML 1.0, RFC 4180; owner statement of broker formats; no target product or broker files observed yet) | Claude implementer subagent session (inputs: docs/specs only + public standards) |
| Definitions and input safety (`definition-files`, `input-safety`, `app-desktop` rev. 12) | Claude session 2026-10-20 (spec-writer; sources: TOML 1.0, SemVer, SPDX, Unicode UAX #15 / UTS #39 / UTS #55, OWASP CSV injection, XXE and LLM prompt-injection guidance, ECMA-48, regex crate docs; owner request) | Claude implementer subagent session (inputs: docs/specs only + public standards) |
| Phase 3c performance, wash sales, corporate actions (`investments-advanced`, `app-desktop` rev. 13) | Claude session 2026-10-20 (spec-writer; sources: IRS Publication 550, Form 8949 instructions, Form 8937, CFA GIPS TWR / Modified Dietz, OpenStax Principles of Finance, rust_decimal docs; no target product observed) | Claude implementer subagent session (inputs: docs/specs only + public standards) |
| Phase 4a business sales (`business-sales`, `app-desktop` rev. 14) | Claude session 2026-10-20 (spec-writer; sources: OpenStax Principles of Accounting (receivables, aging, sales tax), ISO 32000-1 standard fonts; owner plan request; no target product observed) | Claude implementer subagent session (inputs: docs/specs only + public standards) |
| Phase 4b business purchases (`business-purchases`, `app-desktop` rev. 15) | Claude session 2026-10-21 (spec-writer; sources: OpenStax Principles of Accounting, IRS Instructions for Forms 1099-MISC and 1099-NEC, NATP article on the 2026 $2,000 threshold; no target product observed) | Claude implementer subagent session (inputs: docs/specs only + public standards) |
| Phase 4c dimensions, nonprofit reports, statements, templates, IIF (`business-dimensions`, `app-desktop` rev. 16) | Claude session 2026-10-21 (spec-writer; sources: OpenStax, FASB ASU 2016-14 summaries (Deloitte, CPA Journal), the vendor's public IIF overview article (cited by title in `business-dimensions.md`), ISO 32000-1 image XObjects; no target product observed; no import kit or product files consulted) | Claude implementer subagent session (inputs: docs/specs only + public standards) |
| Phase 5a connectivity and SimpleFIN (`connectivity`, `app-desktop` rev. 17) | Claude session 2026-10-21 (spec-writer; sources: SimpleFIN protocol v2 and v1, RFC 9110/7617/8446, keyring crate docs; no target product observed) | Claude implementer subagent session (inputs: docs/specs only + public standards) |
| Phase 5b payment processors (`connectivity-processors`, `app-desktop` rev. 18) | Claude session 2026-10-21 (spec-writer; sources: Stripe, Square and PayPal public API references (endpoint details left to the implementer from those references), OpenStax; no target product observed) | Claude implementer subagent session (inputs: docs/specs + vendor API documentation) |
| Phase 5c OFX Direct Connect and SnapTrade (`connectivity-brokerage`, `app-desktop` rev. 19) | Claude session 2026-10-21 (spec-writer; sources: OFX 2.2 specification, SnapTrade public documentation read 2026-10-21; no target product or prior-art library observed — third-party Schwab and IBKR client libraries found in searches were deliberately not opened) | Claude implementer subagent session (inputs: docs/specs + OFX 2.2 + SnapTrade documentation) |
| Date format and transaction dates (`app-desktop` rev. 20–21, `import-pipeline` rev. 2, `import-ofx` rev. 3, `import-csv` rev. 2, `connectivity` rev. 3) | Claude session 2026-10-22 (spec-writer; sources: ISO 8601, Unicode CLDR English month names, OFX 2.2 §11.4.4.1 (`DTUSER`), SimpleFIN v2 (`transacted_at`); owner request and owner's report about card downloads; no target product observed) | Claude implementer subagent session (inputs: docs/specs + public standards) |
| Report date headings and accounts slide-out pane (`app-desktop` rev. 22–23) | Claude session 2026-10-22 (spec-writer; sources: egui docs, WCAG 2.2 SC 1.4.13 and 2.1.1; owner request; no target product observed) | Claude implementer subagent session (inputs: docs/specs + public library docs) |
| Pop-up editing, fast entry and New button (`app-desktop` rev. 24) | Claude session 2026-10-22 (spec-writer; sources: owner request and defect report, MoneyKeep's own specs and code; no target product observed) | Claude implementer subagent session (inputs: docs/specs + public library docs) |
| Compact top bar, calculator, Features menu (`app-desktop` rev. 25) | Claude session 2026-10-22 (spec-writer; sources: egui/eframe docs, rust_decimal docs, WCAG 2.2; owner request; no target product observed) | Claude implementer subagent session (inputs: docs/specs + public library docs) |
| Book tiles and book profile (`app-desktop` rev. 26) | Claude session 2026-10-22 (spec-writer; sources: egui docs, PNG/JPEG specs, WCAG 2.2; owner request; no target product observed) | Claude implementer subagent session (inputs: docs/specs + public library docs) |
| Sort order, scroll bars, pop-up buttons (`app-desktop` rev. 27) | Claude session 2026-10-22 (spec-writer; sources: egui docs, WCAG 2.2; owner request; no target product observed) | Claude implementer subagent session (inputs: docs/specs + public library docs) |
| Phase 6a expenses and receipts, 6b phone capture (`expenses`, `mobile-capture`, `app-desktop` rev. 28–29, `mobile-capture` rev. 2 incl. Cloudflare API docs read 2026-10-26) | Claude session 2026-10-23 (spec-writer; sources: IRS Publication 463 (record fields only), PNG/JPEG/PDF specs, Cloudflare Tunnel/Access/Pages documentation (CORS and JWT validation pages read 2026-10-23), HTML Living Standard, RFC 9110/6750/7519/7515/8259, CSP 3, eframe/wasm-bindgen/web-sys/Trunk docs; owner request; no target product or expense app observed) | Claude implementer subagent session (inputs: docs/specs + public standards and vendor docs) |
| Dashboards (`dashboards`, `app-desktop` rev. 31) | Claude session 2026-10-28 (spec-writer; sources: egui docs, WCAG 2.2; owner request; no target product observed) | Claude implementer subagent session (inputs: docs/specs + public library docs) |
| UI toolkit change to Slint (`ui-slint`) | Claude session 2026-11-02 (spec-writer; sources: Slint documentation and licence texts; owner decisions; no target product observed) | Claude implementer subagent session (inputs: docs/specs + Slint documentation) |
| Slint UI revision 2: real `.slint` screens (`ui-slint` rev 2) | Claude session 2026-11-02 (spec-writer; sources: Slint documentation; owner decision; no target product observed) | Claude implementer subagent session (inputs: docs/specs + Slint documentation) |
| Desktop rev 32: one click and minimal password prompt (`app-desktop` rev 32) | Claude session 2026-11-04 (spec-writer; sources: owner report; no target product observed) | Claude implementer subagent session (inputs: docs/specs + Slint documentation) |
| EZ Chart of Accounts (`chart-presets`, `ledger-accounts` rev 2, `reports` rev 4, `app-desktop` rev 34) | Claude session 2026-11-04 (spec-writer; sources: owner-authored chart seed and validation rules, OpenStax, IRS tax year 2025 forms and instructions; no target product observed) | Claude implementer subagent session (inputs: docs/specs incl. the owner seed + IRS forms) |

Record of the Phase 0 implementer session: it read only prior-art licence files and
the structural template project's repository layouts and READMEs (not a target
product). It read no finance project's source code.

## Contamination log

| Date | Area | What was found | Action taken |
|---|---|---|---|
| — | — | none | — |

If contamination is found: stop work on the area, remove the code, record it here, and
re-implement from the spec with a different implementer.

## Limits

This protocol reduces legal risk but is not legal advice. Have a lawyer review it,
along with the end-user licence terms of any commercial finance product used to
observe behaviour, before the first public release.
