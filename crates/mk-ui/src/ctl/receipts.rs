//! Receipts in the UI (app-desktop.md revision 28): thumbnail strips, the large
//! viewer with zoom and rotate-for-viewing, adding files (dialog or drag and drop)
//! with the duplicate question, and PDFs opened in the system viewer after a
//! confirmation naming the file. Every change goes through `receipt.*` commands.

use std::collections::HashMap;
use std::rc::Rc;

use base64::Engine;
use serde_json::{Value, json};

use super::ui::{Picture, Ui};
use super::util::{Json, png_picture};
use super::{App, AppEvent};
use crate::{PickedFile, ReceiptTarget};

/// Thumbnail edge in pixels.
const TILE: i32 = 64;
/// Longer side of the picture the viewer shows.
const VIEW_SIDE: u32 = 1600;

#[derive(Debug, Clone)]
pub(crate) struct Viewer {
    pub id: String,
    pub file_name: String,
    /// Percent of the fitted size.
    pub zoom: u32,
    /// Quarter turns clockwise, for viewing only.
    pub quarter: u8,
}

#[derive(Debug, Clone)]
pub(crate) struct Pending {
    target: ReceiptTarget,
    name: String,
    bytes: Vec<u8>,
    message: String,
    rest: Vec<PickedFile>,
}

#[derive(Debug, Default)]
pub(crate) struct ReceiptsState {
    thumbs: HashMap<String, Option<Rc<Picture>>>,
    large: Option<(String, u8, Option<Rc<Picture>>)>,
    pub viewer: Option<Viewer>,
    pub confirm_pdf: Option<(String, String)>,
    pub dup: Option<Pending>,
}

fn picture_of(app: &mut App, id: &str, max_side: Option<u32>) -> Option<Rc<Picture>> {
    let mut args = json!({"receipt": id});
    if let (Some(m), Some(o)) = (max_side, args.as_object_mut()) {
        o.insert("max_side".into(), json!(m));
    }
    app.backend
        .invoke("receipt.thumbnail", args)
        .ok()
        .and_then(|v| {
            v.get("png_base64")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .and_then(|png| png_picture(&png, max_side.unwrap_or(512).max(512)))
}

/// A receipt's thumbnail (cached; `None` for PDFs).
pub(crate) fn thumbnail(app: &mut App, id: &str) -> Option<Rc<Picture>> {
    if let Some(t) = app.receipts.thumbs.get(id) {
        return t.clone();
    }
    let pic = picture_of(app, id, None);
    app.receipts.thumbs.insert(id.to_owned(), pic.clone());
    pic
}

/// What the user did in a strip.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StripAction {
    View(Value),
    Remove(String),
}

/// One thumbnail tile: the picture, or "PDF"; returns whether it was clicked.
pub(crate) fn tile(app: &mut App, ui: &mut Ui, r: &Value, name: &str) -> bool {
    let pic = thumbnail(app, r.str("id"));
    ui.tip(name);
    match pic {
        Some(p) => ui
            .picture(&format!("Receipt thumbnail {name}"), p, TILE)
            .clicked(),
        None => {
            let label = if r.str("kind") == "pdf" {
                format!("PDF receipt {name}")
            } else {
                format!("Receipt {name}")
            };
            ui.named(label);
            ui.button(if r.str("kind") == "pdf" { "PDF" } else { "?" })
                .clicked()
        }
    }
}

/// Thumbnails of `items` in a row; each opens the viewer, with View and (when
/// `removable`) Remove beside it.
pub(crate) fn strip(
    app: &mut App,
    ui: &mut Ui,
    items: &[Value],
    removable: bool,
) -> Option<StripAction> {
    let mut act = None;
    ui.wrap(|ui| {
        for (n, r) in items.iter().enumerate() {
            let name = r.str("file_name").to_owned();
            if tile(app, ui, r, &name) {
                act = Some(StripAction::View(r.clone()));
            }
            ui.named(format!("View receipt {}", n + 1));
            if ui.button("View").clicked() {
                act = Some(StripAction::View(r.clone()));
            }
            if removable {
                ui.named(format!("Remove receipt {}", n + 1));
                if ui.button("Remove").clicked() {
                    act = Some(StripAction::Remove(r.str("id").to_owned()));
                }
            }
        }
    });
    act
}

/// Open a receipt: pictures in the viewer, PDFs after a confirmation.
pub(crate) fn view(app: &mut App, r: &Value) {
    let id = r.str("id").to_owned();
    let name = r.str("file_name").to_owned();
    if r.str("kind") == "pdf" {
        app.receipts.confirm_pdf = Some((id, name));
    } else {
        app.receipts.viewer = Some(Viewer {
            id,
            file_name: name,
            zoom: 100,
            quarter: 0,
        });
    }
}

fn target_args(app: &App, target: ReceiptTarget) -> Option<Value> {
    match target {
        ReceiptTarget::Inbox => Some(json!({})),
        ReceiptTarget::ExpenseForm => app
            .expenses
            .form
            .as_ref()
            .and_then(|f| f.id.clone())
            .map(|id| json!({"expense": id})),
        ReceiptTarget::TransactionPopup => app
            .register
            .popup
            .as_ref()
            .and_then(|p| p.entry.editing.clone())
            .map(|t| json!({"transaction": t})),
    }
}

fn keep_pending(app: &mut App, target: ReceiptTarget, id: &str) {
    match target {
        ReceiptTarget::ExpenseForm => {
            if let Some(f) = app.expenses.form.as_mut() {
                f.pending.push(id.to_owned());
            }
        }
        ReceiptTarget::TransactionPopup => {
            if let Some(p) = app.register.popup.as_mut() {
                p.receipts_pending.push(id.to_owned());
            }
        }
        ReceiptTarget::Inbox => {}
    }
}

/// Add files as receipts to `target`, one by one; a duplicate stops to ask.
pub(crate) fn add_files(app: &mut App, target: ReceiptTarget, files: Vec<PickedFile>) {
    let mut queue = files.into_iter();
    let mut added = 0usize;
    while let Some(f) = queue.next() {
        let bytes = match f.bytes {
            Ok(b) => b,
            Err(msg) => {
                app.notify_error(&crate::UiError::new(
                    "invalid_args",
                    format!("{}: {msg}", f.name),
                ));
                continue;
            }
        };
        match add_one(app, target, &f.name, &bytes, false) {
            Ok(()) => added += 1,
            Err(e) if e.code == "duplicate_receipt" => {
                app.receipts.dup = Some(Pending {
                    target,
                    name: f.name,
                    bytes,
                    message: e.message,
                    rest: queue.collect(),
                });
                return;
            }
            Err(e) => app.notify_error(&crate::UiError::new(
                &e.code,
                format!("{}: {}", f.name, e.message),
            )),
        }
    }
    if added > 0 && target == ReceiptTarget::Inbox {
        let s = if added == 1 { "" } else { "s" };
        app.notify(format!("Added {added} receipt{s} to the inbox."));
    }
}

fn add_one(
    app: &mut App,
    target: ReceiptTarget,
    name: &str,
    bytes: &[u8],
    allow_duplicate: bool,
) -> Result<(), crate::UiError> {
    let attach = target_args(app, target);
    let mut args = attach.clone().unwrap_or_else(|| json!({}));
    if let Some(o) = args.as_object_mut() {
        o.insert(
            "content_base64".into(),
            json!(base64::engine::general_purpose::STANDARD.encode(bytes)),
        );
        o.insert("file_name".into(), json!(name));
        o.insert("source".into(), json!("desktop"));
        o.insert("allow_duplicate".into(), json!(allow_duplicate));
    }
    let r = app.run("receipt.add", args)?;
    if attach.is_none() {
        keep_pending(app, target, r.str("id"));
    }
    Ok(())
}

/// Where dropped files go now: the open expense pop-up, the transaction pop-up,
/// or the Expenses inbox.
pub(crate) fn drop_target(app: &App) -> Option<ReceiptTarget> {
    if app.expenses.form.is_some() {
        return Some(ReceiptTarget::ExpenseForm);
    }
    if app.register.popup.is_some() {
        return Some(ReceiptTarget::TransactionPopup);
    }
    matches!(app.view, crate::View::Expenses).then_some(ReceiptTarget::Inbox)
}

/// The viewer, the PDF confirmation and the duplicate question.
pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    duplicate_dialog(app, ui);
    pdf_dialog(app, ui);
    viewer(app, ui);
}

fn duplicate_dialog(app: &mut App, ui: &mut Ui) {
    let Some(p) = app.receipts.dup.clone() else {
        return;
    };
    let mut choice = ui.dialog_closed("receipt-duplicate").then_some(false);
    ui.dialog("receipt-duplicate", "Receipt already added", 440, |ui| {
        ui.label(p.message.clone());
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                choice = Some(false);
            }
            if ui.primary("Attach anyway").clicked() {
                choice = Some(true);
            }
        });
    });
    let Some(attach) = choice else {
        return;
    };
    app.receipts.dup = None;
    if attach && let Err(e) = add_one(app, p.target, &p.name, &p.bytes, true) {
        app.notify_error(&e);
    }
    if !p.rest.is_empty() {
        add_files(app, p.target, p.rest);
    }
}

fn pdf_dialog(app: &mut App, ui: &mut Ui) {
    let Some((id, name)) = app.receipts.confirm_pdf.clone() else {
        return;
    };
    let mut choice = ui.dialog_closed("receipt-open-pdf").then_some(false);
    ui.dialog("receipt-open-pdf", "Open PDF receipt", 460, |ui| {
        ui.label(format!(
            "Open {name} in your PDF viewer? MoneyKeep never shows PDF files itself."
        ));
        // expenses.md revision 2: the viewer gets a decrypted copy on disk.
        ui.label("A temporary unencrypted copy will be opened.");
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                choice = Some(false);
            }
            if ui.primary("Open").clicked() {
                choice = Some(true);
            }
        });
    });
    let Some(open) = choice else {
        return;
    };
    app.receipts.confirm_pdf = None;
    if !open {
        return;
    }
    let Some(v) = app.read("receipt.get", json!({"receipt": id})) else {
        return;
    };
    match base64::engine::general_purpose::STANDARD.decode(v.str("content_base64")) {
        Ok(data) => app.emit(AppEvent::OpenReceiptPdf {
            file_name: name,
            data,
        }),
        Err(_) => app.notify("This PDF can't be read."),
    }
}

/// The picture turned by `quarter` quarter turns clockwise.
fn turned(p: &Picture, quarter: u8) -> Picture {
    let (w, h) = (p.width as usize, p.height as usize);
    let px = |x: usize, y: usize| -> [u8; 4] {
        let i = (y * w + x) * 4;
        match p.rgba.get(i..i + 4) {
            Some(&[r, g, b, a]) => [r, g, b, a],
            _ => [0, 0, 0, 0],
        }
    };
    let (nw, nh) = if quarter % 2 == 1 { (h, w) } else { (w, h) };
    let mut out = Vec::with_capacity(nw * nh * 4);
    for y in 0..nh {
        for x in 0..nw {
            let c = match quarter % 4 {
                1 => px(y, h - 1 - x),
                2 => px(w - 1 - x, h - 1 - y),
                3 => px(w - 1 - y, x),
                _ => px(x, y),
            };
            out.extend_from_slice(&c);
        }
    }
    Picture {
        width: u32::try_from(nw).unwrap_or(0),
        height: u32::try_from(nh).unwrap_or(0),
        rgba: out,
    }
}

fn large(app: &mut App, id: &str, quarter: u8) -> Option<Rc<Picture>> {
    if let Some((cached, q, p)) = &app.receipts.large
        && cached == id
        && *q == quarter
    {
        return p.clone();
    }
    let pic = picture_of(app, id, Some(VIEW_SIDE)).map(|p| {
        if quarter == 0 {
            p
        } else {
            Rc::new(turned(&p, quarter))
        }
    });
    app.receipts.large = Some((id.to_owned(), quarter, pic.clone()));
    pic
}

fn viewer(app: &mut App, ui: &mut Ui) {
    let Some(mut v) = app.receipts.viewer.clone() else {
        return;
    };
    let pic = large(app, &v.id, v.quarter);
    let max_w = (app.window_width - 200).max(240);
    let max_h = (app.window_height - 260).max(200);
    let mut close = ui.dialog_closed("receipt-viewer");
    ui.dialog(
        "receipt-viewer",
        &format!("Receipt: {}", v.file_name),
        max_w + 48,
        |ui| {
            ui.close_label("Close");
            ui.row(|ui| {
                if ui.button("Zoom out").clicked() {
                    v.zoom = v.zoom.saturating_sub(25).max(25);
                }
                ui.label(format!("{} %", v.zoom));
                if ui.button("Zoom in").clicked() {
                    v.zoom = (v.zoom + 25).min(400);
                }
                if ui.button("Fit").clicked() {
                    v.zoom = 100;
                }
                ui.tip("Turn the picture for viewing (the stored receipt is not changed)");
                if ui.button("Rotate").clicked() {
                    v.quarter = (v.quarter + 1) % 4;
                }
            });
            match &pic {
                Some(p) => {
                    let (w, h) = (i64::from(p.width.max(1)), i64::from(p.height.max(1)));
                    // Fitted (never enlarged), then zoomed.
                    let fit_w = (i64::from(max_w)).min(w).min(w * i64::from(max_h) / h);
                    let shown = fit_w * i64::from(v.zoom) / 100;
                    ui.row(|ui| {
                        ui.picture(
                            "Receipt picture",
                            p.clone(),
                            i32::try_from(shown).unwrap_or(max_w),
                        );
                    });
                }
                None => ui.label("This picture can't be shown."),
            }
            ui.actions(|ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        },
    );
    app.receipts.viewer = if close { None } else { Some(v) };
}

/// Forget cached pictures (after receipts change).
pub(crate) fn forget(app: &mut App, id: &str) {
    app.receipts.thumbs.remove(id);
}
