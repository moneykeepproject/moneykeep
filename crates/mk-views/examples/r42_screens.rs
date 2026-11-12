//! Renders app-desktop.md revision 42 test 236 for review: the status message
//! pop-up at the bottom-right corner, light and dark — an informational message
//! on Home (`message-info-*`), an error with its Details open
//! (`message-error-details-*`) and a message with Undo over a register's
//! bottom-right corner (`message-undo-*`). Run under a display (or Xvfb) with
//! `SLINT_BACKEND=winit-software cargo run -p mk-views --example r42_screens -- <dir>`.
#![allow(
    clippy::unwrap_used,
    clippy::print_stdout,
    clippy::float_arithmetic,
    clippy::type_complexity,
    clippy::indexing_slicing
)]

use std::cell::RefCell;
use std::rc::Rc;

use mk_commands::Session;
use mk_ui::ctl::{App, Ev};
use mk_ui::{Backend, MessagePrefs, UiError, UserConfig};
use mk_views::Views;
use serde_json::{Value, json};
use slint::ComponentHandle;

struct Local {
    session: Option<Session>,
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
        Some("Household".into())
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
        Ok(())
    }
    fn open_book(&mut self, _: &str) -> Result<(), UiError> {
        Ok(())
    }
    fn close_book(&mut self) {
        self.session = None;
    }
}

/// The action id of the item showing `text` (the view model's items).
fn id_of(a: &mut App, text: &str) -> Option<String> {
    let dump = format!("{:?}", a.vm());
    let needle = format!("\", text: {text:?}");
    let at = dump.rfind(&needle)?;
    let head = &dump[..at];
    let start = head.rfind("id: \"")? + 5;
    Some(head[start..].to_owned())
}

fn save_png(path: &std::path::Path, buf: &slint::SharedPixelBuffer<slint::Rgba8Pixel>) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), buf.width(), buf.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap();
    w.write_image_data(buf.as_bytes()).unwrap();
}

/// A demo book with a small chart and enough transactions to fill a register.
fn app(dir: &std::path::Path, theme: mk_ui::Theme, shot: &str) -> App {
    let path = dir.join(format!("{theme:?}-{shot}.moneykeep"));
    let cfg = UserConfig {
        theme,
        date_format: "MM-DD-YYYY".into(),
        // The messages stay for the picture.
        ui: MessagePrefs {
            message_autoclose: false,
            message_seconds: 3,
        },
        ..UserConfig::default()
    };
    let mut a = App::new(Box::new(Local { session: None }), cfg);
    a.create_book(path.to_str().unwrap(), "Household", "USD");
    a.set_today("2026-11-11");
    for acc in [
        json!({"name": "Assets", "type": "asset", "placeholder": true}),
        json!({"name": "Checking", "type": "asset", "subtype": "bank", "parent": "Assets"}),
        json!({"name": "Expenses", "type": "expense", "placeholder": true}),
        json!({"name": "Groceries", "type": "expense", "parent": "Expenses"}),
        json!({"name": "Utilities", "type": "expense", "parent": "Expenses"}),
        json!({"name": "Salary", "type": "income"}),
    ] {
        let _ = a.test_invoke("account.create", acc);
    }
    let payees = [
        ("Green Market", "Expenses:Groceries", "64.20"),
        ("City Power", "Expenses:Utilities", "88.60"),
        ("Corner Grocer", "Expenses:Groceries", "31.75"),
        ("Water Works", "Expenses:Utilities", "42.00"),
    ];
    for m in 5..=10u32 {
        let _ = a.test_invoke(
            "txn.create",
            json!({"date": format!("2026-{m:02}-01"), "payee": "Employer Inc", "splits": [
                {"account": "Salary", "value": "-3200.00"},
                {"account": "Assets:Checking", "value": "3200.00"}]}),
        );
        for (i, (payee, other, value)) in payees.iter().enumerate() {
            let _ = a.test_invoke(
                "txn.create",
                json!({"date": format!("2026-{m:02}-{:02}", 5 + i * 6), "payee": payee, "splits": [
                    {"account": other, "value": value},
                    {"account": "Assets:Checking", "value": format!("-{value}")}]}),
            );
        }
    }
    match shot {
        "message-info" => {
            a.navigate(mk_ui::View::Home);
            a.notify_full_for_tests(
                "Imported 12 transactions from October.csv.",
                false,
                None,
                false,
            );
        }
        "message-error-details" => {
            a.navigate(mk_ui::View::Home);
            a.notify_full_for_tests(
                "Couldn't save the attachment. The folder may be full or read-only.",
                true,
                Some(
                    "io: could not write C:\\Users\\Owner\\Documents\\MoneyKeep\\Receipts\\\
                     2026-10-18 City Power.pdf (os error 112: there is not enough space \
                     on the disk)",
                ),
                false,
            );
        }
        _ => {
            a.navigate(mk_ui::View::Register {
                account: "Assets:Checking".into(),
            });
            a.notify_full_for_tests("Transaction deleted.", false, None, true);
        }
    }
    a
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "screens".into()));
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut jobs = Vec::new();
    for theme in [mk_ui::Theme::Light, mk_ui::Theme::Dark] {
        for shot in ["message-info", "message-error-details", "message-undo"] {
            jobs.push((theme, shot));
        }
    }
    let jobs = Rc::new(RefCell::new(jobs.into_iter()));
    let dir_path = dir.path().to_owned();
    let next: Rc<RefCell<Option<Box<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let next2 = next.clone();
    *next.borrow_mut() = Some(Box::new(move || {
        let Some((theme, shot)) = jobs.borrow_mut().next() else {
            let _ = slint::quit_event_loop();
            return;
        };
        let a = app(&dir_path, theme, shot);
        let views = Views::new(a).unwrap();
        views
            .window()
            .window()
            .set_size(slint::LogicalSize::new(1280.0, 820.0));
        views.show().unwrap();
        let out = out.clone();
        let next3 = next2.clone();
        let cur = Rc::new(RefCell::new(Some(views)));
        let mut frame = 0usize;
        let mut t = 0.0;
        let timer = Rc::new(slint::Timer::default());
        let tm = timer.clone();
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(100),
            move || {
                t += 0.1;
                frame += 1;
                if let Some(v) = cur.borrow().as_ref() {
                    v.tick(t);
                    // No pointer over the page (no stray tooltip in the picture).
                    if frame == 2 {
                        v.window()
                            .window()
                            .dispatch_event(slint::platform::WindowEvent::PointerExited);
                    }
                    if frame == 5 && shot == "message-error-details" {
                        v.with_app(|a| match id_of(a, "Details") {
                            Some(id) => a.act(&id, Ev::Click, ""),
                            None => println!("no Details link"),
                        });
                    }
                }
                if frame > 15 {
                    tm.stop();
                    if let Some(v) = cur.borrow_mut().take() {
                        let word = if theme == mk_ui::Theme::Dark {
                            "dark"
                        } else {
                            "light"
                        };
                        match v.window().window().take_snapshot() {
                            Ok(buf) => {
                                let p = out.join(format!("{shot}-{word}.png"));
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
