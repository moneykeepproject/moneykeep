# Spec: Encryption at rest (password-protected books)

- **Area / module:** storage (`core`)
- **Spec writer:** Claude session 2026-10-19 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-19
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Standard | RFC 8439, *ChaCha20 and Poly1305 for IETF Protocols* | AEAD construction |
| 2 | Standard | IRTF CFRG draft *XChaCha: eXtended-nonce ChaCha and AEAD_XChaCha20_Poly1305* | 192-bit nonces, safe to choose at random |
| 3 | Standard | RFC 9106, *Argon2 Memory-Hard Function for Password Hashing* | Argon2id key derivation and parameter guidance |
| 4 | Guidance | OWASP Password Storage Cheat Sheet | Argon2id minimums |
| 5 | Vendor doc | SQLite C interface, `sqlite3_serialize` / `sqlite3_deserialize` — https://www.sqlite.org/c3ref/deserialize.html | Loading and saving a whole database image in memory |
| 6 | Vendor doc | `libsqlite3-sys` crate documentation — https://docs.rs/crate/libsqlite3-sys/latest | SQLCipher features need a system or vendored OpenSSL |
| 7 | Standard | Unicode Standard Annex #15 (normalisation forms) | NFC before key derivation |
| 8 | Design | MoneyKeep plan document, Phase 2 "encryption at rest"; owner request 2026-10-19 | |

No target product was observed. File layout, names and wording are MoneyKeep's own.

## Purpose

A book holds a household's complete financial history. If a laptop is lost or a file
is copied, a password-protected book must be unreadable without the password, while
everyday use stays as simple as an ordinary book.

## Design decision

MoneyKeep encrypts the **whole book file** with pure-Rust cryptography, holding the
open book in memory, instead of using SQLCipher. Reason: SQLCipher in Rust needs a
system OpenSSL or a vendored OpenSSL build, which on Windows requires extra build
tools (source 6); the owner's machine showed that build friction matters. Household
books are small (typically under 20 MB), so rewriting the encrypted file after each
change is affordable. Crates used must be MIT/Apache (RustCrypto `chacha20poly1305`,
`argon2`, `zeroize`, an OS random source) and pass `cargo deny` without new
exceptions.

Unencrypted books are unchanged: plain SQLite files, opened as today.

## Behaviour

### Encrypted book file

One file, same `.moneykeep` extension. Layout, in order (all integers little-endian):

| Part | Content |
|---|---|
| Magic | 8 bytes `MCBOOKE1` — never a valid SQLite header, so the two formats cannot be confused |
| Format version | u16, = 1 |
| KDF block | algorithm id (1 = Argon2id v1.3), memory KiB (u32), iterations (u32), parallelism (u32), salt (16 random bytes) |
| Key slots | u8 count (1 or 2), then per slot: kind (1 = password, 2 = recovery key), nonce (24 bytes), wrapped data key (32 bytes + 16-byte tag) |
| Payload nonce | 24 bytes, fresh random value on every save |
| Payload | XChaCha20-Poly1305 encryption of the SQLite database image |

- A random 256-bit **data key** encrypts the payload. The data key never changes
  while the book stays encrypted (changing the password only re-wraps it).
- The **password key** is Argon2id(password in Unicode NFC, salt, parameters), 32
  bytes. Default parameters: memory 65,536 KiB (64 MiB), 3 iterations, parallelism 1
  (memory and iterations as in source 3's second recommended option, with
  parallelism 1 so opening costs the same on any machine; above the OWASP minimum). Parameters are read from
  the file, so later versions may strengthen defaults for new files.
- The **recovery key** is 32 random bytes; its slot wraps the data key with the
  recovery key directly (no KDF — it is already full-entropy).
- Each slot is XChaCha20-Poly1305(wrapping key, slot nonce, data key). Associated
  data: for the password slot, magic ‖ version ‖ KDF block ‖ slot kind; for the
  recovery slot, magic ‖ version ‖ slot kind (it does not depend on the KDF block, so
  changing the password — which changes the salt — leaves the recovery slot valid
  without needing the recovery key). The payload's associated data still covers the
  whole header, so any header change is detected.
- The payload's associated data is every byte before the payload (magic through
  payload nonce), so any change to the header is detected.

### Opening

- A file starting with `MCBOOKE1` is encrypted; one starting with the SQLite header is
  plain; anything else is `not_a_book`.
- Opening an encrypted book needs a password **or** a recovery key. Errors (new codes):
  - `password_required` — no secret given.
  - `wrong_password` — no slot unwraps with the given secret (the message does not
    say which slot kinds exist).
  - `book_damaged` — a slot unwraps but the payload fails authentication, the file is
    truncated, or the version is unknown. Message: the book file is damaged or was
    changed outside MoneyKeep; mention the previous version file if it exists.
- The decrypted image is loaded into an in-memory SQLite database; migrations then
  run as for a plain book (and the migrated image is saved).
- No plaintext database, journal, WAL or temp file is ever written to disk for an
  encrypted book. Temp storage is set to memory.
- **Exclusive use.** While a process has an encrypted book open it holds an OS
  exclusive lock on the book file; a second open (any process) fails with
  `book_in_use`. The lock is released on close and on process exit.

### Saving

- After every **mutating** command succeeds, the full database image is serialised,
  encrypted with a fresh payload nonce, and written atomically **before** the command
  reports success: write `<file>.tmp-<random>` in the same folder, flush and sync it,
  rename the current file to `<file>.prev` (replacing an older one), then rename the
  temp file to `<file>`. On Windows use replace-capable renames. If any step fails, the
  command reports an `io` error, the in-memory book is rolled back to its state before
  the command, and the file on disk is the old version.
- `<file>.prev` is the previous encrypted version (same keys). It lets a user recover
  from a damaged file by renaming it.
- Non-mutating commands never write.
- Performance budget: saving a 10 MB book takes under 300 ms in a release build on a
  laptop (performance test, not run in CI debug builds). Opening includes the KDF
  (~0.2–1 s by design).

### Secrets in memory

The password, password key, recovery key and data key are held in buffers that are
zeroed when no longer needed (`zeroize`). Secrets are never logged, never written to
the audit log, never put in error messages, never stored in the app config and never
sent over MCP. The decrypted book is in process memory; the OS may page it to disk
(documented, not prevented).

### Operations

These are **session operations**, not registry commands: they act on the file and
its keys, not on ledger data, so they are **not** exposed as MCP tools. They are
available from the CLI and the app. Each writes an audit entry with no secrets
(`book.encrypted`, `book.password_changed`, `book.recovery_key_changed`,
`book.decrypted`) so History shows when it happened.

| Operation | Effect |
|---|---|
| **Create encrypted** | Creating a new book with a password makes it encrypted from the start; never written in plain form |
| **Encrypt** (`password`, `recovery_key`: bool, default true) | Converts the open plain book in place: writes the encrypted file via the temp-and-rename steps, replacing the plain file, and deletes any `-wal`/`-shm` files. Returns the recovery key (once) when requested |
| **Change password** (`current` password or recovery key, `new`) | Re-wraps the data key with a new salt; payload re-encrypted with a fresh nonce; the recovery slot is kept |
| **New recovery key** (`current` secret) | Replaces (or adds) the recovery slot; returns the new key once; the old recovery key stops working |
| **Remove recovery key** (`current` secret) | Deletes the recovery slot |
| **Save unencrypted copy** (`path`) | Writes a plain SQLite copy to a different path (refuses the book's own path) |
| **Remove encryption** (`current` secret) | Replaces the encrypted file with a plain book in place (after confirmation in front ends) and deletes `<file>.prev` |

Password rules: outer whitespace is trimmed and the result normalised to NFC, both
when setting a password and when opening (so "  secret phrase " opens a book whose
password is "secret phrase"); then at least 8 and at most 1,024 characters. No composition rules. Front ends show a short note
that a long passphrase of several words is best.

Recovery key display form: `MCRK-` followed by the 32 bytes in Crockford base32
(52 characters) in groups of four separated by `-`. Input accepts any case, ignores
spaces and `-`, and maps `O`→`0`, `I`/`L`→`1`. A checksum is not needed: a wrong key
simply fails with `wrong_password`.

Old plaintext data: after Encrypt, the previous plain file's bytes may remain on the
disk until overwritten (SSDs and file systems do not guarantee erasure). Front ends
say so in one line and suggest encrypting before importing real data.

### Command line

- Any CLI command given an encrypted book obtains the secret from, in order: `--password-file <path>` (first line), the `MONEYKEEP_PASSWORD` environment variable (documented as less safe), or an interactive prompt without echo when stdin is a terminal. Otherwise it fails with `password_required`. `--recovery-key` makes the prompt ask for a recovery key instead.
- New subcommands: `encrypt PATH [--no-recovery-key]`, `passwd PATH`,
  `recovery-key PATH [--remove]`, `decrypt PATH --out NEWPATH`,
  `remove-encryption PATH`. Passwords are always asked twice when setting one.
- `mcp` and `watch` obtain the secret once at start the same way.

## Ledger effect

None. The audit entries are non-posting.

## Settings

Per-user app config (not in the book): `lock_after_minutes` (0 = never, default 0;
choices 5, 15, 30, 60).

## Edge cases

- Encrypting a book that is open in another MoneyKeep process: refused
  (`book_in_use` for encrypted books; for a plain book the conversion requires that
  SQLite reports no other connection — it opens the file exclusively first).
- Disk full during save: `io` error, book unchanged on disk and in memory.
- Crash between the two renames: `<file>` is missing and `<file>.prev` holds the last
  good version; opening `<file>` reports it missing and front ends suggest
  `<file>.prev`. (The temp file may also remain and is ignored.)
- A watched-folder import into an encrypted book saves like any command.
- Undo/redo work as normal (they are commands).
- Copying an encrypted book to another machine: opens there with the password.

## Acceptance tests

1. Create an encrypted book with password `correct horse battery`, add an account,
   close; the file starts with `MCBOOKE1`, contains neither the account name nor the
   book name as bytes, and reopening with the password shows the account.
2. Opening without a secret → `password_required`; with `wrong` → `wrong_password`.
3. Flipping one byte in the payload, in the KDF block, or in a slot → `book_damaged`
   or `wrong_password` (slot), never a successful open.
4. Encrypt a plain book with a recovery key: opening with the recovery key (typed in
   lower case with spaces instead of dashes) works; after **New recovery key** the old
   key fails.
5. Change password: the old password fails, the new one works, the recovery key still
   works, and the data key (observable as the payload still decrypting after only the
   slot changed) is unchanged.
6. After a mutating command the file on disk has a new payload nonce and `<file>.prev`
   exists and opens with the same password, showing the state before that command.
7. Simulated write failure (injected) during save: the command fails with `io`, the
   in-memory book does not contain the change, and the file opens to the old state.
8. While one session holds the encrypted book, a second open fails with `book_in_use`.
9. During use of an encrypted book, the book's folder contains no file other than the
   book, `.prev`, and possibly a temp file during the save (no `-wal`, `-shm`,
   `-journal`).
10. **Save unencrypted copy** produces a plain SQLite file with identical
    `report.trial_balance` output; the original stays encrypted.
11. CLI: `MONEYKEEP_PASSWORD=… moneykeep accounts book.moneykeep` lists accounts;
    without it and with no terminal it fails with `password_required`.
12. The MCP tool list for an encrypted book contains no encryption operations.
13. Audit history shows `book.encrypted` and `book.password_changed` entries with no
    secret material in their before/after data.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-10-19) — implementer questions answered

- Recovery-slot associated data excludes the KDF block (see "Encrypted book file").
- `history.undo` / `history.redo` skip `book.*` session audit entries; they are
  records, not undoable changes.
- **Encrypt** replaces the plain file directly (temp file, sync, rename over the
  plain file); it never creates `.prev` (which would be a plaintext copy) and deletes
  `-wal`/`-shm`. **Remove encryption** likewise replaces the file and deletes
  `.prev`. The normal save steps (with `.prev`) apply only to an already-encrypted
  book.
- Exclusive lock: use the `fs4` crate (MIT/Apache) so the workspace MSRV stays 1.85.
  The temp file is locked before it is renamed into place; on Windows files are
  opened with delete sharing so a locked file can be renamed. No separate lock file.
- Rollback after a failed save reloads the in-memory database from the last saved
  ciphertext held in memory.
