//! Starter charts of accounts (chart-presets.md, app-desktop.md revision 34): the
//! preset list with its description panel (Create a new book, Add accounts from a
//! chart…) and the Add accounts from a chart dialog.

use serde_json::{Value, json};

use super::App;
use super::ui::{Align, Column, Tone, Ui};
use super::util::Json;

/// The default selection of the Create a new book dialog.
pub(crate) const DEFAULT_PRESET: &str = "personal";

/// What the preset list and its panel remember.
#[derive(Debug, Clone, Default)]
pub(crate) struct Picker {
    /// The selected preset id.
    pub selected: String,
    /// Show accounts is open.
    pub show_accounts: bool,
}

/// The Add accounts from a chart dialog.
#[derive(Debug, Clone, Default)]
pub(crate) struct AddChart {
    pub picker: Picker,
    /// Codes the user unticked.
    pub unticked: Vec<String>,
    /// The preview for `picker.selected`: (preset id, result).
    pub preview: Option<(String, Value)>,
    pub error: Option<String>,
}

/// Module ids as words ("sales-tax" → "Sales tax").
fn module_words(ids: &[Value]) -> String {
    let words: Vec<String> = ids
        .iter()
        .filter_map(Value::as_str)
        .map(super::util::display_name)
        .collect();
    words.join(", ")
}

/// The built-in presets (cached for the app's life: they are compiled in).
pub(crate) fn presets(app: &mut App) -> Vec<Value> {
    if app.chart_presets.is_none() {
        match app.backend.chart_presets() {
            Ok(v) => app.chart_presets = Some(v.as_array().cloned().unwrap_or_default()),
            Err(e) => {
                app.notify_error(&e);
                return Vec::new();
            }
        }
    }
    app.chart_presets.clone().unwrap_or_default()
}

/// The preset list and the selected preset's panel. Returns whether the selection
/// changed.
pub(crate) fn picker(ui: &mut Ui, presets: &[Value], p: &mut Picker) -> bool {
    if !presets.iter().any(|x| x.str("id") == p.selected) {
        p.selected = presets
            .iter()
            .find(|x| x.str("id") == DEFAULT_PRESET)
            .or_else(|| presets.first())
            .map(|x| x.str("id").to_owned())
            .unwrap_or_default();
    }
    let mut changed = false;
    let cols = vec![
        Column {
            title: "Chart".into(),
            width: 236,
            ..Column::default()
        },
        Column {
            title: "What it holds".into(),
            weight: 1,
            ..Column::default()
        },
    ];
    ui.table("Chart of accounts", cols, |ui| {
        for x in presets {
            let id = x.str("id").to_owned();
            let name = x.str("name").to_owned();
            let selected = id == p.selected;
            let r = ui.table_row(&format!("Chart {name}"), selected, true, |ui| {
                ui.bold();
                ui.label(name.clone());
                ui.tone(Tone::Secondary);
                ui.label(x.str("blurb"));
            });
            if r.clicked() && !selected {
                p.selected = id;
                p.show_accounts = false;
                changed = true;
            }
        }
        // The list scrolls, so the selected chart's panel stays in view below it.
        ui.visible_rows(5);
    });
    let Some(sel) = presets.iter().find(|x| x.str("id") == p.selected) else {
        return changed;
    };
    let name = sel.str("name").to_owned();
    ui.card(&name, |ui| {
        for line in sel.str("description").lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            ui.next().grow();
            ui.label(line);
        }
        let mut facts = vec![format!(
            "{} accounts",
            sel.at("account_count").as_u64().unwrap_or(0)
        )];
        if let Some(y) = sel.at("tax_year").as_i64() {
            facts.push(format!("Tax lines for {y}"));
        }
        let mods = sel.list("modules");
        if !mods.is_empty() {
            facts.push(format!("Also turns on: {}", module_words(mods)));
        }
        ui.secondary(facts.join(" · "));
        ui.row(|ui| {
            let text = if p.show_accounts {
                "Hide accounts"
            } else {
                "Show accounts"
            };
            ui.named(format!("{text} of {name}"));
            if ui.link(text).clicked() {
                p.show_accounts = !p.show_accounts;
            }
        });
    });
    if p.show_accounts {
        accounts_tree(ui, &name, sel.list("accounts"));
    }
    changed
}

/// The read-only tree of a preset's accounts: code, name, tax line.
fn accounts_tree(ui: &mut Ui, name: &str, accounts: &[Value]) {
    let cols = vec![
        Column {
            title: "Code".into(),
            width: 56,
            ..Column::default()
        },
        Column {
            title: "Account".into(),
            weight: 2,
            ..Column::default()
        },
        Column {
            title: "Tax line".into(),
            weight: 1,
            ..Column::default()
        },
    ];
    ui.table(&format!("Accounts of {name}"), cols, |ui| {
        for a in accounts {
            let child = !a.at("parent").is_null();
            ui.table_row(
                &format!("preset-account:{}", a.str("code")),
                false,
                false,
                |ui| {
                    ui.tone(Tone::Secondary).mono();
                    ui.label(a.str("code"));
                    if a.at("header").as_bool() == Some(true) {
                        ui.bold();
                    }
                    ui.label(a.str("name"));
                    ui.tone(Tone::Secondary);
                    ui.label(a.str("tax_line"));
                },
            );
            ui.indent_last(i32::from(child));
        }
        ui.visible_rows(10);
    });
}

// ----- Add accounts from a chart… ------------------------------------------------

pub(crate) fn open_add(app: &mut App) {
    app.add_chart = Some(AddChart {
        picker: Picker {
            selected: DEFAULT_PRESET.into(),
            show_accounts: false,
        },
        ..AddChart::default()
    });
}

pub(crate) fn add_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.add_chart.take() else {
        return;
    };
    let list = presets(app);
    let mut close = ui.dialog_closed("add-chart");
    ui.dialog("add-chart", "Add accounts from a chart", 760, |ui| {
        if picker(ui, &list, &mut d.picker) {
            d.unticked.clear();
            d.error = None;
        }
        let want = d.picker.selected.clone();
        if d.preview.as_ref().is_none_or(|(id, _)| *id != want) {
            match app.read_or_error("chart.preview_add", json!({"preset": want})) {
                Ok(v) => d.preview = Some((want.clone(), v)),
                Err(e) => d.error = Some(e),
            }
        }
        if let Some((_, pv)) = d.preview.clone() {
            preview_table(ui, &pv, &mut d.unticked);
            let adds = pv.at("adds").as_u64().unwrap_or(0);
            let unticked = u64::try_from(d.unticked.len()).unwrap_or(0);
            let mut summary = pv.str("summary").to_owned();
            if unticked > 0 {
                summary.push_str(&format!(" ({unticked} unticked)"));
            }
            ui.secondary(summary);
            if let Some(e) = &d.error {
                ui.error_text(e.clone());
            }
            ui.actions(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                ui.enabled(adds > unticked);
                if ui.primary("Add accounts").clicked() {
                    match app.run("chart.add", json!({"preset": want, "exclude": d.unticked})) {
                        Ok(_) => close = true,
                        Err(e) => d.error = Some(e.display()),
                    }
                }
            });
        } else {
            if let Some(e) = &d.error {
                ui.error_text(e.clone());
            }
            ui.actions(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        }
    });
    if !close {
        app.add_chart = Some(d);
    }
}

/// The preview: a check box per account to add, skipped accounts in secondary
/// text with their reason.
fn preview_table(ui: &mut Ui, pv: &Value, unticked: &mut Vec<String>) {
    let cols = vec![
        Column {
            title: "Add".into(),
            width: 48,
            align: Align::Center,
            ..Column::default()
        },
        Column {
            title: "Code".into(),
            width: 56,
            ..Column::default()
        },
        Column {
            title: "Account".into(),
            weight: 2,
            ..Column::default()
        },
        Column {
            title: "Tax line or reason".into(),
            weight: 2,
            ..Column::default()
        },
    ];
    let mut toggled: Option<(String, bool)> = None;
    ui.table("Accounts to add", cols, |ui| {
        for a in pv.list("accounts") {
            let code = a.str("code").to_owned();
            let full = a.str("full_name").to_owned();
            let skip = a.str("action") == "skip";
            ui.table_row(&format!("add-row:{code}"), false, false, |ui| {
                if skip {
                    ui.label("");
                } else {
                    let mut on = !unticked.contains(&code);
                    ui.named(format!("Add {full}"));
                    if ui.checkbox(&mut on, "").clicked() {
                        toggled = Some((code.clone(), on));
                    }
                }
                ui.tone(Tone::Secondary).mono();
                ui.label(code.clone());
                if skip {
                    ui.tone(Tone::Secondary);
                }
                ui.tip(full.clone());
                ui.label(full.clone());
                ui.tone(Tone::Secondary);
                if skip {
                    ui.label(format!("Skipped: {}", a.str("reason")));
                } else {
                    ui.label(a.str("tax_line"));
                }
            });
        }
        ui.visible_rows(12);
    });
    if let Some((code, on)) = toggled {
        if on {
            unticked.retain(|c| *c != code);
        } else if !unticked.contains(&code) {
            unticked.push(code);
        }
    }
}
