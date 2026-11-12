//! From the controller's view model to Slint models.
//!
//! Lists are kept in long-lived `VecModel`s and updated row by row, only where
//! something changed, so repeated components (a field being typed into, a focused
//! button) stay the same instances and keep their focus and caret.

use std::collections::HashMap;
use std::rc::Rc;

use mk_ui::ctl::ui::{Block, Chart, Column, Dialog, Item, Page, Picture, Row, Section};
use slint::{Color, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

use crate::sl;

fn rgb(c: u32) -> Color {
    let [_, r, g, b] = c.to_be_bytes();
    Color::from_rgb_u8(r, g, b)
}

fn s(text: &str) -> SharedString {
    SharedString::from(text)
}

fn px(n: i32) -> f32 {
    // Pixels as Slint lengths (whole numbers, exact in f32).
    #[allow(clippy::cast_precision_loss)]
    {
        n as f32
    }
}

/// Pictures turned into Slint images, kept while they are shown.
#[derive(Default)]
pub(crate) struct Images {
    cache: HashMap<usize, (Rc<Picture>, Image)>,
    used: Vec<usize>,
}

impl Images {
    fn get(&mut self, p: &Rc<Picture>) -> Image {
        let key = Rc::as_ptr(p) as usize;
        self.used.push(key);
        if let Some((_, img)) = self.cache.get(&key) {
            return img.clone();
        }
        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&p.rgba, p.width, p.height);
        let img = Image::from_rgba8(buf);
        self.cache.insert(key, (p.clone(), img.clone()));
        img
    }

    /// Drop pictures not used since the last call.
    pub(crate) fn sweep(&mut self) {
        let used = std::mem::take(&mut self.used);
        self.cache.retain(|k, _| used.contains(k));
    }
}

fn chart(c: &Chart) -> sl::ChartVm {
    let paths: Vec<sl::ChartPathVm> = c
        .paths
        .iter()
        .map(|p| sl::ChartPathVm {
            commands: s(&p.commands),
            fill: p.fill.map_or(Color::from_argb_u8(0, 0, 0, 0), rgb),
            stroke: p.stroke.map_or(Color::from_argb_u8(0, 0, 0, 0), rgb),
            stroke_width: px(p.stroke_width),
            series: p.series,
            series_fill: p.series != -1 && p.fill.is_some(),
            series_stroke: p.series != -1 && p.stroke.is_some(),
            label: s(&p.label),
        })
        .collect();
    let labels: Vec<sl::ChartLabelVm> = c
        .labels
        .iter()
        .map(|l| sl::ChartLabelVm {
            text: s(&l.text),
            x: px(l.x),
            y: px(l.y),
            align: l.align as i32,
        })
        .collect();
    // The legend's lines, as the controller laid them out for the chart's width.
    let lines = c.legend.iter().map(|l| l.row + 1).max().unwrap_or(0);
    let legend: Vec<sl::LegendRowVm> = (0..lines)
        .map(|r| {
            let entries: Vec<sl::LegendVm> = c
                .legend
                .iter()
                .enumerate()
                .filter(|(_, l)| l.row == r)
                .map(|(i, l)| sl::LegendVm {
                    color: rgb(l.color),
                    series: l.series,
                    text: s(&l.text),
                    tip: s(&l.tip),
                    index: i32::try_from(i).unwrap_or(0),
                    name: s(&format!(
                        "{} (legend)",
                        if l.tip.is_empty() { &l.text } else { &l.tip }
                    )),
                })
                .collect();
            sl::LegendRowVm {
                entries: ModelRc::new(VecModel::from(entries)),
            }
        })
        .collect();
    let pieces: Vec<sl::ChartPieceVm> = c
        .pieces
        .iter()
        .map(|p| sl::ChartPieceVm {
            hx0: px(p.hit.0),
            hy0: px(p.hit.1),
            hx1: px(p.hit.2),
            hy1: px(p.hit.3),
            mx0: px(p.mark.0),
            my0: px(p.mark.1),
            mx1: px(p.mark.2),
            my1: px(p.mark.3),
            ring: p.ring,
            name: s(&p.name),
        })
        .collect();
    sl::ChartVm {
        paths: ModelRc::new(VecModel::from(paths)),
        labels: ModelRc::new(VecModel::from(labels)),
        legend: ModelRc::new(VecModel::from(legend)),
        height: px(c.height),
        note: s(&c.note),
        pieces: ModelRc::new(VecModel::from(pieces)),
        open_more: c.open_more,
        legend_count: i32::try_from(c.legend.len()).unwrap_or(0),
    }
}

/// One item.
pub(crate) fn item(it: &Item, images: &mut Images) -> sl::ItemVm {
    let options: Vec<SharedString> = it.options.iter().map(|o| s(o)).collect();
    let (picture, has_picture) = match &it.picture {
        Some(p) => (images.get(p), true),
        None => (Image::default(), false),
    };
    sl::ItemVm {
        kind: it.kind as i32,
        id: s(&it.id),
        text: s(&it.text),
        label: s(&it.label),
        value: s(&it.value),
        desc: s(&it.desc),
        placeholder: s(&it.placeholder),
        options: ModelRc::new(VecModel::from(options)),
        index: it.index,
        icon: s(&it.icon),
        checked: it.checked,
        enabled: it.enabled,
        tone: it.tone as i32,
        align: it.align as i32,
        mono: it.mono,
        bold: it.bold,
        strike: it.strike,
        width: px(it.width),
        grow: it.grow,
        focus: it.focus,
        select: it.select,
        picture,
        has_picture,
        chart: it.chart.as_deref().map(chart).unwrap_or_default(),
        number: it.number,
        minimum: it.minimum,
        maximum: it.maximum,
        error: s(&it.error),
        color: it.color.map_or(Color::from_argb_u8(0, 0, 0, 0), rgb),
        report_focus: it.report_focus,
        children: ModelRc::new(VecModel::from(
            it.children
                .iter()
                .map(|c| cell(c, images))
                .collect::<Vec<_>>(),
        )),
    }
}

/// One item of a cell group.
fn cell(it: &Item, images: &mut Images) -> sl::CellVm {
    let options: Vec<SharedString> = it.options.iter().map(|o| s(o)).collect();
    let (picture, has_picture) = match &it.picture {
        Some(p) => (images.get(p), true),
        None => (Image::default(), false),
    };
    sl::CellVm {
        kind: it.kind as i32,
        id: s(&it.id),
        text: s(&it.text),
        label: s(&it.label),
        value: s(&it.value),
        desc: s(&it.desc),
        placeholder: s(&it.placeholder),
        options: ModelRc::new(VecModel::from(options)),
        index: it.index,
        icon: s(&it.icon),
        checked: it.checked,
        enabled: it.enabled,
        tone: it.tone as i32,
        align: it.align as i32,
        mono: it.mono,
        bold: it.bold,
        strike: it.strike,
        width: px(it.width),
        grow: it.grow,
        focus: it.focus,
        select: it.select,
        picture,
        has_picture,
        chart: it.chart.as_deref().map(chart).unwrap_or_default(),
        number: it.number,
        minimum: it.minimum,
        maximum: it.maximum,
        error: s(&it.error),
        color: it.color.map_or(Color::from_argb_u8(0, 0, 0, 0), rgb),
        report_focus: it.report_focus,
    }
}

fn column(c: &Column) -> sl::ColumnVm {
    sl::ColumnVm {
        title: s(&c.title),
        align: c.align as i32,
        width: px(c.width),
        sort: c.sort,
        id: s(&c.id),
        weight: c.weight.max(1),
        over: s(&c.over),
    }
}

/// Bring `model` to `new`, converting changed entries with `conv`.
fn sync_flat<T: PartialEq + Clone, S: Clone + 'static>(
    model: &VecModel<S>,
    prev: &mut Vec<T>,
    new: &[T],
    mut conv: impl FnMut(&T) -> S,
) {
    for (i, n) in new.iter().enumerate() {
        match prev.get(i) {
            Some(p) if p == n => {}
            Some(_) => {
                model.set_row_data(i, conv(n));
                if let Some(slot) = prev.get_mut(i) {
                    *slot = n.clone();
                }
            }
            None => {
                model.push(conv(n));
                prev.push(n.clone());
            }
        }
    }
    while model.row_count() > new.len() {
        model.remove(model.row_count() - 1);
    }
    prev.truncate(new.len());
}

/// A list of items.
#[derive(Default)]
pub(crate) struct Items {
    model: Rc<VecModel<sl::ItemVm>>,
    prev: Vec<Item>,
}

impl Items {
    pub(crate) fn sync(&mut self, new: &[Item], images: &mut Images) -> ModelRc<sl::ItemVm> {
        sync_flat(&self.model, &mut self.prev, new, |it| item(it, images));
        // Pictures of unchanged items stay in use.
        for it in new {
            if let Some(p) = &it.picture {
                images.used.push(Rc::as_ptr(p) as usize);
            }
        }
        ModelRc::from(self.model.clone())
    }
}

/// A wrapping row's estimated widest item and total width (logical pixels): the
/// view lays it out on one line when it fits, else as a grid of equal cells.
fn wrap_estimate(r: &Row) -> (f32, f32) {
    use mk_ui::ctl::ui::Kind;
    if r.kind != mk_ui::ctl::ui::RowKind::Wrap {
        return (0.0, 0.0);
    }
    let est = |it: &mk_ui::ctl::ui::Item| -> f32 {
        let text = if it.text.is_empty() {
            &it.label
        } else {
            &it.text
        };
        #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
        let chars = text.chars().count().min(200) as f32 * 7.6;
        #[allow(clippy::cast_precision_loss, clippy::float_arithmetic)]
        let fixed = if it.width > 0 { it.width as f32 } else { 0.0 };
        #[allow(clippy::float_arithmetic)]
        let w = match it.kind {
            Kind::IconButton | Kind::Icon => 32.0,
            Kind::PictureChoice => 0.0,
            Kind::Button | Kind::Primary | Kind::Menu | Kind::Tab | Kind::Toggle => chars + 34.0,
            Kind::Check | Kind::Radio | Kind::Switch => chars + 34.0,
            Kind::Field | Kind::Combo | Kind::Suggest | Kind::Password => 160.0,
            Kind::Spacer | Kind::Separator => 0.0,
            _ => chars + 4.0,
        };
        w.max(fixed)
    };
    let widths: Vec<f32> = r.items.iter().map(est).collect();
    let cell = widths.iter().copied().fold(0.0_f32, f32::max);
    #[allow(clippy::float_arithmetic, clippy::cast_precision_loss)]
    let total = widths.iter().sum::<f32>() + 8.0 * widths.len().saturating_sub(1) as f32;
    (cell, total)
}

/// A list of rows (each with its items).
#[derive(Default)]
pub(crate) struct Rows {
    model: Rc<VecModel<sl::RowVm>>,
    prev: Vec<Row>,
    kids: Vec<Items>,
    fills: Vec<bool>,
}

impl Rows {
    pub(crate) fn sync(&mut self, new: &[Row], images: &mut Images) -> ModelRc<sl::RowVm> {
        for (i, r) in new.iter().enumerate() {
            if self.kids.len() <= i {
                self.kids.push(Items::default());
            }
            let items = match self.kids.get_mut(i) {
                Some(k) => k.sync(&r.items, images),
                None => ModelRc::default(),
            };
            let filler = !r
                .items
                .iter()
                .any(|it| it.grow || it.kind == mk_ui::ctl::ui::Kind::Spacer);
            let meta = Row {
                items: Vec::new(),
                ..r.clone()
            };
            let (wrap_cell, wrap_total) = wrap_estimate(r);
            let conv = |m: &Row| sl::RowVm {
                filler,
                wrap_cell,
                wrap_total,
                kind: m.kind as i32,
                id: s(&m.id),
                caption: s(&m.caption),
                label: s(&m.label),
                selected: m.selected,
                indent: m.indent,
                items: items.clone(),
                menu: ModelRc::new(VecModel::from(
                    m.menu.iter().map(|x| s(x)).collect::<Vec<_>>(),
                )),
                tone: m.tone as i32,
            };
            let same_fill = self.fills.get(i) == Some(&filler);
            if self.fills.len() <= i {
                self.fills.push(filler);
            } else if let Some(f) = self.fills.get_mut(i) {
                *f = filler;
            }
            match self.prev.get(i) {
                Some(p) if *p == meta && same_fill => {}
                Some(_) => {
                    self.model.set_row_data(i, conv(&meta));
                    if let Some(slot) = self.prev.get_mut(i) {
                        *slot = meta;
                    }
                }
                None => {
                    self.model.push(conv(&meta));
                    self.prev.push(meta);
                }
            }
        }
        while self.model.row_count() > new.len() {
            self.model.remove(self.model.row_count() - 1);
        }
        self.prev.truncate(new.len());
        self.kids.truncate(new.len());
        self.fills.truncate(new.len());
        ModelRc::from(self.model.clone())
    }
}

/// The rows of a windowed table: all rows count, only those in the window
/// carry data (the rest are blank until scrolled to).
#[derive(Default)]
pub(crate) struct WindowRows {
    total: std::cell::Cell<usize>,
    first: std::cell::Cell<usize>,
    rows: std::cell::RefCell<Vec<sl::RowVm>>,
    prev: std::cell::RefCell<Vec<Row>>,
    notify: slint::ModelNotify,
}

impl Model for WindowRows {
    type Data = sl::RowVm;
    fn row_count(&self) -> usize {
        self.total.get()
    }
    fn row_data(&self, row: usize) -> Option<sl::RowVm> {
        if row >= self.total.get() {
            return None;
        }
        let first = self.first.get();
        Some(
            row.checked_sub(first)
                .and_then(|i| self.rows.borrow().get(i).cloned())
                .unwrap_or_else(|| sl::RowVm {
                    kind: 3,
                    ..sl::RowVm::default()
                }),
        )
    }
    fn model_tracker(&self) -> &dyn slint::ModelTracker {
        &self.notify
    }
}

impl WindowRows {
    fn update(&self, total: usize, first: usize, rows: &[Row], images: &mut Images) {
        let old_total = self.total.get();
        let old_first = self.first.get();
        let old_len = self.rows.borrow().len();
        let conv = |r: &Row, images: &mut Images| {
            let items: Vec<sl::ItemVm> = r.items.iter().map(|it| item(it, images)).collect();
            sl::RowVm {
                kind: r.kind as i32,
                id: s(&r.id),
                caption: s(&r.caption),
                label: s(&r.label),
                selected: r.selected,
                indent: r.indent,
                items: ModelRc::new(VecModel::from(items)),
                menu: ModelRc::new(VecModel::from(
                    r.menu.iter().map(|x| s(x)).collect::<Vec<_>>(),
                )),
                tone: r.tone as i32,
                filler: false,
                ..sl::RowVm::default()
            }
        };
        if old_total != total {
            *self.rows.borrow_mut() = rows.iter().map(|r| conv(r, images)).collect();
            *self.prev.borrow_mut() = rows.to_vec();
            self.first.set(first);
            self.total.set(total);
            self.notify.reset();
            return;
        }
        // Indices whose data may change: the old and the new window.
        let mut changed: Vec<usize> = Vec::new();
        let prev = self.prev.borrow().clone();
        let mut fresh = Vec::with_capacity(rows.len());
        for (k, r) in rows.iter().enumerate() {
            let idx = first + k;
            let same = idx
                .checked_sub(old_first)
                .and_then(|j| prev.get(j))
                .is_some_and(|p| p == r);
            if !same {
                changed.push(idx);
            }
            fresh.push(conv(r, images));
        }
        for idx in old_first..old_first + old_len {
            if idx < first || idx >= first + rows.len() {
                changed.push(idx);
            }
        }
        *self.rows.borrow_mut() = fresh;
        *self.prev.borrow_mut() = rows.to_vec();
        self.first.set(first);
        for idx in changed {
            if idx < total {
                self.notify.row_changed(idx);
            }
        }
    }
}

/// The rows of one section: a plain list or a windowed table.
enum Kid {
    Plain(Rows),
    Window(Rc<WindowRows>),
}

impl Default for Kid {
    fn default() -> Self {
        Kid::Plain(Rows::default())
    }
}

/// A list of sections.
#[derive(Default)]
pub(crate) struct Sections {
    model: Rc<VecModel<sl::SectionVm>>,
    prev: Vec<Section>,
    kids: Vec<Kid>,
}

impl Sections {
    pub(crate) fn sync(&mut self, new: &[Section], images: &mut Images) -> ModelRc<sl::SectionVm> {
        for (i, sec) in new.iter().enumerate() {
            let windowed = sec.total_rows > 0;
            if self.kids.len() <= i {
                self.kids.push(Kid::default());
            }
            if let Some(k) = self.kids.get_mut(i) {
                let right = matches!(
                    (&*k, windowed),
                    (Kid::Window(_), true) | (Kid::Plain(_), false)
                );
                if !right {
                    *k = if windowed {
                        Kid::Window(Rc::new(WindowRows::default()))
                    } else {
                        Kid::Plain(Rows::default())
                    };
                    // The section's model changes: rebuild its entry.
                    if let Some(p) = self.prev.get_mut(i) {
                        p.id.push('\u{0}');
                    }
                }
            }
            let rows = match self.kids.get_mut(i) {
                Some(Kid::Plain(k)) => k.sync(&sec.rows, images),
                Some(Kid::Window(w)) => {
                    w.update(
                        usize::try_from(sec.total_rows).unwrap_or(0),
                        usize::try_from(sec.first_row).unwrap_or(0),
                        &sec.rows,
                        images,
                    );
                    ModelRc::from(w.clone() as Rc<dyn Model<Data = sl::RowVm>>)
                }
                None => ModelRc::default(),
            };
            let meta = Section {
                rows: Vec::new(),
                first_row: 0,
                ..sec.clone()
            };
            let conv = |m: &Section| sl::SectionVm {
                kind: m.kind as i32,
                id: s(&m.id),
                title: s(&m.title),
                subtitle: s(&m.subtitle),
                open: m.open,
                columns: ModelRc::new(VecModel::from(
                    m.columns.iter().map(column).collect::<Vec<_>>(),
                )),
                rows: rows.clone(),
                visible_rows: m.visible_rows,
                tone: m.tone as i32,
                width: px(m.width),
                center: m.center,
                total_rows: m.total_rows,
                first_row: m.first_row,
                scroll_row: m.scroll_row,
                scroll_serial: m.scroll_serial,
                row_height: px(m.row_height),
                frozen: m.frozen,
                flat: m.flat,
                header: m.columns.iter().any(|c| !c.title.is_empty()),
            };
            match self.prev.get(i) {
                Some(p) if *p == meta => {}
                Some(_) => {
                    self.model.set_row_data(i, conv(&meta));
                    if let Some(slot) = self.prev.get_mut(i) {
                        *slot = meta;
                    }
                }
                None => {
                    self.model.push(conv(&meta));
                    self.prev.push(meta);
                }
            }
        }
        while self.model.row_count() > new.len() {
            self.model.remove(self.model.row_count() - 1);
        }
        self.prev.truncate(new.len());
        self.kids.truncate(new.len());
        ModelRc::from(self.model.clone())
    }
}

/// The dialogs.
#[derive(Default)]
pub(crate) struct Dialogs {
    model: Rc<VecModel<sl::DialogVm>>,
    prev: Vec<Dialog>,
    kids: Vec<Sections>,
}

impl Dialogs {
    pub(crate) fn sync(&mut self, new: &[Dialog], images: &mut Images) -> ModelRc<sl::DialogVm> {
        for (i, d) in new.iter().enumerate() {
            // A different dialog in this slot starts fresh.
            if self.prev.get(i).is_some_and(|p| p.id != d.id) {
                self.kids.truncate(i);
                while self.model.row_count() > i {
                    self.model.remove(self.model.row_count() - 1);
                }
                self.prev.truncate(i);
            }
            if self.kids.len() <= i {
                self.kids.push(Sections::default());
            }
            let sections = match self.kids.get_mut(i) {
                Some(k) => k.sync(&d.sections, images),
                None => ModelRc::default(),
            };
            let meta = Dialog {
                sections: Vec::new(),
                ..d.clone()
            };
            let conv = |m: &Dialog| sl::DialogVm {
                id: s(&m.id),
                title: s(&m.title),
                sections: sections.clone(),
                width: px(m.width),
                close_label: s(&m.close_label),
                icon: s(&m.icon),
                compact: m.compact,
                sheet: m.sheet,
            };
            match self.prev.get(i) {
                Some(p) if *p == meta => {}
                Some(_) => {
                    self.model.set_row_data(i, conv(&meta));
                    if let Some(slot) = self.prev.get_mut(i) {
                        *slot = meta;
                    }
                }
                None => {
                    self.model.push(conv(&meta));
                    self.prev.push(meta);
                }
            }
        }
        while self.model.row_count() > new.len() {
            self.model.remove(self.model.row_count() - 1);
        }
        self.prev.truncate(new.len());
        self.kids.truncate(new.len());
        ModelRc::from(self.model.clone())
    }
}

/// Dashboard blocks: one stable row per block (an element keeps its state, such
/// as a drag in progress, while its data changes).
#[derive(Default)]
pub(crate) struct Board {
    model: Rc<VecModel<sl::BlockVm>>,
    prev: Vec<Block>,
    kids: Vec<(Items, Sections)>,
}

impl Board {
    pub(crate) fn sync(&mut self, new: &[Block], images: &mut Images) -> ModelRc<sl::BlockVm> {
        for (i, b) in new.iter().enumerate() {
            if self.kids.len() <= i {
                self.kids.push((Items::default(), Sections::default()));
            }
            let Some((head, body)) = self.kids.get_mut(i) else {
                continue;
            };
            let head = head.sync(&b.head, images);
            let sections = body.sync(&b.sections, images);
            let (gx, gy, gw, gh) = b.ghost.unwrap_or_default();
            let vm = sl::BlockVm {
                id: s(&b.id),
                title: s(&b.title),
                title_link: b.title_link,
                head,
                x: b.x,
                y: b.y,
                w: b.w,
                h: b.h,
                has_ghost: b.ghost.is_some(),
                gx,
                gy,
                gw,
                gh,
                sections,
            };
            let meta = Block {
                head: Vec::new(),
                sections: Vec::new(),
                ..b.clone()
            };
            match self.prev.get(i) {
                Some(p) if *p == meta => {}
                Some(_) => {
                    self.model.set_row_data(i, vm);
                    if let Some(slot) = self.prev.get_mut(i) {
                        *slot = meta;
                    }
                }
                None => {
                    self.model.push(vm);
                    self.prev.push(meta);
                }
            }
        }
        while self.model.row_count() > new.len() {
            self.model.remove(self.model.row_count() - 1);
            self.prev.pop();
            self.kids.pop();
        }
        ModelRc::from(self.model.clone())
    }
}

/// A page (title, toolbar, side list, sections, board).
#[derive(Default)]
pub(crate) struct PageCache {
    toolbar: Items,
    side: Sections,
    main: Sections,
    footer: Sections,
    tabs: Items,
    board: Board,
    prev: Option<(String, i32, bool, bool, i32)>,
    /// A different screen starts with fresh models.
    key: String,
}

impl PageCache {
    /// Update; returns the page struct when its own fields changed.
    pub(crate) fn sync(
        &mut self,
        page: &Page,
        key: &str,
        images: &mut Images,
    ) -> Option<sl::PageVm> {
        if self.key != key {
            *self = PageCache {
                key: key.to_owned(),
                ..PageCache::default()
            };
        }
        let toolbar = self.toolbar.sync(&page.toolbar, images);
        let side = self.side.sync(&page.side, images);
        let sections = self.main.sync(&page.sections, images);
        let footer = self.footer.sync(&page.footer, images);
        let tabs = self.tabs.sync(&page.tabs, images);
        let board = self.board.sync(&page.board, images);
        let rows = page.board.iter().map(|b| b.y + b.h).max().unwrap_or(0);
        let meta = (
            page.title.clone(),
            page.side_width,
            page.scroll,
            page.board_edit,
            rows,
        );
        if self.prev.as_ref() == Some(&meta) {
            return None;
        }
        self.prev = Some(meta);
        Some(sl::PageVm {
            title: s(&page.title),
            toolbar,
            side,
            side_width: px(page.side_width),
            sections,
            scroll: page.scroll,
            board,
            board_edit: page.board_edit,
            board_rows: rows,
            footer,
            tabs,
        })
    }
}
