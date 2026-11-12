//! Currency facts needed for amount precision (ISO 4217 minor units).

use rust_decimal::Decimal;

/// Number of decimal places (minor units) of a currency, per ISO 4217.
///
/// Codes not in the table (including security symbols) default to 2.
pub fn minor_units(code: &str) -> u32 {
    match code {
        // Zero-decimal currencies.
        "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX"
        | "UYI" | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        // Three-decimal currencies.
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        // Four-decimal units.
        "CLF" | "UYW" => 4,
        _ => 2,
    }
}

/// Active ISO 4217 alphabetic currency codes (plus the fund and precious-metal
/// codes), so a security symbol is never mistaken for a currency.
const ISO_4217: &[&str] = &[
    "AED", "AFN", "ALL", "AMD", "ANG", "AOA", "ARS", "AUD", "AWG", "AZN", "BAM", "BBD", "BDT",
    "BGN", "BHD", "BIF", "BMD", "BND", "BOB", "BOV", "BRL", "BSD", "BTN", "BWP", "BYN", "BZD",
    "CAD", "CDF", "CHE", "CHF", "CHW", "CLF", "CLP", "CNY", "COP", "COU", "CRC", "CUC", "CUP",
    "CVE", "CZK", "DJF", "DKK", "DOP", "DZD", "EGP", "ERN", "ETB", "EUR", "FJD", "FKP", "GBP",
    "GEL", "GHS", "GIP", "GMD", "GNF", "GTQ", "GYD", "HKD", "HNL", "HTG", "HUF", "IDR", "ILS",
    "INR", "IQD", "IRR", "ISK", "JMD", "JOD", "JPY", "KES", "KGS", "KHR", "KMF", "KPW", "KRW",
    "KWD", "KYD", "KZT", "LAK", "LBP", "LKR", "LRD", "LSL", "LYD", "MAD", "MDL", "MGA", "MKD",
    "MMK", "MNT", "MOP", "MRU", "MUR", "MVR", "MWK", "MXN", "MXV", "MYR", "MZN", "NAD", "NGN",
    "NIO", "NOK", "NPR", "NZD", "OMR", "PAB", "PEN", "PGK", "PHP", "PKR", "PLN", "PYG", "QAR",
    "RON", "RSD", "RUB", "RWF", "SAR", "SBD", "SCR", "SDG", "SEK", "SGD", "SHP", "SLE", "SLL",
    "SOS", "SRD", "SSP", "STN", "SVC", "SYP", "SZL", "THB", "TJS", "TMT", "TND", "TOP", "TRY",
    "TTD", "TWD", "TZS", "UAH", "UGX", "USD", "USN", "UYI", "UYU", "UYW", "UZS", "VED", "VES",
    "VND", "VUV", "WST", "XAF", "XAG", "XAU", "XBA", "XBB", "XBC", "XBD", "XCD", "XCG", "XDR",
    "XOF", "XPD", "XPF", "XPT", "XSU", "XTS", "XUA", "XXX", "YER", "ZAR", "ZMW", "ZWG", "ZWL",
];

/// Whether `code` is an ISO 4217 currency code.
pub fn is_iso_currency(code: &str) -> bool {
    ISO_4217.contains(&code)
}

/// Maximum decimal places of a split quantity.
pub const MAX_QUANTITY_DP: u32 = 9;

/// Format an amount for output.
///
/// With `Some(dp)` (a currency's minor units) the amount is written with at least `dp`
/// decimal places, e.g. `1000.00` or `0.00` for USD and `1000` for JPY. Significant
/// digits beyond `dp` are never dropped. With `None` (a security quantity) trailing
/// zeros are removed and nothing is padded.
pub fn format_amount(d: Decimal, scale: Option<u32>) -> String {
    let mut n = d.normalize();
    if let Some(dp) = scale
        && n.scale() < dp
    {
        n.rescale(dp);
    }
    n.to_string()
}

/// Significant decimal places of `d` (trailing zeros do not count).
pub fn decimal_places(d: Decimal) -> u32 {
    d.normalize().scale()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_minor_units() {
        for (code, dp) in [
            ("USD", 2),
            ("EUR", 2),
            ("GBP", 2),
            ("CAD", 2),
            ("AUD", 2),
            ("JPY", 0),
            ("KWD", 3),
            ("ZZZ", 2),
            ("VTI", 2),
        ] {
            assert_eq!(minor_units(code), dp, "{code}");
        }
    }

    #[test]
    fn formats_at_currency_scale() {
        let d = |s: &str| s.parse::<Decimal>().unwrap();
        assert_eq!(format_amount(d("1000"), Some(2)), "1000.00");
        assert_eq!(format_amount(d("0"), Some(2)), "0.00");
        assert_eq!(format_amount(d("-0.5"), Some(2)), "-0.50");
        assert_eq!(format_amount(d("1000.00"), Some(0)), "1000");
        assert_eq!(
            format_amount(d("1.2345"), Some(2)),
            "1.2345",
            "never drops digits"
        );
        assert_eq!(format_amount(d("10.000"), None), "10");
        assert_eq!(format_amount(d("0.125000"), None), "0.125");
    }

    #[test]
    fn trailing_zeros_are_not_significant() {
        assert_eq!(decimal_places("10.050".parse().unwrap()), 2);
        assert_eq!(decimal_places("10.005".parse().unwrap()), 3);
        assert_eq!(decimal_places("100".parse().unwrap()), 0);
    }
}
