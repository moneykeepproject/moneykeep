//! The expense report PDF (expenses.md): a table of expenses with totals by
//! category, the grand total and the reimbursable total, then one section per
//! expense with its receipt pictures fitted to the page width.
//!
//! Same writer rules as invoices (`pdf.rs`): standard Helvetica fonts (not
//! embedded), integer layout in points, image XObjects (JPEG receipts as
//! DCTDecode streams, PNG receipts as FlateDecode RGB). No JavaScript, actions,
//! links, forms, annotations or embedded files are ever written.

use crate::pdf::{Font, Page, Writer, wrap};

/// One table row.
#[derive(Debug, Clone, Default)]
pub(crate) struct Row {
    pub date: String,
    pub merchant: String,
    pub category: String,
    pub paid_with: String,
    pub purpose: String,
    pub amount: String,
    pub receipt: Receipt,
}

/// The Receipt column.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Receipt {
    Yes,
    #[default]
    Missing,
    /// A mileage trip: no receipt is needed.
    NotApplicable,
}

/// One receipt in an expense's section.
#[derive(Debug, Clone)]
pub(crate) enum Item {
    /// A stored baseline JPEG, embedded as it is.
    Jpeg {
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
    /// An RGB raster, zlib-compressed.
    Raster {
        width: u32,
        height: u32,
        flate_rgb: Vec<u8>,
    },
    /// A PDF receipt: listed by name, never merged.
    Pdf { file_name: String },
    /// A line of text in place of a receipt (a trip's route, miles and rate).
    Text(String),
}

#[derive(Debug, Clone)]
pub(crate) struct Section {
    pub heading: String,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Report {
    pub book: String,
    pub title: String,
    pub period: String,
    pub generated: String,
    pub rows: Vec<Row>,
    pub totals: Vec<(String, String)>,
    pub total: String,
    pub reimbursable: String,
    pub reimbursed: String,
    pub owed: String,
    pub sections: Vec<Section>,
}

const W: i64 = 612;
const H: i64 = 792;
const MARGIN: i64 = 50;
const SIZE: i64 = 8;
const LEAD: i64 = 11;
const RIGHT: i64 = W - MARGIN;

/// Column left edges and widths: Date, Merchant, Category, Paid with, Purpose;
/// then Amount (right-aligned at its right edge) and Receipt.
const COLS: [(i64, i64); 5] = [(50, 52), (104, 84), (190, 84), (276, 70), (348, 92)];
const AMOUNT_RIGHT: i64 = 502;
const RECEIPT_X: i64 = 510;

struct Doc {
    pages: Vec<(Page, Vec<usize>)>,
    cur: Page,
    cur_images: Vec<usize>,
    y: i64,
}

impl Doc {
    fn new_page(&mut self) {
        let page = std::mem::replace(&mut self.cur, Page { ops: Vec::new() });
        let imgs = std::mem::take(&mut self.cur_images);
        self.pages.push((page, imgs));
        self.y = H - MARGIN;
    }

    fn need(&mut self, h: i64) -> bool {
        if self.y - h < MARGIN + 20 {
            self.new_page();
            return true;
        }
        false
    }
}

/// The table header line.
fn header(d: &mut Doc, wr: &mut Writer) {
    let y = d.y;
    for ((x, _), label) in COLS
        .iter()
        .zip(["Date", "Merchant", "Category", "Paid with", "Purpose"])
    {
        d.cur.text(*x, y, Font::Bold, SIZE, &wr.encode(label));
    }
    d.cur.right(AMOUNT_RIGHT, y, Font::Bold, SIZE, b"Amount");
    d.cur.text(RECEIPT_X, y, Font::Bold, SIZE, b"Receipt");
    d.cur.rule(MARGIN, RIGHT, y - 4);
    d.y -= LEAD + 4;
}

/// Image size on the page: the page width, at most half the page high, shape kept.
fn fit(w: u32, h: u32) -> (i64, i64) {
    let (w, h) = (i64::from(w.max(1)), i64::from(h.max(1)));
    let max_w = RIGHT - MARGIN;
    let max_h = (H - 2 * MARGIN) / 2 - 24;
    let (mut dw, mut dh) = (max_w, (h * max_w / w).max(1));
    if dh > max_h {
        dh = max_h;
        dw = (w * max_h / h).max(1);
    }
    (dw, dh)
}

/// Render the report: the PDF bytes and the characters replaced with `?`.
pub(crate) fn render(r: &Report) -> (Vec<u8>, Vec<char>) {
    let mut wr = Writer {
        replaced: Vec::new(),
    };
    let mut d = Doc {
        pages: Vec::new(),
        cur: Page { ops: Vec::new() },
        cur_images: Vec::new(),
        y: H - MARGIN,
    };
    let mut images: Vec<&Item> = Vec::new();

    // Heading.
    let y = d.y;
    if !r.book.is_empty() {
        d.cur
            .text(MARGIN, y - 12, Font::Bold, 12, &wr.encode(&r.book));
    }
    d.cur
        .right(RIGHT, y - 18, Font::Bold, 18, &wr.encode(&r.title));
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

    header(&mut d, &mut wr);
    if r.rows.is_empty() {
        let y = d.y;
        d.cur.text(
            MARGIN,
            y,
            Font::Regular,
            SIZE,
            b"No expenses in this period.",
        );
        d.y -= LEAD;
    }
    for row in &r.rows {
        let cells: Vec<Vec<Vec<u8>>> = [
            &row.date,
            &row.merchant,
            &row.category,
            &row.paid_with,
            &row.purpose,
        ]
        .iter()
        .zip(COLS)
        .map(|(t, (_, w))| wrap(&wr.encode(t), Font::Regular, SIZE, w - 4))
        .collect();
        let lines = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
        let h = i64::try_from(lines).unwrap_or(1) * LEAD;
        if d.need(h) {
            header(&mut d, &mut wr);
        }
        let y = d.y;
        for (cell, (x, _)) in cells.iter().zip(COLS) {
            for (i, part) in cell.iter().enumerate() {
                let dy = i64::try_from(i).unwrap_or(0) * LEAD;
                d.cur.text(x, y - dy, Font::Regular, SIZE, part);
            }
        }
        d.cur.right(
            AMOUNT_RIGHT,
            y,
            Font::Regular,
            SIZE,
            &wr.encode(&row.amount),
        );
        let mark: &[u8] = match row.receipt {
            Receipt::Yes => b"Yes",
            Receipt::Missing => b"Missing",
            Receipt::NotApplicable => b"N/A",
        };
        d.cur.text(RECEIPT_X, y, Font::Regular, SIZE, mark);
        d.y -= h;
    }
    let y = d.y + LEAD - 4;
    d.cur.rule(MARGIN, RIGHT, y);
    d.y -= 10;

    // Totals by category, grand total, reimbursable total.
    d.need(LEAD * 2);
    let y = d.y;
    d.cur.text(MARGIN, y, Font::Bold, 10, b"Totals by category");
    d.y -= LEAD + 3;
    for (cat, amount) in &r.totals {
        d.need(LEAD);
        let y = d.y;
        let c = wr.encode(cat);
        d.cur.text(MARGIN, y, Font::Regular, SIZE + 1, &c);
        d.cur
            .right(AMOUNT_RIGHT, y, Font::Regular, SIZE + 1, &wr.encode(amount));
        d.y -= LEAD;
    }
    d.y -= 4;
    let lines: [(&str, &str, Font); 4] = [
        ("Total", &r.total, Font::Bold),
        (
            "Reimbursable (paid with personal funds)",
            &r.reimbursable,
            Font::Regular,
        ),
        ("Reimbursements recorded", &r.reimbursed, Font::Regular),
        ("Reimbursable total owed", &r.owed, Font::Bold),
    ];
    for (label, amount, f) in lines {
        d.need(LEAD + 2);
        let y = d.y;
        d.cur.text(MARGIN, y, f, 10, &wr.encode(label));
        d.cur.right(AMOUNT_RIGHT, y, f, 10, &wr.encode(amount));
        d.y -= LEAD + 3;
    }

    // Receipts, one section per expense.
    if !r.sections.is_empty() {
        d.new_page();
        let y = d.y;
        d.cur.text(MARGIN, y - 14, Font::Bold, 14, b"Receipts");
        d.y -= 30;
    }
    for s in &r.sections {
        d.need(LEAD * 3);
        let y = d.y;
        d.cur
            .text(MARGIN, y, Font::Bold, 10, &wr.encode(&s.heading));
        d.y -= LEAD + 4;
        if s.items.is_empty() {
            let y = d.y;
            d.cur
                .text(MARGIN, y, Font::Regular, SIZE + 1, b"No receipt");
            d.y -= LEAD + 6;
            continue;
        }
        for item in &s.items {
            match item {
                Item::Text(line) => {
                    let t = wr.encode(&line.replace('→', "->"));
                    for part in wrap(&t, Font::Regular, SIZE + 1, RIGHT - MARGIN) {
                        d.need(LEAD);
                        let y = d.y;
                        d.cur.text(MARGIN, y, Font::Regular, SIZE + 1, &part);
                        d.y -= LEAD;
                    }
                    d.y -= 2;
                }
                Item::Pdf { file_name } => {
                    let t = wr.encode(&format!("{file_name} (PDF, attached separately)"));
                    for part in wrap(&t, Font::Regular, SIZE + 1, RIGHT - MARGIN) {
                        d.need(LEAD);
                        let y = d.y;
                        d.cur.text(MARGIN, y, Font::Regular, SIZE + 1, &part);
                        d.y -= LEAD;
                    }
                    d.y -= 2;
                }
                Item::Jpeg { width, height, .. } | Item::Raster { width, height, .. } => {
                    let (dw, dh) = fit(*width, *height);
                    d.need(dh + 8);
                    images.push(item);
                    let n = images.len();
                    d.cur_images.push(n);
                    let y = d.y - dh;
                    d.cur.ops.extend_from_slice(
                        format!("q {dw} 0 0 {dh} {MARGIN} {y} cm /Im{n} Do Q\n").as_bytes(),
                    );
                    d.y -= dh + 10;
                }
            }
        }
        d.y -= 6;
    }

    let last = std::mem::replace(&mut d.cur, Page { ops: Vec::new() });
    let last_images = std::mem::take(&mut d.cur_images);
    d.pages.push((last, last_images));
    let n = d.pages.len();
    if n > 1 {
        for (i, (p, _)) in d.pages.iter_mut().enumerate() {
            let t = format!("Page {} of {n}", i + 1);
            p.right(RIGHT, MARGIN - 20, Font::Regular, 8, t.as_bytes());
        }
    }
    (assemble(&d.pages, &images), wr.replaced)
}

/// Write the file: catalog, page tree, two fonts, the images, then pages.
pub(crate) fn assemble(pages: &[(Page, Vec<usize>)], images: &[&Item]) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let mut obj = |out: &mut Vec<u8>, body: &[u8]| {
        offsets.push(out.len());
        let n = offsets.len();
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    // 1 catalog, 2 pages, 3-4 fonts, 5.. images, then page and content pairs.
    let first_image = 5;
    let first_page = first_image + images.len();
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", first_page + i * 2))
        .collect();
    obj(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    obj(
        &mut out,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages.len()
        )
        .as_bytes(),
    );
    for name in ["Helvetica", "Helvetica-Bold"] {
        obj(
            &mut out,
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /{name} /Encoding /WinAnsiEncoding >>"
            )
            .as_bytes(),
        );
    }
    for img in images {
        let (w, h, filter, data) = match img {
            Item::Jpeg {
                width,
                height,
                data,
            } => (*width, *height, "DCTDecode", data.as_slice()),
            Item::Raster {
                width,
                height,
                flate_rgb,
            } => (*width, *height, "FlateDecode", flate_rgb.as_slice()),
            Item::Pdf { .. } | Item::Text(_) => (1, 1, "FlateDecode", &[][..]),
        };
        let mut body = format!(
            "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /{filter} /Length {} >>\nstream\n",
            data.len()
        )
        .into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        obj(&mut out, &body);
    }
    for (i, (p, imgs)) in pages.iter().enumerate() {
        let content = first_page + 1 + i * 2;
        let xobjects = if imgs.is_empty() {
            String::new()
        } else {
            let refs: Vec<String> = imgs
                .iter()
                .map(|n| format!("/Im{n} {} 0 R", first_image + n - 1))
                .collect();
            format!(" /XObject << {} >>", refs.join(" "))
        };
        obj(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {W} {H}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >>{xobjects} >> /Contents {content} 0 R >>"
            )
            .as_bytes(),
        );
        let mut body = format!("<< /Length {} >>\nstream\n", p.ops.len()).into_bytes();
        body.extend_from_slice(&p.ops);
        body.extend_from_slice(b"\nendstream");
        obj(&mut out, &body);
    }
    // The document information: who made it (branding.md).
    obj(
        &mut out,
        format!("<< /Producer ({}) >>", mk_core::brand::PRODUCT).as_bytes(),
    );
    let info = offsets.len();
    let xref = out.len();
    let total = offsets.len() + 1;
    out.extend_from_slice(format!("xref\n0 {total}\n0000000000 65535 f \n").as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {total} /Root 1 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}
