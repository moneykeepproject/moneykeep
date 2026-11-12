//! Renders security-review-1 Batch B's screens for review, light and dark, into
//! `srB-*.png`: `srB-question-*` (after a password is added: the question about
//! the book's 3 earlier plain backups), `srB-note-*` (Home with the note Keep them
//! leaves) and `srB-about-*` (About MoneyKeep with the Third-party licences link).
//! Run under a display (or Xvfb) with
//! `SLINT_BACKEND=winit-software cargo run -p mk-views --example srb_screens -- <dir>`.
#![allow(
    clippy::unwrap_used,
    clippy::print_stdout,
    clippy::float_arithmetic,
    clippy::type_complexity
)]

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use mk_commands::backups::{self, BackupEnv, BackupSettings};
use mk_commands::{KdfParams, Secret, Session};
use mk_ui::ctl::App;
use mk_ui::{Backend, BackupPrefs, UiError, UserConfig};
use mk_views::Views;
use serde_json::{Value, json};
use slint::ComponentHandle;

const PW: &str = "correct horse battery";

struct Local {
    session: Option<Session>,
    env: Option<BackupEnv>,
}

impl Backend for Local {
    fn invoke(&mut self, command: &str, args: Value) -> Result<Value, UiError> {
        let env = self.env.clone();
        match self.session.as_mut() {
            Some(s) => {
                if let Some(e) = env {
                    s.set_backup_env(e);
                }
                s.invoke(command, args)
                    .map_err(|e| UiError::new(e.code(), e.to_string()))
            }
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
        let pw = Secret::Password(PW.into());
        self.session = Some(
            Session::open_with(Path::new(path), "app", Some(&pw))
                .map_err(|e| UiError::new(e.code(), e.to_string()))?,
        );
        Ok(())
    }
    fn close_book(&mut self) {
        self.session = None;
    }
    fn protection(&self) -> (bool, bool) {
        (true, false)
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
    let folder = root.join("MoneyKeep Backups");
    let prefs = BackupPrefs {
        folder: Some(folder.to_str().unwrap().to_owned()),
        on_open: false,
        ..BackupPrefs::default()
    };
    // Three plain backups, then the password.
    let mut s = Session::create(&book, "Household", "USD", "app").unwrap();
    s.set_kdf(KdfParams::TEST);
    s.invoke(
        "account.create",
        json!({"name": "Checking", "type": "asset"}),
    )
    .unwrap();
    s.set_backup_env(BackupEnv::new(
        BackupSettings::from_json(&serde_json::to_value(&prefs).unwrap()),
        0,
    ));
    for _ in 0..3 {
        s.invoke("book.backup", json!({})).unwrap();
    }
    s.encrypt(PW, false).unwrap();
    drop(s);
    let cfg = UserConfig {
        theme: if dark {
            mk_ui::Theme::Dark
        } else {
            mk_ui::Theme::Light
        },
        date_format: "MM-DD-YYYY".into(),
        backup: prefs,
        ..UserConfig::default()
    };
    let mut a = App::new(
        Box::new(Local {
            session: None,
            env: None,
        }),
        cfg,
    );
    a.set_today("2026-11-10");
    a.open_book(book.to_str().unwrap());
    a.navigate(mk_ui::View::Home);
    match shot {
        "srB-question" => a.review_unprotected_question(),
        "srB-about" => a.open_about_for_review(),
        _ => {}
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
    for shot in ["srB-question", "srB-note", "srB-about"] {
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
        let height = if shot == "srB-about" { 1000.0 } else { 800.0 };
        views
            .window()
            .window()
            .set_size(slint::LogicalSize::new(1280.0, height));
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
