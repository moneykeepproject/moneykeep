# Spec: Connectivity, part 3 — OFX Direct Connect and SnapTrade brokerage feeds (Phase 5c)

- **Area / module:** `bank-feeds` (OFX Direct Connect for bank and card accounts),
  `investments` (OFX investment statements, SnapTrade); providers in `mk-connect`
- **Spec writer:** Claude session 2026-10-21 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-21
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Standard | OFX 2.2 Specification (FDX, 2017; https://financialdataexchange.org/wp-content/uploads/2025/12/OFX-2.2.pdf) — §2.1 HTTP transport, §2.5.1 signon (`SONRQ` incl. `CLIENTUID`, `USERCRED1/2`, `AUTHTOKEN`, `ACCESSKEY`, `SESSCOOKIE`; status codes §2.5.1.7), §2.5.4 MFA challenge, §7 FI profile (`PROFRQ`, `CLIENTUIDREQ`, `USERCRED1LABEL`), §8.5 account information (`ACCTINFORQ`, `SVCSTATUS`), §11 bank and credit-card statement requests, §13.9.1 investment statement request (`INVSTMTRQ`, `INCTRAN`, `INCOO`, `INCPOS`, `INCBAL`) | Read by the spec writer 2026-10-21 (owner-supplied copy, already used in Phase 2/3b) |
| 2 | Vendor doc | SnapTrade — Personal vs commercial keys (https://docs.snaptrade.com/docs/personal-vs-commercial), Request signatures (https://docs.snaptrade.com/docs/request-signatures), Syncing and data freshness (https://docs.snaptrade.com/docs/syncing), Broker access guide (https://docs.snaptrade.com/docs/broker-access-guide), API reference: list accounts, account positions (all), balances, account activities (https://docs.snaptrade.com/reference/…) | Read 2026-10-21. Personal keys are free, for the user's own accounts; personal users connect brokerages in the SnapTrade dashboard; requests are signed (HMAC-SHA256 over canonical JSON of `content`, `path`, `query`, Base64, `Signature` header) with `clientId` and `timestamp` in the query; activities are paged (`offset`, `limit` ≤ 1000, `startDate`/`endDate`) and refreshed once a day, delayed one day; Schwab, Fidelity, Vanguard, IBKR are read-only |
| 3 | Spec | `connectivity.md` (framework, secrets, scheduling, safety), `import-ofx.md`, `import-investments.md` (investment rows, positions, classification), `investments-core.md`, `definition-files.md`, `input-safety.md` | |

Exact SnapTrade paths, header names and field names are taken by the implementer from
the reference pages above and recorded in code comments with the doc URL. No target
product was observed.

**Deferred to 5d** (not in this phase): Schwab Trader API (its documentation is behind
a developer login the spec writer could not read), Interactive Brokers Flex Web
Service (the report field naming in XML is not in the pages read; SnapTrade covers
IBKR read-only meanwhile), and the optional paid aggregator plugin (Plaid/MX).

## Purpose

Bring bank, card and brokerage data in without downloading files:

- **OFX Direct Connect** talks to an institution's own OFX server with the user's
  online-banking credentials — free, but only some institutions still run one.
- **SnapTrade** (personal keys, free) reads brokerage accounts the user connected in
  SnapTrade's dashboard — transactions and holdings for Schwab, Fidelity, Vanguard,
  IBKR and others.

Both feed the existing import pipeline; nothing posts without review unless the link
is set to auto-commit and no row needs review.

## Behaviour

### A. OFX Direct Connect provider (`ofx`)

#### Institution profiles

An institution profile is {`name`, `url` (https only), `org`, `fid`, `broker_id`
(investment accounts), `ofx_version` (`102`, `103`, `151`, `160`, `200`, `203`,
`211`, `220`; default `103`), `app_id` (≤ 5 chars, default `MCRFT`), `app_ver` (4
digits, default from the MoneyKeep version, e.g. `0100`), `notes`}.

- Profiles come from the user (entered by hand on connect) or from an installed
  definition file: new item kind **`[[ofx_institution]]`** (format version 1 kinds
  list extended; same text rules, limits and provenance as other kinds).
- **Credential-phishing guard:** a profile decides where the user's banking password
  is sent. So:
  - On connect, the dialog shows the profile's **host** in large type with the text
    "Your user id and password will be sent to <host>." The user must tick "I
    recognise this address as my bank's" before Connect is enabled.
  - `definition.inspect` lists every `ofx_institution` item's host and warns "This
    file contains bank connection addresses. Only install it if you trust its
    author."; installing a profile never connects anything by itself.
  - A connection stores its own copy of the profile; a later definition update that
    changes the URL host does **not** change existing connections (the connection
    shows "The installed profile now points to <new host>" and the user can choose to
    switch, with the same tick-box).
- `app_id`/`app_ver` are user-editable per connection. Some servers accept only
  particular application ids; MoneyKeep does not ship values imitating another
  product, and the dialog notes "Use only values your institution allows."

#### Requests

- HTTP POST to the profile URL, `Content-Type: application/x-ofx`, the body an OFX
  request in the profile's version: SGML with the 1.x header for `1xx`, XML with the
  `<?OFX OFXHEADER="200" VERSION="…"?>` header for `2xx`. A response is parsed with the
  existing OFX parser (`import-ofx.md`, `import-investments.md`).
- Every request carries `SONRQ` with `DTCLIENT`, `USERID`, `USERPASS`, `LANGUAGE`
  `ENG`, `FI` (`ORG`, `FID`) when the profile has them, `APPID`, `APPVER`, and
  `CLIENTUID`. The multi-factor elements (`CLIENTUID`, `USERCRED1/2`, `AUTHTOKEN`,
  `MFACHALLENGE…`) exist only in the versions that added multi-factor authentication —
  **103, 203, 211 and 220** (source 1, §1.4); for 102, 151, 160 and 200 they are never
  sent, and a server asking for them gets "Choose OFX version 103 or later for this
  institution". Each transaction wrapper gets a fresh `TRNUID` (a UUID).
- **Client unique id:** one per book and installation (a UUID created on first use,
  kept in the app's per-book state, not in the book file, so a copied book on another
  computer gets its own). The connect dialog shows it with a Copy button, because
  some institutions ask the user to register it.
- **Profile request** (`PROFRQ`) on connect: reads `CLIENTUIDREQ` and
  `USERCRED1LABEL`/`USERCRED2LABEL`; when labels are present the dialog asks for those
  extra credentials (stored with the password).
- **Account discovery:** `ACCTINFORQ` with `DTACCTUP` 19900101; lists bank
  (`BANKACCTINFO`: bank id, account id, type), card (`CCACCTINFO`) and investment
  (`INVACCTINFO`: broker id, account id) accounts with `SVCSTATUS`; only `ACTIVE`
  services are offered for linking (others shown greyed with their status). When the
  server doesn't support account info, the user can add an account by hand (type, bank
  id or broker id, account number).
- **Statements** per linked account, from `sync_from` (first sync:
  `bank-feeds.history_days_first_sync`) to now:
  - bank: `STMTRQ` with `BANKACCTFROM` and `INCTRAN` (`DTSTART`, `INCLUDE` Y);
  - card: `CCSTMTRQ` with `CCACCTFROM` and `INCTRAN`;
  - investment: `INVSTMTRQ` with `INVACCTFROM`, `INCTRAN`, `INCOO` N, `INCPOS`
    (`INCLUDE` Y), `INCBAL` Y.
  One OFX request per sync may carry all of a connection's accounts (one transaction
  wrapper each); if the server rejects multi-account requests (a 2000 general error on
  the second wrapper), MoneyKeep falls back to one request per account and remembers
  that for the connection.
- Later syncs start at the last statement's `DTEND` minus 7 days (overlap removed by
  FITID fingerprints, as for files).

#### Status codes and extra authentication

| Code (§2.5.1.7) | Behaviour |
|---|---|
| 0 | Continue |
| 3000 | Send `MFACHALLENGERQ`; show the questions (text, or the label of a `MFAPHRASEID` the spec lists); the answers go in the next `SONRQ` as `MFACHALLENGEANSWER`. Interactive only: a background sync marks the connection needs attention ("Your bank asks extra questions — open Sync to answer") |
| 3001 | "An answer was not accepted" — ask again |
| 15000 | "Your bank requires a new password" — needs attention; MoneyKeep does not change passwords |
| 15500 | "User id or password not accepted" — needs attention; no retry (avoids lockout) |
| 15502 | "Your bank has locked this sign-in" — needs attention, disabled until the user re-enables |
| 15510 | "Your bank does not accept this client id" + the id to register |
| 15512 / 15513 | Ask for an `AUTHTOKEN` (a one-time code from the bank) |
| 15514 / 15515 | Unsupported ("this bank needs an access token MoneyKeep can't obtain") |
| 13504 | "Institution id (FI) missing or wrong" — check the profile |
| 13505, HTTP 5xx | Retry later with backoff (as other providers) |
| other / HTTP 4xx | Needs attention with the code and the server's message (cleaned, R3) |

- `ACCESSKEY` and `SESSCOOKIE` returned by the server are kept with the secret and sent
  on the next signon, as §2.5.1 describes. `USERKEY` (if returned) is used instead of
  the password until `TSKEYEXPIRE`.

#### Secrets

- The OFX user id, password, extra credentials, access key, session cookie and user key
  are one credential, stored per `connectivity.md` (encrypted book or OS store, never
  plain text).
- Per-connection option **"Ask for my password each time"** (default off): nothing
  but the user id is stored; scheduled syncs skip the connection and Home shows "Sync
  needs your password".
- Requests and responses are never logged; error messages never include the request
  body.

### B. SnapTrade provider (`snaptrade`)

- **Credential:** the user's **personal** client id and consumer key (created in the
  SnapTrade dashboard). Commercial keys are out of scope: MoneyKeep never calls the
  register-user endpoint and never stores a user secret. The connect dialog explains
  the steps: create a personal key in the SnapTrade dashboard, connect brokerages
  there, then paste the two values here.
- **Signing:** every request per source 2 — `clientId` and `timestamp` (Unix seconds)
  appended to the query; the signed string is the canonical JSON (keys sorted at every
  level, no whitespace, UTF-8) of `{"content": <body or null>, "path": <path>,
  "query": <raw query string>}`; HMAC-SHA256 with the consumer key; Base64; sent in the
  signature header the reference names for personal keys. Host: `api.snaptrade.com`
  only. A unit test checks the signature against a vector computed by hand in the test
  (documented in the test).
- **Accounts:** the reference's "list all user accounts" operation; each remote account
  shows institution name, account name, number masked to the last 4, and sync status
  (initial sync completed, last successful sync). Only investment-type accounts link to
  MoneyKeep investment accounts (others are listed, not linkable in this phase).
- **Activities:** `GET …/accounts/{id}/activities` with `startDate`/`endDate`,
  `limit` 1000 and `offset` paging until `offset + returned ≥ pagination.total`.
  First sync: `processors`-style setting `brokerage.history_days_first_sync` (default
  365); later syncs start 10 days before the last synced trade date (data is delayed a
  day).
- **Positions and balances:** the "all positions" operation and the balances operation
  for each linked account, for prices and the holdings check (below).
- **Numbers:** JSON numbers are read into `Decimal` from their text, never through a
  binary float (a test feeds `0.1` and `1e-7` style values).
- **Rate limits:** honour `429` with `Retry-After`; at most one sync per 6 hours per
  connection unless Sync now (data refreshes once a day). MoneyKeep never calls the
  connection-refresh operation (it may be billable).
- **Disabled connections** (SnapTrade reports a brokerage connection broken): the
  affected accounts show "Reconnect this brokerage in the SnapTrade dashboard".

#### Activity mapping (SnapTrade `type` → investment row of `import-investments.md`)

| `type` | Row |
|---|---|
| `BUY` | `buy`: quantity \|`units`\|, price `price`, commission `fee`, amount \|`amount`\| |
| `SELL` | `sell`: as buy |
| `DIVIDEND`, `SUBSTITUTE_DIVIDEND` | `dividend` (kind ordinary; substitute noted in memo) |
| `REI` | paired with a `DIVIDEND` of the same security and date (amounts equal) → one `reinvest`; unpaired → `buy` marked needs review "Reinvestment without a matching dividend" |
| `INTEREST` | `interest` |
| `FEE` | `fee` |
| `TAX` | `cash` row, category from rules (default "Taxes withheld" needs review) |
| `CONTRIBUTION`, `WITHDRAWAL` | `cash` row (transfer matching as bank rows) |
| `TRANSFER` | no security → `cash` row; with a security → `shares_in` / `shares_out` by the sign of `units` |
| `EXTERNAL_ASSET_TRANSFER_IN` / `_OUT` | `shares_in` / `shares_out`; basis for `shares_in` = `average_purchase_price` × units from the position when available, else needs review asking for it |
| `STOCK_DIVIDEND` | `shares_in` with zero basis, needs review ("Check the basis rules for this stock dividend") |
| `SPLIT` | `split`: ratio = (held before + `units`) / held before, from the book's quantity on that date; not computable → needs review |
| `OPTIONEXPIRATION`, `OPTIONASSIGNMENT`, `OPTIONEXERCISE`, any activity with `option_symbol` | `unsupported` ("Options are not supported yet") |
| `ADJUSTMENT`, unknown types | `unsupported` with "Unknown activity: <type>" |

- Security resolution as in 3b: by CUSIP/ISIN if present, then FIGI (stored as a new
  optional security identifier), then symbol (`raw_symbol`), else a new security
  proposed with the name from `description`.
- Activities in a currency other than the base currency → `unsupported` (foreign
  currency), as 3b.
- **Fingerprints:** `ext:` + connection + activity `id`. Because the reference says an
  id can change when the brokerage re-sends a transaction, a row with a new id whose
  3b content hash equals a transaction already imported from the same connection
  within 3 days is classed *duplicate* with the note "Same as an earlier transaction
  (the provider changed its id)".

### C. Holdings check and prices (both providers)

- Positions from OFX `INVPOSLIST` or SnapTrade positions:
  - store a **price** per security (source "OFX — <label>" or "SnapTrade", date = the
    price's as-of date; OFX `DTPRICEASOF` 19900101 → no price);
  - compare the provider's **quantity** with MoneyKeep's quantity for the linked
    account **after** the staged batch would be committed; differences (beyond 0.0001
    units) are listed in the sync result and on the review screen ("Provider holds
    12.5 VTI; MoneyKeep will hold 10 after this import") and Home's Attention shows
    "<account>: N holdings differ from <provider>".
  - Cash: provider cash (OFX `AVAILCASH`, SnapTrade cash balance in the base currency)
    vs MoneyKeep's cash for the account, same treatment.
- MoneyKeep never creates adjusting transactions automatically; the review offers
  "Add a shares-in/out row for the difference" (needs review, basis asked).

### Commands

- `connect.add` gains providers `ofx` (setup input: profile or definition item key,
  user id, password, extra credentials, host acknowledged: true — required) and
  `snaptrade` (client id, consumer key). Not MCP.
- `connect.answer_challenge` {connection, answers or auth_token} — not MCP (answers are
  credentials).
- `connect.ofx_profile` {connection} / `connect.set_ofx_profile` {connection,
  app_id, app_ver, ofx_version, accept_new_host} — MCP yes (no secrets; changing the
  host requires `accept_new_host` and is refused through MCP).
- `connect.client_uid` — MCP yes (the id is not secret).
- `connect.add_manual_account` {connection, kind bank|card|investment, bank_id or
  broker_id, account_number, account_type} — MCP yes.
- `connect.holdings_check` {account} — MCP yes; returns the latest comparison.
- `connect.sync`, `connect.link_account`, `connect.list` work for both providers.

### Settings

| Key | Type | Default |
|---|---|---|
| `brokerage.history_days_first_sync` | int 1–1825 | 365 |
| `brokerage.sync_interval_hours` | int 6–168 | 24 |
| `bank-feeds.ofx_default_version` | OFX version | `103` |

## Ledger effect

None directly; both providers stage import batches (bank/card rows as statement rows,
investment rows as in 3b). Prices are stored as price records (source noted).

## Edge cases

- An institution that answers with an HTML page (moved or retired OFX server): "This
  address did not answer as an OFX server" — needs attention; the body is not shown.
- A server that returns a 1.x SGML response to a 2.x request (or the reverse): parsed
  anyway (the parser detects the header).
- OFX statement with no transactions but a balance: the balance still updates the link
  and the reconcile hint.
- SnapTrade Schwab access may need a broker approval on SnapTrade's side before data
  appears; the accounts list shows SnapTrade's sync status ("initial sync not
  completed") rather than an error.
- Clock skew: a SnapTrade 401 with a timestamp complaint shows "Check your computer's
  clock".
- Redirects: followed only to the same host over https; anything else is an error.
- R2 limits: responses over 25 MB are refused; OFX responses go through the existing
  OFX parser limits.

## Acceptance tests

All against local fake HTTPS servers (OFX server and SnapTrade API) with responses
shaped from the sources; no internet.

1. OFX connect: `PROFRQ` shows `USERCRED1LABEL` "PIN:"; the dialog asks for it; the
   signon of the next request carries `USERID`, `USERPASS`, `USERCRED1`, `CLIENTUID`
   and the profile's `APPID`/`APPVER` (asserted by the fake server).
2. `ACCTINFORQ` lists one checking (`ACTIVE`), one card (`ACTIVE`) and one investment
   account (`PEND`); the pending one is shown greyed and cannot be linked.
3. Sync stages a checking statement (3 transactions), a card statement (2) and,
   after the investment account becomes active, an investment statement with one buy,
   one dividend and positions; re-sync stages all as duplicates.
4. Version 102 requests are SGML with the 1.x header and no `CLIENTUID`; version 220
   requests are XML with the 2.x header; both parse the fake server's responses.
5. (Version 103.) Status 3000 → `MFACHALLENGERQ` is sent, two questions are shown, the answers are
   sent in the next signon and the sync completes; in a background sync the connection
   is marked needs attention instead.
6. Status 15500 marks needs attention and no further request is sent in that sync.
7. A server rejecting the second transaction wrapper with 2000 leads to one request per
   account; the fallback is remembered for the next sync.
8. A definition file with an `[[ofx_institution]]` item: inspect lists its host and the
   warning; connecting without `host acknowledged` is refused; a later version of the
   definition with a new host leaves the connection on the old host with the notice.
9. The OFX password is never written to the book in plain text, logs or audit; with
   "Ask for my password each time", the scheduled sync skips the connection and Home
   shows "Sync needs your password".
10. SnapTrade signing: for a fixed consumer key, path, query and empty body, the
    `Signature` header equals the test's hand-computed HMAC-SHA256 Base64 value.
11. SnapTrade accounts: two accounts listed, numbers masked to the last 4; linking one
    to "Brokerage" and syncing pages activities with `offset` 0 and 1000 when `total`
    is 1,200.
12. Mapping: BUY 10 VTI at 220.00 fee 0 amount −2,200.00 → buy; DIVIDEND 12.34 + REI
    12.34 for VTI on the same date → one reinvest; OPTIONEXPIRATION → unsupported;
    `ADJUSTMENT` → unsupported "Unknown activity: ADJUSTMENT".
13. An activity re-sent with a new id but identical content is a duplicate with the
    "provider changed its id" note.
14. Numbers: `price` 0.1 and `units` 3 give amount 0.30 exactly; `1e-7` parses as
    0.0000001.
15. Holdings check: provider 12.5 VTI vs MoneyKeep 10 after the batch → listed
    difference and a Home Attention entry; provider price 231.17 stored with source
    "SnapTrade".
16. A 429 with `Retry-After: 2` waits and retries; a 401 marks needs attention.
17. MCP lists `connect.ofx_profile`, `connect.set_ofx_profile`, `connect.client_uid`,
    `connect.add_manual_account`, `connect.holdings_check`; never `connect.add` or
    `connect.answer_challenge`; `connect.set_ofx_profile` with a new host via MCP is
    refused.
18. A redirect to another host is refused; an HTML answer gives "did not answer as an
    OFX server".

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-11-08) — SnapTrade positions endpoint retired; one failing call no longer loses everything (owner report)

Report: a new personal SnapTrade key with Schwab found the accounts, but every sync ended
with "SnapTrade answered with HTTP 410; MoneyKeep will try again later." No transactions,
no holdings.

Sources (read 2026-11-08): SnapTrade API reference "List account positions"
(`AccountInformation_getUserAccountPositions`, `GET /accounts/{accountId}/positions`) —
marked **deprecated**, pointing to "List all account positions"
(`AccountInformation_getAllAccountPositions`, `GET /accounts/{accountId}/positions/all`,
answer `{results: [...]}` with an `instrument` object per position (`kind`, `symbol`,
`currency`, …), `cash_equivalent`, optional `tax_lots`); SnapTrade's published SDK
README (snaptrade-python-sdk 13.0.13 on PyPI): the deprecated holdings endpoint answers
**410 for customers who signed up after May 11, 2026**; the activities endpoint is not
deprecated, but requests under the legacy `/api/v1` prefix carry a `Deprecation` header;
rate limits 250/minute per customer and possibly 10/minute per account for personal
users (honour `Retry-After` on 429).

Cause: MoneyKeep reads positions from the deprecated per-account endpoint, which SnapTrade
answers with 410 for newer keys. And the sync stops at the first failing call, so the
activities already read (and the balances) were thrown away too.

### Changes

- **Positions** come from `GET /accounts/{accountId}/positions/all`. Each result maps to
  a holding: the instrument's symbol (and description, currency, kind), the quantity, the
  price and average purchase price (field names from the reference). `cash_equivalent`
  positions count toward cash, not securities. Option, future and other non-security
  kinds are listed with their kind and skipped for holdings in this version (a note in
  the sync summary: "2 option positions not imported"). `tax_lots`, when present, are
  ignored in this version.
- **Paths** drop the legacy `/api/v1` prefix where SnapTrade's current reference shows
  paths without it (the signature is computed over the path actually requested). The
  implementer checks each path used (accounts list, positions/all, balances, activities)
  against the current reference; any `Deprecation` header in an answer is logged with the
  path.
- **Independent calls:** for each account, activities, positions and balances are read
  separately. A failure of one does not discard the others: what was read is imported,
  and the connection shows a yellow line naming what failed, e.g. "Holdings couldn't be
  read (SnapTrade answered 410 for positions). Transactions were imported." The red
  line stays only when nothing could be read.
- **410** is reported as "SnapTrade no longer offers this request (<what>). MoneyKeep
  needs an update." (not "will try again later"), since retrying cannot help.
- **429** honours `Retry-After`; calls for one account are spaced to stay under 10 per
  minute.

### Tests (fake SnapTrade server)

19. Positions are read from `/accounts/{id}/positions/all` (`results` with
    `instrument.symbol`); a cash-equivalent position adds to cash; an option position is
    counted in the "not imported" note.
20. Positions answering 410 while activities and balances answer 200: the activities and
    cash are imported; the connection shows the yellow line naming holdings and the 410
    wording; no red line.
21. Every call answering 410: the red line with "SnapTrade no longer offers this request".
22. A 429 with `Retry-After: 2` waits at least 2 seconds (fake clock) before retrying.
