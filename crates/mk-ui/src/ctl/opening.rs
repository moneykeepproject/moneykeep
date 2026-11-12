//! Opening balances (opening-balances.md): the Opening balance wizard for money
//! accounts, the Investment account opening balance wizard, the setup checklist
//! with its progress bar, the later cost basis dialog and the help page. Every
//! number comes from the `opening.*` commands; this module only shows them and
//! collects the answers.

use serde_json::{Value, json};

use super::App;
use super::form::{date_row, set, text_row};
use super::ui::{Tone, Ui};
use super::util::{Json, array};
use crate::{View, fmt};

pub(crate) const MONEY_STEPS: [&str; 5] = ["Account", "Balance", "Start date", "Check", "Done"];
pub(crate) const INV_STEPS: [&str; 6] =
    ["Account", "Cash", "Holdings", "Cost basis", "Check", "Done"];

/// The help page "Opening balances".
pub(crate) const HELP: &str = include_str!("../../../../docs/help/opening-balances.md");

pub(crate) const REASSURE: &str =
    "You can change this any time — the opening balance is an ordinary transaction.";
const BALANCE_HINT: &str = "From your latest statement or your bank's website.";
const NOTHING: &str =
    "Nothing to work back from — your opening balance is simply your balance today.";
const OTHER_SIDE: &str = "Opening Balances is where the starting money comes from.";

/// The kinds a security not in the book yet can be created as
/// (investments-options-bonds-crypto.md: options, bonds and crypto too).
const SEC_KINDS: [(&str, &str); 8] = [
    ("stock", "Stock"),
    ("etf", "ETF"),
    ("mutual_fund", "Mutual fund"),
    ("money_market", "Money market"),
    ("option", "Option"),
    ("bond", "Bond"),
    ("crypto", "Crypto"),
    ("other", "Other"),
];

#[derive(Debug, Default)]
pub(crate) struct OpeningState {
    pub checklist: Option<Checklist>,
    pub money: Option<MoneyWizard>,
    pub inv: Option<InvWizard>,
    pub basis: Option<BasisDialog>,
    pub help_open: bool,
    /// The last command answer: (generation, command and arguments, answer).
    cache: Option<(u64, String, Result<Value, String>)>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct Checklist {
    /// Only these accounts (after linking a feed).
    pub only: Option<Vec<String>>,
    pub error: Option<String>,
}

/// The cash answers shared by both wizards.
#[derive(Debug, Default, Clone)]
pub(crate) struct CashForm {
    /// The feed's balance: None not answered, Some(true) "Yes, use it".
    pub use_feed: Option<bool>,
    pub balance: String,
    pub as_of: String,
    pub owes_me: bool,
    pub statement: bool,
    pub stmt_date: String,
    pub stmt_balance: String,
    /// Pre-start transactions to keep (unticked).
    pub keep: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct MoneyWizard {
    pub account: String,
    pub step: usize,
    pub cash: CashForm,
    /// The other side ("" = the suggestion), and whether Change is open.
    pub offset: String,
    pub offset_open: bool,
    pub other: String,
    /// Loans: "for something you own?" and the asset.
    pub loan: Option<bool>,
    pub asset_name: String,
    pub asset_value: String,
    /// Go ahead was chosen at the reconciled note.
    pub go_ahead: bool,
    pub error: Option<String>,
    pub done: Option<Value>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct Holding {
    pub symbol: String,
    pub name: String,
    pub shares: String,
    /// "", "lots", "total" or "unknown".
    pub level: String,
    /// (acquired, shares, cost) as typed.
    pub lots: Vec<(String, String, String)>,
    pub total: String,
    pub per_share: bool,
    pub acquired: String,
    pub price: String,
    pub kind: usize,
    /// A new bond: maturity and coupon rate.
    pub maturity: String,
    pub coupon: String,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct InvWizard {
    pub account: String,
    pub step: usize,
    pub retirement: bool,
    pub retirement_set: bool,
    pub cash: CashForm,
    pub rows: Vec<Holding>,
    pub prefilled: bool,
    pub paste: String,
    pub paste_open: bool,
    /// Pasted symbols not in the book, waiting for the confirm.
    pub unknown: Vec<String>,
    pub enter_basis: bool,
    pub broker_total: String,
    pub offset: String,
    pub go_ahead: bool,
    pub error: Option<String>,
    pub done: Option<Value>,
}

/// Supply the cost basis of a holding recorded as Basis needed.
#[derive(Debug, Default, Clone)]
pub(crate) struct BasisDialog {
    pub account: String,
    pub symbol: String,
    pub total: String,
    pub per_share: bool,
    pub acquired: String,
    pub error: Option<String>,
}

// ----- opening --------------------------------------------------------------------------

/// Open the setup checklist (Home → Attention, Accounts → … → Opening balances…, or
/// after linking a feed with `only` its accounts).
pub(crate) fn open_checklist(app: &mut App, only: Option<Vec<String>>) {
    app.opening.checklist = Some(Checklist { only, error: None });
}

/// Open the right wizard for an account (brokerage: the investment wizard).
pub(crate) fn open_for(app: &mut App, account: &str) {
    let acc = app.read("account.get", json!({"account": account}));
    let Some(acc) = acc else { return };
    let id = acc.str("id").to_owned();
    if acc.str("subtype") == "brokerage" {
        app.opening.inv = Some(InvWizard {
            account: id,
            retirement: acc.at("retirement").as_bool() == Some(true),
            ..InvWizard::default()
        });
    } else {
        app.opening.money = Some(MoneyWizard {
            account: id,
            ..MoneyWizard::default()
        });
    }
}

/// The menu entry for an account's ⋯ menu: Set opening balance… or Set opening
/// holdings…; none for accounts off the balance sheet.
pub(crate) fn menu_entry(a: &Value) -> Option<&'static str> {
    if a.at("placeholder").as_bool() == Some(true) || a.str("subtype") == "security" {
        return None;
    }
    match (a.str("type"), a.str("subtype")) {
        (_, "brokerage") => Some("Set opening holdings…"),
        ("asset" | "liability", _) => Some("Set opening balance…"),
        _ => None,
    }
}

/// All dialogs of this module (one at a time: a wizard covers the checklist).
pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    if app.opening.help_open {
        help_dialog(app, ui);
        return;
    }
    if app.opening.money.is_some() {
        money_dialog(app, ui);
    } else if app.opening.inv.is_some() {
        inv_dialog(app, ui);
    } else if app.opening.basis.is_some() {
        basis_dialog(app, ui);
    } else if app.opening.checklist.is_some() {
        checklist_dialog(app, ui);
    }
}

/// A reading command, answered once per book change.
fn cached(app: &mut App, command: &str, args: Value) -> Result<Value, String> {
    let key = format!("{command} {args}");
    if let Some((g, k, v)) = &app.opening.cache
        && *g == app.generation
        && *k == key
    {
        return v.clone();
    }
    let v = app.read_or_error(command, args);
    app.opening.cache = Some((app.generation, key, v.clone()));
    v
}

fn amount(text: &str) -> Option<String> {
    match fmt::parse_entry(text) {
        Ok(Some(d)) => Some(d.to_string()),
        _ => None,
    }
}

fn iso(text: &str) -> Option<String> {
    fmt::date_arg(text).ok().filter(|d| !d.is_empty())
}

fn cash_args(c: &CashForm, feed: bool) -> Value {
    let mut a = json!({});
    if !feed || c.use_feed == Some(false) {
        if let Some(b) = amount(&c.balance) {
            set(&mut a, "balance", json!(b));
        }
        if let Some(d) = iso(&c.as_of) {
            set(&mut a, "as_of", json!(d));
        }
    }
    if c.owes_me {
        set(&mut a, "owes_me", json!(true));
    }
    if c.statement {
        set(&mut a, "start", json!("statement"));
        if let Some(d) = iso(&c.stmt_date) {
            set(&mut a, "statement_date", json!(d));
        }
        if let Some(b) = amount(&c.stmt_balance) {
            set(&mut a, "statement_balance", json!(b));
        }
        if !c.keep.is_empty() {
            set(&mut a, "keep", json!(c.keep));
        }
    }
    a
}

fn merge(a: &mut Value, b: Value) {
    if let (Some(o), Some(m)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in m {
            o.insert(k.clone(), v.clone());
        }
    }
}

/// The step bar: "1. Account · 2. Balance · …"; returns a chosen earlier step.
fn step_bar(ui: &mut Ui, steps: &[&str], step: usize, reached: usize) -> Option<usize> {
    let mut go = None;
    ui.wrap(|ui| {
        for (i, name) in steps.iter().enumerate() {
            let done = i < step;
            ui.named(format!(
                "Step {}: {name}{}",
                i + 1,
                if done { ", done" } else { "" }
            ));
            ui.enabled(i <= reached && i + 1 < steps.len());
            if done {
                ui.icon("check");
            }
            if ui.tab(format!("{}. {name}", i + 1), i == step).clicked() && i != step {
                go = Some(i);
            }
        }
    });
    ui.separator();
    go
}

fn date_text(v: &Value) -> String {
    v.as_str().map(fmt::date).unwrap_or_default()
}

/// The day before an ISO date, as shown.
fn day_before(iso_date: &str) -> String {
    iso_date
        .parse::<mk_core::Date>()
        .ok()
        .and_then(mk_core::Date::previous_day)
        .map(|d| fmt::date(&d.to_string()))
        .unwrap_or_default()
}

// ----- the money wizard -------------------------------------------------------------------

fn money_args(app: &App, d: &MoneyWizard, feed: bool) -> Value {
    let mut a = json!({"account": d.account, "today": app.today()});
    merge(&mut a, cash_args(&d.cash, feed));
    if !d.offset.is_empty() {
        set(&mut a, "offset", json!(d.offset));
    }
    if d.go_ahead {
        set(&mut a, "go_ahead", json!(true));
    }
    if d.loan == Some(true)
        && !d.asset_name.trim().is_empty()
        && let Some(v) = amount(&d.asset_value)
    {
        set(
            &mut a,
            "asset",
            json!({"name": d.asset_name.trim(), "value": v}),
        );
    }
    a
}

fn feed_balance(plan: &Value) -> bool {
    plan.at("feed").at("balance").is_string()
}

fn money_plan(app: &mut App, d: &MoneyWizard) -> Result<Value, String> {
    let first = cached(
        app,
        "opening.plan",
        json!({"account": d.account, "today": app.today()}),
    )?;
    let feed = feed_balance(&first);
    let mut args = money_args(app, d, feed);
    if let Some(o) = args.as_object_mut() {
        o.remove("asset");
        o.remove("go_ahead");
    }
    if args.as_object().map_or(0, serde_json::Map::len) <= 2 {
        return Ok(first);
    }
    cached(app, "opening.plan", args)
}

fn money_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.opening.money.clone() else {
        return;
    };
    let mut close = ui.dialog_closed("opening-money") || ui.key("escape");
    let plan = if d.account.is_empty() {
        Err(String::new())
    } else {
        money_plan(app, &d)
    };
    let p = plan.clone().unwrap_or(Value::Null);
    let mut go: Option<usize> = None;
    let mut help = false;
    let mut save = false;
    let mut next_account: Option<String> = None;
    let name = p.at("account").str("name").to_owned();
    ui.dialog("opening-money", "Opening balance", 780, |ui| {
        if d.step < 4 {
            go = step_bar(ui, &MONEY_STEPS, d.step, d.step);
        }
        ui.heading(MONEY_STEPS.get(d.step).copied().unwrap_or(""));
        match d.step {
            0 => account_step(app, ui, &mut d, &p),
            1 => {
                if balance_step(ui, &mut d.cash, &p) {
                    go = Some(2);
                }
            }
            2 => start_step(ui, &mut d.cash, &p),
            3 => check_step(app, ui, &mut d, &p, &mut go, &mut close),
            _ => {
                next_account = done_step(app, ui, &d, &name);
            }
        }
        if let (Err(e), true) = (&plan, d.step > 0)
            && !e.is_empty()
        {
            ui.error_text(e.clone());
        }
        if let Some(e) = &d.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if d.step > 0 && d.step < 4 && ui.button("Back").clicked() {
                go = Some(d.step - 1);
            }
            help = ui.button("Help").clicked();
            ui.spacer();
            if d.step < 4 {
                close |= ui.button("Close").clicked();
            }
            match d.step {
                0 | 2 => {
                    ui.enabled(!d.account.is_empty() && plan.is_ok());
                    if ui.primary("Next").clicked() {
                        go = Some(d.step + 1);
                    }
                }
                1 => {
                    ui.enabled(p.at("ready").as_bool() == Some(true));
                    if ui.primary("Next").clicked() {
                        go = Some(2);
                    }
                }
                3 => {
                    let blocked = !p.list("reconciled").is_empty() && !d.go_ahead;
                    ui.enabled(p.at("ready").as_bool() == Some(true) && !blocked);
                    save = ui.primary("Save").clicked();
                }
                _ => {}
            }
        });
    });
    if help {
        app.opening.help_open = true;
    }
    if save {
        let args = money_args(app, &d, feed_balance(&p));
        match app.run("opening.apply", args) {
            Ok(v) => {
                d.done = Some(v);
                d.error = None;
                d.step = 4;
            }
            Err(e) => d.error = Some(e.display()),
        }
    }
    if let Some(s) = go {
        d.error = None;
        d.step = s;
    }
    if d.step == 4 && d.done.is_none() {
        d.step = 3;
    }
    app.opening.money = if close || d.step > 4 { None } else { Some(d) };
    if let Some(a) = next_account {
        app.opening.money = None;
        open_for(app, &a);
    }
}

fn account_step(app: &mut App, ui: &mut Ui, d: &mut MoneyWizard, p: &Value) {
    let rows: Vec<Value> = cached(app, "opening.checklist", json!({}))
        .map(|c| c.list("accounts").to_vec())
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.str("wizard") == "money")
        .collect();
    let names: Vec<String> = rows.iter().map(|r| r.str("full_name").to_owned()).collect();
    let mut idx = rows
        .iter()
        .position(|r| r.str("account") == d.account)
        .unwrap_or(usize::MAX);
    ui.group(|ui| {
        ui.form("Account", |ui| {
            ui.width(320);
            if ui.combo("Account", &mut idx, &names).changed()
                && let Some(r) = rows.get(idx)
            {
                *d = MoneyWizard {
                    account: r.str("account").to_owned(),
                    ..MoneyWizard::default()
                };
            }
        });
        if !p.is_null() {
            let kind = p.at("account").str("kind");
            ui.form("Kind", |ui| {
                ui.label(upper_first(kind));
            });
        }
    });
    let ex = p.at("existing");
    if ex.is_object() {
        ui.label(format!(
            "{} already has an opening balance of {} on {}. You can change it here.",
            p.at("account").str("name"),
            fmt::group(ex.str("amount")),
            date_text(ex.at("date"))
        ));
    } else if !p.is_null() {
        ui.label("This wizard works out the opening balance from one balance you trust.");
    }
    ui.secondary(REASSURE);
}

fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// The balance you trust; returns true when "Yes, use it" was pressed.
fn balance_step(ui: &mut Ui, c: &mut CashForm, p: &Value) -> bool {
    let feed = p.at("feed");
    let mut yes = false;
    if feed_balance(p) && c.use_feed != Some(false) {
        ui.row(|ui| {
            ui.label("Your bank reports");
            ui.bold();
            ui.label(fmt::group(feed.str("balance")));
            ui.label(format!(
                "on {}. Is that what your bank shows?",
                date_text(feed.at("date"))
            ));
        });
        ui.row(|ui| {
            if ui.primary("Yes, use it").clicked() {
                c.use_feed = Some(true);
                yes = true;
            }
            if ui.button("Use a different balance").clicked() {
                c.use_feed = Some(false);
            }
        });
    } else {
        let liability = p.at("account").at("liability").as_bool() == Some(true);
        ui.label(p.at("account").str("question"));
        ui.group(|ui| {
            ui.form(if liability { "Amount owed" } else { "Balance" }, |ui| {
                ui.width(160);
                ui.hint("0.00");
                ui.field("Balance you trust", &mut c.balance);
            });
            date_row(ui, "As of", &mut c.as_of);
            ui.form("", |ui| ui.secondary(BALANCE_HINT));
            if liability && p.at("account").str("subtype") == "credit-card" {
                ui.form("", |ui| {
                    ui.switch(&mut c.owes_me, "The card owes me");
                });
            }
        });
        if liability {
            ui.secondary("Type what you owe as a positive number; MoneyKeep applies the sign.");
        }
        if feed_balance(p) && ui.link("Use the balance your bank reports").clicked() {
            c.use_feed = None;
        }
    }
    if feed.is_object() {
        ui.secondary(feed.str("pending_text"));
    }
    ui.secondary(REASSURE);
    yes
}

fn start_step(ui: &mut Ui, c: &mut CashForm, p: &Value) {
    let st = p.at("start");
    let nothing = st.at("nothing").as_bool() == Some(true);
    if nothing {
        ui.label(NOTHING);
        return;
    }
    let mut statement = c.statement;
    ui.radio(&mut statement, false, "Keep everything imported");
    let n = st.at("imported").as_u64().unwrap_or(0);
    ui.secondary(format!(
        "All {n} imported transaction{} stay. Your books start on {}.",
        if n == 1 { "" } else { "s" },
        day_before(st.str("earliest"))
    ));
    ui.radio(&mut statement, true, "Start at a statement");
    ui.secondary("Your books start on the statement's ending date, from its ending balance.");
    if statement != c.statement {
        c.statement = statement;
        c.keep.clear();
    }
    if c.statement {
        ui.group(|ui| {
            date_row(ui, "Statement ending date", &mut c.stmt_date);
            ui.form("Ending balance", |ui| {
                ui.width(160);
                ui.hint("0.00");
                ui.field("Ending balance", &mut c.stmt_balance);
            });
        });
        let pre = p.list("pre_start");
        if !pre.is_empty() {
            ui.label(format!(
                "These {} transaction{} before your start date and will be removed (they are already in the starting balance):",
                pre.len(),
                if pre.len() == 1 { " is" } else { "s are" }
            ));
            for r in pre {
                let id = r.str("txn").to_owned();
                let mut removed = !c.keep.contains(&id);
                let text = format!(
                    "{}  {}  {}",
                    date_text(r.at("date")),
                    r.str("payee"),
                    fmt::group(r.str("amount"))
                );
                if ui.checkbox(&mut removed, &text).clicked() {
                    if removed {
                        c.keep.retain(|k| *k != id);
                    } else {
                        c.keep.push(id);
                    }
                }
            }
        }
    }
    ui.secondary(REASSURE);
}

/// The other side, the loan question, the check and the reconciled note.
#[allow(clippy::too_many_lines)]
fn check_step(
    app: &mut App,
    ui: &mut Ui,
    d: &mut MoneyWizard,
    p: &Value,
    go: &mut Option<usize>,
    close: &mut bool,
) {
    let o = p.at("opening");
    let name = p.at("account").str("name").to_owned();
    if o.at("zero").as_bool() == Some(true) {
        ui.label(format!(
            "The opening balance is zero: {name} started from zero. Saving marks it so."
        ));
    } else if o.is_object() {
        ui.row(|ui| {
            ui.label(format!("Opening balance on {}:", date_text(o.at("date"))));
            ui.bold();
            ui.label(fmt::group(o.str("amount")));
            ui.label(format!("in {name}, from {}.", p.at("offset").str("name")));
        });
    }
    // Other side.
    let choices = p.at("offset").list("choices");
    let current = p.at("offset").str("name").to_owned();
    ui.row(|ui| {
        ui.label(format!("Other side: {current}"));
        if ui.link("Change").clicked() {
            d.offset_open = !d.offset_open;
        }
    });
    let why = choices
        .iter()
        .find(|c| c.str("name") == current)
        .map(|c| c.str("why").to_owned())
        .unwrap_or_default();
    if d.offset_open {
        ui.secondary(OTHER_SIDE);
        let mut key = if d.offset.is_empty() {
            choices
                .iter()
                .find(|c| c.str("name") == current)
                .map(|c| c.str("key").to_owned())
                .unwrap_or_default()
        } else {
            d.offset.clone()
        };
        for c in choices {
            ui.radio(&mut key, c.str("key").to_owned(), c.str("name"));
            ui.secondary(c.str("why"));
        }
        let all: Vec<String> = array(app.read("account.list", json!({})))
            .into_iter()
            .filter(|a| a.at("placeholder").as_bool() != Some(true) && a.str("id") != d.account)
            .map(|a| a.str("full_name").to_owned())
            .collect();
        ui.form("Another account", |ui| {
            if super::form::pick(ui, "Another account", &mut d.other, &all, Some("—")) {
                key.clone_from(&d.other);
            }
        });
        if key != d.offset && !key.is_empty() {
            d.offset = key;
        }
    } else if !why.is_empty() {
        ui.secondary(why);
    }
    // Loans: for something you own?
    if p.at("loan").as_bool() == Some(true) {
        ui.label("Is this loan for something you own, such as a house or car?");
        ui.row(|ui| {
            if ui.toggle("Yes", d.loan == Some(true)).clicked() {
                d.loan = Some(true);
            }
            if ui.toggle("No", d.loan == Some(false)).clicked() {
                d.loan = Some(false);
            }
        });
        if d.loan == Some(true) {
            ui.group(|ui| {
                text_row(ui, "What it is", &mut d.asset_name, "e.g. House");
                text_row(ui, "What it is worth", &mut d.asset_value, "0.00");
            });
            ui.secondary(
                "It is added as an asset with its own opening balance, so your net worth counts both.",
            );
        }
    }
    // The check.
    let check = p.at("check");
    if check.is_object() {
        if check.at("matches").as_bool() == Some(true) {
            ui.row(|ui| {
                ui.tone(Tone::Success);
                ui.bold();
                ui.label(if p.at("feed").is_object() {
                    "Matches your bank. ✓"
                } else {
                    "Matches the balance you gave. ✓"
                });
            });
        } else {
            ui.row(|ui| {
                ui.tone(Tone::Warning);
                ui.bold();
                ui.label(format!(
                    "Your balance is {}; MoneyKeep would show {} — a difference of {}.",
                    fmt::group(check.str("trusted")),
                    fmt::group(check.str("balance")),
                    fmt::group(check.str("difference"))
                ));
            });
            ui.label("Likely causes:");
            for c in p.list("causes") {
                if ui.link(format!("• {}", c.str("text"))).clicked() {
                    cause(app, d, c, go, close);
                }
            }
        }
    }
    // The reconciled note.
    if let Some(r) = p.list("reconciled").first() {
        ui.banner(Tone::Warning, |ui| {
            ui.label(format!(
                "Your reconciliation of {} ended at {}. This change would make that period not add up. You can choose an earlier start date, or go ahead and the reconciliation will be marked to check.",
                date_text(r.at("date")),
                fmt::group(r.str("ending"))
            ));
        });
        ui.row(|ui| {
            if ui.toggle("Go ahead", d.go_ahead).clicked() {
                d.go_ahead = true;
            }
            if ui.button("Pick another date").clicked() {
                d.go_ahead = false;
                *go = Some(2);
            }
        });
    }
    ui.secondary(REASSURE);
}

/// A mismatch helper link.
fn cause(app: &mut App, d: &mut MoneyWizard, c: &Value, go: &mut Option<usize>, close: &mut bool) {
    match c.str("kind") {
        "pending" => {
            app.config.settings_section = "Connections".into();
            app.navigate(View::Settings);
            *close = true;
        }
        "duplicates" | "after" => {
            app.navigate(View::Register {
                account: d.account.clone(),
            });
            *close = true;
        }
        "kept" => *go = Some(2),
        "price" => {
            app.navigate(View::Securities);
            *close = true;
        }
        _ => {
            d.cash.use_feed = Some(false);
            *go = Some(1);
        }
    }
}

/// "Checking is set up. ✓" and the next account; returns the account to set up next.
fn done_step(app: &mut App, ui: &mut Ui, d: &MoneyWizard, name: &str) -> Option<String> {
    let shown = d
        .done
        .as_ref()
        .map(|v| v.str("full_name").to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| name.to_owned());
    let short = shown.rsplit(':').next().unwrap_or(&shown).to_owned();
    ui.row(|ui| {
        ui.tone(Tone::Success);
        ui.bold();
        ui.label(format!("{short} is set up. ✓"));
    });
    if let Some(a) = d
        .done
        .as_ref()
        .map(|v| v.at("asset"))
        .filter(|a| a.is_object())
    {
        ui.label(format!(
            "{} was added with an opening balance of {}.",
            a.str("name"),
            fmt::group(a.str("amount"))
        ));
    }
    let id = d
        .done
        .as_ref()
        .map(|v| v.str("account").to_owned())
        .unwrap_or_default();
    next_step(app, ui, &id)
}

/// The checklist's next account after `id`, with Set up … / Later.
fn next_step(app: &mut App, ui: &mut Ui, id: &str) -> Option<String> {
    let mut out = None;
    let c = cached(app, "opening.checklist", json!({})).unwrap_or(Value::Null);
    let next = c
        .list("accounts")
        .iter()
        .find(|r| r.str("status") == "not_set" && r.str("account") != id);
    if let Some(n) = next {
        ui.label(format!(
            "Next: {} — {} of {} accounts done",
            n.str("name"),
            c.at("done"),
            c.at("total")
        ));
        ui.row(|ui| {
            if ui.primary(format!("Set up {}", n.str("name"))).clicked() {
                out = Some(n.str("account").to_owned());
            }
            if ui.button("Later").clicked() {
                out = Some(String::new());
            }
        });
    } else {
        if let Some(e) = c.at("encouragement").as_str() {
            ui.label(e);
        }
        if ui.row(|ui| ui.primary("Done").clicked()) {
            out = Some(String::new());
        }
    }
    out
}

// ----- the investment wizard ---------------------------------------------------------------

fn holding_arg(h: &Holding) -> Option<Value> {
    let symbol = h.symbol.trim().to_uppercase();
    let shares = amount(&h.shares)?;
    if symbol.is_empty() {
        return None;
    }
    let mut v = json!({"symbol": symbol, "shares": shares});
    if !h.name.trim().is_empty() {
        set(&mut v, "name", json!(h.name.trim()));
    }
    if let Some(k) = SEC_KINDS.get(h.kind) {
        set(&mut v, "kind", json!(k.0));
        if k.0 == "bond" && !h.maturity.trim().is_empty() {
            set(
                &mut v,
                "maturity",
                json!(fmt::date_arg_lenient(&h.maturity)),
            );
            if let Some(c) = amount(&h.coupon) {
                set(&mut v, "coupon_rate", json!(c));
            }
        }
    }
    if let Some(p) = amount(&h.price) {
        set(&mut v, "price", json!(p));
    }
    let basis = match h.level.as_str() {
        "unknown" => Some(json!({"level": "unknown"})),
        "total" => {
            let mut b = json!({"level": "total"});
            if let Some(t) = amount(&h.total) {
                set(
                    &mut b,
                    if h.per_share { "per_share" } else { "total" },
                    json!(t),
                );
            }
            if let Some(d) = iso(&h.acquired) {
                set(&mut b, "acquired", json!(d));
            }
            Some(b)
        }
        "lots" => {
            let lots: Vec<Value> = h
                .lots
                .iter()
                .filter_map(|(d, s, c)| {
                    Some(json!({"acquired": iso(d)?, "shares": amount(s)?, "cost": amount(c)?}))
                })
                .collect();
            Some(json!({"level": "lots", "lots": lots}))
        }
        _ => None,
    };
    if let Some(b) = basis {
        set(&mut v, "basis", b);
    }
    Some(v)
}

fn inv_args(app: &App, d: &InvWizard, feed: bool) -> Value {
    let rows: Vec<Value> = d.rows.iter().filter_map(holding_arg).collect();
    let mut a = json!({"account": d.account, "today": app.today(), "holdings": rows,
        "cash": cash_args(&d.cash, feed), "retirement": d.retirement});
    if d.enter_basis {
        set(&mut a, "enter_basis", json!(true));
    }
    if let Some(t) = amount(&d.broker_total) {
        set(&mut a, "broker_total", json!(t));
    }
    if !d.offset.is_empty() {
        set(&mut a, "offset", json!(d.offset));
    }
    if d.go_ahead {
        set(&mut a, "go_ahead", json!(true));
    }
    a
}

fn inv_plan(app: &mut App, d: &InvWizard) -> Result<Value, String> {
    let first = cached(
        app,
        "opening.inv_plan",
        json!({"account": d.account, "today": app.today()}),
    )?;
    let feed = feed_balance(&first);
    let args = inv_args(app, d, feed);
    cached(app, "opening.inv_plan", args)
}

/// Fill the holdings from the provider (SnapTrade), the opening holdings already
/// posted, or the book's holdings today.
fn prefill(app: &mut App, d: &mut InvWizard, p: &Value) {
    d.prefilled = true;
    let existing = p.list("existing_holdings");
    let today: Vec<Value> = app
        .read(
            "inv.holdings",
            json!({"accounts": [d.account], "as_of": app.today()}),
        )
        .map(|h| {
            h.at("accounts")
                .as_array()
                .and_then(|a| a.first())
                .map(|a| a.list("holdings").to_vec())
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let mut rows: Vec<Holding> = Vec::new();
    for f in p.list("feed_holdings") {
        rows.push(Holding {
            symbol: f.str("symbol").to_owned(),
            shares: f.str("shares").to_owned(),
            ..Holding::default()
        });
    }
    if rows.is_empty() {
        for t in &today {
            rows.push(Holding {
                symbol: t.str("symbol").to_owned(),
                name: t.str("name").to_owned(),
                shares: t.str("quantity").to_owned(),
                ..Holding::default()
            });
        }
    }
    for h in &mut rows {
        let Some(e) = existing.iter().find(|e| e.str("symbol") == h.symbol) else {
            continue;
        };
        let lots = e.list("lots");
        if lots
            .iter()
            .any(|l| l.at("basis_needed").as_bool() == Some(true))
        {
            h.level = "unknown".into();
        } else if lots.len() == 1 {
            let l = lots.first().cloned().unwrap_or(Value::Null);
            h.level = "total".into();
            h.total = l.str("cost").to_owned();
            if l.at("various").as_bool() != Some(true) {
                h.acquired = date_text(l.at("acquired"));
            }
        } else if !lots.is_empty() {
            h.level = "lots".into();
            h.lots = lots
                .iter()
                .map(|l| {
                    (
                        date_text(l.at("acquired")),
                        l.str("shares").to_owned(),
                        l.str("cost").to_owned(),
                    )
                })
                .collect();
        }
    }
    d.rows = rows;
    if let Some(c) = p.at("feed_cash").as_str() {
        d.cash.balance = c.to_owned();
    }
}

#[allow(clippy::too_many_lines)]
fn inv_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.opening.inv.clone() else {
        return;
    };
    let mut close = ui.dialog_closed("opening-inv") || ui.key("escape");
    let plan = inv_plan(app, &d);
    let p = plan.clone().unwrap_or(Value::Null);
    if !d.prefilled && !p.is_null() {
        prefill(app, &mut d, &p);
    }
    let basis_step = !d.retirement || d.enter_basis;
    let mut go: Option<usize> = None;
    let mut help = false;
    let mut save = false;
    let mut read_paste = false;
    let mut create_unknown = false;
    let mut next_account = None;
    ui.dialog("opening-inv", "Investment account opening balance", 860, |ui| {
        if d.step < 5 {
            go = step_bar(ui, &INV_STEPS, d.step, d.step);
        }
        ui.heading(INV_STEPS.get(d.step).copied().unwrap_or(""));
        match d.step {
            0 => {
                ui.label(format!(
                    "Set up {}: its cash, the holdings on your start date and their cost basis.",
                    p.at("account").str("full_name")
                ));
                let before = d.retirement;
                ui.switch(
                    &mut d.retirement,
                    "This is a retirement account (IRA, 401(k), Roth and similar)",
                );
                if d.retirement != before {
                    d.retirement_set = true;
                }
                if d.retirement {
                    ui.secondary("Holdings in a retirement account are recorded at market value: no cost basis is needed.");
                }
                let ex = p.at("existing");
                if ex.is_object() || !p.list("existing_holdings").is_empty() {
                    ui.label("This account already has opening balances. You can change them here.");
                }
                ui.secondary(REASSURE);
            }
            1 => {
                if p.at("feed_cash").is_string() && d.cash.use_feed != Some(false) {
                    ui.row(|ui| {
                        ui.label("Your broker reports cash of");
                        ui.bold();
                        ui.label(fmt::group(p.str("feed_cash")));
                        ui.label(". Use a different amount if your broker shows another.");
                    });
                    if ui.link("Use a different amount").clicked() {
                        d.cash.use_feed = Some(false);
                    }
                    if d.cash.balance.is_empty() {
                        d.cash.balance = p.str("feed_cash").to_owned();
                    }
                } else {
                    if balance_step(ui, &mut d.cash, &p) {
                        go = Some(1);
                    }
                    if d.cash.balance.trim().is_empty() {
                        ui.secondary(format!(
                            "Left empty, the cash stays as MoneyKeep shows it today: {}.",
                            fmt::group(p.at("trusted").str("amount"))
                        ));
                    }
                }
                ui.separator();
                start_step(ui, &mut d.cash, &p);
            }
            2 => holdings_step(ui, &mut d, &p, &mut read_paste, &mut create_unknown),
            3 => basis_form(ui, &mut d, &p),
            4 => inv_check(app, ui, &mut d, &p, &mut go, &mut close),
            _ => {
                ui.row(|ui| {
                    ui.tone(Tone::Success);
                    ui.bold();
                    ui.label(format!("{} is set up. ✓", p.at("account").str("name")));
                });
                let n = d
                    .done
                    .as_ref()
                    .and_then(|v| v.at("basis_needed").as_u64())
                    .unwrap_or(0);
                if n > 0 {
                    ui.label(format!(
                        "{} — you can add it later from the checklist.",
                        if n == 1 {
                            "1 holding needs cost basis".to_owned()
                        } else {
                            format!("{n} holdings need cost basis")
                        }
                    ));
                    ui.secondary(p.str("basis_where"));
                }
                let id = d.account.clone();
                next_account = next_step(app, ui, &id);
            }
        }
        if let (Err(e), true) = (&plan, d.step > 0) {
            ui.error_text(e.clone());
        }
        if let Some(e) = &d.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if d.step > 0 && d.step < 5 && ui.button("Back").clicked() {
                go = Some(if d.step == 4 && !basis_step { 2 } else { d.step - 1 });
            }
            help = ui.button("Help").clicked();
            ui.spacer();
            if d.step < 5 {
                close |= ui.button("Close").clicked();
            }
            if d.step < 4 {
                ui.enabled(plan.is_ok());
                if ui.primary("Next").clicked() {
                    go = Some(if d.step == 2 && !basis_step { 4 } else { d.step + 1 });
                }
            } else if d.step == 4 {
                let blocked = !p.list("reconciled").is_empty() && !d.go_ahead;
                ui.enabled(p.at("ready").as_bool() == Some(true) && !blocked);
                save = ui.primary("Save").clicked();
            }
        });
    });
    if help {
        app.opening.help_open = true;
    }
    if read_paste {
        match app.read_or_error("opening.inv_paste", json!({"text": d.paste})) {
            Ok(v) => {
                for r in v.list("rows") {
                    let symbol = r.str("symbol").to_owned();
                    let row = Holding {
                        symbol: symbol.clone(),
                        name: r.str("name").to_owned(),
                        shares: r.str("shares").to_owned(),
                        level: if r.at("cost").is_string() {
                            "total".into()
                        } else {
                            String::new()
                        },
                        total: r.str("cost").to_owned(),
                        ..Holding::default()
                    };
                    match d.rows.iter_mut().find(|h| h.symbol == symbol) {
                        Some(h) => *h = row,
                        None => d.rows.push(row),
                    }
                }
                d.unknown = v
                    .list("unknown")
                    .iter()
                    .filter_map(|u| u.as_str().map(str::to_owned))
                    .collect();
                d.paste.clear();
                d.paste_open = false;
            }
            Err(e) => d.error = Some(e),
        }
    }
    if create_unknown {
        for sym in std::mem::take(&mut d.unknown) {
            let kind = d
                .rows
                .iter()
                .find(|h| h.symbol == sym)
                .and_then(|h| SEC_KINDS.get(h.kind))
                .map_or("stock", |k| k.0);
            if let Err(e) = app.run(
                "security.create",
                json!({"symbol": sym, "name": sym, "kind": kind}),
            ) {
                d.error = Some(e.display());
            }
        }
    }
    if save {
        let args = inv_args(app, &d, feed_balance(&p));
        match app.run("opening.inv_apply", args) {
            Ok(v) => {
                d.done = Some(v);
                d.error = None;
                d.step = 5;
            }
            Err(e) => d.error = Some(e.display()),
        }
    }
    if let Some(s) = go {
        d.error = None;
        d.step = s;
    }
    app.opening.inv = if close { None } else { Some(d) };
    if let Some(a) = next_account {
        app.opening.inv = None;
        if !a.is_empty() {
            open_for(app, &a);
        }
    }
}

fn holdings_step(
    ui: &mut Ui,
    d: &mut InvWizard,
    p: &Value,
    read_paste: &mut bool,
    create_unknown: &mut bool,
) {
    if p.list("feed_holdings").is_empty() {
        ui.label(
            "Type each holding with the shares you hold today, or paste your broker's positions.",
        );
    } else {
        ui.label("These holdings come from your broker. Check them and press Next.");
    }
    let planned = p.list("holdings");
    let mut remove = None;
    ui.row(|ui| {
        for (t, w) in [
            ("Symbol", 110),
            ("Name", 230),
            ("Shares today", 120),
            ("On your start date", 150),
        ] {
            ui.width(w);
            ui.caption(t);
        }
    });
    for (i, h) in d.rows.iter_mut().enumerate() {
        let plan = planned
            .iter()
            .find(|x| x.str("symbol").eq_ignore_ascii_case(h.symbol.trim()))
            .cloned()
            .unwrap_or(Value::Null);
        ui.scope(format!("holding{i}"), |ui| {
            ui.row(|ui| {
                ui.width(110);
                ui.field("Symbol", &mut h.symbol);
                ui.width(230);
                if plan.at("known").as_bool() == Some(true) {
                    ui.label(plan.str("name"));
                } else {
                    ui.hint("Name (new security)");
                    ui.field("Name", &mut h.name);
                }
                ui.width(120);
                ui.field("Shares today", &mut h.shares);
                ui.width(150);
                if plan.at("differs").as_bool() == Some(true) {
                    ui.bold();
                }
                ui.label(plan.str("on_start"));
                if ui.icon_button("close", "Remove holding", false).clicked() {
                    remove = Some(i);
                }
            });
            if plan.is_object()
                && plan.at("known").as_bool() != Some(true)
                && !h.symbol.trim().is_empty()
            {
                ui.row(|ui| {
                    ui.secondary("Not in your book yet; kind:");
                    let labels: Vec<String> = SEC_KINDS.iter().map(|k| k.1.to_owned()).collect();
                    ui.width(160);
                    ui.combo("Security kind", &mut h.kind, &labels);
                });
                if SEC_KINDS.get(h.kind).is_some_and(|k| k.0 == "bond") {
                    ui.row(|ui| {
                        ui.secondary("Matures");
                        ui.width(130);
                        super::register::date_field(ui, "Bond maturity", &mut h.maturity);
                        ui.secondary("Coupon %");
                        ui.width(80);
                        ui.field("Bond coupon", &mut h.coupon);
                    });
                }
            }
            if let Some(problem) = plan.at("problem").as_str() {
                ui.error_text(problem);
            }
        });
    }
    if let Some(i) = remove {
        d.rows.remove(i);
    }
    if planned
        .iter()
        .any(|h| h.at("differs").as_bool() == Some(true))
    {
        ui.secondary("Shares on your start date leave out the purchases, sales and splits recorded after it.");
    }
    ui.row(|ui| {
        if ui.button("Add a holding").clicked() {
            d.rows.push(Holding::default());
        }
        if ui.button("Paste…").clicked() {
            d.paste_open = !d.paste_open;
        }
    });
    if d.paste_open {
        ui.secondary("One line per holding: symbol, shares and (optionally) the total cost — from your broker's positions page or a CSV.");
        ui.next().grow();
        ui.multiline("Pasted positions", &mut d.paste);
        if ui.row(|ui| ui.button("Read pasted lines").clicked()) {
            *read_paste = true;
        }
    }
    if !d.unknown.is_empty() {
        ui.banner(Tone::Warning, |ui| {
            ui.label(format!(
                "Not in your book yet: {}. Create {}?",
                d.unknown.join(", "),
                if d.unknown.len() == 1 { "it" } else { "them" }
            ));
        });
        ui.row(|ui| {
            if ui.primary("Create securities").clicked() {
                *create_unknown = true;
            }
            if ui.button("Not now").clicked() {
                d.unknown.clear();
            }
        });
    }
    ui.secondary("Options: type the symbol like AAPL 261218 C200 and the contracts (a minus sign for contracts you wrote). Bonds: the number of bonds and the price in percent of face. Crypto: up to 8 decimals.");
}

fn basis_form(ui: &mut Ui, d: &mut InvWizard, p: &Value) {
    if d.retirement {
        ui.switch(&mut d.enter_basis, "Enter cost basis anyway");
        ui.secondary("Useful for after-tax contributions.");
    }
    ui.label("For each holding, give what you know — nobody has to stop here.");
    let planned = p.list("holdings");
    for (i, h) in d.rows.iter_mut().enumerate() {
        if h.symbol.trim().is_empty() {
            continue;
        }
        let plan = planned
            .iter()
            .find(|x| x.str("symbol").eq_ignore_ascii_case(h.symbol.trim()))
            .cloned()
            .unwrap_or(Value::Null);
        ui.scope(format!("basis{i}"), |ui| {
            ui.card(
                &format!(
                    "{} — {} shares on your start date",
                    h.symbol.trim().to_uppercase(),
                    plan.str("on_start")
                ),
                |ui| {
                    ui.wrap(|ui| {
                        ui.radio(&mut h.level, "lots".to_owned(), "Exact lots");
                        ui.radio(&mut h.level, "total".to_owned(), "One total");
                        ui.radio(&mut h.level, "unknown".to_owned(), "I don't know yet");
                    });
                    match h.level.as_str() {
                        "lots" => {
                            if h.lots.is_empty() {
                                h.lots.push((
                                    String::new(),
                                    plan.str("on_start").to_owned(),
                                    String::new(),
                                ));
                            }
                            let mut drop = None;
                            for (j, (date, shares, cost)) in h.lots.iter_mut().enumerate() {
                                ui.scope(format!("lot{j}"), |ui| {
                                    ui.row(|ui| {
                                        ui.caption("Bought");
                                        super::register::date_field(ui, "Purchase date", date);
                                        ui.caption("Shares");
                                        ui.width(100);
                                        ui.field("Lot shares", shares);
                                        ui.caption("Cost");
                                        ui.width(120);
                                        ui.field("Lot cost", cost);
                                        if ui.icon_button("close", "Remove lot", false).clicked() {
                                            drop = Some(j);
                                        }
                                    });
                                });
                            }
                            if let Some(j) = drop {
                                h.lots.remove(j);
                            }
                            if ui.row(|ui| ui.button("Add a lot").clicked()) {
                                h.lots.push(Default::default());
                            }
                            ui.secondary("The lots must add up to the shares on your start date.");
                        }
                        "total" => {
                            ui.row(|ui| {
                                ui.width(140);
                                ui.hint("0.00");
                                ui.field(
                                    if h.per_share {
                                        "Average cost per share"
                                    } else {
                                        "Total cost"
                                    },
                                    &mut h.total,
                                );
                                ui.switch(&mut h.per_share, "Average cost per share");
                            });
                            ui.row(|ui| {
                                ui.caption("Bought on (empty for Various)");
                                super::register::date_field(ui, "Bought on", &mut h.acquired);
                            });
                            if plan.at("basis").is_string() {
                                ui.secondary(format!(
                                    "Total cost: {}",
                                    fmt::group(plan.str("basis"))
                                ));
                            }
                        }
                        "unknown" => {
                            if plan.at("price_start").is_null() {
                                ui.row(|ui| {
                                    ui.label("No price on your start date; type it:");
                                    ui.width(120);
                                    ui.field("Price on the start date", &mut h.price);
                                });
                            } else if plan.at("basis").is_string() {
                                ui.secondary(format!(
                                    "Recorded at its market value, {}, and marked Basis needed.",
                                    fmt::group(plan.str("basis"))
                                ));
                            }
                        }
                        _ => {}
                    }
                    if let Some(problem) = plan.at("problem").as_str() {
                        ui.error_text(problem);
                    }
                },
            );
        });
    }
    ui.secondary(format!("Where to find it: {}", p.str("basis_where")));
}

fn inv_check(
    app: &mut App,
    ui: &mut Ui,
    d: &mut InvWizard,
    p: &Value,
    go: &mut Option<usize>,
    close: &mut bool,
) {
    let o = p.at("opening");
    if o.is_object() {
        ui.label(format!(
            "Your books start on {}, with cash of {} from {}.",
            date_text(o.at("date")),
            fmt::group(o.str("amount")),
            p.at("offset").str("name")
        ));
    }
    let rows = p.list("holdings");
    for h in rows {
        let price = h.at("price");
        ui.row(|ui| {
            ui.width(110);
            ui.bold();
            ui.label(h.str("symbol"));
            ui.width(120);
            ui.label(format!("{} shares", h.str("today")));
            ui.width(220);
            if price.is_object() {
                ui.label(format!(
                    "at {} ({})",
                    fmt::group(price.str("price")),
                    date_text(price.at("date"))
                ));
            } else {
                ui.label("no price");
            }
            ui.width(120);
            ui.amount(fmt::group(h.str("value")));
            if h.at("stale").as_bool() == Some(true) {
                ui.tone(Tone::Warning);
                ui.label("price out of date");
            }
        });
    }
    let t = p.at("total");
    for (label, key) in [("Cash", "cash"), ("Total", "total")] {
        ui.row(|ui| {
            ui.width(110);
            if key == "total" {
                ui.bold();
            }
            ui.label(label);
            ui.width(120);
            ui.label("");
            ui.width(220);
            ui.label("");
            ui.width(120);
            if key == "total" {
                ui.bold();
            }
            ui.amount(fmt::group(t.str(key)));
        });
    }
    ui.group(|ui| {
        ui.form("Total your broker shows", |ui| {
            ui.width(160);
            ui.hint("0.00");
            ui.field("Total your broker shows", &mut d.broker_total);
        });
    });
    if t.at("broker").is_string() {
        if t.at("matches").as_bool() == Some(true) {
            ui.row(|ui| {
                ui.tone(Tone::Success);
                ui.bold();
                ui.label("Matches your broker. ✓");
            });
        } else {
            ui.row(|ui| {
                ui.tone(Tone::Warning);
                ui.bold();
                ui.label(format!(
                    "A difference of {}. Compare each holding's value with your broker's.",
                    fmt::group(t.str("difference"))
                ));
            });
            ui.label("Likely causes:");
            let mut m = MoneyWizard {
                account: d.account.clone(),
                ..MoneyWizard::default()
            };
            for c in p.list("inv_causes") {
                if ui.link(format!("• {}", c.str("text"))).clicked() {
                    cause(app, &mut m, c, go, close);
                    if *go == Some(2) || *go == Some(1) {
                        *go = Some(1);
                    }
                }
            }
        }
    }
    if let Some(r) = p.list("reconciled").first() {
        ui.banner(Tone::Warning, |ui| {
            ui.label(format!(
                "Your reconciliation of {} ended at {}. This change would make that period not add up. You can choose an earlier start date, or go ahead and the reconciliation will be marked to check.",
                date_text(r.at("date")),
                fmt::group(r.str("ending"))
            ));
        });
        ui.row(|ui| {
            if ui.toggle("Go ahead", d.go_ahead).clicked() {
                d.go_ahead = true;
            }
            if ui.button("Pick another date").clicked() {
                d.go_ahead = false;
                *go = Some(1);
            }
        });
    }
    ui.secondary(REASSURE);
}

// ----- the checklist ------------------------------------------------------------------------

fn status_text(r: &Value) -> String {
    match r.str("status") {
        "done" if r.at("amount").is_string() => format!(
            "Done ({} on {})",
            fmt::group(r.str("amount")),
            date_text(r.at("date"))
        ),
        "done" => format!("Done ({})", date_text(r.at("date"))),
        "basis_needed" => "Basis needed".to_owned(),
        "zero" => "Started from zero".to_owned(),
        _ => "Not set".to_owned(),
    }
}

fn checklist_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.opening.checklist.clone() else {
        return;
    };
    let mut close = ui.dialog_closed("opening-checklist") || ui.key("escape");
    let args = match &d.only {
        Some(o) => json!({"accounts": o}),
        None => json!({}),
    };
    let c = cached(app, "opening.checklist", args).unwrap_or(Value::Null);
    let mut open: Option<String> = None;
    let mut zero: Option<(String, bool)> = None;
    let mut basis: Option<(String, String)> = None;
    let mut help = false;
    ui.dialog("opening-checklist", "Set up opening balances", 820, |ui| {
        ui.label(
            "Each account starts from the balance it had when you began using MoneyKeep. Set it up from one balance you trust.",
        );
        let text = c.str("progress").to_owned();
        ui.width(760);
        ui.progress(
            &format!("Progress: {text}"),
            i32::try_from(c.at("percent").as_u64().unwrap_or(0)).unwrap_or(0),
        );
        ui.row(|ui| {
            ui.bold();
            ui.label(text.clone());
            if let Some(e) = c.at("encouragement").as_str() {
                ui.tone(Tone::Success);
                ui.label(e);
            }
        });
        if let Some(b) = c.at("basis_text").as_str() {
            ui.row(|ui| {
                ui.tone(Tone::Warning);
                ui.bold();
                ui.label(b);
            });
            ui.secondary(format!("Where to find it: {}", c.str("basis_where")));
        }
        ui.separator();
        for r in c.list("accounts") {
            let id = r.str("account").to_owned();
            let name = r.str("full_name").to_owned();
            let status = r.str("status").to_owned();
            ui.scope(id.clone(), |ui| {
                ui.row(|ui| {
                    ui.width(230);
                    ui.label(name.clone());
                    ui.width(140);
                    ui.secondary(upper_first(r.str("kind")));
                    ui.width(250);
                    ui.named(format!("Status of {name}: {}", status_text(r)));
                    match status.as_str() {
                        "done" => ui.tone(Tone::Success),
                        "basis_needed" => ui.tone(Tone::Warning),
                        _ => ui.tone(Tone::Secondary),
                    };
                    ui.label(status_text(r));
                    ui.spacer();
                    let label = if matches!(status.as_str(), "not_set" | "zero") {
                        "Set up"
                    } else {
                        "Change"
                    };
                    ui.named(format!("{label} {name}"));
                    if ui.button(label).clicked() {
                        open = Some(id.clone());
                    }
                    let entry = if status == "zero" {
                        "Undo \"started from zero\""
                    } else {
                        "This account started from zero"
                    };
                    let entries = vec![entry.to_owned()];
                    ui.named(format!("More for {name}"));
                    if ui.menu("…", &entries).chosen.is_some() {
                        zero = Some((id.clone(), status != "zero"));
                    }
                });
                for sym in r.list("basis_needed") {
                    let sym = sym.as_str().unwrap_or("").to_owned();
                    ui.row(|ui| {
                        ui.width(280);
                        ui.secondary(format!("{sym}: cost basis not entered"));
                        if ui.link(format!("Enter cost basis for {sym}")).clicked() {
                            basis = Some((id.clone(), sym.clone()));
                        }
                    });
                }
            });
        }
        if let Some(e) = &d.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            help = ui.button("Help").clicked();
            ui.spacer();
            close |= ui.primary("Close").clicked();
        });
    });
    if help {
        app.opening.help_open = true;
    }
    if let Some((id, on)) = zero {
        d.error = app
            .run(
                "opening.started_from_zero",
                json!({"account": id, "on": on}),
            )
            .err()
            .map(|e| e.display());
    }
    app.opening.checklist = if close { None } else { Some(d) };
    if let Some(id) = open {
        open_for(app, &id);
    }
    if let Some((account, symbol)) = basis {
        app.opening.basis = Some(BasisDialog {
            account,
            symbol,
            ..BasisDialog::default()
        });
    }
}

fn basis_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.opening.basis.clone() else {
        return;
    };
    let mut close = ui.dialog_closed("opening-basis") || ui.key("escape");
    let mut save = false;
    ui.dialog(
        "opening-basis",
        &format!("Cost basis of {}", d.symbol),
        560,
        |ui| {
            ui.label("Give the total you paid (or the average cost per share). The placeholder at market value is replaced and the difference goes to Opening Balances.");
            ui.group(|ui| {
                ui.form(if d.per_share { "Average cost per share" } else { "Total cost" }, |ui| {
                    ui.width(160);
                    ui.hint("0.00");
                    ui.field("Cost", &mut d.total);
                });
                ui.form("", |ui| {
                    ui.switch(&mut d.per_share, "Average cost per share");
                });
                date_row(ui, "Bought on (empty for Various)", &mut d.acquired);
            });
            ui.secondary(format!(
                "Where to find it: {}",
                "Your broker's cost basis or unrealized gain page, or last year's 1099-B."
            ));
            if let Some(e) = &d.error {
                ui.error_text(e.clone());
            }
            ui.actions(|ui| {
                ui.spacer();
                close |= ui.button("Cancel").clicked();
                save = ui.primary("Save").clicked();
            });
        },
    );
    if save {
        let mut b = json!({"level": "total"});
        match amount(&d.total) {
            Some(t) => set(
                &mut b,
                if d.per_share { "per_share" } else { "total" },
                json!(t),
            ),
            None => {
                d.error = Some("Type the cost.".into());
                app.opening.basis = Some(d);
                return;
            }
        }
        if let Some(a) = iso(&d.acquired) {
            set(&mut b, "acquired", json!(a));
        }
        match app.run(
            "opening.set_basis",
            json!({"account": d.account, "security": d.symbol, "basis": b}),
        ) {
            Ok(_) => close = true,
            Err(e) => d.error = Some(e.display()),
        }
    }
    app.opening.basis = if close { None } else { Some(d) };
}

fn help_dialog(app: &mut App, ui: &mut Ui) {
    let mut close = ui.dialog_closed("opening-help") || ui.key("escape");
    ui.dialog("opening-help", "Opening balances", 820, |ui| {
        for (kind, text) in super::capture_wifi::help_lines(HELP) {
            match kind {
                "h1" => ui.big_heading(text),
                "h2" => ui.heading(text),
                "h3" => {
                    ui.bold();
                    ui.label(text);
                }
                "item" => ui.label(format!("  {text}")),
                _ => ui.label(text),
            }
        }
        ui.actions(|ui| {
            ui.spacer();
            if ui.primary("Close help").clicked() {
                close = true;
            }
        });
    });
    if close {
        app.opening.help_open = false;
    }
}

/// Holdings screen and checklist: "2 holdings need cost basis" with where to find it.
pub(crate) fn basis_line(app: &mut App, ui: &mut Ui, holdings: &Value) {
    let Some(text) = holdings.at("basis_text").as_str() else {
        return;
    };
    ui.row(|ui| {
        ui.tone(Tone::Warning);
        ui.bold();
        ui.label(text);
        if ui.link("Enter cost basis…").clicked() {
            open_checklist(app, None);
        }
    });
    ui.secondary(format!("Where to find it: {}", holdings.str("basis_where")));
}

impl App {
    /// Open an opening balance dialog at a step (review aid and screenshots):
    /// `checklist`, `money|<account>|<step>[|<balance>]` or
    /// `inv|<account>|<step>[|SYMBOL=shares,…]`.
    /// Steps count from 0; the money wizard's Done step (4) saves first.
    pub fn open_opening_review(&mut self, spec: &str) {
        let parts: Vec<&str> = spec.split('|').collect();
        let step = parts
            .get(2)
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        match parts.first().copied() {
            Some("checklist") => open_checklist(self, None),
            Some("money") => {
                open_for(self, parts.get(1).copied().unwrap_or(""));
                let Some(mut d) = self.opening.money.take() else {
                    return;
                };
                if let Some(b) = parts.get(3) {
                    d.cash.balance = (*b).to_owned();
                    d.cash.use_feed = Some(false);
                } else if step >= 2 {
                    d.cash.use_feed = Some(true);
                }
                d.step = step.min(3);
                if step >= 4 {
                    let p = money_plan(self, &d).unwrap_or(Value::Null);
                    let args = money_args(self, &d, feed_balance(&p));
                    if let Ok(v) = self.run("opening.apply", args) {
                        d.done = Some(v);
                        d.step = 4;
                    }
                }
                self.opening.money = Some(d);
            }
            Some("inv") => {
                open_for(self, parts.get(1).copied().unwrap_or(""));
                let Some(mut d) = self.opening.inv.take() else {
                    return;
                };
                let p = inv_plan(self, &d).unwrap_or(Value::Null);
                prefill(self, &mut d, &p);
                // Typed rows: `SYMBOL=shares,…`.
                for pair in parts.get(3).copied().unwrap_or("").split(',') {
                    if let Some((sym, shares)) = pair.split_once('=') {
                        d.rows.push(Holding {
                            symbol: sym.to_owned(),
                            shares: shares.to_owned(),
                            ..Holding::default()
                        });
                    }
                }
                if step >= 3 {
                    for (i, h) in d.rows.iter_mut().enumerate() {
                        if i == 0 {
                            h.level = "lots".into();
                            h.lots = vec![
                                (fmt::date("2020-01-10"), "60".into(), "9000.00".into()),
                                (fmt::date("2021-05-03"), "40".into(), "8000.00".into()),
                            ];
                        } else {
                            h.level = "unknown".into();
                        }
                    }
                }
                d.step = step.min(4);
                self.opening.inv = Some(d);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn help_page_covers_the_spec_topics() {
        let lines = super::super::capture_wifi::help_lines(HELP);
        let heads: Vec<String> = lines
            .iter()
            .filter(|(k, _)| *k == "h2")
            .map(|(_, t)| t.clone())
            .collect();
        for h in [
            "Working backwards",
            "Choosing a start date",
            "The other side",
            "Nothing is locked",
        ] {
            assert!(heads.iter().any(|x| x == h), "{heads:?}");
        }
    }

    #[test]
    fn menu_entries_by_kind() {
        let a = |t: &str, s: &str| json!({"type": t, "subtype": s});
        assert_eq!(
            menu_entry(&a("asset", "bank")),
            Some("Set opening balance…")
        );
        assert_eq!(
            menu_entry(&a("asset", "brokerage")),
            Some("Set opening holdings…")
        );
        assert_eq!(menu_entry(&a("expense", "expense")), None);
        assert_eq!(menu_entry(&a("asset", "security")), None);
    }
}
