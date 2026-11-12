//! Schedule recurrence types (spec: scheduled-transactions.md). The date arithmetic
//! lives in `mk-ledger`.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use crate::Date;

/// How often a schedule repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Frequency {
    Once,
    Daily,
    Weekly,
    SemiMonthly,
    Monthly,
    Yearly,
}

/// A day of the month (1–31) or the month's last day. Also used for the week of the
/// month (1–4 or last).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MonthDay {
    Day(u8),
    Last,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum MonthDayRepr {
    Num(u8),
    Text(String),
}

impl Serialize for MonthDay {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            MonthDay::Day(d) => MonthDayRepr::Num(*d),
            MonthDay::Last => MonthDayRepr::Text("last".into()),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for MonthDay {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match MonthDayRepr::deserialize(d)? {
            MonthDayRepr::Num(n) => Ok(MonthDay::Day(n)),
            MonthDayRepr::Text(t) if t == "last" => Ok(MonthDay::Last),
            MonthDayRepr::Text(t) => Err(serde::de::Error::custom(format!(
                "expected a number or \"last\", got {t:?}"
            ))),
        }
    }
}

impl JsonSchema for MonthDay {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "MonthDay".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "oneOf": [
                {"type": "integer", "minimum": 1, "maximum": 31},
                {"const": "last"}
            ],
            "description": "A number, or \"last\""
        })
    }
}

/// Day of the week.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl Weekday {
    /// 1 = Monday … 7 = Sunday.
    pub fn number(self) -> u8 {
        match self {
            Weekday::Monday => 1,
            Weekday::Tuesday => 2,
            Weekday::Wednesday => 3,
            Weekday::Thursday => 4,
            Weekday::Friday => 5,
            Weekday::Saturday => 6,
            Weekday::Sunday => 7,
        }
    }

    pub fn from_number(n: u8) -> Option<Self> {
        Some(match n {
            1 => Weekday::Monday,
            2 => Weekday::Tuesday,
            3 => Weekday::Wednesday,
            4 => Weekday::Thursday,
            5 => Weekday::Friday,
            6 => Weekday::Saturday,
            7 => Weekday::Sunday,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Weekday::Monday => "Monday",
            Weekday::Tuesday => "Tuesday",
            Weekday::Wednesday => "Wednesday",
            Weekday::Thursday => "Thursday",
            Weekday::Friday => "Friday",
            Weekday::Saturday => "Saturday",
            Weekday::Sunday => "Sunday",
        }
    }
}

fn one() -> u32 {
    1
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

/// A recurrence rule. Which fields apply depends on `frequency`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recurrence {
    pub frequency: Frequency,
    /// Every n-th day/week/month/year (daily, weekly, monthly, yearly). Default 1.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub interval: u32,
    /// Monthly/yearly: day of the month (1–31 or "last"). Defaults to the start date's day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<MonthDay>,
    /// Monthly: weekday, together with `week`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekday: Option<Weekday>,
    /// Monthly: week of the month (1–4 or "last"), together with `weekday`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week: Option<MonthDay>,
    /// Yearly: month 1–12. Defaults to the start date's month.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<u8>,
    /// Semi-monthly: two distinct days (1–31 or "last").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<Vec<MonthDay>>,
}

impl Recurrence {
    /// A recurrence of `frequency` with every other field defaulted.
    pub fn new(frequency: Frequency) -> Self {
        Self {
            frequency,
            interval: 1,
            day: None,
            weekday: None,
            week: None,
            month: None,
            days: None,
        }
    }
}

/// When a schedule ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EndRule {
    #[default]
    Never,
    /// After this many occurrences (entered and skipped both count).
    After(u32),
    /// The last nominal date (inclusive).
    On(Date),
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum EndRepr {
    Text(String),
    After { after: u32 },
    On { on: Date },
}

impl Serialize for EndRule {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            EndRule::Never => EndRepr::Text("never".into()),
            EndRule::After(n) => EndRepr::After { after: *n },
            EndRule::On(d) => EndRepr::On { on: *d },
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for EndRule {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match EndRepr::deserialize(d)? {
            EndRepr::Text(t) if t == "never" => Ok(EndRule::Never),
            EndRepr::Text(t) => Err(serde::de::Error::custom(format!(
                "expected \"never\", {{\"after\": N}} or {{\"on\": date}}, got {t:?}"
            ))),
            EndRepr::After { after } => Ok(EndRule::After(after)),
            EndRepr::On { on } => Ok(EndRule::On(on)),
        }
    }
}

impl JsonSchema for EndRule {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        "EndRule".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "oneOf": [
                {"const": "never"},
                {"type": "object", "properties": {"after": {"type": "integer", "minimum": 1}}, "required": ["after"], "additionalProperties": false},
                {"type": "object", "properties": {"on": {"type": "string", "format": "date"}}, "required": ["on"], "additionalProperties": false}
            ],
            "description": "\"never\", {\"after\": N} occurrences, or {\"on\": date}"
        })
    }
}

/// What to do when a due date falls on a Saturday or Sunday.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WeekendRule {
    #[default]
    None,
    /// Move to the Friday before.
    Before,
    /// Move to the Monday after.
    After,
}

/// Whether a schedule's amount is fixed or an estimate to confirm when entering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AmountKind {
    #[default]
    Fixed,
    Estimate,
}
