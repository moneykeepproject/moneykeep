//! Text decoding: UTF-8 (with or without BOM), otherwise Windows-1252.

/// Windows-1252 code points for bytes 0x80..=0x9F (undefined bytes map to U+FFFD).
const CP1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}', '\u{017D}', '\u{FFFD}',
    '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
];

/// Decode bytes as Windows-1252, whatever they look like.
pub fn decode_cp1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => CP1252_HIGH
                .get(usize::from(b - 0x80))
                .copied()
                .unwrap_or('\u{FFFD}'),
            _ => char::from(b),
        })
        .collect()
}

/// Decode bytes as ISO-8859-1 (Latin-1).
pub fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// Decode file bytes to text.
pub fn decode_text(bytes: &[u8]) -> String {
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(body) {
        Ok(s) => s.to_owned(),
        Err(_) => body
            .iter()
            .map(|&b| match b {
                0x80..=0x9F => CP1252_HIGH
                    .get(usize::from(b - 0x80))
                    .copied()
                    .unwrap_or('\u{FFFD}'),
                _ => char::from(b),
            })
            .collect(),
    }
}

/// Split text into lines on CRLF, LF or CR.
pub(crate) fn lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes.get(i) {
            Some(b'\r') => {
                out.push(text.get(start..i).unwrap_or(""));
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            Some(b'\n') => {
                out.push(text.get(start..i).unwrap_or(""));
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < text.len() {
        out.push(text.get(start..).unwrap_or(""));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_bom_and_cp1252() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFCaf\xC3\xA9"), "Café");
        assert_eq!(decode_text(b"Caf\xE9 \x80"), "Café €");
    }

    #[test]
    fn line_endings() {
        assert_eq!(lines("a\r\nb\rc\nd"), ["a", "b", "c", "d"]);
        assert_eq!(lines("a\n\nb\n"), ["a", "", "b"]);
    }
}
