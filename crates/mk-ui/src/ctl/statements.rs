//! Customer statements and late fees (app-desktop revision 16): the customer view's
//! Statement…, Sales → Statements… (batch) and Sales → Late fees….

use base64::Engine;
use serde_json::{Value, json};

use super::form::{choice, date_row, put};
use super::ui::{Column, Ui, col, col_num};
use super::util::Json;
use super::{App, AppEvent};
use crate::fmt;

const KINDS: [(&str, &str); 2] = [("activity", "Activity"), ("open_items", "Open items")];

#[derive(Debug, Clone, Default)]
pub(crate) struct StatementForm {
    pub customer: String,
    pub from: String,
    pub to: String,
    pub kind: usize,
    pub data: Option<(String, Value)>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BatchForm {
    pub from: String,
    pub to: String,
    pub kind: usize,
    pub combined: bool,
    pub customers: Vec<(String, String, bool)>,
    pub loaded_for: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct LateFeeForm {
    pub as_of: String,
    pub rows: Vec<(Value, bool)>,
    pub loaded_for: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct StatementsState {
    pub statement: Option<StatementForm>,
    pub batch: Option<BatchForm>,
    pub late: Option<LateFeeForm>,
}

fn month_before(to: &str) -> String {
    to.parse::<mk_core::Date>()
        .ok()
        .and_then(|d| d.add_days(-30))
        .map_or_else(|| to.to_owned(), |d| d.to_string())
}

pub(crate) fn open_statement(app: &mut App, customer: &str) {
    let to = app.today();
    app.statements.statement = Some(StatementForm {
        customer: customer.to_owned(),
        from: fmt::date(&month_before(&to)),
        to: fmt::date(&to),
        ..StatementForm::default()
    });
}

pub(crate) fn open_batch(app: &mut App) {
    let to = app.today();
    app.statements.batch = Some(BatchForm {
        from: fmt::date(&month_before(&to)),
        to: fmt::date(&to),
        ..BatchForm::default()
    });
}

pub(crate) fn open_late_fees(app: &mut App) {
    app.statements.late = Some(LateFeeForm {
        as_of: fmt::date(&app.today()),
        ..LateFeeForm::default()
    });
}

pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    statement_dialog(app, ui);
    batch_dialog(app, ui);
    late_fee_dialog(app, ui);
}

fn save_pdf(app: &mut App, pdf: &Value) {
    match base64::engine::general_purpose::STANDARD.decode(pdf.str("content_base64")) {
        Ok(data) => app.emit(AppEvent::SaveBytes {
            file_name: pdf.str("file_name").to_owned(),
            data,
        }),
        Err(e) => app.notify(format!("The PDF could not be read: {e}")),
    }
}

fn kind_row(ui: &mut Ui, kind: &mut usize) {
    ui.form("Kind", |ui| {
        let items: Vec<&str> = KINDS.iter().map(|k| k.1).collect();
        choice(ui, "Statement kind", kind, &items);
    });
}

fn statement_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.statements.statement.take() else {
        return;
    };
    let kind = KINDS.get(f.kind).map_or("activity", |k| k.0);
    let args = json!({"customer": f.customer, "from": fmt::date_arg_lenient(&f.from), "to": fmt::date_arg_lenient(&f.to), "kind": kind});
    let key = args.to_string();
    if f.data.as_ref().is_none_or(|(k, _)| *k != key) {
        match app.backend.invoke("statement.render", args) {
            Ok(v) => {
                f.data = Some((key, v));
                f.error = None;
            }
            Err(e) => {
                f.data = Some((key, Value::Null));
                f.error = Some(e.display());
            }
        }
    }
    let (mut close, mut pdf) = (ui.dialog_closed("statement-dialog"), false);
    ui.dialog(
        "statement-dialog",
        &format!("Statement for {}", f.customer),
        660,
        |ui| {
            ui.group(|ui| {
                date_row(ui, "From date", &mut f.from);
                date_row(ui, "To date", &mut f.to);
                kind_row(ui, &mut f.kind);
            });
            if let Some(e) = &f.error {
                ui.error_text(e.clone());
            }
            let ready = f.data.as_ref().is_some_and(|(_, d)| !d.is_null());
            if let Some((_, d)) = f.data.as_ref().filter(|_| ready) {
                preview(ui, d);
            }
            ui.actions(|ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
                ui.enabled(ready);
                if ui.primary("PDF…").clicked() {
                    pdf = true;
                }
            });
        },
    );
    if pdf && let Some((_, d)) = f.data.as_ref().filter(|(_, d)| !d.is_null()) {
        let d = d.clone();
        save_pdf(app, &d);
    }
    if !close {
        app.statements.statement = Some(f);
    }
}

fn preview(ui: &mut Ui, d: &Value) {
    let cols = vec![
        Column {
            width: 110,
            ..col("Date")
        },
        Column {
            weight: 3,
            ..col("Item")
        },
        Column {
            width: 120,
            ..col_num("Amount")
        },
        Column {
            width: 120,
            ..col_num("Balance")
        },
    ];
    ui.table("", cols, |ui| {
        ui.visible_rows(8);
        if d.str("kind") == "activity" {
            ui.table_row("opening", false, false, |ui| {
                ui.label("");
                ui.label("Opening balance");
                ui.label("");
                ui.amount(fmt::group(d.str("opening")));
            });
        }
        for (i, r) in d.list("rows").iter().enumerate() {
            ui.table_row(&format!("row{i}"), false, false, |ui| {
                ui.label(fmt::date(r.str("date")));
                ui.label(
                    format!("{} {}", r.str("kind"), r.str("reference"))
                        .trim()
                        .to_owned(),
                );
                ui.amount(fmt::group(r.str("amount")));
                ui.amount(fmt::group(r.str("balance")));
            });
        }
    });
    ui.bold();
    ui.label(format!("Amount due {}", fmt::group(d.str("closing"))));
    let cols: Vec<String> = d
        .at("aging")
        .list("columns")
        .iter()
        .map(|c| c.as_str().unwrap_or_default().to_owned())
        .collect();
    let buckets: Vec<String> = d
        .at("aging")
        .list("buckets")
        .iter()
        .map(|c| fmt::group(c.as_str().unwrap_or_default()))
        .collect();
    let aging = cols
        .iter()
        .zip(&buckets)
        .map(|(c, b)| format!("{c} {b}"))
        .collect::<Vec<_>>()
        .join(" · ");
    ui.secondary(format!("Aging: {aging}"));
}

fn batch_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.statements.batch.take() else {
        return;
    };
    if f.loaded_for != f.to {
        f.loaded_for = f.to.clone();
        let rows = app
            .read(
                "report.customer_balances",
                json!({"as_of": fmt::date_arg_lenient(&f.to)}),
            )
            .map(|v| v.list("customers").to_vec())
            .unwrap_or_default();
        f.customers = rows
            .iter()
            .filter(|c| !fmt::is_zero(c.str("balance")))
            .map(|c| {
                (
                    c.str("customer_name").to_owned(),
                    c.str("balance").to_owned(),
                    true,
                )
            })
            .collect();
    }
    let (mut close, mut make) = (ui.dialog_closed("statements-batch"), false);
    ui.dialog("statements-batch", "Statements", 580, |ui| {
        ui.group(|ui| {
            date_row(ui, "From date", &mut f.from);
            date_row(ui, "To date", &mut f.to);
            kind_row(ui, &mut f.kind);
        });
        ui.label("Customers with a balance");
        if f.customers.is_empty() {
            ui.secondary("No customer has a balance on that date.");
        }
        for (name, balance, chosen) in &mut f.customers {
            ui.row(|ui| {
                ui.checkbox(chosen, &format!("{name} {}", fmt::group(balance)));
            });
        }
        ui.row(|ui| {
            ui.checkbox(&mut f.combined, "One PDF for all customers");
        });
        if !f.combined {
            ui.secondary("One PDF per customer, saved to a folder you choose.");
        }
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                close = true;
            }
            ui.enabled(f.customers.iter().any(|c| c.2));
            if ui.primary("Create PDFs…").clicked() {
                make = true;
            }
        });
    });
    if make {
        let chosen: Vec<&str> = f
            .customers
            .iter()
            .filter(|c| c.2)
            .map(|c| c.0.as_str())
            .collect();
        let kind = KINDS.get(f.kind).map_or("activity", |k| k.0);
        let mut a = json!({"to": fmt::date_arg_lenient(&f.to), "customers": chosen, "kind": kind, "combined": f.combined});
        put(&mut a, "from", &fmt::date_arg_lenient(&f.from));
        match app.backend.invoke("statement.batch", a) {
            Ok(v) => {
                if f.combined {
                    save_pdf(app, v.at("pdf"));
                } else {
                    let mut files = Vec::new();
                    for p in v.list("files") {
                        match base64::engine::general_purpose::STANDARD
                            .decode(p.str("content_base64"))
                        {
                            Ok(data) => files.push((p.str("file_name").to_owned(), data)),
                            Err(e) => f.error = Some(format!("A PDF could not be read: {e}")),
                        }
                    }
                    app.emit(AppEvent::SaveFilesToFolder { files });
                }
                close = f.error.is_none();
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    if !close {
        app.statements.batch = Some(f);
    }
}

fn late_fee_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.statements.late.take() else {
        return;
    };
    let key = format!("{}|{}", app.generation, fmt::date_arg_lenient(&f.as_of));
    if f.loaded_for.as_deref() != Some(key.as_str()) {
        f.loaded_for = Some(key);
        match app.backend.invoke(
            "late_fee.preview",
            json!({"as_of": fmt::date_arg_lenient(&f.as_of)}),
        ) {
            Ok(v) => {
                f.rows = v.list("fees").iter().map(|r| (r.clone(), true)).collect();
                f.error = None;
            }
            Err(e) => {
                f.rows.clear();
                f.error = Some(e.display());
            }
        }
    }
    let (mut close, mut assess) = (ui.dialog_closed("late-fees"), false);
    ui.dialog("late-fees", "Late fees", 660, |ui| {
        ui.group(|ui| {
            date_row(ui, "As of date", &mut f.as_of);
        });
        if f.rows.is_empty() && f.error.is_none() {
            ui.secondary("No late fees are due on that date.");
        } else {
            let cols = vec![
                Column {
                    width: 40,
                    ..col("")
                },
                Column {
                    weight: 2,
                    ..col("Customer")
                },
                Column {
                    width: 100,
                    ..col("Invoice")
                },
                Column {
                    width: 90,
                    ..col_num("Days late")
                },
                Column {
                    width: 110,
                    ..col_num("Balance")
                },
                Column {
                    width: 100,
                    ..col_num("Fee")
                },
            ];
            ui.table("", cols, |ui| {
                for (r, chosen) in &mut f.rows {
                    ui.table_row(&format!("fee:{}", r.str("invoice")), false, false, |ui| {
                        ui.named(format!("Charge a late fee on {}", r.str("number")));
                        ui.checkbox(chosen, "");
                        ui.label(r.str("customer_name"));
                        ui.label(r.str("number"));
                        ui.amount(r.at("days_late").to_string());
                        ui.amount(fmt::group(r.str("balance")));
                        ui.amount(fmt::group(r.str("fee")));
                    });
                }
            });
            let total = f
                .rows
                .iter()
                .filter(|r| r.1)
                .filter_map(|r| r.0.str("fee").parse::<mk_core::Decimal>().ok())
                .fold(mk_core::Decimal::ZERO, |s, x| s.checked_add(x).unwrap_or(s));
            ui.label(format!("Total fees {}", fmt::group(&format!("{total:.2}"))));
        }
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Close").clicked() {
                close = true;
            }
            ui.enabled(f.rows.iter().any(|r| r.1));
            if ui.primary("Assess selected").clicked() {
                assess = true;
            }
        });
    });
    if assess {
        let chosen: Vec<&str> = f
            .rows
            .iter()
            .filter(|r| r.1)
            .map(|r| r.0.str("invoice"))
            .collect();
        match app.run(
            "late_fee.assess",
            json!({"as_of": fmt::date_arg_lenient(&f.as_of), "invoices": chosen}),
        ) {
            Ok(v) => {
                let n = v.list("invoices").len();
                let s = if n == 1 { "" } else { "s" };
                app.notify(format!("Created {n} late-fee invoice{s}."));
                close = true;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    if !close {
        app.statements.late = Some(f);
    }
}
