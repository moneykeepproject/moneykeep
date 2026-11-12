//! The Picture editor (app-desktop.md revision 40): crop and zoom a photo, preview
//! the engraving treatment on the book's colour (large and at card size, Before /
//! After), the seven sliders, ink and paper, the styles, the Keep and Remove brushes
//! and the tips. The treatment itself runs in the commands (`book.preview_picture`
//! while editing, `book.set_picture` on Use picture), so every front end gets the
//! same result.

use std::rc::Rc;

use base64::Engine;
use serde_json::{Value, json};

use super::App;
use super::ui::{Item, Kind, Picture, Tone, Ui};
use super::util::{png_picture, tile_color};
use crate::UiError;

/// The frame side of the crop view and the large preview, in pixels.
const FRAME: i32 = 256;
/// The mask side (as `book.set_picture` takes it).
const MASK_SIDE: usize = mk_core::mask::SIDE;

/// The tips (owner request), shown in Settings → Book and in the editor.
pub(crate) const TIPS: [(&str, &str); 6] = [
    (
        "Who:",
        "one person, or a small group, from the chest or waist up. Something that shows what the book is about helps: a tool, an instrument, a building.",
    ),
    (
        "Background:",
        "plain, and different from the clothes: a light wall behind dark clothes, or a darker wall behind light clothes. Busy rooms, trees and patterns are hard to remove.",
    ),
    (
        "Light:",
        "soft and even, from the front: daylight from a window beside the camera, or outdoors in shade or on a cloudy day. Avoid light from behind the person (a window or the sun behind them) and harsh midday sun, which makes deep shadows.",
    ),
    (
        "Framing:",
        "the person fills most of the square, with a little space above the head.",
    ),
    (
        "Quality:",
        "sharp, no filters, at least 800 pixels on the short side.",
    ),
    (
        "Permission:",
        "use photos you took or have the right to use, and ask people before using a picture of them.",
    ),
];
/// The line after the tips.
pub(crate) const TIPS_AFTER: &str = "If the background isn't fully removed, lower or raise Background, then paint with Keep or Remove.";
pub(crate) const TIPS_TITLE: &str = "Tips for a good picture";

/// The slider values and colours, as `book.set_picture` takes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sliders {
    pub detail: i32,
    pub line_weight: i32,
    pub contrast: i32,
    pub brightness: i32,
    pub outline: i32,
    pub curve: i32,
    pub background: i32,
    pub ink: String,
    pub paper: String,
}

impl Default for Sliders {
    fn default() -> Self {
        Sliders {
            detail: 50,
            line_weight: 50,
            contrast: 10,
            brightness: 0,
            outline: 40,
            curve: 30,
            background: 0,
            ink: "black".into(),
            paper: "white".into(),
        }
    }
}

impl Sliders {
    fn from_json(v: &Value) -> Sliders {
        let d = Sliders::default();
        let n = |k: &str, def: i32| {
            v.get(k)
                .and_then(Value::as_i64)
                .and_then(|x| i32::try_from(x).ok())
                .unwrap_or(def)
        };
        let t = |k: &str, def: &str| v.get(k).and_then(Value::as_str).unwrap_or(def).to_owned();
        Sliders {
            detail: n("detail", d.detail),
            line_weight: n("line_weight", d.line_weight),
            contrast: n("contrast", d.contrast),
            brightness: n("brightness", d.brightness),
            outline: n("outline", d.outline),
            curve: n("curve", d.curve),
            background: n("background", d.background),
            ink: t("ink", &d.ink),
            paper: t("paper", &d.paper),
        }
    }

    /// A style: Fine, Classic (the defaults) or Bold; brightness, background and the
    /// colours stay.
    fn style(&mut self, name: &str) {
        let (detail, line_weight, contrast, outline, curve) = match name {
            "Fine" => (85, 35, 10, 30, 25),
            "Bold" => (20, 80, 20, 70, 35),
            _ => (50, 50, 10, 40, 30),
        };
        self.detail = detail;
        self.line_weight = line_weight;
        self.contrast = contrast;
        self.outline = outline;
        self.curve = curve;
    }
}

/// One brush stroke: its mark (1 keep, 2 remove), radius and points, in mask pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stroke {
    mark: u8,
    radius: i32,
    points: Vec<(i32, i32)>,
}

/// The open editor.
#[derive(Debug)]
pub(crate) struct Editor {
    /// The photo as chosen (or the stored source when editing again), base64.
    original_b64: String,
    width: u32,
    height: u32,
    /// The photo scaled to at most 512 a side, for the crop view.
    photo: Rc<Picture>,
    /// The crop square, in the original's pixels.
    crop: (u32, u32, u32),
    zoom: i32,
    pub(crate) sliders: Sliders,
    /// The brush marks brought back from the stored settings.
    base_mask: Vec<u8>,
    strokes: Vec<Stroke>,
    /// The brush in use: 1 keep, 2 remove.
    brush: Option<u8>,
    brush_size: i32,
    /// The cropped photo at 256, base64 PNG (the Before view and the preview's source).
    before_b64: String,
    before: Option<Rc<Picture>>,
    after: Option<Rc<Picture>>,
    show_before: bool,
    error: Option<String>,
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn picture_of(b: Option<&Value>) -> Option<Rc<Picture>> {
    b?.as_str()
        .and_then(|s| b64().decode(s).ok())
        .and_then(|png| png_picture(&png, 512))
}

impl Editor {
    fn mask(&self) -> Vec<u8> {
        let mut m = self.base_mask.clone();
        m.resize(mk_core::mask::LEN, 0);
        let side = i32::try_from(MASK_SIDE).unwrap_or(256);
        for s in &self.strokes {
            let r = s.radius.max(1);
            // Each segment between points is painted with discs along it.
            let mut prev = s.points.first().copied();
            for &p in &s.points {
                let a = prev.unwrap_or(p);
                let steps = ((p.0 - a.0).abs().max((p.1 - a.1).abs()) / r.max(1) * 2).max(1);
                for k in 0..=steps {
                    let cx = a.0 + (p.0 - a.0) * k / steps;
                    let cy = a.1 + (p.1 - a.1) * k / steps;
                    for y in (cy - r).max(0)..=(cy + r).min(side - 1) {
                        for x in (cx - r).max(0)..=(cx + r).min(side - 1) {
                            if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r
                                && let Some(v) = usize::try_from(y * side + x)
                                    .ok()
                                    .and_then(|i| m.get_mut(i))
                            {
                                *v = s.mark;
                            }
                        }
                    }
                }
                prev = Some(p);
            }
        }
        m
    }

    fn settings_json(&self) -> Value {
        let s = &self.sliders;
        let mask = mk_core::mask::pack(&self.mask());
        json!({
            "detail": s.detail, "line_weight": s.line_weight, "contrast": s.contrast,
            "brightness": s.brightness, "outline": s.outline, "curve": s.curve,
            "background": s.background, "ink": s.ink, "paper": s.paper,
            "mask_base64": b64().encode(mask),
        })
    }

    fn crop_json(&self) -> Value {
        json!({"x": self.crop.0, "y": self.crop.1, "size": self.crop.2})
    }

    /// Keep the crop inside the photo, its side set by the zoom.
    fn fit_crop(&mut self, cx: i64, cy: i64) {
        let short = i64::from(self.width.min(self.height));
        let size = (short * 100 / i64::from(self.zoom.clamp(100, 400))).max(1);
        let x = (cx - size / 2).clamp(0, i64::from(self.width) - size);
        let y = (cy - size / 2).clamp(0, i64::from(self.height) - size);
        let u = |v: i64| u32::try_from(v.max(0)).unwrap_or(0);
        self.crop = (u(x), u(y), u(size));
    }

    fn centre(&self) -> (i64, i64) {
        let (x, y, s) = self.crop;
        (
            i64::from(x) + i64::from(s) / 2,
            i64::from(y) + i64::from(s) / 2,
        )
    }
}

/// Open the editor on a chosen photo (after Choose picture…).
pub(crate) fn open_photo(app: &mut App, bytes: &[u8]) -> Result<(), UiError> {
    open_with(app, b64().encode(bytes), None, Vec::new())
}

/// Open the editor on the book's stored source and settings (Edit picture…).
pub(crate) fn open_stored(app: &mut App) -> Result<(), UiError> {
    let src = app.backend.invoke("book.picture_source", json!({}))?;
    let Some(source) = src.get("source_png_base64").and_then(Value::as_str) else {
        return Err(UiError::new(
            "invalid_args",
            "This picture has no photo to edit; choose a picture instead.",
        ));
    };
    let settings = src.get("settings").cloned().unwrap_or(Value::Null);
    let mask = settings
        .get("mask_base64")
        .and_then(Value::as_str)
        .and_then(|m| b64().decode(m).ok())
        .and_then(|runs| mk_core::mask::unpack(&runs))
        .unwrap_or_default();
    open_with(
        app,
        source.to_owned(),
        Some(Sliders::from_json(&settings)),
        mask,
    )
}

fn open_with(
    app: &mut App,
    original_b64: String,
    sliders: Option<Sliders>,
    base_mask: Vec<u8>,
) -> Result<(), UiError> {
    // A first look at the photo: its size, a small copy for the crop view.
    let probe = app.backend.invoke(
        "book.preview_picture",
        json!({"source_png_base64": original_b64, "crop": {"x": 0, "y": 0, "size": 1},
               "include_photo": true}),
    )?;
    let size = |k: &str| {
        probe
            .get(k)
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(1)
    };
    let (width, height) = (size("width"), size("height"));
    let photo = picture_of(probe.get("photo_png_base64"))
        .ok_or_else(|| UiError::new("invalid_args", "Choose a PNG or JPEG picture up to 2 MB."))?;
    let mut ed = Editor {
        original_b64,
        width,
        height,
        photo,
        crop: (0, 0, width.min(height)),
        zoom: 100,
        sliders: sliders.unwrap_or_default(),
        base_mask,
        strokes: Vec::new(),
        brush: None,
        brush_size: 6,
        before_b64: String::new(),
        before: None,
        after: None,
        show_before: false,
        error: None,
    };
    ed.fit_crop(i64::from(width) / 2, i64::from(height) / 2);
    app.picture = Some(ed);
    if app.config.picture_tips_open.is_none() {
        app.config.picture_tips_open = Some(true);
        app.emit(crate::AppEvent::ConfigChanged);
    }
    recrop(app);
    Ok(())
}

/// The crop changed: a new Before from the original, and a new preview.
fn recrop(app: &mut App) {
    let Some(ed) = app.picture.as_ref() else {
        return;
    };
    let args = json!({"source_png_base64": ed.original_b64, "crop": ed.crop_json(),
        "settings": ed.settings_json()});
    let out = app.backend.invoke("book.preview_picture", args);
    let Some(ed) = app.picture.as_mut() else {
        return;
    };
    match out {
        Ok(v) => {
            ed.before_b64 = v
                .get("before_png_base64")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            ed.before = picture_of(v.get("before_png_base64"));
            ed.after = picture_of(v.get("png_base64"));
            ed.error = None;
        }
        Err(e) => ed.error = Some(e.display()),
    }
}

/// The settings changed: a new preview from the cropped photo at 256.
fn refresh(app: &mut App) {
    let Some(ed) = app.picture.as_ref() else {
        return;
    };
    if ed.before_b64.is_empty() {
        recrop(app);
        return;
    }
    let args = json!({"source_png_base64": ed.before_b64,
        "crop": {"x": 0, "y": 0, "size": 256}, "settings": ed.settings_json()});
    let out = app.backend.invoke("book.preview_picture", args);
    let Some(ed) = app.picture.as_mut() else {
        return;
    };
    match out {
        Ok(v) => {
            ed.after = picture_of(v.get("png_base64"));
            ed.error = None;
        }
        Err(e) => ed.error = Some(e.display()),
    }
}

/// The large preview: After with the brush marks tinted while a brush is in use, or
/// Before.
fn shown(ed: &Editor) -> Option<Rc<Picture>> {
    if ed.show_before {
        return ed.before.clone();
    }
    let after = ed.after.clone()?;
    if ed.brush.is_none() || (ed.strokes.is_empty() && ed.base_mask.iter().all(|m| *m == 0)) {
        return Some(after);
    }
    let mask = ed.mask();
    let mut p = (*after).clone();
    if p.width as usize != MASK_SIDE || p.height as usize != MASK_SIDE {
        return Some(after);
    }
    for (px, m) in p.rgba.chunks_exact_mut(4).zip(&mask) {
        let tint: [u16; 3] = match m {
            1 => [0x2E, 0xB8, 0x6A],
            2 => [0xE5, 0x48, 0x4D],
            _ => continue,
        };
        if let [r, g, b, a] = px {
            for (c, t) in [r, g, b].into_iter().zip(tint) {
                *c = u8::try_from((u16::from(*c) * 3 + t * 2) / 5).unwrap_or(255);
            }
            *a = (*a).max(120);
        }
    }
    Some(Rc::new(p))
}

/// A labelled slider row; true when it moved.
fn slider(ui: &mut Ui, label: &str, value: &mut i32, min: i32, max: i32) -> bool {
    let mut changed = false;
    ui.form(label, |ui| {
        ui.next().width(260);
        changed = ui.slider(label, value, min, max).changed();
        ui.next().width(40).tone(Tone::Secondary).quiet();
        ui.label(value.to_string());
    });
    changed
}

/// The editor dialog (when open).
pub(crate) fn dialog(app: &mut App, ui: &mut Ui) {
    if app.picture.is_none() {
        return;
    }
    let mut close = ui.dialog_closed("picture-editor");
    let color = tile_color(&book_color(app));
    let mut tips_open = app.config.picture_tips_open.unwrap_or(true);
    let mut crop_moved = false;
    let mut changed = false;
    let mut save = false;
    ui.dialog("picture-editor", "Picture editor", 900, |ui| {
        let Some(ed) = app.picture.as_mut() else {
            return;
        };
        if let Some(e) = &ed.error {
            ui.error_text(e.clone());
        }
        ui.row(|ui| {
            for (text, width) in [
                ("Crop: drag to move", FRAME + 4),
                ("Preview", FRAME + 4),
                ("Card size", 80),
            ] {
                ui.next().width(width).tone(Tone::Secondary).quiet();
                ui.caption(text);
            }
        });
        ui.row(|ui| {
            // Crop: the photo in a square frame; drag to move.
            let (cx, cy, cs) = ed.crop;
            let scale = |v: u32| {
                i32::try_from(u64::from(v) * u64::from(ed.photo.width) / u64::from(ed.width.max(1)))
                    .unwrap_or(0)
            };
            let resp = ui.item(
                Item {
                    kind: Kind::CropFrame,
                    label: "Crop".into(),
                    desc: "Drag to move the picture in the square".into(),
                    picture: Some(ed.photo.clone()),
                    number: scale(cx),
                    minimum: scale(cy),
                    maximum: scale(cs).max(1),
                    width: FRAME,
                    ..Item::default()
                },
                true,
            );
            if resp.changed()
                && let Some((dx, dy)) = resp.value.split_once(',')
                && let (Ok(dx), Ok(dy)) = (dx.trim().parse::<i64>(), dy.trim().parse::<i64>())
            {
                let (x, y) = ed.centre();
                let per = i64::from(cs) * 1000 / i64::from(FRAME);
                ed.fit_crop(x - dx * per / 1000, y - dy * per / 1000);
                crop_moved = true;
            }
            // The large preview, where the brushes paint.
            let radius = ed.brush_size * 256 / 200;
            let resp = ui.item(
                Item {
                    kind: Kind::PaintCanvas,
                    label: if ed.show_before {
                        "Preview (before)".into()
                    } else {
                        "Preview (after)".into()
                    },
                    picture: shown(ed),
                    color: Some(color),
                    checked: ed.brush.is_some() && !ed.show_before,
                    number: radius,
                    width: FRAME,
                    ..Item::default()
                },
                true,
            );
            if resp.changed()
                && let Some(mark) = ed.brush
            {
                let points: Vec<(i32, i32)> = resp
                    .value
                    .split(';')
                    .filter_map(|p| p.split_once(','))
                    .filter_map(|(x, y)| Some((x.trim().parse().ok()?, y.trim().parse().ok()?)))
                    .collect();
                if !points.is_empty() {
                    ed.strokes.push(Stroke {
                        mark,
                        radius,
                        points,
                    });
                    changed = true;
                }
            }
            // At card size.
            ui.item(
                Item {
                    kind: Kind::PaintCanvas,
                    label: "Preview at card size".into(),
                    picture: if ed.show_before {
                        ed.before.clone()
                    } else {
                        ed.after.clone()
                    },
                    color: Some(color),
                    width: 56,
                    ..Item::default()
                },
                false,
            );
        });
        ui.row(|ui| {
            let mut zoom = ed.zoom;
            ui.next().quiet();
            ui.label("Zoom");
            ui.next().width(150);
            if ui.slider("Zoom", &mut zoom, 100, 400).changed() {
                let (x, y) = ed.centre();
                ed.zoom = zoom;
                ed.fit_crop(x, y);
                crop_moved = true;
            }
            ui.next().width(52).tone(Tone::Secondary).quiet();
            ui.label(format!("{zoom} %"));
            if ui.tab("Before", ed.show_before).clicked() {
                ed.show_before = true;
            }
            if ui.tab("After", !ed.show_before).clicked() {
                ed.show_before = false;
            }
        });
        ui.row(|ui| {
            for (mark, name) in [(1u8, "Keep"), (2u8, "Remove")] {
                let on = ed.brush == Some(mark);
                if ui.toggle(name, on).clicked() {
                    ed.brush = if on { None } else { Some(mark) };
                    if !on {
                        ed.show_before = false;
                    }
                }
            }
            ui.next().quiet();
            ui.label("Brush size");
            ui.next().width(140);
            ui.slider("Brush size", &mut ed.brush_size, 2, 20);
            ui.next().width(40).tone(Tone::Secondary).quiet();
            ui.label(format!("{} %", ed.brush_size));
            ui.enabled(!ed.strokes.is_empty());
            if ui.button("Undo stroke").clicked() {
                ed.strokes.pop();
                changed = true;
            }
            let any = !ed.strokes.is_empty() || ed.base_mask.iter().any(|m| *m != 0);
            ui.enabled(any);
            if ui.button("Clear brushes").clicked() {
                ed.strokes.clear();
                ed.base_mask.clear();
                changed = true;
            }
        });
        ui.card("", |ui| {
            let s = &mut ed.sliders;
            changed |= slider(ui, "Detail", &mut s.detail, 0, 100);
            changed |= slider(ui, "Line weight", &mut s.line_weight, 0, 100);
            changed |= slider(ui, "Contrast", &mut s.contrast, -50, 50);
            changed |= slider(ui, "Brightness", &mut s.brightness, -50, 50);
            changed |= slider(ui, "Outline", &mut s.outline, 0, 100);
            changed |= slider(ui, "Curve", &mut s.curve, 0, 100);
            changed |= slider(ui, "Background", &mut s.background, 0, 100);
            ui.form("Ink", |ui| {
                for (key, name) in [("black", "Black"), ("navy", "Navy"), ("sepia", "Sepia")] {
                    if ui.toggle(name, s.ink == key).clicked() && s.ink != key {
                        s.ink = key.into();
                        changed = true;
                    }
                }
            });
            ui.form("Paper", |ui| {
                for (key, name) in [("white", "White"), ("cream", "Cream")] {
                    if ui.toggle(name, s.paper == key).clicked() && s.paper != key {
                        s.paper = key.into();
                        changed = true;
                    }
                }
            });
            ui.form("Style", |ui| {
                for name in ["Fine", "Classic", "Bold"] {
                    if ui.button(name).clicked() {
                        s.style(name);
                        changed = true;
                    }
                }
                if ui.button("Reset").clicked() {
                    *s = Sliders::default();
                    changed = true;
                }
            });
        });
        ui.collapsing(TIPS_TITLE, &mut tips_open, tips);
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                close = true;
            }
            if ui.primary("Use picture").clicked() {
                save = true;
            }
        });
    });
    if app.config.picture_tips_open != Some(tips_open) {
        app.config.picture_tips_open = Some(tips_open);
        app.emit(crate::AppEvent::ConfigChanged);
    }
    if crop_moved {
        recrop(app);
    } else if changed {
        refresh(app);
    }
    if save && let Some(ed) = app.picture.as_ref() {
        let args = json!({"source_png_base64": ed.original_b64, "crop": ed.crop_json(),
            "settings": ed.settings_json()});
        match app.run("book.set_picture", args) {
            Ok(_) => {
                close = true;
                super::settings::picture_saved(app);
            }
            Err(e) => {
                if let Some(ed) = app.picture.as_mut() {
                    ed.error = Some(e.display());
                }
            }
        }
    }
    if close {
        app.picture = None;
    }
}

/// The tips text (in the editor's panel and under Choose picture…).
pub(crate) fn tips(ui: &mut Ui) {
    for (head, text) in TIPS {
        ui.row(|ui| {
            ui.next().bold();
            ui.label(head);
            ui.next().grow();
            ui.label(text);
        });
    }
    ui.secondary(TIPS_AFTER);
}

fn book_color(app: &mut App) -> String {
    app.backend
        .invoke("book.info", json!({}))
        .ok()
        .and_then(|v| v.get("color").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| "teal".into())
}

/// What the open editor shows: the treated preview (without the brush tint) and
/// the settings as `book.set_picture` would take them (tests and review).
pub(crate) fn snapshot(app: &App) -> Option<(Option<Rc<Picture>>, Value)> {
    let ed = app.picture.as_ref()?;
    Some((ed.after.clone(), ed.settings_json()))
}
