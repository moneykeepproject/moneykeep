//! Tables with one column per period (Income & expenses, Net worth). Category on the
//! left and Total on the right stay put; when the periods do not fit, only the
//! period columns scroll sideways. Amount cells are sized to their content, never
//! truncated; zero amounts show as a dimmed "–".

use super::ui::{Align, Column, Tone, Ui};
use crate::fmt;

pub(crate) use crate::base::period::{Kind, Row, headers};

/// Estimated width of `text` at the body size in the number font, with padding.
pub(crate) fn text_width(text: &str) -> i32 {
    let n = i32::try_from(text.chars().count()).unwrap_or(40);
    n.saturating_mul(9).saturating_add(16)
}

fn is_zero(s: &str) -> bool {
    s.parse::<mk_core::Decimal>().is_ok_and(|d| d.is_zero())
}

fn cell(ui: &mut Ui, amount: &str, strong: bool) {
    if is_zero(amount) {
        ui.tone(Tone::Secondary).tip(fmt::group(amount));
        ui.label("–");
        return;
    }
    ui.mono().tip(fmt::group(amount));
    if strong {
        ui.bold();
    }
    ui.label(fmt::group(amount));
}

/// Draw the table; returns the index of a clicked account row.
pub(crate) fn show(
    ui: &mut Ui,
    id: &str,
    first_title: &str,
    labels: &[String],
    rows: &[Row],
    total_title: Option<&str>,
) -> Option<usize> {
    show_with_year(ui, id, first_title, labels, rows, total_title, None)
}

/// [`show`] with a year shown above the fixed right column's title (net worth's
/// single-date column, revision 22).
#[allow(clippy::too_many_arguments)]
pub(crate) fn show_with_year(
    ui: &mut Ui,
    id: &str,
    first_title: &str,
    labels: &[String],
    rows: &[Row],
    total_title: Option<&str>,
    total_year: Option<&str>,
) -> Option<usize> {
    let heads = headers(labels);
    // The name column: wide enough for the longest name on one line (body text,
    // about 7.5 px a character, with the indent), at most 280 px.
    let name_w = rows
        .iter()
        .map(|r| {
            let n = i32::try_from(r.name.chars().count()).unwrap_or(40);
            let indent = i32::try_from(r.indent).unwrap_or(0).saturating_mul(14);
            n.saturating_mul(15) / 2 + 12 + indent
        })
        .fold(0, i32::max)
        .min(280);
    let mut cols = vec![Column {
        title: first_title.to_owned(),
        width: name_w,
        ..Column::default()
    }];
    for (k, (year, short)) in heads.iter().enumerate() {
        let w = rows
            .iter()
            .filter_map(|r| r.cells.get(k))
            .map(|c| text_width(&fmt::group(c)))
            .fold(text_width(short).max(56), i32::max);
        cols.push(Column {
            title: short.clone(),
            over: year.clone().unwrap_or_default(),
            width: w,
            align: Align::Right,
            ..Column::default()
        });
    }
    if let Some(t) = total_title {
        let w = rows
            .iter()
            .filter_map(|r| r.total.as_deref())
            .map(|c| text_width(&fmt::group(c)))
            .fold(text_width(t).max(72), i32::max);
        cols.push(Column {
            title: t.to_owned(),
            over: total_year.unwrap_or_default().to_owned(),
            width: w,
            align: Align::Right,
            ..Column::default()
        });
    }
    let mut clicked = None;
    ui.scope(id, |ui| {
        ui.period_table("", cols, total_title.is_some(), |ui| {
            for (i, row) in rows.iter().enumerate() {
                ui.row(|ui| {
                    match row.kind {
                        Kind::Account => {
                            ui.named(format!("Show transactions for {}", row.name))
                                .tip(row.name.clone());
                            if ui.link(row.name.clone()).clicked() {
                                clicked = Some(i);
                            }
                        }
                        Kind::Heading | Kind::Total => {
                            ui.bold().tip(row.name.clone());
                            ui.label(row.name.clone());
                        }
                        Kind::Plain => {
                            ui.tip(row.name.clone());
                            ui.label(row.name.clone());
                        }
                    }
                    let strong = row.kind == Kind::Total;
                    for k in 0..labels.len() {
                        match row.cells.get(k) {
                            Some(c) => cell(ui, c, strong),
                            None => ui.label(""),
                        }
                    }
                    if total_title.is_some() {
                        match &row.total {
                            Some(t) => cell(ui, t, strong),
                            None => ui.label(""),
                        }
                    }
                });
                ui.indent_last(i32::try_from(row.indent).unwrap_or(0));
            }
        });
    });
    clicked
}
