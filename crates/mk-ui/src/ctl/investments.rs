//! Investment screens (app-desktop.md revision 10) on the toolkit-free controller:
//! the brokerage account view with Holdings and Activity tabs, the action forms
//! (live totals and the Sell form's lot preview come from `inv.preview`), the Lots
//! dialog and the Securities view. Shown only while the `investments` module is
//! on. Every number comes from a command; the UI only collects text.

use serde_json::{Value, json};

use super::form::set;
use super::ui::{Column, Tone, Ui, col, col_num};
use super::util::Json;
use super::{App, AppEvent, MessageAction, View};
use crate::fmt;

const NOTE: &str = "MoneyKeep keeps records; it does not give tax advice.";

/// Actions: command suffix and button/title label.
pub(crate) const ACTIONS: [(&str, &str); 10] = [
    ("buy", "Buy"),
    ("sell", "Sell"),
    ("dividend", "Dividend"),
    ("reinvest", "Reinvest"),
    ("interest", "Interest"),
    ("fee", "Fee"),
    ("shares_in", "Shares in"),
    ("shares_out", "Shares out"),
    ("split", "Split"),
    ("return_of_capital", "Return of capital"),
];

/// Plain hints for the form fields (revision 10a).
pub(crate) const HINTS: [(&str, &str); 8] = [
    ("date", "YYYY-MM-DD"),
    ("security", "Symbol or name"),
    ("quantity", "Number of shares"),
    ("price", "Per share"),
    ("commission", "0.00"),
    ("total", "Calculated"),
    ("amount", "0.00"),
    ("ratio", "1"),
];

/// Form titles (revision 10a).
const TITLES: [(&str, &str); 10] = [
    ("buy", "Buy shares"),
    ("sell", "Sell shares"),
    ("dividend", "Record a dividend"),
    ("reinvest", "Reinvest a dividend"),
    ("interest", "Record interest"),
    ("fee", "Record a fee"),
    ("shares_in", "Move shares in"),
    ("shares_out", "Move shares out"),
    ("split", "Record a stock split"),
    ("return_of_capital", "Record a return of capital"),
];

pub(crate) fn form_title(action: &str, editing: bool) -> String {
    let t = label_of(&TITLES, action);
    if editing { format!("Edit — {t}") } else { t }
}

const KINDS: [(&str, &str); 5] = [
    ("ordinary", "Ordinary"),
    ("qualified", "Qualified"),
    ("capital_gain_long", "Capital gain, long-term"),
    ("capital_gain_short", "Capital gain, short-term"),
    ("tax_exempt", "Tax-exempt"),
];

const SEC_KINDS: [(&str, &str); 9] = [
    ("stock", "Stock"),
    ("etf", "ETF"),
    ("mutual_fund", "Mutual fund"),
    ("bond", "Bond"),
    ("cd", "CD"),
    ("money_market", "Money market"),
    ("option", "Option"),
    ("crypto", "Crypto"),
    ("other", "Other"),
];

const TAXES: [(&str, &str); 3] = [
    ("taxable", "Taxable"),
    ("municipal", "Municipal (tax-exempt)"),
    ("treasury", "US Treasury"),
];

const METHODS: [(&str, &str); 3] = [
    ("fifo", "First in, first out"),
    ("average", "Average cost"),
    ("specific", "Specific lots"),
];

fn label_of(table: &[(&str, &str)], key: &str) -> String {
    table
        .iter()
        .find(|(k, _)| *k == key)
        .map_or(key.to_owned(), |(_, l)| (*l).to_owned())
}

fn index_of(table: &[(&str, &str)], key: &str) -> usize {
    table.iter().position(|(k, _)| *k == key).unwrap_or(0)
}

/// One action form (new or editing an investment transaction).
#[derive(Debug, Clone, Default)]
pub(crate) struct ActionForm {
    pub action: &'static str,
    pub editing: Option<String>,
    pub account: String,
    pub date: String,
    pub security: String,
    pub quantity: String,
    pub price: String,
    pub commission: String,
    /// Total (buy, sell, reinvest) or amount (the others).
    pub total: String,
    pub total_edited: bool,
    pub kind: usize,
    pub ratio_new: String,
    pub ratio_old: String,
    /// Shares in: (acquired, quantity, basis).
    pub lots_in: Vec<[String; 3]>,
    /// Sell / shares out: choose lots.
    pub choose: bool,
    /// (lot id, acquired, shares available, basis, shares to sell).
    pub lot_rows: Vec<(String, String, String, String, String)>,
    pub lots_key: String,
    pub new_security: Option<[String; 2]>,
    pub new_kind: usize,
    /// A save error, with the field it concerns.
    pub error: Option<(Option<&'static str>, String)>,
    pub preview_key: String,
    pub preview: Option<Value>,
    pub preview_error: Option<(Option<&'static str>, String)>,
    /// Sell / move out: securities held on the form's date (symbol, shares).
    pub held: Vec<(String, String)>,
    pub held_key: String,
    /// The single qualifying security was preselected (once, on opening).
    pub preselected: bool,
    pub confirm_delete: bool,
    /// Sell at a loss: `inv.wash_check` for the 30-day notice.
    pub wash: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SecForm {
    pub editing: Option<String>,
    pub symbol: String,
    pub name: String,
    pub kind: usize,
    pub method: usize,
    pub cusip: String,
    pub error: Option<String>,
    /// Options (investments-options-bonds-crypto.md): underlying, put, strike,
    /// expiration, multiplier.
    pub underlying: String,
    pub put: bool,
    pub strike: String,
    pub expiration: String,
    pub multiplier: String,
    /// Bonds: coupon %, maturity, face per unit, payments per year, tax.
    pub coupon: String,
    pub maturity: String,
    pub face: String,
    pub per_year: String,
    pub tax: usize,
}

#[derive(Debug, Default)]
pub(crate) struct InvState {
    /// (generation, account id, is a brokerage account).
    brokerage: Vec<(u64, String, bool)>,
    pub tab: usize,
    pub as_of: String,
    pub form: Option<ActionForm>,
    pub lots_modal: Option<(String, String)>,
    pub selected_security: Option<String>,
    pub sec_form: Option<SecForm>,
    /// Update prices…: (symbol, date, price).
    pub prices: Option<Vec<(String, String, String)>>,
    pub prices_error: Option<String>,
    pub more_open: bool,
    /// An open corporate action form.
    pub corp: Option<super::corporate::CorpForm>,
    /// The Wash groups… dialog.
    pub wash: Option<super::corporate::WashForm>,
    /// An open Option…, Bond… or Crypto… form.
    pub obc: Option<super::inv_more::ObcForm>,
    /// The dialog of an expired option.
    pub expiry: Option<super::inv_more::ExpiryForm>,
}

/// Whether the investments module is on.
pub(crate) fn enabled(app: &mut App) -> bool {
    app.module_on("investments")
}

/// Whether `account` is a brokerage account shown with the investment view.
pub(crate) fn is_brokerage(app: &mut App, account: &str) -> bool {
    if !enabled(app) {
        return false;
    }
    let generation = app.generation;
    app.inv.brokerage.retain(|(g, ..)| *g == generation);
    if let Some((_, _, b)) = app.inv.brokerage.iter().find(|(_, a, _)| a == account) {
        return *b;
    }
    let b = app
        .backend
        .invoke("account.get", json!({"account": account}))
        .ok()
        .is_some_and(|a| a.str("subtype") == "brokerage");
    app.inv.brokerage.push((generation, account.to_owned(), b));
    b
}

// ----- brokerage account view ----------------------------------------------------------

pub(crate) fn view(app: &mut App, ui: &mut Ui, account: &str) {
    let Some(acc) = app.read("account.get", json!({"account": account})) else {
        return;
    };
    if app.inv.as_of.trim().is_empty() {
        app.inv.as_of = fmt::date(&app.today());
    }
    ui.title(acc.str("full_name"));
    let mut open_opening = false;
    ui.toolbar(|ui| {
        for (key, label) in ACTIONS.iter().take(4) {
            if ui.button(*label).clicked() {
                open_form(app, key, account);
            }
        }
        if ui.button("More…").clicked() {
            app.inv.more_open = !app.inv.more_open;
        }
        if ui.button("Securities").clicked() {
            app.navigate(View::Securities);
        }
        if ui.button("Set opening holdings…").clicked() {
            open_opening = true;
        }
    });
    if open_opening {
        super::opening::open_for(app, account);
    }
    if app.inv.more_open {
        ui.wrap(|ui| {
            // investments-options-bonds-crypto.md: the Option…, Bond… and Crypto…
            // groups.
            for (key, label) in super::inv_more::GROUPS {
                if ui.button(label).clicked() {
                    super::inv_more::open(app, key, account);
                    app.inv.more_open = false;
                }
            }
            for (key, label) in ACTIONS.iter().skip(4) {
                if ui.button(*label).clicked() {
                    open_form(app, key, account);
                    app.inv.more_open = false;
                }
            }
            for (key, label) in super::corporate::KINDS {
                if ui.button(label).clicked() {
                    super::corporate::open(app, key, account);
                    app.inv.more_open = false;
                }
            }
        });
    }
    ui.row(|ui| {
        for (i, t) in ["Holdings", "Activity"].iter().enumerate() {
            if ui.tab(*t, app.inv.tab == i).clicked() {
                app.inv.tab = i;
            }
        }
    });
    if app.inv.tab == 0 {
        holdings_tab(app, ui, account);
    } else {
        activity_tab(app, ui, account);
    }
    form_dialog(app, ui);
    lots_dialog(app, ui);
    super::corporate::form_dialog(app, ui);
}

fn money(ui: &mut Ui, text: &str) {
    ui.tone(if fmt::is_negative(text) {
        Tone::Decrease
    } else {
        Tone::Normal
    });
    ui.amount(fmt::group(text));
}

fn holdings_tab(app: &mut App, ui: &mut Ui, account: &str) {
    ui.row(|ui| {
        ui.caption("As of");
        super::register::date_field(ui, "As of", &mut app.inv.as_of);
    });
    let as_of = fmt::date_arg_lenient(&app.inv.as_of);
    let Some(h) = app.read(
        "inv.holdings",
        json!({"accounts": [account], "as_of": as_of}),
    ) else {
        return;
    };
    let acc = h
        .at("accounts")
        .as_array()
        .and_then(|a| a.first().cloned())
        .unwrap_or(Value::Null);
    // investments-options-bonds-crypto.md: options, bonds and crypto show their terms.
    let details = acc
        .list("holdings")
        .iter()
        .any(|r| !super::inv_more::holding_details(r).is_empty());
    let mut cols = vec![
        Column {
            weight: 4,
            ..col("Security")
        },
        Column {
            width: 100,
            ..col_num("Shares")
        },
        Column {
            width: 90,
            ..col_num("Price")
        },
        Column {
            width: 110,
            ..col("Price date")
        },
        Column {
            width: 120,
            ..col_num("Market value")
        },
        Column {
            width: 120,
            ..col_num("Cost basis")
        },
        Column {
            width: 100,
            ..col_num("Gain")
        },
        Column {
            width: 72,
            ..col_num("Gain %")
        },
        Column {
            width: 72,
            ..col_num("Weight")
        },
    ];
    if details {
        // Room for the terms: the number columns a little narrower.
        for c in cols.iter_mut() {
            c.width = match c.width {
                120 => 100,
                110 => 96,
                100 => 90,
                72 => 64,
                w => w,
            };
        }
        cols.insert(
            1,
            Column {
                weight: 2,
                ..col("Details")
            },
        );
    }
    let mut open = None;
    ui.table("Holdings", cols, |ui| {
        for row in acc.list("holdings") {
            let summary = format!(
                "Holding {}: {} shares, market value {}",
                row.str("symbol"),
                row.str("quantity"),
                row.str("market_value")
            );
            let priced = !row.at("price").is_null();
            let r = ui.table_row(&summary, false, true, |ui| {
                let sec = if row.str("kind") == "option" {
                    row.str("name").to_owned()
                } else {
                    format!("{}  {}", row.str("symbol"), row.str("name"))
                };
                ui.tip(sec.clone());
                ui.label(sec);
                if details {
                    let d = super::inv_more::holding_details(row);
                    if row.at("face_note").is_string() {
                        ui.tip(row.str("face_note"));
                    } else {
                        ui.tip(d.clone());
                    }
                    ui.label(d);
                }
                ui.mono();
                let q = if row.at("quantity_text").is_string() {
                    row.str("quantity_text")
                } else {
                    row.str("quantity")
                };
                ui.amount(q);
                if priced {
                    ui.mono();
                    if row.at("price_percent").is_string() {
                        ui.amount(format!("{}%", fmt::group(row.str("price"))));
                    } else {
                        ui.amount(fmt::group(row.str("price")));
                    }
                    let when = if row.at("price_note").is_string() {
                        row.str("price_note").to_owned()
                    } else {
                        fmt::date(row.str("price_date"))
                    };
                    ui.label(when);
                    money(ui, row.str("market_value"));
                } else {
                    ui.amount("no price");
                    ui.label("");
                    ui.label("");
                }
                ui.amount(fmt::group(row.str("basis")));
                if priced {
                    money(ui, row.str("gain"));
                    ui.amount(row.str("gain_percent"));
                    ui.amount(row.str("weight"));
                } else {
                    // Every cell, so the row lines up with the columns.
                    ui.label("");
                    ui.label("");
                    ui.label("");
                }
            });
            if r.clicked() {
                open = Some(row.str("symbol").to_owned());
            }
        }
        for (title, value, basis) in [
            ("Cash", acc.str("cash").to_owned(), String::new()),
            (
                "Total",
                acc.str("total").to_owned(),
                acc.str("basis").to_owned(),
            ),
        ] {
            ui.table_row(title, false, false, |ui| {
                ui.bold();
                ui.label(title);
                if details {
                    ui.label("");
                }
                ui.label("");
                ui.label("");
                ui.label("");
                ui.amount(fmt::group(&value));
                if !basis.is_empty() {
                    ui.amount(fmt::group(&basis));
                } else {
                    ui.label("");
                }
                ui.label("");
                ui.label("");
                ui.label("");
            });
        }
    });
    if let Some(sym) = open {
        app.inv.lots_modal = Some((account.to_owned(), sym));
    }
    super::opening::basis_line(app, ui, &acc);
    if acc.at("unpriced").as_bool() == Some(true) {
        ui.secondary("Holdings without a price are left out of the totals.");
    }
}

fn activity_tab(app: &mut App, ui: &mut Ui, account: &str) {
    let Some(a) = app.read("inv.activity", json!({"account": account})) else {
        return;
    };
    // The command lists oldest first with running cash balances; Sort transactions
    // by date (revision 27) only changes the order they are shown in.
    let mut rows = a.list("rows").to_vec();
    if app.config.date_order == crate::DateOrder::Descending {
        rows.reverse();
    }
    let cols = vec![
        Column {
            width: 110,
            ..col("Date")
        },
        Column {
            width: 130,
            ..col("Action")
        },
        Column {
            weight: 2,
            ..col("Security")
        },
        Column {
            width: 90,
            ..col_num("Shares")
        },
        Column {
            width: 90,
            ..col_num("Price")
        },
        Column {
            width: 100,
            ..col_num("Commission")
        },
        Column {
            width: 120,
            ..col_num("Cash amount")
        },
        Column {
            width: 120,
            ..col_num("Cash balance")
        },
    ];
    let mut edit = None;
    ui.table("Activity", cols, |ui| {
        for r in &rows {
            let voided = r.at("voided").as_bool() == Some(true);
            let what = if r.at("action").is_null() {
                format!("Transfer {}", r.str("other"))
            } else {
                r.str("label").to_owned()
            };
            let summary = format!(
                "Activity {} {} {} {}, cash {}",
                fmt::date(r.str("date")),
                what,
                r.str("symbol"),
                r.str("shares"),
                r.str("cash")
            );
            let resp = ui.table_row(&summary, false, true, |ui| {
                ui.strike(voided);
                ui.label(fmt::date(r.str("date")));
                ui.strike(voided).tip(what.clone());
                ui.label(what.clone());
                ui.strike(voided);
                ui.label(r.str("symbol"));
                ui.strike(voided).mono();
                ui.amount(r.str("shares"));
                ui.strike(voided).mono();
                ui.amount(fmt::group(r.str("price")));
                ui.strike(voided).mono();
                ui.amount(fmt::group(r.str("commission")));
                ui.strike(voided);
                money(ui, r.str("cash"));
                ui.strike(voided);
                money(ui, r.str("balance"));
            });
            // A click (or Enter) opens the edit form (revision 24); options, bonds
            // and crypto open their own form (investments-options-bonds-crypto.md
            // revision 2).
            if resp.clicked()
                && !r.at("action").is_null()
                && !voided
                && r.at("reconciled").as_bool() != Some(true)
            {
                edit = Some((r.str("txn").to_owned(), r.str("action").to_owned()));
            }
        }
    });
    if let Some((t, action)) = edit
        && !super::inv_more::open_edit(app, &t, account)
        && ACTIONS.iter().any(|(k, _)| *k == action)
    {
        open_edit(app, &t, account);
    }
}

// ----- action forms ----------------------------------------------------------------------

pub(crate) fn open_form(app: &mut App, action: &str, account: &str) {
    let action = ACTIONS
        .iter()
        .find(|(k, _)| *k == action)
        .map_or("buy", |(k, _)| *k);
    app.inv.form = Some(ActionForm {
        action,
        account: account.to_owned(),
        date: fmt::date(&app.today()),
        lots_in: vec![[String::new(), String::new(), String::new()]],
        ratio_new: "2".into(),
        ratio_old: "1".into(),
        ..ActionForm::default()
    });
}

/// Open an investment transaction's form pre-filled from its stored arguments.
fn open_edit(app: &mut App, txn: &str, account: &str) {
    let Some(m) = app.read("inv.get", json!({"txn": txn})) else {
        return;
    };
    let a = m.at("args");
    let action = ACTIONS
        .iter()
        .find(|(k, _)| *k == m.str("action"))
        .map_or("buy", |(k, _)| *k);
    let security = app
        .backend
        .invoke("security.get", json!({"security": a.str("security")}))
        .map(|s| s.str("symbol").to_owned())
        .unwrap_or_else(|_| a.str("security").to_owned());
    let lots = a.list("lots").to_vec();
    let mut f = ActionForm {
        action,
        editing: Some(txn.to_owned()),
        account: account.to_owned(),
        date: fmt::date(a.str("date")),
        security,
        quantity: a.str("quantity").to_owned(),
        price: a.str("price").to_owned(),
        commission: a.str("commission").to_owned(),
        total: a.str("amount").to_owned(),
        total_edited: !a.str("amount").is_empty() && matches!(action, "buy" | "sell" | "reinvest"),
        kind: index_of(&KINDS, a.str("kind")),
        ratio_new: a.str("ratio_new").to_owned(),
        ratio_old: a.str("ratio_old").to_owned(),
        ..ActionForm::default()
    };
    if action == "shares_in" {
        f.lots_in = lots
            .iter()
            .map(|l| {
                [
                    l.str("acquired").to_owned(),
                    l.str("quantity").to_owned(),
                    l.str("basis").to_owned(),
                ]
            })
            .collect();
    } else if !lots.is_empty() {
        f.choose = true;
        f.lot_rows = lots
            .iter()
            .map(|l| {
                (
                    l.str("lot").to_owned(),
                    String::new(),
                    String::new(),
                    String::new(),
                    l.str("quantity").to_owned(),
                )
            })
            .collect();
    }
    app.inv.form = Some(f);
}

fn has(f: &ActionForm, field: &str) -> bool {
    let a = f.action;
    match field {
        "quantity" => matches!(a, "buy" | "sell" | "reinvest" | "shares_in" | "shares_out"),
        "price" => matches!(a, "buy" | "sell" | "reinvest"),
        "commission" => matches!(a, "buy" | "sell"),
        "total" => matches!(a, "buy" | "sell" | "reinvest"),
        "amount" => matches!(a, "dividend" | "interest" | "fee" | "return_of_capital"),
        "kind" => matches!(a, "dividend" | "reinvest"),
        "ratio" => a == "split",
        _ => false,
    }
}

fn put(a: &mut Value, k: &str, v: &str) {
    let v = v.trim().replace(',', "");
    if !v.is_empty() {
        set(a, k, json!(v));
    }
}

/// The command arguments from the form's text.
fn form_args(f: &ActionForm) -> Value {
    let mut a = json!({"date": fmt::date_arg_lenient(&f.date)});
    if f.action != "split" {
        put(&mut a, "account", &f.account);
    }
    put(&mut a, "security", &f.security);
    if has(f, "quantity") {
        put(&mut a, "quantity", &f.quantity);
    }
    if has(f, "price") {
        put(&mut a, "price", &f.price);
    }
    if has(f, "commission") {
        put(&mut a, "commission", &f.commission);
    }
    if (has(f, "total") && f.total_edited) || has(f, "amount") {
        put(&mut a, "amount", &f.total);
    }
    if has(f, "kind") {
        set(
            &mut a,
            "kind",
            json!(KINDS.get(f.kind).map_or("ordinary", |k| k.0)),
        );
    }
    if has(f, "ratio") {
        put(&mut a, "ratio_new", &f.ratio_new);
        put(&mut a, "ratio_old", &f.ratio_old);
    }
    if f.action == "shares_in" {
        let lots: Vec<Value> = f
            .lots_in
            .iter()
            .filter(|l| l.iter().any(|x| !x.trim().is_empty()))
            .map(|l| {
                json!({"acquired": l[0].trim(), "quantity": l[1].trim().replace(',', ""),
                       "basis": l[2].trim().replace(',', "")})
            })
            .collect();
        set(&mut a, "lots", json!(lots));
    }
    if matches!(f.action, "sell" | "shares_out") && f.choose {
        let lots: Vec<Value> = f
            .lot_rows
            .iter()
            .filter(|r| !r.4.trim().is_empty())
            .map(|r| json!({"lot": r.0, "quantity": r.4.trim().replace(',', "")}))
            .collect();
        set(&mut a, "lots", json!(lots));
    }
    a
}

fn hint(key: &str) -> &'static str {
    HINTS
        .iter()
        .find(|(k, _)| *k == key)
        .map_or("", |(_, h)| *h)
}

fn needs_security(f: &ActionForm) -> bool {
    !matches!(f.action, "interest" | "fee")
}

/// The first required field still empty, with a plain message for it.
fn missing(f: &ActionForm) -> Option<(&'static str, &'static str)> {
    let empty = |s: &str| s.trim().is_empty();
    if needs_security(f) && empty(&f.security) {
        return Some(("security", "Choose a security"));
    }
    if has(f, "quantity") && empty(&f.quantity) {
        return Some(("quantity", "Enter the number of shares"));
    }
    if has(f, "price") && empty(&f.price) && !f.total_edited {
        return Some(("price", "Enter the price per share"));
    }
    if has(f, "amount") && empty(&f.total) {
        return Some(("amount", "Enter the amount"));
    }
    if has(f, "ratio") && (empty(&f.ratio_new) || empty(&f.ratio_old)) {
        return Some(("ratio", "Enter the split ratio"));
    }
    if f.action == "shares_in" && f.lots_in.iter().all(|l| l.iter().all(|x| empty(x))) {
        return Some(("lots", "Enter the lots received"));
    }
    None
}

/// A command error in plain words, with the field it concerns.
fn plain_error(e: &crate::UiError, f: &ActionForm) -> (Option<&'static str>, String) {
    let msg = e.message.as_str();
    if e.code == "insufficient_shares" {
        let held = msg
            .rsplit_once(": ")
            .map(|(_, h)| h.trim_end_matches(" held").to_owned())
            .unwrap_or_default();
        let sym = f.security.trim().to_ascii_uppercase();
        return (
            Some("quantity"),
            format!("You hold {held} shares of {sym} in this account on that date"),
        );
    }
    let m = msg.strip_prefix("invalid arguments: ").unwrap_or(msg);
    let pick = [
        ("give a quantity", "quantity", "Enter the number of shares"),
        (
            "the quantity must be",
            "quantity",
            "Enter a number of shares above zero",
        ),
        ("give a price", "price", "Enter the price per share"),
        ("the price must be", "price", "Enter a price above zero"),
        ("give the security", "security", "Choose a security"),
        ("security \"", "security", "No security has that symbol"),
        ("give the amount", "amount", "Enter the amount"),
        ("the amount must be", "amount", "Enter an amount above zero"),
        ("date", "date", ""),
    ];
    for (needle, field, text) in pick {
        if m.contains(needle) {
            // Dates are asked for in the chosen format (revision 20).
            let text = if field == "date" {
                fmt::date_style().error()
            } else {
                text.to_owned()
            };
            return (Some(field), text);
        }
    }
    let mut c = m.chars();
    let text = c.next().map_or(String::new(), |x| {
        x.to_uppercase().collect::<String>() + c.as_str()
    });
    (None, text)
}

fn refresh_preview(app: &mut App, f: &mut ActionForm) {
    // Securities held in the account on the form's date (Sell, Move shares out).
    if matches!(f.action, "sell" | "shares_out") {
        let hkey = format!("{}|{}|{}", app.generation, f.account, f.date.trim());
        if hkey != f.held_key {
            f.held_key = hkey;
            f.held = app
                .backend
                .invoke(
                    "inv.holdings",
                    json!({"accounts": [f.account], "as_of": fmt::date_arg_lenient(&f.date)}),
                )
                .map(|h| {
                    h.at("accounts")
                        .as_array()
                        .and_then(|a| a.first().cloned())
                        .map(|a| {
                            a.list("holdings")
                                .iter()
                                .filter(|x| x.str("quantity") != "0")
                                .map(|x| (x.str("symbol").to_owned(), x.str("quantity").to_owned()))
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .unwrap_or_default();
        }
    }
    if missing(f).is_some() {
        f.preview = None;
        f.preview_key.clear();
        f.preview_error = None;
        return;
    }
    let mut args = form_args(f);
    set(&mut args, "action", json!(f.action));
    if let Some(t) = &f.editing {
        set(&mut args, "txn", json!(t));
    }
    let key = format!("{}|{args}", app.generation);
    if key != f.preview_key {
        f.preview_key = key;
        match app.backend.invoke("inv.preview", args) {
            Ok(p) => {
                if has(f, "total") && !f.total_edited {
                    f.total = p.at("investment").str("amount").to_owned();
                }
                // A sale at a loss: purchases in the last 30 days (wash-sale notice).
                let loss = f.action == "sell" && fmt::is_negative(p.at("disposal").str("gain"));
                f.wash = if loss {
                    app.backend
                        .invoke(
                            "inv.wash_check",
                            json!({"security": f.security.trim(), "date": fmt::date_arg_lenient(&f.date),
                                   "quantity": f.quantity.trim()}),
                        )
                        .ok()
                } else {
                    None
                };
                f.preview = Some(p);
                f.preview_error = None;
            }
            Err(e) => {
                f.preview = None;
                f.preview_error = Some(plain_error(&e, f));
            }
        }
    }
    // Choose lots…: the open lots on the sale date.
    if matches!(f.action, "sell" | "shares_out") && f.choose {
        let lkey = format!("{}|{}|{}|{}", app.generation, f.account, f.security, f.date);
        if lkey != f.lots_key && !f.security.trim().is_empty() {
            f.lots_key = lkey;
            let open = app
                .backend
                .invoke(
                    "inv.lots",
                    json!({"account": f.account, "security": f.security.trim(), "as_of": fmt::date_arg_lenient(&f.date)}),
                )
                .map(|v| v.list("lots").to_vec())
                .unwrap_or_default();
            let old = std::mem::take(&mut f.lot_rows);
            f.lot_rows = open
                .iter()
                .map(|l| {
                    let typed = old
                        .iter()
                        .find(|o| o.0 == l.str("lot"))
                        .map(|o| o.4.clone())
                        .unwrap_or_default();
                    (
                        l.str("lot").to_owned(),
                        l.str("acquired").to_owned(),
                        l.str("quantity").to_owned(),
                        l.str("basis").to_owned(),
                        typed,
                    )
                })
                .collect();
        }
    }
}

/// The error shown next to `key`, if any.
fn field_error(f: &ActionForm, key: &str) -> Option<String> {
    let from_preview = f
        .preview_error
        .as_ref()
        .filter(|(k, _)| *k == Some(key))
        .map(|(_, m)| m.clone());
    let from_save = f
        .error
        .as_ref()
        .filter(|(k, _)| *k == Some(key))
        .map(|(_, m)| m.clone());
    from_save.or(from_preview)
}

/// A labelled number field with its error under it.
fn number_row(
    ui: &mut Ui,
    f_err: Option<String>,
    label: &str,
    name: &str,
    hint_key: &str,
    text: &mut String,
) -> bool {
    let changed = ui.form(label, |ui| {
        ui.width(300)
            .mono()
            .align(super::ui::Align::Right)
            .hint(hint(hint_key));
        ui.field(name, text).changed()
    });
    if let Some(e) = f_err {
        ui.form("", |ui| ui.error_text(e));
    }
    changed
}

fn form_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.inv.form.take() else {
        return;
    };
    refresh_preview(app, &mut f);
    let held_only = matches!(f.action, "sell" | "shares_out");
    // The Security drop-down (revision 10b): held securities for Sell and Move
    // shares out, every security otherwise; a single choice is preselected.
    let mut items: Vec<(String, String)> = if held_only {
        f.held
            .iter()
            .map(|(sym, q)| (sym.clone(), format!("{sym} — {q} held")))
            .collect()
    } else {
        super::util::array(app.read("security.list", json!({})))
            .iter()
            .map(|s| {
                (
                    s.str("symbol").to_owned(),
                    format!("{} — {}", s.str("symbol"), s.str("name")),
                )
            })
            .collect()
    };
    if !f.preselected
        && f.editing.is_none()
        && f.security.trim().is_empty()
        && held_only
        && items.len() == 1
        && let Some((sym, _)) = items.first()
    {
        f.security = sym.clone();
    }
    f.preselected = true;
    if !needs_security(&f) {
        items.insert(0, (String::new(), "No security".to_owned()));
    }
    let title = form_title(f.action, f.editing.is_some());
    let (mut save, mut cancel, mut delete, mut void, mut create_security) =
        (false, false, false, false, false);
    cancel |= ui.dialog_closed("inv-form");
    if ui.key("escape") {
        cancel = true;
    }
    let ready = f.preview.is_some();
    ui.dialog("inv-form", &title, 600, |ui| {
        ui.group(|ui| {
            let e = field_error(&f, "date");
            ui.form("Date", |ui| {
                super::register::date_field(ui, "Date field", &mut f.date)
            });
            if let Some(e) = e {
                ui.form("", |ui| ui.error_text(e));
            }
            let e = field_error(&f, "security");
            ui.form("Security", |ui| {
                let opts: Vec<String> = items.iter().map(|(_, l)| l.clone()).collect();
                let mut idx = items
                    .iter()
                    .position(|(sym, _)| {
                        !sym.is_empty() && sym.eq_ignore_ascii_case(f.security.trim())
                    })
                    .or_else(|| (!needs_security(&f) && f.security.trim().is_empty()).then_some(0))
                    .unwrap_or(usize::MAX);
                ui.width(300);
                if ui.combo("Security field", &mut idx, &opts).changed()
                    && let Some((sym, _)) = items.get(idx)
                {
                    f.security = sym.clone();
                }
                if idx == usize::MAX
                    && let Some(it) = ui.last()
                {
                    it.value = "Choose a security".into();
                }
                if !held_only && ui.button("New security…").clicked() {
                    f.new_security = Some([String::new(), String::new()]);
                }
            });
            if held_only && f.held.is_empty() {
                ui.form("", |ui| {
                    ui.secondary("No shares are held in this account on that date.")
                });
            }
            if let Some(ns) = &mut f.new_security {
                ui.form("", |ui| {
                    ui.width(90);
                    ui.field("New symbol", &mut ns[0]);
                    ui.width(200);
                    ui.field("New security name", &mut ns[1]);
                });
                ui.form("", |ui| {
                    super::form::choice(
                        ui,
                        "New security kind",
                        &mut f.new_kind,
                        &SEC_KINDS.map(|k| k.1),
                    );
                    if ui.button("Create security").clicked() {
                        create_security = true;
                    }
                });
            }
            if let Some(e) = e {
                ui.form("", |ui| ui.error_text(e));
            }
            if has(&f, "quantity") {
                number_row(
                    ui,
                    field_error(&f, "quantity"),
                    "Shares",
                    "Shares field",
                    "quantity",
                    &mut f.quantity,
                );
            }
            if has(&f, "price") {
                number_row(
                    ui,
                    field_error(&f, "price"),
                    "Price",
                    "Price field",
                    "price",
                    &mut f.price,
                );
            }
            if has(&f, "commission") {
                number_row(
                    ui,
                    field_error(&f, "commission"),
                    "Commission",
                    "Commission field",
                    "commission",
                    &mut f.commission,
                );
            }
            if has(&f, "kind") {
                ui.form("Kind", |ui| {
                    super::form::choice(ui, "Dividend kind", &mut f.kind, &KINDS.map(|k| k.1));
                });
            }
            if has(&f, "amount") {
                number_row(
                    ui,
                    field_error(&f, "amount"),
                    "Amount",
                    "Amount field",
                    "amount",
                    &mut f.total,
                );
            }
            if has(&f, "ratio") {
                number_row(
                    ui,
                    field_error(&f, "ratio"),
                    "New shares",
                    "New shares field",
                    "ratio",
                    &mut f.ratio_new,
                );
                number_row(
                    ui,
                    field_error(&f, "ratio_old"),
                    "For old shares",
                    "Old shares field",
                    "ratio",
                    &mut f.ratio_old,
                );
            }
            if has(&f, "total") {
                let err = field_error(&f, "total");
                let changed = ui.form("Total", |ui| {
                    ui.width(300)
                        .mono()
                        .align(super::ui::Align::Right)
                        .hint(hint("total"));
                    ui.tip("Overwrite to match the broker's statement");
                    ui.field("Total field", &mut f.total).changed()
                });
                if changed {
                    f.total_edited = !f.total.trim().is_empty();
                }
                if let Some(e) = err {
                    ui.form("", |ui| ui.error_text(e));
                }
            }
        });
        if f.action == "shares_in" {
            ui.heading("Lots received");
            if let Some(e) = field_error(&f, "lots") {
                ui.error_text(e);
            }
            for (i, l) in f.lots_in.iter_mut().enumerate() {
                ui.row(|ui| {
                    ui.width(120);
                    ui.field(&format!("Lot {} acquired", i + 1), &mut l[0]);
                    ui.width(110).mono();
                    ui.field(&format!("Lot {} shares", i + 1), &mut l[1]);
                    ui.width(120).mono();
                    ui.field(&format!("Lot {} basis", i + 1), &mut l[2]);
                });
            }
            let add = ui.row(|ui| ui.button("Add lot").clicked());
            if add || f.lots_in.is_empty() {
                f.lots_in
                    .push([String::new(), String::new(), String::new()]);
            }
        }
        if held_only {
            sell_preview(ui, &mut f);
        }
        let general = f
            .error
            .as_ref()
            .filter(|(k, _)| k.is_none())
            .or(f.preview_error.as_ref().filter(|(k, _)| k.is_none()))
            .map(|(_, m)| m.clone());
        if let Some(e) = general {
            ui.error_text(e);
        }
        if f.confirm_delete {
            ui.label("Delete this transaction? You can undo it afterwards.");
            ui.row(|ui| {
                if ui.button("Delete transaction").clicked() {
                    delete = true;
                }
                if ui.button("Keep it").clicked() {
                    f.confirm_delete = false;
                }
            });
        }
        ui.actions(|ui| {
            if f.editing.is_some() {
                if ui.button("Delete…").clicked() {
                    f.confirm_delete = true;
                }
                if ui.button("Void").clicked() {
                    void = true;
                }
                ui.spacer();
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save").clicked() {
                save = true;
            }
        });
    });
    if create_security && let Some(ns) = f.new_security.clone() {
        let kind = SEC_KINDS.get(f.new_kind).map_or("stock", |k| k.0);
        match app.run(
            "security.create",
            json!({"symbol": ns[0].trim(), "name": ns[1].trim(), "kind": kind}),
        ) {
            Ok(s) => {
                f.security = s.str("symbol").to_owned();
                f.new_security = None;
                f.error = None;
            }
            Err(e) => f.error = Some(plain_error(&e, &f)),
        }
    }
    if save {
        if let Some((key, text)) = missing(&f) {
            f.error = Some((Some(key), text.to_owned()));
            save = false;
        } else if !ready {
            save = false;
        }
    }
    let txn = f.editing.clone().unwrap_or_default();
    let result = if delete {
        Some(app.run("txn.delete", json!({"id": txn})))
    } else if void {
        Some(app.run("txn.void", json!({"id": txn})))
    } else if save {
        let mut args = form_args(&f);
        match &f.editing {
            Some(t) => {
                set(&mut args, "txn", json!(t));
                set(&mut args, "action", json!(f.action));
                Some(app.run("inv.update", args))
            }
            None => Some(app.run(&format!("inv.{}", f.action), args)),
        }
    } else {
        None
    };
    match result {
        Some(Ok(_)) => {
            app.notify_with(
                format!("{} saved.", label_of(&ACTIONS, f.action)),
                MessageAction::Undo,
            );
        }
        Some(Err(e)) => {
            f.error = Some(plain_error(&e, &f));
            app.inv.form = Some(f);
        }
        None if cancel => {}
        None => app.inv.form = Some(f),
    }
}

/// The Sell form's lots and estimated gains (from `inv.preview`).
fn sell_preview(ui: &mut Ui, f: &mut ActionForm) {
    let method = f
        .preview
        .as_ref()
        .map(|p| p.at("investment").str("method").to_owned())
        .unwrap_or_default();
    ui.row(|ui| {
        if f.choose {
            ui.label("Method: chosen lots");
        } else if !method.is_empty() {
            ui.label(format!("Method: {}", label_of(&METHODS, &method)));
        }
        let mut choose = f.choose;
        if ui.checkbox(&mut choose, "Choose lots…").changed() {
            f.choose = choose;
            f.lots_key.clear();
            f.preview_key.clear();
        }
    });
    if f.choose {
        let cols = vec![
            Column {
                width: 110,
                ..col("Acquired")
            },
            Column {
                width: 100,
                ..col_num("Shares")
            },
            Column {
                width: 110,
                ..col_num("Basis")
            },
            Column {
                width: 130,
                ..col_num("Shares to sell")
            },
        ];
        ui.table("Choose lots", cols, |ui| {
            for (i, r) in f.lot_rows.iter_mut().enumerate() {
                ui.table_row(&format!("lot{i}"), false, false, |ui| {
                    ui.label(r.1.clone());
                    ui.mono();
                    ui.amount(r.2.clone());
                    ui.mono();
                    ui.amount(fmt::group(&r.3));
                    ui.mono().align(super::ui::Align::Right).hint("0");
                    ui.field(&format!("Sell from lot {}", r.1), &mut r.4);
                });
            }
        });
    }
    let Some(p) = &f.preview else {
        return;
    };
    let d = p.at("disposal");
    if d.is_null() {
        return;
    }
    let cols = vec![
        Column {
            width: 110,
            ..col("Acquired")
        },
        Column {
            width: 100,
            ..col_num("Shares")
        },
        Column {
            width: 110,
            ..col_num("Basis")
        },
        Column {
            width: 100,
            ..col("Term")
        },
        Column {
            width: 110,
            ..col_num("Gain")
        },
    ];
    ui.table("Lots sold", cols, |ui| {
        for (i, portion) in d.list("portions").iter().enumerate() {
            ui.table_row(&format!("portion{i}"), false, false, |ui| {
                ui.label(portion.str("acquired"));
                ui.mono();
                ui.amount(portion.str("quantity"));
                ui.mono();
                ui.amount(fmt::group(portion.str("basis")));
                ui.label(if portion.str("term") == "long" {
                    "Long-term"
                } else {
                    "Short-term"
                });
                ui.mono();
                ui.amount(fmt::group(portion.str("gain")));
            });
        }
    });
    if f.action == "sell" {
        ui.label(format!(
            "Short-term gain {}",
            fmt::group(d.str("short_term_gain"))
        ));
        ui.label(format!(
            "Long-term gain {}",
            fmt::group(d.str("long_term_gain"))
        ));
        ui.secondary(NOTE);
        if let Some(w) = &f.wash {
            ui.card("Possible wash sale", |ui| {
                let recent = w.list("recent");
                if recent.is_empty() {
                    ui.label(
                        "No purchases of this security (or its wash group) in the last 30 days.",
                    );
                } else {
                    ui.label("Purchases in the last 30 days:");
                    for r in recent {
                        ui.label(format!(
                            "{}  {} shares of {}  ({})",
                            fmt::date(r.str("date")),
                            r.str("quantity"),
                            r.str("symbol"),
                            r.str("account")
                        ));
                    }
                }
                ui.label(w.str("warning"));
            });
        }
    }
}

fn lots_dialog(app: &mut App, ui: &mut Ui) {
    let Some((account, symbol)) = app.inv.lots_modal.clone() else {
        return;
    };
    let as_of = fmt::date_arg_lenient(&app.inv.as_of);
    let data = app.read(
        "inv.lots",
        json!({"account": account, "security": symbol, "as_of": as_of}),
    );
    let mut close = ui.dialog_closed("inv-lots") || ui.key("escape");
    ui.dialog("inv-lots", &format!("Lots of {symbol}"), 720, |ui| {
        if let Some(d) = &data {
            let cols = vec![
                Column {
                    width: 110,
                    ..col("Acquired")
                },
                Column {
                    width: 100,
                    ..col_num("Shares")
                },
                Column {
                    width: 110,
                    ..col_num("Basis")
                },
                Column {
                    width: 100,
                    ..col_num("Per share")
                },
                Column {
                    width: 100,
                    ..col("Term")
                },
                Column {
                    width: 130,
                    ..col_num("Unrealised gain")
                },
            ];
            ui.table("", cols, |ui| {
                for (i, l) in d.list("lots").iter().enumerate() {
                    ui.table_row(&format!("lot{i}"), false, false, |ui| {
                        ui.label(l.str("acquired"));
                        ui.mono();
                        ui.amount(l.str("quantity"));
                        ui.mono();
                        ui.amount(fmt::group(l.str("basis")));
                        ui.mono();
                        ui.amount(fmt::group(l.str("basis_per_share")));
                        ui.label(if l.str("term") == "long" {
                            "Long-term"
                        } else {
                            "Short-term"
                        });
                        ui.mono();
                        ui.amount(fmt::group(l.str("unrealized_gain")));
                    });
                }
            });
        }
        ui.actions(|ui| {
            if ui.button("Close").clicked() {
                close = true;
            }
        });
    });
    if close {
        app.inv.lots_modal = None;
    }
}

// ----- securities ------------------------------------------------------------------------

pub(crate) fn securities_view(app: &mut App, ui: &mut Ui) {
    if !enabled(app) {
        super::dashboard::view(app, ui, None);
        return;
    }
    ui.title("Securities");
    let list = super::util::array(app.read("security.list", json!({})));
    ui.toolbar(|ui| {
        if ui.button("New security…").clicked() {
            app.inv.sec_form = Some(SecForm::default());
        }
        let sel = app.inv.selected_security.clone();
        ui.enabled(sel.is_some());
        if ui.button("Edit…").clicked()
            && let Some(s) = list.iter().find(|s| Some(s.str("id").to_owned()) == sel)
        {
            app.inv.sec_form = Some(SecForm {
                editing: Some(s.str("id").to_owned()),
                symbol: s.str("symbol").to_owned(),
                name: s.str("name").to_owned(),
                kind: index_of(&SEC_KINDS, s.str("kind")),
                method: index_of(&METHODS, s.str("basis_method")),
                cusip: s.str("cusip").to_owned(),
                coupon: s.at("bond").str("coupon_rate").to_owned(),
                maturity: fmt::date(s.at("bond").str("maturity")),
                face: s.at("bond").str("face_per_unit").to_owned(),
                per_year: s
                    .at("bond")
                    .at("payments_per_year")
                    .to_string()
                    .replace("null", ""),
                tax: index_of(&TAXES, s.at("bond").str("tax")),
                ..SecForm::default()
            });
        }
        if ui.button("Update prices…").clicked() {
            let today = fmt::date(&app.today());
            app.inv.prices = Some(
                list.iter()
                    .filter(|s| !s.str("shares").is_empty() && s.str("shares") != "0")
                    .map(|s| (s.str("symbol").to_owned(), today.clone(), String::new()))
                    .collect(),
            );
            app.inv.prices_error = None;
        }
        if ui.button("Import prices…").clicked() {
            app.emit(AppEvent::ChoosePriceFile);
        }
        if ui.button("Wash groups…").clicked() {
            app.inv.wash = Some(super::corporate::WashForm::default());
        }
    });
    let from = super::inv_import::provenance(app);
    let cols = vec![
        Column {
            width: 90,
            ..col("Symbol")
        },
        Column {
            weight: 3,
            ..col("Name")
        },
        Column {
            width: 120,
            ..col("Kind")
        },
        Column {
            width: 150,
            ..col("Basis method")
        },
        Column {
            width: 170,
            ..col_num("Latest price")
        },
        Column {
            width: 100,
            ..col_num("Shares held")
        },
    ];
    let mut pick = None;
    ui.table("Securities", cols, |ui| {
        for s in &list {
            let id = s.str("id").to_owned();
            let selected = app.inv.selected_security.as_deref() == Some(id.as_str());
            let summary = format!("Security {} {}", s.str("symbol"), s.str("name"));
            let r = ui.table_row(&summary, selected, true, |ui| {
                ui.label(s.str("symbol"));
                let name = match from.get(&("security".to_owned(), id.to_lowercase())) {
                    Some(pkg) => format!("{}  · from {pkg}", s.str("name")),
                    None => s.str("name").to_owned(),
                };
                ui.tip(name.clone());
                ui.label(name);
                ui.label(label_of(&SEC_KINDS, s.str("kind")));
                ui.label(label_of(&METHODS, s.str("basis_method")));
                let latest = s.at("latest_price");
                let lp = if latest.is_null() {
                    "no price".to_owned()
                } else {
                    format!(
                        "{}  {}",
                        fmt::group(latest.str("price")),
                        fmt::date(latest.str("date"))
                    )
                };
                ui.mono();
                ui.amount(lp);
                ui.mono();
                ui.amount(s.str("shares"));
            });
            if r.clicked() {
                pick = Some(id);
            }
        }
    });
    if pick.is_some() {
        app.inv.selected_security = pick;
    }
    if let Some(sel) = app.inv.selected_security.clone()
        && let Some(s) = list.iter().find(|s| s.str("id") == sel)
    {
        let prices = super::util::array(app.read("price.list", json!({"security": sel})));
        ui.card(&format!("Price history: {}", s.str("symbol")), |ui| {
            for p in prices.iter().rev().take(200) {
                ui.mono();
                ui.label(format!(
                    "{}  {}",
                    fmt::date(p.str("date")),
                    fmt::group(p.str("price"))
                ));
            }
        });
    }
    security_form(app, ui);
    prices_dialog(app, ui);
    super::corporate::wash_dialog(app, ui);
}

fn security_form(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.inv.sec_form.take() else {
        return;
    };
    let mut save = false;
    let mut cancel = ui.dialog_closed("security-form") || ui.key("escape");
    let title = if f.editing.is_some() {
        "Edit security"
    } else {
        "New security"
    };
    ui.dialog("security-form", title, 520, |ui| {
        ui.group(|ui| {
            ui.form("Symbol", |ui| {
                ui.width(120);
                ui.field("Security symbol", &mut f.symbol);
            });
            ui.form("Name", |ui| {
                ui.width(260);
                ui.field("Security name", &mut f.name);
            });
            ui.form("Kind", |ui| {
                super::form::choice(ui, "Security kind", &mut f.kind, &SEC_KINDS.map(|k| k.1));
            });
            let kind = SEC_KINDS.get(f.kind).map_or("stock", |k| k.0);
            if kind == "option" && f.editing.is_none() {
                ui.form("Underlying", |ui| {
                    ui.width(120).hint("AAPL");
                    ui.field("Option underlying", &mut f.underlying);
                });
                ui.form("Call or put", |ui| {
                    ui.radio(&mut f.put, false, "Call");
                    ui.radio(&mut f.put, true, "Put");
                });
                ui.form("Strike", |ui| {
                    ui.width(120).mono();
                    ui.field("Option strike", &mut f.strike);
                });
                ui.form("Expiration", |ui| {
                    super::register::date_field(ui, "Option expiration", &mut f.expiration)
                });
                ui.form("Shares per contract", |ui| {
                    ui.width(120).mono().hint("100");
                    ui.field("Option multiplier", &mut f.multiplier);
                });
                ui.secondary("The symbol and name are made from these, e.g. AAPL 261218 C200.");
            }
            if kind == "bond" && f.editing.is_none() {
                ui.form("Coupon rate %", |ui| {
                    ui.width(120).mono().hint("4.25");
                    ui.field("Bond coupon", &mut f.coupon);
                });
                ui.form("Maturity", |ui| {
                    super::register::date_field(ui, "Bond maturity", &mut f.maturity)
                });
                ui.form("Face per bond", |ui| {
                    ui.width(120).mono().hint("1,000");
                    ui.field("Bond face", &mut f.face);
                });
                ui.form("Coupons a year", |ui| {
                    ui.width(120).mono().hint("2");
                    ui.field("Bond coupons a year", &mut f.per_year);
                });
                ui.form("Interest is", |ui| {
                    super::form::choice(ui, "Bond interest is", &mut f.tax, &TAXES.map(|t| t.1));
                });
            }
            ui.form("Basis method", |ui| {
                super::form::choice(ui, "Basis method", &mut f.method, &METHODS.map(|k| k.1));
            });
            ui.form("CUSIP", |ui| {
                ui.width(140);
                ui.field("Security CUSIP", &mut f.cusip);
            });
        });
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save security").clicked() {
                save = true;
            }
        });
    });
    if save {
        let kind = SEC_KINDS.get(f.kind).map_or("stock", |k| k.0);
        let method = METHODS.get(f.method).map_or("fifo", |k| k.0);
        let mut args = json!({"symbol": f.symbol.trim(), "name": f.name.trim(), "kind": kind, "basis_method": method});
        if !f.cusip.trim().is_empty() {
            set(&mut args, "cusip", json!(f.cusip.trim()));
        }
        let put_text = |a: &mut Value, k: &str, v: &str| {
            let v = v.trim().replace(',', "");
            if !v.is_empty() {
                set(a, k, json!(v));
            }
        };
        if f.editing.is_none() && kind == "option" {
            put_text(&mut args, "underlying", &f.underlying);
            set(
                &mut args,
                "right",
                json!(if f.put { "put" } else { "call" }),
            );
            put_text(&mut args, "strike", &f.strike);
            put_text(
                &mut args,
                "expiration",
                &fmt::date_arg_lenient(&f.expiration),
            );
            put_text(&mut args, "multiplier", &f.multiplier);
        }
        if f.editing.is_none() && kind == "bond" && !f.maturity.trim().is_empty() {
            put_text(&mut args, "coupon_rate", &f.coupon);
            put_text(&mut args, "maturity", &fmt::date_arg_lenient(&f.maturity));
            put_text(&mut args, "face_per_unit", &f.face);
            if let Ok(n) = f.per_year.trim().parse::<u32>() {
                set(&mut args, "payments_per_year", json!(n));
            }
            set(
                &mut args,
                "tax",
                json!(TAXES.get(f.tax).map_or("taxable", |t| t.0)),
            );
        }
        let r = match &f.editing {
            Some(id) => {
                set(&mut args, "security", json!(id));
                app.run("security.update", args)
            }
            None => app.run("security.create", args),
        };
        match r {
            Ok(_) => return,
            Err(e) => f.error = Some(e.display()),
        }
    }
    if !cancel {
        app.inv.sec_form = Some(f);
    }
}

fn prices_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut rows) = app.inv.prices.take() else {
        return;
    };
    let mut save = false;
    let mut cancel = ui.dialog_closed("update-prices") || ui.key("escape");
    let error = app.inv.prices_error.clone();
    ui.dialog("update-prices", "Update prices", 560, |ui| {
        if rows.is_empty() {
            ui.label("No security is held.");
        }
        let cols = vec![
            Column {
                width: 110,
                ..col("Security")
            },
            Column {
                width: 150,
                ..col("Date")
            },
            Column {
                width: 140,
                ..col_num("Price")
            },
        ];
        ui.table("", cols, |ui| {
            for (sym, date, price) in rows.iter_mut() {
                ui.table_row(sym.as_str(), false, false, |ui| {
                    ui.label(sym.clone());
                    super::register::date_field(ui, &format!("Price date {sym}"), date);
                    ui.mono().align(super::ui::Align::Right);
                    ui.field(&format!("Price {sym}"), price);
                });
            }
        });
        if let Some(e) = &error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save prices").clicked() {
                save = true;
            }
        });
    });
    if save {
        let mut saved = 0;
        for (sym, date, price) in &rows {
            if price.trim().is_empty() {
                continue;
            }
            match app.run(
                "price.set",
                json!({"security": sym, "date": fmt::date_arg_lenient(date), "price": price.trim().replace(',', "")}),
            ) {
                Ok(_) => saved += 1,
                Err(e) => {
                    app.inv.prices_error = Some(format!("{sym}: {}", e.display()));
                    app.inv.prices = Some(rows);
                    return;
                }
            }
        }
        app.notify(format!("Saved {saved} price(s)."));
        return;
    }
    if !cancel {
        app.inv.prices = Some(rows);
    }
}

/// Import prices from a CSV file chosen in the app's file dialog.
pub(crate) fn import_prices(app: &mut App, name: &str, bytes: &[u8]) {
    use base64::Engine;
    let content = base64::engine::general_purpose::STANDARD.encode(bytes);
    match app.run(
        "price.import",
        json!({"file_name": name, "content_base64": content}),
    ) {
        Ok(v) => {
            let warnings: Vec<String> = v
                .list("warnings")
                .iter()
                .filter_map(|w| w.as_str().map(str::to_owned))
                .collect();
            let mut text = format!(
                "Imported {} price(s) from {name}; {} row(s) skipped.",
                v.at("stored"),
                v.at("skipped")
            );
            if !warnings.is_empty() {
                text.push(' ');
                text.push_str(&warnings.join(" "));
            }
            app.notify(text);
        }
        Err(e) => app.notify_error(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AT76: no hint text mentions "Action"; every action has a plain title.
    #[test]
    fn hints_and_titles_are_plain() {
        for (_, h) in HINTS {
            assert!(!h.contains("Action"), "{h}");
        }
        for (key, _) in ACTIONS {
            let t = form_title(key, false);
            assert!(!t.contains("Action") && !t.is_empty(), "{t}");
            assert!(form_title(key, true).starts_with("Edit"), "{key}");
        }
    }
}
