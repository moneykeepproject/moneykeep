//! Lots and cost basis (spec: investments-core.md, "Lots and cost basis").
//!
//! Pure replay: the command layer feeds one security account's investment events in
//! register order and reads back each disposal's consumed lot portions and the open
//! lots. Nothing here is stored; lots are derived data.

use mk_core::{Date, Decimal, RoundingStrategy};

use crate::LedgerError;

/// Money decimals (base currency amounts in 3a).
const MONEY_DP: u32 = 2;
/// Share quantities keep at most this many decimals.
pub const QUANTITY_DP: u32 = 9;

/// How a disposal picks its shares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Fifo,
    Average,
    Specific,
}

/// One open (or partly consumed) lot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lot {
    pub id: String,
    pub acquired: Date,
    /// Creation order, for ties between lots acquired the same day.
    pub seq: usize,
    pub original: Decimal,
    pub remaining: Decimal,
    /// Remaining basis.
    pub basis: Decimal,
    /// The transaction that created the lot.
    pub txn: String,
}

/// A lot named in a specific-identification disposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub lot: String,
    pub quantity: Decimal,
}

/// One investment event of a security account.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Acquire {
        lot: String,
        acquired: Date,
        quantity: Decimal,
        basis: Decimal,
        txn: String,
    },
    /// A sale (`proceeds` Some) or shares moved out (None: no gain).
    Dispose {
        date: Date,
        quantity: Decimal,
        proceeds: Option<Decimal>,
        method: Method,
        selections: Option<Vec<Selection>>,
    },
    Split {
        new: Decimal,
        old: Decimal,
    },
    /// A return of capital; beyond the open basis the excess is a capital gain per
    /// lot (investments-advanced.md), dated `date` for the holding period.
    ReturnOfCapital {
        amount: Decimal,
        date: Date,
    },
    /// A spin-off: each open lot gives up `percent` % of its basis (rounded to
    /// cents) to a new security; the moved parts come back as portions.
    BasisMove {
        percent: Decimal,
    },
}

/// A consumed part of one lot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Portion {
    pub lot: String,
    pub acquired: Date,
    pub quantity: Decimal,
    pub basis: Decimal,
    pub proceeds: Decimal,
    pub long_term: bool,
}

impl Portion {
    pub fn gain(&self) -> Decimal {
        self.proceeds.checked_sub(self.basis).unwrap_or_default()
    }
}

/// What a disposal consumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disposal {
    pub basis: Decimal,
    pub portions: Vec<Portion>,
    /// Average-method disposal (reports show the acquired date as "Various").
    pub average: bool,
}

/// What applying an event produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    None,
    Disposal(Disposal),
    /// Shares added (or removed, if negative) by a split.
    SplitDelta(Decimal),
    /// Basis moved out of each lot (quantity = the lot's remaining shares,
    /// proceeds zero).
    Moved(Vec<Portion>),
}

/// Why an event cannot apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LotError {
    /// More shares than held: the quantity held at that point.
    Insufficient {
        held: Decimal,
    },
    /// A selection names a lot that is not open, or asks too much of it.
    BadSelection(String),
    /// A return of capital larger than the open basis.
    ExceedsBasis {
        open: Decimal,
    },
    Invalid(String),
    Overflow,
}

impl From<LedgerError> for LotError {
    fn from(e: LedgerError) -> Self {
        match e {
            LedgerError::Overflow => LotError::Overflow,
            LedgerError::Invalid(m) | LedgerError::Rejected(m) => LotError::Invalid(m),
        }
    }
}

type LotResult<T> = Result<T, LotError>;

/// Round half away from zero.
pub fn round(v: Decimal, dp: u32) -> Decimal {
    v.round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero)
}

/// Round money to cents, half away from zero.
pub fn money(v: Decimal) -> Decimal {
    round(v, MONEY_DP)
}

fn add(a: Decimal, b: Decimal) -> LotResult<Decimal> {
    a.checked_add(b).ok_or(LotError::Overflow)
}

fn sub(a: Decimal, b: Decimal) -> LotResult<Decimal> {
    a.checked_sub(b).ok_or(LotError::Overflow)
}

fn mul(a: Decimal, b: Decimal) -> LotResult<Decimal> {
    a.checked_mul(b).ok_or(LotError::Overflow)
}

fn div(a: Decimal, b: Decimal) -> LotResult<Decimal> {
    if b.is_zero() {
        return Err(LotError::Invalid("division by zero".into()));
    }
    a.checked_div(b).ok_or(LotError::Overflow)
}

/// `round(total × part ÷ whole, 2)`.
fn share_of(total: Decimal, part: Decimal, whole: Decimal) -> LotResult<Decimal> {
    Ok(money(div(mul(total, part)?, whole)?))
}

/// The first day a lot acquired on `acquired` is long-term: the day after its
/// anniversary one year on (Feb 29 → Feb 28 of the next year).
pub fn is_long_term(acquired: Date, sold: Date) -> bool {
    match Date::from_ymd_clamped(
        acquired.year().saturating_add(1),
        acquired.month(),
        acquired.day(),
    ) {
        Ok(anniversary) => sold > anniversary,
        Err(_) => false,
    }
}

/// Open lots of one security account, changed by replaying its events in order.
#[derive(Debug, Clone, Default)]
pub struct LotState {
    lots: Vec<Lot>,
    next_seq: usize,
}

impl LotState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open lots, oldest acquired first (ties by creation order).
    pub fn open(&self) -> Vec<&Lot> {
        let mut v: Vec<&Lot> = self
            .lots
            .iter()
            .filter(|l| !l.remaining.is_zero())
            .collect();
        v.sort_by_key(|l| (l.acquired, l.seq));
        v
    }

    pub fn held(&self) -> Decimal {
        self.lots.iter().fold(Decimal::ZERO, |t, l| {
            t.checked_add(l.remaining).unwrap_or(t)
        })
    }

    pub fn basis(&self) -> Decimal {
        self.lots
            .iter()
            .fold(Decimal::ZERO, |t, l| t.checked_add(l.basis).unwrap_or(t))
    }

    /// Indices of open lots in consumption order (oldest acquired first).
    fn order(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.lots.len())
            .filter(|i| self.lots.get(*i).is_some_and(|l| !l.remaining.is_zero()))
            .collect();
        idx.sort_by_key(|i| self.lots.get(*i).map(|l| (l.acquired, l.seq)));
        idx
    }

    pub fn apply(&mut self, event: &Event) -> LotResult<Outcome> {
        match event {
            Event::Acquire {
                lot,
                acquired,
                quantity,
                basis,
                txn,
            } => {
                if quantity.is_sign_negative() || quantity.is_zero() {
                    return Err(LotError::Invalid("a lot needs a positive quantity".into()));
                }
                let seq = self.next_seq;
                self.next_seq = self.next_seq.saturating_add(1);
                self.lots.push(Lot {
                    id: lot.clone(),
                    acquired: *acquired,
                    seq,
                    original: *quantity,
                    remaining: *quantity,
                    basis: *basis,
                    txn: txn.clone(),
                });
                Ok(Outcome::None)
            }
            Event::Dispose {
                date,
                quantity,
                proceeds,
                method,
                selections,
            } => self
                .dispose(*date, *quantity, *proceeds, *method, selections.as_deref())
                .map(Outcome::Disposal),
            Event::Split { new, old } => self.split(*new, *old).map(Outcome::SplitDelta),
            Event::ReturnOfCapital { amount, date } => self.return_of_capital(*amount, *date),
            Event::BasisMove { percent } => self.basis_move(*percent).map(Outcome::Moved),
        }
    }

    fn dispose(
        &mut self,
        date: Date,
        quantity: Decimal,
        proceeds: Option<Decimal>,
        method: Method,
        selections: Option<&[Selection]>,
    ) -> LotResult<Disposal> {
        if quantity.is_sign_negative() || quantity.is_zero() {
            return Err(LotError::Invalid("the quantity must be positive".into()));
        }
        let held = self.held();
        if quantity > held {
            return Err(LotError::Insufficient { held });
        }
        // (lot index, quantity, basis) in consumption order.
        let mut taken: Vec<(usize, Decimal, Decimal)> = Vec::new();
        let mut average = false;
        let mut total_basis = Decimal::ZERO;
        match (selections, method) {
            (Some(sel), _) => {
                let mut sum = Decimal::ZERO;
                for s in sel {
                    sum = add(sum, s.quantity)?;
                }
                if sum != quantity {
                    return Err(LotError::BadSelection(format!(
                        "the chosen lots total {sum} shares, not {quantity}"
                    )));
                }
                for (k, s) in sel.iter().enumerate() {
                    if sel.iter().take(k).any(|p| p.lot == s.lot) {
                        return Err(LotError::BadSelection(format!(
                            "lot {} is chosen twice",
                            s.lot
                        )));
                    }
                    if s.quantity.is_sign_negative() || s.quantity.is_zero() {
                        return Err(LotError::BadSelection(format!(
                            "lot {}: the quantity must be positive",
                            s.lot
                        )));
                    }
                    let i = self
                        .lots
                        .iter()
                        .position(|l| l.id == s.lot && !l.remaining.is_zero())
                        .ok_or_else(|| {
                            LotError::BadSelection(format!("lot {} is not open", s.lot))
                        })?;
                    let lot = self.lots.get(i).ok_or(LotError::Overflow)?;
                    if s.quantity > lot.remaining {
                        return Err(LotError::BadSelection(format!(
                            "lot {} has {} shares, fewer than {}",
                            s.lot, lot.remaining, s.quantity
                        )));
                    }
                    let b = if s.quantity == lot.remaining {
                        lot.basis
                    } else {
                        share_of(lot.basis, s.quantity, lot.remaining)?
                    };
                    taken.push((i, s.quantity, b));
                    total_basis = add(total_basis, b)?;
                }
            }
            (None, Method::Specific) => {
                return Err(LotError::BadSelection(
                    "this security uses specific identification: choose the lots to sell".into(),
                ));
            }
            (None, Method::Fifo) => {
                let mut left = quantity;
                for i in self.order() {
                    if left.is_zero() {
                        break;
                    }
                    let lot = self.lots.get(i).ok_or(LotError::Overflow)?;
                    let q = left.min(lot.remaining);
                    let b = if q == lot.remaining {
                        lot.basis
                    } else {
                        share_of(lot.basis, q, lot.remaining)?
                    };
                    taken.push((i, q, b));
                    total_basis = add(total_basis, b)?;
                    left = sub(left, q)?;
                }
            }
            (None, Method::Average) => {
                average = true;
                let avg = div(self.basis(), held)?;
                total_basis = money(mul(avg, quantity)?);
                let mut left = quantity;
                let mut assigned = Decimal::ZERO;
                let order = self.order();
                let mut picks: Vec<(usize, Decimal)> = Vec::new();
                for i in order {
                    if left.is_zero() {
                        break;
                    }
                    let lot = self.lots.get(i).ok_or(LotError::Overflow)?;
                    let q = left.min(lot.remaining);
                    picks.push((i, q));
                    left = sub(left, q)?;
                }
                let n = picks.len();
                for (k, (i, q)) in picks.into_iter().enumerate() {
                    let b = if k + 1 == n {
                        sub(total_basis, assigned)?
                    } else {
                        money(mul(avg, q)?)
                    };
                    assigned = add(assigned, b)?;
                    taken.push((i, q, b));
                }
            }
        }
        // Proceeds per portion in consumption order, the last taking the remainder.
        let total_proceeds = proceeds.unwrap_or_default();
        let n = taken.len();
        let mut given = Decimal::ZERO;
        let mut portions = Vec::with_capacity(n);
        for (k, (i, q, b)) in taken.iter().enumerate() {
            let p = if proceeds.is_none() {
                Decimal::ZERO
            } else if k + 1 == n {
                sub(total_proceeds, given)?
            } else {
                share_of(total_proceeds, *q, quantity)?
            };
            given = add(given, p)?;
            let lot = self.lots.get(*i).ok_or(LotError::Overflow)?;
            portions.push(Portion {
                lot: lot.id.clone(),
                acquired: lot.acquired,
                quantity: *q,
                basis: *b,
                proceeds: p,
                long_term: is_long_term(lot.acquired, date),
            });
        }
        // Consume.
        if average {
            let avg = div(self.basis(), held)?;
            for (i, q, _) in &taken {
                if let Some(lot) = self.lots.get_mut(*i) {
                    lot.remaining = sub(lot.remaining, *q)?;
                }
            }
            // Re-base every remaining lot to the average; the last takes the remainder.
            let remaining_basis = sub(self.basis(), total_basis)?;
            let order = self.order();
            let n = order.len();
            let mut assigned = Decimal::ZERO;
            for (k, i) in order.into_iter().enumerate() {
                if let Some(lot) = self.lots.get_mut(i) {
                    lot.basis = if k + 1 == n {
                        sub(remaining_basis, assigned)?
                    } else {
                        money(mul(avg, lot.remaining)?)
                    };
                    assigned = add(assigned, lot.basis)?;
                }
            }
            for lot in self.lots.iter_mut().filter(|l| l.remaining.is_zero()) {
                lot.basis = Decimal::ZERO;
            }
        } else {
            for (i, q, b) in &taken {
                if let Some(lot) = self.lots.get_mut(*i) {
                    lot.remaining = sub(lot.remaining, *q)?;
                    lot.basis = sub(lot.basis, *b)?;
                }
            }
        }
        Ok(Disposal {
            basis: total_basis,
            portions,
            average,
        })
    }

    fn split(&mut self, new: Decimal, old: Decimal) -> LotResult<Decimal> {
        if new.is_sign_negative() || new.is_zero() || old.is_sign_negative() || old.is_zero() {
            return Err(LotError::Invalid("a split ratio must be positive".into()));
        }
        let before = self.held();
        for lot in self.lots.iter_mut().filter(|l| !l.remaining.is_zero()) {
            let q = round(div(mul(lot.remaining, new)?, old)?, QUANTITY_DP);
            lot.remaining = q;
        }
        sub(self.held(), before)
    }

    /// Reduce basis pro rata by basis; an amount above the open basis takes every
    /// lot's basis to zero and the excess is a gain, shared by quantity, returned as
    /// a disposal of zero shares.
    fn return_of_capital(&mut self, amount: Decimal, date: Date) -> LotResult<Outcome> {
        if amount.is_sign_negative() || amount.is_zero() {
            return Err(LotError::Invalid("the amount must be positive".into()));
        }
        let open = self.basis();
        let order = self.order();
        if order.is_empty() {
            return Err(LotError::Insufficient {
                held: Decimal::ZERO,
            });
        }
        if amount <= open {
            let n = order.len();
            let mut given = Decimal::ZERO;
            for (k, i) in order.into_iter().enumerate() {
                let Some(lot) = self.lots.get_mut(i) else {
                    continue;
                };
                let cut = if k + 1 == n {
                    sub(amount, given)?
                } else {
                    share_of(amount, lot.basis, open)?
                };
                given = add(given, cut)?;
                lot.basis = sub(lot.basis, cut)?;
            }
            return Ok(Outcome::None);
        }
        let excess = sub(amount, open)?;
        let held = self.held();
        let n = order.len();
        let mut given = Decimal::ZERO;
        let mut portions = Vec::with_capacity(n);
        for (k, i) in order.into_iter().enumerate() {
            let Some(lot) = self.lots.get_mut(i) else {
                continue;
            };
            let gain = if k + 1 == n {
                sub(excess, given)?
            } else {
                share_of(excess, lot.remaining, held)?
            };
            given = add(given, gain)?;
            lot.basis = Decimal::ZERO;
            portions.push(Portion {
                lot: lot.id.clone(),
                acquired: lot.acquired,
                quantity: Decimal::ZERO,
                basis: Decimal::ZERO,
                proceeds: gain,
                long_term: is_long_term(lot.acquired, date),
            });
        }
        Ok(Outcome::Disposal(Disposal {
            basis: open,
            portions,
            average: false,
        }))
    }

    fn basis_move(&mut self, percent: Decimal) -> LotResult<Vec<Portion>> {
        if percent <= Decimal::ZERO || percent >= Decimal::ONE_HUNDRED {
            return Err(LotError::Invalid(
                "the allocation percentage must be more than 0 and less than 100".into(),
            ));
        }
        let mut out = Vec::new();
        for i in self.order() {
            let Some(lot) = self.lots.get_mut(i) else {
                continue;
            };
            let moved = money(div(mul(lot.basis, percent)?, Decimal::ONE_HUNDRED)?);
            lot.basis = sub(lot.basis, moved)?;
            out.push(Portion {
                lot: lot.id.clone(),
                acquired: lot.acquired,
                quantity: lot.remaining,
                basis: moved,
                proceeds: Decimal::ZERO,
                long_term: false,
            });
        }
        Ok(out)
    }
}

/// Convenience for callers that only need the ledger error type.
pub fn lot_error_message(e: &LotError) -> String {
    match e {
        LotError::Insufficient { held } => format!("not enough shares: {held} held"),
        LotError::BadSelection(m) | LotError::Invalid(m) => m.clone(),
        LotError::ExceedsBasis { open } => {
            format!("the return of capital exceeds the open basis {open}")
        }
        LotError::Overflow => "arithmetic overflow".into(),
    }
}

/// Map to a ledger error (for callers that do not distinguish lot errors).
pub fn to_ledger(e: &LotError) -> LedgerError {
    LedgerError::Rejected(lot_error_message(e))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn d(s: &str) -> Date {
        s.parse().unwrap()
    }
    fn n(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    /// The spec's common setup: lot A 10 @ 2,005.00 and lot B 5 @ 1,100.00.
    fn setup() -> LotState {
        let mut st = LotState::new();
        st.apply(&Event::Acquire {
            lot: "A".into(),
            acquired: d("2025-03-10"),
            quantity: n("10"),
            basis: n("2005.00"),
            txn: "a".into(),
        })
        .unwrap();
        st.apply(&Event::Acquire {
            lot: "B".into(),
            acquired: d("2025-11-20"),
            quantity: n("5"),
            basis: n("1100.00"),
            txn: "b".into(),
        })
        .unwrap();
        st
    }

    fn sell(st: &mut LotState, method: Method, sel: Option<Vec<Selection>>) -> Disposal {
        match st
            .apply(&Event::Dispose {
                date: d("2026-04-15"),
                quantity: n("12"),
                proceeds: Some(n("2995.00")),
                method,
                selections: sel,
            })
            .unwrap()
        {
            Outcome::Disposal(x) => x,
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn at1_fifo() {
        let mut st = setup();
        let x = sell(&mut st, Method::Fifo, None);
        let p: Vec<_> = x
            .portions
            .iter()
            .map(|p| (p.lot.as_str(), p.quantity, p.basis, p.proceeds, p.long_term))
            .collect();
        assert_eq!(
            p,
            vec![
                ("A", n("10"), n("2005.00"), n("2495.83"), true),
                ("B", n("2"), n("440.00"), n("499.17"), false)
            ]
        );
        assert_eq!(n("2995.00") - x.basis, n("550.00"));
        let b = st.open();
        assert_eq!(b.len(), 1);
        assert_eq!((b[0].remaining, b[0].basis), (n("3"), n("660.00")));
    }

    #[test]
    fn at2_average() {
        let mut st = setup();
        let x = sell(&mut st, Method::Average, None);
        assert!(x.average);
        assert_eq!(x.basis, n("2484.00"));
        let p: Vec<_> = x
            .portions
            .iter()
            .map(|p| (p.quantity, p.basis, p.proceeds, p.gain(), p.long_term))
            .collect();
        assert_eq!(
            p,
            vec![
                (n("10"), n("2070.00"), n("2495.83"), n("425.83"), true),
                (n("2"), n("414.00"), n("499.17"), n("85.17"), false)
            ]
        );
        assert_eq!(st.basis(), n("621.00"));
        assert_eq!(st.held(), n("3"));
    }

    #[test]
    fn at3_specific() {
        let mut st = setup();
        let x = sell(
            &mut st,
            Method::Fifo,
            Some(vec![
                Selection {
                    lot: "B".into(),
                    quantity: n("5"),
                },
                Selection {
                    lot: "A".into(),
                    quantity: n("7"),
                },
            ]),
        );
        let p: Vec<_> = x
            .portions
            .iter()
            .map(|p| (p.lot.as_str(), p.basis, p.proceeds, p.gain(), p.long_term))
            .collect();
        assert_eq!(
            p,
            vec![
                ("B", n("1100.00"), n("1247.92"), n("147.92"), false),
                ("A", n("1403.50"), n("1747.08"), n("343.58"), true)
            ]
        );
        assert_eq!(n("2995.00") - x.basis, n("491.50"));
        let a = st.open();
        assert_eq!(
            (a[0].id.as_str(), a[0].remaining, a[0].basis),
            ("A", n("3"), n("601.50"))
        );
    }

    #[test]
    fn at4_holding_period_and_leap_day() {
        assert!(!is_long_term(d("2025-03-10"), d("2026-03-10")));
        assert!(is_long_term(d("2025-03-10"), d("2026-03-11")));
        assert!(!is_long_term(d("2024-02-29"), d("2025-02-28")));
        assert!(is_long_term(d("2024-02-29"), d("2025-03-01")));
    }

    #[test]
    fn at7_insufficient_and_specific_needs_lots() {
        let mut st = setup();
        let e = st
            .apply(&Event::Dispose {
                date: d("2026-04-15"),
                quantity: n("20"),
                proceeds: Some(n("1")),
                method: Method::Fifo,
                selections: None,
            })
            .unwrap_err();
        assert_eq!(e, LotError::Insufficient { held: n("15") });
        assert!(matches!(
            st.apply(&Event::Dispose {
                date: d("2026-04-15"),
                quantity: n("1"),
                proceeds: Some(n("1")),
                method: Method::Specific,
                selections: None,
            }),
            Err(LotError::BadSelection(_))
        ));
    }

    #[test]
    fn at5_split_and_at8_return_of_capital() {
        let mut st = setup();
        sell(&mut st, Method::Fifo, None);
        st.apply(&Event::Acquire {
            lot: "C".into(),
            acquired: d("2026-09-30"),
            quantity: n("0.1"),
            basis: n("22.00"),
            txn: "c".into(),
        })
        .unwrap();
        // AT8: B (3 sh, 660.00) and C (0.1 sh, 22.00).
        let mut roc = st.clone();
        roc.apply(&Event::ReturnOfCapital {
            amount: n("30.00"),
            date: d("2026-10-01"),
        })
        .unwrap();
        let b: Vec<_> = roc.open().iter().map(|l| l.basis).collect();
        assert_eq!(b, vec![n("630.97"), n("21.03")]);
        // Beyond the open basis (682.00) the excess 18.00 is a gain, by quantity.
        let mut over = st.clone();
        let out = over
            .apply(&Event::ReturnOfCapital {
                amount: n("700"),
                date: d("2026-10-01"),
            })
            .unwrap();
        let Outcome::Disposal(dsp) = out else {
            panic!("expected a gain")
        };
        assert_eq!(dsp.basis, n("682.00"));
        let gains: Vec<_> = dsp.portions.iter().map(|p| p.proceeds).collect();
        assert_eq!(gains, vec![n("17.42"), n("0.58")]);
        assert!(over.open().iter().all(|l| l.basis.is_zero()));
        // Split 2-for-1: quantities double, basis unchanged.
        let delta = st
            .apply(&Event::Split {
                new: n("2"),
                old: n("1"),
            })
            .unwrap();
        assert_eq!(delta, Outcome::SplitDelta(n("3.1")));
        let q: Vec<_> = st.open().iter().map(|l| (l.remaining, l.basis)).collect();
        assert_eq!(q, vec![(n("6"), n("660.00")), (n("0.2"), n("22.00"))]);
    }

    #[test]
    fn partial_lot_sold_in_pieces_totals_its_basis() {
        let mut st = LotState::new();
        st.apply(&Event::Acquire {
            lot: "L".into(),
            acquired: d("2025-01-01"),
            quantity: n("3"),
            basis: n("100.00"),
            txn: "l".into(),
        })
        .unwrap();
        let mut total = Decimal::ZERO;
        for _ in 0..3 {
            if let Outcome::Disposal(x) = st
                .apply(&Event::Dispose {
                    date: d("2025-06-01"),
                    quantity: n("1"),
                    proceeds: None,
                    method: Method::Fifo,
                    selections: None,
                })
                .unwrap()
            {
                total += x.basis;
            }
        }
        assert_eq!(total, n("100.00"));
    }
}
