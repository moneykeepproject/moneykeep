//! app-desktop.md revision 12 acceptance tests (83–86): definitions screens, and the
//! "cleaned" marker on import rows.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::common::*;
use mk_ui::AppEvent;
use mk_views::testing::Role;
use serde_json::json;

type H = mk_views::testing::Harness;

const BANNER: &str = "Definition files contain import layouts, rules, categories and securities. They cannot run programs or change your transactions. Review what will be added before installing.";

fn starter() -> Vec<u8> {
    fixture("definitions/starter.mkdef")
}

fn open_definitions(h: &mut H) {
    h.state_mut().open_view_spec("settings");
    h.run();
    h.get_by_role_and_label(Role::Tab, "Definitions").click();
    h.run();
}

fn text_input(h: &mut H, label: &str, text: &str) {
    h.get_by_role_and_label(Role::TextInput, label).focus();
    h.run();
    h.get_by_role_and_label(Role::TextInput, label)
        .type_text(text);
    h.run();
}

#[test]
fn app_desktop_at83_drop_install_listed_undo() {
    let (mut h, _b) = harness_with_chart();
    h.state_mut().import_file("starter.mkdef", &starter());
    h.run();
    assert!(has(&h, BANNER));
    assert!(has(&h, "Install Starter kit 1.0.0"));
    assert!(has(&h, "Adds 1 broker profile, 2 categories and 1 rule"));
    for name in [
        "Coffee shops",
        "Expenses:Food",
        "Expenses:Food:Coffee",
        "My broker JSON",
    ] {
        assert!(
            h.query_by_role_and_label(Role::CheckBox, name).is_some(),
            "{name}"
        );
    }
    assert_eq!(h.query_all_by_label("Add").count(), 4);
    click(&mut h, "Install selected");
    assert!(
        has_contains(&h, "Installed Starter kit: 4 items"),
        "{:?} {:?}",
        labels_containing(&h, "nstall"),
        labels_containing(&h, ":")
    );
    open_definitions(&mut h);
    assert!(
        has(&h, "Starter kit 1.0.0"),
        "{:?}",
        labels_containing(&h, "Starter")
    );
    h.get_by_role_and_label(Role::Button, "Undo").click();
    h.run();
    assert!(has(&h, "No definition packages are installed."));
    assert_eq!(cmd(&mut h, "definition.list", json!({})), json!([]));
}

#[test]
fn app_desktop_at84_error_with_key_path_disables_install() {
    let (mut h, _b) = harness_with_chart();
    let bad = String::from_utf8(starter())
        .unwrap()
        .replace("https://example.org/mkdef", "javascript:alert(1)");
    h.state_mut().import_file("bad.mkdef", bad.as_bytes());
    h.run();
    assert!(
        has_contains(&h, "homepage: must be an https:// address"),
        "{:?}",
        labels_containing(&h, "homepage")
    );
    assert!(disabled(&h, "Install selected"));
}

#[test]
fn app_desktop_at85_homepage_is_not_a_link() {
    let (mut h, _b) = harness_with_chart();
    h.state_mut().import_file("starter.mkdef", &starter());
    h.run();
    assert!(has(&h, "https://example.org/mkdef"));
    assert_eq!(h.query_all_by_role(Role::Link).count(), 0);
}

#[test]
fn app_desktop_at86_create_file_reinstalls_as_unchanged() {
    let (mut h, _b) = harness_with_chart();
    for (name, value) in [("Market", "market"), ("Bakery", "bakery")] {
        cmd(
            &mut h,
            "rule.create",
            json!({"name": name, "conditions": [{"field": "payee", "op": "contains", "value": value}],
                   "actions": [{"action": "set_account", "account": "Expenses:Groceries"}]}),
        );
    }
    open_definitions(&mut h);
    click(&mut h, "Create definition file…");
    text_input(&mut h, "Id", "org.example.mine");
    text_input(&mut h, "Name", "My rules");
    for label in ["Market", "Bakery", "Expenses"] {
        h.get_by_role_and_label(Role::CheckBox, label).click();
        h.run();
    }
    click(&mut h, "Validate");
    assert!(
        has(&h, "Valid: 4 items"),
        "{:?}",
        labels_containing(&h, "alid")
    );
    events(&mut h);
    click(&mut h, "Save as…");
    let saved = events(&mut h)
        .into_iter()
        .find_map(|e| match e {
            AppEvent::SaveFile { file_name, content } => Some((file_name, content)),
            _ => None,
        })
        .expect("a save dialog");
    assert_eq!(saved.0, "org.example.mine-1.0.0.mkdef");
    h.state_mut().import_file(&saved.0, saved.1.as_bytes());
    h.run();
    assert!(has(&h, BANNER));
    let unchanged = h.query_all_by_label("Unchanged").count();
    assert!(unchanged >= 3, "{unchanged}");
    for other in ["Add", "Update", "Conflict"] {
        assert_eq!(h.query_all_by_label(other).count(), 0, "{other}");
    }
}

#[test]
fn app_desktop_r12_cleaned_marker_on_import_rows() {
    let (mut h, _b) = harness_with_chart();
    cmd(
        &mut h,
        "csv.profile_save",
        json!({"profile": {"name": "Plain", "columns": {"date": "Date", "payee": "Description", "amount": "Amount"},
               "date_format": "YYYY-MM-DD", "account": "Assets:Checking"}}),
    );
    let file = "Date,Description,Amount\n2026-10-01,Coffee \u{202E}poT,-3.00\n";
    h.state_mut().import_file("bank.csv", file.as_bytes());
    h.run();
    assert!(
        has_contains(&h, "Coffee poT: New (cleaned)"),
        "{:?}",
        labels_containing(&h, "Row 1")
    );
    assert!(has(&h, "cleaned"));
}
