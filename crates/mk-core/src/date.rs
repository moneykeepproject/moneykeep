use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use time::Month;

use crate::{CoreError, CoreResult};

/// A calendar date with no time and no time zone, written `YYYY-MM-DD`.
///
/// The text form sorts the same way as the dates, so storage can compare it as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Date(time::Date);

impl Date {
    pub fn from_ymd(year: i32, month: u8, day: u8) -> CoreResult<Self> {
        let month = Month::try_from(month)
            .map_err(|_| CoreError::InvalidDate(format!("{year}-{month}")))?;
        time::Date::from_calendar_date(year, month, day)
            .map(Self)
            .map_err(|_| CoreError::InvalidDate(format!("{year}-{month}-{day}")))
    }

    /// Wrap a `time::Date`. Years outside 1..=9999 are rejected so the text form stays fixed-width.
    pub fn from_time(date: time::Date) -> CoreResult<Self> {
        if (1..=9999).contains(&date.year()) {
            Ok(Self(date))
        } else {
            Err(CoreError::InvalidDate(date.to_string()))
        }
    }

    pub fn year(self) -> i32 {
        self.0.year()
    }

    pub fn month(self) -> u8 {
        u8::from(self.0.month())
    }

    pub fn day(self) -> u8 {
        self.0.day()
    }

    /// The previous calendar day, if representable.
    pub fn previous_day(self) -> Option<Self> {
        self.0.previous_day().and_then(|d| Self::from_time(d).ok())
    }

    /// The next calendar day, if representable.
    pub fn next_day(self) -> Option<Self> {
        self.0.next_day().and_then(|d| Self::from_time(d).ok())
    }

    /// Whole days from `self` to `other` (negative if `other` is earlier).
    pub fn days_until(self, other: Date) -> i64 {
        i64::from(other.0.to_julian_day()) - i64::from(self.0.to_julian_day())
    }

    /// The date `days` days later (or earlier, if negative).
    pub fn add_days(self, days: i32) -> Option<Self> {
        let jd = self.0.to_julian_day().checked_add(days)?;
        time::Date::from_julian_day(jd)
            .ok()
            .and_then(|d| Self::from_time(d).ok())
    }

    /// Day of the week, 1 = Monday … 7 = Sunday (ISO 8601).
    pub fn iso_weekday(self) -> u8 {
        self.0.weekday().number_from_monday()
    }

    /// Number of days in a month of the proleptic Gregorian calendar (0 for a bad month).
    pub fn days_in_month(year: i32, month: u8) -> u8 {
        Month::try_from(month).map_or(0, |m| m.length(year))
    }

    /// The date in `year`/`month` on `day`, or the month's last day when `day` is
    /// beyond it (`day` 0 is treated as 1).
    pub fn from_ymd_clamped(year: i32, month: u8, day: u8) -> CoreResult<Self> {
        let last = Self::days_in_month(year, month);
        let d = Self::from_ymd(year, month, day.clamp(1, last.max(1)))?;
        Self::from_time(d.0)
    }

    /// Year and month `months` months after this date's month (may be negative).
    pub fn shift_month(year: i32, month: u8, months: i64) -> Option<(i32, u8)> {
        let index = i64::from(year)
            .checked_mul(12)?
            .checked_add(i64::from(month).checked_sub(1)?)?
            .checked_add(months)?;
        let y = i32::try_from(index.div_euclid(12)).ok()?;
        let m = u8::try_from(index.rem_euclid(12).checked_add(1)?).ok()?;
        Some((y, m))
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02}",
            self.year(),
            self.month(),
            self.day()
        )
    }
}

impl FromStr for Date {
    type Err = CoreError;
    fn from_str(s: &str) -> CoreResult<Self> {
        let bad = || CoreError::InvalidDate(s.to_owned());
        let bytes = s.as_bytes();
        if bytes.len() != 10 || bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
            return Err(bad());
        }
        let num = |range: std::ops::Range<usize>| -> CoreResult<u32> {
            let part = s.get(range).ok_or_else(bad)?;
            if part.bytes().all(|b| b.is_ascii_digit()) {
                part.parse::<u32>().map_err(|_| bad())
            } else {
                Err(bad())
            }
        };
        let year = i32::try_from(num(0..4)?).map_err(|_| bad())?;
        let month = u8::try_from(num(5..7)?).map_err(|_| bad())?;
        let day = u8::try_from(num(8..10)?).map_err(|_| bad())?;
        let date = Self::from_ymd(year, month, day).map_err(|_| bad())?;
        Self::from_time(date.0)
    }
}

impl TryFrom<String> for Date {
    type Error = CoreError;
    fn try_from(s: String) -> CoreResult<Self> {
        s.parse()
    }
}

impl From<Date> for String {
    fn from(d: Date) -> Self {
        d.to_string()
    }
}

impl JsonSchema for Date {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "Date".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "format": "date",
            "pattern": r"^\d{4}-\d{2}-\d{2}$",
            "description": "Calendar date, YYYY-MM-DD"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format_round_trip() {
        let d: Date = "2026-06-30".parse().unwrap();
        assert_eq!(d.to_string(), "2026-06-30");
        assert_eq!(d.next_day().unwrap().to_string(), "2026-07-01");
        assert_eq!(d.previous_day().unwrap().to_string(), "2026-06-29");
        assert_eq!(d.add_days(-30).unwrap().to_string(), "2026-05-31");
        assert_eq!(d.days_until("2026-07-03".parse().unwrap()), 3);
        assert_eq!(d.days_until("2026-06-27".parse().unwrap()), -3);
    }

    #[test]
    fn rejects_bad_dates() {
        for s in [
            "2026-02-30",
            "2026-13-01",
            "26-01-01",
            "2026/01/01",
            "2026-1-01",
            "",
        ] {
            assert!(s.parse::<Date>().is_err(), "{s}");
        }
    }

    #[test]
    fn ordering_matches_text_ordering() {
        let a: Date = "2025-12-31".parse().unwrap();
        let b: Date = "2026-01-01".parse().unwrap();
        assert!(a < b);
        assert!(a.to_string() < b.to_string());
    }
}
