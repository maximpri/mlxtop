// SPDX-License-Identifier: MIT
//! The memory panel as headroom: how close wired memory is to the GPU limit,
//! and how much RAM is really left, free plus the file cache the OS gives
//! back on demand. A share of RAM "resident" reads as an alarm on a machine
//! whose cache is full and whose pressure is normal; what is left does not.
use crate::domain::{Sample, Tone};
use crate::formatting::bytes;
use crate::memory_composition::{draw_composition, gpu_limit, kind_colors, segments};
use crate::theme::{CYAN, EDGE, MUTED, PANEL, RED, YELLOW};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

const PREFIX: u16 = 5;

/// `25.6 / 32.0 GiB` where both share a unit, else both in full.
pub(crate) fn of_total(used: u64, total: u64) -> String {
    let (used, total) = (bytes(used), bytes(total));
    match (used.rsplit_once(' '), total.rsplit_once(' ')) {
        (Some((value, unit)), Some((_, total_unit))) if unit == total_unit => {
            format!("{value} / {total}")
        }
        _ => format!("{used} / {total}"),
    }
}

/// What RAM is left for a model to grow into: free, plus file cache.
pub(crate) fn left(sizes: [u64; 5]) -> u64 {
    sizes[3].saturating_add(sizes[4])
}

/// The GPU meter's colour: it nears full as the header's does.
fn gpu_color(wired: u64, limit: u64) -> Color {
    let percent = wired as f64 * 100.0 / limit as f64;
    if percent >= 97.0 {
        RED
    } else if percent >= 90.0 {
        YELLOW
    } else {
        CYAN
    }
}

/// A row of what a meter measures, with what is left at its right. The
/// first wording on the left that fits is used; the reading always stays.
fn text(frame: &mut Frame, row: Rect, left: Vec<Vec<Span<'static>>>, right: Span<'static>) {
    let room = usize::from(row.width);
    let right_width = right.width();
    let mut line = left
        .into_iter()
        .map(Line::from)
        .find(|line| line.width() + 1 + right_width <= room)
        .unwrap_or_default();
    let gap = room.saturating_sub(line.width() + right_width);
    line.spans.push(Span::raw(" ".repeat(gap)));
    line.spans.push(right);
    frame.render_widget(Paragraph::new(line).style(Style::default().bg(PANEL)), row);
}

/// A labelled meter row: prefix, bar, reading. Returns the bar's area.
fn meter(frame: &mut Frame, row: Rect, prefix: &str, reading: &str, color: Color) -> Rect {
    let reading_width = Line::from(reading).width() as u16 + 2;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{prefix:<5}"), Style::default().fg(MUTED)),
            Span::raw(" ".repeat(usize::from(
                row.width.saturating_sub(PREFIX + reading_width),
            ))),
            Span::styled(format!("  {reading}"), Style::default().fg(color)),
        ]))
        .style(Style::default().bg(PANEL)),
        row,
    );
    Rect::new(
        row.x + PREFIX,
        row.y,
        row.width.saturating_sub(PREFIX + reading_width),
        1,
    )
}

/// Draw the panel body. `columns` gives, for a plot of that many columns,
/// what was left in each with the pressure it was under. Returns false when
/// the OS reports no composition to draw.
/// Cells the history's axis labels take (`5.0 GiB ` and a space).
pub(crate) const AXIS_GUTTER: u16 = 9;

pub(crate) fn draw(
    frame: &mut Frame,
    area: Rect,
    sample: &Sample,
    columns: impl Fn(usize) -> Vec<Option<(u64, Tone)>>,
) -> bool {
    let Some(sizes) = segments(sample) else {
        return false;
    };
    if area.is_empty() {
        return true;
    }
    let limit = gpu_limit(sample);
    let meters = 1 + u16::from(limit.is_some());
    // Each meter gets a second row for what it measures and what is left
    // once there is room for them and a row of history.
    let detailed = area.height >= meters * 2;
    let mut y = area.y;
    let bottom = area.bottom();
    let row = |y: u16| Rect::new(area.x, y, area.width, 1);
    if let Some(limit) = limit {
        // What the GPU holds: Metal's own count where the OS gives it (the
        // header reads the same), else wired memory, which bounds it.
        let (used, what) = match sample.gpu_in_use {
            Some(in_use) => (in_use, "GPU memory in use"),
            None => (sample.wired, "wired"),
        };
        let color = gpu_color(used, limit);
        let bar = meter(frame, row(y), "GPU", &of_total(used, limit), color);
        let filled =
            (u128::from(used.min(limit)) * u128::from(bar.width) / u128::from(limit)) as u16;
        for x in 0..bar.width {
            frame.buffer_mut()[(bar.x + x, bar.y)]
                .set_symbol(" ")
                .set_bg(if x < filled { color } else { EDGE });
        }
        y += 1;
        if detailed && y < bottom {
            let (note, room) = if used > limit {
                (format!("{what} over the GPU limit"), "0 B left".to_owned())
            } else {
                (
                    format!("{what}, against the GPU limit"),
                    format!("{} left", bytes(limit - used)),
                )
            };
            let muted = |text: String| vec![Span::styled(text, Style::default().fg(MUTED))];
            text(
                frame,
                row(y),
                vec![muted(format!("{:<5}{note}", "")), muted(note.clone())],
                Span::styled(room, Style::default().fg(color)),
            );
            y += 1;
        }
    }
    if y >= bottom {
        return true;
    }
    let used = sizes[0] + sizes[1] + sizes[2];
    let tone = sample.pressure_tone.color();
    let bar = meter(
        frame,
        row(y),
        "RAM",
        &of_total(used, sample.total_memory),
        tone,
    );
    draw_composition(frame, bar, sample, false);
    y += 1;
    if detailed && y < bottom {
        let legend = |indent: usize| {
            let mut legend = vec![Span::raw(" ".repeat(indent))];
            let kinds = ["wired", "app", "comp", "cache"];
            for (index, (name, color)) in kinds.into_iter().zip(kind_colors()).enumerate() {
                legend.push(Span::styled("█", Style::default().fg(color)));
                legend.push(Span::styled(
                    format!("{name}{}", if index + 1 < kinds.len() { " " } else { "" }),
                    Style::default().fg(MUTED),
                ));
            }
            legend
        };
        text(
            frame,
            row(y),
            vec![legend(usize::from(PREFIX)), legend(0)],
            Span::styled(
                format!("{} left", bytes(left(sizes))),
                Style::default().fg(tone),
            ),
        );
        y += 1;
    }
    // A tall panel (the enlarged view) has room for each kind's size too.
    if area.height >= 14 && y < bottom {
        // One unit for the row when every size shares it; kinds that do not
        // fit are dropped whole, never cut.
        let shown: Vec<(&str, String)> = ["wired", "app", "comp", "cache", "free"]
            .into_iter()
            .zip(sizes)
            .map(|(name, size)| (name, bytes(size)))
            .collect();
        let unit = shown[0].1.rsplit_once(' ').map(|(_, unit)| unit.to_owned());
        let shared = unit
            .as_deref()
            .filter(|unit| shown.iter().all(|(_, size)| size.ends_with(unit)));
        let room = usize::from(area.width.saturating_sub(PREFIX));
        let line = (1..=shown.len())
            .rev()
            .map(|count| {
                let parts: Vec<String> = shown[..count]
                    .iter()
                    .map(|(name, size)| match shared {
                        Some(unit) => format!("{name} {}", size.trim_end_matches(unit).trim_end()),
                        None => format!("{name} {size}"),
                    })
                    .collect();
                let mut text = parts.join(" · ");
                if let Some(unit) = shared {
                    text.push_str(&format!(" {unit}"));
                }
                text
            })
            .find(|text| Line::from(text.as_str()).width() <= room)
            .unwrap_or_default();
        frame.render_widget(
            Paragraph::new(format!("{:<5}{line}", "")).style(Style::default().fg(MUTED).bg(PANEL)),
            row(y),
        );
        y += 1;
    }
    // What was left over time, coloured by the pressure it was under.
    let rows = bottom.saturating_sub(y);
    if rows < 2 || area.width <= 10 {
        return true;
    }
    const GUTTER: u16 = AXIS_GUTTER;
    let plot = Rect::new(area.x + GUTTER, y, area.width - GUTTER, rows);
    let points = columns(usize::from(plot.width));
    if points.iter().all(Option::is_none) {
        return true;
    }
    // The axis rounds up past the peak, in tenths of a GiB, like every other
    // chart's: a steady reading stands below the top as a level, rather
    // than filling the plot into a block that reads as horizontal stripes.
    const TENTH: u64 = (1 << 30) / 10;
    let peak = points
        .iter()
        .flatten()
        .map(|(left, _)| *left)
        .max()
        .unwrap_or(0);
    let ceiling = crate::chart_scale::ceiling(peak.div_ceil(TENTH), 1) * TENTH;
    let label = |y: u16, value: String| {
        (
            Paragraph::new(format!("{value:>8} ")).style(Style::default().fg(MUTED)),
            Rect::new(area.x, y, GUTTER, 1),
        )
    };
    let (top, top_area) = label(plot.y, bytes(ceiling));
    frame.render_widget(top, top_area);
    let (zero, zero_area) = label(plot.bottom() - 1, "0".into());
    frame.render_widget(zero, zero_area);
    const BLOCKS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    for (column, point) in points.iter().enumerate() {
        let Some((value, tone)) = point else {
            continue;
        };
        let eighths = (u128::from(*value) * u128::from(rows) * 8 / u128::from(ceiling)) as u16;
        // A measured sample always shows, even with nothing left.
        let eighths = eighths.max(1);
        for level in 0..rows {
            let filled = eighths.saturating_sub(level * 8).min(8);
            if filled == 0 {
                break;
            }
            // A full cell is painted as background: terminals leave a gap
            // between rows of glyphs, which reads as horizontal stripes.
            frame.buffer_mut()[(plot.x + column as u16, plot.bottom() - 1 - level)]
                .set_symbol(BLOCKS[usize::from(filled)])
                .set_fg(tone.color())
                .set_bg(if filled == 8 { tone.color() } else { PANEL });
        }
    }
    true
}
