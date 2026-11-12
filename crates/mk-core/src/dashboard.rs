//! Dashboard layout rules (dashboards.md): a 12-column snap-to-grid where blocks
//! occupy whole cells, never overlap and are compacted upwards. Pure logic, shared by
//! the commands (validation) and the desktop app (moving, resizing, compacting).

use serde::{Deserialize, Serialize};

/// The grid is this many columns wide.
pub const COLUMNS: u32 = 12;
/// A dashboard holds at most this many blocks.
pub const MAX_BLOCKS: usize = 40;
/// Block widths, in columns.
pub const MIN_W: u32 = 2;
pub const MAX_W: u32 = COLUMNS;
/// Block heights, in rows.
pub const MIN_H: u32 = 1;
pub const MAX_H: u32 = 12;
/// No block starts below this row (40 blocks of the largest height fit above it).
pub const MAX_Y: u32 = 480;

/// A block's rectangle in whole cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Cell {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Cell {
    pub const fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    fn right(self) -> u32 {
        self.x.saturating_add(self.w)
    }

    fn bottom(self) -> u32 {
        self.y.saturating_add(self.h)
    }

    /// Whether two rectangles share at least one cell.
    pub fn overlaps(self, o: Cell) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    /// Whether the columns of two rectangles overlap.
    fn shares_columns(self, o: Cell) -> bool {
        self.x < o.right() && o.x < self.right()
    }
}

/// A kind of block: its name, title, one-line description, size limits and the
/// module it needs (if any).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockType {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub min_w: u32,
    pub min_h: u32,
    pub default_w: u32,
    pub default_h: u32,
}

/// The block types this version knows, in the order the Add block picker lists them.
pub const BLOCK_TYPES: [BlockType; 12] = [
    BlockType {
        name: "account_transactions",
        title: "Account transactions",
        description: "The newest transactions of one account, or those above an amount",
        min_w: 3,
        min_h: 2,
        default_w: 4,
        default_h: 3,
    },
    BlockType {
        name: "scheduled",
        title: "Scheduled transactions",
        description: "What is due next, with a button to enter it",
        min_w: 3,
        min_h: 2,
        default_w: 4,
        default_h: 3,
    },
    BlockType {
        name: "balances",
        title: "Account balances",
        description: "Balances of chosen accounts or of account groups, with group totals",
        min_w: 2,
        min_h: 2,
        default_w: 4,
        default_h: 4,
    },
    BlockType {
        name: "net_worth",
        title: "Net worth",
        description: "Net worth, its change this month and the last 12 months",
        min_w: 3,
        min_h: 1,
        default_w: 4,
        default_h: 2,
    },
    BlockType {
        name: "chart",
        title: "Chart",
        description: "A report chart: spending, income and expenses, net worth and more",
        min_w: 4,
        min_h: 3,
        default_w: 6,
        default_h: 4,
    },
    BlockType {
        name: "report_summary",
        title: "Report summary",
        description: "One report figure compared with the previous period",
        min_w: 2,
        min_h: 1,
        default_w: 3,
        default_h: 2,
    },
    BlockType {
        name: "shortcuts",
        title: "Shortcut buttons",
        description: "Buttons that open registers, reports, screens or dashboards",
        min_w: 2,
        min_h: 1,
        default_w: 4,
        default_h: 2,
    },
    BlockType {
        name: "attention",
        title: "Attention",
        description: "Things that need you: imports to review, overdue items and more",
        min_w: 3,
        min_h: 2,
        default_w: 4,
        default_h: 3,
    },
    BlockType {
        name: "bills_due",
        title: "Bills and invoices due",
        description: "Bills to pay and invoices to collect in the next days",
        min_w: 3,
        min_h: 2,
        default_w: 4,
        default_h: 3,
    },
    BlockType {
        name: "subscriptions",
        title: "Subscriptions",
        description: "The monthly total of subscriptions, the next three charges and open alerts",
        min_w: 3,
        min_h: 2,
        default_w: 4,
        default_h: 3,
    },
    BlockType {
        name: "projects",
        title: "Projects",
        description: "Active projects with a budget bar each",
        min_w: 3,
        min_h: 2,
        default_w: 4,
        default_h: 3,
    },
    BlockType {
        name: "note",
        title: "Note",
        description: "A short text of your own",
        min_w: 2,
        min_h: 1,
        default_w: 3,
        default_h: 2,
    },
];

/// The block type with this name (`None` for types from a newer version).
pub fn block_type(name: &str) -> Option<&'static BlockType> {
    BLOCK_TYPES.iter().find(|t| t.name == name)
}

/// The smallest size of a block of this type (unknown types: the grid minimum).
pub fn min_size(type_name: &str) -> (u32, u32) {
    block_type(type_name).map_or((MIN_W, MIN_H), |t| (t.min_w, t.min_h))
}

/// Why a layout is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    TooMany(usize),
    /// Block index: outside the 12-column grid (or too far down).
    OutOfGrid(usize),
    /// Block index: smaller than its type's minimum.
    TooSmall(usize),
    /// Block index: larger than 12 × 12.
    TooLarge(usize),
    /// Two block indexes that share a cell.
    Overlap(usize, usize),
}

/// Check a layout: each entry is a block's rectangle and its minimum size.
pub fn validate(blocks: &[(Cell, (u32, u32))]) -> Result<(), LayoutError> {
    if blocks.len() > MAX_BLOCKS {
        return Err(LayoutError::TooMany(blocks.len()));
    }
    for (i, (c, (min_w, min_h))) in blocks.iter().enumerate() {
        if c.w > MAX_W || c.h > MAX_H {
            return Err(LayoutError::TooLarge(i));
        }
        if c.w < (*min_w).max(MIN_W) || c.h < (*min_h).max(MIN_H) {
            return Err(LayoutError::TooSmall(i));
        }
        if c.right() > COLUMNS || c.y > MAX_Y {
            return Err(LayoutError::OutOfGrid(i));
        }
    }
    for (i, (a, _)) in blocks.iter().enumerate() {
        for (j, (b, _)) in blocks.iter().enumerate().skip(i + 1) {
            if a.overlaps(*b) {
                return Err(LayoutError::Overlap(i, j));
            }
        }
    }
    Ok(())
}

/// The order blocks are read in (and shown in a single narrow column): by row, then
/// column.
pub fn reading_order(cells: &[Cell]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..cells.len()).collect();
    idx.sort_by_key(|i| cells.get(*i).map(|c| (c.y, c.x)));
    idx
}

/// Where the moved block goes when it ties with another block for a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tie {
    /// The moved block keeps the place; the other moves down.
    First,
    /// The other block keeps the place; the moved block goes below it.
    After,
}

/// Lay blocks out again with `moved` (if any) placed at its requested rectangle:
/// blocks are taken in order of their requested row (then the tie rule, then column)
/// and each drops to the highest row where it overlaps nothing placed before it. So
/// nothing overlaps and no empty rows stay above a block.
fn settle(cells: &[Cell], moved: Option<(usize, Tie)>) -> Vec<Cell> {
    let mut order: Vec<usize> = (0..cells.len()).collect();
    order.sort_by_key(|i| {
        let c = cells.get(*i).copied().unwrap_or(Cell::new(0, 0, 1, 1));
        let rank = match moved {
            Some((m, Tie::First)) if m == *i => 0u8,
            Some((m, Tie::After)) if m == *i => 2,
            _ => 1,
        };
        (c.y, rank, c.x)
    });
    let mut out: Vec<Cell> = cells.to_vec();
    let mut placed: Vec<Cell> = Vec::with_capacity(cells.len());
    for i in order {
        let Some(mut c) = cells.get(i).copied() else {
            continue;
        };
        c.x = c.x.min(COLUMNS.saturating_sub(c.w));
        c.y = 0;
        while let Some(below) = placed
            .iter()
            .filter(|p| p.overlaps(c))
            .map(|p| p.bottom())
            .max()
        {
            c.y = below;
        }
        placed.push(c);
        if let Some(slot) = out.get_mut(i) {
            *slot = c;
        }
    }
    out
}

/// Compact a layout upwards without moving anything sideways.
pub fn compact(cells: &[Cell]) -> Vec<Cell> {
    settle(cells, None)
}

/// Drop block `i` at `target` (snapped by the caller): blocks in the way move down,
/// then everything is compacted upwards. The size is kept within the grid.
pub fn drop_at(cells: &[Cell], i: usize, target: Cell) -> Vec<Cell> {
    let mut c = cells.to_vec();
    if let Some(slot) = c.get_mut(i) {
        let w = target.w.clamp(MIN_W, MAX_W);
        let h = target.h.clamp(MIN_H, MAX_H);
        *slot = Cell::new(target.x.min(COLUMNS - w), target.y.min(MAX_Y), w, h);
    }
    settle(&c, Some((i, Tie::First)))
}

/// A keyboard or menu move of block `i` by one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
    Left,
    Right,
    Wider,
    Narrower,
    Taller,
    Shorter,
}

/// Move or resize block `i` by one step (Move up/down/left/right, Wider/Narrower,
/// Taller/Shorter), respecting its minimum size, then compact. Up and down move it
/// past the nearest block above or below that shares its columns.
pub fn step(cells: &[Cell], i: usize, s: Step, min: (u32, u32)) -> Vec<Cell> {
    let Some(cur) = cells.get(i).copied() else {
        return cells.to_vec();
    };
    let others = || {
        cells
            .iter()
            .enumerate()
            .filter(move |(j, o)| *j != i && o.shares_columns(cur))
            .map(|(_, o)| *o)
    };
    let (min_w, min_h) = (min.0.max(MIN_W), min.1.max(MIN_H));
    let mut c = cur;
    let mut tie = Tie::First;
    match s {
        Step::Up => {
            // Above the nearest block above: take its row, ahead of it.
            match others().filter(|o| o.y < cur.y).map(|o| o.y).max() {
                Some(y) => c.y = y,
                None => return cells.to_vec(),
            }
        }
        Step::Down => match others().filter(|o| o.y > cur.y).map(|o| o.y).min() {
            Some(y) => {
                c.y = y;
                tie = Tie::After;
            }
            None => return cells.to_vec(),
        },
        Step::Left => c.x = cur.x.saturating_sub(1),
        Step::Right => c.x = (cur.x + 1).min(COLUMNS.saturating_sub(cur.w)),
        Step::Wider => {
            c.w = (cur.w + 1).min(MAX_W);
            if c.right() > COLUMNS {
                c.x = COLUMNS - c.w;
            }
        }
        Step::Narrower => c.w = cur.w.saturating_sub(1).max(min_w),
        Step::Taller => c.h = (cur.h + 1).min(MAX_H),
        Step::Shorter => c.h = cur.h.saturating_sub(1).max(min_h),
    }
    let mut next = cells.to_vec();
    if let Some(slot) = next.get_mut(i) {
        *slot = c;
    }
    settle(&next, Some((i, tie)))
}

/// Where a new block of size `w` × `h` goes: below everything, at the left.
pub fn append(cells: &[Cell], w: u32, h: u32) -> Cell {
    let y = cells.iter().map(|c| c.bottom()).max().unwrap_or(0);
    let w = w.clamp(MIN_W, MAX_W);
    Cell::new(0, y.min(MAX_Y), w, h.clamp(MIN_H, MAX_H))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::*;

    fn no_overlap(cells: &[Cell]) -> bool {
        let with: Vec<(Cell, (u32, u32))> = cells.iter().map(|c| (*c, (2, 1))).collect();
        validate(&with).is_ok()
    }

    #[test]
    fn overlap_rules() {
        let a = Cell::new(0, 0, 4, 2);
        assert!(a.overlaps(Cell::new(3, 1, 2, 2)));
        assert!(!a.overlaps(Cell::new(4, 0, 2, 2)));
        assert!(!a.overlaps(Cell::new(0, 2, 2, 2)));
    }

    #[test]
    fn validate_refuses_overlap_out_of_grid_and_too_small() {
        let ok = [
            (Cell::new(0, 0, 4, 2), (2, 1)),
            (Cell::new(4, 0, 8, 2), (2, 1)),
        ];
        assert_eq!(validate(&ok), Ok(()));
        let overlap = [
            (Cell::new(0, 0, 4, 2), (2, 1)),
            (Cell::new(3, 1, 4, 2), (2, 1)),
        ];
        assert_eq!(validate(&overlap), Err(LayoutError::Overlap(0, 1)));
        assert_eq!(
            validate(&[(Cell::new(10, 0, 4, 2), (2, 1))]),
            Err(LayoutError::OutOfGrid(0))
        );
        assert_eq!(
            validate(&[(Cell::new(0, 0, 3, 2), (4, 3))]),
            Err(LayoutError::TooSmall(0))
        );
        assert_eq!(
            validate(&[(Cell::new(0, 0, 1, 2), (1, 1))]),
            Err(LayoutError::TooSmall(0))
        );
        assert_eq!(
            validate(&[(Cell::new(0, 0, 4, 13), (2, 1))]),
            Err(LayoutError::TooLarge(0))
        );
    }

    #[test]
    fn compact_removes_empty_rows_above_blocks() {
        let cells = [
            Cell::new(0, 3, 4, 2),
            Cell::new(4, 7, 4, 2),
            Cell::new(0, 9, 6, 1),
        ];
        let c = compact(&cells);
        assert_eq!(c[0], Cell::new(0, 0, 4, 2));
        assert_eq!(c[1], Cell::new(4, 0, 4, 2));
        assert_eq!(c[2], Cell::new(0, 2, 6, 1));
    }

    #[test]
    fn dropping_onto_another_block_moves_it_down() {
        // Acceptance test 7: a 4 × 2 block dragged onto another.
        let cells = [Cell::new(0, 0, 4, 2), Cell::new(4, 0, 4, 2)];
        let c = drop_at(&cells, 0, Cell::new(4, 0, 4, 2));
        assert_eq!(c[0], Cell::new(4, 0, 4, 2));
        assert_eq!(c[1], Cell::new(4, 2, 4, 2));
        assert!(no_overlap(&c));
    }

    #[test]
    fn drop_is_kept_inside_the_grid_and_compacted() {
        let cells = [Cell::new(0, 0, 4, 2)];
        let c = drop_at(&cells, 0, Cell::new(11, 30, 4, 2));
        assert_eq!(c[0], Cell::new(8, 0, 4, 2));
    }

    #[test]
    fn keyboard_steps_move_and_resize() {
        let cells = [Cell::new(0, 0, 4, 2), Cell::new(0, 2, 4, 3)];
        // Down: past the block below, which moves up into its place.
        let d = step(&cells, 0, Step::Down, (2, 1));
        assert_eq!(d[1], Cell::new(0, 0, 4, 3));
        assert_eq!(d[0], Cell::new(0, 3, 4, 2));
        // Up again restores the order.
        let u = step(&d, 0, Step::Up, (2, 1));
        assert_eq!(u, cells.to_vec());
        // Nothing above: Up changes nothing.
        assert_eq!(step(&cells, 0, Step::Up, (2, 1)), cells.to_vec());
        let r = step(&cells, 0, Step::Right, (2, 1));
        assert_eq!(r[0].x, 1);
        let l = step(&cells, 0, Step::Left, (2, 1));
        assert_eq!(l[0].x, 0);
        let w = step(&cells, 0, Step::Wider, (2, 1));
        assert_eq!(w[0].w, 5);
        let n = step(&cells, 0, Step::Narrower, (4, 1));
        assert_eq!(n[0].w, 4, "never narrower than the type's minimum");
        let t = step(&cells, 0, Step::Taller, (2, 1));
        assert_eq!(t[0].h, 3);
        assert_eq!(t[1].y, 3, "the block below moves down");
        let s = step(&cells, 0, Step::Shorter, (2, 1));
        assert_eq!(s[0].h, 1);
        assert_eq!(s[1].y, 1, "and comes up again when it shrinks");
    }

    #[test]
    fn right_and_wider_stop_at_the_grid_edge() {
        let cells = [Cell::new(8, 0, 4, 2)];
        assert_eq!(step(&cells, 0, Step::Right, (2, 1))[0].x, 8);
        let w = step(&cells, 0, Step::Wider, (2, 1));
        assert_eq!(w[0], Cell::new(7, 0, 5, 2));
    }

    #[test]
    fn reading_order_is_rows_then_columns() {
        let cells = [
            Cell::new(6, 0, 6, 2),
            Cell::new(0, 2, 4, 2),
            Cell::new(0, 0, 6, 2),
        ];
        assert_eq!(reading_order(&cells), vec![2, 0, 1]);
    }

    #[test]
    fn appended_blocks_go_below_everything() {
        let cells = [Cell::new(0, 0, 4, 2), Cell::new(4, 0, 4, 5)];
        assert_eq!(append(&cells, 4, 3), Cell::new(0, 5, 4, 3));
        assert_eq!(append(&[], 30, 0), Cell::new(0, 0, 12, 1));
    }

    #[test]
    fn every_block_type_fits_its_default_size() {
        for t in BLOCK_TYPES {
            assert!(
                t.min_w <= t.default_w && t.min_h <= t.default_h,
                "{}",
                t.name
            );
            assert!(t.default_w <= MAX_W && t.default_h <= MAX_H);
        }
        assert_eq!(min_size("from_the_future"), (MIN_W, MIN_H));
    }
}
