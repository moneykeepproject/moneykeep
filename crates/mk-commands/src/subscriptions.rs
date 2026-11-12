//! Subscription audit (subscriptions.md): detection of repeating payments over the
//! money accounts, the Suggested list with remembered answers, subscriptions and
//! recurring bills with their linked charges, price history with reason notes,
//! alerts for Home → Attention, totals, reviews and the dashboard block's data.
//!
//! Non-posting: subscriptions, answers, notes and settings are book documents,
//! audited and undoable like every other record.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use mk_core::subscription::{self as sub, Band, Cadence};
use mk_core::{AccountSubtype, Date, Decimal};
use mk_import::normalise_payee;
use mk_store::{Doc, DocTable, TxnFilter, today_utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::module_settings as ms;
use crate::sales::{date_ord, invalid, new_id, rejected};
use crate::{CommandError, CommandResult, Ctx, Registry};

/// The module.
pub(crate) const MODULE: &str = "subscriptions";

/// Where dismissed alert keys are kept (not a schema setting; audited and undoable).
const DISMISSED_KEY: &str = "subscriptions.dismissed_alerts";

pub(crate) fn register(r: &mut Registry) {
    let m = MODULE;
    r.add_in(
        m,
        "subscription.list",
        "Subscriptions and recurring bills with totals, monthly equivalents, the year-ago comparison and the biggest increases",
        false,
        list,
    );
    r.add_in(
        m,
        "subscription.get",
        "One subscription or recurring bill: fields, charges, price history and what Make a reminder fills in",
        false,
        get,
    );
    r.add_in(
        m,
        "subscription.make_bill",
        "Make a bill from a subscription: a repeating bill in Scheduled (make_bill_as scheduled_bill, the default) or the bill to open in the bill editor (bill_now)",
        true,
        make_bill,
    );
    r.add_in(
        m,
        "subscription.suggestions",
        "Repeating payments found in the money accounts that are not answered yet",
        false,
        suggestions,
    );
    r.add_in(
        m,
        "subscription.dismissed",
        "Suggestions answered Not recurring (they are never suggested again)",
        false,
        dismissed,
    );
    r.add_in(
        m,
        "subscription.answer",
        "Answer a suggestion: subscription, bill (recurring bill) or not_recurring",
        true,
        answer,
    );
    r.add_in(
        m,
        "subscription.restore_suggestion",
        "Bring back a suggestion answered Not recurring",
        true,
        restore_suggestion,
    );
    r.add_in(
        m,
        "subscription.create",
        "Add a subscription or recurring bill by hand (name, payee to match, account, cadence, price, next charge)",
        true,
        create,
    );
    r.add_in(
        m,
        "subscription.update",
        "Change a subscription's fields",
        true,
        update,
    );
    r.add_in(
        m,
        "subscription.delete",
        "Delete a subscription or recurring bill (its transactions stay)",
        true,
        delete,
    );
    r.add_in(
        m,
        "subscription.set_status",
        "Mark a subscription active, paused, cancelled (on a date) or in a free trial",
        true,
        set_status,
    );
    r.add_in(
        m,
        "subscription.still_active",
        "Answer a missing-charge alert with Still active (the next charge moves on one period)",
        true,
        still_active,
    );
    r.add_in(
        m,
        "subscription.add_charge",
        "Link a missed transaction to a subscription (a transaction belongs to one subscription only)",
        true,
        add_charge,
    );
    r.add_in(
        m,
        "subscription.remove_charge",
        "Unlink a wrong transaction from a subscription",
        true,
        remove_charge,
    );
    r.add_in(
        m,
        "subscription.set_reason",
        "Write the reason note of a price history row",
        true,
        set_reason,
    );
    r.add_in(
        m,
        "subscription.alerts",
        "Open subscription alerts: price rises, charges after cancelling, missing charges, renewals, trials ending, duplicates",
        false,
        alerts_cmd,
    );
    r.add_in(
        m,
        "subscription.dismiss_alert",
        "Dismiss one subscription alert",
        true,
        dismiss_alert,
    );
    r.add_in(
        m,
        "subscription.review",
        "The review queue (active subscriptions, oldest review first) and the saved total of the last 12 months",
        false,
        review,
    );
    r.add_in(
        m,
        "subscription.review_answer",
        "Answer one review step: keep, cancel or downgrade (with the new price)",
        true,
        review_answer,
    );
    r.add_in(
        m,
        "subscription.summary",
        "The monthly total, the next three charges and the number of open alerts (the dashboard block)",
        false,
        summary,
    );
}

// ----- stored records ------------------------------------------------------------------

/// A price history row entered by hand (a review's downgrade).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Manual {
    key: String,
    date: Date,
    old: Decimal,
    new: Decimal,
}

/// An amount saved in a review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Saved {
    date: Date,
    amount: Decimal,
    how: String,
}

/// A subscription or recurring bill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Sub {
    id: String,
    name: String,
    /// `subscription` or `bill`.
    kind: String,
    /// The payee to match (normalised) and how: `equals` or `contains`.
    payee: String,
    #[serde(default = "equals")]
    how: String,
    #[serde(default)]
    account: Option<String>,
    cadence: Cadence,
    /// The expected amount when entered by hand or by a downgrade, from `price_on`.
    #[serde(default)]
    price: Option<Decimal>,
    #[serde(default)]
    price_on: Option<Date>,
    /// An edited next charge date (otherwise computed from the last charge).
    #[serde(default)]
    next_charge: Option<Date>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default = "active")]
    status: String,
    #[serde(default)]
    cancelled_on: Option<Date>,
    #[serde(default)]
    trial_ends: Option<Date>,
    #[serde(default)]
    used_by: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    reviewed_on: Option<Date>,
    /// Transactions linked by hand, and matching ones unlinked by hand.
    #[serde(default)]
    added: Vec<String>,
    #[serde(default)]
    removed: Vec<String>,
    /// Reason notes by history row key.
    #[serde(default)]
    reasons: BTreeMap<String, String>,
    #[serde(default)]
    manual: Vec<Manual>,
    #[serde(default)]
    saved: Vec<Saved>,
    created: Date,
    /// The suggestion answer this record came from.
    #[serde(default)]
    answer: Option<String>,
}

fn equals() -> String {
    "equals".into()
}

fn active() -> String {
    "active".into()
}

/// A suggestion's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Answer {
    payee: String,
    account: String,
    /// `subscription`, `bill` or `not_recurring`.
    answer: String,
    #[serde(default)]
    display: String,
    #[serde(default)]
    subscription: Option<String>,
    #[serde(default)]
    on: Option<Date>,
}

fn load_subs(ctx: &Ctx<'_>) -> CommandResult<Vec<Sub>> {
    let mut out = Vec::new();
    for d in ctx.tx.docs(DocTable::Subscription, None)? {
        out.push(
            serde_json::from_value(d.json)
                .map_err(|e| CommandError::Integrity(format!("bad subscription: {e}")))?,
        );
    }
    Ok(out)
}

impl Sub {
    fn blank(created: Date) -> Self {
        Sub {
            id: String::new(),
            name: String::new(),
            kind: "subscription".into(),
            payee: String::new(),
            how: equals(),
            account: None,
            cadence: Cadence::Monthly,
            price: None,
            price_on: None,
            next_charge: None,
            category: None,
            status: active(),
            cancelled_on: None,
            trial_ends: None,
            used_by: String::new(),
            link: String::new(),
            notes: String::new(),
            reviewed_on: None,
            added: Vec::new(),
            removed: Vec::new(),
            reasons: BTreeMap::new(),
            manual: Vec::new(),
            saved: Vec::new(),
            created,
            answer: None,
        }
    }

    fn is_sub(&self) -> bool {
        self.kind == "subscription"
    }
}

fn save_sub(ctx: &Ctx<'_>, s: &Sub, ord: i64) -> CommandResult<()> {
    let v = serde_json::to_value(s)?;
    ctx.save_doc(
        DocTable::Subscription,
        &s.id,
        Some(&Doc {
            id: s.id.clone(),
            grp: s.account.clone(),
            ord,
            json: v,
        }),
    )
}

/// Save a changed subscription keeping its order.
fn resave(ctx: &Ctx<'_>, s: &Sub) -> CommandResult<()> {
    let ord = ctx
        .tx
        .get_doc(DocTable::Subscription, &s.id)?
        .map_or_else(|| date_ord(s.created), |d| d.ord);
    save_sub(ctx, s, ord)
}

fn find_sub(ctx: &Ctx<'_>, r: &str) -> CommandResult<Sub> {
    let all = load_subs(ctx)?;
    if let Some(s) = all.iter().find(|s| s.id == r) {
        return Ok(s.clone());
    }
    let named: Vec<&Sub> = all
        .iter()
        .filter(|s| s.name.eq_ignore_ascii_case(r.trim()))
        .collect();
    match named.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(CommandError::NotFound(format!("subscription {r:?}"))),
        _ => Err(invalid(format!(
            "several subscriptions are named {r:?}; use the id"
        ))),
    }
}

// ----- charges -------------------------------------------------------------------------

/// One outgoing payment from a money account.
#[derive(Debug, Clone)]
struct Charge {
    txn: String,
    date: Date,
    account: String,
    payee: String,
    norm: String,
    amount: Decimal,
    category: Option<String>,
}

type Cache = HashMap<String, ((i64, i64), Arc<Vec<Charge>>)>;

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// The accounts detection runs over: bank, cash, credit card and other liability.
fn is_money(s: AccountSubtype) -> bool {
    matches!(
        s,
        AccountSubtype::Bank
            | AccountSubtype::Cash
            | AccountSubtype::CreditCard
            | AccountSubtype::OtherLiability
    )
}

/// Every outgoing payment (oldest first), cached until the book changes.
fn charges(ctx: &Ctx<'_>) -> CommandResult<Arc<Vec<Charge>>> {
    let book = ctx.tx.info()?.id;
    let stamp = ctx.tx.change_stamp()?;
    if let Ok(g) = CACHE.lock()
        && let Some((s, c)) = g.as_ref().and_then(|m| m.get(&book))
        && *s == stamp
    {
        return Ok(c.clone());
    }
    let fresh = Arc::new(load_charges(ctx)?);
    if let Ok(mut g) = CACHE.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        if m.len() > 16 {
            m.clear();
        }
        m.insert(book, (stamp, fresh.clone()));
    }
    Ok(fresh)
}

fn load_charges(ctx: &Ctx<'_>) -> CommandResult<Vec<Charge>> {
    let chart = Chart::load(ctx)?;
    let money: Vec<mk_core::AccountId> = chart
        .accounts
        .iter()
        .filter(|a| is_money(a.subtype))
        .map(|a| a.id)
        .collect();
    if money.is_empty() {
        return Ok(Vec::new());
    }
    let any_money = |id: mk_core::AccountId| chart.get(id).is_some_and(|a| a.subtype.is_money());
    let txns = ctx.tx.transactions(&TxnFilter {
        accounts: Some(money.clone()),
        ..TxnFilter::default()
    })?;
    let mut out = Vec::new();
    for t in txns {
        // Transfers between money accounts are not payments.
        let category = t
            .splits
            .iter()
            .filter(|s| !any_money(s.account))
            .max_by(|a, b| a.value.cmp(&b.value))
            .map(|s| s.account.to_string());
        if category.is_none() {
            continue;
        }
        let payee = t.payee.clone().unwrap_or_default();
        let norm = normalise_payee(&payee);
        if norm.is_empty() {
            continue;
        }
        for s in &t.splits {
            if money.contains(&s.account) && s.quantity.is_sign_negative() && !s.quantity.is_zero()
            {
                out.push(Charge {
                    txn: t.id.to_string(),
                    date: t.date,
                    account: s.account.to_string(),
                    payee: payee.clone(),
                    norm: norm.clone(),
                    amount: -s.quantity,
                    category: category.clone(),
                });
            }
        }
    }
    Ok(out)
}

fn matches(s: &Sub, c: &Charge) -> bool {
    if let Some(a) = &s.account
        && *a != c.account
    {
        return false;
    }
    let want = normalise_payee(&s.payee);
    if want.is_empty() {
        return false;
    }
    if s.how == "contains" {
        c.norm.contains(&want)
    } else {
        c.norm == want
    }
}

/// Each subscription's charges (oldest first). A transaction belongs to one
/// subscription only: a hand-linked one to the subscription it was linked to, a
/// matching one to the oldest subscription that matches it.
fn assign<'a>(subs: &[Sub], all: &'a [Charge]) -> HashMap<String, Vec<&'a Charge>> {
    let mut owner: HashMap<&str, &str> = HashMap::new();
    for s in subs {
        for t in &s.added {
            owner.entry(t.as_str()).or_insert(s.id.as_str());
        }
    }
    let mut out: HashMap<String, Vec<&Charge>> = HashMap::new();
    for c in all {
        let to = match owner.get(c.txn.as_str()) {
            Some(id) => Some(*id),
            None => subs
                .iter()
                .find(|s| !s.removed.contains(&c.txn) && matches(s, c))
                .map(|s| s.id.as_str()),
        };
        if let Some(id) = to {
            out.entry(id.to_owned()).or_default().push(c);
        }
    }
    out
}

// ----- settings ------------------------------------------------------------------------

struct Cfg {
    alert_percent: Decimal,
    alert_amount: Decimal,
    bands: Vec<Band>,
    renewal_days: i64,
    trial_days: i64,
}

fn dec_setting(ctx: &Ctx<'_>, key: &str, default: Decimal) -> CommandResult<Decimal> {
    let v = ms::get(ctx, key)?;
    Ok(v.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| v.as_f64().and_then(|_| v.to_string().parse().ok()))
        .unwrap_or(default))
}

fn cfg(ctx: &Ctx<'_>) -> CommandResult<Cfg> {
    let bands = bands_of(&ms::get(ctx, "subscriptions.bands")?);
    Ok(Cfg {
        alert_percent: dec_setting(ctx, "subscriptions.alert_percent", Decimal::ONE)?,
        alert_amount: dec_setting(ctx, "subscriptions.alert_amount", Decimal::new(50, 2))?,
        bands,
        renewal_days: ms::get(ctx, "subscriptions.renewal_days")?
            .as_i64()
            .unwrap_or(14),
        trial_days: ms::get(ctx, "subscriptions.trial_days")?
            .as_i64()
            .unwrap_or(3),
    })
}

/// The bands of a stored setting value.
pub(crate) fn bands_of(v: &Value) -> Vec<Band> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| {
                    let p = match b.get("percent") {
                        Some(Value::String(s)) => s.parse().ok(),
                        Some(Value::Number(n)) => n.to_string().parse().ok(),
                        _ => None,
                    }?;
                    let c = sub::parse_hex(b.get("color").and_then(Value::as_str)?)?;
                    Some(Band {
                        percent: p,
                        color: c,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Validate a bands value: a list of at most 10 `{percent, color}`, percents from
/// 0.1 to 1000 and distinct, colours as #RRGGBB; stored sorted by percent.
pub(crate) fn check_bands(v: &Value) -> Result<Value, String> {
    let list = v
        .as_array()
        .ok_or_else(|| "a list of {percent, color} bands".to_owned())?;
    if list.len() > 10 {
        return Err("at most 10 bands".into());
    }
    let mut out: Vec<(Decimal, String)> = Vec::new();
    for b in list {
        let p: Decimal = match b.get("percent") {
            Some(Value::String(s)) => s.trim().trim_end_matches('%').trim().parse().ok(),
            Some(Value::Number(n)) => n.to_string().parse().ok(),
            _ => None,
        }
        .ok_or_else(|| "each band needs a percentage".to_owned())?;
        if p < Decimal::new(1, 1) || p > Decimal::from(1000) {
            return Err("a band's percentage is from 0.1 to 1000".into());
        }
        let c = b
            .get("color")
            .and_then(Value::as_str)
            .and_then(sub::parse_hex)
            .ok_or_else(|| "a band's colour is a hex colour such as #F2C94C".to_owned())?;
        let p = p.normalize();
        if out.iter().any(|(q, _)| *q == p) {
            return Err(format!("two bands have the same percentage ({p} %)"));
        }
        out.push((p, sub::hex(c)));
    }
    out.sort_by_key(|a| a.0);
    Ok(Value::Array(
        out.into_iter()
            .map(|(p, c)| json!({"percent": p.to_string(), "color": c}))
            .collect(),
    ))
}

// ----- computed views ------------------------------------------------------------------

fn amt(d: Decimal) -> String {
    sub::cents(d).to_string()
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "Nov 3".
fn short_date(d: Date) -> String {
    let m = MONTHS
        .get(usize::from(d.month()).saturating_sub(1))
        .copied()
        .unwrap_or("");
    format!("{m} {}", d.day())
}

/// One price history row.
#[derive(Debug, Clone)]
struct Row {
    key: String,
    date: Date,
    old: Decimal,
    new: Decimal,
    reason: String,
    manual: bool,
}

impl Row {
    fn percent(&self) -> Decimal {
        sub::change_percent(self.old, self.new).unwrap_or(Decimal::ZERO)
    }

    fn json(&self, c: &Cfg) -> Value {
        let pct = self.percent();
        let up = self.new > self.old;
        let band = if up {
            sub::band_for(pct, &c.bands)
        } else {
            None
        };
        json!({
            "key": self.key,
            "date": self.date,
            "old": amt(self.old),
            "new": amt(self.new),
            "change": amt(self.new - self.old),
            "percent": pct.round_dp(1).to_string(),
            "percent_text": sub::percent_text(pct),
            "increase": up,
            "small": sub::is_small(self.old, self.new, c.alert_percent, c.alert_amount),
            "color": band.map(|b| sub::hex(b.color)),
            "dark_text": band.map(|b| sub::dark_text_on(b.color)),
            "reason": self.reason,
            "manual": self.manual,
        })
    }
}

/// What a subscription is, computed from its charges.
struct View<'a> {
    s: &'a Sub,
    charges: Vec<&'a Charge>,
    price: Decimal,
    next: Option<Date>,
    rows: Vec<Row>,
}

fn view<'a>(s: &'a Sub, charges: Vec<&'a Charge>) -> View<'a> {
    let last = charges.last();
    let price = match (last, s.price, s.price_on) {
        (Some(l), Some(p), Some(on)) if on > l.date => p,
        (Some(l), _, _) => l.amount,
        (None, Some(p), _) => p,
        (None, None, _) => Decimal::ZERO,
    };
    let next = match (s.next_charge, last) {
        (Some(n), Some(l)) if n > l.date => Some(n),
        (_, Some(l)) => s.cadence.advance(l.date, 1),
        (n, None) => n,
    };
    let mut rows = Vec::new();
    if s.is_sub() {
        let mut manual: Vec<(&Manual, bool)> = s.manual.iter().map(|m| (m, false)).collect();
        for w in charges.windows(2) {
            let (Some(a), Some(b)) = (w.first(), w.get(1)) else {
                continue;
            };
            if a.amount == b.amount {
                continue;
            }
            let mut reason = s.reasons.get(&b.txn).cloned().unwrap_or_default();
            // A charge at a downgraded price takes the place of the review's row.
            if let Some(m) = manual
                .iter_mut()
                .find(|(m, used)| !*used && m.new == b.amount && b.date >= m.date)
            {
                m.1 = true;
                if reason.is_empty() {
                    reason = s.reasons.get(&m.0.key).cloned().unwrap_or_default();
                }
            }
            rows.push(Row {
                key: b.txn.clone(),
                date: b.date,
                old: a.amount,
                new: b.amount,
                reason,
                manual: false,
            });
        }
        for (m, used) in manual {
            if !used {
                rows.push(Row {
                    key: m.key.clone(),
                    date: m.date,
                    old: m.old,
                    new: m.new,
                    reason: s.reasons.get(&m.key).cloned().unwrap_or_default(),
                    manual: true,
                });
            }
        }
        rows.sort_by(|a, b| a.date.cmp(&b.date).then(a.manual.cmp(&b.manual)));
    }
    View {
        s,
        charges,
        price,
        next,
        rows,
    }
}

fn all_views<'a>(subs: &'a [Sub], all: &'a [Charge]) -> Vec<View<'a>> {
    let mut by = assign(subs, all);
    subs.iter()
        .map(|s| view(s, by.remove(&s.id).unwrap_or_default()))
        .collect()
}

fn status_label(s: &str) -> &'static str {
    match s {
        "paused" => "Paused",
        "cancelled" => "Cancelled",
        "trial" => "Free trial",
        _ => "Active",
    }
}

fn account_name(chart: &Chart, id: Option<&str>) -> Value {
    id.and_then(|i| i.parse().ok())
        .and_then(|i| chart.get(i).map(|_| json!(chart.full_name(i))))
        .unwrap_or(Value::Null)
}

/// The category: the record's, else the newest charge's.
fn category_of(v: &View<'_>) -> Option<String> {
    v.s.category
        .clone()
        .or_else(|| v.charges.last().and_then(|c| c.category.clone()))
}

fn row_json(v: &View<'_>, chart: &Chart, c: &Cfg) -> Value {
    let s = v.s;
    let cat = category_of(v);
    let amounts: Vec<Decimal> = v.charges.iter().map(|c| c.amount).collect();
    let average = if amounts.is_empty() {
        Decimal::ZERO
    } else {
        amounts.iter().sum::<Decimal>() / Decimal::from(amounts.len())
    };
    let last_change = v.rows.last().map(|r| r.json(c));
    json!({
        "id": s.id,
        "name": s.name,
        "kind": s.kind,
        "payee": s.payee,
        "how": s.how,
        "account": s.account,
        "account_name": account_name(chart, s.account.as_deref()),
        "cadence": s.cadence.as_str(),
        "cadence_label": s.cadence.label(),
        "price": amt(v.price),
        "monthly": amt(sub::monthly_equivalent(v.price, s.cadence)),
        "yearly": amt(sub::yearly(v.price, s.cadence)),
        "average": amt(average),
        "last_amount": v.charges.last().map(|c| amt(c.amount)),
        "last_date": v.charges.last().map(|c| c.date),
        "next_charge": v.next,
        "category": cat,
        "category_name": account_name(chart, cat.as_deref()),
        "status": s.status,
        "status_label": status_label(&s.status),
        "cancelled_on": s.cancelled_on,
        "trial_ends": s.trial_ends,
        "used_by": s.used_by,
        "link": s.link,
        "notes": s.notes,
        "reviewed_on": s.reviewed_on,
        "charges": v.charges.len(),
        "last_change": last_change,
    })
}

fn today_or(t: Option<Date>) -> CommandResult<Date> {
    match t {
        Some(t) => Ok(t),
        None => Ok(today_utc()?),
    }
}

fn year_before(d: Date) -> Date {
    sub::add_months(d, -12).unwrap_or(d)
}

// ----- list and get ----------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TodayArgs {
    /// Default: today.
    #[serde(default)]
    today: Option<Date>,
}

fn list(ctx: &Ctx<'_>, a: TodayArgs) -> CommandResult<Value> {
    let today = today_or(a.today)?;
    let c = cfg(ctx)?;
    let chart = Chart::load(ctx)?;
    let subs = load_subs(ctx)?;
    let all = charges(ctx)?;
    let views = all_views(&subs, &all);
    let mut rows = Vec::new();
    let mut bills = Vec::new();
    let (mut monthly, mut yearly_total) = (Decimal::ZERO, Decimal::ZERO);
    let mut by_cat: BTreeMap<String, Decimal> = BTreeMap::new();
    let ago = year_before(today);
    let mut ago_monthly = Decimal::ZERO;
    let mut increases: Vec<(Decimal, Value)> = Vec::new();
    let from = year_before(today);
    for v in &views {
        let mut r = row_json(v, &chart, &c);
        if !v.s.is_sub() {
            let this_year: Decimal = v
                .charges
                .iter()
                .filter(|x| x.date.year() == today.year())
                .map(|x| x.amount)
                .sum();
            let last_year: Decimal = v
                .charges
                .iter()
                .filter(|x| x.date.year() == today.year() - 1)
                .map(|x| x.amount)
                .sum();
            if let Some(o) = r.as_object_mut() {
                o.insert("year_total".into(), json!(amt(this_year)));
                o.insert("last_year_total".into(), json!(amt(last_year)));
                o.insert(
                    "amounts".into(),
                    json!(
                        v.charges
                            .iter()
                            .map(|x| json!({"date": x.date, "amount": amt(x.amount)}))
                            .collect::<Vec<_>>()
                    ),
                );
            }
            bills.push(r);
            continue;
        }
        if v.s.status == "active" {
            let m = sub::monthly_equivalent(v.price, v.s.cadence);
            monthly += m;
            yearly_total += sub::yearly(v.price, v.s.cadence);
            *by_cat
                .entry(category_of(v).unwrap_or_default())
                .or_default() += m;
        }
        // The same date a year ago: what each subscription charged then.
        if let Some(ch) = v.charges.iter().rev().find(|x| x.date <= ago)
            && v.s
                .cadence
                .advance(ch.date, 1)
                .and_then(|n| n.add_days(i32::try_from(v.s.cadence.tolerance()).unwrap_or(0)))
                .is_some_and(|n| n >= ago)
        {
            ago_monthly += sub::monthly_equivalent(ch.amount, v.s.cadence);
        }
        for row in &v.rows {
            if row.new > row.old && row.date > from && row.date <= today {
                let yearly_up = sub::yearly(row.new - row.old, v.s.cadence);
                let mut j = row.json(&c);
                if let Some(o) = j.as_object_mut() {
                    o.insert("subscription".into(), json!(v.s.id));
                    o.insert("name".into(), json!(v.s.name));
                    o.insert("yearly_increase".into(), json!(amt(yearly_up)));
                }
                increases.push((yearly_up, j));
            }
        }
        rows.push(r);
    }
    increases.sort_by_key(|a| std::cmp::Reverse(a.0));
    let categories: Vec<Value> = by_cat
        .into_iter()
        .map(|(k, m)| {
            json!({"category": if k.is_empty() { Value::Null } else { json!(k) },
                "name": account_name(&chart, Some(&k)).as_str().map_or("Uncategorised".to_owned(), str::to_owned),
                "monthly": amt(m)})
        })
        .collect();
    let suggested = suggestion_list(ctx, &subs, &all, &chart)?.len();
    Ok(json!({
        "subscriptions": rows,
        "bills": bills,
        "totals": {
            "monthly": amt(monthly),
            "yearly": amt(yearly_total),
            "year_ago_monthly": amt(ago_monthly),
            "change_monthly": amt(monthly - ago_monthly),
            "by_category": categories,
        },
        "biggest_increases": increases.into_iter().take(5).map(|x| x.1).collect::<Vec<_>>(),
        "suggested": suggested,
        "saved_year": amt(saved_year(&subs, today)),
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SubArgs {
    /// Id (or name when unique).
    subscription: String,
}

fn get(ctx: &Ctx<'_>, a: SubArgs) -> CommandResult<Value> {
    let s = find_sub(ctx, &a.subscription)?;
    let c = cfg(ctx)?;
    let chart = Chart::load(ctx)?;
    let subs = load_subs(ctx)?;
    let all = charges(ctx)?;
    let views = all_views(&subs, &all);
    let v = views
        .iter()
        .find(|v| v.s.id == s.id)
        .ok_or_else(|| CommandError::NotFound(format!("subscription {:?}", a.subscription)))?;
    let mut out = row_json(v, &chart, &c);
    let charges: Vec<Value> = v
        .charges
        .iter()
        .rev()
        .map(|x| {
            json!({"txn": x.txn, "date": x.date, "amount": amt(x.amount), "payee": x.payee,
                "account": x.account, "account_name": account_name(&chart, Some(&x.account)),
                "linked": if s.added.contains(&x.txn) { "hand" } else { "match" }})
        })
        .collect();
    let history: Vec<Value> = v.rows.iter().rev().map(|r| r.json(&c)).collect();
    // Possible missed charges: same account (or any), outgoing, not someone else's,
    // payee containing the first word of the match.
    let owned: BTreeSet<&str> = assign(&subs, &all)
        .values()
        .flatten()
        .map(|x| x.txn.as_str())
        .collect();
    let money_account = s
        .account
        .clone()
        .or_else(|| v.charges.last().map(|x| x.account.clone()));
    let (frequency, interval) = s.cadence.recurrence();
    let mut recurrence = json!({"frequency": frequency});
    if interval != 1
        && let Some(o) = recurrence.as_object_mut()
    {
        o.insert("interval".into(), json!(interval));
    }
    let category = category_of(v);
    let reminder = match (&money_account, &category, v.next) {
        (Some(m), Some(cat), Some(next)) => json!({
            "name": s.name,
            "template": {"payee": s.name, "splits": [
                {"account": m, "value": amt(-v.price)},
                {"account": cat, "value": amt(v.price)},
            ]},
            "recurrence": recurrence,
            "start": next,
            "auto_enter": false,
            "amount_kind": if s.is_sub() { "fixed" } else { "estimate" },
        }),
        _ => Value::Null,
    };
    let candidates: Vec<Value> = all
        .iter()
        .rev()
        .filter(|x| !owned.contains(x.txn.as_str()))
        .filter(|x| money_account.as_ref().is_none_or(|m| *m == x.account))
        .take(50)
        .map(|x| json!({"txn": x.txn, "date": x.date, "amount": amt(x.amount), "payee": x.payee}))
        .collect();
    if let Some(o) = out.as_object_mut() {
        o.insert("charge_list".into(), json!(charges));
        o.insert("history".into(), json!(history));
        o.insert("reminder".into(), reminder);
        o.insert("money_account".into(), json!(money_account));
        o.insert(
            "money_account_name".into(),
            account_name(&chart, money_account.as_deref()),
        );
        o.insert("unlinked".into(), json!(candidates));
    }
    Ok(out)
}

/// Make a bill (subscriptions.md revision 2). With `make_bill_as` `scheduled_bill`
/// a schedule that creates a bill is made (repeating on the cadence, never entered
/// automatically, the price as a fixed amount or the average as an estimate); with
/// `bill_now` nothing changes and `draft` holds the `bill.create` arguments the
/// front end opens in the bill editor.
fn make_bill(ctx: &Ctx<'_>, a: SubArgs) -> CommandResult<Value> {
    if !crate::is_module_enabled(ctx.tx, "bills")? {
        return Err(CommandError::Rejected(
            "Make a bill needs the bills feature on".into(),
        ));
    }
    let s = find_sub(ctx, &a.subscription)?;
    let chart = Chart::load(ctx)?;
    let subs = load_subs(ctx)?;
    let all = charges(ctx)?;
    let views = all_views(&subs, &all);
    let v = views
        .iter()
        .find(|v| v.s.id == s.id)
        .ok_or_else(|| CommandError::NotFound(format!("subscription {:?}", a.subscription)))?;
    let row = row_json(v, &chart, &cfg(ctx)?);
    let category = category_of(v).ok_or_else(|| {
        CommandError::Rejected(format!(
            "{} has no category yet; set one (or link a charge) first",
            s.name
        ))
    })?;
    let amount = if s.is_sub() {
        row.get("price").and_then(Value::as_str).unwrap_or("0")
    } else {
        row.get("average").and_then(Value::as_str).unwrap_or("0")
    }
    .to_owned();
    let today = mk_store::today_utc()?;
    let start = v.next.unwrap_or(today);
    let line = json!({"account": category, "description": s.name, "amount": amount});
    let mode = crate::module_settings::get_str(ctx, "subscriptions.make_bill_as")?;
    if mode == "bill_now" {
        return Ok(json!({
            "mode": mode,
            "draft": {"vendor": s.name, "date": start, "lines": [line]},
            "category_name": chart.resolve(&category).ok().map(|a| chart.full_name(a.id)),
        }));
    }
    let vendor = crate::sales::ensure_vendor(ctx, &s.name)?;
    let (frequency, interval) = s.cadence.recurrence();
    let mut recurrence = json!({"frequency": frequency});
    if interval != 1
        && let Some(o) = recurrence.as_object_mut()
    {
        o.insert("interval".into(), json!(interval));
    }
    let schedule = crate::schedules::create_json(
        ctx,
        json!({
            "name": s.name,
            "recurrence": recurrence,
            "start": start,
            "auto_enter": false,
            "amount_kind": if s.is_sub() { "fixed" } else { "estimate" },
            "creates": "bill",
            "bill": {"vendor": vendor, "lines": [line]},
        }),
    )?;
    Ok(json!({"mode": mode, "schedule": schedule}))
}

// ----- suggestions -----------------------------------------------------------------------

struct Suggestion<'a> {
    key: String,
    cadence: Cadence,
    kind: &'static str,
    run: Vec<&'a Charge>,
}

fn answer_key(account: &str, norm: &str) -> String {
    format!("{account}|{norm}")
}

fn suggestion_list<'a>(
    ctx: &Ctx<'_>,
    subs: &[Sub],
    all: &'a [Charge],
    _chart: &Chart,
) -> CommandResult<Vec<Suggestion<'a>>> {
    let answered: BTreeSet<String> = ctx
        .tx
        .docs(DocTable::SubscriptionAnswer, None)?
        .into_iter()
        .map(|d| d.id)
        .collect();
    let owned: BTreeSet<&str> = assign(subs, all)
        .values()
        .flatten()
        .map(|x| x.txn.as_str())
        .collect();
    let mut groups: BTreeMap<(String, String), Vec<&Charge>> = BTreeMap::new();
    for c in all {
        groups
            .entry((c.norm.clone(), c.account.clone()))
            .or_default()
            .push(c);
    }
    let mut out = Vec::new();
    for ((norm, account), list) in groups {
        let key = answer_key(&account, &norm);
        if answered.contains(&key) || list.iter().any(|c| owned.contains(c.txn.as_str())) {
            continue;
        }
        let dates: Vec<Date> = list.iter().map(|c| c.date).collect();
        let Some(d) = sub::detect(&dates) else {
            continue;
        };
        let run: Vec<&Charge> = list.get(d.start..).unwrap_or_default().to_vec();
        let amounts: Vec<Decimal> = run.iter().map(|c| c.amount).collect();
        let kind = if sub::is_subscription_like(&amounts) {
            "subscription"
        } else {
            "bill"
        };
        out.push(Suggestion {
            key,
            cadence: d.cadence,
            kind,
            run,
        });
    }
    out.sort_by(|a, b| {
        let la = a.run.last().map(|c| c.date);
        let lb = b.run.last().map(|c| c.date);
        lb.cmp(&la).then(a.key.cmp(&b.key))
    });
    Ok(out)
}

fn suggestion_json(s: &Suggestion<'_>, chart: &Chart) -> Value {
    let last = s.run.last();
    let first = s.run.first();
    json!({
        "key": s.key,
        "payee": last.map(|c| c.payee.clone()),
        "payee_normalised": last.map(|c| c.norm.clone()),
        "account": last.map(|c| c.account.clone()),
        "account_name": account_name(chart, last.map(|c| c.account.as_str())),
        "cadence": s.cadence.as_str(),
        "cadence_label": s.cadence.label(),
        "kind": s.kind,
        "last_amount": last.map(|c| amt(c.amount)),
        "last_date": last.map(|c| c.date),
        "charges": s.run.len(),
        "first_date": first.map(|c| c.date),
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

fn suggestions(ctx: &Ctx<'_>, _a: NoArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let subs = load_subs(ctx)?;
    let all = charges(ctx)?;
    let list = suggestion_list(ctx, &subs, &all, &chart)?;
    Ok(Value::Array(
        list.iter().map(|s| suggestion_json(s, &chart)).collect(),
    ))
}

fn dismissed(ctx: &Ctx<'_>, _a: NoArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let mut out = Vec::new();
    for d in ctx.tx.docs(DocTable::SubscriptionAnswer, None)? {
        let a: Answer = serde_json::from_value(d.json)
            .map_err(|e| CommandError::Integrity(format!("bad answer: {e}")))?;
        if a.answer == "not_recurring" {
            out.push(json!({"key": d.id, "payee": if a.display.is_empty() { &a.payee } else { &a.display },
                "payee_normalised": a.payee, "account": a.account,
                "account_name": account_name(&chart, Some(&a.account)), "on": a.on}));
        }
    }
    Ok(Value::Array(out))
}

/// A payee and account naming a suggestion.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AnswerArgs {
    /// The suggestion's key, or give payee and account.
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    payee: Option<String>,
    /// Account id or name.
    #[serde(default)]
    account: Option<String>,
    /// `subscription`, `bill` or `not_recurring`.
    answer: String,
    #[serde(default)]
    today: Option<Date>,
}

fn key_of(
    ctx: &Ctx<'_>,
    key: Option<String>,
    payee: Option<String>,
    account: Option<String>,
) -> CommandResult<String> {
    if let Some(k) = key {
        return Ok(k);
    }
    let (Some(p), Some(acc)) = (payee, account) else {
        return Err(invalid(
            "give the suggestion's key, or its payee and account",
        ));
    };
    let chart = Chart::load(ctx)?;
    let a = chart.resolve(&acc)?;
    Ok(answer_key(&a.id.to_string(), &normalise_payee(&p)))
}

fn answer(ctx: &Ctx<'_>, a: AnswerArgs) -> CommandResult<Value> {
    let today = today_or(a.today)?;
    let key = key_of(ctx, a.key, a.payee, a.account)?;
    if !matches!(a.answer.as_str(), "subscription" | "bill" | "not_recurring") {
        return Err(invalid("the answer is subscription, bill or not_recurring"));
    }
    let chart = Chart::load(ctx)?;
    let subs = load_subs(ctx)?;
    let all = charges(ctx)?;
    let list = suggestion_list(ctx, &subs, &all, &chart)?;
    let s = list
        .iter()
        .find(|s| s.key == key)
        .ok_or_else(|| CommandError::NotFound(format!("suggestion {key:?}")))?;
    let last = s
        .run
        .last()
        .ok_or_else(|| CommandError::Integrity("a suggestion without charges".into()))?;
    let mut ans = Answer {
        payee: last.norm.clone(),
        account: last.account.clone(),
        answer: a.answer.clone(),
        display: last.payee.clone(),
        subscription: None,
        on: Some(today),
    };
    let mut created = Value::Null;
    if a.answer != "not_recurring" {
        let id = new_id();
        let rec = Sub {
            id: id.clone(),
            name: last.payee.trim().to_owned(),
            kind: a.answer.clone(),
            payee: last.norm.clone(),
            account: Some(last.account.clone()),
            cadence: s.cadence,
            category: last.category.clone(),
            answer: Some(key.clone()),
            ..Sub::blank(today)
        };
        save_sub(ctx, &rec, date_ord(today))?;
        ans.subscription = Some(id.clone());
        created = json!(id);
    }
    ctx.save_doc(
        DocTable::SubscriptionAnswer,
        &key,
        Some(&Doc {
            id: key.clone(),
            grp: Some(ans.account.clone()),
            ord: date_ord(today),
            json: serde_json::to_value(&ans)?,
        }),
    )?;
    Ok(json!({"key": key, "answer": a.answer, "subscription": created}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RestoreArgs {
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    payee: Option<String>,
    #[serde(default)]
    account: Option<String>,
}

fn restore_suggestion(ctx: &Ctx<'_>, a: RestoreArgs) -> CommandResult<Value> {
    let key = key_of(ctx, a.key, a.payee, a.account)?;
    let d = ctx
        .tx
        .get_doc(DocTable::SubscriptionAnswer, &key)?
        .ok_or_else(|| CommandError::NotFound(format!("dismissed suggestion {key:?}")))?;
    if d.json.get("answer").and_then(Value::as_str) != Some("not_recurring") {
        return Err(rejected(
            "that payee was confirmed; delete the subscription instead",
        ));
    }
    ctx.save_doc(DocTable::SubscriptionAnswer, &key, None)?;
    Ok(json!({"restored": key}))
}

// ----- create, update, delete ------------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Fields {
    #[serde(default)]
    name: Option<String>,
    /// `subscription` (default) or `bill`.
    #[serde(default)]
    kind: Option<String>,
    /// The payee to match.
    #[serde(default)]
    payee: Option<String>,
    /// `equals` (default) or `contains`.
    #[serde(default)]
    how: Option<String>,
    /// Account id or name; "" for any account.
    #[serde(default)]
    account: Option<String>,
    /// weekly, biweekly, monthly, quarterly, six_monthly or yearly.
    #[serde(default)]
    cadence: Option<String>,
    #[serde(default)]
    price: Option<String>,
    /// "" clears an edited date.
    #[serde(default)]
    next_charge: Option<String>,
    /// An expense account (id or name); "" for the one from the transactions.
    #[serde(default)]
    category: Option<String>,
    /// active, paused, cancelled or trial.
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    cancelled_on: Option<String>,
    #[serde(default)]
    trial_ends: Option<String>,
    #[serde(default)]
    used_by: Option<String>,
    /// An http(s) address; "" clears it.
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    reviewed_on: Option<String>,
    #[serde(default)]
    today: Option<Date>,
}

fn text(v: &str, max: usize, what: &str) -> CommandResult<String> {
    let c = mk_core::text::clean(v, usize::MAX).text.trim().to_owned();
    if c.chars().count() > max {
        return Err(invalid(format!("{what}: at most {max} characters")));
    }
    Ok(c)
}

fn opt_date(v: &str, what: &str) -> CommandResult<Option<Date>> {
    if v.trim().is_empty() {
        return Ok(None);
    }
    v.trim()
        .parse()
        .map(Some)
        .map_err(|_| invalid(format!("{what}: a date (YYYY-MM-DD)")))
}

fn price_of(v: &str) -> CommandResult<Decimal> {
    let p: Decimal = v
        .trim()
        .replace(',', "")
        .parse()
        .map_err(|_| invalid("the price is an amount such as 15.49"))?;
    if p.is_sign_negative() || p > Decimal::from(1_000_000) {
        return Err(invalid("the price is from 0 to 1,000,000"));
    }
    Ok(sub::cents(p))
}

fn apply(ctx: &Ctx<'_>, s: &mut Sub, f: Fields, today: Date) -> CommandResult<()> {
    let chart = Chart::load(ctx)?;
    if let Some(n) = f.name {
        s.name = text(&n, 100, "the name")?;
    }
    if let Some(k) = f.kind {
        if !matches!(k.as_str(), "subscription" | "bill") {
            return Err(invalid("the kind is subscription or bill"));
        }
        s.kind = k;
    }
    if let Some(p) = f.payee {
        s.payee = normalise_payee(&text(&p, 100, "the payee")?);
    }
    if let Some(h) = f.how {
        if !matches!(h.as_str(), "equals" | "contains") {
            return Err(invalid("the payee match is equals or contains"));
        }
        s.how = h;
    }
    if let Some(acc) = f.account {
        s.account = if acc.trim().is_empty() {
            None
        } else {
            let a = chart.resolve(&acc)?;
            if !is_money(a.subtype) {
                return Err(invalid(format!(
                    "{} is not a bank, cash, credit card or other liability account",
                    chart.full_name(a.id)
                )));
            }
            Some(a.id.to_string())
        };
    }
    if let Some(c) = f.cadence {
        s.cadence = c.parse().map_err(|_| {
            invalid("the cadence is weekly, biweekly, monthly, quarterly, six_monthly or yearly")
        })?;
    }
    if let Some(p) = f.price {
        if p.trim().is_empty() {
            s.price = None;
            s.price_on = None;
        } else {
            s.price = Some(price_of(&p)?);
            s.price_on = Some(today);
        }
    }
    if let Some(n) = f.next_charge {
        s.next_charge = opt_date(&n, "the next charge")?;
    }
    if let Some(c) = f.category {
        s.category = if c.trim().is_empty() {
            None
        } else {
            let a = chart.resolve(&c)?;
            if a.account_type != mk_core::AccountType::Expense {
                return Err(invalid(format!(
                    "{} is not an expense account",
                    chart.full_name(a.id)
                )));
            }
            Some(a.id.to_string())
        };
    }
    if let Some(st) = f.status {
        if !matches!(st.as_str(), "active" | "paused" | "cancelled" | "trial") {
            return Err(invalid("the status is active, paused, cancelled or trial"));
        }
        s.status = st;
    }
    if let Some(d) = f.cancelled_on {
        s.cancelled_on = opt_date(&d, "the cancel date")?;
    }
    if let Some(d) = f.trial_ends {
        s.trial_ends = opt_date(&d, "the trial end")?;
    }
    if let Some(u) = f.used_by {
        s.used_by = text(&u, 100, "used by")?;
    }
    if let Some(l) = f.link {
        let l = text(&l, 500, "the link")?;
        if !l.is_empty() {
            let lower = l.to_ascii_lowercase();
            if !(lower.starts_with("https://") || lower.starts_with("http://"))
                || l.contains(char::is_whitespace)
            {
                return Err(invalid("the link is a web address starting with https://"));
            }
        }
        s.link = l;
    }
    if let Some(n) = f.notes {
        s.notes = text(&n, 2000, "the notes")?;
    }
    if let Some(r) = f.reviewed_on {
        s.reviewed_on = opt_date(&r, "the review date")?;
    }
    if s.status == "cancelled" && s.cancelled_on.is_none() {
        s.cancelled_on = Some(today);
    }
    if s.status == "trial" && s.trial_ends.is_none() {
        return Err(invalid("a free trial needs the date it ends (trial_ends)"));
    }
    if s.name.is_empty() {
        s.name = s.payee.clone();
    }
    if s.name.is_empty() || s.payee.is_empty() {
        return Err(invalid("a subscription needs a payee to match"));
    }
    Ok(())
}

fn create(ctx: &Ctx<'_>, f: Fields) -> CommandResult<Value> {
    let today = today_or(f.today)?;
    if f.payee.as_deref().is_none_or(|p| p.trim().is_empty()) {
        return Err(invalid("give the payee to match"));
    }
    if f.cadence.is_none() {
        return Err(invalid("give the cadence"));
    }
    let is_bill = f.kind.as_deref() == Some("bill");
    if !is_bill && f.price.as_deref().is_none_or(|p| p.trim().is_empty()) {
        return Err(invalid("give the price"));
    }
    let mut s = Sub {
        id: new_id(),
        ..Sub::blank(today)
    };
    apply(ctx, &mut s, f, today)?;
    save_sub(ctx, &s, date_ord(today))?;
    Ok(json!({"id": s.id, "name": s.name}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    subscription: String,
    #[serde(flatten)]
    fields: Fields,
}

fn update(ctx: &Ctx<'_>, a: UpdateArgs) -> CommandResult<Value> {
    let today = today_or(a.fields.today)?;
    let mut s = find_sub(ctx, &a.subscription)?;
    apply(ctx, &mut s, a.fields, today)?;
    resave(ctx, &s)?;
    Ok(json!({"id": s.id, "name": s.name}))
}

fn delete(ctx: &Ctx<'_>, a: SubArgs) -> CommandResult<Value> {
    let s = find_sub(ctx, &a.subscription)?;
    ctx.save_doc(DocTable::Subscription, &s.id, None)?;
    // Its suggestion may come back.
    if let Some(k) = &s.answer {
        ctx.save_doc(DocTable::SubscriptionAnswer, k, None)?;
    }
    Ok(json!({"deleted": s.id}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    subscription: String,
    /// active, paused, cancelled or trial.
    status: String,
    /// The cancel date (cancelled) or the trial's end (trial); default today.
    #[serde(default)]
    on: Option<Date>,
    #[serde(default)]
    today: Option<Date>,
}

fn set_status(ctx: &Ctx<'_>, a: StatusArgs) -> CommandResult<Value> {
    let today = today_or(a.today)?;
    let mut s = find_sub(ctx, &a.subscription)?;
    match a.status.as_str() {
        "cancelled" => {
            s.cancelled_on = Some(a.on.unwrap_or(today));
        }
        "trial" => {
            s.trial_ends = Some(
                a.on.or(s.trial_ends)
                    .ok_or_else(|| invalid("give the date the trial ends (on)"))?,
            );
            s.cancelled_on = None;
        }
        "active" | "paused" => {
            s.cancelled_on = None;
        }
        _ => {
            return Err(invalid("the status is active, paused, cancelled or trial"));
        }
    }
    s.status = a.status;
    resave(ctx, &s)?;
    Ok(json!({"id": s.id, "status": s.status, "cancelled_on": s.cancelled_on}))
}

fn still_active(ctx: &Ctx<'_>, a: SubArgs) -> CommandResult<Value> {
    let s = find_sub(ctx, &a.subscription)?;
    let subs = load_subs(ctx)?;
    let all = charges(ctx)?;
    let views = all_views(&subs, &all);
    let next = views
        .iter()
        .find(|v| v.s.id == s.id)
        .and_then(|v| v.next)
        .ok_or_else(|| rejected("this subscription has no expected charge"))?;
    let mut s = s;
    s.next_charge = s.cadence.advance(next, 1);
    s.status = "active".into();
    resave(ctx, &s)?;
    Ok(json!({"id": s.id, "next_charge": s.next_charge}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ChargeArgs {
    subscription: String,
    /// The transaction id.
    txn: String,
}

fn add_charge(ctx: &Ctx<'_>, a: ChargeArgs) -> CommandResult<Value> {
    let s = find_sub(ctx, &a.subscription)?;
    let all = charges(ctx)?;
    if !all.iter().any(|c| c.txn == a.txn) {
        return Err(rejected(
            "that transaction is not a payment from a bank, cash, credit card or other liability account",
        ));
    }
    // One subscription per transaction: take it from any other.
    for mut o in load_subs(ctx)? {
        if o.id != s.id && o.added.contains(&a.txn) {
            o.added.retain(|t| *t != a.txn);
            resave(ctx, &o)?;
        }
    }
    let mut s = find_sub(ctx, &s.id)?;
    s.removed.retain(|t| *t != a.txn);
    if !s.added.contains(&a.txn) {
        s.added.push(a.txn.clone());
    }
    resave(ctx, &s)?;
    Ok(json!({"id": s.id, "added": a.txn}))
}

fn remove_charge(ctx: &Ctx<'_>, a: ChargeArgs) -> CommandResult<Value> {
    let mut s = find_sub(ctx, &a.subscription)?;
    s.added.retain(|t| *t != a.txn);
    if !s.removed.contains(&a.txn) {
        s.removed.push(a.txn.clone());
    }
    resave(ctx, &s)?;
    Ok(json!({"id": s.id, "removed": a.txn}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReasonArgs {
    subscription: String,
    /// The history row's key.
    row: String,
    /// "" removes the note.
    reason: String,
}

fn set_reason(ctx: &Ctx<'_>, a: ReasonArgs) -> CommandResult<Value> {
    let mut s = find_sub(ctx, &a.subscription)?;
    let r = text(&a.reason, 500, "the reason")?;
    if r.is_empty() {
        s.reasons.remove(&a.row);
    } else {
        s.reasons.insert(a.row.clone(), r.clone());
    }
    resave(ctx, &s)?;
    Ok(json!({"id": s.id, "row": a.row, "reason": r}))
}

// ----- alerts ----------------------------------------------------------------------------

fn dismissed_alerts(ctx: &Ctx<'_>) -> CommandResult<BTreeSet<String>> {
    Ok(ctx
        .tx
        .get_setting(MODULE, DISMISSED_KEY)?
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect())
}

fn chip(text: String, color: u32) -> Value {
    json!({"text": text, "color": sub::hex(color), "dark_text": sub::dark_text_on(color)})
}

/// The open alerts, "charged after cancelling" first.
fn alerts(ctx: &Ctx<'_>, today: Date) -> CommandResult<Vec<Value>> {
    let c = cfg(ctx)?;
    let subs = load_subs(ctx)?;
    let all = charges(ctx)?;
    let views = all_views(&subs, &all);
    let gone = dismissed_alerts(ctx)?;
    let mut first = Vec::new();
    let mut rest = Vec::new();
    let target = |id: &str| json!({"screen": "subscriptions", "subscription": id});
    let push = |list: &mut Vec<Value>, key: String, mut v: Value| {
        if gone.contains(&key) {
            return;
        }
        if let Some(o) = v.as_object_mut() {
            o.insert("key".into(), json!(key));
            o.insert(
                "dismiss".into(),
                json!({"command": "subscription.dismiss_alert", "key": key}),
            );
        }
        list.push(v);
    };
    for v in &views {
        let s = v.s;
        // 2. Charged after cancelling.
        if s.status == "cancelled"
            && let Some(on) = s.cancelled_on
        {
            for ch in v.charges.iter().filter(|x| x.date > on) {
                push(
                    &mut first,
                    format!("after:{}:{}", s.id, ch.txn),
                    json!({"kind": "charged_after_cancel", "subscription": s.id,
                        "label": format!("{} charged {} on {} after you cancelled it on {}.",
                            s.name, amt(ch.amount), short_date(ch.date), short_date(on)),
                        "target": target(&s.id),
                        "chip": chip("Charged after cancelling".into(), sub::ALERT_RED)}),
                );
            }
        }
        if !s.is_sub() {
            continue;
        }
        // 1. Price went up (the latest change, until it has a reason).
        if let Some(r) = v.rows.last()
            && r.new > r.old
            && !sub::is_small(r.old, r.new, c.alert_percent, c.alert_amount)
            && r.reason.is_empty()
        {
            let pct = r.percent();
            let mut item = json!({"kind": "price_rise", "subscription": s.id, "row": r.key,
                "label": format!("{}: {} → {} ({}). Add a reason?", s.name, amt(r.old), amt(r.new),
                    sub::percent_text(pct)),
                "target": {"screen": "subscriptions", "subscription": s.id, "reason": r.key}});
            if let Some(b) = sub::band_for(pct, &c.bands)
                && let Some(o) = item.as_object_mut()
            {
                o.insert(
                    "chip".into(),
                    chip(
                        format!("↑ {}", sub::percent_text(pct).trim_start_matches('+')),
                        b.color,
                    ),
                );
            }
            push(&mut rest, format!("rise:{}:{}", s.id, r.key), item);
        }
        // 3. Expected charge missing.
        if s.status == "active"
            && let Some(next) = v.next
        {
            let tol = s.cadence.tolerance();
            let late = next.days_until(today) > tol;
            let seen = v.charges.iter().any(|x| {
                x.date.days_until(next) <= tol
                    && x.date
                        >= next
                            .add_days(-i32::try_from(tol).unwrap_or(0))
                            .unwrap_or(next)
            });
            if late && !seen {
                push(
                    &mut rest,
                    format!("missing:{}:{next}", s.id),
                    json!({"kind": "missing", "subscription": s.id,
                    "label": format!("{}'s charge due {} hasn't appeared. Cancelled, or did the card change?",
                        s.name, short_date(next)),
                    "target": target(&s.id),
                    "actions": [
                        {"label": "Mark cancelled", "command": "subscription.set_status",
                         "args": {"subscription": s.id, "status": "cancelled", "on": next}},
                        {"label": "Still active", "command": "subscription.still_active",
                         "args": {"subscription": s.id}},
                    ]}),
                );
            }
        }
        // 4. Renewal coming up.
        if s.status == "active"
            && matches!(s.cadence, Cadence::Yearly | Cadence::SixMonthly)
            && let Some(next) = v.next
        {
            let d = today.days_until(next);
            if (0..=c.renewal_days).contains(&d) {
                push(
                    &mut rest,
                    format!("renew:{}:{next}", s.id),
                    json!({"kind": "renewal", "subscription": s.id,
                        "label": format!("{} renews on {} for {}.", s.name, short_date(next), amt(v.price)),
                        "target": target(&s.id)}),
                );
            }
        }
        // 5. Trial ending.
        if s.status == "trial"
            && let Some(end) = s.trial_ends
        {
            let d = today.days_until(end);
            if (0..=c.trial_days).contains(&d) {
                push(
                    &mut rest,
                    format!("trial:{}:{end}", s.id),
                    json!({"kind": "trial", "subscription": s.id,
                        "label": format!("The free trial of {} ends on {}; it then costs {}.",
                            s.name, short_date(end), amt(v.price)),
                        "target": target(&s.id)}),
                );
            }
        }
    }
    // 6. Possible duplicates.
    let active: Vec<&View<'_>> = views
        .iter()
        .filter(|v| v.s.is_sub() && v.s.status == "active")
        .collect();
    let mut by_payee: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    for v in &active {
        let acc =
            v.s.account
                .clone()
                .or_else(|| v.charges.last().map(|c| c.account.clone()));
        if let Some(acc) = acc {
            let p = normalise_payee(&v.s.payee);
            by_payee.entry(p.clone()).or_default().insert(acc);
            names.entry(p).or_insert_with(|| v.s.name.clone());
        }
    }
    let chart = Chart::load(ctx)?;
    for (p, accs) in by_payee {
        if accs.len() >= 2 {
            let list: Vec<String> = accs
                .iter()
                .filter_map(|a| account_name(&chart, Some(a)).as_str().map(str::to_owned))
                .collect();
            push(
                &mut rest,
                format!("dup:payee:{p}"),
                json!({"kind": "duplicate", "label": format!("Possible duplicate: {} is charged on {}.",
                    names.get(&p).cloned().unwrap_or(p.clone()), list.join(" and ")),
                    "target": {"screen": "subscriptions"}}),
            );
        }
    }
    let mut by_cat: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for v in &active {
        if let Some(cat) = category_of(v) {
            by_cat.entry(cat).or_default().push(v.s.name.clone());
        }
    }
    for (cat, list) in by_cat {
        if list.len() >= 3 {
            let name = account_name(&chart, Some(&cat))
                .as_str()
                .map_or_else(|| "one category".to_owned(), str::to_owned);
            push(
                &mut rest,
                format!("dup:cat:{cat}"),
                json!({"kind": "duplicate", "label": format!("{} subscriptions in {name}: {}. Any duplicates?",
                    list.len(), list.join(", ")), "target": {"screen": "subscriptions"}}),
            );
        }
    }
    first.extend(rest);
    Ok(first)
}

/// The alerts for Home → Attention (empty when the module is off).
pub(crate) fn attention(ctx: &Ctx<'_>, today: Date) -> CommandResult<Vec<Value>> {
    if !crate::is_module_enabled(ctx.tx, MODULE)? {
        return Ok(Vec::new());
    }
    alerts(ctx, today)
}

fn alerts_cmd(ctx: &Ctx<'_>, a: TodayArgs) -> CommandResult<Value> {
    let list = alerts(ctx, today_or(a.today)?)?;
    Ok(json!({"count": list.len(), "alerts": list}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DismissArgs {
    /// The alert's key.
    key: String,
}

fn dismiss_alert(ctx: &Ctx<'_>, a: DismissArgs) -> CommandResult<Value> {
    let mut gone = dismissed_alerts(ctx)?;
    if !gone.insert(a.key.clone()) {
        return Ok(json!({"dismissed": a.key}));
    }
    let v = json!(gone.into_iter().collect::<Vec<_>>());
    ctx.save_setting(MODULE, DISMISSED_KEY, Some(&v))?;
    Ok(json!({"dismissed": a.key}))
}

// ----- review ----------------------------------------------------------------------------

fn saved_year(subs: &[Sub], today: Date) -> Decimal {
    let from = year_before(today);
    subs.iter()
        .flat_map(|s| s.saved.iter())
        .filter(|x| x.date > from && x.date <= today)
        .map(|x| x.amount)
        .sum()
}

fn review(ctx: &Ctx<'_>, a: TodayArgs) -> CommandResult<Value> {
    let today = today_or(a.today)?;
    let c = cfg(ctx)?;
    let chart = Chart::load(ctx)?;
    let subs = load_subs(ctx)?;
    let all = charges(ctx)?;
    let views = all_views(&subs, &all);
    let mut queue: Vec<&View<'_>> = views
        .iter()
        .filter(|v| v.s.is_sub() && v.s.status == "active")
        .collect();
    queue.sort_by(|a, b| {
        a.s.reviewed_on
            .cmp(&b.s.reviewed_on)
            .then(a.s.name.to_lowercase().cmp(&b.s.name.to_lowercase()))
    });
    Ok(json!({
        "queue": queue.iter().map(|v| row_json(v, &chart, &c)).collect::<Vec<_>>(),
        "saved_year": amt(saved_year(&subs, today)),
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReviewArgs {
    subscription: String,
    /// keep, cancel or downgrade.
    answer: String,
    /// Downgrade: the new price.
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    today: Option<Date>,
}

fn review_answer(ctx: &Ctx<'_>, a: ReviewArgs) -> CommandResult<Value> {
    let today = today_or(a.today)?;
    let mut s = find_sub(ctx, &a.subscription)?;
    let subs = load_subs(ctx)?;
    let all = charges(ctx)?;
    let price = {
        let views = all_views(&subs, &all);
        views
            .iter()
            .find(|v| v.s.id == s.id)
            .map_or(Decimal::ZERO, |v| v.price)
    };
    match a.answer.as_str() {
        "keep" => {}
        "cancel" => {
            s.status = "cancelled".into();
            s.cancelled_on = Some(today);
            s.saved.push(Saved {
                date: today,
                amount: sub::yearly(price, s.cadence),
                how: "cancel".into(),
            });
        }
        "downgrade" => {
            let new = price_of(
                a.price
                    .as_deref()
                    .ok_or_else(|| invalid("give the new price"))?,
            )?;
            if new >= price {
                return Err(invalid(format!(
                    "a downgrade's price is less than {}",
                    amt(price)
                )));
            }
            let key = format!("m:{today}:{}", s.manual.len() + 1);
            s.manual.push(Manual {
                key: key.clone(),
                date: today,
                old: price,
                new,
            });
            s.reasons.insert(key, "Downgraded".into());
            s.price = Some(new);
            s.price_on = Some(today);
            s.saved.push(Saved {
                date: today,
                amount: sub::yearly(price - new, s.cadence),
                how: "downgrade".into(),
            });
        }
        _ => return Err(invalid("the answer is keep, cancel or downgrade")),
    }
    s.reviewed_on = Some(today);
    resave(ctx, &s)?;
    let subs = load_subs(ctx)?;
    Ok(json!({"id": s.id, "answer": a.answer, "saved_year": amt(saved_year(&subs, today))}))
}

// ----- dashboard block -------------------------------------------------------------------

fn summary(ctx: &Ctx<'_>, a: TodayArgs) -> CommandResult<Value> {
    let today = today_or(a.today)?;
    let subs = load_subs(ctx)?;
    let all = charges(ctx)?;
    let views = all_views(&subs, &all);
    let mut monthly = Decimal::ZERO;
    let mut next: Vec<(Date, Value)> = Vec::new();
    for v in &views {
        if !v.s.is_sub() || v.s.status != "active" {
            continue;
        }
        monthly += sub::monthly_equivalent(v.price, v.s.cadence);
        if let Some(n) = v.next.filter(|n| *n >= today) {
            next.push((
                n,
                json!({"subscription": v.s.id, "name": v.s.name, "date": n, "amount": amt(v.price)}),
            ));
        }
    }
    next.sort_by_key(|a| a.0);
    let count = alerts(ctx, today)?.len();
    Ok(json!({
        "monthly": amt(monthly),
        "next": next.into_iter().take(3).map(|x| x.1).collect::<Vec<_>>(),
        "alerts": count,
    }))
}
