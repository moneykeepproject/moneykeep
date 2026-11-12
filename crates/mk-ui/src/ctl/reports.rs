//! Reports: the report list on the left, the selected report on the right with its
//! controls, a chart (where it has one) and the exact table from the command.

use mk_core::Decimal;
use serde_json::{Value, json};

use super::charts;
use super::form::set;
use super::period_table::{self, Kind, Row};
use super::ui::{ChartHit, Column, Ui, col, col_num};
use super::util::Json;
use super::{App, AppEvent};
use crate::View;
use crate::fmt;

pub(crate) use crate::base::reports::REPORTS;

pub(crate) const PRESETS: [(&str, &str); 9] = [
    ("this_month", "This month"),
    ("last_month", "Last month"),
    ("this_quarter", "This quarter"),
    ("this_year", "This year"),
    ("year_to_date", "Year to date"),
    ("last_12_months", "Last 12 months"),
    ("last_year", "Last year"),
    ("all", "All"),
    ("custom", "Custom…"),
];

#[derive(Debug, Default)]
pub(crate) struct ReportsState {
    pub report: usize,
    pub preset: usize,
    pub from: String,
    pub to: String,
    pub interval: usize,
    pub top_level: bool,
    /// Account filter (full name); empty for the report's default accounts.
    pub account: String,
    pub drill: Option<(String, Vec<Value>)>,
    cache: Option<(String, Value)>,
    /// Performance grouping: 0 by account, 1 by security, 2 total.
    pub perf_group: usize,
    /// Aging by invoice (bill) date instead of due date; with detail.
    pub aging_by_invoice: bool,
    pub aging_detail: bool,
    /// Profit and loss columns (index into the available choices).
    pub pl_columns: usize,
    /// A report to show when the view next draws (from a dashboard block or
    /// shortcut): command name, preset and custom From / To (ISO).
    pub pending: Option<(String, String, String, String)>,
    /// The report shown last (a newly chosen Tax summary starts on Last year).
    pub shown: String,
}

/// Which modules' reports are listed (looked up once per build).
#[derive(Debug, Default, Clone, Copy)]
struct On {
    inv: bool,
    sales: bool,
    tax: bool,
    bills: bool,
    pos: bool,
    ten99: bool,
    classes: bool,
    locations: bool,
    funds: bool,
    projects: bool,
}

impl On {
    fn of(app: &mut App) -> Self {
        Self {
            inv: app.module_on("investments"),
            sales: app.module_on("invoicing"),
            tax: app.module_on("sales-tax"),
            bills: app.module_on("bills"),
            pos: app.module_on("purchase-orders"),
            ten99: app.module_on("1099"),
            classes: app.module_on("classes"),
            locations: app.module_on("locations"),
            funds: app.module_on("funds"),
            projects: app.module_on("projects"),
        }
    }
}

/// Open the Reports view on report `name` (the command name without `report.`)
/// with a preset period (`custom` uses `from` / `to`, ISO dates).
pub(crate) fn open(app: &mut App, name: &str, preset: &str, from: &str, to: &str) {
    app.reports.pending = Some((
        name.to_owned(),
        preset.to_owned(),
        from.to_owned(),
        to.to_owned(),
    ));
    app.navigate(View::Reports);
}

/// Profit and loss column choices: (argument, title).
const PL_COLUMNS: [(&str, &str); 8] = [
    ("total", "Total"),
    ("class", "Class"),
    ("location", "Location"),
    ("fund", "Fund"),
    ("project", "Project"),
    ("month", "Month"),
    ("quarter", "Quarter"),
    ("year", "Year"),
];

fn pl_columns(on: On) -> Vec<(&'static str, &'static str)> {
    PL_COLUMNS
        .iter()
        .copied()
        .filter(|(k, _)| match *k {
            "class" => on.classes,
            "location" => on.locations,
            "fund" => on.funds,
            "project" => on.projects,
            _ => true,
        })
        .collect()
}

/// Reports that show a position as of a date.
fn as_of_report(name: &str) -> bool {
    matches!(
        name,
        "ar_aging"
            | "customer_balances"
            | "open_invoices"
            | "balance_sheet"
            | "ap_aging"
            | "vendor_balances"
            | "unpaid_bills"
            | "balance_sheet_by_fund"
            | "statement_of_financial_position"
    )
}

const GROUPS: [(&str, &str); 3] = [
    ("account", "By account"),
    ("security", "By security"),
    ("total", "Total"),
];

fn listed(on: On) -> Vec<(&'static str, &'static str)> {
    REPORTS
        .iter()
        .copied()
        .filter(|(n, _)| match *n {
            "holdings" | "realized_gains" | "investment_income" | "performance" => on.inv,
            "ar_aging" | "customer_balances" | "open_invoices" | "sales_by_customer"
            | "sales_by_item" => on.sales,
            "sales_tax_liability" => on.tax,
            "ap_aging" | "vendor_balances" | "unpaid_bills" | "expenses_by_vendor"
            | "check_register" => on.bills,
            "open_purchase_orders" => on.pos,
            "1099" => on.ten99,
            "balance_sheet_by_fund"
            | "statement_of_activities"
            | "statement_of_financial_position"
            | "functional_expenses"
            | "fund_activity" => on.funds,
            _ => true,
        })
        .collect()
}

const INTERVALS: [(&str, &str); 3] = [("month", "Month"), ("quarter", "Quarter"), ("year", "Year")];

fn name(app: &App, on: On) -> &'static str {
    listed(on)
        .get(app.reports.report)
        .map_or("spending", |r| r.0)
}

/// The To (or As of) date as ISO: the typed date when it reads, else today.
fn to_or_today(app: &App) -> String {
    match fmt::read_date(&app.reports.to) {
        Ok(Some(d)) => d.to_string(),
        _ => app.today(),
    }
}

fn first_bank(app: &mut App) -> String {
    super::util::array(app.read("account.list", json!({})))
        .iter()
        .find(|a| a.str("subtype") == "bank")
        .map(|a| a.str("full_name").to_owned())
        .unwrap_or_default()
}

/// The report command's arguments from the controls.
fn args(app: &mut App, on: On) -> Value {
    let report = name(app, on);
    let mut a = json!({"today": app.today()});
    match PRESETS.get(app.reports.preset).map(|p| p.0) {
        Some("custom") | None => {
            if let Ok(Some(d)) = fmt::read_date(&app.reports.from) {
                set(&mut a, "from", json!(d));
            }
            if let Ok(Some(d)) = fmt::read_date(&app.reports.to) {
                set(&mut a, "to", json!(d));
            }
        }
        Some(p) => set(&mut a, "preset", json!(p)),
    }
    if matches!(
        report,
        "income_expense" | "net_worth" | "investment_income" | "performance"
    ) {
        let i = INTERVALS.get(app.reports.interval).map_or("month", |x| x.0);
        set(&mut a, "interval", json!(i));
    }
    if report == "performance" {
        let g = GROUPS
            .get(app.reports.perf_group)
            .map_or("account", |x| x.0);
        set(&mut a, "group", json!(g));
    }
    if as_of_report(report) {
        let mut b = json!({"as_of": to_or_today(app)});
        if report == "ap_aging" || report == "ar_aging" {
            let basis = match (app.reports.aging_by_invoice, report) {
                (true, "ap_aging") => "bill_date",
                (true, _) => "invoice_date",
                (false, _) => "due_date",
            };
            set(&mut b, "basis", json!(basis));
            set(&mut b, "detail", json!(app.reports.aging_detail));
        }
        if report == "balance_sheet" {
            set(
                &mut b,
                "depth",
                json!(if app.reports.top_level { 1 } else { 99 }),
            );
        }
        return b;
    }
    match report {
        "open_purchase_orders" => return json!({}),
        "1099" => {
            let src = to_or_today(app);
            let year: i64 = src.get(..4).and_then(|y| y.parse().ok()).unwrap_or(2026);
            return json!({"year": year, "include_below_threshold": true});
        }
        "check_register" => {
            let account = if app.reports.account.is_empty() {
                first_bank(app)
            } else {
                app.reports.account.clone()
            };
            set(&mut a, "account", json!(account));
            return a;
        }
        "holdings" => return json!({"as_of": to_or_today(app)}),
        "profit_loss" => {
            let cols = pl_columns(on);
            let c = cols.get(app.reports.pl_columns).map_or("total", |x| x.0);
            set(&mut a, "columns", json!(c));
            if app.reports.top_level {
                set(&mut a, "depth", json!(1));
            }
            return a;
        }
        _ => {}
    }
    if matches!(report, "spending" | "income_expense" | "net_worth") {
        set(
            &mut a,
            "depth",
            json!(if app.reports.top_level { 1 } else { 99 }),
        );
    }
    if matches!(report, "spending" | "cash_flow" | "payees") && !app.reports.account.is_empty() {
        set(&mut a, "accounts", json!([app.reports.account]));
    }
    a
}

fn data(app: &mut App, on: On) -> Option<Value> {
    let command = format!("report.{}", name(app, on));
    let a = args(app, on);
    let key = format!("{}|{command}|{a}", app.generation);
    if let Some((k, v)) = &app.reports.cache
        && *k == key
    {
        return Some(v.clone());
    }
    let v = app.read(&command, a)?;
    app.reports.cache = Some((key, v.clone()));
    Some(v)
}

const SIDE: i32 = 200;

pub(crate) fn view(app: &mut App, ui: &mut Ui) {
    ui.title("Reports");
    let on = On::of(app);
    if app.reports.pl_columns >= pl_columns(on).len() {
        app.reports.pl_columns = 0;
    }
    if let Some((name, preset, from, to)) = app.reports.pending.take() {
        if let Some(i) = listed(on).iter().position(|r| r.0 == name) {
            app.reports.report = i;
        }
        if let Some(p) = PRESETS.iter().position(|p| p.0 == preset) {
            app.reports.preset = p;
        }
        if preset == "custom" {
            app.reports.from = fmt::date(&from);
            app.reports.to = fmt::date(&to);
        }
    }
    if app.reports.report >= listed(on).len() {
        app.reports.report = 0;
    }
    let now = name(app, on);
    if now != app.reports.shown {
        // chart-presets.md: the Tax summary defaults to Last year.
        if now == "tax_summary"
            && let Some(p) = PRESETS.iter().position(|p| p.0 == "last_year")
        {
            app.reports.preset = p;
        }
        app.reports.shown = now.to_owned();
    }
    ui.side(SIDE, |ui| {
        for (i, (_, title)) in listed(on).iter().enumerate() {
            let selected = app.reports.report == i;
            ui.next().grow();
            if ui.tab(*title, selected).clicked() {
                app.reports.report = i;
            }
        }
    });
    controls(app, ui, on);
    let Some(d) = data(app, on) else {
        return;
    };
    let span = if let Some(y) = d.at("year").as_i64() {
        format!("Year {y}")
    } else if d.at("from").is_null() && d.at("as_of").is_null() {
        String::new()
    } else if d.at("as_of").is_string() {
        format!("As of {}", fmt::date(d.str("as_of")))
    } else {
        format!("{} – {}", fmt::date(d.str("from")), fmt::date(d.str("to")))
    };
    if !span.is_empty() {
        ui.secondary(span);
    }
    let width = app.chart_width(SIDE);
    let report = name(app, on);
    ui.scope(format!("report:{report}"), |ui| match report {
        "spending" => spending(app, ui, width, &d),
        "income_expense" => income_expense(app, ui, width, &d),
        "net_worth" => net_worth(app, ui, width, &d),
        "cash_flow" => cash_flow(ui, &d),
        "holdings" => holdings(ui, &d),
        "realized_gains" => realized_gains(ui, &d),
        "investment_income" => investment_income(ui, &d),
        "performance" => performance(app, ui, width, &d),
        "balance_sheet" => balance_sheet(ui, &d),
        "ar_aging" => aging(ui, &d, "Customer", "customers", "customer_name", true),
        "ap_aging" => aging(ui, &d, "Vendor", "vendors", "vendor_name", false),
        "customer_balances" => simple_table(
            ui,
            &d,
            "customers",
            &[
                ("Customer", "customer_name"),
                ("Open", "open"),
                ("Overdue", "overdue"),
                ("Credits", "credits"),
                ("Balance", "balance"),
            ],
        ),
        "open_invoices" => simple_table(
            ui,
            &d,
            "invoices",
            &[
                ("Number", "number"),
                ("Customer", "customer_name"),
                ("Date", "date"),
                ("Due", "due_date"),
                ("Days overdue", "days_overdue"),
                ("Total", "total"),
                ("Balance", "balance"),
            ],
        ),
        "sales_by_customer" => simple_table(
            ui,
            &d,
            "customers",
            &[
                ("Customer", "customer_name"),
                ("Amount", "amount"),
                ("Percent", "percent"),
            ],
        ),
        "sales_by_item" => simple_table(
            ui,
            &d,
            "items",
            &[
                ("Item", "item"),
                ("Quantity", "quantity"),
                ("Amount", "amount"),
                ("Percent", "percent"),
            ],
        ),
        "vendor_balances" => simple_table(
            ui,
            &d,
            "vendors",
            &[
                ("Vendor", "vendor_name"),
                ("Open", "open"),
                ("Overdue", "overdue"),
                ("Credits", "credits"),
                ("Balance", "balance"),
            ],
        ),
        "unpaid_bills" => simple_table(
            ui,
            &d,
            "bills",
            &[
                ("Vendor", "vendor_name"),
                ("Number", "number"),
                ("Date", "date"),
                ("Due", "due_date"),
                ("Days overdue", "days_overdue"),
                ("Total", "total"),
                ("Balance", "balance"),
            ],
        ),
        "expenses_by_vendor" => simple_table(
            ui,
            &d,
            "vendors",
            &[
                ("Vendor", "vendor_name"),
                ("Amount", "amount"),
                ("Percent", "percent"),
            ],
        ),
        "open_purchase_orders" => simple_table(
            ui,
            &d,
            "purchase_orders",
            &[
                ("Number", "number"),
                ("Vendor", "vendor_name"),
                ("Date", "date"),
                ("Expected", "expected_date"),
                ("Total", "total"),
                ("Open", "open"),
                ("Status", "status"),
            ],
        ),
        "check_register" => simple_table(
            ui,
            &d,
            "checks",
            &[
                ("Number", "number"),
                ("Date", "date"),
                ("Payee", "payee"),
                ("Amount", "amount"),
                ("Void", "voided"),
            ],
        ),
        "1099" => simple_table(
            ui,
            &d,
            "vendors",
            &[
                ("Vendor", "vendor_name"),
                ("Form", "form"),
                ("Box", "box"),
                ("Paid", "total"),
                ("Card (excluded)", "excluded_card"),
                ("Threshold", "threshold"),
                ("File", "file"),
            ],
        ),
        "sales_tax_liability" => simple_table(
            ui,
            &d,
            "rates",
            &[
                ("Rate", "name"),
                ("Taxable sales", "taxable_sales"),
                ("Non-taxable sales", "nontaxable_sales"),
                ("Collected", "collected"),
                ("Paid", "paid"),
                ("Balance", "balance"),
            ],
        ),
        "profit_loss" => profit_loss(ui, &d),
        "balance_sheet_by_fund" => balance_by_fund(ui, &d),
        "statement_of_activities" => activities(ui, &d),
        "statement_of_financial_position" => financial_position(ui, &d),
        "functional_expenses" => functional(ui, &d),
        "tax_summary" => tax_summary(ui, &d),
        "fund_activity" => simple_table(
            ui,
            &d,
            "funds",
            &[
                ("Fund", "name"),
                ("Beginning", "beginning"),
                ("Revenue", "revenue"),
                ("Expenses", "expenses_tagged"),
                ("Released in", "released_in"),
                ("Released out", "released_out"),
                ("Transfers in", "transfers_in"),
                ("Transfers out", "transfers_out"),
                ("Other", "other_changes"),
                ("Ending", "ending"),
                ("Overspent by", "overspent_by"),
            ],
        ),
        _ => payees(app, ui, &d),
    });
    drill_dialog(app, ui);
}

fn names(items: &[(&str, &str)]) -> Vec<String> {
    items.iter().map(|p| p.1.to_owned()).collect()
}

fn controls(app: &mut App, ui: &mut Ui, on: On) {
    let report = name(app, on);
    ui.wrap(|ui| {
        ui.width(170);
        ui.combo("Period", &mut app.reports.preset, &names(&PRESETS));
        if PRESETS.get(app.reports.preset).map(|p| p.0) == Some("custom") {
            super::register::date_field(ui, "From date", &mut app.reports.from);
            super::register::date_field(ui, "To date", &mut app.reports.to);
        }
        if matches!(
            report,
            "income_expense" | "net_worth" | "investment_income" | "performance"
        ) {
            ui.width(120);
            ui.combo("Interval", &mut app.reports.interval, &names(&INTERVALS));
        }
        if report == "performance" {
            ui.width(140);
            ui.combo("Group", &mut app.reports.perf_group, &names(&GROUPS));
        }
        if as_of_report(report) {
            super::register::date_field(ui, "As of date", &mut app.reports.to);
        }
        if report == "ap_aging" {
            ui.checkbox(&mut app.reports.aging_by_invoice, "Age by bill date");
            ui.checkbox(&mut app.reports.aging_detail, "Show bills");
        }
        if report == "ar_aging" {
            ui.checkbox(&mut app.reports.aging_by_invoice, "Age by invoice date");
            ui.checkbox(&mut app.reports.aging_detail, "Show invoices");
        }
        if report == "profit_loss" {
            ui.width(120);
            ui.combo(
                "Columns",
                &mut app.reports.pl_columns,
                &names(&pl_columns(on)),
            );
        }
        if matches!(
            report,
            "spending" | "income_expense" | "net_worth" | "balance_sheet" | "profit_loss"
        ) {
            let mut d = usize::from(app.reports.top_level);
            ui.width(150);
            ui.combo(
                "Depth",
                &mut d,
                &["All levels".to_owned(), "Top-level only".to_owned()],
            );
            app.reports.top_level = d == 1;
        }
        if matches!(
            report,
            "spending" | "cash_flow" | "payees" | "check_register"
        ) {
            let mut options = vec!["Default accounts".to_owned()];
            options.extend(
                super::util::array(app.read("account.list", json!({})))
                    .iter()
                    .filter(|a| matches!(a.str("type"), "asset" | "liability"))
                    .map(|a| a.str("full_name").to_owned()),
            );
            let mut idx = if app.reports.account.is_empty() {
                0
            } else {
                options
                    .iter()
                    .position(|o| *o == app.reports.account)
                    .unwrap_or(0)
            };
            ui.width(200);
            if ui.combo("Accounts", &mut idx, &options).changed {
                app.reports.account = if idx == 0 {
                    String::new()
                } else {
                    options.get(idx).cloned().unwrap_or_default()
                };
            }
        }
        if ui.button("Export CSV…").clicked() {
            let a = json!({"report": report, "args": args(app, on), "format": "csv"});
            if let Some(e) = app.read("report.export", a) {
                app.emit(AppEvent::SaveFile {
                    file_name: e.str("file_name").to_owned(),
                    content: e.str("content").to_owned(),
                });
            }
        }
        if report == "tax_summary" && ui.button("Export PDF…").clicked() {
            let a = json!({"report": report, "args": args(app, on), "format": "pdf"});
            if let Some(e) = app.read("report.export", a) {
                use base64::Engine;
                if let Ok(data) =
                    base64::engine::general_purpose::STANDARD.decode(e.str("content_base64"))
                {
                    app.emit(AppEvent::SaveBytes {
                        file_name: e.str("file_name").to_owned(),
                        data,
                    });
                }
            }
        }
    });
}

// ----- tables ----------------------------------------------------------------------

fn dec(s: &str) -> Decimal {
    s.parse().unwrap_or_default()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| x.as_str().unwrap_or("").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn num(ui: &mut Ui, text: &str, strong: bool) {
    if strong {
        ui.bold();
    }
    ui.tip(fmt::group(text));
    ui.amount(fmt::group(text));
}

/// One table row: a name and amounts; returns whether the row was clicked.
fn table_row(
    ui: &mut Ui,
    name: &str,
    indent: i32,
    amounts: &[String],
    strong: bool,
    clickable: bool,
) -> bool {
    let key = if clickable {
        format!("Show transactions for {name}")
    } else {
        format!("{name}#row")
    };
    let clicked = ui
        .table_row(&key, false, clickable, |ui| {
            if strong {
                ui.bold();
            }
            ui.tip(name);
            ui.label(name);
            for a in amounts {
                num(ui, a, strong);
            }
        })
        .clicked();
    ui.indent_last(indent);
    clicked
}

fn heading_row(ui: &mut Ui, text: &str) {
    ui.table_row(&format!("{text}#heading"), false, false, |ui| {
        ui.bold();
        ui.label(text);
    });
}

fn spending(app: &mut App, ui: &mut Ui, width: i32, d: &Value) {
    let rows = d.list("rows").to_vec();
    let mut items: Vec<(String, Decimal)> = rows
        .iter()
        .take(10)
        .map(|r| (r.str("full_name").to_owned(), dec(r.str("amount"))))
        .collect();
    let rest: Decimal = rows.iter().skip(10).fold(Decimal::ZERO, |t, r| {
        t.checked_add(dec(r.str("amount"))).unwrap_or(t)
    });
    if !rest.is_zero() {
        items.push(("Other".into(), rest));
    }
    let desc = format!(
        "Spending chart: {}",
        items
            .iter()
            .map(|(n, v)| format!("{n} {}", fmt::group(&v.to_string())))
            .collect::<Vec<_>>()
            .join(", ")
    );
    // A category's bar opens its register for the report's period (dashboards.md
    // revision 5); "Other" stands for the rest, already listed here.
    let opens = charts::Opens {
        pieces: true,
        more: false,
        period: super::dashboard::period_caption(d.str("from"), d.str("to")),
    };
    if let Some(ChartHit::Piece(i)) =
        ui.card("", |ui| charts::hbars(ui, width, &items, &desc, &opens))
        && let Some((cat, _)) = items.get(i).filter(|x| x.0 != "Other")
    {
        super::dashboard::open_register(app, cat, d.str("from"), d.str("to"));
    }
    let cols = vec![
        Column {
            title: "Category".into(),
            weight: 3,
            ..Column::default()
        },
        Column {
            width: 120,
            ..col_num("Amount")
        },
        Column {
            width: 80,
            ..col_num("Percent")
        },
    ];
    let mut drill = None;
    ui.table("", cols, |ui| {
        for r in &rows {
            let name = r.str("full_name");
            let pct = r.str("percent");
            if table_row(
                ui,
                name,
                0,
                &[r.str("amount").to_owned(), format!("{pct}%")],
                false,
                true,
            ) {
                drill = Some((r.str("id").to_owned(), name.to_owned()));
            }
        }
        table_row(ui, "Total", 0, &[d.str("total").to_owned()], true, false);
    });
    if let Some((id, name)) = drill {
        drill_account(app, &id, &name, d);
    }
    excluded(ui, d);
}

fn income_expense(app: &mut App, ui: &mut Ui, width: i32, d: &Value) {
    let labels: Vec<String> = d
        .list("periods")
        .iter()
        .map(|p| p.str("label").to_owned())
        .collect();
    let inc: Vec<Decimal> = strs(d.at("income").at("totals"))
        .iter()
        .map(|s| dec(s))
        .collect();
    let exp: Vec<Decimal> = strs(d.at("expenses").at("totals"))
        .iter()
        .map(|s| dec(s))
        .collect();
    let net: Vec<Decimal> = strs(d.at("net").at("amounts"))
        .iter()
        .map(|s| dec(s))
        .collect();
    let desc = format!(
        "Income and expenses chart by period: {}",
        labels
            .iter()
            .enumerate()
            .map(|(i, l)| format!(
                "{l} income {} expenses {} net {}",
                fmt::group(&inc.get(i).copied().unwrap_or_default().to_string()),
                fmt::group(&exp.get(i).copied().unwrap_or_default().to_string()),
                fmt::group(&net.get(i).copied().unwrap_or_default().to_string())
            ))
            .collect::<Vec<_>>()
            .join("; ")
    );
    // A month's bar opens this report for that month (dashboards.md revision 5).
    let opens = charts::Opens {
        pieces: true,
        ..charts::Opens::default()
    };
    let hit = ui.card("", |ui| {
        charts::paired(
            ui,
            width,
            &labels,
            [("Income", &inc), ("Expenses", &exp), ("Net", &net)],
            &desc,
            &opens,
        )
    });
    if let Some(ChartHit::Piece(p)) = hit
        && let Some(period) = d.list("periods").get(p / 2)
    {
        let (from, to) = (period.str("from").to_owned(), period.str("to").to_owned());
        open(app, "income_expense", "custom", &from, &to);
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut ids: Vec<Option<String>> = Vec::new();
    for (key, title) in [("income", "Income"), ("expenses", "Expenses")] {
        rows.push(Row {
            name: title.into(),
            indent: 0,
            cells: Vec::new(),
            total: None,
            kind: Kind::Heading,
        });
        ids.push(None);
        let section = d.at(key);
        for r in section.list("rows") {
            rows.push(Row {
                name: r.str("full_name").to_owned(),
                indent: r.at("depth").as_u64().unwrap_or(0),
                cells: strs(r.at("amounts")),
                total: Some(r.str("total").to_owned()),
                kind: Kind::Account,
            });
            ids.push(Some(r.str("id").to_owned()));
        }
        rows.push(Row {
            name: format!("Total {}", title.to_lowercase()),
            indent: 0,
            cells: strs(section.at("totals")),
            total: Some(section.str("total").to_owned()),
            kind: Kind::Total,
        });
        ids.push(None);
    }
    rows.push(Row {
        name: "Net".into(),
        indent: 0,
        cells: strs(d.at("net").at("amounts")),
        total: Some(d.at("net").str("total").to_owned()),
        kind: Kind::Total,
    });
    ids.push(None);
    if let Some(i) = period_table::show(
        ui,
        "income-expense",
        "Category",
        &labels,
        &rows,
        Some("Total"),
    ) && let (Some(Some(id)), Some(row)) = (ids.get(i), rows.get(i))
    {
        let name = row.name.clone();
        drill_account(app, id, &name, d);
    }
    excluded(ui, d);
}

fn net_worth(app: &mut App, ui: &mut Ui, width: i32, d: &Value) {
    let labels: Vec<String> = d
        .list("periods")
        .iter()
        .map(|p| p.str("label").to_owned())
        .collect();
    let nw: Vec<Decimal> = strs(d.at("net_worth")).iter().map(|s| dec(s)).collect();
    let assets: Vec<Decimal> = strs(d.at("assets").at("totals"))
        .iter()
        .map(|s| dec(s))
        .collect();
    let liabilities: Vec<Decimal> = strs(d.at("liabilities").at("totals"))
        .iter()
        .map(|s| dec(s))
        .collect();
    let desc = format!(
        "Net worth chart: {}",
        labels
            .iter()
            .zip(&nw)
            .map(|(l, v)| format!("{l} {}", fmt::group(&v.to_string())))
            .collect::<Vec<_>>()
            .join(", ")
    );
    // A month opens this report as of its end (dashboards.md revision 5).
    let opens = charts::Opens {
        pieces: true,
        ..charts::Opens::default()
    };
    let hit = ui.card("", |ui| {
        charts::lines(
            ui,
            width,
            &labels,
            &[
                ("Net worth", nw),
                ("Assets", assets),
                ("Liabilities", liabilities),
            ],
            &desc,
            &opens,
        )
    });
    if let Some(ChartHit::Piece(i)) = hit
        && let Some(period) = d.list("periods").get(i)
    {
        let (from, to) = (d.str("from").to_owned(), period.str("to").to_owned());
        open(app, "net_worth", "custom", &from, &to);
    }
    let mut rows: Vec<Row> = Vec::new();
    for (key, title) in [("assets", "Assets"), ("liabilities", "Liabilities")] {
        rows.push(Row {
            name: title.into(),
            indent: 0,
            cells: Vec::new(),
            total: None,
            kind: Kind::Heading,
        });
        for r in d.at(key).list("rows") {
            rows.push(Row {
                name: r.str("full_name").to_owned(),
                indent: r.at("depth").as_u64().unwrap_or(0),
                cells: strs(r.at("amounts")),
                total: None,
                kind: Kind::Plain,
            });
        }
        rows.push(Row {
            name: format!("Total {}", title.to_lowercase()),
            indent: 0,
            cells: strs(d.at(key).at("totals")),
            total: None,
            kind: Kind::Total,
        });
    }
    rows.push(Row {
        name: "Net worth".into(),
        indent: 0,
        cells: strs(d.at("net_worth")),
        total: None,
        kind: Kind::Total,
    });
    // The last date stays visible on the right while earlier periods scroll.
    let (last_year, last_title) = labels
        .last()
        .map(|l| crate::base::period::short_label(l))
        .unwrap_or_default();
    let n = labels.len();
    for r in &mut rows {
        if r.cells.len() == n && n > 0 {
            r.total = r.cells.pop();
        }
    }
    let earlier = labels.get(..n.saturating_sub(1)).unwrap_or(&[]).to_vec();
    period_table::show_with_year(
        ui,
        "net-worth",
        "Account",
        &earlier,
        &rows,
        Some(&last_title),
        last_year.as_deref(),
    );
    ui.label(format!("Change {}", fmt::group(d.str("change"))));
    excluded(ui, d);
}

fn cash_flow(ui: &mut Ui, d: &Value) {
    let cols = vec![
        Column {
            title: "Item".into(),
            weight: 3,
            ..Column::default()
        },
        Column {
            width: 130,
            ..col_num("Amount")
        },
    ];
    ui.table("", cols, |ui| {
        table_row(
            ui,
            "Opening",
            0,
            &[d.str("opening").to_owned()],
            true,
            false,
        );
        for (key, title) in [("inflows", "Inflows"), ("outflows", "Outflows")] {
            heading_row(ui, title);
            for r in d.at(key).list("rows") {
                table_row(
                    ui,
                    r.str("name"),
                    1,
                    &[r.str("amount").to_owned()],
                    false,
                    false,
                );
            }
            table_row(
                ui,
                &format!("Total {}", title.to_lowercase()),
                0,
                &[d.at(key).str("total").to_owned()],
                true,
                false,
            );
        }
        table_row(
            ui,
            "Internal transfers",
            0,
            &[d.str("internal").to_owned()],
            false,
            false,
        );
        table_row(
            ui,
            "Closing",
            0,
            &[d.str("closing").to_owned()],
            true,
            false,
        );
    });
    ui.secondary(format!("Check: difference {}", d.str("difference")));
    excluded(ui, d);
}

fn payees(app: &mut App, ui: &mut Ui, d: &Value) {
    let cols = vec![
        Column {
            title: "Payee".into(),
            weight: 3,
            ..Column::default()
        },
        Column {
            width: 110,
            ..col_num("Transactions")
        },
        Column {
            width: 120,
            ..col_num("Spent")
        },
        Column {
            width: 120,
            ..col_num("Received")
        },
    ];
    let mut drill = None;
    ui.table("", cols, |ui| {
        for r in d.list("rows") {
            let name = r.str("payee");
            let amounts = [
                r.at("count").to_string(),
                r.str("spent").to_owned(),
                r.str("received").to_owned(),
            ];
            if table_row(ui, name, 0, &amounts, false, true) {
                drill = Some(name.to_owned());
            }
        }
    });
    if let Some(name) = drill {
        drill_payee(app, &name, d);
    }
}

fn excluded(ui: &mut Ui, d: &Value) {
    let ex = d.list("excluded");
    if !ex.is_empty() {
        let names: Vec<&str> = ex.iter().map(|e| e.str("full_name")).collect();
        ui.secondary(format!(
            "Not included (not in the base currency): {}",
            names.join(", ")
        ));
    }
}

// ----- drill-down ------------------------------------------------------------------

fn drill_account(app: &mut App, id: &str, name: &str, d: &Value) {
    let Some(list) = app.read(
        "txn.list",
        json!({"account": id, "from": d.str("from"), "to": d.str("to")}),
    ) else {
        return;
    };
    let prefix = format!("{name}:");
    let rows = list
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| {
            let mine: Decimal = t
                .list("splits")
                .iter()
                .filter(|s| s.str("account") == id || s.str("account_name").starts_with(&prefix))
                .fold(Decimal::ZERO, |acc, s| acc.checked_add(dec(s.str("value"))).unwrap_or(acc));
            json!({"date": t.str("date"), "account": name, "payee": t.str("payee"), "memo": t.str("memo"), "amount": mine.to_string()})
        })
        .collect();
    app.reports.drill = Some((name.to_owned(), rows));
}

fn drill_payee(app: &mut App, payee: &str, d: &Value) {
    let Some(list) = app.read(
        "txn.list",
        json!({"from": d.str("from"), "to": d.str("to")}),
    ) else {
        return;
    };
    let wanted = payee.trim().to_lowercase();
    let accounts = super::util::array(app.read("account.list", json!({})));
    let is_money = |id: &str| {
        accounts
            .iter()
            .any(|a| a.str("id") == id && matches!(a.str("type"), "asset" | "liability"))
    };
    let rows = list
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|t| {
            let p = t.str("payee").trim().to_lowercase();
            p == wanted || (wanted == "(no payee)" && p.is_empty())
        })
        .map(|t| {
            let money = t.list("splits").iter().find(|s| is_money(s.str("account")));
            json!({
                "date": t.str("date"),
                "account": money.map_or("", |s| s.str("account_name")),
                "payee": t.str("payee"),
                "memo": t.str("memo"),
                "amount": money.map_or("", |s| s.str("value")),
            })
        })
        .collect();
    app.reports.drill = Some((payee.to_owned(), rows));
}

fn drill_dialog(app: &mut App, ui: &mut Ui) {
    let Some((title, rows)) = app.reports.drill.clone() else {
        return;
    };
    let mut close = ui.dialog_closed("report-drill");
    ui.dialog(
        "report-drill",
        &format!("Transactions: {title}"),
        720,
        |ui| {
            let cols = vec![
                Column {
                    width: 100,
                    ..col("Date")
                },
                Column {
                    weight: 2,
                    ..col("Account")
                },
                Column {
                    weight: 2,
                    ..col("Payee")
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
            ui.table("", cols, |ui| {
                ui.visible_rows(10);
                for (i, r) in rows.iter().enumerate() {
                    ui.table_row(&format!("drill:{i}"), false, false, |ui| {
                        ui.label(fmt::date(r.str("date")));
                        ui.tip(r.str("account"));
                        ui.label(r.str("account"));
                        ui.tip(r.str("payee"));
                        ui.label(r.str("payee"));
                        ui.tip(r.str("memo"));
                        ui.label(r.str("memo"));
                        num(ui, r.str("amount"), false);
                    });
                }
            });
            ui.actions(|ui| {
                if ui.primary("Close").clicked() {
                    close = true;
                }
            });
        },
    );
    if close {
        app.reports.drill = None;
    }
}

// ----- investment reports ----------------------------------------------------------

/// A grid of text and number cells; numbers right-aligned in the number font.
enum Cell {
    Text(String),
    Bold(String),
    Num(String),
    Tip(String, String),
    Empty,
}

fn grid(ui: &mut Ui, title: &str, heads: &[&str], numeric_from: usize, rows: Vec<Vec<Cell>>) {
    let cols: Vec<Column> = heads
        .iter()
        .enumerate()
        .map(|(i, h)| {
            if i >= numeric_from {
                col_num(h)
            } else {
                Column {
                    weight: 2,
                    ..col(h)
                }
            }
        })
        .collect();
    ui.table(title, cols, |ui| {
        for (i, r) in rows.into_iter().enumerate() {
            ui.table_row(&format!("r{i}"), false, false, |ui| {
                for c in r {
                    match c {
                        Cell::Text(t) => {
                            ui.tip(t.clone());
                            ui.label(t);
                        }
                        Cell::Bold(t) => {
                            ui.bold();
                            ui.label(t);
                        }
                        Cell::Num(t) => num(ui, &t, false),
                        Cell::Tip(t, tip) => {
                            ui.tip(tip);
                            ui.label(t);
                        }
                        Cell::Empty => ui.label(""),
                    }
                }
            });
        }
    });
}

fn holdings(ui: &mut Ui, d: &Value) {
    for acc in d.list("accounts") {
        let mut rows = Vec::new();
        for h in acc.list("holdings") {
            let mut r = vec![
                Cell::Text(format!("{}  {}", h.str("symbol"), h.str("name"))),
                Cell::Num(h.str("quantity").to_owned()),
            ];
            if h.at("price").is_null() {
                r.push(Cell::Text("no price".into()));
                r.push(Cell::Empty);
            } else {
                r.push(Cell::Num(h.str("price").to_owned()));
                r.push(Cell::Num(h.str("market_value").to_owned()));
            }
            r.push(Cell::Num(h.str("basis").to_owned()));
            r.push(Cell::Num(h.str("gain").to_owned()));
            r.push(Cell::Text(h.str("weight").to_owned()));
            rows.push(r);
        }
        rows.push(vec![
            Cell::Text("Cash".into()),
            Cell::Empty,
            Cell::Empty,
            Cell::Num(acc.str("cash").to_owned()),
        ]);
        rows.push(vec![
            Cell::Bold("Total".into()),
            Cell::Empty,
            Cell::Empty,
            Cell::Num(acc.str("total").to_owned()),
            Cell::Num(acc.str("basis").to_owned()),
        ]);
        ui.scope(acc.str("account"), |ui| {
            grid(
                ui,
                acc.str("full_name"),
                &[
                    "Security",
                    "Shares",
                    "Price",
                    "Market value",
                    "Cost basis",
                    "Gain",
                    "Weight",
                ],
                1,
                rows,
            );
        });
        if acc.at("unpriced").as_bool() == Some(true) {
            ui.secondary("Holdings without a price are left out of the totals.");
        }
    }
}

fn realized_gains(ui: &mut Ui, d: &Value) {
    for sec in d.list("sections") {
        let mut rows = Vec::new();
        for r in sec.list("rows") {
            let tip = wash_tip(r);
            rows.push(vec![
                Cell::Text(r.str("symbol").to_owned()),
                Cell::Num(r.str("quantity").to_owned()),
                Cell::Text(r.str("acquired").to_owned()),
                Cell::Text(r.str("sold").to_owned()),
                Cell::Num(r.str("proceeds").to_owned()),
                Cell::Num(r.str("basis").to_owned()),
                Cell::Num(r.str("gain").to_owned()),
                Cell::Tip(r.str("adjustment_code").to_owned(), tip.clone()),
                Cell::Tip(fmt::group(r.str("adjustment")), tip),
            ]);
        }
        ui.scope(sec.str("term"), |ui| {
            grid(
                ui,
                sec.str("title"),
                &[
                    "Security",
                    "Shares",
                    "Acquired",
                    "Sold",
                    "Proceeds",
                    "Cost basis",
                    "Gain",
                    "Code",
                    "Adjustment",
                ],
                4,
                rows,
            );
        });
        ui.label(format!(
            "{} total: proceeds {}, cost basis {}, gain {}",
            sec.str("title"),
            fmt::group(sec.str("proceeds")),
            fmt::group(sec.str("basis")),
            fmt::group(sec.str("gain"))
        ));
    }
    let t = d.at("total");
    ui.bold();
    ui.label(format!("Total gain {}", fmt::group(t.str("gain"))));
    if !t.str("disallowed").is_empty() {
        ui.label(format!(
            "Disallowed (wash sales) {}",
            fmt::group(t.str("disallowed"))
        ));
    }
    ui.secondary(d.str("note"));
}

/// The tooltip of a wash-sale row: the replacement purchases and the note.
fn wash_tip(r: &Value) -> String {
    if r.at("wash_sale").as_bool() != Some(true) {
        return String::new();
    }
    let reps: Vec<String> = r
        .list("replacements")
        .iter()
        .map(|x| {
            format!(
                "{} {} shares of {} ({})",
                fmt::date(x.str("date")),
                x.str("quantity"),
                x.str("symbol"),
                x.str("account")
            )
        })
        .collect();
    format!(
        "Replacement purchase: {}\n{}",
        reps.join("; "),
        r.str("note")
    )
}

/// A return measure: the percent with "not annualised" as a suffix, or "n/a"
/// with the reason as a tooltip.
fn measure(m: &Value) -> Cell {
    if m.at("percent").as_str() == Some("n/a") || m.at("percent").is_null() {
        return Cell::Tip("n/a".into(), m.str("reason").to_owned());
    }
    if m.at("annualised").as_bool() == Some(false) {
        Cell::Text(format!("{}% not annualised", m.str("percent")))
    } else {
        Cell::Text(format!("{}%", m.str("percent")))
    }
}

fn performance(app: &mut App, ui: &mut Ui, width: i32, d: &Value) {
    let series = d.list("series");
    let labels: Vec<String> = series
        .first()
        .map(|s| {
            s.list("points")
                .iter()
                .map(|p| fmt::date(p.str("date")))
                .collect()
        })
        .unwrap_or_default();
    let lines: Vec<(&str, Vec<Decimal>)> = series
        .iter()
        .map(|s| {
            (
                s.str("name"),
                s.list("points")
                    .iter()
                    .map(|p| dec(p.str("value")))
                    .collect(),
            )
        })
        .collect();
    let desc = format!(
        "Value chart: {}",
        series
            .iter()
            .map(|s| format!(
                "{} ending at {}",
                s.str("name"),
                fmt::group(s.list("points").last().map_or("0.00", |p| p.str("value")))
            ))
            .collect::<Vec<_>>()
            .join("; ")
    );
    if !labels.is_empty() {
        // A point opens Holdings as of its date (dashboards.md revision 5).
        let opens = charts::Opens {
            pieces: true,
            ..charts::Opens::default()
        };
        if let Some(ChartHit::Piece(i)) = ui.card("", |ui| {
            charts::lines(ui, width, &labels, &lines, &desc, &opens)
        }) && let Some(date) = series
            .first()
            .and_then(|s| s.list("points").get(i))
            .map(|p| p.str("date").to_owned())
        {
            open(app, "holdings", "custom", "", &date);
        }
    }
    let mut rows = Vec::new();
    for r in d.list("rows") {
        let mut name = r.str("name").to_owned();
        if r.at("valued_at_cost").as_bool() == Some(true) {
            name.push_str(" (valued at cost)");
        }
        let mut row = vec![Cell::Text(name)];
        for k in [
            "start_value",
            "net_flows",
            "income",
            "fees",
            "end_value",
            "gain",
        ] {
            row.push(Cell::Num(r.str(k).to_owned()));
        }
        row.push(measure(r.at("mwr")));
        row.push(measure(r.at("twr")));
        rows.push(row);
    }
    grid(
        ui,
        "",
        &[
            "Name",
            "Start value",
            "Net flows",
            "Income",
            "Fees",
            "End value",
            "Gain",
            "Money-weighted",
            "Time-weighted",
        ],
        1,
        rows,
    );
    ui.secondary(d.str("explanation"));
    ui.secondary(d.str("note"));
}

fn investment_income(ui: &mut Ui, d: &Value) {
    let labels: Vec<String> = d
        .list("periods")
        .iter()
        .map(|p| p.str("label").to_owned())
        .collect();
    let rows: Vec<Row> = d
        .list("rows")
        .iter()
        .map(|r| Row {
            name: r.str("label").to_owned(),
            indent: 0,
            cells: strs(r.at("amounts")),
            total: Some(r.str("total").to_owned()),
            kind: Kind::Plain,
        })
        .collect();
    period_table::show(ui, "inv-income", "Item", &labels, &rows, Some("Total"));
}

// ----- sales reports and the balance sheet --------------------------------------------

fn numeric(s: &str) -> bool {
    !s.is_empty() && s.replace(',', "").parse::<Decimal>().is_ok()
}

/// A plain table of `rows_key` rows; columns after the first that hold numbers are
/// right-aligned. A `total` value, when present, closes the table.
fn simple_table(ui: &mut Ui, d: &Value, rows_key: &str, cols: &[(&str, &str)]) {
    let list = d.list(rows_key);
    let numeric_col: Vec<bool> = cols
        .iter()
        .enumerate()
        .map(|(i, (_, k))| {
            i > 0
                && !k.ends_with("date")
                && *k != "date"
                && list.iter().any(|r| match r.at(k) {
                    Value::String(s) => numeric(s),
                    Value::Number(_) => true,
                    _ => false,
                })
        })
        .collect();
    let columns: Vec<Column> = cols
        .iter()
        .zip(&numeric_col)
        .map(|((t, _), n)| {
            if *n {
                col_num(t)
            } else {
                Column {
                    weight: 2,
                    ..col(t)
                }
            }
        })
        .collect();
    ui.table("", columns, |ui| {
        for (n, r) in list.iter().enumerate() {
            ui.table_row(&format!("{rows_key}:{n}"), false, false, |ui| {
                for (i, (_, k)) in cols.iter().enumerate() {
                    let v = match r.at(k) {
                        Value::String(s) if *k == "date" || k.ends_with("_date") => fmt::date(s),
                        Value::String(s) => s.clone(),
                        Value::Null => String::new(),
                        other => other.to_string(),
                    };
                    if i > 0 && numeric(&v) {
                        num(ui, &v, false);
                    } else {
                        ui.tip(v.clone());
                        ui.label(v);
                    }
                }
            });
        }
        if d.at("total").is_string() {
            ui.table_row(&format!("{rows_key}:total"), false, false, |ui| {
                ui.bold();
                ui.label("Total");
                for _ in 2..cols.len() {
                    ui.label("");
                }
                num(ui, d.str("total"), true);
            });
        }
    });
}

fn aging(ui: &mut Ui, d: &Value, who: &str, key: &str, name_key: &str, with_detail: bool) {
    let buckets: Vec<String> = strs(d.at("columns"));
    let mut cols = vec![Column {
        weight: 3,
        ..col(who)
    }];
    for b in &buckets {
        cols.push(col_num(b));
    }
    cols.push(col_num("Credits"));
    cols.push(col_num("Total"));
    ui.table("", cols, |ui| {
        for (n, r) in d.list(key).iter().enumerate() {
            ui.table_row(&format!("{key}:{n}"), false, false, |ui| {
                ui.tip(r.str(name_key));
                ui.label(r.str(name_key));
                for b in strs(r.at("buckets")) {
                    num(ui, &b, false);
                }
                num(ui, r.str("credits"), false);
                num(ui, r.str("total"), false);
            });
            let detail_key = if with_detail { "invoices" } else { "bills" };
            for (m, i) in r.list(detail_key).iter().enumerate() {
                ui.table_row(&format!("{key}:{n}:{m}"), false, false, |ui| {
                    ui.label(format!(
                        "{} due {}",
                        i.str("number"),
                        fmt::date(i.str("due_date"))
                    ));
                    for c in &buckets {
                        if i.str("bucket") == c {
                            num(ui, i.str("balance"), false);
                        } else {
                            ui.label("");
                        }
                    }
                    ui.label("");
                    ui.label("");
                });
                ui.indent_last(1);
            }
        }
        let t = d.at("totals");
        ui.table_row(&format!("{key}:total"), false, false, |ui| {
            ui.bold();
            ui.label("Total");
            for b in strs(t.at("buckets")) {
                num(ui, &b, true);
            }
            num(ui, t.str("credits"), true);
            num(ui, t.str("total"), true);
        });
    });
    if with_detail {
        ui.label(format!(
            "Accounts receivable balance {}",
            fmt::group(d.str("ar_balance"))
        ));
        if d.at("balanced").as_bool() == Some(false) {
            ui.error_text(format!(
                "The aging differs from the accounts receivable balance by {} (entries made outside invoices and payments).",
                fmt::group(d.str("difference"))
            ));
        }
    } else {
        ui.label(format!(
            "Accounts payable balance {}",
            fmt::group(d.str("ap_balance"))
        ));
        if d.at("balanced").as_bool() == Some(false) {
            ui.error_text(format!(
                "The aging differs from the accounts payable balance by {} (entries made outside bills and payments).",
                fmt::group(d.str("difference"))
            ));
        }
    }
}

/// A two-column statement: name and amount, headings bold, entries indented.
fn statement(ui: &mut Ui, lines: &[(String, String, i32, bool)]) {
    let cols = vec![
        Column {
            weight: 3,
            ..col("")
        },
        Column {
            width: 150,
            ..col_num("")
        },
    ];
    ui.table("", cols, |ui| {
        for (n, (name, v, indent, strong)) in lines.iter().enumerate() {
            ui.table_row(&format!("line:{n}"), false, false, |ui| {
                if *strong {
                    ui.bold();
                }
                ui.tip(name.clone());
                ui.label(name.clone());
                if v.is_empty() {
                    ui.label("");
                } else {
                    num(ui, v, *strong);
                }
            });
            ui.indent_last(*indent);
        }
    });
}

fn balance_sheet(ui: &mut Ui, d: &Value) {
    let mut lines = Vec::new();
    for (title, key, total, total_label) in [
        ("Assets", "assets", "total_assets", "Total assets"),
        (
            "Liabilities",
            "liabilities",
            "total_liabilities",
            "Total liabilities",
        ),
        ("Equity", "equity", "total_equity", "Total equity"),
    ] {
        lines.push((title.to_owned(), String::new(), 0, true));
        for r in d.list(key) {
            let level = i32::try_from(r.at("level").as_u64().unwrap_or(0)).unwrap_or(0) + 1;
            lines.push((
                r.str("name").to_owned(),
                r.str("balance").to_owned(),
                level,
                false,
            ));
        }
        lines.push((total_label.to_owned(), d.str(total).to_owned(), 0, true));
    }
    lines.push((
        "Total liabilities and equity".to_owned(),
        d.str("total_liabilities_and_equity").to_owned(),
        0,
        true,
    ));
    statement(ui, &lines);
    if d.at("balanced").as_bool() == Some(false) {
        ui.error_text(format!(
            "Assets differ from liabilities and equity by {}.",
            fmt::group(d.str("difference"))
        ));
    }
}

// ----- dimensions and nonprofit reports ---------------------------------------------------

fn labels_of(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| match x {
                    Value::String(s) => s.clone(),
                    other => other.str("label").to_owned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn plain_row(name: &str, cells: Vec<String>, total: Option<String>, kind: Kind) -> Row {
    Row {
        name: name.to_owned(),
        indent: u64::from(kind == Kind::Plain),
        cells,
        total,
        kind,
    }
}

fn heading(name: &str) -> Row {
    plain_row(name, Vec::new(), None, Kind::Heading)
}

fn profit_loss(ui: &mut Ui, d: &Value) {
    let labels = labels_of(d.at("columns"));
    let mut rows = Vec::new();
    for sec in d.list("sections") {
        let title = sec.str("name");
        rows.push(heading(title));
        for r in sec.list("rows") {
            rows.push(plain_row(
                r.str("name"),
                strs(r.at("amounts")),
                Some(r.str("total").to_owned()),
                Kind::Plain,
            ));
        }
        rows.push(plain_row(
            &format!("Total {}", title.to_lowercase()),
            strs(sec.at("totals")),
            Some(sec.str("total").to_owned()),
            Kind::Total,
        ));
    }
    let net = d.at("net_income");
    rows.push(plain_row(
        "Net income",
        strs(net.at("amounts")),
        Some(net.str("total").to_owned()),
        Kind::Total,
    ));
    period_table::show(ui, "profit-loss", "Account", &labels, &rows, Some("Total"));
}

fn three(name: &str, v: &Value, kind: Kind) -> Row {
    let mut cells = strs(v);
    let total = cells.pop();
    plain_row(name, cells, total, kind)
}

fn activities(ui: &mut Ui, d: &Value) {
    let mut labels = labels_of(d.at("columns"));
    let total_title = labels.pop().unwrap_or_else(|| "Total".into());
    let mut rows = vec![heading("Revenue")];
    for r in d.list("revenue") {
        rows.push(three(r.str("name"), r.at("amounts"), Kind::Plain));
    }
    rows.push(three("Total revenue", d.at("total_revenue"), Kind::Total));
    rows.push(three(
        "Net assets released from restrictions",
        d.at("released"),
        Kind::Total,
    ));
    rows.push(heading("Expenses"));
    for r in d.list("expenses") {
        rows.push(three(r.str("name"), r.at("amounts"), Kind::Plain));
    }
    rows.push(three("Total expenses", d.at("total_expenses"), Kind::Total));
    if strs(d.at("other_changes"))
        .iter()
        .any(|v| dec(v) != Decimal::ZERO)
    {
        rows.push(three("Other changes", d.at("other_changes"), Kind::Total));
    }
    rows.push(three(
        "Change in net assets",
        d.at("change_in_net_assets"),
        Kind::Total,
    ));
    rows.push(three(
        "Net assets, beginning of period",
        d.at("net_assets_beginning"),
        Kind::Total,
    ));
    rows.push(three(
        "Net assets, end of period",
        d.at("net_assets_end"),
        Kind::Total,
    ));
    period_table::show(ui, "activities", "", &labels, &rows, Some(&total_title));
    let c = strs(d.at("change_in_net_assets"));
    ui.caption(format!(
        "Change in net assets {}",
        c.iter()
            .map(|v| fmt::group(v))
            .collect::<Vec<_>>()
            .join(" / ")
    ));
}

fn balance_by_fund(ui: &mut Ui, d: &Value) {
    let labels = labels_of(d.at("columns"));
    let mut rows = Vec::new();
    for (title, key, totals, all) in [
        ("Assets", "assets", "total_assets", "total_assets_all"),
        (
            "Liabilities",
            "liabilities",
            "total_liabilities",
            "total_liabilities_all",
        ),
    ] {
        rows.push(heading(title));
        for r in d.list(key) {
            rows.push(plain_row(
                r.str("name"),
                strs(r.at("amounts")),
                Some(r.str("total").to_owned()),
                Kind::Plain,
            ));
        }
        rows.push(plain_row(
            &format!("Total {}", title.to_lowercase()),
            strs(d.at(totals)),
            Some(d.str(all).to_owned()),
            Kind::Total,
        ));
    }
    rows.push(plain_row(
        "Net assets",
        strs(d.at("net_assets")),
        Some(d.str("total_net_assets").to_owned()),
        Kind::Total,
    ));
    period_table::show(
        ui,
        "balance-by-fund",
        "Account",
        &labels,
        &rows,
        Some("Total"),
    );
    if d.at("balanced").as_bool() == Some(false) {
        ui.error_text("Assets less liabilities differ from net assets.");
    }
}

fn financial_position(ui: &mut Ui, d: &Value) {
    let mut lines: Vec<(String, String, i32, bool)> = Vec::new();
    let mut push = |name: &str, v: &str, strong: bool| {
        lines.push((name.to_owned(), v.to_owned(), i32::from(!strong), strong))
    };
    push("Assets", "", true);
    for r in d.list("assets") {
        push(r.str("name"), r.str("balance"), false);
    }
    push("Total assets", d.str("total_assets"), true);
    push("Liabilities", "", true);
    for r in d.list("liabilities") {
        push(r.str("name"), r.str("balance"), false);
    }
    push("Total liabilities", d.str("total_liabilities"), true);
    push("Net assets without donor restrictions", "", true);
    for r in d.list("without_donor_restrictions") {
        push(r.str("name"), r.str("net_assets"), false);
    }
    push(
        "Total without donor restrictions",
        d.str("total_without"),
        true,
    );
    push("Net assets with donor restrictions", "", true);
    for r in d.list("with_donor_restrictions") {
        let name = if r.at("overspent").as_bool() == Some(true) {
            format!("{} (overspent)", r.str("name"))
        } else {
            r.str("name").to_owned()
        };
        push(&name, r.str("net_assets"), false);
    }
    push("Total with donor restrictions", d.str("total_with"), true);
    push("Total net assets", d.str("total_net_assets"), true);
    push(
        "Total liabilities and net assets",
        d.str("total_liabilities_and_net_assets"),
        true,
    );
    statement(ui, &lines);
    if d.at("balanced").as_bool() == Some(false) {
        ui.error_text(format!(
            "Assets differ from liabilities and net assets by {}.",
            fmt::group(d.str("difference"))
        ));
    }
}

fn functional(ui: &mut Ui, d: &Value) {
    let labels = labels_of(d.at("columns"));
    let mut rows = Vec::new();
    for r in d.list("rows") {
        rows.push(plain_row(
            r.str("name"),
            strs(r.at("amounts")),
            Some(r.str("total").to_owned()),
            Kind::Plain,
        ));
    }
    rows.push(plain_row(
        "Total expenses",
        strs(d.at("totals")),
        Some(d.str("total").to_owned()),
        Kind::Total,
    ));
    period_table::show(ui, "functional", "Expense", &labels, &rows, Some("Total"));
}

// ----- Tax summary (chart-presets.md) ----------------------------------------------

fn tax_summary(ui: &mut Ui, d: &Value) {
    if let Some(y) = d.at("tax_year").as_i64() {
        ui.secondary(format!("Tax lines for {y}"));
    }
    ui.secondary(d.str("note"));
    let none = d.str("no_tax_lines");
    if !none.is_empty() {
        ui.banner(super::ui::Tone::Accent, |ui| {
            ui.next().grow();
            ui.label(none);
        });
    }
    let account_cell = |ui: &mut Ui, r: &Value| {
        let full = r.str("full_name");
        ui.row(|ui| {
            let code = r.str("code");
            if !code.is_empty() {
                ui.tone(super::ui::Tone::Secondary).mono().width(44);
                ui.label(code);
            }
            ui.tip(full).grow();
            ui.label(full);
        });
    };
    let activity = d.list("activity");
    if !activity.is_empty() {
        let cols = vec![
            Column {
                weight: 2,
                ..col("Tax line and account")
            },
            col_num("Amount"),
        ];
        ui.table("Activity", cols, |ui| {
            for g in activity {
                let tag = g.str("tax_line");
                heading_row(ui, tag);
                for r in g.list("accounts") {
                    ui.table_row(&format!("{tag}#{}", r.str("account")), false, false, |ui| {
                        account_cell(ui, r);
                        num(ui, r.str("amount"), false);
                    });
                    ui.indent_last(1);
                }
                ui.table_row(&format!("{tag}#total"), false, false, |ui| {
                    ui.bold();
                    ui.label(format!("Total {tag}"));
                    num(ui, g.str("total"), true);
                });
                ui.indent_last(1);
            }
        });
    }
    let balance = d.list("balance");
    if !balance.is_empty() {
        let cols = vec![
            Column {
                weight: 2,
                ..col("Tax line and account")
            },
            col_num("Beginning of year"),
            col_num("End of year"),
        ];
        ui.table("Balances", cols, |ui| {
            for g in balance {
                let tag = g.str("tax_line");
                heading_row(ui, tag);
                for r in g.list("accounts") {
                    ui.table_row(&format!("{tag}#{}", r.str("account")), false, false, |ui| {
                        account_cell(ui, r);
                        num(ui, r.str("beginning"), false);
                        num(ui, r.str("end"), false);
                    });
                    ui.indent_last(1);
                }
                ui.table_row(&format!("{tag}#total"), false, false, |ui| {
                    ui.bold();
                    ui.label(format!("Total {tag}"));
                    num(ui, g.str("beginning"), true);
                    num(ui, g.str("end"), true);
                });
                ui.indent_last(1);
            }
        });
    }
    let untagged = d.at("untagged");
    if !untagged.is_null() {
        let rows = untagged.list("accounts");
        let cols = vec![
            Column {
                weight: 2,
                ..col("Account")
            },
            col_num("Amount"),
        ];
        ui.table("Not tagged", cols, |ui| {
            if rows.is_empty() {
                ui.table_row("untagged#none", false, false, |ui| {
                    ui.tone(super::ui::Tone::Secondary);
                    ui.label("Every income and expense account with activity has a tax line.");
                    ui.label("");
                });
            }
            for r in rows {
                ui.table_row(
                    &format!("untagged#{}", r.str("account")),
                    false,
                    false,
                    |ui| {
                        account_cell(ui, r);
                        num(ui, r.str("amount"), false);
                    },
                );
            }
            ui.table_row("untagged#total", false, false, |ui| {
                ui.bold();
                ui.label("Total not tagged");
                num(ui, untagged.str("total"), true);
            });
        });
    }
    // investments-options-bonds-crypto.md revision 2: crypto rewards and interest by
    // kind, named as in the investment income report.
    let inc = d.at("investment_income");
    if !inc.is_null() {
        let cols = vec![
            Column {
                weight: 2,
                ..col("Item")
            },
            col_num("Amount"),
        ];
        ui.table(inc.str("title"), cols, |ui| {
            // release-prep.md §1: amounts already under a tax line are marked.
            ui.table_row("inv-income#note", false, false, |ui| {
                ui.tone(super::ui::Tone::Secondary);
                ui.label(inc.str("note"));
                ui.label("");
            });
            for r in inc.list("rows") {
                let id = match r.str("id") {
                    "" => r.str("key"),
                    id => id,
                };
                ui.table_row(&format!("inv-income#{id}"), false, false, |ui| {
                    let note = r.str("note");
                    if note.is_empty() {
                        ui.label(r.str("label"));
                    } else {
                        ui.label(format!("{} ({note})", r.str("label")));
                    }
                    num(ui, r.str("amount"), false);
                });
            }
            ui.table_row("inv-income#total", false, false, |ui| {
                ui.bold();
                ui.label(inc.str("total_label"));
                num(ui, inc.str("total"), true);
            });
        });
    }
    // mileage.md: standard-rate mileage, for the preparer.
    let m = d.at("mileage");
    if !m.is_null() {
        let cols = vec![
            Column {
                weight: 2,
                ..col("Vehicle and purpose")
            },
            col_num("Miles"),
            col_num("Value"),
        ];
        ui.table(m.str("title"), cols, |ui| {
            for v in m.list("vehicles") {
                let name = v.str("name");
                heading_row(ui, name);
                for r in v.list("rows") {
                    ui.table_row(
                        &format!("mileage#{}#{}", v.str("vehicle"), r.str("kind")),
                        false,
                        false,
                        |ui| {
                            ui.label(r.str("label"));
                            num(ui, r.str("miles"), false);
                            num(ui, r.str("value"), false);
                        },
                    );
                    ui.indent_last(1);
                }
            }
        });
        ui.secondary(m.str("note"));
    }
    excluded(ui, d);
}
