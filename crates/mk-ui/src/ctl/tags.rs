//! Class / Location / Fund / Project pickers (app-desktop.md revision 16,
//! projects.md) shared by the transaction pop-up, schedules and the document
//! editors. A project brings its fund (when none is chosen) and its billable default.

use serde_json::{Value, json};

use super::App;
use super::form::set;
use super::lists::names;
use super::ui::Ui;
use super::util::Json;

#[derive(Debug, Clone, Default)]
pub(crate) struct Tags {
    pub class: String,
    pub location: String,
    pub fund: String,
    pub project: String,
    pub billable: bool,
    /// The project picker's "Show closed" toggle (projects.md, release-prep.md §4):
    /// a view choice, not part of the values.
    pub show_closed: bool,
}

impl PartialEq for Tags {
    fn eq(&self, o: &Self) -> bool {
        (
            &self.class,
            &self.location,
            &self.fund,
            &self.project,
            self.billable,
        ) == (&o.class, &o.location, &o.fund, &o.project, o.billable)
    }
}

impl Eq for Tags {}

impl Tags {
    /// From a split's `tag_names` (or a record's class/location/fund names).
    pub(crate) fn from_names(v: &Value) -> Self {
        Tags {
            class: v.str("class").to_owned(),
            location: v.str("location").to_owned(),
            fund: v.str("fund").to_owned(),
            project: v.str("project").to_owned(),
            billable: v.at("billable").as_bool() == Some(true),
            show_closed: false,
        }
    }

    /// From ids, named through the lists.
    pub(crate) fn from_ids(v: &Value, app: &mut App) -> Self {
        let l = app.lists().clone();
        let one = |dim: &str, list: &[Value]| {
            let id = v.str(dim);
            list.iter()
                .find(|x| x.str("id") == id)
                .map_or_else(|| id.to_owned(), |x| x.str("name").to_owned())
        };
        Tags {
            class: one("class", &l.classes),
            location: one("location", &l.locations),
            fund: one("fund", &l.funds),
            project: one("project", &l.projects),
            billable: v.at("billable").as_bool() == Some(true),
            show_closed: false,
        }
    }

    /// Set every offered tag on `args` (empty clears).
    pub(crate) fn apply(&self, args: &mut Value, on: &TagsOn) {
        for (key, list, v) in [
            ("class", &on.classes, &self.class),
            ("location", &on.locations, &self.location),
            ("fund", &on.funds, &self.fund),
        ] {
            if list.is_some() {
                set(args, key, json!(v.trim()));
            }
        }
        if on.projects.is_some() {
            set(args, "project", json!(self.project.trim()));
            if on.billable && !self.project.trim().is_empty() {
                set(args, "billable", json!(self.billable));
            }
        }
    }

    /// Set the offered tags that have a value.
    pub(crate) fn apply_given(&self, args: &mut Value, on: &TagsOn) {
        for (key, list, v) in [
            ("class", &on.classes, &self.class),
            ("location", &on.locations, &self.location),
            ("fund", &on.funds, &self.fund),
        ] {
            if list.is_some() && !v.trim().is_empty() {
                set(args, key, json!(v.trim()));
            }
        }
        if on.projects.is_some() && !self.project.trim().is_empty() {
            set(args, "project", json!(self.project.trim()));
            if on.billable {
                set(args, "billable", json!(self.billable));
            }
        }
    }
}

/// Which pickers show (their names) and which are required.
#[derive(Debug, Clone, Default)]
pub(crate) struct TagsOn {
    pub classes: Option<Vec<String>>,
    pub locations: Option<Vec<String>>,
    pub funds: Option<Vec<String>>,
    /// Open projects (projects.md: completed and cancelled ones are not offered).
    pub projects: Option<Vec<String>>,
    /// Completed and cancelled projects: (name, status), offered with "Show closed".
    pub closed_projects: Vec<(String, String)>,
    pub class_required: bool,
    pub location_required: bool,
    pub project_required: bool,
    /// The Billable checkbox shows with a project (module invoicing on).
    pub billable: bool,
    /// Project name → (fund name, billable default), for the pre-filled values.
    pub project_defaults: Vec<(String, String, bool)>,
}

impl TagsOn {
    pub(crate) fn any(&self) -> bool {
        self.classes.is_some()
            || self.locations.is_some()
            || self.funds.is_some()
            || self.projects.is_some()
    }
}

pub(crate) fn options(app: &mut App) -> TagsOn {
    let l = app.lists().clone();
    let on = |m: &str| l.modules.iter().any(|x| x == m);
    let fund_name = |id: &str| {
        l.funds
            .iter()
            .find(|f| f.str("id") == id)
            .map(|f| f.str("name").to_owned())
            .unwrap_or_default()
    };
    TagsOn {
        classes: on("classes").then(|| names(&l.classes, true)),
        locations: on("locations").then(|| names(&l.locations, true)),
        funds: on("funds").then(|| names(&l.funds, true)),
        projects: on("projects").then(|| names(&l.projects, true)),
        closed_projects: l
            .projects
            .iter()
            .filter(|p| p.at("active").as_bool() == Some(false))
            .map(|p| (p.str("name").to_owned(), p.str("status").to_owned()))
            .collect(),
        class_required: l.class_required,
        location_required: l.location_required,
        project_required: l.project_required,
        billable: on("projects") && on("invoicing"),
        project_defaults: l
            .projects
            .iter()
            .map(|p| {
                (
                    p.str("name").to_owned(),
                    if on("funds") {
                        fund_name(p.str("fund"))
                    } else {
                        String::new()
                    },
                    p.at("billable").as_bool() == Some(true),
                )
            })
            .collect(),
    }
}

pub(crate) fn missing(on: &TagsOn, t: &Tags) -> Option<String> {
    if on.classes.is_some() && on.class_required && t.class.trim().is_empty() {
        return Some("Choose a class.".into());
    }
    if on.locations.is_some() && on.location_required && t.location.trim().is_empty() {
        return Some("Choose a location.".into());
    }
    if on.projects.is_some() && on.project_required && t.project.trim().is_empty() {
        return Some("Choose a project.".into());
    }
    None
}

/// The pickers as form rows; returns whether a value changed.
pub(crate) fn rows(ui: &mut Ui, on: &TagsOn, t: &mut Tags, prefix: &str) -> bool {
    let mut changed = false;
    for (title, list, required, v) in [
        ("Class", &on.classes, on.class_required, &mut t.class),
        (
            "Location",
            &on.locations,
            on.location_required,
            &mut t.location,
        ),
        ("Fund", &on.funds, false, &mut t.fund),
    ] {
        let Some(list) = list else {
            continue;
        };
        let caption = if required {
            format!("{title} *")
        } else {
            title.to_owned()
        };
        let name = if prefix.is_empty() {
            title.to_owned()
        } else {
            format!("{prefix} {}", title.to_lowercase())
        };
        ui.form(&caption, |ui| {
            changed |= picker(ui, &name, v, list);
            if required && v.trim().is_empty() {
                ui.error_text("Required");
            }
        });
    }
    if let Some(list) = &on.projects {
        let caption = if on.project_required {
            "Project *"
        } else {
            "Project"
        };
        let name = if prefix.is_empty() {
            "Project".to_owned()
        } else {
            format!("{prefix} project")
        };
        let mut picked = false;
        ui.form(caption, |ui| {
            picked = project_picker(ui, &name, t, list, &on.closed_projects);
            if !on.closed_projects.is_empty() {
                ui.named(if prefix.is_empty() {
                    "Show closed projects".to_owned()
                } else {
                    format!("{prefix} show closed projects")
                });
                ui.checkbox(&mut t.show_closed, "Show closed");
            }
            if on.billable && !t.project.trim().is_empty() {
                ui.named(if prefix.is_empty() {
                    "Billable".to_owned()
                } else {
                    format!("{prefix} billable")
                });
                changed |= ui.checkbox(&mut t.billable, "Billable").clicked();
            }
            if on.project_required && t.project.trim().is_empty() {
                ui.error_text("Required");
            }
        });
        if picked {
            changed = true;
            project_picked(on, t);
        }
    }
    changed
}

/// A project was chosen: its fund fills an empty Fund (the user can change it) and
/// its billable default sets the checkbox (projects.md).
pub(crate) fn project_picked(on: &TagsOn, t: &mut Tags) {
    let Some((_, fund, billable)) = on
        .project_defaults
        .iter()
        .find(|(n, _, _)| n.eq_ignore_ascii_case(t.project.trim()))
    else {
        t.billable = false;
        return;
    };
    t.billable = *billable && on.billable;
    if on.funds.is_some() && t.fund.trim().is_empty() && !fund.is_empty() {
        t.fund.clone_from(fund);
    }
}

/// One picker with a None choice.
/// The project picker: open projects, and with "Show closed" the completed and
/// cancelled ones too, marked "(completed)" or "(cancelled)".
fn project_picker(
    ui: &mut Ui,
    name: &str,
    t: &mut Tags,
    open: &[String],
    closed: &[(String, String)],
) -> bool {
    if !t.show_closed {
        return picker(ui, name, &mut t.project, open);
    }
    let mut values: Vec<String> = open.to_vec();
    let mut labels: Vec<String> = open.to_vec();
    for (n, status) in closed {
        values.push(n.clone());
        labels.push(format!("{n} ({status})"));
    }
    // Pick among the labels, then keep the project's own name.
    let mut shown = values
        .iter()
        .position(|v| v == &t.project)
        .and_then(|i| labels.get(i).cloned())
        .unwrap_or_else(|| t.project.clone());
    let changed = picker(ui, name, &mut shown, &labels);
    if changed {
        t.project = labels
            .iter()
            .position(|l| l == &shown)
            .and_then(|i| values.get(i).cloned())
            .unwrap_or_default();
    }
    changed
}

pub(crate) fn picker(ui: &mut Ui, name: &str, v: &mut String, list: &[String]) -> bool {
    super::form::pick(ui, name, v, list, Some("None"))
}
