//! Customer statements and late fees (business-dimensions.md Part B, module
//! `statements`).

use std::collections::BTreeMap;

use mk_core::{AccountSubtype, AccountType, Date, Decimal};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::module_settings::{self as ms, account_or_create};
use crate::sales::{
    DocArgs, Ledgerish, LineArg, Party, SalesDoc, build_doc, business_header, doc_json, docs,
    find_party, group, invoice_state, money, parties, pdf_answer, post_doc, round2, save_doc,
};
use crate::sales_reports::{BUCKETS, bucket};
use crate::{CommandError, CommandResult, Ctx, Registry};

const STATEMENTS: &str = "statements";

pub(crate) fn register(r: &mut Registry) {
    r.add_in(
        STATEMENTS,
        "statement.render",
        "A customer statement with its PDF",
        false,
        render,
    );
    r.add_in(
        STATEMENTS,
        "statement.batch",
        "Statements for many customers",
        false,
        batch,
    );
    r.add_in(
        STATEMENTS,
        "late_fee.preview",
        "Late fees due on overdue invoices",
        false,
        preview,
    );
    r.add_in(
        STATEMENTS,
        "late_fee.assess",
        "Charge late fees (one invoice per customer)",
        true,
        assess,
    );
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

fn plus(a: Decimal, b: Decimal) -> Decimal {
    a.checked_add(b).unwrap_or(a)
}

fn minus(a: Decimal, b: Decimal) -> Decimal {
    a.checked_sub(b).unwrap_or(a)
}

/// One statement entry: (date, kind, reference, amount; + owed, − paid).
struct Entry {
    date: Date,
    kind: &'static str,
    reference: String,
    amount: Decimal,
}

fn entries(ctx: &Ctx<'_>, l: &Ledgerish, customer: &str) -> CommandResult<Vec<Entry>> {
    let mut out = Vec::new();
    for d in docs(ctx, "invoice")? {
        if d.customer != customer || matches!(d.status.as_str(), "draft" | "void") {
            continue;
        }
        let kind = if d.late_fee_for.is_empty() {
            "Invoice"
        } else {
            "Late fee"
        };
        out.push(Entry {
            date: d.date,
            kind,
            reference: d.number.clone(),
            amount: d.totals.total,
        });
    }
    for c in l
        .credits
        .iter()
        .filter(|c| c.customer == customer && c.status != "void")
    {
        out.push(Entry {
            date: c.date,
            kind: "Credit memo",
            reference: c.number.clone(),
            amount: -c.totals.total,
        });
    }
    for p in l.payments.iter().filter(|p| p.customer == customer) {
        out.push(Entry {
            date: p.date,
            kind: "Payment",
            reference: p.reference.clone().unwrap_or_default(),
            amount: -p.amount,
        });
    }
    for r in l.refunds.iter().filter(|r| r.customer == customer) {
        out.push(Entry {
            date: r.date,
            kind: "Refund",
            reference: String::new(),
            amount: r.amount,
        });
    }
    out.sort_by_key(|e| e.date);
    Ok(out)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenderArgs {
    customer: String,
    from: Date,
    to: Date,
    /// `activity` (default) or `open_items`.
    #[serde(default)]
    kind: Option<String>,
}

/// Statement data and its PDF document for one customer.
fn statement(
    ctx: &Ctx<'_>,
    c: &Party,
    from: Date,
    to: Date,
    kind: &str,
) -> CommandResult<(Value, crate::pdf::SalesPdf)> {
    if from > to {
        return Err(invalid("the period starts after it ends"));
    }
    let l = Ledgerish::load(ctx)?;
    let all = entries(ctx, &l, &c.id)?;
    let opening = all
        .iter()
        .filter(|e| e.date < from)
        .fold(Decimal::ZERO, |s, e| plus(s, e.amount));
    // Aging of open invoices as of `to`.
    let mut aging = [Decimal::ZERO; 5];
    let mut open_rows = Vec::new();
    for d in docs(ctx, "invoice")? {
        if d.customer != c.id || matches!(d.status.as_str(), "draft" | "void") || d.date > to {
            continue;
        }
        let (_, bal, _) = invoice_state(&d, &l, Some(to));
        if bal <= Decimal::ZERO {
            continue;
        }
        let days = d.due_date.unwrap_or(d.date).days_until(to);
        if let Some(slot) = aging.get_mut(bucket(days)) {
            *slot = plus(*slot, bal);
        }
        open_rows.push((d, bal));
    }
    let mut rows = Vec::new();
    let mut lines = Vec::new();
    let closing;
    if kind == "open_items" {
        let mut total = Decimal::ZERO;
        for (d, bal) in &open_rows {
            total = plus(total, *bal);
            rows.push(
                json!({"date": d.date, "kind": "Invoice", "reference": d.number,
                "due_date": d.due_date, "amount": money(d.totals.total), "balance": money(*bal)}),
            );
            lines.push(crate::pdf::PdfLine {
                description: format!(
                    "{}  Invoice {} (due {})",
                    d.date,
                    d.number,
                    d.due_date.map(|x| x.to_string()).unwrap_or_default()
                ),
                quantity: String::new(),
                rate: group(&money(d.totals.total)),
                amount: group(&money(*bal)),
                emphasis: false,
            });
        }
        let credits = crate::sales::credits_total(ctx, &c.id, Some(to))?;
        closing = minus(total, credits);
        if !credits.is_zero() {
            rows.push(json!({"date": to, "kind": "Credits", "reference": "", "amount": money(-credits), "balance": money(closing)}));
            lines.push(crate::pdf::PdfLine {
                description: "Unapplied credits".into(),
                quantity: String::new(),
                rate: group(&money(-credits)),
                amount: group(&money(closing)),
                emphasis: false,
            });
        }
    } else {
        let mut running = opening;
        for e in all.iter().filter(|e| e.date >= from && e.date <= to) {
            running = plus(running, e.amount);
            rows.push(
                json!({"date": e.date, "kind": e.kind, "reference": e.reference,
                "amount": money(e.amount), "balance": money(running)}),
            );
            lines.push(crate::pdf::PdfLine {
                description: format!("{}  {} {}", e.date, e.kind, e.reference)
                    .trim_end()
                    .to_owned(),
                quantity: String::new(),
                rate: group(&money(e.amount)),
                amount: group(&money(running)),
                emphasis: false,
            });
        }
        closing = running;
    }
    let paper = ms::get_str(ctx, "invoicing.paper").unwrap_or_default();
    let tpl = crate::templates::for_document(ctx, None)?;
    let (business_name, business_lines) = business_header(ctx);
    let aging_text = BUCKETS
        .iter()
        .zip(aging.iter())
        .map(|(b, v)| format!("{b} {}", group(&money(*v))))
        .collect::<Vec<_>>()
        .join("   ");
    let mut totals = Vec::new();
    if kind != "open_items" {
        totals.push(("Opening balance".to_owned(), group(&money(opening)), false));
    }
    totals.push(("Closing balance".to_owned(), group(&money(closing)), true));
    let doc = crate::pdf::SalesPdf {
        title: crate::templates::title(tpl.as_ref(), "statement", "Statement"),
        business_name,
        business_lines,
        facts: Some(vec![
            ("Statement date".into(), to.to_string()),
            ("Period".into(), format!("{from} to {to}")),
        ]),
        party_heading: "To",
        bill_to: std::iter::once(c.name.clone())
            .chain(c.company.clone())
            .chain(c.billing_address.iter().cloned())
            .collect(),
        headers: Some(["Date and item", "", "Amount", "Balance"].map(str::to_owned)),
        lines,
        totals: Some(totals),
        message: format!("Aging as of {to}: {aging_text}"),
        a4: paper == "a4",
        style: crate::templates::style(tpl.as_ref()),
        ..crate::pdf::SalesPdf::default()
    };
    let data = json!({
        "customer": c.id, "customer_name": c.name, "from": from, "to": to, "kind": kind,
        "opening": money(opening), "rows": rows, "closing": money(closing),
        "aging": {"columns": BUCKETS, "buckets": aging.iter().map(|v| money(*v)).collect::<Vec<_>>()},
    });
    Ok((data, doc))
}

fn kind_of(k: Option<String>) -> CommandResult<String> {
    let k = k.unwrap_or_else(|| "activity".into());
    if !matches!(k.as_str(), "activity" | "open_items") {
        return Err(invalid("kind is activity or open_items"));
    }
    Ok(k)
}

fn render(ctx: &Ctx<'_>, a: RenderArgs) -> CommandResult<Value> {
    let c = find_party(ctx, &a.customer)?;
    let kind = kind_of(a.kind)?;
    let (mut data, doc) = statement(ctx, &c, a.from, a.to, &kind)?;
    let (bytes, replaced) = crate::pdf::render(&doc);
    let pdf = pdf_answer(&format!("Statement {} {}", c.name, a.to), bytes, &replaced);
    if let (Some(o), Some(p)) = (data.as_object_mut(), pdf.as_object()) {
        for (k, v) in p {
            o.insert(k.clone(), v.clone());
        }
    }
    Ok(data)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BatchArgs {
    to: Date,
    /// Default: 30 days before `to`.
    #[serde(default)]
    from: Option<Date>,
    /// Default: every customer with a balance.
    #[serde(default)]
    customers: Option<Vec<String>>,
    #[serde(default)]
    kind: Option<String>,
    /// One PDF for all customers instead of one each.
    #[serde(default)]
    combined: bool,
}

fn batch(ctx: &Ctx<'_>, a: BatchArgs) -> CommandResult<Value> {
    let kind = kind_of(a.kind)?;
    let from = a.from.unwrap_or_else(|| a.to.add_days(-30).unwrap_or(a.to));
    let chosen: Vec<Party> = match a.customers {
        Some(list) => list
            .iter()
            .map(|r| find_party(ctx, r))
            .collect::<CommandResult<_>>()?,
        None => {
            let l = Ledgerish::load(ctx)?;
            let mut v = Vec::new();
            for p in parties(ctx)?.into_iter().filter(|p| p.is("customer")) {
                let bal = entries(ctx, &l, &p.id)?
                    .iter()
                    .filter(|e| e.date <= a.to)
                    .fold(Decimal::ZERO, |s, e| plus(s, e.amount));
                if !bal.is_zero() {
                    v.push(p);
                }
            }
            v
        }
    };
    let mut docs_out = Vec::new();
    let mut summaries = Vec::new();
    for c in &chosen {
        let (data, doc) = statement(ctx, c, from, a.to, &kind)?;
        summaries.push(
            json!({"customer": c.id, "customer_name": c.name, "closing": data.get("closing")}),
        );
        docs_out.push((c.name.clone(), doc));
    }
    if a.combined {
        let all: Vec<crate::pdf::SalesPdf> = docs_out.into_iter().map(|(_, d)| d).collect();
        let (bytes, replaced) = crate::pdf::render_many(&all);
        let pdf = pdf_answer(&format!("Statements {}", a.to), bytes, &replaced);
        return Ok(json!({"statements": summaries, "pdf": pdf}));
    }
    let files: Vec<Value> = docs_out
        .iter()
        .map(|(name, d)| {
            let (bytes, replaced) = crate::pdf::render(d);
            pdf_answer(&format!("Statement {name} {}", a.to), bytes, &replaced)
        })
        .collect();
    Ok(json!({"statements": summaries, "files": files}))
}

// ----- late fees ----------------------------------------------------------------------

/// A proposed fee: (invoice, fee).
fn proposals(ctx: &Ctx<'_>, as_of: Date) -> CommandResult<Vec<(SalesDoc, Decimal, i64, Decimal)>> {
    let kind = ms::get_str(ctx, "statements.late_fee_kind")?;
    if kind == "none" {
        return Ok(Vec::new());
    }
    let dec = |k: &str| -> CommandResult<Decimal> {
        Ok(ms::get_str(ctx, k)?.parse().unwrap_or_default())
    };
    let (pct, flat, min) = (
        dec("statements.late_fee_percent")?,
        dec("statements.late_fee_flat")?,
        dec("statements.late_fee_min")?,
    );
    let grace = ms::get_i64(ctx, "statements.grace_days")?;
    let l = Ledgerish::load(ctx)?;
    let all = docs(ctx, "invoice")?;
    // Invoice id → the latest date a fee was charged on it.
    let mut charged: BTreeMap<String, Date> = BTreeMap::new();
    for f in all
        .iter()
        .filter(|d| !d.late_fee_for.is_empty() && d.status != "void")
    {
        for id in &f.late_fee_for {
            let e = charged.entry(id.clone()).or_insert(f.date);
            *e = (*e).max(f.date);
        }
    }
    let mut out = Vec::new();
    for d in all {
        if !d.late_fee_for.is_empty() || matches!(d.status.as_str(), "draft" | "void") {
            continue;
        }
        let Some(due) = d.due_date else { continue };
        let late = due.days_until(as_of);
        if late <= grace {
            continue;
        }
        if charged
            .get(&d.id)
            .is_some_and(|last| last.days_until(as_of) < 30)
        {
            continue;
        }
        let (_, bal, _) = invoice_state(&d, &l, Some(as_of));
        if bal <= Decimal::ZERO {
            continue;
        }
        let fee = if kind == "percent" {
            round2(
                bal.checked_mul(pct)
                    .and_then(|x| x.checked_div(Decimal::ONE_HUNDRED))
                    .unwrap_or_default(),
            )
            .max(min)
        } else {
            round2(flat)
        };
        if fee > Decimal::ZERO {
            out.push((d, bal, late, fee));
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PreviewArgs {
    as_of: Date,
}

fn preview(ctx: &Ctx<'_>, a: PreviewArgs) -> CommandResult<Value> {
    let names: BTreeMap<String, String> =
        parties(ctx)?.into_iter().map(|p| (p.id, p.name)).collect();
    let rows: Vec<Value> = proposals(ctx, a.as_of)?
        .iter()
        .map(|(d, bal, late, fee)| {
            json!({"invoice": d.id, "number": d.number, "customer": d.customer,
                "customer_name": names.get(&d.customer), "due_date": d.due_date,
                "days_late": late, "balance": money(*bal), "fee": money(*fee)})
        })
        .collect();
    let kind = ms::get_str(ctx, "statements.late_fee_kind")?;
    Ok(json!({"as_of": a.as_of, "kind": kind, "fees": rows}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AssessArgs {
    as_of: Date,
    /// Invoices (ids or numbers) to charge; default every proposed one.
    #[serde(default)]
    invoices: Option<Vec<String>>,
}

fn assess(ctx: &Ctx<'_>, a: AssessArgs) -> CommandResult<Value> {
    let mut chosen = proposals(ctx, a.as_of)?;
    if let Some(list) = &a.invoices {
        chosen.retain(|(d, ..)| {
            list.iter()
                .any(|r| *r == d.id || r.eq_ignore_ascii_case(&d.number))
        });
        if chosen.len() != list.len() {
            return Err(invalid("some of those invoices have no late fee due"));
        }
    }
    if chosen.is_empty() {
        return Err(CommandError::Rejected("no late fees are due".into()));
    }
    let account = account_or_create(
        ctx,
        "statements.late_fee_account",
        "Late Fee Income",
        AccountType::Income,
        AccountSubtype::Income,
    )?;
    let mut by: BTreeMap<String, Vec<(SalesDoc, Decimal)>> = BTreeMap::new();
    for (d, _, _, fee) in chosen {
        by.entry(d.customer.clone()).or_default().push((d, fee));
    }
    let mut created = Vec::new();
    for (customer, list) in by {
        let lines: Vec<LineArg> = list
            .iter()
            .map(|(d, fee)| LineArg {
                account: Some(account.to_string()),
                description: Some(format!("Late fee — {}", d.number)),
                amount: Some(*fee),
                taxable: Some(false),
                ..LineArg::default()
            })
            .collect();
        let args = DocArgs {
            customer: Some(customer),
            date: Some(a.as_of),
            terms: Some(json!({"type": "due_on_receipt"})),
            lines: Some(lines),
            tax_rate: Some(String::new()),
            memo: Some(format!(
                "Late fee — {}",
                list.iter()
                    .map(|(d, _)| d.number.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            ..DocArgs::default()
        };
        let mut inv = build_doc(ctx, "invoice", args, None)?;
        inv.late_fee_for = list.iter().map(|(d, _)| d.id.clone()).collect();
        inv.status = "open".into();
        post_doc(ctx, &mut inv)?;
        save_doc(ctx, &inv)?;
        created.push(doc_json(ctx, &inv)?);
    }
    Ok(json!({"invoices": created}))
}
