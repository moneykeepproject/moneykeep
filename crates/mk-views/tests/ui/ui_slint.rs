//! ui-slint.md acceptance tests: the move to Slint (About dialog, theme and style,
//! licence policy, no egui left) and revision 2 (real Slint screens).
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_views::testing::{Harness, Role};
use slint::ComponentHandle;

fn about_shown(h: &Harness) {
    assert!(
        h.query_by_role_and_label(Role::Heading, "About MoneyKeep")
            .is_some(),
        "{:?}",
        h.all_labels()
    );
    assert!(h.query_all_by_label("MoneyKeep").count() >= 1);
    assert!(
        h.query_all_by_label("Apps for Financial Management")
            .count()
            >= 1
    );
    assert!(has_contains(
        h,
        &format!("Version {}", env!("CARGO_PKG_VERSION"))
    ));
    assert!(has(h, "MoneyKeep is licensed under MIT OR Apache-2.0."));
    // release-prep.md §5: the project's page.
    assert!(has(
        h,
        "Project page: https://github.com/moneykeepproject/moneykeep"
    ));
    assert!(has(h, "Open the project page"));
    assert!(has(h, "Third-party licences"));
    assert!(has_contains(h, "Slint: Slint Royalty-free License 2.0"));
    // Slint's own About widget (licence section 2(a)).
    assert!(
        h.query_by_role_and_label(Role::Image, "About Slint")
            .is_some()
    );
}

/// AT2: the About dialog shows name, tag line, version, licence and the AboutSlint
/// widget, and is reachable from the top bar and the start screen.
#[test]
fn ui_slint_at2_about_dialog_from_top_bar_and_start_screen() {
    let (mut h, _b) = harness_with_chart();
    h.get_by_role_and_label(Role::Button, "About MoneyKeep")
        .click();
    h.run();
    about_shown(&h);
    click(&mut h, "Close");
    assert!(
        h.query_by_role_and_label(Role::Image, "About Slint")
            .is_none()
    );
    // From the start screen's footer link.
    click(&mut h, "Close book");
    click(&mut h, "About");
    about_shown(&h);
}

/// AT3: the app follows the operating system's light/dark setting by default
/// (the window's colour scheme is left to the system) and uses the Fluent style.
#[test]
fn ui_slint_at3_follows_system_theme_with_fluent_style() {
    let (mut h, _b) = harness_with_chart();
    assert_eq!(h.state().config.theme, mk_ui::Theme::System);
    assert_eq!(h.views().window().get_theme_choice(), 0);
    // A chosen theme is applied; back to the system's.
    h.state_mut().config.theme = mk_ui::Theme::Dark;
    h.run();
    assert!(
        h.views()
            .window()
            .global::<mk_views::sl::Theme>()
            .get_dark()
    );
    h.state_mut().config.theme = mk_ui::Theme::Light;
    h.run();
    assert!(
        !h.views()
            .window()
            .global::<mk_views::sl::Theme>()
            .get_dark()
    );
    let build = include_str!("../../build.rs");
    assert!(
        build.contains("with_style(\"fluent\".into())"),
        "Fluent style"
    );
}

/// AT1 (policy part): the licence exceptions in deny.toml are the Slint crate family
/// only (`cargo deny` itself runs in `cargo xtask ci`).
#[test]
fn ui_slint_at1_licence_exception_is_slint_only() {
    let deny = include_str!("../../../../deny.toml");
    let crates: Vec<&str> = deny
        .lines()
        .filter_map(|l| l.trim().strip_prefix("crate = \""))
        .map(|l| l.trim_end_matches('"'))
        .collect();
    assert!(!crates.is_empty());
    for c in &crates {
        assert!(
            *c == "slint" || c.starts_with("slint-") || c.starts_with("i-slint-"),
            "{c}"
        );
    }
    assert!(deny.contains("Owner decision 2026-11-02"));
    assert!(deny.contains("LicenseRef-Slint-Royalty-free-2.0"));
}

/// AT4: no egui crate remains in Cargo.lock.
#[test]
fn ui_slint_at4_no_egui_in_the_lock_file() {
    let lock = include_str!("../../../../Cargo.lock");
    for name in [
        "egui",
        "eframe",
        "egui_kittest",
        "epaint",
        "egui-winit",
        "emath",
    ] {
        assert!(
            !lock.contains(&format!("name = \"{name}\"\n")),
            "{name} is still in Cargo.lock"
        );
    }
}

/// Revision 2 AT9, as app-desktop.md revision 40 changed the tiles into cards: the
/// cards are all the same size and share their 1 px rules (no gap between cards on
/// a line).
#[test]
fn ui_slint_r2_at9_tiles_are_cards_with_centred_labels() {
    let dir = tempfile::tempdir().unwrap();
    let mut recent = Vec::new();
    for name in ["Household", "Church", "Shop"] {
        let path = dir.path().join(format!("{name}.moneykeep"));
        mk_commands::Session::create(&path, name, "USD", "app").unwrap();
        recent.push(mk_ui::RecentBook::from(path.to_str().unwrap()));
    }
    let app = mk_ui::ctl::App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        mk_ui::UserConfig {
            recent,
            ..mk_ui::UserConfig::default()
        },
    );
    let mut h = harness(app);
    h.set_size(mk_views::testing::vec2(1600.0, 900.0));
    h.run();
    let tiles: Vec<_> = [
        "Open Household",
        "Open Church",
        "Open Shop",
        "Create a New Book",
    ]
    .iter()
    .map(|l| h.get_by_label(l).rect())
    .collect();
    for w in tiles.windows(2) {
        assert_eq!(w[0].top(), w[1].top(), "one row");
        assert_eq!(w[1].left(), w[0].right(), "shared rules: {w:?}");
    }
    for t in &tiles {
        assert_eq!((t.width(), t.height()), (374.0, 312.0));
    }
}

/// Regression (owner's Windows build, 2026-11-03): build.rs once wrote absolute,
/// canonicalised import paths into the generated item.slint and leaf.slint. On
/// Windows (`\\?\C:\...`) they differed from the paths Slint resolves for relative
/// imports, so vm.slint was loaded twice and its structs renamed (`ChartVm2`,
/// `ItemVm2`), and the generated Rust failed with non-primitive casts. The views
/// are now compiled from one copy of ui/ in the build folder with relative imports
/// only.
#[test]
fn generated_views_use_relative_imports_and_load_each_file_once() {
    let out = std::path::Path::new(env!("OUT_DIR"));
    for name in ["item.slint", "leaf.slint"] {
        let text = std::fs::read_to_string(out.join("ui").join(name)).unwrap();
        for line in text
            .lines()
            .filter(|l| l.trim_start().starts_with("import"))
        {
            let path = line.split_once("from \"").map(|(_, p)| p).unwrap_or("");
            assert!(
                !path.starts_with('/') && !path.starts_with('\\') && path.get(1..2) != Some(":"),
                "{name}: absolute import: {line}"
            );
        }
    }
    let code = std::fs::read_to_string(out.join("app.rs")).unwrap();
    // A struct renamed because its file was loaded twice ends in a digit (ChartVm2).
    for vm in ["ItemVm", "CellVm", "ChartVm", "RowVm", "PageVm", "DialogVm"] {
        for n in 2..=9 {
            let twin = format!("{vm}{n}");
            assert!(
                !code
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .any(|w| w == twin),
                "the generated code has {twin}: a .slint file was loaded twice"
            );
        }
    }
}
