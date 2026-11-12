//! The transaction pop-up (app-desktop.md revisions 9, 24 and 27): New
//! transaction and Edit transaction, one field per line, payee and category
//! suggestions, the split editor, fast entry, Delete/Void/Unvoid.
//!
//! Saving goes through `txn.create` / `txn.update`, then `txn.set_reconcile`
//! when the Status asked for differs from the saved one.

use mk_core::Decimal;
use serde_json::{Value, json};

use super::App;
use super::register::{scale, titles, to_raw};
use super::ui::{Tone, Ui};
use super::util::{Json, array};
use crate::AppEvent;
use crate::fmt;

/// The pop-up's transaction fields, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Date,
    Number,
    Payee,
    Category,
    Memo,
    Decrease,
    Increase,
}

impl Field {
    fn key(self) -> &'static str {
        match self {
            Field::Date => "date",
            Field::Number => "number",
            Field::Payee => "payee",
            Field::Category => "category",
            Field::Memo => "memo",
            Field::Decrease => "decrease",
            Field::Increase => "increase",
        }
    }
    /// The item id of the field in the pop-up.
    pub(crate) fn id(self) -> String {
        format!("dialog:txn/popup:{}", self.key())
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Entry {
    pub date: String,
    pub number: String,
    pub payee: String,
    pub category: String,
    pub memo: String,
    pub decrease: String,
    pub increase: String,
    pub editing: Option<String>,
    pub error: Option<String>,
    pub error_field: Option<Field>,
    pub payee_sugg: Vec<Value>,
    pub cat_sugg: Vec<Value>,
    pub tag_names: Value,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SplitLine {
    pub category: String,
    pub memo: String,
    pub amount: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SplitEditor {
    pub lines: Vec<SplitLine>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Mode {
    #[default]
    New,
    Edit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Status {
    #[default]
    Uncleared,
    Cleared,
    Reconciled,
}

impl Status {
    fn of(state: &str) -> Self {
        match state {
            "c" => Self::Cleared,
            "r" => Self::Reconciled,
            _ => Self::Uncleared,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Popup {
    pub mode: Mode,
    pub entry: Entry,
    pub lines: Option<Vec<SplitLine>>,
    pub status: Status,
    pub voided: bool,
    pub row: Option<usize>,
    pub confirm_delete: bool,
    pub confirm_discard: bool,
    /// Focus (and select) this field when the pop-up is next built.
    pub focus: Option<Field>,
    pub added: usize,
    pub split: Option<SplitEditor>,
    pub receipts_pending: Vec<String>,
    /// The category split's class / location / fund (revision 16).
    pub tags: super::tags::Tags,
    pub tags_on: super::tags::TagsOn,
    /// A suggestion was just taken (the Enter that follows moves on, not saves).
    accepted: bool,
}

impl Popup {
    fn reconciled(&self) -> bool {
        self.status == Status::Reconciled
    }
    fn editable(&self, f: Field) -> bool {
        !self.voided
            && !(self.reconciled() && matches!(f, Field::Date | Field::Decrease | Field::Increase))
    }
    fn splits_editable(&self) -> bool {
        !self.voided && !self.reconciled()
    }
    fn partly_filled(&self) -> bool {
        let e = &self.entry;
        [
            &e.number,
            &e.payee,
            &e.category,
            &e.memo,
            &e.decrease,
            &e.increase,
        ]
        .iter()
        .any(|t| !t.trim().is_empty())
            || self.lines.is_some()
    }
}

fn field_for_column(col: usize, first_inc: bool) -> Field {
    let (first, second) = if first_inc {
        (Field::Increase, Field::Decrease)
    } else {
        (Field::Decrease, Field::Increase)
    };
    match col {
        3 => Field::Category,
        4 => Field::Memo,
        6 => first,
        7 => second,
        _ => Field::Payee,
    }
}

/// Open row `i` in Edit transaction with focus in the field of column `col`.
pub(crate) fn open(app: &mut App, i: usize, r: &Value, col: usize) {
    let Some((e, lines)) = super::register::load_entry(app, r) else {
        return;
    };
    let first_inc = titles(&app.register.header).2;
    let mut p = Popup {
        mode: Mode::Edit,
        lines,
        status: Status::of(r.str("reconcile")),
        voided: r.str("status") == "voided",
        row: Some(i),
        entry: e,
        ..Popup::default()
    };
    p.tags_on = super::tags::options(app);
    if p.tags_on.any() && p.lines.is_none() {
        p.tags = category_tags(app, r);
    }
    let mut f = field_for_column(col, first_inc);
    if (p.lines.is_some() && f == Field::Category) || !p.editable(f) {
        f = Field::Payee;
    }
    p.focus = (!p.voided).then_some(f);
    app.register.selected = Some(i);
    app.register.popup = Some(p);
}

/// Open New transaction: dated like the last one saved, else today; focus in
/// Date with its text selected.
pub(crate) fn open_new(app: &mut App) {
    let date = app
        .register
        .last_date
        .clone()
        .unwrap_or_else(|| app.today());
    let mut p = new_form(&date, 0);
    p.tags_on = super::tags::options(app);
    app.register.popup = Some(p);
}

/// The tags of the transaction's other (category) split.
fn category_tags(app: &mut App, r: &Value) -> super::tags::Tags {
    let account = app.register.header.at("account").str("id").to_owned();
    app.read("txn.get", json!({"id": r.str("txn")}))
        .and_then(|t| {
            t.list("splits")
                .iter()
                .find(|s| s.str("account") != account)
                .map(|s| super::tags::Tags::from_names(s.at("tag_names")))
        })
        .unwrap_or_default()
}

/// The receipts the pop-up shows (`None` while the expenses module is off).
fn receipt_items(app: &mut App, p: &Popup) -> Option<Vec<Value>> {
    if !app.module_on("expenses") {
        return None;
    }
    let mut list: Vec<Value> = match &p.entry.editing {
        Some(t) => super::util::array(
            app.backend
                .invoke("receipt.list", json!({"transaction": t}))
                .ok(),
        ),
        None => Vec::new(),
    };
    if !p.receipts_pending.is_empty() {
        list.extend(
            super::expenses::inbox(app)
                .into_iter()
                .filter(|r| p.receipts_pending.iter().any(|x| x == r.str("id"))),
        );
    }
    Some(list)
}

fn new_form(iso: &str, added: usize) -> Popup {
    Popup {
        mode: Mode::New,
        entry: Entry {
            date: fmt::date(iso),
            ..Entry::default()
        },
        focus: Some(Field::Date),
        added,
        ..Popup::default()
    }
}

pub(crate) fn open_split_editor(app: &mut App) {
    if let Some(p) = app.register.popup.as_mut() {
        start_split(p);
    }
}

fn start_split(p: &mut Popup) {
    if p.split.is_some() || !p.splits_editable() {
        return;
    }
    let lines = p.lines.clone().unwrap_or_else(|| {
        let e = &p.entry;
        let first = SplitLine {
            category: e.category.clone(),
            memo: String::new(),
            amount: if e.decrease.is_empty() {
                e.increase.clone()
            } else {
                e.decrease.clone()
            },
        };
        vec![first, SplitLine::default()]
    });
    p.split = Some(SplitEditor { lines, error: None });
}

fn first_amount_field(header: &Value) -> Field {
    if titles(header).2 {
        Field::Increase
    } else {
        Field::Decrease
    }
}

fn entry_amount(e: &Entry) -> Result<Option<Decimal>, String> {
    let dec = fmt::parse_entry(&e.decrease)?;
    let inc = fmt::parse_entry(&e.increase)?;
    Ok(match (dec, inc) {
        (Some(d), _) => Some(-d.abs()),
        (None, Some(i)) => Some(i.abs()),
        (None, None) => None,
    })
}

fn entry_text(e: &mut Entry, f: Field) -> &mut String {
    match f {
        Field::Date => &mut e.date,
        Field::Number => &mut e.number,
        Field::Payee => &mut e.payee,
        Field::Category => &mut e.category,
        Field::Memo => &mut e.memo,
        Field::Decrease => &mut e.decrease,
        Field::Increase => &mut e.increase,
    }
}

fn accept_payee(e: &mut Entry, s: &Value) {
    e.payee = s.str("payee").to_owned();
    if !s.at("other").is_null() {
        e.category = s.at("other").str("full_name").to_owned();
    }
    if let Some(amount) = s
        .at("amount")
        .as_str()
        .and_then(|a| a.parse::<Decimal>().ok())
    {
        if amount.is_sign_negative() {
            e.decrease = (-amount).to_string();
            e.increase.clear();
        } else {
            e.increase = amount.to_string();
            e.decrease.clear();
        }
    }
    e.payee_sugg.clear();
    e.cat_sugg.clear();
}

pub(crate) type FieldError = (String, Option<Field>);

fn txn_args(
    header: &Value,
    e: &Entry,
    lines: Option<&[SplitLine]>,
) -> Result<(&'static str, Value), FieldError> {
    let account = header.at("account").str("id").to_owned();
    let display = match entry_amount(e) {
        Ok(Some(d)) => d,
        Ok(None) => return Err(("Enter an amount.".into(), Some(first_amount_field(header)))),
        Err(m) => return Err((m, Some(first_amount_field(header)))),
    };
    let raw = to_raw(header, display);
    let mut splits = vec![json!({"account": account, "value": raw.to_string()})];
    match lines {
        Some(lines) => {
            let sign = if display.is_sign_negative() {
                -Decimal::ONE
            } else {
                Decimal::ONE
            };
            for l in lines
                .iter()
                .filter(|l| !l.amount.trim().is_empty() || !l.category.trim().is_empty())
            {
                let amt = match fmt::parse_entry(&l.amount) {
                    Ok(Some(a)) => a,
                    Ok(None) => Decimal::ZERO,
                    Err(m) => return Err((m, None)),
                };
                splits.push(json!({"account": l.category.trim(), "value": (-to_raw(header, amt * sign)).to_string(), "memo": l.memo}));
            }
        }
        None => {
            if e.category.trim().is_empty() {
                return Err(("Choose a category.".into(), Some(Field::Category)));
            }
            splits.push(json!({"account": e.category.trim(), "value": (-raw).to_string()}));
        }
    }
    let date = fmt::date_arg(&e.date).map_err(|m| (m, Some(Field::Date)))?;
    let mut args = json!({"date": date, "number": e.number, "payee": e.payee, "memo": e.memo, "splits": splits});
    let cmd = match &e.editing {
        Some(id) => {
            if let Some(o) = args.as_object_mut() {
                o.insert("id".into(), json!(id));
            }
            "txn.update"
        }
        None => "txn.create",
    };
    Ok((cmd, args))
}

fn error_field(msg: &str) -> Option<Field> {
    if msg.contains("date") || msg.contains("closed period") {
        Some(Field::Date)
    } else if msg.contains("split 1") || msg.contains("account") {
        Some(Field::Category)
    } else {
        None
    }
}

#[derive(Default)]
struct Actions {
    save: bool,
    cancel: bool,
    delete: bool,
    void: bool,
    unvoid: bool,
    discard: bool,
    open_split: bool,
    save_split: bool,
    toggle_fast: bool,
}

const FAST_OFF_TIP: &str = "Fast entry is off. Turn it on to add several transactions in a \
row: Enter saves each one and a new blank one opens, without closing this window.";
const FAST_ON_TIP: &str =
    "Fast entry is on: Enter saves and starts the next transaction. Click to turn it off.";

/// `+` or `=` typed at the end of a complete date moves it a day on; `-` a day
/// back (revision 20).
fn date_step(prev: &str, typed: &str) -> Option<String> {
    let complete = fmt::normalise_date(prev.trim()).is_some_and(|d| d == prev.trim());
    let step = if typed == format!("{prev}+") || typed == format!("{prev}=") {
        1
    } else if complete && typed == format!("{prev}-") {
        -1
    } else {
        return None;
    };
    fmt::shift_date(prev, step)
}

/// The suggestion lists after the payee or category changed.
fn refresh_suggestions(app: &mut App, e: &mut Entry, f: Field) {
    let account = app.register.account.clone();
    match f {
        Field::Payee => {
            e.payee_sugg = if e.payee.trim().is_empty() {
                Vec::new()
            } else {
                array(app.read(
                    "payee.suggest",
                    json!({"prefix": e.payee, "account": account, "limit": 8}),
                ))
            };
        }
        Field::Category => {
            e.cat_sugg = if e.category.trim().is_empty() {
                Vec::new()
            } else {
                array(app.read(
                    "account.suggest",
                    json!({"prefix": e.category.trim(), "limit": 8}),
                ))
            };
            // A typed exact name needs no list.
            if e.cat_sugg.len() == 1
                && e.cat_sugg
                    .first()
                    .is_some_and(|s| s.str("full_name").eq_ignore_ascii_case(e.category.trim()))
            {
                e.cat_sugg.clear();
            }
        }
        _ => {}
    }
}

/// The pop-up (when open), and what its buttons and keys do.
pub(crate) fn show(app: &mut App, ui: &mut Ui, header: &Value) {
    let Some(mut p) = app.register.popup.take() else {
        return;
    };
    let fast = app.config.fast_entry;
    let (first_title, second_title, first_inc) = titles(header);
    let (first, second) = if first_inc {
        (Field::Increase, Field::Decrease)
    } else {
        (Field::Decrease, Field::Increase)
    };
    let splitting = p.split.is_some();
    let mut act = Actions::default();
    if ui.key("ctrl+shift+f") {
        act.toggle_fast = true;
    }
    if !splitting && ui.key("ctrl+enter") && !p.voided {
        act.save = true;
    }
    let (title, prefix) = match p.mode {
        Mode::New => ("New transaction", "New"),
        Mode::Edit => ("Edit transaction", "Edit"),
    };
    let focus = p.focus.take();
    let receipts = receipt_items(app, &p);
    let mut receipt_act = None;
    let mut add_receipt = false;
    if ui.dialog_closed("txn") {
        act.cancel = true;
    }
    ui.dialog("txn", title, 520, |ui| {
        ui.close_label("Close");
        ui.group(|ui| {
            if p.reconciled() {
                ui.label(
                    "This transaction is reconciled. Its date and amounts can't be changed here.",
                );
            }
            if p.voided {
                ui.label("This transaction is void. Unvoid it to change it.");
            }
            let rows: [(Field, &str); 7] = [
                (Field::Date, "Date"),
                (Field::Number, "No."),
                (Field::Payee, "Payee"),
                (Field::Category, "Category"),
                (Field::Memo, "Memo"),
                (first, first_title),
                (second, second_title),
            ];
            for (f, label) in rows {
                if f == Field::Category
                    && let Some(lines) = &p.lines
                {
                    let n = lines.len();
                    let editable = p.splits_editable();
                    ui.form(label, |ui| {
                        ui.label(format!("Split ({n})"));
                        if editable && ui.button("Edit splits…").clicked() {
                            act.open_split = true;
                        }
                    });
                    continue;
                }
                let editable = p.editable(f);
                let name = format!("{prefix} {}", label.to_lowercase());
                let error = (p.entry.error_field == Some(f))
                    .then(|| p.entry.error.clone())
                    .flatten();
                ui.form(label, |ui| {
                    ui.enabled(editable)
                        .grow()
                        .id(format!("popup:{}", f.key()))
                        .error(error);
                    if focus == Some(f) && editable {
                        ui.focus().select_all();
                    }
                    if matches!(f, Field::Decrease | Field::Increase) {
                        ui.mono();
                    }
                    if f == Field::Date {
                        let pattern = fmt::date_style().pattern();
                        ui.hint(pattern).tip(format!("Date, format {pattern}"));
                    }
                    let before = entry_text(&mut p.entry, f).clone();
                    let resp = match f {
                        Field::Payee => {
                            let opts: Vec<String> = p
                                .entry
                                .payee_sugg
                                .iter()
                                .map(|s| s.str("payee").to_owned())
                                .collect();
                            let r = ui.suggest(&name, &mut p.entry.payee, opts);
                            if let Some(it) = ui.last() {
                                it.text = "Payee suggestion".into();
                            }
                            r
                        }
                        Field::Category => {
                            let opts: Vec<String> = p
                                .entry
                                .cat_sugg
                                .iter()
                                .map(|s| s.str("full_name").to_owned())
                                .collect();
                            let r = ui.suggest(&name, &mut p.entry.category, opts);
                            if let Some(it) = ui.last() {
                                it.text = "Category suggestion".into();
                            }
                            r
                        }
                        Field::Memo => ui.multiline(&name, &mut p.entry.memo),
                        _ => ui.field(&name, entry_text(&mut p.entry, f)),
                    };
                    if f == Field::Date {
                        // The rest of the pattern after the typed text (drawn greyed).
                        let typed = p.entry.date.clone();
                        let guide = if typed.is_empty() {
                            None
                        } else {
                            fmt::date_style().guide(&typed)
                        };
                        ui.set_last_text(guide.unwrap_or(""));
                    }
                    if resp.focused && matches!(f, Field::Date | Field::Decrease | Field::Increase)
                    {
                        app.request_select(&f.id());
                    }
                    if resp.changed && resp.chosen.is_none() {
                        p.accepted = false;
                        match f {
                            Field::Date => {
                                if let Some(d) = date_step(&before, &p.entry.date) {
                                    p.entry.date = d;
                                }
                            }
                            Field::Decrease if !p.entry.decrease.is_empty() => {
                                p.entry.increase.clear()
                            }
                            Field::Increase if !p.entry.increase.is_empty() => {
                                p.entry.decrease.clear()
                            }
                            Field::Payee | Field::Category => {
                                let mut e = std::mem::take(&mut p.entry);
                                refresh_suggestions(app, &mut e, f);
                                p.entry = e;
                            }
                            _ => {}
                        }
                    }
                    if let Some(i) = resp.chosen {
                        match f {
                            Field::Payee => {
                                if let Some(s) = p.entry.payee_sugg.get(i).cloned() {
                                    accept_payee(&mut p.entry, &s);
                                    p.accepted = true;
                                    app.request_focus(&Field::Category.id());
                                }
                            }
                            Field::Category => {
                                if let Some(s) = p.entry.cat_sugg.get(i).cloned() {
                                    p.entry.category = s.str("full_name").to_owned();
                                    p.entry.cat_sugg.clear();
                                    p.accepted = true;
                                    app.request_focus(&Field::Memo.id());
                                }
                            }
                            _ => {}
                        }
                    }
                    if resp.blurred {
                        match f {
                            Field::Payee => p.entry.payee_sugg.clear(),
                            Field::Category => p.entry.cat_sugg.clear(),
                            _ => {}
                        }
                    }
                    if resp.submitted && !p.voided && f != Field::Memo {
                        if std::mem::take(&mut p.accepted) {
                            // Enter took a suggestion; the save waits for the next.
                        } else {
                            act.save = true;
                        }
                    }
                });
            }
            let fixed = p.voided || p.reconciled();
            ui.form("Status", |ui| {
                for (status, label) in [
                    (Status::Uncleared, "Uncleared"),
                    (Status::Cleared, "Cleared"),
                ] {
                    ui.enabled(!fixed);
                    let mut cur = p.status;
                    if ui.radio(&mut cur, status, label).clicked() {
                        p.status = status;
                    }
                }
                if p.reconciled() {
                    ui.enabled(false);
                    let mut r = true;
                    ui.radio(&mut r, true, "Reconciled");
                }
            });
            if p.lines.is_none() && p.tags_on.any() {
                // A voided row keeps its tags.
                let before = p.tags.clone();
                let on = p.tags_on.clone();
                if super::tags::rows(ui, &on, &mut p.tags, prefix) && p.voided {
                    p.tags = before;
                }
            }
            if let Some(items) = &receipts {
                ui.form("Receipts", |ui| {
                    ui.label("");
                });
                receipt_act = super::receipts::strip(app, ui, items, !p.voided);
                if !p.voided {
                    ui.form("", |ui| {
                        if ui.button("Add receipt…").clicked() {
                            add_receipt = true;
                        }
                    });
                }
            }
            if p.entry.error_field.is_none()
                && let Some(err) = p.entry.error.clone()
            {
                ui.tip("Edit error");
                ui.error_text(err);
            }
        });
        if p.confirm_delete {
            ui.banner(Tone::Warning, |ui| {
                ui.label("Delete this transaction? You can undo it afterwards.");
                ui.row(|ui| {
                    if ui.button("Delete transaction").clicked() {
                        act.delete = true;
                    }
                    if ui.button("Keep it").clicked() {
                        p.confirm_delete = false;
                    }
                });
            });
        }
        if p.confirm_discard {
            ui.banner(Tone::Warning, |ui| {
                ui.label("Discard this transaction?");
                ui.row(|ui| {
                    if ui.button("Discard").clicked() {
                        act.discard = true;
                    }
                    if ui.button("Keep editing").clicked() {
                        p.confirm_discard = false;
                    }
                });
            });
        }
        ui.group(|ui| {
            ui.row(|ui| {
                if p.lines.is_none() && p.splits_editable() {
                    ui.tip("Split this transaction");
                    if ui.button("Split…").clicked() {
                        act.open_split = true;
                    }
                }
                if p.mode == Mode::Edit {
                    if p.voided {
                        if ui.button("Unvoid").clicked() {
                            act.unvoid = true;
                        }
                    } else {
                        if !p.reconciled() && ui.button("Delete…").clicked() {
                            p.confirm_delete = true;
                        }
                        if ui.button("Void").clicked() {
                            act.void = true;
                        }
                    }
                }
                if p.added > 0 {
                    ui.tone(Tone::Success);
                    ui.label(if p.added == 1 {
                        "1 transaction added".to_owned()
                    } else {
                        format!("{} transactions added", p.added)
                    });
                }
                ui.spacer();
                ui.tip(if fast { FAST_ON_TIP } else { FAST_OFF_TIP });
                if ui
                    .icon_toggle(
                        if fast { "foot-filled" } else { "foot" },
                        "Fast entry",
                        fast,
                    )
                    .clicked()
                {
                    act.toggle_fast = true;
                }
                if p.voided {
                    if ui.button("Close").clicked() {
                        act.cancel = true;
                    }
                } else {
                    ui.tip("Save (Enter, Ctrl+Enter)");
                    if ui.primary("Save").clicked() {
                        act.save = true;
                    }
                    if ui.button("Cancel").clicked() {
                        act.cancel = true;
                    }
                }
            });
        });
    });
    if !splitting && ui.key("escape") {
        act.cancel = true;
    }
    split_editor(ui, header, &mut p, &mut act);
    if add_receipt {
        app.emit(AppEvent::ChooseReceiptFiles {
            target: crate::ReceiptTarget::TransactionPopup,
        });
    }
    match receipt_act {
        Some(super::receipts::StripAction::View(r)) => super::receipts::view(app, &r),
        Some(super::receipts::StripAction::Remove(id)) => {
            let cmd = if p.receipts_pending.contains(&id) {
                "receipt.delete"
            } else {
                "receipt.unlink"
            };
            match app.run(cmd, json!({"receipt": id})) {
                Ok(_) => {
                    p.receipts_pending.retain(|x| *x != id);
                    super::receipts::forget(app, &id);
                }
                Err(e) => p.entry.error = Some(e.display()),
            }
        }
        None => {}
    }
    finish(app, header, p, &act);
}

fn split_editor(ui: &mut Ui, header: &Value, p: &mut Popup, act: &mut Actions) {
    let Some(mut ed) = p.split.take() else {
        return;
    };
    let mut keep = !ui.dialog_closed("split");
    if ui.key("escape") {
        keep = false;
    }
    let (first_title, second_title, first_inc) = titles(header);
    ui.dialog("split", "Split transaction", 640, |ui| {
        ui.close_label("Close");
        ui.group(|ui| {
            ui.row(|ui| {
                let e = &mut p.entry;
                let (a, b) = if first_inc {
                    (&mut e.increase, &mut e.decrease)
                } else {
                    (&mut e.decrease, &mut e.increase)
                };
                ui.caption(first_title);
                ui.width(110).mono();
                ui.field(first_title, a);
                ui.caption(second_title);
                ui.width(110).mono();
                ui.field(second_title, b);
            });
            for (i, l) in ed.lines.iter_mut().enumerate() {
                ui.row(|ui| {
                    ui.hint("Category").grow();
                    ui.field(&format!("Line {} category", i + 1), &mut l.category);
                    ui.hint("Memo").width(160);
                    ui.field(&format!("Line {} memo", i + 1), &mut l.memo);
                    ui.hint("Amount").width(110).mono();
                    ui.field(&format!("Line {} amount", i + 1), &mut l.amount);
                });
            }
            ui.row(|ui| {
                if ui.button("Add line").clicked() {
                    ed.lines.push(SplitLine::default());
                }
            });
        });
        let total = entry_amount(&p.entry).ok().flatten().map(|d| d.abs());
        let mut sum = Decimal::ZERO;
        let mut bad = false;
        for l in &ed.lines {
            match fmt::parse_entry(&l.amount) {
                Ok(Some(a)) => sum += a,
                Ok(None) => {}
                Err(_) => bad = true,
            }
        }
        let left = total.map(|t| t - sum);
        let left_text = left.map_or_else(
            || "enter the total first".into(),
            |l| mk_core::currency::format_amount(l, Some(scale(header))),
        );
        let balanced = !bad && left.is_some_and(|l| l.is_zero());
        ui.group(|ui| {
            ui.label(format!("Left to assign {left_text}"));
            if let Some(err) = &ed.error {
                ui.error_text(err.clone());
            }
        });
        ui.actions(|ui| {
            ui.named("Cancel split");
            if ui.button("Cancel").clicked() {
                keep = false;
            }
            ui.enabled(balanced);
            if ui.primary("Save split").clicked() {
                act.save_split = true;
            }
        });
    });
    act.save = false;
    act.cancel = false;
    if keep {
        p.split = Some(ed);
    } else {
        act.save_split = false;
    }
}

fn finish(app: &mut App, header: &Value, mut p: Popup, act: &Actions) {
    if act.toggle_fast {
        app.config.fast_entry = !app.config.fast_entry;
        app.emit(AppEvent::ConfigChanged);
    }
    if act.open_split {
        start_split(&mut p);
    }
    let txn = p.entry.editing.clone().unwrap_or_default();
    let split_lines = if act.save_split {
        p.split.as_ref().map(|s| s.lines.clone())
    } else {
        None
    };
    let saving = !p.voided && (act.save_split || (act.save && p.split.is_none()));
    let result = if act.delete {
        Some(run(app, "txn.delete", json!({"id": txn})))
    } else if act.void {
        Some(run(app, "txn.void", json!({"id": txn})))
    } else if act.unvoid {
        Some(run(app, "txn.unvoid", json!({"id": txn})))
    } else if saving {
        let lines = split_lines.as_deref().or(p.lines.as_deref());
        Some(save(app, header, &p, lines))
    } else {
        None
    };
    let close = match result {
        Some(Ok(())) if saving => {
            let iso = fmt::date_arg(&p.entry.date).unwrap_or_else(|_| app.today());
            app.register.last_date = Some(iso.clone());
            if app.config.fast_entry {
                let added = p.added + usize::from(p.mode == Mode::New);
                let tags_on = p.tags_on.clone();
                p = new_form(&iso, added);
                p.tags_on = tags_on;
                false
            } else {
                true
            }
        }
        Some(Ok(())) => true,
        Some(Err((msg, field))) => {
            match p.split.as_mut() {
                Some(ed) if act.save_split => ed.error = Some(msg),
                _ => {
                    p.entry.error = Some(msg);
                    p.entry.error_field = field;
                }
            }
            false
        }
        None if act.discard => true,
        None if act.cancel => cancel(app, &mut p),
        None => false,
    };
    if close {
        if let Some(row) = p.row {
            app.register.selected = (!act.delete).then_some(row);
        }
        app.blur_for_tests();
    } else {
        app.register.popup = Some(p);
    }
}

fn cancel(app: &App, p: &mut Popup) -> bool {
    if p.confirm_delete {
        p.confirm_delete = false;
        return false;
    }
    if p.confirm_discard {
        return true;
    }
    if p.mode == Mode::New && app.config.fast_entry && p.partly_filled() {
        p.confirm_discard = true;
        return false;
    }
    true
}

fn run(app: &mut App, cmd: &str, args: Value) -> Result<(), FieldError> {
    run_value(app, cmd, args).map(|_| ())
}

fn run_value(app: &mut App, cmd: &str, args: Value) -> Result<Value, FieldError> {
    app.run(cmd, args).map_err(|e| {
        let msg = e.display();
        let field = error_field(&msg);
        (msg, field)
    })
}

fn save(
    app: &mut App,
    header: &Value,
    p: &Popup,
    lines: Option<&[SplitLine]>,
) -> Result<(), FieldError> {
    let (cmd, mut args) = txn_args(header, &p.entry, lines)?;
    if lines.is_none() && p.tags_on.any() {
        if let Some(m) = super::tags::missing(&p.tags_on, &p.tags) {
            return Err((m, None));
        }
        // The category split is the second one.
        if let Some(split) = args.get_mut("splits").and_then(|s| s.get_mut(1)) {
            p.tags.apply(split, &p.tags_on);
        }
    }
    let out = run_value(app, cmd, args)?;
    let id = p
        .entry
        .editing
        .clone()
        .unwrap_or_else(|| out.str("id").to_owned());
    for r in &p.receipts_pending {
        run(
            app,
            "receipt.link",
            json!({"receipt": r, "transaction": id}),
        )?;
    }
    let wanted = match p.status {
        Status::Uncleared => "n",
        Status::Cleared => "c",
        Status::Reconciled => return Ok(()),
    };
    let account = header.at("account").str("id").to_owned();
    let mine = app.read("txn.get", json!({"id": id})).and_then(|t| {
        t.list("splits")
            .iter()
            .position(|s| s.str("account") == account)
            .map(|i| {
                (
                    i,
                    t.list("splits")
                        .get(i)
                        .map(|s| s.str("reconcile").to_owned()),
                )
            })
    });
    if let Some((idx, state)) = mine
        && state.as_deref() != Some(wanted)
    {
        run(
            app,
            "txn.set_reconcile",
            json!({"splits": [{"id": id, "split": idx, "state": wanted}]}),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::date_step;

    #[test]
    fn plus_and_minus_move_a_complete_date() {
        crate::fmt::set_dates(
            mk_core::datefmt::DateStyle::parse("YYYY-MM-DD").unwrap_or_default(),
            mk_core::Date::from_ymd(2026, 10, 22).ok(),
        );
        assert_eq!(
            date_step("2026-10-05", "2026-10-05+").as_deref(),
            Some("2026-10-06")
        );
        assert_eq!(
            date_step("2026-10-05", "2026-10-05-").as_deref(),
            Some("2026-10-04")
        );
        assert_eq!(date_step("2026-10", "2026-10-"), None);
    }
}
