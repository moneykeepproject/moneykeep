//! The report and period helpers shared by the views: period labels and chart ticks.
#![allow(clippy::float_arithmetic)]

use crate::fmt;

/// What a row is.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    /// A section heading ("Income"): name only.
    Heading,
    /// An account row; clickable to show its transactions.
    Account,
    /// An account row that opens nothing.
    Plain,
    /// A totals row (bold).
    Total,
}

#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub name: String,
    pub indent: u64,
    pub cells: Vec<String>,
    pub total: Option<String>,
    pub kind: Kind,
}

/// Short header for a period label: months "Oct" (with the year above the first
/// month of each year), quarters "Q4 2026", years "2026", a single date as its day
/// and month in the chosen date format ("10-19" with the year above).
pub(crate) fn short_label(label: &str) -> (Option<String>, String) {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    if let Some((y, q)) = label.split_once("-Q") {
        return (None, format!("Q{q} {y}"));
    }
    // A single date (net worth's "today" point): its day and month in the chosen
    // date format, the year above (revision 22).
    if let Ok(d) = label.parse::<mk_core::Date>() {
        return (Some(d.year().to_string()), fmt::date_style().day_month(d));
    }
    match label.split_once('-') {
        Some((y, m)) if m.len() == 2 => {
            let idx = m.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
            let name = idx.and_then(|i| MONTHS.get(i)).copied().unwrap_or(m);
            (Some(y.to_owned()), name.to_owned())
        }
        _ => (None, label.to_owned()),
    }
}

/// Headers for a list of period labels: the year is shown above the first period
/// and above every January.
pub(crate) fn headers(labels: &[String]) -> Vec<(Option<String>, String)> {
    let mut last_year: Option<String> = None;
    labels
        .iter()
        .map(|l| {
            let (year, short) = short_label(l);
            let shown = match &year {
                Some(y) if last_year.as_ref() != Some(y) => Some(y.clone()),
                _ => None,
            };
            if year.is_some() {
                last_year = year;
            }
            (shown, short)
        })
        .collect()
}

/// A round tick step (1, 2 or 5 × a power of ten) giving about four intervals.
pub(crate) fn nice_step(span: f64) -> f64 {
    if span <= 0.0 || !span.is_finite() {
        return 1.0;
    }
    let raw = span / 4.0;
    let pow = 10f64.powf(raw.log10().floor());
    let unit = raw / pow;
    let m = if unit <= 1.0 {
        1.0
    } else if unit <= 2.0 {
        2.0
    } else if unit <= 5.0 {
        5.0
    } else {
        10.0
    };
    (m * pow).max(1.0)
}

/// Round ticks covering `lo..=hi` (which include zero), zero among them.
pub(crate) fn ticks(lo: f64, hi: f64) -> Vec<f64> {
    let step = nice_step(hi - lo);
    let first = (lo / step).floor() as i64;
    let last = (hi / step).ceil() as i64;
    let last = if last == first { first + 1 } else { last };
    (first..=last).map(|k| k as f64 * step).collect()
}
