# Spec: macOS releases on a self-hosted Mac runner

- **Area / module:** release, packaging, CI (all five repositories)
- **Spec writer:** Claude session 01M83vYXyuJCXqRVVqa86eVD
- **Date:** 2026-11-11
- **Status:** reviewed, owner decisions O1–O3 recorded 2026-11-11

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Vendor doc | GitHub Docs, "About self-hosted runners", "Adding self-hosted runners", "Configuring the self-hosted runner application as a service", "Managing access to self-hosted runners using groups", "Security hardening for GitHub Actions" (self-hosted runners in public repositories) | Registration, labels, runner groups limited to repositories and workflows, `svc.sh`, fork-PR risk |
| 2 | Vendor doc | GitHub Docs, "Approving workflow runs from public forks" | Require approval for outside collaborators |
| 3 | Vendor doc | Apple Developer documentation, "Notarizing macOS software before distribution" and `xcrun notarytool` help (`store-credentials`, `--keychain-profile`) | Notarization without secrets in CI |
| 4 | Vendor doc | Apple `codesign`, `security` (create-keychain, set-keychain-settings, unlock-keychain), `lipo`, `hdiutil` man pages | Signing keychain, universal binaries, .dmg |
| 5 | Project | keep-launcher `packaging/macos/` and its `release.yml` `macos` job | The project's own existing Mac packaging; reused |
| 6 | Project | `docs/specs/release-packaging.md`, `security-review-1.md` (M9 pinned actions, L9 no password on a command line), `upgrade-resilience-1.md` U3 | Release rules this spec extends |

## Purpose

The owner wants macOS builds of MoneyKeep, CalcKeep, GroupKeep, Form1099Keep and the
launcher, made on a Mac the owner controls (a self-hosted GitHub Actions runner) rather
than on GitHub's Mac machines, signed and notarized so they open without warnings, and
published with each release automatically. Pull-request testing stays on GitHub's own
machines so strangers' code never runs on the owner's Mac.

## Owner decisions (2026-11-11)

- **O1. Apple Developer ID: ad hoc until the owner enrols.** Builds are signed ad hoc
  now; macOS warns on first open (right-click → Open), and the launcher does not
  install them (keep-launcher AGENTS.md: an app without a Mac team is only opened if
  present). When the owner enrols (in MoneyKeep Project's name), Part R step 8 is done
  and `catalog::MONEYKEEP_TEAM_ID` is set in the launcher; no other change is needed.
- **O2. Phone capture is in the first Mac release** (Part C).
- **O3. Each release's Mac job waits for the owner's approval.** The `macos` job uses a
  GitHub environment `mac-release` with the owner as required reviewer, in every
  repository. The other release jobs do not wait.

## Part R — the runner (owner setup, done once; not code)

A runbook, `docs/deploy/mac-runner.md`, written by the implementer, with these steps:

1. **Machine.** An Apple Silicon Mac, macOS 14 or later, at least 60 GB free, on mains
   power and wired network if possible.
2. **A separate macOS user, `mkbuild`** (standard, not administrator). The runner, its
   tools and the signing keychain live only in this user. The owner's own account is
   not used.
3. **Power.** System Settings: prevent sleep when the display is off; start up
   automatically after a power failure; log `mkbuild` in automatically (the runner is
   a per-user launchd agent and needs the session). FileVault users: note that
   automatic login is unavailable with FileVault on; then the Mac must be logged in by
   hand after a restart, and the runbook says so.
4. **Tools,** installed as `mkbuild`: Xcode Command Line Tools (`xcode-select
   --install`); rustup with the stable toolchain and targets `aarch64-apple-darwin`
   and `x86_64-apple-darwin`; `trunk` and the `wasm32-unknown-unknown` target only if
   a release job needs them. Nothing installed with Homebrew is required.
5. **Register the runner at the organisation** `moneykeepproject` (Settings → Actions
   → Runners → New self-hosted runner → macOS, ARM64), name `mk-mac-1`, extra label
   `mk-mac`. Install as a service: `./svc.sh install` then `./svc.sh start`.
6. **Runner group `mac-release`** containing only `mk-mac-1`:
   - Repository access: selected repositories — the five.
   - Workflow access: selected workflows — each repository's
     `.github/workflows/release.yml` at `refs/tags/*` (and `refs/heads/main` for a
     manual run).
   - "Allow public repositories" on (needed once they are public).
   The `Default` group must not contain the Mac.
7. **Forks.** Organisation → Actions → General: "Require approval for all outside
   collaborators" for fork pull-request workflows; "Read repository contents" as the
   default `GITHUB_TOKEN` permission.
8. **Signing keychain** (after O1). As `mkbuild`:
   - Create `mk-signing.keychain-db` with its own password, add it to the search list,
     and set it never to lock (`security set-keychain-settings` with no timeout and no
     lock on sleep).
   - Import the Developer ID Application certificate and key (`.p12`) into it once, and
     allow `codesign` to use the key without prompting (`security
     set-key-partition-list` for `apple-tool:,apple:,codesign:`).
   - Store notarization credentials:
     `xcrun notarytool store-credentials mk-notary --keychain mk-signing.keychain-db`
     (Apple ID, team ID, app-specific password, entered at the prompt).
   - The `.p12` file is then deleted from the Mac. No Apple secret is stored in GitHub.
   - Until O1 is decided, skip this step (builds are signed ad hoc).
9. **Check.** Run the "Mac runner check" workflow (Part W, W6) by hand; it must pass.
10. **Upkeep.** The runner application updates itself. Monthly: macOS updates; `rustup
    update`; check free space (the shared build folder can be deleted at any time).

## Part W — workflows (all five repositories)

- **W1. Which machine.** Each repository's `release.yml` has a `macos` job with
  `runs-on: [self-hosted, macOS, ARM64, mk-mac]`. No other workflow in any repository
  names `self-hosted`.
- **W2. Triggers.** `release.yml` runs only on a pushed version tag (`v*`) and on
  `workflow_dispatch`. It has no `pull_request`, `pull_request_target`,
  `issue_comment` or `workflow_run` trigger. `workflow_dispatch` gains an input
  `macos` with choices `runner` (default), `github` (use GitHub's `macos-15`, signing
  from secrets as the launcher does today, for when the Mac is away) and `skip`
  (publish the release without a Mac asset, noted in the release text). A tag push
  always uses `runner`.
- **W3. Clean checkout, kept build cache.** The job checks out with
  `persist-credentials: false` and `clean: true`, then removes anything untracked
  (`git clean -ffdx`). Builds use `CARGO_TARGET_DIR=$HOME/mk-build/<repo>` (outside
  the workspace) and `CARGO_INCREMENTAL=0`, so later releases rebuild only what
  changed. The job deletes that folder first when `rustc --version` differs from the
  one recorded in it.
- **W4. Signing on the runner.** `packaging/macos/package.sh` signs with the
  Developer ID identity found in `mk-signing.keychain-db` and notarizes with
  `notarytool submit --keychain-profile mk-notary --wait`, then staples. If the
  keychain or profile is missing it signs ad hoc and prints a warning (O1). When
  `macos: github` is chosen it uses the existing secret-based import instead. No
  password or Apple ID appears on any command line or in the log (L9).
- **W5. Waiting.** `timeout-minutes: 120` on the job. A queued job waits for the Mac
  (GitHub cancels it after 24 hours); the publish job waits for it. The runbook says
  to re-run with `macos: github` or `skip` if the Mac is away.
- **W6. "Mac runner check" workflow** (`mac-runner-check.yml`, `workflow_dispatch`
  only, in MoneyKeep): on the runner, prints macOS, Xcode tools, rustup targets and
  free space; checks the signing identity and the `mk-notary` profile are present
  (or reports "ad hoc only"); signs and verifies a tiny test binary. It is in the
  runner group's allowed workflows.
- **W7. Pull-request testing** is unchanged: `ci.yml` keeps `macos-latest` (GitHub's
  machines) for build and tests.
- **W8. Actions.** Only GitHub's own actions, pinned by full commit SHA (M9).
- **W9. Check in code.** Each repository's xtask workflow check (MoneyKeep's `cargo
  xtask workflows`, and an equivalent test in the others) fails if any workflow other
  than `release.yml` and `mac-runner-check.yml` names `self-hosted`, or if a workflow
  naming `self-hosted` has a trigger other than `push` of tags and `workflow_dispatch`.

## Part P — Mac packaging (MoneyKeep, CalcKeep, GroupKeep, Form1099Keep)

Each app gets `packaging/macos/` modelled on the launcher's (Source 5): `Info.plist.in`,
`entitlements.plist`, `package.sh`, `verify.sh`, `dmg/`.

- **P1. Universal binary.** Build `aarch64-apple-darwin` and `x86_64-apple-darwin`
  release binaries and join them with `lipo`. Minimum macOS 11.0, like the launcher.
- **P2. Bundle.** `<App>.app` with bundle identifier `com.moneykeep.<repo>`
  (`com.moneykeep.moneykeep`, `com.moneykeep.calckeep`, `com.moneykeep.groupkeep`,
  `com.moneykeep.form1099keep`), the app icon as `.icns` made from the existing icon
  at build time, the version from the workspace, `THIRD-PARTY-LICENSES.txt` and
  `NOTICE` in `Contents/Resources`. Hardened runtime on; entitlements only what the
  app needs (none beyond the defaults, unless the implementer finds one is required and
  lists it).
- **P3. Disk image.** `<repo>-<ver>-macos-universal.dmg` with the app and an
  Applications shortcut, signed and notarized like the app. `SHA256SUMS.txt` includes
  it.
- **P4. Data locations** follow each app's existing macOS paths (for MoneyKeep the
  `dirs` locations already used: `~/Library/Application Support/MoneyKeep`, backups in
  `~/MoneyKeep Backups`). No change.
- **P5. MoneyKeep extras.** `Contents/MacOS` also holds `moneykeep-server` (the
  capture server and its setup commands, Part C) and the MCP server binary if the
  Windows installer ships it. Each is signed with the same identity, inside-out
  before the bundle.
- **P6. Launcher catalog.** Each app's macOS asset name is added to the launcher's
  catalog, with `mac_team = catalog::MONEYKEEP_TEAM_ID` (None until O1).
- **P7. `cargo xtask` entry.** `cargo xtask installer` on macOS runs
  `packaging/macos/package.sh`; on other systems it is unchanged.

## Part C — phone capture on the Mac (O2)

The Windows design (a machine service with low rights, an administrator helper) is
replaced on macOS by a per-user launchd agent, which needs no administrator rights.
`services.rs` already has a `macos::Launchd` agent; this part makes it complete and
tested.

- **C1. Where things live.** Server program: the copy inside the app bundle,
  `/Applications/MoneyKeep.app/Contents/MacOS/moneykeep-server` (the agent's plist
  points at the bundle's path as installed; if the app is moved, Status shows "Install
  the server again"). Data: `~/Library/Application Support/MoneyKeep/capture-server`
  (permissions 0700). Agent: `~/Library/LaunchAgents/<existing label>.plist`,
  `RunAtLoad` and `KeepAlive` on (restart after a crash, after 10 s:
  `ThrottleInterval` 10). Logs: `server.log` in the data folder as on Windows.
- **C2. When it runs.** A user agent runs while that user is logged in. Help and the
  Status card say: "On a Mac, phone capture works while you're logged in. Expenses
  sent while you're logged out wait on your phone." Fast user switching keeps it
  running.
- **C3. Setup and status** use the same Settings → Phone capture screens and wording as
  Windows wherever they apply. Install, Start, Stop, Update and Remove use
  `launchctl bootstrap gui/<uid>`, `kickstart -k`, `bootout`; no password prompt.
  `services status` reports the same states as on Windows, including the U1.4 refusal
  exit codes (read from `launchctl print`'s last exit status).
- **C4. The inbox key** is sealed with the macOS login keychain through the existing
  `keyring` dependency (service "MoneyKeep capture server", account the data folder's
  path) instead of DPAPI. The U1.2 rules apply: a key that can't be opened falls back
  to the plain key and is sealed again; with no key the server refuses and never makes
  a new one while the inbox has rows.
- **C5. Wi-Fi capture.** The listener and certificate are unchanged. macOS's
  application firewall (when on) asks once whether `moneykeep-server` may accept
  incoming connections. Help explains choosing Allow. Because ad hoc builds (O1) change
  signature with every version, macOS may ask again after each update; with a
  Developer ID it asks once. The firewall card on Windows (rules and Fix) is not shown
  on macOS; instead, if the phone can't reach the server and the firewall is on
  (`/usr/libexec/ApplicationFirewall/socketfilterfw --getglobalstate`), the Status card
  says "macOS's firewall may be blocking phone capture" with **Open Firewall settings**
  (opens System Settings → Network → Firewall).
- **C6. Cloudflare capture.** The existing rule stays: MoneyKeep does not download
  cloudflared on macOS. The dialog says to install Cloudflare's own cloudflared
  package, then press **Check again**. MoneyKeep runs cloudflared only from
  `/usr/local/bin/cloudflared` or `/opt/homebrew/bin/cloudflared` (never PATH), and
  only if `codesign --verify` passes and its Team ID is Cloudflare's, `68WVV388M8`
  (the same exact-signer idea as A2). The owner checked this on 2026-11-11 against
  Cloudflare's own `cloudflared-darwin-arm64.tgz` release (`codesign -dv`:
  `TeamIdentifier=68WVV388M8`), and it matches Cloudflare's WARP app. A Homebrew
  build of cloudflared is compiled on the Mac and isn't signed by Cloudflare, so it
  is refused with the not-signed-by-Cloudflare message; the dialog points to
  Cloudflare's own download. The tunnel is installed as a per-user agent with
  the token file in the data folder (0600), not with `cloudflared service install` as
  root; the existing `osascript` administrator path is removed.
- **C7. Moving data, U1 safety.** The U1.1 move, U1.2 atomic writes, U1.3 old-copy and
  U1.4 format-number rules apply unchanged; there is no machine-data folder on macOS,
  so no move happens there.
- **C8. Uninstall.** Dragging the app to the Trash leaves the agent pointing at a
  missing program; launchd stops retrying it. MoneyKeep's Settings → Phone capture →
  **Remove phone capture from this Mac…** boots the agents out, deletes their plists
  and offers to delete the data folder (default: keep). Help says to do this before
  deleting the app.

## Batches

- **R (owner):** Part R steps 1–7 now; step 8 after O1.
- **MR1:** keep-launcher — W1–W9 for its existing `macos` job; runbook
  `docs/deploy/mac-runner.md` lives in MoneyKeep and the launcher links to it.
- **MR2:** MoneyKeep — Part P (P1–P7), W1–W9, `mac-runner-check.yml`, the runbook.
- **MR2c:** MoneyKeep — Part C (phone capture on the Mac).
- **MR3:** CalcKeep, GroupKeep, Form1099Keep — Part P and W1–W9 each.

## Tests

- **MAC-1.** Workflow check (W9) passes, and fails on a test fixture that adds
  `runs-on: self-hosted` to a `pull_request` workflow.
- **MAC-2.** `release.yml` parses; its `macos` job's `runs-on` is the W1 list when
  `macos` is `runner`, `macos-15` when `github`, and the job is skipped when `skip`.
- **MAC-3.** `package.sh --dry-run` on Linux prints the planned steps (build two
  targets, lipo, bundle layout, sign identity or "ad hoc", notarize profile, dmg name)
  without running Apple tools, and the bundle identifier and asset name match P2/P3.
- **MAC-4.** On the runner (manual, recorded in the release checklist): `codesign
  --verify --deep --strict`, `spctl --assess --type execute` (with O1), `xcrun stapler
  validate`, and the app opens on a second Mac with no warning.
- **MAC-5.** `release.yml`'s `macos` job names environment `mac-release` (O3); W9
  fails if it doesn't.
- **MAC-6.** The launchd plist written by C1 (golden test): label, program path inside
  the bundle, data folder argument, `RunAtLoad`, `KeepAlive`, `ThrottleInterval` 10,
  log paths; XML-escaped paths with spaces and `&`.
- **MAC-7.** C4 with a mock key store: seal, open, damaged-seal fallback, refusal with
  rows and no key.
- **MAC-8.** C6: cloudflared outside the two paths, or with the wrong Team ID, is
  refused with the A2-style message; no `osascript` call remains in `services.rs`.
- **MAC-9.** Screenshots with the platform forced to macOS, light and dark:
  `phone-capture-macos` (Status working, the logged-in note) and
  `phone-capture-macos-firewall` (the C5 card).
- **MAC-10.** On the runner (manual, release checklist): install from the .dmg, set up
  Wi-Fi capture, pair a phone, send an expense, log out and back in, update to the
  next build, and confirm the expense, pairing and certificate survive.

## Ledger effect

Non-posting.

## Settings

None.

## Clean-room check

Sources are GitHub and Apple vendor documentation and the project's own code. No
target product was observed for this spec.
