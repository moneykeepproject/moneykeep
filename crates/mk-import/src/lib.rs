//! MoneyKeep import: pure parsing and classification helpers.
//!
//! Parsers turn file bytes into a format-neutral [`Statement`]. They take bytes, not
//! paths: reading files and watching folders is the job of the application layer.
//! Every parser implements [`Parser`], so further formats (OFX) plug in unchanged.

mod amount;
mod csv;
mod decode;
pub mod definition;
pub mod engrave;
mod error;
mod fingerprint;
mod iif;
pub mod image;
mod invprofile;
pub mod jpeg;
mod ofx;
pub mod ofx_dc;
mod ofx_inv;
pub mod processors;
mod qif;
pub mod receipt;
mod rules;
pub mod safety;
pub mod simplefin;
pub mod snaptrade;
mod statement;
mod xml;

pub use amount::{AmountStyle, parse_amount};
pub use csv::{
    AmountMode, ColumnRef, CsvParser, CsvProfile, DateFormat, NeedsProfile, SkipIf, detect_profile,
    parse_records, suggest_profile, suggest_profile_with,
};
pub use decode::{decode_cp1252, decode_latin1, decode_text};
pub use error::{ImportError, ImportResult};
pub use fingerprint::{content_fingerprint, external_fingerprint, normalise_payee, occurrences};
pub use iif::{IifParser, IifSplit, IifTxn, trntype_word, write_iif};
pub use invprofile::{
    ActionRule, InvProfile, MatchType, NEUTRAL_FIELDS, NumberFormat, PositionsSpec, ProfileFormat,
    SecurityRule, SourceRecord, Suggestion, account_from_file_name, builtin_profiles,
    detect_format, parse_money, parse_with_profile, profile_matches, suggest, validate_profile,
};
pub use ofx::OfxParser;
pub use qif::{DateOrder, QifOptions, QifParser};
pub use rules::{
    Action, Condition, ConditionField, ConditionOp, Rule, RuleKind, RuleOutcome, RuleRow,
    SplitShare, evaluate_rules, split_amount, validate_rule,
};
pub use statement::{
    ClearedFlag, ClosingBalance, DateKind, InvAction, InvRow, InvoiceApply, ListItem, Parser,
    Position, RawRow, RawSplit, Section, SectionKind, SourceSecurity, Statement,
};

/// Largest file accepted for import (50 MB).
pub const MAX_FILE_BYTES: usize = 50 * 1024 * 1024;
/// Most rows in one batch.
pub const MAX_ROWS: usize = 100_000;
