//! Renders Wi-Fi capture's setup dialog (mobile-capture.md revision 10, test 76) at
//! steps 1 (Address), 5 (Certificate on the phone) and 6 (Pair), light and dark,
//! into `desktop10-*.png` for review; app-desktop.md revision 39: Settings → Phone
//! capture with its one Pair a phone… button (`desktop39-settings-*`) and, with
//! Cloudflare capture set up too, the "How will this phone send expenses?" choice
//! (`desktop39-choice-*`). Run under a display (or Xvfb) with
//! `SLINT_BACKEND=winit-software cargo run -p mk-views --example wifi_screens -- <dir>`.
//! The capture server and the computer are the scripted fakes of
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
use mk_commands::capture_setup::LocalAddress;
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

fn app(dir: &std::path::Path, theme: mk_ui::Theme, shot: &str) -> App {
    let installed = shot != "address";
    let path = dir.join(format!("{theme:?}-{shot}.moneykeep"));
    let mut s = Session::create(&path, "Household", "USD", "app").unwrap();
    let host = FakeHost::windows();
    host.with(|st| {
        st.addresses = vec![
            LocalAddress {
                address: "192.168.1.20".into(),
                adapter: "Wi-Fi".into(),
                mac: "a4:b1:c2:d3:e4:f5".into(),
                profile: "Private".into(),
                dhcp: true,
            },
            LocalAddress {
                address: "10.0.0.7".into(),
                adapter: "Ethernet".into(),
                mac: "00:11:22:33:44:55".into(),
                profile: "Private".into(),
                dhcp: false,
            },
        ];
    });
    s.set_connector(Connector {
        fetcher: Arc::new(FakeCloudflare::new()),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(1_793_000_000),
        host: Some(Arc::new(host)),
    });
    s.invoke("module.enable", json!({"module": "expenses"}))
        .unwrap();
    if installed {
        s.invoke(
            "capture.wifi_apply",
            json!({"address": "192.168.1.20", "pin": "246813"}),
        )
        .unwrap();
    }
    if shot == "choice" {
        s.invoke("capture.setup_check_token", json!({"token": TEST_TOKEN}))
            .unwrap();
        s.invoke(
            "capture.setup_apply",
            json!({"emails": ["owner@example.com"]}),
        )
        .unwrap();
    }
    let cfg = UserConfig {
        theme,
        date_format: "MM-DD-YYYY".into(),
        settings_section: "Phone capture".into(),
        ..UserConfig::default()
    };
    let mut a = App::new(Box::new(Local { session: s }), cfg);
    a.navigate(mk_ui::View::Settings);
    a
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
    let dir = tempfile::tempdir().unwrap();
    let mut jobs = Vec::new();
    for theme in [mk_ui::Theme::Light, mk_ui::Theme::Dark] {
        for shot in ["address", "certificate", "pair", "settings", "choice"] {
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
                    if (0.25..0.35).contains(&t) && shot != "settings" {
                        let label = match shot {
                            "address" => "Set up Wi-Fi capture…",
                            "certificate" => "Certificate QR code…",
                            _ => "Pair a phone…",
                        };
                        v.with_app(|a| {
                            a.act(&format!("settings:Phone capture/{label}"), Ev::Click, "")
                        });
                        v.refresh();
                    }
                }
                if t > 1.6 {
                    tm.stop();
                    if let Some(v) = cur.borrow_mut().take() {
                        let step = match shot {
                            "address" => "step1-address",
                            "certificate" => "step5-certificate",
                            "settings" => "settings",
                            "choice" => "choice",
                            _ => "step6-pair",
                        };
                        let prefix = if matches!(shot, "settings" | "choice") {
                            "desktop39"
                        } else {
                            "desktop10"
                        };
                        let word = if theme == mk_ui::Theme::Dark {
                            "dark"
                        } else {
                            "light"
                        };
                        match v.window().window().take_snapshot() {
                            Ok(buf) => {
                                let p = out.join(format!("{prefix}-{step}-{word}.png"));
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
