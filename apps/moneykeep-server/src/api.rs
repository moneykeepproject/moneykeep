//! Routing, authentication and the phone and desktop endpoints.

use std::io::Read;
use std::sync::atomic::Ordering;
use std::time::Instant;

use mk_capture_api::{
    self as api, ApiError, ExpenseList, ExpensePatch, FieldError, Inbox, Lists, NewExpense,
    PAIRING_CODE_LEN, PAIRING_MAX_WRONG, PAIRING_TTL_SECS, PairRequest, PairResponse, PairingCode,
    ReceiptCreated, SubmissionStatus, TripAck, TripList, TripStatus, msg,
};
use serde::Serialize;

use crate::State;
use crate::jwt::Identity;
use crate::limits::Verdict;
use crate::store::{self, PairOutcome};

/// Which listener a request came in on (mobile-capture.md revision 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Via {
    /// The loopback listener: this computer, and Cloudflare's tunnel (the Access
    /// check applies).
    Loopback,
    /// The Wi-Fi listener (HTTPS on the network): the device token is the check,
    /// and the PIN at pairing.
    Wifi,
}

impl Via {
    fn way(self) -> &'static str {
        match self {
            Via::Loopback => api::VIA_CLOUDFLARE,
            Via::Wifi => api::VIA_WIFI,
        }
    }
}

/// The answer to send.
struct Reply {
    status: u16,
    body: Vec<u8>,
    content_type: &'static str,
}

fn json<T: Serialize>(status: u16, v: &T) -> Reply {
    Reply {
        status,
        body: serde_json::to_vec(v).unwrap_or_else(|_| b"{}".to_vec()),
        content_type: "application/json",
    }
}

fn error(status: u16, code: &str, message: &str) -> Reply {
    json(status, &ApiError::new(code, message))
}

fn field_errors(errors: Vec<FieldError>) -> Reply {
    let message = errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let mut e = ApiError::new("invalid", &message);
    e.error.fields = errors;
    json(422, &e)
}

fn internal(state: &State, what: &str) -> Reply {
    (state.log)(&format!("internal error: {what}"));
    error(
        500,
        "internal",
        "Something went wrong on the capture server.",
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Route {
    Pair,
    Lists,
    PostReceipt,
    PostExpense,
    GetExpenses,
    PatchExpense(String),
    DeleteExpense(String),
    DeleteReceipt(String),
    // Trips (mobile-capture.md revision 11).
    PostTrip,
    GetTrips,
    PatchTrip(String),
    DeleteTrip(String),
    DesktopTripAck(String),
    DesktopPutLists,
    DesktopInbox,
    DesktopReceipt(String),
    DesktopAck(String),
    DesktopPairingCode,
    DesktopDevices,
    DesktopRevoke(String),
    DesktopStatus,
    DesktopPin,
    DesktopWifi,
    PairInfo,
    Health,
    /// A file of the phone app (any path outside `/api/`).
    Page(String),
    NotFound,
}

impl Route {
    fn template(&self) -> &'static str {
        match self {
            Route::Pair => "/api/v1/pair",
            Route::Lists => "/api/v1/lists",
            Route::PostReceipt => "/api/v1/receipts",
            Route::PostExpense | Route::GetExpenses => "/api/v1/expenses",
            Route::PatchExpense(_) | Route::DeleteExpense(_) => "/api/v1/expenses/{id}",
            Route::DeleteReceipt(_) => "/api/v1/receipts/{id}",
            Route::PostTrip | Route::GetTrips => "/api/v1/trips",
            Route::PatchTrip(_) | Route::DeleteTrip(_) => "/api/v1/trips/{id}",
            Route::DesktopTripAck(_) => "/api/v1/desktop/trips/{id}/ack",
            Route::DesktopPutLists => "/api/v1/desktop/lists",
            Route::DesktopInbox => "/api/v1/desktop/inbox",
            Route::DesktopReceipt(_) => "/api/v1/desktop/receipts/{id}",
            Route::DesktopAck(_) => "/api/v1/desktop/inbox/{id}/ack",
            Route::DesktopPairingCode => "/api/v1/desktop/pairing-codes",
            Route::DesktopDevices => "/api/v1/desktop/devices",
            Route::DesktopRevoke(_) => "/api/v1/desktop/devices/{id}",
            Route::DesktopStatus => "/api/v1/desktop/status",
            Route::DesktopPin => "/api/v1/desktop/pin",
            Route::DesktopWifi => "/api/v1/desktop/wifi",
            Route::PairInfo => "/api/v1/pair-info",
            Route::Health => "/api/v1/health",
            Route::Page(_) => "(page)",
            Route::NotFound => "(other)",
        }
    }

    fn desktop(&self) -> bool {
        matches!(
            self,
            Route::DesktopPutLists
                | Route::DesktopInbox
                | Route::DesktopReceipt(_)
                | Route::DesktopAck(_)
                | Route::DesktopTripAck(_)
                | Route::DesktopPairingCode
                | Route::DesktopDevices
                | Route::DesktopRevoke(_)
                | Route::DesktopStatus
                | Route::DesktopPin
                | Route::DesktopWifi
        )
    }

    /// Routes a service-token identity may use (revision 2: the status check and the
    /// desktop API, which still needs the desktop token).
    fn service_allowed(&self) -> bool {
        self.desktop() || *self == Route::Health
    }
}

fn id_ok(s: &str) -> Option<String> {
    api::is_receipt_id(s).then(|| s.to_owned())
}

fn route(method: &str, path: &str) -> Route {
    if method == "GET" && !path.starts_with("/api/") && path != "/api" {
        return Route::Page(path.to_owned());
    }
    let segs: Vec<&str> = path
        .strip_prefix(api::API_PREFIX)
        .unwrap_or("\u{0}")
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let r = match (method, segs.as_slice()) {
        ("POST", ["pair"]) => Some(Route::Pair),
        ("GET", ["lists"]) => Some(Route::Lists),
        ("POST", ["receipts"]) => Some(Route::PostReceipt),
        ("DELETE", ["receipts", id]) => id_ok(id).map(Route::DeleteReceipt),
        ("POST", ["expenses"]) => Some(Route::PostExpense),
        ("GET", ["expenses"]) => Some(Route::GetExpenses),
        ("PATCH", ["expenses", id]) => id_ok(id).map(Route::PatchExpense),
        ("DELETE", ["expenses", id]) => id_ok(id).map(Route::DeleteExpense),
        ("POST", ["trips"]) => Some(Route::PostTrip),
        ("GET", ["trips"]) => Some(Route::GetTrips),
        ("PATCH", ["trips", id]) => id_ok(id).map(Route::PatchTrip),
        ("DELETE", ["trips", id]) => id_ok(id).map(Route::DeleteTrip),
        ("POST", ["desktop", "trips", id, "ack"]) => id_ok(id).map(Route::DesktopTripAck),
        ("PUT", ["desktop", "lists"]) => Some(Route::DesktopPutLists),
        ("GET", ["desktop", "inbox"]) => Some(Route::DesktopInbox),
        ("GET", ["desktop", "receipts", id]) => id_ok(id).map(Route::DesktopReceipt),
        ("POST", ["desktop", "inbox", id, "ack"]) => id_ok(id).map(Route::DesktopAck),
        ("POST", ["desktop", "pairing-codes"]) => Some(Route::DesktopPairingCode),
        ("GET", ["desktop", "devices"]) => Some(Route::DesktopDevices),
        ("DELETE", ["desktop", "devices", id]) => id_ok(id).map(Route::DesktopRevoke),
        ("GET", ["desktop", "status"]) => Some(Route::DesktopStatus),
        ("PUT", ["desktop", "pin"]) => Some(Route::DesktopPin),
        ("GET", ["desktop", "wifi"]) => Some(Route::DesktopWifi),
        ("GET", ["pair-info"]) => Some(Route::PairInfo),
        ("GET", ["health"]) => Some(Route::Health),
        _ => None,
    };
    r.unwrap_or(Route::NotFound)
}

fn header(req: &tiny_http::Request, name: &'static str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().trim().to_owned())
}

/// Largest body a route accepts.
fn body_limit(state: &State, r: &Route) -> usize {
    match r {
        Route::PostReceipt => state.config.max_receipt_bytes(),
        Route::DesktopPutLists => api::MAX_LISTS_BYTES,
        _ => api::MAX_JSON_BYTES,
    }
}

/// Read the body within `limit`; `Err` with the 413 reply when it is larger.
fn read_body(req: &mut tiny_http::Request, limit: usize) -> Result<Vec<u8>, Reply> {
    let too_big = || error(413, "too_large", "This upload is too large.");
    let declared = req.body_length();
    // Read (and discard) an oversized body up to twice the limit so the client sees
    // the answer; beyond that the connection is simply answered and closed.
    let cap = u64::try_from(limit.saturating_mul(2)).unwrap_or(u64::MAX);
    if declared.is_some_and(|n| n > limit) {
        let _ = std::io::copy(&mut req.as_reader().take(cap), &mut std::io::sink());
        return Err(too_big());
    }
    let mut body = Vec::with_capacity(declared.unwrap_or(0).min(limit));
    let take = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    if req.as_reader().take(take).read_to_end(&mut body).is_err() {
        return Err(error(
            400,
            "bad_request",
            "The request body could not be read.",
        ));
    }
    if body.len() > limit {
        let _ = std::io::copy(&mut req.as_reader().take(cap), &mut std::io::sink());
        return Err(too_big());
    }
    Ok(body)
}

/// A sent Access token failed the check (mobile-capture.md revision 8).
fn access_refused() -> Reply {
    error(401, "access_refused", msg::ACCESS_REFUSED)
}

/// Log why Access refused a request and keep it for `desktop/status`.
fn refuse(state: &State, reason: &str, now: i64) {
    (state.log)(reason);
    if let Ok(mut p) = state.access_problem.lock() {
        *p = Some(api::AccessProblem {
            reason: reason.to_owned(),
            at: now,
        });
    }
}

/// Answer one request.
pub(crate) fn handle(state: &State, mut req: tiny_http::Request, via: Via) {
    let started = Instant::now();
    let method = req.method().as_str().to_ascii_uppercase();
    let path = req.url().split('?').next().unwrap_or("").to_owned();
    let r = route(&method, &path);
    let method = crate::log_method(&method);
    // Revision 18: a page file in the coding the phone accepts.
    let (reply, extra) = match &r {
        // L3: the phone app's files too need a valid Access token when they are
        // asked for through the tunnel.
        Route::Page(_) if through_tunnel(&req, via) && !tunnel_access_ok(state, &req) => (
            error(403, "access_missing", msg::ACCESS_REFUSED),
            Vec::new(),
        ),
        Route::Page(p) => page(state, p, header(&req, "Accept-Encoding").as_deref()),
        _ => (process(state, &mut req, &r, via), Vec::new()),
    };
    let ms = started.elapsed().as_millis();
    (state.log)(&format!(
        "{method} {} {} {ms}ms{}",
        r.template(),
        reply.status,
        if via == Via::Wifi { " (wifi)" } else { "" }
    ));
    let mut resp = tiny_http::Response::from_data(reply.body)
        .with_status_code(tiny_http::StatusCode(reply.status));
    let cache = if matches!(r, Route::Page(_)) {
        "no-cache"
    } else {
        "no-store"
    };
    // The security headers go with every answer (pages and API alike).
    for (k, v) in [
        ("Content-Type", reply.content_type),
        ("Cache-Control", cache),
    ]
    .into_iter()
    .chain(api::SECURITY_HEADERS)
    .chain(extra.iter().map(|(k, v)| (*k, v.as_str())))
    {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    }
    let _ = req.respond(resp);
}

/// Where a Wi-Fi request is handled (security-review-1 A4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WifiLane {
    /// A paired phone's request (a valid device token): the Wi-Fi workers.
    Paired,
    /// The phone app's files, the health check and pair-info: no body, answered
    /// from memory, on their own workers.
    Public,
    /// Everything else without a valid token (pairing): its own worker, at most
    /// [`crate::wifi::MAX_UNAUTH`] at a time, the body within 10 seconds.
    Unauthenticated,
}

/// Which lane a Wi-Fi request takes (read before its body).
pub(crate) fn wifi_lane(state: &State, req: &tiny_http::Request) -> WifiLane {
    let method = req.method().as_str().to_ascii_uppercase();
    let path = req.url().split('?').next().unwrap_or("").to_owned();
    match route(&method, &path) {
        Route::Page(_) | Route::Health | Route::PairInfo => return WifiLane::Public,
        Route::Pair => return WifiLane::Unauthenticated,
        _ => {}
    }
    let paired = header(req, "Authorization")
        .and_then(|h| {
            h.strip_prefix("Bearer ")
                .or_else(|| h.strip_prefix("bearer "))
                .map(|t| t.trim().to_owned())
        })
        .filter(|t| api::is_token_shape(t))
        .and_then(|t| {
            state
                .store
                .lock()
                .ok()
                .and_then(|s| s.find_token(&store::token_hash(&t)).ok().flatten())
        })
        .is_some_and(|t| !t.revoked && t.kind == "device");
    if paired {
        WifiLane::Paired
    } else {
        WifiLane::Unauthenticated
    }
}

/// The plain-HTTP helper of Wi-Fi capture (revision 10): only the certificate page,
/// the CA certificate and the iPhone profile (all public); every other path is
/// redirected to the HTTPS address.
pub(crate) fn helper(state: &State, req: tiny_http::Request) {
    let started = Instant::now();
    let method = crate::log_method(&req.method().as_str().to_ascii_uppercase());
    let path = req.url().split('?').next().unwrap_or("").to_owned();
    let (status, body, ct, location, label): (u16, Vec<u8>, &str, Option<String>, &str) =
        match (state.wifi.as_ref(), method.as_str(), path.as_str()) {
            (Some(w), "GET" | "HEAD", "/") => {
                let info = w.info.read().map(|i| i.clone()).unwrap_or_default();
                let ua = header(&req, "User-Agent").unwrap_or_default();
                let page = crate::wifi::page_for(&info, w.listen.https_port, &ua);
                (
                    200,
                    page.into_bytes(),
                    "text/html; charset=utf-8",
                    None,
                    "/",
                )
            }
            (Some(w), "GET" | "HEAD", api::CA_CRT_PATH) => (
                200,
                w.ca_der.read().map(|d| d.clone()).unwrap_or_default(),
                "application/x-x509-ca-cert",
                None,
                api::CA_CRT_PATH,
            ),
            (Some(w), "GET" | "HEAD", api::CA_MOBILECONFIG_PATH) => {
                let der = w.ca_der.read().map(|d| d.clone()).unwrap_or_default();
                let name = w.info.read().map(|i| i.ca_name.clone()).unwrap_or_default();
                (
                    200,
                    crate::wifi::mobileconfig(&der, &name).into_bytes(),
                    "application/x-apple-aspen-config",
                    None,
                    api::CA_MOBILECONFIG_PATH,
                )
            }
            (Some(w), _, _) => {
                let to = format!(
                    "https://{}:{}{}",
                    w.listen.target.host(),
                    w.listen.https_port,
                    if path.starts_with('/') {
                        path.as_str()
                    } else {
                        "/"
                    }
                );
                (
                    301,
                    Vec::new(),
                    "text/plain; charset=utf-8",
                    Some(to),
                    "(redirect)",
                )
            }
            (None, _, _) => (
                404,
                b"Not found".to_vec(),
                "text/plain; charset=utf-8",
                None,
                "(off)",
            ),
        };
    (state.log)(&format!(
        "{method} {label} {status} {}ms (wifi helper)",
        started.elapsed().as_millis()
    ));
    let body = if method == "HEAD" { Vec::new() } else { body };
    let mut resp =
        tiny_http::Response::from_data(body).with_status_code(tiny_http::StatusCode(status));
    let mut headers: Vec<(&str, String)> = vec![
        ("Content-Type", ct.to_owned()),
        ("Cache-Control", "no-store".to_owned()),
    ];
    if let Some(l) = location {
        headers.push(("Location", l));
    }
    if path == api::CA_CRT_PATH {
        headers.push((
            "Content-Disposition",
            "attachment; filename=\"moneykeep-ca.crt\"".to_owned(),
        ));
    }
    if path == api::CA_MOBILECONFIG_PATH {
        headers.push((
            "Content-Disposition",
            "attachment; filename=\"moneykeep-ca.mobileconfig\"".to_owned(),
        ));
    }
    for (k, v) in headers
        .iter()
        .map(|(k, v)| (*k, v.as_str()))
        .chain(crate::wifi::HELPER_HEADERS)
    {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    }
    let _ = req.respond(resp);
}

/// Headers Cloudflare's edge adds to every request it sends through the tunnel
/// (a request from this computer has none of them).
const CLOUDFLARE_HEADERS: [&str; 6] = [
    "CF-Connecting-IP",
    "CF-Ray",
    "CF-Visitor",
    "CF-IPCountry",
    "CDN-Loop",
    "Cf-Access-Jwt-Assertion",
];

/// The request came through the tunnel (security-review-1 L3): it arrived on the
/// loopback listener carrying a header only Cloudflare adds.
fn through_tunnel(req: &tiny_http::Request, via: Via) -> bool {
    via == Via::Loopback && CLOUDFLARE_HEADERS.iter().any(|h| header(req, h).is_some())
}

/// A tunnel request carries an Access token that passes the check.
fn tunnel_access_ok(state: &State, req: &tiny_http::Request) -> bool {
    let now = state.clock.now();
    match (
        state.verifier.as_ref(),
        header(req, "Cf-Access-Jwt-Assertion"),
    ) {
        (Some(v), Some(t)) => v.verify(&t, now).is_ok(),
        _ => false,
    }
}

/// The caller's address for failure counting (security-review-1 L2): Cloudflare's
/// `CF-Connecting-IP` only when the request also carried a valid Access token;
/// other tunnel requests count together as "tunnel" (so they never block this
/// computer's own requests); else the connection's address.
fn source(state: &State, req: &tiny_http::Request, via: Via, access_ok: bool) -> String {
    if via == Via::Wifi {
        // The relay connects from a loopback port; the phone's address is known by
        // that port. A `CF-Connecting-IP` header from the network is not believed.
        let port = req.remote_addr().map(|a| a.port());
        let phone = port.and_then(|p| crate::wifi::peer_ip(&state.peers, p));
        return phone.map_or_else(|| "wifi".into(), |ip| ip.to_string());
    }
    if through_tunnel(req, via) {
        if !access_ok {
            return "tunnel".into();
        }
        return header(req, "CF-Connecting-IP")
            .filter(|s| s.len() <= 64)
            .unwrap_or_else(|| "tunnel".into());
    }
    req.remote_addr()
        .map_or_else(|| "unknown".into(), |a| a.ip().to_string())
}

/// A file of the phone app, or the page saying it was not built in. The files are
/// the same for everyone (no data), so no token is needed; Cloudflare Access still
/// stands in front of the public address.
///
/// Revision 18: the Brotli or gzip copy when `accept` (the request's
/// `Accept-Encoding`) allows it, with `Content-Encoding` and `Vary`; the program
/// carries its uncompressed size in `X-MK-Program-Size` (for the loading screen's
/// progress). upgrade-resilience-1 M8: every file carries its build in `X-MK-Build`.
fn page(state: &State, path: &str, accept: Option<&str>) -> (Reply, Vec<(&'static str, String)>) {
    match &state.phone_app {
        Some(app) => match app.get_encoded(path, accept) {
            Some(f) => {
                let mut extra = Vec::new();
                if let Some(c) = f.coding.header() {
                    extra.push(("Content-Encoding", c.to_owned()));
                }
                if f.varies {
                    extra.push(("Vary", "Accept-Encoding".to_owned()));
                }
                // upgrade-resilience-1 M8: the build each file is of, so the
                // service worker never mixes builds.
                extra.push((
                    "X-MK-Build",
                    crate::phone_app::build_text(crate::CAPTURE_VERSION),
                ));
                let p = if path == "/" { "/index.html" } else { path };
                if p == crate::phone_app::PROGRAM {
                    extra.push(("X-MK-Program-Size", f.size.to_string()));
                }
                (
                    Reply {
                        status: 200,
                        body: f.body.to_vec(),
                        content_type: f.content_type,
                    },
                    extra,
                )
            }
            None => (
                Reply {
                    status: 404,
                    body: b"Not found".to_vec(),
                    content_type: "text/plain; charset=utf-8",
                },
                Vec::new(),
            ),
        },
        None => (
            Reply {
                status: 200,
                body: crate::phone_app::NOT_BUILT.as_bytes().to_vec(),
                content_type: "text/html; charset=utf-8",
            },
            Vec::new(),
        ),
    }
}

fn process(state: &State, req: &mut tiny_http::Request, r: &Route, via: Via) -> Reply {
    if let Route::Page(path) = r {
        return page(state, path, None).0;
    }
    // The desktop API stays on this computer: its token never crosses the Wi-Fi.
    if via == Via::Wifi && r.desktop() {
        return error(404, "not_found", "No such address.");
    }
    let now = state.clock.now();
    if now - state.last_purge.load(Ordering::SeqCst) >= 60 {
        state.last_purge.store(now, Ordering::SeqCst);
        if let Ok(s) = state.store.lock()
            && let Err(e) = s.purge(now)
        {
            (state.log)(&format!("housekeeping failed: {e}"));
        }
        if let Ok(mut l) = state.limits.lock() {
            l.tidy(now);
        }
    }
    // 1. Cloudflare Access. The desktop usually connects on the loopback address or
    //    the LAN without Access; a token it does send is still checked. A service
    //    token's identity (the desktop's status check through the public address) may
    //    use only the health route and the desktop API. Only the loopback listener
    //    (behind the tunnel) has the Access check; on the Wi-Fi listener the device
    //    token is the check (revision 10). Every request that came through the
    //    tunnel needs a valid Access token, desktop API included (L3).
    let tunnel = through_tunnel(req, via);
    let checked = match state.verifier.as_ref().filter(|_| via == Via::Loopback) {
        Some(v) => header(req, "Cf-Access-Jwt-Assertion").map(|t| v.verify(&t, now)),
        None => None,
    };
    let access_ok = matches!(checked, Some(Ok(_)));
    let src = source(state, req, via, access_ok);
    let fail = |state: &State| {
        if let Ok(mut l) = state.limits.lock() {
            l.failure(&src, now);
        }
    };
    if state.limits.lock().is_ok_and(|l| l.blocked(&src, now)) {
        return error(429, "too_many_requests", msg::TOO_MANY_REQUESTS);
    }
    if tunnel && !access_ok && !matches!(checked, Some(Err(_))) {
        refuse(
            state,
            &format!(
                "request through the tunnel without a valid Cloudflare Access token on {}",
                r.template()
            ),
            now,
        );
        fail(state);
        return error(403, "access_missing", msg::ACCESS_REFUSED);
    }
    if state.verifier.is_some() && via == Via::Loopback {
        let identity = match checked {
            Some(Ok(id)) => Some(id),
            Some(Err(e)) => {
                refuse(state, &e.describe(), now);
                fail(state);
                return access_refused();
            }
            None if r.service_allowed() => None,
            None => {
                refuse(
                    state,
                    &format!(
                        "request without a Cloudflare Access token on {} \u{2014} is this address behind Access?",
                        r.template()
                    ),
                    now,
                );
                fail(state);
                return error(401, "access_missing", msg::ACCESS_REFUSED);
            }
        };
        if let Some(Identity::Service(client_id)) = &identity {
            let expected = state.config.access_service_client_id.trim();
            let known = expected.is_empty() || expected == client_id;
            if !r.service_allowed() || !known {
                refuse(
                    state,
                    "a service token identity was refused for this address",
                    now,
                );
                fail(state);
                return access_refused();
            }
        }
        // A token passed the check: the settings work again (revision 8).
        if identity.is_some()
            && let Ok(mut p) = state.access_problem.lock()
        {
            *p = None;
        }
    }
    if *r == Route::Health {
        return json(
            200,
            &api::Health {
                ok: true,
                version: crate::VERSION.to_owned(),
                capture_version: Some(crate::CAPTURE_VERSION.to_owned()),
                api_level: api::API_LEVEL,
                wifi_error: state.wifi_error.clone(),
                // L8: whether the Wi-Fi listener answers now.
                wifi_listening: match &state.wifi {
                    Some(w) => Some(w.listening.load(Ordering::SeqCst)),
                    None => state.wifi_error.as_ref().map(|_| false),
                },
            },
        );
    }
    if *r == Route::PairInfo {
        let pin = match state.store.lock().map(|s| s.pin()) {
            Ok(Ok(p)) => p.is_some(),
            Ok(Err(e)) => return internal(state, &e),
            Err(_) => return internal(state, "store lock"),
        };
        return json(
            200,
            &api::PairInfo {
                pin_required: pin || via == Via::Wifi,
                way: via.way().to_owned(),
                // Revision 18: a Wi-Fi phone's loading screen offers Use Cloudflare
                // when the computer can't be reached and this address is known.
                cloudflare_url: (via == Via::Wifi)
                    .then(|| state.config.client_url.clone())
                    .flatten()
                    .filter(|u| u.starts_with("https://")),
            },
        );
    }

    // 2. Bearer token.
    let token = if *r == Route::Pair {
        None
    } else {
        let bearer = header(req, "Authorization")
            .and_then(|h| {
                h.strip_prefix("Bearer ")
                    .or_else(|| h.strip_prefix("bearer "))
                    .map(|t| t.trim().to_owned())
            })
            .filter(|t| api::is_token_shape(t));
        let found = match &bearer {
            Some(t) => match state.store.lock() {
                Ok(s) => s.find_token(&store::token_hash(t)).ok().flatten(),
                Err(_) => None,
            },
            None => None,
        };
        let want = if r.desktop() { "desktop" } else { "device" };
        match found {
            Some(t) if !t.revoked && (t.kind == want || *r == Route::NotFound) => Some(t),
            _ => {
                fail(state);
                return if r.desktop() {
                    error(401, "bad_token", "The desktop token is not valid.")
                } else {
                    error(401, "not_paired", msg::DISCONNECTED)
                };
            }
        }
    };
    if let Some(t) = &token {
        if state
            .limits
            .lock()
            .map(|mut l| l.request(&t.id, now))
            .unwrap_or(Verdict::Ok)
            == Verdict::Limited
        {
            return error(429, "too_many_requests", msg::TOO_MANY_REQUESTS);
        }
        if t.kind == "device"
            && let Ok(s) = state.store.lock()
        {
            let _ = s.touch(&t.id, now);
        }
    }
    if *r == Route::NotFound {
        return error(404, "not_found", "No such address.");
    }

    // 3. Body.
    let body = match read_body(req, body_limit(state, r)) {
        Ok(b) => b,
        Err(reply) => return reply,
    };
    let device = token.as_ref().map(|t| t.id.clone()).unwrap_or_default();
    let content_type = header(req, "Content-Type").unwrap_or_default();
    match r {
        Route::Pair => pair(state, &body, now, via, || fail(state)),
        Route::Lists => get_lists(state, wants_mileage(req.url())),
        Route::PostReceipt => post_receipt(state, &device, &content_type, &body, now),
        Route::PostExpense => post_expense(state, &device, &body, now),
        Route::GetExpenses => get_expenses(state, &device),
        Route::PatchExpense(id) => patch_expense(state, &device, id, &body, now),
        Route::DeleteExpense(id) => delete_expense(state, &device, id),
        Route::DeleteReceipt(id) => delete_receipt(state, &device, id),
        Route::PostTrip => post_trip(state, &device, &body, now),
        Route::GetTrips => get_trips(state, &device),
        Route::PatchTrip(id) => patch_trip(state, &device, id, &body, now),
        Route::DeleteTrip(id) => delete_trip(state, &device, id),
        Route::DesktopTripAck(id) => ack_trip(state, id, &body, now),
        Route::DesktopPutLists => put_lists(state, &body, now),
        Route::DesktopInbox => inbox(state),
        Route::DesktopReceipt(id) => desktop_receipt(state, id),
        Route::DesktopAck(id) => ack(state, id, now),
        Route::DesktopPairingCode => pairing_code(state, now),
        Route::DesktopDevices => devices(state),
        Route::DesktopRevoke(id) => revoke(state, id),
        Route::DesktopStatus => desktop_status(state),
        Route::DesktopPin => set_pin(state, &body),
        Route::DesktopWifi => json(200, &wifi_state(state)),
        Route::Health | Route::PairInfo | Route::Page(_) | Route::NotFound => {
            error(404, "not_found", "No such address.")
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, Reply> {
    serde_json::from_slice(body).map_err(|_| {
        error(
            400,
            "bad_request",
            "The request is not valid JSON for this address.",
        )
    })
}

macro_rules! store {
    ($state:expr) => {
        match $state.store.lock() {
            Ok(s) => s,
            Err(_) => return internal($state, "store lock"),
        }
    };
}

macro_rules! tryr {
    ($state:expr, $e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return internal($state, &e),
        }
    };
}

// ----- phone API -----------------------------------------------------------------------

fn pair(state: &State, body: &[u8], now: i64, via: Via, fail: impl Fn()) -> Reply {
    let p: PairRequest = match parse(body) {
        Ok(p) => p,
        Err(r) => return r,
    };
    // The Capture PIN (revision 10): always on the Wi-Fi address; on the Cloudflare
    // address once one is set.
    let stored_pin = match state.store.lock() {
        Ok(s) => tryr!(state, s.pin()),
        Err(_) => return internal(state, "store lock"),
    };
    if via == Via::Wifi && stored_pin.is_none() {
        return error(409, "pin_not_set", msg::PIN_NOT_SET);
    }
    let pin_ok = match &stored_pin {
        None => true,
        Some(h) => p
            .pin
            .as_deref()
            .is_some_and(|pin| crate::pin::verify(pin.trim(), h)),
    };
    let code = api::normalize_code(&p.code);
    let name = match api::validate_device_name(&p.device_name) {
        Ok(n) => n,
        Err(m) => {
            return field_errors(vec![FieldError {
                field: "device_name".into(),
                message: m,
            }]);
        }
    };
    let s = store!(state);
    let outcome = if code.len() == PAIRING_CODE_LEN {
        tryr!(
            state,
            s.try_pair_pin(&store::token_hash(&code), now, PAIRING_MAX_WRONG, pin_ok)
        )
    } else {
        tryr!(state, s.try_pair(b"-", now, PAIRING_MAX_WRONG))
    };
    match outcome {
        PairOutcome::Paired => {
            let token = tryr!(state, store::new_token());
            let id = tryr!(
                state,
                s.add_device(&name, &store::token_hash(&token), now, via.way())
            );
            json(
                201,
                &PairResponse {
                    device_id: id,
                    device_token: token.to_string(),
                },
            )
        }
        PairOutcome::Wrong => {
            fail();
            error(401, "pair_failed", msg::PAIR_FAILED)
        }
        PairOutcome::PinWrong => {
            fail();
            error(401, "pin_wrong", msg::PIN_WRONG)
        }
    }
}

/// The phone asks for the mileage lists (`?mileage=1`, revision 11); an older
/// phone app, which refuses fields it does not know, gets the lists without them.
fn wants_mileage(url: &str) -> bool {
    url.split_once('?')
        .is_some_and(|(_, q)| q.split('&').any(|p| p == "mileage=1"))
}

fn get_lists(state: &State, mileage: bool) -> Reply {
    let s = store!(state);
    match tryr!(state, s.lists()) {
        Some((_, bytes, at)) => match serde_json::from_slice::<Lists>(&bytes) {
            Ok(mut l) => {
                l.published_at = Some(at);
                if !mileage {
                    l = l.without_mileage();
                }
                json(200, &l)
            }
            Err(e) => internal(state, &e.to_string()),
        },
        None => json(200, &Lists::default()),
    }
}

fn post_receipt(state: &State, device: &str, content_type: &str, body: &[u8], now: i64) -> Reply {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let png = match ct.as_str() {
        "image/jpeg" | "image/jpg" => false,
        "image/png" => true,
        "image/heic" | "image/heif" => return error(415, "unsupported_type", msg::PHOTO_TYPE),
        _ => {
            return error(
                415,
                "unsupported_type",
                "Receipts are uploaded as JPEG or PNG pictures.",
            );
        }
    };
    let magic_ok = if png {
        body.starts_with(&[0x89, b'P', b'N', b'G'])
    } else {
        body.starts_with(&[0xFF, 0xD8, 0xFF])
    };
    if !magic_ok {
        let heic = body.get(4..8) == Some(b"ftyp");
        return if heic {
            error(415, "unsupported_type", msg::PHOTO_TYPE)
        } else {
            error(422, "unreadable", msg::PICTURE)
        };
    }
    {
        let s = store!(state);
        let held = tryr!(state, s.inbox_bytes());
        let len = u64::try_from(body.len()).unwrap_or(u64::MAX);
        if held.saturating_add(len) > state.config.inbox_limit_bytes() {
            return error(507, "storage_full", msg::STORAGE_FULL);
        }
    }
    if state
        .limits
        .lock()
        .map(|mut l| l.upload(device, u64::try_from(body.len()).unwrap_or(u64::MAX), now))
        .unwrap_or(Verdict::Ok)
        == Verdict::Limited
    {
        return error(
            429,
            "too_many_requests",
            "Today's upload allowance is used up — try again tomorrow",
        );
    }
    // Decode, turn upright, strip all metadata (including GPS), scale and re-encode
    // (the same code as the desktop's receipts, expenses.md).
    let name = if png { "receipt.png" } else { "receipt.jpg" };
    let processed = match mk_import::receipt::process(body, name) {
        Ok(p) => p,
        Err(e) => {
            let t = e.to_string();
            return if t.contains(mk_import::receipt::MSG_HEIC) {
                error(415, "unsupported_type", msg::PHOTO_TYPE)
            } else if t.contains(mk_import::receipt::MSG_TOO_BIG) {
                error(413, "too_large", "This upload is too large.")
            } else {
                error(422, "unreadable", msg::PICTURE)
            };
        }
    };
    let (Some(w), Some(h)) = (processed.width, processed.height) else {
        return error(422, "unreadable", msg::PICTURE);
    };
    let s = store!(state);
    let meta = tryr!(
        state,
        s.add_receipt(
            device,
            processed.kind.as_str(),
            &processed.content,
            w,
            h,
            now
        )
    );
    json(
        201,
        &ReceiptCreated {
            receipt_id: meta.id,
            kind: meta.kind,
            bytes: meta.bytes,
            width: w,
            height: h,
        },
    )
}

/// Every receipt must be this phone's, not collected, and free or already in `sub`.
fn receipts_available(
    s: &store::Store,
    device: &str,
    ids: &[String],
    sub: Option<&str>,
) -> Result<bool, String> {
    for id in ids {
        match s.receipt(id)? {
            Some(r)
                if r.device_id == device
                    && !r.collected
                    && (r.submission_id.is_none() || r.submission_id.as_deref() == sub) => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

fn receipt_unavailable() -> Reply {
    field_errors(vec![FieldError {
        field: "receipt_ids".into(),
        message: "A photo is not available on the server any more; add it again.".into(),
    }])
}

fn today(now: i64) -> i64 {
    api::today_from_unix(now, 0)
}

fn post_expense(state: &State, device: &str, body: &[u8], now: i64) -> Reply {
    let e: NewExpense = match parse(body) {
        Ok(e) => e,
        Err(r) => return r,
    };
    if !api::is_uuid(&e.client_id) {
        return field_errors(vec![FieldError {
            field: "client_id".into(),
            message: "client_id must be a UUID.".into(),
        }]);
    }
    let s = store!(state);
    if let Some(existing) = tryr!(state, s.by_client_id(device, &e.client_id)) {
        return json(200, &existing);
    }
    let valid = match api::validate_expense(
        &e.fields,
        &e.receipt_ids,
        today(now),
        state.config.max_receipts_per_expense,
    ) {
        Ok(v) => v,
        Err(errors) => return field_errors(errors),
    };
    if !tryr!(
        state,
        receipts_available(&s, device, &valid.receipt_ids, None)
    ) {
        return receipt_unavailable();
    }
    let sub = tryr!(
        state,
        s.add_submission(device, &e.client_id, &valid.fields, &valid.receipt_ids, now)
    );
    json(201, &sub)
}

fn get_expenses(state: &State, device: &str) -> Reply {
    let s = store!(state);
    let (waiting, collected) = tryr!(state, s.for_device(device));
    json(200, &ExpenseList { waiting, collected })
}

fn collected_reply() -> Reply {
    error(
        409,
        "collected",
        "Your computer already collected this expense; change it there.",
    )
}

fn patch_expense(state: &State, device: &str, id: &str, body: &[u8], now: i64) -> Reply {
    let p: ExpensePatch = match parse(body) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let s = store!(state);
    let Some(sub) = tryr!(state, s.submission(device, id)) else {
        return error(404, "not_found", "No such expense.");
    };
    if sub.status == SubmissionStatus::Collected {
        return collected_reply();
    }
    let valid = match api::validate_expense(
        &p.fields,
        &p.receipt_ids,
        today(now),
        state.config.max_receipts_per_expense,
    ) {
        Ok(v) => v,
        Err(errors) => return field_errors(errors),
    };
    if !tryr!(
        state,
        receipts_available(&s, device, &valid.receipt_ids, Some(id))
    ) {
        return receipt_unavailable();
    }
    tryr!(
        state,
        s.update_submission(id, &valid.fields, &valid.receipt_ids)
    );
    match tryr!(state, s.submission(device, id)) {
        Some(updated) => json(200, &updated),
        None => error(404, "not_found", "No such expense."),
    }
}

fn delete_expense(state: &State, device: &str, id: &str) -> Reply {
    let s = store!(state);
    let Some(sub) = tryr!(state, s.submission(device, id)) else {
        return error(404, "not_found", "No such expense.");
    };
    if sub.status == SubmissionStatus::Collected {
        return collected_reply();
    }
    tryr!(state, s.delete_submission(id));
    json(200, &serde_json::json!({"deleted": true}))
}

// ----- trips (mobile-capture.md revision 11) -------------------------------------------

/// A refused trip: 400 with the reasons (mileage.md M12).
fn trip_refused(errors: Vec<FieldError>) -> Reply {
    let message = errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let mut e = ApiError::new("invalid", &message);
    e.error.fields = errors;
    json(400, &e)
}

fn no_trip() -> Reply {
    error(404, "trip_not_found", "No such trip.")
}

fn trip_done_reply() -> Reply {
    error(
        409,
        "collected",
        "Your computer already took this trip; change it there.",
    )
}

fn post_trip(state: &State, device: &str, body: &[u8], now: i64) -> Reply {
    let t: api::NewTrip = match parse(body) {
        Ok(t) => t,
        Err(r) => return r,
    };
    if !api::is_uuid(&t.client_id) {
        return trip_refused(vec![FieldError {
            field: "client_id".into(),
            message: "client_id must be a UUID.".into(),
        }]);
    }
    let s = store!(state);
    if let Some(existing) = tryr!(state, s.trip_by_client_id(device, &t.client_id)) {
        return json(200, &existing);
    }
    let valid = match api::validate_trip(&t.fields, today(now)) {
        Ok(v) => v,
        Err(errors) => return trip_refused(errors),
    };
    let trip = tryr!(state, s.add_trip(device, &t.client_id, &valid, now));
    json(201, &trip)
}

fn get_trips(state: &State, device: &str) -> Reply {
    let s = store!(state);
    let (waiting, done) = tryr!(state, s.trips_for_device(device));
    json(200, &TripList { waiting, done })
}

fn patch_trip(state: &State, device: &str, id: &str, body: &[u8], now: i64) -> Reply {
    let p: api::TripPatch = match parse(body) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let s = store!(state);
    let Some(trip) = tryr!(state, s.trip(device, id)) else {
        return no_trip();
    };
    if trip.status != TripStatus::Waiting {
        return trip_done_reply();
    }
    let valid = match api::validate_trip(&p.fields, today(now)) {
        Ok(v) => v,
        Err(errors) => return trip_refused(errors),
    };
    tryr!(state, s.update_trip(id, &valid));
    match tryr!(state, s.trip(device, id)) {
        Some(t) => json(200, &t),
        None => no_trip(),
    }
}

fn delete_trip(state: &State, device: &str, id: &str) -> Reply {
    let s = store!(state);
    let Some(trip) = tryr!(state, s.trip(device, id)) else {
        return no_trip();
    };
    if trip.status != TripStatus::Waiting {
        return trip_done_reply();
    }
    tryr!(state, s.delete_trip(id));
    json(200, &serde_json::json!({"deleted": true}))
}

fn ack_trip(state: &State, id: &str, body: &[u8], now: i64) -> Reply {
    let a: TripAck = if body.iter().all(u8::is_ascii_whitespace) {
        TripAck::default()
    } else {
        match parse(body) {
            Ok(a) => a,
            Err(r) => return r,
        }
    };
    let clean = |v: &Option<String>, max: usize| {
        v.as_deref()
            .and_then(|t| api::clean_text(t, max, "").ok().flatten())
    };
    let ack = TripAck {
        refused: a
            .refused
            .as_ref()
            .map(|_| clean(&a.refused, 300).unwrap_or_else(|| "Refused".into())),
        amount: clean(&a.amount, 30),
    };
    let s = store!(state);
    if tryr!(state, s.ack_trip(id, &ack, now)) {
        json(200, &serde_json::json!({"taken": ack.refused.is_none()}))
    } else {
        error(404, "not_found", "No such trip.")
    }
}

fn delete_receipt(state: &State, device: &str, id: &str) -> Reply {
    let s = store!(state);
    let Some(r) = tryr!(state, s.receipt(id)).filter(|r| r.device_id == device) else {
        return error(404, "not_found", "No such photo.");
    };
    if r.collected {
        return collected_reply();
    }
    if let Some(sub) = &r.submission_id {
        tryr!(state, s.detach_receipt(sub, id));
    }
    tryr!(state, s.delete_receipt(id));
    json(200, &serde_json::json!({"deleted": true}))
}

// ----- desktop API ---------------------------------------------------------------------

fn put_lists(state: &State, body: &[u8], now: i64) -> Reply {
    let l: Lists = match parse(body) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let clean = match api::validate_lists(&l) {
        Ok(c) => c,
        Err(m) => return error(422, "invalid", &m),
    };
    let bytes = tryr!(state, serde_json::to_vec(&clean).map_err(|e| e.to_string()));
    let s = store!(state);
    tryr!(state, s.put_lists(&clean.book_id, &bytes, now));
    json(200, &serde_json::json!({"published_at": now}))
}

fn inbox(state: &State) -> Reply {
    let s = store!(state);
    let items = tryr!(state, s.inbox());
    let trips = tryr!(state, s.trip_inbox());
    let lists_book_id = tryr!(state, s.lists()).map(|(b, _, _)| b);
    json(
        200,
        &Inbox {
            items,
            trips,
            lists_book_id,
        },
    )
}

fn desktop_receipt(state: &State, id: &str) -> Reply {
    let s = store!(state);
    match tryr!(state, s.receipt_bytes(id)) {
        Some((kind, bytes)) => Reply {
            status: 200,
            body: bytes,
            content_type: if kind == "png" {
                "image/png"
            } else {
                "image/jpeg"
            },
        },
        None => error(404, "not_found", "No such receipt."),
    }
}

fn ack(state: &State, id: &str, now: i64) -> Reply {
    let s = store!(state);
    if tryr!(state, s.ack(id, now)) {
        json(200, &serde_json::json!({"collected": true}))
    } else {
        error(404, "not_found", "No such submission.")
    }
}

fn pairing_code(state: &State, now: i64) -> Reply {
    let bytes = tryr!(state, store::random::<PAIRING_CODE_LEN>());
    let code = api::pairing_code(&bytes);
    let s = store!(state);
    tryr!(
        state,
        s.new_pairing(&store::token_hash(&code), now + PAIRING_TTL_SECS)
    );
    json(
        201,
        &PairingCode {
            code,
            expires_at: now + PAIRING_TTL_SECS,
            expires_in: PAIRING_TTL_SECS,
            client_url: state.config.client_url.clone(),
        },
    )
}

fn devices(state: &State) -> Reply {
    let s = store!(state);
    json(
        200,
        &api::Devices {
            devices: tryr!(state, s.devices()),
        },
    )
}

fn desktop_status(state: &State) -> Reply {
    let (waiting, last_activity) = {
        let s = store!(state);
        let (_, waiting, _) = tryr!(state, s.counts());
        (waiting, tryr!(state, s.last_activity()))
    };
    json(
        200,
        &api::DesktopStatus {
            waiting,
            last_activity,
            version: crate::VERSION.to_owned(),
            access_problem: state.access_problem.lock().ok().and_then(|p| p.clone()),
            wifi: state.wifi.as_ref().map(|_| wifi_state(state)),
        },
    )
}

/// `PUT /desktop/pin`: keep the Capture PIN's Argon2id hash (revision 10).
fn set_pin(state: &State, body: &[u8]) -> Reply {
    let p: api::SetPin = match parse(body) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let pin = match api::validate_pin(&p.pin) {
        Ok(pin) => zeroize::Zeroizing::new(pin),
        Err(m) => {
            return field_errors(vec![FieldError {
                field: "pin".into(),
                message: m,
            }]);
        }
    };
    let hash = tryr!(state, crate::pin::hash_with(&pin, state.pin_cost));
    let s = store!(state);
    tryr!(state, s.set_pin(&hash));
    json(200, &serde_json::json!({"pin_set": true}))
}

/// The Wi-Fi listener's public facts.
fn wifi_state(state: &State) -> api::WifiState {
    let pin_set = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.pin().ok().flatten())
        .is_some();
    let Some(w) = &state.wifi else {
        return api::WifiState {
            pin_set,
            ..api::WifiState::default()
        };
    };
    let info = w.info.read().map(|i| i.clone()).unwrap_or_default();
    api::WifiState {
        enabled: true,
        address: w.listen.target.ip.to_string(),
        name: w.listen.target.name.clone().unwrap_or_default(),
        https_port: w.listen.https_port,
        http_port: w.listen.http_port,
        fingerprint: info.fingerprint,
        ca_not_after: info.ca_not_after,
        cert_not_before: info.cert_not_before,
        cert_not_after: info.cert_not_after,
        pin_set,
        address_present: (state.address_present)(w.listen.target.ip),
        listening: w.listening.load(Ordering::SeqCst),
    }
}

fn revoke(state: &State, id: &str) -> Reply {
    let s = store!(state);
    if tryr!(state, s.revoke(id)) {
        json(200, &serde_json::json!({"revoked": true}))
    } else {
        error(404, "not_found", "No such phone.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(route("POST", "/api/v1/pair"), Route::Pair);
        assert_eq!(
            route("PATCH", &format!("/api/v1/expenses/{id}")),
            Route::PatchExpense(id.into())
        );
        assert_eq!(route("PATCH", "/api/v1/expenses/../x"), Route::NotFound);
        assert_eq!(
            route("POST", &format!("/api/v1/desktop/inbox/{id}/ack")),
            Route::DesktopAck(id.into())
        );
        assert_eq!(route("GET", "/other"), Route::Page("/other".into()));
        assert_eq!(route("POST", "/other"), Route::NotFound);
        assert_eq!(route("GET", "/api/v1/health"), Route::Health);
        assert_eq!(route("POST", "/api/v1/trips"), Route::PostTrip);
        assert_eq!(
            route("DELETE", &format!("/api/v1/trips/{id}")),
            Route::DeleteTrip(id.into())
        );
        assert!(route("POST", &format!("/api/v1/desktop/trips/{id}/ack")).desktop());
        assert!(wants_mileage("/api/v1/lists?mileage=1"));
        assert!(!wants_mileage("/api/v1/lists"));
        assert!(route("GET", "/api/v1/desktop/status").desktop());
        assert!(!route("GET", "/api/v1/lists").service_allowed());
        assert!(route("GET", "/api/v1/desktop/inbox").desktop());
        assert!(api::DESKTOP_PREFIX.starts_with(api::API_PREFIX));
    }
}
