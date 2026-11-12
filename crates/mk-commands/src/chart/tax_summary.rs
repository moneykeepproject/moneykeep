//! `report.tax_summary` (chart-presets.md): what goes where on the return, by the
//! accounts' tax lines, with a balance section for `Sch L` lines and a Not tagged
//! section.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

use mk_core::currency::{format_amount, minor_units};
use mk_core::{AccountId, AccountType, Decimal};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::pdf::{Font, Page, Writer, wrap};
use crate::reports::{Csv, Dates, resolve};
use crate::{CommandResult, Ctx, Registry};

pub(crate) const HINT: &str = "Tax lines are hints to help you or your preparer, not tax advice. Line numbers can change from year to year.";
pub(crate) const NO_TAX_LINES: &str = "This book's accounts have no tax lines. Add them in Accounts, or add accounts from a tax chart.";

/// Group order by tag prefix (the longest prefix a tag starts with wins).
const PREFIXES: &[&str] = &[
    "1040", "Sch 1-A", "Sch 1", "Sch A", "Sch C", "Sch D", "1120-S", "1065", "1120", "1125-A",
    "Sch K", "Sch L", "Sch M-1", "Sch M-2", "Form",
];

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "report.tax_summary",
        "Totals for the period by account tax line, with Schedule L balances and untagged accounts",
        false,
        tax_summary,
    );
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Args {
    /// Dates (default: preset last_year).
    #[serde(flatten)]
    dates: Dates,
    /// List income and expense accounts with activity but no tax line (default true).
    #[serde(default = "yes")]
    include_untagged: bool,
}

fn yes() -> bool {
    true
}

/// The rank of a tag's prefix; tags matching none come last.
fn prefix_rank(tag: &str) -> usize {
    let t = tag.to_lowercase();
    PREFIXES
        .iter()
        .enumerate()
        .filter(|(_, p)| t.starts_with(&p.to_lowercase()))
        .max_by_key(|(_, p)| p.len())
        .map_or(PREFIXES.len(), |(i, _)| i)
}

/// Natural, case-insensitive comparison: digit runs compare as numbers.
pub(crate) fn natural_cmp(a: &str, b: &str) -> Ordering {
    fn parts(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.to_lowercase().chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, run)) if *d == digit => run.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (pa, pb) = (parts(a), parts(b));
    for (x, y) in pa.iter().zip(pb.iter()) {
        let o = match (x, y) {
            ((true, n), (true, m)) => {
                let n = n.trim_start_matches('0');
                let m = m.trim_start_matches('0');
                n.len().cmp(&m.len()).then_with(|| n.cmp(m))
            }
            ((_, s), (_, t)) => s.cmp(t),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    pa.len().cmp(&pb.len()).then_with(|| a.cmp(b))
}

/// Report tag order (chart-presets.md "Order").
pub(crate) fn tag_cmp(a: &str, b: &str) -> Ordering {
    prefix_rank(a)
        .cmp(&prefix_rank(b))
        .then_with(|| natural_cmp(a, b))
}

fn is_balance_tag(tag: &str) -> bool {
    tag.to_lowercase().starts_with("sch l")
}

fn plus(a: Decimal, b: Decimal) -> Decimal {
    a.checked_add(b).unwrap_or(a)
}

fn tax_summary(ctx: &Ctx<'_>, a: Args) -> CommandResult<Value> {
    build(ctx, a)
}

/// The report from JSON arguments (export).
pub(crate) fn report_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    build(ctx, crate::parse_args(args)?)
}

fn build(ctx: &Ctx<'_>, a: Args) -> CommandResult<Value> {
    let (from, to) = resolve(ctx, &a.dates.or_last_year())?;
    let chart = Chart::load(ctx)?;
    let base = ctx.tx.info()?.base_currency;
    let dp = minor_units(&base);
    let money = |v: Decimal| format_amount(v, Some(dp));
    let tax_year = ctx
        .tx
        .get_setting(crate::modules::CORE, super::TAX_YEAR_KEY)?
        .and_then(|v| v.as_i64());

    // Raw (debit-positive) sums per account: before `from`, and within the period.
    let mut before: HashMap<AccountId, Decimal> = HashMap::new();
    let mut within: HashMap<AccountId, Decimal> = HashMap::new();
    let mut active: HashMap<AccountId, bool> = HashMap::new();
    let mut excluded: Vec<AccountId> = Vec::new();
    for row in ctx.tx.posted_splits(Some(to))? {
        let Some(acc) = chart.get(row.account) else {
            continue;
        };
        if acc.commodity.as_str() != base {
            active.insert(acc.id, true);
            continue;
        }
        if row.date < from {
            let e = before.entry(acc.id).or_default();
            *e = plus(*e, row.quantity);
        } else {
            let e = within.entry(acc.id).or_default();
            *e = plus(*e, row.quantity);
            active.insert(acc.id, true);
        }
    }
    let display = |t: AccountType, v: Decimal| t.display(v);
    let in_base = |id: AccountId| chart.get(id).is_some_and(|x| x.commodity.as_str() == base);

    // Group rows by tag.
    let mut activity: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut activity_tot: HashMap<String, Decimal> = HashMap::new();
    let mut balance: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut balance_tot: HashMap<String, (Decimal, Decimal)> = HashMap::new();
    let mut untagged = Vec::new();
    let mut untagged_total = Decimal::ZERO;
    let mut any_tag = false;
    for (acc, _) in chart.tree_order() {
        if acc.tax_line.is_some() {
            any_tag = true;
        }
        if !in_base(acc.id) {
            let in_period = within.contains_key(&acc.id) || active.contains_key(&acc.id);
            if acc.tax_line.is_some() || in_period {
                excluded.push(acc.id);
            }
            continue;
        }
        let b = before.get(&acc.id).copied().unwrap_or_default();
        let w = within.get(&acc.id).copied().unwrap_or_default();
        let base_row = json!({
            "account": acc.id,
            "full_name": chart.full_name(acc.id),
            "code": acc.code,
        });
        match &acc.tax_line {
            Some(tag) if is_balance_tag(tag) => {
                let begin = display(acc.account_type, b);
                let end = display(acc.account_type, plus(b, w));
                if begin.is_zero() && end.is_zero() && !active.contains_key(&acc.id) {
                    continue;
                }
                let mut r = base_row;
                if let Some(o) = r.as_object_mut() {
                    o.insert("beginning".into(), json!(money(begin)));
                    o.insert("end".into(), json!(money(end)));
                }
                balance.entry(tag.clone()).or_default().push(r);
                let t = balance_tot.entry(tag.clone()).or_default();
                *t = (plus(t.0, begin), plus(t.1, end));
            }
            Some(tag) => {
                if !active.contains_key(&acc.id) {
                    continue;
                }
                let amount = display(acc.account_type, w);
                let mut r = base_row;
                if let Some(o) = r.as_object_mut() {
                    o.insert("amount".into(), json!(money(amount)));
                }
                activity.entry(tag.clone()).or_default().push(r);
                let t = activity_tot.entry(tag.clone()).or_default();
                *t = plus(*t, amount);
            }
            None => {
                let postable = !acc.placeholder
                    && matches!(acc.account_type, AccountType::Income | AccountType::Expense);
                if !a.include_untagged || !postable || !active.contains_key(&acc.id) {
                    continue;
                }
                let amount = display(acc.account_type, w);
                let mut r = base_row;
                if let Some(o) = r.as_object_mut() {
                    o.insert("amount".into(), json!(money(amount)));
                }
                untagged.push(r);
                untagged_total = plus(untagged_total, amount);
            }
        }
    }
    let mut tags: Vec<&String> = activity.keys().collect();
    tags.sort_by(|x, y| tag_cmp(x, y));
    let activity_groups: Vec<Value> = tags
        .into_iter()
        .map(|t| {
            json!({
                "tax_line": t,
                "accounts": activity.get(t).cloned().unwrap_or_default(),
                "total": money(activity_tot.get(t).copied().unwrap_or_default()),
            })
        })
        .collect();
    let mut tags: Vec<&String> = balance.keys().collect();
    tags.sort_by(|x, y| tag_cmp(x, y));
    let balance_groups: Vec<Value> = tags
        .into_iter()
        .map(|t| {
            let (b, e) = balance_tot.get(t).copied().unwrap_or_default();
            json!({
                "tax_line": t,
                "accounts": balance.get(t).cloned().unwrap_or_default(),
                "beginning": money(b),
                "end": money(e),
            })
        })
        .collect();
    let excluded: Vec<Value> = excluded
        .into_iter()
        .map(|id| json!({"account": id, "full_name": chart.full_name(id), "reason": "not in base currency"}))
        .collect();
    let day_before = from.add_days(-1).unwrap_or(from);
    Ok(json!({
        "report": "tax_summary",
        "from": from,
        "to": to,
        "beginning_date": day_before,
        "currency": base,
        "tax_year": tax_year,
        "note": HINT,
        "no_tax_lines": if any_tag { Value::Null } else { json!(NO_TAX_LINES) },
        "activity": activity_groups,
        "balance": balance_groups,
        "untagged": if a.include_untagged {
            json!({"accounts": untagged, "total": money(untagged_total)})
        } else {
            Value::Null
        },
        "excluded": excluded,
        // mileage.md: standard-rate mileage for the period, for the preparer.
        "mileage": crate::mileage::tax_section(ctx, from, to)?,
        // investments-options-bonds-crypto.md revision 2: crypto rewards and interest
        // by kind, named as in the investment income report.
        "investment_income": crate::inv_reports::tax_section(ctx, from, to)?,
    }))
}

fn at<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key).unwrap_or(&Value::Null)
}

fn st(v: &Value) -> String {
    v.as_str().map(str::to_owned).unwrap_or_default()
}

fn list(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

/// CSV: section, tax line, account, code, amount (activity) or beginning/end
/// (balance).
pub(crate) fn csv(data: &Value) -> String {
    let mut c = Csv::new(&[
        "Section".into(),
        "Tax line".into(),
        "Account".into(),
        "Code".into(),
        "Amount".into(),
        "Beginning".into(),
        "End".into(),
        "Included in".into(),
    ]);
    for g in list(at(data, "activity")) {
        let tag = st(at(&g, "tax_line"));
        for r in list(at(&g, "accounts")) {
            c.row(&[
                "Activity".into(),
                tag.clone(),
                st(at(&r, "full_name")),
                st(at(&r, "code")),
                st(at(&r, "amount")),
            ]);
        }
        c.row(&[
            "Activity".into(),
            tag.clone(),
            "Total".into(),
            String::new(),
            st(at(&g, "total")),
        ]);
    }
    for g in list(at(data, "balance")) {
        let tag = st(at(&g, "tax_line"));
        for r in list(at(&g, "accounts")) {
            c.row(&[
                "Balance".into(),
                tag.clone(),
                st(at(&r, "full_name")),
                st(at(&r, "code")),
                String::new(),
                st(at(&r, "beginning")),
                st(at(&r, "end")),
            ]);
        }
        c.row(&[
            "Balance".into(),
            tag.clone(),
            "Total".into(),
            String::new(),
            String::new(),
            st(at(&g, "beginning")),
            st(at(&g, "end")),
        ]);
    }
    let u = at(data, "untagged");
    if !u.is_null() {
        for r in list(at(u, "accounts")) {
            c.row(&[
                "Not tagged".into(),
                String::new(),
                st(at(&r, "full_name")),
                st(at(&r, "code")),
                st(at(&r, "amount")),
            ]);
        }
        c.row(&[
            "Not tagged".into(),
            String::new(),
            "Total".into(),
            String::new(),
            st(at(u, "total")),
        ]);
    }
    let inc = at(data, "investment_income");
    if !inc.is_null() {
        let title = st(at(inc, "title"));
        for r in list(at(inc, "rows")) {
            c.row(&[
                title.clone(),
                String::new(),
                st(at(&r, "label")),
                String::new(),
                st(at(&r, "amount")),
                String::new(),
                String::new(),
                st(at(&r, "included_in")),
            ]);
        }
        c.row(&[
            title.clone(),
            String::new(),
            st(at(inc, "total_label")),
            String::new(),
            st(at(inc, "total")),
        ]);
    }
    let m = at(data, "mileage");
    if !m.is_null() {
        let title = st(at(m, "title"));
        for v in list(at(m, "vehicles")) {
            for r in list(at(&v, "rows")) {
                c.row(&[
                    title.clone(),
                    String::new(),
                    format!(
                        "{} — {} ({} mi)",
                        st(at(&v, "name")),
                        st(at(&r, "label")),
                        st(at(&r, "miles"))
                    ),
                    String::new(),
                    st(at(&r, "value")),
                ]);
            }
        }
    }
    c.out
}

// ----- PDF -------------------------------------------------------------------------

const W: i64 = 612;
const H: i64 = 792;
const MARGIN: i64 = 50;
const RIGHT: i64 = W - MARGIN;
const SIZE: i64 = 9;
const LEAD: i64 = 13;
const CODE_X: i64 = 70;
const NAME_X: i64 = 110;
const COL1: i64 = 470;
const COL2: i64 = RIGHT;

struct Doc {
    pages: Vec<(Page, Vec<usize>)>,
    cur: Page,
    y: i64,
}

impl Doc {
    fn need(&mut self, h: i64) {
        if self.y - h < MARGIN + 20 {
            let p = std::mem::replace(&mut self.cur, Page { ops: Vec::new() });
            self.pages.push((p, Vec::new()));
            self.y = H - MARGIN;
        }
    }
}

/// The standard PDF fonts have no arrow.
fn printable(s: &str) -> String {
    s.replace('→', "->")
}

/// `report.export` with `format: "pdf"` for the Tax summary.
pub(crate) fn export_pdf(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    let data = report_json(ctx, args)?;
    let book = ctx.tx.info()?.name;
    let mut wr = Writer {
        replaced: Vec::new(),
    };
    let mut d = Doc {
        pages: Vec::new(),
        cur: Page { ops: Vec::new() },
        y: H - MARGIN,
    };
    let from = st(at(&data, "from"));
    let to = st(at(&data, "to"));
    let y = d.y;
    d.cur
        .text(MARGIN, y - 12, Font::Bold, 12, &wr.encode(&book));
    d.cur.right(RIGHT, y - 18, Font::Bold, 18, b"Tax summary");
    d.y -= 36;
    let mut facts = format!("{from} to {to}");
    if let Some(year) = at(&data, "tax_year").as_i64() {
        facts.push_str(&format!(" - tax lines for {year}"));
    }
    let y = d.y;
    d.cur.text(MARGIN, y, Font::Regular, 10, &wr.encode(&facts));
    d.y -= LEAD + 2;
    for line in wrap(&wr.encode(HINT), Font::Regular, 8, RIGHT - MARGIN) {
        let y = d.y;
        d.cur.text(MARGIN, y, Font::Regular, 8, &line);
        d.y -= 11;
    }
    let none = at(&data, "no_tax_lines");
    if let Some(n) = none.as_str() {
        for line in wrap(&wr.encode(n), Font::Regular, 8, RIGHT - MARGIN) {
            let y = d.y;
            d.cur.text(MARGIN, y, Font::Regular, 8, &line);
            d.y -= 11;
        }
    }
    d.y -= 10;
    let money = |s: String| crate::sales::group(&s);
    let section = |d: &mut Doc, wr: &mut Writer, title: &str, cols: &[&str]| {
        d.need(LEAD * 3);
        let y = d.y;
        d.cur.text(MARGIN, y, Font::Bold, 11, &wr.encode(title));
        if let Some(c) = cols.first() {
            let x = if cols.len() > 1 { COL1 } else { COL2 };
            d.cur.right(x, y, Font::Bold, SIZE, &wr.encode(c));
        }
        if let Some(c) = cols.get(1) {
            d.cur.right(COL2, y, Font::Bold, SIZE, &wr.encode(c));
        }
        d.cur.rule(MARGIN, RIGHT, y - 4);
        d.y -= LEAD + 4;
    };
    let row =
        |d: &mut Doc, wr: &mut Writer, code: &str, name: &str, vals: &[String], bold: bool| {
            d.need(LEAD);
            let y = d.y;
            let f = if bold { Font::Bold } else { Font::Regular };
            if !code.is_empty() {
                d.cur.text(CODE_X, y, Font::Regular, SIZE, &wr.encode(code));
            }
            let x = if bold { MARGIN + 10 } else { NAME_X };
            let max = if vals.len() > 1 {
                COL1 - 90 - x
            } else {
                COL2 - 90 - x
            };
            let first = wrap(&wr.encode(name), f, SIZE, max)
                .into_iter()
                .next()
                .unwrap_or_default();
            d.cur.text(x, y, f, SIZE, &first);
            match vals {
                [one] => d.cur.right(COL2, y, f, SIZE, &wr.encode(one)),
                [a, b] => {
                    d.cur.right(COL1, y, f, SIZE, &wr.encode(a));
                    d.cur.right(COL2, y, f, SIZE, &wr.encode(b));
                }
                _ => {}
            }
            d.y -= LEAD;
        };
    let heading = |d: &mut Doc, wr: &mut Writer, tag: &str| {
        d.need(LEAD * 2);
        let y = d.y;
        d.cur
            .text(MARGIN, y, Font::Bold, SIZE + 1, &wr.encode(&printable(tag)));
        d.y -= LEAD;
    };
    let activity = list(at(&data, "activity"));
    if !activity.is_empty() {
        section(&mut d, &mut wr, "Activity", &["Amount"]);
        for g in activity {
            heading(&mut d, &mut wr, &st(at(&g, "tax_line")));
            for r in list(at(&g, "accounts")) {
                row(
                    &mut d,
                    &mut wr,
                    &st(at(&r, "code")),
                    &st(at(&r, "full_name")),
                    &[money(st(at(&r, "amount")))],
                    false,
                );
            }
            row(
                &mut d,
                &mut wr,
                "",
                "Total",
                &[money(st(at(&g, "total")))],
                true,
            );
            d.y -= 4;
        }
        d.y -= 8;
    }
    let balance = list(at(&data, "balance"));
    if !balance.is_empty() {
        section(
            &mut d,
            &mut wr,
            "Balances",
            &["Beginning of year", "End of year"],
        );
        for g in balance {
            heading(&mut d, &mut wr, &st(at(&g, "tax_line")));
            for r in list(at(&g, "accounts")) {
                row(
                    &mut d,
                    &mut wr,
                    &st(at(&r, "code")),
                    &st(at(&r, "full_name")),
                    &[money(st(at(&r, "beginning"))), money(st(at(&r, "end")))],
                    false,
                );
            }
            row(
                &mut d,
                &mut wr,
                "",
                "Total",
                &[money(st(at(&g, "beginning"))), money(st(at(&g, "end")))],
                true,
            );
            d.y -= 4;
        }
        d.y -= 8;
    }
    let u = at(&data, "untagged");
    if !u.is_null() {
        section(&mut d, &mut wr, "Not tagged", &["Amount"]);
        for r in list(at(u, "accounts")) {
            row(
                &mut d,
                &mut wr,
                &st(at(&r, "code")),
                &st(at(&r, "full_name")),
                &[money(st(at(&r, "amount")))],
                false,
            );
        }
        row(
            &mut d,
            &mut wr,
            "",
            "Total",
            &[money(st(at(u, "total")))],
            true,
        );
        d.y -= 12;
    }
    let inc = at(&data, "investment_income");
    if !inc.is_null() {
        section(&mut d, &mut wr, &st(at(inc, "title")), &["Amount"]);
        for line in wrap(
            &wr.encode(&st(at(inc, "note"))),
            Font::Regular,
            8,
            RIGHT - MARGIN,
        ) {
            d.need(11);
            let y = d.y;
            d.cur.text(MARGIN, y, Font::Regular, 8, &line);
            d.y -= 11;
        }
        d.y -= 2;
        for r in list(at(inc, "rows")) {
            let amount = money(st(at(&r, "amount")));
            row(&mut d, &mut wr, "", &st(at(&r, "label")), &[amount], false);
            let note = st(at(&r, "note"));
            if !note.is_empty() {
                d.need(11);
                let y = d.y;
                d.cur.text(
                    NAME_X + 10,
                    y,
                    Font::Regular,
                    8,
                    &wr.encode(&printable(&note)),
                );
                d.y -= 11;
            }
        }
        row(
            &mut d,
            &mut wr,
            "",
            &st(at(inc, "total_label")),
            &[money(st(at(inc, "total")))],
            true,
        );
        d.y -= 12;
    }
    let m = at(&data, "mileage");
    if !m.is_null() {
        section(&mut d, &mut wr, &st(at(m, "title")), &["Miles", "Value"]);
        for v in list(at(m, "vehicles")) {
            heading(&mut d, &mut wr, &st(at(&v, "name")));
            for r in list(at(&v, "rows")) {
                row(
                    &mut d,
                    &mut wr,
                    "",
                    &st(at(&r, "label")),
                    &[
                        crate::mileage::miles_text(st(at(&r, "miles")).parse().unwrap_or_default()),
                        money(st(at(&r, "value"))),
                    ],
                    false,
                );
            }
            d.y -= 4;
        }
        for line in wrap(
            &wr.encode(&st(at(m, "note"))),
            Font::Regular,
            8,
            RIGHT - MARGIN,
        ) {
            d.need(11);
            let y = d.y;
            d.cur.text(MARGIN, y, Font::Regular, 8, &line);
            d.y -= 11;
        }
    }
    let last = std::mem::replace(&mut d.cur, Page { ops: Vec::new() });
    d.pages.push((last, Vec::new()));
    let n = d.pages.len();
    if n > 1 {
        for (i, (p, _)) in d.pages.iter_mut().enumerate() {
            let t = format!("Page {} of {n}", i + 1);
            p.right(RIGHT, MARGIN - 20, Font::Regular, 8, t.as_bytes());
        }
    }
    let bytes = crate::expense_pdf::assemble(&d.pages, &[]);
    let file = format!("tax-summary-{from}-to-{to}");
    Ok(crate::sales::pdf_answer(&file, bytes, &wr.replaced))
}
