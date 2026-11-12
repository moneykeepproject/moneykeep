# AGENTS.md — read this first

This file is for every contributor, human or AI agent. Read it completely before
writing code, docs or tests.

## 1. Clean-room protocol (mandatory, applies to every change)

MoneyKeep is a clean-room implementation of personal and small-business accounting
software. The full protocol is in [`docs/clean-room.md`](docs/clean-room.md). The short
version, which you must follow:

- **Know your role for the task.** You are either a **spec writer** (may study target
  products and prior art through use and public docs; writes only `docs/specs/`) or an
  **implementer** (writes code from `docs/specs/` and public standards only). Never both
  for the same feature area.
- **Implementers never read** source code of commercial finance products, any
  proprietary product, or any project listed under "Prior art" in `docs/clean-room.md`,
  whatever its licence (GPL, AGPL, LGPL, BSL, source-available, MIT or BSD).
- **Never** decompile, disassemble or reverse-engineer target binaries or their
  proprietary file formats.
- **Never** copy trademarks of commercial finance products, or their icons, artwork,
  report layouts, help text or UI wording.
- **Never** scrape or automate bank websites (browser automation is out of scope:
  institutions' terms of use prohibit it).
- If you are an AI agent and you are given, or come across, forbidden material: stop,
  do not use it, and say so in your output.
- Every pull request completes the provenance checklist in
  `.github/pull_request_template.md`.

## 2. Architecture rules

- **One command registry.** Every user-visible action is a command in `mk-commands`.
  GUI, CLI, JSON control channel and MCP server all call the same commands. Do not add
  behaviour that only one front end can reach.
- **Layering is enforced** by `cargo xtask layering` (see `xtask/src/layering.rs`):
  `mk-core` depends on no other workspace crate and does no IO; storage, commands and
  apps sit above it. Front ends never talk to SQLite directly.
- **Money is exact.** Use `mk_core::Money` (backed by `rust_decimal`). Floating-point
  arithmetic is denied by clippy (`float_arithmetic`).
- **No panics in shipped code.** `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`
  are denied. Return `Result`. Tests may opt out locally with
  `#[allow(clippy::unwrap_used)]`.
- **History is append-only.** Every state-changing command writes the audit log. Undo
  is a compensating command, never a silent rewrite.
- **Modules gate features.** Optional business features belong to a module that can be
  toggled per book (see `docs/specs/` once written). Turning a module off hides, never
  deletes.

## 3. Workflow

```sh
cargo run -p moneykeep-cli -- --help   # the CLI
cargo test --workspace                  # tests
cargo xtask ci                          # names, fmt, clippy, tests, layering, licence policy
```

Run `cargo xtask ci` before every commit. It must pass.

## 4. Where things are

| Path | What |
|---|---|
| `apps/moneykeep-cli` | CLI front end |
| `apps/keep-mcp` | Optional MCP server program (`keep-mcp`), separate install; per-book AI access rules in `mcp-server.md` |
| `crates/mk-core` | Money, ids, dates, ledger types, errors. Pure data, no IO |
| `crates/mk-ledger` | Ledger rules: validation and balance arithmetic. Pure logic, no IO |
| `crates/mk-control` | Local control channel (loopback JSON, discovery file, approval) — app side and the keep-mcp client |
| `crates/mk-mcp` | MCP server library over stdio (registry commands as tools); used only by `apps/keep-mcp` |
| `crates/mk-watch` | Watched-folder importer shared by the CLI and the app (file IO, no UI) |
| `crates/mk-ui` | The toolkit-free UI controller: application state, view models (plain Rust), formatting and registry command calls; no Slint, no book access, no IO |
| `crates/mk-views` | The views: `.slint` files (Fluent style), MoneyKeep's own SVG icons and the glue that shows `mk-ui`'s view models in Slint windows (desktop and phone); test harness on Slint's testing backend; no business logic |
| `apps/moneykeep-app` | Desktop app: creates the window from `mk-views`; file dialogs, drag and drop, config, worker thread |
| `crates/mk-store` | SQLite schema, migrations, audit log |
| `crates/mk-dpapi` | The capture server's inbox key sealed with Windows DPAPI: the workspace's only `unsafe` code (two calls into `crypt32.dll`), lints `deny` instead of `forbid` for that one module |
| `crates/mk-commands` | Command registry and built-in commands |
| `xtask` | CI tasks and the layering check; `cargo xtask workflows` also enforces the Mac runner rules (only `release.yml` and `mac-runner-check.yml` may use the self-hosted Mac) |
| `packaging/macos` | The macOS app bundle and disk image (`package.sh`, run by `cargo xtask installer` on a Mac; `--dry-run` anywhere), `docs/specs/macos-release.md` |
| `.github/workflows/release.yml` | Releases: a pushed `v<version>` tag or a hand run; the Mac job runs on the owner's Mac after approval (`docs/deploy/mac-runner.md`) |
| `docs/clean-room.md` | The protocol, role roster, contamination log |
| `docs/specs/` | Behaviour specs (the only input for implementers) |
| `docs/parity-checklist.md` | Feature parity tracking |
| `ROADMAP.md` | Phases |
