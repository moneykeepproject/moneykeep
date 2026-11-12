//! Account, transaction and split persistence. Validation lives in `mk-ledger` and
//! `mk-commands`; this module only reads and writes rows.

use mk_core::{
    Account, AccountId, CommodityCode, Date, Decimal, Reconcile, Split, SplitId, SplitTags,
    Transaction, TransactionId, TxnStatus,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{StoreError, StoreResult, WriteTx};

const ACCOUNT_COLS: &str = "id, name, type, subtype, parent_id, commodity, code, description, \
                            placeholder, archived, created_at, tax_line, role, retirement, opening_none";

/// Filter for [`WriteTx::transactions`].
#[derive(Debug, Clone, Default)]
pub struct TxnFilter {
    /// Only transactions with a split in one of these accounts (`None`: all).
    pub accounts: Option<Vec<AccountId>>,
    pub from: Option<Date>,
    pub to: Option<Date>,
    pub include_voided: bool,
    pub limit: Option<u32>,
    pub offset: u32,
}

/// One split with the transaction facts balances need.
#[derive(Debug, Clone)]
pub struct SplitRow {
    pub txn_id: TransactionId,
    pub account: AccountId,
    pub date: Date,
    pub status: TxnStatus,
    pub currency: CommodityCode,
    pub value: Decimal,
    pub quantity: Decimal,
    pub reconcile: Reconcile,
    pub tags: SplitTags,
}

fn corrupt(what: &str, e: impl std::fmt::Display) -> StoreError {
    StoreError::Corrupt(format!("{what}: {e}"))
}

fn parse<T: std::str::FromStr>(what: &str, s: &str) -> StoreResult<T>
where
    T::Err: std::fmt::Display,
{
    s.parse::<T>().map_err(|e| corrupt(what, e))
}

type RawAccount = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    bool,
    bool,
    String,
    Option<String>,
    Option<String>,
    bool,
    bool,
);

fn raw_account(r: &Row<'_>) -> rusqlite::Result<RawAccount> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        r.get(11)?,
        r.get(12)?,
        r.get(13)?,
        r.get(14)?,
    ))
}

fn to_account(raw: RawAccount) -> StoreResult<Account> {
    let (
        id,
        name,
        ty,
        sub,
        parent,
        commodity,
        code,
        description,
        placeholder,
        archived,
        created,
        tax_line,
        role,
        retirement,
        opening_none,
    ) = raw;
    Ok(Account {
        id: parse("account id", &id)?,
        name,
        account_type: parse("account type", &ty)?,
        subtype: parse("account subtype", &sub)?,
        parent: parent.map(|p| parse("parent id", &p)).transpose()?,
        commodity: parse("commodity", &commodity)?,
        code,
        description,
        placeholder,
        archived,
        created_at: created,
        tax_line,
        role,
        retirement,
        opening_none,
    })
}

pub(crate) fn read_accounts(conn: &Connection) -> StoreResult<Vec<Account>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ACCOUNT_COLS} FROM account ORDER BY name COLLATE NOCASE, id"
    ))?;
    let raws = stmt
        .query_map([], raw_account)?
        .collect::<Result<Vec<_>, _>>()?;
    raws.into_iter().map(to_account).collect()
}

fn read_splits(conn: &Connection, txn_id: &str) -> StoreResult<Vec<Split>> {
    let mut stmt = conn.prepare(
        "SELECT account_id, value, quantity, memo, reconcile, import_fingerprint,
                class_id, location_id, fund_id, project_id, billable FROM split
         WHERE txn_id = ?1 ORDER BY idx",
    )?;
    let raws = stmt
        .query_map(params![txn_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                SplitTags {
                    class: r.get::<_, Option<String>>(6)?,
                    location: r.get::<_, Option<String>>(7)?,
                    fund: r.get::<_, Option<String>>(8)?,
                    project: r.get::<_, Option<String>>(9)?,
                    billable: r.get::<_, bool>(10)?,
                },
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    raws.into_iter()
        .map(
            |(account, value, quantity, memo, reconcile, fingerprint, tags)| {
                Ok(Split {
                    account: parse("split account", &account)?,
                    value: parse("split value", &value)?,
                    quantity: parse("split quantity", &quantity)?,
                    memo,
                    reconcile: parse("reconcile", &reconcile)?,
                    fingerprint,
                    tags,
                })
            },
        )
        .collect()
}

type RawTxn = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    String,
    bool,
);

const TXN_COLS: &str = "t.id, t.date, t.currency, t.payee, t.number, t.memo, t.status, t.void_reason, t.reverses, \
     t.entered_at, t.opening";

fn raw_txn(r: &Row<'_>) -> rusqlite::Result<RawTxn> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
    ))
}

fn to_txn(conn: &Connection, raw: RawTxn) -> StoreResult<Transaction> {
    let (
        id,
        date,
        currency,
        payee,
        number,
        memo,
        status,
        void_reason,
        reverses,
        entered_at,
        opening,
    ) = raw;
    let splits = read_splits(conn, &id)?;
    Ok(Transaction {
        id: parse("transaction id", &id)?,
        date: parse("date", &date)?,
        currency: parse("currency", &currency)?,
        payee,
        number,
        memo,
        status: parse("status", &status)?,
        void_reason,
        reverses: reverses.map(|r| parse("reverses", &r)).transpose()?,
        entered_at,
        splits,
        opening,
    })
}

impl WriteTx<'_> {
    // ----- accounts ---------------------------------------------------------

    /// Every account, sorted by name (case-insensitive).
    pub fn accounts(&self) -> StoreResult<Vec<Account>> {
        read_accounts(self.tx)
    }

    pub fn account(&self, id: AccountId) -> StoreResult<Option<Account>> {
        let raw = self
            .tx
            .query_row(
                &format!("SELECT {ACCOUNT_COLS} FROM account WHERE id = ?1"),
                params![id.to_string()],
                raw_account,
            )
            .optional()?;
        raw.map(to_account).transpose()
    }

    /// Insert or fully replace an account row.
    pub fn put_account(&self, a: &Account) -> StoreResult<()> {
        self.tx.execute(
            &format!(
                "INSERT INTO account ({ACCOUNT_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                 ON CONFLICT (id) DO UPDATE SET
                   name = excluded.name, type = excluded.type, subtype = excluded.subtype,
                   parent_id = excluded.parent_id, commodity = excluded.commodity,
                   code = excluded.code, description = excluded.description,
                   placeholder = excluded.placeholder, archived = excluded.archived,
                   created_at = excluded.created_at, tax_line = excluded.tax_line,
                   role = excluded.role, retirement = excluded.retirement,
                   opening_none = excluded.opening_none"
            ),
            params![
                a.id.to_string(),
                a.name,
                a.account_type.as_str(),
                a.subtype.as_str(),
                a.parent.map(|p| p.to_string()),
                a.commodity.as_str(),
                a.code,
                a.description,
                a.placeholder,
                a.archived,
                a.created_at,
                a.tax_line,
                a.role,
                a.retirement,
                a.opening_none,
            ],
        )?;
        Ok(())
    }

    pub fn delete_account(&self, id: AccountId) -> StoreResult<()> {
        self.tx.execute(
            "DELETE FROM account_usage WHERE account_id = ?1",
            params![id.to_string()],
        )?;
        self.tx
            .execute("DELETE FROM account WHERE id = ?1", params![id.to_string()])?;
        Ok(())
    }

    /// Whether the account holds any split now.
    pub fn account_has_live_splits(&self, id: AccountId) -> StoreResult<bool> {
        Ok(self.tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM split WHERE account_id = ?1)",
            params![id.to_string()],
            |r| r.get(0),
        )?)
    }

    /// Whether the account has ever held a split (now, or one later edited away or
    /// deleted).
    pub fn account_ever_used(&self, id: AccountId) -> StoreResult<bool> {
        Ok(self.tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM split WHERE account_id = ?1)
                 OR EXISTS (SELECT 1 FROM account_usage WHERE account_id = ?1)",
            params![id.to_string()],
            |r| r.get(0),
        )?)
    }

    /// Remember that `account` held a split of `txn` that is being removed. The row is
    /// stamped with the command's audit sequence by [`WriteTx::stamp_usage`].
    pub fn record_usage(&self, account: AccountId, txn: TransactionId) -> StoreResult<()> {
        self.tx.execute(
            "INSERT INTO account_usage (account_id, txn_id, seq) VALUES (?1, ?2, NULL)",
            params![account.to_string(), txn.to_string()],
        )?;
        Ok(())
    }

    /// Attach unstamped usage rows to the audit entry `seq` written for this command.
    pub fn stamp_usage(&self, seq: i64) -> StoreResult<()> {
        self.tx.execute(
            "UPDATE account_usage SET seq = ?1 WHERE seq IS NULL",
            params![seq],
        )?;
        Ok(())
    }

    /// Forget usage rows written by audit entry `seq` (its undo restores the splits
    /// or the state before them).
    pub fn forget_usage(&self, seq: i64) -> StoreResult<()> {
        self.tx
            .execute("DELETE FROM account_usage WHERE seq = ?1", params![seq])?;
        Ok(())
    }

    // ----- transactions -----------------------------------------------------

    pub fn transaction(&self, id: TransactionId) -> StoreResult<Option<Transaction>> {
        let raw = self
            .tx
            .query_row(
                &format!("SELECT {TXN_COLS} FROM txn t WHERE t.id = ?1"),
                params![id.to_string()],
                raw_txn,
            )
            .optional()?;
        raw.map(|r| to_txn(self.tx, r)).transpose()
    }

    /// Insert or fully replace a transaction and its splits.
    pub fn put_transaction(&self, t: &Transaction) -> StoreResult<()> {
        let id = t.id.to_string();
        self.tx.execute(
            "INSERT INTO txn (id, date, currency, payee, number, memo, status, void_reason,
                              reverses, entered_at, opening)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (id) DO UPDATE SET
               date = excluded.date, currency = excluded.currency, payee = excluded.payee,
               number = excluded.number, memo = excluded.memo, status = excluded.status,
               void_reason = excluded.void_reason, reverses = excluded.reverses,
               entered_at = excluded.entered_at, opening = excluded.opening",
            params![
                id,
                t.date.to_string(),
                t.currency.as_str(),
                t.payee,
                t.number,
                t.memo,
                t.status.as_str(),
                t.void_reason,
                t.reverses.map(|r| r.to_string()),
                t.entered_at,
                t.opening,
            ],
        )?;
        // release-prep.md §4: the splits keep their ids (from the current rows, or
        // from a delete being undone).
        let mut old = split_keys(self.tx, "split", &id)?;
        if old.is_empty() {
            old = split_keys(self.tx, "split_gone", &id)?;
        }
        let ids = keep_split_ids(&old, &t.splits);
        self.tx
            .execute("DELETE FROM split WHERE txn_id = ?1", params![id])?;
        self.tx
            .execute("DELETE FROM split_gone WHERE txn_id = ?1", params![id])?;
        let mut stmt = self.tx.prepare(
            "INSERT INTO split (txn_id, idx, account_id, value, quantity, memo, reconcile,
                                import_fingerprint, class_id, location_id, fund_id,
                                project_id, billable, split_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        )?;
        for ((idx, s), split_id) in t.splits.iter().enumerate().zip(ids) {
            stmt.execute(params![
                id,
                i64::try_from(idx).map_err(|e| corrupt("split index", e))?,
                s.account.to_string(),
                s.value.to_string(),
                s.quantity.to_string(),
                s.memo,
                s.reconcile.as_str(),
                s.fingerprint,
                s.tags.class,
                s.tags.location,
                s.tags.fund,
                s.tags.project,
                s.tags.billable,
                split_id,
            ])?;
        }
        Ok(())
    }

    /// The splits' own ids of a transaction, in line order (release-prep.md §4).
    pub fn split_ids(&self, txn: TransactionId) -> StoreResult<Vec<String>> {
        let mut stmt = self
            .tx
            .prepare("SELECT split_id FROM split WHERE txn_id = ?1 ORDER BY idx")?;
        let ids = stmt
            .query_map(params![txn.to_string()], |r| r.get::<_, Option<String>>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids.into_iter().map(Option::unwrap_or_default).collect())
    }

    pub fn delete_transaction(&self, id: TransactionId) -> StoreResult<()> {
        // Keep the splits' ids, so undoing the delete gives them back.
        let txn = id.to_string();
        self.tx
            .execute("DELETE FROM split_gone WHERE txn_id = ?1", params![txn])?;
        self.tx.execute(
            "INSERT INTO split_gone (txn_id, idx, split_id, account_id, value, quantity, memo,
                                     class_id, location_id, fund_id, project_id, billable)
             SELECT txn_id, idx, split_id, account_id, value, quantity, memo,
                    class_id, location_id, fund_id, project_id, billable
             FROM split WHERE txn_id = ?1 AND split_id IS NOT NULL",
            params![txn],
        )?;
        self.tx
            .execute("DELETE FROM txn WHERE id = ?1", params![txn])?;
        Ok(())
    }

    /// Transactions in register order (date, entered_at, id).
    pub fn transactions(&self, f: &TxnFilter) -> StoreResult<Vec<Transaction>> {
        let accounts_json = f
            .accounts
            .as_ref()
            .map(|ids| {
                serde_json::to_string(&ids.iter().map(ToString::to_string).collect::<Vec<_>>())
            })
            .transpose()?;
        let mut stmt = self.tx.prepare(&format!(
            "SELECT {TXN_COLS} FROM txn t
             WHERE (?1 IS NULL OR EXISTS (
                      SELECT 1 FROM split s WHERE s.txn_id = t.id
                      AND s.account_id IN (SELECT value FROM json_each(?1))))
               AND (?2 IS NULL OR t.date >= ?2)
               AND (?3 IS NULL OR t.date <= ?3)
               AND (?4 OR t.status = 'posted')
             ORDER BY t.date, t.entered_at, t.id
             LIMIT ?5 OFFSET ?6"
        ))?;
        let raws = stmt
            .query_map(
                params![
                    accounts_json,
                    f.from.map(|d| d.to_string()),
                    f.to.map(|d| d.to_string()),
                    f.include_voided,
                    f.limit.map_or(-1, i64::from),
                    f.offset,
                ],
                raw_txn,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        raws.into_iter().map(|r| to_txn(self.tx, r)).collect()
    }

    /// Whether any transaction is dated in `(after, through]`.
    pub fn any_transaction_between(&self, after: Option<Date>, through: Date) -> StoreResult<bool> {
        Ok(self.tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM txn WHERE (?1 IS NULL OR date > ?1) AND date <= ?2)",
            params![after.map(|d| d.to_string()), through.to_string()],
            |r| r.get(0),
        )?)
    }

    /// Splits of posted transactions dated on or before `as_of` (all dates if `None`).
    pub fn posted_splits(&self, as_of: Option<Date>) -> StoreResult<Vec<SplitRow>> {
        let mut stmt = self.tx.prepare(
            "SELECT s.txn_id, s.account_id, t.date, t.status, t.currency, s.value, s.quantity,
                    s.reconcile, s.class_id, s.location_id, s.fund_id, s.project_id, s.billable
             FROM split s JOIN txn t ON t.id = s.txn_id
             WHERE t.status = 'posted' AND (?1 IS NULL OR t.date <= ?1)
             ORDER BY t.date, t.entered_at, t.id, s.idx",
        )?;
        let raws = stmt
            .query_map(params![as_of.map(|d| d.to_string())], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    SplitTags {
                        class: r.get::<_, Option<String>>(8)?,
                        location: r.get::<_, Option<String>>(9)?,
                        fund: r.get::<_, Option<String>>(10)?,
                        project: r.get::<_, Option<String>>(11)?,
                        billable: r.get::<_, bool>(12)?,
                    },
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        raws.into_iter()
            .map(|(txn, acct, date, status, cur, value, qty, rec, tags)| {
                Ok(SplitRow {
                    txn_id: parse("transaction id", &txn)?,
                    account: parse("account id", &acct)?,
                    date: parse("date", &date)?,
                    status: parse("status", &status)?,
                    currency: parse("currency", &cur)?,
                    value: parse("value", &value)?,
                    quantity: parse("quantity", &qty)?,
                    reconcile: parse("reconcile", &rec)?,
                    tags,
                })
            })
            .collect()
    }

    /// Every setting row as (module, key, value), for snapshots and comparisons.
    pub fn all_settings(&self) -> StoreResult<Vec<(String, String, serde_json::Value)>> {
        let mut stmt = self
            .tx
            .prepare("SELECT module, key, value_json FROM book_setting ORDER BY module, key")?;
        let raws = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        raws.into_iter()
            .map(|(m, k, v)| Ok((m, k, serde_json::from_str(&v)?)))
            .collect()
    }
}

// ----- documents -------------------------------------------------------------

/// Tables holding JSON documents (import, rules, profiles, statements,
/// reconciliations). The set is closed, so table names are never user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DocTable {
    ImportBatch,
    ImportRow,
    ImportAccountMap,
    Statement,
    CsvProfile,
    Rule,
    Reconciliation,
    Schedule,
    ScheduleOccurrence,
    Budget,
    Security,
    Price,
    InvTxn,
    InvImportRow,
    ImportSecurityMap,
    InvProfile,
    Definition,
    Party,
    Item,
    TaxRate,
    SalesDoc,
    Payment,
    Deposit,
    BillDoc,
    BillPayment,
    TxnLink,
    Dimension,
    Template,
    Connection,
    AccountLink,
    OfxInstitution,
    Expense,
    Receipt,
    /// Form1099Keep (migration v15).
    F1099Payer,
    F1099Recipient,
    F1099Form,
    /// Mileage (migration v17).
    Vehicle,
    Trip,
    SavedTrip,
    MileageRate,
    /// Feed rows removed by an opening balance's statement start (migration v18):
    /// the import treats them as duplicates so the feed does not bring them back.
    ImportForgotten,
    /// Subscription audit (migration v19): subscriptions and recurring bills, and
    /// the answers to suggestions (grp = account id).
    Subscription,
    SubscriptionAnswer,
    /// Projects (migration v20): project records (projects.md).
    Project,
}

impl DocTable {
    fn table(self) -> &'static str {
        match self {
            DocTable::ImportBatch => "import_batch",
            DocTable::ImportRow => "import_row",
            DocTable::ImportAccountMap => "import_account_map",
            DocTable::Statement => "statement",
            DocTable::CsvProfile => "csv_profile",
            DocTable::Rule => "rule",
            DocTable::Reconciliation => "reconciliation",
            DocTable::Schedule => "schedule",
            DocTable::ScheduleOccurrence => "schedule_occurrence",
            DocTable::Budget => "budget",
            DocTable::Security => "security",
            DocTable::Price => "price",
            DocTable::InvTxn => "inv_txn",
            DocTable::InvImportRow => "inv_import_row",
            DocTable::ImportSecurityMap => "import_security_map",
            DocTable::InvProfile => "inv_profile",
            DocTable::Definition => "definition",
            DocTable::Party => "party",
            DocTable::Item => "item",
            DocTable::TaxRate => "tax_rate",
            DocTable::SalesDoc => "sales_doc",
            DocTable::Payment => "payment",
            DocTable::Deposit => "deposit",
            DocTable::BillDoc => "bill_doc",
            DocTable::BillPayment => "bill_payment",
            DocTable::TxnLink => "txn_link",
            DocTable::Dimension => "dimension",
            DocTable::Template => "template",
            DocTable::Connection => "connection",
            DocTable::AccountLink => "account_link",
            DocTable::OfxInstitution => "ofx_institution",
            DocTable::Expense => "expense",
            DocTable::Receipt => "receipt",
            DocTable::F1099Payer => "f1099_payer",
            DocTable::F1099Recipient => "f1099_recipient",
            DocTable::F1099Form => "f1099_form",
            DocTable::Vehicle => "vehicle",
            DocTable::Trip => "trip",
            DocTable::SavedTrip => "saved_trip",
            DocTable::MileageRate => "mileage_rate",
            DocTable::ImportForgotten => "import_forgotten",
            DocTable::Subscription => "subscription",
            DocTable::SubscriptionAnswer => "subscription_answer",
            DocTable::Project => "project",
        }
    }
}

/// A stored receipt file: its content and its thumbnail.
pub type ReceiptFile = (Vec<u8>, Option<Vec<u8>>);

/// One stored document.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Doc {
    pub id: String,
    /// Group key (batch id, account id), indexed.
    pub grp: Option<String>,
    /// Order within the group.
    pub ord: i64,
    pub json: serde_json::Value,
}

/// A posted split, for import matching and reconciliation.
#[derive(Debug, Clone)]
pub struct AccountSplit {
    pub txn_id: TransactionId,
    pub index: usize,
    pub date: Date,
    pub entered_at: String,
    pub payee: Option<String>,
    pub quantity: Decimal,
    pub reconcile: Reconcile,
    pub fingerprint: Option<String>,
}

impl WriteTx<'_> {
    pub fn get_doc(&self, t: DocTable, id: &str) -> StoreResult<Option<Doc>> {
        let row = self
            .tx
            .query_row(
                &format!("SELECT id, grp, ord, json FROM {} WHERE id = ?1", t.table()),
                params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(id, grp, ord, json)| {
            Ok(Doc {
                id,
                grp,
                ord,
                json: serde_json::from_str(&json)?,
            })
        })
        .transpose()
    }

    pub fn put_doc(&self, t: DocTable, d: &Doc) -> StoreResult<()> {
        self.tx.execute(
            &format!(
                "INSERT INTO {} (id, grp, ord, json) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (id) DO UPDATE SET grp = excluded.grp, ord = excluded.ord,
                   json = excluded.json",
                t.table()
            ),
            params![d.id, d.grp, d.ord, serde_json::to_string(&d.json)?],
        )?;
        Ok(())
    }

    pub fn delete_doc(&self, t: DocTable, id: &str) -> StoreResult<()> {
        self.tx.execute(
            &format!("DELETE FROM {} WHERE id = ?1", t.table()),
            params![id],
        )?;
        Ok(())
    }

    /// Documents of a table, optionally only one group, ordered by `ord` then `id`.
    pub fn docs(&self, t: DocTable, grp: Option<&str>) -> StoreResult<Vec<Doc>> {
        let mut stmt = self.tx.prepare(&format!(
            "SELECT id, grp, ord, json FROM {} WHERE (?1 IS NULL OR grp = ?1) ORDER BY ord, id",
            t.table()
        ))?;
        let raws = stmt
            .query_map(params![grp], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        raws.into_iter()
            .map(|(id, grp, ord, json)| {
                Ok(Doc {
                    id,
                    grp,
                    ord,
                    json: serde_json::from_str(&json)?,
                })
            })
            .collect()
    }

    /// Store a receipt file (content-addressed: an existing id is left as it is).
    pub fn put_receipt_file(
        &self,
        id: &str,
        content: &[u8],
        thumbnail: Option<&[u8]>,
    ) -> StoreResult<()> {
        self.tx.execute(
            "INSERT INTO receipt_file (id, content, thumbnail) VALUES (?1, ?2, ?3)
             ON CONFLICT (id) DO NOTHING",
            params![id, content, thumbnail],
        )?;
        Ok(())
    }

    /// A receipt file's content and thumbnail.
    pub fn receipt_file(&self, id: &str) -> StoreResult<Option<ReceiptFile>> {
        Ok(self
            .tx
            .query_row(
                "SELECT content, thumbnail FROM receipt_file WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// The transaction and split index in `account` carrying `fingerprint`, if any.
    pub fn fingerprint_split(
        &self,
        account: AccountId,
        fingerprint: &str,
    ) -> StoreResult<Option<(TransactionId, usize)>> {
        let row = self
            .tx
            .query_row(
                "SELECT txn_id, idx FROM split WHERE account_id = ?1 AND import_fingerprint = ?2 LIMIT 1",
                params![account.to_string(), fingerprint],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        row.map(|(t, i)| {
            Ok((
                parse("transaction id", &t)?,
                usize::try_from(i).map_err(|e| corrupt("split index", e))?,
            ))
        })
        .transpose()
    }

    /// Whether a split in `account` carries `fingerprint`.
    pub fn fingerprint_exists(&self, account: AccountId, fingerprint: &str) -> StoreResult<bool> {
        Ok(self.tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM split WHERE account_id = ?1 AND import_fingerprint = ?2)",
            params![account.to_string(), fingerprint],
            |r| r.get(0),
        )?)
    }

    /// Splits of posted transactions in `account` dated within `[from, to]`, in
    /// register order.
    pub fn account_splits(
        &self,
        account: AccountId,
        from: Option<Date>,
        to: Option<Date>,
    ) -> StoreResult<Vec<AccountSplit>> {
        let mut stmt = self.tx.prepare(
            "SELECT s.txn_id, s.idx, t.date, t.entered_at, t.payee, s.quantity, s.reconcile,
                    s.import_fingerprint
             FROM split s JOIN txn t ON t.id = s.txn_id
             WHERE s.account_id = ?1 AND t.status = 'posted'
               AND (?2 IS NULL OR t.date >= ?2) AND (?3 IS NULL OR t.date <= ?3)
             ORDER BY t.date, t.entered_at, t.id, s.idx",
        )?;
        let raws = stmt
            .query_map(
                params![
                    account.to_string(),
                    from.map(|d| d.to_string()),
                    to.map(|d| d.to_string())
                ],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<String>>(7)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        raws.into_iter()
            .map(|(txn, idx, date, entered_at, payee, qty, rec, fp)| {
                Ok(AccountSplit {
                    txn_id: parse("transaction id", &txn)?,
                    index: usize::try_from(idx).map_err(|e| corrupt("split index", e))?,
                    date: parse("date", &date)?,
                    entered_at,
                    payee,
                    quantity: parse("quantity", &qty)?,
                    reconcile: parse("reconcile", &rec)?,
                    fingerprint: fp,
                })
            })
            .collect()
    }
}

// ----- register --------------------------------------------------------------

/// One split of a register, with what the register view needs of its transaction.
#[derive(Debug, Clone)]
pub struct RegisterSplit {
    pub txn_id: TransactionId,
    pub index: usize,
    pub account: AccountId,
    pub date: Date,
    pub entered_at: String,
    pub number: Option<String>,
    pub payee: Option<String>,
    pub txn_memo: Option<String>,
    pub split_memo: Option<String>,
    pub quantity: Decimal,
    pub reconcile: Reconcile,
    pub status: TxnStatus,
    pub imported: bool,
    /// Splits of the transaction outside the account set.
    pub other_count: usize,
    /// The single other split's account when `other_count` is 1.
    pub other_account: Option<AccountId>,
}

/// A transaction's payee and when it was last used, newest first.
#[derive(Debug, Clone)]
pub struct PayeeUse {
    pub txn_id: TransactionId,
    pub payee: String,
}

impl WriteTx<'_> {
    /// Every split in `accounts` of transactions dated on or before `to`, in register
    /// order (date, entered time, id, split index). One query, so large registers stay
    /// fast.
    pub fn register_splits(
        &self,
        accounts: &[AccountId],
        to: Option<Date>,
    ) -> StoreResult<Vec<RegisterSplit>> {
        let set =
            serde_json::to_string(&accounts.iter().map(ToString::to_string).collect::<Vec<_>>())?;
        let mut stmt = self.tx.prepare(
            "WITH acc(id) AS (SELECT value FROM json_each(?1)),
                  mine AS (SELECT s.* FROM split s JOIN txn t ON t.id = s.txn_id
                           WHERE s.account_id IN (SELECT id FROM acc)
                             AND (?2 IS NULL OR t.date <= ?2)),
                  others AS (SELECT o.txn_id, count(*) AS n, min(o.account_id) AS a
                             FROM split o
                             WHERE o.txn_id IN (SELECT txn_id FROM mine)
                               AND o.account_id NOT IN (SELECT id FROM acc)
                             GROUP BY o.txn_id)
             SELECT s.txn_id, s.idx, s.account_id, t.date, t.entered_at, t.number, t.payee, t.memo,
                    s.memo, s.quantity, s.reconcile, t.status, s.import_fingerprint IS NOT NULL,
                    coalesce(x.n, 0), x.a
             FROM mine s JOIN txn t ON t.id = s.txn_id LEFT JOIN others x ON x.txn_id = s.txn_id
             ORDER BY t.date, t.entered_at, t.id, s.idx",
        )?;
        type Raw = (
            String,
            i64,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
            String,
            String,
            bool,
            i64,
            Option<String>,
        );
        let raws = stmt
            .query_map(
                params![set, to.map(|d| d.to_string())],
                |r| -> rusqlite::Result<Raw> {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                        r.get(11)?,
                        r.get(12)?,
                        r.get(13)?,
                        r.get(14)?,
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        raws.into_iter()
            .map(
                |(
                    txn,
                    idx,
                    acc,
                    date,
                    entered_at,
                    number,
                    payee,
                    tmemo,
                    smemo,
                    qty,
                    rec,
                    status,
                    imported,
                    count,
                    other,
                )| {
                    let other_count = usize::try_from(count).map_err(|e| corrupt("count", e))?;
                    Ok(RegisterSplit {
                        txn_id: parse("transaction id", &txn)?,
                        index: usize::try_from(idx).map_err(|e| corrupt("split index", e))?,
                        account: parse("account id", &acc)?,
                        date: parse("date", &date)?,
                        entered_at,
                        number,
                        payee,
                        txn_memo: tmemo,
                        split_memo: smemo,
                        quantity: parse("quantity", &qty)?,
                        reconcile: parse("reconcile", &rec)?,
                        status: parse("status", &status)?,
                        imported,
                        other_count,
                        other_account: if other_count == 1 {
                            other.map(|o| parse("account id", &o)).transpose()?
                        } else {
                            None
                        },
                    })
                },
            )
            .collect()
    }

    /// Posted transactions with a payee, newest first, optionally only those touching
    /// `account`.
    pub fn payee_uses(&self, account: Option<AccountId>) -> StoreResult<Vec<PayeeUse>> {
        let mut stmt = self.tx.prepare(
            "SELECT t.id, t.payee FROM txn t
             WHERE t.status = 'posted' AND t.payee IS NOT NULL AND t.payee <> ''
               AND (?1 IS NULL OR EXISTS (SELECT 1 FROM split s WHERE s.txn_id = t.id AND s.account_id = ?1))
             ORDER BY t.date DESC, t.entered_at DESC, t.id DESC",
        )?;
        let raws = stmt
            .query_map(params![account.map(|a| a.to_string())], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        raws.into_iter()
            .map(|(id, payee)| {
                Ok(PayeeUse {
                    txn_id: parse("transaction id", &id)?,
                    payee,
                })
            })
            .collect()
    }

    /// Most recent use of each account (latest date, then entered time) by a posted
    /// transaction.
    pub fn account_last_used(&self) -> StoreResult<std::collections::HashMap<AccountId, String>> {
        let mut stmt = self.tx.prepare(
            "SELECT s.account_id, max(t.date || ' ' || t.entered_at) FROM split s
             JOIN txn t ON t.id = s.txn_id WHERE t.status = 'posted' GROUP BY s.account_id",
        )?;
        let raws = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        raws.into_iter()
            .map(|(a, when)| Ok((parse("account id", &a)?, when)))
            .collect()
    }
}

/// What identifies a split for keeping its id across an edit: its id, then account,
/// value, quantity, memo and tags (not the reconcile flag or import fingerprint).
struct SplitKey {
    id: String,
    account: String,
    value: String,
    quantity: String,
    memo: Option<String>,
    tags: [Option<String>; 4],
    billable: bool,
}

fn split_keys(conn: &Connection, table: &str, txn: &str) -> StoreResult<Vec<SplitKey>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT split_id, account_id, value, quantity, memo, class_id, location_id, fund_id,
                project_id, billable FROM {table}
         WHERE txn_id = ?1 AND split_id IS NOT NULL ORDER BY idx"
    ))?;
    let rows = stmt
        .query_map(params![txn], |r| {
            Ok(SplitKey {
                id: r.get(0)?,
                account: r.get(1)?,
                value: r.get(2)?,
                quantity: r.get(3)?,
                memo: r.get(4)?,
                tags: [r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?],
                billable: r.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The id of each new split: the id of an unused old split with the same account,
/// amounts, memo and tags; else of one with the same account and project (an amount
/// or memo edited); else a new id.
fn keep_split_ids(old: &[SplitKey], new: &[Split]) -> Vec<String> {
    let mut used = vec![false; old.len()];
    let mut out: Vec<Option<String>> = vec![None; new.len()];
    let same = |o: &SplitKey, s: &Split| {
        o.account == s.account.to_string()
            && o.value == s.value.to_string()
            && o.quantity == s.quantity.to_string()
            && o.memo == s.memo
            && o.tags
                == [
                    s.tags.class.clone(),
                    s.tags.location.clone(),
                    s.tags.fund.clone(),
                    s.tags.project.clone(),
                ]
            && o.billable == s.tags.billable
    };
    let near =
        |o: &SplitKey, s: &Split| o.account == s.account.to_string() && o.tags[3] == s.tags.project;
    for pass in 0..2 {
        for (slot, s) in out.iter_mut().zip(new) {
            if slot.is_some() {
                continue;
            }
            let found = old
                .iter()
                .zip(used.iter_mut())
                .find(|(o, u)| !**u && if pass == 0 { same(o, s) } else { near(o, s) });
            if let Some((o, u)) = found {
                *u = true;
                *slot = Some(o.id.clone());
            }
        }
    }
    out.into_iter()
        .map(|x| x.unwrap_or_else(|| SplitId::new().to_string()))
        .collect()
}
