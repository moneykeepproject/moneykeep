//! Corporate actions and wash sales (investments-advanced.md §2–3): mergers,
//! spin-offs, cash in lieu, symbol changes, wash groups and wash-sale flags. Every
//! corporate action posts ordinary transactions plus metadata; the lot replay in
//! [`crate::investments`] derives the lots and the recognised gains.

use std::collections::HashMap;

use mk_core::{
    AccountId, AccountSubtype, AccountType, CommodityCode, Date, Decimal, Transaction,
    TransactionId, TxnStatus,
};
use mk_ledger::lots;
use mk_store::now_rfc3339;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::accounts::Chart;
use crate::investments::{
    Action, ActionArgs, CorpTerms, INVESTMENTS, InvMeta, Security, SymbolAlias, all_metas,
    base_currency, cash_split, check_name, check_symbol, coded, ensure_sec_account, find_security,
    held_on, invalid, other_account, plus, post, replay, respond, round2, save_meta, save_security,
    securities, split_of, sync,
};
use crate::txns::validate_change;
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    let m = INVESTMENTS;
    r.add_in(
        m,
        "inv.merger",
        "Record a merger: old shares become new shares (tax-free or taxable)",
        true,
        merger,
    );
    r.add_in(
        m,
        "inv.spinoff",
        "Record a spin-off: new shares with part of the parent's basis",
        true,
        spinoff,
    );
    r.add_in(
        m,
        "inv.cash_in_lieu",
        "Record cash received for a fractional share (sold FIFO)",
        true,
        cash_in_lieu,
    );
    r.add_in(
        m,
        "inv.symbol_change",
        "Change a security's symbol from a date (the old symbol stays an alias)",
        true,
        symbol_change,
    );
    r.add_in(
        m,
        "inv.preview_corporate",
        "Preview a merger, spin-off, cash in lieu or symbol change: resulting lots and gains",
        false,
        preview_corporate,
    );
    r.add_in(
        m,
        "inv.wash_check",
        "Before selling at a loss: recent purchases that would make it a wash sale",
        false,
        wash_check,
    );
    r.add_in(
        m,
        "security.link_wash_group",
        "Link substantially identical securities into a wash group",
        true,
        link_wash_group,
    );
    r.add_in(
        m,
        "security.unlink_wash_group",
        "Remove securities from their wash group",
        true,
        unlink_wash_group,
    );
    r.add_in(
        m,
        "security.wash_groups",
        "List wash groups and their securities",
        false,
        wash_groups,
    );
}

fn check_open_period(ctx: &Ctx<'_>, date: Date) -> CommandResult<()> {
    if ctx.tx.closing_date()?.is_some_and(|c| date <= c) {
        return Err(CommandError::Rejected(format!(
            "{date} is in the closed period"
        )));
    }
    Ok(())
}

/// Security accounts holding `symbol` on `date`, with the shares held.
fn holders(
    ctx: &Ctx<'_>,
    chart: &Chart,
    symbol: &str,
    date: Date,
) -> CommandResult<Vec<(AccountId, AccountId, Decimal)>> {
    let accs: Vec<&mk_core::Account> = chart
        .accounts
        .iter()
        .filter(|a| a.subtype == AccountSubtype::Security && a.commodity.as_str() == symbol)
        .collect();
    let held = held_on(ctx, &accs.iter().map(|a| a.id).collect::<Vec<_>>(), date)?;
    let mut out = Vec::new();
    for a in accs {
        let q = held.get(&a.id).copied().unwrap_or_default();
        if q > Decimal::ZERO {
            let broker = a.parent.ok_or_else(|| {
                CommandError::Integrity("security account without a parent".into())
            })?;
            out.push((broker, a.id, q));
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn post_corporate(
    ctx: &Ctx<'_>,
    action: Action,
    broker: AccountId,
    old_acc: AccountId,
    new_acc: AccountId,
    gains: AccountId,
    old: &Security,
    new: &Security,
    date: Date,
    held: Decimal,
    placeholder_new: Decimal,
    ratio: (Option<Decimal>, Option<Decimal>),
    terms: CorpTerms,
    memo: String,
) -> CommandResult<TransactionId> {
    let id = TransactionId::new();
    let mut splits = if action == Action::Merger {
        vec![
            split_of(old_acc, Decimal::ZERO, crate::investments::neg(held)),
            split_of(new_acc, Decimal::ZERO, Decimal::ZERO),
            cash_split(broker, Decimal::ZERO),
            cash_split(gains, Decimal::ZERO),
        ]
    } else {
        vec![
            split_of(old_acc, Decimal::ZERO, Decimal::ZERO),
            split_of(new_acc, Decimal::ZERO, placeholder_new),
        ]
    };
    mk_ledger::sort_splits(&mut splits);
    let base: CommodityCode = base_currency(ctx)?.parse()?;
    let t = Transaction {
        id,
        date,
        currency: base,
        payee: Some(old.symbol.clone()),
        number: None,
        memo: Some(memo),
        status: TxnStatus::Posted,
        void_reason: None,
        reverses: None,
        opening: false,
        entered_at: now_rfc3339()?,
        splits,
    };
    validate_change(ctx, None, Some(&t))?;
    ctx.save_txn(None, &t)?;
    let meta = InvMeta {
        txn: id,
        action,
        account: Some(broker),
        security: Some(old.id.clone()),
        sec_account: Some(old_acc),
        date,
        quantity: Some(held),
        price: None,
        commission: Decimal::ZERO,
        amount: None,
        amount_given: false,
        lots: None,
        method: None,
        kind: None,
        other_account: (action == Action::Merger).then_some(gains),
        ratio_new: ratio.0,
        ratio_old: ratio.1,
        args: ActionArgs {
            account: Some(broker.to_string()),
            security: Some(old.id.clone()),
            date: Some(date),
            ..ActionArgs::default()
        },
        security2: Some(new.id.clone()),
        sec_account2: Some(new_acc),
        corp: Some(terms),
        extra: None,
    };
    save_meta(ctx, &meta)?;
    sync(ctx, old_acc)?;
    sync(ctx, new_acc)?;
    Ok(id)
}

// ----- merger -------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MergerArgs {
    old_security: String,
    new_security: String,
    date: Date,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    ratio_new: Decimal,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    ratio_old: Decimal,
    /// Cash per old share (default 0).
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    cash_per_share: Option<Decimal>,
    /// Value of one new share (required with cash, and for a taxable merger).
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    new_share_value: Option<Decimal>,
    /// `tax_free` or `taxable`.
    mode: String,
    #[serde(default)]
    memo: Option<String>,
}

fn merger(ctx: &Ctx<'_>, a: MergerArgs) -> CommandResult<Value> {
    check_open_period(ctx, a.date)?;
    if !matches!(a.mode.as_str(), "tax_free" | "taxable") {
        return Err(invalid("mode is tax_free or taxable"));
    }
    if a.ratio_new <= Decimal::ZERO || a.ratio_old <= Decimal::ZERO {
        return Err(invalid("the ratio must be positive"));
    }
    let cash = a.cash_per_share.unwrap_or_default();
    if cash.is_sign_negative() {
        return Err(invalid("the cash per share cannot be negative"));
    }
    if (cash > Decimal::ZERO || a.mode == "taxable")
        && !a.new_share_value.is_some_and(|v| v > Decimal::ZERO)
    {
        return Err(invalid(
            "give new_share_value (the value of one new share) for cash or a taxable merger",
        ));
    }
    let old = find_security(ctx, &a.old_security)?;
    let new = find_security(ctx, &a.new_security)?;
    if old.id == new.id {
        return Err(invalid("the old and new securities must differ"));
    }
    let mut chart = Chart::load(ctx)?;
    let hs = holders(ctx, &chart, &old.symbol, a.date)?;
    if hs.is_empty() {
        return Err(coded(
            "insufficient_shares",
            format!("no account holds {} on {}", old.symbol, a.date),
        ));
    }
    let gains = other_account(ctx, &mut chart, None, "Realized Gains", AccountType::Income)?;
    let terms = CorpTerms {
        mode: Some(a.mode.clone()),
        cash_per_share: Some(cash),
        new_share_value: a.new_share_value,
        new_per_parent: None,
        allocation_percent: None,
    };
    let memo = a
        .memo
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| {
            format!(
                "Merger {} into {} ({} for {})",
                old.symbol,
                new.symbol,
                mk_core::currency::format_amount(a.ratio_new, None),
                mk_core::currency::format_amount(a.ratio_old, None)
            )
        });
    let mut out = Vec::new();
    for (broker, old_acc, held) in hs {
        let new_acc = ensure_sec_account(ctx, &mut chart, broker, &new.symbol)?;
        let id = post_corporate(
            ctx,
            Action::Merger,
            broker,
            old_acc,
            new_acc,
            gains,
            &old,
            &new,
            a.date,
            held,
            Decimal::ZERO,
            (Some(a.ratio_new), Some(a.ratio_old)),
            terms.clone(),
            memo.clone(),
        )?;
        out.push(respond(ctx, id)?);
    }
    Ok(json!({"transactions": out}))
}

// ----- spin-off -----------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CashInLieuPart {
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    quantity: Decimal,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
    /// The brokerage account receiving it (needed when several accounts hold the parent).
    #[serde(default)]
    account: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SpinoffArgs {
    parent: String,
    new_security: String,
    date: Date,
    /// New shares per parent share.
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    new_per_parent: Decimal,
    /// Basis percentage moving to the new security (issuer's Form 8937).
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    allocation_percent: Decimal,
    #[serde(default)]
    cash_in_lieu: Option<CashInLieuPart>,
    #[serde(default)]
    memo: Option<String>,
}

fn spinoff(ctx: &Ctx<'_>, a: SpinoffArgs) -> CommandResult<Value> {
    check_open_period(ctx, a.date)?;
    if a.allocation_percent <= Decimal::ZERO || a.allocation_percent >= Decimal::ONE_HUNDRED {
        return Err(invalid(
            "the allocation percentage must be more than 0 and less than 100",
        ));
    }
    if a.new_per_parent <= Decimal::ZERO {
        return Err(invalid("new_per_parent must be positive"));
    }
    let parent = find_security(ctx, &a.parent)?;
    let new = find_security(ctx, &a.new_security)?;
    if parent.id == new.id {
        return Err(invalid("the parent and the new security must differ"));
    }
    let mut chart = Chart::load(ctx)?;
    let hs = holders(ctx, &chart, &parent.symbol, a.date)?;
    if hs.is_empty() {
        return Err(coded(
            "insufficient_shares",
            format!("no account holds {} on {}", parent.symbol, a.date),
        ));
    }
    let gains = other_account(ctx, &mut chart, None, "Realized Gains", AccountType::Income)?;
    let terms = CorpTerms {
        mode: None,
        cash_per_share: None,
        new_share_value: None,
        new_per_parent: Some(a.new_per_parent),
        allocation_percent: Some(a.allocation_percent),
    };
    let memo = a
        .memo
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| {
            format!(
                "Spin-off of {} from {} ({} per share, {}% of basis)",
                new.symbol,
                parent.symbol,
                mk_core::currency::format_amount(a.new_per_parent, None),
                mk_core::currency::format_amount(a.allocation_percent, None)
            )
        });
    let mut out = Vec::new();
    let mut brokers = Vec::new();
    for (broker, old_acc, held) in hs {
        let new_acc = ensure_sec_account(ctx, &mut chart, broker, &new.symbol)?;
        let estimate = lots::round(
            crate::investments::mul(held, a.new_per_parent)?,
            lots::QUANTITY_DP,
        );
        let id = post_corporate(
            ctx,
            Action::Spinoff,
            broker,
            old_acc,
            new_acc,
            gains,
            &parent,
            &new,
            a.date,
            held,
            estimate,
            (None, None),
            terms.clone(),
            memo.clone(),
        )?;
        brokers.push(broker);
        out.push(respond(ctx, id)?);
    }
    if let Some(c) = a.cash_in_lieu {
        let account = match (&c.account, brokers.as_slice()) {
            (Some(acc), _) => acc.clone(),
            (None, [one]) => one.to_string(),
            (None, _) => {
                return Err(invalid(
                    "several accounts hold the parent: give cash_in_lieu.account",
                ));
            }
        };
        out.push(post(
            ctx,
            Action::CashInLieu,
            ActionArgs {
                account: Some(account),
                security: Some(new.id.clone()),
                date: Some(a.date),
                quantity: Some(c.quantity),
                amount: Some(c.amount),
                ..ActionArgs::default()
            },
            None,
        )?);
    }
    Ok(json!({"transactions": out}))
}

// ----- cash in lieu -------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CashInLieuArgs {
    security: String,
    date: Date,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    quantity: Decimal,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
    /// The brokerage account (needed when several accounts hold the security).
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    memo: Option<String>,
}

fn cash_in_lieu(ctx: &Ctx<'_>, a: CashInLieuArgs) -> CommandResult<Value> {
    let sec = find_security(ctx, &a.security)?;
    let account = match a.account {
        Some(acc) => acc,
        None => {
            let chart = Chart::load(ctx)?;
            let hs = holders(ctx, &chart, &sec.symbol, a.date)?;
            match hs.as_slice() {
                [(broker, _, _)] => broker.to_string(),
                [] => {
                    return Err(coded(
                        "insufficient_shares",
                        format!("no account holds {} on {}", sec.symbol, a.date),
                    ));
                }
                _ => return Err(invalid("several accounts hold the security: give account")),
            }
        }
    };
    post(
        ctx,
        Action::CashInLieu,
        ActionArgs {
            account: Some(account),
            security: Some(sec.id),
            date: Some(a.date),
            quantity: Some(a.quantity),
            amount: Some(a.amount),
            memo: a.memo,
            ..ActionArgs::default()
        },
        None,
    )
}

// ----- symbol change ------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SymbolChangeArgs {
    security: String,
    date: Date,
    new_symbol: String,
    #[serde(default)]
    new_name: Option<String>,
}

fn symbol_change(ctx: &Ctx<'_>, a: SymbolChangeArgs) -> CommandResult<Value> {
    check_open_period(ctx, a.date)?;
    let mut s = find_security(ctx, &a.security)?;
    let old_symbol = s.symbol.clone();
    let new_symbol = check_symbol(ctx, &a.new_symbol, Some(&s.id))?;
    if new_symbol.eq_ignore_ascii_case(&old_symbol) {
        return Err(invalid("the new symbol is the same as the current one"));
    }
    s.aliases.push(SymbolAlias {
        symbol: old_symbol.clone(),
        name: s.name.clone(),
        until: a.date,
    });
    s.aliases.sort_by_key(|x| x.until);
    s.symbol = new_symbol.clone();
    if let Some(n) = a.new_name.filter(|n| !n.trim().is_empty()) {
        s.name = check_name(&n)?;
    }
    save_security(ctx, &s)?;
    // Security accounts follow the symbol (their commodity is the symbol).
    let chart = Chart::load(ctx)?;
    let commodity: CommodityCode = new_symbol.parse()?;
    for acc in chart
        .accounts
        .iter()
        .filter(|x| x.subtype == AccountSubtype::Security && x.commodity.as_str() == old_symbol)
    {
        let mut n = acc.clone();
        n.commodity = commodity.clone();
        if n.name.eq_ignore_ascii_case(&old_symbol) {
            let clash = acc.parent.is_some_and(|p| {
                chart
                    .children(p)
                    .any(|c| c.id != acc.id && c.name.eq_ignore_ascii_case(&new_symbol))
            });
            n.name = if clash {
                format!("{new_symbol} shares")
            } else {
                new_symbol.clone()
            };
        }
        ctx.save_account(Some(acc), &n)?;
    }
    Ok(s.json())
}

// ----- preview ------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct PreviewArgs {
    /// `merger`, `spinoff`, `cash_in_lieu` or `symbol_change`.
    action: String,
    /// The action's own arguments.
    #[serde(flatten)]
    rest: Map<String, Value>,
}

/// Run the action in a read transaction (always rolled back) and list the resulting
/// open lots of every security account it touched, with the recognised gain.
fn preview_corporate(ctx: &Ctx<'_>, a: PreviewArgs) -> CommandResult<Value> {
    let args = Value::Object(a.rest);
    let out = match a.action.as_str() {
        "merger" => merger(ctx, crate::parse_args(args)?)?,
        "spinoff" => spinoff(ctx, crate::parse_args(args)?)?,
        "cash_in_lieu" => json!({"transactions": [cash_in_lieu(ctx, crate::parse_args(args)?)?]}),
        "symbol_change" => {
            return Ok(json!({"security": symbol_change(ctx, crate::parse_args(args)?)?}));
        }
        other => return Err(invalid(format!("unknown corporate action {other:?}"))),
    };
    let chart = Chart::load(ctx)?;
    let metas = all_metas(ctx)?;
    let mut accounts: Vec<AccountId> = Vec::new();
    let mut gain = Decimal::ZERO;
    for t in out
        .get("transactions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = t
            .pointer("/transaction/id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<TransactionId>().ok())
        else {
            continue;
        };
        if let Some(m) = metas.get(&id) {
            accounts.extend(m.sec_account);
            accounts.extend(m.sec_account2);
            if let Some(g) = m.other_account
                && let Some(tx) = ctx.tx.transaction(id)?
            {
                for s in tx.splits.iter().filter(|s| s.account == g) {
                    gain = plus(gain, crate::investments::neg(s.value))?;
                }
            }
        }
    }
    accounts.sort();
    accounts.dedup();
    let mut list = Vec::new();
    for acc in accounts {
        let r = replay(ctx, acc, None)?;
        let lots: Vec<Value> = r
            .state
            .open()
            .iter()
            .map(|l| {
                json!({
                    "acquired": l.acquired,
                    "quantity": mk_core::currency::format_amount(l.remaining, None),
                    "basis": mk_core::currency::format_amount(l.basis, Some(2)),
                })
            })
            .collect();
        list.push(json!({
            "account": acc,
            "full_name": chart.full_name(acc),
            "symbol": chart.get(acc).map(|x| x.commodity.to_string()),
            "lots": lots,
        }));
    }
    Ok(json!({
        "accounts": list,
        "gain": mk_core::currency::format_amount(gain, Some(2)),
        "transactions": out.get("transactions").cloned().unwrap_or(Value::Null),
    }))
}

// ----- wash groups --------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LinkArgs {
    /// Security ids or symbols.
    securities: Vec<String>,
    name: String,
}

fn link_wash_group(ctx: &Ctx<'_>, a: LinkArgs) -> CommandResult<Value> {
    let name = a.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(invalid("a wash group name is 1-100 characters"));
    }
    if a.securities.is_empty() {
        return Err(invalid("give the securities to link"));
    }
    for r in &a.securities {
        let mut s = find_security(ctx, r)?;
        s.wash_group = Some(name.clone());
        save_security(ctx, &s)?;
    }
    wash_groups(ctx, crate::builtin::NoArgs {})
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UnlinkArgs {
    securities: Vec<String>,
}

fn unlink_wash_group(ctx: &Ctx<'_>, a: UnlinkArgs) -> CommandResult<Value> {
    for r in &a.securities {
        let mut s = find_security(ctx, r)?;
        s.wash_group = None;
        save_security(ctx, &s)?;
    }
    wash_groups(ctx, crate::builtin::NoArgs {})
}

fn wash_groups(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let mut groups: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
    for s in securities(ctx)? {
        if let Some(g) = &s.wash_group {
            groups
                .entry(g.clone())
                .or_default()
                .push(json!({"id": s.id, "symbol": s.symbol, "name": s.name}));
        }
    }
    Ok(Value::Array(
        groups
            .into_iter()
            .map(|(name, members)| json!({"name": name, "members": members}))
            .collect(),
    ))
}

// ----- wash sales ---------------------------------------------------------------------

/// The wash-sale finding for one realised loss portion.
#[derive(Debug, Clone, Default)]
pub(crate) struct Wash {
    pub disallowed: Decimal,
    pub matched: Decimal,
    pub replacements: Vec<Value>,
    pub ira: bool,
}

#[derive(Debug, Clone)]
struct Acquisition {
    key: String,
    date: Date,
    order: (String, String),
    lot: String,
    quantity: Decimal,
    symbol: String,
    account: String,
    ira: bool,
}

fn group_key(s: &Security) -> String {
    match &s.wash_group {
        Some(g) => format!("group:{g}"),
        None => format!("security:{}", s.id),
    }
}

fn is_ira(chart: &Chart, acc: Option<AccountId>) -> bool {
    let mut cur = acc;
    while let Some(id) = cur {
        let Some(a) = chart.get(id) else { break };
        if a.name
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| w == "IRA")
        {
            return true;
        }
        cur = a.parent;
    }
    false
}

/// Every purchase (buy, reinvestment, shares in) of every security, by wash key.
fn acquisitions(
    ctx: &Ctx<'_>,
    chart: &Chart,
    secs: &[Security],
) -> CommandResult<Vec<Acquisition>> {
    let mut out = Vec::new();
    for m in all_metas(ctx)?.into_values() {
        if !matches!(m.action, Action::Buy | Action::Reinvest | Action::SharesIn) {
            continue;
        }
        let Some(sec) = m
            .security
            .as_ref()
            .and_then(|id| secs.iter().find(|s| s.id == *id))
        else {
            continue;
        };
        // investments-options-bonds-crypto.md: wash sales leave options and crypto out.
        if sec.kind.outside_wash_sales() {
            continue;
        }
        let Some(t) = ctx.tx.transaction(m.txn)? else {
            continue;
        };
        if t.status != TxnStatus::Posted {
            continue;
        }
        let account = m
            .sec_account
            .map(|a| chart.full_name(a))
            .unwrap_or_default();
        let ira = is_ira(chart, m.sec_account);
        let order = (t.entered_at.clone(), t.id.to_string());
        if m.action == Action::SharesIn {
            for (i, l) in m.lots.clone().unwrap_or_default().iter().enumerate() {
                out.push(Acquisition {
                    key: group_key(sec),
                    date: t.date,
                    order: order.clone(),
                    lot: format!("{}:{}", t.id, i + 1),
                    quantity: l.quantity,
                    symbol: sec.symbol.clone(),
                    account: account.clone(),
                    ira,
                });
            }
        } else {
            out.push(Acquisition {
                key: group_key(sec),
                date: t.date,
                order,
                lot: t.id.to_string(),
                quantity: m.quantity.unwrap_or_default(),
                symbol: sec.symbol.clone(),
                account,
                ira,
            });
        }
    }
    out.sort_by(|a, b| (a.date, &a.order, &a.lot).cmp(&(b.date, &b.order, &b.lot)));
    Ok(out)
}

/// Wash-sale flags for every realised loss portion, keyed by (sale, lot):
/// replacements in the same wash group in any account within 30 days before or after
/// the sale, earliest sales first and earliest replacements first, excluding the
/// shares sold and replacements already used.
pub(crate) fn wash_sales(ctx: &Ctx<'_>) -> CommandResult<HashMap<(TransactionId, String), Wash>> {
    let chart = Chart::load(ctx)?;
    let secs = securities(ctx)?;
    let acqs = acquisitions(ctx, &chart, &secs)?;
    // Loss portions: (date, order, key, sale, lot, quantity, loss, sold lots).
    struct Loss {
        date: Date,
        order: (String, String),
        key: String,
        txn: TransactionId,
        lot: String,
        quantity: Decimal,
        loss: Decimal,
        sold: HashMap<String, Decimal>,
    }
    let mut losses: Vec<Loss> = Vec::new();
    for acc in chart
        .accounts
        .iter()
        .filter(|a| a.subtype == AccountSubtype::Security)
    {
        let Some(sec) = secs.iter().find(|s| s.symbol == acc.commodity.as_str()) else {
            continue;
        };
        if sec.kind.outside_wash_sales() {
            continue;
        }
        let r = replay(ctx, acc.id, None)?;
        for f in &r.disposals {
            if !matches!(f.action, Action::Sell | Action::CashInLieu | Action::Merger) {
                continue;
            }
            let Some(t) = ctx.tx.transaction(f.txn)? else {
                continue;
            };
            let mut sold: HashMap<String, Decimal> = HashMap::new();
            for p in &f.disposal.portions {
                let e = sold.entry(p.lot.clone()).or_default();
                *e = e.checked_add(p.quantity).unwrap_or(*e);
            }
            for p in &f.disposal.portions {
                let g = p.gain();
                if g < Decimal::ZERO && p.quantity > Decimal::ZERO {
                    losses.push(Loss {
                        date: f.date,
                        order: (t.entered_at.clone(), t.id.to_string()),
                        key: group_key(sec),
                        txn: f.txn,
                        lot: p.lot.clone(),
                        quantity: p.quantity,
                        loss: g.abs(),
                        sold: sold.clone(),
                    });
                }
            }
        }
    }
    losses.sort_by(|a, b| (a.date, &a.order).cmp(&(b.date, &b.order)));
    let mut used: HashMap<String, Decimal> = HashMap::new();
    let mut out = HashMap::new();
    for l in losses {
        let lo = l.date.add_days(-30).unwrap_or(l.date);
        let hi = l.date.add_days(30).unwrap_or(l.date);
        let mut left = l.quantity;
        let mut w = Wash::default();
        for a in acqs
            .iter()
            .filter(|a| a.key == l.key && a.date >= lo && a.date <= hi)
        {
            if left.is_zero() {
                break;
            }
            let taken = used.get(&a.lot).copied().unwrap_or_default();
            let sold_here = l.sold.get(&a.lot).copied().unwrap_or_default();
            let avail = a
                .quantity
                .checked_sub(taken)
                .and_then(|x| x.checked_sub(sold_here))
                .unwrap_or_default();
            if avail <= Decimal::ZERO {
                continue;
            }
            let take = avail.min(left);
            used.insert(a.lot.clone(), plus(taken, take)?);
            left = left.checked_sub(take).unwrap_or_default();
            w.ira |= a.ira;
            w.replacements.push(json!({
                "date": a.date,
                "symbol": a.symbol,
                "account": a.account,
                "quantity": mk_core::currency::format_amount(take, None),
                "lot": a.lot,
            }));
        }
        w.matched = l.quantity.checked_sub(left).unwrap_or_default();
        if w.matched > Decimal::ZERO {
            w.disallowed = round2(
                l.loss
                    .checked_mul(w.matched)
                    .and_then(|x| x.checked_div(l.quantity))
                    .unwrap_or_default(),
            );
            out.insert((l.txn, l.lot), w);
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WashCheckArgs {
    security: String,
    date: Date,
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    quantity: Option<Decimal>,
}

fn wash_check(ctx: &Ctx<'_>, a: WashCheckArgs) -> CommandResult<Value> {
    let _ = a.quantity;
    let sec = find_security(ctx, &a.security)?;
    if sec.kind.outside_wash_sales() {
        return Ok(
            json!({"security": sec.id, "symbol": sec.symbol, "recent": [],
            "applies": false,
            "warning": "Wash-sale checks do not cover options and crypto."}),
        );
    }
    let chart = Chart::load(ctx)?;
    let secs = securities(ctx)?;
    let key = group_key(&sec);
    let lo = a.date.add_days(-30).unwrap_or(a.date);
    let recent: Vec<Value> = acquisitions(ctx, &chart, &secs)?
        .into_iter()
        .filter(|x| x.key == key && x.date >= lo && x.date <= a.date)
        .map(|x| {
            json!({
                "date": x.date,
                "symbol": x.symbol,
                "account": x.account,
                "quantity": mk_core::currency::format_amount(x.quantity, None),
            })
        })
        .collect();
    let until = a.date.add_days(30).unwrap_or(a.date);
    let mut warning = format!(
        "If you sell {} at a loss on {}, buying it (or a security in its wash group) again on or before {until} would make the sale a wash sale.",
        sec.symbol, a.date
    );
    if !recent.is_empty() {
        warning.push_str(" Purchases in the last 30 days may already make it one.");
    }
    Ok(
        json!({"security": sec.id, "symbol": sec.symbol, "recent": recent, "applies": true,
        "warning": warning}),
    )
}

/// Run an imported corporate action with the terms entered in review.
pub(crate) fn imported(
    ctx: &Ctx<'_>,
    action: mk_import::InvAction,
    date: Date,
    security: &str,
    terms: Value,
) -> CommandResult<Option<TransactionId>> {
    let mut args = match terms {
        Value::Object(m) => m,
        _ => return Err(invalid("the terms must be an object")),
    };
    args.insert("date".into(), json!(date));
    let first_txn = |v: &Value| {
        v.pointer("/transactions/0/transaction/id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok())
    };
    match action {
        mk_import::InvAction::Merger => {
            args.insert("old_security".into(), json!(security));
            Ok(first_txn(&merger(
                ctx,
                crate::parse_args(Value::Object(args))?,
            )?))
        }
        mk_import::InvAction::Spinoff => {
            args.insert("parent".into(), json!(security));
            Ok(first_txn(&spinoff(
                ctx,
                crate::parse_args(Value::Object(args))?,
            )?))
        }
        mk_import::InvAction::SymbolChange => {
            args.insert("security".into(), json!(security));
            symbol_change(ctx, crate::parse_args(Value::Object(args))?)?;
            Ok(None)
        }
        _ => Err(invalid("not a corporate action")),
    }
}
