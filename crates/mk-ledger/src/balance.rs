//! Balance arithmetic: per-commodity sums with overflow checks.

use std::collections::BTreeMap;

use mk_core::{CommodityCode, Decimal};

use crate::{LedgerError, LedgerResult};

/// Amount per commodity. Zero totals are removed by [`Balances::non_zero`].
pub type Balances = BTreeMap<CommodityCode, Decimal>;

/// Add `amount` of `commodity` to `totals`.
pub fn add_to(
    totals: &mut Balances,
    commodity: &CommodityCode,
    amount: Decimal,
) -> LedgerResult<()> {
    let entry = totals.entry(commodity.clone()).or_insert(Decimal::ZERO);
    *entry = entry.checked_add(amount).ok_or(LedgerError::Overflow)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_per_commodity() {
        let usd = CommodityCode::new("USD").unwrap();
        let vti = CommodityCode::new("VTI").unwrap();
        let mut b = Balances::new();
        add_to(&mut b, &usd, Decimal::new(1000, 2)).unwrap();
        add_to(&mut b, &vti, Decimal::new(3, 0)).unwrap();
        add_to(&mut b, &usd, Decimal::new(-250, 2)).unwrap();
        assert_eq!(b[&usd], Decimal::new(750, 2));
        assert_eq!(b[&vti], Decimal::new(3, 0));
    }

    #[test]
    fn overflow_is_an_error() {
        let usd = CommodityCode::new("USD").unwrap();
        let mut b = Balances::new();
        add_to(&mut b, &usd, Decimal::MAX).unwrap();
        assert_eq!(
            add_to(&mut b, &usd, Decimal::MAX),
            Err(LedgerError::Overflow)
        );
    }
}
