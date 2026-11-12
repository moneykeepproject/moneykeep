//! Sales (app-desktop.md revision 14): invoices, payments, estimates, customers, items
//! and sales tax, the invoice/estimate editor, Record payment, Make deposit, the
//! customer view and their dialogs. Every number and rule comes from the commands
//! (`invoice.preview`, `payment.open_invoices`, `deposit.preview`, …); this module only
//! collects input and shows results.

use serde_json::{Value, json};

use super::form::{amount_arg, choice, date_row, pick, put, set, text_row};
use super::lists::{accounts_where, names};
use super::tags::{self, Tags};
use super::ui::{Align, Column, Tone, Ui, col, col_num};
use super::util::{Json, array};
use super::{App, AppEvent, MessageAction};
use crate::fmt;

/// (tab title, module that must be on; None: invoicing itself).
const TABS: [(&str, Option<&str>); 6] = [
    ("Invoices", None),
    ("Payments", None),
    ("Estimates", Some("estimates")),
    ("Customers", None),
    ("Items", Some("items")),
    ("Sales tax", Some("sales-tax")),
];

/// Terms choices: label and the terms value (None: the customer's default).
pub(crate) const TERMS: [(&str, Option<i64>); 6] = [
    ("Customer default", None),
    ("Due on receipt", Some(0)),
    ("Net 15", Some(15)),
    ("Net 30", Some(30)),
    ("Net 45", Some(45)),
    ("Net 60", Some(60)),
];

const METHODS: [(&str, &str); 5] = [
    ("check", "Check"),
    ("cash", "Cash"),
    ("card", "Card"),
    ("bank_transfer", "Bank transfer"),
    ("other", "Other"),
];

const STATUSES: [(&str, &str); 6] = [
    ("", "All"),
    ("draft", "Draft"),
    ("open", "Open"),
    ("partial", "Partly paid"),
    ("paid", "Paid"),
    ("void", "Void"),
];

const ESTIMATE_STATUSES: [(&str, &str); 4] = [
    ("draft", "Draft"),
    ("sent", "Sent"),
    ("accepted", "Accepted"),
    ("declined", "Declined"),
];

const ITEM_KINDS: [(&str, &str); 4] = [
    ("service", "Service"),
    ("product", "Product"),
    ("other_charge", "Other charge"),
    ("discount", "Discount"),
];

/// One line of the editor as typed.
#[derive(Debug, Clone, Default)]
pub(crate) struct LineDraft {
    pub subtotal: bool,
    pub what: String,
    pub description: String,
    pub quantity: String,
    pub rate: String,
    pub amount: String,
    pub taxable: Option<bool>,
    pub tags: Tags,
    /// Billable project costs this line bills (projects.md).
    pub billable_items: Vec<String>,
}

/// The invoice/estimate editor.
#[derive(Debug, Clone, Default)]
pub(crate) struct Editor {
    pub kind: &'static str,
    pub id: Option<String>,
    pub status: String,
    pub number: String,
    pub customer: String,
    pub date: String,
    pub due_date: String,
    pub terms: usize,
    /// Tax picker: "" customer default, "-" none, else a rate name.
    pub tax: String,
    pub message: String,
    pub memo: String,
    pub lines: Vec<LineDraft>,
    pub preview_key: String,
    pub preview: Option<Value>,
    pub preview_error: Option<String>,
    pub error: Option<String>,
    pub confirm: Option<&'static str>,
    pub tags: Tags,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PaymentForm {
    pub customer: String,
    pub date: String,
    pub amount: String,
    pub method: usize,
    pub reference: String,
    pub deposit_to: String,
    /// (invoice id, number, balance, apply amount typed)
    pub applies: Vec<(String, String, String, String)>,
    pub edited: bool,
    pub suggest_key: String,
    pub info: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DepositForm {
    pub bank: String,
    pub date: String,
    pub chosen: Vec<String>,
    pub total: String,
    pub total_key: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CustomerForm {
    pub id: Option<String>,
    pub name: String,
    pub company: String,
    pub contact: String,
    pub email: String,
    pub phone: String,
    pub billing: String,
    pub terms: usize,
    pub tax: String,
    pub exempt: bool,
    pub note: String,
    pub notes: String,
    pub error: Option<String>,
    pub for_editor: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ItemForm {
    pub id: Option<String>,
    pub name: String,
    pub kind: usize,
    pub description: String,
    pub sku: String,
    pub price: String,
    pub percent: bool,
    pub account: String,
    pub taxable: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RateForm {
    pub id: Option<String>,
    pub group: bool,
    pub name: String,
    pub rate: String,
    pub agency: String,
    pub members: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PayTaxForm {
    pub from: String,
    pub date: String,
    pub amounts: Vec<(String, String, String)>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RefundForm {
    pub customer: String,
    pub from: String,
    pub amount: String,
    pub date: String,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct SalesState {
    pub tab: usize,
    pub status_filter: usize,
    pub customer_filter: String,
    pub overdue_only: bool,
    pub editor: Option<Editor>,
    pub payment: Option<PaymentForm>,
    pub deposit: Option<DepositForm>,
    pub customer: Option<String>,
    pub customer_form: Option<CustomerForm>,
    pub item_form: Option<ItemForm>,
    pub rate_form: Option<RateForm>,
    pub pay_tax: Option<PayTaxForm>,
    pub refund: Option<RefundForm>,
}

// ----- helpers --------------------------------------------------------------------------

fn on(app: &mut App, module: &str) -> bool {
    app.lists().modules.iter().any(|m| m == module)
}

/// The status chip words.
pub(crate) fn status_label(status: &str) -> &'static str {
    match status {
        "draft" => "Draft",
        "open" => "Open",
        "partial" => "Partly paid",
        "paid" => "Paid",
        "void" => "Void",
        "sent" => "Sent",
        "accepted" => "Accepted",
        "declined" => "Declined",
        "expired" => "Expired",
        "converted" => "Converted",
        _ => "",
    }
}

fn status_tone(status: &str) -> Tone {
    match status {
        "paid" | "accepted" | "converted" => Tone::Success,
        "void" | "declined" | "expired" => Tone::Secondary,
        "draft" => Tone::Normal,
        _ => Tone::Accent,
    }
}

pub(crate) fn terms_value(i: usize) -> Option<Value> {
    match TERMS.get(i).and_then(|t| t.1) {
        Some(0) => Some(json!({"type": "due_on_receipt"})),
        Some(d) => Some(json!({"type": "net", "days": d})),
        None => None,
    }
}

pub(crate) fn terms_index(v: &Value) -> usize {
    let days = match v.str("type") {
        "due_on_receipt" => Some(0),
        "net" => v.at("days").as_i64(),
        _ => None,
    };
    TERMS
        .iter()
        .position(|t| days.is_some() && t.1 == days)
        .unwrap_or(0)
}

/// An amount cell (number font, right-aligned, the exact value as a tip).
pub(crate) fn money(ui: &mut Ui, text: &str) {
    ui.tip(fmt::group(text));
    ui.amount(fmt::group(text));
}

/// A text cell with the full text as its tip.
pub(crate) fn cell(ui: &mut Ui, text: impl Into<String>) {
    let t = text.into();
    ui.tip(t.clone());
    ui.label(t);
}

/// A status chip.
pub(crate) fn chip(ui: &mut Ui, status: &str) {
    ui.badge(status_label(status), status_tone(status));
}

// ----- the view -------------------------------------------------------------------------

pub(crate) fn view(app: &mut App, ui: &mut Ui) {
    let modules = app.lists().modules.clone();
    ui.title("Sales");
    let tabs: Vec<&str> = TABS
        .iter()
        .filter(|(_, m)| m.is_none_or(|m| modules.iter().any(|x| x == m)))
        .map(|t| t.0)
        .collect();
    if app.sales.tab >= tabs.len() {
        app.sales.tab = 0;
    }
    ui.toolbar(|ui| {
        for (i, t) in tabs.iter().enumerate() {
            let selected = app.sales.tab == i && app.sales.editor.is_none();
            if ui.tab(*t, selected).clicked() {
                app.sales.tab = i;
                app.sales.editor = None;
                app.sales.customer = None;
            }
        }
    });
    if app.sales.editor.is_some() {
        ui.scope("editor", |ui| editor(app, ui));
    } else {
        let tab = tabs.get(app.sales.tab).copied().unwrap_or("Invoices");
        ui.scope(tab, |ui| match tab {
            "Payments" => payments_tab(app, ui),
            "Estimates" => estimates_tab(app, ui),
            "Customers" => match app.sales.customer.clone() {
                Some(id) => customer_view(app, ui, &id),
                None => customers_tab(app, ui),
            },
            "Items" => items_tab(app, ui),
            "Sales tax" => tax_tab(app, ui),
            _ => invoices_tab(app, ui),
        });
    }
    dialogs(app, ui);
}

pub(crate) fn new_editor(app: &mut App, kind: &'static str, customer: &str) {
    app.sales.editor = Some(Editor {
        kind,
        status: "draft".into(),
        customer: customer.to_owned(),
        date: fmt::date(&app.today()),
        lines: vec![LineDraft::default()],
        ..Editor::default()
    });
}

pub(crate) fn open_editor(app: &mut App, kind: &'static str, id: &str) {
    let cmd = format!("{kind}.get");
    let Some(d) = app.read(&cmd, json!({kind: id})) else {
        return;
    };
    let l = app.lists().clone();
    let lines = d
        .list("lines")
        .iter()
        .map(|x| {
            let what = if x.str("item").is_empty() {
                x.str("account")
            } else {
                x.str("item")
            };
            let what = l
                .items
                .iter()
                .find(|i| i.str("id") == what)
                .map(|i| i.str("name").to_owned())
                .or_else(|| {
                    l.accounts
                        .iter()
                        .find(|a| a.str("id") == what)
                        .map(|a| a.str("full_name").to_owned())
                })
                .unwrap_or_else(|| what.to_owned());
            LineDraft {
                subtotal: x.at("subtotal").as_bool() == Some(true),
                what,
                description: x.str("description").to_owned(),
                quantity: x.str("quantity").to_owned(),
                rate: x.str("rate").to_owned(),
                amount: x.str("amount").to_owned(),
                taxable: x.at("taxable").as_bool(),
                tags: Tags::from_names(x),
                billable_items: x
                    .list("billable_items")
                    .iter()
                    .filter_map(|k| k.as_str().map(str::to_owned))
                    .collect(),
            }
        })
        .collect();
    let tax = match d.at("tax_rate").as_str() {
        Some(id) => l
            .rates
            .iter()
            .find(|r| r.str("id") == id)
            .map(|r| r.str("name").to_owned())
            .unwrap_or_default(),
        None => "-".into(),
    };
    app.sales.editor = Some(Editor {
        kind,
        id: Some(d.str("id").to_owned()),
        status: d.str("status").to_owned(),
        number: d.str("number").to_owned(),
        customer: d.str("customer_name").to_owned(),
        date: fmt::date(d.str("date")),
        due_date: fmt::date(d.str("due_date")),
        terms: terms_index(d.at("terms")),
        tax,
        message: d.str("message").to_owned(),
        memo: d.str("memo").to_owned(),
        lines,
        tags: Tags::from_names(d.at("tag_names")),
        ..Editor::default()
    });
}

// ----- lists ----------------------------------------------------------------------------

fn invoices_tab(app: &mut App, ui: &mut Ui) {
    let customers = names(&app.lists().customers, false);
    let statements = on(app, "statements");
    ui.wrap(|ui| {
        if ui.primary("New invoice").clicked() {
            new_editor(app, "invoice", "");
        }
        let l: Vec<&str> = STATUSES.iter().map(|s| s.1).collect();
        choice(ui, "Status filter", &mut app.sales.status_filter, &l);
        pick(
            ui,
            "Customer filter",
            &mut app.sales.customer_filter,
            &customers,
            Some("All customers"),
        );
        ui.checkbox(&mut app.sales.overdue_only, "Overdue only");
        if statements {
            if ui.button("Statements…").clicked() {
                super::statements::open_batch(app);
            }
            if ui.button("Late fees…").clicked() {
                super::statements::open_late_fees(app);
            }
        }
    });
    let mut args = json!({"as_of": app.today(), "overdue": app.sales.overdue_only});
    if let Some((s, _)) = STATUSES.get(app.sales.status_filter) {
        put(&mut args, "status", s);
    }
    put(&mut args, "customer", &app.sales.customer_filter);
    let rows = array(app.read("invoice.list", args));
    if rows.is_empty() {
        ui.card("", |ui| ui.label("No invoices yet."));
        return;
    }
    let cols = vec![
        Column {
            width: 110,
            ..col("Number")
        },
        Column {
            weight: 3,
            ..col("Customer")
        },
        Column {
            width: 110,
            ..col("Date")
        },
        Column {
            width: 110,
            ..col("Due")
        },
        Column {
            width: 120,
            ..col_num("Total")
        },
        Column {
            width: 120,
            ..col_num("Balance")
        },
        Column {
            width: 190,
            ..col("Status")
        },
    ];
    let mut open = None;
    ui.table("", cols, |ui| {
        for r in &rows {
            ui.table_row(&format!("invoice:{}", r.str("id")), false, false, |ui| {
                if ui.link(r.str("number")).clicked() {
                    open = Some(r.str("id").to_owned());
                }
                cell(ui, r.str("customer_name"));
                ui.label(fmt::date(r.str("date")));
                ui.label(fmt::date(r.str("due_date")));
                money(ui, r.str("total"));
                money(ui, r.str("balance"));
                ui.row(|ui| {
                    chip(ui, r.str("status"));
                    if r.at("overdue").as_bool() == Some(true) {
                        ui.badge("Overdue", Tone::Error);
                    }
                });
            });
        }
    });
    if let Some(id) = open {
        open_editor(app, "invoice", &id);
    }
}

fn estimates_tab(app: &mut App, ui: &mut Ui) {
    ui.row(|ui| {
        if ui.primary("New estimate").clicked() {
            new_editor(app, "estimate", "");
        }
    });
    let rows = array(app.read("estimate.list", json!({"as_of": app.today()})));
    if rows.is_empty() {
        ui.card("", |ui| ui.label("No estimates yet."));
        return;
    }
    let cols = vec![
        Column {
            width: 110,
            ..col("Number")
        },
        Column {
            weight: 3,
            ..col("Customer")
        },
        Column {
            width: 110,
            ..col("Date")
        },
        Column {
            width: 110,
            ..col("Expires")
        },
        Column {
            width: 120,
            ..col_num("Total")
        },
        Column {
            width: 120,
            ..col("Status")
        },
    ];
    let mut open = None;
    ui.table("", cols, |ui| {
        for r in &rows {
            ui.table_row(&format!("estimate:{}", r.str("id")), false, false, |ui| {
                if ui.link(r.str("number")).clicked() {
                    open = Some(r.str("id").to_owned());
                }
                cell(ui, r.str("customer_name"));
                ui.label(fmt::date(r.str("date")));
                ui.label(fmt::date(r.str("expires")));
                money(ui, r.str("total"));
                chip(ui, r.str("status"));
            });
        }
    });
    if let Some(id) = open {
        open_editor(app, "estimate", &id);
    }
}

fn payments_tab(app: &mut App, ui: &mut Ui) {
    ui.row(|ui| {
        if ui.primary("Record payment…").clicked() {
            open_payment(app, "");
        }
        if ui.button("Make deposit…").clicked() {
            app.sales.deposit = Some(DepositForm {
                date: fmt::date(&app.today()),
                ..DepositForm::default()
            });
        }
    });
    let rows = array(app.read("payment.list", json!({})));
    if rows.is_empty() {
        ui.card("", |ui| ui.label("No payments yet."));
        return;
    }
    let cols = vec![
        Column {
            width: 110,
            ..col("Date")
        },
        Column {
            weight: 3,
            ..col("Customer")
        },
        Column {
            width: 120,
            ..col("Method")
        },
        Column {
            width: 120,
            ..col_num("Amount")
        },
        Column {
            width: 120,
            ..col_num("Unapplied")
        },
        Column {
            width: 100,
            ..col("Deposited")
        },
    ];
    ui.table("", cols, |ui| {
        for (i, r) in rows.iter().enumerate() {
            ui.table_row(&format!("payment:{i}"), false, false, |ui| {
                ui.label(fmt::date(r.str("date")));
                cell(ui, r.str("customer_name"));
                ui.label(
                    METHODS
                        .iter()
                        .find(|m| m.0 == r.str("method"))
                        .map_or("", |m| m.1),
                );
                money(ui, r.str("amount"));
                money(ui, r.str("unapplied"));
                ui.label(if r.at("deposited").as_bool() == Some(true) {
                    "Yes"
                } else {
                    "Waiting"
                });
            });
        }
    });
}

fn customers_tab(app: &mut App, ui: &mut Ui) {
    ui.row(|ui| {
        if ui.primary("New customer…").clicked() {
            app.sales.customer_form = Some(CustomerForm::default());
        }
    });
    let balances = app
        .read("report.customer_balances", json!({"as_of": app.today()}))
        .unwrap_or(Value::Null);
    let customers = app.lists().customers.clone();
    if customers.is_empty() {
        ui.card("", |ui| ui.label("No customers yet."));
        return;
    }
    let cols = vec![
        Column {
            weight: 3,
            ..col("Name")
        },
        Column {
            width: 140,
            ..col_num("Open balance")
        },
        Column {
            width: 120,
            ..col_num("Overdue")
        },
        Column {
            width: 120,
            ..col_num("Credit")
        },
    ];
    let mut open = None;
    ui.table("", cols, |ui| {
        for c in &customers {
            let b = balances
                .list("customers")
                .iter()
                .find(|x| x.str("customer") == c.str("id"))
                .cloned()
                .unwrap_or(Value::Null);
            let name = if c.at("active").as_bool() == Some(false) {
                format!("{} (inactive)", c.str("name"))
            } else {
                c.str("name").to_owned()
            };
            ui.table_row(&format!("customer:{}", c.str("id")), false, false, |ui| {
                if ui.link(name.clone()).clicked() {
                    open = Some(c.str("id").to_owned());
                }
                money(ui, if b.is_null() { "0.00" } else { b.str("open") });
                money(
                    ui,
                    if b.is_null() {
                        "0.00"
                    } else {
                        b.str("overdue")
                    },
                );
                let credit = b.str("credits").trim_start_matches('-').to_owned();
                money(ui, if credit.is_empty() { "0.00" } else { &credit });
            });
        }
    });
    if open.is_some() {
        app.sales.customer = open;
    }
}

fn customer_view(app: &mut App, ui: &mut Ui, id: &str) {
    let Some(s) = app.read("customer.summary", json!({"party": id})) else {
        app.sales.customer = None;
        return;
    };
    let c = s.at("customer").clone();
    ui.row(|ui| {
        if ui.icon_button("back", "< Customers", false).clicked() {
            app.sales.customer = None;
        }
        ui.heading(c.str("name"));
    });
    ui.card("", |ui| {
        for (k, v) in [
            ("Company", c.str("company").to_owned()),
            ("Contact", c.str("contact").to_owned()),
            ("Email", c.str("email").to_owned()),
            ("Phone", c.str("phone").to_owned()),
            (
                "Billing address",
                c.list("billing_address")
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            ("Terms", c.str("terms_label").to_owned()),
            (
                "Tax",
                if c.at("tax_exempt").as_bool() == Some(true) {
                    format!("Exempt {}", c.str("exemption_note"))
                } else {
                    "Taxable".to_owned()
                },
            ),
        ] {
            ui.form(k, |ui| ui.label(v));
        }
    });
    let statements = on(app, "statements");
    ui.wrap(|ui| {
        if ui.button("Edit…").clicked() {
            app.sales.customer_form = Some(customer_form_from(app, &c));
        }
        if ui.button("New invoice").clicked() {
            new_editor(app, "invoice", c.str("name"));
        }
        if ui.button("Record payment…").clicked() {
            open_payment(app, c.str("name"));
        }
        if statements && ui.button("Statement…").clicked() {
            super::statements::open_statement(app, c.str("name"));
        }
        let has_credit = !fmt::is_zero(s.str("credit"));
        ui.enabled(has_credit);
        if ui.button("Refund…").clicked() {
            app.sales.refund = Some(RefundForm {
                customer: c.str("name").to_owned(),
                amount: s.str("credit").to_owned(),
                date: fmt::date(&app.today()),
                ..RefundForm::default()
            });
        }
        let active = c.at("active").as_bool() != Some(false);
        if ui
            .button(if active {
                "Make inactive"
            } else {
                "Make active"
            })
            .clicked()
            && let Err(e) = app.run("party.set_active", json!({"party": id, "active": !active}))
        {
            app.notify_error(&e);
        }
    });
    ui.label(format!(
        "Open balance {}   Overdue {}   Credit {}",
        fmt::group(s.str("balance")),
        fmt::group(s.str("overdue")),
        fmt::group(s.str("credit"))
    ));
    let mut open = None;
    let cols = vec![
        Column {
            width: 110,
            ..col("Number")
        },
        Column {
            width: 110,
            ..col("Date")
        },
        Column {
            width: 110,
            ..col("Due")
        },
        Column {
            width: 120,
            ..col_num("Balance")
        },
        Column {
            weight: 1,
            ..col("Status")
        },
    ];
    ui.table("Open invoices", cols, |ui| {
        for r in s.list("open_invoices") {
            ui.table_row(&format!("open:{}", r.str("id")), false, false, |ui| {
                if ui.link(r.str("number")).clicked() {
                    open = Some(r.str("id").to_owned());
                }
                ui.label(fmt::date(r.str("date")));
                ui.label(fmt::date(r.str("due_date")));
                money(ui, r.str("balance"));
                chip(ui, r.str("status"));
            });
        }
    });
    let cols = vec![
        Column {
            width: 110,
            ..col("Date")
        },
        Column {
            width: 120,
            ..col_num("Amount")
        },
        Column {
            weight: 1,
            ..col("")
        },
    ];
    ui.table("Payments", cols, |ui| {
        for (i, p) in s.list("payments").iter().enumerate() {
            ui.table_row(&format!("paid:{i}"), false, false, |ui| {
                ui.label(fmt::date(p.str("date")));
                money(ui, p.str("amount"));
                ui.label(format!("unapplied {}", fmt::group(p.str("unapplied"))));
            });
        }
    });
    if let Some(i) = open {
        open_editor(app, "invoice", &i);
    }
}

fn customer_form_from(app: &mut App, c: &Value) -> CustomerForm {
    let rates = app.lists().rates.clone();
    let tax = c
        .at("tax_rate")
        .as_str()
        .and_then(|id| rates.iter().find(|r| r.str("id") == id))
        .map(|r| r.str("name").to_owned())
        .unwrap_or_default();
    CustomerForm {
        id: Some(c.str("id").to_owned()),
        name: c.str("name").to_owned(),
        company: c.str("company").to_owned(),
        contact: c.str("contact").to_owned(),
        email: c.str("email").to_owned(),
        phone: c.str("phone").to_owned(),
        billing: c
            .list("billing_address")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        terms: if c.at("terms").is_null() {
            0
        } else {
            terms_index(c.at("terms"))
        },
        tax,
        exempt: c.at("tax_exempt").as_bool() == Some(true),
        note: c.str("exemption_note").to_owned(),
        notes: c.str("notes").to_owned(),
        error: None,
        for_editor: false,
    }
}

fn items_tab(app: &mut App, ui: &mut Ui) {
    ui.row(|ui| {
        if ui.primary("New item…").clicked() {
            app.sales.item_form = Some(ItemForm::default());
        }
    });
    let items = app.lists().items.clone();
    if items.is_empty() {
        ui.card("", |ui| ui.label("No items yet."));
        return;
    }
    let cols = vec![
        Column {
            weight: 2,
            ..col("Name")
        },
        Column {
            width: 120,
            ..col("Kind")
        },
        Column {
            width: 110,
            ..col_num("Price")
        },
        Column {
            weight: 2,
            ..col("Income account")
        },
        Column {
            width: 80,
            ..col("Taxable")
        },
    ];
    let mut edit = None;
    ui.table("", cols, |ui| {
        for it in &items {
            ui.table_row(&format!("item:{}", it.str("id")), false, false, |ui| {
                if ui.link(it.str("name")).clicked() {
                    edit = Some(it.clone());
                }
                ui.label(
                    ITEM_KINDS
                        .iter()
                        .find(|k| k.0 == it.str("kind"))
                        .map_or("", |k| k.1),
                );
                if it.at("percent").as_bool() == Some(true) {
                    ui.amount(format!("{}%", it.str("price")));
                } else {
                    money(ui, it.str("price"));
                }
                cell(ui, it.str("income_account_name"));
                ui.label(if it.at("taxable").as_bool() == Some(true) {
                    "Yes"
                } else {
                    "No"
                });
            });
        }
    });
    if let Some(it) = edit {
        app.sales.item_form = Some(ItemForm {
            id: Some(it.str("id").to_owned()),
            name: it.str("name").to_owned(),
            kind: ITEM_KINDS
                .iter()
                .position(|k| k.0 == it.str("kind"))
                .unwrap_or(0),
            description: it.str("description").to_owned(),
            sku: it.str("sku").to_owned(),
            price: it.str("price").to_owned(),
            percent: it.at("percent").as_bool() == Some(true),
            account: it.str("income_account_name").to_owned(),
            taxable: it.at("taxable").as_bool() == Some(true),
            error: None,
        });
    }
}

fn tax_tab(app: &mut App, ui: &mut Ui) {
    ui.row(|ui| {
        if ui.primary("New rate…").clicked() {
            app.sales.rate_form = Some(RateForm::default());
        }
        if ui.button("New group…").clicked() {
            app.sales.rate_form = Some(RateForm {
                group: true,
                ..RateForm::default()
            });
        }
    });
    let rates = app.lists().rates.clone();
    let cols = vec![
        Column {
            weight: 2,
            ..col("Name")
        },
        Column {
            width: 100,
            ..col_num("Rate")
        },
        Column {
            weight: 2,
            ..col("Agency")
        },
        Column {
            weight: 2,
            ..col("Members")
        },
    ];
    let mut edit = None;
    ui.table("", cols, |ui| {
        for r in &rates {
            ui.table_row(&format!("rate:{}", r.str("id")), false, false, |ui| {
                if ui.link(r.str("name")).clicked() {
                    edit = Some(r.clone());
                }
                ui.amount(format!("{}%", r.str("rate")));
                cell(ui, r.str("agency"));
                cell(
                    ui,
                    r.list("members")
                        .iter()
                        .map(|m| m.str("name").to_owned())
                        .collect::<Vec<_>>()
                        .join(" + "),
                );
            });
        }
    });
    if let Some(r) = edit {
        app.sales.rate_form = Some(RateForm {
            id: Some(r.str("id").to_owned()),
            group: r.str("kind") == "group",
            name: r.str("name").to_owned(),
            rate: r.str("rate").to_owned(),
            agency: r.str("agency").to_owned(),
            members: r
                .list("members")
                .iter()
                .map(|m| m.str("name").to_owned())
                .collect(),
            error: None,
        });
    }
    let liability = app
        .read(
            "report.sales_tax_liability",
            json!({"preset": "all", "today": app.today()}),
        )
        .unwrap_or(Value::Null);
    let cols = vec![
        Column {
            weight: 2,
            ..col("Rate")
        },
        Column {
            width: 120,
            ..col_num("Collected")
        },
        Column {
            width: 120,
            ..col_num("Paid")
        },
        Column {
            width: 120,
            ..col_num("Balance")
        },
    ];
    ui.table("Sales tax owed", cols, |ui| {
        for (i, r) in liability.list("rates").iter().enumerate() {
            ui.table_row(&format!("owed:{i}"), false, false, |ui| {
                cell(ui, r.str("name"));
                money(ui, r.str("collected"));
                money(ui, r.str("paid"));
                money(ui, r.str("balance"));
            });
        }
    });
    ui.row(|ui| {
        if ui.button("Pay sales tax…").clicked() {
            let amounts = liability
                .list("rates")
                .iter()
                .filter(|r| !fmt::is_zero(r.str("balance")) && !fmt::is_negative(r.str("balance")))
                .map(|r| {
                    (
                        r.str("rate").to_owned(),
                        r.str("name").to_owned(),
                        r.str("balance").to_owned(),
                    )
                })
                .collect();
            app.sales.pay_tax = Some(PayTaxForm {
                date: fmt::date(&app.today()),
                amounts,
                ..PayTaxForm::default()
            });
        }
    });
}

// ----- editor ---------------------------------------------------------------------------

fn doc_args(app: &mut App, e: &Editor, for_preview: bool) -> Value {
    let tags_on = tags::options(app);
    let items_on = on(app, "items");
    let items = app.lists().items.clone();
    let lines: Vec<Value> = e
        .lines
        .iter()
        .map(|l| {
            if l.subtotal {
                let mut v = json!({"subtotal": true});
                put(&mut v, "description", &l.description);
                return v;
            }
            let mut v = json!({});
            let is_item = items_on && items.iter().any(|i| i.str("name") == l.what);
            put(&mut v, if is_item { "item" } else { "account" }, &l.what);
            put(&mut v, "description", &l.description);
            put(&mut v, "quantity", &amount_arg(&l.quantity));
            put(&mut v, "rate", &amount_arg(&l.rate));
            put(&mut v, "amount", &amount_arg(&l.amount));
            if let Some(t) = l.taxable {
                set(&mut v, "taxable", json!(t));
            }
            l.tags.apply_given(&mut v, &tags_on);
            // Income lines have no Billable flag; they may bill project costs.
            if let Some(o) = v.as_object_mut() {
                o.remove("billable");
            }
            if !l.billable_items.is_empty() {
                set(&mut v, "billable_items", json!(l.billable_items));
            }
            v
        })
        .collect();
    let mut a = json!({"lines": lines});
    put(&mut a, "customer", &e.customer);
    put(&mut a, "date", &fmt::date_arg_lenient(&e.date));
    if e.kind == "invoice" {
        put(&mut a, "due_date", &fmt::date_arg_lenient(&e.due_date));
        if let Some(t) = terms_value(e.terms) {
            set(&mut a, "terms", t);
        }
    } else {
        put(&mut a, "expires", &fmt::date_arg_lenient(&e.due_date));
    }
    match e.tax.as_str() {
        "" => {}
        "-" => set(&mut a, "tax_rate", json!("")),
        t => set(&mut a, "tax_rate", json!(t)),
    }
    if !for_preview {
        set(&mut a, "message", json!(e.message));
        set(&mut a, "memo", json!(e.memo));
        put(&mut a, "number", &e.number);
        e.tags.apply(&mut a, &tags_on);
        if let Some(o) = a.as_object_mut() {
            o.remove("billable");
        }
    }
    a
}

fn refresh_preview(app: &mut App, e: &mut Editor) {
    if e.customer.trim().is_empty() {
        e.preview = None;
        e.preview_error = None;
        return;
    }
    let mut a = doc_args(app, e, true);
    set(&mut a, "kind", json!(e.kind));
    if let Some(id) = &e.id {
        set(&mut a, "invoice", json!(id));
    }
    let key = format!("{}|{a}", app.generation);
    if key == e.preview_key {
        return;
    }
    e.preview_key = key;
    match app.backend.invoke("invoice.preview", a) {
        Ok(v) => {
            e.preview = Some(v);
            e.preview_error = None;
        }
        Err(err) => {
            e.preview = None;
            e.preview_error = Some(err.display());
        }
    }
}

fn editor(app: &mut App, ui: &mut Ui) {
    let Some(mut e) = app.sales.editor.take() else {
        return;
    };
    refresh_preview(app, &mut e);
    let title = if e.kind == "estimate" {
        "Estimate"
    } else {
        "Invoice"
    };
    let mut close = false;
    ui.row(|ui| {
        if ui.icon_button("back", "< Back", false).clicked() {
            close = true;
        }
        let heading = match &e.id {
            Some(_) => format!("{title} {}", e.number),
            None => format!("New {}", title.to_lowercase()),
        };
        ui.heading(heading);
        if e.id.is_some() {
            chip(ui, &e.status);
        }
    });
    let customers = names(&app.lists().customers, true);
    let tax_on = on(app, "sales-tax");
    let rates = names(&app.lists().rates, true);
    let mut new_customer = false;
    let mut status_pick = None;
    ui.card("", |ui| {
        ui.form("Customer", |ui| {
            pick(ui, "Customer", &mut e.customer, &customers, None);
            if ui.button("New customer…").clicked() {
                new_customer = true;
            }
        });
        let hint = e
            .preview
            .as_ref()
            .map(|p| p.str("number").to_owned())
            .unwrap_or_default();
        ui.form("Number", |ui| {
            ui.width(140).hint(hint);
            ui.field("Number", &mut e.number);
        });
        ui.form("Date", |ui| {
            super::register::date_field(ui, "Date", &mut e.date)
        });
        if e.kind == "invoice" {
            ui.form("Terms", |ui| {
                let l: Vec<&str> = TERMS.iter().map(|t| t.0).collect();
                choice(ui, "Terms", &mut e.terms, &l);
            });
        } else {
            ui.form("Status", |ui| {
                if e.id.is_some() && e.status != "converted" {
                    let l: Vec<&str> = ESTIMATE_STATUSES.iter().map(|s| s.1).collect();
                    let mut i = ESTIMATE_STATUSES
                        .iter()
                        .position(|s| s.0 == e.status)
                        .unwrap_or(0);
                    if choice(ui, "Estimate status", &mut i, &l) {
                        status_pick = ESTIMATE_STATUSES.get(i).map(|s| s.0);
                    }
                } else {
                    ui.label(status_label(&e.status));
                }
            });
        }
        let label = if e.kind == "invoice" {
            "Due date"
        } else {
            "Valid until"
        };
        let hint = e
            .preview
            .as_ref()
            .map(|p| {
                fmt::date(if e.kind == "invoice" {
                    p.str("due_date")
                } else {
                    p.str("expires")
                })
            })
            .unwrap_or_default();
        ui.form(label, |ui| {
            // An empty due date shows the computed one as its placeholder.
            if e.due_date.is_empty() && !hint.is_empty() {
                ui.width(140).hint(hint);
                ui.field(label, &mut e.due_date);
            } else {
                super::register::date_field(ui, label, &mut e.due_date);
            }
        });
        if tax_on {
            ui.form("Tax", |ui| {
                let mut opts = vec!["Customer default".to_owned(), "No tax".to_owned()];
                opts.extend(rates.iter().cloned());
                let mut i = match e.tax.as_str() {
                    "" => 0,
                    "-" => 1,
                    t => rates.iter().position(|r| r == t).map_or(0, |p| p + 2),
                };
                ui.width(220);
                if ui.combo("Tax", &mut i, &opts).changed() {
                    e.tax = match i {
                        0 => String::new(),
                        1 => "-".into(),
                        n => rates.get(n - 2).cloned().unwrap_or_default(),
                    };
                }
            });
        }
    });
    if let Some(s) = status_pick {
        match app.run(
            "estimate.set_status",
            json!({"estimate": e.id, "status": s}),
        ) {
            Ok(v) => e.status = v.str("status").to_owned(),
            Err(err) => e.error = Some(err.display()),
        }
    }
    let tags_on = tags::options(app);
    if tags_on.any() {
        ui.card("", |ui| {
            tags::rows(ui, &tags_on, &mut e.tags, "");
        });
    }
    if new_customer {
        app.sales.customer_form = Some(CustomerForm {
            for_editor: true,
            ..CustomerForm::default()
        });
    }
    super::projects::invoice_bar(app, ui, &e);
    lines_grid(app, ui, &mut e);
    totals(ui, &e);
    ui.card("", |ui| {
        ui.form("Message", |ui| {
            ui.width(420);
            ui.multiline("Message", &mut e.message);
        });
        ui.form("Memo", |ui| {
            ui.width(420);
            ui.field("Memo (not printed)", &mut e.memo);
        });
    });
    if let Some(err) = e.error.as_ref().or(e.preview_error.as_ref()) {
        ui.error_text(err.clone());
    }
    let action = buttons(ui, &e);
    if close {
        return;
    }
    if let Some(a) = action {
        act(app, &mut e, a);
    }
    if app.sales.editor.is_none() {
        app.sales.editor = Some(e);
    }
}

fn lines_grid(app: &mut App, ui: &mut Ui, e: &mut Editor) {
    let items_on = on(app, "items");
    let options: Vec<String> = if items_on {
        names(&app.lists().items, true)
    } else {
        let l = app.lists().clone();
        accounts_where(&l, |a| a.str("type") == "income")
    };
    let what = if items_on { "item" } else { "account" };
    let computed: Vec<Value> = e
        .preview
        .as_ref()
        .map(|p| p.at("totals").list("lines").to_vec())
        .unwrap_or_default();
    let mut remove = None;
    let mut swap = None;
    let cols = vec![
        Column {
            weight: 3,
            ..col(if items_on { "Item" } else { "Account" })
        },
        Column {
            weight: 3,
            ..col("Description")
        },
        Column {
            width: 64,
            ..col_num("Qty")
        },
        Column {
            width: 90,
            ..col_num("Rate")
        },
        Column {
            width: 100,
            ..col_num("Amount")
        },
        Column {
            width: 64,
            align: Align::Center,
            ..col("Taxable")
        },
        Column {
            width: 160,
            ..col("")
        },
    ];
    let n = e.lines.len();
    ui.table("Lines", cols, |ui| {
        for (i, l) in e.lines.iter_mut().enumerate() {
            let k = i + 1;
            let c = computed.get(i).cloned().unwrap_or(Value::Null);
            ui.table_row(&format!("line{k}"), false, false, |ui| {
                if l.subtotal {
                    ui.bold();
                    ui.label("Subtotal");
                    ui.field(&format!("Line {k} description"), &mut l.description);
                    ui.label("");
                    ui.label("");
                    money(ui, c.str("amount"));
                    ui.label("");
                } else {
                    pick(ui, &format!("Line {k} {what}"), &mut l.what, &options, None);
                    ui.hint(c.str("description"));
                    ui.field(&format!("Line {k} description"), &mut l.description);
                    ui.hint("1").align(Align::Right).mono();
                    ui.field(&format!("Line {k} quantity"), &mut l.quantity);
                    ui.hint(c.str("rate")).align(Align::Right).mono();
                    ui.field(&format!("Line {k} rate"), &mut l.rate);
                    ui.hint(fmt::group(c.str("amount")))
                        .align(Align::Right)
                        .mono();
                    ui.field(&format!("Line {k} amount"), &mut l.amount);
                    let mut taxable = l.taxable.unwrap_or(c.at("taxable").as_bool() == Some(true));
                    ui.tip("Taxable").named(format!("Line {k} taxable"));
                    if ui.checkbox(&mut taxable, "").clicked() {
                        l.taxable = Some(taxable);
                    }
                }
                ui.row(|ui| {
                    ui.enabled(i > 0).tip("Move up");
                    if ui
                        .icon_button("chevron-small", &format!("Move line {k} up"), false)
                        .clicked()
                    {
                        swap = Some((i.saturating_sub(1), i));
                    }
                    ui.enabled(i + 1 < n).tip("Move down");
                    if ui
                        .icon_button("chevron-down", &format!("Move line {k} down"), false)
                        .clicked()
                    {
                        swap = Some((i, i + 1));
                    }
                    ui.tip(format!("Remove line {k}"));
                    if ui.button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
    });
    if let Some((a, b)) = swap {
        e.lines.swap(a, b);
    }
    if let Some(i) = remove
        && i < e.lines.len()
        && e.lines.len() > 1
    {
        e.lines.remove(i);
    }
    ui.row(|ui| {
        if ui.button("Add line").clicked() {
            e.lines.push(LineDraft::default());
        }
        if ui.button("Add subtotal").clicked() {
            e.lines.push(LineDraft {
                subtotal: true,
                ..LineDraft::default()
            });
        }
    });
}

fn totals(ui: &mut Ui, e: &Editor) {
    let Some(p) = &e.preview else {
        return;
    };
    let t = p.at("totals");
    let cols = vec![
        Column {
            weight: 3,
            ..col("")
        },
        Column {
            width: 140,
            ..col_num("")
        },
    ];
    ui.table("", cols, |ui| {
        ui.table_row("Subtotal#totals", false, false, |ui| {
            ui.label("Subtotal");
            money(ui, t.str("subtotal"));
        });
        for (i, x) in t.list("tax").iter().enumerate() {
            ui.table_row(&format!("tax{i}#totals"), false, false, |ui| {
                ui.label(format!("{} ({}%)", x.str("name"), x.str("percent")));
                money(ui, x.str("amount"));
            });
        }
        ui.table_row("Total#totals", false, false, |ui| {
            ui.bold();
            ui.label("Total");
            ui.bold()
                .named(format!("Total {}", fmt::group(t.str("total"))));
            ui.amount(fmt::group(t.str("total")));
        });
    });
}

/// The editor's buttons; returns the one clicked.
fn buttons(ui: &mut Ui, e: &Editor) -> Option<&'static str> {
    let mut out = None;
    let saved = e.id.is_some();
    let ok = e.preview.is_some();
    ui.wrap(|ui| {
        if e.kind == "invoice" {
            let draft = e.status == "draft";
            let void = e.status == "void";
            if draft {
                ui.enabled(ok);
                if ui.button("Save draft").clicked() {
                    out = Some("save_draft");
                }
                ui.enabled(ok);
                if ui.primary("Save and finalise").clicked() {
                    out = Some("finalise");
                }
            } else if !void {
                ui.enabled(ok);
                if ui.primary("Save").clicked() {
                    out = Some("save");
                }
            }
            if saved {
                if ui.button("PDF…").clicked() {
                    out = Some("pdf");
                }
                if !draft && !void && e.status != "paid" && ui.button("Record payment…").clicked()
                {
                    out = Some("payment");
                }
                if ui.button("Duplicate").clicked() {
                    out = Some("duplicate");
                }
                if !draft && !void && ui.button("Void…").clicked() {
                    out = Some("confirm_void");
                }
                if ui.button("Delete…").clicked() {
                    out = Some("confirm_delete");
                }
            }
        } else {
            let converted = e.status == "converted";
            if !converted {
                ui.enabled(ok);
                if ui.primary("Save").clicked() {
                    out = Some("save");
                }
            }
            if saved {
                if !converted && ui.button("Convert to invoice").clicked() {
                    out = Some("convert");
                }
                if ui.button("PDF…").clicked() {
                    out = Some("pdf");
                }
                if ui.button("Duplicate").clicked() {
                    out = Some("duplicate");
                }
                if ui.button("Delete…").clicked() {
                    out = Some("confirm_delete");
                }
            }
        }
    });
    out
}

/// A PDF from a `*.render_pdf` result handed to the shell to save.
pub(crate) fn save_pdf(app: &mut App, p: &Value) -> Result<(), String> {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD
        .decode(p.str("content_base64"))
        .map_err(|err| format!("The PDF could not be read: {err}"))?;
    for w in p.list("warnings") {
        app.notify(w.as_str().unwrap_or_default().to_owned());
    }
    app.emit(AppEvent::SaveBytes {
        file_name: p.str("file_name").to_owned(),
        data,
    });
    Ok(())
}

fn act(app: &mut App, e: &mut Editor, action: &'static str) {
    let kind = e.kind;
    let id = e.id.clone().unwrap_or_default();
    let result: Result<Option<Value>, crate::UiError> = match action {
        "save_draft" | "save" | "finalise" => {
            let mut a = doc_args(app, e, false);
            match &e.id {
                Some(id) => {
                    set(&mut a, kind, json!(id));
                    let r = app.run(&format!("{kind}.update"), a);
                    match (r, action) {
                        (Ok(_), "finalise") => app
                            .run("invoice.finalize", json!({"invoice": id}))
                            .map(Some),
                        (r, _) => r.map(Some),
                    }
                }
                None => {
                    if kind == "invoice" {
                        set(
                            &mut a,
                            "status",
                            json!(if action == "finalise" {
                                "open"
                            } else {
                                "draft"
                            }),
                        );
                    }
                    app.run(&format!("{kind}.create"), a).map(Some)
                }
            }
        }
        "pdf" => {
            if let Some(p) = app.read(&format!("{kind}.render_pdf"), json!({kind: id}))
                && let Err(err) = save_pdf(app, &p)
            {
                e.error = Some(err);
            }
            Ok(None)
        }
        "payment" => {
            let customer = e.customer.clone();
            open_payment(app, &customer);
            Ok(None)
        }
        "duplicate" => app
            .run(
                &format!("{kind}.duplicate"),
                json!({kind: id, "date": app.today()}),
            )
            .map(Some),
        "convert" => match app.run(
            "estimate.convert",
            json!({"estimate": id, "date": app.today()}),
        ) {
            Ok(v) => {
                let inv = v.at("invoice").str("id").to_owned();
                app.notify_with(
                    format!("Converted to invoice {}", v.at("invoice").str("number")),
                    MessageAction::Undo,
                );
                open_editor(app, "invoice", &inv);
                return;
            }
            Err(err) => Err(err),
        },
        "confirm_void" => {
            e.confirm = Some("void");
            Ok(None)
        }
        "confirm_delete" => {
            e.confirm = Some("delete");
            Ok(None)
        }
        _ => Ok(None),
    };
    match result {
        Ok(Some(v)) => {
            e.error = None;
            let new_id = v.str("id").to_owned();
            app.notify_with(
                format!(
                    "{} {} saved",
                    if kind == "estimate" {
                        "Estimate"
                    } else {
                        "Invoice"
                    },
                    v.str("number")
                ),
                MessageAction::Undo,
            );
            open_editor(app, kind, &new_id);
        }
        Ok(None) => {}
        Err(err) => e.error = Some(err.display()),
    }
}

fn confirm_dialog(app: &mut App, ui: &mut Ui) {
    let Some(e) = app.sales.editor.as_ref() else {
        return;
    };
    let Some(what) = e.confirm else {
        return;
    };
    let (kind, id, number) = (e.kind, e.id.clone().unwrap_or_default(), e.number.clone());
    let noun = if kind == "estimate" {
        "estimate"
    } else {
        "invoice"
    };
    let verb = if what == "void" { "Void" } else { "Delete" };
    let mut go = false;
    let mut cancel = ui.dialog_closed("sales-confirm");
    ui.dialog(
        "sales-confirm",
        &format!("{verb} {noun} {number}?"),
        440,
        |ui| {
            ui.label(if what == "void" {
                "The invoice stays in the list as void and its transaction is voided."
            } else {
                "The document and its transaction are removed. You can undo this."
            });
            ui.actions(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                if ui.primary(format!("{verb} {noun}")).clicked() {
                    go = true;
                }
            });
        },
    );
    if cancel && let Some(e) = app.sales.editor.as_mut() {
        e.confirm = None;
    }
    if go {
        match app.run(&format!("{kind}.{what}"), json!({kind: id})) {
            Ok(_) => {
                app.notify_with(
                    format!(
                        "{number} {}",
                        if what == "void" { "voided" } else { "deleted" }
                    ),
                    MessageAction::Undo,
                );
                if what == "void" {
                    open_editor(app, kind, &id);
                } else {
                    app.sales.editor = None;
                }
            }
            Err(err) => {
                if let Some(e) = app.sales.editor.as_mut() {
                    e.confirm = None;
                    e.error = Some(err.display());
                }
            }
        }
    }
}

// ----- dialogs --------------------------------------------------------------------------

pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    confirm_dialog(app, ui);
    payment_dialog(app, ui);
    deposit_dialog(app, ui);
    customer_dialog(app, ui);
    item_dialog(app, ui);
    rate_dialog(app, ui);
    pay_tax_dialog(app, ui);
    refund_dialog(app, ui);
    super::statements::dialogs(app, ui);
    super::projects::add_costs_dialog(app, ui);
}

/// A decimal as money text with two places (for totals the screen adds up).
pub(crate) fn fmt_money(d: mk_core::Decimal) -> String {
    let mut r = d.round_dp(2);
    r.rescale(2);
    r.to_string()
}

pub(crate) fn open_payment(app: &mut App, customer: &str) {
    app.sales.payment = Some(PaymentForm {
        customer: customer.to_owned(),
        date: fmt::date(&app.today()),
        ..PaymentForm::default()
    });
}

fn accounts_of(app: &mut App, pred: impl Fn(&Value) -> bool) -> Vec<String> {
    let l = app.lists().clone();
    accounts_where(&l, pred)
}

fn payment_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.sales.payment.take() else {
        return;
    };
    let key = format!("{}|{}|{}|{}", app.generation, f.customer, f.amount, f.date);
    if key != f.suggest_key && !f.customer.is_empty() {
        f.suggest_key = key;
        let mut a = json!({"customer": f.customer});
        put(&mut a, "amount", &amount_arg(&f.amount));
        if let Ok(Some(d)) = fmt::read_date(&f.date) {
            set(&mut a, "date", json!(d));
        }
        if let Ok(v) = app.backend.invoke("payment.open_invoices", a) {
            if !f.edited {
                f.applies = v
                    .list("invoices")
                    .iter()
                    .map(|i| {
                        (
                            i.str("id").to_owned(),
                            i.str("number").to_owned(),
                            i.str("balance").to_owned(),
                            if fmt::is_zero(i.str("apply")) {
                                String::new()
                            } else {
                                i.str("apply").to_owned()
                            },
                        )
                    })
                    .collect();
            }
            f.info = Some(v);
        }
    }
    let customers = names(&app.lists().customers, true);
    let targets = accounts_of(app, |a| {
        matches!(a.str("subtype"), "bank" | "cash" | "clearing")
    });
    let mut save = false;
    let mut cancel = ui.dialog_closed("record-payment");
    ui.dialog("record-payment", "Record payment", 580, |ui| {
        ui.group(|ui| {
            ui.form("Customer", |ui| {
                if pick(ui, "Payment customer", &mut f.customer, &customers, None) {
                    f.edited = false;
                }
            });
            date_row(ui, "Payment date", &mut f.date);
            if text_row(ui, "Payment amount", &mut f.amount, "0.00").changed() {
                f.edited = false;
            }
            ui.form("Method", |ui| {
                let l: Vec<&str> = METHODS.iter().map(|m| m.1).collect();
                choice(ui, "Method", &mut f.method, &l);
            });
            text_row(ui, "Reference", &mut f.reference, "check number");
            ui.form("Deposit to", |ui| {
                pick(
                    ui,
                    "Deposit to",
                    &mut f.deposit_to,
                    &targets,
                    Some("Default (Payments to Deposit)"),
                );
            });
        });
        if !f.applies.is_empty() {
            let cols = vec![
                Column {
                    weight: 2,
                    ..col("Invoice")
                },
                Column {
                    weight: 2,
                    ..col("Balance")
                },
                Column {
                    width: 120,
                    ..col_num("Apply")
                },
            ];
            ui.table("Apply to open invoices", cols, |ui| {
                ui.visible_rows(5);
                for (_, number, balance, apply) in &mut f.applies {
                    ui.table_row(&format!("apply:{number}"), false, false, |ui| {
                        ui.label(number.clone());
                        ui.label(format!("balance {}", fmt::group(balance)));
                        ui.hint("0.00").align(Align::Right).mono();
                        if ui.field(&format!("Apply to {number}"), apply).changed() {
                            f.edited = true;
                        }
                    });
                }
            });
        }
        if let Some(info) = &f.info {
            if !f.edited {
                ui.label(format!(
                    "Credit left: {}",
                    fmt::group(info.str("credit_left"))
                ));
            }
            if !fmt::is_zero(info.str("existing_credit")) {
                ui.secondary(format!(
                    "The customer already has {} of credit.",
                    fmt::group(info.str("existing_credit"))
                ));
            }
        }
        if let Some(err) = &f.error {
            ui.error_text(err.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save payment").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if save {
        let mut a = json!({"customer": f.customer, "date": fmt::date_arg_lenient(&f.date), "amount": amount_arg(&f.amount)});
        if let Some((m, _)) = METHODS.get(f.method) {
            set(&mut a, "method", json!(m));
        }
        put(&mut a, "reference", &f.reference);
        put(&mut a, "deposit_to", &f.deposit_to);
        let apply: Vec<Value> = f
            .applies
            .iter()
            .filter(|x| !x.3.trim().is_empty())
            .map(|x| json!({"invoice": x.0, "amount": amount_arg(&x.3)}))
            .collect();
        set(&mut a, "apply", Value::Array(apply));
        match app.run("payment.receive", a) {
            Ok(v) => {
                app.notify_with(
                    format!("Payment of {} recorded", fmt::group(v.str("amount"))),
                    MessageAction::Undo,
                );
                if let Some(id) = app.sales.editor.as_ref().and_then(|e| e.id.clone()) {
                    open_editor(app, "invoice", &id);
                }
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.sales.payment = Some(f);
}

fn deposit_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.sales.deposit.take() else {
        return;
    };
    let waiting = array(
        app.backend
            .invoke("payment.list", json!({"undeposited": true}))
            .ok(),
    );
    let key = format!("{}|{:?}", app.generation, f.chosen);
    if key != f.total_key {
        f.total_key = key;
        f.total = app
            .backend
            .invoke("deposit.preview", json!({"payments": f.chosen}))
            .map(|v| v.str("total").to_owned())
            .unwrap_or_default();
    }
    let banks = accounts_of(app, |a| matches!(a.str("subtype"), "bank" | "cash"));
    let mut save = false;
    let mut cancel = ui.dialog_closed("make-deposit");
    ui.dialog("make-deposit", "Make deposit", 580, |ui| {
        ui.group(|ui| {
            ui.form("Bank account", |ui| {
                pick(ui, "Bank account", &mut f.bank, &banks, None);
            });
            date_row(ui, "Deposit date", &mut f.date);
        });
        ui.card("Payments waiting to be deposited", |ui| {
            if waiting.is_empty() {
                ui.label("No payments are waiting.");
            }
            for p in &waiting {
                let id = p.str("id").to_owned();
                let mut on = f.chosen.contains(&id);
                let label = format!(
                    "{} {} {}",
                    fmt::date(p.str("date")),
                    p.str("customer_name"),
                    fmt::group(p.str("amount"))
                );
                ui.row(|ui| {
                    if ui.checkbox(&mut on, &label).clicked() {
                        if on {
                            f.chosen.push(id.clone());
                        } else {
                            f.chosen.retain(|x| *x != id);
                        }
                    }
                });
            }
        });
        ui.label(format!("Deposit total {}", fmt::group(&f.total)));
        if let Some(err) = &f.error {
            ui.error_text(err.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            ui.enabled(!f.chosen.is_empty());
            if ui.primary("Save deposit").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if save {
        let a = json!({"bank_account": f.bank, "date": fmt::date_arg_lenient(&f.date), "payments": f.chosen});
        match app.run("deposit.create", a) {
            Ok(v) => {
                app.notify_with(
                    format!("Deposit of {} saved", fmt::group(v.str("total"))),
                    MessageAction::Undo,
                );
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.sales.deposit = Some(f);
}

fn customer_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.sales.customer_form.take() else {
        return;
    };
    let rates = names(&app.lists().rates, true);
    let tax_on = on(app, "sales-tax");
    let mut save = false;
    let mut cancel = ui.dialog_closed("customer-form");
    let title = if f.id.is_some() {
        "Edit customer"
    } else {
        "New customer"
    };
    ui.dialog("customer-form", title, 580, |ui| {
        ui.group(|ui| {
            text_row(ui, "Customer name", &mut f.name, "");
            text_row(ui, "Company", &mut f.company, "optional");
            text_row(ui, "Contact", &mut f.contact, "optional");
            text_row(ui, "Email", &mut f.email, "optional");
            text_row(ui, "Phone", &mut f.phone, "optional");
            ui.form("Billing address", |ui| {
                ui.width(240);
                ui.multiline("Billing address", &mut f.billing);
            });
            ui.form("Terms", |ui| {
                let l: Vec<&str> = TERMS.iter().map(|t| t.0).collect();
                choice(ui, "Customer terms", &mut f.terms, &l);
            });
            if tax_on {
                ui.form("Tax rate", |ui| {
                    pick(ui, "Customer tax rate", &mut f.tax, &rates, Some("Default"));
                });
            }
            ui.form("", |ui| {
                ui.checkbox(&mut f.exempt, "Tax exempt");
            });
            if f.exempt {
                text_row(ui, "Exemption note", &mut f.note, "certificate number");
            }
            ui.form("Notes", |ui| {
                ui.width(240);
                ui.multiline("Notes", &mut f.notes);
            });
        });
        if let Some(err) = &f.error {
            ui.error_text(err.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save customer").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if save {
        let billing: Vec<&str> = f.billing.lines().filter(|l| !l.trim().is_empty()).collect();
        let mut a = json!({
            "name": f.name, "company": f.company, "contact": f.contact, "email": f.email,
            "phone": f.phone, "billing_address": billing, "tax_exempt": f.exempt,
            "exemption_note": f.note, "notes": f.notes,
            "terms": terms_value(f.terms).unwrap_or(Value::Null),
        });
        if tax_on {
            set(&mut a, "tax_rate", json!(f.tax));
        }
        let cmd = match &f.id {
            Some(id) => {
                set(&mut a, "party", json!(id));
                "party.update"
            }
            None => "party.create",
        };
        match app.run(cmd, a) {
            Ok(v) => {
                app.notify_with(format!("{} saved", v.str("name")), MessageAction::Undo);
                if f.for_editor
                    && let Some(e) = app.sales.editor.as_mut()
                {
                    e.customer = v.str("name").to_owned();
                }
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.sales.customer_form = Some(f);
}

fn item_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.sales.item_form.take() else {
        return;
    };
    let discount = ITEM_KINDS.get(f.kind).map(|k| k.0) == Some("discount");
    let accounts = accounts_of(app, |a| {
        a.str("type") == "income" || (discount && a.str("type") == "expense")
    });
    let mut save = false;
    let mut cancel = ui.dialog_closed("item-form");
    let title = if f.id.is_some() {
        "Edit item"
    } else {
        "New item"
    };
    ui.dialog("item-form", title, 540, |ui| {
        ui.group(|ui| {
            text_row(ui, "Item name", &mut f.name, "");
            ui.form("Kind", |ui| {
                let l: Vec<&str> = ITEM_KINDS.iter().map(|k| k.1).collect();
                choice(ui, "Item kind", &mut f.kind, &l);
            });
            text_row(ui, "Description", &mut f.description, "printed on lines");
            text_row(ui, "SKU", &mut f.sku, "optional");
            let price_label = if discount && f.percent {
                "Percent"
            } else {
                "Price"
            };
            text_row(ui, price_label, &mut f.price, "0.00");
            if discount {
                ui.form("", |ui| {
                    ui.checkbox(&mut f.percent, "A percentage of the lines above");
                });
            }
            ui.form("Income account", |ui| {
                pick(ui, "Income account", &mut f.account, &accounts, None);
            });
            ui.form("", |ui| {
                ui.checkbox(&mut f.taxable, "Taxable");
            });
        });
        if let Some(err) = &f.error {
            ui.error_text(err.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save item").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if save {
        let kind = ITEM_KINDS.get(f.kind).map_or("service", |k| k.0);
        let mut a = json!({
            "name": f.name, "kind": kind, "description": f.description, "sku": f.sku,
            "percent": f.percent, "taxable": f.taxable,
        });
        put(&mut a, "price", &amount_arg(&f.price));
        put(&mut a, "income_account", &f.account);
        let cmd = match &f.id {
            Some(id) => {
                set(&mut a, "item", json!(id));
                "item.update"
            }
            None => "item.create",
        };
        match app.run(cmd, a) {
            Ok(v) => {
                app.notify_with(format!("{} saved", v.str("name")), MessageAction::Undo);
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.sales.item_form = Some(f);
}

fn rate_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.sales.rate_form.take() else {
        return;
    };
    let singles: Vec<String> = app
        .lists()
        .rates
        .iter()
        .filter(|r| r.str("kind") == "rate")
        .map(|r| r.str("name").to_owned())
        .collect();
    let mut save = false;
    let mut cancel = ui.dialog_closed("rate-form");
    let title = match (f.group, f.id.is_some()) {
        (true, false) => "New tax group",
        (true, true) => "Edit tax group",
        (false, false) => "New tax rate",
        (false, true) => "Edit tax rate",
    };
    ui.dialog("rate-form", title, 500, |ui| {
        ui.group(|ui| {
            text_row(ui, "Rate name", &mut f.name, "");
            if f.group {
                ui.form("Member rates", |ui| {
                    ui.label("");
                });
                for s in &singles {
                    let mut on = f.members.contains(s);
                    ui.form("", |ui| {
                        if ui.checkbox(&mut on, s).clicked() {
                            if on {
                                f.members.push(s.clone());
                            } else {
                                f.members.retain(|m| m != s);
                            }
                        }
                    });
                }
            } else {
                text_row(ui, "Rate %", &mut f.rate, "6.25");
                text_row(ui, "Agency", &mut f.agency, "who it is paid to");
            }
        });
        if let Some(err) = &f.error {
            ui.error_text(err.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save rate").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if save {
        let mut a = json!({"name": f.name});
        if f.group {
            set(&mut a, "members", json!(f.members));
        } else {
            put(&mut a, "rate", &f.rate);
            set(&mut a, "agency", json!(f.agency));
        }
        let cmd = match &f.id {
            Some(id) => {
                set(&mut a, "tax_rate", json!(id));
                "tax_rate.update"
            }
            None => {
                set(
                    &mut a,
                    "kind",
                    json!(if f.group { "group" } else { "rate" }),
                );
                "tax_rate.create"
            }
        };
        match app.run(cmd, a) {
            Ok(v) => {
                app.notify_with(format!("{} saved", v.str("name")), MessageAction::Undo);
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.sales.rate_form = Some(f);
}

fn pay_tax_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.sales.pay_tax.take() else {
        return;
    };
    let from = accounts_of(app, |a| {
        matches!(a.str("subtype"), "bank" | "cash" | "credit-card")
    });
    let mut save = false;
    let mut cancel = ui.dialog_closed("pay-sales-tax");
    ui.dialog("pay-sales-tax", "Pay sales tax", 500, |ui| {
        ui.group(|ui| {
            ui.form("Pay from", |ui| {
                pick(ui, "Pay from", &mut f.from, &from, None);
            });
            date_row(ui, "Payment date", &mut f.date);
            for (_, name, amount) in &mut f.amounts {
                let label = format!("Amount for {name}");
                ui.form(&label, |ui| {
                    ui.width(120).mono().align(Align::Right);
                    ui.field(&label, amount);
                });
            }
        });
        if f.amounts.is_empty() {
            ui.label("No sales tax is owed.");
        }
        if let Some(err) = &f.error {
            ui.error_text(err.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            ui.enabled(!f.amounts.is_empty());
            if ui.primary("Pay").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if save {
        let amounts: Vec<Value> = f
            .amounts
            .iter()
            .filter(|x| !x.2.trim().is_empty())
            .map(|x| json!({"rate": x.0, "amount": amount_arg(&x.2)}))
            .collect();
        let a = json!({"from_account": f.from, "date": fmt::date_arg_lenient(&f.date), "amounts": amounts});
        match app.run("sales_tax.pay", a) {
            Ok(v) => {
                app.notify_with(
                    format!("Sales tax of {} paid", fmt::group(v.str("total"))),
                    MessageAction::Undo,
                );
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.sales.pay_tax = Some(f);
}

fn refund_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.sales.refund.take() else {
        return;
    };
    let from = accounts_of(app, |a| {
        matches!(a.str("subtype"), "bank" | "cash" | "credit-card")
    });
    let mut save = false;
    let mut cancel = ui.dialog_closed("refund");
    ui.dialog("refund", &format!("Refund {}", f.customer), 460, |ui| {
        ui.group(|ui| {
            ui.form("Refund from", |ui| {
                pick(ui, "Refund from", &mut f.from, &from, None);
            });
            text_row(ui, "Refund amount", &mut f.amount, "0.00");
            date_row(ui, "Refund date", &mut f.date);
        });
        if let Some(err) = &f.error {
            ui.error_text(err.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save refund").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if save {
        let a = json!({"customer": f.customer, "from_account": f.from,
            "amount": amount_arg(&f.amount), "date": fmt::date_arg_lenient(&f.date)});
        match app.run("customer.refund", a) {
            Ok(v) => {
                app.notify_with(
                    format!("Refund of {} saved", fmt::group(v.str("amount"))),
                    MessageAction::Undo,
                );
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.sales.refund = Some(f);
}
