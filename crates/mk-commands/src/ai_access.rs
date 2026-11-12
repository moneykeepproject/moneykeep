//! Per-book AI access (mcp-server.md revision 3): the settings, the permission
//! groups and the one function that computes which commands an AI assistant may use.
//!
//! The settings live in the book's settings table under module `core`, keys
//! `ai_access.*`. Registry commands cannot change them (`setting.set`,
//! `module.set_setting` and definition files are refused); only the session
//! operation [`set`] (and [`crate::Session::set_ai_access`]) writes them, recording
//! the session audit record `book.ai_access_changed`.
//!
//! [`exposed`] is shared by `keep-mcp` and the app's control channel, so the rules
//! cannot drift between the two routes.

use std::collections::BTreeSet;

use mk_store::{AuditRecord, Store, StoreError, WriteTx};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{CommandError, CommandResult, CommandSpec, NOT_OVER_MCP, Registry, modules};

/// The settings module that holds the keys.
pub const MODULE: &str = modules::CORE;
/// Prefix of every AI access key.
pub const KEY_PREFIX: &str = "ai_access.";
const KEY_ENABLED: &str = "ai_access.enabled";
const KEY_LEVEL: &str = "ai_access.level";
const KEY_GROUPS: &str = "ai_access.groups";
const KEY_APP_ONLY: &str = "ai_access.app_only";

/// The session audit record written by [`set`].
pub const CHANGED_RECORD: &str = "book.ai_access_changed";
/// The session audit record written when the owner allows an assistant in the app.
pub const SESSION_ALLOWED_RECORD: &str = "ai.session_allowed";

/// Where the owner changes AI access (used in refusal messages).
pub const WHERE: &str = "MoneyKeep → Settings → Security → AI assistant access";
/// The refusal for registry commands that try to change an `ai_access.*` key.
pub const REFUSED_KEY: &str = "AI access is changed in Settings → Security";

/// Messages when no route reaches the book (mcp-server.md revision 3).
pub const MSG_PASSWORD: &str = "This book is password protected. Open it in MoneyKeep, turn on AI assistant access, and allow this assistant when asked.";
pub const MSG_APP_ONLY: &str = "This book allows AI access only through MoneyKeep. Open it in MoneyKeep and allow this assistant when asked.";
pub const MSG_OFF: &str =
    "AI access is turned off for this book (MoneyKeep → Settings → Security).";

/// Which commands are available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    #[default]
    ReadOnly,
    ReadWrite,
}

impl Level {
    /// Stored form: `read_only` / `read_write`.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::ReadOnly => "read_only",
            Level::ReadWrite => "read_write",
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        match s {
            "read_only" | "read-only" => Some(Level::ReadOnly),
            "read_write" | "read-write" => Some(Level::ReadWrite),
            _ => None,
        }
    }

    /// The owner-facing name ("Read only", "Read and write").
    pub fn label(self) -> &'static str {
        match self {
            Level::ReadOnly => "Read only",
            Level::ReadWrite => "Read and write",
        }
    }
}

/// A permission group for risky commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Delete,
    Settings,
    Files,
    Connections,
}

impl Group {
    pub const ALL: [Group; 4] = [
        Group::Delete,
        Group::Settings,
        Group::Files,
        Group::Connections,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Group::Delete => "delete",
            Group::Settings => "settings",
            Group::Files => "files",
            Group::Connections => "connections",
        }
    }

    pub fn parse(s: &str) -> Option<Group> {
        Group::ALL.into_iter().find(|g| g.as_str() == s)
    }

    /// The label in the app.
    pub fn label(self) -> &'static str {
        match self {
            Group::Delete => "Delete and void",
            Group::Settings => "Change features and book settings",
            Group::Files => "Read files on this computer",
            Group::Connections => "Use bank feeds and connections",
        }
    }
}

/// The AI access settings of one book.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiAccess {
    pub enabled: bool,
    pub level: Level,
    pub groups: BTreeSet<Group>,
    pub app_only: bool,
}

impl Default for AiAccess {
    /// Off, read only, no groups, only through the app.
    fn default() -> Self {
        Self {
            enabled: false,
            level: Level::ReadOnly,
            groups: BTreeSet::new(),
            app_only: true,
        }
    }
}

impl AiAccess {
    /// The settings as JSON (`{"enabled","level","groups","app_only"}`).
    pub fn to_json(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "level": self.level.as_str(),
            "groups": self.groups.iter().map(|g| g.as_str()).collect::<Vec<_>>(),
            "app_only": self.app_only,
        })
    }

    /// Parse the JSON form; missing or invalid fields take their defaults.
    pub fn from_json(v: &Value) -> AiAccess {
        let d = AiAccess::default();
        AiAccess {
            enabled: v
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(d.enabled),
            level: v
                .get("level")
                .and_then(Value::as_str)
                .and_then(Level::parse)
                .unwrap_or(d.level),
            groups: parse_groups(v.get("groups")).unwrap_or_default(),
            app_only: v
                .get("app_only")
                .and_then(Value::as_bool)
                .unwrap_or(d.app_only),
        }
    }

    /// Group names, sorted.
    pub fn group_names(&self) -> Vec<String> {
        self.groups.iter().map(|g| g.as_str().to_owned()).collect()
    }

    /// One sentence for an assistant's instructions, e.g. "Access: read and write;
    /// delete and void allowed."
    pub fn sentence(&self) -> String {
        if !self.enabled {
            return MSG_OFF.to_owned();
        }
        let level = lower_first(self.level.label());
        if self.groups.is_empty() {
            return format!("Access: {level}.");
        }
        let groups: Vec<String> = self.groups.iter().map(|g| lower_first(g.label())).collect();
        format!("Access: {level}; {} allowed.", groups.join(", "))
    }

    /// The owner-facing access text ("Read and write; also: Delete and void").
    pub fn access_text(&self) -> String {
        let mut s = self.level.label().to_owned();
        if !self.groups.is_empty() {
            let labels: Vec<&str> = self.groups.iter().map(|g| g.label()).collect();
            s.push_str("; also: ");
            s.push_str(&labels.join(", "));
        }
        s
    }
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Group names from JSON; `None` when the value is not an array of known names.
fn parse_groups(v: Option<&Value>) -> Option<BTreeSet<Group>> {
    v?.as_array()?
        .iter()
        .map(|g| g.as_str().and_then(Group::parse))
        .collect()
}

/// Whether a settings key is one of the AI access keys.
pub fn is_ai_key(key: &str) -> bool {
    key.starts_with(KEY_PREFIX)
}

fn read_with(get: impl Fn(&str) -> Result<Option<Value>, StoreError>) -> CommandResult<AiAccess> {
    let d = AiAccess::default();
    Ok(AiAccess {
        enabled: get(KEY_ENABLED)?
            .and_then(|v| v.as_bool())
            .unwrap_or(d.enabled),
        level: get(KEY_LEVEL)?
            .and_then(|v| v.as_str().and_then(Level::parse))
            .unwrap_or(d.level),
        groups: parse_groups(get(KEY_GROUPS)?.as_ref()).unwrap_or_default(),
        app_only: get(KEY_APP_ONLY)?
            .and_then(|v| v.as_bool())
            .unwrap_or(d.app_only),
    })
}

/// The book's settings (defaults for missing or invalid values: access off).
pub fn read(store: &Store) -> CommandResult<AiAccess> {
    read_with(|k| store.get_setting(MODULE, k))
}

/// The book's settings inside a transaction.
pub fn read_tx(tx: &WriteTx<'_>) -> CommandResult<AiAccess> {
    read_with(|k| tx.get_setting(MODULE, k))
}

/// Change the settings (a session operation, never a registry command). Writes the
/// session audit record `book.ai_access_changed` with the old and new settings and
/// returns the new settings. A password-protected book cannot be used directly.
pub fn set(store: &mut Store, actor: &str, new: &AiAccess) -> CommandResult<AiAccess> {
    if store.is_encrypted() && !new.app_only {
        return Err(CommandError::InvalidArgs(
            "Password-protected books are only used through MoneyKeep.".into(),
        ));
    }
    let new = new.clone();
    store.write(|tx| -> CommandResult<AiAccess> {
        let old = read_tx(tx)?;
        tx.set_setting(MODULE, KEY_ENABLED, &json!(new.enabled), actor)?;
        tx.set_setting(MODULE, KEY_LEVEL, &json!(new.level.as_str()), actor)?;
        tx.set_setting(MODULE, KEY_GROUPS, &json!(new.group_names()), actor)?;
        tx.set_setting(MODULE, KEY_APP_ONLY, &json!(new.app_only), actor)?;
        tx.append_audit_record(&AuditRecord {
            actor,
            command: CHANGED_RECORD,
            args: json!({ "old": old.to_json(), "new": new.to_json() }),
            result: json!({}),
            before: None,
            after: None,
            undoes: None,
            redoes: None,
        })?;
        Ok(new)
    })
}

/// Record that the owner allowed an assistant's session in the app.
pub fn record_session_allowed(
    store: &mut Store,
    actor: &str,
    client: &str,
    access: &AiAccess,
) -> CommandResult<()> {
    store.write(|tx| -> CommandResult<()> {
        tx.append_audit_record(&AuditRecord {
            actor,
            command: SESSION_ALLOWED_RECORD,
            args: json!({
                "client": client,
                "level": access.level.as_str(),
                "groups": access.group_names(),
            }),
            result: json!({}),
            before: None,
            after: None,
            undoes: None,
            redoes: None,
        })?;
        Ok(())
    })
}

/// Whether the arguments of `spec` have a top-level property named `path`.
fn takes_path(spec: &CommandSpec) -> bool {
    fn has(schema: &Value) -> bool {
        if schema
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|p| p.contains_key("path"))
        {
            return true;
        }
        ["oneOf", "anyOf", "allOf"].iter().any(|k| {
            schema
                .get(*k)
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().any(has))
        })
    }
    has(&spec.args_schema())
}

/// The permission groups a command belongs to (by rule, so future commands are
/// covered).
pub fn groups_of(spec: &CommandSpec) -> Vec<Group> {
    let n = spec.name;
    let mut out = Vec::new();
    if (spec.mutates && (n.ends_with(".delete") || n.ends_with(".void")))
        || n == "budget.remove_line"
        || n == "definition.uninstall"
        // opening-balances.md: a statement start removes the transactions before it.
        || n == "opening.apply"
        || n == "opening.inv_apply"
    {
        out.push(Group::Delete);
    }
    if (spec.mutates && (n.starts_with("module.") || n.starts_with("book.")))
        || n == "setting.set"
        || n == "processor.set"
    {
        out.push(Group::Settings);
    }
    if takes_path(spec) {
        out.push(Group::Files);
    }
    if crate::connect::is_network(n)
        || (spec.mutates && (n.starts_with("connect.") || n.starts_with("capture.")))
        || n == "capture.revoke_device"
        || n == "capture.publish_lists"
    {
        out.push(Group::Connections);
    }
    out
}

/// Why `spec` is outside the exposed set under `access`, or `None` if it would be
/// exposed by the level and groups (it may still be hidden by a disabled module).
fn reason(spec: &CommandSpec, access: &AiAccess) -> Option<String> {
    if !access.enabled {
        return Some(MSG_OFF.to_owned());
    }
    if NOT_OVER_MCP.contains(&spec.name) {
        return Some("it is never offered to AI assistants".to_owned());
    }
    if access.level == Level::ReadOnly && spec.mutates {
        return Some(format!("this book allows read only ({WHERE})"));
    }
    let missing: Vec<&str> = groups_of(spec)
        .into_iter()
        .filter(|g| !access.groups.contains(g))
        .map(Group::label)
        .collect();
    if missing.is_empty() {
        return None;
    }
    let level = lower_first(access.level.label());
    let quoted: Vec<String> = missing.iter().map(|l| format!("'{l}'")).collect();
    Some(format!(
        "this book allows {level} but not {} ({WHERE})",
        quoted.join(" or ")
    ))
}

/// Whether `spec` passes steps 2–5 of the exposed-set rules.
pub fn allowed(spec: &CommandSpec, access: &AiAccess) -> bool {
    reason(spec, access).is_none()
}

/// The refusal message for a tool or command `shown` that is not in the exposed set;
/// `spec` is the registry's command of that name, if there is one. Never contains a
/// secret.
pub fn refusal(spec: Option<&CommandSpec>, access: &AiAccess, shown: &str) -> String {
    if !access.enabled {
        return MSG_OFF.to_owned();
    }
    match spec.and_then(|s| reason(s, access)) {
        Some(why) if spec.is_some_and(|s| !NOT_OVER_MCP.contains(&s.name)) => {
            format!("{shown} is not available: {why}")
        }
        _ => format!("unknown tool {shown:?}: no such tool is available in this book"),
    }
}

/// The exposed set: the registry commands an AI assistant may use in this book under
/// `access`, sorted by command name. Empty when access is off.
pub fn exposed<'r>(
    registry: &'r Registry,
    store: &Store,
    access: &AiAccess,
) -> CommandResult<Vec<&'r CommandSpec>> {
    if !access.enabled {
        return Ok(Vec::new());
    }
    Ok(registry
        .available_specs(store)?
        .into_iter()
        .filter(|s| allowed(s, access))
        .collect())
}

/// Why a call through the exposed set did not run the command.
#[derive(Debug)]
pub enum ExposedError {
    /// Outside the exposed set (the message says why and where to change it).
    NotAvailable(String),
    /// The command ran and failed.
    Failed(CommandError),
}

/// Run `command` as `actor` only if it is in the exposed set for the book's current
/// settings (the app's control channel uses this).
pub fn invoke_exposed(
    registry: &Registry,
    store: &mut Store,
    actor: &crate::Actor,
    command: &str,
    args: Value,
    files: Option<&dyn crate::FileReader>,
) -> Result<Value, ExposedError> {
    let access = read(store).map_err(ExposedError::Failed)?;
    let ok = exposed(registry, store, &access)
        .map_err(ExposedError::Failed)?
        .iter()
        .any(|s| s.name == command);
    if !ok {
        return Err(ExposedError::NotAvailable(refusal(
            registry.spec(command),
            &access,
            command,
        )));
    }
    registry
        .invoke_with(store, actor, command, args, files)
        .map_err(ExposedError::Failed)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::print_stdout)]
    use super::*;

    fn members(g: Group) -> Vec<&'static str> {
        Registry::with_builtins()
            .specs()
            .into_iter()
            .filter(|s| groups_of(s).contains(&g))
            .map(|s| s.name)
            .collect()
    }

    /// mcp-server.md revision 3: the members of each permission group for the current
    /// registry, so a reviewer sees any change. Update the lists deliberately.
    #[test]
    fn at18_group_membership_for_the_current_registry() {
        for g in Group::ALL {
            println!("{} ({}): {:?}", g.as_str(), g.label(), members(g));
        }
        // Commands offered at all (NOT_OVER_MCP removed): receipt.add also takes a
        // path but is never offered.
        let offered = |g: Group| -> Vec<&'static str> {
            members(g)
                .into_iter()
                .filter(|c| !NOT_OVER_MCP.contains(c))
                .collect()
        };
        assert_eq!(
            offered(Group::Files),
            [
                "definition.inspect",
                "definition.install",
                "definition.validate",
                "import.stage"
            ]
        );
        assert_eq!(
            members(Group::Files).join(" "),
            "definition.inspect definition.install definition.validate import.stage receipt.add"
        );
        let delete = members(Group::Delete);
        for c in [
            "txn.delete",
            "txn.void",
            "account.delete",
            "budget.remove_line",
            "definition.uninstall",
        ] {
            assert!(delete.contains(&c), "{c}: {delete:?}");
        }
        assert!(
            delete.iter().all(|c| c.ends_with(".delete")
                || c.ends_with(".void")
                || *c == "budget.remove_line"
                || *c == "definition.uninstall"
                || c.starts_with("opening.")),
            "{delete:?}"
        );
        let settings = members(Group::Settings);
        for c in [
            "module.enable",
            "module.disable",
            "module.set_setting",
            "setting.set",
            "processor.set",
            "book.update_profile",
        ] {
            assert!(settings.contains(&c), "{c}: {settings:?}");
        }
        assert!(!settings.contains(&"module.list"));
        let conn = members(Group::Connections);
        for c in [
            "connect.sync",
            "capture.revoke_device",
            "capture.publish_lists",
        ] {
            assert!(conn.contains(&c), "{c}: {conn:?}");
        }
        assert!(!conn.contains(&"connect.list"));
        // history.undo / history.redo are ordinary write commands.
        let r = Registry::with_builtins();
        for c in ["history.undo", "history.redo", "txn.create"] {
            assert!(groups_of(r.spec(c).unwrap()).is_empty(), "{c}");
        }
        // The full lists, as a snapshot a reviewer reads in the diff.
        assert_eq!(
            delete.join(" "),
            include_str!("tests/fixtures/ai_groups/delete.txt").trim()
        );
        assert_eq!(
            settings.join(" "),
            include_str!("tests/fixtures/ai_groups/settings.txt").trim()
        );
        assert_eq!(
            conn.join(" "),
            include_str!("tests/fixtures/ai_groups/connections.txt").trim()
        );
    }

    #[test]
    fn defaults_and_invalid_values_mean_off() {
        let d = AiAccess::default();
        assert!(!d.enabled && d.app_only && d.groups.is_empty());
        assert_eq!(d.level, Level::ReadOnly);
        let parsed = AiAccess::from_json(&json!({"enabled": "yes", "groups": ["x"]}));
        assert_eq!(parsed, d);
    }

    #[test]
    fn sentences() {
        let mut a = AiAccess {
            enabled: true,
            level: Level::ReadWrite,
            ..AiAccess::default()
        };
        a.groups.insert(Group::Delete);
        assert_eq!(
            a.sentence(),
            "Access: read and write; delete and void allowed."
        );
        assert_eq!(a.access_text(), "Read and write; also: Delete and void");
        a.groups.clear();
        a.level = Level::ReadOnly;
        assert_eq!(a.sentence(), "Access: read only.");
    }
}
