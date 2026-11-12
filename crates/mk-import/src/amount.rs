//! Amount text parsing (exact decimals only).

use mk_core::Decimal;

use crate::{ImportError, ImportResult};

/// How amount text is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AmountStyle {
    /// `,` is the decimal separator and `.` groups thousands.
    pub decimal_comma: bool,
}

/// Parse amount text: optional sign, currency symbols (`$ € £ ¥`), spaces, thousands
/// separators, and parentheses for negative. Returns `None` for empty text.
pub fn parse_amount(text: &str, style: AmountStyle) -> ImportResult<Option<Decimal>> {
    crate::safety::check_number_len(text)?;
    let bad = || ImportError::Invalid(format!("invalid amount {text:?}"));
    let mut t: String = text
        .chars()
        .filter(|c| !matches!(c, '$' | '€' | '£' | '¥') && !c.is_whitespace())
        .collect();
    if t.is_empty() {
        return Ok(None);
    }
    let mut negative = false;
    if t.starts_with('(') && t.ends_with(')') && t.len() >= 2 {
        negative = true;
        t = t.get(1..t.len() - 1).ok_or_else(bad)?.to_owned();
    }
    if let Some(rest) = t.strip_prefix('-') {
        negative = !negative;
        t = rest.to_owned();
    } else if let Some(rest) = t.strip_prefix('+') {
        t = rest.to_owned();
    }
    let (group, dec) = if style.decimal_comma {
        ('.', ',')
    } else {
        (',', '.')
    };
    let t: String = t
        .chars()
        .filter(|c| *c != group)
        .map(|c| if c == dec { '.' } else { c })
        .collect();
    if t.is_empty()
        || !t.chars().all(|c| c.is_ascii_digit() || c == '.')
        || t.matches('.').count() > 1
    {
        return Err(bad());
    }
    let d: Decimal = t.parse().map_err(|_| bad())?;
    Ok(Some(if negative { -d } else { d }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str, comma: bool) -> String {
        parse_amount(
            s,
            AmountStyle {
                decimal_comma: comma,
            },
        )
        .unwrap()
        .unwrap()
        .to_string()
    }

    #[test]
    fn styles() {
        assert_eq!(p("-1,234.56", false), "-1234.56");
        assert_eq!(p("-1.234,56", true), "-1234.56");
        assert_eq!(p("(1,234.56)", false), "-1234.56");
        assert_eq!(p("€1.234,56", true), "1234.56");
        assert_eq!(p("$12.50", false), "12.50");
        assert_eq!(p("+3", false), "3");
        assert!(parse_amount("", AmountStyle::default()).unwrap().is_none());
        assert!(parse_amount("12a", AmountStyle::default()).is_err());
        assert!(parse_amount("1.2.3", AmountStyle::default()).is_err());
    }
}
