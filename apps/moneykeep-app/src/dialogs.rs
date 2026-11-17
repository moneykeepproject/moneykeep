//! Native file dialogs for the desktop app.
//!
//! macOS: a dialog must not run a modal loop of its own from inside a winit event
//! handler. `rfd::FileDialog` calls `NSSavePanel runModal`, which delivers events
//! while the click that opened it is still being handled; winit then panics
//! ("tried to handle event while another event is currently being handled") inside
//! an Objective-C callback, where the panic cannot unwind, and the app aborts. So on
//! macOS the dialog opens as a sheet on the window (`rfd::AsyncFileDialog`, which
//! returns at once) and its answer is handed back on a later turn of the event loop.
//!
//! Windows and Linux keep the blocking dialog: it answers in the same call, exactly
//! as before.

use std::path::PathBuf;

/// Whether dialogs answer later (macOS) or in the same call (elsewhere).
pub const ANSWER_LATER: bool = cfg!(target_os = "macos");

/// Which dialog to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Choose where to save one file.
    Save,
    /// Choose one existing file.
    File,
    /// Choose any number of existing files.
    Files,
    /// Choose a folder.
    Folder,
}

/// A file dialog to show: what to choose, the file types, a suggested name and folder.
#[derive(Clone, Debug)]
pub struct Ask {
    pub kind: Kind,
    filters: Vec<(String, Vec<String>)>,
    file_name: Option<String>,
    directory: Option<PathBuf>,
}

impl Ask {
    fn new(kind: Kind) -> Self {
        Self {
            kind,
            filters: Vec::new(),
            file_name: None,
            directory: None,
        }
    }

    /// Choose where to save one file.
    pub fn save() -> Self {
        Self::new(Kind::Save)
    }

    /// Choose one existing file.
    pub fn file() -> Self {
        Self::new(Kind::File)
    }

    /// Choose any number of existing files.
    pub fn files() -> Self {
        Self::new(Kind::Files)
    }

    /// Choose a folder.
    pub fn folder() -> Self {
        Self::new(Kind::Folder)
    }

    /// Offer files with these extensions under this name.
    pub fn filter(mut self, name: impl Into<String>, extensions: &[&str]) -> Self {
        self.filters.push((
            name.into(),
            extensions.iter().map(|e| (*e).to_owned()).collect(),
        ));
        self
    }

    /// The suggested file name.
    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }

    /// The folder the dialog starts in.
    pub fn directory(mut self, dir: impl Into<PathBuf>) -> Self {
        self.directory = Some(dir.into());
        self
    }

    /// Show the dialog and wait for the answer (Windows and Linux). Nothing chosen,
    /// or cancelled: an empty list.
    pub fn run_blocking(self) -> Vec<PathBuf> {
        let mut d = rfd::FileDialog::new();
        for (name, exts) in &self.filters {
            d = d.add_filter(name, exts);
        }
        if let Some(n) = &self.file_name {
            d = d.set_file_name(n);
        }
        if let Some(dir) = &self.directory {
            d = d.set_directory(dir);
        }
        match self.kind {
            Kind::Save => d.save_file().into_iter().collect(),
            Kind::File => d.pick_file().into_iter().collect(),
            Kind::Files => d.pick_files().unwrap_or_default(),
            Kind::Folder => d.pick_folder().into_iter().collect(),
        }
    }

    /// Show the dialog without a modal loop (macOS: a sheet on the window). The
    /// dialog opens when this is called; the future completes with the answer.
    /// Nothing chosen, or cancelled: an empty list.
    pub fn run_later(self) -> impl Future<Output = Vec<PathBuf>> + 'static {
        let mut d = rfd::AsyncFileDialog::new();
        for (name, exts) in &self.filters {
            d = d.add_filter(name, exts);
        }
        if let Some(n) = &self.file_name {
            d = d.set_file_name(n);
        }
        if let Some(dir) = &self.directory {
            d = d.set_directory(dir);
        }
        let kind = self.kind;
        async move {
            let paths = |handles: Vec<rfd::FileHandle>| -> Vec<PathBuf> {
                handles.iter().map(|h| h.path().to_path_buf()).collect()
            };
            match kind {
                Kind::Save => paths(d.save_file().await.into_iter().collect()),
                Kind::File => paths(d.pick_file().await.into_iter().collect()),
                Kind::Files => paths(d.pick_files().await.unwrap_or_default()),
                Kind::Folder => paths(d.pick_folder().await.into_iter().collect()),
            }
        }
    }
}

/// The first chosen path as text, if it is valid Unicode (book paths are stored as text).
pub fn first_text(picked: &[PathBuf]) -> Option<String> {
    picked.first().and_then(|p| p.to_str()).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_keeps_filters_name_and_folder() {
        let a = Ask::save()
            .filter("Book", &["moneykeep"])
            .file_name("Household.moneykeep")
            .directory("/tmp");
        assert_eq!(a.kind, Kind::Save);
        assert_eq!(
            a.filters,
            vec![("Book".to_owned(), vec!["moneykeep".to_owned()])]
        );
        assert_eq!(a.file_name.as_deref(), Some("Household.moneykeep"));
        assert_eq!(a.directory, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn only_macos_answers_later() {
        assert_eq!(ANSWER_LATER, cfg!(target_os = "macos"));
    }

    /// Every dialog goes through `Ask`: a blocking `rfd::FileDialog` called from the
    /// app's event handlers aborts MoneyKeep on macOS (see the module notes).
    #[test]
    fn the_app_shell_opens_no_blocking_dialog() {
        for (file, text) in [
            ("main.rs", include_str!("main.rs")),
            ("backend.rs", include_str!("backend.rs")),
            ("config.rs", include_str!("config.rs")),
            ("receipt_viewer.rs", include_str!("receipt_viewer.rs")),
            ("window.rs", include_str!("window.rs")),
        ] {
            assert!(
                !text.contains("rfd::"),
                "{file} uses rfd directly; use dialogs::Ask"
            );
        }
    }

    #[test]
    fn first_text_takes_the_first_path() {
        assert_eq!(first_text(&[]), None);
        assert_eq!(
            first_text(&[PathBuf::from("/a/b.moneykeep"), PathBuf::from("/c")]),
            Some("/a/b.moneykeep".to_owned())
        );
    }
}
