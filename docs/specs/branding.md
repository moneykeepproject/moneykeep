# Spec: Rename to MoneyKeep (owner request 2026-10-30)

- **Area / module:** whole project
- **Spec writer:** Claude session 2026-10-30 (spec-writer role)
- **Date:** 2026-10-30
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Owner request | 2026-10-30: the product is renamed **MoneyKeep**, tag line **"Apps for Financial Management"**; remove all references to the earlier template project and to the reference products from the product; rename the folder `moneykeep`. Owner decisions: keep the reference-product names only in the internal clean-room / provenance records; rewrite the project history | |

## Notation

The previous names are written with a middle dot (Money·Craft, mc·def): read them
without it. No file in the project spells the previous name in one piece, so a
history rewrite that replaces it cannot change the compatibility values (the code
builds them from parts).

## Name and tag line

- Product name **MoneyKeep** everywhere a name is shown or written: window titles,
  start screen heading (with the tag line "Apps for Financial Management" under it in
  secondary text), About text, the phone app (name "MoneyKeep capture", short name
  "Capture"), PDF footers/metadata (Producer "MoneyKeep"), CLI help and `--version`,
  MCP server name, error and notice texts, README, ROADMAP, docs, specs, comments.
- The tag line also appears in README's first lines and the Cargo package
  descriptions of the apps.

## Code and file names

| Old | New |
|---|---|
| crates `mc-core`, `mc-ledger`, `mc-store`, `mc-commands`, `mc-import`, `mc-mcp`, `mc-watch`, `mc-ui`, `mc-connect`, `mc-capture-api` | `mk-core`, `mk-ledger`, … `mk-capture-api` (Rust paths `mk_core`, …) |
| apps `money·craft-app`, `money·craft-cli`, `money·craft-server`, `money·craft-capture` | `moneykeep-app`, `moneykeep-cli`, `moneykeep-server`, `moneykeep-capture` (programs `moneykeep`, `moneykeep-cli`/CLI name `moneykeep`, `moneykeep-server`) |
| book file extension `.money·craft` | `.moneykeep` |
| definition files `.mc·def` | `.mkdef` |
| environment variables `MONEY·CRAFT_*` | `MONEYKEEP_*` |
| config/data folder (OS `ProjectDirs` "Money·Craft") | "MoneyKeep" |
| OS credential store service "Money·Craft" | "MoneyKeep" |
| Windows service `Money·CraftCapture` ("Money·Craft phone capture server") | `MoneyKeepCapture` ("MoneyKeep phone capture server") |
| install folder `…\Money·Craft\capture` (and Linux/macOS equivalents) | `…\MoneyKeep\capture` |
| systemd unit / launchd agent names containing money·craft | moneykeep |
| IndexedDB database name, service-worker cache names in the phone app | moneykeep… |
| temp folder `money·craft-receipts` | `moneykeep-receipts` |

## Compatibility and migration (no data loss)

1. **Books:** `.money·craft` files open exactly as before (same format). File dialogs
   offer `*.moneykeep;*.money·craft`; new books are saved as `.moneykeep`. Existing
   files are never renamed automatically; the start screen's tile menu gains **Rename
   file to .moneykeep** (renames on disk after closing it, updates the recent list).
2. **Config:** on first start, if the new config folder has no config and the old
   "Money·Craft" folder has one, it is **copied** (not moved) — window state, display
   settings, recent books, tile cache, book icons. A note line is written to the log.
3. **Secrets:** reading a secret tries service "MoneyKeep" first, then "Money·Craft";
   a secret found under the old service is re-saved under the new one and the old
   entry deleted. Encrypted-book secrets are unaffected.
4. **Definition files:** `.mc·def` files are still accepted (dialogs, drag and drop);
   exports use `.mkdef`. The file format header (`format`) is unchanged.
5. **Environment variables:** `MONEY·CRAFT_*` are still read when the `MONEYKEEP_*`
   one is not set (with a deprecation note in the log).
6. **Capture server:** **Update server** (mobile-capture revision 4) also migrates an
   existing installation: stops and removes the `Money·CraftCapture` service, installs
   `MoneyKeepCapture` from the new install folder with the same data folder and
   arguments, starts it, checks health, and deletes the old install folder. The data
   folder (inbox, key, `server.toml`, desktop token) moves from `…\Money·Craft\…` to
   `…\MoneyKeep\…` in the same step (copy, verify, then delete). Cloudflare settings
   (tunnel, hostname, Access) are unchanged — the owner's hostname stays as chosen.
   The status light shows "The capture server uses the old name — press Update
   server" until migrated.
7. **Phone app:** the new IndexedDB name reads the old database on first start and
   copies the pairing and outbox, so the phone stays paired; the service worker
   deletes the old caches.
8. **MCP / control channel clients** configured with the old program name keep working
   only if the user updates their configuration — documented in README.

## References to remove

- The earlier template project's name: removed everywhere (README, NOTICE, specs,
  comments, docs). Where a spec described following that project's conventions, it
  now describes the convention itself.
- Reference-product names (the two commercial products the project studies) are
  removed from the product and public files: README, NOTICE (its trademark sentence is
  replaced by a generic "MoneyKeep is an independent project and is not affiliated
  with or endorsed by any other software vendor."), the app, code, comments and
  user-facing docs. They remain only in the internal provenance records:
  `docs/clean-room.md`, the forbidden-source rule in `AGENTS.md`/`CLAUDE.md`, the pull
  request checklist, and spec **Sources** tables that cite public format
  documentation by its real title.
- File-format names that are standards or common names (QIF, OFX, QFX, QBO, IIF) stay:
  they name formats, not products. Their user-facing descriptions avoid product names
  (e.g. "QIF files (interchange format)", "IIF files (accounting interchange)").

## History

The owner chose to rewrite the project history so earlier commits carry the new names
too. After the rename lands, the history is rewritten (old names in file contents,
paths and commit messages replaced as above; internal provenance files keep their
reference-product names). This invalidates earlier bundles: the owner makes a fresh
clone in `C:\dev\moneykeep` from a full bundle; the old folder can be deleted after
the capture server has been migrated (step 6).

## Acceptance tests

1. No file outside the internal provenance records contains the old product name, the
   template project's name or the reference-product names (a CI check,
   `cargo xtask names`, with an allow-list of the provenance files).
2. The start screen shows "MoneyKeep" and "Apps for Financial Management".
3. A `.money·craft` book opens; new books are `.moneykeep`; Rename file to .moneykeep
   works and updates the recent list.
4. With only an old config folder present, first start copies it and shows the same
   recent books and window size.
5. A secret stored under the old service is read, moved to the new service and the
   old entry deleted (fake credential store).
6. `.mc·def` files install; exports are `.mkdef`.
7. `MONEY·CRAFT_VIEW` still works when `MONEYKEEP_VIEW` is unset.
8. Update server migrates an old-name capture installation (fake service manager and
   file system): new service, new folders, old removed, health ok; failure restores
   the old service.
9. The phone app copies pairing and outbox from the old IndexedDB name (fake storage).
10. `moneykeep --version` and the MCP server name say MoneyKeep.

## Revision 2 (2026-10-30) — no carry-over of old books (owner decision)

The owner will start over with new books, so compatibility with the previous name is
removed where it concerns books and their settings:

- Steps 1 (old book extension, "Rename file" action), 2 (config copy), 3 (credential
  store fallback), 4 (old definition-file extension and header key, also the old CSV
  profile key) and 5 (old environment variables) are **removed**, with their tests.
  Only `.moneykeep` books and `.mkdef` definition files are recognised.
- Kept: step 6 (migrating the running capture-server service, folders and data, since
  it is installed on the owner's computer) and step 7 (the phone keeps its pairing).
- QDF and QBW stay mentioned only as file formats MoneyKeep does **not** read (they are
  proprietary, encrypted formats; reading them would need reverse engineering, which
  the clean-room protocol forbids).
