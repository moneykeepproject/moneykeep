//! The browser layer: Fetch to the page's own origin, IndexedDB for the outbox and
//! token, `<input type="file">` for the camera and photo picker, and a canvas for
//! scaling and re-encoding photos (HTML Living Standard, Fetch and IndexedDB
//! specifications; wasm-bindgen / web-sys documentation).
//!
//! Callbacks run on the browser's event loop and leave their results in a shared
//! queue that the UI frame reads through [`Platform`].

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::legacy;
use crate::outbox::Storage;
use crate::photo;
use crate::ui::{PickedPhoto, Platform, Request, Response};

const DB_NAME: &str = "moneykeep-capture";
/// The page's object for the app (web/shell.js): the install offer and the hooks.
const CAPTURE: &str = "mkCapture";
const STORE: &str = "kv";
/// The `localStorage` marker that the phone was paired here before.
const PAIRED_KEY: &str = "mk-paired";
/// Revision 18: what web/shell.js's loading screen reads (its `WAY_KEY` and
/// `CLOUDFLARE_KEY`).
const WAY_PAGE_KEY: &str = "moneykeep-capture-way";
const CLOUDFLARE_PAGE_KEY: &str = "moneykeep-capture-cloudflare";
/// An API call with no answer by then is aborted and counts as "computer not
/// reachable" (mobile-capture.md revision 14); a photo upload gets longer.
const TIMEOUT_MS: i32 = 30_000;
const UPLOAD_TIMEOUT_MS: i32 = 120_000;

#[derive(Default)]
struct Shared {
    next: u64,
    answers: Vec<(u64, Response)>,
    photos: Vec<Result<PickedPhoto, String>>,
    loaded: Option<Vec<(String, String)>>,
    db: Option<web_sys::IdbDatabase>,
    /// Writes made before the database opened.
    queued: Vec<(String, Option<String>)>,
    /// Confirmed writes made before the database opened: (id, key, value).
    queued_confirmed: Vec<(u64, String, String)>,
    /// Finished confirmed writes.
    saved: Vec<(u64, Result<(), String>)>,
    /// IndexedDB could not be opened (private browsing on some browsers).
    no_db: bool,
    persisted: Option<bool>,
}

/// IndexedDB key/value storage (writes are fire-and-forget).
pub struct WebStore {
    shared: Rc<RefCell<Shared>>,
}

impl std::fmt::Debug for WebStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebStore")
    }
}

fn write(db: &web_sys::IdbDatabase, key: &str, value: Option<&str>) {
    let Ok(tx) = db.transaction_with_str_and_mode(STORE, web_sys::IdbTransactionMode::Readwrite)
    else {
        return;
    };
    let Ok(store) = tx.object_store(STORE) else {
        return;
    };
    let k = JsValue::from_str(key);
    let _ = match value {
        Some(v) => store.put_with_key(&JsValue::from_str(v), &k),
        None => store.delete(&k),
    };
}

/// A write whose completion is reported: the transaction's `complete` event (the
/// data is committed), or `error`/`abort`.
fn write_confirmed(
    shared: &Rc<RefCell<Shared>>,
    db: &web_sys::IdbDatabase,
    id: u64,
    key: &str,
    value: &str,
) {
    let fail = |shared: &Rc<RefCell<Shared>>, m: &str| {
        shared.borrow_mut().saved.push((id, Err(m.to_owned())));
    };
    let Ok(tx) = db.transaction_with_str_and_mode(STORE, web_sys::IdbTransactionMode::Readwrite)
    else {
        fail(shared, "no transaction");
        return;
    };
    let Ok(store) = tx.object_store(STORE) else {
        fail(shared, "no store");
        return;
    };
    let done = shared.clone();
    tx.set_oncomplete(Some(&on(move || {
        done.borrow_mut().saved.push((id, Ok(())));
    })));
    let err = shared.clone();
    tx.set_onerror(Some(&on(move || {
        err.borrow_mut()
            .saved
            .push((id, Err("write failed".into())));
    })));
    let abort = shared.clone();
    tx.set_onabort(Some(&on(move || {
        abort
            .borrow_mut()
            .saved
            .push((id, Err("write aborted".into())));
    })));
    if store
        .put_with_key(&JsValue::from_str(value), &JsValue::from_str(key))
        .is_err()
    {
        fail(shared, "put refused");
    }
}

impl Storage for WebStore {
    fn put(&mut self, key: &str, value: &str) {
        let mut s = self.shared.borrow_mut();
        match &s.db {
            Some(db) => write(db, key, Some(value)),
            None => s.queued.push((key.to_owned(), Some(value.to_owned()))),
        }
    }
    fn delete(&mut self, key: &str) {
        let mut s = self.shared.borrow_mut();
        match &s.db {
            Some(db) => write(db, key, None),
            None => s.queued.push((key.to_owned(), None)),
        }
    }
}

/// The browser behind the phone app.
pub struct WebPlatform {
    shared: Rc<RefCell<Shared>>,
    store: WebStore,
}

impl std::fmt::Debug for WebPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebPlatform")
    }
}

impl WebPlatform {
    pub fn new() -> Self {
        let shared = Rc::new(RefCell::new(Shared::default()));
        open_db(shared.clone());
        Self {
            store: WebStore {
                shared: shared.clone(),
            },
            shared,
        }
    }
}

impl Default for WebPlatform {
    fn default() -> Self {
        Self::new()
    }
}

/// A JS callback for a request event, leaked for the request's lifetime (one per
/// request; requests are few).
fn on(f: impl FnOnce() + 'static) -> js_sys::Function {
    Closure::once_into_js(f).unchecked_into()
}

fn open_db(shared: Rc<RefCell<Shared>>) {
    let fail = {
        let shared = shared.clone();
        move || {
            // No IndexedDB (private mode on some browsers): work in memory, and
            // say so when the pairing is saved.
            let mut s = shared.borrow_mut();
            s.no_db = true;
            s.loaded = Some(Vec::new());
            for (id, _, _) in std::mem::take(&mut s.queued_confirmed) {
                s.saved.push((id, Err("no IndexedDB".into())));
            }
        }
    };
    let Some(factory) = web_sys::window().and_then(|w| w.indexed_db().ok().flatten()) else {
        fail();
        return;
    };
    let Ok(req) = factory.open_with_u32(DB_NAME, 1) else {
        fail();
        return;
    };
    let upgrade_req = req.clone();
    req.set_onupgradeneeded(Some(&on(move || {
        if let Ok(db) = upgrade_req.result() {
            let db: web_sys::IdbDatabase = db.unchecked_into();
            let _ = db.create_object_store(STORE);
        }
    })));
    let ok_req = req.clone();
    let ok_shared = shared.clone();
    req.set_onsuccess(Some(&on(move || {
        let Ok(db) = ok_req.result() else {
            ok_shared.borrow_mut().loaded = Some(Vec::new());
            return;
        };
        let db: web_sys::IdbDatabase = db.unchecked_into();
        read_all(ok_shared.clone(), db);
    })));
    req.set_onerror(Some(&on(fail)));
}

fn read_all(shared: Rc<RefCell<Shared>>, db: web_sys::IdbDatabase) {
    let finish = {
        let shared = shared.clone();
        let db = db.clone();
        move |pairs: Vec<(String, String)>| {
            let confirmed = {
                let mut s = shared.borrow_mut();
                for (k, v) in std::mem::take(&mut s.queued) {
                    write(&db, &k, v.as_deref());
                }
                s.db = Some(db.clone());
                s.loaded = Some(pairs);
                std::mem::take(&mut s.queued_confirmed)
            };
            for (id, k, v) in confirmed {
                write_confirmed(&shared, &db, id, &k, &v);
            }
        }
    };
    let target = db.clone();
    read_pairs(&db, move |pairs| {
        if !legacy::should_copy(&pairs) {
            finish(pairs);
            return;
        }
        // First start under the new name (branding.md step 7): copy the pairing and
        // the outbox from the database named before the rename, if there is one.
        read_old(move |old| {
            let mut writer = DbWriter(&target);
            finish(legacy::copy_old(pairs, old, &mut writer));
        });
    });
}

/// Writes straight into an open database.
struct DbWriter<'a>(&'a web_sys::IdbDatabase);

impl Storage for DbWriter<'_> {
    fn put(&mut self, key: &str, value: &str) {
        write(self.0, key, Some(value));
    }
    fn delete(&mut self, key: &str) {
        write(self.0, key, None);
    }
}

/// The entries of the database named before the rename; none when it does not
/// exist (it is not created: its creation is aborted) or cannot be read. The old
/// database is left as it was.
fn read_old(done: impl FnOnce(Vec<(String, String)>) + 'static) {
    let done = Rc::new(RefCell::new(Some(done)));
    let give = {
        let done = done.clone();
        move |pairs: Vec<(String, String)>| {
            if let Some(f) = done.borrow_mut().take() {
                f(pairs);
            }
        }
    };
    let Some(factory) = web_sys::window().and_then(|w| w.indexed_db().ok().flatten()) else {
        give(Vec::new());
        return;
    };
    let Ok(req) = factory.open_with_u32(legacy::OLD_DB_NAME, 1) else {
        give(Vec::new());
        return;
    };
    let upgrade_req = req.clone();
    req.set_onupgradeneeded(Some(&on(move || {
        // No such database: abort, so it is not created.
        if let Some(tx) = upgrade_req.transaction() {
            let _ = tx.abort();
        }
    })));
    let ok_req = req.clone();
    let ok_give = give.clone();
    req.set_onsuccess(Some(&on(move || {
        let Ok(db) = ok_req.result() else {
            ok_give(Vec::new());
            return;
        };
        let db: web_sys::IdbDatabase = db.unchecked_into();
        let close = db.clone();
        read_pairs(&db, move |pairs| {
            close.close();
            ok_give(pairs);
        });
    })));
    req.set_onerror(Some(&on(move || give(Vec::new()))));
}

/// All key/value pairs of a database's store.
fn read_pairs(db: &web_sys::IdbDatabase, done: impl FnOnce(Vec<(String, String)>) + 'static) {
    let store = db
        .transaction_with_str(STORE)
        .and_then(|tx| tx.object_store(STORE));
    let Ok(store) = store else {
        done(Vec::new());
        return;
    };
    let (Ok(keys_req), Ok(vals_req)) = (store.get_all_keys(), store.get_all()) else {
        done(Vec::new());
        return;
    };
    // Both requests complete in order within the transaction; the values arrive last.
    let keys_holder = keys_req.clone();
    let vals_holder = vals_req.clone();
    vals_req.set_onsuccess(Some(&on(move || {
        let keys = keys_holder
            .result()
            .ok()
            .map(|v| js_sys::Array::from(&v))
            .unwrap_or_default();
        let vals = vals_holder
            .result()
            .ok()
            .map(|v| js_sys::Array::from(&v))
            .unwrap_or_default();
        let pairs = keys
            .iter()
            .zip(vals.iter())
            .filter_map(|(k, v)| Some((k.as_string()?, v.as_string()?)))
            .collect();
        done(pairs);
    })));
}

async fn fetch(req: Request) -> Response {
    let fail = Response {
        status: 0,
        body: Vec::new(),
    };
    let Some(window) = web_sys::window() else {
        return fail;
    };
    let init = web_sys::RequestInit::new();
    init.set_method(req.method);
    // Cloudflare Access answers an ended session with a redirect to its sign-in
    // page; seeing it (instead of following it across origins) lets the app ask
    // the user to sign in again while keeping the pairing.
    init.set_redirect(web_sys::RequestRedirect::Manual);
    let Ok(headers) = web_sys::Headers::new() else {
        return fail;
    };
    if let Some(ct) = req.content_type {
        let _ = headers.set("Content-Type", ct);
    }
    if let Some(t) = &req.bearer {
        let _ = headers.set("Authorization", &format!("Bearer {t}"));
    }
    init.set_headers(&headers);
    if let Some(b) = &req.body {
        let arr = js_sys::Uint8Array::from(b.as_slice());
        init.set_body(&arr);
    }
    // Revision 14: a timeout aborts the request (the answer and its body), which
    // then fails like no answer at all.
    let abort = web_sys::AbortController::new().ok();
    if let Some(a) = &abort {
        init.set_signal(Some(&a.signal()));
    }
    let limit = if req.content_type == Some("image/jpeg") {
        UPLOAD_TIMEOUT_MS
    } else {
        TIMEOUT_MS
    };
    let timer = abort.clone().and_then(|a| {
        window
            .set_timeout_with_callback_and_timeout_and_arguments_0(&on(move || a.abort()), limit)
            .ok()
    });
    let resp = fetch_answer(&window, &req, &init).await;
    if let Some(t) = timer {
        window.clear_timeout_with_handle(t);
    }
    resp.unwrap_or(fail)
}

async fn fetch_answer(
    window: &web_sys::Window,
    req: &Request,
    init: &web_sys::RequestInit,
) -> Option<Response> {
    let request = web_sys::Request::new_with_str_and_init(&req.path, init).ok()?;
    let resp = JsFuture::from(window.fetch_with_request(&request))
        .await
        .ok()?;
    let resp: web_sys::Response = resp.unchecked_into();
    if resp.type_() == web_sys::ResponseType::Opaqueredirect {
        return Some(Response {
            status: 302,
            body: Vec::new(),
        });
    }
    let status = resp.status();
    let body = match resp.array_buffer() {
        Ok(p) => match JsFuture::from(p).await {
            Ok(buf) => js_sys::Uint8Array::new(&buf).to_vec(),
            // Aborted while the body arrived (the timeout): no answer.
            Err(_) if status == 200 || status == 201 => return None,
            Err(_) => Vec::new(),
        },
        Err(_) => Vec::new(),
    };
    Some(Response { status, body })
}

/// Wait for an element's `load` (or `error`) event.
async fn loaded(img: &web_sys::HtmlImageElement) -> bool {
    let img2 = img.clone();
    let p = js_sys::Promise::new(&mut |resolve, reject| {
        img2.set_onload(Some(&resolve));
        img2.set_onerror(Some(&reject));
    });
    JsFuture::from(p).await.is_ok()
}

/// Draw `img` at `w` × `h` on a new canvas.
fn draw(
    doc: &web_sys::Document,
    img: &web_sys::HtmlImageElement,
    w: u32,
    h: u32,
) -> Option<(
    web_sys::HtmlCanvasElement,
    web_sys::CanvasRenderingContext2d,
)> {
    let canvas: web_sys::HtmlCanvasElement = doc.create_element("canvas").ok()?.dyn_into().ok()?;
    canvas.set_width(w);
    canvas.set_height(h);
    let ctx: web_sys::CanvasRenderingContext2d = canvas.get_context("2d").ok()??.dyn_into().ok()?;
    ctx.draw_image_with_html_image_element_and_dw_and_dh(img, 0.0, 0.0, f64::from(w), f64::from(h))
        .ok()?;
    Some((canvas, ctx))
}

/// Scale a picked file to ≤ 2,400 px and re-encode it as JPEG (drops metadata), with
/// a small RGBA thumbnail.
async fn prepare(file: web_sys::File) -> Result<PickedPhoto, String> {
    let head = match file.slice_with_i32_and_i32(0, 16) {
        Ok(b) => JsFuture::from(b.array_buffer())
            .await
            .map(|buf| js_sys::Uint8Array::new(&buf).to_vec())
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    photo::check_type(&file.type_(), &file.name(), &head).map_err(str::to_owned)?;
    let bad = || mk_capture_api::msg::PHOTO_TYPE.to_owned();
    let doc = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(bad)?;
    let url = web_sys::Url::create_object_url_with_blob(&file).map_err(|_| bad())?;
    let img = web_sys::HtmlImageElement::new().map_err(|_| bad())?;
    let wait = loaded(&img);
    img.set_src(&url);
    let ok = wait.await;
    let result = async {
        if !ok || img.natural_width() == 0 || img.natural_height() == 0 {
            return Err(bad());
        }
        // Browsers draw images upright by their EXIF orientation.
        let (w, h) = photo::upload_size(img.natural_width(), img.natural_height());
        let (canvas, _) = draw(&doc, &img, w, h).ok_or_else(bad)?;
        let p = js_sys::Promise::new(&mut |resolve, reject| {
            let cb = Closure::once_into_js(move |blob: JsValue| {
                if blob.is_null() {
                    let _ = reject.call0(&JsValue::NULL);
                } else {
                    let _ = resolve.call1(&JsValue::NULL, &blob);
                }
            });
            let _ = canvas.to_blob_with_type_and_encoder_options(
                cb.unchecked_ref(),
                "image/jpeg",
                &JsValue::from_f64(photo::JPEG_QUALITY),
            );
        });
        let blob: web_sys::Blob = JsFuture::from(p).await.map_err(|_| bad())?.unchecked_into();
        let buf = JsFuture::from(blob.array_buffer())
            .await
            .map_err(|_| bad())?;
        let jpeg = js_sys::Uint8Array::new(&buf).to_vec();
        let (tw, th) = photo::thumb_size(w, h);
        let thumb = draw(&doc, &img, tw, th)
            .and_then(|(_, ctx)| {
                ctx.get_image_data(0.0, 0.0, f64::from(tw), f64::from(th))
                    .ok()
            })
            .map(|d| d.data().0)
            .unwrap_or_default();
        Ok(PickedPhoto {
            jpeg,
            width: w,
            height: h,
            thumb,
            thumb_w: tw,
            thumb_h: th,
        })
    }
    .await;
    let _ = web_sys::Url::revoke_object_url(&url);
    result
}

impl Platform for WebPlatform {
    fn now_ms(&self) -> i64 {
        // Milliseconds since 1970 fit an i64 exactly.
        js_sys::Date::now() as i64
    }

    fn today(&self) -> String {
        let d = js_sys::Date::new_0();
        format!(
            "{:04}-{:02}-{:02}",
            d.get_full_year(),
            d.get_month().saturating_add(1),
            d.get_date()
        )
    }

    fn put_confirmed(&mut self, key: &str, value: &str) -> u64 {
        let (id, db, no_db) = {
            let mut s = self.shared.borrow_mut();
            s.next += 1;
            (s.next, s.db.clone(), s.no_db)
        };
        match db {
            Some(db) => write_confirmed(&self.shared, &db, id, key, value),
            None if no_db => self
                .shared
                .borrow_mut()
                .saved
                .push((id, Err("no IndexedDB".into()))),
            None => self.shared.borrow_mut().queued_confirmed.push((
                id,
                key.to_owned(),
                value.to_owned(),
            )),
        }
        id
    }

    fn poll_saved(&mut self) -> Vec<(u64, Result<(), String>)> {
        std::mem::take(&mut self.shared.borrow_mut().saved)
    }

    fn request_persistence(&mut self) {
        let Some(storage) = web_sys::window().map(|w| w.navigator().storage()) else {
            return;
        };
        let Ok(promise) = storage.persist() else {
            return;
        };
        let shared = self.shared.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let kept = JsFuture::from(promise).await.ok().and_then(|v| v.as_bool());
            shared.borrow_mut().persisted = Some(kept.unwrap_or(false));
        });
    }

    fn persisted(&self) -> Option<bool> {
        self.shared.borrow().persisted
    }

    fn install(&self) -> crate::ui::Install {
        let Some(window) = web_sys::window() else {
            return crate::ui::Install::default();
        };
        // shell.js keeps the browser's install offer in `window.mkCapture`.
        let state = js_sys::Reflect::get(&window, &JsValue::from_str(CAPTURE)).ok();
        let flag = |name: &str| {
            state
                .as_ref()
                .and_then(|s| js_sys::Reflect::get(s, &JsValue::from_str(name)).ok())
                .is_some_and(|v| v.is_truthy())
        };
        let ua = window.navigator().user_agent().unwrap_or_default();
        let touch_mac = ua.contains("Macintosh") && window.navigator().max_touch_points() > 1;
        crate::ui::Install {
            installed: flag("standalone"),
            can_prompt: flag("installEvent"),
            ios: ua.contains("iPhone") || ua.contains("iPad") || ua.contains("iPod") || touch_mac,
        }
    }

    fn prompt_install(&mut self) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let install = js_sys::Reflect::get(&window, &JsValue::from_str(CAPTURE))
            .and_then(|s| js_sys::Reflect::get(&s, &JsValue::from_str("install")));
        if let Ok(f) = install.and_then(|f| f.dyn_into::<js_sys::Function>()) {
            let _ = f.call0(&JsValue::NULL);
        }
    }

    fn paired_before(&self) -> bool {
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .is_some_and(|s| {
                [PAIRED_KEY, legacy::OLD_PAIRED_KEY]
                    .iter()
                    .any(|k| s.get_item(k).ok().flatten().is_some())
            })
    }

    /// Revision 18: tell the page (web/shell.js) the first screen is drawn, on the
    /// frame after next, so the loading card fades over a drawn app.
    fn page_ready(&mut self) {
        let Some(w) = web_sys::window() else {
            return;
        };
        let w2 = w.clone();
        let second = on(move || {
            let call = on(|| {
                if let Some(window) = web_sys::window()
                    && let Ok(state) = js_sys::Reflect::get(&window, &JsValue::from_str(CAPTURE))
                    && let Ok(f) = js_sys::Reflect::get(&state, &JsValue::from_str("ready"))
                        .and_then(|f| f.dyn_into::<js_sys::Function>())
                {
                    let _ = f.call0(&state);
                }
            });
            let _ = w2.request_animation_frame(&call);
        });
        let _ = w.request_animation_frame(&second);
    }

    fn remember_way(&mut self, way: &str) {
        if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = s.set_item(WAY_PAGE_KEY, way);
        }
    }

    fn remember_cloudflare(&mut self, url: Option<&str>) {
        if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = match url {
                Some(u) => s.set_item(CLOUDFLARE_PAGE_KEY, u),
                None => s.remove_item(CLOUDFLARE_PAGE_KEY),
            };
        }
    }

    fn set_paired_before(&mut self) {
        if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = s.set_item(PAIRED_KEY, "1");
        }
    }

    /// mobile-capture.md revision 6: navigate to `/` (not a reload) so the page load
    /// reaches the network and Cloudflare Access can show its sign-in.
    fn go_to_start(&mut self) {
        if let Some(w) = web_sys::window() {
            let _ = w.location().assign("/");
        }
    }

    fn take_fragment(&mut self) -> Option<String> {
        let window = web_sys::window()?;
        let location = window.location();
        let hash = location.hash().ok().filter(|h| h.len() > 1)?;
        // Remove the code from the address bar and history.
        let clean = format!(
            "{}{}",
            location.pathname().unwrap_or_default(),
            location.search().unwrap_or_default()
        );
        if let Ok(history) = window.history() {
            let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&clean));
        }
        Some(hash)
    }

    fn online(&self) -> bool {
        web_sys::window().is_none_or(|w| w.navigator().on_line())
    }

    fn utc_offset_min(&self, ms: i64) -> i64 {
        // getTimezoneOffset is UTC minus local, in whole minutes.
        let d = js_sys::Date::new(&JsValue::from_f64(ms as f64));
        -(d.get_timezone_offset() as i64)
    }

    fn visible(&self) -> bool {
        // `document.hidden` (Page Visibility): false when the app is back on screen.
        web_sys::window()
            .and_then(|w| w.document())
            .is_none_or(|d| !d.hidden())
    }

    fn default_device_name(&self) -> String {
        let ua = web_sys::window()
            .and_then(|w| w.navigator().user_agent().ok())
            .unwrap_or_default();
        if ua.contains("iPhone") {
            "iPhone".into()
        } else if ua.contains("iPad") {
            "iPad".into()
        } else if ua.contains("Android") {
            "Android phone".into()
        } else {
            "Phone".into()
        }
    }

    fn random16(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        if let Some(c) = web_sys::window().and_then(|w| w.crypto().ok()) {
            let _ = c.get_random_values_with_u8_array(&mut b);
        }
        b
    }

    fn send(&mut self, req: Request) -> u64 {
        let id = {
            let mut s = self.shared.borrow_mut();
            s.next += 1;
            s.next
        };
        let shared = self.shared.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let r = fetch(req).await;
            shared.borrow_mut().answers.push((id, r));
        });
        id
    }

    fn poll(&mut self) -> Vec<(u64, Response)> {
        std::mem::take(&mut self.shared.borrow_mut().answers)
    }

    fn pick_photos(&mut self, camera: bool) {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let Some(input) = doc
            .create_element("input")
            .ok()
            .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            return;
        };
        input.set_type("file");
        input.set_accept("image/*");
        if camera {
            // HTML `capture`: the rear camera on phones that support it; elsewhere
            // a file picker.
            let _ = input.set_attribute("capture", "environment");
        } else {
            input.set_multiple(true);
        }
        let shared = self.shared.clone();
        let picker = input.clone();
        input.set_onchange(Some(&on(move || {
            let Some(files) = picker.files() else { return };
            for i in 0..files.length() {
                if let Some(f) = files.get(i) {
                    let shared = shared.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let r = prepare(f).await;
                        shared.borrow_mut().photos.push(r);
                    });
                }
            }
        })));
        input.click();
    }

    fn poll_photos(&mut self) -> Vec<Result<PickedPhoto, String>> {
        std::mem::take(&mut self.shared.borrow_mut().photos)
    }

    fn storage(&mut self) -> &mut dyn Storage {
        &mut self.store
    }

    fn loaded(&mut self) -> Option<Vec<(String, String)>> {
        self.shared.borrow_mut().loaded.take()
    }
}

/// Call `window.mkCapture[method](f)` (web/shell.js), if the page has it.
fn hook(method: &str, f: &JsValue) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(state) = js_sys::Reflect::get(&window, &JsValue::from_str(CAPTURE)) else {
        return;
    };
    if let Ok(g) = js_sys::Reflect::get(&state, &JsValue::from_str(method))
        .and_then(|g| g.dyn_into::<js_sys::Function>())
    {
        let _ = g.call1(&state, f);
    }
}

/// A CSS length from the page (a finite, non-negative number of pixels).
fn css_px(v: f64) -> f32 {
    if v.is_finite() && v > 0.0 {
        // CSS sizes of a phone's viewport fit an f32 exactly enough.
        v as f32
    } else {
        0.0
    }
}

/// Connect the window to the page (mobile-capture.md revision 9), before it is
/// shown:
///
/// - **Size.** The window gets the viewport's size now, so Slint never sizes the
///   canvas to the window's preferred size when it shows (it does that for a window
///   without a size of its own, writing it into the canvas's inline style — the
///   cause of the app stopping short of the right and bottom edges); after that it
///   follows the canvas's CSS size and the safe-area insets whenever the page
///   reports a change (`mkCapture.onResize`: a `ResizeObserver` on the canvas and
///   the viewport's resize events). The drawing buffer follows the device pixel
///   ratio through the web backend's own observer of the canvas's device-pixel
///   size.
/// - **Install offer.** `mkCapture.listen` redraws when the browser's install
///   offer arrives or goes, so **Install app** shows at once.
pub fn connect<A: mk_views::phone::PageApp + 'static>(view: &mk_views::phone::PhoneView<A>) {
    use mk_views::slint::ComponentHandle;
    if let Some(w) = web_sys::window() {
        let num = |v: Result<JsValue, JsValue>| v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
        let (width, height) = (css_px(num(w.inner_width())), css_px(num(w.inner_height())));
        if width > 0.0 && height > 0.0 {
            view.window()
                .window()
                .set_size(mk_views::slint::LogicalSize::new(width, height));
        }
    }
    let resized = view.clone();
    let on_resize = Closure::<dyn FnMut(f64, f64, f64, f64)>::new(
        move |width: f64, height: f64, top: f64, bottom: f64| {
            let (width, height) = (css_px(width), css_px(height));
            if width <= 0.0 || height <= 0.0 {
                return;
            }
            let win = resized.window();
            win.set_inset_top(css_px(top));
            win.set_inset_bottom(css_px(bottom));
            win.window()
                .set_size(mk_views::slint::LogicalSize::new(width, height));
        },
    );
    hook("onResize", on_resize.as_ref());
    on_resize.forget();
    let redraw = view.clone();
    let on_change = Closure::<dyn FnMut()>::new(move || redraw.refresh());
    hook("listen", on_change.as_ref());
    on_change.forget();
}

/// Start the app on the page's `<canvas id="canvas">` (once per page).
#[cfg(target_arch = "wasm32")]
pub fn start() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let flag = JsValue::from_str("__moneykeepCaptureStarted");
    if js_sys::Reflect::get(&window, &flag).is_ok_and(|v| v.is_truthy()) {
        return;
    }
    let _ = js_sys::Reflect::set(&window, &flag, &JsValue::TRUE);
    // Slint draws on the page's `<canvas id="canvas">`, as Slint's WebAssembly
    // documentation (docs.slint.dev) sets the winit backend up in a browser. The
    // window shows the pages the app builds; requests and timers advance on a
    // Slint timer.
    let app = crate::ui::CaptureApp::new(WebPlatform::new());
    let Ok(view) = mk_views::phone::PhoneView::new(app) else {
        return;
    };
    connect(&view);
    let _ = view.run(100);
}
