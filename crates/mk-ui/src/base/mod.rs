//! What the controller, the views and the app shell share: the views, screens,
//! the shell's requests ([`AppEvent`]), the per-user config and the small types
//! they carry. No toolkit here.

pub(crate) mod about;
pub(crate) mod accounts;
pub(crate) mod display;
pub(crate) mod period;
pub(crate) mod recent;
pub(crate) mod reports;
pub(crate) mod security;

use mk_core::Date;

use crate::theme::{self, Density, Theme};

pub use recent::{RecentBook, TileCache, TileIcon};

/// Commands that change the book.
pub(crate) fn is_mutating_command(command: &str) -> bool {
    is_mutating(command)
}

/// Today's date from the clock, YYYY-MM-DD.
pub(crate) fn today_iso() -> String {
    today()
}

/// What the main area shows.
#[derive(Debug, Clone, PartialEq)]
pub enum View {
    /// The startup dashboard (revision 31; the Home navigation item).
    Home,
    /// A dashboard by id (dashboards.md).
    Dashboard {
        id: String,
    },
    Register {
        account: String,
    },
    Import {
        batch: String,
    },
    Reconcile {
        account: String,
    },
    Accounts,
    Settings,
    History,
    Scheduled,
    Budget,
    Reports,
    /// Securities and prices (investments module).
    Securities,
    /// Invoices, payments, estimates, customers, items and sales tax (invoicing module).
    Sales,
    /// Bills, bill payments, purchase orders, vendors and 1099 (bills module).
    Purchases,
    /// Expenses and the receipt inbox (expenses module, revision 28).
    Expenses,
    /// Trips and vehicles (mileage module, revision 36).
    Mileage,
    /// Subscriptions and recurring bills (subscriptions module, subscriptions.md).
    Subscriptions,
    /// Projects (projects module, projects.md).
    Projects,
}

/// Start screen or the main window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Start,
    Main,
}

/// Requests for the app shell (things the UI crate cannot do itself).
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    /// Show a "save book" dialog; the app calls [`crate::ctl::App::create_book`] with the path.
    ChooseNewBookPath { name: String, currency: String },
    /// Show an "open book" dialog; the app calls [`crate::ctl::App::open_book`].
    ChooseBookToOpen,
    /// Show an "open import file" dialog; the app calls [`crate::ctl::App::import_file`].
    ChooseImportFile,
    /// Show a folder picker for the watched folder; the app calls
    /// [`crate::ctl::App::set_watch_folder`].
    ChooseWatchFolder,
    /// Start or stop watching the folder in the background.
    Watch(bool),
    /// The recent-books list or display settings changed; persist the config.
    ConfigChanged,
    /// Reset MoneyKeep settings (app-desktop.md revision 41) was confirmed: save the
    /// current settings file as `copy_name` next to it, then call
    /// [`crate::ctl::App::finish_settings_reset`] with where it went (or why not).
    ResetSettings { copy_name: String },
    /// Show a save dialog suggesting `file_name` and write `content` to the chosen file.
    SaveFile { file_name: String, content: String },
    /// Like `SaveFile`, for the recovery key: the dialog suggests a folder other than
    /// the book's.
    SaveRecoveryKey { file_name: String, content: String },
    /// Show a save dialog for an unencrypted copy; the app calls
    /// [`crate::ctl::App::save_unencrypted_copy`] with the path.
    ChooseUnencryptedCopyPath { file_name: String },
    /// Show an "open CSV" dialog for prices; the app calls [`crate::ctl::App::import_prices`].
    ChoosePriceFile,
    /// Show an "open definition file" dialog (`.mkdef`); the app calls
    /// [`crate::ctl::App::open_definition`].
    ChooseDefinitionFile,
    /// Show a save dialog suggesting `file_name` and write the bytes (invoice PDFs).
    SaveBytes { file_name: String, data: Vec<u8> },
    /// Show a folder picker and write each (file name, bytes) into the chosen folder
    /// (customer statements, one PDF each).
    SaveFilesToFolder { files: Vec<(String, Vec<u8>)> },
    /// Show an "open image" dialog (PNG or JPEG) for a document template's logo; the
    /// app calls [`crate::ctl::App::set_template_logo`] with the template and the file.
    ChooseLogoFile { template: String },
    /// Show an "open image" dialog (PNG or JPEG) for the open book's picture
    /// (Settings → Book, revision 26); the app calls [`crate::ctl::App::set_book_icon`],
    /// which opens the Picture editor (revision 40).
    ChooseBookIconFile,
    /// Show the folder holding a book's file in the system's file manager (the
    /// start screen's SHOW IN FOLDER, revision 40).
    ShowInFolder { path: String },
    /// Show an "open files" dialog for receipts (PNG, JPEG or PDF; several at
    /// once, revision 28); the app calls [`crate::ctl::App::add_receipt_files`] with `target`.
    ChooseReceiptFiles { target: ReceiptTarget },
    /// Open a PDF receipt in the system's PDF viewer (after the user confirmed).
    OpenReceiptPdf { file_name: String, data: Vec<u8> },
    /// Show a save dialog for an expense report PDF and write it; once it is
    /// written the app calls [`crate::ctl::App::expense_report_saved`] with `expenses`, which
    /// marks them reported (expenses.md revision 2). Cancelling marks nothing.
    SaveExpenseReport {
        file_name: String,
        data: Vec<u8>,
        expenses: Vec<String>,
    },
    /// The open book was closed: the app deletes this session's temporary copies of
    /// PDF receipts (expenses.md revision 2).
    BookClosed,
    /// Open a Cloudflare page in the browser (phone capture setup, revision 29); the
    /// app opens only `https://dash.cloudflare.com/` and
    /// `https://developers.cloudflare.com/` addresses.
    OpenLink { url: String },
    /// A book was opened (or unlocked) at `path`: the app starts its control channel
    /// when the book allows AI assistants (mcp-server.md revision 3).
    BookOpened { path: String },
    /// The book was locked after being idle: the control channel closes its
    /// connections with reason `locked` (a `BookClosed` follows).
    BookLocked,
    /// The AI access settings changed (Settings → Security).
    AiAccessChanged,
    /// The owner answered approval `id` (Allow or Deny).
    AiAnswer { id: u64, allow: bool },
    /// The owner pressed Disconnect for an assistant.
    AiDisconnect { client: String },
    /// Show a folder picker for the backup folder (`second`: the second place); the
    /// app calls [`crate::ctl::App::set_backup_folder`] (backups.md).
    ChooseBackupFolder { second: bool },
    /// Open a folder in the system's file manager (Settings → Backups, Open folder).
    OpenFolder { path: String },
    /// Open `THIRD-PARTY-LICENSES.txt`, installed beside the programs, in the
    /// system's text viewer (About MoneyKeep → Third-party licences,
    /// security-review-1 B4).
    OpenThirdPartyLicences,
    /// Show an "open file" dialog for a backup file; the app calls
    /// [`crate::ctl::App::restore_from_file`].
    ChooseBackupFile,
    /// Show a save dialog for a restored copy in `dir` (when known) suggesting
    /// `file_name`; the app calls [`crate::ctl::App::restore_to`] with the path.
    ChooseRestoreTarget {
        dir: Option<String>,
        file_name: String,
    },
}

/// Settings → Backups (backups.md): the app settings `backup.*`, the same names
/// as the command line reads from this config.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BackupPrefs {
    /// The backup folder (`None`: `<user folder>\MoneyKeep Backups`, backups.md
    /// revision 3).
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub second_place: Option<String>,
    #[serde(default = "default_true")]
    pub on_open: bool,
    #[serde(default = "default_true")]
    pub on_close: bool,
    #[serde(default = "default_true")]
    pub daily: bool,
    #[serde(default = "ten")]
    pub keep_recent: u32,
    #[serde(default = "seven")]
    pub keep_days: u32,
    #[serde(default = "twelve")]
    pub keep_months: u32,
    #[serde(default = "ten")]
    pub keep_per_reason: u32,
    #[serde(default = "ten")]
    pub suggest_second_every: u32,
    #[serde(default = "fourteen")]
    pub warn_days: u32,
}

fn ten() -> u32 {
    10
}
fn seven() -> u32 {
    7
}
fn twelve() -> u32 {
    12
}
fn fourteen() -> u32 {
    14
}

impl Default for BackupPrefs {
    fn default() -> Self {
        Self {
            folder: None,
            second_place: None,
            on_open: true,
            on_close: true,
            daily: true,
            keep_recent: 10,
            keep_days: 7,
            keep_months: 12,
            keep_per_reason: 10,
            suggest_second_every: 10,
            warn_days: 14,
        }
    }
}

/// What the app remembers about backups between runs (not settings).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct BackupMemo {
    /// Backups the app made (counts towards suggesting a second place).
    #[serde(default)]
    pub made: u64,
    /// The count at which Not now was pressed (the suggestion returns 10 later).
    #[serde(default)]
    pub suggest_from: u64,
    /// The last successful copy to the second place: Unix seconds and local time
    /// (`YYYY-MM-DDTHH:MM`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_ok: Option<(i64, String)>,
    /// Restored copies whose Home note was not dismissed yet: (book path, the
    /// backup's local time `YYYY-MM-DDTHH:MM`, or empty when unknown).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restored: Vec<(String, String)>,
}

/// Per-user settings the app persists (never inside the book, never financial data).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UserConfig {
    /// Recent books, most recent first, each with its tile cache (revision 26;
    /// configs holding plain path strings still load).
    #[serde(default)]
    pub recent: Vec<RecentBook>,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub density: Density,
    /// The main window's last size, position and maximised state (the app shell
    /// writes it; per user, never in a book).
    #[serde(default)]
    pub window: Option<crate::WindowState>,
    /// Widths of the panels the user resized.
    #[serde(default)]
    pub panels: crate::PanelWidths,
    /// Close a password-protected book after this many minutes without input
    /// (0 = never).
    #[serde(default)]
    pub lock_after_minutes: u32,
    /// Text size in percent (90–200; values outside are clamped).
    #[serde(default = "default_text_size")]
    pub text_size: u32,
    /// Interface font family (None = built-in).
    #[serde(default)]
    pub ui_font: Option<String>,
    /// Number (monospace) font family (None = built-in).
    #[serde(default)]
    pub number_font: Option<String>,
    /// The Settings section shown last.
    #[serde(default)]
    pub settings_section: String,
    /// Fast entry in the transaction pop-up (revision 24): Enter saves and a new
    /// blank transaction opens. (The old `edit_popup` field of revision 9 is
    /// ignored when read.)
    #[serde(default)]
    pub fast_entry: bool,
    /// How dates are shown and typed (revision 20): one of
    /// `mk_core::datefmt::PATTERNS`; anything else reads as the default.
    #[serde(default = "default_date_format")]
    pub date_format: String,
    /// The accounts pane: pinned, last group, open when pinned (revision 23). Its
    /// width is `panels.sidebar`.
    #[serde(default)]
    pub accounts_pane: AccountsPaneConfig,
    /// The calculator: position, pinned, open when pinned (revision 25).
    #[serde(default)]
    pub calculator: CalculatorConfig,
    /// Sort transactions by date (revision 27): every account register and the
    /// investment register.
    #[serde(default)]
    pub date_order: DateOrder,
    /// Scroll bar width in points (revision 27; 6–30, values outside are clamped).
    #[serde(default = "default_scroll_width")]
    pub scroll_bar_width: u32,
    /// Status messages (revision 37): `ui.message_autoclose`, `ui.message_seconds`.
    #[serde(default)]
    pub ui: MessagePrefs,
    /// The start screen's card order (revision 40): `last_opened` (default) or
    /// `name`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub start_sort: String,
    /// The Picture editor's tips panel is open (revision 40: open the first time,
    /// then as the user left it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture_tips_open: Option<bool>,
    /// Backups (backups.md): the settings.
    #[serde(default)]
    pub backup: BackupPrefs,
    /// Backups: what the app remembers between runs.
    #[serde(default)]
    pub backup_memo: BackupMemo,
}

/// The seconds Settings → Operation offers for closing status messages.
pub const MESSAGE_SECONDS: [u32; 6] = [2, 3, 5, 8, 10, 15];

/// Status messages close by themselves (Settings → Operation, revision 37).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MessagePrefs {
    /// Close status messages automatically (errors never close by themselves).
    #[serde(default = "default_true")]
    pub message_autoclose: bool,
    /// After this many seconds (one of [`MESSAGE_SECONDS`]; others read as 3).
    #[serde(default = "default_message_seconds")]
    pub message_seconds: u32,
}

impl Default for MessagePrefs {
    fn default() -> Self {
        Self {
            message_autoclose: true,
            message_seconds: default_message_seconds(),
        }
    }
}

impl MessagePrefs {
    /// The seconds in use: a value not offered reads as the default.
    pub fn seconds(&self) -> u32 {
        if MESSAGE_SECONDS.contains(&self.message_seconds) {
            self.message_seconds
        } else {
            default_message_seconds()
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_message_seconds() -> u32 {
    3
}

/// Sort transactions by date (Settings → Operation, revision 27).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateOrder {
    /// Newest first.
    #[default]
    Descending,
    /// Oldest first.
    Ascending,
}

fn default_scroll_width() -> u32 {
    theme::SCROLL_WIDTH_DEFAULT
}

fn default_date_format() -> String {
    mk_core::datefmt::DEFAULT_PATTERN.to_owned()
}

fn default_text_size() -> u32 {
    display::DEFAULT_SIZE
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            recent: Vec::new(),
            theme: Theme::default(),
            density: Density::default(),
            window: None,
            panels: crate::PanelWidths::default(),
            lock_after_minutes: 0,
            text_size: display::DEFAULT_SIZE,
            ui_font: None,
            number_font: None,
            settings_section: String::new(),
            fast_entry: false,
            date_format: default_date_format(),
            accounts_pane: AccountsPaneConfig::default(),
            calculator: CalculatorConfig::default(),
            date_order: DateOrder::default(),
            scroll_bar_width: theme::SCROLL_WIDTH_DEFAULT,
            ui: MessagePrefs::default(),
            start_sort: String::new(),
            picture_tips_open: None,
            backup: BackupPrefs::default(),
            backup_memo: BackupMemo::default(),
        }
    }
}

/// Commands that change the book.
fn is_mutating(command: &str) -> bool {
    !matches!(
        command,
        "book.info"
            | "book.preview_picture"
            | "book.picture_source"
            | "module.list"
            | "setting.get"
            | "account.get"
            | "account.list"
            | "account.balance"
            | "account.register"
            | "account.suggest"
            | "payee.suggest"
            | "txn.get"
            | "txn.list"
            | "report.trial_balance"
            | "history.list"
            | "audit.list"
            | "import.list"
            | "import.get"
            | "rule.list"
            | "rule.test"
            | "csv.profile_list"
            | "csv.profile_export"
            | "reconcile.get"
            | "reconcile.history"
            | "schedule.get"
            | "schedule.list"
            | "schedule.upcoming"
            | "schedule.forecast"
            | "schedule.preview"
            | "budget.list"
            | "budget.get"
            | "budget.suggest"
            | "budget.report"
            | "security.get"
            | "security.list"
            | "price.list"
            | "price.latest"
            | "inv.get"
            | "inv.preview"
            | "inv.preview_sell"
            | "inv.holdings"
            | "inv.lots"
            | "inv.activity"
            | "definition.inspect"
            | "definition.list"
            | "definition.export"
            | "definition.validate"
            | "definition.candidates"
            | "inv_profile.list"
            | "inv_profile.get"
            | "inv_profile.suggest"
            | "inv_profile.preview"
            | "module.settings"
            | "party.get"
            | "party.list"
            | "item.get"
            | "item.list"
            | "tax_rate.list"
            | "invoice.get"
            | "invoice.list"
            | "invoice.preview"
            | "invoice.render_pdf"
            | "estimate.get"
            | "estimate.list"
            | "estimate.render_pdf"
            | "credit_memo.get"
            | "credit_memo.list"
            | "payment.get"
            | "payment.list"
            | "payment.open_invoices"
            | "deposit.list"
            | "deposit.preview"
            | "customer.summary"
            | "bill.get"
            | "bill.list"
            | "bill.preview"
            | "bill.open"
            | "bill.attention"
            | "bill_payment.get"
            | "bill_payment.list"
            | "bill_payment.next_check"
            | "vendor_credit.get"
            | "vendor_credit.list"
            | "vendor.summary"
            | "txn.vendor_links"
            | "po.get"
            | "po.list"
            | "po.render_pdf"
            | "credit_memo.render_pdf"
            | "class.list"
            | "location.list"
            | "fund.list"
            | "fund.attention"
            | "dimension.names"
            | "template.list"
            | "template.get"
            | "statement.render"
            | "statement.batch"
            | "late_fee.preview"
            | "export.iif"
            | "connect.providers"
            | "connect.list"
            | "connect.get"
            | "connect.attention"
            | "connect.unused"
            | "processor.settings"
            | "expense.get"
            | "expense.list"
            | "expense.match_candidates"
            | "expense.report"
            | "receipt.get"
            | "receipt.thumbnail"
            | "receipt.inbox"
            | "receipt.list"
            | "receipt.find_transactions"
            | "vehicle.list"
            | "trip.get"
            | "trip.list"
            | "saved_trip.list"
            | "mileage_rate.list"
            | "mileage.report"
            | "mileage.summary"
            | "subscription.list"
            | "subscription.get"
            | "subscription.suggestions"
            | "subscription.dismissed"
            | "subscription.alerts"
            | "subscription.review"
            | "subscription.summary"
            | "project.list"
            | "project.get"
            | "project.unbilled"
            | "project.alerts"
            | "report.project"
            | "report.projects_summary"
            | "dashboard.list"
            | "dashboard.get"
            | "dashboard.block_data"
            | "chart.presets"
            | "chart.preview_add"
    ) && !command.starts_with("report.")
}

/// Plain words for a command in undo tooltips and history.
pub(crate) fn describe_command(command: &str) -> String {
    match command {
        "txn.create" => "new transaction",
        "txn.update" => "transaction edit",
        "txn.delete" => "transaction delete",
        "txn.void" => "void",
        "txn.unvoid" => "unvoid",
        "txn.reverse" => "reversal",
        "txn.set_reconcile" => "status change",
        "account.create" => "new account",
        "account.update" => "account edit",
        "account.move" => "account move",
        "account.archive" => "archive",
        "account.unarchive" => "unarchive",
        "account.delete" => "account delete",
        "chart.add" => "accounts from a chart",
        "import.stage" => "import staging",
        "import.commit" => "import",
        "import.discard" => "import discard",
        "import.map_account" => "import account choice",
        "import.set_row" => "import row change",
        "definition.install" => "definition install",
        "definition.uninstall" => "definition uninstall",
        "reconcile.finish" => "reconciliation",
        "module.enable" => "feature on",
        "module.disable" => "feature off",
        "setting.set" => "setting change",
        "rule.create" => "new rule",
        "book.set_closing_date" => "closing date",
        "book.update_profile" => "profile change",
        "book.set_icon_image" => "icon picture",
        "book.clear_icon_image" => "icon picture removal",
        "history.undo" => "undo",
        "history.redo" => "redo",
        "book.ai_access_changed" => "AI assistant access change",
        "ai.session_allowed" => "AI assistant allowed",
        "schedule.create" => "new schedule",
        "schedule.update" => "schedule edit",
        "schedule.delete" => "schedule delete",
        "schedule.pause" => "schedule pause",
        "schedule.resume" => "schedule resume",
        "schedule.enter" => "scheduled transaction",
        "schedule.skip" => "skip",
        "schedule.from_txn" => "new schedule",
        "schedule.run_due" => "scheduled transactions",
        "budget.create" => "new budget",
        "budget.set_line" => "budget change",
        "budget.set_month" => "budget change",
        "budget.remove_line" => "budget change",
        "budget.apply_suggestion" => "budget fill",
        "module.set_setting" => "setting change",
        "party.create" => "new customer",
        "party.update" => "customer edit",
        "item.create" => "new item",
        "item.update" => "item edit",
        "tax_rate.create" => "new tax rate",
        "tax_rate.update" => "tax rate edit",
        "invoice.create" => "new invoice",
        "invoice.update" => "invoice edit",
        "invoice.finalize" => "invoice finalise",
        "invoice.void" => "invoice void",
        "invoice.delete" => "invoice delete",
        "invoice.duplicate" => "invoice copy",
        "estimate.create" => "new estimate",
        "estimate.update" => "estimate edit",
        "estimate.convert" => "estimate conversion",
        "payment.receive" => "payment",
        "deposit.create" => "deposit",
        "customer.refund" => "refund",
        "sales_tax.pay" => "sales tax payment",
        "bill.create" => "new bill",
        "bill.update" => "bill edit",
        "bill.void" => "bill void",
        "bill.delete" => "bill delete",
        "bill.pay" => "bill payment",
        "bill.pay_many" => "bill payments",
        "bill_payment.void" => "payment void",
        "vendor_credit.create" => "vendor credit",
        "po.create" => "new purchase order",
        "po.update" => "purchase order edit",
        "po.receive" => "receipt",
        "txn.set_vendor" => "vendor link",
        "class.create" => "new class",
        "class.update" => "class edit",
        "class.set_active" => "class change",
        "class.delete" => "class delete",
        "location.create" => "new location",
        "location.update" => "location edit",
        "location.set_active" => "location change",
        "location.delete" => "location delete",
        "fund.create" => "new fund",
        "fund.update" => "fund edit",
        "fund.set_active" => "fund change",
        "fund.delete" => "fund delete",
        "fund.release" => "release from restriction",
        "fund.transfer" => "fund transfer",
        "template.create" => "new template",
        "template.update" => "template edit",
        "template.delete" => "template delete",
        "template.set_logo" => "logo",
        "template.clear_logo" => "logo removal",
        "late_fee.assess" => "late fees",
        "import.map_name" => "import name choice",
        "import.create_lists" => "import lists",
        "connect.add" => "new connection",
        "connect.link_account" => "account link",
        "connect.sync" => "bank feed sync",
        "connect.disconnect" => "disconnect",
        "connect.set_label" => "connection rename",
        "connect.disable" => "connection off",
        "connect.enable" => "connection on",
        "connect.resume" => "connection finished",
        "connect.delete_credential" => "unused credential delete",
        "connect.dismiss" => "notice dismissal",
        "processor.set" => "processor settings",
        "expense.create" => "new expense",
        "expense.update" => "expense edit",
        "expense.delete" => "expense delete",
        "expense.post" => "expense posting",
        "expense.record_reimbursement" => "reimbursement",
        "expense.mark_reported" => "expenses marked reported",
        "receipt.add" => "new receipt",
        "receipt.link" => "receipt attached",
        "receipt.unlink" => "receipt removed",
        "receipt.delete" => "receipt delete",
        "vehicle.create" => "new vehicle",
        "vehicle.update" => "vehicle edit",
        "vehicle.delete" => "vehicle delete",
        "vehicle.set_odometer" => "odometer readings",
        "trip.create" => "new trip",
        "trip.update" => "trip edit",
        "trip.delete" => "trip delete",
        "trip.post" => "trip posting",
        "saved_trip.create" => "new saved trip",
        "saved_trip.update" => "saved trip edit",
        "saved_trip.delete" => "saved trip delete",
        "mileage_rate.set" => "mileage rate",
        "mileage_rate.delete" => "mileage rate delete",
        "mileage.reapply_rates" => "rates applied again",
        "subscription.answer" => "suggestion answer",
        "subscription.restore_suggestion" => "suggestion back",
        "subscription.create" => "new subscription",
        "subscription.update" => "subscription edit",
        "subscription.delete" => "subscription delete",
        "subscription.set_status" => "subscription status",
        "subscription.still_active" => "still active",
        "subscription.add_charge" => "charge added",
        "subscription.remove_charge" => "charge removed",
        "subscription.set_reason" => "price change reason",
        "subscription.dismiss_alert" => "alert dismissed",
        "subscription.review_answer" => "review answer",
        "subscription.make_bill" => "repeating bill",
        "project.create" => "new project",
        "project.update" => "project edit",
        "project.set_status" => "project status",
        "project.delete" => "project delete",
        "project.set_fund_on_tagged" => "fund on tagged lines",
        "txn.set_project" => "project on transactions",
        "project.billable_lines" => "billable income account",
        "project.dismiss_alert" => "alert dismissed",
        "dashboard.create" => "new dashboard",
        "dashboard.update" => "dashboard rename",
        "dashboard.delete" => "dashboard delete",
        "dashboard.reorder" => "dashboard order",
        "dashboard.set_startup" => "startup dashboard",
        "dashboard.reset_home" => "Home reset",
        "dashboard.add_block" => "new block",
        "dashboard.update_block" => "block settings",
        "dashboard.move_blocks" => "layout change",
        "dashboard.remove_block" => "block removal",
        other => return other.to_owned(),
    }
    .to_owned()
}

/// Where files chosen in a dialog or dropped go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptTarget {
    /// The Expenses screen's inbox.
    Inbox,
    /// The open expense pop-up.
    ExpenseForm,
    /// The open transaction pop-up of the register.
    TransactionPopup,
}

/// A file the app read for a receipt; `bytes` is the refusal text when it could not
/// (or should not) be read.
#[derive(Debug, Clone, PartialEq)]
pub struct PickedFile {
    pub name: String,
    pub bytes: Result<Vec<u8>, String>,
}

/// Largest receipt file the app reads (larger ones are refused before reading).
pub const MAX_RECEIPT_BYTES: u64 = 10 * 1024 * 1024;
/// The refusal for a larger file (the command's own words).
pub const TOO_BIG: &str = "Receipts can be at most 10 MB.";

/// Per-user settings of the pane (app config).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccountsPaneConfig {
    /// Docked beside the rail rather than floating over the main area.
    #[serde(default)]
    pub pinned: bool,
    /// The group shown last (its name; empty for All accounts).
    #[serde(default)]
    pub group: String,
    /// Whether the pinned pane was open (an unpinned pane always starts closed).
    #[serde(default)]
    pub open: bool,
}

/// Per-user settings of the calculator (app config).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CalculatorConfig {
    /// Top-left corner in logical points; `None` for the default (bottom right).
    #[serde(default)]
    pub pos: Option<[i32; 2]>,
    /// Pinned in place: cannot be dragged and stays open after Esc.
    #[serde(default)]
    pub pinned: bool,
    /// Whether the pinned calculator was open (it reopens at start-up).
    #[serde(default)]
    pub open: bool,
}

/// A file the app should stage (kept for API symmetry with dialogs).
#[derive(Debug, Clone, PartialEq)]
pub struct FileRequest {
    pub name: String,
}

pub(crate) fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = i32::try_from(secs / 86_400).unwrap_or(0);
    "1970-01-01"
        .parse::<Date>()
        .ok()
        .and_then(|d| d.add_days(days))
        .map(|d| d.to_string())
        .unwrap_or_default()
}
