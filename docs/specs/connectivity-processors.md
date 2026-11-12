# Spec: Connectivity, part 2 — payment processor payouts (Stripe, Square, PayPal) (Phase 5b)

- **Area / module:** `payment-processing` (already in the catalog); providers in `mk-connect`
- **Spec writer:** Claude session 2026-10-21 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-21
- **Status:** draft — revision 2 (vendor details verified against the fetched references, 2026-10-21)

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Vendor doc | Stripe API reference — Balance Transactions (https://docs.stripe.com/api/balance_transactions), Payouts; Payout reconciliation report (https://docs.stripe.com/reports/report-types/payout-reconciliation) | A payout groups balance transactions; each balance transaction has amount, fee, net, currency, type/reporting category, created and available-on times; restricted (read-only) API keys exist |
| 2 | Vendor doc | Square Payouts API — overview (https://developer.squareup.com/docs/payouts-api/overview), list payout entries (https://developer.squareup.com/docs/payouts-api/list-payout-entries) | Payouts and payout entries with gross, fee and net amounts by entry type; cursor pagination; payout read permission |
| 3 | Vendor doc | PayPal Transaction Search API (https://developer.paypal.com/docs/api/transaction-search/v1/, guide https://developer.paypal.com/docs/transaction-search) | Transactions by date range (a window of at most 31 days per request), event codes, gross and fee amounts; OAuth 2 client credentials; reporting search permission |
| 4 | Textbook | OpenStax, *Principles of Accounting, Volume 1* — clearing accounts, revenue net of fees | Accounting pattern |
| 5 | Spec | `connectivity.md` (framework, secrets, background sync, safety), `import-pipeline.md`, `business-sales.md` (invoices, payments), `input-safety.md` | |

Exact endpoint paths, field names, enum values and pagination parameters are taken
by the implementer from the vendor references above (vendor API documentation is a
permitted input) and recorded in code comments with the doc URL; this spec fixes the
behaviour and the accounting. No target product was observed.

## Purpose

Card and online-payment processors deposit **net** amounts (sales minus fees, refunds
and disputes) days after the sales. The bank feed shows one deposit; the books need
gross sales, fees and refunds. This feature brings each processor's payouts in so
that the deposit, the income and the fees all agree, through a clearing account.

## Accounting pattern

- Each processor connection has a **clearing account** (asset, subtype `clearing`,
  e.g. "Stripe Clearing"), created on connect (or chosen).
- For every **payout** the processor reports, MoneyKeep proposes one transaction
  (in the default **summary** mode) dated the payout's arrival date:
  - gross sales → credit the sales income account,
  - refunds → debit the refunds account (contra-income, default the sales account),
  - processing fees → debit the fees expense account ("Merchant Fees"),
  - disputes/chargebacks and their reversals, adjustments, reserves, taxes collected by
    the processor (where reported) → their mapped accounts,
  - and the **net** to the clearing account (debit),
  so the transaction balances: net = gross − refunds − fees ± others.
- The bank deposit (from the bank feed or a bank file) is a transfer from the clearing
  account; the pipeline's transfer matching pairs it (same amount, within the match
  window) so the clearing account returns to zero. A clearing balance left over after
  the match window is shown on Home ("Stripe Clearing has 125.40 not matched to a
  deposit").
- **Detailed** mode (setting per connection): one transaction per sale/refund/fee/
  dispute entry into the clearing account, plus the payout itself as a transfer row
  (clearing → "in transit" until the bank matches). Same totals.
- **Invoices:** when `invoicing` is on and an entry's description/metadata contains an
  open invoice number (exact `invoicing.number_prefix` + digits match), review proposes
  "Apply to INV-1002" — a `payment.receive` with deposit to the clearing account —
  instead of crediting sales income; the user confirms per row.

## Behaviour

### Providers

| Provider | Credential | Data read | Payout grouping |
|---|---|---|---|
| Stripe | A **restricted, read-only** API key the user creates (MoneyKeep refuses keys whose prefix shows a secret, non-restricted key and explains why) | Payouts and, per payout, its balance transactions | Native payouts |
| Square | A personal access token (production) with payout-read permission | Payouts and their payout entries, per location | Native payouts |
| PayPal | REST app client id + secret; MoneyKeep exchanges them for a short-lived token per sync (client credentials) and never stores the token | Transactions by date window (≤ 31 days per request, paged) | Between consecutive withdrawals to bank; transactions after the last withdrawal stay "pending payout" |

- All use the `connectivity.md` framework: secrets in the encrypted book or OS store,
  only the provider's API host allowed (Stripe `api.stripe.com`, Square
  `connect.squareup.com`, PayPal `api-m.paypal.com`), HTTPS with the OS trust store,
  background syncs with cancel, no network inside book writes, R2 limits on responses,
  R3 cleaning of descriptions, rate limiting (honour the provider's rate-limit
  responses with backoff; at most one sync per hour per connection unless Sync now).
- History on first sync: `processors.history_days_first_sync` (default 90; PayPal
  capped at 3 years by the provider).
- **Currency:** only payouts in the base currency are posted; others are listed with
  "multi-currency needed".
- **Fingerprints:** `ext:` + connection + provider payout id (summary) or entry id
  (detailed). Re-syncs give duplicates, never double posting. A payout that changes
  after posting (the provider reports new entries) is flagged needs review with the
  difference.

### Category mapping

Each provider's entry kinds map to MoneyKeep categories: `sale`, `refund`, `fee`,
`dispute`, `dispute_reversal`, `adjustment`, `reserve`, `tax`, `transfer` (the payout
itself), `other`. The implementer records the vendor-specific mapping table (e.g.
Stripe reporting categories, Square payout entry types, PayPal event code groups) in
the provider module with doc links. Each category has an account setting on the
connection; `other` defaults to an "Uncategorized Expense/Income" by sign and is
always needs review.

### Commands

`connect.add` gains providers `stripe`, `square`, `paypal` (setup input: the key or
client id/secret; not MCP). `processor.settings` {connection} / `processor.set`
{connection, mode, accounts per category, clearing account, invoice matching on/off}
(MCP: yes, no secrets). `connect.sync` works for these providers and stages one batch
per connection with one row per payout (summary) or per entry (detailed); the review
shows each payout's breakdown (gross, refunds, fees, others, net) in a tooltip/expand.

### Settings (module `payment-processing`)

| Key | Type | Default |
|---|---|---|
| `processors.history_days_first_sync` | int 1–1095 | 90 |
| `processors.default_mode` | `summary` / `detailed` | `summary` |
| `processors.sales_account` | income account | "Sales" (created on first use) |
| `processors.fees_account` | expense account | "Merchant Fees" (created on first use) |

## Ledger effect

As in the accounting pattern; through import review and commit only.

## Edge cases

- A payout that failed or was reversed by the bank: posted as a reversal of the
  payout transaction when the provider reports it (needs review).
- Fees billed separately (not netted in payouts): `fee` entries outside payouts are
  staged as their own rows against the clearing account.
- Square multi-location: one section per location (or one per account, setting), all
  into the same clearing account by default.
- PayPal balance kept in PayPal (no withdrawal): transactions stay pending payout;
  the clearing account then represents the PayPal balance (shown on the Accounts list
  with a "held at PayPal" note).

## Acceptance tests

All against a local fake HTTPS server per provider or recorded responses (shapes
taken from the vendor docs); no internet.

1. Stripe: a payout of net 945.30 made of 3 charges totalling 1,000.00 (processing
   fees totalling 30.50) and one refund of 24.20 stages one summary row;
   committing posts Sales −1,000.00 (credit), Refunds 24.20, Merchant Fees 30.50,
   Stripe Clearing 945.30; a bank deposit of 945.30 two days later matches it as a
   transfer and Stripe Clearing returns to 0.00.
2. Re-sync stages that payout as a duplicate.
3. Detailed mode for the same payout stages five entry rows plus the payout transfer;
   committed totals equal test 1.
4. A Stripe charge whose description contains "INV-1002" (open, balance 250.00)
   proposes "Apply to INV-1002"; accepting records the payment into Stripe Clearing
   and the invoice becomes paid.
5. A non-restricted Stripe secret key is refused before any request with the
   explanation.
6. Square: a payout with CHARGE, REFUND and FEE entries posts the same pattern; two
   locations give two sections.
7. PayPal: transactions over 40 days are fetched in two windows (≤ 31 days each);
   three sales, one fee-only adjustment and one withdrawal produce one payout group
   ending at the withdrawal; later sales stay "pending payout".
8. PayPal OAuth: the token request uses client credentials; the token never appears in
   storage, logs or audit; the client secret is stored per `connectivity.md`.
9. A provider rate-limit response triggers backoff; a 401 marks the connection needs
   attention with a reconnect message.
10. A payout in EUR in a USD book is listed with "multi-currency needed" and not
    posted.
11. Entry descriptions with control/bidi characters are cleaned; a 25 MB response is
    refused.
12. MCP lists `processor.settings`, `processor.set`, `connect.sync`; never
    `connect.add`.

## Revision 2 — verified vendor details (2026-10-21)

The spec writer read the vendor references below on 2026-10-21 and checked the 5b
implementation against them. These are corrections; everything else stands.

| # | Reference read |
|---|---|
| 1a | Stripe — List payouts, https://docs.stripe.com/api/payouts/list ; the Payout object |
| 1b | Stripe — Reporting categories, https://docs.stripe.com/reports/reporting-categories |
| 2a | Square — List payouts, https://developer.squareup.com/reference/square/payouts-api/list-payouts |
| 2b | Square — List payout entries, https://developer.squareup.com/reference/square/payouts-api/list-payout-entries ; List locations |
| 3a | PayPal — Transaction event codes, https://developer.paypal.com/docs/transaction-search/transaction-event-codes/ |

### R2.1 Stripe: list payouts from the Payouts endpoint

- **Payout list:** `GET /v1/payouts` with `created[gte]`/`created[lte]` (or
  `arrival_date` filters), `limit=100`, `starting_after` paging. Today the code lists
  `balance_transactions?type=payout`; that transaction's `status` is the *balance*
  status (pending/available), not the payout's, so failed or cancelled payouts are
  never seen. Use the Payout object's `id`, `amount`, `currency`, `arrival_date`,
  `status` (`paid`, `pending`, `in_transit`, `canceled`, `failed`), `failure_code`,
  `type` and `method`.
- **Entries:** keep `GET /v1/balance_transactions?payout=po_…` for what a payout
  contains.
- **Status handling:** only `paid` payouts are staged for posting; `pending` and
  `in_transit` are listed as "on the way" (not posted, re-checked next sync).
  `failed` or `canceled` after posting → stage a reversal of the posted payout
  transaction (needs review) showing `failure_code`; before posting → not staged,
  listed with the reason. This closes the "failed-payout reversal" gap.
- **Connection test:** `GET /v1/payouts?limit=1` (also proves the restricted key has
  payout read permission).
- **Categories** (reporting category → MoneyKeep): `charge` → sale; `refund`,
  `refund_failure` → refund; `fee` → fee; `dispute` → dispute; `dispute_reversal` →
  dispute_reversal; `partial_capture_reversal` → sale (it reduces a charge);
  `payout`, `payout_reversal` → transfer; `risk_reserved_funds`,
  `connect_reserved_funds` → reserve; `tax` → tax; `other_adjustment` → adjustment;
  `transfer`, `transfer_reversal`, `platform_earning` and anything unknown → other
  (needs review). The current table matches; keep the `type` fallback.

### R2.2 Square: version header and permissions

- Send `Square-Version: 2026-09-16` on every request (a constant in the provider
  module with the doc link; bumped deliberately, never "latest").
- Required permissions: `PAYOUTS_READ` and `MERCHANT_PROFILE_READ` (for
  `/v2/locations`). A 401/403 names the missing permission in the "needs attention"
  message.
- Payout list params: `location_id`, `begin_time`/`end_time` (RFC 3339),
  `sort_order`, `cursor`, `limit` ≤ 100; payout statuses other than `PAID` are listed
  but not posted (same rule as Stripe).
- Entry amounts: use `gross_amount_money`, `fee_amount_money`, `net_amount_money`;
  check gross − fee = net per entry and flag the payout needs review when not.
- Unknown entry types → `other` (needs review). The current table stands.

### R2.3 PayPal: map by specific event code

Replace the group-only rule for codes the reference lists; groups remain the
fallback.

| Code(s) | MoneyKeep category | Note |
|---|---|---|
| T0000–T0099 with gross > 0 | sale | received payments |
| T0000–T0099 with gross ≤ 0 | other | money sent from the PayPal balance; needs review |
| T0100–T0199 | fee | |
| T0200–T0299 | other | currency conversion; listed "multi-currency needed" |
| T0400, T0401, T0403 | transfer | withdrawal to bank = **payout boundary** |
| other T04xx | transfer | needs review |
| T1106 | dispute | reversal initiated by PayPal |
| T1107 | refund | refund initiated by merchant |
| T1108, T1109 | fee | fee reversal / fee refund (positive amount reduces fees) |
| other T11xx | refund | needs review |
| T1201 | dispute | chargeback |
| T1202 | dispute_reversal | chargeback reversal |
| other T12xx | dispute | needs review |
| T15xx, T21xx | reserve | holds and releases |
| anything else | other | needs review |

Payout grouping: a group ends at a **T0400/T0401/T0403** withdrawal only (other T04xx
codes are staged on their own, needs review). Transactions with `transaction_status`
other than `S` stay excluded.

### R2.4 Gaps carried from 5b (now in scope)

- **Fees outside payouts:** Stripe `fee` balance transactions with no payout, Square
  fee entries on no payout, and PayPal T01xx after the last withdrawal are staged as
  their own rows against the clearing account (edge case above), fingerprinted by
  entry id.
- **Held at PayPal:** when a PayPal clearing account has a balance and no withdrawal
  in the last 30 days, the Accounts list shows the note "held at PayPal" beside it.

### Added acceptance tests

13. Stripe: a payout listed with `status: in_transit` is shown "on the way" and not
    staged; next sync with `paid` stages it.
14. Stripe: a posted payout later reported `failed` with `failure_code:
    account_closed` stages a reversal row (needs review); committing returns the
    945.30 to Stripe Clearing.
15. Square: every request carries `Square-Version: 2026-09-16` (asserted by the fake
    server); a 403 shows a message naming `PAYOUTS_READ`.
16. PayPal: T1108 of +2.10 maps to fee (reducing Merchant Fees); T1202 maps to
    dispute_reversal; a T0402 is staged on its own and does not close a payout group.
17. A Stripe `fee` transaction with no payout is staged as its own row against
    Stripe Clearing.
18. A PayPal clearing account with 412.00 and no withdrawal for 30 days shows "held at
    PayPal" on the Accounts list.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
