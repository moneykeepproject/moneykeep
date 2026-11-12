//! Opening balances (spec: opening-balances.md).
//!
//! The working-backwards rule turns one balance the user trusts into an opening
//! entry: one ordinary transaction dated D between the account and its other side,
//! marked `opening: true`. `opening.plan` works it out without changing anything (the
//! wizard shows it step by step); `opening.apply` posts or changes the entry as one
//! undoable action. The investment wizard does the same for a brokerage account's
//! cash and adds one opening shares in per holding (`opening.inv_plan`,
//! `opening.inv_apply`). `opening.checklist` lists every balance-sheet account with
//! its status.

use std::collections::{HashMap, HashSet};

use mk_core::currency::{format_amount, minor_units};
use mk_core::{
    Account, AccountId, AccountSubtype, AccountType, CommodityCode, Date, Decimal, Reconcile,
    Split, Transaction, TransactionId, TxnStatus,
};
use mk_store::{Doc, DocTable, TxnFilter, now_rfc3339};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::investments::{
    Action, ActionArgs, INVESTMENTS, InvMeta, LotArg, SecurityKind, all_metas, find_security,
    latest_price, meta_of, post_with, sec_account_of, securities, value_of,
};
use crate::txns::validate_change;
use crate::{CommandError, CommandResult, Ctx, Registry, is_module_enabled};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "opening.plan",
        "Work out an account's opening balance from a balance you trust, without posting \
         (start date, other side, check against the bank, removed transactions)",
        false,
        plan_cmd,
    );
    r.add(
        "opening.apply",
        "Post or change an account's opening balance worked out from a balance you trust \
         (one undoable action; a statement start removes the transactions before it)",
        true,
        apply_cmd,
    );
    r.add(
        "opening.checklist",
        "Every balance-sheet account with its opening balance status and the setup progress",
        false,
        checklist_cmd,
    );
    r.add(
        "opening.started_from_zero",
        "Mark an account as started from zero (it needs no opening balance), or undo that",
        true,
        started_from_zero,
    );
    r.add_in(
        INVESTMENTS,
        "opening.inv_plan",
        "Work out a brokerage account's opening cash, holdings on the start date and cost \
         basis, without posting",
        false,
        inv_plan_cmd,
    );
    r.add_in(
        INVESTMENTS,
        "opening.inv_apply",
        "Post or change a brokerage account's opening cash and holdings (one undoable action)",
        true,
        inv_apply_cmd,
    );
    r.add_in(
        INVESTMENTS,
        "opening.inv_paste",
        "Read pasted positions (lines of symbol, shares and optional cost) into holdings rows",
        false,
        inv_paste,
    );
    r.add_in(
        INVESTMENTS,
        "opening.set_basis",
        "Supply the cost basis of an opening holding recorded as Basis needed",
        true,
        set_basis,
    );
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

fn rejected(m: impl Into<String>) -> CommandError {
    CommandError::Rejected(m.into())
}

fn plus(a: Decimal, b: Decimal) -> CommandResult<Decimal> {
    a.checked_add(b)
        .ok_or_else(|| rejected("arithmetic overflow"))
}

fn minus(a: Decimal, b: Decimal) -> CommandResult<Decimal> {
    a.checked_sub(b)
        .ok_or_else(|| rejected("arithmetic overflow"))
}

/// The payee of every opening entry.
pub(crate) const PAYEE: &str = "Opening balance";

/// Owner's Equity, offered in business books (created on first use).
pub(crate) const OWNERS_EQUITY: &str = "Owner's Equity";

pub(crate) const WHY_OPENING: &str = "This is what you had before you started using MoneyKeep. \
     It is kept separate so your income and spending are not affected.";
pub(crate) const WHY_EQUITY: &str =
    "Choose Owner's Equity if this is money you put into the business.";
pub(crate) const WHY_RECEIVABLE: &str =
    "Choose it if this money is owed by a customer you invoice in MoneyKeep.";
pub(crate) const BASIS_WHERE: &str =
    "Your broker's cost basis or unrealized gain page, or last year's 1099-B.";

/// A balance-sheet account the wizards and the checklist cover.
fn on_balance_sheet(a: &Account) -> bool {
    matches!(a.account_type, AccountType::Asset | AccountType::Liability)
        && !a.placeholder
        && !a.archived
        && a.subtype != AccountSubtype::Security
}

/// Business books (the business feature on): invoicing or bills.
fn business(ctx: &Ctx<'_>) -> CommandResult<bool> {
    Ok(is_module_enabled(ctx.tx, "invoicing")? || is_module_enabled(ctx.tx, "bills")?)
}

/// Raw (debit-positive) quantity of `t`'s splits in `account`.
fn amount_in(t: &Transaction, account: AccountId) -> CommandResult<Decimal> {
    let mut sum = Decimal::ZERO;
    for s in t.splits.iter().filter(|s| s.account == account) {
        sum = plus(sum, s.quantity)?;
    }
    Ok(sum)
}

fn reconciled_in(t: &Transaction, account: AccountId) -> bool {
    t.splits
        .iter()
        .any(|s| s.account == account && s.reconcile == Reconcile::Reconciled)
}

/// The book's Opening Balances account, if there is one.
fn opening_account(ctx: &Ctx<'_>, chart: &Chart) -> CommandResult<Option<AccountId>> {
    if let Some(id) = ctx.tx.opening_account_id()?
        && chart.get(id).is_some()
    {
        return Ok(Some(id));
    }
    Ok(chart
        .accounts
        .iter()
        .find(|a| a.subtype == AccountSubtype::OpeningBalances && !a.placeholder)
        .map(|a| a.id))
}

/// Posted and voided transactions with a split in `account`, in register order.
fn txns_of(ctx: &Ctx<'_>, account: AccountId) -> CommandResult<Vec<Transaction>> {
    Ok(ctx.tx.transactions(&TxnFilter {
        accounts: Some(vec![account]),
        ..TxnFilter::default()
    })?)
}

/// The account's opening entry: the transaction marked `opening` with a split in it,
/// or (accounts set up before opening-balances.md) its earliest transaction when that
/// is against Opening Balances. The flag says whether it was found by the old rule.
pub(crate) fn opening_entry(
    ctx: &Ctx<'_>,
    chart: &Chart,
    account: AccountId,
    txns: &[Transaction],
) -> CommandResult<Option<(Transaction, bool)>> {
    if let Some(t) = txns.iter().find(|t| t.opening) {
        return Ok(Some((t.clone(), false)));
    }
    let Some(ob) = opening_account(ctx, chart)? else {
        return Ok(None);
    };
    let Some(first) = txns.iter().find(|t| t.status == TxnStatus::Posted) else {
        return Ok(None);
    };
    let legacy = first.splits.iter().any(|s| s.account == ob)
        && first.splits.iter().any(|s| s.account == account)
        && first.splits.len() == 2;
    Ok(legacy.then(|| (first.clone(), true)))
}

/// The other side of an opening entry: the split not in `account`.
fn other_side(t: &Transaction, account: AccountId) -> Option<AccountId> {
    t.splits
        .iter()
        .find(|s| s.account != account)
        .map(|s| s.account)
}

// ----- the feed -------------------------------------------------------------------------

/// The bank feed's last balance for an account.
#[derive(Debug, Clone)]
struct Feed {
    /// Raw (as the ledger stores it: an amount owed is negative).
    balance: Option<Decimal>,
    date: Option<Date>,
    include_pending: bool,
    connection: String,
    link: String,
    /// The provider's holdings check (SnapTrade).
    holdings: Option<Value>,
}

fn feed_of(ctx: &Ctx<'_>, account: AccountId) -> CommandResult<Option<Feed>> {
    let me = account.to_string();
    let offset = ctx.connector.map_or(0, |c| c.utc_offset_secs);
    for d in ctx.tx.docs(DocTable::AccountLink, None)? {
        let Ok(l) = serde_json::from_value::<crate::connect::LinkDoc>(d.json) else {
            continue;
        };
        if l.account.as_deref() != Some(me.as_str()) || !l.active {
            continue;
        }
        let balance = l.balance.as_deref().and_then(|b| b.trim().parse().ok());
        let date = l
            .balance_date
            .filter(|t| *t > 0)
            .and_then(|t| mk_import::simplefin::epoch_date(t, offset).ok());
        return Ok(Some(Feed {
            balance,
            date,
            include_pending: l.include_pending,
            connection: l.connection.clone(),
            link: l.id.clone(),
            holdings: l.remote.and_then(|r| r.holdings),
        }));
    }
    Ok(None)
}

/// Fingerprints of committed feed rows that were pending when imported.
fn pending_fingerprints(ctx: &Ctx<'_>) -> CommandResult<HashSet<String>> {
    let mut out = HashSet::new();
    for d in ctx.tx.docs(DocTable::ImportRow, None)? {
        let pending = d
            .json
            .get("raw")
            .and_then(|r| r.get("pending"))
            .and_then(Value::as_bool)
            == Some(true);
        if let (true, Some(fp)) = (pending, d.json.get("fingerprint").and_then(Value::as_str)) {
            out.insert(fp.to_owned());
        }
    }
    Ok(out)
}

/// Whether a feed row with `fingerprint` was removed from `account` by a statement
/// start (the import then treats it as a duplicate).
pub(crate) fn forgotten(
    ctx: &Ctx<'_>,
    account: AccountId,
    fingerprint: &str,
) -> CommandResult<bool> {
    Ok(ctx
        .tx
        .get_doc(
            DocTable::ImportForgotten,
            &forgotten_id(account, fingerprint),
        )?
        .is_some())
}

fn forgotten_id(account: AccountId, fingerprint: &str) -> String {
    format!("{account}|{fingerprint}")
}

// ----- arguments ------------------------------------------------------------------------

/// Where the books start.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Start {
    /// Keep everything imported: the day before the earliest transaction.
    #[default]
    Keep,
    /// Start at a statement: its ending date and balance.
    Statement,
}

/// "Is this loan for something you own?": the asset and its value.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssetArg {
    /// Name of the new asset account (e.g. House).
    pub name: String,
    /// What it is worth on the start date.
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    pub value: Decimal,
}

/// The cash part shared by both wizards.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CashArgs {
    /// The balance you trust, as the bank shows it. For credit cards, loans and other
    /// debts the amount owed, as a positive number. Absent: the bank feed's balance.
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub balance: Option<Decimal>,
    /// The date of that balance (default: the feed's balance date, or today).
    #[serde(default)]
    pub as_of: Option<Date>,
    /// A card or other debt in credit: the balances given are what it owes you.
    #[serde(default)]
    pub owes_me: bool,
    #[serde(default)]
    pub start: Start,
    /// Start at a statement: its ending date.
    #[serde(default)]
    pub statement_date: Option<Date>,
    /// Start at a statement: its ending balance (owed as a positive number for debts).
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub statement_balance: Option<Decimal>,
    /// Transactions on or before a statement start to keep (all others are removed).
    #[serde(default)]
    pub keep: Vec<TransactionId>,
    /// Quick entry (New account): the opening amount itself, dated `date`; nothing is
    /// worked back.
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub amount: Option<Decimal>,
    #[serde(default)]
    pub date: Option<Date>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MoneyArgs {
    /// Account id, full name or code.
    pub account: String,
    #[serde(flatten)]
    pub cash: CashArgs,
    /// The other side: an account (id, full name or code), `opening-balances` or
    /// `owners-equity` (created on first use). Default: Opening Balances.
    #[serde(default)]
    pub offset: Option<String>,
    /// Loans: the thing the loan is for, created as an asset account with its own
    /// opening entry.
    #[serde(default)]
    pub asset: Option<AssetArg>,
    /// Go ahead although a finished reconciliation would no longer add up (it is
    /// marked to check).
    #[serde(default)]
    pub go_ahead: bool,
    /// The date the default as-of date is worked out from (default: today).
    #[serde(default)]
    pub today: Option<Date>,
}

// ----- the plan ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PreStart {
    txn: Transaction,
    amount: Decimal,
    removed: bool,
}

#[derive(Debug, Clone)]
struct Conflict {
    reconciliation: Option<String>,
    date: Date,
    ending: Decimal,
}

/// Everything the wizard shows, worked out.
#[derive(Debug, Clone)]
struct Plan {
    account: Account,
    dp: u32,
    existing: Option<(Transaction, bool)>,
    feed: Option<Feed>,
    /// Raw trusted balance and its date.
    trusted: Option<(Decimal, Date)>,
    source: &'static str,
    start: Start,
    earliest: Option<Date>,
    imported: usize,
    /// The start date D.
    date: Option<Date>,
    /// Raw opening amount.
    opening: Option<Decimal>,
    pre_start: Vec<PreStart>,
    /// Raw balance on the as-of date after the change.
    after: Option<Decimal>,
    pending_counted: usize,
    pending_left_out: usize,
    duplicates: Vec<(TransactionId, TransactionId)>,
    after_as_of: usize,
    conflicts: Vec<Conflict>,
    offset: Option<AccountId>,
    offset_name: String,
    offset_create: bool,
    choices: Vec<Value>,
}

impl Plan {
    fn ty(&self) -> AccountType {
        self.account.account_type
    }
    fn show(&self, raw: Decimal) -> String {
        format_amount(self.ty().display(raw), Some(self.dp))
    }
    fn matches(&self) -> bool {
        match (self.after, self.trusted) {
            (Some(a), Some((t, _))) => a == t,
            _ => true,
        }
    }
}

/// A display-sign amount the user typed (owed positive for debts) as raw.
fn raw_of(ty: AccountType, display: Decimal, owes_me: bool) -> Decimal {
    let d = if owes_me && ty == AccountType::Liability {
        -display
    } else {
        display
    };
    ty.display(d)
}

#[allow(clippy::too_many_lines)]
fn compute(
    ctx: &Ctx<'_>,
    account: &str,
    a: &CashArgs,
    offset: Option<&str>,
    today: Date,
) -> CommandResult<Plan> {
    let chart = Chart::load(ctx)?;
    let acc = chart.resolve(account)?.clone();
    if !on_balance_sheet(&acc) {
        return Err(rejected(format!(
            "{} is not an asset or liability account that holds money; opening balances are for \
             balance-sheet accounts",
            chart.full_name(acc.id)
        )));
    }
    let dp = minor_units(acc.commodity.as_str());
    let ty = acc.account_type;
    let all = txns_of(ctx, acc.id)?;
    let existing = opening_entry(ctx, &chart, acc.id, &all)?;
    let ex_id = existing.as_ref().map(|(t, _)| t.id);
    let others: Vec<Transaction> = all
        .iter()
        .filter(|t| t.status == TxnStatus::Posted && Some(t.id) != ex_id)
        .cloned()
        .collect();
    // The earliest transaction of the account (with its holdings for a brokerage).
    let mut earliest = others.iter().map(|t| t.date).min();
    if acc.subtype == AccountSubtype::Brokerage {
        let metas = all_metas(ctx)?;
        for k in chart.children(acc.id) {
            for t in txns_of(ctx, k.id)? {
                let opening_meta = metas.get(&t.id).is_some_and(|_| t.opening);
                if t.status == TxnStatus::Posted && !opening_meta {
                    earliest = Some(earliest.map_or(t.date, |e: Date| e.min(t.date)));
                }
            }
        }
    }
    let feed = feed_of(ctx, acc.id)?;
    let pending = if feed.is_some() {
        pending_fingerprints(ctx)?
    } else {
        HashSet::new()
    };
    let include_pending = feed.as_ref().is_some_and(|f| f.include_pending);
    let is_pending = |t: &Transaction| {
        t.splits.iter().any(|s| {
            s.account == acc.id && s.fingerprint.as_ref().is_some_and(|f| pending.contains(f))
        })
    };
    let given = a.balance.map(|b| {
        (
            raw_of(ty, b, a.owes_me),
            a.as_of
                .or_else(|| feed.as_ref().and_then(|f| f.date))
                .unwrap_or(today),
        )
    });
    let fed = feed
        .as_ref()
        .and_then(|f| f.balance.map(|b| (b, a.as_of.or(f.date).unwrap_or(today))));
    let (mut trusted, mut source) = match (given, fed) {
        (Some(g), _) => (Some(g), "given"),
        (None, Some(f)) => (Some(f), "feed"),
        (None, None) => (None, "none"),
    };
    let mut date = None;
    let mut opening = None;
    let mut pre_start = Vec::new();
    if let (Some(amount), Some(d)) = (a.amount, a.date) {
        // Quick entry: the amount as given.
        let raw = raw_of(ty, amount, a.owes_me);
        date = Some(d);
        opening = Some(raw);
        source = "amount";
        let mut bal = raw;
        for t in others.iter().filter(|t| t.date <= d) {
            bal = plus(bal, amount_in(t, acc.id)?)?;
        }
        trusted = Some((bal, d));
    } else {
        if a.start == Start::Statement {
            trusted = match (a.statement_date, a.statement_balance) {
                (Some(d), Some(b)) => Some((raw_of(ty, b, a.owes_me), d)),
                _ => None,
            };
            source = if trusted.is_some() {
                "statement"
            } else {
                "none"
            };
            date = a.statement_date;
        }
        if let Some((bal, as_of)) = trusted {
            let d = match (a.start, date, earliest) {
                (Start::Statement, Some(d), _) => d,
                (_, _, Some(e)) => e.previous_day().unwrap_or(e),
                _ => as_of,
            };
            date = Some(d);
            if a.start == Start::Statement {
                for t in others.iter().filter(|t| t.date <= d) {
                    if reconciled_in(t, acc.id) {
                        continue;
                    }
                    pre_start.push(PreStart {
                        txn: t.clone(),
                        amount: amount_in(t, acc.id)?,
                        removed: !a.keep.contains(&t.id),
                    });
                }
            }
            let mut after = Decimal::ZERO;
            for t in others.iter().filter(|t| t.date > d && t.date <= as_of) {
                if include_pending || !is_pending(t) {
                    after = plus(after, amount_in(t, acc.id)?)?;
                }
            }
            opening = Some(minus(bal, after)?);
        }
    }
    let removed: HashSet<TransactionId> = pre_start
        .iter()
        .filter(|p| p.removed)
        .map(|p| p.txn.id)
        .collect();
    // The check: the balance on the as-of date after this entry.
    let after = match (trusted, opening) {
        (Some((_, as_of)), Some(o)) => {
            let mut b = o;
            for t in others
                .iter()
                .filter(|t| t.date <= as_of && !removed.contains(&t.id))
            {
                b = plus(b, amount_in(t, acc.id)?)?;
            }
            Some(b)
        }
        _ => None,
    };
    let window = |t: &&Transaction| {
        trusted.is_some_and(|(_, as_of)| t.date <= as_of)
            && date.is_some_and(|d| t.date > d)
            && is_pending(t)
    };
    let pending_n = others.iter().filter(window).count();
    let (pending_counted, pending_left_out) = if include_pending {
        (pending_n, 0)
    } else {
        (0, pending_n)
    };
    // Possible duplicates: the same amount within 3 days.
    let mut duplicates = Vec::new();
    let kept: Vec<&Transaction> = others.iter().filter(|t| !removed.contains(&t.id)).collect();
    for (i, x) in kept.iter().enumerate() {
        for y in kept.iter().skip(i + 1) {
            if y.date.days_until(x.date).abs() <= 3
                && amount_in(x, acc.id)? == amount_in(y, acc.id)?
            {
                duplicates.push((x.id, y.id));
            }
        }
    }
    let after_as_of = trusted.map_or(0, |(_, as_of)| {
        others.iter().filter(|t| t.date > as_of).count()
    });
    // Finished reconciliations whose period would no longer add up.
    let mut conflicts = Vec::new();
    if let (Some(d), Some(o)) = (date, opening) {
        let balance_on = |day: Date, new: bool| -> CommandResult<Decimal> {
            let mut b = Decimal::ZERO;
            if new {
                if d <= day {
                    b = o;
                }
            } else if let Some((t, _)) = &existing
                && t.status == TxnStatus::Posted
                && t.date <= day
            {
                b = amount_in(t, acc.id)?;
            }
            for t in others.iter().filter(|t| t.date <= day) {
                if new && removed.contains(&t.id) {
                    continue;
                }
                b = plus(b, amount_in(t, acc.id)?)?;
            }
            Ok(b)
        };
        for s in crate::reconcile::sessions(ctx, acc.id)?
            .into_iter()
            .filter(|s| s.status == "finished")
        {
            if balance_on(s.statement_date, false)? != balance_on(s.statement_date, true)? {
                conflicts.push(Conflict {
                    reconciliation: Some(s.id.clone()),
                    date: s.statement_date,
                    ending: s.statement_balance,
                });
            }
        }
        if let Some((t, _)) = &existing
            && reconciled_in(t, acc.id)
            && (t.date != d || amount_in(t, acc.id)? != o)
            && conflicts.is_empty()
        {
            conflicts.push(Conflict {
                reconciliation: None,
                date: t.date,
                ending: ty.display(amount_in(t, acc.id)?),
            });
        }
        conflicts.sort_by_key(|c| c.date);
    }
    // The other side.
    let ob = opening_account(ctx, &chart)?;
    let biz = business(ctx)?;
    let mut choices = vec![json!({
        "key": "opening-balances", "account": ob, "name": "Opening Balances",
        "why": WHY_OPENING, "create": ob.is_none(),
    })];
    let equity = owners_equity_of(&chart);
    if biz {
        choices.push(json!({
            "key": "owners-equity", "account": equity, "name": OWNERS_EQUITY,
            "why": WHY_EQUITY, "create": equity.is_none(),
        }));
    }
    if ty == AccountType::Asset
        && matches!(
            acc.subtype,
            AccountSubtype::OtherAsset | AccountSubtype::Receivable
        )
        && is_module_enabled(ctx.tx, "invoicing")?
        && let Some(r) = crate::chart::roles::holder(&chart, crate::chart::roles::RECEIVABLE)
        && r.id != acc.id
    {
        choices.push(json!({
            "key": r.id.to_string(), "account": r.id, "name": chart.full_name(r.id),
            "why": WHY_RECEIVABLE, "create": false,
        }));
    }
    let existing_offset = existing.as_ref().and_then(|(t, _)| other_side(t, acc.id));
    let (offset, offset_name, offset_create) = match offset.map(str::trim) {
        None | Some("") => match existing_offset {
            Some(id) => (Some(id), chart.full_name(id), false),
            None => (ob, "Opening Balances".to_owned(), ob.is_none()),
        },
        Some("opening-balances") => (ob, "Opening Balances".to_owned(), ob.is_none()),
        Some("owners-equity") => (
            equity,
            equity.map_or_else(|| OWNERS_EQUITY.to_owned(), |id| chart.full_name(id)),
            equity.is_none(),
        ),
        Some(r) => {
            let o = chart.resolve(r)?;
            if o.id == acc.id || o.placeholder {
                return Err(invalid(
                    "the other side must be another account that is not a heading",
                ));
            }
            (Some(o.id), chart.full_name(o.id), false)
        }
    };
    Ok(Plan {
        dp,
        imported: others.len(),
        existing,
        feed,
        trusted,
        source,
        start: a.start,
        earliest,
        date,
        opening,
        pre_start,
        after,
        pending_counted,
        pending_left_out,
        duplicates,
        after_as_of,
        conflicts,
        offset,
        offset_name,
        offset_create,
        choices,
        account: acc,
    })
}

fn owners_equity_of(chart: &Chart) -> Option<AccountId> {
    chart
        .accounts
        .iter()
        .filter(|a| a.subtype == AccountSubtype::OwnerEquity && !a.placeholder && !a.archived)
        .min_by_key(|a| {
            (
                a.name != OWNERS_EQUITY,
                chart.full_name(a.id).to_lowercase(),
            )
        })
        .map(|a| a.id)
}

fn kind_text(a: &Account) -> &'static str {
    match a.subtype {
        AccountSubtype::Bank => "bank account",
        AccountSubtype::Cash => "cash",
        AccountSubtype::CreditCard => "credit card",
        AccountSubtype::Loan => "loan",
        AccountSubtype::Brokerage => "brokerage account",
        AccountSubtype::Receivable => "money owed to you",
        AccountSubtype::FixedAsset => "property or equipment",
        _ if a.account_type == AccountType::Liability => "debt",
        _ => "asset",
    }
}

#[allow(clippy::too_many_lines)]
fn plan_json(ctx: &Ctx<'_>, p: &Plan) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let acc = &p.account;
    let liability = acc.account_type == AccountType::Liability;
    let money = |raw: Decimal| p.show(raw);
    let existing = match &p.existing {
        Some((t, legacy)) => {
            let other = other_side(t, acc.id);
            json!({
                "txn": t.id, "date": t.date, "amount": money(amount_in(t, acc.id)?),
                "offset": other, "offset_name": other.map(|o| chart.full_name(o)),
                "legacy": legacy, "reconciled": reconciled_in(t, acc.id),
            })
        }
        None => Value::Null,
    };
    let feed = match &p.feed {
        Some(f) => json!({
            "balance": f.balance.map(&money), "date": f.date,
            "include_pending": f.include_pending, "connection": f.connection, "link": f.link,
            "pending_text": if f.include_pending {
                "Pending transactions are included, as set for this account."
            } else {
                "Pending transactions are not included, as set for this account."
            },
        }),
        None => Value::Null,
    };
    let pre: Vec<Value> = p
        .pre_start
        .iter()
        .map(|x| {
            json!({
                "txn": x.txn.id, "date": x.txn.date, "payee": x.txn.payee,
                "amount": money(x.amount), "removed": x.removed,
            })
        })
        .collect();
    let removed = p.pre_start.iter().filter(|x| x.removed).count();
    let mut causes = Vec::new();
    if !p.matches() {
        if let Some(f) = &p.feed {
            causes.push(json!({
                "kind": "pending", "included": f.include_pending,
                "count": p.pending_counted + p.pending_left_out,
                "text": if f.include_pending {
                    "Pending transactions are included. Your bank's balance may leave them out."
                } else {
                    "Pending transactions are not included. Your bank's balance may count them."
                },
                "link": f.link, "connection": f.connection,
            }));
        }
        if !p.duplicates.is_empty() {
            causes.push(json!({
                "kind": "duplicates", "count": p.duplicates.len(),
                "text": format!("{} possible duplicate{} (the same amount within 3 days)",
                    p.duplicates.len(), if p.duplicates.len() == 1 { "" } else { "s" }),
                "txns": p.duplicates.iter().flat_map(|(a, b)| [*a, *b]).collect::<Vec<_>>(),
            }));
        }
        if p.after_as_of > 0 {
            causes.push(json!({
                "kind": "after", "count": p.after_as_of,
                "text": format!("{} transaction{} dated after the balance's date",
                    p.after_as_of, if p.after_as_of == 1 { " is" } else { "s are" }),
            }));
        }
        let kept: Vec<&PreStart> = p.pre_start.iter().filter(|x| !x.removed).collect();
        if !kept.is_empty() {
            let mut sum = Decimal::ZERO;
            for k in &kept {
                sum = plus(sum, k.amount)?;
            }
            causes.push(json!({
                "kind": "kept", "count": kept.len(), "amount": money(sum),
                "text": format!("{} transaction{} before your start date {} kept (already in the starting balance)",
                    kept.len(), if kept.len() == 1 { "" } else { "s" },
                    if kept.len() == 1 { "is" } else { "are" }),
            }));
        }
        causes.push(json!({
            "kind": "as_of",
            "text": "The balance may be for a different date: check the date your bank shows.",
        }));
    }
    let conflicts: Vec<Value> = p
        .conflicts
        .iter()
        .map(|c| {
            json!({
                "reconciliation": c.reconciliation, "date": c.date,
                "ending": format_amount(c.ending, Some(p.dp)),
            })
        })
        .collect();
    let difference = match (p.after, p.trusted) {
        (Some(a), Some((t, _))) => Some(money(minus(t, a)?)),
        _ => None,
    };
    Ok(json!({
        "account": {
            "id": acc.id, "name": acc.name, "full_name": chart.full_name(acc.id),
            "type": acc.account_type, "subtype": acc.subtype, "kind": kind_text(acc),
            "liability": liability, "retirement": acc.retirement,
            "opening_none": acc.opening_none,
            "question": if liability { "How much do you owe?" } else { "How much is in this account?" },
            "wizard": if acc.subtype == AccountSubtype::Brokerage { "investment" } else { "money" },
        },
        "existing": existing,
        "feed": feed,
        "ready": p.opening.is_some(),
        "source": p.source,
        "trusted": p.trusted.map(|(b, d)| json!({"amount": money(b), "as_of": d,
            "owes_me": liability && b.is_sign_positive() && !b.is_zero()})),
        "start": {
            "mode": p.start, "date": p.date, "earliest": p.earliest, "imported": p.imported,
            "nothing": p.imported == 0,
        },
        "pre_start": pre,
        "removed": removed,
        "opening": p.opening.map(|o| json!({"amount": money(o), "date": p.date,
            "zero": o.is_zero()})),
        "offset": {
            "account": p.offset, "name": p.offset_name, "create": p.offset_create,
            "choices": p.choices,
        },
        "check": p.after.map(|a| json!({
            "balance": money(a), "trusted": p.trusted.map(|(t, _)| money(t)),
            "difference": difference, "matches": p.matches(),
        })),
        "causes": causes,
        "reconciled": conflicts,
        "loan": acc.subtype == AccountSubtype::Loan,
        "business": business(ctx)?,
    }))
}

fn plan_cmd(ctx: &Ctx<'_>, a: MoneyArgs) -> CommandResult<Value> {
    let today = crate::sales_reports::today_or(a.today)?;
    let p = compute(ctx, &a.account, &a.cash, a.offset.as_deref(), today)?;
    plan_json(ctx, &p)
}

// ----- posting ---------------------------------------------------------------------------

/// The other side's account, created when needed.
fn offset_account(ctx: &Ctx<'_>, p: &Plan, key: Option<&str>) -> CommandResult<AccountId> {
    if let (Some(id), false) = (p.offset, p.offset_create) {
        return Ok(id);
    }
    let mut chart = Chart::load(ctx)?;
    if key.map(str::trim) == Some("owners-equity") {
        return crate::investments::ensure_top(
            ctx,
            &mut chart,
            OWNERS_EQUITY,
            AccountType::Equity,
            AccountSubtype::OwnerEquity,
        );
    }
    crate::investments::opening_balances(ctx, &mut chart)
}

fn opening_txn(
    ctx: &Ctx<'_>,
    old: Option<&Transaction>,
    account: AccountId,
    offset: AccountId,
    date: Date,
    raw: Decimal,
) -> CommandResult<Transaction> {
    let currency: CommodityCode = ctx.tx.info()?.base_currency.parse()?;
    let split = |acc: AccountId, v: Decimal| Split {
        account: acc,
        value: v,
        quantity: v,
        memo: None,
        reconcile: Reconcile::New,
        fingerprint: None,
        tags: mk_core::SplitTags::default(),
    };
    let mut splits = vec![split(account, raw), split(offset, -raw)];
    mk_ledger::sort_splits(&mut splits);
    let mut t = Transaction {
        id: old.map_or_else(TransactionId::new, |o| o.id),
        date,
        currency,
        payee: Some(PAYEE.to_owned()),
        number: old.and_then(|o| o.number.clone()),
        memo: old.and_then(|o| o.memo.clone()),
        status: TxnStatus::Posted,
        void_reason: None,
        reverses: None,
        entered_at: old.map_or_else(now_rfc3339, |o| Ok(o.entered_at.clone()))?,
        splits,
        opening: true,
    };
    if let Some(o) = old {
        mk_ledger::inherit_reconcile(&o.splits, &mut t.splits);
    }
    Ok(t)
}

/// Post the plan: removals, the opening entry and the reconciliations to check.
/// Returns the opening transaction (none when the amount is zero).
fn post_plan(
    ctx: &Ctx<'_>,
    p: &Plan,
    offset_key: Option<&str>,
    go_ahead: bool,
) -> CommandResult<Option<Transaction>> {
    let (Some(date), Some(raw)) = (p.date, p.opening) else {
        return Err(invalid(
            "give the balance you trust (and its date), or a statement's ending date and balance",
        ));
    };
    if !p.conflicts.is_empty() && !go_ahead {
        let c = p.conflicts.first();
        return Err(CommandError::Coded {
            code: "reconciled_period",
            message: format!(
                "Your reconciliation of {} ended at {}. This change would make that period not \
                 add up. Choose an earlier start date, or go ahead and the reconciliation will \
                 be marked to check.",
                c.map(|c| c.date.to_string()).unwrap_or_default(),
                c.map(|c| format_amount(c.ending, Some(p.dp)))
                    .unwrap_or_default()
            ),
        });
    }
    let acc = p.account.id;
    // Removed pre-start transactions: remembered so the feed does not bring them back.
    for x in p.pre_start.iter().filter(|x| x.removed) {
        let Some(cur) = ctx.tx.transaction(x.txn.id)? else {
            continue;
        };
        for s in cur.splits.iter().filter(|s| s.account == acc) {
            if let Some(fp) = &s.fingerprint {
                let id = forgotten_id(acc, fp);
                ctx.save_doc(
                    DocTable::ImportForgotten,
                    &id,
                    Some(&Doc {
                        id: id.clone(),
                        grp: Some(acc.to_string()),
                        ord: crate::investments::date_ord(cur.date),
                        json: json!({"account": acc, "fingerprint": fp, "txn": cur.id,
                            "date": cur.date, "payee": cur.payee}),
                    }),
                )?;
            }
        }
        validate_change(ctx, Some(&cur), None)?;
        crate::txns::remove_with_effects(ctx, &cur)?;
    }
    let old = p.existing.as_ref().map(|(t, _)| t.clone());
    let posted = if raw.is_zero() {
        // A zero opening balance is no transaction: the account started from zero.
        if let Some(o) = &old {
            with_unreconciling(ctx, go_ahead, || validate_change(ctx, Some(o), None))?;
            crate::txns::remove_with_effects(ctx, o)?;
        }
        set_opening_none(ctx, acc, true)?;
        None
    } else {
        let offset = offset_account(ctx, p, offset_key)?;
        let t = opening_txn(ctx, old.as_ref(), acc, offset, date, raw)?;
        with_unreconciling(ctx, go_ahead, || {
            validate_change(ctx, old.as_ref(), Some(&t))
        })?;
        if old.as_ref() != Some(&t) {
            ctx.save_txn(old.as_ref(), &t)?;
        }
        set_opening_none(ctx, acc, false)?;
        Some(t)
    };
    if go_ahead {
        for c in &p.conflicts {
            let Some(id) = &c.reconciliation else {
                continue;
            };
            if let Some(mut s) = crate::reconcile::sessions(ctx, acc)?
                .into_iter()
                .find(|s| &s.id == id)
                && !s.to_check
            {
                s.to_check = true;
                crate::reconcile::save(ctx, &s)?;
            }
        }
    }
    Ok(posted)
}

fn with_unreconciling<T>(
    ctx: &Ctx<'_>,
    on: bool,
    f: impl FnOnce() -> CommandResult<T>,
) -> CommandResult<T> {
    let before = ctx.unreconciling.get();
    if on {
        ctx.unreconciling.set(true);
    }
    let out = f();
    ctx.unreconciling.set(before);
    out
}

fn set_opening_none(ctx: &Ctx<'_>, account: AccountId, on: bool) -> CommandResult<()> {
    let Some(old) = ctx.tx.account(account)? else {
        return Ok(());
    };
    if old.opening_none != on {
        let mut new = old.clone();
        new.opening_none = on;
        ctx.save_account(Some(&old), &new)?;
    }
    Ok(())
}

/// The asset a loan is for: a new asset account with its own opening entry.
fn loan_asset(
    ctx: &Ctx<'_>,
    p: &Plan,
    asset: &AssetArg,
    offset: AccountId,
) -> CommandResult<Value> {
    let date = p
        .date
        .ok_or_else(|| invalid("the loan's start date is not known yet"))?;
    if asset.value.is_sign_negative() || asset.value.is_zero() {
        return Err(invalid("give what the asset is worth (more than zero)"));
    }
    let chart = Chart::load(ctx)?;
    let probe = Account {
        id: AccountId::new(),
        name: String::new(),
        account_type: AccountType::Asset,
        subtype: AccountSubtype::FixedAsset,
        parent: None,
        commodity: p.account.commodity.clone(),
        code: None,
        description: None,
        placeholder: false,
        archived: false,
        created_at: String::new(),
        tax_line: None,
        role: None,
        retirement: false,
        opening_none: false,
    };
    let parent = crate::accounts::default_group(&chart, &probe).or_else(|| {
        chart
            .accounts
            .iter()
            .filter(|g| {
                g.placeholder
                    && !g.archived
                    && g.account_type == AccountType::Asset
                    && g.parent.is_none()
            })
            .min_by_key(|g| g.name.to_lowercase())
            .map(|g| g.id)
    });
    let created = crate::accounts::create_json(
        ctx,
        json!({"name": asset.name, "type": "asset", "subtype": "fixed-asset",
               "parent": parent.map(|p| p.to_string())}),
    )?;
    let id: AccountId = created
        .get("id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| CommandError::Integrity("created account without an id".into()))?;
    let t = opening_txn(ctx, None, id, offset, date, asset.value)?;
    validate_change(ctx, None, Some(&t))?;
    ctx.save_txn(None, &t)?;
    Ok(
        json!({"account": id, "name": chart_name(ctx, id)?, "transaction": t.id,
        "amount": format_amount(asset.value, Some(p.dp))}),
    )
}

fn chart_name(ctx: &Ctx<'_>, id: AccountId) -> CommandResult<String> {
    Ok(Chart::load(ctx)?.full_name(id))
}

fn apply_cmd(ctx: &Ctx<'_>, a: MoneyArgs) -> CommandResult<Value> {
    let today = crate::sales_reports::today_or(a.today)?;
    let p = compute(ctx, &a.account, &a.cash, a.offset.as_deref(), today)?;
    if p.account.subtype == AccountSubtype::Brokerage && a.asset.is_some() {
        return Err(invalid("only a loan can be for something you own"));
    }
    let posted = post_plan(ctx, &p, a.offset.as_deref(), a.go_ahead)?;
    let mut asset = Value::Null;
    if let Some(x) = &a.asset {
        if p.account.subtype != AccountSubtype::Loan {
            return Err(invalid("only a loan can be for something you own"));
        }
        let offset = match &posted {
            Some(t) => other_side(t, p.account.id),
            None => None,
        };
        let offset = match offset {
            Some(o) => o,
            None => offset_account(ctx, &p, a.offset.as_deref())?,
        };
        asset = loan_asset(ctx, &p, x, offset)?;
    }
    let chart = Chart::load(ctx)?;
    Ok(json!({
        "account": p.account.id,
        "full_name": chart.full_name(p.account.id),
        "transaction": posted.as_ref().map(|t| crate::txns::txn_json(&chart, t)).transpose()?,
        "amount": p.opening.map(|o| p.show(o)),
        "date": p.date,
        "removed": p.pre_start.iter().filter(|x| x.removed).count(),
        "asset": asset,
        "reconciliations_to_check": if a.go_ahead { p.conflicts.len() } else { 0 },
        "started_from_zero": posted.is_none(),
    }))
}

// ----- the checklist ---------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ChecklistArgs {
    /// Only these accounts (ids, full names or codes), e.g. those a feed just linked.
    #[serde(default)]
    accounts: Option<Vec<String>>,
}

/// Opening shares in transactions of a brokerage's holdings, with their records.
fn opening_holdings(
    ctx: &Ctx<'_>,
    chart: &Chart,
    brokerage: AccountId,
) -> CommandResult<Vec<(Transaction, InvMeta)>> {
    let mut out = Vec::new();
    for k in chart
        .children(brokerage)
        .filter(|k| k.subtype == AccountSubtype::Security)
    {
        for t in txns_of(ctx, k.id)? {
            if !t.opening || t.status != TxnStatus::Posted {
                continue;
            }
            if let Some(m) = meta_of(ctx, t.id)?
                && m.action == Action::SharesIn
            {
                out.push((t, m));
            }
        }
    }
    Ok(out)
}

fn needs_basis(m: &InvMeta) -> bool {
    m.lots
        .as_ref()
        .is_some_and(|l| l.iter().any(|x| x.basis_needed))
}

/// Holdings (security accounts) whose opening lots still need their cost basis.
pub(crate) fn basis_needed_holdings(
    ctx: &Ctx<'_>,
    brokerage: Option<AccountId>,
) -> CommandResult<Vec<(AccountId, AccountId)>> {
    let mut out = Vec::new();
    for m in all_metas(ctx)?.values() {
        if m.action != Action::SharesIn || !needs_basis(m) {
            continue;
        }
        let (Some(b), Some(sec)) = (m.account, m.sec_account) else {
            continue;
        };
        if brokerage.is_some_and(|x| x != b) {
            continue;
        }
        let posted = ctx
            .tx
            .transaction(m.txn)?
            .is_some_and(|t| t.status == TxnStatus::Posted);
        if posted && !out.contains(&(b, sec)) {
            out.push((b, sec));
        }
    }
    Ok(out)
}

/// The lot ids (`<txn>:<n>`) of shares in lots flagged Basis needed and Various.
pub(crate) fn lot_flags(ctx: &Ctx<'_>) -> CommandResult<HashMap<String, (bool, bool)>> {
    let mut out = HashMap::new();
    for m in all_metas(ctx)?.values() {
        if m.action != Action::SharesIn {
            continue;
        }
        for (i, l) in m.lots.iter().flatten().enumerate() {
            if l.basis_needed || l.acquired_various {
                out.insert(
                    format!("{}:{}", m.txn, i + 1),
                    (l.basis_needed, l.acquired_various),
                );
            }
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_lines)]
fn checklist_cmd(ctx: &Ctx<'_>, a: ChecklistArgs) -> CommandResult<Value> {
    checklist(ctx, a.accounts.as_deref())
}

pub(crate) fn checklist(ctx: &Ctx<'_>, only: Option<&[String]>) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let only: Option<HashSet<AccountId>> = only
        .map(|refs| {
            refs.iter()
                .map(|r| chart.resolve(r).map(|a| a.id))
                .collect::<CommandResult<HashSet<_>>>()
        })
        .transpose()?;
    let linked: HashSet<String> = ctx
        .tx
        .docs(DocTable::AccountLink, None)?
        .into_iter()
        .filter_map(|d| serde_json::from_value::<crate::connect::LinkDoc>(d.json).ok())
        .filter(|l| l.active)
        .filter_map(|l| l.account)
        .collect();
    let inv = crate::investments::is_enabled(ctx)?;
    let basis = if inv {
        basis_needed_holdings(ctx, None)?
    } else {
        Vec::new()
    };
    let mut rows = Vec::new();
    let (mut done, mut total, mut need) = (0usize, 0usize, 0usize);
    for (acc, _) in chart.tree_order() {
        if !on_balance_sheet(acc) || only.as_ref().is_some_and(|o| !o.contains(&acc.id)) {
            continue;
        }
        let dp = minor_units(acc.commodity.as_str());
        let txns = txns_of(ctx, acc.id)?;
        let mut used = txns.iter().any(|t| t.status == TxnStatus::Posted);
        let brokerage = acc.subtype == AccountSubtype::Brokerage;
        let entry = opening_entry(ctx, &chart, acc.id, &txns)?;
        let mut holdings_open = Vec::new();
        if brokerage {
            for k in chart.children(acc.id) {
                used |= ctx.tx.account_has_live_splits(k.id)?;
            }
            if inv {
                holdings_open = opening_holdings(ctx, &chart, acc.id)?;
            }
        }
        let feed = linked.contains(&acc.id.to_string());
        let needing: Vec<String> = basis
            .iter()
            .filter(|(b, _)| *b == acc.id)
            .map(|(_, s)| {
                chart
                    .get(*s)
                    .map(|a| a.commodity.to_string())
                    .unwrap_or_default()
            })
            .collect();
        let set_up = entry.is_some() || !holdings_open.is_empty();
        let status = if acc.opening_none && !set_up {
            "zero"
        } else if !needing.is_empty() {
            "basis_needed"
        } else if set_up {
            "done"
        } else {
            "not_set"
        };
        if status != "zero" {
            total += 1;
        }
        if matches!(status, "done" | "basis_needed") {
            done += 1;
        }
        let needs = status == "not_set" && (used || feed);
        if needs {
            need += 1;
        }
        let (amount, date) = match &entry {
            Some((t, _)) => (
                Some(format_amount(
                    acc.account_type.display(amount_in(t, acc.id)?),
                    Some(dp),
                )),
                Some(t.date),
            ),
            None => (None, holdings_open.first().map(|(t, _)| t.date)),
        };
        rows.push(json!({
            "account": acc.id, "name": acc.name, "full_name": chart.full_name(acc.id),
            "type": acc.account_type, "subtype": acc.subtype, "kind": kind_text(acc),
            "status": status, "amount": amount, "date": date,
            "holdings": holdings_open.len(), "basis_needed": needing,
            "needs": needs, "has_transactions": used, "feed": feed,
            "wizard": if brokerage { "investment" } else { "money" },
            "retirement": acc.retirement,
        }));
    }
    let percent = done.saturating_mul(100).checked_div(total).unwrap_or(100);
    let encouragement = if total > 0 && done == total {
        Some("All accounts are set up. Your net worth now reflects everything you own and owe.")
    } else if total > 0 && done.saturating_mul(2) >= total {
        Some("Halfway there.")
    } else {
        None
    };
    Ok(json!({
        "accounts": rows,
        "done": done,
        "total": total,
        "percent": percent,
        "progress": format!("{done} of {total} accounts set up"),
        "encouragement": encouragement,
        "need_opening": need,
        "holdings_need_basis": basis.len(),
        "basis_text": (!basis.is_empty()).then(|| holdings_text(basis.len())),
        "basis_where": BASIS_WHERE,
    }))
}

pub(crate) fn holdings_text(n: usize) -> String {
    if n == 1 {
        "1 holding needs cost basis".to_owned()
    } else {
        format!("{n} holdings need cost basis")
    }
}

pub(crate) fn accounts_text(n: usize) -> String {
    if n == 1 {
        "1 account needs an opening balance".to_owned()
    } else {
        format!("{n} accounts need an opening balance")
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ZeroArgs {
    /// Account id, full name or code.
    account: String,
    /// true: started from zero (needs no opening balance); false: undo that.
    #[serde(default = "yes")]
    on: bool,
}

fn yes() -> bool {
    true
}

fn started_from_zero(ctx: &Ctx<'_>, a: ZeroArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let acc = chart.resolve(&a.account)?.clone();
    if !on_balance_sheet(&acc) {
        return Err(rejected("only a balance-sheet account can start from zero"));
    }
    set_opening_none(ctx, acc.id, a.on)?;
    Ok(json!({"account": acc.id, "full_name": chart.full_name(acc.id), "opening_none": a.on}))
}

// ----- the investment wizard -------------------------------------------------------------

/// How a holding's cost basis is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Level {
    /// Exact lots: purchase date, shares and cost of each.
    Lots,
    /// One total cost (or an average cost per share).
    Total,
    /// I don't know yet: market value on the start date, flagged Basis needed.
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct LotIn {
    pub acquired: Date,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    pub shares: Decimal,
    /// The lot's total cost.
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    pub cost: Decimal,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BasisArg {
    pub level: Level,
    #[serde(default)]
    pub lots: Vec<LotIn>,
    /// One total: the total cost …
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub total: Option<Decimal>,
    /// … or the average cost per share (multiplied by the shares on the start date).
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub per_share: Option<Decimal>,
    /// One total: when they were bought (default: Various).
    #[serde(default)]
    pub acquired: Option<Date>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HoldingArg {
    pub symbol: String,
    /// For a security not in the book yet.
    #[serde(default)]
    pub name: Option<String>,
    /// For a security not in the book yet: stock, etf, mutual_fund, money_market,
    /// option (the symbol gives its terms, e.g. "AAPL 261218 C200"), bond, crypto or
    /// other.
    #[serde(default)]
    pub kind: Option<SecurityKind>,
    /// Shares today (on the balance's as-of date), as the broker shows them: options
    /// in contracts (negative when written), bonds in units, crypto up to 8 decimals.
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    pub shares: Decimal,
    /// A new bond: its maturity date (with it the bond gets face, coupon and
    /// maturity; units are of `face_per_unit`, prices in percent of face).
    #[serde(default)]
    pub maturity: Option<Date>,
    /// A new bond: the coupon rate in percent.
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub coupon_rate: Option<Decimal>,
    /// A new bond: the face of one unit (default 1,000).
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub face_per_unit: Option<Decimal>,
    /// Absent: not chosen yet.
    #[serde(default)]
    pub basis: Option<BasisArg>,
    /// The price on the start date, when the book has none.
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub price: Option<Decimal>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InvArgs {
    /// Brokerage account (id, full name or code).
    pub account: String,
    /// The account's cash, worked out like a money account.
    #[serde(default)]
    pub cash: CashArgs,
    #[serde(default)]
    pub holdings: Vec<HoldingArg>,
    /// The account's total as the broker shows it on the as-of date.
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub broker_total: Option<Decimal>,
    /// This is a retirement account (stored on the account).
    #[serde(default)]
    pub retirement: Option<bool>,
    /// Retirement accounts: enter cost basis anyway.
    #[serde(default)]
    pub enter_basis: bool,
    #[serde(default)]
    pub offset: Option<String>,
    #[serde(default)]
    pub go_ahead: bool,
    /// Create the securities not in the book yet.
    #[serde(default)]
    pub create_securities: bool,
    #[serde(default)]
    pub today: Option<Date>,
}

/// One holding worked out.
#[derive(Debug, Clone)]
struct HoldingPlan {
    /// A written option (short lots; `on_start` is the number of contracts).
    short: bool,
    symbol: String,
    name: String,
    security: Option<String>,
    kind: SecurityKind,
    today: Decimal,
    on_start: Decimal,
    price_start: Option<(Date, Decimal)>,
    price_as_of: Option<(Date, Decimal)>,
    stale: bool,
    value: Option<Decimal>,
    lots: Option<Vec<LotArg>>,
    level: Option<Level>,
    basis: Option<Decimal>,
    problem: Option<String>,
    existing: Option<(Transaction, InvMeta)>,
}

/// Prices older than this many days before the as-of date are out of date.
const STALE_DAYS: i64 = 7;

/// Shares on `d` from shares today: back out buys, sales and splits after `d`.
fn shares_on(
    ctx: &Ctx<'_>,
    sec_account: Option<AccountId>,
    today_q: Decimal,
    d: Date,
    as_of: Date,
) -> CommandResult<Decimal> {
    let Some(sa) = sec_account else {
        return Ok(today_q);
    };
    let metas = all_metas(ctx)?;
    let mut events: Vec<(Transaction, Option<InvMeta>)> = txns_of(ctx, sa)?
        .into_iter()
        .filter(|t| t.status == TxnStatus::Posted && t.date > d && t.date <= as_of && !t.opening)
        .map(|t| {
            let m = metas.get(&t.id).cloned();
            (t, m)
        })
        .collect();
    events.reverse();
    let mut q = today_q;
    for (t, m) in events {
        match m {
            Some(m) if m.action == Action::Split => {
                if let (Some(n), Some(o)) = (m.ratio_new, m.ratio_old)
                    && !n.is_zero()
                {
                    q = q
                        .checked_mul(o)
                        .and_then(|x| x.checked_div(n))
                        .ok_or_else(|| rejected("arithmetic overflow"))?;
                }
            }
            _ => q = minus(q, amount_in(&t, sa)?)?,
        }
    }
    Ok(q.normalize())
}

fn lots_for(
    h: &HoldingArg,
    on_start: Decimal,
    d: Date,
    market: Option<Decimal>,
    retirement: bool,
    enter_basis: bool,
) -> Result<(Option<Vec<LotArg>>, Option<Level>), String> {
    let at_market = |needed: bool| -> Result<Vec<LotArg>, String> {
        let basis = market.ok_or_else(|| {
            format!(
                "{}: there is no price on the start date; type the price",
                h.symbol
            )
        })?;
        Ok(vec![LotArg {
            acquired: Some(d),
            quantity: on_start,
            basis: Some(basis),
            basis_needed: needed,
            acquired_various: true,
            ..LotArg::default()
        }])
    };
    if retirement && !enter_basis {
        return Ok((Some(at_market(false)?), None));
    }
    let Some(b) = &h.basis else {
        return Ok((None, None));
    };
    match b.level {
        Level::Unknown => Ok((Some(at_market(true)?), Some(Level::Unknown))),
        Level::Total => {
            let total = match (b.total, b.per_share) {
                (Some(t), _) => t,
                (None, Some(per)) => crate::investments::round2(
                    on_start
                        .checked_mul(per)
                        .ok_or_else(|| "arithmetic overflow".to_owned())?,
                ),
                (None, None) => {
                    return Err(format!(
                        "{}: give the total cost or the average cost per share",
                        h.symbol
                    ));
                }
            };
            if total.is_sign_negative() {
                return Err(format!("{}: a cost cannot be negative", h.symbol));
            }
            Ok((
                Some(vec![LotArg {
                    acquired: Some(b.acquired.unwrap_or(d)),
                    quantity: on_start,
                    basis: Some(total),
                    acquired_various: b.acquired.is_none(),
                    ..LotArg::default()
                }]),
                Some(Level::Total),
            ))
        }
        Level::Lots => {
            if b.lots.is_empty() {
                return Err(format!("{}: add the lots", h.symbol));
            }
            let mut sum = Decimal::ZERO;
            let mut out = Vec::new();
            for l in &b.lots {
                if l.shares <= Decimal::ZERO || l.cost.is_sign_negative() {
                    return Err(format!(
                        "{}: each lot needs shares and a cost of zero or more",
                        h.symbol
                    ));
                }
                if l.acquired > d {
                    return Err(format!(
                        "{}: a lot bought after the start date is a purchase, not an opening holding",
                        h.symbol
                    ));
                }
                sum = sum
                    .checked_add(l.shares)
                    .ok_or_else(|| "arithmetic overflow".to_owned())?;
                out.push(LotArg {
                    acquired: Some(l.acquired),
                    quantity: l.shares,
                    basis: Some(l.cost),
                    ..LotArg::default()
                });
            }
            if sum.normalize() != on_start.normalize() {
                return Err(format!(
                    "{}: the lots add up to {} shares, not {}",
                    h.symbol,
                    sum.normalize(),
                    on_start.normalize()
                ));
            }
            Ok((Some(out), Some(Level::Lots)))
        }
    }
}

struct InvPlan {
    cash: Plan,
    retirement: bool,
    rows: Vec<HoldingPlan>,
    total: Option<Decimal>,
    feed_rows: Vec<Value>,
    feed_cash: Option<String>,
    removed_holdings: Vec<(Transaction, InvMeta)>,
}

fn inv_compute(ctx: &Ctx<'_>, a: &InvArgs) -> CommandResult<InvPlan> {
    let today = crate::sales_reports::today_or(a.today)?;
    let chart = Chart::load(ctx)?;
    let b = crate::investments::brokerage(&chart, Some(&a.account))?.clone();
    let mut cash_args = a.cash.clone();
    let feed = feed_of(ctx, b.id)?;
    // SnapTrade: the provider's cash figure when there is one.
    let feed_cash = feed
        .as_ref()
        .and_then(|f| f.holdings.as_ref())
        .and_then(|h| h.get("cash"))
        .and_then(|c| c.get("provider"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let feed_as_of: Option<Date> = feed
        .as_ref()
        .and_then(|f| f.holdings.as_ref())
        .and_then(|h| h.get("as_of"))
        .and_then(|d| serde_json::from_value(d.clone()).ok());
    if cash_args.balance.is_none()
        && cash_args.start == Start::Keep
        && cash_args.amount.is_none()
        && feed.as_ref().is_none_or(|f| f.balance.is_none())
    {
        // The provider's cash, else the cash the book shows today (nothing changes).
        let book = {
            let mut sum = Decimal::ZERO;
            for t in txns_of(ctx, b.id)?
                .iter()
                .filter(|t| t.status == TxnStatus::Posted && t.date <= today)
            {
                sum = plus(sum, amount_in(t, b.id)?)?;
            }
            sum
        };
        cash_args.balance = Some(
            feed_cash
                .as_deref()
                .and_then(|c| c.parse().ok())
                .unwrap_or(book),
        );
        if cash_args.as_of.is_none() {
            cash_args.as_of = feed_as_of;
        }
    }
    let cash = compute(ctx, &a.account, &cash_args, a.offset.as_deref(), today)?;
    let retirement = a.retirement.unwrap_or(b.retirement);
    let d = cash.date.unwrap_or(today);
    let as_of = cash.trusted.map_or(today, |(_, x)| x);
    let secs = securities(ctx)?;
    let existing = opening_holdings(ctx, &chart, b.id)?;
    let mut rows = Vec::new();
    let mut total = cash.after;
    let mut seen = Vec::new();
    for h in &a.holdings {
        let symbol = h.symbol.trim().to_uppercase();
        if symbol.is_empty() {
            continue;
        }
        let sec = secs
            .iter()
            .find(|s| s.symbol.eq_ignore_ascii_case(&symbol) || s.id == h.symbol);
        let kind = sec.map_or(h.kind.unwrap_or(SecurityKind::Stock), |s| s.kind);
        let sa = sec.and_then(|s| sec_account_of(&chart, b.id, &s.symbol));
        seen.extend(sa);
        // investments-options-bonds-crypto.md: written options are negative.
        let signed = shares_on(ctx, sa, h.shares, d, as_of)?;
        let short = kind == SecurityKind::OptionContract && signed < Decimal::ZERO;
        let on_start = if short { signed.abs() } else { signed };
        let worth = |q: Decimal, p: Decimal| -> CommandResult<Decimal> {
            match sec {
                Some(s) => s.worth(q, p),
                None if kind == SecurityKind::Bond && h.maturity.is_some() => {
                    crate::investments::scaled(
                        q,
                        p,
                        h.face_per_unit.unwrap_or(Decimal::ONE_THOUSAND),
                        Decimal::ONE_HUNDRED,
                    )
                }
                None => value_of(kind, q, p),
            }
        };
        let price_start = match sec {
            Some(s) => latest_price(ctx, &s.id, d)?,
            None => None,
        }
        .or_else(|| h.price.map(|p| (d, p)));
        let price_as_of = match sec {
            Some(s) => latest_price(ctx, &s.id, as_of)?,
            None => None,
        }
        .or(price_start);
        let stale = price_as_of.is_some_and(|(pd, _)| pd.days_until(as_of) > STALE_DAYS);
        let value = price_as_of.map(|(_, p)| worth(h.shares, p)).transpose()?;
        if let (Some(v), Some(t)) = (value, total) {
            total = Some(plus(t, v)?);
        } else {
            total = None;
        }
        let market = price_start.map(|(_, p)| worth(on_start, p)).transpose()?;
        let (lots, level, problem) = if on_start <= Decimal::ZERO {
            (
                None,
                None,
                Some(format!(
                    "{symbol}: no shares on the start date (bought after it)"
                )),
            )
        } else {
            match lots_for(h, on_start, d, market, retirement, a.enter_basis) {
                Ok((l, lv)) => (l, lv, None),
                Err(e) => (None, None, Some(e)),
            }
        };
        let basis = lots
            .as_ref()
            .map(|l| {
                l.iter().try_fold(Decimal::ZERO, |acc, x| {
                    plus(acc, x.basis.unwrap_or_default())
                })
            })
            .transpose()?;
        let ex = existing
            .iter()
            .find(|(_, m)| sa.is_some() && m.sec_account == sa)
            .cloned();
        rows.push(HoldingPlan {
            short,
            name: sec.map_or_else(
                || h.name.clone().unwrap_or_else(|| symbol.clone()),
                |s| s.name.clone(),
            ),
            security: sec.map(|s| s.id.clone()),
            symbol,
            kind,
            today: h.shares,
            on_start,
            price_start,
            price_as_of,
            stale,
            value,
            lots,
            level,
            basis,
            problem,
            existing: ex,
        });
    }
    let removed_holdings = existing
        .into_iter()
        .filter(|(_, m)| m.sec_account.is_none_or(|s| !seen.contains(&s)))
        .collect();
    // Prefill: the provider's holdings (SnapTrade), else the book's holdings today.
    let mut feed_rows = Vec::new();
    if let Some(h) = feed.as_ref().and_then(|f| f.holdings.as_ref()) {
        for p in h
            .get("positions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            feed_rows.push(json!({
                "symbol": p.get("security"), "shares": p.get("provider"),
            }));
        }
    }
    Ok(InvPlan {
        cash,
        retirement,
        rows,
        total,
        feed_rows,
        feed_cash,
        removed_holdings,
    })
}

fn inv_plan_json(ctx: &Ctx<'_>, a: &InvArgs, p: &InvPlan) -> CommandResult<Value> {
    let mut v = plan_json(ctx, &p.cash)?;
    let money = |d: Decimal| format_amount(d, Some(p.cash.dp));
    let rows: Vec<Value> = p
        .rows
        .iter()
        .map(|h| {
            json!({
                "symbol": h.symbol, "name": h.name, "security": h.security,
                "known": h.security.is_some(), "kind": h.kind, "short": h.short,
                "today": h.today.normalize().to_string(),
                "on_start": h.on_start.normalize().to_string(),
                "differs": h.today.normalize() != h.on_start.normalize(),
                "price_start": h.price_start.map(|(d, x)| json!({"date": d, "price": crate::investments::price_text(x)})),
                "price": h.price_as_of.map(|(d, x)| json!({"date": d, "price": crate::investments::price_text(x)})),
                "stale": h.stale,
                "value": h.value.map(money),
                "level": h.level,
                "basis": h.basis.map(money),
                "basis_needed": h.lots.as_ref().is_some_and(|l| l.iter().any(|x| x.basis_needed)),
                "various": h.lots.as_ref().is_some_and(|l| l.len() == 1 && l.iter().any(|x| x.acquired_various)),
                "problem": h.problem,
                "existing": h.existing.as_ref().map(|(t, _)| t.id),
            })
        })
        .collect();
    let difference = match (p.total, a.broker_total) {
        (Some(t), Some(b)) => Some(minus(b, t)?),
        _ => None,
    };
    let matches = difference.is_none_or(|x| x.is_zero());
    let mut causes: Vec<Value> = Vec::new();
    if !matches {
        for h in p.rows.iter().filter(|h| h.stale) {
            causes.push(json!({
                "kind": "price", "security": h.security, "symbol": h.symbol,
                "date": h.price_as_of.map(|(d, _)| d),
                "text": format!("A price is out of date: {} was last priced on {}",
                    h.symbol, h.price_as_of.map(|(d, _)| d.to_string()).unwrap_or_default()),
            }));
        }
        if let Some(c) = v.get("causes").and_then(Value::as_array) {
            causes.extend(c.iter().cloned());
        } else {
            causes.push(json!({
                "kind": "as_of",
                "text": "The balance may be for a different date: check the date your broker shows.",
            }));
        }
        if !causes
            .iter()
            .any(|c| c.get("kind") == Some(&json!("as_of")))
        {
            causes.push(json!({
                "kind": "as_of",
                "text": "The total may be for a different date: check the date your broker shows.",
            }));
        }
    }
    if let Some(o) = v.as_object_mut() {
        o.insert("retirement".into(), json!(p.retirement));
        o.insert("basis_step".into(), json!(!p.retirement || a.enter_basis));
        o.insert("holdings".into(), Value::Array(rows));
        o.insert("feed_holdings".into(), Value::Array(p.feed_rows.clone()));
        o.insert("feed_cash".into(), json!(p.feed_cash));
        o.insert(
            "total".into(),
            json!({
                "total": p.total.map(money), "broker": a.broker_total.map(money),
                "difference": difference.map(money), "matches": matches,
                "cash": p.cash.after.map(money),
            }),
        );
        o.insert("inv_causes".into(), Value::Array(causes));
        o.insert(
            "removed_holdings".into(),
            json!(
                p.removed_holdings
                    .iter()
                    .map(|(t, _)| t.id)
                    .collect::<Vec<_>>()
            ),
        );
        o.insert("basis_where".into(), json!(BASIS_WHERE));
        // The opening holdings already posted (a rerun starts from them).
        let chart = Chart::load(ctx)?;
        let mut ex = Vec::new();
        for (t, m) in opening_holdings(ctx, &chart, p.cash.account.id)? {
            let symbol = m
                .sec_account
                .and_then(|s| chart.get(s))
                .map(|a| a.commodity.to_string())
                .unwrap_or_default();
            let lots: Vec<Value> = m
                .lots
                .iter()
                .flatten()
                .map(|l| {
                    json!({"acquired": l.acquired, "shares": l.quantity.normalize().to_string(),
                        "cost": l.basis.map(money), "basis_needed": l.basis_needed,
                        "various": l.acquired_various})
                })
                .collect();
            ex.push(json!({"symbol": symbol, "txn": t.id, "date": t.date,
                "shares": m.quantity.map(|q| q.normalize().to_string()), "lots": lots}));
        }
        o.insert("existing_holdings".into(), Value::Array(ex));
    }
    Ok(v)
}

fn inv_plan_cmd(ctx: &Ctx<'_>, a: InvArgs) -> CommandResult<Value> {
    let p = inv_compute(ctx, &a)?;
    inv_plan_json(ctx, &a, &p)
}

fn inv_apply_cmd(ctx: &Ctx<'_>, a: InvArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let b = crate::investments::brokerage(&chart, Some(&a.account))?.clone();
    if let Some(r) = a.retirement
        && r != b.retirement
    {
        let mut new = b.clone();
        new.retirement = r;
        ctx.save_account(Some(&b), &new)?;
    }
    // Unknown symbols become securities after a confirm (as the price import does).
    let secs = securities(ctx)?;
    let unknown: Vec<&HoldingArg> = a
        .holdings
        .iter()
        .filter(|h| {
            !h.symbol.trim().is_empty()
                && !secs
                    .iter()
                    .any(|s| s.symbol.eq_ignore_ascii_case(h.symbol.trim()) || s.id == h.symbol)
        })
        .collect();
    if !unknown.is_empty() && !a.create_securities {
        return Err(CommandError::Coded {
            code: "unknown_securities",
            message: format!(
                "These securities are not in the book yet: {}. Confirm to create them.",
                unknown
                    .iter()
                    .map(|h| h.symbol.trim().to_uppercase())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }
    for h in unknown {
        let symbol = h.symbol.trim().to_uppercase();
        let kind = h.kind.unwrap_or(SecurityKind::Stock);
        let name = h
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| {
                if kind == SecurityKind::OptionContract {
                    String::new()
                } else {
                    symbol.clone()
                }
            });
        let bond = kind == SecurityKind::Bond && h.maturity.is_some();
        crate::investments::create_security_with(
            ctx,
            crate::investments::SecurityCreateArgs {
                symbol,
                name,
                kind,
                maturity: if bond { h.maturity } else { None },
                coupon_rate: if bond { h.coupon_rate } else { None },
                face_per_unit: if bond { h.face_per_unit } else { None },
                ..Default::default()
            },
        )?;
    }
    let p = inv_compute(ctx, &a)?;
    if let Some(e) = p.rows.iter().find_map(|h| h.problem.clone()) {
        return Err(invalid(e));
    }
    if let Some(h) = p.rows.iter().find(|h| h.lots.is_none()) {
        return Err(invalid(format!(
            "{}: choose how to give its cost basis (exact lots, one total, or I don't know yet)",
            h.symbol
        )));
    }
    let d = p
        .cash
        .date
        .ok_or_else(|| invalid("the start date is not known yet"))?;
    // The cash.
    let cash = if p.cash.opening.is_some_and(|o| o.is_zero()) && p.cash.existing.is_none() {
        None
    } else {
        let posted = post_plan(ctx, &p.cash, a.offset.as_deref(), a.go_ahead)?;
        // A zero cash balance is not "started from zero" for the whole account.
        set_opening_none(ctx, b.id, false)?;
        posted
    };
    let offset = offset_account(ctx, &p.cash, a.offset.as_deref())?;
    // Typed prices on the start date are stored (so valuations use them).
    let mut posted = Vec::new();
    for h in &p.rows {
        let sec = find_security(ctx, &h.symbol)?;
        let given = a
            .holdings
            .iter()
            .find(|x| x.symbol.trim().eq_ignore_ascii_case(&h.symbol))
            .and_then(|x| x.price);
        if let Some(price) = given
            && latest_price(ctx, &sec.id, d)?.is_none()
        {
            crate::investments::store_price(ctx, &sec, d, price)?;
        }
        let args = ActionArgs {
            account: Some(b.id.to_string()),
            security: Some(sec.id.clone()),
            date: Some(d),
            quantity: Some(h.on_start),
            lots: h.lots.clone(),
            short: h.short,
            ..ActionArgs::default()
        };
        let out = post_with(
            ctx,
            Action::SharesIn,
            args,
            h.existing.clone(),
            Some(offset),
            true,
        )?;
        posted.push(
            json!({"symbol": h.symbol, "transaction": out.get("transaction")
            .and_then(|t| t.get("id")).cloned()}),
        );
    }
    for (t, _) in &p.removed_holdings {
        validate_change(ctx, Some(t), None)?;
        crate::txns::remove_with_effects(ctx, t)?;
    }
    if !posted.is_empty() {
        set_opening_none(ctx, b.id, false)?;
    }
    let chart = Chart::load(ctx)?;
    Ok(json!({
        "account": b.id,
        "full_name": chart.full_name(b.id),
        "date": d,
        "cash": cash.map(|t| t.id),
        "holdings": posted,
        "removed_holdings": p.removed_holdings.len(),
        "basis_needed": basis_needed_holdings(ctx, Some(b.id))?.len(),
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PasteArgs {
    /// Lines of "symbol, shares[, cost]" (commas, tabs or semicolons).
    text: String,
}

fn number(s: &str) -> Option<Decimal> {
    let t: String = s
        .trim()
        .chars()
        .filter(|c| !matches!(c, '$' | ',' | ' ' | '\u{a0}'))
        .collect();
    if t.is_empty() {
        return None;
    }
    t.parse().ok()
}

fn inv_paste(ctx: &Ctx<'_>, a: PasteArgs) -> CommandResult<Value> {
    if a.text.len() > 200_000 {
        return Err(invalid("paste at most 200,000 characters"));
    }
    let secs = securities(ctx)?;
    let mut rows = Vec::new();
    let mut skipped = 0usize;
    for line in a.text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cells: Vec<String> = if line.contains('\t') {
            line.split('\t').map(|c| c.trim().to_owned()).collect()
        } else if line.contains(';') {
            line.split(';').map(|c| c.trim().to_owned()).collect()
        } else {
            crate::investments::parse_csv_line(line)
                .into_iter()
                .map(|c| c.trim().to_owned())
                .collect()
        };
        let symbol = cells
            .first()
            .map(|s| mk_core::text::clean(s, 20).text.trim().to_uppercase())
            .unwrap_or_default();
        let shares = cells.get(1).and_then(|s| number(s));
        let (Some(shares), false) = (shares, symbol.is_empty()) else {
            skipped += 1;
            continue;
        };
        let cost = cells.get(2).and_then(|s| number(s));
        let sec = secs.iter().find(|s| s.symbol.eq_ignore_ascii_case(&symbol));
        rows.push(json!({
            "symbol": symbol,
            "shares": shares.normalize().to_string(),
            "cost": cost.map(|c| format_amount(c, Some(2))),
            "known": sec.is_some(),
            "name": sec.map(|s| s.name.clone()),
            "level": cost.map(|_| Level::Total),
        }));
    }
    let unknown: Vec<Value> = rows
        .iter()
        .filter(|r| r.get("known") == Some(&json!(false)))
        .filter_map(|r| r.get("symbol").cloned())
        .collect();
    Ok(json!({"rows": rows, "unknown": unknown, "skipped": skipped}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetBasisArgs {
    /// Brokerage account (id, full name or code).
    account: String,
    /// Security id or symbol.
    security: String,
    basis: BasisArg,
}

fn set_basis(ctx: &Ctx<'_>, a: SetBasisArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let b = crate::investments::brokerage(&chart, Some(&a.account))?.clone();
    let sec = find_security(ctx, &a.security)?;
    let sa = sec_account_of(&chart, b.id, &sec.symbol)
        .ok_or_else(|| CommandError::NotFound(format!("{} in {}", sec.symbol, b.name)))?;
    let (t, m) = opening_holdings(ctx, &chart, b.id)?
        .into_iter()
        .find(|(_, m)| m.sec_account == Some(sa))
        .ok_or_else(|| CommandError::NotFound(format!("an opening holding of {}", sec.symbol)))?;
    if a.basis.level == Level::Unknown {
        return Err(invalid("give the cost basis (exact lots or one total)"));
    }
    let q = m.quantity.unwrap_or_default();
    let h = HoldingArg {
        symbol: sec.symbol.clone(),
        name: None,
        kind: None,
        shares: q,
        basis: Some(a.basis.clone()),
        price: None,
        maturity: None,
        coupon_rate: None,
        face_per_unit: None,
    };
    let (lots, _) = lots_for(&h, q, t.date, None, false, true).map_err(invalid)?;
    let args = ActionArgs {
        account: Some(b.id.to_string()),
        security: Some(sec.id.clone()),
        date: Some(t.date),
        quantity: Some(q),
        lots,
        short: crate::inv_kinds::is_short_in(&m),
        ..ActionArgs::default()
    };
    let out = post_with(ctx, Action::SharesIn, args, Some((t, m)), None, true)?;
    Ok(json!({
        "transaction": out.get("transaction").cloned(),
        "basis_needed": basis_needed_holdings(ctx, Some(b.id))?.len(),
    }))
}
