//! The offline-first outbox (mobile-capture.md "Offline-first"): every submission
//! is saved locally first and sent when the phone is online; failures retry with
//! backoff, and each expense keeps its `client_id`, so a retry never duplicates.
//!
//! The outbox is a state machine with no IO: [`Outbox::next_action`] says what to
//! send next and [`Outbox::complete`] takes the answer. Persistence goes through the
//! [`Storage`] trait (IndexedDB in the browser, a map in tests).
//!
//! upgrade-resilience-1 M10: each stored item carries a format number `v`
//! ([`ITEM_VERSION`]). An item this version can't read (a higher `v`, or one it
//! can't parse) is kept in storage untouched and counted
//! ([`Outbox::unreadable`]); a later version that understands it sends it. M9: a
//! submission the server refuses because a photo is no longer there has its photos
//! uploaded afresh.

use base64::Engine;
use mk_capture_api::{ExpenseFields, ExpensePatch, NewExpense, NewTrip, TripFields, TripPatch};
use serde::{Deserialize, Serialize};

/// First retry delay; it doubles per failure up to [`MAX_BACKOFF_MS`].
pub const BASE_BACKOFF_MS: i64 = 5_000;
pub const MAX_BACKOFF_MS: i64 = 30 * 60 * 1000;
/// Sent items remembered on the phone (for the Sent list's details).
pub const SENT_KEPT: usize = 100;

/// Bytes stored as Base64 in JSON.
mod b64 {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let t = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(t)
            .map_err(serde::de::Error::custom)
    }
}

/// A photo waiting to be sent (already scaled and re-encoded as JPEG).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Photo {
    #[serde(with = "b64")]
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// A small RGBA thumbnail for the photo strip.
    #[serde(with = "b64", default)]
    pub thumb: Vec<u8>,
    #[serde(default)]
    pub thumb_w: u32,
    #[serde(default)]
    pub thumb_h: u32,
    /// Set once the server stored it (never uploaded twice).
    #[serde(default)]
    pub receipt_id: Option<String>,
}

/// Where an outbox item is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// "Save for later": kept on the phone, not sent.
    Draft,
    /// Waiting to be sent (or retried).
    Queued,
    /// The server refused it in a way a retry cannot fix; edit it to send again.
    NeedsAttention,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub client_id: String,
    /// The expense's fields (empty for a trip).
    pub fields: ExpenseFields,
    /// A trip instead of an expense (mobile-capture.md revision 11): sent to
    /// `/trips`, with no photos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trip: Option<TripFields>,
    #[serde(default)]
    pub photos: Vec<Photo>,
    pub state: State,
    /// An edit of a submission already on the server (sent with PATCH).
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    /// Unix milliseconds before which it is not retried.
    #[serde(default)]
    pub next_try_ms: i64,
    #[serde(default)]
    pub last_error: Option<String>,
    pub created_ms: i64,
    /// M9: times its photos were uploaded afresh because the server no longer had
    /// one (at most once before it is parked).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reuploads: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// A submission the phone sent (details for the Sent list).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sent {
    pub client_id: String,
    pub server_id: String,
    pub fields: ExpenseFields,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trip: Option<TripFields>,
    pub photos: usize,
    pub sent_ms: i64,
}

/// Local persistence (IndexedDB in the browser).
pub trait Storage {
    fn put(&mut self, key: &str, value: &str);
    fn delete(&mut self, key: &str);
}

const ITEM_PREFIX: &str = "outbox:";
/// The format number written into each stored item (`"v"`, M10). Items from before
/// it carry none and are read as version 1.
pub const ITEM_VERSION: u32 = 1;

/// The format number of a stored item.
#[derive(Deserialize)]
struct Versioned {
    #[serde(default = "first_version")]
    v: u32,
}

fn first_version() -> u32 {
    1
}

/// The stored item `json`, when this version can read it.
fn read_item(json: &str) -> Option<Item> {
    let Versioned { v } = serde_json::from_str(json).ok()?;
    if v > ITEM_VERSION {
        return None;
    }
    serde_json::from_str(json).ok()
}

/// The Outbox's note for items this version couldn't read (M10).
pub fn unreadable_note(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some(
            "1 saved item couldn't be read by this version. It's kept on this phone and sent once the app is updated."
                .to_owned(),
        ),
        n => Some(format!(
            "{n} saved items couldn't be read by this version. They're kept on this phone and sent once the app is updated."
        )),
    }
}
const SENT_KEY: &str = "sent";

/// What to send next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `POST /receipts` with this photo.
    Upload {
        client_id: String,
        photo: usize,
        jpeg: Vec<u8>,
    },
    /// `POST /expenses`.
    Submit { client_id: String, body: NewExpense },
    /// `PATCH /expenses/{server_id}`.
    Patch {
        client_id: String,
        server_id: String,
        body: ExpensePatch,
    },
    /// `POST /trips` (revision 11).
    SubmitTrip { client_id: String, body: NewTrip },
    /// `PATCH /trips/{server_id}`.
    PatchTrip {
        client_id: String,
        server_id: String,
        body: TripPatch,
    },
}

impl Action {
    pub fn client_id(&self) -> &str {
        match self {
            Action::Upload { client_id, .. }
            | Action::Submit { client_id, .. }
            | Action::Patch { client_id, .. }
            | Action::SubmitTrip { client_id, .. }
            | Action::PatchTrip { client_id, .. } => client_id,
        }
    }

    /// A trip's request.
    pub fn is_trip(&self) -> bool {
        matches!(self, Action::SubmitTrip { .. } | Action::PatchTrip { .. })
    }
}

/// The answer to an [`Action`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Uploaded {
        receipt_id: String,
    },
    Submitted {
        server_id: String,
    },
    /// A network failure, a 5xx, 429 or 507: try again later.
    Retry {
        message: String,
    },
    /// Refused for good (e.g. a field the server rejects): needs editing.
    Refused {
        message: String,
    },
    /// This phone's token was revoked (or never valid): stop until paired again.
    Unpaired,
    /// M9: "A photo is not available on the server any more" (a 422 on
    /// `receipt_ids`): upload the photos afresh when the phone still has them.
    PhotosGone {
        message: String,
    },
}

#[derive(Debug, Default)]
pub struct Outbox {
    pub items: Vec<Item>,
    pub sent: Vec<Sent>,
    /// The client id of the action being sent.
    in_flight: Option<String>,
    /// Set by [`Outcome::Unpaired`]; cleared on pairing.
    pub unpaired: bool,
    /// M10: stored items this version couldn't read (kept in storage).
    pub unreadable: usize,
}

/// Retry delay after `attempts` failures.
pub fn backoff_ms(attempts: u32) -> i64 {
    let shift = attempts.saturating_sub(1).min(20);
    BASE_BACKOFF_MS
        .saturating_mul(1i64 << shift)
        .min(MAX_BACKOFF_MS)
}

impl Outbox {
    /// Rebuild the outbox from stored key/value pairs (after a reload).
    pub fn load(entries: &[(String, String)]) -> Self {
        let mut o = Outbox::default();
        for (k, v) in entries {
            if k.starts_with(ITEM_PREFIX) {
                match read_item(v) {
                    Some(mut item) => {
                        // Nothing is in flight after a reload; retry promptly.
                        item.next_try_ms = 0;
                        o.items.push(item);
                    }
                    // M10: kept as it is, never deleted; counted for the Outbox.
                    None => o.unreadable = o.unreadable.saturating_add(1),
                }
            } else if k == SENT_KEY {
                o.sent = serde_json::from_str(v).unwrap_or_default();
            }
        }
        o.items.sort_by_key(|i| i.created_ms);
        o
    }

    fn save(&self, item: &Item, store: &mut dyn Storage) {
        let Ok(serde_json::Value::Object(mut map)) = serde_json::to_value(item) else {
            return;
        };
        map.insert("v".to_owned(), ITEM_VERSION.into());
        if let Ok(json) = serde_json::to_string(&map) {
            store.put(&format!("{ITEM_PREFIX}{}", item.client_id), &json);
        }
    }

    fn save_sent(&self, store: &mut dyn Storage) {
        if let Ok(json) = serde_json::to_string(&self.sent) {
            store.put(SENT_KEY, &json);
        }
    }

    /// Add or replace an item (saved first, then sent when online).
    pub fn put(&mut self, item: Item, store: &mut dyn Storage) {
        self.save(&item, store);
        match self
            .items
            .iter_mut()
            .find(|i| i.client_id == item.client_id)
        {
            Some(slot) => *slot = item,
            None => self.items.push(item),
        }
    }

    /// Remove an item before it was sent.
    pub fn remove(&mut self, client_id: &str, store: &mut dyn Storage) {
        self.items.retain(|i| i.client_id != client_id);
        store.delete(&format!("{ITEM_PREFIX}{client_id}"));
        if self.in_flight.as_deref() == Some(client_id) {
            self.in_flight = None;
        }
    }

    pub fn get(&self, client_id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.client_id == client_id)
    }

    pub fn busy(&self) -> bool {
        self.in_flight.is_some()
    }

    /// The next request to send, if any (one at a time; nothing while offline,
    /// unpaired or backing off).
    pub fn next_action(&mut self, now_ms: i64, online: bool) -> Option<Action> {
        if !online || self.unpaired || self.in_flight.is_some() {
            return None;
        }
        let item = self
            .items
            .iter()
            .find(|i| i.state == State::Queued && i.next_try_ms <= now_ms)?;
        let action = if let Some(trip) = &item.trip {
            match &item.server_id {
                Some(server_id) => Action::PatchTrip {
                    client_id: item.client_id.clone(),
                    server_id: server_id.clone(),
                    body: TripPatch {
                        fields: trip.clone(),
                    },
                },
                None => Action::SubmitTrip {
                    client_id: item.client_id.clone(),
                    body: NewTrip {
                        client_id: item.client_id.clone(),
                        fields: trip.clone(),
                    },
                },
            }
        } else if let Some((n, p)) = item
            .photos
            .iter()
            .enumerate()
            .find(|(_, p)| p.receipt_id.is_none())
        {
            Action::Upload {
                client_id: item.client_id.clone(),
                photo: n,
                jpeg: p.jpeg.clone(),
            }
        } else {
            let receipt_ids: Vec<String> = item
                .photos
                .iter()
                .filter_map(|p| p.receipt_id.clone())
                .collect();
            match &item.server_id {
                Some(server_id) => Action::Patch {
                    client_id: item.client_id.clone(),
                    server_id: server_id.clone(),
                    body: ExpensePatch {
                        fields: item.fields.clone(),
                        receipt_ids,
                    },
                },
                None => Action::Submit {
                    client_id: item.client_id.clone(),
                    body: NewExpense {
                        client_id: item.client_id.clone(),
                        fields: item.fields.clone(),
                        receipt_ids,
                    },
                },
            }
        };
        self.in_flight = Some(item.client_id.clone());
        Some(action)
    }

    /// Apply the answer to the action in flight.
    pub fn complete(
        &mut self,
        action: &Action,
        outcome: Outcome,
        now_ms: i64,
        store: &mut dyn Storage,
    ) {
        self.in_flight = None;
        let cid = action.client_id().to_owned();
        let Some(pos) = self.items.iter().position(|i| i.client_id == cid) else {
            return;
        };
        match outcome {
            Outcome::Uploaded { receipt_id } => {
                if let (Action::Upload { photo, .. }, Some(item)) =
                    (action, self.items.get_mut(pos))
                {
                    if let Some(p) = item.photos.get_mut(*photo) {
                        p.receipt_id = Some(receipt_id);
                    }
                    item.attempts = 0;
                    item.last_error = None;
                }
                if let Some(item) = self.items.get(pos) {
                    self.save(item, store);
                }
            }
            Outcome::Submitted { server_id } => {
                let item = self.items.remove(pos);
                store.delete(&format!("{ITEM_PREFIX}{}", item.client_id));
                self.sent.retain(|s| s.client_id != item.client_id);
                self.sent.insert(
                    0,
                    Sent {
                        client_id: item.client_id,
                        server_id,
                        fields: item.fields,
                        trip: item.trip,
                        photos: item.photos.len(),
                        sent_ms: now_ms,
                    },
                );
                self.sent.truncate(SENT_KEPT);
                self.save_sent(store);
            }
            Outcome::Retry { message } => {
                if let Some(item) = self.items.get_mut(pos) {
                    item.attempts = item.attempts.saturating_add(1);
                    item.next_try_ms = now_ms.saturating_add(backoff_ms(item.attempts));
                    item.last_error = Some(message);
                }
                if let Some(item) = self.items.get(pos) {
                    self.save(item, store);
                }
            }
            Outcome::Refused { message } => {
                if let Some(item) = self.items.get_mut(pos) {
                    item.state = State::NeedsAttention;
                    item.last_error = Some(message);
                }
                if let Some(item) = self.items.get(pos) {
                    self.save(item, store);
                }
            }
            Outcome::PhotosGone { message } => {
                if let Some(item) = self.items.get_mut(pos) {
                    let all_here =
                        !item.photos.is_empty() && item.photos.iter().all(|p| !p.jpeg.is_empty());
                    if all_here && item.reuploads == 0 {
                        // Sent again with every photo uploaded afresh.
                        for p in &mut item.photos {
                            p.receipt_id = None;
                        }
                        item.reuploads = 1;
                        item.next_try_ms = 0;
                        item.last_error = None;
                    } else {
                        item.state = State::NeedsAttention;
                        item.last_error = Some(if all_here {
                            message
                        } else {
                            mk_capture_api::msg::ADD_PHOTO_AGAIN.to_owned()
                        });
                    }
                }
                if let Some(item) = self.items.get(pos) {
                    self.save(item, store);
                }
            }
            Outcome::Unpaired => {
                self.unpaired = true;
                if let Some(item) = self.items.get_mut(pos) {
                    item.last_error = Some(mk_capture_api::msg::DISCONNECTED.into());
                }
            }
        }
    }

    /// The phone came online (or the 60-second timer fired): retry now.
    pub fn retry_now(&mut self) {
        for i in &mut self.items {
            i.next_try_ms = 0;
        }
    }

    /// Paired again: sending resumes.
    pub fn paired(&mut self) {
        self.unpaired = false;
        self.retry_now();
    }

    /// Forget a sent record (deleted on the server).
    pub fn forget_sent(&mut self, server_id: &str, store: &mut dyn Storage) {
        self.sent.retain(|s| s.server_id != server_id);
        self.save_sent(store);
    }
}

/// A new client id (a version 4 UUID from 16 random bytes).
pub fn uuid_from(random: [u8; 16]) -> String {
    let mut b = random;
    if let Some(x) = b.get_mut(6) {
        *x = (*x & 0x0F) | 0x40;
    }
    if let Some(x) = b.get_mut(8) {
        *x = (*x & 0x3F) | 0x80;
    }
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        h.get(0..8).unwrap_or(""),
        h.get(8..12).unwrap_or(""),
        h.get(12..16).unwrap_or(""),
        h.get(16..20).unwrap_or(""),
        h.get(20..32).unwrap_or("")
    )
}

/// Base64 of bytes (thumbnails in memory).
pub fn encode(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use std::collections::BTreeMap;

    use super::*;

    /// IndexedDB stand-in: survives a "reload" (a new outbox built from it).
    #[derive(Default, Clone)]
    struct Mem(BTreeMap<String, String>);

    impl Storage for Mem {
        fn put(&mut self, key: &str, value: &str) {
            self.0.insert(key.to_owned(), value.to_owned());
        }
        fn delete(&mut self, key: &str) {
            self.0.remove(key);
        }
    }

    impl Mem {
        fn entries(&self) -> Vec<(String, String)> {
            self.0.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        }
    }

    fn item(client: &str) -> Item {
        Item {
            client_id: client.into(),
            fields: ExpenseFields {
                date: Some("2026-10-23".into()),
                merchant: Some("Cafe".into()),
                amount: Some("4.50".into()),
                ..ExpenseFields::default()
            },
            trip: None,
            photos: vec![Photo {
                jpeg: vec![0xFF, 0xD8, 0xFF, 1, 2, 3],
                width: 2400,
                height: 1800,
                thumb: vec![1, 2, 3, 4],
                thumb_w: 1,
                thumb_h: 1,
                receipt_id: None,
            }],
            state: State::Queued,
            server_id: None,
            attempts: 0,
            next_try_ms: 0,
            last_error: None,
            created_ms: 1,
            reuploads: 0,
        }
    }

    /// mobile-capture.md AT15: the outbox keeps a submission across a reload and
    /// sends it when online; a failed send retries with backoff and the same
    /// `client_id`.
    #[test]
    fn at15_outbox_survives_reload_and_retries() {
        let cid = uuid_from([7; 16]);
        let mut store = Mem::default();
        {
            let mut o = Outbox::default();
            o.put(item(&cid), &mut store);
        }
        // Reload.
        let mut o = Outbox::load(&store.entries());
        assert_eq!(o.items.len(), 1);
        assert_eq!(o.items[0].client_id, cid);
        assert_eq!(o.items[0].photos[0].jpeg, vec![0xFF, 0xD8, 0xFF, 1, 2, 3]);
        // Offline: nothing is sent.
        assert_eq!(o.next_action(1000, false), None);
        // Online: the photo goes first; the upload fails and backs off.
        let a = o.next_action(1000, true).unwrap();
        assert!(matches!(a, Action::Upload { photo: 0, .. }));
        assert_eq!(o.next_action(1000, true), None, "one request at a time");
        o.complete(
            &a,
            Outcome::Retry {
                message: "offline".into(),
            },
            1000,
            &mut store,
        );
        assert_eq!(o.items[0].attempts, 1);
        assert_eq!(o.next_action(1000 + BASE_BACKOFF_MS - 1, true), None);
        let a = o.next_action(1000 + BASE_BACKOFF_MS, true).unwrap();
        assert!(matches!(a, Action::Upload { .. }));
        o.complete(
            &a,
            Outcome::Uploaded {
                receipt_id: "r1".into(),
            },
            7000,
            &mut store,
        );
        // The receipt id is saved: a reload now does not upload again.
        let mut o = Outbox::load(&store.entries());
        let a = o.next_action(7000, true).unwrap();
        let Action::Submit { client_id, body } = &a else {
            panic!("{a:?}");
        };
        assert_eq!(client_id, &cid);
        assert_eq!(body.client_id, cid);
        assert_eq!(body.receipt_ids, vec!["r1".to_owned()]);
        // The submit fails twice; each retry carries the same client id and waits
        // longer.
        o.complete(
            &a,
            Outcome::Retry {
                message: "HTTP 502".into(),
            },
            7000,
            &mut store,
        );
        assert_eq!(o.items[0].next_try_ms, 7000 + BASE_BACKOFF_MS);
        let a = o.next_action(7000 + BASE_BACKOFF_MS, true).unwrap();
        o.complete(
            &a,
            Outcome::Retry {
                message: "HTTP 502".into(),
            },
            20_000,
            &mut store,
        );
        assert_eq!(o.items[0].next_try_ms, 20_000 + 2 * BASE_BACKOFF_MS);
        let a = o.next_action(30_000, true).unwrap();
        assert!(matches!(&a, Action::Submit { body, .. } if body.client_id == cid));
        o.complete(
            &a,
            Outcome::Submitted {
                server_id: "s1".into(),
            },
            30_000,
            &mut store,
        );
        assert!(o.items.is_empty());
        assert_eq!(o.sent[0].server_id, "s1");
        // The sent record survives a reload; the outbox stays empty.
        let o = Outbox::load(&store.entries());
        assert!(o.items.is_empty());
        assert_eq!(o.sent.len(), 1);
    }

    /// Revision 11: a trip goes to `/trips` with its client id (no photos), and
    /// its sent record keeps the trip.
    #[test]
    fn trips_are_sent_as_trips() {
        let mut store = Mem::default();
        let mut o = Outbox::default();
        let cid = uuid_from([3; 16]);
        let mut t = item(&cid);
        t.photos.clear();
        t.fields = ExpenseFields::default();
        t.trip = Some(TripFields {
            date: "2026-11-05".into(),
            purpose_kind: "business".into(),
            miles: "12.4".into(),
            round_trip: true,
            ..TripFields::default()
        });
        o.put(t, &mut store);
        let o2 = Outbox::load(&store.entries());
        assert_eq!(
            o2.items[0].trip.as_ref().unwrap().miles,
            "12.4",
            "survives a reload"
        );
        let a = o.next_action(0, true).unwrap();
        assert!(a.is_trip());
        let Action::SubmitTrip { body, .. } = &a else {
            panic!("{a:?}");
        };
        assert_eq!(body.client_id, cid);
        o.complete(
            &a,
            Outcome::Submitted {
                server_id: "t1".into(),
            },
            5,
            &mut store,
        );
        assert!(o.items.is_empty());
        assert!(o.sent[0].trip.is_some());
    }

    #[test]
    fn drafts_unpaired_and_refusals() {
        let mut store = Mem::default();
        let mut o = Outbox::default();
        let mut d = item(&uuid_from([1; 16]));
        d.state = State::Draft;
        o.put(d, &mut store);
        assert_eq!(o.next_action(0, true), None, "drafts are not sent");
        let q = item(&uuid_from([2; 16]));
        o.put(q, &mut store);
        let a = o.next_action(0, true).unwrap();
        o.complete(&a, Outcome::Unpaired, 0, &mut store);
        assert!(o.unpaired);
        assert_eq!(o.next_action(0, true), None);
        assert_eq!(
            o.items[1].last_error.as_deref(),
            Some(mk_capture_api::msg::DISCONNECTED)
        );
        o.paired();
        let a = o.next_action(0, true).unwrap();
        o.complete(
            &a,
            Outcome::Refused {
                message: "no".into(),
            },
            0,
            &mut store,
        );
        assert_eq!(o.items[1].state, State::NeedsAttention);
        assert_eq!(o.next_action(0, true), None);
        assert_eq!(backoff_ms(1), BASE_BACKOFF_MS);
        assert_eq!(backoff_ms(40), MAX_BACKOFF_MS);
        assert!(mk_capture_api::is_uuid(&uuid_from([9; 16])));
    }

    fn gone() -> Outcome {
        Outcome::PhotosGone {
            message: "A photo is not available on the server any more; add it again.".into(),
        }
    }

    /// upgrade-resilience-1 UR-7 (M9): "A photo is not available on the server any
    /// more" clears the item's receipt ids and queues it again, so its photos are
    /// uploaded afresh and the expense sent with the new ids; a second time, or a
    /// photo the phone no longer has, parks it with "Add the photo again".
    #[test]
    fn ur7_photo_gone_requeues_with_photos_reuploaded() {
        let mut store = Mem::default();
        let mut o = Outbox::default();
        let cid = uuid_from([4; 16]);
        o.put(item(&cid), &mut store);
        let a = o.next_action(0, true).unwrap();
        o.complete(
            &a,
            Outcome::Uploaded {
                receipt_id: "r1".into(),
            },
            0,
            &mut store,
        );
        let a = o.next_action(0, true).unwrap();
        assert!(matches!(&a, Action::Submit { body, .. } if body.receipt_ids == ["r1"]));
        o.complete(&a, gone(), 10, &mut store);
        assert_eq!(o.items[0].state, State::Queued);
        assert_eq!(o.items[0].photos[0].receipt_id, None, "cleared");
        // Saved that way: a reload uploads again too.
        let mut o = Outbox::load(&store.entries());
        let a = o.next_action(10, true).unwrap();
        let Action::Upload { jpeg, .. } = &a else {
            panic!("{a:?}");
        };
        assert_eq!(
            jpeg,
            &vec![0xFF, 0xD8, 0xFF, 1, 2, 3],
            "the photo uploaded afresh"
        );
        o.complete(
            &a,
            Outcome::Uploaded {
                receipt_id: "r2".into(),
            },
            11,
            &mut store,
        );
        let a = o.next_action(11, true).unwrap();
        assert!(matches!(&a, Action::Submit { body, .. } if body.receipt_ids == ["r2"]));
        // Refused the same way again: parked with the server's words, not looping.
        o.complete(&a, gone(), 12, &mut store);
        assert_eq!(o.items[0].state, State::NeedsAttention);
        assert!(
            o.items[0]
                .last_error
                .as_deref()
                .unwrap()
                .starts_with("A photo")
        );
        assert_eq!(o.next_action(12, true), None);

        // An edit of a sent expense whose photo is only on the server: parked.
        let mut o = Outbox::default();
        let mut edit = item(&uuid_from([5; 16]));
        edit.server_id = Some("s1".into());
        edit.photos[0].jpeg.clear();
        edit.photos[0].receipt_id = Some("r9".into());
        o.put(edit, &mut store);
        let a = o.next_action(0, true).unwrap();
        assert!(matches!(a, Action::Patch { .. }));
        o.complete(&a, gone(), 1, &mut store);
        assert_eq!(o.items[0].state, State::NeedsAttention);
        assert_eq!(
            o.items[0].last_error.as_deref(),
            Some(mk_capture_api::msg::ADD_PHOTO_AGAIN)
        );
        assert_eq!(
            o.items[0].photos[0].receipt_id.as_deref(),
            Some("r9"),
            "kept"
        );
    }

    /// upgrade-resilience-1 UR-7 (M10): every stored item carries `v`; one this
    /// version can't read (a later `v`, or not readable at all) is kept untouched
    /// and counted; items from before `v` are read as version 1.
    #[test]
    fn ur7_unreadable_items_are_kept_and_counted() {
        let mut store = Mem::default();
        let mut o = Outbox::default();
        o.put(item("now"), &mut store);
        let saved: serde_json::Value =
            serde_json::from_str(store.0.get("outbox:now").unwrap()).unwrap();
        assert_eq!(saved["v"], ITEM_VERSION);
        // Before `v`: an item as an earlier version wrote it.
        let mut old: serde_json::Value = saved.clone();
        old.as_object_mut().unwrap().remove("v");
        old["client_id"] = "old".into();
        store.put("outbox:old", &old.to_string());
        // A later version's item, and one that can't be read at all.
        let mut later = saved.clone();
        later["v"] = 2.into();
        later["client_id"] = "later".into();
        let later = later.to_string();
        store.put("outbox:later", &later);
        store.put("outbox:broken", "{\"v\": 1, \"client_id\": 5}");
        let mut o = Outbox::load(&store.entries());
        assert_eq!(o.unreadable, 2);
        let mut ids: Vec<&str> = o.items.iter().map(|i| i.client_id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["now", "old"]);
        // Sending and saving the readable ones never touches the others.
        while let Some(a) = o.next_action(0, true) {
            let outcome = match &a {
                Action::Upload { .. } => Outcome::Uploaded {
                    receipt_id: "r".into(),
                },
                _ => Outcome::Submitted {
                    server_id: "s".into(),
                },
            };
            o.complete(&a, outcome, 0, &mut store);
        }
        assert!(o.items.is_empty());
        assert_eq!(store.0.get("outbox:later"), Some(&later));
        assert!(store.0.contains_key("outbox:broken"));
        assert_eq!(Outbox::load(&store.entries()).unreadable, 2);
        assert_eq!(unreadable_note(0), None);
        assert!(
            unreadable_note(1)
                .unwrap()
                .starts_with("1 saved item couldn't")
        );
        assert!(
            unreadable_note(3)
                .unwrap()
                .starts_with("3 saved items couldn't be read by this version")
        );
    }
}
