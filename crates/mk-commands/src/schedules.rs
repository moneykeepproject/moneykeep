//! Scheduled transaction commands (spec: scheduled-transactions.md).

use std::collections::BTreeMap;

use mk_core::currency::{format_amount, minor_units};
use mk_core::schedule::{AmountKind, EndRule, Recurrence, WeekendRule};
use mk_core::{AccountId, Basis, CommodityCode, Date, Decimal, TransactionId};
use mk_ledger::schedule::{Nominal, due_date, one_period_after, summary, validate_recurrence};
use mk_store::{Doc, DocTable, now_rfc3339, today_utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::txns::{SplitInput, TxnInput, check_template, create_txn};
use crate::{CommandError, CommandResult, Ctx, Registry};

/// Actor recorded for occurrences entered by `schedule.run_due`.
pub const SCHEDULE_ACTOR: &str = "schedule";

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "schedule.create",
        "Create a scheduled (repeating) transaction",
        true,
        create,
    );
    r.add(
        "schedule.update",
        "Change a schedule's fields; entered and skipped occurrences are kept",
        true,
        update,
    );
    r.add("schedule.pause", "Pause a schedule", true, pause);
    r.add("schedule.resume", "Resume a paused schedule", true, resume);
    r.add(
        "schedule.delete",
        "Delete a schedule; transactions already entered stay",
        true,
        delete,
    );
    r.add("schedule.get", "Get one schedule", false, get);
    r.add(
        "schedule.list",
        "List schedules with their next due date and a plain-words summary",
        false,
        list,
    );
    r.add(
        "schedule.from_txn",
        "Create a schedule that repeats an existing transaction",
        true,
        from_txn,
    );
    r.add(
        "schedule.upcoming",
        "Occurrences not yet entered or skipped, by due date (overdue ones always included)",
        false,
        upcoming,
    );
    r.add(
        "schedule.enter",
        "Enter one occurrence of a schedule as a transaction",
        true,
        enter,
    );
    r.add(
        "schedule.skip",
        "Skip one occurrence of a schedule",
        true,
        skip,
    );
    r.add(
        "schedule.run_due",
        "Enter every due occurrence of active auto-enter schedules (actor `schedule`)",
        true,
        run_due,
    );
    r.add(
        "schedule.preview",
        "Summary and next due dates of a recurrence, without saving anything (for forms)",
        false,
        preview,
    );
    r.add(
        "schedule.forecast",
        "Project an account's balance through the pending occurrences of its schedules",
        false,
        forecast,
    );
}

// ----- stored shapes ---------------------------------------------------------------

/// A transaction without id, date or status.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Template {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// Defaults to the book's base currency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<CommodityCode>,
    /// Splits as for `txn.create` (2 or more, summing to zero); none when the
    /// schedule creates a bill.
    #[serde(default)]
    pub splits: Vec<SplitInput>,
}

/// What a schedule with `creates: "bill"` enters (subscriptions.md revision 2):
/// vendor, terms and bill lines instead of splits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BillTemplate {
    /// Vendor id or name (stored as the id).
    pub vendor: String,
    /// Payment terms; default the vendor's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms: Option<Value>,
    /// Bill lines as for `bill.create` (accounts stored as ids, amounts filled in).
    pub lines: Vec<crate::purchases::BillLineArg>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

/// The only value of `creates` this version knows.
pub(crate) const CREATES_BILL: &str = "bill";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    #[default]
    Active,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Schedule {
    pub id: String,
    pub name: String,
    pub template: Template,
    pub recurrence: Recurrence,
    pub start: Date,
    #[serde(default)]
    pub end: EndRule,
    #[serde(default)]
    pub weekend: WeekendRule,
    #[serde(default)]
    pub amount_kind: AmountKind,
    #[serde(default)]
    pub auto_enter: bool,
    pub remind_days: u32,
    #[serde(default)]
    pub status: Status,
    pub created_at: String,
    /// An investment action entered instead of a plain transaction
    /// (investments-options-bonds-crypto.md coupon reminders): `{action, account,
    /// security}`; the amount comes from the template (or the entered splits).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub investment: Option<Value>,
    /// `"bill"`: entering opens a bill (created through `bill.create`) instead of
    /// posting a transaction (subscriptions.md revision 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creates: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bill: Option<BillTemplate>,
}

impl Schedule {
    pub(crate) fn creates_bill(&self) -> bool {
        self.creates.as_deref() == Some(CREATES_BILL)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Entered,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Occurrence {
    schedule: String,
    nominal: Date,
    due: Date,
    state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    txn: Option<TransactionId>,
    /// The bill a bill schedule's occurrence created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bill: Option<String>,
}

fn occurrence_id(schedule: &str, nominal: Date) -> String {
    format!("{schedule}:{nominal}")
}

fn bad(e: impl std::fmt::Display) -> CommandError {
    CommandError::Integrity(format!("bad stored schedule: {e}"))
}

fn load_all(ctx: &Ctx<'_>) -> CommandResult<Vec<Schedule>> {
    let mut out: Vec<Schedule> = ctx
        .tx
        .docs(DocTable::Schedule, None)?
        .into_iter()
        .map(|d| serde_json::from_value(d.json).map_err(bad))
        .collect::<CommandResult<_>>()?;
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

fn find(ctx: &Ctx<'_>, reference: &str) -> CommandResult<Schedule> {
    if let Some(d) = ctx.tx.get_doc(DocTable::Schedule, reference.trim())? {
        return serde_json::from_value(d.json).map_err(bad);
    }
    let lower = reference.trim().to_lowercase();
    load_all(ctx)?
        .into_iter()
        .find(|s| s.name.to_lowercase() == lower)
        .ok_or_else(|| CommandError::NotFound(format!("no schedule {reference:?}")))
}

fn save(ctx: &Ctx<'_>, s: &Schedule) -> CommandResult<()> {
    ctx.save_doc(
        DocTable::Schedule,
        &s.id.clone(),
        Some(&Doc {
            id: s.id.clone(),
            grp: None,
            ord: 0,
            json: serde_json::to_value(s)?,
        }),
    )
}

/// Stored entered/skipped occurrences by nominal date.
fn states(ctx: &Ctx<'_>, schedule: &str) -> CommandResult<BTreeMap<Date, Occurrence>> {
    ctx.tx
        .docs(DocTable::ScheduleOccurrence, Some(schedule))?
        .into_iter()
        .map(|d| {
            let o: Occurrence = serde_json::from_value(d.json).map_err(bad)?;
            Ok((o.nominal, o))
        })
        .collect()
}

fn save_occurrence(ctx: &Ctx<'_>, o: &Occurrence) -> CommandResult<()> {
    let id = occurrence_id(&o.schedule, o.nominal);
    ctx.save_doc(
        DocTable::ScheduleOccurrence,
        &id.clone(),
        Some(&Doc {
            id,
            grp: Some(o.schedule.clone()),
            ord: 0,
            json: serde_json::to_value(o)?,
        }),
    )
}

/// The schedule and nominal date a transaction was entered from, if any.
pub(crate) fn link_of(ctx: &Ctx<'_>, txn: TransactionId) -> CommandResult<Option<(String, Date)>> {
    for d in ctx.tx.docs(DocTable::ScheduleOccurrence, None)? {
        let o: Occurrence = serde_json::from_value(d.json).map_err(bad)?;
        if o.txn == Some(txn) {
            return Ok(Some((o.schedule, o.nominal)));
        }
    }
    Ok(None)
}

/// A deleted transaction's occurrence becomes pending again.
pub(crate) fn unlink_txn(ctx: &Ctx<'_>, txn: TransactionId) -> CommandResult<()> {
    for d in ctx.tx.docs(DocTable::ScheduleOccurrence, None)? {
        let o: Occurrence = serde_json::from_value(d.json.clone()).map_err(bad)?;
        if o.txn == Some(txn) {
            ctx.save_doc(DocTable::ScheduleOccurrence, &d.id, None)?;
        }
    }
    Ok(())
}

// ----- occurrences -----------------------------------------------------------------

/// Nominal dates with the end rule applied, lazily.
fn nominals(s: &Schedule) -> impl Iterator<Item = Date> + '_ {
    let limit = match s.end {
        EndRule::After(n) => usize::try_from(n).unwrap_or(usize::MAX),
        _ => usize::MAX,
    };
    Nominal::new(&s.recurrence, s.start)
        .take(limit)
        .take(mk_ledger::schedule::MAX_OCCURRENCES)
        .take_while(move |d| match s.end {
            EndRule::On(last) => *d <= last,
            _ => true,
        })
}

/// Pending (not entered or skipped) occurrences as (nominal, due), in nominal order,
/// up to and including nominal dates `until` (or all, when `None`, limited by the
/// end rule and the occurrence cap).
fn pending<'a>(
    s: &'a Schedule,
    handled: &'a BTreeMap<Date, Occurrence>,
    until: Option<Date>,
) -> impl Iterator<Item = (Date, Date)> + 'a {
    nominals(s)
        .take_while(move |d| until.is_none_or(|u| *d <= u))
        .filter(move |d| !handled.contains_key(d))
        .map(move |d| (d, due_date(d, s.weekend)))
}

fn next_due(s: &Schedule, handled: &BTreeMap<Date, Occurrence>) -> Option<(Date, Date)> {
    pending(s, handled, None).next()
}

fn is_nominal(s: &Schedule, date: Date) -> bool {
    nominals(s).take_while(|d| *d <= date).any(|d| d == date)
}

// ----- JSON ------------------------------------------------------------------------

fn template_currency(ctx: &Ctx<'_>, t: &Template) -> CommandResult<CommodityCode> {
    match &t.currency {
        Some(c) => Ok(c.clone()),
        None => Ok(ctx.tx.info()?.base_currency.parse()?),
    }
}

/// Sum of the template's split values in `account` (the first split's account when
/// `None`), and that account.
fn amount_for(
    chart: &Chart,
    t: &Template,
    account: Option<AccountId>,
) -> (Option<AccountId>, Decimal) {
    let target = account.or_else(|| {
        t.splits
            .first()
            .and_then(|s| chart.resolve(&s.account).ok().map(|a| a.id))
    });
    let total = t
        .splits
        .iter()
        .filter(|s| chart.resolve(&s.account).ok().map(|a| a.id) == target)
        .fold(Decimal::ZERO, |acc, s| {
            acc.checked_add(s.value).unwrap_or(acc)
        });
    (target, total)
}

/// A bill schedule's total (the sum of its line amounts).
fn bill_total(s: &Schedule) -> Decimal {
    s.bill
        .as_ref()
        .map(|b| {
            b.lines.iter().fold(Decimal::ZERO, |acc, l| {
                acc.checked_add(l.amount.unwrap_or_default()).unwrap_or(acc)
            })
        })
        .unwrap_or_default()
}

/// A bill schedule as the front ends show it: vendor name, line account names, and
/// an Account / Category / Amount view like `money_view` (Accounts Payable, the
/// line account, minus the total).
fn bill_view(ctx: &Ctx<'_>, chart: &Chart, s: &Schedule, dp: u32) -> CommandResult<Value> {
    let Some(b) = &s.bill else {
        return Ok(Value::Null);
    };
    let vendor = crate::sales::find_party(ctx, &b.vendor).ok();
    let lines: Vec<Value> = b
        .lines
        .iter()
        .map(|l| {
            let account = l.account.as_deref().and_then(|a| chart.resolve(a).ok());
            json!({
                "account": l.account,
                "account_name": account.map(|a| chart.full_name(a.id)),
                "item": l.item,
                "description": l.description,
                "amount": l.amount.map(|x| format_amount(x, Some(dp))),
            })
        })
        .collect();
    let category = match b.lines.as_slice() {
        [one] => one
            .account
            .as_deref()
            .and_then(|a| chart.resolve(a).ok())
            .map(|a| chart.full_name(a.id))
            .unwrap_or_default(),
        many => format!("Split ({})", many.len()),
    };
    let ap = crate::purchases::payable_id(ctx)?;
    let total = bill_total(s);
    Ok(json!({
        "vendor": b.vendor,
        "vendor_name": vendor.map(|v| v.name),
        "terms": b.terms,
        "memo": b.memo,
        "lines": lines,
        "total": format_amount(total, Some(dp)),
        "money": {
            "account": {
                "id": ap,
                "full_name": ap.map_or_else(|| "Accounts Payable".to_owned(), |a| chart.full_name(a)),
            },
            "amount": format_amount(-total, Some(dp)),
            "category": category,
        },
    }))
}

fn template_json(chart: &Chart, t: &Template) -> CommandResult<Value> {
    let mut v = serde_json::to_value(t)?;
    if let Some(splits) = v.get_mut("splits").and_then(Value::as_array_mut) {
        for (sv, s) in splits.iter_mut().zip(&t.splits) {
            if let (Some(o), Ok(a)) = (sv.as_object_mut(), chart.resolve(&s.account)) {
                o.insert("account_name".into(), json!(chart.full_name(a.id)));
            }
        }
    }
    Ok(v)
}

fn schedule_json(ctx: &Ctx<'_>, chart: &Chart, s: &Schedule) -> CommandResult<Value> {
    let handled = states(ctx, &s.id)?;
    let next = next_due(s, &handled);
    let currency = template_currency(ctx, &s.template)?;
    let dp = minor_units(currency.as_str());
    let (account, amount) = amount_for(chart, &s.template, None);
    let bill = bill_view(ctx, chart, s, dp)?;
    let (account, amount) = if s.creates_bill() {
        (None, -bill_total(s))
    } else {
        (account, amount)
    };
    let mut v = serde_json::to_value(s)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("template".into(), template_json(chart, &s.template)?);
        if s.creates_bill() {
            o.insert(
                "account".into(),
                bill.get("money")
                    .and_then(|m| m.get("account"))
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            o.insert("bill_view".into(), bill);
        }
        o.insert(
            "next_due".into(),
            match next {
                Some((nominal, due)) => json!({"nominal": nominal, "due": due}),
                None => Value::Null,
            },
        );
        o.insert("ended".into(), json!(next.is_none()));
        o.insert(
            "summary".into(),
            json!(summary(&s.recurrence, s.start, s.end)),
        );
        if !s.creates_bill() {
            o.insert(
                "account".into(),
                json!(account.map(|a| json!({"id": a, "full_name": chart.full_name(a)}))),
            );
        }
        o.insert("amount".into(), json!(format_amount(amount, Some(dp))));
        o.insert(
            "entered".into(),
            json!(
                handled
                    .values()
                    .filter(|x| x.state == State::Entered)
                    .count()
            ),
        );
        o.insert(
            "skipped".into(),
            json!(
                handled
                    .values()
                    .filter(|x| x.state == State::Skipped)
                    .count()
            ),
        );
    }
    Ok(v)
}

// ----- create / update ----------------------------------------------------------

fn default_remind() -> u32 {
    7
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    /// 1–100 characters; defaults to the template payee.
    #[serde(default)]
    name: Option<String>,
    /// The transaction to enter; leave out (or give no splits) with `creates: "bill"`.
    #[serde(default)]
    template: Template,
    recurrence: Recurrence,
    /// First nominal date (inclusive).
    start: Date,
    #[serde(default)]
    end: EndRule,
    #[serde(default)]
    weekend: WeekendRule,
    #[serde(default)]
    amount_kind: AmountKind,
    #[serde(default)]
    auto_enter: bool,
    /// 0–60 days; default 7.
    #[serde(default = "default_remind")]
    remind_days: u32,
    #[serde(default)]
    status: Status,
    /// `"bill"`: the schedule creates a bill when entered (needs the bills module;
    /// `auto_enter` stays off).
    #[serde(default)]
    creates: Option<String>,
    /// With `creates: "bill"`: vendor, terms and bill lines.
    #[serde(default)]
    bill: Option<BillTemplate>,
}

/// Validate a bill schedule's vendor, terms and lines; accounts become ids and
/// every line gets its amount.
fn check_bill(ctx: &Ctx<'_>, s: &mut Schedule) -> CommandResult<()> {
    if !crate::is_module_enabled(ctx.tx, "bills")? {
        return Err(CommandError::Rejected(
            "schedules that create a bill need the bills feature on".into(),
        ));
    }
    if !s.template.splits.is_empty() {
        return Err(CommandError::InvalidArgs(
            "a schedule that creates a bill has bill lines, not splits".into(),
        ));
    }
    if s.auto_enter {
        return Err(CommandError::InvalidArgs(
            "a schedule that creates a bill is never entered automatically".into(),
        ));
    }
    if s.investment.is_some() {
        return Err(CommandError::InvalidArgs(
            "a schedule creates a bill or an investment action, not both".into(),
        ));
    }
    let b = s
        .bill
        .as_mut()
        .ok_or_else(|| CommandError::InvalidArgs("give the bill (vendor and lines)".into()))?;
    let vendor = crate::purchases::vendor_of(ctx, &b.vendor)?;
    b.vendor = vendor.id.clone();
    if let Some(t) = &b.terms {
        b.terms = Some(crate::purchases::check_terms_value(t)?);
    }
    let (computed, total) = crate::purchases::compute_lines(ctx, &b.lines)?;
    if total <= Decimal::ZERO {
        return Err(CommandError::InvalidArgs(
            "a bill's total must be more than zero".into(),
        ));
    }
    for (l, c) in b.lines.iter_mut().zip(&computed) {
        if l.item.is_none() {
            l.account = Some(c.account.to_string());
        }
        l.amount = Some(c.amount);
    }
    b.memo = b
        .memo
        .take()
        .map(|m| m.trim().to_owned())
        .filter(|m| !m.is_empty());
    s.template.payee = Some(vendor.name);
    Ok(())
}

/// Validate fields and resolve the template's accounts to ids.
fn check(ctx: &Ctx<'_>, s: &mut Schedule) -> CommandResult<()> {
    match s.creates.as_deref() {
        None => {
            if s.bill.is_some() {
                return Err(CommandError::InvalidArgs(
                    "bill lines need creates: \"bill\"".into(),
                ));
            }
        }
        Some(CREATES_BILL) => {}
        Some(other) => {
            return Err(CommandError::InvalidArgs(format!(
                "creates is \"bill\" (not {other:?})"
            )));
        }
    }
    if s.creates_bill() {
        check_bill(ctx, s)?;
    }
    let chart = Chart::load(ctx)?;
    for (i, split) in s.template.splits.iter_mut().enumerate() {
        let acc = chart
            .resolve(&split.account)
            .map_err(|e| CommandError::InvalidArgs(format!("split {i}: {e}")))?;
        split.account = acc.id.to_string();
    }
    let clean = |v: &mut Option<String>| {
        *v = v
            .take()
            .map(|x| x.trim().to_owned())
            .filter(|x| !x.is_empty());
    };
    clean(&mut s.template.payee);
    clean(&mut s.template.memo);
    clean(&mut s.template.number);
    if !s.creates_bill() {
        check_template_input(ctx, s)?;
    }
    finish_check(s)
}

fn check_template_input(ctx: &Ctx<'_>, s: &Schedule) -> CommandResult<()> {
    check_template(
        ctx,
        TxnInput {
            date: s.start,
            currency: s.template.currency.clone(),
            payee: s.template.payee.clone(),
            number: s.template.number.clone(),
            memo: s.template.memo.clone(),
            splits: s.template.splits.clone(),
        },
    )?;
    Ok(())
}

fn finish_check(s: &mut Schedule) -> CommandResult<()> {
    s.name = s.name.trim().to_owned();
    if s.name.is_empty() {
        s.name = s.template.payee.clone().unwrap_or_default();
    }
    let n = s.name.chars().count();
    if !(1..=100).contains(&n) {
        return Err(CommandError::InvalidArgs(
            "a schedule needs a name (or a payee) of 1–100 characters".into(),
        ));
    }
    validate_recurrence(&s.recurrence)?;
    if let EndRule::After(0) = s.end {
        return Err(CommandError::InvalidArgs(
            "end after needs at least 1 occurrence".into(),
        ));
    }
    if let EndRule::On(d) = s.end
        && d < s.start
    {
        return Err(CommandError::InvalidArgs(
            "the end date is before the start date".into(),
        ));
    }
    if s.remind_days > 60 {
        return Err(CommandError::InvalidArgs("remind_days must be 0–60".into()));
    }
    Ok(())
}

fn create(ctx: &Ctx<'_>, a: CreateArgs) -> CommandResult<Value> {
    let mut s = Schedule {
        id: format!("sch_{}", mk_core::Id::<()>::new()),
        name: a.name.unwrap_or_default(),
        template: a.template,
        recurrence: a.recurrence,
        start: a.start,
        end: a.end,
        weekend: a.weekend,
        amount_kind: a.amount_kind,
        auto_enter: a.auto_enter,
        remind_days: a.remind_days,
        status: a.status,
        created_at: now_rfc3339()?,
        investment: None,
        creates: a.creates,
        bill: a.bill,
    };
    check(ctx, &mut s)?;
    save(ctx, &s)?;
    schedule_json(ctx, &Chart::load(ctx)?, &s)
}

/// `schedule.create` from JSON arguments (Make a bill on a subscription).
pub(crate) fn create_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    create(ctx, crate::parse_args(args)?)
}

/// Create a schedule (arguments as `schedule.create`) that enters an investment
/// action (coupon reminders of a bond).
pub(crate) fn create_investment(
    ctx: &Ctx<'_>,
    args: Value,
    investment: Value,
) -> CommandResult<Value> {
    let a: CreateArgs = crate::parse_args(args)?;
    let mut s = Schedule {
        id: format!("sch_{}", mk_core::Id::<()>::new()),
        name: a.name.unwrap_or_default(),
        template: a.template,
        recurrence: a.recurrence,
        start: a.start,
        end: a.end,
        weekend: a.weekend,
        amount_kind: a.amount_kind,
        auto_enter: a.auto_enter,
        remind_days: a.remind_days,
        status: a.status,
        created_at: now_rfc3339()?,
        investment: Some(investment),
        creates: None,
        bill: None,
    };
    check(ctx, &mut s)?;
    save(ctx, &s)?;
    schedule_json(ctx, &Chart::load(ctx)?, &s)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    /// Schedule id or name.
    schedule: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    template: Option<Template>,
    #[serde(default)]
    recurrence: Option<Recurrence>,
    #[serde(default)]
    start: Option<Date>,
    #[serde(default)]
    end: Option<EndRule>,
    #[serde(default)]
    weekend: Option<WeekendRule>,
    #[serde(default)]
    amount_kind: Option<AmountKind>,
    #[serde(default)]
    auto_enter: Option<bool>,
    #[serde(default)]
    remind_days: Option<u32>,
    #[serde(default)]
    status: Option<Status>,
    /// A bill schedule's vendor, terms and lines.
    #[serde(default)]
    bill: Option<BillTemplate>,
}

fn update(ctx: &Ctx<'_>, a: UpdateArgs) -> CommandResult<Value> {
    let old = find(ctx, &a.schedule)?;
    let mut s = old.clone();
    if let Some(v) = a.name {
        s.name = v;
    }
    if let Some(v) = a.template {
        s.template = v;
    }
    if let Some(v) = a.recurrence {
        s.recurrence = v;
    }
    if let Some(v) = a.start {
        s.start = v;
    }
    if let Some(v) = a.end {
        s.end = v;
    }
    if let Some(v) = a.weekend {
        s.weekend = v;
    }
    if let Some(v) = a.amount_kind {
        s.amount_kind = v;
    }
    if let Some(v) = a.auto_enter {
        s.auto_enter = v;
    }
    if let Some(v) = a.remind_days {
        s.remind_days = v;
    }
    if let Some(v) = a.status {
        s.status = v;
    }
    if let Some(v) = a.bill {
        s.bill = Some(v);
    }
    check(ctx, &mut s)?;
    if s != old {
        save(ctx, &s)?;
    }
    schedule_json(ctx, &Chart::load(ctx)?, &s)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RefArgs {
    /// Schedule id or name.
    schedule: String,
}

fn set_status(ctx: &Ctx<'_>, reference: &str, status: Status) -> CommandResult<Value> {
    let mut s = find(ctx, reference)?;
    if s.status == status {
        return Err(CommandError::Rejected(format!(
            "the schedule is already {}",
            if status == Status::Active {
                "active"
            } else {
                "paused"
            }
        )));
    }
    s.status = status;
    save(ctx, &s)?;
    schedule_json(ctx, &Chart::load(ctx)?, &s)
}

fn pause(ctx: &Ctx<'_>, a: RefArgs) -> CommandResult<Value> {
    set_status(ctx, &a.schedule, Status::Paused)
}

fn resume(ctx: &Ctx<'_>, a: RefArgs) -> CommandResult<Value> {
    set_status(ctx, &a.schedule, Status::Active)
}

fn delete(ctx: &Ctx<'_>, a: RefArgs) -> CommandResult<Value> {
    let s = find(ctx, &a.schedule)?;
    for d in ctx.tx.docs(DocTable::ScheduleOccurrence, Some(&s.id))? {
        ctx.save_doc(DocTable::ScheduleOccurrence, &d.id, None)?;
    }
    ctx.save_doc(DocTable::Schedule, &s.id, None)?;
    Ok(json!({"deleted": s.id, "name": s.name}))
}

fn get(ctx: &Ctx<'_>, a: RefArgs) -> CommandResult<Value> {
    let s = find(ctx, &a.schedule)?;
    schedule_json(ctx, &Chart::load(ctx)?, &s)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    /// Only schedules with a split in this account or its sub-accounts.
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    status: Option<Status>,
}

fn touches(chart: &Chart, s: &Schedule, accounts: &[AccountId]) -> bool {
    s.template.splits.iter().any(|sp| {
        chart
            .resolve(&sp.account)
            .is_ok_and(|a| accounts.contains(&a.id))
    })
}

fn list(ctx: &Ctx<'_>, a: ListArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let filter = match &a.account {
        Some(r) => Some(chart.subtree(chart.resolve(r)?.id)),
        None => None,
    };
    let mut out = Vec::new();
    for s in load_all(ctx)? {
        if a.status.is_some_and(|st| st != s.status) {
            continue;
        }
        if filter.as_ref().is_some_and(|f| !touches(&chart, &s, f)) {
            continue;
        }
        out.push(schedule_json(ctx, &chart, &s)?);
    }
    Ok(Value::Array(out))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FromTxnArgs {
    /// Transaction id to repeat.
    txn: TransactionId,
    recurrence: Recurrence,
    #[serde(default)]
    name: Option<String>,
    /// First nominal date; defaults to the transaction's date plus one period.
    #[serde(default)]
    start: Option<Date>,
    #[serde(default)]
    end: EndRule,
    #[serde(default)]
    weekend: WeekendRule,
    #[serde(default)]
    amount_kind: AmountKind,
    #[serde(default)]
    auto_enter: bool,
    #[serde(default = "default_remind")]
    remind_days: u32,
}

fn from_txn(ctx: &Ctx<'_>, a: FromTxnArgs) -> CommandResult<Value> {
    let t = ctx
        .tx
        .transaction(a.txn)?
        .ok_or_else(|| CommandError::NotFound(format!("transaction {}", a.txn)))?;
    validate_recurrence(&a.recurrence)?;
    let start = match a.start {
        Some(d) => d,
        None => one_period_after(&a.recurrence, t.date).ok_or_else(|| {
            CommandError::InvalidArgs("no date one period after the transaction".into())
        })?,
    };
    let template = Template {
        payee: t.payee.clone(),
        number: None,
        memo: t.memo.clone(),
        currency: Some(t.currency.clone()),
        splits: t
            .splits
            .iter()
            .map(|s| SplitInput {
                account: s.account.to_string(),
                value: s.value,
                quantity: (s.quantity != s.value).then_some(s.quantity),
                memo: s.memo.clone(),
                class: s.tags.class.clone(),
                location: s.tags.location.clone(),
                fund: s.tags.fund.clone(),
                project: s.tags.project.clone(),
                billable: s.tags.project.as_ref().map(|_| s.tags.billable),
            })
            .collect(),
    };
    create(
        ctx,
        CreateArgs {
            name: a.name,
            template,
            recurrence: a.recurrence,
            start,
            end: a.end,
            weekend: a.weekend,
            amount_kind: a.amount_kind,
            auto_enter: a.auto_enter,
            remind_days: a.remind_days,
            status: Status::Active,
            creates: None,
            bill: None,
        },
    )
}

// ----- upcoming --------------------------------------------------------------------

fn today_or(today: Option<Date>) -> CommandResult<Date> {
    match today {
        Some(d) => Ok(d),
        None => Ok(today_utc()?),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpcomingArgs {
    /// Default today − 365 days. Pending occurrences before it are still returned,
    /// flagged `before_from`.
    #[serde(default)]
    from: Option<Date>,
    /// Default today + 30 days.
    #[serde(default)]
    to: Option<Date>,
    /// Only schedules with a split in this account; amounts as seen from it.
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    include_paused: bool,
    /// The current date (default: the local date of the machine).
    #[serde(default)]
    today: Option<Date>,
}

fn shift(d: Date, days: i32) -> CommandResult<Date> {
    d.add_days(days)
        .ok_or_else(|| CommandError::InvalidArgs(format!("date out of range near {d}")))
}

fn upcoming(ctx: &Ctx<'_>, a: UpcomingArgs) -> CommandResult<Value> {
    let today = today_or(a.today)?;
    let from = match a.from {
        Some(d) => d,
        None => shift(today, -365)?,
    };
    let to = match a.to {
        Some(d) => d,
        None => shift(today, 30)?,
    };
    let chart = Chart::load(ctx)?;
    let account = match &a.account {
        Some(r) => Some(chart.resolve(r)?.id),
        None => None,
    };
    let mut rows: Vec<(Date, String, Value)> = Vec::new();
    for s in load_all(ctx)? {
        if s.status == Status::Paused && !a.include_paused {
            continue;
        }
        if account.is_some_and(|acc| !touches(&chart, &s, &[acc])) {
            continue;
        }
        let handled = states(ctx, &s.id)?;
        let currency = template_currency(ctx, &s.template)?;
        let dp = minor_units(currency.as_str());
        let (seen_from, amount) = amount_for(&chart, &s.template, account);
        let bill = bill_view(ctx, &chart, &s, dp)?;
        let (amount, money) = if s.creates_bill() {
            (
                -bill_total(&s),
                bill.get("money").cloned().unwrap_or(Value::Null),
            )
        } else {
            (amount, money_view(&chart, &s.template, dp))
        };
        let soon = shift(today, i32::try_from(s.remind_days).unwrap_or(60))?;
        // A weekend move shifts a due date by at most two days.
        for (nominal, due) in pending(&s, &handled, Some(shift(to, 2)?)) {
            if due > to {
                continue;
            }
            let when = if due < today {
                "overdue"
            } else if due <= soon {
                "due_soon"
            } else {
                "later"
            };
            rows.push((
                due,
                s.name.to_lowercase(),
                json!({
                    "schedule": s.id,
                    "name": s.name,
                    "nominal": nominal,
                    "due": due,
                    "payee": s.template.payee,
                    "amount": format_amount(amount, Some(dp)),
                    "account": seen_from.map(|a| json!({"id": a, "full_name": chart.full_name(a)})),
                    "amount_kind": s.amount_kind,
                    "when": when,
                    "before_from": due < from,
                    "remind_days": s.remind_days,
                    "summary": summary(&s.recurrence, s.start, s.end),
                    "category": other_side(&chart, &s.template, seen_from),
                    "money": money,
                    "creates": s.creates,
                }),
            ));
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    Ok(json!({
        "today": today,
        "from": from,
        "to": to,
        "occurrences": rows.into_iter().map(|r| r.2).collect::<Vec<_>>(),
    }))
}

/// The template as seen from its first asset or liability account (what a front end
/// shows in Account / Category / Amount columns), or null when it has none.
fn money_view(chart: &Chart, t: &Template, dp: u32) -> Value {
    let money = t.splits.iter().find_map(|s| {
        chart.resolve(&s.account).ok().filter(|a| {
            matches!(
                a.account_type,
                mk_core::AccountType::Asset | mk_core::AccountType::Liability
            )
        })
    });
    match money {
        Some(a) => {
            let (_, amount) = amount_for(chart, t, Some(a.id));
            json!({
                "account": {"id": a.id, "full_name": chart.full_name(a.id)},
                "amount": format_amount(amount, Some(dp)),
                "category": other_side(chart, t, Some(a.id)),
            })
        }
        None => Value::Null,
    }
}

/// The other side of the template as seen from `account`: one account's full name,
/// or "Split (n)" for several.
fn other_side(chart: &Chart, t: &Template, account: Option<AccountId>) -> String {
    let others: Vec<String> = t
        .splits
        .iter()
        .filter_map(|s| chart.resolve(&s.account).ok())
        .filter(|a| Some(a.id) != account)
        .map(|a| chart.full_name(a.id))
        .collect();
    match others.as_slice() {
        [one] => one.clone(),
        many => format!("Split ({})", many.len()),
    }
}

// ----- enter / skip / run_due ------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EnterArgs {
    /// Schedule id or name.
    schedule: String,
    /// Nominal date of the occurrence; default the earliest pending one.
    #[serde(default)]
    date: Option<Date>,
    /// Date of the created transaction; default the occurrence's due date.
    #[serde(default)]
    entry_date: Option<Date>,
    #[serde(default)]
    payee: Option<String>,
    #[serde(default)]
    memo: Option<String>,
    #[serde(default)]
    number: Option<String>,
    /// A full replacement split list (e.g. the actual amount of an estimated bill).
    #[serde(default)]
    splits: Option<Vec<SplitInput>>,
}

/// The occurrence to act on: the given nominal date (checked) or the earliest pending.
fn pick(
    s: &Schedule,
    handled: &BTreeMap<Date, Occurrence>,
    date: Option<Date>,
) -> CommandResult<Date> {
    match date {
        Some(d) => {
            if !is_nominal(s, d) {
                return Err(CommandError::InvalidArgs(format!(
                    "{d} is not an occurrence date of schedule {:?}",
                    s.name
                )));
            }
            if let Some(o) = handled.get(&d) {
                let what = match o.state {
                    State::Entered => "entered",
                    State::Skipped => "skipped",
                };
                return Err(CommandError::Conflict(format!(
                    "the {d} occurrence of {:?} was already {what}",
                    s.name
                )));
            }
            Ok(d)
        }
        None => next_due(s, handled).map(|(n, _)| n).ok_or_else(|| {
            CommandError::Rejected(format!("schedule {:?} has no pending occurrence", s.name))
        }),
    }
}

/// The arguments `bill.create` takes for a bill schedule's occurrence: the bill
/// lines and vendor, dated `date`, linked back with `from_schedule`.
pub(crate) fn bill_args_for(s: &Schedule, nominal: Date, date: Date) -> Value {
    let Some(b) = &s.bill else {
        return Value::Null;
    };
    let mut v = json!({
        "vendor": b.vendor,
        "date": date,
        "lines": b.lines,
        "from_schedule": {"schedule": s.id, "date": nominal},
    });
    if let Some(o) = v.as_object_mut() {
        if let Some(t) = &b.terms {
            o.insert("terms".into(), t.clone());
        }
        if let Some(m) = &b.memo {
            o.insert("memo".into(), json!(m));
        }
    }
    v
}

/// `bill.create` with `from_schedule`: the occurrence must be pending; returns the
/// schedule id, nominal and due dates.
pub(crate) fn pending_bill_occurrence(
    ctx: &Ctx<'_>,
    schedule: &str,
    date: Option<Date>,
) -> CommandResult<(String, Date, Date)> {
    let s = find(ctx, schedule)?;
    if !s.creates_bill() {
        return Err(CommandError::InvalidArgs(format!(
            "schedule {:?} does not create a bill",
            s.name
        )));
    }
    let handled = states(ctx, &s.id)?;
    let nominal = pick(&s, &handled, date)?;
    Ok((s.id, nominal, due_date(nominal, s.weekend)))
}

/// Record a bill schedule's occurrence as entered by the bill it created.
pub(crate) fn mark_bill_entered(
    ctx: &Ctx<'_>,
    schedule: &str,
    nominal: Date,
    due: Date,
    txn: Option<TransactionId>,
    bill: &str,
) -> CommandResult<()> {
    save_occurrence(
        ctx,
        &Occurrence {
            schedule: schedule.to_owned(),
            nominal,
            due,
            state: State::Entered,
            txn,
            bill: Some(bill.to_owned()),
        },
    )
}

fn enter_one(ctx: &Ctx<'_>, s: &Schedule, a: EnterArgs) -> CommandResult<Value> {
    let handled = states(ctx, &s.id)?;
    let nominal = pick(s, &handled, a.date)?;
    let due = due_date(nominal, s.weekend);
    if s.creates_bill() {
        // Front ends open the bill editor (`bill_args` from `schedule.get`); entering
        // directly creates the bill as scheduled.
        if a.splits.is_some() {
            return Err(CommandError::InvalidArgs(
                "a bill schedule takes no splits; change the amounts with bill.create and from_schedule".into(),
            ));
        }
        let mut args = bill_args_for(s, nominal, a.entry_date.unwrap_or(due));
        if let (Some(o), Some(m)) = (args.as_object_mut(), a.memo) {
            o.insert("memo".into(), json!(m));
        }
        let mut v = crate::purchases::create_bill_json(ctx, args)?;
        if let Some(o) = v.as_object_mut() {
            o.insert("schedule".into(), json!(s.id));
            o.insert("occurrence".into(), json!(nominal));
        }
        return Ok(v);
    }
    let pick_text = |o: Option<String>, t: &Option<String>| o.or_else(|| t.clone());
    let t = if let Some(inv) = &s.investment {
        // The amount: the template's (or the entered splits') positive split.
        let splits = a.splits.unwrap_or_else(|| s.template.splits.clone());
        let amount = splits
            .iter()
            .map(|x| x.value)
            .find(|v| v.is_sign_positive() && !v.is_zero())
            .unwrap_or_default();
        crate::inv_kinds::enter_scheduled(
            ctx,
            inv,
            a.entry_date.unwrap_or(due),
            amount,
            pick_text(a.memo, &s.template.memo),
        )?
    } else {
        create_txn(
            ctx,
            TxnInput {
                date: a.entry_date.unwrap_or(due),
                currency: s.template.currency.clone(),
                payee: pick_text(a.payee, &s.template.payee),
                number: pick_text(a.number, &s.template.number),
                memo: pick_text(a.memo, &s.template.memo),
                splits: a.splits.unwrap_or_else(|| s.template.splits.clone()),
            },
        )?
    };
    save_occurrence(
        ctx,
        &Occurrence {
            schedule: s.id.clone(),
            nominal,
            due,
            state: State::Entered,
            txn: Some(t.id),
            bill: None,
        },
    )?;
    let chart = Chart::load(ctx)?;
    let mut v = crate::txns::txn_json(&chart, &t)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("schedule".into(), json!(s.id));
        o.insert("occurrence".into(), json!(nominal));
    }
    Ok(v)
}

fn enter(ctx: &Ctx<'_>, a: EnterArgs) -> CommandResult<Value> {
    let s = find(ctx, &a.schedule)?;
    enter_one(ctx, &s, a)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SkipArgs {
    /// Schedule id or name.
    schedule: String,
    /// Nominal date of the occurrence; default the earliest pending one.
    #[serde(default)]
    date: Option<Date>,
}

fn skip(ctx: &Ctx<'_>, a: SkipArgs) -> CommandResult<Value> {
    let s = find(ctx, &a.schedule)?;
    let handled = states(ctx, &s.id)?;
    let nominal = pick(&s, &handled, a.date)?;
    save_occurrence(
        ctx,
        &Occurrence {
            schedule: s.id.clone(),
            nominal,
            due: due_date(nominal, s.weekend),
            state: State::Skipped,
            txn: None,
            bill: None,
        },
    )?;
    Ok(json!({"schedule": s.id, "name": s.name, "skipped": nominal}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunDueArgs {
    /// The current date (default: the local date of the machine).
    #[serde(default)]
    today: Option<Date>,
}

fn run_due(ctx: &Ctx<'_>, a: RunDueArgs) -> CommandResult<Value> {
    let today = today_or(a.today)?;
    ctx.actor_as.replace(Some(SCHEDULE_ACTOR.to_owned()));
    ctx.quiet_noop.set(true);
    let mut due_list: Vec<(Date, String, Schedule, Date)> = Vec::new();
    for s in load_all(ctx)? {
        if s.status != Status::Active || !s.auto_enter || s.creates_bill() {
            continue;
        }
        let handled = states(ctx, &s.id)?;
        for (nominal, due) in pending(&s, &handled, Some(shift(today, 2)?)) {
            if due <= today {
                due_list.push((due, s.name.to_lowercase(), s.clone(), nominal));
            }
        }
    }
    due_list.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let mut entered = Vec::new();
    let mut failed = Vec::new();
    for (due, _, s, nominal) in due_list {
        let base = json!({"schedule": s.id, "name": s.name, "nominal": nominal, "due": due});
        let result = enter_one(
            ctx,
            &s,
            EnterArgs {
                schedule: s.id.clone(),
                date: Some(nominal),
                ..EnterArgs::default()
            },
        );
        let mut row = base;
        match result {
            Ok(t) => {
                if let Some(o) = row.as_object_mut() {
                    o.insert("txn".into(), t.get("id").cloned().unwrap_or(Value::Null));
                }
                entered.push(row);
            }
            Err(e) => {
                if let Some(o) = row.as_object_mut() {
                    o.insert(
                        "error".into(),
                        json!({"code": e.code(), "message": e.to_string()}),
                    );
                }
                failed.push(row);
            }
        }
    }
    Ok(json!({"today": today, "entered": entered, "failed": failed}))
}

// ----- forecast --------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ForecastArgs {
    /// Account id, full name or code.
    account: String,
    /// Last date to project (at most today + 3 years).
    to: Date,
    /// The current date (default: the local date of the machine).
    #[serde(default)]
    today: Option<Date>,
}

fn forecast(ctx: &Ctx<'_>, a: ForecastArgs) -> CommandResult<Value> {
    let today = today_or(a.today)?;
    let (y, m) = Date::shift_month(today.year(), today.month(), 36)
        .ok_or_else(|| CommandError::InvalidArgs("date out of range".into()))?;
    let max = Date::from_ymd_clamped(y, m, today.day())?;
    if a.to > max {
        return Err(CommandError::InvalidArgs(format!(
            "a forecast reaches at most three years ahead ({max})"
        )));
    }
    let chart = Chart::load(ctx)?;
    let acc = chart.resolve(&a.account)?.clone();
    let dp = chart.scale_of(&acc.commodity);
    let raw = crate::balances::raw_balances(ctx, &[acc.id], Some(today), Basis::All)?
        .get(&acc.commodity)
        .copied()
        .unwrap_or(Decimal::ZERO);
    let mut items: Vec<(Date, String, Value, Decimal)> = Vec::new();
    for s in load_all(ctx)? {
        if s.status != Status::Active || !touches(&chart, &s, &[acc.id]) {
            continue;
        }
        let handled = states(ctx, &s.id)?;
        let q = s
            .template
            .splits
            .iter()
            .filter(|sp| chart.resolve(&sp.account).is_ok_and(|x| x.id == acc.id))
            .fold(Decimal::ZERO, |t, sp| {
                t.checked_add(sp.quantity.unwrap_or(sp.value)).unwrap_or(t)
            });
        for (nominal, due) in pending(&s, &handled, Some(shift(a.to, 2)?)) {
            if due > a.to {
                continue;
            }
            // Overdue (and due today, not yet entered) occurrences are dated today.
            let date = due.max(today);
            items.push((
                date,
                s.name.to_lowercase(),
                json!({"schedule": s.id, "name": s.name, "payee": s.template.payee,
                       "nominal": nominal, "due": due, "overdue": due < today}),
                q,
            ));
        }
    }
    items.sort_by(|x, y| x.0.cmp(&y.0).then_with(|| x.1.cmp(&y.1)));
    let display = |r: Decimal| acc.account_type.display(r);
    let mut running = raw;
    let start = display(raw);
    let mut lowest = (start, today);
    let mut rows = Vec::new();
    for (date, _, mut v, q) in items {
        running = running.checked_add(q).unwrap_or(running);
        let bal = display(running);
        if bal < lowest.0 {
            lowest = (bal, date);
        }
        if let Some(o) = v.as_object_mut() {
            o.insert("date".into(), json!(date));
            o.insert("amount".into(), json!(format_amount(display(q), dp)));
            o.insert("balance".into(), json!(format_amount(bal, dp)));
        }
        rows.push(v);
    }
    Ok(json!({
        "account": {"id": acc.id, "full_name": chart.full_name(acc.id)},
        "today": today,
        "to": a.to,
        "balance": format_amount(start, dp),
        "rows": rows,
        "lowest": {"balance": format_amount(lowest.0, dp), "date": lowest.1},
    }))
}

// ----- preview ---------------------------------------------------------------------

fn three() -> usize {
    3
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PreviewArgs {
    recurrence: Recurrence,
    /// First nominal date. With `after` instead, the date one period after it.
    #[serde(default)]
    start: Option<Date>,
    #[serde(default)]
    after: Option<Date>,
    #[serde(default)]
    end: EndRule,
    #[serde(default)]
    weekend: WeekendRule,
    /// How many upcoming dates to list (default 3, at most 24).
    #[serde(default = "three")]
    count: usize,
}

fn preview(_ctx: &Ctx<'_>, a: PreviewArgs) -> CommandResult<Value> {
    validate_recurrence(&a.recurrence)?;
    let start = match (a.start, a.after) {
        (Some(s), _) => s,
        (None, Some(d)) => one_period_after(&a.recurrence, d).ok_or_else(|| {
            CommandError::InvalidArgs("no date one period after that date".into())
        })?,
        (None, None) => return Err(CommandError::InvalidArgs("give start or after".into())),
    };
    let s = Schedule {
        id: String::new(),
        name: String::new(),
        template: Template {
            payee: None,
            number: None,
            memo: None,
            currency: None,
            splits: Vec::new(),
        },
        recurrence: a.recurrence,
        start,
        end: a.end,
        weekend: a.weekend,
        amount_kind: AmountKind::Fixed,
        auto_enter: false,
        remind_days: 0,
        status: Status::Active,
        created_at: String::new(),
        investment: None,
        creates: None,
        bill: None,
    };
    let none = BTreeMap::new();
    let next: Vec<Value> = pending(&s, &none, None)
        .take(a.count.min(24))
        .map(|(n, d)| json!({"nominal": n, "due": d}))
        .collect();
    Ok(json!({
        "start": start,
        "summary": summary(&s.recurrence, s.start, s.end),
        "next": next,
    }))
}
