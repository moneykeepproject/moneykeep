//! `moneykeep-app`: the MoneyKeep desktop app.
//!
//! This crate owns everything platform-specific: the Slint window, native file dialogs,
//! dropped files, the per-user config file and threads. All book logic is in
//! registry commands, run on the backend's worker thread.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod backend;
mod config;
mod fonts;
mod receipt_viewer;
mod window;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};

use mk_core::brand;
use mk_ui::AppEvent;
use mk_views::Views;
use slint::ComponentHandle as _;

/// The file dialogs' name for books.
const BOOK_FILTER: &str = "MoneyKeep book";

struct DesktopApp {
    /// The window with the controller (`mk_ui::ctl::App`) inside.
    views: Views,
    watch_handle: backend::WatchHandle,
    book_path: Option<String>,
    watch_stop: Option<Arc<AtomicBool>>,
    watch_rx: Option<Receiver<String>>,
    window_save: window::Debounce,
    /// Temporary copies of PDF receipts opened in the system viewer (deleted when
    /// the book closes and on exit; leftovers removed on start).
    pdf_copies: receipt_viewer::TempCopies,
    /// The control channel for AI assistants (mcp-server.md revision 3): listening
    /// only while the open book allows AI assistants.
    control: Option<mk_control::server::ControlServer>,
    control_host: Arc<backend::WorkerHost>,
    /// The controller's change counter last told to the control channel.
    seen_generation: u64,
}

impl DesktopApp {
    fn new() -> Result<Self, slint::PlatformError> {
        let config = config::load();
        let worker = backend::WorkerBackend::start();
        let watch_handle = worker.watch_handle();
        let control_host = Arc::new(worker.control_host());
        let mut app = mk_ui::ctl::App::new(Box::new(worker), config);
        app.set_font_provider(Box::new(fonts::SystemFonts::discover()));
        if let Some(dir) = config::folder() {
            app.set_settings_folder(dir.display().to_string());
        }
        let book_path = review_start(&mut app);
        // The theme, text size and fonts are in the first view model (no jump).
        let views = Views::new(app)?;
        Ok(Self {
            views,
            watch_handle,
            book_path,
            watch_stop: None,
            watch_rx: None,
            window_save: window::Debounce::default(),
            pdf_copies: receipt_viewer::TempCopies::start(),
            control: None,
            control_host,
            seen_generation: 0,
        })
    }

    fn handle(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::ChooseNewBookPath { name, currency } => {
                let file = rfd::FileDialog::new()
                    .add_filter(BOOK_FILTER, &[brand::BOOK_EXTENSION])
                    .set_file_name(format!("{name}.{}", brand::BOOK_EXTENSION))
                    .save_file();
                if let Some(p) = file.and_then(|p| p.to_str().map(str::to_owned)) {
                    self.views.app_mut().create_book(&p, &name, &currency);
                    self.book_path = Some(p);
                }
            }
            AppEvent::ChooseBookToOpen => {
                let file = rfd::FileDialog::new()
                    .add_filter(BOOK_FILTER, &[brand::BOOK_EXTENSION])
                    .pick_file();
                if let Some(p) = file.and_then(|p| p.to_str().map(str::to_owned)) {
                    self.views.app_mut().open_book(&p);
                    self.book_path = Some(p);
                }
            }
            AppEvent::ChooseImportFile => {
                let file = rfd::FileDialog::new()
                    .add_filter(
                        "Bank and broker files",
                        &["ofx", "qfx", "qbo", "qif", "iif", "csv", "json", "xml"],
                    )
                    .pick_file();
                if let Some(p) = file {
                    let name = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("file")
                        .to_owned();
                    match std::fs::read(&p) {
                        Ok(bytes) => self.views.app_mut().import_file(&name, &bytes),
                        Err(e) => self
                            .views
                            .app_mut()
                            .watch_reported(&format!("Cannot read {name}: {e}")),
                    }
                }
            }
            AppEvent::ChoosePriceFile => {
                let file = rfd::FileDialog::new()
                    .add_filter("Prices (CSV)", &["csv"])
                    .pick_file();
                if let Some(p) = file {
                    let name = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("prices.csv")
                        .to_owned();
                    match std::fs::read(&p) {
                        Ok(bytes) => self.views.app_mut().import_prices(&name, &bytes),
                        Err(e) => self
                            .views
                            .app_mut()
                            .watch_reported(&format!("Cannot read {name}: {e}")),
                    }
                }
            }
            AppEvent::ChooseDefinitionFile => {
                let file = rfd::FileDialog::new()
                    .add_filter(
                        format!("{} definition", brand::PRODUCT),
                        &[brand::DEFINITION_EXTENSION],
                    )
                    .pick_file();
                if let Some(p) = file {
                    let name = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("definition")
                        .to_owned();
                    match std::fs::read(&p) {
                        Ok(bytes) => self.views.app_mut().open_definition(&name, &bytes),
                        Err(e) => self
                            .views
                            .app_mut()
                            .watch_reported(&format!("Cannot read {name}: {e}")),
                    }
                }
            }
            AppEvent::ChooseWatchFolder => {
                if let Some(p) = rfd::FileDialog::new()
                    .pick_folder()
                    .and_then(|p| p.to_str().map(str::to_owned))
                {
                    self.views.app_mut().set_watch_folder(&p);
                }
            }
            AppEvent::Watch(on) => self.watch(on),
            AppEvent::ConfigChanged => config::save(&self.views.app_mut().config),
            AppEvent::ResetSettings { copy_name } => {
                // app-desktop.md revision 41: the copy first; nothing changes
                // when it can't be saved.
                let current = self.views.app().config.clone();
                let saved = match config::folder() {
                    Some(dir) => config::reset_in(&dir, &copy_name, &current)
                        .map(|p| p.display().to_string()),
                    None => Err("no settings folder on this computer".to_owned()),
                };
                self.book_path = None;
                self.views.app_mut().finish_settings_reset(saved);
            }
            AppEvent::SaveRecoveryKey { file_name, content } => {
                // Suggest the documents (or home) folder, not the book's folder.
                let mut dialog = rfd::FileDialog::new().set_file_name(&file_name);
                if let Some(dirs) = directories::UserDirs::new() {
                    let dir = dirs.document_dir().unwrap_or(dirs.home_dir()).to_path_buf();
                    dialog = dialog.set_directory(dir);
                }
                if let Some(p) = dialog.save_file() {
                    match std::fs::write(&p, content) {
                        Ok(()) => self
                            .views
                            .app_mut()
                            .watch_reported(&format!("Saved the recovery key at {}", p.display())),
                        Err(e) => self
                            .views
                            .app_mut()
                            .watch_reported(&format!("Couldn't save {}: {e}", p.display())),
                    }
                }
            }
            AppEvent::ChooseUnencryptedCopyPath { file_name } => {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter(BOOK_FILTER, &[brand::BOOK_EXTENSION])
                    .set_file_name(&file_name)
                    .save_file()
                    .and_then(|p| p.to_str().map(str::to_owned))
                {
                    self.views.app_mut().save_unencrypted_copy(&p);
                }
            }
            AppEvent::SaveBytes { file_name, data } => {
                if let Some(p) = rfd::FileDialog::new().set_file_name(&file_name).save_file() {
                    match std::fs::write(&p, data) {
                        Ok(()) => self
                            .views
                            .app_mut()
                            .watch_reported(&format!("Saved {}", p.display())),
                        Err(e) => self
                            .views
                            .app_mut()
                            .watch_reported(&format!("Couldn't save {}: {e}", p.display())),
                    }
                }
            }
            AppEvent::SaveFilesToFolder { files } => {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    let mut failed = Vec::new();
                    for (name, data) in &files {
                        // Only the file name: never a path from the book.
                        let safe = std::path::Path::new(name)
                            .file_name()
                            .map_or_else(|| "file.pdf".into(), |n| n.to_os_string());
                        if let Err(e) = std::fs::write(dir.join(&safe), data) {
                            failed.push(format!("{name}: {e}"));
                        }
                    }
                    if failed.is_empty() {
                        self.views.app_mut().watch_reported(&format!(
                            "Saved {} files in {}",
                            files.len(),
                            dir.display()
                        ));
                    } else {
                        self.views
                            .app_mut()
                            .watch_reported(&format!("Couldn't save {}", failed.join("; ")));
                    }
                }
            }
            AppEvent::ChooseLogoFile { template } => {
                let file = rfd::FileDialog::new()
                    .add_filter("Logo image", &["png", "jpg", "jpeg"])
                    .pick_file();
                if let Some(p) = file {
                    match std::fs::read(&p) {
                        Ok(bytes) => self.views.app_mut().set_template_logo(&template, &bytes),
                        Err(e) => self
                            .views
                            .app_mut()
                            .watch_reported(&format!("Cannot read {}: {e}", p.display())),
                    }
                }
            }
            AppEvent::ChooseBookIconFile => {
                let file = rfd::FileDialog::new()
                    .add_filter("Picture", &["png", "jpg", "jpeg"])
                    .pick_file();
                if let Some(p) = file {
                    // Larger files are refused by the command without reading them all.
                    let too_big = std::fs::metadata(&p).is_ok_and(|m| m.len() > 2 * 1024 * 1024);
                    let bytes = if too_big {
                        Ok(Vec::new())
                    } else {
                        std::fs::read(&p)
                    };
                    match bytes {
                        Ok(bytes) => self.views.app_mut().set_book_icon(&bytes),
                        Err(e) => self
                            .views
                            .app_mut()
                            .watch_reported(&format!("Cannot read {}: {e}", p.display())),
                    }
                }
            }
            AppEvent::ChooseReceiptFiles { target } => {
                let files = rfd::FileDialog::new()
                    .add_filter("Receipt", &["png", "jpg", "jpeg", "pdf", "heic", "heif"])
                    .pick_files()
                    .unwrap_or_default();
                let picked: Vec<mk_ui::PickedFile> = files
                    .iter()
                    .map(|p| {
                        let name = p
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("receipt")
                            .to_owned();
                        // Larger files are refused without reading them.
                        let too_big =
                            std::fs::metadata(p).is_ok_and(|m| m.len() > mk_ui::MAX_RECEIPT_BYTES);
                        let bytes = if too_big {
                            Err(mk_ui::TOO_BIG.to_owned())
                        } else {
                            std::fs::read(p).map_err(|e| format!("Cannot read the file: {e}"))
                        };
                        mk_ui::PickedFile { name, bytes }
                    })
                    .collect();
                if !picked.is_empty() {
                    self.views.app_mut().add_receipt_files(target, picked);
                }
            }
            AppEvent::OpenReceiptPdf { file_name, data } => {
                match self.pdf_copies.open(&file_name, &data) {
                    Ok(()) => {}
                    Err(e) => self
                        .views
                        .app_mut()
                        .watch_reported(&format!("Couldn't open {file_name}: {e}")),
                }
            }
            AppEvent::SaveExpenseReport {
                file_name,
                data,
                expenses,
            } => {
                // Cancelling the dialog, or a failed write, marks nothing.
                if let Some(p) = rfd::FileDialog::new().set_file_name(&file_name).save_file() {
                    match std::fs::write(&p, data) {
                        Ok(()) => {
                            self.views
                                .app_mut()
                                .watch_reported(&format!("Saved {}", p.display()));
                            self.views.app_mut().expense_report_saved(&expenses);
                        }
                        Err(e) => self
                            .views
                            .app_mut()
                            .watch_reported(&format!("Couldn't save {}: {e}", p.display())),
                    }
                }
            }
            AppEvent::BookClosed => {
                self.pdf_copies.clear();
                self.stop_control(mk_control::server::CloseReason::BookClosed);
            }
            AppEvent::BookLocked => self.stop_control(mk_control::server::CloseReason::Locked),
            AppEvent::BookOpened { path } => {
                // Another book may replace the open one without a close.
                self.stop_control(mk_control::server::CloseReason::BookClosed);
                self.book_path = Some(path.clone());
                let snippets = keep_mcp_snippets(&path);
                self.views.app_mut().ai_keep_mcp(snippets);
                self.ai_sync(false);
            }
            AppEvent::AiAccessChanged => self.ai_sync(true),
            AppEvent::AiAnswer { id, allow } => {
                if let Some(c) = &self.control {
                    c.answer(id, allow);
                }
            }
            AppEvent::AiDisconnect { client } => {
                if let Some(c) = &self.control {
                    c.disconnect(&client);
                }
            }
            AppEvent::ChooseBackupFolder { second } => {
                if let Some(p) = rfd::FileDialog::new()
                    .pick_folder()
                    .and_then(|p| p.to_str().map(str::to_owned))
                {
                    self.views.app_mut().set_backup_folder(&p, second);
                }
            }
            AppEvent::OpenFolder { path } => {
                let dir = std::path::Path::new(&path);
                let _ = std::fs::create_dir_all(dir);
                if let Err(e) = open_folder(dir) {
                    self.views
                        .app_mut()
                        .watch_reported(&format!("Couldn't open the folder {path}: {e}"));
                }
            }
            AppEvent::OpenThirdPartyLicences => {
                if let Err(e) = open_third_party_licences() {
                    self.views.app_mut().watch_reported(&e);
                }
            }
            AppEvent::ChooseBackupFile => {
                let mut dialog =
                    rfd::FileDialog::new().add_filter(BOOK_FILTER, &[brand::BOOK_EXTENSION]);
                if let Some(d) = mk_commands::backups::default_folder().filter(|d| d.is_dir()) {
                    dialog = dialog.set_directory(d);
                }
                if let Some(p) = dialog
                    .pick_file()
                    .and_then(|p| p.to_str().map(str::to_owned))
                {
                    self.views.app_mut().restore_from_file(&p);
                }
            }
            AppEvent::ChooseRestoreTarget { dir, file_name } => {
                let mut dialog = rfd::FileDialog::new()
                    .add_filter(BOOK_FILTER, &[brand::BOOK_EXTENSION])
                    .set_file_name(&file_name);
                if let Some(d) = dir.filter(|d| std::path::Path::new(d).is_dir()) {
                    dialog = dialog.set_directory(d);
                }
                if let Some(p) = dialog
                    .save_file()
                    .and_then(|p| p.to_str().map(str::to_owned))
                {
                    self.views.app_mut().restore_to(&p);
                }
            }
            AppEvent::ShowInFolder { path } => {
                if let Err(e) = show_in_folder(std::path::Path::new(&path)) {
                    self.views
                        .app_mut()
                        .watch_reported(&format!("Couldn't show the folder of {path}: {e}"));
                }
            }
            AppEvent::OpenLink { url } => {
                if let Err(e) = open_link(&url) {
                    self.views
                        .app_mut()
                        .watch_reported(&format!("Couldn't open {url}: {e}"));
                }
            }
            AppEvent::SaveFile { file_name, content } => {
                if let Some(p) = rfd::FileDialog::new().set_file_name(&file_name).save_file() {
                    match std::fs::write(&p, content) {
                        Ok(()) => self
                            .views
                            .app_mut()
                            .watch_reported(&format!("Saved {}", p.display())),
                        Err(e) => self
                            .views
                            .app_mut()
                            .watch_reported(&format!("Couldn't save {}: {e}", p.display())),
                    }
                }
            }
        }
    }

    /// Run the shared watch loop on a background thread (actor `watch`).
    fn watch(&mut self, on: bool) {
        if let Some(stop) = self.watch_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.watch_rx = None;
        if !on {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let flag = Arc::clone(&stop);
        // An encrypted book cannot be opened a second time: run the passes on the
        // worker's own session.
        if self.watch_handle.encrypted() {
            let handle = self.watch_handle.clone();
            let spawned = std::thread::Builder::new()
                .name("moneykeep-watch".into())
                .spawn(move || {
                    while !flag.load(Ordering::Relaxed) {
                        match handle.pass() {
                            Ok(msgs) => {
                                for m in msgs {
                                    let _ = tx.send(m);
                                }
                            }
                            Err(e) => {
                                let _ = tx.send(format!("Watching stopped: {e}"));
                                return;
                            }
                        }
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                });
            if spawned.is_ok() {
                self.watch_stop = Some(stop);
                self.watch_rx = Some(rx);
            }
            return;
        }
        let Some(path) = self.book_path.clone() else {
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("moneykeep-watch".into())
            .spawn(move || {
                let result = mk_watch::watch_book(
                    std::path::Path::new(&path),
                    std::time::Duration::from_secs(2),
                    &flag,
                    |o| {
                        let text = match o {
                            mk_watch::Outcome::Staged { file, .. } => {
                                format!("Imported {} for review.", file.display())
                            }
                            mk_watch::Outcome::Failed { file, error } => {
                                format!("Could not import {}: {error}", file.display())
                            }
                            mk_watch::Outcome::Skipped { error, .. } => error.clone(),
                        };
                        let _ = tx.send(text);
                    },
                );
                if let Err(e) = result {
                    let _ = tx.send(format!("Watching stopped: {e}"));
                }
            });
        if spawned.is_ok() {
            self.watch_stop = Some(stop);
            self.watch_rx = Some(rx);
        }
    }
}

impl DesktopApp {
    /// Listen on the control channel exactly while the open book allows AI
    /// assistants; after a settings change, connected clients are told.
    fn ai_sync(&mut self, settings_changed: bool) {
        let enabled = self.control_host.ai_enabled();
        if !enabled {
            self.stop_control(mk_control::server::CloseReason::AccessOff);
            self.views.app_mut().ai_other_window(false);
            return;
        }
        if let Some(c) = &self.control {
            if settings_changed {
                c.settings_changed();
            }
            return;
        }
        let Some(dir) = mk_control::config_dir() else {
            return;
        };
        let host: Arc<dyn mk_control::server::Host> = self.control_host.clone();
        match mk_control::server::ControlServer::start(
            &dir,
            host,
            Arc::new(mk_control::server::SystemClock::default()),
        ) {
            Ok(c) => {
                self.seen_generation = self.views.app().generation();
                self.control = Some(c);
                self.views.app_mut().ai_other_window(false);
            }
            Err(mk_control::server::StartError::OtherWindow) => {
                self.views.app_mut().ai_other_window(true);
            }
            Err(e) => self.views.app_mut().watch_reported(&e.to_string()),
        }
    }

    fn stop_control(&mut self, reason: mk_control::server::CloseReason) {
        if let Some(c) = self.control.take() {
            c.stop(reason);
        }
        self.views.app_mut().ai_connections(Vec::new());
    }

    /// Control-channel events for the controller; the window asks for attention
    /// when an assistant waits for approval.
    fn control_events(&mut self) {
        let Some(c) = &self.control else { return };
        let events = c.poll_events();
        let generation = self.views.app().generation();
        if generation != self.seen_generation {
            self.seen_generation = generation;
            c.book_changed();
        }
        if events.is_empty() {
            return;
        }
        let mut attention = false;
        self.views.with_app(|a| {
            for e in events {
                match e {
                    mk_control::server::ControlEvent::ApprovalRequested {
                        id,
                        client,
                        book_name,
                        access,
                    } => {
                        attention = true;
                        a.ai_approval_requested(id, &client, &book_name, &access.access_text());
                    }
                    mk_control::server::ControlEvent::ApprovalEnded { id } => {
                        a.ai_approval_ended(id);
                    }
                    mk_control::server::ControlEvent::Connections(list) => a.ai_connections(
                        list.into_iter()
                            .map(|c| mk_ui::ctl::ai::AiClient {
                                access: c.access.access_text(),
                                name: c.client,
                            })
                            .collect(),
                    ),
                    mk_control::server::ControlEvent::BookChanged => a.ai_book_changed(),
                }
            }
        });
        self.seen_generation = self.views.app().generation();
        if attention {
            use slint::winit_030::{WinitWindowAccessor, winit};
            self.views.window().window().with_winit_window(|w| {
                w.request_user_attention(Some(winit::window::UserAttentionType::Critical));
            });
        }
    }

    /// Remember the window's size, position and maximised state (saved at most
    /// once a second; also on exit).
    fn track_window(&mut self) {
        let win = self.views.window().window();
        let last = self.views.app().config.window.unwrap_or(window::DEFAULT);
        let now = window::current(win, last);
        if self.views.app().config.window != Some(now) {
            self.views.app_mut().config.window = Some(now);
            self.window_save.touch();
        }
        if self.window_save.due() {
            config::save(&self.views.app().config);
        }
    }
}

/// Review aid (no effect unless set): open a book and a view at start-up, e.g.
/// `MONEYKEEP_OPEN=/path/book.moneykeep MONEYKEEP_VIEW=register:Assets:Checking`
/// (or `import:/path/file.ofx`). Used for screenshots on a virtual display.
fn review_start(app: &mut mk_ui::ctl::App) -> Option<String> {
    let path = std::env::var("MONEYKEEP_OPEN").ok()?;
    app.open_book(&path);
    if let Ok(view) = std::env::var("MONEYKEEP_VIEW") {
        match view.strip_prefix("import:") {
            Some(file) => {
                let name = std::path::Path::new(file)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("file")
                    .to_owned();
                if let Ok(bytes) = std::fs::read(file) {
                    app.import_file(&name, &bytes);
                }
            }
            None => app.open_view_spec(&view),
        }
    }
    Some(path)
}

impl DesktopApp {
    /// After user input or time passing: watched-folder reports, the controller's
    /// requests, the window state, and text copied to the clipboard.
    fn after(&mut self) {
        if let Some(rx) = &self.watch_rx {
            let msgs: Vec<String> = rx.try_iter().collect();
            if !msgs.is_empty() {
                self.views.with_app(|a| {
                    for m in &msgs {
                        a.watch_reported(m);
                    }
                });
            }
        }
        self.control_events();
        // Handling an event can raise another (a dialog result shows a message).
        for _ in 0..4 {
            let events = self.views.app_mut().take_events();
            if events.is_empty() {
                break;
            }
            for ev in events {
                self.handle(ev);
            }
            self.views.refresh();
        }
        self.track_window();
        let copied = self.views.app_mut().take_copied();
        for text in copied {
            if let Ok(mut c) = arboard::Clipboard::new() {
                let _ = c.set_text(text);
            }
        }
    }

    /// The window is closing: stop watching, save the config, remove temporary
    /// copies.
    fn exit(&mut self) {
        if let Some(stop) = self.watch_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        // backups.md: the closing backup, finished before the app goes.
        self.views.app_mut().shutdown();
        config::save(&self.views.app().config);
        self.pdf_copies.clear();
        self.stop_control(mk_control::server::CloseReason::AppExit);
    }
}

/// The `keep-mcp config` snippets for `book`, when keep-mcp is installed next to
/// the app or on `PATH`.
fn keep_mcp_snippets(book: &str) -> Option<Vec<(String, String)>> {
    let app_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf));
    let path = std::env::var_os("PATH");
    let exe = mk_control::find_keep_mcp(app_dir.as_deref(), path.as_deref())?;
    let exe = std::fs::canonicalize(&exe)
        .unwrap_or(exe)
        .display()
        .to_string();
    let args = vec![mk_control::canonical(std::path::Path::new(book))];
    Some(
        mk_control::ClientKind::ALL
            .iter()
            .map(|k| {
                let v = mk_control::config_snippet(*k, &exe, &args);
                (
                    k.label().to_owned(),
                    serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()),
                )
            })
            .collect(),
    )
}

/// Largest file read when dropped onto the window.
const MAX_DROPPED_BYTES: u64 = 256 * 1024 * 1024;

/// A file dropped onto the window, read for the UI.
fn dropped(path: &std::path::Path) -> mk_ui::PickedFile {
    let bytes = match std::fs::metadata(path) {
        Ok(m) if m.len() > MAX_DROPPED_BYTES => Err("the file is too large".to_owned()),
        Ok(_) => std::fs::read(path).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_owned();
    mk_ui::PickedFile { name, bytes }
}

/// MoneyKeep's own calculator icon for the window and taskbar (app-desktop.md
/// revision 25), drawn by the UI crate at start-up: RGBA pixels and the side
/// length. (The Windows executable's file icon is not embedded: that needs a
/// resource compiler at build time, rc.exe or windres, which the Linux CI build
/// does not have.)
fn window_icon() -> (Vec<u8>, u32) {
    let side = mk_ui::APP_ICON_SIZE;
    (mk_ui::app_icon_rgba(side), side)
}

/// `--version`: the product name and version.
fn version_text() -> String {
    format!("{} {}", brand::PRODUCT, env!("CARGO_PKG_VERSION"))
}

#[allow(clippy::print_stdout)]
fn main() -> Result<(), slint::PlatformError> {
    if std::env::args()
        .nth(1)
        .is_some_and(|a| a == "--version" || a == "-V")
    {
        println!("{}", version_text());
        return Ok(());
    }
    use slint::winit_030::{WinitWindowAccessor, winit};
    // The Slint window with MoneyKeep's views (ui-slint.md revision 2).
    let desktop = Rc::new(RefCell::new(DesktopApp::new()?));
    let views = desktop.borrow().views.clone();
    let win = views.window().window();
    let saved = config::load().window.unwrap_or(window::DEFAULT);
    // The saved size now; position, minimum size and the off-screen fallback once
    // the window exists and the screens are known.
    window::apply(win, &window::restore(&saved, &[]));
    let after = Rc::downgrade(&desktop);
    views.on_after_action(move || {
        if let Some(d) = after.upgrade()
            && let Ok(mut d) = d.try_borrow_mut()
        {
            d.after();
        }
    });
    let exit_app = Rc::clone(&desktop);
    win.on_close_requested(move || {
        exit_app.borrow_mut().exit();
        slint::CloseRequestResponse::HideWindow
    });
    // The controller's clock: timers (idle lock, rail, syncs, phone capture).
    let started = std::time::Instant::now();
    let tick_app = Rc::downgrade(&desktop);
    let ticker = slint::Timer::default();
    ticker.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(100),
        move || {
            if let Some(d) = tick_app.upgrade() {
                let v = d.borrow().views.clone();
                v.tick(started.elapsed().as_secs_f64());
                if let Ok(mut d) = d.try_borrow_mut() {
                    d.after();
                }
            }
        },
    );
    let setup_views = views.clone();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        let win = setup_views.window().window();
        let known = window::screens(win);
        window::apply(win, &window::restore(&saved, &known));
        let (rgba, side) = window_icon();
        win.with_winit_window(|w| {
            w.set_window_icon(winit::window::Icon::from_rgba(rgba, side, side).ok());
        });
        // Files dropped onto the window go to the controller.
        let drop_views = setup_views.clone();
        win.on_winit_window_event(move |_, event| {
            if let winit::event::WindowEvent::DroppedFile(path) = event {
                let file = dropped(path);
                drop_views.with_app(|a| a.drop_files(vec![file]));
            }
            slint::winit_030::EventResult::Propagate
        });
    });
    views.show()?;
    slint::run_event_loop()
}

/// Open a Cloudflare page in the default browser (phone capture setup,
/// app-desktop.md revision 29). Only Cloudflare's dashboard and documentation
/// addresses are opened.
/// Show a book's folder in the system's file manager (the start screen's SHOW IN
/// FOLDER, app-desktop.md revision 40): Explorer selects the file; elsewhere the
/// folder opens. The path goes to the program as one argument, never through a
/// shell.
fn show_in_folder(path: &std::path::Path) -> Result<(), String> {
    let folder = path
        .parent()
        .filter(|f| f.is_dir())
        .ok_or_else(|| "the folder can't be found".to_owned())?;
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("explorer");
        if path.is_file() {
            let mut arg = std::ffi::OsString::from("/select,");
            arg.push(path.as_os_str());
            c.arg(arg);
        } else {
            c.arg(folder);
        }
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        if path.is_file() {
            c.arg("-R").arg(path);
        } else {
            c.arg(folder);
        }
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(folder);
        c
    };
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Open a folder in the system's file manager (Settings → Backups); the path goes
/// to the program as one argument, never through a shell.
fn open_folder(dir: &std::path::Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err("the folder can't be found".to_owned());
    }
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// `THIRD-PARTY-LICENSES.txt` beside this program (security-review-1 B4), opened
/// with the system's viewer for text files.
fn open_third_party_licences() -> Result<(), String> {
    let file = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("THIRD-PARTY-LICENSES.txt")))
        .filter(|f| f.is_file())
        .ok_or_else(|| {
            "The third-party licences file isn't installed beside MoneyKeep (it comes with the installer and cargo xtask release).".to_owned()
        })?;
    let program = if cfg!(target_os = "windows") {
        "notepad"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(&file)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Couldn't open {}: {e}", file.display()))
}

fn open_link(url: &str) -> Result<(), String> {
    let allowed = [
        "https://dash.cloudflare.com/",
        "https://developers.cloudflare.com/",
    ];
    // mobile-capture.md revision 18: Windows' network settings page (exactly).
    let settings = cfg!(target_os = "windows") && url == "ms-settings:network-status"
        // macos-release.md C5: System Settings → Firewall (exactly).
        || cfg!(target_os = "macos")
            && url == mk_commands::capture_wifi::text::MAC_FIREWALL_SETTINGS;
    if !(settings || allowed.iter().any(|a| url.starts_with(a)))
        || url.contains(['"', '\'', ' ', '&', '|', '^'])
    {
        return Err("this address is not opened by MoneyKeep".into());
    }
    let mut cmd = if cfg!(target_os = "windows") {
        std::process::Command::new("explorer")
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// app-desktop.md revision 30 test 182 (platform part): the desktop build puts
    /// copied text on the system clipboard through arboard (with Wayland support),
    /// and Slint's text fields use the system clipboard themselves. Checked on the
    /// manifests, as a headless test cannot reach the operating system clipboard.
    #[test]
    fn app_desktop_at182_system_clipboard_is_built_in() {
        let manifest = include_str!("../Cargo.toml");
        let arboard = manifest
            .lines()
            .find(|l| l.starts_with("arboard"))
            .unwrap_or_default();
        assert!(arboard.contains("wayland-data-control"), "{arboard}");
    }

    /// branding.md AT10 (desktop part): the window title and `--version` say
    /// MoneyKeep.
    #[test]
    fn branding_at10_title_and_version() {
        // The title is set in the window's .slint file.
        let ui = include_str!("../../../crates/mk-views/ui/app.slint");
        assert!(ui.contains("title: \"MoneyKeep\";"));
        assert_eq!(
            version_text(),
            format!("MoneyKeep {}", env!("CARGO_PKG_VERSION"))
        );
    }

    /// app-desktop.md AT158: the window gets an icon of at least 256×256.
    #[test]
    fn app_desktop_at158_window_icon_is_set() {
        let (rgba, side) = window_icon();
        assert!(side >= 256, "{side}");
        assert_eq!(rgba.len(), (side * side * 4) as usize);
    }
}
