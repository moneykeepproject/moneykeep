//! Input safety for imported files (input-safety.md R2–R4): parser limits, strict
//! JSON, and the cleaning and validation applied to a parsed statement before it is
//! staged.

use std::fmt;

use mk_core::text::clean;
use mk_core::{Date, Decimal};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::statement::Statement;
use crate::{ImportError, ImportResult};

/// Longest field value a parser accepts (bytes), before any truncation rule.
pub const MAX_FIELD_BYTES: usize = 64 * 1024;
/// Longest number text (characters).
pub const MAX_NUMBER_CHARS: usize = 40;
/// Deepest JSON or XML nesting.
pub const MAX_DEPTH: usize = 64;
/// Most elements in one JSON array or one record list.
pub const MAX_ELEMENTS: usize = 100_000;

/// Field length limits (characters) after cleaning.
pub const PAYEE_CHARS: usize = 200;
pub const TEXT_CHARS: usize = 1000;

/// Compile a pattern with the input-safety limits: at most 200 characters, the
/// linear-time engine with a 1 MiB compiled-size limit and nesting limit 50.
pub fn regex(pattern: &str) -> ImportResult<regex::Regex> {
    if pattern.chars().count() > 200 {
        return Err(ImportError::Invalid(
            "a pattern is longer than 200 characters".into(),
        ));
    }
    regex::RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .nest_limit(50)
        .build()
        .map_err(|e| ImportError::Invalid(format!("invalid pattern: {e}")))
}

/// A field longer than [`MAX_FIELD_BYTES`] is an error.
pub(crate) fn check_field(value: &str, line: usize) -> ImportResult<()> {
    if value.len() > MAX_FIELD_BYTES {
        return Err(ImportError::Invalid(format!(
            "line {line}: a field is longer than 64 KiB"
        )));
    }
    Ok(())
}

/// A number text longer than [`MAX_NUMBER_CHARS`] is an error.
pub(crate) fn check_number_len(text: &str) -> ImportResult<()> {
    if text.trim().chars().count() > MAX_NUMBER_CHARS {
        return Err(ImportError::Invalid(format!(
            "a number is longer than {MAX_NUMBER_CHARS} characters"
        )));
    }
    Ok(())
}

// ----- strict JSON -------------------------------------------------------------------

/// Parse JSON with the input-safety rules: nesting at most [`MAX_DEPTH`], arrays of at
/// most [`MAX_ELEMENTS`], no duplicate keys, numbers inside the decimal range, strings
/// of at most [`MAX_FIELD_BYTES`].
pub fn parse_json(text: &str) -> ImportResult<Value> {
    let text = text.trim_start_matches('\u{feff}');
    let mut de = serde_json::Deserializer::from_str(text);
    // The depth limit below is lower than serde_json's own recursion limit.
    let v = Strict { depth: 0 }
        .deserialize(&mut de)
        .map_err(|e| ImportError::Invalid(format!("invalid JSON: {e}")))?;
    de.end()
        .map_err(|e| ImportError::Invalid(format!("invalid JSON: {e}")))?;
    Ok(v)
}

#[derive(Clone, Copy)]
struct Strict {
    depth: usize,
}

impl Strict {
    fn deeper<E: de::Error>(self) -> Result<Self, E> {
        if self.depth >= MAX_DEPTH {
            return Err(E::custom(format!("nesting deeper than {MAX_DEPTH}")));
        }
        Ok(Strict {
            depth: self.depth + 1,
        })
    }
}

impl<'de> DeserializeSeed<'de> for Strict {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(self)
    }
}

const DECIMAL_MAX: f64 = 7.9e28;

impl<'de> Visitor<'de> for Strict {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        if !v.is_finite() || v.abs() > DECIMAL_MAX {
            return Err(E::custom("a number is outside the decimal range"));
        }
        Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("a number is outside the decimal range"))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Value, E> {
        if v.len() > MAX_FIELD_BYTES {
            return Err(E::custom("a string is longer than 64 KiB"));
        }
        Ok(Value::String(v.to_owned()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Value, E> {
        if v.len() > MAX_FIELD_BYTES {
            return Err(E::custom("a string is longer than 64 KiB"));
        }
        Ok(Value::String(v))
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let inner = self.deeper()?;
        let mut out = Vec::new();
        while let Some(v) = seq.next_element_seed(inner)? {
            if out.len() >= MAX_ELEMENTS {
                return Err(de::Error::custom(format!(
                    "an array has more than {MAX_ELEMENTS} elements"
                )));
            }
            out.push(v);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let inner = self.deeper()?;
        let mut out = Map::new();
        while let Some(k) = map.next_key::<String>()? {
            if out.contains_key(&k) {
                return Err(de::Error::custom(format!("duplicate key {k:?}")));
            }
            let v = map.next_value_seed(inner)?;
            out.insert(k, v);
        }
        Ok(Value::Object(out))
    }
}

// ----- statement cleaning and validation ----------------------------------------------

fn date_ok(d: Date) -> bool {
    let (lo, hi) = (Date::from_ymd(1900, 1, 1), Date::from_ymd(2199, 12, 31));
    match (lo, hi) {
        (Ok(lo), Ok(hi)) => d >= lo && d <= hi,
        _ => true,
    }
}

/// At most 15 integer digits.
fn amount_ok(a: Decimal) -> bool {
    a.trunc().abs() < Decimal::from(1_000_000_000_000_000_i64)
}

/// Clean one field in place, recording notes.
fn fix(field: &mut String, name: &str, limit: usize, notes: &mut Vec<String>) {
    let c = clean(field, limit);
    if c.changed() {
        for n in c.notes(name, limit) {
            if !notes.contains(&n) {
                notes.push(n);
            }
        }
    }
    *field = c.text;
}

fn fix_opt(field: &mut Option<String>, name: &str, limit: usize, notes: &mut Vec<String>) {
    if let Some(f) = field {
        fix(f, name, limit, notes);
    }
}

/// Apply input-safety.md R3 (text cleaning) and R4 (value ranges) to a parsed
/// statement. Cleaned rows carry notes in `cleaned`; rows with out-of-range values
/// are dropped with a warning naming their line.
pub fn sanitize_statement(st: &mut Statement) {
    let mut dropped: Vec<String> = Vec::new();
    for sec in &mut st.sections {
        let mut ignore = Vec::new();
        fix_opt(
            &mut sec.account_name,
            "account name",
            TEXT_CHARS,
            &mut ignore,
        );
        fix_opt(&mut sec.account_hint, "account", TEXT_CHARS, &mut ignore);
        for s in &mut sec.securities {
            fix_opt(&mut s.name, "security name", TEXT_CHARS, &mut ignore);
            fix_opt(&mut s.ticker, "symbol", TEXT_CHARS, &mut ignore);
            fix_opt(&mut s.cusip, "CUSIP", TEXT_CHARS, &mut ignore);
            fix_opt(&mut s.kind, "kind", TEXT_CHARS, &mut ignore);
        }
        sec.rows.retain(|r| {
            let why = if !date_ok(r.date) {
                Some("its date is outside 1900–2199")
            } else if !amount_ok(r.amount) || r.splits.iter().any(|s| !amount_ok(s.amount)) {
                Some("its amount has more than 15 integer digits")
            } else {
                None
            };
            if let Some(w) = why {
                dropped.push(format!("line {} skipped: {w}", r.line));
            }
            why.is_none()
        });
        for r in &mut sec.rows {
            let mut n = std::mem::take(&mut r.cleaned);
            fix(&mut r.payee, "payee", PAYEE_CHARS, &mut n);
            fix(&mut r.memo, "memo", TEXT_CHARS, &mut n);
            fix(&mut r.number, "number", TEXT_CHARS, &mut n);
            fix(&mut r.category_hint, "category", TEXT_CHARS, &mut n);
            fix_opt(&mut r.external_id, "id", TEXT_CHARS, &mut n);
            fix_opt(&mut r.trntype, "type", TEXT_CHARS, &mut n);
            for s in &mut r.splits {
                fix(&mut s.category_hint, "category", TEXT_CHARS, &mut n);
                fix(&mut s.memo, "memo", TEXT_CHARS, &mut n);
            }
            r.cleaned = n;
        }
        sec.inv_rows.retain(|r| {
            let amounts = [r.quantity, r.price, r.amount, Some(r.commission), r.basis];
            let why = if !date_ok(r.date) {
                Some("its date is outside 1900–2199")
            } else if amounts.iter().flatten().any(|a| !amount_ok(*a)) {
                Some("a number has more than 15 integer digits")
            } else {
                None
            };
            if let Some(w) = why {
                dropped.push(format!("line {} skipped: {w}", r.line));
            }
            why.is_none()
        });
        for r in &mut sec.inv_rows {
            let mut n = std::mem::take(&mut r.cleaned);
            fix(&mut r.memo, "memo", TEXT_CHARS, &mut n);
            fix_opt(&mut r.external_id, "id", TEXT_CHARS, &mut n);
            fix_opt(&mut r.unsupported, "action", TEXT_CHARS, &mut n);
            for note in &mut r.notes {
                fix(note, "note", TEXT_CHARS, &mut Vec::new());
            }
            r.cleaned = n;
        }
    }
    for w in &mut st.warnings {
        *w = clean(w, TEXT_CHARS).text;
    }
    st.warnings.extend(dropped);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn strict_json_rules() {
        assert!(parse_json(r#"{"a":1,"a":2}"#).is_err());
        let deep = format!("{}{}", "[".repeat(65), "]".repeat(65));
        assert!(parse_json(&deep).is_err());
        let ok = format!("{}{}", "[".repeat(64), "]".repeat(64));
        assert!(parse_json(&ok).is_ok());
        assert!(parse_json(&format!("[{}]", "9".repeat(10_000))).is_err());
        assert_eq!(parse_json(r#"{"a":[1,"x"]}"#).unwrap()["a"][1], "x");
    }
}
