//! Budget: month or year view of the default (or chosen) budget against actuals.
//! Shown only while the `budgets` module is on.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::App;
use super::ui::{Align, Column, Tone, Ui, col, col_num};
use super::util::{Json, array};
use crate::fmt;

#[derive(Debug, Default)]
pub(crate) struct BudgetState {
    /// Month shown, YYYY-MM (empty: the current month).
    pub month: String,
    pub year_view: bool,
    /// Budget chosen in the picker (empty: the default).
    pub budget: String,
    /// Budget cell text while being edited, by account id.
    pub edits: HashMap<String, String>,
    pub suggest: Option<SuggestForm>,
    pub error: Option<String>,
    /// (generation, budget, year) → monthly reports, for the year view.
    year_cache: Option<(u64, String, i32, Vec<Value>)>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SuggestForm {
    pub months: usize,
    pub last: bool,
    /// (account id, full name, include, amount)
    pub rows: Vec<(String, String, bool, String)>,
    pub key: String,
    pub error: Option<String>,
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn parse_month(m: &str) -> Option<(i32, u8)> {
    let (y, mo) = m.split_once('-')?;
    Some((y.parse().ok()?, mo.parse().ok()?))
}

fn month_text(y: i32, m: u8) -> String {
    format!("{y:04}-{m:02}")
}

fn shift(m: &str, n: i64) -> String {
    parse_month(m)
        .and_then(|(y, mo)| mk_core::Date::shift_month(y, mo, n))
        .map(|(y, mo)| month_text(y, mo))
        .unwrap_or_else(|| m.to_owned())
}

fn title(m: &str) -> String {
    match parse_month(m) {
        Some((y, mo)) => format!(
            "{} {y}",
            MONTHS
                .get(usize::from(mo).wrapping_sub(1))
                .copied()
                .unwrap_or("")
        ),
        None => m.to_owned(),
    }
}

fn current_month(app: &App) -> String {
    app.today().get(..7).unwrap_or("").to_owned()
}

fn budget_arg(app: &App) -> Value {
    if app.budget.budget.is_empty() {
        Value::Null
    } else {
        json!(app.budget.budget)
    }
}

pub(crate) fn view(app: &mut App, ui: &mut Ui) {
    if app.budget.month.is_empty() {
        app.budget.month = current_month(app);
    }
    let budgets = array(app.read("budget.list", json!({})));
    ui.title("Budget");
    if budgets.is_empty() {
        no_budget(app, ui);
        suggest_dialog(app, ui);
        return;
    }
    controls(app, ui, &budgets);
    if let Some(e) = app.budget.error.clone() {
        ui.error_text(e);
    }
    if app.budget.year_view {
        year(app, ui);
    } else {
        month(app, ui);
    }
    suggest_dialog(app, ui);
}

fn no_budget(app: &mut App, ui: &mut Ui) {
    ui.card("", |ui| {
        ui.label(
            "A budget sets how much you plan to spend and expect to earn in each category every month, then shows \
             what actually happened.",
        );
        ui.row(|ui| {
            if ui.primary("Start from my spending").clicked() && create_default(app) {
                app.budget.suggest = Some(SuggestForm { months: 1, ..SuggestForm::default() });
            }
            if ui.button("Start empty").clicked() {
                create_default(app);
            }
        });
        if let Some(e) = app.budget.error.clone() {
            ui.error_text(e);
        }
    });
}

fn create_default(app: &mut App) -> bool {
    let month = current_month(app);
    match app.run(
        "budget.create",
        json!({"name": "Budget", "start_month": month}),
    ) {
        Ok(_) => {
            app.budget.error = None;
            true
        }
        Err(e) => {
            app.budget.error = Some(e.display());
            false
        }
    }
}

fn controls(app: &mut App, ui: &mut Ui, budgets: &[Value]) {
    let step = if app.budget.year_view { 12 } else { 1 };
    ui.wrap(|ui| {
        ui.tip("Previous");
        if ui.icon_button("back", "Previous month", false).clicked() {
            app.budget.month = shift(&app.budget.month, -step);
        }
        let label = if app.budget.year_view {
            app.budget.month.get(..4).unwrap_or("").to_owned()
        } else {
            title(&app.budget.month)
        };
        ui.bold().width(140).align(Align::Center);
        ui.label(label);
        ui.tip("Next");
        if ui.icon_button("forward", "Next month", false).clicked() {
            app.budget.month = shift(&app.budget.month, step);
        }
        if ui.button("This month").clicked() {
            app.budget.month = current_month(app);
        }
        ui.separator();
        for (label, year) in [("Month", false), ("Year", true)] {
            if ui.tab(label, app.budget.year_view == year).clicked() {
                app.budget.year_view = year;
            }
        }
        if budgets.len() > 1 {
            ui.separator();
            let names: Vec<String> = budgets.iter().map(|b| b.str("name").to_owned()).collect();
            let mut shown = if app.budget.budget.is_empty() {
                budgets
                    .iter()
                    .find(|b| b.at("default").as_bool() == Some(true))
                    .map(|b| b.str("name").to_owned())
                    .unwrap_or_default()
            } else {
                app.budget.budget.clone()
            };
            ui.width(180);
            if ui.combo_str("Budget", &mut shown, &names).changed {
                app.budget.budget = shown;
            }
        }
        ui.spacer();
        if ui.button("Fill from history…").clicked() {
            app.budget.suggest = Some(SuggestForm {
                months: 1,
                ..SuggestForm::default()
            });
        }
        add_category(app, ui);
    });
}

fn add_category(app: &mut App, ui: &mut Ui) {
    let accounts = array(app.read("account.list", json!({})));
    let lined: Vec<String> = app
        .read("budget.get", json!({"budget": budget_arg(app)}))
        .map(|b| {
            b.list("lines")
                .iter()
                .map(|l| l.str("account").to_owned())
                .collect()
        })
        .unwrap_or_default();
    let options: Vec<&Value> = accounts
        .iter()
        .filter(|a| {
            matches!(a.str("type"), "income" | "expense") && !lined.iter().any(|l| l == a.str("id"))
        })
        .collect();
    let names: Vec<String> = options
        .iter()
        .map(|a| a.str("full_name").to_owned())
        .collect();
    let mut idx = usize::MAX;
    ui.width(200).hint("Add category…");
    if ui.combo("Add category…", &mut idx, &names).changed
        && let Some(a) = options.get(idx)
    {
        let args = json!({"budget": budget_arg(app), "account": a.str("id"), "amount": "0"});
        if let Err(e) = app.run("budget.set_line", args) {
            app.budget.error = Some(e.display());
        }
    }
}

// ----- month view ------------------------------------------------------------------

fn cols(actual: &str) -> Vec<Column> {
    vec![
        Column {
            weight: 3,
            ..col("Category")
        },
        Column {
            width: 120,
            ..col_num("Budget")
        },
        Column {
            width: 120,
            ..col_num(actual)
        },
        Column {
            width: 120,
            ..col_num("Remaining")
        },
        Column {
            weight: 2,
            ..col("Progress")
        },
    ]
}

fn month(app: &mut App, ui: &mut Ui) {
    let args = json!({"budget": budget_arg(app), "month": app.budget.month});
    let Some(r) = app.read("budget.report", args) else {
        return;
    };
    for (key, heading, actual) in [
        ("income", "Income", "Received"),
        ("expenses", "Expenses", "Spent"),
    ] {
        let section = r.at(key).clone();
        ui.table(heading, cols(actual), |ui| {
            for row in section.list("rows") {
                line_row(app, ui, row, key == "income");
            }
            let t = section.at("totals");
            ui.table_row(
                &format!("Total {}", heading.to_lowercase()),
                false,
                false,
                |ui| {
                    ui.bold();
                    ui.label(format!("Total {}", heading.to_lowercase()));
                    for k in ["budget", "actual", "remaining"] {
                        ui.bold();
                        ui.amount(fmt::group(t.str(k)));
                    }
                    ui.label("");
                },
            );
        });
    }
    let net = r.at("net");
    ui.table("", cols("Spent"), |ui| {
        ui.table_row("Net", false, false, |ui| {
            ui.bold().tip("Income minus expenses");
            ui.label("Net");
            ui.bold();
            ui.amount(fmt::group(net.str("budget")));
            ui.bold();
            ui.amount(fmt::group(net.str("actual")));
            ui.label("");
            ui.label("");
        });
    });
    not_in_budget(app, ui, &r);
}

fn line_row(app: &mut App, ui: &mut Ui, row: &Value, income: bool) {
    let acc = row.at("account");
    let id = acc.str("id").to_owned();
    let full = acc.str("full_name").to_owned();
    let name = full.rsplit(':').next().unwrap_or(&full).to_owned();
    let depth = i32::try_from(acc.at("depth").as_u64().unwrap_or(0)).unwrap_or(0);
    let rollover = row.at("rollover").as_bool() == Some(true);
    let remaining = row.str("remaining").to_owned();
    let pct = row.str("percent");
    let note = format!(
        "{full}: budget {}, {} {}, remaining {}{}{}",
        fmt::group(row.str("budget")),
        if income { "received" } else { "spent" },
        fmt::group(row.str("actual")),
        fmt::group(&remaining),
        if pct.is_empty() {
            String::new()
        } else {
            format!(", {pct}%")
        },
        if rollover { ", incl. carried" } else { "" },
    );
    let menu = [
        "Use this amount every month",
        if rollover {
            "Stop rolling over"
        } else {
            "Roll over unspent"
        },
        "Remove from budget",
    ];
    let resp = ui.table_row(&note, false, true, |ui| {
        ui.row(|ui| {
            ui.tip(full.clone());
            ui.label(name.clone());
            if rollover {
                ui.tip(format!(
                    "Carried from earlier months: {}",
                    fmt::group(row.str("carried"))
                ));
                ui.secondary("incl. carried");
            }
        });
        budget_cell(app, ui, row, &id, &full);
        ui.amount(fmt::group(row.str("actual")));
        if remaining.starts_with('-') && !income {
            ui.tone(Tone::Decrease);
        }
        if rollover {
            ui.tip(format!("incl. carried {}", fmt::group(row.str("carried"))));
        }
        ui.amount(fmt::group(&remaining));
        progress(ui, row, income);
    });
    ui.indent_last(depth);
    ui.row_menu(&menu);
    if let Some(Some(choice)) = resp.secondary {
        let args = match choice {
            0 => Some((
                "budget.set_line",
                json!({"budget": budget_arg(app), "account": id, "amount": row.str("month_amount"), "overrides": {}}),
            )),
            1 => Some((
                "budget.set_line",
                json!({"budget": budget_arg(app), "account": id, "amount": row.str("default_amount"), "rollover": !rollover}),
            )),
            2 => Some((
                "budget.remove_line",
                json!({"budget": budget_arg(app), "account": id}),
            )),
            _ => None,
        };
        if let Some((cmd, a)) = args
            && let Err(e) = app.run(cmd, a)
        {
            app.budget.error = Some(e.display());
        }
    }
}

fn budget_cell(app: &mut App, ui: &mut Ui, row: &Value, id: &str, full: &str) {
    let month_amount = row.str("month_amount").to_owned();
    let default_amount = row.str("default_amount").to_owned();
    let mut text = app
        .budget
        .edits
        .get(id)
        .cloned()
        .unwrap_or_else(|| month_amount.clone());
    ui.mono().align(Align::Right).id(format!("budget:{id}"));
    let resp = ui.field(&format!("Budget for {full}"), &mut text);
    if resp.changed {
        app.budget.edits.insert(id.to_owned(), text.clone());
    }
    if resp.submitted || resp.blurred {
        app.budget.edits.remove(id);
        let amount = text.trim().replace(',', "");
        if amount != month_amount && !amount.is_empty() {
            // A month that already differs from the line's default gets its own
            // amount; otherwise the default changes.
            let (cmd, args) = if month_amount != default_amount {
                (
                    "budget.set_month",
                    json!({"budget": budget_arg(app), "account": id, "month": app.budget.month, "amount": amount}),
                )
            } else {
                (
                    "budget.set_line",
                    json!({"budget": budget_arg(app), "account": id, "amount": amount}),
                )
            };
            match app.run(cmd, args) {
                Ok(_) => app.budget.error = None,
                Err(e) => app.budget.error = Some(e.display()),
            }
        }
    }
}

/// A progress bar of actual ÷ available; over 100 % in the muted red with the
/// overspent amount. Remaining always shows the sign, so colour is never alone.
fn progress(ui: &mut Ui, row: &Value, income: bool) {
    let available: mk_core::Decimal = row.str("available").parse().unwrap_or_default();
    let actual: mk_core::Decimal = row.str("actual").parse().unwrap_or_default();
    let percent = if available.is_zero() {
        0
    } else {
        actual
            .checked_mul(mk_core::Decimal::from(100))
            .and_then(|p| p.checked_div(available))
            .map(|p| p.round().to_string().parse::<i32>().unwrap_or(0))
            .unwrap_or(0)
            .clamp(0, 100)
    };
    let over = !income && actual > available && !available.is_zero();
    let pct = row.str("percent");
    let tip = if over {
        format!(
            "{pct}% — overspent by {}",
            fmt::group(row.str("remaining").trim_start_matches('-'))
        )
    } else if pct.is_empty() {
        "No budget this month".to_owned()
    } else {
        format!("{pct}%")
    };
    if over {
        ui.tone(Tone::Decrease);
    }
    ui.tip(tip.clone()).grow();
    ui.progress(
        &format!("Progress {} {tip}", row.at("account").str("full_name")),
        percent,
    );
}

fn not_in_budget(app: &mut App, ui: &mut Ui, r: &Value) {
    let rows = r.list("unbudgeted").to_vec();
    if rows.is_empty() {
        return;
    }
    let cols = vec![
        Column {
            weight: 3,
            ..col("Category")
        },
        Column {
            width: 120,
            ..col_num("Actual")
        },
        Column {
            width: 120,
            align: Align::Center,
            ..col("")
        },
    ];
    let mut add = None;
    ui.table("Not in budget", cols, |ui| {
        for row in &rows {
            let full = row.str("full_name").to_owned();
            ui.table_row(&format!("{full}#unbudgeted"), false, false, |ui| {
                ui.tip(full.clone());
                ui.label(full.clone());
                ui.amount(fmt::group(row.str("actual")));
                ui.named(format!("Add {full} to budget"));
                if ui.button("Add").clicked() {
                    add = Some(row.str("account").to_owned());
                }
            });
        }
    });
    if let Some(account) = add {
        let today = app.today();
        let amount = app
            .read(
                "budget.suggest",
                json!({"budget": budget_arg(app), "accounts": [account], "today": today}),
            )
            .and_then(|s| s.list("lines").first().map(|l| l.str("amount").to_owned()))
            .unwrap_or_else(|| "0".into());
        let args = json!({"budget": budget_arg(app), "account": account, "amount": amount});
        if let Err(e) = app.run("budget.set_line", args) {
            app.budget.error = Some(e.display());
        }
    }
}

// ----- year view -------------------------------------------------------------------

fn year(app: &mut App, ui: &mut Ui) {
    let year: i32 = app
        .budget
        .month
        .get(..4)
        .and_then(|y| y.parse().ok())
        .unwrap_or(2026);
    let budget = app.budget.budget.clone();
    let fresh = app
        .budget
        .year_cache
        .as_ref()
        .is_some_and(|(g, b, y, _)| *g == app.generation && *b == budget && *y == year);
    if !fresh {
        let mut reports = Vec::new();
        for m in 1..=12u8 {
            let args = json!({"budget": budget_arg(app), "month": month_text(year, m)});
            reports.push(app.read("budget.report", args).unwrap_or(Value::Null));
        }
        app.budget.year_cache = Some((app.generation, budget, year, reports));
    }
    let Some((_, _, _, reports)) = app.budget.year_cache.clone() else {
        return;
    };
    let mut lines: Vec<(String, String, i32)> = Vec::new();
    if let Some(first) = reports.first() {
        for key in ["income", "expenses"] {
            for r in first.at(key).list("rows") {
                let a = r.at("account");
                lines.push((
                    a.str("id").to_owned(),
                    a.str("full_name").to_owned(),
                    i32::try_from(a.at("depth").as_u64().unwrap_or(0)).unwrap_or(0),
                ));
            }
        }
    }
    let part = |a: &str| {
        if a.parse::<mk_core::Decimal>().is_ok_and(|d| d.is_zero()) {
            "–".to_owned()
        } else {
            fmt::group(a)
        }
    };
    let cell = |report: &Value, id: &str| -> String {
        for key in ["income", "expenses"] {
            if let Some(r) = report
                .at(key)
                .list("rows")
                .iter()
                .find(|r| r.at("account").str("id") == id)
            {
                return format!("{} / {}", part(r.str("actual")), part(r.str("budget")));
            }
        }
        String::new()
    };
    ui.secondary("Each month shows actual / budget.");
    let texts: Vec<Vec<String>> = lines
        .iter()
        .map(|(id, _, _)| reports.iter().map(|r| cell(r, id)).collect())
        .collect();
    let mut cols = vec![col("Category")];
    for (k, m) in SHORT.iter().enumerate() {
        let w = texts
            .iter()
            .filter_map(|t| t.get(k))
            .map(|t| super::period_table::text_width(t))
            .fold(super::period_table::text_width(m).max(72), i32::max);
        cols.push(Column {
            title: (*m).to_owned(),
            width: w,
            align: Align::Right,
            ..Column::default()
        });
    }
    ui.scope("budget-year", |ui| {
        ui.period_table("", cols, false, |ui| {
            for ((_, full, depth), t) in lines.iter().zip(&texts) {
                ui.row(|ui| {
                    let name = full.rsplit(':').next().unwrap_or(full);
                    ui.tip(full.clone());
                    ui.label(name);
                    for text in t {
                        if text == "– / –" {
                            ui.tone(Tone::Secondary);
                            ui.label("–");
                        } else {
                            ui.mono();
                            ui.label(text.clone());
                        }
                    }
                });
                ui.indent_last(*depth);
            }
        });
    });
}

// ----- fill from history -----------------------------------------------------------

const RANGES: [(usize, &str); 3] = [
    (3, "Last 3 months"),
    (6, "Last 6 months"),
    (12, "Last 12 months"),
];

fn suggest_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.budget.suggest.take() else {
        return;
    };
    let current = current_month(app);
    let months = RANGES.get(f.months).map_or(6, |r| r.0);
    let to = shift(&current, -1);
    let from = shift(&to, 1 - i64::try_from(months).unwrap_or(6));
    let key = format!("{from}|{to}|{}|{}", f.last, app.generation);
    if f.key != key {
        f.key = key;
        let args = json!({
            "budget": budget_arg(app), "from_month": from, "to_month": to,
            "method": if f.last { "last" } else { "average" },
        });
        f.rows = app
            .read("budget.suggest", args)
            .map(|s| {
                s.list("lines")
                    .iter()
                    .map(|l| {
                        (
                            l.str("account").to_owned(),
                            l.str("full_name").to_owned(),
                            true,
                            l.str("amount").to_owned(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    let mut apply = false;
    let mut cancel = ui.dialog_closed("budget-suggest");
    ui.dialog("budget-suggest", "Fill from history", 520, |ui| {
        ui.row(|ui| {
            let names: Vec<String> = RANGES.iter().map(|r| r.1.to_owned()).collect();
            ui.width(170);
            ui.combo("Range", &mut f.months, &names);
            ui.radio(&mut f.last, false, "Average");
            ui.radio(&mut f.last, true, "Last month");
        });
        if f.rows.is_empty() {
            ui.label("No spending or income in that range.");
        }
        let cols = vec![
            Column {
                width: 40,
                ..col("")
            },
            Column {
                weight: 3,
                ..col("Category")
            },
            Column {
                width: 120,
                ..col_num("Amount")
            },
        ];
        ui.table("", cols, |ui| {
            ui.visible_rows(9);
            for (acc, name, on, amount) in &mut f.rows {
                ui.table_row(&format!("suggest:{acc}"), false, false, |ui| {
                    ui.named(format!("Include {name}"));
                    ui.checkbox(on, "");
                    ui.label(name.clone());
                    ui.mono().align(Align::Right);
                    ui.field(&format!("Amount for {name}"), amount);
                });
            }
        });
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Apply").clicked() {
                apply = true;
            }
        });
    });
    if cancel {
        return;
    }
    if apply {
        let lines: Vec<Value> = f
            .rows
            .iter()
            .filter(|r| r.2)
            .map(|r| json!({"account": r.0, "amount": r.3.trim().replace(',', "")}))
            .collect();
        match app.run(
            "budget.apply_suggestion",
            json!({"budget": budget_arg(app), "lines": lines}),
        ) {
            Ok(_) => {
                app.notify("Budget filled from history");
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.budget.suggest = Some(f);
}
