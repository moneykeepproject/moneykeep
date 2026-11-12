//! Settings → Phone capture → Start over… (mobile-capture.md revision 17): a preview
//! of what goes in three groups and what is kept (nothing changes until
//! confirmed), Collect now for phone items not yet collected, the typed
//! `START OVER`, then the result list with what was left and where to finish by
//! hand, Try again (the computer part) and Set up phone capture.

use serde_json::{Value, json};

use super::App;
use super::capture::{Kind, busy, start};
use super::ui::Ui;
use super::util::Json;

/// What the confirmation field must hold (case-insensitive).
pub(crate) const CONFIRM_WORD: &str = "START OVER";
pub(crate) const OPEN_LABEL: &str = "Start over…";
pub(crate) const STUCK_LABEL: &str = "Stuck? Start over…";
const INTRO: &str = "Start over removes everything phone capture set up — in Cloudflare, on this computer and in MoneyKeep — so you can begin again from nothing. Nothing changes until you press Start over.";

#[derive(Debug, Default)]
pub(crate) struct StartOver {
    pub(crate) open: bool,
    /// The last preview.
    pub(crate) preview: Option<Value>,
    /// The preview could not be made.
    pub(crate) error: Option<String>,
    /// The typed confirmation.
    pub(crate) typed: String,
    /// The run's result.
    pub(crate) result: Option<Value>,
}

/// `START OVER`, any case, spaces around and between ignored.
pub(crate) fn confirmed(typed: &str) -> bool {
    typed
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .eq_ignore_ascii_case(CONFIRM_WORD)
}

/// Open the dialog (closing a setup dialog it was opened from) and read the
/// preview.
pub(crate) fn open(app: &mut App) {
    app.capture.setup.open = false;
    app.capture.wifi.open = false;
    app.capture.start_over = StartOver {
        open: true,
        ..StartOver::default()
    };
    refresh(app);
}

fn refresh(app: &mut App) {
    if !busy(app, Kind::StartOverPreview) {
        start(
            app,
            "capture.start_over_preview",
            json!({}),
            Kind::StartOverPreview,
        );
    }
}

/// A background job of this dialog ended.
pub(crate) fn finished(app: &mut App, kind: Kind, out: Result<Value, crate::UiError>) {
    let d = &mut app.capture.start_over;
    match (kind, out) {
        (Kind::StartOverPreview, Ok(v)) => {
            d.preview = Some(v);
            d.error = None;
        }
        (Kind::StartOverPreview, Err(e)) => d.error = Some(e.display()),
        (_, Ok(v)) => {
            d.result = Some(v);
            d.typed.clear();
            super::capture::cleared(app);
        }
        (_, Err(e)) => d.error = Some(e.display()),
    }
}

/// Collect now finished while the dialog is open: count again.
pub(crate) fn collected(app: &mut App) {
    if app.capture.start_over.open && app.capture.start_over.result.is_none() {
        refresh(app);
    }
}

/// "Stuck? Start over…" under a setup failure.
pub(crate) fn stuck(app: &mut App, ui: &mut Ui) {
    ui.row(|ui| {
        ui.tip("Remove everything phone capture set up and begin again from nothing");
        if ui.button(STUCK_LABEL).clicked() {
            open(app);
        }
    });
}

/// The Start over card in Settings → Phone capture (always available).
pub(crate) fn card(app: &mut App, ui: &mut Ui) {
    ui.card("Start over", |ui| {
        ui.secondary(
            "Stuck halfway, or deleted pieces by hand? Remove everything phone capture set up and begin again. Your books and backups are never touched.",
        );
        ui.row(|ui| {
            if ui.button(OPEN_LABEL).clicked() {
                open(app);
            }
        });
    });
}

fn group(ui: &mut Ui, title: &str, items: &[Value]) {
    ui.bold();
    ui.label(title);
    for i in items {
        match i.as_str() {
            Some(t) => ui.label(format!("• {t}")),
            None => {
                let place = i.str("where");
                if place.is_empty() {
                    ui.label(format!("• {}", i.str("what")));
                } else {
                    ui.label(format!("• {} — {place}", i.str("what")));
                }
            }
        }
    }
}

pub(crate) fn dialog(app: &mut App, ui: &mut Ui) {
    if !app.capture.start_over.open {
        return;
    }
    let mut close = ui.dialog_closed("capture-start-over") || ui.key("escape");
    let looking = busy(app, Kind::StartOverPreview);
    let working = busy(app, Kind::StartOver);
    let collecting =
        busy(app, Kind::Collect { report: true }) || busy(app, Kind::Collect { report: false });
    let preview = app.capture.start_over.preview.clone();
    let result = app.capture.start_over.result.clone();
    let error = app.capture.start_over.error.clone();
    let mut go = false;
    let mut collect = false;
    let mut again = false;
    let mut set_up = false;
    ui.dialog("capture-start-over", "Start over", 680, |ui| {
        if let Some(r) = &result {
            result_view(ui, r);
            ui.actions(|ui| {
                if working {
                    ui.progress("Removing", -1);
                }
                close |= ui.button("Close").clicked();
                if r.at("computer_left").as_bool() == Some(true) {
                    ui.enabled(!working);
                    again = ui.button("Try again").clicked();
                }
                ui.enabled(!working);
                set_up = ui.primary("Set up phone capture").clicked();
            });
            return;
        }
        ui.label(INTRO);
        if let Some(e) = &error {
            ui.error_text(e.clone());
        }
        let Some(p) = &preview else {
            if looking {
                ui.row(|ui| {
                    ui.progress("Looking", -1);
                    ui.label("Looking at what phone capture set up…");
                });
            }
            ui.actions(|ui| {
                close |= ui.button("Cancel").clicked();
            });
            return;
        };
        let cf = p.at("cloudflare");
        ui.bold();
        ui.label("In Cloudflare");
        if let Some(problem) = cf.at("problem").as_str() {
            ui.error_text(problem);
        }
        for i in cf.list("items") {
            let earlier = if i.at("earlier").as_bool() == Some(true) {
                " (left by an earlier Start over)"
            } else {
                ""
            };
            ui.label(format!("• {}{earlier} — {}", i.str("what"), i.str("where")));
        }
        if let Some(n) = cf.at("note").as_str() {
            ui.secondary(n);
        }
        group(ui, "On this computer", p.list("computer"));
        if let Some(t) = p.at("permission").as_str() {
            ui.secondary(t);
        }
        group(ui, "In MoneyKeep", p.list("moneykeep"));
        if let Some(t) = p.at("uncollected_text").as_str() {
            ui.error_text(t);
            ui.row(|ui| {
                ui.enabled(!collecting && !working);
                collect = ui.button("Collect now").clicked();
                if collecting {
                    ui.progress("Collecting", -1);
                    ui.label("Collecting…");
                }
            });
        }
        group(ui, "Kept", p.list("kept"));
        ui.label(format!("Type {CONFIRM_WORD} to confirm:"));
        ui.width(240);
        ui.field(
            "Type START OVER to confirm",
            &mut app.capture.start_over.typed,
        );
        let ready = confirmed(&app.capture.start_over.typed);
        ui.actions(|ui| {
            if working {
                ui.progress("Removing", -1);
                ui.label("Removing…");
            }
            close |= ui.button("Cancel").clicked();
            ui.enabled(ready && !working && !looking);
            go = ui.primary("Start over").clicked();
        });
    });
    if go || again {
        let typed = if go {
            app.capture.start_over.typed.trim().to_owned()
        } else {
            CONFIRM_WORD.to_owned()
        };
        app.capture.start_over.error = None;
        start(
            app,
            "capture.start_over",
            json!({"confirm": typed}),
            Kind::StartOver,
        );
    } else if collect {
        start(
            app,
            "capture.collect",
            json!({}),
            Kind::Collect { report: true },
        );
    } else if set_up {
        app.capture.start_over = StartOver::default();
        super::capture_wifi::open(app, None);
    } else if close && !working {
        app.capture.start_over = StartOver::default();
    }
}

fn result_view(ui: &mut Ui, r: &Value) {
    ui.bold();
    ui.label(r.str("text"));
    for (key, title) in [
        ("cloudflare", "In Cloudflare"),
        ("computer", "On this computer"),
        ("moneykeep", "In MoneyKeep"),
    ] {
        let steps: Vec<&Value> = r
            .list("steps")
            .iter()
            .filter(|s| s.str("group") == key)
            .collect();
        if steps.is_empty() {
            continue;
        }
        ui.bold();
        ui.label(title);
        for s in steps {
            if s.str("result") == "left" {
                let reason = s.str("reason").trim_end_matches('.');
                ui.error_text(format!(
                    "• {} — left: {reason}. Where: {}",
                    s.str("what"),
                    s.str("where")
                ));
            } else {
                ui.label(format!("• {} — removed", s.str("what")));
            }
        }
    }
    ui.secondary(r.str("phones"));
}
