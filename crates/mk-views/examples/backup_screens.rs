//! Renders backups (backups.md test BK11) for review, light and dark:
//! `settings-backups-*` (Settings → Backups), `restore-list-*` (Restore a backup…
//! with the list of a book's backups) and `attention-second-place-*` (Home with the
//! second place's Attention item); backups.md revision 2: `card-backup-menu-*` (the
//! start screen's book card with its … menu open, showing Back up this book…);
//! revision 3: `settings-backups-*` is tall enough to show the Help button, and
//! `backups-help-*` shows the guide it opens. Run under a display (or Xvfb) with
//! `SLINT_BACKEND=winit-software cargo run -p mk-views --example backup_screens -- <dir>`.
#![allow(
    clippy::unwrap_used,
    clippy::print_stdout,
    clippy::float_arithmetic,
    clippy::type_complexity,
    clippy::indexing_slicing
)]

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use mk_commands::Session;
use mk_commands::backups::{self, BackupEnv, BackupSettings, Reason};
use mk_ui::ctl::App;
use mk_ui::{Backend, BackupPrefs, UiError, UserConfig};
use mk_views::Views;
use serde_json::{Value, json};
use slint::ComponentHandle;

/// Nov 7, 2026 2:20 PM UTC.
const NOW: i64 = 1_794_061_200;

struct Local {
    session: Option<Session>,
    env: Option<BackupEnv>,
}

impl Backend for Local {
    fn invoke(&mut self, command: &str, args: Value) -> Result<Value, UiError> {
        match self.session.as_mut() {
            Some(s) => s
                .invoke(command, args)
                .map_err(|e| UiError::new(e.code(), e.to_string())),
            None => Err(UiError::new("no_book", "no book")),
        }
    }
    fn book_name(&self) -> Option<String> {
        self.session.as_ref().map(|_| "Household".to_owned())
    }
    fn create_book(&mut self, _: &str, _: &str, _: &str, _: Option<&str>) -> Result<(), UiError> {
        Ok(())
    }
    fn open_book(&mut self, path: &str) -> Result<(), UiError> {
        self.session = Some(
            Session::open(Path::new(path), "app")
                .map_err(|e| UiError::new(e.code(), e.to_string()))?,
        );
        Ok(())
    }
    fn close_book(&mut self) {
        self.session = None;
    }
    fn set_backup_settings(&mut self, settings: Value) {
        self.env = Some(BackupEnv::new(BackupSettings::from_json(&settings), 0));
    }
    fn backups_list(&mut self, book: &str) -> Result<Value, UiError> {
        let env = self
            .env
            .clone()
            .ok_or_else(|| UiError::new("unsupported", ""))?;
        Ok(Value::Array(
            backups::list(Path::new(book), &env)
                .iter()
                .map(|e| e.to_json(&env))
                .collect(),
        ))
    }
}

fn app(dir: &Path, shot: &str, dark: bool) -> App {
    let root = dir.join(format!("{shot}-{dark}"));
    std::fs::create_dir_all(&root).unwrap();
    let book = root.join("Household.moneykeep");
    let mut s = Session::create(&book, "Household", "USD", "app").unwrap();
    s.invoke(
        "account.create",
        json!({"name": "Checking", "type": "asset"}),
    )
    .unwrap();
    drop(s);
    let folder = root.join("MoneyKeep Backups");
    let usb = root.join("USB").join("Backups");
    // Backups made over the last weeks.
    let sub = folder.join("Household");
    std::fs::create_dir_all(&sub).unwrap();
    for (ago_hours, reason) in [
        (0, Reason::Opened),
        (26, Reason::Closed),
        (50, Reason::Daily),
        (74, Reason::Manual),
        (170, Reason::BeforeUpdate),
        (400, Reason::Opened),
        (900, Reason::Closed),
    ] {
        let name = backups::file_name_at("Household", NOW - ago_hours * 3600, 0, reason);
        std::fs::copy(&book, sub.join(name)).unwrap();
    }
    let mut prefs = BackupPrefs {
        folder: Some(folder.to_str().unwrap().to_owned()),
        on_open: false,
        ..BackupPrefs::default()
    };
    let mut memo = mk_ui::BackupMemo::default();
    if shot != "restore-list" {
        // An unplugged USB drive (as in backups.md's example).
        prefs.second_place = Some(if shot == "attention-second-place" {
            "E:\\Backups".to_owned()
        } else {
            usb.to_str().unwrap().to_owned()
        });
        memo.second_ok = Some((NOW - 6 * 86_400 - 5 * 3600, "2026-11-01T09:20".into()));
    }
    let cfg = UserConfig {
        theme: if dark {
            mk_ui::Theme::Dark
        } else {
            mk_ui::Theme::Light
        },
        date_format: "MM-DD-YYYY".into(),
        settings_section: "Backups".into(),
        backup: prefs,
        backup_memo: memo,
        ..UserConfig::default()
    };
    let mut a = App::new(
        Box::new(Local {
            session: None,
            env: None,
        }),
        cfg,
    );
    a.set_today("2026-11-07");
    a.set_backup_clock_for_tests(NOW);
    a.open_book(book.to_str().unwrap());
    match shot {
        "card-backup-menu" => a.close_book(),
        "settings-backups" => a.navigate(mk_ui::View::Settings),
        "backups-help" => {
            a.navigate(mk_ui::View::Settings);
            a.open_backups_help(None);
        }
        "restore-list" => {
            a.navigate(mk_ui::View::Settings);
            a.open_restore_dialog(false);
        }
        _ => {
            // Two more days without reaching the second place.
            a.set_backup_clock_for_tests(NOW + 2 * 86_400);
            a.navigate(mk_ui::View::Home);
        }
    }
    a
}

fn save_png(path: &Path, buf: &slint::SharedPixelBuffer<slint::Rgba8Pixel>) {
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
    let dir = tempfile::tempdir().unwrap();
    let mut jobs = Vec::new();
    for shot in [
        "settings-backups",
        "restore-list",
        "attention-second-place",
        "card-backup-menu",
        "backups-help",
    ] {
        for dark in [false, true] {
            jobs.push((shot, dark));
        }
    }
    let jobs = Rc::new(RefCell::new(jobs.into_iter()));
    let dir_path = dir.path().to_owned();
    let next: Rc<RefCell<Option<Box<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let next2 = next.clone();
    *next.borrow_mut() = Some(Box::new(move || {
        let Some((shot, dark)) = jobs.borrow_mut().next() else {
            let _ = slint::quit_event_loop();
            return;
        };
        let a = app(&dir_path, shot, dark);
        let views = Views::new(a).unwrap();
        let width = if shot == "card-backup-menu" {
            1200.0
        } else {
            1280.0
        };
        let height = if shot == "settings-backups" {
            1450.0
        } else {
            900.0
        };
        views
            .window()
            .window()
            .set_size(slint::LogicalSize::new(width, height));
        views.show().unwrap();
        let out = out.clone();
        let next3 = next2.clone();
        let cur = Rc::new(RefCell::new(Some(views)));
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
                    v.refresh();
                    // The card's … button (one book, the start screen at 1200 wide).
                    if shot == "card-backup-menu" && (0.55..0.65).contains(&t) {
                        let w = v.window().window();
                        let position = slint::LogicalPosition::new(556.0, 405.0);
                        let button = slint::platform::PointerEventButton::Left;
                        w.dispatch_event(slint::platform::WindowEvent::PointerMoved { position });
                        w.dispatch_event(slint::platform::WindowEvent::PointerPressed {
                            position,
                            button,
                        });
                        w.dispatch_event(slint::platform::WindowEvent::PointerReleased {
                            position,
                            button,
                        });
                    }
                }
                if t > 1.6 {
                    tm.stop();
                    if let Some(v) = cur.borrow_mut().take() {
                        let theme = if dark { "dark" } else { "light" };
                        match v.window().window().take_snapshot() {
                            Ok(buf) => {
                                let p = out.join(format!("{shot}-{theme}.png"));
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
    slint::Timer::single_shot(std::time::Duration::from_millis(50), move || {
        if let Some(f) = next.borrow().as_ref() {
            f();
        }
    });
    slint::run_event_loop_until_quit().unwrap();
}
