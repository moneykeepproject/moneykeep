//! Payment-processor payouts (connectivity-processors.md): Stripe, Square and PayPal
//! answers parsed with the input-safety limits into payouts made of categorised
//! entries. Pure: the requests are made by the caller.
//!
//! Field names, enums and paging come from the vendors' API documentation:
//! - Stripe payouts: <https://docs.stripe.com/api/payouts/list> (the Payout object);
//!   balance transactions <https://docs.stripe.com/api/balance_transactions/list.md>,
//!   types <https://docs.stripe.com/reports/balance-transaction-types.md>, reporting
//!   categories <https://docs.stripe.com/reports/reporting-categories>,
//!   authentication <https://docs.stripe.com/api/authentication.md>.
//! - Square payouts: <https://developer.squareup.com/reference/square/payouts-api/list-payouts>,
//!   payout entries <https://developer.squareup.com/reference/square/payouts-api/list-payout-entries>.
//! - PayPal transaction search: <https://developer.paypal.com/docs/api/transaction-search/v1/>,
//!   event codes <https://developer.paypal.com/docs/transaction-search/transaction-event-codes/>.
//!
//! (connectivity-processors.md revision 2 records the vendor details checked.)

use mk_core::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ImportError, ImportResult};

/// Largest answer accepted from a processor (bytes).
pub const MAX_RESPONSE_BYTES: usize = 20 * 1024 * 1024;
/// PayPal: at most this many days per transaction search.
pub const PAYPAL_WINDOW_DAYS: i64 = 31;

fn invalid(m: impl Into<String>) -> ImportError {
    ImportError::Invalid(m.into())
}

/// What an entry is, for the accounts it posts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Sale,
    Refund,
    Fee,
    Dispute,
    DisputeReversal,
    Adjustment,
    Reserve,
    Tax,
    /// The payout (withdrawal) itself.
    Transfer,
    Other,
}

impl Category {
    pub const ALL: [Category; 10] = [
        Category::Sale,
        Category::Refund,
        Category::Fee,
        Category::Dispute,
        Category::DisputeReversal,
        Category::Adjustment,
        Category::Reserve,
        Category::Tax,
        Category::Transfer,
        Category::Other,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Category::Sale => "sale",
            Category::Refund => "refund",
            Category::Fee => "fee",
            Category::Dispute => "dispute",
            Category::DisputeReversal => "dispute_reversal",
            Category::Adjustment => "adjustment",
            Category::Reserve => "reserve",
            Category::Tax => "tax",
            Category::Transfer => "transfer",
            Category::Other => "other",
        }
    }

    pub fn from_key(k: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.key() == k)
    }
}

/// One entry of a payout: `gross` signed from the merchant's view (+ in), `fee` the
/// processing fee taken from it (+ charged, − returned).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    /// Epoch seconds.
    pub created: i64,
    pub category: Category,
    pub gross: Decimal,
    pub fee: Decimal,
    pub currency: String,
    /// Cleaned description (input-safety.md R3).
    pub description: String,
    /// The provider's own kind (Stripe type, Square entry type, PayPal event code).
    pub kind: String,
    /// Why this entry needs a look in review (unmapped code, amounts that disagree).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Entry {
    pub fn net(&self) -> Decimal {
        self.gross.checked_sub(self.fee).unwrap_or(self.gross)
    }
}

/// A payout to the bank and the entries it pays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payout {
    pub id: String,
    /// Epoch seconds of the arrival (or withdrawal) date.
    pub arrival: i64,
    /// Epoch seconds the payout was created (re-reads of payouts still on the way).
    #[serde(default)]
    pub created: i64,
    /// Stripe: why a payout failed (`failure_code`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    pub currency: String,
    /// Amount sent to the bank.
    pub net: Decimal,
    /// `paid`, `pending`, `failed`, … (lower case).
    pub status: String,
    /// Square: the location.
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub location_name: Option<String>,
    pub entries: Vec<Entry>,
}

/// What a sync read from a processor.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProcessorData {
    pub payouts: Vec<Payout>,
    /// PayPal: entries after the last withdrawal ("pending payout").
    pub pending: Vec<Entry>,
    /// Entries outside any payout, staged as their own rows against the clearing
    /// account: Stripe fees with no payout, PayPal fees after the last withdrawal, and
    /// PayPal transfers that are not withdrawals to the bank.
    #[serde(default)]
    pub loose: Vec<Entry>,
    pub warnings: Vec<String>,
}

fn clean(s: &str) -> String {
    mk_core::text::clean(s, crate::safety::TEXT_CHARS).text
}

/// Parse an answer with the input-safety limits.
pub fn parse(body: &[u8]) -> ImportResult<Value> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(invalid("the answer is larger than 20 MB"));
    }
    let text = std::str::from_utf8(body).map_err(|_| invalid("the answer is not UTF-8 text"))?;
    crate::safety::parse_json(text)
}

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn int(v: &Value, k: &str) -> ImportResult<i64> {
    match v.get(k) {
        Some(Value::Number(n)) => n
            .as_i64()
            .ok_or_else(|| invalid(format!("{k} is not a whole number"))),
        Some(Value::String(t)) => t
            .trim()
            .parse()
            .map_err(|_| invalid(format!("{k} is not a whole number"))),
        None | Some(Value::Null) => Ok(0),
        Some(_) => Err(invalid(format!("{k} is not a number"))),
    }
}

/// An amount in minor units of `currency` (Stripe, Square) as a decimal.
pub fn minor(amount: i64, currency: &str) -> Decimal {
    Decimal::new(
        amount,
        mk_core::currency::minor_units(&currency.to_ascii_uppercase()),
    )
}

fn decimal_text(t: &str, what: &str) -> ImportResult<Decimal> {
    crate::safety::check_number_len(t)?;
    t.trim()
        .parse::<Decimal>()
        .map_err(|_| invalid(format!("{what}: {t:?} is not a number")))
}

// ----- Stripe ----------------------------------------------------------------------------

/// Stripe key kinds by prefix (<https://docs.stripe.com/api/authentication.md>): only a
/// restricted key (`rk_live_`, `rk_test_`) is accepted; a secret key (`sk_`) grants
/// access to everything and is refused.
pub fn check_stripe_key(key: &str) -> ImportResult<()> {
    let k = key.trim();
    if k.starts_with("sk_") {
        return Err(invalid(
            "this is a Stripe secret key, which can change everything in your Stripe account. Create a restricted key with read access only (Developers > API keys > Create restricted key) and paste that instead",
        ));
    }
    if k.starts_with("pk_") {
        return Err(invalid(
            "this is a publishable key; paste a restricted key with read access",
        ));
    }
    if !(k.starts_with("rk_live_") || k.starts_with("rk_test_")) || k.len() < 16 {
        return Err(invalid(
            "a Stripe restricted key starts with rk_live_ (or rk_test_)",
        ));
    }
    if !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(invalid(
            "the key contains characters a Stripe key never has",
        ));
    }
    Ok(())
}

/// A Stripe list page: its objects and `has_more`.
pub fn stripe_page(body: &[u8]) -> ImportResult<(Vec<Value>, bool)> {
    let v = parse(body)?;
    let data = v
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| invalid("a Stripe list has no data"))?;
    Ok((
        data,
        v.get("has_more").and_then(Value::as_bool) == Some(true),
    ))
}

/// Category of a Stripe balance transaction from its `reporting_category` (falling
/// back to `type`). Reporting categories:
/// <https://docs.stripe.com/reports/reporting-categories.md>.
pub fn stripe_category(reporting_category: &str, kind: &str) -> Category {
    match reporting_category {
        "charge" | "partial_capture_reversal" => Category::Sale,
        "refund" | "refund_failure" => Category::Refund,
        "fee" => Category::Fee,
        "dispute" => Category::Dispute,
        "dispute_reversal" => Category::DisputeReversal,
        "payout" | "payout_reversal" => Category::Transfer,
        "risk_reserved_funds" | "connect_reserved_funds" => Category::Reserve,
        "tax" => Category::Tax,
        "other_adjustment" => Category::Adjustment,
        _ => match kind {
            "charge" | "payment" => Category::Sale,
            "refund" | "payment_refund" | "refund_failure" => Category::Refund,
            "stripe_fee" | "stripe_fx_fee" => Category::Fee,
            "tax_fee" => Category::Tax,
            "payout" | "payout_cancel" | "payout_failure" => Category::Transfer,
            "reserve_hold"
            | "reserve_release"
            | "reserved_funds"
            | "reserve_transaction"
            | "payment_network_reserve_hold"
            | "payment_network_reserve_release" => Category::Reserve,
            "adjustment" => Category::Adjustment,
            _ => Category::Other,
        },
    }
}

/// A Stripe balance transaction as an entry (amounts in minor units; `fee` is the
/// fee taken, `net` = `amount` − `fee`).
pub fn stripe_entry(v: &Value) -> ImportResult<Entry> {
    let id = s(v, "id");
    if id.is_empty() {
        return Err(invalid("a Stripe balance transaction has no id"));
    }
    let currency = s(v, "currency").to_ascii_uppercase();
    let kind = s(v, "type");
    Ok(Entry {
        category: stripe_category(&s(v, "reporting_category"), &kind),
        gross: minor(int(v, "amount")?, &currency),
        fee: minor(int(v, "fee")?, &currency),
        created: int(v, "created")?,
        description: clean(&s(v, "description")),
        currency,
        kind,
        id,
        note: None,
    })
}

/// A Stripe Payout object (<https://docs.stripe.com/api/payouts/object>): `amount` is
/// what is sent to the bank (minor units), `arrival_date` when it is expected, `status`
/// one of `paid`, `pending`, `in_transit`, `canceled`, `failed`, and `failure_code`
/// says why a payout failed.
pub fn stripe_payout(v: &Value) -> ImportResult<Payout> {
    let id = s(v, "id");
    if !id.starts_with("po_") {
        return Err(invalid("a Stripe payout has no payout id"));
    }
    let currency = s(v, "currency").to_ascii_uppercase();
    let created = int(v, "created")?;
    let arrival = match int(v, "arrival_date")? {
        0 => created,
        t => t,
    };
    Ok(Payout {
        net: minor(int(v, "amount")?, &currency),
        status: s(v, "status").to_lowercase(),
        failure_code: Some(clean(&s(v, "failure_code"))).filter(|c| !c.is_empty()),
        id,
        arrival,
        created,
        currency,
        location: None,
        location_name: None,
        entries: Vec::new(),
    })
}

// ----- Square ----------------------------------------------------------------------------

/// A Square list page: the objects under `key` and the next `cursor`.
pub fn square_page(body: &[u8], key: &str) -> ImportResult<(Vec<Value>, Option<String>)> {
    let v = parse(body)?;
    let items = v
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let cursor = v
        .get("cursor")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .map(str::to_owned);
    Ok((items, cursor))
}

/// Category of a Square payout entry type
/// (<https://developer.squareup.com/docs/payouts-api/list-payout-entries>).
pub fn square_category(kind: &str) -> Category {
    match kind {
        "CHARGE" => Category::Sale,
        "REFUND" => Category::Refund,
        "FEE"
        | "PROCESSING_FEE"
        | "PROCESSING_FEE_REFUND"
        | "UNDO_PROCESSING_FEE_REFUND"
        | "DEPOSIT_FEE"
        | "DEPOSIT_FEE_REVERSED"
        | "PAID_SERVICE_FEE"
        | "PAID_SERVICE_FEE_REFUND"
        | "SUBSCRIPTION_FEE"
        | "SUBSCRIPTION_FEE_REFUND"
        | "SUBSCRIPTION_FEE_PAID_REFUND"
        | "THIRD_PARTY_FEE"
        | "THIRD_PARTY_FEE_REFUND"
        | "GIFT_CARD_LOAD_FEE"
        | "GIFT_CARD_LOAD_FEE_REFUND"
        | "UNDO_GIFT_CARD_LOAD_FEE_REFUND"
        | "LOCAL_OFFERS_FEE"
        | "FREE_PROCESSING" => Category::Fee,
        "DISPUTE" | "OPEN_DISPUTE" => Category::Dispute,
        "HOLD_ADJUSTMENT" | "RELEASE_ADJUSTMENT" | "RESERVE_HOLD" | "RESERVE_RELEASE" => {
            Category::Reserve
        }
        "TAX_ON_FEE" => Category::Tax,
        "ADJUSTMENT" | "OTHER_ADJUSTMENT" => Category::Adjustment,
        "PAYOUT" => Category::Transfer,
        _ => Category::Other,
    }
}

fn money(v: &Value, k: &str) -> ImportResult<(Decimal, String)> {
    let m = v.get(k).cloned().unwrap_or(Value::Null);
    let currency = s(&m, "currency_code").to_ascii_uppercase();
    Ok((minor(int(&m, "amount")?, &currency), currency))
}

fn rfc3339_epoch(t: &str) -> i64 {
    time::OffsetDateTime::parse(t.trim(), &time::format_description::well_known::Rfc3339)
        .map_or(0, time::OffsetDateTime::unix_timestamp)
}

fn date_epoch(t: &str) -> i64 {
    let fmt = time::macros::format_description!("[year]-[month]-[day]");
    time::Date::parse(t.trim(), &fmt).map_or(0, |d| d.midnight().assume_utc().unix_timestamp())
}

/// A Square payout (entries are read separately).
pub fn square_payout(v: &Value) -> ImportResult<Payout> {
    let id = s(v, "id");
    if id.is_empty() {
        return Err(invalid("a Square payout has no id"));
    }
    let (net, currency) = money(v, "amount_money")?;
    let arrival = match date_epoch(&s(v, "arrival_date")) {
        0 => rfc3339_epoch(&s(v, "created_at")),
        t => t,
    };
    Ok(Payout {
        id,
        arrival,
        created: rfc3339_epoch(&s(v, "created_at")),
        failure_code: None,
        currency,
        net,
        status: s(v, "status").to_lowercase(),
        location: Some(s(v, "location_id")).filter(|x| !x.is_empty()),
        location_name: None,
        entries: Vec::new(),
    })
}

/// A Square payout entry from `gross_amount_money`, `fee_amount_money` and
/// `net_amount_money`; gross − fee must equal net, else the entry is noted for review.
/// Without a fee amount the fee is gross − net.
pub fn square_entry(v: &Value) -> ImportResult<Entry> {
    let id = s(v, "id");
    if id.is_empty() {
        return Err(invalid("a Square payout entry has no id"));
    }
    let kind = s(v, "type");
    let (gross, currency) = money(v, "gross_amount_money")?;
    let net = match v.get("net_amount_money") {
        Some(_) => money(v, "net_amount_money")?.0,
        None => gross,
    };
    let fee = match v.get("fee_amount_money") {
        Some(Value::Object(_)) => money(v, "fee_amount_money")?.0,
        _ => gross.checked_sub(net).unwrap_or_default(),
    };
    let note = (gross.checked_sub(fee) != Some(net))
        .then(|| format!("Square entry {id}: gross {gross} minus fee {fee} is not the net {net}"));
    let payment = v
        .as_object()
        .and_then(|o| {
            o.iter()
                .find(|(k, _)| k.starts_with("type_") && k.ends_with("_details"))
                .map(|(_, d)| s(d, "payment_id"))
        })
        .unwrap_or_default();
    let description = if payment.is_empty() {
        kind.clone()
    } else {
        format!("{kind} {payment}")
    };
    Ok(Entry {
        category: square_category(&kind),
        fee,
        gross,
        created: rfc3339_epoch(&s(v, "effective_at")),
        currency,
        description: clean(&description),
        kind,
        id,
        note,
    })
}

// ----- PayPal ----------------------------------------------------------------------------

/// The access token of a client-credentials answer (never stored).
pub fn paypal_token(body: &[u8]) -> ImportResult<String> {
    let v = parse(body)?;
    let t = s(&v, "access_token");
    if t.is_empty() {
        return Err(invalid("PayPal sent no access token"));
    }
    Ok(t)
}

/// PayPal withdrawals to the bank: the only codes that end a payout group.
pub const PAYPAL_WITHDRAWALS: [&str; 3] = ["T0400", "T0401", "T0403"];

/// Category of a PayPal transaction event code, and why it needs review when it does.
/// Event codes (<https://developer.paypal.com/docs/transaction-search/transaction-event-codes/>),
/// as tabled in connectivity-processors.md revision 2 (R2.3):
///
/// | Code(s) | Category | Note |
/// |---|---|---|
/// | T0000–T0099, gross > 0 | sale | received payments |
/// | T0000–T0099, gross ≤ 0 | other | money sent from the balance; review |
/// | T0100–T0199 | fee | |
/// | T0200–T0299 | other | currency conversion: multi-currency needed |
/// | T0400, T0401, T0403 | transfer | withdrawal to the bank (payout boundary) |
/// | other T04xx | transfer | review |
/// | T1106 | dispute | reversal initiated by PayPal |
/// | T1107 | refund | refund initiated by the merchant |
/// | T1108, T1109 | fee | fee reversal / refund (a positive amount reduces fees) |
/// | other T11xx | refund | review |
/// | T1201 | dispute | chargeback |
/// | T1202 | dispute_reversal | chargeback reversal |
/// | other T12xx | dispute | review |
/// | T15xx, T21xx | reserve | holds and releases |
/// | anything else | other | review |
pub fn paypal_category(code: &str, gross: Decimal) -> (Category, Option<String>) {
    let group = code.get(1..3).unwrap_or("");
    let review = |why: &str| Some(format!("PayPal event {code}: {why}"));
    match (group, code) {
        ("00", _) if gross > Decimal::ZERO => (Category::Sale, None),
        ("00", _) => (
            Category::Other,
            review("money sent from the PayPal balance"),
        ),
        ("01", _) => (Category::Fee, None),
        ("02", _) => (
            Category::Other,
            review("currency conversion; multi-currency needed"),
        ),
        (_, c) if PAYPAL_WITHDRAWALS.contains(&c) => (Category::Transfer, None),
        ("04", _) => (
            Category::Transfer,
            review("a transfer that is not a withdrawal to the bank"),
        ),
        (_, "T1106") => (Category::Dispute, None),
        (_, "T1107") => (Category::Refund, None),
        (_, "T1108" | "T1109") => (Category::Fee, None),
        ("11", _) => (Category::Refund, review("a reversal of another kind")),
        (_, "T1201") => (Category::Dispute, None),
        (_, "T1202") => (Category::DisputeReversal, None),
        ("12", _) => (Category::Dispute, review("an adjustment of another kind")),
        ("15" | "21", _) => (Category::Reserve, None),
        _ => (Category::Other, review("not a mapped event code")),
    }
}

/// A PayPal transaction search page: entries (successful ones only) and total pages.
pub fn paypal_page(body: &[u8]) -> ImportResult<(Vec<Entry>, i64, Vec<String>)> {
    let v = parse(body)?;
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for d in v
        .get("transaction_details")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let t = d.get("transaction_info").cloned().unwrap_or(Value::Null);
        let id = s(&t, "transaction_id");
        if id.is_empty() {
            continue;
        }
        let status = s(&t, "transaction_status");
        if !status.is_empty() && status != "S" {
            if status == "P" {
                warnings.push(format!("PayPal transaction {id} is still pending"));
            }
            continue;
        }
        let amount = t.get("transaction_amount").cloned().unwrap_or(Value::Null);
        let gross = decimal_text(&s(&amount, "value"), &id)?;
        let fee_v = t
            .get("fee_amount")
            .map(|f| s(f, "value"))
            .unwrap_or_default();
        // A fee is reported as a negative amount.
        let fee = if fee_v.trim().is_empty() {
            Decimal::ZERO
        } else {
            -decimal_text(&fee_v, &id)?
        };
        let code = s(&t, "transaction_event_code");
        let mut description = s(&t, "transaction_subject");
        for extra in [s(&t, "invoice_id"), s(&t, "transaction_note")] {
            if !extra.is_empty() {
                if !description.is_empty() {
                    description.push(' ');
                }
                description.push_str(&extra);
            }
        }
        let (category, note) = paypal_category(&code, gross);
        out.push(Entry {
            category,
            note,
            created: rfc3339_epoch(&s(&t, "transaction_initiation_date")),
            currency: s(&amount, "currency_code").to_ascii_uppercase(),
            description: clean(&description),
            kind: code,
            gross,
            fee,
            id,
        });
    }
    Ok((out, int(&v, "total_pages")?.max(1), warnings))
}

/// PayPal entries grouped: payouts ending at each withdrawal to the bank (T0400, T0401,
/// T0403 only), the entries after the last withdrawal (pending payout), and other
/// transfers (other T04xx), which are staged on their own.
pub fn paypal_group(mut entries: Vec<Entry>) -> (Vec<Payout>, Vec<Entry>, Vec<Entry>) {
    entries.sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
    let mut payouts = Vec::new();
    let mut current = Vec::new();
    let mut alone = Vec::new();
    for e in entries {
        if e.category == Category::Transfer
            && e.gross < Decimal::ZERO
            && PAYPAL_WITHDRAWALS.contains(&e.kind.as_str())
        {
            payouts.push(Payout {
                id: e.id.clone(),
                arrival: e.created,
                created: e.created,
                failure_code: None,
                currency: e.currency.clone(),
                net: -e.gross,
                status: "paid".into(),
                location: None,
                location_name: None,
                entries: std::mem::take(&mut current),
            });
        } else if e.category == Category::Transfer {
            alone.push(e);
        } else {
            current.push(e);
        }
    }
    (payouts, current, alone)
}

/// Search windows of at most 31 days covering `[from, to)` (epoch seconds).
pub fn paypal_windows(from: i64, to: i64) -> Vec<(i64, i64)> {
    let step = PAYPAL_WINDOW_DAYS * 86_400;
    let mut out = Vec::new();
    let mut a = from;
    while a < to {
        let b = a.saturating_add(step).min(to);
        out.push((a, b));
        a = b;
    }
    out
}

/// RFC 3339 text (`2024-01-01T00:00:00Z`) of epoch seconds.
pub fn rfc3339(t: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(t)
        .ok()
        .and_then(|d| {
            d.format(time::macros::format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
            ))
            .ok()
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn stripe_keys() {
        assert!(check_stripe_key("rk_live_abcdefghijklmnop").is_ok());
        assert!(
            check_stripe_key("sk_live_abcdefghijklmnop")
                .unwrap_err()
                .to_string()
                .contains("restricted key")
        );
        assert!(check_stripe_key("pk_live_abcdefghijk").is_err());
        assert!(check_stripe_key("hello").is_err());
    }

    #[test]
    fn minor_units_and_windows() {
        assert_eq!(minor(94530, "usd").to_string(), "945.30");
        assert_eq!(minor(500, "JPY").to_string(), "500");
        let w = paypal_windows(0, 40 * 86_400);
        assert_eq!(w.len(), 2);
        assert!(w.iter().all(|(a, b)| b - a <= 31 * 86_400));
        assert_eq!(rfc3339(1_790_812_800), "2026-10-01T00:00:00Z");
    }

    #[test]
    fn categories() {
        assert_eq!(stripe_category("charge", "charge"), Category::Sale);
        assert_eq!(stripe_category("", "stripe_fee"), Category::Fee);
        assert_eq!(square_category("CHARGE"), Category::Sale);
        assert_eq!(square_category("PROCESSING_FEE"), Category::Fee);
        let e = square_entry(&serde_json::json!({"id": "e1", "type": "CHARGE",
            "gross_amount_money": {"amount": 1000, "currency_code": "USD"},
            "fee_amount_money": {"amount": 30, "currency_code": "USD"},
            "net_amount_money": {"amount": 960, "currency_code": "USD"}}))
        .unwrap();
        assert_eq!(e.fee.to_string(), "0.30");
        assert!(e.note.unwrap().contains("not the net"));
        let cat = |c: &str, g: Decimal| paypal_category(c, g).0;
        assert_eq!(cat("T0006", Decimal::ONE_HUNDRED), Category::Sale);
        assert_eq!(cat("T0006", -Decimal::ONE), Category::Other);
        assert_eq!(cat("T0400", -Decimal::ONE), Category::Transfer);
        assert!(paypal_category("T0400", -Decimal::ONE).1.is_none());
        assert!(paypal_category("T0402", -Decimal::ONE).1.is_some());
        assert_eq!(cat("T1107", -Decimal::ONE), Category::Refund);
        assert_eq!(cat("T1108", Decimal::ONE), Category::Fee);
        assert_eq!(cat("T1202", Decimal::ONE), Category::DisputeReversal);
        assert!(
            paypal_category("T0200", Decimal::ONE)
                .1
                .unwrap()
                .contains("multi-currency needed")
        );
        assert_eq!(cat("T1599", Decimal::ONE), Category::Reserve);
        assert_eq!(cat("T9999", Decimal::ONE), Category::Other);
    }
}
