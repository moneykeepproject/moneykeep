//! Renders the main screens in light and dark into PNG files for review
//! (ui-slint.md revision 2, Tests). Run under a display (or Xvfb) with
//! `SLINT_BACKEND=winit-software cargo run -p mk-views --example screens -- <dir>`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::panic,
    clippy::type_complexity,
    clippy::indexing_slicing,
    clippy::float_arithmetic
)]

use std::cell::RefCell;
use std::rc::Rc;

use mk_commands::Session;
use mk_ui::ctl::App;
use mk_ui::{Backend, UiError, UserConfig};
use mk_views::Views;
use serde_json::{Value, json};
use slint::ComponentHandle;

struct Local {
    session: Option<Session>,
    name: Option<String>,
}

impl Backend for Local {
    fn invoke(&mut self, command: &str, args: Value) -> Result<Value, UiError> {
        match self.session.as_mut() {
            Some(s) => s
                .invoke(command, args)
                .map_err(|e| UiError::new(e.code(), e.to_string())),
            None => Err(UiError::new("no_book", "No book is open.")),
        }
    }
    fn book_name(&self) -> Option<String> {
        self.name.clone()
    }
    fn chart_presets(&mut self) -> Result<Value, UiError> {
        mk_commands::chart::presets_json().map_err(|e| UiError::new(e.code(), e.to_string()))
    }
    fn create_book(
        &mut self,
        path: &str,
        name: &str,
        currency: &str,
        preset: Option<&str>,
    ) -> Result<(), UiError> {
        self.session = Some(
            Session::create_with_preset(std::path::Path::new(path), name, currency, "app", preset)
                .map_err(|e| UiError::new(e.code(), e.to_string()))?,
        );
        self.name = Some(name.to_owned());
        Ok(())
    }
    fn open_book(&mut self, _path: &str) -> Result<(), UiError> {
        Err(UiError::new("io", "not here"))
    }
    fn close_book(&mut self) {
        self.session = None;
        self.name = None;
    }
    fn is_protected_file(&self, path: &str) -> Option<bool> {
        mk_commands::book_kind(std::path::Path::new(path))
            .ok()
            .map(|k| k == mk_commands::BookKind::Encrypted)
    }
}

/// A password-protected book to open (the password prompt's screens).
fn protected_book() -> String {
    let dir = std::env::temp_dir().join(format!("mk-screens-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Parish Funds.moneykeep");
    if !path.exists() {
        drop(
            Session::create_encrypted(
                &path,
                "Parish Funds",
                "USD",
                "screens",
                "correct horse battery",
                true,
                mk_commands::KdfParams::TEST,
            )
            .unwrap(),
        );
    }
    path.to_str().unwrap().to_owned()
}

/// A demo book with a small chart and some transactions.
fn demo_app(dir: &std::path::Path, theme: mk_ui::Theme, n: usize) -> App {
    let sub = dir.join(format!("{theme:?}-{n}"));
    std::fs::create_dir_all(&sub).unwrap();
    let path = sub.join("Household.moneykeep");
    let mut cfg = UserConfig {
        theme,
        date_format: "MM-DD-YYYY".into(),
        ..UserConfig::default()
    };
    cfg.recent = vec![
        mk_ui::RecentBook::from(path.to_str().unwrap()),
        mk_ui::RecentBook::from("/nowhere/Church Treasury.moneykeep"),
    ];
    let mut app = App::new(
        Box::new(Local {
            session: None,
            name: None,
        }),
        cfg,
    );
    app.create_book(path.to_str().unwrap(), "Household", "USD");
    app.set_today("2026-11-02");
    for a in [
        json!({"name": "Assets", "type": "asset", "placeholder": true}),
        json!({"name": "Checking", "type": "asset", "subtype": "bank", "parent": "Assets"}),
        json!({"name": "Savings", "type": "asset", "subtype": "bank", "parent": "Assets"}),
        json!({"name": "Expenses", "type": "expense", "placeholder": true}),
        json!({"name": "Groceries", "type": "expense", "parent": "Expenses"}),
        json!({"name": "Utilities", "type": "expense", "parent": "Expenses"}),
        json!({"name": "Liabilities", "type": "liability", "placeholder": true}),
        json!({"name": "Visa", "type": "liability", "subtype": "credit-card", "parent": "Liabilities"}),
        json!({"name": "Salary", "type": "income"}),
    ] {
        let _ = app.test_invoke("account.create", a);
    }
    let txns = [
        (
            "2026-06-01",
            "Opening balance",
            "Opening Balances",
            "-2450.00",
        ),
        ("2026-06-30", "Employer Inc", "Salary", "-3200.00"),
        ("2026-07-31", "Employer Inc", "Salary", "-3200.00"),
        ("2026-08-31", "Employer Inc", "Salary", "-3350.00"),
        ("2026-09-30", "Employer Inc", "Salary", "-3350.00"),
        ("2026-10-31", "Employer Inc", "Salary", "-3350.00"),
        ("2026-07-04", "Green Market", "Expenses:Groceries", "182.40"),
        ("2026-08-09", "Green Market", "Expenses:Groceries", "214.10"),
        ("2026-09-12", "Corner Grocer", "Expenses:Groceries", "96.75"),
        ("2026-10-03", "Green Market", "Expenses:Groceries", "143.20"),
        ("2026-10-18", "City Power", "Expenses:Utilities", "88.60"),
        ("2026-10-21", "Corner Grocer", "Expenses:Groceries", "61.35"),
        ("2026-10-28", "Water Works", "Expenses:Utilities", "42.00"),
        ("2026-11-01", "Green Market", "Expenses:Groceries", "57.80"),
    ];
    for (date, payee, other, value) in txns {
        let neg = value.starts_with('-');
        let money = if neg {
            value.trim_start_matches('-').to_owned()
        } else {
            format!("-{value}")
        };
        let made = app.test_invoke(
            "txn.create",
            json!({"date": date, "payee": payee, "splits": [
                {"account": other, "value": value}, {"account": "Assets:Checking", "value": money}]}),
        );
        // Older rows cleared or reconciled, so the register shows every status mark.
        let state = if date < "2026-09-01" {
            "r"
        } else if date < "2026-10-20" {
            "c"
        } else {
            "n"
        };
        if let Ok(v) = made
            && let Some(id) = v.get("id").and_then(|i| i.as_str())
            && state != "n"
        {
            let _ = app.test_invoke(
                "txn.set_reconcile",
                json!({"splits": [{"id": id, "split": 1, "state": state}]}),
            );
        }
    }
    app
}

/// A book created the way the Create a new book dialog does it, from `chart` (the
/// preset's name in the list).
fn preset_app(theme: mk_ui::Theme, chart: &str, tag: &str) -> App {
    let dir =
        std::env::temp_dir().join(format!("mk-screens-{}-{tag}-{theme:?}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Household.moneykeep");
    let cfg = UserConfig {
        theme,
        date_format: "MM-DD-YYYY".into(),
        ..UserConfig::default()
    };
    let mut app = App::new(
        Box::new(Local {
            session: None,
            name: None,
        }),
        cfg,
    );
    app.act("Create a New Book", mk_ui::ctl::Ev::Click, "");
    app.act(
        &format!("dialog:create-book/Chart {chart}"),
        mk_ui::ctl::Ev::Click,
        "",
    );
    app.act("dialog:create-book/Create book…", mk_ui::ctl::Ev::Click, "");
    let _ = app.take_events();
    app.create_book(path.to_str().unwrap(), "Household", "USD");
    app.set_today("2026-11-02");
    app
}

/// Home's standard layout plus an income-vs-expenses chart and a spending chart
/// with several categories (dashboards.md revision 4).
fn dashboard_charts(a: &mut App) {
    for name in ["Dining", "Fuel", "Insurance", "Phone", "Clothing", "Gifts"] {
        let _ = a.test_invoke(
            "account.create",
            json!({"name": name, "type": "expense", "parent": "Expenses"}),
        );
    }
    for (date, cat, amount) in [
        ("2026-08-14", "Dining", "64.00"),
        ("2026-09-02", "Fuel", "48.10"),
        ("2026-09-20", "Insurance", "120.00"),
        ("2026-10-05", "Phone", "55.00"),
        ("2026-10-11", "Clothing", "79.99"),
        ("2026-10-24", "Gifts", "35.00"),
        ("2026-10-26", "Dining", "42.50"),
    ] {
        let _ = a.test_invoke(
            "txn.create",
            json!({"date": date, "payee": "Shop", "splits": [
                {"account": format!("Expenses:{cat}"), "value": amount},
                {"account": "Assets:Checking", "value": format!("-{amount}")}]}),
        );
    }
    let home = a
        .test_invoke("dashboard.list", json!({}))
        .ok()
        .and_then(|v| {
            v.as_array().and_then(|l| {
                l.iter()
                    .find(|d| d.get("name").and_then(Value::as_str) == Some("Home"))
                    .and_then(|d| d.get("id").and_then(Value::as_str).map(str::to_owned))
            })
        })
        .unwrap_or_default();
    for (chart, period) in [
        ("income_expense", "last_12_months"),
        ("spending", "this_year"),
    ] {
        let _ = a.test_invoke(
            "dashboard.add_block",
            json!({"dashboard": home, "type": "chart",
                "settings": {"chart": chart, "period": period}}),
        );
    }
    a.open_view_spec("dashboard:Home");
}

fn theme_of(a: &App) -> mk_ui::Theme {
    a.config.theme
}

type Shot = (&'static str, Box<dyn Fn(&mut App)>);

fn shots() -> Vec<Shot> {
    vec![
        ("start", Box::new(|a: &mut App| a.close_book())),
        ("main", Box::new(|_a: &mut App| {})),
        ("dashboard-charts", Box::new(dashboard_charts)),
        // The same with the pointer over a bar (dashboards.md revision 5).
        ("chart-hover", Box::new(dashboard_charts)),
        (
            "settings-display",
            Box::new(|a: &mut App| {
                a.config.settings_section = "Display".into();
                a.navigate(mk_ui::View::Settings);
            }),
        ),
        (
            "settings-book",
            Box::new(|a: &mut App| {
                a.config.settings_section = "Book".into();
                a.navigate(mk_ui::View::Settings);
            }),
        ),
        (
            "accounts-pane",
            Box::new(|a: &mut App| a.act("rail/All accounts", mk_ui::ctl::Ev::Click, "")),
        ),
        (
            "register",
            Box::new(|a: &mut App| {
                a.navigate(mk_ui::View::Register {
                    account: "Assets:Checking".into(),
                });
            }),
        ),
        (
            "report",
            Box::new(|a: &mut App| {
                a.navigate(mk_ui::View::Reports);
                a.act("Income & expenses", mk_ui::ctl::Ev::Click, "");
                a.act("Period", mk_ui::ctl::Ev::Select, "5");
            }),
        ),
        (
            "popup",
            Box::new(|a: &mut App| {
                a.navigate(mk_ui::View::Register {
                    account: "Assets:Checking".into(),
                });
                a.key("", "ctrl+n");
            }),
        ),
        (
            "calculator",
            Box::new(|a: &mut App| {
                a.navigate(mk_ui::View::Register {
                    account: "Assets:Checking".into(),
                });
                a.act("top/Calculator", mk_ui::ctl::Ev::Click, "");
            }),
        ),
        (
            "password",
            Box::new(|a: &mut App| {
                a.close_book();
                a.open_book(&protected_book());
            }),
        ),
        (
            "about",
            Box::new(|a: &mut App| a.act("top/About MoneyKeep", mk_ui::ctl::Ev::Click, "")),
        ),
        // chart-presets.md (app-desktop.md revision 34).
        (
            "create-preset",
            Box::new(|a: &mut App| {
                a.close_book();
                a.act("Create a New Book", mk_ui::ctl::Ev::Click, "");
            }),
        ),
        (
            "create-preset-accounts",
            Box::new(|a: &mut App| {
                a.close_book();
                a.act("Create a New Book", mk_ui::ctl::Ev::Click, "");
                a.act(
                    "dialog:create-book/Chart Small Business",
                    mk_ui::ctl::Ev::Click,
                    "",
                );
                a.act(
                    "dialog:create-book/Show accounts of Small Business",
                    mk_ui::ctl::Ev::Click,
                    "",
                );
            }),
        ),
        (
            "add-chart",
            Box::new(|a: &mut App| {
                *a = preset_app(theme_of(a), "Personal & Household", "add");
                a.navigate(mk_ui::View::Accounts);
                a.act("Add accounts from a chart…", mk_ui::ctl::Ev::Click, "");
                a.act(
                    "dialog:add-chart/Chart Personal — Form 1040",
                    mk_ui::ctl::Ev::Click,
                    "",
                );
            }),
        ),
        (
            "tax-summary",
            Box::new(|a: &mut App| {
                *a = preset_app(theme_of(a), "Personal — Form 1040", "tax");
                for (date, debit, credit, amount) in [
                    ("2025-01-31", "Checking", "4010", "3000.00"),
                    ("2025-02-28", "Checking", "4010", "2000.00"),
                    ("2025-03-15", "Checking", "4700", "18.42"),
                    ("2025-04-01", "6320", "Checking", "1200.00"),
                    ("2025-05-01", "6825", "Checking", "400.00"),
                    ("2025-06-01", "8999", "Checking", "50.00"),
                ] {
                    let _ = a.test_invoke(
                        "txn.create",
                        json!({"date": date, "splits": [
                            {"account": debit, "value": amount},
                            {"account": credit, "value": format!("-{amount}")}]}),
                    );
                }
                a.navigate(mk_ui::View::Reports);
                a.act("Tax summary", mk_ui::ctl::Ev::Click, "");
            }),
        ),
        (
            "account-dialog",
            Box::new(|a: &mut App| {
                *a = preset_app(theme_of(a), "Personal & Household", "dlg");
                let _ = a.test_invoke(
                    "account.update",
                    json!({"account": "6010", "tax_line": "Sch E L3"}),
                );
                a.navigate(mk_ui::View::Accounts);
                let ob = a
                    .test_invoke("account.get", json!({"account": "3900"}))
                    .ok()
                    .and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_owned))
                    .unwrap_or_default();
                a.act(
                    &format!("account-row:{ob}/actions:{ob}"),
                    mk_ui::ctl::Ev::Select,
                    "1",
                );
            }),
        ),
        // ledger-accounts.md revision 3 / app-desktop.md revision 35: Account type
        // and Group in the Edit account dialog.
        (
            "acctkind-edit",
            Box::new(|a: &mut App| {
                a.navigate(mk_ui::View::Accounts);
                let id = a
                    .test_invoke("account.get", json!({"account": "Assets:Checking"}))
                    .ok()
                    .and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_owned))
                    .unwrap_or_default();
                a.act(
                    &format!("account-row:{id}/actions:{id}"),
                    mk_ui::ctl::Ev::Select,
                    "1",
                );
                a.act(
                    "dialog:edit-account/Account type",
                    mk_ui::ctl::Ev::Select,
                    "3",
                );
            }),
        ),
        // connectivity.md revision 4: the register band for staged feed rows.
        (
            "acctkind-review",
            Box::new(|a: &mut App| {
                *a = feed_app(theme_of(a));
                a.navigate(mk_ui::View::Register {
                    account: "Assets:Checking".into(),
                });
            }),
        ),
        // mileage.md (app-desktop.md revision 36).
        ("mileage-trips", Box::new(mileage_app)),
        (
            "mileage-report-dialog",
            Box::new(|a: &mut App| {
                mileage_app(a);
                a.act("trips/Mileage report…", mk_ui::ctl::Ev::Click, "");
            }),
        ),
        (
            "mileage-trip-form",
            Box::new(|a: &mut App| {
                mileage_app(a);
                let id = a
                    .test_invoke(
                        "trip.list",
                        json!({"from": "2026-10-27", "to": "2026-10-27"}),
                    )
                    .ok()
                    .and_then(|v| v[0]["id"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                a.act(
                    &format!("trips/trip:{id}/Open trip 2026-10-27 Office → Supplier"),
                    mk_ui::ctl::Ev::Click,
                    "",
                );
            }),
        ),
        (
            "mileage-settings",
            Box::new(|a: &mut App| {
                mileage_app(a);
                let _ = a.test_invoke(
                    "saved_trip.create",
                    json!({"name": "Pantry run", "from_place": "St. Mark's", "to_place": "Food pantry",
                           "miles": "8.3", "round_trip": true, "purpose_kind": "charity",
                           "purpose": "Food pantry delivery", "vehicle": "Parish van"}),
                );
                a.config.settings_section = "Mileage".into();
                a.navigate(mk_ui::View::Settings);
            }),
        ),
        // subscriptions.md: the screen, its tabs, a detail, settings, review and Home.
        ("subs-list", Box::new(subs_app)),
        (
            "subs-suggested",
            Box::new(|a: &mut App| {
                subs_app(a);
                a.act("Suggested (3)", mk_ui::ctl::Ev::Click, "");
            }),
        ),
        (
            "subs-bills",
            Box::new(|a: &mut App| {
                subs_app(a);
                a.act("Recurring bills", mk_ui::ctl::Ev::Click, "");
            }),
        ),
        (
            "subs-detail",
            Box::new(|a: &mut App| {
                subs_app(a);
                let id = sub_id(a, "Spotify");
                a.open_view_spec(&format!("subscriptions:{id}"));
            }),
        ),
        (
            "subs-bill-detail",
            Box::new(|a: &mut App| {
                subs_app(a);
                let id = sub_id(a, "City Power");
                a.open_view_spec(&format!("subscriptions:{id}"));
            }),
        ),
        (
            "subs-settings",
            Box::new(|a: &mut App| {
                subs_app(a);
                a.config.settings_section = "Subscriptions".into();
                a.navigate(mk_ui::View::Settings);
            }),
        ),
        (
            "subs-review",
            Box::new(|a: &mut App| {
                subs_app(a);
                a.act("Review…", mk_ui::ctl::Ev::Click, "");
                a.act("dialog:review/Keep", mk_ui::ctl::Ev::Click, "");
                a.act(
                    "dialog:review/Cancel subscription",
                    mk_ui::ctl::Ev::Click,
                    "",
                );
            }),
        ),
        (
            "subs-home",
            Box::new(|a: &mut App| {
                subs_app(a);
                a.navigate(mk_ui::View::Home);
            }),
        ),
        (
            "subs-block",
            Box::new(|a: &mut App| {
                subs_app(a);
                let _ = a.test_invoke(
                    "dashboard.add_block",
                    json!({"dashboard": "home", "type": "subscriptions", "x": 0, "y": 0, "w": 4, "h": 3}),
                );
                a.navigate(mk_ui::View::Home);
            }),
        ),
        // projects.md: the list, a detail (summary with budget by account), the
        // Billable tab, the invoice's Add unbilled costs dialog, settings, Home.
        ("proj-list", Box::new(proj_app)),
        (
            "proj-detail",
            Box::new(|a: &mut App| {
                proj_app(a);
                let id = proj_id(a, "Roof repair");
                a.open_view_spec(&format!("projects:{id}"));
            }),
        ),
        (
            "proj-billable",
            Box::new(|a: &mut App| {
                proj_app(a);
                let id = proj_id(a, "Smith kitchen");
                a.open_view_spec(&format!("projects:{id}"));
                click_text(a, "Billable");
            }),
        ),
        (
            "proj-invoice-add",
            Box::new(|a: &mut App| {
                proj_app(a);
                a.navigate(mk_ui::View::Sales);
                click_text(a, "New invoice");
                if let Some(id) = find_id(a, "Customer") {
                    let idx = combo_index(a, &id, "Smith Family");
                    a.act(&id, mk_ui::ctl::Ev::Select, &idx.to_string());
                }
                click_text(a, "Add unbilled costs for Smith kitchen");
            }),
        ),
        (
            "proj-settings",
            Box::new(|a: &mut App| {
                proj_app(a);
                a.config.settings_section = "Projects".into();
                a.navigate(mk_ui::View::Settings);
            }),
        ),
        (
            "proj-home",
            Box::new(|a: &mut App| {
                proj_app(a);
                let _ = a.test_invoke(
                    "dashboard.add_block",
                    json!({"dashboard": "home", "type": "projects", "x": 0, "y": 0, "w": 4, "h": 3}),
                );
                a.navigate(mk_ui::View::Home);
            }),
        ),
        (
            "proj-set",
            Box::new(|a: &mut App| {
                proj_app(a);
                let id = proj_id(a, "Roof repair");
                a.open_view_spec(&format!("projects:{id}"));
                click_text(a, "Set project on transactions…");
            }),
        ),
        // subscriptions.md revision 2: Settings with the Make a bill choice, and
        // Scheduled showing the Bill schedule.
        (
            "subs-makebill-settings",
            Box::new(|a: &mut App| {
                subs_app(a);
                let _ = a.test_invoke("module.enable", json!({"module": "bills"}));
                a.config.settings_section = "Subscriptions".into();
                a.navigate(mk_ui::View::Settings);
            }),
        ),
        (
            "subs-makebill-scheduled",
            Box::new(|a: &mut App| {
                subs_app(a);
                let _ = a.test_invoke("module.enable", json!({"module": "bills"}));
                for n in ["Netflix", "Spotify"] {
                    let id = sub_id(a, n);
                    let _ = a.test_invoke("subscription.make_bill", json!({"subscription": id}));
                }
                a.navigate(mk_ui::View::Scheduled);
            }),
        ),
        // Settings → Connections: the connection's Review row.
        (
            "acctkind-connections",
            Box::new(|a: &mut App| {
                *a = feed_app(theme_of(a));
                a.navigate(mk_ui::View::Settings);
            }),
        ),
    ]
}

/// Every item of the current frame (page toolbar, sections and dialogs), nested
/// items included.
fn frame_items(a: &mut App) -> Vec<mk_ui::ctl::Item> {
    fn walk(items: &[mk_ui::ctl::Item], out: &mut Vec<mk_ui::ctl::Item>) {
        for i in items {
            out.push(i.clone());
            walk(&i.children, out);
        }
    }
    let vm = a.vm();
    let mut out = Vec::new();
    walk(&vm.page.toolbar, &mut out);
    for s in vm
        .page
        .sections
        .iter()
        .chain(vm.page.dialogs.iter().flat_map(|d| d.sections.iter()))
    {
        for r in &s.rows {
            walk(&r.items, &mut out);
        }
    }
    out
}

/// The action id of the item whose accessible name or text is `text`.
fn find_id(a: &mut App, text: &str) -> Option<String> {
    frame_items(a)
        .into_iter()
        .find(|i| i.label == text || (i.label.is_empty() && i.text == text))
        .map(|i| i.id)
}

/// Click the item named `text` (screens only: the menus and dialogs to show).
fn click_text(a: &mut App, text: &str) {
    if let Some(id) = find_id(a, text) {
        a.act(&id, mk_ui::ctl::Ev::Click, "");
    } else {
        println!("screens: no item {text:?}");
    }
}

/// The index of `option` in the combo `id`.
fn combo_index(a: &mut App, id: &str, option: &str) -> usize {
    frame_items(a)
        .into_iter()
        .find(|i| i.id == id)
        .and_then(|i| i.options.iter().position(|o| o == option))
        .unwrap_or(0)
}

/// The id of the project named `name`.
fn proj_id(a: &mut App, name: &str) -> String {
    a.test_invoke("project.get", json!({"project": name}))
        .ok()
        .and_then(|v| v["id"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// projects.md: a church book's roof repair (budget by account, a trip, nearly
/// over budget), a building fund drive with its fund, a client kitchen job with
/// billable costs and markup, a completed job and two sub-projects; then the
/// Projects screen.
fn proj_app(a: &mut App) {
    for m in ["projects", "invoicing", "mileage", "funds"] {
        let _ = a.test_invoke("module.enable", json!({"module": m}));
    }
    for n in ["Materials", "Labour", "Hardware"] {
        let _ = a.test_invoke(
            "account.create",
            json!({"name": n, "type": "expense", "parent": "Expenses"}),
        );
    }
    let _ = a.test_invoke("account.create", json!({"name": "Gifts", "type": "income"}));
    let _ = a.test_invoke(
        "fund.create",
        json!({"name": "Building Fund", "restriction": "with_donor_restrictions"}),
    );
    let _ = a.test_invoke(
        "party.create",
        json!({"name": "Smith Family", "roles": ["customer"]}),
    );
    let _ = a.test_invoke("vehicle.create", json!({"name": "Parish van"}));
    for p in [
        json!({"name": "Roof repair", "start": "2026-05-01", "end": "2026-12-31",
               "budget_mode": "by_account", "budget_lines": [
                   {"account": "Expenses:Materials", "amount": "3000.00"},
                   {"account": "Expenses:Labour", "amount": "2000.00"}],
               "notes": "Replace the north slope and flashing before winter."}),
        json!({"name": "Building drive", "budget": "500.00", "income_budget": "25000.00",
               "fund": "Building Fund"}),
        json!({"name": "Smith kitchen", "customer": "Smith Family", "budget": "8000.00",
               "billable": true, "markup_percent": "10"}),
        json!({"name": "Hall painting", "budget": "1200.00", "status": "completed"}),
        json!({"name": "Grounds"}),
        json!({"name": "Grounds:Fence", "budget": "900.00"}),
        json!({"name": "Grounds:Lighting", "budget": "600.00", "status": "planned"}),
    ] {
        let _ = a.test_invoke("project.create", p);
    }
    let mut spend =
        |date: &str, payee: &str, amount: &str, cat: &str, project: &str, money: &str| {
            let _ = a.test_invoke(
                "txn.create",
                json!({"date": date, "payee": payee, "splits": [
                {"account": format!("Expenses:{cat}"), "value": amount, "project": project},
                {"account": money, "value": format!("-{amount}")}]}),
            );
        };
    let chk = "Assets:Checking";
    let visa = "Liabilities:Visa";
    spend(
        "2026-05-04",
        "Home Depot",
        "1200.00",
        "Materials",
        "Roof repair",
        visa,
    );
    spend(
        "2026-05-18",
        "A1 Roofing",
        "2100.00",
        "Labour",
        "Roof repair",
        chk,
    );
    spend(
        "2026-06-02",
        "Lowes",
        "1340.00",
        "Materials",
        "Roof repair",
        visa,
    );
    spend(
        "2026-06-09",
        "Home Depot",
        "-150.00",
        "Materials",
        "Roof repair",
        visa,
    );
    spend(
        "2026-09-14",
        "Lumber Yard",
        "2400.00",
        "Materials",
        "Smith kitchen",
        chk,
    );
    spend(
        "2026-09-20",
        "Tile World",
        "860.00",
        "Materials",
        "Smith kitchen",
        visa,
    );
    spend(
        "2026-10-02",
        "Electric Co-op",
        "640.00",
        "Labour",
        "Smith kitchen",
        chk,
    );
    spend(
        "2026-03-10",
        "Paint Store",
        "1080.00",
        "Materials",
        "Hall painting",
        chk,
    );
    spend(
        "2026-08-12",
        "Fence Supply",
        "780.00",
        "Materials",
        "Grounds:Fence",
        chk,
    );
    spend(
        "2026-08-30",
        "Print Shop",
        "180.00",
        "Hardware",
        "Building drive",
        chk,
    );
    let _ = a.test_invoke(
        "txn.create",
        json!({"date": "2026-10-04", "payee": "Pledges", "splits": [
            {"account": "Gifts", "value": "-6250.00", "project": "Building drive"},
            {"account": chk, "value": "6250.00"}]}),
    );
    let _ = a.test_invoke(
        "trip.create",
        json!({"date": "2026-06-02", "purpose_kind": "charity", "purpose": "Roof supplies",
               "from_place": "St. Mark's", "to_place": "Lowes", "miles": "18", "round_trip": true,
               "vehicle": "Parish van", "project": "Roof repair"}),
    );
    let _ = a.test_invoke(
        "trip.create",
        json!({"date": "2026-09-14", "purpose_kind": "business", "purpose": "Site visit",
               "from_place": "Office", "to_place": "Smith home", "miles": "40",
               "vehicle": "Parish van", "project": "Smith kitchen"}),
    );
    // One billed invoice for the kitchen job.
    let u = a
        .test_invoke("project.unbilled", json!({"project": "Smith kitchen"}))
        .unwrap_or(Value::Null);
    let first: Vec<Value> = u["projects"][0]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .take(1)
        .map(|i| i["key"].clone())
        .collect();
    if let Ok(l) = a.test_invoke("project.billable_lines", json!({"items": first})) {
        let _ = a.test_invoke(
            "invoice.create",
            json!({"customer": "Smith Family", "date": "2026-09-30", "lines": l["lines"]}),
        );
    }
    a.set_today("2026-11-02");
    a.navigate(mk_ui::View::Projects);
}

/// The id of the subscription named `name`.
fn sub_id(a: &mut App, name: &str) -> String {
    let l = a
        .test_invoke("subscription.list", json!({"today": "2026-11-05"}))
        .unwrap_or(Value::Null);
    l["subscriptions"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(l["bills"].as_array().into_iter().flatten())
        .find(|s| s["name"] == name)
        .and_then(|s| s["id"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// subscriptions.md: a year of streaming, software and utility payments, some
/// confirmed as subscriptions (with price rises in each colour band, a cancelled
/// one charged again and a yearly renewal), two recurring bills and three
/// suggestions left to answer; then the Subscriptions screen.
fn subs_app(a: &mut App) {
    let _ = a.test_invoke("module.enable", json!({"module": "subscriptions"}));
    for n in ["Streaming", "Software", "Fitness", "Phone"] {
        let _ = a.test_invoke(
            "account.create",
            json!({"name": n, "type": "expense", "parent": "Expenses"}),
        );
    }
    let mut pay = |date: String, payee: &str, amount: &str, account: &str, cat: &str| {
        let _ = a.test_invoke(
            "txn.create",
            json!({"date": date, "payee": payee, "splits": [
                {"account": format!("Expenses:{cat}"), "value": amount},
                {"account": account, "value": format!("-{amount}")}]}),
        );
    };
    let chk = "Assets:Checking";
    let visa = "Liabilities:Visa";
    for m in 4..=10 {
        let p = if m < 8 { "15.49" } else { "17.99" };
        pay(format!("2026-{m:02}-05"), "Netflix", p, chk, "Streaming");
    }
    for m in 1..=10 {
        let p = if m < 8 { "9.99" } else { "12.99" };
        pay(format!("2026-{m:02}-08"), "Spotify", p, visa, "Streaming");
    }
    for m in 6..=10 {
        let p = if m < 9 { "2.99" } else { "4.49" };
        pay(
            format!("2026-{m:02}-12"),
            "Cloud Storage",
            p,
            visa,
            "Software",
        );
    }
    for m in 3..=9 {
        pay(format!("2026-{m:02}-20"), "Hulu", "7.99", chk, "Streaming");
    }
    for d in ["2024-11-20", "2025-11-20"] {
        pay(d.into(), "Amazon Prime", "139.00", visa, "Streaming");
    }
    for m in 5..=10 {
        pay(
            format!("2026-{m:02}-01"),
            "City Gym",
            "40.00",
            visa,
            "Fitness",
        );
    }
    pay("2026-08-18".into(), "City Power", "72.30", chk, "Utilities");
    pay("2026-09-18".into(), "City Power", "95.10", chk, "Utilities");
    pay(
        "2026-08-28".into(),
        "Water Works",
        "39.50",
        chk,
        "Utilities",
    );
    pay(
        "2026-09-28".into(),
        "Water Works",
        "41.20",
        chk,
        "Utilities",
    );
    for (d, x) in [
        ("2026-08-15", "55.20"),
        ("2026-09-15", "61.80"),
        ("2026-10-15", "58.40"),
    ] {
        pay(d.into(), "Mobile Phone Co", x, visa, "Phone");
    }
    for d in ["2026-10-12", "2026-10-19", "2026-10-26", "2026-11-02"] {
        pay(d.into(), "Daily Paper", "5.00", chk, "Groceries");
    }
    a.set_today("2026-11-05");
    let sugg = a
        .test_invoke("subscription.suggestions", json!({}))
        .unwrap_or(Value::Null);
    for s in sugg.as_array().into_iter().flatten() {
        let answer = match s["payee"].as_str().unwrap_or("") {
            "Netflix" | "Spotify" | "Cloud Storage" | "Hulu" | "Amazon Prime" | "City Gym" => {
                "subscription"
            }
            "City Power" => "bill",
            _ => continue,
        };
        let _ = a.test_invoke(
            "subscription.answer",
            json!({"key": s["key"], "answer": answer, "today": "2026-11-05"}),
        );
    }
    let hulu = sub_id(a, "Hulu");
    let _ = a.test_invoke(
        "subscription.set_status",
        json!({"subscription": hulu, "status": "cancelled", "on": "2026-10-20"}),
    );
    let _ = a.test_invoke(
        "txn.create",
        json!({"date": "2026-11-03", "payee": "Hulu", "splits": [
            {"account": "Expenses:Streaming", "value": "7.99"},
            {"account": chk, "value": "-7.99"}]}),
    );
    let spotify = sub_id(a, "Spotify");
    if let Ok(g) = a.test_invoke("subscription.get", json!({"subscription": spotify})) {
        let row = g["history"][0]["key"].clone();
        let _ = a.test_invoke(
            "subscription.set_reason",
            json!({"subscription": spotify, "row": row, "reason": "Family plan price rise"}),
        );
    }
    let gym = sub_id(a, "City Gym");
    let _ = a.test_invoke(
        "subscription.update",
        json!({"subscription": gym, "used_by": "Sam", "reviewed_on": "2026-06-01"}),
    );
    a.navigate(mk_ui::View::Subscriptions);
}

/// mileage.md: two vehicles, odometer readings and a mix of trips (log only,
/// reimbursable and posted, from the phone), then the Mileage screen.
fn mileage_app(a: &mut App) {
    for m in ["mileage", "expenses"] {
        let _ = a.test_invoke("module.enable", json!({"module": m}));
    }
    for v in ["2019 Honda CR-V", "Parish van"] {
        let _ = a.test_invoke("vehicle.create", json!({"name": v}));
    }
    let _ = a.test_invoke(
        "vehicle.set_odometer",
        json!({"vehicle": "2019 Honda CR-V", "year": 2026, "start": "30000", "end": "42000"}),
    );
    let trips = [
        json!({"date": "2026-10-06", "vehicle": "2019 Honda CR-V", "from_place": "Office",
               "to_place": "Client", "miles": "12.4", "round_trip": true,
               "purpose": "Quarterly review with Acme"}),
        json!({"date": "2026-10-14", "vehicle": "Parish van", "from_place": "St. Mark's",
               "to_place": "Food pantry", "miles": "8.3", "round_trip": true,
               "purpose_kind": "charity", "purpose": "Food pantry delivery for St. Mark's"}),
        json!({"date": "2026-10-20", "vehicle": "2019 Honda CR-V", "from_place": "Home",
               "to_place": "Clinic", "miles": "6.0", "round_trip": true,
               "purpose_kind": "medical", "purpose": "Physical therapy"}),
        json!({"date": "2026-10-27", "vehicle": "2019 Honda CR-V",
               "odometer_start": "41210.3", "odometer_end": "41236.0",
               "from_place": "Office", "to_place": "Supplier", "purpose": "Pick up samples",
               "reimbursable": true, "driver": "Pat Lee"}),
        json!({"date": "2026-10-30", "vehicle": "2019 Honda CR-V", "from_place": "Office",
               "to_place": "Airport", "miles": "21.5", "purpose": "Conference travel",
               "reimbursable": true, "driver": "Pat Lee", "source": "phone", "client_id": "p-1"}),
        json!({"date": "2026-11-01", "vehicle": "Parish van", "miles": "14.0",
               "purpose_kind": "personal"}),
    ];
    for t in trips {
        let made = a.test_invoke("trip.create", t);
        if let Ok(v) = made
            && v.get("date").and_then(Value::as_str) == Some("2026-10-27")
            && let Some(id) = v.get("id").and_then(Value::as_str)
        {
            let _ = a.test_invoke("trip.post", json!({"trip": id}));
        }
    }
    a.open_view_spec("mileage");
}

/// A book linked to the scripted SimpleFIN server with one synced, unreviewed batch.
fn feed_app(theme: mk_ui::Theme) -> App {
    use mk_commands::connect::{Connector, FakeSimplefin, MemorySecrets};
    let dir =
        std::env::temp_dir().join(format!("mk-screens-{}-feed-{theme:?}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Household.moneykeep");
    let fake = FakeSimplefin::new(
        r#"{"errlist": [], "connections": [{"conn_id": "c1", "name": "First Bank"}],
          "accounts": [
            {"id": "a1", "name": "Everyday", "conn_id": "c1", "currency": "USD",
             "balance": "1200.50", "balance-date": 1790812800, "transactions": [
               {"id": "t1", "posted": 1790553600, "amount": "-45.10", "description": "Corner Grocery"},
               {"id": "t2", "posted": 1790640000, "amount": "2000.00", "description": "Payroll"},
               {"id": "t3", "posted": 1790726400, "amount": "-12.00", "description": "Bakery"}]},
            {"id": "a2", "name": "Chase Sapphire Visa", "conn_id": "c1", "currency": "USD",
             "balance": "-300.00", "balance-date": 1790812800, "transactions": []}]}"#,
    );
    let mut s = Session::create(&path, "Household", "USD", "app").unwrap();
    s.set_connector(Connector {
        fetcher: std::sync::Arc::new(fake),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(1_790_812_800),
        host: None,
    });
    for (c, a) in [
        ("module.enable", json!({"module": "bank-feeds"})),
        (
            "account.create",
            json!({"name": "Assets", "type": "asset", "placeholder": true}),
        ),
        (
            "account.create",
            json!({"name": "Checking", "type": "asset", "subtype": "bank", "parent": "Assets"}),
        ),
        (
            "account.create",
            json!({"name": "Expenses", "type": "expense", "placeholder": true}),
        ),
        (
            "account.create",
            json!({"name": "Groceries", "type": "expense", "parent": "Expenses"}),
        ),
        (
            "txn.create",
            json!({"date": "2026-09-01", "payee": "Opening balance", "splits": [
            {"account": "Assets:Checking", "value": "850.00"},
            {"account": "Opening Balances", "value": "-850.00"}]}),
        ),
        (
            "txn.create",
            json!({"date": "2026-09-14", "payee": "Green Market", "splits": [
            {"account": "Expenses:Groceries", "value": "64.20"},
            {"account": "Assets:Checking", "value": "-64.20"}]}),
        ),
    ] {
        s.invoke(c, a).unwrap();
    }
    let conn = s
        .invoke(
            "connect.add",
            json!({"setup_input": FakeSimplefin::token("shots"), "label": "My bank", "confirm": true}),
        )
        .unwrap();
    let id = conn["id"].as_str().unwrap().to_owned();
    s.invoke(
        "connect.link_account",
        json!({"connection": id, "remote_account": "a1", "account": "Assets:Checking",
               "sync_from": "2026-09-01"}),
    )
    .unwrap();
    s.invoke("connect.sync", json!({"connection": id, "now": true}))
        .unwrap();
    let cfg = UserConfig {
        theme,
        date_format: "MM-DD-YYYY".into(),
        settings_section: "Connections".into(),
        ..UserConfig::default()
    };
    let mut app = App::new(
        Box::new(Local {
            session: Some(s),
            name: Some("Household".into()),
        }),
        cfg,
    );
    app.set_today("2026-10-01");
    app
}

fn save_png(path: &std::path::Path, buf: &slint::SharedPixelBuffer<slint::Rgba8Pixel>) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), buf.width(), buf.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap();
    w.write_image_data(buf.as_bytes()).unwrap();
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "screens".into()));
    std::fs::create_dir_all(&out).unwrap();
    let only: Option<String> = std::env::args().nth(2);
    let dir = tempfile::tempdir().unwrap();
    let mut jobs: Vec<(String, mk_ui::Theme, usize)> = Vec::new();
    for theme in [mk_ui::Theme::Light, mk_ui::Theme::Dark] {
        for (i, (name, _)) in shots().iter().enumerate() {
            if only.as_deref().is_none_or(|o| name.contains(o)) {
                jobs.push(((*name).to_owned(), theme, i));
            }
        }
    }
    let jobs = Rc::new(RefCell::new(jobs.into_iter()));
    let current: Rc<RefCell<Option<Views>>> = Rc::new(RefCell::new(None));
    let dir_path = dir.path().to_owned();
    let next: Rc<RefCell<Option<Box<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let next2 = next.clone();
    *next.borrow_mut() = Some(Box::new(move || {
        let Some((name, theme, i)) = jobs.borrow_mut().next() else {
            let _ = slint::quit_event_loop();
            return;
        };
        let mut app = demo_app(&dir_path, theme, i);
        (shots()[i].1)(&mut app);
        let views = Views::new(app).unwrap();
        views.window().window().set_size(slint::LogicalSize::new(
            1280.0,
            // Long dialogs are also rendered whole, for review.
            if name.ends_with("-accounts")
                || name == "add-chart"
                || name.starts_with("dashboard")
                || name == "chart-hover"
                || name == "mileage-settings"
                || name == "mileage-trip-form"
                || name.starts_with("subs-")
                || name.starts_with("proj-")
            {
                1800.0
            } else {
                820.0
            },
        ));
        views.show().unwrap();
        *current.borrow_mut() = Some(views);
        let cur = current.clone();
        let out = out.clone();
        let next3 = next2.clone();
        let mut t = 0.0;
        let timer = Rc::new(slint::Timer::default());
        let tm = timer.clone();
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(100),
            move || {
                t += 0.1;
                if let Some(v) = cur.borrow().as_ref() {
                    v.tick(t);
                    // Over the spending chart's Utilities bar.
                    if name == "chart-hover" && (0.55..0.65).contains(&t) {
                        v.window().window().dispatch_event(
                            slint::platform::WindowEvent::PointerMoved {
                                position: slint::LogicalPosition::new(300.0, 1162.0),
                            },
                        );
                    }
                }
                if t > 1.2 {
                    tm.stop();
                    if let Some(v) = cur.borrow_mut().take() {
                        match v.window().window().take_snapshot() {
                            Ok(buf) => {
                                let p = out.join(format!(
                                    "{name}-{}.png",
                                    if theme == mk_ui::Theme::Dark {
                                        "dark"
                                    } else {
                                        "light"
                                    }
                                ));
                                save_png(&p, &buf);
                                println!("{}", p.display());
                            }
                            Err(e) => println!("snapshot failed: {e}"),
                        }
                        let _ = v.window().hide();
                    }
                    if let Some(f) = next3.borrow().as_ref() {
                        f();
                    }
                }
            },
        );
        std::mem::forget(timer);
    }));
    let n = next.clone();
    slint::Timer::single_shot(std::time::Duration::from_millis(10), move || {
        if let Some(f) = n.borrow().as_ref() {
            f();
        }
    });
    slint::run_event_loop_until_quit().unwrap();
}
