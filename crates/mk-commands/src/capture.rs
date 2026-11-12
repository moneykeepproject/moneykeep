//! Phone capture, desktop side (mobile-capture.md "Desktop app"): the capture
//! server's address and desktop token, pairing phones, and collecting phone
//! submissions into the book.
//!
//! Requests go through the injected [`crate::connect::Fetcher`] like bank feeds, and
//! never inside a book transaction (connectivity.md revision 2): a short read plans
//! the request, the request runs with no transaction open, and a short write applies
//! the answer. The desktop token is a secret kept per connectivity.md (inside an
//! encrypted book, else in the operating system's credential store).

use std::collections::{BTreeMap, HashSet};

use mk_capture_api::{self as api, Inbox, InboxItem, Lists, TagLists, TripInboxItem};
use mk_core::{AccountSubtype, AccountType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::accounts::Chart;
use crate::connect::{
    Connector, FetchError, HttpMethod, HttpRequest, HttpResponse, NetJob, NetOp, NetStep, SecretRef,
};
use crate::module_settings as ms;
use crate::{CommandError, CommandResult, Ctx, Registry, is_module_enabled};

pub(crate) const MODULE: &str = crate::expenses::MODULE;
pub(crate) const SERVER_KEY: &str = "capture.server";
pub(crate) const STATE_KEY: &str = "capture.state";
const COLLECTED_KEY: &str = "capture.collected";
/// Submission ids remembered as collected (a failed acknowledgement never makes a
/// second collect create duplicates).
const COLLECTED_KEPT: usize = 2000;
const MAX_JSON_ANSWER: usize = 4 * 1024 * 1024;
const MAX_RECEIPT_ANSWER: usize = 12 * 1024 * 1024;

const NOT_SET_UP: &str = "Phone capture is not set up: enter the server address and desktop token in Settings > Phone capture.";

/// Why a trip from the phone is refused: the book's mileage module is off
/// (mobile-capture.md revision 11). The phone shows it on the trip's Sent row.
const TRIPS_OFF: &str = "Mileage is turned off in this book. Turn it on in MoneyKeep (Settings > Features), then send the trip again.";

/// Commands that make requests.
pub(crate) fn is_network(name: &str) -> bool {
    matches!(
        name,
        "capture.test"
            | "capture.collect"
            | "capture.pair_code"
            | "capture.devices"
            | "capture.revoke_device"
            | "capture.publish_lists"
    )
}

pub(crate) fn register(r: &mut Registry) {
    r.add_in(
        MODULE,
        "capture.settings",
        "Phone capture settings: server address, whether a desktop token is stored, automatic collection",
        false,
        settings,
    );
    r.add_in(
        MODULE,
        "capture.set_server",
        "Set the phone capture server's address and desktop token (handles a secret; not offered over MCP)",
        true,
        set_server,
    );
    r.add_in(
        MODULE,
        "capture.test",
        "Check the connection to the phone capture server",
        false,
        network_only::<crate::builtin::NoArgs>,
    );
    r.add_in(
        MODULE,
        "capture.collect",
        "Bring expenses and receipts sent from paired phones into the book",
        true,
        network_only::<CollectArgs>,
    );
    r.add_in(
        MODULE,
        "capture.pair_code",
        "Create a one-time code for pairing a phone (not offered over MCP)",
        false,
        network_only::<crate::builtin::NoArgs>,
    );
    r.add_in(
        MODULE,
        "capture.devices",
        "Paired phones (name, paired, last seen)",
        false,
        network_only::<crate::builtin::NoArgs>,
    );
    r.add_in(
        MODULE,
        "capture.revoke_device",
        "Disconnect a paired phone",
        false,
        network_only::<RevokeArgs>,
    );
    r.add_in(
        MODULE,
        "capture.publish_lists",
        "Send the phone the category, paid-with, merchant and tag names (no balances or numbers)",
        false,
        network_only::<PublishArgs>,
    );
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidArgs(m.into())
}

fn rejected(m: impl Into<String>) -> CommandError {
    CommandError::Rejected(m.into())
}

fn no_network() -> CommandError {
    rejected("this MoneyKeep front end cannot reach the network; use the app")
}

/// Registered so the command is listed and gated; the registry runs it through the
/// network steps instead.
fn network_only<A>(_: &Ctx<'_>, _: A) -> CommandResult<Value> {
    Err(no_network())
}

// ----- configuration ---------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ServerConf {
    /// `https://host[:port]` or `http://host:port` (loopback or trusted LAN).
    pub url: String,
    #[serde(default)]
    pub trusted: bool,
    #[serde(default)]
    pub secret_ref: Option<SecretRef>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CaptureState {
    #[serde(default)]
    last_collect_at: Option<String>,
    #[serde(default)]
    last_count: usize,
    /// Why the last collect failed (cleared by the next one that works).
    #[serde(default)]
    last_collect_error: Option<String>,
    /// upgrade-resilience-1 U1.9: when that error first happened (Unix seconds).
    #[serde(default)]
    last_collect_error_at: Option<i64>,
    /// Which way the last collect reached the server (revision 16c):
    /// `this_computer`, `cloudflare` or `direct`.
    #[serde(default)]
    last_route: Option<String>,
}

pub(crate) fn conf(ctx: &Ctx<'_>) -> CommandResult<Option<ServerConf>> {
    Ok(ctx
        .tx
        .get_setting(MODULE, SERVER_KEY)?
        .and_then(|v| serde_json::from_value(v).ok()))
}

fn state(ctx: &Ctx<'_>) -> CommandResult<CaptureState> {
    Ok(ctx
        .tx
        .get_setting(MODULE, STATE_KEY)?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

fn collected_ids(ctx: &Ctx<'_>) -> CommandResult<Vec<String>> {
    Ok(ctx
        .tx
        .get_setting(MODULE, COLLECTED_KEY)?
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

fn is_private_host(host: &str) -> (bool, bool) {
    // (loopback, private LAN)
    let h = host.trim_start_matches('[').trim_end_matches(']');
    if h == "localhost" || h == "::1" || h.starts_with("127.") {
        return (true, true);
    }
    let octets: Vec<u8> = h.split('.').filter_map(|p| p.parse().ok()).collect();
    if octets.len() == 4 && h.split('.').count() == 4 {
        let lan = match (octets.first(), octets.get(1)) {
            (Some(10), _) => true,
            (Some(192), Some(168)) => true,
            (Some(172), Some(b)) => (16..=31).contains(b),
            (Some(169), Some(254)) => true,
            _ => false,
        };
        return (false, lan);
    }
    let lan = !h.contains('.')
        || h.ends_with(".local")
        || h.ends_with(".lan")
        || h.ends_with(".home.arpa");
    (false, lan)
}

/// Check a server address: returns (base URL, authority, plain http).
pub(crate) fn check_url(url: &str, trusted: bool) -> CommandResult<(String, String, bool)> {
    let u = url.trim().trim_end_matches('/');
    let (http, rest) = if let Some(r) = u.strip_prefix("https://") {
        (false, r)
    } else if let Some(r) = u.strip_prefix("http://") {
        (true, r)
    } else {
        return Err(invalid(
            "The server address starts with https:// (or http:// on this computer or a trusted home network).",
        ));
    };
    if rest.is_empty()
        || rest.contains(['/', '?', '#', '@', ' '])
        || rest.len() > 253
        || !rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'))
    {
        return Err(invalid(
            "The server address is just the scheme and host, such as https://capture-api.example.com.",
        ));
    }
    let authority = rest.to_ascii_lowercase();
    let host = if authority.starts_with('[') {
        authority
            .split(']')
            .next()
            .map(|h| format!("{h}]"))
            .unwrap_or_default()
    } else {
        authority
            .rsplit_once(':')
            .map_or(authority.clone(), |(h, _)| h.to_owned())
    };
    if http {
        let (loopback, lan) = is_private_host(&host);
        if !lan {
            return Err(invalid("Use https:// for a server on the internet."));
        }
        if !loopback && !trusted {
            return Err(invalid(
                "Plain http:// on the network needs \"This network is trusted\" ticked.",
            ));
        }
    }
    Ok((
        format!("{}://{authority}", if http { "http" } else { "https" }),
        authority,
        http,
    ))
}

/// Which way desktop requests reach the capture server (revision 16c).
pub(crate) const ROUTE_THIS_COMPUTER: &str = "this_computer";
pub(crate) const ROUTE_CLOUDFLARE: &str = "cloudflare";
pub(crate) const ROUTE_DIRECT: &str = "direct";

/// The status line's words for a route (revision 16c).
pub fn route_text(route: &str) -> Option<&'static str> {
    match route {
        ROUTE_THIS_COMPUTER => Some("Collected through this computer"),
        ROUTE_CLOUDFLARE => Some("Collected through Cloudflare"),
        _ => None,
    }
}

/// Cloudflare Access stopped a desktop request (revision 16c).
pub const ACCESS_REFUSED: &str = "Cloudflare Access refused MoneyKeep's service token — set up phone capture again or check the service token in Cloudflare.";
/// The capture server itself refused the desktop token (a JSON 401).
pub const TOKEN_REFUSED: &str = "The capture server refused the desktop token. Check the address and token in Settings > Phone capture.";

/// The server and desktop token, ready for requests.
#[derive(Clone)]
pub(crate) struct Endpoint {
    base: String,
    authority: String,
    http: bool,
    token: String,
    /// Cloudflare Access service-token headers (revision 16c), sent with every
    /// desktop request through the public address.
    access: Vec<(String, String)>,
    pub(crate) route: &'static str,
    /// Through Cloudflare, when this computer's loopback address can't be reached.
    pub(crate) fallback: Option<Box<Endpoint>>,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("base", &self.base)
            .field("route", &self.route)
            .field("token", &"(hidden)")
            .field(
                "access",
                &self.access.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Endpoint {
    pub(crate) fn req(
        &self,
        method: HttpMethod,
        path: &str,
        body: Option<Vec<u8>>,
        max: usize,
    ) -> HttpRequest {
        let mut headers = vec![("Authorization".to_owned(), format!("Bearer {}", self.token))];
        headers.extend(self.access.iter().cloned());
        if body.is_some() {
            headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
        }
        HttpRequest {
            method,
            url: format!("{}{}{path}", self.base, api::DESKTOP_PREFIX),
            basic: None,
            authority: self.authority.clone(),
            max_bytes: max,
            headers,
            body,
            allow_http: self.http,
        }
    }
}

pub(crate) fn endpoint(ctx: &Ctx<'_>, c: &Connector) -> CommandResult<Endpoint> {
    let conf = conf(ctx)?.ok_or_else(|| CommandError::Coded {
        code: "not_set_up",
        message: NOT_SET_UP.into(),
    })?;
    let (base, authority, http) = check_url(&conf.url, conf.trusted)?;
    let r = conf.secret_ref.ok_or_else(|| CommandError::Coded {
        code: "not_set_up",
        message: NOT_SET_UP.into(),
    })?;
    let token = crate::connect::get_secret(ctx, c, &r)?.ok_or_else(|| CommandError::Coded {
        code: "not_set_up",
        message: "The desktop token is not stored on this computer; enter it again in Settings > Phone capture.".into(),
    })?;
    // Revision 16c: Cloudflare Access lets a desktop request through the public
    // address only with the setup's service token (Cloudflare Access "Service
    // tokens": the `CF-Access-Client-Id` and `CF-Access-Client-Secret` headers,
    // <https://developers.cloudflare.com/cloudflare-one/identity/service-tokens/>).
    let setup = crate::capture_setup::load(ctx)?;
    let access: Vec<(String, String)> = setup
        .service_token
        .as_ref()
        .and_then(|s| {
            let secret = s
                .secret_ref
                .as_ref()
                .and_then(|r| crate::capture_setup::get_secret(ctx, r).ok().flatten())?;
            Some(vec![
                ("CF-Access-Client-Id".to_owned(), s.client_id.clone()),
                ("CF-Access-Client-Secret".to_owned(), secret),
            ])
        })
        .unwrap_or_default();
    let host = authority
        .rsplit_once(':')
        .filter(|(h, p)| !h.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        .map_or(authority.as_str(), |(h, _)| h);
    let (loopback, lan) = is_private_host(host);
    let public = !loopback && !lan;
    let through = Endpoint {
        base: base.clone(),
        authority: authority.clone(),
        http,
        token: token.clone(),
        access: if public { access } else { Vec::new() },
        route: if loopback {
            ROUTE_THIS_COMPUTER
        } else if public && setup.service_token.is_some() {
            ROUTE_CLOUDFLARE
        } else {
            ROUTE_DIRECT
        },
        fallback: None,
    };
    // The server runs on this computer: loopback, whatever address is saved for
    // display; Cloudflare only when loopback can't be reached.
    let here = setup.installed_server || crate::capture_wifi::load(ctx)?.installed;
    if here && public {
        let port = crate::capture_setup::SERVER_PORT;
        return Ok(Endpoint {
            base: format!("http://127.0.0.1:{port}"),
            authority: format!("127.0.0.1:{port}"),
            http: true,
            token,
            access: Vec::new(),
            route: ROUTE_THIS_COMPUTER,
            fallback: Some(Box::new(through)),
        });
    }
    Ok(through)
}

// ----- plain commands --------------------------------------------------------------------

fn settings(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let conf = conf(ctx)?;
    let st = state(ctx)?;
    let has_token = match (&conf, ctx.connector) {
        (
            Some(ServerConf {
                secret_ref: Some(r),
                ..
            }),
            Some(c),
        ) => crate::connect::get_secret(ctx, c, r)
            .ok()
            .flatten()
            .is_some(),
        (
            Some(ServerConf {
                secret_ref: Some(r),
                ..
            }),
            None,
        ) => r.store == "book" && ctx.tx.get_secret(&r.id)?.is_some(),
        _ => false,
    };
    Ok(json!({
        "configured": conf.is_some() && has_token,
        "url": conf.as_ref().map(|c| c.url.clone()),
        "trusted": conf.as_ref().is_some_and(|c| c.trusted),
        "has_token": has_token,
        "credential_store": conf.as_ref().and_then(|c| c.secret_ref.as_ref().map(|r| r.store.clone())),
        "auto_collect": ms::get(ctx, "capture.auto_collect")?.as_bool().unwrap_or(true),
        "collect_minutes": ms::get(ctx, "capture.collect_minutes")?.as_i64().unwrap_or(15),
        "last_collect_at": st.last_collect_at,
        "last_count": st.last_count,
        "last_collect_error": st.last_collect_error,
        "last_route": st.last_route,
        "last_route_text": st.last_route.as_deref().and_then(route_text),
        "setup": crate::capture_setup::summary(ctx)?,
        "wifi": crate::capture_wifi::summary(ctx)?,
    }))
}

/// Why the last collect failed, if it did (the status light's third check).
pub(crate) fn collect_error(ctx: &Ctx<'_>) -> CommandResult<Option<(String, Option<i64>)>> {
    let st = state(ctx)?;
    Ok(st.last_collect_error.map(|e| (e, st.last_collect_error_at)))
}

/// Keep the server's address and desktop token (guided setup, mobile-capture.md
/// revision 2 B5): the token goes to the book's secrets when the book is encrypted,
/// else to the operating system's credential store.
pub(crate) fn store_server(
    ctx: &Ctx<'_>,
    url: &str,
    trusted: bool,
    token: &str,
) -> CommandResult<()> {
    let (base, _, _) = check_url(url, trusted)?;
    if !api::is_token_shape(token) {
        return Err(rejected(
            "the capture server returned a desktop token of the wrong shape",
        ));
    }
    let book = ctx.tx.info()?.id;
    let sid = format!("{book}:capture-desktop");
    let home = if ctx.encrypted {
        "book"
    } else {
        match ctx.connector {
            Some(c) => crate::connect::secret_home(ctx, c)?,
            None => "book",
        }
    };
    if home == "book" {
        ctx.tx.put_secret(&sid, token)?;
    } else if let Some(c) = ctx.connector {
        c.secrets
            .put(&sid, token)
            .map_err(|e| rejected(format!("the credential store refused: {e}")))?;
    }
    let new = ServerConf {
        url: base,
        trusted,
        secret_ref: Some(SecretRef {
            store: home.to_owned(),
            id: sid,
        }),
    };
    ctx.save_setting(
        MODULE,
        SERVER_KEY,
        Some(&serde_json::to_value(&new).map_err(|e| CommandError::Integrity(e.to_string()))?),
    )
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetServerArgs {
    /// `https://host` (or `http://127.0.0.1:8787`, or a LAN address with `trusted`).
    /// Empty removes the phone capture settings and the stored token.
    url: String,
    /// The desktop token printed by `moneykeep-server init` (kept when omitted).
    #[serde(default)]
    token: Option<String>,
    /// Plain http:// on this network is acceptable ("This network is trusted").
    #[serde(default)]
    trusted: bool,
}

fn set_server(ctx: &Ctx<'_>, a: SetServerArgs) -> CommandResult<Value> {
    *ctx.audit_args.borrow_mut() = Some(json!({
        "url": a.url.trim(), "trusted": a.trusted,
        "token": a.token.as_ref().map(|_| "(hidden)"),
    }));
    let book = ctx.tx.info()?.id;
    let sid = format!("{book}:capture-desktop");
    let old = conf(ctx)?;
    if a.url.trim().is_empty() {
        if let Some(r) = old.and_then(|o| o.secret_ref) {
            if r.store == "book" {
                ctx.tx.delete_secret(&r.id)?;
            } else if let Some(c) = ctx.connector {
                let _ = c.secrets.delete(&r.id);
            }
        }
        ctx.save_setting(MODULE, SERVER_KEY, None)?;
        return Ok(json!({"configured": false}));
    }
    let (base, _, _) = check_url(&a.url, a.trusted)?;
    let token = a.token.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let secret_ref = match token {
        Some(t) => {
            if !api::is_token_shape(t) {
                return Err(invalid(
                    "The desktop token is the 43-character text printed by moneykeep-server init.",
                ));
            }
            let home = if ctx.encrypted {
                "book"
            } else {
                match ctx.connector {
                    Some(c) => crate::connect::secret_home(ctx, c)?,
                    None => {
                        return Err(CommandError::Coded {
                            code: "no_secure_storage",
                            message: "The desktop token can't be saved securely here: protect the book with a password (Settings > Security) or use the desktop app.".into(),
                        });
                    }
                }
            };
            if home == "book" {
                ctx.tx.put_secret(&sid, t)?;
            } else if let Some(c) = ctx.connector {
                c.secrets
                    .put(&sid, t)
                    .map_err(|e| rejected(format!("the credential store refused: {e}")))?;
            }
            Some(SecretRef {
                store: home.to_owned(),
                id: sid,
            })
        }
        None => match old.and_then(|o| o.secret_ref) {
            Some(r) => Some(r),
            None => return Err(invalid("Enter the desktop token.")),
        },
    };
    let new = ServerConf {
        url: base.clone(),
        trusted: a.trusted,
        secret_ref,
    };
    ctx.save_setting(
        MODULE,
        SERVER_KEY,
        Some(&serde_json::to_value(&new).map_err(|e| CommandError::Integrity(e.to_string()))?),
    )?;
    Ok(json!({"configured": true, "url": base, "trusted": a.trusted}))
}

// ----- lists -------------------------------------------------------------------------------

/// The lists document: names only (mobile-capture.md AT13).
pub(crate) fn lists_doc(ctx: &Ctx<'_>) -> CommandResult<Lists> {
    let chart = Chart::load(ctx)?;
    let usable = |a: &&mk_core::Account| !a.placeholder && !a.archived;
    let mut categories: Vec<String> = chart
        .accounts
        .iter()
        .filter(usable)
        .filter(|a| a.account_type == AccountType::Expense)
        .map(|a| chart.full_name(a.id))
        .collect();
    categories.sort_by_key(|n| n.to_lowercase());
    let mut paid_with: Vec<String> = chart
        .accounts
        .iter()
        .filter(usable)
        .filter(|a| {
            matches!(
                a.subtype,
                AccountSubtype::Bank | AccountSubtype::Cash | AccountSubtype::CreditCard
            )
        })
        .map(|a| chart.full_name(a.id))
        .collect();
    paid_with.sort_by_key(|n| n.to_lowercase());
    paid_with.push(crate::expenses::PERSONAL_LABEL.to_owned());
    let mut tags = TagLists::default();
    for (dim, out) in [
        ("class", &mut tags.class),
        ("location", &mut tags.location),
        ("fund", &mut tags.fund),
    ] {
        if is_module_enabled(ctx.tx, crate::dimensions::module_of(dim))? {
            *out = crate::dimensions::records(ctx, dim)?
                .into_iter()
                .filter(|r| r.active)
                .map(|r| r.name)
                .collect();
        }
    }
    tags.project = crate::projects::open_names(ctx)?;
    let info = ctx.tx.info()?;
    let mut l = Lists {
        book_id: info.id,
        book_name: info.name,
        base_currency: info.base_currency,
        categories,
        paid_with,
        merchants: crate::expenses::recent_merchants(ctx, api::MAX_RECENT_MERCHANTS)?,
        tags,
        published_at: None,
        ..Lists::default()
    };
    l.categories.truncate(api::MAX_LIST_ITEMS);
    l.paid_with.truncate(api::MAX_LIST_ITEMS);
    add_mileage_lists(ctx, &mut l)?;
    Ok(l)
}

/// The mileage parts of the lists (mobile-capture.md revision 11): active vehicles,
/// saved trips, purpose kinds, the rate table, whether trips can be reimbursable
/// and the default purpose. Nothing when the module is off, so the document stays
/// as an older server knows it.
fn add_mileage_lists(ctx: &Ctx<'_>, l: &mut Lists) -> CommandResult<()> {
    if !is_module_enabled(ctx.tx, crate::mileage::MODULE)? {
        return Ok(());
    }
    let v = crate::mileage::phone_lists(ctx)?;
    let text = |x: &Value, k: &str| x.get(k).and_then(Value::as_str).map(str::to_owned);
    let arr = |k: &str| {
        v.get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    l.mileage_enabled = v.get("mileage_enabled").and_then(Value::as_bool) == Some(true);
    l.reimbursable_enabled = v.get("reimbursable_enabled").and_then(Value::as_bool) == Some(true);
    l.default_purpose = text(&v, "default_purpose").unwrap_or_default();
    l.purpose_kinds = arr("purpose_kinds")
        .iter()
        .map(|k| api::PurposeKind {
            kind: text(k, "kind").unwrap_or_default(),
            label: text(k, "label").unwrap_or_default(),
        })
        .collect();
    l.vehicles = arr("vehicles")
        .iter()
        .map(|x| api::ListVehicle {
            id: text(x, "id").unwrap_or_default(),
            name: text(x, "name").unwrap_or_default(),
        })
        .collect();
    l.saved_trips = arr("saved_trips")
        .iter()
        .map(|x| api::ListSavedTrip {
            id: text(x, "id").unwrap_or_default(),
            name: text(x, "name").unwrap_or_default(),
            from_place: text(x, "from_place"),
            to_place: text(x, "to_place"),
            miles: text(x, "miles").unwrap_or_default(),
            round_trip: x.get("round_trip").and_then(Value::as_bool) == Some(true),
            purpose_kind: text(x, "purpose_kind").unwrap_or_default(),
            purpose: text(x, "purpose"),
            vehicle: text(x, "vehicle"),
        })
        .collect();
    l.mileage_rates = arr("mileage_rates")
        .iter()
        .map(|x| api::ListRate {
            kind: text(x, "kind").unwrap_or_default(),
            from: text(x, "from").unwrap_or_default(),
            cents: text(x, "cents").unwrap_or_default(),
        })
        .collect();
    l.vehicles.truncate(api::MAX_LIST_ITEMS);
    l.saved_trips.truncate(api::MAX_SAVED_TRIPS);
    l.mileage_rates.truncate(api::MAX_LIST_ITEMS);
    Ok(())
}

/// The `PUT /lists` body, and the same without the mileage parts for a server
/// from before mileage, which refuses fields it does not know (400).
fn lists_bodies(l: &Lists) -> CommandResult<(Vec<u8>, Option<Vec<u8>>)> {
    let full = serde_json::to_vec(l).map_err(|e| CommandError::Integrity(e.to_string()))?;
    let plain = if l.has_mileage() {
        Some(
            serde_json::to_vec(&l.without_mileage())
                .map_err(|e| CommandError::Integrity(e.to_string()))?,
        )
    } else {
        None
    };
    Ok((full, plain))
}

fn lists_hash(l: &Lists) -> String {
    let bytes = serde_json::to_vec(l).unwrap_or_default();
    crate::import::hex(&Sha256::digest(&bytes))
}

// ----- collecting ----------------------------------------------------------------------------

/// The network steps of phone capture.
#[derive(Debug, Clone)]
pub(crate) enum Op {
    Test,
    PairCode,
    Devices,
    Revoke,
    Publish {
        hash: String,
        /// The lists without mileage, sent again when the server refuses the
        /// full document (a server from before revision 11).
        fallback: Option<(Endpoint, Vec<u8>)>,
    },
    Inbox {
        ep: Endpoint,
    },
    Receipts {
        ep: Endpoint,
        items: Vec<InboxItem>,
        trips: Vec<TripInboxItem>,
        already: Vec<String>,
        mismatch: bool,
    },
    Ack {
        result: Value,
        acks: usize,
        fallback: Option<(Endpoint, Vec<u8>)>,
    },
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CollectArgs {
    /// An automatic run: does nothing when automatic collection is off or phone
    /// capture is not set up.
    #[serde(default)]
    scheduled: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RevokeArgs {
    /// The phone's id (from `capture.devices`).
    device: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PublishArgs {
    /// Skip the request when the lists still have this hash (from an earlier publish).
    #[serde(default)]
    known_hash: Option<String>,
}

fn answer(r: Option<&Result<HttpResponse, FetchError>>, ok: &[u16]) -> CommandResult<Vec<u8>> {
    match r {
        None => Err(rejected("The capture server did not answer.")),
        // Access answers an unauthenticated request with a redirect to its sign-in
        // page on `<team>.cloudflareaccess.com`, which the fetcher refuses.
        Some(Err(FetchError::Refused(m))) if m.contains("cloudflareaccess.com") => {
            Err(CommandError::Coded {
                code: "access_refused",
                message: ACCESS_REFUSED.into(),
            })
        }
        Some(Err(e)) => Err(CommandError::Coded {
            code: "unreachable",
            message: format!("The capture server could not be reached: {e}"),
        }),
        Some(Ok(resp)) if ok.contains(&resp.status) => Ok(resp.body.clone()),
        Some(Ok(resp)) => Err(refusal(resp)),
    }
}

fn refusal(resp: &HttpResponse) -> CommandError {
    let server_msg = serde_json::from_slice::<api::ApiError>(&resp.body)
        .ok()
        .map(|e| mk_core::text::clean(&e.error.message, 300).text);
    match resp.status {
        // Revision 16c: the server's own refusal is JSON; Access answers with its
        // page (HTML) or a redirect.
        300..=399 | 401 | 403 if server_msg.is_none() => CommandError::Coded {
            code: "access_refused",
            message: ACCESS_REFUSED.into(),
        },
        401 => CommandError::Coded {
            code: "bad_token",
            message: TOKEN_REFUSED.into(),
        },
        404 => CommandError::NotFound(server_msg.unwrap_or_else(|| "not found".into())),
        s => CommandError::Coded {
            code: "server_refused",
            message: format!(
                "The capture server answered HTTP {s}{}",
                server_msg.map(|m| format!(": {m}")).unwrap_or_default()
            ),
        },
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> CommandResult<T> {
    serde_json::from_slice(body).map_err(|e| {
        rejected(format!(
            "The capture server's answer was not understood: {e}"
        ))
    })
}

impl Registry {
    /// A short read transaction (module `expenses` must be on).
    fn capture_read<T>(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        f: impl FnOnce(&Ctx<'_>) -> CommandResult<T>,
    ) -> CommandResult<T> {
        let encrypted = store.is_encrypted();
        let connector = self.connector.as_ref();
        store.read(|tx| {
            if !is_module_enabled(tx, MODULE)? {
                return Err(CommandError::ModuleDisabled {
                    command: "capture".into(),
                    module: MODULE.into(),
                });
            }
            let mut ctx = Ctx::new(tx, actor);
            ctx.connector = connector;
            ctx.encrypted = encrypted;
            f(&ctx)
        })
    }

    /// Remember why collecting failed (or that it works again); written only when it
    /// changes, so a server that stays down does not fill the history.
    /// Which way the last collect reached the server (revision 16c).
    fn note_route(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        route: &str,
    ) -> CommandResult<()> {
        let current = self.capture_read(store, actor, |ctx| Ok(state(ctx)?.last_route))?;
        if current.as_deref() == Some(route) {
            return Ok(());
        }
        let spec = self
            .spec("capture.collect")
            .ok_or_else(|| CommandError::UnknownCommand("capture.collect".into()))?;
        let route = route.to_owned();
        self.execute(
            store,
            actor,
            spec,
            json!({"route": route}),
            None,
            &|ctx, _| {
                let mut st = state(ctx)?;
                st.last_route = Some(route.clone());
                ctx.save_setting(
                    MODULE,
                    STATE_KEY,
                    Some(
                        &serde_json::to_value(&st)
                            .map_err(|e| CommandError::Integrity(e.to_string()))?,
                    ),
                )?;
                Ok(Value::Null)
            },
        )?;
        Ok(())
    }

    fn note_collect_error(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        error: Option<String>,
    ) -> CommandResult<()> {
        let current = self.capture_read(store, actor, |ctx| Ok(state(ctx)?.last_collect_error))?;
        if current == error {
            return Ok(());
        }
        let spec = self
            .spec("capture.collect")
            .ok_or_else(|| CommandError::UnknownCommand("capture.collect".into()))?;
        let audit = json!({"collect_error": error.is_some()});
        let at = error.as_ref().and(self.connector.as_ref()).map(|c| c.now());
        self.execute(store, actor, spec, audit, None, &|ctx, _| {
            let mut st = state(ctx)?;
            st.last_collect_error.clone_from(&error);
            st.last_collect_error_at = at;
            ctx.save_setting(
                MODULE,
                STATE_KEY,
                Some(
                    &serde_json::to_value(&st)
                        .map_err(|e| CommandError::Integrity(e.to_string()))?,
                ),
            )?;
            Ok(Value::Null)
        })?;
        Ok(())
    }

    /// Start a phone capture network command.
    pub(crate) fn capture_start(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        name: &str,
        args: Value,
    ) -> CommandResult<NetStep> {
        // Module gate first (a book without expenses gets the usual refusal).
        self.capture_read(store, actor, |_| Ok(()))?;
        let c = self.connector.as_ref().ok_or_else(no_network)?;
        let job = |op: Op, reqs: Vec<HttpRequest>| {
            Ok(NetStep::Fetch(NetJob::many(
                NetOp::Capture(op),
                reqs,
                c.fetcher.clone(),
            )))
        };
        match name {
            "capture.collect" => {
                let a: CollectArgs = crate::parse_args(args)?;
                let ep = self.capture_read(store, actor, |ctx| {
                    if a.scheduled {
                        let auto = ms::get(ctx, "capture.auto_collect")?
                            .as_bool()
                            .unwrap_or(true);
                        if !auto {
                            return Ok(Err("automatic collection is off"));
                        }
                        if conf(ctx)?.is_none() {
                            return Ok(Err("phone capture is not set up"));
                        }
                    }
                    endpoint(ctx, c).map(Ok)
                })?;
                match ep {
                    Ok(ep) => {
                        let req = ep.req(HttpMethod::Get, "/inbox", None, MAX_JSON_ANSWER);
                        job(Op::Inbox { ep }, vec![req])
                    }
                    Err(reason) => Ok(NetStep::Done(json!({"skipped": reason}))),
                }
            }
            "capture.test" => {
                let ep = self.capture_read(store, actor, |ctx| endpoint(ctx, c))?;
                job(
                    Op::Test,
                    vec![ep.req(HttpMethod::Get, "/devices", None, MAX_JSON_ANSWER)],
                )
            }
            "capture.pair_code" => {
                let ep = self.capture_read(store, actor, |ctx| endpoint(ctx, c))?;
                job(
                    Op::PairCode,
                    vec![ep.req(HttpMethod::Post, "/pairing-codes", None, 64 * 1024)],
                )
            }
            "capture.devices" => {
                let ep = self.capture_read(store, actor, |ctx| endpoint(ctx, c))?;
                job(
                    Op::Devices,
                    vec![ep.req(HttpMethod::Get, "/devices", None, MAX_JSON_ANSWER)],
                )
            }
            "capture.revoke_device" => {
                let a: RevokeArgs = crate::parse_args(args)?;
                let id = a.device.trim().to_owned();
                if !api::is_receipt_id(&id) {
                    return Err(invalid("device is a phone id from capture.devices"));
                }
                let ep = self.capture_read(store, actor, |ctx| endpoint(ctx, c))?;
                job(
                    Op::Revoke,
                    vec![ep.req(
                        HttpMethod::Delete,
                        &format!("/devices/{id}"),
                        None,
                        64 * 1024,
                    )],
                )
            }
            "capture.publish_lists" => {
                let a: PublishArgs = crate::parse_args(args)?;
                let (ep, lists) = self
                    .capture_read(store, actor, |ctx| Ok((endpoint(ctx, c)?, lists_doc(ctx)?)))?;
                let hash = lists_hash(&lists);
                if a.known_hash.as_deref() == Some(hash.as_str()) {
                    return Ok(NetStep::Done(
                        json!({"published": false, "unchanged": true, "hash": hash}),
                    ));
                }
                let (body, plain) = lists_bodies(&lists)?;
                let req = ep.req(HttpMethod::Put, "/lists", Some(body), 64 * 1024);
                job(
                    Op::Publish {
                        hash,
                        fallback: plain.map(|p| (ep, p)),
                    },
                    vec![req],
                )
            }
            other => Err(CommandError::UnknownCommand(other.to_owned())),
        }
    }

    /// Continue a phone capture command with its answers.
    pub(crate) fn capture_resume(
        &self,
        store: &mut mk_store::Store,
        actor: &crate::Actor,
        c: &Connector,
        op: Op,
        responses: Vec<Result<HttpResponse, FetchError>>,
    ) -> CommandResult<NetStep> {
        let first = responses.first();
        match op {
            Op::Test => {
                let body = answer(first, &[200])?;
                let d: api::Devices = parse(&body)?;
                Ok(NetStep::Done(json!({
                    "ok": true,
                    "devices": d.devices.len(),
                    "message": "Connected to the capture server.",
                })))
            }
            Op::PairCode => {
                let body = answer(first, &[200, 201])?;
                let p: api::PairingCode = parse(&body)?;
                Ok(NetStep::Done(json!({
                    "code": p.code, "expires_at": p.expires_at, "expires_in": p.expires_in,
                    "client_url": p.client_url,
                })))
            }
            Op::Devices => {
                let body = answer(first, &[200])?;
                let d: api::Devices = parse(&body)?;
                Ok(NetStep::Done(json!({"devices": d.devices})))
            }
            Op::Revoke => {
                answer(first, &[200])?;
                Ok(NetStep::Done(json!({"revoked": true})))
            }
            Op::Publish { hash, fallback } => {
                if let (Some((ep, plain)), Some(Ok(resp))) = (fallback, first)
                    && resp.status == 400
                {
                    // An older server: the lists without mileage.
                    let req = ep.req(HttpMethod::Put, "/lists", Some(plain), 64 * 1024);
                    return Ok(NetStep::Fetch(NetJob::many(
                        NetOp::Capture(Op::Publish {
                            hash,
                            fallback: None,
                        }),
                        vec![req],
                        c.fetcher.clone(),
                    )));
                }
                answer(first, &[200])?;
                Ok(NetStep::Done(json!({"published": true, "hash": hash})))
            }
            Op::Inbox { ep } => {
                // Revision 16c: loopback can't be reached: through Cloudflare.
                if matches!(first, Some(Err(FetchError::Network(_))))
                    && let Some(next) = ep.fallback.clone()
                {
                    let req = next.req(HttpMethod::Get, "/inbox", None, MAX_JSON_ANSWER);
                    return Ok(NetStep::Fetch(NetJob::many(
                        NetOp::Capture(Op::Inbox { ep: *next }),
                        vec![req],
                        c.fetcher.clone(),
                    )));
                }
                let body = match answer(first, &[200]) {
                    Ok(b) => b,
                    Err(e) => {
                        self.note_collect_error(store, actor, Some(e.to_string()))?;
                        return Err(e);
                    }
                };
                self.note_collect_error(store, actor, None)?;
                self.note_route(store, actor, ep.route)?;
                let inbox: Inbox = parse(&body)?;
                let (done, book_id) = self.capture_read(store, actor, |ctx| {
                    Ok((collected_ids(ctx)?, ctx.tx.info()?.id))
                })?;
                let done: HashSet<String> = done.into_iter().collect();
                let mismatch = inbox
                    .lists_book_id
                    .as_deref()
                    .is_some_and(|b| !b.is_empty() && b != book_id);
                let trips = inbox.trips;
                let (already, items): (Vec<InboxItem>, Vec<InboxItem>) =
                    inbox.items.into_iter().partition(|i| done.contains(&i.id));
                let already: Vec<String> = already.into_iter().map(|i| i.id).collect();
                let reqs: Vec<HttpRequest> = items
                    .iter()
                    .flat_map(|i| i.receipts.iter())
                    .filter(|r| api::is_receipt_id(&r.id))
                    .map(|r| {
                        ep.req(
                            HttpMethod::Get,
                            &format!("/receipts/{}", r.id),
                            None,
                            MAX_RECEIPT_ANSWER,
                        )
                    })
                    .collect();
                let op = Op::Receipts {
                    ep,
                    items,
                    trips,
                    already,
                    mismatch,
                };
                if reqs.is_empty() {
                    return self.capture_resume(store, actor, c, op, Vec::new());
                }
                Ok(NetStep::Fetch(NetJob::many(
                    NetOp::Capture(op),
                    reqs,
                    c.fetcher.clone(),
                )))
            }
            Op::Receipts {
                ep,
                items,
                trips,
                already,
                mismatch,
            } => {
                // Pair each receipt with its answer, in request order.
                let mut answers = responses.into_iter();
                let mut bytes: BTreeMap<String, Vec<u8>> = BTreeMap::new();
                let mut failed: Vec<String> = Vec::new();
                for item in &items {
                    for r in item.receipts.iter().filter(|r| api::is_receipt_id(&r.id)) {
                        match answers.next() {
                            Some(Ok(resp)) if resp.status == 200 => {
                                bytes.insert(r.id.clone(), resp.body);
                            }
                            _ => failed.push(item.id.clone()),
                        }
                    }
                }
                let ready: Vec<InboxItem> = items
                    .into_iter()
                    .filter(|i| !failed.contains(&i.id))
                    .collect();
                let today = mk_store::today_utc()?;
                let mut result = if ready.is_empty() && trips.is_empty() {
                    json!({"expenses": 0, "ready": 0, "drafts": 0, "inbox_receipts": 0,
                           "trips": 0, "trip_drafts": 0, "trips_refused": 0,
                           "notes": [], "collected": [], "trip_acks": []})
                } else {
                    let spec = self
                        .spec("capture.collect")
                        .ok_or_else(|| CommandError::UnknownCommand("capture.collect".into()))?;
                    let audit = json!({"submissions": ready.len(), "trips": trips.len()});
                    self.execute(store, actor, spec, audit, None, &|ctx, _| {
                        apply_collect(ctx, &ready, &trips, &bytes, today)
                    })?
                };
                if let Some(o) = result.as_object_mut() {
                    o.insert("route".into(), json!(ep.route));
                    o.insert("route_text".into(), json!(route_text(ep.route)));
                }
                let lists = self.capture_read(store, actor, lists_doc)?;
                let mut acks: Vec<String> = result
                    .get("collected")
                    .and_then(Value::as_array)
                    .map(|v| {
                        v.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                acks.extend(already);
                let mut reqs: Vec<HttpRequest> = acks
                    .iter()
                    .map(|id| {
                        ep.req(
                            HttpMethod::Post,
                            &format!("/inbox/{id}/ack"),
                            None,
                            64 * 1024,
                        )
                    })
                    .collect();
                // Trips: taken (with the book's amount) or refused (with why).
                let trip_acks: Vec<Value> = result
                    .get_mut("trip_acks")
                    .map(Value::take)
                    .and_then(|v| v.as_array().cloned())
                    .unwrap_or_default();
                for a in &trip_acks {
                    let Some(id) = a.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    if !api::is_receipt_id(id) {
                        continue;
                    }
                    let body = api::TripAck {
                        refused: a.get("refused").and_then(Value::as_str).map(str::to_owned),
                        amount: a.get("amount").and_then(Value::as_str).map(str::to_owned),
                    };
                    reqs.push(ep.req(
                        HttpMethod::Post,
                        &format!("/trips/{id}/ack"),
                        serde_json::to_vec(&body).ok(),
                        64 * 1024,
                    ));
                }
                let (full, plain) = lists_bodies(&lists)?;
                reqs.push(ep.req(HttpMethod::Put, "/lists", Some(full), 64 * 1024));
                if let Some(o) = result.as_object_mut() {
                    o.insert("failed".into(), json!(failed.len()));
                    o.insert("other_book".into(), json!(mismatch));
                    if mismatch {
                        o.insert(
                            "warning".into(),
                            json!("The phone's lists came from another book; names were matched against this book."),
                        );
                    }
                    o.insert("lists_hash".into(), json!(lists_hash(&lists)));
                }
                let n = reqs.len().saturating_sub(1);
                Ok(NetStep::Fetch(NetJob::many(
                    NetOp::Capture(Op::Ack {
                        result,
                        acks: n,
                        fallback: plain.map(|p| (ep, p)),
                    }),
                    reqs,
                    c.fetcher.clone(),
                )))
            }
            Op::Ack {
                mut result,
                acks,
                fallback,
            } => {
                let ok = |r: &Result<HttpResponse, FetchError>| matches!(r, Ok(resp) if resp.status == 200);
                let acked = responses.iter().take(acks).filter(|r| ok(r)).count();
                let published = responses.get(acks).is_some_and(ok);
                let refused_lists =
                    matches!(responses.get(acks), Some(Ok(resp)) if resp.status == 400);
                if let Some(o) = result.as_object_mut() {
                    // A second round (the lists again) keeps the first counts.
                    if acks > 0 || !o.contains_key("acknowledged") {
                        o.insert("acknowledged".into(), json!(acked));
                        o.insert("ack_failed".into(), json!(acks - acked.min(acks)));
                    }
                    o.insert("lists_published".into(), json!(published));
                }
                if let Some((ep, plain)) = fallback.filter(|_| refused_lists) {
                    // A server from before mileage: the lists without mileage.
                    let req = ep.req(HttpMethod::Put, "/lists", Some(plain), 64 * 1024);
                    return Ok(NetStep::Fetch(NetJob::many(
                        NetOp::Capture(Op::Ack {
                            result,
                            acks: 0,
                            fallback: None,
                        }),
                        vec![req],
                        c.fetcher.clone(),
                    )));
                }
                Ok(NetStep::Done(result))
            }
        }
    }
}

/// The book write of a collect: one expense (or inbox receipts) per submission.
fn apply_collect(
    ctx: &Ctx<'_>,
    items: &[InboxItem],
    trips: &[TripInboxItem],
    bytes: &BTreeMap<String, Vec<u8>>,
    today: mk_core::Date,
) -> CommandResult<Value> {
    let mut done = collected_ids(ctx)?;
    let (mut expenses, mut ready, mut drafts, mut inbox) = (0usize, 0usize, 0usize, 0usize);
    let mut notes: Vec<String> = Vec::new();
    let mut collected: Vec<String> = Vec::new();
    for item in items {
        if done.contains(&item.id) {
            collected.push(item.id.clone());
            continue;
        }
        let date = item.fields.date.clone().unwrap_or_default();
        let expense = if item.fields.has_expense_data() {
            let e = crate::expenses::create_from_phone(ctx, &item.fields, today)?;
            expenses += 1;
            if e.status == "ready" {
                ready += 1;
            } else {
                drafts += 1;
            }
            notes.extend(e.notes);
            Some(e.id)
        } else {
            None
        };
        for (n, r) in item.receipts.iter().enumerate() {
            let Some(b) = bytes.get(&r.id) else { continue };
            let ext = if r.kind == "png" { "png" } else { "jpg" };
            let name = format!("phone-{date}-{}.{ext}", n + 1);
            crate::expenses::add_phone_receipt(ctx, b, &name, expense.as_deref())?;
            if expense.is_none() {
                inbox += 1;
            }
        }
        done.push(item.id.clone());
        collected.push(item.id.clone());
    }
    // Trips (mobile-capture.md revision 11): each becomes a trip with source phone
    // (a resend of its client id makes nothing new); with the module off it is
    // refused with the reason.
    let (mut trip_n, mut trip_drafts, mut trips_refused) = (0usize, 0usize, 0usize);
    let mut trip_acks: Vec<Value> = Vec::new();
    let mileage_on = is_module_enabled(ctx.tx, crate::mileage::MODULE)?;
    for t in trips {
        if !mileage_on {
            trips_refused += 1;
            trip_acks.push(json!({"id": t.id, "refused": TRIPS_OFF}));
            continue;
        }
        let f = &t.fields;
        let some = |s: &str| (!s.trim().is_empty()).then(|| s.to_owned());
        let p = crate::mileage::PhoneTrip {
            client_id: t.client_id.clone(),
            date: some(&f.date),
            vehicle_id: some(&f.vehicle_id),
            purpose_kind: some(&f.purpose_kind),
            purpose: some(&f.purpose),
            from_place: some(&f.from_place),
            to_place: some(&f.to_place),
            miles: some(&f.miles),
            odometer_start: f.odometer_start.clone(),
            odometer_end: f.odometer_end.clone(),
            round_trip: f.round_trip,
            reimbursable: f.reimbursable,
            saved_trip_id: f.saved_trip_id.clone(),
            tags: f.tags.clone(),
        };
        let r = crate::mileage::create_from_phone(ctx, &p, today)?;
        if !r.already_taken {
            trip_n += 1;
            if r.status == "draft" {
                trip_drafts += 1;
            }
            notes.extend(r.notes);
        }
        let amount = crate::mileage::load_trip(ctx, &r.id)?.amount.round_dp(2);
        trip_acks.push(json!({"id": t.id, "amount": format!("{amount:.2}")}));
    }
    if done.len() > COLLECTED_KEPT {
        let cut = done.len() - COLLECTED_KEPT;
        done.drain(..cut);
    }
    ctx.save_setting(MODULE, COLLECTED_KEY, Some(&json!(done)))?;
    let st = CaptureState {
        last_collect_at: Some(mk_store::now_rfc3339()?),
        last_count: collected.len() + trip_n,
        last_collect_error: None,
        last_collect_error_at: None,
        last_route: state(ctx)?.last_route,
    };
    ctx.save_setting(
        MODULE,
        STATE_KEY,
        Some(&serde_json::to_value(&st).map_err(|e| CommandError::Integrity(e.to_string()))?),
    )?;
    Ok(json!({
        "expenses": expenses, "ready": ready, "drafts": drafts, "inbox_receipts": inbox,
        "trips": trip_n, "trip_drafts": trip_drafts, "trips_refused": trips_refused,
        "notes": notes, "collected": collected, "trip_acks": trip_acks,
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn server_addresses() {
        assert_eq!(
            check_url("https://Capture-API.example.com/", false).unwrap(),
            (
                "https://capture-api.example.com".into(),
                "capture-api.example.com".into(),
                false
            )
        );
        assert!(check_url("http://127.0.0.1:8787", false).unwrap().2);
        assert!(check_url("http://192.168.1.20:8787", false).is_err());
        assert!(check_url("http://192.168.1.20:8787", true).is_ok());
        assert!(check_url("http://example.com", true).is_err());
        assert!(check_url("https://u:p@example.com", false).is_err());
        assert!(check_url("https://example.com/path", false).is_err());
        assert!(check_url("ftp://example.com", false).is_err());
    }
}
