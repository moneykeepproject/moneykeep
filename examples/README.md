# Examples and test fixtures

Every file here records its origin. Only fixtures the team created itself, or whose
licence allows use as test data, may be added.

| File | Origin | Licence |
|---|---|---|
| — | — | — |

## Import fixtures (Phase 2a)

All files under `fixtures/` were created by the MoneyKeep implementer, synthetic. They
contain invented payees and amounts and were written by hand for the acceptance tests;
nothing was downloaded or copied from any bank, product or other project.

| File | Origin | Used by |
|---|---|---|
| `fixtures/qif/bank-three.qif` | created by the MoneyKeep implementer, synthetic | import-qif AT1, pipeline AT1/AT2 |
| `fixtures/qif/splits.qif` | created by the MoneyKeep implementer, synthetic | import-qif AT5 |
| `fixtures/qif/two-accounts.qif` | created by the MoneyKeep implementer, synthetic | import-qif AT8 |
| `fixtures/qif/investment.qif` | created by the MoneyKeep implementer, synthetic | import-qif AT9 |
| `fixtures/qif/no-final-caret.qif` | created by the MoneyKeep implementer, synthetic | import-qif AT10 |
| `fixtures/qif/utf8-bom.qif` | created by the MoneyKeep implementer, synthetic | import-qif AT7 |
| `fixtures/qif/cp1252.qif` | created by the MoneyKeep implementer, synthetic (Windows-1252 bytes) | import-qif AT7 |
| `fixtures/qif/dmy.qif` | created by the MoneyKeep implementer, synthetic | import-qif AT2 |
| `fixtures/qif/categories-only.qif` | created by the MoneyKeep implementer, synthetic | import-qif edge case |
| `fixtures/csv/basic.csv` | created by the MoneyKeep implementer, synthetic | import-csv AT1, AT7, AT8 |
| `fixtures/csv/preamble-split-balance.csv` | created by the MoneyKeep implementer, synthetic | import-csv AT3, AT6, AT9 |
| `fixtures/csv/type-column.csv` | created by the MoneyKeep implementer, synthetic | import-csv AT4 |
| `fixtures/csv/decimal-comma.csv` | created by the MoneyKeep implementer, synthetic | import-csv AT5 |

## OFX/QFX fixtures

All created by the MoneyKeep implementer, synthetic: hand-written OFX documents with
invented institutions, account numbers, payees and amounts, built to the structure
described in `docs/specs/import-ofx.md`. Nothing was downloaded or copied from any bank,
product or other project.

| File | Origin | Used by |
|---|---|---|
| `fixtures/ofx/checking-v1.ofx` | created by the MoneyKeep implementer, synthetic | import-ofx AT1, AT2, AT4, AT11 |
| `fixtures/ofx/checking-v2.ofx` | created by the MoneyKeep implementer, synthetic | import-ofx AT2, AT4 |
| `fixtures/ofx/creditcard-v1.ofx` | created by the MoneyKeep implementer, synthetic | import-ofx AT3 |
| `fixtures/ofx/dates-amounts.ofx` | created by the MoneyKeep implementer, synthetic | import-ofx AT5, AT6 |
| `fixtures/ofx/charset1252.ofx` | created by the MoneyKeep implementer, synthetic (Windows-1252 bytes) | import-ofx AT7 |
| `fixtures/ofx/utf8-v2.ofx` | created by the MoneyKeep implementer, synthetic | import-ofx AT7 |
| `fixtures/ofx/pending.ofx` | created by the MoneyKeep implementer, synthetic | import-ofx AT8 |
| `fixtures/ofx/correction-original.ofx`, `correction-replace.ofx`, `correction-delete.ofx` | created by the MoneyKeep implementer, synthetic | import-ofx AT9 |
| `fixtures/ofx/status-error.ofx` | created by the MoneyKeep implementer, synthetic | import-ofx AT10 |
| `fixtures/ofx/web-connect.qfx` | created by the MoneyKeep implementer, synthetic | import-ofx AT12 |
| `fixtures/ofx/doctype-xxe.ofx` | created by the MoneyKeep implementer, synthetic (hostile input) | import-ofx AT13 |
| `fixtures/ofx/foreign-and-fee.ofx` | created by the MoneyKeep implementer, synthetic | import-ofx AT14, AT15 |
| `fixtures/ofx/investment-v2.ofx` | created by the MoneyKeep implementer, synthetic, from the OFX 2.2 specification's element definitions | import-investments AT1–AT9, app-desktop AT79–AT80 |

## Broker fixtures (Phase 3b)

Written by the MoneyKeep implementer with the same field names, quoting and number
styles as broker export samples supplied to the project (which are not committed);
every value (dates, CUSIPs, amounts, descriptions) is invented.

| File | Origin | Used by |
|---|---|---|
| `fixtures/broker/schwab-like.csv` | created by the MoneyKeep implementer, synthetic | import-investments AT12–AT19 |
| `fixtures/broker/schwab-like.json` | created by the MoneyKeep implementer, synthetic | import-investments AT12–AT19 |
| `fixtures/broker/schwab-like.xml` | created by the MoneyKeep implementer, synthetic | import-investments AT12–AT19 |

## Definition and adversarial fixtures (Phase 3d)

All created by the MoneyKeep implementer, synthetic.

| File | Origin | Used by |
|---|---|---|
| `fixtures/definitions/starter.mkdef` | created by the MoneyKeep implementer, synthetic | definition-files AT1, app-desktop AT83–AT85, parser fuzzing seed |
| `fixtures/adversarial/*` | created by the MoneyKeep implementer, synthetic hostile inputs (billion laughs, external and undeclared entities, 10,000-deep JSON/XML/OFX nesting, a 10,000-digit number, invalid UTF-8, a BOM mid-file, NUL and escape bytes, bidi/formula payees, duplicate JSON keys, a hostile definition) | input-safety AT4–AT5, parser fuzzing |
