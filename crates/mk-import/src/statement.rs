//! The format-neutral statement every parser produces.

use mk_core::{CommodityCode, Date, Decimal};
use serde::{Deserialize, Serialize};

use crate::ImportResult;

/// A source's cleared state for a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClearedFlag {
    Cleared,
    Reconciled,
}

/// One split given by the source (QIF `S`/`$`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawSplit {
    pub category_hint: String,
    pub amount: Decimal,
    pub memo: String,
    /// The source's class for this split (IIF `CLASS`).
    #[serde(default)]
    pub class_hint: Option<String>,
}

/// Which date a source gave for a row (import-pipeline.md revision 2).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DateKind {
    /// The source labelled it a transaction (user) date.
    Transaction,
    /// The source labelled it a posting date.
    Posted,
    /// A single, unlabelled date (QIF and the like).
    #[default]
    Unknown,
}

/// One transaction row, from the account holder's view (+ in, − out).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawRow {
    pub external_id: Option<String>,
    /// The transaction date when the source gives one, else the posting date.
    pub date: Date,
    /// What kind of date `date` is.
    #[serde(default)]
    pub date_kind: DateKind,
    pub amount: Decimal,
    pub payee: String,
    pub memo: String,
    pub number: String,
    pub category_hint: String,
    pub cleared: Option<ClearedFlag>,
    pub splits: Vec<RawSplit>,
    /// Source transaction type (OFX `TRNTYPE`), for rules.
    #[serde(default)]
    pub trntype: Option<String>,
    /// Why the source says this row needs review (e.g. a foreign-currency OFX row).
    #[serde(default)]
    pub review: Option<String>,
    /// OFX correction: the earlier FITID this row replaces or deletes.
    #[serde(default)]
    pub correct_fitid: Option<String>,
    /// OFX correction action: `REPLACE` or `DELETE`.
    #[serde(default)]
    pub correct_action: Option<String>,
    /// 1-based line of the record in the file.
    pub line: usize,
    /// Original record text.
    pub raw: String,
    /// What text cleaning changed (input-safety.md R3).
    #[serde(default)]
    pub cleaned: Vec<String>,
    /// The source's class for the row (IIF `CLASS` on the `TRNS` row).
    #[serde(default)]
    pub class_hint: Option<String>,
    /// A bank feed's pending transaction (connectivity.md): never committed without
    /// review, replaced by its posted version.
    #[serde(default)]
    pub pending: bool,
    /// Processor entries naming an open invoice: proposed payments of these amounts
    /// (connectivity-processors.md), applied only when the user accepts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invoices: Vec<InvoiceApply>,
}

/// A proposed invoice payment inside a processor row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceApply {
    /// The invoice number as found (e.g. `INV-1002`).
    pub number: String,
    pub amount: Decimal,
}

/// A closing balance carried by the source (OFX ledger balance, CSV balance column).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosingBalance {
    pub date: Date,
    pub amount: Decimal,
}

/// What kind of account a section describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    #[default]
    Bank,
    Investment,
}

/// A security named by the file (import-investments.md).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SourceSecurity {
    /// Source security key: `CUSIP:922908769`, `TICKER:VTI` or `NAME:…`.
    pub key: String,
    #[serde(default)]
    pub cusip: Option<String>,
    #[serde(default)]
    pub ticker: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// `stock`, `mutual_fund`, `bond`, `other` (or `option`, which is unsupported).
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub price: Option<Decimal>,
    #[serde(default)]
    pub price_date: Option<Date>,
    /// FIGI (SnapTrade), a further identifier for matching securities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub figi: Option<String>,
}

/// An investment action named by the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvAction {
    Buy,
    Sell,
    Dividend,
    Reinvest,
    Interest,
    Fee,
    SharesIn,
    SharesOut,
    Split,
    ReturnOfCapital,
    /// Corporate actions (investments-advanced.md): imported rows always need review.
    Merger,
    Spinoff,
    SymbolChange,
    CashInLieu,
    Unsupported,
}

/// One investment row (amounts are absolute; the action gives the direction).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvRow {
    pub external_id: Option<String>,
    pub date: Date,
    pub action: InvAction,
    /// Source security key (see [`SourceSecurity::key`]).
    #[serde(default)]
    pub security: Option<String>,
    #[serde(default)]
    pub quantity: Option<Decimal>,
    #[serde(default)]
    pub price: Option<Decimal>,
    /// All transaction costs combined.
    #[serde(default)]
    pub commission: Decimal,
    #[serde(default)]
    pub amount: Option<Decimal>,
    /// Dividend kind (`ordinary`, `qualified`, `capital_gain_long`, …).
    #[serde(default)]
    pub kind: Option<String>,
    /// Split ratio (new, old).
    #[serde(default)]
    pub ratio: Option<(Decimal, Decimal)>,
    /// Shares in: the lot's basis and acquired date.
    #[serde(default)]
    pub basis: Option<Decimal>,
    #[serde(default)]
    pub acquired: Option<Date>,
    /// A fee to a named expense account (OFX margin interest).
    #[serde(default)]
    pub expense_account: Option<String>,
    #[serde(default)]
    pub memo: String,
    /// The source id of the transaction this row reverses.
    #[serde(default)]
    pub reversal_of: Option<String>,
    /// Why the row cannot be imported (unsupported actions).
    #[serde(default)]
    pub unsupported: Option<String>,
    #[serde(default)]
    pub needs_review: bool,
    #[serde(default)]
    pub notes: Vec<String>,
    pub line: usize,
    pub raw: String,
    /// What text cleaning changed (input-safety.md R3).
    #[serde(default)]
    pub cleaned: Vec<String>,
}

/// A holding reported by the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub security: String,
    pub quantity: Decimal,
    #[serde(default)]
    pub price: Option<Decimal>,
    #[serde(default)]
    pub market_value: Option<Decimal>,
    #[serde(default)]
    pub as_of: Option<Date>,
}

/// One account's part of a file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Section {
    /// Source key for account mapping (e.g. `qif:Checking`); `None` if the file has no
    /// account identity.
    pub account_key: Option<String>,
    /// Human name of the source account, if any.
    pub account_name: Option<String>,
    /// Masked source account number for display (e.g. last four characters). Parsers
    /// never put a full account number here.
    pub account_hint: Option<String>,
    pub currency: Option<CommodityCode>,
    pub period: Option<(Date, Date)>,
    pub closing: Option<ClosingBalance>,
    /// Available balance, shown for information only (never reconciled against).
    #[serde(default)]
    pub available: Option<ClosingBalance>,
    /// Plain (cash) rows; in an investment section, cash movements of the account.
    pub rows: Vec<RawRow>,
    #[serde(default)]
    pub kind: SectionKind,
    /// Investment sections: securities, investment rows, positions and cash.
    #[serde(default)]
    pub securities: Vec<SourceSecurity>,
    #[serde(default)]
    pub inv_rows: Vec<InvRow>,
    #[serde(default)]
    pub positions: Vec<Position>,
    /// The provider's positions could not be read this time (connectivity-brokerage.md
    /// revision 2): `positions` is empty for that reason, not because nothing is
    /// held, so the holdings check leaves the securities out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub positions_unread: bool,
    #[serde(default)]
    pub cash_balance: Option<ClosingBalance>,
    #[serde(default)]
    pub as_of: Option<Date>,
}

/// A parsed file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Statement {
    pub sections: Vec<Section>,
    /// Non-fatal notes (e.g. "missing final ^", "skipped: investment records (2)").
    pub warnings: Vec<String>,
    /// List records offered for creation (IIF `ACCNT`, `CUST`, `VEND`, `CLASS`).
    #[serde(default)]
    pub lists: Vec<ListItem>,
}

/// A list record named by a file (names only, plus address lines).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ListItem {
    /// `account`, `customer`, `vendor` or `class`.
    pub kind: String,
    pub name: String,
    /// The source's account type text (IIF `ACCNTTYPE`), for accounts.
    #[serde(default)]
    pub account_type: Option<String>,
    #[serde(default)]
    pub address: Vec<String>,
}

impl Statement {
    pub fn row_count(&self) -> usize {
        self.sections
            .iter()
            .map(|s| s.rows.len() + s.inv_rows.len())
            .sum()
    }
}

/// A file-format parser. Implementations are pure: bytes in, statement out.
pub trait Parser {
    /// Short format name (`qif`, `csv`, later `ofx`).
    fn format(&self) -> &'static str;
    fn parse(&self, bytes: &[u8]) -> ImportResult<Statement>;
}
