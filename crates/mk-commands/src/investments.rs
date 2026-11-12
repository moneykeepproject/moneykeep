//! Investments (spec: investments-core.md): securities, prices, investment actions and
//! the lot replay. Every command needs the `investments` module.
//!
//! Each action posts one ordinary transaction plus an `inv_txn` document with its
//! metadata. Lots are derived: [`replay`] recomputes a security account's open lots
//! and every disposal from its investment transactions in register order, and
//! [`sync`] rewrites later sales whose basis changed (in the same command).

use std::collections::{BTreeMap, HashMap, HashSet};

use mk_core::currency::{format_amount, is_iso_currency};
use mk_core::{
    Account, AccountId, AccountSubtype, AccountType, CommodityCode, Date, Decimal, Reconcile,
    Split, Transaction, TransactionId, TxnStatus,
};
use mk_ledger::canonical_amount;
use mk_ledger::lots::{self, Disposal, Event, LotError, LotState, Method, Outcome, Selection};
use mk_store::{Doc, DocTable, now_rfc3339, today_utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::accounts::Chart;
use crate::txns::{txn_json, validate_change};
use crate::{CommandError, CommandResult, Ctx, Registry};

pub const INVESTMENTS: &str = "investments";

pub(crate) fn register(r: &mut Registry) {
    let m = INVESTMENTS;
    r.add_in(
        m,
        "security.create",
        "Create a security",
        true,
        security_create,
    );
    r.add_in(
        m,
        "security.update",
        "Change a security's name, kind, basis method or CUSIP",
        true,
        security_update,
    );
    r.add_in(m, "security.get", "Get one security", false, security_get);
    r.add_in(
        m,
        "security.list",
        "List securities with their latest price and shares held",
        false,
        security_list,
    );
    r.add_in(
        m,
        "security.delete",
        "Delete a security that was never used",
        true,
        security_delete,
    );
    r.add_in(
        m,
        "price.set",
        "Store a security's closing price for a day",
        true,
        price_set,
    );
    r.add_in(
        m,
        "price.import",
        "Import prices from CSV (date, symbol, price columns)",
        true,
        price_import,
    );
    r.add_in(
        m,
        "price.list",
        "List a security's prices",
        false,
        price_list,
    );
    r.add_in(
        m,
        "price.latest",
        "The latest price on or before a date",
        false,
        price_latest,
    );
    r.add_in(m, "inv.buy", "Buy shares (creates a lot)", true, buy);
    r.add_in(m, "inv.sell", "Sell shares", true, sell);
    r.add_in(m, "inv.dividend", "Record a cash dividend", true, dividend);
    r.add_in(
        m,
        "inv.reinvest",
        "Record a reinvested dividend (creates a lot)",
        true,
        reinvest,
    );
    r.add_in(m, "inv.interest", "Record interest earned", true, interest);
    r.add_in(m, "inv.fee", "Record an investment fee", true, fee);
    r.add_in(
        m,
        "inv.shares_in",
        "Move shares into an account with their original lots",
        true,
        shares_in,
    );
    r.add_in(
        m,
        "inv.shares_out",
        "Move shares out of an account (not a sale)",
        true,
        shares_out,
    );
    r.add_in(m, "inv.split", "Record a stock split", true, split);
    r.add_in(
        m,
        "inv.return_of_capital",
        "Record a return of capital (reduces basis)",
        true,
        return_of_capital,
    );
    r.add_in(
        m,
        "inv.update",
        "Replace an investment action with new arguments",
        true,
        update,
    );
    r.add_in(
        m,
        "inv.get",
        "An investment transaction's action and arguments",
        false,
        inv_get,
    );
    r.add_in(
        m,
        "inv.preview_sell",
        "The lots a sale would consume and its gains, without posting",
        false,
        preview_sell,
    );
    r.add_in(
        m,
        "inv.preview",
        "What an action (or an edit of one) would post, without posting",
        false,
        preview,
    );
    r.add_in(
        m,
        "inv.activity",
        "A brokerage account's transactions with their actions and running cash balance",
        false,
        activity,
    );
    crate::inv_kinds::register(r);
    crate::inv_reports::register(r);
}

// ----- errors ----------------------------------------------------------------------

pub(crate) fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

pub(crate) fn coded(code: &'static str, message: impl Into<String>) -> CommandError {
    CommandError::Coded {
        code,
        message: message.into(),
    }
}

pub(crate) fn lot_error(e: LotError, date: Date) -> CommandError {
    match e {
        LotError::Insufficient { held } => coded(
            "insufficient_shares",
            format!(
                "not enough shares on {date}: {} held",
                format_amount(held, None)
            ),
        ),
        LotError::BadSelection(m) => invalid(m),
        LotError::ExceedsBasis { open } => CommandError::Rejected(format!(
            "the return of capital on {date} exceeds the open basis {}",
            format_amount(open, Some(2))
        )),
        LotError::Invalid(m) => invalid(m),
        LotError::Overflow => CommandError::Rejected("arithmetic overflow".into()),
    }
}

fn bad_doc(e: impl std::fmt::Display) -> CommandError {
    CommandError::Integrity(format!("bad stored investment record: {e}"))
}

// ----- securities ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SecurityKind {
    #[default]
    Stock,
    Etf,
    MutualFund,
    Bond,
    MoneyMarket,
    /// Certificate of deposit: quantity is the face amount, price per 100 of face.
    Cd,
    Other,
    /// An option contract (investments-options-bonds-crypto.md): quantity in
    /// contracts, price per share of the underlying.
    #[serde(rename = "option")]
    OptionContract,
    /// A digital asset: quantity in coins (up to 8 decimals), price per coin.
    Crypto,
}

impl SecurityKind {
    /// Bonds and CDs are quoted per 100 of face value (import-investments.md rev. 2).
    pub(crate) fn per_hundred(self) -> bool {
        matches!(self, SecurityKind::Bond | SecurityKind::Cd)
    }

    /// The kinds wash-sale flags leave out (investments-options-bonds-crypto.md).
    pub(crate) fn outside_wash_sales(self) -> bool {
        matches!(self, SecurityKind::OptionContract | SecurityKind::Crypto)
    }
}

/// Market value of `q` units at `price` for a kind without its own terms:
/// round(q × price, 2), ÷ 100 for kinds quoted per 100 of face, × 100 (the default
/// multiplier) for options. [`Security::worth`] uses the security's own terms.
pub(crate) fn value_of(kind: SecurityKind, q: Decimal, price: Decimal) -> CommandResult<Decimal> {
    let (m, d) = match kind {
        SecurityKind::OptionContract => (Decimal::ONE_HUNDRED, Decimal::ONE),
        k if k.per_hundred() => (Decimal::ONE, Decimal::ONE_HUNDRED),
        _ => (Decimal::ONE, Decimal::ONE),
    };
    scaled(q, price, m, d)
}

/// round(q × price × m ÷ d, 2).
pub(crate) fn scaled(q: Decimal, price: Decimal, m: Decimal, d: Decimal) -> CommandResult<Decimal> {
    let gross = mul(mul(q, price)?, m)?;
    let gross = gross
        .checked_div(d)
        .ok_or_else(|| CommandError::Rejected("arithmetic overflow".into()))?;
    Ok(round2(gross))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BasisMethod {
    Fifo,
    Average,
    Specific,
}

impl BasisMethod {
    pub(crate) fn engine(self) -> Method {
        match self {
            BasisMethod::Fifo => Method::Fifo,
            BasisMethod::Average => Method::Average,
            BasisMethod::Specific => Method::Specific,
        }
    }
}

/// One basis-method setting: in effect for sales dated after `since` (the first
/// entry has no date and applies from the start).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct MethodChange {
    since: Option<Date>,
    method: BasisMethod,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Security {
    pub id: String,
    pub symbol: String,
    pub name: String,
    pub kind: SecurityKind,
    pub currency: String,
    pub basis_method: BasisMethod,
    methods: Vec<MethodChange>,
    #[serde(default)]
    pub cusip: Option<String>,
    /// FIGI (from SnapTrade), matched when importing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub figi: Option<String>,
    pub created_at: String,
    /// Earlier symbols (investments-advanced.md symbol change), oldest first.
    #[serde(default)]
    pub aliases: Vec<SymbolAlias>,
    /// Wash group linking substantially identical securities.
    #[serde(default)]
    pub wash_group: Option<String>,
    /// Kind `option`: the contract's terms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<crate::inv_kinds::OptionTerms>,
    /// Kind `bond` with face, coupon and maturity (bonds without terms keep the
    /// import rule: quantity is the face amount, price per 100 of face).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bond: Option<crate::inv_kinds::BondTerms>,
}

/// A symbol the security had until a change date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SymbolAlias {
    pub symbol: String,
    pub name: String,
    /// The change date: the alias was the symbol before this date.
    pub until: Date,
}

impl Security {
    /// The symbol shown for an event dated `date`: "ABC (now XYZ)" before a change.
    pub(crate) fn symbol_on(&self, date: Date) -> String {
        match self.aliases.iter().find(|a| date < a.until) {
            Some(a) => format!("{} (now {})", a.symbol, self.symbol),
            None => self.symbol.clone(),
        }
    }

    /// Whether `symbol` is the security's symbol or one of its earlier ones.
    pub(crate) fn answers_to(&self, symbol: &str) -> bool {
        self.symbol.eq_ignore_ascii_case(symbol)
            || self
                .aliases
                .iter()
                .any(|a| a.symbol.eq_ignore_ascii_case(symbol))
    }
}

impl Security {
    /// (multiplier, divisor) from quantity × price to money: options × their
    /// multiplier, bonds with terms × face per unit ÷ 100, CDs and bonds without terms
    /// ÷ 100.
    pub(crate) fn factor(&self) -> (Decimal, Decimal) {
        if let Some(o) = &self.option {
            return (o.multiplier, Decimal::ONE);
        }
        if let Some(b) = &self.bond {
            return (b.face_per_unit, Decimal::ONE_HUNDRED);
        }
        match self.kind {
            SecurityKind::OptionContract => (Decimal::ONE_HUNDRED, Decimal::ONE),
            k if k.per_hundred() => (Decimal::ONE, Decimal::ONE_HUNDRED),
            _ => (Decimal::ONE, Decimal::ONE),
        }
    }

    /// Market value of `q` units at `price` (rounded to cents).
    pub(crate) fn worth(&self, q: Decimal, price: Decimal) -> CommandResult<Decimal> {
        let (m, d) = self.factor();
        scaled(q, price, m, d)
    }

    /// The method for a sale dated `date`.
    pub(crate) fn method_on(&self, date: Date) -> BasisMethod {
        let mut m = self.basis_method;
        if let Some(first) = self.methods.first() {
            m = first.method;
        }
        for c in &self.methods {
            match c.since {
                None => m = c.method,
                Some(s) if date > s => m = c.method,
                Some(_) => {}
            }
        }
        m
    }

    pub(crate) fn json(&self) -> Value {
        json!({
            "id": self.id,
            "symbol": self.symbol,
            "name": self.name,
            "kind": self.kind,
            "currency": self.currency,
            "basis_method": self.basis_method,
            "cusip": self.cusip,
            "figi": self.figi,
            "aliases": self.aliases.iter().map(|a| json!({"symbol": a.symbol, "name": a.name, "until": a.until})).collect::<Vec<_>>(),
            "wash_group": self.wash_group,
            "option": self.option.as_ref().map(crate::inv_kinds::option_json),
            "bond": self.bond.as_ref().map(crate::inv_kinds::bond_json),
        })
    }
}

pub(crate) fn securities(ctx: &Ctx<'_>) -> CommandResult<Vec<Security>> {
    let mut out = Vec::new();
    for d in ctx.tx.docs(DocTable::Security, None)? {
        out.push(serde_json::from_value::<Security>(d.json).map_err(bad_doc)?);
    }
    out.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    Ok(out)
}

/// A security by id or symbol (case-insensitive).
pub(crate) fn find_security(ctx: &Ctx<'_>, r: &str) -> CommandResult<Security> {
    let r = r.trim();
    let all = securities(ctx)?;
    let pos = all
        .iter()
        .position(|s| s.id == r || s.symbol.eq_ignore_ascii_case(r))
        .or_else(|| all.iter().position(|s| s.answers_to(r)));
    pos.and_then(|i| all.into_iter().nth(i))
        .ok_or_else(|| CommandError::NotFound(format!("security {r:?}")))
}

pub(crate) fn save_security(ctx: &Ctx<'_>, s: &Security) -> CommandResult<()> {
    ctx.save_doc(
        DocTable::Security,
        &s.id,
        Some(&Doc {
            id: s.id.clone(),
            grp: None,
            ord: 0,
            json: serde_json::to_value(s)?,
        }),
    )
}

pub(crate) fn check_symbol(
    ctx: &Ctx<'_>,
    raw: &str,
    except: Option<&str>,
) -> CommandResult<String> {
    check_symbol_for(ctx, raw, except, SecurityKind::Stock)
}

/// The symbol rule of a kind: options use the generated form (spaces allowed, up to
/// 24 characters), crypto letters and digits up to 12, the others core's rule.
pub(crate) fn check_symbol_for(
    ctx: &Ctx<'_>,
    raw: &str,
    except: Option<&str>,
    kind: SecurityKind,
) -> CommandResult<String> {
    let symbol = raw.trim().to_ascii_uppercase();
    match kind {
        SecurityKind::OptionContract => {
            if !CommodityCode::is_option_form(&symbol) {
                return Err(invalid(format!(
                    "option symbol {raw:?} must look like \"AAPL 261218 C200\" (underlying, YYMMDD, C or P and the strike)"
                )));
            }
        }
        SecurityKind::Crypto => {
            let ok = (1..=12).contains(&symbol.chars().count())
                && symbol.chars().all(|c| c.is_ascii_alphanumeric());
            if !ok {
                return Err(invalid(format!(
                    "crypto symbol {raw:?} must be 1-12 letters or digits"
                )));
            }
        }
        _ => {
            let ok_len = (1..=12).contains(&symbol.chars().count());
            let ok_chars = symbol
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
            if !ok_len || !ok_chars {
                return Err(invalid(format!(
                    "symbol {raw:?} must be 1-12 letters, digits, '.' or '-'"
                )));
            }
        }
    }
    if is_iso_currency(&symbol) {
        return Err(invalid(format!(
            "{symbol} is a currency code and cannot be a security symbol"
        )));
    }
    if securities(ctx)?
        .iter()
        .any(|s| s.symbol == symbol && Some(s.id.as_str()) != except)
    {
        return Err(CommandError::Rejected(format!(
            "a security with symbol {symbol} already exists"
        )));
    }
    Ok(symbol)
}

pub(crate) fn check_name(name: &str) -> CommandResult<String> {
    let n = name.trim().to_owned();
    if !(1..=100).contains(&n.chars().count()) {
        return Err(invalid("a security name is 1-100 characters"));
    }
    Ok(n)
}

fn check_method(kind: SecurityKind, m: BasisMethod) -> CommandResult<()> {
    if m == BasisMethod::Average && !matches!(kind, SecurityKind::MutualFund | SecurityKind::Etf) {
        return Err(invalid(
            "the average basis method is only for mutual funds and ETFs",
        ));
    }
    Ok(())
}

fn check_cusip(c: Option<String>) -> CommandResult<Option<String>> {
    let c = c
        .map(|c| c.trim().to_ascii_uppercase())
        .filter(|c| !c.is_empty());
    if let Some(c) = &c
        && (c.chars().count() != 9 || !c.chars().all(|x| x.is_ascii_alphanumeric()))
    {
        return Err(invalid("a CUSIP is 9 letters or digits"));
    }
    Ok(c)
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SecurityCreateArgs {
    /// Options: generated from the terms (or the terms are read from it).
    #[serde(default)]
    pub symbol: String,
    /// Options: generated when left out.
    #[serde(default)]
    pub name: String,
    pub kind: SecurityKind,
    /// Pricing currency; the base currency (the default) in this version.
    #[serde(default)]
    pub currency: Option<CommodityCode>,
    /// fifo (default), average (mutual funds and ETFs) or specific.
    #[serde(default)]
    pub basis_method: Option<BasisMethod>,
    #[serde(default)]
    pub cusip: Option<String>,
    /// Option: the underlying stock or ETF (symbol or id; created if missing).
    #[serde(default)]
    pub underlying: Option<String>,
    /// Option: `call` or `put`.
    #[serde(default)]
    pub right: Option<crate::inv_kinds::Right>,
    /// Option: the strike price (up to 4 decimals).
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub strike: Option<Decimal>,
    /// Option: the expiration date.
    #[serde(default)]
    pub expiration: Option<Date>,
    /// Option: shares per contract (default 100).
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub multiplier: Option<Decimal>,
    /// Bond: face value of one unit (default 1,000).
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub face_per_unit: Option<Decimal>,
    /// Bond: coupon rate in percent (0 for zero-coupon).
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub coupon_rate: Option<Decimal>,
    /// Bond: coupons per year: 1, 2 (default), 4 or 12.
    #[serde(default)]
    pub payments_per_year: Option<u32>,
    /// Bond: the maturity date (a bond given one gets face, coupon and maturity).
    #[serde(default)]
    pub maturity: Option<Date>,
    /// Bond: the first coupon date (default derived back from maturity).
    #[serde(default)]
    pub first_payment: Option<Date>,
    /// Bond: `taxable` (default), `municipal` or `treasury`.
    #[serde(default)]
    pub tax: Option<crate::inv_kinds::BondTax>,
}

pub(crate) fn base_currency(ctx: &Ctx<'_>) -> CommandResult<String> {
    Ok(ctx.tx.info()?.base_currency)
}

/// Create a security (shared with the import commit).
pub(crate) fn create_security(
    ctx: &Ctx<'_>,
    symbol: &str,
    name: &str,
    kind: SecurityKind,
    method: Option<BasisMethod>,
    cusip: Option<String>,
) -> CommandResult<Security> {
    let v = security_create(
        ctx,
        SecurityCreateArgs {
            symbol: symbol.to_owned(),
            name: name.to_owned(),
            kind,
            currency: None,
            basis_method: method,
            cusip,
            ..SecurityCreateArgs::default()
        },
    )?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    find_security(ctx, &id)
}

/// Create a security from full arguments (options and bonds with their terms).
pub(crate) fn create_security_with(
    ctx: &Ctx<'_>,
    a: SecurityCreateArgs,
) -> CommandResult<Security> {
    let v = security_create(ctx, a)?;
    find_security(ctx, v.get("id").and_then(Value::as_str).unwrap_or_default())
}

fn security_create(ctx: &Ctx<'_>, mut a: SecurityCreateArgs) -> CommandResult<Value> {
    // Options and bonds: terms (the option's symbol and name are generated).
    let (option, bond) = crate::inv_kinds::terms_for_create(ctx, &mut a)?;
    let symbol = check_symbol_for(ctx, &a.symbol, None, a.kind)?;
    let base = base_currency(ctx)?;
    if symbol == base {
        return Err(invalid("the symbol must differ from the base currency"));
    }
    let currency = a.currency.map_or(base.clone(), |c| c.to_string());
    if currency != base {
        return Err(invalid(format!(
            "securities are priced in the base currency {base} in this version"
        )));
    }
    let method = a.basis_method.unwrap_or(BasisMethod::Fifo);
    check_method(a.kind, method)?;
    let s = Security {
        id: mk_core::Id::<()>::new().to_string(),
        symbol,
        name: check_name(&a.name)?,
        kind: a.kind,
        currency,
        basis_method: method,
        methods: vec![MethodChange {
            since: None,
            method,
        }],
        cusip: check_cusip(a.cusip)?,
        figi: None,
        created_at: now_rfc3339()?,
        aliases: Vec::new(),
        wash_group: None,
        option,
        bond,
    };
    save_security(ctx, &s)?;
    Ok(s.json())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityUpdateArgs {
    /// Security id or symbol.
    security: String,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<SecurityKind>,
    /// Applies to sales dated after `method_from` (default today).
    #[serde(default)]
    basis_method: Option<BasisMethod>,
    #[serde(default)]
    method_from: Option<Date>,
    #[serde(default)]
    cusip: Option<String>,
}

/// Whether anything refers to the security (an action or a security account).
pub(crate) fn security_used(ctx: &Ctx<'_>, s: &Security) -> CommandResult<bool> {
    if !ctx.tx.docs(DocTable::InvTxn, Some(&s.id))?.is_empty() {
        return Ok(true);
    }
    let chart = Chart::load(ctx)?;
    Ok(chart
        .accounts
        .iter()
        .any(|a| a.subtype == AccountSubtype::Security && a.commodity.as_str() == s.symbol))
}

fn security_update(ctx: &Ctx<'_>, a: SecurityUpdateArgs) -> CommandResult<Value> {
    let old = find_security(ctx, &a.security)?;
    let mut s = old.clone();
    if let Some(sym) = a.symbol {
        let sym = check_symbol_for(ctx, &sym, Some(&s.id), s.kind)?;
        if sym != s.symbol && security_used(ctx, &s)? {
            return Err(CommandError::Rejected(
                "the symbol cannot change after the security has been used".into(),
            ));
        }
        s.symbol = sym;
    }
    if let Some(n) = a.name {
        s.name = check_name(&n)?;
    }
    if let Some(k) = a.kind {
        if (k == SecurityKind::OptionContract) != (s.kind == SecurityKind::OptionContract)
            || (k == SecurityKind::Crypto) != (s.kind == SecurityKind::Crypto)
        {
            return Err(invalid(
                "a security cannot change to or from an option or crypto; create a new one",
            ));
        }
        if k != SecurityKind::Bond {
            s.bond = None;
        }
        s.kind = k;
    }
    if let Some(m) = a.basis_method
        && m != s.basis_method
    {
        let since = match a.method_from {
            Some(d) => d,
            None => today_utc()?,
        };
        s.methods.push(MethodChange {
            since: Some(since),
            method: m,
        });
        s.basis_method = m;
    }
    check_method(s.kind, s.basis_method)?;
    if a.cusip.is_some() {
        s.cusip = check_cusip(a.cusip)?;
    }
    if s != old {
        save_security(ctx, &s)?;
    }
    Ok(s.json())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SecurityRef {
    /// Security id or symbol.
    security: String,
}

fn security_get(ctx: &Ctx<'_>, a: SecurityRef) -> CommandResult<Value> {
    Ok(find_security(ctx, &a.security)?.json())
}

fn security_list(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let today = today_utc()?;
    let held = held_by_symbol(ctx, &chart, None)?;
    let mut out = Vec::new();
    for s in securities(ctx)? {
        let mut v = s.json();
        let latest = latest_price(ctx, &s.id, today)?;
        if let Some(o) = v.as_object_mut() {
            o.insert(
                "latest_price".into(),
                latest.map_or(
                    Value::Null,
                    |(d, p)| json!({"date": d, "price": price_text(p)}),
                ),
            );
            o.insert(
                "shares".into(),
                json!(format_amount(
                    held.get(&s.symbol).copied().unwrap_or_default(),
                    None
                )),
            );
        }
        out.push(v);
    }
    Ok(Value::Array(out))
}

fn security_delete(ctx: &Ctx<'_>, a: SecurityRef) -> CommandResult<Value> {
    let s = find_security(ctx, &a.security)?;
    if security_used(ctx, &s)? {
        return Err(CommandError::Rejected(
            "the security has been used and cannot be deleted".into(),
        ));
    }
    for p in ctx.tx.docs(DocTable::Price, Some(&s.id))? {
        ctx.save_doc(DocTable::Price, &p.id, None)?;
    }
    ctx.save_doc(DocTable::Security, &s.id, None)?;
    Ok(json!({"deleted": s.id}))
}

/// Shares held per symbol across security accounts (posted, as of a date).
pub(crate) fn held_by_symbol(
    ctx: &Ctx<'_>,
    chart: &Chart,
    as_of: Option<Date>,
) -> CommandResult<HashMap<String, Decimal>> {
    let mut out: HashMap<String, Decimal> = HashMap::new();
    for row in ctx.tx.posted_splits(as_of)? {
        if let Some(a) = chart.get(row.account)
            && a.subtype == AccountSubtype::Security
        {
            let slot = out.entry(a.commodity.to_string()).or_default();
            *slot = slot.checked_add(row.quantity).unwrap_or(*slot);
        }
    }
    Ok(out)
}

// ----- prices ----------------------------------------------------------------------

pub(crate) fn price_text(p: Decimal) -> String {
    format_amount(canonical_amount(p, 2), Some(2))
}

pub(crate) fn date_ord(d: Date) -> i64 {
    i64::from(d.year()) * 10_000 + i64::from(d.month()) * 100 + i64::from(d.day())
}

/// Every price of a security, by date.
pub(crate) fn prices_of(ctx: &Ctx<'_>, security: &str) -> CommandResult<BTreeMap<Date, Decimal>> {
    let mut out = BTreeMap::new();
    for d in ctx.tx.docs(DocTable::Price, Some(security))? {
        let date: Date = serde_json::from_value(d.json.get("date").cloned().unwrap_or_default())
            .map_err(bad_doc)?;
        let price: Decimal =
            serde_json::from_value(d.json.get("price").cloned().unwrap_or_default())
                .map_err(bad_doc)?;
        out.insert(date, price);
    }
    Ok(out)
}

/// The latest price on or before `as_of`, with its date.
pub(crate) fn latest_price(
    ctx: &Ctx<'_>,
    security: &str,
    as_of: Date,
) -> CommandResult<Option<(Date, Decimal)>> {
    Ok(prices_of(ctx, security)?
        .range(..=as_of)
        .next_back()
        .map(|(d, p)| (*d, *p)))
}

fn check_price(p: Decimal) -> CommandResult<Decimal> {
    if p.is_sign_negative() || p.is_zero() {
        return Err(invalid("a price must be greater than zero"));
    }
    if mk_core::currency::decimal_places(p) > 6 {
        return Err(invalid("a price has at most 6 decimal places"));
    }
    Ok(p.normalize())
}

pub(crate) fn store_price(
    ctx: &Ctx<'_>,
    s: &Security,
    date: Date,
    price: Decimal,
) -> CommandResult<()> {
    store_price_from(ctx, s, date, price, None)
}

/// Store a price with where it came from (a feed: "SnapTrade", "OFX — <label>").
pub(crate) fn store_price_from(
    ctx: &Ctx<'_>,
    s: &Security,
    date: Date,
    price: Decimal,
    source: Option<&str>,
) -> CommandResult<()> {
    let id = format!("{}:{date}", s.id);
    let mut json = json!({"security": s.id, "date": date, "price": price});
    if let (Some(src), Some(o)) = (source, json.as_object_mut()) {
        o.insert("source".into(), json!(src));
    }
    ctx.save_doc(
        DocTable::Price,
        &id,
        Some(&Doc {
            id: id.clone(),
            grp: Some(s.id.clone()),
            ord: date_ord(date),
            json,
        }),
    )
}

/// The source of the price stored for a date, if any.
pub(crate) fn price_source(
    ctx: &Ctx<'_>,
    s: &Security,
    date: Date,
) -> CommandResult<Option<String>> {
    Ok(ctx
        .tx
        .get_doc(DocTable::Price, &format!("{}:{date}", s.id))?
        .and_then(|d| {
            d.json
                .get("source")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PriceSetArgs {
    /// Security id or symbol.
    security: String,
    date: Date,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    price: Decimal,
}

fn price_set(ctx: &Ctx<'_>, a: PriceSetArgs) -> CommandResult<Value> {
    let s = find_security(ctx, &a.security)?;
    let p = check_price(a.price)?;
    store_price(ctx, &s, a.date, p)?;
    Ok(json!({"security": s.id, "symbol": s.symbol, "date": a.date, "price": price_text(p)}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PriceImportArgs {
    content_base64: String,
    #[serde(default)]
    file_name: Option<String>,
}

pub(crate) fn parse_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_owned()).collect()
}

fn parse_price_date(s: &str) -> Option<Date> {
    if let Ok(d) = s.parse::<Date>() {
        return Some(d);
    }
    let mut it = s.split('/');
    let m: u8 = it.next()?.trim().parse().ok()?;
    let d: u8 = it.next()?.trim().parse().ok()?;
    let y: i32 = it.next()?.trim().parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Date::from_ymd(y, m, d).ok()
}

fn price_import(ctx: &Ctx<'_>, a: PriceImportArgs) -> CommandResult<Value> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(a.content_base64.trim())
        .map_err(|e| invalid(format!("content_base64 is not base64: {e}")))?;
    *ctx.audit_args.borrow_mut() = Some(json!({
        "file_name": a.file_name,
        "bytes": bytes.len(),
    }));
    let text = String::from_utf8_lossy(&bytes).replace('\u{feff}', "");
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header = lines
        .next()
        .map(parse_csv_line)
        .ok_or_else(|| invalid("the file is empty"))?;
    let col = |name: &str| header.iter().position(|h| h.eq_ignore_ascii_case(name));
    let (Some(dc), Some(sc), Some(pc)) = (col("date"), col("symbol"), col("price")) else {
        return Err(invalid(
            "the header row needs date, symbol and price columns",
        ));
    };
    let secs = securities(ctx)?;
    let mut stored = 0usize;
    let mut skipped = 0usize;
    let mut unknown: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for (i, line) in lines.enumerate() {
        let cells = parse_csv_line(line);
        let get = |k: usize| cells.get(k).map(String::as_str).unwrap_or("");
        let symbol = get(sc).to_ascii_uppercase();
        let Some(s) = secs.iter().find(|s| s.symbol == symbol) else {
            skipped += 1;
            if !unknown.contains(&symbol) {
                warnings.push(format!("unknown symbol {symbol:?}: its rows were skipped"));
                unknown.push(symbol);
            }
            continue;
        };
        let row = i + 2;
        let Some(date) = parse_price_date(get(dc)) else {
            skipped += 1;
            warnings.push(format!("row {row}: {:?} is not a date", get(dc)));
            continue;
        };
        let price = get(pc).replace(['$', ','], "");
        match price
            .parse::<Decimal>()
            .map_err(|_| ())
            .and_then(|p| check_price(p).map_err(|_| ()))
        {
            Ok(p) => {
                store_price(ctx, s, date, p)?;
                stored += 1;
            }
            Err(()) => {
                skipped += 1;
                warnings.push(format!("row {row}: {:?} is not a valid price", get(pc)));
            }
        }
    }
    Ok(json!({"stored": stored, "skipped": skipped, "warnings": warnings}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PriceListArgs {
    security: String,
    #[serde(default)]
    from: Option<Date>,
    #[serde(default)]
    to: Option<Date>,
}

fn price_list(ctx: &Ctx<'_>, a: PriceListArgs) -> CommandResult<Value> {
    let s = find_security(ctx, &a.security)?;
    let mut rows = Vec::new();
    for (d, p) in prices_of(ctx, &s.id)?
        .into_iter()
        .filter(|(d, _)| a.from.is_none_or(|f| *d >= f) && a.to.is_none_or(|t| *d <= t))
    {
        let mut row = json!({"date": d, "price": price_text(p)});
        if let (Some(src), Some(o)) = (price_source(ctx, &s, d)?, row.as_object_mut()) {
            o.insert("source".into(), json!(src));
        }
        rows.push(row);
    }
    Ok(Value::Array(rows))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PriceLatestArgs {
    security: String,
    /// Default today.
    #[serde(default)]
    as_of: Option<Date>,
}

fn price_latest(ctx: &Ctx<'_>, a: PriceLatestArgs) -> CommandResult<Value> {
    let s = find_security(ctx, &a.security)?;
    let as_of = match a.as_of {
        Some(d) => d,
        None => today_utc()?,
    };
    Ok(match latest_price(ctx, &s.id, as_of)? {
        Some((d, p)) => json!({"security": s.id, "date": d, "price": price_text(p)}),
        None => Value::Null,
    })
}

// ----- actions: stored metadata ----------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    Buy,
    Sell,
    Dividend,
    Reinvest,
    Interest,
    Fee,
    SharesIn,
    SharesOut,
    Split,
    ReturnOfCapital,
    /// A merger (investments-advanced.md): old shares become new shares.
    Merger,
    /// A spin-off: part of the parent's basis moves to new shares.
    Spinoff,
    /// A fractional quantity sold for cash, FIFO.
    CashInLieu,
    // investments-options-bonds-crypto.md
    /// Buy to open (closes short lots first).
    OptionBuy,
    /// Sell to close.
    OptionSell,
    /// Sell to open (a short position).
    OptionWrite,
    /// Buy to close a short position.
    OptionCover,
    OptionExpire,
    /// A long option exercised: shares of the underlying bought (call) or sold (put).
    OptionExercise,
    /// A short option assigned: shares sold (call) or bought (put).
    OptionAssigned,
    BondBuy,
    BondSell,
    BondInterest,
    BondRedeem,
    /// A fee paid in coin: a disposal at market value.
    CryptoFee,
    /// One coin for another in the same account.
    CryptoSwap,
    /// Staking or other rewards: income and a lot at market value.
    CryptoReward,
    /// Lots moved between two investment accounts with their dates and basis.
    TransferLots,
}

impl Action {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Action::Buy => "Buy",
            Action::Sell => "Sell",
            Action::Dividend => "Dividend",
            Action::Reinvest => "Reinvest",
            Action::Interest => "Interest",
            Action::Fee => "Fee",
            Action::SharesIn => "Shares in",
            Action::SharesOut => "Shares out",
            Action::Split => "Split",
            Action::ReturnOfCapital => "Return of capital",
            Action::Merger => "Merger",
            Action::Spinoff => "Spin-off",
            Action::CashInLieu => "Cash in lieu",
            Action::OptionBuy => "Buy to open",
            Action::OptionSell => "Sell to close",
            Action::OptionWrite => "Sell to open",
            Action::OptionCover => "Buy to close",
            Action::OptionExpire => "Option expired",
            Action::OptionExercise => "Option exercised",
            Action::OptionAssigned => "Option assigned",
            Action::BondBuy => "Buy bond",
            Action::BondSell => "Sell bond",
            Action::BondInterest => "Bond interest",
            Action::BondRedeem => "Bond redeemed",
            Action::CryptoFee => "Fee paid in coin",
            Action::CryptoSwap => "Swap",
            Action::CryptoReward => "Reward",
            Action::TransferLots => "Transfer lots",
        }
    }

    /// The actions of investments-options-bonds-crypto.md (posted and replayed by
    /// `inv_kinds`).
    pub(crate) fn is_extra(self) -> bool {
        matches!(
            self,
            Action::OptionBuy
                | Action::OptionSell
                | Action::OptionWrite
                | Action::OptionCover
                | Action::OptionExpire
                | Action::OptionExercise
                | Action::OptionAssigned
                | Action::BondBuy
                | Action::BondSell
                | Action::BondInterest
                | Action::BondRedeem
                | Action::CryptoFee
                | Action::CryptoSwap
                | Action::CryptoReward
                | Action::TransferLots
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DividendKind {
    Ordinary,
    Qualified,
    CapitalGainLong,
    CapitalGainShort,
    TaxExempt,
}

impl DividendKind {
    fn is_capital_gain(self) -> bool {
        matches!(
            self,
            DividendKind::CapitalGainLong | DividendKind::CapitalGainShort
        )
    }
}

/// A lot argument: `{lot, quantity}` for a sale or shares out (specific
/// identification), `{acquired, quantity, basis}` for shares in.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct LotArg {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquired: Option<Date>,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    pub quantity: Decimal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub basis: Option<Decimal>,
    /// Shares in: the basis is a placeholder at market value, to be supplied later
    /// (opening-balances.md "Basis needed").
    #[serde(default, skip_serializing_if = "mk_core::ledger::is_false")]
    pub basis_needed: bool,
    /// Shares in: bought on several dates ("Various"); `acquired` is the start date.
    #[serde(default, skip_serializing_if = "mk_core::ledger::is_false")]
    pub acquired_various: bool,
}

/// Arguments shared by every action; each action checks the ones it needs.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActionArgs {
    /// Brokerage account (id, full name or code). Not used by `inv.split`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Security id or symbol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<Date>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub quantity: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub price: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub commission: Option<Decimal>,
    /// Total cost (buy), net proceeds (sell) or the amount of a dividend, interest,
    /// fee or return of capital.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub amount: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lots: Option<Vec<LotArg>>,
    /// Dividend kind (default ordinary).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<DividendKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub income_account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expense_account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub ratio_new: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub ratio_old: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    // investments-options-bonds-crypto.md
    /// Bonds: units (of the bond's face per unit); the same as `quantity`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub units: Option<Decimal>,
    /// Bond buy (paid) or sale (received): accrued interest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub accrued_interest: Option<Decimal>,
    /// Crypto swap: the coin given (as `security`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub from_quantity: Option<Decimal>,
    /// Crypto swap: the coin received.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub to_quantity: Option<Decimal>,
    /// Crypto fee, swap, reward: market value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub value: Option<Decimal>,
    /// Crypto swap: a fee paid in cash (added to the basis of what was received).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub fee: Option<Decimal>,
    /// Transfer of lots: a network fee in coin and its market value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub fee_quantity: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    pub fee_value: Option<Decimal>,
    /// Transfer of lots: the investment account receiving them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_account: Option<String>,
    /// Exercise of a put / assignment of a call: lots of the underlying to sell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlying_lots: Option<Vec<LotArg>>,
    /// Shares in (opening holdings): a written option, recorded as short lots whose
    /// basis is the premium received.
    #[serde(default, skip_serializing_if = "mk_core::ledger::is_false")]
    pub short: bool,
}

/// The stored metadata of one investment transaction (`inv_txn` document).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct InvMeta {
    pub txn: TransactionId,
    pub action: Action,
    /// The brokerage account (none for a split).
    pub account: Option<AccountId>,
    pub security: Option<String>,
    /// The security account the action touches (none for a split, interest, fee
    /// without a security).
    pub sec_account: Option<AccountId>,
    pub date: Date,
    pub quantity: Option<Decimal>,
    pub price: Option<Decimal>,
    pub commission: Decimal,
    /// Cost (buy), net proceeds (sell), or the action's amount.
    pub amount: Option<Decimal>,
    /// Whether `amount` was given (it overrides quantity × price).
    pub amount_given: bool,
    pub lots: Option<Vec<LotArg>>,
    pub method: Option<BasisMethod>,
    pub kind: Option<DividendKind>,
    /// The income, expense, Realized Gains or Opening Balances account used.
    pub other_account: Option<AccountId>,
    pub ratio_new: Option<Decimal>,
    pub ratio_old: Option<Decimal>,
    /// The original arguments (for `inv.get` and pre-filled forms).
    pub args: ActionArgs,
    /// Mergers and spin-offs: the new security and its account in this brokerage.
    #[serde(default)]
    pub security2: Option<String>,
    #[serde(default)]
    pub sec_account2: Option<AccountId>,
    /// Mergers and spin-offs: the terms.
    #[serde(default)]
    pub corp: Option<CorpTerms>,
    /// Options, bonds and crypto: what their replay needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<crate::inv_kinds::Extra>,
}

/// The terms of a merger or spin-off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CorpTerms {
    /// Merger: `tax_free` or `taxable`.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub cash_per_share: Option<Decimal>,
    #[serde(default)]
    pub new_share_value: Option<Decimal>,
    /// Spin-off: new shares per parent share and the basis percentage moved.
    #[serde(default)]
    pub new_per_parent: Option<Decimal>,
    #[serde(default)]
    pub allocation_percent: Option<Decimal>,
}

pub(crate) fn meta_of(ctx: &Ctx<'_>, txn: TransactionId) -> CommandResult<Option<InvMeta>> {
    match ctx.tx.get_doc(DocTable::InvTxn, &txn.to_string())? {
        Some(d) => Ok(Some(serde_json::from_value(d.json).map_err(bad_doc)?)),
        None => Ok(None),
    }
}

/// Every investment record, by transaction id.
pub(crate) fn all_metas(ctx: &Ctx<'_>) -> CommandResult<HashMap<TransactionId, InvMeta>> {
    let mut out = HashMap::new();
    for d in ctx.tx.docs(DocTable::InvTxn, None)? {
        let m: InvMeta = serde_json::from_value(d.json).map_err(bad_doc)?;
        out.insert(m.txn, m);
    }
    Ok(out)
}

pub(crate) fn save_meta(ctx: &Ctx<'_>, m: &InvMeta) -> CommandResult<()> {
    let id = m.txn.to_string();
    ctx.save_doc(
        DocTable::InvTxn,
        &id,
        Some(&Doc {
            id: id.clone(),
            grp: m.security.clone(),
            ord: date_ord(m.date),
            json: serde_json::to_value(m)?,
        }),
    )
}

// ----- accounts --------------------------------------------------------------------

pub(crate) fn brokerage<'c>(chart: &'c Chart, r: Option<&str>) -> CommandResult<&'c Account> {
    let r = r.ok_or_else(|| invalid("give the brokerage account"))?;
    let a = chart.resolve(r)?;
    if a.subtype != AccountSubtype::Brokerage {
        return Err(invalid(format!(
            "{} is not a brokerage account",
            chart.full_name(a.id)
        )));
    }
    Ok(a)
}

fn new_account(
    ctx: &Ctx<'_>,
    name: &str,
    ty: AccountType,
    subtype: AccountSubtype,
    parent: Option<AccountId>,
    commodity: CommodityCode,
) -> CommandResult<Account> {
    let a = Account {
        id: AccountId::new(),
        name: name.to_owned(),
        account_type: ty,
        subtype,
        parent,
        commodity,
        code: None,
        description: None,
        placeholder: false,
        archived: false,
        created_at: now_rfc3339()?,
        tax_line: None,
        role: None,
        retirement: false,
        opening_none: false,
    };
    ctx.save_account(None, &a)?;
    Ok(a)
}

/// A top-level account named `name` of type `ty`, created on first use.
pub(crate) fn ensure_top(
    ctx: &Ctx<'_>,
    chart: &mut Chart,
    name: &str,
    ty: AccountType,
    subtype: AccountSubtype,
) -> CommandResult<AccountId> {
    use crate::chart::roles;
    // chart-presets.md: the investments auto accounts are found by role first; the
    // account found by name or created gets the role.
    let role = match (ty, name) {
        (AccountType::Income, "Dividends") => Some(roles::INV_DIVIDENDS),
        (AccountType::Income, "Interest") => Some(roles::INV_INTEREST),
        (AccountType::Income, "Realized Gains") => Some(roles::INV_REALIZED),
        (AccountType::Expense, "Investment Fees") => Some(roles::INV_FEES),
        _ => None,
    };
    if let Some(r) = role
        && let Some(a) = roles::holder(chart, r)
    {
        return Ok(a.id);
    }
    let id =
        if let Some(a) = chart.accounts.iter().find(|a| {
            a.parent.is_none() && a.account_type == ty && a.name.eq_ignore_ascii_case(name)
        }) {
            a.id
        } else {
            let base: CommodityCode = base_currency(ctx)?.parse()?;
            let a = new_account(ctx, name, ty, subtype, None, base)?;
            let id = a.id;
            chart.accounts.push(a);
            id
        };
    if let Some(r) = role {
        roles::give(ctx, id, r)?;
        *chart = Chart::load(ctx)?;
    }
    Ok(id)
}

pub(crate) fn opening_balances(ctx: &Ctx<'_>, chart: &mut Chart) -> CommandResult<AccountId> {
    if let Some(a) = chart
        .accounts
        .iter()
        .find(|a| a.subtype == AccountSubtype::OpeningBalances && !a.placeholder)
    {
        return Ok(a.id);
    }
    ensure_top(
        ctx,
        chart,
        "Opening Balances",
        AccountType::Equity,
        AccountSubtype::OpeningBalances,
    )
}

/// The security account of `symbol` under `brokerage`, created on first use.
pub(crate) fn ensure_sec_account(
    ctx: &Ctx<'_>,
    chart: &mut Chart,
    brokerage: AccountId,
    symbol: &str,
) -> CommandResult<AccountId> {
    if let Some(a) = sec_account_of(chart, brokerage, symbol) {
        return Ok(a);
    }
    let name = if chart
        .children(brokerage)
        .any(|c| c.name.eq_ignore_ascii_case(symbol))
    {
        format!("{symbol} shares")
    } else {
        symbol.to_owned()
    };
    let a = new_account(
        ctx,
        &name,
        AccountType::Asset,
        AccountSubtype::Security,
        Some(brokerage),
        symbol.parse()?,
    )?;
    let id = a.id;
    chart.accounts.push(a);
    Ok(id)
}

pub(crate) fn sec_account_of(
    chart: &Chart,
    brokerage: AccountId,
    symbol: &str,
) -> Option<AccountId> {
    chart
        .children(brokerage)
        .find(|c| c.subtype == AccountSubtype::Security && c.commodity.as_str() == symbol)
        .map(|c| c.id)
}

/// An income or expense account: the named one, or the default (created on first use).
pub(crate) fn other_account(
    ctx: &Ctx<'_>,
    chart: &mut Chart,
    given: Option<&str>,
    default: &str,
    ty: AccountType,
) -> CommandResult<AccountId> {
    match given {
        Some(r) => {
            let a = chart.resolve(r)?;
            if a.account_type != ty {
                return Err(invalid(format!(
                    "{} is not an {ty} account",
                    chart.full_name(a.id)
                )));
            }
            Ok(a.id)
        }
        None => {
            let sub = if ty == AccountType::Income {
                AccountSubtype::Income
            } else {
                AccountSubtype::Expense
            };
            ensure_top(ctx, chart, default, ty, sub)
        }
    }
}

// ----- actions: posting ------------------------------------------------------------

pub(crate) fn need<T: Copy>(v: Option<T>, what: &str) -> CommandResult<T> {
    v.ok_or_else(|| invalid(format!("give {what}")))
}

pub(crate) fn positive(v: Decimal, what: &str) -> CommandResult<Decimal> {
    if v.is_sign_negative() || v.is_zero() {
        return Err(invalid(format!("{what} must be greater than zero")));
    }
    Ok(v)
}

pub(crate) fn money_amount(v: Decimal, what: &str) -> CommandResult<Decimal> {
    if mk_core::currency::decimal_places(v) > 2 {
        return Err(invalid(format!("{what} has more than 2 decimal places")));
    }
    Ok(v)
}

pub(crate) fn quantity_arg(v: Option<Decimal>) -> CommandResult<Decimal> {
    let q = positive(need(v, "a quantity")?, "the quantity")?;
    if mk_core::currency::decimal_places(q) > lots::QUANTITY_DP {
        return Err(invalid("a quantity has at most 9 decimal places"));
    }
    Ok(q)
}

pub(crate) fn split_of(account: AccountId, value: Decimal, quantity: Decimal) -> Split {
    Split {
        account,
        value: canonical_amount(value, 2),
        quantity: canonical_amount(quantity, 0),
        memo: None,
        reconcile: Reconcile::New,
        fingerprint: None,
        tags: Default::default(),
    }
}

pub(crate) fn cash_split(account: AccountId, value: Decimal) -> Split {
    split_of(account, value, value)
}

pub(crate) fn neg(v: Decimal) -> Decimal {
    -v
}

pub(crate) fn round2(v: Decimal) -> Decimal {
    lots::money(v)
}

pub(crate) fn mul(a: Decimal, b: Decimal) -> CommandResult<Decimal> {
    a.checked_mul(b)
        .ok_or_else(|| CommandError::Rejected("arithmetic overflow".into()))
}

pub(crate) fn plus(a: Decimal, b: Decimal) -> CommandResult<Decimal> {
    a.checked_add(b)
        .ok_or_else(|| CommandError::Rejected("arithmetic overflow".into()))
}

pub(crate) fn minus(a: Decimal, b: Decimal) -> CommandResult<Decimal> {
    a.checked_sub(b)
        .ok_or_else(|| CommandError::Rejected("arithmetic overflow".into()))
}

/// What a posting needs: the transaction's splits, payee and memo, and the metadata.
pub(crate) struct Posting {
    pub splits: Vec<Split>,
    pub payee: Option<String>,
    pub memo: Option<String>,
    pub meta: InvMeta,
    /// Security accounts whose lots must be replayed.
    pub replay: Vec<AccountId>,
}

pub(crate) fn describe(
    action: Action,
    symbol: Option<&str>,
    q: Option<Decimal>,
    price: Option<Decimal>,
) -> String {
    let mut s = action.label().to_owned();
    if let Some(q) = q {
        s.push_str(&format!(" {}", format_amount(q, None)));
    }
    if let Some(sym) = symbol {
        s.push_str(&format!(" {sym}"));
    }
    if let Some(p) = price {
        s.push_str(&format!(" @ {}", price_text(p)));
    }
    s
}

fn build_posting(
    ctx: &Ctx<'_>,
    action: Action,
    a: &ActionArgs,
    id: TransactionId,
) -> CommandResult<Posting> {
    let mut chart = Chart::load(ctx)?;
    let date = need(a.date, "a date")?;
    let commission = money_amount(a.commission.unwrap_or_default(), "the commission")?;
    if commission.is_sign_negative() {
        return Err(invalid("the commission cannot be negative"));
    }
    let sec = match (a.security.as_ref().or(a.from.as_ref()), action) {
        (Some(s), _) => Some(find_security(ctx, s)?),
        (None, Action::Interest | Action::Fee) => None,
        (None, _) => return Err(invalid("give the security")),
    };
    // Options in whole contracts, bonds in whole units, crypto up to 8 decimals.
    if let (Some(s), Some(q)) = (&sec, a.quantity.or(a.units)) {
        crate::inv_kinds::check_quantity(s, q)?;
    }
    let symbol = sec.as_ref().map(|s| s.symbol.clone());
    let mut meta = InvMeta {
        txn: id,
        action,
        account: None,
        security: sec.as_ref().map(|s| s.id.clone()),
        sec_account: None,
        date,
        quantity: None,
        price: a.price,
        commission,
        amount: None,
        amount_given: a.amount.is_some(),
        lots: a.lots.clone(),
        method: None,
        kind: None,
        other_account: None,
        ratio_new: None,
        ratio_old: None,
        args: a.clone(),
        security2: None,
        sec_account2: None,
        corp: None,
        extra: None,
    };
    let mut splits = Vec::new();
    let mut replay = Vec::new();
    if action == Action::Split {
        let s = sec.as_ref().ok_or_else(|| invalid("give the security"))?;
        let new = positive(need(a.ratio_new, "ratio_new")?, "ratio_new")?;
        let old = positive(need(a.ratio_old, "ratio_old")?, "ratio_old")?;
        meta.ratio_new = Some(new);
        meta.ratio_old = Some(old);
        // Every account holding the security on that date; quantities are filled by
        // the replay (the zero cash split keeps the entry two-sided).
        let holders: Vec<&Account> = chart
            .accounts
            .iter()
            .filter(|x| x.subtype == AccountSubtype::Security && x.commodity.as_str() == s.symbol)
            .collect();
        let held = held_on(ctx, &holders.iter().map(|x| x.id).collect::<Vec<_>>(), date)?;
        for h in &holders {
            if let Some(q) = held.get(&h.id).filter(|q| !q.is_zero()) {
                // Shares added; the replay settles the exact figure from the lots.
                let after = lots::round(
                    mul(*q, new)?
                        .checked_div(old)
                        .ok_or_else(|| invalid("bad split ratio"))?,
                    lots::QUANTITY_DP,
                );
                splits.push(split_of(h.id, Decimal::ZERO, minus(after, *q)?));
            }
            replay.push(h.id);
        }
        let Some(first) = holders
            .iter()
            .find(|h| held.get(&h.id).is_some_and(|q| !q.is_zero()))
        else {
            return Err(coded(
                "insufficient_shares",
                format!("no account holds {} on {date}", s.symbol),
            ));
        };
        let cash = first
            .parent
            .ok_or_else(|| CommandError::Integrity("security account without a parent".into()))?;
        splits.push(cash_split(cash, Decimal::ZERO));
        return Ok(Posting {
            splits,
            payee: Some(s.symbol.clone()),
            memo: Some(format!(
                "Split {}-for-{} {}",
                format_amount(new, None),
                format_amount(old, None),
                s.symbol
            )),
            meta,
            replay,
        });
    }
    let broker = brokerage(&chart, a.account.as_deref())?.id;
    meta.account = Some(broker);
    if action.is_extra() {
        let s = sec.as_ref().ok_or_else(|| invalid("give the security"))?;
        return crate::inv_kinds::build(ctx, &mut chart, action, a, meta, s, broker, commission);
    }
    let sec_acc = match &symbol {
        Some(sym) => Some(ensure_sec_account(ctx, &mut chart, broker, sym)?),
        None => None,
    };
    meta.sec_account = sec_acc;
    if let Some(x) = sec_acc {
        replay.push(x);
    }
    let sec_acc_or = || sec_acc.ok_or_else(|| invalid("give the security"));
    match action {
        Action::Buy => {
            let q = quantity_arg(a.quantity)?;
            let cost = match a.amount {
                Some(amt) => money_amount(positive(amt, "the amount")?, "the amount")?,
                None => {
                    let p = positive(need(a.price, "a price")?, "the price")?;
                    let v = match &sec {
                        Some(s) => s.worth(q, p)?,
                        None => value_of(SecurityKind::Stock, q, p)?,
                    };
                    plus(v, commission)?
                }
            };
            meta.quantity = Some(q);
            meta.amount = Some(cost);
            splits.push(split_of(sec_acc_or()?, cost, q));
            splits.push(cash_split(broker, neg(cost)));
        }
        Action::Sell | Action::CashInLieu => {
            let q = quantity_arg(a.quantity)?;
            let proceeds = match a.amount {
                Some(amt) => money_amount(amt, "the amount")?,
                None => {
                    let p = positive(need(a.price, "a price")?, "the price")?;
                    let v = match &sec {
                        Some(s) => s.worth(q, p)?,
                        None => value_of(SecurityKind::Stock, q, p)?,
                    };
                    minus(v, commission)?
                }
            };
            let s = sec.as_ref().ok_or_else(|| invalid("give the security"))?;
            meta.quantity = Some(q);
            meta.amount = Some(proceeds);
            // Cash in lieu always takes the oldest shares first.
            meta.method = Some(if action == Action::CashInLieu {
                BasisMethod::Fifo
            } else {
                s.method_on(date)
            });
            let gains = other_account(
                ctx,
                &mut chart,
                a.income_account.as_deref(),
                "Realized Gains",
                AccountType::Income,
            )?;
            meta.other_account = Some(gains);
            // The security value and gain are set by the replay.
            splits.push(cash_split(broker, proceeds));
            splits.push(split_of(sec_acc_or()?, Decimal::ZERO, neg(q)));
            splits.push(cash_split(gains, neg(proceeds)));
        }
        Action::Dividend | Action::Reinvest => {
            let kind = a.kind.unwrap_or(DividendKind::Ordinary);
            meta.kind = Some(kind);
            let default = if kind.is_capital_gain() {
                "Capital Gain Distributions"
            } else {
                "Dividends"
            };
            let income = other_account(
                ctx,
                &mut chart,
                a.income_account.as_deref(),
                default,
                AccountType::Income,
            )?;
            meta.other_account = Some(income);
            if action == Action::Dividend {
                let amt = money_amount(
                    positive(need(a.amount, "the amount")?, "the amount")?,
                    "the amount",
                )?;
                meta.amount = Some(amt);
                splits.push(cash_split(broker, amt));
                splits.push(cash_split(income, neg(amt)));
            } else {
                let q = quantity_arg(a.quantity)?;
                let amt = match (a.amount, a.price) {
                    (Some(amt), _) => money_amount(positive(amt, "the amount")?, "the amount")?,
                    (None, Some(p)) => match &sec {
                        Some(s) => s.worth(q, positive(p, "the price")?)?,
                        None => value_of(SecurityKind::Stock, q, positive(p, "the price")?)?,
                    },
                    (None, None) => return Err(invalid("give the amount or the price")),
                };
                if meta.price.is_none() {
                    meta.price = amt.checked_div(q).map(|p| p.round_dp(6).normalize());
                }
                meta.quantity = Some(q);
                meta.amount = Some(amt);
                splits.push(split_of(sec_acc_or()?, amt, q));
                splits.push(cash_split(income, neg(amt)));
            }
        }
        Action::Interest => {
            let amt = money_amount(
                positive(need(a.amount, "the amount")?, "the amount")?,
                "the amount",
            )?;
            let income = other_account(
                ctx,
                &mut chart,
                a.income_account.as_deref(),
                "Interest",
                AccountType::Income,
            )?;
            meta.amount = Some(amt);
            meta.other_account = Some(income);
            splits.push(cash_split(broker, amt));
            splits.push(cash_split(income, neg(amt)));
        }
        Action::Fee => {
            let amt = money_amount(
                positive(need(a.amount, "the amount")?, "the amount")?,
                "the amount",
            )?;
            let expense = other_account(
                ctx,
                &mut chart,
                a.expense_account.as_deref(),
                "Investment Fees",
                AccountType::Expense,
            )?;
            meta.amount = Some(amt);
            meta.other_account = Some(expense);
            splits.push(cash_split(expense, amt));
            splits.push(cash_split(broker, neg(amt)));
        }
        Action::SharesIn => {
            let q = quantity_arg(a.quantity)?;
            let given = a
                .lots
                .as_ref()
                .filter(|l| !l.is_empty())
                .ok_or_else(|| invalid("give the lots received (acquired, quantity, basis)"))?;
            let mut sum_q = Decimal::ZERO;
            let mut sum_b = Decimal::ZERO;
            for (i, l) in given.iter().enumerate() {
                if l.acquired.is_none() {
                    return Err(invalid(format!("lot {}: give the acquired date", i + 1)));
                }
                positive(l.quantity, "a lot quantity")?;
                let b = money_amount(need(l.basis, "each lot's basis")?, "a lot basis")?;
                if b.is_sign_negative() {
                    return Err(invalid("a lot basis cannot be negative"));
                }
                sum_q = plus(sum_q, l.quantity)?;
                sum_b = plus(sum_b, b)?;
            }
            if sum_q != q {
                return Err(invalid(format!(
                    "the lots total {} shares, not {}",
                    format_amount(sum_q, None),
                    format_amount(q, None)
                )));
            }
            let ob = opening_balances(ctx, &mut chart)?;
            meta.quantity = Some(q);
            meta.amount = Some(sum_b);
            meta.other_account = Some(ob);
            if a.short {
                // A written option held at the start: short lots, premium as basis.
                if sec
                    .as_ref()
                    .is_none_or(|s| s.kind != SecurityKind::OptionContract)
                {
                    return Err(invalid("only options can be held short"));
                }
                meta.extra = Some(crate::inv_kinds::Extra {
                    short: true,
                    ..Default::default()
                });
                splits.push(split_of(sec_acc_or()?, neg(sum_b), neg(q)));
                splits.push(cash_split(ob, sum_b));
            } else {
                splits.push(split_of(sec_acc_or()?, sum_b, q));
                splits.push(cash_split(ob, neg(sum_b)));
            }
        }
        Action::SharesOut => {
            let q = quantity_arg(a.quantity)?;
            let s = sec.as_ref().ok_or_else(|| invalid("give the security"))?;
            let ob = opening_balances(ctx, &mut chart)?;
            meta.quantity = Some(q);
            meta.method = Some(s.method_on(date));
            meta.other_account = Some(ob);
            splits.push(split_of(sec_acc_or()?, Decimal::ZERO, neg(q)));
            splits.push(cash_split(ob, Decimal::ZERO));
        }
        Action::ReturnOfCapital => {
            let amt = money_amount(
                positive(need(a.amount, "the amount")?, "the amount")?,
                "the amount",
            )?;
            meta.amount = Some(amt);
            // Beyond the open basis the excess is a gain; the replay settles it.
            meta.other_account = Some(other_account(
                ctx,
                &mut chart,
                a.income_account.as_deref(),
                "Realized Gains",
                AccountType::Income,
            )?);
            splits.push(cash_split(broker, amt));
            splits.push(split_of(sec_acc_or()?, neg(amt), Decimal::ZERO));
        }
        Action::Split => {}
        Action::Merger | Action::Spinoff => {
            return Err(invalid(
                "mergers and spin-offs are recorded with inv.merger and inv.spinoff",
            ));
        }
        _ => return Err(invalid("this action is recorded with its own command")),
    }
    let payee = symbol.clone().or_else(|| Some(action.label().to_owned()));
    let memo = a.memo.clone().filter(|m| !m.trim().is_empty()).or_else(|| {
        Some(describe(
            action,
            symbol.as_deref(),
            meta.quantity,
            if matches!(
                action,
                Action::Buy | Action::Sell | Action::Reinvest | Action::CashInLieu
            ) {
                meta.price
            } else {
                None
            },
        ))
    });
    Ok(Posting {
        splits,
        payee,
        memo,
        meta,
        replay,
    })
}

/// Shares held per account on a date (posted).
pub(crate) fn held_on(
    ctx: &Ctx<'_>,
    accounts: &[AccountId],
    date: Date,
) -> CommandResult<HashMap<AccountId, Decimal>> {
    let set: HashSet<AccountId> = accounts.iter().copied().collect();
    let mut out: HashMap<AccountId, Decimal> = HashMap::new();
    for row in ctx.tx.posted_splits(Some(date))? {
        if set.contains(&row.account) {
            let slot = out.entry(row.account).or_default();
            *slot = slot.checked_add(row.quantity).unwrap_or(*slot);
        }
    }
    Ok(out)
}

/// Post an action as a new transaction (or replace `existing` with it).
pub(crate) fn post(
    ctx: &Ctx<'_>,
    action: Action,
    a: ActionArgs,
    existing: Option<(Transaction, InvMeta)>,
) -> CommandResult<Value> {
    post_with(ctx, action, a, existing, None, false)
}

/// [`post`], with the other side of shares in replaced by `offset` (an opening
/// entry against Owner's Equity, opening-balances.md). An edit of shares in keeps the
/// other side it had, and any edit keeps the opening mark.
pub(crate) fn post_with(
    ctx: &Ctx<'_>,
    action: Action,
    a: ActionArgs,
    existing: Option<(Transaction, InvMeta)>,
    offset: Option<AccountId>,
    opening: bool,
) -> CommandResult<Value> {
    let (id, entered_at) = match &existing {
        Some((t, _)) => (t.id, t.entered_at.clone()),
        None => (TransactionId::new(), now_rfc3339()?),
    };
    let mut p = build_posting(ctx, action, &a, id)?;
    let keep = existing
        .as_ref()
        .filter(|(_, m)| m.action == Action::SharesIn && action == Action::SharesIn)
        .and_then(|(_, m)| m.other_account);
    if let (Some(to), Some(from)) = (offset.or(keep), p.meta.other_account)
        && action == Action::SharesIn
        && to != from
    {
        for s in &mut p.splits {
            if s.account == from {
                s.account = to;
            }
        }
        p.meta.other_account = Some(to);
    }
    let mut splits = p.splits;
    mk_ledger::sort_splits(&mut splits);
    let base: CommodityCode = base_currency(ctx)?.parse()?;
    let mut t = Transaction {
        id,
        date: p.meta.date,
        currency: base,
        payee: p.payee,
        number: None,
        memo: p.memo,
        status: TxnStatus::Posted,
        void_reason: None,
        reverses: None,
        opening: false,
        entered_at,
        splits,
    };
    let mut replay = p.replay;
    match &existing {
        Some((old, old_meta)) => {
            t.opening = opening || old.opening;
            if old.status == TxnStatus::Voided {
                return Err(CommandError::Rejected(
                    "a voided transaction cannot be changed; unvoid it first".into(),
                ));
            }
            t.number = old.number.clone();
            mk_ledger::inherit_reconcile(&old.splits, &mut t.splits);
            validate_change(ctx, Some(old), Some(&t))?;
            ctx.save_txn(Some(old), &t)?;
            replay.extend(accounts_of(ctx, old_meta, old)?);
        }
        None => {
            t.opening = opening;
            validate_change(ctx, None, Some(&t))?;
            ctx.save_txn(None, &t)?;
        }
    }
    save_meta(ctx, &p.meta)?;
    replay.sort();
    replay.dedup();
    for acc in &replay {
        sync(ctx, *acc)?;
    }
    respond(ctx, t.id)
}

/// The security accounts an investment transaction's lots depend on.
pub(crate) fn accounts_of(
    ctx: &Ctx<'_>,
    meta: &InvMeta,
    txn: &Transaction,
) -> CommandResult<Vec<AccountId>> {
    let mut out: Vec<AccountId> = meta
        .sec_account
        .into_iter()
        .chain(meta.sec_account2)
        .collect();
    if meta.action == Action::Split {
        if let Some(sec) = &meta.security {
            let s = securities(ctx)?.into_iter().find(|s| s.id == *sec);
            if let Some(s) = s {
                let chart = Chart::load(ctx)?;
                out.extend(
                    chart
                        .accounts
                        .iter()
                        .filter(|a| {
                            a.subtype == AccountSubtype::Security
                                && a.commodity.as_str() == s.symbol
                        })
                        .map(|a| a.id),
                );
            }
        }
        out.extend(txn.splits.iter().map(|s| s.account));
    }
    out.sort();
    out.dedup();
    Ok(out)
}

pub(crate) fn respond(ctx: &Ctx<'_>, id: TransactionId) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let t = ctx
        .tx
        .transaction(id)?
        .ok_or_else(|| CommandError::Integrity("posted transaction missing".into()))?;
    let meta = meta_of(ctx, id)?;
    let mut out = json!({
        "transaction": txn_json(&chart, &t)?,
        "investment": meta.as_ref().map(meta_json),
    });
    if let (Some(m), Some(o)) = (&meta, out.as_object_mut())
        && matches!(m.action, Action::Sell | Action::SharesOut)
        && let Some(acc) = m.sec_account
    {
        let r = replay(ctx, acc, None)?;
        if let Some(d) = r.disposals.iter().find(|d| d.txn == id) {
            o.insert("disposal".into(), disposal_json(ctx, &d.disposal, &chart)?);
        }
    }
    // Options, bonds and crypto: the disposal (in either security account) and the
    // realised gain.
    if let (Some(m), Some(o)) = (&meta, out.as_object_mut())
        && m.action.is_extra()
    {
        for acc in m.sec_account.into_iter().chain(m.sec_account2) {
            let r = replay(ctx, acc, None)?;
            let found: Vec<&Found> = r.disposals.iter().filter(|d| d.txn == id).collect();
            if let Some(d) = crate::inv_kinds::merged(&found) {
                o.insert("disposal".into(), disposal_json(ctx, &d, &chart)?);
                break;
            }
        }
    }
    Ok(out)
}

pub(crate) fn meta_json(m: &InvMeta) -> Value {
    json!({
        "txn": m.txn,
        "action": m.action,
        "label": m.action.label(),
        "account": m.account,
        "security": m.security,
        "sec_account": m.sec_account,
        "date": m.date,
        "quantity": m.quantity.map(|q| format_amount(q, None)),
        "price": m.price.map(price_text),
        "commission": format_amount(m.commission, Some(2)),
        "amount": m.amount.map(|v| format_amount(v, Some(2))),
        "amount_given": m.amount_given,
        "lots": m.lots,
        "method": m.method,
        "kind": m.kind,
        "other_account": m.other_account,
        "ratio_new": m.ratio_new.map(|v| format_amount(v, None)),
        "ratio_old": m.ratio_old.map(|v| format_amount(v, None)),
        "args": m.args,
    })
}

pub(crate) fn disposal_json(ctx: &Ctx<'_>, d: &Disposal, _chart: &Chart) -> CommandResult<Value> {
    let _ = ctx;
    let mut short = Decimal::ZERO;
    let mut long = Decimal::ZERO;
    let mut proceeds = Decimal::ZERO;
    let portions: Vec<Value> = d
        .portions
        .iter()
        .map(|p| {
            let g = p.gain();
            if p.long_term {
                long = long.checked_add(g).unwrap_or(long);
            } else {
                short = short.checked_add(g).unwrap_or(short);
            }
            proceeds = proceeds.checked_add(p.proceeds).unwrap_or(proceeds);
            json!({
                "lot": p.lot,
                "acquired": if d.average { json!("Various") } else { json!(p.acquired) },
                "lot_acquired": p.acquired,
                "quantity": format_amount(p.quantity, None),
                "basis": format_amount(p.basis, Some(2)),
                "proceeds": format_amount(p.proceeds, Some(2)),
                "gain": format_amount(g, Some(2)),
                "term": if p.long_term { "long" } else { "short" },
            })
        })
        .collect();
    Ok(json!({
        "basis": format_amount(d.basis, Some(2)),
        "proceeds": format_amount(proceeds, Some(2)),
        "gain": format_amount(proceeds.checked_sub(d.basis).unwrap_or_default(), Some(2)),
        "short_term_gain": format_amount(short, Some(2)),
        "long_term_gain": format_amount(long, Some(2)),
        "average": d.average,
        "portions": portions,
    }))
}

fn buy(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    post(ctx, Action::Buy, a, None)
}
fn sell(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    post(ctx, Action::Sell, a, None)
}
fn dividend(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    post(ctx, Action::Dividend, a, None)
}
fn reinvest(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    post(ctx, Action::Reinvest, a, None)
}
fn interest(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    post(ctx, Action::Interest, a, None)
}
fn fee(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    post(ctx, Action::Fee, a, None)
}
fn shares_in(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    post(ctx, Action::SharesIn, a, None)
}
fn shares_out(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    post(ctx, Action::SharesOut, a, None)
}
fn split(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    post(ctx, Action::Split, a, None)
}
fn return_of_capital(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    post(ctx, Action::ReturnOfCapital, a, None)
}

/// A sale previewed: run in a read transaction, which is always rolled back.
fn preview_sell(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    let out = post(ctx, Action::Sell, a, None)?;
    Ok(out.get("disposal").cloned().unwrap_or(Value::Null))
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PreviewArgs {
    action: Action,
    /// Preview an edit of this investment transaction instead of a new one.
    #[serde(default)]
    txn: Option<TransactionId>,
    /// The action arguments (as for the action's own command).
    #[serde(flatten)]
    rest: Map<String, Value>,
}

/// Run the action in a read transaction (always rolled back) and return what it
/// would post: the transaction, its metadata (with the computed amount) and, for a
/// sale, the consumed lots.
fn preview(ctx: &Ctx<'_>, a: PreviewArgs) -> CommandResult<Value> {
    let args: ActionArgs = crate::parse_args(Value::Object(a.rest))?;
    let existing = match a.txn {
        Some(id) => {
            let t = ctx
                .tx
                .transaction(id)?
                .ok_or_else(|| CommandError::NotFound(format!("transaction {id}")))?;
            let m = meta_of(ctx, id)?
                .ok_or_else(|| invalid("the transaction is not an investment action"))?;
            Some((t, m))
        }
        None => None,
    };
    post(ctx, a.action, args, existing)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ActivityArgs {
    /// Brokerage account (id, full name or code).
    account: String,
}

fn activity(ctx: &Ctx<'_>, a: ActivityArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let b = brokerage(&chart, Some(&a.account))?.id;
    let ids = chart.subtree(b);
    let txns = ctx.tx.transactions(&mk_store::TxnFilter {
        accounts: Some(ids.clone()),
        from: None,
        to: None,
        include_voided: true,
        limit: None,
        offset: 0,
    })?;
    let metas = all_metas(ctx)?;
    let secs = securities(ctx)?;
    let mut balance = Decimal::ZERO;
    let mut rows = Vec::new();
    for t in &txns {
        let posted = t.status == TxnStatus::Posted;
        let cash = value_in(t, b);
        if posted {
            balance = plus(balance, cash)?;
        }
        let m = metas.get(&t.id);
        let symbol = m
            .and_then(|m| m.security.as_ref())
            .and_then(|id| secs.iter().find(|s| s.id == *id))
            .map(|s| s.symbol.clone());
        let other = if m.is_none() {
            t.splits
                .iter()
                .find(|s| !ids.contains(&s.account))
                .map(|s| chart.full_name(s.account))
        } else {
            None
        };
        // Shares moved in this account (a split shows the shares it added here).
        let shares = t
            .splits
            .iter()
            .filter(|s| {
                s.account != b
                    && ids.contains(&s.account)
                    && chart
                        .get(s.account)
                        .is_some_and(|x| x.subtype == AccountSubtype::Security)
            })
            .fold(Decimal::ZERO, |acc, s| {
                acc.checked_add(s.quantity).unwrap_or(acc)
            });
        rows.push(json!({
            "txn": t.id,
            "date": t.date,
            "action": m.map(|m| m.action),
            "label": m.map_or("Transfer", |m| m.action.label()),
            "symbol": symbol,
            "shares": (!shares.is_zero()).then(|| format_amount(shares, None)),
            "price": m.and_then(|m| m.price).map(price_text),
            "commission": m.filter(|m| !m.commission.is_zero()).map(|m| format_amount(m.commission, Some(2))),
            "cash": format_amount(cash, Some(2)),
            "balance": format_amount(balance, Some(2)),
            "other": other,
            "payee": t.payee,
            "memo": t.memo,
            "voided": !posted,
            "reconciled": t.splits.iter().any(|s| s.reconcile == Reconcile::Reconciled),
        }));
    }
    Ok(json!({"account": b, "full_name": chart.full_name(b), "rows": rows}))
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UpdateArgs {
    /// The investment transaction to replace.
    txn: TransactionId,
    /// The action (default: the transaction's current action).
    #[serde(default)]
    action: Option<Action>,
    /// The new action arguments (as for the action's own command).
    #[serde(flatten)]
    rest: Map<String, Value>,
}

fn update(ctx: &Ctx<'_>, a: UpdateArgs) -> CommandResult<Value> {
    let old = ctx
        .tx
        .transaction(a.txn)?
        .ok_or_else(|| CommandError::NotFound(format!("transaction {}", a.txn)))?;
    let meta = meta_of(ctx, a.txn)?
        .ok_or_else(|| invalid("the transaction is not an investment action"))?;
    let args: ActionArgs = crate::parse_args(Value::Object(a.rest))?;
    let action = a.action.unwrap_or(meta.action);
    post(ctx, action, args, Some((old, meta)))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TxnRef {
    txn: TransactionId,
}

fn inv_get(ctx: &Ctx<'_>, a: TxnRef) -> CommandResult<Value> {
    let m = meta_of(ctx, a.txn)?
        .ok_or_else(|| CommandError::NotFound(format!("investment transaction {}", a.txn)))?;
    Ok(meta_json(&m))
}

// ----- replay ----------------------------------------------------------------------

/// A disposal found by a replay.
#[derive(Debug, Clone)]
pub(crate) struct Found {
    pub txn: TransactionId,
    pub date: Date,
    pub action: Action,
    pub disposal: Disposal,
}

/// A lot a merger or spin-off creates in the new security's account.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NewLot {
    pub acquired: Date,
    pub quantity: Decimal,
    pub basis: Decimal,
}

/// The figures of one merger or spin-off in one account.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CorpFigures {
    /// Shares leaving the old security (mergers).
    pub old_quantity: Decimal,
    /// Basis leaving the old (or parent) security.
    pub old_basis: Decimal,
    pub new_lots: Vec<NewLot>,
    pub new_quantity: Decimal,
    pub new_basis: Decimal,
    /// Cash received (merger boot).
    pub cash: Decimal,
    /// Recognised gain (merger with boot or taxable).
    pub gain: Decimal,
}

/// The result of replaying one security account.
#[derive(Debug, Clone, Default)]
pub(crate) struct Replay {
    pub state: LotState,
    pub disposals: Vec<Found>,
    /// Split transaction → shares added in this account.
    pub split_deltas: Vec<(TransactionId, Decimal)>,
    /// Mergers and spin-offs replayed in their old (parent) account.
    pub corporate: Vec<(TransactionId, CorpFigures)>,
    /// Returns of capital: transaction → basis taken (excess gains are disposals).
    pub roc: Vec<(TransactionId, Decimal)>,
    /// Short lots of a written option (basis = premium received).
    pub short: LotState,
    /// Options, bonds and crypto: the posted figures of this account's split (and the
    /// realised gain, when the transaction has one) as the replay worked them out.
    pub fixes: Vec<(TransactionId, crate::inv_kinds::Fix)>,
    /// Lot portions handed to the transaction's other security account (exercise,
    /// assignment, transfer of lots).
    pub handoff: Vec<(TransactionId, Vec<lots::Portion>)>,
}

/// The investment transactions touching a security account, in register order.
pub(crate) fn events_of(
    ctx: &Ctx<'_>,
    account: AccountId,
) -> CommandResult<Vec<(Transaction, InvMeta)>> {
    let chart = Chart::load(ctx)?;
    let acc = chart
        .get(account)
        .ok_or_else(|| CommandError::NotFound("security account".into()))?;
    let symbol = acc.commodity.to_string();
    let Some(sec) = securities(ctx)?.into_iter().find(|s| s.symbol == symbol) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for d in ctx.tx.docs(DocTable::InvTxn, None)? {
        let m: InvMeta = serde_json::from_value(d.json).map_err(bad_doc)?;
        let mine = m.sec_account == Some(account)
            || m.sec_account2 == Some(account)
            || (m.action == Action::Split && m.security.as_deref() == Some(sec.id.as_str()));
        if !mine {
            continue;
        }
        let Some(t) = ctx.tx.transaction(m.txn)? else {
            continue;
        };
        if t.status == TxnStatus::Voided {
            continue;
        }
        out.push((t, m));
    }
    out.sort_by(|(a, _), (b, _)| {
        (a.date, &a.entered_at, a.id.to_string()).cmp(&(b.date, &b.entered_at, b.id.to_string()))
    });
    Ok(out)
}

pub(crate) fn value_in(t: &Transaction, account: AccountId) -> Decimal {
    t.splits
        .iter()
        .filter(|s| s.account == account)
        .fold(Decimal::ZERO, |a, s| a.checked_add(s.value).unwrap_or(a))
}

pub(crate) fn selections(m: &InvMeta) -> Option<Vec<Selection>> {
    let lots = m.lots.as_ref()?;
    if lots.is_empty() {
        return None;
    }
    Some(
        lots.iter()
            .map(|l| Selection {
                lot: l.lot.clone().unwrap_or_default(),
                quantity: l.quantity,
            })
            .collect(),
    )
}

fn ratio(m: &InvMeta) -> CommandResult<(Decimal, Decimal)> {
    Ok((
        m.ratio_new.unwrap_or(Decimal::ONE),
        m.ratio_old.unwrap_or(Decimal::ONE),
    ))
}

/// A merger in its old account: every open lot becomes new shares (and maybe cash).
/// The portions' proceeds are set so their gain is the recognised gain.
fn merger_figures(m: &InvMeta, d: &mut Disposal) -> CommandResult<CorpFigures> {
    let terms = m
        .corp
        .clone()
        .ok_or_else(|| CommandError::Integrity("merger without terms".into()))?;
    let (rn, ro) = ratio(m)?;
    let cps = terms.cash_per_share.unwrap_or_default();
    let nsv = terms.new_share_value.unwrap_or_default();
    let taxable = terms.mode.as_deref() == Some("taxable");
    let mut f = CorpFigures::default();
    for p in d.portions.iter_mut() {
        let new_q = lots::round(
            mul(p.quantity, rn)?
                .checked_div(ro)
                .ok_or_else(|| invalid("bad merger ratio"))?,
            lots::QUANTITY_DP,
        );
        let cash = round2(mul(p.quantity, cps)?);
        let value_new = round2(mul(new_q, nsv)?);
        let (proceeds, new_basis, acquired) = if taxable {
            let proceeds = plus(cash, value_new)?;
            (proceeds, value_new, m.date)
        } else if cash.is_zero() {
            (p.basis, p.basis, p.acquired)
        } else {
            let realized = minus(plus(cash, value_new)?, p.basis)?;
            let recognized = realized.min(cash).max(Decimal::ZERO);
            (
                plus(p.basis, recognized)?,
                plus(minus(p.basis, cash)?, recognized)?,
                p.acquired,
            )
        };
        p.proceeds = proceeds;
        f.old_quantity = plus(f.old_quantity, p.quantity)?;
        f.old_basis = plus(f.old_basis, p.basis)?;
        f.cash = plus(f.cash, cash)?;
        f.gain = plus(f.gain, minus(proceeds, p.basis)?)?;
        f.new_quantity = plus(f.new_quantity, new_q)?;
        f.new_basis = plus(f.new_basis, new_basis)?;
        if !new_q.is_zero() {
            f.new_lots.push(NewLot {
                acquired,
                quantity: new_q,
                basis: new_basis,
            });
        }
    }
    Ok(f)
}

/// A spin-off in its parent account: the moved basis becomes new lots.
fn spinoff_figures(m: &InvMeta, moved: &[lots::Portion]) -> CommandResult<CorpFigures> {
    let terms = m
        .corp
        .clone()
        .ok_or_else(|| CommandError::Integrity("spin-off without terms".into()))?;
    let per = terms.new_per_parent.unwrap_or_default();
    let mut f = CorpFigures::default();
    for p in moved {
        let q = lots::round(mul(p.quantity, per)?, lots::QUANTITY_DP);
        f.old_basis = plus(f.old_basis, p.basis)?;
        f.new_quantity = plus(f.new_quantity, q)?;
        f.new_basis = plus(f.new_basis, p.basis)?;
        if !q.is_zero() {
            f.new_lots.push(NewLot {
                acquired: p.acquired,
                quantity: q,
                basis: p.basis,
            });
        }
    }
    Ok(f)
}

/// Replay a security account's investment transactions (those dated on or before
/// `until`, when given).
pub(crate) fn replay(
    ctx: &Ctx<'_>,
    account: AccountId,
    until: Option<Date>,
) -> CommandResult<Replay> {
    replay_to(ctx, account, until, None)
}

/// Replay, stopping after transaction `stop_after` when given.
pub(crate) fn replay_to(
    ctx: &Ctx<'_>,
    account: AccountId,
    until: Option<Date>,
    stop_after: Option<TransactionId>,
) -> CommandResult<Replay> {
    let mut r = Replay::default();
    for (t, m) in events_of(ctx, account)? {
        if until.is_some_and(|u| t.date > u) {
            break;
        }
        let in_new = m.sec_account2 == Some(account) && m.sec_account != Some(account);
        match m.action {
            Action::Merger | Action::Spinoff if in_new => {
                // The new security's lots come from the old account's lots then.
                let old = m.sec_account.ok_or_else(|| {
                    CommandError::Integrity("corporate action without its account".into())
                })?;
                let src = replay_to(ctx, old, None, Some(t.id))?;
                let f = src
                    .corporate
                    .iter()
                    .find(|(id, _)| *id == t.id)
                    .map(|(_, f)| f.clone())
                    .unwrap_or_default();
                for (i, l) in f.new_lots.iter().enumerate() {
                    r.state
                        .apply(&Event::Acquire {
                            lot: format!("{}:n{}", t.id, i + 1),
                            acquired: l.acquired,
                            quantity: l.quantity,
                            basis: l.basis,
                            txn: t.id.to_string(),
                        })
                        .map_err(|err| lot_error(err, t.date))?;
                }
            }
            Action::Merger => {
                let sel: Vec<Selection> = r
                    .state
                    .open()
                    .iter()
                    .map(|l| Selection {
                        lot: l.id.clone(),
                        quantity: l.remaining,
                    })
                    .collect();
                let held = r.state.held();
                if !held.is_zero() {
                    let out = r
                        .state
                        .apply(&Event::Dispose {
                            date: t.date,
                            quantity: held,
                            proceeds: None,
                            method: lots::Method::Specific,
                            selections: Some(sel),
                        })
                        .map_err(|err| lot_error(err, t.date))?;
                    if let Outcome::Disposal(mut d) = out {
                        let f = merger_figures(&m, &mut d)?;
                        r.corporate.push((t.id, f));
                        r.disposals.push(Found {
                            txn: t.id,
                            date: t.date,
                            action: m.action,
                            disposal: d,
                        });
                    }
                } else {
                    r.corporate.push((t.id, CorpFigures::default()));
                }
            }
            Action::Spinoff => {
                let pct = m
                    .corp
                    .as_ref()
                    .and_then(|c| c.allocation_percent)
                    .unwrap_or_default();
                let out = r
                    .state
                    .apply(&Event::BasisMove { percent: pct })
                    .map_err(|err| lot_error(err, t.date))?;
                if let Outcome::Moved(moved) = out {
                    r.corporate.push((t.id, spinoff_figures(&m, &moved)?));
                }
            }
            x if x.is_extra() || crate::inv_kinds::is_short_in(&m) => {
                crate::inv_kinds::replay_event(ctx, &mut r, account, &t, &m)?;
            }
            _ => {
                let ev = match m.action {
                    Action::Buy | Action::Reinvest => vec![Event::Acquire {
                        lot: t.id.to_string(),
                        acquired: t.date,
                        quantity: m.quantity.unwrap_or_default(),
                        basis: value_in(&t, account),
                        txn: t.id.to_string(),
                    }],
                    Action::SharesIn => m
                        .lots
                        .clone()
                        .unwrap_or_default()
                        .iter()
                        .enumerate()
                        .map(|(i, l)| Event::Acquire {
                            lot: format!("{}:{}", t.id, i + 1),
                            acquired: l.acquired.unwrap_or(t.date),
                            quantity: l.quantity,
                            basis: l.basis.unwrap_or_default(),
                            txn: t.id.to_string(),
                        })
                        .collect(),
                    Action::Sell | Action::SharesOut | Action::CashInLieu => vec![Event::Dispose {
                        date: t.date,
                        quantity: m.quantity.unwrap_or_default(),
                        proceeds: (m.action != Action::SharesOut)
                            .then(|| m.amount.unwrap_or_default()),
                        method: m.method.unwrap_or(BasisMethod::Fifo).engine(),
                        selections: selections(&m),
                    }],
                    Action::Split => vec![Event::Split {
                        new: m.ratio_new.unwrap_or(Decimal::ONE),
                        old: m.ratio_old.unwrap_or(Decimal::ONE),
                    }],
                    Action::ReturnOfCapital => vec![Event::ReturnOfCapital {
                        amount: m.amount.unwrap_or_default(),
                        date: t.date,
                    }],
                    _ => Vec::new(),
                };
                for e in ev {
                    match r.state.apply(&e).map_err(|err| lot_error(err, t.date))? {
                        Outcome::Disposal(d) => {
                            if m.action == Action::ReturnOfCapital {
                                r.roc.push((t.id, d.basis));
                            }
                            r.disposals.push(Found {
                                txn: t.id,
                                date: t.date,
                                action: m.action,
                                disposal: d,
                            });
                        }
                        Outcome::SplitDelta(q) => r.split_deltas.push((t.id, q)),
                        Outcome::Moved(_) => {}
                        Outcome::None => {
                            if m.action == Action::ReturnOfCapital {
                                r.roc.push((t.id, m.amount.unwrap_or_default()));
                            }
                        }
                    }
                }
            }
        }
        if stop_after == Some(t.id) {
            break;
        }
    }
    Ok(r)
}

/// Replay a security account and rewrite every disposal or split whose posted
/// amounts no longer match. A rewrite in the closed period or of a transaction with a
/// reconciled split is refused (`would_change_closed`).
pub(crate) fn sync(ctx: &Ctx<'_>, account: AccountId) -> CommandResult<()> {
    let r = replay(ctx, account, None)?;
    let metas = all_metas(ctx)?;
    let closing = ctx.tx.closing_date()?;
    let rewrite = |old: &Transaction, new: Transaction| -> CommandResult<()> {
        if &new == old {
            return Ok(());
        }
        let reconciled = old
            .splits
            .iter()
            .any(|s| s.reconcile == Reconcile::Reconciled);
        if closing.is_some_and(|c| old.date <= c) || reconciled {
            return Err(coded(
                "would_change_closed",
                format!(
                    "this would change the basis of the {} transaction of {}, which is {}",
                    metas
                        .get(&old.id)
                        .map_or("investment", |m| m.action.label()),
                    old.date,
                    if reconciled {
                        "reconciled"
                    } else {
                        "in the closed period"
                    }
                ),
            ));
        }
        validate_change(ctx, Some(old), Some(&new))?;
        ctx.save_txn(Some(old), &new)
    };
    for f in &r.disposals {
        if matches!(f.action, Action::Merger | Action::ReturnOfCapital) || f.action.is_extra() {
            continue;
        }
        let Some(m) = metas.get(&f.txn) else { continue };
        let Some(old) = ctx.tx.transaction(f.txn)? else {
            continue;
        };
        let mut new = old.clone();
        let basis = f.disposal.basis;
        for s in new.splits.iter_mut() {
            if s.account == account {
                s.value = canonical_amount(neg(basis), 2);
            } else if Some(s.account) == m.other_account {
                let v = match f.action {
                    // Realized Gains: −(proceeds − basis).
                    Action::Sell | Action::CashInLieu => {
                        minus(basis, m.amount.unwrap_or_default())?
                    }
                    // Opening Balances: +basis.
                    _ => basis,
                };
                s.value = canonical_amount(v, 2);
                s.quantity = s.value;
            }
        }
        mk_ledger::sort_splits(&mut new.splits);
        rewrite(&old, new)?;
    }
    for (txn, delta) in &r.split_deltas {
        let Some(old) = ctx.tx.transaction(*txn)? else {
            continue;
        };
        let mut new = old.clone();
        match new.splits.iter_mut().find(|s| s.account == account) {
            Some(s) => s.quantity = canonical_amount(*delta, 0),
            None if !delta.is_zero() => {
                new.splits.push(split_of(account, Decimal::ZERO, *delta));
                mk_ledger::sort_splits(&mut new.splits);
            }
            None => {}
        }
        rewrite(&old, new)?;
    }
    // Mergers and spin-offs: the old (parent) account, the new account, cash, gains.
    let mut downstream: Vec<AccountId> = Vec::new();
    for (txn, f) in &r.corporate {
        let Some(m) = metas.get(txn) else { continue };
        if m.sec_account != Some(account) {
            continue;
        }
        let Some(old) = ctx.tx.transaction(*txn)? else {
            continue;
        };
        let mut new = old.clone();
        let new_acc = m.sec_account2;
        let merger = m.action == Action::Merger;
        let mut set = |acc: AccountId, value: Decimal, quantity: Decimal| {
            let value = canonical_amount(value, 2);
            match new.splits.iter_mut().find(|s| s.account == acc) {
                Some(s) => {
                    s.value = value;
                    s.quantity = quantity;
                }
                None => new.splits.push(split_of(acc, value, quantity)),
            }
        };
        let q_out = if merger {
            canonical_amount(neg(f.old_quantity), 0)
        } else {
            Decimal::ZERO
        };
        set(account, neg(f.old_basis), q_out);
        if let Some(n) = new_acc {
            set(n, f.new_basis, canonical_amount(f.new_quantity, 0));
            downstream.push(n);
        }
        if merger {
            if let Some(cash) = m.account {
                set(cash, f.cash, canonical_amount(f.cash, 2));
            }
            if let Some(g) = m.other_account {
                let v = neg(f.gain);
                set(g, v, canonical_amount(v, 2));
            }
        }
        mk_ledger::sort_splits(&mut new.splits);
        rewrite(&old, new)?;
    }
    // Returns of capital: basis taken from the security, the excess to Realized Gains.
    for (txn, taken) in &r.roc {
        let Some(m) = metas.get(txn) else { continue };
        let Some(old) = ctx.tx.transaction(*txn)? else {
            continue;
        };
        let excess = minus(m.amount.unwrap_or_default(), *taken)?;
        let mut new = old.clone();
        for s in new.splits.iter_mut().filter(|s| s.account == account) {
            s.value = canonical_amount(neg(*taken), 2);
        }
        if let Some(g) = m.other_account {
            if excess > Decimal::ZERO {
                let v = canonical_amount(neg(excess), 2);
                match new.splits.iter_mut().find(|s| s.account == g) {
                    Some(s) => {
                        s.value = v;
                        s.quantity = v;
                    }
                    None => new.splits.push(cash_split(g, v)),
                }
            } else {
                new.splits.retain(|s| s.account != g);
            }
        }
        mk_ledger::sort_splits(&mut new.splits);
        rewrite(&old, new)?;
    }
    // Options, bonds and crypto: this account's split and the realised gain.
    for (txn, fix) in &r.fixes {
        let Some(m) = metas.get(txn) else { continue };
        let Some(old) = ctx.tx.transaction(*txn)? else {
            continue;
        };
        // A hand-over between two security accounts (exercise, assignment, transfer
        // of lots) changes both sides at once, so the transaction stays balanced.
        let mut all = vec![(account, fix.clone())];
        let other = if m.sec_account == Some(account) {
            m.sec_account2
        } else {
            m.sec_account
        };
        if crate::inv_kinds::hands_over(m.action)
            && let Some(o) = other.filter(|o| *o != account)
        {
            let ro = replay(ctx, o, None)?;
            all.extend(
                ro.fixes
                    .iter()
                    .filter(|(id, _)| id == txn)
                    .map(|(_, f)| (o, f.clone())),
            );
        }
        let mut new = old.clone();
        for (acc, f) in &all {
            for sp in new.splits.iter_mut().filter(|sp| sp.account == *acc) {
                sp.value = canonical_amount(f.value, 2);
            }
        }
        let gain = all.iter().find_map(|(_, f)| f.gain);
        if let (Some(g), Some(gain)) = (m.other_account, gain) {
            let v = canonical_amount(neg(gain), 2);
            if v.is_zero() {
                if new.splits.len() > 2 {
                    new.splits.retain(|sp| sp.account != g);
                } else if let Some(sp) = new.splits.iter_mut().find(|sp| sp.account == g) {
                    sp.value = v;
                    sp.quantity = v;
                }
            } else {
                match new.splits.iter_mut().find(|sp| sp.account == g) {
                    Some(sp) => {
                        sp.value = v;
                        sp.quantity = v;
                    }
                    None => new.splits.push(cash_split(g, v)),
                }
            }
        }
        mk_ledger::sort_splits(&mut new.splits);
        rewrite(&old, new)?;
    }
    for (txn, _) in &r.handoff {
        if let Some(m) = metas.get(txn)
            && m.sec_account == Some(account)
            && let Some(n) = m.sec_account2
        {
            downstream.push(n);
        }
    }
    downstream.sort();
    downstream.dedup();
    for n in downstream {
        if n != account {
            sync(ctx, n)?;
        }
    }
    Ok(())
}

// ----- hooks for plain transaction commands ----------------------------------------

/// `txn.update` may change payee, number and memos of an investment transaction,
/// but not its date or amounts (`use_investment_action`).
pub(crate) fn check_plain_update(
    ctx: &Ctx<'_>,
    old: &Transaction,
    new: &Transaction,
) -> CommandResult<()> {
    if meta_of(ctx, old.id)?.is_none() {
        return Ok(());
    }
    let key = |t: &Transaction| {
        let mut v: Vec<(AccountId, Decimal, Decimal)> = t
            .splits
            .iter()
            .map(|s| (s.account, s.value.normalize(), s.quantity.normalize()))
            .collect();
        v.sort();
        v
    };
    if old.date != new.date || key(old) != key(new) || old.currency != new.currency {
        return Err(coded(
            "use_investment_action",
            "this is an investment transaction: change its date, shares or amounts with \
             inv.update (payee and memo edits are allowed)",
        ));
    }
    Ok(())
}

/// Refuse plain commands that would bypass the lot records (`txn.reverse`).
pub(crate) fn refuse_plain(ctx: &Ctx<'_>, id: TransactionId) -> CommandResult<()> {
    if meta_of(ctx, id)?.is_some() {
        return Err(coded(
            "use_investment_action",
            "this is an investment transaction: use the investment actions instead",
        ));
    }
    Ok(())
}

/// After a void, unvoid or delete of an investment transaction: drop its record on
/// delete, and replay the security accounts it affected.
pub(crate) fn after_plain_change(
    ctx: &Ctx<'_>,
    old: &Transaction,
    deleted: bool,
) -> CommandResult<()> {
    let Some(meta) = meta_of(ctx, old.id)? else {
        return Ok(());
    };
    let accounts = accounts_of(ctx, &meta, old)?;
    if deleted {
        ctx.save_doc(DocTable::InvTxn, &old.id.to_string(), None)?;
    }
    for a in accounts {
        sync(ctx, a)?;
    }
    Ok(())
}

// ----- valuation -------------------------------------------------------------------

/// Market value of each security account as of a date: shares × latest price on or
/// before it (rounded to cents), or the cost basis with `priced = false` when there is
/// no price. A CD without a price is valued at face (price 100) with `at_face` set
/// (import-investments.md revision 4).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Valued {
    pub quantity: Decimal,
    pub basis: Decimal,
    pub price: Option<(Date, Decimal)>,
    pub value: Decimal,
    pub at_face: bool,
}

impl Valued {
    /// Whether the value is a market value (priced, or a CD at face).
    pub(crate) fn valued(&self) -> bool {
        self.price.is_some() || self.at_face
    }
}

pub(crate) fn valuations(
    ctx: &Ctx<'_>,
    chart: &Chart,
    as_of: Date,
) -> CommandResult<HashMap<AccountId, Valued>> {
    let mut qty: HashMap<AccountId, (Decimal, Decimal)> = HashMap::new();
    for row in ctx.tx.posted_splits(Some(as_of))? {
        if chart
            .get(row.account)
            .is_some_and(|a| a.subtype == AccountSubtype::Security)
        {
            let slot = qty.entry(row.account).or_default();
            slot.0 = slot.0.checked_add(row.quantity).unwrap_or(slot.0);
            slot.1 = slot.1.checked_add(row.value).unwrap_or(slot.1);
        }
    }
    let secs = securities(ctx)?;
    let mut prices: HashMap<String, BTreeMap<Date, Decimal>> = HashMap::new();
    let kinds: HashMap<String, SecurityKind> =
        secs.iter().map(|s| (s.symbol.clone(), s.kind)).collect();
    let by_symbol: HashMap<String, &Security> =
        secs.iter().map(|s| (s.symbol.clone(), s)).collect();
    let mut out = HashMap::new();
    for (id, (q, b)) in qty {
        let Some(acc) = chart.get(id) else { continue };
        let sym = acc.commodity.to_string();
        let price = match secs.iter().find(|s| s.symbol == sym) {
            Some(s) => {
                if !prices.contains_key(&s.id) {
                    prices.insert(s.id.clone(), prices_of(ctx, &s.id)?);
                }
                prices
                    .get(&s.id)
                    .and_then(|m| m.range(..=as_of).next_back().map(|(d, p)| (*d, *p)))
            }
            None => None,
        };
        let kind = kinds.get(&sym).copied().unwrap_or(SecurityKind::Stock);
        let at_face = price.is_none() && kind == SecurityKind::Cd;
        let worth = |p: Decimal| match by_symbol.get(&sym) {
            Some(s) => s.worth(q, p),
            None => value_of(kind, q, p),
        };
        let value = match price {
            Some((_, p)) => worth(p)?,
            None if at_face => worth(Decimal::ONE_HUNDRED)?,
            None => b,
        };
        out.insert(
            id,
            Valued {
                quantity: q,
                basis: b,
                price,
                value,
                at_face,
            },
        );
    }
    Ok(out)
}

pub(crate) fn is_enabled(ctx: &Ctx<'_>) -> CommandResult<bool> {
    crate::is_module_enabled(ctx.tx, INVESTMENTS)
}

/// `security.update` / `security.delete` from JSON arguments (definition installs).
pub(crate) fn security_update_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    security_update(ctx, crate::parse_args(args)?)
}

pub(crate) fn security_delete_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    security_delete(ctx, crate::parse_args(args)?)
}
