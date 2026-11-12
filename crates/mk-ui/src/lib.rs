//! MoneyKeep's UI controller (specs: app-desktop.md, ui-slint.md revision 2): the
//! application state, the view models the `.slint` views in mk-views show, and the
//! registry command calls. No toolkit here: it builds plain Rust view models and
//! answers user actions, and its logic is tested natively.
//!
//! The UI never touches storage and never computes business results. Every change to
//! a book goes through a [`Backend`], which runs registry commands (actor `app`); the
//! views show what those commands return. The app crate supplies a backend that runs
//! commands on a worker thread and owns files, dialogs and config; tests use an
//! in-process backend.

mod app_icon;
pub mod base;
mod calc;
pub mod ctl;
pub mod fmt;
mod layout;
pub mod theme;

pub use app_icon::{
    APP_ICON_SIZE, ICNS_ENTRIES, ICO_SIZES, app_icon_icns, app_icon_ico, app_icon_on, app_icon_rgba,
};
pub use base::{
    AccountsPaneConfig, AppEvent, BackupMemo, BackupPrefs, CalculatorConfig, DateOrder,
    FileRequest, MAX_RECEIPT_BYTES, MESSAGE_SECONDS, MessagePrefs, PickedFile, ReceiptTarget,
    RecentBook, Screen, TOO_BIG, TileCache, TileIcon, UserConfig, View,
};
pub use ctl::App;
pub use layout::{PanelWidths, Placement, ScreenRect, WindowState, place_window, to_pixels};
pub use theme::{Density, Theme};

/// An installed font family offered in Settings → Display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontInfo {
    pub family: String,
    pub monospace: bool,
}

/// The bytes of one font face (a TrueType/OpenType file or collection) and which
/// face in it to use.
#[derive(Clone)]
pub struct FontBytes {
    pub data: Vec<u8>,
    pub index: u32,
}

impl std::fmt::Debug for FontBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "FontBytes({} bytes, face {})",
            self.data.len(),
            self.index
        )
    }
}

/// Finds installed fonts and reads their files. The app crate supplies one (it
/// owns file access); tests inject a list. The UI never reads font files itself.
pub trait FontProvider {
    /// Installed families with a regular, upright face, alphabetically.
    fn families(&self) -> Vec<FontInfo>;
    /// The regular face of `family`, or `None` when it is missing or unreadable.
    fn load(&self, family: &str) -> Option<FontBytes>;
}

/// A password or recovery key typed by the user. Its text is never shown in debug
/// output.
#[derive(Clone, PartialEq, Eq)]
pub enum UiSecret {
    Password(String),
    RecoveryKey(String),
}

impl std::fmt::Debug for UiSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            UiSecret::Password(_) => "UiSecret::Password(…)",
            UiSecret::RecoveryKey(_) => "UiSecret::RecoveryKey(…)",
        })
    }
}

/// Key operations on the open book (book-encryption.md session operations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityOp {
    /// Protect the open plain book; returns the recovery key when asked for.
    Encrypt {
        password: String,
        recovery: bool,
    },
    ChangePassword {
        current: UiSecret,
        new: String,
    },
    /// Returns the new recovery key.
    NewRecoveryKey {
        current: UiSecret,
    },
    RemoveRecoveryKey {
        current: UiSecret,
    },
    /// Check the secret only (before asking where to save an unencrypted copy).
    Verify {
        current: UiSecret,
    },
    /// Write an unencrypted copy to `path` (never replacing a file); the answer is
    /// where the copy went (the next free name if `path` was taken meanwhile).
    SaveUnencryptedCopy {
        path: String,
    },
    RemoveEncryption {
        current: UiSecret,
    },
}

fn no_backups() -> UiError {
    UiError::new("unsupported", "Backups are not available here.")
}

fn unsupported() -> UiError {
    UiError::new("unsupported", "Password protection is not available here.")
}

use serde_json::Value;

/// A command error as the UI shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct UiError {
    pub code: String,
    pub message: String,
}

impl UiError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }

    /// Plain words for the user.
    pub fn display(&self) -> String {
        if self.code == "busy" {
            format!("The book is busy: {}", self.message)
        } else if self.code == "book_in_use" {
            "This book is open in another MoneyKeep window.".to_owned()
        } else {
            self.message.clone()
        }
    }
}

/// Runs registry commands against the open book.
pub trait Backend {
    /// Run one command and wait for its result.
    fn invoke(&mut self, command: &str, args: Value) -> Result<Value, UiError>;
    /// The open book's display name, or `None` when no book is open.
    fn book_name(&self) -> Option<String>;
    /// Create a book at `path` (from the starter chart `preset`, if any) and open it.
    fn create_book(
        &mut self,
        path: &str,
        name: &str,
        currency: &str,
        preset: Option<&str>,
    ) -> Result<(), UiError>;
    /// Open the book at `path`.
    fn open_book(&mut self, path: &str) -> Result<(), UiError>;
    /// Close the open book.
    fn close_book(&mut self);
    /// Create a password-protected book at `path` and open it; returns the recovery
    /// key when one was asked for.
    fn create_protected_book(
        &mut self,
        _path: &str,
        _name: &str,
        _currency: &str,
        _password: &str,
        _recovery: bool,
        _preset: Option<&str>,
    ) -> Result<Option<String>, UiError> {
        Err(unsupported())
    }
    /// The built-in starter charts (`chart.presets`), readable before a book is open
    /// (the Create a new book dialog, chart-presets.md).
    fn chart_presets(&mut self) -> Result<Value, UiError> {
        self.invoke("chart.presets", Value::Null)
    }
    /// Open a password-protected book with its password or recovery key.
    fn open_book_with(&mut self, _path: &str, _secret: &UiSecret) -> Result<(), UiError> {
        Err(unsupported())
    }
    /// Whether the file at `path` is a password-protected book (from its first
    /// bytes); `None` when it cannot be read.
    fn is_protected_file(&self, _path: &str) -> Option<bool> {
        Some(false)
    }
    /// Whether the open book is password protected, and whether it has a recovery key.
    fn protection(&self) -> (bool, bool) {
        (false, false)
    }
    /// Run a key operation on the open book. The answer is the new recovery key,
    /// or for [`SecurityOp::SaveUnencryptedCopy`] the copy's path.
    fn security(&mut self, _op: SecurityOp) -> Result<Option<String>, UiError> {
        Err(unsupported())
    }
    /// Replace a damaged book with `<path>.prev` (keeping the damaged file as
    /// `<path>.damaged`) and open it.
    fn open_previous(&mut self, _path: &str, _secret: &UiSecret) -> Result<(), UiError> {
        Err(unsupported())
    }
    /// Start a network command (bank-feed sync) in the background; returns a job id.
    /// Backends without background jobs return `unsupported` and the app runs the
    /// command with [`Backend::invoke`] instead.
    fn net_start(&mut self, _command: &str, _args: Value) -> Result<u64, UiError> {
        Err(unsupported())
    }
    /// Background jobs that ended since the last call, with their results.
    fn net_poll(&mut self) -> Vec<(u64, Result<Value, UiError>)> {
        Vec::new()
    }
    /// Stop waiting for a background job; it records nothing.
    fn net_cancel(&mut self, _id: u64) {}
    /// Keep (`Some`) or delete (`None`) the cached 128 × 128 PNG of a recent book's
    /// icon picture (the start screen's tile cache, app-desktop.md revision 26). The
    /// app keeps one file per book in its config folder; failures are ignored.
    fn save_tile_image(&mut self, _book_path: &str, _png: Option<&[u8]>) {}
    /// The cached icon picture of a recent book, if any.
    fn load_tile_image(&self, _book_path: &str) -> Option<Vec<u8>> {
        None
    }
    /// The open book's AI access settings (mcp-server.md revision 3):
    /// `{"enabled","level","groups","app_only"}`; `None` without a book or support.
    fn ai_access(&self) -> Option<Value> {
        None
    }
    // ----- backups (backups.md) -----------------------------------------------------

    /// The backup settings (the JSON of [`BackupPrefs`]), before a book opens and
    /// whenever they change.
    fn set_backup_settings(&mut self, _settings: Value) {}
    /// `<user folder>\MoneyKeep Backups` on this computer (the default backup
    /// folder, backups.md revision 3).
    fn default_backup_folder(&self) -> Option<String> {
        None
    }
    /// Write and delete a small test file in a folder chosen as the backup folder or
    /// the second place (backups.md revision 3). Errors: `backup_blocked` (Windows
    /// blocked it; the message explains) or `backup_folder`. Backends that cannot
    /// check accept every folder.
    fn test_backup_folder(&mut self, _folder: &str) -> Result<(), UiError> {
        Ok(())
    }
    /// Start a backup of the open book in the background (`book.backup`); typing
    /// goes on while it runs. Backends that cannot back up return `unsupported`.
    fn backup_start(&mut self, _reason: &str) -> Result<(), UiError> {
        Err(no_backups())
    }
    /// Backups that finished since the last call (the `book.backup` result or its
    /// error).
    fn backup_poll(&mut self) -> Vec<Result<Value, UiError>> {
        Vec::new()
    }
    /// Whether the open book changed since its last backup (or since it opened).
    fn backup_changed(&mut self) -> bool {
        false
    }
    /// The backup made before updating the book's format when it last opened.
    fn take_open_backup(&mut self) -> Option<Value> {
        None
    }
    /// The backups of the book file `book`, newest first (`book.backups`).
    fn backups_list(&mut self, _book: &str) -> Result<Value, UiError> {
        Err(no_backups())
    }
    /// Copy the backup to `target` and check it (`book.restore_copy`); `secret` for
    /// a password-protected backup. Returns `{"path","encrypted"}`.
    fn restore_copy(
        &mut self,
        _backup: &str,
        _target: &str,
        _secret: Option<&UiSecret>,
    ) -> Result<Value, UiError> {
        Err(no_backups())
    }
    /// Back up the book file `book`, which is not open, without opening it (the
    /// start-screen card's … → Back up this book…: a `manual` backup, backups.md
    /// revision 2); `secret` for a password-protected book. Returns the
    /// `book.backup` result.
    fn backup_book_file(
        &mut self,
        _book: &str,
        _secret: Option<&UiSecret>,
    ) -> Result<Value, UiError> {
        Err(no_backups())
    }
    /// Wait for backups still running (the app is quitting).
    fn finish_backups(&mut self) {}
    /// Change the AI access settings (the session operation `set_ai_access`);
    /// returns the settings now stored.
    fn set_ai_access(&mut self, _settings: Value) -> Result<Value, UiError> {
        Err(UiError::new(
            "unsupported",
            "AI assistant access is not available here.",
        ))
    }
}
