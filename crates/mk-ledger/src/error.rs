use thiserror::Error;

/// A ledger rule was broken.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LedgerError {
    /// The input is malformed whatever the state of the book (bad name, too many
    /// decimals, unbalanced entry).
    #[error("{0}")]
    Invalid(String),

    /// The input is well formed but the book's current state forbids it (closed period,
    /// reconciled split, archived or placeholder account).
    #[error("{0}")]
    Rejected(String),

    #[error("arithmetic overflow")]
    Overflow,
}

impl LedgerError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }

    pub fn rejected(msg: impl Into<String>) -> Self {
        Self::Rejected(msg.into())
    }
}

pub type LedgerResult<T> = Result<T, LedgerError>;
