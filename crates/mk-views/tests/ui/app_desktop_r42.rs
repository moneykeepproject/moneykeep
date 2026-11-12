//! app-desktop.md revision 42: the status message is a pop-up at the window's
//! bottom-right corner that floats over the page, so nothing moves (tests 232–235;
//! the screenshots of test 236 are the `r42_screens` example).
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_arithmetic
)]

use crate::common::*;
use mk_ui::{MessagePrefs, UserConfig};
use mk_views::testing::{Key, Rect, Role};

type H = mk_views::testing::Harness;

/// `Theme.s3`: the pop-up's distance from the window's right and bottom edges.
const S3: f32 = 12.0;

fn shown(h: &H) -> Option<String> {
    h.state().message_for_tests().map(|(t, _)| t)
}

/// The pop-up's bounds, if one is shown.
fn popup(h: &H) -> Option<Rect> {
    h.query_all_by(|n| n.tag() == "message")
        .next()
        .map(|n| n.rect())
        .filter(|r| r.width() > 0.0 && r.height() > 0.0)
}

/// The bounds of the top bar, the rail and the page.
fn regions(h: &H) -> Vec<(String, Rect)> {
    let mut v: Vec<(String, Rect)> = h
        .query_all_by(|n| n.tag() == "region")
        .map(|n| (n.item_id().to_owned(), n.rect()))
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn notify(h: &mut H, text: &str, error: bool) {
    h.state_mut().notify_for_tests(text, error);
    h.views().refresh();
    h.run();
    assert_eq!(shown(h).as_deref(), Some(text));
}

/// The pop-up's × is in reach (not behind a modal dialog).
fn reachable_dismiss(h: &H) -> bool {
    h.nodes()
        .iter()
        .any(|n| n.item_id().starts_with("msg/") && n.label().as_deref() == Some("Dismiss"))
}

fn close_enough(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1.0
}

/// Test 232: Showing and closing an informational message leaves the top-left
/// position and size of the rail, top bar and page unchanged.
#[test]
fn t232_message_moves_nothing() {
    let (mut h, _b) = harness_with_chart();
    open_register(&mut h, "Assets:Checking");
    h.run();
    let before = regions(&h);
    let names: Vec<&str> = before.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["page", "rail", "top"], "{before:?}");
    assert!(popup(&h).is_none());
    notify(&mut h, "Saved.", false);
    assert!(popup(&h).is_some(), "the pop-up shows");
    assert_eq!(regions(&h), before, "during");
    // It closes by itself (3 s) and fades out.
    h.advance(4.0);
    h.run();
    assert_eq!(shown(&h), None);
    assert!(popup(&h).is_none(), "gone");
    assert!(!has(&h, "Dismiss"));
    assert_eq!(regions(&h), before, "after");
}

/// Test 233: The pop-up's right and bottom edges are `Theme.s3` from the window's; a
/// long message makes it 420 px wide with the text wrapped; at the minimum window
/// size it fits inside the window.
#[test]
fn t233_corner_width_and_wrapping() {
    let (mut h, _b) = harness_with_chart();
    notify(&mut h, "Saved.", false);
    let size = h.size();
    let p = popup(&h).unwrap();
    assert!(close_enough(p.right(), size.x - S3), "{p:?} in {size:?}");
    assert!(close_enough(p.bottom(), size.y - S3), "{p:?} in {size:?}");
    assert!(
        p.width() < 420.0 - 1.0,
        "a short message is narrower: {p:?}"
    );
    let short_h = p.height();
    // The × button, named Dismiss, is inside it.
    let x = h.get_by_role_and_label(Role::Button, "Dismiss").rect();
    assert!(p.contains_rect(x), "{x:?} in {p:?}");

    let long = "Imported 42 transactions from the statement file. 3 of them look like \
                transactions you already have and wait in Import review; the rest were \
                added to Assets:Checking.";
    notify(&mut h, long, false);
    let p = popup(&h).unwrap();
    assert!(close_enough(p.width(), 420.0), "{p:?}");
    assert!(close_enough(p.right(), size.x - S3), "{p:?}");
    assert!(close_enough(p.bottom(), size.y - S3), "{p:?}");
    let text = h.get_by_label(long).rect();
    assert!(p.contains_rect(text), "{text:?} in {p:?}");
    assert!(
        text.height() > 30.0 && p.height() > short_h + 10.0,
        "the text wraps: {text:?}, {p:?}"
    );

    // The minimum window size (800 × 560).
    h.set_size(mk_views::testing::vec2(800.0, 560.0));
    h.run();
    let p = popup(&h).unwrap();
    let w = h.window_rect();
    assert!(w.contains_rect(p), "{p:?} in {w:?}");
    assert!(close_enough(p.right(), 800.0 - S3), "{p:?}");
    assert!(close_enough(p.bottom(), 560.0 - S3), "{p:?}");
    assert!(p.width() <= 800.0 - 2.0 * S3);
}

/// Test 234: Revision 37's behaviour with the pop-up: an informational message
/// closes after 3 s, hovering holds it, an error stays after 10 s, × and Esc
/// close it.
#[test]
fn t234_closing_with_the_popup() {
    let (mut h, _b) = harness_with_chart_config(UserConfig {
        ui: MessagePrefs::default(),
        ..iso_config()
    });
    notify(&mut h, "Saved.", false);
    h.advance(2.5);
    assert_eq!(shown(&h).as_deref(), Some("Saved."), "at 2.5 s");
    h.advance(0.75);
    assert_eq!(shown(&h), None, "closed after 3 s");

    // The pointer over the pop-up holds it; it closes 3 s after it leaves.
    notify(&mut h, "Imported 4 transactions.", false);
    h.advance(2.0);
    let p = popup(&h).unwrap();
    h.hover_at(p.center());
    h.advance(5.0);
    assert!(shown(&h).is_some(), "the pointer holds it");
    let size = h.size();
    h.hover_at(mk_views::testing::pos2(size.x / 2.0, size.y / 2.0));
    h.advance(2.5);
    assert!(shown(&h).is_some(), "2.5 s after the pointer left");
    h.advance(0.75);
    assert_eq!(shown(&h), None);

    // Errors stay; × closes them.
    notify(&mut h, "That did not work.", true);
    h.advance(10.0);
    assert_eq!(shown(&h).as_deref(), Some("That did not work."));
    let x = h.get_by_role_and_label(Role::Button, "Dismiss");
    assert_eq!(x.description().as_deref().unwrap_or("Dismiss"), "Dismiss");
    x.click();
    h.run();
    assert_eq!(shown(&h), None, "× closes it");
    assert!(popup(&h).is_none());

    // Esc while focus is in the pop-up closes it.
    notify(&mut h, "That did not work either.", true);
    h.key_press(Key::F6);
    h.run();
    let focused = h.state().focused_item().map(str::to_owned);
    assert!(
        focused.as_deref().is_some_and(|f| f.starts_with("msg/")),
        "F6 moves focus into the pop-up: {focused:?}"
    );
    h.key_press(Key::Escape);
    h.run();
    assert_eq!(shown(&h), None, "Esc closes it");
    assert!(
        !h.state()
            .focused_item()
            .is_some_and(|f| f.starts_with("msg/")),
        "focus left the closed pop-up"
    );
}

/// Revision 42, keyboard: a message never takes focus from a field being typed
/// in; F6 goes into the pop-up and back to the field; Esc outside the pop-up
/// leaves it alone.
#[test]
fn r42_focus_stays_and_f6() {
    let (mut h, _b) = harness_with_chart_config(UserConfig {
        ui: MessagePrefs {
            message_autoclose: false,
            message_seconds: 3,
        },
        ..iso_config()
    });
    open_register(&mut h, "Assets:Checking");
    click(&mut h, "Reconcile…");
    let field = "Statement balance";
    h.get_by_role_and_label(Role::TextInput, field).focus();
    h.run();
    assert!(h.get_by_role_and_label(Role::TextInput, field).is_focused());
    notify(&mut h, "Saved.", false);
    assert!(
        h.get_by_role_and_label(Role::TextInput, field).is_focused(),
        "the pop-up did not take focus"
    );
    h.key_press(Key::F6);
    h.run();
    assert!(
        h.state()
            .focused_item()
            .is_some_and(|f| f.starts_with("msg/")),
        "F6 into the pop-up"
    );
    h.key_press(Key::F6);
    h.run();
    assert!(
        h.get_by_role_and_label(Role::TextInput, field).is_focused(),
        "F6 back to the field"
    );
    assert_eq!(shown(&h).as_deref(), Some("Saved."));
}

/// Test 235: Details grow the pop-up upwards with its bottom edge fixed, capped at
/// half the window height (scrolling beyond).
#[test]
fn t235_details_grow_upwards() {
    let (mut h, _b) = harness_with_chart();
    h.state_mut().notify_full_for_tests(
        "Couldn't open the book.",
        true,
        Some("The file is locked by another program."),
        false,
    );
    h.views().refresh();
    h.run();
    let closed = popup(&h).unwrap();
    click(&mut h, "Details");
    h.run();
    assert!(has(&h, "The file is locked by another program."));
    let open = popup(&h).unwrap();
    assert!(
        close_enough(open.bottom(), closed.bottom()),
        "{closed:?} {open:?}"
    );
    assert!(
        close_enough(open.right(), closed.right()),
        "{closed:?} {open:?}"
    );
    assert!(
        open.top() < closed.top() - 10.0,
        "it grew upwards: {closed:?} {open:?}"
    );
    click(&mut h, "Hide details");
    h.run();
    let again = popup(&h).unwrap();
    assert!(
        close_enough(again.height(), closed.height()),
        "{closed:?} {again:?}"
    );

    // Long details: at most half the window tall, with a scroll bar.
    let long: String = (1..=60)
        .map(|i| format!("Line {i} of the error report from the storage layer."))
        .collect::<Vec<_>>()
        .join("\n");
    h.state_mut()
        .notify_full_for_tests("Couldn't save.", true, Some(&long), false);
    h.views().refresh();
    h.run();
    let closed = popup(&h).unwrap();
    click(&mut h, "Details");
    h.run();
    let open = popup(&h).unwrap();
    let size = h.size();
    assert!(
        close_enough(open.height(), size.y / 2.0),
        "{open:?} in {size:?}"
    );
    assert!(
        close_enough(open.bottom(), closed.bottom()),
        "{closed:?} {open:?}"
    );
    let bar = h
        .query_all_by(|n| n.tag() == "scrollbar" && n.item_id() == "message")
        .next()
        .map(|n| n.rect());
    assert!(
        bar.is_some_and(|b| open.contains_rect(b) && b.height() > 0.0),
        "a scroll bar: {bar:?}"
    );
}

/// Revision 42: a message raised while a dialog is open shows above that
/// dialog's scrim and can be dismissed; one shown before stays under it.
#[test]
fn r42_message_over_a_dialog() {
    let (mut h, _b) = harness_with_chart_config(UserConfig {
        ui: MessagePrefs {
            message_autoclose: false,
            message_seconds: 3,
        },
        ..iso_config()
    });
    notify(&mut h, "Before the dialog.", false);
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Advanced").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::Button, "Reset MoneyKeep settings…")
        .click();
    h.run_steps(3);
    assert!(!h.views().window().get_message_over_dialog());
    assert!(
        !reachable_dismiss(&h),
        "under the dialog's scrim, out of reach"
    );
    notify(&mut h, "Raised by the dialog.", true);
    assert!(h.views().window().get_message_over_dialog());
    assert!(reachable_dismiss(&h), "above the scrim");
    h.get_by_role_and_label(Role::Button, "Dismiss").click();
    h.run();
    assert_eq!(shown(&h), None);
    assert!(has(&h, "Type RESET to confirm"), "the dialog is still open");
}
