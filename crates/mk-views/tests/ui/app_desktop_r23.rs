//! app-desktop.md revision 23: the accounts tab rail and its slide-out pane
//! (acceptance tests 129–137).
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_arithmetic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use crate::common::*;
use mk_ui::ctl::App;
use mk_ui::{AccountsPaneConfig, AppEvent, UserConfig};
use mk_views::testing::Role;
use mk_views::testing::{Key, Modifiers, pos2};
use serde_json::json;

type H = mk_views::testing::Harness;

/// The rail's right edge (the harness's root has a small margin before it; the
/// separator line takes the last point).
fn rail_right(h: &H) -> f32 {
    // The rail keeps a 4-point margin right of its tabs.
    rail_tab(h, "All accounts").rect().right() + 4.0
}

/// The standard chart with money in checking and on the card.
fn book_with(config: UserConfig) -> (H, TestBook) {
    let (mut h, b) = harness_with_chart_config(config);
    for (debit, credit, amount) in [
        ("Assets:Checking", "Opening Balances", "1000.00"),
        ("Expenses:Groceries", "Liabilities:Visa", "35.94"),
    ] {
        cmd(
            &mut h,
            "txn.create",
            json!({"date": "2026-10-03", "splits": [
                {"account": debit, "value": amount},
                {"account": credit, "value": format!("-{amount}")}]}),
        );
    }
    h.run();
    (h, b)
}

fn book() -> (H, TestBook) {
    book_with(iso_config())
}

/// The labels of the rail's tabs, top to bottom.
fn rail_labels(h: &H) -> Vec<String> {
    let mut tabs: Vec<(f32, String)> = h
        .query_all_by_role(Role::Tab)
        .filter(|n| n.rect().right() <= rail_right(h) + 1.0)
        .map(|n| {
            (
                n.rect().top(),
                n.accesskit_node().label().unwrap_or_default(),
            )
        })
        .collect();
    tabs.sort_by(|a, b| a.0.total_cmp(&b.0));
    tabs.into_iter().map(|(_, l)| l).collect()
}

fn selected(h: &H, group: &str) -> bool {
    rail_tab(h, group).accesskit_node().is_selected() == Some(true)
}

/// Whether the pane shows the account (its line is in the list).
fn listed(h: &H, full_name: &str) -> bool {
    has(h, &format!("Account {full_name}"))
}

/// The main area's left edge: Home's "Attention" heading.
fn main_left(h: &H) -> f32 {
    h.get_by_label("Attention").rect().left()
}

/// The pane's right edge: its Close button sits 8 points inside it.
fn pane_right(h: &H) -> f32 {
    // The pane's content has a 12-point margin.
    h.get_by_label("Close account list").rect().right() + 12.0
}

/// Text drawn this frame with its rect.
fn texts(h: &H) -> Vec<(String, mk_views::testing::Rect)> {
    let mut out = Vec::new();
    for c in &h.output().shapes {
        if c.kind == "text" && !c.text.trim().is_empty() {
            out.push((c.text.clone(), c.rect));
        }
    }
    out
}

/// Let `secs` of egui time pass (the harness steps a quarter second at a time).
fn wait(h: &mut H, secs: f32) {
    let steps = (secs * 4.0).ceil() as usize;
    h.run_steps(steps);
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

fn round_trip(config: &UserConfig) -> UserConfig {
    serde_json::from_str(&serde_json::to_string(config).unwrap()).unwrap()
}

#[test]
fn app_desktop_at129_one_tab_per_group_with_accounts() {
    let (h, _b) = book();
    let tabs = rail_labels(&h);
    let groups: Vec<&str> = tabs.iter().map(|t| t.split(',').next().unwrap()).collect();
    assert_eq!(
        groups,
        [
            // The Dashboards tab sits above them (revision 31).
            "Dashboards",
            "All accounts",
            "Cash & bank",
            "Credit cards",
            "Equity",
            "Categories"
        ],
        "{tabs:?}"
    );
    assert_eq!(tabs[2], "Cash & bank, 1,000.00");
    assert_eq!(tabs[3], "Credit cards, 35.94");
    // The icons are drawn shapes: the rail itself draws no text.
    let rail = mk_views::testing::Rect::from_min_max(
        pos2(0.0, rail_tab(&h, "All accounts").rect().top()),
        pos2(rail_right(&h), 900.0),
    );
    let in_rail: Vec<_> = texts(&h)
        .into_iter()
        .filter(|(_, r)| rail.contains_rect(*r))
        .collect();
    assert!(in_rail.is_empty(), "{in_rail:?}");
    // Closed at start: no tab is highlighted.
    assert!(tabs.iter().all(|t| {
        let group = t.split(',').next().unwrap();
        !selected(&h, group)
    }));

    // A book without a credit card has no Credit cards tab.
    let (app, _b2) = new_book_app();
    let mut h = harness(app);
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Checking", "type": "asset", "subtype": "bank"}),
    );
    h.run();
    let tabs = rail_labels(&h);
    assert!(
        tabs.iter().any(|t| t.starts_with("Cash & bank,")),
        "{tabs:?}"
    );
    assert!(
        !tabs.iter().any(|t| t.starts_with("Credit cards")),
        "{tabs:?}"
    );
    // Groups without accounts have no tab either.
    assert!(
        !tabs.iter().any(|t| t.starts_with("Investments")),
        "{tabs:?}"
    );
    h.run();
}

#[test]
fn app_desktop_at130_hover_tooltip_then_open_after_three_seconds() {
    let (mut h, _b) = book();
    rail_tab(&h, "Cash & bank").hover();
    h.run();
    wait(&mut h, 1.0);
    // The tooltip names the group and its total (as the group heading does).
    assert!(
        has(&h, "Cash & bank  1,000.00"),
        "{:?}",
        labels_containing(&h, "Cash")
    );
    assert!(!listed(&h, "Assets:Checking"), "not open yet");
    wait(&mut h, 2.25);
    assert!(listed(&h, "Assets:Checking"), "open after 3 seconds");
    assert!(!listed(&h, "Liabilities:Visa"), "only Cash & bank");
    assert!(selected(&h, "Cash & bank"));

    // Moving off before 3 seconds leaves it closed.
    let (mut h, _b) = book();
    rail_tab(&h, "Cash & bank").hover();
    h.run();
    wait(&mut h, 2.0);
    h.hover_at(pos2(900.0, 500.0));
    h.run();
    wait(&mut h, 3.0);
    assert!(!listed(&h, "Assets:Checking"));

    // Esc dismisses the tooltip while the pointer stays on the tab.
    let (mut h, _b) = book();
    rail_tab(&h, "Credit cards").hover();
    h.run();
    wait(&mut h, 0.75);
    assert!(has(&h, "Credit cards  35.94"));
    h.key_press(Key::Escape);
    h.step();
    h.step();
    assert!(!has(&h, "Credit cards  35.94"), "tooltip dismissed");
    assert!(!listed(&h, "Liabilities:Visa"), "and the pane not open yet");
}

#[test]
fn app_desktop_at131_click_opens_and_closes() {
    let (mut h, _b) = book();
    open_pane(&mut h, "Credit cards");
    assert!(listed(&h, "Liabilities:Visa"), "open at once");
    assert!(!listed(&h, "Assets:Checking"));
    assert!(
        has(&h, "Credit cards  35.94"),
        "the group heading with its total"
    );
    assert!(selected(&h, "Credit cards") && !selected(&h, "All accounts"));
    // Another tab switches the group.
    open_pane(&mut h, "All accounts");
    assert!(listed(&h, "Liabilities:Visa") && listed(&h, "Assets:Checking"));
    open_pane(&mut h, "Credit cards");
    open_pane(&mut h, "Credit cards");
    assert!(!listed(&h, "Liabilities:Visa"), "closed again");
    assert!(!selected(&h, "Credit cards"));
}

#[test]
fn app_desktop_at132_unpinned_pane_floats_and_slides_away() {
    let (mut h, _b) = book();
    let left = main_left(&h);
    // Beside the rail: the page margin and the block's own padding come first.
    assert!(
        (left - rail_right(&h)).abs() < 64.0,
        "main area beside the rail: {left}"
    );
    open_pane(&mut h, "Cash & bank");
    assert!(listed(&h, "Assets:Checking"));
    assert_eq!(main_left(&h), left, "the main area does not move");
    assert!(pane_right(&h) > left + 100.0, "the pane covers it");
    // The pointer away for under a second: still open; over a second: closed.
    h.hover_at(pos2(1000.0, 600.0));
    h.run();
    wait(&mut h, 0.5);
    assert!(listed(&h, "Assets:Checking"));
    wait(&mut h, 1.0);
    assert!(!listed(&h, "Assets:Checking"), "slid away");
    // Clicking an account opens its register and closes the pane.
    open_pane(&mut h, "All accounts");
    click(&mut h, "Account Liabilities:Visa");
    h.run();
    assert!(matches!(h.state().view(), mk_ui::View::Register { .. }));
    assert!(has(&h, "Charge"), "the Visa register");
    assert!(!listed(&h, "Assets:Checking"), "closed");
}

#[test]
fn app_desktop_at133_pinned_pane_docks_and_comes_back() {
    let (mut h, book) = book();
    let left = main_left(&h);
    open_pane(&mut h, "Cash & bank");
    click(&mut h, "Keep open");
    h.run();
    assert!(has(&h, "Let it slide away"), "pinned");
    let docked = main_left(&h);
    assert!(
        docked >= pane_right(&h) - 1.0 && docked > left + 100.0,
        "the main area starts right of the pane: {docked} / {}",
        pane_right(&h)
    );
    h.hover_at(pos2(1000.0, 600.0));
    h.run();
    wait(&mut h, 3.0);
    assert!(listed(&h, "Assets:Checking"), "stays open");
    // Saved (debounced) and restored at start-up.
    assert!(events(&mut h).contains(&AppEvent::ConfigChanged));
    let config = h.state().config.clone();
    assert_eq!(
        config.accounts_pane,
        AccountsPaneConfig {
            pinned: true,
            group: "Cash & bank".into(),
            open: true,
        }
    );
    let width = config.panels.sidebar;
    drop(h);
    let h = reopen(&book, round_trip(&config));
    assert!(listed(&h, "Assets:Checking"), "reopens open");
    assert!(!listed(&h, "Liabilities:Visa"), "on Cash & bank");
    assert!(selected(&h, "Cash & bank"));
    assert!(has(&h, "Let it slide away"), "pinned");
    assert_eq!(h.state().config.panels.sidebar, width);
    assert!((main_left(&h) - docked).abs() < 1.0, "same width");

    // A config written before revision 23 still loads (unpinned, closed, All).
    let old: UserConfig = serde_json::from_str(r#"{"recent": []}"#).unwrap();
    assert_eq!(old.accounts_pane, AccountsPaneConfig::default());

    // An unpinned pane always starts closed.
    let mut unpinned = config.clone();
    unpinned.accounts_pane.pinned = false;
    drop(h);
    let h = reopen(&book, unpinned);
    assert!(!listed(&h, "Assets:Checking"));
}

#[test]
fn app_desktop_at134_dragged_width_is_remembered() {
    let (mut h, book) = book();
    open_pane(&mut h, "All accounts");
    let right = pane_right(&h);
    assert!(
        (right - (rail_right(&h) + 260.0)).abs() < 1.5,
        "default width: {right}"
    );
    let grab = pos2(right - 3.0, 500.0);
    h.hover_at(grab);
    h.run();
    h.drag_at(grab);
    h.run();
    let to = pos2(rail_right(&h) + 300.0, 500.0);
    h.hover_at(to);
    h.run();
    h.drop_at(to);
    h.run();
    assert_eq!(h.state().config.panels.sidebar, Some(300));
    // Close and reopen: still 300.
    open_pane(&mut h, "All accounts");
    if has(&h, "Close account list") {
        click(&mut h, "Close account list");
    }
    assert!(!listed(&h, "Assets:Checking"));
    open_pane(&mut h, "All accounts");
    assert!(
        (pane_right(&h) - (rail_right(&h) + 300.0)).abs() < 1.5,
        "{}",
        pane_right(&h)
    );
    // And after a config round trip.
    let config = round_trip(&h.state().config);
    drop(h);
    let mut h = reopen(&book, config);
    open_pane(&mut h, "All accounts");
    assert!(
        (pane_right(&h) - (rail_right(&h) + 300.0)).abs() < 1.5,
        "{}",
        pane_right(&h)
    );
}

#[test]
fn app_desktop_at135_keyboard() {
    let (mut h, _b) = book();
    let all = rail_tab(&h, "All accounts").rect();
    // Tabbing reaches the rail before anything in the main area.
    let mut reached = false;
    for _ in 0..60 {
        key(&mut h, Key::Tab);
        if rail_tab(&h, "All accounts").is_focused() {
            reached = true;
            break;
        }
        let focused: Vec<mk_views::testing::Rect> = h
            .query_all_by(|n| n.is_focused())
            .map(|n| n.rect())
            .collect();
        assert!(
            focused
                .iter()
                .all(|r| !(r.top() >= all.top() - 1.0 && r.left() >= all.right())),
            "main area focused before the rail: {focused:?}"
        );
    }
    assert!(reached, "Tab reaches the rail");
    key(&mut h, Key::Tab);
    assert!(rail_tab(&h, "Cash & bank").is_focused());
    key(&mut h, Key::Enter);
    assert!(listed(&h, "Assets:Checking"), "Enter opens the pane");
    assert!(!listed(&h, "Liabilities:Visa"));
    assert!(selected(&h, "Cash & bank"));
    key(&mut h, Key::Escape);
    h.run();
    assert!(
        !listed(&h, "Assets:Checking"),
        "Esc closes an unpinned pane"
    );
    // Space acts like a click too.
    rail_tab(&h, "Credit cards").focus();
    h.run();
    key(&mut h, Key::Space);
    assert!(listed(&h, "Liabilities:Visa"), "Space opens it");
    // Ctrl+B toggles the pane on the last group shown.
    h.key_press_modifiers(Modifiers::COMMAND, Key::B);
    h.run();
    assert!(!listed(&h, "Liabilities:Visa"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::B);
    h.run();
    assert!(listed(&h, "Liabilities:Visa") && !listed(&h, "Assets:Checking"));
    // The accessible name is the group name and total, role tab, selected.
    let tab = rail_tab(&h, "Credit cards");
    assert_eq!(tab.accesskit_node().role(), Role::Tab);
    assert_eq!(
        tab.accesskit_node().label().as_deref(),
        Some("Credit cards, 35.94")
    );
    assert_eq!(tab.accesskit_node().is_selected(), Some(true));
}

#[test]
fn app_desktop_at136_reset_display_settings() {
    let mut config = pinned_config(iso_config());
    config.accounts_pane.group = "Credit cards".into();
    config.panels.sidebar = Some(320);
    let (mut h, _b) = book_with(config);
    assert!(listed(&h, "Liabilities:Visa"), "pinned open at start-up");
    click(&mut h, "Settings");
    click(&mut h, "Display");
    click(&mut h, "Reset display settings");
    h.run();
    let c = h.state().config.clone();
    assert_eq!(c.accounts_pane, AccountsPaneConfig::default());
    assert_eq!(c.panels.sidebar, None);
    assert!(!listed(&h, "Liabilities:Visa"), "closed");
    // Ctrl+B reopens on All accounts, unpinned, at the default width.
    h.key_press_modifiers(Modifiers::COMMAND, Key::B);
    h.run();
    assert!(selected(&h, "All accounts"));
    assert!(has(&h, "Keep open"), "unpinned");
    assert!(
        (pane_right(&h) - (rail_right(&h) + 260.0)).abs() < 1.5,
        "{}",
        pane_right(&h)
    );
}

#[test]
fn app_desktop_at137_rail_and_pinned_pane_at_200_percent() {
    let (mut h, _b) = book_with(pinned_config(iso_config()));
    h.set_size(mk_views::testing::vec2(1400.0, 900.0));
    while h.state().text_size() < 200 {
        h.key_press_modifiers(Modifiers::COMMAND, Key::Plus);
        h.run();
    }
    h.run();
    // Node rects are in points: at 200 % the window is 700×450 points.
    let window = h.window_rect();
    assert!((window.width() - 700.0).abs() < 1.0, "{window:?}");
    let rail = rail_right(&h);
    // Rail tabs: inside the window and the rail, none overlapping another.
    let tabs: Vec<mk_views::testing::Rect> = h
        .query_all_by_role(Role::Tab)
        .filter(|n| n.rect().right() <= rail + 1.0)
        .map(|n| n.rect())
        .collect();
    // Dashboards (revision 31), All accounts and three groups.
    assert_eq!(tabs.len(), 6, "{tabs:?}");
    for (i, a) in tabs.iter().enumerate() {
        assert!(window.contains_rect(*a), "tab {a:?} outside the window");
        for b in tabs.iter().skip(i + 1) {
            assert!(!a.intersects(b.shrink(0.5)), "tabs overlap: {a:?} {b:?}");
        }
    }
    // The pinned pane: inside the window, at most half its width, its lines and
    // amounts whole and clear of the rail.
    let right = h.get_by_label("Close account list").rect().right();
    assert!(right <= window.width() / 2.0 + rail, "{right}");
    for label in [
        "Account Assets:Checking",
        "Account Liabilities:Visa",
        "Let it slide away",
        "Close account list",
    ] {
        let r = h.get_by_label(label).rect();
        assert!(window.contains_rect(r), "{label} at {r:?}");
        assert!(r.left() >= rail, "{label} under the rail: {r:?}");
        assert!(r.right() <= right + 1.0, "{label} past the pane: {r:?}");
    }
    let cut: Vec<String> = texts(&h)
        .into_iter()
        .map(|(t, _)| t)
        .filter(|t| t.contains('…') && t.chars().any(|c| c.is_ascii_digit()))
        .collect();
    assert!(cut.is_empty(), "truncated amounts: {cut:?}");
    for amount in ["1,000.00", "35.94"] {
        for r in h.query_all_by_label(amount).map(|n| n.rect()) {
            assert!(window.contains_rect(r), "{amount} at {r:?}");
        }
    }
    // The rail keeps its width in points (it scales with the text size, as the
    // spec's "about 40 logical pixels, scaled by text size"; the Slint rail is 56
    // points on the 8-pixel grid, ui-slint.md revision 2).
    let rail_w = rail - rail_tab(&h, "All accounts").rect().left();
    assert!(rail_w <= 56.0, "rail {rail_w} points");
    // In a window this narrow the pinned pane floats instead of docking, so the main
    // area keeps all its room beside the rail (and the pane stays open).
    assert!(
        // (the page margin and the block's padding: 8 + 8 + 12 points).
        (main_left(&h) - rail).abs() < 32.0,
        "main area at {} beside the rail at {rail}",
        main_left(&h)
    );
    h.hover_at(pos2(650.0, 400.0));
    h.run();
    wait(&mut h, 2.0);
    assert!(has(&h, "Account Assets:Checking"), "pinned: stays open");
    // The main area keeps layout rule 1 with the rail present: the register's Balance
    // column and the + New button fit, no amount truncated. (The pinned
    // pane is closed to look at the register, as a user would.)
    open_register(&mut h, "Assets:Checking");
    click(&mut h, "Close account list");
    h.run();
    for label in ["Balance", "+ New", "Spent", "Received"] {
        let r = h.get_by_label(label).rect();
        assert!(
            window.contains_rect(r),
            "{label} at {r:?} outside {window:?}"
        );
    }
    for amount in ["1,000.00", "-35.94"] {
        for r in h.query_all_by_label(amount).map(|n| n.rect()) {
            assert!(window.contains_rect(r), "{amount} at {r:?}");
        }
    }
    let cut: Vec<String> = texts(&h)
        .into_iter()
        .map(|(t, _)| t)
        .filter(|t| t.contains('…') && t.chars().any(|c| c.is_ascii_digit()))
        .collect();
    assert!(cut.is_empty(), "register: truncated amounts {cut:?}");
}
