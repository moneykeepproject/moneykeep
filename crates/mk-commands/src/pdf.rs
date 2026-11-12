//! A minimal PDF 1.4 writer for invoices and estimates (business-sales.md Part E).
//!
//! Only the standard Helvetica or Times fonts (not embedded) with WinAnsiEncoding,
//! text, lines and an optional logo raster (FlateDecode RGB image XObject from a
//! template). No JavaScript, actions, links, forms, annotations or embedded files are
//! ever written. All layout is integer points.

/// One table row.
#[derive(Debug, Clone, Default)]
pub(crate) struct PdfLine {
    pub description: String,
    pub quantity: String,
    pub rate: String,
    pub amount: String,
    /// Subtotal rows print in bold.
    pub emphasis: bool,
}

/// Everything printed on a sales document.
#[derive(Debug, Clone, Default)]
pub(crate) struct SalesPdf {
    pub title: String,
    pub business_name: String,
    pub business_lines: Vec<String>,
    pub number: String,
    pub date: String,
    /// "Due date" or "Valid until" with the date.
    pub second_date: Option<(String, String)>,
    pub terms: Option<String>,
    pub bill_to: Vec<String>,
    pub lines: Vec<PdfLine>,
    pub subtotal: String,
    pub taxes: Vec<(String, String)>,
    pub total: String,
    pub applied: Option<String>,
    pub balance: Option<String>,
    pub exemption: Option<String>,
    pub message: String,
    pub a4: bool,
    /// The heading over the party block ("Bill to" when empty).
    pub party_heading: &'static str,
    /// Replaces the number/date/due/terms facts when set.
    pub facts: Option<Vec<(String, String)>>,
    /// Replaces the table headings (Description, Quantity, Rate, Amount).
    pub headers: Option<[String; 4]>,
    /// Hide the quantity and rate columns.
    pub hide_qty_rate: bool,
    /// Replaces the totals block: (label, amount, bold).
    pub totals: Option<Vec<(String, String, bool)>>,
    /// Further text blocks after the message (payment instructions).
    pub notes: Vec<String>,
    pub style: Style,
}

/// Template presentation (business-dimensions.md Part C).
#[derive(Debug, Clone, Default)]
pub(crate) struct Style {
    /// Title and rule colour.
    pub accent: Option<(u8, u8, u8)>,
    /// Times instead of Helvetica.
    pub times: bool,
    /// Printed at the foot of every page.
    pub footer: Option<String>,
    pub logo: Option<Logo>,
}

/// A logo raster: 8-bit RGB, zlib-compressed (the PDF FlateDecode stream).
#[derive(Debug, Clone)]
pub(crate) struct Logo {
    pub width: u32,
    pub height: u32,
    pub flate_rgb: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Font {
    Regular,
    Bold,
    TimesRegular,
    TimesBold,
}

impl Font {
    fn bold(self) -> bool {
        matches!(self, Font::Bold | Font::TimesBold)
    }
}

// Times-Roman and Times-Bold advance widths for ASCII 32..=126.
const TIMES: [u16; 95] = [
    250, 333, 408, 500, 500, 833, 778, 180, 333, 333, 500, 564, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 278, 278, 564, 564, 564, 444, 921, 722, 667, 667, 722, 611,
    556, 722, 722, 333, 389, 722, 611, 889, 722, 722, 556, 722, 667, 556, 611, 722, 722, 944, 722,
    722, 611, 333, 278, 333, 469, 500, 333, 444, 500, 444, 500, 444, 333, 500, 500, 278, 278, 500,
    278, 778, 500, 500, 500, 500, 333, 389, 278, 500, 500, 722, 500, 500, 444, 480, 200, 480, 541,
];
const TIMES_BOLD: [u16; 95] = [
    250, 333, 555, 500, 500, 1000, 833, 278, 333, 333, 500, 570, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 333, 333, 570, 570, 570, 500, 930, 722, 667, 722, 722, 667,
    611, 778, 778, 389, 500, 778, 667, 944, 722, 778, 611, 778, 722, 556, 667, 722, 722, 1000, 722,
    722, 667, 333, 278, 333, 581, 500, 333, 500, 556, 444, 556, 444, 333, 500, 556, 278, 333, 556,
    278, 833, 556, 500, 556, 556, 444, 389, 333, 556, 500, 722, 500, 500, 444, 394, 220, 394, 520,
];

// Advance widths (1/1000 em) for ASCII 32..=126 from the standard Helvetica metrics.
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
const HELV_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667,
    611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556,
    278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

/// The WinAnsiEncoding byte for a character, if it has one.
fn win_ansi(c: char) -> Option<u8> {
    let u = u32::from(c);
    if (32..=126).contains(&u) || (0xA0..=0xFF).contains(&u) {
        return u8::try_from(u).ok();
    }
    Some(match c {
        '€' => 0x80,
        '‚' => 0x82,
        'ƒ' => 0x83,
        '„' => 0x84,
        '…' => 0x85,
        '†' => 0x86,
        '‡' => 0x87,
        'ˆ' => 0x88,
        '‰' => 0x89,
        'Š' => 0x8A,
        '‹' => 0x8B,
        'Œ' => 0x8C,
        'Ž' => 0x8E,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '˜' => 0x98,
        '™' => 0x99,
        'š' => 0x9A,
        '›' => 0x9B,
        'œ' => 0x9C,
        'ž' => 0x9E,
        'Ÿ' => 0x9F,
        _ => return None,
    })
}

fn width_of(b: u8, f: Font) -> i64 {
    let table = match f {
        Font::Bold => &HELV_BOLD,
        Font::Regular => &HELV,
        Font::TimesRegular => &TIMES,
        Font::TimesBold => &TIMES_BOLD,
    };
    usize::from(b)
        .checked_sub(32)
        .and_then(|i| table.get(i))
        .map_or(556, |w| i64::from(*w))
}

pub(crate) struct Writer {
    pub replaced: Vec<char>,
}

impl Writer {
    /// Encode text to WinAnsi bytes, replacing what cannot be printed with `?`.
    pub(crate) fn encode(&mut self, s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for c in s.chars() {
            if c.is_control() {
                out.push(b' ');
                continue;
            }
            match win_ansi(c) {
                Some(b) => out.push(b),
                None => {
                    if !self.replaced.contains(&c) {
                        self.replaced.push(c);
                    }
                    out.push(b'?');
                }
            }
        }
        out
    }
}

/// Text width in 1/1000 point.
pub(crate) fn width(bytes: &[u8], f: Font, size: i64) -> i64 {
    bytes
        .iter()
        .map(|b| width_of(*b, f))
        .sum::<i64>()
        .saturating_mul(size)
}

/// Break encoded text into lines no wider than `max` points.
pub(crate) fn wrap(bytes: &[u8], f: Font, size: i64, max: i64) -> Vec<Vec<u8>> {
    let limit = max.saturating_mul(1000);
    let mut lines = Vec::new();
    for para in bytes.split(|b| *b == b'\n') {
        let mut line: Vec<u8> = Vec::new();
        for word in para.split(|b| *b == b' ') {
            let mut candidate = line.clone();
            if !candidate.is_empty() {
                candidate.push(b' ');
            }
            candidate.extend_from_slice(word);
            if width(&candidate, f, size) <= limit {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            // A word longer than the line is broken by character.
            for b in word {
                let mut next = line.clone();
                next.push(*b);
                if width(&next, f, size) > limit && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    line.push(*b);
                } else {
                    line = next;
                }
            }
        }
        lines.push(line);
    }
    lines
}

fn literal(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![b'('];
    for b in bytes {
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(*b);
    }
    out.push(b')');
    out
}

/// Content of one page under construction.
pub(crate) struct Page {
    pub ops: Vec<u8>,
}

impl Page {
    pub(crate) fn text(&mut self, x: i64, y: i64, f: Font, size: i64, bytes: &[u8]) {
        let name = if f.bold() { "F2" } else { "F1" };
        self.ops
            .extend_from_slice(format!("BT /{name} {size} Tf {x} {y} Td ").as_bytes());
        self.ops.extend_from_slice(&literal(bytes));
        self.ops.extend_from_slice(b" Tj ET\n");
    }

    pub(crate) fn right(&mut self, x: i64, y: i64, f: Font, size: i64, bytes: &[u8]) {
        let w = width(bytes, f, size) / 1000;
        self.text(x.saturating_sub(w), y, f, size, bytes);
    }

    pub(crate) fn rule(&mut self, x1: i64, x2: i64, y: i64) {
        self.ops
            .extend_from_slice(format!("0.5 w {x1} {y} m {x2} {y} l S\n").as_bytes());
    }
}

const MARGIN: i64 = 50;
const BODY: i64 = 10;
const LEAD: i64 = 14;

struct Layout {
    w: i64,
    h: i64,
    done: Vec<Page>,
    cur: Page,
    y: i64,
}

impl Layout {
    fn page(&mut self) -> &mut Page {
        &mut self.cur
    }

    fn new_page(&mut self) {
        let full = std::mem::replace(&mut self.cur, Page { ops: Vec::new() });
        self.done.push(full);
        self.y = self.h.saturating_sub(MARGIN);
    }

    /// Make room for `needed` points, starting a new page if necessary.
    fn need(&mut self, needed: i64) -> bool {
        if self.y.saturating_sub(needed) < MARGIN + 30 {
            self.new_page();
            return true;
        }
        false
    }
}

/// Render the document: the PDF bytes and the characters replaced with `?`.
pub(crate) fn render(d: &SalesPdf) -> (Vec<u8>, Vec<char>) {
    render_many(std::slice::from_ref(d))
}

/// Several documents in one file (statement batches); the first one's paper, font
/// and logo apply to the file.
pub(crate) fn render_many(docs: &[SalesPdf]) -> (Vec<u8>, Vec<char>) {
    let mut wr = Writer {
        replaced: Vec::new(),
    };
    let mut pages = Vec::new();
    for d in docs {
        pages.extend(render_pages(d, &mut wr));
    }
    let first = docs.first();
    let (w, h) = if first.is_some_and(|d| d.a4) {
        (595, 842)
    } else {
        (612, 792)
    };
    let style = first.map(|d| d.style.clone()).unwrap_or_default();
    (assemble(&pages, w, h, &style), wr.replaced)
}

fn color(rgb: (u8, u8, u8), stroke: bool) -> String {
    let c = |v: u8| {
        let m = u32::from(v) * 1000 / 255;
        format!("{}.{:03}", m / 1000, m % 1000)
    };
    format!(
        "{} {} {} {}\n",
        c(rgb.0),
        c(rgb.1),
        c(rgb.2),
        if stroke { "RG" } else { "rg" }
    )
}

fn render_pages(d: &SalesPdf, wr: &mut Writer) -> Vec<Page> {
    let (reg, bold) = if d.style.times {
        (Font::TimesRegular, Font::TimesBold)
    } else {
        (Font::Regular, Font::Bold)
    };
    let (w, h) = if d.a4 { (595, 842) } else { (612, 792) };
    let mut l = Layout {
        w,
        h,
        done: Vec::new(),
        cur: Page { ops: Vec::new() },
        y: h - MARGIN,
    };
    let right = w - MARGIN;
    let col_amount = right;
    let col_rate = right - 90;
    let col_qty = right - 175;
    let desc_width = if d.hide_qty_rate {
        col_amount - 110 - MARGIN
    } else {
        col_qty - 60 - MARGIN
    };
    if let Some((r, g, b)) = d.style.accent {
        l.page()
            .ops
            .extend_from_slice(color((r, g, b), true).as_bytes());
    }

    // Header: logo and business on the left, title on the right.
    let top = l.y;
    let mut y = top;
    if let Some(logo) = &d.style.logo {
        let (lw, lh) = logo_box(logo);
        l.page().ops.extend_from_slice(
            format!("q {lw} 0 0 {lh} {MARGIN} {} cm /Im1 Do Q\n", y - lh).as_bytes(),
        );
        y -= lh + 8;
    }
    if !d.business_name.is_empty() {
        let b = wr.encode(&d.business_name);
        l.page().text(MARGIN, y - 14, bold, 14, &b);
        y -= 20;
    }
    for line in &d.business_lines {
        let b = wr.encode(line);
        l.page().text(MARGIN, y - BODY, reg, BODY, &b);
        y -= LEAD;
    }
    let title = wr.encode(&d.title);
    if let Some(c) = d.style.accent {
        l.page().ops.extend_from_slice(color(c, false).as_bytes());
    }
    l.page().right(right, top - 20, bold, 20, &title);
    if d.style.accent.is_some() {
        l.page().ops.extend_from_slice(b"0 0 0 rg\n");
    }
    let mut ry = top - 44;
    let facts: Vec<(String, String)> = match &d.facts {
        Some(f) => f.clone(),
        None => {
            let mut f = vec![
                (format!("{} number", d.title), d.number.clone()),
                ("Date".into(), d.date.clone()),
            ];
            if let Some(x) = &d.second_date {
                f.push(x.clone());
            }
            if let Some(t) = &d.terms {
                f.push(("Terms".into(), t.clone()));
            }
            f
        }
    };
    for (k, v) in &facts {
        let kb = wr.encode(k);
        let vb = wr.encode(v);
        l.page().right(right - 110, ry, bold, BODY, &kb);
        l.page().right(right, ry, reg, BODY, &vb);
        ry -= LEAD;
    }
    l.y = y.min(ry) - 16;

    // Bill to.
    let bt = wr.encode(if d.party_heading.is_empty() {
        "Bill to"
    } else {
        d.party_heading
    });
    let yy = l.y;
    l.page().text(MARGIN, yy, bold, BODY, &bt);
    l.y -= LEAD;
    for line in &d.bill_to {
        let b = wr.encode(line);
        let yy = l.y;
        l.page().text(MARGIN, yy, reg, BODY, &b);
        l.y -= LEAD;
    }
    l.y -= 16;

    let heads: [String; 4] = d
        .headers
        .clone()
        .unwrap_or_else(|| ["Description", "Quantity", "Rate", "Amount"].map(str::to_owned));
    let hide = d.hide_qty_rate;
    let header = |l: &mut Layout, wr: &mut Writer| {
        let y = l.y;
        let p = l.page();
        let [h0, h1, h2, h3] = &heads;
        p.text(MARGIN, y, bold, BODY, &wr.encode(h0));
        if !hide {
            p.right(col_qty, y, bold, BODY, &wr.encode(h1));
            p.right(col_rate, y, bold, BODY, &wr.encode(h2));
        }
        p.right(col_amount, y, bold, BODY, &wr.encode(h3));
        p.rule(MARGIN, right, y - 4);
        l.y -= LEAD + 4;
    };
    header(&mut l, wr);
    for row in &d.lines {
        let f = if row.emphasis { bold } else { reg };
        let desc = wr.encode(&row.description);
        let wrapped = wrap(&desc, f, BODY, desc_width);
        let rows = i64::try_from(wrapped.len()).unwrap_or(1).max(1);
        if l.need(rows * LEAD) {
            header(&mut l, wr);
        }
        let y = l.y;
        let q = wr.encode(&row.quantity);
        let r = wr.encode(&row.rate);
        let a = wr.encode(&row.amount);
        let p = l.page();
        if !hide {
            p.right(col_qty, y, f, BODY, &q);
            p.right(col_rate, y, f, BODY, &r);
        }
        p.right(col_amount, y, f, BODY, &a);
        for (i, part) in wrapped.iter().enumerate() {
            let dy = i64::try_from(i).unwrap_or(0) * LEAD;
            l.page().text(MARGIN, y - dy, f, BODY, part);
        }
        l.y -= rows * LEAD;
    }
    let yy = l.y + LEAD - 4;
    l.page().rule(MARGIN, right, yy);
    l.y -= 6;

    // Totals.
    let mut totals: Vec<(String, String, Font)> = Vec::new();
    match &d.totals {
        Some(list) => {
            for (k, v, b) in list {
                totals.push((k.clone(), v.clone(), if *b { bold } else { reg }));
            }
        }
        None => {
            totals.push(("Subtotal".into(), d.subtotal.clone(), reg));
            for (k, v) in &d.taxes {
                totals.push((k.clone(), v.clone(), reg));
            }
            totals.push(("Total".into(), d.total.clone(), bold));
            if let Some(a) = &d.applied {
                totals.push(("Payments and credits".into(), a.clone(), reg));
            }
            if let Some(b) = &d.balance {
                totals.push(("Balance due".into(), b.clone(), bold));
            }
        }
    }
    for (k, v, f) in &totals {
        l.need(LEAD);
        let y = l.y;
        let kb = wr.encode(k);
        let vb = wr.encode(v);
        l.page().right(col_rate, y, *f, BODY, &kb);
        l.page().right(col_amount, y, *f, BODY, &vb);
        l.y -= LEAD;
    }
    l.y -= 10;
    for extra in d
        .exemption
        .iter()
        .chain(std::iter::once(&d.message))
        .chain(d.notes.iter())
    {
        if extra.trim().is_empty() {
            continue;
        }
        let b = wr.encode(extra);
        for part in wrap(&b, reg, BODY, right - MARGIN) {
            l.need(LEAD);
            let y = l.y;
            l.page().text(MARGIN, y, reg, BODY, &part);
            l.y -= LEAD;
        }
        l.y -= 6;
    }

    // Page numbers.
    let mut pages = std::mem::take(&mut l.done);
    pages.push(std::mem::replace(&mut l.cur, Page { ops: Vec::new() }));
    let n = pages.len();
    let footer = d.style.footer.as_deref().map(|f| wr.encode(f));
    for (i, p) in pages.iter_mut().enumerate() {
        if n > 1 {
            let t = format!("Page {} of {n}", i + 1);
            p.right(right, MARGIN - 20, reg, 8, t.as_bytes());
        }
        if let Some(f) = &footer {
            p.text(MARGIN, MARGIN - 20, reg, 8, f);
        }
    }
    let _ = (l.w, l.h);
    pages
}

/// The logo's size on the page: within 160 × 60 points, keeping its shape.
fn logo_box(logo: &Logo) -> (i64, i64) {
    let (w, h) = (i64::from(logo.width.max(1)), i64::from(logo.height.max(1)));
    if w * 60 > h * 160 {
        (160, (h * 160 / w).max(1))
    } else {
        ((w * 60 / h).max(1), 60)
    }
}

/// Write the file: catalog, page tree, two font dictionaries, pages, xref.
fn assemble(pages: &[Page], w: i64, h: i64, style: &Style) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let mut obj = |out: &mut Vec<u8>, body: &[u8]| {
        offsets.push(out.len());
        let n = offsets.len();
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    let count = pages.len();
    // 1 catalog, 2 pages, 3-4 fonts, 5 logo (when any), then page and content pairs.
    let first = if style.logo.is_some() { 6 } else { 5 };
    let kids: Vec<String> = (0..count)
        .map(|i| format!("{} 0 R", first + i * 2))
        .collect();
    obj(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    obj(
        &mut out,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {count} >>",
            kids.join(" ")
        )
        .as_bytes(),
    );
    let (regular, bold) = if style.times {
        ("Times-Roman", "Times-Bold")
    } else {
        ("Helvetica", "Helvetica-Bold")
    };
    for name in [regular, bold] {
        obj(
            &mut out,
            format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /{name} /Encoding /WinAnsiEncoding >>"
            )
            .as_bytes(),
        );
    }
    let mut xobject = String::new();
    if let Some(logo) = &style.logo {
        let mut body = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            logo.width,
            logo.height,
            logo.flate_rgb.len()
        )
        .into_bytes();
        body.extend_from_slice(&logo.flate_rgb);
        body.extend_from_slice(b"\nendstream");
        obj(&mut out, &body);
        xobject = " /XObject << /Im1 5 0 R >>".into();
    }
    for (i, p) in pages.iter().enumerate() {
        let content = first + 1 + i * 2;
        obj(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >>{xobject} >> /Contents {content} 0 R >>"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_and_wraps() {
        let d = SalesPdf {
            title: "Invoice".into(),
            number: "INV-1".into(),
            bill_to: vec!["Café 東京".into()],
            lines: (0..120)
                .map(|i| PdfLine {
                    description: format!(
                        "Line {i} with a long description that wraps around the column"
                    ),
                    quantity: "1".into(),
                    rate: "1.00".into(),
                    amount: "1.00".into(),
                    emphasis: false,
                })
                .collect(),
            ..SalesPdf::default()
        };
        let (bytes, replaced) = render(&d);
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert_eq!(replaced, vec!['東', '京']);
        let text: String = bytes.iter().map(|b| char::from(*b)).collect();
        assert!(text.contains("(Caf\u{e9} ??)"));
        assert!(text.contains("Page 1 of"));
    }
}
