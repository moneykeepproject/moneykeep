# Spec: Upgrade resilience for phone capture (review of 2026-11-10)

- **Area / module:** capture server and its administrator helper, installer, phone app
  (service worker, outbox), CI; GroupKeep split calculation
- **Spec writer:** spec-writer session (Claude), with the owner
- **Date:** 2026-11-10
- **Status:** reviewed — owner approved 2026-11-10 (all findings; decisions D1–D3 below)

## Goal

Any newer MoneyKeep installs over any older one, or over data kept by an uninstall,
without losing pairings, the Wi-Fi certificate or uncollected expenses, and without
re-pairing phones. A failure part-way always leaves a working server (the old one) and a
message saying what happened. An older MoneyKeep meeting newer data refuses with a clear
message instead of silently making new keys.

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Review | Project doc `claude/upgrade-resilience-review-2026-11-10.md` (H1–H2, M1–M10, L1–L11, test gaps) | Each finding has its file, line, scenario and suggested fix |
| 2 | Vendor doc | Microsoft: `ChangeServiceConfig2` (`SERVICE_CONFIG_FAILURE_ACTIONS`, `SERVICE_CONFIG_FAILURE_ACTIONS_FLAG`), virtual accounts, `CryptProtectData` / `CryptUnprotectData` (`CRYPTPROTECT_LOCAL_MACHINE`), `MoveFileEx`, `FlushFileBuffers`, PowerShell Constrained Language Mode | M5, M6, H2, L10 |
| 3 | Standard | RFC 5280 §4.2.1.10 name constraints | M7 |
| 4 | Standard | W3C Service Workers (install, activate, Cache Storage), IndexedDB | M8–M10, L9 |
| 5 | Vendor doc | GitHub Actions: `windows-latest` runners, artifacts | Batch U3 |
| 6 | Spec | `security-review-1.md` (A1–A6), `mobile-capture.md`, `release-packaging.md` | Behaviour being corrected |
| 7 | Owner decisions | Conversation 2026-11-10 | D1–D3 |

## Owner decisions

- **D1 (M7):** the phone certificate authority stays locked to the one address it was
  made for. A changed address still needs the certificate installed again on each phone;
  the app avoids it where it can and explains it where it can't (see U1.7).
- **D2 (M4):** no rollback. From this version on, the data folder carries a format
  number; an older server meeting a newer one refuses to start with a clear message. The
  help page says downgrades aren't supported.
- **D3 (test gap):** a GitHub Actions Windows job is added (Batch U3). It runs only when
  the repository is pushed.

## Batch U1 — capture server, helper and installer

### U1.1 A safe data move (H1, M1, M3)

- `move_data` first makes sure the service runs **this build's** program: it refreshes
  the service's copy in its install folder (as `update` does) and re-registers the
  service on it, before any data is copied. If the program can't be refreshed, nothing
  is moved and the old server is started again.
- The move health-checks the server on the new folder (as `update` and `reinstall` do).
  On failure the service is pointed back at the old folder with the old arguments, the
  copies are removed, and the old server is started; `moved.json` is written only after
  a passing health check.
- When the source folder has a plain `inbox.key` and no sealed one, any
  `inbox.key.sealed` already in the destination is deleted before sealing (M3). The
  same rule applies to every sealed file with a plain source.
- **Setup:** the service's copy (`CAPTURE_COPY_DIR`) is stopped and refreshed whatever
  `$INSTDIR` is (H1). Setup reads the helper's JSON report from `move-data` and
  `repair-firewall`; on an error it shows the message in a message box (silent installs:
  in the log) and says phone capture needs attention in MoneyKeep (L3).

### U1.2 Writes that survive a full disk or power loss (H2)

- Every key, certificate, `wifi.json`, `server.toml`, plan, report and marker file is
  written to a temporary name in the same folder, flushed, then renamed over the target.
  A failed write leaves the old file as it was.
- **Key opening:** if `inbox.key.sealed` can't be opened and a plain `inbox.key` is
  present, the plain key is used and sealed again (atomically). If neither opens, the
  server refuses to start with "The inbox key can't be opened." It never makes a new key
  while an inbox database with rows exists.
- **The certificate authority is never replaced at start.** If `wifi.json`, the CA
  certificate or the CA key can't be read, the server starts without the Wi-Fi listener
  and reports `wifi_error` in health. The desktop's Status card shows "Wi-Fi capture
  can't start: its certificate files are damaged" with **Make a new certificate**, which
  explains that every phone will need the certificate installed again. Making a new
  authority happens only through that button or Wi-Fi setup.

### U1.3 Stale old copies are never brought back (M2)

- A folder holding `moved.json` is never used as a source by Install the server again or
  by a move. When a move succeeds, the old folder's `server.toml` is also renamed to
  `server.toml.moved`.
- Remove the old copy stays offered while the old folder exists (not only 30 days); after
  30 days it moves from the Status card to the help menu's "Clean up".

### U1.4 Format number and tolerant readers (M4, D2)

- The data folder gets `format.json` (`{"format": 1}`), written atomically by setup,
  move and reinstall. A server or helper whose own format is lower refuses to start or
  to touch the folder: "This phone capture data was made by a newer MoneyKeep. Install
  that version or a newer one." The health endpoint isn't available then, and the data
  folder can't be read by the user, so the server stops with a service-specific exit
  code for each refusal (newer format, newer database, unreadable `server.toml`, key
  can't be opened). `services status` reads it with no prompt and the desktop shows the
  matching message.
- `inbox.sqlite`: `meta.schema` is read at open; a higher number is refused the same way.
  Migrations stay in code, each one numbered.
- **Readers that tolerate new fields:** `Lists`, `wifi.json`, `pair-info` and every API
  body ignore unknown fields; every new field has a default. `server.toml` keeps refusing
  unknown keys, but the refusal is reported through `services status`, not only the log.
  The helper's plan stays strict (the helper ships beside the desktop app).
- The desktop's helper is found only beside the desktop program (drop the PATH fall-back
  in `host.rs`).
- `/api/v1/health` reports `api_level`. The desktop shows "Update the capture server" when
  the server's level is lower than it needs; the phone shows "Your computer's MoneyKeep
  needs updating" when the server's level is lower than the phone app needs.
- **Help page:** downgrades aren't supported; going back to an earlier version means
  uninstalling (removing the data) and pairing phones again.

### U1.5 The service looks after itself (M5, L1, L2)

- The loopback listener retries its bind for up to 60 seconds, as the Wi-Fi listener does.
- The service is registered with failure actions: restart after 10 s, 30 s, then 60 s,
  reset after a day, with the flag that also applies them to a non-zero exit.
- `--allow-lan` is taken from the service's existing command line by update and install,
  unless the plan sets it explicitly (L1).
- Installing or updating the server program keeps the previous program as
  `moneykeep-server.exe.prev` until the new one passes its health check, and puts it back
  on failure (L2).

### U1.6 Opening the inbox key without PowerShell (M6)

- DPAPI sealing and opening use the Windows API through a crate already in the tree
  (`windows-sys` or `windows`, MIT/Apache-2.0); the workspace's ban on `unsafe` stays for
  our own code. PowerShell remains only in setup steps (folder permissions, firewall),
  which run under the helper.
- If those setup steps fail because PowerShell is restricted, the error says so: "Windows
  blocked a PowerShell step (it may be restricted by your organisation)."

### U1.7 Address changes (M7, D1)

- Wi-Fi setup suggests reserving the computer's address in the router, with a link to
  the help page, whenever an address is chosen.
- When the computer's address changes, Change address… says before saving that phones
  will need the certificate installed again, and the Status card afterwards lists the
  phones that haven't connected since, with Certificate QR code….

### U1.8 Smaller items (L4–L8, L10)

- **L4:** uninstall with "remove data" also removes MoneyKeep's tunnel token file and, if
  MoneyKeep installed cloudflared's service, stops and removes that service.
- **L5:** the uninstaller is run through a small stub that passes the original user's
  `%LOCALAPPDATA%` (captured unelevated before UAC), so the right old folder is checked.
- **L6:** when `%ProgramData%\MoneyKeep` exists with the wrong owner, the refusal adds
  "Remove the folder C:\ProgramData\MoneyKeep (it was not made by MoneyKeep's setup) and
  try again", and Setup shows it.
- **L7:** the help page says the sealed key belongs to this Windows installation, so a
  data folder copied to another PC means pairing phones again.
- **L8:** the Wi-Fi listener re-binds when the address disappears and comes back (sleep,
  Wi-Fi drop); health reports `wifi_listening`.
- **L10:** changing an existing LocalSystem service to `NT SERVICE\MoneyKeepCapture` is
  covered by the Windows CI job (U3); if `ChangeServiceConfig` refuses, the helper
  deletes and recreates the service with the same settings.

### U1.9 A stale collection error doesn't outlive the problem (owner testing, 2026-11-10)

After Install the server again succeeded, the Status card kept showing "Collecting from
the server failed: … actively refused it (os error 10061)" until MoneyKeep was
restarted: the stored `last_collect_error` is cleared only by a successful collection,
and Check now doesn't collect.

- **Check now** runs a collection (when collecting is on for this book) before it
  evaluates the status, so a fixed problem clears at once.
- A successful Install the server again, Start the server, Update server or move
  starts a collection right away.
- The collection error line shows when it happened ("Collecting from the server failed
  at 18:12: …"), and an error older than the latest passing health check is shown as a
  grey note under the status, not as the status itself.
- Test UR-11: with a stored collection error and a healthy server, Check now collects
  and the card turns green; after a fake successful reinstall a collection is started.

## Batch U2 — phone app

- **M8, one build at a time:** each build's cache is filled at install and never written
  to afterwards. The page, `shell.js`, `start.js` and the program are always served from
  the same source: when the page's build (in its URL or a header) differs from the
  worker's, everything comes from the network; if the network fails, everything comes
  from the cache of one complete build. The 6-second fallback applies to the whole set.
- **L9:** first install doesn't download the program twice: the worker caches the
  program the page already fetched, or the page waits for the worker's copy.
- **M9:** on "A photo is not available on the server any more", the phone clears that
  item's `receipt_id`s and queues it again so the photos are uploaded afresh. Only if the
  phone no longer has the photo does the item stay parked with "Add the photo again".
- **M10:** each stored outbox item carries a `v` number. Items that can't be read are
  kept, never deleted, and the outbox shows "n saved items couldn't be read by this
  version"; a later version that understands them sends them.

## Batch U3 — Windows CI job

- `.github/workflows/windows.yml`, on push and pull request, `windows-latest`:
  1. `cargo build --workspace` and `cargo test --workspace` excluding the long `ui` binary
     (it runs on Linux), with the `#[cfg(windows)]` code compiled.
  2. `cargo xtask installer --no-wasm-opt`.
  3. **Pre-A1 upgrade:** register a stand-in old service (LocalSystem, data in the
     runner user's `%LOCALAPPDATA%`, plain `inbox.key`, a fixture inbox item and Wi-Fi
     certificate), install, then check: the service runs as `NT SERVICE\MoneyKeepCapture`
     from ProgramData, health passes, the inbox item opens, the CA fingerprint is
     unchanged, the old folder holds `moved.json`.
  4. **Non-default folder (H1):** the same with `/D=C:\MK`.
  5. **Uninstall keeping data, then reinstall:** the helper's Install the server again
     plan recovers the server with the same fingerprint and inbox.
  6. Upgrade from the previous release's installer (from its GitHub release asset) once a
     release exists; skipped with a note before that.
- The job uploads `server.log`, the helper reports and the setup log as artifacts.

## Other repositories

- **GroupKeep (L11):** an override larger than the expense's total is refused when it's
  entered ("This share is more than the whole expense."). If fixed shares together
  exceed the total, the others' shares are 0, never negative, and the expense shows
  "Shares add up to more than the total" until fixed. Delivered with GroupKeep's
  security-review-1 work.

## Tests

- **UR-1 (H1, M1, M3):** move with the service's program outside `$INSTDIR`: the program
  is refreshed first. A server that fails its health check after the move is pointed
  back, the copies removed, no `moved.json`. A destination with an old sealed key and a
  source with a plain key ends with the source's key.
- **UR-2 (H2):** a write cut short for each key and certificate file leaves the old file.
  An empty `inbox.key.sealed` with a plain key present starts and re-seals. A damaged
  `wifi.json` starts without Wi-Fi, keeps the CA files, and health reports `wifi_error`.
- **UR-3 (M2):** a folder with `moved.json` is never offered or accepted as a source.
- **UR-4 (M4):** a newer `format.json`, a newer `meta.schema` and an unknown `server.toml`
  key each refuse with their message, visible through `services status`. `Lists` with an
  extra field is accepted. `api_level` lower than needed shows the update message.
- **UR-5 (M5, L1, L2):** port 8787 held for 5 seconds then freed: the server starts. The
  service config carries the failure actions. Update keeps `--allow-lan`. A program that
  fails its health check is replaced by `.prev`.
- **UR-6 (M6):** key sealing and opening make no PowerShell call (fake process runner
  records none).
- **UR-7 (M8, L9, M9, M10):** Node tests: old worker plus new server then offline serves
  one complete old build; new page with a slow program uses all-network or all-cache;
  first install fetches the program once. A "photo not available" reply re-queues with
  photos re-uploaded. An unreadable item is kept and counted.
- **UR-8 (U3):** the workflow file is valid and its steps run the commands above (static
  check in xtask); the job itself runs on GitHub.
- **UR-9 (L4–L8):** static installer checks for the token and cloudflared removal and
  the stub; the L6 message; the L8 re-bind with a fake address source.
- **UR-10 (GroupKeep L11):** a 30.00 expense, three people, one fixed at 40.00 is
  refused; two fixed shares of 20.00 give the third 0.00 and the notice.

## Ledger effect

None. Expense amounts and imports are unchanged; collection still de-duplicates by
submission id.

## Clean-room check

Written from the review's findings, Microsoft's, W3C, IETF and GitHub documentation,
and the owner's decisions.
