//! Test harness on Slint's testing backend (pinned exact version, ui-slint.md):
//! runs the real `.slint` window without a screen, finds elements by accessible
//! name, clicks, types and presses keys like a user, and checks values and
//! element geometry.
//!
//! Interactive elements carry an accessibility id "<role>|<item id>" (see
//! `ui/blocks.slint`); the role tag gives [`Role`], the item id lets the harness
//! ask the controller about focus.
#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use std::cell::{Cell, Ref, RefMut};

use i_slint_backend_testing::{ElementHandle, ElementQuery};
use mk_ui::ctl::App;
use slint::platform::{Key as SKey, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, SharedString};

use crate::Views;

thread_local! {
    static BACKEND: Cell<bool> = const { Cell::new(false) };
}

/// Use Slint's testing backend on this thread (once).
pub fn init() {
    BACKEND.with(|b| {
        if !b.get() {
            i_slint_backend_testing::init_no_event_loop();
            b.set(true);
        }
    });
}

/// Use Slint's testing backend on this thread (once) with Slint's software
/// renderer, so windows can be drawn into pixels (`PhoneHarness::pixels`). Text
/// is then measured with the system's fonts rather than the backend's fixed
/// metrics, so a test binary uses either this or [`init`], never both.
pub fn init_rendering() {
    BACKEND.with(|b| {
        if !b.get() {
            slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
                i_slint_backend_testing::TestingBackendOptions {
                    mock_time: true,
                    threading: false,
                    renderer_name: Some("software".into()),
                },
            )))
            .expect("platform already initialized");
            b.set(true);
        }
    });
}

/// A drawn window: RGBA pixels, `width` × `height` physical pixels.
#[derive(Debug, Clone)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub rgba: Vec<u8>,
}

impl Pixels {
    /// The colour at logical point (`x`, `y`) as 0xRRGGBB.
    pub fn at(&self, x: f32, y: f32) -> u32 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (px, py) = ((x * self.scale) as u32, (y * self.scale) as u32);
        let px = px.min(self.width.saturating_sub(1));
        let py = py.min(self.height.saturating_sub(1));
        let i = ((py * self.width + px) * 4) as usize;
        match self.rgba.get(i..i + 3) {
            Some(&[r, g, b]) => (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
            _ => 0,
        }
    }

    /// Every colour inside the logical rectangle `r`.
    pub fn colors_in(&self, r: Rect) -> Vec<u32> {
        let mut out = Vec::new();
        let step = 1.0 / self.scale.max(1.0);
        let mut y = r.top();
        while y < r.bottom() {
            let mut x = r.left();
            while x < r.right() {
                out.push(self.at(x, y));
                x += step;
            }
            y += step;
        }
        out
    }
}

/// WCAG 2.2 contrast ratio of two 0xRRGGBB colours (1 to 21).
pub fn contrast(a: u32, b: u32) -> f64 {
    fn lum(c: u32) -> f64 {
        let ch = |v: u32| {
            let s = f64::from(v & 0xff) / 255.0;
            if s <= 0.039_28 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(c >> 16) + 0.7152 * ch(c >> 8) + 0.0722 * ch(c)
    }
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// What an element opens (AccessKit's has-popup).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HasPopup {
    Menu,
}

/// What an element is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Button,
    TextInput,
    PasswordInput,
    MultilineTextInput,
    CheckBox,
    Switch,
    RadioButton,
    ComboBox,
    Tab,
    MenuItem,
    MenuItemCheckBox,
    ListBoxOption,
    Link,
    Label,
    Heading,
    Image,
    ListItem,
    Row,
    ColumnHeader,
    Slider,
    ProgressIndicator,
    ScrollBar,
    TitleBar,
    Unknown,
}

fn role_of_tag(tag: &str) -> Role {
    match tag {
        "button" | "iconbutton" | "toggle" | "menu" | "tile" | "disclosure" | "light" => {
            Role::Button
        }
        "input" => Role::TextInput,
        "password" => Role::PasswordInput,
        "multiline" => Role::MultilineTextInput,
        "check" => Role::CheckBox,
        "switch" => Role::Switch,
        "radio" => Role::RadioButton,
        "combo" => Role::ComboBox,
        "tab" | "railtab" | "icontab" => Role::Tab,
        "menuitem" => Role::MenuItem,
        "menucheck" => Role::MenuItemCheckBox,
        "option" | "suggestion" => Role::ListBoxOption,
        "link" | "chartpiece" => Role::Link,
        "label" | "chartlabel" => Role::Label,
        "heading" => Role::Heading,
        "image" | "aboutslint" => Role::Image,
        "listitem" => Role::ListItem,
        "row" => Role::Row,
        "header" => Role::ColumnHeader,
        "slider" => Role::Slider,
        "progress" => Role::ProgressIndicator,
        "scrollbar" => Role::ScrollBar,
        "titlebar" => Role::TitleBar,
        _ => Role::Unknown,
    }
}

/// Whether an element of role `have` answers a query for `want`. The old toolkit
/// exposed the entries of drop-down lists and menus, and clickable table rows, as
/// buttons; tests written then ask for buttons.
fn role_fits(want: Role, have: Role) -> bool {
    want == have
        || (want == Role::Button
            && matches!(
                have,
                Role::MenuItem
                    | Role::MenuItemCheckBox
                    | Role::ListBoxOption
                    | Role::Row
                    | Role::ListItem
            ))
}

fn role_of_slint(r: Option<i_slint_backend_testing::AccessibleRole>) -> Role {
    use i_slint_backend_testing::AccessibleRole as A;
    match r {
        Some(A::Button) => Role::Button,
        Some(A::TextInput) => Role::TextInput,
        Some(A::Checkbox) => Role::CheckBox,
        Some(A::Combobox) => Role::ComboBox,
        Some(A::Tab) => Role::Tab,
        Some(A::Text) => Role::Label,
        Some(A::Image) => Role::Image,
        Some(A::ListItem) => Role::ListItem,
        Some(A::Slider) => Role::Slider,
        Some(A::ProgressIndicator) => Role::ProgressIndicator,
        Some(A::Switch) => Role::Switch,
        Some(A::RadioButton) => Role::RadioButton,
        _ => Role::Unknown,
    }
}

/// Keys the tests press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    ArrowDown,
    ArrowUp,
    ArrowLeft,
    ArrowRight,
    Escape,
    Tab,
    Backspace,
    Enter,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Space,
    F1,
    F2,
    F6,
    Comma,
    Minus,
    Plus,
    Equals,
    Period,
    Slash,
    Asterisk,
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
}

fn key_text(key: Key) -> SharedString {
    let s: SharedString = match key {
        Key::ArrowDown => SKey::DownArrow.into(),
        Key::ArrowUp => SKey::UpArrow.into(),
        Key::ArrowLeft => SKey::LeftArrow.into(),
        Key::ArrowRight => SKey::RightArrow.into(),
        Key::Escape => SKey::Escape.into(),
        Key::Tab => SKey::Tab.into(),
        Key::Backspace => SKey::Backspace.into(),
        Key::Enter => SKey::Return.into(),
        Key::Delete => SKey::Delete.into(),
        Key::Home => SKey::Home.into(),
        Key::End => SKey::End.into(),
        Key::PageUp => SKey::PageUp.into(),
        Key::PageDown => SKey::PageDown.into(),
        Key::Insert => SKey::Insert.into(),
        Key::F1 => SKey::F1.into(),
        Key::F2 => SKey::F2.into(),
        Key::F6 => SKey::F6.into(),
        Key::Space => " ".into(),
        Key::Comma => ",".into(),
        Key::Minus => "-".into(),
        Key::Plus => "+".into(),
        Key::Equals => "=".into(),
        Key::Period => ".".into(),
        Key::Slash => "/".into(),
        Key::Asterisk => "*".into(),
        Key::Num0 => "0".into(),
        Key::Num1 => "1".into(),
        Key::Num2 => "2".into(),
        Key::Num3 => "3".into(),
        Key::Num4 => "4".into(),
        Key::Num5 => "5".into(),
        Key::Num6 => "6".into(),
        Key::Num7 => "7".into(),
        Key::Num8 => "8".into(),
        Key::Num9 => "9".into(),
        other => format!("{other:?}").to_lowercase().as_str().into(),
    };
    s
}

/// Modifier keys held with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: false,
    };
    /// Ctrl (Cmd on macOS).
    pub const COMMAND: Modifiers = Modifiers {
        ctrl: true,
        shift: false,
        alt: false,
    };
    pub const CTRL: Modifiers = Modifiers::COMMAND;
    pub const SHIFT: Modifiers = Modifiers {
        ctrl: false,
        shift: true,
        alt: false,
    };
    pub const ALT: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: true,
    };
}

impl std::ops::BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, o: Modifiers) -> Modifiers {
        Modifiers {
            ctrl: self.ctrl || o.ctrl,
            shift: self.shift || o.shift,
            alt: self.alt || o.alt,
        }
    }
}

/// A point in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pos2 {
    pub x: f32,
    pub y: f32,
}

impl Pos2 {
    pub const ZERO: Pos2 = Pos2 { x: 0.0, y: 0.0 };
    pub fn to_vec2(self) -> Vec2 {
        vec2(self.x, self.y)
    }
}

impl Vec2 {
    #[allow(clippy::float_arithmetic)]
    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    pub fn to_pos2(self) -> Pos2 {
        pos2(self.x, self.y)
    }
}

impl std::ops::Add<Vec2> for Pos2 {
    type Output = Pos2;
    #[allow(clippy::float_arithmetic)]
    fn add(self, v: Vec2) -> Pos2 {
        pos2(self.x + v.x, self.y + v.y)
    }
}

impl std::ops::Sub for Pos2 {
    type Output = Vec2;
    #[allow(clippy::float_arithmetic)]
    fn sub(self, o: Pos2) -> Vec2 {
        vec2(self.x - o.x, self.y - o.y)
    }
}

impl std::ops::Sub<Vec2> for Vec2 {
    type Output = Vec2;
    #[allow(clippy::float_arithmetic)]
    fn sub(self, o: Vec2) -> Vec2 {
        vec2(self.x - o.x, self.y - o.y)
    }
}

impl std::ops::Div<f32> for Vec2 {
    type Output = Vec2;
    #[allow(clippy::float_arithmetic)]
    fn div(self, d: f32) -> Vec2 {
        vec2(self.x / d, self.y / d)
    }
}

/// A size in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

pub fn vec2(x: f32, y: f32) -> Vec2 {
    Vec2 { x, y }
}

pub fn pos2(x: f32, y: f32) -> Pos2 {
    Pos2 { x, y }
}

/// A rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub min: Pos2,
    pub max: Pos2,
}

impl Rect {
    pub fn from_min_size(min: Pos2, size: Vec2) -> Rect {
        Rect {
            min,
            max: pos2(min.x + size.x, min.y + size.y),
        }
    }
    pub fn from_min_max(min: Pos2, max: Pos2) -> Rect {
        Rect { min, max }
    }
    pub fn left(&self) -> f32 {
        self.min.x
    }
    pub fn top(&self) -> f32 {
        self.min.y
    }
    pub fn right(&self) -> f32 {
        self.max.x
    }
    pub fn bottom(&self) -> f32 {
        self.max.y
    }
    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }
    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }
    pub fn size(&self) -> Vec2 {
        vec2(self.width(), self.height())
    }
    pub fn center(&self) -> Pos2 {
        pos2(
            (self.min.x + self.max.x) / 2.0,
            (self.min.y + self.max.y) / 2.0,
        )
    }
    pub fn shrink(&self, by: f32) -> Rect {
        self.expand(-by)
    }
    pub fn expand(&self, by: f32) -> Rect {
        Rect {
            min: pos2(self.min.x - by, self.min.y - by),
            max: pos2(self.max.x + by, self.max.y + by),
        }
    }
    /// Whether `o` lies inside (with half a pixel of slack).
    pub fn contains_rect(&self, o: Rect) -> bool {
        o.min.x >= self.min.x - 0.5
            && o.min.y >= self.min.y - 0.5
            && o.max.x <= self.max.x + 0.5
            && o.max.y <= self.max.y + 0.5
    }
    pub fn contains(&self, p: Pos2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }
    pub fn intersects(&self, o: Rect) -> bool {
        self.min.x < o.max.x && o.min.x < self.max.x && self.min.y < o.max.y && o.min.y < self.max.y
    }
}

/// A check box's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggled {
    True,
    False,
}

/// A text, image or other element the window shows (for geometry checks).
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub kind: &'static str,
    pub text: String,
    pub rect: Rect,
    /// Font size in pixels (from the type scale).
    pub size: f32,
    /// Whether the text is cut short (Slint texts wrap rather than cut, so this
    /// is only set where an element is narrower than its text's natural width
    /// and the element elides — tile names).
    pub elided: bool,
}

/// What the window shows (texts and images) and what was copied.
#[derive(Debug, Clone, Default)]
pub struct Output {
    pub shapes: Vec<Shape>,
    pub copied: Vec<String>,
}

/// Runs the app's window on the testing backend.
pub struct Harness {
    views: Views,
    time: f64,
    step_ms: u64,
    max_steps: usize,
    /// The field whose text Ctrl+A selected (typing replaces it).
    select_all: Cell<Option<String>>,
    /// Create every element in the next step.
    instantiate: Cell<bool>,
    /// The elements found since the last step or input (finding them is slow).
    cache: std::cell::RefCell<Option<Vec<Found>>>,
}

/// An element as found (before focus is looked up).
#[derive(Clone)]
struct Found {
    el: ElementHandle,
    tag: String,
    item: String,
    role: Role,
    label: String,
    rect: Rect,
    /// Behind a modal dialog: out of reach, but its text can still be read.
    behind: bool,
}

impl std::fmt::Debug for Harness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Harness")
    }
}

impl Harness {
    /// A harness showing `app` in a 1400 × 900 window.
    pub fn new(app: App) -> Self {
        Self::with_size(app, 1400.0, 900.0)
    }

    pub fn with_size(app: App, width: f32, height: f32) -> Self {
        init();
        let views = Views::new(app).expect("the testing backend creates windows");
        views
            .window()
            .window()
            .set_size(slint::LogicalSize::new(width, height));
        views.show().expect("the testing backend shows windows");
        let mut h = Harness {
            views,
            time: 0.0,
            step_ms: 250,
            max_steps: 4,
            select_all: Cell::new(None),
            instantiate: Cell::new(true),
            cache: std::cell::RefCell::new(None),
        };
        h.run();
        h
    }

    /// Steps per `run` (after each action the window settles in a few).
    pub fn with_max_steps(mut self, n: usize) -> Self {
        self.max_steps = n;
        self
    }

    pub fn views(&self) -> &Views {
        &self.views
    }

    pub fn state(&self) -> Ref<'_, App> {
        self.views.app()
    }

    pub fn state_mut(&mut self) -> RefMut<'_, App> {
        self.invalidate();
        self.views.app_mut()
    }

    pub fn window(&self) -> &slint::Window {
        self.views.window().window()
    }

    /// Change the window size (logical pixels).
    pub fn set_size(&mut self, size: Vec2) {
        self.window()
            .set_size(slint::LogicalSize::new(size.x, size.y));
        self.run();
    }

    /// The window's size (logical pixels).
    pub fn size(&self) -> Vec2 {
        let s = self
            .window()
            .size()
            .to_logical(self.window().scale_factor());
        vec2(s.width, s.height)
    }

    /// Texts and images the window shows, and text copied since the last
    /// `take_copied`.
    pub fn output(&self) -> Output {
        let shapes = self
            .nodes()
            .into_iter()
            .filter_map(|n| {
                // A chart's bar or point is a link that draws no text (its name
                // is its label and value).
                if n.tag == "chartpiece" {
                    return None;
                }
                // Icon buttons and rail tabs draw a picture, not text.
                if matches!(
                    n.tag.as_str(),
                    "iconbutton" | "icontab" | "railtab" | "toggle"
                ) {
                    return Some(Shape {
                        kind: "image",
                        text: n.label.clone(),
                        rect: n.rect,
                        size: 14.0,
                        elided: false,
                    });
                }
                let kind = match n.role {
                    Role::Image => "image",
                    Role::Label
                    | Role::Heading
                    | Role::Button
                    | Role::Link
                    | Role::Tab
                    | Role::CheckBox
                    | Role::RadioButton
                    | Role::MenuItemCheckBox
                    | Role::ListItem => "text",
                    _ => return None,
                };
                let size = match n.tag.as_str() {
                    "heading" => 17.0,
                    _ => 14.0,
                };
                // A text named for its row shows its words as its value.
                let drawn = if n.role == Role::Label {
                    n.el.accessible_value()
                        .map(|v| v.to_string())
                        .filter(|v| !v.is_empty())
                } else {
                    None
                };
                Some(Shape {
                    kind,
                    text: drawn.unwrap_or_else(|| n.label.clone()),
                    rect: n.rect,
                    size,
                    elided: false,
                })
            })
            .collect::<Vec<Shape>>();
        // Texts drawn inside other elements (a tile's name under its square, the
        // letter in it) that are not already counted by an element of that name.
        let mut shapes = shapes;
        let inner: Vec<Shape> = ElementQuery::from_root(self.views.window())
            .match_descendants()
            .match_type_name("Text")
            .find_all()
            .into_iter()
            .filter_map(|el| {
                let label = el.accessible_label()?.to_string();
                if label.trim().is_empty() {
                    return None;
                }
                let p = el.absolute_position();
                let sz = el.size();
                let rect = Rect {
                    min: pos2(p.x, p.y),
                    max: pos2(p.x + sz.width, p.y + sz.height),
                };
                let c = rect.center();
                let counted = shapes
                    .iter()
                    .any(|s| s.text == label && s.rect.expand(1.0).contains(c));
                (!counted && sz.width > 1.0).then_some(Shape {
                    kind: "text",
                    text: label,
                    rect,
                    size: 14.0,
                    elided: false,
                })
            })
            .collect();
        shapes.extend(inner);
        Output {
            shapes,
            copied: self.views.app().copied_so_far(),
        }
    }

    /// One step: advance the clock, run timers, animations and change handlers,
    /// and refresh the window from the controller.
    /// Forget the elements found (after anything that may change them).
    fn invalidate(&self) {
        *self.cache.borrow_mut() = None;
    }

    pub fn step(&mut self) {
        self.invalidate();
        #[allow(clippy::cast_precision_loss)]
        {
            self.time += self.step_ms as f64 / 1000.0;
        }
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(self.step_ms));
        self.views.tick(self.time);
        slint::platform::update_timers_and_animations();
        self.views.refresh();
        // Create every element now (repeaters are lazy without rendering), so
        // fields focused on creation report it in this step.
        if self.instantiate.get() {
            let _ = ElementQuery::from_root(self.views.window())
                .match_descendants()
                .find_all();
        }
        slint::platform::update_timers_and_animations();
    }

    /// Steps until the window settled.
    pub fn run(&mut self) {
        // Short steps (about a quarter second in all: animations settle, timers of
        // a second or more do not fire); `run_steps` lets time pass.
        let long = self.step_ms;
        self.step_ms = 60;
        for i in 0..self.max_steps {
            self.instantiate.set(i < 1);
            self.step();
        }
        self.step_ms = long;
        self.instantiate.set(true);
    }

    /// Advance time by `secs` seconds (timers such as the pane sliding away).
    pub fn advance(&mut self, secs: f64) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let steps = ((secs * 1000.0) / (self.step_ms as f64)).ceil() as usize;
        for _ in 0..steps.max(1) {
            self.step();
        }
    }

    pub fn run_steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Text the UI copied since the last call.
    pub fn take_copied(&mut self) -> Vec<String> {
        self.views.app_mut().take_copied()
    }

    // ----- input -------------------------------------------------------------------

    fn dispatch(&self, e: WindowEvent) {
        self.invalidate();
        self.window().dispatch_event(e);
    }

    pub fn key_press(&mut self, key: Key) {
        self.key_press_modifiers(Modifiers::NONE, key);
    }

    pub fn key_press_modifiers(&mut self, m: Modifiers, key: Key) {
        if m.ctrl && key == Key::A {
            let focused = self.views.app().focused_item().map(str::to_owned);
            self.select_all.set(focused);
        }
        let mods: Vec<(SharedString, bool)> = [
            (SKey::Control, m.ctrl),
            (SKey::Shift, m.shift),
            (SKey::Alt, m.alt),
        ]
        .into_iter()
        .map(|(k, on)| (SharedString::from(k), on))
        .collect();
        for (k, on) in &mods {
            if *on {
                self.dispatch(WindowEvent::KeyPressed { text: k.clone() });
            }
        }
        let text = key_text(key);
        self.dispatch(WindowEvent::KeyPressed { text: text.clone() });
        self.dispatch(WindowEvent::KeyReleased { text });
        for (k, on) in mods.iter().rev() {
            if *on {
                self.dispatch(WindowEvent::KeyReleased { text: k.clone() });
            }
        }
    }

    /// Type text into whatever has focus, key by key.
    pub fn type_keys(&mut self, text: &str) {
        for c in text.chars() {
            let s = SharedString::from(c.to_string().as_str());
            self.dispatch(WindowEvent::KeyPressed { text: s.clone() });
            self.dispatch(WindowEvent::KeyReleased { text: s });
        }
    }

    /// Paste `text` into the focused field (as the system clipboard would).
    pub fn paste(&mut self, text: &str) {
        self.invalidate();
        let focused = self.views.app().focused_item().map(str::to_owned);
        let Some(item) = focused else {
            return;
        };
        let target = self
            .nodes()
            .into_iter()
            .find(|n| n.item == item)
            .map(|n| (n.el.clone(), n.value().unwrap_or_default()));
        if let Some((el, cur)) = target {
            let all = self.select_all.take().is_some_and(|s| s == item)
                || self.views.app().is_all_selected(&item);
            let new = if all {
                text.to_owned()
            } else {
                format!("{cur}{text}")
            };
            el.set_accessible_value(new);
        }
    }

    /// Files dropped on the window (the app shell hands them to the controller).
    pub fn drop_files(&mut self, files: Vec<mk_ui::PickedFile>) {
        self.invalidate();
        self.views.with_app(|a| a.drop_files(files));
        self.views.refresh();
    }

    /// Move the pointer to `p`.
    pub fn hover_at(&mut self, p: Pos2) {
        self.dispatch(WindowEvent::PointerMoved {
            position: LogicalPosition::new(p.x, p.y),
        });
    }

    /// Press the left button at `p` (a drag starts).
    pub fn drag_at(&mut self, p: Pos2) {
        let p = LogicalPosition::new(p.x, p.y);
        self.dispatch(WindowEvent::PointerMoved { position: p });
        self.dispatch(WindowEvent::PointerPressed {
            position: p,
            button: PointerEventButton::Left,
        });
    }

    /// Release the left button at `p` (a drag ends).
    pub fn drop_at(&mut self, p: Pos2) {
        let p = LogicalPosition::new(p.x, p.y);
        self.dispatch(WindowEvent::PointerMoved { position: p });
        self.dispatch(WindowEvent::PointerReleased {
            position: p,
            button: PointerEventButton::Left,
        });
    }

    /// Press the left button at `p` without moving the pointer first (a real
    /// mouse: the move came earlier, its own event).
    pub fn press_at(&mut self, p: Pos2) {
        self.dispatch(WindowEvent::PointerPressed {
            position: LogicalPosition::new(p.x, p.y),
            button: PointerEventButton::Left,
        });
    }

    /// Release the left button at `p` without moving the pointer first.
    pub fn release_at(&mut self, p: Pos2) {
        self.dispatch(WindowEvent::PointerReleased {
            position: LogicalPosition::new(p.x, p.y),
            button: PointerEventButton::Left,
        });
    }

    /// The text of the tooltip showing, if any.
    pub fn tooltip(&self) -> Option<String> {
        use crate::sl::Act;
        use slint::ComponentHandle;
        let t = self.views.window().global::<Act>().get_tip();
        (!t.is_empty()).then(|| t.to_string())
    }

    /// Move the pointer out of the window.
    pub fn remove_cursor(&mut self) {
        self.dispatch(WindowEvent::PointerExited);
    }

    /// The window's scale factor (the text size: 1.5 at 150 %).
    pub fn zoom(&self) -> f32 {
        self.window().scale_factor()
    }

    /// The window's content area in logical pixels.
    pub fn window_rect(&self) -> Rect {
        let s = self.size();
        Rect::from_min_size(Pos2::ZERO, s)
    }

    /// A theme colour as 0xRRGGBB (the window's `Theme` global): `scroll-track`,
    /// `scroll-handle`, `page`, `surface`, `accent`.
    pub fn theme_color(&self, name: &str) -> u32 {
        use crate::sl::Theme;
        use slint::ComponentHandle;
        let t = self.views.window().global::<Theme>();
        let c = match name {
            "scroll-track" => t.get_scroll_track(),
            "scroll-handle" => t.get_scroll_handle(),
            "page" => t.get_page(),
            "surface" => t.get_surface(),
            _ => t.get_accent(),
        };
        (u32::from(c.red()) << 16) | (u32::from(c.green()) << 8) | u32::from(c.blue())
    }

    /// Whether the window shows the dark theme.
    pub fn is_dark(&self) -> bool {
        use crate::sl::Theme;
        use slint::ComponentHandle;
        self.views.window().global::<Theme>().get_dark()
    }

    /// Click at `p`.
    pub fn click_at(&mut self, p: Pos2) {
        let p = LogicalPosition::new(p.x, p.y);
        self.dispatch(WindowEvent::PointerMoved { position: p });
        self.dispatch(WindowEvent::PointerPressed {
            position: p,
            button: PointerEventButton::Left,
        });
        self.dispatch(WindowEvent::PointerReleased {
            position: p,
            button: PointerEventButton::Left,
        });
    }

    // ----- queries -----------------------------------------------------------------

    /// Every element with an accessible name or an accessibility id, once each.
    /// An estimate of a text's natural width at `size` pixels (half an em per
    /// character, a little over the interface font's average).
    #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
    pub fn text_width(&self, text: &str, size: f32) -> f32 {
        text.chars().count() as f32 * size * 0.5
    }

    /// The date-format guide drawn in the focused field ("" when none shows).
    pub fn date_guide(&self) -> String {
        self.nodes()
            .into_iter()
            .find(|n| n.tag == "guide" && !n.label.is_empty())
            .map(|n| n.label.clone())
            .unwrap_or_default()
    }

    pub fn nodes(&self) -> Vec<Node<'_>> {
        self.nodes_where(false)
    }

    /// What is behind the open modal dialog (read-only checks, such as a row's
    /// selection, look there when the dialog has no match).
    fn texts_behind(&self) -> Vec<Node<'_>> {
        self.nodes_where(true)
    }

    fn nodes_where(&self, behind: bool) -> Vec<Node<'_>> {
        if self.cache.borrow().is_none() {
            let found = self.find();
            *self.cache.borrow_mut() = Some(found);
        }
        let focused = self.views.app().focused_item().map(str::to_owned);
        self.cache
            .borrow()
            .as_ref()
            .map(|v| {
                v.iter()
                    .filter(|f| f.behind == behind)
                    .map(|f| Node {
                        tag: f.tag.clone(),
                        harness: self,
                        focused: !f.item.is_empty() && focused.as_deref() == Some(f.item.as_str()),
                        item: f.item.clone(),
                        role: f.role,
                        label: f.label.clone(),
                        rect: f.rect,
                        el: f.el.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn find(&self) -> Vec<Found> {
        let all = ElementQuery::from_root(self.views.window())
            .match_descendants()
            .match_predicate(|e| {
                e.accessible_id().is_some_and(|i| i.contains('|'))
                    || e.accessible_label().is_some_and(|l| !l.is_empty())
            })
            .find_all();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out = Vec::new();
        for el in all {
            let aid = el
                .accessible_id()
                .map(|s| s.to_string())
                .unwrap_or_default();
            if aid.contains('|') && !aid.ends_with('|') && !seen.insert(aid.clone()) {
                continue;
            }
            let (tag, item) = aid.split_once('|').unwrap_or(("", ""));
            let role = if tag.is_empty() {
                role_of_slint(el.accessible_role())
            } else {
                role_of_tag(tag)
            };
            let label = el
                .accessible_label()
                .map(|s| s.to_string())
                .unwrap_or_default();
            if label.is_empty()
                && !matches!(
                    role,
                    Role::TextInput | Role::PasswordInput | Role::MultilineTextInput
                )
                && tag.is_empty()
            {
                continue;
            }
            let p = el.absolute_position();
            let sz = el.size();
            let rect = Rect {
                min: pos2(p.x, p.y),
                max: pos2(p.x + sz.width, p.y + sz.height),
            };
            if item.is_empty() {
                let key = format!("{tag}|{label}|{:?}|{:?}", rect.min, rect.max);
                if !seen.insert(key) {
                    continue;
                }
            }
            out.push(Found {
                tag: tag.to_owned(),
                item: item.split('@').next().unwrap_or("").to_owned(),
                role,
                label,
                rect,
                el,
                behind: false,
            });
        }
        // A dialog is modal: what is behind it is out of reach (as a screen reader
        // treats a modal dialog), except the menus and tooltips over it and the
        // floating calculator.
        let top_dialog = out
            .iter()
            .rfind(|f| f.tag == "dialog")
            .map(|f| (f.rect, f.item.clone()));
        out.retain(|f| f.tag != "dialog");
        // The status message pop-up shows above the dialog's scrim when it
        // appeared while the dialog was open (app-desktop.md revision 42).
        let message_over = self
            .views
            .window()
            .get_message_over_dialog()
            .then(|| out.iter().find(|f| f.tag == "message").map(|f| f.rect))
            .flatten();
        if let Some((d, top_id)) = top_dialog {
            // The controls of a dialog under the top one are behind it even where
            // the top one covers them.
            let under = |f: &Found| {
                f.item.starts_with("dialog:")
                    && !top_id.is_empty()
                    && !f.item.starts_with(top_id.as_str())
            };
            for f in &mut out {
                f.behind = under(f)
                    || !(d.expand(1.0).contains_rect(f.rect)
                        || matches!(
                            f.tag.as_str(),
                            "menuitem"
                                | "menucheck"
                                | "option"
                                | "suggestion"
                                | "tooltip"
                                | "titlebar"
                                | "guide"
                        )
                        || f.item.starts_with("calc/")
                        || message_over.is_some_and(|m| m.expand(1.0).contains_rect(f.rect))
                        || (f.role == Role::ScrollBar && d.expand(1.0).intersects(f.rect)));
            }
        }
        out
    }

    fn matching(&self, label: &str, contains: bool, role: Option<Role>) -> Vec<Node<'_>> {
        let found = |h: &Harness| -> bool {
            h.nodes().iter().any(|n| {
                let ok = if contains {
                    n.label.contains(label)
                } else {
                    n.label == label
                };
                ok && role.is_none_or(|r| role_fits(r, n.role))
            })
        };
        if !found(self) {
            // Like a person looking for it: scroll the views until it shows.
            self.reveal(&found);
        }
        let v: Vec<Node<'_>> = self
            .nodes()
            .into_iter()
            .filter(|n| {
                let ok = if contains {
                    n.label.contains(label)
                } else {
                    n.label == label
                };
                ok && role.is_none_or(|r| role_fits(r, n.role))
            })
            .collect();
        // No name matches: the words a text or link draws, when it is named for
        // its row or target ("Show transactions for Salary" draws "Salary").
        let v = if v.is_empty()
            && !contains
            && role.is_none_or(|r| matches!(r, Role::Label | Role::Link))
        {
            self.nodes()
                .into_iter()
                .filter(|n| {
                    matches!(n.role, Role::Label | Role::Link)
                        && n.el.accessible_value().is_some_and(|d| d.as_str() == label)
                })
                .collect()
        } else {
            v
        };
        // Nothing in the modal dialog: a text behind it can still be read.
        let v = if v.is_empty() {
            self.texts_behind()
                .into_iter()
                .filter(|n| {
                    let ok = if contains {
                        n.label.contains(label)
                    } else {
                        n.label == label
                    };
                    ok && role.is_none_or(|r| role_fits(r, n.role))
                })
                .collect()
        } else {
            v
        };
        // Like AccessKit's labelled-by (and testing-library's getByLabelText): a
        // static text that names a control is the control's label, not a second
        // match, so a control with the same name wins over plain text.
        let is_text = |n: &Node<'_>| matches!(n.role, Role::Label | Role::Heading);
        let is_control = |n: &Node<'_>| {
            !matches!(
                n.role,
                Role::Label
                    | Role::Heading
                    | Role::Image
                    | Role::Unknown
                    | Role::Row
                    | Role::ListItem
                    | Role::ColumnHeader
                    | Role::ProgressIndicator
                    | Role::ScrollBar
                    | Role::TitleBar
            )
        };
        if !contains && role.is_none() && v.iter().any(is_control) && v.iter().any(is_text) {
            return v.into_iter().filter(|n| !is_text(n)).collect();
        }
        // A chart's axis label repeats a name the page shows as text (the chart
        // itself is described as a whole): the page's text is the match.
        if v.len() > 1
            && v.iter().any(|n| n.tag == "chartlabel")
            && v.iter().any(|n| n.tag != "chartlabel")
        {
            return v.into_iter().filter(|n| n.tag != "chartlabel").collect();
        }
        v
    }

    /// Scroll each scroll bar through its range until `found` holds; leaves the
    /// view where it was found (or where it was, when nowhere).
    #[allow(clippy::float_arithmetic)]
    fn reveal(&self, found: &dyn Fn(&Harness) -> bool) {
        let bars: Vec<(ElementHandle, f32, f32)> = self
            .nodes()
            .into_iter()
            .filter(|n| n.role == Role::ScrollBar)
            .map(|n| {
                let max = n.el.accessible_value_maximum().unwrap_or(0.0);
                let now =
                    n.el.accessible_value()
                        .and_then(|v| v.parse::<f32>().ok())
                        .unwrap_or(0.0);
                (n.el.clone(), max, now)
            })
            .collect();
        for (bar, max, now) in bars {
            // Steps of 240 px, then the very end.
            let mut at: f32 = 0.0;
            loop {
                bar.set_accessible_value(format!("{at}"));
                self.settle();
                if found(self) {
                    return;
                }
                if at >= max {
                    break;
                }
                at = (at + 240.0).min(max);
            }
            bar.set_accessible_value(format!("{now}"));
            self.settle();
        }
    }

    /// Let the window lay out after a change made from the harness.
    fn settle(&self) {
        self.invalidate();
        slint::platform::update_timers_and_animations();
    }

    #[track_caller]
    fn one<'n>(&self, mut v: Vec<Node<'n>>, what: &str) -> Option<Node<'n>> {
        if v.len() > 1 {
            let found: Vec<String> = v
                .iter()
                .map(|n| {
                    format!(
                        "{:?} {:?} at {:?}",
                        n.role,
                        n.label,
                        (n.rect.min.x, n.rect.min.y)
                    )
                })
                .collect();
            panic!("Found two or more nodes matching {what:?}: {found:?}");
        }
        v.pop()
    }

    #[track_caller]
    pub fn query_by_label(&self, label: &str) -> Option<Node<'_>> {
        let mut v = self.matching(label, false, None);
        // A tooltip repeating a text under the pointer is not a second match.
        if v.len() > 1 && v.iter().any(|n| n.tag != "tooltip") {
            v.retain(|n| n.tag != "tooltip");
        }
        self.one(v, label)
    }
    #[track_caller]
    pub fn get_by_label(&self, label: &str) -> Node<'_> {
        self.query_by_label(label).unwrap_or_else(|| {
            panic!(
                "No nodes found matching label {label:?}; labels: {:?}",
                self.all_labels()
            )
        })
    }
    pub fn query_all_by_label(&self, label: &str) -> impl Iterator<Item = Node<'_>> {
        self.matching(label, false, None).into_iter()
    }
    #[track_caller]
    pub fn get_all_by_label(&self, label: &str) -> impl Iterator<Item = Node<'_>> {
        let v = self.matching(label, false, None);
        assert!(!v.is_empty(), "No nodes found matching label {label:?}");
        v.into_iter()
    }
    #[track_caller]
    pub fn query_by_label_contains(&self, part: &str) -> Option<Node<'_>> {
        self.one(self.matching(part, true, None), part)
    }
    #[track_caller]
    pub fn get_by_label_contains(&self, part: &str) -> Node<'_> {
        self.query_by_label_contains(part)
            .unwrap_or_else(|| panic!("No nodes found with a label containing {part:?}"))
    }
    pub fn query_all_by_label_contains(&self, part: &str) -> impl Iterator<Item = Node<'_>> {
        self.matching(part, true, None).into_iter()
    }
    #[track_caller]
    pub fn query_by_role_and_label(&self, role: Role, label: &str) -> Option<Node<'_>> {
        self.one(self.matching(label, false, Some(role)), label)
    }
    #[track_caller]
    pub fn get_by_role_and_label(&self, role: Role, label: &str) -> Node<'_> {
        self.query_by_role_and_label(role, label)
            .unwrap_or_else(|| {
                panic!(
                    "No {role:?} found with label {label:?}; labels: {:?}",
                    self.all_labels()
                )
            })
    }
    pub fn query_all_by_role_and_label(
        &self,
        role: Role,
        label: &str,
    ) -> impl Iterator<Item = Node<'_>> {
        self.matching(label, false, Some(role)).into_iter()
    }
    pub fn query_all_by_role(&self, role: Role) -> impl Iterator<Item = Node<'_>> {
        self.nodes()
            .into_iter()
            .filter(move |n| role_fits(role, n.role))
    }
    #[track_caller]
    pub fn get_by_role(&self, role: Role) -> Node<'_> {
        let v: Vec<_> = self.query_all_by_role(role).collect();
        self.one(v, "role")
            .unwrap_or_else(|| panic!("No nodes with role {role:?}"))
    }
    pub fn query_all_by(&self, f: impl Fn(&Node<'_>) -> bool) -> impl Iterator<Item = Node<'_>> {
        self.nodes().into_iter().filter(move |n| f(n))
    }
    /// Labels of every element (for failure messages).
    /// Whether a text containing `part` is on screen now (no scrolling to find it).
    pub fn on_screen_contains(&self, part: &str) -> bool {
        self.nodes().iter().any(|n| n.label.contains(part))
    }
    pub fn all_labels(&self) -> Vec<String> {
        self.nodes()
            .into_iter()
            .map(|n| n.label)
            .filter(|l| !l.is_empty())
            .collect()
    }
}

/// One element found in the window.
pub struct Node<'h> {
    tag: String,
    harness: &'h Harness,
    el: ElementHandle,
    item: String,
    role: Role,
    label: String,
    rect: Rect,
    focused: bool,
}

impl std::fmt::Debug for Node<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("role", &self.role)
            .field("label", &self.label)
            .field("rect", &self.rect)
            .finish()
    }
}

impl Node<'_> {
    pub fn role(&self) -> Role {
        self.role
    }
    pub fn label(&self) -> Option<String> {
        Some(self.label.clone()).filter(|l| !l.is_empty())
    }
    /// The kind of element, from its accessibility id (`legend`, `chartbox` …).
    pub fn tag(&self) -> &str {
        &self.tag
    }
    /// The controller's id of the item.
    pub fn item_id(&self) -> &str {
        &self.item
    }
    /// A field's text, a drop-down's choice, a text's text.
    pub fn value(&self) -> Option<String> {
        match self.role {
            Role::Label | Role::Heading => Some(self.label.clone()),
            // AccessKit's platform layer reads a password field as bullets.
            Role::PasswordInput => {
                Some("•".repeat(self.el.accessible_value().map_or(0, |s| s.chars().count())))
            }
            Role::TextInput | Role::MultilineTextInput => Some(
                self.el
                    .accessible_value()
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
            ),
            _ => self
                .el
                .accessible_value()
                .map(|s| s.to_string())
                .filter(|v| !v.is_empty()),
        }
    }
    pub fn rect(&self) -> Rect {
        self.rect
    }
    pub fn is_disabled(&self) -> bool {
        self.el.accessible_enabled() == Some(false)
    }
    pub fn is_focused(&self) -> bool {
        self.focused
    }
    /// Selected (tabs, list entries) or checked.
    pub fn is_selected(&self) -> Option<bool> {
        Some(
            self.el.accessible_item_selected().unwrap_or(false)
                || self.el.accessible_checked().unwrap_or(false),
        )
    }
    /// A check box's or toggle's state (`None` for other elements).
    pub fn toggled(&self) -> Option<Toggled> {
        let on = match self.role {
            Role::CheckBox | Role::Switch | Role::MenuItemCheckBox | Role::RadioButton => {
                Some(self.el.accessible_checked().unwrap_or(false))
            }
            _ if self.el.accessible_checkable() == Some(true) => {
                Some(self.el.accessible_checked().unwrap_or(false))
            }
            // A selectable row: its selection.
            Role::Row | Role::ListItem if self.el.accessible_item_selectable() == Some(true) => {
                Some(self.el.accessible_item_selected().unwrap_or(false))
            }
            _ => None,
        };
        on.map(|b| if b { Toggled::True } else { Toggled::False })
    }
    pub fn description(&self) -> Option<String> {
        self.el
            .accessible_description()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    }
    pub fn placeholder(&self) -> Option<String> {
        self.el
            .accessible_placeholder_text()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
    }
    /// What opens from the element: a menu for menu buttons and drop-downs.
    pub fn has_popup(&self) -> Option<HasPopup> {
        (self.tag == "menu" || self.el.accessible_expandable() == Some(true))
            .then_some(HasPopup::Menu)
    }
    pub fn is_expanded(&self) -> bool {
        self.el.accessible_expanded() == Some(true)
    }
    pub fn numeric_value(&self) -> Option<f64> {
        self.value().and_then(|v| v.parse().ok())
    }
    /// Compatibility with tests written for an AccessKit tree.
    pub fn accesskit_node(&self) -> &Self {
        self
    }

    /// Click (the element's accessible default action; text fields get focus).
    pub fn click(&self) {
        self.harness.invalidate();
        if self.is_disabled() {
            return;
        }
        match self.role {
            Role::TextInput | Role::PasswordInput | Role::MultilineTextInput => self.focus(),
            // A text has no action of its own: click where it is drawn (a table
            // cell opens its row).
            // A rail tab is clicked with the pointer too (the pane then closes a
            // while after the pointer leaves it).
            _ if self.tag == "railtab"
                || (self.role == Role::Label && (!self.tag.is_empty() || self.item.is_empty())) =>
            {
                let c = self.rect.center();
                let w = self.harness.window();
                let p = LogicalPosition::new(c.x, c.y);
                w.dispatch_event(WindowEvent::PointerMoved { position: p });
                w.dispatch_event(WindowEvent::PointerPressed {
                    position: p,
                    button: PointerEventButton::Left,
                });
                w.dispatch_event(WindowEvent::PointerReleased {
                    position: p,
                    button: PointerEventButton::Left,
                });
            }
            _ => self.el.invoke_accessible_default_action(),
        }
    }
    /// Secondary click: opens the element's context menu.
    pub fn click_secondary(&self) {
        self.harness.invalidate();
        let c = self.rect.center();
        let w = self.harness.window();
        let p = LogicalPosition::new(c.x, c.y);
        w.dispatch_event(WindowEvent::PointerMoved { position: p });
        w.dispatch_event(WindowEvent::PointerPressed {
            position: p,
            button: PointerEventButton::Right,
        });
        w.dispatch_event(WindowEvent::PointerReleased {
            position: p,
            button: PointerEventButton::Right,
        });
    }
    /// Give keyboard focus.
    pub fn focus(&self) {
        self.harness.invalidate();
        self.harness.select_all.set(None);
        if self.tag == "block" {
            // A dashboard block's handle takes focus through its default action.
            self.el.invoke_accessible_default_action();
            return;
        }
        if !self.item.is_empty() {
            self.harness.views.with_app(|a| a.request_focus(&self.item));
        }
    }
    /// Type `text` at the end of the field (or replace it after Ctrl+A).
    pub fn type_text(&self, text: &str) {
        self.harness.invalidate();
        let selected = self.harness.select_all.take();
        let replace = (selected.is_some() && selected.as_deref() == Some(self.item.as_str()))
            || (!self.item.is_empty() && self.harness.views.app().is_all_selected(&self.item));
        let cur = self
            .el
            .accessible_value()
            .map(|v| v.to_string())
            .unwrap_or_default();
        let new = if replace {
            text.to_owned()
        } else {
            format!("{cur}{text}")
        };
        self.el.set_accessible_value(new);
    }
    /// Replace the field's text.
    pub fn set_text(&self, text: &str) {
        self.harness.invalidate();
        self.el.set_accessible_value(text);
    }
    /// Move the pointer over the element.
    pub fn hover(&self) {
        self.harness.invalidate();
        let c = self.rect.center();
        self.harness
            .window()
            .dispatch_event(WindowEvent::PointerMoved {
                position: LogicalPosition::new(c.x, c.y),
            });
    }
    /// The element handle (for anything else).
    pub fn element(&self) -> &ElementHandle {
        &self.el
    }
    /// Bring the element into view (a no-op: elements are found off screen too).
    /// Scroll the nearest sideways scroll bar below the element until it is in
    /// view (vertical scrolling happens when it is looked up).
    #[allow(clippy::float_arithmetic)]
    pub fn scroll_to_me(&self) {
        let me = self.rect;
        let bar = self
            .harness
            .nodes()
            .into_iter()
            .filter(|n| {
                n.role == Role::ScrollBar
                    && n.rect.width() > n.rect.height()
                    && n.rect.min.y >= me.min.y
            })
            .min_by(|a, b| (a.rect.min.y - me.max.y).total_cmp(&(b.rect.min.y - me.max.y)));
        let Some(bar) = bar else {
            return;
        };
        let now = bar
            .el
            .accessible_value()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);
        let shift = if me.max.x > bar.rect.max.x {
            me.max.x - bar.rect.max.x + 8.0
        } else if me.min.x < bar.rect.min.x {
            me.min.x - bar.rect.min.x - 8.0
        } else {
            0.0
        };
        if shift != 0.0 {
            bar.el
                .set_accessible_value(format!("{}", (now + shift).max(0.0)));
            self.harness.settle();
        }
    }
}

// ----- the phone window -------------------------------------------------------------------

/// A harness over the phone window (`PhoneWindow`) on Slint's testing backend: the
/// phone app's screens found by accessible name, clicked like a finger would.
pub struct PhoneHarness<A: crate::phone::PageApp + 'static> {
    view: crate::phone::PhoneView<A>,
    step_ms: u64,
}

impl<A: crate::phone::PageApp + 'static> std::fmt::Debug for PhoneHarness<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PhoneHarness")
    }
}

/// An element of the phone window.
#[derive(Clone)]
pub struct PhoneNode {
    el: ElementHandle,
    window: slint::Weak<crate::sl::PhoneWindow>,
    pub label: String,
    rect: Rect,
}

impl std::fmt::Debug for PhoneNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} at {:?}", self.label, self.rect)
    }
}

impl PhoneNode {
    pub fn rect(&self) -> Rect {
        self.rect
    }
    /// Its accessibility id ("<role>|<item id>").
    pub fn aid(&self) -> String {
        self.el
            .accessible_id()
            .map(|s| s.to_string())
            .unwrap_or_default()
    }
    /// Its accessible value (a light's state).
    pub fn value(&self) -> String {
        self.el
            .accessible_value()
            .map(|s| s.to_string())
            .unwrap_or_default()
    }
    /// Whether it is enabled.
    pub fn enabled(&self) -> bool {
        self.el.accessible_enabled().unwrap_or(true)
    }
    /// Whether it is selected (a calendar day).
    pub fn selected(&self) -> bool {
        self.el.accessible_item_selected().unwrap_or(false)
    }
    /// Replace a field's text, as typing does (no focus moves).
    pub fn set_text(&self, text: &str) {
        self.el.set_accessible_value(text);
    }
    /// Tap it: its default action, or a press at its centre for a plain text.
    pub fn click(&self) {
        if self.el.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Text) {
            if let Some(w) = self.window.upgrade() {
                let c = self.rect.center();
                let p = LogicalPosition::new(c.x, c.y);
                let win = w.window();
                win.dispatch_event(WindowEvent::PointerMoved { position: p });
                win.dispatch_event(WindowEvent::PointerPressed {
                    position: p,
                    button: PointerEventButton::Left,
                });
                win.dispatch_event(WindowEvent::PointerReleased {
                    position: p,
                    button: PointerEventButton::Left,
                });
            }
            return;
        }
        self.el.invoke_accessible_default_action();
    }
}

impl<A: crate::phone::PageApp + 'static> PhoneHarness<A> {
    /// A phone window `width` × `height` over `app`.
    pub fn new(app: A, width: f32, height: f32) -> Self {
        init();
        let view = crate::phone::PhoneView::new(app).expect("the testing backend creates windows");
        view.window()
            .window()
            .set_size(slint::LogicalSize::new(width, height));
        view.show().expect("the testing backend shows windows");
        let mut h = PhoneHarness { view, step_ms: 250 };
        h.run_steps(2);
        h
    }

    /// A phone window drawn with Slint's software renderer (see
    /// [`init_rendering`]), for checks of drawn colours.
    pub fn rendered(app: A, width: f32, height: f32) -> Self {
        init_rendering();
        let view = crate::phone::PhoneView::new(app).expect("the testing backend creates windows");
        view.window()
            .window()
            .set_size(slint::LogicalSize::new(width, height));
        view.show().expect("the testing backend shows windows");
        let mut h = PhoneHarness { view, step_ms: 250 };
        h.run_steps(2);
        h
    }

    /// The window as drawn now (needs [`PhoneHarness::rendered`]).
    pub fn pixels(&self) -> Pixels {
        let w = self.view.window().window();
        let shot = w
            .take_snapshot()
            .expect("a window made by `rendered` can be drawn");
        Pixels {
            width: shot.width(),
            height: shot.height(),
            scale: w.scale_factor(),
            rgba: shot.as_bytes().to_vec(),
        }
    }

    pub fn view(&self) -> &crate::phone::PhoneView<A> {
        &self.view
    }

    /// One step: time passes, the app ticks, the window updates.
    pub fn step(&mut self) {
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(self.step_ms));
        self.view.tick();
        slint::platform::update_timers_and_animations();
        let _ = ElementQuery::from_root(self.view.window())
            .match_descendants()
            .find_all();
        slint::platform::update_timers_and_animations();
    }

    pub fn run_steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Resize the window.
    pub fn set_size(&mut self, width: f32, height: f32) {
        self.view
            .window()
            .window()
            .set_size(slint::LogicalSize::new(width, height));
        self.run_steps(2);
    }

    /// Tap at a point of the window (a finger on the dimmed part of a sheet).
    pub fn click_at(&mut self, x: f32, y: f32) {
        let p = LogicalPosition::new(x, y);
        let win = self.view.window().window();
        win.dispatch_event(WindowEvent::PointerMoved { position: p });
        win.dispatch_event(WindowEvent::PointerPressed {
            position: p,
            button: PointerEventButton::Left,
        });
        win.dispatch_event(WindowEvent::PointerReleased {
            position: p,
            button: PointerEventButton::Left,
        });
        self.run_steps(2);
    }

    /// Press a key while no field has focus (a computer's keyboard).
    pub fn type_key(&mut self, text: &str) {
        let win = self.view.window().window();
        let t: slint::SharedString = text.into();
        win.dispatch_event(WindowEvent::KeyPressed { text: t.clone() });
        win.dispatch_event(WindowEvent::KeyReleased { text: t });
        self.run_steps(1);
    }

    /// Every named, visible element.
    pub fn nodes(&self) -> Vec<PhoneNode> {
        let weak = self.view.window().as_weak();
        let mut seen = std::collections::HashSet::new();
        ElementQuery::from_root(self.view.window())
            .match_descendants()
            .match_predicate(|e| {
                // MoneyKeep's elements (the std-widgets' inner texts are not counted).
                e.accessible_label().is_some_and(|l| !l.is_empty())
                    && e.accessible_id().is_some_and(|i| i.contains('|'))
            })
            .find_all()
            .into_iter()
            .filter(|el| {
                let key = format!(
                    "{:?}/{:?}/{:?}",
                    el.accessible_id(),
                    el.absolute_position(),
                    el.size()
                );
                seen.insert(key)
            })
            .map(|el| {
                let p = el.absolute_position();
                let sz = el.size();
                PhoneNode {
                    label: el
                        .accessible_label()
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    rect: Rect {
                        min: pos2(p.x, p.y),
                        max: pos2(p.x + sz.width, p.y + sz.height),
                    },
                    el,
                    window: weak.clone(),
                }
            })
            .collect()
    }

    pub fn query_all_by_label(&self, label: &str) -> impl Iterator<Item = PhoneNode> {
        self.nodes()
            .into_iter()
            .filter(move |n| n.label == label)
            .collect::<Vec<_>>()
            .into_iter()
    }

    pub fn query_all_by_label_contains(&self, part: &str) -> impl Iterator<Item = PhoneNode> {
        let part = part.to_owned();
        self.nodes()
            .into_iter()
            .filter(move |n| n.label.contains(&part))
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// The one element named `label` (a control wins over a text naming it).
    pub fn query_by_label(&self, label: &str) -> Option<PhoneNode> {
        let mut v: Vec<PhoneNode> = self.query_all_by_label(label).collect();
        if v.len() > 1 {
            v.retain(|n| {
                n.el.accessible_role() != Some(i_slint_backend_testing::AccessibleRole::Text)
            });
        }
        v.into_iter().next()
    }

    #[track_caller]
    pub fn get_by_label(&self, label: &str) -> PhoneNode {
        self.query_by_label(label).unwrap_or_else(|| {
            panic!(
                "No element named {label:?}; names: {:?}",
                self.nodes()
                    .iter()
                    .map(|n| n.label.clone())
                    .collect::<Vec<_>>()
            )
        })
    }
}
