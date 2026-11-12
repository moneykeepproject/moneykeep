//! Password rules and texts (book-encryption.md).

/// "Lock after" choices in minutes (0 = never).
pub(crate) const LOCK_CHOICES: [(u32, &str); 5] = [
    (0, "Never"),
    (5, "5 minutes"),
    (15, "15 minutes"),
    (30, "30 minutes"),
    (60, "60 minutes"),
];

pub(crate) const PASSPHRASE_NOTE: &str = "A long passphrase of several words is best.";

/// Check a new password and its confirmation (front-end hint; the store enforces
/// the rule too).
pub(crate) fn new_password_problem(new: &str, confirm: &str) -> Option<String> {
    if new.trim().chars().count() < 8 {
        return Some(
            "Use at least 8 characters. A long passphrase of several words is best.".into(),
        );
    }
    if new != confirm {
        return Some("The two passwords are different.".into());
    }
    None
}
