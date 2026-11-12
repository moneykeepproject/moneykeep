//! Budget commands (spec: budgets.md). Every command needs the `budgets` module.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::str::FromStr;

use mk_core::currency::{format_amount, minor_units};
use mk_core::{AccountId, AccountType, Date, Decimal, RoundingStrategy};
use mk_store::{Doc, DocTable, now_rfc3339, today_utc};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::{CommandError, CommandResult, Ctx, Registry};

pub const BUDGETS: &str = "budgets";

pub(crate) fn register(r: &mut Registry) {
    let m = BUDGETS;
    r.add_in(m, "budget.create", "Create a monthly budget", true, create);
    r.add_in(
        m,
        "budget.update",
        "Rename a budget, change its start month or make it the default",
        true,
        update,
    );
    r.add_in(m, "budget.delete", "Delete a budget", true, delete);
    r.add_in(m, "budget.list", "List budgets", false, list);
    r.add_in(m, "budget.get", "Get one budget with its lines", false, get);
    r.add_in(
        m,
        "budget.set_line",
        "Create or replace a budget line for an income or expense account",
        true,
        set_line,
    );
    r.add_in(
        m,
        "budget.set_month",
        "Set one month's amount of a budget line",
        true,
        set_month,
    );
    r.add_in(
        m,
        "budget.remove_line",
        "Remove a budget line",
        true,
        remove_line,
    );
    r.add_in(
        m,
        "budget.suggest",
        "Suggest monthly amounts from past actuals",
        false,
        suggest,
    );
    r.add_in(
        m,
        "budget.apply_suggestion",
        "Set several budget lines in one change",
        true,
        apply_suggestion,
    );
    r.add_in(
        m,
        "budget.report",
        "Budget against actual for a month or a range of months",
        false,
        report,
    );
}

// ----- months ----------------------------------------------------------------------

/// A calendar month, written `YYYY-MM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct Month {
    year: i32,
    month: u8,
}

impl Month {
    fn of(d: Date) -> Self {
        Self {
            year: d.year(),
            month: d.month(),
        }
    }

    fn index(self) -> i64 {
        i64::from(self.year) * 12 + i64::from(self.month) - 1
    }

    fn shift(self, n: i64) -> Option<Self> {
        Date::shift_month(self.year, self.month, n).map(|(year, month)| Self { year, month })
    }

    /// Months from `self` to `last`, inclusive.
    fn through(self, last: Month) -> Vec<Month> {
        let n = last.index() - self.index();
        (0..=n.max(-1)).filter_map(|k| self.shift(k)).collect()
    }
}

impl fmt::Display for Month {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

impl FromStr for Month {
    type Err = CommandError;
    fn from_str(s: &str) -> CommandResult<Self> {
        let bad = || CommandError::InvalidArgs(format!("{s:?} is not a month (YYYY-MM)"));
        let (y, m) = s.split_once('-').ok_or_else(bad)?;
        if y.len() != 4 || m.len() != 2 {
            return Err(bad());
        }
        let year: i32 = y.parse().map_err(|_| bad())?;
        let month: u8 = m.parse().map_err(|_| bad())?;
        if !(1..=12).contains(&month) || !(1..=9999).contains(&year) {
            return Err(bad());
        }
        Ok(Self { year, month })
    }
}

impl TryFrom<String> for Month {
    type Error = CommandError;
    fn try_from(s: String) -> CommandResult<Self> {
        s.parse()
    }
}

impl From<Month> for String {
    fn from(m: Month) -> Self {
        m.to_string()
    }
}

impl JsonSchema for Month {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Month".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type": "string", "pattern": r"^\d{4}-\d{2}$", "description": "Calendar month, YYYY-MM"})
    }
}

fn overrides_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": ["object", "null"],
        "additionalProperties": {"type": "string", "pattern": r"^[+-]?\d+(\.\d+)?$"},
        "description": "Month (YYYY-MM) → amount for that month"
    })
}

// ----- stored shapes ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Line {
    account: AccountId,
    amount: Decimal,
    #[serde(default)]
    overrides: BTreeMap<Month, Decimal>,
    #[serde(default)]
    rollover: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Budget {
    id: String,
    name: String,
    start_month: Month,
    default: bool,
    #[serde(default)]
    lines: Vec<Line>,
    created_at: String,
}

impl Budget {
    fn month_budget(&self, line: &Line, m: Month) -> Decimal {
        if m < self.start_month {
            return Decimal::ZERO;
        }
        line.overrides.get(&m).copied().unwrap_or(line.amount)
    }
}

fn bad(e: impl fmt::Display) -> CommandError {
    CommandError::Integrity(format!("bad stored budget: {e}"))
}

fn load_all(ctx: &Ctx<'_>) -> CommandResult<Vec<Budget>> {
    let mut out: Vec<Budget> = ctx
        .tx
        .docs(DocTable::Budget, None)?
        .into_iter()
        .map(|d| serde_json::from_value(d.json).map_err(bad))
        .collect::<CommandResult<_>>()?;
    out.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

/// A budget by id or name; the default one when `reference` is `None`.
fn find(ctx: &Ctx<'_>, reference: Option<&str>) -> CommandResult<Budget> {
    let all = load_all(ctx)?;
    match reference.map(str::trim) {
        Some(r) => {
            let lower = r.to_lowercase();
            all.into_iter()
                .find(|b| b.id == r || b.name.to_lowercase() == lower)
                .ok_or_else(|| CommandError::NotFound(format!("no budget {r:?}")))
        }
        None => all
            .into_iter()
            .find(|b| b.default)
            .ok_or_else(|| CommandError::NotFound("there is no budget yet".into())),
    }
}

fn save(ctx: &Ctx<'_>, b: &Budget) -> CommandResult<()> {
    ctx.save_doc(
        DocTable::Budget,
        &b.id.clone(),
        Some(&Doc {
            id: b.id.clone(),
            grp: None,
            ord: 0,
            json: serde_json::to_value(b)?,
        }),
    )
}

fn base_dp(ctx: &Ctx<'_>) -> CommandResult<u32> {
    Ok(minor_units(&ctx.tx.info()?.base_currency))
}

fn money(v: Decimal, dp: u32) -> String {
    format_amount(v, Some(dp))
}

fn budget_json(chart: &Chart, b: &Budget, dp: u32) -> Value {
    let lines: Vec<Value> = b
        .lines
        .iter()
        .map(|l| {
            json!({
                "account": l.account,
                "full_name": chart.full_name(l.account),
                "amount": money(l.amount, dp),
                "overrides": l.overrides.iter().map(|(m, v)| (m.to_string(), json!(money(*v, dp)))).collect::<serde_json::Map<_, _>>(),
                "rollover": l.rollover,
            })
        })
        .collect();
    json!({
        "id": b.id,
        "name": b.name,
        "start_month": b.start_month,
        "default": b.default,
        "lines": lines,
        "created_at": b.created_at,
    })
}

fn respond(ctx: &Ctx<'_>, b: &Budget) -> CommandResult<Value> {
    Ok(budget_json(&Chart::load(ctx)?, b, base_dp(ctx)?))
}

// ----- line inputs -----------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LineInput {
    /// Income or expense account (id, full name or code).
    account: String,
    /// Default monthly amount, ≥ 0.
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
    /// Month → amount, replacing `amount` for that month (replaced entirely when given).
    #[serde(default)]
    #[schemars(schema_with = "overrides_schema")]
    overrides: Option<BTreeMap<Month, Decimal>>,
    #[serde(default)]
    rollover: Option<bool>,
}

fn check_amount(v: Decimal, dp: u32) -> CommandResult<Decimal> {
    if v.is_sign_negative() && !v.is_zero() {
        return Err(CommandError::InvalidArgs(format!(
            "budget amounts are positive numbers; got {v}"
        )));
    }
    if v.scale() > dp && v.round_dp(dp) != v {
        return Err(CommandError::InvalidArgs(format!(
            "{v} has more than {dp} decimal places"
        )));
    }
    Ok(v.round_dp(dp))
}

fn budget_account(chart: &Chart, r: &str) -> CommandResult<AccountId> {
    let a = chart.resolve(r)?;
    if !matches!(a.account_type, AccountType::Income | AccountType::Expense) {
        return Err(CommandError::Rejected(format!(
            "{} is not an income or expense account; budgets cover categories only",
            chart.full_name(a.id)
        )));
    }
    Ok(a.id)
}

/// Create or replace a line from input; absent `overrides`/`rollover` keep the
/// existing line's values.
fn put_line(b: &mut Budget, chart: &Chart, input: LineInput, dp: u32) -> CommandResult<()> {
    let account = budget_account(chart, &input.account)?;
    let amount = check_amount(input.amount, dp)?;
    let existing = b.lines.iter().position(|l| l.account == account);
    let old = existing.and_then(|i| b.lines.get(i)).cloned();
    let overrides = match input.overrides {
        Some(o) => o
            .into_iter()
            .map(|(m, v)| Ok((m, check_amount(v, dp)?)))
            .collect::<CommandResult<_>>()?,
        None => old
            .as_ref()
            .map(|l| l.overrides.clone())
            .unwrap_or_default(),
    };
    let line = Line {
        account,
        amount,
        overrides,
        rollover: input
            .rollover
            .unwrap_or_else(|| old.as_ref().is_some_and(|l| l.rollover)),
    };
    match existing.and_then(|i| b.lines.get_mut(i)) {
        Some(l) => *l = line,
        None => b.lines.push(line),
    }
    Ok(())
}

// ----- commands --------------------------------------------------------------------

fn check_name(ctx: &Ctx<'_>, name: &str, except: Option<&str>) -> CommandResult<String> {
    let name = name.trim().to_owned();
    if !(1..=100).contains(&name.chars().count()) {
        return Err(CommandError::InvalidArgs(
            "a budget name has 1–100 characters".into(),
        ));
    }
    let lower = name.to_lowercase();
    if load_all(ctx)?
        .iter()
        .any(|b| Some(b.id.as_str()) != except && b.name.to_lowercase() == lower)
    {
        return Err(CommandError::Rejected(format!(
            "a budget named {name:?} already exists"
        )));
    }
    Ok(name)
}

fn current_month(today: Option<Date>) -> CommandResult<Month> {
    Ok(Month::of(match today {
        Some(d) => d,
        None => today_utc()?,
    }))
}

/// Make `id` the only default budget.
fn make_default(ctx: &Ctx<'_>, id: &str) -> CommandResult<()> {
    for mut other in load_all(ctx)? {
        if other.id != id && other.default {
            other.default = false;
            save(ctx, &other)?;
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    name: String,
    /// Default: the current month.
    #[serde(default)]
    start_month: Option<Month>,
    #[serde(default)]
    lines: Vec<LineInput>,
    /// Make this the default budget (the first budget always is).
    #[serde(default)]
    default: bool,
    /// The current date (default: the local date of the machine).
    #[serde(default)]
    today: Option<Date>,
}

fn create(ctx: &Ctx<'_>, a: CreateArgs) -> CommandResult<Value> {
    let name = check_name(ctx, &a.name, None)?;
    let first = load_all(ctx)?.is_empty();
    let chart = Chart::load(ctx)?;
    let dp = base_dp(ctx)?;
    let mut b = Budget {
        id: format!("bud_{}", mk_core::Id::<()>::new()),
        name,
        start_month: match a.start_month {
            Some(m) => m,
            None => current_month(a.today)?,
        },
        default: first || a.default,
        lines: Vec::new(),
        created_at: now_rfc3339()?,
    };
    for l in a.lines {
        let account = budget_account(&chart, &l.account)?;
        if b.lines.iter().any(|x| x.account == account) {
            return Err(CommandError::InvalidArgs(format!(
                "{} has two lines; give one line per account",
                chart.full_name(account)
            )));
        }
        put_line(&mut b, &chart, l, dp)?;
    }
    if b.default {
        make_default(ctx, &b.id)?;
    }
    save(ctx, &b)?;
    respond(ctx, &b)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    /// Budget id or name (default: the default budget).
    #[serde(default)]
    budget: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    start_month: Option<Month>,
    /// true makes it the default (clearing the previous one).
    #[serde(default)]
    default: Option<bool>,
}

fn update(ctx: &Ctx<'_>, a: UpdateArgs) -> CommandResult<Value> {
    let old = find(ctx, a.budget.as_deref())?;
    let mut b = old.clone();
    if let Some(n) = a.name {
        b.name = check_name(ctx, &n, Some(&b.id))?;
    }
    if let Some(m) = a.start_month {
        b.start_month = m;
    }
    match a.default {
        Some(true) if !b.default => {
            make_default(ctx, &b.id)?;
            b.default = true;
        }
        Some(false) if b.default => {
            return Err(CommandError::Rejected(
                "one budget is always the default; make another budget the default instead".into(),
            ));
        }
        _ => {}
    }
    if b != old {
        save(ctx, &b)?;
    }
    respond(ctx, &b)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RefArgs {
    /// Budget id or name (default: the default budget).
    #[serde(default)]
    budget: Option<String>,
}

fn delete(ctx: &Ctx<'_>, a: RefArgs) -> CommandResult<Value> {
    let b = find(ctx, a.budget.as_deref())?;
    ctx.save_doc(DocTable::Budget, &b.id, None)?;
    if b.default
        && let Some(mut next) = load_all(ctx)?.into_iter().next()
    {
        next.default = true;
        save(ctx, &next)?;
    }
    Ok(json!({"deleted": b.id, "name": b.name}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

fn list(ctx: &Ctx<'_>, _: NoArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let dp = base_dp(ctx)?;
    Ok(Value::Array(
        load_all(ctx)?
            .iter()
            .map(|b| budget_json(&chart, b, dp))
            .collect(),
    ))
}

fn get(ctx: &Ctx<'_>, a: RefArgs) -> CommandResult<Value> {
    let b = find(ctx, a.budget.as_deref())?;
    respond(ctx, &b)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetLineArgs {
    /// Budget id or name (default: the default budget).
    #[serde(default)]
    budget: Option<String>,
    account: String,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
    #[serde(default)]
    #[schemars(schema_with = "overrides_schema")]
    overrides: Option<BTreeMap<Month, Decimal>>,
    #[serde(default)]
    rollover: Option<bool>,
}

fn set_line(ctx: &Ctx<'_>, a: SetLineArgs) -> CommandResult<Value> {
    let mut b = find(ctx, a.budget.as_deref())?;
    let chart = Chart::load(ctx)?;
    put_line(
        &mut b,
        &chart,
        LineInput {
            account: a.account,
            amount: a.amount,
            overrides: a.overrides,
            rollover: a.rollover,
        },
        base_dp(ctx)?,
    )?;
    save(ctx, &b)?;
    respond(ctx, &b)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetMonthArgs {
    #[serde(default)]
    budget: Option<String>,
    account: String,
    month: Month,
    /// The month's amount; equal to the line's default removes the override.
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
}

fn line_mut<'a>(b: &'a mut Budget, chart: &Chart, account: &str) -> CommandResult<&'a mut Line> {
    let id = chart.resolve(account)?.id;
    let name = chart.full_name(id);
    b.lines
        .iter_mut()
        .find(|l| l.account == id)
        .ok_or_else(|| CommandError::NotFound(format!("the budget has no line for {name}")))
}

fn set_month(ctx: &Ctx<'_>, a: SetMonthArgs) -> CommandResult<Value> {
    let mut b = find(ctx, a.budget.as_deref())?;
    let chart = Chart::load(ctx)?;
    let amount = check_amount(a.amount, base_dp(ctx)?)?;
    let line = line_mut(&mut b, &chart, &a.account)?;
    if amount == line.amount {
        line.overrides.remove(&a.month);
    } else {
        line.overrides.insert(a.month, amount);
    }
    save(ctx, &b)?;
    respond(ctx, &b)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemoveLineArgs {
    #[serde(default)]
    budget: Option<String>,
    account: String,
}

fn remove_line(ctx: &Ctx<'_>, a: RemoveLineArgs) -> CommandResult<Value> {
    let mut b = find(ctx, a.budget.as_deref())?;
    let chart = Chart::load(ctx)?;
    line_mut(&mut b, &chart, &a.account)?;
    let id = chart.resolve(&a.account)?.id;
    b.lines.retain(|l| l.account != id);
    save(ctx, &b)?;
    respond(ctx, &b)
}

// ----- actuals ---------------------------------------------------------------------

/// Display-signed monthly sums per income/expense account (own splits only), in the
/// base currency, for posted transactions in `months`; plus accounts left out for
/// being in another commodity.
struct Actuals {
    by_account: HashMap<AccountId, BTreeMap<Month, Decimal>>,
    excluded: Vec<AccountId>,
}

fn actuals(ctx: &Ctx<'_>, chart: &Chart, first: Month, last: Month) -> CommandResult<Actuals> {
    let base = ctx.tx.info()?.base_currency;
    let mut by_account: HashMap<AccountId, BTreeMap<Month, Decimal>> = HashMap::new();
    let mut excluded: Vec<AccountId> = Vec::new();
    for row in ctx.tx.posted_splits(None)? {
        let m = Month::of(row.date);
        if m < first || m > last {
            continue;
        }
        let Some(acc) = chart.get(row.account) else {
            continue;
        };
        if !matches!(acc.account_type, AccountType::Income | AccountType::Expense) {
            continue;
        }
        if acc.commodity.as_str() != base {
            if !excluded.contains(&acc.id) {
                excluded.push(acc.id);
            }
            continue;
        }
        let v = acc.account_type.display(row.quantity);
        let slot = by_account.entry(acc.id).or_default().entry(m).or_default();
        *slot = slot.checked_add(v).unwrap_or(*slot);
    }
    Ok(Actuals {
        by_account,
        excluded,
    })
}

/// The budget line each account's actuals count toward: the nearest line on the
/// account itself or an ancestor.
fn owners(chart: &Chart, b: &Budget) -> HashMap<AccountId, AccountId> {
    let lined: HashSet<AccountId> = b.lines.iter().map(|l| l.account).collect();
    let mut out = HashMap::new();
    for a in &chart.accounts {
        let mut cur = Some(a.id);
        let mut steps = 0;
        while let Some(c) = cur {
            if lined.contains(&c) {
                out.insert(a.id, c);
                break;
            }
            cur = chart.get(c).and_then(|x| x.parent);
            steps += 1;
            if steps > chart.accounts.len() {
                break;
            }
        }
    }
    out
}

fn sum(values: impl IntoIterator<Item = Decimal>) -> Decimal {
    values
        .into_iter()
        .fold(Decimal::ZERO, |t, v| t.checked_add(v).unwrap_or(t))
}

fn sub(a: Decimal, b: Decimal) -> Decimal {
    a.checked_sub(b).unwrap_or(a)
}

/// Percent with one decimal, rounded half away from zero; `None` when `of` is zero.
fn percent(part: Decimal, of: Decimal) -> Option<Decimal> {
    if of.is_zero() {
        return None;
    }
    part.checked_mul(Decimal::ONE_HUNDRED)
        .and_then(|x| x.checked_div(of))
        .map(|x| x.round_dp_with_strategy(1, RoundingStrategy::MidpointAwayFromZero))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SuggestArgs {
    #[serde(default)]
    budget: Option<String>,
    /// Default: 12 months before the current month.
    #[serde(default)]
    from_month: Option<Month>,
    /// Default: the month before the current month.
    #[serde(default)]
    to_month: Option<Month>,
    #[serde(default)]
    method: Method,
    /// Only these accounts (each counted with its sub-accounts).
    #[serde(default)]
    accounts: Option<Vec<String>>,
    /// The current date (default: the local date of the machine).
    #[serde(default)]
    today: Option<Date>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Method {
    #[default]
    Average,
    Last,
}

fn suggest(ctx: &Ctx<'_>, a: SuggestArgs) -> CommandResult<Value> {
    if let Some(r) = &a.budget {
        find(ctx, Some(r))?;
    }
    let now = current_month(a.today)?;
    let bad_range = || CommandError::InvalidArgs("month out of range".into());
    let to = match a.to_month {
        Some(m) => m,
        None => now.shift(-1).ok_or_else(bad_range)?,
    };
    let from = match a.from_month {
        Some(m) => m,
        None => now.shift(-12).ok_or_else(bad_range)?,
    };
    if from > to {
        return Err(CommandError::InvalidArgs(
            "from_month is after to_month".into(),
        ));
    }
    let chart = Chart::load(ctx)?;
    let act = actuals(ctx, &chart, from, to)?;
    let months = from.through(to);
    let dp = base_dp(ctx)?;
    // Each target account with the accounts whose actuals it sums.
    let targets: Vec<(AccountId, Vec<AccountId>)> = match &a.accounts {
        Some(list) => list
            .iter()
            .map(|r| {
                let id = budget_account(&chart, r)?;
                Ok((id, chart.subtree(id)))
            })
            .collect::<CommandResult<_>>()?,
        None => chart
            .tree_order()
            .into_iter()
            .map(|(acc, _)| acc)
            .filter(|acc| {
                matches!(acc.account_type, AccountType::Income | AccountType::Expense)
                    && chart.children(acc.id).next().is_none()
                    && act.by_account.contains_key(&acc.id)
            })
            .map(|acc| (acc.id, vec![acc.id]))
            .collect(),
    };
    let mut out = Vec::new();
    for (id, members) in targets {
        let monthly: Vec<Decimal> = months
            .iter()
            .map(|m| {
                sum(members
                    .iter()
                    .filter_map(|x| act.by_account.get(x).and_then(|v| v.get(m)).copied()))
            })
            .collect();
        let raw = match a.method {
            Method::Average => {
                let n = Decimal::from(monthly.len().max(1));
                sum(monthly.iter().copied())
                    .checked_div(n)
                    .unwrap_or_default()
            }
            Method::Last => monthly.last().copied().unwrap_or_default(),
        };
        let amount = raw.ceil().max(Decimal::ZERO);
        let acc_type = chart.get(id).map(|x| x.account_type);
        out.push(json!({
            "account": id,
            "full_name": chart.full_name(id),
            "type": acc_type,
            "amount": money(amount, dp),
        }));
    }
    Ok(json!({
        "from_month": from,
        "to_month": to,
        "method": match a.method { Method::Average => "average", Method::Last => "last" },
        "lines": out,
    }))
}

/// Lines as `budget.suggest` returns them (extra fields such as `full_name` are
/// ignored).
#[derive(Debug, Deserialize, JsonSchema)]
struct LooseLine {
    account: String,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ApplyArgs {
    #[serde(default)]
    budget: Option<String>,
    /// Lines from `budget.suggest`, possibly edited.
    lines: Vec<LooseLine>,
}

fn apply_suggestion(ctx: &Ctx<'_>, a: ApplyArgs) -> CommandResult<Value> {
    let mut b = find(ctx, a.budget.as_deref())?;
    let chart = Chart::load(ctx)?;
    let dp = base_dp(ctx)?;
    for l in a.lines {
        put_line(
            &mut b,
            &chart,
            LineInput {
                account: l.account,
                amount: l.amount,
                overrides: None,
                rollover: None,
            },
            dp,
        )?;
    }
    save(ctx, &b)?;
    respond(ctx, &b)
}

// ----- report ----------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReportArgs {
    #[serde(default)]
    budget: Option<String>,
    /// One month (default: the current month).
    #[serde(default)]
    month: Option<Month>,
    /// A range of months (inclusive), instead of `month`.
    #[serde(default)]
    from_month: Option<Month>,
    #[serde(default)]
    to_month: Option<Month>,
    /// The current date (default: the local date of the machine).
    #[serde(default)]
    today: Option<Date>,
}

#[derive(Default, Clone, Copy)]
struct Totals {
    budget: Decimal,
    available: Decimal,
    actual: Decimal,
}

impl Totals {
    fn add(&mut self, o: Totals) {
        self.budget = sum([self.budget, o.budget]);
        self.available = sum([self.available, o.available]);
        self.actual = sum([self.actual, o.actual]);
    }
}

fn report(ctx: &Ctx<'_>, a: ReportArgs) -> CommandResult<Value> {
    let b = find(ctx, a.budget.as_deref())?;
    let (first, last) = match (a.from_month, a.to_month, a.month) {
        (Some(f), Some(t), None) => (f, t),
        (None, None, Some(m)) => (m, m),
        (None, None, None) => {
            let m = current_month(a.today)?;
            (m, m)
        }
        _ => {
            return Err(CommandError::InvalidArgs(
                "give month, or from_month and to_month".into(),
            ));
        }
    };
    if first > last {
        return Err(CommandError::InvalidArgs(
            "from_month is after to_month".into(),
        ));
    }
    let chart = Chart::load(ctx)?;
    let dp = base_dp(ctx)?;
    // Rollover needs actuals from the start month on.
    let earliest = first.min(b.start_month);
    let act = actuals(ctx, &chart, earliest, last)?;
    let owner = owners(&chart, &b);
    // Monthly actuals per line (own account plus unlined descendants).
    let mut line_actual: HashMap<AccountId, BTreeMap<Month, Decimal>> = HashMap::new();
    for (acc, months) in &act.by_account {
        if let Some(line) = owner.get(acc) {
            let slot = line_actual.entry(*line).or_default();
            for (m, v) in months {
                let e = slot.entry(*m).or_default();
                *e = sum([*e, *v]);
            }
        }
    }
    let period = first.through(last);
    let depth: HashMap<AccountId, usize> = chart
        .tree_order()
        .into_iter()
        .map(|(acc, d)| (acc.id, d))
        .collect();
    let order: Vec<AccountId> = chart.tree_order().into_iter().map(|(x, _)| x.id).collect();
    let mut lines: Vec<&Line> = b.lines.iter().collect();
    lines.sort_by_key(|l| {
        order
            .iter()
            .position(|x| *x == l.account)
            .unwrap_or(usize::MAX)
    });

    let mut sections: [(Vec<Value>, Totals); 2] = [
        (Vec::new(), Totals::default()),
        (Vec::new(), Totals::default()),
    ];
    for line in lines {
        let Some(acc) = chart.get(line.account) else {
            continue;
        };
        let idx = usize::from(acc.account_type == AccountType::Expense);
        let actual_in = |m: &Month| {
            line_actual
                .get(&line.account)
                .and_then(|v| v.get(m))
                .copied()
                .unwrap_or_default()
        };
        let budget = sum(period.iter().map(|m| b.month_budget(line, *m)));
        let actual = sum(period.iter().map(actual_in));
        let carried = if line.rollover && first > b.start_month {
            sum(b
                .start_month
                .through(first.shift(-1).unwrap_or(first))
                .iter()
                .map(|m| sub(b.month_budget(line, *m), actual_in(m))))
        } else {
            Decimal::ZERO
        };
        let available = sum([budget, carried]);
        let remaining = sub(available, actual);
        let mut row = json!({
            "account": {"id": acc.id, "full_name": chart.full_name(acc.id), "depth": depth.get(&acc.id).copied().unwrap_or(0)},
            "budget": money(budget, dp),
            "available": money(available, dp),
            "actual": money(actual, dp),
            "remaining": money(remaining, dp),
            "rollover": line.rollover,
            "carried": money(carried, dp),
            "default_amount": money(line.amount, dp),
        });
        if let Some(o) = row.as_object_mut() {
            if let Some(p) = percent(actual, available) {
                o.insert("percent".into(), json!(p.to_string()));
            }
            if last < b.start_month {
                o.insert(
                    "note".into(),
                    json!(format!("This budget starts in {}", b.start_month)),
                );
            }
            if first == last {
                o.insert(
                    "month_amount".into(),
                    json!(money(b.month_budget(line, first), dp)),
                );
            }
        }
        if let Some((rows, totals)) = sections.get_mut(idx) {
            rows.push(row);
            totals.add(Totals {
                budget,
                available,
                actual,
            });
        }
    }
    let section = |(rows, t): &(Vec<Value>, Totals)| {
        json!({
            "rows": rows,
            "totals": {
                "budget": money(t.budget, dp),
                "available": money(t.available, dp),
                "actual": money(t.actual, dp),
                "remaining": money(sub(t.available, t.actual), dp),
            }
        })
    };
    let [inc, exp] = &sections;
    // Accounts with activity that no line covers.
    let mut unbudgeted = Vec::new();
    let mut unbudgeted_total = [Decimal::ZERO, Decimal::ZERO];
    for id in &order {
        if owner.contains_key(id) {
            continue;
        }
        let Some(months) = act.by_account.get(id) else {
            continue;
        };
        let v = sum(period.iter().filter_map(|m| months.get(m).copied()));
        if v.is_zero() {
            continue;
        }
        let Some(acc) = chart.get(*id) else { continue };
        let is_exp = acc.account_type == AccountType::Expense;
        if let Some(t) = unbudgeted_total.get_mut(usize::from(is_exp)) {
            *t = sum([*t, v]);
        }
        unbudgeted.push(json!({
            "account": id,
            "full_name": chart.full_name(*id),
            "type": acc.account_type,
            "actual": money(v, dp),
        }));
    }
    let [ui, ue] = unbudgeted_total;
    Ok(json!({
        "budget": {"id": b.id, "name": b.name, "start_month": b.start_month},
        "from_month": first,
        "to_month": last,
        "income": section(inc),
        "expenses": section(exp),
        "net": {
            "budget": money(sub(inc.1.budget, exp.1.budget), dp),
            "available": money(sub(inc.1.available, exp.1.available), dp),
            "actual": money(sub(inc.1.actual, exp.1.actual), dp),
        },
        "unbudgeted": unbudgeted,
        "unbudgeted_total": {"income": money(ui, dp), "expenses": money(ue, dp)},
        "excluded": act.excluded.iter().map(|id| json!({
            "account": id, "full_name": chart.full_name(*id), "reason": "not in base currency"
        })).collect::<Vec<_>>(),
    }))
}
