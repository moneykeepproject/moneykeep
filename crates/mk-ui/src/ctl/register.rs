//! The register (app-desktop.md revisions 2c, 8, 9, 20, 24, 27): one account's
//! transactions with running balances in a windowed table (only the rows in
//! view are loaded), scheduled rows, the row menu, keys, and the totals line.
//! Balances always come from `account.register`.

use std::collections::HashMap;

use mk_core::{Date, Decimal};
use serde_json::{Value, json};

use super::App;
use super::popup;
use super::ui::{Align, Column, Tone, Ui};
use super::util::{Json, array};
use crate::View;
use crate::fmt;

const PAGE: usize = 200;
const FILTER_LIMIT: usize = 5000;
/// Rows built around the first visible one.
const WINDOW_BEFORE: usize = 20;
const WINDOW_AFTER: usize = 60;
/// Register row height in pixels.
pub(crate) const ROW_HEIGHT: i32 = 36;

#[derive(Debug, Default)]
pub(crate) struct RegisterState {
    pub account: String,
    key: String,
    pub header: Value,
    pages: HashMap<usize, Vec<Value>>,
    filtered: Option<Vec<Value>>,
    pub accounts: Vec<Value>,
    pub filter: String,
    pub from: String,
    pub to: String,
    /// The account `from`–`to` were set for by a chart's piece (dashboards.md
    /// revision 5): another register opens without that range.
    pub range_for: Option<String>,
    desc: bool,
    /// First row the window shows (as the view reported it).
    first_visible: usize,
    /// Scroll request: (shown row, serial).
    scroll: (i32, i32),
    pub selected: Option<usize>,
    pub last_date: Option<String>,
    pub confirm_delete: Option<String>,
    pub reverse: Option<(String, String)>,
    pub focus_filter: bool,
    pub hide_scheduled: bool,
    scheduled: Vec<Value>,
    scheduled_key: String,
    pub scheduled_selected: Option<usize>,
    pub popup: Option<popup::Popup>,
    pub pending_select: Option<String>,
    /// Open the selected row's split view once it is selected (an investment
    /// transaction changed by hand, investments-options-bonds-crypto.md revision 2).
    pub pending_splits: bool,
    pub pending_new: bool,
    vendor_gen: u64,
    vendor_on: bool,
    vendors: Vec<String>,
    vendor_links: HashMap<String, String>,
    /// Staged feed rows for this account (connectivity.md revision 4):
    /// (count, provider, batch) per staged batch.
    pub review: Vec<(u64, String, String)>,
}

impl RegisterState {
    /// Scroll so that shown row `row` is in view.
    fn scroll_to(&mut self, row: usize) {
        self.scroll = (
            i32::try_from(row).unwrap_or(0),
            self.scroll.1.wrapping_add(1),
        );
    }
}

/// The two amount columns by account kind (revision 8).
pub(crate) fn directions(kind: &str, subtype: &str) -> (&'static str, &'static str, bool) {
    match (kind, subtype) {
        (_, "bank" | "cash") => ("Spent", "Received", false),
        (_, "credit-card") => ("Charge", "Payment", true),
        ("liability", _) => ("Increase", "Payment", true),
        _ => ("Decrease", "Increase", false),
    }
}

pub(crate) fn titles(header: &Value) -> (&'static str, &'static str, bool) {
    let a = header.at("account");
    directions(a.str("type"), a.str("subtype"))
}

/// The amount column (6 or 7) for a display increase or decrease.
fn amount_col(first_inc: bool, increase: bool) -> usize {
    if increase == first_inc { 6 } else { 7 }
}

/// An amount without its sign: the column already says which way it moved.
pub(crate) fn unsigned(text: &str) -> &str {
    let t = text.trim();
    t.strip_prefix('-')
        .or_else(|| t.strip_prefix('\u{2212}'))
        .unwrap_or(t)
}

fn debit_normal(header: &Value) -> bool {
    matches!(header.at("account").str("type"), "asset" | "expense")
}

/// A display-signed change of this account as its stored (debit-positive) value.
pub(crate) fn to_raw(header: &Value, display: Decimal) -> Decimal {
    if debit_normal(header) {
        display
    } else {
        -display
    }
}

pub(crate) fn scale(header: &Value) -> u32 {
    mk_core::currency::minor_units(header.at("account").str("commodity"))
}

pub(crate) fn status_word(state: &str) -> &'static str {
    match state {
        "r" => "reconciled",
        "c" => "cleared",
        _ => "uncleared",
    }
}

fn status_glyph(state: &str) -> &'static str {
    match state {
        "r" => "●",
        "c" => "◐",
        _ => "○",
    }
}

/// A column width for `text` in the body font (about 8 px a character), at least
/// `min`.
fn narrow_width(text: &str, min: i32) -> i32 {
    let n = i32::try_from(text.chars().count()).unwrap_or(40);
    n.saturating_mul(8).saturating_add(12).max(min)
}

/// The view's status mark for a reconcile state.
fn status_mark(state: &str) -> &'static str {
    match state {
        "r" => "status:r",
        "c" => "status:c",
        _ => "status:n",
    }
}

fn args(st: &RegisterState, offset: usize, limit: usize) -> Value {
    let mut a = json!({
        "account": st.account, "offset": offset, "limit": limit,
        "order": if st.desc { "desc" } else { "asc" },
    });
    if let Ok(Some(d)) = fmt::read_date(&st.from)
        && let Some(o) = a.as_object_mut()
    {
        o.insert("from".into(), json!(d));
    }
    if let Ok(Some(d)) = fmt::read_date(&st.to)
        && let Some(o) = a.as_object_mut()
    {
        o.insert("to".into(), json!(d));
    }
    a
}

fn total(st: &RegisterState) -> usize {
    match &st.filtered {
        Some(f) => f.len(),
        None => st
            .header
            .at("total_rows")
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0),
    }
}

/// Reload when the book, account, arguments or sort order changed.
fn sync(app: &mut App, account: &str) {
    let desc = app.config.date_order == crate::DateOrder::Descending;
    let key = format!(
        "{}|{account}|{}|{}|{}|{desc}",
        app.generation, app.register.filter, app.register.from, app.register.to
    );
    if app.register.key == key {
        return;
    }
    let newest = |st: &mut RegisterState, n: usize, m: usize| {
        let row = if st.desc {
            0
        } else {
            (n + m).saturating_sub(1)
        };
        st.scroll_to(row);
    };
    let switched = app.register.account != account;
    // A new order without a selected row opens at the newest row too.
    let reorder = !switched && app.register.desc != desc && app.register.selected.is_none();
    if switched {
        app.register.selected = None;
        app.register.popup = None;
        app.register.first_visible = 0;
    } else if app.register.desc != desc {
        let n = total(&app.register);
        if let Some(i) = app.register.selected.filter(|i| *i < n) {
            let j = n - 1 - i;
            app.register.selected = Some(j);
            app.register.scroll_to(j);
        }
    }
    app.register.desc = desc;
    app.register.account = account.to_owned();
    app.register.key = key;
    app.register.pages.clear();
    app.register.filtered = None;
    app.register.accounts = array(app.read("account.list", json!({"include_archived": true})));
    let filter = app.register.filter.trim().to_lowercase();
    let limit = if filter.is_empty() {
        PAGE
    } else {
        FILTER_LIMIT
    };
    let a = args(&app.register, 0, limit);
    let Some(h) = app.read("account.register", a) else {
        app.register.header = Value::Null;
        return;
    };
    let rows = h.list("rows").to_vec();
    if filter.is_empty() {
        app.register.pages.insert(0, rows);
    } else {
        let accounts = app.register.accounts.clone();
        let names = |r: &Value| {
            format!(
                "{} {} {}",
                r.str("payee"),
                r.str("memo"),
                category_text(&accounts, r)
            )
            .to_lowercase()
        };
        app.register.filtered = Some(
            rows.into_iter()
                .filter(|r| names(r).contains(&filter))
                .collect(),
        );
    }
    // The view may name the account by id or full name; attention items use the id.
    let id = h.at("account").str("id").to_owned();
    app.register.review = super::connections::staged_batches(app)
        .iter()
        .filter_map(|b| {
            let n = b
                .list("accounts")
                .iter()
                .find(|p| p.str("account") == id)
                .and_then(|p| p.at("count").as_u64())?;
            Some((n, b.str("provider").to_owned(), b.str("batch").to_owned()))
        })
        .collect();
    app.register.header = h;
    if switched || reorder {
        let n = total(&app.register);
        newest(&mut app.register, n, 0);
    }
}

fn row(app: &mut App, i: usize) -> Option<Value> {
    if let Some(f) = &app.register.filtered {
        return f.get(i).cloned();
    }
    let page = i / PAGE * PAGE;
    if !app.register.pages.contains_key(&page) {
        let a = args(&app.register, page, PAGE);
        let rows = app
            .read("account.register", a)
            .map(|h| h.list("rows").to_vec())
            .unwrap_or_default();
        app.register.pages.insert(page, rows);
    }
    app.register
        .pages
        .get(&page)
        .and_then(|p| p.get(i - page))
        .cloned()
}

fn select_txn(app: &mut App, txn: &str) {
    let n = total(&app.register);
    let mut look: Vec<usize> = (0..n.min(PAGE)).collect();
    look.extend(n.saturating_sub(PAGE).max(n.min(PAGE))..n);
    for i in look {
        if row(app, i).is_some_and(|r| r.str("txn") == txn) {
            app.register.selected = Some(i);
            let shown = shown_index(app, i);
            app.register.scroll_to(shown);
            return;
        }
    }
}

/// The table row of real row `i` (scheduled rows lead when newest is first).
fn shown_index(app: &App, i: usize) -> usize {
    if app.register.desc {
        i + visible_scheduled(app).len()
    } else {
        i
    }
}

fn visible_scheduled(app: &App) -> Vec<Value> {
    if app.register.hide_scheduled || app.register.filtered.is_some() {
        Vec::new()
    } else {
        app.register.scheduled.clone()
    }
}

fn account_type(accounts: &[Value], id: &str) -> String {
    accounts
        .iter()
        .find(|a| a.str("id") == id)
        .map(|a| a.str("type").to_owned())
        .unwrap_or_default()
}

/// What the Category column shows for a row.
pub(crate) fn category_text(accounts: &[Value], r: &Value) -> String {
    let other = r.at("other");
    match other.str("kind") {
        "account" => {
            let name = other.str("full_name");
            if matches!(
                account_type(accounts, other.str("account")).as_str(),
                "asset" | "liability"
            ) {
                format!("Transfer: {name}")
            } else {
                name.to_owned()
            }
        }
        "split" => format!("Split ({})", other.at("count")),
        _ => String::new(),
    }
}

fn split_index(r: &Value) -> Option<(String, usize)> {
    let (t, i) = r.str("split").rsplit_once(':')?;
    Some((t.to_owned(), i.parse().ok()?))
}

fn shift(date: &str, days: i32) -> String {
    date.parse::<Date>()
        .ok()
        .and_then(|d| d.add_days(days))
        .map(|d| d.to_string())
        .unwrap_or_default()
}

/// Pending occurrences for the account in the next 30 days.
fn load_scheduled(app: &mut App, account: &str) {
    let today = app.today();
    let key = format!("{}|{account}|{today}", app.generation);
    if app.register.scheduled_key == key {
        return;
    }
    app.register.scheduled_key = key;
    app.register.scheduled.clear();
    app.register.scheduled_selected = None;
    let to = shift(&today, 30);
    let Ok(f) = app.backend.invoke(
        "schedule.forecast",
        json!({"account": account, "to": to, "today": today}),
    ) else {
        return;
    };
    let upcoming = app
        .backend
        .invoke(
            "schedule.upcoming",
            json!({"account": account, "to": to, "today": today}),
        )
        .map(|v| v.list("occurrences").to_vec())
        .unwrap_or_default();
    app.register.scheduled = f
        .list("rows")
        .iter()
        .map(|r| {
            let mut v = r.clone();
            if let Some(o) = upcoming.iter().find(|o| {
                o.str("schedule") == r.str("schedule") && o.str("nominal") == r.str("nominal")
            }) && let Some(m) = v.as_object_mut()
            {
                for k in ["category", "summary", "amount_kind", "when"] {
                    m.insert(k.into(), o.at(k).clone());
                }
                m.insert("money".into(), json!({"amount": r.str("amount")}));
            }
            v
        })
        .collect();
}

fn load_vendors(app: &mut App) {
    if app.register.vendor_gen == app.generation {
        return;
    }
    app.register.vendor_gen = app.generation;
    app.register.vendor_on = app.module_on("1099");
    if !app.register.vendor_on {
        return;
    }
    app.register.vendors = array(
        app.backend
            .invoke("party.list", json!({"role": "vendor", "active": true}))
            .ok(),
    )
    .iter()
    .map(|p| p.str("name").to_owned())
    .collect();
    app.register.vendor_links = app
        .backend
        .invoke("txn.vendor_links", json!({}))
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
        .into_iter()
        .map(|(t, v)| (t, v.str("vendor_name").to_owned()))
        .collect();
}

/// A date field: the pattern as placeholder, a refusal under it while it does not
/// read as a date.
pub(crate) fn date_field(ui: &mut Ui, name: &str, text: &mut String) -> super::ui::Resp {
    let pattern = fmt::date_style().pattern().to_owned();
    let problem = match fmt::read_date(text) {
        Err(m) if !text.trim().is_empty() => Some(m),
        _ => None,
    };
    let style = fmt::date_style();
    // The rest of the pattern after what is typed (the field draws it greyed
    // while focused; it is never part of the value).
    let guide = if text.is_empty() {
        None
    } else {
        style.guide(text)
    }
    .unwrap_or("")
    .to_owned();
    ui.hint(pattern.clone())
        .width(140)
        .error(problem)
        .tip(format!("Date, format {pattern}"));
    let r = ui.field(name, text);
    ui.set_last_text(&guide);
    r
}

pub(crate) fn view(app: &mut App, ui: &mut Ui, account: &str) {
    if app
        .register
        .range_for
        .as_ref()
        .is_some_and(|a| a != account)
    {
        app.register.range_for = None;
        app.register.from.clear();
        app.register.to.clear();
    }
    sync(app, account);
    if app.register.header.is_null() {
        ui.label("This account cannot be shown.");
        return;
    }
    if let Some(txn) = app.register.pending_select.take() {
        select_txn(app, &txn);
        if std::mem::take(&mut app.register.pending_splits)
            && let Some(i) = app.register.selected
            && let Some(r) = row(app, i)
            && r.str("txn") == txn
        {
            popup::open(app, i, &r, 2);
            popup::open_split_editor(app);
        }
    }
    app.register.pending_splits = false;
    if std::mem::take(&mut app.register.pending_new) {
        popup::open_new(app);
    }
    load_scheduled(app, account);
    load_vendors(app);
    let header = app.register.header.clone();
    let (first_title, second_title, first_inc) = titles(&header);
    ui.title(header.at("account").str("full_name"));
    ui.no_scroll();
    let mut new_clicked = false;
    let mut opening_for: Option<String> = None;
    let projects_on = app.module_on("projects");
    let mut set_project = false;
    ui.toolbar(|ui| {
        ui.hint("Filter").width(180);
        if app.register.focus_filter {
            ui.focus();
            app.register.focus_filter = false;
        }
        ui.field("Filter", &mut app.register.filter);
        date_field(ui, "From date", &mut app.register.from);
        date_field(ui, "To date", &mut app.register.to);
        let mut show = !app.register.hide_scheduled;
        if ui.checkbox(&mut show, "Show scheduled").clicked() {
            app.register.hide_scheduled = !show;
        }
        // A linked account syncs its connection on request (revision 17).
        let id = header.at("account").str("id").to_owned();
        let feed = super::accounts::linked_accounts(app)
            .into_iter()
            .find(|f| f.0 == id);
        if let Some((_, conn, label, _)) = feed {
            if let Some(job) = super::connections::job_of(app, &conn) {
                super::connections::syncing_row(app, ui, job, &label);
            } else {
                ui.tip(format!("Fetch new transactions from {label}"));
                if ui.button("Sync now").clicked() {
                    super::connections::sync_now(app, &conn);
                }
            }
        }
        // opening-balances.md: the account's ⋯ menu (left of + New, which stays
        // rightmost); projects.md: Set project… on chosen transactions.
        let opening = super::opening::menu_entry(header.at("account"));
        let mut entries: Vec<String> = opening.iter().map(|e| (*e).to_owned()).collect();
        if projects_on {
            entries.push("Set project…".to_owned());
        }
        if !entries.is_empty() {
            ui.named("Account actions");
            if let Some(i) = ui.menu("…", &entries).chosen {
                if entries.get(i).map(String::as_str) == Some("Set project…") {
                    set_project = true;
                } else {
                    opening_for = Some(id.clone());
                }
            }
        }
        ui.tip("Add a transaction (Ctrl+N)");
        new_clicked = ui.primary("+ New").clicked();
    });
    if let Some(a) = opening_for {
        super::opening::open_for(app, &a);
    }
    if set_project {
        // The selected row starts ticked; the dialog searches this register.
        let ticked: Vec<String> = app
            .register
            .selected
            .and_then(|i| row(app, i))
            .map(|r| vec![r.str("txn").to_owned()])
            .unwrap_or_default();
        let account = header.at("account").str("id").to_owned();
        super::projects::open_set(app, "", &account, &ticked);
    }
    if new_clicked {
        popup::open_new(app);
    }
    review_band(app, ui);
    keys(app, ui);
    // The date column fits the longest date in the chosen format.
    let longest = mk_core::Date::from_ymd(2026, 9, 30)
        .map(|d| fmt::date_style().format(d))
        .unwrap_or_default();
    let date_w = narrow_width(&longest, 72);
    // Amount columns fit the longest balance shown (the old toolkit measured the
    // loaded amounts the same way), at least their title.
    let amount_chars = ["closing_balance", "cleared_balance", "reconciled_balance"]
        .iter()
        .map(|k| fmt::group(header.str(k)).chars().count())
        .max()
        .unwrap_or(8)
        .max(8);
    let amount_w = narrow_width(&"0".repeat(amount_chars), 72);
    let mut columns = vec![
        Column {
            title: "Date".into(),
            width: date_w,
            ..Column::default()
        },
        Column {
            title: "No.".into(),
            width: 40,
            ..Column::default()
        },
        Column {
            title: "Payee".into(),
            ..Column::default()
        },
        Column {
            title: "Category".into(),
            ..Column::default()
        },
        Column {
            title: "Memo".into(),
            ..Column::default()
        },
        Column {
            title: "Status".into(),
            width: 48,
            align: Align::Center,
            ..Column::default()
        },
        Column {
            title: first_title.into(),
            width: amount_w.max(narrow_width(first_title, 0)),
            align: Align::Right,
            ..Column::default()
        },
        Column {
            title: second_title.into(),
            width: amount_w.max(narrow_width(second_title, 0)),
            align: Align::Right,
            ..Column::default()
        },
        Column {
            title: "Balance".into(),
            width: amount_w,
            align: Align::Right,
            ..Column::default()
        },
    ];
    if app.register.vendor_on {
        columns.push(Column {
            title: "Vendor".into(),
            width: 150,
            ..Column::default()
        });
    }
    let n = total(&app.register);
    let sched = visible_scheduled(app);
    let m = sched.len();
    let newest_first = app.register.desc;
    let count = n + m;
    let first = app
        .register
        .first_visible
        .saturating_sub(WINDOW_BEFORE)
        .min(count);
    let last = (app.register.first_visible + WINDOW_AFTER).min(count);
    let scroll = app.register.scroll;
    ui.table("Transactions", columns.clone(), |ui| {
        ui.window(count, first, scroll, ROW_HEIGHT);
        if let Some(f) = ui.scrolled("Transactions") {
            app.register.first_visible = f;
        }
        for i in first..last {
            let (real, s) = if newest_first {
                (i.checked_sub(m), (i < m).then_some(i))
            } else {
                ((i < n).then_some(i), i.checked_sub(n))
            };
            match (real, s.and_then(|k| sched.get(k).map(|o| (k, o.clone())))) {
                (Some(ri), _) if ri < n => {
                    if let Some(r) = row(app, ri) {
                        row_ui(app, ui, ri, &r, &columns, first_inc);
                    }
                }
                (_, Some((k, o))) => {
                    scheduled_row(app, ui, k, &o, first_inc, first_title, second_title)
                }
                _ => {
                    ui.table_row(&format!("blank:{i}"), false, false, |_| {});
                }
            }
        }
    });
    scheduled_actions(app, ui);
    ui.group(|ui| {
        ui.wrap(|ui| {
            for (label, key) in [
                ("Closing balance", "closing_balance"),
                ("Cleared balance", "cleared_balance"),
                ("Reconciled balance", "reconciled_balance"),
            ] {
                ui.label(format!("{label} {}", fmt::group(header.str(key))));
                ui.separator();
            }
        });
    });
    popup::show(app, ui, &header);
    confirm_dialogs(app, ui);
}

/// The band above the rows when feed rows for this account wait for review
/// (connectivity.md revision 4).
fn review_band(app: &mut App, ui: &mut Ui) {
    let staged = app.register.review.clone();
    for (i, (n, provider, batch)) in staged.iter().enumerate() {
        let (noun, verb) = if *n == 1 {
            ("transaction", "waits")
        } else {
            ("transactions", "wait")
        };
        ui.banner(Tone::Accent, |ui| {
            ui.row(|ui| {
                ui.next().grow();
                ui.label(format!("{n} {noun} from {provider} {verb} for review"));
                ui.named(if i == 0 {
                    "Review staged transactions".to_owned()
                } else {
                    format!("Review staged transactions {}", i + 1)
                });
                if ui.primary("Review").clicked() {
                    super::connections::review(app, batch);
                }
            });
        });
    }
}

fn row_ui(app: &mut App, ui: &mut Ui, i: usize, r: &Value, cols: &[Column], first_inc: bool) {
    let voided = r.str("status") == "voided";
    let category = category_text(&app.register.accounts, r);
    let amount = r.str("amount").to_owned();
    let increase = r.at("increase").as_bool() == Some(true);
    let selected = app.register.selected == Some(i);
    let payee = r.str("payee").to_owned();
    let imported = r.at("imported").as_bool() == Some(true);
    let release = r.at("release_for").is_string();
    let payee_text = if imported {
        format!("{payee}  · imported")
    } else if release {
        format!("{payee}  · for an expense")
    } else {
        payee.clone()
    };
    let tip = if imported {
        format!("{payee} (added from a bank file)")
    } else if release {
        format!(
            "{payee}: made automatically for a restricted-fund expense (right-click: Show expense)"
        )
    } else {
        payee.clone()
    };
    let receipts = r.at("receipts").as_u64().unwrap_or(0) > 0;
    let k = amount_col(first_inc, increase);
    let shown = unsigned(&amount).to_owned();
    let status = if voided {
        "void"
    } else {
        status_word(r.str("reconcile"))
    };
    let money = format!("{} {shown}", cols.get(k).map_or("", |c| c.title.as_str()));
    let summary = format!(
        "Row {}: {} {} {}, {money}, status {status}, balance {}{}",
        i + 1,
        fmt::date(r.str("date")),
        payee,
        category,
        r.str("balance"),
        if receipts { ", has receipts" } else { "" }
    );
    let tone = if voided {
        Tone::Secondary
    } else {
        Tone::Normal
    };
    let resp = ui.table_row(&summary, selected, true, |ui| {
        ui.tone(tone).strike(voided);
        ui.label(fmt::date(r.str("date")));
        ui.tone(tone).strike(voided).tip(r.str("number"));
        ui.label(r.str("number"));
        ui.tone(tone).strike(voided).tip(tip.clone());
        if receipts {
            ui.icon("paperclip");
        }
        ui.label(payee_text.clone());
        ui.tone(tone).strike(voided).tip(category.clone());
        ui.label(category.clone());
        ui.tone(tone).strike(voided).tip(r.str("memo"));
        ui.label(r.str("memo"));
        if voided {
            ui.tone(tone);
            ui.label("Void");
        } else {
            // Drawn as an empty, half-filled or filled circle (app-desktop.md
            // Register); the glyph stays the text for plain-text output.
            let state = r.str("reconcile");
            ui.named(status_word(state))
                .align(Align::Center)
                .icon(status_mark(state));
            ui.label(status_glyph(state));
        }
        for col in [6, 7] {
            if col == k {
                ui.tone(tone).strike(voided);
                ui.amount(fmt::group(&shown));
            } else {
                ui.amount("");
            }
        }
        let bal = r.str("balance");
        let bt = if voided {
            Tone::Secondary
        } else if fmt::is_negative(bal) {
            Tone::Decrease
        } else {
            Tone::Normal
        };
        ui.tone(bt);
        ui.amount(fmt::group(bal));
        if cols.len() > 9 {
            vendor_cell(app, ui, i, r);
        }
    });
    ui.row_menu(&row_menu_entries(app, r));
    if resp.clicked() && app.register.popup.is_none() {
        app.register.selected = Some(i);
        // The column clicked decides the field that takes the focus.
        let col = resp.value.parse::<usize>().unwrap_or(2);
        popup::open(app, i, r, col);
    }
    if let Some(Some(entry)) = resp.secondary {
        app.register.selected = Some(i);
        let entries = row_menu_entries(app, r);
        if let Some(e) = entries.get(entry) {
            row_menu(app, i, r, e);
        }
    }
}

fn vendor_cell(app: &mut App, ui: &mut Ui, i: usize, r: &Value) {
    let other = r.at("other");
    let expense = other.str("kind") == "split"
        || account_type(&app.register.accounts, other.str("account")) == "expense";
    if !expense || r.str("txn").is_empty() {
        ui.label("");
        return;
    }
    let txn = r.str("txn").to_owned();
    let current = app
        .register
        .vendor_links
        .get(&txn)
        .cloned()
        .unwrap_or_default();
    let mut options = vec!["No vendor".to_owned()];
    options.extend(app.register.vendors.iter().cloned());
    let mut idx = if current.is_empty() {
        0
    } else {
        options.iter().position(|o| *o == current).unwrap_or(0)
    };
    let resp = ui.combo(&format!("Vendor for row {}", i + 1), &mut idx, &options);
    if current.is_empty()
        && let Some(it) = ui.last()
    {
        it.value = "—".into();
    }
    if resp.changed() {
        let v = if idx == 0 {
            String::new()
        } else {
            options.get(idx).cloned().unwrap_or_default()
        };
        if v != current {
            let vendor = if v.is_empty() { Value::Null } else { json!(v) };
            if let Err(e) = app.run("txn.set_vendor", json!({"txn": txn, "vendor": vendor})) {
                app.notify_error(&e);
            }
        }
    }
}

fn row_menu_entries(app: &App, r: &Value) -> Vec<&'static str> {
    let _ = app;
    let voided = r.str("status") == "voided";
    let mut v = vec!["Edit"];
    if r.at("release_for").is_string() {
        v.push("Show expense");
    }
    v.push("Split…");
    v.push(if r.str("reconcile") == "n" {
        "Mark cleared"
    } else {
        "Mark uncleared"
    });
    v.push(if voided { "Unvoid" } else { "Void" });
    v.extend(["Repeat this…", "Reverse…", "Delete", "Copy as text"]);
    if r.at("other").str("kind") == "account" {
        v.push("Open other account");
    }
    v
}

fn show_expense(app: &mut App, txn: &str) {
    let Some(t) = app.read("txn.get", json!({"id": txn})) else {
        return;
    };
    let accounts = app.register.accounts.clone();
    let expense = t
        .list("splits")
        .iter()
        .find(|s| {
            accounts
                .iter()
                .any(|a| a.str("id") == s.str("account") && a.str("type") == "expense")
        })
        .or_else(|| t.list("splits").first())
        .map(|s| s.str("account").to_owned());
    if let Some(a) = expense {
        app.navigate(View::Register { account: a });
    }
}

/// Carry out a row menu entry.
pub(crate) fn row_menu(app: &mut App, i: usize, r: &Value, entry: &str) {
    let txn = r.str("txn").to_owned();
    match entry {
        "Edit" => popup::open(app, i, r, 2),
        "Show expense" => {
            if let Some(e) = r.at("release_for").as_str() {
                show_expense(app, e);
            }
        }
        "Split…" => {
            popup::open(app, i, r, 2);
            popup::open_split_editor(app);
        }
        "Mark cleared" | "Mark uncleared" => toggle_status(app, r),
        "Void" | "Unvoid" => {
            let cmd = if entry == "Void" {
                "txn.void"
            } else {
                "txn.unvoid"
            };
            if let Err(e) = app.run(cmd, json!({"id": txn})) {
                app.notify_error(&e);
            }
        }
        "Repeat this…" => super::scheduled_hooks::repeat_txn(app, &txn),
        "Reverse…" => {
            app.register.reverse = Some((txn, fmt::date(&app.today())));
        }
        "Delete" => app.register.confirm_delete = Some(txn),
        "Copy as text" => {
            let text = format!(
                "{}\t{}\t{}\t{}\t{}",
                fmt::date(r.str("date")),
                r.str("payee"),
                category_text(&app.register.accounts, r),
                r.str("amount"),
                r.str("balance")
            );
            app.copy(text);
        }
        "Open other account" => {
            app.navigate(View::Register {
                account: r.at("other").str("account").to_owned(),
            });
        }
        _ => {}
    }
}

fn toggle_status(app: &mut App, r: &Value) {
    match r.str("reconcile") {
        "r" => app.notify(
            "Reconciled transactions keep their status here; undo the reconciliation to change it.",
        ),
        state => {
            let Some((txn, idx)) = split_index(r) else {
                return;
            };
            let next = if state == "c" { "n" } else { "c" };
            if let Err(e) = app.run(
                "txn.set_reconcile",
                json!({"splits": [{"id": txn, "split": idx, "state": next}]}),
            ) {
                app.notify_error(&e);
            }
        }
    }
}

/// Register keys (when the pop-up is not open).
fn keys(app: &mut App, ui: &mut Ui) {
    if app.register.popup.is_some() || app.register.confirm_delete.is_some() {
        return;
    }
    if ui.key("ctrl+n") || ui.key("insert") {
        popup::open_new(app);
        return;
    }
    if ui.key("ctrl+f") {
        app.register.focus_filter = true;
    }
    let n = total(&app.register);
    let down = ui.key("down");
    let up = !down && ui.key("up");
    if n > 0 && (up || down) {
        let cur = app
            .register
            .selected
            .unwrap_or(if down { usize::MAX } else { n });
        let next = if down {
            cur.wrapping_add(1).min(n - 1)
        } else {
            cur.saturating_sub(1)
        };
        app.register.selected = Some(next);
        let shown = shown_index(app, next);
        app.register.scroll_to(shown);
    }
    let Some(sel) = app.register.selected else {
        return;
    };
    let enter = ui.key("enter");
    let space = !enter && ui.key(" ");
    let delete = !enter && !space && ui.key("delete");
    if !(enter || space || delete) {
        return;
    }
    let Some(r) = row(app, sel) else { return };
    if enter {
        popup::open(app, sel, &r, 2);
    }
    if space {
        toggle_status(app, &r);
    }
    if delete {
        app.register.confirm_delete = Some(r.str("txn").to_owned());
    }
}

fn confirm_dialogs(app: &mut App, ui: &mut Ui) {
    if let Some(txn) = app.register.confirm_delete.clone() {
        let mut close = ui.dialog_closed("confirm-delete");
        ui.dialog("confirm-delete", "Delete transaction", 420, |ui| {
            ui.label("Delete this transaction? You can undo it afterwards.");
            ui.actions(|ui| {
                if ui.button("Keep it").clicked() {
                    close = true;
                }
                if ui.primary("Delete transaction").clicked() {
                    if let Err(e) = app.run("txn.delete", json!({"id": txn})) {
                        app.notify_error(&e);
                    }
                    close = true;
                    app.register.selected = None;
                }
            });
        });
        if close {
            app.register.confirm_delete = None;
        }
    }
    if let Some((txn, mut date)) = app.register.reverse.clone() {
        let mut close = ui.dialog_closed("reverse");
        ui.dialog("reverse", "Reverse transaction", 420, |ui| {
            ui.label("Create a reversing transaction dated:");
            date_field(ui, "Reversal date", &mut date);
            ui.actions(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.primary("Reverse").clicked() {
                    match fmt::date_arg(&date) {
                        Ok(d) => {
                            if let Err(e) = app.run("txn.reverse", json!({"id": txn, "date": d})) {
                                app.notify_error(&e);
                            }
                            close = true;
                        }
                        Err(m) => app.notify(m),
                    }
                }
            });
        });
        app.register.reverse = if close { None } else { Some((txn, date)) };
    }
}

#[allow(clippy::too_many_arguments)]
fn scheduled_row(
    app: &mut App,
    ui: &mut Ui,
    k: usize,
    o: &Value,
    first_inc: bool,
    first_title: &str,
    second_title: &str,
) {
    let payee = if o.str("payee").is_empty() {
        o.str("name")
    } else {
        o.str("payee")
    }
    .to_owned();
    let amount = o.str("amount");
    let increase = !amount.starts_with('-');
    let shown = unsigned(amount).to_owned();
    let col = amount_col(first_inc, increase);
    let prefix = if o.str("amount_kind") == "estimate" {
        "about "
    } else {
        ""
    };
    let label = format!(
        "Scheduled row: due {} {payee}, {} {shown}, projected balance {}",
        fmt::date(o.str("date")),
        if col == 6 { first_title } else { second_title },
        o.str("balance")
    );
    let selected = app.register.scheduled_selected == Some(k);
    let resp = ui.table_row(&label, selected, true, |ui| {
        ui.tone(Tone::Secondary);
        ui.label(fmt::date(o.str("date")));
        ui.label("");
        ui.tone(Tone::Secondary);
        ui.label(payee.clone());
        ui.tone(Tone::Secondary);
        ui.label(o.str("category"));
        ui.tone(Tone::Secondary);
        ui.label(o.str("summary"));
        ui.named("scheduled")
            .align(Align::Center)
            .tone(Tone::Secondary);
        ui.label("◷");
        for c in [6, 7] {
            if c == col {
                ui.tone(Tone::Secondary);
                ui.amount(format!("{prefix}{}", fmt::group(&shown)));
            } else {
                ui.amount("");
            }
        }
        ui.tone(Tone::Secondary);
        ui.amount(fmt::group(o.str("balance")));
    });
    if resp.clicked() {
        app.register.scheduled_selected = Some(k);
        app.register.selected = None;
    }
}

fn scheduled_actions(app: &mut App, ui: &mut Ui) {
    let Some(o) = app
        .register
        .scheduled_selected
        .and_then(|k| app.register.scheduled.get(k).cloned())
    else {
        return;
    };
    let payee = if o.str("payee").is_empty() {
        o.str("name")
    } else {
        o.str("payee")
    }
    .to_owned();
    ui.card("", |ui| {
        ui.row(|ui| {
            ui.next().grow();
            ui.label(format!(
                "Scheduled: {payee} due {}",
                fmt::date(o.str("due"))
            ));
            ui.named(format!("Enter scheduled {payee}"));
            if ui.button("Enter").clicked() {
                super::scheduled_hooks::enter(app, &o);
                app.register.scheduled_selected = None;
            }
            ui.named(format!("Skip scheduled {payee}"));
            if ui.button("Skip").clicked() {
                super::scheduled_hooks::skip(app, o.str("schedule"), o.str("nominal"));
                app.register.scheduled_selected = None;
            }
        });
    });
}

/// A row's transaction as an entry, plus the other side's lines when it has more
/// than one other split.
pub(crate) fn load_entry(
    app: &mut App,
    r: &Value,
) -> Option<(popup::Entry, Option<Vec<popup::SplitLine>>)> {
    let t = app.read("txn.get", json!({"id": r.str("txn")}))?;
    let header = app.register.header.clone();
    let account = app.register.account.clone();
    let splits = t.list("splits").to_vec();
    let mine = splits.iter().find(|s| {
        s.str("account") == account || header.at("account").str("id") == s.str("account")
    })?;
    let raw: Decimal = mine.str("value").parse().unwrap_or_default();
    let display = to_raw(&header, raw);
    let mut e = popup::Entry {
        date: fmt::date(t.str("date")),
        number: t.str("number").to_owned(),
        payee: t.str("payee").to_owned(),
        memo: t.str("memo").to_owned(),
        editing: Some(t.str("id").to_owned()),
        ..popup::Entry::default()
    };
    if display.is_sign_negative() {
        e.decrease = (-display).to_string();
    } else {
        e.increase = display.to_string();
    }
    let others: Vec<&Value> = splits.iter().filter(|s| !std::ptr::eq(*s, mine)).collect();
    if let [one] = others.as_slice() {
        e.category = one.str("account_name").to_owned();
        e.tag_names = one.at("tag_names").clone();
        Some((e, None))
    } else {
        let sign = if display.is_sign_negative() {
            -Decimal::ONE
        } else {
            Decimal::ONE
        };
        let lines = others
            .iter()
            .map(|s| {
                let v: Decimal = s.str("value").parse().unwrap_or_default();
                let line = to_raw(&header, -v) * sign;
                popup::SplitLine {
                    category: s.str("account_name").to_owned(),
                    memo: s.str("memo").to_owned(),
                    amount: line.to_string(),
                }
            })
            .collect();
        Some((e, Some(lines)))
    }
}

#[cfg(test)]
mod tests {
    use super::{amount_col, directions, unsigned};

    #[test]
    fn per_kind_columns_revision_8() {
        assert_eq!(directions("asset", "bank"), ("Spent", "Received", false));
        assert_eq!(
            directions("liability", "credit-card"),
            ("Charge", "Payment", true)
        );
        assert_eq!(
            directions("liability", "loan"),
            ("Increase", "Payment", true)
        );
        assert_eq!(directions("asset", ""), ("Decrease", "Increase", false));
        assert_eq!(amount_col(true, true), 6);
        assert_eq!(amount_col(false, false), 6);
        assert_eq!(amount_col(false, true), 7);
        assert_eq!(unsigned("-1,003.15"), "1,003.15");
    }
}
