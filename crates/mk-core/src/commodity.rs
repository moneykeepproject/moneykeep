use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use crate::{CoreError, CoreResult};

/// Identifier of a currency (ISO 4217, e.g. `USD`) or a security symbol (e.g. `VTI`).
///
/// Codes are 1-16 characters of uppercase ASCII letters, digits, `.`, `-` or `_`, or
/// an option symbol of up to 24 characters in the form `<UNDERLYING> <YYMMDD>
/// <C|P><strike>` (investments-options-bonds-crypto.md: spaces for options only).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CommodityCode(String);

impl CommodityCode {
    pub fn new(code: &str) -> CoreResult<Self> {
        let plain = |s: &str| {
            s.chars().all(|c| {
                c.is_ascii_uppercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_')
            })
        };
        let valid = if code.contains(' ') {
            Self::is_option_form(code)
        } else {
            (1..=16).contains(&code.len()) && plain(code)
        };
        if valid {
            Ok(Self(code.to_owned()))
        } else {
            Err(CoreError::InvalidCommodity(code.to_owned()))
        }
    }

    /// `<UNDERLYING> <YYMMDD> <C|P><strike>`, at most 24 characters.
    pub fn is_option_form(code: &str) -> bool {
        let parts: Vec<&str> = code.split(' ').collect();
        let [under, date, strike] = parts.as_slice() else {
            return false;
        };
        let under_ok = (1..=12).contains(&under.len())
            && under
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || matches!(c, '.' | '-'));
        let date_ok = date.len() == 6 && date.chars().all(|c| c.is_ascii_digit());
        let mut sc = strike.chars();
        let right_ok = matches!(sc.next(), Some('C' | 'P'));
        let rest = sc.as_str();
        let strike_ok = !rest.is_empty()
            && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
            && rest.chars().filter(|c| *c == '.').count() <= 1;
        code.len() <= 24 && under_ok && date_ok && right_ok && strike_ok
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommodityCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for CommodityCode {
    type Err = CoreError;
    fn from_str(s: &str) -> CoreResult<Self> {
        Self::new(s)
    }
}

impl TryFrom<String> for CommodityCode {
    type Error = CoreError;
    fn try_from(s: String) -> CoreResult<Self> {
        Self::new(&s)
    }
}

impl From<CommodityCode> for String {
    fn from(c: CommodityCode) -> Self {
        c.0
    }
}

impl JsonSchema for CommodityCode {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "CommodityCode".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": r"^([A-Z0-9._-]{1,16}|[A-Z0-9.-]{1,12} [0-9]{6} [CP][0-9.]{1,10})$",
            "description": "Currency (ISO 4217, e.g. USD) or security symbol (e.g. VTI)"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_currencies_and_symbols() {
        for code in [
            "USD",
            "EUR",
            "VTI",
            "BRK.B",
            "X_1",
            "AAPL 261218 C200",
            "SPY 270115 P452.5",
        ] {
            assert!(CommodityCode::new(code).is_ok(), "{code}");
        }
    }

    #[test]
    fn rejects_bad_codes() {
        for code in [
            "",
            "usd",
            "US D",
            "TOOLONGCOMMODITYCODE",
            "€",
            "AAPL 2612 C200",
            "AAPL 261218 X200",
            "AAPL  261218 C200",
            "VERYLONGUNDERLYING 261218 C2000",
        ] {
            assert!(CommodityCode::new(code).is_err(), "{code}");
        }
    }

    #[test]
    fn serde_round_trip_validates() {
        let c = CommodityCode::new("USD").unwrap();
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, "\"USD\"");
        assert_eq!(serde_json::from_str::<CommodityCode>(&json).unwrap(), c);
        assert!(serde_json::from_str::<CommodityCode>("\"usd\"").is_err());
    }
}
