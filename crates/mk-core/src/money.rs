use std::fmt;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{CommodityCode, CoreError, CoreResult};

/// An exact amount of one commodity. Never constructed from or converted to floats.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Money {
    amount: Decimal,
    commodity: CommodityCode,
}

impl Money {
    pub fn new(amount: Decimal, commodity: CommodityCode) -> Self {
        Self { amount, commodity }
    }

    pub fn zero(commodity: CommodityCode) -> Self {
        Self::new(Decimal::ZERO, commodity)
    }

    /// Parse an amount such as `"-1234.56"` in the given commodity.
    pub fn parse(amount: &str, commodity: CommodityCode) -> CoreResult<Self> {
        amount
            .trim()
            .parse::<Decimal>()
            .map(|a| Self::new(a, commodity))
            .map_err(|_| CoreError::InvalidAmount(amount.to_owned()))
    }

    pub fn amount(&self) -> Decimal {
        self.amount
    }

    pub fn commodity(&self) -> &CommodityCode {
        &self.commodity
    }

    pub fn is_zero(&self) -> bool {
        self.amount.is_zero()
    }

    pub fn checked_add(&self, other: &Money) -> CoreResult<Money> {
        self.same_commodity(other)?;
        self.amount
            .checked_add(other.amount)
            .map(|a| Money::new(a, self.commodity.clone()))
            .ok_or(CoreError::Overflow)
    }

    pub fn checked_sub(&self, other: &Money) -> CoreResult<Money> {
        self.same_commodity(other)?;
        self.amount
            .checked_sub(other.amount)
            .map(|a| Money::new(a, self.commodity.clone()))
            .ok_or(CoreError::Overflow)
    }

    pub fn negate(&self) -> Money {
        Money::new(-self.amount, self.commodity.clone())
    }

    fn same_commodity(&self, other: &Money) -> CoreResult<()> {
        if self.commodity == other.commodity {
            Ok(())
        } else {
            Err(CoreError::CommodityMismatch {
                left: self.commodity.to_string(),
                right: other.commodity.to_string(),
            })
        }
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.amount, self.commodity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usd(s: &str) -> Money {
        Money::parse(s, CommodityCode::new("USD").unwrap()).unwrap()
    }

    #[test]
    fn exact_decimal_addition() {
        // The classic float trap: 0.1 + 0.2 must equal exactly 0.3.
        assert_eq!(usd("0.1").checked_add(&usd("0.2")).unwrap(), usd("0.3"));
    }

    #[test]
    fn subtraction_and_negation() {
        assert_eq!(
            usd("10.00").checked_sub(&usd("12.50")).unwrap(),
            usd("-2.50")
        );
        assert_eq!(usd("5").negate(), usd("-5"));
    }

    #[test]
    fn rejects_mixed_commodities() {
        let eur = Money::parse("1", CommodityCode::new("EUR").unwrap()).unwrap();
        assert!(matches!(
            usd("1").checked_add(&eur),
            Err(CoreError::CommodityMismatch { .. })
        ));
    }

    #[test]
    fn rejects_bad_amounts() {
        let usd_code = CommodityCode::new("USD").unwrap();
        assert!(Money::parse("12.3.4", usd_code.clone()).is_err());
        assert!(Money::parse("abc", usd_code).is_err());
    }

    #[test]
    fn overflow_is_an_error_not_a_panic() {
        let max = Money::new(Decimal::MAX, CommodityCode::new("USD").unwrap());
        assert_eq!(max.checked_add(&usd("1")), Err(CoreError::Overflow));
    }
}
