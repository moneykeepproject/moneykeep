//! Purchases (app-desktop.md revision 15): bills, bill payments and the Pay bills
//! run, purchase orders with Receive…, vendors with the vendor view, and 1099 with the
//! threshold editor. Numbers and rules come from the commands (`bill.preview`,
//! `bill.open`, `report.1099`, …); this module collects input and shows results.

use mk_core::Decimal;
use serde_json::{Value, json};

use super::form::{amount_arg, choice, date_row, pick, put, set, text_row};
use super::lists::{accounts_where, names};
use super::sales::{TERMS, cell, money, terms_index, terms_value};
use super::tags::{self, Tags};
use super::ui::{Align, Column, Tone, Ui, col, col_num};
use super::util::{Json, array};
use super::{App, AppEvent, MessageAction};
use crate::fmt;

const TABS: [(&str, Option<&str>); 5] = [
    ("Bills", None),
    ("Payments", None),
    ("Purchase orders", Some("purchase-orders")),
    ("Vendors", None),
    ("1099", Some("1099")),
];

const METHODS: [(&str, &str); 5] = [
    ("check", "Check"),
    ("bank_transfer", "Bank transfer"),
    ("card", "Card"),
    ("cash", "Cash"),
    ("other", "Other"),
];

const BILL_FILTERS: [&str; 4] = ["All", "Open", "Overdue", "Due in 7 days"];

const MISC_BOXES: [(&str, &str); 5] = [
    ("rents", "Box 1 Rents"),
    ("royalties", "Box 2 Royalties"),
    ("other_income", "Box 3 Other income"),
    ("medical", "Box 6 Medical and health care"),
    (
        "attorney_gross_proceeds",
        "Box 10 Gross proceeds to an attorney",
    ),
];

#[derive(Debug, Clone, Default)]
pub(crate) struct LineDraft {
    pub item_line: bool,
    pub what: String,
    pub description: String,
    pub quantity: String,
    pub cost: String,
    pub amount: String,
    pub tags: Tags,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Editor {
    pub kind: &'static str,
    pub id: Option<String>,
    pub status: String,
    pub vendor: String,
    pub number: String,
    pub date: String,
    pub terms: usize,
    pub due_date: String,
    pub expected: String,
    pub ship_to: String,
    pub memo: String,
    pub message: String,
    pub lines: Vec<LineDraft>,
    pub preview_key: String,
    pub preview: Option<Value>,
    pub preview_error: Option<String>,
    pub error: Option<String>,
    pub confirm: Option<&'static str>,
    pub tags: Tags,
    /// The bill schedule occurrence this new bill enters: (schedule, nominal date).
    pub from_schedule: Option<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PayForm {
    pub vendor: String,
    pub bill: Option<(String, String)>,
    pub from: String,
    pub date: String,
    pub method: usize,
    pub check: String,
    pub amount: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RunForm {
    pub from: String,
    pub date: String,
    pub method: usize,
    pub check: String,
    pub due_by: String,
    pub overdue_only: bool,
    pub chosen: Vec<(String, bool, String)>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ReceiveForm {
    pub po: String,
    pub date: String,
    pub bill_number: String,
    pub lines: Vec<(usize, String, String, bool)>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct VendorForm {
    pub id: Option<String>,
    pub name: String,
    pub company: String,
    pub email: String,
    pub phone: String,
    pub address: String,
    pub terms: usize,
    pub expense: String,
    pub account_number: String,
    pub tracks: bool,
    pub misc: bool,
    pub misc_box: usize,
    pub tin: bool,
    pub for_editor: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct VendorRefund {
    pub vendor: String,
    pub to: String,
    pub amount: String,
    pub date: String,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct PurchasesState {
    pub tab: usize,
    pub filter: usize,
    pub vendor_filter: String,
    pub editor: Option<Editor>,
    pub pay: Option<PayForm>,
    pub run: Option<RunForm>,
    pub receive: Option<ReceiveForm>,
    pub vendor: Option<String>,
    pub vendor_form: Option<VendorForm>,
    pub refund: Option<VendorRefund>,
    pub void_payment: Option<(String, String)>,
    pub year: String,
    pub include_below: bool,
    pub thresholds: Vec<(String, String)>,
    pub thresholds_year: String,
    pub thresholds_error: Option<String>,
}

fn on(app: &mut App, m: &str) -> bool {
    app.lists().modules.iter().any(|x| x == m)
}

fn vendor_names(app: &mut App) -> Vec<String> {
    names(&app.lists().vendors, true)
}

fn accounts_of(app: &mut App, pred: impl Fn(&Value) -> bool) -> Vec<String> {
    let l = app.lists().clone();
    accounts_where(&l, pred)
}

fn pay_accounts(app: &mut App) -> Vec<String> {
    accounts_of(app, |a| {
        matches!(a.str("subtype"), "bank" | "cash" | "credit-card")
    })
}

fn line_accounts(app: &mut App) -> Vec<String> {
    accounts_of(app, |a| {
        matches!(a.str("type"), "expense" | "asset" | "liability") && a.str("subtype") != "payable"
    })
}

pub(crate) fn view(app: &mut App, ui: &mut Ui) {
    let modules = app.lists().modules.clone();
    ui.title("Purchases");
    let tabs: Vec<&str> = TABS
        .iter()
        .filter(|(_, m)| m.is_none_or(|m| modules.iter().any(|x| x == m)))
        .map(|t| t.0)
        .collect();
    if app.purchases.tab >= tabs.len() {
        app.purchases.tab = 0;
    }
    ui.toolbar(|ui| {
        for (i, t) in tabs.iter().enumerate() {
            let selected = app.purchases.tab == i && app.purchases.editor.is_none();
            if ui.tab(*t, selected).clicked() {
                app.purchases.tab = i;
                app.purchases.editor = None;
                app.purchases.vendor = None;
            }
        }
    });
    if app.purchases.editor.is_some() {
        ui.scope("editor", |ui| editor(app, ui));
    } else {
        let tab = tabs.get(app.purchases.tab).copied().unwrap_or("Bills");
        ui.scope(tab, |ui| match tab {
            "Payments" => payments_tab(app, ui),
            "Purchase orders" => pos_tab(app, ui),
            "Vendors" => match app.purchases.vendor.clone() {
                Some(id) => vendor_view(app, ui, &id),
                None => vendors_tab(app, ui),
            },
            "1099" => ten99_tab(app, ui),
            _ => bills_tab(app, ui),
        });
    }
    dialogs(app, ui);
}

// ----- lists ----------------------------------------------------------------------------

fn status_label(s: &str) -> &'static str {
    match s {
        "open" => "Open",
        "partial" => "Partly paid",
        "paid" => "Paid",
        "void" => "Void",
        "partly_received" => "Partly received",
        "received" => "Received",
        "closed" => "Closed",
        "cancelled" => "Cancelled",
        _ => "",
    }
}

fn chip(ui: &mut Ui, s: &str) {
    let tone = match s {
        "paid" | "received" | "closed" => Tone::Success,
        "void" | "cancelled" => Tone::Secondary,
        _ => Tone::Accent,
    };
    ui.badge(status_label(s), tone);
}

pub(crate) fn new_editor(app: &mut App, kind: &'static str, vendor: &str) {
    app.purchases.editor = Some(Editor {
        kind,
        status: "open".into(),
        vendor: vendor.to_owned(),
        date: fmt::date(&app.today()),
        lines: vec![LineDraft::default()],
        ..Editor::default()
    });
}

/// Make a bill (subscriptions.md): a new bill filled from a subscription, opened in
/// the bill editor for the user to check and save.
pub(crate) fn new_bill_for(app: &mut App, vendor: &str, account: &str, amount: &str, date: &str) {
    new_editor(app, "bill", vendor);
    if let Some(e) = app.purchases.editor.as_mut() {
        if !date.is_empty() {
            e.date = fmt::date(date);
        }
        e.lines = vec![LineDraft {
            what: account.to_owned(),
            description: vendor.to_owned(),
            amount: amount.to_owned(),
            ..LineDraft::default()
        }];
    }
}

/// Enter a schedule that creates a bill (subscriptions.md revision 2): a new bill
/// filled from the schedule for its due date; saving enters the occurrence.
pub(crate) fn new_bill_from_schedule(
    app: &mut App,
    vendor: &str,
    lines: &[(String, String, String)],
    date: &str,
    memo: &str,
    occurrence: (String, String),
) {
    new_editor(app, "bill", vendor);
    if let Some(e) = app.purchases.editor.as_mut() {
        if !date.is_empty() {
            e.date = fmt::date(date);
        }
        e.memo = memo.to_owned();
        e.lines = lines
            .iter()
            .map(|(account, description, amount)| LineDraft {
                what: account.clone(),
                description: description.clone(),
                amount: amount.clone(),
                ..LineDraft::default()
            })
            .collect();
        e.from_schedule = Some(occurrence);
    }
}

pub(crate) fn open_editor(app: &mut App, kind: &'static str, id: &str) {
    let (cmd, key) = if kind == "po" {
        ("po.get", "po")
    } else {
        ("bill.get", "bill")
    };
    let Some(d) = app.read(cmd, json!({key: id})) else {
        return;
    };
    let lines = d
        .list("computed")
        .iter()
        .zip(d.list("lines"))
        .map(|(c, l)| LineDraft {
            item_line: !c.at("item").is_null(),
            what: if c.at("item").is_null() {
                c.str("account_name").to_owned()
            } else {
                c.str("item_name").to_owned()
            },
            description: l.str("description").to_owned(),
            quantity: l.str("quantity").to_owned(),
            cost: l.str("cost").to_owned(),
            amount: l.str("amount").to_owned(),
            tags: Tags::from_names(l),
        })
        .collect();
    app.purchases.editor = Some(Editor {
        kind,
        id: Some(d.str("id").to_owned()),
        status: d.str("status").to_owned(),
        vendor: d.str("vendor_name").to_owned(),
        number: d.str("number").to_owned(),
        date: fmt::date(d.str("date")),
        terms: if d.at("terms").is_null() {
            0
        } else {
            terms_index(d.at("terms"))
        },
        due_date: fmt::date(d.str("due_date")),
        expected: fmt::date(d.str("expected_date")),
        ship_to: d
            .list("ship_to")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        memo: d.str("memo").to_owned(),
        message: d.str("message").to_owned(),
        lines,
        ..Editor::default()
    });
}

fn bills_tab(app: &mut App, ui: &mut Ui) {
    let vendors = vendor_names(app);
    ui.wrap(|ui| {
        if ui.primary("New bill").clicked() {
            new_editor(app, "bill", "");
        }
        if ui.button("Pay bills…").clicked() {
            app.purchases.run = Some(RunForm {
                date: fmt::date(&app.today()),
                ..RunForm::default()
            });
        }
        choice(ui, "Bill filter", &mut app.purchases.filter, &BILL_FILTERS);
        pick(
            ui,
            "Vendor filter",
            &mut app.purchases.vendor_filter,
            &vendors,
            Some("All vendors"),
        );
    });
    let today = app.today();
    let mut args = json!({"as_of": today});
    put(&mut args, "vendor", &app.purchases.vendor_filter);
    match app.purchases.filter {
        1 => set(&mut args, "status", json!("open")),
        2 => set(&mut args, "overdue", json!(true)),
        3 => {
            let week = today
                .parse::<mk_core::Date>()
                .ok()
                .and_then(|d| d.add_days(7))
                .map(|d| d.to_string())
                .unwrap_or_default();
            set(&mut args, "due_before", json!(week));
        }
        _ => {}
    }
    let rows = array(app.read("bill.list", args));
    if rows.is_empty() {
        ui.card("", |ui| ui.label("No bills."));
        return;
    }
    let cols = vec![
        Column {
            weight: 3,
            ..col("Vendor")
        },
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
            ui.table_row(&format!("bill:{}", r.str("id")), false, false, |ui| {
                ui.tip("Open the bill");
                if ui.link(r.str("vendor_name")).clicked() {
                    open = Some(r.str("id").to_owned());
                }
                ui.label(r.str("number"));
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
        open_editor(app, "bill", &id);
    }
}

fn payments_tab(app: &mut App, ui: &mut Ui) {
    let rows = array(app.read("bill_payment.list", json!({})));
    if rows.is_empty() {
        ui.card("", |ui| ui.label("No bill payments yet."));
        return;
    }
    let cols = vec![
        Column {
            width: 110,
            ..col("Date")
        },
        Column {
            weight: 3,
            ..col("Vendor")
        },
        Column {
            width: 120,
            ..col("Method")
        },
        Column {
            width: 80,
            ..col("Check")
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
            ..col("")
        },
    ];
    let mut void = None;
    ui.table("", cols, |ui| {
        for r in &rows {
            ui.table_row(&format!("bp:{}", r.str("id")), false, false, |ui| {
                ui.label(fmt::date(r.str("date")));
                cell(ui, r.str("vendor_name"));
                ui.label(
                    METHODS
                        .iter()
                        .find(|m| m.0 == r.str("method"))
                        .map_or("", |m| m.1),
                );
                ui.label(
                    r.at("check_number")
                        .as_i64()
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                );
                money(ui, r.str("amount"));
                money(ui, r.str("unapplied"));
                if r.at("void").as_bool() == Some(true) {
                    ui.badge("Void", Tone::Secondary);
                } else {
                    ui.tip("Void this payment; its bills reopen");
                    if ui.button("Void…").clicked() {
                        let label = match r.at("check_number").as_i64() {
                            Some(n) => format!("check {n}"),
                            None => format!("the payment of {}", fmt::group(r.str("amount"))),
                        };
                        void = Some((r.str("id").to_owned(), label));
                    }
                }
            });
        }
    });
    if void.is_some() {
        app.purchases.void_payment = void;
    }
}

fn pos_tab(app: &mut App, ui: &mut Ui) {
    ui.row(|ui| {
        if ui.primary("New purchase order").clicked() {
            new_editor(app, "po", "");
        }
    });
    let rows = array(app.read("po.list", json!({})));
    if rows.is_empty() {
        ui.card("", |ui| ui.label("No purchase orders yet."));
        return;
    }
    let cols = vec![
        Column {
            width: 110,
            ..col("Number")
        },
        Column {
            weight: 3,
            ..col("Vendor")
        },
        Column {
            width: 110,
            ..col("Date")
        },
        Column {
            width: 110,
            ..col("Expected")
        },
        Column {
            width: 120,
            ..col_num("Total")
        },
        Column {
            width: 140,
            ..col("Status")
        },
    ];
    let mut open = None;
    ui.table("", cols, |ui| {
        for r in &rows {
            ui.table_row(&format!("po:{}", r.str("id")), false, false, |ui| {
                if ui.link(r.str("number")).clicked() {
                    open = Some(r.str("id").to_owned());
                }
                cell(ui, r.str("vendor_name"));
                ui.label(fmt::date(r.str("date")));
                ui.label(fmt::date(r.str("expected_date")));
                money(ui, r.str("total"));
                chip(ui, r.str("status"));
            });
        }
    });
    if let Some(id) = open {
        open_editor(app, "po", &id);
    }
}

fn vendors_tab(app: &mut App, ui: &mut Ui) {
    ui.row(|ui| {
        if ui.primary("New vendor…").clicked() {
            app.purchases.vendor_form = Some(VendorForm::default());
        }
    });
    let balances = app
        .read("report.vendor_balances", json!({"as_of": app.today()}))
        .unwrap_or(Value::Null);
    let vendors = app.lists().vendors.clone();
    if vendors.is_empty() {
        ui.card("", |ui| ui.label("No vendors yet."));
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
        for v in &vendors {
            let b = balances
                .list("vendors")
                .iter()
                .find(|x| x.str("vendor") == v.str("id"))
                .cloned()
                .unwrap_or(Value::Null);
            let or_zero = |s: &str| {
                if s.is_empty() {
                    "0.00".to_owned()
                } else {
                    s.to_owned()
                }
            };
            ui.table_row(&format!("vendor:{}", v.str("id")), false, false, |ui| {
                if ui.link(v.str("name")).clicked() {
                    open = Some(v.str("id").to_owned());
                }
                money(ui, &or_zero(b.str("open")));
                money(ui, &or_zero(b.str("overdue")));
                money(ui, &or_zero(b.str("credits").trim_start_matches('-')));
            });
        }
    });
    if open.is_some() {
        app.purchases.vendor = open;
    }
}

fn vendor_form_from(app: &mut App, v: &Value) -> VendorForm {
    let accounts = app.lists().accounts.clone();
    let expense = v
        .at("default_expense_account")
        .as_str()
        .and_then(|id| accounts.iter().find(|a| a.str("id") == id))
        .map(|a| a.str("full_name").to_owned())
        .unwrap_or_default();
    VendorForm {
        id: Some(v.str("id").to_owned()),
        name: v.str("name").to_owned(),
        company: v.str("company").to_owned(),
        email: v.str("email").to_owned(),
        phone: v.str("phone").to_owned(),
        address: v
            .list("billing_address")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        terms: if v.at("vendor_terms").is_null() {
            0
        } else {
            terms_index(v.at("vendor_terms"))
        },
        expense,
        account_number: v.str("account_number_at_vendor").to_owned(),
        tracks: v.at("tracks_1099").as_bool() == Some(true),
        misc: v.str("form_1099") == "misc",
        misc_box: MISC_BOXES
            .iter()
            .position(|b| b.0 == v.str("misc_box"))
            .unwrap_or(2),
        tin: v.at("tin_on_file").as_bool() == Some(true),
        for_editor: false,
        error: None,
    }
}

fn vendor_view(app: &mut App, ui: &mut Ui, id: &str) {
    let Some(s) = app.read("vendor.summary", json!({"party": id})) else {
        app.purchases.vendor = None;
        return;
    };
    let v = s.at("vendor").clone();
    ui.row(|ui| {
        if ui.icon_button("back", "< Vendors", false).clicked() {
            app.purchases.vendor = None;
        }
        ui.heading(v.str("name"));
    });
    let tracks = v.at("tracks_1099").as_bool() == Some(true);
    let form = if v.str("form_1099") == "misc" {
        format!(
            "1099-MISC, {}",
            MISC_BOXES
                .iter()
                .find(|b| b.0 == v.str("misc_box"))
                .map_or("", |b| b.1)
        )
    } else {
        "1099-NEC".to_owned()
    };
    ui.card("", |ui| {
        for (k, val) in [
            ("Company", v.str("company").to_owned()),
            ("Email", v.str("email").to_owned()),
            ("Phone", v.str("phone").to_owned()),
            (
                "Address",
                v.list("billing_address")
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            (
                "Our account number",
                v.str("account_number_at_vendor").to_owned(),
            ),
            (
                "1099",
                if tracks {
                    form.clone()
                } else {
                    "Not tracked".to_owned()
                },
            ),
            (
                "TIN on file",
                if v.at("tin_on_file").as_bool() == Some(true) {
                    "Yes"
                } else {
                    "No"
                }
                .to_owned(),
            ),
        ] {
            ui.form(k, |ui| ui.label(val));
        }
    });
    ui.row(|ui| {
        if ui.button("Edit…").clicked() {
            app.purchases.vendor_form = Some(vendor_form_from(app, &v));
        }
        if ui.button("New bill").clicked() {
            new_editor(app, "bill", v.str("name"));
        }
        let credit = s.str("credit").to_owned();
        ui.enabled(!fmt::is_zero(&credit));
        if ui.button("Refund received…").clicked() {
            app.purchases.refund = Some(VendorRefund {
                vendor: v.str("name").to_owned(),
                amount: credit,
                date: fmt::date(&app.today()),
                ..VendorRefund::default()
            });
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
            width: 120,
            ..col("Bill")
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
    ui.table("Open bills", cols, |ui| {
        for r in s.list("open_bills") {
            let label = if r.str("number").is_empty() {
                fmt::date(r.str("date"))
            } else {
                r.str("number").to_owned()
            };
            ui.table_row(&format!("ob:{}", r.str("id")), false, false, |ui| {
                if ui.link(label.clone()).clicked() {
                    open = Some(r.str("id").to_owned());
                }
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
            weight: 1,
            ..col("")
        },
        Column {
            width: 120,
            ..col_num("Amount")
        },
        Column {
            width: 80,
            ..col("")
        },
    ];
    ui.table("Payments", cols, |ui| {
        for (i, p) in s.list("payments").iter().enumerate() {
            ui.table_row(&format!("vp:{i}"), false, false, |ui| {
                ui.label(fmt::date(p.str("date")));
                ui.label(
                    p.at("check_number")
                        .as_i64()
                        .map(|n| format!("Check {n}"))
                        .unwrap_or_default(),
                );
                money(ui, p.str("amount"));
                ui.label(if p.at("void").as_bool() == Some(true) {
                    "void"
                } else {
                    ""
                });
            });
        }
        for (i, t) in s.list("tax_payments").iter().enumerate() {
            ui.table_row(&format!("vt:{i}"), false, false, |ui| {
                ui.label(fmt::date(t.str("date")));
                ui.label("Sales tax");
                money(ui, t.str("amount"));
                ui.label("");
            });
        }
    });
    if let Some(b) = open {
        open_editor(app, "bill", &b);
    }
}

fn ten99_tab(app: &mut App, ui: &mut Ui) {
    if app.purchases.year.is_empty() {
        app.purchases.year = app.today().get(..4).unwrap_or("2026").to_owned();
    }
    ui.row(|ui| {
        ui.label("Year");
        ui.width(72);
        ui.field("Year", &mut app.purchases.year);
        ui.checkbox(
            &mut app.purchases.include_below,
            "Include vendors below the threshold",
        );
    });
    let Ok(year) = app.purchases.year.trim().parse::<i32>() else {
        ui.error_text("Type a year such as 2026.");
        return;
    };
    let args = json!({"year": year, "include_below_threshold": app.purchases.include_below});
    let Some(r) = app.read("report.1099", args.clone()) else {
        return;
    };
    ui.secondary(r.str("note"));
    ui.row(|ui| {
        if ui.button("Export CSV…").clicked()
            && let Some(e) = app.read(
                "report.export",
                json!({"report": "1099", "args": args, "format": "csv"}),
            )
        {
            app.emit(AppEvent::SaveFile {
                file_name: e.str("file_name").to_owned(),
                content: e.str("content").to_owned(),
            });
        }
    });
    let cols = vec![
        Column {
            weight: 3,
            ..col("Vendor")
        },
        Column {
            width: 100,
            ..col("Form")
        },
        Column {
            width: 120,
            ..col("Box")
        },
        Column {
            width: 110,
            ..col_num("Paid")
        },
        Column {
            width: 150,
            ..col_num("Card payments (excluded)")
        },
        Column {
            width: 90,
            ..col("TIN on file")
        },
        Column {
            weight: 2,
            ..col("Flags")
        },
    ];
    ui.table("", cols, |ui| {
        for v in r.list("vendors") {
            let note = format!(
                "{} paid {}, card payments excluded {}{}",
                v.str("vendor_name"),
                fmt::group(v.str("total")),
                fmt::group(v.str("excluded_card")),
                if v.at("file").as_bool() == Some(true) {
                    ", flagged to file"
                } else {
                    ""
                }
            );
            ui.table_row(&note, false, true, |ui| {
                cell(ui, v.str("vendor_name"));
                ui.label(v.str("form"));
                ui.label(v.str("box"));
                money(ui, v.str("total"));
                money(ui, v.str("excluded_card"));
                ui.label(if v.at("tin_on_file").as_bool() == Some(true) {
                    "Yes"
                } else {
                    "No"
                });
                ui.row(|ui| {
                    for f in v.list("flags") {
                        let t = match f.as_str().unwrap_or_default() {
                            "file" => "File 1099",
                            other => other,
                        };
                        ui.badge(t, Tone::Accent);
                    }
                });
            });
        }
    });
    if app.purchases.thresholds_year != app.purchases.year {
        app.purchases.thresholds_year = app.purchases.year.clone();
        app.purchases.thresholds = r
            .at("thresholds")
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        app.purchases.thresholds_error = None;
    }
    let mut save = false;
    ui.card(&format!("Thresholds for {year}"), |ui| {
        for (k, v) in &mut app.purchases.thresholds {
            let label = match k.as_str() {
                "nec" => "NEC nonemployee compensation".to_owned(),
                other => MISC_BOXES
                    .iter()
                    .find(|b| b.0 == other)
                    .map_or(other.to_owned(), |b| format!("MISC {}", b.1)),
            };
            ui.form(&label, |ui| {
                ui.width(120).mono().align(Align::Right);
                ui.field(&format!("Threshold {label}"), v);
            });
        }
        if let Some(e) = &app.purchases.thresholds_error {
            ui.error_text(e.clone());
        }
        ui.row(|ui| {
            save = ui.button("Save thresholds").clicked();
        });
    });
    if save {
        let mut table = app
            .backend
            .invoke("module.settings", json!({"module": "1099"}))
            .ok()
            .and_then(|s| {
                s.list("settings")
                    .iter()
                    .find(|x| x.str("key") == "1099.thresholds")
                    .and_then(|x| x.at("value").as_object().cloned())
            })
            .unwrap_or_default();
        let row: serde_json::Map<String, Value> = app
            .purchases
            .thresholds
            .iter()
            .map(|(k, v)| (k.clone(), json!(amount_arg(v))))
            .collect();
        table.insert(year.to_string(), Value::Object(row));
        match app.run(
            "module.set_setting",
            json!({"key": "1099.thresholds", "value": Value::Object(table)}),
        ) {
            Ok(_) => {
                app.purchases.thresholds_error = None;
                app.purchases.thresholds_year.clear();
                app.notify_with(format!("Thresholds for {year} saved"), MessageAction::Undo);
            }
            Err(e) => app.purchases.thresholds_error = Some(e.display()),
        }
    }
}

// ----- editor ---------------------------------------------------------------------------

fn doc_args(app: &mut App, e: &Editor, full: bool) -> Value {
    let tags_on = tags::options(app);
    let lines: Vec<Value> = e
        .lines
        .iter()
        .map(|l| {
            let mut v = json!({});
            put(
                &mut v,
                if l.item_line { "item" } else { "account" },
                &l.what,
            );
            put(&mut v, "description", &l.description);
            put(&mut v, "quantity", &amount_arg(&l.quantity));
            put(&mut v, "cost", &amount_arg(&l.cost));
            put(&mut v, "amount", &amount_arg(&l.amount));
            l.tags.apply_given(&mut v, &tags_on);
            v
        })
        .collect();
    let mut a = json!({"lines": lines});
    put(&mut a, "vendor", &e.vendor);
    put(&mut a, "date", &fmt::date_arg_lenient(&e.date));
    if e.kind == "bill" {
        put(&mut a, "due_date", &fmt::date_arg_lenient(&e.due_date));
        if let Some(t) = terms_value(e.terms) {
            set(&mut a, "terms", t);
        }
    } else {
        put(&mut a, "expected_date", &fmt::date_arg_lenient(&e.expected));
        let ship: Vec<&str> = e.ship_to.lines().filter(|l| !l.trim().is_empty()).collect();
        set(&mut a, "ship_to", json!(ship));
    }
    if full {
        put(&mut a, "number", &e.number);
        set(&mut a, "memo", json!(e.memo));
        if e.kind == "po" {
            set(&mut a, "message", json!(e.message));
        }
        e.tags.apply(&mut a, &tags_on);
        // A bill's Billable flag is per line (projects.md).
        if let Some(o) = a.as_object_mut() {
            o.remove("billable");
        }
    }
    a
}

fn refresh_preview(app: &mut App, e: &mut Editor) {
    if e.vendor.trim().is_empty() {
        e.preview = None;
        e.preview_error = None;
        return;
    }
    let mut a = doc_args(app, e, false);
    set(&mut a, "kind", json!(e.kind));
    if let Some(id) = &e.id {
        set(&mut a, "bill", json!(id));
    }
    let key = format!("{}|{a}", app.generation);
    if key == e.preview_key {
        return;
    }
    e.preview_key = key;
    match app.backend.invoke("bill.preview", a) {
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
    let Some(mut e) = app.purchases.editor.take() else {
        return;
    };
    refresh_preview(app, &mut e);
    let title = if e.kind == "po" {
        "Purchase order"
    } else {
        "Bill"
    };
    let mut close = false;
    ui.row(|ui| {
        if ui.icon_button("back", "< Back", false).clicked() {
            close = true;
        }
        let heading = match (&e.id, e.number.is_empty()) {
            (Some(_), false) => format!("{title} {}", e.number),
            (Some(_), true) => title.to_owned(),
            (None, _) => format!("New {}", title.to_lowercase()),
        };
        ui.heading(heading);
        if e.id.is_some() {
            chip(ui, &e.status);
        }
    });
    if e.id.is_none() && e.from_schedule.is_some() {
        ui.banner(Tone::Accent, |ui| {
            ui.label("Entering a scheduled bill: check the amount, then Save to post it to Accounts Payable.");
        });
    }
    let vendors = vendor_names(app);
    let mut new_vendor = false;
    let mut status_pick = None;
    ui.card("", |ui| {
        ui.form("Vendor", |ui| {
            pick(ui, "Vendor", &mut e.vendor, &vendors, None);
            if ui.button("New vendor…").clicked() {
                new_vendor = true;
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
        if e.kind == "bill" {
            ui.form("Terms", |ui| {
                let l: Vec<&str> = TERMS
                    .iter()
                    .map(|t| if t.1.is_none() { "Vendor default" } else { t.0 })
                    .collect();
                choice(ui, "Terms", &mut e.terms, &l);
            });
            let hint = e
                .preview
                .as_ref()
                .map(|p| fmt::date(p.str("due_date")))
                .unwrap_or_default();
            ui.form("Due date", |ui| {
                if e.due_date.is_empty() && !hint.is_empty() {
                    ui.width(140).hint(hint);
                    ui.field("Due date", &mut e.due_date);
                } else {
                    super::register::date_field(ui, "Due date", &mut e.due_date);
                }
            });
        } else {
            ui.form("Expected", |ui| {
                super::register::date_field(ui, "Expected date", &mut e.expected)
            });
            ui.form("Ship to", |ui| {
                ui.width(240);
                ui.multiline("Ship to", &mut e.ship_to);
            });
            if e.id.is_some() {
                ui.form("Status", |ui| {
                    let vals = ["open", "closed", "cancelled"];
                    let mut i = vals.iter().position(|v| *v == e.status).unwrap_or(0);
                    if choice(ui, "Order status", &mut i, &["Open", "Closed", "Cancelled"]) {
                        status_pick = vals.get(i).copied();
                    }
                });
            }
        }
    });
    if let Some(s) = status_pick {
        match app.run("po.set_status", json!({"po": e.id, "status": s})) {
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
    if new_vendor {
        app.purchases.vendor_form = Some(VendorForm {
            for_editor: true,
            ..VendorForm::default()
        });
    }
    lines_grid(app, ui, &mut e);
    if let Some(p) = &e.preview {
        ui.row(|ui| {
            ui.spacer();
            ui.bold();
            ui.label("Total");
            ui.bold()
                .named(format!("Total {}", fmt::group(p.str("total"))));
            ui.amount(fmt::group(p.str("total")));
        });
    }
    ui.card("", |ui| {
        if e.kind == "po" {
            ui.form("Message", |ui| {
                ui.width(420);
                ui.multiline("Message", &mut e.message);
            });
        }
        ui.form("Memo", |ui| {
            ui.width(420);
            ui.field("Memo", &mut e.memo);
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
    if app.purchases.editor.is_none() {
        app.purchases.editor = Some(e);
    }
}

fn lines_grid(app: &mut App, ui: &mut Ui, e: &mut Editor) {
    let items_on = on(app, "items");
    let items: Vec<String> = names(&app.lists().items, true);
    let accounts = line_accounts(app);
    let computed: Vec<Value> = e
        .preview
        .as_ref()
        .map(|p| p.list("lines").to_vec())
        .unwrap_or_default();
    let mut remove = None;
    let cols = vec![
        Column {
            weight: 3,
            ..col("Item or account")
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
            ..col_num("Cost")
        },
        Column {
            width: 100,
            ..col_num("Amount")
        },
        Column {
            width: 80,
            ..col("")
        },
        Column {
            width: 90,
            ..col("")
        },
    ];
    ui.table("Lines", cols, |ui| {
        for (i, l) in e.lines.iter_mut().enumerate() {
            let k = i + 1;
            let c = computed.get(i).cloned().unwrap_or(Value::Null);
            ui.table_row(&format!("line{k}"), false, false, |ui| {
                if l.item_line {
                    pick(ui, &format!("Line {k} item"), &mut l.what, &items, None);
                } else {
                    pick(
                        ui,
                        &format!("Line {k} account"),
                        &mut l.what,
                        &accounts,
                        None,
                    );
                }
                ui.hint(c.str("description"));
                ui.field(&format!("Line {k} description"), &mut l.description);
                if l.item_line {
                    ui.hint("1").align(Align::Right).mono();
                    ui.field(&format!("Line {k} quantity"), &mut l.quantity);
                    ui.hint(c.str("cost")).align(Align::Right).mono();
                    ui.field(&format!("Line {k} cost"), &mut l.cost);
                } else {
                    ui.label("");
                    ui.label("");
                }
                ui.hint(fmt::group(c.str("amount")))
                    .align(Align::Right)
                    .mono();
                ui.field(&format!("Line {k} amount"), &mut l.amount);
                if items_on {
                    let mut item = l.item_line;
                    ui.tip("An item line");
                    if ui.checkbox(&mut item, "Item").clicked() {
                        l.item_line = item;
                        l.what.clear();
                    }
                } else {
                    ui.label("");
                }
                ui.tip(format!("Remove line {k}"));
                if ui.button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
    });
    if let Some(i) = remove
        && i < e.lines.len()
        && e.lines.len() > 1
    {
        e.lines.remove(i);
    }
    ui.row(|ui| {
        if ui.button("Add expense line").clicked() {
            e.lines.push(LineDraft::default());
        }
        if items_on && ui.button("Add item line").clicked() {
            e.lines.push(LineDraft {
                item_line: true,
                ..LineDraft::default()
            });
        }
    });
}

fn buttons(ui: &mut Ui, e: &Editor) -> Option<&'static str> {
    let mut out = None;
    let ok = e.preview.is_some();
    let saved = e.id.is_some();
    ui.wrap(|ui| {
        let void = e.status == "void";
        if !void {
            ui.enabled(ok);
            if ui.primary("Save").clicked() {
                out = Some("save");
            }
        }
        if !saved {
            return;
        }
        if e.kind == "bill" {
            if !void && e.status != "paid" && ui.button("Pay…").clicked() {
                out = Some("pay");
            }
            if ui.button("Duplicate").clicked() {
                out = Some("duplicate");
            }
            if !void && ui.button("Void…").clicked() {
                out = Some("confirm_void");
            }
        } else {
            if matches!(e.status.as_str(), "open" | "partly_received")
                && ui.button("Receive…").clicked()
            {
                out = Some("receive");
            }
            if ui.button("PDF…").clicked() {
                out = Some("pdf");
            }
        }
        if ui.button("Delete…").clicked() {
            out = Some("confirm_delete");
        }
    });
    out
}

fn act(app: &mut App, e: &mut Editor, action: &'static str) {
    let kind = e.kind;
    let key = if kind == "po" { "po" } else { "bill" };
    let id = e.id.clone().unwrap_or_default();
    match action {
        "save" => {
            let mut a = doc_args(app, e, true);
            let cmd = match &e.id {
                Some(id) => {
                    set(&mut a, "bill", json!(id));
                    format!("{key}.update")
                }
                None => {
                    if let Some((schedule, nominal)) = &e.from_schedule {
                        set(
                            &mut a,
                            "from_schedule",
                            json!({"schedule": schedule, "date": nominal}),
                        );
                    }
                    format!("{key}.create")
                }
            };
            match app.run(&cmd, a) {
                Ok(v) => {
                    e.error = None;
                    let noun = if kind == "po" {
                        "Purchase order"
                    } else {
                        "Bill"
                    };
                    app.notify_with(format!("{noun} saved"), MessageAction::Undo);
                    open_editor(app, kind, v.str("id"));
                }
                Err(err) => e.error = Some(err.display()),
            }
        }
        "pay" => {
            let bal = app
                .read("bill.get", json!({"bill": id}))
                .map(|b| b.str("balance").to_owned())
                .unwrap_or_default();
            open_pay(app, &e.vendor.clone(), Some((id, bal)));
        }
        "duplicate" => match app.run("bill.duplicate", json!({"bill": id, "date": app.today()})) {
            Ok(v) => {
                app.notify_with("Bill copied", MessageAction::Undo);
                open_editor(app, "bill", v.str("id"));
            }
            Err(err) => e.error = Some(err.display()),
        },
        "receive" => {
            if let Some(po) = app.read("po.get", json!({"po": id})) {
                let received = po.list("received").to_vec();
                let lines = po
                    .list("computed")
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        let by_qty = !l.at("item").is_null();
                        let ordered: Decimal = if by_qty {
                            l.str("quantity").parse().unwrap_or_default()
                        } else {
                            l.str("amount").parse().unwrap_or_default()
                        };
                        let got: Decimal = received
                            .get(i)
                            .and_then(Value::as_str)
                            .and_then(|s| s.parse().ok())
                            .unwrap_or_default();
                        let left = ordered
                            .checked_sub(got)
                            .unwrap_or_default()
                            .max(Decimal::ZERO);
                        (
                            i + 1,
                            l.str("description").to_owned(),
                            left.normalize().to_string(),
                            by_qty,
                        )
                    })
                    .collect();
                app.purchases.receive = Some(ReceiveForm {
                    po: id,
                    date: fmt::date(&app.today()),
                    lines,
                    ..ReceiveForm::default()
                });
            }
        }
        "pdf" => {
            if let Some(p) = app.read("po.render_pdf", json!({"po": id}))
                && let Err(err) = super::sales::save_pdf(app, &p)
            {
                e.error = Some(err);
            }
        }
        "confirm_void" => e.confirm = Some("void"),
        "confirm_delete" => e.confirm = Some("delete"),
        _ => {}
    }
}

// ----- dialogs --------------------------------------------------------------------------

pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    confirm_dialog(app, ui);
    void_payment_dialog(app, ui);
    pay_dialog(app, ui);
    run_dialog(app, ui);
    receive_dialog(app, ui);
    vendor_dialog(app, ui);
    refund_dialog(app, ui);
}

fn confirm_dialog(app: &mut App, ui: &mut Ui) {
    let Some(e) = app.purchases.editor.as_ref() else {
        return;
    };
    let Some(what) = e.confirm else {
        return;
    };
    let (kind, id) = (e.kind, e.id.clone().unwrap_or_default());
    let noun = if kind == "po" {
        "purchase order"
    } else {
        "bill"
    };
    let key = if kind == "po" { "po" } else { "bill" };
    let verb = if what == "void" { "Void" } else { "Delete" };
    let mut go = false;
    let mut cancel = ui.dialog_closed("purchases-confirm");
    ui.dialog(
        "purchases-confirm",
        &format!("{verb} this {noun}?"),
        440,
        |ui| {
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
    if cancel && let Some(e) = app.purchases.editor.as_mut() {
        e.confirm = None;
    }
    if go {
        match app.run(&format!("{key}.{what}"), json!({key: id})) {
            Ok(_) => {
                app.notify_with(format!("The {noun} was {what}ed"), MessageAction::Undo);
                if what == "void" {
                    open_editor(app, kind, &id);
                } else {
                    app.purchases.editor = None;
                }
            }
            Err(err) => {
                if let Some(e) = app.purchases.editor.as_mut() {
                    e.confirm = None;
                    e.error = Some(err.display());
                }
            }
        }
    }
}

fn void_payment_dialog(app: &mut App, ui: &mut Ui) {
    let Some((id, label)) = app.purchases.void_payment.clone() else {
        return;
    };
    let mut go = false;
    let mut cancel = ui.dialog_closed("void-bill-payment");
    ui.dialog("void-bill-payment", &format!("Void {label}?"), 440, |ui| {
        ui.label("Its bills reopen. A check number stays used and shows as void.");
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Void payment").clicked() {
                go = true;
            }
        });
    });
    if cancel {
        app.purchases.void_payment = None;
    }
    if go {
        app.purchases.void_payment = None;
        match app.run("bill_payment.void", json!({"payment": id})) {
            Ok(_) => app.notify_with(format!("Voided {label}"), MessageAction::Undo),
            Err(e) => app.notify_error(&e),
        }
    }
}

fn method_for(app: &mut App, from: &str) -> usize {
    let subtype = app
        .lists()
        .accounts
        .iter()
        .find(|a| a.str("full_name") == from)
        .map(|a| a.str("subtype").to_owned())
        .unwrap_or_default();
    match subtype.as_str() {
        "credit-card" => 2,
        "cash" => 3,
        _ => 0,
    }
}

fn next_check(app: &mut App, from: &str) -> String {
    app.backend
        .invoke("bill_payment.next_check", json!({"account": from}))
        .ok()
        .and_then(|v| v.at("next").as_i64())
        .map(|n| n.to_string())
        .unwrap_or_default()
}

pub(crate) fn open_pay(app: &mut App, vendor: &str, bill: Option<(String, String)>) {
    let amount = bill.as_ref().map(|b| b.1.clone()).unwrap_or_default();
    app.purchases.pay = Some(PayForm {
        vendor: vendor.to_owned(),
        bill,
        date: fmt::date(&app.today()),
        amount,
        ..PayForm::default()
    });
}

fn method_rows(ui: &mut Ui, method: &mut usize, check: &mut String, check_label: &str, hint: &str) {
    ui.form("Method", |ui| {
        let l: Vec<&str> = METHODS.iter().map(|m| m.1).collect();
        choice(ui, "Method", method, &l);
    });
    if *method == 0 {
        text_row(ui, check_label, check, hint);
    }
}

fn pay_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.purchases.pay.take() else {
        return;
    };
    let from = pay_accounts(app);
    let mut save = false;
    let mut cancel = ui.dialog_closed("pay-bill");
    let mut from_changed = false;
    ui.dialog("pay-bill", &format!("Pay {}", f.vendor), 500, |ui| {
        ui.group(|ui| {
            ui.form("Pay from", |ui| {
                from_changed = pick(ui, "Pay from", &mut f.from, &from, None);
            });
            date_row(ui, "Payment date", &mut f.date);
            method_rows(
                ui,
                &mut f.method,
                &mut f.check,
                "Check number",
                "first check: type its number",
            );
            text_row(ui, "Payment amount", &mut f.amount, "0.00");
        });
        if let Some(err) = &f.error {
            ui.error_text(err.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Pay").clicked() {
                save = true;
            }
        });
    });
    if from_changed {
        f.method = method_for(app, &f.from);
        f.check = next_check(app, &f.from);
    }
    if cancel {
        return;
    }
    if save {
        let mut a = json!({"vendor": f.vendor, "date": fmt::date_arg_lenient(&f.date), "amount": amount_arg(&f.amount)});
        put(&mut a, "from_account", &f.from);
        if let Some((m, _)) = METHODS.get(f.method) {
            set(&mut a, "method", json!(m));
        }
        if f.method == 0
            && let Ok(n) = f.check.trim().parse::<i64>()
        {
            set(&mut a, "check_number", json!(n));
        }
        if let Some((bill, _)) = &f.bill {
            set(
                &mut a,
                "apply",
                json!([{"bill": bill, "amount": amount_arg(&f.amount)}]),
            );
        }
        match app.run("bill.pay", a) {
            Ok(v) => {
                let check = v
                    .at("check_number")
                    .as_i64()
                    .map(|n| format!(" with check {n}"))
                    .unwrap_or_default();
                app.notify_with(
                    format!("Paid {}{check}", fmt::group(v.str("amount"))),
                    MessageAction::Undo,
                );
                if let Some(id) = app.purchases.editor.as_ref().and_then(|e| e.id.clone()) {
                    open_editor(app, "bill", &id);
                }
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.purchases.pay = Some(f);
}

fn run_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.purchases.run.take() else {
        return;
    };
    let today = app.today();
    let mut args = json!({"as_of": today});
    if f.overdue_only {
        set(&mut args, "overdue", json!(true));
    }
    if let Ok(Some(d)) = fmt::read_date(&f.due_by) {
        set(&mut args, "due_before", json!(d));
    }
    let bills: Vec<Value> = array(app.backend.invoke("bill.list", args).ok())
        .into_iter()
        .filter(|b| matches!(b.str("status"), "open" | "partial"))
        .collect();
    for b in &bills {
        if !f.chosen.iter().any(|c| c.0 == b.str("id")) {
            f.chosen
                .push((b.str("id").to_owned(), false, b.str("balance").to_owned()));
        }
    }
    let from = pay_accounts(app);
    let mut save = false;
    let mut cancel = ui.dialog_closed("pay-bills");
    let mut from_changed = false;
    ui.dialog("pay-bills", "Pay bills", 680, |ui| {
        ui.group(|ui| {
            ui.form("Pay from", |ui| {
                from_changed = pick(ui, "Pay from", &mut f.from, &from, None);
            });
            date_row(ui, "Payment date", &mut f.date);
            method_rows(ui, &mut f.method, &mut f.check, "First check number", "");
        });
        ui.row(|ui| {
            ui.label("Due by");
            super::register::date_field(ui, "Due by", &mut f.due_by);
            ui.checkbox(&mut f.overdue_only, "Overdue");
        });
        if bills.is_empty() {
            ui.label("No bills to pay.");
        }
        let cols = vec![
            Column {
                width: 40,
                ..col("")
            },
            Column {
                weight: 2,
                ..col("Vendor")
            },
            Column {
                width: 100,
                ..col("Number")
            },
            Column {
                width: 110,
                ..col("Due")
            },
            Column {
                width: 120,
                ..col_num("Amount")
            },
        ];
        ui.table("", cols, |ui| {
            ui.visible_rows(6);
            for b in &bills {
                let Some(c) = f.chosen.iter_mut().find(|c| c.0 == b.str("id")) else {
                    continue;
                };
                let label = format!(
                    "Pay {} {} due {} balance {}",
                    b.str("vendor_name"),
                    b.str("number"),
                    fmt::date(b.str("due_date")),
                    fmt::group(b.str("balance"))
                );
                let label = label.trim().to_owned();
                ui.table_row(&format!("run:{}", b.str("id")), false, false, |ui| {
                    ui.named(label.clone());
                    ui.checkbox(&mut c.1, "");
                    ui.label(b.str("vendor_name"));
                    ui.label(b.str("number"));
                    ui.label(fmt::date(b.str("due_date")));
                    ui.mono().align(Align::Right);
                    ui.field(&format!("Amount for {label}"), &mut c.2);
                });
            }
        });
        let total = f
            .chosen
            .iter()
            .filter(|c| c.1 && bills.iter().any(|b| b.str("id") == c.0))
            .filter_map(|c| amount_arg(&c.2).parse::<Decimal>().ok())
            .fold(Decimal::ZERO, |s, x| s.checked_add(x).unwrap_or(s));
        ui.label(format!(
            "Total {}",
            fmt::group(&total.round_dp(2).to_string())
        ));
        if let Some(err) = &f.error {
            ui.error_text(err.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Pay selected").clicked() {
                save = true;
            }
        });
    });
    if from_changed {
        f.method = method_for(app, &f.from);
        f.check = next_check(app, &f.from);
    }
    if cancel {
        return;
    }
    if save {
        let chosen: Vec<Value> = f
            .chosen
            .iter()
            .filter(|c| c.1 && bills.iter().any(|b| b.str("id") == c.0))
            .map(|c| json!({"bill": c.0, "amount": amount_arg(&c.2)}))
            .collect();
        let mut a = json!({"date": fmt::date_arg_lenient(&f.date), "bills": chosen});
        put(&mut a, "from_account", &f.from);
        if let Some((m, _)) = METHODS.get(f.method) {
            set(&mut a, "method", json!(m));
        }
        if f.method == 0
            && let Ok(n) = f.check.trim().parse::<i64>()
        {
            set(&mut a, "first_check_number", json!(n));
        }
        match app.run("bill.pay_many", a) {
            Ok(v) => {
                let n = v.list("payments").len();
                app.notify_with(format!("Recorded {n} payment(s)"), MessageAction::Undo);
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.purchases.run = Some(f);
}

fn receive_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.purchases.receive.take() else {
        return;
    };
    let mut save = false;
    let mut cancel = ui.dialog_closed("receive-po");
    ui.dialog("receive-po", "Receive", 540, |ui| {
        ui.group(|ui| {
            date_row(ui, "Received date", &mut f.date);
            text_row(ui, "Vendor invoice number", &mut f.bill_number, "optional");
            for (n, label, amount, by_qty) in &mut f.lines {
                let what = if *by_qty { "Quantity" } else { "Amount" };
                let name = format!("{what} received for line {n}");
                ui.form(&format!("{n}. {label}"), |ui| {
                    ui.width(110).mono().align(Align::Right);
                    ui.field(&name, amount);
                });
            }
        });
        if let Some(err) = &f.error {
            ui.error_text(err.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Create bill").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if save {
        let lines: Vec<Value> = f
            .lines
            .iter()
            .filter(|l| {
                amount_arg(&l.2)
                    .parse::<Decimal>()
                    .is_ok_and(|d| d > Decimal::ZERO)
            })
            .map(|(n, _, amount, by_qty)| {
                let key = if *by_qty { "quantity" } else { "amount" };
                json!({"po_line": n, key: amount_arg(amount)})
            })
            .collect();
        let mut a = json!({"po": f.po, "date": fmt::date_arg_lenient(&f.date), "lines": lines});
        put(&mut a, "bill_number", &f.bill_number);
        match app.run("po.receive", a) {
            Ok(v) => {
                for w in v.list("warnings") {
                    app.notify(w.as_str().unwrap_or_default().to_owned());
                }
                if v.list("warnings").is_empty() {
                    app.notify_with(
                        format!("Bill of {} created", fmt::group(v.at("bill").str("total"))),
                        MessageAction::Undo,
                    );
                }
                let po = f.po.clone();
                open_editor(app, "po", &po);
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.purchases.receive = Some(f);
}

fn vendor_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.purchases.vendor_form.take() else {
        return;
    };
    let accounts = line_accounts(app);
    let ten99 = on(app, "1099");
    let mut save = false;
    let mut cancel = ui.dialog_closed("vendor-form");
    let title = if f.id.is_some() {
        "Edit vendor"
    } else {
        "New vendor"
    };
    ui.dialog("vendor-form", title, 580, |ui| {
        ui.group(|ui| {
            text_row(ui, "Vendor name", &mut f.name, "");
            text_row(ui, "Company", &mut f.company, "optional");
            text_row(ui, "Email", &mut f.email, "optional");
            text_row(ui, "Phone", &mut f.phone, "optional");
            ui.form("Address", |ui| {
                ui.width(240);
                ui.multiline("Address", &mut f.address);
            });
            ui.form("Terms", |ui| {
                let l: Vec<&str> = TERMS.iter().map(|t| if t.1.is_none() { "Default bill terms" } else { t.0 }).collect();
                choice(ui, "Vendor terms", &mut f.terms, &l);
            });
            ui.form("Default expense account", |ui| {
                pick(ui, "Default expense account", &mut f.expense, &accounts, Some("None"));
            });
            text_row(ui, "Our account number", &mut f.account_number, "optional");
            if ten99 {
                ui.form("", |ui| {
                    ui.checkbox(&mut f.tracks, "Track for 1099");
                });
                if f.tracks {
                    ui.form("Form", |ui| {
                        let mut i = usize::from(f.misc);
                        choice(ui, "1099 form", &mut i, &["1099-NEC", "1099-MISC"]);
                        f.misc = i == 1;
                    });
                    if f.misc {
                        ui.form("Box", |ui| {
                            let l: Vec<&str> = MISC_BOXES.iter().map(|b| b.1).collect();
                            choice(ui, "MISC box", &mut f.misc_box, &l);
                        });
                    }
                    ui.form("", |ui| {
                        ui.checkbox(&mut f.tin, "TIN on file");
                    });
                }
            }
            ui.secondary(
                "MoneyKeep never stores the taxpayer id number itself; it only records that you have it on file.",
            );
        });
        if let Some(err) = &f.error {
            ui.error_text(err.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save vendor").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if save {
        let address: Vec<&str> = f.address.lines().filter(|l| !l.trim().is_empty()).collect();
        let mut a = json!({
            "name": f.name, "company": f.company, "email": f.email, "phone": f.phone,
            "billing_address": address, "vendor_terms": terms_value(f.terms).unwrap_or(Value::Null),
            "default_expense_account": f.expense, "account_number_at_vendor": f.account_number,
        });
        if ten99 {
            set(&mut a, "tracks_1099", json!(f.tracks));
            set(
                &mut a,
                "form_1099",
                json!(if f.misc { "misc" } else { "nec" }),
            );
            if f.misc
                && let Some((b, _)) = MISC_BOXES.get(f.misc_box)
            {
                set(&mut a, "misc_box", json!(b));
            }
            set(&mut a, "tin_on_file", json!(f.tin));
        }
        let cmd = match &f.id {
            Some(id) => {
                set(&mut a, "party", json!(id));
                "party.update"
            }
            None => {
                set(&mut a, "roles", json!(["vendor"]));
                "party.create"
            }
        };
        match app.run(cmd, a) {
            Ok(v) => {
                app.notify_with(format!("{} saved", v.str("name")), MessageAction::Undo);
                if f.for_editor
                    && let Some(e) = app.purchases.editor.as_mut()
                {
                    e.vendor = v.str("name").to_owned();
                }
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.purchases.vendor_form = Some(f);
}

fn refund_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.purchases.refund.take() else {
        return;
    };
    let to = accounts_of(app, |a| matches!(a.str("subtype"), "bank" | "cash"));
    let mut save = false;
    let mut cancel = ui.dialog_closed("vendor-refund");
    ui.dialog(
        "vendor-refund",
        &format!("Refund from {}", f.vendor),
        460,
        |ui| {
            ui.group(|ui| {
                ui.form("Deposit to", |ui| {
                    pick(ui, "Deposit to", &mut f.to, &to, None);
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
        },
    );
    if cancel {
        return;
    }
    if save {
        let a = json!({"vendor": f.vendor, "to_account": f.to, "amount": amount_arg(&f.amount), "date": fmt::date_arg_lenient(&f.date)});
        match app.run("vendor.refund_received", a) {
            Ok(v) => {
                app.notify_with(
                    format!("Refund of {} recorded", fmt::group(v.str("amount"))),
                    MessageAction::Undo,
                );
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.purchases.refund = Some(f);
}
