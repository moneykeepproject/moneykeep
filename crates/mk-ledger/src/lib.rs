//! MoneyKeep ledger rules.
//!
//! Pure validation and arithmetic for accounts, transactions and balances. No IO: the
//! command layer looks facts up in storage and passes them in.

mod account;
mod balance;
mod error;
pub mod lots;
pub mod schedule;
mod txn;

pub use account::{
    MAX_CODE_LEN, MAX_DESCRIPTION_LEN, MAX_NAME_LEN, check_code, check_description, check_subtype,
    normalise_name, would_create_cycle,
};
pub use balance::{Balances, add_to};
pub use error::{LedgerError, LedgerResult};
pub use txn::{
    AccountFacts, MAX_MEMO_LEN, MAX_NUMBER_LEN, MAX_PAYEE_LEN, MAX_SPLITS, MIN_SPLITS, TxnRules,
    canonical_amount, check_archived_untouched, check_closing, check_reconciled_preserved,
    check_transaction, inherit_reconcile, only_reconcile_differs, sort_splits,
};
