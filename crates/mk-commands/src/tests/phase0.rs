//! Phase 0 registry behaviour, kept passing.

use super::*;

#[test]
fn command_names_are_unique() {
    let r = Registry::with_builtins();
    let mut names: Vec<_> = r.specs().iter().map(|s| s.name).collect();
    let len = names.len();
    names.dedup();
    assert_eq!(names.len(), len);
}

#[test]
fn unknown_command_is_an_error() {
    let mut b = Book::new();
    assert_eq!(b.err("nope", json!({})).code(), "unknown_command");
}

#[test]
fn book_info_reads_without_audit() {
    let mut b = Book::new();
    let info = b.run("book.info", json!({}));
    assert_eq!(info["name"], "Test");
    assert_eq!(b.audit_len(), 0);
}

#[test]
fn reads_cannot_write() {
    // Non-mutating commands run in a rolled-back read transaction.
    let mut b = Book::new();
    b.run("account.list", json!({}));
    assert_eq!(b.audit_len(), 0);
}

#[test]
fn enable_module_is_audited() {
    let mut b = Book::new();
    b.run("module.enable", json!({"module": "estimates"}));
    let log = b.s.audit_entries(10).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].command, "module.enable");
    assert_eq!(log[0].actor, "test");
}

#[test]
fn enabling_a_module_enables_its_requirements() {
    let mut b = Book::new();
    let out = b.run("module.enable", json!({"module": "progress-invoicing"}));
    assert_eq!(
        out["enabled"],
        json!(["invoicing", "estimates", "progress-invoicing"])
    );
    assert_eq!(
        b.s.get_setting("estimates", ENABLED_KEY).unwrap(),
        Some(json!(true))
    );
}

#[test]
fn cannot_disable_a_module_another_enabled_module_requires() {
    let mut b = Book::new();
    b.run("module.enable", json!({"module": "progress-invoicing"}));
    let err = b.err("module.disable", json!({"module": "estimates"}));
    assert_eq!(err.code(), "rejected");
    assert_eq!(b.audit_len(), 1);
}

#[test]
fn settings_require_an_enabled_module() {
    let mut b = Book::new();
    let args = json!({"module": "estimates", "key": "valid_days", "value": 30});
    assert_eq!(b.err("setting.set", args.clone()).code(), "rejected");
    b.run("module.enable", json!({"module": "estimates"}));
    b.run("setting.set", args);
    let got = b.run(
        "setting.get",
        json!({"module": "estimates", "key": "valid_days"}),
    );
    assert_eq!(got["value"], json!(30));
}

#[test]
fn disabling_hides_but_keeps_settings() {
    let mut b = Book::new();
    b.run("module.enable", json!({"module": "estimates"}));
    b.run(
        "setting.set",
        json!({"module": "estimates", "key": "valid_days", "value": 45}),
    );
    b.run("module.disable", json!({"module": "estimates"}));
    assert_eq!(
        b.s.get_setting("estimates", "valid_days").unwrap(),
        Some(json!(45))
    );
}

#[test]
fn bad_args_are_rejected() {
    let mut b = Book::new();
    let err = b.err("module.enable", json!({"modul": "estimates"}));
    assert_eq!(err.code(), "invalid_args");
}

/// mcp-server.md acceptance test 4 (registry half): every command has an argument
/// schema, and every schema is valid JSON Schema of type object.
#[test]
fn every_command_has_a_valid_args_schema() {
    let r = Registry::with_builtins();
    assert!(r.specs().len() >= 30);
    for spec in r.specs() {
        let schema = spec.args_schema();
        assert_eq!(schema["type"], "object", "{}: {schema}", spec.name);
        assert!(
            jsonschema::meta::is_valid(&schema),
            "{}: invalid schema {schema}",
            spec.name
        );
        assert!(!spec.summary.is_empty());
    }
}

/// Schemas are generated from the argument types: a valid call validates, and an
/// unknown field is refused by both the schema and the parser.
#[test]
fn schemas_match_parsers() {
    let r = Registry::with_builtins();
    let schema = r.spec("txn.create").unwrap().args_schema();
    let ok = json!({"date": "2026-07-01", "splits": [
        {"account": "A", "value": "1.00"}, {"account": "B", "value": "-1.00", "quantity": null}
    ]});
    assert!(jsonschema::is_valid(&schema, &ok), "{schema}");
    let bad = json!({"date": "2026-07-01", "splits": [], "bogus": 1});
    assert!(!jsonschema::is_valid(&schema, &bad));
    let number_amount = json!({"date": "2026-07-01", "splits": [{"account": "A", "value": 1.5}]});
    assert!(
        !jsonschema::is_valid(&schema, &number_amount),
        "amounts are strings"
    );
}
