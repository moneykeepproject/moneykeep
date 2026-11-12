//! Renders the screens of connectivity.md revision 5, app-desktop.md revision 38 and
//! mobile-capture.md revision 13 for review: Home with a dismissible Attention
//! notice (its ×) and Settings > Phone capture with Wi-Fi set up (the one Pair a
//! phone… button), light, into `r13-*.png`. Run under a display (or Xvfb) with
//! `SLINT_BACKEND=winit-software cargo run -p mk-views --example r13_screens -- <dir>`.
//! The bank feed, capture server and computer are the scripted fakes
//! (`mk_commands::connect::FakeSimplefin`, `mk_commands::capture_setup::testing`):
//! no network, nothing installed.
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
use mk_commands::connect::{Connector, FakeSimplefin, MemorySecrets};
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

const FAILING: &str = r#"{
  "errlist": [{"code": "gen.auth", "msg": "Authentication failed"}],
  "connections": [{"conn_id": "c1", "name": "First Bank"}],
  "accounts": [
    {"id": "a1", "name": "Everyday", "conn_id": "c1", "currency": "USD",
     "balance": "1200.50", "balance-date": 1793000000,
     "transactions": [
       {"id": "t1", "posted": 1792800000, "amount": "-45.10", "description": "Corner Grocery"},
       {"id": "t2", "posted": 1792900000, "amount": "2000.00", "description": "Payroll"}
     ]}
  ]
}"#;

/// The shots: file name, Settings section ("" for Home), a press (if any), and the
/// window height.
const SHOTS: [(&str, &str, Option<&str>, f32); 2] = [
    ("r13-home-attention-dismiss-light", "", None, 900.0),
    (
        "r13-settings-phone-capture-light",
        "Phone capture",
        None,
        1500.0,
    ),
];

fn app(dir: &std::path::Path, i: usize, section: &str) -> App {
    let path = dir.join(format!("r13-{i}.moneykeep"));
    let mut s = Session::create(&path, "Household", "USD", "app").unwrap();
    let cfg = UserConfig {
        theme: mk_ui::Theme::Light,
        date_format: "MM-DD-YYYY".into(),
        settings_section: section.into(),
        ..UserConfig::default()
    };
    if section.is_empty() {
        s.set_connector(Connector {
            fetcher: Arc::new(FakeSimplefin::new(FAILING)),
            secrets: Box::new(MemorySecrets::new(true)),
            utc_offset_secs: 0,
            now: Some(1_793_000_000),
            host: None,
        });
        for (c, a) in [
            ("module.enable", json!({"module": "bank-feeds"})),
            (
                "account.create",
                json!({"name": "Assets", "type": "asset", "placeholder": true}),
            ),
            (
                "account.create",
                json!({"name": "Checking", "type": "asset", "subtype": "bank", "parent": "Assets"}),
            ),
        ] {
            s.invoke(c, a).unwrap();
        }
        let c = s
            .invoke(
                "connect.add",
                json!({"setup_input": FakeSimplefin::token("r13"), "label": "My bank", "confirm": true}),
            )
            .unwrap();
        s.invoke(
            "connect.link_account",
            json!({"connection": c["id"], "remote_account": "a1", "account": "Assets:Checking",
                   "sync_from": "2026-10-01"}),
        )
        .unwrap();
        s.invoke("connect.sync", json!({"now": true})).unwrap();
        let mut a = App::new(Box::new(Local { session: s }), cfg);
        a.navigate(mk_ui::View::Home);
        return a;
    }
    let host = FakeHost::windows();
    host.with(|st| {
        st.addresses = vec![LocalAddress {
            address: "192.168.1.20".into(),
            adapter: "Wi-Fi".into(),
            mac: "a4:b1:c2:d3:e4:f5".into(),
            profile: "Private".into(),
            dhcp: true,
        }];
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
    s.invoke(
        "capture.wifi_apply",
        json!({"address": "192.168.1.20", "pin": "246813"}),
    )
    .unwrap();
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
