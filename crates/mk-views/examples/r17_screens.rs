//! Renders mobile-capture.md revision 17 and app-desktop.md revision 41 for review:
//! the Start over preview (`startover-preview-*`, with phone items not yet
//! collected), its result when Cloudflare can't be reached (`startover-result-*`),
//! and Settings → Advanced → Reset MoneyKeep settings… (`reset-settings-*`), light
//! and dark. Run under a display (or Xvfb) with
//! `SLINT_BACKEND=winit-software cargo run -p mk-views --example r17_screens -- <dir>`.
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

fn app(dir: &std::path::Path, theme: mk_ui::Theme, shot: &str) -> App {
    let path = dir.join(format!("{theme:?}-{shot}.moneykeep"));
    let mut s = Session::create(&path, "Household", "USD", "app").unwrap();
    let host = FakeHost::windows();
    host.with(|st| {
        st.addresses = vec![mk_commands::capture_setup::LocalAddress {
            address: "192.168.1.20".into(),
            adapter: "Wi-Fi".into(),
            profile: "Private".into(),
            ..Default::default()
        }];
    });
    let cf = FakeCloudflare::new();
    s.set_connector(Connector {
        fetcher: Arc::new(cf.clone()),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(1_793_000_000),
        host: Some(Arc::new(host)),
    });
    s.invoke("module.enable", json!({"module": "expenses"}))
        .unwrap();
    if shot != "reset-settings" {
        s.invoke("capture.setup_check_token", json!({"token": TEST_TOKEN}))
            .unwrap();
        s.invoke(
            "capture.setup_apply",
            json!({"emails": ["owner@example.com"]}),
        )
        .unwrap();
        s.invoke(
            "capture.wifi_apply",
            json!({"address": "192.168.1.20", "pin": "246813"}),
        )
        .unwrap();
    }
    if shot == "startover-preview" {
        // Not collected automatically on open, so the dialog shows them waiting.
        s.invoke(
            "module.set_setting",
            json!({"key": "capture.auto_collect", "value": false}),
        )
        .unwrap();
        cf.with(|st| {
            st.inbox = (1..=3u8)
                .map(|n| {
                    json!({"id": format!("{n:032x}"), "client_id": format!("c{n}"),
                           "device_id": format!("{:032x}", 9), "device_name": "Pixel",
                           "submitted_at": 1_790_800_000,
                           "fields": {"date": "2026-10-01", "merchant": "Cafe", "amount": "4.50"},
                           "receipts": []})
                })
                .collect();
        });
    }
    if shot == "startover-result" {
        cf.with(|st| st.cloudflare_down = true);
    }
    let cfg = UserConfig {
        theme,
        date_format: "MM-DD-YYYY".into(),
        settings_section: if shot == "reset-settings" {
            "Advanced".into()
        } else {
            "Phone capture".into()
        },
        ..UserConfig::default()
    };
    let mut a = App::new(Box::new(Local { session: s }), cfg);
    a.set_today("2026-11-09");
    a.set_settings_folder("C:\\Users\\Owner\\AppData\\Roaming\\MoneyKeep\\config");
    a.navigate(mk_ui::View::Settings);
    a
}

/// The clicks (or `=text` typed into the confirmation field) of each shot, one
/// every few frames so the background jobs between them finish.
fn script(shot: &str) -> Vec<&'static str> {
    match shot {
        "startover-preview" => vec!["Start over…"],
        "startover-result" => vec!["Start over…", "=START OVER", "Start over"],
        _ => vec!["Reset MoneyKeep settings…"],
    }
}

fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "screens".into()));
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut jobs = Vec::new();
    for theme in [mk_ui::Theme::Light, mk_ui::Theme::Dark] {
        for shot in ["startover-preview", "startover-result", "reset-settings"] {
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
            .set_size(slint::LogicalSize::new(1280.0, 1240.0));
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
                                let id = id_of_field(a, "Type START OVER to confirm");
                                match id {
                                    Some(id) => a.act(&id, Ev::Edit, text),
                                    None => println!("no confirmation field"),
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
