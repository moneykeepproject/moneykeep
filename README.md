# MoneyKeep

**Apps for Financial Management**

Personal and small-business accounting in pure Rust: registers, budgets and
investments for households, and invoices, bills, AR/AP and sales tax for small
businesses, on one double-entry ledger.

**Status:** early software, first public release. Back up anything you try it on, and
see the known limits below.

Version 0.1.0: [release notes](docs/release-notes/0.1.0.md) · [known limits](docs/known-limits.md).

MoneyKeep is an independent project and is not affiliated with or endorsed by any
other software vendor. See [NOTICE](NOTICE).

## Install

On Windows, install MoneyKeep with MoneyKeep Launcher, or run
`moneykeep-<version>-windows-x64-setup.exe` from the release yourself. It installs MoneyKeep for all users
in `C:\Program Files\MoneyKeep` (Windows asks for administrator permission), with a
Start menu shortcut and, if you choose, a desktop shortcut. The setup includes the
phone capture server built together with the app, so there is nothing to build. Run a
newer setup to update in place; your books and settings are kept. Uninstall from
Settings > Apps > Installed apps; your books are never touched. The setup is not
signed yet, so Windows SmartScreen may warn: choose More info > Run anyway.

Phone capture is described in [docs/deploy/phone-capture.md](docs/deploy/phone-capture.md).

## Build

```sh
cargo run -p moneykeep-cli -- --help
cargo run -p moneykeep-cli -- init my.moneykeep
cargo run -p moneykeep-cli -- commands
cargo xtask ci
```

## Run the desktop app

```sh
cargo run --release -p moneykeep-app
```

## Release build and the Windows setup

```sh
cargo xtask release            # dist/MoneyKeep-<version>/ (all programs, licences, version.json)
cargo xtask release --dry-run  # list the steps and files only
cargo xtask installer          # Windows: dist/moneykeep-<version>-windows-x64-setup.exe and dist/SHA256SUMS.txt
```

`release` builds the phone app (wasm, with Trunk; both installed when missing), the
capture server with the phone app inside, the desktop app, the CLI and `keep-mcp`,
all in release mode from the same checkout, so the server beside the app is always
the right one. `installer` needs NSIS 3 (`winget install NSIS.NSIS`, or
https://nsis.sourceforge.io/Download); the script is `installer/moneykeep.nsi`. The
setup is unsigned unless `MK_SIGN_THUMBPRINT` (the thumbprint of a code-signing
certificate installed in your certificate store, e.g. with `certutil -user
-importpfx`) or `MK_SIGN_CERT` (a `.pfx` file without a password, kept where only you
can read it) is set, in which case the programs and the setup are signed with
`signtool` and a timestamp (`MK_SIGN_TIMESTAMP` overrides the timestamp server). A
password is never passed on a command line, so `MK_SIGN_PASSWORD` is refused.

The start screen creates a new book or opens an existing one. Drop a bank file
(`.ofx`, `.qfx`, `.qbo`, `.qif`, `.csv`) on the window, or use **Import…**, to review
and add transactions. Windows, macOS and Linux are supported; on Linux the app needs
a desktop session (X11 or Wayland) with OpenGL, and file dialogs use the XDG desktop
portal. Per-user settings (recent books, theme, density) are stored in the platform's
config directory, never inside a book.

## Import and export

Bank and broker files (`.ofx`, `.qfx`, `.qbo`, `.qif`, `.csv`, `.json`, `.xml`) are
always staged for review first; nothing reaches the books until you choose **Add to
books**, and every import can be undone. Imported text is cleaned on the way in:
control characters, bidirectional overrides and invisible characters are removed and
the row is marked "cleaned".

Definition files (`.mkdef`, TOML) share import layouts, rules, categories and
securities. They contain data only — nothing in them is ever run — and every install
is reviewed, audited and undoable.

Reports export as CSV. A text cell that starts with `=`, `+`, `-`, `@`, a tab or a
carriage return is written with a leading apostrophe (`'`) so spreadsheets show it as
text instead of running it as a formula; numbers and dates are written as they are
(a negative amount stays `-12.50`).

## Coming from the earlier name

The project was renamed MoneyKeep (see `docs/specs/branding.md`). Books, settings,
saved secrets, definition files and environment variables from before the rename are
not carried over: start with new `.moneykeep` books. Two things are moved for you:

- Phone capture: the status light says "The capture server uses the old name — press
  Update server" until **Update server** moves the service, its install folder and
  its data folder to the new names. Cloudflare settings are unchanged.
- The phone app keeps its pairing and anything waiting to be sent.

**MCP clients** configured with the earlier program name, or with `moneykeep mcp
<book>`, must be updated: the MCP server is now the separate, optional program
`keep-mcp` (`keep-mcp <book>` or `keep-mcp --app`; `keep-mcp config --client
claude-desktop <book>` prints the configuration to paste). AI access is off for every
book until you turn it on in Settings → Security (or with `moneykeep ai-access`); see
`docs/specs/mcp-server.md` revision 3.

## Contributing

Humans and AI agents: start with [`AGENTS.md`](AGENTS.md). The clean-room protocol in
[`docs/clean-room.md`](docs/clean-room.md) is mandatory.

## Licence

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise, any
contribution intentionally submitted for inclusion in the work by you, as defined in
the Apache-2.0 license, shall be dual licensed as above, without any additional terms
or conditions.
