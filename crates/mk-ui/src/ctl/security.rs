//! Password protection (book-encryption.md): the unlock prompt, the recovery key
//! dialog, Settings → Security and its dialogs, and the idle lock.

use super::App;
use super::ui::Ui;
use crate::base::security::{PASSPHRASE_NOTE, new_password_problem};
use crate::{AppEvent, Screen, SecurityOp, UiError, UiSecret};

const LEFTOVER_NOTE: &str =
    "Earlier unencrypted copies of this file may remain on the disk until they are overwritten.";

use crate::base::security::LOCK_CHOICES;

#[derive(Debug, Default)]
pub(crate) struct SecurityState {
    pub unlock: Option<Unlock>,
    pub recovery_key: Option<(String, bool)>,
    pub dialog: Option<Dialog>,
    pub book_path: Option<String>,
    /// When the user last gave any input (controller clock).
    pub last_input: Option<f64>,
    pub copy_pending: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Unlock {
    pub path: String,
    pub recovery: bool,
    pub text: String,
    pub error: Option<String>,
    pub unlocking: bool,
    pub damaged: Option<String>,
    pub focused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogKind {
    Protect,
    ChangePassword,
    NewRecoveryKey,
    RemoveRecoveryKey,
    SaveCopy,
    RemoveProtection,
}

#[derive(Debug, Clone)]
pub(crate) struct Dialog {
    pub kind: DialogKind,
    pub current: String,
    pub use_recovery: bool,
    pub new: String,
    pub confirm: String,
    pub recovery: bool,
    pub error: Option<String>,
}

impl Dialog {
    fn new(kind: DialogKind) -> Self {
        Dialog {
            kind,
            current: String::new(),
            use_recovery: false,
            new: String::new(),
            confirm: String::new(),
            recovery: true,
            error: None,
        }
    }

    fn secret(&self) -> UiSecret {
        if self.use_recovery {
            UiSecret::RecoveryKey(self.current.clone())
        } else {
            UiSecret::Password(self.current.clone())
        }
    }
}

fn book_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_owned()
}

impl App {
    /// Ask for the password of the protected book at `path`.
    pub(crate) fn prompt_unlock(&mut self, path: &str) {
        self.security.unlock = Some(Unlock {
            path: path.to_owned(),
            ..Unlock::default()
        });
    }

    /// The save dialog for an unencrypted copy returned a path.
    pub fn save_unencrypted_copy(&mut self, path: &str) {
        if !std::mem::take(&mut self.security.copy_pending) {
            return;
        }
        match self.backend.security(SecurityOp::SaveUnencryptedCopy {
            path: path.to_owned(),
        }) {
            // The name may have been taken meanwhile: the copy then has the next one.
            Ok(at) => {
                let at = at.unwrap_or_else(|| path.to_owned());
                self.notify(format!("Saved an unencrypted copy at {at}"));
            }
            Err(e) => self.notify_error(&e),
        }
    }

    fn show_recovery_key(&mut self, key: Option<String>) {
        if let Some(k) = key {
            self.security.recovery_key = Some((k, false));
        }
    }
}

/// Lock the book when it has been idle for the chosen time; returns whether it
/// locked.
pub(crate) fn lock_if_idle(app: &mut App) -> bool {
    let now = app.now;
    let last = *app.security.last_input.get_or_insert(now);
    let minutes = app.config.lock_after_minutes;
    if minutes == 0 || app.screen != Screen::Main || !app.backend.protection().0 {
        return false;
    }
    if crate::layout::elapsed(now, last, f64::from(minutes.saturating_mul(60))) {
        let path = app.security.book_path.clone();
        app.emit(AppEvent::BookLocked);
        app.close_book();
        if let Some(p) = path {
            app.prompt_unlock(&p);
        }
        app.security.last_input = Some(now);
        return true;
    }
    false
}

/// Run the unlock asked for with Open (after its spinner was shown).
pub(crate) fn finish_unlock(app: &mut App) {
    let Some(mut u) = app.security.unlock.take() else {
        return;
    };
    u.unlocking = false;
    let secret = if u.recovery {
        UiSecret::RecoveryKey(u.text.clone())
    } else {
        UiSecret::Password(u.text.clone())
    };
    match app.backend.open_book_with(&u.path, &secret) {
        Ok(()) => {
            let path = u.path.clone();
            app.opened(&path);
        }
        Err(e) if e.code == "backup_failed" => {
            let secret = Some(secret.clone());
            super::backups::open_failed(app, &u.path, secret, &e);
        }
        Err(e) => {
            apply_open_error(&mut u, &e);
            app.security.unlock = Some(u);
        }
    }
}

fn apply_open_error(u: &mut Unlock, e: &UiError) {
    u.text.clear();
    u.focused = false;
    match e.code.as_str() {
        "wrong_password" | "password_required" => {
            u.error = Some(if u.recovery {
                "Wrong recovery key.".into()
            } else {
                "Wrong password.".into()
            });
        }
        "book_damaged" => {
            u.error = None;
            u.damaged = Some(e.display());
        }
        _ => u.error = Some(e.display()),
    }
}

/// The name a book is shown by: its tile's name once it has been opened, else
/// its file name.
fn shown_name(app: &App, path: &str) -> String {
    app.config
        .recent
        .iter()
        .find(|r| r.path == path)
        .and_then(|r| r.tile.as_ref())
        .map(|t| t.name.clone())
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| book_name(path))
}

/// The password prompt (app-desktop.md revision 32): a compact card with a lock
/// and the book's name, one field centred in it, and icon buttons — Open (also
/// Enter), the recovery key switch, and Cancel as the × (also Esc).
fn unlock_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut u) = app.security.unlock.take() else {
        return;
    };
    let mut open = false;
    let cancel = ui.dialog_closed("unlock") || ui.key("escape");
    let mut previous = false;
    let name = shown_name(app, &u.path);
    ui.dialog("unlock", &name, 400, |ui| {
        ui.compact_dialog("lock");
        ui.close_label("Cancel");
        ui.group(|ui| {
            ui.row(|ui| {
                let other = if u.recovery {
                    "Use password instead"
                } else {
                    "Use recovery key instead"
                };
                ui.next().id("switch").tip(other);
                if ui.icon_button("key", other, false).clicked() {
                    u.recovery = !u.recovery;
                    u.text.clear();
                    u.error = None;
                    u.focused = false;
                }
                // Named after the switch, so the focus asked for lands on the
                // field that is shown.
                let label = if u.recovery {
                    "Recovery key"
                } else {
                    "Password"
                };
                if !u.focused {
                    ui.focus();
                    u.focused = true;
                }
                ui.next().grow().hint(label);
                let resp = if u.recovery {
                    ui.field(label, &mut u.text)
                } else {
                    ui.password(label, &mut u.text)
                };
                if resp.submitted {
                    open = true;
                }
                ui.next().id("open");
                if ui.primary_icon("unlock", "Open", u.unlocking).clicked() {
                    open = true;
                }
            });
            if let Some(e) = &u.error {
                ui.next().align(super::ui::Align::Center);
                ui.error_text(e.clone());
            }
            if let Some(d) = u.damaged.clone() {
                ui.next().align(super::ui::Align::Center);
                ui.error_text(d.clone());
                if d.contains(".prev") && ui.button("Open previous version").clicked() {
                    previous = true;
                }
            }
        });
    });
    if cancel {
        app.screen = Screen::Start;
        return;
    }
    if previous {
        let secret = if u.recovery {
            UiSecret::RecoveryKey(u.text.clone())
        } else {
            UiSecret::Password(u.text.clone())
        };
        match app.backend.open_previous(&u.path, &secret) {
            Ok(()) => {
                let path = u.path.clone();
                app.opened(&path);
                app.notify("Opened the previous version. The damaged file was kept with the ending .damaged.");
                return;
            }
            Err(e) => apply_open_error(&mut u, &e),
        }
    }
    if open && !u.text.is_empty() && !u.unlocking {
        u.unlocking = true;
        u.error = None;
    }
    app.security.unlock = Some(u);
}

fn recovery_dialog(app: &mut App, ui: &mut Ui) {
    let Some((key, mut stored)) = app.security.recovery_key.clone() else {
        return;
    };
    let mut done = false;
    let mut save = false;
    let _ = ui.dialog_closed("recovery-key");
    ui.dialog("recovery-key", "Your recovery key", 540, |ui| {
        ui.group(|ui| {
            ui.label(
                "Without the password and without this key, nobody — including MoneyKeep — can \
                 open the book. Keep it somewhere safe, away from the book file.",
            );
        });
        ui.card("", |ui| {
            ui.next().mono().bold().named("Recovery key");
            ui.big_heading(key.clone());
            ui.row(|ui| {
                if ui.button("Copy").clicked() {
                    app.copy(key.clone());
                }
                if ui.button("Save to file…").clicked() {
                    save = true;
                }
            });
        });
        ui.group(|ui| {
            ui.checkbox(&mut stored, "I've stored this key somewhere safe");
        });
        ui.actions(|ui| {
            ui.enabled(stored);
            if ui.primary("Done").clicked() {
                done = true;
            }
        });
    });
    if save {
        let name = app.book_name().unwrap_or_else(|| "book".into());
        app.emit(AppEvent::SaveRecoveryKey {
            file_name: format!("{name} recovery key.txt"),
            content: format!(
                "MoneyKeep recovery key for {name}\r\n{key}\r\n\r\nWithout the password and without this key, nobody can open the book.\r\n"
            ),
        });
    }
    app.security.recovery_key = if done { None } else { Some((key, stored)) };
}

/// Settings → Security.
pub(crate) fn settings_section(app: &mut App, ui: &mut Ui) {
    let (protected, has_key) = app.backend.protection();
    ui.card("Password protection", |ui| {
        if protected {
            ui.label("This book is password protected.");
            ui.wrap(|ui| {
                for (label, kind, show) in [
                    ("Change password…", DialogKind::ChangePassword, true),
                    ("New recovery key…", DialogKind::NewRecoveryKey, true),
                    (
                        "Remove recovery key…",
                        DialogKind::RemoveRecoveryKey,
                        has_key,
                    ),
                    ("Save an unencrypted copy…", DialogKind::SaveCopy, true),
                    (
                        "Remove password protection…",
                        DialogKind::RemoveProtection,
                        true,
                    ),
                ] {
                    if show && ui.button(label).clicked() {
                        app.security.dialog = Some(Dialog::new(kind));
                    }
                }
            });
        } else {
            ui.label("This book is not password protected.");
            ui.row(|ui| {
                if ui.button("Protect with a password…").clicked() {
                    app.security.dialog = Some(Dialog::new(DialogKind::Protect));
                }
            });
        }
    });
    ui.card("Lock", |ui| {
        ui.form("Lock after", |ui| {
            let current = app.config.lock_after_minutes;
            let labels: Vec<String> = LOCK_CHOICES.iter().map(|(_, l)| (*l).to_owned()).collect();
            let mut idx = LOCK_CHOICES
                .iter()
                .position(|(m, _)| *m == current)
                .unwrap_or(0);
            if ui.combo("Lock after", &mut idx, &labels).changed()
                && let Some((m, _)) = LOCK_CHOICES.get(idx)
            {
                app.config.lock_after_minutes = *m;
                app.emit(AppEvent::ConfigChanged);
            }
            ui.secondary("without input (password-protected books)");
        });
    });
    super::ai::settings_card(app, ui);
}

fn secret_fields(ui: &mut Ui, d: &mut Dialog) {
    let label = if d.use_recovery {
        "Current recovery key"
    } else {
        "Current password"
    };
    ui.form(label, |ui| {
        ui.next().grow();
        if d.use_recovery {
            ui.field(label, &mut d.current);
        } else {
            ui.password(label, &mut d.current);
        }
    });
    let other = if d.use_recovery {
        "Use password instead"
    } else {
        "Use recovery key instead"
    };
    if ui.link(other).clicked() {
        d.use_recovery = !d.use_recovery;
        d.current.clear();
    }
}

fn new_password_fields(ui: &mut Ui, d: &mut Dialog) {
    ui.form("New password", |ui| {
        ui.next().grow();
        ui.password("New password", &mut d.new);
    });
    ui.form("Confirm new password", |ui| {
        ui.next().grow();
        ui.password("Confirm new password", &mut d.confirm);
    });
    ui.secondary(PASSPHRASE_NOTE);
}

fn settings_dialog(app: &mut App, ui: &mut Ui) {
    let Some(mut d) = app.security.dialog.take() else {
        return;
    };
    let (title, action) = match d.kind {
        DialogKind::Protect => ("Protect with a password", "Protect"),
        DialogKind::ChangePassword => ("Change password", "Save new password"),
        DialogKind::NewRecoveryKey => ("New recovery key", "Create new key"),
        DialogKind::RemoveRecoveryKey => ("Remove recovery key", "Remove key"),
        DialogKind::SaveCopy => ("Save an unencrypted copy", "Choose where to save…"),
        DialogKind::RemoveProtection => ("Remove password protection", "Remove protection"),
    };
    let mut go = false;
    let mut cancel = ui.dialog_closed("security");
    ui.dialog("security", title, 460, |ui| {
        ui.group(|ui| {
            match d.kind {
                DialogKind::Protect => {
                    new_password_fields(ui, &mut d);
                    ui.checkbox(&mut d.recovery, "Create a recovery key");
                    ui.secondary(LEFTOVER_NOTE);
                }
                DialogKind::ChangePassword => {
                    secret_fields(ui, &mut d);
                    new_password_fields(ui, &mut d);
                }
                DialogKind::NewRecoveryKey => {
                    ui.label("The old recovery key stops working.");
                    secret_fields(ui, &mut d);
                }
                DialogKind::RemoveRecoveryKey => {
                    ui.label(
                        "Without a recovery key, a forgotten password means the book cannot be opened.",
                    );
                    secret_fields(ui, &mut d);
                }
                DialogKind::SaveCopy => {
                    ui.label("The copy is not protected: anyone with the file can read it.");
                    secret_fields(ui, &mut d);
                }
                DialogKind::RemoveProtection => {
                    ui.label("The book file will no longer be protected by a password.");
                    secret_fields(ui, &mut d);
                }
            }
            if let Some(e) = &d.error {
                ui.error_text(e.clone());
            }
        });
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if ui.primary(action).clicked() {
                go = true;
            }
        });
    });
    if cancel {
        return;
    }
    if go {
        let needs_new = matches!(d.kind, DialogKind::Protect | DialogKind::ChangePassword);
        if needs_new && let Some(p) = new_password_problem(&d.new, &d.confirm) {
            d.error = Some(p);
            app.security.dialog = Some(d);
            return;
        }
        let op = match d.kind {
            DialogKind::Protect => SecurityOp::Encrypt {
                password: d.new.clone(),
                recovery: d.recovery,
            },
            DialogKind::ChangePassword => SecurityOp::ChangePassword {
                current: d.secret(),
                new: d.new.clone(),
            },
            DialogKind::NewRecoveryKey => SecurityOp::NewRecoveryKey {
                current: d.secret(),
            },
            DialogKind::RemoveRecoveryKey => SecurityOp::RemoveRecoveryKey {
                current: d.secret(),
            },
            DialogKind::SaveCopy => SecurityOp::Verify {
                current: d.secret(),
            },
            DialogKind::RemoveProtection => SecurityOp::RemoveEncryption {
                current: d.secret(),
            },
        };
        match app.backend.security(op) {
            Ok(key) => {
                app.generation += 1;
                app.refresh_history_hints();
                match d.kind {
                    DialogKind::Protect => {
                        app.notify("This book is now password protected.");
                        app.show_recovery_key(key);
                        // Then: earlier backups that aren't protected (security-review-1 B2).
                        super::backups::after_password_added(app);
                    }
                    DialogKind::ChangePassword => app.notify("Password changed."),
                    DialogKind::NewRecoveryKey => app.show_recovery_key(key),
                    DialogKind::RemoveRecoveryKey => app.notify("Recovery key removed."),
                    DialogKind::SaveCopy => {
                        app.security.copy_pending = true;
                        let name = app.book_name().unwrap_or_else(|| "book".into());
                        app.emit(AppEvent::ChooseUnencryptedCopyPath {
                            file_name: format!("{name} (unencrypted).moneykeep"),
                        });
                    }
                    DialogKind::RemoveProtection => app.notify("Password protection removed."),
                }
                return;
            }
            Err(e) => {
                d.error = Some(if e.code == "wrong_password" {
                    "That password didn't work.".into()
                } else {
                    e.display()
                });
                d.current.clear();
            }
        }
    }
    app.security.dialog = Some(d);
}

/// All security dialogs (start screen and main window).
pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    unlock_dialog(app, ui);
    recovery_dialog(app, ui);
    settings_dialog(app, ui);
}
