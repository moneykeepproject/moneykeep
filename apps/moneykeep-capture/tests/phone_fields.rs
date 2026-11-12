//! mobile-capture.md revision 8, test 59: the phone's text fields, drawn in the real
//! phone window with Slint's software renderer, in light and dark. With text in
//! **Pairing code** and **Name for this phone** and focus elsewhere, the text's
//! drawn colour measures at least 4.5:1 against the field's drawn fill, and the fill
//! differs from the card's. The placeholder measures at least 4.5:1 too. The same
//! holds for the expense form's **Merchant** and **Purpose**, and the bottom bar's
//! **Save for later** (a plain button) shows its text at 4.5:1 or more.
//! Revision 13, test 80: the phone's switches are drawn in the accent colour.
//!
//! Its own test binary: drawing needs the testing backend with a renderer, set up
//! once per process (`mk_views::testing::init_rendering`).
#![allow(clippy::unwrap_used, clippy::panic, clippy::float_arithmetic)]

use std::collections::BTreeMap;

use mk_views::slint::ComponentHandle;
use mk_views::testing::{PhoneHarness, Pixels, Rect, contrast, pos2};
use moneykeep_capture::outbox::Storage;
use moneykeep_capture::ui::{CaptureApp, Install, PickedPhoto, Platform, Request, Response};

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

/// A browser stand-in on the Pair screen: nothing stored, nothing sent.
struct Unpaired {
    store: Mem,
    loaded: Option<Vec<(String, String)>>,
}

impl Platform for Unpaired {
    fn now_ms(&self) -> i64 {
        1_793_000_000_000
    }
    fn today(&self) -> String {
        "2026-11-05".into()
    }
    fn online(&self) -> bool {
        true
    }
    fn default_device_name(&self) -> String {
        String::new()
    }
    fn random16(&self) -> [u8; 16] {
        [7; 16]
    }
    fn send(&mut self, _req: Request) -> u64 {
        1
    }
    fn poll(&mut self) -> Vec<(u64, Response)> {
        Vec::new()
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

type Phone = PhoneHarness<CaptureApp<Unpaired>>;

/// The field's fill: the colour that covers most of its inside.
fn fill_of(px: &Pixels, r: Rect) -> u32 {
    let mut counts: BTreeMap<u32, usize> = BTreeMap::new();
    for c in px.colors_in(r.shrink(4.0)) {
        *counts.entry(c).or_default() += 1;
    }
    counts.into_iter().max_by_key(|(_, n)| *n).unwrap().0
}

/// The text's drawn colour: inside the field, the colour that differs most from
/// the fill (the glyphs' solid parts).
fn ink_of(px: &Pixels, r: Rect, fill: u32) -> u32 {
    px.colors_in(r.shrink(4.0))
        .into_iter()
        .max_by(|a, b| contrast(*a, fill).total_cmp(&contrast(*b, fill)))
        .unwrap()
}

fn field(h: &Phone, label: &str) -> Rect {
    let n = h.get_by_label(label);
    assert!(
        n.aid().starts_with("input|") || n.aid().starts_with("multiline|"),
        "{label}: {}",
        n.aid()
    );
    n.rect()
}

/// Theme.field (light, dark).
fn field_fill(dark: bool) -> u32 {
    if dark { 0x37_33_2f } else { 0xef_eb_e5 }
}

/// Theme.accent (light, dark).
fn accent(dark: bool) -> u32 {
    if dark { 0x4f_b3_ae } else { 0x0f_5e_5c }
}

/// The expense form (from Home, paired): Merchant and Purpose with text and no
/// focus, and the bottom bar's Save for later (a plain button), all readable
/// against what they are drawn on (the owner's phone drew the Rev 7 Merchant and
/// Purpose with no box and Save for later white on white).
fn check_form(theme: i32, word: &str) {
    let dark = theme == 2;
    let app = CaptureApp::new(Unpaired {
        store: Mem::default(),
        loaded: Some(vec![("token".to_owned(), "t".repeat(43))]),
    });
    let mut h = Phone::rendered(app, 390.0, 844.0);
    h.view().window().set_theme_choice(theme);
    // The paired app opens on the Expense tab (revision 9).
    h.run_steps(3);
    h.get_by_label("Merchant").set_text("Corner Hardware");
    h.get_by_label("Purpose (optional)")
        .set_text("Shelf brackets");
    h.run_steps(3);
    let px = h.pixels();
    for label in ["Merchant", "Purpose (optional)"] {
        let r = field(&h, label);
        let fill = fill_of(&px, r);
        assert_eq!(fill, field_fill(dark), "{word} {label}: Theme.field");
        let ink = ink_of(&px, r, fill);
        let c = contrast(ink, fill);
        assert!(
            c >= 4.5,
            "{word} {label}: text {ink:06x} on fill {fill:06x} is only {c:.2}:1"
        );
        let card = px.at(r.left() - 18.0, r.center().y);
        assert_ne!(fill, card, "{word} {label}: the fill differs from the card");
        let border = px.at(r.left() - 11.5, r.top() + 20.0);
        assert_ne!(border, accent(dark), "{word} {label}: no focus border");
        assert_ne!(border, fill, "{word} {label}: a border is drawn");
        assert!(
            contrast(border, card) >= 3.0,
            "{word} {label}: border {border:06x} on card {card:06x} under 3:1"
        );
    }
    // Save for later: its text against its fill, its border against the bar.
    let b = h.get_by_label("Save for later");
    assert!(b.aid().starts_with("button|"), "{}", b.aid());
    let r = b.rect();
    let fill = fill_of(&px, r);
    let ink = ink_of(&px, r, fill);
    let c = contrast(ink, fill);
    assert!(
        c >= 4.5,
        "{word} Save for later: text {ink:06x} on {fill:06x} is only {c:.2}:1"
    );
    let edge = px.at(r.left() + 0.5, r.center().y);
    let bar = px.at(r.left() - 4.0, r.center().y);
    assert!(
        contrast(edge, bar) >= 3.0,
        "{word} Save for later: border {edge:06x} on {bar:06x} under 3:1"
    );
}

#[test]
fn at59_phone_fields_are_readable_unfocused() {
    for (theme, word) in [(1, "light"), (2, "dark")] {
        let app = CaptureApp::new(Unpaired {
            store: Mem::default(),
            loaded: Some(Vec::new()),
        });
        let mut h = Phone::rendered(app, 390.0, 844.0);
        h.view().window().set_theme_choice(theme);
        h.run_steps(3);
        assert_eq!(
            h.view().window().global::<mk_views::sl::Theme>().get_dark(),
            theme == 2,
            "{word}: the theme agrees with Fluent's"
        );
        h.get_by_label("Pairing code").set_text("ABCD2345");
        h.get_by_label("Name for this phone")
            .set_text("Owner's phone");
        h.run_steps(3);
        for label in ["Pairing code", "Name for this phone"] {
            let n = h.get_by_label(label);
            assert!(!n.value().is_empty(), "{word} {label}: text typed");
        }
        let px = h.pixels();
        for label in ["Pairing code", "Name for this phone"] {
            let r = field(&h, label);
            // The box around the text input: the input sits 12 px inside it.
            let fill = fill_of(&px, r);
            let ink = ink_of(&px, r, fill);
            let c = contrast(ink, fill);
            assert!(
                c >= 4.5,
                "{word} {label}: text {ink:06x} on fill {fill:06x} is only {c:.2}:1"
            );
            // The card: just outside the field's box (12 px padding, 1 px border).
            let card = px.at(r.left() - 18.0, r.center().y);
            assert_ne!(fill, card, "{word} {label}: the fill differs from the card");
            assert_eq!(fill, field_fill(theme == 2), "{word} {label}: Theme.field");
            // Focus is elsewhere: the border is the plain one, not the accent.
            let border = px.at(r.left() - 11.5, r.center().y);
            assert_ne!(
                border,
                accent(theme == 2),
                "{word} {label}: no focus border"
            );
            assert_ne!(border, fill, "{word} {label}: a border is drawn");
        }
        // The placeholder (the pairing code's hint) is readable too.
        h.get_by_label("Pairing code").set_text("");
        h.run_steps(3);
        let px = h.pixels();
        let r = field(&h, "Pairing code");
        let fill = fill_of(&px, r);
        let ink = ink_of(&px, r, fill);
        let c = contrast(ink, fill);
        assert!(
            c >= 4.5,
            "{word} placeholder {ink:06x} on {fill:06x} is only {c:.2}:1"
        );
        assert!(
            px.colors_in(Rect::from_min_max(
                pos2(r.left(), r.top()),
                pos2(r.right(), r.bottom())
            ))
            .len()
                > 100
        );
        check_form(theme, word);
    }
}

/// Theme.border (light, dark).
fn border(dark: bool) -> u32 {
    if dark { 0x45_41_3d } else { 0xdc_d7_cf }
}

const MILEAGE_LISTS: &str = r#"{"base_currency": "USD", "mileage_enabled": true,
    "reimbursable_enabled": true, "default_purpose": "business",
    "purpose_kinds": [{"kind": "business", "label": "Business"}],
    "vehicles": [{"id": "veh-1", "name": "2019 Honda CR-V"}],
    "saved_trips": [],
    "mileage_rates": [{"kind": "business", "from": "2026-01-01", "cents": "72.5"}]}"#;

/// mobile-capture.md revision 13, test 80: the Mileage form's Round trip switch, on,
/// draws its track in the accent colour (read from the drawn pixels) with a white
/// knob; off, in the border colour; in light and dark.
#[test]
fn at80_round_trip_switch_in_the_accent_colour() {
    for (theme, word) in [(1, "light"), (2, "dark")] {
        let dark = theme == 2;
        let app = CaptureApp::new(Unpaired {
            store: Mem::default(),
            loaded: Some(vec![
                ("token".to_owned(), "t".repeat(43)),
                ("lists".to_owned(), MILEAGE_LISTS.to_owned()),
                ("page".to_owned(), "mileage".to_owned()),
                ("mileage_tip_seen".to_owned(), "1".to_owned()),
            ]),
        });
        let mut h = Phone::rendered(app, 390.0, 1400.0);
        h.view().window().set_theme_choice(theme);
        h.run_steps(3);
        let n = h.get_by_label("Round trip");
        assert!(n.aid().starts_with("switch|"), "{word}: {}", n.aid());
        let r = n.rect();
        let y = r.center().y;
        // Off: the track's free end (right of the knob) is the border colour.
        let px = h.pixels();
        assert_eq!(px.at(r.left() + 44.0, y), border(dark), "{word} off track");
        assert_eq!(px.at(r.left() + 16.0, y), 0xff_ff_ff, "{word} off knob");
        h.get_by_label("Round trip").click();
        h.run_steps(6);
        let px = h.pixels();
        let on = px.at(r.left() + 10.0, y);
        assert_eq!(on, accent(dark), "{word} on track");
        assert_eq!(px.at(r.left() + 36.0, y), 0xff_ff_ff, "{word} on knob");
        // At least 3:1 against the card it sits on.
        let card = px.at(r.right() - 4.0, r.top() + 2.0);
        assert!(
            contrast(on, card) >= 3.0,
            "{word}: on track {on:06x} on {card:06x} under 3:1"
        );
    }
}
