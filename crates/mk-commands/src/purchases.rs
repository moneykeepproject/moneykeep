//! Business purchases (business-purchases.md): vendors' bills, bill payments with
//! check numbering, vendor credits, refunds received, purchase orders and the optional
//! vendor link of ordinary transactions.
//!
//! Bills, vendor credits and purchase orders are `bill_doc` records grouped by kind;
//! bill payments and refunds received are `bill_payment` records. Bills and credits
//! post one transaction each; applications of payments and credits to bills live on
//! the payment or credit and change only balances. Purchase orders never post: what was
//! received is read from the bills linked to them.

use std::collections::BTreeMap;

use mk_core::{
    AccountId, AccountSubtype, AccountType, Date, Decimal, SplitTags, TransactionId, TxnStatus,
};
use mk_store::{DocTable, now_rfc3339};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::module_settings::{self as ms, check_terms, terms_label};
use crate::sales::{
    Party, RefundSource, add, cash, date_ord, decimals, due_from, find_party, group, invalid,
    items, load, money, mul, new_id, parties, post_txn, post_txn_numbered, rejected, remove_txn,
    require_open_period, round2, save, sub, text,
};
use crate::txns::validate_change;
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) const BILLS: &str = "bills";
pub(crate) const POS: &str = "purchase-orders";

pub(crate) fn register(r: &mut Registry) {
    let b = BILLS;
    r.add_in(
        b,
        "bill.create",
        "Enter a vendor's bill (posts it)",
        true,
        bill_create,
    );
    r.add_in(
        b,
        "bill.update",
        "Change a bill (rewrites its transaction)",
        true,
        bill_update,
    );
    r.add_in(
        b,
        "bill.void",
        "Void a bill without payments",
        true,
        bill_void,
    );
    r.add_in(
        b,
        "bill.delete",
        "Delete a bill without payments",
        true,
        bill_delete,
    );
    r.add_in(
        b,
        "bill.get",
        "Get a bill with its balance",
        false,
        bill_get,
    );
    r.add_in(b, "bill.list", "List bills", false, bill_list);
    r.add_in(
        b,
        "bill.duplicate",
        "Copy a bill as a new bill",
        true,
        bill_duplicate,
    );
    r.add_in(
        b,
        "bill.preview",
        "Lines and total for unsaved bill data",
        false,
        bill_preview,
    );
    r.add_in(
        b,
        "bill.open",
        "A vendor's open bills with the amounts a payment would apply",
        false,
        bill_open,
    );
    r.add_in(
        b,
        "bill.attention",
        "Counts of bills overdue and due in the next 7 days",
        false,
        bill_attention,
    );
    r.add_in(b, "bill.pay", "Pay a vendor's bills", true, bill_pay);
    r.add_in(
        b,
        "bill.pay_many",
        "Pay several bills: one payment per vendor",
        true,
        bill_pay_many,
    );
    r.add_in(
        b,
        "bill_payment.apply",
        "Apply a payment's unapplied amount to bills",
        true,
        payment_apply,
    );
    r.add_in(
        b,
        "bill_payment.unapply",
        "Remove a payment's application to a bill",
        true,
        payment_unapply,
    );
    r.add_in(
        b,
        "bill_payment.void",
        "Void a bill payment (its bills reopen)",
        true,
        payment_void,
    );
    r.add_in(
        b,
        "bill_payment.get",
        "Get a bill payment",
        false,
        payment_get,
    );
    r.add_in(
        b,
        "bill_payment.list",
        "List bill payments",
        false,
        payment_list,
    );
    r.add_in(
        b,
        "bill_payment.next_check",
        "The next check number of a bank account",
        false,
        next_check,
    );
    r.add_in(
        b,
        "vendor_credit.create",
        "Enter a vendor credit",
        true,
        credit_create,
    );
    r.add_in(
        b,
        "vendor_credit.apply",
        "Apply a vendor credit to bills",
        true,
        credit_apply,
    );
    r.add_in(
        b,
        "vendor_credit.unapply",
        "Remove a vendor credit's application",
        true,
        credit_unapply,
    );
    r.add_in(
        b,
        "vendor_credit.delete",
        "Delete an unapplied vendor credit",
        true,
        credit_delete,
    );
    r.add_in(
        b,
        "vendor_credit.get",
        "Get a vendor credit",
        false,
        credit_get,
    );
    r.add_in(
        b,
        "vendor_credit.list",
        "List vendor credits",
        false,
        credit_list,
    );
    r.add_in(
        b,
        "vendor.refund_received",
        "Record money a vendor paid back",
        true,
        refund_received,
    );
    r.add_in(
        b,
        "vendor.summary",
        "A vendor's open bills, payments and credits",
        false,
        vendor_summary,
    );
    r.add_in(
        b,
        "txn.set_vendor",
        "Link a transaction to a vendor (1099)",
        true,
        set_vendor,
    );
    r.add_in(
        b,
        "txn.vendor_links",
        "Vendor links of transactions",
        false,
        vendor_links,
    );
    r.add_in(POS, "po.create", "Create a purchase order", true, po_create);
    r.add_in(POS, "po.update", "Change a purchase order", true, po_update);
    r.add_in(
        POS,
        "po.set_status",
        "Open, close or cancel a purchase order",
        true,
        po_set_status,
    );
    r.add_in(
        POS,
        "po.receive",
        "Receive purchase order lines (creates a bill)",
        true,
        po_receive,
    );
    r.add_in(POS, "po.get", "Get a purchase order", false, po_get);
    r.add_in(POS, "po.list", "List purchase orders", false, po_list);
    r.add_in(
        POS,
        "po.render_pdf",
        "The purchase order as a PDF",
        false,
        po_pdf,
    );
    r.add_in(
        POS,
        "po.delete",
        "Delete a purchase order without receipts",
        true,
        po_delete,
    );
}

// ----- records ------------------------------------------------------------------------

/// A line as entered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BillLineArg {
    /// Item id or name (items module): an item line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// Expense, asset or liability account: an expense line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub quantity: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub cost: Option<Decimal>,
    /// Typed amount (expense lines; item lines: otherwise quantity × cost).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub amount: Option<Decimal>,
    /// The purchase order line (1-based) a received line came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub po_line: Option<usize>,
    /// Line class, location and fund (override the document's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fund: Option<String>,
    /// Line project (overrides the document's; module projects).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// A billable project cost (default: the project's billable setting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub billable: Option<bool>,
}

/// A line as computed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BillLine {
    pub item: Option<String>,
    pub item_name: Option<String>,
    pub account: AccountId,
    pub description: String,
    pub quantity: Option<Decimal>,
    pub cost: Option<Decimal>,
    pub amount: Decimal,
    #[serde(default)]
    pub po_line: Option<usize>,
    #[serde(default, skip_serializing_if = "SplitTags::is_empty")]
    pub tags: SplitTags,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BillApp {
    pub bill: String,
    pub amount: Decimal,
    pub date: Date,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BillDoc {
    pub id: String,
    /// `bill`, `vendor_credit` or `po`.
    pub kind: String,
    #[serde(default)]
    pub number: Option<String>,
    pub vendor: String,
    pub date: Date,
    #[serde(default)]
    pub terms: Option<Value>,
    #[serde(default)]
    pub due_date: Option<Date>,
    pub lines: Vec<BillLineArg>,
    pub computed: Vec<BillLine>,
    pub total: Decimal,
    #[serde(default)]
    pub memo: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub ship_to: Vec<String>,
    #[serde(default)]
    pub expected_date: Option<Date>,
    /// Bills and credits: open or void. Purchase orders: open, closed or cancelled.
    pub status: String,
    #[serde(default)]
    pub txn: Option<TransactionId>,
    /// A bill's purchase order.
    #[serde(default)]
    pub po: Option<String>,
    /// Vendor credits: applications to bills.
    #[serde(default)]
    pub applied: Vec<BillApp>,
    pub created_at: String,
    /// Document-level class, location and fund (ids).
    #[serde(default, skip_serializing_if = "SplitTags::is_empty")]
    pub tags: SplitTags,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BillPayment {
    pub id: String,
    pub vendor: String,
    pub date: Date,
    pub amount: Decimal,
    pub method: String,
    pub from_account: AccountId,
    #[serde(default)]
    pub check_number: Option<i64>,
    #[serde(default)]
    pub reference: Option<String>,
    pub txn: TransactionId,
    pub applied: Vec<BillApp>,
    #[serde(default)]
    pub void: bool,
    #[serde(default)]
    pub memo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RefundReceived {
    pub id: String,
    pub vendor: String,
    pub date: Date,
    pub amount: Decimal,
    pub to_account: AccountId,
    pub txn: TransactionId,
    pub sources: Vec<RefundSource>,
}

pub(crate) fn bill_docs(ctx: &Ctx<'_>, kind: &str) -> CommandResult<Vec<BillDoc>> {
    let mut v: Vec<BillDoc> = load(ctx, DocTable::BillDoc, Some(kind))?;
    v.sort_by(|a, b| (a.date, &a.created_at).cmp(&(b.date, &b.created_at)));
    Ok(v)
}

fn noun(kind: &str) -> &'static str {
    match kind {
        "vendor_credit" => "vendor credit",
        "po" => "purchase order",
        _ => "bill",
    }
}

fn find_doc(ctx: &Ctx<'_>, kind: &str, r: &str) -> CommandResult<BillDoc> {
    let r = r.trim();
    let all = bill_docs(ctx, kind)?;
    if let Some(d) = all.iter().find(|d| d.id == r) {
        return Ok(d.clone());
    }
    let hits: Vec<&BillDoc> = all
        .iter()
        .filter(|d| {
            d.number
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(r))
        })
        .collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(CommandError::NotFound(format!("{} {r:?}", noun(kind)))),
        _ => Err(invalid(format!(
            "several {}s are numbered {r}; use the id",
            noun(kind)
        ))),
    }
}

fn save_doc(ctx: &Ctx<'_>, d: &BillDoc) -> CommandResult<()> {
    save(
        ctx,
        DocTable::BillDoc,
        &d.id,
        Some(&d.kind),
        date_ord(d.date),
        d,
    )
}

pub(crate) fn bill_payments(ctx: &Ctx<'_>) -> CommandResult<Vec<BillPayment>> {
    let mut v: Vec<BillPayment> = load(ctx, DocTable::BillPayment, Some("payment"))?;
    v.sort_by_key(|p| (p.date, p.check_number, p.id.clone()));
    Ok(v)
}

fn find_payment(ctx: &Ctx<'_>, r: &str) -> CommandResult<BillPayment> {
    bill_payments(ctx)?
        .into_iter()
        .find(|p| p.id == r.trim())
        .ok_or_else(|| CommandError::NotFound(format!("bill payment {r:?}")))
}

fn save_payment(ctx: &Ctx<'_>, p: &BillPayment) -> CommandResult<()> {
    save(
        ctx,
        DocTable::BillPayment,
        &p.id,
        Some("payment"),
        date_ord(p.date),
        p,
    )
}

pub(crate) fn payable(ctx: &Ctx<'_>) -> CommandResult<AccountId> {
    crate::module_settings::account_or_create_role(
        ctx,
        "bills.payable_account",
        Some(crate::chart::roles::PAYABLE),
        "Accounts Payable",
        AccountType::Liability,
        AccountSubtype::Payable,
    )
}

/// The payable account if it exists (reports never create it).
pub(crate) fn payable_id(ctx: &Ctx<'_>) -> CommandResult<Option<AccountId>> {
    let chart = Chart::load(ctx)?;
    if let Some(id) = ms::get(ctx, "bills.payable_account")?.as_str()
        && let Ok(a) = chart.resolve(id)
    {
        return Ok(Some(a.id));
    }
    if let Some(a) = crate::chart::roles::holder(&chart, crate::chart::roles::PAYABLE) {
        return Ok(Some(a.id));
    }
    Ok(chart
        .accounts
        .iter()
        .find(|a| {
            a.parent.is_none()
                && a.subtype == AccountSubtype::Payable
                && a.name.eq_ignore_ascii_case("Accounts Payable")
        })
        .map(|a| a.id))
}

/// A vendor by id or name (bill schedules).
pub(crate) fn vendor_of(ctx: &Ctx<'_>, r: &str) -> CommandResult<Party> {
    vendor(ctx, r)
}

/// Bill lines checked and computed as `bill.create` does (bill schedules).
pub(crate) fn compute_lines(
    ctx: &Ctx<'_>,
    lines: &[BillLineArg],
) -> CommandResult<(Vec<BillLine>, Decimal)> {
    compute(ctx, lines)
}

/// Payment terms checked as `bill.create` does (bill schedules).
pub(crate) fn check_terms_value(t: &Value) -> CommandResult<Value> {
    check_terms(t)
}

/// `bill.create` from JSON arguments, for commands that create a bill as part of
/// their own change (a bill schedule entered directly).
pub(crate) fn create_bill_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    bill_create(ctx, crate::parse_args(args)?)
}

fn vendor(ctx: &Ctx<'_>, r: &str) -> CommandResult<Party> {
    let p = find_party(ctx, r)?;
    if !p.is("vendor") {
        return Err(invalid(format!("{} is not a vendor", p.name)));
    }
    Ok(p)
}

/// Everything that changes bill balances.
pub(crate) struct Book {
    pub payments: Vec<BillPayment>,
    pub credits: Vec<BillDoc>,
    pub refunds: Vec<RefundReceived>,
}

impl Book {
    pub(crate) fn load(ctx: &Ctx<'_>) -> CommandResult<Self> {
        Ok(Self {
            payments: bill_payments(ctx)?,
            credits: bill_docs(ctx, "vendor_credit")?,
            refunds: load(ctx, DocTable::BillPayment, Some("refund"))?,
        })
    }

    fn apps(&self) -> impl Iterator<Item = &BillApp> {
        self.payments
            .iter()
            .filter(|p| !p.void)
            .flat_map(|p| p.applied.iter())
            .chain(
                self.credits
                    .iter()
                    .filter(|c| c.status != "void")
                    .flat_map(|c| c.applied.iter()),
            )
    }

    /// Paid and credited on a bill as of a date (all when `None`).
    pub(crate) fn applied_to(&self, bill: &str, as_of: Option<Date>) -> Decimal {
        self.apps()
            .filter(|a| a.bill == bill && as_of.is_none_or(|x| a.date <= x))
            .fold(Decimal::ZERO, |s, a| s.checked_add(a.amount).unwrap_or(s))
    }

    fn refunded_from(&self, source: &str, as_of: Option<Date>) -> Decimal {
        self.refunds
            .iter()
            .filter(|r| as_of.is_none_or(|x| r.date <= x))
            .flat_map(|r| r.sources.iter())
            .filter(|s| s.source == source)
            .fold(Decimal::ZERO, |s, x| s.checked_add(x.amount).unwrap_or(s))
    }

    fn applied_sum(apps: &[BillApp], as_of: Option<Date>) -> Decimal {
        apps.iter()
            .filter(|a| as_of.is_none_or(|x| a.date <= x))
            .fold(Decimal::ZERO, |s, a| s.checked_add(a.amount).unwrap_or(s))
    }

    /// A payment's unapplied amount (a prepayment).
    pub(crate) fn unapplied_payment(&self, p: &BillPayment, as_of: Option<Date>) -> Decimal {
        if p.void || as_of.is_some_and(|x| p.date > x) {
            return Decimal::ZERO;
        }
        p.amount
            .checked_sub(Self::applied_sum(&p.applied, as_of))
            .and_then(|x| x.checked_sub(self.refunded_from(&p.id, as_of)))
            .unwrap_or_default()
    }

    /// A vendor credit's unapplied amount.
    pub(crate) fn unapplied_credit(&self, c: &BillDoc, as_of: Option<Date>) -> Decimal {
        if c.status == "void" || as_of.is_some_and(|x| c.date > x) {
            return Decimal::ZERO;
        }
        c.total
            .checked_sub(Self::applied_sum(&c.applied, as_of))
            .and_then(|x| x.checked_sub(self.refunded_from(&c.id, as_of)))
            .unwrap_or_default()
    }

    /// A vendor's unapplied credits and prepayments, oldest first: (id, date, amount).
    pub(crate) fn credits_of(
        &self,
        vendor: &str,
        as_of: Option<Date>,
    ) -> Vec<(String, Date, Decimal)> {
        let mut out: Vec<(String, Date, Decimal)> = self
            .payments
            .iter()
            .filter(|p| p.vendor == vendor)
            .map(|p| (p.id.clone(), p.date, self.unapplied_payment(p, as_of)))
            .chain(
                self.credits
                    .iter()
                    .filter(|c| c.vendor == vendor)
                    .map(|c| (c.id.clone(), c.date, self.unapplied_credit(c, as_of))),
            )
            .filter(|x| x.2 > Decimal::ZERO)
            .collect();
        out.sort_by_key(|x| (x.1, x.0.clone()));
        out
    }
}

/// A bill's balance and display status.
pub(crate) fn bill_state(d: &BillDoc, b: &Book, as_of: Option<Date>) -> (Decimal, Decimal, String) {
    let applied = b.applied_to(&d.id, as_of);
    let balance = d.total.checked_sub(applied).unwrap_or_default();
    let status = match d.status.as_str() {
        "void" => "void".to_owned(),
        _ if balance.is_zero() && d.total > Decimal::ZERO => "paid".to_owned(),
        _ if applied > Decimal::ZERO => "partial".to_owned(),
        _ => "open".to_owned(),
    };
    (applied, balance, status)
}

// ----- lines --------------------------------------------------------------------------

fn compute(ctx: &Ctx<'_>, lines: &[BillLineArg]) -> CommandResult<(Vec<BillLine>, Decimal)> {
    if lines.is_empty() || lines.len() > 500 {
        return Err(invalid("a bill has 1 to 500 lines"));
    }
    let chart = Chart::load(ctx)?;
    let items_on = crate::sales::module_on(ctx, crate::sales::ITEMS)?;
    let all_items = if items_on { items(ctx)? } else { Vec::new() };
    let mut out = Vec::new();
    let mut total = Decimal::ZERO;
    for (i, l) in lines.iter().enumerate() {
        let n = i + 1;
        let description = text(
            l.description.clone(),
            1000,
            &format!("line {n} description"),
        )?;
        for (v, what) in [(l.quantity, "quantity"), (l.cost, "cost")] {
            if v.is_some_and(|x| decimals(x) > 4) {
                return Err(invalid(format!("line {n}: {what} has at most 4 decimals")));
            }
        }
        if l.amount.is_some_and(|x| decimals(x) > 2) {
            return Err(invalid(format!("line {n}: amount has at most 2 decimals")));
        }
        let tags = crate::dimensions::resolve_tags_p(
            ctx,
            l.class.as_deref(),
            l.location.as_deref(),
            l.fund.as_deref(),
            l.project.as_deref(),
            l.billable,
        )
        .map_err(|e| invalid(format!("line {n}: {e}")))?;
        let line = match (&l.item, &l.account) {
            (Some(r), _) => {
                if !items_on {
                    return Err(invalid(format!("line {n}: items are off; name an account")));
                }
                let it = all_items
                    .iter()
                    .find(|x| x.id == *r || x.name.eq_ignore_ascii_case(r.trim()))
                    .ok_or_else(|| invalid(format!("line {n}: no item {r:?}")))?;
                let account = it.expense_account.ok_or_else(|| {
                    invalid(format!(
                        "line {n}: {} has no expense account for purchases; set one on the item",
                        it.name
                    ))
                })?;
                let q = l.quantity.unwrap_or(Decimal::ONE);
                let cost = l.cost.or(it.cost).unwrap_or(it.price);
                let amount = match l.amount {
                    Some(a) => a,
                    None => round2(mul(q, cost)?),
                };
                BillLine {
                    item: Some(it.id.clone()),
                    item_name: Some(it.name.clone()),
                    account,
                    description: description
                        .or_else(|| it.description.clone())
                        .unwrap_or_else(|| it.name.clone()),
                    quantity: Some(q),
                    cost: Some(cost),
                    amount,
                    po_line: l.po_line,
                    tags: tags.clone(),
                }
            }
            (None, Some(acc)) => {
                let a = chart.resolve(acc)?;
                let ok = matches!(
                    a.account_type,
                    AccountType::Expense | AccountType::Asset | AccountType::Liability
                ) && a.subtype != AccountSubtype::Payable;
                if !ok || a.placeholder {
                    return Err(invalid(format!(
                        "line {n}: {} is not an expense, asset or liability account",
                        chart.full_name(a.id)
                    )));
                }
                let amount = match (l.amount, l.quantity, l.cost) {
                    (Some(a), _, _) => a,
                    (None, q, Some(c)) => round2(mul(q.unwrap_or(Decimal::ONE), c)?),
                    _ => return Err(invalid(format!("line {n}: give the amount"))),
                };
                BillLine {
                    item: None,
                    item_name: None,
                    account: a.id,
                    description: description.unwrap_or_default(),
                    quantity: l.quantity,
                    cost: l.cost,
                    amount,
                    po_line: l.po_line,
                    tags: tags.clone(),
                }
            }
            (None, None) => {
                return Err(invalid(format!("line {n}: give an item or an account")));
            }
        };
        total = add(total, line.amount)?;
        out.push(line);
    }
    Ok((out, total))
}

fn lines_json(chart: &Chart, lines: &[BillLine]) -> Vec<Value> {
    lines
        .iter()
        .map(|l| {
            json!({
                "item": l.item, "item_name": l.item_name, "account": l.account,
                "account_name": chart.full_name(l.account), "description": l.description,
                "quantity": l.quantity.map(|q| q.normalize().to_string()),
                "cost": l.cost.map(|c| c.normalize().to_string()),
                "amount": money(l.amount), "po_line": l.po_line,
            })
        })
        .collect()
}

// ----- documents ----------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BillArgs {
    #[serde(default)]
    pub vendor: Option<String>,
    /// The vendor's invoice number (bills, credits) or the PO number.
    #[serde(default)]
    pub number: Option<String>,
    #[serde(default)]
    pub date: Option<Date>,
    #[serde(default)]
    pub terms: Option<Value>,
    #[serde(default)]
    pub due_date: Option<Date>,
    #[serde(default)]
    pub lines: Option<Vec<BillLineArg>>,
    #[serde(default)]
    pub memo: Option<String>,
    /// Purchase orders: printed message.
    #[serde(default)]
    pub message: Option<String>,
    /// Purchase orders: ship-to address lines.
    #[serde(default)]
    pub ship_to: Option<Vec<String>>,
    /// Purchase orders: expected delivery date.
    #[serde(default)]
    pub expected_date: Option<Date>,
    /// Document class, location and fund (lines may override; "" clears).
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub fund: Option<String>,
    /// Document project (lines may override; "" clears; module projects).
    #[serde(default)]
    pub project: Option<String>,
    /// Bills: the schedule occurrence this bill enters (`{schedule, date}`; date
    /// default the earliest pending), for schedules that create a bill.
    #[serde(default)]
    pub from_schedule: Option<FromSchedule>,
}

/// A bill schedule's occurrence (subscriptions.md revision 2).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FromSchedule {
    /// Schedule id or name.
    pub schedule: String,
    /// Nominal date of the occurrence.
    #[serde(default)]
    pub date: Option<Date>,
}

fn vendor_terms(ctx: &Ctx<'_>, v: &Party) -> CommandResult<Value> {
    match &v.vendor_terms {
        Some(t) => Ok(t.clone()),
        None => ms::get(ctx, "bills.default_terms"),
    }
}

/// The next PO number, advancing the sequence when `advance`.
fn po_number(ctx: &Ctx<'_>, typed: Option<String>, advance: bool) -> CommandResult<String> {
    let existing: Vec<String> = bill_docs(ctx, "po")?
        .into_iter()
        .filter_map(|d| d.number.map(|n| n.to_lowercase()))
        .collect();
    if let Some(t) = typed.and_then(|t| text(Some(t), 20, "number").transpose()) {
        let t = t?;
        if existing.contains(&t.to_lowercase()) {
            return Err(rejected(format!("number {t} is already used")));
        }
        return Ok(t);
    }
    let prefix = ms::get_str(ctx, "purchase-orders.number_prefix")?;
    let mut n = ms::get_i64(ctx, "purchase-orders.next_number")?;
    loop {
        let candidate = format!("{prefix}{n}");
        n = n.saturating_add(1);
        if !existing.contains(&candidate.to_lowercase()) {
            if advance {
                ms::put(ctx, "purchase-orders.next_number", json!(n))?;
            }
            return Ok(candidate);
        }
    }
}

fn build(
    ctx: &Ctx<'_>,
    kind: &str,
    a: BillArgs,
    base: Option<BillDoc>,
    advance: bool,
) -> CommandResult<BillDoc> {
    let vref = a
        .vendor
        .clone()
        .or_else(|| base.as_ref().map(|b| b.vendor.clone()))
        .ok_or_else(|| invalid("give the vendor"))?;
    let v = vendor(ctx, &vref)?;
    let date = a
        .date
        .or(base.as_ref().map(|b| b.date))
        .map_or_else(mk_store::today_utc, Ok)?;
    let lines = a
        .lines
        .clone()
        .or_else(|| base.as_ref().map(|b| b.lines.clone()))
        .ok_or_else(|| invalid("give the lines"))?;
    let (computed, total) = compute(ctx, &lines)?;
    if kind != "po" && total <= Decimal::ZERO {
        return Err(invalid(format!(
            "a {}'s total must be more than zero",
            noun(kind)
        )));
    }
    let number = match kind {
        "po" => match &base {
            Some(b) => match &a.number {
                Some(n)
                    if Some(n.to_lowercase()) != b.number.as_ref().map(|x| x.to_lowercase()) =>
                {
                    Some(po_number(ctx, Some(n.clone()), advance)?)
                }
                _ => b.number.clone(),
            },
            None => Some(po_number(ctx, a.number.clone(), advance)?),
        },
        _ => {
            let n = match (&a.number, &base) {
                (Some(n), _) => text(Some(n.clone()), 40, "number")?,
                (None, Some(b)) => b.number.clone(),
                (None, None) => None,
            };
            if let Some(n) = &n {
                let id = base.as_ref().map(|b| b.id.clone()).unwrap_or_default();
                if bill_docs(ctx, kind)?.iter().any(|d| {
                    d.id != id
                        && d.vendor == v.id
                        && d.status != "void"
                        && d.number
                            .as_deref()
                            .is_some_and(|x| x.eq_ignore_ascii_case(n))
                }) {
                    return Err(rejected(format!(
                        "{} already has a {} numbered {n}",
                        v.name,
                        noun(kind)
                    )));
                }
            }
            n
        }
    };
    let terms = match (&a.terms, &base) {
        (Some(t), _) => Some(check_terms(t)?),
        (None, Some(b)) if b.terms.is_some() => b.terms.clone(),
        _ if kind == "bill" => Some(vendor_terms(ctx, &v)?),
        _ => None,
    };
    let due_date = match (kind, a.due_date) {
        ("bill", Some(d)) => Some(d),
        ("bill", None) => {
            let keep = base
                .as_ref()
                .filter(|_| a.terms.is_none() && a.date.is_none())
                .and_then(|b| b.due_date);
            keep.or_else(|| terms.as_ref().map(|t| due_from(t, date)))
        }
        _ => None,
    };
    let memo = match (&a.memo, &base) {
        (Some(m), _) => text(Some(m.clone()), 1000, "memo")?,
        (None, Some(b)) => b.memo.clone(),
        _ => None,
    };
    let message = match (&a.message, &base) {
        (Some(m), _) => text(Some(m.clone()), 2000, "message")?,
        (None, Some(b)) => b.message.clone(),
        (None, None) if kind == "po" => {
            Some(ms::get_str(ctx, "purchase-orders.default_message")?).filter(|m| !m.is_empty())
        }
        _ => None,
    };
    let ship_to = match (&a.ship_to, &base) {
        (Some(s), _) => {
            if s.len() > 5 {
                return Err(invalid("ship to: at most 5 lines"));
            }
            let mut out = Vec::new();
            for l in s {
                if let Some(t) = text(Some(l.clone()), 100, "ship to")? {
                    out.push(t);
                }
            }
            out
        }
        (None, Some(b)) => b.ship_to.clone(),
        _ => Vec::new(),
    };
    Ok(BillDoc {
        id: base.as_ref().map_or_else(new_id, |b| b.id.clone()),
        kind: kind.to_owned(),
        number,
        vendor: v.id,
        date,
        terms,
        due_date,
        lines,
        computed,
        total,
        memo,
        message,
        ship_to,
        expected_date: a
            .expected_date
            .or(base.as_ref().and_then(|b| b.expected_date)),
        status: base
            .as_ref()
            .map_or_else(|| "open".into(), |b| b.status.clone()),
        txn: base.as_ref().and_then(|b| b.txn),
        po: base.as_ref().and_then(|b| b.po.clone()),
        applied: base.as_ref().map(|b| b.applied.clone()).unwrap_or_default(),
        created_at: base
            .as_ref()
            .map_or_else(now_rfc3339, |b| Ok(b.created_at.clone()))?,
        tags: crate::dimensions::doc_tags_p(
            ctx,
            a.class.as_deref(),
            a.location.as_deref(),
            a.fund.as_deref(),
            a.project.as_deref(),
            base.as_ref().map(|b| &b.tags),
        )?,
    })
}

/// Post a bill (AP credit) or vendor credit (AP debit).
fn post(ctx: &Ctx<'_>, d: &mut BillDoc) -> CommandResult<()> {
    let sign = if d.kind == "vendor_credit" {
        Decimal::NEGATIVE_ONE
    } else {
        Decimal::ONE
    };
    let ap = payable(ctx)?;
    let v = find_party(ctx, &d.vendor)?;
    let label = match &d.number {
        Some(n) => format!(
            "{} {n}",
            if d.kind == "bill" {
                "Bill"
            } else {
                "Vendor credit"
            }
        ),
        None => (if d.kind == "bill" {
            "Bill"
        } else {
            "Vendor credit"
        })
        .to_owned(),
    };
    let mut ap_split = cash(ap, mul(-d.total, sign)?, Some(label.clone()));
    ap_split.tags = d.tags.clone();
    ap_split.tags.project = None;
    ap_split.tags.billable = false;
    let mut splits = vec![ap_split];
    for l in &d.computed {
        let memo = Some(l.description.clone()).filter(|m| !m.is_empty());
        let mut sp = cash(l.account, mul(l.amount, sign)?, memo);
        sp.tags = crate::dimensions::merge(&l.tags, &d.tags);
        splits.push(sp);
    }
    let memo = d.memo.clone().or(Some(label));
    d.txn = Some(post_txn(ctx, d.txn, d.date, &v.name, memo, splits)?);
    Ok(())
}

fn doc_json(ctx: &Ctx<'_>, d: &BillDoc) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let b = Book::load(ctx)?;
    let v = find_party(ctx, &d.vendor).ok();
    let mut out = json!({
        "id": d.id, "kind": d.kind, "number": d.number, "vendor": d.vendor,
        "vendor_name": v.as_ref().map(|v| v.name.clone()),
        "date": d.date, "terms": d.terms, "terms_label": d.terms.as_ref().map(terms_label),
        "due_date": d.due_date, "lines": d.lines, "computed": lines_json(&chart, &d.computed),
        "total": money(d.total), "memo": d.memo, "message": d.message, "ship_to": d.ship_to,
        "expected_date": d.expected_date, "txn": d.txn, "po": d.po,
        "tag_names": crate::dimensions::tag_names(ctx, &d.tags)?,
    });
    let o = out
        .as_object_mut()
        .ok_or_else(|| CommandError::Integrity("json".into()))?;
    match d.kind.as_str() {
        "bill" => {
            let (applied, balance, status) = bill_state(d, &b, None);
            o.insert("applied".into(), json!(money(applied)));
            o.insert("balance".into(), json!(money(balance)));
            o.insert("status".into(), json!(status));
            let today = mk_store::today_utc()?;
            o.insert(
                "overdue".into(),
                json!(
                    matches!(status.as_str(), "open" | "partial")
                        && d.due_date.is_some_and(|x| today > x)
                ),
            );
            let mut apps = Vec::new();
            for p in b.payments.iter().filter(|p| !p.void) {
                for a in p.applied.iter().filter(|a| a.bill == d.id) {
                    apps.push(json!({"kind": "payment", "id": p.id, "date": a.date, "amount": money(a.amount), "check_number": p.check_number}));
                }
            }
            for c in b.credits.iter().filter(|c| c.status != "void") {
                for a in c.applied.iter().filter(|a| a.bill == d.id) {
                    apps.push(json!({"kind": "vendor_credit", "id": c.id, "number": c.number, "date": a.date, "amount": money(a.amount)}));
                }
            }
            o.insert("applications".into(), Value::Array(apps));
        }
        "vendor_credit" => {
            o.insert("status".into(), json!(d.status));
            o.insert("applied".into(), json!(d.applied));
            o.insert(
                "unapplied".into(),
                json!(money(b.unapplied_credit(d, None))),
            );
        }
        _ => {
            let received = po_received(ctx, d)?;
            o.insert("status".into(), json!(po_status(d, &received)));
            o.insert(
                "received".into(),
                json!(
                    received
                        .iter()
                        .map(|r| r.normalize().to_string())
                        .collect::<Vec<_>>()
                ),
            );
            let bills: Vec<Value> = bill_docs(ctx, "bill")?
                .iter()
                .filter(|x| x.po.as_deref() == Some(d.id.as_str()))
                .map(|x| json!({"id": x.id, "number": x.number, "date": x.date, "total": money(x.total), "status": x.status}))
                .collect();
            o.insert("bills".into(), Value::Array(bills));
        }
    }
    Ok(out)
}

fn bill_create(ctx: &Ctx<'_>, mut a: BillArgs) -> CommandResult<Value> {
    let occurrence = match a.from_schedule.take() {
        Some(f) => Some(crate::schedules::pending_bill_occurrence(
            ctx,
            &f.schedule,
            f.date,
        )?),
        None => None,
    };
    let mut d = build(ctx, "bill", a, None, true)?;
    post(ctx, &mut d)?;
    save_doc(ctx, &d)?;
    if let Some((schedule, nominal, due)) = occurrence {
        crate::schedules::mark_bill_entered(ctx, &schedule, nominal, due, d.txn, &d.id)?;
    }
    doc_json(ctx, &d)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BillUpdateArgs {
    /// Id (or number when unique).
    #[serde(alias = "vendor_credit", alias = "po")]
    bill: String,
    #[serde(flatten)]
    rest: BillArgs,
}

fn bill_update(ctx: &Ctx<'_>, a: BillUpdateArgs) -> CommandResult<Value> {
    let old = find_doc(ctx, "bill", &a.bill)?;
    if old.status == "void" {
        return Err(rejected("a void bill cannot change"));
    }
    require_open_period(ctx, old.date)?;
    let mut d = build(ctx, "bill", a.rest, Some(old), true)?;
    let b = Book::load(ctx)?;
    if b.applied_to(&d.id, None) > d.total {
        return Err(rejected(
            "the new total is less than the payments and credits applied; remove some first",
        ));
    }
    post(ctx, &mut d)?;
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BillRef {
    #[serde(alias = "vendor_credit", alias = "po")]
    bill: String,
}

fn void_txn(ctx: &Ctx<'_>, id: Option<TransactionId>, reason: String) -> CommandResult<()> {
    if let Some(id) = id
        && let Some(old) = ctx.tx.transaction(id)?
    {
        let mut new = old.clone();
        new.status = TxnStatus::Voided;
        new.void_reason = Some(reason);
        validate_change(ctx, Some(&old), Some(&new))?;
        ctx.save_txn(Some(&old), &new)?;
    }
    Ok(())
}

fn bill_void(ctx: &Ctx<'_>, a: BillRef) -> CommandResult<Value> {
    let mut d = find_doc(ctx, "bill", &a.bill)?;
    if d.status == "void" {
        return Err(rejected("the bill is already void"));
    }
    if Book::load(ctx)?.applied_to(&d.id, None) != Decimal::ZERO {
        return Err(rejected(
            "payments or credits are applied to this bill; remove them first",
        ));
    }
    void_txn(ctx, d.txn, "Bill voided".into())?;
    d.status = "void".into();
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

fn bill_delete(ctx: &Ctx<'_>, a: BillRef) -> CommandResult<Value> {
    let d = find_doc(ctx, "bill", &a.bill)?;
    if Book::load(ctx)?.applied_to(&d.id, None) != Decimal::ZERO {
        return Err(rejected(
            "payments or credits are applied; remove them first",
        ));
    }
    if let Some(t) = d.txn {
        crate::schedules::unlink_txn(ctx, t)?;
    }
    remove_txn(ctx, d.txn)?;
    ctx.save_doc(DocTable::BillDoc, &d.id, None)?;
    Ok(json!({"deleted": d.id}))
}

fn bill_get(ctx: &Ctx<'_>, a: BillRef) -> CommandResult<Value> {
    doc_json(ctx, &find_doc(ctx, "bill", &a.bill)?)
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BillListArgs {
    #[serde(default)]
    vendor: Option<String>,
    /// open, partial, paid, void (bills); open, partly_received, received, closed,
    /// cancelled (purchase orders).
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    from: Option<Date>,
    #[serde(default)]
    to: Option<Date>,
    /// Only bills due on or before this date (unpaid).
    #[serde(default)]
    due_before: Option<Date>,
    #[serde(default)]
    overdue: bool,
    /// Today for `overdue` (default: the clock).
    #[serde(default)]
    as_of: Option<Date>,
}

fn list(ctx: &Ctx<'_>, kind: &str, a: BillListArgs) -> CommandResult<Value> {
    let v = a
        .vendor
        .as_deref()
        .map(|r| find_party(ctx, r))
        .transpose()?;
    let today = a.as_of.map_or_else(mk_store::today_utc, Ok)?;
    let b = Book::load(ctx)?;
    let names: BTreeMap<String, String> =
        parties(ctx)?.into_iter().map(|p| (p.id, p.name)).collect();
    let mut out = Vec::new();
    for d in bill_docs(ctx, kind)?.iter().rev() {
        if v.as_ref().is_some_and(|v| v.id != d.vendor)
            || a.from.is_some_and(|f| d.date < f)
            || a.to.is_some_and(|t| d.date > t)
        {
            continue;
        }
        let (balance, status) = match kind {
            "bill" => {
                let (_, bal, s) = bill_state(d, &b, None);
                (Some(bal), s)
            }
            "vendor_credit" => (Some(b.unapplied_credit(d, None)), d.status.clone()),
            _ => (None, po_status(d, &po_received(ctx, d)?)),
        };
        if a.status.as_ref().is_some_and(|s| *s != status) {
            continue;
        }
        let unpaid = matches!(status.as_str(), "open" | "partial");
        let overdue = kind == "bill" && unpaid && d.due_date.is_some_and(|x| today > x);
        if a.overdue && !overdue {
            continue;
        }
        if let Some(due) = a.due_before
            && !(unpaid && d.due_date.is_some_and(|x| x <= due))
        {
            continue;
        }
        out.push(json!({
            "id": d.id, "number": d.number, "vendor": d.vendor,
            "vendor_name": names.get(&d.vendor), "date": d.date, "due_date": d.due_date,
            "expected_date": d.expected_date, "total": money(d.total),
            "balance": balance.map(money), "status": status, "overdue": overdue,
        }));
    }
    Ok(Value::Array(out))
}

fn bill_list(ctx: &Ctx<'_>, a: BillListArgs) -> CommandResult<Value> {
    list(ctx, "bill", a)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DuplicateArgs {
    bill: String,
    #[serde(default)]
    date: Option<Date>,
}

fn bill_duplicate(ctx: &Ctx<'_>, a: DuplicateArgs) -> CommandResult<Value> {
    let src = find_doc(ctx, "bill", &a.bill)?;
    let args = BillArgs {
        vendor: Some(src.vendor.clone()),
        date: Some(a.date.unwrap_or(src.date)),
        terms: src.terms.clone(),
        lines: Some(
            src.lines
                .iter()
                .cloned()
                .map(|mut l| {
                    l.po_line = None;
                    l
                })
                .collect(),
        ),
        memo: src.memo.clone(),
        class: src.tags.class.clone(),
        location: src.tags.location.clone(),
        fund: src.tags.fund.clone(),
        project: src.tags.project.clone(),
        ..BillArgs::default()
    };
    let mut d = build(ctx, "bill", args, None, true)?;
    post(ctx, &mut d)?;
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PreviewArgs {
    /// `bill` (default), `vendor_credit` or `po`.
    #[serde(default)]
    kind: Option<String>,
    /// Preview an edit of this document.
    #[serde(default)]
    bill: Option<String>,
    #[serde(flatten)]
    rest: BillArgs,
}

fn bill_preview(ctx: &Ctx<'_>, a: PreviewArgs) -> CommandResult<Value> {
    let kind = a.kind.unwrap_or_else(|| "bill".into());
    if !matches!(kind.as_str(), "bill" | "vendor_credit" | "po") {
        return Err(invalid("kind is bill, vendor_credit or po"));
    }
    let base = a
        .bill
        .as_deref()
        .map(|r| find_doc(ctx, &kind, r))
        .transpose()?;
    let d = build(ctx, &kind, a.rest, base, false)?;
    let chart = Chart::load(ctx)?;
    Ok(json!({
        "number": d.number, "due_date": d.due_date,
        "terms_label": d.terms.as_ref().map(terms_label),
        "lines": lines_json(&chart, &d.computed), "total": money(d.total),
    }))
}

// ----- payments -----------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApplyArg {
    /// Bill id (or number when unique).
    bill: String,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
}

/// A vendor's open bills, oldest due first, with balances.
fn open_bills(ctx: &Ctx<'_>, vendor: &str) -> CommandResult<Vec<(BillDoc, Decimal)>> {
    let b = Book::load(ctx)?;
    let mut v: Vec<(BillDoc, Decimal)> = bill_docs(ctx, "bill")?
        .into_iter()
        .filter(|d| d.vendor == vendor && d.status != "void")
        .map(|d| {
            let (_, bal, _) = bill_state(&d, &b, None);
            (d, bal)
        })
        .filter(|(_, bal)| *bal > Decimal::ZERO)
        .collect();
    v.sort_by_key(|(d, _)| (d.due_date.unwrap_or(d.date), d.date, d.created_at.clone()));
    Ok(v)
}

/// Check and resolve applications up to `available` (`None`: no limit).
fn applications(
    ctx: &Ctx<'_>,
    vendor: &str,
    given: Option<Vec<ApplyArg>>,
    available: Option<Decimal>,
    date: Date,
) -> CommandResult<Vec<BillApp>> {
    let open = open_bills(ctx, vendor)?;
    let mut out = Vec::new();
    match given {
        Some(list) => {
            let mut total = Decimal::ZERO;
            for g in list {
                if g.amount.is_zero() {
                    continue;
                }
                if g.amount.is_sign_negative() || decimals(g.amount) > 2 {
                    return Err(invalid(
                        "an applied amount is positive with at most 2 decimals",
                    ));
                }
                let (d, bal) = open
                    .iter()
                    .find(|(d, _)| {
                        d.id == g.bill
                            || d.number
                                .as_deref()
                                .is_some_and(|n| n.eq_ignore_ascii_case(g.bill.trim()))
                    })
                    .ok_or_else(|| {
                        invalid(format!("{} is not an open bill of this vendor", g.bill))
                    })?;
                if g.amount > *bal {
                    return Err(invalid(format!(
                        "{} is more than the bill's balance {}",
                        money(g.amount),
                        money(*bal)
                    )));
                }
                total = add(total, g.amount)?;
                out.push(BillApp {
                    bill: d.id.clone(),
                    amount: g.amount,
                    date: date.max(d.date),
                });
            }
            if available.is_some_and(|x| total > x) {
                return Err(invalid(format!(
                    "{} applied is more than the {} available",
                    money(total),
                    money(available.unwrap_or_default())
                )));
            }
        }
        None => {
            let mut left = available.unwrap_or_default();
            for (d, bal) in open {
                if left <= Decimal::ZERO {
                    break;
                }
                let take = bal.min(left);
                left = sub(left, take)?;
                out.push(BillApp {
                    bill: d.id.clone(),
                    amount: take,
                    date: date.max(d.date),
                });
            }
        }
    }
    Ok(out)
}

fn check_numbers(ctx: &Ctx<'_>) -> CommandResult<serde_json::Map<String, Value>> {
    Ok(ms::get(ctx, "bills.next_check_number")?
        .as_object()
        .cloned()
        .unwrap_or_default())
}

/// Check numbers already used on a bank account (including void checks).
fn used_checks(ctx: &Ctx<'_>, bank: AccountId) -> CommandResult<Vec<i64>> {
    Ok(bill_payments(ctx)?
        .iter()
        .filter(|p| p.from_account == bank)
        .filter_map(|p| p.check_number)
        .collect())
}

fn payment_json(ctx: &Ctx<'_>, p: &BillPayment) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let b = Book::load(ctx)?;
    let bills = bill_docs(ctx, "bill")?;
    let v = find_party(ctx, &p.vendor).ok();
    Ok(json!({
        "id": p.id, "vendor": p.vendor, "vendor_name": v.map(|v| v.name),
        "date": p.date, "amount": money(p.amount), "method": p.method,
        "from_account": p.from_account, "from_account_name": chart.full_name(p.from_account),
        "check_number": p.check_number, "reference": p.reference, "txn": p.txn,
        "applied": p.applied.iter().map(|a| json!({
            "bill": a.bill,
            "number": bills.iter().find(|d| d.id == a.bill).and_then(|d| d.number.clone()),
            "amount": money(a.amount), "date": a.date,
        })).collect::<Vec<_>>(),
        "unapplied": money(b.unapplied_payment(p, None)),
        "void": p.void, "memo": p.memo,
    }))
}

struct PayInput {
    vendor: Party,
    date: Date,
    from: AccountId,
    method: String,
    check_number: Option<i64>,
    reference: Option<String>,
    apply: Option<Vec<ApplyArg>>,
    amount: Option<Decimal>,
    memo: Option<String>,
    tags: SplitTags,
}

fn default_method(chart: &Chart, from: AccountId) -> String {
    match chart.get(from).map(|a| a.subtype) {
        Some(AccountSubtype::CreditCard) => "card".into(),
        Some(AccountSubtype::Bank) => "check".into(),
        Some(AccountSubtype::Cash) => "cash".into(),
        _ => "other".into(),
    }
}

fn pay_from(ctx: &Ctx<'_>, given: Option<&str>) -> CommandResult<AccountId> {
    let chart = Chart::load(ctx)?;
    let r = match given {
        Some(r) => r.to_owned(),
        None => ms::get(ctx, "bills.default_payment_account")?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| invalid("give the account to pay from"))?,
    };
    let a = chart.resolve(&r)?;
    if !matches!(
        a.subtype,
        AccountSubtype::Bank | AccountSubtype::Cash | AccountSubtype::CreditCard
    ) {
        return Err(invalid(format!(
            "{} is not a bank, cash or credit card account",
            chart.full_name(a.id)
        )));
    }
    Ok(a.id)
}

/// Record one bill payment.
fn pay(ctx: &Ctx<'_>, p: PayInput) -> CommandResult<BillPayment> {
    if !matches!(
        p.method.as_str(),
        "check" | "bank_transfer" | "card" | "cash" | "other"
    ) {
        return Err(invalid(
            "method is check, bank_transfer, card, cash or other",
        ));
    }
    let chart = Chart::load(ctx)?;
    let from_acc = chart
        .get(p.from)
        .ok_or_else(|| CommandError::Integrity("account".into()))?;
    let apps = applications(ctx, &p.vendor.id, p.apply, p.amount, p.date)?;
    let applied = apps
        .iter()
        .try_fold(Decimal::ZERO, |s, a| add(s, a.amount))?;
    let amount = p.amount.unwrap_or(applied);
    if amount <= Decimal::ZERO || decimals(amount) > 2 {
        return Err(invalid("the amount is positive with at most 2 decimals"));
    }
    if applied > amount {
        return Err(invalid("more is applied than paid"));
    }
    let mut check_number = None;
    if p.method == "check" {
        if !matches!(
            from_acc.subtype,
            AccountSubtype::Bank | AccountSubtype::Cash
        ) {
            return Err(invalid("checks are written from a bank account"));
        }
        let mut map = check_numbers(ctx)?;
        let key = p.from.to_string();
        let next = map.get(&key).and_then(Value::as_i64);
        let n = p.check_number.or(next).ok_or_else(|| {
            invalid(format!(
                "give the first check number for {}",
                chart.full_name(p.from)
            ))
        })?;
        if n < 1 {
            return Err(invalid("check numbers start at 1"));
        }
        if used_checks(ctx, p.from)?.contains(&n) {
            return Err(rejected(format!(
                "check {n} is already used on {}",
                chart.full_name(p.from)
            )));
        }
        if next.is_none_or(|x| n >= x) {
            map.insert(key, json!(n.saturating_add(1)));
            ms::put(ctx, "bills.next_check_number", Value::Object(map))?;
        }
        check_number = Some(n);
    }
    let ap = payable(ctx)?;
    let memo = p.memo.clone().or_else(|| {
        Some(match check_number {
            Some(n) => format!("Bill payment, check {n}"),
            None => "Bill payment".to_owned(),
        })
    });
    let txn = post_txn_numbered(
        ctx,
        None,
        p.date,
        &p.vendor.name,
        memo,
        {
            let mut v = vec![cash(ap, amount, None), cash(p.from, -amount, None)];
            for sp in &mut v {
                sp.tags = p.tags.clone();
            }
            v
        },
        check_number.map(|n| n.to_string()),
    )?;
    let out = BillPayment {
        id: new_id(),
        vendor: p.vendor.id.clone(),
        date: p.date,
        amount,
        method: p.method,
        from_account: p.from,
        check_number,
        reference: p.reference,
        txn,
        applied: apps,
        void: false,
        memo: p.memo,
    };
    save_payment(ctx, &out)?;
    Ok(out)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PayArgs {
    vendor: String,
    date: Date,
    /// Bank, cash or credit card account (default: setting bills.default_payment_account).
    #[serde(default)]
    from_account: Option<String>,
    /// check, bank_transfer, card, cash or other (default from the account).
    #[serde(default)]
    method: Option<String>,
    /// Checks: the number (default: the account's next check number).
    #[serde(default)]
    check_number: Option<i64>,
    #[serde(default)]
    reference: Option<String>,
    /// Applications; default: oldest due bills first.
    #[serde(default)]
    apply: Option<Vec<ApplyArg>>,
    /// Default: the sum applied.
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    amount: Option<Decimal>,
    #[serde(default)]
    memo: Option<String>,
    /// Class, location and fund for the payment's splits.
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    fund: Option<String>,
}

fn bill_pay(ctx: &Ctx<'_>, a: PayArgs) -> CommandResult<Value> {
    let v = vendor(ctx, &a.vendor)?;
    if a.apply.is_none() && a.amount.is_none() {
        return Err(invalid("give the amount or the bills to pay"));
    }
    let from = pay_from(ctx, a.from_account.as_deref())?;
    let chart = Chart::load(ctx)?;
    let method = a.method.unwrap_or_else(|| default_method(&chart, from));
    let p = pay(
        ctx,
        PayInput {
            vendor: v,
            date: a.date,
            from,
            method,
            check_number: a.check_number,
            reference: text(a.reference, 40, "reference")?,
            apply: a.apply,
            amount: a.amount,
            memo: text(a.memo, 1000, "memo")?,
            tags: crate::dimensions::resolve_tags(
                ctx,
                a.class.as_deref(),
                a.location.as_deref(),
                a.fund.as_deref(),
            )?,
        },
    )?;
    payment_json(ctx, &p)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ManyBill {
    bill: String,
    /// Default: the bill's balance.
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    amount: Option<Decimal>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PayManyArgs {
    date: Date,
    #[serde(default)]
    from_account: Option<String>,
    #[serde(default)]
    method: Option<String>,
    /// Checks: the first number (default: the account's next check number).
    #[serde(default)]
    first_check_number: Option<i64>,
    bills: Vec<ManyBill>,
}

fn bill_pay_many(ctx: &Ctx<'_>, a: PayManyArgs) -> CommandResult<Value> {
    if a.bills.is_empty() {
        return Err(invalid("choose the bills to pay"));
    }
    let from = pay_from(ctx, a.from_account.as_deref())?;
    let chart = Chart::load(ctx)?;
    let method = a.method.unwrap_or_else(|| default_method(&chart, from));
    let b = Book::load(ctx)?;
    // vendor name → (vendor, applications)
    let mut by: BTreeMap<String, (Party, Vec<ApplyArg>)> = BTreeMap::new();
    for x in a.bills {
        let d = find_doc(ctx, "bill", &x.bill)?;
        let (_, bal, status) = bill_state(&d, &b, None);
        if !matches!(status.as_str(), "open" | "partial") {
            return Err(rejected(format!(
                "bill {} is {status}",
                d.number.clone().unwrap_or_else(|| d.id.clone())
            )));
        }
        let v = find_party(ctx, &d.vendor)?;
        let e = by
            .entry(format!("{}\u{0}{}", v.name.to_lowercase(), v.id))
            .or_insert_with(|| (v, Vec::new()));
        e.1.push(ApplyArg {
            bill: d.id.clone(),
            amount: x.amount.unwrap_or(bal),
        });
    }
    let mut next = a.first_check_number;
    let mut out = Vec::new();
    for (_, (v, apply)) in by {
        let p = pay(
            ctx,
            PayInput {
                vendor: v,
                date: a.date,
                from,
                method: method.clone(),
                check_number: next,
                reference: None,
                apply: Some(apply),
                amount: None,
                memo: None,
                tags: SplitTags::default(),
            },
        )?;
        next = p.check_number.map(|n| n.saturating_add(1));
        out.push(payment_json(ctx, &p)?);
    }
    Ok(json!({"payments": out}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PaymentApplyArgs {
    payment: String,
    #[serde(default)]
    apply: Option<Vec<ApplyArg>>,
    #[serde(default)]
    date: Option<Date>,
}

fn payment_apply(ctx: &Ctx<'_>, a: PaymentApplyArgs) -> CommandResult<Value> {
    let mut p = find_payment(ctx, &a.payment)?;
    if p.void {
        return Err(rejected("the payment is void"));
    }
    let available = Book::load(ctx)?.unapplied_payment(&p, None);
    let more = applications(
        ctx,
        &p.vendor,
        a.apply,
        Some(available),
        a.date.unwrap_or(p.date),
    )?;
    p.applied.extend(more);
    save_payment(ctx, &p)?;
    payment_json(ctx, &p)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UnapplyArgs {
    #[serde(alias = "vendor_credit")]
    payment: String,
    bill: String,
}

fn payment_unapply(ctx: &Ctx<'_>, a: UnapplyArgs) -> CommandResult<Value> {
    let mut p = find_payment(ctx, &a.payment)?;
    let d = find_doc(ctx, "bill", &a.bill)?;
    let before = p.applied.len();
    p.applied.retain(|x| x.bill != d.id);
    if p.applied.len() == before {
        return Err(invalid("the payment is not applied to that bill"));
    }
    save_payment(ctx, &p)?;
    payment_json(ctx, &p)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PaymentRef {
    payment: String,
}

fn payment_void(ctx: &Ctx<'_>, a: PaymentRef) -> CommandResult<Value> {
    let mut p = find_payment(ctx, &a.payment)?;
    if p.void {
        return Err(rejected("the payment is already void"));
    }
    if Book::load(ctx)?.refunded_from(&p.id, None) > Decimal::ZERO {
        return Err(rejected("part of this payment was refunded"));
    }
    let reason = match p.check_number {
        Some(n) => format!("Check {n} voided"),
        None => "Bill payment voided".to_owned(),
    };
    void_txn(ctx, Some(p.txn), reason)?;
    p.void = true;
    p.applied.clear();
    save_payment(ctx, &p)?;
    payment_json(ctx, &p)
}

fn payment_get(ctx: &Ctx<'_>, a: PaymentRef) -> CommandResult<Value> {
    payment_json(ctx, &find_payment(ctx, &a.payment)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PaymentListArgs {
    #[serde(default)]
    vendor: Option<String>,
}

fn payment_list(ctx: &Ctx<'_>, a: PaymentListArgs) -> CommandResult<Value> {
    let v = a
        .vendor
        .as_deref()
        .map(|r| find_party(ctx, r))
        .transpose()?;
    let mut out = Vec::new();
    for p in bill_payments(ctx)?.iter().rev() {
        if v.as_ref().is_some_and(|v| v.id != p.vendor) {
            continue;
        }
        out.push(payment_json(ctx, p)?);
    }
    Ok(Value::Array(out))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NextCheckArgs {
    account: String,
}

fn next_check(ctx: &Ctx<'_>, a: NextCheckArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let acc = chart.resolve(&a.account)?;
    let n = check_numbers(ctx)?
        .get(&acc.id.to_string())
        .and_then(Value::as_i64);
    Ok(json!({"account": acc.id, "next": n}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OpenArgs {
    vendor: String,
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    amount: Option<Decimal>,
}

fn bill_open(ctx: &Ctx<'_>, a: OpenArgs) -> CommandResult<Value> {
    let v = vendor(ctx, &a.vendor)?;
    let date = mk_store::today_utc()?;
    let suggested = match a.amount {
        Some(x) => applications(ctx, &v.id, None, Some(x.max(Decimal::ZERO)), date)?,
        None => Vec::new(),
    };
    let rows: Vec<Value> = open_bills(ctx, &v.id)?
        .iter()
        .map(|(d, bal)| {
            let apply = suggested
                .iter()
                .find(|x| x.bill == d.id)
                .map_or(Decimal::ZERO, |x| x.amount);
            json!({"id": d.id, "number": d.number, "date": d.date, "due_date": d.due_date,
                   "total": money(d.total), "balance": money(*bal), "apply": money(apply)})
        })
        .collect();
    let b = Book::load(ctx)?;
    let credit = b
        .credits_of(&v.id, None)
        .iter()
        .try_fold(Decimal::ZERO, |s, x| add(s, x.2))?;
    Ok(json!({"vendor": v.id, "bills": rows, "credit": money(credit)}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AttentionArgs {
    #[serde(default)]
    today: Option<Date>,
}

fn bill_attention(ctx: &Ctx<'_>, a: AttentionArgs) -> CommandResult<Value> {
    let today = a.today.map_or_else(mk_store::today_utc, Ok)?;
    let soon = today.add_days(7).unwrap_or(today);
    let b = Book::load(ctx)?;
    let (mut overdue, mut due_soon) = (0_usize, 0_usize);
    for d in bill_docs(ctx, "bill")? {
        let (_, _, s) = bill_state(&d, &b, None);
        if !matches!(s.as_str(), "open" | "partial") {
            continue;
        }
        match d.due_date {
            Some(x) if x < today => overdue += 1,
            Some(x) if x <= soon => due_soon += 1,
            _ => {}
        }
    }
    Ok(json!({"overdue": overdue, "due_next_7_days": due_soon}))
}

// ----- vendor credits and refunds -----------------------------------------------------

fn credit_create(ctx: &Ctx<'_>, a: BillArgs) -> CommandResult<Value> {
    let mut d = build(ctx, "vendor_credit", a, None, true)?;
    post(ctx, &mut d)?;
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreditApplyArgs {
    vendor_credit: String,
    #[serde(default)]
    apply: Option<Vec<ApplyArg>>,
    #[serde(default)]
    date: Option<Date>,
}

fn credit_apply(ctx: &Ctx<'_>, a: CreditApplyArgs) -> CommandResult<Value> {
    let mut c = find_doc(ctx, "vendor_credit", &a.vendor_credit)?;
    if c.status == "void" {
        return Err(rejected("the credit is void"));
    }
    let available = Book::load(ctx)?.unapplied_credit(&c, None);
    let more = applications(
        ctx,
        &c.vendor,
        a.apply,
        Some(available),
        a.date.unwrap_or(c.date),
    )?;
    c.applied.extend(more);
    save_doc(ctx, &c)?;
    doc_json(ctx, &c)
}

fn credit_unapply(ctx: &Ctx<'_>, a: UnapplyArgs) -> CommandResult<Value> {
    let mut c = find_doc(ctx, "vendor_credit", &a.payment)?;
    let d = find_doc(ctx, "bill", &a.bill)?;
    let before = c.applied.len();
    c.applied.retain(|x| x.bill != d.id);
    if c.applied.len() == before {
        return Err(invalid("the credit is not applied to that bill"));
    }
    save_doc(ctx, &c)?;
    doc_json(ctx, &c)
}

fn credit_delete(ctx: &Ctx<'_>, a: BillRef) -> CommandResult<Value> {
    let c = find_doc(ctx, "vendor_credit", &a.bill)?;
    if !c.applied.is_empty() || Book::load(ctx)?.refunded_from(&c.id, None) > Decimal::ZERO {
        return Err(rejected(
            "the credit is applied or refunded; remove that first",
        ));
    }
    remove_txn(ctx, c.txn)?;
    ctx.save_doc(DocTable::BillDoc, &c.id, None)?;
    Ok(json!({"deleted": c.id}))
}

fn credit_get(ctx: &Ctx<'_>, a: BillRef) -> CommandResult<Value> {
    doc_json(ctx, &find_doc(ctx, "vendor_credit", &a.bill)?)
}

fn credit_list(ctx: &Ctx<'_>, a: BillListArgs) -> CommandResult<Value> {
    list(ctx, "vendor_credit", a)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RefundArgs {
    vendor: String,
    to_account: String,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
    date: Date,
    #[serde(default)]
    memo: Option<String>,
}

fn refund_received(ctx: &Ctx<'_>, a: RefundArgs) -> CommandResult<Value> {
    let v = vendor(ctx, &a.vendor)?;
    if a.amount <= Decimal::ZERO || decimals(a.amount) > 2 {
        return Err(invalid("the amount is positive with at most 2 decimals"));
    }
    let to = Chart::load(ctx)?.resolve(&a.to_account)?.id;
    let credits = Book::load(ctx)?.credits_of(&v.id, None);
    let available = credits.iter().try_fold(Decimal::ZERO, |s, x| add(s, x.2))?;
    if a.amount > available {
        return Err(rejected(format!(
            "{} has {} of credit and prepayments; the refund is {}",
            v.name,
            money(available),
            money(a.amount)
        )));
    }
    let mut left = a.amount;
    let mut sources = Vec::new();
    for (id, _, amt) in credits {
        if left.is_zero() {
            break;
        }
        let take = amt.min(left);
        left = sub(left, take)?;
        sources.push(RefundSource {
            source: id,
            amount: take,
        });
    }
    let ap = payable(ctx)?;
    let txn = post_txn(
        ctx,
        None,
        a.date,
        &v.name,
        text(a.memo, 1000, "memo")?.or(Some("Vendor refund".into())),
        vec![cash(to, a.amount, None), cash(ap, -a.amount, None)],
    )?;
    let r = RefundReceived {
        id: new_id(),
        vendor: v.id,
        date: a.date,
        amount: a.amount,
        to_account: to,
        txn,
        sources,
    };
    save(
        ctx,
        DocTable::BillPayment,
        &r.id,
        Some("refund"),
        date_ord(r.date),
        &r,
    )?;
    Ok(json!({"id": r.id, "txn": txn, "amount": money(r.amount)}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PartyRef {
    party: String,
}

fn vendor_summary(ctx: &Ctx<'_>, a: PartyRef) -> CommandResult<Value> {
    let v = find_party(ctx, &a.party)?;
    let b = Book::load(ctx)?;
    let today = mk_store::today_utc()?;
    let mut open = Vec::new();
    let (mut balance, mut overdue) = (Decimal::ZERO, Decimal::ZERO);
    for d in bill_docs(ctx, "bill")?.iter().filter(|d| d.vendor == v.id) {
        let (_, bal, s) = bill_state(d, &b, None);
        if matches!(s.as_str(), "open" | "partial") {
            balance = add(balance, bal)?;
            if d.due_date.is_some_and(|x| today > x) {
                overdue = add(overdue, bal)?;
            }
            open.push(
                json!({"id": d.id, "number": d.number, "date": d.date, "due_date": d.due_date,
                "total": money(d.total), "balance": money(bal), "status": s}),
            );
        }
    }
    let credit = b
        .credits_of(&v.id, None)
        .iter()
        .try_fold(Decimal::ZERO, |s, x| add(s, x.2))?;
    let pays = b
        .payments
        .iter()
        .filter(|p| p.vendor == v.id)
        .map(|p| payment_json(ctx, p))
        .collect::<CommandResult<Vec<_>>>()?;
    let tax: Vec<crate::sales::TaxPayment> = load(ctx, DocTable::Payment, Some("tax_payment"))?;
    let tax_payments: Vec<Value> = tax
        .iter()
        .filter(|t| t.vendor.as_deref() == Some(v.id.as_str()))
        .map(|t| {
            let total = t
                .amounts
                .iter()
                .fold(Decimal::ZERO, |s, x| s.checked_add(x.amount).unwrap_or(s));
            json!({"id": t.id, "date": t.date, "amount": money(total), "txn": t.txn})
        })
        .collect();
    Ok(json!({
        "vendor": crate::sales::party_json(ctx, &v)?,
        "open_bills": open, "balance": money(balance), "overdue": money(overdue),
        "credit": money(credit), "payments": pays, "tax_payments": tax_payments,
    }))
}

// ----- transaction vendor links -------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetVendorArgs {
    txn: TransactionId,
    /// Vendor id or name; null or "" removes the link.
    #[serde(default)]
    vendor: Option<String>,
}

fn set_vendor(ctx: &Ctx<'_>, a: SetVendorArgs) -> CommandResult<Value> {
    if ctx.tx.transaction(a.txn)?.is_none() {
        return Err(CommandError::NotFound(format!("transaction {}", a.txn)));
    }
    let id = a.txn.to_string();
    match a.vendor.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        Some(r) => {
            let v = vendor(ctx, r)?;
            save(
                ctx,
                DocTable::TxnLink,
                &id,
                None,
                0,
                &json!({"vendor": v.id}),
            )?;
            Ok(json!({"txn": id, "vendor": v.id, "vendor_name": v.name}))
        }
        None => {
            ctx.save_doc(DocTable::TxnLink, &id, None)?;
            Ok(json!({"txn": id, "vendor": null}))
        }
    }
}

/// Transaction id → vendor id.
pub(crate) fn links(ctx: &Ctx<'_>) -> CommandResult<BTreeMap<String, String>> {
    Ok(ctx
        .tx
        .docs(DocTable::TxnLink, None)?
        .into_iter()
        .filter_map(|d| {
            d.json
                .get("vendor")
                .and_then(Value::as_str)
                .map(|v| (d.id.clone(), v.to_owned()))
        })
        .collect())
}

fn vendor_links(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let names: BTreeMap<String, String> =
        parties(ctx)?.into_iter().map(|p| (p.id, p.name)).collect();
    let mut out = serde_json::Map::new();
    for (t, v) in links(ctx)? {
        out.insert(t, json!({"vendor": v, "vendor_name": names.get(&v)}));
    }
    Ok(Value::Object(out))
}

// ----- purchase orders ----------------------------------------------------------------

/// Received quantity (item lines) or amount (expense lines) per PO line.
pub(crate) fn po_received(ctx: &Ctx<'_>, po: &BillDoc) -> CommandResult<Vec<Decimal>> {
    let mut out = vec![Decimal::ZERO; po.computed.len()];
    for b in bill_docs(ctx, "bill")?
        .iter()
        .filter(|b| b.po.as_deref() == Some(po.id.as_str()) && b.status != "void")
    {
        for l in &b.computed {
            let Some(n) = l.po_line else { continue };
            let Some(slot) = n.checked_sub(1).and_then(|i| out.get_mut(i)) else {
                continue;
            };
            let ordered = po.computed.get(n.saturating_sub(1));
            let by_qty = ordered.is_some_and(|o| o.item.is_some());
            let v = if by_qty {
                l.quantity.unwrap_or_default()
            } else {
                l.amount
            };
            *slot = slot.checked_add(v).unwrap_or(*slot);
        }
    }
    Ok(out)
}

fn ordered_of(l: &BillLine) -> Decimal {
    if l.item.is_some() {
        l.quantity.unwrap_or_default()
    } else {
        l.amount
    }
}

pub(crate) fn po_status(po: &BillDoc, received: &[Decimal]) -> String {
    if matches!(po.status.as_str(), "closed" | "cancelled") {
        return po.status.clone();
    }
    let all = po
        .computed
        .iter()
        .zip(received)
        .all(|(l, r)| *r >= ordered_of(l));
    if all && !po.computed.is_empty() {
        "received".into()
    } else if received.iter().any(|r| *r > Decimal::ZERO) {
        "partly_received".into()
    } else {
        "open".into()
    }
}

fn po_create(ctx: &Ctx<'_>, a: BillArgs) -> CommandResult<Value> {
    let d = build(ctx, "po", a, None, true)?;
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

fn has_receipts(ctx: &Ctx<'_>, po: &BillDoc) -> CommandResult<bool> {
    Ok(bill_docs(ctx, "bill")?
        .iter()
        .any(|b| b.po.as_deref() == Some(po.id.as_str())))
}

fn po_update(ctx: &Ctx<'_>, a: BillUpdateArgs) -> CommandResult<Value> {
    let old = find_doc(ctx, "po", &a.bill)?;
    if a.rest.lines.is_some() && has_receipts(ctx, &old)? {
        let keep = a.rest.lines.as_ref().is_some_and(|l| {
            l.len() >= old.lines.len()
                && l.iter()
                    .zip(&old.lines)
                    .all(|(n, o)| n.item == o.item && n.account == o.account)
        });
        if !keep {
            return Err(rejected(
                "lines were received; you can change quantities and add lines, not replace them",
            ));
        }
    }
    let d = build(ctx, "po", a.rest, Some(old), true)?;
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PoStatusArgs {
    po: String,
    /// open, closed or cancelled.
    status: String,
}

fn po_set_status(ctx: &Ctx<'_>, a: PoStatusArgs) -> CommandResult<Value> {
    let mut d = find_doc(ctx, "po", &a.po)?;
    if !matches!(a.status.as_str(), "open" | "closed" | "cancelled") {
        return Err(invalid("status is open, closed or cancelled"));
    }
    d.status = a.status;
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReceiveLine {
    /// 1-based line of the purchase order.
    po_line: usize,
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    quantity: Option<Decimal>,
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    amount: Option<Decimal>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReceiveArgs {
    po: String,
    date: Date,
    /// Default: everything not yet received.
    #[serde(default)]
    lines: Option<Vec<ReceiveLine>>,
    /// The vendor's invoice number for the bill.
    #[serde(default)]
    bill_number: Option<String>,
}

fn po_receive(ctx: &Ctx<'_>, a: ReceiveArgs) -> CommandResult<Value> {
    let po = find_doc(ctx, "po", &a.po)?;
    if matches!(po.status.as_str(), "closed" | "cancelled") {
        return Err(rejected(format!("the purchase order is {}", po.status)));
    }
    let received = po_received(ctx, &po)?;
    let wanted: Vec<(usize, Option<Decimal>, Option<Decimal>)> = match a.lines {
        Some(l) => l
            .into_iter()
            .map(|x| (x.po_line, x.quantity, x.amount))
            .collect(),
        None => po
            .computed
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                let left = ordered_of(l).checked_sub(*received.get(i)?)?;
                (left > Decimal::ZERO).then(|| {
                    if l.item.is_some() {
                        (i + 1, Some(left), None)
                    } else {
                        (i + 1, None, Some(left))
                    }
                })
            })
            .collect(),
    };
    let mut lines = Vec::new();
    let mut warnings = Vec::new();
    for (n, q, amt) in wanted {
        let src = po
            .lines
            .get(n.wrapping_sub(1))
            .ok_or_else(|| invalid(format!("the purchase order has no line {n}")))?;
        let comp = po
            .computed
            .get(n.wrapping_sub(1))
            .ok_or_else(|| invalid(format!("the purchase order has no line {n}")))?;
        let mut l = src.clone();
        l.po_line = Some(n);
        let got = if comp.item.is_some() {
            let q = q.ok_or_else(|| invalid(format!("line {n}: give the quantity received")))?;
            if q <= Decimal::ZERO {
                return Err(invalid(format!("line {n}: the quantity is more than zero")));
            }
            l.quantity = Some(q);
            l.cost = comp.cost;
            l.amount = None;
            q
        } else {
            let amount = amt
                .or(q)
                .ok_or_else(|| invalid(format!("line {n}: give the amount received")))?;
            l.amount = Some(round2(amount));
            l.quantity = None;
            l.cost = None;
            amount
        };
        let before = received.get(n - 1).copied().unwrap_or_default();
        if add(before, got)? > ordered_of(comp) {
            warnings.push(format!("line {n} is received beyond what was ordered"));
        }
        lines.push(l);
    }
    if lines.is_empty() {
        return Err(rejected("nothing is left to receive"));
    }
    let args = BillArgs {
        vendor: Some(po.vendor.clone()),
        number: a.bill_number,
        date: Some(a.date),
        lines: Some(lines),
        memo: po.number.as_ref().map(|n| format!("Received on {n}")),
        class: po.tags.class.clone(),
        location: po.tags.location.clone(),
        fund: po.tags.fund.clone(),
        project: po.tags.project.clone(),
        ..BillArgs::default()
    };
    let mut bill = build(ctx, "bill", args, None, true)?;
    bill.po = Some(po.id.clone());
    post(ctx, &mut bill)?;
    save_doc(ctx, &bill)?;
    Ok(json!({"bill": doc_json(ctx, &bill)?, "po": doc_json(ctx, &po)?, "warnings": warnings}))
}

fn po_get(ctx: &Ctx<'_>, a: BillRef) -> CommandResult<Value> {
    doc_json(ctx, &find_doc(ctx, "po", &a.bill)?)
}

fn po_list(ctx: &Ctx<'_>, a: BillListArgs) -> CommandResult<Value> {
    list(ctx, "po", a)
}

fn po_delete(ctx: &Ctx<'_>, a: BillRef) -> CommandResult<Value> {
    let d = find_doc(ctx, "po", &a.bill)?;
    if has_receipts(ctx, &d)? {
        return Err(rejected("lines were received; cancel or close it instead"));
    }
    ctx.save_doc(DocTable::BillDoc, &d.id, None)?;
    Ok(json!({"deleted": d.id}))
}

fn po_pdf(ctx: &Ctx<'_>, a: BillRef) -> CommandResult<Value> {
    let d = find_doc(ctx, "po", &a.bill)?;
    let v = find_party(ctx, &d.vendor)?;
    let mut message = Vec::new();
    if !d.ship_to.is_empty() {
        message.push(format!("Ship to: {}", d.ship_to.join(", ")));
    }
    if let Some(n) = &v.account_number_at_vendor {
        message.push(format!("Our account number: {n}"));
    }
    if let Some(m) = &d.message {
        message.push(m.clone());
    }
    let paper = ms::get_str(ctx, "invoicing.paper").unwrap_or_default();
    let tpl = crate::templates::for_document(ctx, None)?;
    let number = d.number.clone().unwrap_or_default();
    let doc = crate::pdf::SalesPdf {
        title: crate::templates::title(tpl.as_ref(), "purchase_order", "Purchase order"),
        style: crate::templates::style(tpl.as_ref()),
        business_name: ms::get_str(ctx, "invoicing.business_name").unwrap_or_default(),
        business_lines: ms::get_str(ctx, "invoicing.business_address")
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .filter(|s| !s.trim().is_empty())
            .collect(),
        number: number.clone(),
        date: d.date.to_string(),
        second_date: d
            .expected_date
            .map(|e| ("Expected".to_owned(), e.to_string())),
        terms: None,
        bill_to: std::iter::once(v.name.clone())
            .chain(v.company.clone())
            .chain(v.billing_address.iter().cloned())
            .collect(),
        lines: d
            .computed
            .iter()
            .map(|x| crate::pdf::PdfLine {
                description: x.description.clone(),
                quantity: x
                    .quantity
                    .map(|q| q.normalize().to_string())
                    .unwrap_or_default(),
                rate: x.cost.map(crate::sales::fmt_rate).unwrap_or_default(),
                amount: group(&money(x.amount)),
                emphasis: false,
            })
            .collect(),
        subtotal: group(&money(d.total)),
        taxes: Vec::new(),
        total: group(&money(d.total)),
        applied: None,
        balance: None,
        exemption: None,
        message: message.join("\n"),
        a4: paper == "a4",
        party_heading: "Vendor",
        ..crate::pdf::SalesPdf::default()
    };
    let (bytes, replaced) = crate::pdf::render(&doc);
    let mut warnings = Vec::new();
    if !replaced.is_empty() {
        warnings.push(format!(
            "these characters cannot be printed with the standard PDF fonts and were replaced with '?': {}",
            replaced.iter().collect::<String>()
        ));
    }
    use base64::Engine;
    Ok(json!({
        "file_name": format!("{}.pdf", number.replace(['/', '\\', ':'], "-")),
        "content_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
        "warnings": warnings,
    }))
}
