//! The accounts rail and its slide-out pane (app-desktop.md revision 23): tabs
//! for Dashboards, All accounts and each account group; the pane lists the
//! group's accounts with their balances.

use serde_json::{Value, json};

use super::ui::{Item, Kind, Ui};
use super::util::{Json, array};
use super::{App, AppEvent};
use crate::base::accounts::{GROUPS, first_amount, group_of, members_where};
use crate::{AccountsPaneConfig, View};

/// A rail tab: `None` = All accounts, `Some(i)` = group `i`, `Some(DASHBOARDS)`.
pub(crate) type Tab = Option<usize>;

pub(crate) const DASHBOARDS: usize = 99;

/// Seconds the pointer rests on a tab before its pane opens.
const HOVER_OPEN_SECS: f64 = 3.0;
/// Seconds after the pointer leaves an unpinned pane before it closes.
const AUTO_CLOSE_SECS: f64 = 1.0;
/// Default and smallest pane widths in pixels.
const DEFAULT_WIDTH: i32 = 260;
const MIN_WIDTH: i32 = 160;

fn tab_of(name: &str) -> Tab {
    if name == "Dashboards" {
        return Some(DASHBOARDS);
    }
    GROUPS.iter().position(|g| *g == name)
}

pub(crate) fn tab_name(tab: Tab) -> &'static str {
    if tab == Some(DASHBOARDS) {
        return "Dashboards";
    }
    tab.and_then(|i| GROUPS.get(i).copied())
        .unwrap_or("All accounts")
}

/// The icon of a tab.
fn tab_icon(tab: Tab) -> &'static str {
    match tab {
        Some(DASHBOARDS) => "tiles",
        None => "list",
        Some(0) => "bank",
        Some(1) => "card",
        Some(2) => "chart-line",
        Some(3) => "house-outline",
        Some(4) => "document",
        Some(5) => "scale",
        _ => "tag",
    }
}

#[derive(Debug, Default)]
pub(crate) struct PaneState {
    pub open: bool,
    pub pinned: bool,
    pub group: Tab,
    /// The tab the pointer rests on, since when, and whether it opened the pane.
    pub hover: Option<(Tab, f64, bool)>,
    /// When the pointer left the unpinned pane.
    pub outside_since: Option<f64>,
    /// The pointer has been over the pane since it opened.
    pub armed: bool,
    pub show_archived: bool,
    /// Folded groups: (group, one group shown) → open.
    pub groups_open: std::collections::HashMap<(usize, bool), bool>,
    /// Accounts and balances, by generation.
    cache: Option<(u64, bool, SidebarData)>,
}

impl PaneState {
    pub(crate) fn from_config(c: &AccountsPaneConfig) -> Self {
        PaneState {
            open: c.pinned && c.open,
            pinned: c.pinned,
            group: tab_of(&c.group),
            ..PaneState::default()
        }
    }
}

/// What the account list shows.
#[derive(Debug, Default, Clone)]
pub(crate) struct SidebarData {
    pub list: Vec<Value>,
    pub balances: Vec<(String, String)>,
    pub unpriced: bool,
}

impl App {
    pub(crate) fn open_pane(&mut self, tab: Tab) {
        self.pane.open = true;
        self.pane.group = tab;
        self.pane.outside_since = None;
        self.pane.armed = false;
        self.persist_pane();
    }

    pub(crate) fn close_pane(&mut self) {
        self.pane.open = false;
        self.pane.outside_since = None;
        self.persist_pane();
    }

    pub(crate) fn pane_width(&self) -> i32 {
        self.config
            .panels
            .sidebar
            .and_then(|w| i32::try_from(w).ok())
            .unwrap_or(DEFAULT_WIDTH)
            .max(MIN_WIDTH)
    }

    fn persist_pane(&mut self) {
        let want = AccountsPaneConfig {
            pinned: self.pane.pinned,
            group: self
                .pane
                .group
                .map(|g| tab_name(Some(g)))
                .unwrap_or_default()
                .to_owned(),
            open: self.pane.pinned && self.pane.open,
        };
        if want != self.config.accounts_pane {
            self.config.accounts_pane = want;
            self.config_dirty_since.get_or_insert(self.now);
        }
    }

    /// Accounts with display balances (cached per generation).
    pub(crate) fn sidebar_data(&mut self) -> SidebarData {
        let archived = self.pane.show_archived;
        if let Some((g, a, d)) = &self.pane.cache
            && *g == self.generation
            && *a == archived
        {
            return d.clone();
        }
        let list = array(self.read(
            "account.list",
            json!({"include_archived": archived, "today": self.today()}),
        ));
        let mut balances = Vec::new();
        for a in &list {
            let id = a.str("id").to_owned();
            let children = a.at("placeholder").as_bool() == Some(true);
            let shown = self
                .read(
                    "account.balance",
                    json!({"account": id, "include_children": children}),
                )
                .map(|b| first_amount(b.at("display"), a.str("commodity")))
                .unwrap_or_default();
            balances.push((id, shown));
        }
        let mut data = SidebarData {
            list,
            balances,
            unpriced: false,
        };
        // With investments on, brokerage accounts show their market value.
        if self.module_on("investments")
            && let Ok(h) = self
                .backend
                .invoke("inv.holdings", json!({"as_of": self.today()}))
        {
            data.list.retain(|a| a.str("subtype") != "security");
            for acc in h.list("accounts") {
                let id = acc.str("account");
                if let Some(slot) = data.balances.iter_mut().find(|(i, _)| i == id) {
                    slot.1 = acc.str("total").to_owned();
                }
                data.unpriced |= acc.at("unpriced").as_bool() == Some(true);
            }
        }
        self.pane.cache = Some((self.generation, archived, data.clone()));
        data
    }
}

/// Ctrl+B: open or close the pane.
pub(crate) fn toggle(app: &mut App) {
    if app.pane.open {
        app.close_pane();
    } else {
        app.open_pane(app.pane.group);
    }
}

fn sum_of(accounts: &[&Value], balances: &[(String, String)]) -> String {
    let commodity = accounts
        .iter()
        .find(|a| a.at("placeholder").as_bool() != Some(true))
        .map_or("USD", |a| a.str("commodity"));
    let dp = mk_core::currency::minor_units(commodity);
    let mut t = mk_core::Decimal::ZERO;
    for a in accounts
        .iter()
        .filter(|a| a.at("placeholder").as_bool() != Some(true))
    {
        if let Some(v) = balances
            .iter()
            .find(|(id, _)| id == a.str("id"))
            .and_then(|(_, b)| b.parse::<mk_core::Decimal>().ok())
        {
            t = t.checked_add(v).unwrap_or(t);
        }
    }
    mk_core::currency::format_amount(t, Some(dp))
}

fn group_members(list: &[Value], gi: usize) -> Vec<&Value> {
    members_where(list, |a| group_of(a) == gi)
}

/// A group's heading text, or `None` when it has no account to show.
pub(crate) fn group_title(data: &SidebarData, gi: usize) -> Option<String> {
    let group = GROUPS.get(gi)?;
    let members = group_members(&data.list, gi);
    if members
        .iter()
        .all(|a| a.at("placeholder").as_bool() == Some(true))
    {
        return None;
    }
    Some(if gi == GROUPS.len() - 1 {
        (*group).to_owned()
    } else if gi == 2 && data.unpriced {
        format!(
            "{group}  {}  (no price)",
            crate::fmt::group(&sum_of(&members, &data.balances))
        )
    } else {
        format!(
            "{group}  {}",
            crate::fmt::group(&sum_of(&members, &data.balances))
        )
    })
}

/// A group's total (`None` for Categories).
pub(crate) fn group_total(data: &SidebarData, gi: usize) -> Option<String> {
    (gi + 1 < GROUPS.len())
        .then(|| crate::fmt::group(&sum_of(&group_members(&data.list, gi), &data.balances)))
}

/// Dashboard names for the Dashboards tab's tooltip.
fn dashboard_names(app: &mut App) -> Vec<(String, String)> {
    array(app.backend.invoke("dashboard.list", json!({})).ok())
        .iter()
        .map(|d| (d.str("id").to_owned(), d.str("name").to_owned()))
        .collect()
}

/// The rail's tabs.
pub(crate) fn tabs(app: &mut App, ui: &mut Ui) {
    let data = app.sidebar_data();
    let names: Vec<String> = dashboard_names(app).into_iter().map(|(_, n)| n).collect();
    let mut list: Vec<(Tab, String, String)> = vec![
        (
            Some(DASHBOARDS),
            format!("Dashboards\n{}", names.join("\n")),
            "Dashboards".to_owned(),
        ),
        (None, "All accounts".to_owned(), "All accounts".to_owned()),
    ];
    for gi in 0..GROUPS.len() {
        if let Some(title) = group_title(&data, gi) {
            let name = match group_total(&data, gi) {
                Some(total) => format!("{}, {total}", tab_name(Some(gi))),
                None => tab_name(Some(gi)).to_owned(),
            };
            list.push((Some(gi), title, name));
        }
    }
    for (tab, tip, name) in list {
        let selected = app.pane.open && app.pane.group == tab;
        let resp = ui.item(
            Item {
                kind: Kind::Tab,
                icon: tab_icon(tab).to_owned(),
                label: name.clone(),
                desc: tip,
                checked: selected,
                ..Item::default()
            },
            true,
        );
        if resp.clicked() {
            if selected {
                app.close_pane();
            } else {
                app.open_pane(tab);
            }
            if let Some(h) = app.pane.hover.as_mut() {
                h.2 = true;
            }
        }
        match resp.key.as_deref() {
            Some("hover") => app.pane.hover = Some((tab, app.now, false)),
            Some("unhover") if app.pane.hover.is_some_and(|(t, ..)| t == tab) => {
                app.pane.hover = None;
            }
            _ => {}
        }
    }
}

/// Timers of the rail: a resting pointer opens a tab's pane; an unpinned pane the
/// pointer left closes. Returns whether anything changed.
pub(crate) fn tick(app: &mut App) -> bool {
    let now = app.now;
    let mut changed = false;
    if let Some((tab, since, fired)) = app.pane.hover
        && !fired
        && crate::layout::elapsed(now, since, HOVER_OPEN_SECS)
    {
        if !(app.pane.open && app.pane.group == tab) {
            app.open_pane(tab);
        }
        app.pane.hover = Some((tab, since, true));
        changed = true;
    }
    if app.pane.open
        && !app.pane.pinned
        && let Some(t) = app.pane.outside_since
        && crate::layout::elapsed(now, t, AUTO_CLOSE_SECS)
    {
        app.close_pane();
        changed = true;
    }
    changed
}

fn account_line(app: &mut App, ui: &mut Ui, a: &Value, balances: &[(String, String)]) {
    let depth = i32::try_from(a.at("depth").as_u64().unwrap_or(0)).unwrap_or(0);
    let id = a.str("id").to_owned();
    let bal = balances
        .iter()
        .find(|(i, _)| *i == id)
        .map_or("0.00".to_owned(), |(_, b)| b.clone());
    let shown = crate::fmt::group(&bal);
    let tone = if crate::fmt::is_negative(&bal) {
        super::ui::Tone::Decrease
    } else {
        super::ui::Tone::Normal
    };
    if a.at("placeholder").as_bool() == Some(true) {
        ui.row(|ui| {
            ui.next().bold().grow();
            ui.label(a.str("name"));
            ui.tip(format!("{} balance", a.str("full_name"))).tone(tone);
            ui.amount(shown);
        });
        ui.indent_last(depth);
        return;
    }
    let selected = matches!(&app.view, View::Register { account } if *account == id);
    let name = format!("Account {}", a.str("full_name"));
    ui.next().id(format!("account:{id}"));
    let resp = ui.list_item(&name, selected, |ui| {
        ui.next().grow();
        ui.label(a.str("name"));
        ui.tip(format!("{} balance", a.str("full_name"))).tone(tone);
        ui.amount(shown);
    });
    ui.indent_last(depth);
    if resp.clicked() {
        app.navigate(View::Register { account: id });
        if !app.pane.pinned {
            app.close_pane();
        }
    }
}

/// The pane's header tools and content.
pub(crate) fn pane(app: &mut App, ui: &mut Ui) {
    if !app.pane.open {
        return;
    }
    // The pane's right edge is dragged: "w:<px>" while dragging, "done" at the end.
    if let Some(k) = ui.raw("pane-resize").key {
        ui.consume();
        if let Some(w) = k.strip_prefix("w:").and_then(|w| w.parse::<i32>().ok()) {
            let max = (app.window_width / 2).max(MIN_WIDTH);
            app.config.panels.sidebar = u32::try_from(w.clamp(MIN_WIDTH, max)).ok();
        } else if k == "done" {
            app.emit(AppEvent::ConfigChanged);
        }
    }
    ui.title(tab_name(app.pane.group));
    let pinned = app.pane.pinned;
    let tip = if pinned {
        "Let it slide away"
    } else {
        "Keep open"
    };
    ui.toolbar(|t| {
        t.tip(tip);
        if t.icon_toggle(if pinned { "pin-filled" } else { "pin" }, tip, pinned)
            .clicked()
        {
            app.pane.pinned = !pinned;
            app.pane.outside_since = None;
            app.persist_pane();
        }
        t.tip("Close account list");
        if t.icon_button("close", "Close account list", false)
            .clicked()
        {
            app.close_pane();
        }
    });
    // Pointer over / leaving the pane (auto close).
    match ui
        .action()
        .filter(|a| a.id == "pane" && a.ev == super::ui::Ev::Key)
        .map(|a| a.value.clone())
        .as_deref()
    {
        Some("enter") => {
            ui.consume();
            app.pane.armed = true;
            app.pane.outside_since = None;
        }
        Some("leave") => {
            // (The pointer was over the rail or the pane: leaving arms the close.)
            ui.consume();
            app.pane.armed = true;
            app.pane.outside_since = Some(app.now);
        }
        _ => {}
    }
    if app.pane.group == Some(DASHBOARDS) {
        ui.card("", |ui| {
            if super::dashboard::list_pane(app, ui) && !app.pane.pinned {
                app.close_pane();
            }
        });
        return;
    }
    let data = app.sidebar_data();
    let only = app.pane.group;
    for (gi, _) in GROUPS.iter().enumerate() {
        if only.is_some_and(|o| o != gi) {
            continue;
        }
        let Some(title) = group_title(&data, gi) else {
            continue;
        };
        let members = group_members(&data.list, gi);
        let categories = gi == GROUPS.len() - 1;
        let mut open = *app
            .pane
            .groups_open
            .entry((gi, only.is_some()))
            .or_insert(!categories || only.is_some());
        ui.collapsing(&title, &mut open, |ui| {
            if categories {
                for (sub, ty) in [("Income", "income"), ("Expenses", "expense")] {
                    let part: Vec<&Value> = members
                        .iter()
                        .copied()
                        .filter(|a| a.str("type") == ty)
                        .collect();
                    if part.is_empty() {
                        continue;
                    }
                    ui.next().bold();
                    ui.label(format!(
                        "{sub}  {}",
                        crate::fmt::group(&sum_of(&part, &data.balances))
                    ));
                    for a in part {
                        account_line(app, ui, a, &data.balances);
                    }
                }
            } else {
                for a in members {
                    account_line(app, ui, a, &data.balances);
                }
            }
        });
        app.pane.groups_open.insert((gi, only.is_some()), open);
    }
    ui.group(|ui| {
        let mut archived = app.pane.show_archived;
        if ui.checkbox(&mut archived, "Show archived").clicked() {
            app.pane.show_archived = archived;
        }
    });
}
