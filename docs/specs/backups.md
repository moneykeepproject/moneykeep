# Spec: Automatic backups and restore

- **Area / module:** core (book files), desktop app
- **Spec writer:** spec-writer session (Claude), with the owner
- **Date:** 2026-11-07
- **Status:** reviewed — owner decisions 2026-11-07 (below)

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Standard / vendor doc | SQLite documentation: Online Backup API (sqlite.org/backup.html), `VACUUM INTO` (sqlite.org/lang_vacuum.html), write-ahead logging (sqlite.org/wal.html) | Consistent copies of a database that is in use |
| 2 | Spec | `book-encryption.md`, `app-desktop.md` (revisions 26 and 40, start screen), `dashboards.md` (Attention) | Encrypted books, start screen, Home notices |
| 3 | Owner decisions | Conversation 2026-11-07 | Default folder Documents\MoneyKeep Backups; suggest a second place every 10 backups (setting); rotation as proposed; restore opens a separate copy |

## Purpose

Nobody should lose their books to a deleted file, a failed disk or an upgrade gone
wrong. MoneyKeep makes complete copies by itself, keeps a sensible number of them, nudges
the user to keep a copy somewhere else, and makes getting a book back easy and safe.

## Behaviour

### When a backup is made

1. **On open**, after the book is opened and **before** any format upgrade (migration)
   is applied. When a migration is pending the backup is mandatory: if it fails, the
   book is not upgraded and opening stops with "MoneyKeep couldn't make a backup before
   updating this book, so it wasn't changed: <reason>." (Details, **Try again**,
   **Choose backup folder…**).
2. **On close** (closing the book, opening another one or quitting), if anything changed
   since the last backup.
3. **Daily** while a book stays open: when 24 hours have passed since the last backup and
   something changed.
4. **Back up now** (Settings → Backups) and **Back up this book…** (the start-screen
   card's **…** menu, revision 2): always, labelled "manual".
- Reasons are recorded: `opened`, `before-update`, `closed`, `daily`, `manual`.
- A backup never blocks typing: it runs in the background; the status bar shows "Backing
  up…" then "Backed up 2:20 PM" (auto-closing like other status messages). Failures other
  than the before-update case are reported on Home → Attention ("Last backup failed:
  <reason>") without stopping work.

### The copy

- A complete, consistent copy taken with the database's online backup method (source 1),
  safe while the book is in use, then checked (`PRAGMA integrity_check` on the copy, or
  `quick_check` for books over 200 MB). A copy that fails the check is deleted and
  reported.
- **Encrypted books stay encrypted:** the backup has the same encryption and password as
  the book (`book-encryption.md`); it is never written unencrypted, even temporarily.
- File name: `<book file name> <YYYY-MM-DD HHMM> <reason>.moneykeep` (e.g.
  `Household 2026-11-07 1420 opened.moneykeep`), written first under a temporary name and
  renamed when complete, so a half-written backup never looks like a good one.
- Each book's backups go in a subfolder named after the book file
  (`<backup folder>\Household\`).

### Where

- **Backup folder** (setting): default `Documents\MoneyKeep Backups` (owner decision;
  changed in revision 3 to `<user folder>\MoneyKeep Backups`).
  If it cannot be written, Attention says so with **Choose backup folder…**.
- **Second place** (optional setting): another folder, typically a USB drive or a cloud
  folder (OneDrive, Dropbox, Google Drive). After each backup in the backup folder, the
  same file is copied there. When it is not reachable (drive unplugged), the copy is
  skipped quietly and tried with the next backup; after 7 days without a successful copy
  there, Attention says "Your second backup place (E:\Backups) hasn't been reachable since
  Nov 1."
- **Suggesting a second place** (owner decision): while no second place is set, every
  **10th** backup (setting `suggest_second_every`, default 10; 0 = never) shows a notice
  on Home → Attention: "Keep a copy of your backups somewhere else — a USB drive or a
  cloud folder — so one failed disk can't take your books and their backups together."
  with **Choose a second place…**, **Not now** (asks again after the next 10) and
  **Don't ask again** (sets the setting to 0; undoable in Settings).

### How many are kept (rotation)

Per book, in each place, after every backup:
- keep the **10 most recent**;
- plus the newest backup of each of the last **7 days**;
- plus the newest of each of the last **12 months**;
- `before-update` and `manual` backups are kept until there are more than 10 of each,
  then the oldest of that reason is removed;
- a backup is removed only when no rule keeps it. All the numbers are settings.
- Only files MoneyKeep made (matching the name pattern in that book's subfolder) are ever
  removed.

### Restore

- **Settings → Backups → Restore a backup…** and, on the start screen toolbar, **Restore
  a backup…** (next to Open a book…).
- A list of backups for a book (pick the book first when started from the start screen,
  or browse to any backup file): date and time, reason, size, place (backup folder or
  second place), with the newest first.
- **Open as a copy** (owner decision: restore never replaces the book): asks where to
  save (default: the book's folder, name `<book> restored <YYYY-MM-DD HHMM>.moneykeep`),
  copies the backup there, checks it, and opens the copy. A note at the top of the
  restored book's Home: "This is a copy restored from the backup of Nov 7, 2026 2:20 PM.
  Your original book was not changed." (dismissible).
- An encrypted backup asks for its password (the password when the backup was made).
- Restoring a backup made by a newer MoneyKeep than this one is refused with the version
  needed.

### Settings → Backups (new section, app-wide)

- Backup folder (Choose…, Open folder), Second place (Choose…, Clear, Open folder), last
  backup time and place for the open book, **Back up now**, **Restore a backup…**,
  rotation numbers, `suggest_second_every`, and "Back up when opening / closing / daily"
  switches (on by default; the before-update backup cannot be switched off).
- Help text: what is backed up (the whole book, including attachments stored in it), what
  is not (files outside the book, app settings), and that backups of a password-protected
  book need that book's password.

### Attention items

- "No backup for 14 days" (setting `warn_days`, default 14) when the open book's newest
  backup is older.
- Last backup failed; backup folder not writable; second place not reachable for 7 days;
  the suggestion above.

### Command line and other front ends

`book.backup {reason?}` → `{path, size, second_place_path?}`; `book.backups {book?}` →
list; `book.restore_copy {backup, target}` → `{path}`. The CLI gains `moneykeep backup
<book>` and `moneykeep backups <book>`.

## Ledger effect

None. Backups are files; making one adds no audit entry (restoring opens a different
file).

## Settings

App settings (not stored in books): `backup.folder`, `backup.second_place`,
`backup.on_open` (true), `backup.on_close` (true), `backup.daily` (true),
`backup.keep_recent` (10), `backup.keep_days` (7), `backup.keep_months` (12),
`backup.keep_per_reason` (10), `backup.suggest_second_every` (10), `backup.warn_days` (14).

## Acceptance tests

BK1. Opening a book makes `… opened.moneykeep` in `<folder>\<book>\`; it opens and has
     the same transactions as the book.
BK2. A book needing a migration: a `before-update` backup exists before the migration
     runs; with the folder made unwritable, opening stops with the message and the book
     file is byte-for-byte unchanged.
BK3. Closing after a change makes a `closed` backup; closing without a change makes none.
BK4. An encrypted book's backup cannot be opened without the password and opens with it;
     no unencrypted copy appears on disk at any point (test watches the folder).
BK5. Rotation: 40 backups over 400 simulated days leave the 10 newest, one per day for 7
     days, one per month for 12 months, and at most 10 `manual`; a file not matching the
     pattern in the folder is never removed.
BK6. Second place: copies appear there; with it missing, backups still succeed; after 7
     days the Attention item appears.
BK7. With no second place, the 10th backup shows the suggestion; Not now hides it until
     the 20th; Don't ask again sets the setting to 0.
BK8. Restore → Open as a copy creates `<book> restored <date>.moneykeep`, opens it with
     the note, and the original book file is unchanged.
BK9. A backup failing its integrity check (simulated) is deleted and reported.
BK10. "No backup for 14 days" appears when the newest backup is 15 days old.
BK11. Screenshots light and dark: `settings-backups`, `restore-list`, `attention-second-place`.

## Clean-room check

Written from SQLite's documentation, MoneyKeep's own specs and the owner's decisions. No
other product's screens, wording or code were consulted.

## Revision 2 (2026-11-07) — every way of opening a book is covered (owner request)

- **Every program that opens a book file** makes the mandatory before-update backup when
  a format update is pending, with the same rules as the desktop app (backup folder and
  second place from the app settings; the book is not changed if the backup fails):
  keep-mcp, the folder watcher (`mk-watch`), the CLI (already done), and any other path
  that opens a book through `mk-store` (for example `Session::open` in `mk-commands`).
  The safest way is one opening function in `mk-store`/`mk-commands` that all of them use;
  the plain `Store::open` stays only for tests and for opening backup copies.
  - keep-mcp: when the backup fails, the tool call that opened the book returns the error
    ("MoneyKeep couldn't make a backup before updating this book, so it wasn't changed:
    <reason>") and the book is left closed.
  - The watcher: the file is skipped for this run with the same message in its log and
    status; it is tried again on the next run.
  - Only the before-update backup is made by these programs (no opened/closed/daily
    backups), so they don't fill the folder.
- **Back up now** is removed from the start-screen card's **…** menu wording (no book is
  open there). It stays in Settings → Backups. The card's **…** menu instead gains **Back
  up this book…**, which makes a `manual` backup of that book without opening it in the
  app (asking for the password first for a protected book), then reports "Backed up
  Household 2:20 PM".
- Tests:
  BK12. A book needing a format update opened through keep-mcp gets a `before-update`
        backup; with the backup folder unwritable, the MCP call returns the message and
        the file is byte-for-byte unchanged.
  BK13. Same for the watcher: backup made; on failure the file is skipped, unchanged,
        and the message is in the watcher's status.
  BK14. A guard test fails if any non-test code outside `mk-store` calls the plain
        `Store::open` for a book.
  BK15. Start screen card … → Back up this book… makes a `manual` backup of a closed
        book; for a protected book it asks for the password first.

## Revision 3 (2026-11-08) — default folder in the user folder; Windows blocking explained (owner report)

Report: on the owner's PC, backups to `Documents\MoneyKeep Backups` failed with "os error
2". Windows' ransomware protection (Controlled folder access) blocks unknown programs
from writing to Documents and reports it misleadingly. Backups to
`C:\Users\<name>\MoneyKeep Backups` worked (owner decision: make that the default).

Sources: Microsoft documentation "Protect important folders with controlled folder
access" (learn.microsoft.com/defender-endpoint/controlled-folders) — protected folders
include Documents, Pictures, Videos, Music, Favorites and Desktop; blocked apps can be
allowed in Windows Security.

- **Default backup folder:** `<user folder>\MoneyKeep Backups` (Windows
  `C:\Users\<name>\MoneyKeep Backups`; macOS/Linux `~/MoneyKeep Backups`). A folder
  already chosen by the user is kept. A setting still holding the old default
  (`Documents\MoneyKeep Backups`) is moved to the new default only if no backup was ever
  written there successfully; otherwise it is kept.
- **Choosing a folder tests it:** after **Choose…** (backup folder or second place),
  MoneyKeep writes and deletes a small test file there. If that fails, the folder is not
  saved and the message explains why (below).
- **Explaining a blocked folder:** when creating the folder or the backup file fails
  with "access denied" or "not found" in a folder inside Documents, Pictures, Videos,
  Music, Favorites or Desktop (Windows), the failure reads: "Windows blocked MoneyKeep
  from writing to <folder>. This is usually Windows' ransomware protection (Controlled
  folder access)." with **Choose another folder…** and **How to allow MoneyKeep** (opens
  the Help section). Other failures keep their plain-words message with Details.
- **Help:** `docs/help/backups.md` (supplied by the spec writer) is the user guide; Settings
  → Backups gains a **Help** button that shows it, as the phone capture Help shows its
  guide; the failure message's **How to allow MoneyKeep** opens it at "Why not the
  Documents folder?".
- Tests:
  BK16. The default folder is `<home>\MoneyKeep Backups`; a setting with the old default
        and no backup there moves to the new default; with backups there it is kept.
  BK17. Choosing a folder that can't be written (test hook) is refused with the message
        and the setting is unchanged; a writable one is saved and the test file is gone.
  BK18. A "not found"/"access denied" failure under a Documents path (test hook) shows the
        Controlled folder access message with both buttons; under another path the
        plain message.
  BK19. Settings → Backups → Help shows the guide.

## Revision 4 (2026-11-08) — second place and restore copies on Windows (owner report)

Report: with a second place set, MoneyKeep created the book's folder there but no backup
file appeared (tried twice).

Cause: after copying, the copy is flushed to disk through a handle opened **read-only**
(`File::open(..)?.sync_all()`). On Windows, flushing (`FlushFileBuffers`) needs write
access, so it fails; the temporary file is then deleted and the error is only kept in
the result, not shown. The restore copy (`Open as a copy`) uses the same code and fails
the same way on Windows. (On Linux the tests passed, so it went unnoticed.)

- Copies made by MoneyKeep (second place, restore) are flushed through a handle opened
  for writing (or the copy is written through a handle MoneyKeep opened for writing and
  flushed before closing).
- A second-place copy that fails for any reason other than "the folder isn't there"
  (unplugged drive) is shown: status bar "Backed up. The copy to the second place
  failed: <reason>" and a Home → Attention item with the reason and **Choose…**; the
  7-day reminder stays for the unreachable case.
- A guard test fails if `sync_all` is called on a handle from `File::open` anywhere in
  the workspace.
- Tests: BK20. A second-place copy and a restore copy succeed on all platforms (the
  copy is flushed through a writable handle; unit test of the helper). BK21. A failing
  second-place copy (test hook) shows the status message and the Attention item.
