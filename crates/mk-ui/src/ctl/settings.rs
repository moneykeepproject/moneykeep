//! Settings (app-desktop.md revisions 7, 9, 14, 20, 21, 24, 26, 27, 36): Book,
//! Display, Operation, Security, Import, Connections, Phone capture, Mileage, Definitions,
//! Templates and Features, chosen from the list on the left.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine;
use serde_json::{Value, json};

use super::App;
use super::ui::{Item, Kind, Ui};
use super::util::{Json, array, display_name, png_picture, tile_color};
use crate::base::about::{MAX_DESCRIPTION, MAX_TAGLINE};
use crate::base::display::{DEFAULT_SIZE, clamp_size, larger_size, smaller_size};
use crate::base::recent::{COLORS, PRESETS, SYMBOLS, TileIcon, tile_from_info};
use crate::theme::{self, Density, Theme};
use crate::{AppEvent, DateOrder, FontInfo, FontProvider, View};

/// Settings sections, in order (Book first, revision 26; Display when none was
/// chosen yet).
const SECTIONS: [&str; 15] = [
    "Book",
    "Display",
    "Operation",
    "Security",
    "Backups",
    "Import",
    "Connections",
    "Phone capture",
    "Mileage",
    "Subscriptions",
    "Projects",
    "Definitions",
    "Templates",
    "Features",
    "Advanced",
];

#[derive(Debug, Default)]
pub(crate) struct SettingsState {
    pub refusals: Vec<(String, String)>,
    pub window: String,
    pub loaded: u64,
    pub drafts: Vec<(String, String)>,
    pub drafts_gen: u64,
    pub field_errors: Vec<(String, String)>,
    pub profile: ProfileState,
    /// Reset MoneyKeep settings (revision 41): the typed confirmation while the
    /// dialog is open.
    pub reset: Option<String>,
    /// The folder of the settings file (the app shell tells it), shown in that
    /// dialog.
    pub folder: Option<String>,
}

/// Settings → Book state.
#[derive(Debug, Default)]
pub(crate) struct ProfileState {
    drafts: Vec<(&'static str, String)>,
    drafts_gen: u64,
    mode: Option<&'static str>,
    errors: Vec<(&'static str, String)>,
}

/// Installed fonts (Settings → Display).
#[derive(Default)]
pub(crate) struct FontState {
    provider: Option<Box<dyn FontProvider>>,
    families: Option<Vec<FontInfo>>,
    available: BTreeMap<String, bool>,
    pub search_ui: String,
    pub search_number: String,
    warned: BTreeSet<String>,
}

impl std::fmt::Debug for FontState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontState").finish_non_exhaustive()
    }
}

impl FontState {
    pub(crate) fn set_provider(&mut self, p: Box<dyn FontProvider>) {
        self.provider = Some(p);
        self.families = None;
        self.available.clear();
    }

    fn families(&mut self) -> Vec<FontInfo> {
        if self.families.is_none() {
            let mut list = self
                .provider
                .as_ref()
                .map(|p| p.families())
                .unwrap_or_default();
            list.sort_by_key(|a| a.family.to_lowercase());
            list.dedup_by(|a, b| a.family == b.family);
            self.families = Some(list);
        }
        self.families.clone().unwrap_or_default()
    }

    /// Whether `family` is installed and readable.
    fn usable(&mut self, family: &str) -> bool {
        if let Some(ok) = self.available.get(family) {
            return *ok;
        }
        let listed = self.families().iter().any(|f| f.family == family);
        let ok = listed
            && self
                .provider
                .as_ref()
                .and_then(|p| p.load(family))
                .is_some_and(|b| !b.data.is_empty());
        self.available.insert(family.to_owned(), ok);
        ok
    }

    /// The fonts in use: the chosen ones that are available.
    pub(crate) fn active(&self, config: &crate::UserConfig) -> (Option<String>, Option<String>) {
        let ok = |f: &Option<String>| {
            f.clone()
                .filter(|f| self.available.get(f).copied().unwrap_or(false))
        };
        (ok(&config.ui_font), ok(&config.number_font))
    }
}

/// Check the chosen fonts once and say when one is missing.
pub(crate) fn check_fonts(app: &mut App) {
    let mut missing = None;
    for f in [app.config.ui_font.clone(), app.config.number_font.clone()]
        .into_iter()
        .flatten()
    {
        if !app.fonts.usable(&f) && app.fonts.warned.insert(f.clone()) && missing.is_none() {
            missing = Some(f);
        }
    }
    if let Some(f) = missing {
        app.notify(format!(
            "The font {f} isn't available, so the built-in font is used."
        ));
    }
}

fn setting(app: &mut App, key: &str) -> Value {
    app.read("setting.get", json!({"module": "core", "key": key}))
        .map(|v| v.at("value").clone())
        .unwrap_or(Value::Null)
}

/// Set a core setting; a refusal becomes the message.
pub(crate) fn set_core(app: &mut App, key: &str, value: Value) {
    if let Err(e) = app.run(
        "setting.set",
        json!({"module": "core", "key": key, "value": value}),
    ) {
        app.notify_error(&e);
    }
}

pub(crate) fn set_text_size(app: &mut App, percent: u32) {
    let p = clamp_size(percent);
    if p != app.config.text_size {
        app.config.text_size = p;
        app.emit(AppEvent::ConfigChanged);
    }
}

pub(crate) fn view(app: &mut App, ui: &mut Ui) {
    ui.title("Settings");
    let invoicing = app.module_on("invoicing");
    // Connections serve bank feeds, payment processors and brokerages.
    let feeds = app.module_on("bank-feeds")
        || app.module_on("payment-processing")
        || app.module_on("investments");
    let expenses = app.module_on("expenses");
    let mileage = app.module_on("mileage");
    let subscriptions = app.module_on("subscriptions");
    let projects = app.module_on("projects");
    let sections: Vec<&str> = SECTIONS
        .iter()
        .copied()
        .filter(|s| {
            (*s != "Templates" || invoicing)
                && (*s != "Connections" || feeds)
                && (*s != "Phone capture" || expenses)
                && (*s != "Mileage" || mileage)
                && (*s != "Subscriptions" || subscriptions)
                && (*s != "Projects" || projects)
        })
        .collect();
    let current = if sections.contains(&app.config.settings_section.as_str()) {
        app.config.settings_section.clone()
    } else {
        "Display".to_owned()
    };
    let width = app
        .config
        .panels
        .settings
        .and_then(|w| i32::try_from(w).ok())
        .unwrap_or(200);
    ui.side(width, |ui| {
        for s in sections.iter().copied() {
            let selected = current == s;
            ui.next().grow();
            if ui.tab(s, selected).clicked() && !selected {
                app.config.settings_section = s.to_owned();
                app.emit(AppEvent::ConfigChanged);
            }
        }
    });
    ui.scope(format!("settings:{current}"), |ui| match current.as_str() {
        "Book" => book(app, ui),
        "Security" => super::security::settings_section(app, ui),
        "Backups" => super::backups::settings_section(app, ui),
        "Import" => import(app, ui),
        "Operation" => operation(app, ui),
        "Features" => features(app, ui),
        "Templates" => super::templates::settings_section(app, ui),
        "Definitions" => super::definitions::settings_section(app, ui),
        "Connections" => super::connections::settings_section(app, ui),
        "Phone capture" => super::capture::settings_section(app, ui),
        "Mileage" => super::mileage::settings_section(app, ui),
        "Subscriptions" => super::subscriptions::settings_section(app, ui),
        "Projects" => super::projects::settings_section(app, ui),
        "Advanced" => {
            if expenses {
                super::capture::advanced_section(app, ui);
            }
            reset_card(app, ui);
        }
        _ => display(app, ui),
    });
}

// ----- Display ------------------------------------------------------------------------

fn display(app: &mut App, ui: &mut Ui) {
    let shortcuts = if cfg!(target_os = "macos") {
        "Cmd + = larger, Cmd + − smaller, Cmd + 0 reset"
    } else {
        "Ctrl + = larger, Ctrl + − smaller, Ctrl + 0 reset"
    };
    ui.card("Appearance", |ui| {
        let mut changed = false;
        ui.form("Theme", |ui| {
            for (t, label) in [
                (Theme::System, "Follow system"),
                (Theme::Light, "Light"),
                (Theme::Dark, "Dark"),
            ] {
                changed |= ui.radio(&mut app.config.theme, t, label).clicked();
            }
        });
        ui.form("Density", |ui| {
            for (d, label) in [
                (Density::Comfortable, "Comfortable"),
                (Density::Compact, "Compact"),
            ] {
                changed |= ui.radio(&mut app.config.density, d, label).clicked();
            }
        });
        if changed {
            app.emit(AppEvent::ConfigChanged);
        }
        ui.form("Text size", |ui| {
            let p = app.text_size();
            ui.tip(shortcuts).named("Smaller text");
            if ui.button("−").clicked() {
                set_text_size(app, smaller_size(p));
            }
            let p = app.text_size();
            ui.tip(shortcuts)
                .named(format!("Text size {p} %"))
                .width(64)
                .align(super::ui::Align::Center);
            ui.label(format!("{p} %"));
            ui.tip(shortcuts).named("Larger text");
            if ui.button("+").clicked() {
                set_text_size(app, larger_size(p));
            }
            ui.tip(shortcuts).named("Reset text size");
            if ui.button("Reset").clicked() {
                set_text_size(app, DEFAULT_SIZE);
            }
        });
    });
    ui.card("Fonts", |ui| {
        font_picker(app, ui, false);
        font_picker(app, ui, true);
    });
    ui.card("Scrolling", |ui| {
        ui.form("Scroll bar width", |ui| {
            let mut w = i32::try_from(app.scroll_bar_width()).unwrap_or(14);
            let min = i32::try_from(theme::SCROLL_WIDTH_MIN).unwrap_or(6);
            let max = i32::try_from(theme::SCROLL_WIDTH_MAX).unwrap_or(30);
            if ui.slider("Scroll bar width", &mut w, min, max).changed() {
                let w = theme::clamp_scroll_width(u32::try_from(w).unwrap_or(14));
                if w != app.config.scroll_bar_width {
                    app.config.scroll_bar_width = w;
                    app.emit(AppEvent::ConfigChanged);
                }
            }
            ui.secondary(format!("{} px", app.scroll_bar_width()));
        });
    });
    ui.actions(|ui| {
        if ui.button("Reset display settings").clicked() {
            reset_display(app);
        }
    });
}

fn reset_display(app: &mut App) {
    app.config.theme = Theme::default();
    app.config.scroll_bar_width = theme::SCROLL_WIDTH_DEFAULT;
    app.config.density = Density::default();
    app.config.ui_font = None;
    app.config.number_font = None;
    app.config.panels = crate::PanelWidths::default();
    app.config.text_size = DEFAULT_SIZE;
    app.config.accounts_pane = crate::AccountsPaneConfig::default();
    app.pane.pinned = false;
    app.pane.open = false;
    app.pane.group = None;
    app.reset_calculator();
    app.emit(AppEvent::ConfigChanged);
}

fn font_picker(app: &mut App, ui: &mut Ui, number: bool) {
    let (label, builtin) = if number {
        ("Number font", "Built-in monospace")
    } else {
        ("Interface font", "Built-in")
    };
    let current = if number {
        app.config.number_font.clone()
    } else {
        app.config.ui_font.clone()
    };
    let families: Vec<FontInfo> = app
        .fonts
        .families()
        .into_iter()
        .filter(|f| !number || f.monospace)
        .collect();
    let mut search = if number {
        app.fonts.search_number.clone()
    } else {
        app.fonts.search_ui.clone()
    };
    ui.form(label, |ui| {
        let q = search.trim().to_lowercase();
        let mut options = vec![builtin.to_owned()];
        let shown: Vec<&FontInfo> = families
            .iter()
            .filter(|f| q.is_empty() || f.family.to_lowercase().contains(&q))
            .take(200)
            .collect();
        options.extend(shown.iter().map(|f| f.family.clone()));
        let value = current.clone().unwrap_or_else(|| builtin.to_owned());
        let mut idx = options
            .iter()
            .position(|o| *o == value)
            .unwrap_or(usize::MAX);
        ui.next().width(240);
        let resp = ui.combo_search(
            label,
            &mut idx,
            &options,
            &mut search,
            &format!("Search {}", label.to_lowercase()),
        );
        // The list shows the current one even when the search hides it.
        ui.set_last_value(&value);
        // Entries after the built-in one are named "<label>: <family>".
        ui.entry_prefix(&format!("{label}:"), 1);
        if resp.changed
            && let Some(v) = options.get(idx)
        {
            let family = (v != builtin).then(|| v.clone());
            if number {
                app.config.number_font = family;
            } else {
                app.config.ui_font = family;
            }
            app.emit(AppEvent::ConfigChanged);
            check_fonts(app);
        }
    });
    if number {
        app.fonts.search_number = search;
    } else {
        app.fonts.search_ui = search;
    }
}

// ----- Operation ------------------------------------------------------------------------

fn operation(app: &mut App, ui: &mut Ui) {
    use mk_core::datefmt::DateStyle;
    ui.card("Dates", |ui| {
        let current = app.date_style();
        let today = app.today().parse::<mk_core::Date>().ok();
        let styles: Vec<DateStyle> = DateStyle::all().collect();
        let options: Vec<String> = styles
            .iter()
            .map(|s| format!("{}  {}", s.pattern(), today.map(|d| s.format(d)).unwrap_or_default()))
            .collect();
        let mut idx = styles.iter().position(|s| *s == current).unwrap_or(0);
        ui.form("Date format", |ui| {
            ui.next().width(260);
            if ui.combo("Date format", &mut idx, &options).changed()
                && let Some(s) = styles.get(idx)
                && *s != current
            {
                app.config.date_format = s.pattern().to_owned();
                app.sync_dates();
                app.emit(AppEvent::ConfigChanged);
            }
        });
        if let Some(it) = ui.last() {
            it.value = current.pattern().to_owned();
        }
        ui.secondary(
            "How dates are shown and typed. Dates are always stored the same way, so changing this never changes your data.",
        );
        let mut order = app.config.date_order;
        ui.form("Sort transactions by date", |ui| {
            ui.radio(&mut order, DateOrder::Descending, "Descending (newest first)");
            ui.radio(&mut order, DateOrder::Ascending, "Ascending (oldest first)");
        });
        if order != app.config.date_order {
            app.config.date_order = order;
            app.emit(AppEvent::ConfigChanged);
        }
    });
    messages(app, ui);
    ui.card("Transactions", |ui| {
        module_settings(app, ui, "book");
    });
}

/// Status messages close by themselves (app-desktop.md revision 37).
fn messages(app: &mut App, ui: &mut Ui) {
    ui.card("Status messages", |ui| {
        let mut prefs = app.config.ui;
        ui.form("", |ui| {
            ui.checkbox(
                &mut prefs.message_autoclose,
                "Close status messages automatically",
            );
        });
        let seconds = crate::MESSAGE_SECONDS;
        let options: Vec<String> = seconds.iter().map(|n| format!("{n} seconds")).collect();
        let mut idx = seconds
            .iter()
            .position(|n| *n == prefs.seconds())
            .unwrap_or(1);
        ui.form("After", |ui| {
            ui.next().width(160).enabled(prefs.message_autoclose);
            if ui.combo("After", &mut idx, &options).changed()
                && let Some(n) = seconds.get(idx)
            {
                prefs.message_seconds = *n;
            }
        });
        ui.secondary(
            "A message closes by itself that long after it appears; it waits while the pointer or keyboard is on it. Error messages stay until you press Dismiss.",
        );
        if prefs != app.config.ui {
            app.config.ui = prefs;
            app.emit(AppEvent::ConfigChanged);
        }
    });
}

// ----- Features -------------------------------------------------------------------------

fn features(app: &mut App, ui: &mut Ui) {
    ui.secondary("Turning a feature off hides it. It never deletes data.");
    let modules = array(app.read("module.list", json!({})));
    for m in modules {
        let name = m.str("name").to_owned();
        let mut on = m.at("enabled").as_bool() == Some(true);
        let title = display_name(&name);
        ui.card("", |ui| {
            ui.row(|ui| {
                if ui.checkbox(&mut on, &title).clicked() {
                    let cmd = if on {
                        "module.enable"
                    } else {
                        "module.disable"
                    };
                    match app.run(cmd, json!({"module": name})) {
                        Ok(_) => app.settings.refusals.retain(|(n, _)| *n != name),
                        Err(e) => {
                            app.settings.refusals.retain(|(n, _)| *n != name);
                            app.settings.refusals.push((name.clone(), e.display()));
                            on = !on;
                        }
                    }
                }
                ui.next().grow();
                ui.secondary(m.str("description"));
            });
            if let Some((_, msg)) = app
                .settings
                .refusals
                .iter()
                .find(|(n, _)| *n == name)
                .cloned()
            {
                ui.tip("Refused");
                ui.error_text(msg);
            }
            if on && m.at("has_settings").as_bool() == Some(true) {
                ui.scope(format!("module:{name}"), |ui| {
                    module_settings(app, ui, &name)
                });
            }
        });
    }
}

fn draft(app: &App, key: &str) -> Option<String> {
    app.settings
        .drafts
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

fn set_draft(app: &mut App, key: &str, text: String) {
    app.settings.drafts.retain(|(k, _)| k != key);
    app.settings.drafts.push((key.to_owned(), text));
}

pub(crate) fn save_setting(app: &mut App, key: &str, value: Value) {
    app.settings.field_errors.retain(|(k, _)| k != key);
    if let Err(e) = app.run("module.set_setting", json!({"key": key, "value": value})) {
        app.settings
            .field_errors
            .push((key.to_owned(), e.display()));
    }
}

/// A module's settings as labelled fields that save immediately (revision 14).
pub(crate) fn module_settings(app: &mut App, ui: &mut Ui, module: &str) {
    if app.settings.drafts_gen != app.generation {
        app.settings.drafts.clear();
        app.settings.drafts_gen = app.generation;
    }
    let Some(s) = app.read("module.settings", json!({"module": module})) else {
        return;
    };
    let accounts = array(app.read("account.list", json!({})));
    let rates = if module == "sales-tax" || module == "invoicing" {
        array(app.backend.invoke("tax_rate.list", json!({})).ok())
    } else {
        Vec::new()
    };
    for d in s.list("settings") {
        // Colour bands have their own editor and Make a bill its own card
        // (Settings → Subscriptions).
        if d.str("type") == "bands" || d.str("key") == "subscriptions.make_bill_as" {
            continue;
        }
        let key = d.str("key").to_owned();
        let label = d.str("label").to_owned();
        ui.form(&label, |ui| {
            setting_field(app, ui, d, &key, &label, &accounts, &rates);
        });
        if !d.str("description").is_empty() {
            ui.caption(d.str("description"));
        }
        if let Some((_, e)) = app
            .settings
            .field_errors
            .iter()
            .find(|(k, _)| *k == key)
            .cloned()
        {
            ui.error_text(e);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn setting_field(
    app: &mut App,
    ui: &mut Ui,
    d: &Value,
    key: &str,
    label: &str,
    accounts: &[Value],
    rates: &[Value],
) {
    let value = d.at("value");
    match d.str("type") {
        "bool" => {
            let mut on = value.as_bool() == Some(true);
            ui.named(label);
            if ui.checkbox(&mut on, "").clicked() {
                save_setting(app, key, json!(on));
            }
        }
        "choice" => {
            let choices = d.list("choices").to_vec();
            let labels: Vec<String> = choices.iter().map(|c| c.str("label").to_owned()).collect();
            let mut idx = choices
                .iter()
                .position(|c| c.at("value") == value)
                .unwrap_or(usize::MAX);
            ui.next().width(220);
            if ui.combo(label, &mut idx, &labels).changed()
                && let Some(c) = choices.get(idx)
            {
                save_setting(app, key, c.at("value").clone());
            }
        }
        "template" => {
            let shown = d.at("shown").as_str().map_or_else(
                || "Built-in (choose in Settings, Templates)".to_owned(),
                str::to_owned,
            );
            ui.named(label);
            ui.label(shown);
        }
        "check_numbers" | "account_list" | "thresholds" => {
            let shown = match (d.str("type"), value) {
                ("account_list", Value::Null) => d.str("default_label").to_owned(),
                ("account_list", Value::Array(ids)) => ids
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(|id| accounts.iter().find(|a| a.str("id") == id))
                    .map(|a| a.str("full_name").to_owned())
                    .collect::<Vec<_>>()
                    .join(", "),
                ("check_numbers", Value::Object(m)) if !m.is_empty() => m
                    .iter()
                    .map(|(id, n)| {
                        let name = accounts
                            .iter()
                            .find(|a| a.str("id") == id)
                            .map_or(id.as_str(), |a| a.str("full_name"));
                        format!("{name}: {n}")
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                ("thresholds", Value::Object(m)) if !m.is_empty() => {
                    format!(
                        "Edited for {}",
                        m.keys().cloned().collect::<Vec<_>>().join(", ")
                    )
                }
                ("thresholds", _) => "Built-in (edit in Purchases → 1099)".to_owned(),
                _ => "Not set yet".to_owned(),
            };
            ui.named(label);
            ui.label(shown);
        }
        "account" | "tax_rate" | "terms" | "paid_with" => {
            let options: Vec<(Value, String)> = match d.str("type") {
                "paid_with" => accounts
                    .iter()
                    .filter(|a| {
                        matches!(a.str("subtype"), "bank" | "cash" | "credit-card")
                            && a.at("placeholder").as_bool() != Some(true)
                    })
                    .map(|a| (json!(a.str("id")), a.str("full_name").to_owned()))
                    .chain(std::iter::once((
                        json!("personal"),
                        "Personal funds (reimbursable)".to_owned(),
                    )))
                    .collect(),
                "account" => {
                    let subtypes: Vec<&str> = d
                        .list("subtypes")
                        .iter()
                        .filter_map(Value::as_str)
                        .collect();
                    accounts
                        .iter()
                        .filter(|a| subtypes.contains(&a.str("subtype")))
                        .map(|a| (json!(a.str("id")), a.str("full_name").to_owned()))
                        .collect()
                }
                "tax_rate" => rates
                    .iter()
                    .map(|r| (json!(r.str("id")), r.str("name").to_owned()))
                    .collect(),
                _ => {
                    let mut v = vec![(
                        json!({"type": "due_on_receipt"}),
                        "Due on receipt".to_owned(),
                    )];
                    for days in [15, 30, 45, 60] {
                        v.push((json!({"type": "net", "days": days}), format!("Net {days}")));
                    }
                    v
                }
            };
            let mut all: Vec<(Value, String)> = Vec::new();
            if d.str("type") != "terms" {
                all.push((Value::Null, d.str("default_label").to_owned()));
            }
            all.extend(options);
            let labels: Vec<String> = all.iter().map(|(_, l)| l.clone()).collect();
            let mut idx = all
                .iter()
                .position(|(v, _)| v == value)
                .unwrap_or(usize::MAX);
            ui.next().width(260);
            let resp = ui.combo(label, &mut idx, &labels);
            if !value.is_null()
                && idx == usize::MAX
                && let Some(it) = ui.last()
            {
                it.value = d.at("shown").as_str().unwrap_or_default().to_owned();
            }
            if resp.changed()
                && let Some((v, _)) = all.get(idx)
            {
                save_setting(app, key, v.clone());
            }
        }
        kind => {
            let current = match value {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            let mut text = draft(app, key).unwrap_or(current.clone());
            let multiline = key.ends_with("_address") || key.ends_with("_message");
            ui.next().width(if multiline { 320 } else { 220 });
            let resp = if multiline {
                ui.multiline(label, &mut text)
            } else {
                ui.field(label, &mut text)
            };
            if resp.changed {
                set_draft(app, key, text.clone());
            }
            if (resp.blurred || resp.submitted) && text != current {
                let v = if kind == "int" {
                    text.trim()
                        .parse::<i64>()
                        .map_or_else(|_| json!(text.trim()), |n| json!(n))
                } else {
                    json!(text)
                };
                save_setting(app, key, v);
                if app.settings.field_errors.iter().all(|(k, _)| k != key) {
                    app.settings.drafts.retain(|(k, _)| k != key);
                }
            }
        }
    }
}

// ----- Import ---------------------------------------------------------------------------

fn import(app: &mut App, ui: &mut Ui) {
    if app.settings.loaded != app.generation {
        app.settings.window = setting(app, "import.match_window_days")
            .as_i64()
            .unwrap_or(3)
            .to_string();
        app.settings.loaded = app.generation;
    }
    ui.card("Matching", |ui| {
        ui.form("Match window (days)", |ui| {
            ui.next().width(80);
            let resp = ui.field("Match window (days)", &mut app.settings.window);
            if resp.blurred || resp.submitted {
                match app.settings.window.trim().parse::<i64>() {
                    Ok(n) if (0..=30).contains(&n) => {
                        set_core(app, "import.match_window_days", json!(n))
                    }
                    _ => app.notify("The match window is a whole number of days from 0 to 30."),
                }
            }
        });
        let posted = setting(app, "import.date_source").as_str() == Some("posted");
        let options = vec![
            "Transaction date when given".to_owned(),
            "Posting date".to_owned(),
        ];
        let mut idx = usize::from(posted);
        ui.form("Date to use", |ui| {
            ui.next().width(240);
            if ui.combo("Date to use", &mut idx, &options).changed() && (idx == 1) != posted {
                set_core(
                    app,
                    "import.date_source",
                    json!(if idx == 1 { "posted" } else { "transaction" }),
                );
            }
        });
        ui.caption("For OFX and QFX files and suggested CSV layouts.");
    });
    ui.card("Folder import", |ui| {
        let mut auto = setting(app, "import.auto_commit").as_bool() == Some(true);
        if ui
            .checkbox(
                &mut auto,
                "Add watched-folder files automatically when nothing needs review",
            )
            .clicked()
        {
            set_core(app, "import.auto_commit", json!(auto));
        }
        let folder = setting(app, "import.watch_folder");
        ui.row(|ui| {
            ui.next().grow();
            ui.label(format!(
                "Watched folder: {}",
                folder.as_str().unwrap_or("not set")
            ));
            if ui.button("Choose folder…").clicked() {
                app.emit(AppEvent::ChooseWatchFolder);
            }
        });
        let mut watching = app.watching;
        ui.enabled(folder.is_string());
        if ui
            .checkbox(&mut watching, "Watch while MoneyKeep is open")
            .clicked()
        {
            app.watching = watching;
            app.emit(AppEvent::Watch(watching));
        }
    });
    super::inv_import::profiles_settings(app, ui);
}

// ----- Book (profile, revision 26) --------------------------------------------------------

fn error_of(app: &App, field: &str) -> Option<String> {
    app.settings
        .profile
        .errors
        .iter()
        .find(|(f, _)| *f == field)
        .map(|(_, e)| e.clone())
}

fn pdraft(app: &App, field: &str) -> Option<String> {
    app.settings
        .profile
        .drafts
        .iter()
        .find(|(f, _)| *f == field)
        .map(|(_, t)| t.clone())
}

fn set_pdraft(app: &mut App, field: &'static str, text: String) {
    app.settings.profile.drafts.retain(|(f, _)| *f != field);
    app.settings.profile.drafts.push((field, text));
}

fn save(app: &mut App, field: &'static str, command: &str, args: Value) -> bool {
    app.settings.profile.errors.retain(|(f, _)| *f != field);
    match app.run(command, args) {
        Ok(_) => {
            app.settings.profile.drafts.retain(|(f, _)| *f != field);
            true
        }
        Err(e) => {
            app.settings.profile.errors.push((field, e.display()));
            false
        }
    }
}

/// A chosen picture file (Settings → Book → Your picture): open the Picture editor
/// on it, or show why it cannot be used.
pub(crate) fn choose_picture(app: &mut App, bytes: &[u8]) {
    app.settings.profile.errors.retain(|(f, _)| *f != "picture");
    if let Err(e) = super::picture::open_photo(app, bytes) {
        app.settings.profile.errors.push(("picture", e.display()));
    }
}

/// Use picture saved the treated picture.
pub(crate) fn picture_saved(app: &mut App) {
    app.settings.profile.mode = None;
    app.settings.profile.errors.retain(|(f, _)| *f != "picture");
}

/// A profile text field that saves on Enter or leaving it; returns the text to
/// save.
fn text_field(
    app: &mut App,
    ui: &mut Ui,
    field: &'static str,
    label: &str,
    current: &str,
    multiline: bool,
    limit: usize,
) -> Option<String> {
    let mut text = pdraft(app, field).unwrap_or_else(|| current.to_owned());
    ui.next().width(if limit <= 2 { 56 } else { 320 });
    let resp = if multiline {
        let r = ui.multiline(label, &mut text);
        // One paragraph: Enter saves (app-desktop.md revision 26).
        if let Some(it) = ui.last() {
            it.number = 1;
        }
        r
    } else {
        ui.field(label, &mut text)
    };
    if resp.changed {
        if text.chars().count() > limit {
            text = text.chars().take(limit).collect();
        }
        set_pdraft(app, field, text.clone());
    }
    if (resp.blurred || resp.submitted) && text != current {
        return Some(text);
    }
    None
}

fn book(app: &mut App, ui: &mut Ui) {
    if app.settings.profile.drafts_gen != app.generation {
        app.settings.profile.drafts.clear();
        app.settings.profile.drafts_gen = app.generation;
    }
    let Some(info) = app.read("book.info", json!({})) else {
        return;
    };
    let Some(tile) = tile_from_info(&info) else {
        return;
    };
    ui.secondary("How this book looks on the start screen and in the top bar.");
    ui.card("Profile", |ui| {
        ui.form("Name", |ui| {
            if let Some(name) = text_field(app, ui, "name", "Name", &tile.name, false, 80) {
                save(app, "name", "book.update_profile", json!({ "name": name }));
            }
        });
        if let Some(e) = error_of(app, "name") {
            ui.error_text(e);
        }
        ui.form("Tagline", |ui| {
            // Longer text can be typed; the command refuses it with the count
            // (acceptance test 222).
            if let Some(t) = text_field(app, ui, "tagline", "Tagline", &tile.tagline, false, 120) {
                save(
                    app,
                    "tagline",
                    "book.update_profile",
                    json!({ "tagline": t }),
                );
            }
        });
        let typed = pdraft(app, "tagline").unwrap_or_else(|| tile.tagline.clone());
        let n = typed.chars().count();
        ui.next().tone(if n > MAX_TAGLINE {
            super::ui::Tone::Error
        } else {
            super::ui::Tone::Secondary
        });
        ui.caption(format!("{n} / {MAX_TAGLINE}"));
        if let Some(e) = error_of(app, "tagline") {
            ui.error_text(e);
        }
        ui.form("Description", |ui| {
            if let Some(d) = text_field(
                app,
                ui,
                "description",
                "Description",
                &tile.description,
                true,
                MAX_DESCRIPTION,
            ) {
                save(
                    app,
                    "description",
                    "book.update_profile",
                    json!({ "description": d }),
                );
            }
        });
        let typed = pdraft(app, "description").unwrap_or_else(|| tile.description.clone());
        ui.caption(format!("{} / {MAX_DESCRIPTION}", typed.chars().count()));
        if let Some(e) = error_of(app, "description") {
            ui.error_text(e);
        }
    });
    let started = info.at("chart").str("started_from").to_owned();
    if !started.is_empty() {
        ui.card("Chart of accounts", |ui| {
            ui.form("Started from", |ui| {
                ui.next().grow();
                ui.label(started.clone());
            });
        });
    }
    ui.card("Picture", |ui| icon_fields(app, ui, &info, &tile));
    ui.card("Colour", |ui| {
        ui.wrap(|ui| {
            for c in COLORS {
                let on = tile.color == c;
                let resp = ui.item(
                    Item {
                        kind: Kind::Toggle,
                        label: c.to_owned(),
                        text: String::new(),
                        desc: c.to_owned(),
                        checked: on,
                        color: Some(tile_color(c)),
                        icon: if on { "check".into() } else { String::new() },
                        ..Item::default()
                    },
                    true,
                );
                if resp.clicked() && !on {
                    save(app, "color", "book.update_profile", json!({ "color": c }));
                }
            }
        });
        if let Some(e) = error_of(app, "color") {
            ui.error_text(e);
        }
    });
    ui.heading("Preview");
    {
        let locked = app.backend.protection().0;
        // The preview follows the typed name, tagline and description.
        let mut shown = tile.clone();
        for (field, slot) in [
            ("name", &mut shown.name),
            ("tagline", &mut shown.tagline),
            ("description", &mut shown.description),
        ] {
            if let Some(d) = pdraft(app, field) {
                *slot = d;
            }
        }
        let picture = (tile.icon == TileIcon::Image)
            .then(|| {
                info.at("icon")
                    .get("image_png_base64")
                    .and_then(Value::as_str)
                    .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
                    .and_then(|png| png_picture(&png, 512))
            })
            .flatten();
        let path = app.security.book_path.clone();
        let last_opened = app
            .config
            .recent
            .iter()
            .find(|r| Some(&r.path) == path.as_ref())
            .and_then(|r| r.last_opened.clone());
        let item = super::start::book_card_item(
            app,
            &shown,
            super::start::CardExtras {
                number: 1,
                locked,
                failure: None,
                last_opened,
                path: None,
                label: format!("Preview of {}", shown.name),
            },
            picture,
        );
        ui.tiles("Preview", |ui| {
            ui.item(item, false);
        });
    }
}

fn icon_fields(app: &mut App, ui: &mut Ui, info: &Value, tile: &crate::base::recent::TileCache) {
    let current = match tile.icon {
        TileIcon::Letter { .. } => "letter",
        TileIcon::Symbol { .. } => "symbol",
        TileIcon::Preset { .. } => "preset",
        TileIcon::Image => "picture",
    };
    let mut mode = app.settings.profile.mode.unwrap_or(current);
    let default_letter: String = tile
        .name
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    let letter = match &tile.icon {
        TileIcon::Letter { text } => text.clone(),
        _ => default_letter,
    };
    ui.row(|ui| {
        for (kind, label) in [
            ("letter", "Letter"),
            ("symbol", "Symbol"),
            ("preset", "People and work"),
            ("picture", "Your picture"),
        ] {
            let before = mode;
            if ui.radio(&mut mode, kind, label).clicked() && before != kind {
                app.settings.profile.mode = Some(kind);
                app.settings.profile.errors.retain(|(f, _)| *f != "icon");
                if kind == "letter" && current != "letter" {
                    save(
                        app,
                        "icon",
                        "book.update_profile",
                        json!({"icon": {"kind": "letter", "text": letter}}),
                    );
                    app.settings.profile.mode = None;
                }
            }
        }
    });
    match mode {
        "letter" => {
            ui.form("Letter", |ui| {
                if let Some(text) = text_field(app, ui, "letter", "Icon letter", &letter, false, 2)
                {
                    save(
                        app,
                        "icon",
                        "book.update_profile",
                        json!({"icon": {"kind": "letter", "text": text}}),
                    );
                }
            });
        }
        "symbol" => {
            let chosen = match &tile.icon {
                TileIcon::Symbol { symbol } => symbol.clone(),
                _ => String::new(),
            };
            ui.wrap(|ui| {
                for s in SYMBOLS {
                    let on = chosen == s;
                    let name = s.replace('-', " ");
                    ui.tip(name.clone());
                    if ui.icon_button(&format!("sym-{s}"), &name, on).clicked()
                        && !on
                        && save(
                            app,
                            "icon",
                            "book.update_profile",
                            json!({"icon": {"kind": "symbol", "symbol": s}}),
                        )
                    {
                        app.settings.profile.mode = None;
                    }
                }
            });
        }
        "preset" => {
            let chosen = match &tile.icon {
                TileIcon::Preset { preset } => preset.clone(),
                _ => String::new(),
            };
            let color = tile_color(&tile.color);
            ui.wrap(|ui| {
                for p in PRESETS {
                    let on = chosen == p;
                    let name = p.replace('-', " ");
                    ui.tip(name.clone());
                    let resp = ui.item(
                        Item {
                            kind: Kind::PictureChoice,
                            label: name,
                            icon: format!("preset-{p}"),
                            color: Some(color),
                            checked: on,
                            width: 64,
                            ..Item::default()
                        },
                        true,
                    );
                    if resp.clicked()
                        && !on
                        && save(
                            app,
                            "icon",
                            "book.update_profile",
                            json!({"icon": {"kind": "preset", "preset": p}}),
                        )
                    {
                        app.settings.profile.mode = None;
                    }
                }
            });
        }
        _ => {
            let kind = info.at("icon").str("kind").to_owned();
            let editable = kind == "image" && info.at("icon").get("editable") == Some(&json!(true));
            ui.row(|ui| {
                if ui.button("Choose picture…").clicked() {
                    app.settings.profile.errors.retain(|(f, _)| *f != "picture");
                    app.emit(AppEvent::ChooseBookIconFile);
                }
                ui.enabled(editable);
                if ui.button("Edit picture…").clicked() {
                    app.settings.profile.errors.retain(|(f, _)| *f != "picture");
                    if let Err(e) = super::picture::open_stored(app) {
                        app.settings.profile.errors.push(("picture", e.display()));
                    }
                }
                ui.enabled(kind == "image");
                if ui.button("Remove picture").clicked()
                    && save(app, "picture", "book.clear_icon_image", json!({}))
                {
                    app.settings.profile.mode = None;
                }
            });
            ui.caption("PNG or JPEG, up to 2 MB.");
            if let Some(e) = error_of(app, "picture") {
                ui.error_text(e);
            }
            ui.next().bold();
            ui.label(super::picture::TIPS_TITLE);
            super::picture::tips(ui);
        }
    }
    if let Some(e) = error_of(app, "icon") {
        ui.error_text(e);
    }
    let _ = View::Settings;
}

// ----- Advanced: Reset MoneyKeep settings (revision 41) ---------------------------------------

/// What Reset MoneyKeep settings clears.
pub(crate) const RESET_CLEARS: [&str; 6] = [
    "The recent books list and its card pictures (the books stay where they are; open them again with Open a book…)",
    "Window size and layout, and the panels you resized",
    "Theme, text size, fonts and the other display settings; date format and the other operation settings",
    "The start screen's sort order and the Picture editor's tips panel",
    "Backup settings: the folder, the second place and how many to keep (backup files are kept; the default folder applies again)",
    "Locking a book after inactivity",
];

/// What it does not touch.
pub(crate) const RESET_KEEPS: [&str; 3] = [
    "Your books and everything stored inside them (book settings, pictures, history)",
    "Your backup files",
    "Phone capture (it has its own Start over in Settings > Phone capture)",
];

pub(crate) const RESET_LABEL: &str = "Reset MoneyKeep settings…";
pub(crate) const RESET_WORD: &str = "RESET";

/// The name of the saved copy: `settings-before-reset-<date>.json`.
pub(crate) fn reset_copy_name(today: &str) -> String {
    format!("settings-before-reset-{today}.json")
}

fn reset_card(app: &mut App, ui: &mut Ui) {
    ui.card("Reset MoneyKeep settings", |ui| {
        ui.secondary(
            "Puts MoneyKeep's own settings on this computer back to how they were on first run. Your books and backups are not touched.",
        );
        ui.row(|ui| {
            if ui.button(RESET_LABEL).clicked() {
                app.settings.reset = Some(String::new());
            }
        });
    });
}

/// The preview and confirmation (typed `RESET`).
pub(crate) fn reset_dialog(app: &mut App, ui: &mut Ui) {
    if app.settings.reset.is_none() {
        return;
    }
    let name = reset_copy_name(&app.today());
    let place = match &app.settings.folder {
        Some(f) => format!(
            "Your current settings are saved first as {name} in {f}, so they can be restored by hand."
        ),
        None => format!(
            "Your current settings are saved first as {name} next to the settings file, so they can be restored by hand."
        ),
    };
    let mut go = false;
    let mut cancel = ui.dialog_closed("reset-settings") || ui.key("escape");
    ui.dialog("reset-settings", "Reset MoneyKeep settings", 640, |ui| {
        ui.bold();
        ui.label("Cleared on this computer:");
        for c in RESET_CLEARS {
            ui.label(format!("• {c}"));
        }
        ui.bold();
        ui.label("Not touched:");
        for k in RESET_KEEPS {
            ui.label(format!("• {k}"));
        }
        ui.label(place.clone());
        ui.secondary("MoneyKeep then returns to the start screen as on first run.");
        ui.label(format!("Type {RESET_WORD} to confirm:"));
        ui.width(200);
        let typed = app.settings.reset.get_or_insert_with(String::new);
        ui.field("Type RESET to confirm", typed);
        let ready = typed.trim().eq_ignore_ascii_case(RESET_WORD);
        ui.actions(|ui| {
            cancel |= ui.button("Cancel").clicked();
            ui.enabled(ready);
            go = ui.primary("Reset").clicked();
        });
    });
    if go {
        app.settings.reset = None;
        app.emit(AppEvent::ResetSettings { copy_name: name });
    } else if cancel {
        app.settings.reset = None;
    }
}
