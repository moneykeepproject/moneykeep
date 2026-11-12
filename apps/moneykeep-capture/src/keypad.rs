//! The amount keypad's arithmetic (mobile-capture.md revision 7): an expression of
//! numbers and + − × ÷ typed key by key, worked out with × and ÷ before + and −,
//! left to right. Exact: numbers are fixed-point integers with ten decimal places
//! (no floating point), and every step is checked for overflow. Pure: no UI, no IO.

/// Decimal places kept while working an expression out (division rounds to these).
const PLACES: u32 = 10;
const SCALE: i128 = 10_000_000_000;
/// Decimals a typed number may have.
const TYPED_DECIMALS: usize = 2;
/// The longest expression kept (keys past it are ignored).
const MAX_LEN: usize = 40;

/// The message shown when Done has no usable amount.
pub const MORE_THAN_ZERO: &str = "Amount must be more than 0";

/// A key of the keypad (or of a computer's keyboard while the keypad is open).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Digit(u8),
    Point,
    /// `+`, `-`, `*` or `/`.
    Op(char),
    Backspace,
    Clear,
    Equals,
}

/// Why an expression has no value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalError {
    DivideByZero,
    TooLarge,
}

/// A fixed-point number (`SCALE` units per 1).
type Fixed = i128;

fn is_op(c: char) -> bool {
    matches!(c, '+' | '-' | '*' | '/')
}

/// The number being typed at the end of `expr` (after the last operator).
fn last_number(expr: &str) -> &str {
    match expr.rfind(is_op) {
        Some(i) => expr.get(i + 1..).unwrap_or(""),
        None => expr,
    }
}

/// `expr` after `key` (calculator rules: at most two decimals typed per number, a
/// second "." in one number is ignored, an operator after an operator replaces it,
/// ⌫ removes the last character, Clear sets 0, = replaces the expression with its
/// result). An expression that has no value is kept as it is by =.
pub fn press(expr: &str, key: Key) -> String {
    let mut e = expr.to_owned();
    match key {
        Key::Digit(d) => {
            let d = char::from(b'0' + d.min(9));
            let num = last_number(&e);
            if let Some((_, frac)) = num.split_once('.')
                && frac.len() >= TYPED_DECIMALS
            {
                return e;
            }
            if num == "0" {
                // A lone 0 is replaced by the digit typed.
                e.pop();
            }
            if e.len() < MAX_LEN {
                e.push(d);
            }
        }
        Key::Point => {
            let num = last_number(&e);
            if num.contains('.') || e.len() >= MAX_LEN {
                return e;
            }
            if num.is_empty() {
                e.push('0');
            }
            e.push('.');
        }
        Key::Op(op) => {
            if !is_op(op) {
                return e;
            }
            if e.is_empty() {
                e.push('0');
            }
            if e.ends_with(is_op) {
                e.pop();
            }
            if e.len() < MAX_LEN {
                e.push(op);
            }
        }
        Key::Backspace => {
            e.pop();
            if e.is_empty() {
                e.push('0');
            }
        }
        Key::Clear => e = "0".to_owned(),
        Key::Equals => {
            if let Ok(v) = evaluate(&e) {
                e = plain(round2(v));
            }
        }
    }
    e
}

/// `expr` with `n` added to its value (the quick-add row); unchanged when it has
/// no value.
pub fn add(expr: &str, n: i64) -> String {
    match evaluate(expr).and_then(|v| {
        i128::from(n)
            .checked_mul(SCALE)
            .and_then(|a| v.checked_add(a))
            .ok_or(EvalError::TooLarge)
    }) {
        Ok(v) => plain(round2(v)),
        Err(_) => expr.to_owned(),
    }
}

/// Parse one typed number ("12.50", "3.", ".5", "").
fn number(s: &str) -> Result<Fixed, EvalError> {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut v: Fixed = 0;
    for c in int.chars() {
        let d = c.to_digit(10).ok_or(EvalError::TooLarge)?;
        v = v
            .checked_mul(10)
            .and_then(|v| v.checked_add(i128::from(d)))
            .ok_or(EvalError::TooLarge)?;
    }
    v = v.checked_mul(SCALE).ok_or(EvalError::TooLarge)?;
    let mut unit = SCALE;
    for c in frac.chars().take(PLACES as usize) {
        let d = c.to_digit(10).ok_or(EvalError::TooLarge)?;
        unit /= 10;
        v = v
            .checked_add(i128::from(d) * unit)
            .ok_or(EvalError::TooLarge)?;
    }
    Ok(v)
}

/// `a / b` rounded half away from zero to `places` decimals of `SCALE` units.
fn div_round(a: Fixed, b: Fixed) -> Result<Fixed, EvalError> {
    if b == 0 {
        return Err(EvalError::DivideByZero);
    }
    let q = a / b;
    let r = a % b;
    // |2r| >= |b| rounds away from zero.
    let twice = r.checked_mul(2).ok_or(EvalError::TooLarge)?.abs();
    if twice >= b.abs() {
        let away = if (a < 0) == (b < 0) { 1 } else { -1 };
        q.checked_add(away).ok_or(EvalError::TooLarge)
    } else {
        Ok(q)
    }
}

fn mul(a: Fixed, b: Fixed) -> Result<Fixed, EvalError> {
    div_round(a.checked_mul(b).ok_or(EvalError::TooLarge)?, SCALE)
}

fn div(a: Fixed, b: Fixed) -> Result<Fixed, EvalError> {
    if b == 0 {
        return Err(EvalError::DivideByZero);
    }
    div_round(a.checked_mul(SCALE).ok_or(EvalError::TooLarge)?, b)
}

/// Work `expr` out: × and ÷ before + and −, left to right. An operator at the
/// end is ignored; an empty expression is 0.
pub fn evaluate(expr: &str) -> Result<Fixed, EvalError> {
    let e = expr.trim_end_matches(is_op);
    // Terms joined by + and −; each term is factors joined by × and ÷.
    let mut total: Fixed = 0;
    let mut term: Option<Fixed> = None;
    let mut add_op = '+';
    let mut mul_op = '*';
    let mut start = 0;
    let chars: Vec<(usize, char)> = e.char_indices().collect();
    let mut flush = |slice: &str, next: char, total: &mut Fixed| -> Result<(), EvalError> {
        let n = number(slice)?;
        let t = match term {
            None => n,
            Some(t) if mul_op == '*' => mul(t, n)?,
            Some(t) => div(t, n)?,
        };
        if next == '*' || next == '/' {
            term = Some(t);
            mul_op = next;
        } else {
            *total = if add_op == '+' {
                total.checked_add(t)
            } else {
                total.checked_sub(t)
            }
            .ok_or(EvalError::TooLarge)?;
            term = None;
            add_op = next;
        }
        Ok(())
    };
    for &(i, c) in &chars {
        if is_op(c) {
            flush(e.get(start..i).unwrap_or(""), c, &mut total)?;
            start = i + c.len_utf8();
        }
    }
    flush(e.get(start..).unwrap_or(""), '+', &mut total)?;
    Ok(total)
}

/// Rounded to 2 decimals, half away from zero.
pub fn round2(v: Fixed) -> Fixed {
    let unit = SCALE / 100;
    div_round(v, unit).map_or(v, |q| q.saturating_mul(unit))
}

/// A number without trailing zeros ("16.75", "14", "-3.5").
pub fn plain(v: Fixed) -> String {
    let neg = v < 0;
    let a = v.unsigned_abs();
    let scale = SCALE.unsigned_abs();
    let int = a / scale;
    let frac = a % scale;
    let mut s = if neg {
        format!("-{int}")
    } else {
        int.to_string()
    };
    if frac > 0 {
        let f = format!("{frac:0width$}", width = PLACES as usize);
        s.push('.');
        s.push_str(f.trim_end_matches('0'));
    }
    s
}

/// With exactly 2 decimals ("16.75", "20.00") — the Amount.
pub fn money(v: Fixed) -> String {
    let r = round2(v);
    let neg = r < 0;
    let a = r.unsigned_abs();
    let unit = (SCALE / 100).unsigned_abs();
    let cents = a / unit;
    format!(
        "{}{}.{:02}",
        if neg { "-" } else { "" },
        cents / 100,
        cents % 100
    )
}

/// What Done puts in the Amount: the expression worked out and rounded to 2
/// decimals, or the message when that is 0 or less or has no value.
pub fn done(expr: &str) -> Result<String, &'static str> {
    match evaluate(expr) {
        Ok(v) if round2(v) > 0 => Ok(money(v)),
        _ => Err(MORE_THAN_ZERO),
    }
}

/// The message when Done has no usable miles (mobile-capture.md revision 11).
pub const MILES_MORE_THAN_ZERO: &str = "Miles must be more than 0";
/// The message when an odometer reading is below zero or has no value.
pub const READING_NOT_BELOW_ZERO: &str = "The reading can't be below 0";

/// Rounded to 1 decimal, half away from zero.
fn round1(v: Fixed) -> Fixed {
    let unit = SCALE / 10;
    div_round(v, unit).map_or(v, |q| q.saturating_mul(unit))
}

/// With exactly 1 decimal ("12.4", "25.0") — miles and odometer readings.
pub fn tenths(v: Fixed) -> String {
    let r = round1(v);
    let neg = r < 0;
    let a = r.unsigned_abs();
    let unit = (SCALE / 10).unsigned_abs();
    let t = a / unit;
    format!("{}{}.{}", if neg { "-" } else { "" }, t / 10, t % 10)
}

/// What Done puts in Miles (more than 0) or an odometer reading (`reading`: 0 or
/// more), rounded to 1 decimal, or the message (the sheet stays open).
pub fn done_tenths(expr: &str, reading: bool) -> Result<String, &'static str> {
    match evaluate(expr) {
        Ok(v) if reading && round1(v) >= 0 => Ok(tenths(v)),
        Ok(v) if !reading && round1(v) > 0 => Ok(tenths(v)),
        _ if reading => Err(READING_NOT_BELOW_ZERO),
        _ => Err(MILES_MORE_THAN_ZERO),
    }
}

/// The result line for miles: "= 12.4".
pub fn result_line_tenths(expr: &str) -> String {
    match evaluate(expr) {
        Ok(v) => format!("= {}", tenths(v)),
        Err(EvalError::DivideByZero) => "Can't divide by zero".to_owned(),
        Err(EvalError::TooLarge) => "Too large".to_owned(),
    }
}

/// The expression as shown: "12.50 + 4.25" (× ÷ − for * / -).
pub fn show(expr: &str) -> String {
    let mut out = String::new();
    for c in expr.chars() {
        match c {
            '+' => out.push_str(" + "),
            '-' => out.push_str(" − "),
            '*' => out.push_str(" × "),
            '/' => out.push_str(" ÷ "),
            c => out.push(c),
        }
    }
    out.trim_end().to_owned()
}

/// The result line: "= 16.75", or empty when the expression has no value.
pub fn result_line(expr: &str) -> String {
    match evaluate(expr) {
        Ok(v) => format!("= {}", money(v)),
        Err(EvalError::DivideByZero) => "Can't divide by zero".to_owned(),
        Err(EvalError::TooLarge) => "Too large".to_owned(),
    }
}

/// The expression to start from for an amount already set ("16.75", "20").
pub fn start(amount: &str) -> String {
    let a = amount.trim().replace(',', ".");
    let ok = !a.is_empty()
        && a.chars().all(|c| c.is_ascii_digit() || c == '.')
        && a.matches('.').count() <= 1;
    match ok.then(|| number(&a)).and_then(Result::ok) {
        Some(v) if v > 0 => plain(v),
        _ => "0".to_owned(),
    }
}

/// A key typed on a computer's keyboard ("1", ".", "shift++", "backspace").
pub fn from_key_name(name: &str) -> Option<Key> {
    let n = name.strip_prefix("shift+").unwrap_or(name);
    Some(match n {
        "backspace" => Key::Backspace,
        "=" => Key::Equals,
        "." | "," => Key::Point,
        "+" | "-" | "*" | "/" => Key::Op(n.chars().next()?),
        _ => {
            let mut cs = n.chars();
            let c = cs.next()?;
            if cs.next().is_some() {
                return None;
            }
            let d = c.to_digit(10)?;
            Key::Digit(u8::try_from(d).ok()?)
        }
    })
}

#[cfg(test)]
mod tests_tenths {
    use super::*;

    /// Revision 11: the keypad for miles and odometer readings.
    #[test]
    fn miles_and_readings() {
        assert_eq!(done_tenths("12.4", false), Ok("12.4".into()));
        assert_eq!(done_tenths("12.45", false), Ok("12.5".into()));
        assert_eq!(done_tenths("10/3", false), Ok("3.3".into()));
        assert_eq!(done_tenths("0", false), Err(MILES_MORE_THAN_ZERO));
        assert_eq!(done_tenths("0", true), Ok("0.0".into()));
        assert_eq!(done_tenths("1-2", true), Err(READING_NOT_BELOW_ZERO));
        assert_eq!(result_line_tenths("12.4+0.6"), "= 13.0");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(s: &str) -> String {
        let mut e = "0".to_owned();
        for k in s.split_whitespace() {
            let key = match k {
                "." => Key::Point,
                "+" => Key::Op('+'),
                "−" | "-" => Key::Op('-'),
                "×" => Key::Op('*'),
                "÷" => Key::Op('/'),
                "⌫" => Key::Backspace,
                "C" => Key::Clear,
                "=" => Key::Equals,
                d => Key::Digit(d.parse().unwrap_or(0)),
            };
            e = press(&e, key);
        }
        e
    }

    /// mobile-capture.md revision 7, test 51 (the arithmetic).
    #[test]
    fn calculator_rules() {
        let e = keys("1 2 . 5 0 + 4 . 2 5");
        assert_eq!(show(&e), "12.50 + 4.25");
        assert_eq!(result_line(&e), "= 16.75");
        assert_eq!(done(&e), Ok("16.75".into()));
        assert_eq!(done(&keys("1 0 ÷ 3")), Ok("3.33".into()));
        assert_eq!(done(&keys("2 ÷ 3")), Ok("0.67".into()));
        assert_eq!(keys("2 + 3 × 4 ="), "14");
        assert_eq!(keys("2 × 3 + 4 × 5 − 6 ÷ 2 ="), "23");
        assert_eq!(keys("1 0 − 2 − 3 ="), "5", "left to right");
        assert_eq!(keys("8 ÷ 4 ÷ 2 ="), "1", "left to right");
        // A third decimal digit is ignored; a second point too.
        assert_eq!(keys("1 . 2 3 4"), "1.23");
        assert_eq!(keys("1 . 2 . 3"), "1.23");
        // ⌫ and Clear.
        assert_eq!(keys("1 2 3 ⌫"), "12");
        assert_eq!(keys("5 ⌫"), "0");
        assert_eq!(keys("1 2 + 3 C"), "0");
        // A lone 0 is replaced; an operator after an operator replaces it.
        assert_eq!(keys("0 7"), "7");
        assert_eq!(keys("4 + × 2 ="), "8");
        assert_eq!(keys(". 5"), "0.5");
        // An operator at the end is ignored.
        assert_eq!(evaluate("12+").map(plain), Ok("12".into()));
        // Quick-add.
        assert_eq!(add("0", 20), "20");
        assert_eq!(add("12.5+1", 5), "18.5");
    }

    /// Test 52 (the arithmetic): 0, a negative result and a division by zero
    /// give no amount; half away from zero.
    #[test]
    fn done_refuses_zero_negative_and_division_by_zero() {
        assert_eq!(done("0"), Err(MORE_THAN_ZERO));
        assert_eq!(done(&keys("2 − 5")), Err(MORE_THAN_ZERO));
        assert_eq!(done(&keys("5 ÷ 0")), Err(MORE_THAN_ZERO));
        assert_eq!(done("0.004"), Err(MORE_THAN_ZERO));
        assert_eq!(done("0.005"), Ok("0.01".into()));
        assert_eq!(done("1.125*1"), Ok("1.13".into()));
        assert_eq!(money(-1_250_000_000), "-0.13");
        assert_eq!(result_line("5/0"), "Can't divide by zero");
        // Overflow is an error, not a panic.
        assert!(evaluate("99999999999999999999*99999999999999999999").is_err());
    }

    #[test]
    fn start_and_keyboard_names() {
        assert_eq!(start("16.75"), "16.75");
        assert_eq!(start("20.00"), "20");
        assert_eq!(start("4,50"), "4.5");
        assert_eq!(start(""), "0");
        assert_eq!(start("abc"), "0");
        assert_eq!(from_key_name("7"), Some(Key::Digit(7)));
        assert_eq!(from_key_name("shift++"), Some(Key::Op('+')));
        assert_eq!(from_key_name("shift+*"), Some(Key::Op('*')));
        assert_eq!(from_key_name("/"), Some(Key::Op('/')));
        assert_eq!(from_key_name("."), Some(Key::Point));
        assert_eq!(from_key_name("backspace"), Some(Key::Backspace));
        assert_eq!(from_key_name("a"), None);
        assert_eq!(from_key_name("f1"), None);
    }
}
