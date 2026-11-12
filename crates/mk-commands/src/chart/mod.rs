//! Starter charts of accounts (chart-presets.md): presets, adding accounts from a
//! chart, module packs, account roles and the Tax summary report.

pub(crate) mod roles;
pub(crate) mod seed;
pub(crate) mod tax_summary;

use std::collections::{HashMap, HashSet};

use mk_core::{Account, AccountId, CommodityCode};
use mk_store::now_rfc3339;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::{CommandError, CommandResult, Ctx, Registry, is_module_enabled};
use seed::{ChartAccount, Preset, Seed};

pub(crate) use roles::purpose as role_purpose;

/// Book settings (module `core`) set when a preset is applied.
pub(crate) const PRESET_KEY: &str = "chart.preset";
pub(crate) const TAX_YEAR_KEY: &str = "chart.tax_year";

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "chart.presets",
        "List the built-in starter charts of accounts, with their accounts for previews",
        false,
        presets,
    );
    r.add(
        "chart.preview_add",
        "Show which accounts a starter chart would add to this book and which it skips",
        false,
        preview_add,
    );
    r.add(
        "chart.add",
        "Add the accounts of a starter chart to this book (existing accounts are never changed)",
        true,
        add,
    );
    tax_summary::register(r);
}

pub(crate) fn shipped() -> CommandResult<&'static Seed> {
    seed::shipped().map_err(CommandError::Integrity)
}

/// The preset `id`, or the error book creation and `chart.*` give for an unknown one.
pub(crate) fn find_preset(id: &str) -> CommandResult<&'static Preset> {
    shipped()?
        .preset(id.trim())
        .ok_or_else(|| CommandError::InvalidArgs(format!("Unknown chart preset '{}'", id.trim())))
}

/// Whether `id` names a shipped preset (checked before a book file is written).
pub fn check_preset(id: &str) -> CommandResult<()> {
    find_preset(id).map(|_| ())
}

fn tree_json(chart: &[ChartAccount]) -> Value {
    Value::Array(
        chart
            .iter()
            .map(|a| {
                json!({
                    "code": a.code,
                    "name": a.name,
                    "type": a.account_type,
                    "subtype": a.subtype,
                    "header": a.header,
                    "parent": a.parent,
                    "tax_line": a.tax_line,
                    "note": a.note,
                })
            })
            .collect(),
    )
}

fn presets(_ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    presets_json()
}

/// `chart.presets` without a book (the New Book dialog runs before one exists).
pub fn presets_json() -> CommandResult<Value> {
    let seed = shipped()?;
    let mut out = Vec::new();
    for p in &seed.presets {
        let alone = seed.resolve(p, &[]);
        let mods: Vec<&str> = p.modules.iter().map(String::as_str).collect();
        let with = seed.resolve(p, &mods);
        out.push(json!({
            "id": p.id,
            "name": p.name,
            "blurb": p.blurb,
            "description": p.description.trim(),
            "tax_year": p.tax_year,
            "modules": p.modules,
            "account_count": alone.len(),
            "accounts": tree_json(&with),
        }));
    }
    Ok(Value::Array(out))
}

// ----- planning ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Add,
    AddUnder,
    Skip,
}

#[derive(Debug, Clone)]
struct PlanRow {
    acc: ChartAccount,
    action: Action,
    reason: Option<String>,
    /// Where the account goes: an existing account, a planned one (by code) or the
    /// top level.
    parent: ParentRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParentRef {
    Top,
    Existing(AccountId),
    Planned(String),
}

/// The modules currently on that have a pack.
fn modules_on(ctx: &Ctx<'_>, seed: &Seed) -> CommandResult<Vec<&'static str>> {
    let mut out = Vec::new();
    for m in crate::modules::CATALOG {
        if seed.module_pack(m.name).is_some() && is_module_enabled(ctx.tx, m.name)? {
            out.push(m.name);
        }
    }
    Ok(out)
}

/// Decide, for each account of `accounts` (in code order), whether it is added to
/// the book or skipped, and where it goes (chart-presets.md `chart.preview_add`).
fn plan(chart: &Chart, accounts: &[ChartAccount]) -> Vec<PlanRow> {
    let mut rows: Vec<PlanRow> = Vec::new();
    let mut planned: HashSet<String> = HashSet::new();
    // Planned (code → account) for name clashes among new siblings (cannot happen
    // with a valid seed, but keeps the plan honest).
    let by_code = |code: &str| {
        chart
            .accounts
            .iter()
            .find(|a| a.code.as_deref() == Some(code))
    };
    for acc in accounts {
        let parent = match &acc.parent {
            None => ParentRef::Top,
            Some(pc) if planned.contains(pc) => ParentRef::Planned(pc.clone()),
            Some(pc) => {
                // A skipped header: its children go under the existing account with
                // that code if it is a placeholder of the same type, else top level.
                let same_code = by_code(pc);
                let header_name = accounts.iter().find(|a| &a.code == pc).map(|a| &a.name);
                let same_name = header_name.and_then(|n| {
                    chart
                        .accounts
                        .iter()
                        .find(|a| a.parent.is_none() && a.name.eq_ignore_ascii_case(n))
                });
                match same_code.or(same_name) {
                    Some(e) if e.placeholder && e.account_type == acc.account_type => {
                        ParentRef::Existing(e.id)
                    }
                    _ => ParentRef::Top,
                }
            }
        };
        let skip = |reason: String| PlanRow {
            acc: acc.clone(),
            action: Action::Skip,
            reason: Some(reason),
            parent: parent.clone(),
        };
        if let Some(e) = by_code(&acc.code) {
            let reason = if e.name.eq_ignore_ascii_case(&acc.name) {
                "same code".to_owned()
            } else {
                format!("same code as {}", chart.full_name(e.id))
            };
            rows.push(skip(reason));
            continue;
        }
        if let Some(r) = &acc.role
            && let Some(h) = roles::holder(chart, r)
        {
            rows.push(skip(format!(
                "MoneyKeep already uses {} for this",
                chart.full_name(h.id)
            )));
            continue;
        }
        let parent_id = match &parent {
            ParentRef::Top => Some(None),
            ParentRef::Existing(id) => Some(Some(*id)),
            ParentRef::Planned(_) => None,
        };
        if let Some(pid) = parent_id
            && chart
                .accounts
                .iter()
                .any(|a| a.parent == pid && a.name.eq_ignore_ascii_case(&acc.name))
        {
            rows.push(skip("same name under the same parent".into()));
            continue;
        }
        let action = if matches!(parent, ParentRef::Planned(_)) {
            Action::AddUnder
        } else {
            Action::Add
        };
        planned.insert(acc.code.clone());
        rows.push(PlanRow {
            acc: acc.clone(),
            action,
            reason: None,
            parent,
        });
    }
    rows
}

fn plan_json(chart: &Chart, rows: &[PlanRow]) -> Value {
    let names: HashMap<&str, &str> = rows
        .iter()
        .map(|r| (r.acc.code.as_str(), r.acc.name.as_str()))
        .collect();
    let full = |r: &PlanRow| -> String {
        match &r.parent {
            ParentRef::Top => r.acc.name.clone(),
            ParentRef::Existing(id) => format!("{}:{}", chart.full_name(*id), r.acc.name),
            ParentRef::Planned(c) => format!(
                "{}:{}",
                names.get(c.as_str()).copied().unwrap_or_default(),
                r.acc.name
            ),
        }
    };
    let list: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "code": r.acc.code,
                "name": r.acc.name,
                "full_name": full(r),
                "type": r.acc.account_type,
                "header": r.acc.header,
                "parent": r.acc.parent,
                "tax_line": r.acc.tax_line,
                "action": match r.action {
                    Action::Add => "add",
                    Action::AddUnder => "add_under",
                    Action::Skip => "skip",
                },
                "reason": r.reason,
            })
        })
        .collect();
    let adds = rows.iter().filter(|r| r.action != Action::Skip).count();
    let skips = rows.len().saturating_sub(adds);
    json!({
        "accounts": list,
        "adds": adds,
        "skips": skips,
        "summary": summary(adds, skips),
    })
}

fn summary(adds: usize, skips: usize) -> String {
    let plural = |n: usize| if n == 1 { "account" } else { "accounts" };
    if skips == 0 {
        format!("Adds {adds} {}", plural(adds))
    } else {
        format!(
            "Adds {adds} {}; skips {skips} already in this book",
            plural(adds)
        )
    }
}

/// Create the added rows of `rows` (minus `exclude`, keeping headers that included
/// children need). Returns the accounts created, in creation order.
fn apply(
    ctx: &Ctx<'_>,
    rows: &[PlanRow],
    exclude: &HashSet<String>,
) -> CommandResult<Vec<Account>> {
    let mut wanted: HashSet<&str> = rows
        .iter()
        .filter(|r| r.action != Action::Skip && !exclude.contains(&r.acc.code))
        .map(|r| r.acc.code.as_str())
        .collect();
    for r in rows {
        if wanted.contains(r.acc.code.as_str())
            && let ParentRef::Planned(pc) = &r.parent
        {
            wanted.insert(pc.as_str());
        }
    }
    let base: CommodityCode = ctx.tx.info()?.base_currency.parse()?;
    let mut ids: HashMap<String, AccountId> = HashMap::new();
    let mut created = Vec::new();
    // Headers before children: the parent of a planned child is always a header
    // that appears in `rows`, so two passes are enough.
    for pass_headers in [true, false] {
        for r in rows {
            if r.acc.header != pass_headers || !wanted.contains(r.acc.code.as_str()) {
                continue;
            }
            let parent = match &r.parent {
                ParentRef::Top => None,
                ParentRef::Existing(id) => Some(*id),
                ParentRef::Planned(c) => ids.get(c).copied(),
            };
            let a = Account {
                id: AccountId::new(),
                name: r.acc.name.clone(),
                account_type: r.acc.account_type,
                subtype: r.acc.subtype,
                parent,
                commodity: base.clone(),
                code: Some(r.acc.code.clone()),
                description: r.acc.note.clone(),
                placeholder: r.acc.header,
                archived: false,
                created_at: now_rfc3339()?,
                tax_line: if r.acc.header {
                    None
                } else {
                    r.acc.tax_line.clone()
                },
                role: r.acc.role.clone(),
                retirement: false,
                opening_none: false,
            };
            ctx.save_account(None, &a)?;
            ids.insert(r.acc.code.clone(), a.id);
            created.push(a);
        }
    }
    Ok(created)
}

fn set_chart_settings(ctx: &Ctx<'_>, p: &Preset, only_if_none: bool) -> CommandResult<()> {
    if only_if_none
        && ctx
            .tx
            .get_setting(crate::modules::CORE, PRESET_KEY)?
            .is_some()
    {
        return Ok(());
    }
    ctx.save_setting(crate::modules::CORE, PRESET_KEY, Some(&json!(p.id)))?;
    if let Some(y) = p.tax_year {
        ctx.save_setting(crate::modules::CORE, TAX_YEAR_KEY, Some(&json!(y)))?;
    }
    Ok(())
}

// ----- commands --------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PreviewArgs {
    /// Preset id, as listed by `chart.presets`.
    preset: String,
}

fn preview_add(ctx: &Ctx<'_>, a: PreviewArgs) -> CommandResult<Value> {
    let p = find_preset(&a.preset)?;
    let seed = shipped()?;
    let mods = modules_on(ctx, seed)?;
    let chart = Chart::load(ctx)?;
    let rows = plan(&chart, &seed.resolve(p, &mods));
    let mut v = plan_json(&chart, &rows);
    if let Some(o) = v.as_object_mut() {
        o.insert("preset".into(), json!(p.id));
        o.insert("name".into(), json!(p.name));
    }
    Ok(v)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddArgs {
    /// Preset id, as listed by `chart.presets`.
    preset: String,
    /// Codes of accounts not to add (a header that an added account needs is still
    /// added).
    #[serde(default)]
    exclude: Vec<String>,
}

fn add(ctx: &Ctx<'_>, a: AddArgs) -> CommandResult<Value> {
    let p = find_preset(&a.preset)?;
    let seed = shipped()?;
    let mods = modules_on(ctx, seed)?;
    let chart = Chart::load(ctx)?;
    let rows = plan(&chart, &seed.resolve(p, &mods));
    let exclude: HashSet<String> = a.exclude.iter().map(|c| c.trim().to_owned()).collect();
    let created = apply(ctx, &rows, &exclude)?;
    set_chart_settings(ctx, p, true)?;
    let chart = Chart::load(ctx)?;
    let skips = rows.iter().filter(|r| r.action == Action::Skip).count();
    Ok(json!({
        "preset": p.id,
        "added": created.iter().map(|x| json!({"id": x.id, "code": x.code, "full_name": chart.full_name(x.id)})).collect::<Vec<_>>(),
        "summary": summary(created.len(), skips),
    }))
}

/// Book creation with a preset (chart-presets.md "book creation"): Opening Balances
/// gets code 3900; the preset's modules are switched on; every account of the
/// preset's packs and of the packs of the modules now on is created; the chart
/// settings are set.
pub(crate) fn apply_at_creation(ctx: &Ctx<'_>, preset: &str) -> CommandResult<Value> {
    let p = find_preset(preset)?;
    let seed = shipped()?;
    let chart = Chart::load(ctx)?;
    let opening = ctx.tx.opening_account_id()?;
    let ob_seed = seed
        .resolve(p, &[])
        .into_iter()
        .find(|a| a.role.as_deref() == Some(roles::OPENING_BALANCES));
    if let (Some(id), Some(s)) = (opening, ob_seed)
        && let Some(old) = chart.get(id)
    {
        let mut new = old.clone();
        new.code = Some(s.code.clone());
        new.description = s.note.clone();
        new.role = Some(roles::OPENING_BALANCES.to_owned());
        ctx.save_account(Some(old), &new)?;
    }
    for m in &p.modules {
        crate::builtin::enable_module(ctx, m)?;
    }
    let mods = modules_on(ctx, seed)?;
    let chart = Chart::load(ctx)?;
    let rows = plan(&chart, &seed.resolve(p, &mods));
    let created = apply(ctx, &rows, &HashSet::new())?;
    set_chart_settings(ctx, p, false)?;
    Ok(json!({ "preset": p.id, "accounts_created": created.len() }))
}

/// `module.enable` in a book created from (or extended with) a preset: add the packs
/// of the modules just switched on, with the same skip rules as `chart.preview_add`.
pub(crate) fn add_module_packs(ctx: &Ctx<'_>, newly: &[&str]) -> CommandResult<usize> {
    let Some(id) = ctx.tx.get_setting(crate::modules::CORE, PRESET_KEY)? else {
        return Ok(0);
    };
    let Some(id) = id.as_str() else {
        return Ok(0);
    };
    let seed = shipped()?;
    let Some(p) = seed.preset(id) else {
        return Ok(0);
    };
    let packs: Vec<&str> = newly.iter().filter_map(|m| seed.module_pack(m)).collect();
    if packs.is_empty() {
        return Ok(0);
    }
    let accounts: Vec<ChartAccount> = seed
        .resolve(p, newly)
        .into_iter()
        .filter(|a| packs.contains(&a.pack.as_str()))
        .collect();
    let chart = Chart::load(ctx)?;
    let rows = plan(&chart, &accounts);
    Ok(apply(ctx, &rows, &HashSet::new())?.len())
}

/// "Started from: <preset name> (tax year 2025)" facts for `book.info`.
pub(crate) fn info(ctx: &Ctx<'_>) -> CommandResult<Value> {
    let preset = ctx.tx.get_setting(crate::modules::CORE, PRESET_KEY)?;
    let year = ctx
        .tx
        .get_setting(crate::modules::CORE, TAX_YEAR_KEY)?
        .and_then(|v| v.as_i64());
    let id = preset.as_ref().and_then(Value::as_str);
    let name = id
        .and_then(|i| seed::shipped().ok().and_then(|s| s.preset(i)))
        .map(|p| p.name.clone());
    let started_from = name.as_ref().map(|n| match year {
        Some(y) => format!("{n} (tax year {y})"),
        None => n.clone(),
    });
    Ok(json!({
        "preset": id,
        "preset_name": name,
        "tax_year": year,
        "started_from": started_from,
    }))
}

#[cfg(test)]
mod tests;
