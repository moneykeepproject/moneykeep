//! IIF import review and export (business-dimensions.md Part D): the mapping table of
//! account, class and name texts found in a file, creating the file's list records on
//! request, and `export.iif`.

use std::collections::HashMap;

use mk_core::{AccountId, Date, TxnStatus};
use mk_store::TxnFilter;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::import::{BatchDoc, check_section_account, classify, save_batch, sealed_json, staged};
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "import.map_name",
        "Map an account, class or name text of an IIF file (or create it)",
        true,
        map_name,
    );
    r.add(
        "import.create_lists",
        "Create chosen list records (accounts, customers, vendors, classes) of an IIF file",
        true,
        create_lists,
    );
    r.add(
        "export.iif",
        "Export transactions as an IIF file (limited columns; ASCII)",
        false,
        export_iif,
    );
}

/// One text of the file and what it maps to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NameMap {
    /// `account`, `class` or `name`.
    pub kind: String,
    pub name: String,
    /// Account id or class id (or party id for names).
    #[serde(default)]
    pub mapped: Option<String>,
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

fn push(names: &mut Vec<NameMap>, kind: &str, name: &str) {
    let n = name.trim();
    if n.is_empty()
        || names
            .iter()
            .any(|x| x.kind == kind && x.name.eq_ignore_ascii_case(n))
    {
        return;
    }
    names.push(NameMap {
        kind: kind.into(),
        name: n.to_owned(),
        mapped: None,
    });
}

/// Fill the mapping table from a parsed file and map exact matches.
pub(crate) fn init_names(
    ctx: &Ctx<'_>,
    batch: &mut BatchDoc,
    st: &mk_import::Statement,
) -> CommandResult<()> {
    for sec in &st.sections {
        if let Some(n) = &sec.account_name {
            push(&mut batch.names, "account", n);
        }
        for r in &sec.rows {
            for s in &r.splits {
                push(&mut batch.names, "account", &s.category_hint);
            }
        }
    }
    for sec in &st.sections {
        for r in &sec.rows {
            for c in r
                .class_hint
                .iter()
                .chain(r.splits.iter().filter_map(|s| s.class_hint.as_ref()))
            {
                push(&mut batch.names, "class", c);
            }
        }
    }
    for sec in &st.sections {
        for r in &sec.rows {
            push(&mut batch.names, "name", &r.payee);
        }
    }
    batch.lists = st.lists.clone();
    auto_map(ctx, batch)
}

/// Map texts that name an existing account, class or party exactly.
fn auto_map(ctx: &Ctx<'_>, batch: &mut BatchDoc) -> CommandResult<()> {
    let chart = Chart::load(ctx)?;
    let classes_on = crate::sales::module_on(ctx, crate::dimensions::CLASSES)?;
    let classes = crate::dimensions::records(ctx, "class")?;
    for n in &mut batch.names {
        if n.mapped.is_some() {
            continue;
        }
        if n.kind == "account"
            && let Some(d) = ctx.tx.get_doc(
                mk_store::DocTable::ImportAccountMap,
                &format!("iif:{}", n.name),
            )?
        {
            let remembered = d
                .json
                .get("account")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<AccountId>().ok())
                .filter(|a| chart.get(*a).is_some());
            if let Some(a) = remembered {
                n.mapped = Some(a.to_string());
                continue;
            }
        }
        n.mapped = match n.kind.as_str() {
            "account" => chart
                .accounts
                .iter()
                .find(|a| chart.full_name(a.id).eq_ignore_ascii_case(&n.name))
                .map(|a| a.id.to_string()),
            "class" if classes_on => classes
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(&n.name))
                .map(|c| c.id.clone()),
            _ => None,
        };
    }
    apply_sections(ctx, &chart, batch)
}

/// Sections whose IIF account text is mapped take that account.
fn apply_sections(ctx: &Ctx<'_>, chart: &Chart, batch: &mut BatchDoc) -> CommandResult<()> {
    let map = account_map(batch);
    for sec in &mut batch.sections {
        if sec.account.is_some() {
            continue;
        }
        let Some(name) = &sec.account_name else {
            continue;
        };
        if let Some(acc) = map.get(&name.to_lowercase())
            && check_section_account(chart, sec, *acc).is_ok()
        {
            sec.account = Some(*acc);
        }
    }
    let _ = ctx;
    Ok(())
}

/// Lower-cased account text → mapped account.
pub(crate) fn account_map(batch: &BatchDoc) -> HashMap<String, AccountId> {
    batch
        .names
        .iter()
        .filter(|n| n.kind == "account")
        .filter_map(|n| {
            n.mapped
                .as_deref()
                .and_then(|m| m.parse().ok())
                .map(|a| (n.name.to_lowercase(), a))
        })
        .collect()
}

/// Lower-cased class text → class id.
pub(crate) fn class_map(batch: &BatchDoc) -> HashMap<String, String> {
    batch
        .names
        .iter()
        .filter(|n| n.kind == "class")
        .filter_map(|n| n.mapped.clone().map(|m| (n.name.to_lowercase(), m)))
        .collect()
}

pub(crate) fn names_json(ctx: &Ctx<'_>, batch: &BatchDoc) -> CommandResult<Value> {
    if batch.names.is_empty() {
        return Ok(json!([]));
    }
    let chart = Chart::load(ctx)?;
    let classes = crate::dimensions::records(ctx, "class")?;
    Ok(Value::Array(
        batch
            .names
            .iter()
            .map(|n| {
                let shown = match (n.kind.as_str(), &n.mapped) {
                    ("account", Some(m)) => m.parse().ok().map(|a| chart.full_name(a)),
                    ("class", Some(m)) => {
                        classes.iter().find(|c| c.id == *m).map(|c| c.name.clone())
                    }
                    (_, Some(m)) => Some(m.clone()),
                    _ => None,
                };
                json!({"kind": n.kind, "name": n.name, "mapped": n.mapped, "mapped_name": shown})
            })
            .collect(),
    ))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MapNameArgs {
    batch: String,
    /// The text as it appears in the file.
    name: String,
    /// `account` (default), `class` or `name`.
    #[serde(default)]
    kind: Option<String>,
    /// Map to this account, class or party (id or name); null clears the mapping.
    #[serde(default)]
    to: Option<String>,
    /// Create the account (with this type: asset, liability, equity, income, expense)
    /// or class named like the text, then map to it.
    #[serde(default)]
    create: Option<String>,
    /// With `create` for accounts: the subtype (e.g. bank).
    #[serde(default)]
    subtype: Option<String>,
}

fn create_account(
    ctx: &Ctx<'_>,
    name: &str,
    ty: &str,
    subtype: Option<&str>,
) -> CommandResult<AccountId> {
    let chart = Chart::load(ctx)?;
    let (parent, leaf) = match name.rsplit_once(':') {
        Some((p, l)) => {
            let pa = chart.resolve(p).map_err(|_| {
                invalid(format!(
                    "create the parent account {p} first, or map {name} to an account"
                ))
            })?;
            (Some(chart.full_name(pa.id)), l.trim().to_owned())
        }
        None => (None, name.trim().to_owned()),
    };
    let mut args = json!({"name": leaf, "type": ty});
    if let Some(o) = args.as_object_mut() {
        if let Some(p) = parent {
            o.insert("parent".into(), json!(p));
        }
        if let Some(s) = subtype {
            o.insert("subtype".into(), json!(s));
        }
    }
    let v = crate::accounts::create_json(ctx, args)?;
    v.get("id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| CommandError::Integrity("account without id".into()))
}

fn map_name(ctx: &Ctx<'_>, a: MapNameArgs) -> CommandResult<Value> {
    let mut batch = staged(ctx, &a.batch)?;
    let kind = a.kind.unwrap_or_else(|| "account".into());
    let pos = batch
        .names
        .iter()
        .position(|n| n.kind == kind && n.name.eq_ignore_ascii_case(a.name.trim()))
        .ok_or_else(|| CommandError::NotFound(format!("{kind} {:?} in this file", a.name)))?;
    let text = batch
        .names
        .get(pos)
        .map(|n| n.name.clone())
        .unwrap_or_default();
    let mapped = match (kind.as_str(), &a.create, &a.to) {
        ("account", Some(ty), _) => {
            Some(create_account(ctx, &text, ty, a.subtype.as_deref())?.to_string())
        }
        ("account", None, Some(to)) => Some(Chart::load(ctx)?.resolve(to)?.id.to_string()),
        ("class", Some(_), _) => {
            let v = ctx_run_class(ctx, &text)?;
            Some(v)
        }
        ("class", None, Some(to)) => Some(
            crate::dimensions::resolve_tags(ctx, Some(to), None, None)?
                .class
                .ok_or_else(|| invalid("give a class"))?,
        ),
        ("name", None, Some(to)) => Some(crate::sales::find_party(ctx, to)?.id),
        (_, _, None) => None,
        _ => return Err(invalid("kind is account, class or name")),
    };
    if let Some(n) = batch.names.get_mut(pos) {
        n.mapped = mapped.clone();
    }
    // Remembered for later files naming the same account.
    if kind == "account" {
        let key = format!("iif:{text}");
        match mapped.as_deref() {
            Some(m) => ctx.save_doc(
                mk_store::DocTable::ImportAccountMap,
                &key,
                Some(&mk_store::Doc {
                    id: key.clone(),
                    grp: None,
                    ord: 0,
                    json: json!({"key": key, "account": m}),
                }),
            )?,
            None => ctx.save_doc(mk_store::DocTable::ImportAccountMap, &key, None)?,
        }
    }
    // A section named like the text follows the account mapping.
    if kind == "account" {
        let chart = Chart::load(ctx)?;
        for sec in &mut batch.sections {
            if sec
                .account_name
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(&text))
            {
                let acc = mapped.as_deref().and_then(|m| m.parse::<AccountId>().ok());
                if let Some(acc) = acc {
                    check_section_account(&chart, sec, acc)?;
                }
                sec.account = acc;
            }
        }
    }
    save_batch(ctx, &batch)?;
    classify(ctx, &batch)?;
    sealed_json(ctx, &batch.id)
}

fn ctx_run_class(ctx: &Ctx<'_>, name: &str) -> CommandResult<String> {
    if let Ok(t) = crate::dimensions::resolve_tags(ctx, Some(name), None, None)
        && let Some(c) = t.class
    {
        return Ok(c);
    }
    let v = crate::dimensions::create_class(ctx, name)?;
    v.get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| CommandError::Integrity("class without id".into()))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateListsArgs {
    batch: String,
    /// Indexes (0-based) of the list records to create.
    items: Vec<usize>,
}

/// MoneyKeep (type, subtype) for an IIF account type text.
fn account_kind(t: Option<&str>) -> (&'static str, Option<&'static str>) {
    match t.unwrap_or("").trim().to_uppercase().as_str() {
        "BANK" => ("asset", Some("bank")),
        "AR" => ("asset", Some("receivable")),
        "OCASSET" | "OASSET" => ("asset", Some("other-asset")),
        "FIXASSET" => ("asset", Some("fixed-asset")),
        "AP" => ("liability", Some("payable")),
        "CCARD" => ("liability", Some("credit-card")),
        "OCLIAB" | "LTLIAB" => ("liability", Some("other-liability")),
        "EQUITY" => ("equity", None),
        "INC" | "EXINC" => ("income", None),
        "COGS" => ("expense", Some("cost-of-goods")),
        _ => ("expense", None),
    }
}

fn create_lists(ctx: &Ctx<'_>, a: CreateListsArgs) -> CommandResult<Value> {
    let mut batch = staged(ctx, &a.batch)?;
    let mut created = Vec::new();
    for i in a.items {
        let item = batch
            .lists
            .get(i)
            .cloned()
            .ok_or_else(|| invalid(format!("the file has no list record {i}")))?;
        match item.kind.as_str() {
            "account" => {
                let chart = Chart::load(ctx)?;
                if chart.resolve(&item.name).is_ok() {
                    continue;
                }
                let (ty, sub) = account_kind(item.account_type.as_deref());
                create_account(ctx, &item.name, ty, sub)?;
            }
            "customer" | "vendor" => {
                if crate::sales::find_party(ctx, &item.name).is_ok() {
                    continue;
                }
                let role = if item.kind == "customer" {
                    "customer"
                } else {
                    "vendor"
                };
                crate::sales::party_create_json(
                    ctx,
                    json!({"name": item.name, "roles": [role], "billing_address": item.address}),
                )?;
            }
            _ => {
                ctx_run_class(ctx, &item.name)?;
            }
        }
        created.push(item.name);
    }
    auto_map(ctx, &mut batch)?;
    save_batch(ctx, &batch)?;
    classify(ctx, &batch)?;
    let mut v = sealed_json(ctx, &batch.id)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("created".into(), json!(created));
    }
    Ok(v)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExportArgs {
    from: Date,
    to: Date,
    /// Only transactions touching these accounts (default all).
    #[serde(default)]
    accounts: Option<Vec<String>>,
}

fn export_iif(ctx: &Ctx<'_>, a: ExportArgs) -> CommandResult<Value> {
    let chart = Chart::load(ctx)?;
    let filter = match &a.accounts {
        Some(list) => Some(
            list.iter()
                .map(|r| chart.resolve(r).map(|x| x.id))
                .collect::<CommandResult<Vec<_>>>()?,
        ),
        None => None,
    };
    let base = ctx.tx.info()?.base_currency;
    let classes: HashMap<String, String> = crate::dimensions::records(ctx, "class")?
        .into_iter()
        .map(|r| (r.id, r.name))
        .collect();
    let mut txns = Vec::new();
    let mut skipped = 0_usize;
    let mut all = ctx.tx.transactions(&TxnFilter {
        accounts: filter,
        from: Some(a.from),
        to: Some(a.to),
        include_voided: false,
        limit: None,
        offset: 0,
    })?;
    all.sort_by(|x, y| (x.date, &x.entered_at).cmp(&(y.date, &y.entered_at)));
    for t in all {
        if t.status != TxnStatus::Posted {
            continue;
        }
        if t.currency.as_str() != base
            || t.splits.iter().any(|s| {
                chart
                    .get(s.account)
                    .is_none_or(|x| x.commodity.as_str() != base)
            })
        {
            skipped += 1;
            continue;
        }
        let payee = t.payee.clone().unwrap_or_default();
        txns.push(mk_import::IifTxn {
            date: t.date,
            splits: t
                .splits
                .iter()
                .enumerate()
                .map(|(i, s)| mk_import::IifSplit {
                    account: chart.full_name(s.account),
                    name: if i == 0 { payee.clone() } else { String::new() },
                    class: s
                        .tags
                        .class
                        .as_ref()
                        .and_then(|c| classes.get(c))
                        .cloned()
                        .unwrap_or_default(),
                    amount: s.value,
                })
                .collect(),
        });
    }
    let now = mk_store::now_rfc3339()?;
    let date = now.get(..10).unwrap_or("").to_owned();
    let mdy = match date.split('-').collect::<Vec<_>>().as_slice() {
        [y, m, d] => format!("{m}/{d}/{y}"),
        _ => date.clone(),
    };
    let time = now.get(11..19).unwrap_or("00:00:00");
    let (content, replaced) = mk_import::write_iif(&txns, &mdy, time, env!("CARGO_PKG_VERSION"));
    let mut warnings = Vec::new();
    if replaced > 0 {
        warnings.push(format!(
            "{replaced} characters outside ASCII were written as '?'"
        ));
    }
    if skipped > 0 {
        warnings.push(format!(
            "{skipped} transactions in another currency or with securities were left out"
        ));
    }
    Ok(json!({
        "file_name": format!("moneykeep-{}-to-{}.iif", a.from, a.to),
        "content": content,
        "transactions": txns.len(),
        "warnings": warnings,
        "limitations": [
            "Memos, numbers and document details are not exported.",
            "Characters outside ASCII are written as '?'.",
            "Every transaction is exported as a general journal entry.",
        ],
    }))
}
