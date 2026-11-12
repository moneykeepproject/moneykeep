//! The Expenses screen (app-desktop.md revision 28, spec expenses.md): the Expenses
//! tab (list, filters, + New expense, Add receipts…, Expense report…, Record
//! reimbursement…) and the Inbox tab (receipts not attached to anything), the
//! expense pop-up with its receipts strip and match choice, and the report and
//! reimbursement dialogs. Everything goes through `expense.*` and `receipt.*`.

use std::collections::BTreeSet;

use base64::Engine;
use serde_json::{Value, json};

use super::form::{choice, date_row, pick, set};
use super::lists::accounts_where;
use super::receipts::{self, StripAction};
use super::sales::{cell, money};
use super::tags::{self, Tags, TagsOn};
use super::ui::{Column, Tone, Ui, col, col_num};
use super::util::{Json, array};
use super::{App, AppEvent, MessageAction};
use crate::{ReceiptTarget, fmt};

/// The module this screen belongs to.
pub(crate) const MODULE: &str = "expenses";
const PERSONAL_LABEL: &str = "Personal funds (reimbursable)";
/// Marks a trip id among the expense ids of a saved report.
const TRIP_PREFIX: &str = "trip:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Tab {
    #[default]
    Expenses,
    Inbox,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Form {
    pub id: Option<String>,
    pub date: String,
    pub merchant: String,
    pub amount: String,
    pub category: String,
    pub paid_with: String,
    pub purpose: String,
    pub tags: Tags,
    pub tags_on: TagsOn,
    pub pending: Vec<String>,
    pub status: String,
    pub candidates: Option<Vec<Value>>,
    pub error: Option<String>,
    pub confirm_delete: bool,
    pub added: usize,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ReportDialog {
    pub from: String,
    pub to: String,
    pub reimbursable: bool,
    pub paid_with: String,
    pub category: String,
    pub include_receipts: bool,
    pub mark_reported: bool,
    pub title: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Reimburse {
    pub expenses: Vec<Value>,
    /// Posted reimbursable trips not reimbursed yet, and whether each is included.
    pub trips: Vec<(Value, bool)>,
    pub from_account: String,
    pub date: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Attach {
    pub receipt: String,
    pub file_name: String,
    pub amount: String,
    pub date: String,
    pub payee: String,
    pub results: Option<Vec<Value>>,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct ExpensesState {
    pub tab: Tab,
    pub from: String,
    pub to: String,
    pub status: usize,
    pub paid_with: String,
    pub text: String,
    pub selected: BTreeSet<String>,
    pub form: Option<Form>,
    pub report: Option<ReportDialog>,
    pub reimburse: Option<Reimburse>,
    pub attach: Option<Attach>,
    cache: Option<(u64, String, Vec<Value>)>,
    inbox: Option<(u64, Vec<Value>)>,
}

const STATUSES: [&str; 6] = ["All", "Draft", "Ready", "Posted", "Matched", "Reported"];

fn status_word(s: &str) -> &'static str {
    match s {
        "draft" => "Draft",
        "ready" => "Ready",
        "posted" => "Posted",
        "matched" => "Matched",
        _ => "",
    }
}

fn rows(app: &mut App) -> Vec<Value> {
    let st = &app.expenses;
    let mut args = json!({});
    if let Ok(Some(d)) = fmt::read_date(&st.from) {
        set(&mut args, "from", json!(d.to_string()));
    }
    if let Ok(Some(d)) = fmt::read_date(&st.to) {
        set(&mut args, "to", json!(d.to_string()));
    }
    if st.status > 0 {
        let s = STATUSES.get(st.status).copied().unwrap_or("All");
        set(&mut args, "status", json!(s.to_lowercase()));
    }
    if !st.paid_with.is_empty() {
        set(&mut args, "paid_with", json!(paid_with_arg(&st.paid_with)));
    }
    if !st.text.trim().is_empty() {
        set(&mut args, "text", json!(st.text.trim()));
    }
    let key = args.to_string();
    if let Some((g, k, v)) = &app.expenses.cache
        && *g == app.generation
        && *k == key
    {
        return v.clone();
    }
    let list = array(app.read("expense.list", args));
    app.expenses.cache = Some((app.generation, key, list.clone()));
    list
}

/// Receipts in the inbox (cached per generation).
pub(crate) fn inbox(app: &mut App) -> Vec<Value> {
    if let Some((g, v)) = &app.expenses.inbox
        && *g == app.generation
    {
        return v.clone();
    }
    let list = array(app.backend.invoke("receipt.inbox", json!({})).ok());
    app.expenses.inbox = Some((app.generation, list.clone()));
    list
}

fn paid_with_arg(shown: &str) -> String {
    if shown == PERSONAL_LABEL {
        "personal".into()
    } else {
        shown.to_owned()
    }
}

fn paid_with_options(app: &mut App) -> Vec<String> {
    let l = app.lists().clone();
    let mut v = accounts_where(&l, |a| {
        matches!(a.str("subtype"), "bank" | "cash" | "credit-card")
    });
    v.push(PERSONAL_LABEL.into());
    v
}

fn category_options(app: &mut App) -> Vec<String> {
    let l = app.lists().clone();
    accounts_where(&l, |a| a.str("type") == "expense")
}

/// Draw the screen.
pub(crate) fn view(app: &mut App, ui: &mut Ui) {
    if std::mem::take(&mut app.expenses_tab_inbox) {
        app.expenses.tab = Tab::Inbox;
    }
    if std::mem::take(&mut app.expenses_new) {
        open_new(app, Vec::new());
    }
    ui.title("Expenses");
    let inbox_n = inbox(app).len();
    ui.toolbar(|ui| {
        for (tab, label, name) in [
            (Tab::Expenses, "Expenses".to_owned(), "Expenses"),
            (Tab::Inbox, format!("Inbox ({inbox_n})"), "Inbox"),
        ] {
            ui.named(name);
            if ui.tab(label, app.expenses.tab == tab).clicked() {
                app.expenses.tab = tab;
            }
        }
    });
    match app.expenses.tab {
        Tab::Expenses => ui.scope("expenses", |ui| expenses_tab(app, ui)),
        Tab::Inbox => ui.scope("inbox", |ui| inbox_tab(app, ui)),
    }
    form_dialog(app, ui);
    report_dialog(app, ui);
    reimburse_dialog(app, ui);
    attach_dialog(app, ui);
}

fn expenses_tab(app: &mut App, ui: &mut Ui) {
    let list = rows(app);
    let reimbursable: Vec<Value> = list
        .iter()
        .filter(|e| app.expenses.selected.contains(e.str("id")))
        .filter(|e| {
            e.at("reimbursable").as_bool() == Some(true)
                && e.str("status") == "posted"
                && e.at("reimbursement").is_null()
        })
        .cloned()
        .collect();
    let trips = super::mileage::open_reimbursable(app);
    ui.wrap(|ui| {
        if ui.primary("+ New expense").clicked() {
            open_new(app, Vec::new());
        }
        if ui.button("Add receipts…").clicked() {
            app.emit(AppEvent::ChooseReceiptFiles {
                target: ReceiptTarget::Inbox,
            });
        }
        if ui.button("Expense report…").clicked() {
            open_report(app);
        }
        ui.enabled(!reimbursable.is_empty() || !trips.is_empty());
        if reimbursable.is_empty() && trips.is_empty() {
            ui.tip("Select posted expenses paid with personal funds that are not reimbursed yet.");
        }
        if ui.button("Record reimbursement…").clicked() {
            app.expenses.reimburse = Some(Reimburse {
                expenses: reimbursable.clone(),
                trips: trips.iter().map(|t| (t.clone(), true)).collect(),
                date: fmt::date(&app.today()),
                ..Reimburse::default()
            });
        }
    });
    let paid_options = paid_with_options(app);
    ui.wrap(|ui| {
        ui.label("From");
        super::register::date_field(ui, "Expenses from", &mut app.expenses.from);
        ui.label("To");
        super::register::date_field(ui, "Expenses to", &mut app.expenses.to);
        choice(ui, "Status filter", &mut app.expenses.status, &STATUSES);
        pick(
            ui,
            "Paid with filter",
            &mut app.expenses.paid_with,
            &paid_options,
            Some("Any paid with"),
        );
        ui.width(180).hint("Search");
        ui.field("Search expenses", &mut app.expenses.text);
    });
    if list.is_empty() {
        ui.card("", |ui| {
            ui.label("No expenses yet. Add one with + New expense, or drop receipt pictures here.")
        });
        return;
    }
    let cols = vec![
        Column {
            width: 40,
            ..col("")
        },
        Column {
            width: 110,
            ..col("Date")
        },
        Column {
            weight: 3,
            ..col("Merchant")
        },
        Column {
            weight: 2,
            ..col("Category")
        },
        Column {
            weight: 2,
            ..col("Paid with")
        },
        Column {
            width: 110,
            ..col_num("Amount")
        },
        Column {
            width: 80,
            ..col("Receipt")
        },
        Column {
            width: 120,
            ..col("Status")
        },
        Column {
            width: 110,
            ..col("")
        },
    ];
    let mut open: Option<Value> = None;
    ui.table("", cols, |ui| {
        for e in &list {
            let id = e.str("id").to_owned();
            let merchant = if e.str("merchant").is_empty() {
                "(no merchant)".to_owned()
            } else {
                e.str("merchant").to_owned()
            };
            ui.table_row(&format!("expense:{id}"), false, false, |ui| {
                let mut on = app.expenses.selected.contains(&id);
                ui.named(format!("Select {merchant} {}", e.str("date")));
                if ui.checkbox(&mut on, "").clicked() {
                    if on {
                        app.expenses.selected.insert(id.clone());
                    } else {
                        app.expenses.selected.remove(&id);
                    }
                }
                ui.label(fmt::date(e.str("date")));
                ui.tip("Open this expense");
                if ui.link(merchant.clone()).clicked() {
                    open = Some(e.clone());
                }
                cell(ui, e.str("category_name"));
                cell(ui, e.str("paid_with_name"));
                money(ui, e.str("amount"));
                if e.at("receipt_count").as_u64().unwrap_or(0) > 0 {
                    ui.named("Has receipts").tip("Has receipts");
                    ui.icon_image("paperclip");
                } else {
                    ui.secondary("Missing");
                }
                let mut status = status_word(e.str("status")).to_owned();
                if e.at("reported").as_bool() == Some(true) {
                    status.push_str(", reported");
                }
                ui.label(status);
                if e.str("source") == "phone" {
                    ui.badge("From phone", Tone::Accent);
                } else {
                    ui.label("");
                }
            });
        }
    });
    if let Some(e) = open {
        open_edit(app, &e);
    }
}

fn inbox_tab(app: &mut App, ui: &mut Ui) {
    ui.row(|ui| {
        if ui.button("Add receipts…").clicked() {
            app.emit(AppEvent::ChooseReceiptFiles {
                target: ReceiptTarget::Inbox,
            });
        }
    });
    ui.secondary(
        "Receipts you add or drop here wait in the inbox until they become an expense or are attached to a transaction.",
    );
    let list = inbox(app);
    if list.is_empty() {
        ui.card("", |ui| ui.label("The inbox is empty."));
        return;
    }
    let mut act: Option<(&str, Value)> = None;
    for r in &list {
        let name = r.str("file_name").to_owned();
        ui.card("", |ui| {
            ui.row(|ui| {
                if receipts::tile(app, ui, r, &name) {
                    act = Some(("view", r.clone()));
                }
                ui.bold();
                ui.label(name.clone());
            });
            let added = r.str("added_at").get(..10).unwrap_or_default().to_owned();
            let source = match r.str("source") {
                "phone" => "from your phone",
                "import" => "imported",
                _ => "added on this computer",
            };
            ui.label(format!("Added {}, {source}", fmt::date(&added)));
            ui.row(|ui| {
                ui.named(format!("New expense from {name}"));
                if ui.button("New expense").clicked() {
                    act = Some(("new", r.clone()));
                }
                ui.named(format!("Attach {name} to a transaction"));
                if ui.button("Attach to transaction…").clicked() {
                    act = Some(("attach", r.clone()));
                }
                ui.named(format!("Delete {name}"));
                if ui.button("Delete").clicked() {
                    act = Some(("delete", r.clone()));
                }
            });
        });
    }
    match act {
        Some(("view", r)) => receipts::view(app, &r),
        Some(("new", r)) => open_new(app, vec![r.str("id").to_owned()]),
        Some(("attach", r)) => {
            app.expenses.attach = Some(Attach {
                receipt: r.str("id").to_owned(),
                file_name: r.str("file_name").to_owned(),
                date: fmt::date(r.str("added_at").get(..10).unwrap_or_default()),
                ..Attach::default()
            });
        }
        Some(("delete", r)) => match app.run("receipt.delete", json!({"receipt": r.str("id")})) {
            Ok(_) => {
                receipts::forget(app, r.str("id"));
                app.notify_with("Deleted the receipt.", MessageAction::Undo);
            }
            Err(e) => app.notify_error(&e),
        },
        _ => {}
    }
}

// ----- the expense pop-up ---------------------------------------------------------

fn new_form(app: &mut App, date: &str, pending: Vec<String>, added: usize) -> Form {
    let tags_on = tags::options(app);
    let paid_with = app
        .backend
        .invoke("module.settings", json!({"module": MODULE}))
        .ok()
        .and_then(|s| {
            s.list("settings")
                .iter()
                .find(|x| x.str("key") == "expenses.default_paid_with")
                .map(|x| x.str("shown").to_owned())
        })
        .unwrap_or_default();
    Form {
        date: fmt::date(date),
        paid_with,
        pending,
        tags_on,
        added,
        ..Form::default()
    }
}

/// Open "New expense", with inbox receipts already in it.
pub(crate) fn open_new(app: &mut App, pending: Vec<String>) {
    let today = app.today();
    app.expenses.form = Some(new_form(app, &today, pending, 0));
}

fn open_edit(app: &mut App, e: &Value) {
    let tags_on = tags::options(app);
    app.expenses.form = Some(Form {
        id: Some(e.str("id").to_owned()),
        date: fmt::date(e.str("date")),
        merchant: e.str("merchant").to_owned(),
        amount: e.str("amount").to_owned(),
        category: e.str("category_name").to_owned(),
        paid_with: e.str("paid_with_name").to_owned(),
        purpose: e.str("purpose").to_owned(),
        tags: Tags::from_names(e.at("tag_names")),
        tags_on,
        status: e.str("status").to_owned(),
        notes: e
            .list("notes")
            .iter()
            .filter_map(|n| n.as_str().map(str::to_owned))
            .collect(),
        ..Form::default()
    });
}

fn form_receipts(app: &mut App, f: &Form) -> Vec<Value> {
    match &f.id {
        Some(id) => array(
            app.backend
                .invoke("receipt.list", json!({"expense": id}))
                .ok(),
        ),
        None => inbox(app)
            .into_iter()
            .filter(|r| f.pending.iter().any(|p| p == r.str("id")))
            .collect(),
    }
}

#[derive(Default)]
struct FormActs {
    save: bool,
    post: bool,
    cancel: bool,
    delete: Option<Option<bool>>,
    add_receipt: bool,
    strip: Option<StripAction>,
    matched: Option<String>,
    create_new: bool,
    toggle_fast: bool,
}

fn form_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.expenses.form.take() else {
        return;
    };
    let list = form_receipts(app, &f);
    let categories = category_options(app);
    let paid_options = paid_with_options(app);
    let fast = app.config.fast_entry;
    let posted = matches!(f.status.as_str(), "posted" | "matched");
    let mut act = FormActs {
        cancel: ui.dialog_closed("expense-form") && app.receipts.viewer.is_none(),
        ..FormActs::default()
    };
    let title = if f.id.is_some() {
        "Edit expense"
    } else {
        "New expense"
    };
    ui.dialog("expense-form", title, 560, |ui| {
        ui.close_label("Close");
        if f.added > 0 {
            ui.label(if f.added == 1 {
                "1 expense added".to_owned()
            } else {
                format!("{} expenses added", f.added)
            });
        }
        if posted {
            ui.secondary("This expense is in your books: changes also change its transaction.");
        }
        ui.card("Receipts", |ui| {
            if list.is_empty() {
                ui.secondary("No receipt yet. Add a picture or PDF, or drop it on this window.");
            }
            act.strip = receipts::strip(app, ui, &list, true);
            ui.row(|ui| {
                if ui.button("Add receipt…").clicked() {
                    act.add_receipt = true;
                }
            });
        });
        ui.group(|ui| {
            date_row(ui, "Expense date", &mut f.date);
            ui.form("Merchant", |ui| {
                ui.width(300);
                ui.field("Merchant", &mut f.merchant);
            });
            ui.form("Amount", |ui| {
                ui.width(140).mono();
                ui.field("Amount", &mut f.amount);
            });
            ui.form("Category", |ui| {
                pick(ui, "Category", &mut f.category, &categories, Some("None"));
            });
            ui.form("Paid with", |ui| {
                pick(
                    ui,
                    "Paid with",
                    &mut f.paid_with,
                    &paid_options,
                    Some("None"),
                );
            });
            ui.form("Purpose", |ui| {
                ui.width(300);
                ui.multiline("Purpose", &mut f.purpose);
            });
            let on = f.tags_on.clone();
            tags::rows(ui, &on, &mut f.tags, "Expense");
        });
        for n in &f.notes {
            ui.secondary(n.clone());
        }
        if let Some(c) = &f.candidates {
            ui.card("This expense may already be in your books", |ui| {
                ui.label("Match it to the transaction below, or create a new one.");
                for t in c {
                    ui.row(|ui| {
                        ui.label(fmt::date(t.str("date")));
                        ui.label(t.str("payee"));
                        ui.amount(fmt::group(t.str("amount")));
                        if !t.str("category").is_empty() {
                            ui.secondary(t.str("category"));
                        }
                        ui.named(if c.len() == 1 {
                            "Match".to_owned()
                        } else {
                            format!("Match {} {}", t.str("payee"), t.str("date"))
                        });
                        if ui.button("Match").clicked() {
                            act.matched = Some(t.str("id").to_owned());
                        }
                    });
                }
                ui.row(|ui| {
                    if ui.button("Create new").clicked() {
                        act.create_new = true;
                    }
                });
            });
        }
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        if f.confirm_delete {
            ui.banner(Tone::Warning, |ui| {
                if f.status == "posted" {
                    ui.label(
                        "Delete this expense? Its transaction can go too, or stay in your books.",
                    );
                    ui.row(|ui| {
                        if ui.button("Delete expense and transaction").clicked() {
                            act.delete = Some(Some(true));
                        }
                        if ui.button("Delete expense, keep transaction").clicked() {
                            act.delete = Some(Some(false));
                        }
                        if ui.button("Keep it").clicked() {
                            f.confirm_delete = false;
                        }
                    });
                } else {
                    ui.label("Delete this expense? You can undo it afterwards.");
                    ui.row(|ui| {
                        if ui.button("Delete expense").clicked() {
                            act.delete = Some(None);
                        }
                        if ui.button("Keep it").clicked() {
                            f.confirm_delete = false;
                        }
                    });
                }
            });
        }
        ui.row(|ui| {
            if f.id.is_some() && ui.button("Delete…").clicked() {
                f.confirm_delete = true;
            }
            ui.spacer();
            ui.tip("Fast entry: after saving, a new blank expense opens.");
            if ui
                .icon_toggle(
                    if fast { "foot-filled" } else { "foot" },
                    "Fast entry",
                    fast,
                )
                .clicked()
            {
                act.toggle_fast = true;
            }
            if ui.primary("Save").clicked() {
                act.save = true;
            }
            if !posted {
                ui.tip("Save, then put it in your books (matching a transaction already there)");
                if ui.button("Save and post").clicked() {
                    act.save = true;
                    act.post = true;
                }
            }
            if ui.button("Cancel").clicked() {
                act.cancel = true;
            }
        });
    });
    finish_form(app, f, act);
}

fn form_args(f: &Form) -> Result<Value, String> {
    let date = fmt::date_arg(&f.date)?;
    let amount = match fmt::parse_entry(&f.amount)? {
        Some(a) => json!(a.abs().to_string()),
        None => json!(null),
    };
    let mut args = json!({
        "date": date,
        "merchant": f.merchant.trim(),
        "category": f.category,
        "paid_with": paid_with_arg(&f.paid_with),
        "purpose": f.purpose.trim(),
    });
    if !amount.is_null() {
        set(&mut args, "amount", amount);
    }
    f.tags.apply_given(&mut args, &f.tags_on);
    Ok(args)
}

fn save(app: &mut App, f: &mut Form) -> Result<Value, String> {
    let mut args = form_args(f)?;
    let out = match &f.id {
        Some(id) => {
            set(&mut args, "expense", json!(id));
            app.run("expense.update", args)
        }
        None => {
            if let Some(o) = args.as_object_mut() {
                o.retain(|_, v| v.as_str() != Some(""));
                o.insert("receipts".into(), json!(f.pending));
            }
            app.run("expense.create", args)
        }
    }
    .map_err(|e| e.display())?;
    f.id = Some(out.str("id").to_owned());
    f.pending.clear();
    f.status = out.str("status").to_owned();
    Ok(out)
}

fn post(app: &mut App, f: &mut Form, choice: Option<&str>) -> Result<bool, String> {
    let mut args = json!({"expense": f.id});
    if let Some(c) = choice {
        set(&mut args, "match", json!(c));
    }
    let out = app.run("expense.post", args).map_err(|e| e.display())?;
    if out.str("status") == "needs_choice" {
        f.candidates = Some(out.list("candidates").to_vec());
        return Ok(false);
    }
    f.candidates = None;
    f.status = out.str("status").to_owned();
    let what = if f.status == "matched" {
        "Matched the expense to the transaction in your books."
    } else {
        "Posted the expense."
    };
    app.notify_with(what, MessageAction::Undo);
    Ok(true)
}

fn finish_form(app: &mut App, mut f: Form, act: FormActs) {
    if act.toggle_fast {
        app.config.fast_entry = !app.config.fast_entry;
        app.emit(AppEvent::ConfigChanged);
    }
    if act.add_receipt {
        app.expenses.form = Some(f);
        app.emit(AppEvent::ChooseReceiptFiles {
            target: ReceiptTarget::ExpenseForm,
        });
        return;
    }
    match act.strip {
        Some(StripAction::View(r)) => receipts::view(app, &r),
        Some(StripAction::Remove(id)) => {
            let cmd = if f.pending.contains(&id) {
                "receipt.delete"
            } else {
                "receipt.unlink"
            };
            match app.run(cmd, json!({"receipt": id})) {
                Ok(_) => {
                    f.pending.retain(|p| *p != id);
                    receipts::forget(app, &id);
                }
                Err(e) => f.error = Some(e.display()),
            }
        }
        None => {}
    }
    if let Some(choice) = act.delete {
        let mut args = json!({"expense": f.id});
        if let Some(t) = choice {
            set(&mut args, "delete_transaction", json!(t));
        }
        match app.run("expense.delete", args) {
            Ok(_) => {
                app.notify_with("Deleted the expense.", MessageAction::Undo);
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    let mut close = act.cancel;
    if let Some(t) = &act.matched {
        match post(app, &mut f, Some(t)) {
            Ok(true) => close = true,
            Ok(false) => {}
            Err(e) => f.error = Some(e),
        }
    } else if act.create_new {
        match post(app, &mut f, Some("new")) {
            Ok(true) => close = true,
            Ok(false) => {}
            Err(e) => f.error = Some(e),
        }
    } else if act.save {
        f.error = None;
        match save(app, &mut f) {
            Ok(_) if act.post => match post(app, &mut f, None) {
                Ok(true) => close = true,
                Ok(false) => {}
                Err(e) => f.error = Some(e),
            },
            Ok(_) => {
                app.notify_with("Saved the expense.", MessageAction::Undo);
                close = true;
            }
            Err(e) => f.error = Some(e),
        }
        if close && app.config.fast_entry {
            let iso = fmt::date_arg(&f.date).unwrap_or_else(|_| app.today());
            let added = f.added + 1;
            app.expenses.form = Some(new_form(app, &iso, Vec::new(), added));
            return;
        }
    }
    if !close {
        app.expenses.form = Some(f);
    }
}

// ----- report and reimbursement ---------------------------------------------------

fn open_report(app: &mut App) {
    let today = app.today();
    let first = format!("{}-01", today.get(..7).unwrap_or("2026-01"));
    let last = first
        .parse::<mk_core::Date>()
        .ok()
        .map(|d| {
            let days = mk_core::Date::days_in_month(d.year(), d.month());
            format!("{}-{days:02}", today.get(..7).unwrap_or("2026-01"))
        })
        .unwrap_or_else(|| today.clone());
    app.expenses.report = Some(ReportDialog {
        from: fmt::date(&first),
        to: fmt::date(&last),
        include_receipts: true,
        title: "Expense report".into(),
        ..ReportDialog::default()
    });
}

fn report_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.expenses.report.take() else {
        return;
    };
    let categories = category_options(app);
    let paid_options = paid_with_options(app);
    let (mut save, mut cancel) = (false, ui.dialog_closed("expense-report"));
    ui.dialog("expense-report", "Expense report", 500, |ui| {
        ui.group(|ui| {
            date_row(ui, "Report from", &mut d.from);
            date_row(ui, "Report to", &mut d.to);
            ui.form("Include", |ui| {
                ui.radio(&mut d.reimbursable, false, "All expenses");
                ui.radio(&mut d.reimbursable, true, "Reimbursable only");
            });
            ui.form("Paid with", |ui| {
                pick(
                    ui,
                    "Report paid with",
                    &mut d.paid_with,
                    &paid_options,
                    Some("Any"),
                );
            });
            ui.form("Category", |ui| {
                pick(
                    ui,
                    "Report category",
                    &mut d.category,
                    &categories,
                    Some("Any"),
                );
            });
            ui.form("Title", |ui| {
                ui.width(280);
                ui.field("Report title", &mut d.title);
            });
            ui.form("", |ui| {
                ui.checkbox(&mut d.include_receipts, "Include receipt pictures");
            });
            ui.form("", |ui| {
                ui.checkbox(&mut d.mark_reported, "Mark these expenses as reported");
            });
        });
        if let Some(e) = &d.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save PDF…").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if !save {
        app.expenses.report = Some(d);
        return;
    }
    let (from, to) = match (fmt::date_arg(&d.from), fmt::date_arg(&d.to)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            d.error = Some(e);
            app.expenses.report = Some(d);
            return;
        }
    };
    let mut args = json!({
        "from": from, "to": to,
        "filter": if d.reimbursable { "reimbursable" } else { "all" },
        "include_receipts": d.include_receipts,
        "title": d.title.trim(),
    });
    if !d.paid_with.is_empty() {
        set(&mut args, "paid_with", json!(paid_with_arg(&d.paid_with)));
    }
    if !d.category.is_empty() {
        set(&mut args, "category", json!(d.category));
    }
    match app.run("expense.report", args) {
        Ok(v) => {
            match base64::engine::general_purpose::STANDARD.decode(v.str("content_base64")) {
                Ok(data) => {
                    for w in v.list("warnings") {
                        if let Some(t) = w.as_str() {
                            app.notify(t.to_owned());
                        }
                    }
                    let file_name = v.str("file_name").to_owned();
                    if d.mark_reported {
                        let expenses =
                            v.list("expense_ids")
                                .iter()
                                .filter_map(|x| x.as_str().map(str::to_owned))
                                // Trips ride along marked, so they are flagged too.
                                .chain(v.list("trip_ids").iter().filter_map(|x| {
                                    x.as_str().map(|t| format!("{TRIP_PREFIX}{t}"))
                                }))
                                .collect();
                        app.emit(AppEvent::SaveExpenseReport {
                            file_name,
                            data,
                            expenses,
                        });
                    } else {
                        app.emit(AppEvent::SaveBytes { file_name, data });
                    }
                }
                Err(_) => {
                    d.error = Some("The report could not be made.".into());
                    app.expenses.report = Some(d);
                }
            }
        }
        Err(e) => {
            d.error = Some(e.display());
            app.expenses.report = Some(d);
        }
    }
}

/// After the report PDF was saved: flag its expenses reported.
pub(crate) fn report_saved(app: &mut App, expenses: &[String]) {
    if expenses.is_empty() {
        return;
    }
    let (trips, expenses): (Vec<&String>, Vec<&String>) =
        expenses.iter().partition(|e| e.starts_with(TRIP_PREFIX));
    let trips: Vec<&str> = trips
        .iter()
        .filter_map(|t| t.strip_prefix(TRIP_PREFIX))
        .collect();
    match app.run(
        "expense.mark_reported",
        json!({ "expenses": expenses, "trips": trips }),
    ) {
        Ok(v) => {
            let n = v
                .get("marked_reported")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if n > 0 {
                app.notify(format!(
                    "Marked {n} expense{} as reported.",
                    if n == 1 { "" } else { "s" }
                ));
            }
        }
        Err(e) => app.notify_error(&e),
    }
}

fn reimburse_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.expenses.reimburse.take() else {
        return;
    };
    let l = app.lists().clone();
    let banks = accounts_where(&l, |a| matches!(a.str("subtype"), "bank" | "cash"));
    let total: mk_core::Decimal =
        d.expenses
            .iter()
            .filter_map(|e| e.str("amount").parse::<mk_core::Decimal>().ok())
            .chain(
                d.trips.iter().filter(|(_, on)| *on).filter_map(|(t, _)| {
                    t.str("reimburse_amount").parse::<mk_core::Decimal>().ok()
                }),
            )
            .sum();
    let (mut ok, mut cancel) = (false, ui.dialog_closed("expense-reimburse"));
    ui.dialog("expense-reimburse", "Record reimbursement", 480, |ui| {
        ui.label("Paying yourself back for these expenses records one transfer for their total:");
        for e in &d.expenses {
            ui.label(format!(
                "{}  {}  {}",
                fmt::date(e.str("date")),
                e.str("merchant"),
                fmt::group(e.str("amount"))
            ));
        }
        if !d.trips.is_empty() {
            ui.label("Mileage trips (posted, reimbursable):");
            for (t, on) in &mut d.trips {
                ui.checkbox(on, &super::mileage::reimburse_line(t));
            }
        }
        ui.bold();
        ui.label(format!("Total {}", fmt::group(&total.to_string())));
        ui.group(|ui| {
            ui.form("From account", |ui| {
                pick(ui, "Reimburse from", &mut d.from_account, &banks, None);
            });
            date_row(ui, "Reimbursement date", &mut d.date);
        });
        if let Some(e) = &d.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Record").clicked() {
                ok = true;
            }
        });
    });
    if cancel {
        return;
    }
    if !ok {
        app.expenses.reimburse = Some(d);
        return;
    }
    let date = match fmt::date_arg(&d.date) {
        Ok(x) => x,
        Err(e) => {
            d.error = Some(e);
            app.expenses.reimburse = Some(d);
            return;
        }
    };
    let ids: Vec<&str> = d.expenses.iter().map(|e| e.str("id")).collect();
    let trip_ids: Vec<&str> = d
        .trips
        .iter()
        .filter(|(_, on)| *on)
        .map(|(t, _)| t.str("id"))
        .collect();
    let mut args = json!({"expenses": ids, "from_account": d.from_account, "date": date});
    if !trip_ids.is_empty() {
        set(&mut args, "trips", json!(trip_ids));
    }
    match app.run("expense.record_reimbursement", args) {
        Ok(v) => {
            app.expenses.selected.clear();
            app.notify_with(
                format!(
                    "Recorded a reimbursement of {}.",
                    fmt::group(v.str("total"))
                ),
                MessageAction::Undo,
            );
        }
        Err(e) => {
            d.error = Some(e.display());
            app.expenses.reimburse = Some(d);
        }
    }
}

fn attach_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.expenses.attach.take() else {
        return;
    };
    let (mut search, mut cancel, mut chosen) = (false, ui.dialog_closed("receipt-attach"), None);
    ui.dialog(
        "receipt-attach",
        &format!("Attach {} to a transaction", d.file_name),
        560,
        |ui| {
            ui.wrap(|ui| {
                ui.width(100);
                ui.field("Find amount", &mut d.amount);
                super::register::date_field(ui, "Find date", &mut d.date);
                ui.width(160);
                ui.field("Find payee", &mut d.payee);
                if ui.button("Search").clicked() {
                    search = true;
                }
            });
            if let Some(e) = &d.error {
                ui.error_text(e.clone());
            }
            if let Some(list) = &d.results {
                if list.is_empty() {
                    ui.label("No transactions found. Try fewer details.");
                }
                let cols = vec![
                    Column {
                        width: 110,
                        ..col("Date")
                    },
                    Column {
                        weight: 3,
                        ..col("Payee")
                    },
                    Column {
                        width: 110,
                        ..col_num("Amount")
                    },
                    Column {
                        width: 90,
                        ..col("")
                    },
                ];
                ui.table("", cols, |ui| {
                    ui.visible_rows(8);
                    for t in list {
                        ui.table_row(&format!("found:{}", t.str("id")), false, false, |ui| {
                            ui.label(fmt::date(t.str("date")));
                            cell(ui, t.str("payee"));
                            money(ui, t.str("amount"));
                            ui.named(format!("Attach to {} {}", t.str("payee"), t.str("date")));
                            if ui.button("Attach").clicked() {
                                chosen = Some(t.str("id").to_owned());
                            }
                        });
                    }
                });
            }
            ui.actions(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        },
    );
    if cancel {
        return;
    }
    if let Some(t) = chosen {
        match app.run(
            "receipt.link",
            json!({"receipt": d.receipt, "transaction": t}),
        ) {
            Ok(_) => {
                app.notify_with("Attached the receipt.", MessageAction::Undo);
                return;
            }
            Err(e) => d.error = Some(e.display()),
        }
    }
    if search || d.results.is_none() {
        let mut args = json!({});
        if let Ok(Some(a)) = fmt::parse_entry(&d.amount) {
            set(&mut args, "amount", json!(a.abs().to_string()));
        }
        if let Ok(Some(dt)) = fmt::read_date(&d.date) {
            set(&mut args, "date", json!(dt.to_string()));
        }
        if !d.payee.trim().is_empty() {
            set(&mut args, "text", json!(d.payee.trim()));
        }
        d.results = app
            .read("receipt.find_transactions", args)
            .and_then(|v| v.as_array().cloned());
    }
    app.expenses.attach = Some(d);
}
