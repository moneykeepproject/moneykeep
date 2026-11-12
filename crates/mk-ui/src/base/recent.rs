//! The recent-books list with its tile cache (app-desktop.md revisions 26 and 40):
//! what each start-screen card shows without opening the book. Configs written before
//! revision 26 hold plain path strings; they still load.

use serde::{Deserialize, Serialize};

/// A book's icon as the tile shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TileIcon {
    /// One or two characters, shown upper-cased.
    Letter { text: String },
    /// One of the profile symbols (`house`, `church`, …).
    Symbol { symbol: String },
    /// The stored picture; its PNG (256 × 256 since revision 40) is kept by the
    /// app as a file.
    Image,
    /// One of the 24 preset pictures (revision 40), drawn from the app.
    Preset { preset: String },
}

/// The cached profile of a recent book.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileCache {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub icon: TileIcon,
    pub color: String,
    /// The tagline (revision 40).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tagline: String,
    /// The base currency, for the card's chip (revision 40).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub currency: String,
}

/// One recent book: its path and, once it has been opened, its tile cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RecentEntry")]
pub struct RecentBook {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile: Option<TileCache>,
    /// The day the book was last opened (YYYY-MM-DD, revision 40).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_opened: Option<String>,
}

impl From<String> for RecentBook {
    fn from(path: String) -> Self {
        Self {
            path,
            tile: None,
            last_opened: None,
        }
    }
}

impl From<&str> for RecentBook {
    fn from(path: &str) -> Self {
        path.to_owned().into()
    }
}

/// A recent-list entry as stored: a bare path (before revision 26) or a path with
/// its tile cache.
#[derive(Deserialize)]
#[serde(untagged)]
enum RecentEntry {
    Path(String),
    Book {
        path: String,
        #[serde(default)]
        tile: Option<TileCache>,
        #[serde(default)]
        last_opened: Option<String>,
    },
}

impl From<RecentEntry> for RecentBook {
    fn from(e: RecentEntry) -> Self {
        match e {
            RecentEntry::Path(path) => path.into(),
            RecentEntry::Book {
                path,
                tile,
                last_opened,
            } => Self {
                path,
                tile,
                last_opened,
            },
        }
    }
}

/// The tile cache from a `book.info` result.
pub(crate) fn tile_from_info(info: &serde_json::Value) -> Option<TileCache> {
    let name = info.get("name")?.as_str()?.to_owned();
    let description = info
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let icon = info.get("icon")?;
    let text = |k: &str| icon.get(k).and_then(serde_json::Value::as_str);
    let icon = match text("kind")? {
        "symbol" => TileIcon::Symbol {
            symbol: text("symbol")?.to_owned(),
        },
        "preset" => TileIcon::Preset {
            preset: text("preset")?.to_owned(),
        },
        "image" => TileIcon::Image,
        _ => TileIcon::Letter {
            text: text("text")?.to_owned(),
        },
    };
    let color = info.get("color")?.as_str()?.to_owned();
    let field = |k: &str| {
        info.get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    Some(TileCache {
        name,
        description,
        icon,
        color,
        tagline: field("tagline"),
        currency: field("base_currency"),
    })
}

/// What a tile shows for a book with no cache yet: the file name, the default
/// letter and the default colour (the profile defaults, from the file name).
pub(crate) fn fallback_tile(path: &str) -> TileCache {
    let (name, _) = book_label(path);
    TileCache {
        icon: TileIcon::Letter {
            text: name
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_else(|| "?".to_owned()),
        },
        color: default_color(&name).to_owned(),
        description: String::new(),
        tagline: String::new(),
        currency: String::new(),
        name,
    }
}

/// The preset pictures (revision 40), in the order Settings shows them (the same
/// list as the `book.update_profile` command).
pub const PRESETS: [&str; 24] = [
    "farmer",
    "pastor",
    "teacher",
    "nurse",
    "doctor",
    "carpenter",
    "builder",
    "mechanic",
    "shopkeeper",
    "baker",
    "cook",
    "bookkeeper",
    "lawyer",
    "artist",
    "musician",
    "gardener",
    "fisherman",
    "seamstress",
    "scholar",
    "student",
    "family",
    "couple",
    "grandparents",
    "piggy-bank",
];

/// "Nov 7, 2026" from "2026-11-07" (the card's Last opened line).
pub(crate) fn short_date(iso: &str) -> Option<String> {
    let mut parts = iso.get(..10)?.split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .get(m.parse::<usize>().ok()?.checked_sub(1)?)?;
    Some(format!("{month} {}, {y}", d.parse::<u32>().ok()?))
}

/// The tile colours, in the order the default colour indexes them (the same list
/// as the `book.update_profile` command).
pub(crate) const COLORS: [&str; 10] = [
    "teal", "blue", "indigo", "purple", "rose", "red", "orange", "amber", "green", "slate",
];

/// The profile symbols, in the order Settings shows them.
pub(crate) const SYMBOLS: [&str; 16] = [
    "house",
    "church",
    "briefcase",
    "shop",
    "building",
    "piggy-bank",
    "wallet",
    "chart",
    "heart",
    "star",
    "leaf",
    "car",
    "tools",
    "graduation-cap",
    "people",
    "paw",
];

/// The default colour of a name: index (sum of character codes) mod 10.
pub(crate) fn default_color(name: &str) -> &'static str {
    let sum = name
        .chars()
        .fold(0u64, |acc, c| acc.wrapping_add(u64::from(u32::from(c))));
    COLORS
        .get(usize::try_from(sum % 10).unwrap_or(0))
        .copied()
        .unwrap_or("teal")
}

/// Book name and folder from a book path: `/a/b/Household.moneykeep` →
/// (`Household`, `/a/b`).
pub(crate) fn book_label(path: &str) -> (String, String) {
    let p = std::path::Path::new(path);
    let name = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_owned();
    let folder = p.parent().and_then(|d| d.to_str()).unwrap_or("").to_owned();
    (name, folder)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn old_configs_with_plain_paths_still_load() {
        let v: Vec<RecentBook> = serde_json::from_str(
            r#"["/a/old.moneykeep", {"path": "/b/new.moneykeep",
               "tile": {"name": "New", "icon": {"kind": "symbol", "symbol": "paw"}, "color": "red"}}]"#,
        )
        .unwrap();
        assert_eq!(v[0], RecentBook::from("/a/old.moneykeep"));
        assert_eq!(v[1].tile.as_ref().unwrap().name, "New");
        let back: Vec<RecentBook> =
            serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(back, v);
        assert_eq!(short_date("2026-11-07").as_deref(), Some("Nov 7, 2026"));
        assert_eq!(short_date("2026-13-07"), None);
    }
}
