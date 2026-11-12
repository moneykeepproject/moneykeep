# Known limits

What MoneyKeep 0.1.0 doesn't do yet, or doesn't do on purpose. This list is updated
with each release. For what it does do, see the [release notes](release-notes/0.1.0.md).

## Early software

- **Back up your books.** MoneyKeep is new and not yet ready for real books: keep a
  copy of anything you try it on. MoneyKeep makes its own backups
  ([Backups](help/backups.md)), but keep a copy somewhere else too.
- **A forgotten password can't be recovered.** Without the password and without the
  recovery key, nobody, MoneyKeep included, can open a password-protected book. Store
  the recovery key somewhere safe when it is shown.
- **Books from the earlier name are not carried over.** Books, settings, saved secrets
  and definition files from before the project was renamed MoneyKeep don't open:
  start with new `.moneykeep` books. Phone capture and the phone app's pairing are
  moved for you.

## Installing

- **The Windows setup is not signed yet.** Windows SmartScreen may warn: choose
  **More info**, then **Run anyway**.
- **The Mac build is not signed with an Apple Developer ID yet** (it is signed "ad
  hoc"). macOS warns the first time you open it: right-click MoneyKeep, then choose
  **Open** (or System Settings > Privacy & Security > **Open Anyway**).
- **MoneyKeep Launcher can't install MoneyKeep on a Mac yet.** It installs only Mac
  builds signed with the project's Developer ID. Until then, install from the
  release's `.dmg`; the launcher opens the copy you installed and links to new
  releases.
- **Release files are for Windows (64-bit) and Mac only.** MoneyKeep also runs on
  Linux, but there is no Linux download yet: build it from the source.
- **On a Mac, the command-line program is not included** in the app. The phone capture
  server and the AI assistant connection are.

## On purpose

- **No logging in to bank websites for you.** MoneyKeep never automates or scrapes a
  bank's website: banks' terms of use forbid it. Transactions come from files you
  download from your bank, or from connection services and banks that offer an
  official way in.
- **No reading of other finance programs' own data files.** Bring data over through
  the files those programs export (`.qif`, `.ofx`, `.csv`, `.iif`).
- **No payroll tax filing.**
- **1099s are tracked, not filed.** MoneyKeep keeps 1099 amounts per vendor; to
  prepare the forms, use Form1099Keep.
- **The AI assistant connection is off** for every book until you turn it on in
  Settings → Security.

## Not yet

- **Other currencies:** accounts and transactions in a currency other than the book's
  need the multi-currency feature, and exchange rates are not in yet.
- **Not in yet:** memorised transactions, loans and amortisation schedules, sales
  receipts, recurring invoices, progress invoicing, sales orders, inventory and time
  tracking.
- **Wash sales** are flagged, with the amounts for your tax forms, but the cost basis
  of the replacement shares is not adjusted automatically.
- **Short sales** of stock and the details of margin interest are not handled.
- **Exporting to `.iif`** leaves out memos, numbers, document details, investment
  details and attachments; the app lists these before saving.
- **Some direct broker connections** are planned for a later version. The brokerage
  connection service works with your own personal key only.
- **Statement files in the international bank formats** (ISO 20022 `camt.053`/`camt.052`,
  MT940/MT942) can't be imported yet.
- **There is no full web version** of MoneyKeep yet; the phone app is for capturing
  expenses and trips only.

## Phone capture

- **Receipts are not read for you.** You type the amount and details; reading text off
  receipt pictures is planned.
- **One person sends expenses.** Several people submitting, and approving each other's
  expenses, are planned.
- **Your computer must be on and awake** for phones to send. Expenses wait on the phone
  until then.
- **The phone sends while the app is open.** Web browsers don't let a closed web app
  send reliably in the background, so open the app on your phone once you are home.
- **On a Mac, phone capture works while you're logged in.** Expenses sent while you're
  logged out wait on your phone.
- **On a Mac, the firewall may ask again after each update** whether `moneykeep-server`
  may accept connections, until the Mac build has a Developer ID: choose **Allow**.
- **Moving MoneyKeep on a Mac** to another folder stops phone capture until you press
  **Install the server again** in Settings > Phone capture.
- **Capture through Cloudflare on a Mac** needs Cloudflare's own `cloudflared`
  package. A copy built some other way (for example with Homebrew) is not signed by
  Cloudflare and is refused.
- **If your computer's address changes,** every phone has to install the new
  certificate again.

See [phone capture](deploy/phone-capture.md) for the details.
