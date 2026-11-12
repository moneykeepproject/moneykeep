//! The mileage report PDF (mileage.md): per vehicle the odometer readings, total
//! miles and miles by purpose kind with percentages; then the trips grouped by
//! purpose kind with subtotals; then the deduction value by kind.
//!
//! Same writer rules as the other MoneyKeep PDFs (`pdf.rs`): standard Helvetica
//! fonts, integer layout in points; no JavaScript, actions, links, forms,
//! annotations or embedded files.

use serde_json::Value;

use crate::pdf::{Font, Page, Writer, wrap};

#[derive(Debug, Clone, Default)]
pub(crate) struct Row {
    pub date: String,
    pub vehicle: String,
    pub route: String,
    pub purpose: String,
    pub miles: String,
    pub rate: String,
    pub amount: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Group {
    pub label: String,
    pub rows: Vec<Row>,
    pub miles: String,
    pub amount: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Report {
    pub book: String,
    pub period: String,
    pub generated: String,
    /// `mileage.summary` vehicles.
    pub vehicles: Vec<Value>,
    pub groups: Vec<Group>,
    /// (kind, miles, value) without personal.
    pub totals: Vec<(String, String, String)>,
    pub total_value: String,
}

const W: i64 = 612;
const H: i64 = 792;
const MARGIN: i64 = 50;
const RIGHT: i64 = W - MARGIN;
const SIZE: i64 = 8;
const LEAD: i64 = 11;
/// Trip columns: Date, Vehicle, Route, Purpose (left edges and widths); Miles, Rate
/// and Amount are right-aligned at their right edges.
const COLS: [(i64, i64); 4] = [(50, 50), (102, 78), (182, 120), (304, 116)];
const MILES_R: i64 = 466;
const RATE_R: i64 = 506;
const AMOUNT_R: i64 = RIGHT;

struct Doc {
    pages: Vec<(Page, Vec<usize>)>,
    cur: Page,
    y: i64,
}

impl Doc {
    fn need(&mut self, h: i64) -> bool {
        if self.y - h < MARGIN + 20 {
            let p = std::mem::replace(&mut self.cur, Page { ops: Vec::new() });
            self.pages.push((p, Vec::new()));
            self.y = H - MARGIN;
            return true;
        }
        false
    }
}

/// The standard PDF fonts have no arrow.
fn printable(s: &str) -> String {
    s.replace('→', "->")
}

fn st(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default()
}

fn grouped_miles(s: &str) -> String {
    match s.parse::<mk_core::Decimal>() {
        Ok(d) => crate::mileage::miles_text(d),
        Err(_) => s.to_owned(),
    }
}

fn trip_header(d: &mut Doc, wr: &mut Writer) {
    let y = d.y;
    for ((x, _), label) in COLS.iter().zip(["Date", "Vehicle", "Route", "Purpose"]) {
        d.cur.text(*x, y, Font::Bold, SIZE, &wr.encode(label));
    }
    d.cur.right(MILES_R, y, Font::Bold, SIZE, b"Miles");
    d.cur.right(RATE_R, y, Font::Bold, SIZE, b"Rate");
    d.cur.right(AMOUNT_R, y, Font::Bold, SIZE, b"Amount");
    d.cur.rule(MARGIN, RIGHT, y - 4);
    d.y -= LEAD + 4;
}

pub(crate) fn render(r: &Report) -> (Vec<u8>, Vec<char>) {
    let mut wr = Writer {
        replaced: Vec::new(),
    };
    let mut d = Doc {
        pages: Vec::new(),
        cur: Page { ops: Vec::new() },
        y: H - MARGIN,
    };
    let y = d.y;
    if !r.book.is_empty() {
        d.cur
            .text(MARGIN, y - 12, Font::Bold, 12, &wr.encode(&r.book));
    }
    d.cur
        .right(RIGHT, y - 18, Font::Bold, 18, b"Mileage report");
    d.y -= 36;
    let y = d.y;
    d.cur.text(
        MARGIN,
        y,
        Font::Regular,
        10,
        &wr.encode(&format!("Period: {}", r.period)),
    );
    d.cur.right(
        RIGHT,
        y,
        Font::Regular,
        10,
        &wr.encode(&format!("Generated {}", r.generated)),
    );
    d.y -= 24;

    // Vehicles.
    if r.vehicles.is_empty() {
        let y = d.y;
        d.cur.text(
            MARGIN,
            y,
            Font::Regular,
            SIZE + 1,
            b"No trips in this period.",
        );
        d.y -= LEAD + 6;
    }
    for v in &r.vehicles {
        d.need(LEAD * 5);
        let y = d.y;
        d.cur
            .text(MARGIN, y, Font::Bold, 11, &wr.encode(&st(v, "name")));
        d.y -= LEAD + 3;
        let start = st(v, "odometer_start");
        let end = st(v, "odometer_end");
        let odo = match (start.is_empty(), end.is_empty()) {
            (false, false) => format!(
                "Odometer {} to {}",
                grouped_miles(&start),
                grouped_miles(&end)
            ),
            (false, true) => format!("Odometer at start {}", grouped_miles(&start)),
            (true, false) => format!("Odometer at end {}", grouped_miles(&end)),
            (true, true) => "Odometer readings not recorded; total from the trips logged".into(),
        };
        let y = d.y;
        d.cur
            .text(MARGIN, y, Font::Regular, SIZE + 1, &wr.encode(&odo));
        d.cur.right(
            AMOUNT_R,
            y,
            Font::Bold,
            SIZE + 1,
            &wr.encode(&format!(
                "Total miles {}",
                grouped_miles(&st(v, "total_miles"))
            )),
        );
        d.y -= LEAD + 2;
        let y = d.y;
        d.cur
            .text(MARGIN + 10, y, Font::Bold, SIZE, b"Purpose kind");
        d.cur.right(MILES_R, y, Font::Bold, SIZE, b"Miles");
        d.cur.right(RATE_R, y, Font::Bold, SIZE, b"Percent");
        d.cur.right(AMOUNT_R, y, Font::Bold, SIZE, b"Value");
        d.y -= LEAD;
        for k in v
            .get("kinds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            d.need(LEAD);
            let y = d.y;
            d.cur.text(
                MARGIN + 10,
                y,
                Font::Regular,
                SIZE,
                &wr.encode(&st(&k, "label")),
            );
            d.cur.right(
                MILES_R,
                y,
                Font::Regular,
                SIZE,
                &wr.encode(&grouped_miles(&st(&k, "miles"))),
            );
            d.cur.right(
                RATE_R,
                y,
                Font::Regular,
                SIZE,
                &wr.encode(&format!("{} %", st(&k, "percent"))),
            );
            let value = if st(&k, "kind") == "personal" {
                String::new()
            } else {
                crate::sales::group(&st(&k, "value"))
            };
            d.cur
                .right(AMOUNT_R, y, Font::Regular, SIZE, &wr.encode(&value));
            d.y -= LEAD;
        }
        d.y -= 10;
    }

    // Trips by purpose kind.
    if !r.groups.is_empty() {
        d.need(LEAD * 4);
        let y = d.y;
        d.cur.text(MARGIN, y, Font::Bold, 12, b"Trips");
        d.y -= LEAD + 6;
    }
    for g in &r.groups {
        d.need(LEAD * 4);
        let y = d.y;
        d.cur.text(MARGIN, y, Font::Bold, 10, &wr.encode(&g.label));
        d.y -= LEAD + 2;
        trip_header(&mut d, &mut wr);
        for row in &g.rows {
            let cells: Vec<Vec<Vec<u8>>> = [&row.date, &row.vehicle, &row.route, &row.purpose]
                .iter()
                .zip(COLS)
                .map(|(t, (_, w))| wrap(&wr.encode(&printable(t)), Font::Regular, SIZE, w - 4))
                .collect();
            let lines = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
            let h = i64::try_from(lines).unwrap_or(1) * LEAD;
            if d.need(h) {
                trip_header(&mut d, &mut wr);
            }
            let y = d.y;
            for (cell, (x, _)) in cells.iter().zip(COLS) {
                for (i, part) in cell.iter().enumerate() {
                    let dy = i64::try_from(i).unwrap_or(0) * LEAD;
                    d.cur.text(x, y - dy, Font::Regular, SIZE, part);
                }
            }
            d.cur
                .right(MILES_R, y, Font::Regular, SIZE, &wr.encode(&row.miles));
            d.cur.right(
                RATE_R,
                y,
                Font::Regular,
                SIZE,
                &wr.encode(&format!("{}¢", row.rate)),
            );
            d.cur
                .right(AMOUNT_R, y, Font::Regular, SIZE, &wr.encode(&row.amount));
            d.y -= h;
        }
        d.need(LEAD);
        let y = d.y;
        d.cur.rule(MARGIN, RIGHT, y + LEAD - 4);
        d.cur.text(
            COLS[0].0,
            y - 2,
            Font::Bold,
            SIZE,
            &wr.encode(&format!("Subtotal {}", g.label.to_lowercase())),
        );
        d.cur
            .right(MILES_R, y - 2, Font::Bold, SIZE, &wr.encode(&g.miles));
        d.cur
            .right(AMOUNT_R, y - 2, Font::Bold, SIZE, &wr.encode(&g.amount));
        d.y -= LEAD + 10;
    }

    // Grand totals of deduction value by kind.
    if !r.totals.is_empty() {
        d.need(LEAD * (3 + i64::try_from(r.totals.len()).unwrap_or(0)));
        let y = d.y;
        d.cur.text(
            MARGIN,
            y,
            Font::Bold,
            10,
            b"Deduction value by purpose kind",
        );
        d.cur.right(MILES_R, y, Font::Bold, SIZE, b"Miles");
        d.cur.right(AMOUNT_R, y, Font::Bold, SIZE, b"Value");
        d.cur.rule(MARGIN, RIGHT, y - 4);
        d.y -= LEAD + 4;
        for (label, miles, value) in &r.totals {
            let y = d.y;
            d.cur
                .text(MARGIN, y, Font::Regular, SIZE + 1, &wr.encode(label));
            d.cur
                .right(MILES_R, y, Font::Regular, SIZE + 1, &wr.encode(miles));
            d.cur
                .right(AMOUNT_R, y, Font::Regular, SIZE + 1, &wr.encode(value));
            d.y -= LEAD;
        }
        let y = d.y - 2;
        d.cur.text(MARGIN, y, Font::Bold, 10, b"Total");
        d.cur
            .right(AMOUNT_R, y, Font::Bold, 10, &wr.encode(&r.total_value));
        d.y -= LEAD + 8;
        for line in wrap(
            &wr.encode(crate::mileage::TAX_NOTE),
            Font::Regular,
            8,
            RIGHT - MARGIN,
        ) {
            d.need(LEAD);
            let y = d.y;
            d.cur.text(MARGIN, y, Font::Regular, 8, &line);
            d.y -= LEAD;
        }
    }

    let last = std::mem::replace(&mut d.cur, Page { ops: Vec::new() });
    d.pages.push((last, Vec::new()));
    let n = d.pages.len();
    if n > 1 {
        for (i, (p, _)) in d.pages.iter_mut().enumerate() {
            let t = format!("Page {} of {n}", i + 1);
            p.right(RIGHT, MARGIN - 20, Font::Regular, 8, t.as_bytes());
        }
    }
    (crate::expense_pdf::assemble(&d.pages, &[]), wr.replaced)
}
