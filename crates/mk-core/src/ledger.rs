//! Ledger data types: accounts, transactions, splits. Pure data, no IO.

use std::fmt;
use std::str::FromStr;

use rust_decimal::Decimal;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AccountId, CommodityCode, CoreError, CoreResult, Date, TransactionId};

macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = CoreError;
            fn from_str(s: &str) -> CoreResult<Self> {
                match s {
                    $($text => Ok($name::$variant),)+
                    _ => Err(CoreError::InvalidValue {
                        what: stringify!($name),
                        value: s.to_owned(),
                    }),
                }
            }
        }
    };
}

string_enum!(
    /// The five account types of double-entry bookkeeping.
    AccountType {
        Asset => "asset",
        Liability => "liability",
        Equity => "equity",
        Income => "income",
        Expense => "expense",
    }
);

impl AccountType {
    /// Whether the type's normal balance is a debit (asset, expense).
    pub fn is_debit_normal(self) -> bool {
        matches!(self, AccountType::Asset | AccountType::Expense)
    }

    /// Convert a stored (debit-positive) amount to its display form.
    pub fn display(self, raw: Decimal) -> Decimal {
        if self.is_debit_normal() { raw } else { -raw }
    }

    /// Subtype used when none is given.
    pub fn default_subtype(self) -> AccountSubtype {
        match self {
            AccountType::Asset => AccountSubtype::OtherAsset,
            AccountType::Liability => AccountSubtype::OtherLiability,
            AccountType::Equity => AccountSubtype::OwnerEquity,
            AccountType::Income => AccountSubtype::Income,
            AccountType::Expense => AccountSubtype::Expense,
        }
    }
}

string_enum!(
    /// Finer classification of an account within its type.
    AccountSubtype {
        Bank => "bank",
        Cash => "cash",
        Brokerage => "brokerage",
        Security => "security",
        Receivable => "receivable",
        FixedAsset => "fixed-asset",
        Clearing => "clearing",
        OtherAsset => "other-asset",
        CreditCard => "credit-card",
        Loan => "loan",
        Payable => "payable",
        TaxPayable => "tax-payable",
        OtherLiability => "other-liability",
        OpeningBalances => "opening-balances",
        OwnerEquity => "owner-equity",
        RetainedEarnings => "retained-earnings",
        NetAssets => "net-assets",
        Income => "income",
        Expense => "expense",
        CostOfGoods => "cost-of-goods",
    }
);

impl AccountSubtype {
    /// The six money subtypes (ledger-accounts.md revision 3): bank, cash and other
    /// asset; credit card, loan and other liability. A money account may switch
    /// between them, and so between asset and liability, even after it has splits.
    pub const MONEY: &'static [AccountSubtype] = &[
        AccountSubtype::Bank,
        AccountSubtype::Cash,
        AccountSubtype::OtherAsset,
        AccountSubtype::CreditCard,
        AccountSubtype::Loan,
        AccountSubtype::OtherLiability,
    ];

    /// Whether this is one of the six money subtypes.
    pub fn is_money(self) -> bool {
        Self::MONEY.contains(&self)
    }

    /// The account type this subtype belongs to.
    pub fn account_type(self) -> AccountType {
        use AccountSubtype as S;
        match self {
            S::Bank
            | S::Cash
            | S::Brokerage
            | S::Security
            | S::Receivable
            | S::FixedAsset
            | S::Clearing
            | S::OtherAsset => AccountType::Asset,
            S::CreditCard | S::Loan | S::Payable | S::TaxPayable | S::OtherLiability => {
                AccountType::Liability
            }
            S::OpeningBalances | S::OwnerEquity | S::RetainedEarnings | S::NetAssets => {
                AccountType::Equity
            }
            S::Income => AccountType::Income,
            S::Expense | S::CostOfGoods => AccountType::Expense,
        }
    }
}

string_enum!(
    /// Reconcile state of a split: new, cleared or reconciled.
    Reconcile {
        New => "n",
        Cleared => "c",
        Reconciled => "r",
    }
);

// Generated by `string_enum!`, which cannot carry `#[default]` on a variant.
#[allow(clippy::derivable_impls)]
impl Default for Reconcile {
    fn default() -> Self {
        Reconcile::New
    }
}

string_enum!(
    /// Whether a transaction counts in balances.
    TxnStatus {
        Posted => "posted",
        Voided => "voided",
    }
);

string_enum!(
    /// Which splits a balance counts.
    Basis {
        All => "all",
        Cleared => "cleared",
        Reconciled => "reconciled",
    }
);

#[allow(clippy::derivable_impls)]
impl Default for Basis {
    fn default() -> Self {
        Basis::All
    }
}

impl Basis {
    pub fn includes(self, state: Reconcile) -> bool {
        match self {
            Basis::All => true,
            Basis::Cleared => matches!(state, Reconcile::Cleared | Reconcile::Reconciled),
            Basis::Reconciled => state == Reconcile::Reconciled,
        }
    }
}

/// An account in the chart of accounts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    pub name: String,
    #[serde(rename = "type")]
    pub account_type: AccountType,
    pub subtype: AccountSubtype,
    pub parent: Option<AccountId>,
    pub commodity: CommodityCode,
    pub code: Option<String>,
    pub description: Option<String>,
    pub placeholder: bool,
    pub archived: bool,
    pub created_at: String,
    /// Where the account's total goes on a tax return (chart-presets.md): a free-text
    /// hint such as "Sch C L18". Never on a placeholder.
    #[serde(default)]
    pub tax_line: Option<String>,
    /// The fixed job the account does for the program (chart-presets.md "Roles"),
    /// e.g. `receivable`. Set only by the program; at most one account per role.
    #[serde(default)]
    pub role: Option<String>,
    /// A retirement account (IRA, 401(k), Roth and similar; brokerage accounts only,
    /// opening-balances.md).
    #[serde(default, skip_serializing_if = "is_false")]
    pub retirement: bool,
    /// The account started from zero: it needs no opening balance
    /// (opening-balances.md "Setup checklist").
    #[serde(default, skip_serializing_if = "is_false")]
    pub opening_none: bool,
}

/// For `skip_serializing_if`: leave a false flag out of the JSON.
#[allow(clippy::trivially_copy_pass_by_ref)]
pub fn is_false(b: &bool) -> bool {
    !*b
}

/// One leg of a transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Split {
    pub account: AccountId,
    /// Signed amount in the transaction currency (debit positive).
    pub value: Decimal,
    /// Signed amount in the account's commodity (debit positive).
    pub quantity: Decimal,
    pub memo: Option<String>,
    pub reconcile: Reconcile,
    /// Import fingerprint of the downloaded row this split came from or was matched to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// Class, location, fund and project tags (business-dimensions.md, projects.md),
    /// as record ids.
    #[serde(default, skip_serializing_if = "SplitTags::is_empty")]
    pub tags: SplitTags,
}

/// The dimension tags of a split: ids of a class, a location, a fund and a project,
/// and whether a project cost is billable to the customer (projects.md).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SplitTags {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fund: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// A project cost to put on the customer's invoice (projects.md "Billable costs").
    #[serde(default, skip_serializing_if = "is_false")]
    pub billable: bool,
}

impl SplitTags {
    pub fn is_empty(&self) -> bool {
        self.class.is_none()
            && self.location.is_none()
            && self.fund.is_none()
            && self.project.is_none()
            && !self.billable
    }
}

/// A balanced journal entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub id: TransactionId,
    pub date: Date,
    pub currency: CommodityCode,
    pub payee: Option<String>,
    pub number: Option<String>,
    pub memo: Option<String>,
    pub status: TxnStatus,
    pub void_reason: Option<String>,
    /// The transaction this one reverses, if it was created by `txn.reverse`.
    pub reverses: Option<TransactionId>,
    pub entered_at: String,
    pub splits: Vec<Split>,
    /// The account's opening entry (opening-balances.md): at most one per account.
    #[serde(default, skip_serializing_if = "is_false")]
    pub opening: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_subtype_round_trips_and_has_a_type() {
        for s in AccountSubtype::ALL {
            assert_eq!(s.as_str().parse::<AccountSubtype>().unwrap(), *s);
            let _ = s.account_type();
        }
        for t in AccountType::ALL {
            assert_eq!(t.default_subtype().account_type(), *t);
        }
    }

    #[test]
    fn display_negates_credit_normal_types() {
        let raw = Decimal::new(-20000, 2);
        assert_eq!(AccountType::Liability.display(raw), Decimal::new(20000, 2));
        assert_eq!(AccountType::Asset.display(raw), raw);
    }

    #[test]
    fn serde_names() {
        assert_eq!(
            serde_json::to_string(&AccountSubtype::CreditCard).unwrap(),
            "\"credit-card\""
        );
        assert_eq!(serde_json::to_string(&Reconcile::Cleared).unwrap(), "\"c\"");
    }

    #[test]
    fn basis_filters() {
        assert!(Basis::Cleared.includes(Reconcile::Reconciled));
        assert!(!Basis::Cleared.includes(Reconcile::New));
        assert!(!Basis::Reconciled.includes(Reconcile::Cleared));
    }
}
