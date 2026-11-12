# Spec: Categorisation rules

- **Area / module:** import (`core`)
- **Spec writer:** Claude session 2026-10-18 (spec-writer role; no target or prior-art source read)
- **Date:** 2026-10-18
- **Status:** draft

## Sources

| # | Type | Reference | Notes |
|---|---|---|---|
| 1 | Design | MoneyKeep plan: `mk-rules` crate, "rule: match conditions → payee/category/split template" | |
| 2 | Spec | `import-pipeline.md` | Where rules apply |

The rule model is MoneyKeep's own design. No target product was observed.

## Purpose

Most downloaded transactions repeat (the same grocery store, the same utility). Rules
categorise them automatically and clean up payee names.

## Behaviour

### Rule fields

| Field | Rules |
|---|---|
| `id` | Typed id |
| `name` | 1–100 characters |
| `priority` | Integer; lower runs first; ties by creation order |
| `enabled` | Boolean |
| `accounts` | Optional list of target accounts the rule applies to (empty = all) |
| `conditions` | 1–10 conditions, **all** must hold |
| `actions` | 1–5 actions |
| `stop` | Boolean, default true: when the rule matches, later rules are not tried |

### Conditions

Each is `{field, op, value}`:

| Field | Ops | Value |
|---|---|---|
| `payee`, `memo`, `number` | `contains`, `equals`, `starts_with`, `ends_with`, `regex` | string; text comparisons are case-insensitive on the normalised text (pipeline normalisation); `regex` uses a linear-time engine (no backreferences), max 200 characters |
| `amount` | `eq`, `lt`, `le`, `gt`, `ge`, `between` | decimal string(s), signed (money out negative) |
| `direction` | `eq` | `in` or `out` |
| `category_hint` | `equals`, `contains` | string |
| `trntype` | `equals` | OFX transaction type (`import-ofx.md`), e.g. `FEE`, `INT`, `CHECK`; rows from other formats have none and never match |

### Actions

| Action | Effect on the proposal |
|---|---|
| `set_account` | Other-side account (must not be the target account; must not be placeholder or archived) |
| `set_payee` | Replace payee text |
| `set_memo` | Replace memo text |
| `split` | Replace the other side with fixed-ratio splits: list of {account, percent} summing to exactly 100; amounts are rounded to the currency minor unit, and any rounding remainder goes to the first split |
| `skip` | Propose `skip` for the row |

`set_account` and `split` are mutually exclusive in one rule.

### Evaluation

- Rules run in the pipeline's "other side" step for rows classified **new** that have
  no source splits. Matches and duplicates are not changed by rules.
- Rules run in priority order; the first matching rule's actions apply; if its `stop`
  is false, later matching rules also apply, and a later action overrides an earlier
  action of the same kind.
- The proposal records which rule(s) applied (`rule_ids`) so review can show why.
- User overrides in review (`import.set_row`) always win over rules.

### Learning from review

When the user sets the other-side account of a new row in review, `import.set_row`
accepts `make_rule: true`, which creates a rule `payee equals <normalised payee>` →
`set_account <account>` at the lowest priority, unless an identical rule exists.

### Commands

`rule.create`, `rule.update`, `rule.delete`, `rule.list`, `rule.reorder` (sets
priorities from an ordered id list), `rule.test` (non-mutating: evaluates rules against
given row data and returns the resulting proposal and `rule_ids`). All mutating ones
are audited and undoable.

## Ledger effect

None by itself; rules shape what `import.commit` creates.

## Edge cases

- A rule whose account is later archived: it is skipped with a warning recorded on the
  proposal.
- Invalid regex: `rule.create` / `rule.update` reject it.
- `split` percentages like 33.33 / 33.33 / 33.34 are allowed (must sum to 100).

## Acceptance tests

1. Rule `payee contains "KROGER"` → `Groceries` categorises "KROGER #123 LIMA OH".
2. Two rules both matching, the higher-priority one with `stop: true`: only it applies.
3. `stop: false` chain: rule A sets payee "Kroger", rule B sets account Groceries:
   both apply.
4. `amount between -100.00 and -50.00` matches −75.00 but not −25.00.
5. `split` 50/50 of −10.01 USD gives −5.01 and −5.00 (remainder to the first split).
6. A user override in review beats a rule.
7. `make_rule: true` creates a rule that categorises the same payee on the next import.
8. A regex with a backreference is rejected.
9. `rule.test` returns the proposal and the applied rule ids without changing anything.

## Clean-room check

- [x] No code, pseudo-code from source, or internal identifiers from any target or prior-art project
- [x] No copied UI wording, layouts or screenshots
- [x] Every claim traceable to a source above
