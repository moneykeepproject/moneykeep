//! The floating calculator (app-desktop.md revision 25): a small window over the
//! screens that never touches the book. Keys, display and title bar report to
//! the controller; the arithmetic is `crate::calc`.

use super::{App, CalcVm};
use crate::calc::{Calc, Op};

/// The window's size in logical pixels (for keeping it inside the main window).
pub(crate) const WIDTH: i32 = 248;
pub(crate) const HEIGHT: i32 = 404;
const MARGIN: i32 = 16;

/// The calculator's state.
#[derive(Debug, Default)]
pub(crate) struct CalcState {
    pub open: bool,
    pub pinned: bool,
    pos: Option<(i32, i32)>,
    calc: Calc,
    /// Ask the display to take the keyboard focus (a serial).
    focus: i32,
}

impl CalcState {
    pub(crate) fn from_config(c: &crate::CalculatorConfig) -> Self {
        CalcState {
            open: c.pinned && c.open,
            pinned: c.pinned,
            pos: c.pos.map(|[x, y]| (x, y)),
            ..Self::default()
        }
    }
}

/// The keys: (accessible name, action).
const KEYS: [(&str, Key); 20] = [
    ("clear", Key::Clear),
    ("delete last digit", Key::Backspace),
    ("change sign", Key::Negate),
    ("percent", Key::Percent),
    ("seven", Key::Digit(7)),
    ("eight", Key::Digit(8)),
    ("nine", Key::Digit(9)),
    ("divide", Key::Op(Op::Div)),
    ("four", Key::Digit(4)),
    ("five", Key::Digit(5)),
    ("six", Key::Digit(6)),
    ("multiply", Key::Op(Op::Mul)),
    ("one", Key::Digit(1)),
    ("two", Key::Digit(2)),
    ("three", Key::Digit(3)),
    ("minus", Key::Op(Op::Sub)),
    ("zero", Key::Digit(0)),
    ("point", Key::Point),
    ("equals", Key::Equals),
    ("plus", Key::Op(Op::Add)),
];

#[derive(Debug, Clone, Copy)]
enum Key {
    Digit(u32),
    Point,
    Op(Op),
    Equals,
    Percent,
    Negate,
    Backspace,
    Clear,
}

fn press(c: &mut Calc, k: Key) {
    match k {
        Key::Digit(d) => c.digit(d),
        Key::Point => c.point(),
        Key::Op(op) => c.operation(op),
        Key::Equals => c.equals(),
        Key::Percent => c.percent(),
        Key::Negate => c.negate(),
        Key::Backspace => c.backspace(),
        Key::Clear => c.clear_all(),
    }
}

/// `p` moved so the calculator lies inside a `w`×`h` window (top-left wins).
fn keep_inside(p: (i32, i32), w: i32, h: i32) -> (i32, i32) {
    (
        p.0.clamp(0, (w - WIDTH).max(0)),
        p.1.clamp(0, (h - HEIGHT).max(0)),
    )
}

impl App {
    /// Whether the calculator window is open.
    pub(crate) fn calc_open(&self) -> bool {
        self.calc.open
    }

    /// Whether the calculator is open (tests and the app shell).
    pub fn calculator_open(&self) -> bool {
        self.calc.open
    }

    /// Its remembered position (top-left, logical pixels).
    pub fn calculator_pos(&self) -> Option<(i32, i32)> {
        self.calc.pos
    }

    /// Open or close the calculator; opening gives its display the focus.
    pub(crate) fn toggle_calculator(&mut self) {
        self.calc.open = !self.calc.open;
        if self.calc.open {
            self.calc.focus += 1;
        }
        self.persist_calculator();
    }

    pub(crate) fn reset_calculator(&mut self) {
        self.calc.pos = None;
        self.calc.pinned = false;
        self.config.calculator = crate::CalculatorConfig::default();
    }

    fn persist_calculator(&mut self) {
        let want = crate::CalculatorConfig {
            pos: self.calc.pos.map(|(x, y)| [x, y]),
            pinned: self.calc.pinned,
            open: self.calc.pinned && self.calc.open,
        };
        if want != self.config.calculator {
            self.config.calculator = want;
            self.config_dirty_since.get_or_insert(self.now);
        }
    }

    /// What the calculator window shows.
    pub(crate) fn calc_vm(&self) -> CalcVm {
        let (w, h) = (self.window_width, self.window_height);
        let pos = self
            .calc
            .pos
            .unwrap_or((w - WIDTH - MARGIN, h - HEIGHT - MARGIN));
        let (x, y) = keep_inside(pos, w, h);
        CalcVm {
            open: self.calc.open,
            display: self.calc.calc.display(),
            expression: self.calc.calc.pending(),
            error: self.calc.calc.error().is_some(),
            x,
            y,
            pinned: self.calc.pinned,
            focus: self.calc.focus,
        }
    }

    /// An action on the calculator (`calc/…` ids); returns whether it was one.
    pub(crate) fn calc_act(&mut self, id: &str, value: &str) -> bool {
        let Some(what) = id.strip_prefix("calc/") else {
            return false;
        };
        match what {
            "close" => {
                self.calc.open = false;
                self.persist_calculator();
            }
            "pin" => {
                self.calc.pinned = !self.calc.pinned;
                self.calc.focus += 1;
                self.persist_calculator();
            }
            "copy" => {
                self.copy_result();
                self.calc.focus += 1;
            }
            "move" => {
                if !self.calc.pinned
                    && let Some((x, y)) = value.split_once(',')
                    && let (Ok(x), Ok(y)) = (x.parse::<i32>(), y.parse::<i32>())
                {
                    self.calc.pos =
                        Some(keep_inside((x, y), self.window_width, self.window_height));
                    self.persist_calculator();
                }
            }
            key => {
                if let Some((_, k)) = KEYS.iter().find(|(n, _)| *n == key) {
                    press(&mut self.calc.calc, *k);
                    // Keep typing after a click: the display takes the focus back.
                    self.calc.focus += 1;
                }
            }
        }
        true
    }

    fn copy_result(&mut self) {
        if let Some(t) = self.calc.calc.copy_text() {
            self.copy(t);
        }
    }

    /// A key on the focused calculator display; returns whether it was used.
    pub(crate) fn calc_key(&mut self, name: &str) -> bool {
        let plain = name.strip_prefix("shift+").unwrap_or(name);
        match name {
            "enter" => self.calc.calc.equals(),
            "backspace" => self.calc.calc.backspace(),
            "delete" => self.calc.calc.clear_all(),
            "ctrl+c" => self.copy_result(),
            "escape" => {
                if !self.calc.calc.entry_is_empty() {
                    self.calc.calc.clear_entry();
                } else if !self.calc.pinned {
                    self.calc.open = false;
                    self.persist_calculator();
                }
            }
            _ => {
                let mut chars = plain.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => return self.calc.calc.type_char(c),
                    _ => return false,
                }
            }
        }
        true
    }
}
