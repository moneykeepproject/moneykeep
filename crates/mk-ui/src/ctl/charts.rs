//! Report charts as view-model drawings: accent colour and neutral greys, axes
//! labelled in whole currency units, and an accessible description of the series
//! with the axis ticks. The tables under them always hold the exact numbers.
//!
//! Geometry is worked out in pixels for the width the chart will get and handed
//! over in the drawing's 0–1000 box, so labels keep their size and spacing.
#![allow(clippy::float_arithmetic)]

use std::fmt::Write as _;

use mk_core::Decimal;

pub(crate) use crate::base::period::ticks;

use super::ui::{Align, Chart, ChartHit, ChartLabel, ChartPath, ChartPiece, LegendEntry, Ui};
use crate::fmt;

pub(crate) const HEIGHT: i32 = 190;
const LEFT: f64 = 76.0;
const BOTTOM: f64 = 36.0;
/// Space above the plot.
const TOP: f64 = 10.0;
/// The smallest plot: about two text lines (dashboards.md revision 4).
const MIN_PLOT: f64 = 32.0;
/// The legend as the views lay it out (`ChartView`): lines of [`LEGEND_LINE`] px
/// [`LEGEND_ROW_GAP`] apart under the drawing ([`CHART_GAP`] above them), entries
/// [`ENTRY_GAP`] apart, each a swatch and its gap ([`SWATCH`]) before the text.
pub(crate) const LEGEND_LINE: i32 = 16;
const LEGEND_ROW_GAP: i32 = 4;
const CHART_GAP: i32 = 8;
const ENTRY_GAP: f64 = 12.0;
const SWATCH: f64 = 14.0;
/// Horizontal bars: the tallest and shortest slot per bar (a slot holds a bar and
/// its gap; the shortest still fits a caption line).
const BAR_SLOT: f64 = 24.0;
const MIN_BAR_SLOT: f64 = 16.0;
const MAX_BAR_SLOT: f64 = 36.0;
/// Series indices the views map to theme colours.
const ACCENT: i32 = 0;
const GRID: i32 = -2;
const MUTED: i32 = -3;
const TEXT: i32 = -4;
/// Approximate width of one caption character in pixels.
const CHAR_W: f64 = 7.0;

fn to_f(d: Decimal) -> f64 {
    d.to_string().parse().unwrap_or(0.0)
}

/// Whole currency units with thousands separators.
fn whole(v: f64) -> String {
    let d = Decimal::from_f64_retain(v.round())
        .unwrap_or_default()
        .round();
    fmt::group(&d.to_string())
}

/// Pixel space of one chart, mapped to the 0–1000 drawing box.
struct Geo {
    w: f64,
    h: f64,
}

impl Geo {
    fn x(&self, px: f64) -> i32 {
        (px * 1000.0 / self.w).round() as i32
    }
    fn y(&self, px: f64) -> i32 {
        (px * 1000.0 / self.h).round() as i32
    }
    fn pt(&self, x: f64, y: f64) -> String {
        format!("{} {}", self.x(x), self.y(y))
    }
    fn rect(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> String {
        format!(
            "M {} L {} L {} L {} Z ",
            self.pt(x0, y0),
            self.pt(x1, y0),
            self.pt(x1, y1),
            self.pt(x0, y1)
        )
    }
    /// A small square marker standing for a dot.
    fn dot(&self, x: f64, y: f64, r: f64) -> String {
        self.rect(x - r, y - r, x + r, y + r)
    }
}

/// The plot area in pixels.
#[derive(Clone, Copy)]
struct Plot {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Plot {
    fn new(w: f64, h: f64) -> Self {
        Self {
            left: LEFT,
            top: TOP,
            right: w - 8.0,
            bottom: h - BOTTOM,
        }
    }
    fn width(&self) -> f64 {
        self.right - self.left
    }
}

/// Value axis on round ticks.
struct Axis {
    lo: f64,
    hi: f64,
    ticks: Vec<f64>,
    plot: Plot,
}

impl Axis {
    fn new(values: impl Iterator<Item = f64>, plot: Plot) -> Self {
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for v in values {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        let ticks = ticks(lo, hi);
        let lo = ticks.first().copied().unwrap_or(0.0);
        let hi = ticks.last().copied().unwrap_or(1.0);
        Self {
            lo,
            hi,
            ticks,
            plot,
        }
    }
    fn y(&self, v: f64) -> f64 {
        let span = if self.hi > self.lo {
            self.hi - self.lo
        } else {
            1.0
        };
        self.plot.bottom - (v - self.lo) / span * (self.plot.bottom - self.plot.top)
    }
    fn note(&self) -> String {
        let labels: Vec<String> = self.ticks.iter().map(|t| whole(*t)).collect();
        format!("Axis ticks: {}", labels.join("; "))
    }
    fn draw(&self, g: &Geo, c: &mut Chart) {
        let mut grid = String::new();
        for t in &self.ticks {
            let y = self.y(*t);
            let _ = write!(
                grid,
                "M {} L {} ",
                g.pt(self.plot.left, y),
                g.pt(self.plot.right, y)
            );
            c.labels.push(ChartLabel {
                text: whole(*t),
                x: g.x(self.plot.left - 6.0),
                y: g.y(y),
                align: Align::Right,
            });
        }
        c.paths.push(ChartPath {
            commands: grid,
            stroke: Some(0),
            stroke_width: 1,
            series: GRID,
            ..ChartPath::default()
        });
        let zero = self.y(0.0);
        c.paths.push(ChartPath {
            commands: format!(
                "M {} L {}",
                g.pt(self.plot.left, zero),
                g.pt(self.plot.right, zero)
            ),
            stroke: Some(0),
            stroke_width: 1,
            series: MUTED,
            ..ChartPath::default()
        });
        c.note = self.note();
    }
}

fn text_w(t: &str) -> f64 {
    t.chars().count() as f64 * CHAR_W
}

/// How tall a chart is told to be: its drawing alone (the reports screen, where the
/// card grows with the legend) or the whole chart with its legend (a dashboard
/// block, where the drawing takes what the legend leaves).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fit {
    Drawing(i32),
    Total(i32),
}

impl Fit {
    /// The drawing's height once `legend` pixels are taken, at least `min`.
    fn drawing(self, legend: i32, min: f64) -> f64 {
        let h = match self {
            Fit::Drawing(h) => h,
            Fit::Total(t) => t.saturating_sub(legend),
        };
        f64::from(h).max(min)
    }
}

/// What a click on a chart opens (dashboards.md revision 5): its pieces (bars and
/// points), and its legend and empty area too; `period` names the chart's period
/// in the tooltips of charts whose pieces are not periods.
#[derive(Debug, Clone, Default)]
pub(crate) struct Opens {
    pub pieces: bool,
    pub more: bool,
    pub period: String,
}

/// The smallest hit area of a piece, in pixels (WCAG 2.5.8).
const MIN_HIT: f64 = 24.0;

/// A piece's box in pixels grown to at least [`MIN_HIT`] each way about its centre
/// and kept inside the drawing, as 0–1000 box coordinates.
fn hit_box(g: &Geo, x0: f64, y0: f64, x1: f64, y1: f64) -> (i32, i32, i32, i32) {
    let grow = |a: f64, b: f64, max: f64| {
        let (a, b) = (a.min(b), a.max(b));
        let pad = ((MIN_HIT - (b - a)) / 2.0).max(0.0);
        let (mut a, mut b) = (a - pad, b + pad);
        if a < 0.0 {
            b -= a;
            a = 0.0;
        }
        if b > max {
            a -= b - max;
            b = max;
        }
        (a.max(0.0), b.min(max))
    };
    let (x0, x1) = grow(x0, x1, g.w);
    let (y0, y1) = grow(y0, y1, g.h);
    (g.x(x0), g.y(y0), g.x(x1), g.y(y1))
}

fn box_of(g: &Geo, x0: f64, y0: f64, x1: f64, y1: f64) -> (i32, i32, i32, i32) {
    (
        g.x(x0.min(x1)),
        g.y(y0.min(y1)),
        g.x(x0.max(x1)),
        g.y(y0.max(y1)),
    )
}

/// A period label as words ("2026-10" → "Oct 2026").
pub(crate) fn period_name(label: &str) -> String {
    match crate::base::period::short_label(label) {
        (Some(y), short) if !short.contains(&y) => format!("{short} {y}"),
        (_, short) => short,
    }
}

fn amount(v: Decimal) -> String {
    fmt::group(&v.to_string())
}

/// Hand the chart to the view, clickable as `opens` says.
fn show(ui: &mut Ui, desc: &str, mut c: Chart, opens: &Opens) -> Option<ChartHit> {
    if !opens.pieces {
        c.pieces.clear();
    }
    c.open_more = opens.more;
    ui.chart(desc, c)
}

/// The smallest drawing of an axis chart: the plot about two text lines tall above
/// the period labels.
fn min_axis_drawing() -> f64 {
    TOP + MIN_PLOT + BOTTOM
}

/// Lay the legend out in lines that fit `width` (dashboards.md revision 4): entries
/// flow left to right and wrap, never split; an entry wider than a line is
/// shortened with "…" and keeps its full text as the tooltip.
pub(crate) fn wrap_legend(entries: &[LegendEntry], width: i32) -> Vec<LegendEntry> {
    let width = f64::from(width.max(1));
    let mut out = Vec::with_capacity(entries.len());
    let (mut row, mut used) = (0, 0.0f64);
    for e in entries {
        let mut e = e.clone();
        let full = SWATCH + text_w(&e.text);
        let w = if full > width {
            let keep = ((width - SWATCH) / CHAR_W).floor().max(2.0) as usize;
            e.tip = e.text.clone();
            e.text = format!(
                "{}…",
                e.text
                    .chars()
                    .take(keep.saturating_sub(1))
                    .collect::<String>()
            );
            SWATCH + text_w(&e.text)
        } else {
            full
        };
        if used > 0.0 && used + ENTRY_GAP + w > width {
            row += 1;
            used = 0.0;
        }
        used += if used > 0.0 { ENTRY_GAP } else { 0.0 } + w;
        e.row = row;
        out.push(e);
    }
    out
}

/// The height a laid-out legend takes under the drawing (with the gap above it).
pub(crate) fn legend_height(legend: &[LegendEntry]) -> i32 {
    match legend.iter().map(|e| e.row).max() {
        None => 0,
        Some(last) => {
            let lines = last + 1;
            CHART_GAP + lines * LEGEND_LINE + (lines - 1) * LEGEND_ROW_GAP
        }
    }
}

/// The legend of a series chart from its series names.
fn legend_of(names: &[(&str, i32)]) -> Vec<LegendEntry> {
    names
        .iter()
        .map(|(n, s)| LegendEntry {
            series: *s,
            text: (*n).to_owned(),
            ..LegendEntry::default()
        })
        .collect()
}

/// The smallest whole height (drawing and legend) of a chart `width` pixels wide:
/// horizontal bars need a caption line per bar, axis charts a plot about two text
/// lines tall above their period labels; `legend` names the legend entries.
pub(crate) fn min_height(bars: usize, legend: &[&str], width: i32) -> i32 {
    if bars > 0 {
        return (MIN_BAR_SLOT * bars as f64 + 8.0).ceil() as i32;
    }
    let names: Vec<(&str, i32)> = legend.iter().map(|n| (*n, MUTED)).collect();
    let l = legend_height(&wrap_legend(&legend_of(&names), width));
    min_axis_drawing().ceil() as i32 + l
}

/// Period labels under the plot: the table's short labels, the year under the first
/// labelled point and each new year's first month, never overlapping, the last
/// point always labelled.
fn x_labels(g: &Geo, plot: Plot, labels: &[String], centers: &[f64], c: &mut Chart) {
    let heads = crate::base::period::headers(labels);
    let (lo, hi) = (plot.left - 4.0, plot.right + 6.0);
    let place = |text: &str, x: f64| {
        let w = text_w(text);
        let left = (x - w / 2.0).clamp(lo, (hi - w).max(lo));
        (left, left + w)
    };
    let gap = 8.0;
    let mut chosen: Vec<usize> = Vec::new();
    let mut spans: Vec<(f64, f64)> = Vec::new();
    for (i, ((_, short), x)) in heads.iter().zip(centers).enumerate() {
        let r = place(short, *x);
        if spans.last().is_none_or(|p| r.0 >= p.1 + gap) {
            chosen.push(i);
            spans.push(r);
        }
    }
    let last = heads.len().saturating_sub(1);
    if !heads.is_empty()
        && chosen.last() != Some(&last)
        && let (Some((_, short)), Some(x)) = (heads.get(last), centers.get(last))
    {
        let r = place(short, *x);
        while spans.last().is_some_and(|p| p.1 + gap > r.0) {
            spans.pop();
            chosen.pop();
        }
        chosen.push(last);
        spans.push(r);
    }
    let mut year_spans: Vec<(f64, f64)> = Vec::new();
    for (n, i) in chosen.iter().enumerate() {
        let (Some((year, short)), Some(x)) = (heads.get(*i), centers.get(*i)) else {
            continue;
        };
        let r = place(short, *x);
        c.labels.push(ChartLabel {
            text: short.clone(),
            x: g.x(r.0),
            y: g.y(plot.bottom + 11.0),
            align: Align::Left,
        });
        let year = year.clone().or_else(|| {
            (n == 0)
                .then(|| labels.get(*i).and_then(|l| l.get(..4)).map(str::to_owned))
                .flatten()
                .filter(|y| !short.contains(y.as_str()))
        });
        if let Some(y) = year {
            let yr = place(&y, *x);
            if year_spans.last().is_none_or(|p| yr.0 >= p.1 + gap) {
                c.labels.push(ChartLabel {
                    text: y,
                    x: g.x(yr.0),
                    y: g.y(plot.bottom + 27.0),
                    align: Align::Left,
                });
                year_spans.push(yr);
            }
        }
    }
}

/// Horizontal bars, largest first (the caller passes them in order).
pub(crate) fn hbars(
    ui: &mut Ui,
    width: i32,
    items: &[(String, Decimal)],
    desc: &str,
    opens: &Opens,
) -> Option<ChartHit> {
    hbars_sized(ui, width, Fit::Drawing(0), items, desc, opens)
}

/// [`hbars`] fitted to a height: a dashboard block shares its height out between
/// the bars (a caption line each at least), the reports screen gives each bar a
/// standard slot.
pub(crate) fn hbars_sized(
    ui: &mut Ui,
    width: i32,
    fit: Fit,
    items: &[(String, Decimal)],
    desc: &str,
    opens: &Opens,
) -> Option<ChartHit> {
    if items.is_empty() {
        return None;
    }
    let n = items.len() as f64;
    let slot = match fit {
        Fit::Total(t) => ((f64::from(t) - 8.0) / n).clamp(MIN_BAR_SLOT, MAX_BAR_SLOT),
        Fit::Drawing(_) => BAR_SLOT,
    };
    let bar_h = (slot - 6.0).max(10.0);
    let h = match fit {
        Fit::Total(t) => f64::from(t).max(slot * n + 8.0),
        Fit::Drawing(_) => slot * n + 8.0,
    };
    let w = f64::from(width.max(240));
    let g = Geo { w, h };
    let label_w = (w * 0.3).clamp(90.0, 200.0);
    let max = items
        .iter()
        .map(|(_, v)| to_f(*v))
        .fold(0.0f64, f64::max)
        .max(1.0);
    let mut c = Chart {
        height: h as i32,
        ..Chart::default()
    };
    let (mut main, mut other) = (String::new(), String::new());
    for (i, (label, v)) in items.iter().enumerate() {
        let top = 4.0 + i as f64 * slot + (slot - bar_h) / 2.0;
        let x0 = label_w;
        let bw = ((w - label_w - 80.0).max(10.0) * (to_f(*v).max(0.0) / max)).max(1.0);
        let path = g.rect(x0, top, x0 + bw, top + bar_h);
        // The whole line is the bar's hit area (its label too).
        let slot_top = 4.0 + i as f64 * slot;
        c.pieces.push(ChartPiece {
            hit: hit_box(
                &g,
                0.0,
                slot_top,
                (x0 + bw + 6.0).max(x0 + MIN_HIT),
                slot_top + slot,
            ),
            mark: box_of(&g, x0, top, x0 + bw, top + bar_h),
            ring: false,
            name: if opens.period.is_empty() {
                format!("{label} — {}", amount(*v))
            } else {
                format!("{label} — {}, {}", amount(*v), opens.period)
            },
        });
        if label == "Other" {
            other.push_str(&path);
        } else {
            main.push_str(&path);
        }
        let shown = if text_w(label) > label_w - 10.0 {
            let keep = ((label_w - 10.0) / CHAR_W).max(4.0) as usize;
            format!(
                "{}…",
                label
                    .chars()
                    .take(keep.saturating_sub(1))
                    .collect::<String>()
            )
        } else {
            label.clone()
        };
        c.labels.push(ChartLabel {
            text: shown,
            x: g.x(x0 - 6.0),
            y: g.y(top + bar_h / 2.0),
            align: Align::Right,
        });
        c.labels.push(ChartLabel {
            text: whole(to_f(*v)),
            x: g.x(x0 + bw + 6.0),
            y: g.y(top + bar_h / 2.0),
            align: Align::Left,
        });
    }
    for (cmd, series) in [(main, ACCENT), (other, MUTED)] {
        if !cmd.is_empty() {
            c.paths.push(ChartPath {
                commands: cmd,
                fill: Some(0),
                series,
                ..ChartPath::default()
            });
        }
    }
    show(ui, desc, c, opens)
}

/// Paired bars per period (first series in the accent colour, second in grey) and
/// a line for the third series.
pub(crate) fn paired(
    ui: &mut Ui,
    width: i32,
    labels: &[String],
    series: [(&str, &[Decimal]); 3],
    desc: &str,
    opens: &Opens,
) -> Option<ChartHit> {
    paired_sized(ui, width, Fit::Drawing(HEIGHT), labels, series, desc, opens)
}

/// [`paired`] fitted to a height (a dashboard block gives its whole height; the
/// drawing takes what the legend leaves).
pub(crate) fn paired_sized(
    ui: &mut Ui,
    width: i32,
    fit: Fit,
    labels: &[String],
    series: [(&str, &[Decimal]); 3],
    desc: &str,
    opens: &Opens,
) -> Option<ChartHit> {
    if labels.is_empty() {
        return None;
    }
    let legend = wrap_legend(
        &legend_of(&[
            (series[0].0, ACCENT),
            (series[1].0, MUTED),
            (series[2].0, TEXT),
        ]),
        width,
    );
    let (w, h) = (
        f64::from(width.max(240)),
        fit.drawing(legend_height(&legend), min_axis_drawing()),
    );
    let g = Geo { w, h };
    let plot = Plot::new(w, h);
    let all: Vec<f64> = series
        .iter()
        .flat_map(|(_, v)| v.iter().map(|d| to_f(*d)))
        .collect();
    let axis = Axis::new(all.into_iter(), plot);
    let mut c = Chart {
        height: h as i32,
        ..Chart::default()
    };
    axis.draw(&g, &mut c);
    let n = labels.len() as f64;
    let slot = plot.width() / n;
    let bar_w = (slot * 0.32).min(28.0);
    let mut centers = Vec::new();
    let mut bars = [String::new(), String::new()];
    let mut line = String::new();
    let mut dots = String::new();
    for i in 0..labels.len() {
        let cx = plot.left + slot * (i as f64 + 0.5);
        centers.push(cx);
        for (k, (name, values)) in series.iter().take(2).enumerate() {
            let d = values.get(i).copied().unwrap_or_default();
            let v = to_f(d);
            let x = cx + if k == 0 { -bar_w - 1.0 } else { 1.0 };
            let (y0, y1) = (axis.y(0.0), axis.y(v));
            if let Some(b) = bars.get_mut(k) {
                b.push_str(&g.rect(x, y0.min(y1), x + bar_w, y0.max(y1)));
            }
            // Each bar's half of its period's slot, from the bar's end to the axis.
            let (hx0, hx1) = if k == 0 {
                (cx - slot / 2.0, cx)
            } else {
                (cx, cx + slot / 2.0)
            };
            let label = labels.get(i).map(String::as_str).unwrap_or_default();
            c.pieces.push(ChartPiece {
                hit: hit_box(&g, hx0, y0.min(y1) - 4.0, hx1, y0.max(y1) + 4.0),
                mark: box_of(&g, x, y0, x + bar_w, y1),
                ring: false,
                name: format!("{name} — {}, {}", amount(d), period_name(label)),
            });
        }
        let v = series[2].1.get(i).map_or(0.0, |d| to_f(*d));
        let y = axis.y(v);
        let _ = write!(line, "{} {} ", if i == 0 { "M" } else { "L" }, g.pt(cx, y));
        dots.push_str(&g.dot(cx, y, 3.0));
    }
    for (k, cmd) in bars.into_iter().enumerate() {
        c.paths.push(ChartPath {
            commands: cmd,
            fill: Some(0),
            series: if k == 0 { ACCENT } else { MUTED },
            label: series.get(k).map(|x| x.0.to_owned()).unwrap_or_default(),
            ..ChartPath::default()
        });
    }
    c.paths.push(ChartPath {
        commands: line,
        stroke: Some(0),
        stroke_width: 2,
        series: TEXT,
        label: series[2].0.to_owned(),
        ..ChartPath::default()
    });
    c.paths.push(ChartPath {
        commands: dots,
        fill: Some(0),
        series: TEXT,
        ..ChartPath::default()
    });
    x_labels(&g, plot, labels, &centers, &mut c);
    c.legend = legend;
    show(ui, desc, c, opens)
}

/// Lines per period end; the first series strong in the accent colour, the others
/// lighter.
pub(crate) fn lines(
    ui: &mut Ui,
    width: i32,
    labels: &[String],
    series: &[(&str, Vec<Decimal>)],
    desc: &str,
    opens: &Opens,
) -> Option<ChartHit> {
    lines_sized(ui, width, Fit::Drawing(HEIGHT), labels, series, desc, opens)
}

/// [`lines`] fitted to a height (a dashboard block gives its whole height; the
/// drawing takes what the legend leaves).
pub(crate) fn lines_sized(
    ui: &mut Ui,
    width: i32,
    fit: Fit,
    labels: &[String],
    series: &[(&str, Vec<Decimal>)],
    desc: &str,
    opens: &Opens,
) -> Option<ChartHit> {
    if labels.is_empty() {
        return None;
    }
    let legend = if series.len() > 1 {
        let names: Vec<(&str, i32)> = series
            .iter()
            .enumerate()
            .map(|(k, (n, _))| (*n, if k == 0 { ACCENT } else { MUTED }))
            .collect();
        wrap_legend(&legend_of(&names), width)
    } else {
        Vec::new()
    };
    let (w, h) = (
        f64::from(width.max(240)),
        fit.drawing(legend_height(&legend), min_axis_drawing()),
    );
    let g = Geo { w, h };
    let plot = Plot::new(w, h);
    let all: Vec<f64> = series
        .iter()
        .flat_map(|(_, v)| v.iter().map(|d| to_f(*d)))
        .collect();
    let axis = Axis::new(all.into_iter(), plot);
    let mut c = Chart {
        height: h as i32,
        ..Chart::default()
    };
    axis.draw(&g, &mut c);
    let n = labels.len().max(1) as f64;
    let x = |i: usize| {
        if labels.len() <= 1 {
            (plot.left + plot.right) / 2.0
        } else {
            plot.left + plot.width() * i as f64 / (n - 1.0)
        }
    };
    for (k, (name, values)) in series.iter().enumerate().rev() {
        let mut line = String::new();
        let mut dots = String::new();
        for (i, d) in values.iter().enumerate() {
            let (px, py) = (x(i), axis.y(to_f(*d)));
            let _ = write!(line, "{} {} ", if i == 0 { "M" } else { "L" }, g.pt(px, py));
            dots.push_str(&g.dot(px, py, if k == 0 { 3.0 } else { 2.0 }));
        }
        let s = if k == 0 { ACCENT } else { MUTED };
        if values.len() > 1 {
            c.paths.push(ChartPath {
                commands: line,
                stroke: Some(0),
                stroke_width: if k == 0 { 3 } else { 2 },
                series: s,
                label: (*name).to_owned(),
                ..ChartPath::default()
            });
        }
        c.paths.push(ChartPath {
            commands: dots,
            fill: Some(0),
            series: s,
            ..ChartPath::default()
        });
    }
    // A point per period: a band the plot's height around it (so every series'
    // point there is hit), the first series' point ringed.
    let half = if labels.len() > 1 {
        plot.width() / (n - 1.0) / 2.0
    } else {
        plot.width() / 2.0
    };
    for (i, label) in labels.iter().enumerate() {
        let px = x(i);
        let first = series
            .first()
            .and_then(|s| s.1.get(i))
            .copied()
            .unwrap_or_default();
        let py = axis.y(to_f(first));
        let values: Vec<String> = series
            .iter()
            .filter_map(|(n, v)| v.get(i).map(|d| format!("{n} — {}", amount(*d))))
            .collect();
        c.pieces.push(ChartPiece {
            hit: hit_box(&g, px - half, plot.top, px + half, plot.bottom),
            mark: box_of(&g, px - 7.0, py - 7.0, px + 7.0, py + 7.0),
            ring: true,
            name: format!("{}, {}", values.join("; "), period_name(label)),
        });
    }
    let centers: Vec<f64> = (0..labels.len()).map(x).collect();
    x_labels(&g, plot, labels, &centers, &mut c);
    c.legend = legend;
    show(ui, desc, c, opens)
}

/// A small line without axes (a block's twelve-month trend).
pub(crate) fn sparkline(ui: &mut Ui, width: i32, height: i32, values: &[Decimal], desc: &str) {
    if values.len() < 2 {
        return;
    }
    let (w, h) = (f64::from(width.max(60)), f64::from(height.max(16)));
    let g = Geo { w, h };
    let lo = values.iter().map(|v| to_f(*v)).fold(f64::MAX, f64::min);
    let hi = values.iter().map(|v| to_f(*v)).fold(f64::MIN, f64::max);
    let span = (hi - lo).max(1.0);
    let n = (values.len() - 1) as f64;
    let mut line = String::new();
    for (i, v) in values.iter().enumerate() {
        let x = 2.0 + (w - 4.0) * i as f64 / n;
        let y = h - 2.0 - (to_f(*v) - lo) / span * (h - 4.0);
        let _ = write!(line, "{} {} ", if i == 0 { "M" } else { "L" }, g.pt(x, y));
    }
    let c = Chart {
        height: h as i32,
        paths: vec![ChartPath {
            commands: line,
            stroke: Some(0),
            stroke_width: 2,
            series: ACCENT,
            ..ChartPath::default()
        }],
        ..Chart::default()
    };
    ui.next().width(width);
    ui.chart(desc, c);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn entries(names: &[&str]) -> Vec<LegendEntry> {
        names
            .iter()
            .map(|n| LegendEntry {
                text: (*n).to_owned(),
                ..LegendEntry::default()
            })
            .collect()
    }

    /// The width an entry line takes, as `wrap_legend` estimates it.
    fn line_width(line: &[&LegendEntry]) -> f64 {
        line.iter().map(|e| SWATCH + text_w(&e.text)).sum::<f64>()
            + ENTRY_GAP * line.len().saturating_sub(1) as f64
    }

    #[test]
    fn legend_wraps_to_the_width_in_order_and_never_splits_an_entry() {
        let names = [
            "Groceries",
            "Utilities",
            "Insurance",
            "Dining out",
            "Clothing",
            "Gifts",
        ];
        let wide = wrap_legend(&entries(&names), 900);
        assert!(wide.iter().all(|e| e.row == 0));
        assert_eq!(legend_height(&wide), CHART_GAP + LEGEND_LINE);
        let narrow = wrap_legend(&entries(&names), 220);
        let lines = narrow.iter().map(|e| e.row).max().unwrap() + 1;
        assert!(lines >= 3, "{narrow:?}");
        // Reading order kept, rows only grow.
        assert!(narrow.windows(2).all(|w| w[0].row <= w[1].row));
        assert_eq!(
            narrow.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
            names
        );
        for r in 0..lines {
            let line: Vec<&LegendEntry> = narrow.iter().filter(|e| e.row == r).collect();
            assert!(!line.is_empty());
            assert!(line_width(&line) <= 220.0, "{line:?}");
        }
        assert_eq!(
            legend_height(&narrow),
            CHART_GAP + lines * LEGEND_LINE + (lines - 1) * LEGEND_ROW_GAP
        );
    }

    #[test]
    fn an_entry_wider_than_the_line_is_shortened_with_its_full_text_as_tip() {
        let long = "Expenses:Household:Repairs and maintenance";
        let out = wrap_legend(&entries(&["Net", long]), 160);
        assert_eq!(out[0].tip, "");
        assert!(out[1].text.ends_with('…'), "{:?}", out[1]);
        assert_eq!(out[1].tip, long);
        assert!(SWATCH + text_w(&out[1].text) <= 160.0);
        assert_eq!(out[1].row, 1);
    }

    #[test]
    fn the_drawing_takes_what_the_legend_leaves() {
        let legend = wrap_legend(&entries(&["Income", "Expenses", "Net"]), 400);
        let l = legend_height(&legend);
        assert_eq!(Fit::Total(250).drawing(l, 10.0), f64::from(250 - l));
        assert_eq!(Fit::Drawing(190).drawing(l, 10.0), 190.0);
        // Never below the smallest readable drawing.
        assert_eq!(Fit::Total(40).drawing(l, 78.0), 78.0);
        // A taller total gives a taller drawing.
        assert!(Fit::Total(354).drawing(l, 78.0) > Fit::Total(250).drawing(l, 78.0));
        // The minimum height leaves the plot about two text lines.
        let min = min_height(0, &["Income", "Expenses", "Net"], 400);
        assert!(f64::from(min - l) - TOP - BOTTOM >= MIN_PLOT);
        assert!(min_height(0, &["Income", "Expenses", "Net"], 120) > min);
        assert!(min_height(11, &[], 400) >= 11 * 16);
    }
}
