//! Categorisation rules (spec: import-rules.md). Pure evaluation; storage and account
//! checks live in the command layer.

use mk_core::currency::minor_units;
use mk_core::{AccountId, CommodityCode, Decimal, Id};
use regex::RegexBuilder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::fingerprint::normalise_payee;
use crate::{ImportError, ImportResult};

/// Marker for rule ids.
#[derive(Debug)]
pub enum RuleKind {}

/// Fields a condition can test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConditionField {
    Payee,
    Memo,
    Number,
    Amount,
    Direction,
    CategoryHint,
    /// OFX transaction type (`FEE`, `INT`, `CHECK`, …); rows without one never match.
    Trntype,
}

/// Comparison operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOp {
    Contains,
    Equals,
    StartsWith,
    EndsWith,
    Regex,
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
    Between,
}

/// One condition. `value` is a string, or for `between` a two-element list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub field: ConditionField,
    pub op: ConditionOp,
    pub value: serde_json::Value,
}

/// One account and its percentage in a `split` action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SplitShare {
    /// Account id, full name or code (stored resolved to an id).
    pub account: String,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    pub percent: Decimal,
}

/// What a matching rule does to the proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    SetAccount {
        account: String,
    },
    SetPayee {
        payee: String,
    },
    SetMemo {
        memo: String,
    },
    Split {
        shares: Vec<SplitShare>,
    },
    Skip,
    /// Tag the row's category split with a class (stored as the class id).
    SetClass {
        class: String,
    },
    /// Tag with a location (stored as the location id).
    SetLocation {
        location: String,
    },
    /// Tag with a fund (stored as the fund id).
    SetFund {
        fund: String,
    },
    /// Tag with a project (stored as the project id; projects.md).
    SetProject {
        project: String,
    },
}

/// A stored rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: Id<RuleKind>,
    pub name: String,
    pub priority: i64,
    pub enabled: bool,
    /// Target accounts the rule applies to (empty = all).
    pub accounts: Vec<AccountId>,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    pub stop: bool,
    pub created_at: String,
}

/// Row data rules look at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRow {
    pub target: AccountId,
    pub amount: Decimal,
    pub payee: String,
    pub memo: String,
    pub number: String,
    pub category_hint: String,
    pub trntype: Option<String>,
}

/// The combined effect of the rules that matched.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct RuleOutcome {
    pub rule_ids: Vec<String>,
    pub account: Option<String>,
    pub payee: Option<String>,
    pub memo: Option<String>,
    pub split: Option<Vec<SplitShare>>,
    pub skip: bool,
    pub class: Option<String>,
    pub location: Option<String>,
    pub fund: Option<String>,
    pub project: Option<String>,
}

const MAX_REGEX_LEN: usize = 200;

fn text_value(c: &Condition) -> ImportResult<String> {
    c.value.as_str().map(str::to_owned).ok_or_else(|| {
        ImportError::Invalid(format!("{:?} condition needs a string value", c.field))
    })
}

fn decimal_value(v: &serde_json::Value) -> ImportResult<Decimal> {
    v.as_str()
        .and_then(|s| s.trim().parse::<Decimal>().ok())
        .ok_or_else(|| {
            ImportError::Invalid(format!(
                "amount condition value {v} must be a decimal string"
            ))
        })
}

/// Check a rule's shape: lengths, operator/field combinations, regexes, split sums,
/// and mutually exclusive actions. Account checks are the caller's.
pub fn validate_rule(name: &str, conditions: &[Condition], actions: &[Action]) -> ImportResult<()> {
    let n = name.trim().chars().count();
    if n == 0 || n > 100 {
        return Err(ImportError::Invalid(
            "rule name must be 1-100 characters".into(),
        ));
    }
    if !(1..=10).contains(&conditions.len()) {
        return Err(ImportError::Invalid(
            "a rule needs 1 to 10 conditions".into(),
        ));
    }
    if !(1..=5).contains(&actions.len()) {
        return Err(ImportError::Invalid("a rule needs 1 to 5 actions".into()));
    }
    use ConditionField as F;
    use ConditionOp as O;
    for c in conditions {
        let ok = match c.field {
            F::Payee | F::Memo | F::Number => {
                matches!(
                    c.op,
                    O::Contains | O::Equals | O::StartsWith | O::EndsWith | O::Regex
                )
            }
            F::Amount => matches!(c.op, O::Eq | O::Lt | O::Le | O::Gt | O::Ge | O::Between),
            F::Direction => c.op == O::Eq,
            F::CategoryHint => matches!(c.op, O::Equals | O::Contains),
            F::Trntype => c.op == O::Equals,
        };
        if !ok {
            return Err(ImportError::Invalid(format!(
                "operator {:?} does not apply to field {:?}",
                c.op, c.field
            )));
        }
        match (c.field, c.op) {
            (_, O::Regex) => compile_regex(&text_value(c)?)?.map(|_| ()).unwrap_or(()),
            (F::Amount, O::Between) => {
                let pair = c.value.as_array().filter(|a| a.len() == 2).ok_or_else(|| {
                    ImportError::Invalid("between needs a list of two amounts".into())
                })?;
                for v in pair {
                    decimal_value(v)?;
                }
            }
            (F::Amount, _) => {
                decimal_value(&c.value)?;
            }
            (F::Direction, _) => {
                let v = text_value(c)?;
                if v != "in" && v != "out" {
                    return Err(ImportError::Invalid(
                        "direction is \"in\" or \"out\"".into(),
                    ));
                }
            }
            _ => {
                text_value(c)?;
            }
        }
    }
    let has = |f: fn(&Action) -> bool| actions.iter().any(f);
    if has(|a| matches!(a, Action::SetAccount { .. })) && has(|a| matches!(a, Action::Split { .. }))
    {
        return Err(ImportError::Invalid(
            "set_account and split cannot both be in one rule".into(),
        ));
    }
    for a in actions {
        if let Action::Split { shares } = a {
            if shares.is_empty() {
                return Err(ImportError::Invalid(
                    "split needs at least one share".into(),
                ));
            }
            let mut total = Decimal::ZERO;
            for s in shares {
                if s.percent <= Decimal::ZERO {
                    return Err(ImportError::Invalid(
                        "split percentages must be positive".into(),
                    ));
                }
                total = total
                    .checked_add(s.percent)
                    .ok_or_else(|| ImportError::Invalid("overflow".into()))?;
            }
            if total != Decimal::ONE_HUNDRED {
                return Err(ImportError::Invalid(format!(
                    "split percentages sum to {total}, not 100"
                )));
            }
        }
    }
    Ok(())
}

/// Compile a rule regex: at most 200 characters, linear-time engine (no
/// backreferences or look-around, which the engine rejects).
fn compile_regex(pattern: &str) -> ImportResult<Option<regex::Regex>> {
    if pattern.chars().count() > MAX_REGEX_LEN {
        return Err(ImportError::Invalid(format!(
            "regex longer than {MAX_REGEX_LEN} characters"
        )));
    }
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .size_limit(1 << 20)
        .nest_limit(50)
        .build()
        .map(Some)
        .map_err(|e| ImportError::Invalid(format!("invalid regex: {e}")))
}

fn text_matches(op: ConditionOp, haystack: &str, needle: &str) -> bool {
    let h = normalise_payee(haystack);
    let n = normalise_payee(needle);
    match op {
        ConditionOp::Contains => h.contains(&n),
        ConditionOp::Equals => h == n,
        ConditionOp::StartsWith => h.starts_with(&n),
        ConditionOp::EndsWith => h.ends_with(&n),
        ConditionOp::Regex => compile_regex(needle)
            .ok()
            .flatten()
            .is_some_and(|re| re.is_match(&h)),
        _ => false,
    }
}

fn condition_holds(c: &Condition, row: &RuleRow) -> bool {
    use ConditionField as F;
    use ConditionOp as O;
    let text = |s: &str| text_value(c).is_ok_and(|v| text_matches(c.op, s, &v));
    match c.field {
        F::Payee => text(&row.payee),
        F::Memo => text(&row.memo),
        F::Number => text(&row.number),
        F::CategoryHint => {
            let v = text_value(c).unwrap_or_default().to_lowercase();
            let h = row.category_hint.trim().to_lowercase();
            match c.op {
                O::Equals => h == v.trim(),
                O::Contains => h.contains(v.trim()),
                _ => false,
            }
        }
        F::Trntype => {
            let v = text_value(c).unwrap_or_default();
            row.trntype
                .as_deref()
                .is_some_and(|t| t.trim().eq_ignore_ascii_case(v.trim()))
        }
        F::Direction => {
            let v = text_value(c).unwrap_or_default();
            (v == "in" && row.amount > Decimal::ZERO) || (v == "out" && row.amount < Decimal::ZERO)
        }
        F::Amount => {
            let a = row.amount;
            match c.op {
                O::Between => c
                    .value
                    .as_array()
                    .and_then(|p| {
                        Some((
                            decimal_value(p.first()?).ok()?,
                            decimal_value(p.get(1)?).ok()?,
                        ))
                    })
                    .is_some_and(|(x, y)| a >= x.min(y) && a <= x.max(y)),
                op => decimal_value(&c.value).is_ok_and(|v| match op {
                    O::Eq => a == v,
                    O::Lt => a < v,
                    O::Le => a <= v,
                    O::Gt => a > v,
                    O::Ge => a >= v,
                    _ => false,
                }),
            }
        }
    }
}

/// Evaluate enabled rules (already sorted by priority, then creation) against a row.
/// `usable` says whether an action's account may be used now (not archived or
/// placeholder); a rule naming an unusable account is skipped with a warning.
pub fn evaluate_rules(
    rules: &[Rule],
    row: &RuleRow,
    usable: &dyn Fn(&str) -> bool,
    warnings: &mut Vec<String>,
) -> RuleOutcome {
    let mut out = RuleOutcome::default();
    for rule in rules {
        if !rule.enabled {
            continue;
        }
        if !rule.accounts.is_empty() && !rule.accounts.contains(&row.target) {
            continue;
        }
        if !rule.conditions.iter().all(|c| condition_holds(c, row)) {
            continue;
        }
        let accounts_ok = rule.actions.iter().all(|a| match a {
            Action::SetAccount { account } => usable(account),
            Action::Split { shares } => shares.iter().all(|s| usable(&s.account)),
            _ => true,
        });
        if !accounts_ok {
            warnings.push(format!(
                "rule {:?} was skipped: an account it uses is archived, a placeholder or the target account",
                rule.name
            ));
            continue;
        }
        out.rule_ids.push(rule.id.to_string());
        for a in &rule.actions {
            match a {
                Action::SetAccount { account } => {
                    out.account = Some(account.clone());
                    out.split = None;
                }
                Action::Split { shares } => {
                    out.split = Some(shares.clone());
                    out.account = None;
                }
                Action::SetPayee { payee } => out.payee = Some(payee.clone()),
                Action::SetMemo { memo } => out.memo = Some(memo.clone()),
                Action::Skip => out.skip = true,
                Action::SetClass { class } => out.class = Some(class.clone()),
                Action::SetLocation { location } => out.location = Some(location.clone()),
                Action::SetFund { fund } => out.fund = Some(fund.clone()),
                Action::SetProject { project } => out.project = Some(project.clone()),
            }
        }
        if rule.stop {
            break;
        }
    }
    out
}

/// Divide `amount` by percentages, rounded to the currency's minor unit; the rounding
/// remainder goes to the first share.
pub fn split_amount(
    amount: Decimal,
    shares: &[Decimal],
    currency: &CommodityCode,
) -> ImportResult<Vec<Decimal>> {
    let dp = minor_units(currency.as_str());
    let overflow = || ImportError::Invalid("arithmetic overflow".into());
    let mut parts = Vec::with_capacity(shares.len());
    for p in shares {
        let raw = amount
            .checked_mul(*p)
            .and_then(|v| v.checked_div(Decimal::ONE_HUNDRED))
            .ok_or_else(overflow)?;
        parts.push(raw.round_dp_with_strategy(dp, rust_decimal_strategy()));
    }
    let mut sum = Decimal::ZERO;
    for p in &parts {
        sum = sum.checked_add(*p).ok_or_else(overflow)?;
    }
    let remainder = amount.checked_sub(sum).ok_or_else(overflow)?;
    if let Some(first) = parts.first_mut() {
        *first = first.checked_add(remainder).ok_or_else(overflow)?;
    }
    Ok(parts)
}

/// Shares round toward zero, so the remainder (added to the first share) has the
/// amount's sign.
fn rust_decimal_strategy() -> mk_core::RoundingStrategy {
    mk_core::RoundingStrategy::ToZero
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    #[test]
    fn split_rounding_remainder_to_first() {
        let usd = CommodityCode::new("USD").unwrap();
        assert_eq!(
            split_amount(d("-10.01"), &[d("50"), d("50")], &usd).unwrap(),
            [d("-5.01"), d("-5.00")]
        );
        let parts = split_amount(d("100.00"), &[d("33.33"), d("33.33"), d("33.34")], &usd).unwrap();
        assert_eq!(parts.iter().copied().sum::<Decimal>(), d("100.00"));
    }

    #[test]
    fn validation() {
        let c = |f, op, v| Condition {
            field: f,
            op,
            value: v,
        };
        let set = Action::SetAccount {
            account: "X".into(),
        };
        assert!(
            validate_rule(
                "r",
                &[c(ConditionField::Payee, ConditionOp::Contains, json!("A"))],
                std::slice::from_ref(&set)
            )
            .is_ok()
        );
        assert!(
            validate_rule(
                "r",
                &[c(
                    ConditionField::Payee,
                    ConditionOp::Regex,
                    json!(r"(a)\1")
                )],
                std::slice::from_ref(&set)
            )
            .is_err()
        );
        assert!(
            validate_rule(
                "r",
                &[c(
                    ConditionField::Payee,
                    ConditionOp::Regex,
                    json!("a".repeat(201))
                )],
                std::slice::from_ref(&set)
            )
            .is_err()
        );
        assert!(
            validate_rule(
                "r",
                &[c(ConditionField::Amount, ConditionOp::Contains, json!("1"))],
                std::slice::from_ref(&set)
            )
            .is_err()
        );
        assert!(validate_rule("r", &[], std::slice::from_ref(&set)).is_err());
        let split = Action::Split {
            shares: vec![SplitShare {
                account: "A".into(),
                percent: d("60"),
            }],
        };
        assert!(
            validate_rule(
                "r",
                &[c(ConditionField::Direction, ConditionOp::Eq, json!("out"))],
                &[split]
            )
            .is_err()
        );
    }
}
