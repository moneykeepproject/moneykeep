//! MoneyKeep core types.
//!
//! This crate is pure data: it performs no IO and depends on no other workspace crate.
//! The layering check in `xtask` enforces that.

pub mod brand;
mod commodity;
pub mod currency;
pub mod dashboard;
mod date;
pub mod datefmt;
mod error;
mod id;
pub mod ledger;
pub mod legacy;
pub mod mask;
mod money;
pub mod schedule;
pub mod schema;
pub mod subscription;
pub mod text;

pub use commodity::CommodityCode;
pub use date::Date;
pub use error::{CoreError, CoreResult};
pub use id::{AccountId, BookId, Id, SplitId, TransactionId};
pub use ledger::{
    Account, AccountSubtype, AccountType, Basis, Reconcile, Split, SplitTags, Transaction,
    TxnStatus,
};
pub use money::Money;

/// Re-export so dependants use the same decimal type.
pub use rust_decimal::{Decimal, MathematicalOps, RoundingStrategy};
