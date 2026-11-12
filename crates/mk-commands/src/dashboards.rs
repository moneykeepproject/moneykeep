//! Dashboards (dashboards.md): named screens of blocks on a 12-column grid, stored in
//! the book's own settings (so a version without dashboards opens the book and
//! ignores them), audited and undone like every setting. The built-in Home dashboard
//! exists in every book: until it is first changed it is the standard layout.
//!
//! Block data is read through the existing commands (`account.register`,
//! `schedule.upcoming`, `report.*`, `budget.report`, …), limited to what a block
//! shows.

use std::collections::BTreeSet;

use mk_core::dashboard::{self as grid, Cell, LayoutError};
use mk_core::{Date, Decimal};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::book_profile::{MODULE, SYMBOLS};
use crate::builtin::NoArgs;
use crate::{CommandError, CommandResult, Ctx, Registry, call, is_module_enabled};

/// Setting key prefix of a stored dashboard (`dashboard.<id>`).
const PREFIX: &str = "dashboard.";
/// The book setting naming the startup dashboard (absent: Home).
pub(crate) const STARTUP_KEY: &str = "dashboards.startup";
/// The built-in Home dashboard's id.
pub(crate) const HOME: &str = "home";
const MAX_NAME: usize = 60;
const MAX_TITLE: usize = 60;
const MAX_NOTE: usize = 2000;
const MAX_LABEL: usize = 30;
const MAX_BUTTONS: usize = 12;

/// The sidebar groups an Account balances block can show (app-desktop revision 23).
pub(crate) const GROUPS: [&str; 7] = [
    "Cash & bank",
    "Credit cards",
    "Investments",
    "Other assets",
    "Loans & other debts",
    "Equity",
    "Categories",
];

/// Chart kinds: (name, default title, module needed).
const CHART_KINDS: [(&str, &str, Option<&str>); 6] = [
    ("spending", "Spending by category", None),
    ("income_expense", "Income vs expenses", None),
    ("net_worth", "Net worth over time", None),
    ("account_balance", "Account balance over time", None),
    ("budget", "Budget vs actual", Some("budgets")),
    ("portfolio", "Portfolio value", Some("investments")),
];

/// Report summaries: (name, default title, module needed).
const SUMMARIES: [(&str, &str, Option<&str>); 6] = [
    ("profit_loss", "Net income", None),
    ("cash_flow", "Cash flow net change", None),
    ("ar_total", "Owed by customers", Some("invoicing")),
    ("ap_total", "Owed to vendors", Some("bills")),
    ("sales_tax_due", "Sales tax due", Some("sales-tax")),
    ("1099_total", "1099 total", Some("1099")),
];

const PERIODS: [&str; 6] = [
    "this_month",
    "last_month",
    "this_quarter",
    "this_year",
    "last_12_months",
    "custom",
];

/// Screens a shortcut can open.
pub(crate) const SCREENS: [&str; 10] = [
    "home",
    "scheduled",
    "reports",
    "accounts",
    "expenses",
    "sales",
    "purchases",
    "budget",
    "securities",
    "settings",
];

/// Actions a shortcut can start (each opens its dialog; nothing posts by itself).
pub(crate) const ACTIONS: [&str; 6] = [
    "new_transaction",
    "import_file",
    "reconcile",
    "calculator",
    "new_expense",
    "collect_from_phone",
];

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "dashboard.list",
        "List the dashboards, in order, with the startup one marked",
        false,
        list,
    );
    r.add(
        "dashboard.get",
        "Get one dashboard with its blocks and their layout",
        false,
        get,
    );
    r.add(
        "dashboard.create",
        "Create a dashboard (empty, or a copy of another with `from`)",
        true,
        create,
    );
    r.add(
        "dashboard.update",
        "Rename a dashboard or change its icon",
        true,
        update,
    );
    r.add(
        "dashboard.delete",
        "Delete a dashboard (not Home); deleting the startup dashboard makes Home the startup",
        true,
        delete,
    );
    r.add(
        "dashboard.reorder",
        "Put the dashboards in the given order (every dashboard id, once)",
        true,
        reorder,
    );
    r.add(
        "dashboard.set_startup",
        "Open this dashboard when the book opens",
        true,
        set_startup,
    );
    r.add(
        "dashboard.reset_home",
        "Put Home's blocks back to the standard layout (net worth, account balances, attention, coming up)",
        true,
        reset_home,
    );
    r.add(
        "dashboard.add_block",
        "Add a block to a dashboard (account transactions, scheduled, balances, net worth, chart, report summary, shortcuts, attention, bills due, note)",
        true,
        add_block,
    );
    r.add(
        "dashboard.update_block",
        "Change a block's title or settings",
        true,
        update_block,
    );
    r.add(
        "dashboard.move_blocks",
        "Save a dashboard's whole layout: every block's x, y, w, h in grid cells (no overlap, inside the 12-column grid, sizes within limits)",
        true,
        move_blocks,
    );
    r.add(
        "dashboard.remove_block",
        "Remove a block from a dashboard",
        true,
        remove_block,
    );
    r.add(
        "dashboard.block_data",
        "What one block shows, ready to display (read through the existing commands)",
        false,
        block_data,
    );
}

fn refused(message: impl Into<String>) -> CommandError {
    CommandError::Coded {
        code: "invalid_args",
        message: message.into(),
    }
}

/// Clean one line of user text (input-safety.md R3), trimmed.
fn clean(text: &str) -> String {
    mk_core::text::clean(text, usize::MAX)
        .text
        .trim()
        .to_owned()
}

/// Clean a note: each line cleaned, line breaks kept.
fn clean_note(text: &str) -> String {
    let lines: Vec<String> = text
        .replace("\r\n", "\n")
        .split('\n')
        .map(|l| {
            mk_core::text::clean(l, usize::MAX)
                .text
                .trim_end()
                .to_owned()
        })
        .collect();
    lines.join("\n").trim().to_owned()
}

// ----- stored form ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Block {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub settings: Value,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Fields of a newer version, kept unchanged.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Block {
    fn cell(&self) -> Cell {
        Cell::new(self.x, self.y, self.w, self.h)
    }

    fn set_cell(&mut self, c: Cell) {
        self.x = c.x;
        self.y = c.y;
        self.w = c.w;
        self.h = c.h;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Dashboard {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub order: i64,
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn block(id: &str, kind: &str, title: Option<&str>, settings: Value, c: Cell) -> Block {
    Block {
        id: id.to_owned(),
        kind: kind.to_owned(),
        title: title.map(str::to_owned),
        settings,
        x: c.x,
        y: c.y,
        w: c.w,
        h: c.h,
        extra: Map::new(),
    }
}

/// Home's standard blocks: the information the Home screen always showed.
fn standard_home_blocks() -> Vec<Block> {
    vec![
        block(
            "net-worth",
            "net_worth",
            None,
            json!({}),
            // Tall enough for its chart with axis and month labels.
            Cell::new(0, 0, 6, 3),
        ),
        block(
            "attention",
            "attention",
            None,
            json!({}),
            Cell::new(0, 3, 6, 2),
        ),
        block(
            "coming-up",
            "scheduled",
            Some("Coming up"),
            json!({"mode": "next", "n": 5, "accounts": []}),
            Cell::new(0, 5, 6, 3),
        ),
        block(
            "balances",
            "balances",
            None,
            json!({"source": "groups", "groups": []}),
            Cell::new(6, 0, 6, 8),
        ),
    ]
}

fn standard_home() -> Dashboard {
    Dashboard {
        id: HOME.to_owned(),
        name: "Home".to_owned(),
        icon: Some("house".to_owned()),
        order: 0,
        blocks: standard_home_blocks(),
        extra: Map::new(),
    }
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", mk_core::Id::<()>::new())
}

/// Every dashboard in list order (Home included, stored or standard).
pub(crate) fn load_all(ctx: &Ctx<'_>) -> CommandResult<Vec<Dashboard>> {
    let mut out: Vec<Dashboard> = Vec::new();
    for (module, key, value) in ctx.tx.all_settings()? {
        if module != MODULE || !key.starts_with(PREFIX) {
            continue;
        }
        let d: Dashboard = serde_json::from_value(value)
            .map_err(|e| CommandError::Integrity(format!("bad stored dashboard {key}: {e}")))?;
        out.push(d);
    }
    if !out.iter().any(|d| d.id == HOME) {
        out.push(standard_home());
    }
    out.sort_by(|a, b| {
        (a.order, a.name.to_lowercase(), &a.id).cmp(&(b.order, b.name.to_lowercase(), &b.id))
    });
    Ok(out)
}

/// A dashboard by id or name (case-insensitive).
pub(crate) fn find(ctx: &Ctx<'_>, reference: &str) -> CommandResult<Dashboard> {
    let r = reference.trim();
    let all = load_all(ctx)?;
    let pos = all
        .iter()
        .position(|d| d.id == r)
        .or_else(|| all.iter().position(|d| d.name.eq_ignore_ascii_case(r)));
    pos.and_then(|i| all.into_iter().nth(i))
        .ok_or_else(|| CommandError::NotFound(format!("dashboard {reference:?}")))
}

pub(crate) fn save(ctx: &Ctx<'_>, d: &Dashboard) -> CommandResult<()> {
    ctx.save_setting(
        MODULE,
        &format!("{PREFIX}{}", d.id),
        Some(&serde_json::to_value(d)?),
    )
}

fn startup_id(ctx: &Ctx<'_>) -> CommandResult<String> {
    let stored = ctx
        .tx
        .get_setting(MODULE, STARTUP_KEY)?
        .and_then(|v| v.as_str().map(str::to_owned));
    let all = load_all(ctx)?;
    Ok(stored
        .filter(|s| all.iter().any(|d| d.id == *s))
        .unwrap_or_else(|| HOME.to_owned()))
}

fn check_name(ctx: &Ctx<'_>, name: &str, except: Option<&str>) -> CommandResult<String> {
    let n = clean(name);
    let count = n.chars().count();
    if count == 0 {
        return Err(refused("Give the dashboard a name."));
    }
    if count > MAX_NAME {
        return Err(refused(format!(
            "A dashboard name can be at most {MAX_NAME} characters."
        )));
    }
    if load_all(ctx)?
        .iter()
        .any(|d| Some(d.id.as_str()) != except && d.name.eq_ignore_ascii_case(&n))
    {
        return Err(refused(format!("There is already a dashboard named {n}.")));
    }
    Ok(n)
}

/// Symbols added for dashboards and shortcut buttons (revision 3), after the
/// book-profile symbols.
pub const EXTRA_SYMBOLS: [&str; 13] = [
    "register",
    "report",
    "calendar",
    "receipt",
    "camera",
    "calculator",
    "import",
    "reconcile",
    "dashboard",
    "gear",
    "invoice",
    "bill",
    "plus",
];

/// Every symbol a dashboard or shortcut button can show.
pub fn icon_symbols() -> Vec<&'static str> {
    SYMBOLS
        .iter()
        .chain(EXTRA_SYMBOLS.iter())
        .copied()
        .collect()
}

/// Button sizes of a shortcuts block (medium is the default).
pub(crate) const BUTTON_SIZES: [&str; 3] = ["medium", "small", "large"];

pub(crate) fn check_icon(icon: Option<&str>) -> CommandResult<Option<String>> {
    let all = icon_symbols();
    match icon.map(str::trim) {
        None | Some("") => Ok(None),
        Some(i) if all.contains(&i) => Ok(Some(i.to_owned())),
        Some(i) => Err(refused(format!(
            "Unknown icon {i:?}; choose one of {}.",
            all.join(", ")
        ))),
    }
}

/// The icon a shortcut button shows when none was chosen, by its target.
pub(crate) fn default_icon(t: &Value) -> &'static str {
    match s(t, "kind") {
        "account" => "register",
        "report" => "report",
        "dashboard" => "dashboard",
        "screen" => match s(t, "screen") {
            "home" => "house",
            "scheduled" => "calendar",
            "reports" => "report",
            "accounts" => "wallet",
            "expenses" => "receipt",
            "sales" => "invoice",
            "purchases" => "bill",
            "budget" => "piggy-bank",
            "securities" => "chart",
            "settings" => "gear",
            _ => "dashboard",
        },
        "action" => match s(t, "action") {
            "new_transaction" => "plus",
            "import_file" => "import",
            "reconcile" => "reconcile",
            "calculator" => "calculator",
            "new_expense" => "receipt",
            "collect_from_phone" => "camera",
            _ => "plus",
        },
        _ => "star",
    }
}

// ----- settings ------------------------------------------------------------------------

/// Set `k` in a JSON object.
fn put(v: &mut Value, k: &str, x: Value) {
    if let Some(o) = v.as_object_mut() {
        o.insert(k.to_owned(), x);
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

fn int(v: &Value, k: &str, default: u64, lo: u64, hi: u64, what: &str) -> CommandResult<u64> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(default),
        Some(x) => match x.as_u64() {
            Some(n) if (lo..=hi).contains(&n) => Ok(n),
            _ => Err(refused(format!(
                "{what} must be a whole number from {lo} to {hi}."
            ))),
        },
    }
}

fn one_of<'a>(v: &Value, k: &str, choices: &[&'a str], what: &str) -> CommandResult<&'a str> {
    let given = s(v, k);
    if given.is_empty() {
        return choices
            .first()
            .copied()
            .ok_or_else(|| refused(format!("no {what}")));
    }
    choices
        .iter()
        .find(|c| **c == given)
        .copied()
        .ok_or_else(|| refused(format!("{what} must be one of {}.", choices.join(", "))))
}

/// An account reference resolved to its id. An empty reference stays empty (the block
/// asks to choose an account).
fn account_id(ctx: &Ctx<'_>, reference: &str) -> CommandResult<String> {
    if reference.trim().is_empty() {
        return Ok(String::new());
    }
    let a = call(ctx, "account.get", json!({"account": reference.trim()}))
        .map_err(|_| refused(format!("There is no account {reference:?}.")))?;
    Ok(s(&a, "id").to_owned())
}

fn account_ids(ctx: &Ctx<'_>, v: &Value, k: &str) -> CommandResult<Vec<String>> {
    let mut out = Vec::new();
    for x in v.get(k).and_then(Value::as_array).into_iter().flatten() {
        let r = x.as_str().unwrap_or_default();
        let id = account_id(ctx, r)?;
        if !id.is_empty() && !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

fn period(v: &Value, out: &mut Map<String, Value>) -> CommandResult<()> {
    let p = one_of(v, "period", &PERIODS, "The period")?;
    out.insert("period".into(), json!(p));
    if p == "custom" {
        let from: Date = s(v, "from")
            .parse()
            .map_err(|_| refused("A custom period needs a From date (YYYY-MM-DD)."))?;
        let to: Date = s(v, "to")
            .parse()
            .map_err(|_| refused("A custom period needs a To date (YYYY-MM-DD)."))?;
        if to < from {
            return Err(refused("The period ends before it starts."));
        }
        out.insert("from".into(), json!(from));
        out.insert("to".into(), json!(to));
    }
    Ok(())
}

/// A shortcut button's target, checked and with references resolved.
fn target(ctx: &Ctx<'_>, t: &Value) -> CommandResult<Value> {
    let kind = one_of(
        t,
        "kind",
        &["account", "report", "screen", "dashboard", "action"],
        "A shortcut target",
    )?;
    Ok(match kind {
        "account" => {
            let id = account_id(ctx, s(t, "account"))?;
            json!({"kind": "account", "account": id})
        }
        "report" => {
            let report = s(t, "report").trim();
            if report.is_empty() || report.len() > 40 {
                return Err(refused("A report shortcut needs a report."));
            }
            let mut o = json!({"kind": "report", "report": report});
            // The report's saved period: a preset, or From and To dates.
            let preset = s(t, "preset");
            if !preset.is_empty() {
                if preset.len() > 30 || !preset.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                {
                    return Err(refused(
                        "A report shortcut's preset is a name such as this_month.",
                    ));
                }
                put(&mut o, "preset", json!(preset));
            }
            for k in ["from", "to"] {
                if !s(t, k).is_empty() {
                    let d: Date = s(t, k).parse().map_err(|_| {
                        refused(format!("A report shortcut's {k} date is YYYY-MM-DD."))
                    })?;
                    put(&mut o, k, json!(d));
                }
            }
            o
        }
        "screen" => {
            let screen = one_of(t, "screen", &SCREENS, "The screen")?;
            let mut o = json!({"kind": "screen", "screen": screen});
            let section = clean(s(t, "section"));
            if screen == "settings" && !section.is_empty() {
                put(&mut o, "section", json!(section));
            }
            o
        }
        "dashboard" => {
            let r = s(t, "dashboard");
            let d = find(ctx, r).map_err(|_| refused(format!("There is no dashboard {r:?}.")))?;
            json!({"kind": "dashboard", "dashboard": d.id})
        }
        _ => {
            let action = one_of(t, "action", &ACTIONS, "The action")?;
            let mut o = json!({"kind": "action", "action": action});
            if matches!(action, "new_transaction" | "reconcile") {
                let id = account_id(ctx, s(t, "account"))?;
                put(&mut o, "account", json!(id));
            }
            o
        }
    })
}

/// Validate a block's settings for its type and return their stored form (account
/// references as ids). Unknown types keep their settings unchanged.
pub(crate) fn check_settings(ctx: &Ctx<'_>, kind: &str, v: &Value) -> CommandResult<Value> {
    let v = if v.is_null() { &json!({}) } else { v };
    if !v.is_object() {
        return Err(refused("Block settings must be an object."));
    }
    let mut o = Map::new();
    match kind {
        "account_transactions" => {
            o.insert("account".into(), json!(account_id(ctx, s(v, "account"))?));
            let mode = one_of(v, "mode", &["recent", "above"], "The mode")?;
            o.insert("mode".into(), json!(mode));
            o.insert(
                "n".into(),
                json!(int(v, "n", 5, 1, 50, "The number of transactions")?),
            );
            if mode == "above" {
                let amount: Decimal = match v.get("amount") {
                    Some(Value::String(x)) => x.trim().parse().ok(),
                    Some(Value::Number(x)) => x.to_string().parse().ok(),
                    _ => None,
                }
                .filter(|d: &Decimal| *d >= Decimal::ZERO)
                .ok_or_else(|| refused("Give the amount (zero or more)."))?;
                o.insert("amount".into(), json!(amount.to_string()));
                let dir = one_of(v, "direction", &["either", "in", "out"], "The direction")?;
                o.insert("direction".into(), json!(dir));
                o.insert(
                    "days".into(),
                    json!(int(v, "days", 30, 1, 3650, "The number of days")?),
                );
            }
            o.insert(
                "include_children".into(),
                json!(
                    v.get("include_children")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                ),
            );
            if let Some(m) = v.get("missing_account").and_then(Value::as_str)
                && s(v, "account").is_empty()
            {
                o.insert("missing_account".into(), json!(clean(m)));
            }
        }
        "scheduled" => {
            let mode = one_of(v, "mode", &["next", "days"], "The mode")?;
            o.insert("mode".into(), json!(mode));
            if mode == "next" {
                o.insert(
                    "n".into(),
                    json!(int(v, "n", 5, 1, 50, "The number of transactions")?),
                );
            } else {
                o.insert(
                    "days".into(),
                    json!(int(v, "days", 30, 1, 366, "The number of days")?),
                );
            }
            o.insert("accounts".into(), json!(account_ids(ctx, v, "accounts")?));
        }
        "balances" => {
            let source = one_of(v, "source", &["groups", "accounts"], "The source")?;
            o.insert("source".into(), json!(source));
            if source == "groups" {
                let mut groups = Vec::new();
                for g in v
                    .get("groups")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let g = g.as_str().unwrap_or_default();
                    let known = GROUPS
                        .iter()
                        .find(|x| x.eq_ignore_ascii_case(g))
                        .ok_or_else(|| {
                            refused(format!("Unknown group {g:?}; use {}.", GROUPS.join(", ")))
                        })?;
                    if !groups.contains(known) {
                        groups.push(*known);
                    }
                }
                o.insert("groups".into(), json!(groups));
            } else {
                let ids = account_ids(ctx, v, "accounts")?;
                if ids.is_empty() {
                    return Err(refused("Choose at least one account."));
                }
                o.insert("accounts".into(), json!(ids));
            }
        }
        "chart" => {
            let names: Vec<&str> = CHART_KINDS.iter().map(|k| k.0).collect();
            let k = one_of(v, "chart", &names, "The chart")?;
            o.insert("chart".into(), json!(k));
            period(v, &mut o)?;
            if k == "account_balance" {
                o.insert("account".into(), json!(account_id(ctx, s(v, "account"))?));
            }
        }
        "report_summary" => {
            let names: Vec<&str> = SUMMARIES.iter().map(|k| k.0).collect();
            let k = one_of(v, "report", &names, "The report")?;
            o.insert("report".into(), json!(k));
            period(v, &mut o)?;
        }
        "shortcuts" => {
            let buttons = v
                .get("buttons")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if buttons.is_empty() || buttons.len() > MAX_BUTTONS {
                return Err(refused(format!(
                    "A shortcuts block has 1 to {MAX_BUTTONS} buttons."
                )));
            }
            let mut out = Vec::new();
            for b in &buttons {
                let label = clean(s(b, "label"));
                if label.is_empty() || label.chars().count() > MAX_LABEL {
                    return Err(refused(format!(
                        "A button label has 1 to {MAX_LABEL} characters."
                    )));
                }
                let icon = check_icon(b.get("icon").and_then(Value::as_str))?;
                let t = target(ctx, b.get("target").unwrap_or(&Value::Null))?;
                let mut x = json!({"label": label, "target": t});
                if let Some(i) = icon {
                    put(&mut x, "icon", json!(i));
                }
                out.push(x);
            }
            o.insert("buttons".into(), json!(out));
            let size = one_of(v, "button_size", &BUTTON_SIZES, "The button size")?;
            o.insert("button_size".into(), json!(size));
        }
        "bills_due" => {
            o.insert(
                "days".into(),
                json!(int(v, "days", 14, 1, 366, "The number of days")?),
            );
        }
        "note" => {
            let text = clean_note(s(v, "text"));
            if text.chars().count() > MAX_NOTE {
                return Err(refused(format!(
                    "A note can be at most {MAX_NOTE} characters."
                )));
            }
            o.insert("text".into(), json!(text));
        }
        "net_worth" | "attention" | "subscriptions" | "projects" => {}
        _ => return Ok(v.clone()),
    }
    Ok(Value::Object(o))
}

fn check_title(title: Option<&str>) -> CommandResult<Option<String>> {
    match title.map(clean) {
        None => Ok(None),
        Some(t) if t.is_empty() => Ok(None),
        Some(t) if t.chars().count() > MAX_TITLE => Err(refused(format!(
            "A block title can be at most {MAX_TITLE} characters."
        ))),
        Some(t) => Ok(Some(t)),
    }
}

fn layout_error(d: &Dashboard, e: &LayoutError) -> CommandError {
    let name = |i: &usize| d.blocks.get(*i).map(title_of_stored).unwrap_or_default();
    refused(match e {
        LayoutError::TooMany(n) => format!(
            "A dashboard holds at most {} blocks ({n} given).",
            grid::MAX_BLOCKS
        ),
        LayoutError::OutOfGrid(i) => format!(
            "{} must stay inside the {}-column grid.",
            name(i),
            grid::COLUMNS
        ),
        LayoutError::TooSmall(i) => {
            let (w, h) = d
                .blocks
                .get(*i)
                .map_or((grid::MIN_W, grid::MIN_H), |b| grid::min_size(&b.kind));
            format!(
                "{} must be at least {w} columns wide and {h} rows high.",
                name(i)
            )
        }
        LayoutError::TooLarge(i) => format!(
            "{} can be at most {} columns wide and {} rows high.",
            name(i),
            grid::MAX_W,
            grid::MAX_H
        ),
        LayoutError::Overlap(i, j) => format!("{} and {} overlap.", name(i), name(j)),
    })
}

fn check_layout(d: &Dashboard) -> CommandResult<()> {
    let cells: Vec<(Cell, (u32, u32))> = d
        .blocks
        .iter()
        .map(|b| (b.cell(), grid::min_size(&b.kind)))
        .collect();
    grid::validate(&cells).map_err(|e| layout_error(d, &e))
}

// ----- titles --------------------------------------------------------------------------

fn kind_title(list: &[(&str, &'static str, Option<&str>)], name: &str) -> Option<&'static str> {
    list.iter().find(|k| k.0 == name).map(|k| k.1)
}

/// A block's title without reading the book (stored title, else from its type).
fn title_of_stored(b: &Block) -> String {
    if let Some(t) = &b.title {
        return t.clone();
    }
    match b.kind.as_str() {
        "chart" => kind_title(&CHART_KINDS, s(&b.settings, "chart"))
            .unwrap_or("Chart")
            .to_owned(),
        "report_summary" => kind_title(&SUMMARIES, s(&b.settings, "report"))
            .unwrap_or("Report summary")
            .to_owned(),
        "shortcuts" => "Shortcuts".to_owned(),
        k => grid::block_type(k).map_or_else(|| k.to_owned(), |t| t.title.to_owned()),
    }
}

/// A block's title: its own, else one made from its type and settings.
fn display_title(ctx: &Ctx<'_>, b: &Block) -> String {
    if b.title.is_some() {
        return title_of_stored(b);
    }
    let account = |id: &str| -> Option<Value> {
        (!id.is_empty())
            .then(|| call(ctx, "account.get", json!({"account": id})).ok())
            .flatten()
    };
    match b.kind.as_str() {
        "account_transactions" => account(s(&b.settings, "account"))
            .map(|a| s(&a, "name").to_owned())
            .unwrap_or_else(|| "Account transactions".to_owned()),
        "chart" if s(&b.settings, "chart") == "account_balance" => {
            account(s(&b.settings, "account"))
                .map(|a| format!("{} balance", s(&a, "name")))
                .unwrap_or_else(|| "Account balance over time".to_owned())
        }
        _ => title_of_stored(b),
    }
}

fn block_json(ctx: &Ctx<'_>, b: &Block) -> CommandResult<Value> {
    let mut v = serde_json::to_value(b)?;
    let known = grid::block_type(&b.kind).is_some();
    let (min_w, min_h) = grid::min_size(&b.kind);
    if let Some(o) = v.as_object_mut() {
        o.insert("display_title".into(), json!(display_title(ctx, b)));
        o.insert("known".into(), json!(known));
        o.insert("min_w".into(), json!(min_w));
        o.insert("min_h".into(), json!(min_h));
    }
    Ok(v)
}

fn dashboard_json(ctx: &Ctx<'_>, d: &Dashboard, startup: &str) -> CommandResult<Value> {
    let blocks = d
        .blocks
        .iter()
        .map(|b| block_json(ctx, b))
        .collect::<CommandResult<Vec<_>>>()?;
    Ok(json!({
        "id": d.id, "name": d.name, "icon": d.icon, "order": d.order,
        "home": d.id == HOME, "startup": d.id == startup,
        "columns": grid::COLUMNS, "blocks": blocks,
    }))
}

// ----- commands ------------------------------------------------------------------------

fn list(ctx: &Ctx<'_>, _: NoArgs) -> CommandResult<Value> {
    let startup = startup_id(ctx)?;
    let out: Vec<Value> = load_all(ctx)?
        .iter()
        .map(|d| {
            json!({"id": d.id, "name": d.name, "icon": d.icon, "order": d.order,
                "home": d.id == HOME, "startup": d.id == startup, "blocks": d.blocks.len()})
        })
        .collect();
    Ok(Value::Array(out))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DashArgs {
    /// Dashboard id or name.
    dashboard: String,
}

fn get(ctx: &Ctx<'_>, a: DashArgs) -> CommandResult<Value> {
    let d = find(ctx, &a.dashboard)?;
    dashboard_json(ctx, &d, &startup_id(ctx)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    /// 1–60 characters, unique in the book (case-insensitive).
    name: String,
    /// A symbol from the book-profile symbol list (house, church, briefcase, …).
    #[serde(default)]
    icon: Option<String>,
    /// Copy the blocks of this dashboard (id or name).
    #[serde(default)]
    from: Option<String>,
}

/// Create a dashboard and return it (used by `dashboard.create` and definition installs).
pub(crate) fn create_dashboard(
    ctx: &Ctx<'_>,
    id: Option<String>,
    name: &str,
    icon: Option<&str>,
    blocks: Vec<Block>,
) -> CommandResult<Dashboard> {
    let name = check_name(ctx, name, None)?;
    let icon = check_icon(icon)?;
    let order = load_all(ctx)?
        .iter()
        .map(|d| d.order)
        .max()
        .map_or(0, |m| m.saturating_add(1));
    let d = Dashboard {
        id: id.unwrap_or_else(|| new_id("dash")),
        name,
        icon,
        order,
        blocks,
        extra: Map::new(),
    };
    check_layout(&d)?;
    save(ctx, &d)?;
    Ok(d)
}

fn copy_blocks(blocks: &[Block]) -> Vec<Block> {
    blocks
        .iter()
        .map(|b| Block {
            id: new_id("blk"),
            ..b.clone()
        })
        .collect()
}

fn create(ctx: &Ctx<'_>, a: CreateArgs) -> CommandResult<Value> {
    let blocks = match &a.from {
        Some(f) => copy_blocks(&find(ctx, f)?.blocks),
        None => Vec::new(),
    };
    let d = create_dashboard(ctx, None, &a.name, a.icon.as_deref(), blocks)?;
    dashboard_json(ctx, &d, &startup_id(ctx)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    dashboard: String,
    #[serde(default)]
    name: Option<String>,
    /// A symbol name; an empty string removes the icon.
    #[serde(default)]
    icon: Option<String>,
}

fn update(ctx: &Ctx<'_>, a: UpdateArgs) -> CommandResult<Value> {
    let mut d = find(ctx, &a.dashboard)?;
    if let Some(n) = &a.name {
        d.name = check_name(ctx, n, Some(&d.id))?;
    }
    if let Some(i) = &a.icon {
        d.icon = check_icon(Some(i))?;
    }
    save(ctx, &d)?;
    ctx.quiet_noop.set(true);
    dashboard_json(ctx, &d, &startup_id(ctx)?)
}

fn delete(ctx: &Ctx<'_>, a: DashArgs) -> CommandResult<Value> {
    let d = find(ctx, &a.dashboard)?;
    if d.id == HOME {
        return Err(refused(
            "Home can't be deleted; you can reset it to standard.",
        ));
    }
    if startup_id(ctx)? == d.id {
        ctx.save_setting(MODULE, STARTUP_KEY, None)?;
    }
    ctx.save_setting(MODULE, &format!("{PREFIX}{}", d.id), None)?;
    Ok(json!({"deleted": d.id, "name": d.name, "startup": startup_id(ctx)?}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReorderArgs {
    /// Every dashboard id, once, in the new order.
    ids: Vec<String>,
}

fn reorder(ctx: &Ctx<'_>, a: ReorderArgs) -> CommandResult<Value> {
    let all = load_all(ctx)?;
    let given: BTreeSet<&str> = a.ids.iter().map(String::as_str).collect();
    let have: BTreeSet<&str> = all.iter().map(|d| d.id.as_str()).collect();
    if given != have || given.len() != a.ids.len() {
        return Err(refused("List every dashboard id exactly once."));
    }
    for (i, id) in a.ids.iter().enumerate() {
        if let Some(d) = all.iter().find(|d| d.id == *id) {
            let order = i64::try_from(i).unwrap_or(i64::MAX);
            if d.order != order {
                let mut d = d.clone();
                d.order = order;
                save(ctx, &d)?;
            }
        }
    }
    ctx.quiet_noop.set(true);
    list(ctx, NoArgs {})
}

fn set_startup(ctx: &Ctx<'_>, a: DashArgs) -> CommandResult<Value> {
    let d = find(ctx, &a.dashboard)?;
    let v = (d.id != HOME).then(|| json!(d.id));
    ctx.save_setting(MODULE, STARTUP_KEY, v.as_ref())?;
    ctx.quiet_noop.set(true);
    Ok(json!({"startup": d.id, "name": d.name}))
}

fn reset_home(ctx: &Ctx<'_>, _: NoArgs) -> CommandResult<Value> {
    let mut d = find(ctx, HOME)?;
    d.blocks = standard_home_blocks();
    save(ctx, &d)?;
    ctx.quiet_noop.set(true);
    dashboard_json(ctx, &d, &startup_id(ctx)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddBlockArgs {
    dashboard: String,
    /// Block type: account_transactions, scheduled, balances, net_worth, chart,
    /// report_summary, shortcuts, attention, bills_due, subscriptions, projects or note.
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    settings: Value,
    /// At most 60 characters; empty for the title made from the type and settings.
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    x: Option<u32>,
    #[serde(default)]
    y: Option<u32>,
    #[serde(default)]
    w: Option<u32>,
    #[serde(default)]
    h: Option<u32>,
}

fn add_block(ctx: &Ctx<'_>, a: AddBlockArgs) -> CommandResult<Value> {
    let mut d = find(ctx, &a.dashboard)?;
    let t = grid::block_type(&a.kind).ok_or_else(|| {
        refused(format!(
            "Unknown block type {:?}; use one of {}.",
            a.kind,
            grid::BLOCK_TYPES.map(|t| t.name).join(", ")
        ))
    })?;
    if d.blocks.len() >= grid::MAX_BLOCKS {
        return Err(refused(format!(
            "A dashboard holds at most {} blocks.",
            grid::MAX_BLOCKS
        )));
    }
    let settings = check_settings(ctx, &a.kind, &a.settings)?;
    let title = check_title(a.title.as_deref())?;
    let w = a.w.unwrap_or(t.default_w);
    let h = a.h.unwrap_or(t.default_h);
    let cells: Vec<Cell> = d.blocks.iter().map(Block::cell).collect();
    let mut b = block(
        &new_id("blk"),
        &a.kind,
        title.as_deref(),
        settings,
        grid::append(&cells, w, h),
    );
    b.w = w;
    b.h = h;
    d.blocks.push(b.clone());
    if let (Some(x), Some(y)) = (a.x, a.y) {
        let all: Vec<Cell> = d.blocks.iter().map(Block::cell).collect();
        let placed = grid::drop_at(&all, all.len().saturating_sub(1), Cell::new(x, y, w, h));
        for (blk, c) in d.blocks.iter_mut().zip(placed) {
            blk.set_cell(c);
        }
    }
    check_layout(&d)?;
    save(ctx, &d)?;
    let added = d.blocks.iter().find(|x| x.id == b.id).cloned().unwrap_or(b);
    block_json(ctx, &added)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateBlockArgs {
    dashboard: String,
    block: String,
    /// The block's title; an empty string returns to the title made from its type.
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    settings: Option<Value>,
}

fn update_block(ctx: &Ctx<'_>, a: UpdateBlockArgs) -> CommandResult<Value> {
    let mut d = find(ctx, &a.dashboard)?;
    let b = d
        .blocks
        .iter_mut()
        .find(|b| b.id == a.block)
        .ok_or_else(|| CommandError::NotFound(format!("block {:?}", a.block)))?;
    if let Some(t) = &a.title {
        b.title = check_title(Some(t))?;
    }
    if let Some(st) = &a.settings {
        b.settings = check_settings(ctx, &b.kind, st)?;
    }
    let out = b.clone();
    save(ctx, &d)?;
    ctx.quiet_noop.set(true);
    block_json(ctx, &out)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Placement {
    block: String,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveArgs {
    dashboard: String,
    /// Every block of the dashboard, once, with its rectangle in grid cells.
    layout: Vec<Placement>,
}

fn move_blocks(ctx: &Ctx<'_>, a: MoveArgs) -> CommandResult<Value> {
    let mut d = find(ctx, &a.dashboard)?;
    let given: BTreeSet<&str> = a.layout.iter().map(|p| p.block.as_str()).collect();
    let have: BTreeSet<&str> = d.blocks.iter().map(|b| b.id.as_str()).collect();
    if given != have || given.len() != a.layout.len() {
        return Err(refused(
            "The layout must list every block of the dashboard exactly once.",
        ));
    }
    for p in &a.layout {
        if let Some(b) = d.blocks.iter_mut().find(|b| b.id == p.block) {
            b.set_cell(Cell::new(p.x, p.y, p.w, p.h));
        }
    }
    check_layout(&d)?;
    save(ctx, &d)?;
    ctx.quiet_noop.set(true);
    dashboard_json(ctx, &d, &startup_id(ctx)?)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BlockArgs {
    dashboard: String,
    block: String,
}

fn remove_block(ctx: &Ctx<'_>, a: BlockArgs) -> CommandResult<Value> {
    let mut d = find(ctx, &a.dashboard)?;
    let before = d.blocks.len();
    d.blocks.retain(|b| b.id != a.block);
    if d.blocks.len() == before {
        return Err(CommandError::NotFound(format!("block {:?}", a.block)));
    }
    let cells: Vec<Cell> = d.blocks.iter().map(Block::cell).collect();
    for (b, c) in d.blocks.iter_mut().zip(grid::compact(&cells)) {
        b.set_cell(c);
    }
    save(ctx, &d)?;
    dashboard_json(ctx, &d, &startup_id(ctx)?)
}

// ----- block data ----------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DataArgs {
    dashboard: String,
    block: String,
    /// The current date (default: the local date of the machine).
    #[serde(default)]
    today: Option<Date>,
}

fn module_label(module: &str) -> String {
    let s = module.replace('-', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

fn status(st: &str, message: String) -> Value {
    json!({"status": st, "message": message})
}

fn module_off(module: &str) -> Value {
    status(
        "module_off",
        format!(
            "Turn on {} in Settings → Features to show this",
            module_label(module)
        ),
    )
}

fn dec(v: &Value) -> Decimal {
    v.as_str().and_then(|x| x.parse().ok()).unwrap_or_default()
}

fn money(d: Decimal, commodity: &str) -> String {
    mk_core::currency::format_amount(d, Some(mk_core::currency::minor_units(commodity)))
}

fn days(d: Date, n: i64) -> CommandResult<Date> {
    d.add_days(i32::try_from(n).unwrap_or(0))
        .ok_or_else(|| refused(format!("date out of range near {d}")))
}

fn arr(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn block_data(ctx: &Ctx<'_>, a: DataArgs) -> CommandResult<Value> {
    let d = find(ctx, &a.dashboard)?;
    let b = d
        .blocks
        .iter()
        .find(|b| b.id == a.block)
        .ok_or_else(|| CommandError::NotFound(format!("block {:?}", a.block)))?;
    let today = match a.today {
        Some(t) => t,
        None => mk_store::today_utc()?,
    };
    let st = &b.settings;
    let mut v = match b.kind.as_str() {
        "account_transactions" => account_transactions(ctx, st, today)?,
        "scheduled" => scheduled(ctx, st, today)?,
        "balances" => balances(ctx, st, today)?,
        "net_worth" => net_worth(ctx, today)?,
        "chart" => chart(ctx, st, today)?,
        "report_summary" => summary(ctx, st, today)?,
        "shortcuts" => shortcuts(ctx, st)?,
        "attention" => attention(ctx, today)?,
        "bills_due" => bills_due(ctx, st, today)?,
        "subscriptions" => subscriptions_block(ctx, today)?,
        "projects" => projects_block(ctx)?,
        "note" => json!({"status": "ok", "text": s(st, "text")}),
        _ => status("unknown_type", "This block needs a newer MoneyKeep".into()),
    };
    if let Some(o) = v.as_object_mut() {
        o.entry("status").or_insert(json!("ok"));
        o.insert("block".into(), json!(b.id));
        o.insert("type".into(), json!(b.kind));
        o.insert("title".into(), json!(display_title(ctx, b)));
    }
    Ok(v)
}

/// The account a block names: its record, or the status the block shows instead.
fn block_account(ctx: &Ctx<'_>, id: &str) -> Result<Value, Value> {
    if id.is_empty() {
        return Err(status("choose_account", "Choose an account".into()));
    }
    call(ctx, "account.get", json!({"account": id}))
        .map_err(|_| status("missing_account", "This account no longer exists".into()))
}

/// Whether a register row takes money out of the account (assets and income: a
/// decrease of the shown balance; liabilities, equity and expenses: an increase).
fn money_out(account_type: &str, increase: bool) -> bool {
    if matches!(account_type, "liability" | "equity" | "expense") {
        increase
    } else {
        !increase
    }
}

fn register_row(r: &Value) -> Value {
    let other = r.get("other").cloned().unwrap_or(Value::Null);
    let category = match s(&other, "kind") {
        "account" => s(&other, "full_name").to_owned(),
        "split" => format!(
            "Split ({})",
            other.get("count").cloned().unwrap_or(json!(0))
        ),
        _ => String::new(),
    };
    json!({"txn": r.get("txn"), "date": r.get("date"), "payee": r.get("payee"),
        "amount": r.get("amount"), "increase": r.get("increase"), "category": category})
}

fn account_transactions(ctx: &Ctx<'_>, st: &Value, today: Date) -> CommandResult<Value> {
    let acc = match block_account(ctx, s(st, "account")) {
        Ok(a) => a,
        Err(v) => return Ok(v),
    };
    let n = usize::try_from(st.get("n").and_then(Value::as_u64).unwrap_or(5)).unwrap_or(5);
    let children = st.get("include_children").and_then(Value::as_bool) == Some(true);
    let mut args = json!({"account": acc.get("id"), "order": "desc", "include_voided": false,
        "include_children": children, "limit": n});
    let above = s(st, "mode") == "above";
    if above {
        let back = st.get("days").and_then(Value::as_i64).unwrap_or(30);
        put(&mut args, "from", json!(days(today, -back)?));
        put(&mut args, "to", json!(today));
        put(&mut args, "limit", json!(5000));
    }
    let reg = call(ctx, "account.register", args)?;
    let ty = s(&acc, "type").to_owned();
    let threshold: Decimal = s(st, "amount").parse().unwrap_or_default();
    let dir = s(st, "direction").to_owned();
    let rows: Vec<Value> = arr(reg.get("rows").unwrap_or(&Value::Null))
        .iter()
        .filter(|r| {
            if !above {
                return true;
            }
            let amount = dec(r.get("amount").unwrap_or(&Value::Null)).abs();
            let out = money_out(
                &ty,
                r.get("increase").and_then(Value::as_bool) == Some(true),
            );
            amount >= threshold
                && match dir.as_str() {
                    "in" => !out,
                    "out" => out,
                    _ => true,
                }
        })
        .take(n)
        .map(register_row)
        .collect();
    // The balance today, whatever the rows' range.
    let bal = call(
        ctx,
        "account.balance",
        json!({"account": acc.get("id"), "include_children": children}),
    )?;
    let commodity = s(&acc, "commodity").to_owned();
    Ok(json!({
        "account": {"id": acc.get("id"), "name": acc.get("name"), "full_name": acc.get("full_name"),
            "type": ty},
        "balance": first_amount(bal.get("display").unwrap_or(&Value::Null), &commodity),
        "rows": rows,
    }))
}

/// The amount in `commodity` from a commodity map, else the first, else zero.
fn first_amount(map: &Value, commodity: &str) -> String {
    map.get(commodity)
        .and_then(Value::as_str)
        .or_else(|| {
            map.as_object()
                .and_then(|o| o.values().next())
                .and_then(Value::as_str)
        })
        .map_or_else(|| money(Decimal::ZERO, commodity), str::to_owned)
}

fn scheduled(ctx: &Ctx<'_>, st: &Value, today: Date) -> CommandResult<Value> {
    let next = s(st, "mode") != "days";
    let ahead = if next {
        731
    } else {
        st.get("days").and_then(Value::as_i64).unwrap_or(30)
    };
    let n = if next {
        usize::try_from(st.get("n").and_then(Value::as_u64).unwrap_or(5)).unwrap_or(5)
    } else {
        50
    };
    let accounts: Vec<String> = arr(st.get("accounts").unwrap_or(&Value::Null))
        .iter()
        .filter_map(|x| x.as_str().map(str::to_owned))
        .collect();
    let up = call(
        ctx,
        "schedule.upcoming",
        json!({"today": today, "to": days(today, ahead)?}),
    )?;
    let rows: Vec<Value> = arr(up.get("occurrences").unwrap_or(&Value::Null))
        .into_iter()
        .filter(|o| {
            accounts.is_empty() || {
                let m = o.get("money").unwrap_or(&Value::Null);
                let id = m
                    .get("account")
                    .map(|a| s(a, "id"))
                    .filter(|x| !x.is_empty())
                    .or_else(|| o.get("account").map(|a| s(a, "id")))
                    .unwrap_or_default();
                accounts.iter().any(|a| a == id)
            }
        })
        .take(n)
        .map(|mut o| {
            let overdue = s(&o, "when") == "overdue";
            let money = o.get("money").cloned().unwrap_or(Value::Null);
            let amount = if s(&money, "amount").is_empty() {
                s(&o, "amount").to_owned()
            } else {
                s(&money, "amount").to_owned()
            };
            let account = money
                .get("account")
                .map(|a| s(a, "full_name").to_owned())
                .unwrap_or_default();
            let payee = if s(&o, "payee").is_empty() {
                s(&o, "name").to_owned()
            } else {
                s(&o, "payee").to_owned()
            };
            if let Some(m) = o.as_object_mut() {
                m.insert("overdue".into(), json!(overdue));
                m.insert("shown_amount".into(), json!(amount));
                m.insert("shown_account".into(), json!(account));
                m.insert("shown_payee".into(), json!(payee));
            }
            o
        })
        .collect();
    Ok(json!({"rows": rows}))
}

/// Which sidebar group an account belongs to (app-desktop revision 23).
fn group_of(a: &Value) -> usize {
    match (s(a, "type"), s(a, "subtype")) {
        (_, "bank" | "cash") => 0,
        (_, "credit-card") => 1,
        (_, "brokerage" | "security") => 2,
        ("asset", _) => 3,
        ("liability", _) => 4,
        ("equity", _) => 5,
        _ => 6,
    }
}

fn balances(ctx: &Ctx<'_>, st: &Value, today: Date) -> CommandResult<Value> {
    let list = arr(&call(ctx, "account.list", json!({"today": today}))?);
    let investments = is_module_enabled(ctx.tx, "investments")?;
    let holdings = if investments {
        call(ctx, "inv.holdings", json!({"as_of": today})).ok()
    } else {
        None
    };
    let balance = |a: &Value| -> CommandResult<String> {
        if let Some(h) = &holdings
            && let Some(x) = arr(h.get("accounts").unwrap_or(&Value::Null))
                .iter()
                .find(|x| s(x, "account") == s(a, "id"))
        {
            return Ok(s(x, "total").to_owned());
        }
        let b = call(
            ctx,
            "account.balance",
            json!({"account": s(a, "id"), "include_children": a.get("placeholder") == Some(&json!(true))}),
        )?;
        Ok(first_amount(
            b.get("display").unwrap_or(&Value::Null),
            s(a, "commodity"),
        ))
    };
    let line = |a: &Value| -> CommandResult<Value> {
        Ok(
            json!({"id": a.get("id"), "name": a.get("name"), "full_name": a.get("full_name"),
            "balance": balance(a)?}),
        )
    };
    let shown = |a: &&Value| {
        a.get("placeholder") != Some(&json!(true))
            && !(investments && s(a, "subtype") == "security")
    };
    if s(st, "source") == "accounts" {
        let mut rows = Vec::new();
        let mut missing = 0;
        for id in arr(st.get("accounts").unwrap_or(&Value::Null)) {
            match list.iter().find(|a| a.get("id") == Some(&id)) {
                Some(a) => rows.push(line(a)?),
                None => missing += 1,
            }
        }
        return Ok(json!({"accounts": rows, "missing": missing}));
    }
    let chosen: Vec<String> = arr(st.get("groups").unwrap_or(&Value::Null))
        .iter()
        .filter_map(|g| g.as_str().map(str::to_owned))
        .collect();
    let mut groups = Vec::new();
    for (gi, name) in GROUPS.iter().enumerate() {
        let wanted = if chosen.is_empty() {
            gi + 1 < GROUPS.len()
        } else {
            chosen.iter().any(|c| c == name)
        };
        if !wanted {
            continue;
        }
        let members: Vec<&Value> = list
            .iter()
            .filter(shown)
            .filter(|a| group_of(a) == gi)
            .collect();
        if members.is_empty() {
            continue;
        }
        let commodity = members
            .first()
            .map_or("USD", |a| s(a, "commodity"))
            .to_owned();
        let mut total = Decimal::ZERO;
        let mut rows = Vec::new();
        for a in members {
            let row = line(a)?;
            total = total
                .checked_add(dec(row.get("balance").unwrap_or(&Value::Null)))
                .unwrap_or(total);
            rows.push(row);
        }
        // Categories have no meaningful total.
        let total = (gi + 1 < GROUPS.len()).then(|| money(total, &commodity));
        groups.push(json!({"name": name, "total": total, "accounts": rows}));
    }
    Ok(json!({"groups": groups}))
}

fn net_worth(ctx: &Ctx<'_>, today: Date) -> CommandResult<Value> {
    let tb = call(ctx, "report.trial_balance", json!({}))?;
    let eq = tb.get("equation").cloned().unwrap_or(Value::Null);
    let currency = s(&eq, "currency").to_owned();
    let mut assets = s(&eq, "assets").to_owned();
    let mut liabilities = s(&eq, "liabilities").to_owned();
    let mut net = money(
        dec(eq.get("assets").unwrap_or(&Value::Null))
            .checked_sub(dec(eq.get("liabilities").unwrap_or(&Value::Null)))
            .unwrap_or_default(),
        &currency,
    );
    // The last 12 month ends, the last being today; securities at market value.
    let (y, m) = Date::shift_month(today.year(), today.month(), -11)
        .ok_or_else(|| refused("date out of range"))?;
    let from = Date::from_ymd(y, m, 1)?;
    let nw = call(
        ctx,
        "report.net_worth",
        json!({"from": from, "to": today, "interval": "month", "today": today}),
    )?;
    let series: Vec<String> = arr(nw.get("net_worth").unwrap_or(&Value::Null))
        .iter()
        .map(|x| x.as_str().unwrap_or("0").to_owned())
        .collect();
    let labels: Vec<Value> = arr(nw.get("periods").unwrap_or(&Value::Null))
        .iter()
        .map(|p| p.get("to").cloned().unwrap_or(Value::Null))
        .collect();
    if is_module_enabled(ctx.tx, "investments")? {
        let last = |v: &Value| {
            arr(v)
                .last()
                .and_then(|x| x.as_str().map(str::to_owned))
                .unwrap_or_else(|| "0.00".into())
        };
        net = last(nw.get("net_worth").unwrap_or(&Value::Null));
        assets = last(
            nw.get("assets")
                .and_then(|a| a.get("totals"))
                .unwrap_or(&Value::Null),
        );
        liabilities = last(
            nw.get("liabilities")
                .and_then(|a| a.get("totals"))
                .unwrap_or(&Value::Null),
        );
    }
    // Change since the start of the month: today's figure less last month's end.
    let n = series.len();
    let prev = n
        .checked_sub(2)
        .and_then(|i| series.get(i))
        .and_then(|x| x.parse::<Decimal>().ok())
        .unwrap_or_default();
    let now: Decimal = net.parse().unwrap_or_default();
    Ok(json!({
        "currency": currency, "net_worth": net, "assets": assets, "liabilities": liabilities,
        "change": money(now.checked_sub(prev).unwrap_or_default(), &currency),
        "sparkline": series, "dates": labels,
        "open": {"report": "net_worth", "preset": "last_12_months"},
    }))
}

/// The report arguments for a block's period.
fn period_args(st: &Value, today: Date) -> Value {
    match s(st, "period") {
        "custom" => json!({"from": st.get("from"), "to": st.get("to"), "today": today}),
        "" => json!({"preset": "this_month", "today": today}),
        p => json!({"preset": p, "today": today}),
    }
}

fn open_of(report: &str, st: &Value) -> Value {
    match s(st, "period") {
        "custom" => {
            json!({"report": report, "preset": "custom", "from": st.get("from"), "to": st.get("to")})
        }
        "" => json!({"report": report, "preset": "this_month"}),
        p => json!({"report": report, "preset": p}),
    }
}

/// Each period's dates (`from`, `to`) of a report with periods: what a click on
/// that period's bar or point opens (dashboards.md revision 5).
fn spans(r: &Value) -> Value {
    Value::Array(
        arr(r.get("periods").unwrap_or(&Value::Null))
            .iter()
            .map(|p| json!({"from": p.get("from"), "to": p.get("to")}))
            .collect(),
    )
}

fn strings(v: &Value) -> Vec<String> {
    arr(v)
        .iter()
        .map(|x| x.as_str().unwrap_or("0").to_owned())
        .collect()
}

fn chart(ctx: &Ctx<'_>, st: &Value, today: Date) -> CommandResult<Value> {
    let kind = s(st, "chart");
    if let Some(Some(m)) = CHART_KINDS.iter().find(|k| k.0 == kind).map(|k| k.2)
        && !is_module_enabled(ctx.tx, m)?
    {
        return Ok(module_off(m));
    }
    let args = period_args(st, today);
    let null = Value::Null;
    Ok(match kind {
        "spending" => {
            let mut a = args;
            put(&mut a, "depth", json!(99));
            let r = call(ctx, "report.spending", a)?;
            let rows = arr(r.get("rows").unwrap_or(&null));
            // Top 10, the rest as "Other" (as the Spending report draws it).
            let mut items: Vec<Value> = rows
                .iter()
                .take(10)
                .map(|x| json!({"label": x.get("full_name"), "amount": x.get("amount")}))
                .collect();
            let rest = rows.iter().skip(10).fold(Decimal::ZERO, |t, x| {
                t.checked_add(dec(x.get("amount").unwrap_or(&null)))
                    .unwrap_or(t)
            });
            if !rest.is_zero() {
                items.push(json!({"label": "Other", "amount": rest.to_string()}));
            }
            json!({"chart": "hbars", "items": items, "from": r.get("from"), "to": r.get("to"),
                "open": open_of("spending", st)})
        }
        "income_expense" => {
            let mut a = args;
            put(&mut a, "interval", json!("month"));
            let r = call(ctx, "report.income_expense", a)?;
            let labels: Vec<Value> = arr(r.get("periods").unwrap_or(&null))
                .iter()
                .map(|p| p.get("label").cloned().unwrap_or(Value::Null))
                .collect();
            json!({"chart": "paired", "labels": labels, "periods": spans(&r), "series": [
                {"name": "Income", "values": strings(r.get("income").and_then(|x| x.get("totals")).unwrap_or(&null))},
                {"name": "Expenses", "values": strings(r.get("expenses").and_then(|x| x.get("totals")).unwrap_or(&null))},
                {"name": "Net", "values": strings(r.get("net").and_then(|x| x.get("amounts")).unwrap_or(&null))},
            ], "from": r.get("from"), "to": r.get("to"), "open": open_of("income_expense", st)})
        }
        "net_worth" => {
            let mut a = args;
            put(&mut a, "interval", json!("month"));
            let r = call(ctx, "report.net_worth", a)?;
            let labels: Vec<Value> = arr(r.get("periods").unwrap_or(&null))
                .iter()
                .map(|p| p.get("label").cloned().unwrap_or(Value::Null))
                .collect();
            json!({"chart": "lines", "labels": labels, "periods": spans(&r), "series": [
                {"name": "Net worth", "values": strings(r.get("net_worth").unwrap_or(&null))},
                {"name": "Assets", "values": strings(r.get("assets").and_then(|x| x.get("totals")).unwrap_or(&null))},
                {"name": "Liabilities", "values": strings(r.get("liabilities").and_then(|x| x.get("totals")).unwrap_or(&null))},
            ], "from": r.get("from"), "to": r.get("to"), "open": open_of("net_worth", st)})
        }
        "account_balance" => {
            let acc = match block_account(ctx, s(st, "account")) {
                Ok(a) => a,
                Err(v) => return Ok(v),
            };
            // The period's month ends, from the net worth report's periods.
            let mut a = args;
            put(&mut a, "interval", json!("month"));
            let r = call(ctx, "report.net_worth", a)?;
            let mut labels = Vec::new();
            let mut values = Vec::new();
            for p in arr(r.get("periods").unwrap_or(&null)) {
                let b = call(
                    ctx,
                    "account.balance",
                    json!({"account": acc.get("id"), "as_of": p.get("to")}),
                )?;
                labels.push(p.get("label").cloned().unwrap_or(Value::Null));
                values.push(first_amount(
                    b.get("display").unwrap_or(&null),
                    s(&acc, "commodity"),
                ));
            }
            json!({"chart": "lines", "labels": labels, "periods": spans(&r), "series": [
                {"name": s(&acc, "full_name"), "values": values}],
                "from": r.get("from"), "to": r.get("to"),
                "open": {"register": acc.get("id")}})
        }
        "budget" => {
            let r = call(
                ctx,
                "budget.report",
                json!({"month": format!("{:04}-{:02}", today.year(), today.month()), "today": today}),
            );
            let Ok(r) = r else {
                return Ok(status("no_budget", "Create a budget to show this".into()));
            };
            let rows = arr(r
                .get("expenses")
                .and_then(|x| x.get("rows"))
                .unwrap_or(&null));
            let labels: Vec<Value> = rows
                .iter()
                .take(8)
                .map(|x| {
                    x.get("account")
                        .and_then(|a| a.get("full_name"))
                        .cloned()
                        .unwrap_or(Value::Null)
                })
                .collect();
            let col = |k: &str| -> Vec<String> {
                rows.iter().take(8).map(|x| s(x, k).to_owned()).collect()
            };
            json!({"chart": "paired", "labels": labels,
                "month": format!("{:04}-{:02}", today.year(), today.month()), "series": [
                {"name": "Budget", "values": col("budget")},
                {"name": "Actual", "values": col("actual")},
                {"name": "Remaining", "values": col("remaining")},
            ], "open": {"screen": "budget"}})
        }
        _ => {
            let mut a = args;
            put(&mut a, "group", json!("total"));
            let r = call(ctx, "report.performance", a)?;
            let series = arr(r.get("series").unwrap_or(&null));
            let first = series.first().cloned().unwrap_or(Value::Null);
            let points = arr(first.get("points").unwrap_or(&null));
            json!({"chart": "lines", "labels": points.iter().map(|p| p.get("date").cloned().unwrap_or(Value::Null)).collect::<Vec<_>>(),
                "series": [{"name": "Portfolio value", "values": points.iter().map(|p| s(p, "value").to_owned()).collect::<Vec<_>>()}],
                "from": r.get("from"), "to": r.get("to"), "open": open_of("performance", st)})
        }
    })
}

/// The period before `from`–`to`: as many whole months when it is whole months,
/// else as many days.
fn previous_period(from: Date, to: Date) -> CommandResult<(Date, Date)> {
    let whole = from.day() == 1 && to.day() == Date::days_in_month(to.year(), to.month());
    if whole {
        let months = (i64::from(to.year()) * 12 + i64::from(to.month()))
            - (i64::from(from.year()) * 12 + i64::from(from.month()))
            + 1;
        let (fy, fm) = Date::shift_month(from.year(), from.month(), -months)
            .ok_or_else(|| refused("date out of range"))?;
        let (ty, tm) = Date::shift_month(to.year(), to.month(), -months)
            .ok_or_else(|| refused("date out of range"))?;
        return Ok((
            Date::from_ymd(fy, fm, 1)?,
            Date::from_ymd_clamped(ty, tm, 31)?,
        ));
    }
    let len = from.days_until(to);
    let pto = days(from, -1)?;
    Ok((days(pto, -len)?, pto))
}

fn summary(ctx: &Ctx<'_>, st: &Value, today: Date) -> CommandResult<Value> {
    let report = s(st, "report");
    let Some((_, label, module)) = SUMMARIES.iter().find(|k| k.0 == report).copied() else {
        return Ok(status(
            "unknown_type",
            "This block needs a newer MoneyKeep".into(),
        ));
    };
    if let Some(m) = module
        && !is_module_enabled(ctx.tx, m)?
    {
        return Ok(module_off(m));
    }
    // The period's dates, from a report that resolves presets.
    let span = call(ctx, "report.spending", period_args(st, today))?;
    let from: Date = s(&span, "from").parse().unwrap_or(today);
    let to: Date = s(&span, "to").parse().unwrap_or(today);
    let (pfrom, pto) = previous_period(from, to)?;
    let null = Value::Null;
    let figure = |from: Date, to: Date| -> CommandResult<Decimal> {
        let range = json!({"from": from, "to": to, "today": today});
        Ok(match report {
            "profit_loss" => {
                let r = call(ctx, "report.profit_loss", range)?;
                dec(r
                    .get("net_income")
                    .and_then(|x| x.get("total"))
                    .unwrap_or(&null))
            }
            "cash_flow" => {
                let r = call(ctx, "report.cash_flow", range)?;
                dec(r
                    .get("inflows")
                    .and_then(|x| x.get("total"))
                    .unwrap_or(&null))
                .checked_sub(dec(r
                    .get("outflows")
                    .and_then(|x| x.get("total"))
                    .unwrap_or(&null)))
                .unwrap_or_default()
            }
            "ar_total" | "ap_total" => {
                let name = if report == "ar_total" {
                    "report.ar_aging"
                } else {
                    "report.ap_aging"
                };
                let as_of = if to > today { today } else { to };
                let r = call(ctx, name, json!({"as_of": as_of}))?;
                dec(r
                    .get("totals")
                    .and_then(|x| x.get("total"))
                    .unwrap_or(&null))
            }
            "sales_tax_due" => {
                let r = call(ctx, "report.sales_tax_liability", range)?;
                dec(r
                    .get("totals")
                    .and_then(|x| x.get("balance"))
                    .unwrap_or(&null))
            }
            _ => {
                let r = call(ctx, "report.1099", json!({"year": to.year()}))?;
                arr(r.get("vendors").unwrap_or(&null))
                    .iter()
                    .fold(Decimal::ZERO, |t, v| {
                        t.checked_add(dec(v.get("total").unwrap_or(&null)))
                            .unwrap_or(t)
                    })
            }
        })
    };
    let now = figure(from, to)?;
    let before = if report == "1099_total" {
        let r = call(
            ctx,
            "report.1099",
            json!({"year": to.year().saturating_sub(1)}),
        )?;
        arr(r.get("vendors").unwrap_or(&null))
            .iter()
            .fold(Decimal::ZERO, |t, v| {
                t.checked_add(dec(v.get("total").unwrap_or(&null)))
                    .unwrap_or(t)
            })
    } else {
        figure(pfrom, pto)?
    };
    let open_report = match report {
        "ar_total" => "ar_aging",
        "ap_total" => "ap_aging",
        "sales_tax_due" => "sales_tax_liability",
        "1099_total" => "1099",
        r => r,
    };
    let book = call(ctx, "book.info", json!({}))?;
    let currency = s(&book, "base_currency").to_owned();
    Ok(json!({
        "label": label, "figure": money(now, &currency), "previous": money(before, &currency),
        "change": money(now.checked_sub(before).unwrap_or_default(), &currency),
        "from": from, "to": to, "previous_from": pfrom, "previous_to": pto,
        "open": open_of(open_report, st),
    }))
}

fn shortcuts(ctx: &Ctx<'_>, st: &Value) -> CommandResult<Value> {
    let all = load_all(ctx)?;
    let mut out = Vec::new();
    for b in arr(st.get("buttons").unwrap_or(&Value::Null)) {
        let t = b.get("target").cloned().unwrap_or(Value::Null);
        let mut resolved = t.clone();
        let mut missing = false;
        let account = s(&t, "account");
        if !account.is_empty() {
            match call(ctx, "account.get", json!({"account": account})) {
                Ok(a) => put(
                    &mut resolved,
                    "full_name",
                    a.get("full_name").cloned().unwrap_or(Value::Null),
                ),
                Err(_) => missing = true,
            }
        } else if matches!(s(&t, "kind"), "account") {
            missing = true;
        }
        if s(&t, "kind") == "dashboard" {
            match all.iter().find(|d| d.id == s(&t, "dashboard")) {
                Some(d) => put(&mut resolved, "name", json!(d.name)),
                None => missing = true,
            }
        }
        let icon = b
            .get("icon")
            .and_then(Value::as_str)
            .filter(|i| !i.is_empty())
            .unwrap_or_else(|| default_icon(&t));
        out.push(
            json!({"label": b.get("label"), "icon": icon, "target": resolved,
            "missing": missing}),
        );
    }
    let size = match s(st, "button_size") {
        "" => "medium",
        x => x,
    };
    Ok(json!({"buttons": out, "button_size": size}))
}

fn item(label: String, target: Value) -> Value {
    json!({"label": label, "target": target})
}

fn plural(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// The attention list (what Home always listed).
fn attention(ctx: &Ctx<'_>, today: Date) -> CommandResult<Value> {
    let null = Value::Null;
    let mut items = Vec::new();
    for b in arr(&call(ctx, "import.list", json!({"status": "staged"}))?) {
        let source = if s(&b, "source").is_empty() {
            "file"
        } else {
            s(&b, "source")
        };
        let id = s(&b, "id").to_owned();
        items.push(item(
            format!(
                "Import waiting for review: {source} ({} rows)",
                b.get("rows").cloned().unwrap_or(json!(0))
            ),
            json!({"screen": "import", "batch": id}),
        ));
        if let Ok(full) = call(ctx, "import.get", json!({"batch": id})) {
            let review = arr(full.get("rows").unwrap_or(&null))
                .iter()
                .filter(|r| r.get("needs_review") == Some(&json!(true)))
                .count();
            if review > 0 {
                items.push(item(
                    format!("{review} imported row(s) need review"),
                    json!({"screen": "import", "batch": id}),
                ));
            }
        }
    }
    for a in arr(&call(ctx, "account.list", json!({}))?) {
        if !matches!(s(&a, "type"), "asset" | "liability")
            || a.get("placeholder") == Some(&json!(true))
        {
            continue;
        }
        if call(ctx, "reconcile.get", json!({"account": s(&a, "id")})).is_ok() {
            items.push(item(
                format!("Reconciliation open: {}", s(&a, "full_name")),
                json!({"screen": "reconcile", "account": s(&a, "id")}),
            ));
        }
    }
    // opening-balances.md: accounts without an opening balance, holdings without
    // cost basis; both open the setup checklist.
    let list = crate::opening::checklist(ctx, None)?;
    let need = list
        .get("need_opening")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if need > 0 {
        items.push(item(
            crate::opening::accounts_text(usize::try_from(need).unwrap_or(usize::MAX)),
            json!({"screen": "opening"}),
        ));
    }
    let basis = list
        .get("holdings_need_basis")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if basis > 0 {
        items.push(item(
            crate::opening::holdings_text(usize::try_from(basis).unwrap_or(usize::MAX)),
            json!({"screen": "opening"}),
        ));
    }
    // investments-options-bonds-crypto.md: expired options and matured bonds.
    if crate::investments::is_enabled(ctx)? {
        for (label, target) in crate::inv_kinds::attention(ctx, today)? {
            items.push(item(label, target));
        }
    }
    // Scheduled transactions: overdue, and due in the next 7 days.
    let up = call(
        ctx,
        "schedule.upcoming",
        json!({"today": today, "to": days(today, 30)?}),
    )?;
    let week = days(today, 7)?.to_string();
    let occ = arr(up.get("occurrences").unwrap_or(&null));
    let overdue = occ.iter().filter(|o| s(o, "when") == "overdue").count() as u64;
    let soon = occ
        .iter()
        .filter(|o| s(o, "when") != "overdue" && s(o, "due") <= week.as_str())
        .count();
    if overdue > 0 {
        items.push(item(
            format!(
                "{} overdue",
                plural(overdue, "scheduled transaction", "scheduled transactions")
            ),
            json!({"screen": "scheduled"}),
        ));
    }
    if soon > 0 {
        items.push(item(
            format!("{soon} due in the next 7 days"),
            json!({"screen": "scheduled"}),
        ));
    }
    if is_module_enabled(ctx.tx, "bills")? {
        let a = call(ctx, "bill.attention", json!({"today": today}))?;
        let overdue = a.get("overdue").and_then(Value::as_u64).unwrap_or(0);
        let soon = a
            .get("due_next_7_days")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if overdue > 0 {
            items.push(item(
                format!("{} overdue", plural(overdue, "bill", "bills")),
                json!({"screen": "purchases"}),
            ));
        }
        if soon > 0 {
            items.push(item(
                format!("{} due in the next 7 days", plural(soon, "bill", "bills")),
                json!({"screen": "purchases"}),
            ));
        }
    }
    if is_module_enabled(ctx.tx, "funds")? {
        for f in arr(&call(ctx, "fund.attention", json!({"today": today}))?) {
            items.push(item(
                s(&f, "message").to_owned(),
                json!({"screen": "accounts", "tab": "fund"}),
            ));
        }
    }
    if is_module_enabled(ctx.tx, "expenses")? {
        let phone = arr(&call(ctx, "expense.list", json!({}))?)
            .iter()
            .filter(|e| s(e, "source") == "phone" && matches!(s(e, "status"), "draft" | "ready"))
            .count() as u64;
        if phone > 0 {
            items.push(item(
                format!(
                    "{} from your phone to review",
                    plural(phone, "expense", "expenses")
                ),
                json!({"screen": "expenses", "tab": "expenses"}),
            ));
        }
        let n = arr(&call(ctx, "receipt.inbox", json!({}))?).len() as u64;
        if n > 0 {
            items.push(item(
                format!("{} in the inbox", plural(n, "receipt", "receipts")),
                json!({"screen": "expenses", "tab": "inbox"}),
            ));
        }
    }
    // Trips from the phone that need a choice (a vehicle, a purpose or a
    // destination; mileage.md "Capture server and API").
    if is_module_enabled(ctx.tx, crate::mileage::MODULE)? {
        let phone = arr(&call(ctx, "trip.list", json!({"status": "draft"}))?)
            .iter()
            .filter(|t| s(t, "source") == "phone")
            .count() as u64;
        if phone > 0 {
            items.push(item(
                format!("{} from your phone", plural(phone, "trip", "trips")),
                json!({"screen": "mileage", "tab": "trips"}),
            ));
        }
    }
    let feeds = is_module_enabled(ctx.tx, "bank-feeds")?
        || is_module_enabled(ctx.tx, "payment-processing")?
        || is_module_enabled(ctx.tx, "investments")?;
    if feeds {
        for it in arr(&call(ctx, "connect.attention", json!({"today": today}))?) {
            let msg = s(&it, "message").to_owned();
            let target = match s(&it, "kind") {
                "batch" => json!({"screen": "import", "batch": s(&it, "batch")}),
                "clearing" | "holdings" => {
                    json!({"screen": "register", "account": s(&it, "account")})
                }
                _ => json!({"screen": "settings", "section": "Connections"}),
            };
            let mut v = item(msg, target);
            // connectivity.md revision 5: connection notices carry `connect.dismiss`'s
            // arguments.
            if it.get("dismissible") == Some(&json!(true))
                && let (Some(o), Some(d)) = (v.as_object_mut(), it.get("dismiss"))
            {
                o.insert("dismiss".into(), d.clone());
            }
            items.push(v);
        }
    }
    // subscriptions.md: "charged after cancelling" first, the other alerts last.
    let mut subs = crate::subscriptions::attention(ctx, today)?;
    let split = subs
        .iter()
        .position(|a| a.get("kind").and_then(Value::as_str) != Some("charged_after_cancel"))
        .unwrap_or(subs.len());
    let rest = subs.split_off(split);
    subs.extend(items);
    // projects.md: budget alerts.
    subs.extend(crate::projects::attention(ctx)?);
    subs.extend(rest);
    Ok(json!({"items": subs}))
}

/// The Projects block (projects.md "Alerts and Home").
fn projects_block(ctx: &Ctx<'_>) -> CommandResult<Value> {
    if !is_module_enabled(ctx.tx, crate::projects::MODULE)? {
        return Ok(status(
            "module_off",
            "Turn on Projects in Settings → Features to show this".into(),
        ));
    }
    crate::projects::block(ctx)
}

/// The Subscriptions block (subscriptions.md "Home block").
fn subscriptions_block(ctx: &Ctx<'_>, today: Date) -> CommandResult<Value> {
    if !is_module_enabled(ctx.tx, crate::subscriptions::MODULE)? {
        return Ok(status(
            "module_off",
            "Turn on Subscriptions in Settings → Features to show this".into(),
        ));
    }
    call(ctx, "subscription.summary", json!({"today": today}))
}

fn bills_due(ctx: &Ctx<'_>, st: &Value, today: Date) -> CommandResult<Value> {
    let bills = is_module_enabled(ctx.tx, "bills")?;
    let invoicing = is_module_enabled(ctx.tx, "invoicing")?;
    if !bills && !invoicing {
        return Ok(status(
            "module_off",
            "Turn on Bills or Invoicing in Settings → Features to show this".into(),
        ));
    }
    let ahead = st.get("days").and_then(Value::as_i64).unwrap_or(14);
    let until = days(today, ahead)?;
    let null = Value::Null;
    let mut rows = Vec::new();
    if bills {
        let r = call(
            ctx,
            "report.bills_due",
            json!({"from": days(today, -3650)?, "to": until}),
        )?;
        for b in arr(r.get("bills").unwrap_or(&null)) {
            rows.push(
                json!({"kind": "bill", "id": b.get("bill"), "number": b.get("number"),
                "name": b.get("vendor_name"), "due": b.get("due_date"), "amount": b.get("balance"),
                "overdue": s(&b, "due_date") < today.to_string().as_str()}),
            );
        }
    }
    if invoicing {
        let r = call(ctx, "report.open_invoices", json!({"as_of": today}))?;
        for i in arr(r.get("invoices").unwrap_or(&null)) {
            if s(&i, "due_date") > until.to_string().as_str() {
                continue;
            }
            rows.push(json!({"kind": "invoice", "id": i.get("invoice"), "number": i.get("number"),
                "name": i.get("customer_name"), "due": i.get("due_date"), "amount": i.get("balance"),
                "overdue": s(&i, "due_date") < today.to_string().as_str()}));
        }
    }
    rows.sort_by(|a, b| s(a, "due").cmp(s(b, "due")));
    Ok(json!({"rows": rows}))
}

// ----- definition files ----------------------------------------------------------------

/// A dashboard's portable form for definition files: account and dashboard references
/// as full account names and dashboard names; blocks in reading order; no ids.
pub(crate) fn portable(ctx: &Ctx<'_>, d: &Dashboard) -> CommandResult<Value> {
    let all = load_all(ctx)?;
    let name_of = |id: &str| -> String {
        if id.is_empty() {
            return String::new();
        }
        call(ctx, "account.get", json!({"account": id}))
            .ok()
            .map(|a| s(&a, "full_name").to_owned())
            .unwrap_or_default()
    };
    let cells: Vec<Cell> = d.blocks.iter().map(Block::cell).collect();
    let mut blocks = Vec::new();
    for i in grid::reading_order(&cells) {
        let Some(b) = d.blocks.get(i) else { continue };
        let mut st = b.settings.clone();
        if let Some(o) = st.as_object_mut() {
            if let Some(id) = o.get("account").and_then(Value::as_str).map(str::to_owned) {
                let mut n = name_of(&id);
                if n.is_empty() {
                    n = o
                        .get("missing_account")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                }
                o.remove("missing_account");
                o.insert("account".into(), json!(n));
            }
            if let Some(ids) = o.get("accounts").and_then(Value::as_array).cloned() {
                let names: Vec<String> = ids
                    .iter()
                    .map(|x| name_of(x.as_str().unwrap_or_default()))
                    .filter(|n| !n.is_empty())
                    .collect();
                o.insert("accounts".into(), json!(names));
            }
            if let Some(buttons) = o.get_mut("buttons").and_then(Value::as_array_mut) {
                for btn in buttons {
                    if let Some(t) = btn.get_mut("target").and_then(Value::as_object_mut) {
                        if let Some(id) =
                            t.get("account").and_then(Value::as_str).map(str::to_owned)
                        {
                            t.insert("account".into(), json!(name_of(&id)));
                        }
                        if let Some(id) = t
                            .get("dashboard")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                        {
                            let n = all
                                .iter()
                                .find(|x| x.id == id)
                                .map(|x| x.name.clone())
                                .unwrap_or_default();
                            t.insert("dashboard".into(), json!(n));
                        }
                    }
                    // Files keep the target's fields in the button itself (the
                    // format's nesting limit).
                    if let Some(o) = btn.as_object_mut()
                        && let Some(Value::Object(t)) = o.remove("target")
                    {
                        for (k, v) in t {
                            o.insert(k, v);
                        }
                    }
                }
            }
        }
        let mut x = json!({"type": b.kind, "x": b.x, "y": b.y, "w": b.w, "h": b.h, "settings": st});
        if let Some(t) = &b.title {
            put(&mut x, "title", json!(t));
        }
        blocks.push(x);
    }
    let mut out = json!({"name": d.name, "blocks": blocks});
    if let Some(i) = &d.icon {
        put(&mut out, "icon", json!(i));
    }
    Ok(out)
}

/// Blocks from a portable form: names resolved in this book. An unknown account
/// leaves the block asking to choose one; an unknown dashboard or account in a
/// shortcut leaves that button out of reach until it is set again.
pub(crate) fn from_portable(
    ctx: &Ctx<'_>,
    v: &Value,
    itself: (&str, &str),
) -> CommandResult<(Vec<Block>, Vec<String>)> {
    let all = load_all(ctx)?;
    let mut notes = Vec::new();
    let resolve = |name: &str| -> Option<String> {
        if name.trim().is_empty() {
            return None;
        }
        call(ctx, "account.get", json!({"account": name.trim()}))
            .ok()
            .map(|a| s(&a, "id").to_owned())
    };
    let mut blocks = Vec::new();
    for b in arr(v.get("blocks").unwrap_or(&Value::Null)) {
        let kind = s(&b, "type").to_owned();
        let mut st = b.get("settings").cloned().unwrap_or_else(|| json!({}));
        if let Some(o) = st.as_object_mut() {
            if let Some(name) = o.get("account").and_then(Value::as_str).map(str::to_owned) {
                match resolve(&name) {
                    Some(id) => {
                        o.insert("account".into(), json!(id));
                    }
                    None => {
                        o.insert("account".into(), json!(""));
                        if !name.trim().is_empty() {
                            o.insert("missing_account".into(), json!(name));
                            notes.push(format!("{name} is not an account in this book; its block asks to choose one"));
                        }
                    }
                }
            }
            if let Some(names) = o.get("accounts").and_then(Value::as_array).cloned() {
                let ids: Vec<String> = names
                    .iter()
                    .filter_map(|n| {
                        let n = n.as_str().unwrap_or_default();
                        let id = resolve(n);
                        if id.is_none() {
                            notes.push(format!(
                                "{n} is not an account in this book; it was left out"
                            ));
                        }
                        id
                    })
                    .collect();
                o.insert("accounts".into(), json!(ids));
            }
            if let Some(buttons) = o.get_mut("buttons").and_then(Value::as_array_mut) {
                for btn in buttons.iter_mut() {
                    // A file's button holds its target's fields; the book nests them.
                    if let Some(o) = btn.as_object_mut()
                        && !o.contains_key("target")
                    {
                        let mut t = Map::new();
                        for k in o.keys().cloned().collect::<Vec<_>>() {
                            if k != "label"
                                && k != "icon"
                                && let Some(v) = o.remove(&k)
                            {
                                t.insert(k, v);
                            }
                        }
                        o.insert("target".into(), Value::Object(t));
                    }
                    if let Some(t) = btn.get_mut("target").and_then(Value::as_object_mut) {
                        if let Some(n) = t.get("account").and_then(Value::as_str).map(str::to_owned)
                        {
                            t.insert("account".into(), json!(resolve(&n).unwrap_or_default()));
                        }
                        if let Some(n) = t
                            .get("dashboard")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                        {
                            // The dashboard being installed can name itself.
                            let id = if n.eq_ignore_ascii_case(itself.0) {
                                itself.1.to_owned()
                            } else {
                                all.iter()
                                    .find(|x| x.name.eq_ignore_ascii_case(&n))
                                    .map(|x| x.id.clone())
                                    .unwrap_or_default()
                            };
                            t.insert("dashboard".into(), json!(id));
                        }
                    }
                }
            }
        }
        // Known types are checked like any other block; missing references in
        // shortcut buttons are kept empty (the button shows it).
        let settings = if grid::block_type(&kind).is_some() && kind != "shortcuts" {
            check_settings(ctx, &kind, &st)?
        } else {
            st
        };
        let title = check_title(b.get("title").and_then(Value::as_str))?;
        let num = |k: &str| {
            b.get(k)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0)
        };
        blocks.push(block(
            &new_id("blk"),
            &kind,
            title.as_deref(),
            settings,
            Cell::new(num("x"), num("y"), num("w"), num("h")),
        ));
    }
    Ok((blocks, notes))
}

/// Replace a dashboard's blocks (and icon) from a definition item.
pub(crate) fn replace_from_portable(
    ctx: &Ctx<'_>,
    id: &str,
    v: &Value,
) -> CommandResult<Vec<String>> {
    let mut d = find(ctx, id)?;
    let (blocks, notes) = from_portable(ctx, v, (&d.name, &d.id))?;
    d.blocks = blocks;
    d.icon = check_icon(v.get("icon").and_then(Value::as_str))?;
    check_layout(&d)?;
    save(ctx, &d)?;
    Ok(notes)
}

/// Create a dashboard from a definition item.
pub(crate) fn create_from_portable(
    ctx: &Ctx<'_>,
    name: &str,
    v: &Value,
) -> CommandResult<(String, Vec<String>)> {
    let id = new_id("dash");
    let (blocks, notes) = from_portable(ctx, v, (name.trim(), &id))?;
    let d = create_dashboard(
        ctx,
        Some(id),
        name,
        v.get("icon").and_then(Value::as_str),
        blocks,
    )?;
    Ok((d.id, notes))
}

/// Remove a dashboard installed from a definition (never Home).
pub(crate) fn remove(ctx: &Ctx<'_>, id: &str) -> CommandResult<()> {
    if id == HOME {
        return Ok(());
    }
    if startup_id(ctx)? == id {
        ctx.save_setting(MODULE, STARTUP_KEY, None)?;
    }
    ctx.save_setting(MODULE, &format!("{PREFIX}{id}"), None)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn d(x: &str) -> Date {
        x.parse().unwrap()
    }

    #[test]
    fn previous_periods() {
        assert_eq!(
            previous_period(d("2026-10-01"), d("2026-10-31")).ok(),
            Some((d("2026-09-01"), d("2026-09-30")))
        );
        assert_eq!(
            previous_period(d("2026-07-01"), d("2026-09-30")).ok(),
            Some((d("2026-04-01"), d("2026-06-30")))
        );
        assert_eq!(
            previous_period(d("2026-10-05"), d("2026-10-14")).ok(),
            Some((d("2026-09-25"), d("2026-10-04")))
        );
    }

    #[test]
    fn money_direction_by_account_type() {
        assert!(money_out("asset", false));
        assert!(!money_out("asset", true));
        assert!(money_out("liability", true));
        assert!(money_out("expense", true));
        assert!(!money_out("income", true));
    }
}
