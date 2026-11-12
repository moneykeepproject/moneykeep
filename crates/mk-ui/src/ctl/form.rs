//! Form helpers shared by the business screens: labelled fields, drop-downs and
//! argument building.

use serde_json::{Value, json};

use super::ui::{Resp, Ui};

/// A labelled text field (`hint` as placeholder).
pub(crate) fn text_row(ui: &mut Ui, label: &str, text: &mut String, hint: &str) -> Resp {
    ui.form(label, |ui| {
        ui.width(240);
        if !hint.is_empty() {
            ui.hint(hint);
        }
        ui.field(label, text)
    })
}

/// A labelled date field (revision 20).
pub(crate) fn date_row(ui: &mut Ui, label: &str, text: &mut String) -> Resp {
    ui.form(label, |ui| super::register::date_field(ui, label, text))
}

/// A drop-down over `items`; returns whether the choice changed.
pub(crate) fn choice(ui: &mut Ui, label: &str, selected: &mut usize, items: &[&str]) -> bool {
    let opts: Vec<String> = items.iter().map(|s| (*s).to_owned()).collect();
    ui.width(240);
    ui.combo(label, selected, &opts).changed()
}

/// A drop-down over text options; `none` adds an empty choice with that label.
/// Returns whether the value changed.
pub(crate) fn pick(
    ui: &mut Ui,
    label: &str,
    current: &mut String,
    options: &[String],
    none: Option<&str>,
) -> bool {
    let mut all: Vec<String> = Vec::new();
    if let Some(n) = none {
        all.push(n.to_owned());
    }
    all.extend(options.iter().cloned());
    let offset = usize::from(none.is_some());
    let mut idx = if current.is_empty() {
        if none.is_some() { 0 } else { usize::MAX }
    } else {
        options
            .iter()
            .position(|o| o == current)
            .map_or(usize::MAX, |i| i + offset)
    };
    ui.width(240);
    let resp = ui.combo(label, &mut idx, &all);
    if current.is_empty()
        && none.is_none()
        && let Some(it) = ui.last()
    {
        it.value = "Choose…".into();
    } else if !current.is_empty()
        && idx == usize::MAX
        && let Some(it) = ui.last()
    {
        it.value = current.clone();
    }
    if resp.changed() {
        let before = current.clone();
        *current = if none.is_some() && idx == 0 {
            String::new()
        } else {
            all.get(idx).cloned().unwrap_or_default()
        };
        return *current != before;
    }
    false
}

/// A labelled drop-down row.
pub(crate) fn pick_row(
    ui: &mut Ui,
    label: &str,
    current: &mut String,
    options: &[String],
    none: Option<&str>,
) -> bool {
    ui.form(label, |ui| pick(ui, label, current, options, none))
}

/// Set `key` to the trimmed text when there is any.
pub(crate) fn put(a: &mut Value, key: &str, text: &str) {
    let t = text.trim();
    if !t.is_empty() {
        set(a, key, json!(t));
    }
}

/// Set `key` on a JSON object.
pub(crate) fn set(a: &mut Value, key: &str, v: Value) {
    if let Some(o) = a.as_object_mut() {
        o.insert(key.to_owned(), v);
    }
}

/// A typed amount as a command argument (thousands separators dropped).
pub(crate) fn amount_arg(text: &str) -> String {
    text.trim().replace(',', "")
}

/// A block of colour `w` × `h` pixels (0xRRGGBB).
fn colour_block(ui: &mut Ui, rgb: u32, w: i32, h: i32) {
    ui.push(
        super::ui::Item {
            kind: super::ui::Kind::Separator,
            color: Some(rgb),
            width: w,
            number: h,
            ..super::ui::Item::default()
        },
        false,
    );
}

/// A small colour swatch beside a colour field.
pub(crate) fn swatch(ui: &mut Ui, rgb: u32) {
    colour_block(ui, rgb, 18, 18);
}

/// A thin accent bar `w` pixels wide.
pub(crate) fn bar(ui: &mut Ui, rgb: u32, w: i32) {
    colour_block(ui, rgb, w, 4);
}
