//! How dates are shown and typed in the desktop app (app-desktop.md revision 20).
//!
//! Storage, commands and every machine interface keep ISO `YYYY-MM-DD`; this module
//! only writes a [`Date`] in a chosen pattern and reads what a user typed in it. It is
//! pure: "today" is always passed in.

use crate::Date;

/// The patterns offered, in the order the setting lists them.
pub const PATTERNS: [&str; 24] = [
    // Month first.
    "MM-DD-YYYY",
    "MM/DD/YYYY",
    "MM.DD.YYYY",
    "M/D/YYYY",
    "MM-DD-YY",
    "MM/DD/YY",
    "M/D/YY",
    // Day first.
    "DD-MM-YYYY",
    "DD/MM/YYYY",
    "DD.MM.YYYY",
    "D/M/YYYY",
    "D.M.YYYY",
    "DD-MM-YY",
    "DD/MM/YY",
    "DD.MM.YY",
    // Year first.
    "YYYY-MM-DD",
    "YYYY/MM/DD",
    "YYYY.MM.DD",
    "YYYYMMDD",
    // Month name.
    "DD-Mon-YYYY",
    "DD Mon YYYY",
    "Mon DD, YYYY",
    "Month DD, YYYY",
    "DD Month YYYY",
];

/// The default pattern.
pub const DEFAULT_PATTERN: &str = "MM-DD-YYYY";

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

/// The group a pattern is listed under.
pub fn group_of(pattern: &str) -> &'static str {
    if pattern.contains("Mon") {
        "Month name"
    } else if pattern.starts_with('Y') {
        "Year first"
    } else if pattern.starts_with('D') {
        "Day first"
    } else {
        "Month first"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok {
    Year4,
    Year2,
    Month2,
    Month1,
    Day2,
    Day1,
    MonAbbr,
    MonthName,
    Lit(char),
}

fn tokens(pattern: &str) -> Vec<Tok> {
    const NAMED: [(&str, Tok); 8] = [
        ("Month", Tok::MonthName),
        ("Mon", Tok::MonAbbr),
        ("YYYY", Tok::Year4),
        ("YY", Tok::Year2),
        ("MM", Tok::Month2),
        ("M", Tok::Month1),
        ("DD", Tok::Day2),
        ("D", Tok::Day1),
    ];
    let mut out = Vec::new();
    let mut rest = pattern;
    'outer: while !rest.is_empty() {
        for (name, tok) in NAMED {
            if let Some(r) = rest.strip_prefix(name) {
                out.push(tok);
                rest = r;
                continue 'outer;
            }
        }
        let mut chars = rest.chars();
        if let Some(c) = chars.next() {
            out.push(Tok::Lit(c));
        }
        rest = chars.as_str();
    }
    out
}

/// A field of a date, in the order a pattern writes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Year,
    Month,
    Day,
}

/// Separators accepted in place of a pattern's own (rule 1).
fn is_lenient_sep(c: char) -> bool {
    matches!(c, '-' | '/' | '.' | ' ')
}

/// A date pattern from [`PATTERNS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DateStyle(&'static str);

impl Default for DateStyle {
    fn default() -> Self {
        Self(DEFAULT_PATTERN)
    }
}

impl DateStyle {
    /// The style named `pattern`, if it is one of [`PATTERNS`].
    pub fn parse(pattern: &str) -> Option<Self> {
        PATTERNS.iter().find(|p| **p == pattern).map(|p| Self(p))
    }

    /// Every style, in listing order.
    pub fn all() -> impl Iterator<Item = DateStyle> {
        PATTERNS.iter().map(|p| Self(p))
    }

    /// The pattern text, e.g. `MM-DD-YYYY`.
    pub fn pattern(self) -> &'static str {
        self.0
    }

    /// The refusal shown for text that is not a date in this style.
    pub fn error(self) -> String {
        format!("Enter the date as {}", self.0)
    }

    fn has_names(self) -> bool {
        self.0.contains("Mon")
    }

    fn fields(self) -> Vec<Field> {
        tokens(self.0)
            .into_iter()
            .filter_map(|t| match t {
                Tok::Year4 | Tok::Year2 => Some(Field::Year),
                Tok::Month2 | Tok::Month1 | Tok::MonAbbr | Tok::MonthName => Some(Field::Month),
                Tok::Day2 | Tok::Day1 => Some(Field::Day),
                Tok::Lit(_) => None,
            })
            .collect()
    }

    /// `date` written in this style.
    pub fn format(self, date: Date) -> String {
        let month = usize::from(date.month()).saturating_sub(1);
        let name = MONTHS.get(month).copied().unwrap_or("");
        let mut out = String::new();
        for t in tokens(self.0) {
            match t {
                Tok::Year4 => out.push_str(&format!("{:04}", date.year())),
                Tok::Year2 => out.push_str(&format!("{:02}", date.year().rem_euclid(100))),
                Tok::Month2 => out.push_str(&format!("{:02}", date.month())),
                Tok::Month1 => out.push_str(&date.month().to_string()),
                Tok::Day2 => out.push_str(&format!("{:02}", date.day())),
                Tok::Day1 => out.push_str(&date.day().to_string()),
                Tok::MonAbbr => out.extend(name.chars().take(3)),
                Tok::MonthName => out.push_str(name),
                Tok::Lit(c) => out.push(c),
            }
        }
        out
    }

    /// The day and month of `date` without the year, for a single-date period heading
    /// (revision 22): the pattern's own day/month order, padding and separator
    /// (`MM-DD-YYYY` → `10-19`, `M/D/YYYY` → `1/5`, `DD.MM.YYYY` → `19.10`);
    /// `YYYYMMDD`, which has none, uses `-`. Month names are always abbreviated
    /// (`DD Month YYYY` → `19 Oct`, `Month DD, YYYY` → `Oct 19`).
    pub fn day_month(self, date: Date) -> String {
        let month = usize::from(date.month()).saturating_sub(1);
        let name = MONTHS.get(month).copied().unwrap_or("");
        let mut parts: Vec<String> = Vec::new();
        let mut sep = String::new();
        for t in tokens(self.0) {
            match t {
                Tok::Year4 | Tok::Year2 => {
                    // A separator next to the year is not between day and month.
                    if parts.len() == 1 {
                        sep.clear();
                    }
                }
                Tok::Month2 => parts.push(format!("{:02}", date.month())),
                Tok::Month1 => parts.push(date.month().to_string()),
                Tok::Day2 => parts.push(format!("{:02}", date.day())),
                Tok::Day1 => parts.push(date.day().to_string()),
                Tok::MonAbbr | Tok::MonthName => parts.push(name.chars().take(3).collect()),
                Tok::Lit(c) => {
                    if parts.len() == 1 && c != ',' {
                        sep.push(c);
                    }
                }
            }
        }
        if sep.is_empty() {
            sep.push('-');
        }
        parts.join(&sep)
    }

    /// A value from a command written for display: an ISO date (or the date part of an
    /// ISO date-time, whose time part is kept) in this style; anything else unchanged.
    pub fn show(self, text: &str) -> String {
        let Some(head) = text.get(..10) else {
            return text.to_owned();
        };
        let tail = text.get(10..).unwrap_or("");
        let date_time = tail.is_empty() || tail.starts_with('T') || tail.starts_with(' ');
        match head.parse::<Date>() {
            Ok(d) if date_time => format!("{}{tail}", self.format(d)),
            _ => text.to_owned(),
        }
    }

    /// Read a typed date (revision 20 rules); `today` supplies a missing year and the
    /// two-digit-year window. `None` when the text is not a valid date.
    pub fn read(self, text: &str, today: Date) -> Option<Date> {
        let t = text.trim();
        if t.is_empty() {
            return None;
        }
        if let Some(d) = read_year_first(t, today) {
            return Some(d);
        }
        if self.has_names() {
            self.read_named(t, today)
        } else {
            self.read_numeric(t, today)
        }
    }

    fn read_numeric(self, t: &str, today: Date) -> Option<Date> {
        let fields = self.fields();
        let no_seps = !tokens(self.0).iter().any(|x| matches!(x, Tok::Lit(_)));
        if no_seps {
            // YYYYMMDD: exactly eight digits.
            if t.len() != 8 || !t.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            return make(
                t.get(0..4)?.parse().ok()?,
                t.get(4..6)?.parse().ok()?,
                t.get(6..8)?.parse().ok()?,
            );
        }
        let parts: Vec<&str> = t.split(is_lenient_sep).filter(|p| !p.is_empty()).collect();
        if parts.iter().any(|p| !p.bytes().all(|b| b.is_ascii_digit())) {
            return None;
        }
        // Rule 2: no year → the current year.
        let order: Vec<Field> = if parts.len() == 2 {
            fields.into_iter().filter(|f| *f != Field::Year).collect()
        } else {
            fields
        };
        if parts.len() != order.len() {
            return None;
        }
        let (mut y, mut m, mut d) = (Some(today.year()), None, None);
        for (part, field) in parts.iter().zip(&order) {
            match field {
                Field::Year => y = year_of(part, today),
                Field::Month => m = small(part),
                Field::Day => d = small(part),
            }
        }
        make(y?, m?, d?)
    }

    fn read_named(self, t: &str, today: Date) -> Option<Date> {
        // Words and digit groups, ignoring separators and the comma.
        let mut words: Vec<String> = Vec::new();
        let mut cur = String::new();
        for c in t.chars() {
            let same_kind = cur
                .chars()
                .next()
                .is_none_or(|f| f.is_ascii_digit() == c.is_ascii_digit());
            if c.is_alphanumeric() && same_kind {
                cur.push(c);
            } else {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
                if c.is_alphanumeric() {
                    cur.push(c);
                } else if !(is_lenient_sep(c) || c == ',') {
                    return None;
                }
            }
        }
        if !cur.is_empty() {
            words.push(cur);
        }
        let month_pos = words.iter().position(|w| month_of(w).is_some())?;
        let month = month_of(words.get(month_pos)?)?;
        let numbers: Vec<&String> = words
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != month_pos)
            .map(|(_, w)| w)
            .collect();
        if numbers
            .iter()
            .any(|w| !w.bytes().all(|b| b.is_ascii_digit()))
        {
            return None;
        }
        // The day/month order follows the pattern; the year comes after the day.
        let fields = self.fields();
        let month_first = fields.iter().position(|f| *f == Field::Month)
            < fields.iter().position(|f| *f == Field::Day);
        let day = numbers.first()?;
        let day_pos = words.iter().position(|w| std::ptr::eq(w, *day))?;
        if month_first != (month_pos < day_pos) {
            return None;
        }
        let year = match numbers.as_slice() {
            [_] => Some(today.year()),
            [_, y] => year_of(y, today),
            _ => None,
        }?;
        make(year, month, small(day)?)
    }

    /// The rest of the pattern to show greyed after `typed`, while what is typed so
    /// far fits the pattern character by character; `None` when it does not fit (the
    /// guide is then hidden). For empty text, the whole pattern.
    pub fn guide(self, typed: &str) -> Option<&'static str> {
        let slots: Vec<char> = self.0.chars().collect();
        let name_spans = name_slots(self.0);
        let typed: Vec<char> = typed.chars().collect();
        if typed.len() > slots.len() {
            return None;
        }
        for (i, c) in typed.iter().enumerate() {
            let slot = *slots.get(i)?;
            let fits = if name_spans.get(i).copied().unwrap_or(false) {
                c.is_alphabetic()
            } else if matches!(slot, 'Y' | 'M' | 'D') {
                c.is_ascii_digit()
            } else if is_lenient_sep(slot) {
                is_lenient_sep(*c)
            } else {
                *c == slot
            };
            if !fits {
                return None;
            }
        }
        let byte = self
            .0
            .char_indices()
            .nth(typed.len())
            .map_or(self.0.len(), |(b, _)| b);
        self.0.get(byte..)
    }
}

/// Which character positions of a pattern belong to a month name (`Mon`, `Month`).
fn name_slots(pattern: &str) -> Vec<bool> {
    let mut out = Vec::new();
    for t in tokens(pattern) {
        match t {
            Tok::MonthName => out.extend([true; 5]),
            Tok::MonAbbr => out.extend([true; 3]),
            Tok::Year4 => out.extend([false; 4]),
            Tok::Year2 | Tok::Month2 | Tok::Day2 => out.extend([false; 2]),
            Tok::Month1 | Tok::Day1 | Tok::Lit(_) => out.push(false),
        }
    }
    out
}

/// Rule 4 (ISO, and any four-digit first field): year, month, day.
fn read_year_first(t: &str, _today: Date) -> Option<Date> {
    let parts: Vec<&str> = t.split(is_lenient_sep).collect();
    let [y, m, d] = parts.as_slice() else {
        return None;
    };
    if y.len() != 4 || !y.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    make(y.parse().ok()?, small(m)?, small(d)?)
}

/// A month or day field: one or two digits.
fn small(s: &str) -> Option<u8> {
    ((1..=2).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// A year field: four digits as written, or two digits within current year − 80 …
/// current year + 19.
fn year_of(s: &str, today: Date) -> Option<i32> {
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match s.len() {
        4 => s.parse().ok(),
        2 => {
            let yy: i32 = s.parse().ok()?;
            let base = today.year().checked_sub(80)?;
            Some(base + (yy - base).rem_euclid(100))
        }
        _ => None,
    }
}

/// The month named by a full English name or its three-letter abbreviation.
fn month_of(word: &str) -> Option<u8> {
    let w = word.to_lowercase();
    MONTHS
        .iter()
        .position(|m| {
            let m = m.to_lowercase();
            w == m || (w.chars().count() == 3 && m.starts_with(&w))
        })
        .and_then(|i| u8::try_from(i + 1).ok())
}

fn make(year: i32, month: u8, day: u8) -> Option<Date> {
    if !(1..=9999).contains(&year) {
        return None;
    }
    Date::from_ymd(year, month, day).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn style(p: &str) -> DateStyle {
        DateStyle::parse(p).unwrap()
    }

    fn today() -> Date {
        Date::from_ymd(2026, 10, 22).unwrap()
    }

    fn read(p: &str, t: &str) -> Option<String> {
        style(p).read(t, today()).map(|d| d.to_string())
    }

    #[test]
    fn every_pattern_formats_and_reads_back() {
        let d = Date::from_ymd(2026, 9, 5).unwrap();
        for s in DateStyle::all() {
            let text = s.format(d);
            assert_eq!(s.read(&text, today()), Some(d), "{} {text}", s.pattern());
        }
    }

    #[test]
    fn day_month_headings() {
        let d = Date::from_ymd(2026, 10, 19).unwrap();
        let jan = Date::from_ymd(2026, 1, 5).unwrap();
        let cases = [
            ("MM-DD-YYYY", "10-19", "01-05"),
            ("MM/DD/YY", "10/19", "01/05"),
            ("M/D/YYYY", "10/19", "1/5"),
            ("DD.MM.YYYY", "19.10", "05.01"),
            ("D/M/YYYY", "19/10", "5/1"),
            ("YYYY-MM-DD", "10-19", "01-05"),
            ("YYYY/MM/DD", "10/19", "01/05"),
            ("YYYY.MM.DD", "10.19", "01.05"),
            ("YYYYMMDD", "10-19", "01-05"),
            ("DD-Mon-YYYY", "19-Oct", "05-Jan"),
            ("DD Mon YYYY", "19 Oct", "05 Jan"),
            ("DD Month YYYY", "19 Oct", "05 Jan"),
            ("Mon DD, YYYY", "Oct 19", "Jan 05"),
            ("Month DD, YYYY", "Oct 19", "Jan 05"),
        ];
        for (p, want, want_jan) in cases {
            assert_eq!(style(p).day_month(d), want, "{p}");
            assert_eq!(style(p).day_month(jan), want_jan, "{p}");
        }
    }

    #[test]
    fn formats() {
        let d = Date::from_ymd(2026, 10, 22).unwrap();
        let cases = [
            ("MM-DD-YYYY", "10-22-2026"),
            ("M/D/YY", "10/22/26"),
            ("DD.MM.YY", "22.10.26"),
            ("YYYYMMDD", "20261022"),
            ("DD-Mon-YYYY", "22-Oct-2026"),
            ("DD Mon YYYY", "22 Oct 2026"),
            ("Mon DD, YYYY", "Oct 22, 2026"),
            ("Month DD, YYYY", "October 22, 2026"),
            ("DD Month YYYY", "22 October 2026"),
        ];
        for (p, want) in cases {
            assert_eq!(style(p).format(d), want, "{p}");
        }
        let jan = Date::from_ymd(2026, 1, 5).unwrap();
        assert_eq!(style("M/D/YYYY").format(jan), "1/5/2026");
        assert_eq!(style("D.M.YYYY").format(jan), "5.1.2026");
        let sep = Date::from_ymd(2026, 9, 30).unwrap();
        assert_eq!(style("Mon DD, YYYY").format(sep), "Sep 30, 2026");
    }

    #[test]
    fn show_keeps_times_and_other_text() {
        let s = style("DD Mon YYYY");
        assert_eq!(s.show("2026-10-22"), "22 Oct 2026");
        assert_eq!(s.show("2026-10-22T10:30:00Z"), "22 Oct 2026T10:30:00Z");
        assert_eq!(s.show("2026-10-22 10:30"), "22 Oct 2026 10:30");
        assert_eq!(s.show("not a date"), "not a date");
        assert_eq!(s.show(""), "");
        assert_eq!(s.show("2026-10-221"), "2026-10-221");
    }

    #[test]
    fn reading_rules() {
        // Rule 1: lenient separators and optional leading zeros.
        assert_eq!(read("MM-DD-YYYY", "1/5/26").as_deref(), Some("2026-01-05"));
        assert_eq!(
            read("MM-DD-YYYY", "1-5-2026").as_deref(),
            Some("2026-01-05")
        );
        assert_eq!(
            read("MM-DD-YYYY", "01 05 2026").as_deref(),
            Some("2026-01-05")
        );
        assert_eq!(read("MM-DD-YYYY", "13-01-2026"), None);
        assert_eq!(read("MM-DD-YYYY", "02-30-2026"), None);
        assert_eq!(
            read("DD/MM/YYYY", "05/01/2026").as_deref(),
            Some("2026-01-05")
        );
        assert_eq!(read("YYYYMMDD", "20260105").as_deref(), Some("2026-01-05"));
        assert_eq!(read("YYYYMMDD", "2026015"), None);
        // Rule 2: no year.
        assert_eq!(read("MM-DD-YYYY", "10-22").as_deref(), Some("2026-10-22"));
        assert_eq!(read("DD.MM.YYYY", "22.10").as_deref(), Some("2026-10-22"));
        assert_eq!(read("DD Mon YYYY", "22 oct").as_deref(), Some("2026-10-22"));
        // Rule 3: two- and four-digit years, with the window.
        assert_eq!(
            read("MM/DD/YY", "01/05/2026").as_deref(),
            Some("2026-01-05")
        );
        assert_eq!(read("MM/DD/YY", "01/05/45").as_deref(), Some("2045-01-05"));
        assert_eq!(read("MM/DD/YY", "01/05/46").as_deref(), Some("1946-01-05"));
        assert_eq!(read("MM/DD/YY", "01/05/202"), None);
        // Rule 4: ISO always.
        for p in PATTERNS {
            assert_eq!(read(p, "2026-01-05").as_deref(), Some("2026-01-05"), "{p}");
        }
        // Rule 5: names in any form, any case, optional comma.
        assert_eq!(
            read("Mon DD, YYYY", "oct 22 2026").as_deref(),
            Some("2026-10-22")
        );
        assert_eq!(
            read("Mon DD, YYYY", "October 22, 2026").as_deref(),
            Some("2026-10-22")
        );
        assert_eq!(
            read("DD-Mon-YYYY", "22-OCTOBER-2026").as_deref(),
            Some("2026-10-22")
        );
        assert_eq!(
            read("DD Month YYYY", "22 sep 2026").as_deref(),
            Some("2026-09-22")
        );
        assert_eq!(read("Mon DD, YYYY", "22 Oct 2026"), None, "order follows F");
        assert_eq!(read("Mon DD, YYYY", "Foo 22 2026"), None);
        assert_eq!(read("MM-DD-YYYY", ""), None);
        assert_eq!(read("MM-DD-YYYY", "10-22-2026x"), None);
    }

    #[test]
    fn guide() {
        let s = style("MM-DD-YYYY");
        assert_eq!(s.guide(""), Some("MM-DD-YYYY"));
        assert_eq!(s.guide("10-2"), Some("D-YYYY"));
        assert_eq!(s.guide("10/2"), Some("D-YYYY"), "lenient separator");
        assert_eq!(s.guide("1-"), None, "leading zero left out");
        assert_eq!(s.guide("10-22-2026"), Some(""));
        assert_eq!(s.guide("10-22-20261"), None);
        let n = style("Mon DD, YYYY");
        assert_eq!(n.guide("Oc"), Some("n DD, YYYY"));
        assert_eq!(n.guide("Oct 2"), Some("D, YYYY"));
        assert_eq!(n.guide("October"), None);
        assert_eq!(style("Month DD, YYYY").guide("Octo"), Some("h DD, YYYY"));
    }

    #[test]
    fn listing() {
        assert_eq!(DateStyle::default().pattern(), "MM-DD-YYYY");
        assert_eq!(group_of("M/D/YY"), "Month first");
        assert_eq!(group_of("D.M.YYYY"), "Day first");
        assert_eq!(group_of("YYYYMMDD"), "Year first");
        assert_eq!(group_of("Mon DD, YYYY"), "Month name");
        assert_eq!(DateStyle::parse("nonsense"), None);
        assert_eq!(style("MM-DD-YYYY").error(), "Enter the date as MM-DD-YYYY");
    }
}
