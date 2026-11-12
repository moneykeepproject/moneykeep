use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{CoreError, CoreResult};

/// A typed identifier. UUID v7 (time-ordered) so ids sort by creation time.
///
/// The type parameter keeps account ids and transaction ids from being mixed up.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id<T> {
    uuid: Uuid,
    #[serde(skip)]
    _kind: PhantomData<fn() -> T>,
}

impl<T> Id<T> {
    pub fn new() -> Self {
        Self::from_uuid(Uuid::now_v7())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self {
            uuid,
            _kind: PhantomData,
        }
    }

    pub fn uuid(&self) -> Uuid {
        self.uuid
    }
}

impl<T> Default for Id<T> {
    fn default() -> Self {
        Self::new()
    }
}

// Manual impls so `T` itself need not implement these traits.
impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Id<T> {}
impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
    }
}
impl<T> Eq for Id<T> {}
impl<T> std::hash::Hash for Id<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.uuid.hash(state);
    }
}
impl<T> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.uuid.cmp(&other.uuid)
    }
}
impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.uuid)
    }
}
impl<T> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.uuid.fmt(f)
    }
}
impl<T> FromStr for Id<T> {
    type Err = CoreError;
    fn from_str(s: &str) -> CoreResult<Self> {
        Uuid::parse_str(s)
            .map(Self::from_uuid)
            .map_err(|_| CoreError::InvalidId(s.to_owned()))
    }
}

impl<T> JsonSchema for Id<T> {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "Id".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "format": "uuid" })
    }
}

/// Marker types for typed ids.
pub mod kind {
    #[derive(Debug)]
    pub enum Book {}
    #[derive(Debug)]
    pub enum Account {}
    #[derive(Debug)]
    pub enum Transaction {}
    #[derive(Debug)]
    pub enum Split {}
}

pub type BookId = Id<kind::Book>;
pub type AccountId = Id<kind::Account>;
pub type TransactionId = Id<kind::Transaction>;
/// A split's own id (release-prep.md §4): kept by the store across edits of its
/// transaction, so a link to a split survives reordering its lines.
pub type SplitId = Id<kind::Split>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_ordered() {
        let a = AccountId::new();
        let b = AccountId::new();
        assert_ne!(a, b);
        assert!(a < b, "v7 ids sort by creation time");
    }

    #[test]
    fn parse_round_trip() {
        let a = BookId::new();
        let parsed: BookId = a.to_string().parse().unwrap();
        assert_eq!(a, parsed);
        assert!("not-a-uuid".parse::<BookId>().is_err());
    }
}
