//! The calendar sheet's month grid and date wording (mobile-capture.md revision 7).
//! Days are counted from 1970-01-01 (day 0, a Thursday), as `mk_capture_api`'s
//! date helpers count them. Pure: no UI, no IO.

use mk_capture_api::{DATE_WINDOW_DAYS, civil_from_days, days_from_civil};

pub const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

pub const MONTHS: [&str; 12] = [
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

/// One cell of the month grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// The day (days since 1970-01-01).
    pub days: i64,
    /// Its day of the month.
    pub day: u32,
    /// In the month shown (not the previous or next month).
    pub in_month: bool,
}

/// The weekday of `days`: 0 Sunday … 6 Saturday.
pub fn weekday(days: i64) -> usize {
    usize::try_from((days + 4).rem_euclid(7)).unwrap_or(0)
}

/// The month shown: (year, month 1–12).
pub type Month = (i64, u32);

/// The month holding `days`.
pub fn month_of(days: i64) -> Month {
    let (y, m, _) = civil_from_days(days);
    (y, m)
}

/// The month after (`step` 1) or before (`step` -1) `m`.
pub fn step_month((y, m): Month, step: i32) -> Month {
    let idx = y * 12 + i64::from(m) - 1 + i64::from(step);
    (
        idx.div_euclid(12),
        u32::try_from(idx.rem_euclid(12)).unwrap_or(0) + 1,
    )
}

/// The weeks of month `m`, Sunday first: whole weeks, with the days of the
/// previous and next months that fill them.
pub fn month_grid(m: Month) -> Vec<[Cell; 7]> {
    let first = days_from_civil(m.0, m.1, 1);
    let next = step_month(m, 1);
    let end = days_from_civil(next.0, next.1, 1);
    let mut d = first - i64::try_from(weekday(first)).unwrap_or(0);
    let mut weeks = Vec::new();
    while d < end {
        let week: [Cell; 7] = std::array::from_fn(|i| {
            let days = d + i64::try_from(i).unwrap_or(0);
            let (_, _, day) = civil_from_days(days);
            Cell {
                days,
                day,
                in_month: days >= first && days < end,
            }
        });
        weeks.push(week);
        d += 7;
    }
    weeks
}

/// Whether `days` may be chosen: at most 400 days before or after `today` (the
/// API's limit).
pub fn in_range(days: i64, today: i64) -> bool {
    (days - today).abs() <= DATE_WINDOW_DAYS
}

/// Whether month `m` has a day that may be chosen.
pub fn month_in_range(m: Month, today: i64) -> bool {
    let first = days_from_civil(m.0, m.1, 1);
    let next = step_month(m, 1);
    let last = days_from_civil(next.0, next.1, 1) - 1;
    last >= today - DATE_WINDOW_DAYS && first <= today + DATE_WINDOW_DAYS
}

fn short(s: &str) -> &str {
    s.get(..3).unwrap_or(s)
}

/// "Thu, Nov 5, 2026" (the Date button).
pub fn long_form(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    let month = MONTHS.get(m as usize - 1).copied().unwrap_or("");
    let wd = WEEKDAYS.get(weekday(days)).copied().unwrap_or("");
    format!("{}, {} {d}, {y}", short(wd), short(month))
}

/// "Thursday, November 5, 2026" (a day's accessible name).
pub fn full_name(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    let month = MONTHS.get(m as usize - 1).copied().unwrap_or("");
    let wd = WEEKDAYS.get(weekday(days)).copied().unwrap_or("");
    format!("{wd}, {month} {d}, {y}")
}

/// When the lists were loaded, in local time (`local_ms`: milliseconds since
/// 1970 shifted to local time): "Nov 8, 1:05 PM"; with the year when it is not
/// `this_year`: "Dec 30, 2025, 9:00 AM" (mobile-capture.md revision 14).
pub fn stamp(local_ms: i64, this_year: i64) -> String {
    let days = local_ms.div_euclid(86_400_000);
    let mins = local_ms.rem_euclid(86_400_000) / 60_000;
    let (y, m, d) = civil_from_days(days);
    let month = MONTHS.get(m as usize - 1).copied().unwrap_or("");
    let (h, min) = (mins / 60, mins % 60);
    let half = if h < 12 { "AM" } else { "PM" };
    let h12 = match h % 12 {
        0 => 12,
        x => x,
    };
    let year = if y == this_year {
        String::new()
    } else {
        format!(", {y}")
    };
    format!("{} {d}{year}, {h12}:{min:02} {half}", short(month))
}

/// "November 2026".
pub fn month_title((y, m): Month) -> String {
    let month = MONTHS.get(m as usize - 1).copied().unwrap_or("");
    format!("{month} {y}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::*;
    use mk_capture_api::parse_date;

    fn d(s: &str) -> i64 {
        parse_date(s).unwrap_or(i64::MIN)
    }

    #[test]
    fn weekdays_and_names() {
        assert_eq!(weekday(0), 4, "1970-01-01 was a Thursday");
        assert_eq!(weekday(d("2026-11-05")), 4);
        assert_eq!(weekday(d("2026-11-01")), 0);
        assert_eq!(long_form(d("2026-11-05")), "Thu, Nov 5, 2026");
        assert_eq!(full_name(d("2026-11-05")), "Thursday, November 5, 2026");
        assert_eq!(month_title((2026, 11)), "November 2026");
    }

    #[test]
    fn month_grids() {
        // November 2026 starts on a Sunday: five whole weeks, the last spilling
        // into December.
        let g = month_grid((2026, 11));
        assert_eq!(g.len(), 5);
        assert_eq!(g[0][0].days, d("2026-11-01"));
        assert!(g[0][0].in_month);
        assert_eq!(g[4][1].day, 30);
        assert_eq!(g[4][2].day, 1);
        assert!(!g[4][2].in_month);
        // February 2026 starts on a Sunday and has 28 days: four weeks.
        assert_eq!(month_grid((2026, 2)).len(), 4);
        // August 2026 starts on a Saturday: six weeks, July's days first.
        let a = month_grid((2026, 8));
        assert_eq!(a.len(), 6);
        assert_eq!(a[0][0].day, 26);
        assert!(!a[0][0].in_month);
        assert_eq!(a[0][6].days, d("2026-08-01"));
        // Every row is Sunday to Saturday.
        for w in &a {
            assert_eq!(weekday(w[0].days), 0);
        }
        // Leap years.
        let f = month_grid((2028, 2));
        assert!(f.iter().flatten().any(|c| c.in_month && c.day == 29));
    }

    #[test]
    fn steps_and_range() {
        assert_eq!(step_month((2026, 12), 1), (2027, 1));
        assert_eq!(step_month((2026, 1), -1), (2025, 12));
        let today = d("2026-11-05");
        assert!(in_range(today + 400, today));
        assert!(!in_range(today + 401, today));
        assert!(!in_range(today - 401, today));
        assert!(month_in_range((2027, 12), today));
        assert!(!month_in_range((2028, 1), today));
        assert!(month_in_range((2025, 10), today));
        assert!(!month_in_range((2025, 9), today));
    }

    #[test]
    fn r14_stamps() {
        let at = |date: &str, h: i64, m: i64| d(date) * 86_400_000 + (h * 60 + m) * 60_000;
        assert_eq!(stamp(at("2026-11-08", 13, 5), 2026), "Nov 8, 1:05 PM");
        assert_eq!(stamp(at("2026-11-08", 0, 0), 2026), "Nov 8, 12:00 AM");
        assert_eq!(stamp(at("2026-11-08", 12, 30), 2026), "Nov 8, 12:30 PM");
        assert_eq!(stamp(at("2025-12-30", 9, 0), 2026), "Dec 30, 2025, 9:00 AM");
    }
}
