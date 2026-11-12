//! Options, bonds and crypto in investment accounts (spec:
//! investments-options-bonds-crypto.md): the security terms, the option, bond and
//! crypto actions, short lots of written options, coupon reminders and the Attention
//! items for expired options and matured bonds.
//!
//! The actions post through [`crate::investments::post`] like the core actions and
//! are replayed by the same lot engine: [`build`] makes the posting, [`replay_event`]
//! feeds the lot state (long lots in `Replay::state`, short lots in `Replay::short`)
//! and records the figures the sync writes back ([`Fix`]).

use mk_core::currency::format_amount;
use mk_core::schedule::MonthDay;
use mk_core::{
    AccountId, AccountSubtype, AccountType, Date, Decimal, Transaction, TransactionId, TxnStatus,
};
use mk_ledger::lots::{self, Disposal, Event, Outcome, Portion};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::investments::{
    Action, ActionArgs, BasisMethod, Found, INVESTMENTS, InvMeta, Posting, Replay, Security,
    SecurityCreateArgs, SecurityKind, brokerage, cash_split, describe, ensure_sec_account,
    find_security, held_on, invalid, latest_price, lot_error, minus, money_amount, mul, need, neg,
    other_account, plus, positive, quantity_arg, round2, sec_account_of, securities, selections,
    split_of, value_in,
};
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    let m = INVESTMENTS;
    r.add_in(
        m,
        "inv.option_buy",
        "Buy option contracts to open (closes written contracts first)",
        true,
        option_buy,
    );
    r.add_in(
        m,
        "inv.option_sell",
        "Sell option contracts to close",
        true,
        option_sell,
    );
    r.add_in(
        m,
        "inv.option_write",
        "Write (sell to open) option contracts: a short position",
        true,
        option_write,
    );
    r.add_in(
        m,
        "inv.option_cover",
        "Buy written option contracts back (buy to close)",
        true,
        option_cover,
    );
    r.add_in(
        m,
        "inv.option_expire",
        "Record option contracts that expired",
        true,
        option_expire,
    );
    r.add_in(
        m,
        "inv.option_exercise",
        "Exercise held option contracts (shares bought or sold at the strike)",
        true,
        option_exercise,
    );
    r.add_in(
        m,
        "inv.option_assigned",
        "Record written option contracts assigned (shares sold or bought at the strike)",
        true,
        option_assigned,
    );
    r.add_in(
        m,
        "inv.bond_buy",
        "Buy bonds (accrued interest paid offsets the interest account)",
        true,
        bond_buy,
    );
    r.add_in(m, "inv.bond_sell", "Sell bonds", true, bond_sell);
    r.add_in(
        m,
        "inv.bond_interest",
        "Record a bond coupon received",
        true,
        bond_interest,
    );
    r.add_in(
        m,
        "inv.bond_redeem",
        "Record bonds redeemed at maturity or called",
        true,
        bond_redeem,
    );
    r.add_in(
        m,
        "inv.bond_reminders",
        "Add coupon reminders: a scheduled reminder for each payment date to maturity",
        true,
        bond_reminders,
    );
    r.add_in(
        m,
        "inv.crypto_fee",
        "Record a fee paid in coin (a disposal at market value)",
        true,
        crypto_fee,
    );
    r.add_in(
        m,
        "inv.crypto_swap",
        "Swap one coin for another in the same account",
        true,
        crypto_swap,
    );
    r.add_in(
        m,
        "inv.crypto_reward",
        "Record staking or other rewards received in coin",
        true,
        crypto_reward,
    );
    r.add_in(
        m,
        "inv.transfer_lots",
        "Move shares or coins between two investment accounts with their lots",
        true,
        transfer_lots,
    );
}

// ----- terms ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Right {
    Call,
    Put,
}

impl Right {
    fn letter(self) -> char {
        match self {
            Right::Call => 'C',
            Right::Put => 'P',
        }
    }
    fn word(self) -> &'static str {
        match self {
            Right::Call => "Call",
            Right::Put => "Put",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BondTax {
    #[default]
    Taxable,
    Municipal,
    Treasury,
}

impl BondTax {
    /// The income account its interest goes to (created on first use).
    pub(crate) fn account_name(self) -> &'static str {
        match self {
            BondTax::Taxable => "Interest",
            BondTax::Municipal => "Tax-exempt Interest",
            BondTax::Treasury => "US Treasury Interest",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct OptionTerms {
    /// The underlying security's id and symbol.
    pub underlying: String,
    pub underlying_symbol: String,
    pub right: Right,
    pub strike: Decimal,
    pub expiration: Date,
    pub multiplier: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BondTerms {
    pub face_per_unit: Decimal,
    pub coupon_rate: Decimal,
    pub payments_per_year: u32,
    pub maturity: Date,
    #[serde(default)]
    pub first_payment: Option<Date>,
    #[serde(default)]
    pub tax: BondTax,
}

pub(crate) fn option_json(o: &OptionTerms) -> Value {
    json!({
        "underlying": o.underlying,
        "underlying_symbol": o.underlying_symbol,
        "right": o.right,
        "strike": format_amount(o.strike.normalize(), None),
        "expiration": o.expiration,
        "multiplier": format_amount(o.multiplier.normalize(), None),
    })
}

pub(crate) fn bond_json(b: &BondTerms) -> Value {
    json!({
        "face_per_unit": format_amount(b.face_per_unit, Some(2)),
        "coupon_rate": format_amount(b.coupon_rate.normalize(), None),
        "payments_per_year": b.payments_per_year,
        "maturity": b.maturity,
        "first_payment": b.first_payment,
        "tax": b.tax,
        "interest_account": b.tax.account_name(),
    })
}

/// What an option or bond action's replay needs beyond the core metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Extra {
    /// The action is on a short position (a written option).
    #[serde(default)]
    pub short: bool,
    /// Exercise / assignment: the right, the underlying shares and strike × shares.
    #[serde(default)]
    pub right: Option<Right>,
    #[serde(default)]
    pub shares: Option<Decimal>,
    #[serde(default)]
    pub strike_total: Option<Decimal>,
    /// Exercise of a put / assignment of a call: the underlying's method.
    #[serde(default)]
    pub under_method: Option<BasisMethod>,
    /// Bond buy (paid) or sale (received) and the interest account.
    #[serde(default)]
    pub accrued: Option<Decimal>,
    #[serde(default)]
    pub interest_account: Option<AccountId>,
    /// Swap: the quantity received; a cash fee.
    #[serde(default)]
    pub to_quantity: Option<Decimal>,
    #[serde(default)]
    pub fee: Option<Decimal>,
    /// Transfer of lots: the network fee in coin and its market value.
    #[serde(default)]
    pub fee_quantity: Option<Decimal>,
    #[serde(default)]
    pub fee_value: Option<Decimal>,
    #[serde(default)]
    pub fee_account: Option<AccountId>,
}

/// The figures a replay worked out for one transaction's split in the replayed
/// security account, and the realised gain (Realized Gains = −gain) when the
/// transaction has one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fix {
    pub value: Decimal,
    pub gain: Option<Decimal>,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn strike_text(strike: Decimal) -> String {
    strike.normalize().to_string()
}

/// `AAPL 261218 C200`.
pub(crate) fn option_symbol(under: &str, exp: Date, right: Right, strike: Decimal) -> String {
    format!(
        "{} {:02}{:02}{:02} {}{}",
        under,
        exp.year().rem_euclid(100),
        exp.month(),
        exp.day(),
        right.letter(),
        strike_text(strike)
    )
}

/// `AAPL Dec 18 2026 200 Call`.
pub(crate) fn option_name(under: &str, exp: Date, right: Right, strike: Decimal) -> String {
    let month = MONTHS
        .get(usize::from(exp.month().saturating_sub(1)))
        .copied()
        .unwrap_or("");
    format!(
        "{under} {month} {} {} {} {}",
        exp.day(),
        exp.year(),
        strike_text(strike),
        right.word()
    )
}

/// The underlying, expiration, right and strike read from an option symbol.
pub(crate) fn parse_option_symbol(symbol: &str) -> Option<(String, Date, Right, Decimal)> {
    let symbol = symbol.trim().to_ascii_uppercase();
    if !mk_core::CommodityCode::is_option_form(&symbol) {
        return None;
    }
    let mut parts = symbol.split(' ');
    let under = parts.next()?.to_owned();
    let date = parts.next()?;
    let rs = parts.next()?;
    let yy: i32 = date.get(0..2)?.parse().ok()?;
    let mm: u8 = date.get(2..4)?.parse().ok()?;
    let dd: u8 = date.get(4..6)?.parse().ok()?;
    let exp = Date::from_ymd(2000 + yy, mm, dd).ok()?;
    let right = match rs.chars().next()? {
        'C' => Right::Call,
        'P' => Right::Put,
        _ => return None,
    };
    let strike: Decimal = rs.get(1..)?.parse().ok()?;
    Some((under, exp, right, strike))
}

/// Check and complete `security.create` arguments of an option (the symbol and name
/// are generated; the terms may come from a typed symbol) or a bond with terms.
pub(crate) fn terms_for_create(
    ctx: &Ctx<'_>,
    a: &mut SecurityCreateArgs,
) -> CommandResult<(Option<OptionTerms>, Option<BondTerms>)> {
    let option_given = a.underlying.is_some()
        || a.right.is_some()
        || a.strike.is_some()
        || a.expiration.is_some()
        || a.multiplier.is_some();
    let bond_given = a.face_per_unit.is_some()
        || a.coupon_rate.is_some()
        || a.payments_per_year.is_some()
        || a.maturity.is_some()
        || a.first_payment.is_some()
        || a.tax.is_some();
    match a.kind {
        SecurityKind::OptionContract => {
            if bond_given {
                return Err(invalid("bond terms are only for bonds"));
            }
            let parsed = parse_option_symbol(&a.symbol);
            let under_ref = a
                .underlying
                .clone()
                .filter(|u| !u.trim().is_empty())
                .or_else(|| parsed.as_ref().map(|p| p.0.clone()))
                .ok_or_else(|| invalid("give the option's underlying stock or ETF"))?;
            let right = a
                .right
                .or(parsed.as_ref().map(|p| p.2))
                .ok_or_else(|| invalid("give the option's right: call or put"))?;
            let strike = a
                .strike
                .or(parsed.as_ref().map(|p| p.3))
                .ok_or_else(|| invalid("give the option's strike price"))?;
            let strike = positive(strike, "the strike")?;
            if mk_core::currency::decimal_places(strike) > 4 {
                return Err(invalid("a strike has at most 4 decimal places"));
            }
            let expiration = a
                .expiration
                .or(parsed.as_ref().map(|p| p.1))
                .ok_or_else(|| invalid("give the option's expiration date"))?;
            let multiplier = positive(
                a.multiplier.unwrap_or(Decimal::ONE_HUNDRED),
                "the multiplier",
            )?;
            let under = match find_security(ctx, &under_ref) {
                Ok(s) => s,
                Err(CommandError::NotFound(_)) => {
                    let sym = under_ref.trim().to_ascii_uppercase();
                    crate::investments::create_security(
                        ctx,
                        &sym,
                        &sym,
                        SecurityKind::Stock,
                        None,
                        None,
                    )?
                }
                Err(e) => return Err(e),
            };
            if !matches!(under.kind, SecurityKind::Stock | SecurityKind::Etf) {
                return Err(invalid(format!(
                    "{} is not a stock or ETF and cannot be an option's underlying",
                    under.symbol
                )));
            }
            a.symbol = option_symbol(&under.symbol, expiration, right, strike);
            if a.name.trim().is_empty() {
                a.name = option_name(&under.symbol, expiration, right, strike);
            }
            Ok((
                Some(OptionTerms {
                    underlying: under.id.clone(),
                    underlying_symbol: under.symbol.clone(),
                    right,
                    strike: strike.normalize(),
                    expiration,
                    multiplier: multiplier.normalize(),
                }),
                None,
            ))
        }
        SecurityKind::Bond if bond_given => {
            if option_given {
                return Err(invalid("option terms are only for options"));
            }
            let maturity = a
                .maturity
                .ok_or_else(|| invalid("give the bond's maturity date"))?;
            let face = positive(
                a.face_per_unit.unwrap_or(Decimal::ONE_THOUSAND),
                "the face per unit",
            )?;
            let coupon = a.coupon_rate.unwrap_or_default();
            if coupon.is_sign_negative() || mk_core::currency::decimal_places(coupon) > 4 {
                return Err(invalid(
                    "the coupon rate is a percent of zero or more with at most 4 decimals",
                ));
            }
            let ppy = a.payments_per_year.unwrap_or(2);
            if !matches!(ppy, 1 | 2 | 4 | 12) {
                return Err(invalid("payments per year are 1, 2, 4 or 12"));
            }
            if let Some(f) = a.first_payment
                && f > maturity
            {
                return Err(invalid("the first payment is after the maturity date"));
            }
            Ok((
                None,
                Some(BondTerms {
                    face_per_unit: face.normalize(),
                    coupon_rate: coupon.normalize(),
                    payments_per_year: ppy,
                    maturity,
                    first_payment: a.first_payment,
                    tax: a.tax.unwrap_or_default(),
                }),
            ))
        }
        _ => {
            if option_given {
                return Err(invalid("option terms are only for options"));
            }
            if bond_given {
                return Err(invalid("bond terms are only for bonds"));
            }
            Ok((None, None))
        }
    }
}

/// Options in whole contracts, bonds with terms in whole units, crypto with at most
/// 8 decimals.
pub(crate) fn check_quantity(s: &Security, q: Decimal) -> CommandResult<()> {
    let whole = q.fract().is_zero();
    match s.kind {
        SecurityKind::OptionContract if !whole => {
            Err(invalid("options are bought and sold in whole contracts"))
        }
        SecurityKind::Bond if s.bond.is_some() && !whole => {
            Err(invalid("bonds are bought and sold in whole units"))
        }
        SecurityKind::Crypto if mk_core::currency::decimal_places(q) > 8 => {
            Err(invalid("a crypto quantity has at most 8 decimal places"))
        }
        _ => Ok(()),
    }
}

fn option_terms(s: &Security) -> CommandResult<&OptionTerms> {
    s.option
        .as_ref()
        .ok_or_else(|| invalid(format!("{} is not an option", s.symbol)))
}

fn bond_terms(s: &Security) -> CommandResult<&BondTerms> {
    s.bond.as_ref().ok_or_else(|| {
        invalid(format!(
            "{} has no bond terms: give its face per unit, coupon and maturity in the security",
            s.symbol
        ))
    })
}

fn crypto(s: &Security) -> CommandResult<()> {
    if s.kind != SecurityKind::Crypto {
        return Err(invalid(format!("{} is not crypto", s.symbol)));
    }
    Ok(())
}

/// A description for realised-gain rows: the option's name, the bond's name with its
/// face, "0.25 BTC" for crypto and "10 sh. VTI" for shares.
pub(crate) fn description(s: &Security, quantity: Decimal) -> String {
    let q = format_amount(quantity.normalize(), None);
    match (s.kind, &s.bond) {
        (SecurityKind::OptionContract, _) => format!("{q} {}", s.name),
        (SecurityKind::Bond, Some(b)) => {
            let face = round2(quantity.checked_mul(b.face_per_unit).unwrap_or_default());
            format!("{} face {}", format_amount(face, Some(2)), s.name)
        }
        (SecurityKind::Crypto, _) => format!("{q} {}", s.symbol),
        _ => format!("{q} sh. {}", s.symbol),
    }
}

// ----- commands --------------------------------------------------------------------

fn option_buy(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    crate::investments::post(ctx, Action::OptionBuy, a, None)
}
fn option_sell(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    crate::investments::post(ctx, Action::OptionSell, a, None)
}
fn option_write(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    crate::investments::post(ctx, Action::OptionWrite, a, None)
}
fn option_cover(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    crate::investments::post(ctx, Action::OptionCover, a, None)
}

/// The date defaults to the option's expiration (or the bond's maturity).
fn dated(ctx: &Ctx<'_>, mut a: ActionArgs) -> CommandResult<ActionArgs> {
    if a.date.is_none()
        && let Some(r) = &a.security
    {
        let s = find_security(ctx, r)?;
        a.date = s
            .option
            .as_ref()
            .map(|o| o.expiration)
            .or(s.bond.as_ref().map(|b| b.maturity));
    }
    Ok(a)
}

fn option_expire(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    let a = dated(ctx, a)?;
    crate::investments::post(ctx, Action::OptionExpire, a, None)
}
fn option_exercise(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    let a = dated(ctx, a)?;
    crate::investments::post(ctx, Action::OptionExercise, a, None)
}
fn option_assigned(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    let a = dated(ctx, a)?;
    crate::investments::post(ctx, Action::OptionAssigned, a, None)
}
fn bond_buy(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    crate::investments::post(ctx, Action::BondBuy, a, None)
}
fn bond_sell(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    crate::investments::post(ctx, Action::BondSell, a, None)
}
fn bond_interest(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    crate::investments::post(ctx, Action::BondInterest, a, None)
}
fn bond_redeem(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    let a = dated(ctx, a)?;
    crate::investments::post(ctx, Action::BondRedeem, a, None)
}
fn crypto_fee(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    crate::investments::post(ctx, Action::CryptoFee, a, None)
}
fn crypto_swap(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    crate::investments::post(ctx, Action::CryptoSwap, a, None)
}
fn crypto_reward(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    crate::investments::post(ctx, Action::CryptoReward, a, None)
}
fn transfer_lots(ctx: &Ctx<'_>, a: ActionArgs) -> CommandResult<Value> {
    crate::investments::post(ctx, Action::TransferLots, a, None)
}

// ----- posting ---------------------------------------------------------------------

fn gains_account(ctx: &Ctx<'_>, chart: &mut Chart, a: &ActionArgs) -> CommandResult<AccountId> {
    other_account(
        ctx,
        chart,
        a.income_account.as_deref(),
        "Realized Gains",
        AccountType::Income,
    )
}

/// The interest account of a bond, by its `tax` (created on first use).
pub(crate) fn interest_account(
    ctx: &Ctx<'_>,
    chart: &mut Chart,
    s: &Security,
    given: Option<&str>,
) -> CommandResult<AccountId> {
    let tax = s.bond.as_ref().map(|b| b.tax).unwrap_or_default();
    other_account(ctx, chart, given, tax.account_name(), AccountType::Income)
}

fn contracts(v: Option<Decimal>) -> CommandResult<Decimal> {
    let q = quantity_arg(v)?;
    if !q.fract().is_zero() {
        return Err(invalid("options are bought and sold in whole contracts"));
    }
    Ok(q)
}

/// Money given (≥ 0, two decimals), default zero.
fn money_or_zero(v: Option<Decimal>, what: &str) -> CommandResult<Decimal> {
    let v = money_amount(v.unwrap_or_default(), what)?;
    if v.is_sign_negative() {
        return Err(invalid(format!("{what} cannot be negative")));
    }
    Ok(v)
}

/// Contracts held in a security account on a date (posted; negative when written),
/// leaving out transaction `except` (the one an edit replaces, revision 2).
fn held(
    ctx: &Ctx<'_>,
    acc: AccountId,
    date: Date,
    except: Option<TransactionId>,
) -> CommandResult<Decimal> {
    let mut h = held_on(ctx, &[acc], date)?
        .get(&acc)
        .copied()
        .unwrap_or_default();
    if let Some(id) = except
        && let Some(old) = ctx.tx.transaction(id)?
        && old.status == TxnStatus::Posted
        && old.date <= date
    {
        for sp in old.splits.iter().filter(|sp| sp.account == acc) {
            h = minus(h, sp.quantity)?;
        }
    }
    Ok(h)
}

fn insufficient(what: &str, held: Decimal, date: Date) -> CommandError {
    crate::investments::coded(
        "insufficient_shares",
        format!(
            "not enough {what} on {date}: {} held",
            format_amount(held, None)
        ),
    )
}

/// Build the posting of an option, bond or crypto action.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build(
    ctx: &Ctx<'_>,
    chart: &mut Chart,
    action: Action,
    a: &ActionArgs,
    mut meta: InvMeta,
    s: &Security,
    broker: AccountId,
    commission: Decimal,
) -> CommandResult<Posting> {
    let date = meta.date;
    let mut ex = Extra::default();
    let mut splits = Vec::new();
    let mut replay = Vec::new();
    let mut price_in_memo = false;
    let sec_acc = if action == Action::BondInterest {
        sec_account_of(chart, broker, &s.symbol)
    } else {
        Some(ensure_sec_account(ctx, chart, broker, &s.symbol)?)
    };
    meta.sec_account = sec_acc;
    replay.extend(sec_acc);
    let sec_acc = sec_acc.ok_or_else(|| invalid("give the security"));
    match action {
        Action::OptionBuy | Action::OptionCover => {
            option_terms(s)?;
            let q = contracts(a.quantity)?;
            let cost = match a.amount {
                Some(v) => money_amount(positive(v, "the amount")?, "the amount")?,
                None => {
                    let p = positive(need(a.price, "a price")?, "the price")?;
                    plus(s.worth(q, p)?, commission)?
                }
            };
            meta.quantity = Some(q);
            meta.amount = Some(cost);
            meta.other_account = Some(gains_account(ctx, chart, a)?);
            ex.short = action == Action::OptionCover;
            // The security value and any gain are set by the replay.
            splits.push(split_of(sec_acc?, cost, q));
            splits.push(cash_split(broker, neg(cost)));
            price_in_memo = true;
        }
        Action::OptionSell | Action::BondSell | Action::BondRedeem => {
            if action == Action::OptionSell {
                option_terms(s)?;
            } else {
                bond_terms(s)?;
            }
            let acc = sec_acc?;
            let q = match (action, a.units.or(a.quantity)) {
                (Action::BondRedeem, None) => {
                    let h = held(ctx, acc, date, Some(meta.txn))?;
                    if h <= Decimal::ZERO {
                        return Err(insufficient("units", h, date));
                    }
                    h
                }
                (_, q) => quantity_arg(q)?,
            };
            if action == Action::OptionSell {
                contracts(Some(q))?;
            }
            let price = match (action, a.price) {
                (Action::BondRedeem, None) => Decimal::ONE_HUNDRED,
                (_, p) => positive(need(p, "a price")?, "the price")?,
            };
            let proceeds = match a.amount {
                Some(v) => money_amount(v, "the amount")?,
                None => minus(s.worth(q, price)?, commission)?,
            };
            meta.price = Some(price);
            meta.quantity = Some(q);
            meta.amount = Some(proceeds);
            meta.method = Some(s.method_on(date));
            let gains = gains_account(ctx, chart, a)?;
            meta.other_account = Some(gains);
            let accrued = money_or_zero(a.accrued_interest, "the accrued interest")?;
            splits.push(cash_split(broker, plus(proceeds, accrued)?));
            splits.push(split_of(acc, Decimal::ZERO, neg(q)));
            splits.push(cash_split(gains, neg(proceeds)));
            if !accrued.is_zero() {
                let ia = interest_account(ctx, chart, s, None)?;
                ex.accrued = Some(accrued);
                ex.interest_account = Some(ia);
                let mut sp = cash_split(ia, neg(accrued));
                sp.memo = Some("Accrued interest received".into());
                splits.push(sp);
            }
            price_in_memo = true;
        }
        Action::OptionWrite => {
            option_terms(s)?;
            let q = contracts(a.quantity)?;
            let premium = match a.amount {
                Some(v) => money_amount(positive(v, "the amount")?, "the amount")?,
                None => {
                    let p = positive(need(a.price, "a price")?, "the price")?;
                    minus(s.worth(q, p)?, commission)?
                }
            };
            if premium <= Decimal::ZERO {
                return Err(invalid("the premium after commission must be above zero"));
            }
            meta.quantity = Some(q);
            meta.amount = Some(premium);
            meta.other_account = Some(gains_account(ctx, chart, a)?);
            ex.short = true;
            splits.push(cash_split(broker, premium));
            splits.push(split_of(sec_acc?, neg(premium), neg(q)));
            price_in_memo = true;
        }
        Action::OptionExpire => {
            option_terms(s)?;
            let acc = sec_acc?;
            let h = held(ctx, acc, date, Some(meta.txn))?;
            if h.is_zero() {
                return Err(insufficient("contracts", h, date));
            }
            let q = match a.quantity {
                Some(_) => contracts(a.quantity)?,
                None => h.abs(),
            };
            if q > h.abs() {
                return Err(insufficient("contracts", h.abs(), date));
            }
            ex.short = h < Decimal::ZERO;
            meta.quantity = Some(q);
            meta.amount = Some(Decimal::ZERO);
            let gains = gains_account(ctx, chart, a)?;
            meta.other_account = Some(gains);
            let sq = if ex.short { q } else { neg(q) };
            splits.push(split_of(acc, Decimal::ZERO, sq));
            splits.push(cash_split(gains, Decimal::ZERO));
        }
        Action::OptionExercise | Action::OptionAssigned => {
            let o = option_terms(s)?.clone();
            let acc = sec_acc?;
            let short = action == Action::OptionAssigned;
            let h = held(ctx, acc, date, Some(meta.txn))?;
            let open = if short { neg(h) } else { h };
            if open <= Decimal::ZERO {
                return Err(insufficient(
                    if short {
                        "written contracts"
                    } else {
                        "contracts"
                    },
                    open.max(Decimal::ZERO),
                    date,
                ));
            }
            let q = match a.quantity {
                Some(_) => contracts(a.quantity)?,
                None => open,
            };
            if q > open {
                return Err(insufficient("contracts", open, date));
            }
            let under = find_security(ctx, &o.underlying)?;
            let under_acc = ensure_sec_account(ctx, chart, broker, &under.symbol)?;
            let n = mul(q, o.multiplier)?.normalize();
            let st = round2(mul(n, o.strike)?);
            meta.quantity = Some(q);
            meta.price = Some(o.strike);
            meta.amount = Some(st);
            meta.security2 = Some(under.id.clone());
            meta.sec_account2 = Some(under_acc);
            replay.push(under_acc);
            let gains = gains_account(ctx, chart, a)?;
            meta.other_account = Some(gains);
            ex.short = short;
            ex.right = Some(o.right);
            ex.shares = Some(n);
            ex.strike_total = Some(st);
            ex.under_method = Some(under.method_on(date));
            meta.lots = a.underlying_lots.clone();
            let oq = if short { q } else { neg(q) };
            splits.push(split_of(acc, Decimal::ZERO, oq));
            // Shares bought: an exercised call or an assigned put.
            let buys = (o.right == Right::Call) != short;
            if buys {
                let cash = plus(st, commission)?;
                splits.push(cash_split(broker, neg(cash)));
                splits.push(split_of(under_acc, cash, n));
            } else {
                let cash = minus(st, commission)?;
                splits.push(cash_split(broker, cash));
                splits.push(split_of(under_acc, Decimal::ZERO, neg(n)));
                splits.push(cash_split(gains, neg(cash)));
            }
        }
        Action::BondBuy => {
            bond_terms(s)?;
            let q = quantity_arg(a.units.or(a.quantity))?;
            let cost = match a.amount {
                Some(v) => money_amount(positive(v, "the amount")?, "the amount")?,
                None => {
                    let p = positive(need(a.price, "a price")?, "the price")?;
                    plus(s.worth(q, p)?, commission)?
                }
            };
            let accrued = money_or_zero(a.accrued_interest, "the accrued interest")?;
            meta.quantity = Some(q);
            meta.amount = Some(cost);
            splits.push(split_of(sec_acc?, cost, q));
            splits.push(cash_split(broker, neg(plus(cost, accrued)?)));
            if !accrued.is_zero() {
                let ia = interest_account(ctx, chart, s, None)?;
                ex.accrued = Some(accrued);
                ex.interest_account = Some(ia);
                meta.other_account = Some(ia);
                let mut sp = cash_split(ia, accrued);
                sp.memo = Some("Accrued interest paid".into());
                splits.push(sp);
            }
            price_in_memo = true;
        }
        Action::BondInterest => {
            bond_terms(s)?;
            let amt = money_amount(
                positive(need(a.amount, "the amount")?, "the amount")?,
                "the amount",
            )?;
            let ia = interest_account(ctx, chart, s, a.income_account.as_deref())?;
            meta.amount = Some(amt);
            meta.other_account = Some(ia);
            splits.push(cash_split(broker, amt));
            splits.push(cash_split(ia, neg(amt)));
        }
        Action::CryptoFee => {
            crypto(s)?;
            let q = quantity_arg(a.quantity)?;
            let value = market_value(ctx, s, q, a.value, date)?;
            let fees = other_account(
                ctx,
                chart,
                a.expense_account.as_deref(),
                "Investment Fees",
                AccountType::Expense,
            )?;
            let gains = gains_account(ctx, chart, a)?;
            meta.quantity = Some(q);
            meta.amount = Some(value);
            meta.method = Some(s.method_on(date));
            meta.other_account = Some(gains);
            ex.fee_account = Some(fees);
            splits.push(split_of(sec_acc?, Decimal::ZERO, neg(q)));
            splits.push(cash_split(fees, value));
            splits.push(cash_split(gains, neg(value)));
        }
        Action::CryptoSwap => {
            crypto(s)?;
            let to_ref =
                a.to.as_deref()
                    .ok_or_else(|| invalid("give the coin received"))?;
            let to = find_security(ctx, to_ref)?;
            crypto(&to)?;
            if to.id == s.id {
                return Err(invalid("a swap is between two different coins"));
            }
            let q = quantity_arg(a.from_quantity.or(a.quantity))?;
            check_quantity(s, q)?;
            let tq = quantity_arg(a.to_quantity)?;
            check_quantity(&to, tq)?;
            let value = money_amount(
                positive(need(a.value, "the value received")?, "the value")?,
                "the value",
            )?;
            let fee = money_or_zero(a.fee, "the fee")?;
            let to_acc = ensure_sec_account(ctx, chart, broker, &to.symbol)?;
            let gains = gains_account(ctx, chart, a)?;
            meta.quantity = Some(q);
            meta.amount = Some(value);
            meta.method = Some(s.method_on(date));
            meta.other_account = Some(gains);
            meta.security2 = Some(to.id.clone());
            meta.sec_account2 = Some(to_acc);
            replay.push(to_acc);
            ex.to_quantity = Some(tq);
            ex.fee = (!fee.is_zero()).then_some(fee);
            splits.push(split_of(sec_acc?, Decimal::ZERO, neg(q)));
            splits.push(split_of(to_acc, plus(value, fee)?, tq));
            if !fee.is_zero() {
                splits.push(cash_split(broker, neg(fee)));
            }
            splits.push(cash_split(gains, neg(value)));
            let memo = format!(
                "Swap {} {} for {} {}",
                format_amount(q, None),
                s.symbol,
                format_amount(tq, None),
                to.symbol
            );
            return finish(a, meta, ex, splits, replay, s, Some(memo));
        }
        Action::CryptoReward => {
            crypto(s)?;
            let q = quantity_arg(a.quantity)?;
            let value = market_value(ctx, s, q, a.value, date)?;
            let income = other_account(
                ctx,
                chart,
                a.income_account.as_deref(),
                "Crypto Rewards",
                AccountType::Income,
            )?;
            meta.quantity = Some(q);
            meta.amount = Some(value);
            meta.other_account = Some(income);
            splits.push(split_of(sec_acc?, value, q));
            splits.push(cash_split(income, neg(value)));
        }
        Action::TransferLots => {
            let dest = brokerage(
                chart,
                Some(
                    a.to_account
                        .as_deref()
                        .ok_or_else(|| invalid("give the account receiving them"))?,
                ),
            )?
            .id;
            if dest == broker {
                return Err(invalid("choose two different accounts"));
            }
            let q = quantity_arg(a.quantity)?;
            let dest_acc = ensure_sec_account(ctx, chart, dest, &s.symbol)?;
            meta.quantity = Some(q);
            meta.method = Some(s.method_on(date));
            meta.security2 = Some(s.id.clone());
            meta.sec_account2 = Some(dest_acc);
            replay.push(dest_acc);
            let gains = gains_account(ctx, chart, a)?;
            meta.other_account = Some(gains);
            let fq = match a.fee_quantity {
                Some(f) if !f.is_zero() => Some(quantity_arg(Some(f))?),
                _ => None,
            };
            let mut out_q = q;
            let mut fee_splits = Vec::new();
            if let Some(fq) = fq {
                check_quantity(s, fq)?;
                let fv = market_value(ctx, s, fq, a.fee_value, date)?;
                let fees = other_account(
                    ctx,
                    chart,
                    a.expense_account.as_deref(),
                    "Investment Fees",
                    AccountType::Expense,
                )?;
                ex.fee_quantity = Some(fq);
                ex.fee_value = Some(fv);
                ex.fee_account = Some(fees);
                out_q = plus(out_q, fq)?;
                fee_splits.push(cash_split(fees, fv));
                fee_splits.push(cash_split(gains, neg(fv)));
            }
            splits.push(split_of(sec_acc?, Decimal::ZERO, neg(out_q)));
            splits.push(split_of(dest_acc, Decimal::ZERO, q));
            splits.extend(fee_splits);
            let memo = format!(
                "Transfer {} {} to {}",
                format_amount(q, None),
                s.symbol,
                chart.full_name(dest)
            );
            return finish(a, meta, ex, splits, replay, s, Some(memo));
        }
        _ => return Err(invalid("not an option, bond or crypto action")),
    }
    let memo = describe(
        action,
        Some(&s.symbol),
        meta.quantity,
        if price_in_memo { meta.price } else { None },
    );
    finish(a, meta, ex, splits, replay, s, Some(memo))
}

fn finish(
    a: &ActionArgs,
    mut meta: InvMeta,
    ex: Extra,
    splits: Vec<mk_core::Split>,
    replay: Vec<AccountId>,
    s: &Security,
    memo: Option<String>,
) -> CommandResult<Posting> {
    meta.extra = (ex != Extra::default()).then_some(ex);
    Ok(Posting {
        splits,
        payee: Some(s.symbol.clone()),
        memo: a.memo.clone().filter(|m| !m.trim().is_empty()).or(memo),
        meta,
        replay,
    })
}

/// The market value given, else quantity × the latest price on or before `date`.
fn market_value(
    ctx: &Ctx<'_>,
    s: &Security,
    q: Decimal,
    given: Option<Decimal>,
    date: Date,
) -> CommandResult<Decimal> {
    match given {
        Some(v) => money_amount(positive(v, "the value")?, "the value"),
        None => match latest_price(ctx, &s.id, date)? {
            Some((_, p)) => {
                let v = s.worth(q, p)?;
                if v <= Decimal::ZERO {
                    return Err(invalid("give the value"));
                }
                Ok(v)
            }
            None => Err(invalid(format!(
                "there is no price of {} on {date}: give the value",
                s.symbol
            ))),
        },
    }
}

// ----- replay ----------------------------------------------------------------------

/// Actions whose lots move from one security account to another.
pub(crate) fn hands_over(a: Action) -> bool {
    matches!(
        a,
        Action::OptionExercise | Action::OptionAssigned | Action::TransferLots
    )
}

/// Shares in of a written option (opening holdings): short lots.
pub(crate) fn is_short_in(m: &InvMeta) -> bool {
    m.action == Action::SharesIn && m.extra.as_ref().is_some_and(|e| e.short)
}

fn dispose(
    st: &mut lots::LotState,
    t: &Transaction,
    q: Decimal,
    proceeds: Option<Decimal>,
    method: lots::Method,
    sel: Option<Vec<lots::Selection>>,
) -> CommandResult<Disposal> {
    match st
        .apply(&Event::Dispose {
            date: t.date,
            quantity: q,
            proceeds,
            method,
            selections: sel,
        })
        .map_err(|e| lot_error(e, t.date))?
    {
        Outcome::Disposal(d) => Ok(d),
        _ => Err(CommandError::Integrity(
            "a disposal without a result".into(),
        )),
    }
}

/// A disposal of short lots as realised: proceeds are the premium received, cost
/// what closing them cost; always short-term (Publication 550).
fn short_disposal(d: Disposal) -> Disposal {
    let portions: Vec<Portion> = d
        .portions
        .into_iter()
        .map(|p| Portion {
            basis: p.proceeds,
            proceeds: p.basis,
            long_term: false,
            ..p
        })
        .collect();
    let basis = portions
        .iter()
        .fold(Decimal::ZERO, |t, p| t.checked_add(p.basis).unwrap_or(t));
    Disposal {
        basis,
        portions,
        average: false,
    }
}

fn found(r: &mut Replay, t: &Transaction, action: Action, d: Disposal) {
    r.disposals.push(Found {
        txn: t.id,
        date: t.date,
        action,
        disposal: d,
    });
}

fn gain_of(d: &Disposal) -> Decimal {
    d.portions
        .iter()
        .fold(Decimal::ZERO, |t, p| t.checked_add(p.gain()).unwrap_or(t))
}

fn acquire(
    st: &mut lots::LotState,
    t: &Transaction,
    lot: String,
    acquired: Date,
    q: Decimal,
    basis: Decimal,
) -> CommandResult<()> {
    st.apply(&Event::Acquire {
        lot,
        acquired,
        quantity: q,
        basis,
        txn: t.id.to_string(),
    })
    .map_err(|e| lot_error(e, t.date))?;
    Ok(())
}

/// The portions the other account handed over in transaction `t`.
fn handed(ctx: &Ctx<'_>, from: Option<AccountId>, t: &Transaction) -> CommandResult<Vec<Portion>> {
    let from =
        from.ok_or_else(|| CommandError::Integrity("a hand-over without its account".into()))?;
    let src = crate::investments::replay_to(ctx, from, None, Some(t.id))?;
    Ok(src
        .handoff
        .into_iter()
        .find(|(id, _)| *id == t.id)
        .map(|(_, p)| p)
        .unwrap_or_default())
}

fn sum_basis(p: &[Portion]) -> CommandResult<Decimal> {
    p.iter().try_fold(Decimal::ZERO, |t, x| plus(t, x.basis))
}

/// Replay one option, bond or crypto transaction in security account `account`.
pub(crate) fn replay_event(
    ctx: &Ctx<'_>,
    r: &mut Replay,
    account: AccountId,
    t: &Transaction,
    m: &InvMeta,
) -> CommandResult<()> {
    let ex = m.extra.clone().unwrap_or_default();
    let second = m.sec_account2 == Some(account) && m.sec_account != Some(account);
    let q = m.quantity.unwrap_or_default();
    let amount = m.amount.unwrap_or_default();
    let method = m.method.unwrap_or(BasisMethod::Fifo).engine();
    let fix = |r: &mut Replay, value: Decimal, gain: Option<Decimal>| {
        r.fixes.push((t.id, Fix { value, gain }));
    };
    match m.action {
        Action::SharesIn => {
            for (i, l) in m.lots.clone().unwrap_or_default().iter().enumerate() {
                acquire(
                    &mut r.short,
                    t,
                    format!("{}:{}", t.id, i + 1),
                    l.acquired.unwrap_or(t.date),
                    l.quantity,
                    l.basis.unwrap_or_default(),
                )?;
            }
        }
        Action::BondBuy | Action::CryptoReward => {
            let basis = value_in(t, account);
            acquire(&mut r.state, t, t.id.to_string(), t.date, q, basis)?;
        }
        Action::CryptoSwap if second => {
            let basis = value_in(t, account);
            let tq = ex.to_quantity.unwrap_or_default();
            acquire(&mut r.state, t, t.id.to_string(), t.date, tq, basis)?;
        }
        Action::OptionSell
        | Action::BondSell
        | Action::BondRedeem
        | Action::CryptoFee
        | Action::CryptoSwap => {
            let d = dispose(&mut r.state, t, q, Some(amount), method, selections(m))?;
            let g = gain_of(&d);
            fix(r, neg(d.basis), Some(g));
            found(r, t, m.action, d);
        }
        Action::OptionBuy => {
            // Written contracts are bought back first; the rest opens a long lot.
            let c = q.min(r.short.held());
            let mut value = amount;
            let mut gain = Decimal::ZERO;
            if c > Decimal::ZERO {
                let cost_c = if c == q {
                    amount
                } else {
                    round2(
                        mul(amount, c)?
                            .checked_div(q)
                            .ok_or_else(|| invalid("bad quantity"))?,
                    )
                };
                let d = dispose(&mut r.short, t, c, Some(cost_c), lots::Method::Fifo, None)?;
                let premium = d.basis;
                let d = short_disposal(d);
                gain = gain_of(&d);
                value = plus(premium, minus(amount, cost_c)?)?;
                found(r, t, m.action, d);
                let rest = minus(q, c)?;
                if rest > Decimal::ZERO {
                    let b = minus(amount, cost_c)?;
                    acquire(&mut r.state, t, t.id.to_string(), t.date, rest, b)?;
                }
            } else {
                acquire(&mut r.state, t, t.id.to_string(), t.date, q, amount)?;
            }
            fix(r, value, Some(gain));
        }
        Action::OptionWrite => {
            let long = r.state.held();
            if long > Decimal::ZERO {
                return Err(CommandError::Rejected(format!(
                    "{} contracts are held on {}: sell them to close before writing contracts",
                    format_amount(long, None),
                    t.date
                )));
            }
            acquire(&mut r.short, t, t.id.to_string(), t.date, q, amount)?;
        }
        Action::OptionCover => {
            let d = dispose(&mut r.short, t, q, Some(amount), lots::Method::Fifo, None)
                .map_err(|e| short_error(e, &r.short))?;
            let premium = d.basis;
            let d = short_disposal(d);
            let g = gain_of(&d);
            fix(r, premium, Some(g));
            found(r, t, m.action, d);
        }
        Action::OptionExpire => {
            if ex.short {
                let d = dispose(
                    &mut r.short,
                    t,
                    q,
                    Some(Decimal::ZERO),
                    lots::Method::Fifo,
                    None,
                )
                .map_err(|e| short_error(e, &r.short))?;
                let premium = d.basis;
                let d = short_disposal(d);
                let g = gain_of(&d);
                fix(r, premium, Some(g));
                found(r, t, m.action, d);
            } else {
                let d = dispose(&mut r.state, t, q, Some(Decimal::ZERO), method, None)?;
                let g = gain_of(&d);
                fix(r, neg(d.basis), Some(g));
                found(r, t, m.action, d);
            }
        }
        Action::OptionExercise | Action::OptionAssigned if !second => {
            // The option's lots close with no gain of their own; their basis (or
            // premium) goes to the shares.
            let (d, value) = if ex.short {
                let d = dispose(&mut r.short, t, q, None, lots::Method::Fifo, None)
                    .map_err(|e| short_error(e, &r.short))?;
                let b = d.basis;
                (d, b)
            } else {
                let d = dispose(&mut r.state, t, q, None, lots::Method::Fifo, None)?;
                let b = neg(d.basis);
                (d, b)
            };
            fix(r, value, None);
            r.handoff.push((t.id, d.portions));
        }
        Action::OptionExercise | Action::OptionAssigned => {
            let x = sum_basis(&handed(ctx, m.sec_account, t)?)?;
            let n = ex.shares.unwrap_or_default();
            let st = ex.strike_total.unwrap_or_default();
            let c = m.commission;
            let right = ex.right.unwrap_or(Right::Call);
            let buys = (right == Right::Call) != ex.short;
            if buys {
                // Exercised call: strike + option basis + commission; assigned put:
                // strike − premium + commission.
                let basis = if ex.short {
                    plus(minus(st, x)?, c)?
                } else {
                    plus(plus(st, x)?, c)?
                };
                acquire(&mut r.state, t, t.id.to_string(), t.date, n, basis)?;
                fix(r, basis, None);
            } else {
                // Exercised put: strike − option basis − commission; assigned call:
                // strike + premium − commission.
                let proceeds = if ex.short {
                    minus(plus(st, x)?, c)?
                } else {
                    minus(minus(st, x)?, c)?
                };
                let um = ex.under_method.unwrap_or(BasisMethod::Fifo).engine();
                let d = dispose(&mut r.state, t, n, Some(proceeds), um, selections(m))?;
                let g = gain_of(&d);
                fix(r, neg(d.basis), Some(g));
                found(r, t, m.action, d);
            }
        }
        Action::TransferLots if second => {
            let portions = handed(ctx, m.sec_account, t)?;
            for (i, p) in portions.iter().enumerate() {
                acquire(
                    &mut r.state,
                    t,
                    format!("{}:t{}", t.id, i + 1),
                    p.acquired,
                    p.quantity,
                    p.basis,
                )?;
            }
            fix(r, sum_basis(&portions)?, None);
        }
        Action::TransferLots => {
            let d = dispose(&mut r.state, t, q, None, method, selections(m))?;
            let mut value = neg(d.basis);
            let mut gain = None;
            r.handoff.push((t.id, d.portions));
            if let (Some(fq), Some(fv)) = (ex.fee_quantity, ex.fee_value) {
                let f = dispose(&mut r.state, t, fq, Some(fv), lots::Method::Fifo, None)?;
                value = minus(value, f.basis)?;
                gain = Some(gain_of(&f));
                found(r, t, Action::CryptoFee, f);
            }
            fix(r, value, gain);
        }
        _ => {}
    }
    Ok(())
}

fn short_error(e: CommandError, short: &lots::LotState) -> CommandError {
    match e {
        CommandError::Coded {
            code: "insufficient_shares",
            ..
        } => crate::investments::coded(
            "insufficient_shares",
            format!(
                "not enough written contracts: {} written",
                format_amount(short.held(), None)
            ),
        ),
        other => other,
    }
}

/// The disposals of one transaction in one account as one (for the preview).
pub(crate) fn merged(found: &[&Found]) -> Option<Disposal> {
    let first = found.first()?;
    let mut d = first.disposal.clone();
    for f in found.iter().skip(1) {
        d.basis = d.basis.checked_add(f.disposal.basis).unwrap_or(d.basis);
        d.portions.extend(f.disposal.portions.iter().cloned());
    }
    Some(d)
}

/// Short lots of a security account (for `inv.lots`).
pub(crate) fn short_lots_json(r: &Replay) -> Vec<Value> {
    r.short
        .open()
        .iter()
        .map(|l| {
            json!({
                "lot": l.id,
                "acquired": l.acquired,
                "quantity": format_amount(neg(l.remaining), None),
                "original_quantity": format_amount(neg(l.original), None),
                "basis": format_amount(neg(l.basis), Some(2)),
                "premium": format_amount(l.basis, Some(2)),
                "short": true,
                "term": "short",
                "txn": l.txn,
            })
        })
        .collect()
}

// ----- coupon reminders --------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemindersArgs {
    /// Brokerage account (id, full name or code).
    account: String,
    /// Bond security id or symbol.
    security: String,
    /// Units held (default: the units held in the account today).
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    units: Option<Decimal>,
    /// Default today.
    #[serde(default)]
    today: Option<Date>,
}

/// Every coupon date of a bond, maturity backwards by the payment period.
pub(crate) fn coupon_dates(b: &BondTerms, from: Date) -> Vec<Date> {
    let months = i64::from(12 / b.payments_per_year.max(1));
    let last_day = b.maturity.day() == Date::days_in_month(b.maturity.year(), b.maturity.month());
    let mut out = Vec::new();
    for k in 0..1200_i64 {
        let Some((y, m)) = Date::shift_month(b.maturity.year(), b.maturity.month(), -k * months)
        else {
            break;
        };
        let day = if last_day {
            Date::days_in_month(y, m)
        } else {
            b.maturity.day()
        };
        let Ok(d) = Date::from_ymd_clamped(y, m, day) else {
            break;
        };
        if d < from || b.first_payment.is_some_and(|f| d < f) {
            break;
        }
        out.push(d);
    }
    out.reverse();
    out
}

fn bond_reminders(ctx: &Ctx<'_>, a: RemindersArgs) -> CommandResult<Value> {
    let today = crate::sales_reports::today_or(a.today)?;
    let mut chart = Chart::load(ctx)?;
    let broker = brokerage(&chart, Some(&a.account))?.id;
    let s = find_security(ctx, &a.security)?;
    let b = bond_terms(&s)?.clone();
    if b.coupon_rate.is_zero() {
        return Err(invalid(format!("{} pays no coupon", s.name)));
    }
    let units = match a.units {
        Some(u) => positive(u, "the units")?,
        None => {
            let acc = sec_account_of(&chart, broker, &s.symbol)
                .ok_or_else(|| invalid(format!("no {} is held in this account", s.symbol)))?;
            let h = held(ctx, acc, today, None)?;
            if h <= Decimal::ZERO {
                return Err(invalid(format!("no {} is held in this account", s.symbol)));
            }
            h
        }
    };
    let dates = coupon_dates(&b, today);
    let Some(start) = dates.first().copied() else {
        return Err(invalid(format!("{} has no payment dates left", s.name)));
    };
    let ppy = Decimal::from(b.payments_per_year);
    let amount = round2(
        mul(mul(units, b.face_per_unit)?, b.coupon_rate)?
            .checked_div(Decimal::ONE_HUNDRED)
            .and_then(|x| x.checked_div(ppy))
            .ok_or_else(|| invalid("bad coupon"))?,
    );
    let ia = interest_account(ctx, &mut chart, &s, None)?;
    let last_day = b.maturity.day() == Date::days_in_month(b.maturity.year(), b.maturity.month());
    let day = if last_day {
        json!(MonthDay::Last)
    } else {
        json!(b.maturity.day())
    };
    let args = json!({
        "name": format!("Coupon: {}", s.name),
        "template": {
            "payee": s.name,
            "memo": format!("Coupon {}", s.name),
            "splits": [
                {"account": broker.to_string(), "value": amount.to_string()},
                {"account": ia.to_string(), "value": neg(amount).to_string()},
            ],
        },
        "recurrence": {"frequency": "monthly", "interval": 12 / b.payments_per_year, "day": day},
        "start": start,
        "end": {"on": b.maturity},
        "auto_enter": false,
    });
    let v = crate::schedules::create_investment(
        ctx,
        args,
        json!({"action": "bond_interest", "account": broker.to_string(), "security": s.id}),
    )?;
    Ok(json!({
        "schedule": v,
        "amount": format_amount(amount, Some(2)),
        "dates": dates,
        "count": dates.len(),
    }))
}

/// Enter a coupon reminder's occurrence as `inv.bond_interest`.
pub(crate) fn enter_scheduled(
    ctx: &Ctx<'_>,
    inv: &Value,
    date: Date,
    amount: Decimal,
    memo: Option<String>,
) -> CommandResult<Transaction> {
    let action = inv
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if action != "bond_interest" {
        return Err(CommandError::Integrity(format!(
            "unknown scheduled investment action {action:?}"
        )));
    }
    let args = ActionArgs {
        account: inv
            .get("account")
            .and_then(Value::as_str)
            .map(str::to_owned),
        security: inv
            .get("security")
            .and_then(Value::as_str)
            .map(str::to_owned),
        date: Some(date),
        amount: Some(amount),
        memo,
        ..ActionArgs::default()
    };
    let out = crate::investments::post(ctx, Action::BondInterest, args, None)?;
    let id = out
        .pointer("/transaction/id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| CommandError::Integrity("posted transaction missing".into()))?;
    ctx.tx
        .transaction(id)?
        .ok_or_else(|| CommandError::Integrity("posted transaction missing".into()))
}

// ----- holdings and attention -----------------------------------------------------------

/// Extra holding fields: option terms and days left, bond face, coupon, maturity and
/// price as a percent of face, short positions, crypto's 8 decimals.
pub(crate) fn holding_fields(
    s: &Security,
    quantity: Decimal,
    basis: Decimal,
    price: Option<Decimal>,
    as_of: Date,
) -> Value {
    let mut v = json!({"kind": s.kind});
    let Some(o) = v.as_object_mut() else {
        return v;
    };
    match s.kind {
        SecurityKind::OptionContract => {
            if let Some(t) = &s.option {
                o.insert("underlying".into(), json!(t.underlying_symbol));
                o.insert("right".into(), json!(t.right));
                o.insert("strike".into(), json!(format_amount(t.strike, None)));
                o.insert("expiration".into(), json!(t.expiration));
                o.insert(
                    "days_left".into(),
                    json!(as_of.days_until(t.expiration).max(0)),
                );
            }
            if quantity < Decimal::ZERO {
                o.insert("short".into(), json!(true));
            }
        }
        SecurityKind::Bond => {
            if let Some(b) = &s.bond {
                let face = round2(quantity.checked_mul(b.face_per_unit).unwrap_or_default());
                o.insert("face".into(), json!(format_amount(face, Some(2))));
                o.insert(
                    "coupon_rate".into(),
                    json!(format_amount(b.coupon_rate, None)),
                );
                o.insert("maturity".into(), json!(b.maturity));
                o.insert(
                    "price_percent".into(),
                    json!(price.map(|p| format_amount(p, None))),
                );
                let diff = basis.checked_sub(face).unwrap_or_default();
                if !diff.is_zero() {
                    o.insert(
                        "face_note".into(),
                        json!(format!(
                            "Bought {} face by {}",
                            if diff > Decimal::ZERO {
                                "above"
                            } else {
                                "below"
                            },
                            format_amount(diff.abs(), Some(2))
                        )),
                    );
                }
            }
        }
        SecurityKind::Crypto => {
            o.insert(
                "quantity_text".into(),
                json!(format_amount(quantity.round_dp(8), Some(8))),
            );
        }
        _ => {}
    }
    v
}

/// Home → Attention: options still open after their expiration date, bonds held on
/// or after their maturity date. (label, target)
pub(crate) fn attention(ctx: &Ctx<'_>, today: Date) -> CommandResult<Vec<(String, Value)>> {
    let chart = Chart::load(ctx)?;
    let secs = securities(ctx)?;
    let mut held_q: std::collections::HashMap<AccountId, Decimal> = Default::default();
    for row in ctx.tx.posted_splits(None)? {
        if chart
            .get(row.account)
            .is_some_and(|a| a.subtype == AccountSubtype::Security)
        {
            let e = held_q.entry(row.account).or_default();
            *e = e.checked_add(row.quantity).unwrap_or(*e);
        }
    }
    let mut out = Vec::new();
    for (acc, _) in chart.tree_order() {
        if acc.subtype != AccountSubtype::Security {
            continue;
        }
        let q = held_q.get(&acc.id).copied().unwrap_or_default();
        if q.is_zero() {
            continue;
        }
        let Some(s) = secs.iter().find(|s| s.symbol == acc.commodity.as_str()) else {
            continue;
        };
        let Some(broker) = acc.parent else { continue };
        if let Some(o) = &s.option
            && today > o.expiration
        {
            out.push((
                format!("{} expired — record what happened", s.name),
                json!({"screen": "option_expiry", "account": broker, "security": s.id}),
            ));
        }
        if let Some(b) = &s.bond
            && today >= b.maturity
            && q > Decimal::ZERO
        {
            out.push((
                format!("{} matured — record the redemption", s.name),
                json!({"screen": "bond_redeem", "account": broker, "security": s.id}),
            ));
        }
    }
    Ok(out)
}
