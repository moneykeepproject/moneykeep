//! The account groups of the rail and the accounts view.

use serde_json::Value;

use crate::ctl::util::Json;

/// Sidebar groups, in display order.
pub(crate) const GROUPS: [&str; 7] = [
    "Cash & bank",
    "Credit cards",
    "Investments",
    "Other assets",
    "Loans & other debts",
    "Equity",
    "Categories",
];

/// Which sidebar group an account belongs to.
pub(crate) fn group_of(account: &Value) -> usize {
    match (account.str("type"), account.str("subtype")) {
        (_, "bank" | "cash") => 0,
        (_, "credit-card") => 1,
        (_, "brokerage" | "security") => 2,
        ("asset", _) => 3,
        ("liability", _) => 4,
        ("equity", _) => 5,
        _ => 6,
    }
}

/// The amount in `commodity` from a commodity map, else zero in that commodity.
pub(crate) fn first_amount(map: &Value, commodity: &str) -> String {
    map.get(commodity)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            map.as_object()
                .and_then(|o| o.values().next())
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "0.00".into())
}

/// An amount at the scale of its currency ("0" becomes "0.00" for USD).
pub(crate) fn at_scale(amount: &str, commodity: &str) -> String {
    match amount.parse::<mk_core::Decimal>() {
        Ok(v) => {
            mk_core::currency::format_amount(v, Some(mk_core::currency::minor_units(commodity)))
        }
        Err(_) => amount.to_owned(),
    }
}

/// Non-placeholder accounts matching `pred`, plus placeholder headings that have a
/// matching descendant (in tree order).
pub(crate) fn members_where(list: &[Value], pred: impl Fn(&Value) -> bool) -> Vec<&Value> {
    let in_group: Vec<&Value> = list
        .iter()
        .filter(|a| a.at("placeholder").as_bool() != Some(true) && pred(a))
        .collect();
    let is_ancestor = |p: &Value, a: &Value| {
        a.str("full_name")
            .starts_with(&format!("{}:", p.str("full_name")))
    };
    list.iter()
        .filter(|a| {
            if a.at("placeholder").as_bool() == Some(true) {
                in_group.iter().any(|m| is_ancestor(a, m))
            } else {
                pred(a)
            }
        })
        .collect()
}
