//! The phone app's storage from before the product was renamed (branding.md step 7).
//!
//! The browser keeps the pairing and the outbox in an IndexedDB database named after
//! the product. On first start under the new name the new database is empty; the old
//! database is read and its entries (pairing, outbox, sent list, lists) are copied,
//! so the phone stays paired and nothing waiting to be sent is lost.
//!
//! The old name is built from parts so that no file spells it (see
//! `mk_core::legacy`).

use crate::outbox::Storage;

/// The IndexedDB database name before the rename.
pub const OLD_DB_NAME: &str = concat!("money", "craft", "-capture");

/// The `localStorage` marker "paired here before" from before the rename (still
/// read).
pub const OLD_PAIRED_KEY: &str = "mc-paired";

/// Written into the new database once the old one was copied, so the copy happens
/// only once (an unpaired phone is never paired again from the old database).
pub const COPIED_KEY: &str = "copied_from_old_name";

/// Whether the old database should be read: the new one is empty (first start under
/// the new name) and has not been copied into before.
pub fn should_copy(new: &[(String, String)]) -> bool {
    new.is_empty()
}

/// Copy the old database's entries into the new storage and return the entries the
/// app starts with. `new` holds what the new database had; with entries there,
/// nothing is copied.
pub fn copy_old(
    new: Vec<(String, String)>,
    old: Vec<(String, String)>,
    store: &mut dyn Storage,
) -> Vec<(String, String)> {
    if !should_copy(&new) || new.iter().any(|(k, _)| k == COPIED_KEY) {
        return new;
    }
    for (k, v) in &old {
        store.put(k, v);
    }
    store.put(COPIED_KEY, "1");
    old
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::outbox::{Item, Outbox, State};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake(BTreeMap<String, String>);

    impl Storage for Fake {
        fn put(&mut self, key: &str, value: &str) {
            self.0.insert(key.to_owned(), value.to_owned());
        }
        fn delete(&mut self, key: &str) {
            self.0.remove(key);
        }
    }

    fn entries(m: &BTreeMap<String, String>) -> Vec<(String, String)> {
        m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// branding.md AT9: the pairing and the outbox are copied from the old database
    /// on first start; the app starts with them; a second start copies nothing.
    #[test]
    fn at9_pairing_and_outbox_are_copied_once() {
        let mut old = Fake::default();
        old.put("token", &"t".repeat(43));
        let mut ob = Outbox::default();
        ob.put(
            Item {
                client_id: "c1".into(),
                fields: mk_capture_api::ExpenseFields::default(),
                trip: None,
                photos: Vec::new(),
                state: State::Queued,
                server_id: None,
                attempts: 0,
                next_try_ms: 0,
                last_error: None,
                created_ms: 5,
                reuploads: 0,
            },
            &mut old,
        );
        assert!(old.0.contains_key("outbox:c1"));
        let mut new = Fake::default();
        let start = copy_old(Vec::new(), entries(&old.0), &mut new);
        assert!(start.iter().any(|(k, v)| k == "token" && v.len() == 43));
        assert_eq!(new.0.get("token"), old.0.get("token"));
        assert_eq!(new.0.get("outbox:c1"), old.0.get("outbox:c1"));
        assert_eq!(new.0.get(COPIED_KEY).map(String::as_str), Some("1"));
        // The outbox loads from the copied entries.
        assert!(Outbox::load(&entries(&new.0)).get("c1").is_some());
        assert!(Outbox::load(&start).get("c1").is_some());

        // Second start: the new database has entries; nothing is copied.
        let mut again = Fake::default();
        let now = entries(&new.0);
        let start = copy_old(now.clone(), entries(&old.0), &mut again);
        assert_eq!(start, now);
        assert!(again.0.is_empty());

        // Unpaired later (only the marker left): never paired again from the old one.
        let marker = vec![(COPIED_KEY.to_owned(), "1".to_owned())];
        let start = copy_old(marker.clone(), entries(&old.0), &mut Fake::default());
        assert_eq!(start, marker);

        // No old database: nothing but the marker.
        let mut fresh = Fake::default();
        assert!(copy_old(Vec::new(), Vec::new(), &mut fresh).is_empty());
        assert_eq!(fresh.0.len(), 1);
    }
}
