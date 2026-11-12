//! Phone capture API (mobile-capture.md): the request and response types, the limits
//! and the field validation shared by the capture server (`moneykeep-server`), the
//! desktop's `capture.*` commands and the phone client (`moneykeep-capture`).
//!
//! This crate does no IO of any kind and depends only on `serde`, so it compiles for
//! `wasm32-unknown-unknown`. Callers pass in the current day and any random bytes.
//!
//! JSON follows RFC 8259; amounts travel as decimal strings ("45.20") and dates as
//! ISO 8601 calendar dates ("2026-10-23"), so no floating-point number is ever used.

mod dates;
mod mileage;
mod text;
mod types;
mod validate;

pub use dates::{civil_from_days, days_from_civil, format_date, parse_date, today_from_unix};
pub use mileage::*;
pub use text::clean_text;
pub use types::*;
pub use validate::{
    FieldError, ValidExpense, is_receipt_id, is_token_shape, is_uuid, parse_amount,
    validate_device_name, validate_expense, validate_lists,
};

/// This build's version: `<package version>+<commit time>.<commit>` (revision 4;
/// the package version alone outside a git checkout). The capture server reports
/// it in `/api/v1/health`; the desktop compares it with its own.
pub const BUILD_VERSION: &str = env!("MK_BUILD_VERSION");

/// The capture version (release-packaging.md): `<server package version>+<12 hex>`
/// over the sources the capture server and the phone app are built from, and
/// `Cargo.lock`. The server reports it in `/api/v1/health`; the desktop compares it
/// with its own, so a desktop-only change asks for no server update.
pub const CAPTURE_VERSION: &str = env!("MK_CAPTURE_VERSION");

/// The paths (relative to the workspace, `;`-separated) the capture version covers.
pub const CAPTURE_PATHS: &str = env!("MK_CAPTURE_PATHS");

/// The package version of a build or capture version (`0.9.3` of `0.9.3+…`).
pub fn package_version(v: &str) -> &str {
    v.split_once('+').map_or(v, |(p, _)| p)
}

/// Semantic Versioning 2.0.0 precedence of two package versions (no `+` part):
/// numbers first, then a pre-release (`0.1.0-alpha.1`) is older than its release,
/// and pre-release identifiers compare one by one (numbers numerically and below
/// words, words as text; more identifiers are newer). `None` when either is not a
/// version.
pub fn compare_package_versions(a: &str, b: &str) -> Option<core::cmp::Ordering> {
    use core::cmp::Ordering::{Equal, Greater, Less};
    type Parsed<'a> = (Vec<u64>, Option<Vec<&'a str>>);
    fn parse(v: &str) -> Option<Parsed<'_>> {
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c, Some(p.split('.').collect::<Vec<_>>())),
            None => (v, None),
        };
        let nums = core
            .split('.')
            .map(|n| n.parse::<u64>().ok())
            .collect::<Option<Vec<_>>>()?;
        if pre.as_ref().is_some_and(|p| p.iter().any(|x| x.is_empty())) {
            return None;
        }
        Some((nums, pre))
    }
    let (na, pa) = parse(a)?;
    let (nb, pb) = parse(b)?;
    match na.cmp(&nb) {
        Equal => {}
        o => return Some(o),
    }
    Some(match (pa, pb) {
        (None, None) => Equal,
        (None, Some(_)) => Greater,
        (Some(_), None) => Less,
        (Some(x), Some(y)) => {
            for (i, j) in x.iter().zip(y.iter()) {
                let o = match (i.parse::<u64>(), j.parse::<u64>()) {
                    (Ok(m), Ok(n)) => m.cmp(&n),
                    (Ok(_), Err(_)) => Less,
                    (Err(_), Ok(_)) => Greater,
                    (Err(_), Err(_)) => i.cmp(j),
                };
                if o != Equal {
                    return Some(o);
                }
            }
            x.len().cmp(&y.len())
        }
    })
}

/// Which of two build versions is newer: the package versions first (semver order,
/// so `0.1.0-alpha.2` is newer than `0.1.0-alpha.1`); then a version with build
/// information is newer than one without (servers built before revision 4 report
/// none, revision 4a); then the commit times. `None` when they cannot be ordered
/// (unknown format, or the same time with different commits).
pub fn compare_versions(a: &str, b: &str) -> Option<core::cmp::Ordering> {
    fn parts(v: &str) -> Option<(&str, Option<u64>, Option<&str>)> {
        let (pkg, meta) = match v.split_once('+') {
            Some((p, m)) => (p, Some(m)),
            None => (v, None),
        };
        let (time, commit) = match meta.and_then(|m| m.split_once('.')) {
            Some((t, c)) => (Some(t.parse::<u64>().ok()?), Some(c)),
            None => (None, None),
        };
        Some((pkg, time, commit))
    }
    let (pa, ta, ca) = parts(a)?;
    let (pb, tb, cb) = parts(b)?;
    match compare_package_versions(pa, pb)? {
        core::cmp::Ordering::Equal => {}
        o => return Some(o),
    }
    match (ta, tb) {
        (Some(_), None) => Some(core::cmp::Ordering::Greater),
        (None, Some(_)) => Some(core::cmp::Ordering::Less),
        (Some(x), Some(y)) if x != y => Some(x.cmp(&y)),
        _ if ca == cb => Some(core::cmp::Ordering::Equal),
        _ => None,
    }
}

/// Path prefix of every API route.
pub const API_PREFIX: &str = "/api/v1";
/// Path prefix of the desktop API.
pub const DESKTOP_PREFIX: &str = "/api/v1/desktop";

/// Largest receipt upload by default (megabytes); `max_receipt_mb` in `server.toml`.
pub const DEFAULT_MAX_RECEIPT_MB: u32 = 10;
/// Receipts one expense may carry by default; `max_receipts_per_expense`.
pub const DEFAULT_MAX_RECEIPTS_PER_EXPENSE: usize = 10;
/// Inbox size by default (megabytes); `inbox_limit_mb`.
pub const DEFAULT_INBOX_LIMIT_MB: u64 = 1000;
/// Default listen address of the capture server.
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8787";
/// Wi-Fi capture (mobile-capture.md revision 10): the HTTPS port phones use.
pub const DEFAULT_WIFI_HTTPS_PORT: u16 = 14443;
/// Wi-Fi capture: the plain-HTTP helper port (certificate page and CA certificate).
pub const DEFAULT_WIFI_HTTP_PORT: u16 = 14080;
/// The Capture PIN's length in digits.
pub const PIN_MIN: usize = 6;
pub const PIN_MAX: usize = 12;
/// The Windows Firewall rule Wi-Fi setup adds.
pub const FIREWALL_RULE: &str = "MoneyKeep capture (Wi-Fi)";
/// The certificate files on the helper port.
pub const CA_CRT_PATH: &str = "/ca.crt";
pub const CA_MOBILECONFIG_PATH: &str = "/moneykeep-ca.mobileconfig";
/// `way` / `via` values.
pub const VIA_WIFI: &str = "wifi";
pub const VIA_CLOUDFLARE: &str = "cloudflare";

/// Merchant (payee) text limit, as `expenses.md`.
pub const MAX_MERCHANT: usize = 100;
/// Purpose text limit, as `expenses.md`.
pub const MAX_PURPOSE: usize = 500;
/// Category and paid-with names sent by the phone.
pub const MAX_NAME: usize = 200;
/// A tag value (class, location, fund).
pub const MAX_TAG: usize = 100;
/// A phone's name.
pub const MAX_DEVICE_NAME: usize = 60;
/// Recent merchants in the lists document.
pub const MAX_RECENT_MERCHANTS: usize = 200;
/// Entries of any other list in the lists document.
pub const MAX_LIST_ITEMS: usize = 2000;
/// The lists document's largest size in bytes.
pub const MAX_LISTS_BYTES: usize = 1024 * 1024;
/// Largest JSON body of any request other than a receipt upload.
pub const MAX_JSON_BYTES: usize = 256 * 1024;
/// Dates are accepted within this many days of today, either side.
pub const DATE_WINDOW_DAYS: i64 = 400;
/// Integer digits an amount may have (`input-safety.md` R4).
pub const MAX_AMOUNT_DIGITS: usize = 15;
/// Decimal places an amount may have.
pub const MAX_AMOUNT_DECIMALS: usize = 2;

/// Pairing codes: length, alphabet (no I, O, 0 or 1), lifetime and wrong tries.
pub const PAIRING_CODE_LEN: usize = 8;
pub const PAIRING_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const PAIRING_TTL_SECS: i64 = 600;
pub const PAIRING_MAX_WRONG: u32 = 5;

/// Bearer tokens are this many random bytes, Base64URL without padding (43 chars).
pub const TOKEN_BYTES: usize = 32;
pub const TOKEN_CHARS: usize = 43;

/// Rate limits per token and for failed authentication per source address.
pub const REQUESTS_PER_MINUTE: u32 = 60;
pub const UPLOAD_BYTES_PER_DAY: u64 = 200 * 1024 * 1024;
pub const AUTH_FAILURES_PER_MINUTE: u32 = 10;
pub const AUTH_BACKOFF_SECS: i64 = 15 * 60;

/// JWT time checks allow this much clock difference.
pub const JWT_LEEWAY_SECS: i64 = 60;
/// Certificates are fetched again for an unknown key at most this often.
pub const CERTS_REFRESH_SECS: i64 = 60;

/// Photos are scaled so the longer side is at most this many pixels.
pub const MAX_PHOTO_SIDE: u32 = 2400;
/// Collected submissions listed for a phone.
pub const COLLECTED_SHOWN: usize = 50;
/// Bytes of acknowledged submissions are kept this long (a failed desktop commit can
/// collect again).
pub const ACK_RETENTION_SECS: i64 = 24 * 3600;
/// The health route (mobile-capture.md revision 2, section C): `{"ok", "version"}`
/// only, no names or counts.
pub const HEALTH_PATH: &str = "/api/v1/health";
/// upgrade-resilience-1 U1.4: the level of this program's API, reported by
/// `/api/v1/health` as `api_level` (servers from before report none: level 0). It
/// goes up when the server adds something the desktop or the phone relies on.
pub const API_LEVEL: u32 = 1;
/// The server's API level this desktop needs; a lower one shows "Update the capture
/// server" (level 1: `wifi_error` and the start-up refusals are reported).
pub const DESKTOP_NEEDS_API_LEVEL: u32 = 1;
/// The server's API level this phone app needs; a lower one shows
/// [`msg::SERVER_NEEDS_UPDATE`].
pub const PHONE_NEEDS_API_LEVEL: u32 = 0;

/// The phone's notice for `health` (U1.4): `Some` when the computer's server is
/// older than this phone app needs.
pub fn phone_server_notice(health: &Health) -> Option<&'static str> {
    phone_server_notice_at(health, PHONE_NEEDS_API_LEVEL)
}

/// [`phone_server_notice`] for a phone app that needs API level `needs`.
pub fn phone_server_notice_at(health: &Health, needs: u32) -> Option<&'static str> {
    (health.api_level.cmp(&needs) == core::cmp::Ordering::Less).then_some(msg::SERVER_NEEDS_UPDATE)
}

/// The desktop's status route (waiting count and last phone activity).
pub const DESKTOP_STATUS_PATH: &str = "/api/v1/desktop/status";
/// The fragment that carries a pairing code in the QR code's address
/// (`https://<address>/#pair=<code>`); browsers never send fragments to the server.
pub const PAIR_FRAGMENT: &str = "pair=";

/// The security headers the capture server sends with every response
/// (mobile-capture.md "WASM client", revision 2 section A; revision 3 adds
/// `worker-src` and `manifest-src` for the service worker and the manifest), in
/// this order.
pub const SECURITY_HEADERS: [(&str, &str); 4] = [
    (
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; img-src 'self' blob: data:; connect-src 'self'; style-src 'self' 'unsafe-inline'; worker-src 'self'; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
    ),
    ("X-Content-Type-Options", "nosniff"),
    ("Referrer-Policy", "no-referrer"),
    ("Permissions-Policy", "camera=(self), geolocation=()"),
];

/// Messages shown to people (shared so every front end words them the same way).
pub mod msg {
    pub const NOT_SIGNED_IN: &str = "Not signed in";
    /// A 401 `access_refused` or `access_missing` (revision 8): the server's
    /// settings, not the phone's sign-in, need fixing.
    pub const ACCESS_REFUSED: &str = "The capture server did not accept this Cloudflare sign-in. On the computer, open phone capture setup and press Check.";
    pub const STORAGE_FULL: &str = "Storage full — open MoneyKeep on your computer to collect";
    pub const PICTURE: &str = "This picture can't be read.";
    pub const PHOTO_TYPE: &str =
        "This photo type isn't supported — set your camera to save JPEG (Most Compatible)";
    pub const DISCONNECTED: &str = "This phone was disconnected — pair again to send";
    pub const TOO_MANY_REQUESTS: &str = "Too many requests — try again in a minute";
    pub const PAIR_FAILED: &str =
        "That code didn't work. Check it, or create a new one in MoneyKeep";
    pub const ENTER_SOMETHING: &str = "Enter an expense or add a photo.";
    // Revision 3: installable app and lasting pairing.
    pub const SIGN_IN_AGAIN: &str = "Sign in to Cloudflare again";
    pub const PAIRING_NOT_SAVED: &str = "This browser didn't save the pairing (private browsing?)";
    pub const WAS_DISCONNECTED: &str = "This phone was disconnected from your computer";
    pub const STORAGE_CLEARED: &str = "This browser cleared the app's storage";
    pub const IOS_INSTALL_FIRST: &str = "Install the app first (Share, then Add to Home Screen), then pair inside it — the installed app keeps its own pairing.";
    pub const IOS_INSTALL: &str = "To install: tap Share, then Add to Home Screen";
    pub const ALREADY_PAIRED: &str =
        "This phone is already paired. To pair it again, choose Sign out first.";
    pub const PIN_WRONG: &str = "That PIN is not right. Check it in MoneyKeep on your computer.";
    pub const PIN_NOT_SET: &str =
        "Wi-Fi capture has no Capture PIN yet. Finish Wi-Fi setup in MoneyKeep on your computer.";
    pub const PIN_RULE: &str = "The Capture PIN is 6 to 12 digits.";
    // Revision 15: say what the PIN is.
    /// The phone's PIN field label.
    pub const PIN_LABEL: &str = "Capture PIN";
    /// The help line under [`PIN_LABEL`].
    pub const PIN_HELP: &str = "The 6 to 12 digits you chose when you set up phone capture in MoneyKeep. Forgot it? On the computer: Settings → Phone capture → Change PIN…";
    /// The line under the code in the desktop's pairing windows.
    pub const PHONE_ASKS_PIN: &str = "The phone will also ask for your Capture PIN.";
    /// The status text while the computer can't be reached (mobile-capture.md
    /// revision 14: no answer, a timeout, or a 5xx such as Cloudflare's error page).
    pub const NOT_REACHABLE: &str = "Your computer isn't reachable";
    /// The line under [`NOT_REACHABLE`].
    pub const NOT_REACHABLE_DETAIL: &str = "It may be off, asleep or offline. You can keep adding expenses and trips — they wait in the Outbox and send by themselves when it's back.";
    /// Revision 18: a Wi-Fi phone whose computer doesn't answer at all (the loading
    /// screen, and the offline banner when the app opened from its cache).
    pub const WIFI_UNREACHABLE: &str = "Can't reach your computer on this Wi-Fi";
    /// The line under [`WIFI_UNREACHABLE`].
    pub const WIFI_UNREACHABLE_DETAIL: &str = "Make sure this phone is on the same Wi-Fi as your computer, and that MoneyKeep is running there.";
    /// Revision 18: the Capture PIN field's placeholder.
    pub const PIN_PLACEHOLDER: &str = "6–12 digits";
    /// The Outbox note for a Cloudflare phone while the computer can't be reached.
    pub const SEND_WHEN_ON: &str = "Will send when your computer is on and connected";
    /// The note on the forms while the lists are the ones kept from the last load:
    /// "Lists from Nov 8, 1:05 PM".
    pub const LISTS_FROM: &str = "Lists from";
    /// The Outbox note for a phone that uses the Wi-Fi address.
    pub const SEND_ON_WIFI: &str = "Will send when you are on the same Wi-Fi as your computer";
    pub const STORAGE_KEPT: &str = "Storage: kept";
    pub const STORAGE_MAY_CLEAR: &str = "Storage: may be cleared by the browser";
    /// A capture server from before mileage (mobile-capture.md revision 11): the
    /// trip stays in the Outbox.
    pub const NO_MILEAGE: &str = "This server does not accept mileage yet; update the server";
    /// The one-time tip above the tab bar (revision 11).
    pub const MILEAGE_TIP: &str = "Tap Expense again to log mileage";
    /// upgrade-resilience-1 U1.4: the computer's capture server is older than this
    /// phone app needs.
    pub const SERVER_NEEDS_UPDATE: &str = "Your computer's MoneyKeep needs updating";
    /// … and the line under it.
    pub const SERVER_NEEDS_UPDATE_DETAIL: &str =
        "Update MoneyKeep on your computer. Until then, some things on this phone may not work.";
    /// upgrade-resilience-1 M9: a photo the server no longer has, and this phone
    /// doesn't have either (an edit of a sent expense): the item stays in the Outbox.
    pub const ADD_PHOTO_AGAIN: &str =
        "Add the photo again: it isn't on your computer or this phone any more.";
}

/// A pairing code from eight random bytes (each byte picks one of 32 characters, so
/// every character is equally likely).
pub fn pairing_code(random: &[u8; PAIRING_CODE_LEN]) -> String {
    random
        .iter()
        .map(|b| {
            PAIRING_ALPHABET
                .get(usize::from(b & 31))
                .copied()
                .map_or('A', char::from)
        })
        .collect()
}

/// A typed pairing code made comparable: upper case, without spaces or dashes.
pub fn normalize_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .flat_map(char::to_uppercase)
        .collect()
}

/// The phone page's address with a pairing code in its fragment, for the QR code:
/// `https://<host>/#pair=<code>`.
pub fn pair_url(host: &str, code: &str) -> String {
    let h = host.trim().trim_end_matches('/');
    let h = h.strip_prefix("https://").unwrap_or(h);
    format!("https://{h}/#{PAIR_FRAGMENT}{}", normalize_code(code))
}

/// A Capture PIN as typed: `Ok` with its digits when it is 6–12 digits.
pub fn validate_pin(pin: &str) -> Result<String, String> {
    let p = pin.trim();
    if (PIN_MIN..=PIN_MAX).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()) {
        Ok(p.to_owned())
    } else {
        Err(msg::PIN_RULE.to_owned())
    }
}

/// The certificate page a phone opens first (QR code 1): `http://<host>:<port>/`.
pub fn cert_page_url(host: &str, http_port: u16) -> String {
    format!("http://{}:{http_port}/", host.trim())
}

/// The Wi-Fi pairing address (QR code 2): `https://<host>:<port>/#pair=<code>`.
pub fn wifi_pair_url(host: &str, https_port: u16, code: &str) -> String {
    pair_url(&format!("{}:{https_port}", host.trim()), code)
}

/// The pairing code in a page fragment (`#pair=ABCD2345`, with or without the `#`),
/// when it has the right length and alphabet.
pub fn code_from_fragment(fragment: &str) -> Option<String> {
    let f = fragment.trim().trim_start_matches('#');
    let code = f
        .split('&')
        .find_map(|part| part.strip_prefix(PAIR_FRAGMENT))
        .map(normalize_code)?;
    (code.len() == PAIRING_CODE_LEN && code.bytes().all(|b| PAIRING_ALPHABET.contains(&b)))
        .then_some(code)
}

/// The size that fits `w` × `h` within `side` pixels on the longer side, keeping the
/// aspect ratio (rounded to the nearest pixel, at least 1) and never enlarging.
pub fn fit_within(w: u32, h: u32, side: u32) -> (u32, u32) {
    if w == 0 || h == 0 || side == 0 {
        return (w.min(side), h.min(side));
    }
    let long = w.max(h);
    if long <= side {
        return (w, h);
    }
    let scale = |x: u32| -> u32 {
        let v = (u64::from(x) * u64::from(side) + u64::from(long) / 2) / u64::from(long);
        u32::try_from(v.max(1)).unwrap_or(side)
    };
    if w >= h {
        (side, scale(h))
    } else {
        (scale(w), side)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mobile-capture.md revision 10: the PIN rule and the two QR addresses.
    #[test]
    fn wifi_pin_and_addresses() {
        assert_eq!(validate_pin(" 246813 ").as_deref(), Ok("246813"));
        assert!(validate_pin("12345").is_err());
        assert!(validate_pin("1234567890123").is_err());
        assert!(validate_pin("12 456").is_err());
        assert_eq!(
            cert_page_url("192.168.1.20", DEFAULT_WIFI_HTTP_PORT),
            "http://192.168.1.20:14080/"
        );
        assert_eq!(
            wifi_pair_url("192.168.1.20", DEFAULT_WIFI_HTTPS_PORT, "abcd2345"),
            "https://192.168.1.20:14443/#pair=ABCD2345"
        );
        assert_eq!(
            code_from_fragment("#pair=ABCD2345").as_deref(),
            Some("ABCD2345")
        );
    }

    /// mobile-capture.md AT16: photo scaling keeps ≤ 2,400 px and the aspect ratio.
    #[test]
    fn photo_scaling_fits_and_keeps_aspect() {
        assert_eq!(fit_within(4000, 3000, MAX_PHOTO_SIDE), (2400, 1800));
        assert_eq!(fit_within(3000, 4000, MAX_PHOTO_SIDE), (1800, 2400));
        assert_eq!(fit_within(1200, 900, MAX_PHOTO_SIDE), (1200, 900));
        assert_eq!(fit_within(4032, 3024, MAX_PHOTO_SIDE), (2400, 1800));
        assert_eq!(fit_within(10000, 10, MAX_PHOTO_SIDE), (2400, 2));
        assert_eq!(fit_within(100_000, 1, MAX_PHOTO_SIDE), (2400, 1));
        for (w, h) in [(4001, 2999), (2401, 2401), (5000, 3333), (999, 7000)] {
            let (a, b) = fit_within(w, h, MAX_PHOTO_SIDE);
            assert!(a.max(b) <= MAX_PHOTO_SIDE);
            // Aspect kept within one pixel of rounding.
            let lhs = u64::from(a) * u64::from(h);
            let rhs = u64::from(b) * u64::from(w);
            assert!(
                lhs.abs_diff(rhs) <= u64::from(w.max(h)),
                "{w}x{h} -> {a}x{b}"
            );
        }
    }

    /// Revision 4: build versions order by package version, then commit time.
    #[test]
    fn versions_compare() {
        use core::cmp::Ordering::*;
        assert_eq!(
            compare_versions("0.0.1+100.aaa", "0.0.1+200.bbb"),
            Some(Less)
        );
        assert_eq!(
            compare_versions("0.0.2+100.aaa", "0.0.1+200.bbb"),
            Some(Greater)
        );
        assert_eq!(
            compare_versions("0.0.1+100.aaa", "0.0.1+100.aaa"),
            Some(Equal)
        );
        assert_eq!(compare_versions("0.0.1+100.aaa", "0.0.1+100.bbb"), None);
        assert_eq!(compare_versions("0.0.1", "0.0.1"), Some(Equal));
        // Revision 4a: build information beats none.
        assert_eq!(compare_versions("0.0.1", "0.0.1+100.aaa"), Some(Less));
        assert_eq!(
            compare_versions("0.0.1+1793216209.59ebe798226f", "0.0.1"),
            Some(Greater)
        );
        assert_eq!(compare_versions("0.0.2", "0.0.1+100.aaa"), Some(Greater));
        assert_eq!(compare_versions("x", "0.0.1"), None);
        // release-prep.md §3: pre-release versions in semver order.
        assert_eq!(
            compare_versions("0.1.0-alpha.1+100.aaa", "0.0.1+200.bbb"),
            Some(Greater)
        );
        assert_eq!(
            compare_versions("0.1.0-alpha.2+100.aaa", "0.1.0-alpha.1+200.bbb"),
            Some(Greater)
        );
        assert_eq!(
            compare_versions("0.1.0+100.aaa", "0.1.0-alpha.2+200.bbb"),
            Some(Greater)
        );
        assert_eq!(
            compare_versions("0.1.0-alpha.1+100.aaa", "0.1.0-alpha.1+200.bbb"),
            Some(Less)
        );
        assert_eq!(
            compare_versions("0.1.0-alpha.1", "0.1.0-alpha.1"),
            Some(Equal)
        );
        assert_eq!(compare_versions("0.1.0-alpha", "0.1.0-alpha.1"), Some(Less));
        assert_eq!(
            compare_versions("0.1.0-alpha.10", "0.1.0-alpha.9"),
            Some(Greater)
        );
        assert_eq!(
            compare_versions("0.1.0-beta", "0.1.0-alpha.9"),
            Some(Greater)
        );
        assert_eq!(compare_versions("0.1.0-", "0.1.0"), None);
        assert!(BUILD_VERSION.starts_with(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn pairing_codes_use_the_alphabet() {
        let c = pairing_code(&[0, 1, 31, 32, 255, 8, 9, 200]);
        assert_eq!(c.len(), PAIRING_CODE_LEN);
        assert!(c.bytes().all(|b| PAIRING_ALPHABET.contains(&b)));
        assert!(!c.contains(['I', 'O', '0', '1']));
        assert_eq!(normalize_code(" abcd-efgh "), "ABCDEFGH");
    }

    /// mobile-capture.md AT26 (address part): the QR address carries the code in its
    /// fragment, and the phone reads it back.
    #[test]
    fn pairing_fragment_round_trip() {
        let url = pair_url("capture.example.com", "abcd2345");
        assert_eq!(url, "https://capture.example.com/#pair=ABCD2345");
        let frag = url.split_once('#').map(|(_, f)| f).unwrap_or("");
        assert_eq!(code_from_fragment(frag).as_deref(), Some("ABCD2345"));
        assert_eq!(
            code_from_fragment("#pair=ABCD2345").as_deref(),
            Some("ABCD2345")
        );
        assert_eq!(code_from_fragment("#pair=ABC"), None);
        assert_eq!(code_from_fragment("#pair=ABCD0O1I"), None);
        assert_eq!(code_from_fragment("#other=1"), None);
        assert_eq!(
            pair_url("https://capture.example.com/", "X"),
            "https://capture.example.com/#pair=X"
        );
    }
}
