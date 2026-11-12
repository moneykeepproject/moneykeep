# Spec: Release build, Windows installer and capture server updates

- **Area / module:** build tooling (xtask), installer, desktop capture settings
- **Spec writer:** spec-writer session (Claude), with the owner
- **Date:** 2026-11-07
- **Status:** reviewed — owner decisions 2026-11-07: NSIS installer (zlib licence) approved; unsigned for now

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Spec | `mobile-capture.md` (revisions 2 D, 4, 4a, 10) | Server program found beside the app; version in health; Update server |
| 2 | Vendor doc | NSIS documentation (nsis.sourceforge.io/Docs) | Installer script language, `makensis`, uninstaller, `RequestExecutionLevel` |
| 3 | Owner decisions | Conversation 2026-11-06/07 | Users never build; one build plus an installer; update by a notice with **Update now**, never an unexplained permission prompt |

## Purpose

A user installs MoneyKeep and never builds anything. The desktop app, the capture server
and the phone app always come from the same build, so the server beside the app is
always the right one. When the running capture server needs updating, MoneyKeep says
what it will do and why Windows will ask for permission, and updates only when the user
presses **Update now**.

## Behaviour

### One release build

`cargo xtask release` (developers; replaces running `cargo build --release` and
`cargo xtask dist-capture` separately, which stay available):
1. Builds the phone app (wasm; installs the target and Trunk when missing, as
   `dist-capture` does), then the capture server with the phone app embedded, the
   desktop app, the CLI and keep-mcp, all in release mode from the same checkout.
2. Copies them into `dist/MoneyKeep-<version>/` (Windows: `.exe` names) with
   `LICENSE-*`, `NOTICE`, `ATTRIBUTION.md` and `version.json`
   (`{"version", "capture_version", "commit", "built"}`).
3. Prints the folder and "Run MoneyKeep from this folder, or build the installer with
   cargo xtask installer."

### Capture version (fewer false alarms)

- `capture_version` = `<server package version>+<12 hex>` where the hex is the start of
  the SHA-256 over the git tree ids (or, without git, the file contents) of the paths the
  server program and the phone app are built from (the server and phone app folders, the
  crates they depend on, and `Cargo.lock`), computed by a build script. Changes elsewhere
  (desktop-only code, docs, specs) do not change it.
- The server's health answer adds `capture_version`. The desktop compares its own
  `capture_version` with the running server's: equal → no message; different → the
  update notice below. A server without the field (older builds) is compared by build
  version as today.

### Update notice and Update now

- When the running server's `capture_version` differs from the desktop's **and** the
  server program beside the app has the desktop's `capture_version`:
  - Home → Attention: "A newer capture server is ready." (opens Settings → Phone capture)
  - Settings → Phone capture shows a card:
    **The capture server needs an update**
    "This MoneyKeep was updated, and the capture server that phones send to is still the
    older version. Updating replaces it with the version that came with this MoneyKeep
    and restarts it. It takes a few seconds; phones keep their pairing, and anything in
    a phone's Outbox is sent afterwards.
    Windows will ask for permission, because the capture server runs in the background
    for this computer (it is installed for all users)."
    **Update now** / **Later** (Later hides the Attention item until the app is opened
    again; the card stays).
  - **Update now** runs the existing update (permission prompt, stop, replace, start,
    check). Result: "Capture server updated to 0.9.3." or the failure in plain words with
    Details.
  - If the user declines the Windows prompt: "Not updated — Windows permission was not
    given. Phones can keep sending to the current server; press Update now when ready."
- When the program beside the app is **not** newer (a development build where only the
  desktop was rebuilt): no Attention item; the Settings card reads "This copy of
  MoneyKeep was built without its capture server. Install MoneyKeep with its installer,
  or (developers) build with cargo xtask release." **Update now** is not shown.
- The update is never started without the user pressing **Update now**.

### Windows installer

`cargo xtask installer` runs `cargo xtask release`, then `makensis` on
`installer/moneykeep.nsi`, producing `dist/MoneyKeep-Setup-<version>.exe`. When
`makensis` is missing it says how to get NSIS and stops.

The setup:
- Shows the licence (MIT/Apache notice), the install folder (default
  `C:\Program Files\MoneyKeep`), options **Start menu shortcut** (on), **Desktop
  shortcut** (off).
- Needs administrator permission (per-machine install; the capture server runs as a
  service). The first page says so in one line.
- Installs the desktop app, server, CLI, keep-mcp and the documents above; registers in
  Windows "Installed apps" with name, version, publisher "MoneyKeep", icon, size and the
  uninstaller.
- **Upgrade in place:** when a version is installed, the setup says "MoneyKeep <old>
  will be updated to <new>. Your books and settings are kept." If the capture server
  service is installed from this folder, it is stopped before files are replaced and
  started after; the setup then needs no further update in the app (the running server
  has the new `capture_version`). If MoneyKeep is open, the setup asks to close it.
- **Uninstall** removes the program files, shortcuts and registration; it stops and
  removes the capture service and its firewall rule if installed, and asks "Also remove
  the capture server's data (certificates and paired phones)?" (default No). Books are
  never touched.
- Unsigned by default. When `MK_SIGN_CERT` (a .pfx path) and `MK_SIGN_PASSWORD` are set,
  the app, server and setup are signed with `signtool` and a timestamp; otherwise the
  output says "Not signed: Windows SmartScreen will warn on other computers."
- The app's icon (the calculator) is the setup and uninstaller icon.

### Tests

RP1. `xtask release` (on CI without wasm: a dry run listing the steps and files) writes
     `version.json` whose `capture_version` matches the server's and the desktop's.
RP2. `capture_version` is unchanged by a change in `docs/` or a desktop-only crate, and
     changes with a change in the server or phone app sources.
RP3. Running server with a different `capture_version` and a matching program beside
     the app → Attention item and the card with **Update now**; equal → neither.
RP4. Program beside the app not matching → the "built without its capture server" text
     and no Update now, no Attention item.
RP5. Update now with the permission declined → the "Not updated" message; accepted →
     "Capture server updated to …".
RP6. `makensis` compiles `installer/moneykeep.nsi` (run in CI when `makensis` is
     present, skipped with a note otherwise).

## Ledger effect

None.

## Settings

None new.

## Clean-room check

Written from MoneyKeep's own specs, the NSIS documentation and the owner's decisions.

## Revision 2 (2026-11-08) — installable by keep-launcher (owner decision: Windows-only alpha)

Sources: the owner's keep-launcher (`crates/core/src/platform.rs`): every MoneyKeep app
release names its files `<app>-<version>-<os>-<arch>[-<kind>].<ext>` and carries a
`SHA256SUMS.txt`; the launcher is extended (same batch) to install a new Windows kind,
`-setup.exe` (an NSIS setup run silently). NSIS documentation: silent installs (`/S`),
`IfSilent`, `/D=` install folder.

- **File names:** `cargo xtask installer` writes
  `dist/moneykeep-<version>-windows-x64-setup.exe` (lower-case app id; e.g.
  `moneykeep-0.1.0-alpha.1-windows-x64-setup.exe`) and `dist/SHA256SUMS.txt` with one
  line per published file (`<sha256>  <name>`, two spaces, as `sha256sum` writes).
  `cargo xtask release` keeps writing `dist/MoneyKeep-<version>/`.
- **Silent install** (`/S`, used by keep-launcher): no pages; the default options (Start
  menu shortcut on, Desktop shortcut off); upgrade in place as the normal setup; if
  MoneyKeep is open, it is asked to close and the setup waits up to 30 s, then fails with
  exit code 2 (the launcher shows "Close MoneyKeep and try again"); the capture service
  is stopped and started as usual. Exit code 0 on success, non-zero on any failure.
  Still per-machine (needs administrator permission — Windows asks).
- **Silent uninstall** (`QuietUninstallString`, already registered): removes the program;
  never removes the capture server's data (the question defaults to No and is not asked).
- **Registration** stays as revision 1 (DisplayName "MoneyKeep", DisplayVersion,
  Publisher "MoneyKeep Project", InstallLocation, UninstallString, QuietUninstallString);
  `InstallLocation` must be set (the launcher uses it to find the program).
- Tests: RP7. The dry run lists `moneykeep-0.1.0-alpha.1-windows-x64-setup.exe` and
  `SHA256SUMS.txt`. RP8. `SHA256SUMS.txt` lines match the files' hashes (unit test with
  small files). RP9. The script compiles with `makensis` (when present) and contains the
  silent-mode paths (static check: `IfSilent`/`${If} ${Silent}` around the pages'
  questions and the running-app wait).
