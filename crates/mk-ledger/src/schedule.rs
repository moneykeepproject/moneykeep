//! Recurrence date arithmetic and plain-words summaries (spec:
//! scheduled-transactions.md). Every occurrence is computed from the start date and
//! its position, never from the previous occurrence, so clamped or weekend-moved
//! dates do not drift.

use mk_core::Date;
use mk_core::schedule::{EndRule, Frequency, MonthDay, Recurrence, Weekday, WeekendRule};

use crate::{LedgerError, LedgerResult};

/// Upper bound on generated occurrences (guards against runaway loops).
pub const MAX_OCCURRENCES: usize = 100_000;

fn invalid(m: impl Into<String>) -> LedgerError {
    LedgerError::Invalid(m.into())
}

fn check_day(d: MonthDay, what: &str, max: u8) -> LedgerResult<()> {
    match d {
        MonthDay::Day(n) if (1..=max).contains(&n) => Ok(()),
        MonthDay::Day(n) => Err(invalid(format!(
            "{what} must be 1–{max} or \"last\", not {n}"
        ))),
        MonthDay::Last => Ok(()),
    }
}

/// Check that a recurrence has the parameters its frequency needs and no others.
pub fn validate_recurrence(r: &Recurrence) -> LedgerResult<()> {
    if r.interval == 0 {
        return Err(invalid("interval must be at least 1"));
    }
    let uses_interval = matches!(
        r.frequency,
        Frequency::Daily | Frequency::Weekly | Frequency::Monthly | Frequency::Yearly
    );
    if !uses_interval && r.interval != 1 {
        return Err(invalid(
            "interval applies to daily, weekly, monthly and yearly schedules",
        ));
    }
    let extra = |field: &str| invalid(format!("{field} does not apply to this frequency"));
    if r.frequency != Frequency::SemiMonthly && r.days.is_some() {
        return Err(extra("days"));
    }
    if r.frequency != Frequency::Monthly && (r.weekday.is_some() || r.week.is_some()) {
        return Err(extra("weekday/week"));
    }
    if r.frequency != Frequency::Yearly && r.month.is_some() {
        return Err(extra("month"));
    }
    if !matches!(r.frequency, Frequency::Monthly | Frequency::Yearly) && r.day.is_some() {
        return Err(extra("day"));
    }
    match r.frequency {
        Frequency::SemiMonthly => {
            let days = r
                .days
                .as_deref()
                .ok_or_else(|| invalid("semi_monthly needs days, e.g. [1, 15]"))?;
            let [a, b] = days else {
                return Err(invalid("semi_monthly needs exactly two days"));
            };
            check_day(*a, "a day", 31)?;
            check_day(*b, "a day", 31)?;
            if a == b {
                return Err(invalid("the two days must differ"));
            }
        }
        Frequency::Monthly => {
            if let Some(d) = r.day {
                check_day(d, "day", 31)?;
                if r.weekday.is_some() || r.week.is_some() {
                    return Err(invalid("give either day or weekday + week, not both"));
                }
            }
            match (r.weekday, r.week) {
                (Some(_), Some(w)) => check_day(w, "week", 4)?,
                (None, None) => {}
                _ => return Err(invalid("weekday and week go together")),
            }
        }
        Frequency::Yearly => {
            if let Some(m) = r.month
                && !(1..=12).contains(&m)
            {
                return Err(invalid(format!("month must be 1–12, not {m}")));
            }
            if let Some(d) = r.day {
                check_day(d, "day", 31)?;
            }
        }
        Frequency::Once | Frequency::Daily | Frequency::Weekly => {}
    }
    Ok(())
}

fn on_day(year: i32, month: u8, day: MonthDay) -> Option<Date> {
    let d = match day {
        MonthDay::Day(n) => n,
        MonthDay::Last => 31,
    };
    Date::from_ymd_clamped(year, month, d).ok()
}

/// The `week`-th (1–4 or last) `weekday` of a month.
fn nth_weekday(year: i32, month: u8, weekday: Weekday, week: MonthDay) -> Option<Date> {
    let want = i32::from(weekday.number());
    match week {
        MonthDay::Day(n) => {
            let first = Date::from_ymd(year, month, 1).ok()?;
            let offset = (want - i32::from(first.iso_weekday())).rem_euclid(7);
            first.add_days(offset + 7 * (i32::from(n) - 1))
        }
        MonthDay::Last => {
            let last = Date::from_ymd_clamped(year, month, 31).ok()?;
            let back = (i32::from(last.iso_weekday()) - want).rem_euclid(7);
            last.add_days(-back)
        }
    }
}

/// Nominal dates of a recurrence from `start`, in order, without the end rule.
#[derive(Debug, Clone)]
pub struct Nominal {
    rec: Recurrence,
    start: Date,
    /// Next period index (day/week/month/year step, or month for semi-monthly).
    step: i64,
    /// Semi-monthly: dates of the current month not yet returned.
    pending: Vec<Date>,
    done: bool,
}

impl Nominal {
    pub fn new(rec: &Recurrence, start: Date) -> Self {
        Self {
            rec: rec.clone(),
            start,
            step: 0,
            pending: Vec::new(),
            done: false,
        }
    }

    /// The candidate for period `k` (may be before `start`, then skipped).
    fn candidate(&self, k: i64) -> Option<Option<Date>> {
        let n = i64::from(self.rec.interval);
        let s = self.start;
        Some(match self.rec.frequency {
            Frequency::Once => (k == 0).then_some(s),
            Frequency::Daily => Some(s.add_days(i32::try_from(k.checked_mul(n)?).ok()?)?),
            Frequency::Weekly => {
                Some(s.add_days(i32::try_from(k.checked_mul(n)?.checked_mul(7)?).ok()?)?)
            }
            Frequency::Monthly => {
                let (y, m) = Date::shift_month(s.year(), s.month(), k.checked_mul(n)?)?;
                match (self.rec.weekday, self.rec.week) {
                    (Some(wd), Some(w)) => Some(nth_weekday(y, m, wd, w)?),
                    _ => Some(on_day(
                        y,
                        m,
                        self.rec.day.unwrap_or(MonthDay::Day(s.day())),
                    )?),
                }
            }
            Frequency::Yearly => {
                let y = i32::try_from(i64::from(s.year()).checked_add(k.checked_mul(n)?)?).ok()?;
                let m = self.rec.month.unwrap_or(s.month());
                Some(on_day(
                    y,
                    m,
                    self.rec.day.unwrap_or(MonthDay::Day(s.day())),
                )?)
            }
            Frequency::SemiMonthly => None,
        })
    }
}

impl Iterator for Nominal {
    type Item = Date;

    fn next(&mut self) -> Option<Date> {
        if self.done {
            return None;
        }
        // Guard: a matching date must turn up within a few steps of the start.
        for _ in 0..64 {
            if self.rec.frequency == Frequency::SemiMonthly {
                if self.pending.is_empty() {
                    let (y, m) =
                        Date::shift_month(self.start.year(), self.start.month(), self.step)?;
                    self.step += 1;
                    let mut dates: Vec<Date> = self
                        .rec
                        .days
                        .iter()
                        .flatten()
                        .filter_map(|d| on_day(y, m, *d))
                        .filter(|d| *d >= self.start)
                        .collect();
                    dates.sort();
                    dates.dedup();
                    dates.reverse();
                    self.pending = dates;
                }
                if let Some(d) = self.pending.pop() {
                    return Some(d);
                }
                continue;
            }
            let k = self.step;
            self.step += 1;
            match self.candidate(k) {
                None => {
                    self.done = true;
                    return None;
                }
                Some(None) => {
                    self.done = true;
                    return None;
                }
                Some(Some(d)) if d >= self.start => return Some(d),
                Some(Some(_)) => {}
            }
        }
        self.done = true;
        None
    }
}

/// Every nominal date of a schedule with its end rule applied, up to `until`
/// (inclusive), at most [`MAX_OCCURRENCES`].
pub fn nominal_dates(rec: &Recurrence, start: Date, end: EndRule, until: Date) -> Vec<Date> {
    let limit = match end {
        EndRule::After(n) => usize::try_from(n)
            .unwrap_or(MAX_OCCURRENCES)
            .min(MAX_OCCURRENCES),
        _ => MAX_OCCURRENCES,
    };
    Nominal::new(rec, start)
        .take(limit)
        .take_while(|d| *d <= until)
        .take_while(|d| match end {
            EndRule::On(last) => *d <= last,
            _ => true,
        })
        .collect()
}

/// Whether the end rule allows a `count`-th (1-based) occurrence on `date`.
pub fn within_end(end: EndRule, count: usize, date: Date) -> bool {
    match end {
        EndRule::Never => true,
        EndRule::After(n) => usize::try_from(n).is_ok_and(|n| count <= n),
        EndRule::On(last) => date <= last,
    }
}

/// The due date of a nominal date under a weekend rule.
pub fn due_date(nominal: Date, rule: WeekendRule) -> Date {
    let wd = nominal.iso_weekday();
    let shift = match (rule, wd) {
        (WeekendRule::Before, 6) => -1,
        (WeekendRule::Before, 7) => -2,
        (WeekendRule::After, 6) => 2,
        (WeekendRule::After, 7) => 1,
        _ => 0,
    };
    nominal.add_days(shift).unwrap_or(nominal)
}

/// The first nominal date one period after `date` (for "repeat this transaction").
pub fn one_period_after(rec: &Recurrence, date: Date) -> Option<Date> {
    if rec.frequency == Frequency::Once {
        return Some(date);
    }
    Nominal::new(rec, date).find(|d| *d > date)
}

fn ordinal(n: u8) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

fn day_words(d: MonthDay) -> String {
    match d {
        MonthDay::Day(n) => format!("the {}", ordinal(n)),
        MonthDay::Last => "the last day".to_owned(),
    }
}

fn week_words(w: MonthDay) -> &'static str {
    match w {
        MonthDay::Day(1) => "first",
        MonthDay::Day(2) => "second",
        MonthDay::Day(3) => "third",
        MonthDay::Day(4) => "fourth",
        _ => "last",
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// English name of a month 1–12.
pub fn month_name(m: u8) -> &'static str {
    MONTHS
        .get(usize::from(m).wrapping_sub(1))
        .copied()
        .unwrap_or("")
}

fn every(n: u32, one: &str, many: &str) -> String {
    if n == 1 {
        format!("Every {one}")
    } else {
        format!("Every {n} {many}")
    }
}

/// A short English description of a recurrence and its end, e.g. "Every 2 weeks on
/// Friday" or "Monthly on the last day, 12 times".
pub fn summary(rec: &Recurrence, start: Date, end: EndRule) -> String {
    let n = rec.interval;
    let main = match rec.frequency {
        Frequency::Once => format!("Once on {start}"),
        Frequency::Daily => every(n, "day", "days"),
        Frequency::Weekly => {
            let wd = Weekday::from_number(start.iso_weekday()).map_or("", Weekday::name);
            format!("{} on {wd}", every(n, "week", "weeks"))
        }
        Frequency::SemiMonthly => {
            let mut days = rec.days.clone().unwrap_or_default();
            days.sort();
            let words: Vec<String> = days
                .iter()
                .map(|d| match d {
                    MonthDay::Day(x) => ordinal(*x),
                    MonthDay::Last => "last day".to_owned(),
                })
                .collect();
            format!("Twice a month on the {}", words.join(" and "))
        }
        Frequency::Monthly => {
            let lead = if n == 1 {
                "Monthly".to_owned()
            } else {
                format!("Every {n} months")
            };
            match (rec.weekday, rec.week) {
                (Some(wd), Some(w)) => format!("{lead} on the {} {}", week_words(w), wd.name()),
                _ => format!(
                    "{lead} on {}",
                    day_words(rec.day.unwrap_or(MonthDay::Day(start.day())))
                ),
            }
        }
        Frequency::Yearly => {
            let lead = if n == 1 {
                "Yearly".to_owned()
            } else {
                format!("Every {n} years")
            };
            let month = month_name(rec.month.unwrap_or(start.month()));
            match rec.day.unwrap_or(MonthDay::Day(start.day())) {
                MonthDay::Day(d) => format!("{lead} on {month} {d}"),
                MonthDay::Last => format!("{lead} on the last day of {month}"),
            }
        }
    };
    match end {
        EndRule::Never => main,
        EndRule::After(1) => format!("{main}, 1 time"),
        EndRule::After(k) => format!("{main}, {k} times"),
        EndRule::On(d) => format!("{main}, until {d}"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn d(s: &str) -> Date {
        s.parse().unwrap()
    }

    fn first(rec: &Recurrence, start: &str, n: usize) -> Vec<String> {
        Nominal::new(rec, d(start))
            .take(n)
            .map(|x| x.to_string())
            .collect()
    }

    fn monthly(day: MonthDay) -> Recurrence {
        Recurrence {
            day: Some(day),
            ..Recurrence::new(Frequency::Monthly)
        }
    }

    #[test]
    fn scheduled_transactions_at1_monthly_31_clamps_without_drift() {
        let r = monthly(MonthDay::Day(31));
        assert_eq!(
            first(&r, "2026-01-31", 5),
            [
                "2026-01-31",
                "2026-02-28",
                "2026-03-31",
                "2026-04-30",
                "2026-05-31"
            ]
        );
        let all = nominal_dates(&r, d("2026-01-31"), EndRule::Never, d("2028-03-01"));
        assert!(all.contains(&d("2028-02-29")));
        // Day omitted: the start's day.
        let r = Recurrence::new(Frequency::Monthly);
        assert_eq!(first(&r, "2026-01-31", 2), ["2026-01-31", "2026-02-28"]);
    }

    #[test]
    fn scheduled_transactions_at2_every_two_weeks() {
        let r = Recurrence {
            interval: 2,
            ..Recurrence::new(Frequency::Weekly)
        };
        assert_eq!(
            first(&r, "2026-10-02", 4),
            ["2026-10-02", "2026-10-16", "2026-10-30", "2026-11-13"]
        );
        assert_eq!(
            summary(&r, d("2026-10-02"), EndRule::Never),
            "Every 2 weeks on Friday"
        );
    }

    #[test]
    fn scheduled_transactions_at3_semi_monthly() {
        let r = Recurrence {
            days: Some(vec![MonthDay::Day(15), MonthDay::Last]),
            ..Recurrence::new(Frequency::SemiMonthly)
        };
        validate_recurrence(&r).unwrap();
        assert_eq!(
            first(&r, "2026-02-01", 4),
            ["2026-02-15", "2026-02-28", "2026-03-15", "2026-03-31"]
        );
        // Start alignment: days before the start are not generated.
        assert_eq!(first(&r, "2026-02-20", 2), ["2026-02-28", "2026-03-15"]);
    }

    #[test]
    fn scheduled_transactions_at4_second_tuesday() {
        let r = Recurrence {
            weekday: Some(Weekday::Tuesday),
            week: Some(MonthDay::Day(2)),
            ..Recurrence::new(Frequency::Monthly)
        };
        validate_recurrence(&r).unwrap();
        assert_eq!(
            first(&r, "2026-10-01", 3),
            ["2026-10-13", "2026-11-10", "2026-12-08"]
        );
        // Start after this month's match: the first is next month's.
        assert_eq!(first(&r, "2026-10-14", 1), ["2026-11-10"]);
        let last_fri = Recurrence {
            weekday: Some(Weekday::Friday),
            week: Some(MonthDay::Last),
            ..Recurrence::new(Frequency::Monthly)
        };
        assert_eq!(
            first(&last_fri, "2026-10-01", 2),
            ["2026-10-30", "2026-11-27"]
        );
        assert_eq!(
            summary(&r, d("2026-10-01"), EndRule::Never),
            "Monthly on the second Tuesday"
        );
    }

    #[test]
    fn scheduled_transactions_at5_weekend_before() {
        let r = monthly(MonthDay::Day(1));
        let dates = first(&r, "2026-11-01", 2);
        assert_eq!(dates, ["2026-11-01", "2026-12-01"]);
        assert_eq!(
            due_date(d("2026-11-01"), WeekendRule::Before),
            d("2026-10-30")
        );
        assert_eq!(
            due_date(d("2026-12-01"), WeekendRule::Before),
            d("2026-12-01")
        );
        assert_eq!(
            due_date(d("2026-11-01"), WeekendRule::After),
            d("2026-11-02")
        );
        assert_eq!(
            due_date(d("2026-10-31"), WeekendRule::After),
            d("2026-11-02")
        );
    }

    #[test]
    fn scheduled_transactions_at13_summaries() {
        let s = d("2026-01-15");
        let r = monthly(MonthDay::Last);
        assert_eq!(
            summary(&r, s, EndRule::After(12)),
            "Monthly on the last day, 12 times"
        );
        assert_eq!(
            summary(&monthly(MonthDay::Day(15)), s, EndRule::Never),
            "Monthly on the 15th"
        );
        let q = Recurrence {
            interval: 3,
            ..monthly(MonthDay::Day(1))
        };
        assert_eq!(summary(&q, s, EndRule::Never), "Every 3 months on the 1st");
        let semi = Recurrence {
            days: Some(vec![MonthDay::Day(15), MonthDay::Day(1)]),
            ..Recurrence::new(Frequency::SemiMonthly)
        };
        assert_eq!(
            summary(&semi, s, EndRule::Never),
            "Twice a month on the 1st and 15th"
        );
        let y = Recurrence {
            month: Some(3),
            day: Some(MonthDay::Day(31)),
            ..Recurrence::new(Frequency::Yearly)
        };
        assert_eq!(
            summary(&y, s, EndRule::On(d("2027-06-30"))),
            "Yearly on March 31, until 2027-06-30"
        );
        assert_eq!(
            summary(
                &Recurrence::new(Frequency::Once),
                d("2026-11-01"),
                EndRule::Never
            ),
            "Once on 2026-11-01"
        );
        assert_eq!(
            summary(&Recurrence::new(Frequency::Daily), s, EndRule::Never),
            "Every day"
        );
    }

    #[test]
    fn yearly_feb_29_and_alignment() {
        let r = Recurrence {
            month: Some(2),
            day: Some(MonthDay::Day(29)),
            ..Recurrence::new(Frequency::Yearly)
        };
        assert_eq!(
            first(&r, "2028-01-01", 3),
            ["2028-02-29", "2029-02-28", "2030-02-28"]
        );
        // March 31 yearly starting in May: first is next year.
        let y = Recurrence {
            month: Some(3),
            day: Some(MonthDay::Day(31)),
            ..Recurrence::new(Frequency::Yearly)
        };
        assert_eq!(first(&y, "2026-05-01", 1), ["2027-03-31"]);
    }

    #[test]
    fn end_rules_limit_dates() {
        let r = Recurrence::new(Frequency::Daily);
        let s = d("2026-01-01");
        assert_eq!(
            nominal_dates(&r, s, EndRule::After(3), d("2027-01-01")).len(),
            3
        );
        assert_eq!(
            nominal_dates(&r, s, EndRule::On(d("2026-01-05")), d("2027-01-01")).len(),
            5
        );
        assert_eq!(
            nominal_dates(
                &Recurrence::new(Frequency::Once),
                s,
                EndRule::Never,
                d("2030-01-01")
            )
            .len(),
            1
        );
    }

    #[test]
    fn validation() {
        let mut r = Recurrence::new(Frequency::Weekly);
        r.interval = 0;
        assert!(validate_recurrence(&r).is_err());
        let semi = Recurrence {
            days: Some(vec![MonthDay::Day(15), MonthDay::Day(15)]),
            ..Recurrence::new(Frequency::SemiMonthly)
        };
        assert!(validate_recurrence(&semi).is_err());
        let bad = Recurrence {
            weekday: Some(Weekday::Monday),
            ..Recurrence::new(Frequency::Monthly)
        };
        assert!(validate_recurrence(&bad).is_err());
        assert!(validate_recurrence(&monthly(MonthDay::Day(32))).is_err());
    }

    #[test]
    fn one_period_after_a_date() {
        let r = monthly(MonthDay::Day(15));
        assert_eq!(one_period_after(&r, d("2026-10-15")), Some(d("2026-11-15")));
        let w = Recurrence {
            interval: 2,
            ..Recurrence::new(Frequency::Weekly)
        };
        assert_eq!(one_period_after(&w, d("2026-10-02")), Some(d("2026-10-16")));
    }
}
