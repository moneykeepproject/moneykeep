//! Import review (import-pipeline.md, app-desktop.md revisions 2, 8, 18, 21):
//! sections with account pickers, rows with "what will happen" chips, category
//! pickers, skip toggles, Add to books / Discard; the CSV layout editor; payment
//! processor payouts.

use base64::Engine;
use serde_json::{Value, json};

use super::App;
use super::ui::{Align, Column, Tone, Ui};
use super::util::{Json, array};
use crate::View;
use crate::fmt;

#[derive(Debug, Default)]
pub(crate) struct ImportState {
    cache: Option<(u64, String, Value)>,
    pub accounts: Vec<Value>,
    pub picks: Vec<(String, bool)>,
    pub searches: std::collections::HashMap<String, String>,
    pub always: std::collections::HashMap<String, bool>,
    pub error: Option<String>,
    pub committed: Option<Value>,
    pub csv: Option<(String, Vec<u8>, String)>,
    pub entries_open: std::collections::HashMap<String, bool>,
    pub inv: super::inv_import::InvState,
}

fn set(v: &mut Value, k: &str, x: Value) {
    if let Some(o) = v.as_object_mut() {
        o.insert(k.to_owned(), x);
    }
}

/// Stage file bytes and open the review (or the CSV layout editor).
pub(crate) fn stage(app: &mut App, name: &str, bytes: &[u8], profile: Option<&str>) {
    let mut args = json!({
        "name": name,
        "content_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
    });
    if let Some(p) = profile {
        set(&mut args, "profile", json!(p));
    }
    match app.run("import.stage", args) {
        Ok(v) if !v.at("needs_inv_profile").is_null() => {
            app.import.csv = None;
            super::inv_import::open_editor(app, name, bytes, v.at("needs_inv_profile"));
        }
        Ok(v) if !v.at("needs_profile").is_null() => {
            let suggested = serde_json::to_string_pretty(v.at("needs_profile").at("suggested"))
                .unwrap_or_default();
            app.import.csv = Some((name.to_owned(), bytes.to_vec(), suggested));
            app.import.committed = None;
            app.navigate(View::Import {
                batch: String::new(),
            });
        }
        Ok(v) => {
            app.import.inv = super::inv_import::InvState::default();
            app.import.picks.clear();
            app.import.searches.clear();
            app.import.always.clear();
            app.import.committed = None;
            app.import.error = None;
            app.navigate(View::Import {
                batch: v.str("id").to_owned(),
            });
        }
        Err(e) => app.notify_error(&e),
    }
}

fn load(app: &mut App, batch: &str) -> Option<Value> {
    if let Some((g, b, v)) = &app.import.cache
        && *g == app.generation
        && b == batch
    {
        return Some(v.clone());
    }
    let v = app.read("import.get", json!({"batch": batch}))?;
    app.import.accounts = array(app.read("account.list", json!({})));
    app.import.cache = Some((app.generation, batch.to_owned(), v.clone()));
    let sections = v.list("sections");
    if app.import.picks.len() != sections.len() {
        app.import.picks = sections
            .iter()
            .map(|s| (s.str("account_name_resolved").to_owned(), true))
            .collect();
    }
    Some(v)
}

fn direction_titles(app: &App, b: &Value, r: &Value) -> Option<(&'static str, &'static str)> {
    let section = r
        .at("section")
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0);
    let s = b.list("sections").get(section)?;
    let id = s.at("account").as_str()?;
    let a = app.import.accounts.iter().find(|a| a.str("id") == id)?;
    let (first, second, _) = super::register::directions(a.str("type"), a.str("subtype"));
    Some((first, second))
}

pub(crate) fn chip_text(r: &Value) -> &'static str {
    if r.at("needs_review").as_bool() == Some(true) {
        return "Needs review";
    }
    match r.str("class") {
        "duplicate" => "Already imported",
        "match" => "Matches existing",
        "correction" | "correction_delete" => "Bank correction",
        "date_correction" => "Date corrected",
        "pending" => "Choose an account",
        _ => "New",
    }
}

fn chip_tone(chip: &str) -> Tone {
    match chip {
        "New" => Tone::Accent,
        "Already imported" => Tone::Secondary,
        "Needs review" | "Choose an account first" | "Choose an account" => Tone::Warning,
        _ => Tone::Success,
    }
}

pub(crate) fn view(app: &mut App, ui: &mut Ui, batch: &str) {
    if super::inv_import::editing(app) {
        super::inv_import::profile_editor(app, ui);
        return;
    }
    if app.import.csv.is_some() {
        csv_editor(app, ui);
        return;
    }
    let Some(b) = load(app, batch) else { return };
    ui.title(format!("Import review: {}", b.str("source")));
    for w in b.list("warnings") {
        ui.secondary(w.as_str().unwrap_or(""));
    }
    if let Some(done) = app.import.committed.clone() {
        let corrected = done.at("date_corrected").as_u64().unwrap_or(0);
        ui.card("", |ui| {
            ui.label(format!(
                "Added to books: {} new, {} matched, {} skipped{}.",
                done.at("created"),
                done.at("matched"),
                done.at("skipped"),
                if corrected > 0 {
                    format!(", {corrected} dates corrected")
                } else {
                    String::new()
                }
            ));
            for s in b.list("sections").to_vec() {
                if !s.at("statement").is_null()
                    && let Some(acc) = s.at("account").as_str()
                    && ui.button("Reconcile now").clicked()
                {
                    app.reconcile_start_statement(acc, s.str("statement"));
                }
            }
        });
        return;
    }
    let staged = b.str("status") == "staged";
    if !staged {
        ui.label(format!("This batch is {}.", b.str("status")));
    }
    let accounts = app.import.accounts.clone();
    let iif = b.str("format") == "iif";
    if iif {
        super::iif::mapping(app, ui, &b, &accounts);
    }
    for (si, s) in b.list("sections").iter().enumerate() {
        if iif {
            continue;
        }
        let desc = s.str("account_name");
        let desc = if desc.is_empty() {
            "Bank file account"
        } else {
            desc
        };
        ui.card(desc, |ui| {
            ui.row(|ui| {
                let (pick, remember) = app.import.picks.get(si).cloned().unwrap_or_default();
                let investment = s.str("kind") == "investment";
                let names: Vec<String> = accounts
                    .iter()
                    .filter(|a| {
                        a.at("placeholder").as_bool() != Some(true)
                            && if investment {
                                a.str("subtype") == "brokerage"
                            } else {
                                matches!(a.str("type"), "asset" | "liability")
                            }
                    })
                    .map(|a| a.str("full_name").to_owned())
                    .collect();
                let mut chosen = pick.clone();
                ui.width(260);
                ui.combo_str(&format!("Account for section {}", si + 1), &mut chosen, &names);
                if pick.is_empty()
                    && let Some(it) = ui.last()
                {
                    it.value = "Choose an account…".into();
                }
                let rememberable = !s.at("account_key").is_null();
                let mut remember = remember && rememberable;
                if rememberable {
                    ui.checkbox(&mut remember, "Remember for future files");
                    if let Some(p) = app.import.picks.get_mut(si) {
                        p.1 = remember;
                    }
                }
                if chosen != pick && !chosen.is_empty() {
                    match app.run("import.map_account", json!({"batch": b.str("id"), "section": si, "account": chosen, "remember": remember})) {
                        Ok(_) => {
                            if let Some(p) = app.import.picks.get_mut(si) {
                                p.0 = chosen;
                            }
                            app.import.error = None;
                        }
                        Err(e) => app.import.error = Some(e.display()),
                    }
                }
            });
            if s.str("kind") == "investment" {
                super::inv_import::securities_panel(app, ui, &b, si, s);
            }
        });
    }
    if let Some(err) = app.import.error.clone() {
        ui.error_text(err);
    }
    for (si, s) in b.list("sections").iter().enumerate() {
        if s.str("kind") == "investment" {
            let fed = feed_holdings(app, ui, &b, s);
            super::inv_import::rows_table(app, ui, &b, si);
            if !fed {
                super::inv_import::holdings_check(ui, s);
            }
        }
    }
    let rows: Vec<Value> = b.list("rows").to_vec();
    let resolved_sections: Vec<bool> = b
        .list("sections")
        .iter()
        .map(|s| s.at("resolved").as_bool() == Some(true))
        .collect();
    if b.str("format") == "processor" {
        for r in &rows {
            processor_row(app, ui, &b, r, staged);
        }
    } else if !rows.is_empty() {
        let cols = vec![
            Column {
                title: "Date".into(),
                width: 100,
                ..Column::default()
            },
            Column {
                title: "Payee".into(),
                weight: 2,
                ..Column::default()
            },
            Column {
                title: "Memo".into(),
                ..Column::default()
            },
            Column {
                title: "Amount".into(),
                width: 100,
                align: Align::Right,
                ..Column::default()
            },
            Column {
                title: "What will happen".into(),
                width: 150,
                ..Column::default()
            },
            Column {
                title: "Category".into(),
                weight: 4,
                ..Column::default()
            },
            Column {
                title: "Skip".into(),
                width: 48,
                align: Align::Center,
                ..Column::default()
            },
        ];
        ui.table("Rows", cols, |ui| {
            for (i, r) in rows.iter().enumerate() {
                let section = r
                    .at("section")
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .unwrap_or(0);
                let resolved = resolved_sections.get(section).copied().unwrap_or(false);
                row(app, ui, &b, i, r, staged, resolved);
            }
        });
    }
    let mut counts: Vec<(&str, usize)> = Vec::new();
    let inv_chips = b.list("inv_rows").iter().map(super::inv_import::chip);
    for c in rows.iter().map(chip_text).chain(inv_chips) {
        match counts.iter_mut().find(|(k, _)| *k == c) {
            Some(x) => x.1 += 1,
            None => counts.push((c, 1)),
        }
    }
    let unresolved = b
        .list("sections")
        .iter()
        .any(|s| s.at("resolved").as_bool() != Some(true));
    ui.group(|ui| {
        ui.wrap(|ui| {
            for (k, n) in &counts {
                ui.label(format!("{k}: {n}"));
            }
        });
        ui.row(|ui| {
            ui.enabled(staged && !unresolved && !super::inv_import::securities_blocked(&b));
            if ui.primary("Add to books").clicked() {
                match app.run("import.commit", json!({"batch": b.str("id")})) {
                    Ok(v) => app.import.committed = Some(v),
                    Err(e) => app.import.error = Some(e.display()),
                }
            }
            ui.enabled(staged);
            if ui.button("Discard").clicked() {
                match app.run("import.discard", json!({"batch": b.str("id")})) {
                    Ok(_) => app.navigate(View::Home),
                    Err(e) => app.import.error = Some(e.display()),
                }
            }
        });
    });
}

fn row(app: &mut App, ui: &mut Ui, b: &Value, i: usize, r: &Value, staged: bool, resolved: bool) {
    let id = r.str("id").to_owned();
    let amount = r.str("amount").to_owned();
    let tip = direction_titles(app, b, r).map(|(first, second)| {
        let word = if fmt::is_negative(&amount) {
            first
        } else {
            second
        };
        format!(
            "{word} {}",
            fmt::group(amount.trim().trim_start_matches('-'))
        )
    });
    let chip = if resolved {
        chip_text(r)
    } else {
        "Choose an account first"
    };
    let warnings = r
        .list("warnings")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("; ");
    let cleaned = r.at("raw").list("cleaned").to_vec();
    let mark = if cleaned.is_empty() { "" } else { " (cleaned)" };
    let label = format!("Row {} {}: {chip}{mark}", i + 1, r.str("payee"));
    let mut skip_change = None;
    ui.table_row(&label, false, false, |ui| {
        ui.label(fmt::date(r.str("date")));
        ui.tip(r.str("payee"));
        ui.label(r.str("payee"));
        ui.tip(r.str("memo"));
        ui.label(r.str("memo"));
        if let Some(t) = &tip {
            ui.tip(t.clone());
        }
        ui.tone(if fmt::is_negative(&amount) {
            Tone::Decrease
        } else {
            Tone::Normal
        });
        ui.amount(fmt::group(&amount));
        // The status chip, and "cleaned" (its tooltip lists what was removed);
        // the row's own name says both.
        ui.row(|ui| {
            ui.tip(warnings.clone()).named(label.clone());
            ui.badge(chip, chip_tone(chip));
            if !mark.is_empty() {
                let notes: Vec<&str> = cleaned.iter().filter_map(Value::as_str).collect();
                ui.tip(notes.join("\n"));
                ui.caption("cleaned");
            }
        });
        category_cell(app, ui, b, i, r, staged, resolved);
        let mut skip = resolved && r.str("action") == "skip";
        let was = skip;
        ui.enabled(staged && resolved && r.str("class") != "duplicate")
            .named(format!("Skip row {}", i + 1));
        ui.checkbox(&mut skip, "");
        if skip != was {
            skip_change = Some(skip);
        }
    });
    if let Some(skip) = skip_change {
        set_row(
            app,
            b,
            &id,
            json!({"action": if skip { "skip" } else { "create" }}),
        );
    }
}

fn category_cell(
    app: &mut App,
    ui: &mut Ui,
    b: &Value,
    i: usize,
    r: &Value,
    staged: bool,
    resolved: bool,
) {
    let id = r.str("id").to_owned();
    let others: Vec<String> = r
        .list("other")
        .iter()
        .map(|o| o.str("account_name").to_owned())
        .collect();
    let current = others.join(", ");
    if !resolved {
        ui.label("");
        return;
    }
    if r.str("class") == "date_correction" {
        let what = format!(
            "{} → {}",
            fmt::date(r.str("date_from")),
            fmt::date(r.str("date"))
        );
        ui.tip(what.clone());
        ui.label(what);
        return;
    }
    if r.str("class") == "match" {
        let what = match_text(app, r);
        ui.row(|ui| {
            ui.tip(what.clone()).grow();
            ui.label(what);
            if staged && ui.link("Add as new instead").clicked() {
                set_row(app, b, &id, json!({"action": "create"}));
            }
        });
        return;
    }
    if r.str("class") != "new" || !staged {
        ui.label(current);
        return;
    }
    let by_rule = (r.str("other_source") == "rule").then(|| rule_names(app, r));
    let search = app.import.searches.get(&id).cloned().unwrap_or_default();
    let q = search.trim().to_lowercase();
    let names: Vec<String> = app
        .import
        .accounts
        .iter()
        .filter(|a| a.at("placeholder").as_bool() != Some(true))
        .map(|a| a.str("full_name").to_owned())
        .filter(|n| q.is_empty() || n.to_lowercase().contains(&q))
        .collect();
    let selected = if current.is_empty() {
        "Choose…".to_owned()
    } else {
        current.clone()
    };
    let hover = match &by_rule {
        Some(name) => format!("{selected} (by rule: {name})"),
        None => selected.clone(),
    };
    let mut chosen = None;
    let payee = r.str("payee").to_owned();
    let mut always_v = app.import.always.get(&id).copied().unwrap_or(false);
    let mut always_changed = false;
    let mut search_text = search.clone();
    ui.row(|ui| {
        let mut idx = names
            .iter()
            .position(|n| *n == current)
            .unwrap_or(usize::MAX);
        ui.tip(hover).grow();
        if ui
            .combo_search(
                &format!("Category for row {}", i + 1),
                &mut idx,
                &names,
                &mut search_text,
                &format!("Search categories for row {}", i + 1),
            )
            .changed()
        {
            chosen = names.get(idx).cloned();
        }
        if let Some(it) = ui.last() {
            it.value = selected.clone();
        }
        let label = format!("Always use this for {payee}");
        ui.named(label.clone()).tip(label);
        if ui.checkbox(&mut always_v, "Always").clicked() {
            always_changed = true;
        }
    });
    if search_text != search {
        app.import.searches.insert(id.clone(), search_text);
    }
    if always_changed {
        app.import.always.insert(id.clone(), always_v);
        if always_v && !current.is_empty() {
            set_row(
                app,
                b,
                &id,
                json!({"other_account": current, "make_rule": true}),
            );
        }
    }
    if let Some(name) = chosen {
        if name != current {
            let mut args = json!({"other_account": name});
            if always_v {
                set(&mut args, "make_rule", json!(true));
            }
            set_row(app, b, &id, args);
        }
        app.import.searches.remove(&id);
    }
}

fn match_text(app: &mut App, r: &Value) -> String {
    let link = r.at("link");
    match link.str("kind") {
        "row" => "Matches a transfer in this file".to_owned(),
        _ => match app.read("txn.get", json!({"id": link.str("txn")})) {
            Some(t) => format!(
                "Matches {} {} {}",
                fmt::date(t.str("date")),
                t.str("payee"),
                fmt::group(r.str("amount"))
            ),
            None => "Matches an existing transaction".to_owned(),
        },
    }
}

fn rule_names(app: &mut App, r: &Value) -> String {
    let ids: Vec<&str> = r
        .list("rule_ids")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    array(app.read("rule.list", json!({})))
        .iter()
        .filter(|x| ids.contains(&x.str("id")))
        .map(|x| x.str("name").to_owned())
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn set_row(app: &mut App, b: &Value, row: &str, mut args: Value) {
    set(&mut args, "batch", json!(b.str("id")));
    set(&mut args, "row", json!(row));
    match app.run("import.set_row", args) {
        Ok(_) => {
            app.import.searches.remove(row);
            app.import.error = None;
        }
        Err(e) => app.import.error = Some(e.display()),
    }
}

fn csv_editor(app: &mut App, ui: &mut Ui) {
    let Some((name, bytes, mut text)) = app.import.csv.clone() else {
        return;
    };
    ui.title(format!("CSV layout for {name}"));
    let mut done = false;
    ui.card("Profile", |ui| {
        ui.label(
            "This file's layout is new. Check the suggested profile, give it a name, and save it.",
        );
        ui.next().grow().mono();
        ui.multiline("CSV profile", &mut text);
        if let Some(it) = ui.last() {
            it.width = 640;
        }
    });
    ui.card("First rows of the file", |ui| {
        for line in String::from_utf8_lossy(&bytes).lines().take(6) {
            ui.next().mono();
            ui.label(line.to_owned());
        }
    });
    if let Some(err) = app.import.error.clone() {
        ui.error_text(err);
    }
    ui.actions(|ui| {
        if ui.button("Cancel").clicked() {
            app.import.csv = None;
            app.navigate(View::Home);
            done = true;
        }
        if ui.primary("Save profile and import").clicked() {
            match serde_json::from_str::<Value>(&text) {
                Ok(profile) => {
                    let pname = profile.str("name").to_owned();
                    match app.run("csv.profile_save", json!({"profile": profile})) {
                        Ok(_) => {
                            app.import.csv = None;
                            app.import.error = None;
                            stage(app, &name, &bytes, Some(&pname));
                            done = true;
                        }
                        Err(e) => app.import.error = Some(e.display()),
                    }
                }
                Err(e) => app.import.error = Some(format!("The profile is not valid JSON: {e}")),
            }
        }
    });
    if !done && let Some(c) = app.import.csv.as_mut() {
        c.2 = text;
    }
}

fn short_id(id: &str) -> String {
    if id.chars().count() > 10 {
        format!("{}…", id.chars().take(7).collect::<String>())
    } else {
        id.to_owned()
    }
}

fn processor_row(app: &mut App, ui: &mut Ui, b: &Value, r: &Value, staged: bool) {
    let raw: Value = serde_json::from_str(r.at("raw").str("raw")).unwrap_or(Value::Null);
    let g = |k: &str| fmt::group(raw.str(k));
    let chip = chip_text(r);
    ui.card("", |ui| {
        if raw.at("entries").is_array() && r.str("payee").ends_with("payout") {
            let line = format!(
                "{} · payout {} · gross {} · refunds {} · fees {} · other {} · net {}",
                fmt::date(r.str("date")),
                short_id(raw.str("payout")),
                g("gross"),
                g("refunds"),
                g("fees"),
                g("other"),
                g("net"),
            );
            ui.row(|ui| {
                ui.tip(raw.str("payout")).bold().grow();
                ui.label(line);
                ui.badge(chip, chip_tone(chip));
            });
            let entries = raw.list("entries").to_vec();
            let key = r.str("id").to_owned();
            let mut open = app.import.entries_open.get(&key).copied().unwrap_or(false);
            ui.scope(format!("entries:{key}"), |ui| {
                ui.collapsing(&format!("Entries ({})", entries.len()), &mut open, |ui| {
                    let cols = vec![
                        Column {
                            title: "Entry".into(),
                            width: 110,
                            ..Column::default()
                        },
                        Column {
                            title: "Description".into(),
                            ..Column::default()
                        },
                        Column {
                            title: "Gross".into(),
                            width: 100,
                            align: Align::Right,
                            ..Column::default()
                        },
                        Column {
                            title: "Fee".into(),
                            width: 100,
                            align: Align::Right,
                            ..Column::default()
                        },
                        Column {
                            title: "Net".into(),
                            width: 100,
                            align: Align::Right,
                            ..Column::default()
                        },
                    ];
                    ui.table("Entries", cols, |ui| {
                        for e in &entries {
                            ui.table_row(e.str("id"), false, false, |ui| {
                                ui.label(e.str("kind"));
                                ui.label(e.str("description"));
                                ui.amount(fmt::group(e.str("gross")));
                                ui.amount(fmt::group(e.str("fee")));
                                ui.tip(format!("{} net", e.str("id")));
                                ui.amount(fmt::group(e.str("net")));
                            });
                        }
                    });
                });
            });
            app.import.entries_open.insert(key, open);
        } else {
            ui.row(|ui| {
                ui.next().grow();
                ui.label(format!(
                    "{} · {} · {}",
                    fmt::date(r.str("date")),
                    r.str("payee"),
                    fmt::group(r.str("amount"))
                ));
                ui.badge(chip, chip_tone(chip));
            });
        }
        for w in r.list("warnings") {
            ui.secondary(w.as_str().unwrap_or(""));
        }
        let invoices: Vec<String> = r
            .at("raw")
            .list("invoices")
            .iter()
            .map(|i| i.str("number").to_owned())
            .collect();
        if !invoices.is_empty() && staged {
            let names = invoices.join(", ");
            let decided = r.at("overrides").at("invoice").as_bool();
            ui.row(|ui| {
                match decided {
                    Some(true) => ui.label(format!("Applies to {names}")),
                    Some(false) => ui.label("Posts as sales"),
                    None => ui.label(format!("Apply to {names}?")),
                }
                if decided != Some(true) {
                    ui.named(format!("Accept {names}"));
                    if ui.button("Accept").clicked() {
                        set_row(app, b, r.str("id"), json!({"apply_invoices": true}));
                    }
                }
                if decided != Some(false) {
                    ui.named(format!("Use sales instead of {names}"));
                    if ui.button("Use sales instead").clicked() {
                        set_row(app, b, r.str("id"), json!({"apply_invoices": false}));
                    }
                }
            });
        }
    });
}

fn feed_holdings(app: &mut App, ui: &mut Ui, b: &Value, s: &Value) -> bool {
    let account = s.str("account").to_owned();
    if account.is_empty() || !app.module_on("bank-feeds") {
        return false;
    }
    let Some(h) = app.read("connect.holdings_check", json!({"account": account})) else {
        return false;
    };
    if h.str("batch") != b.str("id") {
        return false;
    }
    let staged = b.str("status") == "staged";
    let mut add: Option<String> = None;
    ui.card("Holdings check", |ui| {
        ui.secondary(format!(
            "{} against MoneyKeep after this import, as of {}",
            h.str("provider"),
            fmt::date(h.str("as_of"))
        ));
        let cols = [
            "Security",
            "Provider",
            "MoneyKeep after import",
            "Difference",
            "",
        ]
        .iter()
        .map(|t| Column {
            title: (*t).into(),
            ..Column::default()
        })
        .collect();
        ui.table("Holdings", cols, |ui| {
            for p in h.list("positions") {
                let differs = p.at("differs").as_bool() == Some(true);
                let tone = if differs { Tone::Error } else { Tone::Normal };
                ui.table_row(p.str("security"), false, false, |ui| {
                    for k in ["security", "provider", "book_after", "difference"] {
                        ui.tone(tone);
                        ui.label(p.str(k));
                    }
                    if differs && staged {
                        ui.named(format!(
                            "Add a row for the {} difference",
                            p.str("security")
                        ));
                        if ui
                            .button("Add a shares-in/out row for the difference")
                            .clicked()
                        {
                            add = Some(p.str("security").to_owned());
                        }
                    } else {
                        ui.label("");
                    }
                });
                if differs {
                    ui.table_row(&format!("{}-msg", p.str("security")), false, false, |ui| {
                        ui.secondary(p.str("message"));
                    });
                }
            }
            let c = h.at("cash");
            if c.is_object() {
                let differs = c.at("differs").as_bool() == Some(true);
                let tone = if differs { Tone::Error } else { Tone::Normal };
                ui.table_row("cash", false, false, |ui| {
                    for t in [
                        "Cash",
                        c.str("provider"),
                        c.str("book_after"),
                        c.str("difference"),
                    ] {
                        ui.tone(tone);
                        ui.label(t);
                    }
                    ui.label("");
                });
            }
        });
        if h.at("agree").as_bool() == Some(true) {
            ui.label("Holdings agree with the provider.");
        }
    });
    if let Some(sec) = add {
        match app.run(
            "import.add_holding_row",
            json!({"batch": b.str("id"), "security": sec}),
        ) {
            Ok(_) => app.import.error = None,
            Err(e) => app.import.error = Some(e.display()),
        }
    }
    true
}
