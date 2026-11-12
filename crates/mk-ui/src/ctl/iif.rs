//! IIF files (app-desktop.md revision 16): the import review's "Accounts in this
//! file" mapping with classes and names, the file's lists, and Export IIF….

use serde_json::{Value, json};

use super::App;
use super::lists::{accounts_where, names};
use super::ui::{Column, Tone, Ui};
use super::util::Json;
use crate::AppEvent;
use crate::fmt;

const CREATE_AS: [(&str, &str); 5] = [
    ("asset", "Asset"),
    ("liability", "Liability"),
    ("equity", "Equity"),
    ("income", "Income"),
    ("expense", "Expense"),
];

pub(crate) const EXPORT_LIMITS: &str = "IIF keeps dates, accounts, payee names, classes and amounts only: no memos, check numbers or document details. Characters outside ASCII are written as '?'. Transactions in another currency or with securities are left out.";

#[derive(Debug, Clone, Default)]
pub(crate) struct ExportForm {
    pub from: String,
    pub to: String,
    pub accounts: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct IifState {
    pub export: Option<ExportForm>,
    pub lists_chosen: Vec<usize>,
    pub error: Option<String>,
}

fn map(app: &mut App, batch: &str, mut a: Value) {
    if let Some(o) = a.as_object_mut() {
        o.insert("batch".into(), json!(batch));
    }
    app.iif.error = app.run("import.map_name", a).err().map(|e| e.display());
}

pub(crate) fn mapping(app: &mut App, ui: &mut Ui, b: &Value, accounts: &[Value]) {
    let batch = b.str("id").to_owned();
    let all_names = b.list("names").to_vec();
    let staged = b.str("status") == "staged";
    let account_names: Vec<String> = accounts
        .iter()
        .filter(|a| a.at("placeholder").as_bool() != Some(true))
        .map(|a| a.str("full_name").to_owned())
        .collect();
    let cols = ["In the file", "In MoneyKeep", "Map to", "Or create as"]
        .iter()
        .map(|t| Column {
            title: (*t).into(),
            ..Column::default()
        })
        .collect();
    ui.table("Accounts in this file", cols, |ui| {
        for n in all_names.iter().filter(|n| n.str("kind") == "account") {
            let name = n.str("name").to_owned();
            let mut to = None;
            let mut create = None;
            ui.table_row(&name, false, false, |ui| {
                ui.label(name.clone());
                let mapped = n.str("mapped_name");
                if mapped.is_empty() {
                    ui.error_text("Not mapped");
                } else {
                    ui.label(mapped);
                }
                let mut idx = usize::MAX;
                ui.enabled(staged);
                if ui
                    .combo(&format!("Map {name}"), &mut idx, &account_names)
                    .changed()
                {
                    to = account_names.get(idx).cloned();
                }
                if let Some(it) = ui.last() {
                    it.value = "Choose…".into();
                }
                let kinds: Vec<String> = CREATE_AS.iter().map(|(_, l)| (*l).to_owned()).collect();
                let mut k = usize::MAX;
                ui.enabled(staged);
                if ui
                    .combo(&format!("Create {name} as"), &mut k, &kinds)
                    .changed()
                {
                    create = CREATE_AS.get(k).map(|c| c.0.to_owned());
                }
                if let Some(it) = ui.last() {
                    it.value = "Create…".into();
                }
            });
            if let Some(t) = to {
                map(app, &batch, json!({"name": name, "to": t}));
            }
            if let Some(k) = create {
                map(app, &batch, json!({"name": name, "create": k}));
            }
        }
    });
    let classes: Vec<Value> = all_names
        .iter()
        .filter(|n| n.str("kind") == "class")
        .cloned()
        .collect();
    if !classes.is_empty() {
        let options = names(&app.lists().classes.clone(), true);
        let on = app.module_on("classes");
        ui.card("Classes in this file", |ui| {
            if !on {
                ui.secondary("Turn on Classes in Settings, Features, to keep the file's classes.");
            }
            for n in &classes {
                let name = n.str("name").to_owned();
                let shown = n.str("mapped_name").to_owned();
                let mut chosen = None;
                let mut create = false;
                ui.form(&name, |ui| {
                    let mut idx = usize::MAX;
                    ui.enabled(staged && on);
                    if ui
                        .combo(&format!("Class for {name}"), &mut idx, &options)
                        .changed()
                    {
                        chosen = options.get(idx).cloned();
                    }
                    if let Some(it) = ui.last() {
                        it.value = if shown.is_empty() {
                            "Not mapped".into()
                        } else {
                            shown.clone()
                        };
                    }
                    ui.enabled(staged && on && shown.is_empty())
                        .tip(format!("Create the class {name}"));
                    if ui.button("Create class").clicked() {
                        create = true;
                    }
                });
                if let Some(c) = chosen {
                    map(app, &batch, json!({"name": name, "kind": "class", "to": c}));
                }
                if create {
                    map(
                        app,
                        &batch,
                        json!({"name": name, "kind": "class", "create": "class"}),
                    );
                }
            }
        });
    }
    let parties: Vec<Value> = all_names
        .iter()
        .filter(|n| n.str("kind") == "name")
        .cloned()
        .collect();
    if !parties.is_empty() {
        let lists = app.lists().clone();
        let mut options = names(&lists.customers, true);
        options.extend(names(&lists.vendors, true));
        options.sort();
        options.dedup();
        ui.card("Names in this file", |ui| {
            for n in &parties {
                let name = n.str("name").to_owned();
                let shown = n.str("mapped_name").to_owned();
                let mut chosen = None;
                ui.form(&name, |ui| {
                    let mut idx = usize::MAX;
                    ui.enabled(staged && !options.is_empty());
                    if ui
                        .combo(
                            &format!("Customer or vendor for {name}"),
                            &mut idx,
                            &options,
                        )
                        .changed()
                    {
                        chosen = options.get(idx).cloned();
                    }
                    if let Some(it) = ui.last() {
                        it.value = if shown.is_empty() {
                            "Payee only".into()
                        } else {
                            shown.clone()
                        };
                    }
                });
                if let Some(c) = chosen {
                    map(app, &batch, json!({"name": name, "kind": "name", "to": c}));
                }
            }
        });
    }
    lists(app, ui, b, staged);
    if let Some(e) = app.iif.error.clone() {
        ui.error_text(e);
    }
}

fn lists(app: &mut App, ui: &mut Ui, b: &Value, staged: bool) {
    let items = b.list("lists").to_vec();
    if items.is_empty() {
        return;
    }
    ui.card("Lists in this file", |ui| {
        for (i, it) in items.iter().enumerate() {
            let mut on = app.iif.lists_chosen.contains(&i);
            let kind = match it.str("kind") {
                "account" => "Account",
                "customer" => "Customer",
                "vendor" => "Vendor",
                _ => "Class",
            };
            let extra = if it.str("account_type").is_empty() {
                String::new()
            } else {
                format!(" ({})", it.str("account_type"))
            };
            if ui
                .checkbox(&mut on, &format!("{kind} {}{extra}", it.str("name")))
                .clicked()
            {
                if on {
                    app.iif.lists_chosen.push(i);
                } else {
                    app.iif.lists_chosen.retain(|x| *x != i);
                }
            }
        }
        ui.row(|ui| {
            ui.enabled(staged && !app.iif.lists_chosen.is_empty());
            if ui.button("Create selected").clicked() {
                let chosen = app.iif.lists_chosen.clone();
                match app.run(
                    "import.create_lists",
                    json!({"batch": b.str("id"), "items": chosen}),
                ) {
                    Ok(_) => {
                        app.iif.lists_chosen.clear();
                        app.iif.error = None;
                    }
                    Err(e) => app.iif.error = Some(e.display()),
                }
            }
        });
    });
}

#[allow(dead_code)] // Opened from Accounts (next step).
pub(crate) fn open_export(app: &mut App) {
    let to = app.today();
    let from = format!("{}-01-01", to.get(..4).unwrap_or("2026"));
    app.iif.export = Some(ExportForm {
        from: fmt::date(&from),
        to: fmt::date(&to),
        ..ExportForm::default()
    });
}

pub(crate) fn export_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.iif.export.take() else {
        return;
    };
    let all = accounts_where(&app.lists().clone(), |_| true);
    let mut save = false;
    let mut close = ui.dialog_closed("export-iif");
    ui.dialog("export-iif", "Export IIF", 560, |ui| {
        ui.form("From date", |ui| {
            super::register::date_field(ui, "From date", &mut f.from);
        });
        ui.form("To date", |ui| {
            super::register::date_field(ui, "To date", &mut f.to);
        });
        ui.form("Accounts", |ui| {
            if f.accounts.is_empty() {
                ui.label("All accounts");
            }
            let options: Vec<String> = all
                .iter()
                .filter(|a| !f.accounts.contains(a))
                .cloned()
                .collect();
            let mut idx = usize::MAX;
            if ui.combo("Add an account", &mut idx, &options).changed()
                && let Some(a) = options.get(idx)
            {
                f.accounts.push(a.clone());
            }
            if let Some(it) = ui.last() {
                it.value = "Add an account…".into();
            }
        });
        let mut remove = None;
        for (i, a) in f.accounts.iter().enumerate() {
            ui.row(|ui| {
                ui.label(a.clone());
                ui.tip(format!("Remove {a}")).named(format!("Remove {a}"));
                if ui.button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            f.accounts.remove(i);
        }
        ui.tone(Tone::Secondary);
        ui.label(EXPORT_LIMITS);
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                close = true;
            }
            if ui.primary("Export…").clicked() {
                save = true;
            }
        });
    });
    if save {
        let mut a =
            json!({"from": fmt::date_arg_lenient(&f.from), "to": fmt::date_arg_lenient(&f.to)});
        if !f.accounts.is_empty()
            && let Some(o) = a.as_object_mut()
        {
            o.insert("accounts".into(), json!(f.accounts));
        }
        match app.backend.invoke("export.iif", a) {
            Ok(v) => {
                let warnings: Vec<String> = v
                    .list("warnings")
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
                app.emit(AppEvent::SaveFile {
                    file_name: v.str("file_name").to_owned(),
                    content: v.str("content").to_owned(),
                });
                if !warnings.is_empty() {
                    app.notify(warnings.join(" "));
                }
                close = true;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    if !close {
        app.iif.export = Some(f);
    }
}
