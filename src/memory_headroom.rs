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

/// The GPU meter's fill: the colour of wired RAM, which GPU memory is part
/// of here, so the two bars read as the same memory.
pub(crate) fn gpu_fill() -> Color {
    kind_colors()[0]
}

/// The part of GPU memory that is not the model's weights.
const GPU_OTHER: Color = Color::Rgb(170, 190, 255);

/// The history's band colours: wired, app and compressed, dimmed.
pub(crate) fn history_colors() -> [Color; 3] {
    let dim = |color: Color| match (color, PANEL) {
        (Color::Rgb(r, g, b), Color::Rgb(pr, pg, pb)) => {
            let mix = |c: u8, p: u8| ((u16::from(c) * 2 + u16::from(p) * 3) / 5) as u8;
            Color::Rgb(mix(r, pr), mix(g, pg), mix(b, pb))
        }
        _ => color,
    };
    let kinds = kind_colors();
    [dim(kinds[0]), dim(kinds[1]), dim(kinds[2])]
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
    columns: impl Fn(usize) -> Vec<Option<Column>>,
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
                gpu_fill()
            } else {
                GPU_OTHER
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
                            Span::styled("█", Style::default().fg(gpu_fill())),
                            muted(format!("model {} ", in_unit(model, unit_ref))),
                            Span::styled("█", Style::default().fg(GPU_OTHER)),
                            muted(format!(
                                "KV cache & other {}{suffix}",
                                in_unit(used - model, unit_ref)
                            )),
                        ],
                        vec![
                            Span::styled("█", Style::default().fg(gpu_fill())),
                            muted(format!("model {} ", in_unit(model, unit_ref))),
                            Span::styled("█", Style::default().fg(GPU_OTHER)),
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
        // Each kind with its size beside what is left, when one row holds
        // both; else what is left under the reading, as for the GPU, and the
        // kinds on up to two rows of their own. Names alone are the last
        // resort: the sizes are the point.
        let item = |index: usize, spaced: bool, with_unit: bool| {
            let last = index + 1 == kinds.len();
            vec![
                Span::styled("█", Style::default().fg(kind_colors()[index])),
                muted(format!(
                    "{} {}{}",
                    kinds[index],
                    in_unit(sizes[index], unit.as_deref()),
                    match (last, with_unit, spaced) {
                        (true, true, _) => suffix.as_str(),
                        (false, _, true) => " ",
                        _ => "",
                    }
                )),
            ]
        };
        let inline = |indent: usize, spaced: bool, with_unit: bool| {
            let mut spans = vec![Span::raw(" ".repeat(indent))];
            for index in 0..kinds.len() {
                spans.extend(item(index, spaced, with_unit));
            }
            // Room before what is left, so the last size does not run into it.
            spans.push(Span::raw(" "));
            spans
        };
        let names = |indent: usize| {
            let mut spans = vec![Span::raw(" ".repeat(indent))];
            for (index, name) in kinds.into_iter().enumerate() {
                spans.push(Span::styled("█", Style::default().fg(kind_colors()[index])));
                spans.push(muted(format!(
                    "{name}{}",
                    if index + 1 < kinds.len() { " " } else { "" }
                )));
            }
            spans
        };
        let prefix = usize::from(PREFIX);
        let room = usize::from(area.width);
        let left_text = format!("{} left", bytes(left(sizes)));
        let right = Span::styled(left_text.clone(), Style::default().fg(tone));
        let fits = |spans: &[Span]| {
            Line::from(spans.to_vec()).width() + 1 + left_text.chars().count() <= room
        };
        let one_row = [
            inline(prefix, true, true),
            inline(0, true, true),
            inline(0, true, false),
        ]
        .into_iter()
        .find(|spans| fits(spans));
        // The kinds on rows of their own, each a whole item, at most two.
        let wrapped = |indent: usize| {
            let mut lines: Vec<Vec<Span<'static>>> = vec![vec![Span::raw(" ".repeat(indent))]];
            for index in 0..kinds.len() {
                let spans = item(index, true, true);
                let width: usize = spans.iter().map(Span::width).sum();
                let used = Line::from(lines.last().cloned().unwrap_or_default()).width();
                // The space after an item may hang over the edge.
                if used + width > room + 1 && used > indent {
                    lines.push(vec![Span::raw(" ".repeat(indent))]);
                }
                if let Some(line) = lines.last_mut() {
                    line.extend(spans);
                }
            }
            lines
        };
        let own_rows = [wrapped(prefix), wrapped(0)].into_iter().find(|lines| {
            lines.len() <= 2
                && y + (lines.len() as u16) < bottom
                && lines
                    .iter()
                    .all(|line| Line::from(line.clone()).width() <= room + 1)
        });
        match (one_row, own_rows) {
            (Some(spans), _) => {
                text(frame, row(y), vec![spans], right);
                y += 1;
            }
            (None, Some(lines)) => {
                text(frame, row(y), vec![vec![]], right);
                y += 1;
                for line in lines {
                    frame.render_widget(
                        Paragraph::new(Line::from(line)).style(Style::default().bg(PANEL)),
                        row(y),
                    );
                    y += 1;
                }
            }
            // No row to spare: the sizes run together, else names alone.
            (None, None) => {
                text(
                    frame,
                    row(y),
                    vec![inline(0, false, false), names(prefix), names(0)],
                    right,
                );
                y += 1;
            }
        }
    }
    // Who holds it: the runtime's own footprint and the largest other app.
    if detailed && y + 3 < bottom {
        let runtime = Some(sample.runtime_memory()).filter(|memory| *memory > 0);
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
    // The meters over time, one chart each: GPU memory against its limit
    // (the top of its chart), and RAM by kind against all of RAM. A model
    // load, a KV cache growing toward the limit or another app shows as its
    // level rising.
    let rows = bottom.saturating_sub(y);
    if rows < 2 || area.width <= AXIS_GUTTER + 2 {
        return true;
    }
    let width = area.width - AXIS_GUTTER;
    let points = columns(usize::from(width));
    if points.iter().all(Option::is_none) {
        return true;
    }
    let bands = history_colors();
    // Both charts where each can have two rows; else RAM alone.
    let gpu_rows = match limit {
        Some(_) if rows >= 4 => rows / 2,
        _ => 0,
    };
    if let Some(limit) = limit.filter(|_| gpu_rows > 0) {
        level_chart(
            frame,
            Rect::new(area.x, y, area.width, gpu_rows),
            "GPU",
            limit,
            &points,
            |column| column.gpu.map(|gpu| ([gpu, gpu, gpu], [bands[0]; 3])),
        );
        y += gpu_rows;
    }
    level_chart(
        frame,
        Rect::new(area.x, y, area.width, rows - gpu_rows),
        "RAM",
        sample.total_memory.max(1),
        &points,
        |column| {
            let [wired, app, compressed, ..] = column.sizes;
            Some(([wired, wired + app, wired + app + compressed], bands))
        },
    );
    true
}

/// One column of the panel's history: RAM by kind, GPU memory in use where
/// a limit is known, and the tone its sample was marked in.
#[derive(Clone, Copy)]
pub(crate) struct Column {
    pub(crate) sizes: [u64; 5],
    pub(crate) gpu: Option<u64>,
    pub(crate) tone: Tone,
}

/// A level over time against `top`: up to three stacked bands per column,
/// each given by where it ends, drawn dim with the stack's top to the
/// eighth of a row. The gutter names the chart and its top.
fn level_chart(
    frame: &mut Frame,
    area: Rect,
    name: &str,
    top: u64,
    points: &[Option<Column>],
    stack: impl Fn(&Column) -> Option<([u64; 3], [Color; 3])>,
) {
    const BLOCKS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let rows = area.height;
    let plot = Rect::new(area.x + AXIS_GUTTER, area.y, area.width - AXIS_GUTTER, rows);
    let gutter = |y: u16| Rect::new(area.x, y, AXIS_GUTTER, 1);
    // The chart's top, then whose it is: "32.0 GiB / GPU" read downward.
    frame_label(frame, gutter(plot.y), bytes(top));
    if rows >= 2 {
        frame_label(frame, gutter(plot.y + 1), name.to_owned());
    }
    let eighths =
        |value: u64| (u128::from(value.min(top)) * u128::from(rows) * 8 / u128::from(top)) as u16;
    for (column, point) in points.iter().enumerate() {
        let Some(point) = point else {
            continue;
        };
        let Some((ends, colors)) = stack(point) else {
            continue;
        };
        let tops = ends.map(eighths);
        // Any reading shows, however small against the top.
        let height = tops[2].max(u16::from(ends[2] > 0));
        let mut top_level = 0;
        for level in 0..rows {
            let base = level * 8;
            if base >= height {
                break;
            }
            // The band at the cell's middle, or the topmost where the stack
            // ends inside the cell.
            let middle = (base + 4).min(height - 1);
            let band = tops.iter().position(|end| middle < *end).unwrap_or(2);
            let filled = usize::from((height - base).min(8));
            let cell = &mut frame.buffer_mut()[(plot.x + column as u16, plot.bottom() - 1 - level)];
            if filled == 8 {
                cell.set_symbol(" ").set_bg(colors[band]);
            } else {
                cell.set_symbol(BLOCKS[filled])
                    .set_fg(colors[band])
                    .set_bg(PANEL);
            }
            top_level = level;
        }
        // A sample marked as part of a slowdown: its column's top in the
        // anomaly colour, as on every other chart.
        if point.tone == Tone::Unusual {
            frame.buffer_mut()[(plot.x + column as u16, plot.bottom() - 1 - top_level)]
                .set_symbol(" ")
                .set_bg(crate::theme::UNUSUAL);
        }
    }
}

fn frame_label(frame: &mut Frame, area: Rect, value: String) {
    frame.render_widget(
        Paragraph::new(format!("{value:>8} ")).style(Style::default().fg(MUTED)),
        area,
    );
}
