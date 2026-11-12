//! Registry tests, including every acceptance test of the Phase 1 ledger specs.
//! Test code may unwrap.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use mk_core::CommodityCode;
use serde_json::json;

mod accounts_spec;
mod advanced_spec;
mod audit_spec;
mod backups_spec;
mod balances_spec;
mod book_picture_spec;
mod book_profile_spec;
mod brokerage_spec;
mod budgets_spec;
mod capture_setup_spec;
mod capture_wifi_spec;
mod connect_spec;
mod dashboards_spec;
mod definitions_spec;
mod dimensions_spec;
mod expenses_spec;
mod import_cleared;
mod import_props;
mod import_spec;
mod input_safety_spec;
mod inv_import_spec;
mod investments_spec;
mod mileage_spec;
mod obc_spec;
mod ofx_spec;
mod opening_spec;
mod phase0;
mod processors_spec;
mod projects_spec;
mod props;
mod purchases_spec;
mod reconcile_spec;
mod register_spec;
mod release_prep_spec;
mod reports_spec;
mod rules_spec;
mod sales_spec;
mod schedules_spec;
mod security_review_spec;
mod statements_spec;
mod subscriptions_spec;
mod txns_spec;

/// Test harness: a fresh in-memory book plus helpers.
pub(crate) struct Book {
    pub r: Registry,
    pub s: Store,
    pub a: Actor,
}

impl Book {
    pub fn new() -> Self {
        Self::with_currency("USD")
    }

    pub fn with_currency(c: &str) -> Self {
        let s = Store::in_memory("Test", &CommodityCode::new(c).unwrap()).unwrap();
        Self {
            r: Registry::with_builtins(),
            s,
            a: Actor("test".into()),
        }
    }

    pub fn try_run(&mut self, cmd: &str, args: Value) -> CommandResult<Value> {
        self.r.invoke(&mut self.s, &self.a, cmd, args)
    }

    pub fn run(&mut self, cmd: &str, args: Value) -> Value {
        match self.try_run(cmd, args.clone()) {
            Ok(v) => v,
            Err(e) => panic!("{cmd} {args} failed: {e}"),
        }
    }

    pub fn try_as(&mut self, actor: &str, cmd: &str, args: Value) -> CommandResult<Value> {
        self.r.invoke(&mut self.s, &Actor(actor.into()), cmd, args)
    }

    pub fn as_actor(&mut self, actor: &str, cmd: &str, args: Value) -> Value {
        self.try_as(actor, cmd, args.clone())
            .unwrap_or_else(|e| panic!("{cmd} {args} as {actor} failed: {e}"))
    }

    pub fn err(&mut self, cmd: &str, args: Value) -> CommandError {
        match self.try_run(cmd, args.clone()) {
            Ok(v) => panic!("{cmd} {args} unexpectedly succeeded: {v}"),
            Err(e) => e,
        }
    }

    /// Create an account and return its id.
    pub fn account(&mut self, args: Value) -> String {
        self.run("account.create", args)["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Standard chart: Assets (placeholder), Assets:Checking, Expenses:Groceries,
    /// Liabilities:Visa.
    pub fn standard(&mut self) {
        self.account(json!({"name": "Assets", "type": "asset", "placeholder": true}));
        self.account(
            json!({"name": "Checking", "type": "asset", "subtype": "bank", "parent": "Assets"}),
        );
        self.account(json!({"name": "Expenses", "type": "expense", "placeholder": true}));
        self.account(json!({"name": "Groceries", "type": "expense", "parent": "Expenses"}));
        self.account(json!({"name": "Liabilities", "type": "liability", "placeholder": true}));
        self.account(json!({"name": "Visa", "type": "liability", "subtype": "credit-card", "parent": "Liabilities"}));
    }

    /// Stage inline QIF content into `account`; returns the batch.
    pub fn stage_qif(&mut self, content: &str, account: &str) -> Value {
        self.run(
            "import.stage",
            json!({"format": "qif", "content": content, "account": account}),
        )
    }

    /// Commit a batch.
    pub fn commit(&mut self, batch: &Value) -> Value {
        self.run("import.commit", json!({"batch": batch["id"]}))
    }

    pub fn txn_count(&mut self) -> usize {
        self.run("txn.list", json!({"include_voided": true}))
            .as_array()
            .unwrap()
            .len()
    }

    /// Two-split transaction `debit` +amount / `credit` -amount; returns its id.
    pub fn transfer(&mut self, date: &str, debit: &str, credit: &str, amount: &str) -> String {
        let neg = format!("-{amount}");
        self.run(
            "txn.create",
            json!({"date": date, "splits": [
                {"account": debit, "value": amount},
                {"account": credit, "value": neg},
            ]}),
        )["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Display balance of an account in one commodity (missing commodity: "absent").
    pub fn shown(&mut self, account: &str, commodity: &str) -> String {
        let b = self.run("account.balance", json!({"account": account}));
        b["display"][commodity]
            .as_str()
            .unwrap_or("absent")
            .to_owned()
    }

    pub fn audit_len(&self) -> usize {
        self.s.audit_entries(10_000).unwrap().len()
    }

    /// Everything undo must restore: accounts, transactions and settings.
    pub fn state(&mut self) -> Value {
        let accounts = self.run("account.list", json!({"include_archived": true}));
        let txns = self.run("txn.list", json!({"include_voided": true}));
        let modules = self.run("module.list", json!({}));
        let closing = self.run("book.info", json!({}))["closing_date"].clone();
        let settings = self
            .s
            .write(|tx| -> CommandResult<Value> { Ok(json!(tx.all_settings()?)) })
            .unwrap();
        // has_splits is derived usage history, not book content.
        let accounts: Vec<Value> = accounts
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                let mut a = a.clone();
                a.as_object_mut().unwrap().remove("has_splits");
                a
            })
            .collect();
        let settings: Vec<Value> = settings
            .as_array()
            .unwrap()
            .iter()
            .map(|row| json!([row[0], row[1], row[2]]))
            .collect();
        let _ = modules;
        json!({"accounts": accounts, "txns": txns, "settings": settings, "closing": closing})
    }
}
