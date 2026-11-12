//! The toolkit-free controller (ui-slint.md revision 2).
//!
//! [`App`] holds the application state and talks to the book only through a
//! [`Backend`] (registry commands). Each refresh it describes the window as a
//! [`Vm`]: the start screen or the main window's top bar, message band, accounts
//! rail and pane, the current page and its dialogs. `mk-views` shows that with
//! `.slint` components and reports what the user does with [`App::act`] and
//! [`App::key`]. Nothing here depends on Slint.

pub mod ui;

mod about;
mod accounts;
pub mod ai;
mod backups;
mod budget;
mod calculator;
mod capture;
mod capture_mac;
mod capture_setup;
mod capture_start_over;
mod capture_wifi;
mod charts;
mod connections;
mod corporate;
mod dashboard;
mod definitions;
mod expenses;
mod form;
mod history;
mod iif;
mod import;
mod inv_import;
mod inv_more;
mod investments;
mod lists;
mod mileage;
mod opening;
mod period_table;
mod picture;
mod popup;
mod presets;
mod projects;
mod purchases;
mod rail;
mod receipts;
mod reconcile;
mod register;
mod reports;
mod sales;
mod scheduled;
mod scheduled_hooks;
mod security;
mod settings;
mod shell;
mod start;
mod statements;
mod subscriptions;
mod tags;
mod templates;
pub(crate) mod util;

use std::collections::HashMap;
use std::rc::Rc;

use serde_json::{Value, json};

use crate::base::recent;
use crate::{
    AppEvent, Backend, DateOrder, FontProvider, PickedFile, ReceiptTarget, RecentBook, Screen,
    TileIcon, UiError, UserConfig, View,
};
pub use ui::{Action, Ev, Item, Page, Picture, Section, Ui};
use util::Json;

/// The accounts pane as the window shows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PaneVm {
    pub open: bool,
    pub pinned: bool,
    pub title: String,
    /// Width in pixels.
    pub width: i32,
    pub tools: Vec<Item>,
    pub sections: Vec<Section>,
}

/// Display settings the window applies (revision 7, revision 27).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DisplayVm {
    /// 0 follow the system, 1 light, 2 dark.
    pub theme: i32,
    /// Text size in percent (the window scale factor follows it).
    pub text_size: u32,
    pub ui_font: Option<String>,
    pub number_font: Option<String>,
    pub scroll_width: u32,
}

/// The calculator window (revision 25).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CalcVm {
    pub open: bool,
    pub display: String,
    pub expression: String,
    /// The display holds an error message (not a number).
    pub error: bool,
    pub x: i32,
    pub y: i32,
    pub pinned: bool,
    pub focus: i32,
}

/// Everything the window shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Vm {
    pub main: bool,
    pub page: Page,
    pub top: Vec<Item>,
    pub banner: Vec<Section>,
    /// The status message pop-up shows above the open dialog's scrim: it
    /// appeared while that dialog was open (app-desktop.md revision 42).
    pub message_over_dialog: bool,
    pub rail: Vec<Item>,
    pub pane: PaneVm,
    pub calc: CalcVm,
    pub display: DisplayVm,
    /// Changes when keyboard focus should go back to the window (no field).
    pub focus_root: i32,
}

/// A link shown after the banner message.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub(crate) enum MessageAction {
    Show(View),
    /// The same as Show, worded "Review" (staged feed rows).
    Review(View),
    Undo,
}

/// The whole UI state. The app crate owns one inside `mk-views`' window.
pub struct App {
    pub(crate) backend: Box<dyn Backend>,
    pub config: UserConfig,
    pub(crate) screen: Screen,
    pub(crate) view: View,
    pub(crate) back: Vec<View>,
    pub(crate) forward: Vec<View>,
    pub(crate) message: Option<(String, bool)>,
    pub(crate) message_action: Option<MessageAction>,
    pub(crate) message_details: Option<String>,
    pub(crate) message_details_open: bool,
    /// When the status message appeared, or the pointer or focus last left it
    /// (revision 37: it closes by itself that long after).
    pub(crate) message_since: f64,
    /// The pointer is over the message pop-up.
    pub(crate) message_hover: bool,
    /// Where F6 took keyboard focus from into the message pop-up (it goes back
    /// there on F6, Esc or ×).
    pub(crate) message_return_focus: Option<String>,
    /// The message the last pass showed, and whether it appeared while a dialog
    /// was open (it then shows above that dialog's scrim).
    message_seen: Option<(String, bool)>,
    message_over_dialog: bool,
    events: Vec<AppEvent>,
    copied: Vec<String>,
    pub(crate) start: start::StartState,
    pub(crate) security: security::SecurityState,
    pub(crate) backups: backups::BackupState,
    pub(crate) ai: ai::AiState,
    pub(crate) settings: settings::SettingsState,
    pub(crate) pane: rail::PaneState,
    pub(crate) open_errors: Vec<(String, String)>,
    pub(crate) generation: u64,
    today_override: Option<String>,
    pub(crate) undo_hint: Option<String>,
    pub(crate) redo_hint: Option<String>,
    pub(crate) profile_name: Option<String>,
    pub(crate) about_open: bool,
    pub watching: bool,
    budgets_on: (u64, bool),
    /// Tile pictures read from the app's cache, by book path.
    pub(crate) tile_pictures: HashMap<String, Option<Rc<Picture>>>,
    /// The Picture editor, while open (revision 40).
    pub(crate) picture: Option<picture::Editor>,
    /// The item with keyboard focus (as the views report it).
    pub(crate) focused: Option<String>,
    focus_want: Option<(String, i32)>,
    select_want: Option<(String, i32)>,
    serial: i32,
    focus_root: i32,
    /// The field whose text was selected and not typed into since.
    all_selected: Option<String>,
    /// Seconds on the controller's clock (the window's timer or the tests).
    pub(crate) now: f64,
    pub(crate) config_dirty_since: Option<f64>,
    pub(crate) fonts: settings::FontState,
    pub(crate) schedule_failures: Vec<String>,
    /// Later on the capture server update notice (release-packaging.md): the Home
    /// Attention item stays hidden until the app is opened again (not per book).
    pub(crate) capture_update_later: bool,
    pub(crate) calc: calculator::CalcState,
    pub(crate) register: register::RegisterState,
    pub(crate) reconcile: reconcile::ReconcileState,
    pub(crate) import: import::ImportState,
    pub(crate) accounts: accounts::AccountsState,
    pub(crate) scheduled: scheduled::ScheduledState,
    pub(crate) reports: reports::ReportsState,
    pub(crate) dash: dashboard::DashState,
    pub(crate) budget: budget::BudgetState,
    pub(crate) sales: sales::SalesState,
    pub(crate) purchases: purchases::PurchasesState,
    pub(crate) expenses: expenses::ExpensesState,
    pub(crate) mileage: mileage::MileageState,
    pub(crate) subs: subscriptions::SubsState,
    pub(crate) projects: projects::ProjectsState,
    pub(crate) receipts: receipts::ReceiptsState,
    pub(crate) statements: statements::StatementsState,
    pub(crate) inv: investments::InvState,
    pub(crate) conn: connections::ConnState,
    pub(crate) capture: capture::CaptureState,
    pub(crate) defs: definitions::DefState,
    pub(crate) templates: templates::TemplatesState,
    /// Opening balances: the wizards, the checklist and their help.
    pub(crate) opening: opening::OpeningState,
    /// Expenses: open on the Inbox tab; open a new expense (from a shortcut).
    pub(crate) expenses_tab_inbox: bool,
    pub(crate) expenses_new: bool,
    /// Window size in logical pixels (chart geometry, the calculator's place).
    pub(crate) window_width: i32,
    pub(crate) window_height: i32,
    /// Feature modules on, per generation.
    modules: (u64, HashMap<String, bool>),
    pub(crate) iif: iif::IifState,
    pub(crate) lists: lists::Lists,
    /// The built-in starter charts (chart-presets.md), read once.
    pub(crate) chart_presets: Option<Vec<Value>>,
    /// The Add accounts from a chart dialog.
    pub(crate) add_chart: Option<presets::AddChart>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("screen", &self.screen)
            .field("view", &self.view)
            .finish_non_exhaustive()
    }
}

/// Commands that change the book (the generation goes up after them).
pub(crate) fn is_mutating(command: &str) -> bool {
    crate::base::is_mutating_command(command)
}

impl App {
    pub fn new(backend: Box<dyn Backend>, config: UserConfig) -> Self {
        let screen = if backend.book_name().is_some() {
            Screen::Main
        } else {
            Screen::Start
        };
        let pane = rail::PaneState::from_config(&config.accounts_pane);
        let calc = calculator::CalcState::from_config(&config.calculator);
        let mut app = App {
            backend,
            config,
            screen,
            view: View::Home,
            back: Vec::new(),
            forward: Vec::new(),
            message: None,
            message_action: None,
            message_details: None,
            message_details_open: false,
            message_since: 0.0,
            message_hover: false,
            message_return_focus: None,
            message_seen: None,
            message_over_dialog: false,
            events: Vec::new(),
            copied: Vec::new(),
            start: start::StartState::default(),
            security: security::SecurityState::default(),
            backups: backups::BackupState::default(),
            ai: ai::AiState::default(),
            settings: settings::SettingsState::default(),
            pane,
            open_errors: Vec::new(),
            generation: 1,
            today_override: None,
            undo_hint: None,
            redo_hint: None,
            profile_name: None,
            about_open: false,
            watching: false,
            budgets_on: (0, false),
            tile_pictures: HashMap::new(),
            picture: None,
            focused: None,
            focus_want: None,
            select_want: None,
            serial: 0,
            focus_root: 0,
            all_selected: None,
            now: 0.0,
            config_dirty_since: None,
            fonts: settings::FontState::default(),
            schedule_failures: Vec::new(),
            capture_update_later: false,
            calc,
            register: register::RegisterState::default(),
            reconcile: reconcile::ReconcileState::default(),
            import: import::ImportState::default(),
            accounts: accounts::AccountsState::default(),
            scheduled: scheduled::ScheduledState::default(),
            reports: reports::ReportsState::default(),
            dash: dashboard::DashState::default(),
            budget: budget::BudgetState::default(),
            sales: sales::SalesState::default(),
            purchases: purchases::PurchasesState::default(),
            expenses: expenses::ExpensesState::default(),
            mileage: mileage::MileageState::default(),
            subs: subscriptions::SubsState::default(),
            projects: projects::ProjectsState::default(),
            receipts: receipts::ReceiptsState::default(),
            statements: statements::StatementsState::default(),
            inv: investments::InvState::default(),
            conn: connections::ConnState::default(),
            capture: capture::CaptureState::default(),
            defs: definitions::DefState::default(),
            templates: templates::TemplatesState::default(),
            opening: opening::OpeningState::default(),
            expenses_tab_inbox: false,
            expenses_new: false,
            window_width: 1400,
            window_height: 900,
            modules: (u64::MAX, HashMap::new()),
            iif: iif::IifState::default(),
            lists: lists::Lists::default(),
            chart_presets: None,
            add_chart: None,
        };
        app.config.text_size = crate::base::display::clamp_size(app.config.text_size);
        app.config.scroll_bar_width = crate::theme::clamp_scroll_width(app.config.scroll_bar_width);
        app.sync_dates();
        backups::push_settings(&mut app);
        if app.screen == Screen::Main {
            app.after_open();
        }
        app
    }

    // ----- small state helpers -----------------------------------------------------

    /// The chosen date style (revision 20).
    pub(crate) fn date_style(&self) -> mk_core::datefmt::DateStyle {
        mk_core::datefmt::DateStyle::parse(&self.config.date_format).unwrap_or_default()
    }

    /// Hand the date style and today's date to the formatting helpers.
    pub(crate) fn sync_dates(&self) {
        crate::fmt::set_dates(self.date_style(), self.today().parse().ok());
    }

    /// Run a command through the backend without going through a view (tests'
    /// setup and the app shell).
    pub fn test_invoke(&mut self, command: &str, args: Value) -> Result<Value, UiError> {
        self.run(command, args)
    }

    /// Use `date` (YYYY-MM-DD) as today instead of the clock (tests, reviews).
    pub fn set_today(&mut self, date: &str) {
        self.today_override = Some(date.to_owned());
        self.generation += 1;
        self.sync_dates();
    }

    /// Today's date, YYYY-MM-DD.
    pub(crate) fn today(&self) -> String {
        self.today_override
            .clone()
            .unwrap_or_else(crate::base::today_iso)
    }

    /// Whether the budgets module is on (cached per generation).
    pub(crate) fn budgets_enabled(&mut self) -> bool {
        if self.budgets_on.0 != self.generation {
            let on = self.module_on("budgets");
            self.budgets_on = (self.generation, on);
        }
        self.budgets_on.1
    }

    /// Whether a feature module is on.
    pub(crate) fn module_on(&mut self, module: &str) -> bool {
        if self.modules.0 != self.generation {
            self.modules = (self.generation, HashMap::new());
        }
        if let Some(on) = self.modules.1.get(module) {
            return *on;
        }
        let on = self
            .backend
            .invoke("setting.get", json!({"module": module, "key": "enabled"}))
            .ok()
            .is_some_and(|v| v.at("value").as_bool() == Some(true));
        self.modules.1.insert(module.to_owned(), on);
        on
    }

    /// Tell the controller the window's width in logical pixels.
    pub fn set_window_width(&mut self, width: i32) {
        self.window_width = width.max(320);
    }

    /// Tell the controller the window's height in logical pixels.
    pub fn set_window_height(&mut self, height: i32) {
        self.window_height = height.max(240);
    }

    /// The width a chart gets in the main area beside a side list of `side` pixels.
    pub(crate) fn chart_width(&self, side: i32) -> i32 {
        let side = if side > 0 { side + 16 } else { 0 };
        (self.window_width - 64 - 48 - 32 - side).max(240)
    }

    /// Events for the app shell since the last call.
    pub fn take_events(&mut self) -> Vec<AppEvent> {
        std::mem::take(&mut self.events)
    }

    /// Text the UI copied for the system clipboard since the last call.
    pub fn take_copied(&mut self) -> Vec<String> {
        std::mem::take(&mut self.copied)
    }

    /// Text copied so far (not taken).
    pub fn copied_so_far(&self) -> Vec<String> {
        self.copied.clone()
    }

    pub(crate) fn copy(&mut self, text: impl Into<String>) {
        self.copied.push(text.into());
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    pub fn book_name(&self) -> Option<String> {
        self.backend.book_name()
    }

    /// Set the font provider (the app crate lists installed fonts with it).
    pub fn set_font_provider(&mut self, provider: Box<dyn FontProvider>) {
        self.fonts.set_provider(provider);
    }

    /// Text size in percent.
    pub fn text_size(&self) -> u32 {
        crate::base::display::clamp_size(self.config.text_size)
    }

    /// Scroll bar width in pixels.
    pub fn scroll_bar_width(&self) -> u32 {
        crate::theme::clamp_scroll_width(self.config.scroll_bar_width)
    }

    /// The fonts in use (interface, number); `None` for the built-in ones.
    pub fn active_fonts(&self) -> (Option<String>, Option<String>) {
        self.fonts.active(&self.config)
    }

    /// Take keyboard focus away from every field (tests use this to leave a field).
    pub fn blur_for_tests(&mut self) {
        self.focused = None;
        self.focus_want = None;
        self.focus_root += 1;
    }

    /// The item that has keyboard focus, if the views reported one.
    pub fn focused_item(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    /// Ask the views to focus item `id`.
    pub fn request_focus(&mut self, id: &str) {
        self.serial += 1;
        self.focus_want = Some((id.to_owned(), self.serial));
    }

    /// Ask the views to select the text of item `id`.
    pub fn request_select(&mut self, id: &str) {
        self.serial += 1;
        self.select_want = Some((id.to_owned(), self.serial));
        self.all_selected = Some(id.to_owned());
    }

    /// Whether the whole text of field `id` is selected (typing replaces it).
    pub fn is_all_selected(&self, id: &str) -> bool {
        self.all_selected.as_deref() == Some(id)
    }

    // ----- commands and messages ---------------------------------------------------

    /// Run a command; on success bump the generation when it changes the book.
    pub(crate) fn run(&mut self, command: &str, args: Value) -> Result<Value, UiError> {
        let out = self.backend.invoke(command, args)?;
        if is_mutating(command) {
            self.generation += 1;
            self.refresh_history_hints();
            if command.starts_with("book.") || command.starts_with("history.") {
                self.refresh_tile();
            }
        }
        Ok(out)
    }

    /// The id of an account named by id, full name or code.
    pub(crate) fn resolve_account(&mut self, r: &str) -> Option<String> {
        if r.trim().is_empty() {
            return None;
        }
        self.backend
            .invoke("account.get", json!({"account": r}))
            .ok()
            .map(|a| a.str("id").to_owned())
    }

    /// Open a view from a short text form (review aid and screenshots):
    /// `home`, `accounts`, `settings`, `register:<account>`, ….
    pub fn open_view_spec(&mut self, spec: &str) {
        let (kind, arg) = spec.split_once(':').unwrap_or((spec, ""));
        let account = self.resolve_account(arg).unwrap_or_default();
        let view = match kind {
            "accounts" => View::Accounts,
            "settings" => View::Settings,
            "history" => View::History,
            "scheduled" => View::Scheduled,
            "budget" => View::Budget,
            "reports" => View::Reports,
            "securities" => View::Securities,
            "sales" => View::Sales,
            "purchases" => View::Purchases,
            "expenses" => View::Expenses,
            "mileage" => View::Mileage,
            "subscriptions" => {
                self.subs.detail = (!arg.is_empty()).then(|| arg.to_owned());
                View::Subscriptions
            }
            "projects" => {
                self.projects.detail = (!arg.is_empty()).then(|| arg.to_owned());
                View::Projects
            }
            "dashboard" => View::Dashboard { id: arg.to_owned() },
            "register" => View::Register { account },
            "reconcile" => View::Reconcile { account },
            "import" if !arg.is_empty() => View::Import {
                batch: arg.to_owned(),
            },
            _ => View::Home,
        };
        self.navigate(view);
    }

    /// Run a command for reading; errors become the banner message.
    pub(crate) fn read(&mut self, command: &str, args: Value) -> Option<Value> {
        match self.backend.invoke(command, args) {
            Ok(v) => Some(v),
            Err(e) => {
                self.notify_error(&e);
                None
            }
        }
    }

    /// Run a reading command; its error as text instead of a notice.
    pub(crate) fn read_or_error(&mut self, command: &str, args: Value) -> Result<Value, String> {
        self.backend.invoke(command, args).map_err(|e| e.display())
    }

    pub(crate) fn notify(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), false));
        self.message_action = None;
        self.message_details = None;
        self.message_since = self.now;
    }

    /// Tests: show a status message (an error when `error`) as an action would.
    pub fn notify_for_tests(&mut self, text: &str, error: bool) {
        if error {
            self.notify_error(&UiError::new("test", text));
        } else {
            self.notify(text);
        }
    }

    /// Tests and screenshots: a status message with Details (closed) and an Undo
    /// link, as an action would show it.
    pub fn notify_full_for_tests(
        &mut self,
        text: &str,
        error: bool,
        details: Option<&str>,
        undo: bool,
    ) {
        self.notify_for_tests(text, error);
        self.message_details = details.map(str::to_owned);
        self.message_details_open = false;
        if undo {
            self.message_action = Some(MessageAction::Undo);
        }
    }

    /// The status message on show and whether it is an error.
    pub fn message_for_tests(&self) -> Option<(String, bool)> {
        self.message.clone()
    }

    pub(crate) fn notify_with(&mut self, text: impl Into<String>, action: MessageAction) {
        self.notify(text);
        self.message_action = Some(action);
    }

    pub(crate) fn notify_error(&mut self, e: &UiError) {
        self.message = Some((e.display(), true));
        self.message_action = None;
        self.message_details = None;
        self.message_since = self.now;
    }

    pub(crate) fn clear_message(&mut self) {
        self.message = None;
        self.message_action = None;
        self.message_details = None;
    }

    /// A book file could not be created or opened: plain words naming the path and
    /// the likely cause; the underlying error under "Details".
    fn notify_file_error(&mut self, creating: bool, path: &str, e: &UiError) {
        if e.code != "io" {
            self.notify_error(e);
            return;
        }
        let text = if creating {
            let cause = if cfg!(windows) {
                "Windows or another program may be blocking MoneyKeep from writing to this \
                 folder (for example Controlled folder access)."
            } else {
                "Check that you can write to this folder."
            };
            format!(
                "Couldn't create the book at {path}. {cause} Choose another folder, or allow \
                 MoneyKeep in your security settings."
            )
        } else {
            format!(
                "Couldn't open the book at {path}. Check that the file is still there and that \
                 you can read and write its folder."
            )
        };
        self.message = Some((text, true));
        self.message_action = None;
        self.message_details = Some(e.message.clone());
        self.message_since = self.now;
    }

    pub(crate) fn emit(&mut self, e: AppEvent) {
        self.events.push(e);
    }

    pub fn navigate(&mut self, view: View) {
        if view != self.view {
            self.back.push(std::mem::replace(&mut self.view, view));
            self.forward.clear();
        }
    }

    pub(crate) fn go_back(&mut self) {
        if let Some(v) = self.back.pop() {
            self.forward.push(std::mem::replace(&mut self.view, v));
        }
    }

    pub(crate) fn go_forward(&mut self) {
        if let Some(v) = self.forward.pop() {
            self.back.push(std::mem::replace(&mut self.view, v));
        }
    }

    // ----- book lifecycle (called by the app shell) ---------------------------------

    /// Create a book at `path` (after the save dialog) and show the main window.
    pub fn create_book(&mut self, path: &str, name: &str, currency: &str) {
        let description = std::mem::take(&mut self.start.description);
        // The chart chosen in the Create a new book dialog (chart-presets.md); a book
        // created without the dialog starts with Opening Balances only.
        let preset = self.start.preset.take();
        let preset = preset.as_deref();
        if self.start.protect {
            let password = std::mem::take(&mut self.start.password);
            self.start.confirm.clear();
            let recovery = !self.start.no_recovery;
            match self
                .backend
                .create_protected_book(path, name, currency, &password, recovery, preset)
            {
                Ok(key) => {
                    self.start.protect = false;
                    self.set_new_description(&description);
                    backups::creating(self);
                    self.opened(path);
                    if let Some(k) = key {
                        self.security.recovery_key = Some((k, false));
                    }
                }
                Err(e) => self.notify_file_error(true, path, &e),
            }
            return;
        }
        match self.backend.create_book(path, name, currency, preset) {
            Ok(()) => {
                self.set_new_description(&description);
                backups::creating(self);
                self.opened(path);
            }
            Err(e) => self.notify_file_error(true, path, &e),
        }
    }

    fn set_new_description(&mut self, description: &str) {
        let tagline = self.start.new_tagline.take();
        let mut args = serde_json::Map::new();
        if !description.trim().is_empty() {
            args.insert("description".into(), json!(description));
        }
        if let Some(t) = tagline {
            args.insert("tagline".into(), json!(t));
        }
        self.start.tagline.clear();
        if args.is_empty() {
            return;
        }
        if let Err(e) = self
            .backend
            .invoke("book.update_profile", Value::Object(args))
        {
            self.notify_error(&e);
        }
    }

    /// A book is open: remember it and show the main window.
    pub(crate) fn opened(&mut self, path: &str) {
        self.open_errors.retain(|(p, _)| p != path);
        self.remember(path);
        self.security.book_path = Some(path.to_owned());
        self.screen = Screen::Main;
        self.after_open();
        backups::on_opened(self);
        self.emit(AppEvent::BookOpened {
            path: path.to_owned(),
        });
    }

    /// Open the book at `path` and show the main window. On failure the path stays
    /// in the recent list with its error.
    pub fn open_book(&mut self, path: &str) {
        if self.backend.is_protected_file(path) == Some(true) {
            if self.screen == Screen::Main {
                self.close_book();
            }
            self.prompt_unlock(path);
            return;
        }
        if self.screen == Screen::Main {
            backups::before_close(self);
        }
        match self.backend.open_book(path) {
            Ok(()) => self.opened(path),
            Err(e) if e.code == "backup_failed" => backups::open_failed(self, path, None, &e),
            Err(e) => {
                self.open_errors.retain(|(p, _)| p != path);
                self.open_errors.push((path.to_owned(), e.display()));
                self.notify_file_error(false, path, &e);
            }
        }
    }

    /// The folder holding the settings file (shown by Reset MoneyKeep settings,
    /// app-desktop.md revision 41).
    pub fn set_settings_folder(&mut self, folder: impl Into<String>) {
        self.settings.folder = Some(folder.into());
    }

    /// Reset MoneyKeep settings (app-desktop.md revision 41), after the app shell
    /// tried to save the current settings file: `Ok(where the copy is)` resets the
    /// app's own settings (books, backup files, book contents and phone capture
    /// are not touched; what the app remembers about backups stays) and returns to
    /// the start screen as on first run; `Err(why)` changes nothing.
    pub fn finish_settings_reset(&mut self, saved: Result<String, String>) {
        let saved = match saved {
            Ok(p) => p,
            Err(e) => {
                self.notify_error(&UiError::new(
                    "reset_failed",
                    format!("MoneyKeep settings were not reset: the current settings could not be saved first ({e})."),
                ));
                return;
            }
        };
        if self.screen == Screen::Main {
            self.close_book();
        }
        let memo = std::mem::take(&mut self.config.backup_memo);
        self.config = UserConfig {
            backup_memo: memo,
            ..UserConfig::default()
        };
        self.pane = rail::PaneState::from_config(&self.config.accounts_pane);
        self.calc = calculator::CalcState::from_config(&self.config.calculator);
        self.tile_pictures.clear();
        self.start = start::StartState::default();
        let folder = self.settings.folder.take();
        self.settings = settings::SettingsState {
            folder,
            ..settings::SettingsState::default()
        };
        self.open_errors.clear();
        self.screen = Screen::Start;
        self.view = View::Home;
        self.back.clear();
        self.forward.clear();
        self.generation += 1;
        self.emit(AppEvent::ConfigChanged);
        self.notify(format!(
            "MoneyKeep settings were reset. The old settings were saved as {saved}."
        ));
    }

    pub fn close_book(&mut self) {
        if self.screen == Screen::Main {
            backups::before_close(self);
        }
        self.backend.close_book();
        self.emit(AppEvent::BookClosed);
        self.ai = ai::AiState {
            keep_mcp: self.ai.keep_mcp.take(),
            ..ai::AiState::default()
        };
        self.security.book_path = None;
        self.profile_name = None;
        self.settings.profile = settings::ProfileState::default();
        self.picture = None;
        self.screen = Screen::Start;
        self.view = View::Home;
        self.back.clear();
        self.forward.clear();
        if self.watching {
            self.watching = false;
            self.emit(AppEvent::Watch(false));
        }
    }

    fn remember(&mut self, path: &str) {
        let tile = self
            .config
            .recent
            .iter()
            .find(|r| r.path == path)
            .and_then(|r| r.tile.clone());
        self.config.recent.retain(|r| r.path != path);
        self.config.recent.insert(
            0,
            RecentBook {
                path: path.to_owned(),
                tile,
                last_opened: Some(self.today()),
            },
        );
        for gone in self
            .config
            .recent
            .split_off(10.min(self.config.recent.len()))
        {
            self.backend.save_tile_image(&gone.path, None);
        }
        self.emit(AppEvent::ConfigChanged);
    }

    /// Refresh the open book's profile name and its tile cache (revision 26).
    pub(crate) fn refresh_tile(&mut self) {
        let Ok(info) = self.backend.invoke("book.info", json!({})) else {
            return;
        };
        self.profile_name = info.get("name").and_then(Value::as_str).map(str::to_owned);
        let Some(path) = self.security.book_path.clone() else {
            return;
        };
        let Some(tile) = recent::tile_from_info(&info) else {
            return;
        };
        let image = if tile.icon == TileIcon::Image {
            use base64::Engine;
            info.at("icon")
                .get("image_png_base64")
                .and_then(Value::as_str)
                .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        } else {
            None
        };
        self.backend.save_tile_image(&path, image.as_deref());
        self.tile_pictures.remove(&path);
        if let Some(entry) = self.config.recent.iter_mut().find(|r| r.path == path)
            && entry.tile.as_ref() != Some(&tile)
        {
            entry.tile = Some(tile);
            self.emit(AppEvent::ConfigChanged);
        }
    }

    /// Remove a book from the recent list with its tile cache.
    pub(crate) fn forget_recent(&mut self, path: &str) {
        self.config.recent.retain(|r| r.path != path);
        self.open_errors.retain(|(p, _)| p != path);
        self.backend.save_tile_image(path, None);
        self.tile_pictures.remove(path);
        self.emit(AppEvent::ConfigChanged);
    }

    /// Open the Picture editor on an image file (after
    /// [`AppEvent::ChooseBookIconFile`], revision 40); a file that is not a usable
    /// picture shows its error in Settings → Book.
    pub fn set_book_icon(&mut self, bytes: &[u8]) {
        settings::choose_picture(self, bytes);
    }

    /// The open Picture editor's treated preview (RGBA, without the brush tint) and
    /// its settings, as `book.set_picture` would take them (tests and review).
    pub fn picture_editor_preview(&self) -> Option<(Option<Rc<Picture>>, Value)> {
        picture::snapshot(self)
    }

    /// Open the Picture editor on the book's stored picture, as Edit picture… does
    /// (review aid and screenshots).
    pub fn edit_book_picture(&mut self) -> Result<(), UiError> {
        picture::open_stored(self)
    }

    fn after_open(&mut self) {
        capture::reset(self);
        self.conn.last_check = None;
        self.refresh_tile();
        self.view = View::Home;
        self.generation += 1;
        self.clear_message();
        self.run_due();
        self.refresh_history_hints();
    }

    /// Enter due occurrences of auto-enter schedules.
    fn run_due(&mut self) {
        self.schedule_failures.clear();
        let today = self.today();
        let Ok(r) = self
            .backend
            .invoke("schedule.run_due", json!({"today": today}))
        else {
            return;
        };
        let n = r.list("entered").len();
        for f in r.list("failed") {
            self.schedule_failures.push(format!(
                "Couldn't enter {} due {}: {}",
                f.str("name"),
                crate::fmt::date(f.str("due")),
                f.at("error").str("message")
            ));
        }
        if n > 0 {
            self.generation += 1;
            let text = if n == 1 {
                "Entered 1 scheduled transaction".to_owned()
            } else {
                format!("Entered {n} scheduled transactions")
            };
            self.notify_with(text, MessageAction::Show(View::History));
        }
    }

    /// The watch loop reported files; refresh what depends on staged batches.
    pub fn watch_reported(&mut self, text: &str) {
        self.generation += 1;
        self.notify(text.to_owned());
    }

    pub(crate) fn refresh_history_hints(&mut self) {
        let Ok(list) = self
            .backend
            .invoke("history.list", json!({"limit": 200, "actor": "app"}))
        else {
            return;
        };
        let rows = list.as_array().cloned().unwrap_or_default();
        let cmd = |e: &Value| crate::base::describe_command(e.str("command"));
        self.undo_hint = rows
            .iter()
            .find(|e| e.at("undoes").is_null() && e.at("undone_by").is_null())
            .map(cmd);
        self.redo_hint = rows
            .iter()
            .find(|e| e.at("redoes").is_null())
            .filter(|e| !e.at("undoes").is_null())
            .and_then(|e| e.at("undoes").as_i64())
            .and_then(|seq| rows.iter().find(|x| x.at("seq").as_i64() == Some(seq)))
            .map(cmd);
    }

    pub(crate) fn undo(&mut self) {
        match self.run("history.undo", json!({})) {
            Ok(v) => self.notify(format!(
                "Undid {}",
                crate::base::describe_command(v.str("command"))
            )),
            Err(e) => self.notify_error(&e),
        }
    }

    pub(crate) fn redo(&mut self) {
        match self.run("history.redo", json!({})) {
            Ok(v) => self.notify(format!(
                "Redid {}",
                crate::base::describe_command(v.str("command"))
            )),
            Err(e) => self.notify_error(&e),
        }
    }

    // ----- files from the app shell -------------------------------------------------

    /// Stage an import file the app read (dialog or drop) and open its review.
    pub fn import_file(&mut self, name: &str, bytes: &[u8]) {
        if mk_core::brand::is_definition_file(name) {
            self.open_definition(name, bytes);
        } else {
            import::stage(self, name, bytes, None);
        }
    }

    /// Open the install review for a definition file (dialog or drop).
    pub fn open_definition(&mut self, name: &str, bytes: &[u8]) {
        definitions::open_file(self, name, bytes);
    }

    /// Files dropped on the window.
    pub fn drop_files(&mut self, files: Vec<PickedFile>) {
        if self.screen == Screen::Main
            && self.module_on(expenses::MODULE)
            && let Some(target) = receipts::drop_target(self)
        {
            receipts::add_files(self, target, files);
            return;
        }
        for f in files {
            if self.screen != Screen::Main {
                self.notify("Open a book before importing files.");
                break;
            }
            self.import_file(&f.name.clone(), &f.bytes.unwrap_or_default());
        }
    }

    /// Set a document template's logo from an image file (after
    /// [`AppEvent::ChooseLogoFile`]); a refusal shows under the logo.
    pub fn set_template_logo(&mut self, template: &str, bytes: &[u8]) {
        templates::set_logo(self, template, bytes);
    }

    /// Import prices from the CSV file chosen after [`AppEvent::ChoosePriceFile`].
    pub fn import_prices(&mut self, name: &str, bytes: &[u8]) {
        investments::import_prices(self, name, bytes);
    }

    /// Add receipt files the app read after [`AppEvent::ChooseReceiptFiles`].
    pub fn add_receipt_files(&mut self, target: ReceiptTarget, files: Vec<PickedFile>) {
        receipts::add_files(self, target, files);
    }

    /// The expense report PDF was written after [`AppEvent::SaveExpenseReport`]:
    /// mark its expenses reported (one undo step).
    pub fn expense_report_saved(&mut self, expenses: &[String]) {
        expenses::report_saved(self, expenses);
    }

    /// Set the watched folder chosen in the folder picker.
    pub fn set_watch_folder(&mut self, path: &str) {
        settings::set_core(self, "import.watch_folder", json!(path));
    }

    // ----- the window ---------------------------------------------------------------

    /// The view model for the window.
    pub fn vm(&mut self) -> Vm {
        self.pass(None).0
    }

    /// The user did `ev` to item `id` (with `value`).
    pub fn act(&mut self, id: &str, ev: Ev, value: &str) {
        match ev {
            Ev::Focus => self.focused = Some(id.to_owned()),
            Ev::Blur if self.focused.as_deref() == Some(id) => {
                self.focused = None;
                if self.all_selected.as_deref() == Some(id) {
                    self.all_selected = None;
                }
            }
            Ev::Edit if self.all_selected.as_deref() == Some(id) => self.all_selected = None,
            _ => {}
        }
        if self.calc_act(id, value) {
            return;
        }
        self.pass(Some(Action::new(id, ev, value)));
    }

    /// A key no element took: `item` is the focused item that saw it first (empty
    /// for none), `name` the key ("ctrl+n", "escape"). Returns whether it was used.
    pub fn key(&mut self, item: &str, name: &str) -> bool {
        if name.is_empty() {
            return false;
        }
        if item == "calc/display" {
            return self.calc_key(name) || self.shortcut(name);
        }
        if item.is_empty() && self.shortcut(name) {
            return true;
        }
        self.pass(Some(Action::new(item, Ev::Key, name))).1
    }

    /// Advance the controller's clock to `now` seconds; returns whether the window
    /// should refresh.
    pub fn tick(&mut self, now: f64) -> bool {
        self.now = now;
        let mut changed = false;
        if self.security.unlock.as_ref().is_some_and(|u| u.unlocking) {
            security::finish_unlock(self);
            changed = true;
        }
        changed |= security::lock_if_idle(self);
        changed |= backups::tick(self);
        changed |= rail::tick(self);
        if self.screen == Screen::Main {
            // Background syncs and phone capture jobs, and their schedules.
            changed |= connections::poll(self);
            connections::scheduled(self);
            capture::scheduled(self);
            changed |= shell::autoclose(self);
        }
        changed |= self.flush_config();
        changed
    }

    /// Note user input (for the idle lock).
    pub fn note_input(&mut self) {
        self.security.last_input = Some(self.now);
    }

    /// Tests: pretend `by` passed without input (the idle lock's clock).
    #[allow(clippy::float_arithmetic)]
    pub fn advance_clock_for_tests(&mut self, by: std::time::Duration) {
        let last = self.security.last_input.unwrap_or(self.now);
        self.security.last_input = Some(last - by.as_secs_f64());
    }

    /// Persist debounced config changes (panel widths) after a short pause.
    fn flush_config(&mut self) -> bool {
        if let Some(t) = self.config_dirty_since
            && crate::layout::elapsed(self.now, t, 0.5)
        {
            self.config_dirty_since = None;
            self.emit(AppEvent::ConfigChanged);
            return true;
        }
        false
    }

    /// Build every part of the window, answering `action` on the way. Returns the
    /// view model and whether the action was answered.
    fn pass(&mut self, mut action: Option<Action>) -> (Vm, bool) {
        self.sync_dates();
        let mut used = false;
        let main = self.screen == Screen::Main;
        let mut vm = Vm {
            main,
            ..Vm::default()
        };
        if main {
            let (p, u) = self.build(&action, |app, ui| {
                ui.scope("top", |ui| shell::top_bar(app, ui))
            });
            vm.top = p
                .sections
                .into_iter()
                .flat_map(|s| s.rows)
                .flat_map(|r| r.items)
                .collect();
            used |= u;
            let (p, u) = self.build(&action, |app, ui| {
                ui.scope("msg", |ui| shell::banner(app, ui))
            });
            vm.banner = p.sections;
            used |= u;
            // A key the pop-up took (Esc, F6) is not seen again by the page.
            if u {
                action = None;
            }
            let (p, u) = self.build(&action, |app, ui| {
                ui.scope("rail", |ui| rail::tabs(app, ui))
            });
            vm.rail = p
                .sections
                .into_iter()
                .flat_map(|s| s.rows)
                .flat_map(|r| r.items)
                .collect();
            used |= u;
            let (p, u) = self.build(&action, |app, ui| {
                ui.scope("pane", |ui| rail::pane(app, ui))
            });
            vm.pane = PaneVm {
                open: self.pane.open,
                pinned: self.pane.pinned,
                title: p.title,
                width: self.pane_width(),
                tools: p.toolbar,
                sections: p.sections,
            };
            used |= u;
        }
        let (page, u) = self.build(&action, |app, ui| {
            match app.screen {
                Screen::Start => start::screen(app, ui),
                Screen::Main => shell::main_view(app, ui),
            }
            security::dialogs(app, ui);
            backups::dialogs(app, ui);
            if app.screen == Screen::Main {
                picture::dialog(app, ui);
            }
            about::dialog(app, ui);
        });
        used |= u;
        vm.page = page;
        if main {
            // A message that appears while a dialog is open shows above its scrim
            // (revision 42); one already shown stays under a dialog opened later.
            if vm.page.dialogs.is_empty() || self.message.is_none() {
                self.message_over_dialog = false;
            } else if self.message != self.message_seen {
                self.message_over_dialog = true;
            }
            self.message_seen = self.message.clone();
            vm.message_over_dialog = self.message_over_dialog;
        }
        vm.display = DisplayVm {
            theme: match self.config.theme {
                crate::Theme::System => 0,
                crate::Theme::Light => 1,
                crate::Theme::Dark => 2,
            },
            text_size: self.text_size(),
            ui_font: self.active_fonts().0,
            number_font: self.active_fonts().1,
            scroll_width: self.scroll_bar_width(),
        };
        vm.focus_root = self.focus_root;
        vm.calc = self.calc_vm();
        (vm, used)
    }

    /// Run one builder over the state.
    fn build(
        &mut self,
        action: &Option<Action>,
        f: impl FnOnce(&mut App, &mut Ui),
    ) -> (Page, bool) {
        // The chosen fonts are checked once (a missing one is said once).
        settings::check_fonts(self);
        let mut ui = Ui::new(action.clone());
        ui.want_focus(self.focus_want.clone());
        ui.want_select(self.select_want.clone());
        ui.focus_serial = self.serial + 1;
        f(self, &mut ui);
        let used = action.is_some() && !ui.acting();
        if let Some(id) = ui.focus_requests.last().cloned() {
            self.serial += 1;
            self.focus_want = Some((id, self.serial));
        }
        if let Some(id) = ui.select_requests.last().cloned() {
            self.serial += 1;
            self.select_want = Some((id.clone(), self.serial));
            self.all_selected = Some(id);
        }
        (ui.finish(), used)
    }

    /// Window-wide shortcuts (keys no item took).
    fn shortcut(&mut self, key: &str) -> bool {
        self.note_input();
        match key {
            "ctrl+=" | "ctrl+shift+=" | "ctrl++" | "ctrl+shift++" => {
                settings::set_text_size(self, crate::base::display::larger_size(self.text_size()));
                true
            }
            "ctrl+-" => {
                settings::set_text_size(self, crate::base::display::smaller_size(self.text_size()));
                true
            }
            "ctrl+0" => {
                settings::set_text_size(self, crate::base::display::DEFAULT_SIZE);
                true
            }
            "ctrl+," if self.screen == Screen::Main => {
                self.navigate(View::Settings);
                true
            }
            "ctrl+z" if self.screen == Screen::Main => {
                self.undo();
                true
            }
            "ctrl+shift+z" | "ctrl+y" if self.screen == Screen::Main => {
                self.redo();
                true
            }
            "ctrl+b" if self.screen == Screen::Main => {
                rail::toggle(self);
                true
            }
            "escape" if self.pane.open && !self.pane.pinned => {
                self.close_pane();
                true
            }
            _ => false,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn date_order(&self) -> DateOrder {
        self.config.date_order
    }
}
