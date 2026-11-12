//! Renders the phone app's Expense, Outbox and Sent tabs, the amount keypad, the
//! calendar and the Pair screen in light and dark into PNG files for review
//! (ui-slint.md revision 2, Tests; mobile-capture.md revisions 7–9, 10, 14, 15 and 18). Run under a display (or Xvfb)
//! with `SLINT_BACKEND=winit-software cargo run -p moneykeep-capture --example
//! phone_screens -- <dir>`. A paired phone with a few published lists; no network.
#![allow(clippy::unwrap_used, clippy::print_stdout, clippy::indexing_slicing)]

use std::collections::BTreeMap;

use mk_views::phone::PhoneView;
use mk_views::slint;
use moneykeep_capture::outbox::Storage;
use moneykeep_capture::ui::{CaptureApp, Install, PickedPhoto, Platform, Request, Response};
use slint::ComponentHandle;

#[derive(Default)]
struct Mem(BTreeMap<String, String>);

impl Storage for Mem {
    fn put(&mut self, key: &str, value: &str) {
        self.0.insert(key.into(), value.into());
    }
    fn delete(&mut self, key: &str) {
        self.0.remove(key);
    }
}

/// A browser stand-in: paired, offline, nothing to send.
struct Review {
    store: Mem,
    loaded: Option<Vec<(String, String)>>,
    /// Revision 10: the Wi-Fi address answers `pair-info` with "PIN required".
    wifi: bool,
    next: u64,
    answers: Vec<(u64, Response)>,
    /// Revision 14: online, but every API call gets Cloudflare's 530 error page
    /// (the computer is off); the clock is then Nov 8, 2026, 1:40 PM.
    unreachable: bool,
}

/// Nov 8, 2026, 1:05 PM (UTC, the review's time zone): the lists' last load.
const LISTS_AT: i64 = 1_794_143_100_000;

impl Platform for Review {
    fn now_ms(&self) -> i64 {
        if self.unreachable {
            LISTS_AT + 35 * 60_000
        } else {
            1_793_000_000_000
        }
    }
    fn today(&self) -> String {
        if self.unreachable {
            "2026-11-08"
        } else {
            "2026-11-02"
        }
        .into()
    }
    fn online(&self) -> bool {
        self.unreachable
    }
    fn default_device_name(&self) -> String {
        "Review phone".into()
    }
    fn random16(&self) -> [u8; 16] {
        [7; 16]
    }
    fn send(&mut self, req: Request) -> u64 {
        self.next += 1;
        if self.unreachable {
            self.answers.push((
                self.next,
                Response {
                    status: 530,
                    body: b"<!DOCTYPE html><title>Cloudflare Tunnel error</title>Error 1033"
                        .to_vec(),
                },
            ));
            return self.next;
        }
        // upgrade-resilience-1 U1.4: a server at API level 1.
        if req.path == "/api/v1/health" {
            self.answers.push((
                self.next,
                Response {
                    status: 200,
                    body: br#"{"ok": true, "version": "0.9.0", "api_level": 1}"#.to_vec(),
                },
            ));
            return self.next;
        }
        if self.wifi && req.path == "/api/v1/pair-info" {
            self.answers.push((
                self.next,
                Response {
                    status: 200,
                    body: br#"{"pin_required": true, "way": "wifi"}"#.to_vec(),
                },
            ));
        }
        self.next
    }
    fn poll(&mut self) -> Vec<(u64, Response)> {
        std::mem::take(&mut self.answers)
    }
    fn pick_photos(&mut self, _camera: bool) {}
    fn poll_photos(&mut self) -> Vec<Result<PickedPhoto, String>> {
        Vec::new()
    }
    fn storage(&mut self) -> &mut dyn Storage {
        &mut self.store
    }
    fn loaded(&mut self) -> Option<Vec<(String, String)>> {
        self.loaded.take()
    }
    fn put_confirmed(&mut self, key: &str, value: &str) -> u64 {
        self.store.put(key, value);
        1
    }
    fn poll_saved(&mut self) -> Vec<(u64, Result<(), String>)> {
        Vec::new()
    }
    fn install(&self) -> Install {
        Install {
            installed: true,
            can_prompt: false,
            ios: false,
        }
    }
}

fn app() -> CaptureApp<Review> {
    let lists = r#"{"book_name": "Household", "base_currency": "USD",
        "categories": ["Expenses:Groceries", "Expenses:Office"],
        "paid_with": ["Liabilities:Visa", "Personal funds (reimbursable)"],
        "merchants": ["Corner Grocery", "Office Depot"]}"#;
    let mut a = CaptureApp::new(Review {
        store: Mem::default(),
        loaded: Some(vec![
            ("token".into(), "review-token".into()),
            ("lists".into(), lists.into()),
        ]),
        wifi: false,
        next: 0,
        answers: Vec::new(),
        unreachable: false,
    });
    a.tick();
    a
}

/// Revision 11: a paired phone whose book has mileage (two vehicles, a saved trip,
/// the 2026 rates), on the Mileage page; with `tags`, classes and funds too (mileage.md
/// revision 2).
fn mileage_app(tags: bool) -> CaptureApp<Review> {
    let lists = r#"{"book_name": "Household", "base_currency": "USD",
        "categories": ["Expenses:Groceries"], "mileage_enabled": true,
        "reimbursable_enabled": true, "default_purpose": "business",
        "purpose_kinds": [{"kind": "business", "label": "Business"},
            {"kind": "medical", "label": "Medical"}, {"kind": "charity", "label": "Charity"},
            {"kind": "personal", "label": "Personal"}],
        "vehicles": [{"id": "v1", "name": "2019 Honda CR-V"}, {"id": "v2", "name": "Church van"}],
        "saved_trips": [{"id": "s1", "name": "Office to client", "from_place": "Office",
            "to_place": "Client", "miles": "12.4", "round_trip": true,
            "purpose_kind": "business", "purpose": "Client visit", "vehicle": "v1"}],
        "mileage_rates": [{"kind": "business", "from": "2026-01-01", "cents": "72.5"},
            {"kind": "business", "from": "2026-07-01", "cents": "76"},
            {"kind": "charity", "from": "2026-01-01", "cents": "14"}]}"#;
    let lists = if tags {
        lists.replacen(
            r#""base_currency": "USD","#,
            r#""base_currency": "USD", "tags": {"class": ["Roof repair", "Youth"],
                "fund": ["General", "Building"]},"#,
            1,
        )
    } else {
        lists.to_owned()
    };
    let mut a = CaptureApp::new(Review {
        store: Mem::default(),
        loaded: Some(vec![
            ("token".into(), "review-token".into()),
            ("lists".into(), lists),
            ("page".into(), "mileage".into()),
            ("mileage_tip_seen".into(), "1".into()),
            ("places".into(), r#"["Client", "Office"]"#.into()),
        ]),
        wifi: false,
        next: 0,
        answers: Vec::new(),
        unreachable: false,
    });
    a.tick();
    a
}

/// A phone that has not been paired yet (the Pair screen).
fn unpaired() -> CaptureApp<Review> {
    let mut a = CaptureApp::new(Review {
        store: Mem::default(),
        loaded: Some(Vec::new()),
        wifi: false,
        next: 0,
        answers: Vec::new(),
        unreachable: false,
    });
    a.tick();
    a
}

/// A phone opening QR code 2 on the Wi-Fi address: the PIN is asked.
fn wifi_pair() -> CaptureApp<Review> {
    let mut a = CaptureApp::new(Review {
        store: Mem::default(),
        loaded: Some(Vec::new()),
        wifi: true,
        next: 0,
        answers: Vec::new(),
        unreachable: false,
    });
    a.tick();
    a.tick();
    a
}

/// Revision 14: a paired Cloudflare phone whose computer is off — the lists kept
/// from the last load and one expense waiting in the Outbox.
fn unreachable_app() -> CaptureApp<Review> {
    let lists = r#"{"book_name": "Household", "base_currency": "USD",
        "categories": ["Expenses:Groceries", "Expenses:Office"],
        "paid_with": ["Liabilities:Visa", "Personal funds (reimbursable)"],
        "merchants": ["Corner Grocery", "Office Depot"]}"#;
    let mut a = CaptureApp::new(Review {
        store: Mem::default(),
        loaded: Some(vec![
            ("token".into(), "review-token".into()),
            ("way".into(), "cloudflare".into()),
            ("lists".into(), lists.into()),
            ("lists_at".into(), LISTS_AT.to_string()),
        ]),
        wifi: false,
        next: 0,
        answers: Vec::new(),
        unreachable: true,
    });
    a.tick();
    a.tick();
    a
}

/// upgrade-resilience-1 Batch U2: an Outbox holding an item parked with "Add the
/// photo again" (M9) and two items a later version saved (M10), on a phone app that
/// needs a newer server than the computer's (U1.4).
fn u2_app() -> CaptureApp<Review> {
    let lists = r#"{"book_name": "Household", "base_currency": "USD",
        "categories": ["Expenses:Groceries", "Expenses:Office"]}"#;
    let parked = serde_json::json!({
        "v": 1,
        "client_id": "0f0f0f0f-0f0f-4f0f-8f0f-0f0f0f0f0f0f",
        "fields": {"date": "2026-11-01", "merchant": "Corner Grocery", "amount": "18.40"},
        "photos": [{"jpeg": "", "width": 0, "height": 0, "receipt_id": "r1"}],
        "state": "needs_attention",
        "server_id": "0123456789abcdef0123456789abcdef",
        "last_error": mk_capture_api::msg::ADD_PHOTO_AGAIN,
        "created_ms": 1
    });
    let mut a = CaptureApp::new(Review {
        store: Mem::default(),
        loaded: Some(vec![
            ("token".into(), "review-token".into()),
            ("lists".into(), lists.into()),
            (
                "outbox:0f0f0f0f-0f0f-4f0f-8f0f-0f0f0f0f0f0f".into(),
                parked.to_string(),
            ),
            (
                "outbox:later-1".into(),
                r#"{"v": 2, "client_id": "later-1"}"#.into(),
            ),
            (
                "outbox:later-2".into(),
                r#"{"v": 2, "client_id": "later-2"}"#.into(),
            ),
        ]),
        wifi: false,
        next: 0,
        answers: Vec::new(),
        unreachable: false,
    });
    a.set_needs_api_level(2);
    a.tick();
    a.tick();
    a
}

fn save_png(path: &std::path::Path, buf: &slint::SharedPixelBuffer<slint::Rgba8Pixel>) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), buf.width(), buf.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap();
    w.write_image_data(buf.as_bytes()).unwrap();
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "screens".into()));
    std::fs::create_dir_all(&out).unwrap();
    let mut views = Vec::new();
    // mobile-capture.md revision 9, test 65: the Expense, Outbox and Sent tabs and
    // the keypad open, at 390 × 844, light and dark (revision 7's form and
    // calendar shots are taken on the Expense tab too).
    for (theme, word) in [(1, "light"), (2, "dark")] {
        for shot in ["expense", "outbox", "sent", "keypad", "calendar"] {
            let v = PhoneView::new(app()).unwrap();
            v.window().set_theme_choice(theme);
            v.window()
                .window()
                .set_size(slint::LogicalSize::new(390.0, 844.0));
            v.act("Merchant", 1, "Corner Grocery");
            match shot {
                "outbox" => {
                    v.act("Save for later", 0, "");
                    v.act("tab:Outbox", 0, "");
                }
                "sent" => v.act("tab:Sent", 0, ""),
                "keypad" => {
                    v.act("amount", 0, "");
                    for k in [
                        "1",
                        "2",
                        "Decimal point",
                        "5",
                        "0",
                        "Plus",
                        "4",
                        "Decimal point",
                        "2",
                        "5",
                    ] {
                        v.act(&format!("dialog:keypad/{k}"), 0, "");
                    }
                }
                "calendar" => v.act("date", 0, ""),
                _ => {}
            }
            v.show().unwrap();
            let prefix = if shot == "calendar" {
                "phone7"
            } else {
                "phone9"
            };
            views.push((v, out.join(format!("{prefix}-{shot}-{word}.png"))));
        }
    }
    // Revision 8, test 59: the Pair screen with both fields filled and no field
    // focused, light and dark.
    for (theme, word) in [(1, "light"), (2, "dark")] {
        let v = PhoneView::new(unpaired()).unwrap();
        v.window().set_theme_choice(theme);
        v.window()
            .window()
            .set_size(slint::LogicalSize::new(390.0, 844.0));
        v.act("Pairing code", 1, "ABCD2345");
        v.act("Name for this phone", 1, "Owner's phone");
        v.show().unwrap();
        views.push((v, out.join(format!("phone8-pair-{word}.png"))));
        // The form: Merchant and Purpose filled, nothing focused, Save for later
        // in the bottom bar.
        let v = PhoneView::new(app()).unwrap();
        v.window().set_theme_choice(theme);
        v.window()
            .window()
            .set_size(slint::LogicalSize::new(390.0, 844.0));
        v.act("Merchant", 1, "Corner Hardware");
        v.act("Purpose (optional)", 1, "Shelf brackets");
        v.show().unwrap();
        views.push((v, out.join(format!("phone8-form-{word}.png"))));
    }
    // Revision 10, test 76: the Wi-Fi Pair screen with the Capture PIN, part typed
    // (masked), light and dark.
    for (theme, word) in [(1, "light"), (2, "dark")] {
        let v = PhoneView::new(wifi_pair()).unwrap();
        v.window().set_theme_choice(theme);
        v.window()
            .window()
            .set_size(slint::LogicalSize::new(390.0, 844.0));
        v.act("Pairing code", 1, "ABCD2345");
        v.act("Name for this phone", 1, "Anna's Pixel");
        for d in ["2", "4", "6", "8"] {
            v.act(d, 0, "");
        }
        v.show().unwrap();
        views.push((v, out.join(format!("phone10-pin-{word}.png"))));
    }
    // Revision 15, test 87: the Pair screen's "Capture PIN" label and the help line
    // saying which PIN it is (nothing typed yet), light and dark.
    for (theme, word) in [(1, "light"), (2, "dark")] {
        let v = PhoneView::new(wifi_pair()).unwrap();
        v.window().set_theme_choice(theme);
        v.window()
            .window()
            .set_size(slint::LogicalSize::new(390.0, 844.0));
        v.act("Pairing code", 1, "ABCD2345");
        v.act("Name for this phone", 1, "Anna's Pixel");
        v.show().unwrap();
        views.push((v, out.join(format!("phone15-pin-{word}.png"))));
    }
    // Revision 11 (mileage.md M14): the Mileage page with miles and a round trip,
    // and with odometer readings, light and dark.
    for (theme, word) in [(1, "light"), (2, "dark")] {
        for shot in ["mileage", "odometer"] {
            let v = PhoneView::new(mileage_app(false)).unwrap();
            v.window().set_theme_choice(theme);
            v.window()
                .window()
                .set_size(slint::LogicalSize::new(390.0, 844.0));
            v.act("Vehicle", 5, "1");
            if shot == "mileage" {
                v.act("miles", 0, "");
                for k in ["1", "2", "Decimal point", "4", "Done"] {
                    v.act(&format!("dialog:keypad/{k}"), 0, "");
                }
                v.act("Round trip", 0, "");
            } else {
                v.act("Enter odometer readings", 0, "");
                v.act("odo-start", 0, "");
                for k in ["4", "5", "2", "1", "0", "Decimal point", "3", "Done"] {
                    v.act(&format!("dialog:keypad/{k}"), 0, "");
                }
                v.act("odo-end", 0, "");
                for k in ["4", "5", "2", "3", "6", "Done"] {
                    v.act(&format!("dialog:keypad/{k}"), 0, "");
                }
            }
            v.act("From", 1, "Office");
            v.act("To", 1, "Client");
            v.act("Purpose", 1, "Quarterly review with the client");
            v.show().unwrap();
            views.push((v, out.join(format!("phone11-{shot}-{word}.png"))));
        }
    }
    // mileage.md revision 2: the tags below Purpose (a tall window shows the whole
    // form), light and dark.
    for (theme, word) in [(1, "light"), (2, "dark")] {
        let v = PhoneView::new(mileage_app(true)).unwrap();
        v.window().set_theme_choice(theme);
        v.window()
            .window()
            .set_size(slint::LogicalSize::new(390.0, 1400.0));
        v.act("Vehicle", 5, "1");
        v.act("To", 1, "Church");
        v.act("Purpose", 1, "Roof inspection");
        v.act("Class (optional)", 5, "1");
        v.act("Fund (optional)", 5, "2");
        v.show().unwrap();
        views.push((v, out.join(format!("phone-mileage-r2-tags-{word}.png"))));
    }
    // mobile-capture.md revision 14: the home screen (the Expense tab) and the
    // Outbox while the computer can't be reached, light and dark.
    for (theme, word) in [(1, "light"), (2, "dark")] {
        for shot in ["home", "outbox"] {
            let v = PhoneView::new(unreachable_app()).unwrap();
            v.window().set_theme_choice(theme);
            v.window()
                .window()
                .set_size(slint::LogicalSize::new(390.0, 844.0));
            if shot == "outbox" {
                v.act("Merchant", 1, "Corner Grocery");
                v.act("Send", 0, "");
                v.act("tab:Outbox", 0, "");
                // The 530 answer to the send arrives.
                v.with_app(|a| {
                    a.tick();
                    a.tick();
                });
            }
            v.show().unwrap();
            views.push((
                v,
                out.join(format!("phone14-unreachable-{shot}-{word}.png")),
            ));
        }
    }
    // upgrade-resilience-1 Batch U2: the Expense tab with "Your computer's
    // MoneyKeep needs updating", and the Outbox with a parked item and the count of
    // items this version couldn't read, light and dark.
    for (theme, word) in [(1, "light"), (2, "dark")] {
        for shot in ["expense", "outbox"] {
            let v = PhoneView::new(u2_app()).unwrap();
            v.window().set_theme_choice(theme);
            v.window()
                .window()
                .set_size(slint::LogicalSize::new(390.0, 844.0));
            if shot == "outbox" {
                v.act("tab:Outbox", 0, "");
            }
            v.show().unwrap();
            views.push((v, out.join(format!("u2-{shot}-{word}.png"))));
        }
    }
    // Revision 18, test 108: the Capture PIN field with its outline, empty and
    // focused (a tap on it), light and dark. MK_PIN_TAP="x,y" moves the tap.
    let tap = std::env::var("MK_PIN_TAP")
        .ok()
        .and_then(|t| {
            let (x, y) = t.split_once(',')?;
            Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))
        })
        .unwrap_or((150.0, 412.0));
    let mut taps = Vec::new();
    for (theme, word) in [(1, "light"), (2, "dark")] {
        for shot in ["empty", "focused"] {
            let v = PhoneView::new(wifi_pair()).unwrap();
            v.window().set_theme_choice(theme);
            v.window()
                .window()
                .set_size(slint::LogicalSize::new(390.0, 844.0));
            v.act("Pairing code", 1, "ABCD2345");
            v.act("Name for this phone", 1, "Anna's Pixel");
            v.show().unwrap();
            if shot == "focused" {
                taps.push(v.clone());
            }
            views.push((v, out.join(format!("phone18-pair-field-{shot}-{word}.png"))));
        }
    }
    slint::Timer::single_shot(std::time::Duration::from_millis(400), move || {
        use slint::platform::{PointerEventButton, WindowEvent};
        let position = slint::LogicalPosition::new(tap.0, tap.1);
        for v in &taps {
            let w = v.window().window();
            w.dispatch_event(WindowEvent::PointerMoved { position });
            w.dispatch_event(WindowEvent::PointerPressed {
                position,
                button: PointerEventButton::Left,
            });
            w.dispatch_event(WindowEvent::PointerReleased {
                position,
                button: PointerEventButton::Left,
            });
        }
    });
    // An optional second argument keeps only the shots whose file name starts
    // with it.
    if let Some(only) = std::env::args().nth(2) {
        views.retain(|(_, p)| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&only))
        });
    }
    slint::Timer::single_shot(std::time::Duration::from_millis(800), move || {
        for (v, path) in &views {
            match v.window().window().take_snapshot() {
                Ok(buf) => {
                    save_png(path, &buf);
                    println!("{}", path.display());
                }
                Err(e) => println!("snapshot failed: {e}"),
            }
            let _ = v.window().hide();
        }
        let _ = slint::quit_event_loop();
    });
    slint::run_event_loop_until_quit().unwrap();
}
