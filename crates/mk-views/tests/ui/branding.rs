//! branding.md acceptance test 2: the start screen's name and tag line.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::ctl::App;
use mk_ui::{RecentBook, UserConfig};

fn start_app(recent: Vec<RecentBook>) -> App {
    App::new(
        Box::new(LocalBackend {
            session: None,
            name: None,
        }),
        UserConfig {
            recent,
            ..UserConfig::default()
        },
    )
}

/// AT2: the start screen shows "MoneyKeep" and "Apps for Financial Management".
#[test]
fn branding_at2_start_screen_name_and_tag_line() {
    let mut h = harness(start_app(Vec::new()));
    h.run();
    assert!(has(&h, "MoneyKeep"));
    assert!(has(&h, "Apps for Financial Management"));
}
