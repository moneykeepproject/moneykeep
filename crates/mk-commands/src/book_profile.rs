//! The book profile (app-desktop.md revisions 26 and 40): name, tagline,
//! description, icon (letter, symbol, preset picture or a stored picture) and colour.
//! The name lives in the book record; the rest are book settings, so every change is
//! audited and undone by the history commands like any other setting.
//!
//! Revision 40 adds the engraving treatment (`book.set_picture`,
//! `book.preview_picture`): the book keeps the 256 × 256 result, the 512 × 512
//! cropped source and the slider values, so the picture can be edited again.

use base64::Engine;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::builtin::NoArgs;
use crate::{CommandError, CommandResult, Ctx, Registry};

/// The settings module holding the profile (the book's own settings, never a
/// feature module, so `setting.set` cannot reach them).
pub(crate) const MODULE: &str = "book";
const DESCRIPTION_KEY: &str = "book.description";
const ICON_KEY: &str = "book.icon";
const IMAGE_KEY: &str = "book.icon_image";
const COLOR_KEY: &str = "book.color";
const TAGLINE_KEY: &str = "book.tagline";
/// The cropped source of an engraved picture (512 × 512 PNG) and its settings.
const SOURCE_KEY: &str = "book.picture_source";
const PICTURE_SETTINGS_KEY: &str = "book.picture_settings";

/// Longest book name and description, in characters.
pub(crate) const MAX_NAME: usize = 80;
pub(crate) const MAX_DESCRIPTION: usize = 200;
/// Longest tagline, in characters (revision 40).
pub(crate) const MAX_TAGLINE: usize = 60;
/// Largest picture file accepted for an icon.
pub(crate) const MAX_IMAGE_BYTES: usize = 2 * 1024 * 1024;
/// Stored icon pictures are this many pixels square (256 since revision 40).
pub(crate) const IMAGE_SIDE: u32 = mk_import::engrave::RESULT_SIDE;

const IMAGE_REFUSED: &str = "Choose a PNG or JPEG picture up to 2 MB.";

/// The symbols an icon can show (MoneyKeep's own drawings).
pub const SYMBOLS: [&str; 16] = [
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

/// The preset pictures (revision 40, the owner's list), drawn by the app.
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

/// The tile colours, in the order the default colour indexes them.
pub const COLORS: [&str; 10] = [
    "teal", "blue", "indigo", "purple", "rose", "red", "orange", "amber", "green", "slate",
];

pub(crate) fn register(r: &mut Registry) {
    r.add(
        "book.update_profile",
        "Change the book's name, tagline, description, icon (letter, symbol or preset picture) or colour; only the given fields change, and renaming never renames the file",
        true,
        update_profile,
    );
    r.add(
        "book.set_icon_image",
        "Use a finished picture (PNG or JPEG, at most 2 MB) as the book's icon without the engraving treatment; it is cropped to a square and stored at 256 × 256",
        true,
        set_icon_image,
    );
    r.add(
        "book.clear_icon_image",
        "Remove the book's picture (with its source and settings); the icon returns to the default letter",
        true,
        clear_icon_image,
    );
    r.add(
        "book.set_picture",
        "Give a photo (PNG or JPEG, at most 2 MB) the engraving treatment and use it as the book's picture; stores the 256 × 256 result, the cropped 512 × 512 source and the settings",
        true,
        set_picture,
    );
    r.add(
        "book.preview_picture",
        "Preview the engraving treatment of a photo without changing the book: returns the treated picture and the cropped photo as PNGs",
        false,
        preview_picture,
    );
    r.add(
        "book.picture_source",
        "The cropped source photo and the settings of the book's engraved picture, for editing it again (empty when the picture has none)",
        false,
        picture_source,
    );
}

fn refused(message: impl Into<String>) -> CommandError {
    CommandError::Coded {
        code: "invalid_args",
        message: message.into(),
    }
}

/// The default icon letter: the name's first character, upper-cased.
fn default_letter(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_owned())
}

/// The default colour: index (sum of the name's character codes) mod 10.
pub(crate) fn default_color(name: &str) -> &'static str {
    let sum = name
        .chars()
        .fold(0u64, |acc, c| acc.wrapping_add(u64::from(u32::from(c))));
    let i = usize::try_from(sum % 10).unwrap_or(0);
    COLORS.get(i).copied().unwrap_or("teal")
}

/// The profile fields `book.info` adds to the book record.
pub(crate) fn profile_json(ctx: &Ctx<'_>, name: &str) -> CommandResult<Value> {
    let description = ctx
        .tx
        .get_setting(MODULE, DESCRIPTION_KEY)?
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let color = ctx
        .tx
        .get_setting(MODULE, COLOR_KEY)?
        .and_then(|v| v.as_str().map(str::to_owned))
        .filter(|c| COLORS.contains(&c.as_str()))
        .unwrap_or_else(|| default_color(name).to_owned());
    let stored = ctx.tx.get_setting(MODULE, ICON_KEY)?;
    let image = ctx
        .tx
        .get_setting(MODULE, IMAGE_KEY)?
        .and_then(|v| v.as_str().map(str::to_owned));
    let tagline = ctx
        .tx
        .get_setting(MODULE, TAGLINE_KEY)?
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let editable = ctx.tx.get_setting(MODULE, SOURCE_KEY)?.is_some();
    let icon = match stored.as_ref().map(|v| (v["kind"].as_str(), v)) {
        Some((Some("symbol"), v)) if v["symbol"].as_str().is_some() => {
            json!({"kind": "symbol", "symbol": v["symbol"]})
        }
        Some((Some("preset"), v)) if v["preset"].as_str().is_some() => {
            json!({"kind": "preset", "preset": v["preset"]})
        }
        Some((Some("letter"), v)) if v["text"].as_str().is_some() => {
            json!({"kind": "letter", "text": v["text"]})
        }
        Some((Some("image"), _)) if image.is_some() => {
            json!({"kind": "image", "image_png_base64": image, "editable": editable})
        }
        _ => json!({"kind": "letter", "text": default_letter(name)}),
    };
    Ok(json!({"description": description, "tagline": tagline, "icon": icon, "color": color}))
}

/// Clean user text (input-safety.md R3: control, bidirectional and invisible
/// characters removed, line breaks become spaces, NFC, trimmed) without cutting it.
fn clean(text: &str) -> String {
    mk_core::text::clean(text, usize::MAX).text
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IconArg {
    /// `letter`, `symbol` or `preset` (pictures are set with `book.set_picture` or
    /// `book.set_icon_image`).
    kind: String,
    /// For `letter`: 1–2 characters, shown upper-cased.
    #[serde(default)]
    text: Option<String>,
    /// For `symbol`: one of house, church, briefcase, shop, building, piggy-bank,
    /// wallet, chart, heart, star, leaf, car, tools, graduation-cap, people, paw.
    #[serde(default)]
    symbol: Option<String>,
    /// For `preset`: one of farmer, pastor, teacher, nurse, doctor, carpenter,
    /// builder, mechanic, shopkeeper, baker, cook, bookkeeper, lawyer, artist,
    /// musician, gardener, fisherman, seamstress, scholar, student, family, couple,
    /// grandparents, piggy-bank.
    #[serde(default)]
    preset: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProfileArgs {
    /// The book's name: 1–80 characters.
    #[serde(default)]
    name: Option<String>,
    /// Plain text, at most 200 characters; line breaks become spaces.
    #[serde(default)]
    description: Option<String>,
    /// Plain text, one line, at most 60 characters.
    #[serde(default)]
    tagline: Option<String>,
    #[serde(default)]
    icon: Option<IconArg>,
    /// One of teal, blue, indigo, purple, rose, red, orange, amber, green, slate.
    #[serde(default)]
    color: Option<String>,
}

fn validated_icon(icon: &IconArg) -> CommandResult<Value> {
    match icon.kind.as_str() {
        "letter" => {
            let text = clean(icon.text.as_deref().unwrap_or_default());
            let n = text.chars().count();
            if n == 0 {
                return Err(refused("Type a letter for the icon."));
            }
            if n > 2 {
                return Err(refused("The icon letter can be at most 2 characters."));
            }
            Ok(json!({"kind": "letter", "text": text.to_uppercase()}))
        }
        "symbol" => {
            let symbol = icon.symbol.as_deref().unwrap_or_default().trim();
            if !SYMBOLS.contains(&symbol) {
                return Err(refused(format!(
                    "Unknown symbol {symbol:?}; choose one of {}.",
                    SYMBOLS.join(", ")
                )));
            }
            Ok(json!({"kind": "symbol", "symbol": symbol}))
        }
        "preset" => {
            let preset = icon.preset.as_deref().unwrap_or_default().trim();
            if !PRESETS.contains(&preset) {
                return Err(refused(format!(
                    "Unknown preset picture {preset:?}; choose one of {}.",
                    PRESETS.join(", ")
                )));
            }
            Ok(json!({"kind": "preset", "preset": preset}))
        }
        "image" => Err(refused(
            "Set a picture with book.set_picture or book.set_icon_image; the icon kind here is letter, symbol or preset.",
        )),
        other => Err(refused(format!(
            "Unknown icon kind {other:?}; use letter, symbol or preset."
        ))),
    }
}

fn update_profile(ctx: &Ctx<'_>, a: ProfileArgs) -> CommandResult<Value> {
    // Validate every given field before changing anything.
    let name = match &a.name {
        Some(n) => {
            let n = clean(n);
            let count = n.chars().count();
            if count == 0 {
                return Err(refused("Give the book a name."));
            }
            if count > MAX_NAME {
                return Err(refused(format!(
                    "The book name can be at most {MAX_NAME} characters."
                )));
            }
            Some(n)
        }
        None => None,
    };
    let description = match &a.description {
        Some(d) => {
            let d = clean(d);
            if d.chars().count() > MAX_DESCRIPTION {
                return Err(refused(format!(
                    "The description can be at most {MAX_DESCRIPTION} characters."
                )));
            }
            Some(d)
        }
        None => None,
    };
    let tagline = match &a.tagline {
        Some(t) => {
            let t = clean(t);
            let count = t.chars().count();
            if count > MAX_TAGLINE {
                return Err(refused(format!(
                    "The tagline can be at most {MAX_TAGLINE} characters ({count} now)."
                )));
            }
            Some(t)
        }
        None => None,
    };
    let icon = a.icon.as_ref().map(validated_icon).transpose()?;
    let color = match &a.color {
        Some(c) => {
            let c = c.trim();
            if !COLORS.contains(&c) {
                return Err(refused(format!(
                    "Unknown colour {c:?}; choose one of {}.",
                    COLORS.join(", ")
                )));
            }
            Some(c.to_owned())
        }
        None => None,
    };
    if let Some(n) = &name {
        ctx.save_book_name(n)?;
    }
    if let Some(d) = &description {
        let v = (!d.is_empty()).then(|| json!(d));
        ctx.save_setting(MODULE, DESCRIPTION_KEY, v.as_ref())?;
    }
    if let Some(t) = &tagline {
        let v = (!t.is_empty()).then(|| json!(t));
        ctx.save_setting(MODULE, TAGLINE_KEY, v.as_ref())?;
    }
    if let Some(i) = &icon {
        ctx.save_setting(MODULE, ICON_KEY, Some(i))?;
        clear_picture(ctx)?;
    }
    if let Some(c) = &color {
        ctx.save_setting(MODULE, COLOR_KEY, Some(&json!(c)))?;
    }
    ctx.quiet_noop.set(true);
    info_json(ctx)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ImageArgs {
    /// The PNG or JPEG file, base64 (at most 2 MB).
    content_base64: String,
}

/// Decode, centre-crop, scale and re-encode an icon picture.
pub(crate) fn icon_png(bytes: &[u8]) -> CommandResult<Vec<u8>> {
    let rgb =
        mk_import::image::decode_image(bytes, MAX_IMAGE_BYTES, mk_import::image::MAX_ICON_SIDE)
            .map_err(|_| refused(IMAGE_REFUSED))?;
    let square =
        mk_import::image::square_thumbnail(&rgb, IMAGE_SIDE).map_err(|_| refused(IMAGE_REFUSED))?;
    mk_import::image::encode_png(&square).map_err(|_| refused(IMAGE_REFUSED))
}

fn set_icon_image(ctx: &Ctx<'_>, a: ImageArgs) -> CommandResult<Value> {
    let b64 = base64::engine::general_purpose::STANDARD;
    // Base64 of 2 MB is under 2.8 MB; anything far larger is refused before decoding.
    if a.content_base64.len() > MAX_IMAGE_BYTES / 3 * 4 + 16 {
        return Err(refused(IMAGE_REFUSED));
    }
    let bytes = b64
        .decode(a.content_base64.trim())
        .map_err(|_| refused(IMAGE_REFUSED))?;
    let png = icon_png(&bytes)?;
    // The audit entry records the file's size, not its content.
    *ctx.audit_args.borrow_mut() = Some(json!({"bytes": bytes.len()}));
    clear_picture(ctx)?;
    ctx.save_setting(MODULE, IMAGE_KEY, Some(&json!(b64.encode(&png))))?;
    ctx.save_setting(MODULE, ICON_KEY, Some(&json!({"kind": "image"})))?;
    info_json(ctx)
}

/// Delete the stored picture, its source and its settings.
fn clear_picture(ctx: &Ctx<'_>) -> CommandResult<()> {
    for key in [IMAGE_KEY, SOURCE_KEY, PICTURE_SETTINGS_KEY] {
        ctx.save_setting(MODULE, key, None)?;
    }
    Ok(())
}

fn clear_icon_image(ctx: &Ctx<'_>, _: NoArgs) -> CommandResult<Value> {
    let current = ctx.tx.get_setting(MODULE, ICON_KEY)?;
    if current.as_ref().and_then(|v| v["kind"].as_str()) == Some("image") {
        ctx.save_setting(MODULE, ICON_KEY, None)?;
    }
    clear_picture(ctx)?;
    ctx.quiet_noop.set(true);
    info_json(ctx)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CropArg {
    /// Left edge of the square, in the photo's pixels.
    x: u32,
    /// Top edge of the square, in the photo's pixels.
    y: u32,
    /// Side of the square, in the photo's pixels.
    size: u32,
}

/// The treatment's settings (app-desktop.md revision 40); missing values take the
/// defaults (the Classic style).
#[derive(Debug, Clone, Deserialize, serde::Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct PictureSettings {
    /// 0–100 (50): line spacing, wide to fine.
    pub detail: i32,
    /// 0–100 (50): how thick lines grow in dark areas.
    pub line_weight: i32,
    /// −50–50 (10).
    pub contrast: i32,
    /// −50–50 (0).
    pub brightness: i32,
    /// 0–100 (40): strength of drawn edges.
    pub outline: i32,
    /// 0–100 (30): how much lines bend with the tones.
    pub curve: i32,
    /// 0–100 (0): how much of the background is removed.
    pub background: i32,
    /// `black`, `navy` or `sepia`.
    pub ink: String,
    /// `white` or `cream`.
    pub paper: String,
    /// The touch-up brushes: a 256 × 256 mask (0 automatic, 1 keep, 2 remove) as
    /// runs of (mark, length − 1) byte pairs, row by row, base64; empty for none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mask_base64: String,
}

impl Default for PictureSettings {
    fn default() -> Self {
        let d = mk_import::engrave::Settings::default();
        PictureSettings {
            detail: d.detail,
            line_weight: d.line_weight,
            contrast: d.contrast,
            brightness: d.brightness,
            outline: d.outline,
            curve: d.curve,
            background: d.background,
            ink: "black".into(),
            paper: "white".into(),
            mask_base64: String::new(),
        }
    }
}

impl PictureSettings {
    /// Check the ranges and names, and decode the mask.
    fn checked(&self) -> CommandResult<(mk_import::engrave::Settings, Option<Vec<u8>>)> {
        use mk_import::engrave as e;
        let ranges = [
            ("Detail", self.detail, 0, 100),
            ("Line weight", self.line_weight, 0, 100),
            ("Contrast", self.contrast, -50, 50),
            ("Brightness", self.brightness, -50, 50),
            ("Outline", self.outline, 0, 100),
            ("Curve", self.curve, 0, 100),
            ("Background", self.background, 0, 100),
        ];
        for (name, v, lo, hi) in ranges {
            if !(lo..=hi).contains(&v) {
                return Err(refused(format!("{name} runs from {lo} to {hi}.")));
            }
        }
        let ink = match self.ink.as_str() {
            "black" => e::INK_BLACK,
            "navy" => e::INK_NAVY,
            "sepia" => e::INK_SEPIA,
            other => {
                return Err(refused(format!(
                    "Unknown ink {other:?}; choose black, navy or sepia."
                )));
            }
        };
        let paper = match self.paper.as_str() {
            "white" => e::PAPER_WHITE,
            "cream" => e::PAPER_CREAM,
            other => {
                return Err(refused(format!(
                    "Unknown paper {other:?}; choose white or cream."
                )));
            }
        };
        let mask = if self.mask_base64.is_empty() {
            None
        } else {
            let packed = base64::engine::general_purpose::STANDARD
                .decode(self.mask_base64.trim())
                .map_err(|_| refused("The brush mask is damaged."))?;
            Some(
                mk_core::mask::unpack(&packed)
                    .ok_or_else(|| refused("The brush mask is damaged."))?,
            )
        };
        Ok((
            e::Settings {
                detail: self.detail,
                line_weight: self.line_weight,
                contrast: self.contrast,
                brightness: self.brightness,
                outline: self.outline,
                curve: self.curve,
                background: self.background,
                ink,
                paper,
            },
            mask,
        ))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PictureArgs {
    /// The photo, PNG or JPEG, base64 (at most 2 MB, 6,000 pixels a side).
    source_png_base64: String,
    /// The square of the photo to use.
    crop: CropArg,
    #[serde(default)]
    settings: PictureSettings,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PreviewArgs {
    /// The photo, PNG or JPEG, base64 (at most 2 MB, 6,000 pixels a side).
    source_png_base64: String,
    /// The square of the photo to use.
    crop: CropArg,
    #[serde(default)]
    settings: PictureSettings,
    /// The preview's side: 256 (default) or 512.
    #[serde(default)]
    size: Option<u32>,
    /// Also return the whole photo scaled to at most 512 pixels a side
    /// (`photo_png_base64`, with its full `width` and `height`), for the crop view.
    #[serde(default)]
    include_photo: bool,
}

fn decode_source(b64: &str) -> CommandResult<mk_import::image::Rgb> {
    if b64.len() > MAX_IMAGE_BYTES / 3 * 4 + 16 {
        return Err(refused(IMAGE_REFUSED));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|_| refused(IMAGE_REFUSED))?;
    mk_import::image::decode_image(&bytes, MAX_IMAGE_BYTES, mk_import::image::MAX_ICON_SIDE)
        .map_err(|_| refused(IMAGE_REFUSED))
}

/// The cropped square of the photo at `side` × `side`.
fn cropped(
    rgb: &mk_import::image::Rgb,
    crop: &CropArg,
    side: u32,
) -> CommandResult<mk_import::image::Rgb> {
    mk_import::engrave::crop_square(rgb, crop.x, crop.y, crop.size, side).map_err(|_| {
        refused(format!(
            "The crop must be a square inside the {} × {} picture.",
            rgb.width, rgb.height
        ))
    })
}

fn png_of(rgba: &mk_import::engrave::Rgba) -> CommandResult<Vec<u8>> {
    mk_import::engrave::encode_png_rgba(rgba).map_err(|e| refused(e.to_string()))
}

fn rgb_png(rgb: &mk_import::image::Rgb) -> CommandResult<Vec<u8>> {
    mk_import::image::encode_png(rgb).map_err(|e| refused(e.to_string()))
}

fn set_picture(ctx: &Ctx<'_>, a: PictureArgs) -> CommandResult<Value> {
    use mk_import::engrave::{RESULT_SIDE, WORK_SIDE};
    let (settings, mask) = a.settings.checked()?;
    let rgb = decode_source(&a.source_png_base64)?;
    let source = cropped(&rgb, &a.crop, WORK_SIDE)?;
    let result = mk_import::engrave::engrave(&source, &settings, mask.as_deref(), RESULT_SIDE)
        .map_err(|e| refused(e.to_string()))?;
    let b64 = base64::engine::general_purpose::STANDARD;
    let result_png = png_of(&result)?;
    let source_png = rgb_png(&source)?;
    // The audit entry records the settings and the crop, not the pictures.
    let mut recorded = serde_json::to_value(&a.settings)?;
    if let Some(o) = recorded.as_object_mut() {
        o.remove("mask_base64");
        o.insert("brushes".into(), json!(!a.settings.mask_base64.is_empty()));
        o.insert(
            "crop".into(),
            json!({"x": a.crop.x, "y": a.crop.y, "size": a.crop.size}),
        );
    }
    *ctx.audit_args.borrow_mut() = Some(recorded);
    ctx.save_setting(MODULE, IMAGE_KEY, Some(&json!(b64.encode(&result_png))))?;
    ctx.save_setting(MODULE, SOURCE_KEY, Some(&json!(b64.encode(&source_png))))?;
    ctx.save_setting(
        MODULE,
        PICTURE_SETTINGS_KEY,
        Some(&serde_json::to_value(&a.settings)?),
    )?;
    ctx.save_setting(MODULE, ICON_KEY, Some(&json!({"kind": "image"})))?;
    info_json(ctx)
}

fn preview_picture(_ctx: &Ctx<'_>, a: PreviewArgs) -> CommandResult<Value> {
    let side = a.size.unwrap_or(256);
    if side != 256 && side != 512 {
        return Err(refused("The preview size is 256 or 512."));
    }
    let (settings, mask) = a.settings.checked()?;
    let rgb = decode_source(&a.source_png_base64)?;
    let before = cropped(&rgb, &a.crop, side)?;
    let after = mk_import::engrave::engrave(&before, &settings, mask.as_deref(), side)
        .map_err(|e| refused(e.to_string()))?;
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut out = json!({
        "png_base64": b64.encode(png_of(&after)?),
        "before_png_base64": b64.encode(rgb_png(&before)?),
    });
    if a.include_photo
        && let Some(o) = out.as_object_mut()
    {
        let longest = rgb.width.max(rgb.height);
        let photo = if longest > 512 {
            let (w, h) = (
                (u64::from(rgb.width) * 512 / u64::from(longest)).max(1),
                (u64::from(rgb.height) * 512 / u64::from(longest)).max(1),
            );
            scaled(
                &rgb,
                u32::try_from(w).unwrap_or(1),
                u32::try_from(h).unwrap_or(1),
            )
        } else {
            rgb.clone()
        };
        o.insert(
            "photo_png_base64".into(),
            json!(b64.encode(rgb_png(&photo)?)),
        );
        o.insert("width".into(), json!(rgb.width));
        o.insert("height".into(), json!(rgb.height));
    }
    Ok(out)
}

/// The whole picture scaled down to `w` × `h` (each output pixel the average of the
/// source pixels it covers).
fn scaled(img: &mk_import::image::Rgb, w: u32, h: u32) -> mk_import::image::Rgb {
    let (sw, sh) = (u64::from(img.width), u64::from(img.height));
    let (w64, h64) = (u64::from(w), u64::from(h));
    let mut data = Vec::new();
    for oy in 0..h64 {
        let (ya, yb) = (
            oy * sh / h64,
            ((oy + 1) * sh / h64).max(oy * sh / h64 + 1).min(sh),
        );
        for ox in 0..w64 {
            let (xa, xb) = (
                ox * sw / w64,
                ((ox + 1) * sw / w64).max(ox * sw / w64 + 1).min(sw),
            );
            let mut sum = [0u64; 3];
            for y in ya..yb {
                for x in xa..xb {
                    let i = usize::try_from((y * sw + x) * 3).unwrap_or(0);
                    for (c, acc) in sum.iter_mut().enumerate() {
                        *acc += u64::from(img.data.get(i + c).copied().unwrap_or(0));
                    }
                }
            }
            let n = ((yb - ya) * (xb - xa)).max(1);
            for acc in sum {
                data.push(u8::try_from((acc + n / 2) / n).unwrap_or(255));
            }
        }
    }
    mk_import::image::Rgb {
        width: w,
        height: h,
        data,
    }
}

fn picture_source(ctx: &Ctx<'_>, _: NoArgs) -> CommandResult<Value> {
    let source = ctx
        .tx
        .get_setting(MODULE, SOURCE_KEY)?
        .and_then(|v| v.as_str().map(str::to_owned));
    let Some(source) = source else {
        return Ok(json!({}));
    };
    let settings = ctx
        .tx
        .get_setting(MODULE, PICTURE_SETTINGS_KEY)?
        .and_then(|v| serde_json::from_value::<PictureSettings>(v).ok())
        .unwrap_or_default();
    Ok(json!({"source_png_base64": source, "settings": settings}))
}

/// `book.info`: the book record, its opening-balances account and its profile.
pub(crate) fn info_json(ctx: &Ctx<'_>) -> CommandResult<Value> {
    let info = ctx.tx.info()?;
    let profile = profile_json(ctx, &info.name)?;
    let mut v = serde_json::to_value(&info)?;
    if let Some(o) = v.as_object_mut() {
        o.insert(
            "opening_balances_account".into(),
            json!(ctx.tx.opening_account_id()?),
        );
        o.insert("chart".into(), crate::chart::info(ctx)?);
        if let Some(p) = profile.as_object() {
            for (k, val) in p {
                o.insert(k.clone(), val.clone());
            }
        }
    }
    Ok(v)
}
