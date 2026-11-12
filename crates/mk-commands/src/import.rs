//! Import pipeline commands (spec: import-pipeline.md): stage → resolve → classify →
//! review → commit. Parsing is in `mk-import`; this module owns staging documents,
//! classification against the ledger, and commit.

use std::collections::{HashMap, HashSet};

use base64::Engine;
use mk_core::{
    Account, AccountId, AccountType, CommodityCode, Date, Decimal, Reconcile, Split, Transaction,
    TransactionId, TxnStatus,
};
use mk_import::{
    ClearedFlag, ClosingBalance, CsvParser, CsvProfile, DateOrder, MAX_FILE_BYTES, MAX_ROWS,
    Parser, QifOptions, QifParser, RawRow, RuleRow, Statement, content_fingerprint, detect_profile,
    evaluate_rules, external_fingerprint, normalise_payee, occurrences, split_amount,
    suggest_profile_with,
};
use mk_ledger::sort_splits;
use mk_store::{Doc, DocTable, now_rfc3339};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::accounts::Chart;
use crate::profiles::load_profiles;
use crate::rules::{load_rules, rule_from_doc};
use crate::txns::validate_change;
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) const UNCAT_EXPENSE: &str = "Uncategorized Expense";
pub(crate) const UNCAT_INCOME: &str = "Uncategorized Income";
const WINDOW_KEY: &str = "import.match_window_days";
/// Core setting: `transaction` (default) or `posted` (import-pipeline.md revision 2).
pub(crate) const DATE_SOURCE_KEY: &str = "import.date_source";

/// Whether imports should always take the posting date (`import.date_source` =
/// `posted`).
pub(crate) fn posted_dates(ctx: &Ctx<'_>) -> CommandResult<bool> {
    Ok(ctx
        .tx
        .get_setting("core", DATE_SOURCE_KEY)?
        .is_some_and(|v| v.as_str() == Some("posted")))
}

/// The OFX reader with the book's date choice.
pub(crate) fn ofx_parser(ctx: &Ctx<'_>) -> CommandResult<mk_import::OfxParser> {
    Ok(mk_import::OfxParser {
        posted_dates: posted_dates(ctx)?,
    })
}

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "import.stage",
        "Parse a bank file (path or inline content) into a staged batch with proposals",
        true,
        stage,
    );
    r.add("import.list", "List import batches", false, list);
    r.add(
        "import.get",
        "Get one import batch with its rows and proposals",
        false,
        get,
    );
    r.add(
        "import.map_account",
        "Resolve a batch section to an account, optionally remembering the mapping",
        true,
        map_account,
    );
    r.add(
        "import.set_row",
        "Override one staged row: action, matched transaction, other-side account, payee, memo",
        true,
        set_row,
    );
    r.add(
        "import.commit",
        "Apply a staged batch: create and match transactions in one undoable command",
        true,
        commit,
    );
    r.add(
        "import.discard",
        "Discard a staged batch; nothing in the ledger changes",
        true,
        discard,
    );
}

// ----- documents -------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SectionDoc {
    pub index: usize,
    /// Source key for mapping (`qif:Name`, `csv:Profile`, `ofx:<hash>`).
    pub account_key: Option<String>,
    pub account_name: Option<String>,
    /// Masked source account number (last four characters), never the full number.
    #[serde(default)]
    pub account_hint: Option<String>,
    pub currency: Option<CommodityCode>,
    pub closing: Option<ClosingBalance>,
    /// Available balance (information only; never used for reconciliation).
    #[serde(default)]
    pub available: Option<ClosingBalance>,
    #[serde(default)]
    pub period: Option<(Date, Date)>,
    pub account: Option<AccountId>,
    pub statement: Option<String>,
    /// Investment sections (import-investments.md).
    #[serde(default)]
    pub kind: mk_import::SectionKind,
    #[serde(default)]
    pub securities: Vec<crate::inv_import::SecRes>,
    #[serde(default)]
    pub positions: Vec<mk_import::Position>,
    /// The provider's positions could not be read (`mk_import::Section`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub positions_unread: bool,
    #[serde(default)]
    pub cash_balance: Option<ClosingBalance>,
    #[serde(default)]
    pub as_of: Option<Date>,
    /// The investment profile that read the file.
    #[serde(default)]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BatchDoc {
    pub id: String,
    pub format: String,
    pub status: String,
    pub created_at: String,
    pub source: Option<String>,
    pub digest: String,
    pub actor: String,
    pub warnings: Vec<String>,
    pub sections: Vec<SectionDoc>,
    /// Digest of the reviewed proposal (sections and every staged row), set by each
    /// review command and re-checked at commit (input-safety.md R4).
    #[serde(default)]
    pub proposal: Option<String>,
    /// IIF: account, class and name texts of the file with the user's mapping.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<crate::iif::NameMap>,
    /// IIF: list records offered for creation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lists: Vec<mk_import::ListItem>,
    /// Bank feeds: the connection that staged the batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed: Option<String>,
}

/// The other side of a proposed split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub(crate) enum Other {
    Account(AccountId),
    UncategorizedExpense,
    UncategorizedIncome,
}

impl Other {
    fn uncategorized_for(value: Decimal) -> Self {
        // A positive (debit) other side is spending; negative (credit) is income.
        if value >= Decimal::ZERO {
            Other::UncategorizedExpense
        } else {
            Other::UncategorizedIncome
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProposedSplit {
    pub account: Other,
    /// Value of the other-side split (the negation of the row's share).
    pub value: Decimal,
    pub memo: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Class {
    /// Section not resolved yet.
    Pending,
    Duplicate,
    Match,
    New,
    /// OFX correction that replaces a committed transaction.
    Correction,
    /// OFX correction that deletes (voids) a committed transaction.
    CorrectionDelete,
    /// A duplicate whose only change is a corrected (transaction) date
    /// (import-pipeline.md revision 2).
    DateCorrection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RowAction {
    Create,
    Match,
    Skip,
}

/// How a match row attaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Link {
    /// An existing split in the target account (attach the fingerprint).
    Split { txn: TransactionId, split: usize },
    /// A row created earlier in this batch (attach to its split in our account).
    Row { row: String },
    /// An existing imported transfer counterpart: replace that transaction's
    /// categorisation split with a split in our account.
    Transfer { txn: TransactionId, split: usize },
    /// OFX REPLACE correction of the committed transaction carrying the old FITID.
    Correct { txn: TransactionId, split: usize },
    /// OFX DELETE correction: void that transaction.
    Void { txn: TransactionId },
    /// Date correction: move that transaction from `from` to the row's date.
    Redate { txn: TransactionId, from: Date },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Overrides {
    pub action: Option<RowAction>,
    pub match_txn: Option<TransactionId>,
    /// The user's other-side account (None = use the automatic proposal).
    pub other: Option<AccountId>,
    pub payee: Option<String>,
    pub memo: Option<String>,
    /// Processor rows naming open invoices: apply the payments (true) or post sales
    /// (false); unset until the user decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invoice: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RowDoc {
    pub id: String,
    pub batch: String,
    pub section: usize,
    pub index: usize,
    pub raw: RawRow,
    pub fingerprint: Option<String>,
    pub class: Class,
    pub action: RowAction,
    pub link: Option<Link>,
    pub other: Vec<ProposedSplit>,
    pub other_source: Option<String>,
    pub payee: String,
    pub memo: String,
    pub needs_review: bool,
    pub rule_ids: Vec<String>,
    pub warnings: Vec<String>,
    pub overrides: Overrides,
    pub result: Option<Value>,
    /// Class, location and fund from rules (or the file), as record ids.
    #[serde(default, skip_serializing_if = "mk_core::SplitTags::is_empty")]
    pub tags: mk_core::SplitTags,
}

fn to_doc<T: Serialize>(id: &str, grp: Option<&str>, ord: i64, v: &T) -> CommandResult<Doc> {
    Ok(Doc {
        id: id.to_owned(),
        grp: grp.map(str::to_owned),
        ord,
        json: serde_json::to_value(v)?,
    })
}

fn from_doc<T: serde::de::DeserializeOwned>(d: &Doc) -> CommandResult<T> {
    serde_json::from_value(d.json.clone())
        .map_err(|e| CommandError::Integrity(format!("bad stored document {}: {e}", d.id)))
}

pub(crate) fn load_batch(ctx: &Ctx<'_>, id: &str) -> CommandResult<BatchDoc> {
    let d = ctx
        .tx
        .get_doc(DocTable::ImportBatch, id)?
        .ok_or_else(|| CommandError::NotFound(format!("import batch {id}")))?;
    from_doc(&d)
}

fn batch_ord(ctx: &Ctx<'_>, id: &str) -> CommandResult<i64> {
    Ok(ctx
        .tx
        .get_doc(DocTable::ImportBatch, id)?
        .map_or(0, |d| d.ord))
}

pub(crate) fn save_batch(ctx: &Ctx<'_>, b: &BatchDoc) -> CommandResult<()> {
    let ord = batch_ord(ctx, &b.id)?;
    ctx.save_doc(
        DocTable::ImportBatch,
        &b.id,
        Some(&to_doc(&b.id, None, ord, b)?),
    )
}

pub(crate) fn load_rows(ctx: &Ctx<'_>, batch: &str) -> CommandResult<Vec<RowDoc>> {
    ctx.tx
        .docs(DocTable::ImportRow, Some(batch))?
        .iter()
        .map(from_doc)
        .collect()
}

fn row_ord(r: &RowDoc) -> i64 {
    i64::try_from(r.section * 1_000_000 + r.index).unwrap_or(i64::MAX)
}

pub(crate) fn save_row(ctx: &Ctx<'_>, r: &RowDoc) -> CommandResult<()> {
    let doc = to_doc(&r.id, Some(&r.batch), row_ord(r), r)?;
    if ctx.tx.get_doc(DocTable::ImportRow, &r.id)?.as_ref() == Some(&doc) {
        return Ok(());
    }
    ctx.save_doc(DocTable::ImportRow, &r.id, Some(&doc))
}

// ----- stage -----------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StageArgs {
    /// `qif`, `csv` or `ofx` (OFX, QFX and QBO files). Inferred from the file name when
    /// omitted.
    #[serde(default)]
    pub format: Option<String>,
    /// Path of the file to read (front ends that can read files only).
    #[serde(default)]
    pub path: Option<String>,
    /// File content as text.
    #[serde(default)]
    pub content: Option<String>,
    /// File content, base64-encoded (for files that are not UTF-8).
    #[serde(default)]
    pub content_base64: Option<String>,
    /// File name for display and format inference.
    #[serde(default)]
    pub name: Option<String>,
    /// Target account for sections without a mapping (id, full name or code).
    #[serde(default)]
    pub account: Option<String>,
    /// QIF: day/month order (`mdy` default, or `dmy`).
    #[serde(default)]
    pub date_order: Option<DateOrder>,
    /// QIF: `,` is the decimal separator.
    #[serde(default)]
    pub decimal_comma: bool,
    /// CSV: profile name (auto-detected when omitted).
    #[serde(default)]
    pub profile: Option<String>,
}

fn infer_format(name: Option<&str>) -> Option<&'static str> {
    let lower = name?.to_lowercase();
    let ext = lower.rsplit('.').next()?;
    match ext {
        "qif" => Some("qif"),
        "iif" => Some("iif"),
        "csv" => Some("csv"),
        "ofx" | "qfx" | "qbo" => Some("ofx"),
        "json" => Some("json"),
        "xml" => Some("xml"),
        _ => None,
    }
}

fn read_input(ctx: &Ctx<'_>, a: &StageArgs) -> CommandResult<Vec<u8>> {
    let given = [
        a.path.is_some(),
        a.content.is_some(),
        a.content_base64.is_some(),
    ]
    .iter()
    .filter(|x| **x)
    .count();
    if given != 1 {
        return Err(CommandError::InvalidArgs(
            "give exactly one of path, content or content_base64".into(),
        ));
    }
    if let Some(c) = &a.content {
        return Ok(c.as_bytes().to_vec());
    }
    if let Some(b) = &a.content_base64 {
        return base64::engine::general_purpose::STANDARD
            .decode(b.trim())
            .map_err(|e| {
                CommandError::InvalidArgs(format!("content_base64 is not valid base64: {e}"))
            });
    }
    let path = a.path.as_deref().unwrap_or_default();
    let reader = ctx.files.ok_or_else(|| {
        CommandError::InvalidArgs(
            "this front end cannot read files; send the file as content or content_base64".into(),
        )
    })?;
    reader
        .read(path)
        .map_err(|e| CommandError::InvalidArgs(format!("cannot read {path}: {e}")))
}

/// The parser for a format, or the CSV `needs_profile` answer.
enum Parsed {
    Statement(Statement, Option<CsvProfile>),
    /// Read with an investment profile (its name).
    Broker(Statement, String),
    NeedsProfile(Value),
}

fn parse_input(ctx: &Ctx<'_>, a: &StageArgs, format: &str, bytes: &[u8]) -> CommandResult<Parsed> {
    let invalid = |e: mk_import::ImportError| CommandError::InvalidArgs(e.to_string());
    match format {
        "qif" => {
            let p = QifParser {
                options: QifOptions {
                    date_order: a.date_order.unwrap_or_default(),
                    decimal_comma: a.decimal_comma,
                },
            };
            Ok(Parsed::Statement(p.parse(bytes).map_err(invalid)?, None))
        }
        "iif" => {
            let p = mk_import::IifParser {
                date_order: a.date_order.unwrap_or_default(),
            };
            Ok(Parsed::Statement(p.parse(bytes).map_err(invalid)?, None))
        }
        "json" | "xml" => match crate::inv_import::parse_broker_file(
            ctx,
            bytes,
            format,
            a.profile.as_deref(),
            a.name.as_deref(),
        )? {
            crate::inv_import::ProfileParse::Parsed(st, name) => Ok(Parsed::Broker(st, name)),
            crate::inv_import::ProfileParse::NeedsProfile(v) => Ok(Parsed::NeedsProfile(v)),
            crate::inv_import::ProfileParse::NotInvestment => Err(CommandError::InvalidArgs(
                "no investment profile reads this file".into(),
            )),
        },
        "csv" => {
            // A broker CSV file matching an investment profile.
            let named_bank = a.profile.as_deref().is_some_and(|n| {
                load_profiles(ctx)
                    .map(|ps| ps.iter().any(|p| p.name.eq_ignore_ascii_case(n.trim())))
                    .unwrap_or(false)
            });
            if !named_bank
                && let crate::inv_import::ProfileParse::Parsed(st, name) =
                    crate::inv_import::parse_broker_file(
                        ctx,
                        bytes,
                        format,
                        a.profile.as_deref(),
                        a.name.as_deref(),
                    )?
            {
                return Ok(Parsed::Broker(st, name));
            }
            let profiles = load_profiles(ctx)?;
            let text = mk_import::decode_text(bytes);
            let profile = match &a.profile {
                Some(name) => profiles
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
                    .cloned()
                    .ok_or_else(|| CommandError::NotFound(format!("CSV profile {name:?}")))?,
                None => match detect_profile(&text, &profiles) {
                    Some(p) => p.clone(),
                    None => {
                        let np =
                            suggest_profile_with(&text, posted_dates(ctx)?).map_err(invalid)?;
                        return Ok(Parsed::NeedsProfile(json!({ "needs_profile": np })));
                    }
                },
            };
            let st = CsvParser {
                profile: profile.clone(),
            }
            .parse(bytes)
            .map_err(invalid)?;
            Ok(Parsed::Statement(st, Some(profile)))
        }
        "ofx" | "qfx" | "qbo" => {
            let st = ofx_parser(ctx)?.parse(bytes).map_err(invalid)?;
            Ok(Parsed::Statement(st, None))
        }
        other => Err(CommandError::InvalidArgs(format!(
            "unsupported import format {other:?} (qif, csv, ofx, json or xml)"
        ))),
    }
}

fn stage(ctx: &Ctx<'_>, mut a: StageArgs) -> CommandResult<Value> {
    let bytes = read_input(ctx, &a)?;
    let name = a.name.clone().or_else(|| {
        a.path
            .as_ref()
            .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p).to_owned())
    });
    // The parsers see the path's file name exactly as a given name (revision 3).
    a.name.clone_from(&name);
    let digest = hex(&Sha256::digest(&bytes));
    // The audit log records what was imported by digest, never the file content.
    *ctx.audit_args.borrow_mut() = Some(json!({
        "format": a.format, "name": name, "account": a.account, "profile": a.profile,
        "bytes": bytes.len(), "sha256": digest,
    }));
    if bytes.len() > MAX_FILE_BYTES {
        return Err(CommandError::InvalidArgs(format!(
            "the file is {} bytes; the limit is 50 MB",
            bytes.len()
        )));
    }
    let format = a
        .format
        .clone()
        .or_else(|| infer_format(name.as_deref()).map(str::to_owned))
        .ok_or_else(|| CommandError::InvalidArgs("give format (qif, csv or ofx)".into()))?
        .to_lowercase();
    let format = if matches!(format.as_str(), "qfx" | "qbo") {
        "ofx".to_owned()
    } else {
        format
    };
    let (statement, profile, broker) = match parse_input(ctx, &a, &format, &bytes)? {
        Parsed::NeedsProfile(v) => return Ok(v),
        Parsed::Statement(s, p) => (s, p, None),
        Parsed::Broker(s, n) => (s, None, Some(n)),
    };
    stage_statement_with(
        ctx,
        statement,
        &format,
        name,
        digest,
        a.account.as_deref(),
        profile.as_ref(),
        broker.as_deref(),
    )
}

/// Stage a parsed statement (tests).
#[cfg(test)]
pub(crate) fn stage_statement(
    ctx: &Ctx<'_>,
    statement: Statement,
    format: &str,
    source: Option<String>,
    digest: String,
    account: Option<&str>,
    profile: Option<&CsvProfile>,
) -> CommandResult<Value> {
    stage_statement_with(
        ctx, statement, format, source, digest, account, profile, None,
    )
}

/// Stage a parsed statement. Shared by every format.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stage_statement_with(
    ctx: &Ctx<'_>,
    mut statement: Statement,
    format: &str,
    source: Option<String>,
    digest: String,
    account: Option<&str>,
    profile: Option<&CsvProfile>,
    broker: Option<&str>,
) -> CommandResult<Value> {
    mk_import::safety::sanitize_statement(&mut statement);
    crate::inv_import::prepare_statement(ctx, &mut statement, format)?;
    if statement.row_count() > MAX_ROWS {
        return Err(CommandError::InvalidArgs(format!(
            "the file has {} rows; a batch holds at most {MAX_ROWS}",
            statement.row_count()
        )));
    }
    let chart = Chart::load(ctx)?;
    let explicit = match account.or(profile.and_then(|p| p.account.as_deref())) {
        Some(r) => Some(chart.resolve(r)?.id),
        None => None,
    };
    let id = mk_core::Id::<()>::new().to_string();
    let mut sections = Vec::new();
    for (i, s) in statement.sections.iter_mut().enumerate() {
        if s.account_key.is_none()
            && let Some(p) = profile
        {
            s.account_key = Some(format!("csv:{}", p.name));
        }
        let mapped = match &s.account_key {
            Some(k) => mapped_account(ctx, k)?,
            None => None,
        };
        let account = mapped.or(explicit);
        sections.push(SectionDoc {
            index: i,
            account_key: s.account_key.clone(),
            account_name: s.account_name.clone(),
            account_hint: s.account_hint.clone(),
            currency: s.currency.clone(),
            closing: s.closing.clone(),
            available: s.available.clone(),
            period: s.period,
            account: None,
            statement: None,
            kind: mk_import::SectionKind::Bank,
            securities: Vec::new(),
            positions: Vec::new(),
            positions_unread: false,
            cash_balance: None,
            as_of: None,
            profile: None,
        });
        if let Some(sec) = sections.last_mut() {
            crate::inv_import::fill_section(sec, s, broker);
        }
        if let (Some(acc), Some(sec)) = (account, sections.last_mut()) {
            check_section_account(&chart, sec, acc)?;
            sec.account = Some(acc);
        }
    }
    let n = ctx.tx.docs(DocTable::ImportBatch, None)?.len();
    let mut batch = BatchDoc {
        id: id.clone(),
        format: format.to_owned(),
        status: "staged".into(),
        created_at: now_rfc3339()?,
        source,
        digest,
        actor: ctx.actor.0.clone(),
        warnings: statement.warnings.clone(),
        sections,
        proposal: None,
        names: Vec::new(),
        lists: Vec::new(),
        feed: None,
    };
    if format == "iif" {
        crate::iif::init_names(ctx, &mut batch, &statement)?;
    }
    let ord = i64::try_from(n + 1).unwrap_or(i64::MAX);
    ctx.save_doc(
        DocTable::ImportBatch,
        &id,
        Some(&to_doc(&id, None, ord, &batch)?),
    )?;
    for (si, s) in statement.sections.iter().enumerate() {
        for (ri, raw) in s.rows.iter().enumerate() {
            let row = RowDoc {
                id: mk_core::Id::<()>::new().to_string(),
                batch: id.clone(),
                section: si,
                index: ri,
                raw: raw.clone(),
                fingerprint: None,
                class: Class::Pending,
                action: RowAction::Skip,
                link: None,
                other: Vec::new(),
                other_source: None,
                payee: raw.payee.clone(),
                memo: raw.memo.clone(),
                needs_review: false,
                rule_ids: Vec::new(),
                tags: mk_core::SplitTags::default(),
                warnings: Vec::new(),
                overrides: Overrides::default(),
                result: None,
            };
            save_row(ctx, &row)?;
        }
        if s.kind == mk_import::SectionKind::Investment {
            crate::inv_import::stage_rows(ctx, &id, si, s)?;
        }
    }
    capture_statements(ctx, &mut batch)?;
    save_batch(ctx, &batch)?;
    classify(ctx, &batch)?;
    crate::inv_import::classify(ctx, &mut batch)?;
    sealed_json(ctx, &batch.id)
}

pub(crate) fn check_section_account(
    chart: &Chart,
    sec: &SectionDoc,
    account: AccountId,
) -> CommandResult<()> {
    let acc = chart
        .get(account)
        .ok_or_else(|| CommandError::NotFound("account".into()))?;
    crate::inv_import::check_account(chart, sec, account)?;
    if acc.placeholder || acc.archived {
        return Err(CommandError::Rejected(format!(
            "{} is a placeholder or archived account and cannot receive imports",
            chart.full_name(acc.id)
        )));
    }
    if let Some(cur) = &sec.currency
        && *cur != acc.commodity
    {
        return Err(CommandError::Rejected(format!(
            "section {} is in {cur}, but {} is in {}; pick an account in {cur}",
            sec.index,
            chart.full_name(acc.id),
            acc.commodity
        )));
    }
    Ok(())
}

fn mapped_account(ctx: &Ctx<'_>, key: &str) -> CommandResult<Option<AccountId>> {
    let Some(d) = ctx.tx.get_doc(DocTable::ImportAccountMap, key)? else {
        return Ok(None);
    };
    let id = d
        .json
        .get("account")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<AccountId>().ok());
    // A mapping to a deleted account is ignored.
    Ok(match id {
        Some(id) if ctx.tx.account(id)?.is_some() => Some(id),
        _ => None,
    })
}

/// Record a statement for each resolved section with a closing balance.
fn capture_statements(ctx: &Ctx<'_>, batch: &mut BatchDoc) -> CommandResult<()> {
    for sec in &mut batch.sections {
        let (Some(acc), Some(closing), None) = (sec.account, &sec.closing, &sec.statement) else {
            continue;
        };
        let sid = mk_core::Id::<()>::new().to_string();
        let doc = json!({
            "id": sid, "account": acc, "date": closing.date, "balance": closing.amount,
            "batch": batch.id, "section": sec.index,
        });
        let ord = i64::from(closing.date.year()) * 10_000
            + i64::from(closing.date.month()) * 100
            + i64::from(closing.date.day());
        ctx.save_doc(
            DocTable::Statement,
            &sid,
            Some(&Doc {
                id: sid.clone(),
                grp: Some(acc.to_string()),
                ord,
                json: doc,
            }),
        )?;
        sec.statement = Some(sid);
    }
    Ok(())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ----- classification ---------------------------------------------------------

pub(crate) fn match_window(ctx: &Ctx<'_>) -> CommandResult<i64> {
    Ok(ctx
        .tx
        .get_setting("core", WINDOW_KEY)?
        .and_then(|v| v.as_i64())
        .unwrap_or(3))
}

fn is_transfer_account(a: &Account) -> bool {
    matches!(a.account_type, AccountType::Asset | AccountType::Liability)
}

fn usable(chart: &Chart, id: AccountId, target: AccountId) -> bool {
    id != target && chart.get(id).is_some_and(|a| !a.placeholder && !a.archived)
}

/// Resolve a source category hint to an account.
fn resolve_hint(
    ctx: &Ctx<'_>,
    chart: &Chart,
    hint: &str,
    target: AccountId,
    section_name: Option<&str>,
    payee: &str,
) -> CommandResult<Option<AccountId>> {
    let h = hint.trim();
    if h.is_empty() {
        return Ok(None);
    }
    let bracket = h.strip_prefix('[').and_then(|x| x.strip_suffix(']'));
    let name = bracket.unwrap_or(h).trim().to_lowercase();
    if bracket.is_some() && normalise_payee(payee) == "OPENING BALANCE" {
        let own = section_name.is_some_and(|s| s.trim().to_lowercase() == name)
            || chart.full_name(target).to_lowercase() == name
            || chart
                .get(target)
                .is_some_and(|a| a.name.to_lowercase() == name);
        if own {
            return ctx.tx.opening_account_id().map_err(Into::into);
        }
    }
    let by_full: Vec<&Account> = chart
        .accounts
        .iter()
        .filter(|a| chart.full_name(a.id).to_lowercase() == name)
        .collect();
    let found = match by_full.as_slice() {
        [one] => Some(one.id),
        _ if bracket.is_some() => {
            let by_leaf: Vec<&Account> = chart
                .accounts
                .iter()
                .filter(|a| a.name.to_lowercase() == name)
                .collect();
            match by_leaf.as_slice() {
                [one] => Some(one.id),
                _ => None,
            }
        }
        _ => None,
    };
    Ok(found.filter(|id| usable(chart, *id, target)))
}

struct Env<'c> {
    chart: Chart,
    rules: Vec<mk_import::Rule>,
    window: i64,
    ctx: &'c Ctx<'c>,
    /// IIF names mapped in review: lower-cased account text → account.
    account_map: HashMap<String, AccountId>,
    /// IIF class text (lower-cased) → class id.
    class_map: HashMap<String, String>,
    /// Projects (a rule's set_project brings the project's fund and billable flag).
    projects: Vec<crate::projects::Project>,
    funds_on: bool,
    /// Fingerprint → the date its transaction was given when last imported (built on
    /// first use).
    imported: std::cell::RefCell<Option<HashMap<String, Date>>>,
}

impl Env<'_> {
    /// The date the transaction carrying `fp` was given by the import that created,
    /// matched or last corrected it (import-pipeline.md revision 2, rule 1).
    fn imported_date(&self, fp: &str) -> CommandResult<Option<Date>> {
        if self.imported.borrow().is_none() {
            let map = imported_dates(self.ctx)?;
            *self.imported.borrow_mut() = Some(map);
        }
        Ok(self
            .imported
            .borrow()
            .as_ref()
            .and_then(|m| m.get(fp).copied()))
    }
}

/// The earliest date imported by a bank feed's committed batches (the start of the
/// desktop's "Fix dates of imported transactions…", connectivity.md revision 3).
pub(crate) fn earliest_imported(ctx: &Ctx<'_>, feed: &str) -> CommandResult<Option<Date>> {
    let mut earliest: Option<Date> = None;
    for d in ctx.tx.docs(DocTable::ImportBatch, None)? {
        let b: BatchDoc = from_doc(&d)?;
        if b.status != "committed" || b.feed.as_deref() != Some(feed) {
            continue;
        }
        for r in load_rows(ctx, &b.id)? {
            if r.result
                .as_ref()
                .is_some_and(|v| v.get("skipped").is_none())
            {
                earliest = Some(earliest.map_or(r.raw.date, |e| e.min(r.raw.date)));
            }
        }
    }
    Ok(earliest)
}

/// Fingerprint → imported date, from the rows of committed batches in batch order
/// (later imports win).
fn imported_dates(ctx: &Ctx<'_>) -> CommandResult<HashMap<String, Date>> {
    let mut map = HashMap::new();
    for d in ctx.tx.docs(DocTable::ImportBatch, None)? {
        let b: BatchDoc = from_doc(&d)?;
        if b.status != "committed" {
            continue;
        }
        let mut rows = load_rows(ctx, &b.id)?;
        rows.sort_by_key(row_ord);
        for r in rows {
            let applied = r.result.as_ref().is_some_and(|v| {
                [
                    "created",
                    "matched",
                    "linked",
                    "corrected",
                    "date_corrected",
                ]
                .iter()
                .any(|k| v.get(*k).is_some())
            });
            if let (true, Some(fp)) = (applied, r.fingerprint) {
                map.insert(fp, r.raw.date);
            }
        }
    }
    Ok(map)
}

/// Recompute fingerprints, classification and proposals for every row of a batch.
pub(crate) fn classify(ctx: &Ctx<'_>, batch: &BatchDoc) -> CommandResult<()> {
    let env = Env {
        chart: Chart::load(ctx)?,
        rules: load_rules(ctx)?
            .iter()
            .map(rule_from_doc)
            .collect::<CommandResult<_>>()?,
        window: match_window(ctx)?,
        ctx,
        account_map: crate::iif::account_map(batch),
        class_map: crate::iif::class_map(batch),
        projects: crate::projects::all(ctx)?,
        funds_on: crate::is_module_enabled(ctx.tx, crate::dimensions::FUNDS)?,
        imported: std::cell::RefCell::new(None),
    };
    let mut rows = load_rows(ctx, &batch.id)?;
    rows.sort_by_key(row_ord);
    // Fingerprints, per section in file order.
    for sec in &batch.sections {
        let in_sec: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.section == sec.index)
            .map(|(i, _)| i)
            .collect();
        let occ = occurrences(in_sec.iter().filter_map(|i| rows.get(*i)).map(|r| {
            (
                r.raw.date,
                r.raw.amount,
                r.raw.payee.as_str(),
                r.raw.number.as_str(),
            )
        }));
        for (pos, i) in in_sec.iter().enumerate() {
            let Some(r) = rows.get_mut(*i) else { continue };
            r.fingerprint = sec.account.map(|acc| match &r.raw.external_id {
                Some(x) => external_fingerprint(acc, x),
                None => content_fingerprint(
                    acc,
                    r.raw.date,
                    r.raw.amount,
                    &r.raw.payee,
                    &r.raw.number,
                    occ.get(pos).copied().unwrap_or(1),
                ),
            });
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut used_splits: HashSet<(TransactionId, usize)> = HashSet::new();
    for i in 0..rows.len() {
        let Some(row) = rows.get(i).cloned() else {
            continue;
        };
        let target = batch.sections.get(row.section).and_then(|s| s.account);
        let section_name = batch
            .sections
            .get(row.section)
            .and_then(|s| s.account_name.clone());
        let updated = classify_row(
            &env,
            row,
            target,
            section_name.as_deref(),
            &mut seen,
            &mut used_splits,
            &mut rows,
            batch,
        )?;
        if let Some(r) = rows.get_mut(i) {
            *r = updated;
        }
    }
    for r in &rows {
        save_row(ctx, r)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn classify_row(
    env: &Env<'_>,
    mut row: RowDoc,
    target: Option<AccountId>,
    section_name: Option<&str>,
    seen: &mut HashSet<String>,
    used: &mut HashSet<(TransactionId, usize)>,
    rows: &mut [RowDoc],
    batch: &BatchDoc,
) -> CommandResult<RowDoc> {
    let ctx = env.ctx;
    row.link = None;
    row.other.clear();
    row.other_source = None;
    row.needs_review = false;
    row.rule_ids.clear();
    row.warnings.clear();
    row.payee = row
        .overrides
        .payee
        .clone()
        .unwrap_or_else(|| row.raw.payee.clone());
    row.memo = row
        .overrides
        .memo
        .clone()
        .unwrap_or_else(|| row.raw.memo.clone());
    let (Some(target), Some(fp)) = (target, row.fingerprint.clone()) else {
        row.class = Class::Pending;
        row.action = RowAction::Skip;
        return Ok(row);
    };
    // 1. Duplicate (or, revision 2, a date correction).
    if seen.contains(&fp) {
        row.class = Class::Duplicate;
        row.action = RowAction::Skip;
        return Ok(row);
    }
    if ctx.tx.fingerprint_exists(target, &fp)? {
        row.class = Class::Duplicate;
        row.action = RowAction::Skip;
        seen.insert(fp.clone());
        classify_redate(env, &mut row, target, &fp)?;
        return Ok(row);
    }
    // opening-balances.md: a feed row removed by a statement start stays away.
    if crate::opening::forgotten(ctx, target, &fp)? {
        row.class = Class::Duplicate;
        row.action = RowAction::Skip;
        seen.insert(fp.clone());
        return Ok(row);
    }
    seen.insert(fp.clone());
    let amount = row.raw.amount;
    // OFX corrections of a committed FITID.
    if let (Some(old_id), Some(action)) = (
        row.raw.correct_fitid.clone(),
        row.raw.correct_action.clone(),
    ) && let Some(done) = classify_correction(env, &mut row, target, &old_id, &action)?
    {
        return Ok(done);
    }
    // User-chosen match.
    if row.overrides.action == Some(RowAction::Match)
        && let Some(txn) = row.overrides.match_txn
    {
        let split = user_match_split(ctx, txn, target, amount)?;
        used.insert((txn, split));
        row.class = Class::Match;
        row.action = RowAction::Match;
        row.link = Some(Link::Split { txn, split });
        return Ok(row);
    }
    // 2. Match to an existing unfingerprinted split.
    if row.overrides.action.is_none()
        && let Some((txn, split)) = find_match(env, target, &row, used)?
    {
        used.insert((txn, split));
        row.class = Class::Match;
        row.action = RowAction::Match;
        row.link = Some(Link::Split { txn, split });
        return Ok(row);
    }
    // 3. New: propose the other side.
    row.class = Class::New;
    row.action = if amount.is_zero() {
        RowAction::Skip
    } else {
        RowAction::Create
    };
    propose_other(env, &mut row, target, section_name)?;
    if let Some(reason) = row.raw.review.clone() {
        row.needs_review = true;
        row.warnings.push(reason);
    }
    if !row.raw.invoices.is_empty() {
        let numbers: Vec<&str> = row.raw.invoices.iter().map(|i| i.number.as_str()).collect();
        match row.overrides.invoice {
            None => {
                row.needs_review = true;
                row.warnings.push(format!(
                    "Apply to {}? (or use sales instead)",
                    numbers.join(", ")
                ));
            }
            Some(true) => row
                .warnings
                .push(format!("Apply to {}", numbers.join(", "))),
            Some(false) => {}
        }
    }
    if row.overrides.action == Some(RowAction::Skip) {
        row.action = RowAction::Skip;
    } else if row.overrides.action == Some(RowAction::Create) {
        row.action = RowAction::Create;
    }
    if row.overrides.action.is_none() && row.action == RowAction::Create {
        link_transfer(env, &mut row, target, used, rows, batch)?;
    }
    Ok(row)
}

/// Revision 2: a duplicate by external id whose transaction date differs from the
/// committed transaction's becomes a date correction when the transaction still has
/// its imported date, no reconciled split and both dates are after the closing date;
/// otherwise it stays a duplicate with a note.
fn classify_redate(
    env: &Env<'_>,
    row: &mut RowDoc,
    target: AccountId,
    fp: &str,
) -> CommandResult<()> {
    let ctx = env.ctx;
    if row.raw.external_id.is_none() || row.raw.date_kind != mk_import::DateKind::Transaction {
        return Ok(());
    }
    let Some((txn, _)) = ctx.tx.fingerprint_split(target, fp)? else {
        return Ok(());
    };
    let Some(t) = ctx.tx.transaction(txn)? else {
        return Ok(());
    };
    if t.date == row.raw.date || t.status != TxnStatus::Posted {
        return Ok(());
    }
    let closing = ctx.tx.closing_date()?;
    let reason = if env.imported_date(fp)? != Some(t.date) {
        Some("it was edited")
    } else if t
        .splits
        .iter()
        .any(|s| s.reconcile == Reconcile::Reconciled)
    {
        Some("it is reconciled")
    } else if closing.is_some_and(|c| t.date <= c || row.raw.date <= c) {
        Some("the period is closed")
    } else {
        None
    };
    if let Some(why) = reason {
        row.warnings.push(format!(
            "Date differs from the bank ({}); left unchanged because {why}",
            row.raw.date
        ));
        return Ok(());
    }
    row.class = Class::DateCorrection;
    row.action = if row.overrides.action == Some(RowAction::Skip) {
        RowAction::Skip
    } else {
        RowAction::Match
    };
    row.link = Some(Link::Redate { txn, from: t.date });
    Ok(())
}

/// Classify an OFX correction whose earlier FITID may be committed. Returns `None`
/// when the row should be classified normally (REPLACE of an unknown FITID).
fn classify_correction(
    env: &Env<'_>,
    row: &mut RowDoc,
    target: AccountId,
    old_id: &str,
    action: &str,
) -> CommandResult<Option<RowDoc>> {
    let ctx = env.ctx;
    let old_fp = external_fingerprint(target, old_id);
    let found = ctx.tx.fingerprint_split(target, &old_fp)?;
    let Some((txn, split)) = found else {
        if action == "DELETE" {
            row.class = Class::New;
            row.action = RowAction::Skip;
            row.warnings.push(format!(
                "correction deletes FITID {old_id}, which was never imported; ignored"
            ));
            return Ok(Some(row.clone()));
        }
        return Ok(None);
    };
    let t = ctx
        .tx
        .transaction(txn)?
        .ok_or_else(|| CommandError::Integrity("fingerprinted transaction missing".into()))?;
    let reconciled = t
        .splits
        .iter()
        .any(|s| s.reconcile == Reconcile::Reconciled);
    if action == "DELETE" {
        if t.status == TxnStatus::Voided {
            row.class = Class::Duplicate;
            row.action = RowAction::Skip;
            return Ok(Some(row.clone()));
        }
        row.class = Class::CorrectionDelete;
        row.link = Some(Link::Void { txn });
        if reconciled {
            row.needs_review = true;
            row.action = RowAction::Skip;
            row.warnings.push(
                "the transaction to delete has a reconciled split; nothing changes automatically"
                    .into(),
            );
        } else {
            row.action = RowAction::Match;
        }
        return Ok(Some(row.clone()));
    }
    row.class = Class::Correction;
    row.link = Some(Link::Correct { txn, split });
    let amount_changes = t
        .splits
        .get(split)
        .is_some_and(|s| s.quantity != row.raw.amount);
    let other_ok = t.splits.len() == 2
        && t.splits
            .iter()
            .enumerate()
            .all(|(i, s)| i == split || s.value == s.quantity);
    if reconciled || (amount_changes && !other_ok) {
        row.needs_review = true;
        row.action = RowAction::Skip;
        row.warnings.push(if reconciled {
            "the corrected transaction has a reconciled split; nothing changes automatically".into()
        } else {
            "the amount changes on a transaction with more than two splits; nothing changes automatically".into()
        });
    } else {
        row.action = RowAction::Match;
    }
    Ok(Some(row.clone()))
}

fn user_match_split(
    ctx: &Ctx<'_>,
    txn: TransactionId,
    target: AccountId,
    amount: Decimal,
) -> CommandResult<usize> {
    let t = ctx
        .tx
        .transaction(txn)?
        .ok_or_else(|| CommandError::NotFound(format!("transaction {txn}")))?;
    t.splits
        .iter()
        .position(|s| s.account == target && s.quantity == amount && s.fingerprint.is_none())
        .ok_or_else(|| {
            CommandError::Rejected(format!(
                "transaction {txn} has no unmatched split of {amount} in the target account"
            ))
        })
}

fn payee_score(a: &str, b: &str) -> usize {
    let a = normalise_payee(a);
    let b = normalise_payee(b);
    let words: HashSet<&str> = a.split(' ').collect();
    b.split(' ').filter(|w| words.contains(w)).count() + usize::from(a == b) * 100
}

fn find_match(
    env: &Env<'_>,
    target: AccountId,
    row: &RowDoc,
    used: &HashSet<(TransactionId, usize)>,
) -> CommandResult<Option<(TransactionId, usize)>> {
    let w = i32::try_from(env.window).unwrap_or(3);
    let from = row.raw.date.add_days(-w);
    let to = row.raw.date.add_days(w);
    let candidates = env.ctx.tx.account_splits(target, from, to)?;
    let best = candidates
        .iter()
        .filter(|s| {
            s.fingerprint.is_none()
                && s.quantity == row.raw.amount
                && !used.contains(&(s.txn_id, s.index))
        })
        .min_by(|a, b| {
            let da = row.raw.date.days_until(a.date).abs();
            let db = row.raw.date.days_until(b.date).abs();
            da.cmp(&db)
                .then_with(|| {
                    payee_score(&row.raw.payee, b.payee.as_deref().unwrap_or("")).cmp(&payee_score(
                        &row.raw.payee,
                        a.payee.as_deref().unwrap_or(""),
                    ))
                })
                .then_with(|| a.entered_at.cmp(&b.entered_at))
        });
    Ok(best.map(|s| (s.txn_id, s.index)))
}

/// Tags the file itself gives a row (an IIF `CLASS` mapped in review).
fn file_tags(env: &Env<'_>, row: &RowDoc) -> mk_core::SplitTags {
    let class = row
        .raw
        .class_hint
        .as_deref()
        .or_else(|| row.raw.splits.iter().find_map(|s| s.class_hint.as_deref()))
        .and_then(|c| env.class_map.get(&c.trim().to_lowercase()).cloned());
    mk_core::SplitTags {
        class,
        ..mk_core::SplitTags::default()
    }
}

fn propose_other(
    env: &Env<'_>,
    row: &mut RowDoc,
    target: AccountId,
    section_name: Option<&str>,
) -> CommandResult<()> {
    let ctx = env.ctx;
    let chart = &env.chart;
    let amount = row.raw.amount;
    let single = |account: Other| ProposedSplit {
        account,
        value: -amount,
        memo: String::new(),
    };
    row.tags = file_tags(env, row);
    // A user's choice always wins.
    if let Some(a) = row.overrides.other {
        row.other = vec![single(Other::Account(a))];
        row.other_source = Some("user".into());
        return Ok(());
    }
    // A deposit equal to a processor payout waiting in its clearing account
    // (connectivity-processors.md): a transfer from that account.
    if row.raw.splits.is_empty()
        && let Some((acc, when)) =
            crate::processors::deposit_source(ctx, target, amount, row.raw.date, env.window)?
    {
        row.other = vec![single(Other::Account(acc))];
        row.other_source = Some("transfer".into());
        row.warnings.push(format!(
            "matches the payout of {when} waiting in {}",
            chart.full_name(acc)
        ));
        return Ok(());
    }
    // 1. Source splits.
    if !row.raw.splits.is_empty() {
        let mut sum = Decimal::ZERO;
        for s in &row.raw.splits {
            sum = sum
                .checked_add(s.amount)
                .ok_or_else(|| CommandError::Rejected("overflow".into()))?;
        }
        if sum == amount {
            let mut out = Vec::new();
            for s in &row.raw.splits {
                let value = -s.amount;
                let mapped = env
                    .account_map
                    .get(&s.category_hint.trim().to_lowercase())
                    .copied();
                let acc = match mapped {
                    Some(a) => Some(a),
                    None => resolve_hint(
                        ctx,
                        chart,
                        &s.category_hint,
                        target,
                        section_name,
                        &row.raw.payee,
                    )?,
                }
                .map_or_else(|| Other::uncategorized_for(value), Other::Account);
                out.push(ProposedSplit {
                    account: acc,
                    value,
                    memo: s.memo.clone(),
                });
            }
            row.other = out;
            row.other_source = Some("splits".into());
            return Ok(());
        }
        row.needs_review = true;
        row.warnings.push(format!(
            "source splits sum to {sum}, not the row amount {amount}"
        ));
        row.other = vec![single(Other::uncategorized_for(-amount))];
        row.other_source = Some("uncategorized".into());
        return Ok(());
    }
    // 2. Rules.
    let rr = RuleRow {
        target,
        amount,
        payee: row.raw.payee.clone(),
        memo: row.raw.memo.clone(),
        number: row.raw.number.clone(),
        category_hint: row.raw.category_hint.clone(),
        trntype: row.raw.trntype.clone(),
    };
    let ok = |id: &str| {
        id.parse::<AccountId>()
            .is_ok_and(|a| usable(chart, a, target))
    };
    let mut warnings = Vec::new();
    let out = evaluate_rules(&env.rules, &rr, &ok, &mut warnings);
    row.warnings.extend(warnings);
    if !out.rule_ids.is_empty() {
        row.rule_ids = out.rule_ids.clone();
        for (slot, v) in [
            (&mut row.tags.class, &out.class),
            (&mut row.tags.location, &out.location),
            (&mut row.tags.fund, &out.fund),
            (&mut row.tags.project, &out.project),
        ] {
            if v.is_some() {
                slot.clone_from(v);
            }
        }
        // projects.md: a rule setting the project also brings the project's fund
        // (when the row has none) and its billable default.
        if out.project.is_some()
            && let Some(p) = out
                .project
                .as_deref()
                .and_then(|id| env.projects.iter().find(|p| p.id == id))
        {
            row.tags.billable = p.billable;
            if row.tags.fund.is_none() && env.funds_on {
                row.tags.fund.clone_from(&p.fund);
            }
        }
        if row.overrides.payee.is_none()
            && let Some(p) = &out.payee
        {
            row.payee = p.clone();
        }
        if row.overrides.memo.is_none()
            && let Some(m) = &out.memo
        {
            row.memo = m.clone();
        }
        if out.skip {
            row.action = RowAction::Skip;
        }
        if let Some(acc) = out
            .account
            .as_deref()
            .and_then(|a| a.parse::<AccountId>().ok())
        {
            row.other = vec![single(Other::Account(acc))];
            row.other_source = Some("rule".into());
            return Ok(());
        }
        if let Some(shares) = &out.split {
            let currency = chart
                .get(target)
                .map(|a| a.commodity.clone())
                .ok_or_else(|| CommandError::NotFound("account".into()))?;
            let pcts: Vec<Decimal> = shares.iter().map(|s| s.percent).collect();
            let parts = split_amount(amount, &pcts, &currency)
                .map_err(|e| CommandError::InvalidArgs(e.to_string()))?;
            row.other = shares
                .iter()
                .zip(parts)
                .filter_map(|(s, part)| {
                    s.account.parse::<AccountId>().ok().map(|a| ProposedSplit {
                        account: Other::Account(a),
                        value: -part,
                        memo: String::new(),
                    })
                })
                .collect();
            row.other_source = Some("rule".into());
            return Ok(());
        }
    }
    // 3. Category hint.
    if let Some(acc) = resolve_hint(
        ctx,
        chart,
        &row.raw.category_hint,
        target,
        section_name,
        &row.raw.payee,
    )? {
        row.other = vec![single(Other::Account(acc))];
        row.other_source = Some("hint".into());
        return Ok(());
    }
    // 4. Uncategorized.
    row.other = vec![single(Other::uncategorized_for(-amount))];
    row.other_source = Some("uncategorized".into());
    Ok(())
}

/// Link a new transfer row to its counterpart: an earlier new row of this batch, or an
/// already imported counterpart transaction in the ledger.
fn link_transfer(
    env: &Env<'_>,
    row: &mut RowDoc,
    target: AccountId,
    used: &mut HashSet<(TransactionId, usize)>,
    rows: &mut [RowDoc],
    batch: &BatchDoc,
) -> CommandResult<()> {
    let chart = &env.chart;
    let other_acc = match row.other.as_slice() {
        [
            ProposedSplit {
                account: Other::Account(a),
                ..
            },
        ] => Some(*a),
        _ => None,
    };
    let section_of = |r: &RowDoc| batch.sections.get(r.section).and_then(|s| s.account);
    let within = |d: Date| row.raw.date.days_until(d).abs() <= env.window;
    // Within this batch.
    let pos = rows.iter().position(|y| {
        y.id != row.id
            && y.class == Class::New
            && y.action == RowAction::Create
            && y.link.is_none()
            && y.raw.splits.is_empty()
            && y.overrides.other.is_none()
            && y.raw.amount == -row.raw.amount
            && within(y.raw.date)
            && row_ord(y) < row_ord(row)
            && !rows
                .iter()
                .any(|z| matches!(&z.link, Some(Link::Row { row: r }) if *r == y.id))
            && {
                let y_target = section_of(y);
                let y_other = match y.other.as_slice() {
                    [
                        ProposedSplit {
                            account: Other::Account(a),
                            ..
                        },
                    ] => Some(*a),
                    _ => None,
                };
                y_other == Some(target) || (other_acc.is_some() && other_acc == y_target)
            }
    });
    if let Some(p) = pos {
        if let Some(y) = rows.get_mut(p) {
            if !matches!(y.other.as_slice(), [ProposedSplit { account: Other::Account(a), .. }] if *a == target)
            {
                y.other = vec![ProposedSplit {
                    account: Other::Account(target),
                    value: -y.raw.amount,
                    memo: String::new(),
                }];
                y.other_source = Some("transfer".into());
            }
            row.class = Class::Match;
            row.action = RowAction::Match;
            row.link = Some(Link::Row { row: y.id.clone() });
            row.other.clear();
            row.other_source = None;
        }
        return Ok(());
    }
    // An already imported counterpart in the ledger.
    let Some(o) = other_acc.filter(|o| chart.get(*o).is_some_and(is_transfer_account)) else {
        return Ok(());
    };
    let w = i32::try_from(env.window).unwrap_or(3);
    let splits =
        env.ctx
            .tx
            .account_splits(o, row.raw.date.add_days(-w), row.raw.date.add_days(w))?;
    for s in splits {
        if s.fingerprint.is_none() || s.quantity != -row.raw.amount {
            continue;
        }
        let Some(t) = env.ctx.tx.transaction(s.txn_id)? else {
            continue;
        };
        if t.splits.len() != 2 {
            continue;
        }
        let ci = if s.index == 0 { 1 } else { 0 };
        let Some(c) = t.splits.get(ci) else { continue };
        let categorisation = chart
            .get(c.account)
            .is_some_and(|a| !is_transfer_account(a));
        if c.fingerprint.is_some()
            || c.reconcile == Reconcile::Reconciled
            || !categorisation
            || c.value != row.raw.amount
        {
            continue;
        }
        if used.contains(&(t.id, ci)) {
            continue;
        }
        used.insert((t.id, ci));
        row.class = Class::Match;
        row.action = RowAction::Match;
        row.link = Some(Link::Transfer {
            txn: t.id,
            split: ci,
        });
        row.other.clear();
        row.other_source = None;
        return Ok(());
    }
    Ok(())
}

// ----- views -------------------------------------------------------------------

fn other_name(chart: &Chart, o: &Other) -> String {
    let by_role = |role: &str, fallback: &str| {
        crate::chart::roles::holder(chart, role)
            .map_or_else(|| fallback.to_owned(), |a| chart.full_name(a.id))
    };
    match o {
        Other::Account(a) => chart.full_name(*a),
        Other::UncategorizedExpense => {
            by_role(crate::chart::roles::UNCATEGORIZED_EXPENSE, UNCAT_EXPENSE)
        }
        Other::UncategorizedIncome => {
            by_role(crate::chart::roles::UNCATEGORIZED_INCOME, UNCAT_INCOME)
        }
    }
}

fn row_json(chart: &Chart, r: &RowDoc) -> CommandResult<Value> {
    let mut v = serde_json::to_value(r)?;
    if let Some(o) = v.as_object_mut() {
        let others: Vec<Value> = r
            .other
            .iter()
            .map(|p| json!({ "account": p.account, "account_name": other_name(chart, &p.account), "value": p.value, "memo": p.memo }))
            .collect();
        o.insert("other".into(), Value::Array(others));
        o.insert("date".into(), json!(r.raw.date));
        o.insert("amount".into(), json!(r.raw.amount));
        o.insert("number".into(), json!(r.raw.number));
        o.insert("category_hint".into(), json!(r.raw.category_hint));
        o.insert("cleared".into(), json!(r.raw.cleared));
        if let Some(Link::Redate { from, .. }) = &r.link {
            o.insert("date_from".into(), json!(from));
        }
    }
    Ok(v)
}

/// Digest of a batch's proposal: its sections and every staged row (bank and
/// investment), excluding commit results.
fn proposal_digest(ctx: &Ctx<'_>, batch: &BatchDoc) -> CommandResult<String> {
    let mut h = Sha256::new();
    h.update(serde_json::to_vec(&batch.sections)?);
    for table in [DocTable::ImportRow, DocTable::InvImportRow] {
        for d in ctx.tx.docs(table, Some(&batch.id))? {
            let mut j = d.json.clone();
            if let Some(o) = j.as_object_mut() {
                o.remove("result");
            }
            h.update(d.id.as_bytes());
            h.update(serde_json::to_vec(&j)?);
        }
    }
    Ok(hex(&h.finalize()))
}

/// Record the reviewed proposal of a batch and return its review JSON (every review
/// command ends here).
pub(crate) fn sealed_json(ctx: &Ctx<'_>, id: &str) -> CommandResult<Value> {
    let mut batch = load_batch(ctx, id)?;
    let digest = proposal_digest(ctx, &batch)?;
    if batch.proposal.as_deref() != Some(digest.as_str()) {
        batch.proposal = Some(digest);
        save_batch(ctx, &batch)?;
    }
    batch_json(ctx, id)
}

pub(crate) fn batch_json(ctx: &Ctx<'_>, id: &str) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let batch = load_batch(ctx, id)?;
    let mut rows = load_rows(ctx, id)?;
    rows.sort_by_key(row_ord);
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for r in &rows {
        let k = match r.class {
            Class::Pending => "pending",
            Class::Duplicate => "duplicate",
            Class::Match => "match",
            Class::New => "new",
            Class::Correction => "correction",
            Class::CorrectionDelete => "correction_delete",
            Class::DateCorrection => "date_correction",
        };
        *counts.entry(k).or_default() += 1;
    }
    let sections: Vec<Value> = batch
        .sections
        .iter()
        .map(|s| {
            let mut v = serde_json::to_value(s).unwrap_or(Value::Null);
            if let Some(o) = v.as_object_mut() {
                o.insert("resolved".into(), json!(s.account.is_some()));
                o.insert(
                    "account_name_resolved".into(),
                    json!(s.account.map(|a| chart.full_name(a))),
                );
                o.insert(
                    "rows".into(),
                    json!(rows.iter().filter(|r| r.section == s.index).count()),
                );
            }
            v
        })
        .collect();
    let mut sections = sections;
    for (v, s) in sections.iter_mut().zip(&batch.sections) {
        let extra = crate::inv_import::section_extra(ctx, s)?;
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in e {
                o.insert(k.clone(), x.clone());
            }
        }
    }
    let inv_rows = crate::inv_import::rows_json(ctx, &batch)?;
    for r in &inv_rows {
        if let Some(c) = r.get("class").and_then(Value::as_str) {
            let k: &'static str = match c {
                "duplicate" => "duplicate",
                "match" => "match",
                "new" => "new",
                "reverse" => "reverse",
                "unsupported" => "unsupported",
                _ => "pending",
            };
            *counts.entry(k).or_default() += 1;
        }
    }
    let row_values = rows
        .iter()
        .map(|r| row_json(&chart, r))
        .collect::<CommandResult<Vec<_>>>()?;
    Ok(json!({
        "id": batch.id, "format": batch.format, "status": batch.status,
        "created_at": batch.created_at, "source": batch.source, "sha256": batch.digest,
        "actor": batch.actor, "warnings": batch.warnings, "sections": sections,
        "counts": counts, "rows": row_values, "inv_rows": inv_rows,
        "names": crate::iif::names_json(ctx, &batch)?, "lists": batch.lists,
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    /// `staged`, `committed` or `discarded`.
    #[serde(default)]
    status: Option<String>,
}

fn list(ctx: &Ctx<'_>, a: ListArgs) -> CommandResult<Value> {
    let mut out = Vec::new();
    for d in ctx.tx.docs(DocTable::ImportBatch, None)? {
        let b: BatchDoc = from_doc(&d)?;
        if a.status.as_ref().is_some_and(|s| *s != b.status) {
            continue;
        }
        let rows = ctx.tx.docs(DocTable::ImportRow, Some(&b.id))?.len();
        out.push(json!({
            "id": b.id, "format": b.format, "status": b.status, "created_at": b.created_at,
            "source": b.source, "sections": b.sections.len(), "rows": rows,
        }));
    }
    Ok(Value::Array(out))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BatchRef {
    /// Batch id.
    pub batch: String,
}

fn get(ctx: &Ctx<'_>, a: BatchRef) -> CommandResult<Value> {
    batch_json(ctx, &a.batch)
}

pub(crate) fn staged(ctx: &Ctx<'_>, id: &str) -> CommandResult<BatchDoc> {
    let b = load_batch(ctx, id)?;
    if b.status != "staged" {
        return Err(CommandError::Rejected(format!(
            "batch {id} is {}, not staged",
            b.status
        )));
    }
    Ok(b)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MapArgs {
    batch: String,
    /// Section index (0-based). Default 0.
    #[serde(default)]
    section: usize,
    /// Account id, full name or code.
    account: String,
    /// Save the mapping for future files from the same source account.
    #[serde(default)]
    remember: bool,
}

fn map_account(ctx: &Ctx<'_>, a: MapArgs) -> CommandResult<Value> {
    let mut batch = staged(ctx, &a.batch)?;
    let chart = Chart::load(ctx)?;
    let acc = chart.resolve(&a.account)?.id;
    let sec = batch.sections.get_mut(a.section).ok_or_else(|| {
        CommandError::InvalidArgs(format!("the batch has no section {}", a.section))
    })?;
    check_section_account(&chart, sec, acc)?;
    if sec.account != Some(acc) {
        sec.account = Some(acc);
        // A statement captured for a previous choice is replaced.
        if let Some(old) = sec.statement.take() {
            ctx.save_doc(DocTable::Statement, &old, None)?;
        }
    }
    if a.remember {
        let key = sec.account_key.clone().ok_or_else(|| {
            CommandError::Rejected("this section has no source account identity to remember".into())
        })?;
        let doc = Doc {
            id: key.clone(),
            grp: None,
            ord: 0,
            json: json!({ "key": key, "account": acc }),
        };
        ctx.save_doc(DocTable::ImportAccountMap, &key, Some(&doc))?;
    }
    capture_statements(ctx, &mut batch)?;
    save_batch(ctx, &batch)?;
    classify(ctx, &batch)?;
    crate::inv_import::classify(ctx, &mut batch)?;
    sealed_json(ctx, &batch.id)
}

// ----- set_row -----------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetRowArgs {
    batch: String,
    /// Row id.
    row: String,
    #[serde(default)]
    action: Option<RowAction>,
    /// With action `match`: the transaction to match.
    #[serde(default)]
    match_txn: Option<TransactionId>,
    /// Other-side account (id, full name or code); null removes your choice and
    /// restores the automatic proposal.
    #[serde(default)]
    other_account: crate::accounts::Patch<String>,
    #[serde(default)]
    payee: Option<String>,
    #[serde(default)]
    memo: Option<String>,
    /// With other_account: create a rule `payee equals <payee>` → that account.
    #[serde(default)]
    make_rule: bool,
    /// Processor rows naming open invoices: true applies the payments, false posts
    /// sales instead.
    #[serde(default)]
    apply_invoices: Option<bool>,
}

fn set_row(ctx: &Ctx<'_>, a: SetRowArgs) -> CommandResult<Value> {
    let batch = staged(ctx, &a.batch)?;
    let d = ctx
        .tx
        .get_doc(DocTable::ImportRow, &a.row)?
        .filter(|d| d.grp.as_deref() == Some(a.batch.as_str()))
        .ok_or_else(|| CommandError::NotFound(format!("row {} in batch {}", a.row, a.batch)))?;
    let mut row: RowDoc = from_doc(&d)?;
    let chart = Chart::load(ctx)?;
    let target = batch.sections.get(row.section).and_then(|s| s.account);
    if let Some(action) = a.action {
        if action == RowAction::Match {
            let txn = a
                .match_txn
                .ok_or_else(|| CommandError::InvalidArgs("action match needs match_txn".into()))?;
            let t =
                target.ok_or_else(|| CommandError::Rejected("resolve the section first".into()))?;
            user_match_split(ctx, txn, t, row.raw.amount)?;
            row.overrides.match_txn = Some(txn);
        } else {
            row.overrides.match_txn = None;
        }
        row.overrides.action = Some(action);
    }
    let mut chosen = None;
    if let Some(other) = a.other_account.0 {
        let id = other
            .as_deref()
            .map(|r| chart.resolve(r).map(|x| x.id))
            .transpose()?;
        if let Some(id) = id {
            let t = target.unwrap_or(id);
            if !usable(&chart, id, t) || Some(id) == target {
                return Err(CommandError::Rejected(format!(
                    "{} cannot be the other side (it is the target, a placeholder or archived)",
                    chart.full_name(id)
                )));
            }
        }
        row.overrides.other = id;
        chosen = id;
    }
    if let Some(p) = a.payee {
        row.overrides.payee = Some(p);
    }
    if let Some(m) = a.memo {
        row.overrides.memo = Some(m);
    }
    if let Some(x) = a.apply_invoices {
        if row.raw.invoices.is_empty() {
            return Err(CommandError::Rejected(
                "this row names no open invoice".into(),
            ));
        }
        row.overrides.invoice = Some(x);
    }
    save_row(ctx, &row)?;
    if a.make_rule {
        let acc = chosen
            .ok_or_else(|| CommandError::InvalidArgs("make_rule needs other_account".into()))?;
        let payee = normalise_payee(row.overrides.payee.as_deref().unwrap_or(&row.raw.payee));
        if payee.is_empty() {
            return Err(CommandError::Rejected(
                "the row has no payee to make a rule from".into(),
            ));
        }
        crate::rules::learn_rule(ctx, &payee, acc)?;
    }
    classify(ctx, &batch)?;
    sealed_json(ctx, &batch.id)
}

// ----- commit / discard -------------------------------------------------------

fn uncategorized_account(
    ctx: &Ctx<'_>,
    which: Other,
    currency: &CommodityCode,
) -> CommandResult<AccountId> {
    let (name, ty, role) = match which {
        Other::UncategorizedExpense => (
            UNCAT_EXPENSE,
            AccountType::Expense,
            crate::chart::roles::UNCATEGORIZED_EXPENSE,
        ),
        Other::UncategorizedIncome => (
            UNCAT_INCOME,
            AccountType::Income,
            crate::chart::roles::UNCATEGORIZED_INCOME,
        ),
        Other::Account(a) => return Ok(a),
    };
    let chart = Chart::load(ctx)?;
    // chart-presets.md lookup rule: the account holding the role first.
    if let Some(a) = crate::chart::roles::holder(&chart, role) {
        return Ok(a.id);
    }
    let id = uncategorized_by_name(ctx, &chart, name, ty, currency)?;
    let base = ctx.tx.info()?.base_currency;
    let in_base = Chart::load(ctx)?
        .get(id)
        .is_some_and(|a| a.commodity.as_str() == base);
    if in_base {
        crate::chart::roles::give(ctx, id, role)?;
    }
    Ok(id)
}

fn uncategorized_by_name(
    ctx: &Ctx<'_>,
    chart: &Chart,
    name: &str,
    ty: AccountType,
    currency: &CommodityCode,
) -> CommandResult<AccountId> {
    if let Some(a) = chart
        .accounts
        .iter()
        .find(|a| a.parent.is_none() && a.name.eq_ignore_ascii_case(name))
    {
        if a.account_type != ty {
            return Err(CommandError::Rejected(format!(
                "a top-level account named {name:?} exists with type {}",
                a.account_type
            )));
        }
        return Ok(a.id);
    }
    let base: CommodityCode = ctx.tx.info()?.base_currency.parse()?;
    let account = Account {
        id: AccountId::new(),
        name: name.into(),
        account_type: ty,
        subtype: ty.default_subtype(),
        parent: None,
        commodity: if *currency == base {
            base
        } else {
            currency.clone()
        },
        code: None,
        description: None,
        placeholder: false,
        archived: false,
        created_at: now_rfc3339()?,
        tax_line: None,
        role: None,
        retirement: false,
        opening_none: false,
    };
    ctx.save_account(None, &account)?;
    Ok(account.id)
}

fn attach(
    ctx: &Ctx<'_>,
    txn: TransactionId,
    split: usize,
    fp: &str,
    expect: Option<(AccountId, Decimal)>,
) -> CommandResult<()> {
    let old = ctx.tx.transaction(txn)?.ok_or_else(|| {
        CommandError::Rejected(format!(
            "matched transaction {txn} no longer exists; re-stage the file"
        ))
    })?;
    let mut new = old.clone();
    let s = new.splits.get_mut(split).ok_or_else(|| {
        CommandError::Rejected("matched split no longer exists; re-stage the file".into())
    })?;
    if s.fingerprint.is_some()
        || expect.is_some_and(|(a, q)| s.account != a || s.quantity != q)
        || old.status != TxnStatus::Posted
    {
        return Err(CommandError::Rejected(format!(
            "transaction {txn} changed since staging; re-stage the file"
        )));
    }
    s.fingerprint = Some(fp.to_owned());
    if s.reconcile == Reconcile::New {
        s.reconcile = Reconcile::Cleared;
    }
    validate_change(ctx, Some(&old), Some(&new))?;
    ctx.save_txn(Some(&old), &new)
}

/// Commit a staged batch (bank feeds' automatic commit).
pub(crate) fn commit_batch(ctx: &Ctx<'_>, id: &str) -> CommandResult<Value> {
    commit(
        ctx,
        BatchRef {
            batch: id.to_owned(),
        },
    )
}

fn commit(ctx: &Ctx<'_>, a: BatchRef) -> CommandResult<Value> {
    let mut batch = staged(ctx, &a.batch)?;
    // History shows the source file and its digest for every import (input-safety R7).
    *ctx.audit_args.borrow_mut() = Some(json!({
        "batch": batch.id, "name": batch.source, "sha256": batch.digest,
    }));
    if let Some(s) = batch.sections.iter().find(|s| s.account.is_none()) {
        return Err(CommandError::Rejected(format!(
            "section {} is not resolved to an account; use import.map_account",
            s.index
        )));
    }
    if let Some(p) = crate::inv_import::securities_ready(&batch) {
        return Err(CommandError::Rejected(p));
    }
    if let Some(sealed) = &batch.proposal
        && *sealed != proposal_digest(ctx, &batch)?
    {
        return Err(CommandError::Rejected(
            "the batch changed after it was reviewed; open the review again before adding it"
                .into(),
        ));
    }
    let mut rows = load_rows(ctx, &batch.id)?;
    rows.sort_by_key(row_ord);
    if let Some(r) = rows
        .iter()
        .find(|r| r.action == RowAction::Create && r.other.is_empty())
    {
        return Err(CommandError::Rejected(format!(
            "row {} (line {}) has no other-side account",
            r.id, r.raw.line
        )));
    }
    let chart = Chart::load(ctx)?;
    let mut created_by_row: HashMap<String, TransactionId> = HashMap::new();
    let (mut created, mut matched, mut skipped) = (0usize, 0usize, 0usize);
    let mut redated = 0usize;
    for row in &mut rows {
        let Some(target) = batch.sections.get(row.section).and_then(|s| s.account) else {
            continue;
        };
        let Some(fp) = row.fingerprint.clone() else {
            continue;
        };
        // Revision 2: a date correction changes only the transaction's date.
        if let (RowAction::Match, Some(Link::Redate { txn, from })) = (&row.action, &row.link) {
            let old = ctx.tx.transaction(*txn)?.ok_or_else(|| {
                CommandError::Rejected(
                    "the transaction to re-date no longer exists; re-stage".into(),
                )
            })?;
            if old.date != *from {
                return Err(CommandError::Rejected(format!(
                    "row at line {}: the transaction's date changed since staging; re-stage",
                    row.raw.line
                )));
            }
            let mut new = old.clone();
            new.date = row.raw.date;
            validate_change(ctx, Some(&old), Some(&new))
                .map_err(|e| prefix_line(e, row.raw.line))?;
            ctx.save_txn(Some(&old), &new)?;
            redated += 1;
            row.result = Some(json!({ "date_corrected": txn, "from": from }));
            continue;
        }
        if row.action == RowAction::Skip
            || row.class == Class::Duplicate
            || ctx.tx.fingerprint_exists(target, &fp)?
            || crate::opening::forgotten(ctx, target, &fp)?
        {
            skipped += 1;
            row.result = Some(json!({ "skipped": true }));
            continue;
        }
        match (&row.action, &row.link) {
            (RowAction::Match, Some(Link::Split { txn, split })) => {
                attach(ctx, *txn, *split, &fp, Some((target, row.raw.amount)))?;
                matched += 1;
                row.result = Some(json!({ "matched": txn }));
            }
            (RowAction::Match, Some(Link::Row { row: other })) => {
                let txn = *created_by_row.get(other).ok_or_else(|| {
                    CommandError::Rejected("linked transfer row was not created".into())
                })?;
                let t = ctx
                    .tx
                    .transaction(txn)?
                    .ok_or_else(|| CommandError::Integrity("created transaction missing".into()))?;
                let split = t
                    .splits
                    .iter()
                    .position(|s| {
                        s.account == target
                            && s.fingerprint.is_none()
                            && s.quantity == row.raw.amount
                    })
                    .ok_or_else(|| {
                        CommandError::Integrity("transfer counterpart split missing".into())
                    })?;
                attach(ctx, txn, split, &fp, None)?;
                matched += 1;
                row.result = Some(json!({ "matched": txn }));
            }
            (RowAction::Match, Some(Link::Transfer { txn, split })) => {
                let old = ctx.tx.transaction(*txn)?.ok_or_else(|| {
                    CommandError::Rejected("transfer counterpart no longer exists; re-stage".into())
                })?;
                let mut new = old.clone();
                let s = new.splits.get_mut(*split).ok_or_else(|| {
                    CommandError::Rejected("transfer counterpart changed; re-stage".into())
                })?;
                if s.fingerprint.is_some()
                    || s.value != row.raw.amount
                    || s.reconcile == Reconcile::Reconciled
                {
                    return Err(CommandError::Rejected(
                        "transfer counterpart changed since staging; re-stage".into(),
                    ));
                }
                s.account = target;
                s.quantity = s.value;
                s.fingerprint = Some(fp.clone());
                s.reconcile = cleared_state(&batch.format, row.raw.cleared);
                sort_splits(&mut new.splits);
                validate_change(ctx, Some(&old), Some(&new))?;
                ctx.save_txn(Some(&old), &new)?;
                matched += 1;
                row.result = Some(json!({ "linked": txn }));
            }
            (RowAction::Match, Some(Link::Correct { txn, split })) => {
                let old = ctx.tx.transaction(*txn)?.ok_or_else(|| {
                    CommandError::Rejected(
                        "the corrected transaction no longer exists; re-stage".into(),
                    )
                })?;
                let mut new = old.clone();
                let clean = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_owned());
                new.date = row.raw.date;
                new.payee = clean(&row.payee);
                new.memo = clean(&row.memo);
                new.number = clean(&row.raw.number);
                let amount = row.raw.amount;
                for (i, s) in new.splits.iter_mut().enumerate() {
                    if i == *split {
                        s.value = amount;
                        s.quantity = amount;
                        s.fingerprint = Some(fp.clone());
                    } else if new_len_two(&old) {
                        s.value = -amount;
                        s.quantity = -amount;
                    }
                }
                sort_splits(&mut new.splits);
                validate_change(ctx, Some(&old), Some(&new))
                    .map_err(|e| prefix_line(e, row.raw.line))?;
                ctx.save_txn(Some(&old), &new)?;
                matched += 1;
                row.result = Some(json!({ "corrected": txn }));
            }
            (RowAction::Match, Some(Link::Void { txn })) => {
                let old = ctx.tx.transaction(*txn)?.ok_or_else(|| {
                    CommandError::Rejected(
                        "the transaction to delete no longer exists; re-stage".into(),
                    )
                })?;
                if old.status == TxnStatus::Posted {
                    let mut new = old.clone();
                    new.status = TxnStatus::Voided;
                    new.void_reason = Some("correction from bank".into());
                    validate_change(ctx, Some(&old), Some(&new))
                        .map_err(|e| prefix_line(e, row.raw.line))?;
                    ctx.save_txn(Some(&old), &new)?;
                }
                matched += 1;
                row.result = Some(json!({ "voided": txn }));
            }
            // Handled above.
            (RowAction::Match, Some(Link::Redate { .. })) => {}
            (RowAction::Match, None) => {
                return Err(CommandError::Integrity(format!(
                    "match row {} has no link",
                    row.id
                )));
            }
            (RowAction::Create, _) => {
                let currency = chart
                    .get(target)
                    .map(|a| a.commodity.clone())
                    .ok_or_else(|| CommandError::NotFound("account".into()))?;
                // Accepted invoice payments go through payment.receive into this
                // (clearing) account; the rest of the row posts as usual.
                let mut own = row.raw.amount;
                let mut others = row.other.clone();
                let mut paid_txn: Option<TransactionId> = None;
                if row.overrides.invoice == Some(true) {
                    for inv in &row.raw.invoices {
                        let Some((id, customer, balance)) =
                            crate::sales::open_invoice_balance(ctx, &inv.number)?
                        else {
                            continue;
                        };
                        let amount = inv.amount.min(balance);
                        let pay = crate::sales::receive_json(
                            ctx,
                            json!({"customer": customer, "date": row.raw.date, "amount": amount,
                                   "method": "card", "deposit_to": target.to_string(),
                                   "reference": row.raw.external_id,
                                   "apply": [{"invoice": id, "amount": amount}]}),
                        )?;
                        paid_txn = pay
                            .get("txn")
                            .and_then(Value::as_str)
                            .and_then(|t| t.parse().ok());
                        own = own.checked_sub(amount).unwrap_or(own);
                        if let Some(p) = others.iter_mut().find(|p| p.memo == "Sales") {
                            p.value = p.value.checked_add(amount).unwrap_or(p.value);
                        }
                    }
                    others.retain(|p| !p.value.is_zero());
                }
                // Nothing left beyond the payment: it carries the row's fingerprint.
                if own.is_zero()
                    && others.is_empty()
                    && let Some(txn) = paid_txn
                {
                    let t = ctx.tx.transaction(txn)?.ok_or_else(|| {
                        CommandError::Integrity("payment transaction missing".into())
                    })?;
                    let split = t
                        .splits
                        .iter()
                        .position(|s| s.account == target)
                        .ok_or_else(|| CommandError::Integrity("payment split missing".into()))?;
                    attach(ctx, txn, split, &fp, None)?;
                    created += 1;
                    row.result = Some(json!({ "created": txn }));
                    continue;
                }
                let mut splits = vec![Split {
                    account: target,
                    value: own,
                    quantity: own,
                    memo: None,
                    reconcile: cleared_state(&batch.format, row.raw.cleared),
                    fingerprint: Some(fp.clone()),
                    tags: mk_core::SplitTags {
                        class: None,
                        location: row.tags.location.clone(),
                        fund: row.tags.fund.clone(),
                        project: None,
                        billable: false,
                    },
                }];
                for p in &others {
                    let acc = uncategorized_account(ctx, p.account, &currency)?;
                    splits.push(Split {
                        account: acc,
                        value: p.value,
                        quantity: p.value,
                        memo: (!p.memo.trim().is_empty()).then(|| p.memo.trim().to_owned()),
                        reconcile: Reconcile::New,
                        fingerprint: None,
                        tags: row.tags.clone(),
                    });
                }
                // Other-side accounts in another commodity need a quantity; imports
                // only create same-currency transactions.
                let after = Chart::load(ctx)?;
                for s in &splits {
                    if after
                        .get(s.account)
                        .is_some_and(|a| a.commodity != currency)
                    {
                        return Err(CommandError::Rejected(format!(
                            "row at line {}: {} is not in {currency}; pick another account in review",
                            row.raw.line,
                            after.full_name(s.account)
                        )));
                    }
                }
                sort_splits(&mut splits);
                let clean = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_owned());
                let t = Transaction {
                    id: TransactionId::new(),
                    date: row.raw.date,
                    currency,
                    payee: clean(&row.payee),
                    number: clean(&row.raw.number),
                    memo: clean(&row.memo),
                    status: TxnStatus::Posted,
                    void_reason: None,
                    reverses: None,
                    opening: false,
                    entered_at: now_rfc3339()?,
                    splits,
                };
                validate_change(ctx, None, Some(&t)).map_err(|e| prefix_line(e, row.raw.line))?;
                ctx.save_txn(None, &t)?;
                created_by_row.insert(row.id.clone(), t.id);
                created += 1;
                row.result = Some(json!({ "created": t.id }));
            }
            (RowAction::Skip, _) => {}
        }
    }
    for r in &rows {
        save_row(ctx, r)?;
    }
    let inv = crate::inv_import::commit(ctx, &mut batch)?;
    batch.status = "committed".into();
    save_batch(ctx, &batch)?;
    Ok(json!({
        "batch": batch.id, "status": batch.status,
        "created": created + inv.created, "matched": matched + inv.matched,
        "skipped": skipped + inv.skipped, "voided": inv.voided,
        "date_corrected": redated,
    }))
}

fn new_len_two(t: &Transaction) -> bool {
    t.splits.len() == 2
}

fn prefix_line(e: CommandError, line: usize) -> CommandError {
    match e {
        CommandError::InvalidArgs(m) => {
            CommandError::InvalidArgs(format!("row at line {line}: {m}"))
        }
        CommandError::Rejected(m) => CommandError::Rejected(format!("row at line {line}: {m}")),
        other => other,
    }
}

/// Reconcile flag of the target-account split of an imported row. Bank and card
/// downloads (OFX/QFX/QBO, CSV) are already posted by the institution, so they start
/// cleared. QIF rows follow their `C` field (cleared or reconciled → `c`, blank →
/// `n`). Only reconciliation sets `r`; other-side splits start at `n`.
fn cleared_state(format: &str, c: Option<ClearedFlag>) -> Reconcile {
    match (format, c) {
        ("qif", None) => Reconcile::New,
        _ => Reconcile::Cleared,
    }
}

fn discard(ctx: &Ctx<'_>, a: BatchRef) -> CommandResult<Value> {
    let mut batch = staged(ctx, &a.batch)?;
    batch.status = "discarded".into();
    save_batch(ctx, &batch)?;
    Ok(json!({ "batch": batch.id, "status": batch.status }))
}

/// Whether every row of a batch is a duplicate, match, date correction or
/// rule-categorised (for the watch folder's optional auto-commit).
pub fn auto_committable(batch: &Value) -> bool {
    let resolved = batch["sections"]
        .as_array()
        .is_some_and(|s| s.iter().all(|x| x["resolved"] == true));
    let rows_ok = batch["rows"].as_array().is_some_and(|rows| {
        rows.iter().all(|r| {
            r["needs_review"] == false
                && (matches!(
                    r["class"].as_str(),
                    Some("duplicate" | "match" | "date_correction")
                ) || (r["class"] == "new" && r["other_source"] == "rule")
                    || r["action"] == "skip")
        })
    });
    resolved && rows_ok
}
