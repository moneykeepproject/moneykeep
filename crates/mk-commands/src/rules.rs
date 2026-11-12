//! Categorisation rule commands (spec: import-rules.md).

use mk_core::{AccountId, Decimal, Id};
use mk_import::{
    Action, Condition, ConditionField, ConditionOp, Rule, RuleKind, RuleRow, evaluate_rules,
    split_amount, validate_rule,
};
use mk_store::{Doc, DocTable, now_rfc3339};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "rule.create",
        "Create a categorisation rule for imports",
        true,
        create,
    );
    r.add("rule.update", "Change a categorisation rule", true, update);
    r.add("rule.delete", "Delete a categorisation rule", true, delete);
    r.add(
        "rule.list",
        "List categorisation rules in priority order",
        false,
        list,
    );
    r.add(
        "rule.reorder",
        "Set rule priorities from an ordered list of rule ids",
        true,
        reorder,
    );
    r.add(
        "rule.test",
        "Evaluate the rules against sample row data without changing anything",
        false,
        test,
    );
}

pub(crate) fn load_rules(ctx: &Ctx<'_>) -> CommandResult<Vec<Doc>> {
    let mut docs = ctx.tx.docs(DocTable::Rule, None)?;
    docs.sort_by(|a, b| {
        let ca = a
            .json
            .get("created_at")
            .and_then(Value::as_str)
            .unwrap_or("");
        let cb = b
            .json
            .get("created_at")
            .and_then(Value::as_str)
            .unwrap_or("");
        a.ord
            .cmp(&b.ord)
            .then_with(|| ca.cmp(cb))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(docs)
}

pub(crate) fn rule_from_doc(d: &Doc) -> CommandResult<Rule> {
    serde_json::from_value(d.json.clone())
        .map_err(|e| CommandError::Integrity(format!("bad stored rule {}: {e}", d.id)))
}

fn rule_doc(r: &Rule) -> CommandResult<Doc> {
    Ok(Doc {
        id: r.id.to_string(),
        grp: None,
        ord: r.priority,
        json: serde_json::to_value(r)?,
    })
}

fn invalid(e: mk_import::ImportError) -> CommandError {
    CommandError::InvalidArgs(e.to_string())
}

/// Resolve account references in actions to ids and check they are usable.
fn resolve_actions(chart: &Chart, actions: &mut [Action]) -> CommandResult<()> {
    let fix = |r: &mut String| -> CommandResult<()> {
        let acc = chart.resolve(r)?;
        if acc.placeholder || acc.archived {
            return Err(CommandError::Rejected(format!(
                "{} is a placeholder or archived account",
                chart.full_name(acc.id)
            )));
        }
        *r = acc.id.to_string();
        Ok(())
    };
    for a in actions {
        match a {
            Action::SetAccount { account } => fix(account)?,
            Action::Split { shares } => {
                for s in shares.iter_mut() {
                    fix(&mut s.account)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Store tag actions with record ids (names or ids accepted).
fn resolve_tag_actions(ctx: &Ctx<'_>, actions: &mut [Action]) -> CommandResult<()> {
    use crate::dimensions::resolve_tags;
    for a in actions {
        match a {
            Action::SetClass { class } => {
                *class = resolve_tags(ctx, Some(class), None, None)?
                    .class
                    .ok_or_else(|| CommandError::InvalidArgs("set_class needs a class".into()))?;
            }
            Action::SetLocation { location } => {
                *location = resolve_tags(ctx, None, Some(location), None)?
                    .location
                    .ok_or_else(|| {
                        CommandError::InvalidArgs("set_location needs a location".into())
                    })?;
            }
            Action::SetFund { fund } => {
                *fund = resolve_tags(ctx, None, None, Some(fund))?
                    .fund
                    .ok_or_else(|| CommandError::InvalidArgs("set_fund needs a fund".into()))?;
            }
            Action::SetProject { project } => {
                *project =
                    crate::dimensions::resolve_tags_p(ctx, None, None, None, Some(project), None)?
                        .project
                        .ok_or_else(|| {
                            CommandError::InvalidArgs("set_project needs a project".into())
                        })?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn resolve_targets(chart: &Chart, accounts: &[String]) -> CommandResult<Vec<AccountId>> {
    accounts
        .iter()
        .map(|r| chart.resolve(r).map(|a| a.id))
        .collect()
}

/// Rules whose actions name a target account are rejected (the other side must
/// differ from the target).
fn check_not_target(rule: &Rule) -> CommandResult<()> {
    for a in &rule.actions {
        let names: Vec<&String> = match a {
            Action::SetAccount { account } => vec![account],
            Action::Split { shares } => shares.iter().map(|s| &s.account).collect(),
            _ => vec![],
        };
        for n in names {
            if rule.accounts.iter().any(|t| t.to_string() == *n) {
                return Err(CommandError::Rejected(
                    "a rule's account cannot be one of the target accounts it applies to".into(),
                ));
            }
        }
    }
    Ok(())
}

fn rule_json(chart: &Chart, r: &Rule) -> CommandResult<Value> {
    let mut v = serde_json::to_value(r)?;
    if let Some(actions) = v.get_mut("actions").and_then(Value::as_array_mut) {
        for a in actions {
            if let Some(id) = a
                .get("account")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<AccountId>().ok())
            {
                a["account_name"] = json!(chart.full_name(id));
            }
        }
    }
    Ok(v)
}

fn next_priority(ctx: &Ctx<'_>) -> CommandResult<i64> {
    Ok(load_rules(ctx)?
        .iter()
        .map(|d| d.ord)
        .max()
        .map_or(0, |m| m.saturating_add(1)))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    name: String,
    /// Lower runs first. Default: after all existing rules.
    #[serde(default)]
    priority: Option<i64>,
    #[serde(default = "yes")]
    enabled: bool,
    /// Target accounts this rule applies to (empty = all).
    #[serde(default)]
    accounts: Vec<String>,
    conditions: Vec<Condition>,
    actions: Vec<Action>,
    #[serde(default = "yes")]
    stop: bool,
}

fn yes() -> bool {
    true
}

fn create(ctx: &Ctx<'_>, a: CreateArgs) -> CommandResult<Value> {
    validate_rule(&a.name, &a.conditions, &a.actions).map_err(invalid)?;
    let chart = Chart::load(ctx)?;
    let mut actions = a.actions;
    resolve_actions(&chart, &mut actions)?;
    resolve_tag_actions(ctx, &mut actions)?;
    let rule = Rule {
        id: Id::<RuleKind>::new(),
        name: a.name.trim().to_owned(),
        priority: match a.priority {
            Some(p) => p,
            None => next_priority(ctx)?,
        },
        enabled: a.enabled,
        accounts: resolve_targets(&chart, &a.accounts)?,
        conditions: a.conditions,
        actions,
        stop: a.stop,
        created_at: now_rfc3339()?,
    };
    check_not_target(&rule)?;
    ctx.save_doc(
        DocTable::Rule,
        &rule.id.to_string(),
        Some(&rule_doc(&rule)?),
    )?;
    rule_json(&chart, &rule)
}

fn load(ctx: &Ctx<'_>, id: &str) -> CommandResult<Rule> {
    let d = ctx
        .tx
        .get_doc(DocTable::Rule, id)?
        .ok_or_else(|| CommandError::NotFound(format!("rule {id}")))?;
    rule_from_doc(&d)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    priority: Option<i64>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    accounts: Option<Vec<String>>,
    #[serde(default)]
    conditions: Option<Vec<Condition>>,
    #[serde(default)]
    actions: Option<Vec<Action>>,
    #[serde(default)]
    stop: Option<bool>,
}

fn update(ctx: &Ctx<'_>, a: UpdateArgs) -> CommandResult<Value> {
    let mut rule = load(ctx, &a.id)?;
    let chart = Chart::load(ctx)?;
    if let Some(n) = a.name {
        rule.name = n.trim().to_owned();
    }
    if let Some(p) = a.priority {
        rule.priority = p;
    }
    if let Some(e) = a.enabled {
        rule.enabled = e;
    }
    if let Some(acc) = a.accounts {
        rule.accounts = resolve_targets(&chart, &acc)?;
    }
    if let Some(c) = a.conditions {
        rule.conditions = c;
    }
    if let Some(mut actions) = a.actions {
        resolve_actions(&chart, &mut actions)?;
        resolve_tag_actions(ctx, &mut actions)?;
        rule.actions = actions;
    }
    if let Some(s) = a.stop {
        rule.stop = s;
    }
    validate_rule(&rule.name, &rule.conditions, &rule.actions).map_err(invalid)?;
    check_not_target(&rule)?;
    ctx.save_doc(
        DocTable::Rule,
        &rule.id.to_string(),
        Some(&rule_doc(&rule)?),
    )?;
    rule_json(&chart, &rule)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IdArgs {
    id: String,
}

fn delete(ctx: &Ctx<'_>, a: IdArgs) -> CommandResult<Value> {
    load(ctx, &a.id)?;
    ctx.save_doc(DocTable::Rule, &a.id, None)?;
    Ok(json!({ "deleted": a.id }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

fn list(ctx: &Ctx<'_>, _: NoArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let rules = load_rules(ctx)?;
    let out = rules
        .iter()
        .map(|d| rule_from_doc(d).and_then(|r| rule_json(&chart, &r)))
        .collect::<CommandResult<Vec<_>>>()?;
    Ok(Value::Array(out))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReorderArgs {
    /// Every rule id, in the new order (first runs first).
    ids: Vec<String>,
}

fn reorder(ctx: &Ctx<'_>, a: ReorderArgs) -> CommandResult<Value> {
    let rules = load_rules(ctx)?;
    let mut given = a.ids.clone();
    given.sort();
    given.dedup();
    let mut all: Vec<String> = rules.iter().map(|d| d.id.clone()).collect();
    all.sort();
    if given.len() != a.ids.len() || given != all {
        return Err(CommandError::InvalidArgs(
            "ids must list every rule exactly once".into(),
        ));
    }
    for (i, id) in a.ids.iter().enumerate() {
        let mut rule = load(ctx, id)?;
        let p = i64::try_from(i).unwrap_or(i64::MAX);
        if rule.priority != p {
            rule.priority = p;
            ctx.save_doc(DocTable::Rule, id, Some(&rule_doc(&rule)?))?;
        }
    }
    list(ctx, NoArgs {})
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TestArgs {
    /// Target account of the imported row (id, full name or code).
    account: String,
    /// Signed amount, money out negative.
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
    #[serde(default)]
    payee: String,
    #[serde(default)]
    memo: String,
    #[serde(default)]
    number: String,
    #[serde(default)]
    category_hint: String,
    /// OFX transaction type, e.g. FEE.
    #[serde(default)]
    trntype: Option<String>,
}

fn test(ctx: &Ctx<'_>, a: TestArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let target = chart.resolve(&a.account)?;
    let rules = load_rules(ctx)?
        .iter()
        .map(rule_from_doc)
        .collect::<CommandResult<Vec<_>>>()?;
    let row = RuleRow {
        target: target.id,
        amount: a.amount,
        payee: a.payee.clone(),
        memo: a.memo.clone(),
        number: a.number,
        category_hint: a.category_hint,
        trntype: a.trntype,
    };
    let usable = |id: &str| {
        id.parse::<AccountId>().is_ok_and(|x| {
            x != target.id
                && chart
                    .get(x)
                    .is_some_and(|acc| !acc.placeholder && !acc.archived)
        })
    };
    let mut warnings = Vec::new();
    let out = evaluate_rules(&rules, &row, &usable, &mut warnings);
    let mut other = Vec::new();
    if let Some(acc) = out
        .account
        .as_deref()
        .and_then(|x| x.parse::<AccountId>().ok())
    {
        other.push(
            json!({ "account": acc, "account_name": chart.full_name(acc), "value": -a.amount }),
        );
    }
    if let Some(shares) = &out.split {
        let pcts: Vec<Decimal> = shares.iter().map(|s| s.percent).collect();
        let parts = split_amount(a.amount, &pcts, &target.commodity).map_err(invalid)?;
        for (s, p) in shares.iter().zip(parts) {
            let acc = s.account.parse::<AccountId>().ok();
            other.push(json!({ "account": acc, "account_name": acc.map(|x| chart.full_name(x)), "value": -p }));
        }
    }
    Ok(json!({
        "rule_ids": out.rule_ids,
        "payee": out.payee.unwrap_or(a.payee),
        "memo": out.memo.unwrap_or(a.memo),
        "skip": out.skip,
        "other": other,
        "warnings": warnings,
    }))
}

/// `import.set_row` with `make_rule`: payee equals → set_account, lowest priority,
/// unless an identical rule exists.
pub(crate) fn learn_rule(ctx: &Ctx<'_>, payee: &str, account: AccountId) -> CommandResult<()> {
    let conditions = vec![Condition {
        field: ConditionField::Payee,
        op: ConditionOp::Equals,
        value: json!(payee),
    }];
    let actions = vec![Action::SetAccount {
        account: account.to_string(),
    }];
    for d in load_rules(ctx)? {
        let r = rule_from_doc(&d)?;
        if r.conditions == conditions && r.actions == actions && r.accounts.is_empty() {
            return Ok(());
        }
    }
    let rule = Rule {
        id: Id::<RuleKind>::new(),
        name: format!("Payee {payee}").chars().take(100).collect(),
        priority: next_priority(ctx)?,
        enabled: true,
        accounts: Vec::new(),
        conditions,
        actions,
        stop: true,
        created_at: now_rfc3339()?,
    };
    ctx.save_doc(
        DocTable::Rule,
        &rule.id.to_string(),
        Some(&rule_doc(&rule)?),
    )
}

/// `rule.create` / `rule.update` from JSON arguments (definition installs reuse the
/// validated command paths).
pub(crate) fn create_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    create(ctx, crate::parse_args(args)?)
}

pub(crate) fn update_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    update(ctx, crate::parse_args(args)?)
}
