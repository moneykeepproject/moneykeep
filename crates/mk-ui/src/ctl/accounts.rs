//! Accounts (app-desktop.md revisions 2, 16): the chart of accounts with kinds
//! and balances, New account…, rename, move, archive, delete; the Classes,
//! Locations and Funds tabs.

use serde_json::{Value, json};

use super::App;
use super::form::{date_row, pick_row, put, set, text_row};
use super::lists::names;
use super::ui::{Align, Column, Tone, Ui};
use super::util::{Json, array};
use crate::View;
use crate::base::accounts::{at_scale, group_of, members_where};

pub(crate) const KINDS: [(&str, &str, &str); 13] = [
    ("Bank account", "asset", "bank"),
    ("Cash", "asset", "cash"),
    ("Credit card", "liability", "credit-card"),
    ("Brokerage account", "asset", "brokerage"),
    ("Money owed to me", "asset", "receivable"),
    ("Property or equipment", "asset", "fixed-asset"),
    ("Other asset", "asset", "other-asset"),
    ("Loan", "liability", "loan"),
    ("Other debt", "liability", "other-liability"),
    ("Income category", "income", "income"),
    ("Expense category", "expense", "expense"),
    ("Equity", "equity", "owner-equity"),
    ("Cost of goods sold", "expense", "cost-of-goods"),
];

#[derive(Debug, Default, Clone)]
pub(crate) struct NewAccount {
    pub name: String,
    pub kind: usize,
    pub parent: String,
    pub code: String,
    pub tax_line: String,
    pub opening: String,
    pub opening_date: String,
    pub error: Option<String>,
}

/// The account dialog for an existing account (chart-presets.md: number, tax line,
/// the role line).
#[derive(Debug, Default, Clone)]
pub(crate) struct EditAccount {
    pub id: String,
    pub name: String,
    pub code: String,
    pub description: String,
    pub tax_line: String,
    pub placeholder: bool,
    pub role: String,
    pub role_purpose: String,
    pub error: Option<String>,
    /// Money accounts (ledger-accounts.md revision 3): the chosen and the current
    /// index in `MONEY_KINDS`; `None` for other accounts.
    pub money: Option<(usize, usize)>,
    /// Brokerage accounts: whether it is a retirement account (opening-balances.md).
    pub retirement: Option<bool>,
    /// A group (placeholder or with sub-accounts) keeps its side.
    pub same_side: bool,
    /// The type of any other account, in words.
    pub type_text: String,
    /// The Group chosen when the side changes ("" = Top level) and its choices.
    pub group: String,
    pub groups: Vec<String>,
    /// The chart as the dialog opened (id, full name, type, subtype, parent,
    /// placeholder, archived).
    pub chart: Vec<Value>,
}

/// The account types a money account can switch among (app-desktop.md revision 35):
/// (label, type, subtype).
pub(crate) const MONEY_KINDS: [(&str, &str, &str); 6] = [
    ("Bank", "asset", "bank"),
    ("Cash", "asset", "cash"),
    ("Other asset", "asset", "other-asset"),
    ("Credit card", "liability", "credit-card"),
    ("Loan", "liability", "loan"),
    ("Other liability", "liability", "other-liability"),
];

fn money_index(a: &Value) -> Option<usize> {
    MONEY_KINDS
        .iter()
        .position(|k| k.1 == a.str("type") && k.2 == a.str("subtype"))
}

/// The accounts of `ty` that may hold account `id`, by full name.
fn group_choices(chart: &[Value], id: &str, ty: &str) -> Vec<String> {
    let mut out: Vec<String> = chart
        .iter()
        .filter(|a| {
            a.str("type") == ty && a.str("id") != id && a.at("archived").as_bool() != Some(true)
        })
        .map(|a| a.str("full_name").to_owned())
        .collect();
    out.sort_by_key(|n| n.to_lowercase());
    out
}

/// The group `account.update` chooses when no parent is given: the first placeholder
/// of the new type, by full name, that holds an account of the new subtype; else
/// the top level ("").
fn default_group(chart: &[Value], id: &str, ty: &str, sub: &str) -> String {
    let mut found: Vec<String> = chart
        .iter()
        .filter(|g| {
            g.at("placeholder").as_bool() == Some(true)
                && g.at("archived").as_bool() != Some(true)
                && g.str("id") != id
                && g.str("type") == ty
                && chart.iter().any(|c| {
                    c.str("parent") == g.str("id") && c.str("id") != id && c.str("subtype") == sub
                })
        })
        .map(|g| g.str("full_name").to_owned())
        .collect();
    found.sort_by_key(|n| n.to_lowercase());
    found.into_iter().next().unwrap_or_default()
}

#[derive(Debug, Default)]
pub(crate) struct AccountsState {
    pub new: Option<NewAccount>,
    pub edit: Option<EditAccount>,
    pub moving: Option<(String, String)>,
    pub renaming: Option<(String, String)>,
    pub error: Option<String>,
    pub tab: String,
    pub dims: DimState,
}

/// A linked account: (account id, connection id, connection label, last sync).
pub(crate) type Linked = (String, String, String, String);

/// Whether connections (feeds, processors, brokers) are offered.
pub(crate) fn connections_on(app: &mut App) -> bool {
    app.module_on("bank-feeds")
        || app.module_on("payment-processing")
        || app.module_on("investments")
}

/// MoneyKeep accounts linked to a connection.
pub(crate) fn linked_accounts(app: &mut App) -> Vec<Linked> {
    if !connections_on(app) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for c in array(app.backend.invoke("connect.list", json!({})).ok()) {
        for a in c.list("accounts") {
            if a.at("active").as_bool() == Some(true) && !a.str("account").is_empty() {
                out.push((
                    a.str("account").to_owned(),
                    c.str("id").to_owned(),
                    c.str("label").to_owned(),
                    c.str("last_sync_at").to_owned(),
                ));
            }
        }
    }
    out
}

const TREE_GROUPS: [&str; 8] = [
    "Cash & bank",
    "Credit cards",
    "Investments",
    "Other assets",
    "Loans & other debts",
    "Equity",
    "Income",
    "Expenses",
];

fn tree_group(a: &Value) -> usize {
    match group_of(a) {
        6 if a.str("type") == "income" => 6,
        6 => 7,
        g => g,
    }
}

/// A friendly kind label; never the internal type or subtype slug.
pub(crate) fn kind_label(a: &Value) -> &'static str {
    if a.at("placeholder").as_bool() == Some(true) {
        return "Group";
    }
    match (a.str("type"), a.str("subtype")) {
        (_, "bank") => "Bank",
        (_, "cash") => "Cash",
        (_, "credit-card") => "Credit card",
        (_, "brokerage") => "Brokerage",
        (_, "security") => "Security",
        (_, "receivable") => "Money owed to me",
        (_, "fixed-asset") => "Property or equipment",
        (_, "loan") => "Loan",
        (_, "other-liability") => "Other debt",
        (_, "opening-balances") => "Opening balances",
        (_, "owner-equity") => "Equity",
        (_, "cost-of-goods") => "Cost of goods sold",
        ("asset", _) => "Other asset",
        ("liability", _) => "Debt",
        ("equity", _) => "Equity",
        ("income", _) => "Income",
        _ => "Expense",
    }
}

/// The dimension tabs shown now (Accounts first).
fn tabs(app: &mut App) -> Vec<(&'static str, &'static str)> {
    let mut out = vec![("", "Chart of accounts")];
    for (dim, module, title, _) in DIMS {
        if app.module_on(module) {
            out.push((dim, title));
        }
    }
    out
}

pub(crate) fn view(app: &mut App, ui: &mut Ui) {
    ui.title("Accounts");
    let tabs = tabs(app);
    if !tabs.iter().any(|t| t.0 == app.accounts.tab) {
        app.accounts.tab.clear();
    }
    if tabs.len() > 1 {
        ui.wrap(|ui| {
            for (dim, title) in &tabs {
                let selected = app.accounts.tab == *dim;
                if ui.tab(*title, selected).clicked() {
                    app.accounts.tab = (*dim).to_owned();
                }
            }
        });
    }
    if !app.accounts.tab.is_empty() {
        let dim = app.accounts.tab.clone();
        dim_view(app, ui, &dim);
        dim_dialogs(app, ui);
        return;
    }
    let investments = app.module_on("investments");
    ui.toolbar(|ui| {
        if ui.primary("New account…").clicked() {
            app.accounts.new = Some(NewAccount::default());
        }
        if investments && ui.button("Securities").clicked() {
            app.navigate(View::Securities);
        }
        if ui.button("Add accounts from a chart…").clicked() {
            super::presets::open_add(app);
        }
        if ui.button("Export IIF…").clicked() {
            super::iif::open_export(app);
        }
        let more = vec!["Opening balances…".to_owned()];
        ui.named("More account actions");
        if ui.menu("…", &more).chosen.is_some() {
            super::opening::open_checklist(app, None);
        }
    });
    if let Some(e) = app.accounts.error.clone() {
        ui.error_text(e);
    }
    account_tree(app, ui);
    new_account_dialog(app, ui);
    edit_account_dialog(app, ui);
    small_dialogs(app, ui);
    super::presets::add_dialog(app, ui);
}

/// The tooltip of the Tax line field (app-desktop.md revision 34).
const TAX_LINE_TIP: &str =
    "Where this account's total goes on your return. A hint for reports, not tax advice.";

/// The line shown for an account with a role.
fn role_line(purpose: &str) -> String {
    format!("Used by MoneyKeep for {purpose}")
}

fn tax_line_row(ui: &mut Ui, text: &mut String) {
    ui.form("Tax line", |ui| {
        ui.width(240);
        ui.hint("e.g. Sch C L18");
        ui.tip(TAX_LINE_TIP);
        ui.field("Tax line", text);
    });
}

fn open_edit(app: &mut App, a: &Value) {
    let chart = array(app.read("account.list", json!({"include_archived": true})));
    let id = a.str("id");
    let has_children = chart.iter().any(|c| c.str("parent") == id);
    let placeholder = a.at("placeholder").as_bool() == Some(true);
    // A heading shows its type as text ("Group"); it holds no transactions.
    let money = money_index(a).filter(|_| !placeholder).map(|i| (i, i));
    app.accounts.edit = Some(EditAccount {
        money,
        retirement: (a.str("subtype") == "brokerage")
            .then(|| a.at("retirement").as_bool() == Some(true)),
        same_side: placeholder || has_children,
        type_text: kind_label(a).to_owned(),
        group: String::new(),
        groups: Vec::new(),
        chart,
        id: a.str("id").to_owned(),
        name: a.str("name").to_owned(),
        code: a.str("code").to_owned(),
        description: a.str("description").to_owned(),
        tax_line: a.str("tax_line").to_owned(),
        placeholder: a.at("placeholder").as_bool() == Some(true),
        role: a.str("role").to_owned(),
        role_purpose: a.str("role_purpose").to_owned(),
        error: None,
    });
}

fn edit_account_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.accounts.edit.clone() else {
        return;
    };
    let mut close = ui.dialog_closed("edit-account");
    ui.dialog("edit-account", "Edit account", 520, |ui| {
        ui.group(|ui| {
            text_row(ui, "Account name", &mut d.name, "");
            text_row(ui, "Account number", &mut d.code, "e.g. 6010");
            ui.form("Description", |ui| {
                ui.next().grow();
                ui.multiline("Description", &mut d.description);
            });
            account_type_rows(ui, &mut d);
            if !d.placeholder {
                tax_line_row(ui, &mut d.tax_line);
            }
            if let Some(r) = d.retirement.as_mut() {
                ui.form("", |ui| {
                    ui.switch(r, "Retirement account (IRA, 401(k), Roth and similar)");
                });
            }
            if !d.role_purpose.is_empty() {
                ui.secondary(role_line(&d.role_purpose));
            }
            if let Some(e) = &d.error {
                ui.error_text(e.clone());
            }
        });
        ui.actions(|ui| {
            let protected = d.role == "opening_balances";
            ui.enabled(!protected);
            if protected {
                ui.tip(role_line(&d.role_purpose));
            }
            if ui.button("Delete").clicked() && !protected {
                match app.run("account.delete", json!({"account": d.id})) {
                    Ok(_) => close = true,
                    Err(e) => d.error = Some(e.display()),
                }
            }
            ui.spacer();
            if ui.button("Cancel").clicked() {
                close = true;
            }
            if ui.primary("Save").clicked() {
                let code = if d.code.trim().is_empty() {
                    Value::Null
                } else {
                    json!(d.code.trim())
                };
                let mut args = json!({"account": d.id, "name": d.name, "code": code,
                    "description": d.description});
                if !d.placeholder {
                    set(&mut args, "tax_line", json!(d.tax_line.trim()));
                }
                if let Some(r) = d.retirement {
                    set(&mut args, "retirement", json!(r));
                }
                if let Some((chosen, current)) = d.money
                    && chosen != current
                    && let (Some(new), Some(old)) =
                        (MONEY_KINDS.get(chosen), MONEY_KINDS.get(current))
                {
                    set(&mut args, "type", json!(new.1));
                    set(&mut args, "subtype", json!(new.2));
                    if new.1 != old.1 {
                        let parent = if d.group.is_empty() {
                            Value::Null
                        } else {
                            json!(d.group)
                        };
                        set(&mut args, "parent", parent);
                    }
                }
                match app.run("account.update", args) {
                    Ok(_) => close = true,
                    Err(e) => d.error = Some(e.display()),
                }
            }
        });
    });
    app.accounts.edit = if close { None } else { Some(d) };
}

/// Account type (a choice for money accounts, text for others) and, when the side
/// changes, Group and the note (app-desktop.md revision 35).
fn account_type_rows(ui: &mut Ui, d: &mut EditAccount) {
    let Some((mut chosen, current)) = d.money else {
        let text = d.type_text.clone();
        ui.form("Account type", |ui| {
            ui.label(text);
        });
        return;
    };
    let side = |i: usize| MONEY_KINDS.get(i).map_or("asset", |k| k.1);
    let offered: Vec<usize> = (0..MONEY_KINDS.len())
        .filter(|i| !d.same_side || side(*i) == side(current))
        .collect();
    let labels: Vec<&str> = offered
        .iter()
        .filter_map(|i| MONEY_KINDS.get(*i).map(|k| k.0))
        .collect();
    let mut pos = offered.iter().position(|i| *i == chosen).unwrap_or(0);
    let mut changed = false;
    ui.form("Account type", |ui| {
        changed = super::form::choice(ui, "Account type", &mut pos, &labels);
    });
    if changed {
        chosen = offered.get(pos).copied().unwrap_or(current);
        d.money = Some((chosen, current));
        if let Some(k) = MONEY_KINDS.get(chosen)
            && side(chosen) != side(current)
        {
            d.groups = group_choices(&d.chart, &d.id, k.1);
            d.group = default_group(&d.chart, &d.id, k.1, k.2);
        }
    }
    if side(chosen) != side(current) {
        let groups = d.groups.clone();
        ui.form("Group", |ui| {
            super::form::pick(ui, "Group", &mut d.group, &groups, Some("Top level"));
        });
        let shown = if side(chosen) == "liability" {
            "a liability"
        } else {
            "an asset"
        };
        ui.secondary(format!(
            "Balances and reports will show this account as {shown}. Its transactions \
             do not change."
        ));
    }
}

fn account_tree(app: &mut App, ui: &mut Ui) {
    let archived = app.pane.show_archived;
    app.pane.show_archived = true;
    let data = app.sidebar_data();
    app.pane.show_archived = archived;
    let list = data.list.clone();
    let feeds = linked_accounts(app);
    for (gi, group) in TREE_GROUPS.iter().enumerate() {
        let members: Vec<Value> = members_where(&list, |a| tree_group(a) == gi)
            .into_iter()
            .cloned()
            .collect();
        if members.is_empty() {
            continue;
        }
        let cols = vec![
            Column {
                title: "Account".into(),
                weight: 3,
                ..Column::default()
            },
            Column {
                title: "Kind".into(),
                width: 180,
                ..Column::default()
            },
            Column {
                title: "Balance".into(),
                width: 140,
                align: Align::Right,
                ..Column::default()
            },
            Column {
                title: "Actions".into(),
                width: 72,
                align: Align::Center,
                ..Column::default()
            },
        ];
        ui.table(group, cols, |ui| {
            for a in &members {
                let bal = data
                    .balances
                    .iter()
                    .find(|(id, _)| id == a.str("id"))
                    .map_or_else(|| "0".to_owned(), |(_, b)| b.clone());
                let feed = feeds.iter().find(|f| f.0 == a.str("id")).cloned();
                tree_row(
                    app,
                    ui,
                    a,
                    &at_scale(&bal, a.str("commodity")),
                    feed.as_ref(),
                );
            }
        });
    }
}

fn tree_row(app: &mut App, ui: &mut Ui, a: &Value, balance: &str, feed: Option<&Linked>) {
    let depth = i32::try_from(a.at("depth").as_u64().unwrap_or(0)).unwrap_or(0);
    let full = a.str("full_name").to_owned();
    let id = a.str("id").to_owned();
    let mut menu_pick = None;
    ui.table_row(&format!("account-row:{id}"), false, false, |ui| {
        ui.row(|ui| {
            if depth > 0 {
                ui.width(depth * 16);
                ui.spacer();
            }
            let archived = a.at("archived").as_bool() == Some(true);
            ui.tip(full.clone()).tone(if archived {
                Tone::Secondary
            } else {
                Tone::Normal
            });
            let code = a.str("code");
            if !code.is_empty() {
                ui.named(format!("Account number of {full}: {code}"));
                ui.tone(Tone::Secondary).mono().width(44);
                ui.label(code);
            }
            if a.at("placeholder").as_bool() == Some(true) {
                ui.bold();
            }
            ui.grow();
            ui.label(a.str("name"));
            if let Some((_, _, label, last)) = feed {
                let when = if last.is_empty() {
                    "not synced yet".to_owned()
                } else {
                    format!(
                        "last sync {}",
                        crate::fmt::date(&last.get(..16).unwrap_or(last).replace('T', " "))
                    )
                };
                ui.tip(format!("Bank feed {label}, {when}"))
                    .named(format!("Feed for {full}"));
                ui.badge("feed", Tone::Accent);
            }
            let note = a.str("note");
            if !note.is_empty() {
                ui.named(format!("{full}: {note}"));
                ui.secondary(note);
            }
            let tax = a.str("tax_line");
            if !tax.is_empty() {
                ui.named(format!("Tax line of {full}: {tax}"))
                    .tip(TAX_LINE_TIP);
                ui.secondary(tax);
            }
        });
        ui.secondary(kind_label(a));
        ui.named(format!("Balance of {full}: {}", crate::fmt::group(balance)))
            .tip(format!("{full} balance"))
            .tone(if crate::fmt::is_negative(balance) {
                Tone::Decrease
            } else {
                Tone::Normal
            });
        ui.amount(crate::fmt::group(balance));
        let archived = a.at("archived").as_bool() == Some(true);
        // The Opening Balances account cannot be deleted (its dialog says why).
        let deletable = a.str("role") != "opening_balances";
        let entries: Vec<String> = [
            "Open register",
            "Edit…",
            "Rename…",
            "Move to…",
            if archived { "Unarchive" } else { "Archive" },
            "Delete",
        ]
        .iter()
        .filter(|e| deletable || **e != "Delete")
        .map(|s| (*s).to_owned())
        .chain(super::opening::menu_entry(a).map(str::to_owned))
        .collect();
        ui.named(format!("Actions for {full}"))
            .id(format!("actions:{id}"));
        let resp = ui.menu("…", &entries);
        if let Some(i) = resp.chosen {
            menu_pick = entries.get(i).cloned();
        }
    });
    ui.indent_last(0);
    let run = |app: &mut App, cmd: &str| {
        app.accounts.error = app
            .run(cmd, json!({"account": id}))
            .err()
            .map(|e| e.display());
    };
    match menu_pick.as_deref() {
        Some("Open register") => app.navigate(View::Register {
            account: id.clone(),
        }),
        Some("Edit…") => open_edit(app, a),
        Some("Rename…") => app.accounts.renaming = Some((id.clone(), a.str("name").to_owned())),
        Some("Move to…") => app.accounts.moving = Some((id.clone(), String::new())),
        Some("Unarchive") => run(app, "account.unarchive"),
        Some("Archive") => run(app, "account.archive"),
        Some("Delete") => run(app, "account.delete"),
        Some("Set opening balance…" | "Set opening holdings…") => {
            super::opening::open_for(app, &id);
        }
        _ => {}
    }
}

fn new_account_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.accounts.new.clone() else {
        return;
    };
    let mut close = ui.dialog_closed("new-account");
    let mut help = false;
    ui.dialog("new-account", "New account", 560, |ui| {
        ui.group(|ui| {
            text_row(ui, "Account name", &mut d.name, "");
            ui.form("Kind", |ui| {
                let kinds: Vec<&str> = KINDS.iter().map(|k| k.0).collect();
                super::form::choice(ui, "Account kind", &mut d.kind, &kinds);
            });
            text_row(ui, "Parent account (optional)", &mut d.parent, "");
            text_row(ui, "Account number (optional)", &mut d.code, "e.g. 6010");
            tax_line_row(ui, &mut d.tax_line);
            ui.form("Opening balance (optional)", |ui| {
                ui.width(160);
                ui.field("Opening balance (optional)", &mut d.opening);
                if ui.link("Help me work it out…").clicked() {
                    help = true;
                }
            });
            date_row(ui, "Opening balance date", &mut d.opening_date);
            if let Some(e) = &d.error {
                ui.error_text(e.clone());
            }
        });
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                close = true;
            }
            if ui.primary("Create account").clicked() {
                match create(app, &d) {
                    Ok(_) => close = true,
                    Err(e) => d.error = Some(e),
                }
            }
        });
    });
    // opening-balances.md: create the account, then open the wizard for it.
    if help {
        let mut quick = d.clone();
        quick.opening.clear();
        match create(app, &quick) {
            Ok(id) => {
                close = true;
                super::opening::open_for(app, &id);
            }
            Err(e) => d.error = Some(e),
        }
    }
    app.accounts.new = if close { None } else { Some(d) };
}

fn create(app: &mut App, d: &NewAccount) -> Result<String, String> {
    let (_, ty, sub) = KINDS.get(d.kind).copied().ok_or("choose a kind")?;
    let mut args = json!({"name": d.name, "type": ty, "subtype": sub});
    if !d.parent.trim().is_empty() {
        set(&mut args, "parent", json!(d.parent.trim()));
    }
    if !d.code.trim().is_empty() {
        set(&mut args, "code", json!(d.code.trim()));
    }
    if !d.tax_line.trim().is_empty() {
        set(&mut args, "tax_line", json!(d.tax_line.trim()));
    }
    let opening = crate::fmt::parse_entry(&d.opening)?;
    let date = crate::fmt::date_arg(&d.opening_date)?;
    if opening.is_some() && date.is_empty() {
        return Err("Give the opening balance a date.".into());
    }
    let created = app.run("account.create", args).map_err(|e| e.display())?;
    if let Some(amount) = opening.filter(|a| !a.is_zero()) {
        // Quick entry: the opening entry itself (opening-balances.md), marked as the
        // account's opening balance. Other kinds keep the plain transaction.
        let res = if matches!(ty, "asset" | "liability") {
            app.run(
                "opening.apply",
                json!({"account": created.str("id"), "amount": amount.to_string(), "date": date}),
            )
        } else {
            let opening_id = app
                .read("book.info", json!({}))
                .map(|i| i.str("opening_balances_account").to_owned())
                .unwrap_or_default();
            let raw = if ty == "expense" { amount } else { -amount };
            app.run(
                "txn.create",
                json!({"date": date, "payee": "Opening balance", "splits": [
                {"account": created.str("id"), "value": raw.to_string()},
                {"account": opening_id, "value": (-raw).to_string()}]}),
            )
        };
        if let Err(e) = res {
            return Err(format!(
                "The account was created, but its opening balance was not: {}",
                e.display()
            ));
        }
    }
    Ok(created.str("id").to_owned())
}

fn small_dialogs(app: &mut App, ui: &mut Ui) {
    if let Some((id, mut name)) = app.accounts.renaming.clone() {
        let mut close = ui.dialog_closed("rename");
        ui.dialog("rename", "Rename account", 440, |ui| {
            text_row(ui, "New name", &mut name, "");
            ui.actions(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.primary("Rename").clicked() {
                    match app.run("account.update", json!({"account": id, "name": name})) {
                        Ok(_) => close = true,
                        Err(e) => app.accounts.error = Some(e.display()),
                    }
                }
            });
        });
        app.accounts.renaming = if close { None } else { Some((id, name)) };
    }
    if let Some((id, mut parent)) = app.accounts.moving.clone() {
        let mut close = ui.dialog_closed("move");
        ui.dialog("move", "Move account", 440, |ui| {
            text_row(ui, "New parent (empty for top level)", &mut parent, "");
            ui.actions(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.primary("Move").clicked() {
                    let p = if parent.trim().is_empty() {
                        Value::Null
                    } else {
                        json!(parent.trim())
                    };
                    match app.run("account.move", json!({"account": id, "parent": p})) {
                        Ok(_) => close = true,
                        Err(e) => app.accounts.error = Some(e.display()),
                    }
                }
            });
        });
        app.accounts.moving = if close { None } else { Some((id, parent)) };
    }
}

// ----- Classes, Locations, Funds (revision 16) ---------------------------------------

pub(crate) const DIMS: [(&str, &str, &str, &str); 3] = [
    ("class", "classes", "Classes", "class"),
    ("location", "locations", "Locations", "location"),
    ("fund", "funds", "Funds", "fund"),
];

const FUNCTIONS: [(&str, &str); 4] = [
    ("", "Not assigned"),
    ("program", "Program"),
    ("management_general", "Management and general"),
    ("fundraising", "Fundraising"),
];

const RESTRICTIONS: [(&str, &str); 2] = [
    ("without_donor_restrictions", "Without donor restrictions"),
    ("with_donor_restrictions", "With donor restrictions"),
];

#[derive(Debug, Clone, Default)]
pub(crate) struct DimForm {
    pub dim: String,
    pub editing: Option<String>,
    pub name: String,
    pub function: usize,
    pub restriction: usize,
    pub purpose: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FundMove {
    pub transfer: bool,
    pub from: String,
    pub to: String,
    pub amount: String,
    pub date: String,
    pub memo: String,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct DimState {
    pub form: Option<DimForm>,
    pub fund_move: Option<FundMove>,
    pub error: Option<String>,
}

fn dim_list(app: &mut App, dim: &str) -> Vec<Value> {
    let l = app.lists();
    match dim {
        "class" => l.classes.clone(),
        "location" => l.locations.clone(),
        _ => l.funds.clone(),
    }
}

fn dim_view(app: &mut App, ui: &mut Ui, dim: &str) {
    let word = DIMS.iter().find(|d| d.0 == dim).map_or("fund", |d| d.3);
    ui.toolbar(|ui| {
        if ui.primary(format!("New {word}…")).clicked() {
            app.accounts.dims.form = Some(DimForm {
                dim: dim.to_owned(),
                ..DimForm::default()
            });
        }
        if dim == "fund" {
            if ui.button("Release from restriction…").clicked() {
                app.accounts.dims.fund_move = Some(FundMove {
                    date: crate::fmt::date(&app.today()),
                    to: "General".into(),
                    ..FundMove::default()
                });
            }
            if ui.button("Transfer between funds…").clicked() {
                app.accounts.dims.fund_move = Some(FundMove {
                    transfer: true,
                    date: crate::fmt::date(&app.today()),
                    ..FundMove::default()
                });
            }
        }
    });
    if let Some(e) = app.accounts.dims.error.clone() {
        ui.error_text(e);
    }
    let rows = dim_list(app, dim);
    if rows.is_empty() {
        ui.secondary(match dim {
            "class" => "No classes yet.",
            "location" => "No locations yet.",
            _ => "No funds yet.",
        });
        return;
    }
    let head = match dim {
        "class" => "Class",
        "location" => "Location",
        _ => "Fund",
    };
    let detail_title = match dim {
        "class" => "Function",
        "fund" => "Restriction",
        _ => "",
    };
    let cols = vec![
        Column {
            title: head.into(),
            weight: 2,
            ..Column::default()
        },
        Column {
            title: detail_title.into(),
            width: 220,
            ..Column::default()
        },
        Column {
            title: "Active".into(),
            width: 64,
            align: Align::Center,
            ..Column::default()
        },
        Column {
            title: "Used".into(),
            width: 64,
            align: Align::Right,
            ..Column::default()
        },
        Column {
            title: if dim == "fund" {
                "Net assets".into()
            } else {
                String::new()
            },
            width: 200,
            align: Align::Right,
            ..Column::default()
        },
        Column {
            title: String::new(),
            width: 90,
            ..Column::default()
        },
    ];
    let mut active_change = None;
    let mut edit = None;
    ui.table("Names", cols, |ui| {
        for r in &rows {
            let name = r.str("name").to_owned();
            let level = i32::try_from(r.at("level").as_u64().unwrap_or(0)).unwrap_or(0);
            let leaf = name.rsplit(':').next().unwrap_or(&name).to_owned();
            ui.table_row(&format!("dim:{}", r.str("id")), false, false, |ui| {
                ui.row(|ui| {
                    if level > 0 {
                        ui.width(level * 16);
                        ui.spacer();
                    }
                    ui.tip(name.clone());
                    ui.label(leaf);
                });
                let detail = match dim {
                    "class" => FUNCTIONS
                        .iter()
                        .find(|f| f.0 == r.str("function"))
                        .map_or("Not assigned", |f| f.1),
                    "fund" => RESTRICTIONS
                        .iter()
                        .find(|f| f.0 == r.str("restriction"))
                        .map_or("", |f| f.1),
                    _ => "",
                };
                ui.label(detail);
                let mut active = r.at("active").as_bool() != Some(false);
                ui.tip("Inactive names are kept on old entries but not offered for new ones")
                    .named(format!("Active {name}"));
                if ui.checkbox(&mut active, "").clicked() {
                    active_change = Some((r.str("id").to_owned(), active));
                }
                ui.amount(r.at("uses").as_u64().unwrap_or(0).to_string());
                if dim == "fund" {
                    ui.row(|ui| {
                        ui.amount(crate::fmt::group(r.str("net_assets")));
                        if r.at("overspent").as_bool() == Some(true) {
                            let over = r.str("net_assets").trim_start_matches('-').to_owned();
                            ui.error_text(format!("Overspent by {}", crate::fmt::group(&over)));
                        }
                    });
                } else {
                    ui.label("");
                }
                ui.named(format!("Edit {name}"));
                if ui.button("Edit…").clicked() {
                    edit = Some(r.clone());
                }
            });
        }
    });
    if let Some((record, active)) = active_change {
        app.accounts.dims.error = app
            .run(
                &format!("{dim}.set_active"),
                json!({"record": record, "active": active}),
            )
            .err()
            .map(|e| e.display());
    }
    if let Some(r) = edit {
        app.accounts.dims.form = Some(DimForm {
            dim: dim.to_owned(),
            editing: Some(r.str("id").to_owned()),
            name: r.str("name").to_owned(),
            function: FUNCTIONS
                .iter()
                .position(|f| f.0 == r.str("function"))
                .unwrap_or(0),
            restriction: RESTRICTIONS
                .iter()
                .position(|f| f.0 == r.str("restriction"))
                .unwrap_or(0),
            purpose: r.str("purpose").to_owned(),
            error: None,
        });
    }
}

fn dim_dialogs(app: &mut App, ui: &mut Ui) {
    if let Some(mut f) = app.accounts.dims.form.take() {
        let word = DIMS.iter().find(|d| d.0 == f.dim).map_or("fund", |d| d.3);
        let title = if f.editing.is_some() {
            format!("Edit {word}")
        } else {
            format!("New {word}")
        };
        let mut save = false;
        let mut cancel = ui.dialog_closed("dimension-form");
        ui.dialog("dimension-form", &title, 480, |ui| {
            ui.group(|ui| {
                text_row(ui, "Name", &mut f.name, "Parent:Child for a level below");
                if f.dim == "class" {
                    ui.form("Function", |ui| {
                        let items: Vec<&str> = FUNCTIONS.iter().map(|x| x.1).collect();
                        super::form::choice(ui, "Function", &mut f.function, &items);
                    });
                }
                if f.dim == "fund" {
                    ui.form("Restriction", |ui| {
                        let items: Vec<&str> = RESTRICTIONS.iter().map(|x| x.1).collect();
                        super::form::choice(ui, "Restriction", &mut f.restriction, &items);
                    });
                    text_row(ui, "Purpose", &mut f.purpose, "");
                }
                if let Some(e) = &f.error {
                    ui.error_text(e.clone());
                }
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
        let mut done = false;
        if save {
            let mut a = json!({"name": f.name.trim()});
            if f.dim == "class" {
                set(
                    &mut a,
                    "function",
                    json!(FUNCTIONS.get(f.function).map_or("", |x| x.0)),
                );
            }
            if f.dim == "fund" {
                set(
                    &mut a,
                    "restriction",
                    json!(
                        RESTRICTIONS
                            .get(f.restriction)
                            .map_or("without_donor_restrictions", |x| x.0)
                    ),
                );
                set(&mut a, "purpose", json!(f.purpose));
            }
            let cmd = match &f.editing {
                Some(id) => {
                    set(&mut a, "record", json!(id));
                    format!("{}.update", f.dim)
                }
                None => format!("{}.create", f.dim),
            };
            match app.run(&cmd, a) {
                Ok(_) => done = true,
                Err(e) => f.error = Some(e.display()),
            }
        }
        if !cancel && !done {
            app.accounts.dims.form = Some(f);
        }
    }
    if let Some(mut m) = app.accounts.dims.fund_move.take() {
        let funds = dim_list(app, "fund");
        let restricted: Vec<String> = funds
            .iter()
            .filter(|f| {
                f.str("restriction") == "with_donor_restrictions"
                    && f.at("active").as_bool() != Some(false)
            })
            .map(|f| f.str("name").to_owned())
            .collect();
        let all = names(&funds, true);
        let title = if m.transfer {
            "Transfer between funds"
        } else {
            "Release from restriction"
        };
        let mut save = false;
        let mut cancel = ui.dialog_closed("fund-move");
        ui.dialog("fund-move", title, 480, |ui| {
            ui.group(|ui| {
                if m.transfer {
                    pick_row(ui, "From fund", &mut m.from, &all, None);
                    pick_row(ui, "To fund", &mut m.to, &all, None);
                } else {
                    pick_row(ui, "Fund", &mut m.from, &restricted, None);
                }
                text_row(ui, "Amount", &mut m.amount, "0.00");
                date_row(ui, "Date", &mut m.date);
                text_row(ui, "Memo", &mut m.memo, "");
                if !m.transfer {
                    ui.secondary(
                        "Moves the amount from the fund's restricted net assets to General.",
                    );
                }
                if let Some(e) = &m.error {
                    ui.error_text(e.clone());
                }
            });
            ui.actions(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                if ui
                    .primary(if m.transfer { "Transfer" } else { "Release" })
                    .clicked()
                {
                    save = true;
                }
            });
        });
        let mut done = false;
        if save {
            let mut a = json!({"amount": super::form::amount_arg(&m.amount), "date": crate::fmt::date_arg_lenient(&m.date)});
            put(&mut a, "memo", &m.memo);
            let cmd = if m.transfer {
                set(&mut a, "from", json!(m.from));
                set(&mut a, "to", json!(m.to));
                "fund.transfer"
            } else {
                set(&mut a, "from_fund", json!(m.from));
                "fund.release"
            };
            match app.run(cmd, a) {
                Ok(_) => done = true,
                Err(e) => m.error = Some(e.display()),
            }
        }
        if !cancel && !done {
            app.accounts.dims.fund_move = Some(m);
        }
    }
}
