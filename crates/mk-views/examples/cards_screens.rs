//! Renders the book cards and book pictures (app-desktop.md revision 40, test 229)
//! for review, light and dark: `start-cards-*` (the start screen with four books
//! and the Create card), `book-settings-presets-*` (Settings → Book with People and
//! work) and `picture-editor-*` (the Picture editor on a photo). Run under a display
//! (or Xvfb) with
//! `SLINT_BACKEND=winit-software cargo run -p mk-views --example cards_screens -- <dir> [photo]`.
//! The photo (PNG or JPEG) defaults to the test photo of the engraving module.
#![allow(
    clippy::unwrap_used,
    clippy::print_stdout,
    clippy::float_arithmetic,
    clippy::type_complexity,
    clippy::indexing_slicing
)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use base64::Engine;
use mk_commands::Session;
use mk_ui::ctl::App;
use mk_ui::{Backend, RecentBook, TileCache, TileIcon, UiError, UserConfig};
use mk_views::Views;
use serde_json::{Value, json};
use slint::ComponentHandle;

struct Local {
    session: Option<Session>,
    protected: Vec<String>,
    images: HashMap<String, Vec<u8>>,
}

impl Backend for Local {
    fn invoke(&mut self, command: &str, args: Value) -> Result<Value, UiError> {
        match self.session.as_mut() {
            Some(s) => s
                .invoke(command, args)
                .map_err(|e| UiError::new(e.code(), e.to_string())),
            None => Err(UiError::new("no_book", "no book")),
        }
    }
    fn book_name(&self) -> Option<String> {
        self.session.as_ref().map(|_| "St. Mark's Parish".into())
    }
    fn create_book(&mut self, _: &str, _: &str, _: &str, _: Option<&str>) -> Result<(), UiError> {
        Ok(())
    }
    fn open_book(&mut self, _: &str) -> Result<(), UiError> {
        Ok(())
    }
    fn close_book(&mut self) {}
    fn is_protected_file(&self, path: &str) -> Option<bool> {
        if path.contains("missing") {
            None
        } else {
            Some(self.protected.iter().any(|p| p == path))
        }
    }
    fn load_tile_image(&self, path: &str) -> Option<Vec<u8>> {
        self.images.get(path).cloned()
    }
}

fn tile(name: &str, tagline: &str, description: &str, icon: TileIcon, color: &str) -> TileCache {
    TileCache {
        name: name.into(),
        tagline: tagline.into(),
        description: description.into(),
        icon,
        color: color.into(),
        currency: "USD".into(),
    }
}

fn photo_b64(photo: &Option<std::path::PathBuf>) -> String {
    let bytes = match photo {
        Some(p) => std::fs::read(p).unwrap(),
        None => mk_import::image::encode_png(&mk_import::engrave::test_photo(800)).unwrap(),
    };
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn app(dir: &std::path::Path, shot: &str, dark: bool, photo: &Option<std::path::PathBuf>) -> App {
    let theme = if dark {
        mk_ui::Theme::Dark
    } else {
        mk_ui::Theme::Light
    };
    if shot == "start-cards" {
        let p = |n: &str| dir.join(n).to_str().unwrap().to_owned();
        // A treated picture for the bakery, from the photo.
        let mut s = Session::create(
            &dir.join(format!("pic-{dark}.moneykeep")),
            "Pic",
            "USD",
            "app",
        )
        .unwrap();
        let info = s
            .invoke(
                "book.set_picture",
                json!({"source_png_base64": photo_b64(photo), "crop": {"x": 0, "y": 0, "size": 800},
                       "settings": {"background": 60}}),
            )
            .unwrap();
        let png = base64::engine::general_purpose::STANDARD
            .decode(info["icon"]["image_png_base64"].as_str().unwrap())
            .unwrap();
        let mut images = HashMap::new();
        images.insert(p("bakery.moneykeep"), png);
        let recent = vec![
            RecentBook {
                path: p("parish.moneykeep"),
                tile: Some(tile(
                    "St. Mark's Parish",
                    "Church office and building fund",
                    "Weekly offerings, the building fund and the youth trip. Kept by the parish office; the council sees the reports each month.",
                    TileIcon::Preset {
                        preset: "pastor".into(),
                    },
                    "purple",
                )),
                last_opened: Some("2026-11-07".into()),
            },
            RecentBook {
                path: p("household.moneykeep"),
                tile: Some(tile(
                    "Household",
                    "Family budget and savings",
                    "Checking, savings and the credit card.",
                    TileIcon::Preset {
                        preset: "family".into(),
                    },
                    "teal",
                )),
                last_opened: Some("2026-11-05".into()),
            },
            RecentBook {
                path: p("bakery.moneykeep"),
                tile: Some(tile(
                    "Corner Bakery LLC",
                    "Small business",
                    "Sales, payroll and the quarterly estimates.",
                    TileIcon::Image,
                    "amber",
                )),
                last_opened: Some("2026-10-30".into()),
            },
            RecentBook {
                path: p("missing-garden.moneykeep"),
                tile: Some(tile(
                    "Garden Club",
                    "Dues and plant sale",
                    "",
                    TileIcon::Letter { text: "G".into() },
                    "green",
                )),
                last_opened: Some("2026-09-12".into()),
            },
        ];
        let local = Local {
            session: None,
            protected: vec![p("household.moneykeep")],
            images,
        };
        return App::new(
            Box::new(local),
            UserConfig {
                recent,
                theme,
                ..UserConfig::default()
            },
        );
    }
    let path = dir.join(format!("{shot}-{dark}.moneykeep"));
    let mut s = Session::create(&path, "St. Mark's Parish", "USD", "app").unwrap();
    s.invoke(
        "book.update_profile",
        json!({"tagline": "Church office and building fund", "color": "purple",
               "description": "Weekly offerings, the building fund and the youth trip.",
               "icon": {"kind": "preset", "preset": "pastor"}}),
    )
    .unwrap();
    if shot == "picture-editor" {
        s.invoke(
            "book.set_picture",
            json!({"source_png_base64": photo_b64(photo), "crop": {"x": 0, "y": 0, "size": 800},
                   "settings": {"background": 60, "detail": 60}}),
        )
        .unwrap();
    }
    let local = Local {
        session: Some(s),
        protected: Vec::new(),
        images: HashMap::new(),
    };
    let cfg = UserConfig {
        theme,
        settings_section: "Book".into(),
        picture_tips_open: Some(false),
        ..UserConfig::default()
    };
    let mut a = App::new(Box::new(local), cfg);
    a.navigate(mk_ui::View::Settings);
    if shot == "picture-editor" {
        a.edit_book_picture().unwrap();
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
    let photo = std::env::args().nth(2).map(std::path::PathBuf::from);
    std::fs::create_dir_all(&out).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut jobs = Vec::new();
    for shot in ["start-cards", "book-settings-presets", "picture-editor"] {
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
        let a = app(&dir_path, shot, dark, &photo);
        let views = Views::new(a).unwrap();
        let (w, h) = match shot {
            "start-cards" => (1200.0, 900.0),
            "picture-editor" => (1100.0, 1000.0),
            _ => (1280.0, 1200.0),
        };
        views
            .window()
            .window()
            .set_size(slint::LogicalSize::new(w, h));
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
                    // No pointer over the window: no hover rule, no tooltip.
                    v.window()
                        .window()
                        .dispatch_event(slint::platform::WindowEvent::PointerExited);
                    v.tick(t);
                    v.refresh();
                }
                if t > 1.6 {
                    tm.stop();
                    if let Some(v) = cur.borrow_mut().take() {
                        match v.window().window().take_snapshot() {
                            Ok(buf) => {
                                let name =
                                    format!("{shot}-{}.png", if dark { "dark" } else { "light" });
                                let p = out.join(name);
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
