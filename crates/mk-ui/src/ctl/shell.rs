//! The main window's frame: top bar, message pop-up, the main area's view switch
//! and the Features menu (app-desktop.md revisions 5, 25, 26, 29).

use super::ui::{Tone, Ui};
use super::{App, MessageAction};
use crate::{AppEvent, View};

/// Whether two views are the same screen (for the navigation highlight).
pub(crate) fn same_kind(a: &View, b: &View) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

impl App {
    /// The screens of the modules that are on, alphabetically (revision 25).
    pub(crate) fn feature_screens(&mut self) -> Vec<(&'static str, View)> {
        let mut items = Vec::new();
        if self.budgets_enabled() {
            items.push(("Budget", View::Budget));
        }
        if self.module_on("bills") {
            items.push(("Purchases", View::Purchases));
        }
        if self.module_on("expenses") {
            items.push(("Expenses", View::Expenses));
        }
        if self.module_on("mileage") {
            items.push(("Mileage", View::Mileage));
        }
        if self.module_on("subscriptions") {
            items.push(("Subscriptions", View::Subscriptions));
        }
        if self.module_on("projects") {
            items.push(("Projects", View::Projects));
        }
        if self.module_on("invoicing") {
            items.push(("Sales", View::Sales));
        }
        if self.module_on("investments") {
            items.push(("Securities", View::Securities));
        }
        items.sort_by_key(|(label, _)| *label);
        items
    }
}

/// The top bar.
pub(crate) fn top_bar(app: &mut App, ui: &mut Ui) {
    ui.row(|ui| {
        ui.enabled(!app.back.is_empty()).tip("Back");
        if ui.icon_button("back", "Back", false).clicked() {
            app.go_back();
        }
        ui.enabled(!app.forward.is_empty()).tip("Forward");
        if ui.icon_button("forward", "Forward", false).clicked() {
            app.go_forward();
        }
        // The profile name (revision 26), else the backend's (file) name.
        let name = app
            .profile_name
            .clone()
            .or_else(|| app.backend.book_name())
            .unwrap_or_default();
        ui.heading(name);
        ui.separator();
        let tip = app
            .undo_hint
            .as_ref()
            .map_or_else(|| "Nothing to undo".to_owned(), |h| format!("Undo {h}"));
        ui.enabled(app.undo_hint.is_some()).tip(tip);
        if ui.icon_button("undo", "Undo", false).clicked() {
            app.undo();
        }
        let tip = app
            .redo_hint
            .as_ref()
            .map_or_else(|| "Nothing to redo".to_owned(), |h| format!("Redo {h}"));
        ui.enabled(app.redo_hint.is_some()).tip(tip);
        if ui.icon_button("redo", "Redo", false).clicked() {
            app.redo();
        }
        ui.separator();
        // Icon buttons like the rest of the bar (their names stay "Import…" and
        // "Reconcile…").
        ui.tip("Import a file");
        if ui.icon_button("sym-import", "Import…", false).clicked() {
            app.emit(AppEvent::ChooseImportFile);
        }
        if matches!(app.view, View::Register { .. }) {
            ui.tip("Reconcile this account");
        }
        if let View::Register { account } = app.view.clone()
            && ui
                .icon_button("sym-reconcile", "Reconcile…", false)
                .clicked()
        {
            app.navigate(View::Reconcile { account });
        }
        ui.separator();
        let current = app.view.clone();
        nav_items(
            app,
            ui,
            &current,
            &[("Home", View::Home), ("Scheduled", View::Scheduled)],
        );
        features_menu(app, ui, &current);
        let mut items: Vec<(&str, View)> = vec![("Reports", View::Reports)];
        match &current {
            View::Register { .. } => items.push(("Register", current.clone())),
            View::Reconcile { .. } => items.push(("Reconcile", current.clone())),
            View::Import { .. } => items.push(("Import review", current.clone())),
            _ => {}
        }
        items.extend([("Accounts", View::Accounts), ("History", View::History)]);
        nav_items(app, ui, &current, &items);
        ui.spacer();
        // The phone capture light (revision 29).
        if let Some(v) = super::capture::top_light(app) {
            ui.tip(super::capture_setup::tooltip(&v));
            if ui
                .light(
                    super::util::Json::str(&v, "state"),
                    18,
                    super::util::Json::str(&v, "accessible_name"),
                )
                .clicked()
            {
                app.config.settings_section = "Phone capture".to_owned();
                app.emit(crate::AppEvent::ConfigChanged);
                app.navigate(View::Settings);
            }
        }
        // AI assistants connected over the control channel (mcp-server.md rev. 3).
        super::ai::indicator(app, ui);
        ui.tip("About MoneyKeep");
        if ui.icon_button("info", "About MoneyKeep", false).clicked() {
            super::about::open(app);
        }
        // Calculator, Settings and Close book at the far right (revision 25).
        let calc_open = app.calc_open();
        ui.tip("Calculator");
        if ui
            .icon_button("calculator-key", "Calculator", calc_open)
            .clicked()
        {
            app.toggle_calculator();
        }
        let settings = matches!(current, View::Settings);
        ui.tip("Settings");
        if ui.icon_tab("gear-key", "Settings", settings).clicked() && !settings {
            app.navigate(View::Settings);
        }
        ui.tip("Close book");
        if ui.icon_button("close-book", "Close book", false).clicked() {
            app.close_book();
        }
    });
}

/// Navigation items as tabs; the current view's is selected.
fn nav_items(app: &mut App, ui: &mut Ui, current: &View, items: &[(&str, View)]) {
    for (label, view) in items {
        let selected = same_kind(current, view);
        if ui.tab(*label, selected).clicked() && !selected {
            app.navigate(view.clone());
        }
    }
}

/// The Features drop-down: hidden when no module screen is on; selected while
/// one of its screens is open, which is checked in the menu.
fn features_menu(app: &mut App, ui: &mut Ui, current: &View) {
    let items = app.feature_screens();
    if items.is_empty() {
        return;
    }
    let names: Vec<String> = items.iter().map(|(l, _)| (*l).to_owned()).collect();
    let checked = items.iter().position(|(_, v)| same_kind(current, v));
    let resp = ui.menu("Features", &names);
    if let Some(it) = ui.last() {
        it.value = "check".into();
        it.index = checked.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1);
        it.checked = checked.is_some();
    }
    if let Some(i) = resp.chosen
        && let Some((_, view)) = items.get(i)
        && Some(i) != checked
    {
        app.navigate(view.clone());
    }
}

/// The id the views report the pointer over the message pop-up with (`enter`,
/// `leave`).
pub const MESSAGE_BAND: &str = "message-band";

/// Whether keyboard focus is on an item of the message pop-up (its ids are in
/// the `msg` scope).
fn band_focused(app: &App) -> bool {
    app.focused
        .as_deref()
        .is_some_and(|f| f.starts_with("msg/"))
}

/// Revision 37: close a status message by itself after the chosen seconds,
/// unless it is an error, the setting is off, or the pointer, keyboard focus or
/// its open Details hold it (it then closes the full time after they leave).
/// Returns whether the window changed.
pub(crate) fn autoclose(app: &mut App) -> bool {
    let Some((_, is_error)) = app.message.as_ref() else {
        app.message_hover = false;
        return false;
    };
    let prefs = app.config.ui;
    if *is_error || !prefs.message_autoclose {
        return false;
    }
    let details = app.message_details_open && app.message_details.is_some();
    if app.message_hover || details || band_focused(app) {
        app.message_since = app.now;
        return false;
    }
    if crate::layout::elapsed(app.now, app.message_since, f64::from(prefs.seconds())) {
        app.clear_message();
        app.message_hover = false;
        return true;
    }
    false
}

/// Close the message from inside the pop-up (× or Esc): keyboard focus, if it
/// was in the pop-up, goes back to where it was before F6 brought it there, else
/// to the window.
fn close_from_popup(app: &mut App) {
    let had_focus = band_focused(app);
    app.clear_message();
    app.message_hover = false;
    if had_focus {
        app.focused = None;
        match app.message_return_focus.take() {
            Some(id) => app.request_focus(&id),
            None => app.focus_root += 1,
        }
    }
}

/// The last text may be narrower than its longest word (a file path): it then
/// wraps inside the pop-up's width instead of pushing × out of the card.
fn shrinkable(ui: &mut Ui) {
    if let Some(it) = ui.last() {
        it.width = 1;
    }
}

/// The status message (revision 42): a pop-up at the window's bottom-right
/// corner that floats over the page (the views place it). The message, its tone
/// (the views draw the tone's icon), its Show / Review / Undo / Details actions
/// and a × button named "Dismiss"; open Details show under it.
pub(crate) fn banner(app: &mut App, ui: &mut Ui) {
    // The pointer over the pop-up, or leaving it (revision 37).
    match ui
        .action()
        .filter(|a| a.id == MESSAGE_BAND && a.ev == super::ui::Ev::Key)
        .map(|a| a.value.clone())
        .as_deref()
    {
        Some("enter") => {
            ui.consume();
            app.message_hover = true;
        }
        Some("leave") => {
            ui.consume();
            if app.message_hover {
                app.message_since = app.now;
            }
            app.message_hover = false;
        }
        _ => {}
    }
    if app.message.is_none() {
        // Focus cannot stay on a pop-up that is gone (it would hold the next one).
        if band_focused(app) {
            app.focused = None;
        }
        app.message_return_focus = None;
        return;
    }
    // Esc while focus is in the pop-up closes it.
    if band_focused(app) && ui.key("escape") {
        close_from_popup(app);
        return;
    }
    // F6 (revision 42): into the pop-up, and from it back to where focus was.
    let mut focus_first = false;
    if ui.key("f6") {
        if band_focused(app) {
            app.focused = None;
            match app.message_return_focus.take() {
                Some(id) => app.request_focus(&id),
                None => app.focus_root += 1,
            }
        } else {
            app.message_return_focus = app.focused.clone();
            focus_first = true;
        }
    }
    let Some((text, is_error)) = app.message.clone() else {
        return;
    };
    let tone = if is_error { Tone::Error } else { Tone::Accent };
    // Its items report keyboard focus, which holds the message.
    ui.report_focus(true);
    let mut close = false;
    ui.banner(tone, |ui| {
        ui.row(|ui| {
            ui.next().grow();
            if is_error {
                ui.error_text(&text);
            } else {
                ui.label(&text);
            }
            shrinkable(ui);
            match app.message_action.clone() {
                Some(MessageAction::Show(view)) => {
                    if ui.link("Show").clicked() {
                        app.navigate(view);
                    }
                }
                Some(MessageAction::Review(view)) => {
                    if ui.link("Review").clicked() {
                        app.clear_message();
                        app.navigate(view);
                    }
                }
                Some(MessageAction::Undo) if ui.link("Undo").clicked() => app.undo(),
                Some(MessageAction::Undo) => {}
                None => {}
            }
            if app.message_details.is_some() {
                let open = app.message_details_open;
                if ui
                    .link(if open { "Hide details" } else { "Details" })
                    .clicked()
                {
                    app.message_details_open = !open;
                }
            }
            // F6 lands on × (the links take the pointer only, as elsewhere).
            if focus_first {
                ui.focus();
            }
            ui.tip("Dismiss");
            if ui.icon_button("close", "Dismiss", false).clicked() {
                close = true;
            }
        });
        if app.message_details_open
            && let Some(d) = app.message_details.clone()
        {
            ui.secondary(d);
            shrinkable(ui);
        }
    });
    ui.report_focus(false);
    if close {
        close_from_popup(app);
    }
}

/// The main area: the current view.
pub(crate) fn main_view(app: &mut App, ui: &mut Ui) {
    match app.view.clone() {
        View::Settings => super::settings::view(app, ui),
        View::Register { account } | View::Reconcile { account }
            if app.resolve_account(&account).is_none() =>
        {
            let reconcile = matches!(app.view, View::Reconcile { .. });
            choose_account(app, ui, reconcile);
        }
        View::Register { account } if super::investments::is_brokerage(app, &account) => {
            super::investments::view(app, ui, &account);
        }
        View::Register { account } => super::register::view(app, ui, &account),
        View::Securities => super::investments::securities_view(app, ui),
        View::Reconcile { account } => super::reconcile::view(app, ui, &account),
        View::Import { batch } => super::import::view(app, ui, &batch),
        View::Scheduled => super::scheduled::view(app, ui),
        View::Accounts => super::accounts::view(app, ui),
        View::History => super::history::view(app, ui),
        View::Reports => super::reports::view(app, ui),
        View::Home => {
            super::backups::home_note(app, ui);
            super::dashboard::view(app, ui, None);
        }
        View::Budget => super::budget::view(app, ui),
        View::Sales => super::sales::view(app, ui),
        View::Purchases => super::purchases::view(app, ui),
        View::Expenses => super::expenses::view(app, ui),
        View::Mileage => super::mileage::view(app, ui),
        View::Subscriptions => super::subscriptions::view(app, ui),
        View::Projects => super::projects::view(app, ui),
        View::Dashboard { id } => super::dashboard::view(app, ui, Some(id)),
    }
    if !matches!(app.view, View::Scheduled) {
        super::scheduled::dialogs(app, ui);
    }
    // Set project… opens from a register too (projects.md).
    if !matches!(app.view, View::Projects) {
        super::projects::dialogs(app, ui);
    }
    super::dashboard::dialogs(app, ui);
    super::receipts::dialogs(app, ui);
    super::iif::export_dialog(app, ui);
    super::definitions::dialogs(app, ui);
    super::connections::dialogs(app, ui);
    super::capture::dialogs(app, ui);
    super::settings::reset_dialog(app, ui);
    super::ai::dialogs(app, ui);
    super::opening::dialogs(app, ui);
    super::inv_more::dialogs(app, ui);
}

/// In place of a view that needs an account it does not have.
fn choose_account(app: &mut App, ui: &mut Ui, reconcile: bool) {
    let (title, message) = if reconcile {
        ("Reconcile", "Choose an account to reconcile")
    } else {
        ("Register", "Choose an account to open its register")
    };
    ui.title(title);
    let list = super::util::array(app.read("account.list", serde_json::json!({})));
    ui.card(message, |ui| {
        for a in list.iter().filter(|a| {
            a.get("placeholder").and_then(serde_json::Value::as_bool) != Some(true)
                && (!reconcile
                    || matches!(
                        a.get("type").and_then(serde_json::Value::as_str),
                        Some("asset" | "liability")
                    ))
        }) {
            let name = a
                .get("full_name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let id = a
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned();
            if ui.link(name).clicked() {
                app.view = if reconcile {
                    View::Reconcile { account: id }
                } else {
                    View::Register { account: id }
                };
            }
        }
    });
}
