//! CSV profile commands (spec: import-csv.md).

use mk_core::brand;
use mk_import::CsvProfile;
use mk_store::{Doc, DocTable};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "csv.profile_save",
        "Create or replace a CSV import profile",
        true,
        save,
    );
    r.add("csv.profile_list", "List CSV import profiles", false, list);
    r.add(
        "csv.profile_delete",
        "Delete a CSV import profile",
        true,
        delete,
    );
    r.add(
        "csv.profile_export",
        "Export a CSV import profile as a JSON document",
        false,
        export,
    );
    r.add(
        "csv.profile_import",
        "Import a CSV profile from an exported JSON document",
        true,
        import,
    );
}

/// Profile documents are keyed by the lower-cased name (names are unique ignoring case).
fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

pub(crate) fn load_profiles(ctx: &Ctx<'_>) -> CommandResult<Vec<CsvProfile>> {
    ctx.tx
        .docs(DocTable::CsvProfile, None)?
        .iter()
        .map(|d| {
            serde_json::from_value(d.json.clone()).map_err(|e| {
                CommandError::Integrity(format!("bad stored CSV profile {}: {e}", d.id))
            })
        })
        .collect()
}

fn store(ctx: &Ctx<'_>, mut p: CsvProfile) -> CommandResult<Value> {
    p.name = p.name.trim().to_owned();
    p.validate()
        .map_err(|e| CommandError::InvalidArgs(e.to_string()))?;
    if let Some(acc) = &p.account {
        crate::accounts::Chart::load(ctx)?.resolve(acc)?;
    }
    let k = key(&p.name);
    let doc = Doc {
        id: k.clone(),
        grp: None,
        ord: 0,
        json: serde_json::to_value(&p)?,
    };
    ctx.save_doc(DocTable::CsvProfile, &k, Some(&doc))?;
    Ok(serde_json::to_value(&p)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveArgs {
    profile: CsvProfile,
}

fn save(ctx: &Ctx<'_>, a: SaveArgs) -> CommandResult<Value> {
    store(ctx, a.profile)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

fn list(ctx: &Ctx<'_>, _: NoArgs) -> CommandResult<Value> {
    let mut p = load_profiles(ctx)?;
    p.sort_by_key(|x| key(&x.name));
    Ok(serde_json::to_value(p)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NameArgs {
    name: String,
}

fn find(ctx: &Ctx<'_>, name: &str) -> CommandResult<CsvProfile> {
    let d = ctx
        .tx
        .get_doc(DocTable::CsvProfile, &key(name))?
        .ok_or_else(|| CommandError::NotFound(format!("CSV profile {name:?}")))?;
    serde_json::from_value(d.json).map_err(|e| CommandError::Integrity(e.to_string()))
}

fn delete(ctx: &Ctx<'_>, a: NameArgs) -> CommandResult<Value> {
    find(ctx, &a.name)?;
    ctx.save_doc(DocTable::CsvProfile, &key(&a.name), None)?;
    Ok(json!({ "deleted": a.name }))
}

/// Exported form: a self-describing document others can import.
fn export(ctx: &Ctx<'_>, a: NameArgs) -> CommandResult<Value> {
    let p = find(ctx, &a.name)?;
    let key = brand::CSV_PROFILE_HEADER;
    Ok(json!({ key: 1, "profile": p }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ImportArgs {
    /// A document produced by csv.profile_export.
    document: Value,
    /// Replace an existing profile with the same name. Default false.
    #[serde(default)]
    replace: bool,
}

fn import(ctx: &Ctx<'_>, a: ImportArgs) -> CommandResult<Value> {
    if a.document.get(brand::CSV_PROFILE_HEADER) != Some(&json!(1)) {
        return Err(CommandError::InvalidArgs(
            "not a MoneyKeep CSV profile export (version 1)".into(),
        ));
    }
    let p: CsvProfile =
        serde_json::from_value(a.document.get("profile").cloned().unwrap_or(Value::Null))?;
    if !a.replace
        && ctx
            .tx
            .get_doc(DocTable::CsvProfile, &key(&p.name))?
            .is_some()
    {
        return Err(CommandError::Rejected(format!(
            "a profile named {:?} exists; pass replace: true to overwrite it",
            p.name
        )));
    }
    store(ctx, p)
}

/// `csv.profile_save` / `csv.profile_delete` from JSON arguments (definition installs).
pub(crate) fn save_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    save(ctx, crate::parse_args(args)?)
}

pub(crate) fn delete_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    delete(ctx, crate::parse_args(args)?)
}
