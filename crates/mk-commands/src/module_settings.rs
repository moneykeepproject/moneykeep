//! Module settings (business-sales.md Part A and Part H): per-module schemas,
//! `module.settings` and `module.set_setting`. Values are stored per book under the
//! module's name; unset keys read as the default. Changes are audited and undoable
//! like every setting.

use mk_core::{AccountSubtype, AccountType, Date, Decimal};
use mk_store::DocTable;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::{CommandError, CommandResult, Ctx, Registry, is_module_enabled};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "module.settings",
        "A module's settings with their current values",
        false,
        settings,
    );
    r.add(
        "module.set_setting",
        "Change one module setting (validated against its schema)",
        true,
        set_setting,
    );
}

/// The kind of value a setting holds.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Kind {
    Bool,
    Int {
        min: i64,
        max: i64,
    },
    Text {
        max: usize,
    },
    Choice(&'static [(&'static str, &'static str)]),
    /// An account of one of these subtypes (stored as its id).
    Account(&'static [AccountSubtype]),
    /// Payment terms: `{"type": "due_on_receipt"}` or `{"type": "net", "days": n}`.
    Terms,
    /// A sales tax rate or group (stored as its id).
    TaxRate,
    #[allow(dead_code)]
    Decimal,
    #[allow(dead_code)]
    Party,
    #[allow(dead_code)]
    Date,
    /// A day of the year as `MM-DD`.
    MonthDay,
    /// Next check number per bank account: `{account id: number}`.
    CheckNumbers,
    /// A list of accounts of these types (stored as ids); null: all of them.
    AccountList(&'static [AccountType]),
    /// 1099 thresholds by year: `{"2026": {"nec": "2000.00", ...}}`.
    Thresholds,
    /// A document template (stored as its id).
    Template,
    /// What an expense is paid with: a bank, cash or credit card account (stored as
    /// its id) or `personal` (expenses.md).
    PaidWith,
    /// Cents per mile with up to one decimal (0–999.9), or null (mileage.md).
    Cents,
    /// A decimal of zero or more, at most `max` (subscriptions.md).
    NonNeg {
        max: i64,
    },
    /// Colour bands `[{"percent": "20", "color": "#F2C94C"}, …]` (subscriptions.md).
    Bands,
}

/// One setting of a module's schema.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Def {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub kind: Kind,
    /// The default as JSON text (`null`: none, or an account created on first use).
    pub default: &'static str,
}

const fn d(
    key: &'static str,
    label: &'static str,
    description: &'static str,
    kind: Kind,
    default: &'static str,
) -> Def {
    Def {
        key,
        label,
        description,
        kind,
        default,
    }
}

const PAPER: &[(&str, &str)] = &[("letter", "Letter"), ("a4", "A4")];
const CONVERT_AS: &[(&str, &str)] = &[("draft", "Draft"), ("open", "Open")];
const LATE_KINDS: &[(&str, &str)] = &[
    ("none", "None"),
    ("percent", "Percentage"),
    ("flat", "Flat amount"),
];

/// Every module setting (business-sales.md Part H).
pub(crate) const SCHEMA: &[Def] = &[
    d(
        "book.fiscal_year_start",
        "Fiscal year starts",
        "The first day of the book's fiscal year (MM-DD); the balance sheet's net income starts here.",
        Kind::MonthDay,
        "\"01-01\"",
    ),
    d(
        "invoicing.number_prefix",
        "Invoice number prefix",
        "Put before the next number when an invoice takes one.",
        Kind::Text { max: 10 },
        "\"INV-\"",
    ),
    d(
        "invoicing.next_number",
        "Next invoice number",
        "The number the next invoice takes; numbers are never reused.",
        Kind::Int {
            min: 1,
            max: 999_999_999,
        },
        "1001",
    ),
    d(
        "invoicing.default_terms",
        "Default terms",
        "Payment terms for customers without their own.",
        Kind::Terms,
        "{\"type\":\"net\",\"days\":30}",
    ),
    d(
        "invoicing.deposit_to",
        "Deposit payments to",
        "Where received payments go: a bank or cash account, or Payments to Deposit to group them into one deposit later.",
        Kind::Account(&[
            AccountSubtype::Bank,
            AccountSubtype::Cash,
            AccountSubtype::Clearing,
        ]),
        "null",
    ),
    d(
        "invoicing.receivable_account",
        "Accounts receivable account",
        "The asset account invoices are owed in.",
        Kind::Account(&[AccountSubtype::Receivable]),
        "null",
    ),
    d(
        "invoicing.business_name",
        "Business name",
        "Printed at the top of invoices and estimates.",
        Kind::Text { max: 100 },
        "\"\"",
    ),
    d(
        "invoicing.business_address",
        "Business address",
        "Printed under the business name (one line per line).",
        Kind::Text { max: 500 },
        "\"\"",
    ),
    d(
        "invoicing.business_email",
        "Business email",
        "Printed on invoices and estimates.",
        Kind::Text { max: 254 },
        "\"\"",
    ),
    d(
        "invoicing.business_phone",
        "Business phone",
        "Printed on invoices and estimates.",
        Kind::Text { max: 40 },
        "\"\"",
    ),
    d(
        "invoicing.default_message",
        "Default invoice message",
        "Printed on new invoices; editable per invoice.",
        Kind::Text { max: 2000 },
        "\"\"",
    ),
    d(
        "invoicing.paper",
        "Paper size",
        "Page size of invoice and estimate PDFs.",
        Kind::Choice(PAPER),
        "\"letter\"",
    ),
    d(
        "estimates.number_prefix",
        "Estimate number prefix",
        "Put before the next number when an estimate takes one.",
        Kind::Text { max: 10 },
        "\"EST-\"",
    ),
    d(
        "estimates.next_number",
        "Next estimate number",
        "The number the next estimate takes.",
        Kind::Int {
            min: 1,
            max: 999_999_999,
        },
        "1001",
    ),
    d(
        "estimates.default_valid_days",
        "Default days valid",
        "How long a new estimate is valid.",
        Kind::Int { min: 1, max: 365 },
        "30",
    ),
    d(
        "estimates.require_acceptance",
        "Require acceptance before converting",
        "Only accepted estimates can become invoices.",
        Kind::Bool,
        "false",
    ),
    d(
        "estimates.convert_as",
        "Converted invoices start as",
        "Whether an estimate converts to a draft or an open invoice.",
        Kind::Choice(CONVERT_AS),
        "\"draft\"",
    ),
    d(
        "estimates.default_message",
        "Default estimate message",
        "Printed on new estimates.",
        Kind::Text { max: 2000 },
        "\"\"",
    ),
    d(
        "items.default_service_taxable",
        "Services taxable by default",
        "New service items start taxable.",
        Kind::Bool,
        "false",
    ),
    d(
        "items.default_product_taxable",
        "Products taxable by default",
        "New product items start taxable.",
        Kind::Bool,
        "true",
    ),
    d(
        "sales-tax.default_rate",
        "Default tax rate",
        "Used on documents for customers without their own rate.",
        Kind::TaxRate,
        "null",
    ),
    d(
        "sales-tax.liability_account",
        "Sales tax liability account",
        "The liability account sales tax is owed in.",
        Kind::Account(&[AccountSubtype::TaxPayable]),
        "null",
    ),
    d(
        "bills.default_terms",
        "Default bill terms",
        "Payment terms for vendors without their own.",
        Kind::Terms,
        "{\"type\":\"net\",\"days\":30}",
    ),
    d(
        "bills.payable_account",
        "Accounts payable account",
        "The liability account bills are owed in.",
        Kind::Account(&[AccountSubtype::Payable]),
        "null",
    ),
    d(
        "bills.default_payment_account",
        "Pay bills from",
        "The bank, cash or card account bill payments start with.",
        Kind::Account(&[
            AccountSubtype::Bank,
            AccountSubtype::Cash,
            AccountSubtype::CreditCard,
        ]),
        "null",
    ),
    d(
        "bills.next_check_number",
        "Next check numbers",
        "The next check number of each bank account; asked for on the first check.",
        Kind::CheckNumbers,
        "{}",
    ),
    d(
        "purchase-orders.number_prefix",
        "Purchase order number prefix",
        "Put before the next number when a purchase order takes one.",
        Kind::Text { max: 10 },
        "\"PO-\"",
    ),
    d(
        "purchase-orders.next_number",
        "Next purchase order number",
        "The number the next purchase order takes.",
        Kind::Int {
            min: 1,
            max: 999_999_999,
        },
        "1001",
    ),
    d(
        "purchase-orders.default_message",
        "Default purchase order message",
        "Printed on new purchase orders.",
        Kind::Text { max: 2000 },
        "\"\"",
    ),
    d(
        "1099.accounts",
        "Accounts that count for 1099",
        "Expense accounts whose payments count toward 1099 totals; leave out goods and materials.",
        Kind::AccountList(&[AccountType::Expense]),
        "null",
    ),
    d(
        "1099.thresholds",
        "1099 thresholds",
        "Reporting thresholds by year and form; check them each year (the law indexes them for inflation after 2026).",
        Kind::Thresholds,
        "{}",
    ),
    d(
        "classes.required",
        "Class required",
        "Income and expense lines cannot be saved without a class.",
        Kind::Bool,
        "false",
    ),
    d(
        "locations.required",
        "Location required",
        "Income and expense lines cannot be saved without a location.",
        Kind::Bool,
        "false",
    ),
    d(
        "funds.auto_release",
        "Release restricted spending automatically",
        "An expense tagged to a fund with donor restrictions creates the matching release from restriction.",
        Kind::Bool,
        "true",
    ),
    d(
        "funds.balance_by_fund",
        "Balance each fund separately",
        "Reserved for self-balancing fund ledgers; not available yet.",
        Kind::Bool,
        "false",
    ),
    d(
        "invoicing.default_template",
        "Default template",
        "The template invoices, estimates, statements and purchase orders print with.",
        Kind::Template,
        "null",
    ),
    d(
        "statements.late_fee_kind",
        "Late fees",
        "Charge late fees on overdue invoices: none, a monthly percentage of the balance, or a flat amount.",
        Kind::Choice(LATE_KINDS),
        "\"none\"",
    ),
    d(
        "statements.late_fee_percent",
        "Late fee percent (monthly)",
        "For percentage fees: the monthly rate, e.g. 1.5.",
        Kind::Decimal,
        "\"1.5\"",
    ),
    d(
        "statements.late_fee_flat",
        "Flat late fee",
        "For flat fees: the amount per overdue invoice.",
        Kind::Decimal,
        "\"0\"",
    ),
    d(
        "statements.late_fee_min",
        "Minimum late fee",
        "Percentage fees are at least this amount.",
        Kind::Decimal,
        "\"0\"",
    ),
    d(
        "statements.grace_days",
        "Grace days",
        "Days after the due date before a fee is charged.",
        Kind::Int { min: 0, max: 365 },
        "0",
    ),
    d(
        "statements.late_fee_account",
        "Late fee income account",
        "The income account late fees go to.",
        Kind::Account(&[AccountSubtype::Income]),
        "null",
    ),
    d(
        "bank-feeds.sync_interval_hours",
        "Sync every (hours)",
        "While the app is open, connections are synced when their last sync is older than this.",
        Kind::Int { min: 1, max: 168 },
        "12",
    ),
    d(
        "bank-feeds.date_source",
        "Transaction date",
        "Which of the provider's dates a feed transaction takes.",
        Kind::Choice(DATE_SOURCES),
        "\"transacted\"",
    ),
    d(
        "processors.history_days_first_sync",
        "History on first sync (days)",
        "How far back a new processor connection reads payouts.",
        Kind::Int { min: 1, max: 1095 },
        "90",
    ),
    d(
        "processors.default_mode",
        "Posting mode",
        "Summary posts one transaction per payout; detailed posts every sale, refund and fee.",
        Kind::Choice(PROCESSOR_MODES),
        "\"summary\"",
    ),
    d(
        "processors.sales_account",
        "Sales account",
        "Where processor sales go (created as Sales on first use).",
        Kind::Account(&[AccountSubtype::Income]),
        "null",
    ),
    d(
        "processors.fees_account",
        "Fees account",
        "Where processing fees go (created as Merchant Fees on first use).",
        Kind::Account(&[AccountSubtype::Expense]),
        "null",
    ),
    d(
        "bank-feeds.history_days_first_sync",
        "History on first sync (days)",
        "How far back a newly linked account starts when no start date is given.",
        Kind::Int { min: 1, max: 730 },
        "90",
    ),
    d(
        "bank-feeds.ofx_default_version",
        "OFX version",
        "The OFX version of an institution entered by hand when none is given.",
        Kind::Choice(OFX_VERSIONS),
        "\"103\"",
    ),
    d(
        "brokerage.history_days_first_sync",
        "Brokerage history on first sync (days)",
        "How far back a newly linked brokerage account (SnapTrade) starts.",
        Kind::Int { min: 1, max: 1825 },
        "365",
    ),
    d(
        "brokerage.sync_interval_hours",
        "Brokerage sync interval (hours)",
        "Scheduled brokerage syncs run at most this often (SnapTrade refreshes once a day).",
        Kind::Int { min: 6, max: 168 },
        "24",
    ),
    d(
        "expenses.reimbursement_account",
        "Reimbursement account",
        "The liability account expenses paid with personal funds are owed from (created as Reimbursements Payable on first use).",
        Kind::Account(&[AccountSubtype::Payable, AccountSubtype::OtherLiability]),
        "null",
    ),
    d(
        "expenses.match_days",
        "Match within (days)",
        "Posting an expense looks for a transaction of the same amount this many days either side of its date.",
        Kind::Int { min: 0, max: 30 },
        "5",
    ),
    d(
        "expenses.default_paid_with",
        "Default paid with",
        "What new expenses are paid with: an account, or personal funds.",
        Kind::PaidWith,
        "null",
    ),
    d(
        "mileage.default_purpose",
        "Default purpose",
        "The purpose kind new trips start with.",
        Kind::Choice(crate::mileage::KIND_CHOICES),
        "\"business\"",
    ),
    d(
        "mileage.show_moving",
        "Show moving",
        "Offer the moving purpose kind (it applies only to certain military and intelligence community moves).",
        Kind::Bool,
        "false",
    ),
    d(
        "mileage.reimbursement_rate",
        "Reimbursement rate (cents per mile)",
        "The rate reimbursable trips are paid back at; empty uses the standard rate of the trip's purpose kind.",
        Kind::Cents,
        "null",
    ),
    d(
        "mileage.expense_account",
        "Mileage expense account",
        "The expense account reimbursable trips are posted to (created as Auto: Mileage reimbursement on first use).",
        Kind::Account(&[AccountSubtype::Expense]),
        "null",
    ),
    d(
        "mileage.distance_unit",
        "Distance unit",
        "Trips are measured in miles.",
        Kind::Choice(&[("mi", "Miles")]),
        "\"mi\"",
    ),
    d(
        "subscriptions.alert_percent",
        "Alert on price changes of at least (%)",
        "A price change alerts when it is at or over this percentage or the amount below; smaller changes are recorded as small.",
        Kind::NonNeg { max: 1000 },
        "\"1\"",
    ),
    d(
        "subscriptions.alert_amount",
        "Or at least (amount)",
        "A price change alerts when it is at or over this amount or the percentage above.",
        Kind::NonNeg { max: 1_000_000 },
        "\"0.50\"",
    ),
    d(
        "subscriptions.bands",
        "Colour bands",
        "Price increases take the colour of the highest band they reach; smaller increases use the normal text colour and decreases are green.",
        Kind::Bands,
        "[{\"percent\":\"20\",\"color\":\"#F2C94C\"},{\"percent\":\"40\",\"color\":\"#D64545\"}]",
    ),
    d(
        "subscriptions.renewal_days",
        "Renewal alert (days before)",
        "Yearly and six-monthly subscriptions are listed under Attention this many days before they renew.",
        Kind::Int { min: 0, max: 90 },
        "14",
    ),
    d(
        "subscriptions.trial_days",
        "Trial ending alert (days before)",
        "Free trials are listed under Attention this many days before they end.",
        Kind::Int { min: 0, max: 30 },
        "3",
    ),
    d(
        "subscriptions.make_bill_as",
        "Make a bill creates",
        "What Make a bill on a subscription does.",
        Kind::Choice(&[
            (
                "scheduled_bill",
                "A repeating bill in Scheduled (posts when you enter it)",
            ),
            ("bill_now", "A bill now (posts to Accounts Payable)"),
        ]),
        "\"scheduled_bill\"",
    ),
    d(
        "projects.required",
        "Project required",
        "Income and expense lines cannot be saved without a project.",
        Kind::Bool,
        "false",
    ),
    d(
        "projects.warn_percent",
        "Budget warning at (%)",
        "Home lists a project under Attention when it has used this share of its budget (and again when it goes over).",
        Kind::Int { min: 1, max: 1000 },
        "90",
    ),
    d(
        "projects.billable_income_account",
        "Billable costs post to",
        "The income account invoice lines for billable project costs post to (created as Billable Expense Income on first use).",
        Kind::Account(&[AccountSubtype::Income]),
        "null",
    ),
    d(
        "capture.auto_collect",
        "Collect automatically",
        "Bring in expenses from your phone when the book opens and then regularly while it is open.",
        Kind::Bool,
        "true",
    ),
    d(
        "capture.collect_minutes",
        "Collect every (minutes)",
        "How often expenses from your phone are collected while the book is open.",
        Kind::Int { min: 5, max: 240 },
        "15",
    ),
];

const OFX_VERSIONS: &[(&str, &str)] = &[
    ("102", "1.0.2"),
    ("103", "1.0.3"),
    ("151", "1.5.1"),
    ("160", "1.6"),
    ("200", "2.0"),
    ("203", "2.0.3"),
    ("211", "2.1.1"),
    ("220", "2.2"),
];

const PROCESSOR_MODES: &[(&str, &str)] = &[("summary", "Summary"), ("detailed", "Detailed")];

const DATE_SOURCES: &[(&str, &str)] = &[
    ("posted", "Posted date"),
    ("transacted", "Transaction date when given"),
];

fn module_of(key: &str) -> &str {
    match key.split_once('.').map_or(key, |(m, _)| m) {
        // connectivity-processors.md names these settings `processors.*`.
        "processors" => "payment-processing",
        // connectivity-brokerage.md: SnapTrade belongs to investments.
        "brokerage" => "investments",
        // mobile-capture.md: phone capture belongs to expenses.
        "capture" => "expenses",
        m => m,
    }
}

pub(crate) fn def(key: &str) -> Option<&'static Def> {
    SCHEMA.iter().find(|d| d.key == key)
}

/// Whether a module declares settings.
pub(crate) fn has_settings(module: &str) -> bool {
    SCHEMA.iter().any(|d| module_of(d.key) == module)
}

fn default_of(d: &Def) -> Value {
    serde_json::from_str(d.default).unwrap_or(Value::Null)
}

/// A setting's value: the stored one, else the default.
pub(crate) fn get(ctx: &Ctx<'_>, key: &str) -> CommandResult<Value> {
    let d = def(key).ok_or_else(|| CommandError::Integrity(format!("unknown setting {key}")))?;
    Ok(ctx
        .tx
        .get_setting(module_of(key), key)?
        .unwrap_or_else(|| default_of(d)))
}

pub(crate) fn get_str(ctx: &Ctx<'_>, key: &str) -> CommandResult<String> {
    Ok(get(ctx, key)?.as_str().unwrap_or_default().to_owned())
}

pub(crate) fn get_i64(ctx: &Ctx<'_>, key: &str) -> CommandResult<i64> {
    Ok(get(ctx, key)?.as_i64().unwrap_or_default())
}

pub(crate) fn get_bool(ctx: &Ctx<'_>, key: &str) -> CommandResult<bool> {
    Ok(get(ctx, key)?.as_bool().unwrap_or_default())
}

/// Store a setting directly (number sequences); audited like any setting.
pub(crate) fn put(ctx: &Ctx<'_>, key: &str, value: Value) -> CommandResult<()> {
    ctx.save_setting(module_of(key), key, Some(&value))
}

fn kind_json(k: &Kind) -> Value {
    match k {
        Kind::Bool => json!({"type": "bool"}),
        Kind::Int { min, max } => json!({"type": "int", "min": min, "max": max}),
        Kind::Text { max } => json!({"type": "text", "max": max}),
        Kind::Choice(c) => {
            json!({"type": "choice", "choices": c.iter().map(|(v, l)| json!({"value": v, "label": l})).collect::<Vec<_>>()})
        }
        Kind::Account(s) => {
            json!({"type": "account", "subtypes": s.iter().map(|x| x.to_string()).collect::<Vec<_>>()})
        }
        Kind::Terms => json!({"type": "terms"}),
        Kind::TaxRate => json!({"type": "tax_rate"}),
        Kind::Decimal => json!({"type": "decimal"}),
        Kind::Party => json!({"type": "party"}),
        Kind::Date => json!({"type": "date"}),
        Kind::MonthDay => json!({"type": "month_day"}),
        Kind::CheckNumbers => json!({"type": "check_numbers"}),
        Kind::AccountList(t) => json!({"type": "account_list", "account_types": t}),
        Kind::Thresholds => json!({"type": "thresholds"}),
        Kind::Template => json!({"type": "template"}),
        Kind::PaidWith => json!({"type": "paid_with"}),
        Kind::Cents => json!({"type": "cents"}),
        Kind::NonNeg { .. } => json!({"type": "decimal"}),
        Kind::Bands => json!({"type": "bands"}),
    }
}

/// Validate payment terms.
pub(crate) fn check_terms(v: &Value) -> CommandResult<Value> {
    let bad = || {
        CommandError::InvalidArgs(
            "terms are {\"type\": \"due_on_receipt\"} or {\"type\": \"net\", \"days\": 1-365}"
                .into(),
        )
    };
    match v.get("type").and_then(Value::as_str) {
        Some("due_on_receipt") => Ok(json!({"type": "due_on_receipt"})),
        Some("net") => {
            let days = v.get("days").and_then(Value::as_i64).ok_or_else(bad)?;
            if !(1..=365).contains(&days) {
                return Err(bad());
            }
            Ok(json!({"type": "net", "days": days}))
        }
        _ => Err(bad()),
    }
}

/// "Due on receipt" or "Net 30".
pub(crate) fn terms_label(v: &Value) -> String {
    match v.get("type").and_then(Value::as_str) {
        Some("net") => format!(
            "Net {}",
            v.get("days").and_then(Value::as_i64).unwrap_or(30)
        ),
        _ => "Due on receipt".into(),
    }
}

fn validate(ctx: &Ctx<'_>, d: &Def, v: &Value) -> CommandResult<Value> {
    let invalid = |m: String| CommandError::InvalidArgs(format!("{}: {m}", d.label));
    Ok(match d.kind {
        Kind::Bool => Value::Bool(
            v.as_bool()
                .ok_or_else(|| invalid("must be true or false".into()))?,
        ),
        Kind::Int { min, max } => {
            let n = v
                .as_i64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                .ok_or_else(|| invalid("must be a whole number".into()))?;
            if n < min || n > max {
                return Err(invalid(format!("must be from {min} to {max}")));
            }
            json!(n)
        }
        Kind::Text { max } => {
            let s = v.as_str().ok_or_else(|| invalid("must be text".into()))?;
            let s = mk_core::text::clean(s, usize::MAX);
            if s.text.chars().count() > max {
                return Err(invalid(format!("at most {max} characters")));
            }
            // Multi-line text keeps its line breaks.
            let raw = v.as_str().unwrap_or_default();
            if raw.contains('\n') {
                json!(
                    raw.lines()
                        .map(|l| mk_core::text::clean(l, max).text)
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            } else {
                json!(s.text)
            }
        }
        Kind::Choice(c) => {
            let s = v
                .as_str()
                .ok_or_else(|| invalid("must be one of the choices".into()))?;
            let hit = c
                .iter()
                .find(|(val, label)| val.eq_ignore_ascii_case(s) || label.eq_ignore_ascii_case(s))
                .ok_or_else(|| {
                    invalid(format!(
                        "one of {}",
                        c.iter().map(|x| x.1).collect::<Vec<_>>().join(", ")
                    ))
                })?;
            json!(hit.0)
        }
        Kind::Account(subtypes) => {
            let r = v
                .as_str()
                .ok_or_else(|| invalid("must name an account".into()))?;
            let chart = Chart::load(ctx)?;
            let a = chart.resolve(r)?;
            if !subtypes.contains(&a.subtype) {
                return Err(invalid(format!(
                    "{} is not a {} account",
                    chart.full_name(a.id),
                    subtypes
                        .iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<_>>()
                        .join(" or ")
                )));
            }
            json!(a.id)
        }
        Kind::Terms => check_terms(v)?,
        Kind::TaxRate => {
            let r = v
                .as_str()
                .ok_or_else(|| invalid("must name a tax rate or group".into()))?;
            let found = ctx
                .tx
                .docs(DocTable::TaxRate, None)?
                .into_iter()
                .find(|x| {
                    x.id == r
                        || x.json
                            .get("name")
                            .and_then(Value::as_str)
                            .is_some_and(|n| n.eq_ignore_ascii_case(r.trim()))
                })
                .ok_or_else(|| invalid(format!("no tax rate or group named {r:?}")))?;
            json!(found.id)
        }
        Kind::Decimal => {
            let s = v
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string());
            let n: Decimal = s
                .trim()
                .parse()
                .map_err(|_| invalid("must be a number".into()))?;
            json!(n.normalize().to_string())
        }
        Kind::Party => {
            let r = v
                .as_str()
                .ok_or_else(|| invalid("must name a party".into()))?;
            json!(crate::sales::find_party(ctx, r)?.id)
        }
        Kind::Date => {
            let s = v.as_str().ok_or_else(|| invalid("must be a date".into()))?;
            let dt: Date = s
                .parse()
                .map_err(|_| invalid("must be a date (YYYY-MM-DD)".into()))?;
            json!(dt)
        }
        Kind::CheckNumbers => {
            let o = v
                .as_object()
                .ok_or_else(|| invalid("an object of account: next number".into()))?;
            let chart = Chart::load(ctx)?;
            let mut out = serde_json::Map::new();
            for (k, n) in o {
                let a = chart.resolve(k)?;
                if !matches!(a.subtype, AccountSubtype::Bank | AccountSubtype::Cash) {
                    return Err(invalid(format!(
                        "{} is not a bank account",
                        chart.full_name(a.id)
                    )));
                }
                let n = n
                    .as_i64()
                    .or_else(|| n.as_str().and_then(|s| s.trim().parse().ok()))
                    .filter(|n| (1..=999_999_999).contains(n))
                    .ok_or_else(|| invalid("check numbers are whole numbers from 1".into()))?;
                out.insert(a.id.to_string(), json!(n));
            }
            Value::Object(out)
        }
        Kind::AccountList(types) => {
            let list = v
                .as_array()
                .ok_or_else(|| invalid("a list of accounts".into()))?;
            let chart = Chart::load(ctx)?;
            let mut ids = Vec::new();
            for r in list {
                let a = chart.resolve(r.as_str().unwrap_or_default())?;
                if !types.contains(&a.account_type) {
                    return Err(invalid(format!(
                        "{} is not an expense account",
                        chart.full_name(a.id)
                    )));
                }
                let id = json!(a.id);
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            Value::Array(ids)
        }
        Kind::Template => {
            let r = v
                .as_str()
                .ok_or_else(|| invalid("must name a template".into()))?;
            json!(crate::templates::find(ctx, r)?.id)
        }
        Kind::PaidWith => {
            let r = v
                .as_str()
                .ok_or_else(|| invalid("an account or personal".into()))?;
            json!(crate::expenses::paid_with_ref(ctx, r)?)
        }
        Kind::Cents => {
            let s = v
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string());
            if s.trim().is_empty() {
                return Ok(Value::Null);
            }
            json!(
                crate::mileage::check_cents(&s)
                    .map_err(|e| invalid(e.to_string()))?
                    .to_string()
            )
        }
        Kind::NonNeg { max } => {
            let s = v
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string());
            let n: Decimal = s
                .trim()
                .trim_end_matches('%')
                .trim()
                .parse()
                .map_err(|_| invalid("must be a number".into()))?;
            if n.is_sign_negative() || n > Decimal::from(max) {
                return Err(invalid(format!("must be from 0 to {max}")));
            }
            json!(n.normalize().to_string())
        }
        Kind::Bands => crate::subscriptions::check_bands(v).map_err(invalid)?,
        Kind::Thresholds => {
            let o = v
                .as_object()
                .ok_or_else(|| invalid("an object of year: thresholds".into()))?;
            let mut out = serde_json::Map::new();
            for (year, row) in o {
                let y: i32 = year
                    .parse()
                    .ok()
                    .filter(|y| (2000..=2100).contains(y))
                    .ok_or_else(|| invalid(format!("{year} is not a year")))?;
                let r = row
                    .as_object()
                    .ok_or_else(|| invalid("each year maps forms to amounts".into()))?;
                let mut clean = serde_json::Map::new();
                for (form, amount) in r {
                    if !THRESHOLD_KEYS.contains(&form.as_str()) {
                        return Err(invalid(format!(
                            "unknown form {form}; use {}",
                            THRESHOLD_KEYS.join(", ")
                        )));
                    }
                    let text = amount
                        .as_str()
                        .map_or_else(|| amount.to_string(), str::to_owned);
                    let n: Decimal = text
                        .trim()
                        .parse()
                        .ok()
                        .filter(|n: &Decimal| !n.is_sign_negative())
                        .ok_or_else(|| invalid(format!("{form}: an amount of zero or more")))?;
                    let mut n = n.round_dp(2);
                    n.rescale(2);
                    clean.insert(form.clone(), json!(n.to_string()));
                }
                out.insert(y.to_string(), Value::Object(clean));
            }
            Value::Object(out)
        }
        Kind::MonthDay => {
            let s = v.as_str().ok_or_else(|| invalid("must be MM-DD".into()))?;
            let (m, day) =
                month_day(s).ok_or_else(|| invalid("must be a day of the year as MM-DD".into()))?;
            json!(format!("{m:02}-{day:02}"))
        }
    })
}

/// The forms and MISC boxes that have a 1099 threshold.
pub(crate) const THRESHOLD_KEYS: [&str; 6] = [
    "nec",
    "rents",
    "royalties",
    "other_income",
    "medical",
    "attorney_gross_proceeds",
];

/// The built-in 1099 threshold: 600.00 up to 2025 and 2,000.00 from 2026 for NEC and
/// MISC boxes 1, 3, 6 and 10; royalties 10.00 every year.
pub(crate) fn default_threshold(year: i32, form: &str) -> Decimal {
    match form {
        "royalties" => Decimal::TEN,
        _ if year >= 2026 => Decimal::from(2000),
        _ => Decimal::from(600),
    }
}

/// The 1099 threshold of a year and form/box: the user's row for that year, else the
/// user's latest earlier row when it is not older than the law change that applies to
/// the year, else the built-in default.
pub(crate) fn threshold(ctx: &Ctx<'_>, year: i32, form: &str) -> CommandResult<Decimal> {
    let table = get(ctx, "1099.thresholds")?;
    let mut best: Option<(i32, Decimal)> = None;
    if let Some(o) = table.as_object() {
        for (y, row) in o {
            let Ok(y) = y.parse::<i32>() else { continue };
            if y > year || best.is_some_and(|b| b.0 >= y) {
                continue;
            }
            if let Some(n) = row
                .get(form)
                .and_then(Value::as_str)
                .and_then(|s| s.parse().ok())
            {
                best = Some((y, n));
            }
        }
    }
    let law_year = if year >= 2026 { 2026 } else { i32::MIN };
    Ok(match best {
        Some((y, n)) if y >= law_year => n,
        _ => default_threshold(year, form),
    })
}

/// The display form of a stored value (account full names, tax rate names).
fn shown(ctx: &Ctx<'_>, d: &Def, v: &Value) -> CommandResult<Value> {
    Ok(match (d.kind, v.as_str()) {
        (Kind::Account(_), Some(id)) => {
            let chart = Chart::load(ctx)?;
            chart
                .resolve(id)
                .map(|a| json!(chart.full_name(a.id)))
                .unwrap_or(Value::Null)
        }
        (Kind::TaxRate, Some(id)) => ctx
            .tx
            .get_doc(DocTable::TaxRate, id)?
            .and_then(|x| x.json.get("name").cloned())
            .unwrap_or(Value::Null),
        (Kind::Terms, _) => json!(terms_label(v)),
        (Kind::Template, Some(id)) => json!(crate::templates::find(ctx, id).ok().map(|t| t.name)),
        (Kind::PaidWith, Some(id)) => json!(crate::expenses::paid_with_name(ctx, id)?),
        _ => v.clone(),
    })
}

/// The label of "no account chosen" now: for the projects' billable income account,
/// the account's name once it exists (release-prep.md §4), else that it is made on
/// first use.
fn default_label_now(ctx: &Ctx<'_>, d: &Def) -> CommandResult<Option<String>> {
    if d.key == "projects.billable_income_account" {
        let name = crate::projects::BILLABLE_INCOME;
        let chart = Chart::load(ctx)?;
        let exists = chart.accounts.iter().any(|a| {
            a.parent.is_none()
                && a.name.eq_ignore_ascii_case(name)
                && a.subtype == AccountSubtype::Income
        });
        return Ok(Some(if exists {
            name.to_owned()
        } else {
            format!("{name} (made on first use)")
        }));
    }
    Ok(default_label(d).map(str::to_owned))
}

fn default_label(d: &Def) -> Option<&'static str> {
    match d.key {
        "invoicing.deposit_to" => Some("Payments to Deposit"),
        "invoicing.receivable_account" => Some("Accounts Receivable"),
        "sales-tax.liability_account" => Some("Sales Tax Payable"),
        "bills.payable_account" => Some("Accounts Payable"),
        "bills.default_payment_account" => Some("None"),
        "1099.accounts" => Some("All expense accounts"),
        "invoicing.default_template" => Some("None"),
        "statements.late_fee_account" => Some("Late Fee Income"),
        "sales-tax.default_rate" => Some("None"),
        "expenses.reimbursement_account" => Some("Reimbursements Payable"),
        "expenses.default_paid_with" => Some("None"),
        "mileage.expense_account" => Some("Auto: Mileage reimbursement"),
        _ => None,
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SettingsArgs {
    module: String,
}

/// `MM-DD` → (month, day); 02-29 is rejected (not every year has it).
pub(crate) fn month_day(s: &str) -> Option<(u8, u8)> {
    let (m, d) = s.trim().split_once('-')?;
    let m: u8 = m.parse().ok()?;
    let d: u8 = d.parse().ok()?;
    ((1..=12).contains(&m) && d >= 1 && d <= Date::days_in_month(2025, m)).then_some((m, d))
}

/// Core (`book.*`) settings are always available; module settings need the module on.
fn module_on(ctx: &Ctx<'_>, module: &str) -> CommandResult<bool> {
    if module == "book" {
        return Ok(true);
    }
    is_module_enabled(ctx.tx, module)
}

fn settings(ctx: &Ctx<'_>, a: SettingsArgs) -> CommandResult<Value> {
    if a.module != "book" {
        crate::builtin::known_module(&a.module)?;
    }
    if !module_on(ctx, &a.module)? {
        return Err(CommandError::Rejected(format!(
            "module {} is off; turn it on to see its settings",
            a.module
        )));
    }
    let mut out = Vec::new();
    for d in SCHEMA.iter().filter(|d| module_of(d.key) == a.module) {
        let value = get(ctx, d.key)?;
        let mut v = json!({
            "key": d.key,
            "label": d.label,
            "description": d.description,
            "default": default_of(d),
            "value": value,
            "shown": shown(ctx, d, &value)?,
            "default_label": default_label_now(ctx, d)?,
        });
        if let (Some(o), Some(k)) = (v.as_object_mut(), kind_json(&d.kind).as_object()) {
            for (kk, vv) in k {
                o.insert(kk.clone(), vv.clone());
            }
        }
        out.push(v);
    }
    Ok(json!({"module": a.module, "settings": out}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetArgs {
    key: String,
    /// The new value; null resets to the default.
    value: Value,
}

fn set_setting(ctx: &Ctx<'_>, a: SetArgs) -> CommandResult<Value> {
    if crate::ai_access::is_ai_key(&a.key) {
        return Err(CommandError::InvalidArgs(
            crate::ai_access::REFUSED_KEY.into(),
        ));
    }
    let d = def(&a.key)
        .ok_or_else(|| CommandError::InvalidArgs(format!("unknown module setting {:?}", a.key)))?;
    let module = module_of(d.key);
    if !module_on(ctx, module)? {
        return Err(CommandError::Rejected(format!(
            "module {module} is off; turn it on to change its settings"
        )));
    }
    let stored = if a.value.is_null() {
        None
    } else {
        // A value that validates to null (an empty rate) resets to the default.
        Some(validate(ctx, d, &a.value)?).filter(|v| !v.is_null())
    };
    if d.key == "funds.balance_by_fund" && stored == Some(Value::Bool(true)) {
        return Err(CommandError::Rejected(
            "self-balancing fund ledgers are not available yet".into(),
        ));
    }
    ctx.save_setting(module, d.key, stored.as_ref())?;
    let value = get(ctx, d.key)?;
    Ok(json!({"key": d.key, "value": value, "shown": shown(ctx, d, &value)?}))
}

/// The account a setting names, or the default account created on first use.
pub(crate) fn account_or_create(
    ctx: &Ctx<'_>,
    key: &str,
    name: &str,
    ty: AccountType,
    subtype: AccountSubtype,
) -> CommandResult<mk_core::AccountId> {
    account_or_create_role(ctx, key, None, name, ty, subtype)
}

/// [`account_or_create`] with the chart-presets.md lookup rule: when no setting
/// names an account, the account holding `role` is used; failing that, the account
/// found by name or newly created gets the role.
pub(crate) fn account_or_create_role(
    ctx: &Ctx<'_>,
    key: &str,
    role: Option<&str>,
    name: &str,
    ty: AccountType,
    subtype: AccountSubtype,
) -> CommandResult<mk_core::AccountId> {
    let chart = Chart::load(ctx)?;
    if let Some(id) = get(ctx, key)?.as_str()
        && let Ok(a) = chart.resolve(id)
    {
        return Ok(a.id);
    }
    // An existing top-level account of that name and subtype, else a new one.
    let find = |a: &mk_core::Account| {
        a.parent.is_none() && a.name.eq_ignore_ascii_case(name) && a.subtype == subtype
    };
    let create = |ctx: &Ctx<'_>| -> CommandResult<mk_core::AccountId> {
        let v = crate::accounts::create_json(
            ctx,
            json!({"name": name, "type": ty, "subtype": subtype}),
        )?;
        v.get("id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| CommandError::Integrity("account without id".into()))
    };
    match role {
        Some(r) => crate::chart::roles::find_or_create(ctx, r, &find, &create),
        None => match chart.accounts.iter().find(|a| find(a)) {
            Some(a) => Ok(a.id),
            None => create(ctx),
        },
    }
}
