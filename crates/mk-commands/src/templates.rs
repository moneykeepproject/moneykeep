//! Document templates (business-dimensions.md Part C): presentation options for
//! invoice, estimate, credit memo, statement and purchase order PDFs, with an optional
//! logo that is decoded and re-encoded on upload (the original file is never stored).

use std::collections::BTreeMap;

use base64::Engine;
use mk_store::{Doc, DocTable, now_rfc3339};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{CommandError, CommandResult, Ctx, Registry};

pub(crate) fn register(r: &mut Registry) {
    let m = crate::sales::INVOICING;
    r.add_in(
        m,
        "template.create",
        "Create a document template",
        true,
        create,
    );
    r.add_in(
        m,
        "template.update",
        "Change a document template",
        true,
        update,
    );
    r.add_in(
        m,
        "template.delete",
        "Delete a document template",
        true,
        delete,
    );
    r.add_in(m, "template.list", "List document templates", false, list);
    r.add_in(m, "template.get", "Get a document template", false, get);
    r.add_in(
        m,
        "template.set_logo",
        "Set a template's logo (PNG or JPEG, at most 512 KB and 2,000 × 2,000 pixels)",
        true,
        set_logo,
    );
    r.add_in(
        m,
        "template.clear_logo",
        "Remove a template's logo",
        true,
        clear_logo,
    );
}

/// The documents a title override can name.
pub(crate) const DOC_KINDS: [&str; 5] = [
    "invoice",
    "estimate",
    "credit_memo",
    "statement",
    "purchase_order",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Show {
    #[serde(default = "yes")]
    pub terms: bool,
    #[serde(default = "yes")]
    pub due_date: bool,
    #[serde(default)]
    pub sku: bool,
    #[serde(default = "yes")]
    pub quantity_rate: bool,
    #[serde(default)]
    pub customer_email: bool,
    #[serde(default = "yes")]
    pub payment_instructions: bool,
}

fn yes() -> bool {
    true
}

impl Default for Show {
    fn default() -> Self {
        Self {
            terms: true,
            due_date: true,
            sku: false,
            quantity_rate: true,
            customer_email: false,
            payment_instructions: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredLogo {
    pub width: u32,
    pub height: u32,
    /// zlib-compressed 8-bit RGB, base64.
    pub rgb_zlib: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Template {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub accent_color: Option<String>,
    /// `helvetica` or `times`.
    pub font: String,
    #[serde(default)]
    pub show: Show,
    #[serde(default)]
    pub payment_instructions: Option<String>,
    #[serde(default)]
    pub footer: Option<String>,
    #[serde(default)]
    pub title_overrides: BTreeMap<String, String>,
    #[serde(default)]
    pub logo: Option<StoredLogo>,
    pub created_at: String,
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

pub(crate) fn templates(ctx: &Ctx<'_>) -> CommandResult<Vec<Template>> {
    let mut v: Vec<Template> = ctx
        .tx
        .docs(DocTable::Template, None)?
        .into_iter()
        .map(|d| serde_json::from_value(d.json))
        .collect::<Result<_, _>>()?;
    v.sort_by_key(|t| t.name.to_lowercase());
    Ok(v)
}

pub(crate) fn find(ctx: &Ctx<'_>, r: &str) -> CommandResult<Template> {
    let r = r.trim();
    templates(ctx)?
        .into_iter()
        .find(|t| t.id == r || t.name.eq_ignore_ascii_case(r))
        .ok_or_else(|| CommandError::NotFound(format!("template {r:?}")))
}

fn save(ctx: &Ctx<'_>, t: &Template) -> CommandResult<()> {
    ctx.save_doc(
        DocTable::Template,
        &t.id,
        Some(&Doc {
            id: t.id.clone(),
            grp: None,
            ord: 0,
            json: serde_json::to_value(t)?,
        }),
    )
}

/// Template options as arguments (also the definition-file shape, without the logo).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TemplateArgs {
    /// The template to change (id or name); update only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `#RRGGBB`; "" clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent_color: Option<String>,
    /// `helvetica` or `times`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<Show>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment_instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footer: Option<String>,
    /// Document kind (invoice, estimate, credit_memo, statement, purchase_order) → title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_overrides: Option<BTreeMap<String, String>>,
}

fn text(v: &str, max: usize, what: &str) -> CommandResult<Option<String>> {
    let c = mk_core::text::clean(v, usize::MAX).text;
    if c.chars().count() > max {
        return Err(invalid(format!("{what}: at most {max} characters")));
    }
    Ok(Some(c).filter(|x| !x.trim().is_empty()))
}

/// `#RRGGBB` → (r, g, b).
pub(crate) fn parse_color(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let p = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok());
    Some((p(0)?, p(2)?, p(4)?))
}

fn apply(ctx: &Ctx<'_>, t: &mut Template, a: TemplateArgs) -> CommandResult<()> {
    if let Some(n) = a.name {
        let n = text(&n, 100, "name")?.ok_or_else(|| invalid("name: 1-100 characters"))?;
        if templates(ctx)?
            .iter()
            .any(|x| x.id != t.id && x.name.eq_ignore_ascii_case(&n))
        {
            return Err(CommandError::Rejected(format!(
                "a template named {n:?} already exists"
            )));
        }
        t.name = n;
    }
    if let Some(c) = a.accent_color {
        t.accent_color = if c.trim().is_empty() {
            None
        } else {
            parse_color(&c).ok_or_else(|| invalid("accent_color is #RRGGBB"))?;
            Some(c.trim().to_uppercase())
        };
    }
    if let Some(f) = a.font {
        if !matches!(f.as_str(), "helvetica" | "times") {
            return Err(invalid("font is helvetica or times"));
        }
        t.font = f;
    }
    if let Some(s) = a.show {
        t.show = s;
    }
    if let Some(p) = a.payment_instructions {
        t.payment_instructions = text(&p, 1000, "payment instructions")?;
    }
    if let Some(f) = a.footer {
        t.footer = text(&f, 500, "footer")?;
    }
    if let Some(o) = a.title_overrides {
        let mut out = BTreeMap::new();
        for (k, v) in o {
            if !DOC_KINDS.contains(&k.as_str()) {
                return Err(invalid(format!(
                    "title overrides name {}",
                    DOC_KINDS.join(", ")
                )));
            }
            if let Some(v) = text(&v, 40, "title")? {
                out.insert(k, v);
            }
        }
        t.title_overrides = out;
    }
    Ok(())
}

pub(crate) fn template_json(t: &Template) -> Value {
    json!({
        "id": t.id, "name": t.name, "accent_color": t.accent_color, "font": t.font,
        "show": t.show, "payment_instructions": t.payment_instructions, "footer": t.footer,
        "title_overrides": t.title_overrides,
        "logo": t.logo.as_ref().map(|l| json!({"width": l.width, "height": l.height})),
    })
}

/// `template.create` from definition-file fields.
pub(crate) fn create_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    create(ctx, crate::parse_args(args)?)
}

/// `template.update` from definition-file fields.
pub(crate) fn update_json(ctx: &Ctx<'_>, args: Value) -> CommandResult<Value> {
    update(ctx, crate::parse_args(args)?)
}

fn create(ctx: &Ctx<'_>, a: TemplateArgs) -> CommandResult<Value> {
    if a.name.is_none() {
        return Err(invalid("give the name"));
    }
    let mut t = Template {
        id: mk_core::Id::<()>::new().to_string(),
        name: String::new(),
        accent_color: None,
        font: "helvetica".into(),
        show: Show::default(),
        payment_instructions: None,
        footer: None,
        title_overrides: BTreeMap::new(),
        logo: None,
        created_at: now_rfc3339()?,
    };
    apply(ctx, &mut t, a)?;
    save(ctx, &t)?;
    Ok(template_json(&t))
}

fn update(ctx: &Ctx<'_>, mut a: TemplateArgs) -> CommandResult<Value> {
    let r = a
        .template
        .take()
        .ok_or_else(|| invalid("give the template"))?;
    let mut t = find(ctx, &r)?;
    apply(ctx, &mut t, a)?;
    save(ctx, &t)?;
    Ok(template_json(&t))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TemplateRef {
    template: String,
}

fn delete(ctx: &Ctx<'_>, a: TemplateRef) -> CommandResult<Value> {
    let t = find(ctx, &a.template)?;
    ctx.save_doc(DocTable::Template, &t.id, None)?;
    if crate::module_settings::get(ctx, "invoicing.default_template")?.as_str()
        == Some(t.id.as_str())
    {
        crate::module_settings::put(ctx, "invoicing.default_template", Value::Null)?;
    }
    Ok(json!({"deleted": t.id}))
}

fn list(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    Ok(Value::Array(
        templates(ctx)?.iter().map(template_json).collect(),
    ))
}

fn get(ctx: &Ctx<'_>, a: TemplateRef) -> CommandResult<Value> {
    let t = find(ctx, &a.template)?;
    let mut v = template_json(&t);
    // A small preview of the stored raster for the editor: RGB, at most 240 pixels
    // wide or 120 high.
    if let (Some(l), Some(o)) = (&t.logo, v.as_object_mut())
        && let Some(p) = logo_preview(l, 240, 120)
    {
        o.insert("logo_preview".into(), p);
    }
    Ok(v)
}

/// The stored logo scaled down (nearest pixel) to fit `max_w` × `max_h`.
fn logo_preview(l: &StoredLogo, max_w: u32, max_h: u32) -> Option<Value> {
    let zipped = base64::engine::general_purpose::STANDARD
        .decode(&l.rgb_zlib)
        .ok()?;
    let full = (l.width as usize)
        .checked_mul(l.height as usize)?
        .checked_mul(3)?;
    let rgb = mk_import::image::inflate(&zipped, full).ok()?;
    if rgb.len() != full || l.width == 0 || l.height == 0 {
        return None;
    }
    // Scale = max(width / max_w, height / max_h), rounded up, at least 1.
    let step = l.width.div_ceil(max_w).max(l.height.div_ceil(max_h)).max(1);
    let (w, h) = (l.width.div_ceil(step), l.height.div_ceil(step));
    let mut out = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for y in 0..h {
        for x in 0..w {
            let sx = (x * step).min(l.width - 1) as usize;
            let sy = (y * step).min(l.height - 1) as usize;
            let i = (sy * l.width as usize + sx) * 3;
            out.extend_from_slice(rgb.get(i..i + 3)?);
        }
    }
    Some(json!({"width": w, "height": h,
        "rgb_base64": base64::engine::general_purpose::STANDARD.encode(out)}))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LogoArgs {
    template: String,
    /// The PNG or JPEG file, base64.
    content_base64: String,
}

fn set_logo(ctx: &Ctx<'_>, a: LogoArgs) -> CommandResult<Value> {
    let mut t = find(ctx, &a.template)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(a.content_base64.trim())
        .map_err(|e| invalid(format!("content_base64 is not valid base64: {e}")))?;
    let rgb = mk_import::image::decode_logo(&bytes).map_err(|e| invalid(e.to_string()))?;
    t.logo = Some(StoredLogo {
        width: rgb.width,
        height: rgb.height,
        rgb_zlib: base64::engine::general_purpose::STANDARD
            .encode(mk_import::image::deflate(&rgb.data)),
    });
    // The audit entry records the file's size, not its content.
    *ctx.audit_args.borrow_mut() = Some(json!({"template": t.id, "bytes": bytes.len()}));
    save(ctx, &t)?;
    Ok(template_json(&t))
}

fn clear_logo(ctx: &Ctx<'_>, a: TemplateRef) -> CommandResult<Value> {
    let mut t = find(ctx, &a.template)?;
    t.logo = None;
    save(ctx, &t)?;
    Ok(template_json(&t))
}

/// The template a document uses: its own, else the book's default, else none.
pub(crate) fn for_document(ctx: &Ctx<'_>, named: Option<&str>) -> CommandResult<Option<Template>> {
    if let Some(n) = named.filter(|n| !n.trim().is_empty()) {
        return find(ctx, n).map(Some);
    }
    if !crate::sales::module_on(ctx, crate::sales::INVOICING)? {
        return Ok(None);
    }
    Ok(
        match crate::module_settings::get(ctx, "invoicing.default_template")?.as_str() {
            Some(id) => find(ctx, id).ok(),
            None => None,
        },
    )
}

/// The PDF style of a template.
pub(crate) fn style(t: Option<&Template>) -> crate::pdf::Style {
    let Some(t) = t else {
        return crate::pdf::Style::default();
    };
    crate::pdf::Style {
        accent: t.accent_color.as_deref().and_then(parse_color),
        times: t.font == "times",
        footer: t.footer.clone(),
        logo: t.logo.as_ref().and_then(|l| {
            base64::engine::general_purpose::STANDARD
                .decode(&l.rgb_zlib)
                .ok()
                .map(|flate_rgb| crate::pdf::Logo {
                    width: l.width,
                    height: l.height,
                    flate_rgb,
                })
        }),
    }
}

/// A document's title under a template.
pub(crate) fn title(t: Option<&Template>, kind: &str, default: &str) -> String {
    t.and_then(|t| t.title_overrides.get(kind).cloned())
        .unwrap_or_else(|| default.to_owned())
}
