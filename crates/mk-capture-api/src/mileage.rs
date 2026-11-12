//! Trips from the phone (mileage.md "Capture server and API", mobile-capture.md
//! revision 11): the `trip` submission kind, the mileage parts of the lists
//! document, and their checks. Miles, odometer readings and rates travel as decimal
//! strings and are worked with as whole tenths, so no floating point is used.

use serde::{Deserialize, Serialize};

use crate::{DATE_WINDOW_DAYS, FieldError, MAX_LIST_ITEMS, Tags, clean_text, parse_date};

/// The purpose kinds a trip can have, in the order pickers show them.
pub const TRIP_KINDS: [&str; 5] = ["business", "medical", "moving", "charity", "personal"];
/// A trip's purpose text limit (as `mileage.md`).
pub const MAX_TRIP_PURPOSE: usize = 500;
/// From / To text limit.
pub const MAX_PLACE: usize = 100;
/// A vehicle's name.
pub const MAX_VEHICLE_NAME: usize = 60;
/// Ids the desktop gives vehicles and saved trips.
pub const MAX_REF: usize = 100;
/// Saved trips a book can have.
pub const MAX_SAVED_TRIPS: usize = 200;
/// Places the phone remembers as suggestions.
pub const PLACES_KEPT: usize = 20;
/// The most miles one trip may have, in tenths (2,000 mi).
pub const MAX_TRIP_TENTHS: u64 = 20_000;
/// The most one-way miles of a round trip, in tenths (1,000 mi).
pub const MAX_ONE_WAY_TENTHS: u64 = 10_000;
/// The largest odometer reading accepted, in tenths (9,999,999.9).
pub const MAX_READING_TENTHS: u64 = 99_999_999;

/// The fields of a trip as typed on the phone. `miles` is one way when
/// `round_trip` is on (the book doubles it); with both odometer readings the
/// miles are end − start.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TripFields {
    /// `YYYY-MM-DD`.
    #[serde(default)]
    pub date: String,
    /// The book's vehicle id (kept as given; the desktop resolves it).
    #[serde(default)]
    pub vehicle_id: String,
    /// business, medical, moving, charity or personal.
    #[serde(default)]
    pub purpose_kind: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub from_place: String,
    #[serde(default)]
    pub to_place: String,
    /// A decimal string with at most one decimal ("12.4").
    #[serde(default)]
    pub miles: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub odometer_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub odometer_end: Option<String>,
    #[serde(default)]
    pub round_trip: bool,
    #[serde(default)]
    pub reimbursable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_trip_id: Option<String>,
    /// Class, location and fund names (mileage.md revision 2), as an expense's.
    #[serde(default, skip_serializing_if = "Tags::is_empty")]
    pub tags: Tags,
}

impl TripFields {
    /// The miles of the whole trip in tenths (twice the one-way miles for a round
    /// trip), when they can be read.
    pub fn total_tenths(&self) -> Option<u64> {
        let m = parse_tenths(&self.miles).ok()?;
        Some(if self.round_trip {
            m.saturating_mul(2)
        } else {
            m
        })
    }

    /// "Office → Client", "Office → Client → Office" for a round trip; the parts
    /// that are empty are left out.
    pub fn route(&self) -> String {
        let from = self.from_place.trim();
        let to = self.to_place.trim();
        match (from.is_empty(), to.is_empty()) {
            (true, true) => String::new(),
            (true, false) => to.to_owned(),
            (false, true) => from.to_owned(),
            (false, false) if self.round_trip => format!("{from} \u{2192} {to} \u{2192} {from}"),
            (false, false) => format!("{from} \u{2192} {to}"),
        }
    }

    /// The Outbox and Sent row: "Mileage · 24.8 mi · Office → Client".
    pub fn summary(&self) -> String {
        let mut parts = vec!["Mileage".to_owned()];
        if let Some(t) = self.total_tenths().filter(|t| *t > 0) {
            parts.push(format!("{} mi", tenths_text(t)));
        }
        let r = self.route();
        if !r.is_empty() {
            parts.push(r);
        }
        parts.join(" \u{b7} ")
    }
}

/// `POST /api/v1/trips`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NewTrip {
    /// A UUID chosen by the phone; a retry with the same id creates nothing new.
    pub client_id: String,
    #[serde(flatten)]
    pub fields: TripFields,
}

/// `PATCH /api/v1/trips/{id}`: replaces the fields while the trip waits.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TripPatch {
    #[serde(flatten)]
    pub fields: TripFields,
}

/// Where a trip is: waiting on the server, taken into the book, or refused by the
/// desktop (with a reason).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TripStatus {
    Waiting,
    Taken,
    Refused,
}

/// A trip as the phone sees it (`GET /api/v1/trips`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TripSubmission {
    pub id: String,
    pub client_id: String,
    pub status: TripStatus,
    /// Unix seconds.
    pub created_at: i64,
    /// When it was taken or refused (Unix seconds).
    #[serde(default)]
    pub done_at: Option<i64>,
    /// Why the desktop refused it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The book's amount for the trip once taken ("17.98"): the book's rate table
    /// wins over the phone's copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    #[serde(flatten)]
    pub fields: TripFields,
}

/// `GET /api/v1/trips`: this phone's waiting trips and its last ones taken or
/// refused.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TripList {
    #[serde(default)]
    pub waiting: Vec<TripSubmission>,
    #[serde(default)]
    pub done: Vec<TripSubmission>,
}

/// A waiting trip in the desktop's inbox (`GET /api/v1/desktop/inbox`, `trips`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TripInboxItem {
    pub id: String,
    pub client_id: String,
    pub device_id: String,
    pub device_name: String,
    /// Unix seconds.
    pub submitted_at: i64,
    pub fields: TripFields,
}

/// `POST /api/v1/desktop/trips/{id}/ack`: the trip was taken (with the book's
/// amount) or refused (with the reason shown on the phone).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TripAck {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
}

// ----- the lists document's mileage parts -------------------------------------------

/// A purpose kind offered on the phone.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PurposeKind {
    pub kind: String,
    #[serde(default)]
    pub label: String,
}

/// An active vehicle.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ListVehicle {
    pub id: String,
    pub name: String,
}

/// A saved trip (a named route that fills the form).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ListSavedTrip {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub from_place: Option<String>,
    #[serde(default)]
    pub to_place: Option<String>,
    /// One way.
    #[serde(default)]
    pub miles: String,
    #[serde(default)]
    pub round_trip: bool,
    #[serde(default)]
    pub purpose_kind: String,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub vehicle: Option<String>,
}

/// A row of the book's rate table: cents per mile of a kind from a date.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ListRate {
    pub kind: String,
    /// `YYYY-MM-DD`.
    pub from: String,
    /// Cents per mile with up to one decimal ("72.5").
    pub cents: String,
}

// ----- numbers -------------------------------------------------------------------------

/// A non-negative decimal with at most one decimal place, as whole tenths. Extra
/// decimal places are allowed only when they are zeros ("72.50").
pub fn parse_tenths(s: &str) -> Result<u64, String> {
    let t = s.trim().replace(',', ".");
    let (int, frac) = t.split_once('.').unwrap_or((t.as_str(), ""));
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if t.is_empty() || !digits(int) || !digits(frac) || (int.is_empty() && frac.is_empty()) {
        return Err("Enter a number such as 12.4.".into());
    }
    if frac.bytes().skip(1).any(|b| b != b'0') {
        return Err("Use at most 1 decimal place.".into());
    }
    let int = int.trim_start_matches('0');
    if int.len() > 9 {
        return Err("The number is too large.".into());
    }
    let whole: u64 = if int.is_empty() {
        0
    } else {
        int.parse()
            .map_err(|_| "The number is too large.".to_owned())?
    };
    let tenth = frac
        .bytes()
        .next()
        .map_or(0, |b| u64::from(b.saturating_sub(b'0')));
    Ok(whole * 10 + tenth)
}

/// Tenths as text with one decimal ("24.8", "25.0").
pub fn tenths_text(t: u64) -> String {
    format!("{}.{}", t / 10, t % 10)
}

/// Tenths of a cent as a rate ("72.5", "14").
pub fn cents_text(t: u64) -> String {
    if t.is_multiple_of(10) {
        (t / 10).to_string()
    } else {
        tenths_text(t)
    }
}

/// The rate (tenths of a cent per mile) for a kind on a date: the row with the
/// latest `from` on or before the date. Personal trips have none.
pub fn rate_for(rates: &[ListRate], kind: &str, date: &str) -> Option<u64> {
    let day = parse_date(date)?;
    rates
        .iter()
        .filter(|r| r.kind == kind)
        .filter_map(|r| Some((parse_date(&r.from)?, parse_tenths(&r.cents).ok()?)))
        .filter(|(from, _)| *from <= day)
        .max_by_key(|(from, _)| *from)
        .map(|(_, c)| c)
}

/// round(miles × cents / 100, 2) in cents, half up, from tenths of each.
pub fn trip_value_cents(miles_tenths: u64, cents_tenths: u64) -> u64 {
    miles_tenths.saturating_mul(cents_tenths).saturating_add(50) / 100
}

/// Cents as money text ("17.98").
pub fn money_text(cents: u64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

// ----- checks -------------------------------------------------------------------------

fn fe(field: &str, message: impl Into<String>) -> FieldError {
    FieldError {
        field: field.to_owned(),
        message: message.into(),
    }
}

/// Whether `k` is one of [`TRIP_KINDS`].
pub fn is_trip_kind(k: &str) -> bool {
    TRIP_KINDS.contains(&k)
}

fn text(v: &str, max: usize, field: &str, what: &str, errors: &mut Vec<FieldError>) -> String {
    match clean_text(v, max, what) {
        Ok(s) => s.unwrap_or_default(),
        Err(m) => {
            errors.push(fe(field, m));
            String::new()
        }
    }
}

fn reading(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Validate and clean a trip against `today` (a day number): the date within
/// ±400 days, a known purpose kind, the purpose and destination (unless personal),
/// the text limits, and miles that are more than 0 with at most one decimal and at
/// most 2,000 per trip (1,000 one way for a round trip). With both odometer
/// readings, the miles become end − start and the round trip is off.
pub fn validate_trip(f: &TripFields, today: i64) -> Result<TripFields, Vec<FieldError>> {
    let mut errors = Vec::new();
    let date = f.date.trim();
    match parse_date(date) {
        _ if date.is_empty() => errors.push(fe("date", "Enter the date.")),
        Some(n) if (n - today).abs() <= DATE_WINDOW_DAYS => {}
        Some(_) => errors.push(fe(
            "date",
            format!("The date must be within {DATE_WINDOW_DAYS} days of today."),
        )),
        None => errors.push(fe("date", "The date must be a date such as 2026-10-23.")),
    }
    let kind = f.purpose_kind.trim().to_ascii_lowercase();
    if !is_trip_kind(&kind) {
        errors.push(fe(
            "purpose_kind",
            "The purpose kind must be business, medical, moving, charity or personal.",
        ));
    }
    let personal = kind == "personal";
    let purpose = text(
        &f.purpose,
        MAX_TRIP_PURPOSE,
        "purpose",
        "Purpose",
        &mut errors,
    );
    let from_place = text(&f.from_place, MAX_PLACE, "from_place", "From", &mut errors);
    let to_place = text(&f.to_place, MAX_PLACE, "to_place", "To", &mut errors);
    let vehicle_id = text(&f.vehicle_id, MAX_REF, "vehicle_id", "Vehicle", &mut errors);
    let saved_trip_id = match f.saved_trip_id.as_deref() {
        Some(s) => Some(text(s, MAX_REF, "saved_trip_id", "Saved trip", &mut errors))
            .filter(|s| !s.is_empty()),
        None => None,
    };
    let tags = crate::validate::clean_tags(&f.tags, &mut errors);
    let mut round_trip = f.round_trip;
    let (mut odometer_start, mut odometer_end) = (None, None);
    let mut miles = String::new();
    match (reading(&f.odometer_start), reading(&f.odometer_end)) {
        (Some(s), Some(e)) => match (parse_tenths(s), parse_tenths(e)) {
            (Ok(s), Ok(e)) if s > MAX_READING_TENTHS || e > MAX_READING_TENTHS => {
                errors.push(fe("odometer_end", "An odometer reading is too large."))
            }
            (Ok(s), Ok(e)) if e <= s => errors.push(fe(
                "odometer_end",
                "The end reading must be more than the start reading.",
            )),
            (Ok(s), Ok(e)) if e - s > MAX_TRIP_TENTHS => {
                errors.push(fe("odometer_end", "A trip can be at most 2,000 miles."));
            }
            (Ok(s), Ok(e)) => {
                odometer_start = Some(tenths_text(s));
                odometer_end = Some(tenths_text(e));
                miles = tenths_text(e - s);
                // The round trip is already in the readings.
                round_trip = false;
            }
            _ => errors.push(fe(
                "odometer_start",
                "An odometer reading must be a number such as 45210.3.",
            )),
        },
        (None, None) => match parse_tenths(&f.miles) {
            Ok(0) => errors.push(fe("miles", "Miles must be more than 0.")),
            Ok(m) if round_trip && m > MAX_ONE_WAY_TENTHS => errors.push(fe(
                "miles",
                "A round trip can be at most 1,000 miles one way.",
            )),
            Ok(m) if m > MAX_TRIP_TENTHS => {
                errors.push(fe("miles", "A trip can be at most 2,000 miles."));
            }
            Ok(m) => miles = tenths_text(m),
            Err(_) if f.miles.trim().is_empty() => errors.push(fe("miles", "Enter the miles.")),
            Err(m) => errors.push(fe("miles", format!("Miles: {m}"))),
        },
        _ => errors.push(fe(
            "odometer_end",
            "Enter both odometer readings, or only the miles.",
        )),
    }
    if errors.is_empty() && !personal {
        if purpose.is_empty() {
            errors.push(fe("purpose", "Enter the purpose of the trip."));
        }
        if to_place.is_empty() {
            errors.push(fe("to_place", "Enter where the trip went."));
        }
    }
    if errors.is_empty() {
        Ok(TripFields {
            date: date.to_owned(),
            vehicle_id,
            purpose_kind: kind,
            purpose,
            from_place,
            to_place,
            miles,
            odometer_start,
            odometer_end,
            round_trip,
            reimbursable: f.reimbursable && !personal,
            saved_trip_id,
            tags,
        })
    } else {
        Err(errors)
    }
}

fn clean_opt(v: &Option<String>, max: usize) -> Option<String> {
    v.as_deref()
        .and_then(|s| clean_text(s, max, "").ok().flatten())
}

/// The mileage parts of a lists document, cleaned: entries that cannot be used are
/// left out; too many entries refuse the document.
pub(crate) fn clean_lists(l: &crate::Lists) -> Result<crate::Lists, String> {
    let too_many = |n: usize, max: usize, what: &str| {
        if n > max {
            Err(format!("{what}: at most {max} entries."))
        } else {
            Ok(())
        }
    };
    too_many(l.vehicles.len(), MAX_LIST_ITEMS, "Vehicles")?;
    too_many(l.saved_trips.len(), MAX_SAVED_TRIPS, "Saved trips")?;
    too_many(l.mileage_rates.len(), MAX_LIST_ITEMS, "Mileage rates")?;
    too_many(l.purpose_kinds.len(), TRIP_KINDS.len(), "Purpose kinds")?;
    let ok_kind = |k: &str| is_trip_kind(k);
    let purpose_kinds = l
        .purpose_kinds
        .iter()
        .filter(|k| ok_kind(&k.kind))
        .map(|k| PurposeKind {
            kind: k.kind.clone(),
            label: clean_text(&k.label, 30, "")
                .ok()
                .flatten()
                .unwrap_or_default(),
        })
        .collect();
    let vehicles = l
        .vehicles
        .iter()
        .filter_map(|v| {
            Some(ListVehicle {
                id: clean_text(&v.id, MAX_REF, "").ok().flatten()?,
                name: clean_text(&v.name, MAX_VEHICLE_NAME, "").ok().flatten()?,
            })
        })
        .collect();
    let saved_trips = l
        .saved_trips
        .iter()
        .filter(|s| ok_kind(&s.purpose_kind))
        .filter_map(|s| {
            Some(ListSavedTrip {
                id: clean_text(&s.id, MAX_REF, "").ok().flatten()?,
                name: clean_text(&s.name, MAX_VEHICLE_NAME, "").ok().flatten()?,
                from_place: clean_opt(&s.from_place, MAX_PLACE),
                to_place: clean_opt(&s.to_place, MAX_PLACE),
                miles: tenths_text(parse_tenths(&s.miles).ok()?),
                round_trip: s.round_trip,
                purpose_kind: s.purpose_kind.clone(),
                purpose: clean_opt(&s.purpose, MAX_TRIP_PURPOSE),
                vehicle: clean_opt(&s.vehicle, MAX_REF),
            })
        })
        .collect();
    let mileage_rates = l
        .mileage_rates
        .iter()
        .filter(|r| ok_kind(&r.kind) && parse_date(&r.from).is_some())
        .filter_map(|r| {
            Some(ListRate {
                kind: r.kind.clone(),
                from: r.from.clone(),
                cents: cents_text(parse_tenths(&r.cents).ok()?),
            })
        })
        .collect();
    Ok(crate::Lists {
        mileage_enabled: l.mileage_enabled,
        reimbursable_enabled: l.reimbursable_enabled && l.mileage_enabled,
        default_purpose: if ok_kind(&l.default_purpose) {
            l.default_purpose.clone()
        } else {
            String::new()
        },
        purpose_kinds,
        vehicles,
        saved_trips,
        mileage_rates,
        ..crate::Lists::default()
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;
    use crate::days_from_civil;

    fn today() -> i64 {
        days_from_civil(2026, 11, 5)
    }

    fn trip() -> TripFields {
        TripFields {
            date: "2026-11-05".into(),
            vehicle_id: "v1".into(),
            purpose_kind: "business".into(),
            purpose: "Client visit".into(),
            from_place: "Office".into(),
            to_place: "Client".into(),
            miles: "12.4".into(),
            round_trip: true,
            ..TripFields::default()
        }
    }

    fn messages(f: &TripFields) -> Vec<String> {
        validate_trip(f, today())
            .unwrap_err()
            .into_iter()
            .map(|e| e.message)
            .collect()
    }

    /// mileage.md M12 (validation layer; the server test checks the answers).
    #[test]
    fn m12_refused_trips_say_why() {
        let mut t = trip();
        t.miles = "0".into();
        assert_eq!(messages(&t), ["Miles must be more than 0."]);
        let mut t = trip();
        t.purpose_kind = "vacation".into();
        assert_eq!(
            messages(&t),
            ["The purpose kind must be business, medical, moving, charity or personal."]
        );
        let mut t = trip();
        t.purpose = "p".repeat(600);
        assert_eq!(messages(&t), ["Purpose can be at most 500 characters."]);
        let mut t = trip();
        t.miles = "1000.1".into();
        assert_eq!(
            messages(&t),
            ["A round trip can be at most 1,000 miles one way."]
        );
        t.round_trip = false;
        assert!(validate_trip(&t, today()).is_ok());
        t.miles = "12.45".into();
        assert_eq!(messages(&t), ["Miles: Use at most 1 decimal place."]);
        let mut t = trip();
        t.to_place = " ".into();
        assert_eq!(messages(&t), ["Enter where the trip went."]);
        t.purpose_kind = "personal".into();
        t.purpose.clear();
        assert!(validate_trip(&t, today()).is_ok(), "personal needs neither");
    }

    /// M2 / M3 on the phone side: round trip and odometer readings.
    #[test]
    fn miles_round_trip_and_odometer() {
        let ok = validate_trip(&trip(), today()).unwrap();
        assert_eq!(ok.miles, "12.4");
        assert_eq!(ok.total_tenths(), Some(248));
        assert_eq!(
            ok.summary(),
            "Mileage \u{b7} 24.8 mi \u{b7} Office \u{2192} Client \u{2192} Office"
        );
        let mut t = trip();
        t.odometer_start = Some("45210.3".into());
        t.odometer_end = Some("45236".into());
        t.miles = "999".into();
        let ok = validate_trip(&t, today()).unwrap();
        assert_eq!(ok.miles, "25.7");
        assert!(!ok.round_trip, "the readings hold the round trip");
        assert_eq!(ok.odometer_end.as_deref(), Some("45236.0"));
        t.odometer_end = Some("45210.3".into());
        assert_eq!(
            messages(&t),
            ["The end reading must be more than the start reading."]
        );
        t.odometer_end = None;
        assert_eq!(
            messages(&t),
            ["Enter both odometer readings, or only the miles."]
        );
    }

    #[test]
    fn rates_and_values() {
        let rates = vec![
            ListRate {
                kind: "business".into(),
                from: "2026-01-01".into(),
                cents: "72.5".into(),
            },
            ListRate {
                kind: "business".into(),
                from: "2026-07-01".into(),
                cents: "76".into(),
            },
            ListRate {
                kind: "charity".into(),
                from: "2025-01-01".into(),
                cents: "14.0".into(),
            },
        ];
        assert_eq!(rate_for(&rates, "business", "2026-06-30"), Some(725));
        assert_eq!(rate_for(&rates, "business", "2026-07-01"), Some(760));
        assert_eq!(rate_for(&rates, "charity", "2026-07-01"), Some(140));
        assert_eq!(rate_for(&rates, "business", "2025-12-31"), None);
        assert_eq!(rate_for(&rates, "personal", "2026-07-01"), None);
        // M2: 24.8 mi × 72.5¢ = $17.98.
        assert_eq!(money_text(trip_value_cents(248, 725)), "17.98");
        assert_eq!(cents_text(725), "72.5");
        assert_eq!(cents_text(140), "14");
        assert_eq!(parse_tenths("72.50"), Ok(725));
        assert!(parse_tenths("-1").is_err());
        assert!(parse_tenths("1e3").is_err());
    }
}
