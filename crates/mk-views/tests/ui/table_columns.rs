//! Fields in fixed-width table columns stay inside their column (invoice lines:
//! Qty, Rate and Amount drew 160 px wide over the next columns).
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
use crate::common::*;
use mk_views::testing::Role;
use serde_json::json;

#[test]
fn table_fields_stay_in_their_columns() {
    let (mut h, _b) = harness_with_chart();
    for m in ["invoicing", "items"] {
        cmd(&mut h, "module.enable", json!({"module": m}));
    }
    cmd(
        &mut h,
        "account.create",
        json!({"name": "Consulting Income", "type": "income"}),
    );
    cmd(
        &mut h,
        "item.create",
        json!({"name": "Consulting", "kind": "service", "price": "125.00", "income_account": "Consulting Income"}),
    );
    cmd(
        &mut h,
        "party.create",
        json!({"name": "Acme LLC", "terms": {"type": "net", "days": 30}}),
    );
    h.run();
    open_feature(&mut h, "Sales");
    h.get_by_role_and_label(Role::Tab, "Invoices").click();
    h.run();
    click(&mut h, "New invoice");
    click(&mut h, "Line 1 item");
    h.get_by_role_and_label(Role::Button, "Consulting").click();
    h.run();
    type_into(&mut h, "Line 1 quantity", "10");
    h.run();
    let nodes = h.nodes();
    let find = |tag: &str, label: &str| {
        nodes
            .iter()
            .find(|n| n.tag() == tag && n.label().as_deref() == Some(label))
            .map(|n| n.rect())
            .unwrap_or_else(|| panic!("{tag} {label}"))
    };
    for (head, field) in [
        ("Qty", "Line 1 quantity"),
        ("Rate", "Line 1 rate"),
        ("Amount", "Line 1 amount"),
        ("Description", "Line 1 description"),
    ] {
        let col = nodes
            .iter()
            .find(|n| n.label().as_deref() == Some(head) && n.rect().height() > 0.0)
            .map(|n| n.rect())
            .unwrap_or_else(|| panic!("header {head}"));
        let f = find("input", field);
        assert!(
            f.left() >= col.left() - 0.5 && f.right() <= col.right() + 0.5,
            "{field} {f:?} outside its column {head} {col:?}"
        );
    }
}
