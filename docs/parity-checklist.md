# Parity checklist

Feature areas MoneyKeep aims to cover, in MoneyKeep's own words and grouping
(clean-room: not copied from any target product's feature lists or menus). Each area
needs a spec in `docs/specs/` before implementation.

Status: `—` not started · `spec` spec written · `wip` in progress · `done` shipped

## Ledger core (Phase 1)

| Area | Module | Status |
|---|---|---|
| Chart of accounts (types, subtypes, hierarchy, placeholders) | core | done (Phase 1) |
| Transactions with splits, per-commodity balancing | core | done (Phase 1) |
| Account balances and trial balance | core | done (Phase 1) |
| Multi-currency values and exchange rates | multi-currency | wip (Phase 1: foreign-currency accounts and transactions gated by the module; exchange rates not started) |
| Audit log and compensating-command undo | core | done (Phase 1) |
| Closing date / period lock | core | done (Phase 1) |
| MCP server (dual-era: MCP 2026-07-28 stateless requests, subscriptions; legacy `initialize` 2024-11-05 to 2025-11-25) | core | done (Phase 1, spec revision 2) |
| AI assistant access per book: off by default, read only / read and write, permission groups, app-only approval; separate optional `keep-mcp` program | core | done (2026-11-04, `mcp-server.md` revision 3) |

## Personal finance (Phase 2–3)

| Area | Module | Status |
|---|---|---|
| Account registers with running balance | core | done (Phase 2c: `account.register`, register view) |
| Display customisation: saved text size (90–200 %), interface and number fonts from installed system fonts, Settings sections | core | done (Phase 2e) |
| Payees and categories | core | — |
| Categorisation rules and memorised transactions | core | wip (Phase 2a: categorisation rules done; memorised transactions not started) |
| File import: OFX/QFX/QBO, QIF, CSV profiles | core | done (Phase 2a; OFX 1.x/2.x, QFX, QBO bank and card statements) |
| Watched-folder auto-import | core | done (Phase 2a) |
| Shareable definition files (`.mkdef`: import layouts, rules, categories, securities) with review, provenance, update and uninstall | core | done (Phase 3d) |
| Input safety: bounded parsers with fuzzing, text cleaning, commit re-check, CSV formula-injection protection, terminal and AI-agent output rules | core | done (Phase 3d) |
| Duplicate detection and match review | core | done (Phase 2a) |
| Statement reconciliation | core | done (Phase 2a) |
| Scheduled and recurring transactions, reminders | core | done (Phase 2b: `schedule.*`, run_due, forecast; Scheduled view, register scheduled rows, Home reminders) |
| Subscription audit: repeating payments, price history with reasons, price-rise and charged-after-cancel alerts, reviews | subscriptions | done (2026-11-07, `subscriptions.md`: detection by normalised payee and account with six cadences, Suggested list with remembered answers and Dismissed suggestions, add by hand, linked charges, price history with colour bands, six Attention alerts, totals and year-ago comparison, Review mode, Subscriptions dashboard block; revision 2: Make a bill as a repeating bill in Scheduled or a bill now, single-step price changes count as subscriptions) |
| Budgets by category and period | budgets | done (Phase 2b: monthly budgets with overrides and rollover, suggestions, report; Budget view) |
| Loans and amortisation | core | — |
| Net worth, spending, cash-flow reports | core | done (Phase 2b: spending, income & expenses, net worth, cash flow, payees, CSV export; Reports view with charts) |
| Securities, lots, cost-basis methods | investments | done (Phase 3a: securities and prices with CSV import, brokerage and security accounts, ten actions, FIFO/average/specific lots with replay; Holdings, Activity, action forms, Securities view) |
| Investment statement import: OFX `INVSTMTRS`, broker CSV/JSON/XML profiles | investments | done (Phase 3b: OFX buys/sells/income/reinvest/transfers/splits with SECLIST security matching, positions and cash check; broker profiles with field and action mapping, live-preview editor, paired maturity and reinvest rows, built-in Charles Schwab CSV/JSON/XML profiles with review of unverified action texts; Move shares in first; per-100 pricing for bonds and CDs; options not supported) |
| Corporate actions (splits, spin-offs, reinvested dividends) | investments | done (Phase 3a: splits, reinvested dividends, return of capital, shares in/out; Phase 3c: mergers tax-free with or without boot and taxable, spin-offs with allocation % and cash in lieu, cash in lieu, symbol changes with import aliases, return of capital beyond basis as gain) |
| Options, bonds and crypto | investments | done (2026-11-06, `investments-options-bonds-crypto.md`: option and crypto kinds, generated option symbols, written options as short lots, buy/sell to open and close, expiry, exercise and assignment with Publication 550 basis rules, expiry Attention dialog; bonds with face, coupon, maturity, accrued interest, interest accounts by tax, coupon reminders and maturity Attention; crypto with 8 decimals, fees in coin, swaps, rewards and lot transfers; holdings details, Form 8949 export descriptions; opening wizard kinds) |
| Realised and unrealised gains, performance | investments | done (Phase 3a: realised gains short/long-term, unrealised gains, investment income, net worth at market value; Phase 3c: performance report with money-weighted (XIRR) and time-weighted (Modified Dietz) returns and value chart; wash-sale flags with Form 8949 code W and adjustment amounts, wash groups) |
| Attachments (receipts, statements) | core | — |
| Encryption at rest | core | done (Phase 2d: password-protected books — Argon2id + XChaCha20-Poly1305, recovery key, in-memory book with atomic saves, exclusive lock; CLI and desktop screens) |

## Small business (Phase 4)

| Area | Module | Status |
|---|---|---|
| Module framework and Settings page | core | done (Phase 4a: per-module settings schemas, Settings → Features expands to each module's settings) |
| Customers, vendors, contacts | business | done (Phase 4a: customers with terms, tax exemption and default tax rate; Phase 4b: vendors with terms, default expense account, 1099 fields and a TIN-on-file flag) |
| Products and services | items | done (Phase 4a: service, product, other charge and discount items) |
| Estimates | estimates | done (Phase 4a: statuses, expiry, conversion to invoices) |
| Progress invoicing | progress-invoicing | — |
| Invoices, sales receipts, credit memos, refunds | business | wip (Phase 4a: invoices, credit memos, refunds; sales receipts not started) |
| Recurring invoices | recurring-invoices | — |
| Customer payments and application to invoices | business | done (Phase 4a: auto-apply oldest first, customer credit) |
| Undeposited funds and bank deposits | business | done (Phase 4a: Payments to Deposit and grouped deposits) |
| Bills, vendor credits, bill payments | bills | done (Phase 4b: bills to Accounts Payable, check numbering per bank, pay-bills run, prepayments, voided checks, vendor credits, refunds received) |
| Purchase orders | purchase-orders | done (Phase 4b: non-posting orders, receipts create linked bills, PDF) |
| Sales orders | sales-orders | — |
| AR and AP aging | business | done (Phase 4a: A/R aging; Phase 4b: A/P aging; both tied to their ledger balances) |
| Customer statements, late fees | statements | done (Phase 4c: activity and open-item statements with aging, batch PDFs, late fees by percent or flat amount with grace days and a minimum, assessed as invoices) |
| Sales tax codes, rates, liability report | sales-tax | done (Phase 4a: rates, groups, per-component tax, liability report, paying the agency) |
| Classes, locations | classes, locations | done (Phase 4c: name trees, class functions, tags on splits and document lines, required-tag settings, P&L by class or location, rules that set tags) |
| Funds and restricted net assets (nonprofit) | funds | done (Phase 4c: General plus restricted funds, automatic releases linked to their expense, transfers, overspent warnings, statements of activities and financial position, functional expenses, fund activity) |
| Projects and job costing | projects | done (2026-11-07, `projects.md`: project records with sub-projects, budget total or by account, customer, fund link, billable default and markup; project tag on splits, documents, rules, schedules, definition files, expenses, trips and the phone; bulk Set project; Projects screen with Summary, Transactions, Mileage, Documents and Billable tabs; report.project (CSV/PDF), report.projects_summary, P&L columns by project; budget alerts on Home and a Projects dashboard block; billable costs onto invoices with markup to Billable Expense Income) |
| Time tracking, mileage | time, mileage | mileage: desktop and book done (`mileage.md`: vehicles and odometer per year, dated standard rates with Re-apply, trips by miles or odometer with round trips and saved trips, reimbursable trips through expenses, mileage report PDF/CSV, Tax summary section); phone and capture server pending; time: — |
| Inventory (average cost) | inventory | — |
| 1099 tracking | 1099 | done (Phase 4b: NEC and MISC boxes, card payments excluded, account mapping, per-year thresholds; no filing) |
| P&L, balance sheet, trial balance, general ledger | core | wip (trial balance done in Phase 1; balance sheet with retained earnings and net income in Phase 4a; profit and loss by total, class, location, fund, month, quarter or year and balance sheet by fund in Phase 4c; general ledger not started) |
| Invoice and estimate PDF templates | business | done (Phase 4a: built-in layout; Phase 4c: templates with accent colour, Helvetica or Times, show options, payment instructions, footer, title overrides and a PNG/JPEG logo checked for size; shared by statements and purchase orders and by definition files without the logo) |
| IIF import/export | business | done (Phase 4c: TRNS/SPL/ENDTRNS import with account, class and name mapping, list records created on request; general-journal export with stated limits) |

## Connectivity (Phase 5)

| Area | Module | Status |
|---|---|---|
| Provider trait and plugin interface | bank-feeds | wip (Phase 5a: injected fetcher and credential-store interfaces, `mk-connect` as the only network crate; plugin interface not started) |
| SimpleFIN | bank-feeds | done (Phase 5a: v2 with v1 fallback, setup-token claim with confirmation, host-locked HTTPS with the OS trust store, credentials in the encrypted book or OS store, 10-day overlap, rate limits and backoff, pending rows, errlist, auto-commit like watched folders; Settings > Connections, feed markers, Sync now, Home attention, `moneykeep sync`) |
| OFX Direct Connect | bank-feeds | done (Phase 5c: institution profiles entered by hand or from `[[ofx_institution]]` definitions, host acknowledgement before any password is sent, SGML 1.x and XML 2.x requests, client id per book and computer, profile and account information, bank, card and investment statements, status codes, extra questions and one-time codes, one request per account fallback, "Ask for my password each time") |
| Brokerage APIs (SnapTrade, exports) | investments | wip (Phase 5c: SnapTrade personal keys with signed requests, accounts, paged activities mapped to investment rows, positions and balances, re-sent activity ids as duplicates, holdings check after the import and feed prices; Schwab Trader API and IBKR Flex deferred to 5d) |
| Optional aggregator plugin | bank-feeds | — |
| Payment-processor payouts and fees | payment-processing | done (Phase 5b: Stripe restricted keys, Square access tokens, PayPal client credentials with the token never stored; host-locked, rate-limit backoff; payouts through a clearing account in summary or detailed mode with per-category accounts; changed payouts flagged with the difference; deposits matched from the clearing account; invoice-number matching to `payment.receive`; Processor settings, payout review rows, Home notice for unmatched clearing balances). Revision 2: Stripe payouts from the Payouts endpoint with their own status (only `paid` posted, on-the-way payouts re-read, failed payouts after posting reversed), `Square-Version` and permission-naming refusals, PayPal mapping by event code with only T0400/T0401/T0403 closing a payout, Stripe and PayPal fees outside payouts as their own rows, "held at PayPal" on the Accounts list. Not yet: Square fee entries outside payouts (no endpoint in the referenced Payouts API) |

## Infrastructure

| Area | Status |
|---|---|
| Workspace, CI, layering check, licence policy | done (Phase 0) |
| Clean-room docs, spec template, PR provenance checklist | done (Phase 0) |
| SQLite storage with migrations | done (Phase 0) |
| Command registry, CLI skeleton | done (Phase 0) |
| MCP server, JSON control channel | done (MCP server in Phase 1 for MCP 2026-07-28 and legacy revisions; revision 3, 2026-11-04: separate `keep-mcp` program, per-book AI access, and the app's local JSON control channel serving `keep-mcp` with owner approval; other control-channel clients not started) |
| Desktop app (egui, moved to Slint by ui-slint.md) | done (Phase 2c: `moneykeep-app`; start screen, sidebar, home, register, import review, reconcile, accounts, settings, history) |
| wasm build | — |
