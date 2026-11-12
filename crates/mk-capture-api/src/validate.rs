//! Field validation (mobile-capture.md: lengths, amount > 0 with at most two
//! decimals, date within ±400 days of today).

use serde::{Deserialize, Serialize};

use crate::{
    DATE_WINDOW_DAYS, ExpenseFields, Lists, MAX_AMOUNT_DECIMALS, MAX_AMOUNT_DIGITS,
    MAX_DEVICE_NAME, MAX_LIST_ITEMS, MAX_MERCHANT, MAX_NAME, MAX_PURPOSE, MAX_RECENT_MERCHANTS,
    MAX_TAG, TOKEN_CHARS, Tags, clean_text, msg, parse_date,
};

/// One refused field with its message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

fn fe(field: &str, message: impl Into<String>) -> FieldError {
    FieldError {
        field: field.to_owned(),
        message: message.into(),
    }
}

/// Validated, cleaned fields of a submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidExpense {
    pub fields: ExpenseFields,
    pub receipt_ids: Vec<String>,
}

/// Check a decimal amount: digits with an optional `.` and at most two decimals,
/// more than zero, at most 15 integer digits. Returns it without leading zeros.
pub fn parse_amount(s: &str) -> Result<String, String> {
    let t = s.trim();
    let (int, frac) = t.split_once('.').unwrap_or((t, ""));
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if t.is_empty()
        || !digits(int)
        || !digits(frac)
        || (int.is_empty() && frac.is_empty())
        || (t.contains('.') && frac.is_empty())
    {
        return Err("The amount must be a number such as 12.50.".into());
    }
    if frac.len() > MAX_AMOUNT_DECIMALS {
        return Err(format!(
            "The amount can have at most {MAX_AMOUNT_DECIMALS} decimal places."
        ));
    }
    let int = int.trim_start_matches('0');
    if int.len() > MAX_AMOUNT_DIGITS {
        return Err("The amount is too large.".into());
    }
    if int.is_empty() && frac.bytes().all(|b| b == b'0') {
        return Err("The amount must be more than zero.".into());
    }
    let int = if int.is_empty() { "0" } else { int };
    Ok(if frac.is_empty() {
        int.to_owned()
    } else {
        format!("{int}.{frac}")
    })
}

/// A UUID in its 8-4-4-4-12 hexadecimal form.
pub fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// A receipt or submission id issued by the server (32 lower-case hex digits).
pub fn is_receipt_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// A bearer token's shape: 43 Base64URL characters (32 bytes, no padding).
pub fn is_token_shape(s: &str) -> bool {
    s.len() == TOKEN_CHARS
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn opt_text(
    v: &Option<String>,
    max: usize,
    field: &str,
    what: &str,
    errors: &mut Vec<FieldError>,
) -> Option<String> {
    match v.as_deref().map(|s| clean_text(s, max, what)) {
        None | Some(Ok(None)) => None,
        Some(Ok(Some(s))) => Some(s),
        Some(Err(m)) => {
            errors.push(fe(field, m));
            None
        }
    }
}

/// The tags of an expense or a trip (mileage.md revision 2), cleaned.
pub(crate) fn clean_tags(t: &Tags, errors: &mut Vec<FieldError>) -> Tags {
    Tags {
        class: opt_text(&t.class, MAX_TAG, "class", "Class", errors),
        location: opt_text(&t.location, MAX_TAG, "location", "Location", errors),
        fund: opt_text(&t.fund, MAX_TAG, "fund", "Fund", errors),
        project: opt_text(&t.project, MAX_TAG, "project", "Project", errors),
    }
}

/// Validate and clean a submission's fields and receipt list against `today` (a day
/// number). A submission needs a date when it has expense fields, and at least one
/// receipt when it has none.
pub fn validate_expense(
    fields: &ExpenseFields,
    receipt_ids: &[String],
    today: i64,
    max_receipts: usize,
) -> Result<ValidExpense, Vec<FieldError>> {
    let mut errors = Vec::new();
    let merchant = opt_text(
        &fields.merchant,
        MAX_MERCHANT,
        "merchant",
        "Merchant",
        &mut errors,
    );
    let purpose = opt_text(
        &fields.purpose,
        MAX_PURPOSE,
        "purpose",
        "Purpose",
        &mut errors,
    );
    let category = opt_text(
        &fields.category,
        MAX_NAME,
        "category",
        "Category",
        &mut errors,
    );
    let paid_with = opt_text(
        &fields.paid_with,
        MAX_NAME,
        "paid_with",
        "Paid with",
        &mut errors,
    );
    let tags = clean_tags(&fields.tags, &mut errors);
    let amount = match fields.amount.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(a) => match parse_amount(a) {
            Ok(v) => Some(v),
            Err(m) => {
                errors.push(fe("amount", m));
                None
            }
        },
    };
    let date = match fields.date.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(d) => match parse_date(d) {
            Some(n) if (n - today).abs() <= DATE_WINDOW_DAYS => Some(d.to_owned()),
            Some(_) => {
                errors.push(fe(
                    "date",
                    format!("The date must be within {DATE_WINDOW_DAYS} days of today."),
                ));
                None
            }
            None => {
                errors.push(fe("date", "The date must be a date such as 2026-10-23."));
                None
            }
        },
    };
    let clean = ExpenseFields {
        date,
        merchant,
        amount,
        category,
        paid_with,
        purpose,
        tags,
    };
    if receipt_ids.len() > max_receipts {
        errors.push(fe(
            "receipt_ids",
            format!("An expense can have at most {max_receipts} receipts."),
        ));
    }
    let mut ids: Vec<String> = Vec::new();
    for r in receipt_ids {
        if !crate::is_receipt_id(r) {
            errors.push(fe("receipt_ids", "A receipt id is not valid."));
            break;
        }
        if !ids.contains(r) {
            ids.push(r.clone());
        }
    }
    if errors.is_empty() {
        if clean.has_expense_data() && clean.date.is_none() {
            errors.push(fe("date", "Enter the date."));
        } else if !clean.has_expense_data() && ids.is_empty() {
            errors.push(fe("merchant", msg::ENTER_SOMETHING));
        }
    }
    if errors.is_empty() {
        Ok(ValidExpense {
            fields: clean,
            receipt_ids: ids,
        })
    } else {
        Err(errors)
    }
}

/// A phone's name, cleaned (`None` falls back to "Phone").
pub fn validate_device_name(name: &str) -> Result<String, String> {
    Ok(clean_text(name, MAX_DEVICE_NAME, "The phone's name")?.unwrap_or_else(|| "Phone".into()))
}

/// Check the lists document's sizes and clean its names.
pub fn validate_lists(l: &Lists) -> Result<Lists, String> {
    let list = |v: &[String], max_items: usize, max_len: usize, what: &str| {
        if v.len() > max_items {
            return Err(format!("{what}: at most {max_items} entries."));
        }
        let mut out = Vec::with_capacity(v.len());
        for s in v {
            if let Some(c) = clean_text(s, max_len, what)?
                && !out.contains(&c)
            {
                out.push(c);
            }
        }
        Ok(out)
    };
    let mileage = crate::mileage::clean_lists(l)?;
    Ok(Lists {
        book_id: clean_text(&l.book_id, 100, "Book id")?.unwrap_or_default(),
        book_name: clean_text(&l.book_name, MAX_NAME, "Book name")?.unwrap_or_default(),
        base_currency: clean_text(&l.base_currency, 10, "Currency")?.unwrap_or_default(),
        categories: list(&l.categories, MAX_LIST_ITEMS, MAX_NAME, "Categories")?,
        paid_with: list(&l.paid_with, MAX_LIST_ITEMS, MAX_NAME, "Paid with")?,
        merchants: list(
            &l.merchants,
            MAX_RECENT_MERCHANTS,
            MAX_MERCHANT,
            "Merchants",
        )?,
        tags: crate::TagLists {
            class: list(&l.tags.class, MAX_LIST_ITEMS, MAX_TAG, "Classes")?,
            location: list(&l.tags.location, MAX_LIST_ITEMS, MAX_TAG, "Locations")?,
            fund: list(&l.tags.fund, MAX_LIST_ITEMS, MAX_TAG, "Funds")?,
            project: list(&l.tags.project, MAX_LIST_ITEMS, MAX_TAG, "Projects")?,
        },
        published_at: None,
        ..mileage
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::days_from_civil;

    fn today() -> i64 {
        days_from_civil(2026, 10, 23)
    }

    fn fields(amount: &str, date: &str, purpose: &str) -> ExpenseFields {
        ExpenseFields {
            date: Some(date.into()),
            merchant: Some("Office Depot".into()),
            amount: Some(amount.into()),
            purpose: Some(purpose.into()),
            ..ExpenseFields::default()
        }
    }

    fn messages(f: &ExpenseFields) -> Vec<String> {
        validate_expense(f, &[], today(), 10)
            .unwrap_err()
            .into_iter()
            .map(|e| e.message)
            .collect()
    }

    /// mobile-capture.md AT7 (validation layer; the server test checks the answers).
    #[test]
    fn refused_fields_have_messages() {
        assert_eq!(
            messages(&fields("0", "2026-10-23", "x")),
            ["The amount must be more than zero."]
        );
        assert_eq!(
            messages(&fields("1.005", "2026-10-23", "x")),
            ["The amount can have at most 2 decimal places."]
        );
        assert_eq!(
            messages(&fields("1", "2026-10-23", &"p".repeat(501))),
            ["Purpose can be at most 500 characters."]
        );
        assert_eq!(
            messages(&fields("1", "2028-10-23", "x")),
            ["The date must be within 400 days of today."]
        );
        let ok =
            validate_expense(&fields("045.20", "2026-10-21", " Lunch "), &[], today(), 10).unwrap();
        assert_eq!(ok.fields.amount.as_deref(), Some("45.20"));
        assert_eq!(ok.fields.purpose.as_deref(), Some("Lunch"));
    }

    #[test]
    fn amounts() {
        assert_eq!(parse_amount("12.5"), Ok("12.5".into()));
        assert_eq!(parse_amount("0.01"), Ok("0.01".into()));
        assert!(parse_amount("-1").is_err());
        assert!(parse_amount("1e3").is_err());
        assert!(parse_amount("1.").is_err());
        assert!(parse_amount(".").is_err());
        assert!(parse_amount("0.00").is_err());
        assert!(parse_amount(&"9".repeat(16)).is_err());
        assert!(parse_amount(&"9".repeat(15)).is_ok());
    }

    #[test]
    fn receipt_only_and_empty() {
        let id = "0123456789abcdef0123456789abcdef".to_owned();
        let v = validate_expense(
            &ExpenseFields::default(),
            std::slice::from_ref(&id),
            today(),
            10,
        )
        .unwrap();
        assert_eq!(v.receipt_ids, vec![id]);
        assert_eq!(
            messages(&ExpenseFields::default()),
            [msg::ENTER_SOMETHING.to_owned()]
        );
        let no_date = ExpenseFields {
            merchant: Some("A".into()),
            ..ExpenseFields::default()
        };
        assert_eq!(messages(&no_date), ["Enter the date."]);
        let many: Vec<String> = (0..11).map(|i| format!("{i:032x}")).collect();
        assert!(validate_expense(&fields("1", "2026-10-23", "x"), &many, today(), 10).is_err());
    }

    #[test]
    fn shapes() {
        assert!(is_uuid("01234567-89ab-cdef-0123-456789abcdef"));
        assert!(!is_uuid("01234567-89ab-cdef-0123-456789abcdeg"));
        assert!(is_token_shape(&"a".repeat(43)));
        assert!(!is_token_shape(&"a=".repeat(22)));
        assert_eq!(validate_device_name("  ").unwrap(), "Phone");
    }
}
