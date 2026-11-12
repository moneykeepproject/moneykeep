//! Test support: an in-process backend over `mk_commands::Session`, and helpers to
//! drive the UI through the accessibility tree.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;

use mk_commands::{KdfParams, Secret, Session};
use mk_ui::ctl::App;
use mk_ui::{AppEvent, Backend, SecurityOp, UiError, UiSecret, UserConfig};
#[allow(unused_imports)]
pub use mk_views::testing::{
    Harness, Key, Modifiers, Node, Pos2, Rect, Role, Toggled, Vec2, pos2, vec2,
};
use serde_json::{Value, json};

/// Runs commands in-process (the app runs them on a worker thread instead).
#[derive(Debug)]
pub struct LocalBackend {
    pub session: Option<Session>,
    pub name: Option<String>,
}

thread_local! {
    /// Background network jobs of the test backend (one runner per test thread).
    static RUNNER: std::cell::RefCell<mk_commands::connect::NetRunner> =
        std::cell::RefCell::new(mk_commands::connect::NetRunner::default());
}

thread_local! {
    /// The test backend's tile picture cache (the app keeps files instead).
    pub static TILE_IMAGES: std::cell::RefCell<std::collections::BTreeMap<String, Vec<u8>>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

thread_local! {
    /// The test backend's backup settings (none until a test sets a folder, so
    /// other tests never write backups) and the finished backups not polled yet.
    pub static BACKUP_ENV: std::cell::RefCell<Option<mk_commands::backups::BackupEnv>> =
        const { std::cell::RefCell::new(None) };
    static BACKUP_DONE: std::cell::RefCell<Vec<Result<Value, UiError>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Test hooks of the backup env (backups.md BK17, BK18, BK21), kept when the
/// settings change.
#[derive(Debug, Clone, Default)]
pub struct BackupHooks {
    pub folder_error: Option<std::io::ErrorKind>,
    pub protected: Option<Vec<PathBuf>>,
    pub fail_second: bool,
}

thread_local! {
    pub static BACKUP_HOOKS: std::cell::RefCell<BackupHooks> =
        std::cell::RefCell::new(BackupHooks::default());
}

fn hooked(mut env: mk_commands::backups::BackupEnv) -> mk_commands::backups::BackupEnv {
    BACKUP_HOOKS.with(|h| {
        let h = h.borrow();
        env.simulate_folder_error = h.folder_error;
        env.protected_override.clone_from(&h.protected);
        env.fail_second = h.fail_second;
    });
    env
}

fn backup_env() -> Result<mk_commands::backups::BackupEnv, UiError> {
    BACKUP_ENV
        .with(|e| e.borrow().clone())
        .map(hooked)
        .ok_or_else(|| UiError::new("unsupported", "no backup folder in this test"))
}

fn err(e: &mk_commands::CommandError) -> UiError {
    UiError::new(e.code(), e.to_string())
}

impl Backend for LocalBackend {
    fn invoke(&mut self, command: &str, args: Value) -> Result<Value, UiError> {
        match self.session.as_mut() {
            Some(s) => {
                // The backup commands run with the test's backup folder, as the app's
                // session has its backup settings.
                if let Ok(env) = backup_env() {
                    s.set_backup_env(env);
                }
                s.invoke(command, args).map_err(|e| err(&e))
            }
            None => Err(UiError::new("no_book", "No book is open.")),
        }
    }
    fn book_name(&self) -> Option<String> {
        self.name.clone()
    }
    fn net_start(&mut self, command: &str, args: Value) -> Result<u64, UiError> {
        let s = self
            .session
            .as_mut()
            .ok_or_else(|| UiError::new("no_book", "No book is open."))?;
        Ok(RUNNER.with(|r| r.borrow_mut().start(s, command, args)))
    }
    fn net_poll(&mut self) -> Vec<(u64, Result<Value, UiError>)> {
        let Some(s) = self.session.as_mut() else {
            return Vec::new();
        };
        RUNNER.with(|r| {
            r.borrow_mut()
                .poll(s)
                .into_iter()
                .map(|(id, out)| (id, out.map_err(|e| err(&e))))
                .collect()
        })
    }
    fn net_cancel(&mut self, id: u64) {
        RUNNER.with(|r| r.borrow_mut().cancel(id));
    }
    fn save_tile_image(&mut self, book_path: &str, png: Option<&[u8]>) {
        TILE_IMAGES.with(|m| {
            let mut m = m.borrow_mut();
            match png {
                Some(b) => m.insert(book_path.to_owned(), b.to_vec()),
                None => m.remove(book_path),
            };
        });
    }
    fn load_tile_image(&self, book_path: &str) -> Option<Vec<u8>> {
        TILE_IMAGES.with(|m| m.borrow().get(book_path).cloned())
    }
    fn chart_presets(&mut self) -> Result<Value, UiError> {
        mk_commands::chart::presets_json().map_err(|e| UiError::new(e.code(), e.to_string()))
    }
    fn create_book(
        &mut self,
        path: &str,
        name: &str,
        currency: &str,
        preset: Option<&str>,
    ) -> Result<(), UiError> {
        self.session = Some(
            Session::create_with_preset(std::path::Path::new(path), name, currency, "app", preset)
                .map_err(|e| err(&e))?,
        );
        self.name = Some(name.to_owned());
        Ok(())
    }
    fn open_book(&mut self, path: &str) -> Result<(), UiError> {
        let mut s = Session::open(std::path::Path::new(path), "app").map_err(|e| err(&e))?;
        let info = s.invoke("book.info", json!({})).map_err(|e| err(&e))?;
        self.name = info["name"].as_str().map(str::to_owned);
        self.session = Some(s);
        Ok(())
    }
    fn close_book(&mut self) {
        self.session = None;
        self.name = None;
    }
    fn create_protected_book(
        &mut self,
        path: &str,
        name: &str,
        currency: &str,
        password: &str,
        recovery: bool,
        preset: Option<&str>,
    ) -> Result<Option<String>, UiError> {
        let (s, key) = Session::create_encrypted_with_preset(
            std::path::Path::new(path),
            name,
            currency,
            "app",
            password,
            recovery,
            KdfParams::TEST,
            preset,
        )
        .map_err(|e| err(&e))?;
        self.session = Some(s);
        self.name = Some(name.to_owned());
        Ok(key)
    }
    fn open_book_with(&mut self, path: &str, secret: &UiSecret) -> Result<(), UiError> {
        self.session = None;
        let mut s = Session::open_with(std::path::Path::new(path), "app", Some(&to_secret(secret)))
            .map_err(|e| err(&e))?;
        s.set_kdf(KdfParams::TEST);
        let info = s.invoke("book.info", json!({})).map_err(|e| err(&e))?;
        self.name = info["name"].as_str().map(str::to_owned);
        self.session = Some(s);
        Ok(())
    }
    fn is_protected_file(&self, path: &str) -> Option<bool> {
        mk_commands::book_kind(std::path::Path::new(path))
            .ok()
            .map(|k| k == mk_commands::BookKind::Encrypted)
    }
    fn protection(&self) -> (bool, bool) {
        self.session
            .as_ref()
            .map_or((false, false), |s| (s.is_encrypted(), s.has_recovery_key()))
    }
    fn ai_access(&self) -> Option<Value> {
        Some(self.session.as_ref()?.ai_access().ok()?.to_json())
    }
    fn set_backup_settings(&mut self, settings: Value) {
        let s = mk_commands::backups::BackupSettings::from_json(&settings);
        let env = s
            .folder
            .is_some()
            .then(|| mk_commands::backups::BackupEnv::new(s, 0));
        BACKUP_ENV.with(|e| *e.borrow_mut() = env);
    }
    fn test_backup_folder(&mut self, folder: &str) -> Result<(), UiError> {
        let env = hooked(BACKUP_ENV.with(|e| e.borrow().clone()).unwrap_or_default());
        mk_commands::backups::test_folder(std::path::Path::new(folder), &env).map_err(|e| err(&e))
    }
    fn backup_start(&mut self, reason: &str) -> Result<(), UiError> {
        let env = backup_env()?;
        let s = self
            .session
            .as_mut()
            .ok_or_else(|| UiError::new("no_book", "No book is open."))?;
        s.set_backup_env(env);
        // Synchronous here; reported at the next poll as the app's worker would.
        let r = s
            .invoke("book.backup", json!({"reason": reason}))
            .map_err(|e| err(&e));
        BACKUP_DONE.with(|d| d.borrow_mut().push(r));
        Ok(())
    }
    fn backup_poll(&mut self) -> Vec<Result<Value, UiError>> {
        BACKUP_DONE.with(|d| std::mem::take(&mut *d.borrow_mut()))
    }
    fn backup_changed(&mut self) -> bool {
        self.session
            .as_mut()
            .is_some_and(Session::changed_since_backup)
    }
    fn backups_list(&mut self, book: &str) -> Result<Value, UiError> {
        let env = backup_env()?;
        Ok(Value::Array(
            mk_commands::backups::list(std::path::Path::new(book), &env)
                .iter()
                .map(|e| e.to_json(&env))
                .collect(),
        ))
    }
    fn restore_copy(
        &mut self,
        backup: &str,
        target: &str,
        secret: Option<&UiSecret>,
    ) -> Result<Value, UiError> {
        let secret = secret.map(to_secret);
        mk_commands::backups::restore_copy(
            std::path::Path::new(backup),
            std::path::Path::new(target),
            secret.as_ref(),
            None,
        )
        .map(|r| json!({"path": r.path.display().to_string(), "encrypted": r.encrypted}))
        .map_err(|e| err(&e))
    }
    fn backup_book_file(
        &mut self,
        book: &str,
        secret: Option<&UiSecret>,
    ) -> Result<Value, UiError> {
        let env = backup_env()?;
        let secret = secret.map(to_secret);
        mk_commands::backups::back_up_file(
            std::path::Path::new(book),
            secret.as_ref(),
            mk_commands::backups::Reason::Manual,
            &env,
        )
        .map(|m| m.to_json())
        .map_err(|e| err(&e))
    }
    fn set_ai_access(&mut self, settings: Value) -> Result<Value, UiError> {
        let s = self
            .session
            .as_mut()
            .ok_or_else(|| UiError::new("no_book", "No book is open."))?;
        s.set_ai_access(&mk_commands::ai_access::AiAccess::from_json(&settings))
            .map(|a| a.to_json())
            .map_err(|e| err(&e))
    }
    fn security(&mut self, op: SecurityOp) -> Result<Option<String>, UiError> {
        let s = self
            .session
            .as_mut()
            .ok_or_else(|| UiError::new("no_book", "No book is open."))?;
        s.set_kdf(KdfParams::TEST);
        let r = match op {
            SecurityOp::Encrypt { password, recovery } => s.encrypt(&password, recovery),
            SecurityOp::ChangePassword { current, new } => {
                s.change_password(&to_secret(&current), &new).map(|()| None)
            }
            SecurityOp::NewRecoveryKey { current } => {
                s.new_recovery_key(&to_secret(&current)).map(Some)
            }
            SecurityOp::RemoveRecoveryKey { current } => {
                s.remove_recovery_key(&to_secret(&current)).map(|()| None)
            }
            SecurityOp::Verify { current } => s.verify_secret(&to_secret(&current)).map(|()| None),
            SecurityOp::SaveUnencryptedCopy { path } => s
                .save_unencrypted_copy(std::path::Path::new(&path))
                .map(|p| Some(p.display().to_string())),
            SecurityOp::RemoveEncryption { current } => {
                s.remove_encryption(&to_secret(&current)).map(|()| None)
            }
        };
        r.map_err(|e| err(&e))
    }
}

fn to_secret(s: &UiSecret) -> Secret {
    match s {
        UiSecret::Password(p) => Secret::Password(p.clone()),
        UiSecret::RecoveryKey(k) => Secret::RecoveryKey(k.clone()),
    }
}

/// A book in a temporary directory, opened in the app.
#[derive(Debug)]
pub struct TestBook {
    pub dir: tempfile::TempDir,
    pub path: PathBuf,
}

/// The config the tests of revisions before 20 run with: ISO dates, as their
/// expectations are written (revision 20 changed the default to MM-DD-YYYY; its own
/// tests use [`new_book_app_with`] and the default).
pub fn iso_config() -> UserConfig {
    UserConfig {
        date_format: "YYYY-MM-DD".into(),
        ..UserConfig::default()
    }
}

/// `config` with Sort transactions by date = Ascending (oldest first): for tests
/// written before revision 27 made Descending the default, where row order or row
/// numbers matter.
pub fn ascending(config: UserConfig) -> UserConfig {
    UserConfig {
        date_order: mk_ui::DateOrder::Ascending,
        ..config
    }
}

pub fn new_book_app() -> (App, TestBook) {
    new_book_app_with(iso_config())
}

pub fn new_book_app_with(config: UserConfig) -> (App, TestBook) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.moneykeep");
    let mut app = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        config,
    );
    app.create_book(path.to_str().unwrap(), "Test Books", "USD");
    (app, TestBook { dir, path })
}

/// A harness over a fresh book with the standard test chart: Assets:Checking (bank),
/// Expenses:Groceries, Liabilities:Visa (credit card), Income:Salary.
pub fn harness_with_chart() -> (Harness, TestBook) {
    harness_with_chart_config(iso_config())
}

/// [`harness_with_chart`] with a given user config.
pub fn harness_with_chart_config(config: UserConfig) -> (Harness, TestBook) {
    let (app, book) = new_book_app_with(config);
    let mut h = harness(app);
    for args in [
        json!({"name": "Assets", "type": "asset", "placeholder": true}),
        json!({"name": "Checking", "type": "asset", "subtype": "bank", "parent": "Assets"}),
        json!({"name": "Expenses", "type": "expense", "placeholder": true}),
        json!({"name": "Groceries", "type": "expense", "parent": "Expenses"}),
        json!({"name": "Liabilities", "type": "liability", "placeholder": true}),
        json!({"name": "Visa", "type": "liability", "subtype": "credit-card", "parent": "Liabilities"}),
        json!({"name": "Salary", "type": "income"}),
    ] {
        cmd(&mut h, "account.create", args);
    }
    h.run();
    (h, book)
}

pub fn harness(app: App) -> Harness {
    Harness::new(app)
}

/// Run a command directly through the app's backend (test setup only; the UI
/// itself goes through views).
pub fn cmd(h: &mut Harness, command: &str, args: Value) -> Value {
    let v = h.state_mut().test_invoke(command, args.clone());
    h.views().refresh();
    v.unwrap_or_else(|e| panic!("{command} {args}: {}", e.message))
}

pub fn has(h: &Harness, label: &str) -> bool {
    h.query_by_label(label).is_some()
}

pub fn has_contains(h: &Harness, part: &str) -> bool {
    h.query_by_label_contains(part).is_some()
}

pub fn click(h: &mut Harness, label: &str) {
    h.get_by_label(label).click();
    h.run();
}

pub fn type_into(h: &mut Harness, label: &str, text: &str) {
    h.get_by_label(label).focus();
    h.run();
    h.get_by_label(label).type_text(text);
    h.run();
}

pub fn key(h: &mut Harness, k: Key) {
    h.key_press(k);
    h.run();
}

/// The text value of a field (empty when none).
pub fn value(h: &Harness, label: &str) -> String {
    h.get_by_label(label).value().unwrap_or_default()
}

pub fn disabled(h: &Harness, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().is_disabled()
}

pub fn events(h: &mut Harness) -> Vec<AppEvent> {
    h.state_mut().take_events()
}

/// Open the register of an account through the accounts pane (revision 23): the
/// All accounts tab opens the pane when the account is not already shown.
pub fn open_register(h: &mut Harness, full_name: &str) {
    let label = format!("Account {full_name}");
    if !has(h, &label) {
        open_pane(h, "All accounts");
    }
    click(h, &label);
}

/// Open "New transaction" in the open register with its + New button (revision
/// 24), unless the pop-up is already showing it.
pub fn open_new(h: &mut Harness) {
    if !has(h, "New transaction") {
        click(h, "+ New");
    }
}

/// Type into the New transaction pop-up's fields, given by their label in lower
/// case ("date", "payee", "category", "spent", …). Date and amounts are selected
/// when reached, so typing replaces them.
pub fn fill_new(h: &mut Harness, fields: &[(&str, &str)]) {
    for (field, text) in fields {
        let label = format!("New {field}");
        if *field == "date" || *field == "memo" {
            h.get_by_label(&label).focus();
            h.run();
            h.key_press_modifiers(Modifiers::COMMAND, Key::A);
            h.get_by_label(&label).type_text(text);
            h.run();
        } else {
            type_into(h, &label, text);
        }
    }
}

/// Add a transaction through the pop-up: + New, type the fields, then Save.
pub fn add_txn(h: &mut Harness, fields: &[(&str, &str)]) {
    open_new(h);
    fill_new(h, fields);
    click(h, "Save");
}

/// A rail tab by its group name ("All accounts", "Cash & bank", …); group tabs'
/// accessible names carry the total after a comma.
pub fn rail_tab<'h>(h: &'h Harness, group: &str) -> Node<'h> {
    let prefix = format!("{group},");
    h.query_all_by_role(Role::Tab)
        .find(|n| {
            let l = n.accesskit_node().label().unwrap_or_default();
            l == group || l.starts_with(&prefix)
        })
        .unwrap_or_else(|| panic!("no rail tab {group}"))
}

/// Click a rail tab, opening (or closing) the accounts pane on its group.
pub fn open_pane(h: &mut Harness, group: &str) {
    rail_tab(h, group).click();
    h.run();
}

/// A config with the accounts pane pinned open on All accounts: the layout the
/// app had before revision 23 (an always-visible account list beside the main
/// area).
pub fn pinned_config(mut config: UserConfig) -> UserConfig {
    config.accounts_pane = mk_ui::AccountsPaneConfig {
        pinned: true,
        group: String::new(),
        open: true,
    };
    config
}

/// The top bar's Features menu button (revision 25), when shown.
pub fn features_button<'h>(h: &'h Harness) -> Option<Node<'h>> {
    h.query_by_role_and_label(Role::Button, "Features")
}

/// The items of the Features menu, top to bottom (opening and closing it); empty
/// when Features is hidden.
pub fn feature_items(h: &mut Harness) -> Vec<String> {
    let Some(button) = features_button(h) else {
        return Vec::new();
    };
    button.click();
    h.run();
    let mut items: Vec<(f32, String)> = h
        .query_all_by_role(Role::MenuItemCheckBox)
        .map(|n| {
            (
                n.rect().top(),
                n.accesskit_node().label().unwrap_or_default(),
            )
        })
        .collect();
    items.sort_by(|a, b| a.0.total_cmp(&b.0));
    features_button(h).unwrap().click();
    h.run();
    items.into_iter().map(|(_, l)| l).collect()
}

/// Open a module screen ("Budget", "Purchases", "Sales", "Securities") from the
/// top bar's Features menu (revision 25; they were navigation items before).
pub fn open_feature(h: &mut Harness, item: &str) {
    features_button(h)
        .unwrap_or_else(|| panic!("no Features menu for {item}"))
        .click();
    h.run();
    h.get_by_role_and_label(Role::MenuItemCheckBox, item)
        .click();
    h.run();
}

/// Click a section in Settings' section list ("Display", "Features", …): its tab,
/// not the top bar's Features menu.
pub fn settings_section(h: &mut Harness, name: &str) {
    h.get_by_role_and_label(Role::Tab, name).click();
    h.run();
}

/// Labels of every node whose label contains `part`.
pub fn labels_containing(h: &Harness, part: &str) -> Vec<String> {
    h.query_all_by_label_contains(part)
        .filter_map(|n| {
            n.accesskit_node()
                .label()
                .or_else(|| n.accesskit_node().value())
        })
        .collect()
}

pub fn fixture(path: &str) -> Vec<u8> {
    let p = format!(
        "{}/../../examples/fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

/// The book file at `path` is password protected on disk. On Windows an open
/// encrypted book's lock covers its bytes, so its file can't be read while the app
/// has it open; that lock (`book_in_use`) is only ever an encrypted book's.
pub fn encrypted_on_disk(path: &std::path::Path) -> bool {
    match mk_commands::book_kind(path) {
        Ok(kind) => kind == mk_commands::BookKind::Encrypted,
        Err(e) => cfg!(windows) && e.code() == "book_in_use",
    }
}
