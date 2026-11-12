//! Classes, locations and funds (business-dimensions.md Part A): records, split tags,
//! required tags, automatic releases from restriction, releases and interfund
//! transfers, and net assets by fund.

use std::collections::BTreeMap;

use mk_core::{
    AccountId, AccountType, Date, Decimal, Split, SplitTags, Transaction, TransactionId, TxnStatus,
};
use mk_store::{Doc, DocTable, now_rfc3339};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::txns::validate_change;
use crate::{CommandError, CommandResult, Ctx, Registry, is_module_enabled};

pub(crate) const CLASSES: &str = "classes";
pub(crate) const LOCATIONS: &str = "locations";
pub(crate) const FUNDS: &str = "funds";
/// The General fund's fixed id: untagged splits belong to it.
pub(crate) const GENERAL: &str = "general";
pub(crate) const RELEASE_ACCOUNT: &str = "Net Assets Released from Restrictions";
pub(crate) const TRANSFER_ACCOUNT: &str = "Interfund Transfers";

pub(crate) fn register(r: &mut Registry) {
    r.add_in(
        CLASSES,
        "class.create",
        "Create a class",
        true,
        class_create,
    );
    r.add_in(
        CLASSES,
        "class.update",
        "Change a class",
        true,
        class_update,
    );
    r.add_in(CLASSES, "class.list", "List classes", false, class_list);
    r.add_in(
        CLASSES,
        "class.set_active",
        "Make a class active or inactive",
        true,
        class_set_active,
    );
    r.add_in(
        CLASSES,
        "class.delete",
        "Delete a class never used",
        true,
        class_delete,
    );
    r.add_in(
        LOCATIONS,
        "location.create",
        "Create a location",
        true,
        location_create,
    );
    r.add_in(
        LOCATIONS,
        "location.update",
        "Change a location",
        true,
        location_update,
    );
    r.add_in(
        LOCATIONS,
        "location.list",
        "List locations",
        false,
        location_list,
    );
    r.add_in(
        LOCATIONS,
        "location.set_active",
        "Make a location active or inactive",
        true,
        location_set_active,
    );
    r.add_in(
        LOCATIONS,
        "location.delete",
        "Delete a location never used",
        true,
        location_delete,
    );
    r.add_in(FUNDS, "fund.create", "Create a fund", true, fund_create);
    r.add_in(FUNDS, "fund.update", "Change a fund", true, fund_update);
    r.add_in(
        FUNDS,
        "fund.list",
        "List funds with their net assets",
        false,
        fund_list,
    );
    r.add_in(
        FUNDS,
        "fund.set_active",
        "Make a fund active or inactive",
        true,
        fund_set_active,
    );
    r.add_in(
        FUNDS,
        "fund.delete",
        "Delete a fund never used",
        true,
        fund_delete,
    );
    r.add_in(
        FUNDS,
        "fund.release",
        "Release net assets from a donor restriction",
        true,
        fund_release,
    );
    r.add_in(
        FUNDS,
        "fund.transfer",
        "Move net assets between funds",
        true,
        fund_transfer,
    );
    r.add_in(
        FUNDS,
        "fund.attention",
        "Restricted funds that are overspent",
        false,
        fund_attention,
    );
    r.add(
        "dimension.names",
        "Names of classes, locations and funds by id",
        false,
        dimension_names,
    );
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

fn rejected(m: impl Into<String>) -> CommandError {
    CommandError::Rejected(m.into())
}

fn add(a: Decimal, b: Decimal) -> Decimal {
    a.checked_add(b).unwrap_or(a)
}

fn sub(a: Decimal, b: Decimal) -> Decimal {
    a.checked_sub(b).unwrap_or(a)
}

// ----- records ------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DimRecord {
    pub id: String,
    /// `class`, `location` or `fund`.
    pub dim: String,
    /// Full name with `:` for levels.
    pub name: String,
    pub active: bool,
    /// Classes: `program`, `management_general` or `fundraising`.
    #[serde(default)]
    pub function: Option<String>,
    /// Funds: `without_donor_restrictions` or `with_donor_restrictions`.
    #[serde(default)]
    pub restriction: Option<String>,
    #[serde(default)]
    pub purpose: Option<String>,
    pub created_at: String,
}

impl DimRecord {
    pub(crate) fn restricted(&self) -> bool {
        self.restriction.as_deref() == Some("with_donor_restrictions")
    }
}

fn general_record() -> DimRecord {
    DimRecord {
        id: GENERAL.into(),
        dim: "fund".into(),
        name: "General".into(),
        active: true,
        function: None,
        restriction: Some("without_donor_restrictions".into()),
        purpose: None,
        created_at: String::new(),
    }
}

/// Records of a dimension, sorted by name; funds always include General.
pub(crate) fn records(ctx: &Ctx<'_>, dim: &str) -> CommandResult<Vec<DimRecord>> {
    let mut v: Vec<DimRecord> = ctx
        .tx
        .docs(DocTable::Dimension, Some(dim))?
        .into_iter()
        .map(|d| serde_json::from_value(d.json))
        .collect::<Result<_, _>>()?;
    if dim == "fund" && !v.iter().any(|r| r.id == GENERAL) {
        v.push(general_record());
    }
    v.sort_by_key(|r| r.name.to_lowercase());
    Ok(v)
}

/// Create the General fund record (on enabling the module and on first fund use).
pub(crate) fn ensure_general(ctx: &Ctx<'_>) -> CommandResult<()> {
    if ctx.tx.get_doc(DocTable::Dimension, GENERAL)?.is_none() {
        let mut g = general_record();
        g.created_at = now_rfc3339()?;
        save_record(ctx, &g)?;
    }
    Ok(())
}

fn save_record(ctx: &Ctx<'_>, r: &DimRecord) -> CommandResult<()> {
    ctx.save_doc(
        DocTable::Dimension,
        &r.id,
        Some(&Doc {
            id: r.id.clone(),
            grp: Some(r.dim.clone()),
            ord: 0,
            json: serde_json::to_value(r)?,
        }),
    )
}

pub(crate) fn module_of(dim: &str) -> &'static str {
    match dim {
        "class" => CLASSES,
        "location" => LOCATIONS,
        "project" => crate::projects::MODULE,
        _ => FUNDS,
    }
}

fn find(ctx: &Ctx<'_>, dim: &str, r: &str) -> CommandResult<DimRecord> {
    let r = r.trim();
    records(ctx, dim)?
        .into_iter()
        .find(|x| x.id == r || x.name.eq_ignore_ascii_case(r))
        .ok_or_else(|| CommandError::NotFound(format!("{dim} {r:?}")))
}

fn clean_name(name: &str) -> CommandResult<String> {
    let parts: Vec<String> = name
        .split(':')
        .map(|p| mk_core::text::clean(p, usize::MAX).text.trim().to_owned())
        .collect();
    if parts.iter().any(String::is_empty) {
        return Err(invalid("name: each level needs text (Parent:Child)"));
    }
    let n = parts.join(":");
    if n.chars().count() > 100 {
        return Err(invalid("name: at most 100 characters"));
    }
    Ok(n)
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DimArgs {
    /// The record to change (id or name); update only.
    #[serde(default, alias = "class", alias = "location", alias = "fund")]
    record: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// Classes: program, management_general or fundraising ("" clears it).
    #[serde(default)]
    function: Option<String>,
    /// Funds: without_donor_restrictions or with_donor_restrictions.
    #[serde(default)]
    restriction: Option<String>,
    /// Funds: purpose (at most 1,000 characters).
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    active: Option<bool>,
}

fn apply(ctx: &Ctx<'_>, r: &mut DimRecord, a: DimArgs) -> CommandResult<()> {
    if let Some(n) = a.name {
        let n = clean_name(&n)?;
        if records(ctx, &r.dim)?
            .iter()
            .any(|x| x.id != r.id && x.name.eq_ignore_ascii_case(&n))
        {
            return Err(rejected(format!("a {} named {n:?} already exists", r.dim)));
        }
        if r.id == GENERAL && n != "General" {
            return Err(rejected("the General fund keeps its name"));
        }
        r.name = n;
    }
    if let Some(f) = a.function {
        if r.dim != "class" {
            return Err(invalid("only classes have a function"));
        }
        r.function = match f.trim() {
            "" => None,
            "program" | "management_general" | "fundraising" => Some(f.trim().to_owned()),
            _ => {
                return Err(invalid(
                    "function is program, management_general or fundraising",
                ));
            }
        };
    }
    if let Some(x) = a.restriction {
        if r.dim != "fund" {
            return Err(invalid("only funds have a restriction"));
        }
        if !matches!(
            x.as_str(),
            "without_donor_restrictions" | "with_donor_restrictions"
        ) {
            return Err(invalid(
                "restriction is without_donor_restrictions or with_donor_restrictions",
            ));
        }
        if r.id == GENERAL && x != "without_donor_restrictions" {
            return Err(rejected("the General fund has no donor restrictions"));
        }
        r.restriction = Some(x);
    }
    if let Some(p) = a.purpose {
        if r.dim != "fund" {
            return Err(invalid("only funds have a purpose"));
        }
        let p = mk_core::text::clean(&p, usize::MAX).text;
        if p.chars().count() > 1000 {
            return Err(invalid("purpose: at most 1,000 characters"));
        }
        r.purpose = Some(p).filter(|x| !x.trim().is_empty());
    }
    if let Some(x) = a.active {
        if r.id == GENERAL && !x {
            return Err(rejected("the General fund stays active"));
        }
        r.active = x;
    }
    Ok(())
}

fn record_json(ctx: &Ctx<'_>, r: &DimRecord) -> CommandResult<Value> {
    let mut v = serde_json::to_value(r)?;
    let uses = usage(ctx, &r.dim)?.get(&r.id).copied().unwrap_or(0);
    if let Some(o) = v.as_object_mut() {
        o.insert("uses".into(), json!(uses));
        o.insert("level".into(), json!(r.name.matches(':').count()));
    }
    Ok(v)
}

/// How many splits use each record of a dimension.
fn usage(ctx: &Ctx<'_>, dim: &str) -> CommandResult<BTreeMap<String, usize>> {
    let mut out = BTreeMap::new();
    for t in all_txns(ctx)? {
        for s in &t.splits {
            if let Some(id) = tag_of(&s.tags, dim) {
                *out.entry(id.to_owned()).or_insert(0) += 1;
            }
        }
    }
    Ok(out)
}

fn all_txns(ctx: &Ctx<'_>) -> CommandResult<Vec<Transaction>> {
    Ok(ctx.tx.transactions(&mk_store::TxnFilter {
        accounts: None,
        from: None,
        to: None,
        include_voided: true,
        limit: None,
        offset: 0,
    })?)
}

pub(crate) fn tag_of<'a>(t: &'a SplitTags, dim: &str) -> Option<&'a str> {
    match dim {
        "class" => t.class.as_deref(),
        "location" => t.location.as_deref(),
        "project" => t.project.as_deref(),
        _ => t.fund.as_deref(),
    }
}

fn create(ctx: &Ctx<'_>, dim: &str, a: DimArgs) -> CommandResult<Value> {
    if a.name.is_none() {
        return Err(invalid("give the name"));
    }
    if dim == "fund" {
        ensure_general(ctx)?;
    }
    let mut r = DimRecord {
        id: mk_core::Id::<()>::new().to_string(),
        dim: dim.into(),
        name: String::new(),
        active: true,
        function: None,
        restriction: (dim == "fund").then(|| "without_donor_restrictions".to_owned()),
        purpose: None,
        created_at: now_rfc3339()?,
    };
    apply(ctx, &mut r, a)?;
    save_record(ctx, &r)?;
    record_json(ctx, &r)
}

fn update(ctx: &Ctx<'_>, dim: &str, mut a: DimArgs) -> CommandResult<Value> {
    let key = a
        .record
        .take()
        .ok_or_else(|| invalid(format!("give the {dim}")))?;
    if dim == "fund" {
        ensure_general(ctx)?;
    }
    let mut r = find(ctx, dim, &key)?;
    apply(ctx, &mut r, a)?;
    save_record(ctx, &r)?;
    record_json(ctx, &r)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    #[serde(default)]
    active: Option<bool>,
}

fn list(ctx: &Ctx<'_>, dim: &str, a: ListArgs) -> CommandResult<Value> {
    let uses = usage(ctx, dim)?;
    let net = if dim == "fund" {
        Some(net_assets(ctx, mk_store::today_utc()?)?)
    } else {
        None
    };
    let mut out = Vec::new();
    for r in records(ctx, dim)? {
        if a.active.is_some_and(|x| x != r.active) {
            continue;
        }
        let mut v = serde_json::to_value(&r)?;
        if let Some(o) = v.as_object_mut() {
            o.insert("uses".into(), json!(uses.get(&r.id).copied().unwrap_or(0)));
            o.insert("level".into(), json!(r.name.matches(':').count()));
            if let Some(n) = &net {
                let amount = n.get(&r.id).copied().unwrap_or_default();
                o.insert("net_assets".into(), json!(crate::sales::money(amount)));
                o.insert(
                    "overspent".into(),
                    json!(r.restricted() && amount.is_sign_negative() && !amount.is_zero()),
                );
            }
        }
        out.push(v);
    }
    Ok(Value::Array(out))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ActiveArgs {
    #[serde(alias = "class", alias = "location", alias = "fund")]
    record: String,
    active: bool,
}

fn set_active(ctx: &Ctx<'_>, dim: &str, a: ActiveArgs) -> CommandResult<Value> {
    update(
        ctx,
        dim,
        DimArgs {
            record: Some(a.record),
            active: Some(a.active),
            ..DimArgs::default()
        },
    )
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RefArgs {
    #[serde(alias = "class", alias = "location", alias = "fund")]
    record: String,
}

fn delete(ctx: &Ctx<'_>, dim: &str, a: RefArgs) -> CommandResult<Value> {
    let r = find(ctx, dim, &a.record)?;
    if r.id == GENERAL {
        return Err(rejected("the General fund cannot be deleted"));
    }
    let used = usage(ctx, dim)?.contains_key(&r.id)
        || [
            DocTable::SalesDoc,
            DocTable::BillDoc,
            DocTable::Rule,
            DocTable::Schedule,
        ]
        .iter()
        .try_fold(false, |acc, t| -> CommandResult<bool> {
            Ok(acc
                || ctx
                    .tx
                    .docs(*t, None)?
                    .iter()
                    .any(|d| d.json.to_string().contains(&format!("\"{}\"", r.id))))
        })?;
    if used {
        return Err(rejected(format!(
            "{} is in use; make it inactive instead",
            r.name
        )));
    }
    let children = records(ctx, dim)?.iter().any(|x| {
        x.name
            .to_lowercase()
            .starts_with(&format!("{}:", r.name.to_lowercase()))
    });
    if children {
        return Err(rejected(format!("{} has sub-{dim}s", r.name)));
    }
    ctx.save_doc(DocTable::Dimension, &r.id, None)?;
    Ok(json!({"deleted": r.id}))
}

fn class_create(ctx: &Ctx<'_>, a: DimArgs) -> CommandResult<Value> {
    create(ctx, "class", a)
}
fn class_update(ctx: &Ctx<'_>, a: DimArgs) -> CommandResult<Value> {
    update(ctx, "class", a)
}
fn class_list(ctx: &Ctx<'_>, a: ListArgs) -> CommandResult<Value> {
    list(ctx, "class", a)
}
fn class_set_active(ctx: &Ctx<'_>, a: ActiveArgs) -> CommandResult<Value> {
    set_active(ctx, "class", a)
}
fn class_delete(ctx: &Ctx<'_>, a: RefArgs) -> CommandResult<Value> {
    delete(ctx, "class", a)
}
fn location_create(ctx: &Ctx<'_>, a: DimArgs) -> CommandResult<Value> {
    create(ctx, "location", a)
}
fn location_update(ctx: &Ctx<'_>, a: DimArgs) -> CommandResult<Value> {
    update(ctx, "location", a)
}
fn location_list(ctx: &Ctx<'_>, a: ListArgs) -> CommandResult<Value> {
    list(ctx, "location", a)
}
fn location_set_active(ctx: &Ctx<'_>, a: ActiveArgs) -> CommandResult<Value> {
    set_active(ctx, "location", a)
}
fn location_delete(ctx: &Ctx<'_>, a: RefArgs) -> CommandResult<Value> {
    delete(ctx, "location", a)
}
fn fund_create(ctx: &Ctx<'_>, a: DimArgs) -> CommandResult<Value> {
    create(ctx, "fund", a)
}
fn fund_update(ctx: &Ctx<'_>, a: DimArgs) -> CommandResult<Value> {
    update(ctx, "fund", a)
}
fn fund_list(ctx: &Ctx<'_>, a: ListArgs) -> CommandResult<Value> {
    list(ctx, "fund", a)
}
fn fund_set_active(ctx: &Ctx<'_>, a: ActiveArgs) -> CommandResult<Value> {
    set_active(ctx, "fund", a)
}
fn fund_delete(ctx: &Ctx<'_>, a: RefArgs) -> CommandResult<Value> {
    delete(ctx, "fund", a)
}

fn dimension_names(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let mut out = serde_json::Map::new();
    for dim in ["class", "location", "fund"] {
        let m: serde_json::Map<String, Value> = records(ctx, dim)?
            .into_iter()
            .map(|r| (r.id, json!(r.name)))
            .collect();
        out.insert(dim.into(), Value::Object(m));
    }
    let m: serde_json::Map<String, Value> = crate::projects::all(ctx)?
        .into_iter()
        .map(|p| (p.id, json!(p.name)))
        .collect();
    out.insert("project".into(), Value::Object(m));
    Ok(Value::Object(out))
}

// ----- tags ---------------------------------------------------------------------------

/// Resolve tag names or ids for the dimensions that are on. `None` leaves a dimension
/// untagged; an empty text is "no tag" too.
pub(crate) fn resolve_tags(
    ctx: &Ctx<'_>,
    class: Option<&str>,
    location: Option<&str>,
    fund: Option<&str>,
) -> CommandResult<SplitTags> {
    let one = |dim: &str, v: Option<&str>| -> CommandResult<Option<String>> {
        let Some(v) = v.map(str::trim).filter(|v| !v.is_empty()) else {
            return Ok(None);
        };
        if !is_module_enabled(ctx.tx, module_of(dim))? {
            return Err(invalid(format!(
                "{dim} tags need the {} feature on",
                module_of(dim)
            )));
        }
        let r = find(ctx, dim, v)?;
        Ok(Some(r.id))
    };
    Ok(SplitTags {
        class: one("class", class)?,
        location: one("location", location)?,
        fund: one("fund", fund)?,
        project: None,
        billable: false,
    })
}

/// [`resolve_tags`] with the project tag (projects.md): a project name or id ("" for
/// none; needs the projects module), and `billable` (default the project's
/// `billable`). A split given a project and no fund at all (`fund` absent, not "")
/// takes the project's fund when the funds module is on.
pub(crate) fn resolve_tags_p(
    ctx: &Ctx<'_>,
    class: Option<&str>,
    location: Option<&str>,
    fund: Option<&str>,
    project: Option<&str>,
    billable: Option<bool>,
) -> CommandResult<SplitTags> {
    let mut t = resolve_tags(ctx, class, location, fund)?;
    let Some(p) = project.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(t);
    };
    if !is_module_enabled(ctx.tx, crate::projects::MODULE)? {
        return Err(invalid("project tags need the projects feature on"));
    }
    let rec = crate::projects::find(ctx, p)?;
    t.project = Some(rec.id.clone());
    t.billable = billable.unwrap_or(rec.billable);
    if fund.is_none()
        && t.fund.is_none()
        && let Some(f) = &rec.fund
        && is_module_enabled(ctx.tx, FUNDS)?
    {
        t.fund = Some(f.clone());
    }
    Ok(t)
}

/// Names for tag ids (for output).
pub(crate) fn tag_names(ctx: &Ctx<'_>, t: &SplitTags) -> CommandResult<Value> {
    let name = |dim: &str, id: Option<&str>| -> CommandResult<Value> {
        Ok(match id {
            Some(id) => json!(
                records(ctx, dim)?
                    .into_iter()
                    .find(|r| r.id == id)
                    .map(|r| r.name)
            ),
            None => Value::Null,
        })
    };
    let project = match t.project.as_deref() {
        Some(id) => json!(
            crate::projects::all(ctx)?
                .into_iter()
                .find(|p| p.id == id)
                .map(|p| p.name)
        ),
        None => Value::Null,
    };
    Ok(json!({
        "class": name("class", t.class.as_deref())?,
        "location": name("location", t.location.as_deref())?,
        "fund": name("fund", t.fund.as_deref())?,
        "project": project,
        "billable": t.billable,
    }))
}

/// Accounts exempt from the required-tag rule (fund mechanics).
fn mechanics(chart: &Chart, id: AccountId) -> bool {
    chart.get(id).is_some_and(|a| {
        a.parent.is_none() && (a.name == RELEASE_ACCOUNT || a.name == TRANSFER_ACCOUNT)
    })
}

/// `classes.required` / `locations.required`: every income or expense split of a
/// posted transaction carries the tag.
pub(crate) fn check_required(ctx: &Ctx<'_>, t: &Transaction) -> CommandResult<()> {
    if t.status != TxnStatus::Posted {
        return Ok(());
    }
    let chart = Chart::load(ctx)?;
    for (dim, module, key) in [
        ("class", CLASSES, "classes.required"),
        ("location", LOCATIONS, "locations.required"),
        ("project", crate::projects::MODULE, "projects.required"),
    ] {
        if !is_module_enabled(ctx.tx, module)? || !crate::module_settings::get_bool(ctx, key)? {
            continue;
        }
        for (i, s) in t.splits.iter().enumerate() {
            let Some(a) = chart.get(s.account) else {
                continue;
            };
            if !matches!(a.account_type, AccountType::Income | AccountType::Expense)
                || mechanics(&chart, s.account)
            {
                continue;
            }
            if tag_of(&s.tags, dim).is_none() {
                return Err(rejected(format!(
                    "split {} ({}) needs a {dim}",
                    i + 1,
                    chart.full_name(s.account)
                )));
            }
        }
    }
    Ok(())
}

// ----- releases -----------------------------------------------------------------------

fn named_account(
    ctx: &Ctx<'_>,
    name: &str,
    ty: &str,
    subtype: &str,
    create: bool,
) -> CommandResult<Option<AccountId>> {
    let chart = Chart::load(ctx)?;
    if let Some(a) = chart
        .accounts
        .iter()
        .find(|a| a.parent.is_none() && a.name == name)
    {
        return Ok(Some(a.id));
    }
    if !create {
        return Ok(None);
    }
    let v =
        crate::accounts::create_json(ctx, json!({"name": name, "type": ty, "subtype": subtype}))?;
    Ok(v.get("id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok()))
}

pub(crate) fn release_account(ctx: &Ctx<'_>, create: bool) -> CommandResult<Option<AccountId>> {
    named_account(ctx, RELEASE_ACCOUNT, "income", "income", create)
}

pub(crate) fn transfer_account(ctx: &Ctx<'_>, create: bool) -> CommandResult<Option<AccountId>> {
    named_account(ctx, TRANSFER_ACCOUNT, "equity", "net-assets", create)
}

fn split(account: AccountId, value: Decimal, memo: Option<String>, tags: SplitTags) -> Split {
    Split {
        account,
        value,
        quantity: value,
        memo,
        reconcile: mk_core::Reconcile::New,
        fingerprint: None,
        tags,
    }
}

fn link_of(ctx: &Ctx<'_>, expense: TransactionId) -> CommandResult<Option<TransactionId>> {
    Ok(ctx
        .tx
        .get_doc(DocTable::TxnLink, &format!("release:{expense}"))?
        .and_then(|d| {
            d.json
                .get("release")
                .and_then(Value::as_str)
                .and_then(|s| s.parse().ok())
        }))
}

/// Whether a transaction is an automatic release (it has an expense pointing at it).
pub(crate) fn release_of(ctx: &Ctx<'_>, txn: TransactionId) -> CommandResult<Option<String>> {
    Ok(ctx
        .tx
        .docs(DocTable::TxnLink, Some("release"))?
        .into_iter()
        .find(|d| d.json.get("release").and_then(Value::as_str) == Some(txn.to_string().as_str()))
        .and_then(|d| {
            d.json
                .get("expense")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }))
}

/// Every automatic release: release transaction id → expense transaction id.
pub(crate) fn release_links(
    ctx: &Ctx<'_>,
) -> CommandResult<std::collections::HashMap<String, String>> {
    Ok(ctx
        .tx
        .docs(DocTable::TxnLink, Some("release"))?
        .into_iter()
        .filter_map(|d| {
            let r = d.json.get("release")?.as_str()?.to_owned();
            let e = d.json.get("expense")?.as_str()?.to_owned();
            Some((r, e))
        })
        .collect())
}

/// Keep an expense's automatic release in step with it (business-dimensions.md: created,
/// updated and undone with the expense). Runs after every transaction save of a command.
pub(crate) fn after_save(ctx: &Ctx<'_>, t: &Transaction) -> CommandResult<()> {
    if ctx.restoring.get() || ctx.syncing.get() || !is_module_enabled(ctx.tx, FUNDS)? {
        return Ok(());
    }
    if !crate::module_settings::get_bool(ctx, "funds.auto_release")? {
        return Ok(());
    }
    ctx.syncing.set(true);
    let r = sync(ctx, t);
    ctx.syncing.set(false);
    r
}

/// Remove an expense's automatic release with it.
pub(crate) fn after_remove(ctx: &Ctx<'_>, t: &Transaction) -> CommandResult<()> {
    if ctx.restoring.get() || ctx.syncing.get() {
        return Ok(());
    }
    if let Some(rid) = link_of(ctx, t.id)? {
        ctx.syncing.set(true);
        let r = (|| -> CommandResult<()> {
            if let Some(rel) = ctx.tx.transaction(rid)? {
                validate_change(ctx, Some(&rel), None)?;
                ctx.remove_txn(&rel)?;
            }
            ctx.save_doc(DocTable::TxnLink, &format!("release:{}", t.id), None)
        })();
        ctx.syncing.set(false);
        r?;
    }
    Ok(())
}

fn sync(ctx: &Ctx<'_>, t: &Transaction) -> CommandResult<()> {
    let chart = Chart::load(ctx)?;
    let funds = records(ctx, "fund")?;
    let restricted = |id: &str| funds.iter().any(|f| f.id == id && f.restricted());
    // Restricted fund → (amount, class of its first split).
    let mut by: BTreeMap<String, (Decimal, Option<String>)> = BTreeMap::new();
    if t.status == TxnStatus::Posted {
        for s in &t.splits {
            let expense = chart
                .get(s.account)
                .is_some_and(|a| a.account_type == AccountType::Expense);
            let Some(f) = s.tags.fund.as_deref().filter(|f| expense && restricted(f)) else {
                continue;
            };
            let e = by
                .entry(f.to_owned())
                .or_insert((Decimal::ZERO, s.tags.class.clone()));
            e.0 = add(e.0, s.value);
        }
    }
    by.retain(|_, (amount, _)| !amount.is_zero());
    let existing = link_of(ctx, t.id)?;
    let old = match existing {
        Some(id) => ctx.tx.transaction(id)?,
        None => None,
    };
    if by.is_empty() {
        if let Some(rel) = old {
            validate_change(ctx, Some(&rel), None)?;
            ctx.remove_txn(&rel)?;
            ctx.save_doc(DocTable::TxnLink, &format!("release:{}", t.id), None)?;
        }
        return Ok(());
    }
    let acc = release_account(ctx, true)?
        .ok_or_else(|| CommandError::Integrity("release account".into()))?;
    let mut splits = Vec::new();
    let label = t.payee.clone().unwrap_or_else(|| "expense".into());
    for (fund, (amount, class)) in &by {
        let memo = Some(format!("Released for {label}"));
        splits.push(split(
            acc,
            *amount,
            memo.clone(),
            SplitTags {
                class: class.clone(),
                fund: Some(fund.clone()),
                ..SplitTags::default()
            },
        ));
        splits.push(split(
            acc,
            -*amount,
            memo,
            SplitTags {
                class: class.clone(),
                fund: Some(GENERAL.into()),
                ..SplitTags::default()
            },
        ));
    }
    mk_ledger::sort_splits(&mut splits);
    let rel = Transaction {
        id: old.as_ref().map_or_else(TransactionId::new, |o| o.id),
        date: t.date,
        currency: t.currency.clone(),
        payee: Some("Released from restrictions".into()),
        number: None,
        memo: Some(format!("Automatic release for {label} ({})", t.date)),
        status: TxnStatus::Posted,
        void_reason: None,
        reverses: None,
        opening: false,
        entered_at: old
            .as_ref()
            .map_or_else(now_rfc3339, |o| Ok(o.entered_at.clone()))?,
        splits,
    };
    if old.as_ref() == Some(&rel) {
        return Ok(());
    }
    validate_change(ctx, old.as_ref(), Some(&rel))?;
    ctx.save_txn(old.as_ref(), &rel)?;
    ensure_general(ctx)?;
    if existing.is_none() {
        let id = format!("release:{}", t.id);
        ctx.save_doc(
            DocTable::TxnLink,
            &id,
            Some(&Doc {
                id: id.clone(),
                grp: Some("release".into()),
                ord: 0,
                json: json!({"expense": t.id, "release": rel.id}),
            }),
        )?;
    }
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReleaseArgs {
    /// A fund with donor restrictions.
    from_fund: String,
    /// Default: General.
    #[serde(default)]
    to_fund: Option<String>,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
    date: Date,
    #[serde(default)]
    memo: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn two_sided(
    ctx: &Ctx<'_>,
    account: AccountId,
    from: &DimRecord,
    to: &DimRecord,
    amount: Decimal,
    date: Date,
    payee: &str,
    memo: Option<String>,
) -> CommandResult<Transaction> {
    let t = Transaction {
        id: TransactionId::new(),
        date,
        currency: ctx.tx.info()?.base_currency.parse()?,
        payee: Some(payee.into()),
        number: None,
        memo,
        status: TxnStatus::Posted,
        void_reason: None,
        reverses: None,
        opening: false,
        entered_at: now_rfc3339()?,
        splits: {
            let mut v = vec![
                split(
                    account,
                    amount,
                    None,
                    SplitTags {
                        fund: Some(from.id.clone()),
                        ..SplitTags::default()
                    },
                ),
                split(
                    account,
                    -amount,
                    None,
                    SplitTags {
                        fund: Some(to.id.clone()),
                        ..SplitTags::default()
                    },
                ),
            ];
            mk_ledger::sort_splits(&mut v);
            v
        },
    };
    validate_change(ctx, None, Some(&t))?;
    ctx.save_txn(None, &t)?;
    Ok(t)
}

fn check_amount(amount: Decimal) -> CommandResult<()> {
    if amount <= Decimal::ZERO || amount.normalize().scale() > 2 {
        return Err(invalid("the amount is positive with at most 2 decimals"));
    }
    Ok(())
}

fn fund_release(ctx: &Ctx<'_>, a: ReleaseArgs) -> CommandResult<Value> {
    ensure_general(ctx)?;
    check_amount(a.amount)?;
    let from = find(ctx, "fund", &a.from_fund)?;
    if !from.restricted() {
        return Err(invalid(format!("{} has no donor restrictions", from.name)));
    }
    let to = find(ctx, "fund", a.to_fund.as_deref().unwrap_or(GENERAL))?;
    if to.restricted() {
        return Err(invalid(
            "a release goes to a fund without donor restrictions",
        ));
    }
    let available = net_assets(ctx, a.date)?
        .get(&from.id)
        .copied()
        .unwrap_or_default();
    let acc =
        release_account(ctx, true)?.ok_or_else(|| CommandError::Integrity("account".into()))?;
    let memo = a
        .memo
        .map(|m| mk_core::text::clean(&m, 1000).text)
        .or(Some(format!("Released from {}", from.name)));
    let t = two_sided(
        ctx,
        acc,
        &from,
        &to,
        a.amount,
        a.date,
        "Released from restrictions",
        memo,
    )?;
    let mut warnings = Vec::new();
    if a.amount > available {
        warnings.push(format!(
            "{} has {} of net assets; the release makes it overspent",
            from.name,
            crate::sales::money(available)
        ));
    }
    Ok(json!({"txn": t.id, "warnings": warnings}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TransferArgs {
    from: String,
    to: String,
    #[schemars(schema_with = "mk_core::schema::decimal_string")]
    amount: Decimal,
    date: Date,
    #[serde(default)]
    memo: Option<String>,
}

fn fund_transfer(ctx: &Ctx<'_>, a: TransferArgs) -> CommandResult<Value> {
    ensure_general(ctx)?;
    check_amount(a.amount)?;
    let from = find(ctx, "fund", &a.from)?;
    let to = find(ctx, "fund", &a.to)?;
    if from.id == to.id {
        return Err(invalid("choose two different funds"));
    }
    let acc =
        transfer_account(ctx, true)?.ok_or_else(|| CommandError::Integrity("account".into()))?;
    let memo = a
        .memo
        .map(|m| mk_core::text::clean(&m, 1000).text)
        .or(Some(format!("{} to {}", from.name, to.name)));
    let t = two_sided(
        ctx,
        acc,
        &from,
        &to,
        a.amount,
        a.date,
        "Transfer between funds",
        memo,
    )?;
    Ok(json!({"txn": t.id}))
}

// ----- net assets ---------------------------------------------------------------------

/// The fund a split counts for: its tag, else General.
pub(crate) fn fund_of(s_tags: &SplitTags) -> &str {
    s_tags.fund.as_deref().unwrap_or(GENERAL)
}

/// Net assets by fund as of a date (business-dimensions.md): equity and income tagged
/// to a fund add to it; expenses reduce their fund, except that expenses tagged to a
/// restricted fund count against General (their release moves the amount).
pub(crate) fn net_assets(ctx: &Ctx<'_>, as_of: Date) -> CommandResult<BTreeMap<String, Decimal>> {
    let chart = Chart::load(ctx)?;
    let funds = records(ctx, "fund")?;
    let restricted = |id: &str| funds.iter().any(|f| f.id == id && f.restricted());
    let base: mk_core::CommodityCode = ctx.tx.info()?.base_currency.parse()?;
    let mut out: BTreeMap<String, Decimal> = funds
        .iter()
        .map(|f| (f.id.clone(), Decimal::ZERO))
        .collect();
    for row in ctx.tx.posted_splits(Some(as_of))? {
        if row.currency != base {
            continue;
        }
        let Some(a) = chart.get(row.account) else {
            continue;
        };
        let f = fund_of(&row.tags).to_owned();
        match a.account_type {
            AccountType::Equity | AccountType::Income => {
                let e = out.entry(f).or_default();
                *e = sub(*e, row.value);
            }
            AccountType::Expense => {
                let target = if restricted(&f) {
                    GENERAL.to_owned()
                } else {
                    f
                };
                let e = out.entry(target).or_default();
                *e = sub(*e, row.value);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Overspent restricted funds as of a date: (name, amount over).
pub(crate) fn overspent(ctx: &Ctx<'_>, as_of: Date) -> CommandResult<Vec<(String, Decimal)>> {
    let net = net_assets(ctx, as_of)?;
    Ok(records(ctx, "fund")?
        .into_iter()
        .filter(|f| f.restricted())
        .filter_map(|f| {
            let n = net.get(&f.id).copied().unwrap_or_default();
            (n < Decimal::ZERO).then(|| (f.name, -n))
        })
        .collect())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AttentionArgs {
    #[serde(default)]
    today: Option<Date>,
}

fn fund_attention(ctx: &Ctx<'_>, a: AttentionArgs) -> CommandResult<Value> {
    let today = a.today.map_or_else(mk_store::today_utc, Ok)?;
    let rows: Vec<Value> = overspent(ctx, today)?
        .into_iter()
        .map(|(name, over)| {
            json!({"fund": name, "over": crate::sales::money(over),
                   "message": format!("{name} fund is overspent by {}", crate::sales::group(&crate::sales::money(over)))})
        })
        .collect();
    Ok(Value::Array(rows))
}

/// A line's tags with the document's filling the gaps.
pub(crate) fn merge(line: &SplitTags, doc: &SplitTags) -> SplitTags {
    // A line with its own project keeps its billable flag; otherwise the document's
    // project and flag apply.
    let (project, billable) = match &line.project {
        Some(p) => (Some(p.clone()), line.billable),
        None => (doc.project.clone(), doc.billable),
    };
    SplitTags {
        class: line.class.clone().or_else(|| doc.class.clone()),
        location: line.location.clone().or_else(|| doc.location.clone()),
        fund: line.fund.clone().or_else(|| doc.fund.clone()),
        project,
        billable,
    }
}

/// Document-level tags from arguments: given fields replace, others keep `base`;
/// with the document's project (projects.md: a document-level default with per-line
/// overrides).
pub(crate) fn doc_tags_p(
    ctx: &Ctx<'_>,
    class: Option<&str>,
    location: Option<&str>,
    fund: Option<&str>,
    project: Option<&str>,
    base: Option<&SplitTags>,
) -> CommandResult<SplitTags> {
    let given = resolve_tags_p(ctx, class, location, fund, project, None)?;
    let keep = base.cloned().unwrap_or_default();
    let has_project = given.project.is_some();
    let (project, billable) = if project.is_some() {
        (given.project, given.billable)
    } else {
        (keep.project, keep.billable)
    };
    Ok(SplitTags {
        project,
        billable,
        class: if class.is_some() {
            given.class
        } else {
            keep.class
        },
        location: if location.is_some() {
            given.location
        } else {
            keep.location
        },
        fund: if fund.is_some() || (keep.fund.is_none() && has_project) {
            given.fund
        } else {
            keep.fund
        },
    })
}

/// Create a class by name (IIF review).
pub(crate) fn create_class(ctx: &Ctx<'_>, name: &str) -> CommandResult<Value> {
    create(
        ctx,
        "class",
        DimArgs {
            name: Some(name.to_owned()),
            ..DimArgs::default()
        },
    )
}
