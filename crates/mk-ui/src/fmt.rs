//! Number formatting and entry parsing for display. Amounts arrive from commands as
//! exact decimal strings; this module only adds thousands separators and reads what
//! the user types. It never computes a balance.

use std::cell::Cell;

use mk_core::datefmt::DateStyle;
use mk_core::{Date, Decimal};

// ----- dates (app-desktop.md revision 20) ------------------------------------------
//
// Views hold dates as the user sees and types them; commands always receive and
// return ISO `YYYY-MM-DD`. The chosen style and today's date are set by the app at the
// start of every frame (and when a book opens), so every view and widget on the UI
// thread uses the same rules without passing them around.

thread_local! {
    static STYLE: Cell<DateStyle> = Cell::new(DateStyle::default());
    static TODAY: Cell<Option<Date>> = const { Cell::new(None) };
}

/// Set the date style and today's date for this thread's views.
pub(crate) fn set_dates(style: DateStyle, today: Option<Date>) {
    STYLE.with(|s| s.set(style));
    TODAY.with(|t| t.set(today));
}

/// The chosen date style.
pub(crate) fn date_style() -> DateStyle {
    STYLE.with(Cell::get)
}

fn today() -> Option<Date> {
    TODAY.with(Cell::get)
}

/// A command's ISO date (or date-time) as the user sees it; other text unchanged.
pub(crate) fn date(iso: &str) -> String {
    date_style().show(iso)
}

/// Read a typed date: `Ok(None)` for empty text, `Ok(Some(date))` for a valid one,
/// and the refusal ("Enter the date as …") otherwise.
pub(crate) fn read_date(text: &str) -> Result<Option<Date>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let style = date_style();
    today()
        .and_then(|t| style.read(text, t))
        .map(Some)
        .ok_or_else(|| style.error())
}

/// A typed date as the ISO text a command takes (empty text stays empty), or the
/// refusal.
pub(crate) fn date_arg(text: &str) -> Result<String, String> {
    read_date(text).map(|d| d.map(|d| d.to_string()).unwrap_or_default())
}

/// A typed date as ISO when it reads, else the text as typed (trimmed): for filters
/// and other places where a command reports the problem itself.
pub(crate) fn date_arg_lenient(text: &str) -> String {
    date_arg(text).unwrap_or_else(|_| text.trim().to_owned())
}

/// A typed date rewritten in the chosen style when it is valid; unchanged otherwise.
pub(crate) fn normalise_date(text: &str) -> Option<String> {
    match read_date(text) {
        Ok(Some(d)) => Some(date_style().format(d)),
        _ => None,
    }
}

/// A typed date moved by `days` (the register's `+`/`-`), in the chosen style.
pub(crate) fn shift_date(text: &str, days: i32) -> Option<String> {
    let d = read_date(text).ok().flatten()?.add_days(days)?;
    Some(date_style().format(d))
}

/// Insert thousands separators: `-1234.50` → `-1,234.50`.
pub fn group(amount: &str) -> String {
    let (sign, body) = match amount.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", amount),
    };
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    let digits: Vec<char> = int.chars().collect();
    let mut out = String::new();
    for (i, c) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*c);
    }
    match frac {
        Some(f) => format!("{sign}{out}.{f}"),
        None => format!("{sign}{out}"),
    }
}

/// Read an amount the user typed: digits, optional `.` decimals, optional `,`
/// grouping, optional leading sign. `None` for empty text.
pub fn parse_entry(text: &str) -> Result<Option<Decimal>, String> {
    let t: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',')
        .collect();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<Decimal>()
        .map(Some)
        .map_err(|_| format!("{text:?} is not an amount"))
}

/// Whether an amount string is exactly zero.
pub fn is_zero(amount: &str) -> bool {
    amount.parse::<Decimal>().is_ok_and(|d| d.is_zero())
}

/// Whether an amount string is negative.
pub fn is_negative(amount: &str) -> bool {
    amount
        .parse::<Decimal>()
        .is_ok_and(|d| d.is_sign_negative() && !d.is_zero())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouping() {
        assert_eq!(group("1150.00"), "1,150.00");
        assert_eq!(group("-1234567.5"), "-1,234,567.5");
        assert_eq!(group("950.00"), "950.00");
        assert_eq!(group("0.00"), "0.00");
        assert_eq!(group("100"), "100");
    }

    #[test]
    fn parsing() {
        assert_eq!(
            parse_entry(" 1,000.00 ").unwrap().unwrap().to_string(),
            "1000.00"
        );
        assert!(parse_entry("").unwrap().is_none());
        assert!(parse_entry("12a").is_err());
        assert!(is_zero("0.00") && !is_zero("0.01"));
        assert!(is_negative("-0.01") && !is_negative("0"));
    }
}
