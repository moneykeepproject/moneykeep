//! Renders the screens of mobile-capture.md revision 12 and app-desktop.md
//! revision 37 for review: Wi-Fi setup step 1 (the blank local name), Settings >
//! Phone capture (Wi-Fi first, Cloudflare after) and Settings > Operation (Close
//! status messages automatically), light, into `r12-*.png`. Run under a display
//! (or Xvfb) with
//! `SLINT_BACKEND=winit-software cargo run -p mk-views --example r12_screens -- <dir>`.
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
use mk_commands::capture_setup::testing::{FakeCloudflare, FakeHost};
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

/// The shots: file name, Settings section, a button to press there (if any), and
/// the window height.
const SHOTS: [(&str, &str, Option<&str>, f32); 3] = [
    (
        "r12-wifi-step1-light",
        "Phone capture",
        Some("settings:Phone capture/Set up Wi-Fi capture…"),
        1000.0,
    ),
    (
        "r12-settings-phone-capture-light",
        "Phone capture",
        None,
        1500.0,
    ),
    ("r12-settings-operation-light", "Operation", None, 1000.0),
];

fn app(dir: &std::path::Path, i: usize, section: &str) -> App {
    let path = dir.join(format!("r12-{i}.moneykeep"));
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
    let cfg = UserConfig {
        theme: mk_ui::Theme::Light,
        date_format: "MM-DD-YYYY".into(),
        settings_section: section.into(),
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
    let jobs = Rc::new(RefCell::new(SHOTS.iter().copied().enumerate()));
    let dir_path = dir.path().to_owned();
    let next: Rc<RefCell<Option<Box<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let next2 = next.clone();
    *next.borrow_mut() = Some(Box::new(move || {
        let Some((i, (file, section, press, height))) = jobs.borrow_mut().next() else {
            let _ = slint::quit_event_loop();
            return;
        };
        let views = Views::new(app(&dir_path, i, section)).unwrap();
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
                    if let Some(id) = press
                        && (0.25..0.35).contains(&t)
                    {
                        v.with_app(|a| a.act(id, Ev::Click, ""));
                        v.refresh();
                    }
                }
                if t > 1.6 {
                    tm.stop();
                    if let Some(v) = cur.borrow_mut().take() {
                        match v.window().window().take_snapshot() {
                            Ok(buf) => {
                                let p = out.join(format!("{file}.png"));
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
