//! Renders mobile-capture.md revision 16 for review into `cap16-*.png`: the setup
//! dialog's Install step when a cloudflared service runs a deleted tunnel (the
//! replace message with Replace it / Leave it, `cap16-install-*`), and a removal
//! that did not finish with Retry undo (`cap16-undo-*`), light and dark. Run under a
//! display (or Xvfb) with
//! `SLINT_BACKEND=winit-software cargo run -p mk-views --example cap16_screens -- <dir>`.
//! The Cloudflare API and the computer are the scripted fakes of
//! `mk_commands::capture_setup::testing`: no network, nothing installed.
#![allow(
    clippy::unwrap_used,
    clippy::print_stdout,
    clippy::float_arithmetic,
    clippy::type_complexity,
    clippy::indexing_slicing
)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mk_commands::Session;
use mk_commands::capture_setup::testing::{FakeCloudflare, FakeHost, TEST_TOKEN};
use mk_commands::capture_setup::{PERMISSIONS, ServiceState};
use mk_commands::connect::{Connector, MemorySecrets};
use mk_ui::ctl::{App, Ev};
use mk_ui::{Backend, UiError, UserConfig};
use mk_views::Views;
use serde_json::{Value, json};
use slint::ComponentHandle;

struct Local {
    session: Session,
}

impl Backend for Local {
    fn invoke(&mut self, command: &str, args: Value) -> Result<Value, UiError> {
        self.session
            .invoke(command, args)
            .map_err(|e| UiError::new(e.code(), e.to_string()))
    }
    fn book_name(&self) -> Option<String> {
        Some("Household".into())
    }
    fn create_book(
        &mut self,
        _path: &str,
        _name: &str,
        _currency: &str,
        _preset: Option<&str>,
    ) -> Result<(), UiError> {
        Ok(())
    }
    fn open_book(&mut self, _: &str) -> Result<(), UiError> {
        Ok(())
    }
    fn close_book(&mut self) {}
}

fn app(dir: &std::path::Path, theme: mk_ui::Theme, shot: &str) -> App {
    let path = dir.join(format!("{theme:?}-{shot}.moneykeep"));
    let mut s = Session::create(&path, "Household", "USD", "app").unwrap();
    let host = FakeHost::windows();
    let cf = FakeCloudflare::new();
    if shot == "install" {
        host.with(|st| {
            st.report.cloudflared_installed = true;
            st.report.cloudflared_service = Some(ServiceState::Running);
            st.report.cloudflared_tunnel = Some("0d5c0a2e-6f1b-4c3d-9e8f-112233445566".into());
            st.report.cloudflared_account = Some("acc1".into());
        });
    }
    s.set_connector(Connector {
        fetcher: Arc::new(cf.clone()),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(1_793_000_000),
        host: Some(Arc::new(host)),
    });
    s.invoke("module.enable", json!({"module": "expenses"}))
        .unwrap();
    s.invoke("capture.setup_check_token", json!({"token": TEST_TOKEN}))
        .unwrap();
    if shot == "undo" {
        s.invoke(
            "capture.setup_apply",
            json!({"emails": ["owner@example.com"]}),
        )
        .unwrap();
        // The token lost its DNS permission: that one step fails.
        cf.with(|st| {
            st.granted.remove(PERMISSIONS[4]);
        });
    }
    let cfg = UserConfig {
        theme,
        date_format: "MM-DD-YYYY".into(),
        settings_section: "Advanced".into(),
        ..UserConfig::default()
    };
    let mut a = App::new(Box::new(Local { session: s }), cfg);
    a.navigate(mk_ui::View::Settings);
    a
}

/// The action id of the item showing `text` (the view model's items).
fn id_of(a: &mut App, text: &str) -> Option<String> {
    let dump = format!("{:?}", a.vm());
    let needle = format!("\", text: {text:?}");
    let at = dump.find(&needle)?;
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

/// The clicks (or `=text` edits of the emails field) of each shot, one every few
/// frames so the background jobs between them finish.
fn script(shot: &str) -> Vec<&'static str> {
    if shot == "install" {
        vec![
            "Continue setting up phone capture over Cloudflare…",
            "Next",
            "Check token",
            "Next",
            "=owner@example.com",
            "Next",
            "Next",
        ]
    } else {
        vec!["Remove phone capture over Cloudflare…", "Remove"]
    }
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "screens".into()));
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut jobs = Vec::new();
    for theme in [mk_ui::Theme::Light, mk_ui::Theme::Dark] {
        for shot in ["install", "undo"] {
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
            .set_size(slint::LogicalSize::new(1280.0, 1000.0));
        views.show().unwrap();
        let out = out.clone();
        let next3 = next2.clone();
        let cur = Rc::new(RefCell::new(Some(views)));
        let steps = script(shot);
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
                    if frame.is_multiple_of(5)
                        && let Some(step) = steps.get(frame / 5 - 1)
                    {
                        v.with_app(|a| {
                            if let Some(text) = step.strip_prefix('=') {
                                let id = id_of_field(a, "Email addresses allowed");
                                match id {
                                    Some(id) => a.act(&id, Ev::Edit, text),
                                    None => println!("no field for the emails"),
                                }
                            } else {
                                match id_of(a, step) {
                                    Some(id) => a.act(&id, Ev::Click, ""),
                                    None => println!("no item {step:?}"),
                                }
                            }
                        });
                        v.refresh();
                    }
                }
                if frame > steps.len() * 5 + 25 {
                    tm.stop();
                    if let Some(v) = cur.borrow_mut().take() {
                        let word = if theme == mk_ui::Theme::Dark {
                            "dark"
                        } else {
                            "light"
                        };
                        match v.window().window().take_snapshot() {
                            Ok(buf) => {
                                let p = out.join(format!("cap16-{shot}-{word}.png"));
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

/// The action id of the field whose accessible name is `label`.
fn id_of_field(a: &mut App, label: &str) -> Option<String> {
    let dump = format!("{:?}", a.vm());
    let needle = format!("{label:?}");
    let mut from = 0;
    while let Some(rel) = dump[from..].find(&needle) {
        let at = from + rel;
        let head = &dump[..at];
        let item = head.rfind("Item {")?;
        if head[item..].contains("kind: Field") {
            let start = head[item..].find("id: \"")? + item + 5;
            let end = dump[start..].find('"')? + start;
            return Some(dump[start..end].to_owned());
        }
        from = at + needle.len();
    }
    None
}
