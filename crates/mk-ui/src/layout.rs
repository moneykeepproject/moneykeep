//! Pixel arithmetic for layout. The workspace denies float arithmetic so that money
//! is never a float; window geometry and timers are where floats are the right type.
//! They live here, and none of it touches amounts.
#![allow(clippy::float_arithmetic)]

// ----- window and panel geometry (revision 8) ------------------------------------

/// The main window's last size and position, in logical (desktop) pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WindowState {
    pub width: u32,
    pub height: u32,
    /// Outer top-left corner; `None` lets the system choose.
    #[serde(default)]
    pub x: Option<i32>,
    #[serde(default)]
    pub y: Option<i32>,
    #[serde(default)]
    pub maximized: bool,
}

/// Widths of the panels the user can resize, in points (`None` = default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct PanelWidths {
    #[serde(default)]
    pub sidebar: Option<u32>,
    #[serde(default)]
    pub reports: Option<u32>,
    #[serde(default)]
    pub settings: Option<u32>,
}

/// One connected screen, in logical pixels; the first is the primary screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Where and how big to open the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub width: u32,
    pub height: u32,
    pub position: Option<(i32, i32)>,
    pub maximized: bool,
    /// The smallest inner size the window may be resized to.
    pub min_width: u32,
    pub min_height: u32,
}

/// The minimum inner window size in logical pixels (revision 8a), whatever the
/// text size.
pub const MIN_WINDOW: (u32, u32) = (1000, 700);

fn overlap(a0: i64, a_len: i64, b0: i64, b_len: i64) -> i64 {
    (a0 + a_len).min(b0 + b_len) - a0.max(b0)
}

/// Where to open the window from its saved state. When the saved position would
/// leave the window mostly (more than half) off every screen, it opens centred on
/// the primary screen at the saved size, shrunk to fit. With no screen
/// information the saved state is used as it is.
///
/// The minimum size is 1000×700, or the primary screen's size if that is smaller;
/// a saved size below it is raised to it.
pub fn place_window(saved: &WindowState, screens: &[ScreenRect]) -> Placement {
    let (min_width, min_height) = screens.first().map_or(MIN_WINDOW, |s| {
        (
            MIN_WINDOW.0.min(s.width.max(1)),
            MIN_WINDOW.1.min(s.height.max(1)),
        )
    });
    let keep = Placement {
        width: saved.width.max(min_width),
        height: saved.height.max(min_height),
        position: saved.x.zip(saved.y),
        maximized: saved.maximized,
        min_width,
        min_height,
    };
    let Some(primary) = screens.first() else {
        return keep;
    };
    let (w, h) = (i64::from(keep.width), i64::from(keep.height));
    let visible = |(x, y): (i32, i32)| {
        screens
            .iter()
            .map(|s| {
                let ow = overlap(i64::from(x), w, i64::from(s.x), i64::from(s.width));
                let oh = overlap(i64::from(y), h, i64::from(s.y), i64::from(s.height));
                ow.max(0) * oh.max(0)
            })
            .max()
            .unwrap_or(0)
    };
    match keep.position {
        None => keep,
        Some(p) if visible(p) * 2 >= w * h => keep,
        Some(_) => {
            let width = keep.width.min(primary.width.max(1));
            let height = keep.height.min(primary.height.max(1));
            let cx = i64::from(primary.x) + (i64::from(primary.width) - i64::from(width)) / 2;
            let cy = i64::from(primary.y) + (i64::from(primary.height) - i64::from(height)) / 2;
            Placement {
                width,
                height,
                position: Some((
                    i32::try_from(cx).unwrap_or(0),
                    i32::try_from(cy).unwrap_or(0),
                )),
                ..keep
            }
        }
    }
}

/// A length in points as whole logical pixels at `zoom` (points × zoom).
pub fn to_pixels(points: f32, zoom: f32) -> i32 {
    let v = (points * zoom).round();
    if v.is_finite() {
        #[allow(clippy::cast_possible_truncation)]
        let px = v.clamp(-1.0e6, 1.0e6) as i32;
        px
    } else {
        0
    }
}

/// Whole seconds as an time span (seconds).
pub(crate) fn secs_f64(secs: i64) -> f64 {
    f64::from(i32::try_from(secs).unwrap_or(i32::MAX))
}

/// `now` plus `secs` seconds (seconds since the app started).
pub(crate) fn add_secs(now: f64, secs: i64) -> f64 {
    now + secs_f64(secs)
}

/// Whole seconds left until `until` (0 once passed; at most a day).
pub(crate) fn remaining_secs(now: f64, until: f64) -> u64 {
    let left = (until - now).ceil();
    if left.is_finite() && left > 0.0 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = left.min(86_400.0) as u64;
        n
    } else {
        0
    }
}

/// Whether at least `secs` seconds passed between `then` and `now` (seconds since the app started).
pub(crate) fn elapsed(now: f64, then: f64, secs: f64) -> bool {
    now - then >= secs
}
