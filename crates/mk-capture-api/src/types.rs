//! Request and response bodies (JSON, RFC 8259).

use serde::{Deserialize, Serialize};

/// `POST /api/v1/pair`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
    #[serde(default)]
    pub device_name: String,
    /// The Capture PIN (mobile-capture.md revision 10): required on the Wi-Fi address,
    /// and on the Cloudflare address once a PIN is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
}

/// What the Pair screen needs to know before pairing (`GET /api/v1/pair-info`,
/// revision 10): whether the PIN is asked, and which way in this address is.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PairInfo {
    pub pin_required: bool,
    /// `wifi` or `cloudflare`.
    pub way: String,
    /// On the Wi-Fi address: the computer's Cloudflare address, when phone capture
    /// over Cloudflare is set up too (revision 18: **Use Cloudflare** on the loading
    /// screen when the Wi-Fi address doesn't answer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloudflare_url: Option<String>,
}

/// The desktop sets the Capture PIN (`PUT /api/v1/desktop/pin`, revision 10). The
/// server keeps only its Argon2id hash.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SetPin {
    pub pin: String,
}

impl core::fmt::Debug for SetPin {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SetPin(hidden)")
    }
}

/// The Wi-Fi listener as the server has it (`GET /api/v1/desktop/wifi`, and in
/// `desktop/status`; revision 10). Public facts only: no key, no PIN.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WifiState {
    pub enabled: bool,
    /// The IPv4 address the phones use.
    #[serde(default)]
    pub address: String,
    /// The optional local name (e.g. `moneykeep.home.arpa`).
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub https_port: u16,
    #[serde(default)]
    pub http_port: u16,
    /// The CA's SHA-256 fingerprint, 8 groups of 4 hex digits.
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub ca_not_after: i64,
    #[serde(default)]
    pub cert_not_before: i64,
    #[serde(default)]
    pub cert_not_after: i64,
    #[serde(default)]
    pub pin_set: bool,
    /// This computer still has the address (an adapter switched off or another
    /// network takes it away).
    #[serde(default)]
    pub address_present: bool,
    /// The Wi-Fi listener answers (it started on the address).
    #[serde(default)]
    pub listening: bool,
}

/// The answer to a pairing: the phone's bearer token (shown to the phone only, once).
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PairResponse {
    pub device_id: String,
    pub device_token: String,
}

impl std::fmt::Debug for PairResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairResponse")
            .field("device_id", &self.device_id)
            .field("device_token", &"(hidden)")
            .finish()
    }
}

/// Tags of an expense (names, resolved by the desktop).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Tags {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fund: Option<String>,
    /// projects.md: the project (an open project's name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl Tags {
    pub fn is_empty(&self) -> bool {
        self.class.is_none()
            && self.location.is_none()
            && self.fund.is_none()
            && self.project.is_none()
    }
}

/// The fields of an expense as typed on the phone. Every field is optional: a
/// submission with no fields and at least one receipt is a receipt-only submission.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExpenseFields {
    /// `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merchant: Option<String>,
    /// A decimal string such as `45.20`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paid_with: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    #[serde(default, skip_serializing_if = "Tags::is_empty")]
    pub tags: Tags,
}

impl ExpenseFields {
    /// No field filled in.
    pub fn is_empty(&self) -> bool {
        self.date.is_none()
            && self.merchant.is_none()
            && self.amount.is_none()
            && self.category.is_none()
            && self.paid_with.is_none()
            && self.purpose.is_none()
            && self.tags.is_empty()
    }

    /// Fields other than the date filled in (the date alone does not make an
    /// expense: the form always fills it).
    pub fn has_expense_data(&self) -> bool {
        self.merchant.is_some()
            || self.amount.is_some()
            || self.category.is_some()
            || self.paid_with.is_some()
            || self.purpose.is_some()
            || !self.tags.is_empty()
    }
}

/// `POST /api/v1/expenses`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NewExpense {
    /// A UUID chosen by the phone; a retry with the same id creates nothing new.
    pub client_id: String,
    #[serde(flatten)]
    pub fields: ExpenseFields,
    #[serde(default)]
    pub receipt_ids: Vec<String>,
}

/// `PATCH /api/v1/expenses/{id}`: replaces the fields and receipts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExpensePatch {
    #[serde(flatten)]
    pub fields: ExpenseFields,
    #[serde(default)]
    pub receipt_ids: Vec<String>,
}

/// Where a submission is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionStatus {
    Waiting,
    Collected,
}

/// A submission as the phone sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission {
    pub id: String,
    pub client_id: String,
    pub status: SubmissionStatus,
    /// Unix seconds.
    pub created_at: i64,
    #[serde(default)]
    pub collected_at: Option<i64>,
    #[serde(flatten)]
    pub fields: ExpenseFields,
    #[serde(default)]
    pub receipt_ids: Vec<String>,
}

/// `GET /api/v1/expenses`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExpenseList {
    pub waiting: Vec<Submission>,
    pub collected: Vec<Submission>,
}

/// `POST /api/v1/receipts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptCreated {
    pub receipt_id: String,
    /// `jpeg` or `png`.
    pub kind: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
}

/// Tag names per dimension.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TagLists {
    #[serde(default)]
    pub class: Vec<String>,
    #[serde(default)]
    pub location: Vec<String>,
    #[serde(default)]
    pub fund: Vec<String>,
    /// projects.md: open projects (completed and cancelled ones are not offered).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project: Vec<String>,
}

/// The lists document the desktop publishes (`PUT /api/v1/desktop/lists`) and the
/// phone reads (`GET /api/v1/lists`). Names only: never balances, account numbers or
/// transactions. Fields a later version adds are ignored (upgrade-resilience-1 U1.4:
/// every new field has a default).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Lists {
    /// The book the lists came from (a collect into another book warns).
    #[serde(default)]
    pub book_id: String,
    /// The book profile's name.
    #[serde(default)]
    pub book_name: String,
    #[serde(default)]
    pub base_currency: String,
    /// Expense categories (full account names).
    #[serde(default)]
    pub categories: Vec<String>,
    /// Paid-with choices (account names and "Personal funds (reimbursable)").
    #[serde(default)]
    pub paid_with: Vec<String>,
    /// Recent merchants, most recent first.
    #[serde(default)]
    pub merchants: Vec<String>,
    #[serde(default)]
    pub tags: TagLists,
    /// Unix seconds; set by the server when stored.
    #[serde(default)]
    pub published_at: Option<i64>,
    // Mileage (mobile-capture.md revision 11). Left out of the document when the
    // book's mileage module is off, so an older server still accepts it; the
    // server sends them only to a phone that asks (`GET /lists?mileage=1`), so an
    // older phone app still reads its lists.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mileage_enabled: bool,
    /// The book's expenses module is on (trips can be reimbursable).
    #[serde(default, skip_serializing_if = "is_false")]
    pub reimbursable_enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_purpose: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub purpose_kinds: Vec<crate::PurposeKind>,
    /// Active vehicles only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vehicles: Vec<crate::ListVehicle>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub saved_trips: Vec<crate::ListSavedTrip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mileage_rates: Vec<crate::ListRate>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Lists {
    /// Whether any mileage part is filled in.
    pub fn has_mileage(&self) -> bool {
        self.mileage_enabled
            || self.reimbursable_enabled
            || !self.default_purpose.is_empty()
            || !self.purpose_kinds.is_empty()
            || !self.vehicles.is_empty()
            || !self.saved_trips.is_empty()
            || !self.mileage_rates.is_empty()
    }

    /// The document without its mileage parts (for an older server or phone).
    pub fn without_mileage(&self) -> Lists {
        Lists {
            book_id: self.book_id.clone(),
            book_name: self.book_name.clone(),
            base_currency: self.base_currency.clone(),
            categories: self.categories.clone(),
            paid_with: self.paid_with.clone(),
            merchants: self.merchants.clone(),
            tags: self.tags.clone(),
            published_at: self.published_at,
            ..Lists::default()
        }
    }
}

/// A stored receipt in the desktop's inbox listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiptMeta {
    pub id: String,
    pub kind: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    /// SHA-256 (hex) of the stored bytes.
    pub sha256: String,
}

/// A waiting submission (`GET /api/v1/desktop/inbox`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxItem {
    pub id: String,
    pub client_id: String,
    pub device_id: String,
    pub device_name: String,
    /// Unix seconds.
    pub submitted_at: i64,
    pub fields: ExpenseFields,
    pub receipts: Vec<ReceiptMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Inbox {
    pub items: Vec<InboxItem>,
    /// Waiting trips (mobile-capture.md revision 11); a desktop from before
    /// mileage ignores them and they keep waiting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trips: Vec<crate::TripInboxItem>,
    /// The book id of the published lists, if any.
    #[serde(default)]
    pub lists_book_id: Option<String>,
}

/// `POST /api/v1/desktop/pairing-codes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingCode {
    pub code: String,
    /// Unix seconds.
    pub expires_at: i64,
    /// Seconds from now.
    pub expires_in: i64,
    /// The phone page's address, from `client_url` in `server.toml`.
    #[serde(default)]
    pub client_url: Option<String>,
}

/// A paired phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// Unix seconds.
    pub paired_at: i64,
    #[serde(default)]
    pub last_seen: Option<i64>,
    /// The phone's way in (revision 10): `wifi` or `cloudflare`.
    #[serde(default)]
    pub via: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Devices {
    pub devices: Vec<Device>,
}

/// `GET /api/v1/health`: only whether the server runs and its version.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub version: String,
    /// The capture version (release-packaging.md); absent from servers built
    /// before it, which the desktop compares by `version` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_version: Option<String>,
    /// upgrade-resilience-1 U1.4: the server's API level ([`crate::API_LEVEL`]); 0
    /// from servers built before it.
    #[serde(default)]
    pub api_level: u32,
    /// U1.2: Wi-Fi capture is set up but could not start (its certificate files
    /// are damaged); the server runs without it. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wifi_error: Option<String>,
    /// L8: whether the Wi-Fi listener answers now; absent when Wi-Fi capture is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wifi_listening: Option<bool>,
}

/// `GET /api/v1/desktop/status` (desktop token): what the status light's tooltip
/// shows.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DesktopStatus {
    /// Submissions waiting to be collected.
    pub waiting: usize,
    /// Unix seconds of the newest submission received, if any.
    #[serde(default)]
    pub last_activity: Option<i64>,
    pub version: String,
    /// The latest request Cloudflare Access refused, until a token passes again
    /// (mobile-capture.md revision 8).
    #[serde(default)]
    pub access_problem: Option<AccessProblem>,
    /// The Wi-Fi listener, when Wi-Fi capture is on (revision 10).
    #[serde(default)]
    pub wifi: Option<WifiState>,
}

/// Why the capture server refused a phone's Cloudflare sign-in: the log text (no
/// token contents) and when (Unix seconds).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AccessProblem {
    pub reason: String,
    pub at: i64,
}

/// Every error answer: `{"error": {"code", "message"}}` plus field details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub error: ErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<crate::FieldError>,
}

impl ApiError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            error: ErrorBody {
                code: code.to_owned(),
                message: message.to_owned(),
                fields: Vec::new(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// upgrade-resilience-1 UR-4: readers tolerate fields a later version adds.
    #[test]
    fn ur4_bodies_ignore_new_fields() {
        let l: Lists = serde_json::from_str(
            r#"{"book_id": "b", "categories": ["Expenses:Office"], "a_later_field": {"x": 1}}"#,
        )
        .unwrap();
        assert_eq!(l.categories, ["Expenses:Office"]);
        let p: PairInfo =
            serde_json::from_str(r#"{"pin_required": true, "way": "wifi", "later": 2}"#).unwrap();
        assert!(p.pin_required);
        let h: Health = serde_json::from_str(r#"{"ok": true, "version": "0.1"}"#).unwrap();
        assert_eq!(h.api_level, 0, "a server from before api_level");
        assert_eq!(crate::phone_server_notice(&h), None);
        let h: Health =
            serde_json::from_str(r#"{"ok": true, "version": "9", "api_level": 7, "new": 1}"#)
                .unwrap();
        assert_eq!(h.api_level, 7);
    }

    #[test]
    fn flattened_fields_round_trip() {
        let e = NewExpense {
            client_id: "c".into(),
            fields: ExpenseFields {
                merchant: Some("Cafe".into()),
                amount: Some("4.50".into()),
                ..ExpenseFields::default()
            },
            receipt_ids: vec!["r1".into()],
        };
        let j = serde_json::to_string(&e).unwrap();
        assert!(j.contains("\"merchant\":\"Cafe\""), "{j}");
        let back: NewExpense = serde_json::from_str(&j).unwrap();
        assert_eq!(back, e);
        assert!(
            !format!(
                "{:?}",
                PairResponse {
                    device_id: "d".into(),
                    device_token: "SECRET".into()
                }
            )
            .contains("SECRET")
        );
    }
}
