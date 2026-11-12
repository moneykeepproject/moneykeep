//! Lists the business screens share (customers, vendors, items, tax rates,
//! accounts, classes, locations, funds), loaded once per book generation.

use serde_json::{Value, json};

use super::App;
use super::util::{Json, array};

#[derive(Debug, Default, Clone)]
#[allow(dead_code)] // The business screens (next step) read the rest.
pub(crate) struct Lists {
    generation: u64,
    pub modules: Vec<String>,
    pub customers: Vec<Value>,
    pub items: Vec<Value>,
    pub rates: Vec<Value>,
    pub accounts: Vec<Value>,
    pub vendors: Vec<Value>,
    pub classes: Vec<Value>,
    pub locations: Vec<Value>,
    pub funds: Vec<Value>,
    /// Projects (projects.md); `active` is false for completed and cancelled ones.
    pub projects: Vec<Value>,
    pub class_required: bool,
    pub location_required: bool,
    pub project_required: bool,
}

impl App {
    /// The shared lists, refreshed when the book changed.
    pub(crate) fn lists(&mut self) -> &Lists {
        if self.lists.generation != self.generation {
            self.refresh_lists();
        }
        &self.lists
    }

    fn refresh_lists(&mut self) {
        let list =
            |app: &mut App, cmd: &str, args: Value| array(app.backend.invoke(cmd, args).ok());
        let modules: Vec<String> = list(self, "module.list", json!({}))
            .iter()
            .filter(|m| m.at("enabled").as_bool() == Some(true))
            .map(|m| m.str("name").to_owned())
            .collect();
        let on = |m: &str| modules.iter().any(|x| x == m);
        let customers = if on("invoicing") {
            list(self, "party.list", json!({"role": "customer"}))
        } else {
            Vec::new()
        };
        let items = if on("items") {
            list(self, "item.list", json!({}))
        } else {
            Vec::new()
        };
        let rates = if on("sales-tax") {
            list(self, "tax_rate.list", json!({}))
        } else {
            Vec::new()
        };
        let vendors = if on("bills") {
            list(self, "party.list", json!({"role": "vendor"}))
        } else {
            Vec::new()
        };
        let accounts = list(self, "account.list", json!({}));
        let classes = if on("classes") {
            list(self, "class.list", json!({}))
        } else {
            Vec::new()
        };
        let locations = if on("locations") {
            list(self, "location.list", json!({}))
        } else {
            Vec::new()
        };
        let funds = if on("funds") {
            list(self, "fund.list", json!({}))
        } else {
            Vec::new()
        };
        let projects = if on("projects") {
            self.backend
                .invoke("project.list", json!({}))
                .ok()
                .map(|v| v.list("projects").to_vec())
                .unwrap_or_default()
                .into_iter()
                .map(|mut p| {
                    let open = p.at("open").as_bool() != Some(false);
                    if let Some(o) = p.as_object_mut() {
                        o.insert("active".into(), json!(open));
                    }
                    p
                })
                .collect()
        } else {
            Vec::new()
        };
        let required = |app: &mut App, module: &str, key: &str| -> bool {
            on(module)
                && app
                    .backend
                    .invoke("module.settings", json!({"module": module}))
                    .ok()
                    .is_some_and(|s| {
                        s.list("settings")
                            .iter()
                            .any(|x| x.str("key") == key && x.at("value").as_bool() == Some(true))
                    })
        };
        let class_required = required(self, "classes", "classes.required");
        let location_required = required(self, "locations", "locations.required");
        let project_required = required(self, "projects", "projects.required");
        self.lists = Lists {
            generation: self.generation,
            modules,
            customers,
            items,
            rates,
            accounts,
            vendors,
            classes,
            locations,
            funds,
            projects,
            class_required,
            location_required,
            project_required,
        };
    }
}

/// Names of a list's entries (active ones only when asked).
pub(crate) fn names(list: &[Value], active_only: bool) -> Vec<String> {
    list.iter()
        .filter(|x| !active_only || x.at("active").as_bool() != Some(false))
        .map(|x| x.str("name").to_owned())
        .collect()
}

/// Full names of the non-placeholder accounts matching `pred`.
pub(crate) fn accounts_where(lists: &Lists, pred: impl Fn(&Value) -> bool) -> Vec<String> {
    lists
        .accounts
        .iter()
        .filter(|a| a.at("placeholder").as_bool() != Some(true) && pred(a))
        .map(|a| a.str("full_name").to_owned())
        .collect()
}
