//! Text from the phone, cleaned as `input-safety.md` R3 describes (control,
//! bidirectional-formatting and invisible characters removed, spaces collapsed) and
//! then checked against its limit. Unlike file imports, text over the limit is
//! refused rather than truncated: the person typing can shorten it.

fn removed(c: char) -> bool {
    matches!(
        c,
        '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{061C}'
            | '\u{200B}'..='\u{200D}'
            | '\u{2060}'
            | '\u{FEFF}'
            | '\u{180E}'
    ) || (c.is_control() && !matches!(c, '\t' | '\r' | '\n'))
}

/// Clean `s`; `Ok(None)` when nothing is left, `Err` with a message naming `what`
/// when the cleaned text has more than `max` characters.
pub fn clean_text(s: &str, max: usize, what: &str) -> Result<Option<String>, String> {
    let mut out = String::with_capacity(s.len().min(max.saturating_mul(4)));
    let mut space = false;
    for c in s.chars() {
        if removed(c) {
            continue;
        }
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    if out.is_empty() {
        return Ok(None);
    }
    if out.chars().count() > max {
        return Err(format!("{what} can be at most {max} characters."));
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_and_limits() {
        assert_eq!(
            clean_text("  Office\u{202E} \u{0007}Depot\t\n ", 100, "Merchant"),
            Ok(Some("Office Depot".into()))
        );
        assert_eq!(clean_text(" \u{200B} ", 100, "Merchant"), Ok(None));
        assert_eq!(
            clean_text(&"x".repeat(101), 100, "Merchant"),
            Err("Merchant can be at most 100 characters.".into())
        );
        assert_eq!(
            clean_text(&"é".repeat(100), 100, "Merchant").map(|s| s.is_some()),
            Ok(true)
        );
    }
}
