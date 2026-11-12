//! Corporate action forms (app-desktop.md revision 13): Merger…, Spin-off…, Symbol
//! change… and Cash in lieu…, each with a live preview of the resulting lots from
//! `inv.preview_corporate`; and the Wash groups… dialog of the Securities view.

use serde_json::{Value, json};

use super::form::set;
use super::ui::{Column, Ui, col, col_num};
use super::util::Json;
use super::{App, MessageAction};
use crate::fmt;

const MODES: [(&str, &str); 2] = [("tax_free", "Tax-free"), ("taxable", "Taxable")];

/// An open corporate action form.
#[derive(Debug, Clone, Default)]
pub(crate) struct CorpForm {
    /// `merger`, `spinoff`, `symbol_change` or `cash_in_lieu`.
    pub kind: &'static str,
    pub account: String,
    pub date: String,
    pub security: String,
    pub new_security: String,
    pub ratio_new: String,
    pub ratio_old: String,
    pub cash_per_share: String,
    pub new_share_value: String,
    pub mode: usize,
    pub new_per_parent: String,
    pub allocation_percent: String,
    pub cil_quantity: String,
    pub cil_amount: String,
    pub new_symbol: String,
    pub new_name: String,
    pub quantity: String,
    pub amount: String,
    pub preview_key: String,
    pub preview: Option<Value>,
    pub preview_error: Option<String>,
    pub error: Option<String>,
}

/// The Wash groups… dialog.
#[derive(Debug, Clone, Default)]
pub(crate) struct WashForm {
    pub security: String,
    pub group: String,
    pub error: Option<String>,
}

pub(crate) const KINDS: [(&str, &str); 4] = [
    ("merger", "Merger…"),
    ("spinoff", "Spin-off…"),
    ("symbol_change", "Symbol change…"),
    ("cash_in_lieu", "Cash in lieu…"),
];

fn title(kind: &str) -> &'static str {
    match kind {
        "merger" => "Merger",
        "spinoff" => "Spin-off",
        "symbol_change" => "Symbol change",
        _ => "Cash in lieu",
    }
}

pub(crate) fn open(app: &mut App, kind: &'static str, account: &str) {
    app.inv.corp = Some(CorpForm {
        kind,
        account: account.to_owned(),
        date: fmt::date(&app.today()),
        ratio_new: "1".into(),
        ratio_old: "1".into(),
        ..CorpForm::default()
    });
}

fn put(a: &mut Value, key: &str, text: &str) {
    let t = text.trim().replace(',', "");
    if !t.is_empty() {
        set(a, key, json!(t));
    }
}

/// The command arguments from the form.
fn args(f: &CorpForm) -> Value {
    let mut a = json!({"date": fmt::date_arg_lenient(&f.date)});
    match f.kind {
        "merger" => {
            put(&mut a, "old_security", &f.security);
            put(&mut a, "new_security", &f.new_security);
            put(&mut a, "ratio_new", &f.ratio_new);
            put(&mut a, "ratio_old", &f.ratio_old);
            put(&mut a, "cash_per_share", &f.cash_per_share);
            put(&mut a, "new_share_value", &f.new_share_value);
            set(
                &mut a,
                "mode",
                json!(MODES.get(f.mode).map_or("tax_free", |m| m.0)),
            );
        }
        "spinoff" => {
            put(&mut a, "parent", &f.security);
            put(&mut a, "new_security", &f.new_security);
            put(&mut a, "new_per_parent", &f.new_per_parent);
            put(&mut a, "allocation_percent", &f.allocation_percent);
            if !f.cil_quantity.trim().is_empty() && !f.cil_amount.trim().is_empty() {
                let mut c = json!({"account": f.account});
                put(&mut c, "quantity", &f.cil_quantity);
                put(&mut c, "amount", &f.cil_amount);
                set(&mut a, "cash_in_lieu", c);
            }
        }
        "symbol_change" => {
            put(&mut a, "security", &f.security);
            put(&mut a, "new_symbol", &f.new_symbol);
            put(&mut a, "new_name", &f.new_name);
        }
        _ => {
            set(&mut a, "account", json!(f.account));
            put(&mut a, "security", &f.security);
            put(&mut a, "quantity", &f.quantity);
            put(&mut a, "amount", &f.amount);
        }
    }
    a
}

fn refresh(app: &mut App, f: &mut CorpForm) {
    let a = args(f);
    let key = a.to_string();
    if key == f.preview_key {
        return;
    }
    f.preview_key = key;
    let mut p = a;
    set(&mut p, "action", json!(f.kind));
    match app.backend.invoke("inv.preview_corporate", p) {
        Ok(v) => {
            f.preview = Some(v);
            f.preview_error = None;
        }
        Err(e) => {
            f.preview = None;
            f.preview_error = Some(e.display());
        }
    }
}

fn field(ui: &mut Ui, label: &str, text: &mut String, hint: &str) {
    ui.form(label, |ui| {
        ui.width(240);
        if !hint.is_empty() {
            ui.hint(hint);
        }
        ui.field(&format!("{label} field"), text);
    });
}

pub(crate) fn form_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.inv.corp.take() else {
        return;
    };
    refresh(app, &mut f);
    let mut save = false;
    let mut cancel = ui.dialog_closed("corporate-form") || ui.key("escape");
    ui.dialog("corporate-form", title(f.kind), 600, |ui| {
        ui.group(|ui| {
            ui.form("Date", |ui| super::register::date_field(ui, "Date field", &mut f.date));
            match f.kind {
                "merger" => {
                    field(ui, "Old security", &mut f.security, "symbol");
                    field(ui, "New security", &mut f.new_security, "symbol");
                    field(ui, "New shares", &mut f.ratio_new, "1");
                    field(ui, "For old shares", &mut f.ratio_old, "2");
                    field(ui, "Cash per old share", &mut f.cash_per_share, "0.00");
                    field(ui, "Value of one new share", &mut f.new_share_value, "needed with cash");
                    ui.form("Treatment", |ui| {
                        super::form::choice(ui, "Treatment", &mut f.mode, &MODES.map(|m| m.1));
                    });
                }
                "spinoff" => {
                    field(ui, "Parent security", &mut f.security, "symbol");
                    field(ui, "New security", &mut f.new_security, "symbol");
                    field(ui, "New shares per parent share", &mut f.new_per_parent, "0.5");
                    field(ui, "Basis moving to the new security %", &mut f.allocation_percent, "12.5");
                    field(ui, "Cash in lieu: shares", &mut f.cil_quantity, "optional");
                    field(ui, "Cash in lieu: amount", &mut f.cil_amount, "optional");
                }
                "symbol_change" => {
                    field(ui, "Security", &mut f.security, "symbol");
                    field(ui, "New symbol", &mut f.new_symbol, "");
                    field(ui, "New name", &mut f.new_name, "optional");
                }
                _ => {
                    field(ui, "Security", &mut f.security, "symbol");
                    field(ui, "Fractional shares", &mut f.quantity, "0.5");
                    field(ui, "Cash", &mut f.amount, "0.00");
                }
            }
        });
        if f.kind == "spinoff" {
            ui.secondary("The basis percentage comes from the company's published basis statement (IRS Form 8937).");
        }
        preview(ui, &f);
        if let Some(e) = f.error.as_ref().or(f.preview_error.as_ref()) {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            ui.enabled(f.preview.is_some());
            if ui.primary("Save").clicked() {
                save = true;
            }
        });
    });
    if cancel {
        return;
    }
    if save {
        let command = format!("inv.{}", f.kind);
        match app.run(&command, args(&f)) {
            Ok(_) => {
                app.notify_with(format!("{} recorded", title(f.kind)), MessageAction::Undo);
                return;
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    app.inv.corp = Some(f);
}

/// The resulting lots of every account the action touches, and the gain.
fn preview(ui: &mut Ui, f: &CorpForm) {
    let Some(p) = &f.preview else {
        ui.secondary("Fill in the terms to see the resulting lots.");
        return;
    };
    if f.kind == "symbol_change" {
        ui.label(format!(
            "From {}: {}",
            fmt::normalise_date(&f.date).unwrap_or_else(|| f.date.trim().to_owned()),
            p.at("security").str("symbol")
        ));
        return;
    }
    ui.heading("Resulting lots");
    for acc in p.list("accounts") {
        let note = format!(
            "Preview lots of {}: {}",
            acc.str("symbol"),
            acc.list("lots")
                .iter()
                .map(|l| format!(
                    "{} shares, basis {}, acquired {}",
                    l.str("quantity"),
                    fmt::group(l.str("basis")),
                    l.str("acquired")
                ))
                .collect::<Vec<_>>()
                .join("; ")
        );
        let cols = vec![
            Column {
                width: 110,
                ..col("Acquired")
            },
            Column {
                width: 110,
                ..col_num("Shares")
            },
            Column {
                width: 120,
                ..col_num("Basis")
            },
        ];
        ui.table(acc.str("full_name"), cols, |ui| {
            for (i, l) in acc.list("lots").iter().enumerate() {
                ui.table_row(&format!("lot{i}"), false, false, |ui| {
                    ui.label(l.str("acquired"));
                    ui.mono();
                    ui.amount(l.str("quantity"));
                    ui.mono();
                    ui.amount(fmt::group(l.str("basis")));
                });
            }
        });
        // The lots in one sentence for screen readers.
        ui.named(note);
        ui.caption("");
    }
    if !fmt::is_zero(p.str("gain")) {
        ui.label(format!("Recognised gain {}", fmt::group(p.str("gain"))));
    }
}

// ----- wash groups ------------------------------------------------------------------------

pub(crate) fn wash_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut w) = app.inv.wash.take() else {
        return;
    };
    let groups = super::util::array(app.read("security.wash_groups", json!({})));
    let mut close = ui.dialog_closed("wash-groups") || ui.key("escape");
    let mut unlink: Option<String> = None;
    let mut link = false;
    ui.dialog("wash-groups", "Wash groups", 560, |ui| {
        ui.secondary(
            "Link securities you consider substantially identical; wash-sale checks then treat them as one. MoneyKeep makes no judgement of its own.",
        );
        if groups.is_empty() {
            ui.label("No wash groups yet.");
        }
        for g in &groups {
            ui.bold();
            ui.label(g.str("name"));
            for m in g.list("members") {
                ui.row(|ui| {
                    ui.label(format!("{}  {}", m.str("symbol"), m.str("name")));
                    ui.tip(format!("Remove {} from {}", m.str("symbol"), g.str("name")));
                    if ui.button("Remove").clicked() {
                        unlink = Some(m.str("id").to_owned());
                    }
                });
            }
        }
        ui.row(|ui| {
            ui.width(140);
            ui.field("Security", &mut w.security);
            ui.width(200);
            ui.field("Group name", &mut w.group);
            if ui.button("Add to group").clicked() {
                link = true;
            }
        });
        if let Some(e) = &w.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Close").clicked() {
                close = true;
            }
        });
    });
    if let Some(id) = unlink
        && let Err(e) = app.run("security.unlink_wash_group", json!({"securities": [id]}))
    {
        w.error = Some(e.display());
    }
    if link {
        match app.run(
            "security.link_wash_group",
            json!({"securities": [w.security.trim()], "name": w.group.trim()}),
        ) {
            Ok(_) => {
                w.security.clear();
                w.error = None;
            }
            Err(e) => w.error = Some(e.display()),
        }
    }
    if !close {
        app.inv.wash = Some(w);
    }
}
