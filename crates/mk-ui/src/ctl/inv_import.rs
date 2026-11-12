//! Investment import (app-desktop.md revision 11, import-investments.md): the
//! Securities panel, the investment rows table, Enter basis… / Move shares in
//! first…, the Holdings check, the profile editor for broker files, and Settings
//! → Import → Investment profiles.

use std::collections::{HashMap, HashSet};

use base64::Engine;
use serde_json::{Value, json};

use super::App;
use super::ui::{Align, Column, Tone, Ui};
use super::util::{Json, array};
use crate::View;
use crate::fmt;

const SEC_KINDS: [(&str, &str); 7] = [
    ("stock", "Stock"),
    ("etf", "ETF"),
    ("mutual_fund", "Mutual fund"),
    ("bond", "Bond"),
    ("cd", "CD"),
    ("money_market", "Money market"),
    ("other", "Other"),
];

const METHODS: [(&str, &str); 3] = [
    ("fifo", "First in, first out"),
    ("average", "Average cost"),
    ("specific", "Specific lots"),
];

const CHOICES: [(&str, &str, &str); 22] = [
    ("buy", "", "Buy"),
    ("sell", "", "Sell"),
    ("dividend", "ordinary", "Dividend — ordinary"),
    ("dividend", "qualified", "Dividend — qualified"),
    (
        "dividend",
        "capital_gain_long",
        "Dividend — capital gain long",
    ),
    (
        "dividend",
        "capital_gain_short",
        "Dividend — capital gain short",
    ),
    ("dividend", "tax_exempt", "Dividend — tax exempt"),
    ("reinvest", "", "Reinvest"),
    ("interest", "", "Interest"),
    ("fee", "", "Fee"),
    ("shares_in", "", "Shares in"),
    ("shares_out", "", "Shares out"),
    ("split", "", "Split"),
    ("return_of_capital", "", "Return of capital"),
    ("cash", "", "Cash"),
    ("ignore", "", "Ignore"),
    ("sell_quantity", "", "Maturity: position change"),
    ("sell_proceeds", "", "Maturity: cash"),
    ("merger", "", "Merger"),
    ("spinoff", "", "Spin-off"),
    ("symbol_change", "", "Symbol change"),
    ("cash_in_lieu", "", "Cash in lieu"),
];

const MATCH_TYPES: [(&str, &str); 3] = [
    ("equals", "equals"),
    ("starts_with", "starts with"),
    ("contains", "contains"),
];

const DATE_FORMATS: [&str; 7] = [
    "YYYY-MM-DD",
    "MM/DD/YYYY",
    "DD/MM/YYYY",
    "M/D/YYYY",
    "D/M/YYYY",
    "DD.MM.YYYY",
    "YYYYMMDD",
];

const FIELDS: [(&str, &str); 13] = [
    ("date", "Date"),
    ("action", "Action"),
    ("symbol", "Symbol"),
    ("cusip", "CUSIP"),
    ("security_name", "Security name"),
    ("quantity", "Shares"),
    ("price", "Price"),
    ("commission", "Commission"),
    ("fees", "Fees"),
    ("amount", "Amount"),
    ("description", "Description"),
    ("external_id", "Transaction id"),
    ("account", "Account"),
];

#[derive(Debug, Clone, Default)]
pub(crate) struct ProfileEditor {
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub format: String,
    pub records: String,
    pub sample: Vec<(String, String)>,
    pub keys: Vec<String>,
    pub fields: HashMap<String, String>,
    pub date_format: usize,
    pub decimal_comma: bool,
    pub actions: Vec<(String, Option<usize>, usize)>,
    pub signature: Value,
    pub name: String,
    pub preview_key: String,
    pub preview: Value,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct InvState {
    pub editor: Option<ProfileEditor>,
    pub edits: HashMap<String, (String, String)>,
    pub basis_form: Option<(String, String, String, bool)>,
    pub changing: HashSet<String>,
}

fn set(v: &mut Value, k: &str, x: Value) {
    if let Some(o) = v.as_object_mut() {
        o.insert(k.to_owned(), x);
    }
}

pub(crate) fn editing(app: &App) -> bool {
    app.import.inv.editor.is_some()
}

pub(crate) fn open_editor(app: &mut App, name: &str, bytes: &[u8], s: &Value) {
    let mut fields = HashMap::new();
    if let Some(o) = s.at("fields").as_object() {
        for (k, v) in o {
            fields.insert(k.clone(), v.as_str().unwrap_or("").to_owned());
        }
    }
    let df = s.str("date_format");
    let ed = ProfileEditor {
        file_name: name.to_owned(),
        bytes: bytes.to_vec(),
        format: s.str("format").to_owned(),
        records: s.str("records").to_owned(),
        sample: s
            .list("sample")
            .iter()
            .filter_map(|p| {
                let a = p.as_array()?;
                Some((
                    a.first()?.as_str()?.to_owned(),
                    a.get(1)?.as_str()?.to_owned(),
                ))
            })
            .collect(),
        keys: s
            .list("keys")
            .iter()
            .filter_map(|k| k.as_str().map(str::to_owned))
            .collect(),
        fields,
        date_format: DATE_FORMATS.iter().position(|f| *f == df).unwrap_or(1),
        actions: s
            .list("action_texts")
            .iter()
            .filter_map(|t| t.as_str().map(|t| (t.to_owned(), None, 0)))
            .collect(),
        signature: s.at("profile").at("signature").clone(),
        ..ProfileEditor::default()
    };
    app.import.inv.editor = Some(ed);
    app.import.committed = None;
    app.navigate(View::Import {
        batch: String::new(),
    });
}

fn profile_json(ed: &ProfileEditor) -> Value {
    let fields: serde_json::Map<String, Value> = ed
        .fields
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.clone(), json!(v)))
        .collect();
    let actions: Vec<Value> = ed
        .actions
        .iter()
        .filter_map(|(text, choice, mt)| {
            let (action, kind, _) = CHOICES.get((*choice)?)?;
            let mut r = json!({
                "match_type": MATCH_TYPES.get(*mt).map_or("equals", |m| m.0),
                "text": text,
                "action": action,
            });
            if !kind.is_empty() {
                set(&mut r, "kind", json!(kind));
            }
            Some(r)
        })
        .collect();
    json!({
        "name": if ed.name.trim().is_empty() { "New profile" } else { ed.name.trim() },
        "format": ed.format,
        "signature": ed.signature,
        "records": ed.records,
        "fields": fields,
        "date_format": DATE_FORMATS.get(ed.date_format).copied().unwrap_or("MM/DD/YYYY"),
        "number_format": if ed.decimal_comma {
            json!({"decimal": ",", "thousands": "."})
        } else {
            json!({"decimal": ".", "thousands": ","})
        },
        "actions": actions,
    })
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

pub(crate) fn profile_editor(app: &mut App, ui: &mut Ui) {
    let Some(mut ed) = app.import.inv.editor.take() else {
        return;
    };
    ui.title(format!("Read {}", ed.file_name));
    ui.secondary("Tell MoneyKeep where each value is in this broker file. The preview shows the rows as they will be read.");
    let mut save = false;
    let mut cancel = false;
    ui.card("File", |ui| {
        ui.form("Format", |ui| ui.label(ed.format.to_uppercase()));
        if ed.format != "csv" {
            ui.form("Record path", |ui| {
                ui.width(320);
                ui.field("Record path", &mut ed.records);
            });
        }
        ui.form("Date format", |ui| {
            ui.combo("Date format", &mut ed.date_format, &strings(&DATE_FORMATS));
        });
        ui.form("Numbers", |ui| {
            ui.checkbox(&mut ed.decimal_comma, "Comma is the decimal point");
        });
    });
    ui.card("First record", |ui| {
        for (k, v) in ed.sample.iter().take(16) {
            ui.secondary(format!("{k}: {v}"));
        }
    });
    ui.card("Fields", |ui| {
        let mut keys: Vec<String> = vec!["(not used)".to_owned()];
        keys.extend(ed.keys.iter().cloned());
        for (field, label) in FIELDS {
            ui.form(label, |ui| {
                let current = ed.fields.get(field).cloned().unwrap_or_default();
                let mut idx = keys.iter().position(|k| *k == current).unwrap_or(0);
                ui.width(240);
                ui.combo(&format!("Field {label}"), &mut idx, &keys);
                let chosen = if idx == 0 {
                    String::new()
                } else {
                    keys.get(idx).cloned().unwrap_or_default()
                };
                ed.fields.insert(field.to_owned(), chosen);
            });
        }
    });
    ui.card("Actions", |ui| {
        if ed.actions.is_empty() {
            ui.secondary("Choose the Action field to list the file's action texts.");
        }
        let mut labels: Vec<String> = vec!["(not mapped)".into()];
        labels.extend(CHOICES.iter().map(|c| c.2.to_owned()));
        let mts: Vec<String> = MATCH_TYPES.iter().map(|m| m.1.to_owned()).collect();
        for (text, choice, mt) in ed.actions.iter_mut() {
            ui.form(text.as_str(), |ui| {
                let mut idx = choice.map_or(0, |c| c + 1);
                ui.width(240);
                ui.combo(&format!("Action for {text}"), &mut idx, &labels);
                *choice = idx.checked_sub(1);
                ui.combo(&format!("Match for {text}"), mt, &mts);
            });
        }
    });
    let profile = profile_json(&ed);
    let key = format!("{profile}");
    if key != ed.preview_key {
        ed.preview_key = key;
        ed.preview = app
            .read(
                "inv_profile.preview",
                json!({"profile": profile, "name": ed.file_name,
                       "content_base64": base64::engine::general_purpose::STANDARD.encode(&ed.bytes)}),
            )
            .unwrap_or(Value::Null);
    }
    if !ed.preview.at("error").is_null() {
        ui.error_text(ed.preview.str("error"));
    }
    let cols = vec![
        Column {
            title: "Date".into(),
            width: 110,
            ..Column::default()
        },
        Column {
            title: "Action".into(),
            width: 140,
            ..Column::default()
        },
        Column {
            title: "Security".into(),
            ..Column::default()
        },
        Column {
            title: "Shares".into(),
            width: 100,
            align: Align::Right,
            ..Column::default()
        },
        Column {
            title: "Price".into(),
            width: 100,
            align: Align::Right,
            ..Column::default()
        },
        Column {
            title: "Amount".into(),
            width: 110,
            align: Align::Right,
            ..Column::default()
        },
    ];
    ui.table("Preview", cols, |ui| {
        for (i, r) in ed.preview.list("rows").iter().enumerate() {
            ui.table_row(&format!("preview:{i}"), false, false, |ui| {
                ui.label(fmt::date(r.str("date")));
                let action = if r.str("action") == "unsupported" {
                    r.str("unsupported")
                } else {
                    r.str("action")
                };
                ui.label(action.to_owned());
                ui.label(r.str("security"));
                ui.amount(r.str("quantity"));
                ui.amount(r.str("price"));
                ui.amount(fmt::group(r.str("amount")));
            });
        }
    });
    for w in ed.preview.list("warnings") {
        ui.secondary(w.as_str().unwrap_or(""));
    }
    ui.card("Save", |ui| {
        ui.form("Save as", |ui| {
            ui.width(240);
            ui.field("Profile name", &mut ed.name);
        });
        if let Some(e) = &ed.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save profile and continue").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        app.navigate(View::Home);
        return;
    }
    if save {
        if ed.name.trim().is_empty() {
            ed.error = Some("Enter a name for the profile.".into());
        } else {
            match app.run("inv_profile.save", json!({"profile": profile_json(&ed)})) {
                Ok(_) => {
                    let name = ed.name.trim().to_owned();
                    let (file, bytes) = (ed.file_name.clone(), ed.bytes.clone());
                    super::import::stage(app, &file, &bytes, Some(&name));
                    return;
                }
                Err(e) => ed.error = Some(e.display()),
            }
        }
    }
    app.import.inv.editor = Some(ed);
}

pub(crate) fn securities_panel(app: &mut App, ui: &mut Ui, b: &Value, si: usize, s: &Value) {
    let secs = s.list("securities").to_vec();
    let shown: Vec<&Value> = secs
        .iter()
        .filter(|x| x.str("how") != "unsupported")
        .collect();
    if !shown.iter().any(|x| x.str("how") == "new") && app.import.inv.changing.is_empty() {
        return;
    }
    let existing = array(app.read("security.list", json!({})));
    let staged = b.str("status") == "staged";
    ui.heading("Securities in this file");
    for x in shown {
        let key = x.str("key").to_owned();
        let file_label = [x.str("ticker"), x.str("name"), x.str("cusip")]
            .iter()
            .filter(|t| !t.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" · ");
        ui.wrap(|ui| {
            ui.label(file_label);
            if x.str("how") == "new" {
                ui.badge("New security", Tone::Accent);
                let p = x.at("proposed");
                let entry = app
                    .import
                    .inv
                    .edits
                    .entry(key.clone())
                    .or_insert_with(|| (p.str("symbol").to_owned(), p.str("name").to_owned()));
                let mut sym = entry.0.clone();
                let mut name = entry.1.clone();
                ui.width(110);
                let r1 = ui.field(&format!("Symbol for {key}"), &mut sym);
                ui.width(220);
                let r2 = ui.field(&format!("Name for {key}"), &mut name);
                let mut kind = SEC_KINDS
                    .iter()
                    .position(|k| k.0 == p.str("kind"))
                    .unwrap_or(0);
                let kinds: Vec<String> = SEC_KINDS.iter().map(|k| k.1.to_owned()).collect();
                let kc = ui
                    .combo(&format!("Kind for {key}"), &mut kind, &kinds)
                    .changed();
                let mut method = METHODS
                    .iter()
                    .position(|m| m.0 == p.str("basis_method"))
                    .unwrap_or(0);
                let methods: Vec<String> = METHODS.iter().map(|m| m.1.to_owned()).collect();
                let mc = ui
                    .combo(&format!("Basis method for {key}"), &mut method, &methods)
                    .changed();
                if let Some(e) = app.import.inv.edits.get_mut(&key) {
                    *e = (sym.clone(), name.clone());
                }
                let commit_text = (r1.blurred || r2.blurred || r1.submitted || r2.submitted)
                    && (sym != p.str("symbol") || name != p.str("name"));
                if staged && (commit_text || kc || mc) {
                    let args = json!({
                        "batch": b.str("id"), "section": si, "key": key,
                        "symbol": sym, "name": name,
                        "kind": SEC_KINDS.get(kind).map_or("stock", |k| k.0),
                        "basis_method": METHODS.get(method).map_or("fifo", |m| m.0),
                    });
                    if let Err(e) = app.run("import.set_security", args) {
                        app.import.error = Some(e.display());
                    }
                }
                if staged {
                    use_existing(app, ui, b, si, &key, &existing);
                }
                if !x.at("problem").is_null() {
                    ui.error_text(x.str("problem"));
                } else if x.at("flagged").as_bool() == Some(true) {
                    ui.secondary("Symbol made from the name: check it");
                }
            } else {
                ui.label(format!("Matches {}", x.str("symbol")));
                if staged {
                    if app.import.inv.changing.contains(&key) {
                        use_existing(app, ui, b, si, &key, &existing);
                        if ui.button("New security").clicked() {
                            app.import.inv.changing.remove(&key);
                            let symbol = if x.str("ticker").is_empty() {
                                x.str("symbol")
                            } else {
                                x.str("ticker")
                            };
                            let args = json!({"batch": b.str("id"), "section": si, "key": key,
                                              "symbol": symbol, "name": x.str("name")});
                            if let Err(e) = app.run("import.set_security", args) {
                                app.import.error = Some(e.display());
                            }
                        }
                    } else if ui.button("Change…").clicked() {
                        app.import.inv.changing.insert(key.clone());
                    }
                }
            }
        });
    }
}

fn use_existing(app: &mut App, ui: &mut Ui, b: &Value, si: usize, key: &str, existing: &[Value]) {
    if existing.is_empty() {
        return;
    }
    let mut pick = 0usize;
    let mut items: Vec<String> = vec!["Use existing…".into()];
    items.extend(existing.iter().map(|e| e.str("symbol").to_owned()));
    ui.combo(&format!("Use existing for {key}"), &mut pick, &items);
    if let Some(e) = pick.checked_sub(1).and_then(|i| existing.get(i)) {
        app.import.inv.changing.remove(key);
        let args =
            json!({"batch": b.str("id"), "section": si, "key": key, "security": e.str("id")});
        if let Err(e) = app.run("import.set_security", args) {
            app.import.error = Some(e.display());
        }
    }
}

fn action_label(a: &str) -> &'static str {
    match a {
        "buy" => "Buy",
        "sell" => "Sell",
        "dividend" => "Dividend",
        "reinvest" => "Reinvest",
        "interest" => "Interest",
        "fee" => "Fee",
        "shares_in" => "Shares in",
        "shares_out" => "Shares out",
        "split" => "Split",
        "return_of_capital" => "Return of capital",
        _ => "Other",
    }
}

pub(crate) fn chip(r: &Value) -> &'static str {
    match r.str("class") {
        "duplicate" => "Already imported",
        "match" => "Matches existing",
        "unsupported" => "Not supported",
        "reverse" if r.str("action") == "void" => "Will void",
        "pending" => "Choose an account first",
        _ if r.at("needs_review").as_bool() == Some(true) => "Needs review",
        _ => "New",
    }
}

pub(crate) fn rows_table(app: &mut App, ui: &mut Ui, b: &Value, si: usize) {
    let rows: Vec<Value> = b
        .list("inv_rows")
        .iter()
        .filter(|r| r.at("section").as_u64() == u64::try_from(si).ok())
        .cloned()
        .collect();
    if rows.is_empty() {
        return;
    }
    let staged = b.str("status") == "staged";
    let cols = vec![
        Column {
            title: "Date".into(),
            width: 110,
            ..Column::default()
        },
        Column {
            title: "Action".into(),
            width: 110,
            ..Column::default()
        },
        Column {
            title: "Security".into(),
            ..Column::default()
        },
        Column {
            title: "Shares".into(),
            width: 90,
            align: Align::Right,
            ..Column::default()
        },
        Column {
            title: "Price".into(),
            width: 90,
            align: Align::Right,
            ..Column::default()
        },
        Column {
            title: "Amount".into(),
            width: 110,
            align: Align::Right,
            ..Column::default()
        },
        Column {
            title: "What will happen".into(),
            width: 170,
            ..Column::default()
        },
        Column {
            title: "Skip".into(),
            width: 48,
            align: Align::Center,
            ..Column::default()
        },
    ];
    ui.table("Investment rows", cols, |ui| {
        for (i, r) in rows.iter().enumerate() {
            let c = chip(r);
            let cleaned = r.at("row").list("cleaned").to_vec();
            let notes: Vec<String> = r
                .list("notes")
                .iter()
                .filter_map(|n| n.as_str().map(str::to_owned))
                .collect();
            let tip = format!(
                "{}\n{}",
                notes.join("\n"),
                mk_core::text::escape_raw(r.at("row").str("raw"))
            );
            let label = format!(
                "Investment row {}: {} {} {}: {c}",
                i + 1,
                action_label(r.str("inv_action")),
                r.str("symbol"),
                fmt::group(r.str("amount"))
            );
            let mut skip_change = None;
            ui.table_row(&label, false, false, |ui| {
                ui.label(fmt::date(r.str("date")));
                ui.label(action_label(r.str("inv_action")));
                ui.tip(r.str("symbol"));
                ui.label(r.str("symbol"));
                ui.amount(r.str("quantity"));
                ui.amount(r.str("price"));
                ui.amount(fmt::group(r.str("amount")));
                let mark = if cleaned.is_empty() { "" } else { " (cleaned)" };
                ui.tip(tip.trim().to_owned()).named(label.clone());
                ui.badge(
                    format!("{c}{mark}"),
                    if c == "New" {
                        Tone::Accent
                    } else {
                        Tone::Secondary
                    },
                );
                let mut skip = r.str("action") == "skip";
                let was = skip;
                ui.enabled(staged && matches!(r.str("class"), "new"))
                    .named(format!("Skip investment row {}", i + 1));
                ui.checkbox(&mut skip, "");
                if skip != was {
                    skip_change = Some(skip);
                }
            });
            if let Some(skip) = skip_change {
                set_inv_row(app, b, r.str("id"), json!({"skip": skip}));
            }
            let needs_basis = r.str("inv_action") == "shares_in"
                && r.at("row").at("basis").is_null()
                && r.at("overrides").at("basis").is_null()
                && r.str("class") == "new";
            let proposal = !r.at("proposal").is_null() && r.str("class") == "new";
            if staged && (needs_basis || proposal) {
                ui.table_row(&format!("{label} actions"), false, false, |ui| {
                    ui.label("");
                    ui.row(|ui| {
                        if needs_basis && ui.link("Enter basis…").clicked() {
                            app.import.inv.basis_form = Some((
                                r.str("id").to_owned(),
                                String::new(),
                                fmt::date(r.at("row").str("acquired")),
                                false,
                            ));
                        }
                        if proposal {
                            let p = r.at("proposal");
                            let accepted = r.at("overrides").at("move_in").as_bool() == Some(true);
                            let label = if accepted {
                                "Move shares in first: accepted (change…)".to_owned()
                            } else {
                                format!("Move shares in first… ({} shares)", p.str("quantity"))
                            };
                            if ui.link(label).clicked() {
                                app.import.inv.basis_form = Some((
                                    r.str("id").to_owned(),
                                    p.str("basis").to_owned(),
                                    String::new(),
                                    true,
                                ));
                            }
                        }
                    });
                });
            }
        }
    });
    basis_dialog(app, ui, b);
}

fn set_inv_row(app: &mut App, b: &Value, row: &str, mut args: Value) {
    set(&mut args, "batch", json!(b.str("id")));
    set(&mut args, "row", json!(row));
    if let Err(e) = app.run("import.set_inv_row", args) {
        app.import.error = Some(e.display());
    }
}

fn basis_dialog(app: &mut App, ui: &mut Ui, b: &Value) {
    let Some((row, mut basis, mut acquired, move_in)) = app.import.inv.basis_form.take() else {
        return;
    };
    let mut save = false;
    let mut cancel = ui.dialog_closed("inv-basis");
    let title = if move_in {
        "Move shares in first"
    } else {
        "Enter basis"
    };
    ui.dialog("inv-basis", title, 440, |ui| {
        if move_in {
            ui.secondary("The sale needs shares bought before this file. Enter their cost basis; the acquired date may be left blank.");
        }
        ui.form("Cost basis", |ui| {
            ui.width(140).mono();
            ui.field("Cost basis", &mut basis);
        });
        ui.form("Acquired", |ui| {
            super::register::date_field(ui, "Acquired date", &mut acquired);
        });
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save").clicked() {
                save = true;
            }
        });
    });
    if save {
        let mut args = if move_in {
            json!({"move_in": true})
        } else {
            json!({})
        };
        let (bk, ak) = if move_in {
            ("move_in_basis", "move_in_acquired")
        } else {
            ("basis", "acquired")
        };
        if !basis.trim().is_empty() {
            set(&mut args, bk, json!(basis.trim().replace(',', "")));
        }
        if !acquired.trim().is_empty() {
            set(&mut args, ak, json!(fmt::date_arg_lenient(&acquired)));
        }
        set_inv_row(app, b, &row, args);
        return;
    }
    if !cancel {
        app.import.inv.basis_form = Some((row, basis, acquired, move_in));
    }
}

pub(crate) fn holdings_check(ui: &mut Ui, s: &Value) {
    let pc = s.at("positions_check");
    if pc.is_null() {
        return;
    }
    ui.card("Holdings check", |ui| {
        ui.label(pc.str("message"));
        let diffs = pc.list("differences").to_vec();
        if !diffs.is_empty() {
            let cols = vec![
                Column {
                    title: "Security".into(),
                    ..Column::default()
                },
                Column {
                    title: "In your book".into(),
                    width: 140,
                    align: Align::Right,
                    ..Column::default()
                },
                Column {
                    title: "On the statement".into(),
                    width: 140,
                    align: Align::Right,
                    ..Column::default()
                },
            ];
            ui.table("Differences", cols, |ui| {
                for d in &diffs {
                    ui.table_row(d.str("security"), false, false, |ui| {
                        ui.label(d.str("security"));
                        ui.amount(d.str("book"));
                        ui.amount(d.str("statement"));
                    });
                }
            });
        }
        let cash = pc.at("cash");
        if !cash.is_null() {
            ui.label(format!(
                "Cash: {} in your book, {} on the statement",
                fmt::group(cash.str("book")),
                fmt::group(cash.str("statement"))
            ));
        }
    });
}

pub(crate) fn securities_blocked(b: &Value) -> bool {
    b.list("sections").iter().any(|s| {
        s.list("securities")
            .iter()
            .any(|x| x.str("how") == "new" && !x.at("problem").is_null())
    })
}

/// Which installed definition package brought each object: (kind, name) → package.
pub(crate) fn provenance(app: &mut App) -> HashMap<(String, String), String> {
    let mut out = HashMap::new();
    for p in array(app.read("definition.list", json!({}))) {
        for i in p.list("items") {
            out.insert(
                (i.str("kind").to_owned(), i.str("object").to_lowercase()),
                p.str("name").to_owned(),
            );
        }
    }
    out
}

/// Settings → Import → Investment profiles.
pub(crate) fn profiles_settings(app: &mut App, ui: &mut Ui) {
    let list = array(app.read("inv_profile.list", json!({})));
    let from = provenance(app);
    ui.card("Investment profiles", |ui| {
        for p in list {
            ui.row(|ui| {
                ui.next().grow();
                ui.label(format!(
                    "{} ({})",
                    p.str("name"),
                    p.str("format").to_uppercase()
                ));
                if let Some(pkg) = from.get(&(
                    "investment_profile".to_owned(),
                    p.str("name").to_lowercase(),
                )) {
                    ui.secondary(format!("from {pkg}"));
                }
                if p.at("builtin").as_bool() == Some(true) {
                    ui.secondary("built in");
                } else {
                    ui.tip("Delete this profile")
                        .id(format!("delete-profile:{}", p.str("name")));
                    if ui.button("Delete").clicked()
                        && let Err(e) =
                            app.run("inv_profile.delete", json!({"name": p.str("name")}))
                    {
                        app.notify_error(&e);
                    }
                }
            });
        }
        ui.secondary("Profiles are created when you import a broker file that no profile reads.");
    });
}
