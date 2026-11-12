//! The view-model builder (ui-slint.md revision 2).
//!
//! Screens describe themselves as plain data: pages of sections (cards, tables,
//! tile grids), rows and items (texts, buttons, fields, …). The `.slint` views in
//! `mk-views` show that data with Slint's layouts and Fluent widgets, and report
//! what the user did as an [`Action`] (an item id, what happened, and a value).
//!
//! A screen is one function over the application state and a [`Ui`]. The
//! controller runs it twice per user action: once with the action, where the item
//! the action names answers `clicked()` / `changed()` and the function does what
//! the action means, and once without, to build the fresh view model. Item ids are
//! the scope plus the item's label (with a counter for repeats), so the same state
//! always yields the same ids.
//!
//! Nothing here knows about Slint; it is plain Rust, tested natively.

use std::collections::HashMap;
use std::rc::Rc;

/// What an item is. The numbers are shared with `mk-views` (`ui/vm.slint`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum Kind {
    #[default]
    Text = 0,
    Title = 1,
    Subtitle = 2,
    Caption = 3,
    Button = 4,
    Primary = 5,
    Link = 6,
    Field = 7,
    Password = 8,
    Multiline = 9,
    Check = 10,
    Radio = 11,
    Combo = 12,
    /// A navigation or section tab (selectable).
    Tab = 13,
    /// A selectable chip / toggle button.
    Toggle = 14,
    /// An icon button (`icon` names the drawing).
    IconButton = 15,
    Separator = 16,
    Spacer = 17,
    Progress = 18,
    Image = 19,
    /// An amount: number font, right-aligned.
    Amount = 20,
    /// A button opening a menu of `options`.
    Menu = 21,
    /// A small coloured label.
    Badge = 22,
    /// A switch (on/off).
    Switch = 23,
    /// A book tile (start screen) or a feature tile.
    Tile = 24,
    /// A chart (`chart` holds its drawing).
    Chart = 25,
    /// A plain icon (not clickable).
    Icon = 26,
    /// A text field with suggestions below it (`options`).
    Suggest = 27,
    /// A list entry (whole-row click target).
    ListItem = 28,
    /// Slint's AboutSlint widget.
    AboutSlint = 29,
    /// A status light (phone capture).
    Light = 30,
    /// A slider (`value` holds the number).
    Slider = 31,
    /// A menu item with a check mark (the Features menu).
    MenuCheck = 32,
    /// Several items in one table cell (`children`).
    Group = 33,
    /// A square action tile (the phone's home screen): `icon` over `text` on
    /// `color`, a count badge from `number`.
    ActionTile = 34,
    /// A keypad key (the phone's amount keypad, mobile-capture.md revision 7):
    /// `text` or `icon` on it; `value` is its style: empty the normal key, "op"
    /// accent-tinted, "primary", "quiet".
    Key = 35,
    /// A calendar day: `text` the day number, `checked` selected, `value` "today"
    /// rings it, tone Secondary mutes it (another month), disabled when out of range.
    Day = 36,
    /// The phone's Amount card, one button: `text` the amount, `value` the
    /// currency symbol, `placeholder` the caption; tone Secondary while unset.
    AmountCard = 37,
    /// A button drawn like a field (the phone's Date): `text`, `icon` at the end,
    /// `placeholder` the caption.
    FieldButton = 38,
    /// A receipt thumbnail with a remove (×) button: `picture` (or `text` when
    /// there is none); a click carries the value "remove" for the ×, `desc` names
    /// the × for screen readers.
    Thumb = 39,
    /// A tab of the phone's bottom tab bar (mobile-capture.md revision 9): `icon`
    /// over `text`; `checked` the current tab; a count badge from `number`.
    BottomTab = 40,
    /// A book card (the start screen and the Settings preview, app-desktop.md
    /// revision 40): `text` the name, `label` the accessible name, `desc` the
    /// accessible description, `placeholder` the tooltip, `value` the letter (or "+"
    /// for the Create card), `icon` a symbol (`sym-…`) or preset picture
    /// (`preset-…`), `picture` a stored picture, `color` the book's colour,
    /// `checked` a failed book, `options` the … menu. `children` hold the texts, by
    /// position: [`CARD_BADGE`], [`CARD_CURRENCY`], [`CARD_TAGLINE`],
    /// [`CARD_STATUS`] (tone Error red, else blue), [`CARD_DESCRIPTION`],
    /// [`CARD_LAST_OPENED`], [`CARD_BUTTON`], [`CARD_LINK`]. A click carries the
    /// value "" (the card), "open", "remove" or "folder".
    BookCard = 41,
    /// A square picture choice: `icon` a preset picture (`preset-…`) on `color`;
    /// `checked` chosen.
    PictureChoice = 42,
    /// The Picture editor's crop frame (`width` square): `picture` the photo and
    /// the crop square in its pixels — `number` x, `minimum` y, `maximum` side. A
    /// drag sends Edit with "dx,dy" in frame pixels.
    CropFrame = 43,
    /// A picture on `color` (`width` square) that a brush paints on while
    /// `checked`: `number` the brush radius in 1/256 of the side; a stroke sends
    /// Edit with "x,y;x,y;…" in 0–255.
    PaintCanvas = 44,
}

/// The positions of a [`Kind::BookCard`]'s texts in its `children`.
pub const CARD_BADGE: usize = 0;
pub const CARD_CURRENCY: usize = 1;
pub const CARD_TAGLINE: usize = 2;
pub const CARD_STATUS: usize = 3;
pub const CARD_DESCRIPTION: usize = 4;
pub const CARD_LAST_OPENED: usize = 5;
pub const CARD_BUTTON: usize = 6;
pub const CARD_LINK: usize = 7;

/// Text colour / emphasis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum Tone {
    #[default]
    Normal = 0,
    Secondary = 1,
    Error = 2,
    Success = 3,
    Warning = 4,
    Accent = 5,
    /// Amount decreases (negative numbers).
    Decrease = 6,
    /// Amount increases.
    Increase = 7,
}

/// Horizontal alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum Align {
    #[default]
    Left = 0,
    Center = 1,
    Right = 2,
}

/// An RGBA picture (tile pictures, receipts, logos).
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// Row-major RGBA, 4 bytes per pixel.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}x{})", self.width, self.height)
    }
}

/// A chart drawing in a 1000 × 1000 view box: filled and stroked paths (SVG path
/// syntax) with their colours, and labels placed in the same box.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chart {
    pub paths: Vec<ChartPath>,
    pub labels: Vec<ChartLabel>,
    /// The drawing's height in pixels (its width follows the card); the legend's
    /// lines come under it.
    pub height: i32,
    /// Legend entries, laid out in lines (`row`).
    pub legend: Vec<LegendEntry>,
    /// An accessible note read with the chart (the axis ticks).
    pub note: String,
    /// The pieces a click opens (dashboards.md revision 5), in reading order; empty
    /// when the chart's data opens nothing.
    pub pieces: Vec<ChartPiece>,
    /// A click on a legend entry or the empty chart area opens something too.
    pub open_more: bool,
}

/// A clickable piece of a chart: a bar or a point.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChartPiece {
    /// Where the pointer hits it, in the drawing's 0–1000 box (at least 24 × 24
    /// pixels, so small points are easy to hit).
    pub hit: (i32, i32, i32, i32),
    /// What lights up under the pointer or keyboard, in the same box: the bar, or a
    /// ring around the point.
    pub mark: (i32, i32, i32, i32),
    pub ring: bool,
    /// Its label and value ("Groceries — 412.50, Oct 2026"): the accessible name
    /// and the tooltip.
    pub name: String,
}

/// What a click on a chart hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartHit {
    /// A piece (its index in [`Chart::pieces`]).
    Piece(usize),
    /// A legend entry (its index in [`Chart::legend`]).
    Legend(usize),
    /// The empty chart area.
    Background,
}

impl ChartHit {
    /// From the value a chart's click sends ("p:2", "l:0" or empty).
    pub fn from_value(v: &str) -> Option<ChartHit> {
        if v.is_empty() {
            return Some(ChartHit::Background);
        }
        let (k, n) = v.split_once(':')?;
        let n: usize = n.parse().ok()?;
        match k {
            "p" => Some(ChartHit::Piece(n)),
            "l" => Some(ChartHit::Legend(n)),
            _ => None,
        }
    }
}

/// A legend entry: a series colour (or a colour of its own) and its text.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LegendEntry {
    /// Index into the theme's series colours, or -1 to use `color`.
    pub series: i32,
    pub color: u32,
    pub text: String,
    /// The legend line it sits on (0 first), as laid out for the chart's width.
    pub row: i32,
    /// The full text when `text` was shortened to fit, else empty.
    pub tip: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChartPath {
    pub commands: String,
    /// 0xRRGGBB; with `series` ≥ 0 a `Some` means "fill in the series colour".
    pub fill: Option<u32>,
    pub stroke: Option<u32>,
    pub stroke_width: i32,
    /// Index into the theme's series colours, or -1.
    pub series: i32,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChartLabel {
    pub text: String,
    /// Position in the view box (0–1000).
    pub x: i32,
    pub y: i32,
    pub align: Align,
}

/// One item: a text, a button, a field …
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: Kind,
    /// The action id (empty for items that do nothing).
    pub id: String,
    /// The shown text.
    pub text: String,
    /// The accessible name (the shown text when empty).
    pub label: String,
    /// A field's text, a combo's current choice, a light's state.
    pub value: String,
    /// Accessible description and tooltip.
    pub desc: String,
    pub placeholder: String,
    /// Choices of a combo, menu or suggestions.
    pub options: Vec<String>,
    /// The index of the chosen option (combo), or -1.
    pub index: i32,
    /// Icon name (see `mk-views/ui/icons.slint`).
    pub icon: String,
    pub checked: bool,
    pub enabled: bool,
    pub tone: Tone,
    pub align: Align,
    pub mono: bool,
    pub bold: bool,
    pub strike: bool,
    /// Preferred width in pixels (0: natural).
    pub width: i32,
    /// Take the remaining width of the row.
    pub grow: bool,
    /// Focus request serial (>0: focus when it changes).
    pub focus: i32,
    /// Select-all request serial.
    pub select: i32,
    pub picture: Option<Rc<Picture>>,
    pub chart: Option<Rc<Chart>>,
    /// Numbers for sliders and progress (in hundredths).
    pub number: i32,
    pub minimum: i32,
    pub maximum: i32,
    /// Shown when the item has the error state (a field's refusal).
    pub error: String,
    /// A colour of its own (tiles, legend swatches), 0xRRGGBB.
    pub color: Option<u32>,
    /// The items of a cell group.
    pub children: Vec<Item>,
    /// The views report keyboard focus on this item (the message band's buttons,
    /// app-desktop.md revision 37).
    pub report_focus: bool,
}

impl Default for Item {
    fn default() -> Self {
        Item {
            kind: Kind::Text,
            id: String::new(),
            text: String::new(),
            label: String::new(),
            value: String::new(),
            desc: String::new(),
            placeholder: String::new(),
            options: Vec::new(),
            index: -1,
            icon: String::new(),
            checked: false,
            enabled: true,
            tone: Tone::Normal,
            align: Align::Left,
            mono: false,
            bold: false,
            strike: false,
            width: 0,
            grow: false,
            focus: 0,
            select: 0,
            picture: None,
            chart: None,
            number: 0,
            minimum: 0,
            maximum: 0,
            error: String::new(),
            color: None,
            children: Vec::new(),
            report_focus: false,
        }
    }
}

impl Item {
    /// The accessible name.
    pub fn name(&self) -> &str {
        if self.label.is_empty() {
            &self.text
        } else {
            &self.label
        }
    }
}

/// How a row lays out its items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum RowKind {
    /// Items side by side.
    #[default]
    Plain = 0,
    /// A caption column, then the items (forms).
    Form = 1,
    /// Items side by side, wrapping onto more lines when narrow.
    Wrap = 2,
    /// A table row: one item per column.
    Table = 3,
    /// A clickable list entry (the whole row is the target).
    List = 4,
    /// Buttons at the end of a card or dialog (right-aligned).
    Actions = 5,
}

/// The id of a text that is part of its row's accessible name (not exposed on its own).
pub const QUIET: &str = "~";

/// A line of items.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub kind: RowKind,
    /// The action id of a clickable row (`List`, or a `Table` row that opens).
    pub id: String,
    /// Form rows: the caption before the items.
    pub caption: String,
    /// Accessible name of a clickable row.
    pub label: String,
    pub selected: bool,
    /// Indentation steps (tree levels).
    pub indent: i32,
    pub items: Vec<Item>,
    /// Context menu entries (right-click).
    pub menu: Vec<String>,
    pub tone: Tone,
}

/// What a section is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum SectionKind {
    /// Rows straight on the page.
    #[default]
    Plain = 0,
    /// A rounded card with an optional title.
    Card = 1,
    /// A table with column headers.
    Table = 2,
    /// A grid of tiles (each row's items are tiles, laid out in columns).
    Tiles = 3,
    /// A card whose content can be folded away (`open`).
    Collapsing = 4,
    /// A notice band (messages).
    Banner = 5,
    /// A grid of equal columns (`columns.len()` of them), rows `row_height` tall;
    /// an item spans `maximum` columns (0 counts as 1) from column `minimum`,
    /// both set by [`Ui::grid`] (the phone's keypad and calendar).
    Grid = 6,
}

/// A table column.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Column {
    pub title: String,
    pub align: Align,
    /// Width in pixels (0: share the rest).
    pub width: i32,
    /// 0 unsorted, 1 ascending, 2 descending.
    pub sort: i32,
    /// Action id for a header click (sorting); empty when not sortable.
    pub id: String,
    /// Share of the spare width for a column without a width (0 counts as 1).
    pub weight: i32,
    /// A second, smaller line above the title (period tables: the year).
    pub over: String,
}

/// A block of rows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Section {
    pub kind: SectionKind,
    pub id: String,
    pub title: String,
    pub subtitle: String,
    /// Collapsing sections: whether the content shows.
    pub open: bool,
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    /// Tables: rows shown before the table scrolls (0: all).
    pub visible_rows: i32,
    /// Tone of a banner.
    pub tone: Tone,
    /// Width hint in pixels (0: full width).
    pub width: i32,
    /// Tile grids: centred in the width.
    pub center: bool,
    /// Windowed tables (registers): all rows, the first row in `rows`, and a
    /// scroll request (row, serial).
    pub total_rows: i32,
    pub first_row: i32,
    pub scroll_row: i32,
    pub scroll_serial: i32,
    /// Row height in pixels for windowed tables.
    pub row_height: i32,
    /// Period tables: 0 an ordinary table; 1 the first column stays put while the
    /// others scroll sideways; 2 the last column (Total) stays put too.
    pub frozen: i32,
    /// Drawn without a card of its own (inside a dashboard block).
    pub flat: bool,
}

/// A dialog (modal window over the page).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dialog {
    pub id: String,
    pub title: String,
    pub sections: Vec<Section>,
    /// Preferred width in pixels.
    pub width: i32,
    /// Accessible name of the title bar's × button.
    pub close_label: String,
    /// An icon before the title (the password prompt's lock).
    pub icon: String,
    /// A compact dialog whose body sits centred in the card (the password
    /// prompt, app-desktop.md revision 32).
    pub compact: bool,
    /// 0 a dialog; 1 a bottom sheet over a dimmed screen (a tap on the dimmed
    /// part closes it); 2 a full-screen viewer (any tap closes it). The phone.
    pub sheet: i32,
}

/// A page: a heading, optional side list, sections and dialogs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    pub title: String,
    /// Items after the title (toolbar).
    pub toolbar: Vec<Item>,
    /// A list beside the content (Settings sections, report list).
    pub side: Vec<Section>,
    pub side_width: i32,
    pub sections: Vec<Section>,
    pub dialogs: Vec<Dialog>,
    /// Scroll the content (false for pages holding their own scrolling table).
    pub scroll: bool,
    /// Dashboard blocks on a snap grid, after the sections.
    pub board: Vec<Block>,
    /// The board is being edited (blocks drag and resize).
    pub board_edit: bool,
    /// A bar fixed to the bottom of the screen (the phone form's Send and Save).
    pub footer: Vec<Section>,
    /// The phone's tab bar, under the footer at the very bottom (its
    /// [`Kind::BottomTab`] items).
    pub tabs: Vec<Item>,
}

/// A dashboard block: a cell on the grid (columns of [`BOARD_COLUMNS`], rows of
/// [`BOARD_ROW`] pixels), a title bar with items, and its own sections.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Block {
    /// Action id of the title bar (drag handle, keyboard moves).
    pub id: String,
    pub title: String,
    /// The title opens something (a click on it acts on `id`).
    pub title_link: bool,
    /// Items at the right of the title bar.
    pub head: Vec<Item>,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Where a dragged or resized block would land.
    pub ghost: Option<(i32, i32, i32, i32)>,
    pub sections: Vec<Section>,
}

/// Board grid columns.
pub const BOARD_COLUMNS: i32 = 12;
/// Board row height in pixels (about four text lines).
pub const BOARD_ROW: i32 = 96;

/// What the user did to an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ev {
    Click,
    /// A field's text changed (`value` is the new text).
    Edit,
    /// Enter pressed in a field.
    Enter,
    Focus,
    Blur,
    /// An option chosen (`value` is its index).
    Select,
    /// Right-click / context menu entry (`value`: entry index, or empty to open).
    Secondary,
    /// A key while the item had focus (`value`: key name, e.g. `down`, `escape`).
    Key,
}

impl Ev {
    /// From the number the views send.
    pub fn from_code(n: i32) -> Option<Ev> {
        Some(match n {
            0 => Ev::Click,
            1 => Ev::Edit,
            2 => Ev::Enter,
            3 => Ev::Focus,
            4 => Ev::Blur,
            5 => Ev::Select,
            6 => Ev::Secondary,
            7 => Ev::Key,
            _ => return None,
        })
    }
}

/// One user action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub id: String,
    pub ev: Ev,
    pub value: String,
}

impl Action {
    pub fn new(id: &str, ev: Ev, value: &str) -> Self {
        Action {
            id: id.to_owned(),
            ev,
            value: value.to_owned(),
        }
    }
}

/// What happened to the item just added (only ever true in the action pass).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resp {
    pub id: String,
    pub clicked: bool,
    pub changed: bool,
    pub submitted: bool,
    pub focused: bool,
    pub blurred: bool,
    /// Right-click; `Some(i)` when context menu entry `i` was chosen.
    pub secondary: Option<Option<usize>>,
    /// The option chosen (combos and menus).
    pub chosen: Option<usize>,
    /// The option chosen, by its text (drop-down lists report the text).
    pub chosen_text: Option<String>,
    /// A key pressed while focused.
    pub key: Option<String>,
    /// The value sent with a click (a table row: the column clicked) or an edit.
    pub value: String,
}

impl Resp {
    pub fn clicked(&self) -> bool {
        self.clicked
    }
    pub fn changed(&self) -> bool {
        self.changed
    }
}

/// Attributes for the next item (`ui.next()`).
#[derive(Debug, Clone, Default)]
pub struct Next {
    enabled: Option<bool>,
    desc: Option<String>,
    label: Option<String>,
    width: Option<i32>,
    grow: bool,
    tone: Option<Tone>,
    align: Option<Align>,
    mono: bool,
    bold: bool,
    strike: bool,
    focus: bool,
    select: bool,
    placeholder: Option<String>,
    error: Option<String>,
    id: Option<String>,
    icon: Option<String>,
    quiet: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Main,
    Side,
    Dialog(usize),
    Block(usize),
    Footer,
}

/// Builds a page and answers the pending action.
#[derive(Debug)]
pub struct Ui {
    action: Option<Action>,
    consumed: bool,
    page: Page,
    target: Target,
    /// Section being filled in the current target.
    section: Option<usize>,
    /// The row being filled (inside `row`).
    row: Option<Row>,
    scope: Vec<String>,
    seen: HashMap<String, u32>,
    next: Next,
    /// Focus wanted on an item id, with its serial.
    focus_want: Option<(String, i32)>,
    /// Focus requests made while building (ids).
    pub(crate) focus_requests: Vec<String>,
    /// Select-all requests made while building (ids).
    pub(crate) select_requests: Vec<String>,
    select_want: Option<(String, i32)>,
    /// Items go to the page's toolbar (inside `toolbar`).
    in_toolbar: bool,
    /// Items go to the page's tab bar (inside `tab_bar`).
    in_tabs: bool,
    /// The serial a focus or select request made while building gets.
    pub(crate) focus_serial: i32,
    /// Interactive items added now report keyboard focus.
    report_focus: bool,
}

/// Builds pages for an app outside the desktop controller (the phone app),
/// keeping focus and select-all requests from one build to the next.
#[derive(Debug, Default)]
pub struct PageBuilder {
    serial: i32,
    focus_want: Option<(String, i32)>,
    select_want: Option<(String, i32)>,
}

impl PageBuilder {
    /// Build a page with `f`, answering `action` on the way; returns the page and
    /// whether the action was answered.
    pub fn build(&mut self, action: Option<Action>, f: impl FnOnce(&mut Ui)) -> (Page, bool) {
        let acting = action.is_some();
        let mut ui = Ui::new(action);
        ui.want_focus(self.focus_want.clone());
        ui.want_select(self.select_want.clone());
        ui.focus_serial = self.serial + 1;
        f(&mut ui);
        let used = acting && !ui.acting();
        if let Some(id) = ui.focus_requests.last().cloned() {
            self.serial += 1;
            self.focus_want = Some((id, self.serial));
        }
        if let Some(id) = ui.select_requests.last().cloned() {
            self.serial += 1;
            self.select_want = Some((id, self.serial));
        }
        (ui.finish(), used)
    }
}

impl Ui {
    /// A builder; `action` is the user's action to answer, if any.
    pub fn new(action: Option<Action>) -> Self {
        Ui {
            action,
            consumed: false,
            page: Page {
                scroll: true,
                ..Page::default()
            },
            target: Target::Main,
            section: None,
            row: None,
            scope: Vec::new(),
            seen: HashMap::new(),
            next: Next::default(),
            focus_want: None,
            focus_requests: Vec::new(),
            select_requests: Vec::new(),
            select_want: None,
            focus_serial: 1,
            in_toolbar: false,
            in_tabs: false,
            report_focus: false,
        }
    }

    /// Interactive items added from now on report keyboard focus (`Ev::Focus`
    /// and `Ev::Blur`), or stop doing so.
    pub fn report_focus(&mut self, on: bool) {
        self.report_focus = on;
    }

    /// Give focus to the item with this id (serial > 0) when it is built.
    pub fn want_focus(&mut self, id: Option<(String, i32)>) {
        self.focus_want = id;
    }

    /// Select the text of the item with this id when it is built.
    pub fn want_select(&mut self, id: Option<(String, i32)>) {
        self.select_want = id;
    }

    /// Whether this is the action pass.
    pub fn acting(&self) -> bool {
        self.action.is_some() && !self.consumed
    }

    /// The pending action, if not yet answered.
    pub fn action(&self) -> Option<&Action> {
        if self.consumed {
            None
        } else {
            self.action.as_ref()
        }
    }

    /// Mark the pending action answered (for actions handled outside items).
    pub fn consume(&mut self) {
        self.consumed = true;
    }

    /// The finished page.
    pub fn finish(mut self) -> Page {
        self.end_row();
        self.page
    }

    // ----- structure -----------------------------------------------------------

    fn sections(&mut self) -> &mut Vec<Section> {
        match self.target {
            Target::Main => &mut self.page.sections,
            Target::Side => &mut self.page.side,
            Target::Dialog(i) => match self.page.dialogs.get_mut(i) {
                Some(d) => &mut d.sections,
                None => &mut self.page.sections,
            },
            Target::Block(i) => match self.page.board.get_mut(i) {
                Some(b) => &mut b.sections,
                None => &mut self.page.sections,
            },
            Target::Footer => &mut self.page.footer,
        }
    }

    fn toolbar_items(&mut self) -> &mut Vec<Item> {
        match self.target {
            Target::Block(i) if i < self.page.board.len() => match self.page.board.get_mut(i) {
                Some(b) => &mut b.head,
                None => &mut self.page.toolbar,
            },
            _ => &mut self.page.toolbar,
        }
    }

    fn end_row(&mut self) {
        if let Some(row) = self.row.take() {
            self.push_row(row);
        }
    }

    fn push_row(&mut self, row: Row) {
        if self.section.is_none() {
            self.sections().push(Section::default());
            let n = self.sections().len();
            self.section = Some(n.saturating_sub(1));
        }
        let idx = self.section.unwrap_or_default();
        if let Some(s) = self.sections().get_mut(idx) {
            s.rows.push(row);
        }
    }

    /// Set the page title.
    pub fn title(&mut self, text: impl Into<String>) {
        self.page.title = text.into();
    }

    /// Items added in `f` go to the page's toolbar (after the title).
    pub fn toolbar<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        let saved = self.in_toolbar;
        self.in_toolbar = true;
        let r = f(self);
        self.in_toolbar = saved;
        r
    }

    /// Items added in `f` go to the page's bottom tab bar (the phone).
    pub fn tab_bar<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        let saved = self.in_tabs;
        self.in_tabs = true;
        let r = f(self);
        self.in_tabs = saved;
        r
    }

    /// The page does not scroll as a whole (it holds a scrolling table).
    pub fn no_scroll(&mut self) {
        self.page.scroll = false;
    }

    /// Run `f` with ids prefixed by `scope`.
    pub fn scope<R>(&mut self, scope: impl Into<String>, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.scope.push(scope.into());
        let r = f(self);
        self.scope.pop();
        r
    }

    /// A new section of `kind`; rows go into it until the next section.
    pub fn begin_section(&mut self, kind: SectionKind, title: &str) -> usize {
        self.end_row();
        let id = self.make_id(&format!("section:{title}"));
        let flat = matches!(self.target, Target::Block(_));
        self.sections().push(Section {
            kind,
            id,
            title: title.to_owned(),
            open: true,
            flat,
            ..Section::default()
        });
        let n = self.sections().len().saturating_sub(1);
        self.section = Some(n);
        n
    }

    /// The section being filled.
    pub fn section_mut(&mut self) -> Option<&mut Section> {
        let idx = self.section?;
        self.sections().get_mut(idx)
    }

    /// Rows after this go onto the page again (a plain section).
    pub fn end_section(&mut self) {
        self.end_row();
        self.section = None;
    }

    /// A card holding what `f` adds.
    pub fn card<R>(&mut self, title: &str, f: impl FnOnce(&mut Ui) -> R) -> R {
        let saved = self.section;
        self.begin_section(SectionKind::Card, title);
        let r = f(self);
        self.end_row();
        self.section = None;
        let _ = saved;
        r
    }

    /// A plain group (rows straight on the page, separated from what follows).
    pub fn group<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.begin_section(SectionKind::Plain, "");
        let r = f(self);
        self.end_section();
        r
    }

    /// A card that folds away; returns `f`'s result when open. `open` is the
    /// stored state, toggled by clicking the header.
    pub fn collapsing<R>(
        &mut self,
        title: &str,
        open: &mut bool,
        f: impl FnOnce(&mut Ui) -> R,
    ) -> Option<R> {
        self.begin_section(SectionKind::Collapsing, title);
        let id = self.section_mut().map(|s| s.id.clone()).unwrap_or_default();
        if self.hit(&id, Ev::Click) {
            *open = !*open;
        }
        if let Some(s) = self.section_mut() {
            s.open = *open;
        }
        let r = if *open { Some(f(self)) } else { None };
        self.end_section();
        r
    }

    /// A notice band.
    pub fn banner(&mut self, tone: Tone, f: impl FnOnce(&mut Ui)) {
        self.begin_section(SectionKind::Banner, "");
        if let Some(s) = self.section_mut() {
            s.tone = tone;
        }
        f(self);
        self.end_section();
    }

    /// Items side by side.
    pub fn row<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.row_kind(RowKind::Plain, "", f)
    }

    /// Items side by side, wrapping when narrow.
    pub fn wrap<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.row_kind(RowKind::Wrap, "", f)
    }

    /// A form row: `caption`, then the items.
    pub fn form<R>(&mut self, caption: &str, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.row_kind(RowKind::Form, caption, f)
    }

    /// Buttons at the end of a card or dialog.
    pub fn actions<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.row_kind(RowKind::Actions, "", f)
    }

    fn row_kind<R>(&mut self, kind: RowKind, caption: &str, f: impl FnOnce(&mut Ui) -> R) -> R {
        if let Some(row) = &self.row {
            if row.kind == RowKind::Table {
                // Inside a table row: one cell holding several items.
                let start = row.items.len();
                let r = f(self);
                if let Some(row) = self.row.as_mut() {
                    let children: Vec<Item> =
                        row.items.drain(start.min(row.items.len())..).collect();
                    row.items.push(Item {
                        kind: Kind::Group,
                        children,
                        grow: true,
                        ..Item::default()
                    });
                }
                return r;
            }
            // Nested rows flatten into the row being built.
            return f(self);
        }
        self.row = Some(Row {
            kind,
            caption: caption.to_owned(),
            ..Row::default()
        });
        let r = f(self);
        self.end_row();
        r
    }

    /// A clickable list entry (`label` is its accessible name). Returns its
    /// response; `f` adds what the entry shows (texts, amounts).
    pub fn list_item(&mut self, label: &str, selected: bool, f: impl FnOnce(&mut Ui)) -> Resp {
        self.end_row();
        let id = self.take_id(label);
        let resp = self.respond(&id, true);
        self.row = Some(Row {
            kind: RowKind::List,
            id: id.clone(),
            label: label.to_owned(),
            selected,
            ..Row::default()
        });
        f(self);
        self.end_row();
        resp
    }

    /// Context menu entries for the last list or table row.
    pub fn row_menu(&mut self, entries: &[&str]) {
        let idx = self.section;
        if let Some(i) = idx
            && let Some(s) = self.sections().get_mut(i)
            && let Some(r) = s.rows.last_mut()
        {
            r.menu = entries.iter().map(|e| (*e).to_owned()).collect();
        }
    }

    /// Set the indentation of the last row.
    pub fn indent_last(&mut self, steps: i32) {
        if let Some(i) = self.section
            && let Some(s) = self.sections().get_mut(i)
            && let Some(r) = s.rows.last_mut()
        {
            r.indent = steps;
        }
    }

    /// A table: `columns`, then rows added with [`Ui::table_row`].
    pub fn table<R>(
        &mut self,
        title: &str,
        columns: Vec<Column>,
        f: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        self.begin_section(SectionKind::Table, title);
        if let Some(s) = self.section_mut() {
            s.columns = columns;
        }
        let r = f(self);
        self.end_section();
        r
    }

    /// A table whose first column (and, with `total`, last column) stays put while
    /// the period columns between scroll sideways when they do not fit.
    pub fn period_table<R>(
        &mut self,
        title: &str,
        columns: Vec<Column>,
        total: bool,
        f: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        self.begin_section(SectionKind::Table, title);
        if let Some(s) = self.section_mut() {
            // The scrolling part's width rides in `row_height` (periods plus gaps).
            let end = if total {
                columns.len().saturating_sub(1)
            } else {
                columns.len()
            };
            let periods = columns.get(1..end).unwrap_or(&[]);
            let gaps = i32::try_from(periods.len().saturating_sub(1)).unwrap_or(0) * 12;
            s.row_height = periods.iter().map(|c| c.width).sum::<i32>() + gaps;
            s.columns = columns;
            s.frozen = if total { 2 } else { 1 };
        }
        let r = f(self);
        self.end_section();
        r
    }

    /// Make the current table windowed: `total` rows, of which `rows` hold the
    /// ones from `first`; ask to scroll to `scroll` (row, serial).
    pub fn window(&mut self, total: usize, first: usize, scroll: (i32, i32), row_height: i32) {
        if let Some(s) = self.section_mut() {
            s.total_rows = i32::try_from(total).unwrap_or(i32::MAX);
            s.first_row = i32::try_from(first).unwrap_or(0);
            s.scroll_row = scroll.0;
            s.scroll_serial = scroll.1;
            s.row_height = row_height;
        }
    }

    /// Whether the pending action is the windowed table `id` reporting its first
    /// visible row; returns it.
    pub fn scrolled(&mut self, id_label: &str) -> Option<usize> {
        let id = self.full_id(&format!("section:{id_label}"));
        let a = self.action.as_ref()?;
        if self.consumed || a.id != id || a.ev != Ev::Key {
            return None;
        }
        let n = a.value.strip_prefix("scroll:")?.parse().ok()?;
        self.consumed = true;
        Some(n)
    }

    /// A key action ("escape", "ctrl+n") not taken by anything built before:
    /// whether it is `name` (and take it).
    pub fn key(&mut self, name: &str) -> bool {
        match &self.action {
            Some(a) if !self.consumed && a.ev == Ev::Key && a.value == name => {
                self.consumed = true;
                true
            }
            _ => false,
        }
    }

    /// Rows shown before the current table scrolls.
    pub fn visible_rows(&mut self, n: i32) {
        if let Some(s) = self.section_mut() {
            s.visible_rows = n;
        }
    }

    /// A header click on a sortable column of the current table.
    pub fn sorted_column(&mut self, id: &str) -> bool {
        let id = self.full_id(id);
        self.hit(&id, Ev::Click)
    }

    /// One table row; `key` names it (its id when `clickable`). `f` adds one item
    /// per column.
    pub fn table_row(
        &mut self,
        key: &str,
        selected: bool,
        clickable: bool,
        f: impl FnOnce(&mut Ui),
    ) -> Resp {
        self.end_row();
        let id = if clickable {
            self.take_id(key)
        } else {
            String::new()
        };
        let resp = if clickable {
            self.respond(&id, true)
        } else {
            Resp::default()
        };
        self.row = Some(Row {
            kind: RowKind::Table,
            id,
            label: key.to_owned(),
            selected,
            ..Row::default()
        });
        self.scope.push(key.to_owned());
        f(self);
        self.scope.pop();
        self.end_row();
        resp
    }

    /// A tile grid; tiles added in `f` with [`Ui::tile`].
    pub fn tiles<R>(&mut self, title: &str, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.begin_section(SectionKind::Tiles, title);
        self.row = Some(Row::default());
        let r = f(self);
        self.end_section();
        r
    }

    /// Centre the current section's content (tile grids).
    pub fn center_section(&mut self) {
        if let Some(s) = self.section_mut() {
            s.center = true;
        }
    }

    /// The side list (beside the content).
    pub fn side<R>(&mut self, width: i32, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.end_row();
        let (t, s) = (self.target, self.section);
        self.target = Target::Side;
        self.section = None;
        self.page.side_width = width;
        let r = f(self);
        self.end_row();
        self.target = t;
        self.section = s;
        r
    }

    /// A dialog over the page; returns `f`'s result.
    pub fn dialog<R>(
        &mut self,
        key: &str,
        title: &str,
        width: i32,
        f: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        self.end_row();
        let (t, s) = (self.target, self.section);
        let id = self.make_id(&format!("dialog:{key}"));
        self.page.dialogs.push(Dialog {
            id,
            title: title.to_owned(),
            sections: Vec::new(),
            width,
            close_label: "Close dialog".into(),
            ..Dialog::default()
        });
        self.target = Target::Dialog(self.page.dialogs.len().saturating_sub(1));
        self.section = None;
        self.scope.push(format!("dialog:{key}"));
        let r = f(self);
        self.scope.pop();
        self.end_row();
        self.target = t;
        self.section = s;
        r
    }

    /// A bottom sheet over the page (the phone's keypad and calendar): like
    /// [`Ui::dialog`]; its close action (a tap on the dimmed part) is
    /// [`Ui::dialog_closed`] with the same key. `full` makes it a full-screen
    /// viewer that any tap closes.
    pub fn sheet<R>(
        &mut self,
        key: &str,
        title: &str,
        full: bool,
        f: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        self.dialog(key, title, 480, |ui| {
            if let Target::Dialog(i) = ui.target
                && let Some(d) = ui.page.dialogs.get_mut(i)
            {
                d.sheet = if full { 2 } else { 1 };
            }
            f(ui)
        })
    }

    /// Sections added in `f` go to the bar fixed at the bottom of the screen.
    pub fn footer<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.end_row();
        let (t, s) = (self.target, self.section);
        self.target = Target::Footer;
        self.section = None;
        let r = f(self);
        self.end_row();
        self.target = t;
        self.section = s;
        r
    }

    /// A grid of `cols` equal columns, rows `row_height` pixels tall: each row
    /// added in `f` with [`Ui::row`] fills the columns left to right, an item
    /// spanning as many columns as its `maximum` says (set it with
    /// [`Ui::last`]; 0 counts as 1).
    pub fn grid<R>(
        &mut self,
        title: &str,
        cols: usize,
        row_height: i32,
        f: impl FnOnce(&mut Ui) -> R,
    ) -> R {
        self.begin_section(SectionKind::Grid, title);
        if let Some(s) = self.section_mut() {
            s.columns = vec![Column::default(); cols];
            s.row_height = row_height;
        }
        let r = f(self);
        self.end_row();
        if let Some(s) = self.section_mut() {
            for row in &mut s.rows {
                let mut col = 0;
                for it in &mut row.items {
                    it.maximum = it.maximum.max(1);
                    it.minimum = col;
                    col += it.maximum;
                }
            }
        }
        self.end_section();
        r
    }

    /// A dashboard block at `cell` (x, y, w, h in grid cells). Inside, `toolbar`
    /// fills the title bar's right side and everything else the body.
    pub fn block<R>(
        &mut self,
        key: &str,
        title: &str,
        cell: (i32, i32, i32, i32),
        f: impl FnOnce(&mut Ui) -> R,
    ) -> (Resp, R) {
        self.end_row();
        let (t, s) = (self.target, self.section);
        let id = self.make_id(&format!("block:{key}"));
        let resp = self.respond(&id, true);
        self.page.board.push(Block {
            id,
            title: title.to_owned(),
            x: cell.0,
            y: cell.1,
            w: cell.2,
            h: cell.3,
            ..Block::default()
        });
        self.target = Target::Block(self.page.board.len().saturating_sub(1));
        self.section = None;
        self.scope.push(format!("block:{key}"));
        let r = f(self);
        self.scope.pop();
        self.end_row();
        self.target = t;
        self.section = s;
        (resp, r)
    }

    /// The last block: its title opens something; where it would land.
    pub fn block_extra(&mut self, title_link: bool, ghost: Option<(i32, i32, i32, i32)>) {
        if let Some(b) = self.page.board.last_mut() {
            b.title_link = title_link;
            b.ghost = ghost;
        }
    }

    /// The board is in edit mode.
    pub fn board_edit(&mut self, on: bool) {
        self.page.board_edit = on;
    }

    /// Name the current dialog's × button (default "Close dialog").
    pub fn close_label(&mut self, name: &str) {
        if let Target::Dialog(i) = self.target
            && let Some(d) = self.page.dialogs.get_mut(i)
        {
            d.close_label = name.to_owned();
        }
    }

    /// Make the current dialog compact, with `icon` before its title.
    pub fn compact_dialog(&mut self, icon: &str) {
        if let Target::Dialog(i) = self.target
            && let Some(d) = self.page.dialogs.get_mut(i)
        {
            d.icon = icon.to_owned();
            d.compact = true;
        }
    }

    /// Whether the action is Escape / closing the dialog `key` (its close button
    /// or the Escape key).
    pub fn dialog_closed(&mut self, key: &str) -> bool {
        let id = self.make_id_peek(&format!("dialog:{key}"));
        self.hit(&id, Ev::Click)
    }

    // ----- ids and responses ------------------------------------------------------

    fn full_id(&self, label: &str) -> String {
        if self.scope.is_empty() {
            label.to_owned()
        } else {
            format!("{}/{label}", self.scope.join("/"))
        }
    }

    fn make_id(&mut self, label: &str) -> String {
        let base = self.full_id(label);
        let n = self.seen.entry(base.clone()).or_insert(0);
        *n += 1;
        if *n == 1 { base } else { format!("{base}#{n}") }
    }

    fn make_id_peek(&self, label: &str) -> String {
        self.full_id(label)
    }

    /// The id for the next item: the one set with `next().id()`, else from `label`.
    fn take_id(&mut self, label: &str) -> String {
        match self.next.id.take() {
            Some(id) => self.make_id(&id),
            None => self.make_id(label),
        }
    }

    /// Whether the pending action is `ev` on `id` (and answer it).
    pub fn hit(&mut self, id: &str, ev: Ev) -> bool {
        if self.consumed {
            return false;
        }
        match &self.action {
            Some(a) if a.id == id && a.ev == ev => {
                self.consumed = true;
                true
            }
            _ => false,
        }
    }

    /// The response for item `id` to the pending action.
    fn respond(&mut self, id: &str, enabled: bool) -> Resp {
        let mut r = Resp {
            id: id.to_owned(),
            ..Resp::default()
        };
        if self.consumed || !enabled {
            return r;
        }
        let Some(a) = &self.action else {
            return r;
        };
        if a.id != id {
            return r;
        }
        match a.ev {
            Ev::Click => {
                r.clicked = true;
                r.value = a.value.clone();
            }
            Ev::Edit => {
                r.changed = true;
                r.value = a.value.clone();
            }
            Ev::Enter => r.submitted = true,
            Ev::Focus => r.focused = true,
            Ev::Blur => r.blurred = true,
            Ev::Select => {
                if let Some(t) = a.value.strip_prefix('=') {
                    r.chosen_text = Some(t.to_owned());
                    r.changed = true;
                } else {
                    r.chosen = a.value.parse().ok();
                    r.changed = r.chosen.is_some();
                }
            }
            Ev::Secondary => r.secondary = Some(a.value.parse().ok()),
            Ev::Key => {
                // Keys are taken only by `Ui::key` or explicitly (`consume`), so an
                // unhandled key goes on to the window's shortcuts.
                r.key = Some(a.value.clone());
                return r;
            }
        }
        self.consumed = true;
        r
    }

    fn action_value(&self) -> String {
        self.action
            .as_ref()
            .map(|a| a.value.clone())
            .unwrap_or_default()
    }

    // ----- next-item attributes ----------------------------------------------------

    /// Attributes for the next item.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> &mut Self {
        self
    }
    /// The next item is enabled only when `on`.
    pub fn enabled(&mut self, on: bool) -> &mut Self {
        self.next.enabled = Some(on);
        self
    }
    /// Tooltip / accessible description of the next item.
    /// The next text is part of its row's accessible name (a list entry's own
    /// label): drawn, not read a second time.
    pub fn quiet(&mut self) -> &mut Self {
        self.next.quiet = true;
        self
    }
    pub fn tip(&mut self, text: impl Into<String>) -> &mut Self {
        let t = text.into();
        if !t.is_empty() {
            self.next.desc = Some(t);
        }
        self
    }
    /// Accessible name of the next item (when it differs from its text).
    pub fn named(&mut self, name: impl Into<String>) -> &mut Self {
        self.next.label = Some(name.into());
        self
    }
    /// Id of the next item (instead of one made from its label).
    pub fn id(&mut self, id: impl Into<String>) -> &mut Self {
        self.next.id = Some(id.into());
        self
    }
    pub fn width(&mut self, px: i32) -> &mut Self {
        self.next.width = Some(px);
        self
    }
    pub fn grow(&mut self) -> &mut Self {
        self.next.grow = true;
        self
    }
    pub fn tone(&mut self, tone: Tone) -> &mut Self {
        self.next.tone = Some(tone);
        self
    }
    pub fn align(&mut self, align: Align) -> &mut Self {
        self.next.align = Some(align);
        self
    }
    pub fn mono(&mut self) -> &mut Self {
        self.next.mono = true;
        self
    }
    pub fn bold(&mut self) -> &mut Self {
        self.next.bold = true;
        self
    }
    pub fn strike(&mut self, on: bool) -> &mut Self {
        self.next.strike = on;
        self
    }
    /// Focus the next item now.
    pub fn focus(&mut self) -> &mut Self {
        self.next.focus = true;
        self
    }
    /// Select the next field's text now.
    pub fn select_all(&mut self) -> &mut Self {
        self.next.select = true;
        self
    }
    pub fn hint(&mut self, text: impl Into<String>) -> &mut Self {
        self.next.placeholder = Some(text.into());
        self
    }
    /// A refusal shown under the next field.
    pub fn error(&mut self, text: Option<String>) -> &mut Self {
        self.next.error = text.filter(|t| !t.is_empty());
        self
    }
    pub fn icon(&mut self, name: &str) -> &mut Self {
        self.next.icon = Some(name.to_owned());
        self
    }

    /// Add an item, applying the next-item attributes; returns its response.
    pub fn push(&mut self, mut item: Item, interactive: bool) -> Resp {
        let next = std::mem::take(&mut self.next);
        if let Some(e) = next.enabled {
            item.enabled = item.enabled && e;
        }
        if let Some(d) = next.desc {
            item.desc = d;
        }
        if let Some(l) = next.label {
            item.label = l;
        }
        if let Some(w) = next.width {
            item.width = w;
        }
        item.grow |= next.grow;
        if let Some(t) = next.tone {
            item.tone = t;
        }
        if let Some(a) = next.align {
            item.align = a;
        }
        item.mono |= next.mono;
        item.bold |= next.bold;
        item.strike |= next.strike;
        if let Some(p) = next.placeholder {
            item.placeholder = p;
        }
        if let Some(e) = next.error {
            item.error = e;
        }
        if let Some(i) = next.icon {
            item.icon = i;
        }
        if next.quiet && !interactive {
            QUIET.clone_into(&mut item.id);
        }
        item.report_focus |= interactive && self.report_focus;
        let resp = if interactive {
            let base = if let Some(id) = next.id {
                id
            } else {
                item.name().to_owned()
            };
            item.id = self.make_id(&base);
            let id = item.id.clone();
            self.respond(&id, item.enabled)
        } else {
            Resp::default()
        };
        if next.focus {
            self.focus_requests.push(item.id.clone());
            item.focus = self.focus_serial;
        }
        if next.select {
            self.select_requests.push(item.id.clone());
            item.select = self.focus_serial;
        }
        if let Some((fid, serial)) = &self.focus_want
            && !item.id.is_empty()
            && *fid == item.id
        {
            item.focus = *serial;
        }
        if let Some((sid, serial)) = &self.select_want
            && !item.id.is_empty()
            && *sid == item.id
        {
            item.select = *serial;
        }
        if self.in_tabs {
            self.page.tabs.push(item);
            return resp;
        }
        if self.in_toolbar {
            self.toolbar_items().push(item);
            return resp;
        }
        match &mut self.row {
            Some(row) => row.items.push(item),
            None => self.push_row(Row {
                items: vec![item],
                ..Row::default()
            }),
        }
        resp
    }

    // ----- items --------------------------------------------------------------------

    fn text_item(&mut self, kind: Kind, text: impl Into<String>) {
        self.push(
            Item {
                kind,
                text: text.into(),
                ..Item::default()
            },
            false,
        );
    }

    /// Body text.
    pub fn label(&mut self, text: impl Into<String>) {
        self.text_item(Kind::Text, text);
    }
    /// A secondary (dimmer) text.
    pub fn secondary(&mut self, text: impl Into<String>) {
        self.tone(Tone::Secondary);
        self.text_item(Kind::Text, text);
    }
    /// An error text.
    pub fn error_text(&mut self, text: impl Into<String>) {
        self.tone(Tone::Error);
        self.text_item(Kind::Text, text);
    }
    /// A heading (subtitle size; the page title is [`Ui::title`]).
    pub fn heading(&mut self, text: impl Into<String>) {
        self.text_item(Kind::Subtitle, text);
    }
    /// A large heading.
    pub fn big_heading(&mut self, text: impl Into<String>) {
        self.text_item(Kind::Title, text);
    }
    /// Small print.
    pub fn caption(&mut self, text: impl Into<String>) {
        self.text_item(Kind::Caption, text);
    }
    /// An amount in the number font, right-aligned.
    pub fn amount(&mut self, text: impl Into<String>) {
        self.next.align.get_or_insert(Align::Right);
        self.text_item(Kind::Amount, text);
    }
    pub fn separator(&mut self) {
        self.text_item(Kind::Separator, "");
    }
    /// Flexible space (pushes later items to the end of a row).
    pub fn spacer(&mut self) {
        self.push(
            Item {
                kind: Kind::Spacer,
                grow: true,
                ..Item::default()
            },
            false,
        );
    }
    pub fn badge(&mut self, text: impl Into<String>, tone: Tone) {
        self.tone(tone);
        self.text_item(Kind::Badge, text);
    }

    /// A small label on a colour of its own (`color` 0xRRGGBB) with dark or light
    /// text (`dark`), chosen by the caller for enough contrast (subscriptions.md).
    pub fn color_badge(&mut self, text: impl Into<String>, color: u32, dark: bool) {
        self.text_item(Kind::Badge, text);
        if let Some(it) = self.last() {
            it.color = Some(color);
            it.value = if dark { "chip-dark" } else { "chip-light" }.into();
        }
    }

    /// A button.
    pub fn button(&mut self, text: impl Into<String>) -> Resp {
        self.push(
            Item {
                kind: Kind::Button,
                text: text.into(),
                ..Item::default()
            },
            true,
        )
    }
    /// The main (accent) button of a form.
    pub fn primary(&mut self, text: impl Into<String>) -> Resp {
        self.push(
            Item {
                kind: Kind::Primary,
                text: text.into(),
                ..Item::default()
            },
            true,
        )
    }
    /// The main (accent) button drawn as an icon only; `name` is its accessible
    /// name and tooltip. While `busy` it shows a spinner instead of the icon.
    pub fn primary_icon(&mut self, icon: &str, name: &str, busy: bool) -> Resp {
        self.push(
            Item {
                kind: Kind::Primary,
                icon: icon.to_owned(),
                label: name.to_owned(),
                desc: name.to_owned(),
                value: if busy {
                    "busy".to_owned()
                } else {
                    String::new()
                },
                ..Item::default()
            },
            true,
        )
    }
    pub fn link(&mut self, text: impl Into<String>) -> Resp {
        self.push(
            Item {
                kind: Kind::Link,
                text: text.into(),
                ..Item::default()
            },
            true,
        )
    }
    /// An icon button; `name` is its accessible name, `icon` the drawing.
    pub fn icon_button(&mut self, icon: &str, name: &str, checked: bool) -> Resp {
        self.push(
            Item {
                kind: Kind::IconButton,
                icon: icon.to_owned(),
                label: name.to_owned(),
                checked,
                ..Item::default()
            },
            true,
        )
    }
    /// An icon button that turns something on and off (exposed as checkable).
    pub fn icon_toggle(&mut self, icon: &str, name: &str, on: bool) -> Resp {
        self.push(
            Item {
                kind: Kind::IconButton,
                icon: icon.to_owned(),
                label: name.to_owned(),
                value: "toggle".to_owned(),
                checked: on,
                ..Item::default()
            },
            true,
        )
    }
    /// An icon button that selects a view, like a navigation tab (the Settings
    /// gear): exposed as a tab with its selected state.
    pub fn icon_tab(&mut self, icon: &str, name: &str, selected: bool) -> Resp {
        self.push(
            Item {
                kind: Kind::IconButton,
                icon: icon.to_owned(),
                label: name.to_owned(),
                value: "tab".to_owned(),
                checked: selected,
                ..Item::default()
            },
            true,
        )
    }
    /// A plain icon.
    pub fn icon_image(&mut self, icon: &str) {
        self.push(
            Item {
                kind: Kind::Icon,
                icon: icon.to_owned(),
                ..Item::default()
            },
            false,
        );
    }
    /// A selectable tab.
    pub fn tab(&mut self, text: impl Into<String>, selected: bool) -> Resp {
        self.push(
            Item {
                kind: Kind::Tab,
                text: text.into(),
                checked: selected,
                ..Item::default()
            },
            true,
        )
    }
    /// A toggle button (selected or not).
    pub fn toggle(&mut self, text: impl Into<String>, selected: bool) -> Resp {
        self.push(
            Item {
                kind: Kind::Toggle,
                text: text.into(),
                checked: selected,
                ..Item::default()
            },
            true,
        )
    }

    /// A single-line text field named `label`, editing `text`.
    pub fn field(&mut self, label: &str, text: &mut String) -> Resp {
        self.field_kind(Kind::Field, label, text)
    }
    pub fn password(&mut self, label: &str, text: &mut String) -> Resp {
        self.field_kind(Kind::Password, label, text)
    }
    pub fn multiline(&mut self, label: &str, text: &mut String) -> Resp {
        self.field_kind(Kind::Multiline, label, text)
    }

    fn field_kind(&mut self, kind: Kind, label: &str, text: &mut String) -> Resp {
        let item = Item {
            kind,
            label: label.to_owned(),
            value: text.clone(),
            ..Item::default()
        };
        let resp = self.push(item, true);
        if resp.changed {
            *text = self.action_value();
            self.set_last_value(text);
        }
        resp
    }

    /// A field with suggestions shown below it while focused.
    pub fn suggest(&mut self, label: &str, text: &mut String, options: Vec<String>) -> Resp {
        let item = Item {
            kind: Kind::Suggest,
            label: label.to_owned(),
            value: text.clone(),
            options,
            ..Item::default()
        };
        let resp = self.push(item, true);
        if resp.changed && resp.chosen.is_none() {
            *text = self.action_value();
            self.set_last_value(text);
        }
        resp
    }

    /// The pending action on an absolute `id` (an element of the shell that is not
    /// an item, such as the pane's resize grip).
    pub fn raw(&mut self, id: &str) -> Resp {
        self.respond(id, true)
    }

    /// Set the last item's text (a date field's format guide).
    /// A drop-down's entries from index `from` on are named "<prefix> <entry>".
    pub fn entry_prefix(&mut self, prefix: &str, from: i32) {
        if let Some(it) = self.last_item() {
            prefix.clone_into(&mut it.text);
            it.minimum = from;
        }
    }

    pub fn set_last_text(&mut self, text: &str) {
        if let Some(it) = self.last_item() {
            text.clone_into(&mut it.text);
        }
    }

    fn last_item(&mut self) -> Option<&mut Item> {
        if self.in_tabs {
            return self.page.tabs.last_mut();
        }
        if self.in_toolbar {
            return self.toolbar_items().last_mut();
        }
        if self.row.is_some() {
            return self.row.as_mut().and_then(|r| r.items.last_mut());
        }
        let idx = self.section?;
        self.sections()
            .get_mut(idx)
            .and_then(|s| s.rows.last_mut())
            .and_then(|r| r.items.last_mut())
    }

    pub fn set_last_value(&mut self, v: &str) {
        if let Some(it) = self.last_item() {
            it.value = v.to_owned();
        }
    }

    /// The last item added (to adjust what the helpers set).
    pub fn last(&mut self) -> Option<&mut Item> {
        self.last_item()
    }

    /// A check box.
    pub fn checkbox(&mut self, on: &mut bool, text: &str) -> Resp {
        let resp = self.push(
            Item {
                kind: Kind::Check,
                text: text.to_owned(),
                checked: *on,
                ..Item::default()
            },
            true,
        );
        let mut resp = resp;
        if resp.clicked {
            *on = !*on;
            resp.changed = true;
            if let Some(it) = self.last_item() {
                it.checked = *on;
            }
        }
        resp
    }
    /// A switch.
    pub fn switch(&mut self, on: &mut bool, text: &str) -> Resp {
        let resp = self.push(
            Item {
                kind: Kind::Switch,
                text: text.to_owned(),
                checked: *on,
                ..Item::default()
            },
            true,
        );
        let mut resp = resp;
        if resp.clicked {
            *on = !*on;
            resp.changed = true;
            if let Some(it) = self.last_item() {
                it.checked = *on;
            }
        }
        resp
    }
    /// A radio button; clicking sets `current` to `value`.
    pub fn radio<T: PartialEq + Clone>(&mut self, current: &mut T, value: T, text: &str) -> Resp {
        let resp = self.push(
            Item {
                kind: Kind::Radio,
                text: text.to_owned(),
                checked: *current == value,
                ..Item::default()
            },
            true,
        );
        if resp.clicked {
            *current = value;
            if let Some(it) = self.last_item() {
                it.checked = true;
            }
        }
        resp
    }
    /// A drop-down list named `label`; `index` is the chosen option.
    pub fn combo(&mut self, label: &str, index: &mut usize, options: &[String]) -> Resp {
        let resp = self.push(
            Item {
                kind: Kind::Combo,
                label: label.to_owned(),
                options: options.to_vec(),
                index: i32::try_from(*index).unwrap_or(-1),
                value: options.get(*index).cloned().unwrap_or_default(),
                ..Item::default()
            },
            true,
        );
        let mut resp = resp;
        let picked = resp.chosen.or_else(|| {
            resp.chosen_text
                .as_ref()
                .and_then(|t| options.iter().position(|o| o == t))
        });
        resp.chosen = picked;
        resp.changed = picked.is_some();
        if let Some(i) = picked
            && i < options.len()
        {
            *index = i;
            let v = options.get(i).cloned().unwrap_or_default();
            if let Some(it) = self.last_item() {
                it.index = i32::try_from(i).unwrap_or(-1);
                it.value = v;
            }
        }
        resp
    }
    /// A drop-down list with a search box at the top of its list (`search_label`
    /// names the box); `search` holds what was typed, and `options` should be
    /// filtered by it.
    pub fn combo_search(
        &mut self,
        label: &str,
        index: &mut usize,
        options: &[String],
        search: &mut String,
        search_label: &str,
    ) -> Resp {
        self.next.placeholder = Some(search_label.to_owned());
        let resp = self.combo(label, index, options);
        if let Some(k) = &resp.key
            && let Some(t) = k.strip_prefix("search:")
        {
            *search = t.to_owned();
            self.consumed = true;
        }
        resp
    }

    /// A drop-down over string values: `value` is the chosen one.
    pub fn combo_str(&mut self, label: &str, value: &mut String, options: &[String]) -> Resp {
        let mut idx = options
            .iter()
            .position(|o| o == value)
            .unwrap_or(usize::MAX);
        let resp = self.combo(label, &mut idx, options);
        if resp.changed
            && let Some(v) = options.get(idx)
        {
            *value = v.clone();
        }
        resp
    }
    /// A button opening a menu; returns the chosen entry.
    pub fn menu(&mut self, text: &str, entries: &[String]) -> Resp {
        self.push(
            Item {
                kind: Kind::Menu,
                text: text.to_owned(),
                options: entries.to_vec(),
                ..Item::default()
            },
            true,
        )
    }
    /// A menu entry with a check mark (the Features menu).
    pub fn menu_check(&mut self, text: &str, checked: bool) -> Resp {
        self.push(
            Item {
                kind: Kind::MenuCheck,
                text: text.to_owned(),
                checked,
                ..Item::default()
            },
            true,
        )
    }
    /// A progress bar (`percent` 0–100, or -1 for "busy").
    pub fn progress(&mut self, label: &str, percent: i32) {
        self.push(
            Item {
                kind: Kind::Progress,
                label: label.to_owned(),
                number: percent,
                ..Item::default()
            },
            false,
        );
    }
    /// A status light (`state`: green, yellow, red or grey), `diameter` pixels,
    /// named `name`; its tip from [`Ui::tip`].
    pub fn light(&mut self, state: &str, diameter: i32, name: &str) -> Resp {
        self.push(
            Item {
                kind: Kind::Light,
                value: state.to_owned(),
                label: name.to_owned(),
                width: diameter,
                ..Item::default()
            },
            true,
        )
    }
    /// A picture.
    pub fn picture(&mut self, name: &str, pic: Rc<Picture>, width: i32) -> Resp {
        self.push(
            Item {
                kind: Kind::Image,
                label: name.to_owned(),
                picture: Some(pic),
                width,
                ..Item::default()
            },
            true,
        )
    }
    /// A chart.
    /// A chart; with pieces (or `open_more`) it can be clicked and reached with
    /// the keyboard, and the answer says what was hit.
    pub fn chart(&mut self, name: &str, chart: Chart) -> Option<ChartHit> {
        let live = !chart.pieces.is_empty() || chart.open_more;
        if live {
            self.next.id = Some("chart".into());
        }
        let resp = self.push(
            Item {
                kind: Kind::Chart,
                label: name.to_owned(),
                chart: Some(Rc::new(chart)),
                grow: true,
                ..Item::default()
            },
            live,
        );
        if resp.clicked {
            ChartHit::from_value(&resp.value)
        } else {
            None
        }
    }
    /// A slider over whole numbers.
    pub fn slider(&mut self, label: &str, value: &mut i32, min: i32, max: i32) -> Resp {
        let resp = self.push(
            Item {
                kind: Kind::Slider,
                label: label.to_owned(),
                number: *value,
                minimum: min,
                maximum: max,
                value: value.to_string(),
                ..Item::default()
            },
            true,
        );
        if resp.changed
            && let Ok(v) = self.action_value().parse::<i32>()
        {
            *value = v.clamp(min, max);
            let n = *value;
            if let Some(it) = self.last_item() {
                it.number = n;
                it.value = n.to_string();
            }
        }
        resp
    }
    /// Slint's AboutSlint widget.
    pub fn about_slint(&mut self) {
        self.push(
            Item {
                kind: Kind::AboutSlint,
                label: "About Slint".into(),
                ..Item::default()
            },
            false,
        );
    }
    /// Any item, as built by the caller.
    pub fn item(&mut self, item: Item, interactive: bool) -> Resp {
        self.push(item, interactive)
    }
}

/// A sortable column.
pub fn col(title: &str) -> Column {
    Column {
        title: title.to_owned(),
        ..Column::default()
    }
}

/// A right-aligned (amount) column.
pub fn col_num(title: &str) -> Column {
    Column {
        title: title.to_owned(),
        align: Align::Right,
        ..Column::default()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn form(ui: &mut Ui, name: &mut String, saved: &mut bool) {
        ui.title("Form");
        ui.card("Details", |ui| {
            ui.form("Name", |ui| {
                ui.field("Name", name);
            });
            ui.actions(|ui| {
                if ui.primary("Save").clicked() {
                    *saved = true;
                }
            });
        });
    }

    #[test]
    fn builds_and_answers_actions() {
        let mut name = String::from("a");
        let mut saved = false;
        let mut ui = Ui::new(None);
        form(&mut ui, &mut name, &mut saved);
        let page = ui.finish();
        assert_eq!(page.title, "Form");
        let card = &page.sections[0];
        assert_eq!(card.kind, SectionKind::Card);
        assert_eq!(card.rows[0].kind, RowKind::Form);
        assert_eq!(card.rows[0].items[0].value, "a");
        let field_id = card.rows[0].items[0].id.clone();
        let save_id = card.rows[1].items[0].id.clone();
        assert_ne!(field_id, save_id);

        let mut ui = Ui::new(Some(Action::new(&field_id, Ev::Edit, "abc")));
        form(&mut ui, &mut name, &mut saved);
        assert_eq!(name, "abc");
        assert!(!saved);

        let mut ui = Ui::new(Some(Action::new(&save_id, Ev::Click, "")));
        form(&mut ui, &mut name, &mut saved);
        assert!(saved);
    }

    #[test]
    fn repeated_labels_get_distinct_ids() {
        let mut ui = Ui::new(None);
        ui.button("Edit");
        ui.button("Edit");
        let page = ui.finish();
        let a = &page.sections[0].rows[0].items[0].id;
        let b = &page.sections[0].rows[1].items[0].id;
        assert_ne!(a, b);
    }

    #[test]
    fn disabled_items_ignore_actions() {
        let mut ui = Ui::new(Some(Action::new("Go", Ev::Click, "")));
        ui.enabled(false);
        assert!(!ui.button("Go").clicked());
    }

    /// The phone's grid, bottom bar and sheets (mobile-capture.md revision 7).
    #[test]
    fn grid_footer_and_sheet() {
        let mut ui = Ui::new(None);
        ui.sheet("keypad", "", false, |ui| {
            ui.grid("Keys", 4, 56, |ui| {
                ui.row(|ui| {
                    ui.button("Clear");
                    ui.button("=");
                    ui.button("Done");
                    if let Some(it) = ui.last() {
                        it.maximum = 2;
                    }
                });
            });
        });
        ui.footer(|ui| {
            ui.primary("Send");
        });
        let page = ui.finish();
        assert_eq!(page.dialogs[0].sheet, 1);
        let grid = &page.dialogs[0].sections[0];
        assert_eq!(grid.kind, SectionKind::Grid);
        assert_eq!((grid.columns.len(), grid.row_height), (4, 56));
        let cols: Vec<(i32, i32)> = grid.rows[0]
            .items
            .iter()
            .map(|i| (i.minimum, i.maximum))
            .collect();
        assert_eq!(cols, vec![(0, 1), (1, 1), (2, 2)]);
        assert_eq!(page.footer[0].rows[0].items[0].text, "Send");
        assert!(page.sections.is_empty(), "nothing on the page itself");
        let mut ui = Ui::new(Some(Action::new("dialog:keypad", Ev::Click, "")));
        assert!(ui.dialog_closed("keypad"));
    }

    #[test]
    fn combo_choice() {
        let opts = vec!["A".to_owned(), "B".to_owned()];
        let mut i = 0;
        let mut ui = Ui::new(Some(Action::new("Kind", Ev::Select, "1")));
        assert!(ui.combo("Kind", &mut i, &opts).changed());
        assert_eq!(i, 1);
    }
}
