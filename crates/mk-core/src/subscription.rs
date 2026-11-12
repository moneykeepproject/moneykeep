//! Subscription audit (subscriptions.md): cadences, detection of repeating charges,
//! subscription vs recurring-bill classification, monthly equivalents, colour bands
//! and the automatic chip text colour. Pure logic, no IO.

use std::fmt;
use std::str::FromStr;

use rust_decimal::{Decimal, RoundingStrategy};

use crate::{CoreError, CoreResult, Date};

/// How often a subscription charges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Cadence {
    Weekly,
    Biweekly,
    Monthly,
    Quarterly,
    SixMonthly,
    Yearly,
}

impl Cadence {
    /// Shortest first: detection tries them in this order.
    pub const ALL: [Cadence; 6] = [
        Cadence::Weekly,
        Cadence::Biweekly,
        Cadence::Monthly,
        Cadence::Quarterly,
        Cadence::SixMonthly,
        Cadence::Yearly,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Cadence::Weekly => "weekly",
            Cadence::Biweekly => "biweekly",
            Cadence::Monthly => "monthly",
            Cadence::Quarterly => "quarterly",
            Cadence::SixMonthly => "six_monthly",
            Cadence::Yearly => "yearly",
        }
    }

    /// The label shown in lists.
    pub fn label(self) -> &'static str {
        match self {
            Cadence::Weekly => "Weekly",
            Cadence::Biweekly => "Every two weeks",
            Cadence::Monthly => "Monthly",
            Cadence::Quarterly => "Quarterly",
            Cadence::SixMonthly => "Every six months",
            Cadence::Yearly => "Yearly",
        }
    }

    /// Charges per year (for monthly equivalents).
    pub fn per_year(self) -> u32 {
        match self {
            Cadence::Weekly => 52,
            Cadence::Biweekly => 26,
            Cadence::Monthly => 12,
            Cadence::Quarterly => 4,
            Cadence::SixMonthly => 2,
            Cadence::Yearly => 1,
        }
    }

    /// How many of the last charges must fit: 3 for monthly and shorter, 2 for
    /// quarterly and longer.
    pub fn qualifying(self) -> usize {
        match self {
            Cadence::Weekly | Cadence::Biweekly | Cadence::Monthly => 3,
            _ => 2,
        }
    }

    /// The nominal length in days and the tolerance (monthly uses the calendar).
    fn days(self) -> (i64, i64) {
        match self {
            Cadence::Weekly => (7, 2),
            Cadence::Biweekly => (14, 3),
            Cadence::Monthly => (30, 4),
            Cadence::Quarterly => (91, 7),
            Cadence::SixMonthly => (182, 10),
            Cadence::Yearly => (365, 14),
        }
    }

    /// The matching tolerance in days.
    pub fn tolerance(self) -> i64 {
        self.days().1
    }

    /// Whether `next` comes `periods` periods after `prev`, within the tolerance.
    pub fn fits(self, prev: Date, next: Date, periods: u32) -> bool {
        let (nominal, tol) = self.days();
        if self == Cadence::Monthly {
            let Some(expected) = add_months(prev, i64::from(periods)) else {
                return false;
            };
            return expected.days_until(next).abs() <= tol;
        }
        let gap = prev.days_until(next);
        (gap - nominal * i64::from(periods)).abs() <= tol
    }

    /// The date `periods` periods after `from` (calendar months for monthly and
    /// longer, so the charge day stays put; a month-end date stays at month end).
    pub fn advance(self, from: Date, periods: i64) -> Option<Date> {
        match self {
            Cadence::Weekly => from.add_days(i32::try_from(periods.checked_mul(7)?).ok()?),
            Cadence::Biweekly => from.add_days(i32::try_from(periods.checked_mul(14)?).ok()?),
            Cadence::Monthly => add_months(from, periods),
            Cadence::Quarterly => add_months(from, periods.checked_mul(3)?),
            Cadence::SixMonthly => add_months(from, periods.checked_mul(6)?),
            Cadence::Yearly => add_months(from, periods.checked_mul(12)?),
        }
    }

    /// The recurrence `schedule.create` takes for this cadence (Make a reminder).
    pub fn recurrence(self) -> (&'static str, u32) {
        match self {
            Cadence::Weekly => ("weekly", 1),
            Cadence::Biweekly => ("weekly", 2),
            Cadence::Monthly => ("monthly", 1),
            Cadence::Quarterly => ("monthly", 3),
            Cadence::SixMonthly => ("monthly", 6),
            Cadence::Yearly => ("yearly", 1),
        }
    }
}

impl fmt::Display for Cadence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for Cadence {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Cadence {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl FromStr for Cadence {
    type Err = CoreError;
    fn from_str(s: &str) -> CoreResult<Self> {
        let t = s.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        Cadence::ALL
            .into_iter()
            .find(|c| {
                c.as_str() == t
                    || c.label().eq_ignore_ascii_case(s.trim())
                    || (t == "every_two_weeks" && *c == Cadence::Biweekly)
                    || (t == "every_six_months" && *c == Cadence::SixMonthly)
                    || (t == "annual" && *c == Cadence::Yearly)
            })
            .ok_or_else(|| CoreError::InvalidValue {
                what: "cadence",
                value: s.to_owned(),
            })
    }
}

/// `date` moved by `months` calendar months; the day is clamped to the month's
/// length, and a month-end date stays at month end.
pub fn add_months(date: Date, months: i64) -> Option<Date> {
    let (y, m) = Date::shift_month(date.year(), date.month(), months)?;
    let month_end = date.day() == Date::days_in_month(date.year(), date.month());
    let day = if month_end {
        Date::days_in_month(y, m)
    } else {
        date.day()
    };
    Date::from_ymd_clamped(y, m, day).ok()
}

/// What detection found in one group of charges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    pub cadence: Cadence,
    /// Index of the first charge of the trailing run that fits the cadence.
    pub start: usize,
}

/// The cadence of charges sorted by date (oldest first), if their last 3 (monthly
/// and shorter) or 2 (quarterly and longer) fit one. A single missed period inside
/// the run is allowed. Fits without a missed period win over fits with one.
pub fn detect(dates: &[Date]) -> Option<Detected> {
    for allow_miss in [false, true] {
        for c in Cadence::ALL {
            let n = c.qualifying();
            if dates.len() < n {
                continue;
            }
            let tail = dates.get(dates.len() - n..)?;
            let mut misses = 0;
            let mut ok = true;
            for w in tail.windows(2) {
                let (Some(a), Some(b)) = (w.first(), w.get(1)) else {
                    ok = false;
                    break;
                };
                if c.fits(*a, *b, 1) {
                    continue;
                }
                if allow_miss && misses == 0 && c.fits(*a, *b, 2) {
                    misses += 1;
                    continue;
                }
                ok = false;
                break;
            }
            if !ok {
                continue;
            }
            // Extend backwards while the intervals still fit (one missed period
            // allowed over the whole run).
            let mut start = dates.len() - n;
            while start > 0 {
                let (Some(a), Some(b)) = (dates.get(start - 1), dates.get(start)) else {
                    break;
                };
                if c.fits(*a, *b, 1) {
                    start -= 1;
                } else if misses == 0 && c.fits(*a, *b, 2) {
                    misses += 1;
                    start -= 1;
                } else {
                    break;
                }
            }
            return Some(Detected { cadence: c, start });
        }
    }
    None
}

/// Whether the amounts (oldest first) look like a subscription (subscriptions.md
/// revision 2): runs of equal amounts in which every run except the first and the
/// last has at least 2 charges, so a single price step (139.00 then 149.00) counts.
/// Otherwise (amounts vary from charge to charge) a recurring bill.
pub fn is_subscription_like(amounts: &[Decimal]) -> bool {
    let mut runs: Vec<usize> = Vec::new();
    let mut prev: Option<Decimal> = None;
    for a in amounts {
        match (prev, runs.last_mut()) {
            (Some(p), Some(n)) if p == *a => *n += 1,
            _ => runs.push(1),
        }
        prev = Some(*a);
    }
    match runs.as_slice() {
        [] => false,
        [one] => *one >= 2,
        [_, middle @ .., _] => middle.iter().all(|n| *n >= 2),
    }
}

/// Round money to cents, halves away from zero.
pub fn cents(d: Decimal) -> Decimal {
    let mut r = d.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    r.rescale(2);
    r
}

/// The monthly equivalent: price × charges per year ÷ 12, to the cent.
pub fn monthly_equivalent(price: Decimal, cadence: Cadence) -> Decimal {
    cents(price * Decimal::from(cadence.per_year()) / Decimal::from(12))
}

/// The yearly amount: price × charges per year.
pub fn yearly(price: Decimal, cadence: Cadence) -> Decimal {
    cents(price * Decimal::from(cadence.per_year()))
}

/// The change from `old` to `new` in percent (unrounded); `None` when `old` is zero.
pub fn change_percent(old: Decimal, new: Decimal) -> Option<Decimal> {
    if old.is_zero() {
        return None;
    }
    Some((new - old) * Decimal::ONE_HUNDRED / old)
}

/// A change shown to one decimal: "+16.1%" / "−3.0%".
pub fn percent_text(p: Decimal) -> String {
    let mut r = p.round_dp_with_strategy(1, RoundingStrategy::MidpointAwayFromZero);
    r.rescale(1);
    if r.is_sign_negative() {
        format!("-{}%", r.abs())
    } else {
        format!("+{r}%")
    }
}

/// Whether a change is "small" (below both alert thresholds): neither its amount
/// reaches `alert_amount` nor its percentage reaches `alert_percent`.
pub fn is_small(old: Decimal, new: Decimal, alert_percent: Decimal, alert_amount: Decimal) -> bool {
    let amount = (new - old).abs();
    let pct = change_percent(old, new).map_or(Decimal::ONE_HUNDRED, |p| p.abs());
    amount < alert_amount && pct < alert_percent
}

/// A colour band: changes at or over `percent` take `color` (0xRRGGBB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub percent: Decimal,
    pub color: u32,
}

/// The default bands: 20 % yellow (#F2C94C) and 40 % red (#D64545).
pub fn default_bands() -> Vec<Band> {
    vec![
        Band {
            percent: Decimal::from(20),
            color: 0x00F2_C94C,
        },
        Band {
            percent: Decimal::from(40),
            color: 0x00D6_4545,
        },
    ]
}

/// The red of "charged after cancelling" (regardless of bands).
pub const ALERT_RED: u32 = 0x00D6_4545;

/// The band an increase of `percent` reaches (the highest threshold at or under it);
/// decreases and changes under the first band have none.
pub fn band_for(percent: Decimal, bands: &[Band]) -> Option<Band> {
    if percent <= Decimal::ZERO {
        return None;
    }
    bands
        .iter()
        .filter(|b| percent >= b.percent)
        .max_by(|a, b| a.percent.cmp(&b.percent))
        .copied()
}

/// "#F2C94C" (or "F2C94C", or "#abc") → 0xF2C94C.
pub fn parse_hex(s: &str) -> Option<u32> {
    let t = s.trim().trim_start_matches('#');
    let full: String = match t.len() {
        3 => t.chars().flat_map(|c| [c, c]).collect(),
        6 => t.to_owned(),
        _ => return None,
    };
    if !full.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(&full, 16).ok()
}

/// 0xF2C94C → "#F2C94C".
pub fn hex(color: u32) -> String {
    format!("#{:06X}", color & 0x00FF_FFFF)
}

/// sRGB channel (0–255) → linear light × 1 000 000 (WCAG 2 relative luminance).
const LINEAR: [u32; 256] = [
    0, 304, 607, 911, 1214, 1518, 1821, 2125, 2428, 2732, 3035, 3347, 3677, 4025, 4391, 4777, 5182,
    5605, 6049, 6512, 6995, 7499, 8023, 8568, 9134, 9721, 10330, 10960, 11612, 12286, 12983, 13702,
    14444, 15209, 15996, 16807, 17642, 18500, 19382, 20289, 21219, 22174, 23153, 24158, 25187,
    26241, 27321, 28426, 29557, 30713, 31896, 33105, 34340, 35601, 36889, 38204, 39546, 40915,
    42311, 43735, 45186, 46665, 48172, 49707, 51269, 52861, 54480, 56128, 57805, 59511, 61246,
    63010, 64803, 66626, 68478, 70360, 72272, 74214, 76185, 78187, 80220, 82283, 84376, 86500,
    88656, 90842, 93059, 95307, 97587, 99899, 102242, 104616, 107023, 109462, 111932, 114435,
    116971, 119538, 122139, 124772, 127438, 130136, 132868, 135633, 138432, 141263, 144128, 147027,
    149960, 152926, 155926, 158961, 162029, 165132, 168269, 171441, 174647, 177888, 181164, 184475,
    187821, 191202, 194618, 198069, 201556, 205079, 208637, 212231, 215861, 219526, 223228, 226966,
    230740, 234551, 238398, 242281, 246201, 250158, 254152, 258183, 262251, 266356, 270498, 274677,
    278894, 283149, 287441, 291771, 296138, 300544, 304987, 309469, 313989, 318547, 323143, 327778,
    332452, 337164, 341914, 346704, 351533, 356400, 361307, 366253, 371238, 376262, 381326, 386429,
    391572, 396755, 401978, 407240, 412543, 417885, 423268, 428690, 434154, 439657, 445201, 450786,
    456411, 462077, 467784, 473531, 479320, 485150, 491021, 496933, 502886, 508881, 514918, 520996,
    527115, 533276, 539479, 545724, 552011, 558340, 564712, 571125, 577580, 584078, 590619, 597202,
    603827, 610496, 617207, 623960, 630757, 637597, 644480, 651406, 658375, 665387, 672443, 679542,
    686685, 693872, 701102, 708376, 715694, 723055, 730461, 737910, 745404, 752942, 760525, 768151,
    775822, 783538, 791298, 799103, 806952, 814847, 822786, 830770, 838799, 846873, 854993, 863157,
    871367, 879622, 887923, 896269, 904661, 913099, 921582, 930111, 938686, 947307, 955973, 964686,
    973445, 982251, 991102, 1_000_000,
];

/// Relative luminance × 10¹⁰.
fn luminance(color: u32) -> u128 {
    let ch = |shift: u32| -> u128 {
        let i = usize::try_from((color >> shift) & 0xFF).unwrap_or(0);
        u128::from(LINEAR.get(i).copied().unwrap_or(0))
    };
    2126 * ch(16) + 7152 * ch(8) + 722 * ch(0)
}

/// The WCAG contrast ratio of two colours × 1000 (4500 = 4.5:1).
pub fn contrast_milli(a: u32, b: u32) -> u128 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    // 0.05 in units of 10⁻¹⁰.
    let off = 500_000_000;
    (hi + off) * 1000 / (lo + off)
}

/// Whether text on a `background` chip should be dark (black) rather than light
/// (white): whichever contrasts more. One of the two always reaches 4.5:1.
pub fn dark_text_on(background: u32) -> bool {
    contrast_milli(background, 0x0000_0000) >= contrast_milli(background, 0x00FF_FFFF)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        s.parse().unwrap()
    }

    fn dates(v: &[&str]) -> Vec<Date> {
        v.iter().map(|s| d(s)).collect()
    }

    #[test]
    fn monthly_with_month_end() {
        let v = dates(&["2026-01-31", "2026-02-28", "2026-03-31"]);
        assert_eq!(detect(&v).unwrap().cadence, Cadence::Monthly);
        let v = dates(&["2026-01-05", "2026-02-08", "2026-03-04"]);
        assert_eq!(detect(&v).unwrap().cadence, Cadence::Monthly);
    }

    #[test]
    fn missed_period() {
        let v = dates(&["2026-01-05", "2026-03-05", "2026-04-05"]);
        let got = detect(&v).unwrap();
        assert_eq!(got.cadence, Cadence::Monthly);
        assert_eq!(got.start, 0);
    }

    #[test]
    fn weekly_and_not() {
        let v = dates(&["2026-10-05", "2026-10-12", "2026-10-19"]);
        assert_eq!(detect(&v).unwrap().cadence, Cadence::Weekly);
        let v = dates(&["2026-01-01", "2026-01-10", "2026-02-19", "2026-05-05"]);
        assert_eq!(detect(&v), None);
    }

    #[test]
    fn yearly_two() {
        let v = dates(&["2025-12-02", "2026-12-02"]);
        assert_eq!(detect(&v).unwrap().cadence, Cadence::Yearly);
    }

    #[test]
    fn kinds() {
        let x = |v: &[&str]| -> Vec<Decimal> { v.iter().map(|s| s.parse().unwrap()).collect() };
        assert!(is_subscription_like(&x(&[
            "15.49", "15.49", "15.49", "15.49", "17.99", "17.99"
        ])));
        assert!(is_subscription_like(&x(&["9.99", "9.99", "12.99"])));
        assert!(!is_subscription_like(&x(&["82.10", "95.40", "120.55"])));
        assert!(is_subscription_like(&x(&["139.00", "139.00"])));
        // Revision 2: a single step, also at the start of the run.
        assert!(is_subscription_like(&x(&["139.00", "149.00"])));
        assert!(is_subscription_like(&x(&["9.99", "12.99", "12.99"])));
        assert!(is_subscription_like(&x(&[
            "9.99", "12.99", "12.99", "14.99"
        ])));
        assert!(!is_subscription_like(&x(&["9.99", "12.99", "14.99"])));
    }

    #[test]
    fn equivalents() {
        assert_eq!(
            monthly_equivalent("139.00".parse().unwrap(), Cadence::Yearly).to_string(),
            "11.58"
        );
        assert_eq!(
            monthly_equivalent("5.00".parse().unwrap(), Cadence::Weekly).to_string(),
            "21.67"
        );
    }

    #[test]
    fn bands_and_contrast() {
        let b = default_bands();
        let p = |s: &str| -> Decimal { s.parse().unwrap() };
        assert_eq!(band_for(p("16.1"), &b), None);
        assert_eq!(band_for(p("25"), &b).unwrap().color, 0xF2C94C);
        assert_eq!(band_for(p("45"), &b).unwrap().color, 0xD64545);
        assert_eq!(band_for(p("-50"), &b), None);
        for c in [
            0xF2C94C, 0xD64545, 0x27AE60, 0x8E44AD, 0x000000, 0xFFFFFF, 0x777777,
        ] {
            let text = if dark_text_on(c) { 0 } else { 0xFFFFFF };
            assert!(contrast_milli(c, text) >= 4500, "{c:06X}");
        }
        assert!(dark_text_on(0xF2C94C));
        assert!(!dark_text_on(0x8E44AD));
        assert_eq!(parse_hex("#f2c94c"), Some(0xF2C94C));
        assert_eq!(parse_hex("#abc"), Some(0xAABBCC));
        assert_eq!(parse_hex("nope"), None);
        assert_eq!(hex(0xF2C94C), "#F2C94C");
    }

    #[test]
    fn small_changes() {
        let p = |s: &str| -> Decimal { s.parse().unwrap() };
        assert!(!is_small(p("15.49"), p("15.79"), p("1"), p("0.50")));
        assert!(is_small(p("15.49"), p("15.59"), p("1"), p("0.50")));
        assert_eq!(
            percent_text(change_percent(p("15.49"), p("17.99")).unwrap()),
            "+16.1%"
        );
    }
}
