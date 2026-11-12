//! Duplicate-detection fingerprints and payee normalisation.

use std::collections::HashMap;

use mk_core::{AccountId, Date, Decimal};
use sha2::{Digest, Sha256};

/// Upper-case, collapse whitespace, strip leading/trailing punctuation.
pub fn normalise_payee(payee: &str) -> String {
    let upper = payee.to_uppercase();
    let collapsed = upper.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
        .to_owned()
}

/// Fingerprint of a row with a source id.
pub fn external_fingerprint(account: AccountId, external_id: &str) -> String {
    format!("ext:{account}:{external_id}")
}

/// Fingerprint of a row without a source id.
pub fn content_fingerprint(
    account: AccountId,
    date: Date,
    amount: Decimal,
    payee: &str,
    number: &str,
    occurrence: usize,
) -> String {
    let material = format!(
        "{date}|{}|{}|{}|{occurrence}",
        amount.normalize(),
        normalise_payee(payee),
        number.trim()
    );
    let digest = Sha256::digest(material.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("hash:{account}:{hex}")
}

/// Occurrence number (1, 2, 3, …) of each row among rows with identical date, amount,
/// normalised payee and number, in the given (file) order.
pub fn occurrences<'a>(
    rows: impl IntoIterator<Item = (Date, Decimal, &'a str, &'a str)>,
) -> Vec<usize> {
    let mut seen: HashMap<(Date, Decimal, String, String), usize> = HashMap::new();
    rows.into_iter()
        .map(|(d, a, p, n)| {
            let key = (d, a.normalize(), normalise_payee(p), n.trim().to_owned());
            let c = seen.entry(key).or_insert(0);
            *c += 1;
            *c
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation() {
        assert_eq!(
            normalise_payee("  kroger   #123 lima, oh. "),
            "KROGER #123 LIMA, OH"
        );
        assert_eq!(normalise_payee("--Shell--"), "SHELL");
    }

    #[test]
    fn identical_rows_get_distinct_fingerprints() {
        let a = AccountId::new();
        let d: Date = "2026-10-01".parse().unwrap();
        let amt: Decimal = "-5.00".parse().unwrap();
        let occ = occurrences(vec![
            (d, amt, "Coffee", ""),
            (d, "-5".parse().unwrap(), "COFFEE ", ""),
        ]);
        assert_eq!(occ, [1, 2]);
        let f1 = content_fingerprint(a, d, amt, "Coffee", "", 1);
        let f2 = content_fingerprint(a, d, amt, "Coffee", "", 2);
        assert_ne!(f1, f2);
        assert_eq!(
            f1,
            content_fingerprint(a, d, "-5".parse().unwrap(), "COFFEE", "", 1)
        );
        assert!(f1.starts_with("hash:"));
        assert_eq!(external_fingerprint(a, "X1"), format!("ext:{a}:X1"));
    }
}
