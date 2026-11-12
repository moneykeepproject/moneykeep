//! Calendar dates as day numbers (days since 1970-01-01, proleptic Gregorian), with
//! integer arithmetic only. The conversion is the standard era-based civil calendar
//! computation (400-year cycles of 146,097 days).

/// Day number of a calendar date.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let d = i64::from(d);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Calendar date of a day number.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// Day number of an ISO date `YYYY-MM-DD` (years 1900–2199, `input-safety.md` R4).
pub fn parse_date(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let part = s.get(r)?;
        if !part.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let y = i64::from(num(0..4)?);
    let m = num(5..7)?;
    let d = num(8..10)?;
    if !(1900..=2199).contains(&y) || !(1..=12).contains(&m) || d == 0 {
        return None;
    }
    if d > days_in_month(y, m) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// `YYYY-MM-DD` of a day number.
pub fn format_date(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Today's day number from Unix seconds and a UTC offset in seconds.
pub fn today_from_unix(unix_secs: i64, utc_offset_secs: i64) -> i64 {
    (unix_secs + utc_offset_secs).div_euclid(86_400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(
            parse_date("2026-10-23"),
            Some(days_from_civil(2026, 10, 23))
        );
        for z in [-25_000, -1, 0, 1, 59, 60, 11_016, 20_000, 60_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(format_date(days_from_civil(2024, 2, 29)), "2024-02-29");
        assert_eq!(parse_date("2025-02-29"), None);
        assert_eq!(parse_date("2026-13-01"), None);
        assert_eq!(parse_date("26-10-23"), None);
        assert_eq!(parse_date("2026-1a-01"), None);
        assert_eq!(today_from_unix(86_399, 0), 0);
        assert_eq!(today_from_unix(86_399, 3600), 1);
        assert_eq!(today_from_unix(0, -3600), -1);
    }
}
