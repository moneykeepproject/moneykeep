use thiserror::Error;

/// A file could not be parsed. Messages name the line where possible.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ImportError {
    #[error("{0}")]
    Invalid(String),

    #[error("line {line}: {message}")]
    Line { line: usize, message: String },
}

impl ImportError {
    pub fn at(line: usize, message: impl Into<String>) -> Self {
        Self::Line {
            line,
            message: message.into(),
        }
    }
}

pub type ImportResult<T> = Result<T, ImportError>;
