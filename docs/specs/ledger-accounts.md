# Spec: Accounts and chart of accounts

- **Area / module:** ledger core (`core`)
- **Spec writer:** Claude session 2026-10-18 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-18
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Textbook | OpenStax, *Principles of Accounting, Volume 1: Financial Accounting* (open textbook) — https://open.umn.edu/opentextbooks/textbooks/principles-of-accounting-volume-1-financial-accounting | Account types, normal balances, chart of accounts, the accounting equation |
| 2 | Standard | ISO 4217 currency codes — https://www.iso.org/iso-4217-currency-codes.html | Account commodity codes |
| 3 | Design | MoneyKeep plan document, "Core data model" | Table list and invariants |

No target product was observed for this spec. Everything here is general double-entry
bookkeeping practice and MoneyKeep's own design choices.

## Purpose

Users organise money into accounts: things they own (assets), things they owe
(liabilities), the owner's or organisation's stake (equity), money coming in (income)
and money going out (expenses). Accounts nest into a tree so reports can roll up.

## Behaviour

### Account fields

| Field | Rules |
|---|---|
| `id` | Typed id, generated, immutable |
| `name` | 1–100 characters after trimming; must not contain `:` (the path separator); unique among siblings (case-insensitive) |
| `type` | One of `asset`, `liability`, `equity`, `income`, `expense`; immutable after the account has any split |
| `subtype` | See table below; must belong to `type`; may change while the account has no splits |
| `parent` | Optional; parent must have the **same type**; no cycles |
| `commodity` | Commodity code the account is denominated in; defaults to the book's base currency; immutable after the account has any split |
| `code` | Optional account number, 1–20 characters, unique in the book |
| `description` | Optional free text, up to 1000 characters |
| `placeholder` | Boolean, default false. A placeholder account groups children and may not hold splits |
| `archived` | Boolean, default false. See "Archiving" |
| `created_at` | Set on creation |

### Subtypes

| Type | Subtypes |
|---|---|
| asset | `bank`, `cash`, `brokerage`, `security`, `receivable`, `fixed-asset`, `clearing`, `other-asset` |
| liability | `credit-card`, `loan`, `payable`, `tax-payable`, `other-liability` |
| equity | `opening-balances`, `owner-equity`, `retained-earnings`, `net-assets` |
| income | `income` |
| expense | `expense` |

`security` accounts hold a security commodity (e.g. `VTI`), not a currency; their
commodity must differ from the book's base currency. All other subtypes default to a
currency. Using a currency other than the base currency requires the `multi-currency`
module.

### Full name

An account's full name is its ancestors' names and its own joined with `:`, e.g.
`Expenses:Utilities:Electric`. Full names are unique because sibling names are.
Commands that accept an account may take its `id`, its full name, or its `code`.

### Sign convention

Internally every amount is stored as a signed number where **debit is positive and
credit is negative**. Each type has a normal balance used for display:

| Type | Normal balance | Displayed balance |
|---|---|---|
| asset, expense | debit | stored sum as-is |
| liability, equity, income | credit | stored sum negated |

So a credit card with $200 owed is stored as −200 and shown as 200 owed; a salary
income account that received $3,000 is stored as −3,000 and shown as 3,000 earned.

### Default accounts

Creating a book creates exactly one account: a top-level equity account named
`Opening Balances`, subtype `opening-balances`. It is the default other side for
opening-balance entries. It can be renamed, but not deleted, and not archived while it
holds splits.

Starter charts (personal, sole proprietor, nonprofit, …) are a later feature and are
out of scope here.

### Archiving

- An account can be archived only when its balance is zero in every commodity and it
  has no unarchived children.
- Archived accounts reject new splits and edits that would add or change their splits.
  Existing splits stay and still count in historical reports.
- Archived accounts are hidden from default lists; `include_archived` shows them.
- Unarchiving is always allowed.

### Deleting

An account can be deleted only if it has never had a split and has no children.
Otherwise the command is rejected and the user is told to archive it instead.

### Moving

Changing `parent` is allowed at any time, provided the new parent has the same type,
is not a descendant of the account, and sibling-name uniqueness still holds.

## Ledger effect

Account commands are non-posting: they never create transactions.

## Commands

| Command | Mutates | Notes |
|---|---|---|
| `account.create` | yes | fields above; returns the account |
| `account.update` | yes | name, code, description, subtype, placeholder (placeholder may become true only if the account has no splits) |
| `account.move` | yes | new parent or none |
| `account.archive` / `account.unarchive` | yes | |
| `account.delete` | yes | |
| `account.get` | no | by id, full name or code |
| `account.list` | no | tree order; filters: `type`, `include_archived` |

## Settings

None in this spec.

## Edge cases

- Name with leading/trailing spaces is trimmed; an all-whitespace name is invalid.
- Two siblings `Food` and `food` conflict.
- Moving an account under its own child is rejected.
- Changing `type` of an account with children is rejected (children must match).
- A placeholder account may become a non-placeholder at any time; it then accepts
  splits. The reverse requires that it has no splits.

## Acceptance tests

1. Create `Assets` (asset, placeholder) and `Checking` (asset/bank, parent Assets);
   full name is `Assets:Checking`.
2. Creating `Expenses:Food` under an asset parent is rejected (type mismatch).
3. Creating a second `checking` under `Assets` is rejected (case-insensitive clash).
4. Posting to `Assets` (placeholder) is rejected.
5. A credit-card account with stored balance −200 displays 200.
6. Archiving `Checking` with balance 10.00 is rejected; with 0.00 it succeeds; a new
   transaction touching it is then rejected.
7. Deleting an account that ever had a split is rejected; deleting a fresh one works.
8. A new book contains exactly one account, `Opening Balances` (equity).
9. Creating an account in `EUR` in a `USD` book is rejected while `multi-currency` is
   off and allowed after enabling it.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-11-04) — starter charts, tax lines, roles

`chart-presets.md` adds the starter charts this spec left for later. Accounts gain
an optional `tax_line` (user-editable) and a program-set `role`; "Default accounts"
still holds for a book created without a preset (AT 8 unchanged). A book created
from a preset also holds that preset's accounts. See `chart-presets.md` for the
field rules, the role lookup rule and its acceptance tests (CP1–CP28).

## Revision 3 (2026-11-05) — changing a balance-sheet account's kind (owner request)

Owner report: linking SimpleFIN created every account as Bank (the provider does not
say which accounts are cards), and nothing could turn one into a credit card.

- `account.update` accepts `type` together with `subtype` for a **money account**:
  an account whose type is asset with subtype `bank`, `cash` or `other-asset`, or
  liability with subtype `credit-card`, `loan` or `other-liability`. The new pair must
  also be one of those six. Within that set the change is allowed **even when the
  account has splits**: stored amounts are debit-positive whatever the type, so a
  purchase (a credit to the account) is the same split in a bank account and in a
  card; only the displayed balance's sign and the side of the balance sheet change.
  Every other type or subtype change keeps the existing rules (type immutable once
  used; subtype only while unused).
- A **placeholder** account, or one with children, cannot change type (its children
  would have a different type from their parent); it may change subtype within the
  same type as above.
- When the type changes, the account's parent must have the new type: the command
  takes an optional `parent` (an account of the new type, or none for top level). When
  `parent` is not given, the account moves under the first placeholder account (by
  full name) of the new type that already holds an account of the new subtype (for
  example a "Credit Cards" group holding other cards), else to the top level. Its
  name must still be unique among
  its new siblings; a clash is refused with the name to change.
- Links (bank feeds, reconcile history, scheduled transactions, budgets) refer to the
  account id and are unchanged. The audit log records the old and new type, subtype and
  parent; the change is undoable.
- Balances and reports change accordingly: past net worth now counts the account as a
  liability, which is the correct reading.

Acceptance tests:

R3-1. A bank account with a 50.00 deposit and a 20.00 purchase (balance 30.00) changed
      to liability/credit-card keeps both splits; its stored sum stays +30.00 and is
      displayed negated as −30.00 (a credit balance on the card); the balance sheet
      lists it under liabilities, and net worth (assets − liabilities) is unchanged.
R3-2. Changing a brokerage, receivable or payable account's type is refused; changing a
      placeholder with children is refused.
R3-3. With a placeholder liability "Credit Cards" holding one card, a bank account changed to
      credit-card without `parent` moves under it; with a sibling of the same name there,
      the change is refused naming the clash.
R3-4. Undo restores type, subtype and parent.
