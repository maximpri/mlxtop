// SPDX-License-Identifier: MIT
//! The memory panel as headroom: how close wired memory is to the GPU limit,
//! and how much RAM is really left, free plus the file cache the OS gives
//! back on demand. A share of RAM "resident" reads as an alarm on a machine
//! whose cache is full and whose pressure is normal; what is left does not.
use crate::domain::{Sample, Tone};
use crate::formatting::bytes;
use crate::memory_composition::{draw_composition, gpu_limit, kind_colors, segments};
use crate::theme::{BLUE, CYAN, EDGE, MUTED, PANEL, RED, YELLOW};
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

/// Cells the history's axis labels take (`36.0 GiB` and a space).
pub(crate) const AXIS_GUTTER: u16 = 9;

/// The model's own GPU memory, when the runtime reports it and it is part
/// of what the GPU holds: the rest is the KV cache, working memory and
/// anything else on the GPU.
fn model_share(sample: &Sample, used: u64) -> Option<u64> {
    sample
        .llm_model_memory
        .filter(|model| *model > 0 && *model <= used)
}

/// `16.0` when `value` shares `unit` with the row, else the value in full.
fn in_unit(value: u64, unit: Option<&str>) -> String {
    let text = bytes(value);
    match (unit, text.rsplit_once(' ')) {
        (Some(unit), Some((number, own))) if own == unit => number.to_owned(),
        _ => text,
    }
}

/// The unit every value would print in, when they share one.
fn shared_unit(values: &[u64]) -> Option<String> {
    let units: Vec<String> = values
        .iter()
        .filter_map(|value| {
            bytes(*value)
                .rsplit_once(' ')
                .map(|(_, unit)| unit.to_owned())
        })
        .collect();
    units
        .first()
        .filter(|first| units.iter().all(|unit| unit == *first))
        .cloned()
}

/// Draw the panel body: what the GPU holds against its limit and what RAM
/// holds by kind, each with what is left, who holds the most, and RAM by
/// kind over time with the GPU limit marked. `columns` gives, for a plot of
/// that many columns, each column's RAM by kind. Returns false when the OS
/// reports no composition to draw.
pub(crate) fn draw(
    frame: &mut Frame,
    area: Rect,
    sample: &Sample,
    columns: impl Fn(usize) -> Vec<Option<([u64; 5], Tone)>>,
) -> bool {
    let Some(sizes) = segments(sample) else {
        return false;
    };
    if area.is_empty() {
        return true;
    }
    let limit = gpu_limit(sample);
    let meters = 1 + u16::from(limit.is_some());
    // Each meter gets a second row saying what fills it once there is room
    // for them and a row of history.
    let detailed = area.height >= meters * 2;
    let mut y = area.y;
    let bottom = area.bottom();
    let row = |y: u16| Rect::new(area.x, y, area.width, 1);
    let muted = |text: String| Span::styled(text, Style::default().fg(MUTED));

    if let Some(limit) = limit {
        // What the GPU holds: Metal's own count where the OS gives it, else
        // wired memory, which bounds it (GPU memory is wired RAM here).
        let used = sample.gpu_in_use.unwrap_or(sample.wired);
        let color = gpu_color(used, limit);
        let model = model_share(sample, used);
        let bar = meter(frame, row(y), "GPU", &of_total(used, limit), color);
        // The model's weights, then everything else the GPU holds, then
        // what is left before the limit.
        let cells = |value: u64| {
            (u128::from(value.min(limit)) * u128::from(bar.width) / u128::from(limit)) as u16
        };
        let filled = cells(used);
        let weights = model.map_or(filled, cells);
        let near_full = color != CYAN;
        for x in 0..bar.width {
            let fill = if x >= filled {
                EDGE
            } else if near_full {
                color
            } else if x < weights {
                CYAN
            } else {
                BLUE
            };
            frame.buffer_mut()[(bar.x + x, bar.y)]
                .set_symbol(" ")
                .set_bg(fill);
        }
        y += 1;
        if detailed && y < bottom {
            let room = if used > limit {
                "over the limit".to_owned()
            } else {
                format!("{} left", bytes(limit - used))
            };
            let parts = match model {
                Some(model) => {
                    let unit = shared_unit(&[model, used - model]);
                    let unit_ref = unit.as_deref();
                    let suffix = unit
                        .as_ref()
                        .map(|unit| format!(" {unit}"))
                        .unwrap_or_default();
                    vec![
                        vec![
                            Span::raw(" ".repeat(usize::from(PREFIX))),
                            Span::styled("█", Style::default().fg(CYAN)),
                            muted(format!("model {} ", in_unit(model, unit_ref))),
                            Span::styled("█", Style::default().fg(BLUE)),
                            muted(format!(
                                "KV cache & other {}{suffix}",
                                in_unit(used - model, unit_ref)
                            )),
                        ],
                        vec![
                            Span::styled("█", Style::default().fg(CYAN)),
                            muted(format!("model {} ", in_unit(model, unit_ref))),
                            Span::styled("█", Style::default().fg(BLUE)),
                            muted(format!("other {}{suffix}", in_unit(used - model, unit_ref))),
                        ],
                    ]
                }
                // Without the model's own figure, the meter says it all.
                None => vec![vec![]],
            };
            text(
                frame,
                row(y),
                parts,
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
        // Each kind with its size: the legend is the breakdown.
        let kinds = ["wired", "app", "comp", "cache"];
        let unit = shared_unit(&sizes[..4]);
        let suffix = unit
            .as_ref()
            .map(|unit| format!(" {unit}"))
            .unwrap_or_default();
        let legend = |indent: usize, sized: bool| {
            let mut legend = vec![Span::raw(" ".repeat(indent))];
            for (index, ((name, color), size)) in
                kinds.into_iter().zip(kind_colors()).zip(sizes).enumerate()
            {
                let last = index + 1 == kinds.len();
                legend.push(Span::styled("█", Style::default().fg(color)));
                legend.push(muted(if sized {
                    format!(
                        "{name} {}{}",
                        in_unit(size, unit.as_deref()),
                        if last { suffix.as_str() } else { " " }
                    )
                } else {
                    format!("{name}{}", if last { "" } else { " " })
                }));
            }
            legend
        };
        text(
            frame,
            row(y),
            vec![
                legend(usize::from(PREFIX), true),
                legend(0, true),
                legend(usize::from(PREFIX), false),
                legend(0, false),
            ],
            Span::styled(
                format!("{} left", bytes(left(sizes))),
                Style::default().fg(tone),
            ),
        );
        y += 1;
    }
    // Who holds it: the runtime's own footprint and the largest other app.
    if detailed && y + 3 < bottom {
        let runtime = sample
            .process_memory
            .as_ref()
            .map(|reading| reading.footprint)
            .filter(|footprint| *footprint > 0)
            .or((sample.llm_rss > 0).then_some(sample.llm_rss));
        let mut parts = Vec::new();
        if let Some(runtime) = runtime.filter(|_| sample.llm_provider != "none") {
            parts.push(format!("{} {}", sample.llm_provider, bytes(runtime)));
        }
        if let Some((name, rss)) = &sample.largest_other {
            parts.push(format!(
                "{} {}",
                crate::formatting::compact_label(name, 22),
                bytes(*rss)
            ));
        }
        if !parts.is_empty() {
            let line = format!("{:<5}by app: {}", "", parts.join(" · "));
            frame.render_widget(
                Paragraph::new(crate::formatting::compact_label(
                    &line,
                    usize::from(area.width),
                ))
                .style(Style::default().fg(MUTED).bg(PANEL)),
                row(y),
            );
            y += 1;
        }
    }
    // RAM by kind over time, against all of RAM, with the GPU limit marked:
    // a model load, a growing KV cache or another app shows as its band.
    let rows = bottom.saturating_sub(y);
    if rows < 2 || area.width <= AXIS_GUTTER + 2 {
        return true;
    }
    let plot = Rect::new(area.x + AXIS_GUTTER, y, area.width - AXIS_GUTTER, rows);
    let points = columns(usize::from(plot.width));
    if points.iter().all(Option::is_none) {
        return true;
    }
    let total = sample.total_memory.max(1);
    let gutter = |y: u16| Rect::new(area.x, y, AXIS_GUTTER, 1);
    // The GPU limit's row, from the bottom, when it falls inside the plot.
    let limit_level = limit
        .filter(|limit| *limit < total)
        .map(|limit| (u128::from(limit) * u128::from(rows) / u128::from(total)) as u16)
        .filter(|level| *level < rows);
    // The top is all of RAM, unless the named limit line sits there.
    if limit_level != Some(rows - 1) {
        frame_label(frame, gutter(plot.y), bytes(total));
    }
    frame_label(frame, gutter(plot.bottom() - 1), "0".into());
    // Rows from the bottom, in eighths of a row, for a byte count.
    let height = |value: u64| {
        (u128::from(value.min(total)) * u128::from(rows) * 8 / u128::from(total)) as u16
    };
    let colors = kind_colors();
    for (column, point) in points.iter().enumerate() {
        let Some((kinds, tone)) = point else {
            continue;
        };
        // Wired, app and compressed stacked; cache and free stay empty.
        let mut tops = [0_u16; 3];
        let mut sum = 0;
        for (kind, size) in kinds.iter().take(3).enumerate() {
            sum += size;
            tops[kind] = height(sum);
        }
        let mut top_level = None;
        for level in 0..rows {
            let middle = level * 8 + 4;
            let Some(kind) = tops.iter().position(|top| middle < *top) else {
                break;
            };
            frame.buffer_mut()[(plot.x + column as u16, plot.bottom() - 1 - level)]
                .set_symbol(" ")
                .set_bg(colors[kind]);
            top_level = Some(level);
        }
        // A sample marked as part of a slowdown: its column's top in the
        // anomaly colour, as on every other chart.
        if *tone == Tone::Unusual {
            let level = top_level.unwrap_or(0);
            frame.buffer_mut()[(plot.x + column as u16, plot.bottom() - 1 - level)]
                .set_symbol(" ")
                .set_bg(crate::theme::UNUSUAL);
        }
    }
    // The GPU limit as a line across the plot, where it falls inside it.
    if let (Some(limit), Some(level)) = (limit, limit_level) {
        {
            let y = plot.bottom() - 1 - level;
            for x in plot.x..plot.right() {
                let cell = &mut frame.buffer_mut()[(x, y)];
                // Across a band the dash is dark; above it, muted.
                let on_band = colors[..3].contains(&cell.bg);
                cell.set_symbol("┄")
                    .set_fg(if on_band { PANEL } else { MUTED });
            }
            // Named on the line itself, so it never reads as an axis value.
            let name = format!(" GPU limit {} ", bytes(limit));
            let width = Line::from(name.as_str()).width() as u16;
            if width + 2 < plot.width {
                frame.render_widget(
                    Paragraph::new(name).style(Style::default().fg(MUTED).bg(PANEL)),
                    Rect::new(plot.right() - width - 1, y, width, 1),
                );
            }
        }
    }
    true
}

fn frame_label(frame: &mut Frame, area: Rect, value: String) {
    frame.render_widget(
        Paragraph::new(format!("{value:>8} ")).style(Style::default().fg(MUTED)),
        area,
    );
}
