# Spec: Investment import (Phase 3b)

- **Area / module:** import + `investments`
- **Spec writer:** Claude session 2026-10-20 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-20
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Standard | OFX 2.2 Specification (FDX, 2017), ch. 13 "Investments": §13.8 securities (`SECID`, `SECLIST`, `SECINFO`), §13.9.1 sign conventions, §13.9.2.2 `INVSTMTRS`, §13.9.2.3 `INVBANKTRAN`, §13.9.2.4 transactions (`INVTRAN`, `INVBUY`, `INVSELL`, the transaction aggregates, `REVERSALFITID`), §13.9.2.6 positions (`INVPOS`), §13.9.2.7 balances | Owner-supplied PDF |
| 2 | Standard | RFC 8259 (JSON), W3C XML 1.0, RFC 4180 (CSV) | Generic file syntax |
| 3 | Spec | `import-pipeline.md`, `import-ofx.md`, `import-csv.md`, `investments-core.md` | Batches, sections, fingerprints, review, CSV profiles, investment actions |
| 4 | Owner | Owner's broker offers CSV, JSON and XML downloads (2026-10-20); samples to follow | Built-in profiles for that broker are added in a later revision of this spec once samples arrive |

No target product was observed. Names and wording are MoneyKeep's own.

## Purpose

Bring a broker's transaction downloads into investment accounts through the same
stage → review → commit pipeline as bank files: no duplicates, securities recognised,
cost basis built by the normal investment actions, and a check that the book's
holdings agree with the broker's.

## Behaviour

Requires the `investments` module for investment sections; with it off, investment
sections are reported as skipped (as `import-ofx.md` does today) and nothing else
changes.

### Statement model additions

`import-pipeline.md`'s statement gains a section kind `investment` with:

- account identity (source key), currency, period, as-of date;
- **securities** found in the file: source security key (CUSIP or other id + id
  type, or ticker, or name), ticker, name, kind, optional price and price date;
- **investment rows**, each with: `external_id`, `date`, `action` (one of the
  `investments-core.md` actions: `buy`, `sell`, `dividend`, `reinvest`, `interest`,
  `fee`, `shares_in`, `shares_out`, `split`, `return_of_capital`, plus `cash` for
  plain cash movements and `reverse` / `unsupported`), source security key,
  `quantity`, `price`, `commission` (all transaction costs combined), `amount`
  (absolute cash total), dividend `kind`, split ratio, transfer basis and acquired
  date, `memo`, the original `raw` record, and a `notes` list;
- **positions** (security key, quantity, price, market value, as-of date) and
  **cash balance**, when the file has them.

### Staging, security resolution and classification

- **Section → account:** as for bank files, through `import_account_map`; the account
  must be a brokerage account.
- **Security resolution**, per source security key, in order: a stored mapping
  (`import_security_map`, saved on commit); an existing security with the same CUSIP;
  the same ticker (symbol); otherwise the security is **proposed as new** with the
  file's ticker (or a symbol derived from the name, flagged for review), name and kind
  (stock / mutual fund / bond / other from the file, else `stock`). The user can
  accept, edit, or map it to an existing security before commit.
- **Fingerprint:** `ext:` + account + `external_id` when present; otherwise `hash:` +
  account + SHA-256 of `date | action | security key | quantity | amount |
  occurrence`.
- **Classification:**
  1. *Duplicate* — the fingerprint is already on a transaction in the account.
  2. *Match* — an existing investment transaction in the account with no fingerprint,
     the same action and security, the same quantity and amount, and a date within
     the match window. Commit attaches the fingerprint.
  3. *Reverse* — a row whose source names a transaction being reversed (OFX
     `REVERSALFITID`): proposed as **void** the original (found by fingerprint); if
     the original is not in the book the row is skipped with a note.
  4. *Unsupported* — actions outside `investments-core.md` (options, short sales,
     sub-account journals, foreign currency): skipped, with the reason shown.
  5. *New* — everything else; flagged `needs_review` when a note says so (e.g. income
     type MISC, missing transfer basis).
- **Commit** runs the actions in date order (then file order) through the same rules
  as the `inv.*` commands, in **one command**: securities created, mappings saved,
  transactions created or matched or voided, lots replayed once. Undo reverses the
  whole commit. Rows created from a download start cleared (`c`), as bank rows do.
- **Prices:** security prices found in the file (security list or positions) are
  stored with their dates when the setting `import.store_prices` is on (default on);
  an existing price for the same day is not overwritten.
- **Positions check:** after staging, and again after commit, the statement's
  positions and cash are compared with `inv.holdings` for the account as of the
  statement's as-of date. Differences (security, book quantity, statement quantity;
  cash) are listed in the batch result; agreement says "Holdings agree with the
  statement". This is informational and never blocks commit.

### OFX investment statements (`INVSTMTRS`)

Parsing extends `import-ofx.md` (same SGML/XML handling, charset, date and amount
rules). `INVSTMTRS` sections are no longer skipped when the module is on.

- **Account:** `INVACCTFROM` (`BROKERID` + `ACCTID`), hashed into the source key; only
  the last four digits are shown, as for bank accounts.
- **Securities:** `SECLIST` entries (`STOCKINFO`, `MFINFO`, `DEBTINFO`, `OTHERINFO`;
  `OPTINFO` → unsupported) give `SECID` (`UNIQUEID` + `UNIQUEIDTYPE`, e.g. CUSIP),
  `TICKER`, `SECNAME`, `UNITPRICE` + `DTASOF`. The source security key is
  `UNIQUEIDTYPE:UNIQUEID`.
- **Signs (§13.9.1):** in buys `UNITS` is positive and `TOTAL` negative; in sells
  `UNITS` negative and `TOTAL` positive; prices, commissions, fees, loads and taxes are
  always positive. MoneyKeep uses absolute values for quantity and amount.
- **Date:** `DTTRADE` (for splits, the day of record). `FITID` → `external_id`.
  `MEMO` → memo.
- **Transaction mapping:**

| OFX aggregate | MoneyKeep row |
|---|---|
| `BUYSTOCK`, `BUYMF`, `BUYOTHER`, `BUYDEBT` | `buy`: quantity `UNITS`, price `UNITPRICE`, commission = `COMMISSION`+`FEES`+`TAXES`+`LOAD`, amount = \|`TOTAL`\|. `BUYDEBT` notes that accrued interest (`ACCRDINT`) is not included. `BUYTYPE` BUYTOCOVER → unsupported |
| `SELLSTOCK`, `SELLMF`, `SELLOTHER`, `SELLDEBT` | `sell`: quantity \|`UNITS`\|, price, commission as above (plus `WITHHOLDING`, `PENALTY`), amount = `TOTAL`. `SELLTYPE` SELLSHORT → unsupported. The file's `GAIN` is ignored (MoneyKeep computes gains) |
| `INCOME` | `INCOMETYPE` DIV → `dividend` kind `ordinary` (setting `import.ofx_div_kind`, default `ordinary`); CGLONG → `capital_gain_long`; CGSHORT → `capital_gain_short`; INTEREST → `interest`; MISC → `dividend` `ordinary`, needs review. `TAXEXEMPT` Y → kind `tax_exempt`. A negative `TOTAL` (an adjustment) → needs review |
| `REINVEST` | `reinvest`: kind from `INCOMETYPE` as above, quantity `UNITS`, price `UNITPRICE`, amount \|`TOTAL`\|; a non-zero commission/fee → needs review (reinvest carries no commission in 3a) |
| `INVEXPENSE` | `fee` with the security |
| `MARGININTEREST` | `fee` to the expense account `Margin Interest` (created on first use) |
| `RETOFCAP` | `return_of_capital`, amount `TOTAL` |
| `SPLIT` | `split`: ratio `NUMERATOR`:`DENOMINATOR` (else `NEWUNITS`:`OLDUNITS`). `FRACCASH` > 0 adds a note: cash in lieu of fractional shares needs a manual sale entry |
| `TRANSFER` | `TFERACTION` IN → `shares_in` (one lot: quantity `UNITS`, basis `AVGCOSTBASIS`, acquired `DTPURCHASE` else trade date; missing basis → needs review, the review asks for it); OUT → `shares_out`. `POSTYPE` SHORT → unsupported |
| `INVBANKTRAN` / `STMTTRN` | `cash`: a plain cash row in the brokerage account, categorised exactly like a bank row (rules, transfer matching, uncategorised accounts) |
| `JRNLFUND`, `JRNLSEC`, `CLOSUREOPT`, `BUYOPT`, `SELLOPT` | unsupported |
| Any aggregate with `REVERSALFITID` | `reverse` (see classification) |
| `CURRENCY`/`ORIGCURRENCY` different from the base currency | unsupported (foreign currency) |

- **Positions:** `INVPOSLIST` `POSSTOCK`/`POSMF`/`POSDEBT`/`POSOTHER` → `INVPOS`
  `SECID`, `UNITS`, `UNITPRICE`, `MKTVAL`, `DTPRICEASOF` (19900101 means unknown: no
  price stored). **Cash:** `INVBAL` `AVAILCASH`.
- **401(k)** sub-account and loan fields are ignored with one note per file.

### CSV, JSON and XML broker files — layout profiles

Broker files in these formats have no common standard, so MoneyKeep reads them with
a saved **investment profile**, much like bank CSV profiles (`import-csv.md`).

**Profile fields:**

| Field | Meaning |
|---|---|
| `name` | e.g. "My broker — CSV" |
| `format` | `csv`, `json` or `xml` |
| `signature` | How a file is recognised: CSV header names; JSON key names present; XML root and record element names. Matching files use the profile automatically |
| `records` | Where the records are: CSV — all data rows (with `skip_rows` before the header); JSON — a path to the array (e.g. `data.transactions`, `[*]` for a top-level array); XML — the path of the repeating record element (e.g. `Statement/Transactions/Transaction`) |
| `fields` | For each neutral field (`date`, `action`, `symbol`, `cusip`, `security_name`, `quantity`, `price`, `commission`, `fees`, `amount`, `description`, `external_id`, `account`): the CSV column, JSON key path (dot notation, relative to the record) or XML child element / `@attribute` path. Unused fields are blank |
| `date_format` | One of the formats accepted by `import-csv.md` |
| `number_format` | Decimal point `.` or `,`; thousands separator; parentheses mean negative (yes/no) |
| `actions` | An ordered list of rules mapping the broker's action text to a MoneyKeep action (and dividend kind): match type `equals` / `starts_with` / `contains` (case-insensitive) on the action field, or on the description field when the profile has no action field. First match wins; no match → row `unsupported` with "Unknown action: <text>" |
| `signs` | Whether `quantity` and `amount` carry the cash direction (MoneyKeep takes absolute values and relies on the action) — always true; recorded so the editor can explain it |
| `account_key` | Optional field whose value identifies the account (files covering several accounts become several sections) |
| `positions` | Optional: a second records path and field set for holdings, if the file includes them |

**Commands:** `inv_profile.save`, `inv_profile.list`, `inv_profile.get`,
`inv_profile.delete`, `inv_profile.suggest` (given a file: detects the format, the
record path, likely field mappings from common names — e.g. date / trade date,
symbol / ticker, quantity / shares, price, commission, amount / net amount, action /
activity / type — and the distinct action texts found, so the user only has to map
them), and `inv_profile.preview` (a profile + a file → the first 20 neutral rows with
any errors). `import.stage` accepts `.csv`, `.json` and `.xml` for investment
sections when a profile matches, or names a profile explicitly.

**Parsing rules:** JSON per RFC 8259 (UTF-8, BOM ignored); numbers may be JSON numbers
or strings. XML per XML 1.0 with external entities and DTDs **disabled** (no
network or file access); namespaces ignored by local name. CSV per `import-csv.md`.
A file larger than the import size limit is rejected as for other formats.

**Built-in profiles** for the owner's broker will be added (a later revision of this
spec, from the owner's anonymised samples) and are recognised automatically.

## Ledger effect

As `investments-core.md` actions and `import-pipeline.md` commits.

## Settings

| Key | Type | Default |
|---|---|---|
| `import.store_prices` | bool | true |
| `import.ofx_div_kind` | `ordinary` / `qualified` | `ordinary` |

## Edge cases

- A file listing the same security under two ids (CUSIP and ticker) resolves both to
  one security once the user maps them; the mapping is remembered.
- A sell in the file for shares bought before the book's history began: commit fails
  that row with `insufficient_shares`; the review offers to add a **Move shares in**
  first (with basis and acquired date to be entered).
- Re-importing an overlapping file: rows already committed are duplicates.
- A positions-only file (no transactions): stages no rows but reports the positions
  check and stores prices.

## Acceptance tests

OFX fixture `examples/fixtures/ofx/investment-v2.ofx` (written by the implementer from
§13.9, synthetic data) with: `SECLIST` VTI (CUSIP 922908769, price 125.00 as of
2026-10-16) and a mutual fund; `BUYSTOCK` 10 VTI @ 200.00, commission 5.00, `TOTAL`
−2005.00; `SELLSTOCK` −12 units, `TOTAL` 2995.00; `INCOME` DIV 15.00; `REINVEST` DIV
22.00, 0.1 units @ 220.00; `INVBANKTRAN` deposit 5,000.00; `SPLIT` 2:1; `TRANSFER` IN
with `AVGCOSTBASIS`; one `BUYOPT`; `INVPOSLIST` and `INVBAL`.

1. Staging gives one investment section; VTI resolves as a new security (ticker VTI,
   name from `SECNAME`, CUSIP kept); the option row is `unsupported` with its reason;
   the deposit is a `cash` row.
2. The buy row: quantity 10, price 200.00, commission 5.00, amount 2,005.00; the sell
   row: quantity 12, amount 2,995.00.
3. Commit creates the transactions through the action rules: lots and gains equal
   those of `investments-core.md` for the same trades; one audit entry; undo removes
   everything including the created security.
4. Re-staging the same file: every committed row is a duplicate.
5. A row with `REVERSALFITID` naming the buy proposes voiding that buy.
6. The price 125.00 for 2026-10-16 is stored; with `import.store_prices` off it is not.
7. The positions check reports agreement for VTI when the book matches, and a
   difference line when one share is missing.
8. `INCOME` with `INCOMETYPE` MISC is flagged needs review; CGLONG becomes a long-term
   capital gain distribution.
9. JSON: a fixture `{"data":{"transactions":[{"tradeDate":"10/01/2026","activity":
   "Bought","symbol":"VTI","shares":"10","price":"200","fees":"5","netAmount":
   "-2005.00","id":"T1"}]}}` with a profile (records `data.transactions`, action rule
   `equals Bought → buy`) stages a buy identical to OFX test 2; `inv_profile.suggest`
   proposes those field mappings and lists `Bought` as an unmapped action text.
10. XML: the same record as `<Statement><Transactions><Transaction id="T1">…` with
    child elements, using a profile with `@id` for `external_id`, stages the same buy.
    A document with a DTD or external entity is rejected without fetching anything.
11. CSV: the same record with headers `Trade Date,Activity,Symbol,Shares,Price,
    Fees,Net Amount,Id` stages the same buy; an action text with no rule is
    `unsupported` with "Unknown action: …".

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above

## Revision 2 (2026-10-20) — owner's broker samples (Charles Schwab)

The owner supplied anonymised CSV, JSON and XML transaction downloads from Charles
Schwab, all three covering 2026-04-20 to 2026-10-20 with identical records. They
reveal four needs beyond revision 1. (Schwab is named only to label the built-in
profile; MoneyKeep is not affiliated with it.)

### What the files look like

- **CSV:** header `"Date","Action","Symbol","Description","Quantity","Price","Fees &
  Comm","Amount"`; every field quoted; dates `MM/DD/YYYY`; money with a leading `$`
  and `-` before it for outflows (`-$1065.66`); quantities with thousands commas
  (`40,000`, `-10,000`); empty strings for absent values.
- **JSON:** an object with `FromDate`, `ToDate`, `TotalTransactionsAmount`,
  `TotalFeesAndCommAmount` and an array `BrokerageTransactions` of objects with string
  values: `Date`, `Action`, `Symbol`, `Description`, `Quantity`, `Price`,
  `Fees & Comm`, `Amount` (with thousands commas, e.g. `-$1,065.66`), `AcctgRuleCd`.
- **XML:** root `BrokerageHistoryResponse` with `FromDate`, `ToDate`, the two totals,
  and `BrokerageTransactions/BrokerageTransaction` elements whose children are
  `Date`, `Action`, `Symbol`, `Description`, `Quantity`, `Price`, `FeesAndCommission`,
  `Amount`, `AcctgRuleCd`; empty values are empty elements (`<Price />`).
- No transaction ids (fingerprints use the hash form) and no account number inside
  the file; the account appears in the file name (`…_XXX039_Transactions_…`).
- Action texts seen: `Buy`, `Buy to Open` (an option), `Credit Interest` (cash
  interest, no symbol), `CD Interest` (interest from a CD, with its CUSIP),
  `CD Deposit Funds` and `CD Deposit Adj` (a CD maturing: cash at face value, and the
  removal of the CD position, as two rows on the same date with the same symbol).
- The `Symbol` of a CD is its 9-character CUSIP; the description carries issuer,
  rate and maturity (`AMERIS BANK 4.25%27CD FDIC INS DUE 04/16/27US`).
- **CD prices are quoted per 100 of face value**: Quantity 40,000 (face), Price
  `$100.00`, Amount `-$40,000.00`.
- The file total `TotalTransactionsAmount` equals the sum of the `Amount` values
  (checked: 9,871.16).

### Changes

1. **Price per 100 (amends `investments-core.md`).** Security kinds gain `cd`. For
   kinds `bond` and `cd` the quantity is the face amount and the price is a
   percentage of face: value = round(quantity × price ÷ 100, 2). This applies to
   `inv.buy`, `inv.sell`, holdings market value and every preview. (OFX positions
   already quote bonds as a percentage of par, §13.9.2.6.1.) A CD's default basis
   method is `fifo`; average is not offered.
2. **Paired rows.** An action rule may map a text to `sell_quantity` or
   `sell_proceeds`. Rows with these actions are paired by **same date and same
   security**: the quantity-only row supplies the quantity, the amount-only row the
   amount, and together they stage as one `sell` (price = amount × 100 ÷ quantity
   for bond/cd kinds, else amount ÷ quantity). A row left unpaired is flagged needs
   review ("Maturity cash without the position change", or the reverse). The pair's
   raw records are both kept.
3. **Money and number parsing.** Profile `number_format` gains `currency_symbols`
   (default `$`, `€`, `£`): stripped wherever they appear, with a minus sign allowed
   before or after the symbol (`-$1,065.66`, `$-1,065.66`), and thousands separators
   accepted in every numeric field, whatever the field.
4. **Account from the file name.** Profile field `account_from_file_name`: a pattern
   with one capture group; its match becomes the section's account key (so the
   account mapping is remembered per account). Without a match the section is
   unresolved, as usual.
5. **Security kind and identity hints.** Profile field `security_rules`: ordered
   rules that set the security's id type and kind from the symbol and description,
   e.g. "symbol is 9 letters/digits and description contains `CD`" → id type CUSIP,
   kind `cd`; "symbol is 9 letters/digits" → id type CUSIP, kind `bond`; otherwise
   ticker, kind `stock`. The proposed new security's name is the description with
   any trailing country code (` US`, `US`) removed, at most 100 characters.
6. **File total check.** Profile field `total_field` (JSON/XML path, e.g.
   `TotalTransactionsAmount`): if present, the sum of the file's `Amount` values is
   compared with it and a warning is shown when they differ (never blocks).
7. **Positions before the file.** When a `sell` (or paired maturity) would oversell
   because the purchase predates the file, the review's **Move shares in first**
   proposal pre-fills, for `cd`/`bond` kinds, basis = face × 100 ÷ 100 (i.e. the face
   amount) and acquired date blank (the user enters it; the holding period matters
   little for a CD redeemed at face, so a blank date is accepted and the lot is
   treated as acquired one day before the first transaction in the book for it,
   with a note).

### Built-in profiles: "Charles Schwab — CSV", "— JSON", "— XML"

Data only (no special code), recognised by their signatures:

| Item | CSV | JSON | XML |
|---|---|---|---|
| Signature | header exactly as above | keys `BrokerageTransactions` and `TotalTransactionsAmount` | root `BrokerageHistoryResponse` |
| Records | data rows | `BrokerageTransactions` | `BrokerageHistoryResponse/BrokerageTransactions/BrokerageTransaction` |
| Fees field | `Fees & Comm` | `Fees & Comm` | `FeesAndCommission` |
| Other fields | `Date`, `Action`, `Symbol`, `Description`, `Quantity`, `Price`, `Amount` (same names in all three) | | |
| Total field | — | `TotalTransactionsAmount` | `TotalTransactionsAmount` |
| Account from file name | `_XXX(\d{3,4})_` in all three | | |
| Date format | `MM/DD/YYYY` | | |

Action rules (all three; `equals`, case-insensitive):

| Broker text | MoneyKeep |
|---|---|
| `Buy` | `buy` |
| `Sell` | `sell` |
| `Credit Interest` | `interest` (no security) |
| `CD Interest`, `Bond Interest` | `interest` with the security |
| `CD Deposit Funds` | `sell_proceeds` |
| `CD Deposit Adj` | `sell_quantity` |
| `Qualified Dividend` | `dividend` kind `qualified` |
| `Cash Dividend`, `Non-Qualified Div` | `dividend` kind `ordinary` |
| `Reinvest Dividend` | `dividend` (cash leg; see below) |
| `Reinvest Shares` | `reinvest_shares` |
| `Long Term Cap Gain` | `dividend` kind `capital_gain_long` |
| `Short Term Cap Gain` | `dividend` kind `capital_gain_short` |
| `Journal`, `MoneyLink Transfer`, `Funds Received`, `Wire Funds` | `cash` |
| `Service Fee`, `ADR Mgmt Fee`, `Foreign Tax Paid` | `fee` |
| `Stock Split` | `split` (needs review: the ratio is entered in the review) |
| `Buy to Open`, `Sell to Close`, `Sell to Open`, `Buy to Close`, `Expired`, `Assigned`, `Exchange or Exercise` | unsupported ("options are not supported yet") |

Only `Buy`, `Buy to Open`, `Credit Interest`, `CD Interest`, `CD Deposit Funds` and
`CD Deposit Adj` were seen in the samples; the others are MoneyKeep's best
expectation of the broker's wording and are flagged in the profile as **unverified**:
a row matched by an unverified rule is staged normally but marked needs review the
first time each such text is seen, so the owner can confirm or correct it.

`Reinvest Dividend` + `Reinvest Shares` on the same date and security are paired like
maturities: the dividend row's amount and the shares row's quantity (and price) stage
as one `reinvest`; unpaired rows stage as a plain dividend or a needs-review buy.

### Additional acceptance tests

Fixtures `examples/fixtures/broker/schwab-like.csv|json|xml`, written by the
implementer with **the same field names, quoting and number styles** as the samples
but invented values (the owner's files are not committed).

12. Each of the three fixtures is recognised by its built-in profile without the
    editor, and the three stage identical rows.
13. A CD bought at Quantity 25,000, Price $100.00, Amount −$25,000.00 stages a `buy`
    of face 25,000 at 100 with value 25,000.00 (not 2,500,000.00); holdings value it at
    25,000.00 with price 100.
14. `CD Deposit Funds` $25,000.00 and `CD Deposit Adj` −25,000 on the same date and
    CUSIP stage as one `sell` of 25,000 at 100 for 25,000.00, gain 0.00 when the buy
    is in the book; `CD Interest` $247.88 with that CUSIP stages as interest linked to
    the security.
15. A maturity whose buy is not in the book proposes Move shares in first with basis
    = the face amount.
16. `Buy to Open` stages as unsupported with "options are not supported yet".
17. `Credit Interest` $0.32 with no symbol stages as interest with no security.
18. The JSON/XML file total check passes for a consistent fixture and warns when one
    amount is altered.
19. A file named `…_XXX039_Transactions_….json` gets account key `039`; after mapping
    it once, the next file with `XXX039` resolves automatically.
20. Money parsing: `-$1,065.66`, `$-1,065.66`, `-$1065.66` and `"40,000"` parse to
    −1065.66, −1065.66, −1065.66 and 40000.

## Revision 3 (2026-10-20) — review of the owner's real files

The owner's three Schwab files staged correctly (17 rows each; CD buys at face value;
maturities paired into sells; the pre-file maturity flagged "Move shares in first";
the option row unsupported). Two fixes:

1. `import.stage` with `path` (CLI `import`, watched folder) must pass the path's
   file name to the parser exactly as `name` does, so `account_from_file_name` works
   there too (it currently yields no account key, and "Remember" is refused).
2. Proposed security names drop a trailing maturity marker: `XXX**MATURED**`,
   `**MATURED**` (and surrounding spaces), so a matured CD first seen in a file is
   named "CHARLES SCHWAB B 3.75" rather than "CHARLES SCHWAB B 3.75XXX**MATURED**".

21. `import.stage` {`path`: ".../Designated_Bene_Individual_XXX039_Transactions_x.json"}
    gives account key `039`, and mapping with `remember` succeeds.
22. A security first seen in a `CD Deposit Adj` row with description
    `CHARLES SCHWAB B 3.75XXX**MATURED** US` is proposed as "CHARLES SCHWAB B 3.75".

## Revision 4 (2026-10-20) — valuing holdings from trade-only files

Committing the owner's file left the Ameris CD (40,000 face, bought at 100) with
market value "unknown", because Schwab transaction files carry no price list, so the
account total omitted 40,000.00.

1. With `import.store_prices` on, each committed buy, sell or reinvest row also
   stores its trade price as that security's price for the trade date, unless a
   price for that date already exists (a price list in the file wins).
2. (Amends `investments-core.md` holdings.) A `cd` with no price on or before the
   valuation date is valued at face (price 100), shown as "at face value" instead of
   "no price", and counted in totals. Bonds without a price stay "no price".

23. After committing the owner-style file, holdings show the 40,000 CD at price
    100.00 (from the 2026-10-06 buy) and market value 40,000.00; the account total is
    cash + 40,000.00.
24. A CD created by `inv.shares_in` with no price anywhere is valued at face with the
    "at face value" marker.
