//! Projects (projects.md): project records with a budget (total or by account), an
//! optional customer, fund, billable default and markup; the project tag on splits,
//! documents, trips and expenses (the tag rules are in `dimensions.rs`); bulk Set
//! project; the project and projects-summary reports; budget alerts and the Projects
//! dashboard block; and billable costs put onto invoices.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mk_core::{AccountId, AccountType, Date, Decimal, Transaction, TransactionId, TxnStatus};
use mk_store::{Doc, DocTable, now_rfc3339};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::sales::{money, round2};
use crate::txns::validate_change;
use crate::{CommandError, CommandResult, Ctx, Registry, is_module_enabled};

pub(crate) const MODULE: &str = "projects";
const DISMISSED_KEY: &str = "dismissed_alerts";
/// The income account billable costs post to when the setting names none.
pub(crate) const BILLABLE_INCOME: &str = "Billable Expense Income";

const STATUSES: [(&str, &str); 5] = [
    ("planned", "Planned"),
    ("active", "Active"),
    ("on_hold", "On hold"),
    ("completed", "Completed"),
    ("cancelled", "Cancelled"),
];

pub(crate) fn register(r: &mut Registry) {
    let m = MODULE;
    r.add_in(m, "project.create", "Create a project", true, create);
    r.add_in(m, "project.update", "Change a project", true, update);
    r.add_in(
        m,
        "project.list",
        "Projects with budget, spent, % used, income, net, unbilled costs and miles",
        false,
        list,
    );
    r.add_in(
        m,
        "project.get",
        "One project: fields, summary (income, expenses by account, mileage, budget vs actual), tagged transactions, trips, documents and billable costs",
        false,
        get,
    );
    r.add_in(
        m,
        "project.set_status",
        "Change a project's status (planned, active, on_hold, completed, cancelled)",
        true,
        set_status,
    );
    r.add_in(
        m,
        "project.delete",
        "Delete a project that was never used (otherwise complete or cancel it)",
        true,
        delete,
    );
    r.add_in(
        m,
        "project.set_fund_on_tagged",
        "Set the project's fund on its tagged splits that have no fund",
        true,
        set_fund_on_tagged,
    );
    r.add_in(
        m,
        "txn.set_project",
        "Set (or clear) the project on the income and expense lines of several transactions in one undoable change",
        true,
        set_project,
    );
    r.add_in(
        m,
        "project.unbilled",
        "Billable costs not yet on an invoice, by project (optionally for one customer)",
        false,
        unbilled,
    );
    r.add_in(
        m,
        "project.billable_lines",
        "Invoice lines for chosen billable costs (one per item or one per expense account), with the project's markup; creates the billable income account on first use",
        true,
        billable_lines,
    );
    r.add_in(
        m,
        "project.alerts",
        "Budget alerts: projects at the warning percentage or over budget",
        false,
        alerts_cmd,
    );
    r.add_in(
        m,
        "project.dismiss_alert",
        "Dismiss a project budget alert",
        true,
        dismiss_alert,
    );
    r.add_in(
        m,
        "report.project",
        "One project (with its sub-projects): income and expenses by account, mileage, net, budget vs actual and remaining",
        false,
        report_project,
    );
    r.add_in(
        m,
        "report.projects_summary",
        "One row per project: budget, expenses, % used, income, net and miles",
        false,
        report_summary,
    );
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

fn rejected(m: impl Into<String>) -> CommandError {
    CommandError::Rejected(m.into())
}

fn at<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&Value::Null)
}

fn plus(a: Decimal, b: Decimal) -> Decimal {
    a.checked_add(b).unwrap_or(a)
}

fn minus(a: Decimal, b: Decimal) -> Decimal {
    a.checked_sub(b).unwrap_or(a)
}

// ----- records ------------------------------------------------------------------------

/// A budget line: an expense account (with its sub-accounts) or, with no account,
/// "Other" (every expense no other line covers).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BudgetLine {
    #[serde(default)]
    pub account: Option<AccountId>,
    pub amount: Decimal,
}

fn total_mode() -> String {
    "total".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Project {
    pub id: String,
    /// Unique; `:` separates sub-projects (`House:Roof repair`).
    pub name: String,
    pub status: String,
    /// A party id (who the work is for).
    #[serde(default)]
    pub customer: Option<String>,
    #[serde(default)]
    pub start: Option<Date>,
    #[serde(default)]
    pub end: Option<Date>,
    /// `total` or `by_account`.
    #[serde(default = "total_mode")]
    pub budget_mode: String,
    /// `total`: the budget.
    #[serde(default)]
    pub budget: Option<Decimal>,
    /// `by_account`: the lines (the budget is their sum).
    #[serde(default)]
    pub budget_lines: Vec<BudgetLine>,
    #[serde(default)]
    pub income_budget: Option<Decimal>,
    /// A fund id (module funds).
    #[serde(default)]
    pub fund: Option<String>,
    /// Default for new tagged expense lines (module invoicing).
    #[serde(default)]
    pub billable: bool,
    #[serde(default)]
    pub markup_percent: Decimal,
    /// Cents per mile billable trips are billed at (default: the trip's value).
    #[serde(default)]
    pub mileage_bill_rate: Option<Decimal>,
    #[serde(default)]
    pub notes: Option<String>,
    pub created_at: String,
}

impl Project {
    /// Completed and cancelled projects are closed: hidden from tag pickers.
    pub(crate) fn open(&self) -> bool {
        !matches!(self.status.as_str(), "completed" | "cancelled")
    }

    /// The project's own budget (by account: the sum of its lines).
    fn own_budget(&self) -> Option<Decimal> {
        if self.budget_mode == "by_account" {
            if self.budget_lines.is_empty() {
                None
            } else {
                Some(
                    self.budget_lines
                        .iter()
                        .fold(Decimal::ZERO, |a, l| plus(a, l.amount)),
                )
            }
        } else {
            self.budget
        }
    }
}

/// Every project, sorted by name.
pub(crate) fn all(ctx: &Ctx<'_>) -> CommandResult<Vec<Project>> {
    let mut v: Vec<Project> = ctx
        .tx
        .docs(DocTable::Project, None)?
        .into_iter()
        .map(|d| serde_json::from_value(d.json))
        .collect::<Result<_, _>>()?;
    v.sort_by_key(|p| p.name.to_lowercase());
    Ok(v)
}

/// A project by id or name.
pub(crate) fn find(ctx: &Ctx<'_>, r: &str) -> CommandResult<Project> {
    let r = r.trim();
    all(ctx)?
        .into_iter()
        .find(|p| p.id == r || p.name.eq_ignore_ascii_case(r))
        .ok_or_else(|| CommandError::NotFound(format!("project {r:?}")))
}

fn save(ctx: &Ctx<'_>, p: &Project) -> CommandResult<()> {
    ctx.save_doc(
        DocTable::Project,
        &p.id,
        Some(&Doc {
            id: p.id.clone(),
            grp: None,
            ord: 0,
            json: serde_json::to_value(p)?,
        }),
    )
}

/// The ids of `p` and its sub-projects.
fn family(projects: &[Project], p: &Project) -> BTreeSet<String> {
    let prefix = format!("{}:", p.name.to_lowercase());
    projects
        .iter()
        .filter(|x| x.id == p.id || x.name.to_lowercase().starts_with(&prefix))
        .map(|x| x.id.clone())
        .collect()
}

/// The budget a project's totals compare with: its own, or (a parent without one)
/// the sum of its sub-projects' budgets.
fn budget_of(projects: &[Project], p: &Project) -> Option<Decimal> {
    if let Some(b) = p.own_budget() {
        return Some(b);
    }
    let prefix = format!("{}:", p.name.to_lowercase());
    let subs: Vec<Decimal> = projects
        .iter()
        .filter(|x| x.name.to_lowercase().starts_with(&prefix))
        .filter_map(Project::own_budget)
        .collect();
    if subs.is_empty() {
        None
    } else {
        Some(subs.iter().fold(Decimal::ZERO, |a, b| plus(a, *b)))
    }
}

fn clean_name(name: &str) -> CommandResult<String> {
    let parts: Vec<String> = name
        .split(':')
        .map(|p| mk_core::text::clean(p, usize::MAX).text.trim().to_owned())
        .collect();
    if parts.iter().any(String::is_empty) {
        return Err(invalid("name: each level needs text (Parent:Child)"));
    }
    let n = parts.join(":");
    if n.chars().count() > 100 {
        return Err(invalid("name: at most 100 characters"));
    }
    Ok(n)
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BudgetLineArg {
    /// An expense account (id, full name or code); "" or absent: Other.
    #[serde(default)]
    account: Option<String>,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProjectArgs {
    /// project.update only: the project (id or name).
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// planned, active (default), on_hold, completed or cancelled.
    #[serde(default)]
    status: Option<String>,
    /// A customer (party id or name); "" clears.
    #[serde(default)]
    customer: Option<String>,
    /// YYYY-MM-DD; "" clears.
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
    /// total (default) or by_account. Switching to total keeps the lines' sum;
    /// switching to by_account starts with one line Other holding the total.
    #[serde(default)]
    budget_mode: Option<String>,
    /// total: the budget amount; "" clears.
    #[serde(default)]
    budget: Option<String>,
    /// by_account: the budget lines.
    #[serde(default)]
    budget_lines: Option<Vec<BudgetLineArg>>,
    /// Expected income; "" clears.
    #[serde(default)]
    income_budget: Option<String>,
    /// A fund (id or name; module funds); "" clears.
    #[serde(default)]
    fund: Option<String>,
    /// Default for new tagged expense lines (module invoicing).
    #[serde(default)]
    billable: Option<bool>,
    /// Markup on billable costs when invoiced, 0–1000 (%).
    #[serde(default)]
    markup_percent: Option<String>,
    /// Cents per mile for billable trips; "" clears (the trip's value is used).
    #[serde(default)]
    mileage_bill_rate: Option<String>,
    /// At most 2,000 characters.
    #[serde(default)]
    notes: Option<String>,
}

fn amount_arg(v: &str, what: &str) -> CommandResult<Option<Decimal>> {
    let t = v.trim().replace(',', "");
    if t.is_empty() {
        return Ok(None);
    }
    let d: Decimal = t
        .parse()
        .map_err(|_| invalid(format!("{what}: not an amount ({v:?})")))?;
    if d.is_sign_negative() || d.scale() > 2 {
        return Err(invalid(format!(
            "{what}: zero or more, with at most 2 decimals"
        )));
    }
    Ok(Some(d))
}

fn check_line_amount(d: Decimal) -> CommandResult<Decimal> {
    if d.is_sign_negative() || d.normalize().scale() > 2 {
        return Err(invalid(
            "budget line: an amount of zero or more, with at most 2 decimals",
        ));
    }
    Ok(round2(d))
}

fn date_arg(v: &str, what: &str) -> CommandResult<Option<Date>> {
    let t = v.trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse()
        .map(Some)
        .map_err(|_| invalid(format!("{what}: a date YYYY-MM-DD")))
}

fn apply(ctx: &Ctx<'_>, p: &mut Project, a: ProjectArgs) -> CommandResult<()> {
    if let Some(n) = a.name {
        let n = clean_name(&n)?;
        if all(ctx)?
            .iter()
            .any(|x| x.id != p.id && x.name.eq_ignore_ascii_case(&n))
        {
            return Err(rejected(format!("a project named {n:?} already exists")));
        }
        p.name = n;
    }
    if let Some(s) = a.status {
        let s = s.trim().to_lowercase();
        if !STATUSES.iter().any(|x| x.0 == s) {
            return Err(invalid(
                "status is planned, active, on_hold, completed or cancelled",
            ));
        }
        p.status = s;
    }
    if let Some(c) = a.customer {
        p.customer = match c.trim() {
            "" => None,
            c => Some(crate::sales::find_party(ctx, c)?.id),
        };
    }
    if let Some(d) = a.start {
        p.start = date_arg(&d, "start")?;
    }
    if let Some(d) = a.end {
        p.end = date_arg(&d, "end")?;
    }
    if let (Some(s), Some(e)) = (p.start, p.end)
        && e < s
    {
        return Err(invalid("the end date is before the start date"));
    }
    if let Some(m) = a.budget_mode {
        let m = m.trim().to_lowercase();
        if m != "total" && m != "by_account" {
            return Err(invalid("budget_mode is total or by_account"));
        }
        if m != p.budget_mode {
            if m == "total" {
                // Switching from lines to a total keeps the sum.
                p.budget = p.own_budget();
                p.budget_lines.clear();
            } else {
                // From a total to lines: one line Other holding the total.
                p.budget_lines = p
                    .budget
                    .map(|b| {
                        vec![BudgetLine {
                            account: None,
                            amount: b,
                        }]
                    })
                    .unwrap_or_default();
                p.budget = None;
            }
            p.budget_mode = m;
        }
    }
    if let Some(b) = a.budget {
        if p.budget_mode == "by_account" {
            return Err(invalid(
                "a budget by account takes budget_lines, not budget",
            ));
        }
        p.budget = amount_arg(&b, "budget")?;
    }
    if let Some(lines) = a.budget_lines {
        if p.budget_mode != "by_account" {
            return Err(invalid(
                "budget_lines need budget_mode by_account (or give budget)",
            ));
        }
        if lines.len() > 100 {
            return Err(invalid("at most 100 budget lines"));
        }
        let chart = Chart::load(ctx)?;
        let mut out: Vec<BudgetLine> = Vec::new();
        for l in lines {
            let account = match l.account.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(r) => {
                    let acc = chart.resolve(r)?;
                    if acc.account_type != AccountType::Expense {
                        return Err(invalid(format!(
                            "budget line: {} is not an expense account",
                            chart.full_name(acc.id)
                        )));
                    }
                    Some(acc.id)
                }
            };
            if out.iter().any(|x| x.account == account) {
                return Err(invalid("each budget line names a different account"));
            }
            out.push(BudgetLine {
                account,
                amount: check_line_amount(l.amount)?,
            });
        }
        p.budget_lines = out;
    }
    if let Some(b) = a.income_budget {
        p.income_budget = amount_arg(&b, "income_budget")?;
    }
    if let Some(f) = a.fund {
        p.fund = match f.trim() {
            "" => None,
            f => {
                if !is_module_enabled(ctx.tx, crate::dimensions::FUNDS)? {
                    return Err(invalid("a project's fund needs the funds feature on"));
                }
                crate::dimensions::resolve_tags(ctx, None, None, Some(f))?.fund
            }
        };
    }
    if let Some(b) = a.billable {
        if b && !is_module_enabled(ctx.tx, crate::sales::INVOICING)? {
            return Err(invalid("billable costs need the invoicing feature on"));
        }
        p.billable = b;
    }
    if let Some(m) = a.markup_percent {
        let d = amount_arg(&m, "markup_percent")?.unwrap_or_default();
        if d > Decimal::from(1000) {
            return Err(invalid("markup_percent: at most 1,000"));
        }
        p.markup_percent = d;
    }
    if let Some(r) = a.mileage_bill_rate {
        p.mileage_bill_rate = match r.trim() {
            "" => None,
            s => Some(crate::mileage::check_cents(s)?),
        };
    }
    if let Some(n) = a.notes {
        let n = mk_core::text::clean(&n, usize::MAX).text.trim().to_owned();
        if n.chars().count() > 2000 {
            return Err(invalid("notes: at most 2,000 characters"));
        }
        p.notes = Some(n).filter(|x| !x.is_empty());
    }
    Ok(())
}

fn create(ctx: &Ctx<'_>, a: ProjectArgs) -> CommandResult<Value> {
    if a.name.is_none() {
        return Err(invalid("give the name"));
    }
    if a.project.is_some() {
        return Err(invalid("project is for project.update"));
    }
    let mut p = Project {
        id: format!("prj_{}", mk_core::Id::<()>::new()),
        name: String::new(),
        status: "active".into(),
        customer: None,
        start: None,
        end: None,
        budget_mode: total_mode(),
        budget: None,
        budget_lines: Vec::new(),
        income_budget: None,
        fund: None,
        billable: false,
        markup_percent: Decimal::ZERO,
        mileage_bill_rate: None,
        notes: None,
        created_at: now_rfc3339()?,
    };
    apply(ctx, &mut p, a)?;
    save(ctx, &p)?;
    project_json(ctx, &p)
}

fn update(ctx: &Ctx<'_>, mut a: ProjectArgs) -> CommandResult<Value> {
    let key = a
        .project
        .take()
        .ok_or_else(|| invalid("give the project"))?;
    let old = find(ctx, &key)?;
    let mut p = old.clone();
    apply(ctx, &mut p, a)?;
    if p != old {
        save(ctx, &p)?;
    }
    project_json(ctx, &p)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    project: String,
    status: String,
}

fn set_status(ctx: &Ctx<'_>, a: StatusArgs) -> CommandResult<Value> {
    update(
        ctx,
        ProjectArgs {
            project: Some(a.project),
            status: Some(a.status),
            ..ProjectArgs::default()
        },
    )
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RefArgs {
    /// Id or name.
    project: String,
}

/// Where a project is used: tagged splits, and other documents naming it.
fn usage(ctx: &Ctx<'_>, id: &str) -> CommandResult<(usize, usize)> {
    let splits = all_txns(ctx)?
        .iter()
        .flat_map(|t| t.splits.iter())
        .filter(|s| s.tags.project.as_deref() == Some(id))
        .count();
    let needle = format!("\"{id}\"");
    let mut docs = 0;
    for t in [
        DocTable::SalesDoc,
        DocTable::BillDoc,
        DocTable::Rule,
        DocTable::Schedule,
        DocTable::Trip,
        DocTable::Expense,
    ] {
        docs += ctx
            .tx
            .docs(t, None)?
            .iter()
            .filter(|d| d.json.to_string().contains(&needle))
            .count();
    }
    Ok((splits, docs))
}

fn delete(ctx: &Ctx<'_>, a: RefArgs) -> CommandResult<Value> {
    let p = find(ctx, &a.project)?;
    let (splits, docs) = usage(ctx, &p.id)?;
    if splits > 0 {
        return Err(rejected(format!(
            "{} is used on {splits} tagged split{}; mark it completed or cancelled instead",
            p.name,
            if splits == 1 { "" } else { "s" }
        )));
    }
    if docs > 0 {
        return Err(rejected(format!(
            "{} is used on {docs} document{}, rule{} or trip{}; mark it completed or cancelled instead",
            p.name,
            if docs == 1 { "" } else { "s" },
            if docs == 1 { "" } else { "s" },
            if docs == 1 { "" } else { "s" },
        )));
    }
    let projects = all(ctx)?;
    if family(&projects, &p).len() > 1 {
        return Err(rejected(format!("{} has sub-projects", p.name)));
    }
    ctx.save_doc(DocTable::Project, &p.id, None)?;
    Ok(json!({"deleted": p.id, "name": p.name}))
}

fn all_txns(ctx: &Ctx<'_>) -> CommandResult<Vec<Transaction>> {
    Ok(ctx.tx.transactions(&mk_store::TxnFilter {
        accounts: None,
        from: None,
        to: None,
        include_voided: false,
        limit: None,
        offset: 0,
    })?)
}

fn posted(ctx: &Ctx<'_>) -> CommandResult<Vec<Transaction>> {
    Ok(all_txns(ctx)?
        .into_iter()
        .filter(|t| t.status == TxnStatus::Posted)
        .collect())
}

fn set_fund_on_tagged(ctx: &Ctx<'_>, a: RefArgs) -> CommandResult<Value> {
    let p = find(ctx, &a.project)?;
    let Some(fund) = p.fund.clone() else {
        return Err(rejected(format!("{} has no fund", p.name)));
    };
    let mut changed = 0;
    for before in all_txns(ctx)? {
        let mut after = before.clone();
        let mut n = 0;
        for s in &mut after.splits {
            if s.tags.project.as_deref() == Some(p.id.as_str()) && s.tags.fund.is_none() {
                s.tags.fund = Some(fund.clone());
                n += 1;
            }
        }
        if n > 0 {
            validate_change(ctx, Some(&before), Some(&after))?;
            ctx.save_txn(Some(&before), &after)?;
            changed += n;
        }
    }
    Ok(json!({"project": p.id, "changed": changed}))
}

/// How many splits tagged with the project have no fund (the project form's offer).
fn without_fund(txns: &[Transaction], id: &str) -> usize {
    txns.iter()
        .flat_map(|t| t.splits.iter())
        .filter(|s| s.tags.project.as_deref() == Some(id) && s.tags.fund.is_none())
        .count()
}

// ----- bulk Set project ------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetProjectArgs {
    /// Transaction ids.
    transactions: Vec<TransactionId>,
    /// The project (id or name); "" clears the project.
    project: String,
    /// Replace another project already on a line (after the user confirmed).
    #[serde(default)]
    replace: bool,
    /// Only count what would change (for the confirm); change nothing.
    #[serde(default)]
    preview: bool,
}

/// The lines Set project tags: income and expense lines, or every line of a
/// transaction that has none (a transfer; shown as not counted).
fn target_lines(chart: &Chart, t: &Transaction) -> Vec<usize> {
    let cat: Vec<usize> = t
        .splits
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            chart.get(s.account).is_some_and(|a| {
                matches!(a.account_type, AccountType::Income | AccountType::Expense)
            })
        })
        .map(|(i, _)| i)
        .collect();
    if cat.is_empty() {
        (0..t.splits.len()).collect()
    } else {
        cat
    }
}

fn set_project(ctx: &Ctx<'_>, a: SetProjectArgs) -> CommandResult<Value> {
    if a.transactions.is_empty() || a.transactions.len() > 5000 {
        return Err(invalid("give 1 to 5,000 transactions"));
    }
    let target = match a.project.trim() {
        "" => None,
        r => Some(find(ctx, r)?),
    };
    let target_id = target.as_ref().map(|p| p.id.clone());
    let funds_on = is_module_enabled(ctx.tx, crate::dimensions::FUNDS)?;
    let chart = Chart::load(ctx)?;
    let mut work: Vec<(Transaction, Transaction)> = Vec::new();
    let mut other = 0usize;
    let mut lines = 0usize;
    for id in &a.transactions {
        let before = ctx
            .tx
            .transaction(*id)?
            .ok_or_else(|| CommandError::NotFound(format!("transaction {id}")))?;
        let mut after = before.clone();
        for i in target_lines(&chart, &before) {
            let Some(s) = after.splits.get_mut(i) else {
                continue;
            };
            if s.tags.project == target_id {
                continue;
            }
            if s.tags.project.is_some() && target_id.is_some() {
                other += 1;
            }
            lines += 1;
            s.tags.project.clone_from(&target_id);
            let expense = chart
                .get(s.account)
                .is_some_and(|x| x.account_type == AccountType::Expense);
            s.tags.billable = target.as_ref().is_some_and(|p| p.billable && expense);
            if let Some(p) = &target
                && funds_on
                && s.tags.fund.is_none()
            {
                s.tags.fund.clone_from(&p.fund);
            }
        }
        if after != before {
            work.push((before, after));
        }
    }
    if a.preview {
        ctx.quiet_noop.set(true);
        return Ok(json!({"transactions": work.len(), "lines": lines, "replaces": other}));
    }
    if other > 0 && !a.replace {
        return Err(CommandError::Conflict(format!(
            "{other} line{} already {} another project; confirm to replace",
            if other == 1 { "" } else { "s" },
            if other == 1 { "has" } else { "have" }
        )));
    }
    for (before, after) in &work {
        validate_change(ctx, Some(before), Some(after))?;
        ctx.save_txn(Some(before), after)?;
    }
    Ok(json!({
        "project": target_id,
        "transactions": work.len(),
        "lines": lines,
        "replaced": other,
    }))
}

// ----- activity --------------------------------------------------------------------------

/// A tagged line.
struct Line {
    txn: TransactionId,
    idx: usize,
    date: Date,
    payee: String,
    memo: String,
    account: AccountId,
    project: String,
    /// `expense`, `income` or `transfer` (not counted).
    kind: &'static str,
    /// Expense: debit positive (a refund is negative). Income: credit positive.
    amount: Decimal,
    billable: bool,
}

fn lines_of(
    chart: &Chart,
    txns: &[Transaction],
    ids: &BTreeSet<String>,
    from: Option<Date>,
    to: Option<Date>,
) -> Vec<Line> {
    let mut out = Vec::new();
    for t in txns {
        if from.is_some_and(|f| t.date < f) || to.is_some_and(|x| t.date > x) {
            continue;
        }
        for (idx, s) in t.splits.iter().enumerate() {
            let Some(p) = s.tags.project.as_deref().filter(|p| ids.contains(*p)) else {
                continue;
            };
            let Some(acc) = chart.get(s.account) else {
                continue;
            };
            let (kind, amount) = match acc.account_type {
                AccountType::Expense => ("expense", s.value),
                AccountType::Income => ("income", -s.value),
                _ => ("transfer", s.value),
            };
            out.push(Line {
                txn: t.id,
                idx,
                date: t.date,
                payee: t.payee.clone().unwrap_or_default(),
                memo: s
                    .memo
                    .clone()
                    .or_else(|| t.memo.clone())
                    .unwrap_or_default(),
                account: s.account,
                project: p.to_owned(),
                kind,
                amount,
                billable: s.tags.billable,
            });
        }
    }
    out.sort_by_key(|l| (l.date, l.txn.to_string()));
    out
}

/// Trips tagged with the project (not drafts).
fn trips_of(
    ctx: &Ctx<'_>,
    ids: &BTreeSet<String>,
    from: Option<Date>,
    to: Option<Date>,
) -> CommandResult<Vec<crate::mileage::TripDoc>> {
    if !is_module_enabled(ctx.tx, crate::mileage::MODULE)? {
        return Ok(Vec::new());
    }
    let mut v: Vec<crate::mileage::TripDoc> = crate::mileage::all_trips(ctx)?
        .into_iter()
        .filter(|t| t.status != "draft")
        .filter(|t| t.tags.project.as_deref().is_some_and(|p| ids.contains(p)))
        .filter(|t| from.is_none_or(|f| t.date >= f) && to.is_none_or(|x| t.date <= x))
        .collect();
    v.sort_by_key(|t| (t.date, t.id.clone()));
    Ok(v)
}

/// Totals of one project (with sub-projects) over a period.
struct Totals {
    expenses: Decimal,
    income: Decimal,
    miles: Decimal,
    mileage_value: Decimal,
    /// The value of trips that are not posted as an expense (what "Count mileage as
    /// a cost" adds; posted reimbursements are already expenses).
    mileage_cost: Decimal,
}

fn totals(lines: &[Line], trips: &[crate::mileage::TripDoc]) -> Totals {
    let mut t = Totals {
        expenses: Decimal::ZERO,
        income: Decimal::ZERO,
        miles: Decimal::ZERO,
        mileage_value: Decimal::ZERO,
        mileage_cost: Decimal::ZERO,
    };
    for l in lines {
        match l.kind {
            "expense" => t.expenses = plus(t.expenses, l.amount),
            "income" => t.income = plus(t.income, l.amount),
            _ => {}
        }
    }
    for tr in trips {
        t.miles = plus(t.miles, tr.miles);
        t.mileage_value = plus(t.mileage_value, tr.amount);
        if tr.transaction.is_none() {
            t.mileage_cost = plus(t.mileage_cost, tr.amount);
        }
    }
    t
}

/// Spent as a percentage of the budget, one decimal.
fn percent_of(spent: Decimal, budget: Option<Decimal>) -> Option<Decimal> {
    let b = budget.filter(|b| !b.is_zero())?;
    let p = spent
        .checked_mul(Decimal::ONE_HUNDRED)?
        .checked_div(b)?
        .round_dp_with_strategy(1, mk_core::RoundingStrategy::MidpointAwayFromZero);
    let mut p = p;
    p.rescale(1);
    Some(p)
}

/// `ok`, `warn` (at the warning percentage) or `over` (over 100 %).
fn level(percent: Option<Decimal>, warn: Decimal) -> &'static str {
    match percent {
        Some(p) if p > Decimal::ONE_HUNDRED => "over",
        Some(p) if p >= warn => "warn",
        _ => "ok",
    }
}

fn warn_percent(ctx: &Ctx<'_>) -> CommandResult<Decimal> {
    Ok(Decimal::from(crate::module_settings::get_i64(
        ctx,
        "projects.warn_percent",
    )?))
}

fn customer_name(ctx: &Ctx<'_>, p: &Project) -> Option<String> {
    p.customer
        .as_deref()
        .and_then(|c| crate::sales::find_party(ctx, c).ok())
        .map(|c| c.name)
}

fn fund_name(ctx: &Ctx<'_>, p: &Project) -> CommandResult<Option<String>> {
    let Some(f) = &p.fund else {
        return Ok(None);
    };
    Ok(crate::dimensions::records(ctx, "fund")?
        .into_iter()
        .find(|r| r.id == *f)
        .map(|r| r.name))
}

fn status_label(s: &str) -> &'static str {
    STATUSES.iter().find(|x| x.0 == s).map_or("Active", |x| x.1)
}

fn project_json(ctx: &Ctx<'_>, p: &Project) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let projects = all(ctx)?;
    let mut v = serde_json::to_value(p)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("status_label".into(), json!(status_label(&p.status)));
        o.insert("open".into(), json!(p.open()));
        o.insert("active".into(), json!(p.open()));
        o.insert("customer_name".into(), json!(customer_name(ctx, p)));
        o.insert("fund_name".into(), json!(fund_name(ctx, p)?));
        o.insert("level".into(), json!(p.name.matches(':').count()));
        o.insert(
            "budget_total".into(),
            json!(budget_of(&projects, p).map(money)),
        );
        o.insert(
            "budget_lines".into(),
            json!(
                p.budget_lines
                    .iter()
                    .map(|l| json!({
                        "account": l.account,
                        "account_name": l.account.map_or_else(|| "Other".to_owned(), |a| chart.full_name(a)),
                        "amount": money(l.amount),
                    }))
                    .collect::<Vec<_>>()
            ),
        );
        o.insert("budget".into(), json!(p.budget.map(money)));
        o.insert("income_budget".into(), json!(p.income_budget.map(money)));
        o.insert(
            "markup_percent".into(),
            json!(p.markup_percent.normalize().to_string()),
        );
        o.insert(
            "mileage_bill_rate".into(),
            json!(p.mileage_bill_rate.map(crate::mileage::plain)),
        );
    }
    Ok(v)
}

// ----- list ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    /// Only projects with this status; `open` for every not completed or cancelled.
    #[serde(default)]
    status: Option<String>,
}

/// One project's row (list, summary report, dashboard block).
fn row(
    ctx: &Ctx<'_>,
    projects: &[Project],
    p: &Project,
    txns: &[Transaction],
    chart: &Chart,
    billed: &HashMap<String, (String, String)>,
    dates: (Option<Date>, Option<Date>),
) -> CommandResult<Value> {
    let ids = family(projects, p);
    let lines = lines_of(chart, txns, &ids, dates.0, dates.1);
    let trips = trips_of(ctx, &ids, dates.0, dates.1)?;
    let t = totals(&lines, &trips);
    let budget = budget_of(projects, p);
    let pct = percent_of(t.expenses, budget);
    let warn = warn_percent(ctx)?;
    let invoicing = is_module_enabled(ctx.tx, crate::sales::INVOICING)?;
    let unbilled = if invoicing {
        let items = items_for(ctx, projects, &ids, txns, chart)?;
        let open: Vec<&Item> = items
            .iter()
            .filter(|i| !billed.contains_key(&i.key))
            .collect();
        Some(json!({
            "count": open.len(),
            "amount": money(open.iter().fold(Decimal::ZERO, |a, i| plus(a, i.amount))),
        }))
    } else {
        None
    };
    Ok(json!({
        "id": p.id,
        "name": p.name,
        "level": p.name.matches(':').count(),
        "status": p.status,
        "status_label": status_label(&p.status),
        "open": p.open(),
        "customer": p.customer,
        "customer_name": customer_name(ctx, p),
        "fund": p.fund,
        "billable": p.billable,
        "budget": budget.map(money),
        "spent": money(t.expenses),
        "percent": pct.map(|x| x.to_string()),
        "level_of_use": level(pct, warn),
        "remaining": budget.map(|b| money(minus(b, t.expenses))),
        "income": money(t.income),
        "net": money(minus(t.income, t.expenses)),
        "miles": crate::mileage::plain(t.miles),
        "mileage_value": money(t.mileage_value),
        "unbilled": unbilled,
    }))
}

fn list(ctx: &Ctx<'_>, a: ListArgs) -> CommandResult<Value> {
    let projects = all(ctx)?;
    let txns = posted(ctx)?;
    let chart = Chart::load(ctx)?;
    let billed = billed_map(ctx)?;
    let mut rows = Vec::new();
    let mut tb = Decimal::ZERO;
    let mut ts = Decimal::ZERO;
    let mut ti = Decimal::ZERO;
    for p in &projects {
        match a.status.as_deref() {
            Some("open") if !p.open() => continue,
            Some(s) if s != "open" && s != p.status => continue,
            _ => {}
        }
        let r = row(ctx, &projects, p, &txns, &chart, &billed, (None, None))?;
        // Totals add top-level projects only (a parent includes its sub-projects).
        if !p.name.contains(':') {
            let d = |k: &str| -> Decimal {
                r.get(k)
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default()
            };
            tb = plus(tb, d("budget"));
            ts = plus(ts, d("spent"));
            ti = plus(ti, d("income"));
        }
        rows.push(r);
    }
    Ok(json!({
        "projects": rows,
        "totals": {"budget": money(tb), "spent": money(ts), "income": money(ti), "net": money(minus(ti, ts))},
        "warn_percent": warn_percent(ctx)?.to_string(),
        "invoicing": is_module_enabled(ctx.tx, crate::sales::INVOICING)?,
    }))
}

// ----- the project report ----------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReportArgs {
    /// Id or name; its sub-projects are included.
    project: String,
    /// Default: the project's start (or all dates).
    #[serde(default)]
    from: Option<Date>,
    /// Default: the project's end (or all dates).
    #[serde(default)]
    to: Option<Date>,
    /// Show the Mileage section (default true).
    #[serde(default = "yes")]
    include_mileage: bool,
    /// Count mileage as a cost (adds unposted trips' value to the expenses).
    #[serde(default)]
    count_mileage: bool,
}

fn yes() -> bool {
    true
}

fn by_account(chart: &Chart, lines: &[Line], kind: &str) -> (Vec<Value>, Decimal) {
    let mut m: BTreeMap<String, (AccountId, Decimal)> = BTreeMap::new();
    for l in lines.iter().filter(|l| l.kind == kind) {
        let name = chart.full_name(l.account);
        let e = m
            .entry(name.to_lowercase())
            .or_insert((l.account, Decimal::ZERO));
        e.1 = plus(e.1, l.amount);
    }
    let mut total = Decimal::ZERO;
    let rows = m
        .into_values()
        .map(|(a, v)| {
            total = plus(total, v);
            json!({"account": a, "name": chart.full_name(a), "amount": money(v)})
        })
        .collect();
    (rows, total)
}

/// Budget vs actual per line (by account) or for the total.
fn budget_rows(
    chart: &Chart,
    projects: &[Project],
    p: &Project,
    lines: &[Line],
    spent: Decimal,
) -> Vec<Value> {
    let fmt = |name: String, budget: Option<Decimal>, actual: Decimal| {
        json!({
            "name": name,
            "budget": budget.map(money),
            "actual": money(actual),
            "remaining": budget.map(|b| money(minus(b, actual))),
            "percent": percent_of(actual, budget).map(|x| x.to_string()),
        })
    };
    if p.budget_mode != "by_account" || p.budget_lines.is_empty() {
        return Vec::new();
    }
    let covered: Vec<(Option<AccountId>, Vec<AccountId>)> = p
        .budget_lines
        .iter()
        .map(|l| {
            (
                l.account,
                l.account.map(|a| chart.subtree(a)).unwrap_or_default(),
            )
        })
        .collect();
    let expense: Vec<&Line> = lines.iter().filter(|l| l.kind == "expense").collect();
    let in_lines = |acc: AccountId| covered.iter().any(|(_, sub)| sub.contains(&acc));
    let mut out = Vec::new();
    for (bl, (_, sub)) in p.budget_lines.iter().zip(&covered) {
        let actual = match bl.account {
            Some(_) => expense
                .iter()
                .filter(|l| sub.contains(&l.account))
                .fold(Decimal::ZERO, |a, l| plus(a, l.amount)),
            None => expense
                .iter()
                .filter(|l| !in_lines(l.account))
                .fold(Decimal::ZERO, |a, l| plus(a, l.amount)),
        };
        let name = bl
            .account
            .map_or_else(|| "Other".to_owned(), |a| chart.full_name(a));
        out.push(fmt(name, Some(bl.amount), actual));
    }
    if !p.budget_lines.iter().any(|l| l.account.is_none()) {
        let rest = expense
            .iter()
            .filter(|l| !in_lines(l.account))
            .fold(Decimal::ZERO, |a, l| plus(a, l.amount));
        if !rest.is_zero() {
            out.push(fmt("Not budgeted".into(), None, rest));
        }
    }
    out.push(fmt("Total".into(), budget_of(projects, p), spent));
    out
}

fn mileage_section(ctx: &Ctx<'_>, trips: &[crate::mileage::TripDoc]) -> CommandResult<Value> {
    let mut by: BTreeMap<String, (Decimal, Decimal)> = BTreeMap::new();
    for t in trips {
        let e = by
            .entry(t.purpose_kind.clone())
            .or_insert((Decimal::ZERO, Decimal::ZERO));
        e.0 = plus(e.0, t.miles);
        e.1 = plus(e.1, t.amount);
    }
    let mut miles = Decimal::ZERO;
    let mut value = Decimal::ZERO;
    let rows: Vec<Value> = by
        .into_iter()
        .map(|(k, (m, v))| {
            miles = plus(miles, m);
            value = plus(value, v);
            json!({"purpose_kind": k, "label": crate::mileage::kind_label(&k),
                   "miles": crate::mileage::plain(m), "value": money(v)})
        })
        .collect();
    let mut list = Vec::new();
    for t in trips {
        list.push(json!({
            "id": t.id, "date": t.date, "purpose_kind": t.purpose_kind,
            "purpose": t.purpose,
            "route": crate::mileage::route_of(t.from_place.as_deref(), t.to_place.as_deref(), t.round_trip),
            "vehicle": crate::mileage::vehicle_name(ctx, t.vehicle.as_deref())?,
            "miles": crate::mileage::plain(t.miles),
            "rate_cents": crate::mileage::plain(t.rate_cents),
            "value": money(t.amount),
            "billable": t.tags.billable,
            "posted": t.transaction.is_some(),
        }));
    }
    Ok(
        json!({"rows": rows, "miles": crate::mileage::plain(miles), "value": money(value), "trips": list}),
    )
}

fn report_data(
    ctx: &Ctx<'_>,
    p: &Project,
    from: Option<Date>,
    to: Option<Date>,
    include_mileage: bool,
    count_mileage: bool,
) -> CommandResult<Value> {
    let projects = all(ctx)?;
    let chart = Chart::load(ctx)?;
    let txns = posted(ctx)?;
    let from = from.or(p.start);
    let to = to.or(p.end);
    let ids = family(&projects, p);
    let lines = lines_of(&chart, &txns, &ids, from, to);
    let trips = trips_of(ctx, &ids, from, to)?;
    let t = totals(&lines, &trips);
    let (income_rows, income) = by_account(&chart, &lines, "income");
    let (expense_rows, expenses) = by_account(&chart, &lines, "expense");
    let counted = count_mileage && include_mileage;
    let cost = if counted {
        plus(expenses, t.mileage_cost)
    } else {
        expenses
    };
    let budget = budget_of(&projects, p);
    let warn = warn_percent(ctx)?;
    let pct = percent_of(cost, budget);
    let mut budget_lines = budget_rows(&chart, &projects, p, &lines, cost);
    if budget_lines.is_empty() && budget.is_some() {
        budget_lines.push(json!({
            "name": "Total", "budget": budget.map(money), "actual": money(cost),
            "remaining": budget.map(|b| money(minus(b, cost))),
            "percent": pct.map(|x| x.to_string()),
        }));
    }
    Ok(json!({
        "project": p.id,
        "name": p.name,
        "status": p.status,
        "status_label": status_label(&p.status),
        "customer_name": customer_name(ctx, p),
        "fund_name": fund_name(ctx, p)?,
        "sub_projects": ids.len().saturating_sub(1),
        "from": from,
        "to": to,
        "income": {"rows": income_rows, "total": money(income)},
        "expenses": {"rows": expense_rows, "total": money(expenses)},
        "mileage": if include_mileage { mileage_section(ctx, &trips)? } else { Value::Null },
        "count_mileage": counted,
        "mileage_cost": money(if counted { t.mileage_cost } else { Decimal::ZERO }),
        "cost": money(cost),
        "net": money(minus(income, cost)),
        "budget": budget.map(money),
        "income_budget": p.income_budget.map(money),
        "budget_mode": p.budget_mode,
        "budget_lines": budget_lines,
        "percent": pct.map(|x| x.to_string()),
        "level_of_use": level(pct, warn),
        "remaining": budget.map(|b| money(minus(b, cost))),
    }))
}

fn report_project(ctx: &Ctx<'_>, a: ReportArgs) -> CommandResult<Value> {
    let p = find(ctx, &a.project)?;
    report_data(ctx, &p, a.from, a.to, a.include_mileage, a.count_mileage)
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SummaryArgs {
    #[serde(default)]
    from: Option<Date>,
    #[serde(default)]
    to: Option<Date>,
    /// A status, or `open` (default: every project).
    #[serde(default)]
    status: Option<String>,
}

fn report_summary(ctx: &Ctx<'_>, a: SummaryArgs) -> CommandResult<Value> {
    let projects = all(ctx)?;
    let txns = posted(ctx)?;
    let chart = Chart::load(ctx)?;
    let billed = HashMap::new();
    let mut rows = Vec::new();
    for p in &projects {
        match a.status.as_deref() {
            Some("open") if !p.open() => continue,
            Some(s) if s != "open" && s != p.status => continue,
            _ => {}
        }
        let r = row(ctx, &projects, p, &txns, &chart, &billed, (a.from, a.to))?;
        rows.push(json!({
            "id": p.id, "name": p.name, "level": at(&r, "level"), "status": p.status,
            "status_label": status_label(&p.status), "budget": at(&r, "budget"),
            "expenses": at(&r, "spent"), "percent": at(&r, "percent"), "income": at(&r, "income"),
            "net": at(&r, "net"), "miles": at(&r, "miles"),
        }));
    }
    Ok(json!({"from": a.from, "to": a.to, "status": a.status, "rows": rows}))
}

// ----- get ----------------------------------------------------------------------------------

fn get(ctx: &Ctx<'_>, a: RefArgs) -> CommandResult<Value> {
    let p = find(ctx, &a.project)?;
    let projects = all(ctx)?;
    let chart = Chart::load(ctx)?;
    let txns = posted(ctx)?;
    let ids = family(&projects, &p);
    let mut out = project_json(ctx, &p)?;
    let summary = report_data(ctx, &p, None, None, true, false)?;
    let lines = lines_of(&chart, &txns, &ids, None, None);
    let names: HashMap<String, String> = projects
        .iter()
        .map(|x| (x.id.clone(), x.name.clone()))
        .collect();
    let transactions: Vec<Value> = lines
        .iter()
        .map(|l| {
            json!({
                "txn": l.txn, "split": l.idx, "date": l.date, "payee": l.payee, "memo": l.memo,
                "account": l.account, "account_name": chart.full_name(l.account),
                "project": l.project, "project_name": names.get(&l.project),
                "kind": l.kind, "counted": l.kind != "transfer",
                "amount": money(l.amount), "billable": l.billable,
            })
        })
        .collect();
    let mut documents = Vec::new();
    let tagged = |t: &mk_core::SplitTags| t.project.as_deref().is_some_and(|x| ids.contains(x));
    for kind in ["invoice", "estimate", "credit_memo"] {
        for d in crate::sales::docs(ctx, kind)? {
            if tagged(&d.tags) || d.totals.lines.iter().any(|l| tagged(&l.tags)) {
                documents.push(json!({"kind": d.kind, "id": d.id, "number": d.number,
                    "date": d.date, "total": money(d.totals.total), "status": d.status}));
            }
        }
    }
    if is_module_enabled(ctx.tx, "bills")? {
        for d in crate::purchases::bill_docs(ctx, "bill")? {
            if tagged(&d.tags) || d.computed.iter().any(|l| tagged(&l.tags)) {
                documents.push(json!({"kind": "bill", "id": d.id, "number": d.number,
                    "date": d.date, "total": money(d.total), "status": d.status}));
            }
        }
    }
    let billable = if is_module_enabled(ctx.tx, crate::sales::INVOICING)? {
        let billed = billed_map(ctx)?;
        let items = items_for(ctx, &projects, &ids, &txns, &chart)?;
        let (done, open): (Vec<&Item>, Vec<&Item>) =
            items.iter().partition(|i| billed.contains_key(&i.key));
        json!({
            "unbilled": open.iter().map(|i| i.json(None)).collect::<Vec<_>>(),
            "billed": done.iter().map(|i| i.json(billed.get(&i.key))).collect::<Vec<_>>(),
            "unbilled_total": money(open.iter().fold(Decimal::ZERO, |a, i| plus(a, i.amount))),
            "unbilled_with_markup": money(open.iter().fold(Decimal::ZERO, |a, i| plus(a, i.with_markup))),
        })
    } else {
        Value::Null
    };
    if let Some(o) = out.as_object_mut() {
        o.insert("summary".into(), summary);
        o.insert("transactions".into(), json!(transactions));
        o.insert("documents".into(), json!(documents));
        o.insert("billable_items".into(), billable);
        o.insert(
            "untagged_fund_splits".into(),
            json!(if p.fund.is_some() {
                without_fund(&txns, &p.id)
            } else {
                0
            }),
        );
        o.insert(
            "sub_projects".into(),
            json!(
                projects
                    .iter()
                    .filter(|x| x.id != p.id && ids.contains(&x.id))
                    .map(|x| json!({"id": x.id, "name": x.name}))
                    .collect::<Vec<_>>()
            ),
        );
    }
    Ok(out)
}

// ----- billable costs ------------------------------------------------------------------------

/// A billable cost: a tagged expense line or a tagged trip.
struct Item {
    /// `split:{split id}` or `trip:{id}`.
    key: String,
    kind: &'static str,
    project: String,
    project_name: String,
    date: Date,
    payee: String,
    account: Option<AccountId>,
    account_name: String,
    memo: String,
    amount: Decimal,
    with_markup: Decimal,
}

impl Item {
    fn json(&self, billed: Option<&(String, String)>) -> Value {
        json!({
            "key": self.key, "kind": self.kind, "project": self.project,
            "project_name": self.project_name, "date": self.date, "payee": self.payee,
            "account": self.account, "account_name": self.account_name, "memo": self.memo,
            "amount": money(self.amount), "with_markup": money(self.with_markup),
            "invoice": billed.map(|b| b.0.clone()),
            "invoice_number": billed.map(|b| b.1.clone()),
        })
    }
}

fn with_markup(amount: Decimal, markup: Decimal) -> Decimal {
    if markup.is_zero() {
        return amount;
    }
    let factor = plus(Decimal::ONE, markup / Decimal::ONE_HUNDRED);
    round2(amount.checked_mul(factor).unwrap_or(amount))
}

/// Billable costs of the projects in `ids`.
fn items_for(
    ctx: &Ctx<'_>,
    projects: &[Project],
    ids: &BTreeSet<String>,
    txns: &[Transaction],
    chart: &Chart,
) -> CommandResult<Vec<Item>> {
    let by_id: HashMap<&str, &Project> = projects.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut out = Vec::new();
    // release-prep.md §4: an expense line is known by its split's own id, which
    // editing the transaction (reordering its lines) keeps.
    let mut split_ids: HashMap<mk_core::TransactionId, Vec<String>> = HashMap::new();
    for l in lines_of(chart, txns, ids, None, None) {
        if l.kind != "expense" || !l.billable {
            continue;
        }
        let Some(p) = by_id.get(l.project.as_str()) else {
            continue;
        };
        if let std::collections::hash_map::Entry::Vacant(e) = split_ids.entry(l.txn) {
            e.insert(ctx.tx.split_ids(l.txn)?);
        }
        let Some(split) = split_ids
            .get(&l.txn)
            .and_then(|v| v.get(l.idx))
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        out.push(Item {
            key: format!("split:{split}"),
            kind: "expense",
            project: p.id.clone(),
            project_name: p.name.clone(),
            date: l.date,
            payee: l.payee.clone(),
            account: Some(l.account),
            account_name: chart.full_name(l.account),
            memo: l.memo.clone(),
            amount: l.amount,
            with_markup: with_markup(l.amount, p.markup_percent),
        });
    }
    for t in trips_of(ctx, ids, None, None)? {
        // A posted (reimbursed) trip bills through its expense line instead.
        if !t.tags.billable || t.transaction.is_some() {
            continue;
        }
        let Some(p) = t.tags.project.as_deref().and_then(|x| by_id.get(x)) else {
            continue;
        };
        let amount = match p.mileage_bill_rate {
            Some(c) => crate::mileage::value_of(t.miles, c),
            None => t.amount,
        };
        out.push(Item {
            key: format!("trip:{}", t.id),
            kind: "mileage",
            project: p.id.clone(),
            project_name: p.name.clone(),
            date: t.date,
            payee: crate::mileage::route_of(
                t.from_place.as_deref(),
                t.to_place.as_deref(),
                t.round_trip,
            ),
            account: None,
            account_name: "Mileage".into(),
            memo: format!(
                "{} mi{}",
                crate::mileage::plain(t.miles),
                t.purpose
                    .as_deref()
                    .map(|x| format!(", {x}"))
                    .unwrap_or_default()
            ),
            amount,
            with_markup: with_markup(amount, p.markup_percent),
        });
    }
    out.sort_by(|a, b| (a.date, &a.key).cmp(&(b.date, &b.key)));
    Ok(out)
}

/// Billed items: key → (invoice id, invoice number), from invoice lines that are
/// not void. Deleting the line or the invoice makes the item unbilled again.
fn billed_map(ctx: &Ctx<'_>) -> CommandResult<HashMap<String, (String, String)>> {
    let mut m = HashMap::new();
    if !is_module_enabled(ctx.tx, crate::sales::INVOICING)? {
        return Ok(m);
    }
    for d in crate::sales::docs(ctx, "invoice")? {
        if d.status == "void" {
            continue;
        }
        for l in &d.lines {
            for k in &l.billable_items {
                m.insert(k.clone(), (d.id.clone(), d.number.clone()));
            }
        }
    }
    Ok(m)
}

/// An invoice's lines may carry billable items only once each, and only items that
/// exist (`doc` is the invoice being saved).
pub(crate) fn check_billable(ctx: &Ctx<'_>, doc: &str, keys: &[String]) -> CommandResult<()> {
    if keys.is_empty() {
        return Ok(());
    }
    let projects = all(ctx)?;
    let ids: BTreeSet<String> = projects.iter().map(|p| p.id.clone()).collect();
    let items = items_for(ctx, &projects, &ids, &posted(ctx)?, &Chart::load(ctx)?)?;
    let billed = billed_map(ctx)?;
    let mut seen = BTreeSet::new();
    for k in keys {
        if !seen.insert(k.clone()) {
            return Err(invalid(format!("billable cost {k} is on two lines")));
        }
        if !items.iter().any(|i| i.key == *k) {
            return Err(invalid(format!("{k} is not a billable project cost")));
        }
        if let Some((id, number)) = billed.get(k)
            && id != doc
        {
            return Err(rejected(format!(
                "a billable cost is already on invoice {number}"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UnbilledArgs {
    /// Only this project (id or name; with its sub-projects).
    #[serde(default)]
    project: Option<String>,
    /// Only projects for this customer (party id or name).
    #[serde(default)]
    customer: Option<String>,
}

fn unbilled(ctx: &Ctx<'_>, a: UnbilledArgs) -> CommandResult<Value> {
    if !is_module_enabled(ctx.tx, crate::sales::INVOICING)? {
        return Ok(json!({"projects": []}));
    }
    let projects = all(ctx)?;
    let chart = Chart::load(ctx)?;
    let txns = posted(ctx)?;
    let billed = billed_map(ctx)?;
    let customer = match a.customer.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(c) => Some(crate::sales::find_party(ctx, c)?.id),
    };
    let only = match &a.project {
        Some(r) => Some(find(ctx, r)?),
        None => None,
    };
    let mut out = Vec::new();
    for p in &projects {
        if customer
            .as_ref()
            .is_some_and(|c| p.customer.as_ref() != Some(c))
        {
            continue;
        }
        let ids = match &only {
            Some(o) if family(&projects, o).contains(&p.id) => BTreeSet::from([p.id.clone()]),
            Some(_) => continue,
            None => BTreeSet::from([p.id.clone()]),
        };
        let items: Vec<Item> = items_for(ctx, &projects, &ids, &txns, &chart)?
            .into_iter()
            .filter(|i| !billed.contains_key(&i.key))
            .collect();
        if items.is_empty() {
            continue;
        }
        let total = items.iter().fold(Decimal::ZERO, |a, i| plus(a, i.amount));
        let marked = items
            .iter()
            .fold(Decimal::ZERO, |a, i| plus(a, i.with_markup));
        out.push(json!({
            "project": p.id, "name": p.name, "customer": p.customer,
            "customer_name": customer_name(ctx, p),
            "markup_percent": p.markup_percent.normalize().to_string(),
            "count": items.len(), "total": money(total), "with_markup": money(marked),
            "items": items.iter().map(|i| i.json(None)).collect::<Vec<_>>(),
        }));
    }
    Ok(json!({"projects": out}))
}

/// The account billable costs post to (created on first use).
fn billable_income(ctx: &Ctx<'_>) -> CommandResult<AccountId> {
    crate::module_settings::account_or_create(
        ctx,
        "projects.billable_income_account",
        BILLABLE_INCOME,
        AccountType::Income,
        mk_core::AccountSubtype::Income,
    )
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LinesArgs {
    /// Item keys from `project.unbilled`.
    items: Vec<String>,
    /// One summary line per expense account instead of one line per item.
    #[serde(default)]
    summary: bool,
    /// The invoice being edited (its own items count as unbilled).
    #[serde(default)]
    invoice: Option<String>,
}

fn billable_lines(ctx: &Ctx<'_>, a: LinesArgs) -> CommandResult<Value> {
    ctx.quiet_noop.set(true);
    if !is_module_enabled(ctx.tx, crate::sales::INVOICING)? {
        return Err(rejected("billable costs need the invoicing feature on"));
    }
    if a.items.is_empty() {
        return Err(invalid("choose at least one cost"));
    }
    let projects = all(ctx)?;
    let ids: BTreeSet<String> = projects.iter().map(|p| p.id.clone()).collect();
    let chart = Chart::load(ctx)?;
    let all_items = items_for(ctx, &projects, &ids, &posted(ctx)?, &chart)?;
    let billed = billed_map(ctx)?;
    let mut chosen: Vec<&Item> = Vec::new();
    for k in &a.items {
        let it = all_items
            .iter()
            .find(|i| i.key == *k)
            .ok_or_else(|| invalid(format!("{k} is not a billable project cost")))?;
        if let Some((id, number)) = billed.get(k)
            && a.invoice.as_deref() != Some(id.as_str())
        {
            return Err(rejected(format!(
                "a chosen cost is already on invoice {number}"
            )));
        }
        chosen.push(it);
    }
    let account = billable_income(ctx)?;
    let chart = Chart::load(ctx)?;
    let mut lines = Vec::new();
    if a.summary {
        let mut groups: BTreeMap<(String, String), Vec<&Item>> = BTreeMap::new();
        for it in &chosen {
            groups
                .entry((it.project.clone(), it.account_name.clone()))
                .or_default()
                .push(it);
        }
        for ((project, name), items) in groups {
            let amount = items
                .iter()
                .fold(Decimal::ZERO, |a, i| plus(a, i.with_markup));
            lines.push(json!({
                "account": account,
                "description": format!("{name} ({} item{})", items.len(), if items.len() == 1 { "" } else { "s" }),
                "amount": money(amount),
                "project": project,
                "billable_items": items.iter().map(|i| i.key.clone()).collect::<Vec<_>>(),
            }));
        }
    } else {
        for it in &chosen {
            let mut d = format!("{} {}", it.date, it.payee);
            if !it.memo.is_empty() {
                d.push_str(" — ");
                d.push_str(&it.memo);
            }
            lines.push(json!({
                "account": account,
                "description": d.trim(),
                "amount": money(it.with_markup),
                "project": it.project,
                "billable_items": [it.key],
            }));
        }
    }
    Ok(json!({
        "account": account,
        "account_name": chart.full_name(account),
        "lines": lines,
    }))
}

// ----- alerts and the dashboard block --------------------------------------------------------

fn dismissed(ctx: &Ctx<'_>) -> CommandResult<BTreeSet<String>> {
    Ok(ctx
        .tx
        .get_setting(MODULE, DISMISSED_KEY)?
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect())
}

/// The date spending last crossed `limit` (from below `limit` to at or over it when
/// `strict` is false; over it when true), if it is past it now.
fn crossing(lines: &[Line], limit: Decimal, strict: bool) -> Option<Date> {
    let past = |v: Decimal| if strict { v > limit } else { v >= limit };
    let mut cum = Decimal::ZERO;
    let mut last = None;
    for l in lines.iter().filter(|l| l.kind == "expense") {
        let before = past(cum);
        cum = plus(cum, l.amount);
        let now = past(cum);
        if now && !before {
            last = Some(l.date);
        }
        if !now {
            last = None;
        }
    }
    last
}

fn alerts(ctx: &Ctx<'_>) -> CommandResult<Vec<Value>> {
    if !is_module_enabled(ctx.tx, MODULE)? {
        return Ok(Vec::new());
    }
    let projects = all(ctx)?;
    let txns = posted(ctx)?;
    let chart = Chart::load(ctx)?;
    let warn = warn_percent(ctx)?;
    let gone = dismissed(ctx)?;
    let mut out = Vec::new();
    for p in projects.iter().filter(|p| p.open()) {
        let Some(budget) = budget_of(&projects, p).filter(|b| !b.is_zero()) else {
            continue;
        };
        let ids = family(&projects, p);
        let lines = lines_of(&chart, &txns, &ids, None, None);
        let spent = totals(&lines, &[]).expenses;
        let target = json!({"screen": "projects", "project": p.id});
        let (key, label, kind) = if let Some(d) = crossing(&lines, budget, true) {
            (
                format!("{}:over:{d}", p.id),
                format!(
                    "{} is over budget by {}",
                    p.name,
                    crate::sales::group(&money(minus(spent, budget)))
                ),
                "project_over_budget",
            )
        } else {
            let limit = budget.checked_mul(warn).unwrap_or(budget) / Decimal::ONE_HUNDRED;
            let Some(d) = crossing(&lines, limit, false) else {
                continue;
            };
            let pct = percent_of(spent, Some(budget)).unwrap_or_default();
            (
                format!("{}:warn:{d}", p.id),
                format!(
                    "{} has used {} % of its budget",
                    p.name,
                    pct.trunc().normalize()
                ),
                "project_budget_warning",
            )
        };
        if gone.contains(&key) {
            continue;
        }
        out.push(json!({
            "kind": kind, "project": p.id, "label": label, "target": target, "key": key,
            "dismiss": {"command": "project.dismiss_alert", "key": key},
        }));
    }
    Ok(out)
}

/// Home → Attention items (dashboards.md).
pub(crate) fn attention(ctx: &Ctx<'_>) -> CommandResult<Vec<Value>> {
    alerts(ctx)
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

fn alerts_cmd(ctx: &Ctx<'_>, _a: NoArgs) -> CommandResult<Value> {
    Ok(json!({"alerts": alerts(ctx)?}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DismissArgs {
    key: String,
}

fn dismiss_alert(ctx: &Ctx<'_>, a: DismissArgs) -> CommandResult<Value> {
    let mut gone = dismissed(ctx)?;
    if gone.insert(a.key.clone()) {
        let v = json!(gone.into_iter().collect::<Vec<_>>());
        ctx.save_setting(MODULE, DISMISSED_KEY, Some(&v))?;
    }
    Ok(json!({"dismissed": a.key}))
}

/// The Projects dashboard block: open projects with a budget bar each.
pub(crate) fn block(ctx: &Ctx<'_>) -> CommandResult<Value> {
    let projects = all(ctx)?;
    let txns = posted(ctx)?;
    let chart = Chart::load(ctx)?;
    let billed = HashMap::new();
    let mut rows = Vec::new();
    for p in projects.iter().filter(|p| p.status == "active") {
        rows.push(row(
            ctx,
            &projects,
            p,
            &txns,
            &chart,
            &billed,
            (None, None),
        )?);
    }
    Ok(json!({"status": "ok", "projects": rows}))
}

// ----- names for other modules -----------------------------------------------------------------

/// Open projects for the phone (`GET /lists`) and tag pickers.
pub(crate) fn open_names(ctx: &Ctx<'_>) -> CommandResult<Vec<String>> {
    if !is_module_enabled(ctx.tx, MODULE)? {
        return Ok(Vec::new());
    }
    Ok(all(ctx)?
        .into_iter()
        .filter(Project::open)
        .map(|p| p.name)
        .collect())
}

// ----- CSV and PDF export ----------------------------------------------------------------------

/// `report.export` for report.project and report.projects_summary (CSV).
pub(crate) fn export(
    ctx: &Ctx<'_>,
    name: &str,
    args: Value,
) -> CommandResult<Option<(Value, String)>> {
    if name != "project" && name != "projects_summary" {
        return Ok(None);
    }
    if !is_module_enabled(ctx.tx, MODULE)? {
        return Err(CommandError::ModuleDisabled {
            command: format!("report.{name}"),
            module: MODULE.into(),
        });
    }
    let s = |v: &Value| v.as_str().map(str::to_owned).unwrap_or_default();
    if name == "project" {
        let d = report_project(ctx, crate::parse_args(args)?)?;
        let mut c = crate::reports::Csv::new(&["Section".into(), "Name".into(), "Amount".into()]);
        for r in at(at(&d, "income"), "rows")
            .as_array()
            .into_iter()
            .flatten()
        {
            c.row(&["Income".into(), s(at(r, "name")), s(at(r, "amount"))]);
        }
        c.row(&[
            "Income".into(),
            "Total income".into(),
            s(at(at(&d, "income"), "total")),
        ]);
        for r in at(at(&d, "expenses"), "rows")
            .as_array()
            .into_iter()
            .flatten()
        {
            c.row(&["Expenses".into(), s(at(r, "name")), s(at(r, "amount"))]);
        }
        c.row(&[
            "Expenses".into(),
            "Total expenses".into(),
            s(at(at(&d, "expenses"), "total")),
        ]);
        if let Some(m) = at(&d, "mileage").as_object() {
            for r in m
                .get("rows")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                c.row(&[
                    "Mileage".into(),
                    format!("{} ({} mi)", s(at(r, "label")), s(at(r, "miles"))),
                    s(at(r, "value")),
                ]);
            }
        }
        if at(&d, "count_mileage").as_bool() == Some(true) {
            c.row(&[
                "Expenses".into(),
                "Mileage counted as a cost".into(),
                s(at(&d, "mileage_cost")),
            ]);
        }
        c.row(&["Net".into(), "Net".into(), s(at(&d, "net"))]);
        for r in at(&d, "budget_lines").as_array().into_iter().flatten() {
            c.row(&[
                "Budget".into(),
                s(at(r, "name")),
                format!(
                    "{} / {} / {}",
                    s(at(r, "budget")),
                    s(at(r, "actual")),
                    s(at(r, "remaining"))
                ),
            ]);
        }
        let mut data = d.clone();
        if let Some(o) = data.as_object_mut() {
            if o.get("from").is_some_and(Value::is_null) {
                o.insert("from".into(), json!("start"));
            }
            if o.get("to").is_some_and(Value::is_null) {
                o.insert("to".into(), json!("end"));
            }
        }
        return Ok(Some((data, c.out)));
    }
    let d = report_summary(ctx, crate::parse_args(args)?)?;
    let mut c = crate::reports::Csv::new(&[
        "Project".into(),
        "Status".into(),
        "Budget".into(),
        "Expenses".into(),
        "% used".into(),
        "Income".into(),
        "Net".into(),
        "Miles".into(),
    ]);
    for r in at(&d, "rows").as_array().into_iter().flatten() {
        c.row(&[
            s(at(r, "name")),
            s(at(r, "status_label")),
            s(at(r, "budget")),
            s(at(r, "expenses")),
            s(at(r, "percent")),
            s(at(r, "income")),
            s(at(r, "net")),
            s(at(r, "miles")),
        ]);
    }
    let mut data = d.clone();
    if let Some(o) = data.as_object_mut() {
        if o.get("from").is_some_and(Value::is_null) {
            o.insert("from".into(), json!("start"));
        }
        if o.get("to").is_some_and(Value::is_null) {
            o.insert("to".into(), json!("end"));
        }
    }
    Ok(Some((data, c.out)))
}

/// `report.export` with `format: "pdf"` for report.project.
pub(crate) fn export_pdf(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    let d = report_project(ctx, crate::parse_args(args)?)?;
    let book = ctx.tx.info()?.name;
    let s = |v: &Value| v.as_str().map(str::to_owned).unwrap_or_default();
    let g = |v: &Value| crate::sales::group(&s(v));
    let mut rows: Vec<(String, String, bool)> = Vec::new();
    let section = |rows: &mut Vec<(String, String, bool)>,
                   title: &str,
                   list: &Value,
                   total: (&str, &Value)| {
        rows.push((title.to_owned(), String::new(), true));
        for r in list.as_array().into_iter().flatten() {
            rows.push((format!("  {}", s(at(r, "name"))), g(at(r, "amount")), false));
        }
        rows.push((total.0.to_owned(), g(total.1), true));
        rows.push((String::new(), String::new(), false));
    };
    section(
        &mut rows,
        "Income",
        at(at(&d, "income"), "rows"),
        ("Total income", at(at(&d, "income"), "total")),
    );
    section(
        &mut rows,
        "Expenses",
        at(at(&d, "expenses"), "rows"),
        ("Total expenses", at(at(&d, "expenses"), "total")),
    );
    if let Some(m) = at(&d, "mileage").as_object() {
        rows.push((
            "Mileage (not counted as a cost)".into(),
            String::new(),
            true,
        ));
        for r in m
            .get("rows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            rows.push((
                format!("  {} ({} mi)", s(at(r, "label")), s(at(r, "miles"))),
                g(at(r, "value")),
                false,
            ));
        }
        rows.push((String::new(), String::new(), false));
    }
    if at(&d, "count_mileage").as_bool() == Some(true) {
        rows.push((
            "Mileage counted as a cost".into(),
            g(at(&d, "mileage_cost")),
            false,
        ));
    }
    rows.push(("Net".into(), g(at(&d, "net")), true));
    rows.push((String::new(), String::new(), false));
    let budget = at(&d, "budget_lines")
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !budget.is_empty() {
        rows.push((
            "Budget vs actual (budget / actual / remaining)".into(),
            String::new(),
            true,
        ));
        for r in &budget {
            rows.push((
                format!("  {}", s(at(r, "name"))),
                format!(
                    "{} / {} / {}",
                    g(at(r, "budget")),
                    g(at(r, "actual")),
                    g(at(r, "remaining"))
                ),
                false,
            ));
        }
    }
    let mut heading = vec![s(at(&d, "name"))];
    if let Some(f) = at(&d, "fund_name").as_str() {
        heading.push(format!("Fund: {f}"));
    }
    if let Some(c) = at(&d, "customer_name").as_str() {
        heading.push(format!("Customer: {c}"));
    }
    let period = match (at(&d, "from").as_str(), at(&d, "to").as_str()) {
        (Some(f), Some(t)) => format!("{f} to {t}"),
        (Some(f), None) => format!("From {f}"),
        (None, Some(t)) => format!("Through {t}"),
        (None, None) => "All dates".into(),
    };
    let (bytes, replaced) = simple_pdf(&book, "Project report", &heading, &period, &rows);
    let file = format!(
        "project-{}",
        s(at(&d, "name"))
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
    );
    Ok(crate::sales::pdf_answer(&file, bytes, &replaced))
}

/// A one-column report PDF: the book name, a title, heading lines, the period, then
/// rows of (label, amount, bold), on as many Letter pages as needed.
fn simple_pdf(
    book: &str,
    title: &str,
    heading: &[String],
    period: &str,
    rows: &[(String, String, bool)],
) -> (Vec<u8>, Vec<char>) {
    use crate::pdf::{Font, Page, Writer};
    const H: i64 = 792;
    const MARGIN: i64 = 50;
    const RIGHT: i64 = 612 - MARGIN;
    const LEAD: i64 = 14;
    let mut wr = Writer {
        replaced: Vec::new(),
    };
    let mut pages: Vec<(Page, Vec<usize>)> = Vec::new();
    let mut cur = Page { ops: Vec::new() };
    let mut y = H - MARGIN;
    cur.text(MARGIN, y - 12, Font::Bold, 12, &wr.encode(book));
    cur.right(RIGHT, y - 18, Font::Bold, 18, &wr.encode(title));
    y -= 40;
    for h in heading {
        cur.text(MARGIN, y, Font::Bold, 11, &wr.encode(&h.replace('→', "->")));
        y -= LEAD;
    }
    cur.text(MARGIN, y, Font::Regular, 10, &wr.encode(period));
    y -= LEAD + 8;
    cur.rule(MARGIN, RIGHT, y + 6);
    y -= 6;
    for (label, amount, bold) in rows {
        if y < MARGIN + 30 {
            let p = std::mem::replace(&mut cur, Page { ops: Vec::new() });
            pages.push((p, Vec::new()));
            y = H - MARGIN;
        }
        let f = if *bold { Font::Bold } else { Font::Regular };
        let line = crate::pdf::wrap(&wr.encode(label), f, 9, 380)
            .into_iter()
            .next()
            .unwrap_or_default();
        cur.text(MARGIN, y, f, 9, &line);
        if !amount.is_empty() {
            cur.right(RIGHT, y, f, 9, &wr.encode(amount));
        }
        y -= LEAD;
    }
    pages.push((cur, Vec::new()));
    let n = pages.len();
    if n > 1 {
        for (i, (p, _)) in pages.iter_mut().enumerate() {
            let t = format!("Page {} of {n}", i + 1);
            p.right(RIGHT, MARGIN - 20, Font::Regular, 8, t.as_bytes());
        }
    }
    (crate::expense_pdf::assemble(&pages, &[]), wr.replaced)
}
