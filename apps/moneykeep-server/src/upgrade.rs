//! Upgrade resilience (upgrade-resilience-1 U1.4, owner decision D2): the data
//! folder's format number and the server's start-up refusals.
//!
//! The data folder carries `format.json` (`{"format": 1}`), written by setup, move
//! and reinstall. A server or helper whose own [`DATA_FORMAT`] is lower refuses the
//! folder; downgrades are not supported. Because the health endpoint isn't there
//! then, and on Windows the user can't open the data folder, the service stops with
//! a service-specific exit code for each refusal ([`Refusal::code`]), which
//! `services status` reads without a permission prompt.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The data folder's format file.
pub const FORMAT_FILE: &str = "format.json";
/// The data folder format this program writes and understands.
pub const DATA_FORMAT: u32 = 1;

/// Why the server refused to start (each has its own exit code).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// `format.json` names a newer format.
    NewerFormat,
    /// `inbox.sqlite`'s `meta.schema` is newer.
    NewerDatabase,
    /// `server.toml` can't be read by this version (an unknown key, say).
    Settings,
    /// No inbox key opens (U1.2).
    Key,
}

impl Refusal {
    pub const ALL: [Refusal; 4] = [
        Refusal::NewerFormat,
        Refusal::NewerDatabase,
        Refusal::Settings,
        Refusal::Key,
    ];

    /// The process's exit code, and on Windows the service-specific exit code
    /// (`ERROR_SERVICE_SPECIFIC_ERROR` with this number).
    pub fn code(self) -> u8 {
        match self {
            Refusal::NewerFormat => 81,
            Refusal::NewerDatabase => 82,
            Refusal::Settings => 83,
            Refusal::Key => 84,
        }
    }

    pub fn from_code(code: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|r| u32::from(r.code()) == code)
    }

    /// What the desktop shows.
    pub fn message(self) -> &'static str {
        match self {
            Refusal::NewerFormat | Refusal::NewerDatabase => crate::store::NEWER_DATA,
            Refusal::Settings => SETTINGS_REFUSED,
            Refusal::Key => crate::store::KEY_UNOPENABLE,
        }
    }

    /// The refusal a start-up error stands for, if it is one.
    pub fn of_error(e: &str) -> Option<Self> {
        if e.starts_with(crate::store::NEWER_DATA) {
            Some(Refusal::NewerDatabase)
        } else if e.starts_with(crate::store::KEY_UNOPENABLE) {
            Some(Refusal::Key)
        } else if e.contains(&format!("{} is not valid", crate::config::CONFIG_FILE))
            || e.starts_with(SETTINGS_REFUSED)
        {
            Some(Refusal::Settings)
        } else {
            None
        }
    }
}

/// The refusal of a `server.toml` this version can't read.
pub const SETTINGS_REFUSED: &str =
    "The capture server's settings (server.toml) can't be read by this version of MoneyKeep.";

#[derive(Debug, Deserialize)]
struct FormatDoc {
    #[serde(default)]
    format: Option<u32>,
}

/// The format `dir` was written with: `None` when it has no (readable) `format.json`
/// (data from before format numbers). Unknown fields are ignored.
pub fn read_format(dir: &Path) -> Option<u32> {
    let bytes = std::fs::read(dir.join(FORMAT_FILE)).ok()?;
    serde_json::from_slice::<FormatDoc>(&bytes).ok()?.format
}

/// Refuse a folder of a newer format.
pub fn check_format(dir: &Path) -> Result<(), String> {
    match read_format(dir) {
        Some(f) if f > DATA_FORMAT => Err(crate::store::NEWER_DATA.into()),
        _ => Ok(()),
    }
}

/// Write `format.json` (atomically) unless a newer format is there.
pub fn write_format(dir: &Path) -> Result<(), String> {
    check_format(dir)?;
    if read_format(dir) == Some(DATA_FORMAT) {
        return Ok(());
    }
    let text = serde_json::json!({ "format": DATA_FORMAT }).to_string();
    crate::store::write_private(&dir.join(FORMAT_FILE), text.as_bytes())
}

/// Everything that refuses `dir` before its key is touched: a newer format, a newer
/// inbox, a `server.toml` this version can't read. Used at start and by `services
/// status` where the folder can be read.
pub fn check_dir(dir: &Path) -> Result<(), (Refusal, String)> {
    if check_format(dir).is_err() {
        return Err((Refusal::NewerFormat, crate::store::NEWER_DATA.into()));
    }
    if crate::store::schema_of(dir)
        .ok()
        .flatten()
        .is_some_and(|v| v > crate::store::SCHEMA_VERSION)
    {
        return Err((Refusal::NewerDatabase, crate::store::NEWER_DATA.into()));
    }
    if let Ok(text) = std::fs::read_to_string(dir.join(crate::config::CONFIG_FILE))
        && let Err(e) = crate::Config::parse(&text)
    {
        return Err((Refusal::Settings, format!("{SETTINGS_REFUSED} {e}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn codes_round_trip_and_errors_are_classified() {
        for r in Refusal::ALL {
            assert_eq!(Refusal::from_code(u32::from(r.code())), Some(r));
        }
        assert_eq!(Refusal::from_code(1), None);
        assert_eq!(
            Refusal::of_error(crate::store::KEY_UNOPENABLE),
            Some(Refusal::Key)
        );
        assert_eq!(
            Refusal::of_error("server.toml is not valid: unknown field `x`"),
            Some(Refusal::Settings)
        );
        assert_eq!(Refusal::of_error("cannot listen"), None);
    }

    #[test]
    fn format_file() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read_format(d.path()), None);
        assert!(check_format(d.path()).is_ok());
        write_format(d.path()).unwrap();
        assert_eq!(read_format(d.path()), Some(DATA_FORMAT));
        std::fs::write(
            d.path().join(FORMAT_FILE),
            r#"{"format": 2, "later": true}"#,
        )
        .unwrap();
        assert_eq!(
            check_format(d.path()).unwrap_err(),
            crate::store::NEWER_DATA
        );
        assert!(
            write_format(d.path()).is_err(),
            "a newer format is never lowered"
        );
        assert_eq!(check_dir(d.path()).unwrap_err().0, Refusal::NewerFormat);
    }
}
