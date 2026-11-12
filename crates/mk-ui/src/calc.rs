//! The calculator's arithmetic (app-desktop.md revision 25): a simple calculator
//! over exact decimals that evaluates left to right as entered (no precedence).
//! Pure logic: no UI, no IO, and it never touches a book.

use mk_core::{Decimal, RoundingStrategy};

/// Digits an entry may have (rust_decimal keeps 28 significant digits exactly).
const MAX_DIGITS: usize = 28;
/// Division results are rounded to this many decimal places.
const DIV_PLACES: u32 = 10;

/// An arithmetic operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

impl Op {
    /// The operation's sign as shown in the pending line.
    pub fn symbol(self) -> &'static str {
        match self {
            Op::Add => "+",
            Op::Sub => "−",
            Op::Mul => "×",
            Op::Div => "÷",
        }
    }
}

/// Why a calculation has no result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalcError {
    DivideByZero,
    TooLarge,
}

impl CalcError {
    /// The text shown in the display.
    pub fn message(self) -> &'static str {
        match self {
            CalcError::DivideByZero => "Can't divide by zero",
            CalcError::TooLarge => "Too large",
        }
    }
}

/// The calculator's state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Calc {
    /// The number being typed, as typed ("-12.50"); empty when none.
    entry: String,
    /// The entry was produced by % or ± rather than typed: a digit replaces it.
    entry_computed: bool,
    /// The left operand or the last result.
    acc: Option<Decimal>,
    /// The operation waiting for its right operand.
    op: Option<Op>,
    /// The last operation and right operand, repeated by another `=`.
    last: Option<(Op, Decimal)>,
    error: Option<CalcError>,
    /// `=` was just pressed: a digit starts a new calculation.
    done: bool,
}

/// `a op b`, exactly; division rounded to 10 places, half to even.
pub fn apply(a: Decimal, op: Op, b: Decimal) -> Result<Decimal, CalcError> {
    let r = match op {
        Op::Add => a.checked_add(b),
        Op::Sub => a.checked_sub(b),
        Op::Mul => a.checked_mul(b),
        Op::Div => {
            if b.is_zero() {
                return Err(CalcError::DivideByZero);
            }
            a.checked_div(b).map(|q| {
                q.round_dp_with_strategy(DIV_PLACES, RoundingStrategy::MidpointNearestEven)
            })
        }
    }
    .ok_or(CalcError::TooLarge)?;
    tidy(r)
}

/// Drop trailing zeros and a negative zero's sign; more than 28 integer digits is
/// too large.
fn tidy(d: Decimal) -> Result<Decimal, CalcError> {
    if d.is_zero() {
        return Ok(Decimal::ZERO);
    }
    let limit = Decimal::try_from_i128_with_scale(10_i128.pow(MAX_DIGITS as u32), 0)
        .map_err(|_| CalcError::TooLarge)?;
    if d.abs() >= limit {
        return Err(CalcError::TooLarge);
    }
    Ok(d.normalize())
}

/// A value as plain text, ungrouped: `1250.5`.
pub fn plain(d: Decimal) -> String {
    tidy(d).unwrap_or(d).to_string()
}

impl Calc {
    /// The entry's value (`None` when nothing is typed).
    fn entry_value(&self) -> Option<Decimal> {
        let t = self.entry.trim_end_matches('.');
        if t.is_empty() || t == "-" {
            return None;
        }
        t.parse::<Decimal>().ok()
    }

    /// The number the display shows (entry, else result, else 0).
    pub fn value(&self) -> Option<Decimal> {
        if self.error.is_some() {
            return None;
        }
        Some(self.entry_value().or(self.acc).unwrap_or(Decimal::ZERO))
    }

    /// The display text, grouped with commas: the entry as typed, the result, or
    /// the error.
    pub fn display(&self) -> String {
        if let Some(e) = self.error {
            return e.message().to_owned();
        }
        if !self.entry.is_empty() {
            return crate::fmt::group(&self.entry);
        }
        crate::fmt::group(&plain(self.acc.unwrap_or(Decimal::ZERO)))
    }

    /// The pending operation ("1,250 +"), empty when none.
    pub fn pending(&self) -> String {
        match (self.acc, self.op, self.error) {
            (Some(a), Some(op), None) => {
                format!("{} {}", crate::fmt::group(&plain(a)), op.symbol())
            }
            _ => String::new(),
        }
    }

    /// The displayed value for the clipboard, ungrouped (`None` while an error
    /// shows).
    pub fn copy_text(&self) -> Option<String> {
        self.value().map(plain)
    }

    /// Whether nothing is typed and no error shows (Esc then closes).
    pub fn entry_is_empty(&self) -> bool {
        self.entry.is_empty() && self.error.is_none()
    }

    pub fn error(&self) -> Option<CalcError> {
        self.error
    }

    fn fail(&mut self, e: CalcError) {
        *self = Self {
            error: Some(e),
            ..Self::default()
        };
    }

    /// Start a fresh entry if the last key finished a calculation or computed the
    /// entry.
    fn begin_typing(&mut self) {
        if self.error.is_some() {
            *self = Self::default();
        }
        if self.done {
            self.done = false;
            self.acc = None;
            self.entry.clear();
        }
        if self.entry_computed {
            self.entry_computed = false;
            self.entry.clear();
        }
    }

    /// A digit key, 0–9 (other values are ignored).
    pub fn digit(&mut self, d: u32) {
        let Some(c) = char::from_digit(d, 10) else {
            return;
        };
        self.begin_typing();
        let digits = self.entry.chars().filter(char::is_ascii_digit).count();
        if digits >= MAX_DIGITS {
            return;
        }
        // No leading zeros: "0" then "5" is "5".
        match self.entry.as_str() {
            "0" => self.entry.clear(),
            "-0" => self.entry = "-".to_owned(),
            _ => {}
        }
        self.entry.push(c);
    }

    /// The decimal point.
    pub fn point(&mut self) {
        self.begin_typing();
        if self.entry.contains('.') {
            return;
        }
        if self.entry.is_empty() || self.entry == "-" {
            self.entry.push('0');
        }
        self.entry.push('.');
    }

    /// An operation key: evaluates what is pending (left to right) and waits for
    /// the next operand. Pressing another operation instead changes it.
    pub fn operation(&mut self, op: Op) {
        if self.error.is_some() {
            return;
        }
        if let Some(v) = self.entry_value() {
            let left = match (self.acc, self.op) {
                (Some(a), Some(p)) => apply(a, p, v),
                _ => Ok(v),
            };
            match left {
                Ok(l) => self.acc = Some(l),
                Err(e) => return self.fail(e),
            }
        } else if self.acc.is_none() {
            self.acc = Some(Decimal::ZERO);
        }
        self.entry.clear();
        self.entry_computed = false;
        self.done = false;
        self.op = Some(op);
    }

    /// `=`: finish the pending operation, or repeat the last one.
    pub fn equals(&mut self) {
        if self.error.is_some() {
            return;
        }
        let entry = self.entry_value();
        let step = match (self.acc, self.op) {
            // "5 + =" uses the left operand again.
            (Some(a), Some(op)) => Some((a, op, entry.unwrap_or(a))),
            _ => self.last.map(|(op, b)| {
                let left = entry.or(self.acc).unwrap_or(Decimal::ZERO);
                (left, op, b)
            }),
        };
        match step {
            Some((a, op, b)) => match apply(a, op, b) {
                Ok(r) => {
                    self.acc = Some(r);
                    self.last = Some((op, b));
                }
                Err(e) => return self.fail(e),
            },
            None => {
                if let Some(v) = entry {
                    match tidy(v) {
                        Ok(v) => self.acc = Some(v),
                        Err(e) => return self.fail(e),
                    }
                }
            }
        }
        self.op = None;
        self.entry.clear();
        self.entry_computed = false;
        self.done = true;
    }

    /// `%`: for + and − the entry becomes that percentage of the left operand
    /// (`200 + 10 %` → `200 + 20`); otherwise entry ÷ 100.
    pub fn percent(&mut self) {
        let Some(v) = self.value() else {
            return;
        };
        let r = match (self.acc, self.op) {
            (Some(a), Some(Op::Add | Op::Sub)) => {
                apply(a, Op::Mul, v).and_then(|x| apply(x, Op::Div, Decimal::ONE_HUNDRED))
            }
            _ => apply(v, Op::Div, Decimal::ONE_HUNDRED),
        };
        self.set_computed(r);
    }

    /// `±`: change the sign of the entry (or of the result shown).
    pub fn negate(&mut self) {
        if self.error.is_some() {
            return;
        }
        if self.entry.is_empty() {
            let Some(v) = self.value() else {
                return;
            };
            if self.op.is_some() && !self.done {
                // Nothing typed for the right operand yet: start it as −0.
                self.entry = "-0".to_owned();
                return;
            }
            self.set_computed(Ok(-v));
            return;
        }
        if let Some(rest) = self.entry.strip_prefix('-') {
            self.entry = rest.to_owned();
        } else {
            self.entry.insert(0, '-');
        }
    }

    fn set_computed(&mut self, r: Result<Decimal, CalcError>) {
        match r {
            Ok(v) => {
                if self.done {
                    // A result turned into a new entry: the old one is no longer pending.
                    self.done = false;
                    self.acc = None;
                }
                self.entry = plain(v);
                self.entry_computed = true;
            }
            Err(e) => self.fail(e),
        }
    }

    /// `⌫`: delete the last character typed.
    pub fn backspace(&mut self) {
        if self.error.is_some() {
            self.error = None;
            return;
        }
        self.entry.pop();
        if self.entry == "-" || self.entry == "-0" {
            self.entry.clear();
        }
        self.entry_computed = false;
    }

    /// Clear the entry (and an error), keeping the pending operation.
    pub fn clear_entry(&mut self) {
        self.entry.clear();
        self.entry_computed = false;
        self.error = None;
    }

    /// `C`: clear everything.
    pub fn clear_all(&mut self) {
        *self = Self::default();
    }

    /// A typed character: digits, `.`, `+ - * x /`, `=`, `%`. Returns whether it
    /// meant something.
    pub fn type_char(&mut self, c: char) -> bool {
        match c {
            '0'..='9' => self.digit(c.to_digit(10).unwrap_or(0)),
            '.' | ',' => self.point(),
            '+' => self.operation(Op::Add),
            '-' | '−' => self.operation(Op::Sub),
            '*' | 'x' | 'X' | '×' => self.operation(Op::Mul),
            '/' | '÷' => self.operation(Op::Div),
            '=' => self.equals(),
            '%' => self.percent(),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Feed keys: characters as typed, plus `C` (clear), `<` (backspace) and `~`
    /// (±).
    fn run(keys: &str) -> Calc {
        let mut c = Calc::default();
        for k in keys.chars().filter(|k| !k.is_whitespace()) {
            match k {
                'C' => c.clear_all(),
                '<' => c.backspace(),
                '~' => c.negate(),
                _ => {
                    assert!(c.type_char(k), "{k}");
                }
            }
        }
        c
    }

    #[test]
    fn adds_decimals_exactly() {
        assert_eq!(run("12.5+7.5=").display(), "20");
        assert_eq!(run("0.1+0.2=").display(), "0.3");
    }

    #[test]
    fn evaluates_left_to_right() {
        assert_eq!(run("2+3*4=").display(), "20");
        let c = run("2+3*");
        assert_eq!(c.display(), "5");
        assert_eq!(c.pending(), "5 ×");
    }

    #[test]
    fn division_rounds_to_ten_places() {
        assert_eq!(run("10/3=").display(), "3.3333333333");
        assert_eq!(run("2/3=").display(), "0.6666666667");
        assert_eq!(run("1/8=").display(), "0.125");
    }

    #[test]
    fn divide_by_zero_is_an_error() {
        let c = run("1/0=");
        assert_eq!(c.display(), "Can't divide by zero");
        assert_eq!(c.copy_text(), None);
        // A digit starts afresh.
        assert_eq!(run("1/0=5").display(), "5");
    }

    #[test]
    fn percent_and_repeated_equals() {
        let mut c = run("200+10%");
        assert_eq!(c.display(), "20");
        assert_eq!(c.pending(), "200 +");
        c.equals();
        assert_eq!(c.display(), "220");
        c.equals();
        assert_eq!(c.display(), "240");
        assert_eq!(run("200*10%=").display(), "20");
        assert_eq!(run("50%").display(), "0.5");
        assert_eq!(run("200-10%=").display(), "180");
    }

    #[test]
    fn repeat_equals_after_new_entry() {
        assert_eq!(run("2*3==").display(), "18");
        assert_eq!(run("2*3=5=").display(), "15");
        assert_eq!(run("5+=").display(), "10");
    }

    #[test]
    fn display_groups_and_keeps_the_entry_as_typed() {
        let c = run("1250.00");
        assert_eq!(c.display(), "1,250.00");
        assert_eq!(c.copy_text().as_deref(), Some("1250"));
        let c = run("1250.5+");
        assert_eq!(c.pending(), "1,250.5 +");
        assert_eq!(c.copy_text().as_deref(), Some("1250.5"));
        assert_eq!(run("1234567*1000=").display(), "1,234,567,000");
    }

    #[test]
    fn editing_keys() {
        assert_eq!(run("123<").display(), "12");
        assert_eq!(run("1<<").display(), "0");
        assert_eq!(run("12~").display(), "-12");
        assert_eq!(run("12~~").display(), "12");
        assert_eq!(run("12+3=~").display(), "-15");
        assert_eq!(run("10-~4=").display(), "14");
        assert_eq!(run("12+3C").display(), "0");
        assert_eq!(run("007").display(), "7");
        assert_eq!(run(".5").display(), "0.5");
        assert_eq!(run("1..5").display(), "1.5");
        let mut c = run("12+3");
        c.clear_entry();
        assert!(c.entry_is_empty());
        assert_eq!(c.pending(), "12 +");
        c.type_char('4');
        c.equals();
        assert_eq!(c.display(), "16");
    }

    #[test]
    fn digit_after_result_starts_a_new_calculation() {
        assert_eq!(run("2+3=7").display(), "7");
        assert_eq!(run("2+3=+1=").display(), "6");
    }

    #[test]
    fn too_large() {
        let nines = "9".repeat(28);
        let c = run(&format!("{nines}*{nines}="));
        assert_eq!(c.display(), "Too large");
        let c = run(&format!("{nines}+1="));
        assert_eq!(c.display(), "Too large");
        // Typing stops at 28 digits.
        let c = run(&format!("{nines}5"));
        assert_eq!(c.copy_text().unwrap(), nines);
    }

    #[test]
    fn changing_the_operation() {
        assert_eq!(run("8+-2=").display(), "6");
    }
}
