//! Text safety (input-safety.md R3, R6; definition-files.md text rules): control,
//! bidirectional-formatting and invisible characters, and the cleaning applied to
//! every text value imported from a data file.

use unicode_normalization::UnicodeNormalization;

/// C0 and C1 control characters (U+0000–U+001F, U+007F–U+009F).
pub fn is_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1F}' | '\u{7F}'..='\u{9F}')
}

/// Bidirectional formatting characters (UTS #55, CVE-2021-42574).
pub fn is_bidi(c: char) -> bool {
    matches!(
        c,
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}'
    )
}

/// Zero-width and invisible characters.
pub fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{180E}'
    )
}

/// Text normalised to NFC.
pub fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// The result of cleaning one text value.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cleaned {
    pub text: String,
    /// Control characters (other than tab, CR, LF) were removed.
    pub controls: bool,
    /// Bidirectional or invisible characters were removed.
    pub invisible: bool,
    /// The value was cut to its length limit.
    pub truncated: bool,
}

impl Cleaned {
    /// Plain notes describing what changed (empty when nothing did).
    pub fn notes(&self, field: &str, limit: usize) -> Vec<String> {
        let mut n = Vec::new();
        if self.controls {
            n.push(format!("control characters removed from {field}"));
        }
        if self.invisible {
            n.push(format!("invisible characters removed from {field}"));
        }
        if self.truncated {
            n.push(format!("{field} cut to {limit} characters"));
        }
        n
    }

    pub fn changed(&self) -> bool {
        self.controls || self.invisible || self.truncated
    }
}

/// Clean an imported text value (input-safety.md R3): tab, CR and LF become a space;
/// other control characters, bidirectional formatting and invisible characters are
/// removed; the text is normalised to NFC, trimmed, runs of spaces collapsed, and cut
/// to `limit` characters.
pub fn clean(text: &str, limit: usize) -> Cleaned {
    let mut out = Cleaned::default();
    let mut s = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\t' | '\r' | '\n') {
            s.push(' ');
        } else if is_control(c) {
            out.controls = true;
        } else if is_bidi(c) || is_invisible(c) {
            out.invisible = true;
        } else {
            s.push(c);
        }
    }
    let normal: String = s.nfc().collect();
    let mut collapsed = String::with_capacity(normal.len());
    let mut space = false;
    for c in normal.trim().chars() {
        if c == ' ' {
            if !space {
                collapsed.push(' ');
            }
            space = true;
        } else {
            collapsed.push(c);
            space = false;
        }
    }
    if collapsed.chars().count() > limit {
        out.truncated = true;
        collapsed = collapsed
            .chars()
            .take(limit)
            .collect::<String>()
            .trim_end()
            .to_owned();
    }
    out.text = collapsed;
    out
}

/// Text made safe for a terminal (input-safety.md R6): control characters other than
/// tab and line feed, and bidirectional formatting characters, are removed.
pub fn terminal_safe(text: &str) -> String {
    text.chars()
        .filter(|c| matches!(c, '\t' | '\n') || !(is_control(*c) || is_bidi(*c)))
        .collect()
}

/// Original record text for display: control characters shown as Unicode control
/// pictures (`␀`, `␛`, …) and bidirectional/invisible characters as `<U+XXXX>`, so
/// nothing in it acts as formatting.
pub fn escape_raw(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let n = u32::from(c);
        if c == '\n' || c == '\t' {
            out.push(c);
        } else if n < 0x20 {
            out.push(char::from_u32(0x2400 + n).unwrap_or('\u{FFFD}'));
        } else if n == 0x7F {
            out.push('\u{2421}');
        } else if is_control(c) || is_bidi(c) || is_invisible(c) {
            out.push_str(&format!("<U+{n:04X}>"));
        } else {
            out.push(c);
        }
    }
    out
}

/// Scripts a letter belongs to, for the mixed-script warning (a coarse subset of
/// UTS #39: Latin, Greek and Cyrillic, the usual look-alike sources).
fn script(c: char) -> Option<u8> {
    match c {
        'A'..='Z' | 'a'..='z' | '\u{00C0}'..='\u{024F}' => Some(0),
        '\u{0370}'..='\u{03FF}' => Some(1),
        '\u{0400}'..='\u{052F}' => Some(2),
        _ => None,
    }
}

/// Whether text mixes Latin, Greek and Cyrillic letters (a look-alike risk).
pub fn mixed_script(text: &str) -> bool {
    let mut seen: Option<u8> = None;
    for s in text.chars().filter_map(script) {
        match seen {
            None => seen = Some(s),
            Some(x) if x != s => return true,
            Some(_) => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_controls_bidi_and_spaces() {
        let c = clean("  A\u{1b}[2J\u{0}B\u{202E}C\tD   E ", 100);
        assert_eq!(c.text, "A[2JBC D E");
        assert!(c.controls && c.invisible && !c.truncated);
        let c = clean("abcdef", 3);
        assert_eq!(c.text, "abc");
        assert!(c.truncated);
        assert_eq!(clean("e\u{301}", 10).text, "\u{e9}");
    }

    #[test]
    fn terminal_and_raw_display() {
        assert_eq!(terminal_safe("a\u{1b}[31mb\u{9b}\n"), "a[31mb\n");
        assert_eq!(
            escape_raw("a\u{0}\u{1b}\u{202E}"),
            "a\u{2400}\u{241B}<U+202E>"
        );
        assert!(mixed_script("P\u{0430}yee"));
        assert!(!mixed_script("Café"));
    }
}
