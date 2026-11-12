//! Expenses and receipts (expenses.md): receipts stored in the book, expenses that
//! post as ordinary transactions or match existing ones, reimbursements of spending
//! from personal funds, and the PDF expense report.
//!
//! Receipt files live in their own content-addressed table and are written once;
//! the receipt and expense records are documents, so every change is audited and
//! undone like any other record without copying file bytes into the audit log.

use std::collections::{BTreeMap, HashMap, HashSet};

use base64::Engine;
use mk_core::{
    AccountId, AccountSubtype, AccountType, Date, Decimal, Reconcile, Split, SplitTags,
    Transaction, TransactionId, TxnStatus,
};
use mk_store::{DocTable, now_rfc3339, today_utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::accounts::Chart;
use crate::module_settings::{self as ms};
use crate::sales::{date_ord, invalid, money, new_id, rejected, require_open_period};
use crate::txns::validate_change;
use crate::{CommandError, CommandResult, Ctx, Registry, is_module_enabled};

pub(crate) const MODULE: &str = "expenses";
/// `paid_with` of spending from the owner's own money.
pub(crate) const PERSONAL: &str = "personal";
pub(crate) const PERSONAL_LABEL: &str = "Personal funds (reimbursable)";
/// Most receipts one expense can carry.
const MAX_RECEIPTS: usize = 10;
const MAX_MERCHANT: usize = 100;
const MAX_PURPOSE: usize = 500;
const INBOX: &str = "inbox";

pub(crate) fn register(r: &mut Registry) {
    r.add_in(
        MODULE,
        "expense.create",
        "Record an expense (date, merchant, amount, category, paid with, purpose, tags); incomplete ones are drafts",
        true,
        expense_create,
    );
    r.add_in(
        MODULE,
        "expense.update",
        "Change an expense; a posted or matched one changes its transaction too",
        true,
        expense_update,
    );
    r.add_in(
        MODULE,
        "expense.delete",
        "Delete an expense; a posted one asks whether its transaction goes too, a matched one is only unlinked",
        true,
        expense_delete,
    );
    r.add_in(
        MODULE,
        "expense.get",
        "One expense with its receipts",
        false,
        expense_get,
    );
    r.add_in(
        MODULE,
        "expense.list",
        "Expenses, newest first, filtered by period, status, paid with, category or text",
        false,
        expense_list,
    );
    r.add_in(
        MODULE,
        "expense.post",
        "Post a ready expense: match an existing transaction of the same amount, or create a new one",
        true,
        expense_post,
    );
    r.add_in(
        MODULE,
        "expense.match_candidates",
        "Transactions an expense could match (same amount out of its paid-with account, near its date)",
        false,
        expense_match_candidates,
    );
    r.add_in(
        MODULE,
        "expense.record_reimbursement",
        "Record paying yourself back for posted reimbursable expenses: one transfer for their total",
        true,
        expense_record_reimbursement,
    );
    r.add_in(
        MODULE,
        "expense.report",
        "The expense report PDF for a period (optionally with receipt pictures); changes nothing",
        false,
        expense_report,
    );
    r.add_in(
        MODULE,
        "expense.mark_reported",
        "Mark expenses as reported (after their report was saved); one undo step",
        true,
        expense_mark_reported,
    );
    r.add_in(
        MODULE,
        "receipt.add",
        "Store a receipt (PNG, JPEG or PDF up to 10 MB); pictures are turned upright, scaled and stripped of metadata",
        true,
        receipt_add,
    );
    r.add_in(
        MODULE,
        "receipt.get",
        "A receipt's stored file",
        false,
        receipt_get,
    );
    r.add_in(
        MODULE,
        "receipt.thumbnail",
        "A receipt picture as a small PNG (256 pixels unless a size is given)",
        false,
        receipt_thumbnail,
    );
    r.add_in(
        MODULE,
        "receipt.link",
        "Attach a receipt to an expense or a transaction",
        true,
        receipt_link,
    );
    r.add_in(
        MODULE,
        "receipt.unlink",
        "Detach a receipt; it goes back to the inbox",
        true,
        receipt_unlink,
    );
    r.add_in(
        MODULE,
        "receipt.delete",
        "Delete a receipt",
        true,
        receipt_delete,
    );
    r.add_in(
        MODULE,
        "receipt.inbox",
        "Receipts not attached to anything, newest first",
        false,
        receipt_inbox,
    );
    r.add_in(
        MODULE,
        "receipt.list",
        "The receipts of an expense or a transaction",
        false,
        receipt_list,
    );
    r.add_in(
        MODULE,
        "receipt.find_transactions",
        "Transactions to attach a receipt to, searched by amount, date and payee",
        false,
        receipt_find_transactions,
    );
}

// ----- records ----------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReceiptDoc {
    pub id: String,
    /// `jpeg`, `png` or `pdf`.
    pub kind: String,
    /// Size of the stored file.
    pub bytes: usize,
    /// SHA-256 of the file as it was added (duplicate detection).
    pub sha256: String,
    /// The stored file's id (SHA-256 of the stored content).
    pub file: String,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    pub added_at: String,
    /// `desktop`, `phone` or `import`.
    pub source: String,
    pub file_name: String,
    #[serde(default)]
    pub expense: Option<String>,
    #[serde(default)]
    pub transaction: Option<TransactionId>,
}

impl ReceiptDoc {
    fn grp(&self) -> String {
        match (&self.transaction, &self.expense) {
            (Some(t), _) => format!("txn:{t}"),
            (None, Some(e)) => format!("expense:{e}"),
            (None, None) => INBOX.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ExpenseDoc {
    pub id: String,
    pub date: Date,
    #[serde(default)]
    pub merchant: Option<String>,
    #[serde(default)]
    pub amount: Option<Decimal>,
    #[serde(default)]
    pub category: Option<AccountId>,
    /// `personal` or an account id.
    #[serde(default)]
    pub paid_with: Option<String>,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub tags: SplitTags,
    /// `draft`, `ready`, `posted` or `matched`.
    pub status: String,
    #[serde(default)]
    pub reported: bool,
    /// `desktop` or `phone`.
    pub source: String,
    #[serde(default)]
    pub transaction: Option<TransactionId>,
    /// The transfer that paid the owner back (personal funds).
    #[serde(default)]
    pub reimbursement: Option<TransactionId>,
    pub created_at: String,
    /// Notes about how it was made (e.g. a category from the phone that the book
    /// does not have, mobile-capture.md).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl ExpenseDoc {
    fn personal(&self) -> bool {
        self.paid_with.as_deref() == Some(PERSONAL)
    }

    fn linked(&self) -> bool {
        self.transaction.is_some()
    }

    /// The required fields still missing, in form order.
    fn missing(&self) -> Vec<&'static str> {
        let mut m = Vec::new();
        if self.merchant.is_none() {
            m.push("merchant");
        }
        if self.amount.is_none() {
            m.push("amount");
        }
        if self.category.is_none() {
            m.push("category");
        }
        if self.paid_with.is_none() {
            m.push("paid with");
        }
        m
    }

    fn refresh_status(&mut self) {
        if !self.linked() {
            self.status = if self.missing().is_empty() {
                "ready"
            } else {
                "draft"
            }
            .into();
        }
    }
}

fn not_found(what: &str, id: &str) -> CommandError {
    CommandError::NotFound(format!("{what} {id}"))
}

fn load_expense(ctx: &Ctx<'_>, id: &str) -> CommandResult<ExpenseDoc> {
    let d = ctx
        .tx
        .get_doc(DocTable::Expense, id.trim())?
        .ok_or_else(|| not_found("expense", id))?;
    serde_json::from_value(d.json).map_err(crate::sales::bad_doc)
}

fn all_expenses(ctx: &Ctx<'_>) -> CommandResult<Vec<ExpenseDoc>> {
    crate::sales::load(ctx, DocTable::Expense, None)
}

fn save_expense(ctx: &Ctx<'_>, e: &ExpenseDoc) -> CommandResult<()> {
    crate::sales::save(ctx, DocTable::Expense, &e.id, None, date_ord(e.date), e)
}

fn load_receipt(ctx: &Ctx<'_>, id: &str) -> CommandResult<ReceiptDoc> {
    let d = ctx
        .tx
        .get_doc(DocTable::Receipt, id.trim())?
        .ok_or_else(|| not_found("receipt", id))?;
    serde_json::from_value(d.json).map_err(crate::sales::bad_doc)
}

fn receipts_in(ctx: &Ctx<'_>, grp: &str) -> CommandResult<Vec<ReceiptDoc>> {
    crate::sales::load(ctx, DocTable::Receipt, Some(grp))
}

fn all_receipts(ctx: &Ctx<'_>) -> CommandResult<Vec<ReceiptDoc>> {
    crate::sales::load(ctx, DocTable::Receipt, None)
}

fn save_receipt(ctx: &Ctx<'_>, r: &ReceiptDoc) -> CommandResult<()> {
    let ord = i64::try_from(
        r.added_at
            .bytes()
            .filter(u8::is_ascii_digit)
            .take(14)
            .fold(0u64, |a, b| a * 10 + u64::from(b - b'0')),
    )
    .unwrap_or(0);
    crate::sales::save(ctx, DocTable::Receipt, &r.id, Some(&r.grp()), ord, r)
}

/// The receipts an expense shows: its own, or its transaction's once posted.
fn receipts_of(ctx: &Ctx<'_>, e: &ExpenseDoc) -> CommandResult<Vec<ReceiptDoc>> {
    match e.transaction {
        Some(t) => receipts_in(ctx, &format!("txn:{t}")),
        None => receipts_in(ctx, &format!("expense:{}", e.id)),
    }
}

/// Receipts per transaction id (empty while the module is off: the paperclip hides).
pub(crate) fn receipt_counts(ctx: &Ctx<'_>) -> CommandResult<HashMap<String, usize>> {
    let mut out = HashMap::new();
    if !is_module_enabled(ctx.tx, MODULE)? {
        return Ok(out);
    }
    for r in all_receipts(ctx)? {
        if let Some(t) = r.transaction {
            *out.entry(t.to_string()).or_insert(0) += 1;
        }
    }
    Ok(out)
}

/// A deleted transaction: its receipts go to the inbox; expenses posted or matched
/// to it become unposted; a reimbursement it recorded is forgotten.
pub(crate) fn after_txn_delete(ctx: &Ctx<'_>, txn: TransactionId) -> CommandResult<()> {
    for mut r in receipts_in(ctx, &format!("txn:{txn}"))? {
        r.transaction = None;
        r.expense = None;
        save_receipt(ctx, &r)?;
    }
    for mut e in all_expenses(ctx)? {
        let mut changed = false;
        if e.transaction == Some(txn) {
            e.transaction = None;
            e.refresh_status();
            changed = true;
        }
        if e.reimbursement == Some(txn) {
            e.reimbursement = None;
            changed = true;
        }
        if changed {
            save_expense(ctx, &e)?;
        }
    }
    Ok(())
}

// ----- references -------------------------------------------------------------------

/// Resolve a paid-with reference: `personal` (or its label) or a bank, cash or credit
/// card account; returns `personal` or the account id.
pub(crate) fn paid_with_ref(ctx: &Ctx<'_>, r: &str) -> CommandResult<String> {
    let t = r.trim();
    if t.eq_ignore_ascii_case(PERSONAL)
        || t.eq_ignore_ascii_case(PERSONAL_LABEL)
        || t.eq_ignore_ascii_case("personal funds")
    {
        return Ok(PERSONAL.to_owned());
    }
    let chart = Chart::load(ctx)?;
    let a = chart.resolve(t)?;
    if !matches!(
        a.subtype,
        AccountSubtype::Bank | AccountSubtype::Cash | AccountSubtype::CreditCard
    ) || a.placeholder
    {
        return Err(invalid(format!(
            "Paid with is a bank, cash or credit card account, or {PERSONAL_LABEL}; {} is not",
            chart.full_name(a.id)
        )));
    }
    Ok(a.id.to_string())
}

/// The display name of a stored paid-with value.
pub(crate) fn paid_with_name(ctx: &Ctx<'_>, v: &str) -> CommandResult<String> {
    if v == PERSONAL {
        return Ok(PERSONAL_LABEL.to_owned());
    }
    let chart = Chart::load(ctx)?;
    Ok(chart
        .resolve(v)
        .map(|a| chart.full_name(a.id))
        .unwrap_or_default())
}

fn category_ref(chart: &Chart, r: &str) -> CommandResult<AccountId> {
    let a = chart.resolve(r.trim())?;
    if a.account_type != AccountType::Expense || a.placeholder {
        return Err(invalid(format!(
            "The category is an expense account; {} is not",
            chart.full_name(a.id)
        )));
    }
    Ok(a.id)
}

fn base_scale(ctx: &Ctx<'_>) -> CommandResult<u32> {
    Ok(mk_core::currency::minor_units(
        &ctx.tx.info()?.base_currency,
    ))
}

fn check_amount(ctx: &Ctx<'_>, a: Decimal) -> CommandResult<Decimal> {
    if a <= Decimal::ZERO {
        return Err(invalid("The amount must be more than zero."));
    }
    let dp = base_scale(ctx)?;
    if a.normalize().scale() > dp {
        return Err(invalid(format!(
            "The amount can have at most {dp} decimal places."
        )));
    }
    Ok(mk_ledger::canonical_amount(a, dp))
}

fn txn_id(r: &str) -> CommandResult<TransactionId> {
    r.trim()
        .parse()
        .map_err(|_| invalid(format!("{r:?} is not a transaction id")))
}

fn load_txn(ctx: &Ctx<'_>, id: TransactionId) -> CommandResult<Transaction> {
    ctx.tx
        .transaction(id)?
        .ok_or_else(|| not_found("transaction", &id.to_string()))
}

// ----- output -----------------------------------------------------------------------

fn receipt_json(r: &ReceiptDoc) -> Value {
    json!({
        "id": r.id, "kind": r.kind, "bytes": r.bytes, "sha256": r.sha256,
        "width": r.width, "height": r.height, "added_at": r.added_at,
        "source": r.source, "file_name": r.file_name,
        "expense": r.expense, "transaction": r.transaction,
    })
}

fn expense_json(ctx: &Ctx<'_>, chart: &Chart, e: &ExpenseDoc) -> CommandResult<Value> {
    let receipts: Vec<Value> = receipts_of(ctx, e)?.iter().map(receipt_json).collect();
    let paid_name = match &e.paid_with {
        Some(p) => Some(paid_with_name(ctx, p)?),
        None => None,
    };
    Ok(json!({
        "id": e.id,
        "date": e.date,
        "merchant": e.merchant,
        "amount": e.amount.map(money),
        "category": e.category,
        "category_name": e.category.map(|c| chart.full_name(c)),
        "paid_with": e.paid_with,
        "paid_with_name": paid_name,
        "reimbursable": e.personal(),
        "purpose": e.purpose,
        "tags": e.tags,
        "tag_names": crate::dimensions::tag_names(ctx, &e.tags)?,
        "status": e.status,
        "reported": e.reported,
        "source": e.source,
        "transaction": e.transaction,
        "reimbursement": e.reimbursement,
        "missing": e.missing(),
        "notes": e.notes,
        "receipt_count": receipts.len(),
        "receipts": receipts,
    }))
}

// ----- expense commands -------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExpenseArgs {
    date: Date,
    /// The payee, at most 100 characters.
    #[serde(default)]
    merchant: Option<String>,
    /// A positive amount in the book's currency.
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    amount: Option<Decimal>,
    /// An expense account (id, full name or code).
    #[serde(default)]
    category: Option<String>,
    /// A bank, cash or credit card account, or `personal` (Personal funds,
    /// reimbursable). Defaults to the `expenses.default_paid_with` setting.
    #[serde(default)]
    paid_with: Option<String>,
    /// Business purpose or note, at most 500 characters.
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    fund: Option<String>,
    /// A project (module projects); "" clears.
    #[serde(default)]
    project: Option<String>,
    /// A billable project cost (default: the project's setting).
    #[serde(default)]
    billable: Option<bool>,
    /// Receipts to attach (from the inbox), at most 10.
    #[serde(default)]
    receipts: Vec<String>,
    /// `desktop` (default) or `phone`.
    #[serde(default)]
    source: Option<String>,
}

fn clean_text(v: Option<String>, max: usize, what: &str) -> CommandResult<Option<String>> {
    crate::sales::text(v, max, what)
}

fn expense_create(ctx: &Ctx<'_>, a: ExpenseArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let source = match a.source.as_deref().map(str::trim) {
        None | Some("") | Some("desktop") => "desktop",
        Some("phone") => "phone",
        Some(other) => return Err(invalid(format!("source is desktop or phone, not {other}"))),
    };
    let paid_with = match a.paid_with.as_deref().map(str::trim) {
        Some("") => None,
        Some(p) => Some(paid_with_ref(ctx, p)?),
        None => ms::get(ctx, "expenses.default_paid_with")?
            .as_str()
            .and_then(|v| paid_with_ref(ctx, v).ok()),
    };
    let mut e = ExpenseDoc {
        id: new_id(),
        date: a.date,
        merchant: clean_text(a.merchant, MAX_MERCHANT, "merchant")?,
        amount: a.amount.map(|x| check_amount(ctx, x)).transpose()?,
        category: match a.category.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(c) => Some(category_ref(&chart, c)?),
        },
        paid_with,
        purpose: clean_text(a.purpose, MAX_PURPOSE, "purpose")?,
        tags: crate::dimensions::resolve_tags_p(
            ctx,
            a.class.as_deref(),
            a.location.as_deref(),
            a.fund.as_deref(),
            a.project.as_deref(),
            a.billable,
        )?,
        status: String::new(),
        reported: false,
        source: source.to_owned(),
        transaction: None,
        reimbursement: None,
        created_at: now_rfc3339()?,
        notes: Vec::new(),
    };
    e.refresh_status();
    if a.receipts.len() > MAX_RECEIPTS {
        return Err(invalid(format!(
            "An expense can have at most {MAX_RECEIPTS} receipts."
        )));
    }
    save_expense(ctx, &e)?;
    for r in &a.receipts {
        let mut doc = load_receipt(ctx, r)?;
        doc.expense = Some(e.id.clone());
        doc.transaction = None;
        save_receipt(ctx, &doc)?;
    }
    expense_json(ctx, &chart, &e)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    expense: String,
    #[serde(default)]
    date: Option<Date>,
    /// "" clears it (the expense becomes a draft).
    #[serde(default)]
    merchant: Option<String>,
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    amount: Option<Decimal>,
    /// "" clears it.
    #[serde(default)]
    category: Option<String>,
    /// "" clears it.
    #[serde(default)]
    paid_with: Option<String>,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    fund: Option<String>,
    /// A project (module projects); "" clears.
    #[serde(default)]
    project: Option<String>,
    /// A billable project cost (default: the project's setting).
    #[serde(default)]
    billable: Option<bool>,
}

fn expense_update(ctx: &Ctx<'_>, a: UpdateArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let old = load_expense(ctx, &a.expense)?;
    let mut e = old.clone();
    if let Some(d) = a.date {
        e.date = d;
    }
    if let Some(m) = a.merchant {
        e.merchant = clean_text(Some(m), MAX_MERCHANT, "merchant")?;
    }
    if let Some(x) = a.amount {
        e.amount = Some(check_amount(ctx, x)?);
    }
    if let Some(c) = a.category {
        e.category = match c.trim() {
            "" => None,
            c => Some(category_ref(&chart, c)?),
        };
    }
    if let Some(p) = a.paid_with {
        e.paid_with = match p.trim() {
            "" => None,
            p => Some(paid_with_ref(ctx, p)?),
        };
    }
    if let Some(p) = a.purpose {
        e.purpose = clean_text(Some(p), MAX_PURPOSE, "purpose")?;
    }
    if a.class.is_some()
        || a.location.is_some()
        || a.fund.is_some()
        || a.project.is_some()
        || a.billable.is_some()
    {
        let pick = |given: Option<String>, cur: &Option<String>| given.or_else(|| cur.clone());
        let billable = a.billable.or_else(|| {
            (a.project.is_none() && old.tags.project.is_some()).then_some(old.tags.billable)
        });
        e.tags = crate::dimensions::resolve_tags_p(
            ctx,
            pick(a.class, &old.tags.class).as_deref(),
            pick(a.location, &old.tags.location).as_deref(),
            pick(a.fund, &old.tags.fund).as_deref(),
            pick(a.project, &old.tags.project).as_deref(),
            billable,
        )?;
    }
    let closed = ctx.tx.closing_date()?;
    if closed.is_some_and(|c| old.date <= c || e.date <= c) {
        return Err(rejected(format!(
            "{} is in the closed period; the expense can't be changed",
            if closed.is_some_and(|c| old.date <= c) {
                old.date
            } else {
                e.date
            }
        )));
    }
    if let Some(t) = e.transaction {
        if !e.missing().is_empty() {
            return Err(rejected(format!(
                "A posted expense needs its {}.",
                e.missing().join(", ")
            )));
        }
        if e.paid_with != old.paid_with {
            return Err(rejected(
                "Paid with can't change once the expense is posted; delete it and enter it again.",
            ));
        }
        update_txn(ctx, &old, &e, t)?;
    } else {
        e.refresh_status();
    }
    save_expense(ctx, &e)?;
    expense_json(ctx, &chart, &e)
}

/// Carry an edit of a posted or matched expense into its transaction: date, payee,
/// amount and category (and memo for a transaction the expense created).
fn update_txn(
    ctx: &Ctx<'_>,
    old: &ExpenseDoc,
    e: &ExpenseDoc,
    t: TransactionId,
) -> CommandResult<()> {
    let before = load_txn(ctx, t)?;
    if before.splits.len() != 2 {
        return Err(rejected(
            "This expense's transaction has more than two lines; change it in the register.",
        ));
    }
    let amount = e.amount.unwrap_or_default();
    let mut after = before.clone();
    after.date = e.date;
    after.payee.clone_from(&e.merchant);
    if e.status == "posted" {
        after.memo.clone_from(&e.purpose);
    }
    for s in &mut after.splits {
        if s.value > Decimal::ZERO {
            if let Some(c) = e.category {
                s.account = c;
            }
            if e.tags != old.tags {
                s.tags = e.tags.clone();
            }
            s.value = amount;
            s.quantity = amount;
        } else {
            s.value = -amount;
            s.quantity = -amount;
        }
    }
    mk_ledger::sort_splits(&mut after.splits);
    validate_change(ctx, Some(&before), Some(&after))?;
    if after != before {
        ctx.save_txn(Some(&before), &after)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeleteArgs {
    expense: String,
    /// For a posted expense: true deletes its transaction too, false keeps it. Must
    /// be given for a posted expense.
    #[serde(default)]
    delete_transaction: Option<bool>,
}

fn expense_delete(ctx: &Ctx<'_>, a: DeleteArgs) -> CommandResult<Value> {
    let e = load_expense(ctx, &a.expense)?;
    if ctx.tx.closing_date()?.is_some_and(|c| e.date <= c) {
        return Err(rejected(format!(
            "{} is in the closed period; the expense can't be deleted",
            e.date
        )));
    }
    let mut deleted_txn = None;
    match (e.status.as_str(), e.transaction, a.delete_transaction) {
        ("posted", Some(_), None) => {
            return Err(CommandError::Coded {
                code: "needs_choice",
                message:
                    "This expense is posted. Delete its transaction too, or keep the transaction?"
                        .into(),
            });
        }
        ("posted", Some(t), Some(true)) => {
            if let Some(txn) = ctx.tx.transaction(t)? {
                if txn
                    .splits
                    .iter()
                    .any(|s| s.reconcile == Reconcile::Reconciled)
                {
                    return Err(rejected(
                        "The transaction has reconciled lines and can't be deleted; keep it.",
                    ));
                }
                validate_change(ctx, Some(&txn), None)?;
                ctx.remove_txn(&txn)?;
                crate::schedules::unlink_txn(ctx, t)?;
                deleted_txn = Some(t);
            }
        }
        _ => {}
    }
    // Receipts of an unposted expense, or of a deleted transaction, go to the inbox.
    let grp = match (e.transaction, deleted_txn) {
        (_, Some(t)) => Some(format!("txn:{t}")),
        (None, None) => Some(format!("expense:{}", e.id)),
        (Some(_), None) => None,
    };
    let mut to_inbox = 0usize;
    if let Some(g) = grp {
        for mut r in receipts_in(ctx, &g)? {
            r.expense = None;
            r.transaction = None;
            save_receipt(ctx, &r)?;
            to_inbox += 1;
        }
    }
    ctx.save_doc(DocTable::Expense, &e.id, None)?;
    if let Some(t) = deleted_txn {
        // Other expenses never share a transaction, but reimbursements may point at
        // it.
        after_txn_delete(ctx, t)?;
    }
    Ok(json!({
        "deleted": e.id,
        "transaction_deleted": deleted_txn,
        "receipts_to_inbox": to_inbox,
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExpenseRef {
    expense: String,
}

fn expense_get(ctx: &Ctx<'_>, a: ExpenseRef) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let e = load_expense(ctx, &a.expense)?;
    expense_json(ctx, &chart, &e)
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    #[serde(default)]
    from: Option<Date>,
    #[serde(default)]
    to: Option<Date>,
    /// draft, ready, posted, matched or reported.
    #[serde(default)]
    status: Option<String>,
    /// An account or `personal`.
    #[serde(default)]
    paid_with: Option<String>,
    #[serde(default)]
    category: Option<String>,
    /// Matched against merchant and purpose, ignoring case.
    #[serde(default)]
    text: Option<String>,
}

fn select(ctx: &Ctx<'_>, chart: &Chart, a: &ListArgs) -> CommandResult<Vec<ExpenseDoc>> {
    let paid = a
        .paid_with
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .map(|p| paid_with_ref(ctx, p))
        .transpose()?;
    let cat = a
        .category
        .as_deref()
        .filter(|c| !c.trim().is_empty())
        .map(|c| chart.resolve(c.trim()).map(|x| x.id))
        .transpose()?;
    let status = a
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "all");
    if let Some(s) = status
        && !matches!(s, "draft" | "ready" | "posted" | "matched" | "reported")
    {
        return Err(invalid(format!(
            "status is draft, ready, posted, matched or reported, not {s}"
        )));
    }
    let text = a
        .text
        .as_deref()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty());
    let mut out: Vec<ExpenseDoc> = all_expenses(ctx)?
        .into_iter()
        .filter(|e| a.from.is_none_or(|f| e.date >= f) && a.to.is_none_or(|t| e.date <= t))
        .filter(|e| match status {
            None => true,
            Some("reported") => e.reported,
            Some(s) => e.status == s,
        })
        .filter(|e| {
            paid.as_ref()
                .is_none_or(|p| e.paid_with.as_ref() == Some(p))
        })
        .filter(|e| cat.is_none_or(|c| e.category == Some(c)))
        .filter(|e| {
            text.as_ref().is_none_or(|t| {
                [&e.merchant, &e.purpose]
                    .iter()
                    .any(|f| f.as_deref().is_some_and(|v| v.to_lowercase().contains(t)))
            })
        })
        .collect();
    out.sort_by(|x, y| {
        x.date
            .cmp(&y.date)
            .then_with(|| x.created_at.cmp(&y.created_at))
    });
    Ok(out)
}

fn expense_list(ctx: &Ctx<'_>, a: ListArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let mut rows = select(ctx, &chart, &a)?;
    rows.reverse();
    let out = rows
        .iter()
        .map(|e| expense_json(ctx, &chart, e))
        .collect::<CommandResult<Vec<_>>>()?;
    Ok(Value::Array(out))
}

// ----- posting ----------------------------------------------------------------------

/// Transactions an expense could match: money out of its paid-with account of the
/// same amount, within the match window, posted, not linked to another expense.
fn candidates(ctx: &Ctx<'_>, e: &ExpenseDoc) -> CommandResult<Vec<Transaction>> {
    let (Some(paid), Some(amount)) = (e.paid_with.as_deref(), e.amount) else {
        return Ok(Vec::new());
    };
    if paid == PERSONAL {
        return Ok(Vec::new());
    }
    let account: AccountId = paid
        .parse()
        .map_err(|_| CommandError::Integrity("bad paid-with account".into()))?;
    let days = i32::try_from(ms::get_i64(ctx, "expenses.match_days")?).unwrap_or(5);
    let linked: HashSet<TransactionId> = all_expenses(ctx)?
        .iter()
        .filter(|x| x.id != e.id)
        .filter_map(|x| x.transaction)
        .collect();
    let splits = ctx
        .tx
        .account_splits(account, e.date.add_days(-days), e.date.add_days(days))?;
    let mut seen = HashSet::new();
    let mut out: Vec<Transaction> = Vec::new();
    for s in splits {
        if s.quantity != -amount || linked.contains(&s.txn_id) || !seen.insert(s.txn_id) {
            continue;
        }
        if let Some(t) = ctx.tx.transaction(s.txn_id)?
            && t.status == TxnStatus::Posted
        {
            out.push(t);
        }
    }
    out.sort_by_key(|t| (e.date.days_until(t.date).abs(), t.date));
    Ok(out)
}

fn candidate_json(chart: &Chart, t: &Transaction, amount: Decimal) -> Value {
    let other: Vec<String> = t
        .splits
        .iter()
        .filter(|s| s.value > Decimal::ZERO)
        .map(|s| chart.full_name(s.account))
        .collect();
    json!({
        "id": t.id, "date": t.date, "payee": t.payee, "memo": t.memo,
        "amount": money(amount), "category": other.join(", "),
        "uncategorized": uncategorized_other(chart, t).is_some(),
    })
}

/// The index of the Uncategorized split of a two-split transaction, if that is
/// its other side.
fn uncategorized_other(chart: &Chart, t: &Transaction) -> Option<usize> {
    if t.splits.len() != 2 {
        return None;
    }
    t.splits.iter().position(|s| {
        chart.get(s.account).is_some_and(|a| {
            a.role.as_deref() == Some(crate::chart::roles::UNCATEGORIZED_EXPENSE)
                || (a.parent.is_none() && a.name.eq_ignore_ascii_case(crate::import::UNCAT_EXPENSE))
        })
    })
}

fn expense_match_candidates(ctx: &Ctx<'_>, a: ExpenseRef) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let e = load_expense(ctx, &a.expense)?;
    let amount = e.amount.unwrap_or_default();
    Ok(Value::Array(
        candidates(ctx, &e)?
            .iter()
            .map(|t| candidate_json(&chart, t, amount))
            .collect(),
    ))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PostArgs {
    expense: String,
    /// A candidate transaction's id to match, or `new` to create a transaction.
    /// Without it, a single candidate or several are returned for the user to
    /// choose (nothing changes); with none, a new transaction is created.
    #[serde(default, rename = "match")]
    matched: Option<String>,
}

fn move_receipts(ctx: &Ctx<'_>, e: &ExpenseDoc, t: TransactionId) -> CommandResult<usize> {
    let mut n = 0;
    for mut r in receipts_in(ctx, &format!("expense:{}", e.id))? {
        r.transaction = Some(t);
        r.expense = None;
        save_receipt(ctx, &r)?;
        n += 1;
    }
    Ok(n)
}

fn expense_post(ctx: &Ctx<'_>, a: PostArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let mut e = load_expense(ctx, &a.expense)?;
    if e.linked() {
        return Err(rejected("This expense is already posted."));
    }
    e.refresh_status();
    let missing = e.missing();
    if !missing.is_empty() {
        return Err(rejected(format!(
            "Fill in the {} before posting.",
            missing.join(", ")
        )));
    }
    require_open_period(ctx, e.date)?;
    let amount = e.amount.unwrap_or_default();
    let choice = a
        .matched
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    if let Some(m) = choice.filter(|m| !m.eq_ignore_ascii_case("new")) {
        let id = txn_id(m)?;
        let found = candidates(ctx, &e)?.into_iter().find(|t| t.id == id);
        let Some(before) = found else {
            return Err(rejected(
                "That transaction doesn't match this expense (amount, account, date or already matched).",
            ));
        };
        let mut after = before.clone();
        if let (Some(i), Some(cat)) = (uncategorized_other(&chart, &before), e.category) {
            if let Some(s) = after.splits.get_mut(i) {
                s.account = cat;
                s.tags = e.tags.clone();
            }
            mk_ledger::sort_splits(&mut after.splits);
        }
        validate_change(ctx, Some(&before), Some(&after))?;
        if after != before {
            ctx.save_txn(Some(&before), &after)?;
        }
        let moved = move_receipts(ctx, &e, id)?;
        e.transaction = Some(id);
        e.status = "matched".into();
        save_expense(ctx, &e)?;
        let mut v = expense_json(ctx, &chart, &e)?;
        set_key(&mut v, "receipts_moved", json!(moved));
        return Ok(v);
    }
    if choice.is_none() {
        let list = candidates(ctx, &e)?;
        if !list.is_empty() {
            ctx.quiet_noop.set(true);
            return Ok(json!({
                "status": "needs_choice",
                "expense": e.id,
                "candidates": list.iter().map(|t| candidate_json(&chart, t, amount)).collect::<Vec<_>>(),
            }));
        }
    }
    let category = e
        .category
        .ok_or_else(|| CommandError::Integrity("ready expense without category".into()))?;
    let credit = if e.personal() {
        crate::module_settings::account_or_create_role(
            ctx,
            "expenses.reimbursement_account",
            Some(crate::chart::roles::REIMBURSEMENTS),
            "Reimbursements Payable",
            AccountType::Liability,
            AccountSubtype::Payable,
        )?
    } else {
        e.paid_with
            .as_deref()
            .unwrap_or_default()
            .parse()
            .map_err(|_| CommandError::Integrity("bad paid-with account".into()))?
    };
    let split = |account: AccountId, value: Decimal, tags: SplitTags| Split {
        account,
        value,
        quantity: value,
        memo: None,
        reconcile: Reconcile::New,
        fingerprint: None,
        tags,
    };
    let id = crate::sales::post_txn(
        ctx,
        None,
        e.date,
        e.merchant.as_deref().unwrap_or_default(),
        e.purpose.clone(),
        vec![
            split(category, amount, e.tags.clone()),
            split(credit, -amount, SplitTags::default()),
        ],
    )?;
    let moved = move_receipts(ctx, &e, id)?;
    e.transaction = Some(id);
    e.status = "posted".into();
    save_expense(ctx, &e)?;
    let mut v = expense_json(ctx, &chart, &e)?;
    set_key(&mut v, "receipts_moved", json!(moved));
    Ok(v)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReimburseArgs {
    /// Posted expenses paid with personal funds and not yet reimbursed.
    #[serde(default)]
    expenses: Vec<String>,
    /// Posted reimbursable trips not yet reimbursed (mileage.md).
    #[serde(default)]
    trips: Vec<String>,
    /// The bank or cash account the money comes from.
    from_account: String,
    /// Defaults to today.
    #[serde(default)]
    date: Option<Date>,
}

fn expense_record_reimbursement(ctx: &Ctx<'_>, a: ReimburseArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    if a.expenses.is_empty() && a.trips.is_empty() {
        return Err(invalid("Choose at least one expense."));
    }
    let from = chart.resolve(a.from_account.trim())?;
    if !matches!(from.subtype, AccountSubtype::Bank | AccountSubtype::Cash) {
        return Err(invalid(format!(
            "Reimbursements come from a bank or cash account; {} is not",
            chart.full_name(from.id)
        )));
    }
    let from_id = from.id;
    let date = match a.date {
        Some(d) => d,
        None => today_utc()?,
    };
    require_open_period(ctx, date)?;
    let mut list = Vec::new();
    let mut seen = HashSet::new();
    for r in &a.expenses {
        let e = load_expense(ctx, r)?;
        if !seen.insert(e.id.clone()) {
            continue;
        }
        if !e.personal() || e.status != "posted" {
            return Err(rejected(format!(
                "{} isn't a posted expense paid with personal funds.",
                e.merchant.as_deref().unwrap_or("This expense")
            )));
        }
        if e.reimbursement.is_some() {
            return Err(rejected(format!(
                "{} ({}) is already reimbursed.",
                e.merchant.as_deref().unwrap_or("The expense"),
                e.date
            )));
        }
        list.push(e);
    }
    let mut trips = Vec::new();
    if !a.trips.is_empty() && !is_module_enabled(ctx.tx, crate::mileage::MODULE)? {
        return Err(rejected("Trips need the Mileage feature."));
    }
    for r in &a.trips {
        let t = crate::mileage::load_trip(ctx, r)?;
        if !seen.insert(t.id.clone()) {
            continue;
        }
        if !t.reimbursable || t.status != "posted" {
            return Err(rejected(format!(
                "The trip on {} isn't a posted reimbursable trip.",
                t.date
            )));
        }
        if t.reimbursement.is_some() {
            return Err(rejected(format!(
                "The trip on {} is already reimbursed.",
                t.date
            )));
        }
        trips.push(t);
    }
    // Owed per liability account (the credit side of each expense's transaction).
    let mut owed: BTreeMap<AccountId, Decimal> = BTreeMap::new();
    let mut total = Decimal::ZERO;
    let posted = list
        .iter()
        .map(|e| (e.transaction, e.amount.unwrap_or_default()))
        .chain(trips.iter().map(|t| (t.transaction, t.reimburse_amount)));
    for (txn, amount) in posted {
        let t = load_txn(ctx, txn.unwrap_or_default())?;
        let liability = t
            .splits
            .iter()
            .find(|s| s.value < Decimal::ZERO)
            .map(|s| s.account)
            .ok_or_else(|| CommandError::Integrity("expense transaction without credit".into()))?;
        *owed.entry(liability).or_insert(Decimal::ZERO) += amount;
        total += amount;
    }
    let mut memo = String::from("Reimbursement for ");
    let parts: Vec<String> = list
        .iter()
        .map(|e| {
            format!(
                "{} {} {}",
                e.date,
                e.merchant.as_deref().unwrap_or_default(),
                e.amount.map(money).unwrap_or_default()
            )
        })
        .chain(trips.iter().map(|t| {
            format!(
                "{} Mileage {} mi {}",
                t.date,
                t.miles,
                money(t.reimburse_amount)
            )
        }))
        .collect();
    memo.push_str(&parts.join("; "));
    let memo: String = memo.chars().take(MAX_PURPOSE).collect();
    let mut splits: Vec<Split> = owed
        .iter()
        .map(|(acc, v)| Split {
            account: *acc,
            value: *v,
            quantity: *v,
            memo: None,
            reconcile: Reconcile::New,
            fingerprint: None,
            tags: SplitTags::default(),
        })
        .collect();
    splits.push(Split {
        account: from_id,
        value: -total,
        quantity: -total,
        memo: None,
        reconcile: Reconcile::New,
        fingerprint: None,
        tags: SplitTags::default(),
    });
    let id = crate::sales::post_txn(ctx, None, date, "Reimbursement", Some(memo), splits)?;
    for mut e in list.clone() {
        e.reimbursement = Some(id);
        save_expense(ctx, &e)?;
    }
    for mut t in trips.clone() {
        t.reimbursement = Some(id);
        crate::mileage::save_trip(ctx, &t)?;
    }
    Ok(json!({
        "transaction": id,
        "total": money(total),
        "expenses": list.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
        "trips": trips.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
    }))
}

// ----- receipts ---------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddArgs {
    /// The file, base64 (or `path` in front ends that read files).
    #[serde(default)]
    content_base64: Option<String>,
    #[serde(default)]
    path: Option<String>,
    /// The original file name (kept, cleaned, for display).
    #[serde(default)]
    file_name: Option<String>,
    /// `desktop` (default), `phone` or `import`.
    #[serde(default)]
    source: Option<String>,
    /// Store it even when the same file is already in the book.
    #[serde(default)]
    allow_duplicate: bool,
    /// Attach it to this expense straight away.
    #[serde(default)]
    expense: Option<String>,
    /// Attach it to this transaction straight away.
    #[serde(default)]
    transaction: Option<String>,
}

fn hex(bytes: &[u8]) -> String {
    crate::import::hex(bytes)
}

fn clean_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let c = mk_core::text::clean(base, usize::MAX).text;
    let c: String = c
        .chars()
        .filter(|ch| !matches!(ch, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .take(120)
        .collect();
    if c.trim().is_empty() {
        "receipt".into()
    } else {
        c.trim().to_owned()
    }
}

/// Where a receipt is attached, in words.
fn attached_to(ctx: &Ctx<'_>, chart: &Chart, r: &ReceiptDoc) -> CommandResult<String> {
    if let Some(t) = r.transaction
        && let Some(txn) = ctx.tx.transaction(t)?
    {
        let _ = chart;
        return Ok(format!(
            "the transaction {} on {}",
            txn.payee.as_deref().unwrap_or("(no payee)"),
            txn.date
        ));
    }
    if let Some(e) = &r.expense
        && let Ok(x) = load_expense(ctx, e)
    {
        return Ok(format!(
            "the expense {} on {}",
            x.merchant.as_deref().unwrap_or("(no merchant)"),
            x.date
        ));
    }
    Ok("a receipt in the inbox".into())
}

/// Check a link target and count: returns (expense id, transaction id).
fn target(
    ctx: &Ctx<'_>,
    expense: Option<&str>,
    transaction: Option<&str>,
) -> CommandResult<(Option<String>, Option<TransactionId>)> {
    let expense = expense.map(str::trim).filter(|s| !s.is_empty());
    let transaction = transaction.map(str::trim).filter(|s| !s.is_empty());
    match (expense, transaction) {
        (Some(_), Some(_)) => Err(invalid("give an expense or a transaction, not both")),
        (Some(x), None) => {
            let e = load_expense(ctx, x)?;
            if let Some(t) = e.transaction {
                return Ok((None, Some(t)));
            }
            if receipts_of(ctx, &e)?.len() >= MAX_RECEIPTS {
                return Err(rejected(format!(
                    "An expense can have at most {MAX_RECEIPTS} receipts."
                )));
            }
            Ok((Some(e.id), None))
        }
        (None, Some(t)) => {
            let id = txn_id(t)?;
            load_txn(ctx, id)?;
            Ok((None, Some(id)))
        }
        (None, None) => Ok((None, None)),
    }
}

fn receipt_add(ctx: &Ctx<'_>, a: AddArgs) -> CommandResult<Value> {
    let refused = |m: String| CommandError::Coded {
        code: "invalid_args",
        message: m,
    };
    // Base64 of 10 MB is under 14 MB: anything far larger is refused before decoding.
    let bytes = match (&a.content_base64, &a.path) {
        (Some(b), None) => {
            if b.len() > mk_import::receipt::MAX_BYTES / 3 * 4 + 16 {
                return Err(refused(mk_import::receipt::MSG_TOO_BIG.into()));
            }
            base64::engine::general_purpose::STANDARD
                .decode(b.trim())
                .map_err(|e| invalid(format!("content_base64 is not valid base64: {e}")))?
        }
        (None, Some(p)) => {
            let reader = ctx
                .files
                .ok_or_else(|| invalid("this front end cannot read files; send content_base64"))?;
            reader
                .read(p)
                .map_err(|e| invalid(format!("cannot read {p}: {e}")))?
        }
        _ => return Err(invalid("give exactly one of content_base64 or path")),
    };
    let file_name = clean_file_name(
        a.file_name
            .as_deref()
            .or(a.path.as_deref())
            .unwrap_or("receipt"),
    );
    let source = match a.source.as_deref().map(str::trim) {
        None | Some("") | Some("desktop") => "desktop",
        Some("phone") => "phone",
        Some("import") => "import",
        Some(o) => {
            return Err(invalid(format!(
                "source is desktop, phone or import, not {o}"
            )));
        }
    };
    let sha = hex(&Sha256::digest(&bytes));
    *ctx.audit_args.borrow_mut() = Some(json!({
        "file_name": file_name, "bytes": bytes.len(), "sha256": sha, "source": source,
        "expense": a.expense, "transaction": a.transaction,
    }));
    let (expense, transaction) = target(ctx, a.expense.as_deref(), a.transaction.as_deref())?;
    if !a.allow_duplicate
        && let Some(dup) = all_receipts(ctx)?.into_iter().find(|r| r.sha256 == sha)
    {
        let chart = Chart::load(ctx)?;
        return Err(CommandError::Coded {
            code: "duplicate_receipt",
            message: format!(
                "This receipt is already attached to {}.",
                attached_to(ctx, &chart, &dup)?
            ),
        });
    }
    let r = store_receipt(ctx, &bytes, sha, file_name, source, expense, transaction)?;
    Ok(receipt_json(&r))
}

/// Check, normalise and store a receipt file (no duplicate check).
fn store_receipt(
    ctx: &Ctx<'_>,
    bytes: &[u8],
    sha: String,
    file_name: String,
    source: &str,
    expense: Option<String>,
    transaction: Option<TransactionId>,
) -> CommandResult<ReceiptDoc> {
    let refused = |m: String| CommandError::Coded {
        code: "invalid_args",
        message: m,
    };
    let p = mk_import::receipt::process(bytes, &file_name).map_err(|e| {
        let text = e.to_string();
        let msg = [
            mk_import::receipt::MSG_TOO_BIG,
            mk_import::receipt::MSG_PICTURE,
            mk_import::receipt::MSG_HEIC,
            mk_import::receipt::MSG_PDF,
            mk_import::receipt::MSG_PDF_PAGES,
            mk_import::receipt::MSG_KIND,
            mk_import::receipt::MSG_PIXELS,
        ]
        .into_iter()
        .find(|m| text.contains(m))
        .unwrap_or(mk_import::receipt::MSG_PICTURE);
        refused(msg.to_owned())
    })?;
    let file = hex(&Sha256::digest(&p.content));
    ctx.tx
        .put_receipt_file(&file, &p.content, p.thumbnail_png.as_deref())?;
    let r = ReceiptDoc {
        id: new_id(),
        kind: p.kind.as_str().to_owned(),
        bytes: p.content.len(),
        sha256: sha,
        file,
        width: p.width,
        height: p.height,
        added_at: now_rfc3339()?,
        source: source.to_owned(),
        file_name,
        expense,
        transaction,
    };
    save_receipt(ctx, &r)?;
    Ok(r)
}

/// A receipt collected from the phone: attached to `expense`, or to the inbox.
/// Returns the receipt's id.
pub(crate) fn add_phone_receipt(
    ctx: &Ctx<'_>,
    bytes: &[u8],
    file_name: &str,
    expense: Option<&str>,
) -> CommandResult<String> {
    let sha = hex(&Sha256::digest(bytes));
    let r = store_receipt(
        ctx,
        bytes,
        sha,
        clean_file_name(file_name),
        "phone",
        expense.map(str::to_owned),
        None,
    )?;
    Ok(r.id)
}

/// What a phone expense became.
#[derive(Debug, Clone)]
pub(crate) struct PhoneExpense {
    pub id: String,
    pub status: String,
    pub notes: Vec<String>,
}

/// An expense from the phone (mobile-capture.md "Collect"): names are resolved
/// against the book; a category, paid-with choice, tag or amount the book cannot use
/// is left empty with a note, so the expense stays a draft for review.
/// Tags typed on the phone (an expense's, or a trip's: mileage.md revision 2), by
/// name; one the book does not know (or whose module is off) is dropped with a note.
pub(crate) fn phone_tags(
    ctx: &Ctx<'_>,
    t: &mk_capture_api::Tags,
    notes: &mut Vec<String>,
) -> mk_core::SplitTags {
    let one = |dim: &str, v: &Option<String>, notes: &mut Vec<String>| {
        let v = v
            .as_deref()
            .map(|t| mk_core::text::clean(t, 100).text)
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())?;
        let r = match dim {
            "class" => crate::dimensions::resolve_tags(ctx, Some(&v), None, None).map(|t| t.class),
            "location" => {
                crate::dimensions::resolve_tags(ctx, None, Some(&v), None).map(|t| t.location)
            }
            "project" => crate::dimensions::resolve_tags_p(ctx, None, None, None, Some(&v), None)
                .map(|t| t.project),
            _ => crate::dimensions::resolve_tags(ctx, None, None, Some(&v)).map(|t| t.fund),
        };
        match r {
            Ok(id) => id,
            Err(_) => {
                notes.push(format!("{} from phone not found: {v}", capitalised(dim)));
                None
            }
        }
    };
    let mut tags = mk_core::SplitTags {
        class: one("class", &t.class, notes),
        location: one("location", &t.location, notes),
        fund: one("fund", &t.fund, notes),
        project: one("project", &t.project, notes),
        billable: false,
    };
    // projects.md: the project's fund (when none came) and billable default.
    if let Some(p) = tags
        .project
        .as_deref()
        .and_then(|id| crate::projects::find(ctx, id).ok())
    {
        if !p.open() {
            notes.push(format!("Project from phone is closed: {}", p.name));
        }
        tags.billable = p.billable;
        if tags.fund.is_none()
            && crate::is_module_enabled(ctx.tx, crate::dimensions::FUNDS).unwrap_or(false)
        {
            tags.fund = p.fund;
        }
    }
    tags
}

pub(crate) fn create_from_phone(
    ctx: &Ctx<'_>,
    f: &mk_capture_api::ExpenseFields,
    today: Date,
) -> CommandResult<PhoneExpense> {
    let chart = Chart::load(ctx)?;
    let mut notes = Vec::new();
    let text = |v: &Option<String>, max: usize| {
        v.as_deref()
            .map(|t| mk_core::text::clean(t, max).text)
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
    };
    let date = match f.date.as_deref().map(str::parse::<Date>) {
        Some(Ok(d)) => d,
        Some(Err(_)) => {
            notes.push(format!(
                "Date from phone not usable: {}",
                f.date.as_deref().unwrap_or_default()
            ));
            today
        }
        None => today,
    };
    let amount = match f.amount.as_deref() {
        None => None,
        Some(a) => match a
            .trim()
            .parse::<Decimal>()
            .map_err(|_| invalid("amount"))
            .and_then(|x| check_amount(ctx, x))
        {
            Ok(x) => Some(x),
            Err(_) => {
                notes.push(format!("Amount from phone not usable: {a}"));
                None
            }
        },
    };
    let category = match text(&f.category, 200) {
        None => None,
        Some(c) => match category_ref(&chart, &c) {
            Ok(id) => Some(id),
            Err(_) => {
                notes.push(format!("Category from phone not found: {c}"));
                None
            }
        },
    };
    let paid_with = match text(&f.paid_with, 200) {
        Some(p) => match paid_with_ref(ctx, &p) {
            Ok(v) => Some(v),
            Err(_) => {
                notes.push(format!("Paid with from phone not found: {p}"));
                None
            }
        },
        None => ms::get(ctx, "expenses.default_paid_with")?
            .as_str()
            .and_then(|v| paid_with_ref(ctx, v).ok()),
    };
    let tags = phone_tags(ctx, &f.tags, &mut notes);
    let mut e = ExpenseDoc {
        id: new_id(),
        date,
        merchant: text(&f.merchant, MAX_MERCHANT),
        amount,
        category,
        paid_with,
        purpose: text(&f.purpose, MAX_PURPOSE),
        tags,
        status: String::new(),
        reported: false,
        source: "phone".into(),
        transaction: None,
        reimbursement: None,
        created_at: now_rfc3339()?,
        notes,
    };
    e.refresh_status();
    save_expense(ctx, &e)?;
    Ok(PhoneExpense {
        id: e.id,
        status: e.status,
        notes: e.notes,
    })
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Recent merchants of expenses, most recent first (for the phone's lists).
pub(crate) fn recent_merchants(ctx: &Ctx<'_>, max: usize) -> CommandResult<Vec<String>> {
    let mut all = all_expenses(ctx)?;
    all.sort_by(|a, b| {
        b.date
            .cmp(&a.date)
            .then_with(|| b.created_at.cmp(&a.created_at))
    });
    let mut out: Vec<String> = Vec::new();
    for e in all {
        if let Some(m) = e.merchant
            && !out.iter().any(|x| x.eq_ignore_ascii_case(&m))
        {
            out.push(m);
        }
        if out.len() >= max {
            break;
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReceiptRef {
    receipt: String,
}

fn stored_file(ctx: &Ctx<'_>, r: &ReceiptDoc) -> CommandResult<(Vec<u8>, Option<Vec<u8>>)> {
    ctx.tx
        .receipt_file(&r.file)?
        .ok_or_else(|| CommandError::Integrity(format!("receipt file {} is missing", r.file)))
}

fn mime(kind: &str) -> &'static str {
    match kind {
        "jpeg" => "image/jpeg",
        "png" => "image/png",
        _ => "application/pdf",
    }
}

fn receipt_get(ctx: &Ctx<'_>, a: ReceiptRef) -> CommandResult<Value> {
    let r = load_receipt(ctx, &a.receipt)?;
    let (content, _) = stored_file(ctx, &r)?;
    let mut v = receipt_json(&r);
    set_key(&mut v, "mime", json!(mime(&r.kind)));
    set_key(
        &mut v,
        "content_base64",
        json!(base64::engine::general_purpose::STANDARD.encode(content)),
    );
    Ok(v)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ThumbArgs {
    receipt: String,
    /// Longer side in pixels (64–2400); the stored 256-pixel thumbnail when absent.
    #[serde(default)]
    max_side: Option<u32>,
}

fn receipt_thumbnail(ctx: &Ctx<'_>, a: ThumbArgs) -> CommandResult<Value> {
    let r = load_receipt(ctx, &a.receipt)?;
    let (content, thumb) = stored_file(ctx, &r)?;
    let png = if r.kind == "pdf" {
        None
    } else {
        match a.max_side {
            None | Some(256) => thumb,
            Some(side) => {
                let side = side.clamp(64, mk_import::receipt::STORED_SIDE);
                let img = mk_import::receipt::decode_stored(&content)
                    .map_err(|e| CommandError::Integrity(format!("stored receipt: {e}")))?;
                Some(
                    mk_import::receipt::fitted_png(&img, side)
                        .map_err(|e| CommandError::Integrity(format!("stored receipt: {e}")))?,
                )
            }
        }
    };
    Ok(json!({
        "id": r.id, "kind": r.kind, "file_name": r.file_name,
        "png_base64": png.map(|p| base64::engine::general_purpose::STANDARD.encode(p)),
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LinkArgs {
    receipt: String,
    #[serde(default)]
    expense: Option<String>,
    #[serde(default)]
    transaction: Option<String>,
}

fn receipt_link(ctx: &Ctx<'_>, a: LinkArgs) -> CommandResult<Value> {
    let mut r = load_receipt(ctx, &a.receipt)?;
    let (expense, transaction) = target(ctx, a.expense.as_deref(), a.transaction.as_deref())?;
    if expense.is_none() && transaction.is_none() {
        return Err(invalid(
            "give the expense or transaction to attach the receipt to",
        ));
    }
    r.expense = expense;
    r.transaction = transaction;
    save_receipt(ctx, &r)?;
    Ok(receipt_json(&r))
}

fn receipt_unlink(ctx: &Ctx<'_>, a: ReceiptRef) -> CommandResult<Value> {
    let mut r = load_receipt(ctx, &a.receipt)?;
    r.expense = None;
    r.transaction = None;
    save_receipt(ctx, &r)?;
    Ok(receipt_json(&r))
}

fn receipt_delete(ctx: &Ctx<'_>, a: ReceiptRef) -> CommandResult<Value> {
    let r = load_receipt(ctx, &a.receipt)?;
    // The stored file stays (written once), so Undo can bring the receipt back.
    ctx.save_doc(DocTable::Receipt, &r.id, None)?;
    Ok(json!({"deleted": r.id}))
}

fn receipt_inbox(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let mut list = receipts_in(ctx, INBOX)?;
    list.reverse();
    Ok(Value::Array(list.iter().map(receipt_json).collect()))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListReceiptsArgs {
    #[serde(default)]
    expense: Option<String>,
    #[serde(default)]
    transaction: Option<String>,
}

fn receipt_list(ctx: &Ctx<'_>, a: ListReceiptsArgs) -> CommandResult<Value> {
    let list = match (a.expense.as_deref(), a.transaction.as_deref()) {
        (Some(e), None) => receipts_of(ctx, &load_expense(ctx, e)?)?,
        (None, Some(t)) => receipts_in(ctx, &format!("txn:{}", txn_id(t)?))?,
        _ => return Err(invalid("give an expense or a transaction")),
    };
    Ok(Value::Array(list.iter().map(receipt_json).collect()))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FindArgs {
    /// The amount, either sign.
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    amount: Option<Decimal>,
    /// Around this date (± `days`, default 7).
    #[serde(default)]
    date: Option<Date>,
    #[serde(default)]
    days: Option<u32>,
    /// Part of the payee or memo.
    #[serde(default)]
    text: Option<String>,
    /// At most this many (default 50).
    #[serde(default)]
    limit: Option<usize>,
}

fn receipt_find_transactions(ctx: &Ctx<'_>, a: FindArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let days = i32::try_from(a.days.unwrap_or(7).min(366)).unwrap_or(7);
    let (from, to) = match a.date {
        Some(d) => (d.add_days(-days), d.add_days(days)),
        None => (None, None),
    };
    let text = a
        .text
        .as_deref()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty());
    let amount = a.amount.map(|x| x.abs());
    let counts = receipt_counts(ctx)?;
    let mut list = ctx.tx.transactions(&mk_store::TxnFilter {
        accounts: None,
        from,
        to,
        include_voided: false,
        ..Default::default()
    })?;
    list.reverse();
    let mut out = Vec::new();
    for t in list {
        if let Some(x) = amount
            && !t.splits.iter().any(|s| s.value.abs() == x)
        {
            continue;
        }
        if let Some(q) = &text {
            let hit = [&t.payee, &t.memo]
                .iter()
                .any(|f| f.as_deref().is_some_and(|v| v.to_lowercase().contains(q)));
            if !hit {
                continue;
            }
        }
        let total: Decimal = t
            .splits
            .iter()
            .filter(|s| s.value > Decimal::ZERO)
            .map(|s| s.value)
            .sum();
        let accounts: Vec<String> = t
            .splits
            .iter()
            .map(|s| chart.full_name(s.account))
            .collect();
        out.push(json!({
            "id": t.id, "date": t.date, "payee": t.payee, "memo": t.memo,
            "amount": money(total), "accounts": accounts,
            "receipts": counts.get(&t.id.to_string()).copied().unwrap_or(0),
        }));
        if out.len() >= a.limit.unwrap_or(50) {
            break;
        }
    }
    Ok(Value::Array(out))
}

// ----- report -----------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReportArgs {
    from: Date,
    to: Date,
    /// `all` (default) or `reimbursable`.
    #[serde(default)]
    filter: Option<String>,
    /// Only expenses paid with this account (or `personal`).
    #[serde(default)]
    paid_with: Option<String>,
    /// Only expenses in this category.
    #[serde(default)]
    category: Option<String>,
    /// Add each expense's receipt pictures after the table (default true).
    #[serde(default = "yes")]
    include_receipts: bool,
    /// Default "Expense report".
    #[serde(default)]
    title: Option<String>,
}

fn yes() -> bool {
    true
}

fn expense_report(ctx: &Ctx<'_>, a: ReportArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    if a.to < a.from {
        return Err(invalid("the period ends before it starts"));
    }
    let reimbursable = match a.filter.as_deref().map(str::trim) {
        None | Some("") | Some("all") => false,
        Some("reimbursable") => true,
        Some(o) => return Err(invalid(format!("filter is all or reimbursable, not {o}"))),
    };
    let mut list = select(
        ctx,
        &chart,
        &ListArgs {
            from: Some(a.from),
            to: Some(a.to),
            paid_with: if reimbursable {
                Some(PERSONAL.into())
            } else {
                a.paid_with.clone()
            },
            category: a.category.clone(),
            ..ListArgs::default()
        },
    )?;
    if reimbursable
        && let Some(p) = a.paid_with.as_deref().filter(|p| !p.trim().is_empty())
        && paid_with_ref(ctx, p)? != PERSONAL
    {
        list.clear();
    }
    list.sort_by(|x, y| {
        x.date
            .cmp(&y.date)
            .then_with(|| x.merchant.cmp(&y.merchant))
            .then_with(|| x.created_at.cmp(&y.created_at))
    });
    let title = crate::sales::text(a.title.clone(), 100, "title")?
        .unwrap_or_else(|| "Expense report".into());
    let mut rows = Vec::new();
    let mut by_cat: BTreeMap<String, Decimal> = BTreeMap::new();
    let mut total = Decimal::ZERO;
    let mut owed_all = Decimal::ZERO;
    let mut repaid = Decimal::ZERO;
    let mut sections = Vec::new();
    for e in &list {
        let amount = e.amount.unwrap_or_default();
        let cat = e
            .category
            .map(|c| chart.full_name(c))
            .unwrap_or_else(|| "(no category)".into());
        let paid = match &e.paid_with {
            Some(p) => paid_with_name(ctx, p)?,
            None => String::new(),
        };
        let receipts = receipts_of(ctx, e)?;
        rows.push(crate::expense_pdf::Row {
            date: e.date.to_string(),
            merchant: e.merchant.clone().unwrap_or_default(),
            category: cat.clone(),
            paid_with: if e.personal() {
                "Personal funds".into()
            } else {
                paid
            },
            purpose: e.purpose.clone().unwrap_or_default(),
            amount: crate::sales::group(&money(amount)),
            receipt: if receipts.is_empty() {
                crate::expense_pdf::Receipt::Missing
            } else {
                crate::expense_pdf::Receipt::Yes
            },
        });
        *by_cat.entry(cat).or_insert(Decimal::ZERO) += amount;
        total += amount;
        if e.personal() {
            owed_all += amount;
            if e.reimbursement.is_some() {
                repaid += amount;
            }
        }
        if a.include_receipts {
            let mut items = Vec::new();
            for r in &receipts {
                let (content, _) = stored_file(ctx, r)?;
                items.push(match r.kind.as_str() {
                    "jpeg" => crate::expense_pdf::Item::Jpeg {
                        width: r.width.unwrap_or(1),
                        height: r.height.unwrap_or(1),
                        data: content,
                    },
                    "png" => {
                        let img = mk_import::receipt::decode_stored(&content)
                            .map_err(|e| CommandError::Integrity(format!("stored receipt: {e}")))?;
                        crate::expense_pdf::Item::Raster {
                            width: img.width,
                            height: img.height,
                            flate_rgb: mk_import::image::deflate(&img.data),
                        }
                    }
                    _ => crate::expense_pdf::Item::Pdf {
                        file_name: r.file_name.clone(),
                    },
                });
            }
            sections.push(crate::expense_pdf::Section {
                heading: format!(
                    "{}  {}  {}",
                    e.date,
                    e.merchant.as_deref().unwrap_or_default(),
                    crate::sales::group(&money(amount))
                ),
                items,
            });
        }
    }
    // Posted reimbursable trips (mileage.md): like personal-funds expenses, with the
    // route, miles and rate in place of a receipt.
    let mut trips = crate::mileage::report_trips(ctx, a.from, a.to)?;
    let cat_filter = a
        .category
        .as_deref()
        .filter(|c| !c.trim().is_empty())
        .map(|c| chart.resolve(c.trim()).map(|x| x.id))
        .transpose()?;
    let paid_personal = match a.paid_with.as_deref().filter(|p| !p.trim().is_empty()) {
        Some(p) => paid_with_ref(ctx, p)? == PERSONAL,
        None => true,
    };
    if !paid_personal {
        trips.clear();
    }
    trips.retain(|t| cat_filter.is_none_or(|c| t.category == Some(c)));
    trips.sort_by(|x, y| {
        x.date
            .cmp(&y.date)
            .then_with(|| x.created_at.cmp(&y.created_at))
    });
    for t in &trips {
        let amount = t.reimburse_amount;
        let cat = t
            .category
            .map(|c| chart.full_name(c))
            .unwrap_or_else(|| "(no category)".into());
        let vehicle = crate::mileage::vehicle_name(ctx, t.vehicle.as_deref())?;
        let route = t.route().replace('→', "->");
        let purpose = match &t.purpose {
            Some(p) if !route.is_empty() => format!("{route} — {p}"),
            Some(p) => p.clone(),
            None => route.clone(),
        };
        rows.push(crate::expense_pdf::Row {
            date: t.date.to_string(),
            merchant: format!("Mileage — {vehicle}"),
            category: cat.clone(),
            paid_with: "Personal funds".into(),
            purpose,
            amount: crate::sales::group(&money(amount)),
            receipt: crate::expense_pdf::Receipt::NotApplicable,
        });
        *by_cat.entry(cat).or_insert(Decimal::ZERO) += amount;
        total += amount;
        owed_all += amount;
        if t.reimbursement.is_some() {
            repaid += amount;
        }
        if a.include_receipts {
            sections.push(crate::expense_pdf::Section {
                heading: format!(
                    "{}  Mileage — {vehicle}  {}",
                    t.date,
                    crate::sales::group(&money(amount))
                ),
                items: vec![crate::expense_pdf::Item::Text(format!(
                    "{}: {} mi × {}¢ = {}",
                    if route.is_empty() {
                        "Trip"
                    } else {
                        route.as_str()
                    },
                    t.miles,
                    crate::mileage::plain(t.reimburse_rate()),
                    crate::sales::group(&money(amount))
                ))],
            });
        }
    }
    if !trips.is_empty() {
        rows.sort_by(|x, y| x.date.cmp(&y.date));
        sections.sort_by(|x, y| x.heading.get(..10).cmp(&y.heading.get(..10)));
    }
    let info = ctx.tx.info()?;
    let report = crate::expense_pdf::Report {
        book: info.name,
        title: title.clone(),
        period: format!("{} to {}", a.from, a.to),
        generated: today_utc()?.to_string(),
        rows,
        totals: by_cat
            .into_iter()
            .map(|(k, v)| (k, crate::sales::group(&money(v))))
            .collect(),
        total: crate::sales::group(&money(total)),
        reimbursable: crate::sales::group(&money(owed_all)),
        reimbursed: crate::sales::group(&money(repaid)),
        owed: crate::sales::group(&money(owed_all - repaid)),
        sections,
    };
    let (bytes, replaced) = crate::expense_pdf::render(&report);
    let file = format!("{title} {} to {}", a.from, a.to);
    let mut out = crate::sales::pdf_answer(&file, bytes, &replaced);
    set_key(&mut out, "expenses", json!(list.len()));
    set_key(&mut out, "trips", json!(trips.len()));
    set_key(
        &mut out,
        "trip_ids",
        json!(trips.iter().map(|t| t.id.clone()).collect::<Vec<_>>()),
    );
    set_key(&mut out, "total", json!(money(total)));
    // The included expenses, for `expense.mark_reported` once the PDF is saved.
    set_key(
        &mut out,
        "expense_ids",
        json!(list.iter().map(|e| e.id.clone()).collect::<Vec<_>>()),
    );
    Ok(out)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MarkReportedArgs {
    /// The expenses to flag as reported (usually a saved report's `expense_ids`).
    #[serde(default)]
    expenses: Vec<String>,
    /// The trips to flag as reported (a saved report's `trip_ids`).
    #[serde(default)]
    trips: Vec<String>,
}

/// Set the `reported` flag (expenses.md revision 2). Expenses already flagged are
/// left alone; when nothing changes no audit entry is written.
fn expense_mark_reported(ctx: &Ctx<'_>, a: MarkReportedArgs) -> CommandResult<Value> {
    ctx.quiet_noop.set(true);
    if a.expenses.is_empty() && a.trips.is_empty() {
        return Err(invalid("Choose at least one expense."));
    }
    let mut seen = HashSet::new();
    let mut marked = 0usize;
    for r in &a.expenses {
        let mut e = load_expense(ctx, r)?;
        if !seen.insert(e.id.clone()) || e.reported {
            continue;
        }
        e.reported = true;
        save_expense(ctx, &e)?;
        marked += 1;
    }
    let mut seen_trips = HashSet::new();
    for r in &a.trips {
        let mut t = crate::mileage::load_trip(ctx, r)?;
        if !seen_trips.insert(t.id.clone()) || t.reported {
            continue;
        }
        t.reported = true;
        crate::mileage::save_trip(ctx, &t)?;
        marked += 1;
    }
    Ok(json!({"marked_reported": marked, "expenses": seen.len(), "trips": seen_trips.len()}))
}

/// Insert a key into a JSON object (no-op for other values).
pub(crate) fn set_key(v: &mut Value, key: &str, value: Value) {
    if let Some(o) = v.as_object_mut() {
        o.insert(key.to_owned(), value);
    }
}
