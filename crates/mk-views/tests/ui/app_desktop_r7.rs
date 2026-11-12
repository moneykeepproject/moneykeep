//! app-desktop.md revision 7 acceptance tests (52–57): text size, fonts, Settings
//! sections and layout at large sizes. Fonts come from an injected list (the
//! built-in font files under test names); nothing is read from the system.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::ctl::App;
use mk_ui::theme::{Density, Theme};
use mk_ui::{AppEvent, FontBytes, FontInfo, FontProvider, UserConfig};
use mk_views::testing::Harness;
use mk_views::testing::Key;
use serde_json::json;

type H = Harness;

/// Test fonts: the built-in font files under other family names.
struct TestFonts {
    fonts: Vec<(String, bool, Vec<u8>)>,
}

/// Stand-in font file bytes (the window finds installed fonts by family name; the
/// provider only has to say the file can be read).
fn builtin(name: &str) -> Vec<u8> {
    name.as_bytes().to_vec()
}

impl TestFonts {
    fn standard() -> Self {
        Self {
            fonts: vec![
                ("Test Sans".into(), false, builtin("Ubuntu-Light")),
                ("Test Mono".into(), true, builtin("Hack")),
            ],
        }
    }

    fn without(name: &str) -> Self {
        let mut f = Self::standard();
        f.fonts.retain(|(n, _, _)| n != name);
        f
    }
}

impl FontProvider for TestFonts {
    fn families(&self) -> Vec<FontInfo> {
        self.fonts
            .iter()
            .map(|(n, m, _)| FontInfo {
                family: n.clone(),
                monospace: *m,
            })
            .collect()
    }
    fn load(&self, family: &str) -> Option<FontBytes> {
        self.fonts
            .iter()
            .find(|(n, _, _)| n == family)
            .map(|(_, _, d)| FontBytes {
                data: d.clone(),
                index: 0,
            })
    }
}

fn app_with(config: UserConfig, fonts: TestFonts) -> App {
    let mut app = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        config,
    );
    app.set_font_provider(Box::new(fonts));
    app
}

fn display_settings(h: &mut H) {
    click(h, "Settings");
    click(h, "Display");
}

#[test]
fn app_desktop_at52_text_size_persists_and_follows_shortcuts() {
    // Set 150 % in Settings.
    let (mut app, _book) = new_book_app();
    app.set_font_provider(Box::new(TestFonts::standard()));
    let mut h = harness(app);
    display_settings(&mut h);
    assert!(has(&h, "Text size 100 %"));
    // 100 → 115 → 130 → 150.
    for _ in 0..3 {
        click(&mut h, "Larger text");
    }
    assert!(
        has(&h, "Text size 150 %"),
        "{:?}",
        labels_containing(&h, "Text size")
    );
    assert!(events(&mut h).contains(&AppEvent::ConfigChanged));
    // Close and reopen: the config round trip starts at 150 %.
    let saved = serde_json::to_string(&h.state().config).unwrap();
    let config: UserConfig = serde_json::from_str(&saved).unwrap();
    assert_eq!(config.text_size, 150);
    let (mut app, _book2) = new_book_app();
    app.config = config;
    app.set_font_provider(Box::new(TestFonts::standard()));
    let mut h = harness(app);
    h.run();
    assert!((h.zoom() - 1.5).abs() < 1e-6, "zoom {}", h.zoom());
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::Plus);
    h.run();
    assert_eq!(h.state().text_size(), 175);
    assert!((h.zoom() - 1.75).abs() < 1e-6);
    click(&mut h, "Settings");
    click(&mut h, "Display");
    assert!(has(&h, "Text size 175 %"));
    h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::Num0);
    h.run();
    assert_eq!(h.state().text_size(), 100);
    assert!(has(&h, "Text size 100 %"));
    // Out-of-range values are clamped.
    let mut h = harness(app_with(
        UserConfig {
            text_size: 400,
            ..UserConfig::default()
        },
        TestFonts::standard(),
    ));
    h.run();
    assert_eq!(h.state().text_size(), 200);
}

#[test]
fn app_desktop_at53_choose_font_and_missing_font_fallback() {
    let (mut app, _book) = new_book_app();
    app.set_font_provider(Box::new(TestFonts::standard()));
    let mut h = harness(app);
    display_settings(&mut h);
    click(&mut h, "Interface font");
    click(&mut h, "Interface font: Test Sans");
    h.run();
    assert_eq!(h.state().active_fonts().0.as_deref(), Some("Test Sans"));
    assert_eq!(h.state().config.ui_font.as_deref(), Some("Test Sans"));
    // Restart with the family gone: built-in font, one message, setting kept.
    let config = h.state().config.clone();
    let mut h = harness(app_with(config, TestFonts::without("Test Sans")));
    for _ in 0..3 {
        h.run();
    }
    assert_eq!(h.state().active_fonts().0, None);
    let msgs = labels_containing(&h, "isn't available");
    assert_eq!(
        msgs,
        ["The font Test Sans isn't available, so the built-in font is used."]
    );
    assert_eq!(h.state().config.ui_font.as_deref(), Some("Test Sans"));
}

#[test]
fn app_desktop_at54_number_font_lists_only_monospace() {
    let (mut app, _book) = new_book_app();
    app.set_font_provider(Box::new(TestFonts::standard()));
    let mut h = harness(app);
    display_settings(&mut h);
    click(&mut h, "Number font");
    assert!(has(&h, "Built-in monospace"));
    assert!(has(&h, "Number font: Test Mono"));
    assert!(!has(&h, "Number font: Test Sans"));
    click(&mut h, "Number font: Test Mono");
    h.run();
    assert_eq!(h.state().active_fonts().1.as_deref(), Some("Test Mono"));
    // The interface list has every family.
    click(&mut h, "Interface font");
    assert!(has(&h, "Interface font: Test Sans") && has(&h, "Interface font: Test Mono"));
}

/// The window in points: node rects are in points, so at 200 % a 1400×900 window is
/// 700×450 points.
#[allow(clippy::float_arithmetic)]
fn window(h: &H, size: mk_views::testing::Vec2) -> mk_views::testing::Rect {
    let ppp = h.zoom();
    let w = mk_views::testing::Rect::from_min_size(mk_views::testing::Pos2::ZERO, size / ppp);
    assert!(
        (h.window_rect().width() - w.width()).abs() < 1.0,
        "{w:?} / {:?}",
        h.window_rect()
    );
    w
}

fn no_cut_amounts(h: &H, what: &str) {
    let mut texts = Vec::new();
    for c in &h.output().shapes {
        if c.kind == "text" {
            texts.push(if c.elided {
                format!("{}…", c.text)
            } else {
                c.text.clone()
            });
        }
    }
    let cut: Vec<&String> = texts
        .iter()
        .filter(|t| t.contains('…') && t.chars().any(|c| c.is_ascii_digit()))
        .collect();
    assert!(cut.is_empty(), "{what}: truncated amounts {cut:?}");
}

#[test]
fn app_desktop_at55_layout_at_large_text_sizes() {
    for (percent, size) in [
        (200, mk_views::testing::vec2(1400.0, 900.0)),
        (150, mk_views::testing::vec2(1000.0, 700.0)),
    ] {
        let (mut h, _book) = harness_with_chart();
        h.set_size(size);
        h.state_mut().set_today("2026-11-05");
        while h.state().text_size() < percent {
            h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::Plus);
            h.run();
        }
        assert_eq!(h.state().text_size(), percent);
        cmd(
            &mut h,
            "txn.create",
            json!({"date": "2026-10-01", "payee": "Opening balance", "splits": [
                {"account": "Assets:Checking", "value": "12345.67"},
                {"account": "Opening Balances", "value": "-12345.67"}]}),
        );
        cmd(
            &mut h,
            "txn.create",
            json!({"date": "2026-10-03", "payee": "Pay", "splits": [
                {"account": "Assets:Checking", "value": "2500.00"},
                {"account": "Salary", "value": "-2500.00"}]}),
        );
        h.run();
        h.run();
        let w = window(&h, size);
        // The account list is in the rail's pane (revision 23), which floats over
        // the main area and slides away once the register opens.
        open_register(&mut h, "Assets:Checking");
        h.run();
        assert!(
            !has(&h, "Account Assets:Checking"),
            "{percent} %: the pane slid away"
        );
        for label in ["Balance", "+ New"] {
            let r = h.get_by_label(label).rect();
            assert!(
                w.contains_rect(r),
                "{percent} %: {label} at {r:?} outside {w:?}"
            );
        }
        no_cut_amounts(&h, "register");
        click(&mut h, "Reports");
        click(&mut h, "Income & expenses");
        click(&mut h, "Period");
        click(&mut h, "This year");
        h.run();
        let total = h.get_by_label("Total").rect();
        assert!(w.contains_rect(total), "{percent} %: Total at {total:?}");
        no_cut_amounts(&h, "income & expenses");
    }
}

#[test]
fn app_desktop_at56_settings_sections() {
    let (app, _book) = new_book_app();
    let mut h = harness(app);
    click(&mut h, "Settings");
    for s in ["Display", "Security", "Import", "Features"] {
        assert!(has(&h, s), "{s}");
    }
    // Display is first and shown by default.
    assert!(has(&h, "Text size 100 %"));
    click(&mut h, "Security");
    let r = h.get_by_label("Protect with a password…").rect();
    assert!(
        window(&h, mk_views::testing::vec2(1400.0, 900.0)).contains_rect(r),
        "{r:?}"
    );
    assert!(has(&h, "Lock after"));
    // The last section is remembered per user.
    assert_eq!(h.state().config.settings_section, "Security");
    settings_section(&mut h, "Features");
    assert!(has(&h, "Estimates"));
    click(&mut h, "Import");
    assert!(has_contains(&h, "Watched folder"));
}

#[test]
fn app_desktop_at57_reset_display_settings() {
    let config = UserConfig {
        theme: Theme::Dark,
        density: Density::Compact,
        text_size: 130,
        ui_font: Some("Test Sans".into()),
        number_font: Some("Test Mono".into()),
        ..UserConfig::default()
    };
    let (mut app, _book) = new_book_app();
    app.config = config;
    app.set_font_provider(Box::new(TestFonts::standard()));
    let mut h = harness(app);
    h.run();
    assert_eq!(
        h.state().active_fonts(),
        (Some("Test Sans".into()), Some("Test Mono".into()))
    );
    assert!((h.zoom() - 1.3).abs() < 1e-6);
    display_settings(&mut h);
    click(&mut h, "Reset display settings");
    h.run();
    let c = &h.state().config;
    assert_eq!(c.theme, Theme::System);
    assert_eq!(c.density, Density::Comfortable);
    assert_eq!(c.text_size, 100);
    assert_eq!(c.ui_font, None);
    assert_eq!(c.number_font, None);
    assert_eq!(h.state().active_fonts(), (None, None));
    assert!((h.zoom() - 1.0).abs() < 1e-6);
}

// ----- Revision 7a -----------------------------------------------------------------

/// Every drawn text: (text, elided, number of visible glyphs).
fn drawn(h: &H) -> Vec<(String, bool, usize)> {
    let mut out = Vec::new();
    for c in &h.output().shapes {
        if c.kind == "text" {
            out.push((c.text.clone(), c.elided, c.text.chars().count()));
        }
    }
    out
}

#[test]
fn app_desktop_at58_text_columns_keep_eight_characters() {
    let (mut h, _book) = harness_with_chart();
    h.set_size(mk_views::testing::vec2(1200.0, 800.0));
    h.state_mut().set_today("2026-10-20");
    for (name, start) in [
        ("Greenfield Utility Company", "2026-10-22"),
        ("Northside Insurance Brokers", "2026-10-29"),
    ] {
        cmd(
            &mut h,
            "schedule.create",
            json!({"name": name, "start": start, "recurrence": {"frequency": "weekly", "interval": 2},
                   "template": {"payee": name, "splits": [
                       {"account": "Expenses:Groceries", "value": "42.00"},
                       {"account": "Assets:Checking", "value": "-42.00"}]}}),
        );
    }
    // 130 %.
    while h.state().text_size() < 130 {
        h.key_press_modifiers(mk_views::testing::Modifiers::COMMAND, Key::Plus);
        h.run();
    }
    click(&mut h, "Scheduled");
    h.run();
    let texts = drawn(&h);
    for header in ["Payee", "Account", "Category"] {
        let found: Vec<_> = texts.iter().filter(|(t, _, _)| t == header).collect();
        assert!(!found.is_empty(), "{header} drawn");
        assert!(
            found.iter().all(|(_, elided, _)| !elided),
            "{header} header cut: {found:?}"
        );
    }
    for cell in [
        "Greenfield Utility Company",
        "Assets:Checking",
        "Expenses:Groceries",
    ] {
        let found: Vec<_> = texts.iter().filter(|(t, _, _)| t == cell).collect();
        assert!(!found.is_empty(), "{cell} drawn");
        for (_, elided, glyphs) in found {
            // At least the first 6 characters (plus "…" when cut) are visible.
            assert!(
                !elided || *glyphs >= 7,
                "{cell}: only {glyphs} glyphs shown"
            );
        }
    }
    // Repeats may truncate, with the full text in the tooltip (summary drawn).
    assert!(
        texts
            .iter()
            .any(|(t, _, _)| t.starts_with("Every 2 weeks on"))
    );
}
