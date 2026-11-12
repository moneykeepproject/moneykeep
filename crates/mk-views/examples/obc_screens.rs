//! Renders the options, bonds and crypto screens of
//! investments-options-bonds-crypto.md for review, light, into `obc-*.png`: the
//! Option… form writing a put (with its preview), the expired-option dialog, the
//! Bond… form buying with accrued interest, the Crypto… swap form and the holdings of
//! a brokerage account with a held and a written option, a bond and a coin; and
//! (revision 2, `obc2-*.png`) the Edit form of a written put opened from the Activity
//! tab and the investment income report with crypto rewards and interest by kind.
//! Run under a display (or Xvfb) with
//! `SLINT_BACKEND=winit-software cargo run -p mk-views --example obc_screens -- <dir> [prefix]`
//! (only the shots whose file starts with `prefix`).
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

/// (file, review spec or "" for the holdings, window height).
const SHOTS: [(&str, &str, f32); 7] = [
    (
        "obc-option-write-put",
        "form|option|Assets:Brokerage|sell|open|XYZ 261218 P50|1|2.00|0.65|||||",
        1000.0,
    ),
    (
        "obc-option-expiry",
        "expiry|Assets:Brokerage|AAPL 261016 C200",
        900.0,
    ),
    (
        "obc-bond-buy",
        "form|bond|Assets:Brokerage|buy||T27|5|98.50|5.00|42.10|||",
        1050.0,
    ),
    (
        "obc-crypto-swap",
        "form|crypto|Assets:Brokerage|swap||ETH|1||||SOL|50|3000.00",
        1000.0,
    ),
    ("obc-holdings", "", 800.0),
    ("obc2-edit-written-put", "edit-put", 1000.0),
    (
        "obc2-investment-income",
        "report|investment_income|2026-01-01|2026-12-31",
        800.0,
    ),
];

fn app(dir: &std::path::Path, i: usize, spec: &str) -> App {
    let path = dir.join(format!("obc-{i}.moneykeep"));
    let mut s = Session::create(&path, "Household", "USD", "app").unwrap();
    let acc = |s: &mut Session, a: Value| {
        s.invoke("account.create", a).unwrap();
    };
    s.invoke("module.enable", json!({"module": "investments"}))
        .unwrap();
    acc(
        &mut s,
        json!({"name": "Assets", "type": "asset", "placeholder": true}),
    );
    acc(
        &mut s,
        json!({"name": "Checking", "type": "asset", "subtype": "bank", "parent": "Assets"}),
    );
    acc(
        &mut s,
        json!({"name": "Brokerage", "type": "asset", "subtype": "brokerage", "parent": "Assets"}),
    );
    for (c, a) in [
        (
            "txn.create",
            json!({"date": "2025-01-02", "payee": "Deposit", "splits": [
                {"account": "Assets:Brokerage", "value": "20000.00"},
                {"account": "Assets:Checking", "value": "-20000.00"}]}),
        ),
        (
            "security.create",
            json!({"kind": "option", "underlying": "AAPL", "right": "call", "strike": "200",
                   "expiration": "2026-12-18"}),
        ),
        (
            "security.create",
            json!({"kind": "option", "underlying": "AAPL", "right": "call", "strike": "200",
                   "expiration": "2026-10-16"}),
        ),
        (
            "security.create",
            json!({"kind": "option", "underlying": "XYZ", "right": "put", "strike": "50",
                   "expiration": "2026-12-18"}),
        ),
        (
            "security.create",
            json!({"symbol": "T27", "name": "Treasury 4.25% 2027", "kind": "bond",
                   "coupon_rate": "4.25", "maturity": "2027-08-15", "tax": "treasury"}),
        ),
        (
            "security.create",
            json!({"symbol": "BTC", "name": "Bitcoin", "kind": "crypto"}),
        ),
        (
            "security.create",
            json!({"symbol": "ETH", "name": "Ether", "kind": "crypto"}),
        ),
        (
            "security.create",
            json!({"symbol": "SOL", "name": "Solana", "kind": "crypto"}),
        ),
        (
            "inv.option_buy",
            json!({"account": "Assets:Brokerage", "security": "AAPL 261218 C200",
                   "date": "2026-03-02", "quantity": "2", "price": "5.10", "commission": "1.30"}),
        ),
        (
            "inv.option_buy",
            json!({"account": "Assets:Brokerage", "security": "AAPL 261016 C200",
                   "date": "2026-03-02", "quantity": "1", "price": "3.00"}),
        ),
        (
            "inv.buy",
            json!({"account": "Assets:Brokerage", "security": "ETH", "date": "2026-01-10",
                   "quantity": "1", "amount": "2000.00"}),
        ),
    ] {
        s.invoke(c, a).unwrap();
    }
    if spec.is_empty() {
        for (c, a) in [
            (
                "inv.option_write",
                json!({"account": "Assets:Brokerage", "security": "XYZ 261218 P50",
                       "date": "2026-09-01", "quantity": "1", "price": "2.00", "commission": "0.65"}),
            ),
            (
                "inv.bond_buy",
                json!({"account": "Assets:Brokerage", "security": "T27", "date": "2026-03-02",
                       "units": "5", "price": "98.50", "accrued_interest": "42.10",
                       "commission": "5.00"}),
            ),
            (
                "inv.buy",
                json!({"account": "Assets:Brokerage", "security": "BTC", "date": "2026-01-10",
                       "quantity": "0.12345678", "amount": "8000.00"}),
            ),
            (
                "price.set",
                json!({"security": "AAPL 261218 C200", "date": "2026-11-05", "price": "7.25"}),
            ),
            (
                "price.set",
                json!({"security": "XYZ 261218 P50", "date": "2026-11-05", "price": "0.80"}),
            ),
            (
                "price.set",
                json!({"security": "T27", "date": "2026-11-05", "price": "99.10"}),
            ),
            (
                "price.set",
                json!({"security": "BTC", "date": "2026-11-05", "price": "68250"}),
            ),
            (
                "price.set",
                json!({"security": "ETH", "date": "2026-11-05", "price": "2450"}),
            ),
        ] {
            s.invoke(c, a).unwrap();
        }
    }
    let mut spec = spec.to_owned();
    if spec == "edit-put" {
        let out = s
            .invoke(
                "inv.option_write",
                json!({"account": "Assets:Brokerage", "security": "XYZ 261218 P50",
                       "date": "2026-09-01", "quantity": "1", "price": "2.00", "commission": "0.65"}),
            )
            .unwrap();
        let txn = out["transaction"]["id"].as_str().unwrap().to_owned();
        spec = format!("edit|{txn}|Assets:Brokerage");
    }
    if spec.starts_with("report") {
        for (c, a) in [
            (
                "security.create",
                json!({"symbol": "ACME28", "name": "Acme 5% 2028", "kind": "bond",
                       "coupon_rate": "5", "maturity": "2028-06-01"}),
            ),
            (
                "security.create",
                json!({"symbol": "CITY30", "name": "City 3% 2030", "kind": "bond",
                       "coupon_rate": "3", "maturity": "2030-06-01", "tax": "municipal"}),
            ),
            (
                "inv.crypto_reward",
                json!({"account": "Assets:Brokerage", "security": "ETH", "date": "2026-02-01",
                       "quantity": "0.1", "value": "300.00"}),
            ),
            (
                "inv.bond_buy",
                json!({"account": "Assets:Brokerage", "security": "ACME28", "date": "2026-03-02",
                       "units": "5", "price": "98.50", "accrued_interest": "42.10",
                       "commission": "5.00"}),
            ),
            (
                "inv.bond_interest",
                json!({"account": "Assets:Brokerage", "security": "ACME28", "date": "2026-06-01",
                       "amount": "106.25"}),
            ),
            (
                "inv.bond_interest",
                json!({"account": "Assets:Brokerage", "security": "CITY30", "date": "2026-06-01",
                       "amount": "45.00"}),
            ),
            (
                "inv.bond_interest",
                json!({"account": "Assets:Brokerage", "security": "T27", "date": "2026-08-15",
                       "amount": "21.25"}),
            ),
        ] {
            s.invoke(c, a).unwrap();
        }
    }
    let spec = spec.as_str();
    let cfg = UserConfig {
        theme: mk_ui::Theme::Light,
        ..UserConfig::default()
    };
    let mut a = App::new(Box::new(Local { session: s }), cfg);
    a.set_today("2026-11-06");
    if spec.starts_with("expiry") {
        a.navigate(mk_ui::View::Home);
    } else {
        a.open_view_spec("register:Assets:Brokerage");
    }
    if !spec.is_empty() {
        a.open_obc_review(spec);
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
    let only = std::env::args().nth(2).unwrap_or_default();
    let jobs = Rc::new(RefCell::new(
        SHOTS
            .iter()
            .copied()
            .enumerate()
            .filter(move |(_, (f, _, _))| f.starts_with(only.as_str()))
            .collect::<Vec<_>>()
            .into_iter(),
    ));
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
