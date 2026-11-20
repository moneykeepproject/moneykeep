# Spec: Crates other Keep apps copy

- **Area / module:** workspace structure (`mk-core`, `mk-ledger`, `mk-store`, `mk-import`, `mk-capture-api`, `mk-commands`, `mk-connect`); no user-visible behaviour
- **Spec writer:** Claude session 2026-11-19 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-11-19
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Spec | `docs/specs/moneykeep-ez.md` in the `moneykeep-ez` repository | MoneyKeep EZ keeps a real MoneyKeep book and copies the ledger, book file, command registry, import pipeline and bank feeds unchanged |
| 2 | Report | MoneyKeep EZ implementer, 2026-11-19 | `mk-commands` depends on `mk-capture-api`, whose build script expects the capture apps; `mk-connect` has a test-only dependency on `apps/moneykeep-server`, so the crates cannot be loaded as a workspace without it |
| 3 | Spec | `release-packaging.md` | The capture version worked out by `mk-capture-api`'s build script |
| 4 | Owner rule | The Keep apps copy from each other rather than depend on each other | |

No target product was observed.

## Purpose

Another Keep app must be able to copy MoneyKeep's ledger crates as they are, build them
and run their tests, with none of MoneyKeep's apps beside them. Today two things stop
that. This spec removes them and adds a check so it stays true.

## Behaviour

### The set

The **copy set** is seven crates: `mk-core`, `mk-ledger`, `mk-store`, `mk-import`,
`mk-capture-api`, `mk-commands`, `mk-connect`.

Rules for every crate in the set:

1. Its dependencies and its test-only dependencies inside this workspace are crates in
   the set. Never an app under `apps/`, and never a crate outside the set.
2. It builds, and its own tests pass, in a workspace that holds only the set.
3. Building it there prints no warning that comes from MoneyKeep's own build scripts.

### Changes this needs

- **`mk-connect`'s phone-capture tests** that start the real capture server move out of
  `mk-connect`, to `apps/moneykeep-server`'s tests or a new test-only crate outside the
  set. The same cases are still tested; none is dropped. `mk-connect` then has no
  dependency of any kind on `moneykeep-server`.
- **`mk-capture-api`'s build script.** When the capture server and the phone app are not
  in the workspace it is built in, the script uses the value it already uses when the
  capture version cannot be worked out, and prints nothing. Inside MoneyKeep's own
  workspace nothing changes: the version is worked out as `release-packaging.md` says,
  and a failure there still warns.
- **`mk-capture-api`'s test of the capture version** checks MoneyKeep's paths only when
  those apps are in the workspace. Without them it passes without checking them, and
  says so in its output.
- No public item of any crate in the set changes. No behaviour of MoneyKeep, its capture
  server or its phone app changes.

### The check

`cargo xtask standalone` (new, part of `cargo xtask ci`):

1. Copies the seven crates to a temporary folder.
2. Writes a workspace `Cargo.toml` there with MoneyKeep's `[workspace.package]`,
   `[workspace.dependencies]` (only the entries the set uses) and `[workspace.lints]`.
3. Runs `cargo build --workspace` and `cargo test --workspace` in it and fails if either
   fails, or if the build output holds a `warning:` line from a build script.
4. The layering check also refuses a dependency from a crate in the set on a crate
   outside it or on an app, with the names of both.

Where the network or the toolchain make the temporary build impossible, the task says it
was skipped and why; it never reports a pass it did not run.

### Releases

A release's notes name the copy set and say that it builds on its own from that release.
Another app copies from a released version only, never from an unreleased checkout.

## Ledger effect

Non-posting. No book is read or changed.

## Settings

None.

## Edge cases

- **A new crate becomes a dependency of a crate in the set.** The layering check fails
  until the new crate is added to the set here, in this spec, or the dependency is
  removed.
- **A crate in the set needs test data from an app.** The test moves out of the set.
- **The temporary folder cannot be removed** (a file is held open on Windows): the task
  reports the folder and still gives the build's result.

## Acceptance tests

1. `cargo xtask standalone` passes on a clean checkout.
2. With a test-only dependency on `moneykeep-server` put back in `mk-connect`, the
   layering check fails and names `mk-connect` and `moneykeep-server`.
3. In the temporary workspace, building `mk-capture-api` prints no warning and its tests
   pass; in MoneyKeep's own workspace the capture version is the same as before this
   change for the same sources.
4. Every phone-capture case that was tested in `mk-connect` before is still tested, in
   its new place (the implementer's report lists each moved test and where it went).
5. `cargo xtask ci` passes, and the public items of the seven crates are unchanged.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
