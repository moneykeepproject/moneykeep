//! Settings → Templates (app-desktop revision 16): document templates for invoices,
//! estimates, credit memos, statements and purchase orders — accent colour, font,
//! what to show, payment instructions, footer and logo — and the default template.

use std::rc::Rc;

use base64::Engine;
use serde_json::{Value, json};

use super::form::{choice, set};
use super::ui::{Picture, Ui};
use super::util::Json;
use super::{App, AppEvent};

const FONTS: [(&str, &str); 2] = [("helvetica", "Helvetica"), ("times", "Times")];

/// (key in `show`, label)
const SHOW: [(&str, &str); 6] = [
    ("terms", "Show terms"),
    ("due_date", "Show due date"),
    ("sku", "Show SKU"),
    ("quantity_rate", "Show quantity and rate"),
    ("customer_email", "Show customer email"),
    ("payment_instructions", "Show payment instructions"),
];

/// The logo limits, as stated next to the upload button.
pub(crate) const LOGO_LIMITS: &str = "PNG or JPEG, at most 512 KB and 2,000 × 2,000 pixels.";

#[derive(Debug, Clone, Default)]
pub(crate) struct TemplateForm {
    /// Editing this template (id); `None` for a new one.
    pub editing: Option<String>,
    pub name: String,
    pub accent: String,
    pub font: usize,
    pub show: Vec<(String, bool)>,
    pub payment_instructions: String,
    pub footer: String,
    pub error: Option<String>,
}

/// A logo preview: (template id, generation) and the picture.
type Preview = ((String, u64), Option<Rc<Picture>>);

#[derive(Debug, Default)]
pub(crate) struct TemplatesState {
    pub form: Option<TemplateForm>,
    /// The last logo upload's refusal (shown under the logo).
    pub logo_error: Option<String>,
    /// The logo preview for (template id, generation).
    preview: Option<Preview>,
}

fn form_from(t: &Value) -> TemplateForm {
    TemplateForm {
        editing: Some(t.str("id").to_owned()),
        name: t.str("name").to_owned(),
        accent: t.str("accent_color").to_owned(),
        font: FONTS.iter().position(|f| f.0 == t.str("font")).unwrap_or(0),
        show: SHOW
            .iter()
            .map(|(k, _)| ((*k).to_owned(), t.at("show").at(k).as_bool() == Some(true)))
            .collect(),
        payment_instructions: t.str("payment_instructions").to_owned(),
        footer: t.str("footer").to_owned(),
        error: None,
    }
}

fn new_form() -> TemplateForm {
    TemplateForm {
        show: SHOW
            .iter()
            .map(|(k, _)| ((*k).to_owned(), !matches!(*k, "sku" | "customer_email")))
            .collect(),
        ..TemplateForm::default()
    }
}

/// Upload a logo file for a template (the app calls this after `ChooseLogoFile`).
pub(crate) fn set_logo(app: &mut App, template: &str, bytes: &[u8]) {
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    match app.run(
        "template.set_logo",
        json!({"template": template, "content_base64": b64}),
    ) {
        Ok(_) => app.templates.logo_error = None,
        Err(e) => app.templates.logo_error = Some(e.display()),
    }
}

pub(crate) fn settings_section(app: &mut App, ui: &mut Ui) {
    let list = super::util::array(app.read("template.list", json!({})));
    ui.card("Templates", |ui| {
        ui.label("Templates set the look of invoices, estimates, credit memos, statements and purchase orders.");
        default_picker(app, ui, &list);
        ui.wrap(|ui| {
            if ui.primary("New template…").clicked() {
                app.templates.form = Some(new_form());
                app.templates.logo_error = None;
            }
            for t in &list {
                let editing = app.templates.form.as_ref().and_then(|f| f.editing.as_deref()) == Some(t.str("id"));
                ui.named(format!("Template {}", t.str("name")));
                if ui.toggle(t.str("name"), editing).clicked() {
                    app.templates.form = Some(form_from(t));
                    app.templates.logo_error = None;
                }
            }
        });
        if list.is_empty() && app.templates.form.is_none() {
            ui.secondary("No templates yet; documents use the built-in look.");
        }
    });
    editor(app, ui);
}

fn default_picker(app: &mut App, ui: &mut Ui, list: &[Value]) {
    let current = app
        .read("module.settings", json!({"module": "invoicing"}))
        .and_then(|s| {
            s.list("settings")
                .iter()
                .find(|x| x.str("key") == "invoicing.default_template")
                .map(|x| x.at("value").as_str().unwrap_or_default().to_owned())
        })
        .unwrap_or_default();
    let mut options = vec!["Built-in".to_owned()];
    options.extend(list.iter().map(|t| t.str("name").to_owned()));
    let mut idx = if current.is_empty() {
        0
    } else {
        list.iter()
            .position(|t| t.str("id") == current)
            .map_or(0, |i| i + 1)
    };
    let changed = ui.form("Default template", |ui| {
        ui.width(240);
        ui.combo("Default template", &mut idx, &options).changed()
    });
    if changed {
        let v = if idx == 0 {
            Value::Null
        } else {
            list.get(idx - 1)
                .map_or(Value::Null, |t| json!(t.str("id")))
        };
        if let Err(e) = app.run(
            "module.set_setting",
            json!({"key": "invoicing.default_template", "value": v}),
        ) {
            app.notify_error(&e);
        }
    }
}

/// `#RRGGBB` → 0xRRGGBB (for the swatch and preview only).
fn swatch(text: &str) -> Option<u32> {
    let h = text.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    u32::from_str_radix(h, 16).ok()
}

fn editor(app: &mut App, ui: &mut Ui) {
    let Some(mut f) = app.templates.form.take() else {
        return;
    };
    let heading = if f.editing.is_some() {
        format!("Template {}", f.name)
    } else {
        "New template".to_owned()
    };
    let (mut save, mut close, mut delete) = (false, false, false);
    ui.card(&heading, |ui| {
        ui.group(|ui| {
            super::form::text_row(ui, "Template name", &mut f.name, "");
            ui.form("Accent colour", |ui| {
                ui.width(110).hint("#RRGGBB");
                ui.field("Accent colour", &mut f.accent);
                if let Some(c) = swatch(&f.accent) {
                    super::form::swatch(ui, c);
                }
            });
            ui.form("Font", |ui| {
                choice(ui, "Font", &mut f.font, &FONTS.map(|x| x.1));
            });
            for (i, (k, on)) in f.show.iter_mut().enumerate() {
                let label = SHOW.iter().find(|s| s.0 == k).map_or("", |s| s.1);
                ui.form(if i == 0 { "Show" } else { "" }, |ui| {
                    ui.checkbox(on, label);
                });
            }
            ui.form("Payment instructions", |ui| {
                ui.width(320);
                ui.multiline("Payment instructions", &mut f.payment_instructions);
            });
            super::form::text_row(ui, "Footer", &mut f.footer, "");
        });
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        ui.row(|ui| {
            if ui.primary("Save template").clicked() {
                save = true;
            }
            if f.editing.is_some() && ui.button("Delete template").clicked() {
                delete = true;
            }
            if ui.button("Close").clicked() {
                close = true;
            }
        });
    });
    if let Some(id) = f.editing.clone() {
        logo_and_preview(app, ui, &id, &f);
    }
    if save {
        let show: serde_json::Map<String, Value> =
            f.show.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        let font = FONTS.get(f.font).map_or("helvetica", |x| x.0);
        let mut a = json!({"name": f.name.trim(), "accent_color": f.accent.trim(), "font": font,
            "show": show, "payment_instructions": f.payment_instructions, "footer": f.footer});
        let cmd = match &f.editing {
            Some(id) => {
                set(&mut a, "template", json!(id));
                "template.update"
            }
            None => "template.create",
        };
        match app.run(cmd, a) {
            Ok(t) => {
                f = form_from(&t);
                app.notify("Template saved.");
            }
            Err(e) => f.error = Some(e.display()),
        }
    }
    if delete {
        let id = f.editing.clone().unwrap_or_default();
        match app.run("template.delete", json!({"template": id})) {
            Ok(_) => close = true,
            Err(e) => f.error = Some(e.display()),
        }
    }
    if !close {
        app.templates.form = Some(f);
    }
}

fn logo_and_preview(app: &mut App, ui: &mut Ui, id: &str, f: &TemplateForm) {
    let t = app
        .read("template.get", json!({"template": id}))
        .unwrap_or(Value::Null);
    ui.card("Logo", |ui| {
        ui.row(|ui| {
            if ui.button("Upload logo…").clicked() {
                app.emit(AppEvent::ChooseLogoFile {
                    template: id.to_owned(),
                });
            }
            if t.at("logo").is_object() && ui.button("Remove logo").clicked() {
                match app.run("template.clear_logo", json!({"template": id})) {
                    Ok(_) => app.templates.logo_error = None,
                    Err(e) => app.notify_error(&e),
                }
            }
        });
        ui.secondary(LOGO_LIMITS);
        if let Some(e) = app.templates.logo_error.clone() {
            ui.tip("Logo refused");
            ui.error_text(e);
        }
    });
    // The preview: accent bar, logo, title and footer as the document shows them.
    ui.card("Preview", |ui| {
        let accent = swatch(&f.accent).unwrap_or(0x28_28_28);
        super::form::bar(ui, accent, 256);
        if let Some(p) = t.at("logo_preview").as_object() {
            let p = Value::Object(p.clone());
            if let Some(pic) = logo_picture(app, id, &p) {
                let label = format!(
                    "Logo preview {} × {}",
                    t.at("logo").at("width").as_u64().unwrap_or(0),
                    t.at("logo").at("height").as_u64().unwrap_or(0)
                );
                let w = i32::try_from(pic.width).unwrap_or(200);
                ui.picture(&label, pic, w);
            }
        }
        ui.heading("Invoice");
        ui.label("INV-1001 · Customer name");
        if !f.payment_instructions.trim().is_empty() {
            ui.label(f.payment_instructions.trim());
        }
        if !f.footer.trim().is_empty() {
            ui.caption(f.footer.trim());
        }
    });
}

fn logo_picture(app: &mut App, id: &str, p: &Value) -> Option<Rc<Picture>> {
    let key = (id.to_owned(), app.generation);
    if let Some((k, pic)) = &app.templates.preview
        && *k == key
    {
        return pic.clone();
    }
    let w = u32::try_from(p.at("width").as_u64().unwrap_or(0)).unwrap_or(0);
    let h = u32::try_from(p.at("height").as_u64().unwrap_or(0)).unwrap_or(0);
    let pic = base64::engine::general_purpose::STANDARD
        .decode(p.str("rgb_base64"))
        .ok()
        .and_then(|rgb| {
            let n = usize::try_from(w).ok()? * usize::try_from(h).ok()?;
            if w == 0 || h == 0 || rgb.len() != n * 3 {
                return None;
            }
            let rgba: Vec<u8> = rgb
                .chunks_exact(3)
                .flat_map(|c| match c {
                    [r, g, b] => [*r, *g, *b, 255],
                    _ => [0, 0, 0, 255],
                })
                .collect();
            Some(Rc::new(Picture {
                width: w,
                height: h,
                rgba,
            }))
        });
    app.templates.preview = Some((key, pic.clone()));
    pic
}
