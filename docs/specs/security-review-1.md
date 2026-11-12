# Spec: Fixes from the first code review (before publishing)

- **Area / module:** capture server and its administrator helper, backups and book
  encryption, SnapTrade, release packaging, repository housekeeping (all five repos)
- **Spec writer:** spec-writer session (Claude), with the owner
- **Date:** 2026-11-09
- **Status:** reviewed — owner approved 2026-11-09 (all findings; decisions D1–D4 below)

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Review | Project doc `claude/code-review-2026-11-09.md` (findings H1–H2, M1–M9, L1–L13) | Each finding has its file, line and trigger |
| 2 | Vendor doc | Microsoft: service accounts (LocalService, virtual accounts `NT SERVICE\<name>`), `ChangeServiceConfig`, service SIDs (`SERVICE_SID_TYPE_UNRESTRICTED`), `icacls` / security descriptors, `CreateFile` share modes, `MoveFileEx` (no `MOVEFILE_REPLACE_EXISTING`), `WinVerifyTrust` / Authenticode | Windows details |
| 3 | Vendor doc | Cloudflare: cloudflared Windows install (MSI, install folder), signer of its releases | H2, M1 |
| 4 | Vendor doc | SnapTrade API: request signing (`timestamp` query parameter, signature) | M7 |
| 5 | Spec | `mobile-capture.md`, `backups.md`, `book-encryption.md`, `connectivity-brokerage.md`, `release-packaging.md`, `release-prep.md` | Behaviour being corrected |
| 6 | Owner decisions | Conversation 2026-11-09 | D1–D4 |

## Owner decisions

- **D1 (H1):** the capture service runs under its own virtual service account, with its
  data under `C:\ProgramData\MoneyKeep\capture-server`. Existing data is moved once, so
  phones stay paired.
- **D2 (M9):** keep-launcher gets its own release workflow in its repository and stops
  calling the storytold organisation's shared workflows.
- **D3 (L11):** Form1099Keep's uncompiled prototype files are deleted.
- **D4 (L13):** the target products' names in the repositories' agent and clean-room
  documents are replaced with "commercial finance products" (wording adjusted to fit
  the sentence), and their file formats with "their proprietary file formats".

## Batch A — administrator and capture server

### A1. The service runs with low rights (H1, D1)

- The service `MoneyKeepCapture` runs as the virtual account
  `NT SERVICE\MoneyKeepCapture` (service SID type unrestricted), not LocalSystem.
- Data folder: `%ProgramData%\MoneyKeep\capture-server`. The elevated helper creates it
  and sets its permissions with inheritance turned off: full control for SYSTEM and
  Administrators, modify for `NT SERVICE\MoneyKeepCapture`, nothing for anyone else.
  The parent `%ProgramData%\MoneyKeep` gets the same owner and no write access for
  Users. If the folder already exists, the helper checks it is a real folder (not a link
  or junction) and that its owner is SYSTEM or Administrators before using it. If either
  check fails, it refuses with "The capture server's folder isn't safe to use: <path>".
- The service's command line passes this folder as `--data`. The desktop app talks to
  the server only over its API (as now), so it needs no access to the folder.
- **Moving existing data:** when the helper finds the service using a folder under a
  user profile (the old `%LOCALAPPDATA%\MoneyKeep\data\capture-server`), it stops the
  service and copies the known files (store, inbox, certificates, keys, `wifi.json`,
  settings) into the new folder. It copies file contents only and doesn't follow links:
  any entry that is a link is skipped and reported. It then points the service at the
  new folder and starts it. The old folder is left in place. The Status card offers
  "Remove the old copy" for 30 days afterwards.
- Private keys stay sealed (DPAPI with machine scope, or for the service's own account).
  `inbox.key` is no longer written beside them as plain text (L5): the service seals it
  the same way.
- The Wi-Fi certificate and the ports keep working unchanged. Phones need no new
  pairing.
- The uninstaller and Start over (revision 17) remove the new folder, and the old one if
  it still exists. Before deleting, the uninstaller checks for links as in A2's rule
  (L8).

### A2. Only Cloudflare's own cloudflared is run (H2, M1)

- cloudflared is looked for only in the folders its MSI installs to (`%ProgramFiles%\cloudflared`,
  `%ProgramFiles(x86)%\cloudflared`), never on PATH or in user folders.
- Before running it as administrator, the helper checks its Authenticode signature:
  valid, chained to a trusted root, and an exact signer subject match for Cloudflare's
  organisation (O=Cloudflare, Inc., C=US, as published). A file that fails is not run,
  and the user sees "The cloudflared program on this computer isn't signed by Cloudflare,
  so MoneyKeep won't run it."
- The downloaded MSI goes into a folder the helper creates under `%ProgramData%\MoneyKeep\tmp`
  (administrators only), not the user's temp folder. It is kept open with no write or
  delete sharing from the signature check until `msiexec` has finished. The same exact
  signer rule applies to it.

### A3. The helper only does what the user approved (M2)

- The desktop app writes the plan, computes its SHA-256 and passes it to the helper on
  the command line (`--plan-sha256`). The helper reads the plan once, with no write
  sharing, checks the hash and refuses on a mismatch ("The setup plan changed after you
  approved it. Nothing was changed.").
- Whatever the plan says, the helper only:
  - removes and adds the firewall rule named "MoneyKeep capture (Wi-Fi)", for TCP 14443
    and 14080, for the capture server's program path;
  - removes Block rules for that program path (revision 18);
  - uses the data folder from A1 and the one old folder that the service's current
    command line names;
  - installs the tunnel token the plan carries (the token is the user's own; the hash
    check protects it).
- A plan asking for anything else is refused whole.

### A4. Pairing can't stall the server (M3)

- The Wi-Fi listener reads request bodies with a 10-second timeout for unauthenticated
  routes (`/api/v1/pair` and anything else before a token is checked). Paired requests
  keep the current limits.
- Unauthenticated requests use their own worker, so they never occupy the workers that
  serve paired phones. At most 4 unauthenticated connections are open at a time, and no
  more than 2 from one address; more are closed at once.

### A5. Smaller capture-server items (L1, L2, L3, L6)

- **L1:** the tunnel token is passed to cloudflared as Cloudflare documents for keeping
  it off the command line, if it supports that (a token file or an environment variable
  read by the service). If it doesn't, the current behaviour stays, and the help page
  says that local administrators can see the token.
- **L2:** `CF-Connecting-IP` is believed only on requests that also carry a valid
  Cloudflare Access token. Otherwise the connection's own address is used.
- **L3:** every route reached through the tunnel requires a valid Access token, desktop
  API included (as the phone routes already do when Access is set).
- **L6:** `server.log` rotates at 5 MB, keeping 3 old files. Logged method text is
  limited to 16 printable characters.

### A6. When the capture service is missing (found in owner testing, 2026-11-10)

After Batch A was installed, the `MoneyKeepCapture` service did not exist (it was
already gone before Setup ran, so `services move-data` did nothing). The data was still
in the user's old folder, `%LOCALAPPDATA%\MoneyKeep\data\capture-server`, with its
Wi-Fi certificate, pairings and an inbox that may hold expenses not yet collected. The
Status card showed "the server isn't running" with only Check now, and the firewall Fix
was refused ("a firewall rule for other ports"), because the helper looks for the Wi-Fi
ports only in `server.toml` in the new folder.

- **Detecting it:** the Status check tells "not installed" apart from "stopped". When
  the service is not installed and a capture-server data folder with `server.toml`
  exists (the new folder, or the user's old folder), the card shows "Phone capture
  isn't working: the capture server isn't installed" and a primary button **Install the
  server again**. When the service is installed but stopped, the card offers **Start
  the server** (one permission prompt) in addition to Check now.
- **Install the server again:** one helper plan, one permission prompt. It prepares and
  protects the new folder as A1 does. When the new folder has no `server.toml` and the
  user's old folder has one, the old folder's known files are copied in (A1 rules: no
  links, refused junctions, inbox key sealed), and the old folder is marked as moved,
  so the 30-day **Remove the old copy** offer applies. The service is then installed
  with the virtual account and the new folder, started and health-checked. The Wi-Fi
  firewall rule is recreated in the same plan. On failure nothing in the old folder
  changes and the service is removed again; the card shows the error.
- **Where the old folder may be:** A3 allows a plan to name a source folder only when it
  is the helper's own `%LOCALAPPDATA%\MoneyKeep\data\capture-server` (the helper runs
  elevated as the same user) or the folder from before the rename; any other folder is
  refused whole.
- **Ports for the firewall rule (A3):** the helper takes the expected ports from, in
  order, the plan's Wi-Fi step, `server.toml` in the new folder, then `server.toml` in
  the allowed source folder. A Fix plan is never refused only because the data has not
  moved yet.
- **Order of problems:** while the service is not installed, the firewall banner is not
  shown; reinstalling recreates the rule. A Fix button is never offered for a problem
  the helper would refuse.
- **Setup (installer):** unchanged when the service is missing; it doesn't install a
  service the user had removed. The desktop app makes the offer above.

## Batch B — book data, connections, packaging, housekeeping

### B1. Restore never replaces a file (M4)

- Restore reserves the target name by creating it with `create_new`, writes to a
  temporary file, and moves it into place with a rename that refuses to replace (Windows
  `MoveFileEx` without `MOVEFILE_REPLACE_EXISTING`; elsewhere `renameat2` with
  `RENAME_NOREPLACE`, or a hard link and then unlink). If the name was taken in the
  meantime, it picks the next free name (`… restored … (2)`). The same applies to the
  copy in `encrypted.rs`.

### B2. Adding a password protects old backups too (M5)

- After a password is added, MoneyKeep counts the book's unprotected backups in the
  backup folder and the second place. It asks: "<n> earlier backups of this book aren't
  password-protected. Protect them now / Delete them / Keep them". Protect now re-writes
  each one encrypted with the new password, checks it, then deletes the plain copy.
- Keep them leaves a Home note ("<n> backups of <book> aren't password-protected") with
  the same two actions.
- The CLI's `password add` prints the same count and offers `moneykeep backups protect
  <book>`.

### B3. SnapTrade requests are signed when sent (M7)

- The timestamp and signature are made for each request when it is sent, including
  retries and later pages. A 401 is reported as "the personal key was rejected" only if
  the same request, signed fresh, is refused a second time.

### B4. Third-party licences are shipped (M8)

- `cargo xtask release` and `installer` generate `THIRD-PARTY-LICENSES.txt`. It lists
  every crate compiled into the shipped programs, with its version and licence, and the
  licence text (with copyright lines) for each licence and holder. It is generated from
  `cargo metadata` and the crates' licence files, with no new dependency. If one is
  needed, it must use an allowed licence.
- The installer installs the file. About gets a "Third-party licences" link that opens it.
- `NOTICE` says the book pictures are under the Pexels, Pixabay and Unsplash licences,
  points to `ATTRIBUTION.md`, and names Slint's licence (L12).

### B5. Smaller items (L4, L7, L10, L13)

- **L4:** the password settings read from a book are bounded (memory at most 1 GiB,
  iterations and lanes within the ranges MoneyKeep writes, times 4). A book outside them
  is refused with "This book's password settings aren't ones MoneyKeep makes."
- **L7:** the CLI's password commands make the before-update backup first, as the app
  does.
- **L10:** `.gitignore` adds `*.moneykeep.prev`, `*.moneykeep.tmp-*`, `*.pfx`, `*.p12`,
  `*.p8`, `*.pem` and `*.key` (keeping tracked test keys working by explicit `!` lines
  or by moving them under a fixtures path that is listed).
- **L13 (D4):** in `AGENTS.md`, `CLAUDE.md`, `.github/pull_request_template.md` and
  `docs/clean-room.md`:
  - each target product's name becomes "commercial finance products";
  - their file-format names become "their proprietary file formats";
  - the trademark notice names no company; it refers to "trademarks of commercial finance
    products".
  - The rules themselves don't change. The names check in xtask is extended so the
    product names can't come back into these files.

## Other repositories

Each is done in its own repository by an implementer and delivered by bundle like
MoneyKeep.

- **keep-launcher:**
  - **M9 (D2):** `release.yml` (and the arm64 and macOS workflows, if present) become
    self-contained workflows in this repository, with no `uses:` of another
    organisation's workflow. Signing secrets are used only in its own steps. Third-party
    actions are pinned by full commit SHA. `sonarcloud.yml` is removed, or made
    self-contained if the owner wants SonarCloud; the default is to remove it.
  - **L9:** `sign.ps1` passes no password on the command line; it uses the same rule as
    MoneyKeep's release tool (release-prep §4).
  - **L13:**
    - Add a clean-room section to `AGENTS.md` matching MoneyKeep's (with D4 wording).
    - Make it say the catalog has five apps.
    - Correct what `AGENTS.md` and `CLAUDE.md` say about how workflows are called.
  - **M8:** ship `THIRD-PARTY-LICENSES.txt` as in B4.
  - **L10:** the `.gitignore` additions above.
- **form1099keep:**
  - **M6:** Form1099Keep doesn't update a book's format.
    - An older book gets "Open this book in MoneyKeep first, so it can update it safely."
    - A newer one gets "This book was made by a newer MoneyKeep. Update Form1099Keep."
    - It writes its marker only to a book at the format it understands, and only after
      the user confirms opening.
  - **L11 (D3):** delete the uncompiled prototype files (`src/app.rs`, `src/db/`,
    `src/ui/`, `src/exporters/`, `src/csv_import/`, `src/validation/`) and their
    references in `src/lib.rs` comments.
  - **L13:** `CLAUDE.md` gets its own copy of the clean-room rules (D4 wording) instead
    of pointing to `../moneykeep/AGENTS.md`.
  - **M8, L10:** as above, adding `*.moneykeep` to `.gitignore`.
  - **L12:** README and `NOTICE` state Slint's licence in text, not only a badge.
- **calckeep, groupkeep:**
  - **L13:** add an `AGENTS.md` with the clean-room rules (D4 wording).
  - **M8, L10, L12:** as above.
- **L8 (all installers):** before a recursive delete, the uninstaller checks that the
  folder is a real folder, not a junction or link, and that it is under the expected
  profile or ProgramData path. If either check fails, it leaves the folder and says so in
  the details. When a separate administrator account approved the uninstall, the
  profile checked is the one the uninstall was started from (the original user's, passed
  by the unelevated stub), not the administrator's.

## Tests

- **SR-A1:** the service is installed with the virtual account and a data folder under
  ProgramData. The folder check rejects a junction and a folder owned by Users. The move
  copies the known files, skips a link and leaves the old folder.
  - Tested with a fake for the service manager and the file system; a Windows-only test
    runs when on Windows.
- **SR-A2:** a cloudflared outside the install folders is not used. A wrongly signed one
  is refused with the message. The MSI handle has no write sharing between check and
  install (fake).
- **SR-A3:** a plan whose hash doesn't match is refused with nothing changed. A plan
  naming another firewall rule, another port or another folder is refused whole.
- **SR-A4:** two idle pair requests don't block a paired upload, which succeeds within 2
  seconds. A body that doesn't arrive within 10 seconds is closed. A fifth
  unauthenticated connection is closed.
- **SR-A5:**
  - L2: a forged `CF-Connecting-IP` without an Access token is ignored.
  - L3: a desktop route through the tunnel without an Access token gets 403.
  - L6: `server.log` rotates at 5 MB.
- **SR-A6:**
  - With no service and `server.toml` only in the old user folder, the card shows "the
    capture server isn't installed" with Install the server again, and no firewall
    banner.
  - Install the server again copies the old folder's files into the new one, seals the
    inbox key, marks the old folder moved, installs the service with the virtual account
    and the new folder, recreates the firewall rule with the old folder's ports, and
    the Wi-Fi pairing and inbox items are still there afterwards.
  - A plan naming any other source folder is refused whole.
  - A firewall Fix plan with `server.toml` only in the old folder is accepted with its
    ports.
  - A stopped (installed) service shows Start the server, which starts it.
  - A failed install leaves the old folder untouched and no service behind.
- **SR-B1:** restore when the target name appears during the copy writes `(2)` and
  leaves the other file byte for byte.
- **SR-B2:** adding a password with 3 plain backups shows the count. Protect now leaves 3
  encrypted backups that open with the password and no plain ones.
- **SR-B3:** two requests a fake minute apart carry different timestamps. A retry is
  re-signed.
- **SR-B4:** the release dry run produces `THIRD-PARTY-LICENSES.txt` listing every crate
  in `cargo metadata` for the shipped programs. The installer script installs it.
- **SR-B5:**
  - L4: a book asking for 4 TiB is refused before deriving.
  - L7: CLI `password add` makes a backup first.
  - L13: the names check fails if a product name is put back.

## Ledger effect

None. Restored copies and protected backups change files only, not amounts.

## Clean-room check

Written from the review's findings, Microsoft's, Cloudflare's and SnapTrade's own
documentation, and the owner's decisions.
