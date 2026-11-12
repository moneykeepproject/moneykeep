//! dashboards.md revision 4 (Test 20): chart legends always visible — the legend
//! and the bars' labels stay inside the block at its minimum, a medium and the
//! narrow size; a taller block gives a taller drawing. Revision 5 (Tests 21–24):
//! a chart's bars, points and legend open what they stand for, with the pointer
//! and the keyboard; nothing opens in Edit layout.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_arithmetic
)]

use crate::common::*;
use mk_ui::View;
use mk_views::testing::{Key, Rect, Role, vec2};
use serde_json::{Value, json};

type H = mk_views::testing::Harness;

const TODAY: &str = "2026-10-20";
/// The body text line height in points at the default style.
const LINE: f32 = 16.0;
/// The block's padding (`BlockView`): nothing drawn closer to its edge than this.
const PAD: f32 = 8.0;

/// Seven spending categories and a year of income.
pub(crate) fn chart_book() -> (H, TestBook) {
    let (mut h, b) = harness_with_chart();
    h.state_mut().set_today(TODAY);
    for name in [
        "Utilities",
        "Insurance",
        "Dining",
        "Clothing",
        "Phone",
        "Gifts",
    ] {
        cmd(
            &mut h,
            "account.create",
            json!({"name": name, "type": "expense", "parent": "Expenses"}),
        );
    }
    for (date, account, amount) in [
        ("2026-06-30", "Salary", "-3200.00"),
        ("2026-07-31", "Salary", "-3200.00"),
        ("2026-08-31", "Salary", "-3350.00"),
        ("2026-09-30", "Salary", "-3350.00"),
        ("2026-07-04", "Expenses:Groceries", "182.40"),
        ("2026-08-09", "Expenses:Utilities", "88.60"),
        ("2026-08-20", "Expenses:Insurance", "120.00"),
        ("2026-09-02", "Expenses:Dining", "64.00"),
        ("2026-09-12", "Expenses:Clothing", "79.99"),
        ("2026-10-05", "Expenses:Phone", "55.00"),
        ("2026-10-11", "Expenses:Gifts", "35.00"),
        ("2026-10-12", "Expenses:Groceries", "61.35"),
    ] {
        let money = amount
            .strip_prefix('-')
            .map_or_else(|| format!("-{amount}"), str::to_owned);
        cmd(
            &mut h,
            "txn.create",
            json!({"date": date, "payee": "Shop", "splits": [
                {"account": account, "value": amount},
                {"account": "Assets:Checking", "value": money}]}),
        );
    }
    h.run();
    (h, b)
}

pub(crate) fn dashboard(h: &mut H, name: &str) -> String {
    cmd(h, "dashboard.create", json!({"name": name}))["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

pub(crate) fn add_chart(h: &mut H, dash: &str, title: &str, chart: &str, period: &str) -> String {
    cmd(
        h,
        "dashboard.add_block",
        json!({"dashboard": dash, "type": "chart", "title": title,
            "settings": {"chart": chart, "period": period}}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

pub(crate) fn show(h: &mut H, name: &str) {
    h.state_mut().open_view_spec(&format!("dashboard:{name}"));
    h.run();
}

fn place(h: &mut H, dash: &str, cells: &[(&str, u32, u32, u32, u32)]) {
    let layout: Vec<Value> = cells
        .iter()
        .map(|(b, x, y, w, hh)| json!({"block": b, "x": x, "y": y, "w": w, "h": hh}))
        .collect();
    cmd(
        h,
        "dashboard.move_blocks",
        json!({"dashboard": dash, "layout": layout}),
    );
    h.run();
}

/// One chart block as drawn: the block's bounds, its drawing area, its legend
/// entries and its labels.
#[derive(Debug)]
struct Drawn {
    block: Rect,
    drawing: Rect,
    legend: Vec<Rect>,
    labels: Vec<(String, Rect)>,
}

fn inside(outer: Rect, r: Rect) -> bool {
    r.left() >= outer.left() - 0.5
        && r.right() <= outer.right() + 0.5
        && r.top() >= outer.top() - 0.5
        && r.bottom() <= outer.bottom() + 0.5
}

/// The chart whose block has this title.
fn drawn(h: &H, title: &str) -> Drawn {
    let t = h.get_by_label(title).rect();
    let block = h
        .query_all_by(|n| n.tag() == "boardblock")
        .map(|n| n.rect())
        .find(|r| r.contains(t.center()))
        .unwrap_or_else(|| panic!("no block around {title:?}"));
    let drawing = h
        .query_all_by(|n| n.role() == mk_views::testing::Role::Image)
        .map(|n| n.rect())
        .find(|r| block.contains(r.min))
        .unwrap_or_else(|| {
            panic!(
                "no chart in {title:?} {block:?}: {:?}",
                h.query_all_by(|n| !n.tag().is_empty())
                    .map(|n| format!("{} {:?} {:?}", n.tag(), n.label(), n.rect()))
                    .collect::<Vec<_>>()
            )
        });
    // A legend entry: plain, or a link when it opens its series (revision 5).
    let legend = h
        .query_all_by(|n| {
            n.tag() == "legend" || (n.tag() == "link" && n.item_id().contains("/legend:"))
        })
        .map(|n| n.rect())
        .filter(|r| block.contains(r.min))
        .collect();
    let labels = h
        .query_all_by(|n| n.tag() == "chartlabel")
        .filter(|n| block.contains(n.rect().min))
        .map(|n| (n.label().unwrap_or_default(), n.rect()))
        .collect();
    Drawn {
        block,
        drawing,
        legend,
        labels,
    }
}

/// Test 20's checks for one block: every legend entry and bar label fully inside
/// the block's padding, the drawing at least two text lines tall.
fn check(h: &H, title: &str, legend: usize, bars: usize, at: &str) -> Drawn {
    let d = drawn(h, title);
    let area = d.block.shrink(PAD - 0.5);
    assert_eq!(d.legend.len(), legend, "{at}: {title} legend {d:?}");
    for r in &d.legend {
        assert!(
            r.width() > 20.0,
            "{at}: {title} legend entry squeezed {r:?}"
        );
        assert!(
            inside(area, *r),
            "{at}: {title} legend {r:?} outside {area:?}"
        );
    }
    assert!(inside(area, d.drawing), "{at}: {title} drawing {d:?}");
    if let Some(last) = d.legend.iter().map(|r| r.bottom()).reduce(f32::max) {
        assert!(last > d.drawing.bottom(), "{at}: legend under the drawing");
    }
    let cats: Vec<&(String, Rect)> = d
        .labels
        .iter()
        .filter(|(l, _)| l.starts_with("Expenses:"))
        .collect();
    assert!(cats.len() >= bars, "{at}: {title} bars {:?}", d.labels);
    for (l, r) in &d.labels {
        assert!(
            inside(area, *r),
            "{at}: {title} label {l:?} {r:?} outside {area:?}"
        );
    }
    assert!(
        d.drawing.height() >= 2.0 * LINE,
        "{at}: {title} drawing {:?}",
        d.drawing
    );
    d
}

#[test]
fn dashboards_at20_legends_and_bar_labels_stay_inside_chart_blocks() {
    let (mut h, _b) = chart_book();
    let dash = dashboard(&mut h, "Charts");
    let ie = add_chart(
        &mut h,
        &dash,
        "Money in and out",
        "income_expense",
        "last_12_months",
    );
    let sp = add_chart(&mut h, &dash, "Where it went", "spending", "this_year");
    // The minimum size (4 × 3).
    place(&mut h, &dash, &[(&ie, 0, 0, 4, 3), (&sp, 4, 0, 4, 3)]);
    show(&mut h, "Charts");
    let small = check(&h, "Money in and out", 3, 0, "minimum");
    let small_sp = check(&h, "Where it went", 0, 7, "minimum");
    // One row taller: a taller drawing, the legend still inside.
    place(&mut h, &dash, &[(&ie, 0, 0, 4, 4), (&sp, 4, 0, 4, 4)]);
    let taller = check(&h, "Money in and out", 3, 0, "one row taller");
    let taller_sp = check(&h, "Where it went", 0, 7, "one row taller");
    assert!(
        taller.drawing.height() > small.drawing.height() + 50.0,
        "{small:?} {taller:?}"
    );
    assert!(taller_sp.drawing.height() > small_sp.drawing.height() + 50.0);
    // A medium size (6 × 4).
    place(&mut h, &dash, &[(&ie, 0, 0, 6, 4), (&sp, 6, 0, 6, 4)]);
    check(&h, "Money in and out", 3, 0, "medium");
    check(&h, "Where it went", 0, 7, "medium");
    // The narrow layout (one column at full width), back at the minimum size.
    place(&mut h, &dash, &[(&ie, 0, 0, 4, 3), (&sp, 4, 0, 4, 3)]);
    h.set_size(vec2(640.0, 900.0));
    h.run();
    let n = check(&h, "Money in and out", 3, 0, "narrow");
    let n_sp = check(&h, "Where it went", 0, 7, "narrow");
    assert!(n_sp.block.top() >= n.block.bottom(), "one column");
}

/// A narrow block wraps its legend onto more lines, all inside the block, and
/// the drawing gives up the room.
#[test]
fn dashboards_at20_legend_wraps_in_a_narrow_block() {
    let (mut h, _b) = chart_book();
    let dash = dashboard(&mut h, "Charts");
    let nw = add_chart(&mut h, &dash, "Worth", "net_worth", "last_12_months");
    place(&mut h, &dash, &[(&nw, 0, 0, 12, 3)]);
    show(&mut h, "Charts");
    let wide = check(&h, "Worth", 3, 0, "full width");
    let lines = |d: &Drawn| {
        d.legend
            .iter()
            .map(|r| r.top().round() as i32)
            .collect::<std::collections::BTreeSet<i32>>()
            .len()
    };
    assert_eq!(lines(&wide), 1, "{wide:?}");
    // A window just wide enough for twelve columns, the block at its narrowest.
    place(&mut h, &dash, &[(&nw, 0, 0, 4, 3)]);
    h.set_size(vec2(860.0, 900.0));
    h.run();
    let narrow = check(&h, "Worth", 3, 0, "narrow block");
    assert!(lines(&narrow) >= 2, "{narrow:?}");
    assert!(narrow.drawing.height() < wide.drawing.height());
    // Entries on a line keep even gaps and never overlap.
    let mut l = narrow.legend.clone();
    l.sort_by(|a, b| {
        (a.top(), a.left())
            .partial_cmp(&(b.top(), b.left()))
            .unwrap()
    });
    for w in l.windows(2) {
        if (w[0].top() - w[1].top()).abs() < 1.0 {
            assert!(w[1].left() >= w[0].right() + 8.0, "{l:?}");
        }
    }
}

/// The links of a chart's pieces whose names contain `part`.
fn pieces<'h>(h: &'h H, part: &str) -> Vec<mk_views::testing::Node<'h>> {
    h.query_all_by(|n| {
        n.role() == Role::Link
            && n.item_id().contains("/piece:")
            && n.label().is_some_and(|l| l.contains(part))
    })
    .collect()
}

fn piece_at(h: &H, part: &str) -> Rect {
    let found = pieces(h, part);
    assert_eq!(found.len(), 1, "one piece named {part:?}: {found:?}");
    found[0].rect()
}

fn spending_board(h: &mut H) -> String {
    let dash = dashboard(h, "Charts");
    let sp = add_chart(h, &dash, "Where it went", "spending", "this_year");
    place(h, &dash, &[(&sp, 0, 0, 6, 4)]);
    show(h, "Charts");
    dash
}

/// Test 21: a click on a category's bar opens its register for the block's period;
/// the pointer shows a tooltip with the label and value first; Back returns.
#[test]
fn dashboards_at21_category_bar_opens_its_register_for_the_period() {
    let (mut h, _b) = chart_book();
    spending_board(&mut h);
    let bar = piece_at(&h, "Expenses:Dining —");
    h.hover_at(bar.center());
    h.run();
    assert_eq!(
        h.tooltip().as_deref(),
        Some("Expenses:Dining — 64.00, 2026-01-01 – 2026-12-31")
    );
    h.click_at(bar.center());
    h.run();
    assert_eq!(
        h.state().view(),
        &View::Register {
            account: "Expenses:Dining".into()
        }
    );
    assert_eq!(value(&h, "From date"), "2026-01-01");
    assert_eq!(value(&h, "To date"), "2026-12-31");
    // Back returns to the dashboard.
    click(&mut h, "Back");
    assert!(
        matches!(h.state().view(), View::Dashboard { .. }),
        "{:?}",
        h.state().view()
    );
    // The empty chart area still opens the report.
    let groceries = piece_at(&h, "Expenses:Groceries —");
    let drawing = h
        .query_all_by(|n| n.role() == Role::Image)
        .map(|n| n.rect())
        .find(|r| r.contains(groceries.center()))
        .unwrap();
    h.click_at(mk_views::testing::pos2(
        drawing.right() - 4.0,
        drawing.bottom() - 4.0,
    ));
    h.run();
    assert_eq!(h.state().view(), &View::Reports);
}

/// Test 22: a click on a month's income bar opens Income & expenses for that month.
#[test]
fn dashboards_at22_month_bar_opens_income_and_expenses_for_the_month() {
    let (mut h, _b) = chart_book();
    let dash = dashboard(&mut h, "Charts");
    let ie = add_chart(
        &mut h,
        &dash,
        "Money in and out",
        "income_expense",
        "last_12_months",
    );
    place(&mut h, &dash, &[(&ie, 0, 0, 8, 4)]);
    show(&mut h, "Charts");
    let bar = piece_at(&h, "Income — 3,350.00, Aug 2026");
    // At least 24 × 24 points to hit.
    assert!(bar.width() >= 23.5 && bar.height() >= 23.5, "{bar:?}");
    h.click_at(bar.center());
    h.run();
    assert_eq!(h.state().view(), &View::Reports);
    assert_eq!(
        h.get_by_role_and_label(Role::Tab, "Income & expenses")
            .accesskit_node()
            .toggled(),
        Some(mk_views::testing::Toggled::True)
    );
    assert_eq!(value(&h, "From date"), "2026-08-01");
    assert_eq!(value(&h, "To date"), "2026-08-31");
    // The legend's Income opens the report for the block's period.
    show(&mut h, "Charts");
    let legend = h
        .query_all_by(|n| n.role() == Role::Link && n.item_id().contains("/legend:"))
        .map(|n| (n.label().unwrap_or_default(), n.rect()))
        .collect::<Vec<_>>();
    assert_eq!(legend.len(), 3, "{legend:?}");
    assert_eq!(legend[0].0, "Income (legend)");
    h.click_at(legend[0].1.center());
    h.run();
    assert_eq!(h.state().view(), &View::Reports);
    // The block's own period (a preset: no custom dates).
    assert!(!has(&h, "From date"));
}

/// Test 23: Tab reaches the chart, Right twice, Enter opens the third piece; every
/// piece is a link named with its value.
#[test]
fn dashboards_at23_keyboard_reaches_and_opens_pieces() {
    let (mut h, _b) = chart_book();
    spending_board(&mut h);
    let names: Vec<String> = pieces(&h, " — ")
        .iter()
        .map(|n| n.label().unwrap())
        .collect();
    assert_eq!(names.len(), 7, "{names:?}");
    for n in &names {
        assert!(
            n.starts_with("Expenses:")
                && n.contains(" — ")
                && n.chars().any(|c| c.is_ascii_digit()),
            "{n}"
        );
    }
    let mut tabs = 0;
    while !h
        .state()
        .focused_item()
        .is_some_and(|f| f.ends_with("/chart"))
    {
        key(&mut h, Key::Tab);
        tabs += 1;
        assert!(tabs < 80, "Tab never reached the chart");
    }
    key(&mut h, Key::ArrowRight);
    key(&mut h, Key::ArrowRight);
    key(&mut h, Key::Enter);
    // The third piece in reading order (largest first).
    let third = names[2].split(" — ").next().unwrap().to_owned();
    assert_eq!(third, "Expenses:Utilities", "{names:?}");
    assert_eq!(h.state().view(), &View::Register { account: third });
}

/// Test 24: in Edit layout a click on a bar opens nothing.
#[test]
fn dashboards_at24_edit_layout_clicks_open_nothing() {
    let (mut h, _b) = chart_book();
    spending_board(&mut h);
    let bar = piece_at(&h, "Expenses:Groceries —");
    click(&mut h, "Dashboard menu");
    click(&mut h, "Edit layout");
    assert!(
        pieces(&h, " — ").is_empty(),
        "no clickable pieces while editing"
    );
    h.click_at(bar.center());
    h.run();
    assert!(
        matches!(h.state().view(), View::Dashboard { .. }),
        "{:?}",
        h.state().view()
    );
    assert!(has(&h, "Done"));
}

/// The range a chart's bar asked for belongs to that register only.
#[test]
fn dashboards_r5_chart_range_does_not_follow_to_other_registers() {
    let (mut h, _b) = chart_book();
    spending_board(&mut h);
    h.click_at(piece_at(&h, "Expenses:Dining —").center());
    h.run();
    assert_eq!(value(&h, "From date"), "2026-01-01");
    open_register(&mut h, "Assets:Checking");
    assert_eq!(value(&h, "From date"), "");
    assert_eq!(value(&h, "To date"), "");
}

/// Reports-screen charts follow the same table: a category's bar opens its
/// register for the report's period.
#[test]
fn dashboards_r5_reports_screen_spending_bar_opens_the_register() {
    let (mut h, _b) = chart_book();
    h.state_mut().navigate(View::Reports);
    h.run();
    click(&mut h, "Spending");
    let bar = pieces(&h, "Expenses:Phone —");
    assert_eq!(bar.len(), 1);
    let at = bar[0].rect().center();
    h.click_at(at);
    h.run();
    assert_eq!(
        h.state().view(),
        &View::Register {
            account: "Expenses:Phone".into()
        }
    );
    assert_eq!(value(&h, "From date"), "2026-10-01");
}

/// A line chart's point has a hit area of at least 24 × 24 points and opens the
/// Net worth report as of that month's end.
#[test]
fn dashboards_r5_net_worth_point_opens_the_report_as_of_month_end() {
    let (mut h, _b) = chart_book();
    let dash = dashboard(&mut h, "Charts");
    let nw = add_chart(&mut h, &dash, "Worth", "net_worth", "last_12_months");
    place(&mut h, &dash, &[(&nw, 0, 0, 6, 4)]);
    show(&mut h, "Charts");
    let points = pieces(&h, ", ");
    assert_eq!(points.len(), 12, "{points:?}");
    for p in &points {
        assert!(
            p.rect().width() >= 23.5 && p.rect().height() >= 23.5,
            "{p:?}"
        );
    }
    let aug = piece_at(&h, "Aug 2026");
    h.hover_at(aug.center());
    h.run();
    assert!(
        h.tooltip()
            .is_some_and(|t| t.starts_with("Net worth — ") && t.ends_with(", Aug 2026")),
        "{:?}",
        h.tooltip()
    );
    h.click_at(aug.center());
    h.run();
    assert_eq!(h.state().view(), &View::Reports);
    assert_eq!(value(&h, "To date"), "2026-08-31");
}
