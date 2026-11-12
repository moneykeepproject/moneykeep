//! app-desktop.md revision 22: dates in report and chart headings (acceptance tests
//! 127–128).
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::UserConfig;
use serde_json::json;

type H = mk_views::testing::Harness;

/// A book on 2026-10-22 with a checking account, some income and the given format.
fn book(format: &str) -> (H, TestBook) {
    let cfg = UserConfig {
        date_format: format.into(),
        ..UserConfig::default()
    };
    let (mut h, b) = harness_with_chart_config(cfg);
    h.state_mut().set_today("2026-10-22");
    for (date, amount) in [("2026-09-05", "1500.00"), ("2026-10-05", "2500.00")] {
        cmd(
            &mut h,
            "txn.create",
            json!({"date": date, "payee": "Pay", "splits": [
                {"account": "Assets:Checking", "value": amount},
                {"account": "Salary", "value": format!("-{amount}")}]}),
        );
    }
    h.run();
    (h, b)
}

fn preset(h: &mut H, name: &str) {
    click(h, "Period");
    click(h, name);
    h.run();
}

fn rects(h: &H, label: &str) -> Vec<mk_views::testing::Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).collect()
}

#[test]
fn app_desktop_at127_single_date_heading_uses_the_date_format() {
    let (mut h, _b) = book("MM-DD-YYYY");
    click(&mut h, "Reports");
    click(&mut h, "Net worth");
    preset(&mut h, "This month");
    let day = rects(&h, "10-22");
    assert!(!day.is_empty(), "{:?}", labels_containing(&h, "10"));
    assert!(rects(&h, "Oct 22").is_empty(), "the old heading is gone");
    // The year is in the line above the day and month.
    let years = rects(&h, "2026");
    assert!(
        years
            .iter()
            .any(|y| day.iter().any(|d| y.bottom() <= d.top() + 0.5)),
        "2026 above 10-22: {years:?} / {day:?}"
    );

    // Day-first with a month name: "22 Oct".
    let (mut h, _b) = book("DD Mon YYYY");
    click(&mut h, "Reports");
    click(&mut h, "Net worth");
    preset(&mut h, "This month");
    assert!(
        !rects(&h, "22 Oct").is_empty(),
        "{:?}",
        labels_containing(&h, "Oct")
    );
    // A monthly report still heads its months "Oct".
    click(&mut h, "Income & expenses");
    preset(&mut h, "This year");
    for m in ["Sep", "Oct", "2026"] {
        assert!(
            !rects(&h, m).is_empty(),
            "{m}: {:?}",
            labels_containing(&h, m)
        );
    }
}

#[test]
fn app_desktop_at128_date_range_subtitle_uses_the_full_format() {
    let (mut h, _b) = book("DD/MM/YYYY");
    click(&mut h, "Reports");
    click(&mut h, "Income & expenses");
    preset(&mut h, "This month");
    assert!(has(&h, "01/10/2026 – 31/10/2026"));
    preset(&mut h, "Custom…");
    for (field, text) in [("From date", "01/10/2026"), ("To date", "22/10/2026")] {
        h.get_by_label(field).focus();
        h.run();
        h.key_press_modifiers(
            mk_views::testing::Modifiers::COMMAND,
            mk_views::testing::Key::A,
        );
        h.run();
        h.get_by_label(field).type_text(text);
        h.run();
    }
    h.state_mut().blur_for_tests();
    h.run();
    assert!(
        has(&h, "01/10/2026 – 22/10/2026"),
        "{:?}",
        labels_containing(&h, "2026")
    );
}
