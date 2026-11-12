//! The Projects screen (projects.md): the list (status, customer, budget, spent, %
//! used as a bar coloured at the warning percentage and over 100 %, income, net,
//! unbilled), the detail with its tabs Summary, Transactions, Mileage, Documents and
//! Billable, the project form (budget mode selector, budget lines, fund, billable
//! default and markup), Set project on transactions… (a search across registers
//! with checkboxes; one undoable command), Report… (CSV or PDF), Settings →
//! Projects, the Projects dashboard block and the invoice editor's "unbilled costs"
//! bar with its Add dialog. Everything goes through the `project.*` commands.

use serde_json::{Value, json};

use super::form::set;
use super::ui::{Column, Tone, Ui, col, col_num};
use super::util::{Json, array};
use super::{App, MessageAction};
use crate::{AppEvent, View, fmt};

pub(crate) const MODULE: &str = "projects";

const STATUSES: [(&str, &str); 5] = [
    ("planned", "Planned"),
    ("active", "Active"),
    ("on_hold", "On hold"),
    ("completed", "Completed"),
    ("cancelled", "Cancelled"),
];

/// The list's status filter: (argument, label).
const FILTERS: [(&str, &str); 7] = [
    ("", "All projects"),
    ("open", "Open (not completed or cancelled)"),
    ("planned", "Planned"),
    ("active", "Active"),
    ("on_hold", "On hold"),
    ("completed", "Completed"),
    ("cancelled", "Cancelled"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Tab {
    #[default]
    Summary,
    Transactions,
    Mileage,
    Documents,
    Billable,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Form {
    pub id: Option<String>,
    pub name: String,
    pub status: usize,
    pub customer: String,
    pub start: String,
    pub end: String,
    pub by_account: bool,
    pub budget: String,
    /// Budget lines: (expense account full name, "" for Other; amount).
    pub lines: Vec<(String, String)>,
    pub income_budget: String,
    pub fund: String,
    pub billable: bool,
    pub markup: String,
    pub mileage_rate: String,
    pub notes: String,
    pub error: Option<String>,
    pub confirm_delete: bool,
}

/// Set project on transactions…: a search with checkboxes.
#[derive(Debug, Clone, Default)]
pub(crate) struct SetForm {
    pub project: String,
    pub account: String,
    pub contains: String,
    pub from: String,
    pub to: String,
    /// Search results (txn.list rows) and which are ticked (by id).
    pub results: Vec<Value>,
    pub chosen: Vec<String>,
    pub searched: bool,
    /// The confirm before replacing other projects: how many lines.
    pub confirm: Option<u64>,
    pub error: Option<String>,
}

/// The invoice editor's Add unbilled costs dialog.
#[derive(Debug, Clone, Default)]
pub(crate) struct AddCosts {
    pub name: String,
    pub items: Vec<Value>,
    pub chosen: Vec<String>,
    pub summary: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct ProjectsState {
    pub filter: usize,
    pub detail: Option<String>,
    pub tab: Tab,
    pub form: Option<Form>,
    pub set: Option<SetForm>,
    pub add: Option<AddCosts>,
    pub count_mileage: bool,
    cache: Option<(u64, String, Value)>,
}

fn cached(app: &mut App, command: &str, args: Value) -> Value {
    let key = format!("{command}{args}");
    if let Some((g, k, v)) = &app.projects.cache
        && *g == app.generation
        && *k == key
    {
        return v.clone();
    }
    let v = app.read(command, args).unwrap_or(Value::Null);
    app.projects.cache = Some((app.generation, key, v.clone()));
    v
}

/// Open the screen on a project (Attention items, the dashboard block).
pub(crate) fn open(app: &mut App, target: &Value) {
    let id = target.str("project");
    app.projects.detail = (!id.is_empty()).then(|| id.to_owned());
    app.projects.tab = Tab::Summary;
    app.navigate(View::Projects);
}

fn money(ui: &mut Ui, text: &str) {
    super::sales::money(ui, text);
}

/// The % used bar: amber at the warning percentage, red over budget.
fn used_bar(ui: &mut Ui, name: &str, percent: &str, level: &str) {
    used_bar_w(ui, name, percent, level, 80);
}

fn used_bar_w(ui: &mut Ui, name: &str, percent: &str, level: &str, width: i32) {
    let n: i32 = percent
        .parse::<mk_core::Decimal>()
        .ok()
        .map(|p| p.round())
        .and_then(|p| p.to_string().parse().ok())
        .unwrap_or(0);
    let tone = match level {
        "over" => Tone::Error,
        "warn" => Tone::Warning,
        _ => Tone::Normal,
    };
    ui.row(|ui| {
        ui.tone(tone).width(width).tip(format!("{percent} % used"));
        ui.progress(name, n.max(0));
        let text = if percent.is_empty() {
            "—".to_owned()
        } else {
            format!("{percent} %")
        };
        match level {
            "over" => ui.tone(Tone::Error).label(text),
            "warn" => ui.tone(Tone::Warning).label(text),
            _ => ui.label(text),
        }
    });
}

/// Draw the screen.
pub(crate) fn view(app: &mut App, ui: &mut Ui) {
    if let Some(id) = app.projects.detail.clone() {
        detail(app, ui, &id);
    } else {
        list(app, ui);
    }
    dialogs(app, ui);
}

/// The dialogs of the screen (also drawn over the register and the invoice editor).
pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    form_dialog(app, ui);
    set_dialog(app, ui);
}

fn list(app: &mut App, ui: &mut Ui) {
    ui.title("Projects");
    let mut new = false;
    ui.toolbar(|ui| {
        let labels: Vec<&str> = FILTERS.iter().map(|f| f.1).collect();
        ui.named("Show");
        super::form::choice(ui, "Show", &mut app.projects.filter, &labels);
        ui.spacer();
        if ui.button("Set project on transactions…").clicked() {
            open_set(app, "", "", &[]);
        }
        new = ui.primary("+ New project").clicked();
    });
    if new {
        open_form(app, None);
    }
    let status = FILTERS.get(app.projects.filter).map_or("", |f| f.0);
    let args = if status.is_empty() {
        json!({})
    } else {
        json!({"status": status})
    };
    let data = cached(app, "project.list", args);
    let rows = data.list("projects").to_vec();
    let invoicing = data.at("invoicing").as_bool() == Some(true);
    if rows.is_empty() {
        ui.card("", |ui| {
            ui.label("No projects yet. A project gathers what a piece of work cost and brought in — a roof repair, a building fund drive, a client job — across accounts, cards and documents.");
            ui.secondary("Use + New project, then tag transactions, bills, invoices, expenses and trips with it.");
        });
        return;
    }
    let mut cols = vec![
        Column {
            weight: 3,
            ..col("Project")
        },
        Column {
            width: 100,
            ..col("Status")
        },
        Column {
            weight: 2,
            ..col("Customer")
        },
        Column {
            width: 100,
            ..col_num("Budget")
        },
        Column {
            width: 100,
            ..col_num("Spent")
        },
        Column {
            width: 150,
            ..col("% used")
        },
        Column {
            width: 100,
            ..col_num("Income")
        },
        Column {
            width: 100,
            ..col_num("Net")
        },
    ];
    if invoicing {
        cols.push(Column {
            width: 110,
            ..col_num("Unbilled")
        });
    }
    let mut open_id = None;
    ui.table("Projects", cols, |ui| {
        for r in &rows {
            let id = r.str("id").to_owned();
            let name = r.str("name").to_owned();
            ui.table_row(&format!("project:{id}"), false, false, |ui| {
                ui.named(format!("Open {name}"));
                let level = usize::try_from(r.at("level").as_u64().unwrap_or(0)).unwrap_or(0);
                let shown = match name.rsplit_once(':') {
                    Some((_, leaf)) if level > 0 => format!("{}{leaf}", "    ".repeat(level)),
                    _ => name.clone(),
                };
                ui.tip(name.clone());
                if ui.link(shown).clicked() {
                    open_id = Some(id.clone());
                }
                status_badge(ui, r.str("status"), r.str("status_label"));
                ui.label(r.at("customer_name").as_str().unwrap_or(""));
                if r.at("budget").is_null() {
                    ui.label("");
                } else {
                    money(ui, r.str("budget"));
                }
                money(ui, r.str("spent"));
                if r.at("budget").is_null() {
                    ui.secondary("No budget");
                } else {
                    used_bar(
                        ui,
                        &format!("{name} budget used"),
                        r.str("percent"),
                        r.str("level_of_use"),
                    );
                }
                money(ui, r.str("income"));
                money(ui, r.str("net"));
                if invoicing {
                    let u = r.at("unbilled");
                    if u.at("count").as_u64().unwrap_or(0) == 0 {
                        ui.label("");
                    } else {
                        ui.tip(format!("{} unbilled costs", u.at("count")));
                        money(ui, u.str("amount"));
                    }
                }
            });
        }
        let t = data.at("totals");
        ui.table_row("project:total", false, false, |ui| {
            ui.bold().label("Total");
            ui.label("");
            ui.label("");
            ui.bold().amount(fmt::group(t.str("budget")));
            ui.bold().amount(fmt::group(t.str("spent")));
            ui.label("");
            ui.bold().amount(fmt::group(t.str("income")));
            ui.bold().amount(fmt::group(t.str("net")));
            if invoicing {
                ui.label("");
            }
        });
    });
    ui.caption(format!(
        "Bars turn amber at {} % of the budget and red over it (Settings → Projects). Parents include their sub-projects; the total adds top-level projects.",
        data.str("warn_percent")
    ));
    if let Some(id) = open_id {
        app.projects.detail = Some(id);
        app.projects.tab = Tab::Summary;
    }
}

fn status_badge(ui: &mut Ui, status: &str, label: &str) {
    let tone = match status {
        "active" => Tone::Success,
        "planned" => Tone::Accent,
        "on_hold" => Tone::Warning,
        _ => Tone::Secondary,
    };
    ui.badge(label, tone);
}

// ----- detail ---------------------------------------------------------------------------

fn detail(app: &mut App, ui: &mut Ui, id: &str) {
    let g = cached(app, "project.get", json!({"project": id}));
    if g.is_null() {
        app.projects.detail = None;
        ui.title("Projects");
        return;
    }
    let name = g.str("name").to_owned();
    ui.title(name.clone());
    let invoicing = !g.at("billable_items").is_null();
    let mut act: Option<&str> = None;
    ui.toolbar(|ui| {
        if ui.button("← All projects").clicked() {
            act = Some("back");
        }
        status_badge(ui, g.str("status"), g.str("status_label"));
        ui.spacer();
        if ui.button("Edit…").clicked() {
            act = Some("edit");
        }
        if g.at("open").as_bool() == Some(true) {
            if ui.button("Mark completed").clicked() {
                act = Some("complete");
            }
        } else if ui.button("Reopen").clicked() {
            act = Some("reopen");
        }
        ui.tip("Export the project report");
        if let Some(i) = ui
            .menu(
                "Report…",
                &["Export CSV…".to_owned(), "Export PDF…".to_owned()],
            )
            .chosen
        {
            act = Some(if i == 0 { "csv" } else { "pdf" });
        }
        if ui.button("Set project on transactions…").clicked() {
            act = Some("set");
        }
    });
    let mut facts: Vec<String> = Vec::new();
    if let Some(c) = g.at("customer_name").as_str() {
        facts.push(format!("For {c}"));
    }
    if let Some(f) = g.at("fund_name").as_str() {
        facts.push(format!("Fund: {f}"));
    }
    match (g.str("start"), g.str("end")) {
        ("", "") => {}
        (s, "") => facts.push(format!("From {}", fmt::date(s))),
        ("", e) => facts.push(format!("Until {}", fmt::date(e))),
        (s, e) => facts.push(format!("{} – {}", fmt::date(s), fmt::date(e))),
    }
    if g.at("billable").as_bool() == Some(true) {
        let m = g.str("markup_percent");
        facts.push(if m == "0" {
            "Billable".to_owned()
        } else {
            format!("Billable, {m} % markup")
        });
    }
    if !facts.is_empty() {
        ui.secondary(facts.join(" · "));
    }
    if !g.str("notes").is_empty() {
        ui.caption(g.str("notes"));
    }
    ui.row(|ui| {
        let mut tabs = vec![
            (Tab::Summary, "Summary"),
            (Tab::Transactions, "Transactions"),
            (Tab::Mileage, "Mileage"),
            (Tab::Documents, "Documents"),
        ];
        if invoicing {
            tabs.push((Tab::Billable, "Billable"));
        }
        for (t, label) in tabs {
            ui.named(label);
            if ui.tab(label, app.projects.tab == t).clicked() {
                app.projects.tab = t;
            }
        }
    });
    if !invoicing && app.projects.tab == Tab::Billable {
        app.projects.tab = Tab::Summary;
    }
    match app.projects.tab {
        Tab::Summary => ui.scope("summary", |ui| summary_tab(app, ui, &g)),
        Tab::Transactions => ui.scope("txns", |ui| transactions_tab(app, ui, &g)),
        Tab::Mileage => ui.scope("mileage", |ui| mileage_tab(ui, &g)),
        Tab::Documents => ui.scope("docs", |ui| documents_tab(app, ui, &g)),
        Tab::Billable => ui.scope("billable", |ui| billable_tab(ui, &g)),
    }
    let report_args =
        || json!({"project": g.str("id"), "count_mileage": app.projects.count_mileage});
    match act {
        Some("back") => app.projects.detail = None,
        Some("edit") => open_form(app, Some(&g)),
        Some("complete") | Some("reopen") => {
            let status = if act == Some("complete") {
                "completed"
            } else {
                "active"
            };
            match app.run(
                "project.set_status",
                json!({"project": g.str("id"), "status": status}),
            ) {
                Ok(_) => app.notify_with(
                    format!(
                        "{name} is {}.",
                        if status == "completed" {
                            "completed"
                        } else {
                            "active again"
                        }
                    ),
                    MessageAction::Undo,
                ),
                Err(e) => app.notify_error(&e),
            }
        }
        Some("csv") => {
            let a = json!({"report": "report.project", "format": "csv", "args": report_args()});
            if let Some(e) = app.read("report.export", a) {
                app.emit(AppEvent::SaveFile {
                    file_name: e.str("file_name").to_owned(),
                    content: e.str("content").to_owned(),
                });
            }
        }
        Some("pdf") => {
            let a = json!({"report": "report.project", "format": "pdf", "args": report_args()});
            if let Some(p) = app.read("report.export", a)
                && let Err(e) = super::sales::save_pdf(app, &p)
            {
                app.notify(e);
            }
        }
        Some("set") => open_set(app, &name, "", &[]),
        _ => {}
    }
}

fn summary_tab(app: &mut App, ui: &mut Ui, g: &Value) {
    let s = if app.projects.count_mileage {
        cached(
            app,
            "report.project",
            json!({"project": g.str("id"), "count_mileage": true}),
        )
    } else {
        g.at("summary").clone()
    };
    let cols4 = || {
        vec![
            Column {
                weight: 1,
                ..col("Budget")
            },
            Column {
                weight: 1,
                ..col("Spent")
            },
            Column {
                weight: 1,
                ..col("Remaining")
            },
            Column {
                weight: 1,
                ..col("Net")
            },
        ]
    };
    ui.table("At a glance", cols4(), |ui| {
        ui.table_row("glance", false, false, |ui| {
            ui.big_heading(if s.at("budget").is_null() {
                "—".to_owned()
            } else {
                fmt::group(s.str("budget"))
            });
            ui.big_heading(fmt::group(s.str("cost")));
            ui.big_heading(if s.at("remaining").is_null() {
                "—".to_owned()
            } else {
                fmt::group(s.str("remaining"))
            });
            ui.big_heading(fmt::group(s.str("net")));
        });
    });
    if !s.at("budget").is_null() {
        ui.card("", |ui| {
            ui.form("Budget used", |ui| {
                used_bar_w(
                    ui,
                    "Share of the budget used",
                    s.str("percent"),
                    s.str("level_of_use"),
                    420,
                );
            });
        });
    }
    ui.wrap(|ui| {
        ui.card("Income", |ui| {
            account_rows(ui, s.at("income"), "No tagged income.");
            if let Some(b) = s.at("income_budget").as_str() {
                ui.secondary(format!("Expected income {}", fmt::group(b)));
            }
        });
        ui.card("Expenses by account", |ui| {
            account_rows(ui, s.at("expenses"), "No tagged expenses yet.");
            if s.at("count_mileage").as_bool() == Some(true) {
                ui.row(|ui| {
                    ui.label("Mileage counted as a cost");
                    ui.spacer();
                    ui.amount(fmt::group(s.str("mileage_cost")));
                });
            }
        });
    });
    let lines = s.list("budget_lines").to_vec();
    if !lines.is_empty() {
        let cols = vec![
            Column {
                weight: 3,
                ..col("Budget line")
            },
            Column {
                width: 120,
                ..col_num("Budget")
            },
            Column {
                width: 120,
                ..col_num("Actual")
            },
            Column {
                width: 120,
                ..col_num("Remaining")
            },
            Column {
                width: 90,
                ..col_num("% used")
            },
        ];
        ui.table("Budget vs actual", cols, |ui| {
            for (i, l) in lines.iter().enumerate() {
                let total = l.str("name") == "Total";
                ui.table_row(&format!("budget:{i}"), false, false, |ui| {
                    if total {
                        ui.bold().label(l.str("name"));
                    } else {
                        ui.label(l.str("name"));
                    }
                    if l.at("budget").is_null() {
                        ui.label("");
                    } else {
                        money(ui, l.str("budget"));
                    }
                    money(ui, l.str("actual"));
                    let rem = l.str("remaining");
                    if rem.starts_with('-') {
                        ui.tone(Tone::Error).amount(fmt::group(rem));
                    } else {
                        money(ui, rem);
                    }
                    let p = l.str("percent");
                    ui.amount(if p.is_empty() {
                        String::new()
                    } else {
                        format!("{p} %")
                    });
                });
            }
        });
    }
    let m = s.at("mileage");
    ui.card("Mileage", |ui| {
        let rows = m.list("rows");
        if rows.is_empty() {
            ui.secondary("No trips tagged with this project.");
        }
        for (i, r) in rows.iter().enumerate() {
            ui.scope(format!("mi-{i}"), |ui| {
                ui.row(|ui| {
                    ui.label(format!("{} — {} mi", r.str("label"), r.str("miles")));
                    ui.spacer();
                    ui.amount(fmt::group(r.str("value")));
                });
            });
        }
        let mut count = app.projects.count_mileage;
        if ui.checkbox(&mut count, "Count mileage as a cost").clicked() {
            app.projects.count_mileage = count;
        }
        ui.caption("Shown separately at each trip's standard rate; it is not added to the expenses unless ticked (reimbursed trips are already expenses).");
    });
}

fn account_rows(ui: &mut Ui, sec: &Value, empty: &str) {
    let rows = sec.list("rows");
    if rows.is_empty() {
        ui.secondary(empty.to_owned());
        return;
    }
    for (i, r) in rows.iter().enumerate() {
        ui.scope(format!("acct-{i}"), |ui| {
            ui.row(|ui| {
                ui.label(r.str("name"));
                ui.spacer();
                ui.amount(fmt::group(r.str("amount")));
            });
        });
    }
    ui.row(|ui| {
        ui.bold().label("Total");
        ui.spacer();
        ui.bold().amount(fmt::group(sec.str("total")));
    });
}

fn transactions_tab(app: &mut App, ui: &mut Ui, g: &Value) {
    let rows = g.list("transactions").to_vec();
    if rows.is_empty() {
        ui.card("", |ui| {
            ui.label("Nothing is tagged with this project yet.");
            ui.secondary("Pick the project on a split, a bill or invoice line, an expense or a trip, or use Set project on transactions….");
        });
        return;
    }
    let subs = !g.list("sub_projects").is_empty();
    let mut cols = vec![
        Column {
            width: 100,
            ..col("Date")
        },
        Column {
            weight: 2,
            ..col("Payee")
        },
        Column {
            weight: 2,
            ..col("Account")
        },
        Column {
            weight: 2,
            ..col("Memo")
        },
        Column {
            width: 110,
            ..col_num("Amount")
        },
    ];
    if subs {
        cols.push(Column {
            weight: 2,
            ..col("Project")
        });
    }
    cols.push(Column {
        width: 90,
        ..col("")
    });
    let mut open: Option<(String, String)> = None;
    ui.table("Tagged lines", cols, |ui| {
        for (i, r) in rows.iter().enumerate() {
            ui.table_row(&format!("line:{i}"), false, false, |ui| {
                ui.label(fmt::date(r.str("date")));
                super::sales::cell(ui, r.str("payee"));
                super::sales::cell(ui, r.str("account_name"));
                if r.at("counted").as_bool() == Some(false) {
                    ui.secondary("transfer, not counted");
                } else if r.at("billable").as_bool() == Some(true) {
                    ui.row(|ui| {
                        super::sales::cell(ui, r.str("memo"));
                        ui.badge("Billable", Tone::Accent);
                    });
                } else {
                    super::sales::cell(ui, r.str("memo"));
                }
                money(ui, r.str("amount"));
                if subs {
                    ui.label(r.at("project_name").as_str().unwrap_or(""));
                }
                ui.named(format!(
                    "Open in register: {} {}",
                    r.str("payee"),
                    fmt::date(r.str("date"))
                ));
                if ui.button("Open").clicked() {
                    open = Some((r.str("account").to_owned(), r.str("txn").to_owned()));
                }
            });
        }
    });
    if let Some((account, txn)) = open {
        app.register.pending_select = Some(txn);
        app.navigate(View::Register { account });
    }
}

fn mileage_tab(ui: &mut Ui, g: &Value) {
    let m = g.at("summary").at("mileage");
    let trips = m.list("trips").to_vec();
    if trips.is_empty() {
        ui.card("", |ui| {
            ui.label("No trips are tagged with this project. Pick the project on a trip (or on the phone's Mileage form).");
        });
        return;
    }
    let cols = vec![
        Column {
            width: 100,
            ..col("Date")
        },
        Column {
            weight: 3,
            ..col("Route")
        },
        Column {
            weight: 2,
            ..col("Purpose")
        },
        Column {
            weight: 2,
            ..col("Vehicle")
        },
        Column {
            width: 80,
            ..col_num("Miles")
        },
        Column {
            width: 80,
            ..col_num("Rate (¢)")
        },
        Column {
            width: 100,
            ..col_num("Value")
        },
    ];
    ui.table("Trips", cols, |ui| {
        for t in &trips {
            ui.table_row(&format!("trip:{}", t.str("id")), false, false, |ui| {
                ui.label(fmt::date(t.str("date")));
                super::sales::cell(ui, t.str("route"));
                super::sales::cell(ui, t.at("purpose").as_str().unwrap_or(""));
                ui.label(t.str("vehicle"));
                ui.amount(t.str("miles"));
                ui.amount(t.str("rate_cents"));
                money(ui, t.str("value"));
            });
        }
        ui.table_row("trip:total", false, false, |ui| {
            ui.bold().label("Total");
            ui.label("");
            ui.label("");
            ui.label("");
            ui.bold().amount(m.str("miles"));
            ui.label("");
            ui.bold().amount(fmt::group(m.str("value")));
        });
    });
    ui.caption("The value is miles × the trip purpose's standard rate. It is shown apart from the expenses.");
}

fn documents_tab(app: &mut App, ui: &mut Ui, g: &Value) {
    let docs = g.list("documents").to_vec();
    if docs.is_empty() {
        ui.card("", |ui| {
            ui.label("No invoices, bills or estimates are tagged with this project.");
        });
        return;
    }
    let cols = vec![
        Column {
            width: 120,
            ..col("Kind")
        },
        Column {
            weight: 2,
            ..col("Number")
        },
        Column {
            width: 100,
            ..col("Date")
        },
        Column {
            width: 120,
            ..col_num("Total")
        },
        Column {
            width: 100,
            ..col("Status")
        },
        Column {
            width: 90,
            ..col("")
        },
    ];
    let mut open: Option<(String, String)> = None;
    ui.table("Documents", cols, |ui| {
        for d in &docs {
            ui.table_row(&format!("doc:{}", d.str("id")), false, false, |ui| {
                ui.label(match d.str("kind") {
                    "invoice" => "Invoice",
                    "estimate" => "Estimate",
                    "credit_memo" => "Credit memo",
                    _ => "Bill",
                });
                ui.label(d.at("number").as_str().unwrap_or(""));
                ui.label(fmt::date(d.str("date")));
                money(ui, d.str("total"));
                ui.label(d.str("status"));
                ui.named(format!("Open {} {}", d.str("kind"), d.str("number")));
                if ui.button("Open").clicked() {
                    open = Some((d.str("kind").to_owned(), d.str("id").to_owned()));
                }
            });
        }
    });
    match open {
        Some((k, id)) if k == "bill" => {
            super::purchases::open_editor(app, "bill", &id);
            app.navigate(View::Purchases);
        }
        Some((k, id)) => {
            let kind: &'static str = match k.as_str() {
                "estimate" => "estimate",
                _ => "invoice",
            };
            super::sales::open_editor(app, kind, &id);
            app.navigate(View::Sales);
        }
        None => {}
    }
}

fn billable_tab(ui: &mut Ui, g: &Value) {
    let b = g.at("billable_items");
    let cols = |billed: bool| {
        let mut c = vec![
            Column {
                width: 100,
                ..col("Date")
            },
            Column {
                weight: 2,
                ..col("Payee")
            },
            Column {
                weight: 2,
                ..col("Account")
            },
            Column {
                weight: 2,
                ..col("Memo")
            },
            Column {
                width: 110,
                ..col_num("Amount")
            },
            Column {
                width: 120,
                ..col_num("With markup")
            },
        ];
        if billed {
            c.push(Column {
                width: 110,
                ..col("Invoice")
            });
        }
        c
    };
    let row = |ui: &mut Ui, i: &Value, billed: bool| {
        ui.table_row(&format!("bill:{}", i.str("key")), false, false, |ui| {
            ui.label(fmt::date(i.str("date")));
            super::sales::cell(ui, i.str("payee"));
            ui.label(i.str("account_name"));
            super::sales::cell(ui, i.str("memo"));
            money(ui, i.str("amount"));
            money(ui, i.str("with_markup"));
            if billed {
                ui.label(i.at("invoice_number").as_str().unwrap_or(""));
            }
        });
    };
    let open = b.list("unbilled").to_vec();
    let done = b.list("billed").to_vec();
    ui.table(
        &format!(
            "Unbilled ({}, {} with markup)",
            fmt::group(b.str("unbilled_total")),
            fmt::group(b.str("unbilled_with_markup"))
        ),
        cols(false),
        |ui| {
            for i in &open {
                row(ui, i, false);
            }
        },
    );
    if open.is_empty() {
        ui.secondary("Nothing to bill. Tick Billable on a tagged expense line or trip; new lines take the project's default.");
    } else {
        ui.caption("To bill them, start an invoice for the project's customer and use Add… on the unbilled costs bar.");
    }
    ui.table("Billed", cols(true), |ui| {
        for i in &done {
            row(ui, i, true);
        }
    });
    if done.is_empty() {
        ui.secondary("Nothing billed yet.");
    }
}

// ----- the project form -------------------------------------------------------------------

fn open_form(app: &mut App, g: Option<&Value>) {
    let f = match g {
        None => Form {
            status: 1,
            markup: "0".into(),
            ..Form::default()
        },
        Some(g) => Form {
            id: Some(g.str("id").to_owned()),
            name: g.str("name").to_owned(),
            status: STATUSES
                .iter()
                .position(|s| s.0 == g.str("status"))
                .unwrap_or(1),
            customer: g.at("customer_name").as_str().unwrap_or("").to_owned(),
            start: fmt::date(g.str("start")),
            end: fmt::date(g.str("end")),
            by_account: g.str("budget_mode") == "by_account",
            budget: g.at("budget").as_str().unwrap_or("").to_owned(),
            lines: g
                .list("budget_lines")
                .iter()
                .map(|l| {
                    (
                        if l.at("account").is_null() {
                            String::new()
                        } else {
                            l.str("account_name").to_owned()
                        },
                        l.str("amount").to_owned(),
                    )
                })
                .collect(),
            income_budget: g.at("income_budget").as_str().unwrap_or("").to_owned(),
            fund: g.at("fund_name").as_str().unwrap_or("").to_owned(),
            billable: g.at("billable").as_bool() == Some(true),
            markup: g.str("markup_percent").to_owned(),
            mileage_rate: g.at("mileage_bill_rate").as_str().unwrap_or("").to_owned(),
            notes: g.str("notes").to_owned(),
            ..Form::default()
        },
    };
    app.projects.form = Some(f);
}

fn form_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.projects.form.take() else {
        return;
    };
    let lists = app.lists().clone();
    let on = |m: &str| lists.modules.iter().any(|x| x == m);
    let expenses = super::lists::accounts_where(&lists, |a| a.str("type") == "expense");
    let customers = super::lists::names(&lists.customers, false);
    let parties: Vec<String> = if customers.is_empty() {
        array(app.read("party.list", json!({})))
            .iter()
            .map(|p| p.str("name").to_owned())
            .collect()
    } else {
        customers
    };
    let funds = super::lists::names(&lists.funds, true);
    let (mut save, mut cancel, mut delete, mut fund_all) =
        (false, ui.dialog_closed("project-form"), false, false);
    let untagged =
        f.id.as_ref()
            .map(|id| {
                cached(app, "project.get", json!({"project": id}))
                    .at("untagged_fund_splits")
                    .as_u64()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
    let title = if f.id.is_some() {
        "Edit project"
    } else {
        "New project"
    };
    ui.dialog("project-form", title, 620, |ui| {
        ui.group(|ui| {
            ui.form("Name", |ui| {
                ui.width(300).hint("e.g. Roof repair, or House:Roof repair");
                ui.field("Name", &mut f.name);
            });
            ui.form("Status", |ui| {
                let labels: Vec<&str> = STATUSES.iter().map(|s| s.1).collect();
                super::form::choice(ui, "Status", &mut f.status, &labels);
            });
            ui.form("Customer", |ui| {
                super::form::pick(ui, "Customer", &mut f.customer, &parties, Some("None"));
            });
            ui.form("Start", |ui| {
                super::register::date_field(ui, "Start", &mut f.start);
                ui.label("End");
                super::register::date_field(ui, "End", &mut f.end);
            });
            ui.form("Budget", |ui| {
                ui.radio(&mut f.by_account, false, "Total");
                ui.radio(&mut f.by_account, true, "By account");
            });
            if f.by_account {
                if f.lines.is_empty() {
                    let total = f.budget.trim().to_owned();
                    f.lines.push((String::new(), total));
                }
                let mut remove = None;
                let n = f.lines.len();
                for (i, (acct, amount)) in f.lines.iter_mut().enumerate() {
                    ui.scope(format!("line-{i}"), |ui| {
                        ui.form(if i == 0 { "Budget lines" } else { "" }, |ui| {
                            super::form::pick(
                                ui,
                                &format!("Budget line {} account", i + 1),
                                acct,
                                &expenses,
                                Some("Other"),
                            );
                            ui.width(120).mono().hint("0.00");
                            ui.field(&format!("Budget line {} amount", i + 1), amount);
                            if n > 1 {
                                ui.named(format!("Remove budget line {}", i + 1));
                                if ui.button("Remove").clicked() {
                                    remove = Some(i);
                                }
                            }
                        });
                    });
                }
                if let Some(i) = remove {
                    f.lines.remove(i);
                }
                let total = f.lines.iter().fold(mk_core::Decimal::ZERO, |a, (_, v)| {
                    a.checked_add(
                        v.trim().replace(',', "").parse().unwrap_or_default(),
                    )
                    .unwrap_or(a)
                });
                ui.form("", |ui| {
                    if ui.button("+ Add line").clicked() {
                        f.lines.push((String::new(), String::new()));
                    }
                    ui.secondary(format!(
                        "Total {}",
                        fmt::group(&super::sales::fmt_money(total))
                    ));
                });
            } else {
                ui.form("Budget amount", |ui| {
                    ui.width(140).mono().hint("0.00");
                    ui.field("Budget amount", &mut f.budget);
                });
            }
            ui.form("Expected income", |ui| {
                ui.width(140).mono().hint("optional");
                ui.field("Expected income", &mut f.income_budget);
            });
            if on("funds") {
                ui.form("Fund", |ui| {
                    super::form::pick(ui, "Fund", &mut f.fund, &funds, Some("None"));
                });
                if untagged > 0 && !f.fund.is_empty() {
                    ui.form("", |ui| {
                        ui.named("Also set the fund on tagged splits");
                        fund_all = ui
                            .button(format!(
                                "Also set the fund on {untagged} tagged split{} without a fund",
                                if untagged == 1 { "" } else { "s" }
                            ))
                            .clicked();
                    });
                }
            }
            if on("invoicing") {
                ui.form("Billable", |ui| {
                    ui.checkbox(&mut f.billable, "New tagged expenses are billable");
                });
                ui.form("Markup (%)", |ui| {
                    ui.width(80).mono();
                    ui.field("Markup (%)", &mut f.markup);
                });
                if on("mileage") {
                    ui.form("Bill trips at (¢/mi)", |ui| {
                        ui.width(80).mono().hint("standard");
                        ui.field("Bill trips at (cents per mile)", &mut f.mileage_rate);
                    });
                }
            }
            ui.form("Notes", |ui| {
                ui.width(360);
                ui.multiline("Notes", &mut f.notes);
            });
        });
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        if f.confirm_delete {
            ui.banner(Tone::Warning, |ui| {
                ui.label("Delete this project? Only a project that was never used can be deleted; otherwise mark it completed or cancelled.");
                ui.row(|ui| {
                    if ui.button("Delete project").clicked() {
                        delete = true;
                    }
                    if ui.button("Keep it").clicked() {
                        f.confirm_delete = false;
                    }
                });
            });
        }
        ui.actions(|ui| {
            if f.id.is_some() && ui.button("Delete…").clicked() {
                f.confirm_delete = true;
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if fund_all && let Some(id) = &f.id {
        match app.run("project.set_fund_on_tagged", json!({"project": id})) {
            Ok(v) => app.notify_with(
                format!("Set the fund on {} tagged splits.", v.at("changed")),
                MessageAction::Undo,
            ),
            Err(e) => f.error = Some(e.display()),
        }
    }
    if delete {
        match app.run("project.delete", json!({"project": f.id})) {
            Ok(_) => {
                app.projects.detail = None;
                app.notify_with("Deleted the project.", MessageAction::Undo);
                return;
            }
            Err(e) => {
                f.error = Some(e.display());
                f.confirm_delete = false;
            }
        }
    }
    if save {
        let read = |t: &str| -> Result<String, String> {
            match fmt::read_date(t) {
                Ok(Some(d)) => Ok(d.to_string()),
                Ok(None) => Ok(String::new()),
                Err(e) => Err(e),
            }
        };
        let (Ok(start), Ok(end)) = (read(&f.start), read(&f.end)) else {
            f.error = Some("Check the dates.".into());
            app.projects.form = Some(f);
            return;
        };
        let mut a = json!({
            "name": f.name,
            "status": STATUSES.get(f.status).map_or("active", |s| s.0),
            "customer": f.customer,
            "start": start,
            "end": end,
            "budget_mode": if f.by_account { "by_account" } else { "total" },
            "income_budget": f.income_budget,
            "notes": f.notes,
        });
        if f.by_account {
            set(
                &mut a,
                "budget_lines",
                json!(
                    f.lines
                        .iter()
                        .filter(|(acct, v)| !(acct.is_empty() && v.trim().is_empty()))
                        .map(|(acct, v)| json!({
                            "account": acct,
                            "amount": v.trim().replace(',', "").parse::<mk_core::Decimal>().map(|d| d.to_string()).unwrap_or_else(|_| v.trim().to_owned()),
                        }))
                        .collect::<Vec<_>>()
                ),
            );
        } else {
            set(&mut a, "budget", json!(f.budget));
        }
        if app.module_on("funds") {
            set(&mut a, "fund", json!(f.fund));
        }
        if app.module_on("invoicing") {
            set(&mut a, "billable", json!(f.billable));
            set(&mut a, "markup_percent", json!(f.markup));
            if app.module_on("mileage") {
                set(&mut a, "mileage_bill_rate", json!(f.mileage_rate));
            }
        }
        let (cmd, label) = match &f.id {
            Some(id) => {
                // Switching modes first (the command keeps the sum or makes Other),
                // then the lines or amount typed.
                set(&mut a, "project", json!(id));
                ("project.update", "Saved the project.")
            }
            None => ("project.create", "Created the project."),
        };
        match app.run(cmd, a) {
            Ok(v) => {
                app.notify_with(label, MessageAction::Undo);
                if f.id.is_none() {
                    app.projects.detail = Some(v.str("id").to_owned());
                    app.projects.tab = Tab::Summary;
                }
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.projects.form = Some(f);
}

// ----- Set project on transactions… ------------------------------------------------------------

/// Open the Set project dialog: for `project` (a name, or "" to choose), searching
/// `account` (an id, or every register), with `ticked` transactions chosen.
pub(crate) fn open_set(app: &mut App, project: &str, account: &str, ticked: &[String]) {
    let account_name = if account.is_empty() {
        String::new()
    } else {
        app.lists()
            .accounts
            .iter()
            .find(|a| a.str("id") == account)
            .map(|a| a.str("full_name").to_owned())
            .unwrap_or_default()
    };
    let mut f = SetForm {
        project: project.to_owned(),
        account: account_name,
        chosen: ticked.to_vec(),
        ..SetForm::default()
    };
    // The project's dates, or the last 90 days.
    if let Some(p) = app
        .lists()
        .projects
        .iter()
        .find(|p| p.str("name") == project)
        .cloned()
        && let Some(g) = app.read("project.get", json!({"project": p.str("id")}))
    {
        f.from = fmt::date(g.str("start"));
        f.to = fmt::date(g.str("end"));
    }
    if f.from.is_empty() {
        let today = app.today();
        f.from = today
            .parse::<mk_core::Date>()
            .ok()
            .and_then(|d| d.add_days(-90))
            .map(|d| fmt::date(&d.to_string()))
            .unwrap_or_default();
    }
    search(app, &mut f);
    app.projects.set = Some(f);
}

fn search(app: &mut App, f: &mut SetForm) {
    let mut a = json!({"limit": 500});
    let accounts = app.lists().accounts.clone();
    if !f.account.trim().is_empty()
        && let Some(acc) = accounts.iter().find(|x| x.str("full_name") == f.account)
    {
        set(&mut a, "account", json!(acc.str("id")));
    }
    if let Ok(Some(d)) = fmt::read_date(&f.from) {
        set(&mut a, "from", json!(d.to_string()));
    }
    if let Ok(Some(d)) = fmt::read_date(&f.to) {
        set(&mut a, "to", json!(d.to_string()));
    }
    let q = f.contains.trim().to_lowercase();
    let rows = array(app.read("txn.list", a));
    f.results = rows
        .into_iter()
        .filter(|t| t.str("status") != "voided")
        .filter(|t| {
            q.is_empty()
                || t.str("payee").to_lowercase().contains(&q)
                || t.str("memo").to_lowercase().contains(&q)
                || t.list("splits").iter().any(|s| {
                    s.str("memo").to_lowercase().contains(&q)
                        || s.str("account_name").to_lowercase().contains(&q)
                })
        })
        .collect();
    f.results.sort_by(|a, b| b.str("date").cmp(a.str("date")));
    f.searched = true;
}

/// The category side of a transaction, its amount and its current project names.
fn txn_cells(app: &mut App, t: &Value) -> (String, String, String) {
    let accounts = app.lists().accounts.clone();
    let projects = app.lists().projects.clone();
    let kind = |id: &str| {
        accounts
            .iter()
            .find(|a| a.str("id") == id)
            .map(|a| a.str("type").to_owned())
            .unwrap_or_default()
    };
    let cats: Vec<&Value> = t
        .list("splits")
        .iter()
        .filter(|s| matches!(kind(s.str("account")).as_str(), "income" | "expense"))
        .collect();
    let category = match cats.as_slice() {
        [] => "Transfer".to_owned(),
        [one] => one.str("account_name").to_owned(),
        many => format!("Split ({})", many.len()),
    };
    // As the register shows it: the money accounts' side (spending negative).
    let amount = t
        .list("splits")
        .iter()
        .filter(|s| matches!(kind(s.str("account")).as_str(), "asset" | "liability"))
        .fold(mk_core::Decimal::ZERO, |a, s| {
            a.checked_add(s.str("value").parse().unwrap_or_default())
                .unwrap_or(a)
        });
    let mut names: Vec<String> = t
        .list("splits")
        .iter()
        .filter_map(|s| s.at("tags").at("project").as_str())
        .filter_map(|id| {
            projects
                .iter()
                .find(|p| p.str("id") == id)
                .map(|p| p.str("name").to_owned())
        })
        .collect();
    names.dedup();
    (category, super::sales::fmt_money(amount), names.join(", "))
}

fn set_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.projects.set.take() else {
        return;
    };
    let mut close = ui.dialog_closed("set-project");
    let projects = super::lists::names(&app.lists().projects, true);
    let money_accounts = super::lists::accounts_where(&app.lists().clone(), |a| {
        matches!(a.str("type"), "asset" | "liability")
    });
    let mut do_search = false;
    let mut apply = false;
    let mut replace = false;
    let cells: Vec<(String, String, String)> = f
        .results
        .clone()
        .iter()
        .map(|t| txn_cells(app, t))
        .collect();
    ui.dialog("set-project", "Set project on transactions", 880, |ui| {
        ui.form("Project", |ui| {
            super::form::pick(ui, "Project", &mut f.project, &projects, Some("No project (clear)"));
        });
        ui.form("Search", |ui| {
            super::form::pick(ui, "Account", &mut f.account, &money_accounts, Some("All registers"));
            ui.width(220).hint("payee, memo or category");
            let r = ui.field("Contains", &mut f.contains);
            do_search |= r.submitted;
            super::register::date_field(ui, "From", &mut f.from);
            super::register::date_field(ui, "To", &mut f.to);
            do_search |= ui.button("Search").clicked();
        });
        let cols = vec![
            Column {
                width: 40,
                ..col("")
            },
            Column {
                width: 100,
                ..col("Date")
            },
            Column {
                weight: 3,
                ..col("Payee")
            },
            Column {
                weight: 3,
                ..col("Category")
            },
            Column {
                width: 110,
                ..col_num("Amount")
            },
            Column {
                weight: 2,
                ..col("Project now")
            },
        ];
        let title = format!("{} found, {} ticked", f.results.len(), f.chosen.len());
        ui.table(&title, cols, |ui| {
            for (t, (cat, amount, now)) in f.results.iter().zip(&cells) {
                let id = t.str("id").to_owned();
                ui.table_row(&format!("txn:{id}"), false, false, |ui| {
                    let mut on = f.chosen.contains(&id);
                    ui.named(format!(
                        "Tick {} {}",
                        t.str("payee"),
                        fmt::date(t.str("date"))
                    ));
                    if ui.checkbox(&mut on, "").clicked() {
                        if on {
                            f.chosen.push(id.clone());
                        } else {
                            f.chosen.retain(|x| *x != id);
                        }
                    }
                    ui.label(fmt::date(t.str("date")));
                    super::sales::cell(ui, t.str("payee"));
                    super::sales::cell(ui, cat.clone());
                    money(ui, amount);
                    ui.label(now.clone());
                });
            }
        });
        if f.searched && f.results.is_empty() {
            ui.secondary("Nothing found. Widen the dates or clear the text.");
        }
        ui.caption("The project goes on the income and expense lines (on every line of a transfer, which is shown as not counted). One change: Undo takes it back.");
        if let Some(n) = f.confirm {
            ui.banner(Tone::Warning, |ui| {
                ui.label(format!(
                    "{n} line{} already {} another project. Replace {}?",
                    if n == 1 { "" } else { "s" },
                    if n == 1 { "has" } else { "have" },
                    if n == 1 { "it" } else { "them" }
                ));
                ui.row(|ui| {
                    replace = ui.button("Replace").clicked();
                    if ui.button("Keep them").clicked() {
                        f.confirm = None;
                    }
                });
            });
        }
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Select all").clicked() {
                f.chosen = f.results.iter().map(|t| t.str("id").to_owned()).collect();
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
            ui.enabled(!f.chosen.is_empty());
            let label = if f.project.is_empty() {
                format!("Clear the project on {}", f.chosen.len())
            } else {
                format!("Set project on {}", f.chosen.len())
            };
            apply = ui.primary(label).clicked();
        });
    });
    if close {
        return;
    }
    if do_search {
        search(app, &mut f);
    }
    if apply || replace {
        let args = json!({"transactions": f.chosen, "project": f.project});
        if apply && !replace {
            let mut p = args.clone();
            set(&mut p, "preview", json!(true));
            if let Some(v) = app.read("txn.set_project", p)
                && v.at("replaces").as_u64().unwrap_or(0) > 0
            {
                f.confirm = v.at("replaces").as_u64();
                app.projects.set = Some(f);
                return;
            }
        }
        let mut a = args;
        if replace {
            set(&mut a, "replace", json!(true));
        }
        match app.run("txn.set_project", a) {
            Ok(v) => {
                app.notify_with(
                    format!(
                        "{} on {} transaction{}.",
                        if f.project.is_empty() {
                            "Cleared the project".to_owned()
                        } else {
                            format!("Set {}", f.project)
                        },
                        v.at("transactions"),
                        if v.at("transactions").as_u64() == Some(1) {
                            ""
                        } else {
                            "s"
                        }
                    ),
                    MessageAction::Undo,
                );
                return;
            }
            Err(e) => {
                f.error = Some(e.display());
                f.confirm = None;
            }
        }
    }
    app.projects.set = Some(f);
}

// ----- the invoice editor's unbilled costs --------------------------------------------------

/// "N unbilled costs for Roof repair (1,240.00) — Add…" above an invoice's lines
/// when the customer has projects with unbilled costs (projects.md).
pub(crate) fn invoice_bar(app: &mut App, ui: &mut Ui, e: &super::sales::Editor) {
    if e.kind != "invoice" || e.customer.trim().is_empty() || !app.module_on(MODULE) {
        return;
    }
    let u = cached(
        app,
        "project.unbilled",
        json!({"customer": e.customer.trim()}),
    );
    let on_editor: Vec<&String> = e
        .lines
        .iter()
        .flat_map(|l| l.billable_items.iter())
        .collect();
    for p in u.list("projects") {
        let items: Vec<Value> = p
            .list("items")
            .iter()
            .filter(|i| !on_editor.iter().any(|k| *k == i.str("key")))
            .cloned()
            .collect();
        if items.is_empty() {
            continue;
        }
        let total = items.iter().fold(mk_core::Decimal::ZERO, |a, i| {
            a.checked_add(i.str("amount").parse().unwrap_or_default())
                .unwrap_or(a)
        });
        let mut add = false;
        ui.scope(format!("unbilled-{}", p.str("project")), |ui| {
            ui.banner(Tone::Accent, |ui| {
                ui.row(|ui| {
                    ui.label(format!(
                        "{} unbilled cost{} for {} ({})",
                        items.len(),
                        if items.len() == 1 { "" } else { "s" },
                        p.str("name"),
                        fmt::group(&super::sales::fmt_money(total))
                    ));
                    ui.named(format!("Add unbilled costs for {}", p.str("name")));
                    add = ui.button("Add…").clicked();
                });
            });
        });
        if add {
            app.projects.add = Some(AddCosts {
                name: p.str("name").to_owned(),
                chosen: items.iter().map(|i| i.str("key").to_owned()).collect(),
                items,
                summary: false,
                error: None,
            });
        }
    }
}

/// The Add unbilled costs dialog (drawn with the sales dialogs).
pub(crate) fn add_costs_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.projects.add.take() else {
        return;
    };
    let mut close = ui.dialog_closed("add-costs");
    let mut add = false;
    ui.dialog(
        "add-costs",
        &format!("Add unbilled costs for {}", f.name),
        900,
        |ui| {
            let cols = vec![
                Column {
                    width: 40,
                    ..col("")
                },
                Column {
                    width: 100,
                    ..col("Date")
                },
                Column {
                    weight: 3,
                    ..col("Payee")
                },
                Column {
                    weight: 2,
                    ..col("Account")
                },
                Column {
                    weight: 2,
                    ..col("Memo")
                },
                Column {
                    width: 100,
                    ..col_num("Amount")
                },
                Column {
                    width: 120,
                    ..col_num("With markup")
                },
            ];
            ui.table("", cols, |ui| {
                for i in &f.items {
                    let key = i.str("key").to_owned();
                    ui.table_row(&format!("cost:{key}"), false, false, |ui| {
                        let mut on = f.chosen.contains(&key);
                        ui.named(format!("Add {} {}", i.str("payee"), fmt::date(i.str("date"))));
                        if ui.checkbox(&mut on, "").clicked() {
                            if on {
                                f.chosen.push(key.clone());
                            } else {
                                f.chosen.retain(|x| *x != key);
                            }
                        }
                        ui.label(fmt::date(i.str("date")));
                        super::sales::cell(ui, i.str("payee"));
                        ui.label(i.str("account_name"));
                        super::sales::cell(ui, i.str("memo"));
                        money(ui, i.str("amount"));
                        money(ui, i.str("with_markup"));
                    });
                }
            });
            let total = f
                .items
                .iter()
                .filter(|i| f.chosen.iter().any(|k| k == i.str("key")))
                .fold(mk_core::Decimal::ZERO, |a, i| {
                    a.checked_add(i.str("with_markup").parse().unwrap_or_default())
                        .unwrap_or(a)
                });
            ui.form("Lines", |ui| {
                ui.radio(&mut f.summary, false, "One line per item");
                ui.radio(&mut f.summary, true, "One summary line per expense account");
            });
            ui.caption("Lines post to Billable Expense Income (Settings → Projects) with the project's markup; the costs show as billed while the lines are on the invoice.");
            if let Some(e) = &f.error {
                ui.error_text(e.clone());
            }
            ui.actions(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                ui.enabled(!f.chosen.is_empty());
                add = ui
                    .primary(format!(
                        "Add {} ({})",
                        f.chosen.len(),
                        fmt::group(&super::sales::fmt_money(total))
                    ))
                    .clicked();
            });
        },
    );
    if close {
        return;
    }
    if add {
        let invoice = app.sales.editor.as_ref().and_then(|e| e.id.clone());
        match app.run(
            "project.billable_lines",
            json!({"items": f.chosen, "summary": f.summary, "invoice": invoice}),
        ) {
            Ok(v) => {
                let projects = app.lists().projects.clone();
                if let Some(e) = app.sales.editor.as_mut() {
                    // Replace a single empty starting line.
                    if e.lines.len() == 1
                        && e.lines.first().is_some_and(|l| {
                            l.what.is_empty() && l.amount.is_empty() && l.description.is_empty()
                        })
                    {
                        e.lines.clear();
                    }
                    for l in v.list("lines") {
                        let project = projects
                            .iter()
                            .find(|p| p.str("id") == l.str("project"))
                            .map(|p| p.str("name").to_owned())
                            .unwrap_or_default();
                        e.lines.push(super::sales::LineDraft {
                            what: v.str("account_name").to_owned(),
                            description: l.str("description").to_owned(),
                            amount: l.str("amount").to_owned(),
                            tags: super::tags::Tags {
                                project,
                                ..super::tags::Tags::default()
                            },
                            billable_items: l
                                .list("billable_items")
                                .iter()
                                .filter_map(|k| k.as_str().map(str::to_owned))
                                .collect(),
                            ..super::sales::LineDraft::default()
                        });
                    }
                }
                app.notify(format!(
                    "Added {} line{}; save the invoice to bill them.",
                    v.list("lines").len(),
                    if v.list("lines").len() == 1 { "" } else { "s" }
                ));
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.projects.add = Some(f);
}

// ----- Settings → Projects and the dashboard block ---------------------------------------------

pub(crate) fn settings_section(app: &mut App, ui: &mut Ui) {
    ui.card("Projects", |ui| {
        super::settings::module_settings(app, ui, MODULE);
    });
    ui.caption("Projects gather income and expenses across accounts by tagging lines; completed and cancelled projects leave the pickers and the phone but stay in reports.");
}

/// The Projects block: active projects with a budget bar each.
pub(crate) fn block(app: &mut App, ui: &mut Ui, data: &Value) {
    let rows = data.list("projects");
    if rows.is_empty() {
        ui.secondary("No active projects.");
        return;
    }
    let mut open_id = None;
    for (i, r) in rows.iter().enumerate() {
        ui.scope(format!("proj-{i}"), |ui| {
            ui.row(|ui| {
                ui.named(format!("Open {}", r.str("name")));
                ui.tip(if r.at("budget").is_null() {
                    format!("{} spent", fmt::group(r.str("spent")))
                } else {
                    format!(
                        "{} of {} spent",
                        fmt::group(r.str("spent")),
                        fmt::group(r.str("budget"))
                    )
                });
                if ui.link(r.str("name")).clicked() {
                    open_id = Some(r.str("id").to_owned());
                }
                ui.spacer();
                if r.at("budget").is_null() {
                    ui.secondary(format!("{} spent", fmt::group(r.str("spent"))));
                } else {
                    used_bar_w(
                        ui,
                        &format!("{} budget used", r.str("name")),
                        r.str("percent"),
                        r.str("level_of_use"),
                        90,
                    );
                }
            });
        });
    }
    if let Some(id) = open_id {
        open(app, &json!({"project": id}));
    }
}
