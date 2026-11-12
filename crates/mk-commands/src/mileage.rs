//! Mileage tracking (mileage.md): vehicles with odometer readings per year, the dated
//! standard rate table, trips (miles or odometer, round trips, saved trips), log-only
//! trips for the tax record and reimbursable trips that post like personal-funds
//! expenses, the mileage report (PDF or CSV) and the yearly summary the Tax summary
//! shows.
//!
//! Every record is a document, so every change is audited and undone like any other.
//! Log-only trips never touch the ledger.

use std::collections::{BTreeMap, HashMap};

use mk_core::{
    AccountId, AccountSubtype, AccountType, Date, Decimal, Reconcile, Split, SplitTags,
    TransactionId,
};
use mk_store::{DocTable, now_rfc3339, today_utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::accounts::Chart;
use crate::expenses::set_key;
use crate::module_settings as ms;
use crate::sales::{date_ord, invalid, money, new_id, rejected, require_open_period};
use crate::txns::validate_change;
use crate::{CommandError, CommandResult, Ctx, Registry, is_module_enabled};

pub(crate) const MODULE: &str = "mileage";

/// Purpose kinds, in display order.
pub(crate) const KIND_CHOICES: &[(&str, &str)] = &[
    ("business", "Business"),
    ("medical", "Medical"),
    ("moving", "Moving"),
    ("charity", "Charity"),
    ("personal", "Personal"),
];

const MAX_VEHICLE_NAME: usize = 60;
const MAX_NOTE: usize = 100;
const MAX_PURPOSE: usize = 500;
const MAX_PLACE: usize = 100;
const MAX_DRIVER: usize = 100;
const MAX_SAVED: usize = 200;

pub(crate) const NO_RATE: &str =
    "No standard rate for this date; add one in Settings → Mileage → Rates";
pub(crate) const ODOMETER_ORDER: &str = "Odometer readings out of order for this vehicle";
pub(crate) const DUPLICATE: &str = "A trip with these details already exists. Save anyway?";
pub(crate) const TAX_NOTE: &str = "Information for your tax preparer; not entered in the books.";
pub(crate) const RATES_NOTE: &str = "Standard mileage rates are announced by the IRS (irs.gov). MoneyKeep does not download them: when new rates are announced, add a row for each kind with the date they start.";
pub(crate) const CHOOSE_VEHICLE: &str = "Choose a vehicle";
const EXPENSE_PARENT: &str = "Auto";
const EXPENSE_NAME: &str = "Mileage reimbursement";

pub(crate) fn register(r: &mut Registry) {
    r.add_in(
        MODULE,
        "vehicle.create",
        "Add a vehicle (name, plate, note)",
        true,
        vehicle_create,
    );
    r.add_in(
        MODULE,
        "vehicle.update",
        "Change a vehicle's name, plate, note or whether it is active",
        true,
        vehicle_update,
    );
    r.add_in(
        MODULE,
        "vehicle.delete",
        "Delete a vehicle that has no trips (otherwise make it inactive)",
        true,
        vehicle_delete,
    );
    r.add_in(
        MODULE,
        "vehicle.list",
        "Vehicles with their odometer readings and miles per year",
        false,
        vehicle_list,
    );
    r.add_in(
        MODULE,
        "vehicle.set_odometer",
        "Record a vehicle's odometer at the start and end of a tax year",
        true,
        vehicle_set_odometer,
    );
    r.add_in(
        MODULE,
        "trip.create",
        "Log a trip (date, vehicle, purpose, route, miles or odometer readings, round trip, reimbursable)",
        true,
        trip_create,
    );
    r.add_in(
        MODULE,
        "trip.update",
        "Change a trip; a posted one changes its transaction too",
        true,
        trip_update,
    );
    r.add_in(
        MODULE,
        "trip.delete",
        "Delete a trip; a posted one asks whether its transaction goes too",
        true,
        trip_delete,
    );
    r.add_in(MODULE, "trip.get", "One trip", false, trip_get);
    r.add_in(
        MODULE,
        "trip.list",
        "Trips, newest first, filtered by period, vehicle, purpose kind, status or text",
        false,
        trip_list,
    );
    r.add_in(
        MODULE,
        "trip.post",
        "Post a ready reimbursable trip: debit its category, credit Reimbursements Payable",
        true,
        trip_post,
    );
    r.add_in(
        MODULE,
        "saved_trip.create",
        "Save a named route that fills the trip form",
        true,
        saved_create,
    );
    r.add_in(
        MODULE,
        "saved_trip.update",
        "Change a saved trip",
        true,
        saved_update,
    );
    r.add_in(
        MODULE,
        "saved_trip.delete",
        "Delete a saved trip",
        true,
        saved_delete,
    );
    r.add_in(
        MODULE,
        "saved_trip.list",
        "Saved trips by name",
        false,
        saved_list,
    );
    r.add_in(
        MODULE,
        "mileage_rate.list",
        "The standard mileage rate table",
        false,
        rate_list,
    );
    r.add_in(
        MODULE,
        "mileage_rate.set",
        "Add or change a standard rate row (kind, from date, cents per mile)",
        true,
        rate_set,
    );
    r.add_in(
        MODULE,
        "mileage_rate.delete",
        "Delete a standard rate row",
        true,
        rate_delete,
    );
    r.add_in(
        MODULE,
        "mileage.reapply_rates",
        "Apply the rate table again to the trips of a period (preview shows the count and difference)",
        true,
        reapply_rates,
    );
    r.add_in(
        MODULE,
        "mileage.report",
        "The mileage report for a period as PDF or CSV; changes nothing",
        false,
        report,
    );
    r.add_in(
        MODULE,
        "mileage.summary",
        "Miles and standard-rate value by vehicle and purpose kind for a year",
        false,
        summary,
    );
}

// ----- records ----------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Odometer {
    #[serde(default)]
    pub start: Option<Decimal>,
    #[serde(default)]
    pub end: Option<Decimal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct VehicleDoc {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub plate: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    pub active: bool,
    /// Readings per tax year ("2026").
    #[serde(default)]
    pub odometer: BTreeMap<String, Odometer>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RateDoc {
    pub id: String,
    pub kind: String,
    pub from: Date,
    pub cents: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SavedTripDoc {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub from_place: Option<String>,
    #[serde(default)]
    pub to_place: Option<String>,
    /// One-way distance.
    pub miles: Decimal,
    #[serde(default)]
    pub round_trip: bool,
    pub purpose_kind: String,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub vehicle: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TripDoc {
    pub id: String,
    pub date: Date,
    /// None only for a trip from the phone whose vehicle the book does not know.
    #[serde(default)]
    pub vehicle: Option<String>,
    pub purpose_kind: String,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub from_place: Option<String>,
    #[serde(default)]
    pub to_place: Option<String>,
    /// Miles of the whole trip (twice the one-way distance for a round trip).
    pub miles: Decimal,
    #[serde(default)]
    pub odometer_start: Option<Decimal>,
    #[serde(default)]
    pub odometer_end: Option<Decimal>,
    #[serde(default)]
    pub round_trip: bool,
    /// The standard rate used when saved (0 for personal or no rate).
    pub rate_cents: Decimal,
    /// Deduction value (log-only) or standard value: miles × rate.
    pub amount: Decimal,
    #[serde(default)]
    pub reimbursable: bool,
    #[serde(default)]
    pub driver: Option<String>,
    /// The reimbursement rate when it is not the standard rate (fixed when posted).
    #[serde(default)]
    pub reimburse_rate_cents: Option<Decimal>,
    #[serde(default)]
    pub reimburse_amount: Decimal,
    /// None: the `mileage.expense_account` default (chosen when posted).
    #[serde(default)]
    pub category: Option<AccountId>,
    #[serde(default)]
    pub tags: SplitTags,
    /// `draft` (needs a vehicle), `logged`, `ready` or `posted`.
    pub status: String,
    #[serde(default)]
    pub reported: bool,
    /// `desktop` or `phone`.
    pub source: String,
    #[serde(default)]
    pub transaction: Option<TransactionId>,
    #[serde(default)]
    pub reimbursement: Option<TransactionId>,
    #[serde(default)]
    pub saved_trip: Option<String>,
    /// The phone's id for idempotent resending.
    #[serde(default)]
    pub client_id: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl TripDoc {
    /// The rate reimbursements use.
    pub(crate) fn reimburse_rate(&self) -> Decimal {
        self.reimburse_rate_cents.unwrap_or(self.rate_cents)
    }

    /// The miles as entered: one way for a round trip.
    fn entered_miles(&self) -> Decimal {
        if self.round_trip {
            self.miles / Decimal::TWO
        } else {
            self.miles
        }
    }

    /// "From → To", "From → To → From", or whichever place is known.
    pub(crate) fn route(&self) -> String {
        route_of(
            self.from_place.as_deref(),
            self.to_place.as_deref(),
            self.round_trip,
        )
    }

    fn refresh_status(&mut self) {
        if self.status == "posted" {
            return;
        }
        self.status = if self.vehicle.is_none() {
            "draft"
        } else if self.reimbursable {
            "ready"
        } else {
            "logged"
        }
        .into();
    }
}

pub(crate) fn route_of(from: Option<&str>, to: Option<&str>, round: bool) -> String {
    let from = from.filter(|s| !s.is_empty());
    let to = to.filter(|s| !s.is_empty());
    match (from, to) {
        (Some(f), Some(t)) if round => format!("{f} → {t} → {f}"),
        (Some(f), Some(t)) => format!("{f} → {t}"),
        (None, Some(t)) if round => format!("{t} and back"),
        (None, Some(t)) => t.to_owned(),
        (Some(f), None) => f.to_owned(),
        (None, None) => String::new(),
    }
}

/// A field of a JSON object (null when missing).
fn at<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key).unwrap_or(&Value::Null)
}

pub(crate) fn kind_label(kind: &str) -> &'static str {
    KIND_CHOICES
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or("", |(_, l)| l)
}

fn not_found(what: &str, id: &str) -> CommandError {
    CommandError::NotFound(format!("{what} {id}"))
}

fn bad(e: impl std::fmt::Display) -> CommandError {
    crate::sales::bad_doc(e)
}

fn get<T: serde::de::DeserializeOwned>(
    ctx: &Ctx<'_>,
    t: DocTable,
    id: &str,
    what: &str,
) -> CommandResult<T> {
    let d = ctx
        .tx
        .get_doc(t, id.trim())?
        .ok_or_else(|| not_found(what, id))?;
    serde_json::from_value(d.json).map_err(bad)
}

pub(crate) fn all_vehicles(ctx: &Ctx<'_>) -> CommandResult<Vec<VehicleDoc>> {
    let mut v: Vec<VehicleDoc> = crate::sales::load(ctx, DocTable::Vehicle, None)?;
    v.sort_by_key(|x| x.name.to_lowercase());
    Ok(v)
}

fn save_vehicle(ctx: &Ctx<'_>, v: &VehicleDoc) -> CommandResult<()> {
    crate::sales::save(ctx, DocTable::Vehicle, &v.id, None, 0, v)
}

pub(crate) fn all_rates(ctx: &Ctx<'_>) -> CommandResult<Vec<RateDoc>> {
    crate::sales::load(ctx, DocTable::MileageRate, None)
}

fn save_rate(ctx: &Ctx<'_>, r: &RateDoc) -> CommandResult<()> {
    crate::sales::save(
        ctx,
        DocTable::MileageRate,
        &r.id,
        Some(&r.kind),
        date_ord(r.from),
        r,
    )
}

pub(crate) fn all_saved(ctx: &Ctx<'_>) -> CommandResult<Vec<SavedTripDoc>> {
    let mut v: Vec<SavedTripDoc> = crate::sales::load(ctx, DocTable::SavedTrip, None)?;
    v.sort_by_key(|x| x.name.to_lowercase());
    Ok(v)
}

fn save_saved(ctx: &Ctx<'_>, s: &SavedTripDoc) -> CommandResult<()> {
    crate::sales::save(ctx, DocTable::SavedTrip, &s.id, None, 0, s)
}

pub(crate) fn load_trip(ctx: &Ctx<'_>, id: &str) -> CommandResult<TripDoc> {
    get(ctx, DocTable::Trip, id, "trip")
}

pub(crate) fn all_trips(ctx: &Ctx<'_>) -> CommandResult<Vec<TripDoc>> {
    crate::sales::load(ctx, DocTable::Trip, None)
}

pub(crate) fn save_trip(ctx: &Ctx<'_>, t: &TripDoc) -> CommandResult<()> {
    crate::sales::save(ctx, DocTable::Trip, &t.id, None, date_ord(t.date), t)
}

// ----- values -----------------------------------------------------------------------

fn parse_dec(v: &Value, what: &str) -> CommandResult<Decimal> {
    let s = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
    s.trim()
        .replace(',', "")
        .parse::<Decimal>()
        .map_err(|_| invalid(format!("{what} must be a number")))
}

fn one_decimal(d: Decimal, what: &str) -> CommandResult<Decimal> {
    if d.normalize().scale() > 1 {
        return Err(invalid(format!("{what} can have at most 1 decimal")));
    }
    let mut d = d;
    d.rescale(1);
    Ok(d)
}

/// Cents per mile: 0–999.9 with at most one decimal.
pub(crate) fn check_cents(s: &str) -> CommandResult<Decimal> {
    let d = parse_dec(&json!(s), "The rate")?;
    if d.is_sign_negative() || d > Decimal::new(9999, 1) {
        return Err(invalid("The rate is from 0 to 999.9 cents per mile"));
    }
    one_decimal(d, "The rate")
}

fn check_miles(d: Decimal, what: &str) -> CommandResult<Decimal> {
    if d <= Decimal::ZERO {
        return Err(invalid(format!("{what} must be more than zero")));
    }
    one_decimal(d, what)
}

fn check_reading(d: Decimal) -> CommandResult<Decimal> {
    if d.is_sign_negative() || d > Decimal::from(10_000_000) {
        return Err(invalid("An odometer reading is from 0 to 10,000,000"));
    }
    one_decimal(d, "An odometer reading")
}

fn check_kind(k: &str) -> CommandResult<String> {
    let t = k.trim().to_lowercase();
    if KIND_CHOICES
        .iter()
        .any(|(v, l)| *v == t || l.eq_ignore_ascii_case(&t))
    {
        Ok(KIND_CHOICES
            .iter()
            .find(|(v, l)| *v == t || l.eq_ignore_ascii_case(&t))
            .map(|(v, _)| (*v).to_owned())
            .unwrap_or_default())
    } else {
        Err(invalid(format!(
            "purpose kind is business, medical, moving, charity or personal, not {k}"
        )))
    }
}

fn text(v: Option<String>, max: usize, what: &str) -> CommandResult<Option<String>> {
    crate::sales::text(v, max, what)
}

/// round(miles × cents / 100, 2).
pub(crate) fn value_of(miles: Decimal, cents: Decimal) -> Decimal {
    crate::sales::round2(miles.checked_mul(cents).unwrap_or_default() / Decimal::ONE_HUNDRED)
}

/// The standard rate of a kind on a date: the row with the latest `from` on or before
/// it. Personal trips have none (0); a date before the first row has none either.
pub(crate) fn rate_for(rates: &[RateDoc], kind: &str, date: Date) -> Option<Decimal> {
    rates
        .iter()
        .filter(|r| r.kind == kind && r.from <= date)
        .max_by_key(|r| r.from)
        .map(|r| r.cents)
}

/// Text of a decimal without trailing zeros ("72.5", "76").
pub(crate) fn plain(d: Decimal) -> String {
    d.normalize().to_string()
}

/// Miles with one decimal and digit grouping ("12,000.0").
pub(crate) fn miles_text(d: Decimal) -> String {
    let mut d = d.round_dp(1);
    d.rescale(1);
    crate::sales::group(&d.to_string())
}

fn percent(part: Decimal, whole: Decimal) -> String {
    if whole.is_zero() {
        return "0.0".into();
    }
    let p = (part * Decimal::ONE_HUNDRED / whole)
        .round_dp_with_strategy(1, mk_core::RoundingStrategy::MidpointAwayFromZero);
    let mut p = p;
    p.rescale(1);
    p.to_string()
}

fn year_of(y: i32) -> String {
    y.to_string()
}

// ----- vehicles ---------------------------------------------------------------------

/// A vehicle by id or name (ignoring case).
fn find_vehicle(ctx: &Ctx<'_>, r: &str) -> CommandResult<VehicleDoc> {
    let t = r.trim();
    all_vehicles(ctx)?
        .into_iter()
        .find(|v| v.id == t || v.name.eq_ignore_ascii_case(t))
        .ok_or_else(|| not_found("vehicle", t))
}

fn vehicle_name_ok(ctx: &Ctx<'_>, name: &str, except: Option<&str>) -> CommandResult<String> {
    let n = text(Some(name.to_owned()), MAX_VEHICLE_NAME, "name")?
        .ok_or_else(|| invalid("A vehicle needs a name (1–60 characters)"))?;
    if all_vehicles(ctx)?
        .iter()
        .any(|v| Some(v.id.as_str()) != except && v.name.eq_ignore_ascii_case(&n))
    {
        return Err(rejected(format!("There is already a vehicle named {n}")));
    }
    Ok(n)
}

fn vehicle_json(v: &VehicleDoc, trips: &[TripDoc]) -> Value {
    let mut years: BTreeMap<String, (Decimal, usize)> = BTreeMap::new();
    for t in trips.iter().filter(|t| t.vehicle.as_deref() == Some(&v.id)) {
        let e = years.entry(year_of(t.date.year())).or_default();
        e.0 += t.miles;
        e.1 += 1;
    }
    let odometer: serde_json::Map<String, Value> = v
        .odometer
        .iter()
        .map(|(y, o)| {
            let total = match (o.start, o.end) {
                (Some(s), Some(e)) => Some(e - s),
                _ => None,
            };
            (
                y.clone(),
                json!({"start": o.start, "end": o.end, "miles": total}),
            )
        })
        .collect();
    json!({
        "id": v.id, "name": v.name, "plate": v.plate, "note": v.note, "active": v.active,
        "odometer": odometer,
        "trip_miles": years.iter().map(|(y, (m, _))| (y.clone(), json!(m.to_string()))).collect::<serde_json::Map<_, _>>(),
        "trips": trips.iter().filter(|t| t.vehicle.as_deref() == Some(&v.id)).count(),
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct VehicleArgs {
    /// 1–60 characters, unique ignoring case (e.g. "2019 Honda CR-V").
    name: String,
    #[serde(default)]
    plate: Option<String>,
    #[serde(default)]
    note: Option<String>,
    /// Default true.
    #[serde(default)]
    active: Option<bool>,
}

fn vehicle_create(ctx: &Ctx<'_>, a: VehicleArgs) -> CommandResult<Value> {
    let v = VehicleDoc {
        id: new_id(),
        name: vehicle_name_ok(ctx, &a.name, None)?,
        plate: text(a.plate, MAX_NOTE, "plate")?,
        note: text(a.note, MAX_NOTE, "note")?,
        active: a.active.unwrap_or(true),
        odometer: BTreeMap::new(),
        created_at: now_rfc3339()?,
    };
    save_vehicle(ctx, &v)?;
    Ok(vehicle_json(&v, &[]))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct VehicleUpdateArgs {
    vehicle: String,
    #[serde(default)]
    name: Option<String>,
    /// "" clears it.
    #[serde(default)]
    plate: Option<String>,
    /// "" clears it.
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    active: Option<bool>,
}

fn vehicle_update(ctx: &Ctx<'_>, a: VehicleUpdateArgs) -> CommandResult<Value> {
    let mut v = find_vehicle(ctx, &a.vehicle)?;
    if let Some(n) = a.name {
        v.name = vehicle_name_ok(ctx, &n, Some(&v.id))?;
    }
    if let Some(p) = a.plate {
        v.plate = text(Some(p), MAX_NOTE, "plate")?;
    }
    if let Some(n) = a.note {
        v.note = text(Some(n), MAX_NOTE, "note")?;
    }
    if let Some(on) = a.active {
        v.active = on;
    }
    save_vehicle(ctx, &v)?;
    Ok(vehicle_json(&v, &all_trips(ctx)?))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct VehicleRef {
    vehicle: String,
}

fn vehicle_delete(ctx: &Ctx<'_>, a: VehicleRef) -> CommandResult<Value> {
    let v = find_vehicle(ctx, &a.vehicle)?;
    if all_trips(ctx)?
        .iter()
        .any(|t| t.vehicle.as_deref() == Some(&v.id))
    {
        return Err(rejected(format!(
            "{} has trips and can't be deleted; make it inactive instead.",
            v.name
        )));
    }
    // Saved trips that named it keep their route without a vehicle.
    for mut s in all_saved(ctx)? {
        if s.vehicle.as_deref() == Some(&v.id) {
            s.vehicle = None;
            save_saved(ctx, &s)?;
        }
    }
    ctx.save_doc(DocTable::Vehicle, &v.id, None)?;
    Ok(json!({"deleted": v.id}))
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct VehicleListArgs {
    /// Default true; false lists only active vehicles.
    #[serde(default)]
    include_inactive: Option<bool>,
}

fn vehicle_list(ctx: &Ctx<'_>, a: VehicleListArgs) -> CommandResult<Value> {
    let trips = all_trips(ctx)?;
    let all = a.include_inactive.unwrap_or(true);
    Ok(Value::Array(
        all_vehicles(ctx)?
            .iter()
            .filter(|v| all || v.active)
            .map(|v| vehicle_json(v, &trips))
            .collect(),
    ))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OdometerArgs {
    vehicle: String,
    /// The tax year.
    year: i32,
    /// The reading on 1 January or when placed in service; "" or null clears it,
    /// absent keeps it.
    #[serde(default)]
    start: Option<Value>,
    /// The reading on 31 December or when retired; "" or null clears it.
    #[serde(default)]
    end: Option<Value>,
}

fn reading_arg(v: Option<Value>, cur: Option<Decimal>) -> CommandResult<Option<Decimal>> {
    match v {
        None => Ok(cur),
        Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(x) => Ok(Some(check_reading(parse_dec(&x, "An odometer reading")?)?)),
    }
}

fn vehicle_set_odometer(ctx: &Ctx<'_>, a: OdometerArgs) -> CommandResult<Value> {
    if !(1900..=2200).contains(&a.year) {
        return Err(invalid(format!("{} is not a year", a.year)));
    }
    let mut v = find_vehicle(ctx, &a.vehicle)?;
    let key = year_of(a.year);
    let cur = v.odometer.get(&key).cloned().unwrap_or_default();
    let o = Odometer {
        start: reading_arg(a.start, cur.start)?,
        end: reading_arg(a.end, cur.end)?,
    };
    if let (Some(s), Some(e)) = (o.start, o.end)
        && e < s
    {
        return Err(invalid(
            "The end reading must not be less than the start reading",
        ));
    }
    if o.start.is_none() && o.end.is_none() {
        v.odometer.remove(&key);
    } else {
        v.odometer.insert(key, o);
    }
    save_vehicle(ctx, &v)?;
    Ok(vehicle_json(&v, &all_trips(ctx)?))
}

// ----- rates ------------------------------------------------------------------------

fn rate_json(r: &RateDoc) -> Value {
    json!({"id": r.id, "kind": r.kind, "kind_label": kind_label(&r.kind), "from": r.from, "cents": r.cents.to_string()})
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RateListArgs {
    #[serde(default)]
    kind: Option<String>,
}

fn rate_list(ctx: &Ctx<'_>, a: RateListArgs) -> CommandResult<Value> {
    let kind = a.kind.as_deref().map(check_kind).transpose()?;
    let mut rows = all_rates(ctx)?;
    rows.retain(|r| kind.as_ref().is_none_or(|k| &r.kind == k));
    rows.sort_by(|x, y| {
        x.from
            .cmp(&y.from)
            .then_with(|| kind_rank(&x.kind).cmp(&kind_rank(&y.kind)))
    });
    Ok(json!({"rates": rows.iter().map(rate_json).collect::<Vec<_>>(), "note": RATES_NOTE}))
}

fn kind_rank(k: &str) -> usize {
    KIND_CHOICES
        .iter()
        .position(|(v, _)| *v == k)
        .unwrap_or(KIND_CHOICES.len())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RateSetArgs {
    /// business, medical, moving or charity.
    kind: String,
    from: Date,
    /// Cents per mile, up to one decimal (e.g. "72.5").
    cents: Value,
    /// The row to change; absent adds a row.
    #[serde(default)]
    rate: Option<String>,
}

fn rate_set(ctx: &Ctx<'_>, a: RateSetArgs) -> CommandResult<Value> {
    let kind = check_kind(&a.kind)?;
    if kind == "personal" {
        return Err(invalid("Personal trips have no standard rate"));
    }
    let cents = check_cents(
        &a.cents
            .as_str()
            .map_or_else(|| a.cents.to_string(), str::to_owned),
    )?;
    let rows = all_rates(ctx)?;
    let id = match a.rate.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(r) => {
            if !rows.iter().any(|x| x.id == r) {
                return Err(not_found("rate", r));
            }
            r.to_owned()
        }
        None => new_id(),
    };
    if rows
        .iter()
        .any(|x| x.id != id && x.kind == kind && x.from == a.from)
    {
        return Err(rejected("A rate for this kind already starts on that date"));
    }
    let r = RateDoc {
        id,
        kind,
        from: a.from,
        cents,
    };
    save_rate(ctx, &r)?;
    Ok(rate_json(&r))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RateRef {
    rate: String,
}

fn rate_delete(ctx: &Ctx<'_>, a: RateRef) -> CommandResult<Value> {
    let r: RateDoc = get(ctx, DocTable::MileageRate, &a.rate, "rate")?;
    ctx.save_doc(DocTable::MileageRate, &r.id, None)?;
    Ok(json!({"deleted": r.id}))
}

// ----- saved trips ------------------------------------------------------------------

fn saved_json(s: &SavedTripDoc, names: &HashMap<String, String>) -> Value {
    json!({
        "id": s.id, "name": s.name, "from_place": s.from_place, "to_place": s.to_place,
        "miles": s.miles.to_string(), "round_trip": s.round_trip,
        "purpose_kind": s.purpose_kind, "purpose": s.purpose,
        "vehicle": s.vehicle,
        "vehicle_name": s.vehicle.as_ref().and_then(|v| names.get(v)).cloned(),
        "route": route_of(s.from_place.as_deref(), s.to_place.as_deref(), s.round_trip),
    })
}

fn vehicle_names(ctx: &Ctx<'_>) -> CommandResult<HashMap<String, String>> {
    Ok(all_vehicles(ctx)?
        .into_iter()
        .map(|v| (v.id, v.name))
        .collect())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SavedArgs {
    /// 1–60 characters, unique ignoring case.
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    from_place: Option<String>,
    #[serde(default)]
    to_place: Option<String>,
    /// One-way miles.
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    miles: Option<Decimal>,
    #[serde(default)]
    round_trip: Option<bool>,
    #[serde(default)]
    purpose_kind: Option<String>,
    #[serde(default)]
    purpose: Option<String>,
    /// A vehicle, or "" for none.
    #[serde(default)]
    vehicle: Option<String>,
    /// For saved_trip.update: the saved trip to change.
    #[serde(default)]
    saved_trip: Option<String>,
}

fn apply_saved(ctx: &Ctx<'_>, s: &mut SavedTripDoc, a: SavedArgs) -> CommandResult<()> {
    if let Some(n) = a.name {
        let n = text(Some(n), MAX_VEHICLE_NAME, "name")?
            .ok_or_else(|| invalid("A saved trip needs a name (1–60 characters)"))?;
        if all_saved(ctx)?
            .iter()
            .any(|x| x.id != s.id && x.name.eq_ignore_ascii_case(&n))
        {
            return Err(rejected(format!("There is already a saved trip named {n}")));
        }
        s.name = n;
    }
    if let Some(f) = a.from_place {
        s.from_place = text(Some(f), MAX_PLACE, "from")?;
    }
    if let Some(t) = a.to_place {
        s.to_place = text(Some(t), MAX_PLACE, "to")?;
    }
    if let Some(m) = a.miles {
        s.miles = check_miles(m, "Miles")?;
    }
    if let Some(r) = a.round_trip {
        s.round_trip = r;
    }
    if let Some(k) = a.purpose_kind {
        s.purpose_kind = check_kind(&k)?;
    }
    if let Some(p) = a.purpose {
        s.purpose = text(Some(p), MAX_PURPOSE, "purpose")?;
    }
    if let Some(v) = a.vehicle {
        s.vehicle = match v.trim() {
            "" => None,
            r => Some(find_vehicle(ctx, r)?.id),
        };
    }
    let total = if s.round_trip {
        s.miles * Decimal::TWO
    } else {
        s.miles
    };
    if total > Decimal::from(2000) {
        return Err(invalid(
            "A trip is at most 2,000 miles (1,000 one way for a round trip)",
        ));
    }
    Ok(())
}

fn saved_create(ctx: &Ctx<'_>, a: SavedArgs) -> CommandResult<Value> {
    if all_saved(ctx)?.len() >= MAX_SAVED {
        return Err(rejected(format!(
            "At most {MAX_SAVED} saved trips; delete one first."
        )));
    }
    if a.name.is_none() {
        return Err(invalid("A saved trip needs a name (1–60 characters)"));
    }
    if a.miles.is_none() {
        return Err(invalid("A saved trip needs its one-way miles"));
    }
    let mut s = SavedTripDoc {
        id: new_id(),
        name: String::new(),
        from_place: None,
        to_place: None,
        miles: Decimal::ONE,
        round_trip: false,
        purpose_kind: ms::get_str(ctx, "mileage.default_purpose")?,
        purpose: None,
        vehicle: None,
    };
    if s.purpose_kind.is_empty() {
        s.purpose_kind = "business".into();
    }
    if a.saved_trip.is_some() {
        return Err(invalid("saved_trip is only for saved_trip.update"));
    }
    apply_saved(ctx, &mut s, a)?;
    save_saved(ctx, &s)?;
    Ok(saved_json(&s, &vehicle_names(ctx)?))
}

fn saved_update(ctx: &Ctx<'_>, mut a: SavedArgs) -> CommandResult<Value> {
    let id = a
        .saved_trip
        .take()
        .ok_or_else(|| invalid("give the saved_trip to change"))?;
    let mut s: SavedTripDoc = get(ctx, DocTable::SavedTrip, &id, "saved trip")?;
    apply_saved(ctx, &mut s, a)?;
    save_saved(ctx, &s)?;
    Ok(saved_json(&s, &vehicle_names(ctx)?))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SavedRef {
    saved_trip: String,
}

fn saved_delete(ctx: &Ctx<'_>, a: SavedRef) -> CommandResult<Value> {
    let s: SavedTripDoc = get(ctx, DocTable::SavedTrip, &a.saved_trip, "saved trip")?;
    ctx.save_doc(DocTable::SavedTrip, &s.id, None)?;
    Ok(json!({"deleted": s.id}))
}

fn saved_list(ctx: &Ctx<'_>, _: crate::builtin::NoArgs) -> CommandResult<Value> {
    let names = vehicle_names(ctx)?;
    Ok(Value::Array(
        all_saved(ctx)?
            .iter()
            .map(|s| saved_json(s, &names))
            .collect(),
    ))
}

// ----- trips ------------------------------------------------------------------------

/// Warnings of one trip: no standard rate, odometer readings out of order.
fn warnings(t: &TripDoc, rates: &[RateDoc], all: &[TripDoc]) -> Vec<String> {
    let mut w = Vec::new();
    if t.purpose_kind != "personal" && rate_for(rates, &t.purpose_kind, t.date).is_none() {
        w.push(NO_RATE.to_owned());
    }
    if let (Some(start), Some(end)) = (t.odometer_start, t.odometer_end) {
        let clash = all.iter().any(|o| {
            o.id != t.id
                && o.vehicle == t.vehicle
                && match (o.odometer_start, o.odometer_end) {
                    (Some(os), Some(oe)) => {
                        (o.date > t.date && os < end) || (o.date < t.date && oe > start)
                    }
                    _ => false,
                }
        });
        if clash {
            w.push(ODOMETER_ORDER.to_owned());
        }
    }
    if t.vehicle.is_none() {
        w.push(CHOOSE_VEHICLE.to_owned());
    }
    w
}

/// Names a trip's JSON needs, loaded once for a list.
struct View<'c> {
    chart: Chart,
    vehicles: HashMap<String, String>,
    rates: Vec<RateDoc>,
    trips: Vec<TripDoc>,
    ctx: &'c Ctx<'c>,
}

impl<'c> View<'c> {
    fn load(ctx: &'c Ctx<'c>) -> CommandResult<Self> {
        Ok(Self {
            chart: Chart::load(ctx)?,
            vehicles: vehicle_names(ctx)?,
            rates: all_rates(ctx)?,
            trips: all_trips(ctx)?,
            ctx,
        })
    }

    fn json(&self, t: &TripDoc) -> CommandResult<Value> {
        let category_name = match t.category {
            Some(c) => Some(self.chart.full_name(c)),
            None if t.reimbursable => Some(default_category_name(self.ctx, &self.chart)?),
            None => None,
        };
        Ok(json!({
            "id": t.id,
            "date": t.date,
            "vehicle": t.vehicle,
            "vehicle_name": t.vehicle.as_ref().and_then(|v| self.vehicles.get(v)).cloned(),
            "purpose_kind": t.purpose_kind,
            "purpose_kind_label": kind_label(&t.purpose_kind),
            "purpose": t.purpose,
            "from_place": t.from_place,
            "to_place": t.to_place,
            "route": t.route(),
            "miles": t.miles.to_string(),
            "entered_miles": t.entered_miles().round_dp(1).to_string(),
            "odometer_start": t.odometer_start.map(|d| d.to_string()),
            "odometer_end": t.odometer_end.map(|d| d.to_string()),
            "round_trip": t.round_trip,
            "rate_cents": t.rate_cents.to_string(),
            "amount": money(t.amount),
            "reimbursable": t.reimbursable,
            "driver": t.driver,
            "reimburse_rate_cents": t.reimburse_rate().to_string(),
            "reimburse_rate_custom": t.reimburse_rate_cents.map(|d| d.to_string()),
            "reimburse_amount": money(t.reimburse_amount),
            "category": t.category,
            "category_name": category_name,
            "tags": t.tags,
            "tag_names": crate::dimensions::tag_names(self.ctx, &t.tags)?,
            "status": t.status,
            "reported": t.reported,
            "source": t.source,
            "transaction": t.transaction,
            "reimbursement": t.reimbursement,
            "saved_trip": t.saved_trip,
            "client_id": t.client_id,
            "created_at": t.created_at,
            "notes": t.notes,
            "warnings": warnings(t, &self.rates, &self.trips),
        }))
    }
}

/// The name the default mileage expense account has (or will have).
fn default_category_name(ctx: &Ctx<'_>, chart: &Chart) -> CommandResult<String> {
    if let Some(id) = ms::get(ctx, "mileage.expense_account")?.as_str()
        && let Ok(a) = chart.resolve(id)
    {
        return Ok(chart.full_name(a.id));
    }
    Ok(format!("{EXPENSE_PARENT}:{EXPENSE_NAME}"))
}

/// The mileage expense account: the setting's, else Auto:Mileage reimbursement
/// (created on first use as an expense account under a top-level Auto).
fn expense_account(ctx: &Ctx<'_>) -> CommandResult<AccountId> {
    let chart = Chart::load(ctx)?;
    if let Some(id) = ms::get(ctx, "mileage.expense_account")?.as_str()
        && let Ok(a) = chart.resolve(id)
    {
        return Ok(a.id);
    }
    let full = format!("{EXPENSE_PARENT}:{EXPENSE_NAME}");
    if let Ok(a) = chart.resolve(&full)
        && a.account_type == AccountType::Expense
        && !a.placeholder
    {
        return Ok(a.id);
    }
    let parent = chart
        .accounts
        .iter()
        .find(|a| {
            a.parent.is_none()
                && a.name.eq_ignore_ascii_case(EXPENSE_PARENT)
                && a.account_type == AccountType::Expense
        })
        .map(|a| a.id);
    let parent = match parent {
        Some(p) => p,
        None => id_of(crate::accounts::create_json(
            ctx,
            json!({"name": EXPENSE_PARENT, "type": "expense"}),
        )?)?,
    };
    id_of(crate::accounts::create_json(
        ctx,
        json!({"name": EXPENSE_NAME, "type": "expense", "parent": parent.to_string()}),
    )?)
}

fn id_of(v: Value) -> CommandResult<AccountId> {
    v.get("id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| CommandError::Integrity("account without id".into()))
}

fn category_ref(chart: &Chart, r: &str) -> CommandResult<AccountId> {
    let a = chart.resolve(r.trim())?;
    if a.account_type != AccountType::Expense || a.placeholder {
        return Err(invalid(format!(
            "The category is an expense account; {} is not",
            chart.full_name(a.id)
        )));
    }
    Ok(a.id)
}

/// Fields of trip.create and trip.update.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TripArgs {
    /// trip.update only: the trip to change.
    #[serde(default)]
    trip: Option<String>,
    #[serde(default)]
    date: Option<Date>,
    /// A vehicle (id or name). With one active vehicle it is the default.
    #[serde(default)]
    vehicle: Option<String>,
    /// business, medical, moving, charity or personal (default the
    /// `mileage.default_purpose` setting).
    #[serde(default)]
    purpose_kind: Option<String>,
    /// The business or charitable purpose, at most 500 characters (required unless
    /// personal).
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    from_place: Option<String>,
    /// Required unless personal.
    #[serde(default)]
    to_place: Option<String>,
    /// Miles as entered (one way for a round trip), up to 1 decimal. Not needed when
    /// both odometer readings are given.
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    miles: Option<Decimal>,
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    odometer_start: Option<Decimal>,
    #[serde(default)]
    #[schemars(schema_with = "mk_core::schema::optional_decimal_string")]
    odometer_end: Option<Decimal>,
    /// trip.update: remove the odometer readings (miles are then typed).
    #[serde(default)]
    clear_odometer: bool,
    /// The stored miles are twice the one-way miles; off with odometer readings.
    #[serde(default)]
    round_trip: Option<bool>,
    /// Paid back through expense reports (needs the expenses module).
    #[serde(default)]
    reimbursable: Option<bool>,
    /// Who is paid back (default the book's name), at most 100 characters.
    #[serde(default)]
    driver: Option<String>,
    /// Cents per mile for the reimbursement; "" means the standard rate. Default the
    /// `mileage.reimbursement_rate` setting.
    #[serde(default)]
    reimburse_rate_cents: Option<String>,
    /// An expense account for a reimbursable trip; "" means the
    /// `mileage.expense_account` default.
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    fund: Option<String>,
    /// A project (module projects); "" clears.
    #[serde(default)]
    project: Option<String>,
    /// A billable trip (projects.md; default the project's setting).
    #[serde(default)]
    billable: Option<bool>,
    /// trip.create: fill the fields not given from this saved trip.
    #[serde(default)]
    saved_trip: Option<String>,
    /// `desktop` (default) or `phone`.
    #[serde(default)]
    source: Option<String>,
    /// The phone's id for the trip: a second create with the same id returns the
    /// trip already made instead of a copy.
    #[serde(default)]
    client_id: Option<String>,
    /// Save even when a trip with the same date, vehicle and miles exists.
    #[serde(default)]
    allow_duplicate: bool,
}

/// Apply arguments to a trip and check it; `old` is the stored trip on update.
fn apply(ctx: &Ctx<'_>, t: &mut TripDoc, a: TripArgs, old: Option<&TripDoc>) -> CommandResult<()> {
    let chart = Chart::load(ctx)?;
    if let Some(d) = a.date {
        t.date = d;
    }
    if let Some(v) = a.vehicle.as_deref() {
        t.vehicle = match v.trim() {
            "" => None,
            r => Some(find_vehicle(ctx, r)?.id),
        };
    }
    if let Some(k) = a.purpose_kind.as_deref() {
        t.purpose_kind = check_kind(k)?;
    }
    if let Some(p) = a.purpose {
        t.purpose = text(Some(p), MAX_PURPOSE, "purpose")?;
    }
    if let Some(f) = a.from_place {
        t.from_place = text(Some(f), MAX_PLACE, "from")?;
    }
    if let Some(p) = a.to_place {
        t.to_place = text(Some(p), MAX_PLACE, "to")?;
    }
    // Distance: odometer readings, else miles typed (one way for a round trip).
    let entered = t.entered_miles();
    if a.clear_odometer {
        t.odometer_start = None;
        t.odometer_end = None;
    }
    if let Some(s) = a.odometer_start {
        t.odometer_start = Some(check_reading(s)?);
    }
    if let Some(e) = a.odometer_end {
        t.odometer_end = Some(check_reading(e)?);
    }
    if let Some(r) = a.round_trip {
        t.round_trip = r;
    }
    match (t.odometer_start, t.odometer_end) {
        (Some(s), Some(e)) => {
            if e <= s {
                return Err(invalid(
                    "The odometer end reading must be more than the start reading",
                ));
            }
            // The round trip is already in the readings.
            t.round_trip = false;
            t.miles = e - s;
        }
        _ => {
            let one = match a.miles {
                Some(m) => check_miles(m, "Miles")?,
                None if old.is_some() || t.miles > Decimal::ZERO => entered,
                None => return Err(invalid("Enter the miles, or both odometer readings")),
            };
            let one = check_miles(one, "Miles")?;
            if t.round_trip && one > Decimal::from(1000) {
                return Err(invalid(
                    "A round trip is at most 1,000 miles one way (2,000 per trip)",
                ));
            }
            t.miles = if t.round_trip {
                one * Decimal::TWO
            } else {
                one
            };
        }
    }
    if t.miles > Decimal::from(2000) {
        return Err(invalid("A trip is at most 2,000 miles"));
    }
    let mut miles = t.miles;
    miles.rescale(1);
    t.miles = miles;
    if let Some(r) = a.reimbursable {
        t.reimbursable = r;
    }
    if t.reimbursable && !is_module_enabled(ctx.tx, crate::expenses::MODULE)? {
        return Err(rejected(
            "Reimbursable trips need the Expenses feature; turn it on in Settings → Features.",
        ));
    }
    if t.purpose_kind == "personal" && t.reimbursable {
        return Err(invalid("Personal trips are not reimbursed"));
    }
    if let Some(d) = a.driver {
        t.driver = text(Some(d), MAX_DRIVER, "driver")?;
    }
    if t.reimbursable && t.driver.is_none() {
        t.driver = Some(ctx.tx.info()?.name);
    }
    if let Some(r) = a.reimburse_rate_cents {
        t.reimburse_rate_cents = match r.trim() {
            "" => None,
            s => Some(check_cents(s)?),
        };
    }
    if let Some(c) = a.category {
        t.category = match c.trim() {
            "" => None,
            c => Some(category_ref(&chart, c)?),
        };
    }
    if a.class.is_some()
        || a.location.is_some()
        || a.fund.is_some()
        || a.project.is_some()
        || a.billable.is_some()
    {
        let pick = |given: Option<String>, cur: &Option<String>| given.or_else(|| cur.clone());
        let cur = t.tags.clone();
        let billable = a
            .billable
            .or_else(|| (a.project.is_none() && cur.project.is_some()).then_some(cur.billable));
        t.tags = crate::dimensions::resolve_tags_p(
            ctx,
            pick(a.class, &cur.class).as_deref(),
            pick(a.location, &cur.location).as_deref(),
            pick(a.fund, &cur.fund).as_deref(),
            pick(a.project, &cur.project).as_deref(),
            billable,
        )?;
    }
    // Required fields (a phone trip without a vehicle stays a draft).
    if t.purpose_kind != "personal" {
        if t.purpose.is_none() {
            return Err(invalid(
                "Enter the purpose of the trip (required unless personal)",
            ));
        }
        if t.to_place.is_none() {
            return Err(invalid(
                "Enter where the trip went (required unless personal)",
            ));
        }
    }
    if t.vehicle.is_none() && t.source != "phone" {
        return Err(invalid(
            "Choose a vehicle; add one with vehicle.create first",
        ));
    }
    // Duplicate check.
    let changed =
        old.is_none_or(|o| o.date != t.date || o.vehicle != t.vehicle || o.miles != t.miles);
    if changed && !a.allow_duplicate {
        let dup = all_trips(ctx)?.into_iter().any(|o| {
            o.id != t.id && o.date == t.date && o.vehicle == t.vehicle && o.miles == t.miles
        });
        if dup {
            return Err(CommandError::Coded {
                code: "duplicate_trip",
                message: DUPLICATE.into(),
            });
        }
    }
    compute(ctx, t)?;
    Ok(())
}

/// Rate, value and reimbursement amount from the book's rate table.
fn compute(ctx: &Ctx<'_>, t: &mut TripDoc) -> CommandResult<()> {
    let rates = all_rates(ctx)?;
    t.rate_cents = if t.purpose_kind == "personal" {
        Decimal::ZERO
    } else {
        rate_for(&rates, &t.purpose_kind, t.date).unwrap_or(Decimal::ZERO)
    };
    t.amount = value_of(t.miles, t.rate_cents);
    t.reimburse_amount = if t.reimbursable {
        value_of(t.miles, t.reimburse_rate())
    } else {
        Decimal::ZERO
    };
    t.refresh_status();
    Ok(())
}

fn source_of(s: Option<&str>) -> CommandResult<&'static str> {
    match s.map(str::trim) {
        None | Some("") | Some("desktop") => Ok("desktop"),
        Some("phone") => Ok("phone"),
        Some(o) => Err(invalid(format!("source is desktop or phone, not {o}"))),
    }
}

fn new_trip(ctx: &Ctx<'_>, date: Date, source: &str) -> CommandResult<TripDoc> {
    let mut kind = ms::get_str(ctx, "mileage.default_purpose")?;
    if kind.is_empty() {
        kind = "business".into();
    }
    let active: Vec<VehicleDoc> = all_vehicles(ctx)?
        .into_iter()
        .filter(|v| v.active)
        .collect();
    let reimburse_rate = ms::get(ctx, "mileage.reimbursement_rate")?
        .as_str()
        .and_then(|s| s.parse::<Decimal>().ok());
    Ok(TripDoc {
        id: new_id(),
        date,
        vehicle: match active.as_slice() {
            [one] => Some(one.id.clone()),
            _ => None,
        },
        purpose_kind: kind,
        purpose: None,
        from_place: None,
        to_place: None,
        miles: Decimal::ZERO,
        odometer_start: None,
        odometer_end: None,
        round_trip: false,
        rate_cents: Decimal::ZERO,
        amount: Decimal::ZERO,
        reimbursable: false,
        driver: None,
        reimburse_rate_cents: reimburse_rate,
        reimburse_amount: Decimal::ZERO,
        category: None,
        tags: SplitTags::default(),
        status: String::new(),
        reported: false,
        source: source.to_owned(),
        transaction: None,
        reimbursement: None,
        saved_trip: None,
        client_id: None,
        created_at: now_rfc3339()?,
        notes: Vec::new(),
    })
}

fn trip_create(ctx: &Ctx<'_>, mut a: TripArgs) -> CommandResult<Value> {
    if a.trip.is_some() {
        return Err(invalid("trip is only for trip.update"));
    }
    let source = source_of(a.source.as_deref())?;
    let client_id = text(a.client_id.take(), 100, "client_id")?;
    if let Some(c) = &client_id
        && let Some(t) = all_trips(ctx)?
            .into_iter()
            .find(|t| t.client_id.as_deref() == Some(c))
    {
        ctx.quiet_noop.set(true);
        let v = View::load(ctx)?;
        let mut out = v.json(&t)?;
        set_key(&mut out, "already_taken", json!(true));
        return Ok(out);
    }
    let date = match a.date {
        Some(d) => d,
        None => today_utc()?,
    };
    let mut t = new_trip(ctx, date, source)?;
    t.client_id = client_id;
    // A saved trip fills what was not given.
    if let Some(s) = a.saved_trip.take().filter(|s| !s.trim().is_empty()) {
        let s: SavedTripDoc = get(ctx, DocTable::SavedTrip, &s, "saved trip")?;
        t.saved_trip = Some(s.id.clone());
        a.from_place = a.from_place.or(s.from_place);
        a.to_place = a.to_place.or(s.to_place);
        a.purpose_kind = a.purpose_kind.or(Some(s.purpose_kind));
        a.purpose = a.purpose.or(s.purpose);
        if a.vehicle.is_none() {
            a.vehicle = s.vehicle;
        }
        if a.miles.is_none() && a.odometer_start.is_none() && a.odometer_end.is_none() {
            a.miles = Some(s.miles);
            a.round_trip = a.round_trip.or(Some(s.round_trip));
        }
    }
    apply(ctx, &mut t, a, None)?;
    save_trip(ctx, &t)?;
    View::load(ctx)?.json(&t)
}

fn trip_update(ctx: &Ctx<'_>, mut a: TripArgs) -> CommandResult<Value> {
    let id = a
        .trip
        .take()
        .ok_or_else(|| invalid("give the trip to change"))?;
    if a.saved_trip.is_some() || a.client_id.is_some() || a.source.is_some() {
        return Err(invalid(
            "saved_trip, source and client_id are only for trip.create",
        ));
    }
    let old = load_trip(ctx, &id)?;
    let mut t = old.clone();
    if old.status == "posted" && a.reimbursable == Some(false) {
        return Err(rejected(
            "Reimbursable can't change once the trip is posted; delete it and enter it again.",
        ));
    }
    apply(ctx, &mut t, a, Some(&old))?;
    let closed = ctx.tx.closing_date()?;
    if closed.is_some_and(|c| old.date <= c || t.date <= c) && t.transaction.is_some() {
        return Err(rejected(
            "The trip's transaction is in the closed period; it can't be changed",
        ));
    }
    if old.status == "posted" {
        // The posted amount follows the frozen reimbursement rate.
        if let Some(tx) = t.transaction {
            update_txn(ctx, &old, &t, tx)?;
        }
    }
    save_trip(ctx, &t)?;
    View::load(ctx)?.json(&t)
}

fn memo_of(t: &TripDoc) -> String {
    let mut m = format!(
        "Mileage: {} ({} mi × {}¢)",
        t.route(),
        t.miles,
        plain(t.reimburse_rate())
    );
    if let Some(p) = &t.purpose {
        m.push_str(" — ");
        m.push_str(p);
    }
    m.chars().take(MAX_PURPOSE).collect()
}

fn update_txn(ctx: &Ctx<'_>, old: &TripDoc, t: &TripDoc, id: TransactionId) -> CommandResult<()> {
    let before = ctx
        .tx
        .transaction(id)?
        .ok_or_else(|| not_found("transaction", &id.to_string()))?;
    if before.splits.len() != 2 {
        return Err(rejected(
            "This trip's transaction has more than two lines; change it in the register.",
        ));
    }
    let amount = t.reimburse_amount;
    if amount <= Decimal::ZERO {
        return Err(rejected("A posted trip needs an amount above zero."));
    }
    let mut after = before.clone();
    after.date = t.date;
    after.payee.clone_from(&t.driver);
    after.memo = Some(memo_of(t));
    for s in &mut after.splits {
        if s.value > Decimal::ZERO {
            if let Some(c) = t.category {
                s.account = c;
            }
            if t.tags != old.tags {
                s.tags = t.tags.clone();
            }
            s.value = amount;
            s.quantity = amount;
        } else {
            s.value = -amount;
            s.quantity = -amount;
        }
    }
    mk_ledger::sort_splits(&mut after.splits);
    validate_change(ctx, Some(&before), Some(&after))?;
    if after != before {
        ctx.save_txn(Some(&before), &after)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeleteArgs {
    trip: String,
    /// For a posted trip: true deletes its transaction too, false keeps it. Must be
    /// given for a posted trip.
    #[serde(default)]
    delete_transaction: Option<bool>,
}

fn trip_delete(ctx: &Ctx<'_>, a: DeleteArgs) -> CommandResult<Value> {
    let t = load_trip(ctx, &a.trip)?;
    let mut deleted_txn = None;
    match (t.transaction, a.delete_transaction) {
        (Some(_), None) => {
            return Err(CommandError::Coded {
                code: "needs_choice",
                message:
                    "This trip is posted. Delete its transaction too, or keep the transaction?"
                        .into(),
            });
        }
        (Some(id), Some(true)) => {
            if let Some(txn) = ctx.tx.transaction(id)? {
                if txn
                    .splits
                    .iter()
                    .any(|s| s.reconcile == Reconcile::Reconciled)
                {
                    return Err(rejected(
                        "The transaction has reconciled lines and can't be deleted; keep it.",
                    ));
                }
                validate_change(ctx, Some(&txn), None)?;
                ctx.remove_txn(&txn)?;
                crate::schedules::unlink_txn(ctx, id)?;
                deleted_txn = Some(id);
            }
        }
        _ => {}
    }
    ctx.save_doc(DocTable::Trip, &t.id, None)?;
    if let Some(id) = deleted_txn {
        crate::expenses::after_txn_delete(ctx, id)?;
        after_txn_delete(ctx, id)?;
    }
    Ok(json!({"deleted": t.id, "transaction_deleted": deleted_txn}))
}

/// A deleted transaction: trips posted to it become ready again; a reimbursement it
/// recorded is forgotten.
pub(crate) fn after_txn_delete(ctx: &Ctx<'_>, txn: TransactionId) -> CommandResult<()> {
    for mut t in all_trips(ctx)? {
        let mut changed = false;
        if t.transaction == Some(txn) {
            t.transaction = None;
            t.status = String::new();
            t.refresh_status();
            changed = true;
        }
        if t.reimbursement == Some(txn) {
            t.reimbursement = None;
            changed = true;
        }
        if changed {
            save_trip(ctx, &t)?;
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TripRef {
    trip: String,
}

fn trip_get(ctx: &Ctx<'_>, a: TripRef) -> CommandResult<Value> {
    let t = load_trip(ctx, &a.trip)?;
    View::load(ctx)?.json(&t)
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    #[serde(default)]
    from: Option<Date>,
    #[serde(default)]
    to: Option<Date>,
    #[serde(default)]
    vehicle: Option<String>,
    /// A purpose kind.
    #[serde(default)]
    kind: Option<String>,
    /// draft, logged, ready, posted, reimbursable (ready or posted) or reported.
    #[serde(default)]
    status: Option<String>,
    /// Matched against purpose, places and driver, ignoring case.
    #[serde(default)]
    text: Option<String>,
}

fn select(ctx: &Ctx<'_>, a: &ListArgs) -> CommandResult<Vec<TripDoc>> {
    let vehicle = a
        .vehicle
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .map(|v| find_vehicle(ctx, v).map(|x| x.id))
        .transpose()?;
    let kind = a
        .kind
        .as_deref()
        .filter(|k| !k.trim().is_empty() && *k != "all")
        .map(check_kind)
        .transpose()?;
    let status = a
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "all");
    if let Some(s) = status
        && !matches!(
            s,
            "draft" | "logged" | "ready" | "posted" | "reimbursable" | "reported"
        )
    {
        return Err(invalid(format!(
            "status is draft, logged, ready, posted, reimbursable or reported, not {s}"
        )));
    }
    let text = a
        .text
        .as_deref()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty());
    let mut out: Vec<TripDoc> = all_trips(ctx)?
        .into_iter()
        .filter(|t| a.from.is_none_or(|f| t.date >= f) && a.to.is_none_or(|x| t.date <= x))
        .filter(|t| vehicle.is_none() || t.vehicle == vehicle)
        .filter(|t| kind.as_ref().is_none_or(|k| &t.purpose_kind == k))
        .filter(|t| match status {
            None => true,
            Some("reimbursable") => t.reimbursable,
            Some("reported") => t.reported,
            Some(s) => t.status == s,
        })
        .filter(|t| {
            text.as_ref().is_none_or(|q| {
                [&t.purpose, &t.from_place, &t.to_place, &t.driver]
                    .iter()
                    .any(|f| f.as_deref().is_some_and(|v| v.to_lowercase().contains(q)))
            })
        })
        .collect();
    out.sort_by(|x, y| {
        x.date
            .cmp(&y.date)
            .then_with(|| x.created_at.cmp(&y.created_at))
    });
    Ok(out)
}

fn trip_list(ctx: &Ctx<'_>, a: ListArgs) -> CommandResult<Value> {
    let mut rows = select(ctx, &a)?;
    rows.reverse();
    let v = View::load(ctx)?;
    Ok(Value::Array(
        rows.iter()
            .map(|t| v.json(t))
            .collect::<CommandResult<Vec<_>>>()?,
    ))
}

fn trip_post(ctx: &Ctx<'_>, a: TripRef) -> CommandResult<Value> {
    if !is_module_enabled(ctx.tx, crate::expenses::MODULE)? {
        return Err(rejected(
            "Posting trips needs the Expenses feature; turn it on in Settings → Features.",
        ));
    }
    let mut t = load_trip(ctx, &a.trip)?;
    if t.transaction.is_some() {
        return Err(rejected("This trip is already posted."));
    }
    if !t.reimbursable {
        return Err(rejected(
            "Only reimbursable trips are posted; log-only trips stay out of the books.",
        ));
    }
    if t.vehicle.is_none() {
        return Err(rejected("Choose a vehicle before posting."));
    }
    require_open_period(ctx, t.date)?;
    // The book's table wins: recompute before posting.
    compute(ctx, &mut t)?;
    let amount = t.reimburse_amount;
    if amount <= Decimal::ZERO {
        return Err(rejected(
            "Nothing to reimburse: this trip's rate is 0. Set a reimbursement rate.",
        ));
    }
    let category = match t.category {
        Some(c) => c,
        None => expense_account(ctx)?,
    };
    let credit = ms::account_or_create_role(
        ctx,
        "expenses.reimbursement_account",
        Some(crate::chart::roles::REIMBURSEMENTS),
        "Reimbursements Payable",
        AccountType::Liability,
        AccountSubtype::Payable,
    )?;
    let split = |account: AccountId, value: Decimal, tags: SplitTags| Split {
        account,
        value,
        quantity: value,
        memo: None,
        reconcile: Reconcile::New,
        fingerprint: None,
        tags,
    };
    // The reimbursement rate is fixed once the trip is in the books.
    t.reimburse_rate_cents = Some(t.reimburse_rate());
    t.category = Some(category);
    let id = crate::sales::post_txn(
        ctx,
        None,
        t.date,
        t.driver.as_deref().unwrap_or_default(),
        Some(memo_of(&t)),
        vec![
            split(category, amount, t.tags.clone()),
            split(credit, -amount, SplitTags::default()),
        ],
    )?;
    t.transaction = Some(id);
    t.status = "posted".into();
    save_trip(ctx, &t)?;
    View::load(ctx)?.json(&t)
}

// ----- re-apply rates ---------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReapplyArgs {
    from: Date,
    to: Date,
    /// true: report what would change and change nothing.
    #[serde(default)]
    preview: bool,
}

fn reapply_rates(ctx: &Ctx<'_>, a: ReapplyArgs) -> CommandResult<Value> {
    if a.to < a.from {
        return Err(invalid("the period ends before it starts"));
    }
    ctx.quiet_noop.set(true);
    let rates = all_rates(ctx)?;
    let mut changes = Vec::new();
    let mut difference = Decimal::ZERO;
    for t in all_trips(ctx)?
        .into_iter()
        .filter(|t| t.date >= a.from && t.date <= a.to)
    {
        let rate = if t.purpose_kind == "personal" {
            Decimal::ZERO
        } else {
            rate_for(&rates, &t.purpose_kind, t.date).unwrap_or(Decimal::ZERO)
        };
        if rate == t.rate_cents {
            continue;
        }
        let mut n = t.clone();
        n.rate_cents = rate;
        n.amount = value_of(n.miles, rate);
        // Unposted reimbursable trips at the standard rate follow it; posted trips
        // keep the amount that is in the books.
        if n.reimbursable && n.transaction.is_none() {
            n.reimburse_amount = value_of(n.miles, n.reimburse_rate());
        }
        difference += n.amount - t.amount;
        changes.push(json!({
            "trip": t.id, "date": t.date,
            "old_rate_cents": t.rate_cents.to_string(), "new_rate_cents": rate.to_string(),
            "old_amount": money(t.amount), "new_amount": money(n.amount),
        }));
        if !a.preview {
            save_trip(ctx, &n)?;
        }
    }
    Ok(json!({
        "preview": a.preview,
        "trips": changes.len(),
        "difference": money(difference),
        "changes": changes,
    }))
}

// ----- summary ----------------------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SummaryArgs {
    /// A tax year; or give from and to.
    #[serde(default)]
    year: Option<i32>,
    #[serde(default)]
    from: Option<Date>,
    #[serde(default)]
    to: Option<Date>,
    #[serde(default)]
    vehicle: Option<String>,
    /// Only these purpose kinds (all when absent).
    #[serde(default)]
    kinds: Option<Vec<String>>,
}

fn period(a: &SummaryArgs) -> CommandResult<(Date, Date)> {
    match (a.year, a.from, a.to) {
        (Some(y), None, None) => Ok((Date::from_ymd(y, 1, 1)?, Date::from_ymd(y, 12, 31)?)),
        (None, Some(f), Some(t)) if f <= t => Ok((f, t)),
        (None, Some(_), Some(_)) => Err(invalid("the period ends before it starts")),
        _ => Err(invalid("give a year, or from and to")),
    }
}

/// Totals per vehicle and kind for a period (the report's first part and the Tax
/// summary's Mileage section).
pub(crate) fn summary_data(
    ctx: &Ctx<'_>,
    from: Date,
    to: Date,
    vehicle: Option<&str>,
    kinds: Option<&[String]>,
) -> CommandResult<(Value, Vec<TripDoc>)> {
    let vehicle = vehicle
        .filter(|v| !v.trim().is_empty())
        .map(|v| find_vehicle(ctx, v).map(|x| x.id))
        .transpose()?;
    let kinds: Option<Vec<String>> = kinds
        .map(|ks| {
            ks.iter()
                .map(|k| check_kind(k))
                .collect::<CommandResult<_>>()
        })
        .transpose()?;
    let trips: Vec<TripDoc> = select(
        ctx,
        &ListArgs {
            from: Some(from),
            to: Some(to),
            ..ListArgs::default()
        },
    )?
    .into_iter()
    .filter(|t| vehicle.is_none() || t.vehicle == vehicle)
    .collect();
    let shown: Vec<TripDoc> = trips
        .iter()
        .filter(|t| kinds.as_ref().is_none_or(|ks| ks.contains(&t.purpose_kind)))
        .cloned()
        .collect();
    // The odometer of the year applies when the period is within one year.
    let year = (from.year() == to.year()).then(|| year_of(from.year()));
    let mut out = Vec::new();
    let mut totals: BTreeMap<usize, (Decimal, Decimal)> = BTreeMap::new();
    for v in all_vehicles(ctx)? {
        if vehicle.as_ref().is_some_and(|x| *x != v.id) {
            continue;
        }
        let mine: Vec<&TripDoc> = trips
            .iter()
            .filter(|t| t.vehicle.as_deref() == Some(&v.id))
            .collect();
        let odo = year.as_ref().and_then(|y| v.odometer.get(y)).cloned();
        if mine.is_empty() && odo.is_none() {
            continue;
        }
        let trip_miles: Decimal = mine.iter().map(|t| t.miles).sum();
        let odo_miles = odo.as_ref().and_then(|o| match (o.start, o.end) {
            (Some(s), Some(e)) => Some(e - s),
            _ => None,
        });
        let total = odo_miles.unwrap_or(trip_miles);
        let mut by_kind: BTreeMap<usize, (Decimal, Decimal)> = BTreeMap::new();
        for t in &mine {
            if kinds
                .as_ref()
                .is_some_and(|ks| !ks.contains(&t.purpose_kind))
            {
                continue;
            }
            let e = by_kind.entry(kind_rank(&t.purpose_kind)).or_default();
            e.0 += t.miles;
            if t.purpose_kind != "personal" {
                e.1 += t.amount;
            }
        }
        let rows: Vec<Value> = by_kind
            .iter()
            .map(|(rank, (m, val))| {
                let k = KIND_CHOICES.get(*rank).map_or("", |x| x.0);
                let e = totals.entry(*rank).or_default();
                e.0 += *m;
                e.1 += *val;
                json!({"kind": k, "label": kind_label(k), "miles": m.to_string(),
                       "value": money(*val), "percent": percent(*m, total)})
            })
            .collect();
        out.push(json!({
            "vehicle": v.id,
            "name": v.name,
            "odometer_start": odo.as_ref().and_then(|o| o.start).map(|d| d.to_string()),
            "odometer_end": odo.as_ref().and_then(|o| o.end).map(|d| d.to_string()),
            "odometer_miles": odo_miles.map(|d| d.to_string()),
            "trip_miles": trip_miles.to_string(),
            "total_miles": total.to_string(),
            "kinds": rows,
        }));
    }
    let totals: Vec<Value> = totals
        .iter()
        .map(|(rank, (m, val))| {
            let k = KIND_CHOICES.get(*rank).map_or("", |x| x.0);
            json!({"kind": k, "label": kind_label(k), "miles": m.to_string(), "value": money(*val)})
        })
        .collect();
    let deduction: Decimal = shown
        .iter()
        .filter(|t| t.purpose_kind != "personal")
        .map(|t| t.amount)
        .sum();
    Ok((
        json!({
            "from": from, "to": to, "year": year,
            "vehicles": out, "totals": totals,
            "trips": shown.len(),
            "deduction_value": money(deduction),
        }),
        shown,
    ))
}

fn summary(ctx: &Ctx<'_>, a: SummaryArgs) -> CommandResult<Value> {
    let (from, to) = period(&a)?;
    Ok(summary_data(ctx, from, to, a.vehicle.as_deref(), a.kinds.as_deref())?.0)
}

/// The Tax summary's Mileage section for a period, or None when there are no trips
/// (or the module is off).
pub(crate) fn tax_section(ctx: &Ctx<'_>, from: Date, to: Date) -> CommandResult<Option<Value>> {
    if !is_module_enabled(ctx.tx, MODULE)? {
        return Ok(None);
    }
    let (data, trips) = summary_data(ctx, from, to, None, None)?;
    if trips.is_empty() {
        return Ok(None);
    }
    let pick = |rows: &Value| -> Vec<Value> {
        rows.as_array()
            .map(|r| {
                r.iter()
                    .filter(|x| {
                        let k = x.get("kind").and_then(Value::as_str).unwrap_or_default();
                        matches!(k, "business" | "medical" | "charity")
                            || (k == "moving"
                                && x.get("miles").and_then(Value::as_str) != Some("0.0"))
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    };
    let vehicles: Vec<Value> = data
        .get("vehicles")
        .and_then(Value::as_array)
        .map(|vs| {
            vs.iter()
                .map(|v| {
                    json!({"vehicle": at(v, "vehicle"), "name": at(v, "name"),
                           "rows": pick(at(v, "kinds"))})
                })
                .filter(|v| at(v, "rows").as_array().is_some_and(|r| !r.is_empty()))
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(json!({
        "title": "Mileage (standard rates)",
        "note": TAX_NOTE,
        "vehicles": vehicles,
        "totals": pick(at(&data, "totals")),
    })))
}

// ----- report -----------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReportArgs {
    from: Date,
    to: Date,
    #[serde(default)]
    vehicle: Option<String>,
    /// Only these purpose kinds (all when absent).
    #[serde(default)]
    kinds: Option<Vec<String>>,
    /// `pdf` (default) or `csv`.
    #[serde(default)]
    format: Option<String>,
}

fn report(ctx: &Ctx<'_>, a: ReportArgs) -> CommandResult<Value> {
    if a.to < a.from {
        return Err(invalid("the period ends before it starts"));
    }
    let format = match a.format.as_deref().map(str::trim) {
        None | Some("") | Some("pdf") => "pdf",
        Some("csv") => "csv",
        Some(o) => return Err(invalid(format!("format is pdf or csv, not {o}"))),
    };
    let (data, trips) = summary_data(ctx, a.from, a.to, a.vehicle.as_deref(), a.kinds.as_deref())?;
    let names = vehicle_names(ctx)?;
    let mut groups: BTreeMap<usize, Vec<&TripDoc>> = BTreeMap::new();
    for t in &trips {
        groups
            .entry(kind_rank(&t.purpose_kind))
            .or_default()
            .push(t);
    }
    let file = format!("mileage-report-{}-to-{}", a.from, a.to);
    let vname = |t: &TripDoc| {
        t.vehicle
            .as_ref()
            .and_then(|v| names.get(v))
            .cloned()
            .unwrap_or_default()
    };
    if format == "csv" {
        let h = |s: &str| s.to_owned();
        let mut c = crate::reports::Csv::new(&[
            h("Section"),
            h("Vehicle"),
            h("Purpose kind"),
            h("Date"),
            h("Route"),
            h("Purpose"),
            h("Miles"),
            h("Rate (cents)"),
            h("Amount"),
            h("Percent"),
            h("Odometer start"),
            h("Odometer end"),
        ]);
        let st = |v: &Value| v.as_str().unwrap_or_default().to_owned();
        for v in at(&data, "vehicles")
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            c.row(&[
                h("Vehicle"),
                st(at(&v, "name")),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                st(at(&v, "total_miles")),
                String::new(),
                String::new(),
                String::new(),
                st(at(&v, "odometer_start")),
                st(at(&v, "odometer_end")),
            ]);
            for k in at(&v, "kinds").as_array().cloned().unwrap_or_default() {
                c.row(&[
                    h("Vehicle by kind"),
                    st(at(&v, "name")),
                    st(at(&k, "label")),
                    String::new(),
                    String::new(),
                    String::new(),
                    st(at(&k, "miles")),
                    String::new(),
                    st(at(&k, "value")),
                    st(at(&k, "percent")),
                ]);
            }
        }
        for (rank, list) in &groups {
            let label = KIND_CHOICES.get(*rank).map_or("", |x| x.1);
            let (mut m, mut val) = (Decimal::ZERO, Decimal::ZERO);
            for t in list {
                m += t.miles;
                val += t.amount;
                c.row(&[
                    h("Trip"),
                    vname(t),
                    label.to_owned(),
                    t.date.to_string(),
                    t.route(),
                    t.purpose.clone().unwrap_or_default(),
                    t.miles.to_string(),
                    t.rate_cents.to_string(),
                    money(t.amount),
                    String::new(),
                    t.odometer_start.map(|d| d.to_string()).unwrap_or_default(),
                    t.odometer_end.map(|d| d.to_string()).unwrap_or_default(),
                ]);
            }
            c.row(&[
                h("Subtotal"),
                String::new(),
                label.to_owned(),
                String::new(),
                String::new(),
                String::new(),
                m.to_string(),
                String::new(),
                money(val),
            ]);
        }
        for k in at(&data, "totals").as_array().cloned().unwrap_or_default() {
            if *at(&k, "kind") == "personal" {
                continue;
            }
            c.row(&[
                h("Total"),
                String::new(),
                st(at(&k, "label")),
                String::new(),
                String::new(),
                String::new(),
                st(at(&k, "miles")),
                String::new(),
                st(at(&k, "value")),
            ]);
        }
        return Ok(json!({
            "file_name": format!("{file}.csv"),
            "content": c.out,
            "trips": trips.len(),
            "warnings": [],
        }));
    }
    let info = ctx.tx.info()?;
    let r = crate::mileage_pdf::Report {
        book: info.name,
        period: format!("{} to {}", a.from, a.to),
        generated: today_utc()?.to_string(),
        vehicles: at(&data, "vehicles")
            .as_array()
            .cloned()
            .unwrap_or_default(),
        groups: groups
            .iter()
            .map(|(rank, list)| crate::mileage_pdf::Group {
                label: KIND_CHOICES.get(*rank).map_or("", |x| x.1).to_owned(),
                rows: list
                    .iter()
                    .map(|t| crate::mileage_pdf::Row {
                        date: t.date.to_string(),
                        vehicle: vname(t),
                        route: t.route(),
                        purpose: t.purpose.clone().unwrap_or_default(),
                        miles: miles_text(t.miles),
                        rate: plain(t.rate_cents),
                        amount: crate::sales::group(&money(t.amount)),
                    })
                    .collect(),
                miles: miles_text(list.iter().map(|t| t.miles).sum()),
                amount: crate::sales::group(&money(list.iter().map(|t| t.amount).sum())),
            })
            .collect(),
        totals: at(&data, "totals")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|k| *at(k, "kind") != "personal")
            .map(|k| {
                (
                    at(k, "label").as_str().unwrap_or_default().to_owned(),
                    miles_text(
                        at(k, "miles")
                            .as_str()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or_default(),
                    ),
                    crate::sales::group(at(k, "value").as_str().unwrap_or_default()),
                )
            })
            .collect(),
        total_value: crate::sales::group(at(&data, "deduction_value").as_str().unwrap_or_default()),
    };
    let (bytes, replaced) = crate::mileage_pdf::render(&r);
    let mut out = crate::sales::pdf_answer(&file, bytes, &replaced);
    set_key(&mut out, "trips", json!(trips.len()));
    Ok(out)
}

// ----- expenses and the phone -------------------------------------------------------

/// Posted reimbursable trips in a period (expense.report), when the module is on.
pub(crate) fn report_trips(ctx: &Ctx<'_>, from: Date, to: Date) -> CommandResult<Vec<TripDoc>> {
    if !is_module_enabled(ctx.tx, MODULE)? {
        return Ok(Vec::new());
    }
    Ok(all_trips(ctx)?
        .into_iter()
        .filter(|t| t.date >= from && t.date <= to && t.reimbursable && t.status == "posted")
        .collect())
}

/// A vehicle's name ("" when unknown).
pub(crate) fn vehicle_name(ctx: &Ctx<'_>, id: Option<&str>) -> CommandResult<String> {
    Ok(id
        .and_then(|i| vehicle_names(ctx).ok()?.get(i).cloned())
        .unwrap_or_default())
}

/// A trip submitted from the phone (mobile-capture.md revision 11), as plain fields
/// for the capture collector to fill from its own submission type.
#[derive(Debug, Clone, Default)]
pub(crate) struct PhoneTrip {
    pub client_id: String,
    pub date: Option<String>,
    pub vehicle_id: Option<String>,
    pub purpose_kind: Option<String>,
    pub purpose: Option<String>,
    pub from_place: Option<String>,
    pub to_place: Option<String>,
    pub miles: Option<String>,
    pub odometer_start: Option<String>,
    pub odometer_end: Option<String>,
    pub round_trip: bool,
    pub reimbursable: bool,
    pub saved_trip_id: Option<String>,
    /// Class, location and fund names (revision 2).
    pub tags: mk_capture_api::Tags,
}

/// What a phone trip became.
#[derive(Debug, Clone)]
pub(crate) struct PhoneTripResult {
    pub id: String,
    pub status: String,
    pub notes: Vec<String>,
    /// The same client_id was taken before; nothing new was made.
    pub already_taken: bool,
}

/// Take a trip from the phone into the book: the book's rate table wins; an unknown
/// or inactive vehicle leaves the trip a draft with "Choose a vehicle"; anything else
/// the book cannot use is dropped with a note, so the trip is never refused.
pub(crate) fn create_from_phone(
    ctx: &Ctx<'_>,
    p: &PhoneTrip,
    today: Date,
) -> CommandResult<PhoneTripResult> {
    if let Some(t) = all_trips(ctx)?
        .into_iter()
        .find(|t| t.client_id.as_deref() == Some(p.client_id.as_str()))
    {
        return Ok(PhoneTripResult {
            id: t.id,
            status: t.status,
            notes: t.notes,
            already_taken: true,
        });
    }
    let mut notes = Vec::new();
    let date = match p.date.as_deref().map(str::parse::<Date>) {
        Some(Ok(d)) => d,
        Some(Err(_)) => {
            notes.push(format!(
                "Date from phone not usable: {}",
                p.date.as_deref().unwrap_or_default()
            ));
            today
        }
        None => today,
    };
    let mut t = new_trip(ctx, date, "phone")?;
    t.client_id = Some(p.client_id.chars().take(100).collect());
    let vehicles = all_vehicles(ctx)?;
    t.vehicle = p
        .vehicle_id
        .as_deref()
        .and_then(|id| vehicles.iter().find(|v| v.id == id && v.active))
        .map(|v| v.id.clone());
    if t.vehicle.is_none() {
        notes.push(CHOOSE_VEHICLE.to_owned());
    }
    let clean = |v: &Option<String>, max: usize| {
        v.as_deref()
            .map(|s| mk_core::text::clean(s, max).text)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    };
    if let Some(k) = p.purpose_kind.as_deref() {
        match check_kind(k) {
            Ok(k) => t.purpose_kind = k,
            Err(_) => notes.push(format!("Purpose kind from phone not known: {k}")),
        }
    }
    t.purpose = clean(&p.purpose, MAX_PURPOSE);
    // Revision 2: the tags chosen on the phone, by name.
    t.tags = crate::expenses::phone_tags(ctx, &p.tags, &mut notes);
    t.from_place = clean(&p.from_place, MAX_PLACE);
    t.to_place = clean(&p.to_place, MAX_PLACE);
    t.saved_trip = p.saved_trip_id.clone().filter(|s| {
        all_saved(ctx)
            .map(|l| l.iter().any(|x| &x.id == s))
            .unwrap_or(false)
    });
    let num = |s: &Option<String>| s.as_deref().and_then(|x| x.trim().parse::<Decimal>().ok());
    let (os, oe) = (
        num(&p.odometer_start).and_then(|d| check_reading(d).ok()),
        num(&p.odometer_end).and_then(|d| check_reading(d).ok()),
    );
    match (os, oe) {
        (Some(s), Some(e)) if e > s && e - s <= Decimal::from(2000) => {
            t.odometer_start = Some(s);
            t.odometer_end = Some(e);
            t.miles = e - s;
        }
        _ => {
            let one = num(&p.miles).and_then(|m| check_miles(m, "Miles").ok());
            t.round_trip = p.round_trip;
            match one {
                Some(m)
                    if (!p.round_trip && m <= Decimal::from(2000))
                        || (p.round_trip && m <= Decimal::from(1000)) =>
                {
                    t.miles = if p.round_trip { m * Decimal::TWO } else { m };
                }
                _ => {
                    notes.push(format!(
                        "Miles from phone not usable: {}",
                        p.miles.as_deref().unwrap_or_default()
                    ));
                    t.vehicle = None;
                    t.miles = Decimal::ONE;
                }
            }
        }
    }
    t.miles.rescale(1);
    t.reimbursable = p.reimbursable
        && t.purpose_kind != "personal"
        && is_module_enabled(ctx.tx, crate::expenses::MODULE)?;
    if p.reimbursable && !t.reimbursable {
        notes.push("Reimbursable from phone not available in this book".into());
    }
    if t.reimbursable {
        t.driver = Some(ctx.tx.info()?.name);
    }
    if t.purpose_kind != "personal" && (t.purpose.is_none() || t.to_place.is_none()) {
        // Incomplete: kept for the user to finish.
        notes.push("Purpose or destination missing".into());
        t.vehicle = None;
    }
    t.notes = notes;
    compute(ctx, &mut t)?;
    save_trip(ctx, &t)?;
    Ok(PhoneTripResult {
        id: t.id,
        status: t.status,
        notes: t.notes,
        already_taken: false,
    })
}

/// The mileage lists the phone receives (mobile-capture.md revision 11).
pub(crate) fn phone_lists(ctx: &Ctx<'_>) -> CommandResult<Value> {
    let enabled = is_module_enabled(ctx.tx, MODULE)?;
    if !enabled {
        return Ok(json!({"mileage_enabled": false}));
    }
    let names = vehicle_names(ctx)?;
    let show_moving = ms::get_bool(ctx, "mileage.show_moving")?;
    let kinds: Vec<Value> = KIND_CHOICES
        .iter()
        .filter(|(k, _)| *k != "moving" || show_moving)
        .map(|(k, l)| json!({"kind": k, "label": l}))
        .collect();
    let mut rates = all_rates(ctx)?;
    rates.sort_by_key(|r| (r.from, kind_rank(&r.kind)));
    Ok(json!({
        "mileage_enabled": true,
        "reimbursable_enabled": is_module_enabled(ctx.tx, crate::expenses::MODULE)?,
        "default_purpose": ms::get_str(ctx, "mileage.default_purpose")?,
        "purpose_kinds": kinds,
        "vehicles": all_vehicles(ctx)?.iter().filter(|v| v.active)
            .map(|v| json!({"id": v.id, "name": v.name})).collect::<Vec<_>>(),
        "saved_trips": all_saved(ctx)?.iter().map(|s| saved_json(s, &names)).collect::<Vec<_>>(),
        "mileage_rates": rates.iter().map(rate_json).collect::<Vec<_>>(),
    }))
}
