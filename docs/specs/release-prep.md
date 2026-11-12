# Spec: Alpha release preparation (fixes before 0.1.0-alpha.1)

- **Area / module:** reports (tax summary), installer, build tooling, projects, capture
- **Spec writer:** spec-writer session (Claude), with the owner
- **Date:** 2026-11-08
- **Status:** reviewed — owner approved the batch 2026-11-08

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Spec | `investments-options-bonds-crypto.md` (revisions 2, 3), `reports.md` (tax summary), `release-packaging.md`, `projects.md`, `mobile-capture.md` | Behaviour being corrected |
| 2 | Vendor doc | NSIS documentation (`ReadRegStr`, `SetShellVarContext`, `VIProductVersion`) | Installer details |
| 3 | Standard | Semantic Versioning 2.0.0 (semver.org) | Pre-release versions such as `0.1.0-alpha.1` |
| 4 | Owner decisions | Conversation 2026-11-08 | Fix the release blockers, set the alpha version, then release notes |

## 1. Tax summary: no amount counted twice

Problem: the Tax summary lists accounts by tax line ("activity") and, since revision 2 of
the investments spec, also an **Investment income** section (interest by kind, crypto
rewards, fees, realized gains). When an account behind one of those rows (Investment
Fees, Crypto Rewards, an interest account) also has a tax line, the same money appears in
both places, and a preparer adding the page up counts it twice.

Rule:
- In the Investment income section, each row says where its money comes from. A row
  whose amount is posted to an account that has a tax line shows the amount and, in the
  place of a separate figure, the note "Included above under <tax line>" (PDF and screen)
  and `included_in` = the tax line in the data and CSV. Such rows are left out of the
  section's own total.
- Rows that come from lots rather than accounts (realized short/long-term gains, and
  accrued interest adjustments) are never "included above" and always count.
- The section's heading gets one line: "Details of investment income. Amounts already
  listed under a tax line above are marked and not added again."
- `report.investment_income` itself is unchanged (it is not a tax page).
- Tests: RP-T1. Investment Fees tagged with a tax line and a coin fee of 6.00: the fee
  appears once under that tax line and in the Investment income section marked "Included
  above under …", and the section total excludes it. RP-T2. Same book with the tag
  removed: the fee is counted in the section total and not under any tax line. RP-T3. A
  realized gain always counts in the section total.

## 2. Uninstall removes the right capture-server data

Problem: the uninstaller removes `%LOCALAPPDATA%\MoneyKeep\data\capture-server` of the
account that approved the administrator prompt, which may not be the user whose data the
capture service uses.

Rule:
- Before removing the capture service, the uninstaller reads the service's registered
  command line (as the installer already does to find it) and takes its `--data <folder>`
  value. That folder is the one offered for removal.
- The question shows the folder: "Also remove the capture server's data (certificates
  and paired phones) in <folder>?" (default No).
- When the service is not installed or has no `--data`, the uninstaller looks in the
  current user's `%LOCALAPPDATA%` as now; when nothing is found it asks nothing.
- The folder is removed only if it contains the capture server's own marker file (the
  server's store file); otherwise the uninstaller leaves it and says so in the details.
- Test (RP6 extension, run when `makensis` is present): the script reads the service
  command line and builds the question from its `--data` value (static check of the
  script plus a unit test of the parsing helper in xtask if the parsing is done there).

## 3. Version 0.1.0-alpha.1

> **Revised 2026-11-11 (owner decision):** the first public release is plain `0.1.0`,
> not `0.1.0-alpha.1`, in all five repositories, so the launcher offers it without the
> pre-release setting. `VERSION_QUAD` is then `0.1.0.0`, the installer file
> `moneykeep-0.1.0-windows-x64-setup.exe`, and the dry-run test lists `MoneyKeep-0.1.0`.
> The rules below stay as written for any later pre-release (`-alpha.N`, `-beta.N`,
> `-rc.N`).

- The workspace version becomes `0.1.0-alpha.1` (semver pre-release, source 3) for the
  desktop app, CLI, keep-mcp, the capture server and the phone app.
- Shown as "0.1.0-alpha.1" in About, the start screen footer, `--version` and
  `version.json`; installer file `MoneyKeep-Setup-0.1.0-alpha.1.exe`.
- Windows file and installer versions need four numbers: `VERSION_QUAD` is `0.1.0.1`
  (the alpha number as the fourth part); the readable product version stays
  `0.1.0-alpha.1`.
- The capture version keeps working with the new version (its `+<hash>` part follows
  the full version).
- Upgrade check in the installer compares versions by semver (so `0.1.0-alpha.2` is newer
  than `0.1.0-alpha.1`, and `0.1.0` newer than both).
- Test: the release dry run lists `MoneyKeep-0.1.0-alpha.1` and the setup name; a unit
  test of the installer's version comparison (in xtask) for the three cases above.

## 4. Smaller fixes

- **Projects — Show closed:** tag pickers get the "Show closed" toggle the projects spec
  asked for (off by default; when on, completed and cancelled projects are listed,
  marked "(completed)"/"(cancelled)").
- **Projects — Billable costs post to:** the setting shows "Billable Expense Income" once
  that account exists (it was created on first use but the setting stayed blank).
- **Projects — billed links:** an invoice line's link to the costs it billed is kept by
  the split's own id, not by the transaction and line position, so editing the billed
  transaction (reordering lines) keeps the link. Existing links are converted when the
  book is opened (migration). Test: reorder lines of a billed transaction; the item still
  shows billed with its invoice number.
- **Capture version:** the root `Cargo.toml` is included in the hashed paths (it carries
  workspace settings that change the built server).
- **Signing password:** `MK_SIGN_PASSWORD` is never put on a command line. signtool is
  given the certificate through a file only readable by the user, or the password through
  standard input where signtool supports it; if neither is possible, signing asks for the
  certificate to be installed in the user's certificate store and signs by its thumbprint
  (`MK_SIGN_THUMBPRINT`). Test: the dry run's printed signtool command contains no
  password.
- **Unused message:** the unused `UPDATE_FAILED` constant in `capture_setup.rs` is
  removed or used for the failure message it names (the implementer picks, keeping the
  wording "The capture server was not updated" if used).

## 5. Where MoneyKeep is published (owner, 2026-11-08)

- The GitHub organisation is **moneykeepproject**; the repository is
  `github.com/moneykeepproject/moneykeep`. The workspace `repository` field (now
  `https://github.com/OWNER/moneykeep`) becomes that address, and About shows it as the
  project's page.
- The owner's keep-launcher (a separate repository) must use the same owner
  (`moneykeepproject`) to find releases; that change is made in keep-launcher, not here.

## 6. Names (owner, 2026-11-08)

- Workspace `authors` = `["MoneyKeep Project contributors"]`; `LICENSE-MIT` copyright line
  `Copyright (c) 2026 MoneyKeep Project contributors` (and the same wording in `NOTICE`).
- The publisher shown by Windows (installer `PUBLISHER`, file version info "Company") is
  **MoneyKeep Project**.
- `*.bundle` is added to `.gitignore`.

## Ledger effect

None, except the billed-link migration (no amounts change).

## Clean-room check

Written from MoneyKeep's own specs, the NSIS and SemVer documents and the owner's
decisions.
