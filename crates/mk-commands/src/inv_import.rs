//! Investment import (spec: import-investments.md): investment sections of a batch,
//! security resolution, classification, the commit through the `inv.*` action
//! rules, prices, the positions check, and investment profiles for broker CSV, JSON
//! and XML files. Cash rows of an investment section are ordinary bank rows and go
//! through `import.rs` unchanged.

use std::collections::{HashMap, HashSet};

use mk_core::currency::format_amount;
use mk_core::{
    AccountId, AccountSubtype, AccountType, Date, Decimal, Reconcile, TransactionId, TxnStatus,
};
use mk_import::{InvAction, InvProfile, InvRow, Section, SectionKind, SourceSecurity, Statement};
use mk_store::{Doc, DocTable};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::accounts::Chart;
use crate::import::{BatchDoc, SectionDoc, hex, match_window};
use crate::investments::{
    Action, ActionArgs, BasisMethod, LotArg, Security, SecurityKind, all_metas, create_security,
    find_security, held_on, is_enabled, post, prices_of, sec_account_of, securities, store_price,
    valuations,
};
use crate::txns::validate_change;
use crate::{CommandError, CommandResult, Ctx, Registry};

const STORE_PRICES_KEY: &str = "import.store_prices";
const DIV_KIND_KEY: &str = "import.ofx_div_kind";
const CONFIRMED_KEY: &str = "import.confirmed_action_texts";
const UNVERIFIED: &str = "unverified action text: ";

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "import.set_security",
        "Choose how a file's security is recorded: an existing security or a new one",
        true,
        set_security,
    );
    r.add(
        "import.add_holding_row",
        "Add a shares-in or shares-out row for a holdings difference a feed reported (needs review; the basis is asked)",
        true,
        add_holding_row,
    );
    r.add(
        "import.set_inv_row",
        "Change a staged investment row: skip, transfer basis, split ratio, Move shares in first",
        true,
        set_inv_row,
    );
    r.add(
        "inv_profile.save",
        "Save an investment profile",
        true,
        profile_save,
    );
    r.add(
        "inv_profile.list",
        "List investment profiles (saved and built-in)",
        false,
        profile_list,
    );
    r.add(
        "inv_profile.get",
        "Get an investment profile",
        false,
        profile_get,
    );
    r.add(
        "inv_profile.delete",
        "Delete a saved investment profile",
        true,
        profile_delete,
    );
    r.add(
        "inv_profile.suggest",
        "Propose a profile for a broker file: format, records, fields, action texts",
        false,
        profile_suggest,
    );
    r.add(
        "inv_profile.preview",
        "The first 20 rows of a file as a profile reads them",
        false,
        profile_preview,
    );
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

fn bad_doc(e: impl std::fmt::Display) -> CommandError {
    CommandError::Integrity(format!("bad stored import record: {e}"))
}

// ----- documents -------------------------------------------------------------------

/// A proposed new security.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NewSec {
    pub symbol: String,
    pub name: String,
    /// `stock`, `etf`, `mutual_fund`, `bond`, `cd`, `money_market`, `other`.
    pub kind: String,
    pub basis_method: String,
}

/// The user's choice for a source security.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub(crate) enum SecChoice {
    Existing { security: String },
    New(NewSec),
}

/// A source security and how it resolves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SecRes {
    pub source: SourceSecurity,
    #[serde(default)]
    pub choice: Option<SecChoice>,
    /// Resolved existing security id.
    #[serde(default)]
    pub security: Option<String>,
    /// `mapping`, `cusip`, `ticker`, `user`, `new` or `unsupported`.
    #[serde(default)]
    pub how: String,
    #[serde(default)]
    pub proposed: Option<NewSec>,
    /// The proposed symbol was derived from the name (check it).
    #[serde(default)]
    pub flagged: bool,
    /// Why the proposal cannot be committed (empty or clashing symbol).
    #[serde(default)]
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct InvOverrides {
    pub skip: Option<bool>,
    pub basis: Option<Decimal>,
    pub acquired: Option<Date>,
    pub ratio_new: Option<Decimal>,
    pub ratio_old: Option<Decimal>,
    pub move_in: Option<bool>,
    pub move_in_basis: Option<Decimal>,
    pub move_in_acquired: Option<Date>,
    /// Corporate actions: the terms the file lacks (the command's own arguments,
    /// without the security and date, which come from the row).
    #[serde(default)]
    pub terms: Option<Value>,
}

/// A Move shares in first proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MoveIn {
    pub quantity: Decimal,
    pub basis: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct InvRowDoc {
    pub id: String,
    pub batch: String,
    pub section: usize,
    pub index: usize,
    pub row: InvRow,
    pub fingerprint: Option<String>,
    /// `pending`, `duplicate`, `match`, `reverse`, `unsupported`, `new`.
    pub class: String,
    /// `create`, `match`, `void`, `skip`.
    pub action: String,
    pub link: Option<TransactionId>,
    pub needs_review: bool,
    pub notes: Vec<String>,
    pub overrides: InvOverrides,
    pub proposal: Option<MoveIn>,
    pub result: Option<Value>,
}

fn row_ord(r: &InvRowDoc) -> i64 {
    i64::try_from(r.section * 1_000_000 + r.index).unwrap_or(i64::MAX)
}

fn save_row(ctx: &Ctx<'_>, r: &InvRowDoc) -> CommandResult<()> {
    let doc = Doc {
        id: r.id.clone(),
        grp: Some(r.batch.clone()),
        ord: row_ord(r),
        json: serde_json::to_value(r)?,
    };
    if ctx.tx.get_doc(DocTable::InvImportRow, &r.id)?.as_ref() == Some(&doc) {
        return Ok(());
    }
    ctx.save_doc(DocTable::InvImportRow, &r.id, Some(&doc))
}

pub(crate) fn load_rows(ctx: &Ctx<'_>, batch: &str) -> CommandResult<Vec<InvRowDoc>> {
    let mut v: Vec<InvRowDoc> = ctx
        .tx
        .docs(DocTable::InvImportRow, Some(batch))?
        .into_iter()
        .map(|d| serde_json::from_value(d.json).map_err(bad_doc))
        .collect::<CommandResult<_>>()?;
    v.sort_by_key(row_ord);
    Ok(v)
}

fn setting_bool(ctx: &Ctx<'_>, key: &str, default: bool) -> CommandResult<bool> {
    Ok(ctx
        .tx
        .get_setting("core", key)?
        .and_then(|v| v.as_bool())
        .unwrap_or(default))
}

fn confirmed_texts(ctx: &Ctx<'_>) -> CommandResult<Vec<String>> {
    Ok(ctx
        .tx
        .get_setting("core", CONFIRMED_KEY)?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

// ----- staging ---------------------------------------------------------------------

/// Before staging: drop investment sections when the module is off (with a warning),
/// and apply the OFX dividend-kind setting.
pub(crate) fn prepare_statement(
    ctx: &Ctx<'_>,
    st: &mut Statement,
    format: &str,
) -> CommandResult<()> {
    let on = is_enabled(ctx)?;
    let n = st
        .sections
        .iter()
        .filter(|s| s.kind == SectionKind::Investment)
        .count();
    if !on && n > 0 {
        st.sections.retain(|s| s.kind != SectionKind::Investment);
        st.warnings.push(format!(
            "skipped: investment statements ({n}); turn on the investments module to import them"
        ));
        return Ok(());
    }
    if format == "ofx" {
        let qualified = ctx
            .tx
            .get_setting("core", DIV_KIND_KEY)?
            .and_then(|v| v.as_str().map(str::to_owned))
            .is_some_and(|k| k == "qualified");
        if qualified {
            for s in &mut st.sections {
                for r in &mut s.inv_rows {
                    let is_div = r.raw.starts_with("INCOME") || r.raw.starts_with("REINVEST");
                    if is_div && r.kind.as_deref() == Some("ordinary") && !r.needs_review {
                        r.kind = Some("qualified".into());
                    }
                }
            }
        }
    }
    Ok(())
}

/// Copy a statement section's investment parts into its section document.
pub(crate) fn fill_section(doc: &mut SectionDoc, s: &Section, profile: Option<&str>) {
    doc.kind = s.kind;
    doc.securities = s
        .securities
        .iter()
        .map(|src| SecRes {
            source: src.clone(),
            choice: None,
            security: None,
            how: String::new(),
            proposed: None,
            flagged: false,
            problem: None,
        })
        .collect();
    doc.positions = s.positions.clone();
    doc.positions_unread = s.positions_unread;
    doc.cash_balance = s.cash_balance.clone();
    doc.as_of = s.as_of;
    doc.profile = profile.map(str::to_owned);
}

/// Save a section's investment rows.
pub(crate) fn stage_rows(ctx: &Ctx<'_>, batch: &str, si: usize, s: &Section) -> CommandResult<()> {
    for (ri, row) in s.inv_rows.iter().enumerate() {
        save_row(
            ctx,
            &InvRowDoc {
                id: mk_core::Id::<()>::new().to_string(),
                batch: batch.to_owned(),
                section: si,
                index: ri,
                row: row.clone(),
                fingerprint: None,
                class: "pending".into(),
                action: "skip".into(),
                link: None,
                needs_review: false,
                notes: Vec::new(),
                overrides: InvOverrides::default(),
                proposal: None,
                result: None,
            },
        )?;
    }
    Ok(())
}

/// Investment sections need a brokerage account.
pub(crate) fn check_account(
    chart: &Chart,
    sec: &SectionDoc,
    account: AccountId,
) -> CommandResult<()> {
    if sec.kind == SectionKind::Investment {
        let ok = chart
            .get(account)
            .is_some_and(|a| a.subtype == AccountSubtype::Brokerage);
        if !ok {
            return Err(CommandError::Rejected(format!(
                "section {} is an investment statement; pick a brokerage account",
                sec.index
            )));
        }
    }
    Ok(())
}

// ----- security resolution -----------------------------------------------------------

fn parse_kind(s: &str) -> CommandResult<SecurityKind> {
    serde_json::from_value(json!(s)).map_err(|_| invalid(format!("unknown security kind {s:?}")))
}

fn parse_method(s: &str) -> CommandResult<BasisMethod> {
    serde_json::from_value(json!(s)).map_err(|_| invalid(format!("unknown basis method {s:?}")))
}

/// A symbol derived from a name: its letters and digits, upper case, at most 12.
fn derive_symbol(name: &str) -> String {
    name.split_whitespace()
        .next()
        .unwrap_or("")
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect::<String>()
        .to_ascii_uppercase()
}

fn mapped_security(ctx: &Ctx<'_>, key: &str) -> CommandResult<Option<String>> {
    Ok(ctx
        .tx
        .get_doc(DocTable::ImportSecurityMap, key)?
        .and_then(|d| {
            d.json
                .get("security")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }))
}

/// Resolve every source security of a section (in place).
fn resolve_securities(ctx: &Ctx<'_>, sec: &mut SectionDoc) -> CommandResult<()> {
    let all = securities(ctx)?;
    for r in &mut sec.securities {
        r.security = None;
        r.proposed = None;
        r.flagged = false;
        r.problem = None;
        if r.source.kind.as_deref() == Some("option") {
            r.how = "unsupported".into();
            continue;
        }
        match &r.choice {
            Some(SecChoice::Existing { security }) => {
                if let Some(s) = all.iter().find(|s| s.id == *security) {
                    r.security = Some(s.id.clone());
                    r.how = "user".into();
                    continue;
                }
            }
            Some(SecChoice::New(n)) => {
                r.how = "new".into();
                r.proposed = Some(n.clone());
                r.problem = symbol_problem(&all, &n.symbol);
                continue;
            }
            None => {}
        }
        if let Some(id) = mapped_security(ctx, &r.source.key)?
            && all.iter().any(|s| s.id == id)
        {
            r.security = Some(id);
            r.how = "mapping".into();
            continue;
        }
        if let Some(c) = &r.source.cusip
            && let Some(s) = all.iter().find(|s| s.cusip.as_deref() == Some(c.as_str()))
        {
            r.security = Some(s.id.clone());
            r.how = "cusip".into();
            continue;
        }
        if let Some(f) = &r.source.figi
            && let Some(s) = all.iter().find(|s| s.figi.as_deref() == Some(f.as_str()))
        {
            r.security = Some(s.id.clone());
            r.how = "figi".into();
            continue;
        }
        if let Some(t) = &r.source.ticker {
            // The current symbol first, then an earlier one (symbol changes).
            let found = all
                .iter()
                .find(|s| s.symbol.eq_ignore_ascii_case(t))
                .or_else(|| all.iter().find(|s| s.answers_to(t)));
            if let Some(s) = found {
                r.security = Some(s.id.clone());
                r.how = "ticker".into();
                continue;
            }
        }
        // A CUSIP-only security (a CD or bond) uses its CUSIP as the symbol.
        if let Some(c) = r
            .source
            .cusip
            .as_ref()
            .filter(|_| r.source.ticker.is_none())
            && let Some(s) = all.iter().find(|s| s.symbol.eq_ignore_ascii_case(c))
        {
            r.security = Some(s.id.clone());
            r.how = "ticker".into();
            continue;
        }
        let name = r
            .source
            .name
            .clone()
            .or_else(|| r.source.ticker.clone())
            .unwrap_or_else(|| r.source.key.clone());
        let (symbol, flagged) = match (&r.source.ticker, &r.source.cusip) {
            (Some(t), _) => (t.clone(), false),
            (None, Some(c)) => (c.clone(), false),
            (None, None) => (derive_symbol(&name), true),
        };
        let kind = match r.source.kind.as_deref() {
            Some(k @ ("stock" | "mutual_fund" | "bond" | "cd" | "other" | "etf")) => k.to_owned(),
            _ => "stock".to_owned(),
        };
        let n = NewSec {
            symbol: symbol.clone(),
            name: name.chars().take(100).collect(),
            kind,
            basis_method: "fifo".into(),
        };
        r.problem = symbol_problem(&all, &n.symbol);
        r.proposed = Some(n);
        r.flagged = flagged;
        r.how = "new".into();
    }
    Ok(())
}

fn symbol_problem(all: &[Security], symbol: &str) -> Option<String> {
    let s = symbol.trim();
    if s.is_empty() {
        return Some("enter a symbol".into());
    }
    if all.iter().any(|x| x.symbol.eq_ignore_ascii_case(s)) {
        return Some(format!(
            "{} is already a different security; use it or pick another symbol",
            s.to_ascii_uppercase()
        ));
    }
    None
}

/// How a row's source security resolves: existing id, new proposal, or unsupported.
fn res_of<'a>(sec: &'a SectionDoc, key: Option<&str>) -> Option<&'a SecRes> {
    let k = key?;
    sec.securities.iter().find(|r| r.source.key == k)
}

fn kind_of_res(r: &SecRes, all: &[Security]) -> SecurityKind {
    if let Some(id) = &r.security
        && let Some(s) = all.iter().find(|s| s.id == *id)
    {
        return s.kind;
    }
    r.proposed
        .as_ref()
        .and_then(|p| parse_kind(&p.kind).ok())
        .unwrap_or(SecurityKind::Stock)
}

// ----- classification ----------------------------------------------------------------

fn action_of(a: InvAction) -> Option<Action> {
    Some(match a {
        InvAction::Buy => Action::Buy,
        InvAction::Sell => Action::Sell,
        InvAction::Dividend => Action::Dividend,
        InvAction::Reinvest => Action::Reinvest,
        InvAction::Interest => Action::Interest,
        InvAction::Fee => Action::Fee,
        InvAction::SharesIn => Action::SharesIn,
        InvAction::SharesOut => Action::SharesOut,
        InvAction::Split => Action::Split,
        InvAction::ReturnOfCapital => Action::ReturnOfCapital,
        InvAction::CashInLieu => Action::CashInLieu,
        InvAction::Merger
        | InvAction::Spinoff
        | InvAction::SymbolChange
        | InvAction::Unsupported => return None,
    })
}

fn action_word(a: InvAction) -> &'static str {
    match a {
        InvAction::Buy => "buy",
        InvAction::Sell => "sell",
        InvAction::Dividend => "dividend",
        InvAction::Reinvest => "reinvest",
        InvAction::Interest => "interest",
        InvAction::Fee => "fee",
        InvAction::SharesIn => "shares_in",
        InvAction::SharesOut => "shares_out",
        InvAction::Split => "split",
        InvAction::ReturnOfCapital => "return_of_capital",
        InvAction::Merger => "merger",
        InvAction::Spinoff => "spinoff",
        InvAction::SymbolChange => "symbol_change",
        InvAction::CashInLieu => "cash_in_lieu",
        InvAction::Unsupported => "unsupported",
    }
}

fn fingerprint(account: AccountId, r: &InvRow, occurrence: usize) -> String {
    match &r.external_id {
        Some(x) => format!("ext:{account}:{x}"),
        None => {
            let material = format!(
                "{}|{}|{}|{}|{}|{occurrence}",
                r.date,
                action_word(r.action),
                r.security.as_deref().unwrap_or(""),
                r.quantity
                    .map(|q| q.normalize().to_string())
                    .unwrap_or_default(),
                r.amount
                    .map(|q| q.normalize().to_string())
                    .unwrap_or_default(),
            );
            format!(
                "hash:{account}:{}",
                hex(&Sha256::digest(material.as_bytes()))
            )
        }
    }
}

/// A fingerprint already on a split in the brokerage account or its security
/// accounts: that transaction.
fn fingerprint_txn(
    ctx: &Ctx<'_>,
    chart: &Chart,
    account: AccountId,
    fp: &str,
) -> CommandResult<Option<TransactionId>> {
    for a in chart.subtree(account) {
        if let Some((t, _)) = ctx.tx.fingerprint_split(a, fp)? {
            return Ok(Some(t));
        }
    }
    Ok(None)
}

/// Resolve securities and classify every investment row of a batch. Saves the batch
/// (with the resolution) and the rows.
pub(crate) fn classify(ctx: &Ctx<'_>, batch: &mut BatchDoc) -> CommandResult<()> {
    if !batch
        .sections
        .iter()
        .any(|s| s.kind == SectionKind::Investment)
    {
        return Ok(());
    }
    for sec in &mut batch.sections {
        if sec.kind == SectionKind::Investment {
            resolve_securities(ctx, sec)?;
        }
    }
    let chart = Chart::load(ctx)?;
    let all = securities(ctx)?;
    let metas = all_metas(ctx)?;
    let window = match_window(ctx)?;
    let confirmed = confirmed_texts(ctx)?;
    let mut rows = load_rows(ctx, &batch.id)?;
    // Occurrence numbers for rows without an id.
    let mut occ: HashMap<String, usize> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut used: HashSet<TransactionId> = HashSet::new();
    // Shares the batch adds or removes before each row (per security key), in date order.
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by_key(|i| rows.get(*i).map(|r| (r.section, r.row.date, r.index)));
    let mut batch_delta: HashMap<(usize, String), Decimal> = HashMap::new();
    for i in order {
        let Some(r) = rows.get_mut(i) else { continue };
        let Some(sec) = batch.sections.get(r.section) else {
            continue;
        };
        r.notes = r.row.notes.clone();
        r.needs_review = r.row.needs_review;
        r.link = None;
        r.proposal = None;
        let Some(target) = sec.account else {
            r.class = "pending".into();
            r.action = "skip".into();
            r.fingerprint = None;
            continue;
        };
        let key_material = format!(
            "{}|{}|{}|{:?}|{:?}",
            r.row.date,
            action_word(r.row.action),
            r.row.security.as_deref().unwrap_or(""),
            r.row.quantity,
            r.row.amount
        );
        let n = occ
            .entry(format!("{}|{key_material}", r.section))
            .or_insert(0);
        *n += 1;
        let fp = fingerprint(target, &r.row, *n);
        r.fingerprint = Some(fp.clone());
        // 1. Duplicate.
        if seen.contains(&fp) || fingerprint_txn(ctx, &chart, target, &fp)?.is_some() {
            r.class = "duplicate".into();
            r.action = "skip".into();
            continue;
        }
        seen.insert(fp);
        // 3. Reverse.
        if let Some(orig) = &r.row.reversal_of {
            let ofp = format!("ext:{target}:{orig}");
            match fingerprint_txn(ctx, &chart, target, &ofp)? {
                Some(t) => {
                    r.class = "reverse".into();
                    r.action = "void".into();
                    r.link = Some(t);
                }
                None => {
                    r.class = "reverse".into();
                    r.action = "skip".into();
                    r.notes.push(format!(
                        "the reversed transaction {orig} is not in the book"
                    ));
                }
            }
            continue;
        }
        // 4. Unsupported.
        let res = res_of(sec, r.row.security.as_deref());
        let why = r.row.unsupported.clone().or_else(|| {
            res.filter(|x| x.how == "unsupported")
                .map(|_| "options are not supported".to_owned())
        });
        if let Some(w) = why {
            r.class = "unsupported".into();
            r.action = "skip".into();
            if !r.notes.contains(&w) {
                r.notes.insert(0, w);
            }
            continue;
        }
        let action = action_of(r.row.action);
        let existing_sec = res.and_then(|x| x.security.clone());
        // 2. Match.
        let mut matched = None;
        if let (Some(a), Some(sid)) = (action, &existing_sec) {
            for m in metas.values() {
                if m.account != Some(target)
                    || m.action != a
                    || m.security.as_deref() != Some(sid.as_str())
                    || used.contains(&m.txn)
                    || (m.date.days_until(r.row.date)).abs() > window
                    || m.quantity != r.row.quantity
                    || m.amount != r.row.amount
                {
                    continue;
                }
                let Some(t) = ctx.tx.transaction(m.txn)? else {
                    continue;
                };
                if t.status != TxnStatus::Posted || t.splits.iter().any(|s| s.fingerprint.is_some())
                {
                    continue;
                }
                matched = Some(t.id);
                break;
            }
        }
        if let Some(t) = matched {
            used.insert(t);
            r.class = "match".into();
            r.action = "match".into();
            r.link = Some(t);
            continue;
        }
        // 5. New.
        r.class = "new".into();
        r.action = if r.overrides.skip == Some(true) {
            "skip".into()
        } else {
            "create".into()
        };
        for n in &r.row.notes {
            if let Some(text) = n.strip_prefix(UNVERIFIED)
                && !confirmed.iter().any(|c| c.eq_ignore_ascii_case(text))
            {
                r.needs_review = true;
            }
        }
        if r.row.action == InvAction::SharesIn
            && r.row.basis.is_none()
            && r.overrides.basis.is_none()
        {
            r.needs_review = true;
        }
        // Corporate actions always need review (files rarely carry their terms).
        if matches!(
            r.row.action,
            InvAction::Merger
                | InvAction::Spinoff
                | InvAction::SymbolChange
                | InvAction::CashInLieu
        ) {
            r.needs_review = true;
        }
        if r.row.action == InvAction::Split
            && r.row.ratio.is_none()
            && (r.overrides.ratio_new.is_none() || r.overrides.ratio_old.is_none())
        {
            r.needs_review = true;
        }
        // Shares available for a sell or move out.
        if let (Some(key), true) = (
            r.row.security.clone(),
            matches!(
                r.row.action,
                InvAction::Sell | InvAction::SharesOut | InvAction::CashInLieu
            ),
        ) {
            let book = match existing_sec
                .as_ref()
                .and_then(|id| all.iter().find(|s| s.id == *id))
                .and_then(|s| sec_account_of(&chart, target, &s.symbol))
            {
                Some(acc) => held_on(ctx, &[acc], r.row.date)?
                    .get(&acc)
                    .copied()
                    .unwrap_or_default(),
                None => Decimal::ZERO,
            };
            let before = batch_delta
                .get(&(r.section, key.clone()))
                .copied()
                .unwrap_or_default();
            let available = book.checked_add(before).unwrap_or(book);
            let q = r.row.quantity.unwrap_or_default();
            if q > available && r.action == "create" {
                let short = q.checked_sub(available).unwrap_or(q);
                let per100 = res.is_some_and(|x| kind_of_res(x, &all).per_hundred());
                r.proposal = Some(MoveIn {
                    quantity: short,
                    basis: per100.then_some(short),
                });
                if r.overrides.move_in != Some(true) {
                    r.needs_review = true;
                    r.notes.push(format!(
                        "you hold {} on that date: Move shares in first",
                        format_amount(available, None)
                    ));
                }
            }
        }
        if r.action == "create"
            && let Some(key) = &r.row.security
        {
            let q = r.row.quantity.unwrap_or_default();
            let slot = batch_delta.entry((r.section, key.clone())).or_default();
            match r.row.action {
                InvAction::Buy | InvAction::Reinvest | InvAction::SharesIn => {
                    *slot = slot.checked_add(q).unwrap_or(*slot);
                }
                InvAction::Sell | InvAction::SharesOut => {
                    let moved_in = if r.overrides.move_in == Some(true) {
                        r.proposal.as_ref().map_or(Decimal::ZERO, |p| p.quantity)
                    } else {
                        Decimal::ZERO
                    };
                    *slot = slot
                        .checked_add(moved_in)
                        .and_then(|x| x.checked_sub(q))
                        .unwrap_or(*slot);
                }
                _ => {}
            }
        }
    }
    for r in &rows {
        save_row(ctx, r)?;
    }
    crate::import::save_batch(ctx, batch)
}

// ----- review commands ---------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetSecurityArgs {
    batch: String,
    #[serde(default)]
    section: usize,
    /// The file's source security key.
    key: String,
    /// Use this existing security (id or symbol).
    #[serde(default)]
    security: Option<String>,
    /// Or record it as a new security with these details.
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    basis_method: Option<String>,
    /// Forget the choice (back to the automatic resolution).
    #[serde(default)]
    reset: bool,
}

fn set_security(ctx: &Ctx<'_>, a: SetSecurityArgs) -> CommandResult<Value> {
    let mut batch = crate::import::staged(ctx, &a.batch)?;
    let sec = batch
        .sections
        .get_mut(a.section)
        .ok_or_else(|| invalid(format!("the batch has no section {}", a.section)))?;
    let r = sec
        .securities
        .iter_mut()
        .find(|r| r.source.key == a.key)
        .ok_or_else(|| CommandError::NotFound(format!("security {} in the file", a.key)))?;
    if a.reset {
        r.choice = None;
    } else if let Some(s) = &a.security {
        r.choice = Some(SecChoice::Existing {
            security: find_security(ctx, s)?.id,
        });
    } else {
        let base = r.proposed.clone().unwrap_or(NewSec {
            symbol: String::new(),
            name: String::new(),
            kind: "stock".into(),
            basis_method: "fifo".into(),
        });
        let n = NewSec {
            symbol: a
                .symbol
                .map_or(base.symbol, |s| s.trim().to_ascii_uppercase()),
            name: a.name.map_or(base.name, |s| s.trim().to_owned()),
            kind: a.kind.unwrap_or(base.kind),
            basis_method: a.basis_method.unwrap_or(base.basis_method),
        };
        parse_kind(&n.kind)?;
        parse_method(&n.basis_method)?;
        r.choice = Some(SecChoice::New(n));
    }
    classify(ctx, &mut batch)?;
    crate::import::sealed_json(ctx, &batch.id)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetInvRowArgs {
    batch: String,
    row: String,
    #[serde(default)]
    skip: Option<bool>,
    /// Shares in: the lot's total basis.
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    basis: Option<Decimal>,
    #[serde(default)]
    acquired: Option<Date>,
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    ratio_new: Option<Decimal>,
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    ratio_old: Option<Decimal>,
    /// Accept (true) or decline (false) the Move shares in first proposal.
    #[serde(default)]
    move_in: Option<bool>,
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    move_in_basis: Option<Decimal>,
    #[serde(default)]
    move_in_acquired: Option<Date>,
    /// Corporate actions: the missing terms (e.g. new_security, ratio_new, ratio_old,
    /// mode for a merger; new_security, new_per_parent, allocation_percent for a
    /// spin-off; new_symbol for a symbol change).
    #[serde(default)]
    terms: Option<Value>,
}

fn set_inv_row(ctx: &Ctx<'_>, a: SetInvRowArgs) -> CommandResult<Value> {
    let mut batch = crate::import::staged(ctx, &a.batch)?;
    let d = ctx
        .tx
        .get_doc(DocTable::InvImportRow, &a.row)?
        .filter(|d| d.grp.as_deref() == Some(a.batch.as_str()))
        .ok_or_else(|| CommandError::NotFound(format!("row {} in batch {}", a.row, a.batch)))?;
    let mut r: InvRowDoc = serde_json::from_value(d.json).map_err(bad_doc)?;
    let o = &mut r.overrides;
    if a.skip.is_some() {
        o.skip = a.skip;
    }
    if a.basis.is_some() {
        o.basis = a.basis;
    }
    if a.acquired.is_some() {
        o.acquired = a.acquired;
    }
    if a.ratio_new.is_some() {
        o.ratio_new = a.ratio_new;
    }
    if a.ratio_old.is_some() {
        o.ratio_old = a.ratio_old;
    }
    if a.move_in.is_some() {
        o.move_in = a.move_in;
    }
    if a.move_in_basis.is_some() {
        o.move_in_basis = a.move_in_basis;
    }
    if a.move_in_acquired.is_some() {
        o.move_in_acquired = a.move_in_acquired;
    }
    if a.terms.is_some() {
        o.terms = a.terms;
    }
    save_row(ctx, &r)?;
    classify(ctx, &mut batch)?;
    crate::import::sealed_json(ctx, &batch.id)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HoldingRowArgs {
    batch: String,
    /// The security of the difference (its symbol or source key).
    security: String,
}

/// connectivity-brokerage.md part C: MoneyKeep never adjusts holdings by itself; the
/// review offers this row for the difference, which the user checks (basis asked).
fn add_holding_row(ctx: &Ctx<'_>, a: HoldingRowArgs) -> CommandResult<Value> {
    let mut batch = crate::import::staged(ctx, &a.batch)?;
    let feed = batch
        .feed
        .clone()
        .ok_or_else(|| CommandError::Rejected("this batch did not come from a feed".into()))?;
    let rows = load_rows(ctx, &batch.id)?;
    let mut found = None;
    for sec in batch
        .sections
        .iter()
        .filter(|s| s.kind == SectionKind::Investment)
    {
        let Some(account) = sec.account else { continue };
        for l in crate::connect::links_of(ctx, &feed)? {
            if l.account.as_deref() != Some(account.to_string().as_str()) {
                continue;
            }
            let diffs = l
                .remote
                .as_ref()
                .and_then(|r| r.holdings.as_ref())
                .filter(|h| h.get("batch").and_then(Value::as_str) == Some(batch.id.as_str()))
                .and_then(|h| h.get("differences").and_then(Value::as_array).cloned())
                .unwrap_or_default();
            if let Some(d) = diffs.iter().find(|d| {
                d.get("security").and_then(Value::as_str) == Some(a.security.as_str())
                    || d.get("key").and_then(Value::as_str) == Some(a.security.as_str())
            }) {
                found = Some((sec.index, sec.as_of, d.clone()));
            }
        }
    }
    let (si, as_of, d) = found.ok_or_else(|| {
        CommandError::NotFound(format!(
            "a holdings difference for {} in this batch",
            a.security
        ))
    })?;
    let diff: Decimal = d
        .get("difference")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .replace(',', "")
        .parse()
        .map_err(|_| CommandError::Integrity("bad holdings difference".into()))?;
    let key = d
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let date = as_of.map_or_else(mk_store::today_utc, Ok)?;
    let incoming = diff > Decimal::ZERO;
    let index = rows
        .iter()
        .filter(|r| r.section == si)
        .map(|r| r.index + 1)
        .max()
        .unwrap_or(0);
    let note = if incoming {
        "Added for the holdings difference: enter the basis of these shares"
    } else {
        "Added for the holdings difference: check which lots leave"
    };
    save_row(
        ctx,
        &InvRowDoc {
            id: mk_core::Id::<()>::new().to_string(),
            batch: batch.id.clone(),
            section: si,
            index,
            row: InvRow {
                external_id: None,
                date,
                action: if incoming {
                    InvAction::SharesIn
                } else {
                    InvAction::SharesOut
                },
                security: Some(key),
                quantity: Some(diff.abs()),
                price: None,
                commission: Decimal::ZERO,
                amount: None,
                kind: None,
                ratio: None,
                basis: None,
                acquired: None,
                expense_account: None,
                memo: "Holdings difference".into(),
                reversal_of: None,
                unsupported: None,
                needs_review: true,
                notes: vec![note.into()],
                line: 0,
                raw: "holdings difference".into(),
                cleaned: Vec::new(),
            },
            fingerprint: None,
            class: "pending".into(),
            action: "skip".into(),
            link: None,
            needs_review: true,
            notes: Vec::new(),
            overrides: InvOverrides::default(),
            proposal: None,
            result: None,
        },
    )?;
    classify(ctx, &mut batch)?;
    crate::import::sealed_json(ctx, &batch.id)
}

// ----- commit -------------------------------------------------------------------------

/// Counts from the investment part of a commit.
#[derive(Debug, Default)]
pub(crate) struct Committed {
    pub created: usize,
    pub matched: usize,
    pub voided: usize,
    pub skipped: usize,
}

fn cleared_fingerprint(
    ctx: &Ctx<'_>,
    txn: TransactionId,
    chart: &Chart,
    target: AccountId,
    fp: &str,
) -> CommandResult<()> {
    let old = ctx
        .tx
        .transaction(txn)?
        .ok_or_else(|| CommandError::Integrity("imported transaction missing".into()))?;
    let mut new = old.clone();
    let subtree = chart.subtree(target);
    let pick = new
        .splits
        .iter()
        .position(|s| s.account == target)
        .or_else(|| new.splits.iter().position(|s| subtree.contains(&s.account)));
    let Some(i) = pick else {
        return Ok(());
    };
    if let Some(s) = new.splits.get_mut(i) {
        if s.fingerprint.is_some() {
            return Err(CommandError::Rejected(format!(
                "transaction {txn} changed since staging; re-stage the file"
            )));
        }
        s.fingerprint = Some(fp.to_owned());
        if s.reconcile == Reconcile::New {
            s.reconcile = Reconcile::Cleared;
        }
    }
    validate_change(ctx, Some(&old), Some(&new))?;
    ctx.save_txn(Some(&old), &new)
}

fn row_error(e: CommandError, r: &InvRowDoc) -> CommandError {
    let line = r.row.line;
    match e {
        CommandError::Coded { code, message } => CommandError::Coded {
            code,
            message: format!("row at line {line}: {message}"),
        },
        CommandError::InvalidArgs(m) => {
            CommandError::InvalidArgs(format!("row at line {line}: {m}"))
        }
        CommandError::Rejected(m) => CommandError::Rejected(format!("row at line {line}: {m}")),
        other => other,
    }
}

/// Commit the investment rows of a batch (called by `import.commit`, same command).
pub(crate) fn commit(ctx: &Ctx<'_>, batch: &mut BatchDoc) -> CommandResult<Committed> {
    let mut out = Committed::default();
    if !batch
        .sections
        .iter()
        .any(|s| s.kind == SectionKind::Investment)
    {
        return Ok(out);
    }
    let mut rows = load_rows(ctx, &batch.id)?;
    let store_prices = setting_bool(ctx, STORE_PRICES_KEY, true)?;
    let mut confirmed = confirmed_texts(ctx)?;
    // Securities: create the proposed ones that rows or positions need; remember every
    // resolution.
    let mut ids: HashMap<(usize, String), String> = HashMap::new();
    for sec in batch
        .sections
        .iter_mut()
        .filter(|s| s.kind == SectionKind::Investment)
    {
        let needed: HashSet<String> = rows
            .iter()
            .filter(|r| r.section == sec.index && matches!(r.action.as_str(), "create" | "match"))
            .filter_map(|r| r.row.security.clone())
            .chain(sec.positions.iter().map(|p| p.security.clone()))
            .collect();
        for r in &mut sec.securities {
            let id = match (&r.security, &r.proposed) {
                (Some(id), _) => Some(id.clone()),
                (None, Some(p)) if needed.contains(&r.source.key) => {
                    if let Some(problem) = &r.problem {
                        return Err(CommandError::Rejected(format!(
                            "security {}: {problem}",
                            r.source.key
                        )));
                    }
                    let mut s = create_security(
                        ctx,
                        &p.symbol,
                        &p.name,
                        parse_kind(&p.kind)?,
                        Some(parse_method(&p.basis_method)?),
                        r.source.cusip.clone(),
                    )?;
                    if r.source.figi.is_some() {
                        s.figi.clone_from(&r.source.figi);
                        crate::investments::save_security(ctx, &s)?;
                    }
                    Some(s.id)
                }
                _ => None,
            };
            if let Some(id) = id {
                if r.security.is_none() {
                    r.security = Some(id.clone());
                    r.how = "created".into();
                    r.proposed = None;
                }
                ids.insert((sec.index, r.source.key.clone()), id.clone());
                let key = r.source.key.clone();
                let doc = Doc {
                    id: key.clone(),
                    grp: None,
                    ord: 0,
                    json: json!({"key": key, "security": id}),
                };
                if ctx.tx.get_doc(DocTable::ImportSecurityMap, &key)?.as_ref() != Some(&doc) {
                    ctx.save_doc(DocTable::ImportSecurityMap, &key, Some(&doc))?;
                }
                if store_prices {
                    let s = find_security(ctx, &id)?;
                    let existing = prices_of(ctx, &s.id)?;
                    let mut prices: Vec<(Date, Decimal)> = Vec::new();
                    if let (Some(p), Some(d)) = (r.source.price, r.source.price_date) {
                        prices.push((d, p));
                    }
                    for pos in sec.positions.iter().filter(|p| p.security == key) {
                        if let (Some(p), Some(d)) = (pos.price, pos.as_of) {
                            prices.push((d, p));
                        }
                    }
                    for (d, p) in prices {
                        if p > Decimal::ZERO && !existing.contains_key(&d) {
                            store_price(ctx, &s, d, p.normalize())?;
                        }
                    }
                }
            }
        }
    }
    // Rows in date order, then file order.
    rows.sort_by_key(|r| (r.row.date, r.section, r.index));
    let chart = Chart::load(ctx)?;
    for r in &mut rows {
        let Some(sec) = batch.sections.get(r.section) else {
            continue;
        };
        let Some(target) = sec.account else { continue };
        let Some(fp) = r.fingerprint.clone() else {
            continue;
        };
        if fingerprint_txn(ctx, &chart, target, &fp)?.is_some() {
            out.skipped += 1;
            r.result = Some(json!({"skipped": true}));
            continue;
        }
        match r.action.as_str() {
            "match" => {
                let t = r
                    .link
                    .ok_or_else(|| CommandError::Integrity("match without a link".into()))?;
                cleared_fingerprint(ctx, t, &chart, target, &fp)?;
                out.matched += 1;
                r.result = Some(json!({"matched": t}));
            }
            "void" => {
                let t = r
                    .link
                    .ok_or_else(|| CommandError::Integrity("void without a link".into()))?;
                let old = ctx.tx.transaction(t)?.ok_or_else(|| {
                    CommandError::Rejected(
                        "the transaction to void no longer exists; re-stage".into(),
                    )
                })?;
                if old.status == TxnStatus::Posted {
                    let mut new = old.clone();
                    new.status = TxnStatus::Voided;
                    new.void_reason = Some("reversed by the broker".into());
                    validate_change(ctx, Some(&old), Some(&new)).map_err(|e| row_error(e, r))?;
                    ctx.save_txn(Some(&old), &new)?;
                    crate::investments::after_plain_change(ctx, &old, false)
                        .map_err(|e| row_error(e, r))?;
                }
                out.voided += 1;
                r.result = Some(json!({"voided": t}));
            }
            "create"
                if matches!(
                    r.row.action,
                    InvAction::Merger | InvAction::Spinoff | InvAction::SymbolChange
                ) =>
            {
                let sec_id = r
                    .row
                    .security
                    .as_ref()
                    .and_then(|k| ids.get(&(r.section, k.clone())).cloned())
                    .ok_or_else(|| row_error(invalid("choose the security"), r))?;
                let terms = r.overrides.terms.clone().ok_or_else(|| {
                    row_error(invalid("enter the terms of this corporate action"), r)
                })?;
                let id = crate::corporate::imported(ctx, r.row.action, r.row.date, &sec_id, terms)
                    .map_err(|e| row_error(e, r))?;
                if let Some(id) = id {
                    let chart = Chart::load(ctx)?;
                    cleared_fingerprint(ctx, id, &chart, target, &fp)?;
                }
                out.created += 1;
                r.result = Some(json!({"created": id}));
            }
            "create" => {
                let action = action_of(r.row.action).ok_or_else(|| {
                    CommandError::Integrity("unsupported row marked create".into())
                })?;
                let sec_id = match &r.row.security {
                    Some(k) => ids.get(&(r.section, k.clone())).cloned(),
                    None => None,
                };
                // Move shares in first.
                if r.overrides.move_in == Some(true)
                    && let (Some(p), Some(sid)) = (r.proposal.clone(), &sec_id)
                {
                    let basis = r.overrides.move_in_basis.or(p.basis).ok_or_else(|| {
                        row_error(invalid("enter the basis of the shares moved in"), r)
                    })?;
                    let s = find_security(ctx, sid)?;
                    let acquired = match r.overrides.move_in_acquired {
                        Some(d) => d,
                        None => first_date(ctx, &chart, target, &s)?
                            .unwrap_or(r.row.date)
                            .previous_day()
                            .unwrap_or(r.row.date),
                    };
                    let args = ActionArgs {
                        account: Some(target.to_string()),
                        security: Some(sid.clone()),
                        date: Some(acquired.min(r.row.date)),
                        quantity: Some(p.quantity),
                        lots: Some(vec![LotArg {
                            lot: None,
                            acquired: Some(acquired),
                            quantity: p.quantity,
                            basis: Some(basis),
                            ..LotArg::default()
                        }]),
                        memo: Some(
                            "Moved in before an imported sale (acquired date assumed)".into(),
                        ),
                        ..ActionArgs::default()
                    };
                    post(ctx, Action::SharesIn, args, None).map_err(|e| row_error(e, r))?;
                }
                let args = action_args(ctx, r, target, sec_id.clone())?;
                let v = post(ctx, action, args, None).map_err(|e| row_error(e, r))?;
                // The trade price as that day's price, unless one exists (revision 4).
                if store_prices
                    && matches!(action, Action::Buy | Action::Sell | Action::Reinvest)
                    && let (Some(p), Some(sid)) =
                        (r.row.price.filter(|p| *p > Decimal::ZERO), &sec_id)
                {
                    let s = find_security(ctx, sid)?;
                    if !prices_of(ctx, &s.id)?.contains_key(&r.row.date) {
                        store_price(ctx, &s, r.row.date, p.normalize())?;
                    }
                }
                let id: TransactionId = v
                    .get("transaction")
                    .and_then(|t| t.get("id"))
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| {
                        CommandError::Integrity("posted transaction without id".into())
                    })?;
                let chart = Chart::load(ctx)?;
                cleared_fingerprint(ctx, id, &chart, target, &fp)?;
                for n in &r.row.notes {
                    if let Some(text) = n.strip_prefix(UNVERIFIED)
                        && !confirmed.iter().any(|c| c.eq_ignore_ascii_case(text))
                    {
                        confirmed.push(text.to_owned());
                    }
                }
                out.created += 1;
                r.result = Some(json!({"created": id}));
            }
            _ => {
                out.skipped += 1;
                r.result = Some(json!({"skipped": true}));
            }
        }
    }
    for r in &rows {
        save_row(ctx, r)?;
    }
    if confirmed != confirmed_texts(ctx)? {
        ctx.save_setting("core", CONFIRMED_KEY, Some(&json!(confirmed)))?;
    }
    Ok(out)
}

/// The first transaction date in the book for a security in an account.
fn first_date(
    ctx: &Ctx<'_>,
    chart: &Chart,
    target: AccountId,
    s: &Security,
) -> CommandResult<Option<Date>> {
    let Some(acc) = sec_account_of(chart, target, &s.symbol) else {
        return Ok(None);
    };
    Ok(ctx
        .tx
        .posted_splits(None)?
        .iter()
        .filter(|r| r.account == acc)
        .map(|r| r.date)
        .min())
}

fn action_args(
    ctx: &Ctx<'_>,
    r: &InvRowDoc,
    target: AccountId,
    sec_id: Option<String>,
) -> CommandResult<ActionArgs> {
    let row = &r.row;
    let mut a = ActionArgs {
        account: Some(target.to_string()),
        security: sec_id,
        date: Some(row.date),
        memo: Some(row.memo.clone()).filter(|m| !m.trim().is_empty()),
        ..ActionArgs::default()
    };
    let need_amount = || {
        row.amount
            .ok_or_else(|| row_error(invalid("the row has no amount"), r))
    };
    match row.action {
        InvAction::Buy | InvAction::Sell | InvAction::CashInLieu => {
            a.quantity = row.quantity;
            a.price = row.price;
            a.commission = Some(row.commission);
            a.amount = row.amount;
        }
        InvAction::Dividend | InvAction::Interest | InvAction::ReturnOfCapital => {
            a.amount = Some(need_amount()?);
            if row.action == InvAction::Dividend {
                a.kind = row
                    .kind
                    .as_deref()
                    .and_then(|k| serde_json::from_value(json!(k)).ok());
            }
        }
        InvAction::Fee => {
            a.amount = Some(need_amount()?);
            if let Some(name) = &row.expense_account {
                let mut chart = Chart::load(ctx)?;
                let id = crate::investments::ensure_top(
                    ctx,
                    &mut chart,
                    name,
                    AccountType::Expense,
                    AccountSubtype::Expense,
                )?;
                a.expense_account = Some(id.to_string());
                a.security = None;
            }
        }
        InvAction::Reinvest => {
            a.quantity = row.quantity;
            a.price = row.price;
            a.amount = row.amount;
            a.kind = row
                .kind
                .as_deref()
                .and_then(|k| serde_json::from_value(json!(k)).ok());
        }
        InvAction::SharesIn => {
            let q = row
                .quantity
                .ok_or_else(|| row_error(invalid("the row has no quantity"), r))?;
            let basis = r.overrides.basis.or(row.basis).ok_or_else(|| {
                row_error(invalid("enter the cost basis of the shares moved in"), r)
            })?;
            a.quantity = Some(q);
            a.lots = Some(vec![LotArg {
                lot: None,
                acquired: r.overrides.acquired.or(row.acquired).or(Some(row.date)),
                quantity: q,
                basis: Some(basis),
                ..LotArg::default()
            }]);
        }
        InvAction::SharesOut => {
            a.quantity = row.quantity;
        }
        InvAction::Split => {
            let (n, o) = match (r.overrides.ratio_new, r.overrides.ratio_old, row.ratio) {
                (Some(n), Some(o), _) => (n, o),
                (_, _, Some((n, o))) => (n, o),
                _ => return Err(row_error(invalid("enter the split ratio"), r)),
            };
            a.account = None;
            a.ratio_new = Some(n);
            a.ratio_old = Some(o);
        }
        InvAction::Unsupported
        | InvAction::Merger
        | InvAction::Spinoff
        | InvAction::SymbolChange => {}
    }
    Ok(a)
}

// ----- feeds (connectivity-brokerage.md part C) -------------------------------------------

/// An existing security a source security names (mapping, CUSIP, FIGI, ticker).
fn existing_security(
    ctx: &Ctx<'_>,
    all: &[Security],
    key: &str,
    src: Option<&SourceSecurity>,
) -> CommandResult<Option<Security>> {
    if let Some(id) = mapped_security(ctx, key)?
        && let Some(s) = all.iter().find(|s| s.id == id)
    {
        return Ok(Some(s.clone()));
    }
    if let Some(src) = src {
        if let Some(c) = &src.cusip
            && let Some(s) = all.iter().find(|s| s.cusip.as_deref() == Some(c.as_str()))
        {
            return Ok(Some(s.clone()));
        }
        if let Some(f) = &src.figi
            && let Some(s) = all.iter().find(|s| s.figi.as_deref() == Some(f.as_str()))
        {
            return Ok(Some(s.clone()));
        }
    }
    let ticker = src
        .and_then(|s| s.ticker.clone())
        .or_else(|| key.strip_prefix("TICKER:").map(str::to_owned));
    Ok(ticker.and_then(|t| all.iter().find(|s| s.answers_to(&t)).cloned()))
}

/// Store the prices a feed reports for securities already in the book (the source is
/// noted with each price; an existing price for the date is kept).
pub(crate) fn store_feed_prices(ctx: &Ctx<'_>, st: &Statement, source: &str) -> CommandResult<()> {
    if !is_enabled(ctx)? {
        return Ok(());
    }
    let all = securities(ctx)?;
    for sec in st
        .sections
        .iter()
        .filter(|s| s.kind == SectionKind::Investment)
    {
        for p in &sec.positions {
            let (Some(price), Some(date)) = (p.price, p.as_of) else {
                continue;
            };
            if price <= Decimal::ZERO || date.year() <= 1990 {
                continue;
            }
            let src = sec.securities.iter().find(|s| s.key == p.security);
            let Some(s) = existing_security(ctx, &all, &p.security, src)? else {
                continue;
            };
            if !prices_of(ctx, &s.id)?.contains_key(&date) {
                crate::investments::store_price_from(
                    ctx,
                    &s,
                    date,
                    price.normalize(),
                    Some(source),
                )?;
            }
        }
    }
    Ok(())
}

/// The book's quantity of a security in a brokerage account before a date.
pub(crate) fn book_quantity_before(
    ctx: &Ctx<'_>,
    account: AccountId,
    key: &str,
    date: Date,
) -> CommandResult<Option<Decimal>> {
    let all = securities(ctx)?;
    let Some(s) = existing_security(ctx, &all, key, None)? else {
        return Ok(None);
    };
    let chart = Chart::load(ctx)?;
    let Some(acc) = sec_account_of(&chart, account, &s.symbol) else {
        return Ok(None);
    };
    let Some(before) = date.add_days(-1) else {
        return Ok(None);
    };
    let q = ctx
        .tx
        .posted_splits(Some(before))?
        .iter()
        .filter(|r| r.account == acc)
        .fold(Decimal::ZERO, |a, r| a.checked_add(r.quantity).unwrap_or(a));
    Ok(Some(q))
}

/// Units beyond which a holding differs.
const HOLDING_TOLERANCE: &str = "0.0001";

fn plus(a: Decimal, b: Decimal) -> Decimal {
    a.checked_add(b).unwrap_or(a)
}

/// The holdings check of a feed batch: per investment section, the provider's
/// quantities and cash against MoneyKeep's after the batch would be committed. Each
/// result is kept on the connection's remote account (Home and `connect.holdings_check`).
pub(crate) fn feed_holdings(
    ctx: &Ctx<'_>,
    conn: &crate::connect::ConnDoc,
    batch_id: &str,
    provider: &str,
) -> CommandResult<Value> {
    if !is_enabled(ctx)? {
        return Ok(Value::Array(Vec::new()));
    }
    let batch = crate::import::load_batch(ctx, batch_id)?;
    let rows = load_rows(ctx, batch_id)?;
    let cash_rows = crate::import::load_rows(ctx, batch_id)?;
    let chart = Chart::load(ctx)?;
    let all = securities(ctx)?;
    let tolerance: Decimal = HOLDING_TOLERANCE.parse().unwrap_or_default();
    let mut out = Vec::new();
    for sec in batch
        .sections
        .iter()
        .filter(|s| s.kind == SectionKind::Investment)
    {
        let Some(account) = sec.account else { continue };
        if sec.positions.is_empty() && sec.cash_balance.is_none() {
            continue;
        }
        let Some(as_of) = sec.as_of.or(sec.period.map(|p| p.1)) else {
            continue;
        };
        let vals = valuations(ctx, &chart, as_of)?;
        let mut mine: Vec<&InvRowDoc> = rows
            .iter()
            .filter(|r| r.section == sec.index && r.action == "create")
            .collect();
        mine.sort_by_key(|r| (r.row.date, r.index));
        // The symbol of a source key: resolved, proposed, or the key itself.
        let symbol_of = |key: &str| -> String {
            let res = sec.securities.iter().find(|r| r.source.key == key);
            res.and_then(|r| r.security.as_ref())
                .and_then(|id| all.iter().find(|s| s.id == *id))
                .map(|s| s.symbol.clone())
                .or_else(|| res.and_then(|r| r.proposed.as_ref().map(|n| n.symbol.clone())))
                .unwrap_or_else(|| key.trim_start_matches("TICKER:").to_owned())
        };
        let mut keys: Vec<String> = sec.positions.iter().map(|p| p.security.clone()).collect();
        for r in mine.iter().filter(|_| !sec.positions_unread) {
            if let Some(k) = &r.row.security
                && !keys.contains(k)
            {
                keys.push(k.clone());
            }
        }
        // Securities the book holds in the account that the provider does not list.
        let listed: Vec<String> = keys.iter().map(|k| symbol_of(k)).collect();
        for child in chart.children(account).filter(|_| !sec.positions_unread) {
            if child.subtype == AccountSubtype::Security
                && !listed.iter().any(|s| s == child.commodity.as_str())
                && vals.get(&child.id).is_some_and(|v| !v.quantity.is_zero())
            {
                keys.push(format!("TICKER:{}", child.commodity));
            }
        }
        let mut lines = Vec::new();
        let mut diffs = Vec::new();
        for key in &keys {
            let symbol = symbol_of(key);
            let mut q = sec_account_of(&chart, account, &symbol)
                .and_then(|a| vals.get(&a))
                .map_or(Decimal::ZERO, |v| v.quantity);
            for r in mine
                .iter()
                .filter(|r| r.row.security.as_deref() == Some(key.as_str()))
            {
                let n = r.row.quantity.unwrap_or_default();
                q = match r.row.action {
                    InvAction::Buy | InvAction::Reinvest | InvAction::SharesIn => plus(q, n),
                    InvAction::Sell | InvAction::SharesOut => plus(q, -n),
                    InvAction::Split => match r.row.ratio {
                        Some((new, old)) if !old.is_zero() => q
                            .checked_mul(new)
                            .and_then(|x| x.checked_div(old))
                            .unwrap_or(q),
                        _ => q,
                    },
                    _ => q,
                };
            }
            let provider_q = sec
                .positions
                .iter()
                .find(|p| p.security == *key)
                .map_or(Decimal::ZERO, |p| p.quantity);
            let diff = plus(provider_q, -q);
            let differs = diff.abs() > tolerance;
            let shown = |d: Decimal| format_amount(d.normalize(), None);
            let line = json!({
                "security": symbol, "key": key, "provider": shown(provider_q),
                "book_after": shown(q), "difference": shown(diff), "differs": differs,
                "message": format!("{} holds {} {symbol}; MoneyKeep will hold {} after this import",
                    if provider == "SnapTrade" { "SnapTrade" } else { "The bank" },
                    shown(provider_q), shown(q)),
            });
            if differs {
                diffs.push(line.clone());
            }
            lines.push(line);
        }
        let cash = match &sec.cash_balance {
            Some(c) => {
                let mut book = ctx
                    .tx
                    .posted_splits(Some(as_of))?
                    .iter()
                    .filter(|r| r.account == account)
                    .fold(Decimal::ZERO, |a, r| plus(a, r.quantity));
                for r in &mine {
                    let amt = r.row.amount.unwrap_or_default();
                    book = match r.row.action {
                        InvAction::Buy => plus(book, -amt),
                        InvAction::Fee => plus(book, -amt),
                        InvAction::Sell
                        | InvAction::Dividend
                        | InvAction::Interest
                        | InvAction::ReturnOfCapital
                        | InvAction::CashInLieu => plus(book, amt),
                        _ => book,
                    };
                }
                for r in cash_rows.iter().filter(|r| {
                    r.section == sec.index && r.action == crate::import::RowAction::Create
                }) {
                    book = plus(book, r.raw.amount);
                }
                let diff = plus(c.amount, -book);
                let differs = diff.abs() >= Decimal::new(1, 2);
                json!({"provider": format_amount(c.amount, Some(2)), "book_after": format_amount(book, Some(2)),
                    "difference": format_amount(diff, Some(2)), "differs": differs})
            }
            None => Value::Null,
        };
        let cash_differs = cash.get("differs").and_then(Value::as_bool) == Some(true);
        let result = json!({
            "account": account, "account_name": chart.full_name(account), "provider": provider,
            "as_of": as_of, "batch": batch_id, "positions": lines, "differences": diffs,
            "cash": cash, "agree": diffs.is_empty() && !cash_differs,
        });
        for mut l in crate::connect::links_of(ctx, &conn.id)? {
            if l.account.as_deref() == Some(account.to_string().as_str()) {
                let mut r = l.remote.clone().unwrap_or_default();
                r.holdings = Some(result.clone());
                l.remote = Some(r);
                crate::connect::save_link_doc(ctx, &l)?;
            }
        }
        out.push(result);
    }
    Ok(Value::Array(out))
}

// ----- review output -----------------------------------------------------------------

/// The positions check of a section: the statement's holdings and cash against the
/// book's as of the statement date.
fn positions_check(ctx: &Ctx<'_>, sec: &SectionDoc) -> CommandResult<Value> {
    let (Some(account), true) = (
        sec.account,
        !sec.positions.is_empty() || sec.cash_balance.is_some(),
    ) else {
        return Ok(Value::Null);
    };
    let as_of = sec
        .as_of
        .or(sec.cash_balance.as_ref().map(|c| c.date))
        .or(sec.positions.iter().find_map(|p| p.as_of))
        .or(sec.period.map(|p| p.1));
    let Some(as_of) = as_of else {
        return Ok(Value::Null);
    };
    let chart = Chart::load(ctx)?;
    let vals = valuations(ctx, &chart, as_of)?;
    let all = securities(ctx)?;
    let mut diffs = Vec::new();
    let mut agree = true;
    let mut lines = Vec::new();
    for p in &sec.positions {
        let res = sec.securities.iter().find(|r| r.source.key == p.security);
        if res.is_some_and(|r| r.how == "unsupported") {
            continue;
        }
        let symbol = res
            .and_then(|r| r.security.as_ref())
            .and_then(|id| all.iter().find(|s| s.id == *id))
            .map(|s| s.symbol.clone());
        let shown = symbol
            .clone()
            .or_else(|| res.and_then(|r| r.proposed.as_ref().map(|n| n.symbol.clone())))
            .unwrap_or_else(|| p.security.clone());
        let book = symbol
            .as_ref()
            .and_then(|s| sec_account_of(&chart, account, s))
            .and_then(|acc| vals.get(&acc))
            .map_or(Decimal::ZERO, |v| v.quantity);
        let same = book.normalize() == p.quantity.normalize();
        agree &= same;
        let line = json!({
            "security": shown,
            "book": format_amount(book, None),
            "statement": format_amount(p.quantity, None),
            "agree": same,
        });
        if !same {
            diffs.push(line.clone());
        }
        lines.push(line);
    }
    let cash = match &sec.cash_balance {
        Some(c) => {
            let book = ctx
                .tx
                .posted_splits(Some(as_of))?
                .iter()
                .filter(|r| r.account == account)
                .fold(Decimal::ZERO, |a, r| a.checked_add(r.quantity).unwrap_or(a));
            let same = book == c.amount;
            agree &= same;
            json!({"book": format_amount(book, Some(2)), "statement": format_amount(c.amount, Some(2)), "agree": same})
        }
        None => Value::Null,
    };
    Ok(json!({
        "as_of": as_of,
        "agree": agree,
        "message": if agree { "Holdings agree with the statement" } else { "Holdings differ from the statement" },
        "positions": lines,
        "differences": diffs,
        "cash": cash,
    }))
}

/// Investment fields of a section in `import.get`.
pub(crate) fn section_extra(ctx: &Ctx<'_>, sec: &SectionDoc) -> CommandResult<Value> {
    if sec.kind != SectionKind::Investment {
        return Ok(Value::Null);
    }
    let all = securities(ctx)?;
    let secs: Vec<Value> = sec
        .securities
        .iter()
        .map(|r| {
            let existing = r
                .security
                .as_ref()
                .and_then(|id| all.iter().find(|s| s.id == *id));
            json!({
                "key": r.source.key,
                "ticker": r.source.ticker,
                "name": r.source.name,
                "cusip": r.source.cusip,
                "file_kind": r.source.kind,
                "how": r.how,
                "security": r.security,
                "symbol": existing.map(|s| s.symbol.clone()),
                "proposed": r.proposed,
                "flagged": r.flagged,
                "problem": r.problem,
            })
        })
        .collect();
    Ok(json!({
        "securities": secs,
        "positions_check": positions_check(ctx, sec)?,
    }))
}

/// Investment rows of a batch in `import.get`.
pub(crate) fn rows_json(ctx: &Ctx<'_>, batch: &BatchDoc) -> CommandResult<Vec<Value>> {
    let all = securities(ctx)?;
    let rows = load_rows(ctx, &batch.id)?;
    let mut out = Vec::new();
    for r in rows {
        let sec = batch.sections.get(r.section);
        let res = sec.and_then(|s| res_of(s, r.row.security.as_deref()));
        let symbol = res
            .and_then(|x| {
                x.security
                    .as_ref()
                    .and_then(|id| all.iter().find(|s| s.id == *id))
                    .map(|s| s.symbol.clone())
                    .or_else(|| x.proposed.as_ref().map(|p| p.symbol.clone()))
            })
            .or_else(|| res.and_then(|x| x.source.ticker.clone()));
        let mut v = serde_json::to_value(&r)?;
        if let Some(o) = v.as_object_mut() {
            o.insert("date".into(), json!(r.row.date));
            o.insert("inv_action".into(), json!(action_word(r.row.action)));
            o.insert("symbol".into(), json!(symbol));
            o.insert(
                "quantity".into(),
                json!(r.row.quantity.map(|q| format_amount(q, None))),
            );
            o.insert(
                "price".into(),
                json!(r.row.price.map(|p| p.normalize().to_string())),
            );
            o.insert(
                "amount".into(),
                json!(r.row.amount.map(|a| format_amount(a, Some(2)))),
            );
            o.insert(
                "commission".into(),
                json!(format_amount(r.row.commission, Some(2))),
            );
        }
        out.push(v);
    }
    Ok(out)
}

/// Whether every proposed security of the batch can be committed.
pub(crate) fn securities_ready(batch: &BatchDoc) -> Option<String> {
    batch
        .sections
        .iter()
        .flat_map(|s| s.securities.iter())
        .find_map(|r| {
            r.problem
                .clone()
                .map(|p| format!("security {}: {p}", r.source.key))
        })
}

// ----- profiles -----------------------------------------------------------------------

fn profile_id(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Saved and built-in investment profiles (saved first).
pub(crate) fn all_profiles(ctx: &Ctx<'_>) -> CommandResult<Vec<InvProfile>> {
    let mut out: Vec<InvProfile> = ctx
        .tx
        .docs(DocTable::InvProfile, None)?
        .into_iter()
        .map(|d| serde_json::from_value(d.json).map_err(bad_doc))
        .collect::<CommandResult<_>>()?;
    out.extend(mk_import::builtin_profiles());
    Ok(out)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProfileSaveArgs {
    /// The profile (see `inv_profile.suggest` for its shape).
    #[schemars(with = "Value")]
    profile: InvProfile,
}

fn profile_save(ctx: &Ctx<'_>, a: ProfileSaveArgs) -> CommandResult<Value> {
    let mut p = a.profile;
    p.builtin = false;
    p.name = p.name.trim().to_owned();
    mk_import::validate_profile(&p).map_err(|e| invalid(e.to_string()))?;
    if mk_import::builtin_profiles()
        .iter()
        .any(|b| b.name.eq_ignore_ascii_case(&p.name))
    {
        return Err(CommandError::Rejected(format!(
            "{} is a built-in profile; save yours under another name",
            p.name
        )));
    }
    let id = profile_id(&p.name);
    ctx.save_doc(
        DocTable::InvProfile,
        &id,
        Some(&Doc {
            id: id.clone(),
            grp: None,
            ord: 0,
            json: serde_json::to_value(&p)?,
        }),
    )?;
    Ok(serde_json::to_value(&p)?)
}

fn profile_list(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    Ok(serde_json::to_value(all_profiles(ctx)?)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProfileName {
    name: String,
}

fn find_profile(ctx: &Ctx<'_>, name: &str) -> CommandResult<InvProfile> {
    all_profiles(ctx)?
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
        .ok_or_else(|| CommandError::NotFound(format!("investment profile {name:?}")))
}

fn profile_get(ctx: &Ctx<'_>, a: ProfileName) -> CommandResult<Value> {
    Ok(serde_json::to_value(find_profile(ctx, &a.name)?)?)
}

fn profile_delete(ctx: &Ctx<'_>, a: ProfileName) -> CommandResult<Value> {
    let p = find_profile(ctx, &a.name)?;
    if p.builtin {
        return Err(CommandError::Rejected(
            "built-in profiles cannot be deleted".into(),
        ));
    }
    ctx.save_doc(DocTable::InvProfile, &profile_id(&p.name), None)?;
    Ok(json!({"deleted": p.name}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FileArgs {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    content_base64: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

fn file_bytes(a: &FileArgs) -> CommandResult<Vec<u8>> {
    use base64::Engine;
    match (&a.content, &a.content_base64) {
        (Some(c), None) => Ok(c.as_bytes().to_vec()),
        (None, Some(b)) => base64::engine::general_purpose::STANDARD
            .decode(b.trim())
            .map_err(|e| invalid(format!("content_base64 is not valid base64: {e}"))),
        _ => Err(invalid("give exactly one of content or content_base64")),
    }
}

fn profile_suggest(ctx: &Ctx<'_>, a: FileArgs) -> CommandResult<Value> {
    let _ = ctx;
    let bytes = file_bytes(&a)?;
    let s = mk_import::suggest(&bytes, a.name.as_deref()).map_err(|e| invalid(e.to_string()))?;
    Ok(serde_json::to_value(s)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PreviewArgs {
    #[schemars(with = "Value")]
    profile: InvProfile,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    content_base64: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// One neutral row for the profile editor's preview.
fn neutral(r: &InvRow) -> Value {
    json!({
        "line": r.line,
        "date": r.date,
        "action": action_word(r.action),
        "security": r.security,
        "quantity": r.quantity.map(|q| format_amount(q, None)),
        "price": r.price.map(|p| p.normalize().to_string()),
        "commission": format_amount(r.commission, Some(2)),
        "amount": r.amount.map(|a| format_amount(a, Some(2))),
        "kind": r.kind,
        "memo": r.memo,
        "unsupported": r.unsupported,
        "needs_review": r.needs_review,
        "notes": r.notes,
    })
}

pub(crate) fn preview_rows(st: &Statement) -> Vec<Value> {
    let mut rows: Vec<(usize, Value)> = Vec::new();
    for s in &st.sections {
        for r in &s.inv_rows {
            rows.push((r.line, neutral(r)));
        }
        for c in &s.rows {
            rows.push((
                c.line,
                json!({"line": c.line, "date": c.date, "action": "cash",
                       "amount": format_amount(c.amount, Some(2)), "memo": c.payee}),
            ));
        }
    }
    rows.sort_by_key(|(l, _)| *l);
    rows.into_iter().take(20).map(|(_, v)| v).collect()
}

fn profile_preview(ctx: &Ctx<'_>, a: PreviewArgs) -> CommandResult<Value> {
    let _ = ctx;
    let bytes = file_bytes(&FileArgs {
        content: a.content.clone(),
        content_base64: a.content_base64.clone(),
        name: a.name.clone(),
    })?;
    match mk_import::parse_with_profile(&bytes, &a.profile, a.name.as_deref()) {
        Ok(st) => Ok(json!({"rows": preview_rows(&st), "warnings": st.warnings, "error": null})),
        Err(e) => Ok(json!({"rows": [], "warnings": [], "error": e.to_string()})),
    }
}

/// Parse a broker file with a matching (or named) investment profile, or report that
/// it needs one.
pub(crate) enum ProfileParse {
    Parsed(Statement, String),
    NeedsProfile(Value),
    NotInvestment,
}

pub(crate) fn parse_broker_file(
    ctx: &Ctx<'_>,
    bytes: &[u8],
    format: &str,
    profile: Option<&str>,
    file_name: Option<&str>,
) -> CommandResult<ProfileParse> {
    let profiles = all_profiles(ctx)?;
    let chosen = match profile {
        Some(n) => profiles
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(n.trim()))
            .cloned(),
        None => profiles
            .iter()
            .find(|p| mk_import::profile_matches(bytes, p))
            .cloned(),
    };
    match chosen {
        Some(p) => {
            if !is_enabled(ctx)? {
                return Err(CommandError::ModuleDisabled {
                    command: "import.stage".into(),
                    module: crate::investments::INVESTMENTS.into(),
                });
            }
            let st = mk_import::parse_with_profile(bytes, &p, file_name)
                .map_err(|e| invalid(e.to_string()))?;
            Ok(ProfileParse::Parsed(st, p.name))
        }
        None if format == "csv" => Ok(ProfileParse::NotInvestment),
        None => {
            let s = mk_import::suggest(bytes, file_name).map_err(|e| invalid(e.to_string()))?;
            Ok(ProfileParse::NeedsProfile(
                json!({ "needs_inv_profile": s }),
            ))
        }
    }
}

/// `inv_profile.save` / `inv_profile.delete` from JSON arguments (definition installs).
pub(crate) fn profile_save_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    profile_save(ctx, crate::parse_args(args)?)
}

pub(crate) fn profile_delete_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    profile_delete(ctx, crate::parse_args(args)?)
}
