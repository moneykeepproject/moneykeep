# Backups

MoneyKeep makes copies of your books by itself, so a deleted file, a failed disk or an
update that goes wrong can't take your records with it. This page explains where the
copies go, when they are made, how many are kept, how to keep a copy somewhere else, and
how to get a book back.

## Where your backups are

By default, backups go in a folder called **MoneyKeep Backups** in your user folder:

    C:\Users\<your name>\MoneyKeep Backups\

Each book gets its own folder inside it, named after the book's file. For example, the
book `Household.moneykeep` is backed up to:

    C:\Users\<your name>\MoneyKeep Backups\Household\

To open the folder, go to **Settings → Backups** and press **Open folder**.

To use a different folder, press **Choose…** next to **Backup folder**. MoneyKeep tries
writing a small test file there first and tells you straight away if it can't.

### Why not the Documents folder?

Windows can protect the Documents, Pictures and Desktop folders from programs it doesn't
know, with a feature called **ransomware protection** (its full name is "Controlled
folder access"). When it is on, Windows quietly stops MoneyKeep from writing there, and
the backup fails with a confusing message. Your user folder is not protected that way,
which is why MoneyKeep uses it.

If you'd rather keep backups in Documents, allow MoneyKeep first:

1. Open **Windows Security** (search for it in the Start menu).
2. Choose **Virus & threat protection**, then **Manage ransomware protection**.
3. Under **Controlled folder access**, choose **Allow an app through Controlled folder
   access**, then **Add an allowed app → Browse all apps**.
4. Add `C:\Program Files\MoneyKeep\moneykeep-app.exe`. If you use them, also add
   `moneykeep.exe` (the command line) and `keep-mcp.exe` (the AI connection) from the same
   folder.
5. Back in MoneyKeep, go to **Settings → Backups → Choose…** and pick the folder in
   Documents.

If a backup ever fails because Windows blocked it, MoneyKeep says so and offers these
steps or **Choose another folder…**.

## When a backup is made

- **When you open a book.**
- **Before MoneyKeep updates a book to a new format** (after you install a new version).
  This one always happens and can't be switched off. If it can't be made, MoneyKeep
  doesn't update or change the book, and tells you why.
- **When you close a book**, if anything changed.
- **Once a day** while a book stays open, if anything changed.
- **Whenever you ask**: **Settings → Backups → Back up now**, or on the start screen, the
  book's **…** menu → **Back up this book…** (this backs up a book without opening it).

Backups happen in the background; you can keep working. The status bar shows "Backing
up…" and then "Backed up" with the time.

The AI connection (keep-mcp), the folder watcher and the command line only make the
backup before a format update, so they don't fill the folder.

## What a backup contains

A backup is a complete copy of the book: every account, transaction, attachment and
setting stored in the book. Each copy is checked after it is written; a copy that fails
the check is deleted and you're told.

Not included: files outside the book (for example the original bank downloads you
imported) and the app's own settings.

**Password-protected books stay protected.** A backup of a protected book has the same
password as the book had when the backup was made. MoneyKeep never writes an unprotected
copy, not even for a moment. Keep that password safe: without it, the backup can't be
opened.

## File names

Each backup's name says which book it is, when it was made and why:

    Household 2026-11-08 1420 opened.moneykeep

The reasons are **opened**, **before-update**, **closed**, **daily** and **manual**.

## How many are kept

So the folder doesn't grow forever, MoneyKeep removes older backups it no longer needs.
For each book it keeps:

- the **10 most recent**;
- the newest backup of each of the last **7 days**;
- the newest backup of each of the last **12 months**;
- the last **10** "before-update" backups and the last **10** "manual" backups.

All of these numbers can be changed in **Settings → Backups → How many to keep**.
MoneyKeep only ever removes backup files it made itself; anything else you put in the
folder is left alone.

## Keep a copy somewhere else (second place)

A backup on the same disk as your books doesn't help if that disk fails. MoneyKeep can
copy every backup to a **second place** too: a USB drive, or a folder that syncs to the
cloud (OneDrive, Dropbox or Google Drive).

To set it up: **Settings → Backups → Second place → Choose…** and pick the folder.

- If the second place isn't available (the USB drive isn't plugged in), MoneyKeep skips
  it quietly and tries again with the next backup.
- If it hasn't been reachable for 7 days, Home shows a reminder.
- Until you set up a second place, MoneyKeep suggests it every 10th backup. You can
  answer **Not now** or **Don't ask again**, and change how often it asks in Settings →
  Backups.

## Getting a book back (restore)

1. On the start screen, press **Restore a backup…** (next to **Open a book…**). Or, with a
   book open, go to **Settings → Backups → Restore a backup…**.
2. Pick the book, then the backup you want. The list shows each backup's date and time,
   reason, size and where it is (backup folder or second place), newest first. You can
   also browse to any backup file.
3. Press **Open as a copy** and choose where to save it.

MoneyKeep never overwrites your book when restoring. It saves the backup as a new file,
for example `Household restored 2026-11-08 1420.moneykeep`, and opens it. A note at the
top of its Home says it is a restored copy and that your original book wasn't changed.
Look through it; if it's the one you want, use it from now on and keep or delete the old
file as you like.

A protected backup asks for its password (the password the book had when the backup was
made). A backup made by a newer version of MoneyKeep can't be opened until you update.

## Reminders on Home

Home → Attention tells you when:

- a book has no backup yet;
- the newest backup is more than **14 days** old (you can change the number);
- the last backup failed, with the reason;
- the backup folder can't be written to;
- the second place hasn't been reachable for 7 days;
- it's time to suggest a second place.

## Copying books yourself

You can still copy a book file yourself (for example to take it to another computer).
Close MoneyKeep first, then copy the `.moneykeep` file. Don't copy the files ending in
`-wal` or `-shm` on their own; MoneyKeep tidies those up when the book is closed.
