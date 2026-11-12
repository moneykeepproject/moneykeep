//! mileage.md M11 and M13 (desktop part), mobile-capture.md revision 11: trips sent
//! from a phone to the real capture server on 127.0.0.1 are taken into the book by
//! `capture.collect` through this crate's fetcher; the lists the desktop publishes
//! carry the active vehicles, the saved trips and the rates. No internet.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::sync::Arc;

use mk_commands::Session;
use mk_commands::connect::{Connector, MemorySecrets};
use mk_connect::HttpsFetcher;
use moneykeep_server::testing::{LocalServer, call};
use serde_json::{Value, json};

struct T {
    s: Session,
    server: LocalServer,
    _book: tempfile::TempDir,
    _data: tempfile::TempDir,
}

fn setup() -> T {
    let book = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let server = LocalServer::start(data.path()).unwrap();
    let mut s = Session::create(
        &book.path().join("b.moneykeep"),
        "Mileage Books",
        "USD",
        "app",
    )
    .unwrap();
    s.set_connector(Connector {
        fetcher: Arc::new(HttpsFetcher::trusting(Vec::new())),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: None,
        host: None,
    });
    for m in ["expenses", "mileage"] {
        s.invoke("module.enable", json!({"module": m})).unwrap();
    }
    s.invoke(
        "capture.set_server",
        json!({"url": server.url(), "token": server.desktop_token}),
    )
    .unwrap();
    T {
        s,
        server,
        _book: book,
        _data: data,
    }
}

fn today() -> String {
    mk_capture_api::format_date(mk_capture_api::today_from_unix(
        moneykeep_server::Clock::default().now(),
        0,
    ))
}

fn trip(client: &str, vehicle: &str) -> Value {
    json!({"client_id": client, "date": today(), "vehicle_id": vehicle,
        "purpose_kind": "business", "purpose": "Client visit", "from_place": "Office",
        "to_place": "Client", "miles": "12.4", "round_trip": true})
}

/// A phone's request with its device token.
fn phone(t: &T, token: &str, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    let b = body.map(Value::to_string);
    let (s, out) = call(
        &t.server.url(),
        method,
        &format!("/api/v1{path}"),
        &[
            ("Authorization", &format!("Bearer {token}")),
            ("Content-Type", "application/json"),
        ],
        b.as_deref().map(str::as_bytes),
    )
    .unwrap();
    (s, serde_json::from_slice(&out).unwrap_or(Value::Null))
}

fn attention(t: &mut T) -> Vec<String> {
    let v =
        t.s.invoke(
            "dashboard.block_data",
            json!({"dashboard": "home", "block": "attention"}),
        )
        .unwrap();
    v["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|i| i["label"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// M11 (desktop part): the trip sent from the phone is taken as a trip with source
/// phone and status logged, the phone sees it taken with the book's amount, and the
/// same client_id sent again (a second phone, so the server keeps it) makes no
/// copy. An unknown vehicle makes a draft with "Choose a vehicle" and Home
/// Attention says "1 trip from your phone".
#[test]
fn m11_desktop_takes_trips_from_the_phone() {
    let mut t = setup();
    let car =
        t.s.invoke("vehicle.create", json!({"name": "2019 Honda CR-V"}))
            .unwrap();
    let car = car["id"].as_str().unwrap().to_owned();
    let ph = t.server.pair_phone("Owner's phone").unwrap();
    let cid = "8d2f1b6c-3e4a-4b5c-9d6e-7f8091a2b3c4";
    let (s, sent) = phone(&t, &ph, "POST", "/trips", Some(&trip(cid, &car)));
    assert_eq!(s, 201, "{sent}");

    let out = t.s.invoke("capture.collect", json!({})).unwrap();
    assert_eq!(out["trips"], 1, "{out}");
    assert_eq!(out["trip_drafts"], 0, "{out}");
    assert_eq!(out["acknowledged"], 1, "{out}");
    assert_eq!(out["lists_published"], true, "{out}");
    let list = t.s.invoke("trip.list", json!({})).unwrap();
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 1);
    let tr = &list[0];
    assert_eq!(tr["source"], "phone");
    assert_eq!(tr["status"], "logged");
    assert_eq!(tr["miles"], "24.8");
    assert_eq!(tr["route"], "Office \u{2192} Client \u{2192} Office");
    assert_eq!(tr["client_id"], cid);
    let amount = tr["amount"].as_str().unwrap().to_owned();
    // The phone sees it taken, with the book's amount.
    let (_, mine) = phone(&t, &ph, "GET", "/trips", None);
    assert_eq!(mine["done"][0]["status"], "taken", "{mine}");
    assert_eq!(mine["done"][0]["amount"], amount.as_str());
    assert!(attention(&mut t).iter().all(|i| !i.contains("trip")));

    // The same client_id again (another phone, so the server stores it): no copy.
    let ph2 = t.server.pair_phone("Second phone").unwrap();
    let (s, _) = phone(&t, &ph2, "POST", "/trips", Some(&trip(cid, &car)));
    assert_eq!(s, 201);
    let out = t.s.invoke("capture.collect", json!({})).unwrap();
    assert_eq!(out["trips"], 0, "{out}");
    assert_eq!(out["acknowledged"], 1, "{out}");
    assert_eq!(
        t.s.invoke("trip.list", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // An unknown vehicle: a draft that needs a choice.
    let (s, _) = phone(
        &t,
        &ph,
        "POST",
        "/trips",
        Some(&trip("9d2f1b6c-3e4a-4b5c-9d6e-7f8091a2b3c4", "no-such-car")),
    );
    assert_eq!(s, 201);
    let out = t.s.invoke("capture.collect", json!({})).unwrap();
    assert_eq!(out["trip_drafts"], 1, "{out}");
    let drafts = t.s.invoke("trip.list", json!({"status": "draft"})).unwrap();
    assert_eq!(drafts[0]["notes"][0], "Choose a vehicle");
    assert!(
        attention(&mut t).contains(&"1 trip from your phone".to_owned()),
        "{:?}",
        attention(&mut t)
    );

    // Mileage off in the book: the trip is refused and the phone says why.
    t.s.invoke("module.disable", json!({"module": "mileage"}))
        .unwrap();
    let (_, sent) = phone(
        &t,
        &ph,
        "POST",
        "/trips",
        Some(&trip("ad2f1b6c-3e4a-4b5c-9d6e-7f8091a2b3c4", &car)),
    );
    let out = t.s.invoke("capture.collect", json!({})).unwrap();
    assert_eq!(out["trips_refused"], 1, "{out}");
    let (_, mine) = phone(&t, &ph, "GET", "/trips", None);
    let refused = mine["done"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == sent["id"])
        .unwrap()
        .clone();
    assert_eq!(refused["status"], "refused");
    assert!(
        refused["reason"]
            .as_str()
            .unwrap()
            .starts_with("Mileage is turned off in this book."),
        "{refused}"
    );
}

/// M13 (desktop part): the lists the phone receives hold only active vehicles and
/// the saved trips, with the rate table, the default purpose and whether trips can
/// be reimbursable; with mileage off they have none of it.
#[test]
fn m13_lists_hold_active_vehicles_and_saved_trips() {
    let mut t = setup();
    let car =
        t.s.invoke("vehicle.create", json!({"name": "2019 Honda CR-V"}))
            .unwrap();
    let car = car["id"].as_str().unwrap().to_owned();
    let old =
        t.s.invoke("vehicle.create", json!({"name": "Old truck"}))
            .unwrap();
    t.s.invoke(
        "vehicle.update",
        json!({"vehicle": old["id"], "active": false}),
    )
    .unwrap();
    t.s.invoke(
        "saved_trip.create",
        json!({"name": "Office to client", "from_place": "Office", "to_place": "Client",
               "miles": "12.4", "round_trip": true, "purpose_kind": "business",
               "purpose": "Client visit", "vehicle": car}),
    )
    .unwrap();
    let out = t.s.invoke("capture.publish_lists", json!({})).unwrap();
    assert_eq!(out["published"], true, "{out}");
    let ph = t.server.pair_phone("Owner's phone").unwrap();
    let (s, l) = phone(&t, &ph, "GET", "/lists?mileage=1", None);
    assert_eq!(s, 200);
    let l: mk_capture_api::Lists = serde_json::from_value(l).unwrap();
    assert!(l.mileage_enabled && l.reimbursable_enabled);
    assert_eq!(l.default_purpose, "business");
    let names: Vec<&str> = l.vehicles.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["2019 Honda CR-V"], "active vehicles only");
    assert_eq!(l.saved_trips.len(), 1);
    let st = &l.saved_trips[0];
    assert_eq!(
        (st.miles.as_str(), st.round_trip, st.vehicle.as_deref()),
        ("12.4", true, Some(car.as_str()))
    );
    assert!(
        l.mileage_rates
            .iter()
            .any(|r| r.kind == "business" && r.from == "2026-07-01" && r.cents == "76")
    );
    let kinds: Vec<&str> = l.purpose_kinds.iter().map(|k| k.kind.as_str()).collect();
    assert_eq!(kinds, ["business", "medical", "charity", "personal"]);
    // A phone that does not ask gets the lists without mileage.
    let (_, plain) = phone(&t, &ph, "GET", "/lists", None);
    assert!(plain.get("vehicles").is_none(), "{plain}");
    // Mileage off: none of it.
    t.s.invoke("module.disable", json!({"module": "mileage"}))
        .unwrap();
    t.s.invoke("capture.publish_lists", json!({})).unwrap();
    let (_, l) = phone(&t, &ph, "GET", "/lists?mileage=1", None);
    let l: mk_capture_api::Lists = serde_json::from_value(l).unwrap();
    assert!(!l.has_mileage());
}

/// mileage.md revision 2 (desktop part): with classes and funds on, the lists the
/// phone receives with the mileage lists (`GET /lists?mileage=1`) carry their names;
/// a trip sent with a class and a fund arrives tagged with them (a draft when it
/// still needs its vehicle); a tag the book does not know is dropped with a note.
/// (The revision's example names a project; this book has no project entries, so the
/// test uses the dimensions it has.)
#[test]
fn mileage_r2_trips_arrive_tagged() {
    let mut t = setup();
    for m in ["classes", "funds"] {
        t.s.invoke("module.enable", json!({"module": m})).unwrap();
    }
    t.s.invoke("class.create", json!({"name": "Roof repair"}))
        .unwrap();
    t.s.invoke(
        "fund.create",
        json!({"name": "Building", "restriction": "with_donor_restrictions"}),
    )
    .unwrap();
    let car =
        t.s.invoke("vehicle.create", json!({"name": "2019 Honda CR-V"}))
            .unwrap();
    let car = car["id"].as_str().unwrap().to_owned();
    t.s.invoke("capture.publish_lists", json!({})).unwrap();
    let ph = t.server.pair_phone("Owner's phone").unwrap();
    let (s, l) = phone(&t, &ph, "GET", "/lists?mileage=1", None);
    assert_eq!(s, 200);
    let l: mk_capture_api::Lists = serde_json::from_value(l).unwrap();
    assert!(l.mileage_enabled);
    assert_eq!(l.tags.class, ["Roof repair"]);
    assert!(l.tags.fund.contains(&"Building".to_owned()), "{:?}", l.tags);
    assert!(l.tags.location.is_empty(), "locations are off");

    // Sent with a class and a fund, but no vehicle: a draft tagged with both.
    let mut body = trip("bd2f1b6c-3e4a-4b5c-9d6e-7f8091a2b3c4", "");
    body["tags"] = json!({"class": "Roof repair", "fund": "Building"});
    let (s, sent) = phone(&t, &ph, "POST", "/trips", Some(&body));
    assert_eq!(s, 201, "{sent}");
    assert_eq!(sent["tags"]["class"], "Roof repair", "{sent}");
    let out = t.s.invoke("capture.collect", json!({})).unwrap();
    assert_eq!(out["trips"], 1, "{out}");
    assert_eq!(out["trip_drafts"], 1, "{out}");
    let drafts = t.s.invoke("trip.list", json!({"status": "draft"})).unwrap();
    let id = drafts[0]["id"].as_str().unwrap().to_owned();
    let tr = t.s.invoke("trip.get", json!({"trip": id})).unwrap();
    assert_eq!(tr["tag_names"]["class"], "Roof repair", "{tr}");
    assert_eq!(tr["tag_names"]["fund"], "Building", "{tr}");
    assert!(tr["tag_names"]["location"].is_null());

    // With its vehicle it is logged, still tagged; an unknown class is dropped with
    // a note.
    let mut body = trip("cd2f1b6c-3e4a-4b5c-9d6e-7f8091a2b3c4", &car);
    body["tags"] = json!({"class": "No such class", "fund": "Building"});
    let (s, _) = phone(&t, &ph, "POST", "/trips", Some(&body));
    assert_eq!(s, 201);
    t.s.invoke("capture.collect", json!({})).unwrap();
    let logged =
        t.s.invoke("trip.list", json!({"status": "logged"}))
            .unwrap();
    let id = logged[0]["id"].as_str().unwrap().to_owned();
    let tr = t.s.invoke("trip.get", json!({"trip": id})).unwrap();
    assert_eq!(tr["status"], "logged", "{tr}");
    assert_eq!(tr["tag_names"]["fund"], "Building", "{tr}");
    assert!(tr["tag_names"]["class"].is_null(), "{tr}");
    assert!(
        tr["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n == "Class from phone not found: No such class"),
        "{tr}"
    );
}
