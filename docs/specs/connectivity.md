# Spec: Connectivity, part 1 — connection framework and SimpleFIN feeds (Phase 5a)

- **Area / module:** `bank-feeds` (already in the catalog); new crate `mk-connect`
- **Spec writer:** Claude session 2026-10-21 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-21
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Standard | SimpleFIN Protocol v2.0.0 (2026-03-19) — https://simplefin.org/protocol.html; v1 — https://www.simplefin.org/protocol-v1.html | Setup token (Base64 URL) claimed with POST to obtain an Access URL with Basic credentials; `GET /accounts` with `start-date`, `end-date`, `pending`, `account`, `balances-only`, `version`; Account Set (`errlist`, `connections`, `accounts`), Account and Transaction fields; epoch timestamps; numeric strings; `GET /info` versions; 402/403 meanings; "strings must be sanitized before display" |
| 2 | Standard | RFC 9110 (HTTP semantics), RFC 7617 (Basic authentication), RFC 8446 (TLS 1.3) | Transport |
| 3 | Vendor doc | `keyring` crate documentation — https://docs.rs/keyring (Windows Credential Manager, macOS Keychain, Linux Secret Service) | Secret storage outside the book |
| 4 | Spec | `import-pipeline.md`, `input-safety.md`, `book-encryption.md`, `scheduled-transactions.md`, `mcp-server.md` | Feeds land in the import pipeline; safety rules; secrets |
| 5 | Design | MoneyKeep plan "Connectivity landscape": SimpleFIN first (cheap, self-host friendly), `mk-connect` provider trait, all feeds share the import path; no browser automation or scraping | |

No target product was observed. Names and wording are MoneyKeep's own.

## Purpose

Bring bank and card transactions in automatically, without downloading files, through
providers the user authorises, while keeping credentials safe and every new
transaction reviewable exactly like a file import.

## Behaviour

### Architecture

- New crate **`mk-connect`**: provider trait + providers. It is the **only** crate
  allowed to do network IO (the `xtask` denied-crate check is extended: HTTP crates
  may appear in `mk-connect` and the app only; `mk-core`, `mk-ledger`, `mk-store`,
  `mk-import`, `mk-commands`, `mk-ui` stay network-free).
- A provider turns its data into the **format-neutral statement** of
  `import-pipeline.md` (sections with account identity, currency, balances and raw
  rows). From there, staging, fingerprints (provider transaction id → `external_id`),
  rules, matching, review and commit are unchanged. A feed never writes to the ledger
  directly.
- Provider trait (conceptual): `info()`, `connect(setup_input) → Credential`,
  `accounts(credential) → Vec<RemoteAccount>`, `fetch(credential, accounts, since,
  until) → Statement`, `disconnect(credential)`. Providers declare their allowed hosts.
- Commands live in `mk-commands` but call `mk-connect` through an injected fetcher
  interface, so commands stay testable offline (tests use recorded responses).

### Connections and account links

| Record | Fields |
|---|---|
| Connection | `id`, `provider` (`simplefin`), `label` (user text), `status` (`active`, `needs_attention`, `revoked`, `disabled`), `secret_ref` (see Secrets), `created_at`, `last_sync_at`, `last_error` (code + message), provider metadata (for SimpleFIN: host, protocol version in use, server connections list with `conn_id`, `name`, `org_id`, `org_url`) |
| Account link | `connection`, `remote_account_id`, remote `name`, remote `conn_id`, currency, last balance and `balance-date`, the linked MoneyKeep `account` (or none = ignored), `sync_from` date, `include_pending` (default false), `auto_commit` (default false) |

### Secrets

- A credential (the SimpleFIN Access URL, which contains a username and password) is
  **never** stored in plain text in the book, config, logs, audit log, errors or MCP
  output.
- Storage, in order:
  1. If the book is password protected (`book-encryption.md`), the credential is
     stored **inside the book** in a secrets table (encrypted with the book).
  2. Otherwise, in the operating system's credential store via `keyring`, under
     service `MoneyKeep` and an id derived from the book id and connection id; the
     book stores only that reference.
  3. If neither is available, connecting is refused with a plain message suggesting
     password protection.
- Displaying a connection shows the host only (`bridge.simplefin.org`), never the
  username or password. Exports, definitions and IIF never include connection data.
- `connect.disconnect` deletes the stored credential (and asks the provider to revoke
  when it supports that; SimpleFIN v2 has no revoke endpoint — the user is told to
  revoke at the provider's site).

### SimpleFIN provider

- **Connect:** the user pastes a **setup token**. MoneyKeep trims it, Base64-decodes
  it (standard or URL-safe alphabet, padding optional), and requires the result to be
  an `https://` URL whose path contains `/claim/`, of at most 2,000 characters.
  Before claiming it shows the claim host and asks the user to confirm. It then sends
  `POST` to that URL with an empty body. 200 → the body must be an `https://` URL
  with userinfo `user:password@host` and a path, ≤ 2,000 characters; it becomes the
  credential. 403 → "This setup token was already used or does not exist. If you did
  not use it, someone else may have — revoke access at your provider." Other statuses
  → error with status code. The token is not stored.
- **Allowed hosts:** after connecting, requests go only to the Access URL's host (no
  redirects to other hosts; redirects within the host followed at most 3 times; only
  `https`; TLS 1.2+ with certificate validation using the OS trust store).
- **Version:** `GET {access}/info` once per connect → choose `2` if offered, else
  `1`; stored.
- **Fetch:** `GET {access}/accounts?version=N&start-date=S[&end-date=E][&pending=1]
  [&account=ID…]` with Basic auth from the URL userinfo (credentials removed from the
  URL itself). `S` = the earlier of `sync_from` (first sync) and the last successful
  sync minus **10 days** overlap (to catch late-posting transactions; duplicates are
  removed by fingerprints), as epoch seconds at 00:00 UTC. Rate: at most one
  `/accounts` request per connection per hour unless the user presses **Sync now**,
  and never more than 24 per connection per day.
- **Response handling:** HTTP 200 JSON (≤ 20 MB, parsed with `input-safety.md` R2
  limits). 402 → connection `needs_attention`: "Your SimpleFIN subscription needs
  payment." 403 → `revoked`: "Access was revoked or the credentials are wrong —
  reconnect with a new setup token." Network failures and 5xx → kept `active`,
  `last_error` set, retried at the next scheduled time with backoff (1 h, 2 h, 4 h, max
  24 h).
- **Mapping to the statement:**
  - Each Account → a section with account identity `simplefin:` + connection id + `:`
    + `conn_id` + `:` + `id` (hashed like OFX account keys), display name
    "<connection name> — <account name>", currency (ISO code; a custom-currency URL →
    section rejected with a note), closing balance `balance` at `balance-date` (and
    available balance) for statement capture and reconciliation.
  - Each Transaction → a raw row: `external_id` = `id`, date = the local date of
    `transacted_at` when present, else of `posted` (setting
    `bank-feeds.date_source`: `posted` (default) or `transacted`), amount = `amount`
    (positive = money in, matching the pipeline's sign), payee = `description`
    (cleaned per R3), memo empty. Pending transactions (`pending: true` or
    `posted: 0`) are staged only when `include_pending` and are marked pending; a
    pending row is replaced when its posted version arrives (same id) or dropped when
    it disappears from a later fetch.
  - `errlist` entries → shown on the connection (`con.*` → that connection needs
    attention; `act.*` → that account; `gen.*` → the whole connection) with the
    provider's `msg` cleaned and shown as plain text. v1 `errors` strings are shown the
    same way.
  - `extra` objects are ignored (kept in `raw` only up to 4 KB).
- **v1 compatibility:** v1 responses carry an `org` object per account instead of
  `connections`/`conn_id`; the org's domain/name stand in for the connection.

### Commands

| Command | MCP | Notes |
|---|---|---|
| `connect.providers` | yes | Lists providers and their status |
| `connect.add` {provider, setup_input, label} | **no** (handles a secret) | Claims and stores the credential; lists remote accounts |
| `connect.list` / `connect.get` | yes | No secrets; host only |
| `connect.link_account` {connection, remote_account, account or none, sync_from, include_pending, auto_commit} | yes | |
| `connect.sync` {connection or all, `now`: bool} | yes | Fetches and **stages** an import batch per connection (source "SimpleFIN — <label>"); with `auto_commit` on a link and no row needing review, commits those rows (as `import.auto_commit` does for watched folders); returns batch ids, counts, errors |
| `connect.disconnect` {connection} | **no** | Deletes the credential; links kept (inactive) for history |
| `connect.set_label`, `connect.disable` / `enable` | yes | |

All mutating commands are audited (without secrets) and undoable where they change
book data (staging is undone by discarding; commits as usual).

### Scheduling

- The desktop app runs `connect.sync` for active connections on book open (if the
  last sync is older than `bank-feeds.sync_interval_hours`, default 12) and then on
  that interval while open. No background service runs when the app is closed.
- The CLI gains `moneykeep sync PATH` (for the user's own scheduler) and `watch`
  includes feeds when `--feeds` is given.
- Home's Attention list shows staged feed batches ("SimpleFIN: 12 new transactions to
  review") and connections needing attention.

## Settings

| Key | Type | Default |
|---|---|---|
| `bank-feeds.sync_interval_hours` | int 1–168 | 12 |
| `bank-feeds.date_source` | `posted` / `transacted` | `posted` |
| `bank-feeds.history_days_first_sync` | int 1–730 | 90 |

## Ledger effect

None directly; feeds create import batches.

## Edge cases

- The same bank account downloaded by file and by feed: fingerprints differ (OFX
  FITID vs SimpleFIN id), so the second source's rows **match** (amount/date) rather
  than duplicate; review shows "Matches existing".
- A remote account disappears from responses: its link shows "not seen since <date>";
  nothing is deleted.
- Clock skew: dates are derived from epoch seconds in the user's local time zone at
  sync time (setting not needed); `balance-date` is shown with its time.
- Book copied to another computer: OS-store credentials are not present there → the
  connection shows "credential not found on this computer — reconnect".

## Acceptance tests

All network tests use a local fake SimpleFIN server (an HTTP server in the test
process on `127.0.0.1` with a self-signed certificate trusted only by the test) or
recorded responses through the injected fetcher; no test touches the internet.

1. A setup token decoding to `https://bridge.test/simplefin/claim/abc` is claimed with
   POST; the returned Access URL is stored in the OS store (test double) or, for an
   encrypted book, in the book; `connect.get` shows host `bridge.test` and no
   credentials anywhere in its JSON; the audit log contains no credential text.
2. A token decoding to `http://…`, to a URL without `/claim/`, or to non-URL text is
   rejected before any request; a 403 claim shows the "already used" message.
3. `GET /info` returning `{"versions":["1","2"]}` selects version 2.
4. A v2 Account Set with two connections, three accounts (one in EUR), five
   transactions (one pending) stages: two sections in USD, the EUR one rejected only
   if the linked account's commodity differs; four rows (pending excluded); amounts and
   descriptions as given; balances captured with `balance-date`.
5. Re-syncing with an overlapping window stages the previously committed rows as
   duplicates and only new ones as new.
6. A pending transaction staged with `include_pending` is replaced by its posted
   version on the next sync (same `id`), not duplicated.
7. 402 → connection `needs_attention` with the payment message; 403 → `revoked`.
8. `errlist` with `con.auth` for one connection marks that connection's accounts
   needing attention and shows the cleaned `msg`; a `msg` containing control and bidi
   characters is shown cleaned.
9. A redirect to another host is refused; a response of 25 MB is refused; JSON nested
   100 deep is refused.
10. `auto_commit` on a link commits clean rows and leaves a row matched by no rule
    (uncategorised) for review only when `import.auto_commit` rules say so (same
    behaviour as watched folders).
11. The MCP tool list contains `connect.sync` and `connect.list` but not
    `connect.add` or `connect.disconnect`; `connect.sync` output contains no
    credentials.
12. The xtask check fails if `mk-commands` gains an HTTP dependency.
13. A v1 response (accounts with `org`) stages the same way as v2.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-10-21) — responsiveness and claim safety

Review of the implementation found two problems:

1. **No network inside a book write.** `connect.sync` and `connect.add` must perform
   their HTTP requests **before** opening the book's write transaction: fetch (or
   claim) first, holding no lock on the book, then stage/save in a short write. If
   the book changed meanwhile, staging works as usual (fingerprints and matching
   handle it). The desktop app runs syncs in the background: the window stays
   responsive, the connection shows "Syncing…" with a spinner, other commands
   continue (they queue behind only the short write), and a finished sync updates
   Home Attention. The user can cancel a running sync; cancelling discards the fetched
   data and records nothing.
2. **Claim safety.** After a successful claim, the Access URL is written to the
   credential store **immediately** and, if anything later fails (book save, account
   listing), the dialog keeps the connection as "Not finished — resume" with the
   stored credential so the user can finish linking without a new token. A credential
   with no connection record after 24 hours is listed under Settings → Connections as
   "Unused credential" with Delete.

### Additional acceptance tests

14. With a fake server that delays `/accounts` by 5 s, another command (e.g. a
    register query) issued during the sync completes in under 1 s.
15. Cancelling a sync during the delay records no batch and no audit entry.
16. A failure injected after the claim leaves a resumable connection whose credential
    is in the store; resuming lists accounts without a new claim.

## Revision 3 (2026-10-22) — transaction dates by default

Owner request (`app-desktop.md` revision 21): imported dates should match the bank's
register, which lists transaction dates.

- `bank-feeds.date_source` now defaults to **`transacted`**. A book that never set it
  gets the new default; a book that explicitly chose `posted` keeps it.
- A row dated from `transacted_at` has `date_kind` `transaction`; a row dated from
  `posted` (because `transacted_at` is absent or the setting is `posted`) has
  `posted`. Rows already committed with the posting date are corrected by the date
  correction rule (`import-pipeline.md` revision 2) when they are fetched again.
- `connect.sync` gains optional `from` (date): that run's `start-date` is `from`
  instead of the usual window (still limited by what the provider returns). The
  desktop app's **Fix dates of imported transactions…** uses the earliest date
  imported for the connection's linked accounts.

Acceptance tests:

17. With no explicit setting, a transaction with `transacted_at` two days before
    `posted` stages dated `transacted_at` (local date).
18. A connection whose committed rows were dated by `posted`: `connect.sync` with
    `from` = the earliest imported date stages them as date corrections; commit moves
    them; reconciled ones stay duplicates with the note.

## Revision 4 (2026-11-05) — account kind when linking, and finding staged feed batches (owner report)

1. **Kind when creating accounts.** In the link step, each remote account set to
   "Create a new account" shows a **Kind** choice: Bank, Credit card, Loan, Investment
   (Investment only for providers that report holdings, as today). The default is the
   provider's hint when it gives one (OFX account type, SimpleFIN `org`/name containing
   "card", "visa", "mastercard", "amex", "discover" or "credit" → Credit card; "loan",
   "mortgage" → Loan), else Bank. The account is created with that type and subtype.
2. **Finding what waits for review.** Besides Home's Attention block:
   - Settings → Connections shows on each connection with staged rows "N new
     transactions to review" with a **Review** button opening that batch.
   - A register whose account has staged, uncommitted feed rows shows a band above the
     rows: "N transactions from SimpleFIN wait for review" with **Review**.
   - The sync result notice ("SimpleFIN: N new transactions to review.") has a
     **Review** action.

Acceptance tests:
18. Linking a SimpleFIN account named "Chase Sapphire Visa" with Create a new account
    defaults Kind to Credit card and creates a liability/credit-card account.
19. After a sync stages rows for account A, Settings → Connections and A's register
    both offer Review, which opens the batch.

## Revision 5 (2026-11-06) — provider notices can be dismissed; SimpleFIN 90-day cap (owner report)

Owner report: Home → Attention showed "SimpleFIN: Requested date range exceeds limit of
90 days and was capped." with no way to close it.

1. **No over-long requests.** SimpleFIN returns at most 90 days of transactions per
   request. A SimpleFIN fetch never asks for a start date more than 90 days before
   today: an earlier start (first sync history, a link's `sync_from`, an explicit
   `from`) is clamped to today − 90 days, and the sync result says once "SimpleFIN
   gives at most 90 days of history; older transactions can be imported from a file."
   Other providers keep their own limits.
2. **Informational notices don't flag the connection.** An `errlist` entry whose message
   says a request was capped or adjusted (code `gen.*` with "capped"/"limit" in the text,
   or any entry the provider marks informational) is shown in that sync's result only;
   it does not set the connection to "Needs attention" and does not appear under
   Attention. Real failures (authentication, connection errors, `con.*`/`act.*`) keep
   today's behaviour.
3. **Dismiss.** Every Attention item of kind `connection` (a connection or link message)
   has a **Dismiss** action (`connect.dismiss {connection, link?, message}`). The
   dismissed message is stored on the connection (or link); that item no longer appears
   until a later sync produces a different message for it, or the same failure occurs
   again after a successful sync in between. Dismissing does not change the connection's
   status shown in Settings → Connections, except that a "Needs attention" status that
   came only from the dismissed message returns to Active. Dismiss is undoable and
   audited. Items that are work to do (batches to review, holdings that differ) are not
   dismissible: they leave when the work is done.

Acceptance tests:
20. A SimpleFIN link with `sync_from` 200 days ago fetches with start-date today − 90.
21. A fetch whose errlist holds only "Requested date range exceeds limit of 90 days and
    was capped." leaves the connection Active and adds no Attention item; the sync result
    shows the note.
22. A connection message ("Authentication failed") dismissed from Attention disappears;
    the next sync with the same failure and no success in between keeps it hidden; a
    sync that succeeds and then a sync that fails again shows it again. Undo restores it.
