//! Scheduled (scheduled-transactions.md): upcoming occurrences (Overdue / Due
//! soon / Later this month), all schedules, the schedule form (new, edit,
//! "Repeat this…"), the estimate form and delete.

use serde_json::{Value, json};

use super::form::set;
use super::tags::{Tags, TagsOn};
use super::ui::{Align, Column, Tone, Ui};
use super::util::{Json, array};
use super::{App, MessageAction};
use crate::fmt;

#[derive(Debug, Default)]
pub(crate) struct ScheduledState {
    pub all: bool,
    pub form: Option<ScheduleForm>,
    pub estimate: Option<EstimateForm>,
    pub confirm_delete: Option<(String, String)>,
    pub error: Option<String>,
}

pub(crate) const REPEATS: [&str; 8] = [
    "Once",
    "Every week",
    "Every 2 weeks",
    "Twice a month",
    "Every month",
    "Every 3 months",
    "Every year",
    "Custom…",
];

const FREQUENCIES: [(&str, &str); 5] = [
    ("daily", "Daily"),
    ("weekly", "Weekly"),
    ("semi_monthly", "Twice a month"),
    ("monthly", "Monthly"),
    ("yearly", "Yearly"),
];

const WEEKDAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];
const WEEKS: [&str; 5] = ["1", "2", "3", "4", "last"];
const ENDS: [&str; 3] = ["Never", "After N times", "On date"];
const WEEKEND: [(&str, &str); 3] = [
    ("none", "None"),
    ("before", "Friday before"),
    ("after", "Monday after"),
];

#[derive(Debug, Clone, Default)]
pub(crate) struct ScheduleForm {
    pub editing: Option<String>,
    pub from_txn: Option<String>,
    pub name: String,
    pub payee: String,
    pub account: String,
    pub category: String,
    pub amount: String,
    pub out: bool,
    pub memo: String,
    pub tags: Tags,
    pub multi: Option<Vec<Value>>,
    pub repeats: usize,
    pub frequency: usize,
    pub interval: String,
    pub day: String,
    pub by_weekday: bool,
    pub weekday: usize,
    pub week: usize,
    pub day2: String,
    pub start: String,
    pub end_kind: usize,
    pub end_after: String,
    pub end_on: String,
    pub weekend: usize,
    pub estimate: bool,
    pub auto: bool,
    pub remind: String,
    pub error: Option<String>,
    pub preview: Option<(String, String, Vec<String>)>,
    pub payee_sugg: Vec<Value>,
    pub payee_query: String,
    pub original: Option<Box<ScheduleForm>>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EstimateForm {
    pub schedule: String,
    pub nominal: String,
    pub payee: String,
    pub date: String,
    pub amount: String,
    pub memo: String,
    pub splits: Vec<Value>,
    pub error: Option<String>,
    pub focused: bool,
}

pub(crate) fn view(app: &mut App, ui: &mut Ui) {
    ui.title("Scheduled");
    ui.toolbar(|ui| {
        for (label, all) in [("Upcoming", false), ("All schedules", true)] {
            let selected = app.scheduled.all == all;
            if ui.tab(label, selected).clicked() {
                app.scheduled.all = all;
            }
        }
        if ui.primary("New schedule…").clicked() {
            app.scheduled.form = Some(new_form(app));
        }
    });
    if let Some(e) = app.scheduled.error.clone() {
        ui.error_text(e);
    }
    if app.scheduled.all {
        all_schedules(app, ui);
    } else {
        upcoming(app, ui);
    }
    dialogs(app, ui);
}

fn shift_days(date: &str, days: i32) -> String {
    date.parse::<mk_core::Date>()
        .ok()
        .and_then(|d| d.add_days(days))
        .map(|d| d.to_string())
        .unwrap_or_default()
}

/// Occurrences from `schedule.upcoming` for the Scheduled view and Home.
pub(crate) fn upcoming_rows(app: &mut App) -> Vec<Value> {
    let today = app.today();
    let to = shift_days(&today, 30);
    app.read("schedule.upcoming", json!({"today": today, "to": to}))
        .map(|v| v.list("occurrences").to_vec())
        .unwrap_or_default()
}

pub(crate) fn amount_text(o: &Value) -> String {
    let amount = fmt::group(o.at("money").str("amount"));
    let amount = if amount.is_empty() {
        fmt::group(o.str("amount"))
    } else {
        amount
    };
    if o.str("amount_kind") == "estimate" {
        format!("about {amount}")
    } else {
        amount
    }
}

fn upcoming(app: &mut App, ui: &mut Ui) {
    let rows = upcoming_rows(app);
    if rows.is_empty() {
        ui.card("", |ui| {
            ui.label("Nothing is scheduled in the next 30 days.")
        });
        return;
    }
    let soon_days: Vec<u64> = rows
        .iter()
        .filter(|r| r.str("when") == "due_soon")
        .filter_map(|r| r.at("remind_days").as_u64())
        .collect();
    let soon_title = match soon_days.first() {
        Some(n) if soon_days.iter().all(|x| x == n) => format!("Due in the next {n} days"),
        _ => "Due soon".to_owned(),
    };
    let mut header_done = false;
    for (when, title) in [
        ("overdue", "Overdue".to_owned()),
        ("due_soon", soon_title),
        ("later", "Later this month".to_owned()),
    ] {
        let group: Vec<Value> = rows
            .iter()
            .filter(|r| r.str("when") == when)
            .cloned()
            .collect();
        if group.is_empty() {
            continue;
        }
        // One header row for all groups (the later groups share its columns).
        let first = !std::mem::replace(&mut header_done, true);
        let t = |s: &str| if first { s.to_owned() } else { String::new() };
        let cols = vec![
            Column {
                title: t("Due"),
                width: 110,
                ..Column::default()
            },
            Column {
                title: t("Payee"),
                weight: 3,
                ..Column::default()
            },
            Column {
                title: t("Account"),
                weight: 2,
                ..Column::default()
            },
            Column {
                title: t("Category"),
                weight: 2,
                ..Column::default()
            },
            Column {
                title: t("Amount"),
                width: 120,
                align: Align::Right,
                ..Column::default()
            },
            Column {
                title: t("Repeats"),
                weight: 2,
                ..Column::default()
            },
            Column {
                title: t("Actions"),
                width: 200,
                align: Align::Center,
                ..Column::default()
            },
        ];
        ui.table(&title, cols, |ui| {
            for o in &group {
                upcoming_row(app, ui, o);
            }
        });
    }
}

fn upcoming_row(app: &mut App, ui: &mut Ui, o: &Value) {
    let payee = if o.str("payee").is_empty() {
        o.str("name")
    } else {
        o.str("payee")
    }
    .to_owned();
    let money = o.at("money");
    let amount = amount_text(o);
    let due = fmt::date(o.str("due"));
    let label = format!("Scheduled {payee} due {due}, {amount}, {}", o.str("when"));
    let (mut enter_it, mut skip_it, mut edit_it) = (false, false, false);
    let is_bill = o.str("creates") == "bill";
    ui.table_row(&label, false, false, |ui| {
        ui.label(due.clone());
        if is_bill {
            ui.row(|ui| {
                ui.tip(payee.clone()).named(label.clone()).grow();
                ui.label(payee.clone());
                ui.tip("Entering it opens the bill editor; the bill posts when you save it");
                ui.badge("Bill", Tone::Accent);
            });
        } else {
            ui.tip(payee.clone()).named(label.clone());
            ui.label(payee.clone());
        }
        ui.tip(money.at("account").str("full_name"));
        ui.label(money.at("account").str("full_name"));
        ui.tip(money.str("category"));
        ui.label(money.str("category"));
        ui.amount(amount.clone());
        ui.tip(o.str("summary"));
        ui.label(o.str("summary"));
        ui.row(|ui| {
            ui.named(format!("Enter {payee} due {due}"));
            enter_it = ui.button("Enter").clicked();
            ui.named(format!("Skip {payee} due {due}"));
            skip_it = ui.button("Skip").clicked();
            ui.named(format!("Edit {payee} due {due}"));
            edit_it = ui.button("Edit").clicked();
        });
    });
    if enter_it {
        enter(app, o);
    }
    if skip_it {
        skip(app, o.str("schedule"), o.str("nominal"));
    }
    if edit_it {
        edit(app, o.str("schedule"));
    }
}

/// Enter an occurrence: directly for a fixed amount, through the amount form for
/// an estimate, through the bill editor for a schedule that creates a bill.
pub(crate) fn enter(app: &mut App, o: &Value) {
    if o.str("creates") == "bill" {
        enter_bill(app, o.str("schedule"), o.str("nominal"), o.str("due"));
        return;
    }
    if o.str("amount_kind") == "estimate" {
        enter_estimate(app, o);
        return;
    }
    match app.run(
        "schedule.enter",
        json!({"schedule": o.str("schedule"), "date": o.str("nominal")}),
    ) {
        Ok(t) => app.notify(format!(
            "Entered {} on {}",
            o.str("name"),
            fmt::date(t.str("date"))
        )),
        Err(e) => app.scheduled.error = Some(e.display()),
    }
}

/// A bill schedule's occurrence (subscriptions.md revision 2): the bill editor
/// pre-filled for that due date; saving goes through `bill.create`.
pub(crate) fn enter_bill(app: &mut App, schedule: &str, nominal: &str, due: &str) {
    let Some(s) = app.read("schedule.get", json!({"schedule": schedule})) else {
        return;
    };
    let b = s.at("bill_view");
    let lines: Vec<(String, String, String)> = b
        .list("lines")
        .iter()
        .map(|l| {
            (
                l.str("account_name").to_owned(),
                l.str("description").to_owned(),
                l.str("amount").to_owned(),
            )
        })
        .collect();
    super::purchases::new_bill_from_schedule(
        app,
        b.str("vendor_name"),
        &lines,
        due,
        b.str("memo"),
        (schedule.to_owned(), nominal.to_owned()),
    );
    app.navigate(super::View::Purchases);
}

/// The amount form for an estimate occurrence.
pub(crate) fn enter_estimate(app: &mut App, o: &Value) {
    let schedule = o.str("schedule").to_owned();
    let Some(s) = app.read("schedule.get", json!({"schedule": schedule})) else {
        return;
    };
    let amount = o
        .at("money")
        .str("amount")
        .trim_start_matches('-')
        .to_owned();
    app.scheduled.estimate = Some(EstimateForm {
        schedule,
        nominal: o.str("nominal").to_owned(),
        payee: o.str("payee").to_owned(),
        date: fmt::date(o.str("due")),
        amount: if amount.is_empty() {
            o.str("amount").trim_start_matches('-').to_owned()
        } else {
            amount
        },
        memo: s.at("template").str("memo").to_owned(),
        splits: s.at("template").list("splits").to_vec(),
        error: None,
        focused: false,
    });
}

pub(crate) fn skip(app: &mut App, schedule: &str, nominal: &str) {
    match app.run(
        "schedule.skip",
        json!({"schedule": schedule, "date": nominal}),
    ) {
        Ok(_) => app.notify_with("Skipped —", MessageAction::Undo),
        Err(e) => app.scheduled.error = Some(e.display()),
    }
}

fn all_schedules(app: &mut App, ui: &mut Ui) {
    let list = array(app.read("schedule.list", json!({})));
    if list.is_empty() {
        ui.card("", |ui| {
            ui.label("No schedules yet. Use New schedule… or Repeat this… on a register row.")
        });
        return;
    }
    let cols = vec![
        Column {
            title: "Name".into(),
            weight: 3,
            ..Column::default()
        },
        Column {
            title: "Account".into(),
            weight: 2,
            ..Column::default()
        },
        Column {
            title: "Amount".into(),
            width: 120,
            align: Align::Right,
            ..Column::default()
        },
        Column {
            title: "Repeats".into(),
            weight: 2,
            ..Column::default()
        },
        Column {
            title: "Next due".into(),
            width: 110,
            ..Column::default()
        },
        Column {
            title: "Status".into(),
            width: 80,
            ..Column::default()
        },
        Column {
            title: "Actions".into(),
            width: 230,
            align: Align::Center,
            ..Column::default()
        },
    ];
    let mut action: Option<(String, String, &str)> = None;
    ui.table("All schedules", cols, |ui| {
        for s in &list {
            let name = s.str("name").to_owned();
            let id = s.str("id").to_owned();
            let paused = s.str("status") == "paused";
            let is_bill = s.str("creates") == "bill";
            ui.table_row(&format!("schedule:{id}"), false, false, |ui| {
                ui.row(|ui| {
                    ui.tip(name.clone()).grow();
                    ui.label(name.clone());
                    if is_bill {
                        ui.tip(
                            "Entering it opens the bill editor; the bill posts when you save it",
                        );
                        ui.badge("Bill", Tone::Accent);
                    }
                });
                ui.tip(s.at("account").str("full_name"));
                ui.label(s.at("account").str("full_name"));
                ui.amount(fmt::group(s.str("amount")));
                ui.tip(s.str("summary"));
                ui.label(s.str("summary"));
                let next = s.at("next_due").str("due");
                ui.label(if next.is_empty() {
                    "Ended".to_owned()
                } else {
                    fmt::date(next)
                });
                ui.label(if paused { "Paused" } else { "Active" });
                ui.row(|ui| {
                    let label = if paused { "Resume" } else { "Pause" };
                    ui.named(format!("{label} {name}"));
                    if ui.button(label).clicked() {
                        action = Some((
                            id.clone(),
                            name.clone(),
                            if paused {
                                "schedule.resume"
                            } else {
                                "schedule.pause"
                            },
                        ));
                    }
                    ui.named(format!("Edit {name}"));
                    if ui.button("Edit").clicked() {
                        action = Some((id.clone(), name.clone(), "edit"));
                    }
                    ui.named(format!("Delete {name}"));
                    if ui.button("Delete").clicked() {
                        action = Some((id.clone(), name.clone(), "delete"));
                    }
                });
            });
        }
    });
    match action {
        Some((id, _, "edit")) => edit(app, &id),
        Some((id, name, "delete")) => app.scheduled.confirm_delete = Some((id, name)),
        Some((id, _, cmd)) => {
            if let Err(e) = app.run(cmd, json!({"schedule": id})) {
                app.scheduled.error = Some(e.display());
            }
        }
        None => {}
    }
}

fn new_form(app: &App) -> ScheduleForm {
    ScheduleForm {
        out: true,
        repeats: 4,
        frequency: 3,
        interval: "1".into(),
        start: fmt::date(&app.today()),
        remind: "7".into(),
        end_after: "12".into(),
        ..ScheduleForm::default()
    }
}

pub(crate) fn edit(app: &mut App, id: &str) {
    let Some(s) = app.read("schedule.get", json!({"schedule": id})) else {
        return;
    };
    let mut f = new_form(app);
    f.editing = Some(id.to_owned());
    f.name = s.str("name").to_owned();
    f.start = fmt::date(s.str("start"));
    fill_template(app, &mut f, s.at("template"));
    fill_recurrence(&mut f, s.at("recurrence"));
    match s.at("end") {
        Value::Object(o) if o.contains_key("after") => {
            f.end_kind = 1;
            f.end_after = s.at("end").at("after").to_string();
        }
        Value::Object(o) if o.contains_key("on") => {
            f.end_kind = 2;
            f.end_on = fmt::date(s.at("end").str("on"));
        }
        _ => f.end_kind = 0,
    }
    f.weekend = WEEKEND
        .iter()
        .position(|(k, _)| *k == s.str("weekend"))
        .unwrap_or(0);
    f.estimate = s.str("amount_kind") == "estimate";
    f.auto = s.at("auto_enter").as_bool() == Some(true);
    f.remind = s.at("remind_days").to_string();
    app.scheduled.form = Some(f);
}

pub(crate) fn repeat_txn(app: &mut App, txn: &str) {
    let Some(t) = app.read("txn.get", json!({"id": txn})) else {
        return;
    };
    let mut f = new_form(app);
    f.from_txn = Some(txn.to_owned());
    f.name = t.str("payee").to_owned();
    fill_template(app, &mut f, &t);
    if let Ok(p) = app.run(
        "schedule.preview",
        json!({"recurrence": {"frequency": "monthly"}, "after": t.str("date")}),
    ) {
        f.start = fmt::date(p.str("start"));
    }
    f.original = Some(Box::new(f.clone()));
    app.scheduled.form = Some(f);
}

/// Make a reminder (subscriptions.md): the schedule form filled from the
/// `schedule.create` arguments a subscription gives, never entered automatically.
pub(crate) fn from_subscription(app: &mut App, args: &Value) {
    if args.is_null() {
        return;
    }
    let mut f = new_form(app);
    f.name = args.str("name").to_owned();
    fill_template(app, &mut f, args.at("template"));
    fill_recurrence(&mut f, args.at("recurrence"));
    f.start = fmt::date(args.str("start"));
    f.auto = false;
    f.estimate = args.str("amount_kind") == "estimate";
    app.scheduled.form = Some(f);
}

fn is_money_type(t: &str) -> bool {
    matches!(t, "asset" | "liability")
}

fn fill_template(app: &mut App, f: &mut ScheduleForm, t: &Value) {
    let accounts = array(app.read("account.list", json!({"include_archived": true})));
    let find = |id: &str| accounts.iter().find(|a| a.str("id") == id);
    let type_of = |id: &str| {
        find(id)
            .map(|a| a.str("type").to_owned())
            .unwrap_or_default()
    };
    let name_of = |id: &str| {
        find(id)
            .map(|a| a.str("full_name").to_owned())
            .unwrap_or_default()
    };
    f.payee = t.str("payee").to_owned();
    f.memo = t.str("memo").to_owned();
    let splits = t.list("splits");
    let money = splits
        .iter()
        .find(|s| is_money_type(&type_of(s.str("account"))));
    if let Some(m) = money {
        f.account = name_of(m.str("account"));
        let v = m.str("value");
        f.out = v.starts_with('-');
        f.amount = v.trim_start_matches('-').to_owned();
    }
    let others: Vec<&Value> = splits
        .iter()
        .filter(|s| money.is_none_or(|m| !std::ptr::eq(*s, m)))
        .collect();
    match others.as_slice() {
        [one] => {
            f.category = name_of(one.str("account"));
            f.tags = Tags::from_ids(one, app);
        }
        _ => {
            f.multi = Some(splits.to_vec());
            f.category = format!("Split ({})", others.len());
        }
    }
}

fn fill_recurrence(f: &mut ScheduleForm, r: &Value) {
    let freq = r.str("frequency");
    let interval = r.at("interval").as_u64().unwrap_or(1);
    let plain = r.at("day").is_null() && r.at("weekday").is_null() && r.at("month").is_null();
    f.repeats = match (freq, interval) {
        ("once", _) => 0,
        ("weekly", 1) => 1,
        ("weekly", 2) => 2,
        ("semi_monthly", _) if r.at("days") == &json!([1, 15]) => 3,
        ("monthly", 1) if plain => 4,
        ("monthly", 3) if plain => 5,
        ("yearly", 1) if plain => 6,
        _ => 7,
    };
    f.frequency = FREQUENCIES
        .iter()
        .position(|(k, _)| *k == freq)
        .unwrap_or(3);
    f.interval = interval.to_string();
    let day_text = |v: &Value| match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    };
    f.day = day_text(r.at("day"));
    if let Some(days) = r.at("days").as_array() {
        f.day = days.first().map(day_text).unwrap_or_default();
        f.day2 = days.get(1).map(day_text).unwrap_or_default();
    }
    f.by_weekday = !r.at("weekday").is_null();
    f.weekday = WEEKDAYS
        .iter()
        .position(|w| *w == r.str("weekday"))
        .unwrap_or(0);
    f.week = WEEKS
        .iter()
        .position(|w| *w == day_text(r.at("week")))
        .unwrap_or(0);
}

fn day_value(s: &str) -> Value {
    let t = s.trim();
    match t.parse::<u64>() {
        Ok(n) => json!(n),
        Err(_) if t.is_empty() => Value::Null,
        Err(_) => json!(t.to_lowercase()),
    }
}

fn recurrence(f: &ScheduleForm) -> Value {
    match f.repeats {
        0 => json!({"frequency": "once"}),
        1 => json!({"frequency": "weekly"}),
        2 => json!({"frequency": "weekly", "interval": 2}),
        3 => json!({"frequency": "semi_monthly", "days": [1, 15]}),
        4 => json!({"frequency": "monthly"}),
        5 => json!({"frequency": "monthly", "interval": 3}),
        6 => json!({"frequency": "yearly"}),
        _ => {
            let freq = FREQUENCIES.get(f.frequency).map_or("monthly", |x| x.0);
            let mut r = json!({"frequency": freq});
            let interval = f.interval.trim().parse::<u64>().unwrap_or(1);
            if matches!(freq, "daily" | "weekly" | "monthly" | "yearly") && interval != 1 {
                set(&mut r, "interval", json!(interval));
            }
            match freq {
                "semi_monthly" => set(
                    &mut r,
                    "days",
                    json!([day_value(&f.day), day_value(&f.day2)]),
                ),
                "monthly" if f.by_weekday => {
                    set(
                        &mut r,
                        "weekday",
                        json!(WEEKDAYS.get(f.weekday).copied().unwrap_or("monday")),
                    );
                    set(
                        &mut r,
                        "week",
                        day_value(WEEKS.get(f.week).copied().unwrap_or("1")),
                    );
                }
                "monthly" | "yearly" => {
                    let d = day_value(&f.day);
                    if !d.is_null() {
                        set(&mut r, "day", d);
                    }
                }
                _ => {}
            }
            r
        }
    }
}

fn end_value(f: &ScheduleForm) -> Value {
    match f.end_kind {
        1 => json!({"after": f.end_after.trim().parse::<u64>().unwrap_or(0)}),
        2 => json!({"on": fmt::date_arg_lenient(&f.end_on)}),
        _ => json!("never"),
    }
}

fn weekend_value(f: &ScheduleForm) -> &'static str {
    WEEKEND.get(f.weekend).map_or("none", |x| x.0)
}

fn schedule_args(app: &mut App, f: &ScheduleForm) -> Result<Value, String> {
    let template = match &f.multi {
        Some(splits) => json!({
            "payee": f.payee.trim(), "memo": f.memo.trim(),
            "splits": splits.iter().map(|s| json!({"account": s.str("account"), "value": s.str("value"),
                "memo": s.at("memo"), "class": s.at("class"), "location": s.at("location"), "fund": s.at("fund")})).collect::<Vec<_>>(),
        }),
        None => {
            let amount = f.amount.trim().replace(',', "");
            if amount.is_empty() {
                return Err("Enter an amount.".into());
            }
            if f.account.trim().is_empty() || f.category.trim().is_empty() {
                return Err("Choose an account and a category.".into());
            }
            let neg = format!("-{amount}");
            let (money, other) = if f.out {
                (neg, amount.clone())
            } else {
                (amount.clone(), neg)
            };
            let mut cat = json!({"account": f.category.trim(), "value": other});
            let on = super::tags::options(app);
            if let Some(m) = super::tags::missing(&on, &f.tags) {
                return Err(m);
            }
            f.tags.apply_given(&mut cat, &on);
            json!({
                "payee": f.payee.trim(), "memo": f.memo.trim(),
                "splits": [cat, {"account": f.account.trim(), "value": money}],
            })
        }
    };
    Ok(json!({
        "name": f.name.trim(),
        "template": template,
        "recurrence": recurrence(f),
        "start": fmt::date_arg_lenient(&f.start),
        "end": end_value(f),
        "weekend": weekend_value(f),
        "amount_kind": if f.estimate { "estimate" } else { "fixed" },
        "auto_enter": f.auto,
        "remind_days": f.remind.trim().parse::<u64>().unwrap_or(7),
    }))
}

fn refresh_preview(app: &mut App, f: &mut ScheduleForm) {
    let args = json!({
        "recurrence": recurrence(f),
        "start": fmt::date_arg_lenient(&f.start),
        "end": end_value(f),
        "weekend": weekend_value(f),
        "count": 3,
    });
    let key = args.to_string();
    if f.preview.as_ref().is_some_and(|p| p.0 == key) {
        return;
    }
    let (summary, next) = match app.backend.invoke("schedule.preview", args) {
        Ok(p) => (
            p.str("summary").to_owned(),
            p.list("next")
                .iter()
                .map(|d| d.str("due").to_owned())
                .collect(),
        ),
        Err(e) => (e.display(), Vec::new()),
    };
    f.preview = Some((key, summary, next));
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn form_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.scheduled.form.take() else {
        return;
    };
    refresh_preview(app, &mut f);
    let tags_on: TagsOn = super::tags::options(app);
    if f.payee != f.payee_query {
        f.payee_query = f.payee.clone();
        f.payee_sugg = if f.payee.trim().is_empty() {
            Vec::new()
        } else {
            array(
                app.backend
                    .invoke(
                        "payee.suggest",
                        json!({"prefix": f.payee.trim(), "limit": 5}),
                    )
                    .ok(),
            )
            .into_iter()
            .filter(|s| s.str("payee") != f.payee)
            .collect()
        };
    }
    let accounts = array(app.backend.invoke("account.list", json!({})).ok());
    let usable: Vec<&Value> = accounts
        .iter()
        .filter(|a| a.at("placeholder").as_bool() != Some(true))
        .collect();
    let money: Vec<String> = usable
        .iter()
        .filter(|a| is_money_type(a.str("type")))
        .map(|a| a.str("full_name").to_owned())
        .collect();
    let categories: Vec<String> = usable
        .iter()
        .map(|a| a.str("full_name").to_owned())
        .filter(|n| *n != f.account)
        .collect();
    let (out_label, in_label, _) = accounts
        .iter()
        .find(|a| a.str("full_name") == f.account)
        .map_or(("Spent", "Received", false), |a| {
            super::register::directions(a.str("type"), a.str("subtype"))
        });
    let mut save = false;
    let mut cancel = ui.dialog_closed("schedule-form");
    let title = if f.editing.is_some() {
        "Edit schedule"
    } else {
        "New schedule"
    };
    ui.dialog("schedule-form", title, 600, |ui| {
        ui.group(|ui| {
            ui.form("Payee", |ui| {
                ui.width(260);
                ui.field("Schedule payee", &mut f.payee);
            });
            for sugg in f.payee_sugg.clone() {
                let p = sugg.str("payee").to_owned();
                ui.form("", |ui| {
                    ui.tip("Use this payee");
                    if ui.link(p.clone()).clicked() {
                        f.payee = p.clone();
                        f.payee_query = p.clone();
                        f.payee_sugg.clear();
                        if f.category.is_empty() {
                            f.category = sugg.at("other").str("full_name").to_owned();
                        }
                        if f.amount.is_empty() {
                            f.amount = sugg.str("amount").trim_start_matches('-').to_owned();
                        }
                    }
                });
            }
            ui.form("Name", |ui| {
                ui.width(260);
                ui.field("Schedule name", &mut f.name);
            });
            ui.form("Account", |ui| {
                super::form::pick(ui, "Schedule account", &mut f.account, &money, None);
            });
            ui.form("Category", |ui| {
                if f.multi.is_some() {
                    ui.tip("The schedule keeps its split lines; edit them in the register.");
                    ui.label(f.category.clone());
                } else {
                    super::form::pick(ui, "Schedule category", &mut f.category, &categories, None);
                }
            });
            ui.form("Amount", |ui| {
                ui.enabled(f.multi.is_none()).width(120).hint("0.00").mono();
                ui.field("Schedule amount", &mut f.amount);
                ui.radio(&mut f.out, true, out_label);
                ui.radio(&mut f.out, false, in_label);
            });
            ui.form("Memo", |ui| {
                ui.width(260);
                ui.field("Schedule memo", &mut f.memo);
            });
            if f.multi.is_none() {
                super::tags::rows(ui, &tags_on, &mut f.tags, "Schedule");
            }
            ui.form("Repeats", |ui| {
                ui.width(220);
                ui.combo("Repeats", &mut f.repeats, &strs(&REPEATS));
            });
            if f.repeats == 7 {
                let names: Vec<String> = FREQUENCIES.iter().map(|x| x.1.to_owned()).collect();
                ui.form("Frequency", |ui| {
                    ui.width(220);
                    ui.combo("Frequency", &mut f.frequency, &names);
                });
                let freq = FREQUENCIES.get(f.frequency).map_or("monthly", |x| x.0);
                if freq != "semi_monthly" {
                    ui.form("Every", |ui| {
                        ui.width(64);
                        ui.field("Interval", &mut f.interval);
                    });
                }
                match freq {
                    "semi_monthly" => {
                        ui.form("Days", |ui| {
                            ui.width(64);
                            ui.field("First day", &mut f.day);
                            ui.width(64);
                            ui.field("Second day", &mut f.day2);
                        });
                    }
                    "monthly" => {
                        ui.form("On", |ui| {
                            ui.radio(&mut f.by_weekday, false, "Day of month");
                            ui.radio(&mut f.by_weekday, true, "Weekday");
                        });
                        if f.by_weekday {
                            ui.form("Week", |ui| {
                                ui.width(100);
                                ui.combo("Week of month", &mut f.week, &strs(&WEEKS));
                                ui.width(140);
                                ui.combo("Weekday", &mut f.weekday, &strs(&WEEKDAYS));
                            });
                        } else {
                            ui.form("Day", |ui| {
                                ui.width(64);
                                ui.field("Day of month", &mut f.day);
                            });
                        }
                    }
                    "yearly" => {
                        ui.form("Day", |ui| {
                            ui.width(64);
                            ui.field("Day of month", &mut f.day);
                        });
                    }
                    _ => {}
                }
            }
            ui.form("Start date", |ui| {
                super::register::date_field(ui, "Start date", &mut f.start);
            });
            ui.form("Ends", |ui| {
                ui.width(160);
                ui.combo("Ends", &mut f.end_kind, &strs(&ENDS));
                match f.end_kind {
                    1 => {
                        ui.width(64);
                        ui.field("End after times", &mut f.end_after);
                    }
                    2 => {
                        super::register::date_field(ui, "End date", &mut f.end_on);
                    }
                    _ => {}
                }
            });
            let names: Vec<String> = WEEKEND.iter().map(|x| x.1.to_owned()).collect();
            ui.form("If it falls on a weekend", |ui| {
                ui.width(180);
                ui.combo("If it falls on a weekend", &mut f.weekend, &names);
            });
            ui.form("", |ui| {
                ui.checkbox(&mut f.estimate, "Amount changes each time");
            });
            ui.form("", |ui| {
                ui.checkbox(&mut f.auto, "Enter automatically");
            });
            ui.form("Remind me", |ui| {
                ui.width(56);
                ui.field("Remind me days before", &mut f.remind);
                ui.label("days before");
            });
        });
        if let Some((_, summary, next)) = &f.preview {
            ui.card("", |ui| {
                ui.next().bold();
                ui.label(summary.clone());
                if !next.is_empty() {
                    ui.wrap(|ui| {
                        ui.label("Next:");
                        for d in next {
                            ui.label(fmt::date(d));
                        }
                    });
                }
            });
        }
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Save schedule").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if save {
        match save_form(app, &f) {
            Ok(name) => {
                app.notify(format!("Saved schedule {name}"));
                return;
            }
            Err(e) => f.error = Some(e),
        }
    }
    app.scheduled.form = Some(f);
}

fn unchanged_template(f: &ScheduleForm) -> bool {
    f.original.as_ref().is_some_and(|o| {
        o.payee == f.payee
            && o.account == f.account
            && o.category == f.category
            && o.amount == f.amount
            && o.out == f.out
            && o.memo == f.memo
    })
}

fn save_form(app: &mut App, f: &ScheduleForm) -> Result<String, String> {
    let mut args = schedule_args(app, f)?;
    if f.name.trim().is_empty()
        && let Some(o) = args.as_object_mut()
    {
        o.remove("name");
    }
    let result = match (&f.editing, &f.from_txn) {
        (Some(id), _) => {
            set(&mut args, "schedule", json!(id));
            app.run("schedule.update", args)
        }
        (None, Some(txn)) if unchanged_template(f) => {
            let mut a = json!({
                "txn": txn,
                "recurrence": args.at("recurrence").clone(),
                "start": args.at("start").clone(),
                "end": args.at("end").clone(),
                "weekend": args.at("weekend").clone(),
                "amount_kind": args.at("amount_kind").clone(),
                "auto_enter": args.at("auto_enter").clone(),
                "remind_days": args.at("remind_days").clone(),
            });
            if !f.name.trim().is_empty() {
                set(&mut a, "name", json!(f.name.trim()));
            }
            app.run("schedule.from_txn", a)
        }
        _ => app.run("schedule.create", args),
    };
    result
        .map(|s| s.str("name").to_owned())
        .map_err(|e| e.display())
}

fn estimate_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.scheduled.estimate.take() else {
        return;
    };
    let mut confirm = false;
    let mut cancel = ui.dialog_closed("estimate-form");
    ui.dialog("estimate-form", &format!("Enter {}", f.payee), 460, |ui| {
        ui.group(|ui| {
            ui.label("This amount changes each time. Check it against the bill.");
            ui.form("Date", |ui| {
                super::register::date_field(ui, "Entry date", &mut f.date);
            });
            ui.form("Amount", |ui| {
                ui.width(120).mono();
                if !f.focused {
                    ui.focus().select_all();
                    f.focused = true;
                }
                ui.field("Actual amount", &mut f.amount);
            });
            ui.form("Memo", |ui| {
                ui.width(260);
                ui.field("Entry memo", &mut f.memo);
            });
            if let Some(e) = &f.error {
                ui.error_text(e.clone());
            }
        });
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary("Enter transaction").clicked() {
                confirm = true;
            }
        });
    });
    if cancel {
        return;
    }
    if confirm {
        let amount = f.amount.trim().replace(',', "");
        let mut args = json!({
            "schedule": f.schedule, "date": f.nominal,
            "entry_date": fmt::date_arg_lenient(&f.date), "memo": f.memo.trim(),
        });
        if f.splits.len() == 2 {
            let splits: Vec<Value> = f
                .splits
                .iter()
                .map(|s| {
                    let v = if s.str("value").starts_with('-') {
                        format!("-{amount}")
                    } else {
                        amount.clone()
                    };
                    json!({"account": s.str("account"), "value": v})
                })
                .collect();
            set(&mut args, "splits", json!(splits));
        }
        match app.run("schedule.enter", args) {
            Ok(t) => {
                app.notify(format!(
                    "Entered {} on {}",
                    f.payee,
                    fmt::date(t.str("date"))
                ));
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.scheduled.estimate = Some(f);
}

fn delete_dialog(app: &mut App, ui: &mut Ui) {
    let Some((id, name)) = app.scheduled.confirm_delete.clone() else {
        return;
    };
    let mut close = ui.dialog_closed("delete-schedule");
    ui.dialog(
        "delete-schedule",
        &format!("Delete schedule {name}?"),
        440,
        |ui| {
            ui.label("Transactions already entered from it stay in the books.");
            ui.actions(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.primary("Delete schedule").clicked() {
                    if let Err(e) = app.run("schedule.delete", json!({"schedule": id})) {
                        app.scheduled.error = Some(e.display());
                    }
                    close = true;
                }
            });
        },
    );
    if close {
        app.scheduled.confirm_delete = None;
    }
}

/// The scheduled dialogs (also shown over the register: Repeat this…, estimates).
pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    form_dialog(app, ui);
    estimate_dialog(app, ui);
    delete_dialog(app, ui);
}
