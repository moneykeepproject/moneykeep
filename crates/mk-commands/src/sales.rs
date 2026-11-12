//! Business sales (business-sales.md Parts B–G): parties, items, sales tax, invoices,
//! estimates, credit memos, payments, deposits, refunds and sales tax payments.
//!
//! Documents are `sales_doc` records (grouped by kind); payments, refunds and tax
//! payments are `payment` records; deposits are `deposit` records. Every posting is
//! an ordinary transaction whose id the record keeps; applications of payments and
//! credits to invoices live on the payment or credit memo and change only balances.

use std::collections::BTreeMap;

use mk_core::{
    AccountId, AccountSubtype, AccountType, CommodityCode, Date, Decimal, Reconcile,
    RoundingStrategy, Split, SplitTags, Transaction, TransactionId, TxnStatus,
};
use mk_store::{Doc, DocTable, now_rfc3339};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::module_settings::{self as ms, check_terms, terms_label};
use crate::txns::validate_change;
use crate::{CommandError, CommandResult, Ctx, Registry, is_module_enabled};

pub(crate) const INVOICING: &str = "invoicing";
pub(crate) const ITEMS: &str = "items";
pub(crate) const SALES_TAX: &str = "sales-tax";
pub(crate) const ESTIMATES: &str = "estimates";
/// Parties serve customers (invoicing) and vendors (bills).
pub(crate) const PARTIES: &str = "invoicing|bills";

pub(crate) fn register(r: &mut Registry) {
    let inv = INVOICING;
    let pm = PARTIES;
    r.add_in(
        pm,
        "party.create",
        "Create a customer or vendor (party)",
        true,
        party_create,
    );
    r.add_in(pm, "party.update", "Change a party", true, party_update);
    r.add_in(pm, "party.get", "Get a party", false, party_get);
    r.add_in(pm, "party.list", "List parties", false, party_list);
    r.add_in(
        pm,
        "party.set_active",
        "Make a party active or inactive",
        true,
        party_set_active,
    );
    r.add_in(
        pm,
        "party.delete",
        "Delete a party that was never used",
        true,
        party_delete,
    );
    r.add_in(
        ITEMS,
        "item.create",
        "Create a product or service item",
        true,
        item_create,
    );
    r.add_in(ITEMS, "item.update", "Change an item", true, item_update);
    r.add_in(ITEMS, "item.get", "Get an item", false, item_get);
    r.add_in(ITEMS, "item.list", "List items", false, item_list);
    r.add_in(
        ITEMS,
        "item.set_active",
        "Make an item active or inactive",
        true,
        item_set_active,
    );
    r.add_in(
        ITEMS,
        "item.delete",
        "Delete an item that was never used",
        true,
        item_delete,
    );
    r.add_in(
        SALES_TAX,
        "tax_rate.create",
        "Create a sales tax rate or group",
        true,
        tax_create,
    );
    r.add_in(
        SALES_TAX,
        "tax_rate.update",
        "Change a sales tax rate or group",
        true,
        tax_update,
    );
    r.add_in(
        SALES_TAX,
        "tax_rate.list",
        "List sales tax rates and groups",
        false,
        tax_list,
    );
    r.add_in(
        SALES_TAX,
        "tax_rate.delete",
        "Delete a tax rate or group never used",
        true,
        tax_delete,
    );
    r.add_in(
        SALES_TAX,
        "sales_tax.pay",
        "Pay sales tax to the agency",
        true,
        tax_pay,
    );
    r.add_in(
        inv,
        "invoice.create",
        "Create an invoice (draft or open)",
        true,
        invoice_create,
    );
    r.add_in(
        inv,
        "invoice.update",
        "Change an invoice",
        true,
        invoice_update,
    );
    r.add_in(
        inv,
        "invoice.finalize",
        "Open a draft invoice (posts it)",
        true,
        invoice_finalize,
    );
    r.add_in(inv, "invoice.void", "Void an invoice", true, invoice_void);
    r.add_in(
        inv,
        "invoice.delete",
        "Delete a draft, or an open invoice without payments",
        true,
        invoice_delete,
    );
    r.add_in(
        inv,
        "invoice.get",
        "Get an invoice with its balance",
        false,
        invoice_get,
    );
    r.add_in(inv, "invoice.list", "List invoices", false, invoice_list);
    r.add_in(
        inv,
        "invoice.duplicate",
        "Copy an invoice as a new draft",
        true,
        invoice_duplicate,
    );
    r.add_in(
        inv,
        "invoice.preview",
        "Totals and tax for unsaved invoice data",
        false,
        invoice_preview,
    );
    r.add_in(
        inv,
        "invoice.render_pdf",
        "The invoice as a PDF",
        false,
        invoice_pdf,
    );
    r.add_in(
        inv,
        "payment.receive",
        "Record a payment from a customer",
        true,
        payment_receive,
    );
    r.add_in(
        inv,
        "payment.apply",
        "Apply a payment's credit to invoices",
        true,
        payment_apply,
    );
    r.add_in(
        inv,
        "payment.unapply",
        "Remove a payment's application to an invoice",
        true,
        payment_unapply,
    );
    r.add_in(
        inv,
        "payment.delete",
        "Delete a payment (not deposited)",
        true,
        payment_delete,
    );
    r.add_in(inv, "payment.get", "Get a payment", false, payment_get);
    r.add_in(
        inv,
        "payment.open_invoices",
        "A customer's open invoices with the amounts a payment would apply, oldest first",
        false,
        payment_open_invoices,
    );
    r.add_in(
        inv,
        "deposit.preview",
        "The total of a deposit of the chosen payments",
        false,
        deposit_preview,
    );
    r.add_in(
        inv,
        "payment.list",
        "List payments received",
        false,
        payment_list,
    );
    r.add_in(
        inv,
        "deposit.create",
        "Deposit received payments into a bank account",
        true,
        deposit_create,
    );
    r.add_in(
        inv,
        "deposit.delete",
        "Undo a deposit (payments return to Payments to Deposit)",
        true,
        deposit_delete,
    );
    r.add_in(inv, "deposit.list", "List deposits", false, deposit_list);
    r.add_in(
        inv,
        "credit_memo.create",
        "Create a credit memo",
        true,
        credit_create,
    );
    r.add_in(
        inv,
        "credit_memo.apply",
        "Apply a credit memo to invoices",
        true,
        credit_apply,
    );
    r.add_in(
        inv,
        "credit_memo.unapply",
        "Remove a credit memo's application",
        true,
        credit_unapply,
    );
    r.add_in(
        inv,
        "credit_memo.delete",
        "Delete an unapplied credit memo",
        true,
        credit_delete,
    );
    r.add_in(
        inv,
        "credit_memo.get",
        "Get a credit memo",
        false,
        credit_get,
    );
    r.add_in(
        inv,
        "credit_memo.render_pdf",
        "The credit memo as a PDF",
        false,
        credit_pdf,
    );
    r.add_in(
        inv,
        "credit_memo.list",
        "List credit memos",
        false,
        credit_list,
    );
    r.add_in(
        inv,
        "customer.refund",
        "Refund a customer's unapplied credit",
        true,
        refund,
    );
    r.add_in(
        inv,
        "customer.summary",
        "A customer's open invoices, payments and credit",
        false,
        customer_summary,
    );
    r.add_in(
        ESTIMATES,
        "estimate.create",
        "Create an estimate",
        true,
        estimate_create,
    );
    r.add_in(
        ESTIMATES,
        "estimate.update",
        "Change an estimate",
        true,
        estimate_update,
    );
    r.add_in(
        ESTIMATES,
        "estimate.set_status",
        "Mark an estimate draft, sent, accepted or declined",
        true,
        estimate_set_status,
    );
    r.add_in(
        ESTIMATES,
        "estimate.convert",
        "Turn an estimate into an invoice",
        true,
        estimate_convert,
    );
    r.add_in(
        ESTIMATES,
        "estimate.delete",
        "Delete an estimate",
        true,
        estimate_delete,
    );
    r.add_in(
        ESTIMATES,
        "estimate.get",
        "Get an estimate",
        false,
        estimate_get,
    );
    r.add_in(
        ESTIMATES,
        "estimate.list",
        "List estimates",
        false,
        estimate_list,
    );
    r.add_in(
        ESTIMATES,
        "estimate.duplicate",
        "Copy an estimate as a new draft",
        true,
        estimate_duplicate,
    );
    r.add_in(
        ESTIMATES,
        "estimate.render_pdf",
        "The estimate as a PDF",
        false,
        estimate_pdf,
    );
}

// ----- helpers ------------------------------------------------------------------------

pub(crate) fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

pub(crate) fn rejected(m: impl Into<String>) -> CommandError {
    CommandError::Rejected(m.into())
}

pub(crate) fn bad_doc(e: impl std::fmt::Display) -> CommandError {
    CommandError::Integrity(format!("bad stored record: {e}"))
}

pub(crate) fn round2(v: Decimal) -> Decimal {
    v.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

pub(crate) fn add(a: Decimal, b: Decimal) -> CommandResult<Decimal> {
    a.checked_add(b)
        .ok_or_else(|| rejected("arithmetic overflow"))
}

pub(crate) fn sub(a: Decimal, b: Decimal) -> CommandResult<Decimal> {
    a.checked_sub(b)
        .ok_or_else(|| rejected("arithmetic overflow"))
}

pub(crate) fn mul(a: Decimal, b: Decimal) -> CommandResult<Decimal> {
    a.checked_mul(b)
        .ok_or_else(|| rejected("arithmetic overflow"))
}

pub(crate) fn money(v: Decimal) -> String {
    mk_core::currency::format_amount(round2(v), Some(2))
}

pub(crate) fn new_id() -> String {
    mk_core::Id::<()>::new().to_string()
}

pub(crate) fn text(v: Option<String>, max: usize, what: &str) -> CommandResult<Option<String>> {
    let Some(s) = v else { return Ok(None) };
    let c = mk_core::text::clean(&s, usize::MAX).text;
    if c.chars().count() > max {
        return Err(invalid(format!("{what}: at most {max} characters")));
    }
    Ok(Some(c).filter(|x| !x.is_empty()))
}

pub(crate) fn lines_text(v: Option<Vec<String>>, what: &str) -> CommandResult<Vec<String>> {
    let v = v.unwrap_or_default();
    if v.len() > 5 {
        return Err(invalid(format!("{what}: at most 5 lines")));
    }
    let mut out = Vec::new();
    for l in v {
        if let Some(t) = text(Some(l), 100, what)? {
            out.push(t);
        }
    }
    Ok(out)
}

pub(crate) fn decimals(v: Decimal) -> u32 {
    v.normalize().scale()
}

pub(crate) fn date_ord(d: Date) -> i64 {
    i64::from(d.year()) * 10_000 + i64::from(d.month()) * 100 + i64::from(d.day())
}

pub(crate) fn load<T: serde::de::DeserializeOwned>(
    ctx: &Ctx<'_>,
    t: DocTable,
    grp: Option<&str>,
) -> CommandResult<Vec<T>> {
    ctx.tx
        .docs(t, grp)?
        .into_iter()
        .map(|d| serde_json::from_value(d.json).map_err(bad_doc))
        .collect()
}

pub(crate) fn save<T: Serialize>(
    ctx: &Ctx<'_>,
    t: DocTable,
    id: &str,
    grp: Option<&str>,
    ord: i64,
    v: &T,
) -> CommandResult<()> {
    ctx.save_doc(
        t,
        id,
        Some(&Doc {
            id: id.to_owned(),
            grp: grp.map(str::to_owned),
            ord,
            json: serde_json::to_value(v)?,
        }),
    )
}

pub(crate) fn base_currency(ctx: &Ctx<'_>) -> CommandResult<CommodityCode> {
    Ok(ctx.tx.info()?.base_currency.parse()?)
}

pub(crate) fn cash(account: AccountId, value: Decimal, memo: Option<String>) -> Split {
    Split {
        account,
        value,
        quantity: value,
        memo,
        reconcile: Reconcile::New,
        fingerprint: None,
        tags: Default::default(),
    }
}

/// Post (or rewrite) a transaction; `existing` keeps the id and reconcile flags.
pub(crate) fn post_txn(
    ctx: &Ctx<'_>,
    existing: Option<TransactionId>,
    date: Date,
    payee: &str,
    memo: Option<String>,
    splits: Vec<Split>,
) -> CommandResult<TransactionId> {
    post_txn_numbered(ctx, existing, date, payee, memo, splits, None)
}

/// Like [`post_txn`], setting the transaction's number (a check number) when given.
pub(crate) fn post_txn_numbered(
    ctx: &Ctx<'_>,
    existing: Option<TransactionId>,
    date: Date,
    payee: &str,
    memo: Option<String>,
    splits: Vec<Split>,
    number: Option<String>,
) -> CommandResult<TransactionId> {
    let mut splits: Vec<Split> = splits.into_iter().filter(|s| !s.value.is_zero()).collect();
    if splits.is_empty() {
        return Err(rejected("nothing to post: every amount is zero"));
    }
    mk_ledger::sort_splits(&mut splits);
    let old = match existing {
        Some(id) => ctx.tx.transaction(id)?,
        None => None,
    };
    let mut t = Transaction {
        id: existing.unwrap_or_default(),
        date,
        currency: base_currency(ctx)?,
        payee: Some(payee.to_owned()),
        number: None,
        memo,
        status: TxnStatus::Posted,
        void_reason: None,
        reverses: None,
        opening: false,
        entered_at: now_rfc3339()?,
        splits,
    };
    if let Some(o) = &old {
        t.entered_at = o.entered_at.clone();
        t.number = o.number.clone();
        mk_ledger::inherit_reconcile(&o.splits, &mut t.splits);
    }
    if number.is_some() {
        t.number = number;
    }
    validate_change(ctx, old.as_ref(), Some(&t))?;
    ctx.save_txn(old.as_ref(), &t)?;
    Ok(t.id)
}

pub(crate) fn remove_txn(ctx: &Ctx<'_>, id: Option<TransactionId>) -> CommandResult<()> {
    if let Some(id) = id
        && let Some(t) = ctx.tx.transaction(id)?
    {
        validate_change(ctx, Some(&t), None)?;
        ctx.remove_txn(&t)?;
    }
    Ok(())
}

pub(crate) fn module_on(ctx: &Ctx<'_>, m: &str) -> CommandResult<bool> {
    is_module_enabled(ctx.tx, m)
}

fn receivable(ctx: &Ctx<'_>) -> CommandResult<AccountId> {
    crate::module_settings::account_or_create_role(
        ctx,
        "invoicing.receivable_account",
        Some(crate::chart::roles::RECEIVABLE),
        "Accounts Receivable",
        AccountType::Asset,
        AccountSubtype::Receivable,
    )
}

pub(crate) fn clearing(ctx: &Ctx<'_>) -> CommandResult<AccountId> {
    let find = |a: &mk_core::Account| {
        a.parent.is_none()
            && a.subtype == AccountSubtype::Clearing
            && a.name.eq_ignore_ascii_case("Payments to Deposit")
    };
    let create = |ctx: &Ctx<'_>| -> CommandResult<AccountId> {
        let v = crate::accounts::create_json(
            ctx,
            json!({"name": "Payments to Deposit", "type": "asset", "subtype": "clearing"}),
        )?;
        v.get("id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| CommandError::Integrity("account without id".into()))
    };
    crate::chart::roles::find_or_create(ctx, crate::chart::roles::DEPOSIT_CLEARING, &find, &create)
}

fn liability(ctx: &Ctx<'_>) -> CommandResult<AccountId> {
    crate::module_settings::account_or_create_role(
        ctx,
        "sales-tax.liability_account",
        Some(crate::chart::roles::SALES_TAX),
        "Sales Tax Payable",
        AccountType::Liability,
        AccountSubtype::TaxPayable,
    )
}

// ----- parties ------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Party {
    pub id: String,
    pub name: String,
    pub roles: Vec<String>,
    #[serde(default)]
    pub company: Option<String>,
    #[serde(default)]
    pub contact: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub billing_address: Vec<String>,
    #[serde(default)]
    pub shipping_address: Vec<String>,
    #[serde(default)]
    pub terms: Option<Value>,
    #[serde(default)]
    pub tax_exempt: bool,
    #[serde(default)]
    pub exemption_note: Option<String>,
    #[serde(default)]
    pub tax_rate: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    pub active: bool,
    pub created_at: String,
    // Vendor fields (business-purchases.md).
    #[serde(default)]
    pub vendor_terms: Option<Value>,
    #[serde(default)]
    pub default_expense_account: Option<AccountId>,
    #[serde(default)]
    pub account_number_at_vendor: Option<String>,
    #[serde(default)]
    pub tracks_1099: bool,
    /// `nec` or `misc`.
    #[serde(default = "default_form")]
    pub form_1099: String,
    /// MISC box: rents, royalties, other_income, medical, attorney_gross_proceeds.
    #[serde(default)]
    pub misc_box: Option<String>,
    /// Whether the vendor's taxpayer id is on file (the number itself is never stored).
    #[serde(default)]
    pub tin_on_file: bool,
}

fn default_form() -> String {
    "nec".into()
}

impl Party {
    pub(crate) fn is(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }
}

pub(crate) fn parties(ctx: &Ctx<'_>) -> CommandResult<Vec<Party>> {
    let mut v: Vec<Party> = load(ctx, DocTable::Party, None)?;
    v.sort_by_key(|p| p.name.to_lowercase());
    Ok(v)
}

pub(crate) fn find_party(ctx: &Ctx<'_>, r: &str) -> CommandResult<Party> {
    let r = r.trim();
    parties(ctx)?
        .into_iter()
        .find(|p| p.id == r || p.name.eq_ignore_ascii_case(r))
        .ok_or_else(|| CommandError::NotFound(format!("party {r:?}")))
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PartyArgs {
    /// Party id or name (update only).
    #[serde(default)]
    party: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// `customer` (default) and/or `vendor`.
    #[serde(default)]
    roles: Option<Vec<String>>,
    #[serde(default)]
    company: Option<String>,
    #[serde(default)]
    contact: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    phone: Option<String>,
    #[serde(default)]
    billing_address: Option<Vec<String>>,
    #[serde(default)]
    shipping_address: Option<Vec<String>>,
    /// `{"type": "due_on_receipt"}` or `{"type": "net", "days": 30}`.
    #[serde(default)]
    terms: Option<Value>,
    #[serde(default)]
    tax_exempt: Option<bool>,
    #[serde(default)]
    exemption_note: Option<String>,
    /// Tax rate or group (id or name).
    #[serde(default)]
    tax_rate: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    /// Default terms for bills.
    #[serde(default)]
    vendor_terms: Option<Value>,
    #[serde(default)]
    default_expense_account: Option<String>,
    #[serde(default)]
    account_number_at_vendor: Option<String>,
    #[serde(default)]
    tracks_1099: Option<bool>,
    /// `nec` or `misc`.
    #[serde(default)]
    form_1099: Option<String>,
    /// For MISC: rents, royalties, other_income, medical or attorney_gross_proceeds.
    #[serde(default)]
    misc_box: Option<String>,
    #[serde(default)]
    tin_on_file: Option<bool>,
}

fn apply_party(ctx: &Ctx<'_>, p: &mut Party, a: PartyArgs) -> CommandResult<()> {
    if let Some(n) = a.name {
        let n = text(Some(n), 100, "name")?.ok_or_else(|| invalid("name: 1-100 characters"))?;
        if parties(ctx)?
            .iter()
            .any(|x| x.id != p.id && x.name.eq_ignore_ascii_case(&n))
        {
            return Err(rejected(format!("a party named {n:?} already exists")));
        }
        p.name = n;
    }
    if let Some(r) = a.roles {
        let mut roles = Vec::new();
        for x in r {
            if !matches!(x.as_str(), "customer" | "vendor") {
                return Err(invalid("roles are customer and vendor"));
            }
            if !roles.contains(&x) {
                roles.push(x);
            }
        }
        if roles.is_empty() {
            return Err(invalid("give at least one role"));
        }
        p.roles = roles;
    }
    if a.company.is_some() {
        p.company = text(a.company, 100, "company")?;
    }
    if a.contact.is_some() {
        p.contact = text(a.contact, 100, "contact")?;
    }
    if a.email.is_some() {
        p.email = text(a.email, 254, "email")?;
        if let Some(e) = &p.email
            && e.matches('@').count() != 1
        {
            return Err(invalid("email: must contain one @"));
        }
    }
    if a.phone.is_some() {
        p.phone = text(a.phone, 40, "phone")?;
    }
    if a.billing_address.is_some() {
        p.billing_address = lines_text(a.billing_address, "billing address")?;
    }
    if a.shipping_address.is_some() {
        p.shipping_address = lines_text(a.shipping_address, "shipping address")?;
    }
    if let Some(t) = a.terms {
        p.terms = if t.is_null() {
            None
        } else {
            Some(check_terms(&t)?)
        };
    }
    if let Some(x) = a.tax_exempt {
        p.tax_exempt = x;
    }
    if a.exemption_note.is_some() {
        p.exemption_note = text(a.exemption_note, 200, "exemption note")?;
    }
    if let Some(t) = a.tax_rate {
        p.tax_rate = if t.trim().is_empty() {
            None
        } else {
            Some(find_tax(ctx, &t)?.id)
        };
    }
    if a.notes.is_some() {
        p.notes = text(a.notes, 4000, "notes")?;
    }
    if let Some(t) = a.vendor_terms {
        p.vendor_terms = if t.is_null() {
            None
        } else {
            Some(check_terms(&t)?)
        };
    }
    if let Some(acc) = a.default_expense_account {
        p.default_expense_account = if acc.trim().is_empty() {
            None
        } else {
            Some(Chart::load(ctx)?.resolve(&acc)?.id)
        };
    }
    if a.account_number_at_vendor.is_some() {
        p.account_number_at_vendor = text(a.account_number_at_vendor, 40, "account number")?;
    }
    if let Some(x) = a.tracks_1099 {
        p.tracks_1099 = x;
    }
    if let Some(f) = a.form_1099 {
        if !matches!(f.as_str(), "nec" | "misc") {
            return Err(invalid("form_1099 is nec or misc"));
        }
        p.form_1099 = f;
    }
    if let Some(b) = a.misc_box {
        p.misc_box = if b.trim().is_empty() {
            None
        } else if matches!(
            b.as_str(),
            "rents" | "royalties" | "other_income" | "medical" | "attorney_gross_proceeds"
        ) {
            Some(b)
        } else {
            return Err(invalid(
                "misc_box is rents, royalties, other_income, medical or attorney_gross_proceeds",
            ));
        };
    }
    if p.form_1099 == "misc" && p.misc_box.is_none() {
        p.misc_box = Some("other_income".into());
    }
    if let Some(x) = a.tin_on_file {
        p.tin_on_file = x;
    }
    Ok(())
}

pub(crate) fn save_party(ctx: &Ctx<'_>, p: &Party) -> CommandResult<()> {
    save(ctx, DocTable::Party, &p.id, None, 0, p)
}

pub(crate) fn party_json(ctx: &Ctx<'_>, p: &Party) -> CommandResult<Value> {
    let mut v = serde_json::to_value(p)?;
    let terms = effective_terms(ctx, p)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("terms_label".into(), json!(terms_label(&terms)));
    }
    Ok(v)
}

fn party_create(ctx: &Ctx<'_>, mut a: PartyArgs) -> CommandResult<Value> {
    if a.name.is_none() {
        return Err(invalid("give the name"));
    }
    if a.roles.is_none() {
        let role = if module_on(ctx, INVOICING)? {
            "customer"
        } else {
            "vendor"
        };
        a.roles = Some(vec![role.into()]);
    }
    let mut p = Party {
        id: new_id(),
        name: String::new(),
        roles: Vec::new(),
        company: None,
        contact: None,
        email: None,
        phone: None,
        billing_address: Vec::new(),
        shipping_address: Vec::new(),
        terms: None,
        tax_exempt: false,
        exemption_note: None,
        tax_rate: None,
        notes: None,
        active: true,
        created_at: now_rfc3339()?,
        vendor_terms: None,
        default_expense_account: None,
        account_number_at_vendor: None,
        tracks_1099: false,
        form_1099: default_form(),
        misc_box: None,
        tin_on_file: false,
    };
    apply_party(ctx, &mut p, a)?;
    save_party(ctx, &p)?;
    party_json(ctx, &p)
}

/// `party.create` from JSON arguments (IIF lists).
pub(crate) fn party_create_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    party_create(ctx, crate::parse_args(args)?)
}

/// The vendor named `name`: an existing vendor, an existing party given the vendor
/// role, or a new vendor (Make a bill on a subscription). Returns the party id.
pub(crate) fn ensure_vendor(ctx: &Ctx<'_>, name: &str) -> CommandResult<String> {
    match find_party(ctx, name) {
        Ok(p) if p.is("vendor") => Ok(p.id),
        Ok(mut p) => {
            let mut roles = p.roles.clone();
            roles.push("vendor".into());
            apply_party(
                ctx,
                &mut p,
                PartyArgs {
                    roles: Some(roles),
                    ..PartyArgs::default()
                },
            )?;
            save_party(ctx, &p)?;
            Ok(p.id)
        }
        Err(CommandError::NotFound(_)) => {
            let v = party_create(
                ctx,
                PartyArgs {
                    name: Some(name.to_owned()),
                    roles: Some(vec!["vendor".into()]),
                    ..PartyArgs::default()
                },
            )?;
            v.get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| CommandError::Integrity("party without id".into()))
        }
        Err(e) => Err(e),
    }
}

fn party_update(ctx: &Ctx<'_>, mut a: PartyArgs) -> CommandResult<Value> {
    let r = a.party.take().ok_or_else(|| invalid("give the party"))?;
    let mut p = find_party(ctx, &r)?;
    apply_party(ctx, &mut p, a)?;
    save_party(ctx, &p)?;
    party_json(ctx, &p)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PartyRef {
    party: String,
}

fn party_get(ctx: &Ctx<'_>, a: PartyRef) -> CommandResult<Value> {
    party_json(ctx, &find_party(ctx, &a.party)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PartyListArgs {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    active: Option<bool>,
    #[serde(default)]
    search: Option<String>,
}

fn party_list(ctx: &Ctx<'_>, a: PartyListArgs) -> CommandResult<Value> {
    let q = a.search.unwrap_or_default().to_lowercase();
    let mut out = Vec::new();
    for p in parties(ctx)? {
        if a.role.as_ref().is_some_and(|r| !p.roles.contains(r)) {
            continue;
        }
        if a.active.is_some_and(|x| x != p.active) {
            continue;
        }
        if !q.is_empty()
            && !p.name.to_lowercase().contains(&q)
            && !p
                .company
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains(&q)
        {
            continue;
        }
        out.push(party_json(ctx, &p)?);
    }
    Ok(Value::Array(out))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ActiveArgs {
    #[serde(alias = "item")]
    party: String,
    active: bool,
}

fn party_set_active(ctx: &Ctx<'_>, a: ActiveArgs) -> CommandResult<Value> {
    let mut p = find_party(ctx, &a.party)?;
    p.active = a.active;
    save_party(ctx, &p)?;
    party_json(ctx, &p)
}

pub(crate) fn party_used(ctx: &Ctx<'_>, id: &str) -> CommandResult<bool> {
    let docs: Vec<SalesDoc> = load(ctx, DocTable::SalesDoc, None)?;
    let pays: Vec<Value> = load(ctx, DocTable::Payment, None)?;
    let bills: Vec<Value> = load(ctx, DocTable::BillDoc, None)?;
    let bill_pays: Vec<Value> = load(ctx, DocTable::BillPayment, None)?;
    let vendor_of = |v: &Value| v.get("vendor").and_then(Value::as_str) == Some(id);
    Ok(docs.iter().any(|d| d.customer == id)
        || pays
            .iter()
            .any(|p| p.get("customer").and_then(Value::as_str) == Some(id))
        || bills.iter().any(vendor_of)
        || bill_pays.iter().any(vendor_of))
}

fn party_delete(ctx: &Ctx<'_>, a: PartyRef) -> CommandResult<Value> {
    let p = find_party(ctx, &a.party)?;
    if party_used(ctx, &p.id)? {
        return Err(rejected(format!(
            "{} has documents or payments; make it inactive instead",
            p.name
        )));
    }
    ctx.save_doc(DocTable::Party, &p.id, None)?;
    Ok(json!({"deleted": p.id}))
}

pub(crate) fn effective_terms(ctx: &Ctx<'_>, p: &Party) -> CommandResult<Value> {
    match &p.terms {
        Some(t) => Ok(t.clone()),
        None => Ok(if module_on(ctx, INVOICING)? {
            ms::get(ctx, "invoicing.default_terms")?
        } else {
            json!({"type": "net", "days": 30})
        }),
    }
}

pub(crate) fn due_from(terms: &Value, date: Date) -> Date {
    match terms.get("type").and_then(Value::as_str) {
        Some("net") => {
            let days = terms.get("days").and_then(Value::as_i64).unwrap_or(30);
            date.add_days(i32::try_from(days).unwrap_or(30))
                .unwrap_or(date)
        }
        _ => date,
    }
}

// ----- items --------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Item {
    pub id: String,
    pub name: String,
    /// `service`, `product`, `other_charge` or `discount`.
    pub kind: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub sku: Option<String>,
    pub price: Decimal,
    /// Discount items: `price` is a percentage.
    #[serde(default)]
    pub percent: bool,
    pub income_account: AccountId,
    pub taxable: bool,
    pub active: bool,
    /// Purchase side (bills, purchase orders): the account debited when bought.
    #[serde(default)]
    pub expense_account: Option<AccountId>,
    /// Default purchase cost.
    #[serde(default)]
    pub cost: Option<Decimal>,
}

pub(crate) fn items(ctx: &Ctx<'_>) -> CommandResult<Vec<Item>> {
    let mut v: Vec<Item> = load(ctx, DocTable::Item, None)?;
    v.sort_by_key(|i| i.name.to_lowercase());
    Ok(v)
}

pub(crate) fn find_item(ctx: &Ctx<'_>, r: &str) -> CommandResult<Item> {
    let r = r.trim();
    items(ctx)?
        .into_iter()
        .find(|i| {
            i.id == r
                || i.name.eq_ignore_ascii_case(r)
                || i.sku.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(r))
        })
        .ok_or_else(|| CommandError::NotFound(format!("item {r:?}")))
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ItemArgs {
    #[serde(default)]
    item: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    sku: Option<String>,
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    price: Option<Decimal>,
    #[serde(default)]
    percent: Option<bool>,
    #[serde(default)]
    income_account: Option<String>,
    #[serde(default)]
    taxable: Option<bool>,
    /// Account debited when the item is bought (expense or asset); "" clears it.
    #[serde(default)]
    expense_account: Option<String>,
    /// Default purchase cost.
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    cost: Option<Decimal>,
}

fn apply_item(ctx: &Ctx<'_>, it: &mut Item, a: ItemArgs) -> CommandResult<()> {
    let all = items(ctx)?;
    if let Some(n) = a.name {
        let n = text(Some(n), 100, "name")?.ok_or_else(|| invalid("name: 1-100 characters"))?;
        if all
            .iter()
            .any(|x| x.id != it.id && x.name.eq_ignore_ascii_case(&n))
        {
            return Err(rejected(format!("an item named {n:?} already exists")));
        }
        it.name = n;
    }
    if let Some(k) = a.kind {
        if !matches!(
            k.as_str(),
            "service" | "product" | "other_charge" | "discount"
        ) {
            return Err(invalid(
                "kind is service, product, other_charge or discount",
            ));
        }
        it.kind = k;
    }
    if a.description.is_some() {
        it.description = text(a.description, 1000, "description")?;
    }
    if a.sku.is_some() {
        it.sku = text(a.sku, 40, "sku")?;
        if let Some(s) = &it.sku
            && all.iter().any(|x| {
                x.id != it.id && x.sku.as_deref().is_some_and(|y| y.eq_ignore_ascii_case(s))
            })
        {
            return Err(rejected(format!("SKU {s:?} is already used")));
        }
    }
    if let Some(p) = a.percent {
        it.percent = p && it.kind == "discount";
    }
    if let Some(p) = a.price {
        if p.is_sign_negative() {
            return Err(invalid("price cannot be negative"));
        }
        if decimals(p) > 4 {
            return Err(invalid("price: at most 4 decimals"));
        }
        it.price = p.normalize();
    }
    if it.percent && it.price > Decimal::ONE_HUNDRED {
        return Err(invalid("a percentage discount is 0-100"));
    }
    if let Some(acc) = a.income_account {
        let chart = Chart::load(ctx)?;
        let x = chart.resolve(&acc)?;
        let ok = match it.kind.as_str() {
            "discount" => matches!(x.account_type, AccountType::Income | AccountType::Expense),
            _ => x.account_type == AccountType::Income,
        };
        if !ok || x.placeholder {
            return Err(invalid(format!(
                "{} is not an income account items can post to",
                chart.full_name(x.id)
            )));
        }
        it.income_account = x.id;
    }
    if let Some(t) = a.taxable {
        it.taxable = t;
    }
    if let Some(acc) = a.expense_account {
        if acc.trim().is_empty() {
            it.expense_account = None;
        } else {
            let chart = Chart::load(ctx)?;
            let x = chart.resolve(&acc)?;
            if !matches!(x.account_type, AccountType::Expense | AccountType::Asset) || x.placeholder
            {
                return Err(invalid(format!(
                    "{} is not an expense or asset account",
                    chart.full_name(x.id)
                )));
            }
            it.expense_account = Some(x.id);
        }
    }
    if let Some(c) = a.cost {
        if c.is_sign_negative() || decimals(c) > 4 {
            return Err(invalid("cost: zero or more with at most 4 decimals"));
        }
        it.cost = Some(c.normalize());
    }
    Ok(())
}

fn item_json(ctx: &Ctx<'_>, it: &Item) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let mut v = serde_json::to_value(it)?;
    if let Some(o) = v.as_object_mut() {
        o.insert(
            "income_account_name".into(),
            json!(chart.full_name(it.income_account)),
        );
        o.insert("price".into(), json!(it.price.to_string()));
        o.insert("cost".into(), json!(it.cost.map(|c| c.to_string())));
        o.insert(
            "expense_account_name".into(),
            json!(it.expense_account.map(|a| chart.full_name(a))),
        );
    }
    Ok(v)
}

fn item_create(ctx: &Ctx<'_>, a: ItemArgs) -> CommandResult<Value> {
    let kind = a.kind.clone().unwrap_or_else(|| "service".into());
    let taxable_default = match kind.as_str() {
        "product" => ms::get_bool(ctx, "items.default_product_taxable")?,
        "service" => ms::get_bool(ctx, "items.default_service_taxable")?,
        _ => false,
    };
    if a.name.is_none() || a.income_account.is_none() {
        return Err(invalid("give the name and the income account"));
    }
    let mut it = Item {
        id: new_id(),
        name: String::new(),
        kind,
        description: None,
        sku: None,
        price: Decimal::ZERO,
        percent: false,
        income_account: AccountId::default(),
        taxable: taxable_default,
        active: true,
        expense_account: None,
        cost: None,
    };
    apply_item(ctx, &mut it, a)?;
    save(ctx, DocTable::Item, &it.id, None, 0, &it)?;
    item_json(ctx, &it)
}

fn item_update(ctx: &Ctx<'_>, mut a: ItemArgs) -> CommandResult<Value> {
    let r = a.item.take().ok_or_else(|| invalid("give the item"))?;
    let mut it = find_item(ctx, &r)?;
    apply_item(ctx, &mut it, a)?;
    save(ctx, DocTable::Item, &it.id, None, 0, &it)?;
    item_json(ctx, &it)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ItemRef {
    item: String,
}

fn item_get(ctx: &Ctx<'_>, a: ItemRef) -> CommandResult<Value> {
    item_json(ctx, &find_item(ctx, &a.item)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ItemListArgs {
    #[serde(default)]
    active: Option<bool>,
}

fn item_list(ctx: &Ctx<'_>, a: ItemListArgs) -> CommandResult<Value> {
    let mut out = Vec::new();
    for it in items(ctx)? {
        if a.active.is_some_and(|x| x != it.active) {
            continue;
        }
        out.push(item_json(ctx, &it)?);
    }
    Ok(Value::Array(out))
}

fn item_set_active(ctx: &Ctx<'_>, a: ActiveArgs) -> CommandResult<Value> {
    let mut it = find_item(ctx, &a.party)?;
    it.active = a.active;
    save(ctx, DocTable::Item, &it.id, None, 0, &it)?;
    item_json(ctx, &it)
}

fn item_delete(ctx: &Ctx<'_>, a: ItemRef) -> CommandResult<Value> {
    let it = find_item(ctx, &a.item)?;
    let docs: Vec<SalesDoc> = load(ctx, DocTable::SalesDoc, None)?;
    if docs.iter().any(|d| {
        d.lines
            .iter()
            .any(|l| l.item.as_deref() == Some(it.id.as_str()))
    }) {
        return Err(rejected(format!(
            "{} is used on documents; make it inactive instead",
            it.name
        )));
    }
    ctx.save_doc(DocTable::Item, &it.id, None)?;
    Ok(json!({"deleted": it.id}))
}

// ----- sales tax ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TaxRate {
    pub id: String,
    /// `rate` or `group`.
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub rate: Option<Decimal>,
    #[serde(default)]
    pub agency: Option<String>,
    /// The agency as a vendor party (business-purchases.md), when it is one.
    #[serde(default)]
    pub agency_vendor: Option<String>,
    #[serde(default)]
    pub members: Vec<String>,
    pub active: bool,
}

pub(crate) fn tax_rates(ctx: &Ctx<'_>) -> CommandResult<Vec<TaxRate>> {
    let mut v: Vec<TaxRate> = load(ctx, DocTable::TaxRate, None)?;
    v.sort_by_key(|t| t.name.to_lowercase());
    Ok(v)
}

pub(crate) fn find_tax(ctx: &Ctx<'_>, r: &str) -> CommandResult<TaxRate> {
    let r = r.trim();
    tax_rates(ctx)?
        .into_iter()
        .find(|t| t.id == r || t.name.eq_ignore_ascii_case(r))
        .ok_or_else(|| CommandError::NotFound(format!("tax rate {r:?}")))
}

/// The component rates of a rate or group: (id, name, percent).
pub(crate) fn components(ctx: &Ctx<'_>, id: &str) -> CommandResult<Vec<(String, String, Decimal)>> {
    let all = tax_rates(ctx)?;
    let Some(t) = all.iter().find(|t| t.id == id) else {
        return Ok(Vec::new());
    };
    if t.kind == "group" {
        Ok(t.members
            .iter()
            .filter_map(|m| all.iter().find(|x| x.id == *m))
            .map(|x| (x.id.clone(), x.name.clone(), x.rate.unwrap_or_default()))
            .collect())
    } else {
        Ok(vec![(
            t.id.clone(),
            t.name.clone(),
            t.rate.unwrap_or_default(),
        )])
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaxArgs {
    /// Rate or group id or name (update only).
    #[serde(default)]
    tax_rate: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// Percent 0-100, up to 4 decimals (rates).
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    rate: Option<Decimal>,
    /// The tax agency: a vendor (id or name) or free text.
    #[serde(default)]
    agency: Option<String>,
    /// Member rates (groups).
    #[serde(default)]
    members: Option<Vec<String>>,
    #[serde(default)]
    active: Option<bool>,
}

fn apply_tax(ctx: &Ctx<'_>, t: &mut TaxRate, a: TaxArgs) -> CommandResult<()> {
    let all = tax_rates(ctx)?;
    if let Some(n) = a.name {
        let n = text(Some(n), 100, "name")?.ok_or_else(|| invalid("name: 1-100 characters"))?;
        if all
            .iter()
            .any(|x| x.id != t.id && x.name.eq_ignore_ascii_case(&n))
        {
            return Err(rejected(format!("a tax rate named {n:?} already exists")));
        }
        t.name = n;
    }
    if let Some(r) = a.rate {
        if t.kind == "group" {
            return Err(invalid("a group's rate is the sum of its members"));
        }
        if r.is_sign_negative() || r > Decimal::ONE_HUNDRED || decimals(r) > 4 {
            return Err(invalid(
                "rate: a percent from 0 to 100 with at most 4 decimals",
            ));
        }
        t.rate = Some(r.normalize());
    }
    if a.agency.is_some() {
        t.agency = text(a.agency, 100, "agency")?;
        t.agency_vendor = match &t.agency {
            Some(n) => parties(ctx)?
                .into_iter()
                .find(|p| p.is("vendor") && (p.id == *n || p.name.eq_ignore_ascii_case(n)))
                .map(|p| {
                    t.agency = Some(p.name.clone());
                    p.id
                }),
            None => None,
        };
    }
    if let Some(m) = a.members {
        if t.kind != "group" {
            return Err(invalid("only groups have members"));
        }
        let mut ids = Vec::new();
        for r in m {
            let x = all
                .iter()
                .find(|x| x.id == r || x.name.eq_ignore_ascii_case(r.trim()))
                .ok_or_else(|| CommandError::NotFound(format!("tax rate {r:?}")))?;
            if x.kind != "rate" {
                return Err(invalid("a group's members are rates"));
            }
            if !ids.contains(&x.id) {
                ids.push(x.id.clone());
            }
        }
        if ids.is_empty() {
            return Err(invalid("a group needs member rates"));
        }
        t.members = ids;
    }
    if let Some(x) = a.active {
        t.active = x;
    }
    Ok(())
}

fn tax_json(ctx: &Ctx<'_>, t: &TaxRate) -> CommandResult<Value> {
    let total = components(ctx, &t.id)?
        .iter()
        .try_fold(Decimal::ZERO, |s, c| add(s, c.2))?;
    let all = tax_rates(ctx)?;
    Ok(json!({
        "id": t.id, "kind": t.kind, "name": t.name,
        "rate": total.normalize().to_string(),
        "agency": t.agency,
        "agency_vendor": t.agency_vendor,
        "members": t.members.iter().filter_map(|m| all.iter().find(|x| x.id == *m)).map(|x| json!({"id": x.id, "name": x.name, "rate": x.rate.map(|r| r.to_string())})).collect::<Vec<_>>(),
        "active": t.active,
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaxCreateArgs {
    /// `rate` (default) or `group`.
    #[serde(default)]
    kind: Option<String>,
    #[serde(flatten)]
    rest: TaxArgs,
}

fn tax_create(ctx: &Ctx<'_>, a: TaxCreateArgs) -> CommandResult<Value> {
    let kind = a.kind.unwrap_or_else(|| "rate".into());
    if !matches!(kind.as_str(), "rate" | "group") {
        return Err(invalid("kind is rate or group"));
    }
    if a.rest.name.is_none() {
        return Err(invalid("give the name"));
    }
    if kind == "rate" && a.rest.rate.is_none() {
        return Err(invalid("give the rate"));
    }
    if kind == "group" && a.rest.members.is_none() {
        return Err(invalid("give the member rates"));
    }
    let mut t = TaxRate {
        id: new_id(),
        kind,
        name: String::new(),
        rate: None,
        agency: None,
        agency_vendor: None,
        members: Vec::new(),
        active: true,
    };
    apply_tax(ctx, &mut t, a.rest)?;
    save(ctx, DocTable::TaxRate, &t.id, None, 0, &t)?;
    tax_json(ctx, &t)
}

fn tax_update(ctx: &Ctx<'_>, mut a: TaxArgs) -> CommandResult<Value> {
    let r = a
        .tax_rate
        .take()
        .ok_or_else(|| invalid("give the tax rate"))?;
    let mut t = find_tax(ctx, &r)?;
    apply_tax(ctx, &mut t, a)?;
    save(ctx, DocTable::TaxRate, &t.id, None, 0, &t)?;
    tax_json(ctx, &t)
}

fn tax_list(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let mut out = Vec::new();
    for t in tax_rates(ctx)? {
        out.push(tax_json(ctx, &t)?);
    }
    Ok(Value::Array(out))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaxRef {
    tax_rate: String,
}

fn tax_delete(ctx: &Ctx<'_>, a: TaxRef) -> CommandResult<Value> {
    let t = find_tax(ctx, &a.tax_rate)?;
    let docs: Vec<SalesDoc> = load(ctx, DocTable::SalesDoc, None)?;
    let used = docs.iter().any(|d| {
        d.tax_rate.as_deref() == Some(t.id.as_str()) || d.totals.tax.iter().any(|x| x.rate == t.id)
    }) || tax_rates(ctx)?.iter().any(|g| g.members.contains(&t.id));
    if used {
        return Err(rejected(format!(
            "{} is used; make it inactive instead",
            t.name
        )));
    }
    ctx.save_doc(DocTable::TaxRate, &t.id, None)?;
    Ok(json!({"deleted": t.id}))
}

// ----- documents ----------------------------------------------------------------------

/// One line as entered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct LineArg {
    /// Item id or name (items module).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// Income account (without items).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub quantity: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub rate: Option<Decimal>,
    /// Typed amount (otherwise quantity × rate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub amount: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taxable: Option<bool>,
    /// A subtotal line.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub subtotal: bool,
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
    /// Invoices: the billable project costs this line bills (keys from
    /// `project.unbilled`); they show as billed while the line exists.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub billable_items: Vec<String>,
}

/// A line as computed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LineOut {
    /// `item`, `account`, `discount` or `subtotal`.
    pub kind: String,
    pub item: Option<String>,
    pub item_name: Option<String>,
    pub account: Option<AccountId>,
    pub description: String,
    pub quantity: Option<Decimal>,
    pub rate: Option<Decimal>,
    pub amount: Decimal,
    pub taxable: bool,
    #[serde(default, skip_serializing_if = "SplitTags::is_empty")]
    pub tags: SplitTags,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TaxLine {
    pub rate: String,
    pub name: String,
    pub percent: Decimal,
    pub base: Decimal,
    pub amount: Decimal,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Totals {
    pub lines: Vec<LineOut>,
    pub subtotal: Decimal,
    pub taxable_base: Decimal,
    pub nontaxable: Decimal,
    pub tax: Vec<TaxLine>,
    pub tax_total: Decimal,
    pub total: Decimal,
}

/// An application of a payment or credit memo to an invoice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Application {
    pub invoice: String,
    pub amount: Decimal,
    pub date: Date,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SalesDoc {
    pub id: String,
    /// `invoice`, `estimate` or `credit_memo`.
    pub kind: String,
    pub number: String,
    pub customer: String,
    pub date: Date,
    #[serde(default)]
    pub terms: Option<Value>,
    #[serde(default)]
    pub due_date: Option<Date>,
    #[serde(default)]
    pub expires: Option<Date>,
    pub lines: Vec<LineArg>,
    #[serde(default)]
    pub tax_rate: Option<String>,
    #[serde(default)]
    pub memo: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    /// Invoices: draft, open, void (open shows as partial/paid by balance).
    /// Estimates: draft, sent, accepted, declined, converted.
    pub status: String,
    #[serde(default)]
    pub txn: Option<TransactionId>,
    /// The estimate an invoice came from, or the invoice an estimate became.
    #[serde(default)]
    pub linked: Option<String>,
    /// Credit memos: applications to invoices.
    #[serde(default)]
    pub applied: Vec<Application>,
    pub totals: Totals,
    /// Snapshot of the customer's exemption when the document was saved.
    #[serde(default)]
    pub exempt: bool,
    #[serde(default)]
    pub exemption_note: Option<String>,
    pub created_at: String,
    /// Document-level class, location and fund (ids).
    #[serde(default, skip_serializing_if = "SplitTags::is_empty")]
    pub tags: SplitTags,
    /// The template the document prints with (id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Late-fee invoices: the invoices the fee was charged on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub late_fee_for: Vec<String>,
}

pub(crate) fn docs(ctx: &Ctx<'_>, kind: &str) -> CommandResult<Vec<SalesDoc>> {
    let mut v: Vec<SalesDoc> = load(ctx, DocTable::SalesDoc, Some(kind))?;
    v.sort_by(|a, b| (a.date, &a.number).cmp(&(b.date, &b.number)));
    Ok(v)
}

pub(crate) fn find_doc(ctx: &Ctx<'_>, kind: &str, r: &str) -> CommandResult<SalesDoc> {
    let r = r.trim();
    docs(ctx, kind)?
        .into_iter()
        .find(|d| d.id == r || d.number.eq_ignore_ascii_case(r))
        .ok_or_else(|| CommandError::NotFound(format!("{} {r:?}", kind.replace('_', " "))))
}

pub(crate) fn save_doc(ctx: &Ctx<'_>, d: &SalesDoc) -> CommandResult<()> {
    save(
        ctx,
        DocTable::SalesDoc,
        &d.id,
        Some(&d.kind),
        date_ord(d.date),
        d,
    )
}

/// Compute a document's lines and totals.
pub(crate) fn compute(
    ctx: &Ctx<'_>,
    lines: &[LineArg],
    tax_rate: Option<&str>,
    exempt: bool,
) -> CommandResult<Totals> {
    if lines.is_empty() || lines.len() > 500 {
        return Err(invalid("a document has 1 to 500 lines"));
    }
    let items_on = module_on(ctx, ITEMS)?;
    let all_items = if items_on { items(ctx)? } else { Vec::new() };
    let chart = Chart::load(ctx)?;
    let mut out: Vec<LineOut> = Vec::new();
    // Group sums since the previous subtotal: (all non-discount, taxable non-discount).
    let mut group = (Decimal::ZERO, Decimal::ZERO);
    let mut base = Decimal::ZERO;
    let mut subtotal = Decimal::ZERO;
    for (i, l) in lines.iter().enumerate() {
        let n = i + 1;
        if l.subtotal {
            out.push(LineOut {
                kind: "subtotal".into(),
                description: l.description.clone().unwrap_or_else(|| "Subtotal".into()),
                amount: group.0,
                ..LineOut::default()
            });
            group = (Decimal::ZERO, Decimal::ZERO);
            continue;
        }
        let description = text(
            l.description.clone(),
            1000,
            &format!("line {n} description"),
        )?;
        for (v, what) in [(l.quantity, "quantity"), (l.rate, "rate")] {
            if v.is_some_and(|x| decimals(x) > 4) {
                return Err(invalid(format!("line {n}: {what} has at most 4 decimals")));
            }
        }
        let tags = crate::dimensions::resolve_tags_p(
            ctx,
            l.class.as_deref(),
            l.location.as_deref(),
            l.fund.as_deref(),
            l.project.as_deref(),
            Some(false),
        )
        .map_err(|e| invalid(format!("line {n}: {e}")))?;
        let (kind, item, account, desc, rate, taxable, percent) = match (&l.item, &l.account) {
            (Some(r), _) if items_on => {
                let it = all_items
                    .iter()
                    .find(|x| x.id == *r || x.name.eq_ignore_ascii_case(r.trim()))
                    .ok_or_else(|| invalid(format!("line {n}: no item {r:?}")))?;
                let kind = if it.kind == "discount" {
                    "discount"
                } else {
                    "item"
                };
                (
                    kind,
                    Some(it.clone()),
                    it.income_account,
                    description
                        .or_else(|| it.description.clone())
                        .unwrap_or_else(|| it.name.clone()),
                    l.rate.or(Some(it.price)),
                    l.taxable.unwrap_or(it.taxable),
                    it.percent,
                )
            }
            (Some(_), _) => {
                return Err(invalid(format!(
                    "line {n}: items are off; name an income account"
                )));
            }
            (None, Some(acc)) => {
                let a = chart.resolve(acc)?;
                if !matches!(a.account_type, AccountType::Income | AccountType::Expense)
                    || a.placeholder
                {
                    return Err(invalid(format!(
                        "line {n}: {} is not an income account",
                        chart.full_name(a.id)
                    )));
                }
                (
                    "account",
                    None,
                    a.id,
                    description.unwrap_or_default(),
                    l.rate,
                    l.taxable.unwrap_or(false),
                    false,
                )
            }
            (None, None) => {
                return Err(invalid(format!(
                    "line {n}: give an item or an income account"
                )));
            }
        };
        if kind == "discount" {
            let amount = match (l.amount, percent) {
                (Some(a), _) => -round2(a.abs()),
                (None, true) => -round2(
                    mul(group.0, rate.unwrap_or_default())?
                        .checked_div(Decimal::ONE_HUNDRED)
                        .unwrap_or_default(),
                ),
                // A fixed discount is per unit: quantity (default 1) × rate.
                (None, false) => -round2(mul(
                    l.quantity.unwrap_or(Decimal::ONE),
                    rate.unwrap_or_default(),
                )?),
            };
            // The discount's taxable share, in proportion to the lines it follows.
            let share = if group.0.is_zero() {
                Decimal::ZERO
            } else {
                round2(
                    mul(amount, group.1)?
                        .checked_div(group.0)
                        .unwrap_or_default(),
                )
            };
            if !exempt {
                base = add(base, share)?;
            }
            subtotal = add(subtotal, amount)?;
            out.push(LineOut {
                kind: "discount".into(),
                item: item.as_ref().map(|i| i.id.clone()),
                item_name: item.as_ref().map(|i| i.name.clone()),
                account: Some(account),
                description: desc,
                quantity: if percent {
                    None
                } else {
                    Some(l.quantity.unwrap_or(Decimal::ONE))
                },
                rate,
                amount,
                taxable: !share.is_zero(),
                tags: tags.clone(),
            });
            continue;
        }
        let q = l.quantity.unwrap_or(Decimal::ONE);
        let amount = match l.amount {
            Some(a) => round2(a),
            None => round2(mul(q, rate.unwrap_or_default())?),
        };
        group.0 = add(group.0, amount)?;
        if taxable {
            group.1 = add(group.1, amount)?;
            if !exempt {
                base = add(base, amount)?;
            }
        }
        subtotal = add(subtotal, amount)?;
        out.push(LineOut {
            kind: kind.into(),
            item: item.as_ref().map(|i| i.id.clone()),
            item_name: item.as_ref().map(|i| i.name.clone()),
            account: Some(account),
            description: desc,
            quantity: Some(q),
            rate,
            amount,
            taxable,
            tags,
        });
    }
    let mut tax = Vec::new();
    let mut tax_total = Decimal::ZERO;
    if let (false, Some(r), true) = (exempt, tax_rate, module_on(ctx, SALES_TAX)?) {
        for (id, name, pct) in components(ctx, r)? {
            let amount = round2(
                mul(base, pct)?
                    .checked_div(Decimal::ONE_HUNDRED)
                    .unwrap_or_default(),
            );
            tax_total = add(tax_total, amount)?;
            tax.push(TaxLine {
                rate: id,
                name,
                percent: pct,
                base,
                amount,
            });
        }
    }
    let total = add(subtotal, tax_total)?;
    Ok(Totals {
        lines: out,
        subtotal,
        taxable_base: base,
        nontaxable: sub(subtotal, base)?,
        tax,
        tax_total,
        total,
    })
}

fn totals_json(chart: &Chart, t: &Totals) -> Value {
    json!({
        "lines": t.lines.iter().map(|l| json!({
            "kind": l.kind, "item": l.item, "item_name": l.item_name,
            "account": l.account, "account_name": l.account.map(|a| chart.full_name(a)),
            "description": l.description,
            "quantity": l.quantity.map(|q| q.normalize().to_string()),
            "rate": l.rate.map(|r| r.normalize().to_string()),
            "amount": money(l.amount), "taxable": l.taxable,
        })).collect::<Vec<_>>(),
        "subtotal": money(t.subtotal),
        "taxable_base": money(t.taxable_base),
        "tax": t.tax.iter().map(|x| json!({"rate": x.rate, "name": x.name, "percent": x.percent.normalize().to_string(), "amount": money(x.amount)})).collect::<Vec<_>>(),
        "tax_total": money(t.tax_total),
        "total": money(t.total),
    })
}

/// Applications to an invoice from every payment and credit memo: (amount, by date).
pub(crate) struct Ledgerish {
    pub payments: Vec<Payment>,
    pub credits: Vec<SalesDoc>,
    pub refunds: Vec<Refund>,
}

impl Ledgerish {
    pub(crate) fn load(ctx: &Ctx<'_>) -> CommandResult<Self> {
        Ok(Self {
            payments: load(ctx, DocTable::Payment, Some("payment"))?,
            credits: docs(ctx, "credit_memo")?,
            refunds: load(ctx, DocTable::Payment, Some("refund"))?,
        })
    }

    /// Amount applied to an invoice as of a date (all when `None`).
    pub(crate) fn applied_to(&self, invoice: &str, as_of: Option<Date>) -> Decimal {
        let ok = |d: Date| as_of.is_none_or(|x| d <= x);
        let from_p = self
            .payments
            .iter()
            .flat_map(|p| p.applied.iter())
            .filter(|a| a.invoice == invoice && ok(a.date))
            .fold(Decimal::ZERO, |s, a| s.checked_add(a.amount).unwrap_or(s));
        let from_c = self
            .credits
            .iter()
            .filter(|c| c.status != "void")
            .flat_map(|c| c.applied.iter())
            .filter(|a| a.invoice == invoice && ok(a.date))
            .fold(Decimal::ZERO, |s, a| s.checked_add(a.amount).unwrap_or(s));
        from_p.checked_add(from_c).unwrap_or(from_p)
    }

    fn refunded_from(&self, source: &str, as_of: Option<Date>) -> Decimal {
        self.refunds
            .iter()
            .filter(|r| as_of.is_none_or(|x| r.date <= x))
            .flat_map(|r| r.sources.iter())
            .filter(|s| s.source == source)
            .fold(Decimal::ZERO, |s, x| s.checked_add(x.amount).unwrap_or(s))
    }

    /// A payment's unapplied amount.
    pub(crate) fn unapplied_payment(&self, p: &Payment, as_of: Option<Date>) -> Decimal {
        if as_of.is_some_and(|x| p.date > x) {
            return Decimal::ZERO;
        }
        let applied = p
            .applied
            .iter()
            .filter(|a| as_of.is_none_or(|x| a.date <= x))
            .fold(Decimal::ZERO, |s, a| s.checked_add(a.amount).unwrap_or(s));
        p.amount
            .checked_sub(applied)
            .and_then(|x| x.checked_sub(self.refunded_from(&p.id, as_of)))
            .unwrap_or_default()
    }

    /// A credit memo's unapplied amount.
    pub(crate) fn unapplied_credit(&self, c: &SalesDoc, as_of: Option<Date>) -> Decimal {
        if c.status == "void" || as_of.is_some_and(|x| c.date > x) {
            return Decimal::ZERO;
        }
        let applied = c
            .applied
            .iter()
            .filter(|a| as_of.is_none_or(|x| a.date <= x))
            .fold(Decimal::ZERO, |s, a| s.checked_add(a.amount).unwrap_or(s));
        c.totals
            .total
            .checked_sub(applied)
            .and_then(|x| x.checked_sub(self.refunded_from(&c.id, as_of)))
            .unwrap_or_default()
    }
}

/// An invoice's balance and display status.
pub(crate) fn invoice_state(
    d: &SalesDoc,
    l: &Ledgerish,
    as_of: Option<Date>,
) -> (Decimal, Decimal, String) {
    let applied = l.applied_to(&d.id, as_of);
    let balance = d.totals.total.checked_sub(applied).unwrap_or_default();
    let status = match d.status.as_str() {
        "draft" | "void" => d.status.clone(),
        _ if d.totals.total > Decimal::ZERO && balance.is_zero() => "paid".into(),
        _ if applied > Decimal::ZERO => "partial".into(),
        _ => "open".into(),
    };
    (applied, balance, status)
}

fn estimate_status(d: &SalesDoc, today: Date) -> String {
    if matches!(d.status.as_str(), "draft" | "sent") && d.expires.is_some_and(|e| today > e) {
        "expired".into()
    } else {
        d.status.clone()
    }
}

pub(crate) fn doc_json(ctx: &Ctx<'_>, d: &SalesDoc) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let customer = find_party(ctx, &d.customer).ok();
    let l = Ledgerish::load(ctx)?;
    let mut v = json!({
        "id": d.id, "kind": d.kind, "number": d.number,
        "customer": d.customer, "customer_name": customer.as_ref().map(|c| c.name.clone()),
        "date": d.date, "terms": d.terms, "terms_label": d.terms.as_ref().map(terms_label),
        "due_date": d.due_date, "expires": d.expires,
        "lines": d.lines, "tax_rate": d.tax_rate,
        "memo": d.memo, "message": d.message, "txn": d.txn, "linked": d.linked,
        "totals": totals_json(&chart, &d.totals),
        "exempt": d.exempt, "exemption_note": d.exemption_note,
        "tag_names": crate::dimensions::tag_names(ctx, &d.tags)?,
    });
    let o = v
        .as_object_mut()
        .ok_or_else(|| CommandError::Integrity("json".into()))?;
    match d.kind.as_str() {
        "invoice" => {
            let (applied, balance, status) = invoice_state(d, &l, None);
            o.insert("applied".into(), json!(money(applied)));
            o.insert("balance".into(), json!(money(balance)));
            o.insert("status".into(), json!(status));
            let mut apps = Vec::new();
            for p in &l.payments {
                for a in p.applied.iter().filter(|a| a.invoice == d.id) {
                    apps.push(json!({"kind": "payment", "id": p.id, "date": a.date, "amount": money(a.amount), "reference": p.reference}));
                }
            }
            for c in &l.credits {
                for a in c.applied.iter().filter(|a| a.invoice == d.id) {
                    apps.push(json!({"kind": "credit_memo", "id": c.id, "number": c.number, "date": a.date, "amount": money(a.amount)}));
                }
            }
            o.insert("applications".into(), Value::Array(apps));
            let today = mk_store::today_utc()?;
            o.insert(
                "overdue".into(),
                json!(
                    matches!(status.as_str(), "open" | "partial")
                        && d.due_date.is_some_and(|x| today > x)
                ),
            );
        }
        "estimate" => {
            o.insert(
                "status".into(),
                json!(estimate_status(d, mk_store::today_utc()?)),
            );
        }
        _ => {
            o.insert("status".into(), json!(d.status));
            o.insert("applied".into(), json!(d.applied));
            o.insert(
                "unapplied".into(),
                json!(money(l.unapplied_credit(d, None))),
            );
        }
    }
    Ok(v)
}

/// Document arguments shared by invoices, estimates and credit memos.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DocArgs {
    #[serde(default)]
    pub customer: Option<String>,
    #[serde(default)]
    pub number: Option<String>,
    #[serde(default)]
    pub date: Option<Date>,
    #[serde(default)]
    pub terms: Option<Value>,
    #[serde(default)]
    pub due_date: Option<Date>,
    #[serde(default)]
    pub expires: Option<Date>,
    #[serde(default)]
    pub lines: Option<Vec<LineArg>>,
    /// Tax rate or group (id or name); "" for none.
    #[serde(default)]
    pub tax_rate: Option<String>,
    #[serde(default)]
    pub memo: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    /// Invoices: `draft` or `open` (default open).
    #[serde(default)]
    pub status: Option<String>,
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
    /// The template to print with (id or name; "" for the default).
    #[serde(default)]
    pub template: Option<String>,
}

/// The number for a new document: typed (must be unique) or the next in sequence.
fn take_number(
    ctx: &Ctx<'_>,
    kind: &str,
    typed: Option<String>,
    advance: bool,
) -> CommandResult<String> {
    let existing: Vec<String> = docs(ctx, kind)?
        .into_iter()
        .map(|d| d.number.to_lowercase())
        .collect();
    if let Some(t) = typed.and_then(|t| text(Some(t), 20, "number").transpose()) {
        let t = t?;
        if existing.contains(&t.to_lowercase()) {
            return Err(rejected(format!("number {t} is already used")));
        }
        return Ok(t);
    }
    let (prefix_key, next_key) = match kind {
        "invoice" => ("invoicing.number_prefix", "invoicing.next_number"),
        "estimate" => ("estimates.number_prefix", "estimates.next_number"),
        _ => ("", ""),
    };
    let (prefix, mut n) = if prefix_key.is_empty() {
        ("CM-".to_owned(), 1001)
    } else {
        (ms::get_str(ctx, prefix_key)?, ms::get_i64(ctx, next_key)?)
    };
    loop {
        let candidate = format!("{prefix}{n}");
        n = n.saturating_add(1);
        if !existing.contains(&candidate.to_lowercase()) {
            if advance && !next_key.is_empty() {
                ms::put(ctx, next_key, json!(n))?;
            }
            return Ok(candidate);
        }
    }
}

fn resolve_tax(
    ctx: &Ctx<'_>,
    given: Option<&str>,
    customer: &Party,
) -> CommandResult<Option<String>> {
    if !module_on(ctx, SALES_TAX)? {
        return Ok(None);
    }
    match given {
        Some(r) if r.trim().is_empty() => Ok(None),
        Some(r) => Ok(Some(find_tax(ctx, r)?.id)),
        None => Ok(match &customer.tax_rate {
            Some(t) => Some(t.clone()),
            None => ms::get(ctx, "sales-tax.default_rate")?
                .as_str()
                .map(str::to_owned),
        }),
    }
}

/// Build (or rebuild) a document from arguments.
pub(crate) fn build_doc(
    ctx: &Ctx<'_>,
    kind: &str,
    a: DocArgs,
    base: Option<SalesDoc>,
) -> CommandResult<SalesDoc> {
    build(ctx, kind, a, base, true)
}

fn build(
    ctx: &Ctx<'_>,
    kind: &str,
    a: DocArgs,
    base: Option<SalesDoc>,
    advance: bool,
) -> CommandResult<SalesDoc> {
    let customer_ref = a
        .customer
        .clone()
        .or_else(|| base.as_ref().map(|b| b.customer.clone()))
        .ok_or_else(|| invalid("give the customer"))?;
    let customer = find_party(ctx, &customer_ref)?;
    if !customer.roles.iter().any(|r| r == "customer") {
        return Err(invalid(format!("{} is not a customer", customer.name)));
    }
    let date = a
        .date
        .or(base.as_ref().map(|b| b.date))
        .map_or_else(mk_store::today_utc, Ok)?;
    let lines = a
        .lines
        .clone()
        .or_else(|| base.as_ref().map(|b| b.lines.clone()))
        .ok_or_else(|| invalid("give the lines"))?;
    let tax_given = a.tax_rate.clone().or_else(|| {
        base.as_ref()
            .map(|b| b.tax_rate.clone().unwrap_or_default())
    });
    let tax_rate = resolve_tax(ctx, tax_given.as_deref(), &customer)?;
    // Exemption is taken from the customer when the document is (re)saved.
    let exempt = customer.tax_exempt;
    let totals = compute(ctx, &lines, tax_rate.as_deref(), exempt)?;
    let terms = match (&a.terms, &base) {
        (Some(t), _) => Some(check_terms(t)?),
        (None, Some(b)) if b.terms.is_some() => b.terms.clone(),
        _ if kind == "invoice" => Some(effective_terms(ctx, &customer)?),
        _ => None,
    };
    let due_date = match (kind, a.due_date) {
        ("invoice", Some(d)) => Some(d),
        ("invoice", None) => {
            let keep = base
                .as_ref()
                .filter(|_| a.terms.is_none() && a.date.is_none())
                .and_then(|b| b.due_date);
            keep.or_else(|| terms.as_ref().map(|t| due_from(t, date)))
        }
        _ => None,
    };
    let expires = match (kind, a.expires) {
        ("estimate", Some(e)) => Some(e),
        ("estimate", None) => base
            .as_ref()
            .and_then(|b| b.expires)
            .filter(|_| a.date.is_none())
            .or_else(|| {
                let days = ms::get_i64(ctx, "estimates.default_valid_days").unwrap_or(30);
                date.add_days(i32::try_from(days).unwrap_or(30))
            }),
        _ => None,
    };
    let default_message = match kind {
        "invoice" => ms::get_str(ctx, "invoicing.default_message")?,
        "estimate" => ms::get_str(ctx, "estimates.default_message")?,
        _ => String::new(),
    };
    let message = match (&a.message, &base) {
        (Some(m), _) => text(Some(m.clone()), 2000, "message")?,
        (None, Some(b)) => b.message.clone(),
        (None, None) => Some(default_message).filter(|m| !m.is_empty()),
    };
    let memo = match (&a.memo, &base) {
        (Some(m), _) => text(Some(m.clone()), 1000, "memo")?,
        (None, Some(b)) => b.memo.clone(),
        _ => None,
    };
    let number = match &base {
        Some(b) => match &a.number {
            Some(n) if !n.eq_ignore_ascii_case(&b.number) => {
                take_number(ctx, kind, Some(n.clone()), advance)?
            }
            _ => b.number.clone(),
        },
        None => take_number(ctx, kind, a.number.clone(), advance)?,
    };
    let doc = SalesDoc {
        id: base.as_ref().map_or_else(new_id, |b| b.id.clone()),
        kind: kind.into(),
        number,
        customer: customer.id.clone(),
        date,
        terms,
        due_date,
        expires,
        lines,
        tax_rate,
        memo,
        message,
        status: base
            .as_ref()
            .map_or_else(|| "draft".into(), |b| b.status.clone()),
        txn: base.as_ref().and_then(|b| b.txn),
        linked: base.as_ref().and_then(|b| b.linked.clone()),
        applied: base.as_ref().map(|b| b.applied.clone()).unwrap_or_default(),
        totals,
        exempt,
        exemption_note: customer.exemption_note.clone().filter(|_| exempt),
        created_at: base
            .as_ref()
            .map_or_else(now_rfc3339, |b| Ok(b.created_at.clone()))?,
        tags: {
            let mut t = crate::dimensions::doc_tags_p(
                ctx,
                a.class.as_deref(),
                a.location.as_deref(),
                a.fund.as_deref(),
                a.project.as_deref(),
                base.as_ref().map(|b| &b.tags),
            )?;
            // Income lines are never billable costs.
            t.billable = false;
            t
        },
        template: match &a.template {
            Some(t) if t.trim().is_empty() => None,
            Some(t) => Some(crate::templates::find(ctx, t)?.id),
            None => base.as_ref().and_then(|b| b.template.clone()),
        },
        late_fee_for: base
            .as_ref()
            .map(|b| b.late_fee_for.clone())
            .unwrap_or_default(),
    };
    // projects.md: billable costs go on invoice lines, once each.
    let keys: Vec<String> = doc
        .lines
        .iter()
        .flat_map(|l| l.billable_items.iter().cloned())
        .collect();
    if !keys.is_empty() {
        if kind != "invoice" {
            return Err(invalid("billable project costs go on invoices only"));
        }
        crate::projects::check_billable(ctx, &doc.id, &keys)?;
    }
    Ok(doc)
}

/// Post an invoice (or credit memo, reversed) as one transaction.
pub(crate) fn post_doc(ctx: &Ctx<'_>, d: &mut SalesDoc) -> CommandResult<()> {
    let sign = if d.kind == "credit_memo" {
        Decimal::NEGATIVE_ONE
    } else {
        Decimal::ONE
    };
    let ar = receivable(ctx)?;
    let mut by_account: Vec<((AccountId, SplitTags), Decimal)> = Vec::new();
    for l in d.totals.lines.iter().filter(|l| l.kind != "subtotal") {
        if let Some(a) = l.account {
            let key = (a, crate::dimensions::merge(&l.tags, &d.tags));
            match by_account.iter_mut().find(|(k, _)| *k == key) {
                Some((_, e)) => *e = add(*e, l.amount)?,
                None => by_account.push((key, l.amount)),
            }
        }
    }
    let tagged = |mut s: Split, t: &SplitTags| {
        s.tags = t.clone();
        s
    };
    // The receivable and tax lines carry no project (projects.md: only income and
    // expense lines count).
    let mut doc_tags = d.tags.clone();
    doc_tags.project = None;
    doc_tags.billable = false;
    let mut splits = vec![tagged(
        cash(ar, mul(d.totals.total, sign)?, Some(d.number.clone())),
        &doc_tags,
    )];
    for ((acc, tags), amt) in by_account {
        splits.push(tagged(cash(acc, mul(-amt, sign)?, None), &tags));
    }
    if !d.totals.tax.is_empty() {
        let liab = liability(ctx)?;
        for t in &d.totals.tax {
            splits.push(tagged(
                cash(
                    liab,
                    mul(-t.amount, sign)?,
                    Some(format!("Sales tax: {}", t.name)),
                ),
                &doc_tags,
            ));
        }
    }
    let customer = find_party(ctx, &d.customer)?;
    let memo = format!(
        "{} {}",
        if d.kind == "credit_memo" {
            "Credit memo"
        } else {
            "Invoice"
        },
        d.number
    );
    d.txn = Some(post_txn(
        ctx,
        d.txn,
        d.date,
        &customer.name,
        Some(memo),
        splits,
    )?);
    Ok(())
}

pub(crate) fn require_open_period(ctx: &Ctx<'_>, date: Date) -> CommandResult<()> {
    if ctx.tx.closing_date()?.is_some_and(|c| date <= c) {
        return Err(rejected(format!("{date} is in the closed period")));
    }
    Ok(())
}

// ----- invoices -----------------------------------------------------------------------

fn invoice_create(ctx: &Ctx<'_>, a: DocArgs) -> CommandResult<Value> {
    let status = a.status.clone().unwrap_or_else(|| "open".into());
    if !matches!(status.as_str(), "draft" | "open") {
        return Err(invalid("status is draft or open"));
    }
    let mut d = build_doc(ctx, "invoice", a, None)?;
    d.status = status;
    if d.status == "open" {
        post_doc(ctx, &mut d)?;
    }
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DocUpdateArgs {
    /// Id or number.
    #[serde(alias = "estimate", alias = "credit_memo")]
    invoice: String,
    #[serde(flatten)]
    rest: DocArgs,
}

fn invoice_update(ctx: &Ctx<'_>, a: DocUpdateArgs) -> CommandResult<Value> {
    let old = find_doc(ctx, "invoice", &a.invoice)?;
    if old.status == "void" {
        return Err(rejected("a void invoice cannot change"));
    }
    if a.rest.status.is_some() {
        return Err(invalid("use invoice.finalize to open a draft"));
    }
    let mut d = build_doc(ctx, "invoice", a.rest, Some(old.clone()))?;
    let l = Ledgerish::load(ctx)?;
    let applied = l.applied_to(&d.id, None);
    if applied > d.totals.total && d.totals.total > Decimal::ZERO {
        return Err(rejected(
            "the new total is less than the payments applied; remove some first",
        ));
    }
    if d.status != "draft" {
        require_open_period(ctx, old.date)?;
        post_doc(ctx, &mut d)?;
    }
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InvoiceRef {
    #[serde(alias = "estimate", alias = "credit_memo")]
    invoice: String,
}

fn invoice_finalize(ctx: &Ctx<'_>, a: InvoiceRef) -> CommandResult<Value> {
    let mut d = find_doc(ctx, "invoice", &a.invoice)?;
    if d.status != "draft" {
        return Err(rejected("only a draft invoice can be finalised"));
    }
    d.status = "open".into();
    post_doc(ctx, &mut d)?;
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

fn invoice_void(ctx: &Ctx<'_>, a: InvoiceRef) -> CommandResult<Value> {
    let mut d = find_doc(ctx, "invoice", &a.invoice)?;
    if d.status == "void" {
        return Err(rejected("the invoice is already void"));
    }
    let l = Ledgerish::load(ctx)?;
    if l.applied_to(&d.id, None) != Decimal::ZERO {
        return Err(rejected(
            "payments or credits are applied to this invoice; remove them first",
        ));
    }
    if let Some(id) = d.txn
        && let Some(old) = ctx.tx.transaction(id)?
    {
        let mut new = old.clone();
        new.status = TxnStatus::Voided;
        new.void_reason = Some(format!("Invoice {} voided", d.number));
        validate_change(ctx, Some(&old), Some(&new))?;
        ctx.save_txn(Some(&old), &new)?;
    }
    d.status = "void".into();
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

fn invoice_delete(ctx: &Ctx<'_>, a: InvoiceRef) -> CommandResult<Value> {
    let d = find_doc(ctx, "invoice", &a.invoice)?;
    let l = Ledgerish::load(ctx)?;
    if l.applied_to(&d.id, None) != Decimal::ZERO {
        return Err(rejected(
            "payments or credits are applied; remove them first",
        ));
    }
    remove_txn(ctx, d.txn)?;
    ctx.save_doc(DocTable::SalesDoc, &d.id, None)?;
    if let Some(e) = &d.linked
        && let Ok(mut est) = find_doc(ctx, "estimate", e)
    {
        est.status = "accepted".into();
        est.linked = None;
        save_doc(ctx, &est)?;
    }
    Ok(json!({"deleted": d.id, "number": d.number}))
}

fn invoice_get(ctx: &Ctx<'_>, a: InvoiceRef) -> CommandResult<Value> {
    doc_json(ctx, &find_doc(ctx, "invoice", &a.invoice)?)
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DocListArgs {
    #[serde(default)]
    customer: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    from: Option<Date>,
    #[serde(default)]
    to: Option<Date>,
    /// Only overdue invoices (as of today, or `as_of`).
    #[serde(default)]
    overdue: bool,
    #[serde(default)]
    as_of: Option<Date>,
}

fn list_docs(ctx: &Ctx<'_>, kind: &str, a: DocListArgs) -> CommandResult<Value> {
    let customer = a
        .customer
        .as_deref()
        .map(|c| find_party(ctx, c))
        .transpose()?;
    let today = a.as_of.map_or_else(mk_store::today_utc, Ok)?;
    let l = Ledgerish::load(ctx)?;
    let all = parties(ctx)?;
    let mut out = Vec::new();
    for d in docs(ctx, kind)?.iter().rev() {
        if customer.as_ref().is_some_and(|c| c.id != d.customer) {
            continue;
        }
        if a.from.is_some_and(|f| d.date < f) || a.to.is_some_and(|t| d.date > t) {
            continue;
        }
        let (balance, status) = match kind {
            "invoice" => {
                let (_, b, s) = invoice_state(d, &l, None);
                (Some(b), s)
            }
            "estimate" => (None, estimate_status(d, today)),
            _ => (Some(l.unapplied_credit(d, None)), d.status.clone()),
        };
        if a.status.as_ref().is_some_and(|s| *s != status) {
            continue;
        }
        let overdue = kind == "invoice"
            && matches!(status.as_str(), "open" | "partial")
            && d.due_date.is_some_and(|x| today > x);
        if a.overdue && !overdue {
            continue;
        }
        out.push(json!({
            "id": d.id, "number": d.number, "customer": d.customer,
            "customer_name": all.iter().find(|p| p.id == d.customer).map(|p| p.name.clone()),
            "date": d.date, "due_date": d.due_date, "expires": d.expires,
            "total": money(d.totals.total),
            "balance": balance.map(money),
            "status": status, "overdue": overdue,
        }));
    }
    Ok(Value::Array(out))
}

fn invoice_list(ctx: &Ctx<'_>, a: DocListArgs) -> CommandResult<Value> {
    list_docs(ctx, "invoice", a)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DuplicateArgs {
    #[serde(alias = "estimate")]
    invoice: String,
    #[serde(default)]
    date: Option<Date>,
}

fn duplicate(ctx: &Ctx<'_>, kind: &str, a: DuplicateArgs) -> CommandResult<Value> {
    let src = find_doc(ctx, kind, &a.invoice)?;
    let args = DocArgs {
        customer: Some(src.customer.clone()),
        date: Some(a.date.unwrap_or(src.date)),
        terms: src.terms.clone(),
        lines: Some(src.lines.clone()),
        tax_rate: Some(src.tax_rate.clone().unwrap_or_default()),
        memo: src.memo.clone(),
        message: src.message.clone(),
        class: src.tags.class.clone(),
        location: src.tags.location.clone(),
        fund: src.tags.fund.clone(),
        project: src.tags.project.clone(),
        ..DocArgs::default()
    };
    // A copy does not bill the same costs again.
    let mut args = args;
    if let Some(lines) = args.lines.as_mut() {
        for l in lines {
            l.billable_items.clear();
        }
    }
    let d = build_doc(ctx, kind, args, None)?;
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

fn invoice_duplicate(ctx: &Ctx<'_>, a: DuplicateArgs) -> CommandResult<Value> {
    duplicate(ctx, "invoice", a)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PreviewArgs {
    /// Preview an edit of this document (id or number) instead of a new one.
    #[serde(default)]
    #[serde(alias = "estimate")]
    invoice: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(flatten)]
    rest: DocArgs,
}

/// Totals for unsaved data: run in a read transaction (always rolled back).
fn invoice_preview(ctx: &Ctx<'_>, a: PreviewArgs) -> CommandResult<Value> {
    let kind = a.kind.unwrap_or_else(|| "invoice".into());
    let base = a
        .invoice
        .as_deref()
        .map(|r| find_doc(ctx, &kind, r))
        .transpose()?;
    let mut args = a.rest;
    if base.is_some() {
        args.number = None;
    }
    let d = build(ctx, &kind, args, base, false)?;
    let chart = Chart::load(ctx)?;
    Ok(json!({
        "number": d.number, "due_date": d.due_date, "expires": d.expires,
        "terms_label": d.terms.as_ref().map(terms_label),
        "tax_rate": d.tax_rate,
        "totals": totals_json(&chart, &d.totals),
    }))
}

/// The book's business name and address lines for PDFs.
pub(crate) fn business_header(ctx: &Ctx<'_>) -> (String, Vec<String>) {
    (
        ms::get_str(ctx, "invoicing.business_name").unwrap_or_default(),
        ms::get_str(ctx, "invoicing.business_address")
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .chain([
                ms::get_str(ctx, "invoicing.business_email").unwrap_or_default(),
                ms::get_str(ctx, "invoicing.business_phone").unwrap_or_default(),
            ])
            .filter(|s| !s.trim().is_empty())
            .collect(),
    )
}

/// The `{file_name, content_base64, warnings}` answer for a rendered PDF.
pub(crate) fn pdf_answer(file: &str, bytes: Vec<u8>, replaced: &[char]) -> Value {
    let mut warnings = Vec::new();
    if !replaced.is_empty() {
        warnings.push(format!(
            "these characters cannot be printed with the standard PDF fonts and were replaced with '?': {}",
            replaced.iter().collect::<String>()
        ));
    }
    use base64::Engine;
    json!({
        "file_name": format!("{}.pdf", file.replace(['/', '\\', ':'], "-")),
        "content_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
        "warnings": warnings,
    })
}

fn render(ctx: &Ctx<'_>, d: &SalesDoc, template: Option<&str>) -> CommandResult<Value> {
    let customer = find_party(ctx, &d.customer)?;
    let l = Ledgerish::load(ctx)?;
    let (applied, balance, _) = invoice_state(d, &l, None);
    let paper = ms::get_str(ctx, "invoicing.paper").unwrap_or_default();
    let tpl = crate::templates::for_document(ctx, template.or(d.template.as_deref()))?;
    let show = tpl.as_ref().map(|t| t.show.clone()).unwrap_or_default();
    let (kind, default_title) = match d.kind.as_str() {
        "estimate" => ("estimate", "Estimate"),
        "credit_memo" => ("credit_memo", "Credit memo"),
        _ => ("invoice", "Invoice"),
    };
    let title = crate::templates::title(tpl.as_ref(), kind, default_title);
    let mut facts = vec![
        (format!("{default_title} number"), d.number.clone()),
        ("Date".to_owned(), d.date.to_string()),
    ];
    match d.kind.as_str() {
        "estimate" => {
            if let Some(e) = d.expires {
                facts.push(("Valid until".into(), e.to_string()));
            }
        }
        "invoice" if show.due_date => {
            if let Some(e) = d.due_date {
                facts.push(("Due date".into(), e.to_string()));
            }
        }
        _ => {}
    }
    if show.terms
        && let Some(t) = &d.terms
    {
        facts.push(("Terms".into(), terms_label(t)));
    }
    let skus: BTreeMap<String, String> = if show.sku && module_on(ctx, ITEMS)? {
        items(ctx)?
            .into_iter()
            .filter_map(|i| i.sku.map(|s| (i.id, s)))
            .collect()
    } else {
        BTreeMap::new()
    };
    let (business_name, business_lines) = business_header(ctx);
    let mut notes = Vec::new();
    if show.payment_instructions
        && d.kind == "invoice"
        && let Some(p) = tpl.as_ref().and_then(|t| t.payment_instructions.clone())
    {
        notes.push(p);
    }
    let doc = crate::pdf::SalesPdf {
        title,
        business_name,
        business_lines,
        number: d.number.clone(),
        date: d.date.to_string(),
        facts: Some(facts),
        bill_to: std::iter::once(customer.name.clone())
            .chain(customer.company.clone())
            .chain(customer.billing_address.iter().cloned())
            .chain(customer.email.clone().filter(|_| show.customer_email))
            .collect(),
        lines: d
            .totals
            .lines
            .iter()
            .map(|x| crate::pdf::PdfLine {
                description: match x.item.as_ref().and_then(|i| skus.get(i)) {
                    Some(sku) => format!("{} (SKU {sku})", x.description),
                    None => x.description.clone(),
                },
                quantity: x
                    .quantity
                    .map(|q| q.normalize().to_string())
                    .unwrap_or_default(),
                rate: x
                    .rate
                    .filter(|_| x.kind != "subtotal")
                    .map(fmt_rate)
                    .unwrap_or_default(),
                amount: group(&money(x.amount)),
                emphasis: x.kind == "subtotal",
            })
            .collect(),
        hide_qty_rate: !show.quantity_rate,
        subtotal: group(&money(d.totals.subtotal)),
        taxes: d
            .totals
            .tax
            .iter()
            .map(|t| {
                (
                    format!("{} ({}%)", t.name, t.percent.normalize()),
                    group(&money(t.amount)),
                )
            })
            .collect(),
        total: group(&money(d.totals.total)),
        applied: (d.kind == "invoice").then(|| group(&money(applied))),
        balance: (d.kind == "invoice").then(|| group(&money(balance))),
        exemption: d.exempt.then(|| {
            format!(
                "Tax exempt{}",
                d.exemption_note
                    .as_ref()
                    .map(|n| format!(": {n}"))
                    .unwrap_or_default()
            )
        }),
        message: d.message.clone().unwrap_or_default(),
        notes,
        a4: paper == "a4",
        style: crate::templates::style(tpl.as_ref()),
        ..crate::pdf::SalesPdf::default()
    };
    let (bytes, replaced) = crate::pdf::render(&doc);
    Ok(pdf_answer(&d.number, bytes, &replaced))
}

pub(crate) fn fmt_rate(r: Decimal) -> String {
    let r = r.normalize();
    if r.scale() < 2 {
        group(&mk_core::currency::format_amount(r, Some(2)))
    } else {
        group(&r.to_string())
    }
}

/// Thousands separators for an amount text.
pub(crate) fn group(s: &str) -> String {
    let (sign, rest) = s.strip_prefix('-').map_or(("", s), |r| ("-", r));
    let (int, frac) = rest
        .split_once('.')
        .map_or((rest, None), |(a, b)| (a, Some(b)));
    let mut out = String::new();
    let n = int.len();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (n - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    match frac {
        Some(f) => format!("{sign}{out}.{f}"),
        None => format!("{sign}{out}"),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PdfArgs {
    #[serde(alias = "estimate", alias = "credit_memo")]
    invoice: String,
    /// A template (id or name) instead of the document's or the default.
    #[serde(default)]
    template: Option<String>,
}

fn invoice_pdf(ctx: &Ctx<'_>, a: PdfArgs) -> CommandResult<Value> {
    render(
        ctx,
        &find_doc(ctx, "invoice", &a.invoice)?,
        a.template.as_deref(),
    )
}

fn credit_pdf(ctx: &Ctx<'_>, a: PdfArgs) -> CommandResult<Value> {
    render(
        ctx,
        &find_doc(ctx, "credit_memo", &a.invoice)?,
        a.template.as_deref(),
    )
}

// ----- payments -----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Payment {
    pub id: String,
    pub customer: String,
    pub date: Date,
    pub amount: Decimal,
    pub method: String,
    #[serde(default)]
    pub reference: Option<String>,
    pub deposit_to: AccountId,
    pub txn: TransactionId,
    pub applied: Vec<Application>,
    #[serde(default)]
    pub deposit: Option<String>,
    #[serde(default)]
    pub memo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RefundSource {
    pub source: String,
    pub amount: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Refund {
    pub id: String,
    pub customer: String,
    pub date: Date,
    pub amount: Decimal,
    pub from_account: AccountId,
    pub txn: TransactionId,
    pub sources: Vec<RefundSource>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TaxPaid {
    pub rate: String,
    pub name: String,
    pub amount: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TaxPayment {
    pub id: String,
    pub date: Date,
    pub from_account: AccountId,
    pub amounts: Vec<TaxPaid>,
    pub txn: TransactionId,
    /// The agency vendor paid, when every rate paid names the same one.
    #[serde(default)]
    pub vendor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Deposit {
    pub id: String,
    pub bank: AccountId,
    pub date: Date,
    pub payments: Vec<String>,
    pub lines: Vec<DepositLine>,
    pub total: Decimal,
    pub txn: TransactionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DepositLine {
    pub account: String,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    pub amount: Decimal,
    #[serde(default)]
    pub memo: Option<String>,
}

fn payments(ctx: &Ctx<'_>) -> CommandResult<Vec<Payment>> {
    let mut v: Vec<Payment> = load(ctx, DocTable::Payment, Some("payment"))?;
    v.sort_by_key(|p| (p.date, p.id.clone()));
    Ok(v)
}

fn find_payment(ctx: &Ctx<'_>, id: &str) -> CommandResult<Payment> {
    payments(ctx)?
        .into_iter()
        .find(|p| p.id == id.trim())
        .ok_or_else(|| CommandError::NotFound(format!("payment {id:?}")))
}

fn save_payment(ctx: &Ctx<'_>, p: &Payment) -> CommandResult<()> {
    save(
        ctx,
        DocTable::Payment,
        &p.id,
        Some("payment"),
        date_ord(p.date),
        p,
    )
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApplyArg {
    /// Invoice id or number.
    invoice: String,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
}

/// Check and resolve applications for a customer, up to `available`.
fn applications(
    ctx: &Ctx<'_>,
    customer: &str,
    given: Option<Vec<ApplyArg>>,
    available: Decimal,
    date: Date,
) -> CommandResult<Vec<Application>> {
    let l = Ledgerish::load(ctx)?;
    let open: Vec<(SalesDoc, Decimal)> = docs(ctx, "invoice")?
        .into_iter()
        .filter(|d| d.customer == customer && !matches!(d.status.as_str(), "draft" | "void"))
        .map(|d| {
            let (_, b, _) = invoice_state(&d, &l, None);
            (d, b)
        })
        .filter(|(_, b)| *b > Decimal::ZERO)
        .collect();
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
                        d.id == g.invoice || d.number.eq_ignore_ascii_case(g.invoice.trim())
                    })
                    .ok_or_else(|| {
                        invalid(format!(
                            "{} is not an open invoice of this customer",
                            g.invoice
                        ))
                    })?;
                if g.amount > *bal {
                    return Err(invalid(format!(
                        "{}: {} is more than its balance {}",
                        d.number,
                        money(g.amount),
                        money(*bal)
                    )));
                }
                total = add(total, g.amount)?;
                out.push(Application {
                    invoice: d.id.clone(),
                    amount: g.amount,
                    date: date.max(d.date),
                });
            }
            if total > available {
                return Err(invalid(format!(
                    "{} applied is more than the {} available",
                    money(total),
                    money(available)
                )));
            }
        }
        None => {
            let mut left = available;
            for (d, bal) in open {
                if left <= Decimal::ZERO {
                    break;
                }
                let take = bal.min(left);
                left = sub(left, take)?;
                out.push(Application {
                    invoice: d.id.clone(),
                    amount: take,
                    date: date.max(d.date),
                });
            }
        }
    }
    Ok(out)
}

fn payment_json(ctx: &Ctx<'_>, p: &Payment) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let l = Ledgerish::load(ctx)?;
    let invs = docs(ctx, "invoice")?;
    let c = find_party(ctx, &p.customer).ok();
    Ok(json!({
        "id": p.id, "customer": p.customer, "customer_name": c.map(|c| c.name),
        "date": p.date, "amount": money(p.amount), "method": p.method,
        "reference": p.reference, "deposit_to": p.deposit_to,
        "deposit_to_name": chart.full_name(p.deposit_to), "txn": p.txn,
        "applied": p.applied.iter().map(|a| json!({
            "invoice": a.invoice,
            "number": invs.iter().find(|d| d.id == a.invoice).map(|d| d.number.clone()),
            "amount": money(a.amount), "date": a.date,
        })).collect::<Vec<_>>(),
        "unapplied": money(l.unapplied_payment(p, None)),
        "deposit": p.deposit,
        "deposited": p.deposit.is_some() || chart.get(p.deposit_to).is_some_and(|a| a.subtype != AccountSubtype::Clearing),
        "memo": p.memo,
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReceiveArgs {
    customer: String,
    date: Date,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
    /// cash, check, card, bank_transfer or other.
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    reference: Option<String>,
    /// Bank, cash or Payments to Deposit (default from settings).
    #[serde(default)]
    deposit_to: Option<String>,
    /// Applications; default: oldest open invoices first.
    #[serde(default)]
    apply: Option<Vec<ApplyArg>>,
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

fn deposit_account(ctx: &Ctx<'_>, given: Option<&str>) -> CommandResult<AccountId> {
    let chart = Chart::load(ctx)?;
    let r = match given {
        Some(r) => Some(r.to_owned()),
        None => ms::get(ctx, "invoicing.deposit_to")?
            .as_str()
            .map(str::to_owned),
    };
    match r {
        Some(r) => {
            let a = chart.resolve(&r)?;
            if !matches!(
                a.subtype,
                AccountSubtype::Bank | AccountSubtype::Cash | AccountSubtype::Clearing
            ) {
                return Err(invalid(format!(
                    "{} is not a bank, cash or clearing account",
                    chart.full_name(a.id)
                )));
            }
            Ok(a.id)
        }
        None => clearing(ctx),
    }
}

fn payment_receive(ctx: &Ctx<'_>, a: ReceiveArgs) -> CommandResult<Value> {
    let c = find_party(ctx, &a.customer)?;
    if a.amount <= Decimal::ZERO || decimals(a.amount) > 2 {
        return Err(invalid("the amount is positive with at most 2 decimals"));
    }
    let method = a.method.unwrap_or_else(|| "check".into());
    if !matches!(
        method.as_str(),
        "cash" | "check" | "card" | "bank_transfer" | "other"
    ) {
        return Err(invalid(
            "method is cash, check, card, bank_transfer or other",
        ));
    }
    let deposit_to = deposit_account(ctx, a.deposit_to.as_deref())?;
    let applied = applications(ctx, &c.id, a.apply, a.amount, a.date)?;
    let ar = receivable(ctx)?;
    let reference = text(a.reference, 40, "reference")?;
    let txn = post_txn(
        ctx,
        None,
        a.date,
        &c.name,
        Some(format!(
            "Payment{}",
            reference
                .as_ref()
                .map(|r| format!(" {r}"))
                .unwrap_or_default()
        )),
        {
            let tags = crate::dimensions::resolve_tags(
                ctx,
                a.class.as_deref(),
                a.location.as_deref(),
                a.fund.as_deref(),
            )?;
            let mut v = vec![cash(deposit_to, a.amount, None), cash(ar, -a.amount, None)];
            for sp in &mut v {
                sp.tags = tags.clone();
            }
            v
        },
    )?;
    let p = Payment {
        id: new_id(),
        customer: c.id,
        date: a.date,
        amount: a.amount,
        method,
        reference,
        deposit_to,
        txn,
        applied,
        deposit: None,
        memo: text(a.memo, 1000, "memo")?,
    };
    save_payment(ctx, &p)?;
    payment_json(ctx, &p)
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
    let l = Ledgerish::load(ctx)?;
    let available = l.unapplied_payment(&p, None);
    let more = applications(
        ctx,
        &p.customer,
        a.apply,
        available,
        a.date.unwrap_or(p.date),
    )?;
    p.applied.extend(more);
    save_payment(ctx, &p)?;
    payment_json(ctx, &p)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UnapplyArgs {
    #[serde(alias = "credit_memo")]
    payment: String,
    invoice: String,
}

fn payment_unapply(ctx: &Ctx<'_>, a: UnapplyArgs) -> CommandResult<Value> {
    let mut p = find_payment(ctx, &a.payment)?;
    let inv = find_doc(ctx, "invoice", &a.invoice)?;
    let before = p.applied.len();
    p.applied.retain(|x| x.invoice != inv.id);
    if p.applied.len() == before {
        return Err(invalid(format!(
            "the payment is not applied to {}",
            inv.number
        )));
    }
    save_payment(ctx, &p)?;
    payment_json(ctx, &p)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PaymentRef {
    payment: String,
}

fn payment_delete(ctx: &Ctx<'_>, a: PaymentRef) -> CommandResult<Value> {
    let p = find_payment(ctx, &a.payment)?;
    if p.deposit.is_some() {
        return Err(rejected(
            "the payment is in a deposit; remove it from the deposit first",
        ));
    }
    let l = Ledgerish::load(ctx)?;
    if l.refunded_from(&p.id, None) > Decimal::ZERO {
        return Err(rejected("part of this payment was refunded"));
    }
    remove_txn(ctx, Some(p.txn))?;
    ctx.save_doc(DocTable::Payment, &p.id, None)?;
    Ok(json!({"deleted": p.id}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OpenInvoicesArgs {
    customer: String,
    /// The payment amount to spread (default 0).
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    amount: Option<Decimal>,
    #[serde(default)]
    date: Option<Date>,
}

fn payment_open_invoices(ctx: &Ctx<'_>, a: OpenInvoicesArgs) -> CommandResult<Value> {
    let c = find_party(ctx, &a.customer)?;
    let amount = a.amount.unwrap_or_default().max(Decimal::ZERO);
    let date = a.date.map_or_else(mk_store::today_utc, Ok)?;
    let suggested = applications(ctx, &c.id, None, amount, date)?;
    let l = Ledgerish::load(ctx)?;
    let mut rows = Vec::new();
    let mut applied = Decimal::ZERO;
    for d in docs(ctx, "invoice")?
        .iter()
        .filter(|d| d.customer == c.id && !matches!(d.status.as_str(), "draft" | "void"))
    {
        let (_, bal, _) = invoice_state(d, &l, None);
        if bal <= Decimal::ZERO {
            continue;
        }
        let apply = suggested
            .iter()
            .find(|x| x.invoice == d.id)
            .map_or(Decimal::ZERO, |x| x.amount);
        applied = add(applied, apply)?;
        rows.push(
            json!({"id": d.id, "number": d.number, "date": d.date, "due_date": d.due_date,
            "total": money(d.totals.total), "balance": money(bal), "apply": money(apply)}),
        );
    }
    let credits = credits_of(ctx, &c.id, None)?
        .iter()
        .try_fold(Decimal::ZERO, |s, x| add(s, x.2))?;
    Ok(
        json!({"customer": c.id, "invoices": rows, "applied": money(applied),
        "credit_left": money(sub(amount, applied)?), "existing_credit": money(credits)}),
    )
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DepositPreviewArgs {
    payments: Vec<String>,
    #[serde(default)]
    other_lines: Vec<DepositLine>,
}

fn deposit_preview(ctx: &Ctx<'_>, a: DepositPreviewArgs) -> CommandResult<Value> {
    let mut total = Decimal::ZERO;
    for r in &a.payments {
        total = add(total, find_payment(ctx, r)?.amount)?;
    }
    for l in &a.other_lines {
        total = add(total, l.amount)?;
    }
    Ok(json!({"total": money(total), "count": a.payments.len()}))
}

fn payment_get(ctx: &Ctx<'_>, a: PaymentRef) -> CommandResult<Value> {
    payment_json(ctx, &find_payment(ctx, &a.payment)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PaymentListArgs {
    #[serde(default)]
    customer: Option<String>,
    /// Only payments waiting in Payments to Deposit.
    #[serde(default)]
    undeposited: bool,
}

fn payment_list(ctx: &Ctx<'_>, a: PaymentListArgs) -> CommandResult<Value> {
    let c = a
        .customer
        .as_deref()
        .map(|r| find_party(ctx, r))
        .transpose()?;
    let chart = Chart::load(ctx)?;
    let mut out = Vec::new();
    for p in payments(ctx)?.iter().rev() {
        if c.as_ref().is_some_and(|c| c.id != p.customer) {
            continue;
        }
        let waiting = p.deposit.is_none()
            && chart
                .get(p.deposit_to)
                .is_some_and(|x| x.subtype == AccountSubtype::Clearing);
        if a.undeposited && !waiting {
            continue;
        }
        out.push(payment_json(ctx, p)?);
    }
    Ok(Value::Array(out))
}

// ----- deposits -----------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DepositArgs {
    bank_account: String,
    date: Date,
    payments: Vec<String>,
    #[serde(default)]
    other_lines: Vec<DepositLine>,
    #[serde(default)]
    memo: Option<String>,
    /// Class, location and fund for the deposit's splits.
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    fund: Option<String>,
}

fn deposit_create(ctx: &Ctx<'_>, a: DepositArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let bank = chart.resolve(&a.bank_account)?;
    if !matches!(bank.subtype, AccountSubtype::Bank | AccountSubtype::Cash) {
        return Err(invalid(format!(
            "{} is not a bank or cash account",
            chart.full_name(bank.id)
        )));
    }
    let bank = bank.id;
    if a.payments.is_empty() && a.other_lines.is_empty() {
        return Err(invalid("choose the payments to deposit"));
    }
    let clear = clearing(ctx)?;
    let mut chosen = Vec::new();
    let mut from_clearing = Decimal::ZERO;
    for r in &a.payments {
        let p = find_payment(ctx, r)?;
        if p.deposit.is_some() {
            return Err(rejected("a chosen payment is already in a deposit"));
        }
        if p.deposit_to != clear {
            return Err(rejected("a chosen payment went straight to a bank account"));
        }
        if p.date > a.date {
            return Err(invalid("a payment is dated after the deposit"));
        }
        from_clearing = add(from_clearing, p.amount)?;
        chosen.push(p);
    }
    let mut splits = vec![cash(clear, -from_clearing, None)];
    let mut total = from_clearing;
    for l in &a.other_lines {
        let acc = chart.resolve(&l.account)?;
        if decimals(l.amount) > 2 {
            return Err(invalid("deposit lines have at most 2 decimals"));
        }
        splits.push(cash(acc.id, -l.amount, text(l.memo.clone(), 200, "memo")?));
        total = add(total, l.amount)?;
    }
    splits.push(cash(bank, total, None));
    let tags = crate::dimensions::resolve_tags(
        ctx,
        a.class.as_deref(),
        a.location.as_deref(),
        a.fund.as_deref(),
    )?;
    for sp in &mut splits {
        sp.tags = tags.clone();
    }
    let id = new_id();
    let txn = post_txn(
        ctx,
        None,
        a.date,
        "Deposit",
        text(a.memo, 1000, "memo")?.or(Some(format!("Deposit of {} payments", chosen.len()))),
        splits,
    )?;
    for p in &mut chosen {
        p.deposit = Some(id.clone());
        save_payment(ctx, p)?;
    }
    let d = Deposit {
        id: id.clone(),
        bank,
        date: a.date,
        payments: chosen.iter().map(|p| p.id.clone()).collect(),
        lines: a.other_lines,
        total,
        txn,
    };
    save(ctx, DocTable::Deposit, &id, None, date_ord(a.date), &d)?;
    Ok(json!({"id": id, "txn": txn, "total": money(total), "payments": d.payments}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DepositRef {
    deposit: String,
}

fn deposit_delete(ctx: &Ctx<'_>, a: DepositRef) -> CommandResult<Value> {
    let all: Vec<Deposit> = load(ctx, DocTable::Deposit, None)?;
    let d = all
        .into_iter()
        .find(|d| d.id == a.deposit.trim())
        .ok_or_else(|| CommandError::NotFound(format!("deposit {:?}", a.deposit)))?;
    remove_txn(ctx, Some(d.txn))?;
    for pid in &d.payments {
        if let Ok(mut p) = find_payment(ctx, pid) {
            p.deposit = None;
            save_payment(ctx, &p)?;
        }
    }
    ctx.save_doc(DocTable::Deposit, &d.id, None)?;
    Ok(json!({"deleted": d.id}))
}

fn deposit_list(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let mut all: Vec<Deposit> = load(ctx, DocTable::Deposit, None)?;
    all.sort_by_key(|d| std::cmp::Reverse(d.date));
    Ok(Value::Array(
        all.iter()
            .map(|d| json!({"id": d.id, "date": d.date, "bank": d.bank, "bank_name": chart.full_name(d.bank), "total": money(d.total), "payments": d.payments, "txn": d.txn}))
            .collect(),
    ))
}

// ----- credit memos -------------------------------------------------------------------

fn credit_create(ctx: &Ctx<'_>, a: DocArgs) -> CommandResult<Value> {
    let mut d = build_doc(ctx, "credit_memo", a, None)?;
    d.status = "open".into();
    post_doc(ctx, &mut d)?;
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreditApplyArgs {
    credit_memo: String,
    #[serde(default)]
    apply: Option<Vec<ApplyArg>>,
    #[serde(default)]
    date: Option<Date>,
}

fn credit_apply(ctx: &Ctx<'_>, a: CreditApplyArgs) -> CommandResult<Value> {
    let mut c = find_doc(ctx, "credit_memo", &a.credit_memo)?;
    let l = Ledgerish::load(ctx)?;
    let available = l.unapplied_credit(&c, None);
    let more = applications(
        ctx,
        &c.customer,
        a.apply,
        available,
        a.date.unwrap_or(c.date),
    )?;
    c.applied.extend(more);
    save_doc(ctx, &c)?;
    doc_json(ctx, &c)
}

fn credit_unapply(ctx: &Ctx<'_>, a: UnapplyArgs) -> CommandResult<Value> {
    let mut c = find_doc(ctx, "credit_memo", &a.payment)?;
    let inv = find_doc(ctx, "invoice", &a.invoice)?;
    let before = c.applied.len();
    c.applied.retain(|x| x.invoice != inv.id);
    if c.applied.len() == before {
        return Err(invalid(format!(
            "the credit memo is not applied to {}",
            inv.number
        )));
    }
    save_doc(ctx, &c)?;
    doc_json(ctx, &c)
}

fn credit_delete(ctx: &Ctx<'_>, a: InvoiceRef) -> CommandResult<Value> {
    let c = find_doc(ctx, "credit_memo", &a.invoice)?;
    let l = Ledgerish::load(ctx)?;
    if !c.applied.is_empty() || l.refunded_from(&c.id, None) > Decimal::ZERO {
        return Err(rejected(
            "the credit memo is applied or refunded; remove that first",
        ));
    }
    remove_txn(ctx, c.txn)?;
    ctx.save_doc(DocTable::SalesDoc, &c.id, None)?;
    Ok(json!({"deleted": c.id}))
}

fn credit_get(ctx: &Ctx<'_>, a: InvoiceRef) -> CommandResult<Value> {
    doc_json(ctx, &find_doc(ctx, "credit_memo", &a.invoice)?)
}

fn credit_list(ctx: &Ctx<'_>, a: DocListArgs) -> CommandResult<Value> {
    list_docs(ctx, "credit_memo", a)
}

// ----- refunds ------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RefundArgs {
    customer: String,
    from_account: String,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
    date: Date,
    #[serde(default)]
    memo: Option<String>,
}

/// A customer's unapplied credits, oldest first: (source id, date, amount).
fn credits_of(
    ctx: &Ctx<'_>,
    customer: &str,
    as_of: Option<Date>,
) -> CommandResult<Vec<(String, Date, Decimal)>> {
    let l = Ledgerish::load(ctx)?;
    let mut out: Vec<(String, Date, Decimal)> = l
        .payments
        .iter()
        .filter(|p| p.customer == customer)
        .map(|p| (p.id.clone(), p.date, l.unapplied_payment(p, as_of)))
        .chain(
            l.credits
                .iter()
                .filter(|c| c.customer == customer)
                .map(|c| (c.id.clone(), c.date, l.unapplied_credit(c, as_of))),
        )
        .filter(|(_, _, a)| *a > Decimal::ZERO)
        .collect();
    out.sort_by_key(|(id, d, _)| (*d, id.clone()));
    Ok(out)
}

fn refund(ctx: &Ctx<'_>, a: RefundArgs) -> CommandResult<Value> {
    let c = find_party(ctx, &a.customer)?;
    if a.amount <= Decimal::ZERO || decimals(a.amount) > 2 {
        return Err(invalid("the amount is positive with at most 2 decimals"));
    }
    let chart = Chart::load(ctx)?;
    let from = chart.resolve(&a.from_account)?.id;
    let credits = credits_of(ctx, &c.id, None)?;
    let available = credits.iter().try_fold(Decimal::ZERO, |s, x| add(s, x.2))?;
    if a.amount > available {
        return Err(rejected(format!(
            "{} has {} of credit; the refund is {}",
            c.name,
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
    let ar = receivable(ctx)?;
    let txn = post_txn(
        ctx,
        None,
        a.date,
        &c.name,
        text(a.memo, 1000, "memo")?.or(Some("Refund".into())),
        vec![cash(ar, a.amount, None), cash(from, -a.amount, None)],
    )?;
    let r = Refund {
        id: new_id(),
        customer: c.id,
        date: a.date,
        amount: a.amount,
        from_account: from,
        txn,
        sources,
    };
    save(
        ctx,
        DocTable::Payment,
        &r.id,
        Some("refund"),
        date_ord(r.date),
        &r,
    )?;
    Ok(json!({"id": r.id, "txn": txn, "amount": money(r.amount)}))
}

fn customer_summary(ctx: &Ctx<'_>, a: PartyRef) -> CommandResult<Value> {
    let c = find_party(ctx, &a.party)?;
    let l = Ledgerish::load(ctx)?;
    let today = mk_store::today_utc()?;
    let mut open = Vec::new();
    let mut balance = Decimal::ZERO;
    let mut overdue = Decimal::ZERO;
    for d in docs(ctx, "invoice")?.iter().filter(|d| d.customer == c.id) {
        let (_, b, s) = invoice_state(d, &l, None);
        if matches!(s.as_str(), "open" | "partial") {
            balance = add(balance, b)?;
            if d.due_date.is_some_and(|x| today > x) {
                overdue = add(overdue, b)?;
            }
            open.push(json!({"id": d.id, "number": d.number, "date": d.date, "due_date": d.due_date, "total": money(d.totals.total), "balance": money(b), "status": s}));
        }
    }
    let credits = credits_of(ctx, &c.id, None)?;
    let credit = credits.iter().try_fold(Decimal::ZERO, |s, x| add(s, x.2))?;
    let pays: Vec<Value> = l
        .payments
        .iter()
        .filter(|p| p.customer == c.id)
        .map(|p| payment_json(ctx, p))
        .collect::<CommandResult<_>>()?;
    Ok(json!({
        "customer": party_json(ctx, &c)?,
        "open_invoices": open,
        "balance": money(balance),
        "overdue": money(overdue),
        "credit": money(credit),
        "payments": pays,
    }))
}

// ----- sales tax payments -------------------------------------------------------------

/// Per rate: (name, taxable sales, non-taxable sales, collected, paid).
pub(crate) type TaxRow = (String, Decimal, Decimal, Decimal, Decimal);

/// Tax collected and paid per component rate in a date range, by rate id.
pub(crate) fn tax_balances(
    ctx: &Ctx<'_>,
    from: Option<Date>,
    to: Date,
) -> CommandResult<BTreeMap<String, TaxRow>> {
    // (name, taxable sales, non-taxable sales, collected, paid)
    let mut out: BTreeMap<String, TaxRow> = BTreeMap::new();
    let inside = |d: Date| from.is_none_or(|f| d >= f) && d <= to;
    for kind in ["invoice", "credit_memo"] {
        let sign = if kind == "credit_memo" {
            Decimal::NEGATIVE_ONE
        } else {
            Decimal::ONE
        };
        for d in docs(ctx, kind)? {
            if matches!(d.status.as_str(), "draft" | "void") || !inside(d.date) {
                continue;
            }
            for t in &d.totals.tax {
                let e = out.entry(t.rate.clone()).or_insert((
                    t.name.clone(),
                    Decimal::ZERO,
                    Decimal::ZERO,
                    Decimal::ZERO,
                    Decimal::ZERO,
                ));
                e.1 = add(e.1, mul(t.base, sign)?)?;
                e.2 = add(e.2, mul(d.totals.nontaxable, sign)?)?;
                e.3 = add(e.3, mul(t.amount, sign)?)?;
            }
        }
    }
    let paid: Vec<TaxPayment> = load(ctx, DocTable::Payment, Some("tax_payment"))?;
    for p in paid.iter().filter(|p| inside(p.date)) {
        for x in &p.amounts {
            let e = out.entry(x.rate.clone()).or_insert((
                x.name.clone(),
                Decimal::ZERO,
                Decimal::ZERO,
                Decimal::ZERO,
                Decimal::ZERO,
            ));
            e.4 = add(e.4, x.amount)?;
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaxPayArg {
    rate: String,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TaxPayArgs {
    from_account: String,
    date: Date,
    /// Amounts per rate; default: each rate's balance as of `as_of` (default `date`).
    #[serde(default)]
    amounts: Option<Vec<TaxPayArg>>,
    #[serde(default)]
    as_of: Option<Date>,
}

fn tax_pay(ctx: &Ctx<'_>, a: TaxPayArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let from = chart.resolve(&a.from_account)?.id;
    let rates = tax_rates(ctx)?;
    let amounts: Vec<TaxPaid> = match a.amounts {
        Some(list) => list
            .into_iter()
            .map(|x| {
                let t = rates
                    .iter()
                    .find(|r| r.id == x.rate || r.name.eq_ignore_ascii_case(x.rate.trim()))
                    .ok_or_else(|| CommandError::NotFound(format!("tax rate {:?}", x.rate)))?;
                Ok(TaxPaid {
                    rate: t.id.clone(),
                    name: t.name.clone(),
                    amount: round2(x.amount),
                })
            })
            .collect::<CommandResult<_>>()?,
        None => tax_balances(ctx, None, a.as_of.unwrap_or(a.date))?
            .into_iter()
            .filter_map(|(id, (name, _, _, c, p))| {
                let bal = c.checked_sub(p)?;
                (bal > Decimal::ZERO).then_some(TaxPaid {
                    rate: id,
                    name,
                    amount: bal,
                })
            })
            .collect(),
    };
    let total = amounts
        .iter()
        .try_fold(Decimal::ZERO, |s, x| add(s, x.amount))?;
    if total <= Decimal::ZERO {
        return Err(rejected("there is no sales tax to pay"));
    }
    let liab = liability(ctx)?;
    let mut splits: Vec<Split> = amounts
        .iter()
        .map(|x| cash(liab, x.amount, Some(format!("Sales tax: {}", x.name))))
        .collect();
    splits.push(cash(from, -total, None));
    let vendors: Vec<Option<String>> = amounts
        .iter()
        .map(|x| {
            rates
                .iter()
                .find(|r| r.id == x.rate)
                .and_then(|r| r.agency_vendor.clone())
        })
        .collect();
    let vendor = match vendors.first() {
        Some(Some(v)) if vendors.iter().all(|x| x.as_deref() == Some(v.as_str())) => {
            find_party(ctx, v).ok()
        }
        _ => None,
    };
    let payee = vendor.as_ref().map_or("Sales tax", |v| v.name.as_str());
    let txn = post_txn(
        ctx,
        None,
        a.date,
        payee,
        Some("Sales tax payment".into()),
        splits,
    )?;
    let p = TaxPayment {
        id: new_id(),
        date: a.date,
        from_account: from,
        amounts,
        txn,
        vendor: vendor.as_ref().map(|v| v.id.clone()),
    };
    save(
        ctx,
        DocTable::Payment,
        &p.id,
        Some("tax_payment"),
        date_ord(p.date),
        &p,
    )?;
    Ok(
        json!({"id": p.id, "txn": txn, "total": money(total), "vendor": p.vendor,
              "amounts": p.amounts.iter().map(|x| json!({"rate": x.rate, "name": x.name, "amount": money(x.amount)})).collect::<Vec<_>>()}),
    )
}

// ----- estimates ----------------------------------------------------------------------

fn estimate_create(ctx: &Ctx<'_>, a: DocArgs) -> CommandResult<Value> {
    let status = a.status.clone().unwrap_or_else(|| "draft".into());
    if !matches!(status.as_str(), "draft" | "sent") {
        return Err(invalid("a new estimate is draft or sent"));
    }
    let mut d = build_doc(ctx, "estimate", DocArgs { status: None, ..a }, None)?;
    d.status = status;
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

fn estimate_update(ctx: &Ctx<'_>, a: DocUpdateArgs) -> CommandResult<Value> {
    let old = find_doc(ctx, "estimate", &a.invoice)?;
    if old.status == "converted" {
        return Err(rejected("a converted estimate cannot change"));
    }
    let d = build_doc(
        ctx,
        "estimate",
        DocArgs {
            status: None,
            ..a.rest
        },
        Some(old),
    )?;
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StatusArgs {
    estimate: String,
    /// draft, sent, accepted or declined.
    status: String,
}

fn estimate_set_status(ctx: &Ctx<'_>, a: StatusArgs) -> CommandResult<Value> {
    let mut d = find_doc(ctx, "estimate", &a.estimate)?;
    if !matches!(
        a.status.as_str(),
        "draft" | "sent" | "accepted" | "declined"
    ) {
        return Err(invalid("status is draft, sent, accepted or declined"));
    }
    if d.status == "converted" {
        return Err(rejected("the estimate was converted"));
    }
    d.status = a.status;
    save_doc(ctx, &d)?;
    doc_json(ctx, &d)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ConvertArgs {
    estimate: String,
    #[serde(default)]
    date: Option<Date>,
}

fn estimate_convert(ctx: &Ctx<'_>, a: ConvertArgs) -> CommandResult<Value> {
    let mut est = find_doc(ctx, "estimate", &a.estimate)?;
    if est.status == "converted" {
        return Err(rejected("the estimate was already converted"));
    }
    if est.status == "declined" {
        return Err(rejected("a declined estimate cannot convert"));
    }
    if ms::get_bool(ctx, "estimates.require_acceptance")? && est.status != "accepted" {
        return Err(rejected(
            "only accepted estimates convert (setting: require acceptance)",
        ));
    }
    let as_open = ms::get_str(ctx, "estimates.convert_as")? == "open";
    let args = DocArgs {
        customer: Some(est.customer.clone()),
        date: Some(a.date.map_or_else(mk_store::today_utc, Ok)?),
        lines: Some(est.lines.clone()),
        tax_rate: Some(est.tax_rate.clone().unwrap_or_default()),
        message: est.message.clone(),
        memo: est.memo.clone(),
        class: est.tags.class.clone(),
        location: est.tags.location.clone(),
        fund: est.tags.fund.clone(),
        project: est.tags.project.clone(),
        ..DocArgs::default()
    };
    let mut inv = build_doc(ctx, "invoice", args, None)?;
    inv.linked = Some(est.id.clone());
    inv.status = if as_open {
        "open".into()
    } else {
        "draft".into()
    };
    if as_open {
        post_doc(ctx, &mut inv)?;
    }
    save_doc(ctx, &inv)?;
    est.status = "converted".into();
    est.linked = Some(inv.id.clone());
    save_doc(ctx, &est)?;
    Ok(json!({"estimate": doc_json(ctx, &est)?, "invoice": doc_json(ctx, &inv)?}))
}

fn estimate_delete(ctx: &Ctx<'_>, a: InvoiceRef) -> CommandResult<Value> {
    let d = find_doc(ctx, "estimate", &a.invoice)?;
    ctx.save_doc(DocTable::SalesDoc, &d.id, None)?;
    Ok(json!({"deleted": d.id}))
}

fn estimate_get(ctx: &Ctx<'_>, a: InvoiceRef) -> CommandResult<Value> {
    doc_json(ctx, &find_doc(ctx, "estimate", &a.invoice)?)
}

fn estimate_list(ctx: &Ctx<'_>, a: DocListArgs) -> CommandResult<Value> {
    list_docs(ctx, "estimate", a)
}

fn estimate_duplicate(ctx: &Ctx<'_>, a: DuplicateArgs) -> CommandResult<Value> {
    duplicate(ctx, "estimate", a)
}

fn estimate_pdf(ctx: &Ctx<'_>, a: PdfArgs) -> CommandResult<Value> {
    render(
        ctx,
        &find_doc(ctx, "estimate", &a.invoice)?,
        a.template.as_deref(),
    )
}

/// A customer's unapplied credits and payments as of a date.
pub(crate) fn credits_total(
    ctx: &Ctx<'_>,
    customer: &str,
    as_of: Option<Date>,
) -> CommandResult<Decimal> {
    credits_of(ctx, customer, as_of)?
        .iter()
        .try_fold(Decimal::ZERO, |s, x| add(s, x.2))
}

/// An open invoice by its number: (id, customer id, balance), when it has a balance.
pub(crate) fn open_invoice_balance(
    ctx: &Ctx<'_>,
    number: &str,
) -> CommandResult<Option<(String, String, Decimal)>> {
    let l = Ledgerish::load(ctx)?;
    Ok(docs(ctx, "invoice")?
        .into_iter()
        .find(|d| d.number.eq_ignore_ascii_case(number.trim()))
        .and_then(|d| {
            let (_, balance, status) = invoice_state(&d, &l, None);
            (matches!(status.as_str(), "open" | "partial") && balance > Decimal::ZERO)
                .then(|| (d.id.clone(), d.customer.clone(), balance))
        }))
}

/// `payment.receive` from JSON arguments (processor rows applying invoices).
pub(crate) fn receive_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    payment_receive(ctx, crate::parse_args(args)?)
}
