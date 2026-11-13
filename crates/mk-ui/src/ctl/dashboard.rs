//! Dashboards (dashboards.md, app-desktop.md revision 31): the dashboard view with
//! its snap-to-grid blocks, Edit layout (drag, resize, keyboard and menu moves), the
//! Add block picker and block settings, and the Dashboards list in the rail's pane.
//! Layout rules live in `mk_core::dashboard`; the views draw the grid.

use std::collections::{BTreeSet, HashMap};

use mk_core::Decimal;
use mk_core::dashboard::{self as grid, Cell, Step};
use serde_json::{Value, json};

use super::charts;
use super::form::set;
use super::ui::{BOARD_COLUMNS, BOARD_ROW, ChartHit, Column, Tone, Ui, col, col_num};
use super::util::{Json, array};
use super::{App, AppEvent};
use crate::View;
use crate::fmt;

/// The narrowest a grid column may be before blocks stack in one column.
/// A block placed on the board: id, cell and the ghost of where it would land.
type Placed = (String, (i32, i32, i32, i32), Option<(i32, i32, i32, i32)>);
/// A list row: its accessible name and its cells (text, amount?, tone).
type ListRow = (String, Vec<(String, bool, Tone)>);

const MIN_COLUMN: i32 = 60;
const GAP: i32 = 8;
/// A block's title bar (30) and padding around and under it (`BlockView`), plus a
/// little room so the body never needs to scroll.
const BODY_CHROME: i32 = 30 + 24;

/// One edit-layout session: the layout being edited.
#[derive(Debug, Clone)]
struct Edit {
    dashboard: String,
    cells: Vec<(String, Cell)>,
    /// A block being dragged or resized and the cell it would land on.
    moving: Option<(String, Cell)>,
}

#[derive(Debug, Clone, PartialEq)]
enum Naming {
    New,
    Rename(String),
    Duplicate(String),
}

/// One shortcut button in the settings form.
#[derive(Debug, Clone, Default)]
struct ButtonForm {
    label: String,
    icon: String,
    kind: String,
    account: String,
    report: String,
    screen: String,
    dashboard: String,
    action: String,
}

/// The block settings form (new block or Settings…).
#[derive(Debug, Clone, Default)]
struct BlockForm {
    dashboard: String,
    block: Option<String>,
    kind: String,
    title: String,
    account: String,
    mode: String,
    n: String,
    amount: String,
    direction: String,
    days: String,
    include_children: bool,
    source: String,
    accounts: BTreeSet<String>,
    groups: BTreeSet<String>,
    chart: String,
    period: String,
    from: String,
    to: String,
    report: String,
    buttons: Vec<ButtonForm>,
    button_size: String,
    text: String,
    error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct DashState {
    list: Option<(u64, Vec<Value>)>,
    current: Option<(u64, String, Value)>,
    data: HashMap<(String, String), (String, Value)>,
    edit: Option<Edit>,
    naming_icon: String,
    picker: Option<String>,
    form: Option<BlockForm>,
    naming: Option<(Naming, String, Option<String>)>,
    confirm_delete: Option<(String, String)>,
    error: Option<String>,
}

// ----- data -------------------------------------------------------------------------

/// The dashboards in list order (cached per book change).
pub(crate) fn list(app: &mut App) -> Vec<Value> {
    if let Some((g, l)) = &app.dash.list
        && *g == app.generation
    {
        return l.clone();
    }
    let l = array(app.backend.invoke("dashboard.list", json!({})).ok());
    app.dash.list = Some((app.generation, l.clone()));
    l
}

/// The dashboards' names, in order (the rail tab's tooltip).
pub(crate) fn names(app: &mut App) -> Vec<String> {
    list(app).iter().map(|d| d.str("name").to_owned()).collect()
}

fn startup_id(app: &mut App) -> String {
    list(app)
        .iter()
        .find(|d| d.at("startup").as_bool() == Some(true))
        .map_or_else(|| "home".to_owned(), |d| d.str("id").to_owned())
}

/// Open a dashboard: the startup one is the Home view.
pub(crate) fn open(app: &mut App, id: &str) {
    if id == startup_id(app) {
        app.navigate(View::Home);
    } else {
        app.navigate(View::Dashboard { id: id.to_owned() });
    }
}

fn current_id(app: &mut App) -> String {
    match app.view.clone() {
        View::Dashboard { id } => id,
        _ => startup_id(app),
    }
}

fn get(app: &mut App, id: &str) -> Option<Value> {
    if let Some((g, i, v)) = &app.dash.current
        && *g == app.generation
        && i == id
    {
        return Some(v.clone());
    }
    let v = app
        .backend
        .invoke("dashboard.get", json!({"dashboard": id}))
        .ok()?;
    app.dash.current = Some((app.generation, id.to_owned(), v.clone()));
    Some(v)
}

/// A block's data as shown: the Attention block adds the app's own items
/// (release-packaging.md: "A newer capture server is ready.") first.
fn shown_data(app: &mut App, dash: &str, b: &Value) -> Value {
    let mut v = block_data(app, dash, b.str("id"));
    if b.str("type") == "attention"
        && let Some(item) = super::capture::attention_item(app)
        && let Some(Value::Array(items)) = v.get_mut("items")
    {
        items.insert(0, item);
    }
    v
}

fn block_data(app: &mut App, dash: &str, block: &str) -> Value {
    let key = (dash.to_owned(), block.to_owned());
    let stamp = format!("{}|{}", app.generation, app.today());
    if let Some((s, v)) = app.dash.data.get(&key)
        && *s == stamp
    {
        return v.clone();
    }
    let v = match app.backend.invoke(
        "dashboard.block_data",
        json!({"dashboard": dash, "block": block, "today": app.today()}),
    ) {
        Ok(v) => v,
        Err(e) => json!({"status": "error", "message": e.display()}),
    };
    app.dash.data.insert(key, (stamp, v.clone()));
    v
}

fn cells_of(d: &Value) -> Vec<(String, Cell)> {
    d.list("blocks")
        .iter()
        .map(|b| {
            let n = |k: &str| {
                b.at(k)
                    .as_u64()
                    .and_then(|x| u32::try_from(x).ok())
                    .unwrap_or(0)
            };
            (
                b.str("id").to_owned(),
                Cell::new(n("x"), n("y"), n("w"), n("h")),
            )
        })
        .collect()
}

fn min_of(d: &Value, block: &str) -> (u32, u32) {
    d.list("blocks")
        .iter()
        .find(|b| b.str("id") == block)
        .map_or((grid::MIN_W, grid::MIN_H), |b| {
            grid::min_size(b.str("type"))
        })
}

/// Keep an edit session in step with the stored blocks: added blocks join at the
/// bottom, removed ones leave.
fn sync_edit(app: &mut App, d: &Value) {
    let id = d.str("id").to_owned();
    let Some(e) = app.dash.edit.as_mut().filter(|e| e.dashboard == id) else {
        return;
    };
    let stored = cells_of(d);
    e.cells.retain(|(b, _)| stored.iter().any(|(s, _)| s == b));
    let mut added = false;
    for (b, c) in stored {
        if !e.cells.iter().any(|(x, _)| *x == b) {
            let all: Vec<Cell> = e.cells.iter().map(|x| x.1).collect();
            let at = grid::append(&all, c.w, c.h);
            e.cells.push((b, at));
            added = true;
        }
    }
    if added {
        let all: Vec<Cell> = e.cells.iter().map(|x| x.1).collect();
        for (slot, c) in e.cells.iter_mut().zip(grid::compact(&all)) {
            slot.1 = c;
        }
    }
}

fn editing(app: &App, dash: &str) -> bool {
    app.dash.edit.as_ref().is_some_and(|e| e.dashboard == dash)
}

/// Apply a move or resize step to the edited layout.
fn step_block(app: &mut App, d: &Value, block: &str, s: Step) {
    let min = min_of(d, block);
    let Some(e) = app.dash.edit.as_mut() else {
        return;
    };
    let Some(i) = e.cells.iter().position(|(b, _)| b == block) else {
        return;
    };
    let all: Vec<Cell> = e.cells.iter().map(|x| x.1).collect();
    for (slot, c) in e.cells.iter_mut().zip(grid::step(&all, i, s, min)) {
        slot.1 = c;
    }
}

/// Drop `block` at `target` (a drag or resize ending).
fn drop_block(app: &mut App, d: &Value, block: &str, target: Cell) {
    let (min_w, min_h) = min_of(d, block);
    let Some(e) = app.dash.edit.as_mut() else {
        return;
    };
    e.moving = None;
    let Some(i) = e.cells.iter().position(|(b, _)| b == block) else {
        return;
    };
    let t = Cell::new(
        target.x,
        target.y,
        target
            .w
            .max(min_w)
            .min(grid::COLUMNS - target.x.min(grid::COLUMNS - 1)),
        target.h.max(min_h),
    );
    let all: Vec<Cell> = e.cells.iter().map(|x| x.1).collect();
    for (slot, c) in e.cells.iter_mut().zip(grid::drop_at(&all, i, t)) {
        slot.1 = c;
    }
}

/// Done: save the edited layout as one `dashboard.move_blocks` (one undo step).
fn finish_edit(app: &mut App, d: &Value) {
    let Some(e) = app.dash.edit.take() else {
        return;
    };
    if e.cells == cells_of(d) {
        return;
    }
    let layout: Vec<Value> = e
        .cells
        .iter()
        .map(|(b, c)| json!({"block": b, "x": c.x, "y": c.y, "w": c.w, "h": c.h}))
        .collect();
    if let Err(err) = app.run(
        "dashboard.move_blocks",
        json!({"dashboard": e.dashboard, "layout": layout}),
    ) {
        app.dash.error = Some(err.display());
        app.dash.edit = Some(e);
    }
}

// ----- navigation targets ------------------------------------------------------------

/// Go where an attention item, a shortcut or a block points.
fn go(app: &mut App, t: &Value) {
    match (t.str("kind"), t.str("screen")) {
        ("account", _) => app.navigate(View::Register {
            account: t.str("account").to_owned(),
        }),
        ("dashboard", _) => {
            let id = t.str("dashboard").to_owned();
            open(app, &id);
        }
        ("report", _) => super::reports::open(
            app,
            t.str("report"),
            t.str("preset"),
            t.str("from"),
            t.str("to"),
        ),
        ("action", _) => action(app, t),
        (_, "import") => app.navigate(View::Import {
            batch: t.str("batch").to_owned(),
        }),
        (_, "reconcile") => app.navigate(View::Reconcile {
            account: t.str("account").to_owned(),
        }),
        (_, "register") => app.navigate(View::Register {
            account: t.str("account").to_owned(),
        }),
        (_, "scheduled") => app.navigate(View::Scheduled),
        (_, "purchases") => app.navigate(View::Purchases),
        (_, "sales") => app.navigate(View::Sales),
        (_, "budget") => app.navigate(View::Budget),
        (_, "securities") => app.navigate(View::Securities),
        (_, "reports") => app.navigate(View::Reports),
        (_, "home") => app.navigate(View::Home),
        // opening-balances.md: the setup checklist (a dialog over Home).
        (_, "opening") => super::opening::open_checklist(app, None),
        // investments-options-bonds-crypto.md: an expired option, a matured bond.
        (_, "option_expiry") => {
            super::inv_more::open_expiry(app, t.str("account"), t.str("security"))
        }
        (_, "bond_redeem") => {
            super::inv_more::open_redeem(app, t.str("account"), t.str("security"))
        }
        (_, "accounts") => {
            if t.str("tab") == "fund" {
                app.accounts.tab = "fund".into();
            }
            app.navigate(View::Accounts);
        }
        (_, "subscriptions") => super::subscriptions::open(app, t),
        (_, "projects") => super::projects::open(app, t),
        (_, "mileage") => {
            // Trips from the phone to finish (mileage.md).
            app.mileage.tab = super::mileage::Tab::Trips;
            app.navigate(View::Mileage);
        }
        (_, "expenses") => {
            // The inbox, or the expense list (phone expenses to review).
            app.expenses_tab_inbox = t.str("tab") == "inbox";
            if !app.expenses_tab_inbox {
                app.expenses.tab = super::expenses::Tab::Expenses;
            }
            app.navigate(View::Expenses);
        }
        (_, "settings") => {
            if !t.str("section").is_empty() {
                app.config.settings_section = t.str("section").to_owned();
            }
            app.navigate(View::Settings);
        }
        _ => {}
    }
}

/// A shortcut's action: each opens its dialog or screen; nothing posts by itself.
fn action(app: &mut App, t: &Value) {
    let account = t.str("account").to_owned();
    match t.str("action") {
        "new_transaction" => {
            app.register.pending_new = true;
            app.navigate(View::Register { account });
        }
        "reconcile" => app.navigate(View::Reconcile { account }),
        "import_file" => app.emit(AppEvent::ChooseImportFile),
        "calculator" => {
            if !app.calc_open() {
                app.toggle_calculator();
            }
        }
        "new_expense" => {
            app.expenses_new = true;
            app.navigate(View::Expenses);
        }
        "collect_from_phone" => {
            app.config.settings_section = "Phone capture".to_owned();
            app.navigate(View::Settings);
        }
        _ => {}
    }
}

// ----- the view ----------------------------------------------------------------------

/// The dashboard view: `id` names it (`None`: the startup dashboard, the Home view).
pub(crate) fn view(app: &mut App, ui: &mut Ui, id: Option<String>) {
    let id = id.unwrap_or_else(|| startup_id(app));
    let Some(d) = get(app, &id).or_else(|| {
        let s = startup_id(app);
        get(app, &s)
    }) else {
        ui.title("Home");
        return;
    };
    let id = d.str("id").to_owned();
    sync_edit(app, &d);
    header(app, ui, &d);
    if let Some(e) = app.dash.error.clone() {
        ui.error_text(e);
    }
    blocks(app, ui, &d, &id);
}

const DASH_MENU: [&str; 7] = [
    "Rename…",
    "Duplicate…",
    "Set as startup",
    "Reset to standard",
    "Delete…",
    "Move up",
    "Move down",
];

/// The header: the dashboard's icon and name, the ⋯ menu at the far right, and
/// Done just left of it while editing (dashboards.md revision 3).
fn header(app: &mut App, ui: &mut Ui, d: &Value) {
    let id = d.str("id").to_owned();
    let edit = editing(app, &id);
    ui.title(d.str("name"));
    let home = d.at("home").as_bool() == Some(true);
    ui.toolbar(|ui| {
        if let Some(sym) = d.at("icon").as_str() {
            ui.icon_image(&format!("sym-{sym}"));
        }
        if edit && ui.primary("Done").clicked() {
            finish_edit(app, d);
        }
        let mut entries: Vec<String> = Vec::new();
        if !edit {
            entries.push("Edit layout".into());
        }
        entries.push("+ Add block".into());
        let menu = dashboard_entries(home, false);
        entries.extend(menu.iter().map(|s| (*s).to_owned()));
        ui.icon("more").tip("More").named("Dashboard menu");
        if let Some(i) = ui.menu("", &entries).chosen {
            match entries.get(i).map(String::as_str) {
                Some("Edit layout") => start_edit(app, d),
                Some("+ Add block") => app.dash.picker = Some(id.clone()),
                Some(other) => {
                    let other = other.to_owned();
                    dashboard_action(app, &other, &id, d.str("name"));
                }
                None => {}
            }
        }
    });
    if edit {
        ui.board_edit(true);
        ui.secondary(
            "Drag a block by its title bar, or focus it and use the arrow keys (Shift to resize).",
        );
    }
}

fn start_edit(app: &mut App, d: &Value) {
    app.dash.error = None;
    app.dash.edit = Some(Edit {
        dashboard: d.str("id").to_owned(),
        cells: cells_of(d),
        moving: None,
    });
}

/// The entries of a dashboard's menu (its header, or its line in the list).
fn dashboard_entries(home: bool, in_list: bool) -> Vec<&'static str> {
    let mut v = Vec::new();
    if in_list {
        v.push("Open");
    }
    v.extend(["Rename…", "Duplicate…", "Set as startup"]);
    if in_list {
        v.extend(["Move up", "Move down"]);
    }
    v.push(if home {
        "Reset to standard"
    } else {
        "Delete…"
    });
    v
}

fn dashboard_action(app: &mut App, entry: &str, id: &str, name: &str) {
    debug_assert!(DASH_MENU.len() == 7);
    match entry {
        "Open" => open(app, id),
        "Rename…" => {
            app.dash.naming_icon = list(app)
                .iter()
                .find(|d| d.str("id") == id)
                .and_then(|d| d.at("icon").as_str().map(str::to_owned))
                .unwrap_or_default();
            app.dash.naming = Some((Naming::Rename(id.to_owned()), name.to_owned(), None));
        }
        "Duplicate…" => {
            app.dash.naming = Some((
                Naming::Duplicate(id.to_owned()),
                format!("{name} copy"),
                None,
            ))
        }
        "Set as startup" => {
            if let Err(e) = app.run("dashboard.set_startup", json!({"dashboard": id})) {
                app.dash.error = Some(e.display());
            }
        }
        "Move up" => reorder(app, id, -1),
        "Move down" => reorder(app, id, 1),
        "Reset to standard" => {
            if let Err(e) = app.run("dashboard.reset_home", json!({})) {
                app.dash.error = Some(e.display());
            }
        }
        "Delete…" => app.dash.confirm_delete = Some((id.to_owned(), name.to_owned())),
        _ => {}
    }
}

fn reorder(app: &mut App, id: &str, delta: i64) {
    let mut ids: Vec<String> = list(app).iter().map(|d| d.str("id").to_owned()).collect();
    let Some(i) = ids.iter().position(|x| x == id) else {
        return;
    };
    let j = if delta < 0 {
        i.checked_sub(1)
    } else {
        Some(i + 1).filter(|j| *j < ids.len())
    };
    let Some(j) = j else {
        return;
    };
    ids.swap(i, j);
    if let Err(e) = app.run("dashboard.reorder", json!({"ids": ids})) {
        app.dash.error = Some(e.display());
    }
}

fn to_i(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(0)
}

fn cell_tuple(c: Cell) -> (i32, i32, i32, i32) {
    (to_i(c.x), to_i(c.y), to_i(c.w), to_i(c.h))
}

/// Parse "x,y" from a drag report.
fn pair(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// The board's width in pixels.
fn board_width(app: &App) -> i32 {
    (app.window_width - 64 - 48 - 16).max(240)
}

fn blocks(app: &mut App, ui: &mut Ui, d: &Value, id: &str) {
    let edit = editing(app, id);
    let stored = cells_of(d);
    let base: Vec<(String, Cell)> = match &app.dash.edit {
        Some(e) if edit => e.cells.clone(),
        _ => stored,
    };
    let width = board_width(app);
    let narrow = width < BOARD_COLUMNS * MIN_COLUMN;
    let colw = (width - GAP * (BOARD_COLUMNS - 1)) / BOARD_COLUMNS;
    // While dragging or resizing, the others make room where the block would land.
    let moving = app
        .dash
        .edit
        .as_ref()
        .filter(|_| edit)
        .and_then(|e| e.moving.clone());
    let mut shown = base.clone();
    if let Some((m, target)) = &moving
        && let Some(i) = shown.iter().position(|(b, _)| b == m)
    {
        let all: Vec<Cell> = shown.iter().map(|x| x.1).collect();
        for (slot, c) in shown.iter_mut().zip(grid::drop_at(&all, i, *target)) {
            slot.1 = c;
        }
    }
    if d.list("blocks").is_empty() {
        ui.secondary(
            "This dashboard has no blocks yet. Add one with + Add block in the dashboard menu.",
        );
    }
    // A Chart block smaller than its legend and a readable drawing need grows to
    // its minimum, moving the blocks below it down (dashboards.md revision 4).
    let dash_id = d.str("id").to_owned();
    let mut grown = false;
    for i in 0..shown.len() {
        let Some((bid, c)) = shown.get(i).cloned() else {
            continue;
        };
        let Some(b) = d
            .list("blocks")
            .iter()
            .find(|x| x.str("id") == bid)
            .cloned()
        else {
            continue;
        };
        let cols = if narrow { BOARD_COLUMNS } else { to_i(c.w) };
        let body_w = cols * colw + (cols - 1) * GAP - 24;
        let data = shown_data(app, &dash_id, &b);
        if let Some(need) = chart_rows(&b, &data, body_w)
            && c.h < need
        {
            let all: Vec<Cell> = shown.iter().map(|x| x.1).collect();
            let target = Cell::new(c.x, c.y, c.w, need);
            for (slot, n) in shown.iter_mut().zip(grid::drop_at(&all, i, target)) {
                slot.1 = n;
            }
            grown = true;
        }
    }
    // While editing, the grown size is the layout Done saves.
    if grown
        && edit
        && moving.is_none()
        && let Some(e) = app.dash.edit.as_mut()
    {
        e.cells.clone_from(&shown);
    }
    // Outside Edit layout, list blocks stop at their content (in whole grid rows)
    // and the board closes up, so the gaps stay even; the saved layout is unchanged.
    if !edit {
        let dash = d.str("id").to_owned();
        let mut fitted: Vec<Cell> = Vec::with_capacity(shown.len());
        for (bid, c) in &shown {
            let b = d
                .list("blocks")
                .iter()
                .find(|x| x.str("id") == bid)
                .cloned();
            let fit = b.and_then(|b| {
                let data = shown_data(app, &dash, &b);
                // The Attention block also draws the backup notices and schedule
                // failures, which are not among its items.
                let extra = if b.str("type") == "attention" {
                    let cols = if narrow { BOARD_COLUMNS } else { to_i(c.w) };
                    attention_extra_lines(app, cols * colw + (cols - 1) * GAP - 24)
                } else {
                    0
                };
                content_rows(&b, &data, extra)
            });
            let mut c = *c;
            if let Some(rows) = fit {
                c.h = c.h.min(rows.max(grid::MIN_H));
            }
            fitted.push(c);
        }
        for (slot, c) in shown.iter_mut().zip(grid::compact(&fitted)) {
            slot.1 = c;
        }
    }
    // Narrow windows: one column in reading order, the saved layout unchanged.
    let mut placed: Vec<Placed> = Vec::new();
    if narrow {
        let only: Vec<Cell> = shown.iter().map(|x| x.1).collect();
        let mut y = 0;
        for i in grid::reading_order(&only) {
            if let Some((b, c)) = shown.get(i) {
                placed.push((b.clone(), (0, y, BOARD_COLUMNS, to_i(c.h)), None));
                y += to_i(c.h);
            }
        }
    } else {
        for (b, c) in &shown {
            match &moving {
                // The moving block keeps its place (the view moves it with the
                // pointer); a ghost marks where it lands.
                Some((m, target)) if m == b => {
                    let orig = base.iter().find(|(x, _)| x == b).map_or(*c, |x| x.1);
                    placed.push((b.clone(), cell_tuple(orig), Some(cell_tuple(*target))));
                }
                _ => placed.push((b.clone(), cell_tuple(*c), None)),
            }
        }
    }
    let all_blocks = d.list("blocks").to_vec();
    let dash = d.str("id").to_owned();
    for (bid, cell, ghost) in placed {
        let Some(b) = all_blocks.iter().find(|x| x.str("id") == bid).cloned() else {
            continue;
        };
        let title = b.str("display_title").to_owned();
        let data = shown_data(app, &dash, &b);
        let link = !edit && opens_something(&b, &data);
        let body_h = body_height(cell.3);
        let body_w = cell.2 * colw + (cell.2 - 1) * GAP - 24;
        let mut menu_pick = None;
        let (resp, ()) = ui.block(&bid, &title, cell, |ui| {
            ui.toolbar(|ui| {
                if b.str("type") == "account_transactions" && data.str("status") == "ok" {
                    let bal = fmt::group(data.str("balance"));
                    ui.named(format!("{} balance {bal}", data.at("account").str("name")));
                    ui.amount(bal);
                }
                let entries = block_entries(edit);
                ui.icon("more")
                    .tip("More")
                    .named(format!("Menu for {title}"));
                menu_pick = ui
                    .menu("", &entries)
                    .chosen
                    .and_then(|i| entries.get(i).cloned());
            });
            body(app, ui, &b, &data, body_w, body_h);
        });
        ui.block_extra(link, ghost);
        if resp.clicked() && link {
            open_block(app, &b, &data);
        }
        if let Some(k) = resp.key.clone() {
            ui.consume();
            block_key(app, d, &bid, &k);
        }
        if let Some(entry) = menu_pick {
            block_menu(app, d, &b, &entry);
        }
    }
}

/// A block's body height in pixels for `rows` grid rows (the title bar and the
/// padding taken).
fn body_height(rows: i32) -> i32 {
    rows * BOARD_ROW + (rows - 1) * GAP - BODY_CHROME
}

/// Grid rows a Chart block needs at `body_w` pixels wide: its drawing at least
/// about two text lines tall after the legend's lines, every bar a caption line
/// (dashboards.md revision 4); `None` for other blocks.
fn chart_rows(b: &Value, data: &Value, body_w: i32) -> Option<u32> {
    if b.str("type") != "chart" || data.str("status") != "ok" {
        return None;
    }
    let names: Vec<&str> = data.list("series").iter().map(|s| s.str("name")).collect();
    let need = match data.str("chart") {
        "hbars" => charts::min_height(data.list("items").len().max(1), &[], body_w),
        "paired" => charts::min_height(0, &names, body_w),
        _ if names.len() > 1 => charts::min_height(0, &names, body_w),
        _ => charts::min_height(0, &[], body_w),
    };
    let rows = (1..=i32::try_from(grid::MAX_H).unwrap_or(12))
        .find(|r| body_height(*r) >= need)
        .unwrap_or(12);
    u32::try_from(rows).ok()
}

/// Lines the Attention block draws besides its items at `body_w` pixels wide: each
/// backup notice's wrapped text and its row of buttons (backups.md: a folder path can
/// make the text long), and each schedule failure.
fn attention_extra_lines(app: &mut App, body_w: i32) -> usize {
    // About 7 px a character in the body text; never fewer than 20 a line.
    let per_line = usize::try_from(body_w / 7).unwrap_or(0).max(20);
    let wrapped = |t: &str| t.chars().count().div_ceil(per_line).max(1);
    let notices: usize = super::backups::notices(app)
        .iter()
        .map(|n| wrapped(&n.text) + usize::from(!n.buttons.is_empty()))
        .sum();
    let failures: usize = app.schedule_failures.iter().map(|f| wrapped(f)).sum();
    notices + failures
}

/// Grid rows a list block needs for its content (title bar, padding and one line
/// per row, plus `extra` lines drawn besides its rows); `None` for blocks drawn to
/// their cell (charts, shortcuts …).
fn content_rows(b: &Value, data: &Value, extra: usize) -> Option<u32> {
    if data.str("status") != "ok" {
        return Some(1);
    }
    let lines = match b.str("type") {
        "balances" => {
            let groups = data.list("groups");
            data.list("accounts").len()
                + groups.len()
                + groups
                    .iter()
                    .map(|g| g.list("accounts").len())
                    .sum::<usize>()
                + usize::from(data.at("missing").as_u64().unwrap_or(0) > 0)
        }
        "attention" => (data.list("items").len() + extra).max(1),
        "scheduled" | "account_transactions" => data.list("rows").len().max(1),
        _ => return None,
    };
    // Title bar 30, padding 24, about 30 px a line.
    let px = 30 + 24 + 30 * i64::try_from(lines).unwrap_or(i64::MAX / 64).min(1_000);
    let (row, gap) = (i64::from(BOARD_ROW), i64::from(GAP));
    let rows = (px + gap + row + gap - 1) / (row + gap);
    u32::try_from(rows.max(1)).ok()
}

/// A key report from a block's title bar: arrows (move, Shift resizes), drags
/// and resizes in progress or ending.
fn block_key(app: &mut App, d: &Value, bid: &str, k: &str) {
    let cur = app
        .dash
        .edit
        .as_ref()
        .and_then(|e| e.cells.iter().find(|(b, _)| b == bid).map(|x| x.1));
    let Some(cur) = cur else {
        return;
    };
    let step = match k {
        "up" => Some(Step::Up),
        "down" => Some(Step::Down),
        "left" => Some(Step::Left),
        "right" => Some(Step::Right),
        "shift+up" => Some(Step::Shorter),
        "shift+down" => Some(Step::Taller),
        "shift+left" => Some(Step::Narrower),
        "shift+right" => Some(Step::Wider),
        _ => None,
    };
    if let Some(s) = step {
        step_block(app, d, bid, s);
        return;
    }
    let (verb, rest) = k.split_once(':').unwrap_or((k, ""));
    let Some((a, b)) = pair(rest) else {
        return;
    };
    let target = match verb {
        "drag" | "drop" => Cell::new(
            a.min(grid::COLUMNS.saturating_sub(cur.w)),
            b.min(grid::MAX_Y),
            cur.w,
            cur.h,
        ),
        _ => Cell::new(cur.x, cur.y, a.max(1), b.max(1)),
    };
    match verb {
        "drag" | "resize" => {
            if let Some(e) = app.dash.edit.as_mut() {
                e.moving = Some((bid.to_owned(), target));
            }
        }
        "drop" | "resized" => drop_block(app, d, bid, target),
        _ => {}
    }
}

fn block_entries(edit: bool) -> Vec<String> {
    let mut v: Vec<String> = ["Settings…", "Refresh", "Duplicate", "Remove"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    if edit {
        v.extend(
            [
                "Move up",
                "Move down",
                "Move left",
                "Move right",
                "Wider",
                "Narrower",
                "Taller",
                "Shorter",
            ]
            .iter()
            .map(|s| (*s).to_owned()),
        );
    }
    v
}

fn block_menu(app: &mut App, d: &Value, b: &Value, entry: &str) {
    let dash = d.str("id").to_owned();
    let bid = b.str("id").to_owned();
    match entry {
        "Settings…" => app.dash.form = Some(form_for(app, &dash, b)),
        "Refresh" => {
            app.dash.data.remove(&(dash, bid));
        }
        "Duplicate" => {
            let mut args =
                json!({"dashboard": dash, "type": b.str("type"), "settings": b.at("settings")});
            if let Some(t) = b.at("title").as_str() {
                set(&mut args, "title", json!(t));
            }
            if let Err(e) = app.run("dashboard.add_block", args) {
                app.dash.error = Some(e.display());
            }
        }
        "Remove" => {
            if let Err(e) = app.run(
                "dashboard.remove_block",
                json!({"dashboard": dash, "block": bid}),
            ) {
                app.dash.error = Some(e.display());
            }
        }
        other => {
            let s = match other {
                "Move up" => Step::Up,
                "Move down" => Step::Down,
                "Move left" => Step::Left,
                "Move right" => Step::Right,
                "Wider" => Step::Wider,
                "Narrower" => Step::Narrower,
                "Taller" => Step::Taller,
                _ => Step::Shorter,
            };
            step_block(app, d, &bid, s);
        }
    }
}

/// Whether clicking the block's title goes somewhere.
fn opens_something(b: &Value, data: &Value) -> bool {
    matches!(
        b.str("type"),
        "account_transactions" | "net_worth" | "report_summary" | "chart"
    ) && data.str("status") == "ok"
}

fn open_block(app: &mut App, b: &Value, data: &Value) {
    if b.str("type") == "account_transactions" {
        let account = data.at("account").str("id").to_owned();
        app.navigate(View::Register { account });
        return;
    }
    let o = data.at("open");
    if !o.str("register").is_empty() {
        let account = o.str("register").to_owned();
        app.navigate(View::Register { account });
    } else if o.str("screen") == "budget" {
        app.navigate(View::Budget);
    } else if !o.str("report").is_empty() {
        super::reports::open(
            app,
            o.str("report"),
            o.str("preset"),
            o.str("from"),
            o.str("to"),
        );
    }
}

fn dec(s: &str) -> Decimal {
    s.parse().unwrap_or_default()
}

fn body(app: &mut App, ui: &mut Ui, b: &Value, data: &Value, width: i32, height: i32) {
    if data.str("status") != "ok" {
        ui.secondary(data.str("message"));
        return;
    }
    match b.str("type") {
        "account_transactions" => account_rows(app, ui, data, width),
        "scheduled" => scheduled_rows(app, ui, data),
        "balances" => balances(app, ui, data),
        "net_worth" => net_worth(ui, data, width, height),
        "chart" => chart(app, ui, b, data, width, height),
        "report_summary" => summary(ui, data),
        "shortcuts" => shortcuts(app, ui, data),
        "attention" => attention(app, ui, data),
        "bills_due" => bills(app, ui, data),
        "subscriptions" => super::subscriptions::block(app, ui, data),
        "projects" => super::projects::block(app, ui, data),
        "note" => ui.label(data.str("text")),
        _ => ui.secondary("This block needs a newer MoneyKeep"),
    }
}

/// A list of clickable rows; returns the index of a clicked row.
fn row_list(ui: &mut Ui, cols: Vec<Column>, rows: &[ListRow]) -> Option<usize> {
    let mut clicked = None;
    ui.table("", cols, |ui| {
        for (i, (name, cells)) in rows.iter().enumerate() {
            let r = ui.table_row(name, false, true, |ui| {
                for (text, mono, tone) in cells {
                    ui.tone(*tone).tip(text.clone());
                    if *mono {
                        ui.amount(text.clone());
                    } else {
                        ui.label(text.clone());
                    }
                }
            });
            if r.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

fn plain(t: impl Into<String>) -> (String, bool, Tone) {
    (t.into(), false, Tone::Normal)
}

fn money(t: impl Into<String>) -> (String, bool, Tone) {
    (t.into(), true, Tone::Normal)
}

fn account_rows(app: &mut App, ui: &mut Ui, data: &Value, width: i32) {
    let rows = data.list("rows").to_vec();
    if rows.is_empty() {
        ui.secondary("No transactions to show.");
        return;
    }
    let wide = width > 420;
    let account = data.at("account").str("id").to_owned();
    let mut cols = vec![
        Column {
            width: 96,
            ..col("")
        },
        Column {
            weight: 2,
            ..col("")
        },
    ];
    if wide {
        cols.push(Column {
            weight: 2,
            ..col("")
        });
    }
    cols.push(Column {
        width: 110,
        ..col_num("")
    });
    let list: Vec<ListRow> = rows
        .iter()
        .map(|r| {
            let date = fmt::date(r.str("date"));
            let payee = r.str("payee").to_owned();
            let amount = fmt::group(r.str("amount"));
            let mut cells = vec![plain(date.clone()), plain(payee.clone())];
            if wide {
                cells.push(plain(r.str("category")));
            }
            cells.push(money(amount.clone()));
            (format!("{date} {payee} {amount}"), cells)
        })
        .collect();
    if let Some(i) = row_list(ui, cols, &list)
        && let Some(r) = rows.get(i)
    {
        app.register.pending_select = Some(r.str("txn").to_owned());
        app.navigate(View::Register { account });
    }
}

fn scheduled_rows(app: &mut App, ui: &mut Ui, data: &Value) {
    let rows = data.list("rows").to_vec();
    if rows.is_empty() {
        ui.label("Nothing scheduled.");
        return;
    }
    let cols = vec![
        Column {
            width: 64,
            ..col("")
        },
        Column {
            width: 120,
            ..col("")
        },
        Column {
            weight: 2,
            ..col("")
        },
        Column {
            weight: 2,
            ..col("")
        },
        Column {
            width: 110,
            ..col_num("")
        },
    ];
    let mut edit = None;
    let mut enter = None;
    ui.table("", cols, |ui| {
        for (i, o) in rows.iter().enumerate() {
            let due = fmt::date(o.str("due"));
            let payee = o.str("shown_payee").to_owned();
            let amount = fmt::group(o.str("shown_amount"));
            let overdue = o.at("overdue").as_bool() == Some(true);
            let label = format!(
                "Scheduled {payee} due {due}, {amount}{}",
                if overdue { ", overdue" } else { "" }
            );
            let r = ui.table_row(&label, false, true, |ui| {
                ui.named(format!("Enter {payee} due {due}"));
                if ui.button("Enter").clicked() {
                    enter = Some(i);
                }
                let when = if overdue {
                    format!("Overdue {due}")
                } else {
                    due.clone()
                };
                if overdue {
                    ui.tone(Tone::Decrease);
                }
                ui.label(when);
                ui.tip(payee.clone());
                ui.label(payee.clone());
                ui.tip(o.str("shown_account"));
                ui.label(o.str("shown_account"));
                ui.amount(amount.clone());
            });
            if r.clicked() {
                edit = Some(i);
            }
        }
    });
    if let Some(o) = enter.and_then(|i| rows.get(i)) {
        super::scheduled::enter(app, o);
    } else if let Some(o) = edit.and_then(|i| rows.get(i)) {
        super::scheduled::edit(app, o.str("schedule"));
    }
}

fn balances(app: &mut App, ui: &mut Ui, data: &Value) {
    let cols = vec![
        Column {
            weight: 3,
            ..col("")
        },
        Column {
            width: 130,
            ..col_num("")
        },
    ];
    let mut go_to = None;
    ui.table("", cols, |ui| {
        let mut line = |ui: &mut Ui, a: &Value, indent: i32| {
            let full = a.str("full_name").to_owned();
            let bal = fmt::group(a.str("balance"));
            ui.table_row(&format!("{full}#balance"), false, false, |ui| {
                ui.named(format!("{full} register")).tip(full.clone());
                if ui.link(a.str("name")).clicked() {
                    go_to = Some(a.str("id").to_owned());
                }
                ui.named(format!("{full} balance {bal}"));
                ui.amount(bal.clone());
            });
            ui.indent_last(indent);
        };
        for a in data.list("accounts") {
            line(ui, a, 0);
        }
        for g in data.list("groups") {
            let name = g.str("name").to_owned();
            ui.table_row(&format!("{name}#group"), false, false, |ui| {
                ui.bold().named(format!("{name} group"));
                ui.label(name.clone());
                if let Some(t) = g.at("total").as_str() {
                    let t = fmt::group(t);
                    ui.bold().named(format!("{name} balance {t}"));
                    ui.amount(t);
                }
            });
            for a in g.list("accounts") {
                line(ui, a, 1);
            }
        }
    });
    if data.at("missing").as_u64().unwrap_or(0) > 0 {
        ui.secondary("Some chosen accounts no longer exist.");
    }
    if let Some(account) = go_to {
        app.navigate(View::Register { account });
    }
}

fn net_worth(ui: &mut Ui, data: &Value, width: i32, height: i32) {
    let cur = data.str("currency");
    ui.heading(format!(
        "Net worth {} {cur}",
        fmt::group(data.str("net_worth"))
    ));
    ui.label(format!(
        "Assets {}  ·  Liabilities {}",
        fmt::group(data.str("assets")),
        fmt::group(data.str("liabilities"))
    ));
    let change = data.str("change");
    let sign = if fmt::is_negative(change) || fmt::is_zero(change) {
        ""
    } else {
        "+"
    };
    ui.secondary(format!("Change this month {sign}{}", fmt::group(change)));
    let values: Vec<Decimal> = data
        .list("sparkline")
        .iter()
        .map(|v| dec(v.as_str().unwrap_or("0")))
        .collect();
    if values.len() > 1 {
        let desc = format!(
            "Net worth, last 12 months: {}",
            values
                .iter()
                .map(|v| fmt::group(&v.to_string()))
                .collect::<Vec<_>>()
                .join(", ")
        );
        // The card's full width with month labels and a value axis, drawn like the
        // reports' line charts (review of ui-slint.md revision 2).
        let dates: Vec<String> = data
            .list("dates")
            .iter()
            // Month ends (the last is today): labelled by month.
            .map(|d| d.as_str().unwrap_or_default().chars().take(7).collect())
            .collect();
        if dates.len() == values.len() {
            // Below the three text lines (about 80 px).
            let h = charts::Fit::Total((height - 84).clamp(96, 240));
            charts::lines_sized(
                ui,
                width,
                h,
                &dates,
                &[("Net worth", values)],
                &desc,
                &charts::Opens::default(),
            );
        } else {
            charts::sparkline(ui, width, 48, &values, &desc);
        }
    }
}

/// A Chart block's chart: the whole body height is the chart's, legend included
/// (dashboards.md revision 4: the drawing takes what the legend leaves).
fn chart(app: &mut App, ui: &mut Ui, b: &Value, data: &Value, width: i32, height: i32) {
    let h = charts::Fit::Total(height);
    // In Edit layout nothing in a block opens (dashboards.md revision 5).
    let live = app.dash.edit.is_none();
    let opens = charts::Opens {
        pieces: live,
        more: live,
        period: period_caption(data.str("from"), data.str("to")),
    };
    let strs = |v: &Value| -> Vec<Decimal> {
        v.as_array()
            .map(|a| a.iter().map(|x| dec(x.as_str().unwrap_or("0"))).collect())
            .unwrap_or_default()
    };
    let labels: Vec<String> = data
        .list("labels")
        .iter()
        .map(|l| {
            let s = l.as_str().unwrap_or_default();
            if s.len() == 10 {
                fmt::date(s)
            } else {
                s.to_owned()
            }
        })
        .collect();
    let series = data.list("series");
    match data.str("chart") {
        "hbars" => {
            let items: Vec<(String, Decimal)> = data
                .list("items")
                .iter()
                .map(|i| (i.str("label").to_owned(), dec(i.str("amount"))))
                .collect();
            if items.is_empty() {
                ui.secondary("Nothing spent in this period.");
                return;
            }
            let desc = format!(
                "Spending chart: {}",
                items
                    .iter()
                    .map(|(n, v)| format!("{n} {}", fmt::group(&v.to_string())))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if let Some(hit) = charts::hbars_sized(ui, width, h, &items, &desc, &opens) {
                let piece = match hit {
                    ChartHit::Piece(i) => items.get(i).map(|x| x.0.clone()),
                    _ => None,
                };
                match piece {
                    // A category's register for the block's period; "Other" (the
                    // grouped rest) opens the report.
                    Some(cat) if cat != "Other" => {
                        open_register(app, &cat, data.str("from"), data.str("to"));
                    }
                    _ => open_block(app, b, data),
                }
            }
        }
        "paired" => {
            let get = |k: usize| {
                series
                    .get(k)
                    .map(|s| strs(s.at("values")))
                    .unwrap_or_default()
            };
            let name = |k: usize| {
                series
                    .get(k)
                    .map(|s| s.str("name").to_owned())
                    .unwrap_or_default()
            };
            let (a, bb, c) = (get(0), get(1), get(2));
            let (na, nb, nc) = (name(0), name(1), name(2));
            let desc = format!(
                "Chart: {}",
                labels
                    .iter()
                    .enumerate()
                    .map(|(i, l)| format!(
                        "{l} {na} {} {nb} {}",
                        fmt::group(&a.get(i).copied().unwrap_or_default().to_string()),
                        fmt::group(&bb.get(i).copied().unwrap_or_default().to_string())
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            let hit = charts::paired_sized(
                ui,
                width,
                h,
                &labels,
                [(&na, &a), (&nb, &bb), (&nc, &c)],
                &desc,
                &opens,
            );
            if let Some(hit) = hit {
                paired_click(app, b, data, &labels, hit);
            }
        }
        _ => {
            let lines: Vec<(String, Vec<Decimal>)> = series
                .iter()
                .map(|s| (s.str("name").to_owned(), strs(s.at("values"))))
                .collect();
            let refs: Vec<(&str, Vec<Decimal>)> =
                lines.iter().map(|(n, v)| (n.as_str(), v.clone())).collect();
            let desc = format!(
                "Chart: {}",
                lines
                    .iter()
                    .map(|(n, v)| format!(
                        "{n} ending at {}",
                        fmt::group(&v.last().copied().unwrap_or_default().to_string())
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            if let Some(hit) = charts::lines_sized(ui, width, h, &labels, &refs, &desc, &opens) {
                lines_click(app, b, data, hit);
            }
        }
    }
}

/// A period in words for tooltips: a whole month as "Oct 2026", else its dates.
pub(crate) fn period_caption(from: &str, to: &str) -> String {
    if from.is_empty() || to.is_empty() {
        return String::new();
    }
    let month = from.get(..7);
    let whole = from.ends_with("-01")
        && month == to.get(..7)
        && to
            .parse::<mk_core::Date>()
            .ok()
            .and_then(mk_core::Date::next_day)
            .is_some_and(|n| n.day() == 1);
    match month {
        Some(m) if whole => charts::period_name(m),
        _ => format!("{} – {}", fmt::date(from), fmt::date(to)),
    }
}

/// A category's (or account's) register showing `from`–`to` (ISO dates; either
/// may be empty).
pub(crate) fn open_register(app: &mut App, account: &str, from: &str, to: &str) {
    let show = |d: &str| {
        if d.is_empty() {
            String::new()
        } else {
            fmt::date(d)
        }
    };
    app.register.from = show(from);
    app.register.to = show(to);
    app.register.range_for = Some(account.to_owned());
    app.navigate(View::Register {
        account: account.to_owned(),
    });
}

/// A period's dates from the block data's `periods`.
fn span(data: &Value, i: usize) -> Option<(String, String)> {
    data.list("periods")
        .get(i)
        .map(|p| (p.str("from").to_owned(), p.str("to").to_owned()))
}

/// Paired bars: income vs expenses (a month's bar opens Income & expenses for that
/// month) and budget vs actual (a category's bar opens its register for the
/// month; the budget bar opens the Budget screen).
fn paired_click(app: &mut App, b: &Value, data: &Value, labels: &[String], hit: ChartHit) {
    let ChartHit::Piece(p) = hit else {
        // The legend's Budget opens the Budget screen; the rest the report.
        if hit == ChartHit::Legend(0) && b.at("settings").str("chart") == "budget" {
            app.navigate(View::Budget);
        } else {
            open_block(app, b, data);
        }
        return;
    };
    let (i, k) = (p / 2, p % 2);
    if b.at("settings").str("chart") == "budget" {
        if k == 0 {
            app.navigate(View::Budget);
        } else if let Some(cat) = labels.get(i) {
            let from = format!("{}-01", data.str("month"));
            open_register(app, cat, &from, "");
        }
        return;
    }
    match span(data, i) {
        Some((from, to)) => super::reports::open(app, "income_expense", "custom", &from, &to),
        None => open_block(app, b, data),
    }
}

/// Lines: net worth (a month opens the Net worth report as of its end), an
/// account's balance (its register at the last transaction on or before the date)
/// and portfolio value (Holdings as of the date).
fn lines_click(app: &mut App, b: &Value, data: &Value, hit: ChartHit) {
    let ChartHit::Piece(i) = hit else {
        open_block(app, b, data);
        return;
    };
    match b.at("settings").str("chart") {
        "net_worth" => match span(data, i) {
            Some((_, to)) => {
                super::reports::open(app, "net_worth", "custom", data.str("from"), &to);
            }
            None => open_block(app, b, data),
        },
        "account_balance" => {
            let account = data.at("open").str("register").to_owned();
            let Some((_, to)) = span(data, i) else {
                return open_block(app, b, data);
            };
            let last = app
                .read(
                    "account.register",
                    json!({"account": account, "to": to, "order": "desc", "limit": 1, "offset": 0}),
                )
                .and_then(|r| r.list("rows").first().map(|x| x.str("txn").to_owned()))
                .filter(|t| !t.is_empty());
            app.register.from.clear();
            app.register.to.clear();
            app.register.range_for = None;
            app.register.pending_select = last;
            app.navigate(View::Register { account });
        }
        "portfolio" => {
            let date = data
                .list("labels")
                .get(i)
                .and_then(|l| l.as_str())
                .unwrap_or_default()
                .to_owned();
            super::reports::open(app, "holdings", "custom", "", &date);
        }
        _ => open_block(app, b, data),
    }
}

fn summary(ui: &mut Ui, data: &Value) {
    let label = data.str("label");
    let figure = fmt::group(data.str("figure"));
    ui.mono().named(format!("{label} {figure}"));
    ui.big_heading(figure);
    ui.secondary(format!(
        "Previous period {} (change {})",
        fmt::group(data.str("previous")),
        fmt::group(data.str("change"))
    ));
}

/// Every symbol a dashboard or shortcut button can show: the book-profile symbols
/// and those dashboards add (dashboards.md revision 3).
pub(crate) fn icon_symbols() -> Vec<&'static str> {
    crate::base::recent::SYMBOLS
        .iter()
        .copied()
        .chain([
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
        ])
        .collect()
}

/// An icon field: a menu (named "`label`: shown") listing every symbol; with
/// `default`, a first choice leaves it to the target.
fn icon_picker(ui: &mut Ui, label: &str, value: &mut String, default: bool) {
    let none = if default {
        "Default for the target"
    } else {
        "No icon"
    };
    let shown = if value.is_empty() {
        if default { "Default" } else { "None" }.to_owned()
    } else {
        value.clone()
    };
    let mut entries = vec![none.to_owned()];
    entries.extend(icon_symbols().iter().map(|s| format!("Icon {s}")));
    ui.form(label, |ui| {
        if !value.is_empty() {
            ui.icon(&format!("sym-{value}"));
        }
        ui.named(format!("{label}: {shown}"));
        if let Some(i) = ui
            .menu(if value.is_empty() { &shown } else { "" }, &entries)
            .chosen
        {
            *value = if i == 0 {
                String::new()
            } else {
                icon_symbols()
                    .get(i - 1)
                    .map(|s| (*s).to_owned())
                    .unwrap_or_default()
            };
        }
    });
}

fn shortcuts(app: &mut App, ui: &mut Ui, data: &Value) {
    let buttons = data.list("buttons").to_vec();
    let size = match data.str("button_size") {
        "" => "medium",
        x => x,
    };
    let mut clicked = None;
    ui.wrap(|ui| {
        for (i, b) in buttons.iter().enumerate() {
            let label = b.str("label").to_owned();
            let missing = b.at("missing").as_bool() == Some(true);
            ui.enabled(!missing);
            if missing {
                ui.tip("Choose an account or dashboard for this button in Settings…");
            }
            let sym = b.str("icon");
            if !sym.is_empty() {
                ui.icon(&format!("sym-{sym}"));
            }
            let r = ui.button(label);
            // Heights in `number` (pixels): Small is a plain button, Medium a
            // taller one, Large a square-ish tile.
            if let Some(it) = ui.last() {
                match size {
                    "large" => {
                        it.width = 160;
                        it.number = 112;
                    }
                    "medium" => {
                        it.width = 128;
                        it.number = 44;
                    }
                    _ => {}
                }
            }
            if r.clicked() {
                clicked = Some(i);
            }
        }
    });
    if let Some(b) = clicked.and_then(|i| buttons.get(i)) {
        go(app, b.at("target"));
    }
}

fn attention(app: &mut App, ui: &mut Ui, data: &Value) {
    let failures = app.schedule_failures.clone();
    let items = data.list("items").to_vec();
    // backups.md: the backup notices come first.
    let backups = super::backups::attention(app, ui);
    if items.is_empty() && failures.is_empty() && !backups {
        ui.label("Nothing needs your attention.");
    }
    for f in &failures {
        ui.error_text(f.clone());
    }
    let mut target = None;
    let mut dismiss = None;
    let mut action: Option<Value> = None;
    for (i, it) in items.iter().enumerate() {
        ui.scope(format!("attention-{i}"), |ui| {
            // subscriptions.md: a coloured chip (the band colour, or red for a
            // charge after cancelling) on the line above the notice.
            let chip = it.at("chip");
            if chip.is_object() {
                let color = mk_core::subscription::parse_hex(chip.str("color")).unwrap_or(0xD64545);
                ui.row(|ui| {
                    ui.named(chip.str("text").to_owned());
                    ui.color_badge(
                        chip.str("text"),
                        color,
                        chip.at("dark_text").as_bool() != Some(false),
                    );
                });
            }
            ui.row(|ui| {
                if ui.link(it.str("label")).clicked() {
                    target = Some(it.at("target").clone());
                }
                for a in it.list("actions") {
                    ui.named(format!("{}: {}", a.str("label"), it.str("label")));
                    if ui.button(a.str("label")).clicked() {
                        action = Some(a.clone());
                    }
                }
                // app-desktop revision 38: dismissible notices end with ×.
                if it.at("dismiss").is_object() {
                    ui.spacer();
                    ui.tip("Dismiss");
                    if ui.icon_button("close", "Dismiss", false).clicked() {
                        dismiss = Some(it.at("dismiss").clone());
                    }
                }
            });
        });
    }
    if let Some(a) = action {
        match app.run(a.str("command"), a.at("args").clone()) {
            Ok(_) => app.notify_with(
                format!("{}: done.", a.str("label")),
                super::MessageAction::Undo,
            ),
            Err(e) => app.notify_error(&e),
        }
    } else if let Some(d) = dismiss {
        dismiss_notice(app, d);
    } else if let Some(t) = target {
        go(app, &t);
    }
}

/// Dismiss a provider notice (`connect.dismiss`, connectivity.md revision 5): the
/// line goes at once and the message offers Undo.
pub(crate) fn dismiss_notice(app: &mut App, args: Value) {
    // Subscription alerts name their own command (subscriptions.md).
    let (command, args) = match args.at("command").as_str() {
        Some(c) => {
            let mut a = args.clone();
            if let Some(o) = a.as_object_mut() {
                o.remove("command");
            }
            (c.to_owned(), a)
        }
        None => ("connect.dismiss".to_owned(), args),
    };
    match app.run(&command, args) {
        Ok(_) => app.notify_with("Notice dismissed.", super::MessageAction::Undo),
        Err(e) => app.notify_error(&e),
    }
}

fn bills(app: &mut App, ui: &mut Ui, data: &Value) {
    let rows = data.list("rows").to_vec();
    if rows.is_empty() {
        ui.label("Nothing due.");
        return;
    }
    let cols = vec![
        Column {
            width: 140,
            ..col("")
        },
        Column {
            weight: 2,
            ..col("")
        },
        Column {
            weight: 2,
            ..col("")
        },
        Column {
            width: 110,
            ..col_num("")
        },
    ];
    let list: Vec<ListRow> = rows
        .iter()
        .map(|r| {
            let due = fmt::date(r.str("due"));
            let overdue = r.at("overdue").as_bool() == Some(true);
            let when = if overdue {
                format!("Overdue {due}")
            } else {
                due.clone()
            };
            let amount = fmt::group(r.str("amount"));
            let what = if r.str("kind") == "bill" {
                "Bill"
            } else {
                "Invoice"
            };
            let name = format!(
                "{what} {} {} due {due}, {amount}",
                r.str("number"),
                r.str("name")
            );
            (
                name,
                vec![
                    (
                        when,
                        false,
                        if overdue {
                            Tone::Decrease
                        } else {
                            Tone::Normal
                        },
                    ),
                    plain(r.str("name")),
                    plain(format!("{what} {}", r.str("number"))),
                    money(amount),
                ],
            )
        })
        .collect();
    if let Some(r) = row_list(ui, cols, &list).and_then(|i| rows.get(i)) {
        app.navigate(if r.str("kind") == "bill" {
            View::Purchases
        } else {
            View::Sales
        });
    }
}

// ----- the Dashboards list (rail pane) -------------------------------------------------

/// The Dashboards list; returns whether a dashboard was opened.
pub(crate) fn list_pane(app: &mut App, ui: &mut Ui) -> bool {
    let mut opened = false;
    ui.row(|ui| {
        if ui.button("+ New dashboard").clicked() {
            app.dash.naming = Some((Naming::New, String::new(), None));
        }
    });
    let current = current_id(app);
    let on_dashboard = matches!(app.view, View::Home | View::Dashboard { .. });
    for d in list(app) {
        let id = d.str("id").to_owned();
        let name = d.str("name").to_owned();
        let home = d.at("home").as_bool() == Some(true);
        let selected = on_dashboard && id == current;
        let mut pick = None;
        ui.next().id(format!("dashboard:{id}"));
        let r = ui.list_item(&format!("Dashboard {name}"), selected, |ui| {
            match d.at("icon").as_str() {
                Some(sym) => ui.icon_image(&format!("sym-{sym}")),
                None => ui.icon_image("tiles"),
            }
            ui.next().grow();
            ui.quiet().label(name.clone());
            if d.at("startup").as_bool() == Some(true) {
                ui.named("Startup dashboard")
                    .tip("Opens when the book opens");
                ui.icon_image("star-filled");
            }
            let entries: Vec<String> = dashboard_entries(home, true)
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
            ui.icon("more")
                .tip("More")
                .named(format!("Menu for dashboard {name}"));
            pick = ui
                .menu("", &entries)
                .chosen
                .and_then(|i| entries.get(i).cloned());
        });
        if r.clicked() {
            open(app, &id);
            opened = true;
        }
        if let Some(entry) = pick {
            if entry == "Open" {
                opened = true;
            }
            dashboard_action(app, &entry, &id, &name);
        }
    }
    opened
}

// ----- dialogs ---------------------------------------------------------------------------

pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    naming_dialog(app, ui);
    delete_dialog(app, ui);
    picker(app, ui);
    form_dialog(app, ui);
}

fn naming_dialog(app: &mut App, ui: &mut Ui) {
    let Some((kind, mut name, mut error)) = app.dash.naming.clone() else {
        return;
    };
    let mut icon = app.dash.naming_icon.clone();
    let mut close = ui.dialog_closed("dashboard-name");
    let title = match kind {
        Naming::New => "New dashboard",
        Naming::Rename(_) => "Rename dashboard",
        Naming::Duplicate(_) => "Duplicate dashboard",
    };
    let mut go_ahead = false;
    ui.dialog("dashboard-name", title, 440, |ui| {
        ui.form("Dashboard name", |ui| {
            ui.width(260);
            if ui.field("Dashboard name", &mut name).submitted {
                go_ahead = true;
            }
        });
        if matches!(kind, Naming::Rename(_)) {
            icon_picker(ui, "Dashboard icon", &mut icon, false);
        }
        if let Some(e) = &error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                close = true;
            }
            let label = if matches!(kind, Naming::Rename(_)) {
                "Save"
            } else {
                "Create"
            };
            if ui.primary(label).clicked() {
                go_ahead = true;
            }
        });
    });
    if go_ahead && !close {
        let r = match &kind {
            Naming::New => app.run("dashboard.create", json!({"name": name})),
            Naming::Rename(id) => app.run(
                "dashboard.update",
                json!({"dashboard": id, "name": name, "icon": icon}),
            ),
            Naming::Duplicate(id) => app.run("dashboard.create", json!({"name": name, "from": id})),
        };
        match r {
            Ok(v) => {
                close = true;
                if !matches!(kind, Naming::Rename(_)) {
                    let id = v.str("id").to_owned();
                    open(app, &id);
                }
            }
            Err(e) => error = Some(e.display()),
        }
    }
    app.dash.naming_icon = icon;
    app.dash.naming = if close {
        None
    } else {
        Some((kind, name, error))
    };
}

fn delete_dialog(app: &mut App, ui: &mut Ui) {
    let Some((id, name)) = app.dash.confirm_delete.clone() else {
        return;
    };
    let mut close = ui.dialog_closed("dashboard-delete");
    ui.dialog("dashboard-delete", "Delete dashboard", 420, |ui| {
        ui.label(format!("Delete the dashboard {name}? You can undo this."));
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                close = true;
            }
            if ui.primary("Delete").clicked() {
                match app.run("dashboard.delete", json!({"dashboard": id})) {
                    Ok(_) => {
                        if matches!(&app.view, View::Dashboard { id: v } if *v == id) {
                            app.navigate(View::Home);
                        }
                    }
                    Err(e) => app.dash.error = Some(e.display()),
                }
                close = true;
            }
        });
    });
    if close {
        app.dash.confirm_delete = None;
    }
}

/// + Add block: the block types with one-line descriptions.
fn picker(app: &mut App, ui: &mut Ui) {
    let Some(dash) = app.dash.picker.clone() else {
        return;
    };
    let mut close = ui.dialog_closed("add-block");
    ui.dialog("add-block", "Add block", 560, |ui| {
        let cols = vec![
            Column {
                width: 170,
                ..col("")
            },
            Column {
                weight: 3,
                ..col("")
            },
        ];
        ui.table("", cols, |ui| {
            for t in grid::BLOCK_TYPES {
                ui.table_row(t.name, false, false, |ui| {
                    if ui.button(t.title).clicked() {
                        close = true;
                        if matches!(
                            t.name,
                            "net_worth" | "attention" | "subscriptions" | "projects"
                        ) {
                            if let Err(e) = app.run(
                                "dashboard.add_block",
                                json!({"dashboard": dash, "type": t.name}),
                            ) {
                                app.dash.error = Some(e.display());
                            }
                        } else {
                            app.dash.form = Some(new_form(&dash, t.name));
                        }
                    }
                    ui.secondary(t.description);
                });
            }
        });
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
    });
    if close {
        app.dash.picker = None;
    }
}

fn new_form(dash: &str, kind: &str) -> BlockForm {
    let mut f = BlockForm {
        dashboard: dash.to_owned(),
        kind: kind.to_owned(),
        mode: if kind == "scheduled" {
            "next".into()
        } else {
            "recent".into()
        },
        n: "5".into(),
        direction: "either".into(),
        days: if kind == "bills_due" {
            "14".into()
        } else {
            "30".into()
        },
        source: "groups".into(),
        chart: "spending".into(),
        period: "this_month".into(),
        report: "profit_loss".into(),
        ..BlockForm::default()
    };
    if kind == "shortcuts" {
        f.buttons.push(ButtonForm {
            kind: "screen".into(),
            screen: "reports".into(),
            label: "Reports".into(),
            ..ButtonForm::default()
        });
    }
    f
}

/// The accounts list (id, full name).
fn accounts(app: &mut App) -> Vec<(String, String)> {
    array(app.backend.invoke("account.list", json!({})).ok())
        .iter()
        .filter(|a| a.at("placeholder").as_bool() != Some(true))
        .map(|a| (a.str("id").to_owned(), a.str("full_name").to_owned()))
        .collect()
}

fn form_for(app: &mut App, dash: &str, b: &Value) -> BlockForm {
    let accts = accounts(app);
    let name = |id: &str| {
        accts
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, n)| n.clone())
            .unwrap_or_default()
    };
    let st = b.at("settings");
    let num = |k: &str, d: &str| {
        st.at(k)
            .as_u64()
            .map_or_else(|| d.to_owned(), |n| n.to_string())
    };
    let mut f = new_form(dash, b.str("type"));
    f.block = Some(b.str("id").to_owned());
    f.title = b.at("title").as_str().unwrap_or_default().to_owned();
    f.account = name(st.str("account"));
    if !st.str("mode").is_empty() {
        f.mode = st.str("mode").to_owned();
    }
    f.n = num("n", "5");
    f.days = num("days", &f.days);
    f.amount = st.str("amount").to_owned();
    if !st.str("direction").is_empty() {
        f.direction = st.str("direction").to_owned();
    }
    f.include_children = st.at("include_children").as_bool() == Some(true);
    if !st.str("source").is_empty() {
        f.source = st.str("source").to_owned();
    }
    f.accounts = st
        .list("accounts")
        .iter()
        .map(|x| name(x.as_str().unwrap_or_default()))
        .collect();
    f.groups = st
        .list("groups")
        .iter()
        .filter_map(|x| x.as_str().map(str::to_owned))
        .collect();
    if !st.str("chart").is_empty() {
        f.chart = st.str("chart").to_owned();
    }
    if !st.str("period").is_empty() {
        f.period = st.str("period").to_owned();
    }
    f.from = st.at("from").as_str().map(fmt::date).unwrap_or_default();
    f.to = st.at("to").as_str().map(fmt::date).unwrap_or_default();
    if !st.str("report").is_empty() {
        f.report = st.str("report").to_owned();
    }
    f.text = st.str("text").to_owned();
    if b.str("type") == "shortcuts" {
        let dashes = list(app);
        f.buttons = st
            .list("buttons")
            .iter()
            .map(|x| {
                let t = x.at("target");
                ButtonForm {
                    label: x.str("label").to_owned(),
                    icon: x.str("icon").to_owned(),
                    kind: t.str("kind").to_owned(),
                    account: name(t.str("account")),
                    report: t.str("report").to_owned(),
                    screen: t.str("screen").to_owned(),
                    dashboard: dashes
                        .iter()
                        .find(|d| d.str("id") == t.str("dashboard"))
                        .map(|d| d.str("name").to_owned())
                        .unwrap_or_default(),
                    action: t.str("action").to_owned(),
                }
            })
            .collect();
        f.button_size = st.str("button_size").to_owned();
    }
    f
}

/// A combo over (value, title) choices.
fn combo(ui: &mut Ui, label: &str, value: &mut String, choices: &[(&str, &str)]) {
    let titles: Vec<String> = choices.iter().map(|(_, t)| (*t).to_owned()).collect();
    let mut idx = choices
        .iter()
        .position(|(v, _)| v == value)
        .unwrap_or(usize::MAX);
    ui.form(label, |ui| {
        ui.width(240);
        if ui.combo(label, &mut idx, &titles).changed
            && let Some((v, _)) = choices.get(idx)
        {
            *value = (*v).to_owned();
        }
    });
}

fn account_combo(ui: &mut Ui, label: &str, value: &mut String, accts: &[(String, String)]) {
    let names: Vec<String> = accts.iter().map(|(_, n)| n.clone()).collect();
    ui.form(label, |ui| {
        ui.width(260).hint("Choose an account");
        ui.combo_str(label, value, &names);
    });
}

fn text_row(ui: &mut Ui, label: &str, value: &mut String) {
    ui.form(label, |ui| {
        ui.width(200);
        ui.field(label, value);
    });
}

const PERIODS: [(&str, &str); 6] = [
    ("this_month", "This month"),
    ("last_month", "Last month"),
    ("this_quarter", "This quarter"),
    ("this_year", "This year"),
    ("last_12_months", "Last 12 months"),
    ("custom", "Custom…"),
];

fn form_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.dash.form.clone() else {
        return;
    };
    let accts = accounts(app);
    let dashes: Vec<String> = names(app);
    let mut close = ui.dialog_closed("block-settings");
    let mut save = false;
    let title = grid::block_type(&f.kind).map_or("Block", |t| t.title);
    ui.dialog("block-settings", &format!("{title} settings"), 560, |ui| {
        ui.group(|ui| {
            text_row(ui, "Block title (optional)", &mut f.title);
            match f.kind.as_str() {
                "account_transactions" => {
                    account_combo(ui, "Account", &mut f.account, &accts);
                    combo(
                        ui,
                        "Show",
                        &mut f.mode,
                        &[("recent", "Most recent"), ("above", "Above an amount")],
                    );
                    text_row(ui, "How many", &mut f.n);
                    if f.mode == "above" {
                        text_row(ui, "Amount at least", &mut f.amount);
                        combo(
                            ui,
                            "Direction",
                            &mut f.direction,
                            &[
                                ("either", "Either direction"),
                                ("in", "Money in"),
                                ("out", "Money out"),
                            ],
                        );
                        text_row(ui, "Within the last days", &mut f.days);
                    }
                    ui.form("", |ui| {
                        ui.checkbox(&mut f.include_children, "Include sub-accounts");
                    });
                }
                "scheduled" => {
                    combo(
                        ui,
                        "Show",
                        &mut f.mode,
                        &[("next", "The next few"), ("days", "The next days")],
                    );
                    if f.mode == "next" {
                        text_row(ui, "How many", &mut f.n);
                    } else {
                        text_row(ui, "Days ahead", &mut f.days);
                    }
                    ui.label("Accounts (none ticked: all)");
                    checklist(ui, &mut f.accounts, accts.iter().map(|(_, n)| n.as_str()));
                }
                "balances" => {
                    combo(
                        ui,
                        "Show",
                        &mut f.source,
                        &[
                            ("groups", "Account groups"),
                            ("accounts", "Chosen accounts"),
                        ],
                    );
                    if f.source == "groups" {
                        ui.label("Groups (none ticked: all but categories)");
                        checklist(
                            ui,
                            &mut f.groups,
                            crate::base::accounts::GROUPS.iter().copied(),
                        );
                    } else {
                        checklist(ui, &mut f.accounts, accts.iter().map(|(_, n)| n.as_str()));
                    }
                }
                "chart" => {
                    combo(
                        ui,
                        "Chart",
                        &mut f.chart,
                        &[
                            ("spending", "Spending by category"),
                            ("income_expense", "Income vs expenses"),
                            ("net_worth", "Net worth over time"),
                            ("account_balance", "Account balance over time"),
                            ("budget", "Budget vs actual"),
                            ("portfolio", "Portfolio value"),
                        ],
                    );
                    if f.chart == "account_balance" {
                        account_combo(ui, "Account", &mut f.account, &accts);
                    }
                    period_fields(ui, &mut f);
                }
                "report_summary" => {
                    combo(
                        ui,
                        "Report",
                        &mut f.report,
                        &[
                            ("profit_loss", "Profit and loss: net income"),
                            ("cash_flow", "Cash flow: net change"),
                            ("ar_total", "A/R total"),
                            ("ap_total", "A/P total"),
                            ("sales_tax_due", "Sales tax due"),
                            ("1099_total", "1099 total"),
                        ],
                    );
                    period_fields(ui, &mut f);
                }
                "shortcuts" => shortcut_fields(ui, &mut f, &accts, &dashes),
                "bills_due" => text_row(ui, "Days ahead", &mut f.days),
                "note" => {
                    ui.row(|ui| {
                        ui.grow();
                        ui.multiline("Note text", &mut f.text);
                    });
                }
                _ => {}
            }
        });
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                close = true;
            }
            if ui.primary("Save").clicked() {
                save = true;
            }
        });
    });
    if save && !close {
        match save_form(app, &f) {
            Ok(()) => close = true,
            Err(e) => f.error = Some(e),
        }
    }
    app.dash.form = if close { None } else { Some(f) };
}

fn checklist<'a>(ui: &mut Ui, set: &mut BTreeSet<String>, items: impl Iterator<Item = &'a str>) {
    ui.wrap(|ui| {
        for n in items {
            let mut on = set.contains(n);
            if ui.checkbox(&mut on, n).changed {
                if on {
                    set.insert(n.to_owned());
                } else {
                    set.remove(n);
                }
            }
        }
    });
}

fn period_fields(ui: &mut Ui, f: &mut BlockForm) {
    combo(ui, "Period", &mut f.period, &PERIODS);
    if f.period == "custom" {
        ui.form("From", |ui| {
            super::register::date_field(ui, "From", &mut f.from)
        });
        ui.form("To", |ui| super::register::date_field(ui, "To", &mut f.to));
    }
}

fn shortcut_fields(ui: &mut Ui, f: &mut BlockForm, accts: &[(String, String)], dashes: &[String]) {
    if f.button_size.is_empty() {
        f.button_size = "medium".into();
    }
    combo(
        ui,
        "Button size",
        &mut f.button_size,
        &[("small", "Small"), ("medium", "Medium"), ("large", "Large")],
    );
    let mut remove = None;
    let n = f.buttons.len();
    for (i, b) in f.buttons.iter_mut().enumerate() {
        let k = i + 1;
        ui.separator();
        ui.scope(format!("button{i}"), |ui| {
            text_row(ui, &format!("Button {k} label"), &mut b.label);
            icon_picker(ui, &format!("Button {k} icon"), &mut b.icon, true);
            combo(
                ui,
                &format!("Button {k} goes to"),
                &mut b.kind,
                &[
                    ("account", "An account register"),
                    ("report", "A report"),
                    ("screen", "A screen"),
                    ("dashboard", "A dashboard"),
                    ("action", "An action"),
                ],
            );
            match b.kind.as_str() {
                "account" => {
                    account_combo(ui, &format!("Button {k} account"), &mut b.account, accts)
                }
                "report" => {
                    let reports: Vec<(&str, &str)> = super::reports::REPORTS.to_vec();
                    combo(ui, &format!("Button {k} report"), &mut b.report, &reports);
                }
                "screen" => combo(
                    ui,
                    &format!("Button {k} screen"),
                    &mut b.screen,
                    &[
                        ("home", "Home"),
                        ("scheduled", "Scheduled"),
                        ("reports", "Reports"),
                        ("accounts", "Accounts"),
                        ("expenses", "Expenses"),
                        ("sales", "Sales"),
                        ("purchases", "Purchases"),
                        ("budget", "Budget"),
                        ("securities", "Securities"),
                        ("settings", "Settings"),
                    ],
                ),
                "dashboard" => {
                    let d: Vec<(&str, &str)> =
                        dashes.iter().map(|x| (x.as_str(), x.as_str())).collect();
                    combo(ui, &format!("Button {k} dashboard"), &mut b.dashboard, &d);
                }
                _ => {
                    combo(
                        ui,
                        &format!("Button {k} action"),
                        &mut b.action,
                        &[
                            ("new_transaction", "New transaction"),
                            ("import_file", "Import file…"),
                            ("reconcile", "Reconcile an account"),
                            ("calculator", "Calculator"),
                            ("new_expense", "New expense"),
                            ("collect_from_phone", "Collect from phone"),
                        ],
                    );
                    if matches!(b.action.as_str(), "new_transaction" | "reconcile") {
                        account_combo(ui, &format!("Button {k} account"), &mut b.account, accts);
                    }
                }
            }
            if n > 1 {
                ui.row(|ui| {
                    if ui.button(format!("Remove button {k}")).clicked() {
                        remove = Some(i);
                    }
                });
            }
        });
    }
    if let Some(i) = remove {
        f.buttons.remove(i);
    }
    if f.buttons.len() < 12 {
        ui.row(|ui| {
            if ui.button("+ Add button").clicked() {
                f.buttons.push(ButtonForm {
                    kind: "screen".into(),
                    screen: "home".into(),
                    label: "Home".into(),
                    ..ButtonForm::default()
                });
            }
        });
    }
}

fn save_form(app: &mut App, f: &BlockForm) -> Result<(), String> {
    let num = |s: &str| -> Value {
        s.trim()
            .parse::<u64>()
            .map_or(json!(s.trim()), |n| json!(n))
    };
    let date = |s: &str| -> Result<String, String> { fmt::date_arg(s) };
    let settings = match f.kind.as_str() {
        "account_transactions" => {
            let mut s = json!({"account": f.account, "mode": f.mode, "n": num(&f.n), "include_children": f.include_children});
            if f.mode == "above" {
                set(&mut s, "amount", json!(f.amount.trim().replace(',', "")));
                set(&mut s, "direction", json!(f.direction));
                set(&mut s, "days", num(&f.days));
            }
            s
        }
        "scheduled" => {
            let mut s = json!({"mode": f.mode, "accounts": f.accounts});
            if f.mode == "next" {
                set(&mut s, "n", num(&f.n));
            } else {
                set(&mut s, "days", num(&f.days));
            }
            s
        }
        "balances" => {
            if f.source == "groups" {
                json!({"source": "groups", "groups": f.groups})
            } else {
                json!({"source": "accounts", "accounts": f.accounts})
            }
        }
        "chart" | "report_summary" => {
            let mut s = json!({"period": f.period});
            if f.kind == "chart" {
                set(&mut s, "chart", json!(f.chart));
                if f.chart == "account_balance" {
                    set(&mut s, "account", json!(f.account));
                }
            } else {
                set(&mut s, "report", json!(f.report));
            }
            if f.period == "custom" {
                set(&mut s, "from", json!(date(&f.from)?));
                set(&mut s, "to", json!(date(&f.to)?));
            }
            s
        }
        "shortcuts" => {
            let buttons: Vec<Value> = f
                .buttons
                .iter()
                .map(|b| {
                    let t = match b.kind.as_str() {
                        "account" => json!({"kind": "account", "account": b.account}),
                        "report" => json!({"kind": "report", "report": b.report}),
                        "screen" => json!({"kind": "screen", "screen": b.screen}),
                        "dashboard" => json!({"kind": "dashboard", "dashboard": b.dashboard}),
                        _ => json!({"kind": "action", "action": b.action, "account": b.account}),
                    };
                    let mut x = json!({"label": b.label, "target": t});
                    if !b.icon.is_empty() {
                        set(&mut x, "icon", json!(b.icon));
                    }
                    x
                })
                .collect();
            let size = if f.button_size.is_empty() {
                "medium"
            } else {
                f.button_size.as_str()
            };
            json!({"buttons": buttons, "button_size": size})
        }
        "bills_due" => json!({"days": num(&f.days)}),
        "note" => json!({"text": f.text}),
        _ => json!({}),
    };
    let r = match &f.block {
        Some(b) => app.run(
            "dashboard.update_block",
            json!({"dashboard": f.dashboard, "block": b, "title": f.title, "settings": settings}),
        ),
        None => app.run(
            "dashboard.add_block",
            json!({"dashboard": f.dashboard, "type": f.kind, "title": f.title, "settings": settings}),
        ),
    };
    r.map(|_| ()).map_err(|e| e.display())
}
