//! Account field rules.

use std::collections::HashMap;

use mk_core::{AccountId, AccountSubtype, AccountType};

use crate::{LedgerError, LedgerResult};

pub const MAX_NAME_LEN: usize = 100;
pub const MAX_CODE_LEN: usize = 20;
pub const MAX_DESCRIPTION_LEN: usize = 1000;

/// Trim a name and check it: 1-100 characters, no `:` (the full-name separator).
pub fn normalise_name(raw: &str) -> LedgerResult<String> {
    let name = raw.trim();
    let len = name.chars().count();
    if len == 0 {
        return Err(LedgerError::invalid("account name must not be empty"));
    }
    if len > MAX_NAME_LEN {
        return Err(LedgerError::invalid(format!(
            "account name is {len} characters; the maximum is {MAX_NAME_LEN}"
        )));
    }
    if name.contains(':') {
        return Err(LedgerError::invalid(
            "account name must not contain ':' (it separates names in a full name)",
        ));
    }
    Ok(name.to_owned())
}

/// Account number: 1-20 characters after trimming.
pub fn check_code(raw: &str) -> LedgerResult<String> {
    let code = raw.trim();
    let len = code.chars().count();
    if len == 0 || len > MAX_CODE_LEN {
        return Err(LedgerError::invalid(format!(
            "account code must be 1-{MAX_CODE_LEN} characters"
        )));
    }
    Ok(code.to_owned())
}

pub fn check_description(text: &str) -> LedgerResult<()> {
    if text.chars().count() > MAX_DESCRIPTION_LEN {
        return Err(LedgerError::invalid(format!(
            "description is longer than {MAX_DESCRIPTION_LEN} characters"
        )));
    }
    Ok(())
}

/// The subtype must belong to the account type.
pub fn check_subtype(account_type: AccountType, subtype: AccountSubtype) -> LedgerResult<()> {
    if subtype.account_type() == account_type {
        Ok(())
    } else {
        Err(LedgerError::invalid(format!(
            "subtype {subtype} belongs to type {}, not {account_type}",
            subtype.account_type()
        )))
    }
}

/// Whether making `new_parent` the parent of `account` would create a cycle, given
/// the current parent links.
pub fn would_create_cycle(
    account: AccountId,
    new_parent: AccountId,
    parents: &HashMap<AccountId, Option<AccountId>>,
) -> bool {
    let mut cursor = Some(new_parent);
    let mut steps = 0usize;
    while let Some(id) = cursor {
        if id == account {
            return true;
        }
        steps += 1;
        if steps > parents.len() {
            // Corrupt data with an existing cycle: refuse rather than loop.
            return true;
        }
        cursor = parents.get(&id).copied().flatten();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_checked() {
        assert_eq!(normalise_name("  Food ").unwrap(), "Food");
        assert!(normalise_name("   ").is_err());
        assert!(normalise_name("Expenses:Food").is_err());
        assert!(normalise_name(&"x".repeat(101)).is_err());
        assert!(normalise_name(&"x".repeat(100)).is_ok());
    }

    #[test]
    fn codes_are_checked() {
        assert_eq!(check_code(" 1000 ").unwrap(), "1000");
        assert!(check_code("").is_err());
        assert!(check_code(&"1".repeat(21)).is_err());
    }

    #[test]
    fn subtype_must_match_type() {
        assert!(check_subtype(AccountType::Asset, AccountSubtype::Bank).is_ok());
        assert!(check_subtype(AccountType::Asset, AccountSubtype::CreditCard).is_err());
    }

    #[test]
    fn cycles_are_detected() {
        let a = AccountId::new();
        let b = AccountId::new();
        let c = AccountId::new();
        let parents = HashMap::from([(a, None), (b, Some(a)), (c, Some(b))]);
        assert!(would_create_cycle(a, c, &parents), "a under its grandchild");
        assert!(would_create_cycle(a, a, &parents), "a under itself");
        assert!(!would_create_cycle(c, a, &parents));
    }
}
