//! Phase 0 built-in commands: book info, modules, settings.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::modules::{self, CORE};
use crate::{CommandError, CommandResult, Ctx, ENABLED_KEY, Registry, is_module_enabled};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "book.info",
        "Show the book's name, base currency, schema version and profile (description, icon, tile colour)",
        false,
        book_info,
    );
    r.add(
        "module.list",
        "List feature modules and whether each is enabled for this book",
        false,
        module_list,
    );
    r.add(
        "module.enable",
        "Turn a feature module on, along with any modules it requires",
        true,
        module_enable,
    );
    r.add(
        "module.disable",
        "Turn a feature module off (hides it; never deletes data or settings)",
        true,
        module_disable,
    );
    r.add(
        "setting.get",
        "Read one setting of a module",
        false,
        setting_get,
    );
    r.add(
        "setting.set",
        "Change one setting of an enabled module",
        true,
        setting_set,
    );
}

/// Arguments of a command that takes none.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoArgs {}

fn book_info(ctx: &Ctx<'_>, _: NoArgs) -> CommandResult<Value> {
    crate::book_profile::info_json(ctx)
}

fn module_list(ctx: &Ctx<'_>, _: NoArgs) -> CommandResult<Value> {
    let mut out = Vec::new();
    for m in modules::CATALOG {
        out.push(json!({
            "name": m.name,
            "description": m.description,
            "requires": m.requires,
            "has_settings": crate::module_settings::has_settings(m.name),
            "enabled": is_module_enabled(ctx.tx, m.name)?,
        }));
    }
    Ok(Value::Array(out))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ModuleArgs {
    /// Module name, as listed by `module.list`.
    module: String,
}

pub(crate) fn known_module(name: &str) -> CommandResult<&'static modules::ModuleSpec> {
    if name == CORE {
        return Err(CommandError::Rejected(
            "the core module is always on".into(),
        ));
    }
    modules::find(name).ok_or_else(|| CommandError::InvalidArgs(format!("unknown module {name:?}")))
}

fn module_enable(ctx: &Ctx<'_>, a: ModuleArgs) -> CommandResult<Value> {
    let (order, newly_enabled) = enable_module(ctx, &a.module)?;
    // chart-presets.md: a book with a starter chart also gets the module's accounts.
    let accounts_added = crate::chart::add_module_packs(ctx, &newly_enabled)?;
    Ok(json!({
        "enabled": order,
        "newly_enabled": newly_enabled,
        "accounts_added": accounts_added,
    }))
}

/// Turn `module` on with its requirements; returns (all of them, those newly on).
pub(crate) fn enable_module(
    ctx: &Ctx<'_>,
    module: &str,
) -> CommandResult<(Vec<&'static str>, Vec<&'static str>)> {
    let target = known_module(module)?;
    // Requirements first (depth-first), then the module itself.
    let mut order: Vec<&'static str> = Vec::new();
    collect_requirements(target, &mut order)?;
    let mut newly_enabled = Vec::new();
    for name in &order {
        if !is_module_enabled(ctx.tx, name)? {
            ctx.save_setting(name, ENABLED_KEY, Some(&Value::Bool(true)))?;
            newly_enabled.push(*name);
        }
    }
    if order.contains(&crate::dimensions::FUNDS) {
        crate::dimensions::ensure_general(ctx)?;
    }
    Ok((order, newly_enabled))
}

fn collect_requirements(
    m: &'static modules::ModuleSpec,
    out: &mut Vec<&'static str>,
) -> CommandResult<()> {
    if out.contains(&m.name) {
        return Ok(());
    }
    for r in m.requires {
        collect_requirements(known_module(r)?, out)?;
    }
    out.push(m.name);
    Ok(())
}

fn module_disable(ctx: &Ctx<'_>, a: ModuleArgs) -> CommandResult<Value> {
    let target = known_module(&a.module)?;
    let mut blockers = Vec::new();
    for d in modules::dependants(target.name) {
        if is_module_enabled(ctx.tx, d.name)? {
            blockers.push(d.name);
        }
    }
    if !blockers.is_empty() {
        return Err(CommandError::Rejected(format!(
            "{} is required by enabled module(s): {}; disable those first",
            target.name,
            blockers.join(", ")
        )));
    }
    ctx.save_setting(target.name, ENABLED_KEY, Some(&Value::Bool(false)))?;
    Ok(json!({ "disabled": target.name }))
}

/// Every enabled module's requirements are enabled too.
pub(crate) fn check_module_consistency(ctx: &Ctx<'_>) -> CommandResult<()> {
    for m in modules::CATALOG {
        if !is_module_enabled(ctx.tx, m.name)? {
            continue;
        }
        for r in m.requires {
            if !is_module_enabled(ctx.tx, r)? {
                return Err(CommandError::Rejected(format!(
                    "module {} requires {r}, which would be disabled",
                    m.name
                )));
            }
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SettingKeyArgs {
    module: String,
    key: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SettingSetArgs {
    module: String,
    key: String,
    /// Any JSON value.
    value: Value,
}

pub(crate) fn check_setting_target(ctx: &Ctx<'_>, module: &str, key: &str) -> CommandResult<()> {
    if crate::ai_access::is_ai_key(key) {
        return Err(CommandError::InvalidArgs(
            crate::ai_access::REFUSED_KEY.into(),
        ));
    }
    if key.is_empty() || key == ENABLED_KEY {
        return Err(CommandError::InvalidArgs(
            "use module.enable / module.disable to toggle a module".into(),
        ));
    }
    if module == CORE && key.starts_with("chart.") {
        return Err(CommandError::Rejected(
            "the book's starter chart settings are set by the program".into(),
        ));
    }
    if module != CORE {
        known_module(module)?;
        if !is_module_enabled(ctx.tx, module)? {
            return Err(CommandError::Rejected(format!(
                "module {module:?} is disabled; enable it to change its settings"
            )));
        }
    }
    Ok(())
}

fn setting_get(ctx: &Ctx<'_>, a: SettingKeyArgs) -> CommandResult<Value> {
    let value = ctx.tx.get_setting(&a.module, &a.key)?;
    Ok(json!({ "module": a.module, "key": a.key, "value": value }))
}

fn setting_set(ctx: &Ctx<'_>, a: SettingSetArgs) -> CommandResult<Value> {
    check_setting_target(ctx, &a.module, &a.key)?;
    // Reserved (investments-advanced.md): wash sales are flagged, not yet applied.
    if a.key == "investments.wash_sale_mode" && a.value != json!("flag") {
        return Err(CommandError::InvalidArgs(
            "investments.wash_sale_mode accepts only \"flag\" in this version".into(),
        ));
    }
    ctx.save_setting(&a.module, &a.key, Some(&a.value))?;
    Ok(json!({ "module": a.module, "key": a.key, "value": a.value }))
}
