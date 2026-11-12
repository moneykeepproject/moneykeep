//! The phone capture app's window: shows the pages an app builds with mk-ui's
//! page builder in `PhoneWindow` and hands user actions back to it. The app keeps
//! its own logic (requests, outbox); this is only the glue.

use std::cell::{Cell, RefCell, RefMut};
use std::rc::{Rc, Weak};

use mk_ui::ctl::ui::{Action, Ev, Page, PageBuilder, Ui};
use slint::{ComponentHandle, SharedString};

use crate::convert::{Dialogs, Images, PageCache};
use crate::sl;

/// An app drawn in the phone window.
pub trait PageApp {
    /// Build the current page (answering the action in `ui`, if any).
    fn build(&mut self, ui: &mut Ui);
    /// Time passed: poll requests and timers; whether the page may have changed.
    fn tick(&mut self) -> bool {
        false
    }
    /// A key no element took (`name`: "escape", "ctrl+v" …); whether it was used.
    fn key(&mut self, _name: &str) -> bool {
        false
    }
    /// A key telling which screen this is (fresh models when it changes).
    fn screen_key(&self) -> String {
        String::new()
    }
}

#[derive(Default)]
struct Caches {
    images: Images,
    page: PageCache,
    dialogs: Dialogs,
}

struct Shared<A: PageApp> {
    window: sl::PhoneWindow,
    app: RefCell<A>,
    builder: RefCell<PageBuilder>,
    caches: RefCell<Caches>,
    refreshing: Cell<bool>,
    dirty: Cell<bool>,
    /// The bottom sheets shown (their ids), to give the window its keys back
    /// when one opens.
    sheets: RefCell<Vec<String>>,
}

/// The phone window with its app.
pub struct PhoneView<A: PageApp + 'static> {
    shared: Rc<Shared<A>>,
}

impl<A: PageApp + 'static> Clone for PhoneView<A> {
    fn clone(&self) -> Self {
        PhoneView {
            shared: Rc::clone(&self.shared),
        }
    }
}

impl<A: PageApp + 'static> std::fmt::Debug for PhoneView<A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PhoneView")
    }
}

impl<A: PageApp + 'static> PhoneView<A> {
    /// Create the window (not shown yet) over `app`.
    pub fn new(app: A) -> Result<Self, slint::PlatformError> {
        let window = sl::PhoneWindow::new()?;
        let shared = Rc::new(Shared {
            window,
            app: RefCell::new(app),
            builder: RefCell::new(PageBuilder::default()),
            caches: RefCell::new(Caches::default()),
            refreshing: Cell::new(false),
            dirty: Cell::new(false),
            sheets: RefCell::new(Vec::new()),
        });
        let weak: Weak<Shared<A>> = Rc::downgrade(&shared);
        let act = shared.window.global::<sl::Act>();
        let w = weak.clone();
        act.on_act(move |id: SharedString, ev: i32, value: SharedString| {
            if let Some(s) = w.upgrade() {
                PhoneView { shared: s }.act(&id, ev, &value);
            }
        });
        act.on_key(move |spec: SharedString| -> bool {
            let Some(s) = weak.upgrade() else {
                return false;
            };
            let name = spec.split_once('\n').map_or(spec.as_str(), |(_, n)| n);
            PhoneView { shared: s }.key(name)
        });
        let view = PhoneView { shared };
        view.refresh();
        Ok(view)
    }

    pub fn window(&self) -> &sl::PhoneWindow {
        &self.shared.window
    }

    /// The app for changes; call [`PhoneView::refresh`] afterwards.
    pub fn app_mut(&self) -> RefMut<'_, A> {
        self.shared.app.borrow_mut()
    }

    /// Change the app and refresh the window.
    pub fn with_app<R>(&self, f: impl FnOnce(&mut A) -> R) -> R {
        let r = f(&mut self.shared.app.borrow_mut());
        self.refresh();
        r
    }

    fn build(&self, action: Option<Action>) -> Option<(Page, String)> {
        let mut app = self.shared.app.try_borrow_mut().ok()?;
        let mut builder = self.shared.builder.try_borrow_mut().ok()?;
        let (page, _) = builder.build(action, |ui| app.build(ui));
        Some((page, app.screen_key()))
    }

    /// A user action from the window: answered in one build, then shown.
    pub fn act(&self, id: &str, ev: i32, value: &str) {
        let Some(ev) = Ev::from_code(ev) else {
            return;
        };
        let _ = self.build(Some(Action::new(id, ev, value)));
        self.refresh();
    }

    /// A key no element took.
    pub fn key(&self, name: &str) -> bool {
        let used = self
            .shared
            .app
            .try_borrow_mut()
            .is_ok_and(|mut a| a.key(name));
        if used {
            self.refresh();
        }
        used
    }

    /// Let time pass for the app; refresh when something changed.
    pub fn tick(&self) {
        let changed = self.shared.app.try_borrow_mut().is_ok_and(|mut a| a.tick());
        if changed {
            self.refresh();
        }
    }

    /// Rebuild the page and show it.
    pub fn refresh(&self) {
        if self.shared.refreshing.get() {
            self.shared.dirty.set(true);
            return;
        }
        self.shared.refreshing.set(true);
        loop {
            self.shared.dirty.set(false);
            if let Some((page, key)) = self.build(None) {
                self.apply(&page, &key);
            }
            if !self.shared.dirty.get() {
                break;
            }
        }
        self.shared.refreshing.set(false);
    }

    fn apply(&self, page: &Page, key: &str) {
        let w = &self.shared.window;
        let Ok(mut c) = self.shared.caches.try_borrow_mut() else {
            return;
        };
        let c = &mut *c;
        if let Some(p) = c.page.sync(page, key, &mut c.images) {
            w.set_page(p);
        }
        w.set_dialogs(c.dialogs.sync(&page.dialogs, &mut c.images));
        c.images.sweep();
        // A sheet that opens takes the keys from a field that had them (typing on
        // a computer goes to the keypad, and a phone's keyboard closes).
        let sheets: Vec<String> = page
            .dialogs
            .iter()
            .filter(|d| d.sheet != 0)
            .map(|d| d.id.clone())
            .collect();
        if let Ok(mut old) = self.shared.sheets.try_borrow_mut() {
            if sheets.iter().any(|id| !old.contains(id)) {
                w.set_focus_root(w.get_focus_root().wrapping_add(1));
            }
            *old = sheets;
        }
    }

    /// Show the window.
    pub fn show(&self) -> Result<(), slint::PlatformError> {
        self.shared.window.show()
    }

    /// Show the window and run Slint's event loop, ticking the app every
    /// `every_ms` milliseconds.
    pub fn run(&self, every_ms: u64) -> Result<(), slint::PlatformError> {
        let timer = slint::Timer::default();
        let me = self.clone();
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(every_ms),
            move || me.tick(),
        );
        self.shared.window.run()
    }
}
