//! app-desktop.md revision 25: compact top bar with icon buttons, the Features
//! menu and the calculator (acceptance tests 149–157 and 159; AT158, the window
//! icon, is tested in the app crate).
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_arithmetic
)]

use crate::common::*;
use mk_ui::ctl::App;
use mk_ui::{UserConfig, View};
use mk_views::testing::{Key, Modifiers, Pos2, pos2, vec2};
use mk_views::testing::{Role, Toggled};
use serde_json::json;

type H = mk_views::testing::Harness;

/// Let `secs` of egui time pass (the harness steps a quarter second at a time).
fn wait(h: &mut H, secs: usize) {
    h.run_steps(secs * 4);
}

/// The top bar's control with this accessible name and role.
fn control<'h>(h: &'h H, role: Role, name: &'h str) -> mk_views::testing::Node<'h> {
    h.get_by_role_and_label(role, name)
}

fn gear(h: &H) -> mk_views::testing::Node<'_> {
    control(h, Role::Tab, "Settings")
}

fn toggled(n: &mk_views::testing::Node<'_>) -> bool {
    n.accesskit_node().toggled() == Some(Toggled::True)
}

/// Open (or close) the calculator with its top bar button.
fn calculator_button(h: &mut H) {
    control(h, Role::Button, "Calculator").click();
    h.run();
}

/// Press calculator keys by their accessible names.
fn keys(h: &mut H, names: &[&str]) {
    for n in names {
        control(h, Role::Button, n).click();
        h.run();
    }
}

/// The calculator's display text ("Result …" without the prefix).
fn result(h: &H) -> String {
    let labels = labels_containing(h, "Result ");
    labels
        .iter()
        .find_map(|l| l.strip_prefix("Result "))
        .unwrap_or_else(|| panic!("no calculator display: {labels:?}"))
        .to_owned()
}

/// The calculator's title bar (drag here to move it).
fn title_bar(h: &H) -> mk_views::testing::Rect {
    control(h, Role::TitleBar, "Calculator").rect()
}

/// Drag the calculator's title bar by `delta`.
fn drag_title(h: &mut H, delta: mk_views::testing::Vec2) {
    let from = title_bar(h).center();
    drag(h, from, from + delta);
}

fn drag(h: &mut H, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run();
    h.drag_at(from);
    h.run();
    h.hover_at(pos2((from.x + to.x) / 2.0, (from.y + to.y) / 2.0));
    h.run();
    h.hover_at(to);
    h.run();
    h.drop_at(to);
    h.run();
}

/// Give the calculator the keyboard focus by clicking its display.
fn focus_calculator(h: &mut H) {
    let display = h
        .query_all_by_label_contains("Result ")
        .next()
        .unwrap()
        .rect();
    h.hover_at(display.center());
    h.run();
    h.drag_at(display.center());
    h.drop_at(display.center());
    h.run();
}

/// Text put on the clipboard by the last queued input event (the harness runs
/// one frame per event and keeps the last frame's output).
fn copied(h: &mut H) -> Vec<String> {
    h.step();
    let out = h.take_copied();
    h.run();
    out
}

fn round_trip(config: &UserConfig) -> UserConfig {
    serde_json::from_str(&serde_json::to_string(config).unwrap()).unwrap()
}

/// A fresh app over the same book file with `config` (a restart).
fn reopen(book: &TestBook, config: UserConfig) -> H {
    let mut app = App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        config,
    );
    app.open_book(book.path.to_str().unwrap());
    let mut h = harness(app);
    h.run();
    h
}

#[test]
fn app_desktop_at149_icon_buttons_with_names_and_tooltips() {
    let (mut h, _b) = harness_with_chart();
    for (role, name) in [
        (Role::Button, "Undo"),
        (Role::Button, "Redo"),
        (Role::Button, "Calculator"),
        (Role::Tab, "Settings"),
        (Role::Button, "Close book"),
    ] {
        let r = control(&h, role, name).rect();
        // Small icon buttons, all the same size, one control high (32 points on
        // the Slint 8-pixel grid; the old toolkit's were a text line high).
        assert!(
            (r.width() - r.height()).abs() < 1.0 && r.height() <= 32.5,
            "{name}: {r:?}"
        );
    }
    // Nothing to redo: dimmed (disabled), its tooltip still shown.
    assert!(disabled(&h, "Redo"));
    control(&h, Role::Button, "Redo").hover();
    h.run();
    wait(&mut h, 1);
    assert!(
        has(&h, "Nothing to redo"),
        "{:?}",
        labels_containing(&h, "redo")
    );

    control(&h, Role::Tab, "Settings").hover();
    h.run();
    wait(&mut h, 1);
    assert!(
        h.query_all_by_label("Settings").count() >= 2,
        "the tooltip says Settings"
    );

    open_register(&mut h, "Assets:Checking");
    add_txn(
        &mut h,
        &[
            ("payee", "Costco"),
            ("category", "Expenses:Groceries"),
            ("spent", "80.00"),
        ],
    );
    assert!(!disabled(&h, "Undo"));
    control(&h, Role::Button, "Undo").hover();
    h.run();
    wait(&mut h, 1);
    assert!(
        has(&h, "Undo new transaction"),
        "{:?}",
        labels_containing(&h, "Undo")
    );
}

#[test]
fn app_desktop_at150_gear_opens_settings_and_ctrl_comma() {
    let (mut h, _b) = harness_with_chart();
    // No "Settings" text item: the only control named Settings is the gear.
    let named: Vec<mk_views::testing::Rect> =
        h.query_all_by_label("Settings").map(|n| n.rect()).collect();
    assert_eq!(named.len(), 1, "{named:?}");
    assert!((named[0].width() - named[0].height()).abs() < 1.0);
    assert!(!toggled(&gear(&h)));
    gear(&h).click();
    h.run();
    assert_eq!(h.state().view(), &View::Settings);
    assert!(has(&h, "Display"), "the Settings view");
    assert!(toggled(&gear(&h)), "the gear shows selected");
    assert_eq!(gear(&h).accesskit_node().is_selected(), Some(true));

    click(&mut h, "Home");
    assert!(!toggled(&gear(&h)));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Comma);
    h.run();
    assert_eq!(h.state().view(), &View::Settings);
    assert!(toggled(&gear(&h)));
}

#[test]
fn app_desktop_at151_calculator_settings_close_book_rightmost() {
    for invoicing in [false, true] {
        let (mut h, _b) = harness_with_chart();
        if invoicing {
            cmd(&mut h, "module.enable", json!({"module": "invoicing"}));
            h.run();
        }
        open_register(&mut h, "Assets:Checking");
        // The top bar's controls: everything above the rail (whose first tab is
        // Dashboards since revision 31).
        let bar_bottom = rail_tab(&h, "Dashboards").rect().top();
        let mut controls: Vec<(f32, String)> = h
            .query_all_by_role(Role::Button)
            .chain(h.query_all_by_role(Role::Tab))
            .filter(|n| n.rect().bottom() <= bar_bottom)
            .map(|n| {
                (
                    n.rect().right(),
                    n.accesskit_node().label().unwrap_or_default(),
                )
            })
            .collect();
        controls.sort_by(|a, b| a.0.total_cmp(&b.0));
        let last: Vec<&str> = controls
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(|(_, l)| l.as_str())
            .collect();
        assert_eq!(
            last,
            ["Calculator", "Settings", "Close book"],
            "{controls:?}"
        );
        // At the far right of the window, on the same line as the navigation.
        let close = control(&h, Role::Button, "Close book").rect();
        assert!(close.right() > 1300.0, "{close:?}");
        let home = control(&h, Role::Tab, "Home").rect();
        assert!((close.center().y - home.center().y).abs() < 4.0);
    }
}

#[test]
fn app_desktop_at152_features_lists_module_screens_alphabetically() {
    let (mut h, _b) = harness_with_chart();
    assert!(features_button(&h).is_none(), "absent with no module on");
    for m in ["invoicing", "bills", "budgets"] {
        cmd(&mut h, "module.enable", json!({"module": m}));
    }
    h.run();
    assert_eq!(feature_items(&mut h), ["Budget", "Purchases", "Sales"]);
    // The old navigation items are gone.
    for item in ["Budget", "Purchases", "Sales"] {
        assert!(
            h.query_by_role_and_label(Role::Tab, item).is_none(),
            "{item}"
        );
    }
    for m in ["invoicing", "bills", "budgets"] {
        cmd(&mut h, "module.disable", json!({"module": m}));
    }
    h.run();
    assert!(features_button(&h).is_none(), "absent with all four off");
    assert!(feature_items(&mut h).is_empty());
    cmd(&mut h, "module.enable", json!({"module": "investments"}));
    h.run();
    assert_eq!(feature_items(&mut h), ["Securities"]);
    open_feature(&mut h, "Securities");
    assert_eq!(h.state().view(), &View::Securities);
}

#[test]
fn app_desktop_at153_open_screen_selects_features_and_checks_item() {
    let (mut h, _b) = harness_with_chart();
    for m in ["invoicing", "bills"] {
        cmd(&mut h, "module.enable", json!({"module": m}));
    }
    h.run();
    let selected = |h: &H| features_button(h).unwrap().accesskit_node().is_selected();
    assert_eq!(selected(&h), Some(false));
    open_feature(&mut h, "Sales");
    assert_eq!(h.state().view(), &View::Sales);
    assert_eq!(selected(&h), Some(true));
    // The menu: a menu button; Sales checked, Purchases not.
    let button = features_button(&h).unwrap();
    assert_eq!(
        button.accesskit_node().has_popup(),
        Some(mk_views::testing::HasPopup::Menu)
    );
    button.click();
    h.run();
    let item = |h: &H, name: &str| {
        h.get_by_role_and_label(Role::MenuItemCheckBox, name)
            .accesskit_node()
            .toggled()
    };
    assert_eq!(item(&h, "Sales"), Some(Toggled::True));
    assert_eq!(item(&h, "Purchases"), Some(Toggled::False));
    h.get_by_role_and_label(Role::MenuItemCheckBox, "Purchases")
        .click();
    h.run();
    assert_eq!(h.state().view(), &View::Purchases);
    assert!(
        h.query_by_role_and_label(Role::MenuItemCheckBox, "Sales")
            .is_none(),
        "the menu closed"
    );
    click(&mut h, "Home");
    assert_eq!(selected(&h), Some(false));
}

#[test]
fn app_desktop_at154_calculator_arithmetic_by_clicking() {
    let (mut h, _b) = harness_with_chart();
    assert!(!h.state().calculator_open());
    calculator_button(&mut h);
    assert!(h.state().calculator_open());
    assert!(has(&h, "Result 0"));
    keys(
        &mut h,
        &[
            "one", "two", "point", "five", "plus", "seven", "point", "five",
        ],
    );
    assert_eq!(result(&h), "7.5");
    let pending = h
        .query_all_by_label_contains("Result ")
        .next()
        .unwrap()
        .accesskit_node()
        .description();
    assert_eq!(pending.as_deref(), Some("Pending 12.5 +"));
    keys(&mut h, &["equals"]);
    assert_eq!(result(&h), "20");

    for (keys_pressed, shown) in [
        (
            &["two", "plus", "three", "multiply", "four", "equals"][..],
            "20",
        ),
        (
            &["one", "zero", "divide", "three", "equals"][..],
            "3.3333333333",
        ),
        (
            &["one", "divide", "zero", "equals"][..],
            "Can't divide by zero",
        ),
        (
            &[
                "two", "zero", "zero", "plus", "one", "zero", "percent", "equals",
            ][..],
            "220",
        ),
    ] {
        keys(&mut h, &["clear"]);
        keys(&mut h, keys_pressed);
        assert_eq!(result(&h), shown, "{keys_pressed:?}");
    }
    // = again repeats + 20.
    keys(&mut h, &["equals"]);
    assert_eq!(result(&h), "240");
    // Grouping, change sign, delete last digit, percent of a product.
    keys(&mut h, &["clear", "one", "two", "three", "four", "five"]);
    assert_eq!(result(&h), "12,345");
    keys(&mut h, &["delete last digit", "change sign"]);
    assert_eq!(result(&h), "-1,234");
    // Clicking the button again closes it.
    calculator_button(&mut h);
    assert!(!h.state().calculator_open());
    assert!(labels_containing(&h, "Result ").is_empty());
}

#[test]
fn app_desktop_at155_keyboard_copy_and_other_fields() {
    let (mut h, _b) = harness_with_chart();
    open_register(&mut h, "Assets:Checking");
    calculator_button(&mut h);
    // Opening it gives it the focus.
    h.type_keys("12*3");
    h.run();
    key(&mut h, Key::Enter);
    assert_eq!(result(&h), "36");
    assert!(
        matches!(h.state().view(), View::Register { .. }) && !has(&h, "Edit transaction"),
        "Enter went to the calculator, not the register"
    );
    // Ctrl+C (Cmd+C on macOS), as one key event.
    h.key_press_modifiers(Modifiers::COMMAND, Key::C);
    h.run();
    assert_eq!(copied(&mut h), ["36"]);
    // The Copy button copies too, ungrouped.
    keys(&mut h, &["multiply", "one", "zero", "zero", "equals"]);
    assert_eq!(result(&h), "3,600");
    control(&h, Role::Button, "Copy").click();
    assert_eq!(copied(&mut h), ["3600"]);

    // A register pop-up field takes typing while the calculator is open but not
    // focused.
    click(&mut h, "+ New");
    assert!(h.state().calculator_open());
    type_into(&mut h, "New payee", "Costco 12");
    assert_eq!(value(&h, "New payee"), "Costco 12");
    assert_eq!(result(&h), "3,600", "the calculator did not see the typing");
    key(&mut h, Key::Escape);
    assert!(h.state().calculator_open(), "Esc went to the pop-up");
}

#[test]
fn app_desktop_at156_unpinned_and_pinned() {
    let (mut h, book) = harness_with_chart();
    calculator_button(&mut h);
    // Unpinned: dragging the title bar moves it.
    let before = title_bar(&h);
    drag_title(&mut h, vec2(-300.0, -200.0));
    let after = title_bar(&h);
    assert!(
        (after.min - before.min - vec2(-300.0, -200.0)).length() < 2.0,
        "{before:?} -> {after:?}"
    );
    // Esc on an empty entry closes it (a typed entry is cleared first).
    focus_calculator(&mut h);
    h.type_keys("7");
    h.run();
    key(&mut h, Key::Escape);
    assert!(h.state().calculator_open());
    assert_eq!(result(&h), "0");
    key(&mut h, Key::Escape);
    assert!(!h.state().calculator_open());

    // Pinned.
    calculator_button(&mut h);
    let pin = control(&h, Role::Button, "Pin in place");
    assert!(!toggled(&pin));
    pin.click();
    h.run();
    assert!(toggled(&control(&h, Role::Button, "Pin in place")));
    let pinned_at = title_bar(&h);
    assert!(
        (pinned_at.min - after.min).length() < 2.0,
        "reopens where it was"
    );
    drag_title(&mut h, vec2(200.0, 100.0));
    assert!(
        (title_bar(&h).min - pinned_at.min).length() < 0.5,
        "a pinned calculator does not move"
    );
    focus_calculator(&mut h);
    key(&mut h, Key::Escape);
    assert!(h.state().calculator_open(), "Esc does not close it");
    click(&mut h, "Reports");
    assert_eq!(h.state().view(), &View::Reports);
    assert!(h.state().calculator_open(), "stays open across screens");

    // Config round trip: reopens pinned at the same position.
    wait(&mut h, 2);
    let config = round_trip(&h.state().config);
    assert!(config.calculator.pinned && config.calculator.open);
    let pos = h.state().calculator_pos().unwrap();
    drop(h);
    let h = reopen(&book, config);
    assert!(h.state().calculator_open());
    assert!(toggled(&control(&h, Role::Button, "Pin in place")));
    assert_eq!(h.state().calculator_pos(), Some(pos));
    assert!((title_bar(&h).min - pinned_at.min).length() < 0.5);

    // Reset display settings puts it back unpinned at its default place.
    let mut h = h;
    click(&mut h, "Settings");
    click(&mut h, "Display");
    click(&mut h, "Reset display settings");
    h.run();
    assert_eq!(h.state().config.calculator.pos, None);
    assert!(!h.state().config.calculator.pinned);
    assert!(!toggled(&control(&h, Role::Button, "Pin in place")));
}

#[test]
fn app_desktop_at157_kept_inside_the_window() {
    let (mut h, _b) = harness_with_chart();
    calculator_button(&mut h);
    let inside = |h: &H| {
        let title = title_bar(h);
        let close = control(h, Role::Button, "Close calculator").rect();
        let copy = control(h, Role::Button, "Copy").rect();
        let window = h.window_rect();
        assert!(
            title.left() >= window.left()
                && title.top() >= window.top()
                && close.right() <= window.right()
                && copy.bottom() <= window.bottom(),
            "{title:?} {close:?} {copy:?} in {window:?}"
        );
    };
    inside(&h);
    drag_title(&mut h, vec2(900.0, 900.0));
    inside(&h);
    drag_title(&mut h, vec2(-3000.0, -3000.0));
    inside(&h);
    // Also after the window shrinks.
    drag_title(&mut h, vec2(2000.0, 2000.0));
    h.set_size(vec2(1000.0, 700.0));
    h.run();
    h.run();
    inside(&h);
}

#[test]
fn app_desktop_at159_calculator_never_changes_the_book() {
    let (mut h, _b) = harness_with_chart();
    let audit = |h: &mut H| cmd(h, "audit.list", json!({"limit": 1000}));
    let history = |h: &mut H| cmd(h, "history.list", json!({"limit": 1000}));
    let (a0, h0) = (audit(&mut h), history(&mut h));
    calculator_button(&mut h);
    keys(
        &mut h,
        &["four", "plus", "two", "equals", "percent", "change sign"],
    );
    h.type_keys("9/3=");
    h.run();
    key(&mut h, Key::Backspace);
    key(&mut h, Key::Delete);
    control(&h, Role::Button, "Copy").click();
    h.run();
    control(&h, Role::Button, "Pin in place").click();
    h.run();
    calculator_button(&mut h);
    assert_eq!(audit(&mut h), a0);
    assert_eq!(history(&mut h), h0);
}

/// The calculator's keys are drawn (no font glyph needed for ÷ × − ⌫ ±) and each
/// has the accessible name of its meaning.
#[test]
fn app_desktop_r25_calculator_key_names() {
    let (mut h, _b) = harness_with_chart();
    calculator_button(&mut h);
    for name in [
        "zero",
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "point",
        "plus",
        "minus",
        "multiply",
        "divide",
        "equals",
        "clear",
        "delete last digit",
        "change sign",
        "percent",
        "Copy",
        "Pin in place",
        "Close calculator",
    ] {
        assert!(
            h.query_by_role_and_label(Role::Button, name).is_some(),
            "{name}"
        );
    }
    control(&h, Role::Button, "Close calculator").click();
    h.run();
    assert!(!h.state().calculator_open());
}

/// The top bar keeps a single line at 1000×700 with every module screen on.
#[test]
fn app_desktop_r25_top_bar_fits_at_minimum_size() {
    let (mut h, _b) = harness_with_chart();
    for m in ["invoicing", "bills", "budgets", "investments"] {
        cmd(&mut h, "module.enable", json!({"module": m}));
    }
    h.set_size(vec2(1000.0, 700.0));
    h.run();
    let home = control(&h, Role::Tab, "Home").rect();
    for (role, name) in [
        (Role::Button, "Features"),
        (Role::Button, "Calculator"),
        (Role::Tab, "Settings"),
        (Role::Button, "Close book"),
    ] {
        let r = control(&h, role, name).rect();
        assert!(r.right() <= 1000.0, "{name} {r:?}");
        assert!(
            (r.center().y - home.center().y).abs() < 4.0,
            "{name} wrapped"
        );
    }
}
