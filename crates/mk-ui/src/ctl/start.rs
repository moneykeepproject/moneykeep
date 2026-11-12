//! The start screen (app-desktop.md revisions 26 and 40): the top bar with the
//! MoneyKeep mark and the theme button, the toolbar (Open a book…, sort, search),
//! the book cards with the Create card, the About link, and the Create a new book
//! dialog.

use std::rc::Rc;

use super::App;
use super::ui::{
    CARD_BADGE, CARD_BUTTON, CARD_CURRENCY, CARD_DESCRIPTION, CARD_LAST_OPENED, CARD_LINK,
    CARD_STATUS, CARD_TAGLINE, Item, Kind, Picture, Tone, Ui,
};
use super::util::{png_picture, tile_color};
use crate::AppEvent;
use crate::base::recent::{TileCache, TileIcon, fallback_tile, short_date};

#[derive(Debug)]
pub(crate) struct StartState {
    pub name: String,
    pub currency: String,
    pub tagline: String,
    pub description: String,
    pub protect: bool,
    pub password: String,
    pub confirm: String,
    pub no_recovery: bool,
    pub dialog: bool,
    /// The chart of accounts list and its panel (chart-presets.md).
    pub picker: super::presets::Picker,
    /// The chart the next created book starts from (set by Create book…).
    pub preset: Option<String>,
    /// The tagline the next created book gets (set by Create book…).
    pub new_tagline: Option<String>,
    /// The search box (revision 40).
    pub search: String,
}

impl Default for StartState {
    fn default() -> Self {
        StartState {
            name: "My Books".into(),
            currency: "USD".into(),
            tagline: String::new(),
            description: String::new(),
            protect: false,
            password: String::new(),
            confirm: String::new(),
            no_recovery: false,
            dialog: false,
            picker: super::presets::Picker {
                selected: super::presets::DEFAULT_PRESET.into(),
                show_accounts: false,
            },
            preset: None,
            new_tagline: None,
            search: String::new(),
        }
    }
}

/// Why a card cannot open its book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The file is missing or unreadable.
    NotFound(String),
    /// Opening failed with this error.
    CouldNotOpen(String),
}

/// What the start screen shows for one recent book.
struct CardRow {
    path: String,
    tile: TileCache,
    locked: bool,
    failure: Option<Failure>,
    last_opened: Option<String>,
}

/// The sort choices (revision 40) and their config values.
const SORTS: [(&str, &str); 2] = [("last_opened", "Last opened"), ("name", "Name (A–Z)")];

/// The colour's text colour on a badge: near-black on amber, else white (each at
/// least 4.5:1, revision 26).
fn on_color(color: &str) -> bool {
    color == "amber"
}

/// A text part of a card.
fn part(text: impl Into<String>) -> Item {
    Item {
        text: text.into(),
        ..Item::default()
    }
}

/// The picture of a book (its stored picture loaded from the tile cache).
fn card_picture(app: &mut App, path: Option<&str>, tile: &TileCache) -> Option<Rc<Picture>> {
    match (&tile.icon, path) {
        (TileIcon::Image, Some(p)) => {
            if !app.tile_pictures.contains_key(p) {
                let pic = app
                    .backend
                    .load_tile_image(p)
                    .and_then(|png| png_picture(&png, 512));
                app.tile_pictures.insert(p.to_owned(), pic);
            }
            app.tile_pictures.get(p).cloned().flatten()
        }
        _ => None,
    }
}

/// The values a card shows besides the profile.
pub(crate) struct CardExtras {
    pub number: usize,
    pub locked: bool,
    pub failure: Option<Failure>,
    pub last_opened: Option<String>,
    pub path: Option<String>,
    /// The accessible name ("Open <name>", "Preview of <name>").
    pub label: String,
}

/// The card item of a book.
pub(crate) fn book_card_item(
    app: &mut App,
    tile: &TileCache,
    extras: CardExtras,
    picture: Option<Rc<Picture>>,
) -> Item {
    let picture = picture.or_else(|| card_picture(app, extras.path.as_deref(), tile));
    let (letter, icon) = match &tile.icon {
        TileIcon::Letter { text } => (text.to_uppercase(), String::new()),
        TileIcon::Symbol { symbol } => (String::new(), format!("sym-{symbol}")),
        TileIcon::Preset { preset } => (String::new(), format!("preset-{preset}")),
        TileIcon::Image => (String::new(), String::new()),
    };
    let (status, status_tone, description) = match &extras.failure {
        Some(Failure::NotFound(e)) => ("FILE NOT FOUND", Tone::Error, e.clone()),
        Some(Failure::CouldNotOpen(e)) => ("COULDN'T OPEN", Tone::Error, e.clone()),
        None if extras.locked => ("PASSWORD PROTECTED", Tone::Normal, tile.description.clone()),
        None => ("", Tone::Normal, tile.description.clone()),
    };
    let mut tip = tile.name.clone();
    for line in [&tile.tagline, &tile.description] {
        if !line.is_empty() {
            tip.push('\n');
            tip.push_str(line);
        }
    }
    if let Some(p) = &extras.path {
        tip.push('\n');
        tip.push_str(p);
    }
    if extras.locked {
        tip.push_str("\nPassword protected");
    }
    let mut children = vec![Item::default(); CARD_LINK + 1];
    let parts = [
        (CARD_BADGE, part(format!("{:02}", extras.number))),
        (CARD_CURRENCY, part(tile.currency.clone())),
        (CARD_TAGLINE, part(tile.tagline.clone())),
        (
            CARD_STATUS,
            Item {
                tone: status_tone,
                ..part(status)
            },
        ),
        (CARD_DESCRIPTION, part(description.clone())),
        (
            CARD_LAST_OPENED,
            part(
                extras
                    .last_opened
                    .as_deref()
                    .and_then(short_date)
                    .map(|d| format!("Last opened: {d}"))
                    .unwrap_or_default(),
            ),
        ),
        (
            CARD_BUTTON,
            part(if extras.failure.is_some() {
                "REMOVE FROM LIST"
            } else {
                "OPEN"
            }),
        ),
        (
            CARD_LINK,
            part(if extras.path.is_some() {
                "SHOW IN FOLDER ↗"
            } else {
                ""
            }),
        ),
    ];
    for (i, p) in parts {
        if let Some(c) = children.get_mut(i) {
            *c = p;
        }
    }
    Item {
        kind: Kind::BookCard,
        text: tile.name.clone(),
        label: extras.label,
        desc: description,
        placeholder: tip,
        value: letter,
        icon,
        color: Some(tile_color(&tile.color)),
        bold: on_color(&tile.color),
        strike: extras.locked,
        checked: extras.failure.is_some(),
        picture,
        children,
        ..Item::default()
    }
}

/// The Create card (revision 40).
fn create_card() -> Item {
    let mut children = vec![Item::default(); CARD_LINK + 1];
    for (i, text) in [
        (CARD_BADGE, "NEW BOOK"),
        (CARD_TAGLINE, "Household, business, church or club"),
        (CARD_BUTTON, "CREATE"),
    ] {
        if let Some(c) = children.get_mut(i) {
            *c = part(text);
        }
    }
    Item {
        kind: Kind::BookCard,
        text: "Create a New Book".into(),
        value: "+".into(),
        icon: "plus".into(),
        placeholder: "Create a new book".into(),
        children,
        ..Item::default()
    }
}

/// The top bar's items: the MoneyKeep mark (a picture) and the theme button (its
/// click carries "dark" or "light", the theme the views show now).
fn top_bar(app: &mut App, ui: &mut Ui) {
    let dark = app.config.theme == crate::Theme::Dark;
    ui.toolbar(|ui| {
        let side = 64;
        let mark = Picture {
            width: side,
            height: side,
            rgba: crate::app_icon_rgba(side),
        };
        ui.item(
            Item {
                kind: Kind::Image,
                label: mk_core::brand::PRODUCT.into(),
                picture: Some(Rc::new(mark)),
                width: 22,
                ..Item::default()
            },
            false,
        );
        let name = "Switch between light and dark";
        ui.tip(name);
        let resp = ui.icon_button("theme", name, false);
        if resp.clicked() {
            let showing_dark = match resp.value.as_str() {
                "dark" => true,
                "light" => false,
                _ => dark,
            };
            app.config.theme = if showing_dark {
                crate::Theme::Light
            } else {
                crate::Theme::Dark
            };
            app.emit(AppEvent::ConfigChanged);
        }
    });
}

pub(crate) fn screen(app: &mut App, ui: &mut Ui) {
    top_bar(app, ui);
    if let Some((text, is_error)) = app.message.clone().filter(|_| !app.start.dialog) {
        ui.banner(if is_error { Tone::Error } else { Tone::Accent }, |ui| {
            ui.next().grow();
            if is_error {
                ui.error_text(&text);
            } else {
                ui.label(&text);
            }
            super::backups::open_failed_actions(app, ui);
            if let Some(d) = app.message_details.clone() {
                let mut open = app.message_details_open;
                if ui
                    .link(if open { "Hide details" } else { "Details" })
                    .clicked()
                {
                    open = !open;
                }
                app.message_details_open = open;
                if open {
                    ui.secondary(d);
                }
            }
        });
    }
    // The toolbar row.
    let mut sort = SORTS
        .iter()
        .position(|(k, _)| *k == app.config.start_sort)
        .unwrap_or(0);
    ui.group(|ui| {
        ui.row(|ui| {
            if ui.button("Open a book…").clicked() {
                app.emit(AppEvent::ChooseBookToOpen);
            }
            // backups.md: next to Open a book…, in the same outlined style.
            if ui.button("Restore a backup…").clicked() {
                super::backups::open_restore(app, true);
            }
            ui.spacer();
            ui.next().mono().quiet();
            ui.caption("SORT");
            ui.next().width(170);
            let names: Vec<String> = SORTS.iter().map(|(_, n)| (*n).to_owned()).collect();
            if ui.combo("Sort", &mut sort, &names).changed() {
                app.config.start_sort = SORTS
                    .get(sort)
                    .map(|(k, _)| (*k).to_owned())
                    .unwrap_or_default();
                if app.config.start_sort == "last_opened" {
                    app.config.start_sort.clear();
                }
                app.emit(AppEvent::ConfigChanged);
            }
            ui.next().width(220).hint("Search books");
            ui.field("Search books", &mut app.start.search);
        });
    });
    let mut rows: Vec<CardRow> = app
        .config
        .recent
        .clone()
        .into_iter()
        .map(|r| {
            let protected = app.backend.is_protected_file(&r.path);
            let failure = app
                .open_errors
                .iter()
                .find(|(p, _)| *p == r.path)
                .map(|(_, e)| Failure::CouldNotOpen(e.clone()))
                .or_else(|| {
                    protected.is_none().then(|| {
                        Failure::NotFound("This book's file can't be found or read.".to_owned())
                    })
                });
            CardRow {
                tile: r.tile.clone().unwrap_or_else(|| fallback_tile(&r.path)),
                locked: protected == Some(true),
                failure,
                last_opened: r.last_opened.clone(),
                path: r.path,
            }
        })
        .collect();
    if sort == 1 {
        rows.sort_by_key(|r| r.tile.name.to_lowercase());
    }
    let needle = app.start.search.trim().to_lowercase();
    if !needle.is_empty() {
        rows.retain(|r| {
            [&r.tile.name, &r.tile.tagline, &r.tile.description]
                .iter()
                .any(|t| t.to_lowercase().contains(&needle))
        });
    }
    let mut open = None;
    let mut remove = None;
    let mut folder = None;
    let mut back_up: Option<(String, String, bool)> = None;
    let mut create = false;
    ui.tiles("Books", |ui| {
        ui.center_section();
        for (i, row) in rows.iter().enumerate() {
            let item = book_card_item(
                app,
                &row.tile,
                CardExtras {
                    number: i + 1,
                    locked: row.locked,
                    failure: row.failure.clone(),
                    last_opened: row.last_opened.clone(),
                    path: Some(row.path.clone()),
                    label: format!("Open {}", row.tile.name),
                },
                None,
            );
            let mut item = item;
            item.options = vec![
                "Open".into(),
                "Show in folder".into(),
                // backups.md revision 2: a manual backup without opening the book.
                super::backups::BACK_UP_THIS_BOOK.into(),
                "Remove from list".into(),
            ];
            ui.next().id(format!("tile:{}", row.path));
            let resp = ui.item(item, true);
            if resp.clicked() {
                match resp.value.as_str() {
                    "remove" => remove = Some(row.path.clone()),
                    "folder" => folder = Some(row.path.clone()),
                    _ if row.failure.is_some() && resp.value == "open" => {
                        remove = Some(row.path.clone());
                    }
                    _ => open = Some(row.path.clone()),
                }
            }
            match resp.secondary {
                Some(Some(0)) => open = Some(row.path.clone()),
                Some(Some(1)) => folder = Some(row.path.clone()),
                Some(Some(2)) => {
                    back_up = Some((row.path.clone(), row.tile.name.clone(), row.locked))
                }
                Some(Some(3)) => remove = Some(row.path.clone()),
                _ => {}
            }
            if let Some(k) = resp.key {
                tile_key(app, &rows, &row.path, &k);
            }
        }
        if ui.item(create_card(), true).clicked() {
            create = true;
        }
    });
    if rows.is_empty() && !needle.is_empty() {
        ui.group(|ui| {
            ui.secondary(format!("No books match “{}”.", app.start.search.trim()));
        });
    }
    ui.group(|ui| {
        ui.row(|ui| {
            ui.spacer();
            // The product's tag line (branding.md AT2) and the footer link to About
            // MoneyKeep (ui-slint.md).
            ui.next().tone(Tone::Secondary);
            ui.label(mk_core::brand::TAGLINE);
            ui.next().tone(Tone::Secondary).quiet();
            ui.label("·");
            if ui.link("About").clicked() {
                super::about::open(app);
            }
            ui.spacer();
        });
    });
    if create {
        app.start.dialog = true;
        app.start.picker = super::presets::Picker {
            selected: super::presets::DEFAULT_PRESET.into(),
            show_accounts: false,
        };
        app.clear_message();
    }
    create_dialog(app, ui);
    if let Some(p) = open {
        app.open_book(&p);
    }
    if let Some(p) = remove {
        app.forget_recent(&p);
    }
    if let Some(path) = folder {
        app.emit(AppEvent::ShowInFolder { path });
    }
    if let Some((path, name, locked)) = back_up {
        super::backups::back_up_card(app, &path, &name, locked);
    }
}

/// Arrow keys move between cards (left/right along the list; up/down by the
/// columns the window reported, else by one).
fn tile_key(app: &mut App, rows: &[CardRow], path: &str, key: &str) {
    let Some(i) = rows.iter().position(|r| r.path == path) else {
        return;
    };
    let count = rows.len() + 1;
    let target = match key {
        "left" => i.checked_sub(1),
        "right" => Some(i + 1).filter(|n| *n < count),
        _ => None,
    };
    if let Some(t) = target {
        let id = rows.get(t).map_or_else(
            || "Create a New Book".to_owned(),
            |r| format!("tile:{}", r.path),
        );
        app.request_focus(&id);
    }
}

/// The Create a new book dialog (revisions 26 and 40).
fn create_dialog(app: &mut App, ui: &mut Ui) {
    if !app.start.dialog {
        return;
    }
    let mut close = ui.dialog_closed("create-book");
    let presets = super::presets::presets(app);
    ui.dialog("create-book", "Create a new book", 760, |ui| {
        if let Some((text, true)) = app.message.clone() {
            ui.error_text(text);
        }
        ui.group(|ui| {
            ui.form("Book name", |ui| {
                ui.next().grow();
                ui.field("Book name", &mut app.start.name);
            });
            ui.form("Base currency", |ui| {
                ui.next().width(120);
                ui.field("Base currency", &mut app.start.currency);
            });
            ui.form("Tagline (optional)", |ui| {
                ui.next().grow();
                let r = ui.field("Tagline (optional)", &mut app.start.tagline);
                if r.changed {
                    let max = crate::base::about::MAX_TAGLINE;
                    if app.start.tagline.chars().count() > max {
                        app.start.tagline = app.start.tagline.chars().take(max).collect();
                    }
                }
            });
            ui.form("Description (optional)", |ui| {
                ui.next().grow();
                let r = ui.field("Description (optional)", &mut app.start.description);
                if r.changed {
                    let max = crate::base::about::MAX_DESCRIPTION;
                    if app.start.description.chars().count() > max {
                        app.start.description = app.start.description.chars().take(max).collect();
                    }
                }
            });
        });
        if !presets.is_empty() {
            super::presets::picker(ui, &presets, &mut app.start.picker);
        }
        ui.group(|ui| {
            ui.checkbox(&mut app.start.protect, "Protect with a password");
            if app.start.protect {
                ui.form("Password", |ui| {
                    ui.next().grow();
                    ui.password("Password", &mut app.start.password);
                });
                ui.form("Confirm password", |ui| {
                    ui.next().grow();
                    ui.password("Confirm password", &mut app.start.confirm);
                });
                ui.secondary(crate::base::security::PASSPHRASE_NOTE);
                let mut recovery = !app.start.no_recovery;
                ui.checkbox(&mut recovery, "Create a recovery key");
                app.start.no_recovery = !recovery;
            }
        });
        ui.actions(|ui| {
            if ui.button("Cancel").clicked() {
                app.clear_message();
                close = true;
            }
            if ui.primary("Create book…").clicked() {
                let (name, currency) = (
                    app.start.name.trim().to_owned(),
                    app.start.currency.trim().to_uppercase(),
                );
                let problem = app
                    .start
                    .protect
                    .then(|| {
                        crate::base::security::new_password_problem(
                            &app.start.password,
                            &app.start.confirm,
                        )
                    })
                    .flatten();
                if name.is_empty() {
                    app.message = Some(("Give the book a name.".into(), true));
                } else if let Some(p) = problem {
                    app.message = Some((p, true));
                } else {
                    app.clear_message();
                    close = true;
                    app.start.preset =
                        Some(app.start.picker.selected.clone()).filter(|p| !p.is_empty());
                    app.start.new_tagline =
                        Some(app.start.tagline.trim().to_owned()).filter(|t| !t.is_empty());
                    app.emit(AppEvent::ChooseNewBookPath { name, currency });
                }
            }
        });
    });
    if close {
        app.start.dialog = false;
    }
}
