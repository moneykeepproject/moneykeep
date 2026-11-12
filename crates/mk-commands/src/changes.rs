//! Entity snapshots for the audit log, and the only way commands change state.
//!
//! Every mutation goes through a `Ctx` method here, which records the entity as it
//! was before (first time it is touched in the command) and as it is after. The
//! registry writes both into the command's audit entry; undo and redo replay them.

use std::collections::BTreeMap;

use mk_core::{Account, Date, Transaction};
use mk_store::{Doc, DocTable};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{CommandError, CommandResult, Ctx};

/// Kinds of entity a command can change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Account,
    Transaction,
    Setting,
    Book,
    /// The book's name (app-desktop.md revision 26); id is the book id.
    BookName,
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
    /// Form1099Keep tables (migration v15).
    F1099Payer,
    F1099Recipient,
    F1099Form,
    /// Mileage (migration v17).
    Vehicle,
    Trip,
    SavedTrip,
    MileageRate,
    /// Feed rows an opening balance removed (migration v18).
    ImportForgotten,
    /// Subscription audit (migration v19).
    Subscription,
    SubscriptionAnswer,
    /// Projects (migration v20).
    Project,
}

impl Kind {
    /// The document table of a document kind.
    pub fn doc_table(self) -> Option<DocTable> {
        Some(match self {
            Kind::ImportBatch => DocTable::ImportBatch,
            Kind::ImportRow => DocTable::ImportRow,
            Kind::ImportAccountMap => DocTable::ImportAccountMap,
            Kind::Statement => DocTable::Statement,
            Kind::CsvProfile => DocTable::CsvProfile,
            Kind::Rule => DocTable::Rule,
            Kind::Reconciliation => DocTable::Reconciliation,
            Kind::Schedule => DocTable::Schedule,
            Kind::ScheduleOccurrence => DocTable::ScheduleOccurrence,
            Kind::Budget => DocTable::Budget,
            Kind::Security => DocTable::Security,
            Kind::Price => DocTable::Price,
            Kind::InvTxn => DocTable::InvTxn,
            Kind::InvImportRow => DocTable::InvImportRow,
            Kind::ImportSecurityMap => DocTable::ImportSecurityMap,
            Kind::InvProfile => DocTable::InvProfile,
            Kind::Definition => DocTable::Definition,
            Kind::Party => DocTable::Party,
            Kind::Item => DocTable::Item,
            Kind::TaxRate => DocTable::TaxRate,
            Kind::SalesDoc => DocTable::SalesDoc,
            Kind::Payment => DocTable::Payment,
            Kind::Deposit => DocTable::Deposit,
            Kind::BillDoc => DocTable::BillDoc,
            Kind::BillPayment => DocTable::BillPayment,
            Kind::TxnLink => DocTable::TxnLink,
            Kind::Dimension => DocTable::Dimension,
            Kind::Template => DocTable::Template,
            Kind::Connection => DocTable::Connection,
            Kind::AccountLink => DocTable::AccountLink,
            Kind::OfxInstitution => DocTable::OfxInstitution,
            Kind::Expense => DocTable::Expense,
            Kind::Receipt => DocTable::Receipt,
            Kind::F1099Payer => DocTable::F1099Payer,
            Kind::F1099Recipient => DocTable::F1099Recipient,
            Kind::F1099Form => DocTable::F1099Form,
            Kind::Vehicle => DocTable::Vehicle,
            Kind::Trip => DocTable::Trip,
            Kind::SavedTrip => DocTable::SavedTrip,
            Kind::MileageRate => DocTable::MileageRate,
            Kind::ImportForgotten => DocTable::ImportForgotten,
            Kind::Subscription => DocTable::Subscription,
            Kind::SubscriptionAnswer => DocTable::SubscriptionAnswer,
            Kind::Project => DocTable::Project,
            Kind::Account | Kind::Transaction | Kind::Setting | Kind::Book | Kind::BookName => {
                return None;
            }
        })
    }

    pub fn of_table(t: DocTable) -> Kind {
        match t {
            DocTable::ImportBatch => Kind::ImportBatch,
            DocTable::ImportRow => Kind::ImportRow,
            DocTable::ImportAccountMap => Kind::ImportAccountMap,
            DocTable::Statement => Kind::Statement,
            DocTable::CsvProfile => Kind::CsvProfile,
            DocTable::Rule => Kind::Rule,
            DocTable::Reconciliation => Kind::Reconciliation,
            DocTable::Schedule => Kind::Schedule,
            DocTable::ScheduleOccurrence => Kind::ScheduleOccurrence,
            DocTable::Budget => Kind::Budget,
            DocTable::Security => Kind::Security,
            DocTable::Price => Kind::Price,
            DocTable::InvTxn => Kind::InvTxn,
            DocTable::InvImportRow => Kind::InvImportRow,
            DocTable::ImportSecurityMap => Kind::ImportSecurityMap,
            DocTable::InvProfile => Kind::InvProfile,
            DocTable::Definition => Kind::Definition,
            DocTable::Party => Kind::Party,
            DocTable::Item => Kind::Item,
            DocTable::TaxRate => Kind::TaxRate,
            DocTable::SalesDoc => Kind::SalesDoc,
            DocTable::Payment => Kind::Payment,
            DocTable::Deposit => Kind::Deposit,
            DocTable::BillDoc => Kind::BillDoc,
            DocTable::BillPayment => Kind::BillPayment,
            DocTable::TxnLink => Kind::TxnLink,
            DocTable::Dimension => Kind::Dimension,
            DocTable::Template => Kind::Template,
            DocTable::Connection => Kind::Connection,
            DocTable::AccountLink => Kind::AccountLink,
            DocTable::OfxInstitution => Kind::OfxInstitution,
            DocTable::Expense => Kind::Expense,
            DocTable::Receipt => Kind::Receipt,
            DocTable::F1099Payer => Kind::F1099Payer,
            DocTable::F1099Recipient => Kind::F1099Recipient,
            DocTable::F1099Form => Kind::F1099Form,
            DocTable::Vehicle => Kind::Vehicle,
            DocTable::Trip => Kind::Trip,
            DocTable::SavedTrip => Kind::SavedTrip,
            DocTable::MileageRate => Kind::MileageRate,
            DocTable::ImportForgotten => Kind::ImportForgotten,
            DocTable::Subscription => Kind::Subscription,
            DocTable::SubscriptionAnswer => Kind::SubscriptionAnswer,
            DocTable::Project => Kind::Project,
        }
    }
}

/// Identity of an entity: kind plus id. Settings use `module/key`; the book uses its id.
pub(crate) type Key = (Kind, String);

/// One entity snapshot in an audit entry's `before` or `after` list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub kind: Kind,
    pub id: String,
    pub data: Value,
}

#[derive(Debug, Clone, Default)]
struct Change {
    before: Option<Value>,
    after: Option<Value>,
}

/// Changes made by one command, in first-touched order.
#[derive(Debug, Default)]
pub(crate) struct ChangeLog {
    order: Vec<Key>,
    changes: BTreeMap<Key, Change>,
}

impl ChangeLog {
    fn record(&mut self, key: Key, before: Option<Value>, after: Option<Value>) {
        match self.changes.get_mut(&key) {
            Some(c) => c.after = after,
            None => {
                self.order.push(key.clone());
                self.changes.insert(key, Change { before, after });
            }
        }
    }

    /// True when no entity actually changed.
    pub fn is_empty(&self) -> bool {
        self.changes.values().all(|c| c.before == c.after)
    }

    /// `(before, after)` for the audit entry. `before` is null when nothing existed
    /// before (pure creation); `after` is null when nothing exists after (pure
    /// deletion). A command that changed nothing records `after = []`.
    pub fn to_audit(&self) -> (Option<Value>, Option<Value>) {
        let mut before = Vec::new();
        let mut after = Vec::new();
        for key in &self.order {
            let Some(c) = self.changes.get(key) else {
                continue;
            };
            if c.before == c.after {
                continue;
            }
            let (kind, id) = key;
            if let Some(b) = &c.before {
                before.push(json!({ "kind": kind, "id": id, "data": b }));
            }
            if let Some(a) = &c.after {
                after.push(json!({ "kind": kind, "id": id, "data": a }));
            }
        }
        let nothing = before.is_empty() && after.is_empty();
        let before = (!before.is_empty()).then_some(Value::Array(before));
        let after = if after.is_empty() && !nothing {
            None
        } else {
            Some(Value::Array(after))
        };
        (before, after)
    }
}

/// Parse an audit entry's snapshot list (null → empty).
pub(crate) fn parse_snapshots(v: Option<&Value>) -> CommandResult<Vec<Snapshot>> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| CommandError::Integrity(format!("bad audit snapshot: {e}"))),
    }
}

pub(crate) fn setting_id(module: &str, key: &str) -> String {
    format!("{module}/{key}")
}

impl Ctx<'_> {
    fn log(&self, key: Key, before: Option<Value>, after: Option<Value>) {
        self.changes.borrow_mut().record(key, before, after);
    }

    /// Insert or replace an account. `old` is its current state (None if new).
    pub(crate) fn save_account(&self, old: Option<&Account>, new: &Account) -> CommandResult<()> {
        self.tx.put_account(new)?;
        self.log(
            (Kind::Account, new.id.to_string()),
            old.map(serde_json::to_value).transpose()?,
            Some(serde_json::to_value(new)?),
        );
        Ok(())
    }

    pub(crate) fn remove_account(&self, old: &Account) -> CommandResult<()> {
        self.tx.delete_account(old.id)?;
        self.log(
            (Kind::Account, old.id.to_string()),
            Some(serde_json::to_value(old)?),
            None,
        );
        Ok(())
    }

    /// Insert or replace a transaction. Accounts whose splits disappear are marked as
    /// having had a split (so they can no longer be deleted).
    pub(crate) fn save_txn(
        &self,
        old: Option<&Transaction>,
        new: &Transaction,
    ) -> CommandResult<()> {
        if let Some(old) = old {
            self.note_removed_splits(old, Some(new))?;
        }
        self.tx.put_transaction(new)?;
        self.log(
            (Kind::Transaction, new.id.to_string()),
            old.map(serde_json::to_value).transpose()?,
            Some(serde_json::to_value(new)?),
        );
        crate::dimensions::after_save(self, new)
    }

    pub(crate) fn remove_txn(&self, old: &Transaction) -> CommandResult<()> {
        self.note_removed_splits(old, None)?;
        self.tx.delete_transaction(old.id)?;
        self.log(
            (Kind::Transaction, old.id.to_string()),
            Some(serde_json::to_value(old)?),
            None,
        );
        crate::dimensions::after_remove(self, old)
    }

    fn note_removed_splits(
        &self,
        old: &Transaction,
        new: Option<&Transaction>,
    ) -> CommandResult<()> {
        if self.suppress_usage.get() {
            return Ok(());
        }
        let mut seen = Vec::new();
        for s in &old.splits {
            let still = new.is_some_and(|n| n.splits.iter().any(|ns| ns.account == s.account));
            if !still && !seen.contains(&s.account) {
                seen.push(s.account);
                self.tx.record_usage(s.account, old.id)?;
            }
        }
        Ok(())
    }

    /// Set (Some) or remove (None) a module setting.
    pub(crate) fn save_setting(
        &self,
        module: &str,
        key: &str,
        value: Option<&Value>,
    ) -> CommandResult<()> {
        // AI access is a session operation only (mcp-server.md revision 3): no
        // registry command, definition file or undo writes it.
        if crate::ai_access::is_ai_key(key) {
            return Err(crate::CommandError::InvalidArgs(
                crate::ai_access::REFUSED_KEY.into(),
            ));
        }
        let before = self.tx.get_setting(module, key)?;
        match value {
            Some(v) => self.tx.set_setting(module, key, v, &self.actor.0)?,
            None => self.tx.delete_setting(module, key)?,
        }
        let snap =
            |v: Option<Value>| v.map(|v| json!({ "module": module, "key": key, "value": v }));
        self.log(
            (Kind::Setting, setting_id(module, key)),
            snap(before),
            snap(value.cloned()),
        );
        Ok(())
    }

    /// Insert, replace (`new` Some) or delete (`new` None) a document.
    pub(crate) fn save_doc(&self, t: DocTable, id: &str, new: Option<&Doc>) -> CommandResult<()> {
        let old = self.tx.get_doc(t, id)?;
        match new {
            Some(d) => self.tx.put_doc(t, d)?,
            None => self.tx.delete_doc(t, id)?,
        }
        self.log(
            (Kind::of_table(t), id.to_owned()),
            old.map(serde_json::to_value).transpose()?,
            new.map(serde_json::to_value).transpose()?,
        );
        Ok(())
    }

    /// Rename the book.
    pub(crate) fn save_book_name(&self, name: &str) -> CommandResult<()> {
        let info = self.tx.info()?;
        self.tx.set_name(name)?;
        self.log(
            (Kind::BookName, info.id),
            Some(json!({ "name": info.name })),
            Some(json!({ "name": name })),
        );
        Ok(())
    }

    /// Set or clear the book's closing date.
    pub(crate) fn save_closing_date(&self, date: Option<Date>) -> CommandResult<()> {
        let before = self.tx.closing_date()?;
        self.tx.set_closing_date(date)?;
        let id = self.tx.info()?.id;
        self.log(
            (Kind::Book, id),
            Some(json!({ "closing_date": before })),
            Some(json!({ "closing_date": date })),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_before_and_last_after_are_kept() {
        let mut log = ChangeLog::default();
        let k = (Kind::Setting, "m/k".to_owned());
        log.record(k.clone(), Some(json!(1)), Some(json!(2)));
        log.record(k, Some(json!(2)), Some(json!(3)));
        let (b, a) = log.to_audit();
        assert_eq!(b.unwrap()[0]["data"], json!(1));
        assert_eq!(a.unwrap()[0]["data"], json!(3));
    }

    #[test]
    fn creation_has_null_before_and_noop_has_empty_after() {
        let mut log = ChangeLog::default();
        log.record((Kind::Account, "a".into()), None, Some(json!({})));
        let (b, a) = log.to_audit();
        assert!(b.is_none());
        assert_eq!(a.unwrap().as_array().unwrap().len(), 1);
        let (b, a) = ChangeLog::default().to_audit();
        assert!(b.is_none());
        assert_eq!(a, Some(json!([])));
    }
}
