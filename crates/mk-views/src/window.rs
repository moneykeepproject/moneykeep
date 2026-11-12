//! The desktop window: shows the controller's view model in `AppWindow` and
//! hands user actions back to it.

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::{Rc, Weak};

use mk_ui::ctl::{App, Ev, Vm};
use slint::{ComponentHandle, ModelRc, SharedString};

use crate::convert::{Dialogs, Images, Items, PageCache, Sections};
use crate::sl;

#[derive(Default)]
struct Caches {
    images: Images,
    page: PageCache,
    dialogs: Dialogs,
    top: Items,
    banner: Sections,
    /// The message pop-up fades out (revision 42): when it started (the
    /// controller's clock). The last message stays in the window until then.
    message_closing: Option<f64>,
    /// Whether the window shows a message (or one fading out).
    message_shown: bool,
    rail: Items,
    pane_tools: Items,
    pane: Sections,
    last: Option<Vm>,
    /// The screen the last view model showed (`page_key` and the dialog count).
    last_screen: Option<(String, usize)>,
    /// The scale factor the system gave the window, and the one last applied.
    base_scale: Option<f32>,
    applied_scale: Option<f32>,
}

struct Shared {
    window: sl::AppWindow,
    app: RefCell<App>,
    caches: RefCell<Caches>,
    refreshing: Cell<bool>,
    dirty: Cell<bool>,
    /// The window size (physical pixels) the last view model was built for.
    built_size: Cell<(u32, u32)>,
    /// Called after every user action (the app shell handles events then).
    after_action: RefCell<Option<Box<dyn Fn()>>>,
    /// The controller's clock at the last tick.
    now: Cell<f64>,
}

/// The main window with its controller.
#[derive(Clone)]
pub struct Views {
    shared: Rc<Shared>,
}

impl std::fmt::Debug for Views {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Views")
    }
}

/// How long the message pop-up fades out, in seconds (revision 42).
const MESSAGE_FADE: f64 = 0.15;

/// A key for "which screen is this" (fresh models when it changes).
fn page_key(app: &App) -> String {
    format!(
        "{:?}/{:?}",
        app.screen(),
        std::mem::discriminant(app.view())
    )
}

impl Views {
    /// Create the window (not shown yet) over `app`.
    pub fn new(app: App) -> Result<Self, slint::PlatformError> {
        let window = sl::AppWindow::new()?;
        let shared = Rc::new(Shared {
            window,
            app: RefCell::new(app),
            caches: RefCell::new(Caches::default()),
            refreshing: Cell::new(false),
            dirty: Cell::new(false),
            built_size: Cell::new((0, 0)),
            after_action: RefCell::new(None),
            now: Cell::new(0.0),
        });
        let weak: Weak<Shared> = Rc::downgrade(&shared);
        let act = shared.window.global::<sl::Act>();
        let w = weak.clone();
        act.on_act(move |id: SharedString, ev: i32, value: SharedString| {
            if let Some(s) = w.upgrade() {
                let views = Views { shared: s };
                views.act(&id, ev, &value);
            }
        });
        let w = weak;
        act.on_key(move |spec: SharedString| -> bool {
            let Some(s) = w.upgrade() else {
                return false;
            };
            let views = Views { shared: s };
            let (item, name) = spec.split_once('\n').unwrap_or(("", spec.as_str()));
            views.key(item, name)
        });
        let views = Views { shared };
        views.refresh();
        Ok(views)
    }

    pub fn window(&self) -> &sl::AppWindow {
        &self.shared.window
    }

    /// The controller (do not hold the borrow across calls into the window).
    pub fn app(&self) -> Ref<'_, App> {
        self.shared.app.borrow()
    }

    /// The controller for changes; call [`Views::refresh`] afterwards.
    pub fn app_mut(&self) -> RefMut<'_, App> {
        self.shared.app.borrow_mut()
    }

    /// Change the controller and refresh the window.
    pub fn with_app<R>(&self, f: impl FnOnce(&mut App) -> R) -> R {
        let r = f(&mut self.shared.app.borrow_mut());
        self.refresh();
        r
    }

    /// Run `f` after every user action (the app shell reads events then).
    pub fn on_after_action(&self, f: impl Fn() + 'static) {
        *self.shared.after_action.borrow_mut() = Some(Box::new(f));
    }

    fn after(&self) {
        if let Some(f) = self.shared.after_action.borrow().as_ref() {
            f();
        }
    }

    /// A user action from the views.
    pub fn act(&self, id: &str, ev: i32, value: &str) {
        let Some(ev) = Ev::from_code(ev) else {
            return;
        };
        if let Ok(mut app) = self.shared.app.try_borrow_mut() {
            app.note_input();
            app.act(id, ev, value);
        }
        self.refresh();
        self.after();
    }

    /// A key no element took (`item` saw it first, or empty).
    pub fn key(&self, item: &str, name: &str) -> bool {
        let used = match self.shared.app.try_borrow_mut() {
            Ok(mut app) => app.key(item, name),
            Err(_) => false,
        };
        if used {
            self.refresh();
            self.after();
        }
        used
    }

    /// Advance the controller's clock; refresh when something changed.
    pub fn tick(&self, now: f64) {
        let changed = match self.shared.app.try_borrow_mut() {
            Ok(mut app) => app.tick(now),
            Err(_) => false,
        };
        // A resized window: layouts that depend on its size (the dashboard grid,
        // the calculator's place) are rebuilt.
        self.shared.now.set(now);
        let size = self.shared.window.window().size();
        let resized = self.shared.built_size.get() != (size.width, size.height);
        // A message pop-up that finished fading out leaves the window.
        #[allow(clippy::float_arithmetic)]
        let faded = self
            .shared
            .caches
            .try_borrow()
            .ok()
            .and_then(|c| c.message_closing)
            .is_some_and(|t| now - t >= MESSAGE_FADE);
        if faded && let Ok(mut c) = self.shared.caches.try_borrow_mut() {
            let c = &mut *c;
            c.message_closing = None;
            c.message_shown = false;
            self.shared
                .window
                .set_banner(c.banner.sync(&[], &mut c.images));
        }
        if changed || resized {
            self.refresh();
            self.after();
        }
    }

    /// Rebuild the view model and show it.
    pub fn refresh(&self) {
        if self.shared.refreshing.get() {
            self.shared.dirty.set(true);
            return;
        }
        self.shared.refreshing.set(true);
        loop {
            self.shared.dirty.set(false);
            let built = match self.shared.app.try_borrow_mut() {
                Ok(mut app) => {
                    let win = self.shared.window.window();
                    self.shared
                        .built_size
                        .set((win.size().width, win.size().height));
                    let scale = win.scale_factor().max(0.5);
                    #[allow(clippy::float_arithmetic, clippy::cast_possible_truncation)]
                    let width = (win.size().width as f32 / scale) as i32;
                    if width > 0 {
                        app.set_window_width(width);
                    }
                    #[allow(clippy::float_arithmetic, clippy::cast_possible_truncation)]
                    let height = (win.size().height as f32 / scale) as i32;
                    if height > 0 {
                        app.set_window_height(height);
                    }
                    Some((app.vm(), page_key(&app)))
                }
                Err(_) => None,
            };
            if let Some((vm, key)) = built {
                self.apply(&vm, &key);
            }
            if !self.shared.dirty.get() {
                break;
            }
        }
        self.shared.refreshing.set(false);
    }

    fn apply(&self, vm: &Vm, key: &str) {
        let w = &self.shared.window;
        let Ok(mut c) = self.shared.caches.try_borrow_mut() else {
            return;
        };
        let c = &mut *c;
        // Another screen or dialog: the element a tooltip named is gone (it never
        // saw the pointer leave), so its tooltip goes too.
        let screen = (key.to_owned(), vm.page.dialogs.len());
        if c.last_screen.as_ref() != Some(&screen) {
            if c.last_screen.is_some() {
                w.global::<sl::Act>().set_tip(SharedString::new());
            }
            c.last_screen = Some(screen);
        }
        if let Some(page) = c.page.sync(&vm.page, key, &mut c.images) {
            w.set_page(page);
        }
        w.set_dialogs(c.dialogs.sync(&vm.page.dialogs, &mut c.images));
        w.set_screen(i32::from(vm.main));
        if vm.main {
            w.set_top_items(c.top.sync(&vm.top, &mut c.images));
            Self::apply_message(w, c, vm, self.shared.now.get());
            w.set_rail_items(c.rail.sync(&vm.rail, &mut c.images));
            w.set_pane_open(vm.pane.open);
            w.set_pane_pinned(vm.pane.pinned);
            w.set_pane_title(vm.pane.title.as_str().into());
            #[allow(clippy::cast_precision_loss)]
            w.set_pane_width(vm.pane.width as f32);
            w.set_pane_tools(c.pane_tools.sync(&vm.pane.tools, &mut c.images));
            w.set_pane_sections(c.pane.sync(&vm.pane.sections, &mut c.images));
        } else {
            w.set_top_items(ModelRc::default());
            w.set_rail_items(ModelRc::default());
            w.set_pane_open(false);
        }
        w.set_calc_open(vm.calc.open);
        #[allow(clippy::cast_precision_loss)]
        w.set_calc(sl::CalcVm {
            display: vm.calc.display.as_str().into(),
            expression: vm.calc.expression.as_str().into(),
            error: vm.calc.error,
            x: vm.calc.x as f32,
            y: vm.calc.y as f32,
            pinned: vm.calc.pinned,
            focus: vm.calc.focus,
        });
        if c.last.as_ref().map(|l| &l.display) != Some(&vm.display) {
            w.set_theme_choice(vm.display.theme);
            w.set_ui_font(vm.display.ui_font.clone().unwrap_or_default().into());
            w.set_number_font(vm.display.number_font.clone().unwrap_or_default().into());
            #[allow(clippy::cast_precision_loss)]
            w.set_scroll_width(vm.display.scroll_width as f32);
            Self::apply_scale(w, c, vm.display.text_size);
        }
        w.set_focus_root(vm.focus_root);
        c.images.sweep();
        c.last = Some(vm.clone());
    }

    /// The message pop-up (revision 42): a new message shows at once (fading
    /// in); a closed one stays while it fades out, then [`Views::tick`] removes
    /// it. With reduced motion it goes at once.
    fn apply_message(w: &sl::AppWindow, c: &mut Caches, vm: &Vm, now: f64) {
        w.set_message_over_dialog(vm.message_over_dialog);
        let reduce = w.global::<sl::Theme>().get_reduce_motion();
        if vm.banner.is_empty() && c.message_shown && !reduce {
            if c.message_closing.is_none() {
                c.message_closing = Some(now);
            }
            w.set_message_open(false);
            return;
        }
        c.message_closing = None;
        c.message_shown = !vm.banner.is_empty();
        w.set_banner(c.banner.sync(&vm.banner, &mut c.images));
        w.set_message_open(c.message_shown);
    }

    /// Text size maps to the window scale factor (ui-slint.md, revision 7).
    fn apply_scale(w: &sl::AppWindow, c: &mut Caches, percent: u32) {
        let current = w.window().scale_factor();
        let base = match (c.base_scale, c.applied_scale) {
            // The system changed the scale since we set it.
            (Some(_), Some(applied)) if (current - applied).abs() > 0.001 => current,
            (Some(b), _) => b,
            (None, _) => current,
        };
        c.base_scale = Some(base);
        #[allow(clippy::cast_precision_loss)]
        let want = base * (percent as f32) / 100.0;
        if (current - want).abs() > 0.001 {
            w.window()
                .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
                    scale_factor: want,
                });
            // The same pixels are fewer logical pixels now: lay out for them.
            let size = w.window().size().to_logical(want);
            w.window()
                .dispatch_event(slint::platform::WindowEvent::Resized { size });
        }
        c.applied_scale = Some(want);
    }

    /// Show the window.
    pub fn show(&self) -> Result<(), slint::PlatformError> {
        self.shared.window.show()
    }
}
