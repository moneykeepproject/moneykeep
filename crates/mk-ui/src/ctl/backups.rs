//! Backups (backups.md): backing up on open, on close and daily in the background
//! with status messages, Settings → Backups, the Home Attention notices, the
//! restored copy's note, the before-update failure on opening, and Restore a
//! backup…. The files are the backend's business (`book.backup`, `book.backups`,
//! `book.restore_copy`); this module only decides when and shows what happened.

use serde_json::Value;

use super::App;
use super::ui::{Column, Tone, Ui, col, col_num};
use super::util::Json;
use crate::{AppEvent, Screen, UiError, UiSecret, View};

/// The status message while a backup runs.
const BACKING_UP: &str = "Backing up…";
const SECTION: &str = "Backups";
const DAY: i64 = 86_400;

/// The user guide (`docs/help/backups.md`, backups.md revision 3), shown by Settings →
/// Backups → Help as the phone capture Help shows its guide.
pub(crate) const HELP: &str = include_str!("../../../../docs/help/backups.md");
/// The guide's section that **How to allow MoneyKeep** opens.
pub(crate) const HELP_BLOCKED: &str = "Why not the Documents folder?";
/// The buttons of a folder Windows blocked (backups.md revision 3).
pub(crate) const CHOOSE_ANOTHER: &str = "Choose another folder…";
pub(crate) const HOW_TO_ALLOW: &str = "How to allow MoneyKeep";
/// The error code of a folder Windows blocked (`mk_commands::backups::BLOCKED`).
const BLOCKED: &str = "backup_blocked";

/// The suggestion to keep a copy somewhere else (owner decision).
pub(crate) const SUGGESTION: &str = "Keep a copy of your backups somewhere else — a USB drive or a cloud folder — so one failed disk can't take your books and their backups together.";

#[derive(Debug, Default)]
pub(crate) struct BackupState {
    /// The next opening is a new book (no backup on open).
    skip_open: bool,
    /// Backups started and not reported yet.
    running: u32,
    /// When a backup was last started or the book opened (Unix seconds).
    last_try: Option<i64>,
    /// The last successful backup of the open book (the `book.backup` result).
    last_ok: Option<Value>,
    /// The last backup of the open book failed.
    failure: Option<UiError>,
    /// The open book's backups as last listed: (book, controller clock, list).
    list: Option<(String, f64, Vec<Value>)>,
    /// The clock of the last daily check (controller seconds).
    daily_checked: f64,
    /// Unix seconds to use instead of the clock (tests).
    clock: Option<i64>,
    restore: Option<Restore>,
    /// Opening stopped because the before-update backup failed.
    open_failed: Option<OpenFailed>,
    /// Settings → Backups number fields, as typed.
    drafts: Option<Vec<String>>,
    /// Back up this book… on a start-screen card, asking for the book's password.
    card: Option<CardBackup>,
    /// The guide is open: `Some(None)` from its start, `Some(Some(heading))` at a
    /// section (backups.md revision 3).
    help: Option<Option<&'static str>>,
    /// A chosen folder failed its test write and was not saved.
    refused: Option<Refused>,
    /// The last copy to the second place failed (not an unplugged drive; backups.md
    /// revision 4): the reason.
    second_failure: Option<String>,
    /// A password was just added and this many earlier backups aren't protected:
    /// the question is shown (security-review-1 B2).
    unprotected_ask: Option<usize>,
}

/// The answers to the question about earlier plain backups, and the Home note's
/// buttons (security-review-1 B2).
pub(crate) const PROTECT_THEM: &str = "Protect them now";
pub(crate) const DELETE_THEM: &str = "Delete them";
pub(crate) const KEEP_THEM: &str = "Keep them";

/// A folder that could not be written when it was chosen (backups.md revision 3).
#[derive(Debug, Clone)]
struct Refused {
    second: bool,
    folder: String,
    error: UiError,
}

/// The start-screen card's Back up this book… for a password-protected book.
#[derive(Debug)]
struct CardBackup {
    path: String,
    name: String,
    password: String,
    error: Option<String>,
}

/// The start-screen card's … menu item (backups.md revision 2).
pub(crate) const BACK_UP_THIS_BOOK: &str = "Back up this book…";

#[derive(Debug, Clone)]
struct OpenFailed {
    path: String,
    secret: Option<UiSecret>,
}

#[derive(Debug, Default)]
struct Restore {
    /// Opened from the start screen (pick the book first).
    from_start: bool,
    /// The book picked (index in the recent list) when started from the start screen.
    book: usize,
    selected: Option<usize>,
    /// The listed backups: (book path, rows).
    rows: Option<(String, Result<Vec<Value>, String>)>,
    /// The backup being restored: (path, local time `YYYY-MM-DDTHH:MM` or empty).
    backup: Option<(String, String)>,
    /// Where the copy goes, while the password is asked.
    target: Option<String>,
    asking: bool,
    password: String,
    error: Option<String>,
}

// ----- time and names ------------------------------------------------------------------

/// Unix seconds now (the test clock when set).
fn now(app: &App) -> i64 {
    app.backups.clock.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
    })
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Parts of a local `YYYY-MM-DDTHH:MM`.
fn parts(at: &str) -> Option<(i32, usize, u32, u32, u32)> {
    let y = at.get(0..4)?.parse().ok()?;
    let m: usize = at.get(5..7)?.parse().ok()?;
    let d = at.get(8..10)?.parse().ok()?;
    let h = at.get(11..13)?.parse().ok()?;
    let min = at.get(14..16)?.parse().ok()?;
    (1..=12).contains(&m).then_some((y, m, d, h, min))
}

fn clock_text(h: u32, min: u32) -> String {
    let (h12, ampm) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    format!("{h12}:{min:02} {ampm}")
}

/// "2:20 PM".
pub(crate) fn time_text(at: &str) -> String {
    parts(at).map_or_else(|| at.to_owned(), |(_, _, _, h, m)| clock_text(h, m))
}

/// "Nov 7, 2026 2:20 PM".
pub(crate) fn when_text(at: &str) -> String {
    parts(at).map_or_else(
        || at.to_owned(),
        |(y, m, d, h, min)| {
            format!(
                "{} {d}, {y} {}",
                MONTHS.get(m - 1).copied().unwrap_or(""),
                clock_text(h, min)
            )
        },
    )
}

/// "Nov 1".
fn day_text(at: &str) -> String {
    parts(at).map_or_else(
        || at.to_owned(),
        |(_, m, d, _, _)| format!("{} {d}", MONTHS.get(m - 1).copied().unwrap_or("")),
    )
}

/// The local time of `unix` as `YYYY-MM-DDTHH:MM`, using the offset the last
/// backup showed (UTC before any).
fn local_of(app: &App, unix: i64) -> String {
    let offset = app
        .backups
        .last_ok
        .as_ref()
        .and_then(|v| Some(seconds_of(v.str("at"))? - v.at("unix").as_i64()?))
        .unwrap_or(0);
    let t = unix + offset;
    let days = t.div_euclid(DAY);
    let secs = t.rem_euclid(DAY);
    let date = "1970-01-01"
        .parse::<mk_core::Date>()
        .ok()
        .and_then(|d| d.add_days(i32::try_from(days).unwrap_or(0)))
        .map(|d| d.to_string())
        .unwrap_or_default();
    format!("{date}T{:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// Seconds since 1970 of a local `YYYY-MM-DDTHH:MM` read as if it were UTC.
fn seconds_of(at: &str) -> Option<i64> {
    let (_, _, _, h, m) = parts(at)?;
    let day: mk_core::Date = at.get(..10)?.parse().ok()?;
    let epoch: mk_core::Date = "1970-01-01".parse().ok()?;
    let days = epoch.days_until(day);
    Some(days * DAY + i64::from(h) * 3600 + i64::from(m) * 60)
}

fn reason_text(r: &str) -> &'static str {
    match r {
        "opened" => "Opened",
        "before-update" => "Before update",
        "closed" => "Closed",
        "daily" => "Daily",
        "manual" => "Manual",
        _ => "Backup",
    }
}

fn size_text(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let kb = bytes.div_ceil(1024);
    if kb < 1024 {
        return format!("{kb} KB");
    }
    let tenths = bytes * 10 / (1024 * 1024);
    format!("{}.{} MB", tenths / 10, tenths % 10)
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn folder_of(path: &str) -> Option<String> {
    let name = file_name(path);
    let dir = path.get(..path.len().saturating_sub(name.len()))?;
    let dir = dir.trim_end_matches(['/', '\\']);
    (!dir.is_empty()).then(|| dir.to_owned())
}

fn stem_of(path: &str) -> String {
    let name = file_name(path);
    name.strip_suffix(&format!(".{}", mk_core::brand::BOOK_EXTENSION))
        .unwrap_or(name)
        .to_owned()
}

/// The book name and local time in a backup's file name
/// (`<book> <YYYY-MM-DD HHMM> <reason>[ n].moneykeep`).
fn parse_backup_name(path: &str) -> Option<(String, String)> {
    let stem = stem_of(path);
    let mut words: Vec<&str> = stem.split(' ').collect();
    if words
        .last()
        .is_some_and(|w| w.bytes().all(|b| b.is_ascii_digit()))
        && words.len() > 4
    {
        words.pop();
    }
    let reason = words.pop()?;
    reason_text(reason).ne("Backup").then_some(())?;
    let hhmm = words.pop()?;
    let date = words.pop()?;
    let at = format!("{date}T{}:{}", hhmm.get(..2)?, hhmm.get(2..)?);
    parts(&at)?;
    (!words.is_empty()).then(|| (words.join(" "), at))
}

/// `<book> restored <YYYY-MM-DD HHMM>.moneykeep`.
fn restored_name(stem: &str, at: &str) -> String {
    let ext = mk_core::brand::BOOK_EXTENSION;
    match (at.get(..10), at.get(11..13), at.get(14..16)) {
        (Some(d), Some(h), Some(m)) if parts(at).is_some() => {
            format!("{stem} restored {d} {h}{m}.{ext}")
        }
        _ => format!("{stem} restored.{ext}"),
    }
}

// ----- settings and status ---------------------------------------------------------------

/// Tell the backend the settings.
pub(crate) fn push_settings(app: &mut App) {
    let v = serde_json::to_value(&app.config.backup).unwrap_or(Value::Null);
    app.backend.set_backup_settings(v);
}

fn settings_changed(app: &mut App) {
    push_settings(app);
    app.backups.list = None;
    if let Some(r) = app.backups.restore.as_mut() {
        r.rows = None;
    }
    app.backups.drafts = None;
    app.emit(AppEvent::ConfigChanged);
}

/// A status message about backups, unless another message is showing.
fn say(app: &mut App, text: String) {
    let ours = app
        .message
        .as_ref()
        .is_none_or(|(m, err)| !*err && (m == BACKING_UP || m.starts_with("Backed up ")));
    if ours {
        app.notify(text);
    }
}

/// Start a backup in the background.
pub(crate) fn start(app: &mut App, reason: &str) {
    app.backups.last_try = Some(now(app));
    match app.backend.backup_start(reason) {
        Ok(()) => {
            app.backups.running += 1;
            say(app, BACKING_UP.to_owned());
        }
        Err(e) if e.code == "unsupported" => {}
        Err(e) => failed(app, e),
    }
}

fn failed(app: &mut App, e: UiError) {
    if app.message.as_ref().is_some_and(|(m, _)| m == BACKING_UP) {
        app.clear_message();
    }
    app.backups.failure = Some(e);
}

/// Remember how the copy to the second place went; the reason when it failed
/// (backups.md revision 4).
fn note_second(app: &mut App, v: &Value) -> Option<String> {
    if !v.str("second_place_path").is_empty() {
        app.config.backup_memo.second_ok = Some((now(app), v.str("at").to_owned()));
        app.backups.second_failure = None;
    }
    let failed = v.str("second_place_failed").to_owned();
    if failed.is_empty() {
        return None;
    }
    app.backups.second_failure = Some(failed.clone());
    Some(failed)
}

/// The status message of a backup whose copy to the second place failed.
fn second_failed_text(reason: &str) -> String {
    format!("Backed up. The copy to the second place failed: {reason}")
}

fn succeeded(app: &mut App, v: Value) {
    app.backups.failure = None;
    app.config.backup_memo.made += 1;
    let second = note_second(app, &v);
    app.emit(AppEvent::ConfigChanged);
    match second {
        Some(reason) => {
            app.notify_error(&UiError::new("second_place", second_failed_text(&reason)));
        }
        None => say(app, format!("Backed up {}", time_text(v.str("at")))),
    }
    app.backups.last_ok = Some(v);
    app.backups.list = None;
}

/// A book was opened: forget the last book's state and back up (unless the
/// before-update backup was just made, or the book is new).
pub(crate) fn on_opened(app: &mut App) {
    app.backups.open_failed = None;
    app.backups.failure = None;
    app.backups.last_ok = None;
    app.backups.list = None;
    app.backups.last_try = Some(now(app));
    app.backups.daily_checked = app.now;
    let skip = std::mem::take(&mut app.backups.skip_open);
    if let Some(v) = app.backend.take_open_backup() {
        succeeded(app, v);
    } else if !skip && app.config.backup.on_open {
        start(app, "opened");
    }
}

/// The next opening is a book just created.
pub(crate) fn creating(app: &mut App) {
    app.backups.skip_open = true;
}

/// Before the open book closes (closing it, opening another, quitting).
pub(crate) fn before_close(app: &mut App) {
    if app.config.backup.on_close && app.backend.backup_changed() {
        start(app, "closed");
    }
    app.backups.last_ok = None;
    app.backups.failure = None;
    app.backups.list = None;
}

/// The controller's clock: finished backups, and the daily backup.
pub(crate) fn tick(app: &mut App) -> bool {
    let mut changed = false;
    for r in app.backend.backup_poll() {
        app.backups.running = app.backups.running.saturating_sub(1);
        match r {
            Ok(v) => succeeded(app, v),
            Err(e) if e.code == "unsupported" => {}
            Err(e) => failed(app, e),
        }
        changed = true;
    }
    if app.screen == Screen::Main
        && app.config.backup.daily
        && app.backups.running == 0
        && crate::layout::elapsed(app.now, app.backups.daily_checked, 60.0)
    {
        app.backups.daily_checked = app.now;
        let last = app.backups.last_try.unwrap_or_else(|| now(app));
        if now(app) - last >= DAY && app.backend.backup_changed() {
            start(app, "daily");
            changed = true;
        }
    }
    changed
}

// ----- the open book's backups and Attention ---------------------------------------------

/// The open book's backups (listed at most once a minute).
fn listed(app: &mut App) -> Option<Vec<Value>> {
    let book = app.security.book_path.clone()?;
    let fresh = app
        .backups
        .list
        .as_ref()
        .is_some_and(|(b, at, _)| *b == book && !crate::layout::elapsed(app.now, *at, 60.0));
    if !fresh {
        let rows = app.backend.backups_list(&book).ok()?;
        let rows = rows.as_array().cloned().unwrap_or_default();
        app.backups.list = Some((book, app.now, rows));
    }
    app.backups.list.as_ref().map(|(_, _, r)| r.clone())
}

/// How many of the open book's backups (both places) aren't password-protected.
fn unprotected_count(app: &mut App) -> usize {
    listed(app).map_or(0, |rows| {
        rows.iter()
            .filter(|r| r.at("encrypted").as_bool() == Some(false))
            .count()
    })
}

/// "3 earlier backups of this book aren't password-protected."
pub(crate) fn unprotected_question(n: usize) -> String {
    if n == 1 {
        "1 earlier backup of this book isn't password-protected.".to_owned()
    } else {
        format!("{n} earlier backups of this book aren't password-protected.")
    }
}

/// The Home note: "3 backups of Household aren't password-protected."
fn unprotected_note(n: usize, book: &str) -> String {
    if n == 1 {
        format!("1 backup of {book} isn't password-protected.")
    } else {
        format!("{n} backups of {book} aren't password-protected.")
    }
}

/// A password was just added to the open book: ask about its earlier plain backups.
pub(crate) fn after_password_added(app: &mut App) {
    app.backups.list = None;
    let n = unprotected_count(app);
    app.backups.unprotected_ask = (n > 0).then_some(n);
}

/// Protect them now or Delete them (`book.protect_backups`,
/// `book.delete_unprotected_backups`).
fn handle_unprotected(app: &mut App, protect: bool) {
    let cmd = if protect {
        "book.protect_backups"
    } else {
        "book.delete_unprotected_backups"
    };
    let r = app.backend.invoke(cmd, serde_json::json!({}));
    app.backups.list = None;
    match r {
        Ok(v) => {
            let done = v.at("done").as_u64().unwrap_or(0);
            let failed = v.at("failed").as_array().cloned().unwrap_or_default();
            let what = if done == 1 { "backup" } else { "backups" };
            let mut text = if protect {
                format!("Protected {done} earlier {what} with this book's password.")
            } else {
                format!("Deleted {done} earlier {what} that weren't password-protected.")
            };
            if let Some(first) = failed.first() {
                text.push_str(&format!(
                    " {} couldn't be changed: {}",
                    failed.len(),
                    first.at("reason").as_str().unwrap_or_default()
                ));
            }
            app.notify(text);
        }
        Err(e) => app.notify_error(&e),
    }
}

/// The question after a password was added (security-review-1 B2).
fn unprotected_dialog(app: &mut App, ui: &mut Ui) {
    let Some(n) = app.backups.unprotected_ask else {
        return;
    };
    let mut answer: Option<&'static str> = None;
    if ui.dialog_closed("unprotected-backups") || ui.key("escape") {
        answer = Some(KEEP_THEM);
    }
    ui.dialog("unprotected-backups", "Earlier backups", 520, |ui| {
        ui.group(|ui| {
            ui.label(unprotected_question(n));
            ui.secondary(
                "Protect them now writes each one again with the new password and then deletes the unprotected copy.",
            );
        });
        ui.actions(|ui| {
            if ui.button(KEEP_THEM).clicked() {
                answer = Some(KEEP_THEM);
            }
            if ui.button(DELETE_THEM).clicked() {
                answer = Some(DELETE_THEM);
            }
            ui.spacer();
            if ui.primary(PROTECT_THEM).clicked() {
                answer = Some(PROTECT_THEM);
            }
        });
    });
    let Some(a) = answer else { return };
    app.backups.unprotected_ask = None;
    match a {
        PROTECT_THEM => handle_unprotected(app, true),
        DELETE_THEM => handle_unprotected(app, false),
        // Keep them: the Home note stays until they are protected or deleted.
        _ => {}
    }
}

/// One Home → Attention notice.
#[derive(Debug, Clone)]
pub(crate) struct Notice {
    pub key: &'static str,
    pub text: String,
    pub buttons: Vec<&'static str>,
}

/// The backup notices for the open book.
pub(crate) fn notices(app: &mut App) -> Vec<Notice> {
    let mut out = Vec::new();
    if app.screen != Screen::Main {
        return out;
    }
    let prefs = app.config.backup.clone();
    if let Some(e) = app.backups.failure.clone() {
        if e.code == BLOCKED {
            out.push(Notice {
                key: "blocked",
                text: e.message.clone(),
                buttons: vec![CHOOSE_ANOTHER, HOW_TO_ALLOW],
            });
        } else if e.code == "backup_folder" {
            out.push(Notice {
                key: "folder",
                text: format!("MoneyKeep {}.", e.message.trim_end_matches('.')),
                buttons: vec!["Choose backup folder…"],
            });
        } else {
            out.push(Notice {
                key: "failed",
                text: format!("Last backup failed: {}", e.message),
                buttons: vec!["Back up now"],
            });
        }
    }
    let second = prefs.second_place.clone().filter(|s| !s.trim().is_empty());
    if let Some(place) = &second
        && let Some(reason) = app.backups.second_failure.clone()
    {
        out.push(Notice {
            key: "second-failed",
            text: format!("The copy to your second backup place ({place}) failed: {reason}"),
            buttons: vec!["Choose…"],
        });
    }
    if let Some(place) = &second
        && let Some((ok, at)) = app.config.backup_memo.second_ok.clone()
        && now(app) - ok > 7 * DAY
    {
        out.push(Notice {
            key: "second",
            text: format!(
                "Your second backup place ({place}) hasn't been reachable since {}.",
                day_text(&at)
            ),
            buttons: vec!["Choose a second place…"],
        });
    }
    let memo = &app.config.backup_memo;
    if second.is_none()
        && prefs.suggest_second_every > 0
        && memo.made > 0
        && memo.made >= memo.suggest_from + u64::from(prefs.suggest_second_every)
    {
        out.push(Notice {
            key: "suggest",
            text: SUGGESTION.to_owned(),
            buttons: vec!["Choose a second place…", "Not now", "Don't ask again"],
        });
    }
    // Earlier backups that aren't password-protected (security-review-1 B2): only
    // for a protected book, and not while the question is still asked.
    if app.backups.unprotected_ask.is_none() {
        let n = unprotected_count(app);
        if n > 0 && app.backend.protection().0 {
            let book = app
                .book_name()
                .or_else(|| app.security.book_path.as_deref().map(stem_of))
                .unwrap_or_else(|| "this book".to_owned());
            out.push(Notice {
                key: "unprotected",
                text: unprotected_note(n, &book),
                buttons: vec![PROTECT_THEM, DELETE_THEM],
            });
        }
    }
    if prefs.warn_days > 0
        && app.backups.running == 0
        && let Some(rows) = listed(app)
    {
        let newest = rows.first().and_then(|r| r.at("unix").as_i64());
        let now = now(app);
        match newest {
            Some(t) if now - t > i64::from(prefs.warn_days) * DAY => out.push(Notice {
                key: "old",
                text: format!("No backup for {} days.", (now - t) / DAY),
                buttons: vec!["Back up now"],
            }),
            None if app.backups.last_ok.is_none() => out.push(Notice {
                key: "old",
                text: "This book has no backup yet.".to_owned(),
                buttons: vec!["Back up now"],
            }),
            _ => {}
        }
    }
    out
}

/// Show the backup notices in the Attention block; returns whether there were any.
pub(crate) fn attention(app: &mut App, ui: &mut Ui) -> bool {
    let list = notices(app);
    let mut act: Option<(&'static str, &'static str)> = None;
    for n in &list {
        ui.scope(format!("backup-{}", n.key), |ui| {
            // Wrapped text (a folder path can be long), then the buttons.
            ui.label(&n.text);
            // The buttons on their own line, so a long notice keeps them visible.
            ui.row(|ui| {
                for b in &n.buttons {
                    ui.named(format!("{b}: {}", n.text));
                    if ui.button(*b).clicked() {
                        act = Some((n.key, b));
                    }
                }
            });
        });
    }
    if let Some((key, button)) = act {
        notice_action(app, key, button);
    }
    !list.is_empty()
}

fn notice_action(app: &mut App, key: &str, button: &str) {
    match button {
        PROTECT_THEM => handle_unprotected(app, true),
        DELETE_THEM => handle_unprotected(app, false),
        "Back up now" => start(app, "manual"),
        "Choose backup folder…" => app.emit(AppEvent::ChooseBackupFolder { second: false }),
        "Choose a second place…" | "Choose…" => {
            app.emit(AppEvent::ChooseBackupFolder { second: true });
        }
        CHOOSE_ANOTHER => app.emit(AppEvent::ChooseBackupFolder { second: false }),
        HOW_TO_ALLOW => app.backups.help = Some(Some(HELP_BLOCKED)),
        "Not now" => {
            app.config.backup_memo.suggest_from = app.config.backup_memo.made;
            app.emit(AppEvent::ConfigChanged);
        }
        "Don't ask again" => {
            app.config.backup.suggest_second_every = 0;
            settings_changed(app);
            app.notify("You won't be asked again. You can turn the suggestion back on in Settings → Backups.");
        }
        _ => {
            let _ = key;
            open_settings(app);
        }
    }
}

fn open_settings(app: &mut App) {
    app.config.settings_section = SECTION.to_owned();
    app.emit(AppEvent::ConfigChanged);
    app.navigate(View::Settings);
}

/// The note at the top of a restored copy's Home.
pub(crate) fn home_note(app: &mut App, ui: &mut Ui) {
    let Some(book) = app.security.book_path.clone() else {
        return;
    };
    let Some((_, at)) = app
        .config
        .backup_memo
        .restored
        .iter()
        .find(|(p, _)| *p == book)
        .cloned()
    else {
        return;
    };
    let text = if at.is_empty() {
        "This is a copy restored from a backup. Your original book was not changed.".to_owned()
    } else {
        format!(
            "This is a copy restored from the backup of {}. Your original book was not changed.",
            when_text(&at)
        )
    };
    let mut dismiss = false;
    ui.scope("restored-note", |ui| {
        ui.banner(Tone::Accent, |ui| {
            ui.next().grow();
            ui.label(&text);
            ui.tip("Dismiss");
            dismiss = ui.icon_button("close", "Dismiss", false).clicked();
        });
    });
    if dismiss {
        app.config.backup_memo.restored.retain(|(p, _)| *p != book);
        app.emit(AppEvent::ConfigChanged);
    }
}

// ----- opening stopped by a failed before-update backup ----------------------------------

/// Opening `path` failed with `backup_failed`: the start screen shows the message
/// with Details, Try again and Choose backup folder….
pub(crate) fn open_failed(app: &mut App, path: &str, secret: Option<UiSecret>, e: &UiError) {
    if app.screen == Screen::Main {
        app.close_book();
    }
    app.backups.open_failed = Some(OpenFailed {
        path: path.to_owned(),
        secret,
    });
    app.message = Some((e.message.clone(), true));
    app.message_action = None;
    app.message_details = Some(
        e.message
            .split_once(": ")
            .map_or_else(|| e.message.clone(), |(_, r)| r.to_owned()),
    );
    app.message_since = app.now;
}

/// Try again and Choose backup folder… in the start screen's message.
pub(crate) fn open_failed_actions(app: &mut App, ui: &mut Ui) {
    if app.backups.open_failed.is_none() {
        return;
    }
    if ui.button("Try again").clicked() {
        retry_open(app);
    }
    if ui.button("Choose backup folder…").clicked() {
        app.emit(AppEvent::ChooseBackupFolder { second: false });
    }
    let blocked = app
        .message
        .as_ref()
        .is_some_and(|(m, _)| m.contains("Controlled folder access"));
    if blocked && ui.button(HOW_TO_ALLOW).clicked() {
        app.backups.help = Some(Some(HELP_BLOCKED));
    }
}

fn retry_open(app: &mut App) {
    let Some(f) = app.backups.open_failed.take() else {
        return;
    };
    app.clear_message();
    let r = match &f.secret {
        Some(s) => app.backend.open_book_with(&f.path, s),
        None => app.backend.open_book(&f.path),
    };
    match r {
        Ok(()) => app.opened(&f.path),
        Err(e) if e.code == "backup_failed" => open_failed(app, &f.path, f.secret, &e),
        Err(e) => app.notify_error(&e),
    }
}

// ----- Settings → Backups ------------------------------------------------------------------

/// The number settings: (label, value).
fn numbers(p: &crate::BackupPrefs) -> [(&'static str, u32); 6] {
    [
        ("Keep the most recent", p.keep_recent),
        ("One a day for (days)", p.keep_days),
        ("One a month for (months)", p.keep_months),
        ("Before-update, manual (each)", p.keep_per_reason),
        ("Suggest a second place every", p.suggest_second_every),
        ("Warn after days without one", p.warn_days),
    ]
}

fn set_number(p: &mut crate::BackupPrefs, i: usize, n: u32) {
    match i {
        0 => p.keep_recent = n,
        1 => p.keep_days = n,
        2 => p.keep_months = n,
        3 => p.keep_per_reason = n,
        4 => p.suggest_second_every = n,
        _ => p.warn_days = n,
    }
}

fn folder_shown(app: &App) -> String {
    match app
        .config
        .backup
        .folder
        .clone()
        .filter(|f| !f.trim().is_empty())
    {
        Some(f) => f,
        None => app
            .backend
            .default_backup_folder()
            .unwrap_or_else(|| "MoneyKeep Backups in your user folder".to_owned()),
    }
}

/// The last backup of the open book: (local time, place).
fn last_backup(app: &mut App) -> Option<(String, String)> {
    if let Some(v) = app.backups.last_ok.clone() {
        let place = if v.str("second_place_path").is_empty() {
            "the backup folder"
        } else {
            "the backup folder and the second place"
        };
        return Some((v.str("at").to_owned(), place.to_owned()));
    }
    let rows = listed(app)?;
    let r = rows.first()?;
    let place = if r.str("place") == "second_place" {
        "the second place"
    } else {
        "the backup folder"
    };
    Some((r.str("at").to_owned(), place.to_owned()))
}

pub(crate) fn settings_section(app: &mut App, ui: &mut Ui) {
    let folder = folder_shown(app);
    ui.card("Backup folder", |ui| {
        ui.row(|ui| {
            ui.next().grow();
            let default = app
                .config
                .backup
                .folder
                .as_deref()
                .is_none_or(|f| f.trim().is_empty());
            ui.label(if default {
                format!("{folder} (default)")
            } else {
                folder.clone()
            });
            ui.named("Choose backup folder");
            if ui.button("Choose…").clicked() {
                app.emit(AppEvent::ChooseBackupFolder { second: false });
            }
            ui.named("Open backup folder");
            if ui.button("Open folder").clicked() {
                app.emit(AppEvent::OpenFolder {
                    path: folder.clone(),
                });
            }
        });
        if app.security.book_path.is_some() {
            match last_backup(app) {
                Some((at, place)) => ui.secondary(format!(
                    "Last backup of this book: {} in {place}.",
                    when_text(&at)
                )),
                None => ui.secondary("This book has no backup yet."),
            }
        }
        if let Some(e) = app.backups.failure.clone() {
            ui.error_text(format!("Last backup failed: {}", e.message));
            if e.code == BLOCKED {
                ui.row(|ui| {
                    if ui.button(CHOOSE_ANOTHER).clicked() {
                        app.emit(AppEvent::ChooseBackupFolder { second: false });
                    }
                    if ui.button(HOW_TO_ALLOW).clicked() {
                        app.backups.help = Some(Some(HELP_BLOCKED));
                    }
                });
            }
        }
        ui.row(|ui| {
            ui.enabled(app.security.book_path.is_some() && app.backups.running == 0);
            if ui.primary("Back up now").clicked() {
                start(app, "manual");
            }
            if ui.button("Restore a backup…").clicked() {
                open_restore(app, false);
            }
            if app.backups.running > 0 {
                ui.progress("Backing up", -1);
                ui.label(BACKING_UP);
            }
        });
    });
    ui.card("Second place", |ui| {
        let second = app.config.backup.second_place.clone().filter(|s| !s.trim().is_empty());
        ui.row(|ui| {
            ui.next().grow();
            ui.label(second.clone().unwrap_or_else(|| "Not set".to_owned()));
            ui.named("Choose second place");
            if ui.button("Choose…").clicked() {
                app.emit(AppEvent::ChooseBackupFolder { second: true });
            }
            ui.enabled(second.is_some());
            ui.named("Clear second place");
            if ui.button("Clear").clicked() {
                app.config.backup.second_place = None;
                app.config.backup_memo.second_ok = None;
                app.backups.second_failure = None;
                settings_changed(app);
            }
            ui.enabled(second.is_some());
            ui.named("Open second place");
            if ui.button("Open folder").clicked()
                && let Some(p) = second.clone()
            {
                app.emit(AppEvent::OpenFolder { path: p });
            }
        });
        if second.is_some()
            && let Some(reason) = app.backups.second_failure.clone()
        {
            ui.error_text(format!("The last copy to the second place failed: {reason}"));
        }
        ui.secondary(
            "After each backup, the same file is copied here too — a USB drive or a cloud folder (OneDrive, Dropbox, Google Drive). When it isn't reachable, the copy is skipped and tried again with the next backup.",
        );
    });
    ui.card("When to back up", |ui| {
        let mut p = app.config.backup.clone();
        ui.switch(&mut p.on_open, "Back up when opening a book");
        ui.switch(&mut p.on_close, "Back up when closing a book (if anything changed)");
        ui.switch(
            &mut p.daily,
            "Back up daily while a book stays open (if anything changed)",
        );
        ui.secondary(
            "A backup is always made before MoneyKeep updates a book to a new format; that one can't be switched off.",
        );
        if p != app.config.backup {
            app.config.backup = p;
            settings_changed(app);
        }
    });
    ui.card("How many to keep", |ui| {
        let current = numbers(&app.config.backup);
        let mut drafts = app
            .backups
            .drafts
            .take()
            .unwrap_or_else(|| current.iter().map(|(_, n)| n.to_string()).collect());
        let mut commit: Option<(usize, String)> = None;
        for (i, (label, _)) in current.iter().enumerate() {
            ui.form(label, |ui| {
                ui.next().width(80);
                if let Some(d) = drafts.get_mut(i) {
                    let resp = ui.field(label, d);
                    if resp.blurred || resp.submitted {
                        commit = Some((i, d.clone()));
                    }
                }
            });
        }
        ui.secondary(
            "Per book, in each place: the most recent backups, the newest of each recent day and month, and the before-update and manual backups up to their own count. Only backup files MoneyKeep made are ever removed. The suggestion counts backups; 0 turns the suggestion or the warning off.",
        );
        app.backups.drafts = Some(drafts);
        if let Some((i, text)) = commit {
            match text.trim().parse::<u32>() {
                Ok(n) if n <= 1000 && (i != 0 || n >= 1) => {
                    let mut p = app.config.backup.clone();
                    set_number(&mut p, i, n);
                    if p != app.config.backup {
                        app.config.backup = p;
                        settings_changed(app);
                    }
                }
                _ => {
                    app.backups.drafts = None;
                    app.notify(if i == 0 {
                        "Keep at least 1 recent backup (a whole number up to 1000)."
                    } else {
                        "Use a whole number from 0 to 1000."
                    });
                }
            }
        }
    });
    ui.card("About backups", |ui| {
        ui.label("A backup is the whole book, including the attachments stored in it (receipts, pictures).");
        ui.label("Files outside the book and the app's own settings are not backed up.");
        ui.label("Backups of a password-protected book stay protected: they need the book's password from when the backup was made.");
        ui.label("Restoring never replaces a book: the backup opens as a separate copy.");
        ui.row(|ui| {
            ui.named("Backups help");
            if ui.button("Help").clicked() {
                app.backups.help = Some(None);
            }
        });
    });
}

// ----- the guide and a refused folder (revision 3) ---------------------------------------

fn rank(kind: &str) -> u8 {
    match kind {
        "h1" => 1,
        "h2" => 2,
        "h3" => 3,
        _ => u8::MAX,
    }
}

/// The guide's lines, from the start or from the section headed `anchor` to the next
/// heading of the same or a higher level.
pub(crate) fn help_section(anchor: Option<&str>) -> Vec<(&'static str, String)> {
    let all = super::capture_wifi::help_lines(HELP);
    let Some(a) = anchor else {
        return all;
    };
    let Some(start) = all.iter().position(|(k, t)| rank(k) != u8::MAX && t == a) else {
        return all;
    };
    let level = all.get(start).map_or(u8::MAX, |(k, _)| rank(k));
    all.into_iter()
        .enumerate()
        .skip(start)
        .take_while(|(i, (k, _))| *i == start || rank(k) > level)
        .map(|(_, l)| l)
        .collect()
}

fn help_dialog(app: &mut App, ui: &mut Ui) {
    let Some(anchor) = app.backups.help else {
        return;
    };
    let mut close = ui.dialog_closed("backups-help") || ui.key("escape");
    let mut whole = false;
    ui.dialog("backups-help", "Backups help", 820, |ui| {
        for (kind, text) in help_section(anchor) {
            match kind {
                "h1" => ui.big_heading(text),
                "h2" => ui.heading(text),
                "h3" => {
                    ui.bold();
                    ui.label(text);
                }
                "item" => ui.label(format!("  {text}")),
                _ => ui.label(text),
            }
        }
        ui.actions(|ui| {
            if anchor.is_some() && ui.button("Show the whole guide").clicked() {
                whole = true;
            }
            ui.spacer();
            if ui.primary("Close help").clicked() {
                close = true;
            }
        });
    });
    if close {
        app.backups.help = None;
    } else if whole {
        app.backups.help = Some(None);
    }
}

/// A chosen folder could not be written: it was not saved; why, with Choose another
/// folder… and, when Windows blocked it, How to allow MoneyKeep.
fn refused_dialog(app: &mut App, ui: &mut Ui) {
    let Some(r) = app.backups.refused.clone() else {
        return;
    };
    let mut close = ui.dialog_closed("folder-refused") || ui.key("escape");
    let mut choose = false;
    let mut how = false;
    let blocked = r.error.code == BLOCKED;
    ui.dialog("folder-refused", "Can't use this folder", 600, |ui| {
        ui.group(|ui| {
            ui.error_text(r.error.display());
            ui.label(if r.second {
                "The second place was not changed."
            } else {
                "The backup folder was not changed."
            });
            ui.secondary(r.folder.clone());
        });
        ui.actions(|ui| {
            if ui.button(CHOOSE_ANOTHER).clicked() {
                choose = true;
            }
            if blocked && ui.button(HOW_TO_ALLOW).clicked() {
                how = true;
            }
            ui.spacer();
            if ui.primary("Close").clicked() {
                close = true;
            }
        });
    });
    if choose {
        app.backups.refused = None;
        app.emit(AppEvent::ChooseBackupFolder { second: r.second });
    } else if how {
        app.backups.help = Some(Some(HELP_BLOCKED));
    } else if close {
        app.backups.refused = None;
    }
}

// ----- Restore a backup… -----------------------------------------------------------------

/// Open Restore a backup… (`from_start`: pick the book first).
pub(crate) fn open_restore(app: &mut App, from_start: bool) {
    let from_start = from_start || app.security.book_path.is_none();
    app.backups.restore = Some(Restore {
        from_start,
        ..Restore::default()
    });
}

/// The book whose backups are listed.
fn restore_book(app: &App) -> Option<String> {
    let r = app.backups.restore.as_ref()?;
    if r.from_start {
        app.config.recent.get(r.book).map(|b| b.path.clone())
    } else {
        app.security.book_path.clone()
    }
}

fn book_label(app: &App, path: &str) -> String {
    app.config
        .recent
        .iter()
        .find(|b| b.path == path)
        .and_then(|b| b.tile.as_ref().map(|t| t.name.clone()))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| stem_of(path))
}

// ----- Back up this book… (start-screen card) ---------------------------------------------

/// Card … → Back up this book…: a `manual` backup of a book that is not open, without
/// opening it. A password-protected book asks for its password first.
pub(crate) fn back_up_card(app: &mut App, path: &str, name: &str, locked: bool) {
    if locked {
        app.backups.card = Some(CardBackup {
            path: path.to_owned(),
            name: name.to_owned(),
            password: String::new(),
            error: None,
        });
        return;
    }
    // Errors are shown by the start screen's message.
    let _ = back_up_file(app, path, name, None);
}

/// Back up the closed book; on success the status message "Backed up <book> 2:20 PM".
fn back_up_file(
    app: &mut App,
    path: &str,
    name: &str,
    secret: Option<&UiSecret>,
) -> Result<(), UiError> {
    match app.backend.backup_book_file(path, secret) {
        Ok(v) => {
            app.config.backup_memo.made += 1;
            let second = note_second(app, &v);
            app.emit(AppEvent::ConfigChanged);
            match second {
                Some(reason) => app.notify_error(&UiError::new(
                    "second_place",
                    format!("Backed up {name}. The copy to the second place failed: {reason}"),
                )),
                None => app.notify(format!("Backed up {name} {}", time_text(v.str("at")))),
            }
            Ok(())
        }
        Err(e) => {
            if !matches!(e.code.as_str(), "password_required" | "wrong_password") {
                app.notify_error(&UiError::new(
                    &e.code,
                    format!("Couldn't back up {name}: {}", e.display()),
                ));
            }
            Err(e)
        }
    }
}

/// The password dialog of Back up this book… for a protected book.
fn card_dialog(app: &mut App, ui: &mut Ui) {
    if app.backups.card.is_none() {
        return;
    }
    if ui.dialog_closed("back-up-book") || ui.key("escape") {
        app.backups.card = None;
        return;
    }
    let Some(mut c) = app.backups.card.take() else {
        return;
    };
    let mut go = false;
    let mut cancel = false;
    ui.dialog("back-up-book", "Back up this book", 520, |ui| {
        ui.group(|ui| {
            ui.label(format!(
                "{} is password protected. Enter its password to back it up; the backup stays protected with the same password.",
                c.name
            ));
            ui.form("Password", |ui| {
                ui.next().width(320);
                if ui.password("Book password", &mut c.password).submitted {
                    go = true;
                }
            });
        });
        if let Some(e) = &c.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            ui.spacer();
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            ui.enabled(!c.password.is_empty());
            if ui.primary("Back up").clicked() {
                go = true;
            }
        });
    });
    if cancel {
        return;
    }
    if go && !c.password.is_empty() {
        let secret = UiSecret::Password(std::mem::take(&mut c.password));
        match back_up_file(app, &c.path.clone(), &c.name.clone(), Some(&secret)) {
            Ok(()) => return,
            Err(e) if e.code == "wrong_password" || e.code == "password_required" => {
                c.error = Some("Wrong password.".to_owned());
            }
            // Shown as the start screen's message.
            Err(_) => return,
        }
    }
    app.backups.card = Some(c);
}

pub(crate) fn dialogs(app: &mut App, ui: &mut Ui) {
    // After the recovery key is shown and saved.
    if app.backups.unprotected_ask.is_some() && app.security.recovery_key.is_none() {
        unprotected_dialog(app, ui);
        return;
    }
    if app.backups.help.is_some() {
        help_dialog(app, ui);
        return;
    }
    if app.backups.refused.is_some() {
        refused_dialog(app, ui);
        return;
    }
    card_dialog(app, ui);
    if app.backups.restore.is_none() {
        return;
    }
    if ui.dialog_closed("restore") || ui.key("escape") {
        app.backups.restore = None;
        return;
    }
    let book = restore_book(app);
    if let Some(b) = book.clone() {
        let stale = app
            .backups
            .restore
            .as_ref()
            .is_some_and(|r| r.rows.as_ref().is_none_or(|(p, _)| *p != b));
        if stale {
            let rows = app
                .backend
                .backups_list(&b)
                .map(|v| v.as_array().cloned().unwrap_or_default())
                .map_err(|e| e.display());
            if let Some(r) = app.backups.restore.as_mut() {
                r.rows = Some((b, rows));
                r.selected = None;
            }
        }
    }
    let Some(mut r) = app.backups.restore.take() else {
        return;
    };
    let names: Vec<String> = app
        .config
        .recent
        .iter()
        .map(|b| book_label(app, &b.path))
        .collect();
    let mut browse = false;
    let mut chosen: Option<(String, String)> = None;
    let mut cont = false;
    let mut cancel = false;
    ui.dialog("restore", "Restore a backup", 760, |ui| {
        ui.group(|ui| {
            if r.from_start {
                if names.is_empty() {
                    ui.label("No books in the list yet: browse to a backup file.");
                } else {
                    let mut i = r.book.min(names.len().saturating_sub(1));
                    ui.form("Book", |ui| {
                        ui.next().width(320);
                        if ui.combo("Book", &mut i, &names).changed() {
                            r.book = i;
                            r.rows = None;
                            r.selected = None;
                        }
                    });
                }
            } else if let Some(b) = &book {
                ui.label(format!("Backups of {}", book_label(app, b)));
            }
            ui.secondary("Restoring opens the backup as a separate copy; your book is not changed.");
        });
        let rows = r.rows.as_ref().map(|(_, rows)| rows.clone());
        match rows {
            Some(Err(e)) => ui.error_text(e),
            Some(Ok(rows)) if rows.is_empty() => ui.label("No backups of this book were found."),
            Some(Ok(rows)) => {
                let cols: Vec<Column> = vec![
                    Column {
                        width: 220,
                        ..col("Date and time")
                    },
                    Column {
                        width: 140,
                        ..col("Reason")
                    },
                    Column {
                        width: 110,
                        ..col_num("Size")
                    },
                    col("Place"),
                ];
                ui.table("Backups", cols, |ui| {
                    ui.visible_rows(8);
                    for (i, row) in rows.iter().enumerate() {
                        let selected = r.selected == Some(i);
                        let place = if row.str("place") == "second_place" {
                            "Second place"
                        } else {
                            "Backup folder"
                        };
                        let when = when_text(row.str("at"));
                        let reason = reason_text(row.str("reason"));
                        let key = format!("Backup of {when}, {reason}, {place} ({})", i + 1);
                        let resp = ui.table_row(&key, selected, true, |ui| {
                            ui.label(when.clone());
                            ui.label(reason);
                            ui.label(size_text(row.at("size").as_u64().unwrap_or(0)));
                            ui.label(place);
                        });
                        if resp.clicked() {
                            r.selected = Some(i);
                            r.asking = false;
                            r.error = None;
                        }
                    }
                });
            }
            None => {}
        }
        if r.asking {
            ui.group(|ui| {
                ui.label("This backup is password protected. Enter the password the book had when the backup was made.");
                ui.form("Password", |ui| {
                    ui.next().width(320);
                    if ui.password("Backup password", &mut r.password).submitted {
                        cont = true;
                    }
                });
            });
        }
        if let Some(e) = &r.error {
            ui.error_text(e.clone());
        }
        ui.actions(|ui| {
            if ui.button("Browse for a backup file…").clicked() {
                browse = true;
            }
            ui.spacer();
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
            if r.asking {
                ui.enabled(!r.password.is_empty());
                if ui.primary("Continue").clicked() {
                    cont = true;
                }
            } else {
                let row = match (&r.rows, r.selected) {
                    (Some((_, Ok(rows))), Some(i)) => rows.get(i).cloned(),
                    _ => None,
                };
                ui.enabled(row.is_some());
                if ui.primary("Open as a copy").clicked()
                    && let Some(row) = row
                {
                    chosen = Some((row.str("path").to_owned(), row.str("at").to_owned()));
                }
            }
        });
    });
    if cancel {
        return;
    }
    app.backups.restore = Some(r);
    if browse {
        app.emit(AppEvent::ChooseBackupFile);
    } else if let Some((path, at)) = chosen {
        choose_target(app, &path, &at, book.as_deref());
    } else if cont && let Some(t) = app.backups.restore.as_ref().and_then(|r| r.target.clone()) {
        app.restore_to(&t);
    }
}

/// Ask where to save the copy: the book's folder, `<book> restored <time>`.
fn choose_target(app: &mut App, backup: &str, at: &str, book: Option<&str>) {
    let stem = book
        .map(stem_of)
        .or_else(|| parse_backup_name(backup).map(|(s, _)| s))
        .unwrap_or_else(|| stem_of(backup));
    let dir = book.and_then(folder_of);
    if let Some(r) = app.backups.restore.as_mut() {
        r.backup = Some((backup.to_owned(), at.to_owned()));
        r.target = None;
        r.asking = false;
        r.password.clear();
        r.error = None;
    }
    app.emit(AppEvent::ChooseRestoreTarget {
        dir,
        file_name: restored_name(&stem, at),
    });
}

impl App {
    /// The folder picker for Settings → Backups returned `path`.
    /// The folder is first tested with a small file (backups.md revision 3); one
    /// that can't be written is not saved and the reason is shown.
    pub fn set_backup_folder(&mut self, path: &str, second: bool) {
        if let Err(error) = self.backend.test_backup_folder(path) {
            self.backups.refused = Some(Refused {
                second,
                folder: path.to_owned(),
                error,
            });
            return;
        }
        self.backups.refused = None;
        if second {
            self.backups.second_failure = None;
            self.config.backup.second_place = Some(path.to_owned());
            // The 7-day clock of "not reachable" starts now.
            let at = local_of(self, now(self));
            self.config.backup_memo.second_ok = Some((now(self), at));
        } else {
            self.config.backup.folder = Some(path.to_owned());
            if self
                .backups
                .failure
                .as_ref()
                .is_some_and(|e| e.code == "backup_folder" || e.code == BLOCKED)
            {
                self.backups.failure = None;
            }
        }
        settings_changed(self);
        if !second && self.backups.open_failed.is_some() {
            retry_open(self);
        }
    }

    /// Restore a backup… (review aid and tests): `from_start` picks the book first.
    pub fn open_restore_dialog(&mut self, from_start: bool) {
        open_restore(self, from_start);
    }

    /// The "open file" dialog for a backup returned `path`: ask where to save the copy.
    pub fn restore_from_file(&mut self, path: &str) {
        if self.backups.restore.is_none() {
            open_restore(self, self.screen == Screen::Start);
        }
        let at = parse_backup_name(path)
            .map(|(_, at)| at)
            .unwrap_or_default();
        choose_target(self, path, &at, None);
    }

    /// The save dialog for the restored copy returned `target`: copy, check and open
    /// it with its note. An encrypted backup asks for its password first.
    pub fn restore_to(&mut self, target: &str) {
        let Some(r) = self.backups.restore.as_mut() else {
            return;
        };
        let Some((backup, at)) = r.backup.clone() else {
            return;
        };
        r.target = Some(target.to_owned());
        let secret = (r.asking && !r.password.is_empty())
            .then(|| UiSecret::Password(std::mem::take(&mut r.password)));
        let copied = self.backend.restore_copy(&backup, target, secret.as_ref());
        let v = match copied {
            Ok(v) => v,
            Err(e) => {
                if let Some(r) = self.backups.restore.as_mut() {
                    match e.code.as_str() {
                        "password_required" => {
                            r.asking = true;
                            r.error = None;
                        }
                        "wrong_password" => {
                            r.asking = true;
                            r.error = Some("Wrong password.".to_owned());
                        }
                        _ => {
                            r.error = Some(e.display());
                            r.target = None;
                        }
                    }
                }
                return;
            }
        };
        let path = v.str("path").to_owned();
        let path = if path.is_empty() {
            target.to_owned()
        } else {
            path
        };
        self.backups.restore = None;
        if self.screen == Screen::Main {
            self.close_book();
        }
        self.config.backup_memo.restored.retain(|(p, _)| *p != path);
        self.config.backup_memo.restored.push((path.clone(), at));
        self.emit(AppEvent::ConfigChanged);
        let opened = match (&secret, v.at("encrypted").as_bool() == Some(true)) {
            (Some(s), true) => self.backend.open_book_with(&path, s),
            (None, true) => {
                self.prompt_unlock(&path);
                return;
            }
            _ => self.backend.open_book(&path),
        };
        match opened {
            Ok(()) => self.opened(&path),
            Err(e) => self.notify_error(&e),
        }
    }

    /// Tests: use `unix` seconds as the backups' clock.
    pub fn set_backup_clock_for_tests(&mut self, unix: i64) {
        self.backups.clock = Some(unix);
    }

    /// The app is quitting: the closing backup, then wait for running backups.
    pub fn shutdown(&mut self) {
        if self.screen == Screen::Main {
            self.close_book();
        }
        self.backend.finish_backups();
        for v in self.backend.backup_poll().into_iter().flatten() {
            self.config.backup_memo.made += 1;
            let _ = note_second(self, &v);
        }
    }

    /// Settings → Backups → Help (review aid and tests): the guide from its start,
    /// or at `section`.
    pub fn open_backups_help(&mut self, section: Option<&'static str>) {
        self.backups.help = Some(section);
    }

    /// The Home Attention backup notices with their buttons (tests and review).
    pub fn backup_notice_buttons(&mut self) -> Vec<(String, Vec<String>)> {
        notices(self)
            .into_iter()
            .map(|n| (n.text, n.buttons.iter().map(|b| (*b).to_owned()).collect()))
            .collect()
    }

    /// Ask the question about the open book's earlier plain backups now, as after a
    /// password is added (screen reviews).
    pub fn review_unprotected_question(&mut self) {
        after_password_added(self);
    }

    /// Open About MoneyKeep (screen reviews).
    pub fn open_about_for_review(&mut self) {
        super::about::open(self);
    }

    /// The question about earlier plain backups, when it is asked (tests and review).
    pub fn unprotected_backups_question(&self) -> Option<String> {
        self.backups.unprotected_ask.map(unprotected_question)
    }

    /// The Home Attention backup notices (tests and review): their texts.
    pub fn backup_notices(&mut self) -> Vec<String> {
        notices(self).into_iter().map(|n| n.text).collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn texts() {
        assert_eq!(when_text("2026-11-07T14:20"), "Nov 7, 2026 2:20 PM");
        assert_eq!(time_text("2026-11-07T00:05"), "12:05 AM");
        assert_eq!(day_text("2026-11-01T09:00"), "Nov 1");
        assert_eq!(size_text(2_621_440), "2.5 MB");
        assert_eq!(
            parse_backup_name("/b/Household/Household 2026-11-07 1420 opened 2.moneykeep"),
            Some(("Household".into(), "2026-11-07T14:20".into()))
        );
        assert_eq!(parse_backup_name("/b/notes.moneykeep"), None);
        assert_eq!(
            restored_name("Household", "2026-11-07T14:20"),
            "Household restored 2026-11-07 1420.moneykeep"
        );
        assert_eq!(folder_of("/a/b/c.moneykeep").as_deref(), Some("/a/b"));
        assert_eq!(seconds_of("1970-01-02T00:01"), Some(86_460));
    }

    /// backups.md revision 3: How to allow MoneyKeep opens the guide at its section.
    #[test]
    fn the_guide_opens_at_why_not_documents() {
        let lines = help_section(Some(HELP_BLOCKED));
        assert_eq!(lines.first().map(|(_, t)| t.as_str()), Some(HELP_BLOCKED));
        let text: String = lines
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Controlled folder access"), "{text}");
        assert!(text.contains("Allow an app through Controlled folder access"));
        assert!(
            !text.contains("When a backup is made"),
            "stops at the next section"
        );
        assert!(help_section(None).len() > lines.len());
    }
}
