//! History: the audit log in plain words, with Undo and Redo.

use serde_json::json;

use super::App;
use super::ui::{Column, Ui};
use super::util::{Json, array};

pub(crate) fn view(app: &mut App, ui: &mut Ui) {
    ui.title("History");
    ui.toolbar(|ui| {
        if ui.button("Undo").clicked() {
            app.undo();
        }
        if ui.button("Redo").clicked() {
            app.redo();
        }
        super::ai::history_filter(app, ui);
    });
    let mut entries = array(app.read("history.list", json!({"limit": 200})));
    if app.ai.history_only {
        entries.retain(|e| e.str("actor").starts_with("mcp:"));
    }
    let cols = vec![
        Column {
            title: "When".into(),
            width: 170,
            ..Column::default()
        },
        Column {
            title: "Who".into(),
            width: 110,
            ..Column::default()
        },
        Column {
            title: "What".into(),
            ..Column::default()
        },
    ];
    ui.table("History", cols, |ui| {
        for (i, e) in entries.iter().enumerate() {
            let when = crate::fmt::date(&e.str("at").get(..16).unwrap_or("").replace('T', " "));
            let mut what = crate::base::describe_command(e.str("command"));
            let args = e.at("args");
            if !args.str("sha256").is_empty() && !args.str("name").is_empty() {
                let sha = args.str("sha256");
                what.push_str(&format!(
                    " — {} (SHA-256 {})",
                    args.str("name"),
                    sha.get(..12).unwrap_or(sha)
                ));
            }
            if e.at("undone").as_bool() == Some(true) {
                what.push_str(" (undone)");
            }
            ui.table_row(&format!("history:{i}"), false, false, |ui| {
                ui.label(when);
                ui.label(e.str("actor"));
                ui.label(what);
            });
        }
    });
}
