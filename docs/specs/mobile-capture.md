# Spec: Phone capture — capture server, WASM client, Cloudflare Tunnel and Pages (Phase 6b)

- **Area / module:** `expenses` (phone capture); new apps `moneykeep-server` and
  `moneykeep-capture` (WASM); shared crate `mk-capture-api`
- **Spec writer:** Claude session 2026-10-23 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-23
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Vendor doc | Cloudflare Tunnel (cloudflared) documentation — public hostnames routed to a local service | The server listens on loopback; `cloudflared` connects outward, so no port is opened on the router |
| 2 | Vendor doc | Cloudflare Access — CORS (https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/cors/) | Browsers send no cookies on preflight, so Access blocks cross-origin preflights; recommended alternative: a Worker on the calling domain's `/api/*` path that forwards with a service token |
| 3 | Vendor doc | Cloudflare Access — Validate JWTs (https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/validating-json/) | Header `Cf-Access-Jwt-Assertion`; certs at `https://<team>.cloudflareaccess.com/cdn-cgi/access/certs`; check `iss` = team domain, `aud` = application AUD tag, expiry; RS256; keys rotate (every 6 weeks by default, old keys valid 7 days) — fetch at runtime, match `kid` |
| 4 | Vendor doc | Cloudflare Pages and Pages Functions (`functions/` directory routing, environment secrets), Access service tokens (`CF-Access-Client-Id`, `CF-Access-Client-Secret` headers) | |
| 5 | Standard | HTML Living Standard — `input type=file`, `accept`, `capture` (`environment` = rear camera on supporting phones; ignored elsewhere, falling back to a file picker), `multiple`; IndexedDB; Fetch; Canvas `toBlob` | |
| 6 | Standard | RFC 9110 (HTTP), RFC 6750 (bearer tokens), RFC 7519 / 7515 (JWT/JWS), RFC 8259 (JSON), CSP Level 3 | |
| 7 | Library docs | eframe/egui web (wasm32) documentation, wasm-bindgen / web-sys documentation, Trunk documentation | |
| 8 | Spec | `expenses.md`, `connectivity.md` (secrets rules), `input-safety.md`, `book-encryption.md` | |
| 9 | Owner request | 2026-10-23: WASM client on Cloudflare Pages via Cloudflare Tunnel; camera and device photos; manual entry on the phone; single submitter; always-on server; OCR later. Owner already runs Cloudflare Tunnel at home | |

No target product was observed.

## Purpose

Let the owner photograph receipts and enter expenses on a phone anywhere; the
expenses wait safely at home until the desktop app collects them into the book.

## Architecture

```
Phone browser ──HTTPS──> Cloudflare Pages site (WASM client + /api/* Function)
                               │  (Access login; Function adds service token)
                               ▼
                    Cloudflare Access ──> Tunnel hostname ──> cloudflared (home)
                                                                  │ loopback
                                                                  ▼
                                                  moneykeep-server (127.0.0.1:8787)
                                                                  ▲  desktop token
                                       MoneyKeep desktop app ────┘  (collects, publishes lists)
```

- **The book never leaves the desktop.** The server holds only an **inbox** of
  submissions (expense fields + receipt images) and a small **lists** document the
  desktop publishes (category names, paid-with choices, recent merchants). The
  server never opens a book and needs no book password.
- Everything the phone sends is a *proposal*: it becomes a draft/ready expense in the
  book only when the desktop collects it, and posts only through `expenses.md` rules.

### Crates and apps

- `crates/mk-capture-api`: the request/response types (serde), limits and validation
  shared by server, desktop and client. No IO. Compiles for wasm32.
- `apps/moneykeep-server`: headless binary. Only crate besides `mk-connect` and the
  apps allowed to do network IO (xtask layering/denied-crate rules extended). It does
  not depend on `mk-store` or `mk-commands`.
- `apps/moneykeep-capture`: eframe web app (wasm32-unknown-unknown) built with Trunk
  into `dist/`, plus `web/functions/api/[[path]].js` (the Pages Function) and
  `web/_headers` (security headers). Depends on `mk-capture-api` only (and egui,
  eframe, wasm-bindgen, web-sys, js-sys).

## Behaviour

### Capture server (`moneykeep-server`)

- **Commands:** `moneykeep-server init --data DIR` (creates the data folder, the
  inbox store, a random **desktop token** printed once, and a config file);
  `moneykeep-server run --data DIR`; `moneykeep-server rotate-desktop-token`;
  `moneykeep-server status`. Runs as a foreground process suitable for a Windows
  service wrapper or systemd; the deploy guide shows both.
- **Listen address:** `127.0.0.1:8787` by default; any non-loopback address requires
  `--allow-lan` and prints a warning. Plain HTTP only on loopback/LAN; TLS is
  Cloudflare's job.
- **Config** (`server.toml`): `access_team_domain`, `access_aud` (required unless
  `access_required = false`, which is allowed only when listening on loopback and is
  meant for local testing), `max_receipt_mb` (default 10), `max_receipts_per_expense`
  (10), `inbox_limit_mb` (default 1,000).
- **Store:** a single SQLite file in the data folder. Receipt bytes and expense fields
  are encrypted at rest with XChaCha20-Poly1305 (same primitives as
  `book-encryption.md`) under a 256-bit key kept in the OS credential store, or, where
  none exists (headless Linux), in `inbox.key` in the data folder with owner-only
  permissions (the guide says to protect the folder). Items are deleted once the
  desktop acknowledges them.

#### Authentication — every request must pass all that apply

1. **Cloudflare Access JWT** (`Cf-Access-Jwt-Assertion`): verified per source 3 —
   RS256 signature against the team's certs (fetched over HTTPS from the team domain,
   cached, refreshed on unknown `kid` at most once a minute), `iss`, `aud`, `exp`/`nbf`
   with 60 s leeway. Missing or invalid → 401 with no detail beyond "Not signed in".
2. **Bearer token** (`Authorization: Bearer …`, RFC 6750): either the **desktop token**
   (desktop API) or a **device token** (phone API). Tokens are 256-bit random,
   Base64URL; the server stores only SHA-256 hashes and compares in constant time.
3. Rate limits per token: 60 requests/min, 200 MB/day uploads; failed auth: 10/min per
   source address, then 15-minute backoff.

#### Phone API (`/api/v1`, device token)

| Method & path | Purpose |
|---|---|
| `POST /pair` {code, device_name} | Exchanges a one-time pairing code (see desktop) for a device token. Code: 8 characters from an unambiguous alphabet, valid 10 minutes, single use, 5 wrong tries invalidate it. No bearer token needed (Access JWT still required) |
| `GET /lists` | Categories, paid-with choices, recent merchants (≤ 200), tag lists, base currency, book profile name |
| `POST /receipts` (body: image bytes; `Content-Type` image/jpeg or image/png; ≤ max_receipt_mb) | Stores an uploaded receipt; returns `receipt_id`. The server decodes, auto-rotates, **strips all metadata incl. GPS**, limits to 2,400 px, re-encodes (as `expenses.md`); refuses anything else with a clear message |
| `POST /expenses` {client_id (UUID), date, merchant, amount, category?, paid_with?, purpose?, tags?, receipt_ids[]} | Creates a submission; `client_id` makes retries idempotent. Fields validated by `mk-capture-api` (lengths, amount > 0 with ≤ 2 decimals, date within ±400 days of today) |
| `GET /expenses` | This device's submissions not yet collected, plus the last 50 collected (status `waiting` / `collected`, collected time) |
| `PATCH /expenses/{id}`, `DELETE /expenses/{id}` | Only while `waiting` |
| `DELETE /receipts/{id}` | Only while not part of a collected submission |

Receipt-only submissions (photos without fields) are allowed: they become inbox
receipts on the desktop.

#### Desktop API (`/api/v1/desktop`, desktop token)

| Method & path | Purpose |
|---|---|
| `PUT /lists` | Publishes the lists document (from the open book) |
| `GET /inbox` | Waiting submissions with receipt metadata |
| `GET /receipts/{id}` | Receipt bytes |
| `POST /inbox/{id}/ack` | Marks collected; deletes stored bytes after 24 h (kept briefly so a failed desktop commit can retry) |
| `POST /pairing-codes` | Creates a pairing code |
| `GET /devices`, `DELETE /devices/{id}` | Lists paired phones (name, paired, last seen), revokes one |

- Responses never include tokens or keys; logs record method, path template, status
  and timing only (no bodies, no tokens, no amounts or merchants).

### Desktop app

- Settings → **Phone capture** (shown while `expenses` is on): **Server address**
  (`https://…` or `http://127.0.0.1:…`/LAN with a "This network is trusted" tick),
  **Desktop token** (masked; stored per `connectivity.md` secrets rules), **Test
  connection**, **Pair a phone…** (shows the pairing code large, with its expiry
  countdown and the client's address to open), **Paired phones** list with Revoke,
  **Collect automatically** (on book open and every 15 minutes while open; default
  on).
- **Collect:** downloads waiting submissions, creates expenses (`source` phone; status
  `ready` when complete, else `draft`) and receipts (inbox if no fields) in one book
  transaction, then acknowledges them. Names are resolved against the book: unknown
  category/paid-with → left empty (draft) with a note "Category from phone not found:
  <text>". Collected items show in Expenses with a "From phone" badge and Home
  Attention "N expenses from your phone to review".
- **Publish lists** after collecting and whenever accounts change (debounced 1 min).
- Commands: `capture.settings`, `capture.set_server` (MCP no — token), `capture.test`,
  `capture.collect`, `capture.pair_code` (MCP no), `capture.devices`,
  `capture.revoke_device`. Network calls happen before the book write (as
  `connectivity.md` revision 2).

### WASM client (`moneykeep-capture`)

- Phone-first layout (portrait, 360–430 px wide, also usable on tablets/desktop
  browsers); text size follows the browser; tap targets ≥ 44 px.
- **Pair screen** (first run): "Enter the code shown in MoneyKeep on your computer",
  8-character field, device name (default from the browser), Pair. The device token is
  stored in IndexedDB; **Sign out** on the menu deletes it.
- **Home:** three large buttons — **Take photo** (opens the rear camera via a file
  input with `accept="image/*"` and `capture="environment"`), **Choose photos** (file
  input `accept="image/*" multiple`, no capture), **Enter expense** (form without a
  photo) — and below them the **Outbox** and **Sent** lists.
- **Expense form:** photo strip (thumbnails, add more, remove), Date (defaults to
  today; date picker), Merchant (suggestions from recent merchants), Amount (numeric
  keypad, `inputmode="decimal"`), Category and Paid with (pickers from the lists;
  optional), Purpose (optional), tags when published. **Send** (primary) and **Save
  for later**.
- **Before upload** the client scales photos so the longer side is ≤ 2,400 px and
  re-encodes as JPEG (quality 0.85) via a canvas, which also drops metadata; HEIC or
  unsupported types show "This photo type isn't supported — set your camera to save
  JPEG (Most Compatible)". The server re-checks everything anyway.
- **Offline-first:** every submission is saved to IndexedDB first (Outbox) and sent
  when online (on load, on `online` events, and every 60 s while open); retries with
  backoff; each expense keeps its `client_id` so a retry never duplicates. Outbox items
  can be edited or deleted before sending.
- **Sent:** status from `GET /expenses` (Waiting for your computer / Collected on
  <date>); waiting items can be edited or deleted.
- No third-party scripts, analytics or fonts from other hosts. `web/_headers` sets:
  `Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval';
  img-src 'self' blob: data:; connect-src 'self'; style-src 'self' 'unsafe-inline';
  frame-ancestors 'none'; base-uri 'none'; form-action 'none'`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`,
  `Permissions-Policy: camera=(self), geolocation=()`.
- **Pages Function** `web/functions/api/[[path]].js`: forwards `/api/*` requests
  (method, path, query, body, `Content-Type`, `Authorization`) to
  `CAPTURE_API_ORIGIN` (the tunnel hostname, env var) adding `CF-Access-Client-Id` and
  `CF-Access-Client-Secret` from Pages secrets; strips `Cookie` and `Set-Cookie`;
  refuses bodies over 12 MB; only forwards to that one origin. The browser therefore
  talks only to its own origin (no CORS).

### Deployment guide

`docs/deploy/phone-capture.md` (written by the implementer from the vendor docs, in
plain steps): install and init the server; run it as a service (Windows and Linux);
create a Cloudflare Tunnel public hostname (e.g. `capture-api.<domain>`) to
`http://127.0.0.1:8787`; protect **both** the Pages site and the API hostname with
Cloudflare Access (an Allow policy for the owner's email; a Service Auth policy for the
service token on the API app); create the service token; deploy `dist/` + `web/` to
Pages (custom domain e.g. `capture.<domain>`), set `CAPTURE_API_ORIGIN`,
`CF_ACCESS_CLIENT_ID`, `CF_ACCESS_CLIENT_SECRET` as Pages secrets; put the team
domain and API AUD tag in `server.toml`; set the desktop's server address and token;
pair the phone; add to home screen. Also: how to revoke a lost phone, rotate tokens,
and what to back up (nothing essential: the inbox is temporary).

## Settings

| Key | Where | Default |
|---|---|---|
| `capture.server_url`, desktop token | book's secrets (per connectivity) | none |
| `capture.auto_collect` | module `expenses` | true |
| `capture.collect_minutes` | module `expenses`, 5–240 | 15 |

## Edge cases

- Desktop closed for weeks: the inbox keeps items (up to `inbox_limit_mb`, then
  uploads get 507 "Storage full — open MoneyKeep on your computer to collect").
- Server clock skew beyond JWT leeway: 401 with a log line about the clock.
- A phone revoked while offline: its outbox stays on the phone and shows "This phone
  was disconnected — pair again to send" on the next try.
- Lists not yet published: the form still works with free text; the desktop resolves
  names on collect.
- Several books on one desktop: the server serves one book's lists; collecting into a
  different book is allowed but warns when the lists came from another book (book id
  stored with the lists).

## Acceptance tests

Server (integration tests against an in-process server on loopback with a test RSA
key and a fake certs endpoint; no internet):

1. A request without `Cf-Access-Jwt-Assertion`, with a bad signature, wrong `aud`,
   wrong `iss` or expired token → 401; a valid JWT plus valid device token → 200.
2. Unknown `kid` triggers one certs refresh; a rotated key is accepted after refresh.
3. Pairing: a code exchanges once for a device token; reuse, expiry (10 min) and the
   6th wrong try fail; the stored token is only a hash (DB file scanned for the token
   text).
4. Revoking a device makes its next request 401.
5. Uploading a JPEG with GPS EXIF stores an image with no EXIF; a 12 MB body → 413; a
   PNG decompression bomb → 422 "This picture can't be read."
6. `POST /expenses` twice with the same `client_id` creates one submission.
7. Field validation: amount 0, amount 1.005, a 501-char purpose and a date 2 years
   ahead are refused with messages.
8. Inbox data at rest: the SQLite file does not contain a known merchant string or the
   JPEG's bytes in clear.
9. Binding to `0.0.0.0` without `--allow-lan` refuses to start; `access_required =
   false` on a non-loopback address refuses to start.
10. Logs from a full submission contain no token, merchant or amount.

Desktop:

11. `capture.collect` against the test server creates one ready expense with its
    receipt and one inbox receipt for a photo-only submission, acknowledges both, and
    a second collect creates nothing.
12. An unknown category from the phone creates a draft with the note.
13. Lists published contain expense categories and paid-with choices but no balances,
    account numbers or transactions.
14. MCP does not list `capture.set_server` or `capture.pair_code`.

Client (logic tests run natively; plus a wasm32 build in CI):

15. The outbox keeps a submission across a reload (IndexedDB layer mocked behind a
    trait) and sends it when online; a failed send retries with backoff and the same
    `client_id`.
16. Photo scaling chooses ≤ 2,400 px on the longer side and keeps aspect.
17. `cargo build -p moneykeep-capture --target wasm32-unknown-unknown` succeeds in
    `cargo xtask ci` (skipped with a clear message only if the target cannot be
    installed in the CI environment).
18. The Pages Function forwards only to `CAPTURE_API_ORIGIN`, adds the two service
    token headers, strips cookies and refuses a 13 MB body (tested with a small Node or
    Workers-compatible test runner if available; otherwise reviewed and documented as
    untested).
19. `web/_headers` contains the CSP and other headers exactly as listed.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-10-26) — one address, guided setup, status light (owner request)

Sources added: Cloudflare API — create a remotely-managed tunnel
(https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/get-started/create-remote-tunnel-api/):
`POST /accounts/{account}/cfd_tunnel` (`config_src: cloudflare`, returns id and
token), `PUT …/cfd_tunnel/{id}/configurations` (ingress list ending with a catch-all),
proxied CNAME `<id>.cfargotunnel.com` via `POST /zones/{zone}/dns_records`,
`cloudflared service install <token>`, status via `GET …/cfd_tunnel/{id}`; token
permissions Cloudflare Tunnel Edit and DNS Edit. Cloudflare Pages Direct Upload
(drag-and-drop cannot deploy a `functions` folder — the reason Pages is dropped).
Cloudflare Access application, policy and service-token endpoints and the
organization's team domain (`/accounts/{account}/access/apps`, `…/policies`,
`…/service_tokens`, `…/access/organizations`) from the Cloudflare API reference.
Owner decisions 2026-10-26: serve the phone app from the capture server; the server
runs on the same PC as MoneyKeep; delete the Cloudflare token after setup by default.

### A. One address — Cloudflare Pages removed

- The capture server serves the phone app itself (the WASM client's files embedded in
  the server program at build time) at `/` on the same hostname as the API. There is
  one public hostname (default `capture.<domain>`), one Access application, no Pages
  project, no Pages Function, no `CAPTURE_API_ORIGIN`, and no service token between
  Pages and the server. Requests are same-origin, so there is no CORS.
- The security headers of `web/_headers` are sent by the server on every page
  response (unchanged values; `connect-src 'self'` now means the same hostname).
- Phone requests still need the Access JWT and the device token, as before.
- AT18 (Pages Function) is withdrawn; AT19 now checks the server's response headers.
- `web/functions/` and `web/_headers` are removed; the deploy guide is rewritten for
  the guided setup (section B) with a manual fallback.

### B. Guided setup (desktop)

Turning on **Phone capture** (Settings → Phone capture → **Set up phone capture…**)
opens a step-by-step dialog. Each step has one primary button that does the work and
shows a check mark or a plain error with **Try again**. Steps:

1. **Before you start** (text and links only): you need your domain on Cloudflare,
   Zero Trust turned on once in the Cloudflare dashboard (team name, free plan), and
   an **API token** created with these permissions — shown as a checklist with a
   **Copy list** button: Account · Cloudflare Tunnel · Edit; Account · Access: Apps and
   Policies · Edit; Account · Access: Service Tokens · Edit; Account · Access:
   Organizations, Identity Providers, and Groups · Read; Zone · DNS · Edit (for the
   chosen zone); Zone · Zone · Read. Button **Open Cloudflare token page** (opens the
   browser).
2. **Connect to Cloudflare:** paste the token (masked). **Check token** verifies it,
   lists the accounts and zones it can see (pick if more than one) and reports any
   missing permission by name.
3. **Choose the address and who can sign in:** subdomain (default `capture`), shown
   as the full address; email address(es) allowed (default the user's email if known,
   else empty, required). The address must not already exist in DNS unless it was
   made by MoneyKeep.
4. **Where does your tunnel run?** MoneyKeep lists the account's existing tunnels.
   Choices:
   - **Create a new tunnel on this computer** (default when none is running here);
   - **Add to my existing tunnel <name>** — then: **This computer** or **Another
     device on my network** (e.g. a router or server). For another device, the server
     must listen on the computer's LAN address (it is set to `--allow-lan` on that one
     address only) and the ingress service becomes `http://<this computer's LAN
     address>:8787`; the dialog shows which address it detected and lets the user
     change it.
5. **Install:** **Install** asks Windows for permission once (UAC) and then:
   - installs `moneykeep-server` as a background service that starts with the
     computer (data folder under the user's application data, owner-only), runs
     `init`, keeps the desktop token in MoneyKeep's secrets;
   - for a new tunnel: creates it through the API, and installs `cloudflared` as a
     service with the tunnel token — using an existing `cloudflared` installation if
     present, otherwise downloading the official Windows installer from Cloudflare's
     GitHub releases over HTTPS and refusing it unless its Authenticode signature is
     valid and issued to Cloudflare; an existing `cloudflared` service running a
     different tunnel is never replaced (the user is told to choose "Add to my
     existing tunnel");
   - sets the tunnel's ingress (keeping existing rules of an existing tunnel, adding
     the new hostname before the catch-all) and creates the proxied DNS record;
   - creates the Access application for the hostname with an Allow policy for the
     emails, reads its AUD tag and the team domain, and writes both into
     `server.toml`;
   - creates an Access **service token** "MoneyKeep desktop status" with a Service
     Auth policy on the same application, stored in MoneyKeep's secrets; the server
     accepts a service-token identity only for `GET /api/v1/health` and the desktop
     API (which still also needs the desktop token). This lets the desktop reach the
     server through the public address for the status check and when away from home.
   On Linux/macOS the same steps use systemd/launchd and an existing `cloudflared`
   (download not attempted; the dialog says how to install it).
6. **Check:** runs the status checks (section C) and shows the light.
7. **Pair your phone:** shows a **QR code** of `https://<address>/#pair=<code>` with
   the 8-character code under it and the countdown. The fragment is never sent to the
   server; the phone app reads it and fills the code. Scanning opens the page, the
   phone signs in with Cloudflare's email code once, and pairing completes.
8. **Finish:** a tick box **Delete Cloudflare token — each change requires a new
   token**, ticked by default. Ticked: MoneyKeep erases its stored copy of the API
   token and shows a link to Cloudflare's token page to revoke it there too (a token
   can only delete itself at Cloudflare with extra permissions, which are not asked
   for). Unticked: the token stays in MoneyKeep's secrets for later changes.

Later changes (address, emails, remove phone capture) start the same dialog at the
relevant step and ask for a token when none is stored. **Remove phone capture**
deletes, through the API, what setup created (DNS record, ingress rule or tunnel,
Access application, policies, service token) and uninstalls the services, after a
confirmation listing them.

Each API call is logged in the setup dialog's **Details** list (method, path
template, result) without tokens. Cloudflare API errors are shown with their message.
Network calls never happen inside a book write.

### C. Status light

- A round light in the top bar to the left of the Calculator button (revision 25),
  shown while phone capture is set up, plus a large one at the top of Settings →
  Phone capture.
- Checks every 60 s while the app is open (and on **Check now**):
  1. **Server:** `GET /api/v1/health` on the local address → running, version.
  2. **Internet path:** `GET https://<address>/api/v1/health` with the service token →
     Cloudflare, Access, the tunnel and the server all work.
  3. **Collect:** the last collect succeeded (or none was due).
- Colours (with shapes and text for colour-blind users, WCAG 1.4.1):
  - **Green** (filled circle) "Phone capture is working" — 1 and 2 pass, 3 not failing.
  - **Yellow** (circle with "!") — something is partly wrong; the tooltip names it:
    "The internet address isn't reaching your computer (tunnel down?)", "Collecting
    from the server failed: <reason>", "Server version differs from the app".
  - **Red** (circle with "×") "Phone capture isn't working: the server isn't
    running" (1 fails) — with **Start server** when the service exists but is
    stopped.
  - **Grey** (hollow circle) "Phone capture is off".
- Tooltip also shows: last phone activity (time of the newest submission received),
  items waiting, last check time. Clicking the light opens Settings → Phone capture.
- Accessible name "Phone capture status: <state text>".
- `GET /api/v1/health` returns only `{"ok": true, "version": "…"}`; no counts or names
  without the desktop token. Waiting count and last activity come from
  `GET /api/v1/desktop/status` (desktop token).

### D. Building

`cargo xtask dist-capture` (new) installs the wasm32 target and Trunk if missing,
builds the phone app, and builds `moneykeep-server` with it embedded. The deploy
guide and the setup dialog say to run it once before setup when the server program is
missing. A server built without the embedded app serves a page saying "The phone app
wasn't built into this server — run cargo xtask dist-capture".

### Commands added

`capture.setup_check_token` {token} (MCP no), `capture.setup_plan` {…} (dry run: what
will be created), `capture.setup_apply` (MCP no), `capture.setup_remove` (MCP no),
`capture.status` (MCP yes), `capture.forget_cloudflare_token` (MCP no).

### Acceptance tests (Cloudflare API and services faked; no internet)

20. Check token reports missing permissions by name and lists accounts and zones.
21. Setup with "Create a new tunnel": calls create tunnel, configurations (hostname
    then catch-all), DNS CNAME to `<id>.cfargotunnel.com` proxied, Access app, Allow
    policy with the emails, service token and Service Auth policy; writes AUD and team
    domain to `server.toml`; installs both services (fake service manager).
22. "Add to my existing tunnel" keeps the tunnel's existing ingress rules and inserts
    the new hostname before the catch-all; "another device" uses the LAN address.
23. An existing `cloudflared` service with a different tunnel is not replaced.
24. A downloaded `cloudflared` installer with an invalid signature is refused.
25. Finish with Delete Cloudflare token ticked erases the stored token; unticked keeps
    it; the token never appears in logs, config or the Details list.
26. The QR code decodes to `https://<address>/#pair=<code>`; the phone app reads the
    fragment and pre-fills the code; the fragment is not in any server request.
27. Status: local health ok + public health ok → green; public fails → yellow with the
    tunnel text; local fails → red with Start server; feature off → grey; accessible
    name matches.
28. A service-token identity can call `/api/v1/health` and the desktop API (with the
    desktop token) but not the phone API.
29. The server serves the phone app at `/` with the security headers; without an
    embedded app it serves the explanatory page.
30. Remove phone capture deletes exactly what setup created and uninstalls the
    services.

## Revision 3 (2026-10-27) — installable app (PWA) and lasting pairing (owner report)

Owner report: after closing the phone's browser window, the phone app asks to pair
again. Owner request: make it an installable app (PWA) whose pairing lasts.

Sources: W3C Web Application Manifest; W3C Service Workers; WHATWG Storage Standard
(`navigator.storage.persist()`, persistent vs best-effort storage); browser vendors'
install requirements (manifest with name, icons of 192 and 512 px, `start_url`,
`display`, served over HTTPS); Cloudflare Access application `session_duration`.

### Installable app

- `manifest.webmanifest`: `name` "MoneyKeep capture", `short_name` "Capture",
  `id` "/", `start_url` "/", `scope` "/", `display` "standalone", `orientation`
  "portrait", theme and background colours, and **icons** 192 × 192 and 512 × 512 PNG
  (`purpose` "any" and a "maskable" 512 version) plus a 180 × 180 `apple-touch-icon`
  linked from `index.html` (with `apple-mobile-web-app-capable` and title metas). The
  icons are the MoneyKeep calculator drawing (`app-desktop.md` revision 25), generated
  at build time by the server build (or committed as generated files with their
  generator) — no third-party artwork.
- A **service worker** `sw.js` (scope `/`): caches the app shell (index.html, start.js,
  the wasm module and its JS glue, manifest, icons) on install under a versioned cache
  name; serves the shell cache-first so the app opens offline and the outbox works;
  never caches `/api/*` requests or responses; deletes old caches on activate.
  Registered from `start.js` after load.
- The server serves `manifest.webmanifest` as `application/manifest+json` and
  `sw.js` with `Cache-Control: no-cache`; the CSP gains `worker-src 'self';
  manifest-src 'self'`.
- The app shows an **Install** hint once on the home screen when the browser offers
  installation (`beforeinstallprompt`: an **Install app** button); on iPhone/iPad
  (no such event) it shows "To install: tap Share, then Add to Home Screen". Dismissible;
  not shown when already running installed (`display-mode: standalone`).

### Lasting pairing

- After pairing (and at each start when paired), the app calls
  `navigator.storage.persist()` so the browser keeps its storage; the result is
  remembered and shown in the menu's **About this phone** ("Storage: kept" / "Storage:
  may be cleared by the browser").
- The device token is written to IndexedDB **and the app waits for the write
  transaction to complete** before showing the paired home screen; a failed write
  shows "This browser didn't save the pairing (private browsing?)".
- The app must never forget the token except on **Sign out** or when the server
  answers `not_paired` for it. Every other failure (offline, Cloudflare Access login
  page instead of JSON, 5xx, timeouts) keeps the token and shows the matching message.
  When Access's session has ended, API calls get Access's login redirect instead of
  JSON: the app shows "Sign in to Cloudflare again" with a **Sign in** button that
  reloads the page (so Access shows its login) — the pairing is kept.
- When the app opens without a token, the Pair screen explains why when known:
  "This phone was disconnected from your computer" (server said not_paired earlier,
  remembered as a flag), or "This browser cleared the app's storage" (a marker in
  `localStorage` shows it had been paired before but IndexedDB is empty).
- **iPhone/iPad note** shown on the Pair screen when running in Safari (not
  installed): "Install the app first (Share → Add to Home Screen), then pair inside
  it — the installed app keeps its own pairing."
- **Access session length:** guided setup (revision 2) sets the Access application's
  `session_duration` to **30 days** (`720h`) so the phone rarely needs to sign in
  again; existing setups are updated by the dialog's **Repair** action (new, uses a
  token like other changes). The deploy guide's manual steps say the same.

### Acceptance tests

31. The manifest has the fields above and both icon sizes; `/manifest.webmanifest` is
    served as `application/manifest+json`; `/sw.js` with `Cache-Control: no-cache`;
    the icons decode as PNG at the stated sizes.
32. The service worker script caches only shell files and contains a rule that
    `/api/` requests go to the network (checked by a JS test with Node 22 using a
    minimal fake of the service-worker globals).
33. A Cloudflare Access login redirect (HTTP 302 to the team domain, or an HTML 200)
    in answer to `/api/v1/lists` keeps the token and shows "Sign in to Cloudflare
    again"; a 401 `not_paired` clears it and shows the disconnected message; a network
    failure keeps it.
34. Pairing waits for the storage write; a storage failure shows the private-browsing
    message; the token survives a simulated restart (fake storage reloaded).
35. The CSP header includes `worker-src 'self'` and `manifest-src 'self'`.
36. Guided setup creates the Access application with `session_duration` "720h";
    Repair updates an existing application's session duration.

## Revision 4 (2026-10-27) — updating the server, menu position (owner reports)

Owner reports: rebuilding the server failed because the running service locks
`target\release\moneykeep-server.exe` ("failed to remove file"), and stopping the
service from a normal PowerShell window is refused; the phone kept getting the old app.
Owner request: the phone app's menu button belongs at the left of the title bar.

### Server runs from its own copy

- Setup (and Repair) installs the server program into a fixed **install folder**
  (Windows: `%ProgramFiles%\MoneyKeep\capture\moneykeep-server.exe`; Linux/macOS:
  `~/.local/share/moneykeep/capture/` / `~/Library/Application Support/MoneyKeep/capture/`)
  and registers the service with that path — never the build folder. Building the
  project therefore never touches the running program.
- Existing installs whose service points at the build folder are moved by the
  **Update server** action below (it re-registers the service path).

### Update server (desktop)

- The status light turns **yellow** with "The capture server is older than this app"
  (or "…newer…") when `GET /api/v1/health`'s `version` (which includes the build's
  git commit) differs from the desktop's, with an **Update server** button in the
  tooltip and in Settings → Phone capture.
- **Update server**: finds the newly built server program (next to the desktop
  program, i.e. `target\release`, or on PATH); refuses with "Build it first: cargo
  xtask dist-capture" when missing or when it has no phone app embedded or is not
  newer; otherwise asks Windows for permission once (UAC) and, elevated: stops the
  service, waits for the process to exit (up to 30 s, then ends it), copies the new
  program into the install folder (keeping the old one as `.old` until the new one
  starts and answers health), re-registers the service path if needed, starts it, and
  checks health; on failure restores the old program and starts it.
- `cargo xtask dist-capture` no longer fails when an old service still holds the
  build-folder program: it builds to a temporary target name and reports "Built —
  open MoneyKeep and press Update server" (or, when the service still runs from the
  build folder, explains that Update server moves it).
- Command `capture.update_server` (MCP no).

### Service permissions

- On Windows the service keeps the default service security (administrators can
  stop/start it). MoneyKeep never asks the user to type service commands: Start
  server, Update server and Remove use the one-time permission prompt.

### Phone app title bar

- The menu button (☰, drawn as three lines, not a font glyph) is the **first** item
  at the left of the title bar, followed by "MoneyKeep capture"; the menu opens
  below it, left-aligned. Tap target ≥ 44 px.

### Acceptance tests

37. Setup registers the service with the install-folder path (fake service manager);
    a build-folder path in an existing registration is replaced by Update server.
38. Version differs → yellow with the update text and button; Update server stops,
    copies, restarts and verifies (fakes); a failed start restores the old program.
39. Update server refuses a missing server program or one without the embedded app,
    with the build message.
40. The phone app's first title-bar widget is the menu button (accessible name
    "Menu"), left of the title.

### Revision 4a (2026-10-28) — version comparison (owner report)

Owner report: Update server refused "The built server program
(0.0.1+1793216209.59ebe798226f) is not newer than the running one (0.0.1)". Servers
built before revision 4 report no build information. Rules:

- A version **with** build information is newer than the same package version
  **without** it.
- Same package version, both with build information: the later build time is newer;
  equal times with different commits, or any order that can't be decided, count as
  "different" — Update server then asks "The built server can't be compared with the
  running one. Update anyway?" (Update / Cancel) instead of refusing.
- Only an equal version (same build time and commit) or an older one is refused, with
  "The running server is already this build" / "…newer than the built one".

Tests 41: `0.0.1+…` vs `0.0.1` → newer and the update proceeds; equal → refused with
the "already this build" text; undecidable → the confirmation; older → refused.

## Revision 5 (2026-10-28) — tiles on the phone home screen (owner request)

The phone app's home screen shows its three actions as **tiles** in a grid (2 per row
on phones, 3 on wider screens): each tile is a rounded square about 120–160 px, with a
large drawn icon above a short label — **Take photo** (camera), **Choose photos**
(picture/gallery), **Enter expense** (receipt with a pencil) — plus **Outbox**
(tray with an up arrow, with a count badge when items wait) and **Sent** (check mark
in a circle). Icons are painted shapes (no font glyphs). Tiles have the app's accent
colour for the three capture actions and a neutral colour for Outbox and Sent; pressed
state darkens; accessible names are the labels (with the count for Outbox). The Outbox
and Sent lists open from their tiles (instead of being always shown below).

Tests 42–43: the home screen shows five tiles in a grid with the labels and icons;
tapping Outbox shows the outbox list; the Outbox tile's accessible name includes the
count.

## Revision 6 (2026-11-05) — Sign in reaches Cloudflare; the phone gets updates (owner report)

Owner report: on the phone, pairing shows "Sign in to Cloudflare again"; pressing
**Sign in** shows no Cloudflare sign-in, only the Pair screen again. Another browser
tab behaves the same.

Cause: the revision 3 service worker serves the app shell **cache-first**. **Sign in**
reloads the page, the reload is answered from the cache, and the request never reaches
Cloudflare Access, so Access cannot show its sign-in. The same rule means a phone keeps
the cached app until the cache name changes, so updates (including the Slint phone app)
may never reach it.

Sources: W3C Service Workers (`FetchEvent.respondWith`, `Request.mode` "navigate",
opaque-redirect responses to navigations); Fetch Standard (redirect mode "manual",
`Response.type` "opaqueredirect"); Cloudflare Access (unauthenticated requests get a
redirect to the team domain's sign-in, which returns to the original URL).

### Service worker

- **Page loads** (requests with `mode` "navigate" for a shell path) go to the
  **network first**, and the network's answer is used as it is, **including a redirect**
  (an opaque redirect is passed to `respondWith` so the browser follows it to Access's
  sign-in). Only when the network fails (offline, a thrown fetch) is the cached
  `/index.html` (or `/`) used, so the app still opens offline.
- **Other shell files** (the JS glue, the wasm module, start.js, manifest, icons):
  network first; a response that is `ok` (status 200, not opaque) replaces the cached
  copy; a failed fetch, a redirect or any other status falls back to the cached copy
  (and to the network's answer when nothing is cached).
- `/api/` and non-GET requests: unchanged (network, never cached).
- The cache name changes to `moneykeep-capture-v3` so phones holding the old cache drop
  it on activation. A new `sw.js` still calls `skipWaiting()` and `clients.claim()`.

### Sign in button

- **Sign in** navigates to `/` (`location.assign`, replacing the reload), so after
  Access's sign-in the user lands on the app's start page, where the pairing (kept,
  revision 3) is read again. The banner text stays "Sign in to Cloudflare again".
- If the app is running installed (standalone) on iPhone/iPad, the navigation stays in
  the installed app's window (same origin); nothing else changes.

### Acceptance tests (Node test of sw.js with the fake globals, plus the app's unit test)

44. A navigation to `/` while online uses the network's answer; when the network answers
    with an opaque redirect, that redirect is what `respondWith` receives (not the
    cached page).
45. A navigation to `/` while the network fails answers with the cached shell page.
46. A shell file (the wasm module) is fetched from the network and the cache is updated
    with an `ok` answer; when the network fails, or answers with a redirect, the cached
    copy is used.
47. Install uses cache name `moneykeep-capture-v3`; activation deletes
    `moneykeep-capture-v2`.
48. Pressing **Sign in** calls the platform's navigate-to-start (fake platform records
    it) and keeps the token. AT33 stays as it is otherwise.

## Revision 7 (2026-11-05) — phone-friendly expense screen, calendar and amount keypad (owner request)

Owner request: make the phone app more mobile friendly, like a sample page the owner
showed (the page is a layout reference only: it is not committed and nothing is copied
from it). Add a date picker that still defaults to today, and a calculator-like pop-up
of buttons for the amount, with buttons that match the rest of the app.

Everything is drawn by the Slint phone views in the app's own theme (light and dark,
following the system). Nothing is loaded from other hosts, and the CSP is unchanged.

### Whole phone app

- **Top bar** that stays at the top while scrolling. Left to right: a back arrow on
  screens other than Home, the screen title, the status light (connected / waiting /
  offline, as today), and the menu.
- Content is one column, at most 480 px wide and centred, with 16 px side margins.
  Cards have 16 px rounded corners and 16 px padding, and are 12 px apart.
- **Tap targets are at least 48 px tall.** Fields are 52 px tall, and their text is at
  least 16 px so the browser never zooms in. Captions sit above their fields in a
  smaller muted style.
- **Bottom action bar** on the expense form, fixed to the bottom of the screen above
  the system's safe area: **Send** (primary, full width) with **Save for later** (secondary) next to
  it. Cancel becomes the top bar's back arrow, and leaving a form with changes asks
  "Discard this expense?" (Discard / Keep editing).
- Home keeps its revision 5 tiles inside the new frame.

### Expense form (order and look)

1. **Amount card:** a large amount display (32 px, tabular digits, currency symbol
   muted before it) that shows "0.00" in the muted colour until set. The whole card is
   one button, "Amount, <value>"; tapping it opens the **amount keypad**. The amount is
   not a text field on the phone, so the system keyboard never opens for it.
2. **Date:** a field-styled button showing the date in the phone's long form (for
   example "Thu, Nov 5, 2026") with a calendar icon; tapping it opens the
   **calendar**.
3. **Merchant** (text field with the recent-merchant suggestions as chips, as today).
4. **Category** and **Paid with** pickers, then **Purpose** (optional).
5. **Receipt photos:** **Camera** and **Gallery** buttons side by side (same actions as
   Add photo / Choose today) and a row of 64 px thumbnails, each with a remove (×)
   button. Tapping a thumbnail shows it full screen; tap anywhere to close.

### Amount keypad (bottom sheet)

- Slides up from the bottom over a dimmed screen; tapping the dimmed part or Back closes
  it with the value kept as it was when opened.
- Shows the expression and its result at the top (for example "12.50 + 4.25" with
  "= 16.75" below).
- **Quick-add row:** +5, +10, +20, +50 (adds to the current value).
- **Keys** in a 4-column grid, each at least 56 px tall:
  `7 8 9 ÷` / `4 5 6 ×` / `1 2 3 −` / `. 0 ⌫ +`, and a last row **Clear** | **=** |
  **Done**, where Done spans the last two columns.
- Calculator rules: left to right with × and ÷ before + and −; at most 2 decimals typed
  per number; a second "." in one number is ignored; ⌫ removes the last character;
  Clear sets 0; = replaces the expression with its result. **Done** works out the
  expression, rounds to 2 decimals (half away from zero) and puts it in the Amount.
  A result of 0 or less, or a division by zero, shows "Amount must be more than 0"
  in the sheet and does not close it.
- Buttons use the app's own button styles: digits and "." in the normal button style,
  operators in the accent-tinted style, Done in the primary style, Clear in the quiet
  style. Pressed keys darken like other buttons.
- On a computer browser, typing digits, ".", + − * /, Backspace, Enter (= Done) and
  Escape (close) works while the sheet is open.

### Calendar (bottom sheet)

- A new expense's date is **today** (the phone's local date) every time the form opens;
  a saved or sent expense keeps its date.
- The sheet shows the month and year with previous/next month buttons, a weekday row
  (Sunday first), and a grid of days at least 44 × 44 px. Today has a ring, the
  selected day is filled with the accent colour, and days of the next and previous
  months are muted.
- Tapping a day selects it and closes the sheet. A **Today** button jumps back to and
  selects today. Days more than 400 days before or after today (the API's limit) are
  shown disabled.
- Each day has an accessible name such as "Thursday, November 5, 2026", with
  "selected" or "today" added.

### Acceptance tests

49. A new expense form's date is today; tapping Date opens the calendar on today's
    month; tapping a day sets that date and closes; Today selects today; previous and
    next change the month; days beyond 400 days are disabled.
50. Editing a waiting expense keeps its stored date.
51. Tapping the Amount card opens the keypad; "1 2 . 5 0 + 4 . 2 5 Done" sets 16.75;
    "1 0 ÷ 3 Done" sets 3.33; "2 + 3 × 4 =" shows 14; a third decimal digit is
    ignored; ⌫ and Clear work; +20 adds 20.
52. Done with 0, a negative result or a division by zero shows "Amount must be more than
    0" and keeps the sheet open; closing with the dimmed area keeps the old amount.
53. All tap targets on Home, the form, the keypad and the calendar are at least 48 px
    tall (calendar days at least 44 px), checked at 360 px and 430 px wide.
54. The form's bottom bar shows Send and Save for later. Back with changes asks
    "Discard this expense?"; Back with no changes returns Home.
55. Screenshots at 390 × 844, light and dark: Home, the form, the keypad open and the
    calendar open.

## Revision 8 (2026-11-05) — refused Cloudflare tokens are explained; readable pairing fields (owner report)

Owner report: pairing still says "Sign in to Cloudflare again"; **Sign in** reloads
the Pair screen and nothing else, though Cloudflare is signed in in another tab. The
text typed in **Pairing code** and **Name for this phone** cannot be seen until the
field is entered.

Cause (from the owner's `server.log`): after the rename moved the address to
`moneykeepcapture.<domain>`, the phone's requests arrived first **without** a
Cloudflare Access token, then with a token whose **audience** (the Access
application's AUD tag) differs from `access_aud` in `server.toml`. The server answers
both with 401 `not_signed_in`. The phone treats that answer like Access's sign-in
redirect, so it shows **Sign in**; signing in cannot help because Cloudflare already
has a session — the server's settings are what is wrong. The server logs the audience
case only as "access token audience mismatch" and the missing-token case not at all.

Sources: Cloudflare Access application token documentation (the
`Cf-Access-Jwt-Assertion` header; the `aud` claim is the application's AUD tag);
Cloudflare API reference for Access applications (`GET
/accounts/{account}/access/apps/{app}` answers with `domain` and `aud`); WCAG 2.2
success criterion 1.4.3 (contrast at least 4.5:1).

### Server

- A refused token gets a distinct answer: 401 with code `access_refused` when a token
  was sent and failed the check, and 401 `access_missing` when no token was sent and
  the route needs one. `not_signed_in` is no longer sent by the server. Message for
  both: "The capture server did not accept this Cloudflare sign-in. On the computer,
  open phone capture setup and press Check."
- The log says what to fix, never token contents:
  - audience: `access token audience mismatch: expected <configured AUD>, got <AUD(s)
    in the token>` (AUD tags are identifiers, not secrets);
  - missing: `request without a Cloudflare Access token on <path template> — is this
    address behind Access?`;
  - each other `JwtError` as today.
- `GET /api/v1/desktop/status` gains `access_problem`: the latest refusal's log text
  and its time (Unix seconds), or null; it clears after a request passes the Access
  check.

### Phone

- Only a redirect or an HTML page (Access's own sign-in) counts as "sign in again" and
  shows **Sign in**. A 401 with `access_refused` or `access_missing` (or a legacy
  `not_signed_in`) shows the server's message on the Pair screen (and in the outbox
  status) with **no Sign in button**, and keeps any stored pairing.
- The outbox's leftover "Not signed in — reload the page to sign in again" wording is
  replaced by the same message.

### Desktop

- Phone capture setup's status shows `access_problem` when present, in words: "Phone
  requests are being refused by the capture server: <reason>." with a **Check** button.
- **Check** (also run when setup is reopened on a complete setup) reads the Access
  application setup made (`GET …/access/apps/{id}`); when its `aud` differs from the
  recorded one, it records the new value, rewrites `access_aud` in `server.toml` and
  restarts the server (the existing update path); when its `domain` differs from the
  setup's address, it sets the domain to the address (`PUT …/access/apps/{id}`, the
  other fields unchanged). Each change is listed in Details. When the application no
  longer exists, Check says so and offers to create it again (the existing step).

### Pairing fields (all phone text fields)

- Typed text is readable whether or not the field has focus, in light and dark: the
  field fill is its own colour, distinct from the card (light: a slightly darker grey
  than the card; dark: a slightly lighter grey than the card), and the text is the
  theme's main text colour, with contrast at least 4.5:1 against the fill. Placeholder
  text uses the muted colour (at least 4.5:1 too). Focus adds the accent border only;
  it does not change the text colour.

### Tests

56. Server: a token with a different audience gets 401 `access_refused` and the log
    line names both AUDs; no token gets 401 `access_missing` and a log line;
    `desktop/status` reports the latest problem and clears it after a good request.
57. Phone: `access_refused` while pairing shows the message, no **Sign in** button, and
    does not set the sign-in state; a 302 still shows **Sign in** (AT48 unchanged).
58. Desktop: Check with a changed `aud` rewrites `server.toml`, records it and restarts
    the server; a changed domain sends the `PUT`; a missing application is reported.
59. Phone fields, in the real phone window in light and dark: with text in **Pairing
    code** and **Name for this phone** and focus elsewhere, the text colour against the
    field fill measures at least 4.5:1 (read from the drawn colours), and the fill
    differs from the card. Screenshots `phone8-pair-{light,dark}.png` at 390×844 with
    both fields filled and unfocused.

## Revision 9 (2026-11-05) — full-screen app, one Expense screen, bottom tabs (owner request)

Owner report and request, from screenshots on an Android phone (Chrome, installed app):

- The app does not fill the screen: the drawn area stops short of the right and
  bottom edges, so the top bar and tiles look pushed left with an empty strip right
  and below.
- After tapping into the Expense screen a dark line is drawn around the whole app
  (the browser's focus outline on the canvas).
- Merge the home tiles and the Expense screen; put navigation smaller and at the
  bottom. Agreed design below (the owner approved it as described).

Sources: Web App Manifest / `beforeinstallprompt` (WICG Manifest Incubations);
CSS Values and Units Level 4 (dynamic viewport units `dvh`/`dvw`); CSS
Basic UI (`outline`); CSS Environment Variables (`env(safe-area-inset-*)`); HTML
`meta name="viewport"` (`viewport-fit=cover`); `ResizeObserver` and `visualViewport`
(W3C/WICG drafts); Slint 1.18 documentation for the web backend (canvas element and
window size).

### Full-screen drawing area

- The page and the canvas fill the visible viewport exactly, in both directions, at
  load and whenever it changes: rotation, the browser's address bar showing or
  hiding, the installed app's window, and the on-screen keyboard. Use dynamic
  viewport units (with `100%`/`innerHeight` fallback) and resize the Slint window to
  the canvas's CSS size whenever it changes (`ResizeObserver` on the canvas, plus
  `visualViewport` resize). The drawing is sharp at the device pixel ratio.
- No page scrollbars, no rubber-band scroll of the page itself (`overscroll-behavior:
  none`), no margins; the body background matches the app's window background for
  the theme so no other colour shows while resizing.
- The canvas never shows a focus outline or tap highlight (`outline: none`,
  `-webkit-tap-highlight-color: transparent`); keyboard focus is shown by the app's
  own focus rings.
- Safe areas: the top bar's background extends under the status bar inset and the
  tab bar's under the bottom inset; their content stays inside the safe area.
- The content column stays centred (at most 480 px, per revision 7) on wide screens;
  the top bar and tab bar backgrounds span the full width with their content in the
  same centred column.

### One Expense screen with bottom tabs

- The app opens on the **Expense** tab: a new expense form (revision 7 layout) with
  today's date. There is no separate home screen; the revision 5 tiles are removed
  (**Take photo**, **Choose photos** and **Enter expense** are covered by the form's
  Camera and Gallery buttons and the form itself).
- A **tab bar** is fixed at the bottom, 56 px tall plus the safe-area inset, full
  width, top border hairline: three equal tabs **Expense** (receipt-with-pencil
  icon), **Outbox** (tray with up arrow; a count badge when items wait) and **Sent**
  (check in a circle). Each tab: a 24 px painted icon over a 12 px label; the
  selected tab uses the accent colour, the others the muted colour. Tap target is
  the whole tab (at least 48 px tall). Accessible names are the labels, with the
  count for Outbox and "selected" for the current tab.
- On the Expense tab the **Send** (primary, wide) and **Save for later** row sits
  directly above the tab bar, fixed (revision 7's bottom bar, now above the tabs).
  Outbox and Sent show no action row.
- The top bar shows the current tab's title (**Expense**, **Outbox**, **Sent**), the
  status light and the menu. No back arrow on the three tabs. A screen opened from a
  list (an outbox item opened for editing, a sent item's details) shows the back
  arrow and returns to its tab; editing an outbox item uses the same form with its
  own date.
- Switching tabs keeps an unfinished new expense as it is (amount, date, fields,
  photos) for this session; it is not discarded and no discard question is asked.
  The revision 7 "Discard this expense?" question remains only for the back arrow
  from an opened outbox item with unsaved changes. A **Clear** item in the menu (on
  the Expense tab, when the form has anything in it) asks "Clear this expense?"
  (Clear / Keep) and starts a fresh one.
- After **Send** or **Save for later** succeeds, the form resets to a new expense
  with today's date, a short confirmation ("Sent" / "Saved for later") shows for
  about 3 seconds above the action row, and the Outbox badge updates.
- Pairing, the "sign in"/server messages and the iOS install note keep their own
  screens before pairing; the tab bar appears only once paired.


### Install app button appears on the first visit

Owner report: **Install app** did not show until the page was opened a second time.
Cause: `start.js` stores the browser's `beforeinstallprompt` offer in
`window.mcCapture`, but the app only reads it when it draws, and nothing makes it
draw when the offer arrives (on a first visit the browser fires it seconds later,
after the service worker is active).

- When `beforeinstallprompt` or `appinstalled` fires, `start.js` tells the app (a
  callback the app registers, or a `mkcapture-install` event the app listens for) and
  the app redraws, so **Install app** appears or disappears within a second without
  any tap.
- The service worker is registered as soon as `start.js` runs (not after `load`), so
  the browser can make its offer sooner on the first visit.
- `window.mcCapture` is renamed `window.mkCapture` (branding) with no other change.

### Tests

60. The phone window at 390×844 and at 412×915: the top bar spans the full window
    width and the tab bar touches the bottom edge (no gap right or below); resizing
    the window to 844×390 and back relayouts with no gap.
61. The page: `index.html` styles give the canvas no outline and no tap highlight,
    use dynamic viewport height, and the start script resizes the window when the
    canvas size changes (a JS test with Node 22 in `tests-js/` on the resize helper).
62. Opening the paired app shows the Expense tab with today's date; the tab bar has
    Expense (selected), Outbox (with count) and Sent; no tiles exist.
63. Fill amount and merchant, switch to Outbox and back: the values are still there;
    Send resets the form, shows "Sent" and the badge count changes accordingly; Clear
    asks and resets.
64. An outbox item opened from the Outbox tab shows the back arrow; back with changes
    asks to discard; back without changes returns to the Outbox tab.
65. Screenshots at 390×844, light and dark: `phone9-expense`, `phone9-outbox`,
    `phone9-sent`, `phone9-keypad`.

66. Install offer: a JS test (Node 22, `tests-js/`) that a `beforeinstallprompt`
    arriving after start calls the app's registered callback once and stores the
    event; in the phone harness, setting the install state and firing the
    notification shows **Install app** without other input.

Tests 42–43 (tiles) are replaced by 62; AT40 keeps checking the top bar's menu.

## Revision 10 (2026-11-05) — Wi-Fi capture (the normal setup); Cloudflare moves to Advanced (owner decision)

Owner decision, after discussion: phone capture's normal setup works on the home (or
office) Wi-Fi only, with no Cloudflare account. Cloudflare remains as the advanced
setup (it also works on the home Wi-Fi, and away from it). One capture server serves
both; one phone app serves both. The owner confirmed: a one-time certificate install
on each phone is acceptable; the computer has a fixed address; pairing uses the QR
code **and** a Capture PIN; user help is long and thorough.

Sources: W3C Secure Contexts (service workers, installation and persistent storage
need a secure origin); HTML Standard (origin = scheme, host, port; storage is per
origin); RFC 5280 (X.509 certificates; basicConstraints, keyUsage, nameConstraints,
subjectAltName with dNSName and iPAddress); CA/Browser Forum Baseline Requirements
(the 398-day limit applies to publicly trusted certificates, not to roots a user
installs); Android documentation "Install & remove certificates" (Settings > Security
> Encryption & credentials > Install a certificate > CA certificate); Apple Platform
Deployment "Trust manually installed certificate profiles in iOS and iPadOS"
(Settings > General > VPN & Device Management, then Settings > General > About >
Certificate Trust Settings); Apple Configuration Profile Reference
(`com.apple.security.root` payload); Microsoft documentation for Windows Defender
Firewall rules (`New-NetFirewallRule`, profiles Domain/Private/Public) and network
location profiles; RFC 8375 (`home.arpa`); RFC 9106 (Argon2id).

### Why HTTPS on the Wi-Fi, and why a fixed address

- Browsers let a web page install as an app, keep a copy to open offline and keep its
  storage only on a **secure** address (HTTPS). Over plain `http://<ip>` the page
  would load at home but could not be installed and would not open away from the
  Wi-Fi, so expenses could not be entered and kept until home. So the Wi-Fi server
  speaks HTTPS with a certificate from a small certificate authority (CA) that
  MoneyKeep makes on the computer and that each phone trusts once.
- A phone keeps the app, its pairing and its unsent expenses **per address**. A new
  address is a different app to the phone; what was stored under the old one stays
  there. Hence the computer needs a fixed address (a DHCP reservation or a static
  address, optionally with a local name), and setup says so plainly.

### Modes and the one server

- **Wi-Fi** (Settings > Phone capture): the server listens with HTTPS on the
  computer's chosen local address, port **14443** by default (changeable, 1024–65535),
  plus a plain-HTTP helper port, **14080** by default, that serves only the
  certificate-install page and the CA certificate (both public) and redirects every
  other path to the HTTPS address.
- **Cloudflare** (Settings > Advanced > Phone capture over Cloudflare): unchanged from
  revisions 2–8 (loopback listener behind the tunnel and Access).
- Both may be on at once, each with its own listener. The Access check applies only
  to the loopback/Cloudflare listener; on the Wi-Fi listener the device token is the
  check (and the PIN at pairing). The desktop's own connection is unchanged.
- The devices list shows each phone's way in ("Wi-Fi" or "Cloudflare") with its name,
  pairing date and last contact; removing works the same for both.
- A phone uses one way in: the Wi-Fi address and the Cloudflare address are two
  separate apps to the phone. The help says to pick Cloudflare for a phone that must
  send from away from home.

### The certificate authority and server certificate

- Made by setup on the computer: a CA with an ECDSA P-256 key, valid 10 years, named
  "MoneyKeep Capture CA (<computer name>)", with **name constraints** limiting it to
  the chosen address (the IP address and, when given, the local name), so the phone's
  trust cannot be used for any other site. Key usage: certificate signing only.
- The server certificate: P-256, valid 825 days, subjectAltName = the IP address and
  the local name if any; renewed automatically 30 days before it ends (the server
  picks the new one up without dropping paired phones; nothing changes on the phone).
- The CA key is never stored in plain text: it is kept in the operating system's
  credential store (as the inbox key is), with the same fallback the inbox key uses
  (`--key-file`, the existing protected file). The server's certificate key is kept
  like the inbox key. Neither is ever sent to the phone or written to logs.
- Setup shows the CA's **fingerprint** (SHA-256, shown as 8 groups of 4 hex digits)
  so the owner can compare it with the one the phone shows before trusting it (the
  download page is plain HTTP).
- **Change address** (setup, later): makes a new server certificate (and a new CA when
  the constraints must change), with a clear warning first: "Phones paired to the old
  address must send everything in their Outbox first. After the change they must
  install the new certificate and pair again; anything still on them for the old
  address stays there." It lists the phones that have not been in contact for over a
  day.

### Desktop: Wi-Fi setup (a guided dialog, like the Cloudflare one)

1. **Address.** Lists the computer's local IPv4 addresses (with adapter names); the
   owner picks one (or types it) and may add a local name (e.g.
   `moneykeep.home.arpa`). Explains why it must be fixed, with a **How do I make it
   fixed?** link to the help section. Checks the address is on this computer and the
   ports are free.
2. **Capture PIN.** 6–12 digits, entered twice; stored only as an Argon2id hash (as
   book passwords are derived); **Change PIN** later. Not the book password, and the
   dialog says why ("the phone never needs the key to your book").
3. **Firewall.** On Windows, adds an inbound rule for the two ports, **Private and
   Domain profiles only**, named "MoneyKeep capture (Wi-Fi)", through the existing
   elevated service path; warns when the chosen adapter's network is set to
   **Public** and explains how to change it (Settings > Network & internet > the
   network > Private). macOS/Linux: explains the equivalent.
4. **Start.** Installs/updates the server with the Wi-Fi listener and checks it
   answers on the chosen address.
5. **Certificate on the phone.** QR code 1 opens `http://<address>:14080/` on the
   phone: a page with the CA's fingerprint, a **Download certificate** button
   (Android: `.crt`; iPhone/iPad: a `.mobileconfig` profile with the root payload)
   and step-by-step instructions for the phone it detects (both shown, the detected
   one first). The desktop shows the same steps beside the QR code.
6. **Pair.** QR code 2 opens `https://<address>:14443/#pair=<code>` (the existing
   pairing fragment). The app asks for the **Capture PIN** and a name for the phone,
   then pairs and syncs; the dialog shows "Paired: <name>" when done. Then it tells
   the owner to install the app to the home screen (Android: **Install app**;
   iPhone: Share > Add to Home Screen, then pair inside the installed app — the
   iPhone app has its own storage).
   - The code expires after 10 minutes, as today. A wrong PIN counts toward the
     existing failure limit; 5 wrong PINs for one code end that code.

The dialog can be closed and resumed, has **Details** (what was made, ports, rule
name, certificate dates, fingerprint) and **Remove Wi-Fi capture** (stops the
listener, removes the firewall rule and the certificates; paired Wi-Fi phones are
removed too, after a warning).

### Phone app

- Pairing screen: when opened with a pairing code from the Wi-Fi address it asks for
  the **Capture PIN** (digits keypad, masked, a show/hide eye) and **Name for this
  phone**. Wrong PIN: "That PIN is not right. Check it in MoneyKeep on your
  computer." A PIN is also required for Cloudflare pairing when a PIN is set.
- **Sending when back on the Wi-Fi.** Expenses stay in the Outbox while the server
  cannot be reached. The app tries to send when it opens, when the phone reports it is
  online, when the app comes back to the screen, and every 60 seconds while it is
  open. The status light is yellow with "Waiting — not connected to your computer"
  while items wait and the server cannot be reached; the Outbox says "Will send when
  you are on the same Wi-Fi as your computer" for a Wi-Fi phone.
- Web browsers do not let a closed web app send in the background reliably, so the
  help says: open the app once you are home and it sends by itself.

### Help (long and thorough; in `docs/deploy/phone-capture.md` and in the dialog's Help)

The help is written for someone who has never set up a network device. It is
organised as numbered steps with short paragraphs, and covers at least:

1. **What you need**: the computer with MoneyKeep running and on the same Wi-Fi or
   wired network as the phone; a phone with Chrome (Android) or Safari
   (iPhone/iPad); a few minutes; for an iPhone, a passcode on the phone (iOS requires
   it to trust a certificate).
2. **Give the computer a fixed address** — why (a phone stores the app per address),
   and how:
   - **Home router**: find the router's admin page (usually printed on the router),
     find "DHCP reservation", "Address reservation", "Static lease" or "Fixed IP",
     and reserve the computer's current address for its network adapter (MAC
     address). Restart the computer and check the address in MoneyKeep.
   - **OPNsense/pfSense-style firewalls**: Services > DHCPv4 > the LAN interface >
     static mapping; optionally a host override in the DNS resolver for a name such as
     `moneykeep.home.arpa`.
   - **Corporate or office network**: do not change network settings yourself. Ask
     your network or IT team for a **fixed IP address via a DHCP reservation** for
     this computer (give them its MAC address, shown in Details), and ask whether
     phones on the Wi-Fi may reach this computer on the two ports (many office and
     guest Wi-Fi networks block devices from reaching each other — "client
     isolation"). If they cannot allow it, use Cloudflare (Advanced) instead.
   - **Static address set on the computer itself**: only when you know your network's
     free range; a reservation is better.
   - Laptops: the reservation is per adapter; Wi-Fi and Ethernet are different
     adapters with different addresses — pick the one you use.
3. **Run Wi-Fi setup in MoneyKeep** — each dialog step explained, with what to do if
   a check fails.
4. **Install the certificate on the phone** — why it is safe (only valid for this one
   address; compare the fingerprint), then:
   - **Android** (wording varies by maker and version): scan QR code 1, tap
     **Download certificate**; open Settings > Security (or Security & privacy) >
     More security settings > Encryption & credentials > **Install a certificate** >
     **CA certificate** > Install anyway > choose the downloaded file. The phone may
     ask for its screen lock. Check under **Trusted credentials > User** that
     "MoneyKeep Capture CA" is listed.
   - **iPhone/iPad**: scan QR code 1 in the Camera app, open it in **Safari**, tap
     **Download certificate** > Allow; open Settings > **Profile Downloaded** (or
     General > VPN & Device Management) > Install, enter the passcode; then Settings >
     General > About > **Certificate Trust Settings** and turn on full trust for
     "MoneyKeep Capture CA".
   - Compare the fingerprint the phone shows with the one in MoneyKeep.
5. **Pair the phone** — scan QR code 2, enter the PIN and a name, then install the app
   to the home screen; on iPhone pair inside the installed app.
6. **Everyday use** — entering expenses at home or away; when they are sent; the
   status light; MoneyKeep must be running (or the capture service installed) to
   receive.
7. **If the computer's address changes** — send the Outbox first, then Change address,
   reinstall the certificate, pair again; how to remove the old app from the phone.
8. **Removing** — remove the phone in MoneyKeep; remove the app from the phone;
   remove the certificate (Android: Trusted credentials > User > remove; iPhone:
   VPN & Device Management > the profile > Remove).
9. **Troubleshooting** — "This site can't be reached" (same network? computer awake
   and MoneyKeep/the service running? firewall rule present? network profile Public?
   phone VPN, "Private DNS" or ad-blocker apps; guest Wi-Fi isolation; mobile data on
   instead of Wi-Fi); "Your connection is not private" (certificate not installed or
   not trusted, wrong address, phone date wrong); "That PIN is not right"; the app
   will not install; expenses stay in the Outbox; Install app button missing.
10. **Security notes** — what the PIN and the device key protect; that the CA is
    limited to this address; that anyone on the network still needs a code from the
    computer plus the PIN to pair; removing a lost phone.
11. **When to use Cloudflare instead** (Advanced) — sending from away from home,
    networks that block device-to-device traffic.

### Edge cases

- The chosen address disappears (adapter off, different network): the status light
  goes yellow on the desktop with "Wi-Fi capture: this computer no longer has
  <address>"; the server keeps running on the loopback listener for Cloudflare.
- Port in use: setup names the port and suggests the next free one.
- Phone date far wrong: the help covers it; the server's certificate starts one day
  before issue to tolerate small clock differences.
- No PIN set (a Cloudflare setup from before this revision): pairing over Cloudflare
  works without a PIN until one is set; the Wi-Fi setup always sets one.

### Tests

67. CA and server certificate: the CA has basicConstraints CA, keyCertSign only,
    nameConstraints matching the address (IP and name); the server certificate
    verifies against it for the address, is refused for any other name/IP; renewal
    within 30 days of the end makes a new certificate with the same CA.
68. Keys: the CA and server keys are stored through the credential-store path (fake
    in tests) and never appear in config files, logs or API answers.
69. Server: the Wi-Fi listener serves the app and API over TLS and does not ask for
    an Access token; the helper port serves only `/`, `/ca.crt` and
    `/moneykeep-ca.mobileconfig` and redirects everything else to HTTPS; the loopback
    listener still requires Access.
70. Pairing with a PIN: right PIN pairs; wrong PIN refused with `pin_wrong` and counts
    toward the limit; 5 wrong PINs end the code; the PIN is stored only as an
    Argon2id hash.
71. Desktop setup: address list, PIN twice and length rules, firewall rule request
    (Private and Domain only) through the fake host, Public-profile warning, QR 1 and
    QR 2 addresses, resume after closing, Remove Wi-Fi capture removes what was made.
72. Change address: warning text, new certificate with the new address, phones not
    seen for a day listed.
73. Phone: PIN screen (masked, show/hide, digits), wrong-PIN message; outbox retries
    on open, on online, on visibility and every 60 s; the waiting status text for a
    Wi-Fi phone.
74. Settings: Phone capture opens Wi-Fi setup; Cloudflare setup is under Settings >
    Advanced; both can be on; the devices list shows each phone's way in.
75. Help: `docs/deploy/phone-capture.md` has the sections above (a doc test checks
    the headings, including the corporate-network paragraph), and the dialog's Help
    opens it.
76. Screenshots: the desktop Wi-Fi setup at steps 1, 5 and 6; the phone certificate
    page (Android and iPhone wording) and the PIN screen at 390×844, light and dark.

Crates: certificate generation may use a certificate-generation crate under an
already-allowed licence (MIT/Apache-2.0/ISC); TLS uses the `rustls` and `ring`
already in the server. No new licence and no deny exception without asking the owner.

## Revision 11 (2026-11-05) — mileage on the phone and the server (owner request)

Specified in `mileage.md`, sections "Phone (mobile-capture.md revision 11)" and "Capture
server and API": tapping the selected Expense tab toggles the page to Mileage (and
back); trips use the same Outbox and Sent lists; the server accepts, stores and lists
`trip` submissions over both ways in (Wi-Fi and Cloudflare); the lists endpoint gains
vehicles, saved trips and rates. Acceptance tests M10–M14 there.

## Revision 12 (2026-11-06) — Wi-Fi setup: leave the name blank; Wi-Fi first (owner report)

Owner report: pairing with a local name did not work (the phone could not find the
name: a name needs the router or a DNS server to answer for it), while the fixed IP
address did. Most users will not run their own DNS.

- In step 1 of **Set up Wi-Fi capture** and in **Change address…**, the name field is
  labelled **Local name (optional — leave blank)** with the line under it: "Blank is
  best: the IP address is used for the certificate and pairing. Use a name only if
  your router or DNS server answers for it." The field is empty by default (never
  pre-filled from the computer's name).
- When a name is typed, Start checks that it resolves on this computer to the chosen
  address; when it does not, step 1 shows "This name does not lead to <address> on
  this computer. Leave it blank to use the address." and Next stays available only
  after **Use it anyway**.
- Help: the name section starts with "Leave the name blank unless your network has
  its own DNS (an OPNsense or router host override, or your company's DNS). With a
  fixed IP address no name is needed." To drop a name already in use, **Change
  address…** with the name cleared makes a new certificate for the address alone
  (the phones install the new certificate once; their pairing is kept).
- **Settings → Phone capture** lists the Wi-Fi part first (setup, status, phones) and
  the Cloudflare part after it ("Advanced: phone capture over Cloudflare").

Tests:
77. Step 1 shows the new label and line; the field starts empty; with an empty name
    the QR codes and certificate use the address only.
78. A name that does not resolve to the address shows the warning and needs Use it
    anyway.
79. Settings → Phone capture: the Wi-Fi heading comes before the Cloudflare heading.

## Revision 13 (2026-11-06) — switches in the app's colour (owner request)

- Switches on the phone (Round trip, Reimbursable, Enter odometer readings, and any
  other) draw their "on" track in the app's accent colour (the teal of the tabs, the
  calendar's selected day and the primary button), not the default blue. The "off" track
  uses the muted border colour; the knob is white in both. Contrast of the on track
  against the page is at least 3:1, and the state is also clear without colour (knob
  position).
- Test 80: the Mileage form's Round trip switch, on, draws its track in the accent
  colour (read from the drawn pixels); off, in the border colour.

**Correction to revision 12 (help text).** A phone keeps the app, its pairing and its
Outbox per address, so removing the name with **Change address…** makes the phone see a
new app. The help and the Change address dialog agree on this: "Before you change it,
send everything in the phone's Outbox. Afterwards install the new certificate, open the
new address on the phone, install the app again and pair it." The help no longer says the
pairing is kept. The phone at the old address shows nothing new; the old home-screen icon
can be removed.

## Revision 14 (2026-11-08) — the app opens and keeps working while the computer is off (owner report)

Report: with the capture PC turned off, opening the phone app through the Cloudflare
address showed Cloudflare's "Error 1033 — Cloudflare Tunnel error" page instead of the
app.

Cause: the revision 9 service worker uses the network's answer for a page load "as it
is", and only falls back to the cached app when the fetch fails. With the PC off, the
fetch does not fail: Cloudflare answers with its own error page (HTTP status 530 for a
tunnel that is down; 502, 503, 504 and 520–527 in similar cases), and that page is
shown.

Sources: W3C Service Workers; Cloudflare documentation on 5xx/1xxx error pages
(developers.cloudflare.com/support/troubleshooting/http-status-codes/cloudflare-5xx-errors/
and the Cloudflare Tunnel troubleshooting page for error 1033).

### Service worker

- **Page loads:** network first, as before, and a redirect is still passed through (so
  Cloudflare Access's sign-in still works). New: an answer with status **500 or higher**
  is treated like a failed fetch — the cached app page is used. (A 4xx answer is still
  shown as it is.) When nothing is cached, the network's answer is shown.
- **Other shell files:** unchanged (a non-`ok` answer already falls back to the cache).
- Cache name `moneykeep-capture-v4` (activation deletes older caches).

### The app while the computer can't be reached

- API calls that get status 500 or higher, a Cloudflare error page (HTML instead of
  JSON with a 5xx status), or a timeout are "computer not reachable" — the pairing is
  kept, nothing is lost, and expenses and trips stay in the Outbox (revision 9 retry
  rules: on open, on `online`, when the app comes back to the screen, every 60 s).
- Status light yellow with **"Your computer isn't reachable"** and, under it: "It may be
  off, asleep or offline. You can keep adding expenses and trips — they wait in the
  Outbox and send by themselves when it's back." For a Cloudflare phone the Outbox line
  reads "Will send when your computer is on and connected"; for a Wi-Fi phone it stays
  "Will send when you are on the same Wi-Fi as your computer".
- Lists that come from the computer (accounts, categories, tags, vehicles, saved trips)
  are kept from the last successful load (IndexedDB) and used while the computer can't
  be reached, with a small note "Lists from Nov 8, 1:05 PM". A phone that has never
  loaded them shows the forms with free-text category, as on first pairing.
- Help (`docs/deploy/phone-capture.md` and the in-app Help) gains a short section
  "When your computer is off": the app still opens if it has been opened once before;
  new entries wait in the Outbox; open the app again after the computer is back on (the
  browser doesn't send in the background reliably); to have it always reachable, keep
  the computer on and set Windows so it doesn't sleep while plugged in.

### Acceptance tests

80. A navigation to `/` answered with status 530 (Cloudflare error HTML) uses the cached
    shell page; a 503 likewise; a 404 is shown as it is; an opaque redirect is still
    passed through; with no cache, the 530 answer is shown.
81. Install uses `moneykeep-capture-v4`; activation deletes `-v3`.
82. An expense saved while every API call answers 530 stays in the Outbox with the
    yellow "Your computer isn't reachable" status, the pairing is kept, and it is sent
    (once, same `client_id`) when calls succeed again.
83. With the lists cached and the API answering 530, the Expense form offers the cached
    categories and shows the "Lists from …" note.

## Revision 15 (2026-11-08) — no mixed versions; say what the PIN is (owner report)

Report: after installing a new build, the installed phone app showed only a blank screen
after its logo; deleting the site's data in Chrome fixed it. Then the pairing screen asked
for "6 to 12 digits" without saying they are the Capture PIN the owner chose.

Cause (likely): the service worker's cache name did not change between builds
(`moneykeep-capture-v4` since revision 9), so a page could start with the new JS glue and
an old cached wasm module (or the reverse) when one of them was answered by Cloudflare
Access's sign-in redirect and the cached copy was used instead. Glue and module from
different builds cannot start.

### Service worker

- The cache name includes the build: `moneykeep-capture-<capture_version>` (the
  server substitutes it into `sw.js` when serving it, or the build writes it). A new
  build installs a new cache and activation deletes every other `moneykeep-capture-*`
  cache.
- The shell files of one build are served **together**: if any shell file (glue JS or
  wasm module) cannot be fetched from the network with an `ok` answer, all shell files
  come from the cache of one build; the worker never answers one from the network and
  the other from an older cache.
- When the network answers a shell file with a redirect (Access sign-in) and the cached
  build differs from the page's build, the page is reloaded once with navigation, so the
  browser goes through the sign-in (no blank screen).
- `start.js`: if the module fails to start (instantiate error), it shows a short message
  instead of a blank screen: "MoneyKeep couldn't start. Close the app and open it again
  while your computer is on." with a **Reload** button.

### Pairing wording

- The phone's PIN field label: **Capture PIN**, with the help line "The 6 to 12 digits you
  chose when you set up phone capture in MoneyKeep. Forgot it? On the computer: Settings
  → Phone capture → Change PIN…".
- The desktop's pairing window (both Wi-Fi and Cloudflare) adds under the code: "The
  phone will also ask for your Capture PIN."

### Acceptance tests

84. `sw.js` served by the server carries the build's capture version in its cache name;
    activation deletes `moneykeep-capture-v4` and other builds' caches.
85. With the cache holding build A and the network answering the glue with build B but
    the wasm with a redirect, the worker answers both from build A's cache (never a mix);
    the page is told to reload.
86. A failing module start shows the message and Reload (start.js unit test with a fake
    init).
87. The phone pairing screen shows "Capture PIN" and the help line; the desktop pairing
    window shows "The phone will also ask for your Capture PIN."

## Revision 16 (2026-11-09) — setting up again after removing a tunnel (owner report)

Report: the owner deleted the old tunnel in Cloudflare and uninstalled the apps to test
from a clean start. Cloudflare setup then stopped at Install with "A cloudflared service
for another tunnel already runs on this computer; it was not replaced. Choose 'Add to my
existing tunnel'", Install greyed out, although that tunnel no longer exists. The undo
after the failed attempt also failed: `PUT …/cfd_tunnel/{tunnel}/configurations`
answered "HTTP 400: Bad Configuration: … doesn't contain any ingress rules" (twice).

Sources: Cloudflare Tunnel documentation — remotely-managed tunnel configuration
(`config.ingress` must end with a catch-all rule such as `{"service": "http_status:404"}`);
`cloudflared service uninstall` removes the Windows service; a tunnel's token identifies
its tunnel id.

### A leftover cloudflared service

- When a cloudflared Windows service exists, setup reads which tunnel it runs (the
  tunnel id in its token / command line) and looks for that tunnel in the account.
  - **The tunnel still exists:** as today ("Add to my existing tunnel" is offered and
    preselected; the message explains why).
  - **The tunnel no longer exists** (deleted in Cloudflare) or belongs to another account:
    the message reads "A cloudflared service on this computer runs a tunnel that no
    longer exists in your Cloudflare account. MoneyKeep can replace it with the new
    tunnel." with **Replace it** (default) and **Leave it** (Install stays unavailable
    with the reason). Replace stops and removes that service (same elevated step as the
    install, one Windows permission prompt) and installs cloudflared for the new tunnel.
- The plan and the Details list show "Replace the cloudflared service of a deleted
  tunnel" as a step.

### Undo must leave valid settings

- Removing MoneyKeep's ingress rule from a tunnel's configuration always writes a
  configuration that ends with the catch-all `{"service": "http_status:404"}` (keeping
  the user's other rules in order). If the tunnel was created by this setup, the undo
  deletes the tunnel instead of editing it.
- An undo step that fails is listed with its reason and **Retry undo**; the setup is not
  shown as cleanly undone while anything is left (the list says what is left and where to
  remove it by hand in the Cloudflare dashboard).

### Clean start help

- Settings → Phone capture gains **Remove phone capture** guidance in the help
  (`docs/deploy/phone-capture.md`): what Remove takes away (Access app, policies, service
  token, DNS record, MoneyKeep's ingress rule or its tunnel, the capture and cloudflared
  services it installed), and how to clean up by hand after deleting things in the
  dashboard first: `cloudflared service uninstall` in an administrator terminal, or
  `sc.exe stop cloudflared` and `sc.exe delete cloudflared`.

### Tests

88. A cloudflared service whose tunnel id is not in the account → the replace message,
    Replace preselected, Install available; Leave keeps Install unavailable with the reason.
89. A cloudflared service whose tunnel exists → today's "Add to my existing tunnel".
90. Undo of an added ingress rule on a tunnel with no other rules writes
    `ingress: [{"service":"http_status:404"}]`; with other rules they are kept in order
    before the catch-all; a tunnel created by setup is deleted.
91. A failing undo step is listed with its reason and Retry undo.

## Revision 17 (2026-11-09) — Start over (owner request)

Purpose: one place to remove everything phone capture set up — in Cloudflare, on this
computer and in MoneyKeep — so someone stuck halfway (or who deleted pieces by hand) can
begin again from nothing. Books and backups are never touched.

### Where

- **Settings → Phone capture → Start over…**, always available (also while setup is
  incomplete, broken, or was never finished), and offered on the setup wizard's failure
  screens ("Stuck? Start over…").

### The preview (nothing changes until confirmed)

A dialog lists, in three groups, what will be removed and where, each with what it is:
- **In Cloudflare** (only things MoneyKeep recorded as created by its setup, by id): the
  Access application and its policies, the service token, the DNS record, and the tunnel
  if setup created it — otherwise only MoneyKeep's ingress rule in that tunnel (the
  remaining rules kept, ending with the catch-all, revision 16).
- **On this computer:** the capture server service; the cloudflared service if
  MoneyKeep installed it, or if the tunnel it runs no longer exists in the account
  (revision 16) — a cloudflared service for a tunnel that still exists and that MoneyKeep
  did not install is listed under **Kept**; the Wi-Fi firewall rule; the capture server's
  data folder (certificates, paired phones, the inbox).
- **In MoneyKeep:** the Capture PIN, the saved Cloudflare API token, the setup state,
  phone items not yet collected (counted: "3 expenses from phones not yet collected will
  be lost — collect them first?" with **Collect now**), the pairing list.
- **Kept:** books, backups, Cloudflare items MoneyKeep didn't create (other tunnels,
  records, apps — named when MoneyKeep can see them), the MoneyKeep program.
When Cloudflare can't be reached or the saved token no longer works, the Cloudflare group
says so and lists what to delete by hand (with the dashboard place for each:
Zero Trust → Access → Applications; → Service Auth; DNS → Records; Zero Trust →
Networks → Tunnels), and the rest still proceeds.

### Confirmation and permission

- The **Start over** button is enabled after typing `START OVER` (case-insensitive).
- One Windows permission prompt covers the computer part (services, firewall, the
  capture data folder under the service's account); the dialog says so before it
  appears. Declining the prompt does the Cloudflare and MoneyKeep parts and lists the
  computer part as left, with **Try again**.

### Result

- A list of every step with ok / removed / left (with the reason and where to finish by
  hand). "Phone capture is cleared. Set up phone capture to begin again." with **Set up
  phone capture**. An audit entry records that Start over ran (not the token or PIN).
- Phones still holding the old pairing show "This phone is no longer paired" next time
  and keep their Outbox; pairing again sends it.

### Tests

92. Preview lists the three groups and Kept; nothing changes until confirmed; the
    button needs `START OVER`.
93. With a setup-created tunnel: the tunnel, Access app, policies, token and DNS record
    are deleted (fake Cloudflare); with an existing tunnel only MoneyKeep's rule is
    removed and the catch-all kept.
94. A cloudflared service for an existing tunnel not installed by MoneyKeep is kept; for
    a deleted tunnel it is removed.
95. Uncollected items are counted and Collect now collects them before clearing.
96. Cloudflare unreachable (fake server down): the computer and MoneyKeep parts run; the
    Cloudflare items are listed as left with their dashboard places.
97. Permission declined: Cloudflare and MoneyKeep parts done; computer part left with
    Try again.
98. Books and backups are byte-for-byte unchanged after Start over.

### Revision 16a (2026-11-09) — a tunnel in another account is not "deleted"

A cloudflared service whose tunnel belongs to **another Cloudflare account** (the
service's account id differs from the token's) may be a live tunnel the owner uses for
something else. It is not treated as deleted: the message reads "A cloudflared service on
this computer runs a tunnel in another Cloudflare account. MoneyKeep leaves it alone
unless you choose to replace it." with **Leave it** preselected and **Replace it**
available (with the warning "Anything that tunnel serves will stop working."). Only a
404 / `deleted_at` in the same account preselects Replace it. Test 89a.

### Revision 16b (2026-11-09) — leftover cloudflared event-log entry (owner report)

Report: after the old service was deleted with `sc.exe delete`, installing failed:
"cloudflared agent service is installed … Cannot install event logger:
SYSTEM\CurrentControlSet\Services\EventLog\Application\Cloudflared registry key already
exists". `sc.exe delete` removes the service but not its Windows event-log source.

- Before `cloudflared service install`, the elevated step removes a stale event-log
  source key `HKLM\SYSTEM\CurrentControlSet\Services\EventLog\Application\Cloudflared`
  when no cloudflared service exists.
- If the install still reports only the event-logger error and the service now exists
  (`sc query` / service manager), the install counts as successful (logged as a note).
- Replace it (revision 16), Start over (revision 17) and Remove phone capture remove that
  key together with the service.
- Help: the manual clean-up adds
  `reg delete "HKLM\SYSTEM\CurrentControlSet\Services\EventLog\Application\Cloudflared" /f`.
- Test 89b: with the key present and no service, the step deletes it before installing;
  an event-logger-only error with the service present counts as installed.

### Revision 16c (2026-11-09) — collecting after a Cloudflare setup (owner report)

Report: after setting up Cloudflare capture again, phones paired, but **Check now** /
collecting failed with "The capture server refused the desktop token (or asks for a
Cloudflare Access sign-in)" in the same book.

Cause: the desktop's collection requests (`/api/v1/desktop/*`, `capture.rs`
`Endpoint::req`) send only the desktop token. When the saved server address is the public
Cloudflare address, Cloudflare Access stops them before they reach the server, because
the Access service-token headers (`CF-Access-Client-Id`, `CF-Access-Client-Secret`) that
the setup's own status check sends (revision 5) are not added to collection.

- **Same computer:** when the capture server runs on this computer (setup's "This
  computer" choice, or the service is found locally), MoneyKeep collects through
  `http://127.0.0.1:8787` (loopback; no Access in the way), whatever address is saved for
  display.
- **Through Cloudflare** (the server runs on another computer, or loopback can't be
  reached): every desktop request carries the stored Access service-token headers in
  addition to the desktop token.
- The status line says which way it collected ("Collected through this computer" /
  "Collected through Cloudflare").
- An Access refusal (a redirect to `*.cloudflareaccess.com` or a 403 with Access's page)
  is reported as "Cloudflare Access refused MoneyKeep's service token — set up phone
  capture again or check the service token in Cloudflare", separately from a refused
  desktop token (a JSON 401 from the server).
- Test 89c: with a public address saved and the server local, collection uses loopback;
  with a remote server it sends both Access headers; an Access redirect yields the Access
  message, a server 401 the token message.

## Revision 18 (2026-11-09) — a loading screen, and a faster start (owner request)

Report: after the phone's launch logo, about 6 seconds of blank screen, then "Loading…"
at the top, until the app appears. Owner decisions: a centered loading screen; serve the
program compressed; shrink it at build time with `wasm-opt` (Binaryen, Apache-2.0 —
owner approved adding it as a build tool; nothing of it ships in MoneyKeep).

Sources: WHATWG Fetch (`Response.body` streams, `Content-Length`), WebAssembly JS API
(`instantiateStreaming`), RFC 9110 (content negotiation, `Accept-Encoding`,
`Content-Encoding`, `Vary`), RFC 7932 (Brotli) / RFC 1952 (gzip), Binaryen `wasm-opt`
documentation.

### Loading screen (in `index.html`, no script needed to show it)

- Shown the instant the page opens, centered both ways in the visible area (safe-area
  aware): the app icon (96 px), "MoneyKeep Capture" (title), a status line and a thin
  progress bar (240 px wide, rounded).
  - While the program downloads: "Downloading… 45 %" with the bar filled to match (from
    the streamed byte count against the uncompressed size the server sends in a header,
    `X-MK-Program-Size`, or the compressed `Content-Length` when that is all there is);
    an indeterminate bar when no size is known.
  - While it is being prepared: "Getting ready…" (indeterminate bar).
  - From the service worker's cache: straight to "Getting ready…".
- Colours follow the phone's light/dark setting with the app's own page colours (no
  white flash); text meets 4.5:1.
- It fades out (150 ms) when the app has drawn its first frame (the app tells the page),
  and is removed from the page. The old "Loading…" text at the top is removed.
- A start failure (revision 15) shows its message and **Reload** inside the same centered
  card.
- Accessible: `role="status"` with the status text, `aria-busy` until removed.

### Smaller and compressed program

- **Build:** `cargo xtask release` / `installer` (and `dist-capture`) run `wasm-opt -Oz`
  (keeping the features the build uses) on the phone app's wasm after Trunk. When
  `wasm-opt` is missing, the build stops with how to get it (`winget install
  Binaryen` is not available everywhere, so: "Download Binaryen from its GitHub releases
  and put its bin folder on PATH", with the release page's address), unless
  `--no-wasm-opt` is given (developers; prints that the program is larger).
- The release folder records the program's size before and after in `version.json`
  (`phone_program_bytes`), and the build prints both.
- **Server:** the embedded phone files are stored with precompressed Brotli and gzip
  copies made at build time (the program, the JS glue, `index.html`). The server answers
  with the best encoding the phone accepts (`Accept-Encoding`: `br`, then `gzip`, else
  identity), sets `Content-Encoding` and `Vary: Accept-Encoding`, and sends
  `X-MK-Program-Size` (uncompressed size) for the program. Through Cloudflare the
  precompressed copy is passed on as is.
- The service worker stores what it fetched; caching and the per-build rules
  (revision 15) are unchanged.

### Tests

99. `index.html` contains the centered loading card with `role="status"` and no script
    is needed to show it (static check); the old top "Loading…" is gone.
100. Progress: a fake fetch streaming a program of known size updates the status to
     "Downloading… 50 %" halfway, then "Getting ready…"; with no size, the bar is
     indeterminate (shell.js test).
101. The card is removed after the app's first-frame signal; a start failure shows the
     message and Reload in the card.
102. Server: `Accept-Encoding: br` gets the Brotli copy with `Content-Encoding: br` and
     `Vary`; `gzip` only gets gzip; none gets identity; bytes decompress to the original;
     `X-MK-Program-Size` equals the original size.
103. The build runs `wasm-opt` and records both sizes; with `wasm-opt` missing it stops
     with the instructions; `--no-wasm-opt` builds with a note.
104. Screenshots 390×844 light and dark: `phone18-loading-download`, `phone18-loading-ready`.

### Can't reach the computer on Wi-Fi (owner report, 2026-11-09)

Report: after reinstalling, phones showed a blank screen on Wi-Fi while the computer
itself could open `https://<address>:14443`. MoneyKeep's firewall rule was gone
(uninstall removes it; reinstall doesn't add it back), and Windows had made its own rule,
"moneykeep-server", when the server first listened. Running **Change address…** again
fixed it.

- **Loading card:** when the Wi-Fi address doesn't answer within 8 seconds (no response
  at all, as opposed to an error response), the card shows "Can't reach your computer on
  this Wi-Fi" and the line "Make sure this phone is on the same Wi-Fi as your computer,
  and that MoneyKeep is running there." It also shows **Try again**, plus **Use Cloudflare**
  when a Cloudflare address is paired. The app never stays blank. If the app is in the
  service worker's cache, it opens offline as in revision 14, and this message shows as
  the offline banner.
- **Firewall rule put back:** whenever Wi-Fi capture is turned on and the rule "MoneyKeep
  capture (Wi-Fi)" is missing or disabled, MoneyKeep puts it back:
  - **when the installer runs:** silently, as administrator;
  - **when the app starts:** it asks once with the administrator prompt ("Phones can't
    reach this computer: the firewall rule is missing. Fix").
  - The rule is the same one Wi-Fi setup makes: inbound TCP 14443 and 14080, Private and
    Domain profiles.
- **Windows' own Block rules:** the Status card checks for inbound rules that name
  `moneykeep-server.exe` with Action Block (Windows makes these when its "allow access"
  pop-up is dismissed). It reports "Windows is blocking the capture server" with **Fix**,
  which removes those Block rules with the administrator prompt. Allow rules made by
  Windows are left alone.
- **Network is Public:** when Windows calls the current network Public, the Status card
  says "Windows treats this network as public, so phones can't connect over Wi-Fi". It
  links to Windows' network settings (`ms-settings:network-status`). MoneyKeep doesn't
  change the network type itself.
- **Start over (revision 17)** removes the rule along with the rest, as now. Wi-Fi setup
  afterwards makes it again.

### PIN field visible (owner report, 2026-11-09)

- The phone's pairing-code field (and every text field on the phone) has a visible
  outline. It is 1.5 px in the theme's border colour, at least 3:1 against the
  background (WCAG 1.4.11). When the field has focus, the outline turns to the accent
  colour and becomes 2 px. The field is at least 44 px tall and has a visible label
  above it. Its placeholder reads "6–12 digits".

### Tests (continued)

105. A Wi-Fi address that never answers shows "Can't reach your computer on this Wi-Fi"
     within 8 seconds, with Try again; with a Cloudflare address paired, Use Cloudflare
     is offered too (shell.js test with a fake fetch that never resolves).
106. Firewall check (Windows, using a fake for the firewall calls): a missing rule
     gives the Fix action and recreates the rule with the right ports and profiles. A
     Block rule for `moneykeep-server.exe` is reported and removed by Fix, and an Allow
     rule for it is left alone. A Public network gives the Public message.
107. The installer script recreates the rule when Wi-Fi capture is set (static check of
     the script).
108. Screenshots 390×844 light and dark: `phone18-wifi-unreachable`,
     `phone18-pair-field` (empty and focused).
