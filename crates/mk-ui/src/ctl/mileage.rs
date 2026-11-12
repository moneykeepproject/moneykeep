//! The Mileage screen (app-desktop.md revision 36, spec mileage.md): the Trips tab
//! (filters, table with totals, + New trip, Mileage report…) and the Vehicles tab
//! (vehicles with their odometer for the year), the trip and vehicle dialogs, the
//! report dialog and Settings → Mileage (defaults, Rates with Re-apply rates…, Saved
//! trips). Everything goes through `trip.*`, `vehicle.*`, `saved_trip.*`,
//! `mileage_rate.*` and `mileage.*`.

use base64::Engine;
use mk_core::Decimal;
use serde_json::{Value, json};

use super::form::{date_row, pick, set};
use super::lists::accounts_where;
use super::sales::{cell, money};
use super::tags::{self, Tags, TagsOn};
use super::ui::{Column, Tone, Ui, col, col_num};
use super::util::{Json, array};
use super::{App, AppEvent, MessageAction};
use crate::fmt;

/// The module this screen belongs to.
pub(crate) const MODULE: &str = "mileage";

/// Purpose kinds and their labels, in display order.
const KINDS: [(&str, &str); 5] = [
    ("business", "Business"),
    ("medical", "Medical"),
    ("moving", "Moving"),
    ("charity", "Charity"),
    ("personal", "Personal"),
];

const NO_VEHICLE: &str = "Add a vehicle first: trips are logged per vehicle.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Tab {
    #[default]
    Trips,
    Vehicles,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TripForm {
    pub id: Option<String>,
    pub saved: String,
    pub date: String,
    pub vehicle: String,
    pub kind: String,
    pub purpose: String,
    pub from: String,
    pub to: String,
    pub odometer: bool,
    pub miles: String,
    pub start: String,
    pub end: String,
    pub round_trip: bool,
    pub reimbursable: bool,
    pub driver: String,
    pub rate: String,
    pub category: String,
    pub tags: Tags,
    pub tags_on: TagsOn,
    pub status: String,
    pub source: String,
    pub warnings: Vec<String>,
    pub notes: Vec<String>,
    pub error: Option<String>,
    pub duplicate: bool,
    pub confirm_delete: bool,
    /// Add vehicle inline: the name being typed.
    pub new_vehicle: Option<String>,
    /// Save as a saved trip: the name being typed.
    pub save_as: Option<String>,
    /// Post after saving (kept while the duplicate question is open).
    pub post_after: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct VehicleForm {
    pub id: Option<String>,
    pub name: String,
    pub plate: String,
    pub note: String,
    pub active: bool,
    pub year: String,
    pub start: String,
    pub end: String,
    pub error: Option<String>,
    pub confirm_delete: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ReportDialog {
    pub from: String,
    pub to: String,
    pub vehicle: String,
    pub kinds: [bool; 5],
    pub csv: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RateForm {
    pub id: Option<String>,
    pub kind: usize,
    pub from: String,
    pub cents: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SavedForm {
    pub id: Option<String>,
    pub name: String,
    pub from: String,
    pub to: String,
    pub miles: String,
    pub round_trip: bool,
    pub kind: String,
    pub purpose: String,
    pub vehicle: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Reapply {
    pub from: String,
    pub to: String,
    pub preview: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct MileageState {
    pub tab: Tab,
    pub year: usize,
    pub from: String,
    pub to: String,
    pub vehicle: String,
    pub kind: usize,
    pub form: Option<TripForm>,
    pub vehicle_form: Option<VehicleForm>,
    pub report: Option<ReportDialog>,
    pub rate_form: Option<RateForm>,
    pub saved_form: Option<SavedForm>,
    pub reapply: Option<Reapply>,
    cache: Option<(u64, String, Vec<Value>)>,
    lists: Option<(u64, Lists)>,
}

/// What the dialogs need from the book, loaded once per generation.
#[derive(Debug, Clone, Default)]
struct Lists {
    vehicles: Vec<Value>,
    saved: Vec<Value>,
    rates: Vec<Value>,
    show_moving: bool,
    default_kind: String,
    expenses_on: bool,
}

fn lists(app: &mut App) -> Lists {
    if let Some((g, l)) = &app.mileage.lists
        && *g == app.generation
    {
        return l.clone();
    }
    let vehicles = array(app.backend.invoke("vehicle.list", json!({})).ok());
    let saved = array(app.backend.invoke("saved_trip.list", json!({})).ok());
    let rates = app
        .backend
        .invoke("mileage_rate.list", json!({}))
        .ok()
        .map(|v| v.list("rates").to_vec())
        .unwrap_or_default();
    let settings = app
        .backend
        .invoke("module.settings", json!({"module": MODULE}))
        .ok()
        .map(|s| s.list("settings").to_vec())
        .unwrap_or_default();
    let value = |key: &str| {
        settings
            .iter()
            .find(|x| x.str("key") == key)
            .map(|x| x.at("value").clone())
            .unwrap_or(Value::Null)
    };
    let l = Lists {
        vehicles,
        saved,
        rates,
        show_moving: value("mileage.show_moving").as_bool() == Some(true),
        default_kind: value("mileage.default_purpose")
            .as_str()
            .unwrap_or("business")
            .to_owned(),
        expenses_on: app.module_on("expenses"),
    };
    app.mileage.lists = Some((app.generation, l.clone()));
    l
}

fn kind_label(kind: &str) -> &'static str {
    KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or("", |(_, l)| l)
}

/// The kinds a picker offers: moving only when turned on (or already chosen).
fn kinds_shown(l: &Lists, current: &str) -> Vec<(&'static str, &'static str)> {
    KINDS
        .iter()
        .copied()
        .filter(|(k, _)| *k != "moving" || l.show_moving || current == "moving")
        .collect()
}

fn kind_combo(ui: &mut Ui, label: &str, kind: &mut String, l: &Lists) {
    let shown = kinds_shown(l, kind);
    let labels: Vec<String> = shown.iter().map(|(_, x)| (*x).to_owned()).collect();
    let mut idx = shown
        .iter()
        .position(|(k, _)| k == kind)
        .unwrap_or(usize::MAX);
    ui.width(200);
    if ui.combo(label, &mut idx, &labels).changed()
        && let Some((k, _)) = shown.get(idx)
    {
        *kind = (*k).to_owned();
    }
}

fn active_vehicles(l: &Lists) -> Vec<String> {
    l.vehicles
        .iter()
        .filter(|v| v.at("active").as_bool() == Some(true))
        .map(|v| v.str("name").to_owned())
        .collect()
}

/// The standard rate of a kind on a date, from the book's table.
fn rate_for(l: &Lists, kind: &str, date: &str) -> Option<Decimal> {
    if kind == "personal" {
        return Some(Decimal::ZERO);
    }
    l.rates
        .iter()
        .filter(|r| r.str("kind") == kind && r.str("from") <= date)
        .max_by(|a, b| a.str("from").cmp(b.str("from")))
        .and_then(|r| r.str("cents").parse().ok())
}

fn dec(text: &str) -> Option<Decimal> {
    fmt::parse_entry(text).ok().flatten()
}

fn plain(d: Decimal) -> String {
    d.normalize().to_string()
}

fn miles_shown(d: Decimal) -> String {
    let mut d = d.round_dp(1);
    d.rescale(1);
    fmt::group(&d.to_string())
}

fn value_of(miles: Decimal, cents: Decimal) -> Decimal {
    (miles * cents / Decimal::ONE_HUNDRED)
        .round_dp_with_strategy(2, mk_core::RoundingStrategy::MidpointAwayFromZero)
}

fn money_text(d: Decimal) -> String {
    let mut d = d;
    d.rescale(2);
    fmt::group(&d.to_string())
}

fn year_options(app: &App) -> Vec<String> {
    let y: i32 = app
        .today()
        .get(..4)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2026);
    let mut v = vec!["Any year".to_owned()];
    v.extend((0..5).map(|i| (y - i).to_string()));
    v
}

fn rows(app: &mut App) -> Vec<Value> {
    let st = &app.mileage;
    let mut args = json!({});
    if let Ok(Some(d)) = fmt::read_date(&st.from) {
        set(&mut args, "from", json!(d.to_string()));
    }
    if let Ok(Some(d)) = fmt::read_date(&st.to) {
        set(&mut args, "to", json!(d.to_string()));
    }
    if !st.vehicle.is_empty() {
        set(&mut args, "vehicle", json!(st.vehicle));
    }
    if st.kind > 0
        && let Some((k, _)) = KINDS.get(st.kind - 1)
    {
        set(&mut args, "kind", json!(k));
    }
    let key = args.to_string();
    if let Some((g, k, v)) = &app.mileage.cache
        && *g == app.generation
        && *k == key
    {
        return v.clone();
    }
    let list = array(app.read("trip.list", args));
    app.mileage.cache = Some((app.generation, key, list.clone()));
    list
}

/// Draw the screen.
pub(crate) fn view(app: &mut App, ui: &mut Ui) {
    ui.title("Mileage");
    ui.toolbar(|ui| {
        for (tab, label) in [(Tab::Trips, "Trips"), (Tab::Vehicles, "Vehicles")] {
            ui.named(label);
            if ui.tab(label, app.mileage.tab == tab).clicked() {
                app.mileage.tab = tab;
            }
        }
    });
    match app.mileage.tab {
        Tab::Trips => ui.scope("trips", |ui| trips_tab(app, ui)),
        Tab::Vehicles => ui.scope("vehicles", |ui| vehicles_tab(app, ui)),
    }
    trip_dialog(app, ui);
    vehicle_dialog(app, ui);
    report_dialog(app, ui);
}

fn status_text(t: &Value) -> String {
    let mut s = match t.str("status") {
        "draft" => "Needs a vehicle",
        "ready" => "Reimbursable, ready",
        "posted" if !t.at("reimbursement").is_null() => "Reimbursable, reimbursed",
        "posted" => "Reimbursable, posted",
        _ => "Log only",
    }
    .to_owned();
    if t.at("reported").as_bool() == Some(true) {
        s.push_str(", reported");
    }
    s
}

fn trips_tab(app: &mut App, ui: &mut Ui) {
    let l = lists(app);
    ui.wrap(|ui| {
        if ui.primary("+ New trip").clicked() {
            open_new(app, &l);
        }
        if ui.button("Mileage report…").clicked() {
            open_report(app);
        }
    });
    let years = year_options(app);
    let vehicle_names: Vec<String> = l
        .vehicles
        .iter()
        .map(|v| v.str("name").to_owned())
        .collect();
    let mut kind_labels = vec!["Any purpose"];
    kind_labels.extend(KINDS.iter().map(|(_, x)| *x));
    ui.wrap(|ui| {
        ui.width(130);
        if ui.combo("Year", &mut app.mileage.year, &years).changed() {
            match years.get(app.mileage.year).filter(|_| app.mileage.year > 0) {
                Some(y) => {
                    app.mileage.from = fmt::date(&format!("{y}-01-01"));
                    app.mileage.to = fmt::date(&format!("{y}-12-31"));
                }
                None => {
                    app.mileage.from.clear();
                    app.mileage.to.clear();
                }
            }
        }
        ui.label("From");
        super::register::date_field(ui, "Trips from", &mut app.mileage.from);
        ui.label("To");
        super::register::date_field(ui, "Trips to", &mut app.mileage.to);
        pick(
            ui,
            "Vehicle filter",
            &mut app.mileage.vehicle,
            &vehicle_names,
            Some("Any vehicle"),
        );
        super::form::choice(
            ui,
            "Purpose kind filter",
            &mut app.mileage.kind,
            &kind_labels,
        );
    });
    if l.vehicles.is_empty() {
        ui.card("", |ui| {
            ui.label(NO_VEHICLE);
            ui.row(|ui| {
                if ui.button("Add vehicle").clicked() {
                    open_vehicle(app, None);
                }
            });
        });
        return;
    }
    let list = rows(app);
    if list.is_empty() {
        ui.card("", |ui| {
            ui.label("No trips here yet. Log one with + New trip.");
        });
        return;
    }
    let cols = vec![
        Column {
            width: 104,
            ..col("Date")
        },
        Column {
            weight: 2,
            ..col("Vehicle")
        },
        Column {
            weight: 4,
            ..col("Route")
        },
        Column {
            width: 96,
            ..col("Purpose kind")
        },
        Column {
            weight: 3,
            ..col("Purpose")
        },
        Column {
            width: 80,
            ..col_num("Miles")
        },
        Column {
            width: 64,
            ..col_num("Rate")
        },
        Column {
            width: 96,
            ..col_num("Amount")
        },
        Column {
            weight: 2,
            ..col("Status")
        },
        Column {
            width: 104,
            ..col("")
        },
    ];
    let mut open: Option<Value> = None;
    let (mut miles, mut value, mut repay) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    ui.table("", cols, |ui| {
        for t in &list {
            let id = t.str("id").to_owned();
            let m = dec(t.str("miles")).unwrap_or_default();
            miles += m;
            let shown_amount = if t.at("reimbursable").as_bool() == Some(true) {
                repay += dec(t.str("reimburse_amount")).unwrap_or_default();
                t.str("reimburse_amount").to_owned()
            } else {
                if t.str("purpose_kind") != "personal" {
                    value += dec(t.str("amount")).unwrap_or_default();
                }
                t.str("amount").to_owned()
            };
            ui.table_row(&format!("trip:{id}"), false, false, |ui| {
                let route = if t.str("route").is_empty() {
                    "(no route)".to_owned()
                } else {
                    t.str("route").to_owned()
                };
                ui.tip("Open this trip");
                ui.named(format!("Open trip {} {route}", t.str("date")));
                if ui.link(fmt::date(t.str("date"))).clicked() {
                    open = Some(t.clone());
                }
                cell(ui, t.str("vehicle_name"));
                cell(ui, route);
                ui.label(t.str("purpose_kind_label"));
                cell(ui, t.str("purpose"));
                ui.amount(miles_shown(m));
                let rate = if t.str("purpose_kind") == "personal" {
                    String::new()
                } else {
                    let r = if t.at("reimbursable").as_bool() == Some(true) {
                        t.str("reimburse_rate_cents")
                    } else {
                        t.str("rate_cents")
                    };
                    format!("{}¢", dec(r).map(plain).unwrap_or_default())
                };
                ui.amount(rate);
                if t.str("purpose_kind") == "personal" {
                    ui.label("");
                } else {
                    money(ui, &shown_amount);
                }
                let w = t.list("warnings");
                if w.is_empty() {
                    ui.label(status_text(t));
                } else {
                    let tip: Vec<&str> = w.iter().filter_map(Value::as_str).collect();
                    ui.tip(tip.join("\n")).tone(Tone::Warning);
                    ui.label(format!("{} (check)", status_text(t)));
                }
                if t.str("source") == "phone" {
                    ui.badge("From phone", Tone::Accent);
                } else {
                    ui.label("");
                }
            });
        }
        ui.table_row("trip:total", false, false, |ui| {
            ui.bold();
            ui.label("Total");
            ui.label(format!(
                "{} trip{}",
                list.len(),
                if list.len() == 1 { "" } else { "s" }
            ));
            ui.label("");
            ui.label("");
            ui.label("");
            ui.bold().amount(miles_shown(miles));
            ui.label("");
            ui.bold().amount(money_text(value + repay));
            ui.label("");
            ui.label("");
        });
    });
    ui.secondary(format!(
        "Deduction value of log-only trips {} (information for your tax preparer, not in the books); reimbursable trips {}.",
        money_text(value),
        money_text(repay)
    ));
    if let Some(t) = open {
        open_edit(app, &t);
    }
}

// ----- trip dialog --------------------------------------------------------------------

fn open_new(app: &mut App, l: &Lists) {
    let today = app.today();
    let tags_on = tags::options(app);
    let active = active_vehicles(l);
    let vehicle = match active.as_slice() {
        [one] => one.clone(),
        _ => String::new(),
    };
    let new_vehicle = l.vehicles.is_empty().then(String::new);
    app.mileage.form = Some(TripForm {
        date: fmt::date(&today),
        vehicle,
        kind: l.default_kind.clone(),
        tags_on,
        new_vehicle,
        ..TripForm::default()
    });
}

fn open_edit(app: &mut App, t: &Value) {
    let tags_on = tags::options(app);
    let odometer = !t.at("odometer_start").is_null() || !t.at("odometer_end").is_null();
    app.mileage.form = Some(TripForm {
        id: Some(t.str("id").to_owned()),
        date: fmt::date(t.str("date")),
        vehicle: t.str("vehicle_name").to_owned(),
        kind: t.str("purpose_kind").to_owned(),
        purpose: t.str("purpose").to_owned(),
        from: t.str("from_place").to_owned(),
        to: t.str("to_place").to_owned(),
        odometer,
        miles: t.str("entered_miles").to_owned(),
        start: t.str("odometer_start").to_owned(),
        end: t.str("odometer_end").to_owned(),
        round_trip: t.at("round_trip").as_bool() == Some(true),
        reimbursable: t.at("reimbursable").as_bool() == Some(true),
        driver: t.str("driver").to_owned(),
        rate: t.str("reimburse_rate_custom").to_owned(),
        category: if t.at("category").is_null() {
            String::new()
        } else {
            t.str("category_name").to_owned()
        },
        tags: Tags::from_names(t.at("tag_names")),
        tags_on,
        status: t.str("status").to_owned(),
        source: t.str("source").to_owned(),
        warnings: t
            .list("warnings")
            .iter()
            .filter_map(|n| n.as_str().map(str::to_owned))
            .collect(),
        notes: t
            .list("notes")
            .iter()
            .filter_map(|n| n.as_str().map(str::to_owned))
            .collect(),
        ..TripForm::default()
    });
}

/// The trip's miles from the form: the odometer difference, or the miles typed
/// (twice them for a round trip).
fn form_miles(f: &TripForm) -> Option<Decimal> {
    if f.odometer {
        let (s, e) = (dec(&f.start)?, dec(&f.end)?);
        return (e > s).then(|| e - s);
    }
    let m = dec(&f.miles)?;
    Some(if f.round_trip { m * Decimal::TWO } else { m })
}

#[derive(Default)]
struct TripActs {
    save: bool,
    post: bool,
    anyway: bool,
    cancel: bool,
    delete: Option<Option<bool>>,
    add_vehicle: bool,
    save_as: bool,
    fill: Option<Value>,
}

fn trip_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.mileage.form.take() else {
        return;
    };
    let l = lists(app);
    let vehicles = active_vehicles(&l);
    let saved_names: Vec<String> = l.saved.iter().map(|s| s.str("name").to_owned()).collect();
    let categories = {
        let ls = app.lists().clone();
        accounts_where(&ls, |a| a.str("type") == "expense")
    };
    let posted = f.status == "posted";
    let mut act = TripActs {
        cancel: ui.dialog_closed("trip-form"),
        ..TripActs::default()
    };
    let title = if f.id.is_some() {
        "Edit trip"
    } else {
        "New trip"
    };
    ui.dialog("trip-form", title, 600, |ui| {
        ui.close_label("Close");
        if posted {
            ui.secondary("This trip is in your books: changes also change its transaction.");
        }
        if f.source == "phone" {
            ui.badge("From phone", Tone::Accent);
        }
        ui.group(|ui| {
            if !saved_names.is_empty() && f.id.is_none() {
                ui.form("Saved trip", |ui| {
                    let before = f.saved.clone();
                    if pick(ui, "Saved trip", &mut f.saved, &saved_names, Some("None"))
                        && f.saved != before
                    {
                        act.fill = l.saved.iter().find(|s| s.str("name") == f.saved).cloned();
                    }
                });
            }
            date_row(ui, "Trip date", &mut f.date);
            ui.form("Vehicle", |ui| {
                if f.new_vehicle.is_none() {
                    pick(ui, "Vehicle", &mut f.vehicle, &vehicles, None);
                    if ui.button("Add vehicle").clicked() {
                        f.new_vehicle = Some(String::new());
                    }
                } else if let Some(n) = f.new_vehicle.as_mut() {
                    ui.width(220).hint("e.g. 2019 Honda CR-V");
                    ui.field("New vehicle name", n);
                    if ui.button("Add").clicked() {
                        act.add_vehicle = true;
                    }
                    if !vehicles.is_empty() && ui.button("Choose a vehicle").clicked() {
                        f.new_vehicle = None;
                    }
                }
            });
            ui.form("Purpose kind", |ui| {
                kind_combo(ui, "Purpose kind", &mut f.kind, &l);
            });
            ui.form("Purpose", |ui| {
                ui.width(320)
                    .hint("e.g. Food pantry delivery for St. Mark's");
                ui.field("Purpose", &mut f.purpose);
            });
            ui.form("From", |ui| {
                ui.width(240);
                ui.field("From", &mut f.from);
            });
            ui.form("To", |ui| {
                ui.width(240);
                ui.field("To", &mut f.to);
            });
            ui.form("", |ui| {
                if ui
                    .checkbox(&mut f.odometer, "Enter odometer readings")
                    .clicked()
                    && f.odometer
                {
                    f.round_trip = false;
                }
            });
            if f.odometer {
                ui.form("Odometer start", |ui| {
                    ui.width(140).mono();
                    ui.field("Odometer start", &mut f.start);
                });
                ui.form("Odometer end", |ui| {
                    ui.width(140).mono();
                    ui.field("Odometer end", &mut f.end);
                });
            } else {
                ui.form(if f.round_trip { "Miles one way" } else { "Miles" }, |ui| {
                    ui.width(140).mono();
                    ui.field("Miles", &mut f.miles);
                });
            }
            ui.form("", |ui| {
                ui.enabled(!f.odometer);
                if f.odometer {
                    ui.tip("Odometer readings already include the way back.");
                }
                ui.checkbox(&mut f.round_trip, "Round trip");
            });
            let miles = form_miles(&f);
            if f.round_trip
                && !f.odometer
                && let (Some(one), Some(m)) = (dec(&f.miles), miles)
            {
                ui.secondary(format!(
                    "One way {} mi → {} mi",
                    miles_shown(one),
                    miles_shown(m)
                ));
            }
            if l.expenses_on && f.kind != "personal" {
                ui.form("", |ui| {
                    ui.enabled(!posted);
                    ui.checkbox(&mut f.reimbursable, "Reimbursable");
                });
            }
            let iso = fmt::date_arg(&f.date).unwrap_or_default();
            let standard = rate_for(&l, &f.kind, &iso);
            if f.reimbursable && l.expenses_on {
                ui.form("Driver", |ui| {
                    ui.width(240).hint("The book's name");
                    ui.field("Driver", &mut f.driver);
                });
                ui.form("Reimbursement rate", |ui| {
                    ui.width(100).mono().hint("Standard");
                    ui.field("Reimbursement rate (cents per mile)", &mut f.rate);
                    ui.secondary("¢ per mile; empty uses the standard rate");
                });
                ui.form("Category", |ui| {
                    pick(
                        ui,
                        "Category",
                        &mut f.category,
                        &categories,
                        Some("Auto: Mileage reimbursement"),
                    );
                });
                let on = f.tags_on.clone();
                tags::rows(ui, &on, &mut f.tags, "Trip");
            }
            // The computed value.
            let rate = if f.reimbursable {
                dec(&f.rate).or(standard)
            } else {
                standard
            };
            let what = if f.reimbursable {
                "Amount"
            } else {
                "Deduction value"
            };
            match (miles, rate) {
                (Some(m), Some(r)) if f.kind != "personal" => {
                    ui.form(what, |ui| {
                        ui.label(format!(
                            "{} mi × {}¢ = {}",
                            miles_shown(m),
                            plain(r),
                            money_text(value_of(m, r))
                        ));
                    });
                }
                (Some(m), _) if f.kind == "personal" => {
                    ui.form("Miles", |ui| {
                        ui.label(format!(
                            "{} mi (personal: no rate)",
                            miles_shown(m)
                        ));
                    });
                }
                (_, None) if f.kind != "personal" && !iso.is_empty() => {
                    ui.form(what, |ui| {
                        ui.tone(Tone::Warning);
                        ui.label(
                            "No standard rate for this date; add one in Settings → Mileage → Rates",
                        );
                    });
                }
                _ => {}
            }
        });
        for w in f.warnings.iter().chain(f.notes.iter()) {
            ui.tone(Tone::Warning);
            ui.label(w.clone());
        }
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        if f.duplicate {
            ui.banner(Tone::Warning, |ui| {
                ui.label("A trip with these details already exists. Save anyway?");
                ui.row(|ui| {
                    if ui.button("Save anyway").clicked() {
                        act.anyway = true;
                    }
                    if ui.button("Don't save").clicked() {
                        f.duplicate = false;
                    }
                });
            });
        }
        if let Some(name) = f.save_as.as_mut() {
            ui.card("Save as a saved trip", |ui| {
                ui.form("Name", |ui| {
                    ui.width(240).hint("e.g. Office to St. Mark's");
                    ui.field("Saved trip name", name);
                });
                ui.row(|ui| {
                    if ui.button("Save route").clicked() {
                        act.save_as = true;
                    }
                });
            });
        }
        if f.confirm_delete {
            ui.banner(Tone::Warning, |ui| {
                if posted {
                    ui.label("Delete this trip? Its transaction can go too, or stay in your books.");
                    ui.row(|ui| {
                        if ui.button("Delete trip and transaction").clicked() {
                            act.delete = Some(Some(true));
                        }
                        if ui.button("Delete trip, keep transaction").clicked() {
                            act.delete = Some(Some(false));
                        }
                        if ui.button("Keep it").clicked() {
                            f.confirm_delete = false;
                        }
                    });
                } else {
                    ui.label("Delete this trip? You can undo it afterwards.");
                    ui.row(|ui| {
                        if ui.button("Delete trip").clicked() {
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
            if f.save_as.is_none() && ui.button("Save as a saved trip…").clicked() {
                f.save_as = Some(f.to.clone());
            }
            ui.spacer();
            if ui.primary("Save").clicked() {
                act.save = true;
            }
            if f.reimbursable && !posted && l.expenses_on {
                ui.tip("Save, then put it in your books (debit the category, credit Reimbursements Payable)");
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
    finish_trip(app, f, act);
}

fn trip_args(f: &TripForm, allow_duplicate: bool) -> Result<Value, String> {
    let date = fmt::date_arg(&f.date)?;
    let mut args = json!({
        "date": date,
        "purpose_kind": f.kind,
        "purpose": f.purpose.trim(),
        "from_place": f.from.trim(),
        "to_place": f.to.trim(),
    });
    if !f.vehicle.is_empty() {
        set(&mut args, "vehicle", json!(f.vehicle));
    }
    if f.odometer {
        let start = dec(&f.start).ok_or("Enter the odometer start reading.")?;
        let end = dec(&f.end).ok_or("Enter the odometer end reading.")?;
        set(&mut args, "odometer_start", json!(start.to_string()));
        set(&mut args, "odometer_end", json!(end.to_string()));
        set(&mut args, "round_trip", json!(false));
    } else {
        let m = dec(&f.miles).ok_or("Enter the miles.")?;
        set(&mut args, "miles", json!(m.to_string()));
        set(&mut args, "round_trip", json!(f.round_trip));
        if f.id.is_some() {
            set(&mut args, "clear_odometer", json!(true));
        }
    }
    let reimbursable = f.reimbursable && f.kind != "personal";
    set(&mut args, "reimbursable", json!(reimbursable));
    if reimbursable {
        set(&mut args, "driver", json!(f.driver.trim()));
        set(&mut args, "reimburse_rate_cents", json!(f.rate.trim()));
        set(&mut args, "category", json!(f.category));
        f.tags.apply_given(&mut args, &f.tags_on);
    }
    if allow_duplicate {
        set(&mut args, "allow_duplicate", json!(true));
    }
    Ok(args)
}

fn save_trip(app: &mut App, f: &mut TripForm, allow_duplicate: bool) -> Result<Value, String> {
    let mut args = trip_args(f, allow_duplicate)?;
    let out = match &f.id {
        Some(id) => {
            set(&mut args, "trip", json!(id));
            app.run("trip.update", args)
        }
        None => {
            if let Some(o) = args.as_object_mut() {
                o.retain(|k, v| v.as_str() != Some("") || k == "reimburse_rate_cents");
                if !f.saved.is_empty() {
                    let l = lists(app);
                    if let Some(s) = l.saved.iter().find(|s| s.str("name") == f.saved) {
                        o.insert("saved_trip".into(), json!(s.str("id")));
                    }
                }
            }
            app.run("trip.create", args)
        }
    };
    match out {
        Ok(v) => {
            f.id = Some(v.str("id").to_owned());
            f.status = v.str("status").to_owned();
            f.duplicate = false;
            Ok(v)
        }
        Err(e) if e.code == "duplicate_trip" => {
            f.duplicate = true;
            Err(String::new())
        }
        Err(e) => Err(e.display()),
    }
}

fn fill_from_saved(f: &mut TripForm, s: &Value) {
    f.from = s.str("from_place").to_owned();
    f.to = s.str("to_place").to_owned();
    f.miles = s.str("miles").to_owned();
    f.round_trip = s.at("round_trip").as_bool() == Some(true);
    f.odometer = false;
    f.kind = s.str("purpose_kind").to_owned();
    f.purpose = s.str("purpose").to_owned();
    if !s.str("vehicle_name").is_empty() {
        f.vehicle = s.str("vehicle_name").to_owned();
    }
}

fn finish_trip(app: &mut App, mut f: TripForm, act: TripActs) {
    if let Some(s) = &act.fill {
        fill_from_saved(&mut f, s);
    }
    if act.add_vehicle {
        let name = f.new_vehicle.clone().unwrap_or_default();
        match app.run("vehicle.create", json!({"name": name.trim()})) {
            Ok(v) => {
                f.vehicle = v.str("name").to_owned();
                f.new_vehicle = None;
                f.error = None;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    if act.save_as {
        let name = f.save_as.clone().unwrap_or_default();
        let mut args = json!({
            "name": name.trim(), "from_place": f.from.trim(), "to_place": f.to.trim(),
            "round_trip": f.round_trip, "purpose_kind": f.kind, "purpose": f.purpose.trim(),
            "vehicle": f.vehicle,
        });
        let one_way = if f.odometer {
            form_miles(&f)
        } else {
            dec(&f.miles)
        };
        if let Some(m) = one_way {
            set(&mut args, "miles", json!(m.to_string()));
        }
        match app.run("saved_trip.create", args) {
            Ok(_) => {
                f.save_as = None;
                f.error = None;
                app.notify_with(
                    format!("Saved the route {}.", name.trim()),
                    MessageAction::Undo,
                );
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    if let Some(choice) = act.delete {
        let mut args = json!({"trip": f.id});
        if let Some(t) = choice {
            set(&mut args, "delete_transaction", json!(t));
        }
        match app.run("trip.delete", args) {
            Ok(_) => {
                app.notify_with("Deleted the trip.", MessageAction::Undo);
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    let mut close = act.cancel;
    if act.save {
        f.post_after = act.post;
    }
    if act.save || act.anyway {
        f.error = None;
        match save_trip(app, &mut f, act.anyway) {
            Ok(_) if f.post_after => match app.run("trip.post", json!({"trip": f.id})) {
                Ok(_) => {
                    app.notify_with("Posted the trip.", MessageAction::Undo);
                    close = true;
                }
                Err(e) => f.error = Some(e.display()),
            },
            Ok(_) => {
                app.notify_with("Saved the trip.", MessageAction::Undo);
                close = true;
            }
            Err(e) if e.is_empty() => {}
            Err(e) => f.error = Some(e),
        }
    }
    if !close {
        app.mileage.form = Some(f);
    }
}

// ----- vehicles -----------------------------------------------------------------------

fn this_year(app: &App) -> String {
    let years = year_options(app);
    if app.mileage.year > 0
        && let Some(y) = years.get(app.mileage.year)
    {
        return y.clone();
    }
    app.today().get(..4).unwrap_or("2026").to_owned()
}

fn vehicles_tab(app: &mut App, ui: &mut Ui) {
    let l = lists(app);
    let year = this_year(app);
    ui.wrap(|ui| {
        if ui.primary("+ Add vehicle").clicked() {
            open_vehicle(app, None);
        }
    });
    ui.secondary(format!(
        "Odometer readings for {year} (1 January or when placed in service, 31 December or when retired) give the year's total miles."
    ));
    if l.vehicles.is_empty() {
        ui.card("", |ui| ui.label(NO_VEHICLE));
        return;
    }
    let cols = vec![
        Column {
            weight: 3,
            ..col("Vehicle")
        },
        Column {
            weight: 2,
            ..col("Plate")
        },
        Column {
            width: 90,
            ..col("Active")
        },
        Column {
            width: 120,
            ..col_num("Odometer start")
        },
        Column {
            width: 120,
            ..col_num("Odometer end")
        },
        Column {
            width: 110,
            ..col_num("Total miles")
        },
        Column {
            width: 110,
            ..col_num("Trip miles")
        },
    ];
    let mut open: Option<Value> = None;
    ui.table(&format!("Vehicles, {year}"), cols, |ui| {
        for v in &l.vehicles {
            let odo = v.at("odometer").at(&year);
            ui.table_row(&format!("vehicle:{}", v.str("id")), false, false, |ui| {
                ui.tip("Edit this vehicle");
                if ui.link(v.str("name")).clicked() {
                    open = Some(v.clone());
                }
                cell(ui, v.str("plate"));
                ui.label(if v.at("active").as_bool() == Some(true) {
                    "Active"
                } else {
                    "Inactive"
                });
                for k in ["start", "end", "miles"] {
                    ui.amount(dec(odo.str(k)).map(miles_shown).unwrap_or_default());
                }
                ui.amount(
                    dec(v.at("trip_miles").str(&year))
                        .map(miles_shown)
                        .unwrap_or_else(|| "0.0".into()),
                );
            });
        }
    });
    if let Some(v) = open {
        open_vehicle(app, Some(&v));
    }
}

fn open_vehicle(app: &mut App, v: Option<&Value>) {
    let year = this_year(app);
    app.mileage.vehicle_form = Some(match v {
        Some(v) => {
            let odo = v.at("odometer").at(&year);
            VehicleForm {
                id: Some(v.str("id").to_owned()),
                name: v.str("name").to_owned(),
                plate: v.str("plate").to_owned(),
                note: v.str("note").to_owned(),
                active: v.at("active").as_bool() == Some(true),
                start: odo.str("start").to_owned(),
                end: odo.str("end").to_owned(),
                year,
                ..VehicleForm::default()
            }
        }
        None => VehicleForm {
            active: true,
            year,
            ..VehicleForm::default()
        },
    });
}

fn vehicle_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.mileage.vehicle_form.take() else {
        return;
    };
    let (mut save, mut cancel, mut delete) = (false, ui.dialog_closed("vehicle-form"), false);
    let title = if f.id.is_some() {
        "Edit vehicle"
    } else {
        "Add vehicle"
    };
    ui.dialog("vehicle-form", title, 480, |ui| {
        ui.group(|ui| {
            ui.form("Name", |ui| {
                ui.width(260).hint("e.g. 2019 Honda CR-V");
                ui.field("Vehicle name", &mut f.name);
            });
            ui.form("Plate", |ui| {
                ui.width(160);
                ui.field("Plate", &mut f.plate);
            });
            ui.form("Note", |ui| {
                ui.width(260);
                ui.field("Note", &mut f.note);
            });
            ui.form("", |ui| {
                ui.checkbox(&mut f.active, "Active (shown in pickers)");
            });
            ui.form("Tax year", |ui| {
                ui.width(80).mono();
                ui.field("Tax year", &mut f.year);
            });
            ui.form("Odometer start", |ui| {
                ui.width(140).mono();
                ui.field("Odometer at start of year", &mut f.start);
            });
            ui.form("Odometer end", |ui| {
                ui.width(140).mono();
                ui.field("Odometer at end of year", &mut f.end);
            });
        });
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        if f.confirm_delete {
            ui.banner(Tone::Warning, |ui| {
                ui.label("Delete this vehicle? A vehicle with trips can only be made inactive.");
                ui.row(|ui| {
                    if ui.button("Delete vehicle").clicked() {
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
    if delete {
        match app.run("vehicle.delete", json!({"vehicle": f.id})) {
            Ok(_) => {
                app.notify_with("Deleted the vehicle.", MessageAction::Undo);
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    if save {
        let base = json!({"name": f.name.trim(), "plate": f.plate.trim(), "note": f.note.trim(),
                          "active": f.active});
        let out = match &f.id {
            Some(id) => {
                let mut a = base;
                set(&mut a, "vehicle", json!(id));
                app.run("vehicle.update", a)
            }
            None => {
                let mut a = base;
                if let Some(o) = a.as_object_mut() {
                    o.retain(|_, v| v.as_str() != Some(""));
                }
                app.run("vehicle.create", a)
            }
        };
        match out {
            Ok(v) => {
                let id = v.str("id").to_owned();
                let year: i32 = f.year.trim().parse().unwrap_or(0);
                let had = f.id.is_some();
                if year > 0
                    && (had || !f.start.trim().is_empty() || !f.end.trim().is_empty())
                    && let Err(e) = app.run(
                        "vehicle.set_odometer",
                        json!({"vehicle": id, "year": year,
                               "start": f.start.trim().replace(',', ""),
                               "end": f.end.trim().replace(',', "")}),
                    )
                {
                    f.id = Some(id);
                    f.error = Some(e.display());
                    app.mileage.vehicle_form = Some(f);
                    return;
                }
                app.notify_with("Saved the vehicle.", MessageAction::Undo);
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.mileage.vehicle_form = Some(f);
}

// ----- report -------------------------------------------------------------------------

fn open_report(app: &mut App) {
    let years = year_options(app);
    let (from, to) = if app.mileage.year > 0
        && let Some(y) = years.get(app.mileage.year)
    {
        (format!("{y}-01-01"), format!("{y}-12-31"))
    } else {
        let y = app.today().get(..4).unwrap_or("2026").to_owned();
        (format!("{y}-01-01"), format!("{y}-12-31"))
    };
    app.mileage.report = Some(ReportDialog {
        from: fmt::date(&from),
        to: fmt::date(&to),
        vehicle: app.mileage.vehicle.clone(),
        kinds: [true; 5],
        ..ReportDialog::default()
    });
}

fn report_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.mileage.report.take() else {
        return;
    };
    let l = lists(app);
    let names: Vec<String> = l
        .vehicles
        .iter()
        .map(|v| v.str("name").to_owned())
        .collect();
    let (mut save, mut cancel) = (false, ui.dialog_closed("mileage-report"));
    ui.dialog("mileage-report", "Mileage report", 520, |ui| {
        ui.group(|ui| {
            date_row(ui, "Report from", &mut d.from);
            date_row(ui, "Report to", &mut d.to);
            ui.form("Vehicle", |ui| {
                pick(ui, "Report vehicle", &mut d.vehicle, &names, Some("All vehicles"));
            });
            ui.form("Purpose kinds", |ui| {
                for (i, (k, label)) in KINDS.iter().enumerate() {
                    if *k == "moving" && !l.show_moving {
                        continue;
                    }
                    if let Some(on) = d.kinds.get_mut(i) {
                        ui.checkbox(on, label);
                    }
                }
            });
            ui.form("Format", |ui| {
                ui.radio(&mut d.csv, false, "PDF");
                ui.radio(&mut d.csv, true, "CSV (spreadsheet)");
            });
        });
        ui.secondary(
            "Per vehicle: odometer readings, total miles and miles by purpose with percentages; then every trip by purpose with subtotals and the deduction value by kind.",
        );
        if let Some(e) = &d.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save report…").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if !save {
        app.mileage.report = Some(d);
        return;
    }
    let (from, to) = match (fmt::date_arg(&d.from), fmt::date_arg(&d.to)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            d.error = Some(e);
            app.mileage.report = Some(d);
            return;
        }
    };
    let kinds: Vec<&str> = KINDS
        .iter()
        .zip(d.kinds.iter())
        .filter(|((k, _), on)| **on && (*k != "moving" || l.show_moving))
        .map(|((k, _), _)| *k)
        .collect();
    if kinds.is_empty() {
        d.error = Some("Choose at least one purpose kind.".into());
        app.mileage.report = Some(d);
        return;
    }
    let mut args = json!({"from": from, "to": to, "format": if d.csv { "csv" } else { "pdf" }});
    if kinds.len() < KINDS.len() - usize::from(!l.show_moving) {
        set(&mut args, "kinds", json!(kinds));
    }
    if !d.vehicle.is_empty() {
        set(&mut args, "vehicle", json!(d.vehicle));
    }
    match app.backend.invoke("mileage.report", args) {
        Ok(v) if d.csv => app.emit(AppEvent::SaveFile {
            file_name: v.str("file_name").to_owned(),
            content: v.str("content").to_owned(),
        }),
        Ok(v) => match base64::engine::general_purpose::STANDARD.decode(v.str("content_base64")) {
            Ok(data) => {
                for w in v.list("warnings") {
                    if let Some(t) = w.as_str() {
                        app.notify(t.to_owned());
                    }
                }
                app.emit(AppEvent::SaveBytes {
                    file_name: v.str("file_name").to_owned(),
                    data,
                });
            }
            Err(_) => {
                d.error = Some("The report could not be made.".into());
                app.mileage.report = Some(d);
            }
        },
        Err(e) => {
            d.error = Some(e.display());
            app.mileage.report = Some(d);
        }
    }
}

// ----- Settings → Mileage -------------------------------------------------------------

/// Settings → Mileage: defaults, the rate table and the saved trips.
pub(crate) fn settings_section(app: &mut App, ui: &mut Ui) {
    ui.card("Defaults", |ui| {
        super::settings::module_settings(app, ui, MODULE);
    });
    let rates = app
        .read("mileage_rate.list", json!({}))
        .unwrap_or(Value::Null);
    let mut edit: Option<Value> = None;
    let mut delete: Option<String> = None;
    {
        ui.secondary("Standard rates in cents per mile by purpose kind. A trip uses the row of its kind with the latest start date on or before the trip, and keeps that rate when saved.");
        let cols = vec![
            Column {
                width: 120,
                ..col("From")
            },
            Column {
                weight: 2,
                ..col("Purpose kind")
            },
            Column {
                width: 140,
                ..col_num("Cents per mile")
            },
            Column {
                width: 160,
                ..col("")
            },
        ];
        ui.table("Standard rates", cols, |ui| {
            for r in rates.list("rates") {
                let id = r.str("id").to_owned();
                ui.table_row(&format!("rate:{id}"), false, false, |ui| {
                    ui.label(fmt::date(r.str("from")));
                    ui.label(r.str("kind_label"));
                    ui.amount(format!("{}¢", r.str("cents")));
                    ui.row(|ui| {
                        let what = format!("{} from {}", r.str("kind_label"), r.str("from"));
                        ui.named(format!("Edit rate {what}"));
                        if ui.button("Edit").clicked() {
                            edit = Some(r.clone());
                        }
                        ui.named(format!("Delete rate {what}"));
                        if ui.button("Delete").clicked() {
                            delete = Some(id.clone());
                        }
                    });
                });
            }
        });
        ui.caption(rates.str("note"));
        ui.row(|ui| {
            if ui.button("+ Add rate").clicked() {
                app.mileage.rate_form = Some(RateForm::default());
            }
            if ui.button("Re-apply rates…").clicked() {
                let y = app.today().get(..4).unwrap_or("2026").to_owned();
                app.mileage.reapply = Some(Reapply {
                    from: fmt::date(&format!("{y}-01-01")),
                    to: fmt::date(&format!("{y}-12-31")),
                    ..Reapply::default()
                });
            }
        });
    }
    if let Some(r) = edit {
        app.mileage.rate_form = Some(RateForm {
            id: Some(r.str("id").to_owned()),
            kind: ["business", "medical", "moving", "charity"]
                .iter()
                .position(|k| *k == r.str("kind"))
                .unwrap_or(0),
            from: fmt::date(r.str("from")),
            cents: r.str("cents").to_owned(),
            error: None,
        });
    }
    if let Some(id) = delete {
        match app.run("mileage_rate.delete", json!({"rate": id})) {
            Ok(_) => app.notify_with("Deleted the rate.", MessageAction::Undo),
            Err(e) => app.notify_error(&e),
        }
    }
    saved_card(app, ui);
    rate_dialog(app, ui);
    reapply_dialog(app, ui);
    saved_dialog(app, ui);
}

fn saved_card(app: &mut App, ui: &mut Ui) {
    let l = lists(app);
    let mut edit: Option<Value> = None;
    let mut delete: Option<String> = None;
    {
        if l.saved.is_empty() {
            ui.card("Saved trips", |ui| {
                ui.secondary("Routes you drive often; choose one in the trip form to fill it in (up to 200).");
            });
        } else {
            ui.secondary("Saved trips: routes you drive often; choose one in the trip form to fill it in (up to 200).");
            let cols = vec![
                Column {
                    weight: 2,
                    ..col("Name")
                },
                Column {
                    weight: 3,
                    ..col("Route")
                },
                Column {
                    width: 90,
                    ..col_num("One way")
                },
                Column {
                    width: 100,
                    ..col("Purpose kind")
                },
                Column {
                    weight: 2,
                    ..col("Vehicle")
                },
                Column {
                    width: 160,
                    ..col("")
                },
            ];
            ui.table("Saved trips", cols, |ui| {
                for s in &l.saved {
                    let id = s.str("id").to_owned();
                    ui.table_row(&format!("saved:{id}"), false, false, |ui| {
                        cell(ui, s.str("name"));
                        cell(ui, s.str("route"));
                        ui.amount(dec(s.str("miles")).map(miles_shown).unwrap_or_default());
                        ui.label(kind_label(s.str("purpose_kind")));
                        cell(ui, s.str("vehicle_name"));
                        ui.row(|ui| {
                            ui.named(format!("Edit saved trip {}", s.str("name")));
                            if ui.button("Edit").clicked() {
                                edit = Some(s.clone());
                            }
                            ui.named(format!("Delete saved trip {}", s.str("name")));
                            if ui.button("Delete").clicked() {
                                delete = Some(id.clone());
                            }
                        });
                    });
                }
            });
        }
        ui.row(|ui| {
            if ui.button("+ New saved trip").clicked() {
                app.mileage.saved_form = Some(SavedForm {
                    kind: l.default_kind.clone(),
                    ..SavedForm::default()
                });
            }
        });
    }
    if let Some(s) = edit {
        app.mileage.saved_form = Some(SavedForm {
            id: Some(s.str("id").to_owned()),
            name: s.str("name").to_owned(),
            from: s.str("from_place").to_owned(),
            to: s.str("to_place").to_owned(),
            miles: s.str("miles").to_owned(),
            round_trip: s.at("round_trip").as_bool() == Some(true),
            kind: s.str("purpose_kind").to_owned(),
            purpose: s.str("purpose").to_owned(),
            vehicle: s.str("vehicle_name").to_owned(),
            error: None,
        });
    }
    if let Some(id) = delete {
        match app.run("saved_trip.delete", json!({"saved_trip": id})) {
            Ok(_) => app.notify_with("Deleted the saved trip.", MessageAction::Undo),
            Err(e) => app.notify_error(&e),
        }
    }
}

fn rate_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.mileage.rate_form.take() else {
        return;
    };
    let (mut save, mut cancel) = (false, ui.dialog_closed("rate-form"));
    let title = if f.id.is_some() {
        "Edit rate"
    } else {
        "Add rate"
    };
    ui.dialog("rate-form", title, 420, |ui| {
        ui.group(|ui| {
            ui.form("Purpose kind", |ui| {
                super::form::choice(
                    ui,
                    "Rate purpose kind",
                    &mut f.kind,
                    &["Business", "Medical", "Moving", "Charity"],
                );
            });
            date_row(ui, "Starts on", &mut f.from);
            ui.form("Cents per mile", |ui| {
                ui.width(100).mono();
                ui.field("Cents per mile", &mut f.cents);
            });
        });
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
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
    if save {
        let kind = ["business", "medical", "moving", "charity"]
            .get(f.kind)
            .copied()
            .unwrap_or("business");
        match fmt::date_arg(&f.from) {
            Err(e) => f.error = Some(e),
            Ok(from) => {
                let mut args = json!({"kind": kind, "from": from, "cents": f.cents.trim()});
                if let Some(id) = &f.id {
                    set(&mut args, "rate", json!(id));
                }
                match app.run("mileage_rate.set", args) {
                    Ok(_) => {
                        app.notify_with(
                            "Saved the rate. Trips already saved keep their rate until you re-apply rates.",
                            MessageAction::Undo,
                        );
                        return;
                    }
                    Err(e) => f.error = Some(e.display()),
                }
            }
        }
    }
    app.mileage.rate_form = Some(f);
}

fn reapply_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.mileage.reapply.take() else {
        return;
    };
    let (mut preview, mut apply, mut cancel) = (false, false, ui.dialog_closed("reapply-rates"));
    ui.dialog("reapply-rates", "Re-apply rates", 460, |ui| {
        ui.secondary("Trips keep the rate they were saved with. This applies the rate table again to the trips of a period.");
        ui.group(|ui| {
            date_row(ui, "Re-apply from", &mut d.from);
            date_row(ui, "Re-apply to", &mut d.to);
        });
        if let Some(p) = &d.preview {
            let n = p.at("trips").as_u64().unwrap_or(0);
            ui.bold();
            ui.label(if n == 0 {
                "No trip in this period changes.".to_owned()
            } else {
                format!(
                    "{n} trip{} change; total difference {}.",
                    if n == 1 { "" } else { "s" },
                    fmt::group(p.str("difference"))
                )
            });
        }
        if let Some(e) = &d.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.button("Preview").clicked() {
                preview = true;
            }
            let ready = d.preview.as_ref().is_some_and(|p| p.at("trips").as_u64() > Some(0));
            ui.enabled(ready);
            if ui.primary("Apply").clicked() {
                apply = true;
            }
        });
    });
    if cancel {
        return;
    }
    if preview || apply {
        match (fmt::date_arg(&d.from), fmt::date_arg(&d.to)) {
            (Ok(from), Ok(to)) => {
                let args = json!({"from": from, "to": to, "preview": !apply});
                let out = if apply {
                    app.run("mileage.reapply_rates", args)
                } else {
                    app.backend.invoke("mileage.reapply_rates", args)
                };
                match out {
                    Ok(v) if apply => {
                        let n = v.at("trips").as_u64().unwrap_or(0);
                        app.notify_with(
                            format!(
                                "Applied the rates to {n} trip{}.",
                                if n == 1 { "" } else { "s" }
                            ),
                            MessageAction::Undo,
                        );
                        return;
                    }
                    Ok(v) => {
                        d.preview = Some(v);
                        d.error = None;
                    }
                    Err(e) => d.error = Some(e.display()),
                }
            }
            (Err(e), _) | (_, Err(e)) => d.error = Some(e),
        }
    }
    app.mileage.reapply = Some(d);
}

fn saved_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.mileage.saved_form.take() else {
        return;
    };
    let l = lists(app);
    let names: Vec<String> = l
        .vehicles
        .iter()
        .map(|v| v.str("name").to_owned())
        .collect();
    let (mut save, mut cancel) = (false, ui.dialog_closed("saved-trip-form"));
    let title = if f.id.is_some() {
        "Edit saved trip"
    } else {
        "New saved trip"
    };
    ui.dialog("saved-trip-form", title, 520, |ui| {
        ui.group(|ui| {
            ui.form("Name", |ui| {
                ui.width(240);
                ui.field("Saved trip name", &mut f.name);
            });
            ui.form("From", |ui| {
                ui.width(240);
                ui.field("From", &mut f.from);
            });
            ui.form("To", |ui| {
                ui.width(240);
                ui.field("To", &mut f.to);
            });
            ui.form("Miles one way", |ui| {
                ui.width(120).mono();
                ui.field("Miles one way", &mut f.miles);
            });
            ui.form("", |ui| {
                ui.checkbox(&mut f.round_trip, "Round trip");
            });
            ui.form("Purpose kind", |ui| {
                kind_combo(ui, "Purpose kind", &mut f.kind, &l);
            });
            ui.form("Purpose", |ui| {
                ui.width(300);
                ui.field("Purpose", &mut f.purpose);
            });
            ui.form("Vehicle", |ui| {
                pick(ui, "Vehicle", &mut f.vehicle, &names, Some("Any"));
            });
        });
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
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
    if save {
        let mut args = json!({
            "name": f.name.trim(), "from_place": f.from.trim(), "to_place": f.to.trim(),
            "round_trip": f.round_trip, "purpose_kind": f.kind, "purpose": f.purpose.trim(),
            "vehicle": f.vehicle,
        });
        if let Some(m) = dec(&f.miles) {
            set(&mut args, "miles", json!(m.to_string()));
        }
        let out = match &f.id {
            Some(id) => {
                set(&mut args, "saved_trip", json!(id));
                app.run("saved_trip.update", args)
            }
            None => app.run("saved_trip.create", args),
        };
        match out {
            Ok(_) => {
                app.notify_with("Saved the route.", MessageAction::Undo);
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.mileage.saved_form = Some(f);
}

// ----- Expenses: trips in Record reimbursement… ---------------------------------------

/// Posted reimbursable trips not reimbursed yet (empty when the module is off).
pub(crate) fn open_reimbursable(app: &mut App) -> Vec<Value> {
    if !app.module_on(MODULE) {
        return Vec::new();
    }
    array(
        app.backend
            .invoke("trip.list", json!({"status": "posted"}))
            .ok(),
    )
    .into_iter()
    .filter(|t| t.at("reimbursable").as_bool() == Some(true) && t.at("reimbursement").is_null())
    .collect()
}

/// A trip's line in the reimbursement dialog.
pub(crate) fn reimburse_line(t: &Value) -> String {
    format!(
        "{}  Mileage — {}  {} mi  {}",
        fmt::date(t.str("date")),
        t.str("vehicle_name"),
        dec(t.str("miles")).map(miles_shown).unwrap_or_default(),
        fmt::group(t.str("reimburse_amount"))
    )
}
