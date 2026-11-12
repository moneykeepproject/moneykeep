//! The main window's size and position (app-desktop revision 8): restored at
//! start-up with the off-screen fallback, saved when it changes (at most once a
//! second) and on exit. Per user, in the app config; never in a book.

use std::time::{Duration, Instant};

use mk_ui::{ScreenRect, WindowState};
use slint::winit_030::{WinitWindowAccessor, winit};

/// The window size used when nothing was saved.
pub const DEFAULT: WindowState = WindowState {
    width: 1200,
    height: 800,
    x: None,
    y: None,
    maximized: false,
};

/// The connected screens in logical pixels, primary first (empty until the window
/// exists).
pub fn screens(window: &slint::Window) -> Vec<ScreenRect> {
    window
        .with_winit_window(|w| {
            let mut monitors: Vec<winit::monitor::MonitorHandle> = w.available_monitors().collect();
            if let Some(p) = w.primary_monitor()
                && let Some(i) = monitors.iter().position(|m| *m == p)
            {
                let m = monitors.remove(i);
                monitors.insert(0, m);
            }
            monitors
                .iter()
                .map(|m| {
                    let scale = m.scale_factor();
                    let pos = m.position().to_logical::<i32>(scale);
                    let size = m.size().to_logical::<u32>(scale);
                    ScreenRect {
                        x: pos.x,
                        y: pos.y,
                        width: size.width,
                        height: size.height,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Where and how large the window opens, from the saved state (logical pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub width: f32,
    pub height: f32,
    pub min_width: f32,
    pub min_height: f32,
    pub position: Option<(f32, f32)>,
    pub maximized: bool,
}

/// The geometry for the saved state on these screens (off-screen positions fall
/// back to the primary screen; never below the minimum size).
pub fn restore(saved: &WindowState, screens: &[ScreenRect]) -> Geometry {
    let p = mk_ui::place_window(saved, screens);
    #[allow(clippy::cast_precision_loss)]
    Geometry {
        width: p.width as f32,
        height: p.height as f32,
        min_width: p.min_width as f32,
        min_height: p.min_height as f32,
        position: p.position.map(|(x, y)| (x as f32, y as f32)),
        maximized: p.maximized,
    }
}

/// Apply a geometry to the window (the minimum size once the window exists).
pub fn apply(window: &slint::Window, g: &Geometry) {
    window.set_size(slint::LogicalSize::new(g.width, g.height));
    if let Some((x, y)) = g.position {
        window.set_position(slint::LogicalPosition::new(x, y));
    }
    window.set_maximized(g.maximized);
    let (mw, mh) = (g.min_width, g.min_height);
    window.with_winit_window(|w| {
        w.set_min_inner_size(Some(winit::dpi::LogicalSize::new(mw, mh)));
    });
}

/// The window's current state (logical pixels). While maximised the last normal
/// size and position are kept; while minimised nothing changes.
pub fn current(window: &slint::Window, last: WindowState) -> WindowState {
    if window.is_minimized() {
        return last;
    }
    let mut s = last;
    if window.is_maximized() {
        s.maximized = true;
        return s;
    }
    s.maximized = false;
    let scale = window.scale_factor().max(0.1);
    let size = window.size().to_logical(scale);
    if size.width >= 1.0 && size.height >= 1.0 {
        s.width = u32::try_from(mk_ui::to_pixels(size.width, 1.0)).unwrap_or(s.width);
        s.height = u32::try_from(mk_ui::to_pixels(size.height, 1.0)).unwrap_or(s.height);
    }
    let pos = window.position().to_logical(scale);
    s.x = Some(mk_ui::to_pixels(pos.x, 1.0));
    s.y = Some(mk_ui::to_pixels(pos.y, 1.0));
    s
}

/// Debounce: a change is saved once it is at least a second old.
#[derive(Debug, Default)]
pub struct Debounce {
    since: Option<Instant>,
}

impl Debounce {
    pub fn touch(&mut self) {
        self.since.get_or_insert_with(Instant::now);
    }

    /// Whether to save now (the app's timer asks again while waiting).
    pub fn due(&mut self) -> bool {
        match self.since {
            Some(t) if t.elapsed() >= Duration::from_secs(1) => {
                self.since = None;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AT62 (revision 8a): a saved 900×600 window restores as 1000×700 and cannot
    /// be resized below 1000×700.
    #[test]
    fn app_desktop_at62_minimum_window_size() {
        let saved = WindowState {
            width: 900,
            height: 600,
            x: Some(100),
            y: Some(80),
            maximized: false,
        };
        let screen = [ScreenRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }];
        let g = restore(&saved, &screen);
        assert_eq!((g.width, g.height), (1000.0, 700.0));
        assert_eq!((g.min_width, g.min_height), (1000.0, 700.0));
        assert_eq!(g.position, Some((100.0, 80.0)));
        // Without screen information the minimum still applies.
        let g = restore(&saved, &[]);
        assert_eq!((g.width, g.height), (1000.0, 700.0));
        assert_eq!((g.min_width, g.min_height), (1000.0, 700.0));
        // A screen smaller than the minimum caps it.
        let small = [ScreenRect {
            x: 0,
            y: 0,
            width: 960,
            height: 640,
        }];
        let g = restore(&saved, &small);
        assert_eq!((g.min_width, g.min_height), (960.0, 640.0));
        assert_eq!((g.width, g.height), (960.0, 640.0));
        // A larger saved size is kept.
        let big = WindowState {
            width: 1400,
            height: 900,
            ..saved
        };
        let g = restore(&big, &screen);
        assert_eq!((g.width, g.height), (1400.0, 900.0));
    }
}
