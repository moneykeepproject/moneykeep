use thiserror::Error;

/// Errors raised by core value types.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoreError {
    #[error("invalid commodity code {0:?}: expected 1-16 characters A-Z, 0-9, '.', '-' or '_'")]
    InvalidCommodity(String),

    #[error("commodity mismatch: {left} vs {right}")]
    CommodityMismatch { left: String, right: String },

    #[error("arithmetic overflow")]
    Overflow,

    #[error("invalid amount {0:?}")]
    InvalidAmount(String),

    #[error("invalid id {0:?}")]
    InvalidId(String),

    #[error("invalid date {0:?}: expected YYYY-MM-DD")]
    InvalidDate(String),

    #[error("invalid {what} {value:?}")]
    InvalidValue { what: &'static str, value: String },
}

pub type CoreResult<T> = Result<T, CoreError>;
