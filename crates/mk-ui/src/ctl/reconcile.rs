//! Reconcile (reconciliation.md): start with a statement, tick money out and
//! money in, see the difference, finish (with an adjustment), cancel, history.

use serde_json::{Value, json};

use super::App;
use super::ui::{Tone, Ui};
use super::util::{Json, array};
use crate::View;
use crate::fmt;

#[derive(Debug, Default)]
pub(crate) struct ReconcileState {
    pub date: String,
    pub balance: String,
    pub adjust: Option<String>,
    pub error: Option<String>,
}

impl App {
    /// Start reconciling `account` from a captured statement.
    pub(crate) fn reconcile_start_statement(&mut self, account: &str, statement: &str) {
        match self.run(
            "reconcile.start",
            json!({"account": account, "statement": statement}),
        ) {
            Ok(_) => self.navigate(View::Reconcile {
                account: account.to_owned(),
            }),
            Err(e) => self.notify_error(&e),
        }
    }
}

pub(crate) fn view(app: &mut App, ui: &mut Ui, account: &str) {
    let name = app
        .read("account.get", json!({"account": account}))
        .map(|i| i.str("full_name").to_owned())
        .unwrap_or_default();
    ui.title(format!("Reconcile {name}"));
    match app
        .backend
        .invoke("reconcile.get", json!({"account": account}))
    {
        Ok(session) => open_session(app, ui, account, &session),
        Err(_) => start(app, ui, account),
    }
    history(app, ui, account);
}

fn start(app: &mut App, ui: &mut Ui, account: &str) {
    let batches = array(app.read("import.list", json!({"status": "committed"})));
    let mut offers = Vec::new();
    for b in batches {
        if let Some(full) = app.read("import.get", json!({"batch": b.str("id")})) {
            for s in full.list("sections") {
                if s.str("account") == account && !s.at("statement").is_null() {
                    let c = s.at("closing");
                    let file = if b.str("source").is_empty() {
                        "an imported file"
                    } else {
                        b.str("source")
                    };
                    offers.push((
                        format!(
                            "Use statement from {file} ({}, {})",
                            fmt::date(c.str("date")),
                            fmt::group(c.str("amount"))
                        ),
                        s.str("statement").to_owned(),
                    ));
                }
            }
        }
    }
    if app.reconcile.date.is_empty() {
        app.reconcile.date = fmt::date(&app.today());
    }
    ui.card("Statement", |ui| {
        for (label, st) in &offers {
            ui.row(|ui| {
                if ui.button(label.clone()).clicked() {
                    app.reconcile_start_statement(account, st);
                }
            });
        }
        if !offers.is_empty() {
            ui.label("Or enter the statement's closing date and balance:");
        }
        ui.form("Statement date", |ui| {
            super::register::date_field(ui, "Statement date", &mut app.reconcile.date);
        });
        ui.form("Statement balance", |ui| {
            ui.hint("e.g. 1,234.56").width(160).mono();
            ui.field("Statement balance", &mut app.reconcile.balance);
        });
        if let Some(e) = app.reconcile.error.clone() {
            ui.error_text(e);
        }
        ui.actions(|ui| {
            if ui.primary("Start reconciling").clicked() {
                let args = json!({"account": account, "statement_date": fmt::date_arg_lenient(&app.reconcile.date), "statement_balance": app.reconcile.balance.trim().replace(',', "")});
                match app.run("reconcile.start", args) {
                    Ok(_) => app.reconcile.error = None,
                    Err(e) => app.reconcile.error = Some(e.display()),
                }
            }
        });
    });
}

fn candidate_list(app: &mut App, ui: &mut Ui, account: &str, title: &str, items: &[Value]) {
    ui.card(title, |ui| {
        if items.is_empty() {
            ui.secondary("Nothing here.");
        }
        for c in items {
            let mut ticked = c.at("selected").as_bool() == Some(true);
            let label = format!(
                "{} {} {}",
                fmt::date(c.str("date")),
                c.str("payee"),
                fmt::group(c.str("amount"))
            );
            ui.next().id(format!("cand:{}", c.str("split")));
            if ui.checkbox(&mut ticked, &label).clicked() {
                let cmd = if ticked {
                    "reconcile.select"
                } else {
                    "reconcile.unselect"
                };
                if let Err(e) =
                    app.run(cmd, json!({"account": account, "splits": [c.str("split")]}))
                {
                    app.reconcile.error = Some(e.display());
                }
            }
        }
    });
}

fn open_session(app: &mut App, ui: &mut Ui, account: &str, s: &Value) {
    let cands = s.list("candidates");
    let (out, inn): (Vec<Value>, Vec<Value>) = cands
        .iter()
        .cloned()
        .partition(|c| fmt::is_negative(c.str("amount")));
    candidate_list(app, ui, account, "Money out", &out);
    candidate_list(app, ui, account, "Money in", &inn);
    let diff = s.str("difference").to_owned();
    let balanced = fmt::is_zero(&diff);
    let ticked: mk_core::Decimal = cands
        .iter()
        .filter(|c| c.at("selected").as_bool() == Some(true))
        .filter_map(|c| c.str("amount").parse::<mk_core::Decimal>().ok())
        .sum();
    ui.card("", |ui| {
        ui.wrap(|ui| {
            ui.label(format!(
                "Statement balance {}",
                fmt::group(s.str("statement_balance"))
            ));
            ui.separator();
            ui.label(format!(
                "Starting balance {}",
                fmt::group(s.str("opening_balance"))
            ));
            ui.separator();
            ui.label(format!("Ticked total {}", fmt::group(&ticked.to_string())));
        });
        let diff_text = if balanced {
            "Balanced".to_owned()
        } else {
            format!("Difference {}", fmt::group(&diff))
        };
        ui.tone(if balanced {
            Tone::Success
        } else {
            Tone::Normal
        });
        ui.big_heading(diff_text);
        if let Some(e) = app.reconcile.error.clone() {
            ui.error_text(e);
        }
        ui.row(|ui| {
            ui.enabled(balanced);
            if ui.primary("Finish").clicked() {
                match app.run("reconcile.finish", json!({"account": account})) {
                    Ok(_) => {
                        app.reconcile.error = None;
                        app.notify("Reconciliation finished.");
                    }
                    Err(e) => app.reconcile.error = Some(e.display()),
                }
            }
            if !balanced && ui.button("Finish with an adjustment…").clicked() {
                app.reconcile.adjust = Some(String::new());
            }
            if ui.button("Cancel reconciliation").clicked()
                && let Err(e) = app.run("reconcile.cancel", json!({"account": account}))
            {
                app.reconcile.error = Some(e.display());
            }
        });
    });
    if let Some(mut acc) = app.reconcile.adjust.clone() {
        let mut close = ui.dialog_closed("adjust");
        ui.dialog("adjust", "Finish with an adjustment", 460, |ui| {
            ui.label(format!(
                "Record an adjustment of {} to balance this reconciliation.",
                fmt::group(&diff)
            ));
            ui.form("Adjustment account", |ui| {
                ui.next().grow();
                ui.field("Adjustment account", &mut acc);
            });
            ui.actions(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.primary("Confirm adjustment").clicked() {
                    match app.run(
                        "reconcile.finish",
                        json!({"account": account, "adjust": true, "adjustment_account": acc.trim()}),
                    ) {
                        Ok(_) => close = true,
                        Err(e) => app.reconcile.error = Some(e.display()),
                    }
                }
            });
        });
        app.reconcile.adjust = if close { None } else { Some(acc) };
    }
}

fn history(app: &mut App, ui: &mut Ui, account: &str) {
    let past = array(app.read("reconcile.history", json!({"account": account})));
    ui.card("Past reconciliations", |ui| {
        if past.is_empty() {
            ui.secondary("None yet.");
            return;
        }
        for p in &past {
            ui.label(format!(
                "{}  statement balance {}",
                fmt::date(p.str("statement_date")),
                fmt::group(p.str("statement_balance"))
            ));
        }
        ui.row(|ui| {
            if ui.button("Undo last reconciliation").clicked()
                && let Err(e) = app.run("reconcile.undo_last", json!({"account": account}))
            {
                app.reconcile.error = Some(e.display());
            }
        });
    });
}
