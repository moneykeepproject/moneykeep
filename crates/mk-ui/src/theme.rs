//! The per-user look settings (theme, density, scroll bar width) and the text size
//! floor. The colours themselves live in mk-views' `theme.slint`.

use serde::{Deserialize, Serialize};

/// Theme preference stored in the per-user config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// Row density of the register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

impl Density {
    /// Register row height in points.
    pub fn row_height(self) -> f32 {
        match self {
            Density::Comfortable => 28.0,
            Density::Compact => 22.0,
        }
    }
}

/// Smallest text size anywhere in the app, in logical points.
pub const MIN_TEXT: f32 = 12.0;

/// Scroll bar width in points (revision 27): default, smallest and largest.
pub const SCROLL_WIDTH_DEFAULT: u32 = 14;
pub const SCROLL_WIDTH_MIN: u32 = 6;
pub const SCROLL_WIDTH_MAX: u32 = 30;

/// A configured scroll bar width kept within 6–30.
pub fn clamp_scroll_width(width: u32) -> u32 {
    width.clamp(SCROLL_WIDTH_MIN, SCROLL_WIDTH_MAX)
}
