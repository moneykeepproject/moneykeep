//! The phone screens (mobile-capture.md "WASM client"): Pair, then three tabs
//! (revision 9): Expense (a new expense: the form), Outbox and Sent, with an item
//! opened from a list on its own screen. Revision 11: tapping the selected Expense
//! tab switches its page to Mileage (a trip form) and back. Everything the
//! browser does (requests, IndexedDB, the file input and the canvas) sits behind
//! [`Platform`], so these screens build and run natively too.

use std::collections::HashMap;

use std::rc::Rc;

use mk_capture_api::{
    self as api, ExpenseFields, ExpenseList, Lists, PairResponse, ReceiptCreated, Submission,
    SubmissionStatus, Tags, TripFields, TripList, TripStatus, TripSubmission, msg,
};
use mk_ui::ctl::ui::{Align, Item as UiItem, Kind, Picture, Tone, Ui};

use crate::calendar;
use crate::keypad;
use crate::outbox::{Action, Item, Outbox, Outcome, Photo, State, Storage};

/// A request to the page's own origin (`/api/v1/...`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: &'static str,
    pub path: String,
    pub body: Option<Vec<u8>>,
    pub content_type: Option<&'static str>,
    pub bearer: Option<String>,
}

/// An answer; status 0 means the network failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// A photo picked and prepared by the browser (scaled, re-encoded as JPEG).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedPhoto {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub thumb: Vec<u8>,
    pub thumb_w: u32,
    pub thumb_h: u32,
}

/// What the browser provides.
pub trait Platform {
    fn now_ms(&self) -> i64;
    /// Today's local date, `YYYY-MM-DD`.
    fn today(&self) -> String;
    fn online(&self) -> bool;
    fn default_device_name(&self) -> String;
    fn random16(&self) -> [u8; 16];
    /// Start a request; its answer comes from [`Platform::poll`] with this id.
    fn send(&mut self, req: Request) -> u64;
    fn poll(&mut self) -> Vec<(u64, Response)>;
    /// Open the camera (`capture="environment"`) or the photo picker (`multiple`).
    fn pick_photos(&mut self, camera: bool);
    fn poll_photos(&mut self) -> Vec<Result<PickedPhoto, String>>;
    fn storage(&mut self) -> &mut dyn Storage;
    /// The stored entries, once, after local storage has been read.
    fn loaded(&mut self) -> Option<Vec<(String, String)>>;
    /// The page address's fragment (`#pair=CODE` from the QR code shown by
    /// MoneyKeep), once; the browser removes it from the address. Fragments are
    /// never sent to the server.
    fn take_fragment(&mut self) -> Option<String> {
        None
    }
    /// Save `value` under `key` and report, through [`Platform::poll_saved`] with the
    /// returned id, once the write has completed (or failed). Used for the device
    /// token, so the paired screen is shown only once the pairing is stored.
    fn put_confirmed(&mut self, key: &str, value: &str) -> u64;
    fn poll_saved(&mut self) -> Vec<(u64, Result<(), String>)>;
    /// Ask the browser to keep this site's storage (`navigator.storage.persist()`).
    fn request_persistence(&mut self) {}
    /// Whether the browser promised to keep the storage, once known.
    fn persisted(&self) -> Option<bool> {
        None
    }
    /// Installation: running installed, the browser offers installing, iPhone/iPad.
    fn install(&self) -> Install {
        Install::default()
    }
    /// Show the browser's install prompt.
    fn prompt_install(&mut self) {}
    /// A small marker outside IndexedDB (`localStorage`) saying the app was paired on
    /// this browser before.
    fn paired_before(&self) -> bool {
        false
    }
    fn set_paired_before(&mut self) {}
    /// Go to the app's start page (`/`) through the network, so Cloudflare Access
    /// can redirect to its sign-in (mobile-capture.md revision 6).
    fn go_to_start(&mut self) {}
    /// The page is on screen (`document.visibilityState`, revision 10): coming
    /// back to the app sends the outbox at once.
    fn visible(&self) -> bool {
        true
    }
    /// Minutes to add to UTC for local time at `ms` (`-Date.getTimezoneOffset()`),
    /// for the "Lists from …" note (revision 14).
    fn utc_offset_min(&self, _ms: i64) -> i64 {
        0
    }
    /// Revision 18: the app has drawn its first screen; the page's loading card
    /// can go (`mkCapture.ready()`).
    fn page_ready(&mut self) {}
    /// Revision 18: what the page's loading screen reads before the app runs
    /// (`localStorage`): this phone's way in (`wifi` or `cloudflare`).
    fn remember_way(&mut self, way: &str) {
        let _ = way;
    }
    /// … and the computer's Cloudflare address for **Use Cloudflare** (`None`
    /// forgets it).
    fn remember_cloudflare(&mut self, url: Option<&str>) {
        let _ = url;
    }
}

/// What the browser says about installing the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Install {
    /// Running as the installed app (`display-mode: standalone`).
    pub installed: bool,
    /// The browser offered installing (`beforeinstallprompt`).
    pub can_prompt: bool,
    /// iPhone or iPad (no install prompt; Share → Add to Home Screen).
    pub ios: bool,
}

const TOKEN_KEY: &str = "token";
/// Set when the server said this phone is no longer paired (revision 3).
const DISCONNECTED_KEY: &str = "disconnected";
const HINT_KEY: &str = "install_hint_dismissed";
const LISTS_KEY: &str = "lists";
/// When the lists were last loaded from the computer (Unix ms, revision 14).
const LISTS_AT_KEY: &str = "lists_at";
/// The phone's way in, remembered at pairing (`wifi` or `cloudflare`, revision 10).
const WAY_KEY: &str = "way";
/// The Expense tab's page last shown: `mileage` or `expense` (revision 11).
const PAGE_KEY: &str = "page";
/// The one-time "Tap Expense again to log mileage" tip was closed.
const TIP_KEY: &str = "mileage_tip_seen";
/// The last places used on trips (From and To suggestions), most recent first.
const PLACES_KEY: &str = "places";
const REFRESH_MS: i64 = 60_000;
const MAX_PHOTOS: usize = api::DEFAULT_MAX_RECEIPTS_PER_EXPENSE;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Pair,
    PairInfo,
    Lists,
    Expenses,
    Trips,
    Outbox,
    DeleteSent(String),
    /// upgrade-resilience-1 U1.4: the server's API level.
    Health,
}

/// The screens. Once paired, the app is three tabs (mobile-capture.md revision 9):
/// Expense (a new expense), Outbox and Sent; an item opened from the Outbox or Sent
/// list is shown on `Edit`, which returns to its tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Loading,
    Pair,
    /// The Expense tab: the new-expense form.
    Expense,
    /// The Outbox tab.
    Outbox,
    /// The Sent tab.
    Sent,
    /// An outbox item or a sent expense opened for changes.
    Edit,
}

/// How long the "Sent" / "Saved for later" confirmation shows.
const NOTICE_MS: i64 = 3_000;

#[derive(Debug, Clone, Default)]
struct Form {
    client_id: String,
    /// Editing a submission waiting on the server.
    server_id: Option<String>,
    date: String,
    merchant: String,
    amount: String,
    category: String,
    paid_with: String,
    purpose: String,
    class: String,
    location: String,
    fund: String,
    /// projects.md: an open project of the book.
    project: String,
    photos: Vec<Photo>,
    error: Option<String>,
    /// What the form held when it opened ([`CaptureApp::snapshot`]): leaving with
    /// anything else asks first (revision 7).
    baseline: Option<String>,
    /// A trip instead of an expense (revision 11).
    trip: Option<TripForm>,
}

/// The trip form's fields (mileage.md "Phone"); the date is the form's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TripForm {
    saved_trip_id: String,
    vehicle_id: String,
    purpose_kind: String,
    /// One way when `round_trip` is on.
    miles: String,
    /// "Enter odometer readings".
    odometer: bool,
    odo_start: String,
    odo_end: String,
    round_trip: bool,
    from_place: String,
    to_place: String,
    purpose: String,
    reimbursable: bool,
    /// Tags (mileage.md revision 2), as the Expense form's.
    class: String,
    location: String,
    fund: String,
    project: String,
}

/// What a keypad sheet fills in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Amount,
    Miles,
    OdoStart,
    OdoEnd,
}

/// A bottom sheet over the form (mobile-capture.md revision 7).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Sheet {
    /// The amount keypad: the expression typed, and Done's refusal.
    Keypad {
        expr: String,
        error: Option<&'static str>,
        target: Target,
    },
    /// The calendar on a month.
    Calendar { month: calendar::Month },
    /// A receipt photo full screen.
    Photo(usize),
}

/// The phone app.
pub struct CaptureApp<P: Platform> {
    pub p: P,
    screen: Screen,
    token: Option<String>,
    pub outbox: Outbox,
    lists: Lists,
    /// When `lists` were loaded from the computer (kept in IndexedDB with them).
    lists_at: Option<i64>,
    server: ExpenseList,
    /// This phone's trips on the server (revision 11).
    trips: TripList,
    /// The Expense tab shows Mileage (remembered on the phone).
    mileage: bool,
    /// The Mileage page's unfinished trip while another page shows.
    trip_draft: Option<Form>,
    /// The vehicle and purpose kind of the last trip saved (the next starts with them).
    last_trip: Option<(String, String)>,
    tip_seen: bool,
    places: Vec<String>,
    pair_code: String,
    device_name: String,
    pair_error: Option<String>,
    form: Option<Form>,
    banner: Option<(bool, String)>,
    pending: Vec<(u64, Pending)>,
    action: Option<Action>,
    last_refresh_ms: i64,
    was_online: bool,
    thumbs: HashMap<String, Rc<Picture>>,
    /// The device token being saved (write id, token).
    saving: Option<(u64, String)>,
    /// Cloudflare Access's session ended: show Sign in.
    signin: bool,
    /// Why the Pair screen shows, when known.
    pair_reason: Option<&'static str>,
    hint_dismissed: bool,
    about: bool,
    sheet: Option<Sheet>,
    /// "Discard this expense?" is asked.
    discard: bool,
    /// The Expense tab's unfinished new expense while another screen shows
    /// (revision 9: switching tabs keeps it).
    draft: Option<Form>,
    /// The tab an opened item returns to.
    edit_from: Screen,
    /// "Sent" / "Saved for later" above the action row, until this time (ms).
    notice: Option<(&'static str, i64)>,
    /// "Clear this expense?" is asked.
    clear_ask: bool,
    /// The Capture PIN as typed on the Pair screen (revision 10), shown or masked.
    pin: String,
    pin_shown: bool,
    /// The server asks for the PIN (`pair-info`).
    pin_required: bool,
    /// This phone uses the Wi-Fi address.
    wifi: bool,
    /// A code from the QR code waits for `pair-info` before pairing.
    auto_pair: bool,
    /// The last request did not reach the computer (revision 14: no answer, a
    /// timeout, or a 5xx that is not the capture server's own error).
    unreachable: bool,
    /// Revision 18: … and it was no answer at all (status 0: the network failed or
    /// timed out), as opposed to an error answer.
    no_answer: bool,
    /// The computer's Cloudflare address, from `pair-info` on the Wi-Fi address.
    cloudflare_url: Option<String>,
    /// The page was told the first screen is drawn (revision 18).
    ready_told: bool,
    was_visible: bool,
    /// upgrade-resilience-1 U1.4: the computer's server is older than this phone
    /// app needs ([`api::phone_server_notice`]), from its last health answer.
    server_notice: Option<&'static str>,
    /// The API level this phone app needs ([`api::PHONE_NEEDS_API_LEVEL`]; tests
    /// raise it).
    needs_api_level: u32,
}

impl<P: Platform> std::fmt::Debug for CaptureApp<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureApp")
            .field("screen", &self.screen)
            .field("outbox", &self.outbox.items.len())
            .finish_non_exhaustive()
    }
}

fn opt(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

fn server_message(r: &Response) -> String {
    serde_json::from_slice::<api::ApiError>(&r.body)
        .map(|e| e.error.message)
        .unwrap_or_else(|_| format!("The server answered {}.", r.status))
}

fn error_code(r: &Response) -> String {
    serde_json::from_slice::<api::ApiError>(&r.body)
        .map(|e| e.error.code)
        .unwrap_or_default()
}

/// Whether an answer is Cloudflare Access's sign-in instead of the API: a redirect
/// (the browser layer reports an opaque redirect as 302) or an HTML page.
pub fn is_access_login(r: &Response) -> bool {
    matches!(r.status, 301 | 302 | 303 | 307 | 308)
        || (r.status == 200
            && r.body
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| *b == b'<'))
}

/// The capture server refused the Cloudflare sign-in (mobile-capture.md revision
/// 8): a 401 `access_refused` or `access_missing`, or the older `not_signed_in`.
/// Signing in again cannot help (Cloudflare already has a session); the server's
/// settings need fixing, so this returns the message to show instead of **Sign in**.
pub fn access_refused(r: &Response) -> Option<String> {
    if r.status != 401 {
        return None;
    }
    match error_code(r).as_str() {
        "access_refused" | "access_missing" => {
            let m = server_message(r);
            Some(if m.trim().is_empty() {
                msg::ACCESS_REFUSED.to_owned()
            } else {
                m
            })
        }
        "not_signed_in" => Some(msg::ACCESS_REFUSED.to_owned()),
        _ => None,
    }
}

/// The computer can't be reached (mobile-capture.md revision 14): no answer or a
/// timeout (status 0), or status 500 or higher that is not the capture server's
/// own JSON error — Cloudflare's error page while the computer is off (530 for a
/// tunnel that is down; 502–504, 520–527). The capture server's own 5xx errors
/// (its 507 "Storage full") mean the computer answered, so they keep their message.
pub fn not_reachable(r: &Response) -> bool {
    r.status == 0 || (r.status >= 500 && serde_json::from_slice::<api::ApiError>(&r.body).is_err())
}

/// How an answer to an outbox request is treated. Only `not_paired` ends the
/// pairing; everything else keeps the token.
pub fn classify(r: &Response) -> Result<Vec<u8>, Outcome> {
    if is_access_login(r) {
        return Err(Outcome::Retry {
            message: msg::SIGN_IN_AGAIN.into(),
        });
    }
    if let Some(message) = access_refused(r) {
        return Err(Outcome::Retry { message });
    }
    if not_reachable(r) {
        // Revision 14: kept in the Outbox, the pairing kept.
        return Err(Outcome::Retry {
            message: msg::NOT_REACHABLE.into(),
        });
    }
    match r.status {
        200 | 201 => Ok(r.body.clone()),
        401 if error_code(r) == "not_paired" => Err(Outcome::Unpaired),
        401 => Err(Outcome::Retry {
            message: msg::ACCESS_REFUSED.into(),
        }),
        400 | 404 | 409 | 413 | 415 | 422 => Err(Outcome::Refused {
            message: server_message(r),
        }),
        _ => Err(Outcome::Retry {
            message: server_message(r),
        }),
    }
}

/// [`classify`] for an outbox action: a trip that a capture server from before
/// mileage does not know (its 404 for an unknown address, or 405) stays in the
/// Outbox and is tried again later (mobile-capture.md revision 11).
///
/// upgrade-resilience-1 M9: an expense refused because one of its photos is no
/// longer on the server (a 422 on `receipt_ids`) is [`Outcome::PhotosGone`]: the
/// photos are uploaded afresh.
pub fn classify_for(a: &Action, r: &Response) -> Result<Vec<u8>, Outcome> {
    if matches!(a, Action::Submit { .. } | Action::Patch { .. })
        && r.status == 422
        && let Ok(e) = serde_json::from_slice::<api::ApiError>(&r.body)
        && e.error.fields.iter().any(|f| f.field == "receipt_ids")
    {
        return Err(Outcome::PhotosGone {
            message: e.error.message,
        });
    }
    if a.is_trip() && (r.status == 405 || (r.status == 404 && error_code(r) == "not_found")) {
        return Err(Outcome::Retry {
            message: msg::NO_MILEAGE.into(),
        });
    }
    classify(r)
}

impl<P: Platform> CaptureApp<P> {
    pub fn new(p: P) -> Self {
        Self {
            p,
            screen: Screen::Loading,
            token: None,
            outbox: Outbox::default(),
            lists: Lists::default(),
            lists_at: None,
            server: ExpenseList::default(),
            trips: TripList::default(),
            mileage: false,
            trip_draft: None,
            last_trip: None,
            tip_seen: false,
            places: Vec::new(),
            pair_code: String::new(),
            device_name: String::new(),
            pair_error: None,
            form: None,
            banner: None,
            pending: Vec::new(),
            action: None,
            last_refresh_ms: 0,
            was_online: true,
            thumbs: HashMap::new(),
            saving: None,
            signin: false,
            pair_reason: None,
            hint_dismissed: false,
            about: false,
            sheet: None,
            discard: false,
            draft: None,
            edit_from: Screen::Outbox,
            notice: None,
            clear_ask: false,
            pin: String::new(),
            pin_shown: false,
            pin_required: false,
            wifi: false,
            auto_pair: false,
            unreachable: false,
            no_answer: false,
            cloudflare_url: None,
            ready_told: false,
            was_visible: true,
            server_notice: None,
            needs_api_level: api::PHONE_NEEDS_API_LEVEL,
        }
    }

    fn api(
        &mut self,
        method: &'static str,
        path: &str,
        body: Option<Vec<u8>>,
        ct: Option<&'static str>,
        what: Pending,
    ) {
        let id = self.p.send(Request {
            method,
            path: format!("{}{path}", api::API_PREFIX),
            body,
            content_type: ct,
            bearer: self.token.clone(),
        });
        self.pending.push((id, what));
    }

    fn refresh(&mut self) {
        if self.token.is_none() {
            return;
        }
        if !self.pending.iter().any(|(_, w)| *w == Pending::Lists) {
            // Revision 11: ask for the mileage lists too (an older server ignores it).
            self.api("GET", "/lists?mileage=1", None, None, Pending::Lists);
        }
        if !self.pending.iter().any(|(_, w)| *w == Pending::Expenses) {
            self.api("GET", "/expenses", None, None, Pending::Expenses);
        }
        if !self.pending.iter().any(|(_, w)| *w == Pending::Trips) {
            self.api("GET", "/trips", None, None, Pending::Trips);
        }
        if !self.pending.iter().any(|(_, w)| *w == Pending::Health) {
            self.api("GET", "/health", None, None, Pending::Health);
        }
    }

    /// Everything that happens without a frame being drawn: loading, answers,
    /// sending the outbox and the timers.
    pub fn tick(&mut self) {
        let now = self.p.now_ms();
        if self.screen == Screen::Loading {
            if let Some(entries) = self.p.loaded() {
                self.outbox = Outbox::load(&entries);
                let mut disconnected = false;
                for (k, v) in &entries {
                    if k == TOKEN_KEY {
                        self.token = Some(v.clone());
                    } else if k == LISTS_KEY {
                        self.lists = serde_json::from_str(v).unwrap_or_default();
                    } else if k == LISTS_AT_KEY {
                        self.lists_at = v.parse().ok();
                    } else if k == DISCONNECTED_KEY {
                        disconnected = true;
                    } else if k == HINT_KEY {
                        self.hint_dismissed = true;
                    } else if k == WAY_KEY {
                        self.wifi = v == api::VIA_WIFI;
                        self.p.remember_way(v);
                    } else if k == PAGE_KEY {
                        self.mileage = v == "mileage";
                    } else if k == TIP_KEY {
                        self.tip_seen = true;
                    } else if k == PLACES_KEY {
                        self.places = serde_json::from_str(v).unwrap_or_default();
                    }
                }
                if self.token.is_none() {
                    self.pair_reason = if disconnected {
                        Some(msg::WAS_DISCONNECTED)
                    } else if self.p.paired_before() {
                        Some(msg::STORAGE_CLEARED)
                    } else {
                        None
                    };
                } else {
                    self.p.request_persistence();
                }
                self.device_name = self.p.default_device_name();
                // A pairing code from the QR code (mobile-capture.md revision 2 B7):
                // fill it in and pair at once — but only when not paired. A paired
                // phone that opens the QR address again (from the browser's history
                // or a shortcut) keeps its pairing (revision 3).
                let scanned = self
                    .p
                    .take_fragment()
                    .and_then(|f| api::code_from_fragment(&f));
                if let Some(code) = scanned {
                    if self.token.is_some() {
                        self.banner = Some((false, msg::ALREADY_PAIRED.into()));
                    } else {
                        // Revision 10: ask first whether a Capture PIN is needed;
                        // without one the code pairs at once as before.
                        self.pair_code = code;
                        self.auto_pair = true;
                    }
                }
                if self.token.is_none() {
                    self.api("GET", "/pair-info", None, None, Pending::PairInfo);
                }
                if self.token.is_some() {
                    self.go_tab(Screen::Expense);
                } else {
                    self.screen = Screen::Pair;
                }
                self.refresh();
                self.last_refresh_ms = now;
            } else {
                return;
            }
        }
        for (id, resp) in self.p.poll() {
            let Some(pos) = self.pending.iter().position(|(p, _)| *p == id) else {
                continue;
            };
            let (_, what) = self.pending.remove(pos);
            self.answer(what, resp);
        }
        for (id, result) in self.p.poll_saved() {
            if self.saving.as_ref().is_some_and(|(w, _)| *w == id)
                && let Some((_, token)) = self.saving.take()
            {
                self.saved(token, result);
            }
        }
        let online = self.p.online();
        if online && !self.was_online {
            // The `online` event: send right away.
            self.outbox.retry_now();
            self.refresh();
        }
        self.was_online = online;
        // Back on screen (revision 10): the phone may be home on the Wi-Fi again.
        let visible = self.p.visible();
        if visible && !self.was_visible {
            self.outbox.retry_now();
            self.refresh();
        }
        self.was_visible = visible;
        if now - self.last_refresh_ms >= REFRESH_MS {
            self.last_refresh_ms = now;
            self.outbox.retry_now();
            self.refresh();
        }
        if self.token.is_some()
            && self.action.is_none()
            && let Some(a) = self.outbox.next_action(now, online)
        {
            self.send_action(&a);
            self.action = Some(a);
        }
        for r in self.p.poll_photos() {
            match r {
                Ok(ph) => self.add_photo(ph),
                Err(m) => self.banner = Some((true, m)),
            }
        }
    }

    fn send_action(&mut self, a: &Action) {
        match a {
            Action::Upload { jpeg, .. } => self.api(
                "POST",
                "/receipts",
                Some(jpeg.clone()),
                Some("image/jpeg"),
                Pending::Outbox,
            ),
            Action::Submit { body, .. } => {
                let b = serde_json::to_vec(body).unwrap_or_default();
                self.api(
                    "POST",
                    "/expenses",
                    Some(b),
                    Some("application/json"),
                    Pending::Outbox,
                )
            }
            Action::Patch {
                server_id, body, ..
            } => {
                let b = serde_json::to_vec(body).unwrap_or_default();
                self.api(
                    "PATCH",
                    &format!("/expenses/{server_id}"),
                    Some(b),
                    Some("application/json"),
                    Pending::Outbox,
                )
            }
            Action::SubmitTrip { body, .. } => {
                let b = serde_json::to_vec(body).unwrap_or_default();
                self.api(
                    "POST",
                    "/trips",
                    Some(b),
                    Some("application/json"),
                    Pending::Outbox,
                )
            }
            Action::PatchTrip {
                server_id, body, ..
            } => {
                let b = serde_json::to_vec(body).unwrap_or_default();
                self.api(
                    "PATCH",
                    &format!("/trips/{server_id}"),
                    Some(b),
                    Some("application/json"),
                    Pending::Outbox,
                )
            }
        }
    }

    fn answer(&mut self, what: Pending, r: Response) {
        let now = self.p.now_ms();
        if what != Pending::PairInfo {
            self.unreachable = not_reachable(&r);
            self.no_answer = r.status == 0;
        }
        match what {
            Pending::PairInfo => {
                let info = (r.status == 200 && !is_access_login(&r))
                    .then(|| serde_json::from_slice::<api::PairInfo>(&r.body).ok())
                    .flatten();
                if let Some(i) = &info {
                    self.pin_required = i.pin_required;
                    self.wifi = i.way == api::VIA_WIFI;
                    self.cloudflare_url = i
                        .cloudflare_url
                        .clone()
                        .filter(|u| u.starts_with("https://"));
                }
                if self.auto_pair {
                    self.auto_pair = false;
                    // Without an answer (an older server, or Access's sign-in) pair
                    // as before; with a PIN to enter, wait for it.
                    if !self.pin_required {
                        self.send_pair();
                    }
                }
            }
            Pending::Pair => {
                if r.status == 201 {
                    match serde_json::from_slice::<PairResponse>(&r.body) {
                        Ok(p) => {
                            // The paired screen shows once the token is stored.
                            let id = self.p.put_confirmed(TOKEN_KEY, &p.device_token);
                            self.saving = Some((id, p.device_token));
                            self.pair_code.clear();
                            self.pair_error = None;
                        }
                        Err(_) => self.pair_error = Some("The answer was not understood.".into()),
                    }
                } else if is_access_login(&r) {
                    self.signin = true;
                    self.pair_error = Some(msg::SIGN_IN_AGAIN.into());
                } else if let Some(m) = access_refused(&r) {
                    // The server's settings, not the sign-in (revision 8).
                    self.pair_error = Some(m);
                } else if not_reachable(&r) {
                    self.pair_error = Some(if self.wifi {
                        "Can't reach your computer. Is this phone on the same Wi-Fi?".into()
                    } else {
                        "No connection. Try again when online.".into()
                    });
                } else if error_code(&r) == "pin_wrong" {
                    self.pin_required = true;
                    self.pin.clear();
                    self.pair_error = Some(msg::PIN_WRONG.into());
                } else if error_code(&r) == "pin_not_set" {
                    self.pin_required = true;
                    self.pair_error = Some(server_message(&r));
                } else {
                    self.pair_error = Some(server_message(&r));
                }
            }
            Pending::Lists => {
                if r.status == 200 && !is_access_login(&r) {
                    if let Ok(l) = serde_json::from_slice::<Lists>(&r.body) {
                        if let Ok(json) = serde_json::to_string(&l) {
                            self.p.storage().put(LISTS_KEY, &json);
                            self.p.storage().put(LISTS_AT_KEY, &now.to_string());
                            self.lists_at = Some(now);
                        }
                        self.lists = l;
                    }
                } else {
                    self.note_failure(&r);
                }
            }
            Pending::Expenses => {
                if r.status == 200 && !is_access_login(&r) {
                    if let Ok(l) = serde_json::from_slice::<ExpenseList>(&r.body) {
                        self.server = l;
                    }
                } else {
                    self.note_failure(&r);
                }
            }
            Pending::Trips => {
                // An older server has no trips (404): nothing to show.
                if r.status == 200 && !is_access_login(&r) {
                    if let Ok(l) = serde_json::from_slice::<TripList>(&r.body) {
                        self.trips = l;
                    }
                } else if r.status != 404 {
                    self.note_failure(&r);
                }
            }
            Pending::Health => {
                // U1.4: an answer that isn't the health document changes nothing.
                if r.status == 200
                    && !is_access_login(&r)
                    && let Ok(h) = serde_json::from_slice::<api::Health>(&r.body)
                {
                    self.server_notice = api::phone_server_notice_at(&h, self.needs_api_level);
                }
            }
            Pending::DeleteSent(id) => {
                if r.status == 200 {
                    let store = self.p.storage();
                    self.outbox.forget_sent(&id, store);
                    self.refresh();
                } else {
                    self.banner = Some((true, server_message(&r)));
                }
            }
            Pending::Outbox => {
                let Some(a) = self.action.take() else { return };
                let outcome = match classify_for(&a, &r) {
                    Ok(body) => match &a {
                        Action::Upload { .. } => serde_json::from_slice::<ReceiptCreated>(&body)
                            .map(|c| Outcome::Uploaded {
                                receipt_id: c.receipt_id,
                            })
                            .unwrap_or(Outcome::Retry {
                                message: "The answer was not understood.".into(),
                            }),
                        Action::Submit { .. } | Action::Patch { .. } => {
                            serde_json::from_slice::<Submission>(&body)
                                .map(|s| Outcome::Submitted { server_id: s.id })
                                .unwrap_or(Outcome::Retry {
                                    message: "The answer was not understood.".into(),
                                })
                        }
                        Action::SubmitTrip { .. } | Action::PatchTrip { .. } => {
                            serde_json::from_slice::<TripSubmission>(&body)
                                .map(|s| Outcome::Submitted { server_id: s.id })
                                .unwrap_or(Outcome::Retry {
                                    message: "The answer was not understood.".into(),
                                })
                        }
                    },
                    Err(o) => o,
                };
                let sent = matches!(outcome, Outcome::Submitted { .. });
                let unpaired = outcome == Outcome::Unpaired;
                if matches!(&outcome, Outcome::Retry { message } if message == msg::SIGN_IN_AGAIN) {
                    self.signin = true;
                }
                let store = self.p.storage();
                self.outbox.complete(&a, outcome, now, store);
                if unpaired {
                    self.forget_pairing();
                }
                if sent {
                    self.refresh();
                }
            }
        }
    }

    /// The token write finished.
    fn saved(&mut self, token: String, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.token = Some(token);
                self.pin.clear();
                let way = if self.wifi {
                    api::VIA_WIFI
                } else {
                    api::VIA_CLOUDFLARE
                };
                self.p.storage().put(WAY_KEY, way);
                self.p.remember_way(way);
                let cf = self.cloudflare_url.clone();
                self.p
                    .remember_cloudflare(cf.as_deref().filter(|_| self.wifi));
                self.p.storage().delete(DISCONNECTED_KEY);
                self.p.set_paired_before();
                self.p.request_persistence();
                self.pair_reason = None;
                self.outbox.paired();
                self.banner = None;
                self.signin = false;
                self.go_tab(Screen::Expense);
                self.refresh();
            }
            Err(_) => self.pair_error = Some(msg::PAIRING_NOT_SAVED.into()),
        }
    }

    /// The server said this phone is no longer paired: the only answer that ends the
    /// pairing (besides Sign out). The outbox stays on the phone.
    fn forget_pairing(&mut self) {
        self.p.storage().delete(TOKEN_KEY);
        self.p.storage().put(DISCONNECTED_KEY, "1");
        self.token = None;
        self.outbox.unpaired = true;
        self.pair_reason = Some(msg::WAS_DISCONNECTED);
        self.banner = Some((true, msg::DISCONNECTED.into()));
        // An expense being entered stays on screen (it can still be saved for
        // later); otherwise the Pair screen shows.
        let entering = self.screen == Screen::Edit
            || (self.screen == Screen::Expense
                && self.form.as_ref().is_some_and(Self::form_changed));
        if !entering {
            self.screen = Screen::Pair;
        }
    }

    fn note_failure(&mut self, r: &Response) {
        if r.status == 401 && error_code(r) == "not_paired" {
            self.forget_pairing();
        } else if is_access_login(r) {
            self.signin = true;
        } else if let Some(m) = access_refused(r) {
            self.banner = Some((true, m));
        }
    }

    fn add_photo(&mut self, ph: PickedPhoto) {
        // The form shown, or the Expense tab's kept one (a tab was switched while
        // the picker was open).
        let form = match (&mut self.form, &mut self.draft) {
            (Some(f), _) if f.trip.is_none() => f,
            (_, Some(f)) => f,
            _ => return,
        };
        if form.photos.len() >= MAX_PHOTOS {
            form.error = Some(format!("An expense can have at most {MAX_PHOTOS} photos."));
            return;
        }
        form.photos.push(Photo {
            jpeg: ph.jpeg,
            width: ph.width,
            height: ph.height,
            thumb: ph.thumb,
            thumb_w: ph.thumb_w,
            thumb_h: ph.thumb_h,
            receipt_id: None,
        });
    }

    /// Show `f` (an outbox item or a sent expense) for changes; leaving it with
    /// changes asks first, and it returns to the tab it was opened from.
    fn open_form(&mut self, mut f: Form) {
        f.baseline = Some(Self::snapshot(&f));
        if self.screen == Screen::Expense
            && let Some(cur) = self.form.take()
        {
            self.park(cur);
        }
        if matches!(self.screen, Screen::Outbox | Screen::Sent) {
            self.edit_from = self.screen;
        }
        self.form = Some(f);
        self.sheet = None;
        self.discard = false;
        self.screen = Screen::Edit;
    }

    /// Whether the form differs from what it held when it opened.
    fn form_changed(f: &Form) -> bool {
        f.baseline.as_ref() != Some(&Self::snapshot(f))
    }

    /// What a form holds, for telling whether it changed.
    fn snapshot(f: &Form) -> String {
        match &f.trip {
            Some(t) => format!("trip|{}|{t:?}", f.date),
            None => format!("{:?}|{}", Self::fields_of(f), f.photos.len()),
        }
    }

    /// Keep the Expense tab's unfinished form (an expense or a trip) while another
    /// page or tab shows.
    fn park(&mut self, f: Form) {
        if f.trip.is_some() {
            self.trip_draft = Some(f);
        } else {
            self.draft = Some(f);
        }
    }

    /// The kept form of a page, or a new one when it was left untouched.
    fn unpark(&mut self, trip: bool) -> Form {
        let kept = if trip {
            self.trip_draft.take()
        } else {
            self.draft.take()
        };
        match kept.filter(Self::form_changed) {
            Some(f) => f,
            None if trip => self.fresh_trip(),
            None => self.fresh_form(),
        }
    }

    /// The Expense tab shows Mileage (the book has the mileage module on).
    fn showing_mileage(&self) -> bool {
        self.mileage && self.lists.mileage_enabled
    }

    /// Tapping the selected Expense tab: Expense ⇄ Mileage (revision 11). The
    /// unfinished form of each page is kept; the choice is remembered.
    fn toggle_page(&mut self) {
        if let Some(f) = self.form.take() {
            self.park(f);
        }
        self.mileage = !self.showing_mileage();
        let word = if self.mileage { "mileage" } else { "expense" };
        self.p.storage().put(PAGE_KEY, word);
        self.close_tip();
        self.sheet = None;
        self.clear_ask = false;
        let f = self.unpark(self.mileage);
        self.form = Some(f);
    }

    fn close_tip(&mut self) {
        if !self.tip_seen {
            self.tip_seen = true;
            self.p.storage().put(TIP_KEY, "1");
        }
    }

    /// The top bar's back arrow (only on an opened item): asks first when it has
    /// changes.
    fn back(&mut self) {
        if self.screen == Screen::Edit {
            if self.form.as_ref().is_some_and(Self::form_changed) {
                self.discard = true;
            } else {
                self.close_edit();
            }
        }
    }

    /// Leave an opened item for the tab it came from.
    fn close_edit(&mut self) {
        self.form = None;
        self.sheet = None;
        self.discard = false;
        self.screen = self.edit_from;
    }

    /// A new expense with today's date (its baseline set, so changes show).
    fn fresh_form(&mut self) -> Form {
        let mut f = self.new_form();
        f.baseline = Some(Self::snapshot(&f));
        f
    }

    /// A new trip with today's date, the last trip's vehicle and purpose kind (or
    /// the only vehicle, and the book's default purpose).
    fn fresh_trip(&mut self) -> Form {
        let mut f = self.new_form();
        let (vehicle, kind) = self.last_trip.clone().unwrap_or_default();
        let vehicles = &self.lists.vehicles;
        let vehicle_id = if vehicles.iter().any(|v| v.id == vehicle) {
            vehicle
        } else if let [only] = vehicles.as_slice() {
            only.id.clone()
        } else {
            String::new()
        };
        let purpose_kind = if api::is_trip_kind(&kind) {
            kind
        } else if api::is_trip_kind(&self.lists.default_purpose) {
            self.lists.default_purpose.clone()
        } else {
            "business".to_owned()
        };
        f.trip = Some(TripForm {
            vehicle_id,
            purpose_kind,
            ..TripForm::default()
        });
        f.baseline = Some(Self::snapshot(&f));
        f
    }

    /// Show a tab. Leaving the Expense tab keeps its unfinished expense as it is;
    /// coming back shows it again (a new one when it was left untouched, so its
    /// date is today's).
    fn go_tab(&mut self, tab: Screen) {
        if self.screen == tab {
            return;
        }
        match self.screen {
            Screen::Expense => {
                if let Some(f) = self.form.take() {
                    self.park(f);
                }
            }
            Screen::Edit => self.form = None,
            _ => {}
        }
        self.sheet = None;
        self.discard = false;
        self.clear_ask = false;
        self.screen = tab;
        if tab == Screen::Expense {
            let f = self.unpark(self.showing_mileage());
            self.form = Some(f);
        }
    }

    fn new_form(&mut self) -> Form {
        Form {
            client_id: crate::outbox::uuid_from(self.p.random16()),
            date: self.p.today(),
            ..Form::default()
        }
    }

    fn form_from_item(i: &Item) -> Form {
        if let Some(t) = &i.trip {
            return Form {
                client_id: i.client_id.clone(),
                server_id: i.server_id.clone(),
                date: t.date.clone(),
                trip: Some(trip_form_from(t)),
                ..Form::default()
            };
        }
        let f = &i.fields;
        let s = |v: &Option<String>| v.clone().unwrap_or_default();
        Form {
            client_id: i.client_id.clone(),
            server_id: i.server_id.clone(),
            date: s(&f.date),
            merchant: s(&f.merchant),
            amount: s(&f.amount),
            category: s(&f.category),
            paid_with: s(&f.paid_with),
            purpose: s(&f.purpose),
            class: s(&f.tags.class),
            location: s(&f.tags.location),
            fund: s(&f.tags.fund),
            project: s(&f.tags.project),
            photos: i.photos.clone(),
            error: None,
            baseline: None,
            trip: None,
        }
    }

    /// A trip on the server opened for changes (`server`: it is still waiting), or
    /// a refused one to send again as a new trip.
    fn form_from_trip(&mut self, sub: &TripSubmission, fields: &TripFields, server: bool) -> Form {
        Form {
            client_id: if server {
                sub.client_id.clone()
            } else {
                crate::outbox::uuid_from(self.p.random16())
            },
            server_id: server.then(|| sub.id.clone()),
            date: fields.date.clone(),
            trip: Some(trip_form_from(fields)),
            ..Form::default()
        }
    }

    fn form_from_submission(&mut self, sub: &Submission) -> Form {
        let f = &sub.fields;
        let s = |v: &Option<String>| v.clone().unwrap_or_default();
        Form {
            client_id: crate::outbox::uuid_from(self.p.random16()),
            server_id: Some(sub.id.clone()),
            date: s(&f.date),
            merchant: s(&f.merchant),
            amount: s(&f.amount),
            category: s(&f.category),
            paid_with: s(&f.paid_with),
            purpose: s(&f.purpose),
            class: s(&f.tags.class),
            location: s(&f.tags.location),
            fund: s(&f.tags.fund),
            project: s(&f.tags.project),
            // Photos already on the server are kept by id.
            photos: sub
                .receipt_ids
                .iter()
                .map(|r| Photo {
                    jpeg: Vec::new(),
                    width: 0,
                    height: 0,
                    thumb: Vec::new(),
                    thumb_w: 0,
                    thumb_h: 0,
                    receipt_id: Some(r.clone()),
                })
                .collect(),
            error: None,
            baseline: None,
            trip: None,
        }
    }

    fn fields_of(f: &Form) -> ExpenseFields {
        ExpenseFields {
            date: opt(&f.date),
            merchant: opt(&f.merchant),
            amount: opt(&f.amount).map(|a| a.replace(',', ".")),
            category: opt(&f.category),
            paid_with: opt(&f.paid_with),
            purpose: opt(&f.purpose),
            tags: Tags {
                class: opt(&f.class),
                location: opt(&f.location),
                fund: opt(&f.fund),
                project: opt(&f.project),
            },
        }
    }

    /// Remember a trip's places for the From and To suggestions (the last 20).
    fn remember_places(&mut self, t: &TripForm) {
        for p in [&t.to_place, &t.from_place] {
            let p = p.trim();
            if p.is_empty() {
                continue;
            }
            self.places.retain(|x| !x.eq_ignore_ascii_case(p));
            self.places.insert(0, p.to_owned());
        }
        self.places.truncate(api::PLACES_KEPT);
        if let Ok(json) = serde_json::to_string(&self.places) {
            self.p.storage().put(PLACES_KEY, &json);
        }
    }

    /// Save the trip form to the outbox (revision 11).
    fn save_trip(&mut self, mut f: Form, t: TripForm, send: bool) {
        let trip = trip_fields_of(&f, &t, self.lists.reimbursable_enabled);
        if send {
            let today = api::parse_date(&self.p.today()).unwrap_or(0);
            let mut problems: Vec<String> = Vec::new();
            if !self.lists.vehicles.is_empty() && trip.vehicle_id.is_empty() {
                problems.push("Choose a vehicle.".into());
            }
            if let Err(errors) = api::validate_trip(&trip, today) {
                problems.extend(errors.into_iter().map(|e| e.message));
            }
            if !problems.is_empty() {
                f.error = Some(problems.join(" "));
                self.form = Some(f);
                return;
            }
        }
        self.remember_places(&t);
        self.last_trip = Some((t.vehicle_id.clone(), t.purpose_kind.clone()));
        let item = Item {
            client_id: f.client_id.clone(),
            fields: ExpenseFields::default(),
            trip: Some(trip),
            photos: Vec::new(),
            state: if send { State::Queued } else { State::Draft },
            server_id: f.server_id,
            attempts: 0,
            next_try_ms: 0,
            last_error: None,
            created_ms: self.p.now_ms(),
            reuploads: 0,
        };
        let store = self.p.storage();
        self.outbox.put(item, store);
        self.after_save(send, true);
    }

    /// After saving: an opened item returns to its tab; the Expense tab starts a
    /// new form of the same kind and confirms (revision 9).
    fn after_save(&mut self, send: bool, trip: bool) {
        self.sheet = None;
        self.discard = false;
        if self.screen == Screen::Edit {
            self.close_edit();
        } else if self.token.is_none() {
            self.screen = Screen::Pair;
        } else {
            let f = if trip {
                self.fresh_trip()
            } else {
                self.fresh_form()
            };
            self.form = Some(f);
            let word = if send { "Sent" } else { "Saved for later" };
            self.notice = Some((word, self.p.now_ms() + NOTICE_MS));
        }
    }

    /// Save the form to the outbox (`send`: queued; else a draft).
    fn save_form(&mut self, send: bool) {
        let Some(mut f) = self.form.take() else {
            return;
        };
        if let Some(t) = f.trip.clone() {
            self.save_trip(f, t, send);
            return;
        }
        let fields = Self::fields_of(&f);
        if send {
            let today = api::parse_date(&self.p.today()).unwrap_or(0);
            if let Err(errors) = api::validate_expense(
                &fields,
                &f.photos
                    .iter()
                    .map(|p| p.receipt_id.clone().unwrap_or_else(|| "0".repeat(32)))
                    .collect::<Vec<_>>(),
                today,
                MAX_PHOTOS,
            ) {
                f.error = Some(
                    errors
                        .into_iter()
                        .map(|e| e.message)
                        .collect::<Vec<_>>()
                        .join(" "),
                );
                self.form = Some(f);
                return;
            }
        }
        let item = Item {
            client_id: f.client_id.clone(),
            fields,
            trip: None,
            photos: f.photos,
            state: if send { State::Queued } else { State::Draft },
            server_id: f.server_id,
            attempts: 0,
            next_try_ms: 0,
            last_error: None,
            created_ms: self.p.now_ms(),
            reuploads: 0,
        };
        let store = self.p.storage();
        self.outbox.put(item, store);
        self.after_save(send, false);
    }

    /// A photo's thumbnail as a picture (cached by `key`).
    fn thumb(&mut self, key: &str, p: &Photo) -> Option<Rc<Picture>> {
        if p.thumb.is_empty() || p.thumb_w == 0 || p.thumb_h == 0 {
            return None;
        }
        if let Some(t) = self.thumbs.get(key) {
            return Some(Rc::clone(t));
        }
        let n = usize::try_from(p.thumb_w)
            .ok()?
            .saturating_mul(usize::try_from(p.thumb_h).ok()?);
        if n.saturating_mul(4) != p.thumb.len() {
            return None;
        }
        let pic = Rc::new(Picture {
            width: p.thumb_w,
            height: p.thumb_h,
            rgba: p.thumb.clone(),
        });
        self.thumbs.insert(key.to_owned(), Rc::clone(&pic));
        Some(pic)
    }

    /// The API level this phone app needs (upgrade-resilience-1 U1.4). Raised
    /// only by tests and the review screenshots, to show the notice.
    #[doc(hidden)]
    pub fn set_needs_api_level(&mut self, level: u32) {
        self.needs_api_level = level;
    }

    /// Whether requests or sending are under way (the window ticks faster).
    pub fn busy(&self) -> bool {
        !self.pending.is_empty() || self.action.is_some()
    }

    /// Build the current screen with mk-ui's page builder (drawn by mk-views'
    /// `PhoneWindow`).
    pub fn view(&mut self, ui: &mut Ui) {
        // Revision 18: nothing while the phone's storage is read (the page's
        // loading card stays over the app until its first real screen is drawn).
        if self.screen == Screen::Loading {
            return;
        }
        if !self.ready_told {
            self.ready_told = true;
            self.p.page_ready();
        }
        self.top_bar(ui);
        if let Some((err, m)) = self.banner.clone() {
            ui.banner(if err { Tone::Error } else { Tone::Normal }, |ui| {
                ui.row(|ui| {
                    ui.next().grow();
                    ui.label(m);
                    ui.tip("Dismiss");
                    if ui.icon_button("close", "Dismiss", false).clicked() {
                        self.banner = None;
                    }
                });
            });
        }
        if self.signin {
            ui.banner(Tone::Warning, |ui| {
                ui.row(|ui| {
                    ui.next().grow();
                    ui.bold();
                    ui.label(msg::SIGN_IN_AGAIN);
                    if ui.button("Sign in").clicked() {
                        self.p.go_to_start();
                    }
                });
            });
        }
        self.not_reachable_banner(ui);
        self.server_notice_banner(ui);
        match self.screen {
            Screen::Loading => {}
            Screen::Pair => self.pair_screen(ui),
            Screen::Expense => {
                self.install_hint(ui, false);
                self.form_screen(ui);
            }
            Screen::Edit => self.form_screen(ui),
            Screen::Outbox => self.outbox_screen(ui),
            Screen::Sent => self.sent_screen(ui),
        }
        self.tab_bar(ui);
        self.about_dialog(ui);
        self.discard_dialog(ui);
        self.clear_dialog(ui);
    }

    /// The bottom tab bar, once paired, on the three tabs (revision 9): Expense,
    /// Outbox (with the count of items waiting) and Sent.
    fn tab_bar(&mut self, ui: &mut Ui) {
        if self.token.is_none()
            || !matches!(self.screen, Screen::Expense | Screen::Outbox | Screen::Sent)
        {
            return;
        }
        let waiting = self.outbox.items.len();
        // Revision 11: the Expense tab names the page it shows, and tapping it
        // while selected switches the page.
        let (page, page_icon, other) = if self.showing_mileage() {
            ("Mileage", "sym-car", "Expense")
        } else {
            ("Expense", "receipt-pencil", "Mileage")
        };
        let tabs = [
            (Screen::Expense, "Expense", page, page_icon, 0),
            (Screen::Outbox, "Outbox", "Outbox", "outbox", waiting),
            (Screen::Sent, "Sent", "Sent", "sent", 0),
        ];
        let toggles = self.lists.mileage_enabled;
        let mut chosen = None;
        ui.tab_bar(|ui| {
            for (screen, key, text, icon, count) in tabs {
                let mut name = text.to_owned();
                if count > 0 {
                    name.push_str(&format!(", {count} waiting"));
                }
                if screen == self.screen {
                    name.push_str(", selected");
                    if screen == Screen::Expense && toggles {
                        name.push_str(&format!(", tap again for {other}"));
                    }
                }
                ui.next().id(format!("tab:{key}"));
                let item = UiItem {
                    kind: Kind::BottomTab,
                    text: text.to_owned(),
                    label: name,
                    icon: icon.to_owned(),
                    checked: screen == self.screen,
                    number: i32::try_from(count).unwrap_or(i32::MAX),
                    ..UiItem::default()
                };
                if ui.item(item, true).clicked() {
                    chosen = Some(screen);
                }
            }
        });
        match chosen {
            Some(Screen::Expense) if self.screen == Screen::Expense && toggles => {
                self.toggle_page();
            }
            Some(tab) => self.go_tab(tab),
            None => {}
        }
    }

    /// "Clear this expense?" from the menu on the Expense tab.
    fn clear_dialog(&mut self, ui: &mut Ui) {
        if !self.clear_ask {
            return;
        }
        let mut keep = ui.dialog_closed("clear");
        let mut clear = false;
        let what = self.form_word();
        ui.dialog("clear", &format!("Clear this {what}?"), 360, |ui| {
            ui.close_label("Keep");
            ui.label(format!(
                "What you entered is removed and a new {what} starts."
            ));
            ui.actions(|ui| {
                if ui.button("Clear").clicked() {
                    clear = true;
                }
                if ui.primary("Keep").clicked() {
                    keep = true;
                }
            });
        });
        if clear {
            self.clear_ask = false;
            self.sheet = None;
            if self.screen == Screen::Expense {
                let f = if what == "trip" {
                    self.fresh_trip()
                } else {
                    self.fresh_form()
                };
                self.form = Some(f);
            }
        } else if keep {
            self.clear_ask = false;
        }
    }

    /// "trip" or "expense": what the form shown holds.
    fn form_word(&self) -> &'static str {
        if self.form.as_ref().is_some_and(|f| f.trip.is_some()) {
            "trip"
        } else {
            "expense"
        }
    }

    /// The status light: offline, waiting to send, or connected.
    fn status(&self) -> (&'static str, &'static str) {
        if !self.p.online() {
            ("red", "Status: offline")
        } else if self.not_reachable_shown() {
            // Revision 14: the computer is off, asleep or offline (revision 10's
            // "not on the same Wi-Fi" too).
            ("yellow", self.not_reachable_text().0)
        } else if self.signin
            || self.outbox.unpaired
            || self.outbox.items.iter().any(|i| i.state != State::Draft)
        {
            ("yellow", "Status: waiting")
        } else {
            ("green", "Status: connected")
        }
    }

    /// The words while the computer can't be reached: revision 18's Wi-Fi message
    /// when a Wi-Fi phone gets no answer at all, else revision 14's.
    fn not_reachable_text(&self) -> (&'static str, &'static str) {
        if self.wifi && self.no_answer {
            (msg::WIFI_UNREACHABLE, msg::WIFI_UNREACHABLE_DETAIL)
        } else {
            (msg::NOT_REACHABLE, msg::NOT_REACHABLE_DETAIL)
        }
    }

    /// The phone is online and paired, and the computer can't be reached.
    fn not_reachable_shown(&self) -> bool {
        self.unreachable && self.token.is_some() && self.p.online()
    }

    /// "Lists from Nov 8, 1:05 PM": the lists kept from the last load are used
    /// while the computer can't be reached (revision 14).
    fn lists_note(&self) -> Option<String> {
        let at = self.lists_at?;
        let local = |ms: i64| ms.saturating_add(self.p.utc_offset_min(ms).saturating_mul(60_000));
        let now = local(self.p.now_ms());
        let (this_year, _, _) = api::civil_from_days(now.div_euclid(86_400_000));
        Some(format!(
            "{} {}",
            msg::LISTS_FROM,
            calendar::stamp(local(at), this_year)
        ))
    }

    /// The yellow status text under the top bar while the computer can't be
    /// reached; on the forms, with the "Lists from …" note.
    /// U1.4: "Your computer's MoneyKeep needs updating" on the paired screens.
    fn server_notice_banner(&mut self, ui: &mut Ui) {
        let Some(notice) = self.server_notice else {
            return;
        };
        if self.token.is_none()
            || !matches!(
                self.screen,
                Screen::Expense | Screen::Edit | Screen::Outbox | Screen::Sent
            )
        {
            return;
        }
        ui.banner(Tone::Warning, |ui| {
            ui.bold();
            ui.label(notice);
            ui.label(msg::SERVER_NEEDS_UPDATE_DETAIL);
        });
    }

    fn not_reachable_banner(&mut self, ui: &mut Ui) {
        if !self.not_reachable_shown()
            || !matches!(
                self.screen,
                Screen::Expense | Screen::Edit | Screen::Outbox | Screen::Sent
            )
        {
            return;
        }
        let note = matches!(self.screen, Screen::Expense | Screen::Edit)
            .then(|| self.lists_note())
            .flatten();
        let (title, detail) = self.not_reachable_text();
        ui.banner(Tone::Warning, |ui| {
            ui.bold();
            ui.label(title);
            ui.label(detail);
            if let Some(n) = note {
                ui.secondary(n);
            }
        });
    }

    /// The top bar (revision 7): the back arrow on screens other than Home, the
    /// screen title, the status light and the menu.
    fn top_bar(&mut self, ui: &mut Ui) {
        let trip = self.form_word() == "trip";
        let title = match self.screen {
            Screen::Edit if self.form.as_ref().is_some_and(|f| f.server_id.is_some()) => {
                if trip {
                    "Change trip"
                } else {
                    "Change expense"
                }
            }
            Screen::Edit | Screen::Expense if trip => "Mileage",
            Screen::Edit | Screen::Expense => "Expense",
            Screen::Outbox => "Outbox",
            Screen::Sent => "Sent",
            _ => "MoneyKeep capture",
        };
        // Only an opened item has the back arrow; the tabs have none.
        let back = self.screen == Screen::Edit;
        let paired = self.token.is_some() && self.screen != Screen::Pair;
        let (state, name) = self.status();
        // Clear, on the Expense tab when the form has anything in it (revision 9).
        let clearable =
            self.screen == Screen::Expense && self.form.as_ref().is_some_and(Self::form_changed);
        let mut entries = Vec::new();
        if clearable {
            entries.push("Clear".to_owned());
        }
        entries.extend(["Refresh", "About this phone", "Sign out"].map(str::to_owned));
        let mut go_back = false;
        let mut chosen = None;
        ui.toolbar(|ui| {
            if back {
                ui.tip("Back");
                go_back = ui.icon_button("back", "Back", false).clicked();
            } else {
                // The title lines up with the content's 16 px margin.
                ui.next().width(8);
                ui.label("");
            }
            ui.next().grow();
            ui.heading(title);
            if self.screen != Screen::Loading {
                ui.tip(name);
                ui.item(
                    UiItem {
                        kind: Kind::Light,
                        value: state.to_owned(),
                        label: name.to_owned(),
                        width: 16,
                        ..UiItem::default()
                    },
                    false,
                );
            }
            if paired {
                // A 48 px tap target.
                ui.icon("list").named("Menu").tip("Menu").width(48);
                chosen = ui
                    .menu("", &entries)
                    .chosen
                    .and_then(|i| entries.get(i).cloned());
            }
        });
        if go_back {
            self.back();
        }
        match chosen.as_deref() {
            Some("Clear") => self.clear_ask = true,
            Some("Refresh") => {
                self.outbox.retry_now();
                self.refresh();
            }
            Some("About this phone") => self.about = true,
            Some("Sign out") => {
                self.p.storage().delete(TOKEN_KEY);
                self.token = None;
                self.pair_reason = None;
                self.form = None;
                self.draft = None;
                self.sheet = None;
                self.screen = Screen::Pair;
            }
            _ => {}
        }
    }

    /// "Discard this expense?" when leaving a changed form.
    fn discard_dialog(&mut self, ui: &mut Ui) {
        if !self.discard {
            return;
        }
        let mut keep = ui.dialog_closed("discard");
        let mut discard = false;
        let what = self.form_word();
        ui.dialog("discard", &format!("Discard this {what}?"), 360, |ui| {
            ui.close_label("Keep editing");
            ui.label("What you entered is not saved.");
            ui.actions(|ui| {
                if ui.button("Discard").clicked() {
                    discard = true;
                }
                if ui.primary("Keep editing").clicked() {
                    keep = true;
                }
            });
        });
        if discard {
            self.close_edit();
        } else if keep {
            self.discard = false;
        }
    }

    /// A field with its caption above it, full width.
    fn field(ui: &mut Ui, caption: &str, name: &str, hint: &str, text: &mut String) {
        ui.caption(caption);
        ui.row(|ui| {
            ui.next().grow();
            if !hint.is_empty() {
                ui.hint(hint);
            }
            ui.field(name, text);
        });
    }

    fn pair_screen(&mut self, ui: &mut Ui) {
        if let Some(reason) = self.pair_reason {
            ui.bold();
            ui.label(reason);
        }
        let install = self.p.install();
        if install.ios && !install.installed {
            ui.label(msg::IOS_INSTALL_FIRST);
        }
        // The browser's install offer shows here too: on a first visit the phone
        // is not paired yet (revision 9).
        self.install_hint(ui, true);
        if self.saving.is_some() {
            ui.label("Saving the pairing…");
            return;
        }
        ui.card("", |ui| {
            Self::field(
                ui,
                "Enter the code shown in MoneyKeep on your computer",
                "Pairing code",
                "8-character code",
                &mut self.pair_code,
            );
            self.pair_code = self.pair_code.chars().take(12).collect();
            Self::field(
                ui,
                "Name for this phone",
                "Name for this phone",
                "",
                &mut self.device_name,
            );
            self.device_name = self
                .device_name
                .chars()
                .take(api::MAX_DEVICE_NAME)
                .collect();
            if self.pin_required {
                self.pin_pad(ui);
            }
            let waiting = self.pending.iter().any(|(_, w)| *w == Pending::Pair);
            ui.row(|ui| {
                ui.enabled(!waiting);
                if ui.primary("Pair").clicked() {
                    self.send_pair();
                }
            });
            if let Some(e) = &self.pair_error {
                ui.error_text(e.clone());
            }
        });
    }

    /// The Capture PIN (revision 10): masked dots (or the digits, with the eye), and a
    /// digits keypad, so the phone's own keyboard is not needed. Revision 15: the
    /// label "Capture PIN" and a help line saying which PIN it is.
    fn pin_pad(&mut self, ui: &mut Ui) {
        ui.caption(msg::PIN_LABEL);
        ui.secondary(msg::PIN_HELP);
        let n = self.pin.chars().count();
        // Revision 18: a real text field with a visible outline (the theme's
        // border colour, the accent and 2 px with focus), at least 44 px tall,
        // under its label; the keypad below types into it too.
        ui.row(|ui| {
            ui.next()
                .grow()
                .id("capture-pin")
                .hint(msg::PIN_PLACEHOLDER)
                .named(format!(
                    "Capture PIN, {n} digit{} entered{}",
                    if n == 1 { "" } else { "s" },
                    if self.pin_shown { ", shown" } else { "" }
                ));
            let mut typed = self.pin.clone();
            let changed = if self.pin_shown {
                ui.field(msg::PIN_LABEL, &mut typed).changed()
            } else {
                ui.password(msg::PIN_LABEL, &mut typed).changed()
            };
            if changed {
                self.pin = typed
                    .chars()
                    .filter(char::is_ascii_digit)
                    .take(api::PIN_MAX)
                    .collect();
                if self.pin != typed {
                    ui.set_last_value(&self.pin);
                }
            }
            let (icon, name) = if self.pin_shown {
                ("eye-off", "Hide PIN")
            } else {
                ("eye", "Show PIN")
            };
            if ui.icon_toggle(icon, name, self.pin_shown).clicked() {
                self.pin_shown = !self.pin_shown;
            }
        });
        let n = self.pin.chars().count();
        let mut pressed: Option<char> = None;
        ui.grid("", 3, 52, |ui| {
            for row in [["1", "2", "3"], ["4", "5", "6"], ["7", "8", "9"]] {
                ui.row(|ui| {
                    for d in row {
                        if key(ui, d, d, "", "") {
                            pressed = d.chars().next();
                        }
                    }
                });
            }
            ui.row(|ui| {
                if key(ui, "Clear", "Clear PIN", "", "quiet") {
                    pressed = Some('c');
                }
                if key(ui, "0", "0", "", "") {
                    pressed = Some('0');
                }
                if key(ui, "", "Delete digit", "backspace", "") {
                    pressed = Some('b');
                }
            });
        });
        match pressed {
            Some('c') => self.pin.clear(),
            Some('b') => {
                self.pin.pop();
            }
            Some(d) if d.is_ascii_digit() && n < api::PIN_MAX => self.pin.push(d),
            _ => {}
        }
    }

    /// The Install hint, once, until dismissed (revision 3); `offer_only`: only the
    /// browser's offer (**Install app**), not the iPhone note (the Pair screen has
    /// its own).
    fn install_hint(&mut self, ui: &mut Ui, offer_only: bool) {
        let install = self.p.install();
        let ios = install.ios && !offer_only;
        if install.installed || self.hint_dismissed || !(install.can_prompt || ios) {
            return;
        }
        ui.card("", |ui| {
            if install.can_prompt {
                ui.label("Install MoneyKeep capture on this phone so it opens like an app and keeps its pairing.");
            } else {
                ui.label(msg::IOS_INSTALL);
            }
            ui.row(|ui| {
                if install.can_prompt && ui.primary("Install app").clicked() {
                    self.p.prompt_install();
                    self.dismiss_hint();
                }
                if ui.button("Not now").clicked() {
                    self.dismiss_hint();
                }
            });
        });
    }

    fn dismiss_hint(&mut self) {
        self.hint_dismissed = true;
        self.p.storage().put(HINT_KEY, "1");
    }

    /// `POST /pair` with the code and the phone's name.
    fn send_pair(&mut self) {
        let pin = self.pin.trim();
        let body = serde_json::to_vec(&api::PairRequest {
            code: api::normalize_code(&self.pair_code),
            device_name: self.device_name.clone(),
            pin: (!pin.is_empty()).then(|| pin.to_owned()),
        })
        .unwrap_or_default();
        self.pair_error = None;
        self.api(
            "POST",
            "/pair",
            Some(body),
            Some("application/json"),
            Pending::Pair,
        );
    }

    /// About: storage, installation, and the toolkit's notice (AboutSlint).
    fn about_dialog(&mut self, ui: &mut Ui) {
        if !self.about {
            return;
        }
        let mut close = ui.dialog_closed("about") || ui.key("escape");
        let storage = match self.p.persisted() {
            Some(true) => msg::STORAGE_KEPT,
            Some(false) => msg::STORAGE_MAY_CLEAR,
            None => "Storage: checking…",
        };
        let installed = self.p.install().installed;
        ui.dialog("about", "About this phone", 360, |ui| {
            ui.label(storage);
            ui.label(if installed {
                "Running as the installed app"
            } else {
                "Running in the browser"
            });
            ui.secondary("MoneyKeep capture is part of MoneyKeep (Apps for Financial Management).");
            ui.about_slint();
            ui.actions(|ui| {
                if ui.primary("Close").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.about = false;
        }
    }

    fn outbox_screen(&mut self, ui: &mut Ui) {
        // upgrade-resilience-1 M10: items a later version saved, kept on the phone.
        if let Some(note) = crate::outbox::unreadable_note(self.outbox.unreadable) {
            ui.banner(Tone::Normal, |ui| {
                ui.label(note);
            });
        }
        if self.outbox.items.is_empty() {
            ui.secondary("Nothing waiting to send.");
        } else if self.outbox.items.iter().any(|i| i.state == State::Queued) {
            if self.wifi {
                ui.secondary(msg::SEND_ON_WIFI);
            } else if self.not_reachable_shown() {
                // Revision 14: a Cloudflare phone while the computer is off.
                ui.secondary(msg::SEND_WHEN_ON);
            }
        }
        let now = self.p.now_ms();
        let online = self.p.online();
        let mut edit: Option<Form> = None;
        let mut delete: Option<String> = None;
        for i in &self.outbox.items {
            let status = match i.state {
                State::Draft => "Saved for later".to_owned(),
                State::NeedsAttention => i
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "Needs attention".into()),
                State::Queued if self.outbox.unpaired => msg::DISCONNECTED.to_owned(),
                State::Queued if !online => "Waiting for a connection".to_owned(),
                State::Queued if i.next_try_ms > now => format!(
                    "{} — trying again in {} s",
                    i.last_error.clone().unwrap_or_else(|| "Not sent".into()),
                    (i.next_try_ms - now) / 1000 + 1
                ),
                State::Queued => "Sending…".to_owned(),
            };
            ui.scope(i.client_id.clone(), |ui| {
                ui.card("", |ui| {
                    ui.bold();
                    ui.label(match &i.trip {
                        Some(t) => t.summary(),
                        None => summary(&i.fields, i.photos.len()),
                    });
                    ui.label(status);
                    ui.row(|ui| {
                        if ui.button("Edit").clicked() {
                            edit = Some(Self::form_from_item(i));
                        }
                        if ui.button("Delete").clicked() {
                            delete = Some(i.client_id.clone());
                        }
                    });
                });
            });
        }
        if let Some(f) = edit {
            self.open_form(f);
        }
        if let Some(c) = delete {
            let store = self.p.storage();
            self.outbox.remove(&c, store);
        }
    }

    fn sent_screen(&mut self, ui: &mut Ui) {
        let waiting = self.server.waiting.clone();
        let collected = self.server.collected.clone();
        let trips: Vec<TripSubmission> = self
            .trips
            .waiting
            .iter()
            .chain(self.trips.done.iter())
            .cloned()
            .collect();
        if waiting.is_empty() && collected.is_empty() && trips.is_empty() {
            ui.secondary("Nothing sent yet.");
        }
        let mut edit_sub: Option<Submission> = None;
        let mut del_sub: Option<String> = None;
        for s in waiting.iter().chain(collected.iter()) {
            let local = self.outbox.sent.iter().find(|x| x.client_id == s.client_id);
            let fields = if s.fields.is_empty() {
                local.map(|l| l.fields.clone()).unwrap_or_default()
            } else {
                s.fields.clone()
            };
            ui.scope(s.id.clone(), |ui| {
                ui.card("", |ui| {
                    ui.bold();
                    ui.label(summary(&fields, s.receipt_ids.len()));
                    match s.status {
                        SubmissionStatus::Waiting => {
                            ui.label("Waiting for your computer");
                            ui.row(|ui| {
                                if ui.button("Edit").clicked() {
                                    edit_sub = Some(s.clone());
                                }
                                if ui.button("Delete").clicked() {
                                    del_sub = Some(s.id.clone());
                                }
                            });
                        }
                        SubmissionStatus::Collected => {
                            let day = s
                                .collected_at
                                .map(|t| api::format_date(t.div_euclid(86_400)))
                                .unwrap_or_default();
                            ui.label(format!("Collected on {day}"));
                        }
                    }
                });
            });
        }
        // Trips (revision 11): waiting, taken with the book's amount, or refused
        // with the reason (and sent again as a new trip).
        let symbol = currency_symbol(&self.lists.base_currency).to_owned();
        let mut open_trip: Option<(TripSubmission, TripFields, bool)> = None;
        for s in &trips {
            let local = self.outbox.sent.iter().find(|x| x.client_id == s.client_id);
            let fields = if s.fields == TripFields::default() {
                local.and_then(|l| l.trip.clone()).unwrap_or_default()
            } else {
                s.fields.clone()
            };
            ui.scope(format!("trip:{}", s.id), |ui| {
                ui.card("", |ui| {
                    ui.bold();
                    ui.label(fields.summary());
                    let day = s
                        .done_at
                        .map(|t| api::format_date(t.div_euclid(86_400)))
                        .unwrap_or_default();
                    match s.status {
                        TripStatus::Waiting => {
                            ui.label("Waiting for your computer");
                            ui.row(|ui| {
                                if ui.button("Edit").clicked() {
                                    open_trip = Some((s.clone(), fields.clone(), true));
                                }
                                if ui.button("Delete").clicked() {
                                    del_sub = Some(format!("trip:{}", s.id));
                                }
                            });
                        }
                        TripStatus::Taken => ui.label(match &s.amount {
                            Some(a) => format!("Collected on {day} \u{b7} {symbol}{a}"),
                            None => format!("Collected on {day}"),
                        }),
                        TripStatus::Refused => {
                            ui.label(format!(
                                "Not taken: {}",
                                s.reason.clone().unwrap_or_default()
                            ));
                            ui.row(|ui| {
                                if ui.button("Send again").clicked() {
                                    open_trip = Some((s.clone(), fields.clone(), false));
                                }
                            });
                        }
                    }
                });
            });
        }
        if let Some((sub, fields, server)) = open_trip {
            let f = self.form_from_trip(&sub, &fields, server);
            self.open_form(f);
        }
        if let Some(s) = edit_sub {
            let f = self.form_from_submission(&s);
            self.open_form(f);
        }
        if let Some(id) = del_sub {
            let (path, id) = match id.strip_prefix("trip:") {
                Some(t) => (format!("/trips/{t}"), t.to_owned()),
                None => (format!("/expenses/{id}"), id),
            };
            self.api("DELETE", &path, None, None, Pending::DeleteSent(id));
        }
    }

    /// The expense form (revision 7): the Amount card, Date, Merchant, the pickers,
    /// Purpose, the receipt photos; Send and Save for later in the bottom bar.
    fn form_screen(&mut self, ui: &mut Ui) {
        let mut f = match self.form.take() {
            Some(f) => f,
            None if self.screen == Screen::Expense => self.fresh_form(),
            None => {
                self.screen = self.edit_from;
                return;
            }
        };
        if f.trip.is_some() {
            self.trip_screen(ui, f);
            return;
        }
        let lists = self.lists.clone();
        let today = api::parse_date(&self.p.today()).unwrap_or(0);
        // 1. The Amount card: one button that opens the keypad.
        let set = !f.amount.trim().is_empty();
        let shown = if set {
            f.amount.clone()
        } else {
            "0.00".to_owned()
        };
        let amount = UiItem {
            kind: Kind::AmountCard,
            text: shown.clone(),
            value: currency_symbol(&lists.base_currency).to_owned(),
            placeholder: "Amount".to_owned(),
            label: format!("Amount, {shown}"),
            tone: if set { Tone::Normal } else { Tone::Secondary },
            grow: true,
            ..UiItem::default()
        };
        ui.group(|ui| {
            ui.next().id("amount");
            if ui.item(amount, true).clicked() {
                self.sheet = Some(Sheet::Keypad {
                    expr: keypad::start(&f.amount),
                    error: None,
                    target: Target::Amount,
                });
            }
        });
        ui.card("", |ui| {
            // 2. Date: a field-styled button that opens the calendar.
            let day = api::parse_date(&f.date);
            let date_text = day.map_or_else(|| f.date.clone(), calendar::long_form);
            ui.caption("Date");
            ui.row(|ui| {
                ui.next().id("date");
                let b = UiItem {
                    kind: Kind::FieldButton,
                    text: date_text.clone(),
                    label: format!("Date, {date_text}"),
                    icon: "sym-calendar".to_owned(),
                    grow: true,
                    ..UiItem::default()
                };
                if ui.item(b, true).clicked() {
                    self.sheet = Some(Sheet::Calendar {
                        month: calendar::month_of(day.unwrap_or(today)),
                    });
                }
            });
            // 3. Merchant with the recent merchants as suggestions.
            Self::field(ui, "Merchant", "Merchant", "", &mut f.merchant);
            f.merchant = f.merchant.chars().take(api::MAX_MERCHANT).collect();
            let typed = f.merchant.trim().to_lowercase();
            if !typed.is_empty() {
                let hits: Vec<String> = lists
                    .merchants
                    .iter()
                    .filter(|m| m.to_lowercase().starts_with(&typed) && m.to_lowercase() != typed)
                    .take(4)
                    .cloned()
                    .collect();
                if !hits.is_empty() {
                    ui.wrap(|ui| {
                        for m in hits {
                            if ui.button(m.clone()).clicked() {
                                f.merchant = m;
                            }
                        }
                    });
                }
            }
            // 4. Category and Paid with, then Purpose.
            pick(ui, "Category", &mut f.category, &lists.categories);
            pick(ui, "Paid with", &mut f.paid_with, &lists.paid_with);
            if !lists.tags.class.is_empty() {
                pick(ui, "Class", &mut f.class, &lists.tags.class);
            }
            if !lists.tags.location.is_empty() {
                pick(ui, "Location", &mut f.location, &lists.tags.location);
            }
            if !lists.tags.fund.is_empty() {
                pick(ui, "Fund", &mut f.fund, &lists.tags.fund);
            }
            if !lists.tags.project.is_empty() {
                pick(ui, "Project", &mut f.project, &lists.tags.project);
            }
            ui.caption("Purpose (optional)");
            ui.row(|ui| {
                ui.next().grow();
                ui.multiline("Purpose (optional)", &mut f.purpose);
            });
        });
        // 5. Receipt photos: Camera and Gallery, then the thumbnails.
        let mut remove: Option<usize> = None;
        let mut open_photo: Option<usize> = None;
        let mut pick_camera = None;
        let photos = f.photos.clone();
        ui.card("Receipt photos", |ui| {
            if f.photos.len() < MAX_PHOTOS {
                ui.row(|ui| {
                    ui.next().grow().icon("sym-camera");
                    if ui.button("Camera").clicked() {
                        pick_camera = Some(true);
                    }
                    ui.next().grow().icon("gallery");
                    if ui.button("Gallery").clicked() {
                        pick_camera = Some(false);
                    }
                });
            }
            if !photos.is_empty() {
                ui.grid("", 3, 88, |ui| {
                    for chunk in photos.chunks(3).enumerate() {
                        let (row, list) = chunk;
                        ui.row(|ui| {
                            for (k, p) in list.iter().enumerate() {
                                let n = row * 3 + k;
                                let key = format!("{}:{n}", f.client_id);
                                let thumb = UiItem {
                                    kind: Kind::Thumb,
                                    text: format!("Photo {}", n + 1),
                                    label: format!("Photo {}", n + 1),
                                    desc: format!("Remove photo {}", n + 1),
                                    picture: self.thumb(&key, p),
                                    ..UiItem::default()
                                };
                                let r = ui.item(thumb, true);
                                if r.clicked() {
                                    if r.value == "remove" {
                                        remove = Some(n);
                                    } else {
                                        open_photo = Some(n);
                                    }
                                }
                            }
                        });
                    }
                });
            }
        });
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        if let Some(camera) = pick_camera {
            self.p.pick_photos(camera);
        }
        if let Some(n) = remove
            && n < f.photos.len()
        {
            f.photos.remove(n);
            self.thumbs.retain(|k, _| !k.starts_with(&f.client_id));
        }
        if let Some(n) = open_photo {
            self.sheet = Some(Sheet::Photo(n));
        }
        let action = self.action_row(ui, None);
        self.sheets(ui, &mut f, today);
        self.form = Some(f);
        match action {
            1 => self.save_form(true),
            2 => self.save_form(false),
            _ => {}
        }
    }

    /// The action row: Send (wide) and Save for later, fixed above the tab bar,
    /// with the confirmation of the last one above it for a few seconds, and
    /// revision 11's one-time tip above them, and a trip's computed line (always in
    /// sight). 1: Send, 2: Save for later.
    fn action_row(&mut self, ui: &mut Ui, line: Option<String>) -> i32 {
        let now = self.p.now_ms();
        if self.notice.is_some_and(|(_, until)| now >= until) {
            self.notice = None;
        }
        let notice = self.notice.filter(|_| self.screen == Screen::Expense);
        let tip = self.screen == Screen::Expense && self.lists.mileage_enabled && !self.tip_seen;
        let mut close_tip = false;
        let mut action = 0;
        ui.footer(|ui| {
            if tip {
                ui.row(|ui| {
                    ui.next().grow().icon("sym-car");
                    if ui.button(msg::MILEAGE_TIP).clicked() {
                        close_tip = true;
                    }
                    ui.tip("Close tip");
                    if ui.icon_button("close", "Close tip", false).clicked() {
                        close_tip = true;
                    }
                });
            }
            if let Some((word, _)) = notice {
                ui.row(|ui| {
                    ui.next().grow().align(Align::Center);
                    ui.bold();
                    ui.label(word);
                });
            } else if let Some(l) = line {
                ui.row(|ui| {
                    ui.next().grow().align(Align::Center);
                    ui.bold();
                    ui.label(l);
                });
            }
            ui.row(|ui| {
                ui.next().grow();
                if ui.primary("Send").clicked() {
                    action = 1;
                }
                if ui.button("Save for later").clicked() {
                    action = 2;
                }
            });
        });
        if close_tip {
            self.close_tip();
        }
        action
    }

    /// The Mileage page's form (mileage.md "Phone"): Saved trip, Date, Vehicle,
    /// Purpose kind, the Miles card or the odometer readings, Round trip, From and
    /// To with the last places as suggestions, Purpose, Reimbursable, and the
    /// computed line; Send and Save for later as for expenses.
    fn trip_screen(&mut self, ui: &mut Ui, mut f: Form) {
        let lists = self.lists.clone();
        let today = api::parse_date(&self.p.today()).unwrap_or(0);
        let mut t = f.trip.clone().unwrap_or_default();
        ui.card("", |ui| {
            if !lists.saved_trips.is_empty() {
                let caption = "Saved trip (optional)";
                ui.caption(caption);
                ui.row(|ui| {
                    ui.next().grow();
                    let mut options = vec!["None".to_owned()];
                    options.extend(lists.saved_trips.iter().map(|s| s.name.clone()));
                    let mut idx = if t.saved_trip_id.is_empty() {
                        0
                    } else {
                        lists
                            .saved_trips
                            .iter()
                            .position(|s| s.id == t.saved_trip_id)
                            .map_or(usize::MAX, |i| i + 1)
                    };
                    if ui.combo(caption, &mut idx, &options).changed() {
                        match idx.checked_sub(1).and_then(|i| lists.saved_trips.get(i)) {
                            Some(st) => apply_saved(&mut t, st, &lists),
                            None => t.saved_trip_id.clear(),
                        }
                    }
                });
            }
            self.date_field(ui, &f, today);
            ui.caption("Vehicle");
            ui.row(|ui| {
                ui.next().grow();
                if lists.vehicles.is_empty() {
                    ui.secondary(
                        "No vehicles yet. Add one in MoneyKeep on your computer; this trip waits there for a vehicle.",
                    );
                    return;
                }
                let mut options = vec!["Choose a vehicle".to_owned()];
                options.extend(lists.vehicles.iter().map(|v| v.name.clone()));
                let mut idx = lists
                    .vehicles
                    .iter()
                    .position(|v| v.id == t.vehicle_id)
                    .map_or(0, |i| i + 1);
                if ui.combo("Vehicle", &mut idx, &options).changed() {
                    t.vehicle_id = idx
                        .checked_sub(1)
                        .and_then(|i| lists.vehicles.get(i))
                        .map(|v| v.id.clone())
                        .unwrap_or_default();
                }
            });
            let kinds = purpose_kinds(&lists, &t.purpose_kind);
            ui.caption("Purpose kind");
            ui.row(|ui| {
                ui.next().grow();
                let options: Vec<String> = kinds.iter().map(|k| k.1.clone()).collect();
                let mut idx = kinds
                    .iter()
                    .position(|k| k.0 == t.purpose_kind)
                    .unwrap_or(usize::MAX);
                if ui.combo("Purpose kind", &mut idx, &options).changed()
                    && let Some(k) = kinds.get(idx)
                {
                    t.purpose_kind = k.0.clone();
                }
            });
            // Miles typed, or the odometer's Start and End (below).
            if ui.switch(&mut t.odometer, "Enter odometer readings").changed() && t.odometer {
                // The round trip is already in the readings.
                t.round_trip = false;
            }
        });
        // The Miles card (opens the keypad), or the odometer readings.
        if t.odometer {
            ui.card("", |ui| {
                for (caption, value, target, id) in [
                    ("Start", t.odo_start.clone(), Target::OdoStart, "odo-start"),
                    ("End", t.odo_end.clone(), Target::OdoEnd, "odo-end"),
                ] {
                    ui.caption(caption);
                    ui.row(|ui| {
                        ui.next().id(id);
                        let shown = if value.is_empty() {
                            "Enter reading".to_owned()
                        } else {
                            value.clone()
                        };
                        let b = UiItem {
                            kind: Kind::FieldButton,
                            text: shown.clone(),
                            label: format!("Odometer {}, {shown}", caption.to_lowercase()),
                            icon: "sym-calculator".to_owned(),
                            grow: true,
                            ..UiItem::default()
                        };
                        if ui.item(b, true).clicked() {
                            self.sheet = Some(Sheet::Keypad {
                                expr: keypad::start(&value),
                                error: None,
                                target,
                            });
                        }
                    });
                }
            });
        } else {
            let set = !t.miles.trim().is_empty();
            let shown = if set {
                t.miles.clone()
            } else {
                "0.0".to_owned()
            };
            let caption = if t.round_trip {
                "Miles, one way"
            } else {
                "Miles"
            };
            let card = UiItem {
                kind: Kind::AmountCard,
                text: shown.clone(),
                desc: "mi".to_owned(),
                placeholder: caption.to_owned(),
                label: format!("{caption}, {shown}"),
                tone: if set { Tone::Normal } else { Tone::Secondary },
                grow: true,
                ..UiItem::default()
            };
            ui.group(|ui| {
                ui.next().id("miles");
                if ui.item(card, true).clicked() {
                    self.sheet = Some(Sheet::Keypad {
                        expr: keypad::start(&t.miles),
                        error: None,
                        target: Target::Miles,
                    });
                }
            });
        }
        ui.card("", |ui| {
            ui.enabled(!t.odometer);
            ui.switch(&mut t.round_trip, "Round trip");
            if t.round_trip
                && let Ok(one) = api::parse_tenths(&t.miles)
                && one > 0
            {
                ui.secondary(format!(
                    "One way {} mi \u{2192} {} mi",
                    api::tenths_text(one),
                    api::tenths_text(one.saturating_mul(2))
                ));
            }
            for (caption, which) in [("From", false), ("To", true)] {
                let text = if which {
                    &mut t.to_place
                } else {
                    &mut t.from_place
                };
                Self::field(ui, caption, caption, "", text);
                *text = text.chars().take(api::MAX_PLACE).collect();
                let typed = text.trim().to_lowercase();
                if !typed.is_empty() {
                    let hits: Vec<String> = self
                        .places
                        .iter()
                        .filter(|p| {
                            let l = p.to_lowercase();
                            l.starts_with(&typed) && l != typed
                        })
                        .take(4)
                        .cloned()
                        .collect();
                    if !hits.is_empty() {
                        ui.wrap(|ui| {
                            for p in hits {
                                if ui.button(p.clone()).clicked() {
                                    *text = p;
                                }
                            }
                        });
                    }
                }
            }
            let personal = t.purpose_kind == "personal";
            ui.caption(if personal {
                "Purpose (optional)"
            } else {
                "Purpose"
            });
            ui.row(|ui| {
                ui.next().grow();
                ui.multiline("Purpose", &mut t.purpose);
            });
            t.purpose = t.purpose.chars().take(api::MAX_TRIP_PURPOSE).collect();
            // mileage.md revision 2: the tags below Purpose, each only when the
            // book has entries for it (as the Expense form).
            for (label, value, choices) in [
                ("Class", &mut t.class, &lists.tags.class),
                ("Location", &mut t.location, &lists.tags.location),
                ("Fund", &mut t.fund, &lists.tags.fund),
                ("Project", &mut t.project, &lists.tags.project),
            ] {
                if !choices.is_empty() {
                    pick(ui, label, value, choices);
                }
            }
            if lists.reimbursable_enabled && !personal {
                ui.switch(&mut t.reimbursable, "Reimbursable");
            }
        });
        if let Some(e) = &f.error {
            ui.error_text(e.clone());
        }
        let line = trip_line(&t, &f.date, &lists);
        let action = self.action_row(ui, Some(line));
        f.trip = Some(t);
        self.sheets(ui, &mut f, today);
        self.form = Some(f);
        match action {
            1 => self.save_form(true),
            2 => self.save_form(false),
            _ => {}
        }
    }

    /// The Date field (a field-styled button that opens the calendar).
    fn date_field(&mut self, ui: &mut Ui, f: &Form, today: i64) {
        let day = api::parse_date(&f.date);
        let date_text = day.map_or_else(|| f.date.clone(), calendar::long_form);
        ui.caption("Date");
        ui.row(|ui| {
            ui.next().id("date");
            let b = UiItem {
                kind: Kind::FieldButton,
                text: date_text.clone(),
                label: format!("Date, {date_text}"),
                icon: "sym-calendar".to_owned(),
                grow: true,
                ..UiItem::default()
            };
            if ui.item(b, true).clicked() {
                self.sheet = Some(Sheet::Calendar {
                    month: calendar::month_of(day.unwrap_or(today)),
                });
            }
        });
    }

    /// The open sheet over the form: the keypad, the calendar or a photo.
    fn sheets(&mut self, ui: &mut Ui, f: &mut Form, today: i64) {
        match self.sheet.clone() {
            Some(Sheet::Keypad {
                expr,
                error,
                target,
            }) => self.keypad_sheet(ui, f, expr, error, target),
            Some(Sheet::Calendar { month }) => self.calendar_sheet(ui, f, month, today),
            Some(Sheet::Photo(n)) => {
                let closed = ui.dialog_closed("photo");
                let pic = f
                    .photos
                    .get(n)
                    .cloned()
                    .and_then(|p| self.thumb(&format!("{}:{n}", f.client_id), &p));
                ui.sheet("photo", "", true, |ui| {
                    ui.close_label("Close photo");
                    ui.row(|ui| {
                        ui.spacer();
                        match pic {
                            Some(p) => {
                                ui.picture(&format!("Photo {}", n + 1), p, 340);
                            }
                            None => ui.label(format!("Photo {}", n + 1)),
                        }
                        ui.spacer();
                    });
                });
                if closed || n >= f.photos.len() {
                    self.sheet = None;
                }
            }
            None => {}
        }
    }

    /// The amount keypad (revision 7).
    fn keypad_sheet(
        &mut self,
        ui: &mut Ui,
        f: &mut Form,
        mut expr: String,
        mut error: Option<&'static str>,
        target: Target,
    ) {
        let closed = ui.dialog_closed("keypad");
        let mut done = false;
        let mut pressed: Option<keypad::Key> = None;
        let mut added: Option<i64> = None;
        ui.sheet("keypad", "", false, |ui| {
            ui.close_label("Close keypad");
            let what = match target {
                Target::Amount => "",
                Target::Miles => "Miles",
                Target::OdoStart => "Odometer start",
                Target::OdoEnd => "Odometer end",
            };
            if !what.is_empty() {
                ui.caption(what);
            }
            ui.row(|ui| {
                ui.next().grow().align(Align::Right);
                ui.big_heading(keypad::show(&expr));
            });
            ui.row(|ui| {
                ui.next().grow().align(Align::Right);
                ui.secondary(if target == Target::Amount {
                    keypad::result_line(&expr)
                } else {
                    keypad::result_line_tenths(&expr)
                });
            });
            if let Some(e) = error {
                ui.row(|ui| {
                    ui.next().grow().align(Align::Right);
                    ui.error_text(e);
                });
            }
            // The quick-add row (not for odometer readings).
            if matches!(target, Target::Amount | Target::Miles) {
                ui.grid("", 4, 48, |ui| {
                    ui.row(|ui| {
                        for n in [5, 10, 20, 50] {
                            if key(ui, &format!("+{n}"), &format!("Add {n}"), "", "") {
                                added = Some(n);
                            }
                        }
                    });
                });
            }
            ui.grid("", 4, 56, |ui| {
                use keypad::Key as K;
                let rows: [[KeySpec; 4]; 4] = [
                    [
                        ("7", "7", "", "", K::Digit(7)),
                        ("8", "8", "", "", K::Digit(8)),
                        ("9", "9", "", "", K::Digit(9)),
                        ("", "Divide", "divide", "op", K::Op('/')),
                    ],
                    [
                        ("4", "4", "", "", K::Digit(4)),
                        ("5", "5", "", "", K::Digit(5)),
                        ("6", "6", "", "", K::Digit(6)),
                        ("", "Multiply", "multiply", "op", K::Op('*')),
                    ],
                    [
                        ("1", "1", "", "", K::Digit(1)),
                        ("2", "2", "", "", K::Digit(2)),
                        ("3", "3", "", "", K::Digit(3)),
                        ("", "Minus", "minus", "op", K::Op('-')),
                    ],
                    [
                        (".", "Decimal point", "", "", K::Point),
                        ("0", "0", "", "", K::Digit(0)),
                        ("", "Delete", "backspace", "", K::Backspace),
                        ("", "Plus", "plus", "op", K::Op('+')),
                    ],
                ];
                for r in rows {
                    ui.row(|ui| {
                        for (face, name, icon, style, k) in r {
                            if key(ui, face, name, icon, style) {
                                pressed = Some(k);
                            }
                        }
                    });
                }
                ui.row(|ui| {
                    if key(ui, "Clear", "Clear", "", "quiet") {
                        pressed = Some(K::Clear);
                    }
                    if key(ui, "=", "Equals", "", "op") {
                        pressed = Some(K::Equals);
                    }
                    if key(ui, "Done", "Done", "", "primary") {
                        done = true;
                    }
                    if let Some(it) = ui.last() {
                        it.maximum = 2;
                    }
                });
            });
        });
        if let Some(k) = pressed {
            expr = keypad::press(&expr, k);
            error = None;
        }
        if let Some(n) = added {
            expr = keypad::add(&expr, n);
            error = None;
        }
        if closed {
            // Closed without Done: the amount stays as it was.
            self.sheet = None;
        } else if done {
            self.keypad_done(f, &expr, target);
        } else {
            self.sheet = Some(Sheet::Keypad {
                expr,
                error,
                target,
            });
        }
    }

    /// Done: the amount (or the miles, or a reading), or the refusal shown in the
    /// sheet (which stays open).
    fn keypad_done(&mut self, f: &mut Form, expr: &str, target: Target) {
        let result = match target {
            Target::Amount => keypad::done(expr),
            Target::Miles => keypad::done_tenths(expr, false),
            Target::OdoStart | Target::OdoEnd => keypad::done_tenths(expr, true),
        };
        match result {
            Ok(v) => {
                match (target, f.trip.as_mut()) {
                    (Target::Amount, _) => f.amount = v,
                    (Target::Miles, Some(t)) => t.miles = v,
                    (Target::OdoStart, Some(t)) => t.odo_start = v,
                    (Target::OdoEnd, Some(t)) => t.odo_end = v,
                    _ => {}
                }
                self.sheet = None;
            }
            Err(e) => {
                self.sheet = Some(Sheet::Keypad {
                    expr: expr.to_owned(),
                    error: Some(e),
                    target,
                });
            }
        }
    }

    /// The calendar (revision 7).
    fn calendar_sheet(
        &mut self,
        ui: &mut Ui,
        f: &mut Form,
        mut month: calendar::Month,
        today: i64,
    ) {
        let closed = ui.dialog_closed("calendar");
        let selected = api::parse_date(&f.date);
        let mut picked: Option<i64> = None;
        let mut step = 0;
        ui.sheet("calendar", "", false, |ui| {
            ui.close_label("Close calendar");
            ui.row(|ui| {
                ui.enabled(calendar::month_in_range(
                    calendar::step_month(month, -1),
                    today,
                ));
                ui.tip("Previous month");
                if ui.icon_button("back", "Previous month", false).clicked() {
                    step = -1;
                }
                ui.next().grow().align(Align::Center);
                ui.heading(calendar::month_title(month));
                ui.enabled(calendar::month_in_range(
                    calendar::step_month(month, 1),
                    today,
                ));
                ui.tip("Next month");
                if ui.icon_button("forward", "Next month", false).clicked() {
                    step = 1;
                }
            });
            ui.grid("", 7, 24, |ui| {
                ui.row(|ui| {
                    for wd in calendar::WEEKDAYS {
                        ui.next().align(Align::Center).named(wd);
                        ui.caption(wd.get(..3).unwrap_or(wd));
                    }
                });
            });
            ui.grid("", 7, 48, |ui| {
                for week in calendar::month_grid(month) {
                    ui.row(|ui| {
                        for c in week {
                            let mut name = calendar::full_name(c.days);
                            if c.days == today {
                                name.push_str(", today");
                            }
                            if Some(c.days) == selected {
                                name.push_str(", selected");
                            }
                            ui.next().id(format!("day:{}", api::format_date(c.days)));
                            let d = UiItem {
                                kind: Kind::Day,
                                text: c.day.to_string(),
                                label: name,
                                checked: Some(c.days) == selected,
                                value: if c.days == today {
                                    "today".to_owned()
                                } else {
                                    String::new()
                                },
                                tone: if c.in_month {
                                    Tone::Normal
                                } else {
                                    Tone::Secondary
                                },
                                enabled: calendar::in_range(c.days, today),
                                ..UiItem::default()
                            };
                            if ui.item(d, true).clicked() {
                                picked = Some(c.days);
                            }
                        }
                    });
                }
            });
            ui.actions(|ui| {
                if ui.button("Today").clicked() {
                    picked = Some(today);
                }
            });
        });
        if let Some(d) = picked {
            f.date = api::format_date(d);
            self.sheet = None;
        } else if closed {
            self.sheet = None;
        } else {
            if step != 0 {
                month = calendar::step_month(month, step);
            }
            self.sheet = Some(Sheet::Calendar { month });
        }
    }

    /// A key typed on a computer's keyboard while nothing else took it.
    fn key_typed(&mut self, name: &str) -> bool {
        if self.discard {
            if name == "escape" {
                self.discard = false;
                return true;
            }
            return false;
        }
        match self.sheet.clone() {
            Some(Sheet::Keypad { expr, target, .. }) => {
                if name == "escape" {
                    self.sheet = None;
                    return true;
                }
                if name == "enter" {
                    if let Some(mut f) = self.form.take() {
                        self.keypad_done(&mut f, &expr, target);
                        self.form = Some(f);
                    }
                    return true;
                }
                match keypad::from_key_name(name) {
                    Some(k) => {
                        self.sheet = Some(Sheet::Keypad {
                            expr: keypad::press(&expr, k),
                            error: None,
                            target,
                        });
                        true
                    }
                    None => false,
                }
            }
            Some(_) if name == "escape" => {
                self.sheet = None;
                true
            }
            // The Capture PIN takes digits from a hardware keyboard too.
            None if self.screen == Screen::Pair && self.pin_required => match name {
                "backspace" => {
                    self.pin.pop();
                    true
                }
                d if d.len() == 1
                    && d.bytes().all(|b| b.is_ascii_digit())
                    && self.pin.len() < api::PIN_MAX =>
                {
                    self.pin.push_str(d);
                    true
                }
                _ => false,
            },
            _ if name == "escape" && self.about => {
                self.about = false;
                true
            }
            _ if name == "escape" && self.clear_ask => {
                self.clear_ask = false;
                true
            }
            _ => false,
        }
    }
}

/// A keypad key: face, accessible name, icon, style and what it does.
type KeySpec = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    keypad::Key,
);

/// A keypad key (`face` drawn, or `icon`; `name` its accessible name); whether
/// it was tapped.
fn key(ui: &mut Ui, face: &str, name: &str, icon: &str, style: &str) -> bool {
    ui.item(
        UiItem {
            kind: Kind::Key,
            text: face.to_owned(),
            label: name.to_owned(),
            icon: icon.to_owned(),
            value: style.to_owned(),
            ..UiItem::default()
        },
        true,
    )
    .clicked()
}

/// The currency symbol shown before the amount.
fn currency_symbol(code: &str) -> &str {
    match code {
        "" | "USD" | "CAD" | "AUD" | "NZD" | "MXN" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" | "CNY" => "¥",
        "INR" => "₹",
        "KRW" => "₩",
        other => other,
    }
}

impl<P: Platform + 'static> mk_views::phone::PageApp for CaptureApp<P> {
    fn build(&mut self, ui: &mut Ui) {
        self.view(ui);
    }
    fn tick(&mut self) -> bool {
        CaptureApp::tick(self);
        true
    }
    fn key(&mut self, name: &str) -> bool {
        self.key_typed(name)
    }
    fn screen_key(&self) -> String {
        format!("{:?}", self.screen)
    }
}

/// The trip form from a trip's fields.
fn trip_form_from(t: &TripFields) -> TripForm {
    let odometer = t.odometer_start.is_some() && t.odometer_end.is_some();
    TripForm {
        saved_trip_id: t.saved_trip_id.clone().unwrap_or_default(),
        vehicle_id: t.vehicle_id.clone(),
        purpose_kind: t.purpose_kind.clone(),
        miles: if odometer {
            String::new()
        } else {
            t.miles.clone()
        },
        odometer,
        odo_start: t.odometer_start.clone().unwrap_or_default(),
        odo_end: t.odometer_end.clone().unwrap_or_default(),
        round_trip: t.round_trip,
        from_place: t.from_place.clone(),
        to_place: t.to_place.clone(),
        purpose: t.purpose.clone(),
        reimbursable: t.reimbursable,
        class: t.tags.class.clone().unwrap_or_default(),
        location: t.tags.location.clone().unwrap_or_default(),
        fund: t.tags.fund.clone().unwrap_or_default(),
        project: t.tags.project.clone().unwrap_or_default(),
    }
}

/// The miles of the whole trip in tenths, when known: end − start, or the miles
/// (twice for a round trip).
fn trip_tenths(t: &TripForm) -> Option<u64> {
    if t.odometer {
        let s = api::parse_tenths(&t.odo_start).ok()?;
        let e = api::parse_tenths(&t.odo_end).ok()?;
        return e.checked_sub(s).filter(|d| *d > 0);
    }
    let m = api::parse_tenths(&t.miles).ok()?;
    Some(if t.round_trip { m.saturating_mul(2) } else { m })
}

/// The trip as sent (`reimbursable_on`: the book's expenses module is on).
fn trip_fields_of(f: &Form, t: &TripForm, reimbursable_on: bool) -> TripFields {
    let reading = |v: &str| (t.odometer && !v.trim().is_empty()).then(|| v.trim().to_owned());
    TripFields {
        date: f.date.trim().to_owned(),
        vehicle_id: t.vehicle_id.clone(),
        purpose_kind: t.purpose_kind.clone(),
        purpose: t.purpose.trim().to_owned(),
        from_place: t.from_place.trim().to_owned(),
        to_place: t.to_place.trim().to_owned(),
        miles: if t.odometer {
            trip_tenths(t).map(api::tenths_text).unwrap_or_default()
        } else {
            t.miles.trim().to_owned()
        },
        odometer_start: reading(&t.odo_start),
        odometer_end: reading(&t.odo_end),
        round_trip: t.round_trip && !t.odometer,
        reimbursable: t.reimbursable && reimbursable_on && t.purpose_kind != "personal",
        saved_trip_id: opt(&t.saved_trip_id),
        tags: Tags {
            class: opt(&t.class),
            location: opt(&t.location),
            fund: opt(&t.fund),
            project: opt(&t.project),
        },
    }
}

/// The computed line: "24.8 mi × 72.5¢ = $17.98" (the phone's copy of the book's
/// rate table; the book recomputes when it takes the trip).
fn trip_line(t: &TripForm, date: &str, lists: &Lists) -> String {
    let total = trip_tenths(t).unwrap_or(0);
    let miles = api::tenths_text(total);
    if t.purpose_kind == "personal" {
        return format!("{miles} mi \u{b7} personal, no rate");
    }
    match api::rate_for(&lists.mileage_rates, &t.purpose_kind, date) {
        Some(c) => format!(
            "{miles} mi \u{d7} {}\u{a2} = {}{}",
            api::cents_text(c),
            currency_symbol(&lists.base_currency),
            api::money_text(api::trip_value_cents(total, c))
        ),
        None => format!("{miles} mi \u{b7} no standard rate for this date"),
    }
}

/// A saved trip fills the form: route, one-way miles, round trip, purpose kind,
/// purpose and vehicle (when it is one of the active ones).
fn apply_saved(t: &mut TripForm, s: &api::ListSavedTrip, lists: &Lists) {
    t.saved_trip_id.clone_from(&s.id);
    t.from_place = s.from_place.clone().unwrap_or_default();
    t.to_place = s.to_place.clone().unwrap_or_default();
    t.miles.clone_from(&s.miles);
    t.round_trip = s.round_trip;
    t.odometer = false;
    if api::is_trip_kind(&s.purpose_kind) {
        t.purpose_kind.clone_from(&s.purpose_kind);
    }
    t.purpose = s.purpose.clone().unwrap_or_default();
    if let Some(v) = s
        .vehicle
        .as_ref()
        .filter(|v| lists.vehicles.iter().any(|x| &x.id == *v))
    {
        t.vehicle_id.clone_from(v);
    }
}

/// The purpose kinds offered (kind, label): the book's, else Business, Medical,
/// Charity and Personal; the trip's own kind stays offered.
fn purpose_kinds(lists: &Lists, current: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = lists
        .purpose_kinds
        .iter()
        .filter(|k| api::is_trip_kind(&k.kind))
        .map(|k| {
            let label = if k.label.is_empty() {
                capitalised(&k.kind)
            } else {
                k.label.clone()
            };
            (k.kind.clone(), label)
        })
        .collect();
    if v.is_empty() {
        v = ["business", "medical", "charity", "personal"]
            .iter()
            .map(|k| ((*k).to_owned(), capitalised(k)))
            .collect();
    }
    if api::is_trip_kind(current) && !v.iter().any(|k| k.0 == current) {
        v.push((current.to_owned(), capitalised(current)));
    }
    v
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// "2026-10-23 · Office Depot · 45.20 · 1 photo".
fn summary(f: &ExpenseFields, photos: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    for v in [&f.date, &f.merchant, &f.amount].into_iter().flatten() {
        parts.push(v.clone());
    }
    if photos > 0 {
        parts.push(format!(
            "{photos} photo{}",
            if photos == 1 { "" } else { "s" }
        ));
    }
    if parts.is_empty() {
        "Expense".into()
    } else {
        parts.join(" · ")
    }
}

/// A picker from a published list (free text when nothing was published yet).
fn pick(ui: &mut Ui, label: &str, value: &mut String, choices: &[String]) {
    let caption = format!("{label} (optional)");
    ui.caption(caption.clone());
    ui.row(|ui| {
        ui.next().grow();
        if choices.is_empty() {
            ui.field(&caption, value);
            return;
        }
        let mut options = vec!["None".to_owned()];
        options.extend(choices.iter().cloned());
        let mut idx = if value.is_empty() {
            0
        } else {
            choices
                .iter()
                .position(|c| c == value)
                .map_or(usize::MAX, |i| i + 1)
        };
        if ui.combo(&caption, &mut idx, &options).changed() {
            *value = if idx == 0 {
                String::new()
            } else {
                options.get(idx).cloned().unwrap_or_default()
            };
        }
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Default)]
    struct Mem(BTreeMap<String, String>);

    impl Storage for Mem {
        fn put(&mut self, key: &str, value: &str) {
            self.0.insert(key.into(), value.into());
        }
        fn delete(&mut self, key: &str) {
            self.0.remove(key);
        }
    }

    /// A scripted browser: answers come from `replies` by path.
    #[derive(Default)]
    struct Fake {
        now: i64,
        online: bool,
        store: Mem,
        loaded: Option<Vec<(String, String)>>,
        fragment: Option<String>,
        /// Token writes fail (private browsing).
        fail_writes: bool,
        saved: Vec<(u64, Result<(), String>)>,
        persist_asked: usize,
        marker: bool,
        install: Install,
        starts: usize,
        sent: Vec<(u64, Request)>,
        answers: Vec<(u64, Response)>,
        next: u64,
        /// Each new expense gets its own id.
        ids: std::cell::Cell<u8>,
        /// The page is hidden (another app on screen).
        hidden: bool,
        /// Revision 18: the page was told the first screen is drawn; what it was
        /// told to remember.
        readies: usize,
        way: Option<String>,
        cloudflare: Option<Option<String>>,
    }

    impl Platform for Fake {
        fn page_ready(&mut self) {
            self.readies += 1;
        }
        fn remember_way(&mut self, way: &str) {
            self.way = Some(way.to_owned());
        }
        fn remember_cloudflare(&mut self, url: Option<&str>) {
            self.cloudflare = Some(url.map(str::to_owned));
        }
        fn now_ms(&self) -> i64 {
            self.now
        }
        fn today(&self) -> String {
            "2026-10-23".into()
        }
        fn online(&self) -> bool {
            self.online
        }
        fn default_device_name(&self) -> String {
            "Test phone".into()
        }
        fn random16(&self) -> [u8; 16] {
            let n = self.ids.get().wrapping_add(1);
            self.ids.set(n);
            [n; 16]
        }
        fn send(&mut self, req: Request) -> u64 {
            self.next += 1;
            self.sent.push((self.next, req));
            self.next
        }
        fn poll(&mut self) -> Vec<(u64, Response)> {
            std::mem::take(&mut self.answers)
        }
        fn pick_photos(&mut self, _camera: bool) {}
        fn poll_photos(&mut self) -> Vec<Result<PickedPhoto, String>> {
            Vec::new()
        }
        fn storage(&mut self) -> &mut dyn Storage {
            &mut self.store
        }
        fn loaded(&mut self) -> Option<Vec<(String, String)>> {
            self.loaded.take()
        }
        fn take_fragment(&mut self) -> Option<String> {
            self.fragment.take()
        }
        fn put_confirmed(&mut self, key: &str, value: &str) -> u64 {
            self.next += 1;
            if self.fail_writes {
                self.saved
                    .push((self.next, Err("QuotaExceededError".into())));
            } else {
                self.store.put(key, value);
                self.saved.push((self.next, Ok(())));
            }
            self.next
        }
        fn poll_saved(&mut self) -> Vec<(u64, Result<(), String>)> {
            std::mem::take(&mut self.saved)
        }
        fn request_persistence(&mut self) {
            self.persist_asked += 1;
        }
        fn persisted(&self) -> Option<bool> {
            Some(true)
        }
        fn install(&self) -> Install {
            self.install
        }
        fn paired_before(&self) -> bool {
            self.marker
        }
        fn set_paired_before(&mut self) {
            self.marker = true;
        }
        fn go_to_start(&mut self) {
            self.starts += 1;
        }
        fn visible(&self) -> bool {
            !self.hidden
        }
    }

    fn reply(app: &mut CaptureApp<Fake>, id: u64, status: u16, body: &str) {
        app.p.answers.push((
            id,
            Response {
                status,
                body: body.as_bytes().to_vec(),
            },
        ));
    }

    /// The screens drive the outbox: a queued item is sent once paired, a revoked
    /// phone keeps its outbox and says so.
    #[test]
    fn pairing_sending_and_disconnect() {
        let mut app = CaptureApp::new(Fake {
            online: true,
            loaded: Some(Vec::new()),
            ..Fake::default()
        });
        app.tick();
        assert_eq!(app.screen, Screen::Pair);
        app.pair_code = "abcd efgh".into();
        let body = serde_json::to_vec(&api::PairRequest {
            code: api::normalize_code(&app.pair_code),
            device_name: app.device_name.clone(),
            pin: None,
        })
        .unwrap();
        app.api(
            "POST",
            "/pair",
            Some(body),
            Some("application/json"),
            Pending::Pair,
        );
        let (id, req) = app.p.sent.last().cloned().unwrap();
        assert_eq!(req.path, "/api/v1/pair");
        assert!(String::from_utf8_lossy(req.body.as_deref().unwrap()).contains("ABCDEFGH"));
        reply(
            &mut app,
            id,
            201,
            &format!(r#"{{"device_id":"d","device_token":"{}"}}"#, "t".repeat(43)),
        );
        app.tick();
        assert_eq!(app.screen, Screen::Expense);
        assert_eq!(app.p.store.0.get("token").map(String::len), Some(43));
        // An expense typed without a photo goes straight to POST /expenses.
        let mut f = app.new_form();
        f.merchant = "Cafe".into();
        f.amount = "4,50".into();
        app.form = Some(f);
        app.save_form(true);
        app.tick();
        let (id, req) = app
            .p
            .sent
            .iter()
            .rev()
            .find(|(_, r)| r.path == "/api/v1/expenses" && r.method == "POST")
            .cloned()
            .unwrap();
        assert_eq!(req.bearer.as_deref().map(str::len), Some(43));
        let body: serde_json::Value = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["amount"], "4.50");
        assert_eq!(body["date"], "2026-10-23");
        // The phone was revoked meanwhile.
        reply(
            &mut app,
            id,
            401,
            r#"{"error":{"code":"not_paired","message":"x"}}"#,
        );
        app.tick();
        assert!(app.outbox.unpaired);
        assert_eq!(app.outbox.items.len(), 1, "the outbox stays on the phone");
        assert_eq!(
            app.banner.as_ref().map(|b| b.1.as_str()),
            Some(msg::DISCONNECTED)
        );
        // not_paired is the one answer that ends the pairing (revision 3).
        assert!(app.token.is_none());
        assert!(!app.p.store.0.contains_key("token"));
        assert_eq!(app.screen, Screen::Pair);
        assert_eq!(app.pair_reason, Some(msg::WAS_DISCONNECTED));
        // Validation before queueing.
        let mut f = app.new_form();
        f.amount = "1.005".into();
        f.merchant = "X".into();
        app.form = Some(f);
        app.save_form(true);
        assert!(
            app.form
                .as_ref()
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .contains("2 decimal")
        );
    }

    /// mobile-capture.md AT26 (phone part): the QR code's `#pair=` fragment fills in
    /// the code and pairs; the fragment is in no request.
    #[test]
    fn at26_pair_code_from_fragment() {
        let url = api::pair_url("capture.example.com", "abcd2345");
        let fragment = format!("#{}", url.split_once('#').unwrap().1);
        let mut app = CaptureApp::new(Fake {
            online: true,
            loaded: Some(Vec::new()),
            fragment: Some(fragment),
            ..Fake::default()
        });
        app.tick();
        assert_eq!(app.pair_code, "ABCD2345");
        assert_eq!(app.screen, Screen::Pair);
        // Revision 10: first whether a PIN is asked; without one it pairs at once.
        let (info, req) = app.p.sent.last().cloned().unwrap();
        assert_eq!(req.path, "/api/v1/pair-info");
        reply(
            &mut app,
            info,
            200,
            r#"{"pin_required": false, "way": "cloudflare"}"#,
        );
        app.tick();
        let (_, req) = app.p.sent.last().cloned().unwrap();
        assert_eq!((req.method, req.path.as_str()), ("POST", "/api/v1/pair"));
        let body: serde_json::Value = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["code"], "ABCD2345");
        assert_eq!(body["device_name"], "Test phone");
        for (_, r) in &app.p.sent {
            assert!(
                !r.path.contains('#') && !r.path.contains("pair="),
                "{}",
                r.path
            );
        }
        // A fragment that isn't a code is ignored.
        let mut app = CaptureApp::new(Fake {
            online: true,
            loaded: Some(Vec::new()),
            fragment: Some("#pair=nope".into()),
            ..Fake::default()
        });
        app.tick();
        assert!(app.pair_code.is_empty());
        assert!(!app.p.sent.iter().any(|(_, r)| r.path == "/api/v1/pair"));
    }

    /// mobile-capture.md test 73 (logic): the Wi-Fi Pair screen asks for the PIN
    /// before pairing; a wrong PIN is explained; the outbox retries on open, on
    /// `online`, on coming back to the screen and every 60 s; a Wi-Fi phone's
    /// waiting text.
    #[test]
    fn at73_pin_pairing_and_retries() {
        let mut app = CaptureApp::new(Fake {
            online: true,
            loaded: Some(Vec::new()),
            fragment: Some("#pair=ABCD2345".into()),
            ..Fake::default()
        });
        app.tick();
        let (info, _) = app.p.sent.last().cloned().unwrap();
        reply(
            &mut app,
            info,
            200,
            r#"{"pin_required": true, "way": "wifi"}"#,
        );
        app.tick();
        assert!(app.pin_required && app.wifi);
        assert!(
            !app.p.sent.iter().any(|(_, r)| r.path == "/api/v1/pair"),
            "waits for the PIN"
        );
        // Digits only, at most 12.
        for k in ["2", "4", "x", "6", "8", "1", "3"] {
            app.key_typed(k);
        }
        assert_eq!(app.pin, "246813");
        app.key_typed("backspace");
        app.key_typed("3");
        app.send_pair();
        let (id, req) = app.p.sent.last().cloned().unwrap();
        let body: serde_json::Value = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["pin"], "246813");
        reply(
            &mut app,
            id,
            401,
            r#"{"error": {"code": "pin_wrong", "message": "x"}}"#,
        );
        app.tick();
        assert_eq!(
            app.pair_error.as_deref(),
            Some("That PIN is not right. Check it in MoneyKeep on your computer.")
        );
        assert!(app.pin.is_empty(), "typed again");
        // Pairing succeeds: the way in is kept.
        app.pin = "246813".into();
        app.send_pair();
        let (id, _) = app.p.sent.last().cloned().unwrap();
        reply(
            &mut app,
            id,
            201,
            &serde_json::json!({"device_id": "d1", "device_token": "t".repeat(43)}).to_string(),
        );
        app.tick();
        app.tick();
        assert_eq!(app.screen, Screen::Expense);
        assert_eq!(app.p.store.0.get("way").map(String::as_str), Some("wifi"));

        // Retries: a queued expense that cannot reach the computer.
        let mut app = CaptureApp::new(Fake {
            online: true,
            loaded: Some(vec![
                ("token".to_owned(), "t".repeat(43)),
                ("way".to_owned(), "wifi".to_owned()),
            ]),
            ..Fake::default()
        });
        app.tick();
        assert!(app.wifi);
        let sends = |app: &CaptureApp<Fake>| {
            app.p
                .sent
                .iter()
                .filter(|(_, r)| r.path == "/api/v1/expenses" && r.method == "POST")
                .count()
        };
        let mut f = app.fresh_form();
        f.amount = "12.50".into();
        f.merchant = "Hardware store".into();
        app.form = Some(f);
        app.save_form(true);
        app.tick();
        assert_eq!(sends(&app), 1, "sent at once");
        // No answer from the computer (another network).
        let fail = |app: &mut CaptureApp<Fake>| {
            let (id, _) = app
                .p
                .sent
                .iter()
                .rev()
                .find(|(_, r)| r.path == "/api/v1/expenses")
                .cloned()
                .unwrap();
            reply(app, id, 0, "");
            app.tick();
        };
        fail(&mut app);
        assert_eq!(app.status(), ("yellow", msg::WIFI_UNREACHABLE));
        // Coming back to the screen sends again at once.
        app.p.hidden = true;
        app.tick();
        assert_eq!(sends(&app), 1);
        app.p.hidden = false;
        app.tick();
        assert_eq!(sends(&app), 2, "on visibility");
        fail(&mut app);
        // The phone reports it is online again.
        app.p.online = false;
        app.tick();
        app.p.online = true;
        app.tick();
        assert_eq!(sends(&app), 3, "on online");
        fail(&mut app);
        // Every 60 seconds while open.
        app.p.now += 60_000;
        app.tick();
        assert_eq!(sends(&app), 4, "every 60 s");
        // The Outbox explains when it will send.
        let mut h = Phone::new(app, 390.0, 844.0);
        h.run_steps(2);
        h.view().with_app(|a| a.go_tab(Screen::Outbox));
        h.run_steps(3);
        assert!(
            h.query_by_label("Will send when you are on the same Wi-Fi as your computer")
                .is_some()
        );
    }

    /// A phone on the Wi-Fi Pair screen, the PIN asked.
    fn pin_screen() -> CaptureApp<Fake> {
        let mut app = CaptureApp::new(Fake {
            online: true,
            loaded: Some(Vec::new()),
            fragment: Some("#pair=ABCD2345".into()),
            ..Fake::default()
        });
        app.tick();
        let (info, _) = app.p.sent.last().cloned().unwrap();
        reply(
            &mut app,
            info,
            200,
            r#"{"pin_required": true, "way": "wifi"}"#,
        );
        app.tick();
        app
    }

    /// Revision 18 (tests 99, 101 and 105, the app's side): nothing is drawn while
    /// the storage is read (no loading text of its own); the page is told once when the first
    /// screen is drawn; a Wi-Fi phone keeps its way and the computer's Cloudflare
    /// address for the loading screen; no answer at all on Wi-Fi gives the Wi-Fi
    /// words, an error answer or a Cloudflare phone revision 14's.
    #[test]
    fn r18_first_screen_and_wifi_words() {
        let app = CaptureApp::new(Fake {
            online: true,
            ..Fake::default()
        });
        let mut h = Phone::new(app, 390.0, 844.0);
        h.run_steps(3);
        assert_eq!(h.view().with_app(|a| a.p.readies), 0);
        assert!(h.query_all_by_label_contains("Loading").next().is_none());
        h.view().with_app(|a| a.p.loaded = Some(Vec::new()));
        h.run_steps(3);
        assert!(h.query_by_label("Pair").is_some(), "the first real screen");
        assert_eq!(h.view().with_app(|a| a.p.readies), 1);
        h.run_steps(3);
        assert_eq!(h.view().with_app(|a| a.p.readies), 1, "once");

        // pair-info on the Wi-Fi address names the Cloudflare address.
        let mut app = CaptureApp::new(Fake {
            online: true,
            loaded: Some(Vec::new()),
            fragment: Some("#pair=ABCD2345".into()),
            ..Fake::default()
        });
        app.tick();
        let (info, _) = app.p.sent.last().cloned().unwrap();
        reply(
            &mut app,
            info,
            200,
            r#"{"pin_required": true, "way": "wifi", "cloudflare_url": "https://capture.example.com"}"#,
        );
        app.tick();
        assert_eq!(
            app.cloudflare_url.as_deref(),
            Some("https://capture.example.com")
        );
        app.saved("t".repeat(43), Ok(()));
        assert_eq!(app.p.way.as_deref(), Some("wifi"));
        assert_eq!(
            app.p.cloudflare,
            Some(Some("https://capture.example.com".to_owned()))
        );
        // An address that is not https is never kept.
        let mut app2 = CaptureApp::new(Fake {
            online: true,
            loaded: Some(Vec::new()),
            fragment: Some("#pair=ABCD2345".into()),
            ..Fake::default()
        });
        app2.tick();
        let (info, _) = app2.p.sent.last().cloned().unwrap();
        reply(
            &mut app2,
            info,
            200,
            r#"{"pin_required": true, "way": "wifi", "cloudflare_url": "javascript:alert(1)"}"#,
        );
        app2.tick();
        assert_eq!(app2.cloudflare_url, None);
        // A phone paired before tells the page its way when the app opens.
        let mut app3 = CaptureApp::new(Fake {
            online: true,
            loaded: Some(vec![
                ("token".to_owned(), "t".repeat(43)),
                ("way".to_owned(), "wifi".to_owned()),
            ]),
            ..Fake::default()
        });
        app3.tick();
        assert_eq!(app3.p.way.as_deref(), Some("wifi"));
        // The words.
        app3.unreachable = true;
        app3.no_answer = true;
        assert_eq!(app3.status(), ("yellow", msg::WIFI_UNREACHABLE));
        assert_eq!(
            app3.not_reachable_text(),
            (msg::WIFI_UNREACHABLE, msg::WIFI_UNREACHABLE_DETAIL)
        );
        app3.no_answer = false;
        assert_eq!(app3.status(), ("yellow", msg::NOT_REACHABLE));
        app3.no_answer = true;
        app3.wifi = false;
        assert_eq!(app3.status(), ("yellow", msg::NOT_REACHABLE));
    }

    /// Test 73 (screen): the PIN is masked, the eye shows and hides it, the keys are
    /// digits, and the wrong-PIN message shows — in the real phone window.
    #[test]
    fn at73_pin_screen() {
        let mut h = Phone::new(pin_screen(), 390.0, 844.0);
        h.run_steps(3);
        assert!(h.query_by_label("Capture PIN, 0 digits entered").is_some());
        for d in ["2", "4", "6", "8", "1", "3"] {
            h.get_by_label(d).click();
            h.run_steps(1);
        }
        // Masked: the accessible name never carries the digits.
        assert!(h.query_by_label("Capture PIN, 6 digits entered").is_some());
        assert!(h.query_all_by_label_contains("246813").next().is_none());
        assert!(h.view().with_app(|a| a.pin == "246813" && !a.pin_shown));
        h.get_by_label("Show PIN").click();
        h.run_steps(2);
        assert!(
            h.query_by_label("Capture PIN, 6 digits entered, shown")
                .is_some()
        );
        h.get_by_label("Hide PIN").click();
        h.run_steps(2);
        assert!(h.query_by_label("Capture PIN, 6 digits entered").is_some());
        h.get_by_label("Delete digit").click();
        h.run_steps(1);
        assert!(h.query_by_label("Capture PIN, 5 digits entered").is_some());
        for k in ["0", "1", "9", "Clear PIN", "Name for this phone"] {
            assert!(h.query_by_label(k).is_some(), "{k}");
        }
        // Test 87: the label and the help line saying which PIN it is.
        assert!(h.query_by_label("Capture PIN").is_some());
        assert!(h.query_by_label(msg::PIN_HELP).is_some());
        assert_eq!(
            msg::PIN_HELP,
            "The 6 to 12 digits you chose when you set up phone capture in MoneyKeep. Forgot it? On the computer: Settings → Phone capture → Change PIN…"
        );
        let top = |l: &str| h.query_by_label(l).map(|n| n.rect().min.y);
        assert!(top("Capture PIN") < top(msg::PIN_HELP));
        assert!(top(msg::PIN_HELP) < top("Capture PIN, 5 digits entered"));
        // The wrong-PIN message.
        h.view().with_app(|a| {
            a.pin = "111111".into();
            a.send_pair();
            let (id, _) = a.p.sent.last().cloned().unwrap();
            reply(
                a,
                id,
                401,
                r#"{"error": {"code": "pin_wrong", "message": "x"}}"#,
            );
        });
        h.run_steps(3);
        assert!(
            h.query_by_label("That PIN is not right. Check it in MoneyKeep on your computer.")
                .is_some()
        );
    }

    /// mobile-capture.md revision 3, the cause of "asks to pair again": a paired
    /// phone opening the page again from an address that still carries the used
    /// `#pair=` code (Chrome's history or address-bar suggestion of the QR address,
    /// a home-screen shortcut made from it) must stay paired; it must not drop the
    /// stored token and try the old code.
    #[test]
    fn reopening_the_qr_address_keeps_the_pairing() {
        let token = "t".repeat(43);
        let mut app = CaptureApp::new(Fake {
            online: true,
            loaded: Some(vec![("token".to_owned(), token.clone())]),
            fragment: Some("#pair=ABCD2345".into()),
            ..Fake::default()
        });
        app.tick();
        assert_eq!(app.screen, Screen::Expense, "still paired");
        assert_eq!(app.token.as_deref(), Some(token.as_str()));
        assert!(
            !app.p.sent.iter().any(|(_, r)| r.path == "/api/v1/pair"),
            "the used code is not tried again"
        );
    }

    fn paired_app(online: bool) -> CaptureApp<Fake> {
        let mut app = CaptureApp::new(Fake {
            online,
            loaded: Some(vec![("token".to_owned(), "t".repeat(43))]),
            ..Fake::default()
        });
        app.tick();
        assert_eq!(app.screen, Screen::Expense);
        app
    }

    fn lists_request(app: &CaptureApp<Fake>) -> u64 {
        app.p
            .sent
            .iter()
            .rev()
            .find(|(_, r)| r.path.starts_with("/api/v1/lists"))
            .map(|(id, _)| *id)
            .unwrap()
    }

    /// mobile-capture.md AT33: Access's sign-in (a 302 or an HTML page) keeps the
    /// token and asks to sign in again; `not_paired` clears it; a network failure
    /// keeps it.
    #[test]
    fn at33_only_not_paired_ends_the_pairing() {
        for (status, body) in [
            (302, ""),
            (
                200,
                "<!DOCTYPE html><html><title>Sign in ・ Cloudflare Access</title>",
            ),
        ] {
            let mut app = paired_app(true);
            let id = lists_request(&app);
            reply(&mut app, id, status, body);
            app.tick();
            assert!(app.token.is_some(), "{status}: token kept");
            assert!(app.signin, "{status}: sign in again");
            assert_eq!(app.screen, Screen::Expense);
            assert!(
                app.lists.book_name.is_empty(),
                "the HTML is not taken as lists"
            );
        }
        // The outbox treats the sign-in page as "try again", never as unpaired.
        let r = Response {
            status: 302,
            body: Vec::new(),
        };
        assert_eq!(
            classify(&r),
            Err(Outcome::Retry {
                message: msg::SIGN_IN_AGAIN.into()
            })
        );
        // A network failure keeps the token.
        let mut app = paired_app(true);
        let id = lists_request(&app);
        reply(&mut app, id, 0, "");
        app.tick();
        assert!(app.token.is_some());
        assert!(!app.signin);
        // not_paired clears it and says why.
        let mut app = paired_app(true);
        let id = lists_request(&app);
        reply(
            &mut app,
            id,
            401,
            r#"{"error":{"code":"not_paired","message":"x"}}"#,
        );
        app.tick();
        assert!(app.token.is_none());
        assert_eq!(app.screen, Screen::Pair);
        assert_eq!(app.pair_reason, Some(msg::WAS_DISCONNECTED));
        // ... and remembers it across a restart.
        let entries: Vec<(String, String)> = app.p.store.0.clone().into_iter().collect();
        let mut again = CaptureApp::new(Fake {
            online: true,
            loaded: Some(entries),
            ..Fake::default()
        });
        again.tick();
        assert_eq!(again.screen, Screen::Pair);
        assert_eq!(again.pair_reason, Some(msg::WAS_DISCONNECTED));
    }

    /// mobile-capture.md revision 6, test 48: pressing **Sign in** (in the real phone
    /// window) goes to the start page through the platform, not a reload, and keeps
    /// the token.
    #[test]
    fn at48_sign_in_goes_to_start_and_keeps_the_token() {
        let mut app = paired_app(true);
        let id = lists_request(&app);
        reply(&mut app, id, 302, "");
        app.tick();
        assert!(app.signin);
        assert_eq!(app.p.starts, 0);
        let mut h = Phone::new(app, 400.0, 800.0);
        h.run_steps(3);
        assert!(h.query_by_label(msg::SIGN_IN_AGAIN).is_some());
        h.get_by_label("Sign in").click();
        h.run_steps(3);
        h.view().with_app(|app| {
            assert_eq!(app.p.starts, 1, "navigate to start called once");
            assert!(app.token.is_some(), "token kept");
        });
    }

    /// mobile-capture.md revision 8, test 57: `access_refused` while pairing shows
    /// the server's message and no **Sign in** button, and does not set the sign-in
    /// state; a 302 still shows **Sign in** (AT48 above). The outbox and a paired
    /// phone's refresh say the same and keep the pairing.
    #[test]
    fn at57_refused_sign_in_is_explained_without_sign_in() {
        let refused = format!(
            r#"{{"error":{{"code":"access_refused","message":"{}"}}}}"#,
            msg::ACCESS_REFUSED
        );
        let mut app = CaptureApp::new(Fake {
            online: true,
            loaded: Some(Vec::new()),
            ..Fake::default()
        });
        app.tick();
        assert_eq!(app.screen, Screen::Pair);
        app.pair_code = "ABCD2345".into();
        app.device_name = "Phone".into();
        app.send_pair();
        let (id, req) = app.p.sent.last().cloned().unwrap();
        assert_eq!(req.path, "/api/v1/pair");
        reply(&mut app, id, 401, &refused);
        app.tick();
        assert!(!app.signin, "no sign-in state");
        assert_eq!(app.pair_error.as_deref(), Some(msg::ACCESS_REFUSED));
        let mut h = Phone::new(app, 390.0, 844.0);
        h.run_steps(3);
        assert!(h.query_by_label(msg::ACCESS_REFUSED).is_some());
        assert!(h.query_by_label("Sign in").is_none(), "no Sign in button");
        assert!(h.query_by_label(msg::SIGN_IN_AGAIN).is_none());

        // The outbox and the paired phone: the same words, the token kept.
        for code in ["access_refused", "access_missing", "not_signed_in"] {
            let body = format!(r#"{{"error":{{"code":"{code}","message":"x"}}}}"#);
            let want = if code == "not_signed_in" {
                msg::ACCESS_REFUSED
            } else {
                "x"
            };
            assert_eq!(
                classify(&Response {
                    status: 401,
                    body: body.clone().into_bytes(),
                }),
                Err(Outcome::Retry {
                    message: want.into()
                }),
                "{code}"
            );
            let mut app = paired_app(true);
            let id = lists_request(&app);
            reply(&mut app, id, 401, &body);
            app.tick();
            assert!(app.token.is_some(), "{code}: pairing kept");
            assert!(!app.signin, "{code}: no Sign in");
            assert_eq!(app.banner, Some((true, want.to_owned())), "{code}");
        }
        // Any other 401 no longer says "reload the page".
        assert_eq!(
            classify(&Response {
                status: 401,
                body: Vec::new(),
            }),
            Err(Outcome::Retry {
                message: msg::ACCESS_REFUSED.into()
            })
        );
    }

    fn pair_reply(app: &mut CaptureApp<Fake>) {
        let code = api::pair_url("capture.example.com", "ABCD2345");
        app.p.fragment = Some(format!("#{}", code.split_once('#').unwrap().1));
        app.tick();
        let (info, req) = app.p.sent.last().cloned().unwrap();
        assert_eq!(req.path, "/api/v1/pair-info");
        reply(
            app,
            info,
            200,
            r#"{"pin_required": false, "way": "cloudflare"}"#,
        );
        app.tick();
        let (id, req) = app.p.sent.last().cloned().unwrap();
        assert_eq!(req.path, "/api/v1/pair");
        reply(
            app,
            id,
            201,
            &format!(r#"{{"device_id":"d","device_token":"{}"}}"#, "k".repeat(43)),
        );
    }

    /// mobile-capture.md AT34: pairing waits for the storage write; a failed write
    /// shows the private-browsing message; the token survives a restart.
    #[test]
    fn at34_pairing_is_stored_before_home_and_survives_a_restart() {
        // A failed write.
        let mut app = CaptureApp::new(Fake {
            online: true,
            loaded: Some(Vec::new()),
            fail_writes: true,
            ..Fake::default()
        });
        pair_reply(&mut app);
        app.tick();
        assert_eq!(app.screen, Screen::Pair);
        assert_eq!(app.pair_error.as_deref(), Some(msg::PAIRING_NOT_SAVED));
        assert!(app.token.is_none());
        // A write that completes: Home only after it, persistence asked, marker set.
        let mut app = CaptureApp::new(Fake {
            online: true,
            loaded: Some(Vec::new()),
            ..Fake::default()
        });
        let code = api::pair_url("capture.example.com", "ABCD2345");
        app.p.fragment = Some(format!("#{}", code.split_once('#').unwrap().1));
        app.tick();
        let (info, _) = app.p.sent.last().cloned().unwrap();
        reply(&mut app, info, 404, "{}");
        app.tick();
        let (id, req) = app.p.sent.last().cloned().unwrap();
        assert_eq!(req.path, "/api/v1/pair", "an older server: pairs as before");
        reply(
            &mut app,
            id,
            201,
            &format!(r#"{{"device_id":"d","device_token":"{}"}}"#, "k".repeat(43)),
        );
        // The answer arrives, the write is started; it completes in a later poll.
        let answers = std::mem::take(&mut app.p.answers);
        for (id, r) in answers {
            let pos = app.pending.iter().position(|(p, _)| *p == id).unwrap();
            let (_, what) = app.pending.remove(pos);
            app.answer(what, r);
        }
        assert_eq!(app.screen, Screen::Pair, "not before the write completes");
        assert!(app.saving.is_some());
        app.tick();
        assert_eq!(app.screen, Screen::Expense);
        assert!(app.p.marker);
        assert!(app.p.persist_asked >= 1);
        // A restart with the stored entries: still paired, persistence asked again.
        let entries: Vec<(String, String)> = app.p.store.0.clone().into_iter().collect();
        let mut again = CaptureApp::new(Fake {
            online: true,
            loaded: Some(entries),
            ..Fake::default()
        });
        again.tick();
        assert_eq!(again.screen, Screen::Expense);
        assert_eq!(again.token.as_deref(), Some("k".repeat(43).as_str()));
        assert_eq!(again.p.persist_asked, 1);
        // Storage cleared by the browser: the Pair screen says so.
        let mut cleared = CaptureApp::new(Fake {
            online: true,
            loaded: Some(Vec::new()),
            marker: true,
            ..Fake::default()
        });
        cleared.tick();
        assert_eq!(cleared.pair_reason, Some(msg::STORAGE_CLEARED));
    }

    type Phone = mk_views::testing::PhoneHarness<CaptureApp<Fake>>;

    /// mobile-capture.md AT40, as revision 7 changes it: the menu button ("Menu")
    /// is in the top bar, after the title and the status light (revision 4 had it
    /// first); a 48 px tap target; its menu opens below it, inside the window.
    /// Driven in the real phone window (`PhoneWindow`) on Slint's testing backend.
    #[test]
    fn at40_menu_button_in_the_top_bar() {
        let mut h = Phone::new(paired_app(true), 400.0, 800.0);
        h.run_steps(3);
        let menu = h.get_by_label("Menu").rect();
        // Revision 9: the app opens on the Expense tab, whose title is "Expense".
        let title = h
            .query_all_by_label("Expense")
            .find(|n| n.aid().starts_with("heading|"))
            .map(|n| n.rect())
            .unwrap();
        let light = h.get_by_label("Status: connected").rect();
        assert!(
            title.max.x <= light.min.x,
            "title {title:?} before the light"
        );
        assert!(light.max.x <= menu.min.x, "light {light:?} before the menu");
        for r in [title, light] {
            assert!((menu.center().y - r.center().y).abs() < 20.0, "same row");
        }
        assert!(menu.width() >= 48.0 && menu.height() >= 48.0, "{menu:?}");
        assert!(menu.max.y <= 56.0, "in the top bar");
        // The tabs have no back arrow.
        assert!(h.query_by_label("Back").is_none());
        h.get_by_label("Menu").click();
        h.run_steps(3);
        let sign_out = h.get_by_label("Sign out").rect();
        assert!(sign_out.min.y >= menu.max.y - 1.0, "below the button");
        assert!(sign_out.max.x <= 400.0, "inside the window");
        assert!(sign_out.height() >= 48.0, "{sign_out:?}");
        // About: the phone's storage and the toolkit's notice (AboutSlint).
        h.get_by_label("About this phone").click();
        h.run_steps(3);
        assert!(h.query_by_label(msg::STORAGE_KEPT).is_some());
        assert!(h.query_by_label("About Slint").is_some());
    }

    /// The paired app with the form open on a new expense.
    fn form_app() -> Phone {
        let mut h = Phone::new(paired_app(true), 390.0, 844.0);
        h.run_steps(3);
        h
    }

    fn date_of(h: &Phone) -> String {
        h.view()
            .with_app(|a| a.form.as_ref().map(|f| f.date.clone()).unwrap_or_default())
    }

    fn amount_of(h: &Phone) -> String {
        h.view().with_app(|a| {
            a.form
                .as_ref()
                .map(|f| f.amount.clone())
                .unwrap_or_default()
        })
    }

    /// mobile-capture.md revision 7, test 49: a new expense's date is today; the
    /// calendar opens on today's month; a day sets the date and closes; Today
    /// selects today; previous and next change the month; days beyond 400 days are
    /// disabled. Day names are accessible names.
    #[test]
    fn at49_calendar() {
        let mut h = form_app();
        assert_eq!(date_of(&h), "2026-10-23");
        assert!(h.query_by_label("Date, Fri, Oct 23, 2026").is_some());
        h.get_by_label("Date, Fri, Oct 23, 2026").click();
        h.run_steps(3);
        assert!(h.query_by_label("October 2026").is_some(), "today's month");
        let today = h.get_by_label("Friday, October 23, 2026, today, selected");
        assert!(today.selected());
        assert!(h.query_by_label("Sunday").is_some(), "weekday row");
        // Sunday first: the 1st of October 2026 is a Thursday, in the 5th column.
        let sun = h.get_by_label("Sunday, September 27, 2026").rect();
        let thu = h.get_by_label("Thursday, October 1, 2026").rect();
        assert!((sun.top() - thu.top()).abs() < 1.0 && thu.left() > sun.left());
        // A day sets the date and closes.
        h.get_by_label("Tuesday, October 6, 2026").click();
        h.run_steps(3);
        assert_eq!(date_of(&h), "2026-10-06");
        assert!(h.query_by_label("October 2026").is_none(), "closed");
        // Previous and next change the month; Today selects today.
        h.get_by_label("Date, Tue, Oct 6, 2026").click();
        h.run_steps(3);
        h.get_by_label("Next month").click();
        h.run_steps(2);
        assert!(h.query_by_label("November 2026").is_some());
        h.get_by_label("Previous month").click();
        h.get_by_label("Previous month").click();
        h.run_steps(2);
        assert!(h.query_by_label("September 2026").is_some());
        h.get_by_label("Today").click();
        h.run_steps(3);
        assert_eq!(date_of(&h), "2026-10-23");
        // 400 days: 2027-11-27 is the last day that may be chosen.
        h.get_by_label("Date, Fri, Oct 23, 2026").click();
        h.run_steps(2);
        for _ in 0..13 {
            h.get_by_label("Next month").click();
            h.run_steps(1);
        }
        assert!(h.query_by_label("November 2027").is_some());
        assert!(h.get_by_label("Saturday, November 27, 2027").enabled());
        assert!(!h.get_by_label("Sunday, November 28, 2027").enabled());
        h.get_by_label("Sunday, November 28, 2027").click();
        h.run_steps(2);
        assert_eq!(date_of(&h), "2026-10-23", "a disabled day does nothing");
        assert!(!h.get_by_label("Next month").enabled(), "no month beyond");
    }

    /// Test 50: editing a waiting expense keeps its stored date.
    #[test]
    fn at50_editing_keeps_the_date() {
        let mut app = paired_app(false);
        let mut f = app.new_form();
        f.date = "2026-09-14".into();
        f.merchant = "Cafe".into();
        app.form = Some(f);
        app.save_form(false);
        let mut h = Phone::new(app, 390.0, 844.0);
        h.run_steps(2);
        h.get_by_label("Outbox, 1 waiting").click();
        h.run_steps(2);
        h.get_by_label("Edit").click();
        h.run_steps(3);
        assert_eq!(date_of(&h), "2026-09-14");
        assert!(h.query_by_label("Date, Mon, Sep 14, 2026").is_some());
        h.get_by_label("Date, Mon, Sep 14, 2026").click();
        h.run_steps(2);
        assert!(
            h.query_by_label("September 2026").is_some(),
            "its own month"
        );
    }

    fn tap(h: &mut Phone, keys: &str) {
        for k in keys.split_whitespace() {
            let name = match k {
                "." => "Decimal point",
                "+" => "Plus",
                "−" => "Minus",
                "×" => "Multiply",
                "÷" => "Divide",
                "⌫" => "Delete",
                "=" => "Equals",
                k => k,
            };
            h.get_by_label(name).click();
            h.run_steps(1);
        }
    }

    /// Test 51: the keypad: 12.50 + 4.25 Done sets 16.75; 10 ÷ 3 Done sets 3.33;
    /// 2 + 3 × 4 = shows 14; a third decimal digit is ignored; ⌫ and Clear work;
    /// +20 adds 20. Keys from a computer's keyboard work too.
    #[test]
    fn at51_amount_keypad() {
        let mut h = form_app();
        let card = h.get_by_label("Amount, 0.00");
        assert!(card.rect().height() >= 48.0);
        card.click();
        h.run_steps(3);
        tap(&mut h, "1 2 . 5 0 + 4 . 2 5");
        assert!(h.query_by_label("12.50 + 4.25").is_some(), "the expression");
        assert!(h.query_by_label("= 16.75").is_some(), "its result");
        tap(&mut h, "Done");
        h.run_steps(2);
        assert_eq!(amount_of(&h), "16.75");
        assert!(h.query_by_label("Done").is_none(), "closed");
        assert!(h.query_by_label("Amount, 16.75").is_some());
        // Reopened, it starts from the amount; Clear sets 0.
        h.get_by_label("Amount, 16.75").click();
        h.run_steps(3);
        assert!(h.query_by_label("16.75").is_some());
        tap(&mut h, "Clear 1 0 ÷ 3 Done");
        assert_eq!(amount_of(&h), "3.33");
        h.get_by_label("Amount, 3.33").click();
        h.run_steps(3);
        tap(&mut h, "Clear 2 + 3 × 4 =");
        assert!(h.query_by_label("14").is_some(), "= shows 14");
        tap(&mut h, ". 5 6 7");
        assert!(
            h.query_by_label("14.56").is_some(),
            "a third decimal is ignored"
        );
        tap(&mut h, "⌫ ⌫");
        assert!(h.query_by_label("14.").is_some());
        tap(&mut h, "Clear");
        assert!(h.query_by_label("= 0.00").is_some());
        h.get_by_label("Add 20").click();
        h.run_steps(1);
        assert!(h.query_by_label("20").is_some(), "+20 adds 20");
        h.get_by_label("Add 5").click();
        h.run_steps(1);
        tap(&mut h, "Done");
        assert_eq!(amount_of(&h), "25.00");
        // A computer's keyboard: digits, operators, Backspace, Enter (= Done).
        h.get_by_label("Amount, 25.00").click();
        h.run_steps(3);
        tap(&mut h, "Clear");
        for t in ["7", "*", "3", "+", "1", "x"] {
            h.type_key(t);
        }
        assert!(h.query_by_label("7 × 3 + 1").is_some(), "typed keys");
        h.type_key("\u{8}");
        assert!(h.query_by_label("7 × 3 +").is_some(), "Backspace");
        h.type_key("2");
        h.type_key("\n");
        h.run_steps(2);
        assert_eq!(amount_of(&h), "23.00", "Enter is Done");
    }

    /// Test 52: Done with 0, a negative result or a division by zero says "Amount
    /// must be more than 0" and keeps the sheet open; the dimmed area and Escape
    /// close it with the old amount.
    #[test]
    fn at52_keypad_refusals_and_closing() {
        let mut h = form_app();
        h.get_by_label("Amount, 0.00").click();
        h.run_steps(3);
        for keys in ["Done", "Clear 2 − 5 Done", "Clear 5 ÷ 0 Done"] {
            tap(&mut h, keys);
            assert!(
                h.query_by_label(keypad::MORE_THAN_ZERO).is_some(),
                "{keys}: refused"
            );
            assert!(h.query_by_label("Done").is_some(), "{keys}: still open");
            assert_eq!(amount_of(&h), "");
        }
        tap(&mut h, "Clear 9 Done");
        assert_eq!(amount_of(&h), "9.00");
        // The dimmed area closes the sheet, keeping the old amount.
        h.get_by_label("Amount, 9.00").click();
        h.run_steps(3);
        tap(&mut h, "Clear 4 4");
        h.click_at(195.0, 60.0);
        h.run_steps(2);
        assert!(h.query_by_label("Done").is_none(), "closed");
        assert_eq!(amount_of(&h), "9.00");
        // Escape too.
        h.get_by_label("Amount, 9.00").click();
        h.run_steps(3);
        tap(&mut h, "Clear 1");
        h.type_key("\u{1b}");
        h.run_steps(2);
        assert!(h.query_by_label("Done").is_none(), "closed");
        assert_eq!(amount_of(&h), "9.00");
    }

    /// Every tap target on screen (buttons, keys, days, fields, pickers, tiles).
    fn targets(h: &Phone) -> Vec<mk_views::testing::PhoneNode> {
        h.nodes()
            .into_iter()
            .filter(|n| {
                let aid = n.aid();
                [
                    "button|",
                    "iconbutton|",
                    "tile|",
                    "combo|",
                    "input|",
                    "menu|",
                    "key|",
                    "day|",
                    "thumb|",
                    "multiline|",
                    "tab|",
                ]
                .iter()
                .any(|p| aid.starts_with(p))
            })
            .collect()
    }

    fn check_targets(h: &Phone, what: &str, width: f32) -> usize {
        let t = targets(h);
        assert!(!t.is_empty(), "{what}");
        for n in &t {
            // 44 / 48 px, less half a pixel of rounding.
            let least = if n.aid().starts_with("day|") {
                43.5
            } else {
                47.5
            };
            assert!(
                n.rect().height() >= least,
                "{what} at {width}: {n:?} ({}) is {} tall",
                n.aid(),
                n.rect().height()
            );
        }
        t.len()
    }

    /// Test 53: every tap target on the form (the Expense tab, with its tab bar),
    /// the keypad and the calendar is at least 48 px tall (calendar days at least
    /// 44 px), at 360 and 430 px wide; revision 9 has no separate home screen.
    #[test]
    fn at53_tap_targets() {
        for width in [360.0, 430.0] {
            let mut app = paired_app(true);
            app.lists = serde_json::from_str(
                r#"{"categories":["Expenses:Office"],"paid_with":["Visa"],"merchants":["Office Depot"]}"#,
            )
            .unwrap();
            let mut h = Phone::new(app, width, 844.0);
            h.run_steps(3);
            h.view().with_app(|a| {
                if let Some(f) = a.form.as_mut() {
                    f.photos.push(Photo {
                        jpeg: vec![1],
                        width: 2,
                        height: 2,
                        thumb: vec![200; 16],
                        thumb_w: 2,
                        thumb_h: 2,
                        receipt_id: None,
                    });
                }
            });
            h.run_steps(2);
            let n = check_targets(&h, "form", width);
            assert!(n >= 10, "the form's targets: {n}");
            // Nothing is wider than the screen.
            for t in targets(&h) {
                assert!(t.rect().right() <= width + 0.5, "{t:?}");
            }
            h.get_by_label("Amount, 0.00").click();
            h.run_steps(3);
            let keys = check_targets(&h, "keypad", width);
            assert!(keys >= 24, "the keypad's keys: {keys}");
            let done = h.get_by_label("Done").rect();
            let eq = h.get_by_label("Equals").rect();
            assert!(done.width() > eq.width() * 1.8, "Done spans two columns");
            assert!(eq.height() >= 56.0, "keys at least 56 px: {eq:?}");
            h.type_key("\u{1b}");
            h.run_steps(2);
            h.get_by_label("Date, Fri, Oct 23, 2026").click();
            h.run_steps(3);
            check_targets(&h, "calendar", width);
            let day = h
                .get_by_label("Friday, October 23, 2026, today, selected")
                .rect();
            assert!(day.width() >= 44.0 && day.height() >= 44.0, "{day:?}");
        }
    }

    /// Test 54 as revision 9 changes it: the action row (Send, wide, and Save for
    /// later) sits directly above the tab bar; the tabs have no back arrow (Cancel
    /// was the back arrow, which now only an opened item has, test 64); Save for
    /// later saves a draft.
    #[test]
    fn at54_action_row_above_the_tab_bar() {
        let mut h = form_app();
        let send = h.get_by_label("Send").rect();
        let later = h.get_by_label("Save for later").rect();
        let tab = h.get_by_label("Expense, selected").rect();
        assert!((send.top() - later.top()).abs() < 1.0, "side by side");
        assert!(send.width() > later.width(), "Send takes the width");
        assert!(
            send.bottom() <= tab.top(),
            "above the tabs: {send:?} {tab:?}"
        );
        assert!(
            tab.top() - send.bottom() < 24.0,
            "directly above: {send:?} {tab:?}"
        );
        assert!(h.query_by_label("Cancel").is_none());
        assert!(h.query_by_label("Back").is_none());
        h.get_by_label("Save for later").click();
        h.run_steps(3);
        h.view().with_app(|a| {
            assert_eq!(a.outbox.items.len(), 1);
            assert_eq!(a.outbox.items[0].state, State::Draft);
        });
    }

    /// The window's background colour and the top bar's and tab bar's (the
    /// surface), as drawn: light theme.
    const PAGE: u32 = 0xf4_f2_ef;
    const SURFACE: u32 = 0xfd_fc_fa;

    fn rgb(px: &mk_views::testing::Pixels, x: f32, y: f32) -> u32 {
        px.at(x, y)
    }

    /// Test 60: at 390×844 and 412×915 the top bar spans the full window width and
    /// the tab bar touches the bottom edge (no gap right or below), read from the
    /// drawn pixels; turning to 844×390 and back lays out again with no gap.
    #[test]
    fn at60_the_app_fills_the_window() {
        let check = |h: &Phone, w: f32, ht: f32| {
            let px = h.pixels();
            assert_eq!(
                (px.width, px.height),
                (w as u32, ht as u32),
                "the drawing is the window's size"
            );
            // The top bar (surface colour) reaches the right edge; the tab bar the
            // bottom edge and both corners.
            for (x, y, what) in [
                (2.0, 28.0, "top bar, left"),
                (w - 2.0, 28.0, "top bar, right edge"),
                (2.0, ht - 2.0, "tab bar, bottom left"),
                (w - 2.0, ht - 2.0, "tab bar, bottom right corner"),
                (w / 2.0, ht - 1.0, "tab bar, bottom edge"),
            ] {
                assert_eq!(
                    rgb(&px, x, y),
                    SURFACE,
                    "{what} at {w}×{ht}: {:06x}",
                    rgb(&px, x, y)
                );
            }
            // The page (not the surface) shows between the bars at the right edge.
            assert_eq!(
                rgb(&px, w - 2.0, 72.0),
                PAGE,
                "page at the right at {w}×{ht}: {:06x}",
                rgb(&px, w - 2.0, 72.0)
            );
            let tab = h.get_by_label("Expense, selected").rect();
            assert!(
                tab.bottom() > ht - 12.0,
                "tabs at the bottom: {tab:?} in {ht}"
            );
            // The tabs fill the centred column (at most 480 px).
            let sent = h.get_by_label("Sent").rect();
            let column = w.min(480.0);
            assert!(
                (tab.left() - (w - column) / 2.0).abs() < 1.0
                    && (sent.right() - (w + column) / 2.0).abs() < 1.0,
                "tabs span the column: {tab:?} {sent:?} in {w}"
            );
        };
        for (w, ht) in [(390.0, 844.0), (412.0, 915.0)] {
            let mut h = Phone::rendered(paired_app(true), w, ht);
            h.view().window().set_theme_choice(1);
            h.run_steps(3);
            check(&h, w, ht);
            h.set_size(ht, w);
            h.run_steps(2);
            check(&h, ht, w);
            h.set_size(w, ht);
            h.run_steps(2);
            check(&h, w, ht);
        }
    }

    /// Test 62: the paired app opens on the Expense tab with today's date; the tab
    /// bar has Expense (selected), Outbox (with its count) and Sent; there are no
    /// tiles.
    #[test]
    fn at62_opens_on_the_expense_tab() {
        let mut app = paired_app(false);
        app.screen = Screen::Outbox;
        let mut f = app.new_form();
        f.merchant = "Cafe".into();
        app.form = Some(f);
        app.save_form(false);
        app.screen = Screen::Loading;
        app.go_tab(Screen::Expense);
        let mut h = Phone::new(app, 390.0, 844.0);
        h.run_steps(3);
        assert!(
            h.query_by_label("Date, Fri, Oct 23, 2026").is_some(),
            "today"
        );
        assert!(h.query_by_label("Amount, 0.00").is_some());
        let tabs: Vec<_> = h
            .nodes()
            .into_iter()
            .filter(|n| n.aid().starts_with("tab|"))
            .collect();
        let names: Vec<&str> = tabs.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(names, ["Expense, selected", "Outbox, 1 waiting", "Sent"]);
        assert!(tabs[0].selected() && !tabs[1].selected() && !tabs[2].selected());
        // Three equal tabs, at least 48 px tall, in one row.
        for t in &tabs {
            assert!(t.rect().height() >= 48.0, "{t:?}");
            assert!(
                (t.rect().width() - tabs[0].rect().width()).abs() < 1.0,
                "{t:?}"
            );
            assert!((t.rect().top() - tabs[0].rect().top()).abs() < 1.0, "{t:?}");
        }
        for gone in ["Take photo", "Choose photos", "Enter expense"] {
            assert!(h.query_by_label(gone).is_none(), "{gone}");
        }
        assert!(
            !h.nodes().iter().any(|n| n.aid().starts_with("tile|")),
            "no tiles"
        );
        // The other tabs: their titles, the right tab selected, no action row.
        h.get_by_label("Outbox, 1 waiting").click();
        h.run_steps(3);
        assert!(h.query_by_label("Outbox, 1 waiting, selected").is_some());
        assert_eq!(h.query_all_by_label("Edit").count(), 1, "the outbox list");
        assert!(h.query_by_label("Send").is_none() && h.query_by_label("Back").is_none());
        h.get_by_label("Sent").click();
        h.run_steps(3);
        assert!(h.query_by_label("Sent, selected").is_some());
        assert!(h.query_by_label("Nothing sent yet.").is_some());
        assert!(h.query_by_label("Save for later").is_none());
    }

    /// Test 63: an unfinished expense survives switching tabs; Send resets the
    /// form, says "Sent" for about 3 seconds and the Outbox count changes; Clear
    /// asks and resets.
    #[test]
    fn at63_tabs_keep_the_expense_send_and_clear() {
        let mut h = Phone::new(paired_app(false), 390.0, 844.0);
        h.run_steps(3);
        h.get_by_label("Amount, 0.00").click();
        h.run_steps(3);
        tap(&mut h, "1 2 Done");
        h.get_by_label("Merchant").set_text("Corner Grocery");
        h.run_steps(2);
        h.get_by_label("Outbox").click();
        h.run_steps(3);
        assert!(
            h.query_by_label("Discard this expense?").is_none(),
            "nothing asked"
        );
        h.get_by_label("Expense").click();
        h.run_steps(3);
        assert!(
            h.query_by_label("Amount, 12.00").is_some(),
            "the amount is kept"
        );
        h.view()
            .with_app(|a| assert_eq!(a.form.as_ref().unwrap().merchant, "Corner Grocery"));
        // Send: queued (offline), a new expense, "Sent", the count.
        h.get_by_label("Send").click();
        h.run_steps(1);
        assert!(h.query_by_label("Amount, 0.00").is_some(), "a new expense");
        assert_eq!(date_of(&h), "2026-10-23");
        let notice = |h: &Phone, word: &str| {
            h.query_all_by_label(word)
                .any(|n| n.aid().starts_with("label|"))
        };
        assert!(notice(&h, "Sent"), "the confirmation");
        let tab = h.get_by_label("Outbox, 1 waiting").rect();
        let send = h.get_by_label("Send").rect();
        let shown = h
            .query_all_by_label("Sent")
            .find(|n| n.aid().starts_with("label|"))
            .unwrap()
            .rect();
        assert!(shown.bottom() <= send.top() && send.bottom() <= tab.top());
        h.view().with_app(|a| a.p.now += 3_500);
        h.run_steps(1);
        assert!(!notice(&h, "Sent"), "gone after about 3 seconds");
        // Save for later: "Saved for later", the count again.
        h.get_by_label("Save for later").click();
        h.run_steps(1);
        assert!(notice(&h, "Saved for later"));
        assert!(h.query_by_label("Outbox, 2 waiting").is_some());
        // Clear: only when the form has something in it; asks; resets.
        h.get_by_label("Menu").click();
        h.run_steps(2);
        assert!(h.query_by_label("Clear").is_none(), "nothing to clear");
        assert!(h.query_by_label("Refresh").is_some(), "the menu is open");
        h.get_by_label("Refresh").click();
        h.run_steps(1);
        h.get_by_label("Merchant").set_text("Hardware");
        h.run_steps(2);
        h.get_by_label("Menu").click();
        h.run_steps(2);
        h.get_by_label("Clear").click();
        h.run_steps(2);
        assert!(h.query_by_label("Clear this expense?").is_some());
        h.get_by_label("Keep").click();
        h.run_steps(2);
        assert!(h.query_by_label("Clear this expense?").is_none());
        h.view()
            .with_app(|a| assert_eq!(a.form.as_ref().unwrap().merchant, "Hardware"));
        h.get_by_label("Menu").click();
        h.run_steps(2);
        h.get_by_label("Clear").click();
        h.run_steps(2);
        let clear = h
            .query_all_by_label("Clear")
            .find(|n| n.aid().starts_with("button|"))
            .unwrap();
        clear.click();
        h.run_steps(2);
        h.view().with_app(|a| {
            assert_eq!(a.form.as_ref().unwrap().merchant, "");
            assert_eq!(a.outbox.items.len(), 2, "nothing saved by Clear");
        });
    }

    /// Test 64: an outbox item opened from the Outbox tab shows the back arrow;
    /// back with changes asks to discard; back without changes returns to the
    /// Outbox tab. The Expense tab's own unfinished expense is kept meanwhile.
    #[test]
    fn at64_opened_item_back_and_discard() {
        let mut app = paired_app(false);
        let mut f = app.new_form();
        f.merchant = "Cafe".into();
        f.amount = "3.00".into();
        f.date = "2026-10-20".into();
        app.form = Some(f);
        app.save_form(false);
        let mut h = Phone::new(app, 390.0, 844.0);
        h.run_steps(3);
        h.get_by_label("Merchant").set_text("Unfinished");
        h.run_steps(2);
        h.get_by_label("Outbox, 1 waiting").click();
        h.run_steps(3);
        h.get_by_label("Edit").click();
        h.run_steps(3);
        assert!(h.query_by_label("Back").is_some(), "the back arrow");
        assert!(
            h.query_by_label("Date, Tue, Oct 20, 2026").is_some(),
            "its own date"
        );
        assert!(
            !h.nodes().iter().any(|n| n.aid().starts_with("tab|")),
            "an opened item has no tab bar"
        );
        // No changes: back to the Outbox tab.
        h.get_by_label("Back").click();
        h.run_steps(3);
        assert!(h.query_by_label("Outbox, 1 waiting, selected").is_some());
        // Changes: Back asks.
        h.get_by_label("Edit").click();
        h.run_steps(3);
        h.get_by_label("Amount, 3.00").click();
        h.run_steps(3);
        tap(&mut h, "Clear 4 Done");
        h.get_by_label("Back").click();
        h.run_steps(3);
        assert!(h.query_by_label("Discard this expense?").is_some());
        h.get_by_label("Keep editing").click();
        h.run_steps(3);
        assert_eq!(amount_of(&h), "4.00", "still editing");
        h.get_by_label("Back").click();
        h.run_steps(3);
        h.get_by_label("Discard").click();
        h.run_steps(3);
        assert!(h.query_by_label("Outbox, 1 waiting, selected").is_some());
        h.view()
            .with_app(|a| assert_eq!(a.outbox.items[0].fields.amount.as_deref(), Some("3.00")));
        // The Expense tab still holds its own expense.
        h.get_by_label("Expense").click();
        h.run_steps(3);
        h.view().with_app(|a| {
            assert_eq!(a.form.as_ref().unwrap().merchant, "Unfinished");
        });
    }

    /// Test 66, the app's half: the install offer arriving (the page's
    /// notification) shows **Install app** with no other input — on the Expense
    /// tab and, for a first visit, on the Pair screen.
    #[test]
    fn at66_install_offer_shows_at_once() {
        let mut h = Phone::new(paired_app(true), 390.0, 844.0);
        h.run_steps(3);
        assert!(h.query_by_label("Install app").is_none());
        // The browser's offer arrives; the page calls the app's listener
        // (`PhoneView::refresh`, as web::connect registers it).
        h.view().app_mut().p.install.can_prompt = true;
        h.view().refresh();
        assert!(h.query_by_label("Install app").is_some(), "shown at once");
        // A first visit: not paired yet.
        let mut first = CaptureApp::new(Fake {
            online: true,
            loaded: Some(Vec::new()),
            ..Fake::default()
        });
        first.tick();
        let h = Phone::new(first, 390.0, 844.0);
        assert!(h.query_by_label("Install app").is_none());
        h.view().app_mut().p.install.can_prompt = true;
        h.view().refresh();
        assert!(
            h.query_by_label("Install app").is_some(),
            "on the Pair screen"
        );
        assert!(h.query_by_label("Pair").is_some());
    }

    /// Lists with mileage (revision 11): one active vehicle, a saved trip, rates.
    const MILEAGE_LISTS: &str = r#"{"base_currency": "USD", "mileage_enabled": true,
        "reimbursable_enabled": true, "default_purpose": "business",
        "purpose_kinds": [{"kind": "business", "label": "Business"},
            {"kind": "medical", "label": "Medical"}, {"kind": "charity", "label": "Charity"},
            {"kind": "personal", "label": "Personal"}],
        "vehicles": [{"id": "veh-1", "name": "2019 Honda CR-V"},
            {"id": "veh-2", "name": "Church van"}],
        "saved_trips": [{"id": "s1", "name": "Office to client", "from_place": "Office",
            "to_place": "Client", "miles": "12.4", "round_trip": true,
            "purpose_kind": "business", "purpose": "Client visit", "vehicle": "veh-2"}],
        "mileage_rates": [{"kind": "business", "from": "2026-01-01", "cents": "72.5"},
            {"kind": "charity", "from": "2026-01-01", "cents": "14"}]}"#;

    fn mileage_app(online: bool) -> CaptureApp<Fake> {
        let mut app = CaptureApp::new(Fake {
            online,
            loaded: Some(vec![
                ("token".to_owned(), "t".repeat(43)),
                ("lists".to_owned(), MILEAGE_LISTS.to_owned()),
            ]),
            ..Fake::default()
        });
        app.tick();
        app
    }

    fn trip_of(h: &Phone) -> TripForm {
        h.view().with_app(|a| {
            a.form
                .as_ref()
                .and_then(|f| f.trip.clone())
                .unwrap_or_default()
        })
    }

    /// mileage.md M10: on the Expense tab, tapping Expense shows the Mileage form
    /// (title, tab label and icon change; the one-time tip); tapping again returns
    /// with the unfinished expense kept, and the unfinished trip is kept too; the
    /// choice is remembered on the phone.
    #[test]
    fn m10_expense_tab_toggles_mileage() {
        let mut h = Phone::new(mileage_app(false), 390.0, 844.0);
        h.run_steps(3);
        assert!(h.query_by_label(msg::MILEAGE_TIP).is_some(), "the tip");
        h.get_by_label("Merchant").set_text("Corner Grocery");
        h.run_steps(2);
        let tab = h
            .get_by_label("Expense, selected, tap again for Mileage")
            .aid();
        assert!(tab.starts_with("tab|"), "{tab}");
        h.get_by_label("Expense, selected, tap again for Mileage")
            .click();
        h.run_steps(3);
        // The Mileage page: title, tab name, the form.
        assert!(
            h.query_by_label("Mileage, selected, tap again for Expense")
                .is_some()
        );
        assert!(
            h.query_all_by_label("Mileage")
                .any(|n| n.aid().starts_with("heading|")),
            "the title"
        );
        assert!(h.query_by_label("Miles, 0.0").is_some());
        assert!(h.query_by_label("Merchant").is_none());
        assert!(h.query_by_label(msg::MILEAGE_TIP).is_none(), "tip closed");
        assert_eq!(
            h.view()
                .with_app(|a| a.p.store.0.get("page").cloned())
                .as_deref(),
            Some("mileage")
        );
        assert!(
            h.view()
                .with_app(|a| a.p.store.0.contains_key("mileage_tip_seen"))
        );
        // The vehicle is not preselected with two active vehicles; the kind is the
        // book's default.
        let t = trip_of(&h);
        assert_eq!(
            (t.vehicle_id.as_str(), t.purpose_kind.as_str()),
            ("", "business")
        );
        h.get_by_label("From").set_text("Office");
        h.run_steps(2);
        // Another tab and back: still the Mileage page, with the trip kept.
        h.get_by_label("Outbox").click();
        h.run_steps(3);
        assert!(
            h.query_by_label("Mileage").is_some(),
            "the tab's name follows the page"
        );
        h.get_by_label("Mileage").click();
        h.run_steps(3);
        assert_eq!(trip_of(&h).from_place, "Office");
        // Back to Expense: the unfinished expense is there.
        h.get_by_label("Mileage, selected, tap again for Expense")
            .click();
        h.run_steps(3);
        h.view().with_app(|a| {
            let f = a.form.as_ref().unwrap();
            assert!(f.trip.is_none());
            assert_eq!(f.merchant, "Corner Grocery");
        });
        assert!(
            h.query_all_by_label("Expense")
                .any(|n| n.aid().starts_with("heading|"))
        );
        // The choice is remembered: a restart opens on the page last shown.
        let mut app = mileage_app(false);
        app.p.loaded = Some(vec![
            ("token".to_owned(), "t".repeat(43)),
            ("lists".to_owned(), MILEAGE_LISTS.to_owned()),
            ("page".to_owned(), "mileage".to_owned()),
            ("mileage_tip_seen".to_owned(), "1".to_owned()),
        ]);
        app.screen = Screen::Loading;
        app.tick();
        assert!(app.form.as_ref().unwrap().trip.is_some());
        // Without mileage in the book the tab does not toggle.
        let mut h = Phone::new(paired_app(false), 390.0, 844.0);
        h.run_steps(3);
        h.get_by_label("Expense, selected").click();
        h.run_steps(3);
        assert!(h.query_by_label("Merchant").is_some());
        assert!(h.query_by_label(msg::MILEAGE_TIP).is_none());
    }

    /// M13 (phone part) and M2: choosing the saved trip fills the route, miles,
    /// round trip, purpose and vehicle; the computed line; the odometer switch
    /// shows Start and End and turns Round trip off.
    #[test]
    fn m13_saved_trip_fills_the_form() {
        let mut app = mileage_app(false);
        app.toggle_page();
        // Tall enough to show the whole form at once.
        let mut h = Phone::new(app, 390.0, 1400.0);
        h.run_steps(3);
        // The saved trip chosen in its picker.
        h.view().act("Saved trip (optional)", 5, "1");
        h.run_steps(2);
        let t = trip_of(&h);
        assert_eq!(
            (
                t.from_place.as_str(),
                t.to_place.as_str(),
                t.miles.as_str(),
                t.round_trip,
                t.purpose.as_str(),
                t.vehicle_id.as_str()
            ),
            ("Office", "Client", "12.4", true, "Client visit", "veh-2")
        );
        assert!(h.query_by_label("Miles, one way, 12.4").is_some());
        assert!(
            h.query_by_label("24.8 mi \u{d7} 72.5\u{a2} = $17.98")
                .is_some(),
            "the computed line: {:?}",
            h.nodes()
                .iter()
                .map(|n| n.label.clone())
                .filter(|l| l.contains("mi"))
                .collect::<Vec<_>>()
        );
        assert!(
            h.query_by_label("One way 12.4 mi \u{2192} 24.8 mi")
                .is_some()
        );
        // The saved-trip picker is real and lists it.
        assert!(h.query_by_label("Saved trip (optional)").is_some());
        // Odometer readings: Start and End with the keypad, round trip off.
        h.get_by_label("Enter odometer readings").click();
        h.run_steps(3);
        assert!(!trip_of(&h).round_trip);
        h.get_by_label("Odometer start, Enter reading").click();
        h.run_steps(3);
        tap(&mut h, "4 5 2 1 0 . 3 Done");
        h.get_by_label("Odometer end, Enter reading").click();
        h.run_steps(3);
        tap(&mut h, "4 5 2 3 6 Done");
        let t = trip_of(&h);
        assert_eq!(
            (t.odo_start.as_str(), t.odo_end.as_str()),
            ("45210.3", "45236.0")
        );
        // M3: 25.7 mi, the round trip disabled.
        assert!(
            h.query_by_label("25.7 mi \u{d7} 72.5\u{a2} = $18.63")
                .is_some()
        );
        assert!(!h.get_by_label("Round trip").enabled());
    }

    /// M11 (phone part): a trip saved for later appears in the Outbox as "Mileage ·
    /// 24.8 mi · Office → Client"; Send posts it to `/trips` with its client_id;
    /// after Send the form resets with today's date, the same vehicle and purpose
    /// kind; the places are remembered; an older server keeps it in the Outbox.
    #[test]
    fn m11_trips_go_through_the_outbox() {
        let mut app = mileage_app(true);
        app.toggle_page();
        let mut h = Phone::new(app, 390.0, 844.0);
        h.run_steps(3);
        h.view().with_app(|a| {
            let f = a.form.as_mut().unwrap();
            let t = f.trip.as_mut().unwrap();
            t.vehicle_id = "veh-1".into();
            t.purpose_kind = "charity".into();
            t.miles = "12.4".into();
            t.from_place = "Office".into();
            t.to_place = "Client".into();
            t.purpose = "Food pantry delivery".into();
        });
        h.run_steps(2);
        // Save for later: in the Outbox.
        h.get_by_label("Save for later").click();
        h.run_steps(2);
        let t = trip_of(&h);
        assert_eq!(
            (
                t.vehicle_id.as_str(),
                t.purpose_kind.as_str(),
                t.from_place.as_str()
            ),
            ("veh-1", "charity", ""),
            "a new trip with the same vehicle and kind"
        );
        h.get_by_label("Outbox, 1 waiting").click();
        h.run_steps(3);
        assert!(
            h.query_by_label("Mileage \u{b7} 12.4 mi \u{b7} Office \u{2192} Client")
                .is_some()
        );
        // Edit it: the trip form; Send from there.
        h.get_by_label("Edit").click();
        h.run_steps(3);
        assert!(
            h.query_all_by_label("Mileage")
                .any(|n| n.aid().starts_with("heading|"))
        );
        h.view()
            .with_app(|a| a.form.as_mut().unwrap().trip.as_mut().unwrap().round_trip = true);
        h.run_steps(1);
        h.get_by_label("Send").click();
        h.run_steps(3);
        let (id, req) = h.view().with_app(|a| {
            a.p.sent
                .iter()
                .rev()
                .find(|(_, r)| r.path == "/api/v1/trips" && r.method == "POST")
                .cloned()
                .unwrap()
        });
        let body: serde_json::Value = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["miles"], "12.4");
        assert_eq!(body["round_trip"], true);
        assert_eq!(body["vehicle_id"], "veh-1");
        assert_eq!(body["purpose_kind"], "charity");
        assert_eq!(body["date"], "2026-10-23");
        let cid = body["client_id"].as_str().unwrap().to_owned();
        assert!(api::is_uuid(&cid));
        // An older server does not know /trips: the trip stays in the Outbox.
        h.view().with_app(|a| {
            reply(
                a,
                id,
                404,
                r#"{"error":{"code":"not_found","message":"No such address."}}"#,
            );
            a.tick();
            assert_eq!(a.outbox.items.len(), 1);
            assert_eq!(a.outbox.items[0].state, State::Queued);
            assert_eq!(
                a.outbox.items[0].last_error.as_deref(),
                Some(msg::NO_MILEAGE)
            );
            // Sent again later with the same client_id; this time it is stored.
            a.outbox.retry_now();
            a.tick();
            let (id, req) =
                a.p.sent
                    .iter()
                    .rev()
                    .find(|(_, r)| r.path == "/api/v1/trips")
                    .cloned()
                    .unwrap();
            let again: serde_json::Value =
                serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
            assert_eq!(again["client_id"], cid.as_str());
            reply(
                a,
                id,
                201,
                &serde_json::json!({"id": "0123456789abcdef0123456789abcdef", "client_id": cid,
                    "status": "waiting", "created_at": 1, "date": "2026-10-23",
                    "purpose_kind": "charity", "miles": "12.4", "round_trip": true,
                    "from_place": "Office", "to_place": "Client"})
                .to_string(),
            );
            a.tick();
            assert!(a.outbox.items.is_empty());
            assert_eq!(a.places, ["Office", "Client"], "the places are remembered");
            assert!(a.p.store.0.get("places").unwrap().contains("Client"));
        });
        // The Sent list shows it taken with the book's amount.
        h.view().with_app(|a| {
            a.trips = serde_json::from_value(serde_json::json!({"waiting": [], "done": [
                {"id": "0123456789abcdef0123456789abcdef", "client_id": cid, "status": "taken",
                 "created_at": 1, "done_at": 1_793_000_000, "amount": "3.47",
                 "date": "2026-10-23", "purpose_kind": "charity", "miles": "12.4",
                 "round_trip": true, "from_place": "Office", "to_place": "Client"}]}))
            .unwrap();
            a.go_tab(Screen::Sent);
        });
        h.run_steps(3);
        assert!(
            h.query_by_label(
                "Mileage \u{b7} 24.8 mi \u{b7} Office \u{2192} Client \u{2192} Office"
            )
            .is_some()
        );
        assert!(h.query_all_by_label_contains("$3.47").next().is_some());
        // Validation before queueing: a trip without miles is not sent.
        h.view().with_app(|a| {
            a.go_tab(Screen::Expense);
            let f = a.form.as_mut().unwrap();
            f.trip.as_mut().unwrap().to_place = "Somewhere".into();
            a.save_form(true);
            assert!(
                a.form
                    .as_ref()
                    .unwrap()
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("Enter the miles.")
            );
        });
    }

    /// mileage.md revision 2 (phone part): the Mileage form shows the tags the book
    /// has entries for (as the Expense form), below Purpose; the trip sent carries
    /// the chosen ones; a book without tag entries shows none.
    #[test]
    fn mileage_r2_tags_below_purpose_and_sent_with_the_trip() {
        let lists = MILEAGE_LISTS.replacen(
            r#"{"base_currency": "USD","#,
            r#"{"base_currency": "USD", "tags": {"class": ["Roof repair", "Youth"],
                "fund": ["General", "Building"]},"#,
            1,
        );
        let mut app = CaptureApp::new(Fake {
            online: true,
            loaded: Some(vec![
                ("token".to_owned(), "t".repeat(43)),
                ("lists".to_owned(), lists),
            ]),
            ..Fake::default()
        });
        app.tick();
        app.toggle_page();
        let mut h = Phone::new(app, 390.0, 1600.0);
        h.run_steps(3);
        assert!(h.query_by_label("Class (optional)").is_some());
        assert!(h.query_by_label("Fund (optional)").is_some());
        assert!(
            h.query_by_label("Location (optional)").is_none(),
            "no locations in the book"
        );
        let y = |h: &Phone, l: &str| {
            h.query_all_by_label(l)
                .map(|n| n.rect().min.y)
                .reduce(f32::min)
                .unwrap()
        };
        assert!(y(&h, "Purpose") < y(&h, "Class (optional)"));
        assert!(y(&h, "Class (optional)") < y(&h, "Fund (optional)"));
        assert!(y(&h, "Fund (optional)") < y(&h, "Reimbursable"));
        h.view().act("Class (optional)", 5, "1");
        h.view().act("Fund (optional)", 5, "2");
        h.run_steps(2);
        let t = trip_of(&h);
        assert_eq!(
            (t.class.as_str(), t.fund.as_str()),
            ("Roof repair", "Building")
        );
        h.view().with_app(|a| {
            let f = a.form.as_mut().unwrap();
            let t = f.trip.as_mut().unwrap();
            t.vehicle_id = "veh-1".into();
            t.miles = "12.4".into();
            t.to_place = "Church".into();
            t.purpose = "Roof inspection".into();
        });
        h.run_steps(2);
        h.get_by_label("Send").click();
        h.run_steps(3);
        let req = h.view().with_app(|a| {
            a.p.sent
                .iter()
                .rev()
                .find(|(_, r)| r.path == "/api/v1/trips" && r.method == "POST")
                .map(|(_, r)| r.clone())
                .unwrap()
        });
        let body: serde_json::Value = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
        assert_eq!(
            body["tags"],
            serde_json::json!({"class": "Roof repair", "fund": "Building"}),
            "{body}"
        );
        // Opened again from the Outbox, the tags are kept.
        h.view().with_app(|a| {
            let f = CaptureApp::<Fake>::form_from_item(&a.outbox.items[0]);
            let t = f.trip.unwrap();
            assert_eq!(
                (t.class.as_str(), t.fund.as_str()),
                ("Roof repair", "Building")
            );
        });
        // Without tag entries in the book: no tag fields.
        let mut app = mileage_app(false);
        app.toggle_page();
        let mut h = Phone::new(app, 390.0, 1600.0);
        h.run_steps(3);
        for l in ["Class (optional)", "Location (optional)", "Fund (optional)"] {
            assert!(h.query_by_label(l).is_none(), "{l}");
        }
    }

    #[test]
    fn answers_are_classified() {
        let r = |status, body: &str| Response {
            status,
            body: body.as_bytes().to_vec(),
        };
        assert!(matches!(classify(&r(0, "")), Err(Outcome::Retry { .. })));
        assert!(matches!(classify(&r(503, "")), Err(Outcome::Retry { .. })));
        assert!(matches!(classify(&r(507, "")), Err(Outcome::Retry { .. })));
        assert!(
            matches!(classify(&r(422, r#"{"error":{"code":"invalid","message":"Bad"}}"#)), Err(Outcome::Refused { message }) if message == "Bad")
        );
        assert_eq!(
            classify(&r(401, r#"{"error":{"code":"not_paired","message":""}}"#)),
            Err(Outcome::Unpaired)
        );
        assert!(classify(&r(201, "{}")).is_ok());
        // Revision 14: Cloudflare's error pages and a timeout are "not reachable";
        // the capture server's own 507 keeps its message.
        for status in [0, 500, 502, 503, 504, 520, 522, 524, 527, 530] {
            assert_eq!(
                classify(&r(status, CF_1033)),
                Err(Outcome::Retry {
                    message: msg::NOT_REACHABLE.into()
                }),
                "{status}"
            );
        }
        let full = r(
            507,
            r#"{"error":{"code":"storage_full","message":"Storage full — open MoneyKeep on your computer to collect"}}"#,
        );
        assert!(!not_reachable(&full));
        assert_eq!(
            classify(&full),
            Err(Outcome::Retry {
                message: msg::STORAGE_FULL.into()
            })
        );
        assert!(!not_reachable(&r(404, CF_1033)));
    }

    /// Cloudflare's page for a tunnel that is down (its status is 530).
    const CF_1033: &str = "<!DOCTYPE html><html><head><title>Cloudflare Tunnel error | example.com | Cloudflare</title></head><body><h1>Error 1033</h1></body></html>";

    /// Answer every request sent so far that has no answer yet.
    fn answer_all(app: &mut CaptureApp<Fake>, status: u16, body: &str) {
        let ids: Vec<u64> = app.pending.iter().map(|(id, _)| *id).collect();
        for id in ids {
            reply(app, id, status, body);
        }
        app.tick();
    }

    /// mobile-capture.md test 82: an expense saved while every API call answers
    /// 530 stays in the Outbox with the yellow status, the pairing is kept, and it
    /// is sent once, with the same `client_id`, when calls succeed again.
    #[test]
    fn at82_expense_waits_while_the_computer_is_off() {
        let mut app = CaptureApp::new(Fake {
            online: true,
            loaded: Some(vec![
                ("token".to_owned(), "t".repeat(43)),
                ("way".to_owned(), "cloudflare".to_owned()),
            ]),
            ..Fake::default()
        });
        app.tick();
        answer_all(&mut app, 530, CF_1033);
        assert_eq!(app.status(), ("yellow", msg::NOT_REACHABLE));
        let mut f = app.fresh_form();
        f.amount = "12.50".into();
        f.merchant = "Hardware store".into();
        app.form = Some(f);
        app.save_form(true);
        app.tick();
        let posts = |app: &CaptureApp<Fake>| -> Vec<String> {
            app.p
                .sent
                .iter()
                .filter(|(_, r)| r.path == "/api/v1/expenses" && r.method == "POST")
                .map(|(_, r)| {
                    let b: serde_json::Value =
                        serde_json::from_slice(r.body.as_deref().unwrap()).unwrap();
                    b["client_id"].as_str().unwrap().to_owned()
                })
                .collect()
        };
        assert_eq!(posts(&app).len(), 1);
        // Every call answers with Cloudflare's error page, several times over.
        for _ in 0..3 {
            answer_all(&mut app, 530, CF_1033);
            app.p.now += 60_000;
            app.tick();
        }
        answer_all(&mut app, 503, "<html>Service unavailable</html>");
        assert!(app.token.is_some(), "the pairing is kept");
        assert!(!app.p.store.0.contains_key("disconnected"));
        assert!(!app.outbox.unpaired && !app.signin);
        assert_eq!(app.outbox.items.len(), 1, "in the Outbox");
        assert_eq!(app.outbox.items[0].state, State::Queued);
        assert_eq!(
            app.outbox.items[0].last_error.as_deref(),
            Some(msg::NOT_REACHABLE)
        );
        assert_eq!(app.status(), ("yellow", msg::NOT_REACHABLE));
        let cid = app.outbox.items[0].client_id.clone();
        // The screens say so.
        let mut h = Phone::new(app, 390.0, 844.0);
        h.run_steps(3);
        assert!(h.query_by_label(msg::NOT_REACHABLE_DETAIL).is_some());
        h.view().with_app(|a| a.go_tab(Screen::Outbox));
        h.run_steps(3);
        assert!(h.query_by_label(msg::SEND_WHEN_ON).is_some());
        assert!(
            h.query_by_label("Will send when you are on the same Wi-Fi as your computer")
                .is_none()
        );
        h.view().with_app(|app| computer_back(app, &cid));
    }

    /// The computer answers again (test 82): the Outbox sends.
    fn computer_back(app: &mut CaptureApp<Fake>, cid: &str) {
        let posts = |app: &CaptureApp<Fake>| -> Vec<String> {
            app.p
                .sent
                .iter()
                .filter(|(_, r)| r.path == "/api/v1/expenses" && r.method == "POST")
                .map(|(_, r)| {
                    let b: serde_json::Value =
                        serde_json::from_slice(r.body.as_deref().unwrap()).unwrap();
                    b["client_id"].as_str().unwrap().to_owned()
                })
                .collect()
        };
        app.p.now += 60_000;
        app.tick();
        for _ in 0..3 {
            let pend: Vec<(u64, Pending)> = app.pending.clone();
            for (id, what) in pend {
                let body = match what {
                    Pending::Outbox => serde_json::json!({
                        "id": "0123456789abcdef0123456789abcdef", "client_id": cid,
                        "status": "waiting", "created_at": 1})
                    .to_string(),
                    Pending::Lists => r#"{"base_currency": "USD"}"#.to_owned(),
                    Pending::Trips => r#"{"waiting": [], "taken": []}"#.to_owned(),
                    _ => r#"{"waiting": [], "collected": []}"#.to_owned(),
                };
                reply(
                    app,
                    id,
                    if what == Pending::Outbox { 201 } else { 200 },
                    &body,
                );
            }
            app.tick();
        }
        assert!(app.outbox.items.is_empty(), "sent");
        assert_eq!(app.outbox.sent.len(), 1);
        let ids = posts(app);
        assert!(
            ids.len() >= 2 && ids.iter().all(|c| c == cid),
            "the same client id: {ids:?}"
        );
        assert_eq!(app.outbox.sent[0].client_id, cid);
        assert_eq!(app.status(), ("green", "Status: connected"));
    }

    /// mobile-capture.md test 83: with the lists cached and the API answering
    /// 530, the Expense form offers the cached categories and shows "Lists from …".
    #[test]
    fn at83_cached_lists_while_the_computer_is_off() {
        // Nov 8, 2026, 1:05 PM UTC.
        let at = api::parse_date("2026-11-08").unwrap() * 86_400_000 + (13 * 60 + 5) * 60_000;
        let lists = r#"{"base_currency": "USD", "categories": ["Office supplies", "Fuel"]}"#;
        let mut app = CaptureApp::new(Fake {
            online: true,
            now: at + 3_600_000,
            loaded: Some(vec![
                ("token".to_owned(), "t".repeat(43)),
                ("lists".to_owned(), lists.to_owned()),
                ("lists_at".to_owned(), at.to_string()),
            ]),
            ..Fake::default()
        });
        app.tick();
        answer_all(&mut app, 530, CF_1033);
        assert!(app.lists.categories.iter().any(|c| c == "Fuel"), "kept");
        let mut h = Phone::new(app, 390.0, 844.0);
        h.run_steps(3);
        assert!(h.query_by_label("Lists from Nov 8, 1:05 PM").is_some());
        assert!(h.query_by_label(msg::NOT_REACHABLE).is_some());
        // The category picker offers the cached categories (a choice list, not
        // the free-text field).
        let category = h.get_by_label("Category (optional)").aid();
        assert!(category.starts_with("combo|"), "{category}");
        // A successful load replaces them and stores when.
        h.view().with_app(|app| {
            app.p.now += 60_000;
            app.tick();
            let id = lists_request(app);
            reply(
                app,
                id,
                200,
                r#"{"base_currency": "USD", "categories": ["Meals"]}"#,
            );
            app.tick();
            assert_eq!(app.p.store.0.get("lists_at"), Some(&app.p.now.to_string()));
            assert_eq!(app.lists.categories, vec!["Meals".to_owned()]);
        });
        // A phone that never loaded the lists: no note, a free-text category.
        let mut app = paired_app(true);
        answer_all(&mut app, 530, CF_1033);
        assert!(app.lists_note().is_none());
        assert!(app.lists.categories.is_empty());
    }

    fn health_request(app: &CaptureApp<Fake>) -> u64 {
        app.p
            .sent
            .iter()
            .rev()
            .find(|(_, r)| r.path == api::HEALTH_PATH && r.method == "GET")
            .map(|(id, _)| *id)
            .unwrap()
    }

    /// upgrade-resilience-1 U1.4 / UR-4 (the phone's side): the phone reads the
    /// server's `api_level` with its other requests and shows "Your computer's
    /// MoneyKeep needs updating" while it is lower than the phone app needs; a
    /// server from before `api_level` is level 0.
    #[test]
    fn ur4_phone_shows_the_server_needs_updating() {
        let mut app = paired_app(true);
        assert_eq!(api::PHONE_NEEDS_API_LEVEL, 0);
        let id = health_request(&app);
        reply(&mut app, id, 200, r#"{"ok": true, "version": "0.9.0"}"#);
        app.tick();
        assert_eq!(app.server_notice, None, "this phone app needs level 0");
        // A phone app that needs level 2 meets a level-1 server.
        app.needs_api_level = 2;
        app.p.now += 61_000;
        app.tick();
        let id = health_request(&app);
        reply(
            &mut app,
            id,
            200,
            r#"{"ok": true, "version": "0.9.0", "api_level": 1, "later": true}"#,
        );
        app.tick();
        assert_eq!(app.server_notice, Some(msg::SERVER_NEEDS_UPDATE));
        let mut h = Phone::new(app, 390.0, 844.0);
        h.run_steps(3);
        assert!(h.query_by_label(msg::SERVER_NEEDS_UPDATE).is_some());
        assert!(h.query_by_label(msg::SERVER_NEEDS_UPDATE_DETAIL).is_some());
        h.view().with_app(|a| a.go_tab(Screen::Outbox));
        h.run_steps(2);
        assert!(
            h.query_by_label(msg::SERVER_NEEDS_UPDATE).is_some(),
            "every tab"
        );
        // A failed or unreadable health answer keeps what was known; an updated
        // server clears it.
        h.view().with_app(|app| {
            app.p.now += 61_000;
            app.tick();
            let id = health_request(app);
            reply(app, id, 530, CF_1033);
            app.tick();
            assert_eq!(app.server_notice, Some(msg::SERVER_NEEDS_UPDATE));
            app.p.now += 61_000;
            app.tick();
            let id = health_request(app);
            reply(
                app,
                id,
                200,
                r#"{"ok": true, "version": "1.0.0", "api_level": 2}"#,
            );
            app.tick();
            assert_eq!(app.server_notice, None);
        });
        h.run_steps(2);
        assert!(h.query_by_label(msg::SERVER_NEEDS_UPDATE).is_none());
    }

    /// upgrade-resilience-1 UR-7 (M9): the server's "A photo is not available on the
    /// server any more" (422 on `receipt_ids`) is told apart from other refusals.
    #[test]
    fn ur7_photo_not_available_is_its_own_outcome() {
        let gone = Response {
            status: 422,
            body: br#"{"error": {"code": "invalid", "message": "A photo is not available on the server any more; add it again.", "fields": [{"field": "receipt_ids", "message": "A photo is not available on the server any more; add it again."}]}}"#.to_vec(),
        };
        let submit = Action::Submit {
            client_id: "c".into(),
            body: api::NewExpense {
                client_id: "c".into(),
                fields: ExpenseFields::default(),
                receipt_ids: vec!["r1".into()],
            },
        };
        assert!(matches!(
            classify_for(&submit, &gone),
            Err(Outcome::PhotosGone { message }) if message.starts_with("A photo is not available")
        ));
        let patch = Action::Patch {
            client_id: "c".into(),
            server_id: "s".into(),
            body: api::ExpensePatch {
                fields: ExpenseFields::default(),
                receipt_ids: vec!["r1".into()],
            },
        };
        assert!(matches!(
            classify_for(&patch, &gone),
            Err(Outcome::PhotosGone { .. })
        ));
        // Another field's refusal stays a refusal.
        let other = Response {
            status: 422,
            body: br#"{"error": {"code": "invalid", "message": "Bad date.", "fields": [{"field": "date", "message": "Bad date."}]}}"#.to_vec(),
        };
        assert!(matches!(
            classify_for(&submit, &other),
            Err(Outcome::Refused { .. })
        ));
    }

    /// upgrade-resilience-1 UR-7 (M10): the Outbox counts the saved items this
    /// version couldn't read.
    #[test]
    fn ur7_outbox_shows_unreadable_items() {
        let mut app = CaptureApp::new(Fake {
            online: false,
            loaded: Some(vec![
                ("token".to_owned(), "t".repeat(43)),
                (
                    "outbox:later-1".to_owned(),
                    r#"{"v": 2, "client_id": "later-1", "something_new": 1}"#.to_owned(),
                ),
                ("outbox:later-2".to_owned(), "not json".to_owned()),
            ]),
            ..Fake::default()
        });
        // The browser's storage holds what was loaded.
        for (k, v) in app.p.loaded.clone().unwrap() {
            app.p.store.0.insert(k, v);
        }
        app.tick();
        assert_eq!(app.outbox.unreadable, 2);
        assert!(app.outbox.items.is_empty());
        let mut h = Phone::new(app, 390.0, 844.0);
        h.view().with_app(|a| a.go_tab(Screen::Outbox));
        h.run_steps(3);
        let note = crate::outbox::unreadable_note(2).unwrap();
        assert!(note.starts_with("2 saved items couldn't be read by this version"));
        assert!(h.query_by_label(&note).is_some(), "{note}");
        // Nothing was deleted: the stored entries are still there as they were.
        h.view().with_app(|a| {
            assert_eq!(
                a.p.store.0.get("outbox:later-2").map(String::as_str),
                Some("not json")
            );
            assert!(a.p.store.0.contains_key("outbox:later-1"));
        });
    }
}
