//! Definition file commands (definition-files.md): inspect, install, list, uninstall,
//! export and validate `.mkdef` packages. A definition only supplies data; every
//! item is installed through the same validated command code as user-made objects,
//! and each install or uninstall is one audited, undoable command. This module reads
//! files only through the front end's file reader and never touches the network.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use base64::Engine;
use mk_core::{AccountId, AccountType};
use mk_import::definition::{
    DefItem, Definition, Header, ItemKind, canonical_bank_profile, canonical_inv_profile,
    canonical_rule, canonical_security, file_sha256, key_from_name, parse_definition,
    write_definition,
};
use mk_import::{Action, CsvProfile, InvProfile};
use mk_store::{Doc, DocTable};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::accounts::Chart;
use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "definition.inspect",
        "Read a definition file and show what installing it would do",
        false,
        inspect,
    );
    r.add(
        "definition.install",
        "Install the chosen items of an inspected definition file",
        true,
        install,
    );
    r.add(
        "definition.list",
        "List installed definition packages",
        false,
        list,
    );
    r.add(
        "definition.uninstall",
        "Remove a package's items that were not edited since install",
        true,
        uninstall,
    );
    r.add(
        "definition.export",
        "Write chosen profiles, rules, categories, securities, templates and dashboards as a definition file",
        false,
        export,
    );
    r.add(
        "definition.candidates",
        "What can go into a definition file: profiles, rules (with reasons when not), categories, securities",
        false,
        candidates,
    );
    r.add(
        "definition.validate",
        "Check a definition file for errors and warnings",
        false,
        validate,
    );
}

fn invalid(msg: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(msg.into())
}

// ----- input --------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FileArgs {
    /// The file as text.
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    content_base64: Option<String>,
    /// A path the front end reads (CLI).
    #[serde(default)]
    path: Option<String>,
    /// The file name, for display.
    #[serde(default)]
    name: Option<String>,
}

fn read_file(ctx: &Ctx<'_>, a: &FileArgs) -> CommandResult<Vec<u8>> {
    let given = [
        a.content.is_some(),
        a.content_base64.is_some(),
        a.path.is_some(),
    ]
    .iter()
    .filter(|x| **x)
    .count();
    if given != 1 {
        return Err(invalid(
            "give exactly one of content, content_base64 or path",
        ));
    }
    if let Some(c) = &a.content {
        return Ok(c.as_bytes().to_vec());
    }
    if let Some(b) = &a.content_base64 {
        return base64::engine::general_purpose::STANDARD
            .decode(b.trim())
            .map_err(|e| invalid(format!("content_base64 is not valid base64: {e}")));
    }
    let path = a.path.as_deref().unwrap_or_default();
    let reader = ctx.files.ok_or_else(|| {
        invalid("this front end cannot read files; send the file as content or content_base64")
    })?;
    reader
        .read(path)
        .map_err(|e| invalid(format!("cannot read {path}: {e}")))
}

fn file_name(a: &FileArgs) -> Option<String> {
    a.name.clone().or_else(|| {
        a.path
            .as_ref()
            .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p).to_owned())
    })
}

// ----- the book's side ----------------------------------------------------------------

fn hash(v: &Value) -> String {
    Sha256::digest(v.to_string().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn type_word(t: AccountType) -> Option<&'static str> {
    match t {
        AccountType::Income => Some("income"),
        AccountType::Expense => Some("expense"),
        _ => None,
    }
}

/// A rule as stored, with its category references written as paths; `None` when it
/// names a non-category account (or targets specific accounts) and so cannot be
/// written to a definition.
fn rule_canonical(
    chart: &Chart,
    tag_names: &HashMap<String, String>,
    r: &mk_import::Rule,
) -> Result<Value, String> {
    if !r.accounts.is_empty() {
        return Err("it applies only to specific accounts".into());
    }
    let path = |id: &str| -> Result<String, String> {
        let acc = chart
            .resolve(id)
            .map_err(|_| format!("it names a missing account {id}"))?;
        if type_word(acc.account_type).is_none() {
            return Err(format!(
                "it names {}, which is not a category",
                chart.full_name(acc.id)
            ));
        }
        Ok(chart.full_name(acc.id))
    };
    let mut actions = Vec::new();
    for a in &r.actions {
        actions.push(match a {
            Action::SetAccount { account } => Action::SetAccount {
                account: path(account)?,
            },
            Action::Split { shares } => Action::Split {
                shares: shares
                    .iter()
                    .map(|s| {
                        Ok(mk_import::SplitShare {
                            account: path(&s.account)?,
                            percent: s.percent,
                        })
                    })
                    .collect::<Result<_, String>>()?,
            },
            Action::SetClass { class } => Action::SetClass {
                class: tag_names
                    .get(class)
                    .cloned()
                    .unwrap_or_else(|| class.clone()),
            },
            Action::SetLocation { location } => Action::SetLocation {
                location: tag_names
                    .get(location)
                    .cloned()
                    .unwrap_or_else(|| location.clone()),
            },
            Action::SetFund { fund } => Action::SetFund {
                fund: tag_names.get(fund).cloned().unwrap_or_else(|| fund.clone()),
            },
            Action::SetProject { project } => Action::SetProject {
                project: tag_names
                    .get(project)
                    .cloned()
                    .unwrap_or_else(|| project.clone()),
            },
            other => other.clone(),
        });
    }
    Ok(canonical_rule(
        &r.name,
        r.enabled,
        &r.conditions,
        &actions,
        r.stop,
    ))
}

/// Everything in the book a definition item can be compared with.
struct Book {
    chart: Chart,
    /// Lower-cased full name → account.
    by_path: HashMap<String, AccountId>,
    bank: HashMap<String, CsvProfile>,
    inv: HashMap<String, InvProfile>,
    rules: Vec<mk_import::Rule>,
    securities: Vec<crate::investments::Security>,
    /// Class, location and fund id → name.
    tag_names: HashMap<String, String>,
    templates: Vec<crate::templates::Template>,
    /// Installed OFX institution profiles by lower-cased name.
    ofx: HashMap<String, mk_import::ofx_dc::Institution>,
    /// Dashboards: (id, name, portable form with names for references).
    dashboards: Vec<(String, String, Value)>,
}

impl Book {
    fn load(ctx: &Ctx<'_>) -> CommandResult<Self> {
        let chart = Chart::load(ctx)?;
        let by_path = chart
            .accounts
            .iter()
            .map(|a| (chart.full_name(a.id).to_lowercase(), a.id))
            .collect();
        let bank = crate::profiles::load_profiles(ctx)?
            .into_iter()
            .map(|p| (p.name.to_lowercase(), p))
            .collect();
        let inv = crate::inv_import::all_profiles(ctx)?
            .into_iter()
            .map(|p| (p.name.to_lowercase(), p))
            .collect();
        let rules = crate::rules::load_rules(ctx)?
            .iter()
            .map(crate::rules::rule_from_doc)
            .collect::<CommandResult<_>>()?;
        let securities = crate::investments::securities(ctx)?;
        let mut tag_names = HashMap::new();
        for dim in ["class", "location", "fund"] {
            for r in crate::dimensions::records(ctx, dim)? {
                tag_names.insert(r.id, r.name);
            }
        }
        for p in crate::projects::all(ctx)? {
            tag_names.insert(p.id, p.name);
        }
        let mut dashboards = Vec::new();
        for d in crate::dashboards::load_all(ctx)? {
            let p = crate::dashboards::portable(ctx, &d)?;
            dashboards.push((d.id, d.name, p));
        }
        Ok(Self {
            dashboards,
            chart,
            by_path,
            bank,
            inv,
            rules,
            securities,
            tag_names,
            templates: crate::templates::templates(ctx)?,
            ofx: crate::ofx_conn::institutions(ctx)?
                .into_iter()
                .map(|i| (i.name.to_lowercase(), i))
                .collect(),
        })
    }

    fn category(&self, path: &str) -> Option<&mk_core::Account> {
        self.by_path
            .get(&path.to_lowercase())
            .and_then(|id| self.chart.get(*id))
    }

    fn category_canonical(&self, id: AccountId) -> Option<Value> {
        let a = self.chart.get(id)?;
        Some(mk_import::definition::canonical_category_tax(
            &self.chart.full_name(id),
            type_word(a.account_type)?,
            a.description.as_deref(),
            a.tax_line.as_deref(),
        ))
    }

    fn security_canonical(s: &crate::investments::Security) -> Value {
        let word = |v: Value| v.as_str().unwrap_or_default().to_owned();
        canonical_security(
            &s.symbol,
            &s.name,
            &word(serde_json::to_value(s.kind).unwrap_or(Value::Null)),
            &word(serde_json::to_value(s.basis_method).unwrap_or(Value::Null)),
            s.cusip.as_deref(),
        )
    }

    /// The current canonical form of an installed object, by kind and object ref.
    fn current(&self, kind: ItemKind, object: &str) -> Option<Value> {
        match kind {
            ItemKind::Category => self.category_canonical(object.parse().ok()?),
            ItemKind::Security => self
                .securities
                .iter()
                .find(|s| s.id == object)
                .map(Self::security_canonical),
            ItemKind::BankProfile => self
                .bank
                .get(&object.to_lowercase())
                .map(canonical_bank_profile),
            ItemKind::InvestmentProfile => self
                .inv
                .get(&object.to_lowercase())
                .map(canonical_inv_profile),
            ItemKind::Rule => self
                .rules
                .iter()
                .find(|r| r.id.to_string() == object)
                .and_then(|r| rule_canonical(&self.chart, &self.tag_names, r).ok()),
            ItemKind::DocumentTemplate => self
                .templates
                .iter()
                .find(|t| t.id == object)
                .map(template_canonical),
            ItemKind::OfxInstitution => self
                .ofx
                .get(&object.to_lowercase())
                .map(mk_import::definition::canonical_ofx_institution),
            ItemKind::Dashboard => self
                .dashboards
                .iter()
                .find(|d| d.0 == object)
                .map(|d| dashboard_canonical(&d.2)),
        }
    }

    /// An object of the same kind and name from anywhere: (object ref, canonical).
    fn by_name(&self, item: &DefItem) -> Option<(String, Option<Value>)> {
        match item.kind {
            ItemKind::Category => {
                let a = self.category(&item.name)?;
                Some((a.id.to_string(), self.category_canonical(a.id)))
            }
            ItemKind::Security => self
                .securities
                .iter()
                .find(|s| s.symbol.eq_ignore_ascii_case(&item.name))
                .map(|s| (s.id.clone(), Some(Self::security_canonical(s)))),
            ItemKind::BankProfile => self
                .bank
                .get(&item.name.to_lowercase())
                .map(|p| (p.name.clone(), Some(canonical_bank_profile(p)))),
            ItemKind::InvestmentProfile => self
                .inv
                .get(&item.name.to_lowercase())
                .map(|p| (p.name.clone(), Some(canonical_inv_profile(p)))),
            ItemKind::Rule => {
                // Rule names are not unique: only an identical rule counts.
                self.rules.iter().find_map(|r| {
                    let c = rule_canonical(&self.chart, &self.tag_names, r).ok()?;
                    (c == comparable(item)).then(|| (r.id.to_string(), Some(c)))
                })
            }
            ItemKind::DocumentTemplate => self
                .templates
                .iter()
                .find(|t| t.name.eq_ignore_ascii_case(&item.name))
                .map(|t| (t.id.clone(), Some(template_canonical(t)))),
            ItemKind::OfxInstitution => self.ofx.get(&item.name.to_lowercase()).map(|i| {
                (
                    i.name.clone(),
                    Some(mk_import::definition::canonical_ofx_institution(i)),
                )
            }),
            ItemKind::Dashboard => self
                .dashboards
                .iter()
                .find(|d| d.1.eq_ignore_ascii_case(&item.name))
                .map(|d| (d.0.clone(), Some(dashboard_canonical(&d.2)))),
        }
    }
}

/// A dashboard's definition form (references by name).
fn dashboard_canonical(portable: &Value) -> Value {
    let m = portable.as_object().cloned().unwrap_or_default();
    mk_import::definition::canonical_dashboard(&m)
}

/// A template's definition form (never the logo).
fn template_canonical(t: &crate::templates::Template) -> Value {
    let v = crate::templates::template_json(t);
    let m = v.as_object().cloned().unwrap_or_default();
    mk_import::definition::canonical_template(&m)
}

/// An item's fields as compared with the book (category codes are not compared).
fn comparable(item: &DefItem) -> Value {
    let mut v = item.fields.clone();
    if item.kind == ItemKind::Category
        && let Some(o) = v.as_object_mut()
    {
        o.remove("code");
    }
    v
}

// ----- packages -----------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PackageItem {
    kind: String,
    key: String,
    name: String,
    /// Account id, rule id, security id, or profile name.
    object: String,
    /// Hash of the object's canonical form right after install.
    hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Package {
    id: String,
    version: String,
    name: String,
    header: Value,
    sha256: String,
    file_name: Option<String>,
    installed_at: String,
    items: Vec<PackageItem>,
    #[serde(default)]
    extensions: Map<String, Value>,
}

fn kind_of(s: &str) -> Option<ItemKind> {
    ItemKind::ALL.into_iter().find(|k| k.table() == s)
}

fn load_packages(ctx: &Ctx<'_>) -> CommandResult<Vec<Package>> {
    ctx.tx
        .docs(DocTable::Definition, None)?
        .into_iter()
        .map(|d| {
            serde_json::from_value(d.json)
                .map_err(|e| CommandError::Integrity(format!("bad stored definition: {e}")))
        })
        .collect()
}

fn save_package(ctx: &Ctx<'_>, p: &Package) -> CommandResult<()> {
    ctx.save_doc(
        DocTable::Definition,
        &p.id,
        Some(&Doc {
            id: p.id.clone(),
            grp: None,
            ord: 0,
            json: serde_json::to_value(p)?,
        }),
    )
}

/// Whether an installed item was changed since install (or no longer exists).
fn edited(book: &Book, it: &PackageItem) -> Option<bool> {
    let kind = kind_of(&it.kind)?;
    Some(match book.current(kind, &it.object) {
        Some(c) => hash(&c) != it.hash,
        None => true,
    })
}

// ----- inspect ------------------------------------------------------------------------

/// What installing an item would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Proposal {
    Add,
    Update,
    Conflict,
    Unchanged,
}

struct Planned<'a> {
    item: &'a DefItem,
    proposal: Proposal,
    /// The existing object this item updates or conflicts with.
    existing: Option<String>,
    /// Updating would overwrite local edits.
    edited: bool,
    notes: Vec<String>,
}

fn plan<'a>(book: &Book, def: &'a Definition, installed: Option<&Package>) -> Vec<Planned<'a>> {
    let in_file: BTreeSet<String> = def
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::Category)
        .map(|i| i.name.to_lowercase())
        .collect();
    def.items
        .iter()
        .map(|item| {
            let mut notes = Vec::new();
            let mine = installed.and_then(|p| {
                p.items
                    .iter()
                    .find(|x| x.kind == item.kind.table() && x.key == item.key)
            });
            let current = mine.and_then(|m| book.current(item.kind, &m.object).map(|c| (m, c)));
            let (proposal, existing, edited) = match current {
                Some((m, c)) => {
                    if c == comparable(item) {
                        (Proposal::Unchanged, Some(m.object.clone()), false)
                    } else {
                        (Proposal::Update, Some(m.object.clone()), hash(&c) != m.hash)
                    }
                }
                None => match book.by_name(item) {
                    Some((obj, c)) => {
                        if c.as_ref() == Some(&comparable(item)) {
                            (Proposal::Unchanged, Some(obj), false)
                        } else {
                            (Proposal::Conflict, Some(obj), false)
                        }
                    }
                    None => (Proposal::Add, None, false),
                },
            };
            if item.kind == ItemKind::Category
                && proposal == Proposal::Conflict
                && let Some(a) = book.category(&item.name)
                && type_word(a.account_type) != item.fields.get("type").and_then(Value::as_str)
            {
                notes.push(format!(
                    "{} already exists as a different type of account",
                    book.chart.full_name(a.id)
                ));
            }
            if item.kind == ItemKind::Rule {
                for p in rule_paths(item) {
                    let in_book = book
                        .category(&p)
                        .is_some_and(|a| type_word(a.account_type).is_some());
                    if !in_book && !in_file.contains(&p.to_lowercase()) {
                        notes.push(format!(
                            "the category {p} does not exist; the rule will be installed turned off"
                        ));
                    }
                }
            }
            if edited {
                notes.push("changed in this book since it was installed".into());
            }
            Planned {
                item,
                proposal,
                existing,
                edited,
                notes,
            }
        })
        .collect()
}

fn rule_paths(item: &DefItem) -> Vec<String> {
    let actions: Vec<Action> = item
        .fields
        .get("actions")
        .cloned()
        .and_then(|a| serde_json::from_value(a).ok())
        .unwrap_or_default();
    actions
        .iter()
        .flat_map(|a| match a {
            Action::SetAccount { account } => vec![account.clone()],
            Action::Split { shares } => shares.iter().map(|s| s.account.clone()).collect(),
            _ => Vec::new(),
        })
        .collect()
}

fn count_phrase(counts: &BTreeMap<ItemKind, usize>) -> String {
    let parts: Vec<String> = counts
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(k, n)| {
            let (one, many) = k.words();
            format!("{n} {}", if *n == 1 { one } else { many })
        })
        .collect();
    match parts.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// Summary order: profiles first, as people read them.
fn summary_rank(k: ItemKind) -> usize {
    match k {
        ItemKind::InvestmentProfile => 0,
        ItemKind::BankProfile => 1,
        ItemKind::Category => 2,
        ItemKind::Rule => 3,
        ItemKind::Security => 4,
        ItemKind::DocumentTemplate => 5,
        ItemKind::OfxInstitution => 6,
        ItemKind::Dashboard => 7,
    }
}

fn summary(planned: &[Planned<'_>], warnings: usize) -> String {
    let mut add: Vec<(usize, ItemKind, usize)> = Vec::new();
    let mut upd: Vec<(usize, ItemKind, usize)> = Vec::new();
    for p in planned {
        let list = match p.proposal {
            Proposal::Add => &mut add,
            Proposal::Update => &mut upd,
            _ => continue,
        };
        match list.iter_mut().find(|(_, k, _)| *k == p.item.kind) {
            Some(x) => x.2 += 1,
            None => list.push((summary_rank(p.item.kind), p.item.kind, 1)),
        }
    }
    let phrase = |l: &mut Vec<(usize, ItemKind, usize)>| {
        l.sort();
        let parts: Vec<String> = l
            .iter()
            .map(|(_, k, n)| {
                let (one, many) = k.words();
                format!("{n} {}", if *n == 1 { one } else { many })
            })
            .collect();
        match parts.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [init @ .., last] => format!("{} and {last}", init.join(", ")),
        }
    };
    let mut out = Vec::new();
    let a = phrase(&mut add);
    if !a.is_empty() {
        out.push(format!("Adds {a}"));
    }
    let u = phrase(&mut upd);
    if !u.is_empty() {
        out.push(if out.is_empty() {
            format!("Updates {u}")
        } else {
            format!("updates {u}")
        });
    }
    let conflicts = planned
        .iter()
        .filter(|p| p.proposal == Proposal::Conflict)
        .count();
    if conflicts > 0 {
        out.push(format!(
            "{conflicts} conflict{}",
            if conflicts == 1 { "" } else { "s" }
        ));
    }
    if out.is_empty() {
        out.push("Nothing to install: everything is unchanged".into());
    }
    if warnings > 0 {
        out.push(format!(
            "{warnings} warning{}",
            if warnings == 1 { "" } else { "s" }
        ));
    }
    out.join("; ")
}

fn header_json(h: &Header) -> Value {
    serde_json::to_value(h).unwrap_or(Value::Null)
}

fn default_choice(p: &Planned<'_>) -> &'static str {
    match p.proposal {
        Proposal::Add => "install",
        Proposal::Update if !p.edited => "install",
        _ => "skip",
    }
}

fn planned_json(p: &Planned<'_>) -> Value {
    json!({
        "kind": p.item.kind,
        "key": p.item.key,
        "name": p.item.name,
        "action": p.proposal,
        "default": default_choice(p),
        "edited": p.edited,
        "notes": p.notes,
        "fields": p.item.fields,
    })
}

fn parse_or_errors(bytes: &[u8]) -> Result<Definition, Value> {
    parse_definition(bytes).map_err(|errors| json!({"valid": false, "errors": errors}))
}

fn inspect(ctx: &Ctx<'_>, a: FileArgs) -> CommandResult<Value> {
    let bytes = read_file(ctx, &a)?;
    let def = match parse_or_errors(&bytes) {
        Ok(d) => d,
        Err(v) => return Ok(v),
    };
    let book = Book::load(ctx)?;
    let packages = load_packages(ctx)?;
    let installed = packages.iter().find(|p| p.id == def.header.id);
    let planned = plan(&book, &def, installed);
    // connectivity-brokerage.md: bank connection addresses decide where a password goes.
    let hosts: Vec<Value> = def
        .items
        .iter()
        .filter(|i| i.kind == ItemKind::OfxInstitution)
        .map(|i| {
            let url = i
                .fields
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default();
            json!({"key": i.key, "name": i.name,
                "host": mk_import::ofx_dc::host_of(url).unwrap_or_default()})
        })
        .collect();
    let mut warnings = def.warnings.clone();
    if !hosts.is_empty() {
        warnings.insert(
            0,
            "This file contains bank connection addresses. Only install it if you trust its author."
                .to_owned(),
        );
    }
    Ok(json!({
        "valid": true,
        "file_name": file_name(&a),
        "header": header_json(&def.header),
        "sha256": def.sha256,
        "installed_version": installed.map(|p| p.version.clone()),
        "items": planned.iter().map(planned_json).collect::<Vec<_>>(),
        "bank_hosts": hosts,
        "warnings": warnings,
        "errors": [],
        "summary": summary(&planned, def.warnings.len()),
    }))
}

fn validate(_ctx: &Ctx<'_>, a: FileArgs) -> CommandResult<Value> {
    let bytes = read_file(_ctx, &a)?;
    match parse_definition(&bytes) {
        Ok(d) => {
            let mut counts: BTreeMap<ItemKind, usize> = BTreeMap::new();
            for i in &d.items {
                *counts.entry(i.kind).or_default() += 1;
            }
            Ok(json!({
                "valid": true,
                "errors": [],
                "warnings": d.warnings,
                "sha256": d.sha256,
                "summary": format!("Valid: {}", {
                    let c = count_phrase(&counts);
                    if c.is_empty() { "no items".to_owned() } else { c }
                }),
            }))
        }
        Err(errors) => Ok(json!({"valid": false, "errors": errors, "warnings": []})),
    }
}

// ----- install ------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Choice {
    kind: String,
    key: String,
    /// `install`, `skip`, and for conflicts `replace` or `rename`.
    choice: String,
    /// The new name for `rename` (profile or security name, category path).
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InstallArgs {
    #[serde(flatten)]
    file: FileArgs,
    /// The SHA-256 `definition.inspect` reported; the file must still match it.
    sha256: String,
    /// Per-item choices; items not listed take their default.
    #[serde(default)]
    choices: Vec<Choice>,
}

fn install(ctx: &Ctx<'_>, a: InstallArgs) -> CommandResult<Value> {
    let bytes = read_file(ctx, &a.file)?;
    let digest = file_sha256(&bytes);
    let fname = file_name(&a.file);
    *ctx.audit_args.borrow_mut() = Some(json!({
        "name": fname, "sha256": digest, "bytes": bytes.len(),
    }));
    if !digest.eq_ignore_ascii_case(a.sha256.trim()) {
        return Err(CommandError::Rejected(
            "the file is not the one that was inspected (its SHA-256 differs); inspect it again"
                .into(),
        ));
    }
    let def = parse_definition(&bytes).map_err(|e| invalid(e.join("\n")))?;
    let book = Book::load(ctx)?;
    let mut packages = load_packages(ctx)?;
    let pos = packages.iter().position(|p| p.id == def.header.id);
    let installed = pos.and_then(|i| packages.get(i)).cloned();
    let planned = plan(&book, &def, installed.as_ref());
    let choice_of = |p: &Planned<'_>| -> (String, Option<String>) {
        a.choices
            .iter()
            .find(|c| c.kind == p.item.kind.table() && c.key == p.item.key)
            .map_or((default_choice(p).to_owned(), None), |c| {
                (c.choice.clone(), c.name.clone())
            })
    };
    let mut items: Vec<PackageItem> = installed
        .as_ref()
        .map(|p| p.items.clone())
        .unwrap_or_default();
    let mut done = 0usize;
    let mut notes: Vec<String> = Vec::new();
    let mut rule_base = crate::rules::load_rules(ctx)?
        .iter()
        .map(|d| d.ord)
        .max()
        .map_or(0, |m| m.saturating_add(1));
    let mut sorted: Vec<&Planned<'_>> = planned.iter().collect();
    sorted.sort_by(|x, y| {
        let depth = |p: &Planned<'_>| p.item.name.matches(':').count();
        (
            x.item.kind,
            depth(x),
            x.item.priority.unwrap_or(0),
            &x.item.key,
        )
            .cmp(&(
                y.item.kind,
                depth(y),
                y.item.priority.unwrap_or(0),
                &y.item.key,
            ))
    });
    for p in sorted {
        let (choice, rename) = choice_of(p);
        let what = match (p.proposal, choice.as_str()) {
            (_, "skip") | (Proposal::Unchanged, _) => continue,
            (Proposal::Add, "install") => Op::Add(None),
            (Proposal::Update, "install" | "replace") => Op::Replace,
            (Proposal::Conflict, "replace") => Op::Replace,
            (Proposal::Conflict | Proposal::Add, "rename") => {
                let n = rename
                    .filter(|n| !n.trim().is_empty())
                    .ok_or_else(|| invalid(format!("rename {}: give the new name", p.item.key)))?;
                Op::Add(Some(n.trim().to_owned()))
            }
            (_, other) => {
                return Err(invalid(format!(
                    "{} {}: choice {other:?} does not apply to a{} {} item",
                    p.item.kind.table(),
                    p.item.key,
                    if p.proposal == Proposal::Add || p.proposal == Proposal::Update {
                        "n"
                    } else {
                        ""
                    },
                    match p.proposal {
                        Proposal::Add => "added",
                        Proposal::Update => "updated",
                        Proposal::Conflict => "conflicting",
                        Proposal::Unchanged => "unchanged",
                    }
                )));
            }
        };
        let (object, note) = apply(ctx, p, &what, &mut rule_base)?;
        if let Some(n) = note {
            notes.push(n);
        }
        // Record provenance with the object's canonical form as installed.
        let after = Book::load(ctx)?;
        let canonical = after.current(p.item.kind, &object).unwrap_or(Value::Null);
        let name = match &what {
            Op::Add(Some(n)) => n.clone(),
            _ => p.item.name.clone(),
        };
        items.retain(|x| !(x.kind == p.item.kind.table() && x.key == p.item.key));
        items.push(PackageItem {
            kind: p.item.kind.table().into(),
            key: p.item.key.clone(),
            name,
            object,
            hash: hash(&canonical),
        });
        done += 1;
    }
    if done == 0 {
        return Ok(json!({
            "installed": 0, "id": def.header.id, "name": def.header.name,
            "message": "Nothing to install: everything is unchanged or skipped",
        }));
    }
    let pkg = Package {
        id: def.header.id.clone(),
        version: def.header.version.clone(),
        name: def.header.name.clone(),
        header: header_json(&def.header),
        sha256: def.sha256.clone(),
        file_name: fname,
        installed_at: mk_store::now_rfc3339()?,
        items,
        extensions: def.extensions.clone(),
    };
    save_package(ctx, &pkg)?;
    if let Some(i) = pos
        && let Some(slot) = packages.get_mut(i)
    {
        *slot = pkg;
    }
    Ok(json!({
        "installed": done,
        "id": def.header.id,
        "name": def.header.name,
        "version": def.header.version,
        "notes": notes,
        "message": format!("Installed {}: {done} item{}", def.header.name, if done == 1 { "" } else { "s" }),
    }))
}

enum Op {
    /// Create a new object (optionally under another name).
    Add(Option<String>),
    /// Overwrite the existing object.
    Replace,
}

/// Install one item; returns the object reference and an optional note.
fn apply(
    ctx: &Ctx<'_>,
    p: &Planned<'_>,
    op: &Op,
    rule_base: &mut i64,
) -> CommandResult<(String, Option<String>)> {
    let item = p.item;
    let rename = match op {
        Op::Add(n) => n.clone(),
        Op::Replace => None,
    };
    let f = &item.fields;
    match item.kind {
        ItemKind::Dashboard => {
            let note = |notes: Vec<String>| (!notes.is_empty()).then(|| notes.join("; "));
            if let (Op::Replace, Some(obj)) = (op, &p.existing) {
                let notes = crate::dashboards::replace_from_portable(ctx, obj, f)?;
                return Ok((obj.clone(), note(notes)));
            }
            let name = rename.unwrap_or_else(|| item.name.clone());
            let (id, notes) = crate::dashboards::create_from_portable(ctx, &name, f)?;
            Ok((id, note(notes)))
        }
        ItemKind::OfxInstitution => {
            let mut inst: mk_import::ofx_dc::Institution = serde_json::from_value(f.clone())
                .map_err(|e| invalid(format!("{}: {e}", item.key)))?;
            if let Some(n) = rename {
                inst.name = n;
            }
            inst.check().map_err(|e| invalid(e.to_string()))?;
            if let (Op::Replace, Some(obj)) = (op, &p.existing)
                && !obj.eq_ignore_ascii_case(&inst.name)
            {
                crate::ofx_conn::delete_institution(ctx, obj)?;
            }
            // Installing never connects anything; existing connections keep their copy.
            crate::ofx_conn::save_institution(ctx, &inst)?;
            Ok((inst.name, None))
        }
        ItemKind::DocumentTemplate => {
            let name = rename.unwrap_or_else(|| item.name.clone());
            let mut args = f.clone();
            if let Some(o) = args.as_object_mut() {
                o.insert("name".into(), json!(name));
                o.remove("key");
            }
            if let (Op::Replace, Some(obj)) = (op, &p.existing) {
                if let Some(o) = args.as_object_mut() {
                    o.insert("template".into(), json!(obj));
                }
                crate::templates::update_json(ctx, args)?;
                return Ok((obj.clone(), None));
            }
            let v = crate::templates::create_json(ctx, args)?;
            Ok((
                v.get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                None,
            ))
        }
        ItemKind::Category => {
            let path = rename.unwrap_or_else(|| item.name.clone());
            let kind = f.get("type").and_then(Value::as_str).unwrap_or("expense");
            let desc = f.get("description").cloned().unwrap_or(Value::Null);
            let tax_line = f.get("tax_line").cloned().unwrap_or(Value::Null);
            if let (Op::Replace, Some(obj)) = (op, &p.existing) {
                let chart = Chart::load(ctx)?;
                let acc = chart.resolve(obj)?;
                if type_word(acc.account_type) != Some(kind) {
                    return Err(CommandError::Rejected(format!(
                        "{} is a different type of account; rename the category instead",
                        chart.full_name(acc.id)
                    )));
                }
                let placeholder = acc.placeholder;
                let mut args = json!({"account": obj, "description": desc});
                if !placeholder
                    && !tax_line.is_null()
                    && let Some(o) = args.as_object_mut()
                {
                    o.insert("tax_line".into(), tax_line.clone());
                }
                crate::accounts::update_json(ctx, args)?;
                return Ok((obj.clone(), None));
            }
            let id = ensure_category(ctx, &path, kind, &desc)?;
            if !tax_line.is_null() {
                let chart = Chart::load(ctx)?;
                if chart.get(id).is_some_and(|a| !a.placeholder) {
                    crate::accounts::update_json(
                        ctx,
                        json!({"account": id.to_string(), "tax_line": tax_line}),
                    )?;
                }
            }
            Ok((id.to_string(), None))
        }
        ItemKind::Security => {
            let s = |k: &str| f.get(k).and_then(Value::as_str).map(str::to_owned);
            if let (Op::Replace, Some(obj)) = (op, &p.existing) {
                crate::investments::security_update_json(
                    ctx,
                    json!({"security": obj, "name": s("name"), "kind": s("kind"),
                           "basis_method": s("basis_method"), "cusip": s("cusip")}),
                )?;
                return Ok((obj.clone(), None));
            }
            let kind: crate::investments::SecurityKind =
                serde_json::from_value(f.get("kind").cloned().unwrap_or(json!("stock")))
                    .map_err(|e| invalid(e.to_string()))?;
            let method: Option<crate::investments::BasisMethod> = f
                .get("basis_method")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| invalid(e.to_string()))?;
            let symbol = rename.unwrap_or_else(|| item.name.clone());
            let sec = crate::investments::create_security(
                ctx,
                &symbol,
                &s("name").unwrap_or_default(),
                kind,
                method,
                s("cusip"),
            )?;
            Ok((sec.id, None))
        }
        ItemKind::BankProfile => {
            let mut prof = f.clone();
            let name = rename.unwrap_or_else(|| item.name.clone());
            if let Some(o) = prof.as_object_mut() {
                o.insert("name".into(), json!(name));
            }
            if let (Op::Replace, Some(obj)) = (op, &p.existing)
                && !obj.eq_ignore_ascii_case(&name)
            {
                crate::profiles::delete_json(ctx, json!({"name": obj}))?;
            }
            crate::profiles::save_json(ctx, json!({"profile": prof}))?;
            Ok((name, None))
        }
        ItemKind::InvestmentProfile => {
            let mut prof = f.clone();
            let name = rename.unwrap_or_else(|| item.name.clone());
            if let Some(o) = prof.as_object_mut() {
                o.insert("name".into(), json!(name));
            }
            crate::inv_import::profile_save_json(ctx, json!({"profile": prof}))?;
            Ok((name, None))
        }
        ItemKind::Rule => {
            let chart = Chart::load(ctx)?;
            let mut missing = Vec::new();
            for path in rule_paths(item) {
                let ok = chart
                    .resolve(&path)
                    .is_ok_and(|a| type_word(a.account_type).is_some());
                if !ok {
                    missing.push(path);
                }
            }
            let mut args = f.clone();
            let mut note = None;
            // Tag actions naming classes, locations or funds this book lacks: the rule is
            // installed turned off without them.
            let mut missing_tags = Vec::new();
            if let Some(acts) = args.get_mut("actions").and_then(Value::as_array_mut) {
                acts.retain(|x| {
                    let (dim, key) = match x.get("action").and_then(Value::as_str) {
                        Some("set_class") => ("class", "class"),
                        Some("set_location") => ("location", "location"),
                        Some("set_fund") => ("fund", "fund"),
                        Some("set_project") => ("project", "project"),
                        _ => return true,
                    };
                    let name = x.get(key).and_then(Value::as_str).unwrap_or_default();
                    let ok = match dim {
                        "class" => crate::dimensions::resolve_tags(ctx, Some(name), None, None),
                        "location" => crate::dimensions::resolve_tags(ctx, None, Some(name), None),
                        "project" => crate::dimensions::resolve_tags_p(
                            ctx,
                            None,
                            None,
                            None,
                            Some(name),
                            None,
                        ),
                        _ => crate::dimensions::resolve_tags(ctx, None, None, Some(name)),
                    }
                    .is_ok();
                    if !ok {
                        missing_tags.push(format!("{dim} {name}"));
                    }
                    ok
                });
                if acts.is_empty() {
                    acts.push(json!({"action": "skip"}));
                }
            }
            if !missing_tags.is_empty() {
                if let Some(o) = args.as_object_mut() {
                    o.insert("enabled".into(), json!(false));
                }
                note = Some(format!(
                    "rule {} installed turned off: {} does not exist",
                    item.name,
                    missing_tags.join(", ")
                ));
            }
            if let Some(o) = args.as_object_mut()
                && !missing.is_empty()
            {
                // Leave the actions out of reach until the categories exist.
                o.insert("enabled".into(), json!(false));
                let keep: Vec<Value> = o
                    .get("actions")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter(|x| {
                                !matches!(
                                    x.get("action").and_then(Value::as_str),
                                    Some("set_account" | "split")
                                )
                            })
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                o.insert(
                    "actions".into(),
                    if keep.is_empty() {
                        json!([{"action": "skip"}])
                    } else {
                        Value::Array(keep)
                    },
                );
                note = Some(format!(
                    "rule {} installed turned off: the category {} does not exist",
                    item.name,
                    missing.join(", ")
                ));
            }
            if let (Op::Replace, Some(obj)) = (op, &p.existing) {
                if let Some(o) = args.as_object_mut() {
                    o.insert("id".into(), json!(obj));
                }
                crate::rules::update_json(ctx, args)?;
                return Ok((obj.clone(), note));
            }
            if let Some(o) = args.as_object_mut() {
                o.insert("priority".into(), json!(*rule_base));
            }
            *rule_base = rule_base.saturating_add(1);
            let v = crate::rules::create_json(ctx, args)?;
            let id = v
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Ok((id, note))
        }
    }
}

/// Create a category path, adding missing parents; returns the leaf's id.
fn ensure_category(
    ctx: &Ctx<'_>,
    path: &str,
    kind: &str,
    description: &Value,
) -> CommandResult<AccountId> {
    let segs: Vec<&str> = path.split(':').collect();
    let mut parent: Option<String> = None;
    let mut last = None;
    for (i, seg) in segs.iter().enumerate() {
        let full = segs.get(..=i).map(|s| s.join(":")).unwrap_or_default();
        let chart = Chart::load(ctx)?;
        let existing = chart
            .accounts
            .iter()
            .find(|a| chart.full_name(a.id).eq_ignore_ascii_case(&full))
            .map(|a| (a.id, a.account_type));
        let id = match existing {
            Some((id, t)) => {
                if type_word(t) != Some(kind) {
                    return Err(CommandError::Rejected(format!(
                        "{full} exists as a different type of account"
                    )));
                }
                id
            }
            None => {
                let mut args = json!({"name": seg, "type": kind});
                if let Some(o) = args.as_object_mut() {
                    if let Some(p) = &parent {
                        o.insert("parent".into(), json!(p));
                    }
                    if i + 1 == segs.len() && !description.is_null() {
                        o.insert("description".into(), description.clone());
                    }
                }
                let v = crate::accounts::create_json(ctx, args)?;
                v.get("id")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| CommandError::Integrity("account without id".into()))?
            }
        };
        parent = Some(full);
        last = Some(id);
    }
    last.ok_or_else(|| invalid("empty category path"))
}

// ----- list / uninstall ---------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

fn list(ctx: &Ctx<'_>, _: NoArgs) -> CommandResult<Value> {
    let book = Book::load(ctx)?;
    let mut out = Vec::new();
    for p in load_packages(ctx)? {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut changed = Vec::new();
        let mut items = Vec::new();
        for it in &p.items {
            *counts.entry(it.kind.clone()).or_default() += 1;
            let e = edited(&book, it).unwrap_or(false);
            if e {
                changed.push(json!({"kind": it.kind, "key": it.key, "name": it.name}));
            }
            items.push(json!({
                "kind": it.kind, "key": it.key, "name": it.name, "object": it.object,
                "edited": e, "keep_reason": keep_reason(ctx, &book, it)?,
            }));
        }
        out.push(json!({
            "id": p.id, "version": p.version, "name": p.name, "sha256": p.sha256,
            "file_name": p.file_name, "installed_at": p.installed_at, "header": p.header,
            "counts": counts, "edited": changed, "items": items,
        }));
    }
    out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(Value::Array(out))
}

/// Why uninstalling would keep an item (`None`: it would be removed).
fn keep_reason(
    ctx: &Ctx<'_>,
    book: &Book,
    it: &PackageItem,
) -> CommandResult<Option<&'static str>> {
    let Some(kind) = kind_of(&it.kind) else {
        return Ok(None);
    };
    if book.current(kind, &it.object).is_none() {
        return Ok(Some("no longer in the book"));
    }
    if edited(book, it) == Some(true) {
        return Ok(Some("edited since install"));
    }
    Ok(match kind {
        ItemKind::Security => {
            let used = book
                .securities
                .iter()
                .find(|s| s.id == it.object)
                .map(|s| crate::investments::security_used(ctx, s))
                .transpose()?
                .unwrap_or(false);
            used.then_some("has holdings or transactions")
        }
        ItemKind::Category => match it.object.parse::<AccountId>() {
            Ok(id) if ctx.tx.account_ever_used(id)? => Some("has transactions"),
            _ => None,
        },
        _ => None,
    })
}

fn candidates(ctx: &Ctx<'_>, _: NoArgs) -> CommandResult<Value> {
    let book = Book::load(ctx)?;
    let mut bank: Vec<&String> = book.bank.values().map(|p| &p.name).collect();
    bank.sort();
    let mut inv: Vec<&String> = book.inv.values().map(|p| &p.name).collect();
    inv.sort();
    let mut rules: Vec<(i64, Value)> = book
        .rules
        .iter()
        .map(|r| {
            let why = rule_canonical(&book.chart, &book.tag_names, r).err();
            (
                r.priority,
                json!({"id": r.id.to_string(), "name": r.name, "ok": why.is_none(), "reason": why}),
            )
        })
        .collect();
    rules.sort_by_key(|(p, _)| *p);
    let mut cats: Vec<String> = book
        .chart
        .accounts
        .iter()
        .filter(|a| type_word(a.account_type).is_some())
        .map(|a| book.chart.full_name(a.id))
        .collect();
    cats.sort();
    let mut secs: Vec<&String> = book.securities.iter().map(|s| &s.symbol).collect();
    secs.sort();
    Ok(json!({
        "bank_profiles": bank,
        "investment_profiles": inv,
        "rules": rules.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        "categories": cats,
        "securities": secs,
        "dashboards": book.dashboards.iter().map(|d| d.1.clone()).collect::<Vec<_>>(),
    }))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IdArgs {
    /// Package id.
    id: String,
}

fn uninstall(ctx: &Ctx<'_>, a: IdArgs) -> CommandResult<Value> {
    let pkg = load_packages(ctx)?
        .into_iter()
        .find(|p| p.id == a.id)
        .ok_or_else(|| CommandError::NotFound(format!("definition package {}", a.id)))?;
    let book = Book::load(ctx)?;
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    let mut keep = |it: &PackageItem, why: &str| {
        kept.push(json!({"kind": it.kind, "key": it.key, "name": it.name, "reason": why}));
    };
    // Rules, profiles and securities first, then categories deepest first.
    let mut order: Vec<&PackageItem> = pkg.items.iter().collect();
    order.sort_by_key(|it| {
        let k = kind_of(&it.kind).unwrap_or(ItemKind::Rule);
        (
            std::cmp::Reverse(k),
            std::cmp::Reverse(it.name.matches(':').count()),
        )
    });
    for it in order {
        let Some(kind) = kind_of(&it.kind) else {
            continue;
        };
        if book.current(kind, &it.object).is_none() {
            continue;
        }
        if edited(&book, it) == Some(true) {
            keep(it, "edited since install");
            continue;
        }
        match kind {
            ItemKind::OfxInstitution => {
                crate::ofx_conn::delete_institution(ctx, &it.object)?;
            }
            ItemKind::Dashboard => {
                crate::dashboards::remove(ctx, &it.object)?;
            }
            ItemKind::DocumentTemplate => {
                ctx.save_doc(DocTable::Template, &it.object, None)?;
            }
            ItemKind::Rule => {
                ctx.save_doc(DocTable::Rule, &it.object, None)?;
            }
            ItemKind::BankProfile => {
                crate::profiles::delete_json(ctx, json!({"name": it.object}))?;
            }
            ItemKind::InvestmentProfile => {
                crate::inv_import::profile_delete_json(ctx, json!({"name": it.object}))?;
            }
            ItemKind::Security => {
                let used = book
                    .securities
                    .iter()
                    .find(|s| s.id == it.object)
                    .map(|s| crate::investments::security_used(ctx, s))
                    .transpose()?
                    .unwrap_or(false);
                if used {
                    keep(it, "has holdings or transactions");
                    continue;
                }
                crate::investments::security_delete_json(ctx, json!({"security": it.object}))?;
            }
            ItemKind::Category => {
                let Ok(id) = it.object.parse::<AccountId>() else {
                    continue;
                };
                let chart = Chart::load(ctx)?;
                let Some(acc) = chart.get(id).cloned() else {
                    continue;
                };
                if ctx.tx.account_ever_used(id)? {
                    keep(it, "has transactions");
                    continue;
                }
                if chart.children(id).next().is_some() {
                    keep(it, "has sub-accounts");
                    continue;
                }
                ctx.remove_account(&acc)?;
            }
        }
        removed.push(json!({"kind": it.kind, "key": it.key, "name": it.name}));
    }
    ctx.save_doc(DocTable::Definition, &pkg.id, None)?;
    Ok(json!({"id": pkg.id, "name": pkg.name, "removed": removed, "kept": kept}))
}

// ----- export -------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExportArgs {
    id: String,
    version: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    /// Bank (CSV) profile names.
    #[serde(default)]
    bank_profiles: Vec<String>,
    /// Investment profile names (built-in ones included).
    #[serde(default)]
    investment_profiles: Vec<String>,
    /// Rule ids or names.
    #[serde(default)]
    rules: Vec<String>,
    /// Category paths; each includes its whole subtree.
    #[serde(default)]
    categories: Vec<String>,
    /// Security symbols.
    #[serde(default)]
    securities: Vec<String>,
    /// Document template names (without their logos).
    #[serde(default)]
    document_templates: Vec<String>,
    /// Dashboard names (account and dashboard references written as names).
    #[serde(default)]
    dashboards: Vec<String>,
}

fn export(ctx: &Ctx<'_>, a: ExportArgs) -> CommandResult<Value> {
    let book = Book::load(ctx)?;
    let installed = load_packages(ctx)?.into_iter().find(|p| p.id == a.id);
    let mut items: Vec<DefItem> = Vec::new();
    let mut used: BTreeMap<ItemKind, BTreeSet<String>> = BTreeMap::new();
    // Reuse the package's keys for objects it installed, so re-exports match.
    let key_for = |kind: ItemKind,
                   object: &str,
                   name: &str,
                   used: &mut BTreeMap<ItemKind, BTreeSet<String>>| {
        let taken = used.entry(kind).or_default();
        let from_pkg = installed.as_ref().and_then(|p| {
            p.items
                .iter()
                .find(|x| x.kind == kind.table() && x.object.eq_ignore_ascii_case(object))
                .map(|x| x.key.clone())
        });
        let base = from_pkg.unwrap_or_else(|| key_from_name(name));
        let mut key = base.clone();
        let mut n = 2;
        while taken.contains(&key) {
            key = format!("{}-{n}", base.chars().take(60).collect::<String>());
            n += 1;
        }
        taken.insert(key.clone());
        key
    };
    for name in &a.bank_profiles {
        let p = book
            .bank
            .get(&name.trim().to_lowercase())
            .ok_or_else(|| CommandError::NotFound(format!("bank profile {name:?}")))?;
        let key = key_for(ItemKind::BankProfile, &p.name, &p.name, &mut used);
        items.push(DefItem {
            kind: ItemKind::BankProfile,
            key,
            name: p.name.clone(),
            fields: canonical_bank_profile(p),
            priority: None,
        });
    }
    for name in &a.investment_profiles {
        let p = book
            .inv
            .get(&name.trim().to_lowercase())
            .ok_or_else(|| CommandError::NotFound(format!("investment profile {name:?}")))?;
        let key = key_for(ItemKind::InvestmentProfile, &p.name, &p.name, &mut used);
        items.push(DefItem {
            kind: ItemKind::InvestmentProfile,
            key,
            name: p.name.clone(),
            fields: canonical_inv_profile(p),
            priority: None,
        });
    }
    let mut cats: BTreeSet<AccountId> = BTreeSet::new();
    for path in &a.categories {
        let root = book
            .category(path.trim())
            .ok_or_else(|| CommandError::NotFound(format!("category {path:?}")))?;
        if type_word(root.account_type).is_none() {
            return Err(invalid(format!(
                "{} is not an income or expense category",
                book.chart.full_name(root.id)
            )));
        }
        let mut stack = vec![root.id];
        while let Some(id) = stack.pop() {
            if cats.insert(id) {
                stack.extend(book.chart.children(id).map(|c| c.id));
            }
        }
    }
    for id in cats {
        let Some(c) = book.category_canonical(id) else {
            continue;
        };
        let path = book.chart.full_name(id);
        let key = key_for(ItemKind::Category, &id.to_string(), &path, &mut used);
        items.push(DefItem {
            kind: ItemKind::Category,
            key,
            name: path,
            fields: c,
            priority: None,
        });
    }
    let mut refused = Vec::new();
    let mut chosen: Vec<&mk_import::Rule> = Vec::new();
    for r in &a.rules {
        let matches: Vec<&mk_import::Rule> = book
            .rules
            .iter()
            .filter(|x| x.id.to_string() == *r || x.name.eq_ignore_ascii_case(r.trim()))
            .collect();
        if matches.is_empty() {
            return Err(CommandError::NotFound(format!("rule {r:?}")));
        }
        chosen.extend(matches);
    }
    chosen.sort_by_key(|r| (r.priority, r.id.to_string()));
    chosen.dedup_by_key(|r| r.id);
    for (i, r) in chosen.iter().enumerate() {
        match rule_canonical(&book.chart, &book.tag_names, r) {
            Ok(c) => {
                let key = key_for(ItemKind::Rule, &r.id.to_string(), &r.name, &mut used);
                items.push(DefItem {
                    kind: ItemKind::Rule,
                    key,
                    name: r.name.clone(),
                    fields: c,
                    priority: Some(i64::try_from(i).unwrap_or(i64::MAX).saturating_mul(10)),
                });
            }
            Err(why) => refused.push(format!("{}: {why}", r.name)),
        }
    }
    if !refused.is_empty() {
        return Err(CommandError::Rejected(format!(
            "these rules point at accounts that are not categories and cannot be shared: {}",
            refused.join("; ")
        )));
    }
    for sym in &a.securities {
        let s = book
            .securities
            .iter()
            .find(|s| s.symbol.eq_ignore_ascii_case(sym.trim()))
            .ok_or_else(|| CommandError::NotFound(format!("security {sym:?}")))?;
        let key = key_for(ItemKind::Security, &s.id, &s.symbol, &mut used);
        items.push(DefItem {
            kind: ItemKind::Security,
            key,
            name: s.symbol.clone(),
            fields: Book::security_canonical(s),
            priority: None,
        });
    }
    for name in &a.document_templates {
        let t = book
            .templates
            .iter()
            .find(|t| t.id == *name || t.name.eq_ignore_ascii_case(name.trim()))
            .ok_or_else(|| CommandError::NotFound(format!("template {name:?}")))?;
        let key = key_for(ItemKind::DocumentTemplate, &t.id, &t.name, &mut used);
        items.push(DefItem {
            kind: ItemKind::DocumentTemplate,
            key,
            name: t.name.clone(),
            fields: template_canonical(t),
            priority: None,
        });
    }
    for name in &a.dashboards {
        let d = book
            .dashboards
            .iter()
            .find(|d| d.0 == *name || d.1.eq_ignore_ascii_case(name.trim()))
            .ok_or_else(|| CommandError::NotFound(format!("dashboard {name:?}")))?;
        let key = key_for(ItemKind::Dashboard, &d.0, &d.1, &mut used);
        items.push(DefItem {
            kind: ItemKind::Dashboard,
            key,
            name: d.1.clone(),
            fields: dashboard_canonical(&d.2),
            priority: None,
        });
    }
    let header = Header {
        format: mk_import::definition::FORMAT_VERSION,
        id: a.id.trim().to_owned(),
        version: a.version.trim().to_owned(),
        name: a.name.trim().to_owned(),
        description: a.description.filter(|d| !d.trim().is_empty()),
        authors: a.authors,
        license: a.license.filter(|l| !l.trim().is_empty()),
        homepage: a.homepage.filter(|h| !h.trim().is_empty()),
        created: Some(mk_store::today_utc()?.to_string()),
        requires: Vec::new(),
        tags: a.tags,
    };
    let extensions = installed.map(|p| p.extensions).unwrap_or_default();
    let content = write_definition(&header, &items, &extensions);
    // The written file must read back cleanly.
    let check = parse_definition(content.as_bytes()).map_err(|e| invalid(e.join("\n")))?;
    Ok(json!({
        "file_name": format!(
            "{}-{}.{}",
            header.id,
            header.version,
            mk_core::brand::DEFINITION_EXTENSION
        ),
        "content": content,
        "warnings": check.warnings,
        "items": check.items.len(),
    }))
}
