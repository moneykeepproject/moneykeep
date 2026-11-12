//! Feature-module catalog.
//!
//! Optional features belong to a module that is toggled per book. Turning a module
//! off hides its commands; it never deletes data. Module names and descriptions are
//! MoneyKeep's own (clean-room).

use serde::Serialize;

/// The always-on module. Commands in it can never be disabled.
pub const CORE: &str = "core";

#[derive(Debug, Clone, Serialize)]
pub struct ModuleSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// Modules that must be enabled for this one to be enabled.
    pub requires: &'static [&'static str],
}

/// Phase 0 catalog. Each module gains commands and a settings schema in later phases.
pub const CATALOG: &[ModuleSpec] = &[
    m(
        "invoicing",
        "Customers, invoices and payments received",
        &[],
    ),
    m(
        "estimates",
        "Quotes to customers that can convert to invoices",
        &["invoicing"],
    ),
    m(
        "progress-invoicing",
        "Invoice an estimate in several parts",
        &["invoicing", "estimates"],
    ),
    m(
        "sales-orders",
        "Customer orders before invoicing",
        &["invoicing"],
    ),
    m(
        "purchase-orders",
        "Orders to vendors before bills",
        &["bills"],
    ),
    m(
        "bills",
        "Vendors, bills, bill payments and vendor credits (accounts payable)",
        &[],
    ),
    m(
        "recurring-invoices",
        "Invoices generated on a schedule",
        &["invoicing"],
    ),
    m("items", "Products and services lists", &[]),
    m("inventory", "Quantity on hand and average cost", &["items"]),
    m("sales-tax", "Tax codes, rates and liability tracking", &[]),
    m("classes", "Tag transactions by class for reporting", &[]),
    m(
        "locations",
        "Tag transactions by location or department",
        &[],
    ),
    m(
        "funds",
        "Nonprofit fund accounting and restricted net assets",
        &[],
    ),
    m(
        "projects",
        "Projects: gather what a piece of work cost and brought in across accounts and documents, against its budget, with the miles driven for it",
        &[],
    ),
    m("time", "Time tracking", &[]),
    m(
        "mileage",
        "Mileage: log trips by vehicle and purpose, apply standard mileage rates, and include reimbursable trips in expense reports",
        &[],
    ),
    m(
        "subscriptions",
        "Subscription audit: find repeating payments, keep their price history and catch price rises, charges after cancelling and forgotten trials",
        &[],
    ),
    m("1099", "Vendor 1099 tracking", &["bills"]),
    m(
        "multi-currency",
        "Foreign-currency accounts and exchange rates",
        &[],
    ),
    m("budgets", "Budgets by account and period", &[]),
    m("investments", "Securities, lots and cost basis", &[]),
    m(
        "statements",
        "Customer statements and late fees",
        &["invoicing"],
    ),
    m(
        "payment-processing",
        "Processor payouts and fees (Stripe, Square, PayPal)",
        &[],
    ),
    m("bank-feeds", "Live transaction feeds from providers", &[]),
    m(
        "expenses",
        "Expenses and receipts: record spending with photos of receipts and print expense reports",
        &[],
    ),
];

const fn m(
    name: &'static str,
    description: &'static str,
    requires: &'static [&'static str],
) -> ModuleSpec {
    ModuleSpec {
        name,
        description,
        requires,
    }
}

pub fn find(name: &str) -> Option<&'static ModuleSpec> {
    CATALOG.iter().find(|m| m.name == name)
}

/// Enabled modules that directly require `name`.
pub fn dependants(name: &str) -> impl Iterator<Item = &'static ModuleSpec> + '_ {
    CATALOG.iter().filter(move |m| m.requires.contains(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_names_are_unique() {
        let mut names: Vec<_> = CATALOG.iter().map(|m| m.name).collect();
        names.sort_unstable();
        let len = names.len();
        names.dedup();
        assert_eq!(names.len(), len);
    }

    #[test]
    fn every_requirement_exists_and_is_not_core() {
        for m in CATALOG {
            for r in m.requires {
                assert!(find(r).is_some(), "{} requires unknown {r}", m.name);
                assert_ne!(*r, CORE);
            }
        }
    }
}
