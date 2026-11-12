//! Per-user app config: JSON in the platform config directory. Never inside a book,
//! never financial data.

use std::path::{Path, PathBuf};

use mk_core::brand;
use mk_ui::UserConfig;

const CONFIG_FILE: &str = "config.json";

fn dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("org", brand::FOLDER, brand::FOLDER)
        .map(|d| d.config_dir().to_path_buf())
}

fn path() -> Option<PathBuf> {
    dir().map(|d| d.join(CONFIG_FILE))
}

/// The folder holding the settings file (shown by Reset MoneyKeep settings).
pub fn folder() -> Option<PathBuf> {
    dir()
}

/// Reset MoneyKeep settings (app-desktop.md revision 41), in the settings folder
/// `dir`: first the current settings file is saved as `copy_name` beside it (the
/// file as it is on disk, else `current`; a name already taken gets ` (2)`, ` (3)`
/// …), then the default settings are written (keeping only what the app remembers
/// about backups, `memo`) and the card pictures of the recent books are deleted.
/// Books and backup files are never touched. Returns where the copy is.
pub fn reset_in(dir: &Path, copy_name: &str, current: &UserConfig) -> Result<PathBuf, String> {
    let ok_name = copy_name.starts_with("settings-before-reset-")
        && copy_name.ends_with(".json")
        && !copy_name.contains(['/', '\\', ':'])
        && !copy_name.contains("..");
    if !ok_name {
        return Err(format!("{copy_name} is not a settings copy name"));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let bytes = match std::fs::read(dir.join(CONFIG_FILE)) {
        Ok(b) => b,
        Err(_) => serde_json::to_vec_pretty(current).map_err(|e| e.to_string())?,
    };
    let stem = copy_name.trim_end_matches(".json");
    let mut target = dir.join(copy_name);
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("{stem} ({n}).json"));
        n += 1;
    }
    std::fs::write(&target, &bytes)
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    if std::fs::read(&target).ok().as_deref() != Some(bytes.as_slice()) {
        return Err(format!("{} could not be read back", target.display()));
    }
    let fresh = UserConfig {
        backup_memo: current.backup_memo.clone(),
        ..UserConfig::default()
    };
    let text = serde_json::to_string_pretty(&fresh).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(CONFIG_FILE), text).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(dir.join("book-icons"));
    Ok(target)
}

/// The folder of cached book icon pictures (app-desktop.md revision 26).
pub fn icons_dir() -> Option<PathBuf> {
    dir().map(|d| d.join("book-icons"))
}

/// The cache file of a recent book's icon picture inside `icons`: named by a hash
/// of the book's path (64-bit FNV-1a, stable across versions and platforms).
pub fn tile_image_file(icons: &std::path::Path, book_path: &str) -> PathBuf {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in book_path.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    icons.join(format!("{h:016x}.png"))
}

/// Write (`Some`) or delete (`None`) a cached icon picture; failures are ignored.
pub fn save_tile_image(icons: &std::path::Path, book_path: &str, png: Option<&[u8]>) {
    let file = tile_image_file(icons, book_path);
    match png {
        Some(bytes) => {
            let _ = std::fs::create_dir_all(icons);
            let _ = std::fs::write(file, bytes);
        }
        None => {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// A cached icon picture (at most 1 MB is read; the pictures are 128 × 128).
pub fn load_tile_image(icons: &std::path::Path, book_path: &str) -> Option<Vec<u8>> {
    let file = tile_image_file(icons, book_path);
    let len = std::fs::metadata(&file).ok()?.len();
    if len > 1 << 20 {
        return None;
    }
    std::fs::read(file).ok()
}

/// Load the config (defaults when missing or unreadable).
pub fn load() -> UserConfig {
    let mut c: UserConfig = dir().map(|d| load_from(&d)).unwrap_or_default();
    // backups.md revision 3: a setting still holding the old default
    // (Documents\MoneyKeep Backups) keeps it only when backups were made there.
    let folder = mk_commands::backups::resolve_folder_setting(
        c.backup.folder.as_deref(),
        mk_commands::backups::old_default_folder().as_deref(),
    );
    if folder != c.backup.folder {
        c.backup.folder = folder;
        save(&c);
    }
    c
}

/// The config in the folder `dir` (defaults when missing or unreadable).
pub fn load_from(dir: &Path) -> UserConfig {
    std::fs::read_to_string(dir.join(CONFIG_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Save the config; failures are ignored (the app keeps working).
pub fn save(config: &UserConfig) {
    let Some(p) = path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(config) {
        let _ = std::fs::write(p, text);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// app-desktop.md revision 41 test 230 (the files): the saved copy holds the
    /// settings file as it was; the recent list, display and backup settings are
    /// back to the defaults and the card pictures gone; books and backup files are
    /// byte-for-byte unchanged.
    #[test]
    fn t230_reset_saves_a_copy_and_keeps_books_and_backups() {
        use mk_ui::{RecentBook, Theme};
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("config");
        let books = root.path().join("Books");
        let backups = root.path().join("MoneyKeep Backups");
        std::fs::create_dir_all(&books).unwrap();
        std::fs::create_dir_all(&backups).unwrap();
        let book = books.join("Household.moneykeep");
        let backup = backups.join("Household 2026-11-09 0900.moneykeep");
        std::fs::write(&book, b"SQLite format 3\0 book").unwrap();
        std::fs::write(&backup, b"SQLite format 3\0 backup").unwrap();
        let mut c = UserConfig {
            theme: Theme::Dark,
            text_size: 150,
            start_sort: "name".into(),
            recent: vec![RecentBook {
                path: book.display().to_string(),
                tile: None,
                last_opened: Some("2026-11-09".into()),
            }],
            ..UserConfig::default()
        };
        c.backup.folder = Some(backups.display().to_string());
        c.backup.keep_recent = 3;
        c.backup_memo.made = 7;
        std::fs::create_dir_all(&dir).unwrap();
        let before = serde_json::to_string_pretty(&c).unwrap();
        std::fs::write(dir.join(CONFIG_FILE), &before).unwrap();
        save_tile_image(
            &dir.join("book-icons"),
            &book.display().to_string(),
            Some(b"png"),
        );
        let copy = reset_in(&dir, "settings-before-reset-2026-11-09.json", &c).unwrap();
        assert_eq!(copy, dir.join("settings-before-reset-2026-11-09.json"));
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), before);
        let after = load_from(&dir);
        assert!(after.recent.is_empty());
        assert_eq!(after.theme, Theme::default());
        assert_eq!(after.text_size, UserConfig::default().text_size);
        assert_eq!(after.start_sort, "");
        assert_eq!(after.backup, UserConfig::default().backup);
        assert_eq!(after.backup_memo.made, 7, "what the app remembers stays");
        assert!(!dir.join("book-icons").exists());
        assert_eq!(std::fs::read(&book).unwrap(), b"SQLite format 3\0 book");
        assert_eq!(std::fs::read(&backup).unwrap(), b"SQLite format 3\0 backup");
        // A second reset the same day keeps the first copy.
        let copy2 = reset_in(&dir, "settings-before-reset-2026-11-09.json", &after).unwrap();
        assert_eq!(copy2, dir.join("settings-before-reset-2026-11-09 (2).json"));
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), before);
        // Only a settings copy name is accepted.
        assert!(reset_in(&dir, "../config.json", &after).is_err());
    }

    #[test]
    fn tile_images_are_kept_per_book_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let icons = dir.path().join("book-icons");
        assert_eq!(load_tile_image(&icons, "/a/b.moneykeep"), None);
        save_tile_image(&icons, "/a/b.moneykeep", Some(b"png bytes"));
        save_tile_image(&icons, "/a/c.moneykeep", Some(b"other"));
        assert_eq!(
            load_tile_image(&icons, "/a/b.moneykeep").as_deref(),
            Some(&b"png bytes"[..])
        );
        assert_ne!(
            tile_image_file(&icons, "/a/b.moneykeep"),
            tile_image_file(&icons, "/a/c.moneykeep")
        );
        save_tile_image(&icons, "/a/b.moneykeep", None);
        assert_eq!(load_tile_image(&icons, "/a/b.moneykeep"), None);
        assert!(load_tile_image(&icons, "/a/c.moneykeep").is_some());
        // Stable name: FNV-1a of the path.
        let name = tile_image_file(&icons, "");
        assert!(name.ends_with("cbf29ce484222325.png"), "{name:?}");
    }
}
