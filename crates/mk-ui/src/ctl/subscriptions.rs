//! The Subscriptions screen (subscriptions.md): tabs Subscriptions, Recurring bills
//! and Suggested (N) with totals, monthly equivalents, the year-ago comparison and
//! the Biggest increases panel; the detail view with charges, price history and
//! reason notes and the actions Mark cancelled, Pause, Make a reminder and Make a
//! bill; Add by hand; Review mode; ⋯ → Dismissed suggestions; and Settings →
//! Subscriptions (alert thresholds and the colour bands). Everything goes through
//! the `subscription.*` commands.

use mk_core::subscription as sub;
use serde_json::{Value, json};

use super::form::set;
use super::ui::{Column, Tone, Ui, col, col_num};
use super::util::{Json, array};
use super::{App, MessageAction};
use crate::{AppEvent, View, fmt};

/// The module this screen belongs to.
pub(crate) const MODULE: &str = "subscriptions";

const CADENCES: [(&str, &str); 6] = [
    ("weekly", "Weekly"),
    ("biweekly", "Every two weeks"),
    ("monthly", "Monthly"),
    ("quarterly", "Quarterly"),
    ("six_monthly", "Every six months"),
    ("yearly", "Yearly"),
];

const STATUSES: [(&str, &str); 4] = [
    ("active", "Active"),
    ("trial", "Free trial"),
    ("paused", "Paused"),
    ("cancelled", "Cancelled"),
];

/// Colour presets of the band colour chooser.
const PRESETS: [(&str, u32); 8] = [
    ("Yellow", 0xF2C94C),
    ("Orange", 0xF2994A),
    ("Red", 0xD64545),
    ("Pink", 0xE0569B),
    ("Purple", 0x8E44AD),
    ("Blue", 0x2F80ED),
    ("Green", 0x27AE60),
    ("Grey", 0x828282),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Tab {
    #[default]
    Subscriptions,
    Bills,
    Suggested,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Form {
    pub id: Option<String>,
    pub bill: bool,
    pub name: String,
    pub payee: String,
    pub contains: bool,
    pub account: String,
    pub cadence: usize,
    pub price: String,
    pub next: String,
    pub category: String,
    pub status: usize,
    pub trial_ends: String,
    pub used_by: String,
    pub link: String,
    pub notes: String,
    pub error: Option<String>,
    pub confirm_delete: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Review {
    pub queue: Vec<Value>,
    pub at: usize,
    pub downgrade: Option<String>,
    pub saved: String,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct SubsState {
    pub tab: Tab,
    pub detail: Option<String>,
    pub form: Option<Form>,
    pub review: Option<Review>,
    pub dismissed: bool,
    /// The address waiting for the open-link confirmation.
    pub link: Option<String>,
    /// Reason drafts by history row key, and the row to focus.
    pub reasons: Vec<(String, String)>,
    pub focus_reason: Option<String>,
    /// Settings → Subscriptions: band drafts (percent, hex) for one generation.
    pub bands: Option<(u64, Vec<(String, String)>)>,
    pub bands_error: Option<String>,
    cache: Option<(u64, String, Value)>,
}

fn cached(app: &mut App, command: &str, args: Value) -> Value {
    let key = format!("{command}{args}");
    if let Some((g, k, v)) = &app.subs.cache
        && *g == app.generation
        && *k == key
    {
        return v.clone();
    }
    let v = app.read(command, args).unwrap_or(Value::Null);
    app.subs.cache = Some((app.generation, key, v.clone()));
    v
}

fn today(app: &App) -> Value {
    json!(app.today())
}

fn list_data(app: &mut App) -> Value {
    let t = today(app);
    cached(app, "subscription.list", json!({"today": t}))
}

/// Open the screen on a subscription (Attention items), optionally on one history
/// row's reason note.
pub(crate) fn open(app: &mut App, target: &Value) {
    let id = target.str("subscription");
    app.subs.detail = (!id.is_empty()).then(|| id.to_owned());
    let reason = target.str("reason");
    app.subs.focus_reason = (!reason.is_empty()).then(|| reason.to_owned());
    if id.is_empty() {
        app.subs.tab = Tab::Subscriptions;
    }
    app.navigate(View::Subscriptions);
}

/// A chip in a colour of its own, with dark or light text for 4.5:1 contrast.
fn colour_chip(ui: &mut Ui, text: &str, color: u32, dark: bool, name: &str) {
    ui.named(name.to_owned());
    ui.color_badge(text, color, dark);
}

/// The coloured change chip of a history row: banded increases on their colour,
/// other increases in the normal text colour, decreases in the positive colour.
/// Colour is never the only signal: the arrow and the percentage are always shown.
pub(crate) fn change_chip(ui: &mut Ui, row: &Value) {
    if row.is_null() {
        ui.label("");
        return;
    }
    let pct = row.str("percent_text");
    if row.at("increase").as_bool() == Some(true) {
        let text = format!("↑ {}", pct.trim_start_matches('+'));
        match row.str("color") {
            "" => ui.label(text),
            c => {
                let color = sub::parse_hex(c).unwrap_or(0xF2C94C);
                let dark = row.at("dark_text").as_bool() != Some(false);
                colour_chip(ui, &text, color, dark, &format!("Price up {pct}"));
            }
        }
    } else {
        ui.tone(Tone::Success)
            .label(format!("↓ {}", pct.trim_start_matches('-')));
    }
}

/// Draw the screen.
pub(crate) fn view(app: &mut App, ui: &mut Ui) {
    if let Some(id) = app.subs.detail.clone() {
        detail(app, ui, &id);
    } else {
        main(app, ui);
    }
    form_dialog(app, ui);
    review_dialog(app, ui);
    dismissed_dialog(app, ui);
    link_dialog(app, ui);
}

fn main(app: &mut App, ui: &mut Ui) {
    ui.title("Subscriptions");
    let data = list_data(app);
    let n = data.at("suggested").as_u64().unwrap_or(0);
    let mut open_menu = None;
    ui.toolbar(|ui| {
        let suggested = format!("Suggested ({n})");
        for (tab, label) in [
            (Tab::Subscriptions, "Subscriptions"),
            (Tab::Bills, "Recurring bills"),
            (Tab::Suggested, suggested.as_str()),
        ] {
            ui.named(label);
            if ui.tab(label, app.subs.tab == tab).clicked() {
                app.subs.tab = tab;
            }
        }
        ui.spacer();
        if ui.button("+ Add by hand").clicked() {
            open_form(app, None);
        }
        if ui.button("Review…").clicked() {
            start_review(app);
        }
        ui.tip("More");
        let r = ui.menu("⋯", &["Dismissed suggestions".to_owned()]);
        open_menu = r.chosen;
    });
    if open_menu == Some(0) {
        app.subs.dismissed = true;
    }
    match app.subs.tab {
        Tab::Subscriptions => ui.scope("subs", |ui| subs_tab(app, ui, &data)),
        Tab::Bills => ui.scope("bills", |ui| bills_tab(app, ui, &data)),
        Tab::Suggested => ui.scope("suggested", |ui| suggested_tab(app, ui)),
    }
}

fn money(ui: &mut Ui, text: &str) {
    super::sales::money(ui, text);
}

fn subs_tab(app: &mut App, ui: &mut Ui, data: &Value) {
    let rows = data.list("subscriptions").to_vec();
    let totals = data.at("totals");
    let cols = vec![
        Column {
            weight: 1,
            ..col("Per month")
        },
        Column {
            weight: 1,
            ..col("Per year")
        },
        Column {
            weight: 1,
            ..col("A year ago, per month")
        },
        Column {
            weight: 1,
            ..col("Saved in reviews, a year")
        },
    ];
    ui.table("Totals (active subscriptions)", cols, |ui| {
        ui.table_row("totals", false, false, |ui| {
            ui.big_heading(fmt::group(totals.str("monthly")));
            ui.big_heading(fmt::group(totals.str("yearly")));
            ui.big_heading(fmt::group(totals.str("year_ago_monthly")));
            ui.big_heading(fmt::group(data.str("saved_year")));
        });
    });
    let change = totals.str("change_monthly");
    if !change.is_empty() && totals.str("year_ago_monthly") != "0.00" {
        let up = !change.starts_with('-');
        ui.tone(if up { Tone::Warning } else { Tone::Success })
            .label(format!(
                "{} {} a month versus the same date a year ago",
                if up { "↑" } else { "↓" },
                fmt::group(change.trim_start_matches('-'))
            ));
    }
    if rows.is_empty() {
        ui.card("", |ui| {
            ui.label("No subscriptions yet. Confirm one from Suggested, or add one by hand.");
        });
        return;
    }
    let cols = vec![
        Column {
            weight: 3,
            ..col("Name")
        },
        Column {
            width: 130,
            ..col("Cadence")
        },
        Column {
            width: 90,
            ..col_num("Price")
        },
        Column {
            width: 100,
            ..col_num("Per month")
        },
        Column {
            width: 110,
            ..col("Next charge")
        },
        Column {
            width: 96,
            ..col("Status")
        },
        Column {
            width: 120,
            ..col("Last change")
        },
    ];
    let mut open = None;
    ui.table("Subscriptions", cols, |ui| {
        for r in &rows {
            let id = r.str("id").to_owned();
            ui.table_row(&format!("sub:{id}"), false, false, |ui| {
                ui.named(format!("Open {}", r.str("name")));
                if ui.link(r.str("name")).clicked() {
                    open = Some(id.clone());
                }
                ui.label(r.str("cadence_label"));
                money(ui, r.str("price"));
                money(ui, r.str("monthly"));
                if r.str("status") == "cancelled" {
                    ui.label("");
                } else {
                    ui.label(fmt::date(r.str("next_charge")));
                }
                let tone = match r.str("status") {
                    "active" => Tone::Success,
                    "trial" => Tone::Accent,
                    _ => Tone::Secondary,
                };
                ui.badge(r.str("status_label"), tone);
                change_chip(ui, r.at("last_change"));
            });
        }
        ui.table_row("sub:total", false, false, |ui| {
            ui.bold().label("Total (active)");
            ui.label("");
            ui.label("");
            ui.bold().amount(fmt::group(totals.str("monthly")));
            ui.label("");
            ui.label("");
            ui.label("");
        });
    });
    ui.wrap(|ui| {
        ui.card("Biggest increases (last 12 months)", |ui| {
            let inc = data.list("biggest_increases");
            if inc.is_empty() {
                ui.secondary("No price rises in the last 12 months.");
            }
            for (i, r) in inc.iter().enumerate() {
                ui.scope(format!("inc-{i}"), |ui| {
                    ui.row(|ui| {
                        if ui.link(r.str("name")).clicked() {
                            open = Some(r.str("subscription").to_owned());
                        }
                        ui.label(format!(
                            "{} → {} on {}",
                            fmt::group(r.str("old")),
                            fmt::group(r.str("new")),
                            fmt::date(r.str("date"))
                        ));
                        change_chip(ui, r);
                        ui.secondary(format!("+{} a year", fmt::group(r.str("yearly_increase"))));
                    });
                });
            }
        });
        ui.card("By category (per month)", |ui| {
            for (i, c) in totals.list("by_category").iter().enumerate() {
                ui.scope(format!("cat-{i}"), |ui| {
                    ui.row(|ui| {
                        ui.label(c.str("name"));
                        ui.spacer();
                        ui.amount(fmt::group(c.str("monthly")));
                    });
                });
            }
        });
    });
    if let Some(id) = open {
        app.subs.detail = Some(id);
    }
}

fn bills_tab(app: &mut App, ui: &mut Ui, data: &Value) {
    let rows = data.list("bills").to_vec();
    ui.secondary("Recurring bills vary from charge to charge (electric, phone, insurance); they have no price alerts.");
    if rows.is_empty() {
        ui.card("", |ui| {
            ui.label("No recurring bills yet. Answer It's a recurring bill on a suggestion, or add one by hand.");
        });
        return;
    }
    let cols = vec![
        Column {
            weight: 3,
            ..col("Name")
        },
        Column {
            width: 130,
            ..col("Cadence")
        },
        Column {
            width: 100,
            ..col_num("Average")
        },
        Column {
            width: 100,
            ..col_num("Last")
        },
        Column {
            width: 110,
            ..col("Next charge")
        },
        Column {
            width: 110,
            ..col_num("This year")
        },
        Column {
            width: 110,
            ..col_num("Last year")
        },
    ];
    let mut open = None;
    ui.table("Recurring bills", cols, |ui| {
        for r in &rows {
            let id = r.str("id").to_owned();
            ui.table_row(&format!("bill:{id}"), false, false, |ui| {
                ui.named(format!("Open {}", r.str("name")));
                if ui.link(r.str("name")).clicked() {
                    open = Some(id.clone());
                }
                ui.label(r.str("cadence_label"));
                money(ui, r.str("average"));
                money(ui, r.str("last_amount"));
                ui.label(fmt::date(r.str("next_charge")));
                money(ui, r.str("year_total"));
                money(ui, r.str("last_year_total"));
            });
        }
    });
    if let Some(id) = open {
        app.subs.detail = Some(id);
    }
}

fn suggested_tab(app: &mut App, ui: &mut Ui) {
    let list = array(Some(cached(app, "subscription.suggestions", json!({}))));
    ui.secondary(
        "Payments that repeat on a schedule. Your answer is remembered for the payee and account.",
    );
    if list.is_empty() {
        ui.card("", |ui| {
            ui.label("Nothing to suggest. New suggestions appear after imports and feed syncs.");
        });
        return;
    }
    let cols = vec![
        Column {
            weight: 3,
            ..col("Payee")
        },
        Column {
            width: 150,
            ..col("Account")
        },
        Column {
            width: 110,
            ..col("Cadence")
        },
        Column {
            width: 100,
            ..col_num("Last amount")
        },
        Column {
            width: 150,
            ..col("Charges")
        },
        Column {
            width: 470,
            ..col("")
        },
    ];
    let mut answer: Option<(String, &str, String)> = None;
    ui.table("Suggested", cols, |ui| {
        for s in &list {
            let key = s.str("key").to_owned();
            let payee = s.str("payee").to_owned();
            ui.table_row(&format!("sugg:{key}"), false, false, |ui| {
                ui.label(payee.clone());
                ui.label(s.str("account_name"));
                ui.label(s.str("cadence_label"));
                money(ui, s.str("last_amount"));
                ui.label(format!(
                    "{} since {}",
                    s.at("charges"),
                    fmt::date(s.str("first_date"))
                ));
                ui.row(|ui| {
                    let (first, second) = if s.str("kind") == "bill" {
                        ("bill", "subscription")
                    } else {
                        ("subscription", "bill")
                    };
                    for kind in [first, second] {
                        let label = if kind == "bill" {
                            "It's a recurring bill"
                        } else {
                            "It's a subscription"
                        };
                        ui.named(format!("{label}: {payee}"));
                        let r = if kind == first {
                            ui.primary(label)
                        } else {
                            ui.button(label)
                        };
                        if r.clicked() {
                            answer = Some((key.clone(), kind, payee.clone()));
                        }
                    }
                    ui.named(format!("Not recurring: {payee}"));
                    if ui.button("Not recurring").clicked() {
                        answer = Some((key.clone(), "not_recurring", payee.clone()));
                    }
                });
            });
        }
    });
    if let Some((key, kind, payee)) = answer {
        let t = today(app);
        match app.run(
            "subscription.answer",
            json!({"key": key, "answer": kind, "today": t}),
        ) {
            Ok(_) => {
                let msg = match kind {
                    "not_recurring" => {
                        format!("{payee} won't be suggested again (⋯ → Dismissed suggestions).")
                    }
                    "bill" => format!("{payee} is now under Recurring bills."),
                    _ => format!("{payee} is now under Subscriptions."),
                };
                app.notify_with(msg, MessageAction::Undo);
            }
            Err(e) => app.notify_error(&e),
        }
    }
}

// ----- detail ---------------------------------------------------------------------------

fn detail(app: &mut App, ui: &mut Ui, id: &str) {
    let g = cached(app, "subscription.get", json!({"subscription": id}));
    if g.is_null() {
        app.subs.detail = None;
        ui.title("Subscriptions");
        return;
    }
    let is_sub = g.str("kind") == "subscription";
    ui.title(g.str("name"));
    let mut act: Option<&str> = None;
    ui.toolbar(|ui| {
        if ui.button("← All subscriptions").clicked() {
            act = Some("back");
        }
        ui.badge(
            g.str("status_label"),
            if g.str("status") == "active" {
                Tone::Success
            } else {
                Tone::Secondary
            },
        );
        ui.spacer();
        if ui.button("Edit…").clicked() {
            act = Some("edit");
        }
        if g.str("status") == "cancelled" || g.str("status") == "paused" {
            if ui.button("Mark active").clicked() {
                act = Some("active");
            }
        } else {
            if ui.button("Mark cancelled").clicked() {
                act = Some("cancel");
            }
            if ui.button("Pause").clicked() {
                act = Some("pause");
            }
        }
        ui.enabled(!g.at("reminder").is_null());
        if !g.at("reminder").is_null() {
            ui.tip(
                "A scheduled transaction that only reminds you (it is never entered automatically)",
            );
        } else {
            ui.tip("Needs a charge or an account and category first");
        }
        if ui.button("Make a reminder").clicked() {
            act = Some("reminder");
        }
        if app.module_on("bills") && ui.button("Make a bill").clicked() {
            act = Some("bill");
        }
        if !g.str("link").is_empty() && ui.button("Open link…").clicked() {
            act = Some("link");
        }
    });
    ui.wrap(|ui| {
        ui.card("Details", |ui| {
            let match_text = format!(
                "{} {}",
                if g.str("how") == "contains" {
                    "contains"
                } else {
                    "equals"
                },
                g.str("payee")
            );
            let rows: Vec<(&str, String)> = vec![
                (
                    "Kind",
                    if is_sub {
                        "Subscription".to_owned()
                    } else {
                        "Recurring bill".to_owned()
                    },
                ),
                ("Payee", match_text),
                (
                    "Account",
                    g.at("account_name")
                        .as_str()
                        .unwrap_or("Any account")
                        .to_owned(),
                ),
                ("Cadence", g.str("cadence_label").to_owned()),
                (
                    if is_sub { "Price" } else { "Average" },
                    fmt::group(if is_sub {
                        g.str("price")
                    } else {
                        g.str("average")
                    }),
                ),
                ("Per month", fmt::group(g.str("monthly"))),
                ("Per year", fmt::group(g.str("yearly"))),
                ("Next charge", fmt::date(g.str("next_charge"))),
                (
                    "Category",
                    g.at("category_name").as_str().unwrap_or("").to_owned(),
                ),
                ("Used by", g.str("used_by").to_owned()),
                ("Last reviewed", fmt::date(g.str("reviewed_on"))),
                ("Notes", g.str("notes").to_owned()),
            ];
            for (label, value) in rows {
                if value.is_empty() {
                    continue;
                }
                ui.form(label, |ui| ui.label(value));
            }
            if g.str("status") == "cancelled" {
                ui.form("Cancelled on", |ui| {
                    ui.label(fmt::date(g.str("cancelled_on")))
                });
            }
            if g.str("status") == "trial" {
                ui.form("Trial ends", |ui| ui.label(fmt::date(g.str("trial_ends"))));
            }
        });
        if is_sub {
            history_card(app, ui, &g);
        } else {
            bill_chart(ui, &g);
        }
    });
    charges_card(app, ui, &g);
    match act {
        Some("back") => app.subs.detail = None,
        Some("edit") => open_form(app, Some(&g)),
        Some(s @ ("cancel" | "pause" | "active")) => {
            let status = match s {
                "cancel" => "cancelled",
                "pause" => "paused",
                _ => "active",
            };
            let t = today(app);
            match app.run(
                "subscription.set_status",
                json!({"subscription": g.str("id"), "status": status, "on": t, "today": t}),
            ) {
                Ok(_) => app.notify_with(
                    format!("{} is {}.", g.str("name"), status),
                    MessageAction::Undo,
                ),
                Err(e) => app.notify_error(&e),
            }
        }
        Some("reminder") => {
            super::scheduled::from_subscription(app, g.at("reminder"));
        }
        Some("bill") => {
            // subscriptions.md revision 2: `make_bill_as` decides between a repeating
            // bill in Scheduled and a bill now (the editor, as before).
            match app.run(
                "subscription.make_bill",
                json!({"subscription": g.str("id")}),
            ) {
                Ok(r) if r.str("mode") == "bill_now" => {
                    let line = r
                        .at("draft")
                        .list("lines")
                        .first()
                        .cloned()
                        .unwrap_or_default();
                    super::purchases::new_bill_for(
                        app,
                        g.str("name"),
                        r.at("category_name").as_str().unwrap_or(""),
                        if is_sub {
                            line.str("amount")
                        } else {
                            g.str("last_amount")
                        },
                        g.str("next_charge"),
                    );
                    app.navigate(View::Purchases);
                }
                Ok(_) => app.notify_with(
                    format!(
                        "{} is a repeating bill in Scheduled; nothing posts until you enter it.",
                        g.str("name")
                    ),
                    MessageAction::Undo,
                ),
                Err(e) => app.notify_error(&e),
            }
        }
        Some("link") => app.subs.link = Some(g.str("link").to_owned()),
        _ => {}
    }
}

fn history_card(app: &mut App, ui: &mut Ui, g: &Value) {
    let id = g.str("id").to_owned();
    let rows = g.list("history").to_vec();
    let focus = app.subs.focus_reason.take();
    let mut save: Option<(String, String)> = None;
    {
        if rows.is_empty() {
            ui.card("Price history", |ui| {
                ui.secondary("No price changes yet.");
            });
            return;
        }
        let cols = vec![
            Column {
                width: 104,
                ..col("Date")
            },
            Column {
                width: 80,
                ..col_num("Old")
            },
            Column {
                width: 80,
                ..col_num("New")
            },
            Column {
                width: 80,
                ..col_num("Change")
            },
            Column {
                width: 112,
                ..col("")
            },
            Column {
                weight: 3,
                ..col("Reason")
            },
        ];
        ui.table("Price history", cols, |ui| {
            for r in &rows {
                let key = r.str("key").to_owned();
                ui.table_row(&format!("hist:{key}"), false, false, |ui| {
                    ui.label(fmt::date(r.str("date")));
                    money(ui, r.str("old"));
                    money(ui, r.str("new"));
                    money(ui, r.str("change"));
                    ui.row(|ui| {
                        change_chip(ui, r);
                        if r.at("small").as_bool() == Some(true) {
                            ui.badge("small", Tone::Secondary);
                        }
                    });
                    let stored = r.str("reason").to_owned();
                    let mut text = app
                        .subs
                        .reasons
                        .iter()
                        .find(|(k, _)| *k == key)
                        .map_or_else(|| stored.clone(), |(_, t)| t.clone());
                    if focus.as_deref() == Some(key.as_str()) {
                        ui.focus();
                    }
                    ui.hint("Add a reason");
                    let resp = ui.field(&format!("Reason for {}", r.str("date")), &mut text);
                    if resp.changed {
                        app.subs.reasons.retain(|(k, _)| *k != key);
                        app.subs.reasons.push((key.clone(), text.clone()));
                    }
                    if (resp.blurred || resp.submitted) && text != stored {
                        save = Some((key.clone(), text.clone()));
                    }
                });
            }
        });
    }
    if let Some((row, reason)) = save {
        app.subs.reasons.retain(|(k, _)| *k != row);
        match app.run(
            "subscription.set_reason",
            json!({"subscription": id, "row": row, "reason": reason.trim()}),
        ) {
            Ok(_) => app.notify_with("Saved the reason.", MessageAction::Undo),
            Err(e) => app.notify_error(&e),
        }
    }
}

fn bill_chart(ui: &mut Ui, g: &Value) {
    ui.card("Amounts", |ui| {
        let list = g.list("charge_list");
        let mut pts: Vec<(String, mk_core::Decimal)> = list
            .iter()
            .map(|c| {
                (
                    fmt::date(c.str("date")),
                    c.str("amount").parse().unwrap_or_default(),
                )
            })
            .collect();
        pts.reverse();
        if pts.len() >= 2 {
            let labels: Vec<String> = pts.iter().map(|p| p.0.clone()).collect();
            let values: Vec<mk_core::Decimal> = pts.iter().map(|p| p.1).collect();
            super::charts::lines(
                ui,
                520,
                &labels,
                &[("Amount", values)],
                &format!("{} amounts over time", g.str("name")),
                &super::charts::Opens::default(),
            );
        } else {
            ui.secondary("A chart appears after two charges.");
        }
        ui.label(format!(
            "Average {} · last {}",
            fmt::group(g.str("average")),
            fmt::group(g.str("last_amount"))
        ));
    });
}

fn charges_card(app: &mut App, ui: &mut Ui, g: &Value) {
    let id = g.str("id").to_owned();
    let list = g.list("charge_list").to_vec();
    let unlinked = g.list("unlinked").to_vec();
    let mut remove = None;
    let mut add = None;
    {
        ui.secondary("The matching transactions (linked, not copied). A transaction belongs to one subscription only.");
        let cols = vec![
            Column {
                width: 104,
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
                width: 96,
                ..col_num("Amount")
            },
            Column {
                width: 100,
                ..col("")
            },
        ];
        ui.table("Charges", cols, |ui| {
            for c in &list {
                let txn = c.str("txn").to_owned();
                ui.table_row(&format!("charge:{txn}"), false, false, |ui| {
                    ui.label(fmt::date(c.str("date")));
                    super::sales::cell(ui, c.str("payee"));
                    ui.label(c.str("account_name"));
                    money(ui, c.str("amount"));
                    ui.named(format!("Remove charge {}", c.str("date")));
                    if ui.button("Remove").clicked() {
                        remove = Some(txn.clone());
                    }
                });
            }
        });
        if !unlinked.is_empty() {
            let labels: Vec<String> = unlinked
                .iter()
                .map(|u| {
                    format!(
                        "{} {} {}",
                        fmt::date(u.str("date")),
                        u.str("payee"),
                        fmt::group(u.str("amount"))
                    )
                })
                .collect();
            ui.row(|ui| {
                let mut idx = usize::MAX;
                ui.width(360);
                if ui.combo("Add a missed charge", &mut idx, &labels).changed()
                    && let Some(u) = unlinked.get(idx)
                {
                    add = Some(u.str("txn").to_owned());
                }
                if let Some(it) = ui.last() {
                    it.value = "Add a missed charge…".into();
                }
            });
        }
    }
    let run = |app: &mut App, cmd: &str, txn: String, msg: &str| match app
        .run(cmd, json!({"subscription": id, "txn": txn}))
    {
        Ok(_) => app.notify_with(msg.to_owned(), MessageAction::Undo),
        Err(e) => app.notify_error(&e),
    };
    if let Some(t) = remove {
        run(app, "subscription.remove_charge", t, "Removed the charge.");
    }
    if let Some(t) = add {
        run(app, "subscription.add_charge", t, "Added the charge.");
    }
}

fn link_dialog(app: &mut App, ui: &mut Ui) {
    let Some(url) = app.subs.link.clone() else {
        return;
    };
    let mut close = ui.dialog_closed("open-link");
    let mut go = false;
    ui.dialog("open-link", "Open this address?", 480, |ui| {
        ui.label("This opens your web browser at:");
        ui.mono().label(url.clone());
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                close = true;
            }
            if ui.primary("Open in browser").clicked() {
                go = true;
            }
        });
    });
    if go {
        app.emit(AppEvent::OpenLink { url });
        close = true;
    }
    if close {
        app.subs.link = None;
    }
}

// ----- add by hand and edit ---------------------------------------------------------

fn open_form(app: &mut App, g: Option<&Value>) {
    let f = match g {
        None => Form {
            cadence: 2,
            next: String::new(),
            ..Form::default()
        },
        Some(g) => Form {
            id: Some(g.str("id").to_owned()),
            bill: g.str("kind") == "bill",
            name: g.str("name").to_owned(),
            payee: g.str("payee").to_owned(),
            contains: g.str("how") == "contains",
            account: g.at("account_name").as_str().unwrap_or("").to_owned(),
            cadence: CADENCES
                .iter()
                .position(|c| c.0 == g.str("cadence"))
                .unwrap_or(2),
            price: g.str("price").to_owned(),
            next: fmt::date(g.str("next_charge")),
            category: g.at("category_name").as_str().unwrap_or("").to_owned(),
            status: STATUSES
                .iter()
                .position(|s| s.0 == g.str("status"))
                .unwrap_or(0),
            trial_ends: fmt::date(g.str("trial_ends")),
            used_by: g.str("used_by").to_owned(),
            link: g.str("link").to_owned(),
            notes: g.str("notes").to_owned(),
            ..Form::default()
        },
    };
    app.subs.form = Some(f);
}

fn form_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.subs.form.take() else {
        return;
    };
    let lists = app.lists().clone();
    let money_accounts = super::lists::accounts_where(&lists, |a| {
        matches!(
            a.str("subtype"),
            "bank" | "cash" | "credit-card" | "other-liability"
        )
    });
    let expenses = super::lists::accounts_where(&lists, |a| a.str("type") == "expense");
    let (mut save, mut cancel, mut delete) = (false, ui.dialog_closed("sub-form"), false);
    let title = if f.id.is_some() {
        "Edit subscription"
    } else {
        "Add by hand"
    };
    ui.dialog("sub-form", title, 560, |ui| {
        ui.group(|ui| {
            ui.form("Kind", |ui| {
                ui.radio(&mut f.bill, false, "Subscription");
                ui.radio(&mut f.bill, true, "Recurring bill");
            });
            ui.form("Name", |ui| {
                ui.width(280).hint("e.g. Netflix");
                ui.field("Name", &mut f.name);
            });
            ui.form("Payee to match", |ui| {
                ui.width(220).hint("as it appears in the register");
                ui.field("Payee to match", &mut f.payee);
                ui.checkbox(&mut f.contains, "Contains (not equals)");
            });
            ui.form("Account", |ui| {
                super::form::pick(
                    ui,
                    "Account",
                    &mut f.account,
                    &money_accounts,
                    Some("Any account"),
                );
            });
            ui.form("Cadence", |ui| {
                let labels: Vec<&str> = CADENCES.iter().map(|c| c.1).collect();
                super::form::choice(ui, "Cadence", &mut f.cadence, &labels);
            });
            ui.form(if f.bill { "Typical amount" } else { "Price" }, |ui| {
                ui.width(120).mono();
                ui.field("Price", &mut f.price);
            });
            ui.form("Next charge", |ui| {
                super::register::date_field(ui, "Next charge", &mut f.next);
            });
            ui.form("Category", |ui| {
                super::form::pick(
                    ui,
                    "Category",
                    &mut f.category,
                    &expenses,
                    Some("From the transactions"),
                );
            });
            ui.form("Status", |ui| {
                let labels: Vec<&str> = STATUSES.iter().map(|c| c.1).collect();
                super::form::choice(ui, "Status", &mut f.status, &labels);
            });
            if f.status == 1 {
                ui.form("Trial ends", |ui| {
                    super::register::date_field(ui, "Trial ends", &mut f.trial_ends);
                });
            }
            ui.form("Used by", |ui| {
                ui.width(220).hint("e.g. a family member");
                ui.field("Used by", &mut f.used_by);
            });
            ui.form("Link", |ui| {
                ui.width(320)
                    .hint("https://… (account or cancellation page)");
                ui.field("Link", &mut f.link);
            });
            ui.form("Notes", |ui| {
                ui.width(320);
                ui.multiline("Notes", &mut f.notes);
            });
        });
        ui.caption("Useful before three charges exist: a new subscription or a free trial.");
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        if f.confirm_delete {
            ui.banner(Tone::Warning, |ui| {
                ui.label("Delete this subscription? Its transactions stay in the registers.");
                ui.row(|ui| {
                    if ui.button("Delete subscription").clicked() {
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
        match app.run("subscription.delete", json!({"subscription": f.id})) {
            Ok(_) => {
                app.subs.detail = None;
                app.notify_with("Deleted the subscription.", MessageAction::Undo);
                return;
            }
            Err(e) => f.error = Some(e.display()),
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
        let dates = (read(&f.next), read(&f.trial_ends));
        let (Ok(next), Ok(trial)) = dates else {
            f.error = Some("Check the dates.".into());
            app.subs.form = Some(f);
            return;
        };
        let mut a = json!({
            "name": f.name.trim(),
            "kind": if f.bill { "bill" } else { "subscription" },
            "payee": if f.payee.trim().is_empty() { f.name.trim() } else { f.payee.trim() },
            "how": if f.contains { "contains" } else { "equals" },
            "account": f.account,
            "cadence": CADENCES.get(f.cadence).map_or("monthly", |c| c.0),
            "price": super::form::amount_arg(&f.price),
            "next_charge": next,
            "category": f.category,
            "status": STATUSES.get(f.status).map_or("active", |c| c.0),
            "used_by": f.used_by.trim(),
            "link": f.link.trim(),
            "notes": f.notes.trim(),
            "today": app.today(),
        });
        if !trial.is_empty() {
            set(&mut a, "trial_ends", json!(trial));
        }
        let out = match &f.id {
            Some(id) => {
                set(&mut a, "subscription", json!(id));
                app.run("subscription.update", a)
            }
            None => {
                if let Some(o) = a.as_object_mut() {
                    o.retain(|_, v| v.as_str() != Some(""));
                }
                app.run("subscription.create", a)
            }
        };
        match out {
            Ok(v) => {
                if f.id.is_none() {
                    app.subs.detail = Some(v.str("id").to_owned());
                }
                app.notify_with("Saved the subscription.", MessageAction::Undo);
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.subs.form = Some(f);
}

// ----- review -----------------------------------------------------------------------

fn start_review(app: &mut App) {
    let t = today(app);
    let Some(r) = app.read("subscription.review", json!({"today": t})) else {
        return;
    };
    app.subs.review = Some(Review {
        queue: r.list("queue").to_vec(),
        at: 0,
        downgrade: None,
        saved: r.str("saved_year").to_owned(),
        error: None,
    });
}

fn review_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut r) = app.subs.review.take() else {
        return;
    };
    let mut close = ui.dialog_closed("review");
    let mut answer: Option<&str> = None;
    let total = r.queue.len();
    ui.dialog("review", "Review subscriptions", 520, |ui| {
        let done = r.at.min(total);
        let pct = (done * 100)
            .checked_div(total)
            .and_then(|p| i32::try_from(p).ok())
            .unwrap_or(100);
        ui.progress(&format!("{done} of {total} reviewed"), pct);
        ui.label(format!("{done} of {total} reviewed"));
        match r.queue.get(r.at) {
            None => {
                ui.heading("Review finished");
                ui.label(if total == 0 {
                    "There are no active subscriptions to review.".to_owned()
                } else {
                    "Every active subscription has been looked at.".to_owned()
                });
            }
            Some(s) => {
                ui.heading(s.str("name"));
                ui.label(format!(
                    "{} {} · {} a year · last reviewed {}",
                    fmt::group(s.str("price")),
                    s.str("cadence_label").to_lowercase(),
                    fmt::group(s.str("yearly")),
                    if s.str("reviewed_on").is_empty() {
                        "never".to_owned()
                    } else {
                        fmt::date(s.str("reviewed_on"))
                    }
                ));
                if !s.at("last_change").is_null() {
                    ui.row(|ui| {
                        ui.secondary("Last change");
                        change_chip(ui, s.at("last_change"));
                    });
                }
                if !s.str("used_by").is_empty() {
                    ui.secondary(format!("Used by {}", s.str("used_by")));
                }
                if let Some(p) = r.downgrade.as_mut() {
                    ui.form("New price", |ui| {
                        ui.width(120).mono().focus();
                        ui.field("New price", p);
                    });
                    ui.row(|ui| {
                        if ui.primary("Save downgrade").clicked() {
                            answer = Some("downgrade");
                        }
                        if ui.button("Back").clicked() {
                            answer = Some("back");
                        }
                    });
                } else {
                    ui.row(|ui| {
                        if ui.primary("Keep").clicked() {
                            answer = Some("keep");
                        }
                        if ui.button("Cancel subscription").clicked() {
                            answer = Some("cancel");
                        }
                        if ui.button("Downgrade…").clicked() {
                            answer = Some("start_downgrade");
                        }
                        if ui.button("Skip").clicked() {
                            answer = Some("skip");
                        }
                    });
                }
            }
        }
        if let Some(e) = &r.error {
            ui.error_text(e.clone());
        }
        ui.banner(Tone::Success, |ui| {
            ui.bold()
                .label(format!("You've saved {} a year", fmt::group(&r.saved)));
        });
        ui.actions(|ui| {
            if ui.button("Close").clicked() {
                close = true;
            }
        });
    });
    if close {
        return;
    }
    let current = r.queue.get(r.at).cloned();
    match (answer, current) {
        (Some("skip"), _) => r.at += 1,
        (Some("start_downgrade"), _) => r.downgrade = Some(String::new()),
        (Some("back"), _) => r.downgrade = None,
        (Some(a @ ("keep" | "cancel" | "downgrade")), Some(s)) => {
            let mut args = json!({"subscription": s.str("id"), "answer": a, "today": app.today()});
            if a == "downgrade" {
                set(
                    &mut args,
                    "price",
                    json!(super::form::amount_arg(
                        r.downgrade.as_deref().unwrap_or("")
                    )),
                );
            }
            match app.run("subscription.review_answer", args) {
                Ok(v) => {
                    r.saved = v.str("saved_year").to_owned();
                    r.at += 1;
                    r.downgrade = None;
                    r.error = None;
                }
                Err(e) => r.error = Some(e.display()),
            }
        }
        _ => {}
    }
    app.subs.review = Some(r);
}

// ----- dismissed suggestions ----------------------------------------------------------

fn dismissed_dialog(app: &mut App, ui: &mut Ui) {
    if !app.subs.dismissed {
        return;
    }
    let list = array(Some(cached(app, "subscription.dismissed", json!({}))));
    let mut close = ui.dialog_closed("dismissed");
    let mut undo = None;
    ui.dialog("dismissed", "Dismissed suggestions", 520, |ui| {
        if list.is_empty() {
            ui.label("No dismissed suggestions.");
        }
        let cols = vec![
            Column {
                weight: 3,
                ..col("Payee")
            },
            Column {
                weight: 2,
                ..col("Account")
            },
            Column {
                width: 100,
                ..col("")
            },
        ];
        ui.table("Dismissed", cols, |ui| {
            for d in &list {
                ui.table_row(&format!("dis:{}", d.str("key")), false, false, |ui| {
                    ui.label(d.str("payee"));
                    ui.label(d.str("account_name"));
                    ui.named(format!("Undo {}", d.str("payee")));
                    if ui.button("Undo").clicked() {
                        undo = Some(d.str("key").to_owned());
                    }
                });
            }
        });
        ui.actions(|ui| {
            if ui.button("Close").clicked() {
                close = true;
            }
        });
    });
    if let Some(k) = undo {
        match app.run("subscription.restore_suggestion", json!({"key": k})) {
            Ok(_) => app.notify_with("The suggestion is back.", MessageAction::Undo),
            Err(e) => app.notify_error(&e),
        }
    }
    if close {
        app.subs.dismissed = false;
    }
}

// ----- Settings → Subscriptions ---------------------------------------------------------

fn bands(app: &mut App) -> Vec<(String, String)> {
    if let Some((g, b)) = &app.subs.bands
        && *g == app.generation
    {
        return b.clone();
    }
    let s = app
        .read("module.settings", json!({"module": MODULE}))
        .unwrap_or(Value::Null);
    let v = s
        .list("settings")
        .iter()
        .find(|x| x.str("key") == "subscriptions.bands")
        .map(|x| x.at("value").clone())
        .unwrap_or(Value::Null);
    let b: Vec<(String, String)> = v
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| (x.str("percent").to_owned(), x.str("color").to_owned()))
                .collect()
        })
        .unwrap_or_default();
    app.subs.bands = Some((app.generation, b.clone()));
    b
}

fn save_bands(app: &mut App, b: &[(String, String)]) {
    let v: Vec<Value> = b
        .iter()
        .map(|(p, c)| json!({"percent": p.trim().trim_end_matches('%').trim(), "color": c.trim()}))
        .collect();
    match app.run(
        "module.set_setting",
        json!({"key": "subscriptions.bands", "value": v}),
    ) {
        Ok(_) => {
            app.subs.bands_error = None;
            app.notify_with("Saved the colour bands.", MessageAction::Undo);
        }
        Err(e) => {
            app.subs.bands_error = Some(e.display());
            app.subs.bands = Some((app.generation, b.to_vec()));
        }
    }
}

pub(crate) fn settings_section(app: &mut App, ui: &mut Ui) {
    ui.card("Alerts", |ui| {
        super::settings::module_settings(app, ui, MODULE);
    });
    make_bill_card(app, ui);
    let mut b = bands(app);
    let mut changed = false;
    let mut commit = false;
    let mut remove = None;
    {
        let cols = vec![
            Column {
                width: 120,
                ..col("Preview")
            },
            Column {
                width: 110,
                ..col("From (%)")
            },
            Column {
                weight: 3,
                ..col("Colour")
            },
            Column {
                width: 100,
                ..col("")
            },
        ];
        ui.table("Colour bands", cols, |ui| {
            for (i, (pct, hex)) in b.iter_mut().enumerate() {
                ui.table_row(&format!("band:{i}"), false, false, |ui| {
                    let color = sub::parse_hex(hex);
                    match color {
                        Some(c) => colour_chip(
                            ui,
                            &format!("↑ {}%", pct.trim()),
                            c,
                            sub::dark_text_on(c),
                            &format!("Band {} preview", i + 1),
                        ),
                        None => ui.tone(Tone::Error).label("Not a colour"),
                    }
                    ui.width(90).mono();
                    let r = ui.field(&format!("Band {} percentage", i + 1), pct);
                    changed |= r.changed;
                    commit |= r.blurred || r.submitted;
                    ui.row(|ui| {
                        if let Some(c) = color {
                            super::form::swatch(ui, c);
                        }
                        ui.width(100).mono();
                        let r = ui.field(&format!("Band {} colour (hex)", i + 1), hex);
                        changed |= r.changed;
                        commit |= r.blurred || r.submitted;
                        let names: Vec<String> = PRESETS
                            .iter()
                            .map(|p| format!("{} {}", p.0, sub::hex(p.1)))
                            .collect();
                        ui.named(format!("Band {} colour presets", i + 1));
                        if let Some(k) = ui.menu("Presets", &names).chosen
                            && let Some(p) = PRESETS.get(k)
                        {
                            *hex = sub::hex(p.1);
                            changed = true;
                            commit = true;
                        }
                    });
                    ui.named(format!("Remove band {}", i + 1));
                    if ui.button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
        });
        if b.is_empty() {
            ui.secondary("No bands: every increase uses the normal text colour.");
        }
        ui.row(|ui| {
            if ui.button("+ Add band").clicked() {
                let next = b
                    .iter()
                    .filter_map(|(p, _)| p.trim().parse::<i64>().ok())
                    .max()
                    .map_or(20, |m| m + 20);
                b.push((next.to_string(), "#8E44AD".into()));
                commit = true;
                changed = true;
            }
            if ui.button("Restore defaults").clicked() {
                b = vec![
                    ("20".into(), "#F2C94C".into()),
                    ("40".into(), "#D64545".into()),
                ];
                commit = true;
                changed = true;
            }
        });
        ui.secondary("A price increase takes the colour of the highest band it reaches. Increases under the first band use the normal text colour; decreases are green and not banded. The chip's text is dark or light, whichever reads best on the colour.");
        ui.row(|ui| {
            ui.secondary("Examples:");
            ui.label("↑ 5.0% (under the first band)");
            ui.tone(Tone::Success).label("↓ 10.0% (a decrease)");
        });
        if let Some(e) = &app.subs.bands_error {
            ui.error_text(e.clone());
        }
    }
    if let Some(i) = remove {
        if i < b.len() {
            b.remove(i);
        }
        changed = true;
        commit = true;
    }
    if changed {
        app.subs.bands = Some((app.generation, b.clone()));
    }
    if commit {
        save_bands(app, &b);
    }
}

// ----- Home: the Subscriptions block --------------------------------------------------------

/// The Subscriptions dashboard block: monthly total, next three charges, open alerts.
pub(crate) fn block(app: &mut App, ui: &mut Ui, data: &Value) {
    let mut open: Option<Value> = None;
    ui.row(|ui| {
        ui.big_heading(fmt::group(data.str("monthly")));
        ui.secondary("a month");
    });
    for (i, n) in data.list("next").iter().enumerate() {
        ui.scope(format!("next-{i}"), |ui| {
            ui.row(|ui| {
                ui.label(fmt::date(n.str("date")));
                if ui.link(n.str("name")).clicked() {
                    open = Some(json!({"subscription": n.str("subscription")}));
                }
                ui.spacer();
                ui.amount(fmt::group(n.str("amount")));
            });
        });
    }
    if data.list("next").is_empty() {
        ui.secondary("No charges coming up.");
    }
    let count = data.at("alerts").as_u64().unwrap_or(0);
    let text = match count {
        0 => "No open alerts".to_owned(),
        1 => "1 open alert".to_owned(),
        n => format!("{n} open alerts"),
    };
    ui.row(|ui| {
        if count > 0 {
            ui.badge(text.clone(), Tone::Warning);
        } else {
            ui.secondary(text.clone());
        }
        ui.spacer();
        if ui.link("Open Subscriptions").clicked() {
            open = Some(json!({}));
        }
    });
    if let Some(t) = open {
        self::open(app, &t);
    }
}

/// Settings → Subscriptions → Make a bill (subscriptions.md revision 2).
fn make_bill_card(app: &mut App, ui: &mut Ui) {
    const CHOICES: [(&str, &str); 2] = [
        (
            "scheduled_bill",
            "A repeating bill in Scheduled (posts when you enter it)",
        ),
        ("bill_now", "A bill now (posts to Accounts Payable)"),
    ];
    let current = app
        .read("module.settings", json!({"module": MODULE}))
        .and_then(|s| {
            s.list("settings")
                .iter()
                .find(|d| d.str("key") == "subscriptions.make_bill_as")
                .map(|d| d.str("value").to_owned())
        })
        .unwrap_or_else(|| "scheduled_bill".to_owned());
    let mut chosen = current.clone();
    ui.card("Make a bill", |ui| {
        for (i, (value, label)) in CHOICES.into_iter().enumerate() {
            ui.form(if i == 0 { "Make a bill creates" } else { "" }, |ui| {
                ui.radio(&mut chosen, value.to_owned(), label);
            });
        }
        if !app.module_on("bills") {
            ui.caption("Make a bill appears on a subscription when Bills is on in Features.");
        } else {
            ui.caption("A repeating bill repeats on the subscription's cadence, is never entered automatically, and opens the bill editor when you enter it so you can correct the amount.");
        }
    });
    if chosen != current {
        super::settings::save_setting(app, "subscriptions.make_bill_as", json!(chosen));
    }
}
