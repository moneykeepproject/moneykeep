//! Ordered, append-only schema migrations.
//!
//! Never edit a migration that has shipped; add a new one. The schema version is
//! stored in SQLite's `user_version` pragma and each applied migration is also
//! recorded in `schema_migration` for diagnostics.

pub(crate) struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
    /// Data changes that need code, run after `sql` in the same transaction.
    pub fix: Option<fn(&rusqlite::Connection) -> rusqlite::Result<()>>,
}

pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "book, settings, audit log",
        fix: None,
        sql: r#"
CREATE TABLE schema_migration (
    version     INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    applied_at  TEXT NOT NULL
);

-- Exactly one row: the book this file holds.
CREATE TABLE book (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    base_currency   TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    closing_date    TEXT,
    singleton       INTEGER NOT NULL DEFAULT 1 UNIQUE CHECK (singleton = 1)
);

-- Per-book module toggles and module settings, as typed JSON values.
CREATE TABLE book_setting (
    module      TEXT NOT NULL,
    key         TEXT NOT NULL,
    value_json  TEXT NOT NULL,
    changed_by  TEXT NOT NULL,
    changed_at  TEXT NOT NULL,
    PRIMARY KEY (module, key)
);

-- Append-only record of every state-changing command.
CREATE TABLE audit_log (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    at          TEXT NOT NULL,
    actor       TEXT NOT NULL,
    command     TEXT NOT NULL,
    args_json   TEXT NOT NULL,
    result_json TEXT NOT NULL
);

CREATE TRIGGER audit_log_no_update BEFORE UPDATE ON audit_log
BEGIN SELECT RAISE(ABORT, 'audit_log is append-only'); END;

CREATE TRIGGER audit_log_no_delete BEFORE DELETE ON audit_log
BEGIN SELECT RAISE(ABORT, 'audit_log is append-only'); END;
"#,
    },
    Migration {
        version: 2,
        name: "accounts, transactions, splits",
        fix: None,
        sql: r#"
-- Chart of accounts. Name rules (trimmed, no ':', unique among siblings ignoring
-- case) are enforced by the command layer.
CREATE TABLE account (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    type        TEXT NOT NULL CHECK (type IN ('asset','liability','equity','income','expense')),
    subtype     TEXT NOT NULL,
    parent_id   TEXT REFERENCES account (id),
    commodity   TEXT NOT NULL,
    code        TEXT UNIQUE,
    description TEXT,
    placeholder INTEGER NOT NULL DEFAULT 0 CHECK (placeholder IN (0, 1)),
    archived    INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
    created_at  TEXT NOT NULL
);
CREATE INDEX account_parent ON account (parent_id);

-- Amounts are exact decimals stored as text; dates are YYYY-MM-DD text.
CREATE TABLE txn (
    id          TEXT PRIMARY KEY,
    date        TEXT NOT NULL,
    currency    TEXT NOT NULL,
    payee       TEXT,
    number      TEXT,
    memo        TEXT,
    status      TEXT NOT NULL CHECK (status IN ('posted', 'voided')),
    void_reason TEXT,
    reverses    TEXT,
    entered_at  TEXT NOT NULL
);
CREATE INDEX txn_order ON txn (date, entered_at, id);

CREATE TABLE split (
    txn_id      TEXT NOT NULL REFERENCES txn (id) ON DELETE CASCADE,
    idx         INTEGER NOT NULL,
    account_id  TEXT NOT NULL REFERENCES account (id),
    value       TEXT NOT NULL,
    quantity    TEXT NOT NULL,
    memo        TEXT,
    reconcile   TEXT NOT NULL CHECK (reconcile IN ('n', 'c', 'r')),
    PRIMARY KEY (txn_id, idx)
);
CREATE INDEX split_account ON split (account_id);

-- An account "has had a split" if it has a live split or appears here: a split that
-- was removed by an edit or a delete. `seq` is the audit entry that removed it, so
-- undoing that entry can remove exactly these rows.
CREATE TABLE account_usage (
    account_id  TEXT NOT NULL REFERENCES account (id),
    txn_id      TEXT NOT NULL,
    seq         INTEGER
);
CREATE INDEX account_usage_account ON account_usage (account_id);
CREATE INDEX account_usage_seq ON account_usage (seq);

-- The default "Opening Balances" account of the book.
ALTER TABLE book ADD COLUMN opening_account_id TEXT REFERENCES account (id);

-- Books created before this migration get their default account now.
INSERT INTO account (id, name, type, subtype, commodity, created_at)
SELECT lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' ||
             substr(hex(randomblob(2)), 2) || '-8' || substr(hex(randomblob(2)), 2) || '-' ||
             hex(randomblob(6))),
       'Opening Balances', 'equity', 'opening-balances', base_currency, created_at
FROM book;
UPDATE book SET opening_account_id = (SELECT id FROM account LIMIT 1);
"#,
    },
    Migration {
        version: 3,
        name: "audit snapshots and undo links",
        fix: None,
        sql: r#"
-- Adding columns does not touch existing rows' content; rows stay append-only.
ALTER TABLE audit_log ADD COLUMN before_json TEXT;
ALTER TABLE audit_log ADD COLUMN after_json TEXT;
ALTER TABLE audit_log ADD COLUMN undoes INTEGER REFERENCES audit_log (seq);
ALTER TABLE audit_log ADD COLUMN redoes INTEGER REFERENCES audit_log (seq);
CREATE INDEX audit_log_undoes ON audit_log (undoes);
CREATE INDEX audit_log_actor ON audit_log (actor, seq);
"#,
    },
    Migration {
        version: 4,
        name: "import, rules, CSV profiles, statements, reconciliation",
        fix: None,
        sql: r#"
-- Fingerprint of the downloaded row a split came from or was matched to.
ALTER TABLE split ADD COLUMN import_fingerprint TEXT;
CREATE INDEX split_fingerprint ON split (account_id, import_fingerprint);

-- Import and reconciliation entities are JSON documents with an indexed group key
-- (batch id or account id) and an order key. The command layer owns their shape.
CREATE TABLE import_batch (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE import_row (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE import_account_map (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE statement (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE csv_profile (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE rule (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE reconciliation (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE INDEX import_row_grp ON import_row (grp, ord);
CREATE INDEX statement_grp ON statement (grp, ord);
CREATE INDEX reconciliation_grp ON reconciliation (grp, ord);
"#,
    },
    Migration {
        version: 5,
        name: "schedules, schedule occurrences, budgets",
        fix: None,
        sql: r#"
-- JSON documents like the import tables. Occurrences are grouped by schedule id;
-- only entered and skipped occurrences are stored.
CREATE TABLE schedule (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE schedule_occurrence (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE budget (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE INDEX schedule_occurrence_grp ON schedule_occurrence (grp, ord);
"#,
    },
    Migration {
        version: 6,
        name: "securities, prices, investment transaction metadata",
        fix: None,
        sql: r#"
-- JSON documents like the other module tables. Prices are grouped by security id and
-- ordered by date; investment metadata is keyed by transaction id and grouped by
-- security id. Lots are derived data and are not stored.
CREATE TABLE security (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE price (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE inv_txn (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE INDEX price_grp ON price (grp, ord);
CREATE INDEX inv_txn_grp ON inv_txn (grp, ord);
"#,
    },
    Migration {
        version: 7,
        name: "investment import rows, security mappings, investment profiles",
        fix: None,
        sql: r#"
-- Staged investment rows are grouped by batch id; security mappings are keyed by the
-- file's source security key; investment profiles by their lower-cased name.
CREATE TABLE inv_import_row (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE import_security_map (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE inv_profile (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE INDEX inv_import_row_grp ON inv_import_row (grp, ord);
"#,
    },
    Migration {
        version: 8,
        name: "definition packages; clean control and invisible characters from text",
        fix: Some(clean_text_v8),
        sql: r#"
-- Installed definition packages (definition-files.md), keyed by package id.
CREATE TABLE definition (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
"#,
    },
    Migration {
        version: 9,
        name: "parties, items, sales tax rates, sales documents, payments, deposits",
        fix: None,
        sql: r#"
-- Business sales (business-sales.md). Sales documents (invoices, estimates, credit
-- memos, refunds, sales tax payments) and payments are grouped by kind.
CREATE TABLE party (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE item (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE tax_rate (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE sales_doc (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE payment (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE deposit (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE INDEX sales_doc_grp ON sales_doc (grp, ord);
CREATE INDEX payment_grp ON payment (grp, ord);
"#,
    },
    Migration {
        version: 10,
        name: "bills, vendor credits, purchase orders, bill payments, transaction vendor links",
        fix: None,
        sql: r#"
-- Business purchases (business-purchases.md). Bills, vendor credits and purchase
-- orders are grouped by kind; bill payments and refunds received likewise.
CREATE TABLE bill_doc (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE bill_payment (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
-- Optional vendor link of an ordinary transaction (1099), keyed by transaction id.
CREATE TABLE txn_link (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE INDEX bill_doc_grp ON bill_doc (grp, ord);
CREATE INDEX bill_payment_grp ON bill_payment (grp, ord);
"#,
    },
    Migration {
        version: 11,
        name: "class, location and fund tags on splits; dimensions; document templates",
        fix: None,
        sql: r#"
-- Dimension tags (business-dimensions.md): record ids, NULL when untagged.
ALTER TABLE split ADD COLUMN class_id TEXT;
ALTER TABLE split ADD COLUMN location_id TEXT;
ALTER TABLE split ADD COLUMN fund_id TEXT;
-- Classes, locations and funds, grouped by dimension.
CREATE TABLE dimension (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
-- Document templates (logos as re-encoded RGB rasters).
CREATE TABLE template (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
"#,
    },
    Migration {
        version: 12,
        name: "bank-feed connections and account links; secrets of encrypted books",
        fix: None,
        sql: r#"
-- Bank-feed connections (connectivity.md): no credentials here, only a reference.
CREATE TABLE connection (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
-- Remote accounts of a connection (grp = connection id).
CREATE TABLE account_link (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE INDEX account_link_grp ON account_link (grp);
-- Credentials, written only while the book is password protected (the whole file is
-- encrypted then). Never audited, never part of undo, never exported.
CREATE TABLE secret (id TEXT PRIMARY KEY, value TEXT NOT NULL);
"#,
    },
    Migration {
        version: 13,
        name: "OFX institution profiles from definition files",
        fix: None,
        sql: r#"
-- OFX Direct Connect institution profiles (connectivity-brokerage.md): installed from
-- definition files; a connection keeps its own copy.
CREATE TABLE ofx_institution (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
"#,
    },
    Migration {
        version: 14,
        name: "expenses, receipts and receipt files",
        fix: None,
        sql: r#"
-- Expenses (expenses.md), grouped by status.
CREATE TABLE expense (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
-- Receipts: what each receipt is and what it is attached to (grp = "expense:<id>",
-- "txn:<id>" or "inbox").
CREATE TABLE receipt (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE INDEX receipt_grp ON receipt (grp, ord);
-- The stored files themselves, keyed by the SHA-256 of the stored content. Written
-- once and never changed, so audit entries and undo refer to them by id instead of
-- carrying their bytes.
CREATE TABLE receipt_file (id TEXT PRIMARY KEY, content BLOB NOT NULL, thumbnail BLOB);
"#,
    },
    Migration {
        version: 15,
        name: "information returns (Form1099Keep)",
        fix: None,
        sql: r#"
-- Information returns kept by Form1099Keep (form1099keep-tables.md). MoneyKeep itself
-- does not read these tables. Payers and recipients hold taxpayer id numbers, which
-- Form1099Keep writes only while the book is password protected.
CREATE TABLE f1099_payer (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE TABLE f1099_recipient (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
-- One row per form (grp = tax year).
CREATE TABLE f1099_form (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE INDEX f1099_form_grp ON f1099_form (grp, ord);
"#,
    },
    Migration {
        version: 16,
        name: "account tax lines and roles",
        fix: None,
        sql: r#"
-- chart-presets.md: an optional tax-line hint per account, and the program's role for
-- an account (at most one account per role).
ALTER TABLE account ADD COLUMN tax_line TEXT;
ALTER TABLE account ADD COLUMN role TEXT;
CREATE UNIQUE INDEX account_role ON account (role) WHERE role IS NOT NULL;
UPDATE account SET role = 'opening_balances'
 WHERE id = (SELECT opening_account_id FROM book LIMIT 1);
"#,
    },
    Migration {
        version: 17,
        name: "mileage: vehicles, trips, saved trips and standard rates",
        fix: None,
        sql: r#"
-- Mileage tracking (mileage.md). Vehicles with their odometer readings per tax year.
CREATE TABLE vehicle (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
-- Trips, ordered by date.
CREATE TABLE trip (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE INDEX trip_ord ON trip (ord);
-- Named routes that fill the trip form.
CREATE TABLE saved_trip (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
-- The dated standard rate table (grp = purpose kind). The shipped rows are the IRS
-- standard rates of 2025 and 2026 (mileage.md Sources 1-2); the user adds, edits and
-- deletes rows like any other record.
CREATE TABLE mileage_rate (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
INSERT INTO mileage_rate (id, grp, ord, json) VALUES
 ('rate-business-2025-01-01', 'business', 20250101, '{"id":"rate-business-2025-01-01","kind":"business","from":"2025-01-01","cents":"70.0"}'),
 ('rate-medical-2025-01-01', 'medical', 20250101, '{"id":"rate-medical-2025-01-01","kind":"medical","from":"2025-01-01","cents":"21.0"}'),
 ('rate-moving-2025-01-01', 'moving', 20250101, '{"id":"rate-moving-2025-01-01","kind":"moving","from":"2025-01-01","cents":"21.0"}'),
 ('rate-charity-2025-01-01', 'charity', 20250101, '{"id":"rate-charity-2025-01-01","kind":"charity","from":"2025-01-01","cents":"14.0"}'),
 ('rate-business-2026-01-01', 'business', 20260101, '{"id":"rate-business-2026-01-01","kind":"business","from":"2026-01-01","cents":"72.5"}'),
 ('rate-medical-2026-01-01', 'medical', 20260101, '{"id":"rate-medical-2026-01-01","kind":"medical","from":"2026-01-01","cents":"20.5"}'),
 ('rate-moving-2026-01-01', 'moving', 20260101, '{"id":"rate-moving-2026-01-01","kind":"moving","from":"2026-01-01","cents":"20.5"}'),
 ('rate-charity-2026-01-01', 'charity', 20260101, '{"id":"rate-charity-2026-01-01","kind":"charity","from":"2026-01-01","cents":"14.0"}'),
 ('rate-business-2026-07-01', 'business', 20260701, '{"id":"rate-business-2026-07-01","kind":"business","from":"2026-07-01","cents":"76.0"}'),
 ('rate-medical-2026-07-01', 'medical', 20260701, '{"id":"rate-medical-2026-07-01","kind":"medical","from":"2026-07-01","cents":"23.5"}'),
 ('rate-moving-2026-07-01', 'moving', 20260701, '{"id":"rate-moving-2026-07-01","kind":"moving","from":"2026-07-01","cents":"23.5"}'),
 ('rate-charity-2026-07-01', 'charity', 20260701, '{"id":"rate-charity-2026-07-01","kind":"charity","from":"2026-07-01","cents":"14.0"}');
"#,
    },
    Migration {
        version: 18,
        name: "opening balances: opening entries, retirement accounts, forgotten feed rows",
        fix: None,
        sql: r#"
-- opening-balances.md: a transaction may be its account's opening entry; a brokerage
-- account may be a retirement account; an account may have started from zero.
ALTER TABLE txn ADD COLUMN opening INTEGER NOT NULL DEFAULT 0;
ALTER TABLE account ADD COLUMN retirement INTEGER NOT NULL DEFAULT 0;
ALTER TABLE account ADD COLUMN opening_none INTEGER NOT NULL DEFAULT 0;
-- Feed rows removed by a statement start (grp = account id): imports skip them as
-- duplicates.
CREATE TABLE import_forgotten (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
CREATE INDEX import_forgotten_grp ON import_forgotten (grp, ord);
"#,
    },
    Migration {
        version: 19,
        name: "subscription audit: subscriptions, recurring bills and suggestion answers",
        fix: None,
        sql: r#"
-- subscriptions.md: subscriptions and recurring bills (their charges are the matching
-- transactions, linked, not copied).
CREATE TABLE subscription (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
-- Answers to suggestions per normalised payee and account (grp = account id).
CREATE TABLE subscription_answer (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
"#,
    },
    Migration {
        version: 20,
        name: "projects: project records and the project tag on splits",
        fix: None,
        sql: r#"
-- projects.md: project records; a split may carry one project and be billable.
CREATE TABLE project (id TEXT PRIMARY KEY, grp TEXT, ord INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL);
ALTER TABLE split ADD COLUMN project_id TEXT;
ALTER TABLE split ADD COLUMN billable INTEGER NOT NULL DEFAULT 0;
"#,
    },
    Migration {
        version: 21,
        name: "split ids; billed project costs linked by split id",
        fix: Some(split_ids_v21),
        sql: r#"
-- release-prep.md §4: each split has its own id, kept when its transaction is edited
-- (lines reordered), so links to a split (billed project costs) survive the edit.
ALTER TABLE split ADD COLUMN split_id TEXT;
CREATE UNIQUE INDEX split_by_id ON split (split_id);
-- The ids of a deleted transaction's splits, so undoing the delete keeps them.
CREATE TABLE split_gone (
    txn_id      TEXT NOT NULL,
    idx         INTEGER NOT NULL,
    split_id    TEXT NOT NULL,
    account_id  TEXT NOT NULL,
    value       TEXT NOT NULL,
    quantity    TEXT NOT NULL,
    memo        TEXT,
    class_id    TEXT,
    location_id TEXT,
    fund_id     TEXT,
    project_id  TEXT,
    billable    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX split_gone_txn ON split_gone (txn_id);
"#,
    },
];

/// Migration v21: give every split an id, and convert invoice lines' links to the
/// project costs they billed from `txn:<transaction>:<line position>` to
/// `split:<split id>` (release-prep.md §4), recording the conversion in the audit
/// log (actor `migration`). A link whose line no longer exists is left as it was.
fn split_ids_v21(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let mut rows: Vec<(String, i64)> = Vec::new();
    {
        let mut st = conn.prepare("SELECT txn_id, idx FROM split WHERE split_id IS NULL")?;
        let r = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        for x in r {
            rows.push(x?);
        }
    }
    for (txn, idx) in rows {
        conn.execute(
            "UPDATE split SET split_id = ?1 WHERE txn_id = ?2 AND idx = ?3",
            rusqlite::params![mk_core::SplitId::new().to_string(), txn, idx],
        )?;
    }
    let mut docs: Vec<(String, String)> = Vec::new();
    {
        let mut st = conn.prepare("SELECT id, json FROM sales_doc")?;
        let r = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        for x in r {
            docs.push(x?);
        }
    }
    let mut converted = 0usize;
    for (id, json) in docs {
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&json) else {
            continue;
        };
        let mut changed = false;
        let lines = v.get_mut("lines").and_then(serde_json::Value::as_array_mut);
        for line in lines.into_iter().flatten() {
            let items = line
                .get_mut("billable_items")
                .and_then(serde_json::Value::as_array_mut);
            for item in items.into_iter().flatten() {
                let Some(key) = item.as_str() else {
                    continue;
                };
                let Some((txn, idx)) = key
                    .strip_prefix("txn:")
                    .and_then(|k| k.rsplit_once(':'))
                    .and_then(|(t, i)| Some((t.to_owned(), i.parse::<i64>().ok()?)))
                else {
                    continue;
                };
                let split: Option<String> = conn
                    .query_row(
                        "SELECT split_id FROM split WHERE txn_id = ?1 AND idx = ?2",
                        rusqlite::params![txn, idx],
                        |r| r.get(0),
                    )
                    .ok();
                if let Some(split) = split {
                    *item = serde_json::Value::String(format!("split:{split}"));
                    changed = true;
                    converted += 1;
                }
            }
        }
        if changed {
            conn.execute(
                "UPDATE sales_doc SET json = ?1 WHERE id = ?2",
                rusqlite::params![v.to_string(), id],
            )?;
        }
    }
    if converted > 0 {
        let at = crate::now_rfc3339().unwrap_or_default();
        conn.execute(
            "INSERT INTO audit_log (at, actor, command, args_json, result_json) VALUES (?1, 'migration', 'migration.billed_links', ?2, ?3)",
            rusqlite::params![
                at,
                serde_json::json!({"migration": 21}).to_string(),
                serde_json::json!({"converted": converted}).to_string()
            ],
        )?;
    }
    Ok(())
}

/// One text value needing input-safety.md R3 cleaning (control, bidirectional or
/// invisible characters); other text is left exactly as it was.
fn cleaned(text: &str) -> Option<String> {
    let c = mk_core::text::clean(text, usize::MAX);
    (c.controls || c.invisible).then_some(c.text)
}

/// Migration v8: clean payees, memos and security names imported before the
/// input-safety rules, and record what changed in the audit log (actor `migration`).
fn clean_text_v8(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let mut changes: Vec<serde_json::Value> = Vec::new();
    let mut txns: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    {
        let mut st = conn.prepare("SELECT id, payee, memo FROM txn")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        for r in rows {
            txns.push(r?);
        }
    }
    for (id, payee, memo) in txns {
        for (field, value) in [("payee", payee), ("memo", memo)] {
            if let Some(new) = value.as_deref().and_then(cleaned) {
                conn.execute(
                    &format!("UPDATE txn SET {field} = ?1 WHERE id = ?2"),
                    rusqlite::params![new, id],
                )?;
                changes.push(serde_json::json!({"table": "txn", "id": id, "field": field}));
            }
        }
    }
    let mut splits: Vec<(String, i64, String)> = Vec::new();
    {
        let mut st = conn.prepare("SELECT txn_id, idx, memo FROM split WHERE memo IS NOT NULL")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        for r in rows {
            splits.push(r?);
        }
    }
    for (txn, idx, memo) in splits {
        if let Some(new) = cleaned(&memo) {
            conn.execute(
                "UPDATE split SET memo = ?1 WHERE txn_id = ?2 AND idx = ?3",
                rusqlite::params![new, txn, idx],
            )?;
            changes.push(serde_json::json!({"table": "split", "id": format!("{txn}:{idx}"), "field": "memo"}));
        }
    }
    let mut secs: Vec<(String, String)> = Vec::new();
    {
        let mut st = conn.prepare("SELECT id, json FROM security")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        for r in rows {
            secs.push(r?);
        }
    }
    for (id, json) in secs {
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&json) else {
            continue;
        };
        let Some(new) = v.get("name").and_then(|n| n.as_str()).and_then(cleaned) else {
            continue;
        };
        if let Some(o) = v.as_object_mut() {
            o.insert("name".into(), serde_json::Value::String(new));
        }
        conn.execute(
            "UPDATE security SET json = ?1 WHERE id = ?2",
            rusqlite::params![v.to_string(), id],
        )?;
        changes.push(serde_json::json!({"table": "security", "id": id, "field": "name"}));
    }
    if !changes.is_empty() {
        let at = crate::now_rfc3339().unwrap_or_default();
        let n = changes.len();
        conn.execute(
            "INSERT INTO audit_log (at, actor, command, args_json, result_json) VALUES (?1, 'migration', 'migration.clean_text', ?2, ?3)",
            rusqlite::params![
                at,
                serde_json::json!({"migration": 8, "cleaned": changes}).to_string(),
                serde_json::json!({"cleaned": n}).to_string()
            ],
        )?;
    }
    Ok(())
}

/// Latest schema version this build understands.
pub(crate) fn latest_version() -> i64 {
    MIGRATIONS.last().map_or(0, |m| m.version)
}
