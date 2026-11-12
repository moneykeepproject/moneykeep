//! Small helpers for the controller: JSON access, pictures, tile colours.

use std::rc::Rc;

use serde_json::Value;

use super::ui::Picture;

/// Lenient access to command results.
pub(crate) trait Json {
    fn at(&self, key: &str) -> &Value;
    fn str(&self, key: &str) -> &str;
    fn list(&self, key: &str) -> &[Value];
}

static NULL: Value = Value::Null;

impl Json for Value {
    fn at(&self, key: &str) -> &Value {
        self.get(key).unwrap_or(&NULL)
    }
    fn str(&self, key: &str) -> &str {
        self.get(key).and_then(Value::as_str).unwrap_or("")
    }
    fn list(&self, key: &str) -> &[Value] {
        self.get(key)
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }
}

/// A command result's array (empty when it is not one).
pub(crate) fn array(v: Option<Value>) -> Vec<Value> {
    v.and_then(|v| v.as_array().cloned()).unwrap_or_default()
}

/// Decode a PNG (8-bit, up to `max_side` pixels a side) into an RGBA picture.
pub(crate) fn png_picture(bytes: &[u8], max_side: u32) -> Option<Rc<Picture>> {
    let side = usize::try_from(max_side).ok()?;
    let limit = side.saturating_mul(side).saturating_mul(8).max(4 << 20);
    let mut d =
        png::Decoder::new_with_limits(std::io::Cursor::new(bytes), png::Limits { bytes: limit });
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = d.read_info().ok()?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w == 0 || h == 0 || w > max_side || h > max_side {
        return None;
    }
    let mut buf = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buf).ok()?;
    let (wu, hu) = (usize::try_from(w).ok()?, usize::try_from(h).ok()?);
    let channels = match frame.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return None,
    };
    let mut rgba = Vec::with_capacity(wu.checked_mul(hu)?.checked_mul(4)?);
    for row in 0..hu {
        let start = row.checked_mul(frame.line_size)?;
        let line = buf.get(start..start.checked_add(frame.line_size)?)?;
        for px in line.chunks_exact(channels).take(wu) {
            match *px {
                [g] => rgba.extend_from_slice(&[g, g, g, 255]),
                [g, a] => rgba.extend_from_slice(&[g, g, g, a]),
                [r, g, b] => rgba.extend_from_slice(&[r, g, b, 255]),
                [r, g, b, a] => rgba.extend_from_slice(&[r, g, b, a]),
                _ => return None,
            }
        }
    }
    Some(Rc::new(Picture {
        width: w,
        height: h,
        rgba,
    }))
}

/// A tile colour's background (0xRRGGBB); text on it is white except on amber.
pub(crate) fn tile_color(name: &str) -> u32 {
    match name {
        "blue" => 0x1D4ED8,
        "indigo" => 0x4338CA,
        "purple" => 0x7E22CE,
        "rose" => 0xBE123C,
        "red" => 0xB91C1C,
        "orange" => 0xC2410C,
        "amber" => 0xFBBF24,
        "green" => 0x15803D,
        "slate" => 0x475569,
        _ => 0x0F766E,
    }
}

/// "progress-invoicing" → "Progress invoicing".
pub(crate) fn display_name(module: &str) -> String {
    let s = module.replace('-', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn module_names_read_as_words() {
        assert_eq!(display_name("progress-invoicing"), "Progress invoicing");
        assert_eq!(display_name(""), "");
    }

    #[test]
    fn json_access_is_lenient() {
        let v = serde_json::json!({"a": "x", "l": [1]});
        assert_eq!(v.str("a"), "x");
        assert_eq!(v.str("b"), "");
        assert_eq!(v.list("l").len(), 1);
        assert!(v.at("zz").is_null());
    }
}
