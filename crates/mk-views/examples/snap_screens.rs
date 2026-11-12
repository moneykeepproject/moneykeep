//! Renders Settings > Connections with a SnapTrade connection whose last sync could
//! not read the holdings (connectivity-brokerage.md revision 2: the yellow line
//! naming what failed, the transactions imported), light and dark, into
//! `snap-partial-*.png`. Run under a display (or Xvfb) with
//! `SLINT_BACKEND=winit-software cargo run -p mk-views --example snap_screens -- <dir>`.
//! SnapTrade is the registry's scripted fake: no network.
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
use mk_commands::connect::{Connector, MemorySecrets};
use mk_commands::snap_conn::FakeSnaptrade;
use mk_ui::ctl::App;
use mk_ui::{Backend, UiError, UserConfig};
use mk_views::Views;
use serde_json::{Value, json};
use slint::ComponentHandle;

/// 2026-10-04 00:00 UTC.
const NOW: i64 = 1_791_072_000;

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
    fn create_book(&mut self, _: &str, _: &str, _: &str, _: Option<&str>) -> Result<(), UiError> {
        Ok(())
    }
    fn open_book(&mut self, _: &str) -> Result<(), UiError> {
        Ok(())
    }
    fn close_book(&mut self) {}
}

fn app(dir: &std::path::Path, dark: bool) -> App {
    let path = dir.join(format!("snap-{dark}.moneykeep"));
    let mut s = Session::create(&path, "Household", "USD", "app").unwrap();
    let fake = FakeSnaptrade::standard();
    fake.with(|st| {
        let act: Value = serde_json::from_str(
            r#"{"id": "b1", "type": "BUY", "trade_date": "2026-09-02T00:00:00.000Z",
                "units": 10, "price": 220.00, "amount": -2200.00, "fee": 0,
                "currency": {"code": "USD"}, "description": "BUY VTI",
                "symbol": {"symbol": "VTI", "raw_symbol": "VTI", "description": "Vanguard Total Stock Market ETF"}}"#,
        )
        .unwrap();
        st.activities.insert("acct-1".into(), vec![act]);
        st.balances.insert(
            "acct-1".into(),
            vec![json!({"currency": {"code": "USD"}, "cash": "300.71"})],
        );
        st.fail.insert(
            "positions".into(),
            (410, r#"{"detail": "Gone", "status_code": 410}"#.into()),
        );
    });
    s.set_connector(Connector {
        fetcher: Arc::new(fake),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(NOW),
        host: None,
    });
    for (c, a) in [
        ("module.enable", json!({"module": "bank-feeds"})),
        ("module.enable", json!({"module": "investments"})),
        (
            "account.create",
            json!({"name": "Assets", "type": "asset", "placeholder": true}),
        ),
        (
            "account.create",
            json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
        ),
    ] {
        s.invoke(c, a).unwrap();
    }
    let c = s
        .invoke(
            "connect.add",
            json!({"provider": "snaptrade", "client_id": "TESTCLIENT",
                   "consumer_key": "test-consumer-key", "label": "Schwab via SnapTrade"}),
        )
        .unwrap();
    s.invoke(
        "connect.link_account",
        json!({"connection": c["id"], "remote_account": "acct-1", "account": "Assets:Brokerage"}),
    )
    .unwrap();
    let out = s.invoke("connect.sync", json!({"now": true})).unwrap();
    println!("{}", out["connections"][0]["partial"]);
    let cfg = UserConfig {
        theme: if dark {
            mk_ui::Theme::Dark
        } else {
            mk_ui::Theme::Light
        },
        date_format: "MM-DD-YYYY".into(),
        settings_section: "Connections".into(),
        ..UserConfig::default()
    };
    let mut a = App::new(Box::new(Local { session: s }), cfg);
    a.set_today("2026-10-04");
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
    let jobs = Rc::new(RefCell::new([false, true].into_iter()));
    let dir_path = dir.path().to_owned();
    let next: Rc<RefCell<Option<Box<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let next2 = next.clone();
    *next.borrow_mut() = Some(Box::new(move || {
        let Some(dark) = jobs.borrow_mut().next() else {
            let _ = slint::quit_event_loop();
            return;
        };
        let views = Views::new(app(&dir_path, dark)).unwrap();
        views
            .window()
            .window()
            .set_size(slint::LogicalSize::new(1280.0, 900.0));
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
                                let p = out.join(format!("snap-partial-{theme}.png"));
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
