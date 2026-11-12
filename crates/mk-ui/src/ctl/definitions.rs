//! Definitions screens (app-desktop.md revision 12): Settings → Definitions, the
//! install review, the uninstall confirmation and the Create definition file form.
//! Everything shown comes from the `definition.*` commands; the homepage is plain
//! text with a Copy button, never a link.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use base64::Engine;
use serde_json::{Value, json};

use super::form::set;
use super::ui::Ui;
use super::util::Json;
use super::{App, AppEvent, MessageAction};

pub(crate) const BANNER: &str = "Definition files contain import layouts, rules, categories and securities. They cannot run programs or change your transactions. Review what will be added before installing.";

/// An open install review.
#[derive(Debug, Clone)]
pub(crate) struct Review {
    pub file_name: String,
    pub content_base64: String,
    pub inspect: Value,
    /// "kind:key" → (choice, new name).
    pub choices: HashMap<String, (String, String)>,
    /// "kind:key" of items whose fields are shown.
    pub fields_open: BTreeSet<String>,
}

/// The Create definition file form.
#[derive(Debug, Clone, Default)]
pub(crate) struct CreateForm {
    pub id: String,
    pub version: String,
    pub name: String,
    pub description: String,
    pub authors: String,
    pub license: String,
    pub tags: String,
    pub homepage: String,
    pub search: String,
    /// list name → chosen entries (rule ids, names, paths, symbols).
    pub chosen: BTreeMap<String, BTreeSet<String>>,
    pub candidates: Value,
    pub result: Option<Result<String, String>>,
}

#[derive(Debug, Default)]
pub(crate) struct DefState {
    pub review: Option<Review>,
    pub create: Option<CreateForm>,
    /// Package JSON awaiting uninstall confirmation.
    pub uninstall: Option<Value>,
    /// Package id whose details are shown.
    pub details: Option<String>,
}

fn key_of(item: &Value) -> String {
    format!("{}:{}", item.str("kind"), item.str("key"))
}

fn kind_title(kind: &str) -> &'static str {
    match kind {
        "category" => "Categories",
        "security" => "Securities",
        "bank_profile" => "Bank profiles",
        "investment_profile" => "Broker profiles",
        "rule" => "Rules",
        _ => "Other",
    }
}

fn action_word(a: &str) -> &'static str {
    match a {
        "add" => "Add",
        "update" => "Update",
        "conflict" => "Conflict",
        _ => "Unchanged",
    }
}

/// Open the install review for a `.mkdef` file (dialog or drop).
pub(crate) fn open_file(app: &mut App, name: &str, bytes: &[u8]) {
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    match app.run(
        "definition.inspect",
        json!({"content_base64": b64, "name": name}),
    ) {
        Ok(v) => {
            let choices = v
                .list("items")
                .iter()
                .map(|i| (key_of(i), (i.str("default").to_owned(), String::new())))
                .collect();
            app.defs.review = Some(Review {
                file_name: name.to_owned(),
                content_base64: b64,
                inspect: v,
                choices,
                fields_open: BTreeSet::new(),
            });
        }
        Err(e) => app.notify_error(&e),
    }
}

/// Draw any open definitions dialog.
pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    review_dialog(app, ui);
    create_dialog(app, ui);
    uninstall_dialog(app, ui);
    details_dialog(app, ui);
}

fn join(h: &Value, k: &str) -> String {
    h.list(k)
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn review_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut r) = app.defs.review.take() else {
        return;
    };
    let mut install = false;
    let mut cancel = ui.dialog_closed("definition-review") || ui.key("escape");
    let mut copy = None;
    let v = r.inspect.clone();
    let valid = v.at("valid").as_bool() == Some(true);
    let h = v.at("header");
    let title = if valid {
        format!("Install {} {}", h.str("name"), h.str("version"))
    } else {
        format!("Install {}", r.file_name)
    };
    ui.dialog("definition-review", &title, 660, |ui| {
        ui.bold();
        ui.label(BANNER);
        if valid {
            ui.group(|ui| {
                let row = |ui: &mut Ui, label: &str, text: String| {
                    if !text.is_empty() {
                        ui.form(label, |ui| ui.label(text));
                    }
                };
                row(ui, "Id", h.str("id").to_owned());
                row(ui, "Authors", join(h, "authors"));
                row(ui, "License", h.str("license").to_owned());
                row(ui, "Created", h.str("created").to_owned());
                row(ui, "Tags", join(h, "tags"));
                if !h.str("homepage").is_empty() {
                    ui.form("Homepage", |ui| {
                        // Plain text: never a link.
                        ui.label(h.str("homepage"));
                        ui.named(format!("Copy {}", h.str("homepage")));
                        if ui.button("Copy").clicked() {
                            copy = Some(h.str("homepage").to_owned());
                        }
                    });
                }
                let sha = v.str("sha256");
                ui.form("SHA-256", |ui| {
                    ui.tip(sha);
                    ui.label(sha.get(..16).unwrap_or(sha));
                });
            });
            if !h.str("description").is_empty() {
                ui.label(h.str("description"));
            }
            ui.bold();
            ui.label(v.str("summary"));
        }
        for e in v.list("errors") {
            ui.error_text(e.as_str().unwrap_or(""));
        }
        for w in v.list("warnings") {
            ui.secondary(format!("Warning: {}", w.as_str().unwrap_or("")));
        }
        let mut groups: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
        for i in v.list("items") {
            groups.entry(i.str("kind")).or_default().push(i);
        }
        for (kind, items) in groups {
            ui.heading(kind_title(kind));
            for i in items {
                item_row(ui, &mut r, i);
            }
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            let errors = !valid || !v.list("errors").is_empty();
            ui.enabled(!errors);
            if ui.primary("Install selected").clicked() {
                install = true;
            }
        });
    });
    if let Some(c) = copy {
        app.copy(c);
    }
    if cancel {
        return;
    }
    if install {
        let choices: Vec<Value> = r
            .choices
            .iter()
            .filter_map(|(k, (choice, name))| {
                let (kind, key) = k.split_once(':')?;
                let mut c = json!({"kind": kind, "key": key, "choice": choice});
                if choice == "rename" {
                    set(&mut c, "name", json!(name));
                }
                Some(c)
            })
            .collect();
        let args = json!({"content_base64": r.content_base64, "name": r.file_name,
                          "sha256": v.str("sha256"), "choices": choices});
        match app.run("definition.install", args) {
            Ok(out) => {
                if out.at("installed").as_u64().unwrap_or(0) > 0 {
                    app.notify_with(out.str("message"), MessageAction::Undo);
                } else {
                    app.notify(out.str("message"));
                }
            }
            Err(e) => {
                app.notify_error(&e);
                app.defs.review = Some(r);
            }
        }
        return;
    }
    app.defs.review = Some(r);
}

fn item_row(ui: &mut Ui, r: &mut Review, i: &Value) {
    let key = key_of(i);
    let action = i.str("action");
    let (choice, rename) = r.choices.get(&key).cloned().unwrap_or_default();
    ui.scope(key.clone(), |ui| {
        ui.wrap(|ui| {
            match action {
                "unchanged" => {
                    let mut off = false;
                    ui.enabled(false);
                    ui.checkbox(&mut off, i.str("name"));
                }
                "conflict" => {
                    let mut on = choice != "skip";
                    if ui.checkbox(&mut on, i.str("name")).changed() {
                        let c = if on { "replace" } else { "skip" };
                        r.choices.insert(key.clone(), (c.into(), rename.clone()));
                    }
                }
                _ => {
                    let mut on = choice == "install";
                    if ui.checkbox(&mut on, i.str("name")).changed() {
                        let c = if on { "install" } else { "skip" };
                        r.choices.insert(key.clone(), (c.into(), rename.clone()));
                    }
                }
            }
            ui.secondary(action_word(action));
            if action == "conflict" {
                let opts = [
                    ("replace", "Replace"),
                    ("rename", "Rename…"),
                    ("skip", "Skip"),
                ];
                let mut sel = opts.iter().position(|o| o.0 == choice).unwrap_or(2);
                let labels: Vec<String> = opts.iter().map(|o| o.1.to_owned()).collect();
                ui.width(140);
                if ui
                    .combo(
                        &format!("Conflict choice for {}", i.str("name")),
                        &mut sel,
                        &labels,
                    )
                    .changed()
                {
                    let c = opts.get(sel).map_or("skip", |o| o.0);
                    r.choices.insert(key.clone(), (c.into(), rename.clone()));
                }
                if choice == "rename" {
                    let mut name = rename.clone();
                    ui.width(200);
                    if ui
                        .field(&format!("New name for {}", i.str("name")), &mut name)
                        .changed()
                    {
                        r.choices.insert(key.clone(), ("rename".into(), name));
                    }
                }
            }
            for n in i.list("notes") {
                ui.secondary(n.as_str().unwrap_or(""));
            }
        });
        let mut open = r.fields_open.contains(&key);
        let was = open;
        ui.collapsing("Fields", &mut open, |ui| {
            let text = serde_json::to_string_pretty(i.at("fields")).unwrap_or_default();
            ui.mono();
            ui.label(text);
        });
        if open != was {
            if open {
                r.fields_open.insert(key.clone());
            } else {
                r.fields_open.remove(&key);
            }
        }
    });
}

// ----- Settings → Definitions -------------------------------------------------------------

pub(crate) fn settings_section(app: &mut App, ui: &mut Ui) {
    ui.card("Definitions", |ui| {
        ui.row(|ui| {
            if ui.button("Install from file…").clicked() {
                app.emit(AppEvent::ChooseDefinitionFile);
            }
            if ui.button("Create definition file…").clicked() {
                open_create(app);
            }
        });
        ui.secondary(
            "Definition files (.mkdef) share import layouts, rules, categories and securities. You can also drop one on the window.",
        );
    });
    let list = super::util::array(app.read("definition.list", json!({})));
    if list.is_empty() {
        ui.card("", |ui| ui.label("No definition packages are installed."));
    }
    for p in list {
        ui.card(&format!("{} {}", p.str("name"), p.str("version")), |ui| {
            let counts: Vec<String> = p
                .at("counts")
                .as_object()
                .map(|o| {
                    o.iter()
                        .map(|(k, n)| format!("{n} {}", kind_title(k).to_lowercase()))
                        .collect()
                })
                .unwrap_or_default();
            ui.secondary(format!(
                "{} · installed {} · {}",
                p.str("id"),
                crate::fmt::date(p.str("installed_at").get(..10).unwrap_or("")),
                counts.join(", ")
            ));
            let edited = p.list("edited").len();
            if edited > 0 {
                ui.label(format!(
                    "{edited} item{} edited since install",
                    if edited == 1 { "" } else { "s" }
                ));
            }
            ui.scope(p.str("id").to_owned(), |ui| {
                ui.row(|ui| {
                    if ui.button("Details").clicked() {
                        app.defs.details = Some(p.str("id").to_owned());
                    }
                    if ui.button("Export again…").clicked() {
                        export_again(app, &p);
                    }
                    if ui.button("Uninstall…").clicked() {
                        app.defs.uninstall = Some(p.clone());
                    }
                });
            });
        });
    }
}

/// Names of a package's items of one kind, as `definition.export` takes them.
fn names_of(p: &Value, kind: &str) -> Vec<String> {
    p.list("items")
        .iter()
        .filter(|i| {
            i.str("kind") == kind && i.at("keep_reason").as_str() != Some("no longer in the book")
        })
        .map(|i| {
            if kind == "rule" || kind == "dashboard" {
                i.str("object").to_owned()
            } else {
                i.str("name").to_owned()
            }
        })
        .collect()
}

fn export_again(app: &mut App, p: &Value) {
    let h = p.at("header");
    let mut args = json!({
        "id": p.str("id"), "version": p.str("version"), "name": p.str("name"),
        "bank_profiles": names_of(p, "bank_profile"),
        "investment_profiles": names_of(p, "investment_profile"),
        "rules": names_of(p, "rule"),
        "categories": names_of(p, "category"),
        "securities": names_of(p, "security"),
        "dashboards": names_of(p, "dashboard"),
        "authors": h.at("authors"), "tags": h.at("tags"),
    });
    for k in ["description", "license", "homepage"] {
        if let Some(s) = h.at(k).as_str() {
            set(&mut args, k, json!(s));
        }
    }
    match app.run("definition.export", args) {
        Ok(v) => app.emit(AppEvent::SaveFile {
            file_name: v.str("file_name").to_owned(),
            content: v.str("content").to_owned(),
        }),
        Err(e) => app.notify_error(&e),
    }
}

fn uninstall_dialog(app: &mut App, ui: &mut Ui) {
    let Some(p) = app.defs.uninstall.take() else {
        return;
    };
    let mut confirm = false;
    let mut cancel = ui.dialog_closed("definition-uninstall") || ui.key("escape");
    ui.dialog(
        "definition-uninstall",
        &format!("Uninstall {}", p.str("name")),
        520,
        |ui| {
            let (gone, kept): (Vec<&Value>, Vec<&Value>) = p
                .list("items")
                .iter()
                .partition(|i| i.at("keep_reason").is_null());
            ui.heading("Will be removed");
            for i in &gone {
                ui.label(format!("{}: {}", kind_title(i.str("kind")), i.str("name")));
            }
            if !kept.is_empty() {
                ui.heading("Kept");
                for i in &kept {
                    ui.label(format!(
                        "{}: {} ({})",
                        kind_title(i.str("kind")),
                        i.str("name"),
                        i.str("keep_reason")
                    ));
                }
            }
            ui.actions(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                if ui.primary("Uninstall").clicked() {
                    confirm = true;
                }
            });
        },
    );
    if confirm {
        match app.run("definition.uninstall", json!({"id": p.str("id")})) {
            Ok(v) => app.notify_with(
                format!(
                    "Uninstalled {}: {} removed, {} kept",
                    p.str("name"),
                    v.list("removed").len(),
                    v.list("kept").len()
                ),
                MessageAction::Undo,
            ),
            Err(e) => app.notify_error(&e),
        }
    } else if !cancel {
        app.defs.uninstall = Some(p);
    }
}

fn details_dialog(app: &mut App, ui: &mut Ui) {
    let Some(id) = app.defs.details.take() else {
        return;
    };
    let Some(p) = super::util::array(app.read("definition.list", json!({})))
        .into_iter()
        .find(|p| p.str("id") == id)
    else {
        return;
    };
    let mut close = ui.dialog_closed("definition-details") || ui.key("escape");
    ui.dialog(
        "definition-details",
        &format!("{} {}", p.str("name"), p.str("version")),
        560,
        |ui| {
            ui.label(format!("Id: {}", p.str("id")));
            if !p.str("file_name").is_empty() {
                ui.label(format!("File: {}", p.str("file_name")));
            }
            ui.row(|ui| {
                ui.label("SHA-256:");
                ui.mono();
                ui.label(p.str("sha256"));
            });
            for i in p.list("items") {
                let mut line = format!("{}: {}", kind_title(i.str("kind")), i.str("name"));
                if i.at("edited").as_bool() == Some(true) {
                    line.push_str(" (edited since install)");
                }
                ui.label(line);
            }
            ui.actions(|ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        },
    );
    if !close {
        app.defs.details = Some(id);
    }
}

// ----- Create definition file -------------------------------------------------------------

fn open_create(app: &mut App) {
    let candidates = app
        .read("definition.candidates", json!({}))
        .unwrap_or(Value::Null);
    app.defs.create = Some(CreateForm {
        version: "1.0.0".into(),
        candidates,
        ..CreateForm::default()
    });
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
        .collect()
}

fn export_args(f: &CreateForm) -> Value {
    let chosen = |k: &str| {
        f.chosen
            .get(k)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>()
    };
    let mut args = json!({
        "id": f.id.trim(), "version": f.version.trim(), "name": f.name.trim(),
        "authors": split_list(&f.authors), "tags": split_list(&f.tags),
        "bank_profiles": chosen("bank_profiles"),
        "investment_profiles": chosen("investment_profiles"),
        "rules": chosen("rules"),
        "categories": chosen("categories"),
        "securities": chosen("securities"),
        "dashboards": chosen("dashboards"),
    });
    for (k, v) in [
        ("description", &f.description),
        ("license", &f.license),
        ("homepage", &f.homepage),
    ] {
        if !v.trim().is_empty() {
            set(&mut args, k, json!(v.trim()));
        }
    }
    args
}

fn checklist(ui: &mut Ui, f: &mut CreateForm, list: &str, title: &str) {
    let entries = f.candidates.list(list).to_vec();
    let search = f.search.to_lowercase();
    ui.heading(title);
    let mut any = false;
    ui.scope(list.to_owned(), |ui| {
        for e in &entries {
            let (id, label, ok, reason) = match e {
                Value::String(s) => (s.clone(), s.clone(), true, String::new()),
                other => (
                    other.str("id").to_owned(),
                    other.str("name").to_owned(),
                    other.at("ok").as_bool() != Some(false),
                    other.str("reason").to_owned(),
                ),
            };
            if !search.is_empty() && !label.to_lowercase().contains(&search) {
                continue;
            }
            any = true;
            let chosen = f.chosen.entry(list.to_owned()).or_default();
            let mut on = chosen.contains(&id);
            ui.row(|ui| {
                ui.enabled(ok);
                if ui.checkbox(&mut on, &label).changed() {
                    if on {
                        chosen.insert(id.clone());
                    } else {
                        chosen.remove(&id);
                    }
                }
                if !ok {
                    ui.secondary(format!("Cannot be shared: {reason}"));
                }
            });
        }
    });
    if !any {
        ui.secondary("None");
    }
}

fn create_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.defs.create.take() else {
        return;
    };
    let (mut validate, mut save) = (false, false);
    let mut cancel = ui.dialog_closed("definition-create") || ui.key("escape");
    ui.dialog("definition-create", "Create definition file", 640, |ui| {
        ui.group(|ui| {
            for (label, field, hint) in [
                ("Id", &mut f.id, "org.example.my-layouts"),
                ("Version", &mut f.version, "1.0.0"),
                ("Name", &mut f.name, ""),
                ("Description", &mut f.description, ""),
                ("Authors", &mut f.authors, "comma-separated"),
                ("License", &mut f.license, "e.g. CC0-1.0"),
                ("Tags", &mut f.tags, "comma-separated"),
                ("Homepage", &mut f.homepage, "https://"),
            ] {
                super::form::text_row(ui, label, field, hint);
            }
        });
        ui.row(|ui| {
            ui.width(260);
            ui.field("Search items", &mut f.search);
        });
        checklist(ui, &mut f, "bank_profiles", "Bank profiles");
        checklist(ui, &mut f, "investment_profiles", "Broker profiles");
        checklist(ui, &mut f, "rules", "Rules");
        checklist(ui, &mut f, "categories", "Category subtrees");
        checklist(ui, &mut f, "securities", "Securities");
        checklist(ui, &mut f, "dashboards", "Dashboards");
        match &f.result {
            Some(Ok(msg)) => ui.label(msg.clone()),
            Some(Err(e)) => ui.error_text(e.clone()),
            None => {}
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.button("Validate").clicked() {
                validate = true;
            }
            if ui.primary("Save as…").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if validate || save {
        match app.backend.invoke("definition.export", export_args(&f)) {
            Ok(v) => {
                let warnings: Vec<&str> = v
                    .list("warnings")
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                f.result = Some(Ok(if warnings.is_empty() {
                    format!("Valid: {} items", v.at("items"))
                } else {
                    format!("Valid with warnings: {}", warnings.join("; "))
                }));
                if save {
                    app.emit(AppEvent::SaveFile {
                        file_name: v.str("file_name").to_owned(),
                        content: v.str("content").to_owned(),
                    });
                    return;
                }
            }
            Err(e) => f.result = Some(Err(e.display())),
        }
    }
    app.defs.create = Some(f);
}
