//! Renders the opening balance wizards of opening-balances.md for review, light, into
//! `ob-*.png`: each step of the money wizard for a feed checking account (the start
//! date and the Check showing the match), a credit card's balance step, the setup
//! checklist with its progress bar, the investment wizard's holdings and cost basis
//! steps (with "I don't know yet") and Home's Attention with the checklist item. Run
//! under a display (or Xvfb) with
//! `SLINT_BACKEND=winit-software cargo run -p mk-views --example ob_screens -- <dir>`.
//! The bank feed is the scripted fake (`mk_commands::connect::FakeSimplefin`): no
//! network.
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
use mk_commands::connect::{Connector, FakeSimplefin, MemorySecrets};
use mk_ui::ctl::App;
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

/// 2026-11-06 00:00 UTC.
const NOV6: i64 = 1_793_923_200;

const FEED: &str = r#"{
  "errlist": [],
  "connections": [{"conn_id": "c1", "name": "First Bank"}],
  "accounts": [
    {"id": "a1", "name": "Everyday", "conn_id": "c1", "currency": "USD",
     "balance": "2341.10", "balance-date": 1793923200,
     "transactions": [
       {"id": "t1", "posted": 1785801600, "amount": "2000.00", "description": "Payroll"},
       {"id": "t2", "posted": 1789084800, "amount": "-1035.35", "description": "Rent"},
       {"id": "t3", "posted": 1793836800, "amount": "-500.00", "description": "Savings transfer"}
     ]}
  ]
}"#;

/// (file, review spec or "" for Home, window height).
const SHOTS: [(&str, &str, f32); 11] = [
    ("ob-money-1-account", "money|Assets:Checking|0", 900.0),
    ("ob-money-2-balance", "money|Assets:Checking|1", 900.0),
    ("ob-money-3-start-date", "money|Assets:Checking|2", 900.0),
    ("ob-money-4-check", "money|Assets:Checking|3", 900.0),
    ("ob-money-5-done", "money|Assets:Checking|4", 900.0),
    ("ob-card-balance", "money|Liabilities:Visa|1|512.30", 900.0),
    ("ob-checklist", "checklist", 900.0),
    (
        "ob-inv-holdings",
        "inv|Assets:Brokerage|2|VTI=100,FXAIX=52.3",
        900.0,
    ),
    (
        "ob-inv-basis",
        "inv|Assets:Brokerage|3|VTI=100,FXAIX=52.3",
        1200.0,
    ),
    (
        "ob-inv-check",
        "inv|Assets:Brokerage|4|VTI=100,FXAIX=52.3",
        900.0,
    ),
    ("ob-home-attention", "", 900.0),
];

fn app(dir: &std::path::Path, i: usize, spec: &str) -> App {
    let path = dir.join(format!("ob-{i}.moneykeep"));
    let mut s = Session::create(&path, "Household", "USD", "app").unwrap();
    s.set_connector(Connector {
        fetcher: Arc::new(FakeSimplefin::new(FEED)),
        secrets: Box::new(MemorySecrets::new(true)),
        utc_offset_secs: 0,
        now: Some(NOV6),
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
            json!({"name": "Checking", "type": "asset", "subtype": "bank", "parent": "Assets"}),
        ),
        (
            "account.create",
            json!({"name": "Savings", "type": "asset", "subtype": "bank", "parent": "Assets"}),
        ),
        (
            "account.create",
            json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
        ),
        (
            "account.create",
            json!({"name": "Liabilities", "type": "liability", "placeholder": true}),
        ),
        (
            "account.create",
            json!({"name": "Visa", "type": "liability", "subtype": "credit-card", "parent": "Liabilities"}),
        ),
        (
            "account.create",
            json!({"name": "Expenses", "type": "expense", "placeholder": true}),
        ),
        (
            "account.create",
            json!({"name": "Groceries", "type": "expense", "parent": "Expenses"}),
        ),
        (
            "security.create",
            json!({"symbol": "VTI", "name": "Total Stock Market ETF", "kind": "etf"}),
        ),
        (
            "security.create",
            json!({"symbol": "FXAIX", "name": "500 Index Fund", "kind": "mutual_fund"}),
        ),
        (
            "price.set",
            json!({"security": "VTI", "date": "2026-08-01", "price": "265.40"}),
        ),
        (
            "price.set",
            json!({"security": "VTI", "date": "2026-11-05", "price": "301.12"}),
        ),
        (
            "price.set",
            json!({"security": "FXAIX", "date": "2026-08-01", "price": "201.05"}),
        ),
        (
            "price.set",
            json!({"security": "FXAIX", "date": "2026-11-05", "price": "214.88"}),
        ),
        (
            "txn.create",
            json!({"date": "2026-08-04", "payee": "Deposit", "splits": [
                {"account": "Assets:Brokerage", "value": "1500.00"},
                {"account": "Assets:Savings", "value": "-1500.00"}]}),
        ),
        (
            "txn.create",
            json!({"date": "2026-10-12", "payee": "Corner Grocery", "splits": [
                {"account": "Expenses:Groceries", "value": "84.20"},
                {"account": "Liabilities:Visa", "value": "-84.20"}]}),
        ),
    ] {
        s.invoke(c, a).unwrap();
    }
    let c = s
        .invoke(
            "connect.add",
            json!({"setup_input": FakeSimplefin::token("ob"), "label": "My bank", "confirm": true}),
        )
        .unwrap();
    s.invoke(
        "connect.link_account",
        json!({"connection": c["id"], "remote_account": "a1", "account": "Assets:Checking",
               "sync_from": "2026-08-01"}),
    )
    .unwrap();
    let out = s.invoke("connect.sync", json!({"now": true})).unwrap();
    s.invoke(
        "import.commit",
        json!({"batch": out["connections"][0]["batch"]}),
    )
    .unwrap();
    if spec == "checklist" || spec.is_empty() {
        // Some accounts done, so the progress bar shows progress.
        s.invoke(
            "opening.apply",
            json!({"account": "Assets:Checking", "today": "2026-11-06"}),
        )
        .unwrap();
        s.invoke(
            "opening.apply",
            json!({"account": "Assets:Savings", "balance": "8200.00", "as_of": "2026-11-06"}),
        )
        .unwrap();
    }
    if spec.is_empty() {
        s.invoke(
            "opening.started_from_zero",
            json!({"account": "Assets:Savings", "on": false}),
        )
        .unwrap();
    }
    let cfg = UserConfig {
        theme: mk_ui::Theme::Light,
        date_format: "MM-DD-YYYY".into(),
        ..UserConfig::default()
    };
    let mut a = App::new(Box::new(Local { session: s }), cfg);
    a.set_today("2026-11-06");
    a.navigate(mk_ui::View::Home);
    if !spec.is_empty() {
        a.open_opening_review(spec);
    }
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
        let Some((i, (file, spec, height))) = jobs.borrow_mut().next() else {
            let _ = slint::quit_event_loop();
            return;
        };
        let views = Views::new(app(&dir_path, i, spec)).unwrap();
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
