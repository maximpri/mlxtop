// SPDX-License-Identifier: MIT
//! The memory panel, laid out as the hardware is. Where the GPU has no
//! memory of its own (Apple Silicon, NVIDIA's integrated GPUs) there is one
//! pool: what the GPU uses is RAM, so it is drawn as a part of the one RAM
//! bar, never as a second meter. Where cards have their own memory there
//! are two pools and two meters, VRAM and RAM. What is left of RAM is free
//! memory plus the file cache the OS gives back on demand, or the room
//! under the GPU limit where that is less. A share of RAM "resident" reads as an alarm on a machine whose
//! cache is full and whose pressure is normal; what is left does not.
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

/// What one pool has left for a model to grow into, as the panel says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Left {
    /// Free RAM and the file cache the OS gives back on demand.
    Ram(u64),
    /// The room under the GPU limit, where that is less.
    ToLimit(u64),
    /// GPU memory is past its limit.
    OverLimit,
}

/// What is left on a machine whose GPU memory is `memory`: free RAM and
/// file cache, or on one pool the room under the GPU limit where that is
/// less. Against the limit counts Metal's own count of GPU memory, else
/// wired RAM, which bounds it. `None` when the OS reports no composition.
pub(crate) fn left_for(sample: &Sample, memory: crate::gpu::Memory) -> Option<Left> {
    segments(sample).map(|sizes| left_of(sample, sizes, memory))
}

/// `left_for` over RAM's composition, `sizes`.
fn left_of(sample: &Sample, sizes: [u64; 5], memory: crate::gpu::Memory) -> Left {
    let ram = left(sizes);
    let limit = gpu_limit(sample).filter(|_| memory == crate::gpu::Memory::Shared);
    let limited = sample.gpu_in_use.unwrap_or(sample.wired);
    match limit {
        Some(limit) if limited > limit => Left::OverLimit,
        Some(limit) if limit - limited < ram => Left::ToLimit(limit - limited),
        _ => Left::Ram(ram),
    }
}

/// GPU memory's colour: the colour of wired RAM, which it is the larger
/// part of on a machine running a model. The header's meter uses it too.
pub(crate) fn gpu_fill() -> Color {
    kind_colors()[0]
}

/// The part of GPU memory that is not the model's weights.
const GPU_OTHER: Color = Color::Rgb(170, 190, 255);

/// Wired RAM the GPU does not hold: the kernel and locked pages.
const OTHER_WIRED: Color = Color::Rgb(74, 96, 178);

/// The history's band colours, dimmed: GPU memory, other wired RAM, app
/// and compressed.
pub(crate) fn history_colors() -> [Color; 4] {
    let kinds = kind_colors();
    [gpu_fill(), OTHER_WIRED, kinds[1], kinds[2]].map(dim)
}

/// A colour two fifths of the way out of the panel's background.
fn dim(color: Color) -> Color {
    match (color, PANEL) {
        (Color::Rgb(r, g, b), Color::Rgb(pr, pg, pb)) => {
            let mix = |c: u8, p: u8| ((u16::from(c) * 2 + u16::from(p) * 3) / 5) as u8;
            Color::Rgb(mix(r, pr), mix(g, pg), mix(b, pb))
        }
        _ => color,
    }
}

/// The warning colour of GPU memory near its limit, as the header's meter.
fn gpu_tone(used: u64, limit: u64) -> Option<Color> {
    let percent = used as f64 * 100.0 / limit as f64;
    if percent >= 97.0 {
        Some(RED)
    } else if percent >= 90.0 {
        Some(YELLOW)
    } else {
        None
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

/// One entry of the breakdown: a kind of memory, its colour and its size.
#[derive(Clone, Copy)]
struct Part {
    name: &'static str,
    color: Color,
    size: u64,
}

/// The breakdown as legend rows: one row where it fits, else two balanced
/// rows of whole entries. `None` when the sizes cannot be shown whole.
fn legend_rows(parts: &[Part], room: usize, rows: usize) -> Option<Vec<Vec<Span<'static>>>> {
    let sizes: Vec<u64> = parts.iter().map(|part| part.size).collect();
    let unit = shared_unit(&sizes);
    let item = |index: usize, last: bool| {
        let part = parts[index];
        let size = in_unit(part.size, unit.as_deref());
        let text = match (&unit, last) {
            (Some(unit), true) => format!("{} {size} {unit}", part.name),
            _ => format!("{} {size}", part.name),
        };
        vec![
            Span::styled("█", Style::default().fg(part.color)),
            Span::styled(text, Style::default().fg(MUTED)),
        ]
    };
    let line = |indent: usize, range: std::ops::Range<usize>| {
        let mut spans = vec![Span::raw(" ".repeat(indent))];
        let end = range.end;
        for index in range {
            if spans.len() > 1 {
                spans.push(Span::raw(" "));
            }
            spans.extend(item(index, index + 1 == end));
        }
        spans
    };
    let width = |spans: &[Span]| spans.iter().map(Span::width).sum::<usize>();
    for indent in [usize::from(PREFIX), 0] {
        let whole = line(indent, 0..parts.len());
        if rows >= 1 && width(&whole) <= room {
            return Some(vec![whole]);
        }
    }
    if rows < 2 {
        return None;
    }
    for indent in [usize::from(PREFIX), 0] {
        // The split that leaves the two rows nearest in width.
        let split = (1..parts.len())
            .map(|at| (line(indent, 0..at), line(indent, at..parts.len())))
            .filter(|(first, second)| width(first) <= room && width(second) <= room)
            .min_by_key(|(first, second)| width(first).max(width(second)));
        if let Some((first, second)) = split {
            return Some(vec![first, second]);
        }
    }
    None
}

/// Draw the panel body: one bar of all RAM by what holds it, GPU memory
/// first; what is left, and how much more context fits; each part's size;
/// who holds the most; and the same parts over time. `columns` gives, for a
/// plot of that many columns, each column's memory. Returns false when the
/// OS reports no composition.
pub(crate) fn draw(
    frame: &mut Frame,
    area: Rect,
    sample: &Sample,
    columns: impl Fn(usize) -> Vec<Option<Column>>,
) -> bool {
    draw_for(frame, area, sample, sample.gpu_memory(), columns)
}

/// `draw` for a machine whose GPU memory is `memory`.
pub(crate) fn draw_for(
    frame: &mut Frame,
    area: Rect,
    sample: &Sample,
    memory: crate::gpu::Memory,
    columns: impl Fn(usize) -> Vec<Option<Column>>,
) -> bool {
    let Some(sizes) = segments(sample) else {
        return false;
    };
    if area.is_empty() {
        return true;
    }
    let [wired, app, compressed, cache, _] = sizes;
    // Cards with memory of their own: a second pool, with its own meter.
    let dedicated = match memory {
        crate::gpu::Memory::Dedicated { used, total } => Some((used, total)),
        _ => None,
    };
    let shared = memory == crate::gpu::Memory::Shared;
    let limit = gpu_limit(sample).filter(|_| shared);
    // Shared memory: what the GPU holds is part of wired RAM. A count
    // above wired RAM is not a part of it to draw: the kinds then stay
    // whole and the count is only held against the limit.
    let gpu = sample.gpu_in_use.filter(|gpu| shared && *gpu <= wired);
    // Against the limit: Metal's own count, else wired RAM, which bounds it.
    let limited = sample.gpu_in_use.unwrap_or(sample.wired);
    let warning = limit.and_then(|limit| gpu_tone(limited, limit));
    let model = gpu.and_then(|gpu| model_share(sample, gpu));
    // Each meter gets a second row once there is room for it.
    let detailed = area.height >= 2 * (1 + u16::from(dedicated.is_some()));
    let mut y = area.y;
    let bottom = area.bottom();
    let row = |y: u16| Rect::new(area.x, y, area.width, 1);
    let tone = sample.pressure_tone.color();

    if let Some((vram, total)) = dedicated {
        let near_full = gpu_tone(vram, total);
        let bar = meter(
            frame,
            row(y),
            "VRAM",
            &of_total(vram, total),
            near_full.unwrap_or(CYAN),
        );
        let filled =
            (u128::from(vram.min(total)) * u128::from(bar.width) / u128::from(total)) as u16;
        for x in 0..bar.width {
            frame.buffer_mut()[(bar.x + x, bar.y)]
                .set_symbol(" ")
                .set_bg(if x < filled {
                    near_full.unwrap_or_else(gpu_fill)
                } else {
                    EDGE
                });
        }
        y += 1;
        if detailed && y < bottom {
            text(
                frame,
                row(y),
                vec![vec![]],
                Span::styled(
                    format!("{} left", bytes(total.saturating_sub(vram))),
                    Style::default().fg(near_full.unwrap_or(CYAN)),
                ),
            );
            y += 1;
        }
        if y >= bottom {
            return true;
        }
    }

    // All of RAM in one bar.
    let used = wired + app + compressed;
    let bar = meter(
        frame,
        row(y),
        "RAM",
        &of_total(used, sample.total_memory),
        tone,
    );
    draw_composition(frame, bar, sample, false);
    // The wired segment split: the model's weights, the rest of what the
    // GPU holds, then wired RAM that is not the GPU's.
    if let Some(gpu) = gpu.filter(|_| wired > 0) {
        let wired_color = kind_colors()[0];
        let cells = (0..bar.width)
            .take_while(|x| frame.buffer_mut()[(bar.x + x, bar.y)].bg == wired_color)
            .count() as u64;
        let share = |value: u64| {
            ((u128::from(value) * u128::from(cells) * 2 + u128::from(wired))
                / (u128::from(wired) * 2)) as u64
        };
        let gpu_cells = share(gpu);
        let model_cells = model.map_or(gpu_cells, share);
        for x in 0..cells {
            let fill = if x >= gpu_cells {
                OTHER_WIRED
            } else if let Some(warning) = warning {
                warning
            } else if x < model_cells {
                gpu_fill()
            } else {
                GPU_OTHER
            };
            frame.buffer_mut()[(bar.x + x as u16, bar.y)].set_bg(fill);
        }
    }
    y += 1;

    let room = usize::from(area.width);
    let prefix = usize::from(PREFIX);
    // The breakdown, most detailed first: the GPU's part split where the
    // runtime gives its model's size.
    let mut breakdowns: Vec<Vec<Part>> = Vec::new();
    let rest = [
        Part {
            name: "app",
            color: kind_colors()[1],
            size: app,
        },
        Part {
            name: "comp",
            color: kind_colors()[2],
            size: compressed,
        },
        Part {
            name: "cache",
            color: kind_colors()[3],
            size: cache,
        },
    ];
    match gpu {
        Some(gpu) => {
            let gpu_color = warning.unwrap_or_else(gpu_fill);
            let other = Part {
                name: "other wired",
                color: OTHER_WIRED,
                size: wired - gpu,
            };
            if let Some(model) = model {
                let mut parts = vec![
                    Part {
                        name: "model",
                        color: gpu_color,
                        size: model,
                    },
                    Part {
                        name: "KV cache & other",
                        color: warning.unwrap_or(GPU_OTHER),
                        size: gpu - model,
                    },
                    other,
                ];
                parts.extend(rest);
                breakdowns.push(parts);
            }
            let mut parts = vec![
                Part {
                    name: "GPU",
                    color: gpu_color,
                    size: gpu,
                },
                other,
            ];
            parts.extend(rest);
            breakdowns.push(parts);
        }
        None => {
            let mut parts = vec![Part {
                name: "wired",
                color: kind_colors()[0],
                size: wired,
            }];
            parts.extend(rest);
            breakdowns.push(parts);
        }
    }

    // Who holds it: the runtime's own footprint and the largest other app.
    let holders = {
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
        (!parts.is_empty()).then(|| format!("{:<5}by app: {}", "", parts.join(" · ")))
    };
    // The same pool over time, where there is width for it and samples.
    let points = (area.width > AXIS_GUTTER + 2)
        .then(|| columns(usize::from(area.width - AXIS_GUTTER)))
        .filter(|points| points.iter().any(Option::is_some));

    if detailed && y < bottom {
        // What is left for a model to grow into: free RAM and file cache,
        // or the room under the GPU limit where that is less.
        let headroom = match left_of(sample, sizes, memory) {
            Left::OverLimit => Span::styled("over the GPU limit", Style::default().fg(RED)),
            Left::ToLimit(room) => Span::styled(
                format!("{} to GPU limit", bytes(room)),
                Style::default().fg(warning.unwrap_or(tone)),
            ),
            Left::Ram(room) => {
                Span::styled(format!("{} left", bytes(room)), Style::default().fg(tone))
            }
        };
        // How close GPU memory is to its limit, beside what is left. A
        // GPU that shares RAM without a count of its part says so: there
        // is no VRAM to look for.
        let style = Style::default().fg(warning.unwrap_or(MUTED));
        let gpu_note: Option<Vec<Span<'static>>> = match limit {
            Some(limit) => {
                let percent = (limited as f64 * 100.0 / limit as f64).round();
                Some(vec![
                    Span::styled(
                        format!("GPU at {percent:.0}% of {} limit", bytes(limit)),
                        style,
                    ),
                    Span::styled(format!("GPU {percent:.0}% of {}", bytes(limit)), style),
                    Span::styled(format!("GPU {percent:.0}% of limit"), style),
                ])
            }
            None if shared && gpu.is_none() && !sample.gpus.is_empty() => Some(vec![
                Span::styled("GPU shares this RAM", style),
                Span::styled("GPU shares RAM", style),
            ]),
            None => None,
        };
        let beside = |spans: &[Span]| {
            spans.iter().map(Span::width).sum::<usize>() + 1 + headroom.width() <= room
        };
        // The GPU note's own row, where it does not fit beside what is left.
        let mut note_row: Option<Span<'static>> = None;
        match gpu_note {
            Some(notes) => {
                let indented =
                    |note: &Span<'static>| vec![Span::raw(" ".repeat(prefix)), note.clone()];
                let fitting: Vec<Vec<Span<'static>>> = notes
                    .iter()
                    .flat_map(|note| [indented(note), vec![note.clone()]])
                    .filter(|spans| beside(spans))
                    .collect();
                let placed = !fitting.is_empty();
                text(
                    frame,
                    row(y),
                    if placed { fitting } else { vec![vec![]] },
                    headroom,
                );
                y += 1;
                if !placed {
                    note_row = Some(
                        notes
                            .iter()
                            .find(|note| note.width() <= room)
                            .cloned()
                            .unwrap_or_default(),
                    );
                }
            }
            // No GPU limit to report: the breakdown sits beside what is
            // left when one row holds both.
            None => {
                let inline = breakdowns
                    .iter()
                    .filter_map(|parts| legend_rows(parts, room, 1))
                    .filter_map(|rows| rows.into_iter().next())
                    .find(|spans| beside(spans));
                let placed = inline.is_some();
                text(frame, row(y), vec![inline.unwrap_or_default()], headroom);
                y += 1;
                if placed {
                    breakdowns.clear();
                }
            }
        }
        // Each part with its size, in the rows free for it: the legend is
        // the breakdown. Names alone are the last resort: the sizes are the
        // point.
        let legend = |free_rows: usize| -> Vec<Vec<Span<'static>>> {
            let names = || {
                let parts = breakdowns.last()?;
                [prefix, 0].into_iter().find_map(|indent| {
                    let mut spans = vec![Span::raw(" ".repeat(indent))];
                    for (index, part) in parts.iter().enumerate() {
                        spans.push(Span::styled("█", Style::default().fg(part.color)));
                        spans.push(Span::styled(
                            format!(
                                "{}{}",
                                part.name,
                                if index + 1 < parts.len() { " " } else { "" }
                            ),
                            Style::default().fg(MUTED),
                        ));
                    }
                    (free_rows >= 1 && spans.iter().map(Span::width).sum::<usize>() <= room)
                        .then_some(vec![spans])
                })
            };
            breakdowns
                .iter()
                .find_map(|parts| legend_rows(parts, room, free_rows.min(2)))
                .or_else(names)
                .unwrap_or_default()
        };
        // What the rows below hold, with the forecast's row or without it.
        let below = |forecast: bool| -> Option<Below> {
            let mut at = y;
            let note = note_row.is_some() && at < bottom;
            at += u16::from(note);
            if forecast && at >= bottom {
                return None;
            }
            at += u16::from(forecast);
            let legend = legend(usize::from(bottom.saturating_sub(at)));
            at += legend.len() as u16;
            let holders = holders.is_some() && at + 3 < bottom;
            at += u16::from(holders);
            let rows = bottom.saturating_sub(at);
            let charts = match (&points, dedicated) {
                (None, _) => 0,
                (Some(_), _) if rows < 2 => 0,
                (Some(_), Some(_)) if rows >= 4 => 2,
                (Some(_), _) => 1,
            };
            Some(Below {
                note,
                legend,
                holders,
                charts,
            })
        };
        // How much more context fits, under what is left. It is the first
        // row to go: shown only where everything else keeps its rows.
        let forecast = crate::context_left::forecast_for(sample, memory)
            .map(|forecast| forecast.label())
            .filter(|label| Line::from(label.as_str()).width() <= room)
            .filter(|_| below(true).is_some_and(|with| below(false) == Some(with)));
        if let Some(label) = forecast {
            text(
                frame,
                row(y),
                vec![vec![]],
                Span::styled(label, Style::default().fg(MUTED)),
            );
            y += 1;
        }
        if let Some(note) = note_row.filter(|_| y < bottom) {
            frame.render_widget(
                Paragraph::new(Line::from(note)).style(Style::default().bg(PANEL)),
                row(y),
            );
            y += 1;
        }
        for spans in legend(usize::from(bottom.saturating_sub(y))) {
            frame.render_widget(
                Paragraph::new(Line::from(spans)).style(Style::default().bg(PANEL)),
                row(y),
            );
            y += 1;
        }
    }
    if let Some(line) = holders.filter(|_| detailed && y + 3 < bottom) {
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
    // The same pool over time, against all of RAM: GPU memory, other wired
    // RAM, app and compressed, stacked. A model load or a growing KV cache
    // shows as the GPU band rising, another app as the app band.
    let rows = bottom.saturating_sub(y);
    let Some(points) = points.filter(|_| rows >= 2) else {
        return true;
    };
    let bands = history_colors();
    // Without a GPU count the wired band is all one colour.
    let wired_color = dim(kind_colors()[0]);
    let wired_only = [wired_color, wired_color, bands[2], bands[3]];
    // Two pools, two charts, where each can have two rows; else RAM alone.
    let vram_rows = match dedicated {
        Some(_) if rows >= 4 => rows / 2,
        _ => 0,
    };
    if let Some((_, total)) = dedicated.filter(|_| vram_rows > 0) {
        level_chart(
            frame,
            Rect::new(area.x, y, area.width, vram_rows),
            Some("VRAM"),
            total.max(1),
            &points,
            |column| column.gpu.map(|vram| ([vram; 4], [bands[0]; 4])),
        );
        y += vram_rows;
    }
    level_chart(
        frame,
        Rect::new(area.x, y, area.width, rows - vram_rows),
        (vram_rows > 0).then_some("RAM"),
        sample.total_memory.max(1),
        &points,
        |column| {
            let [wired, app, compressed, ..] = column.sizes;
            let (gpu, colors) = match column.gpu.filter(|gpu| shared && *gpu <= wired) {
                Some(gpu) => (gpu, bands),
                None => (0, wired_only),
            };
            Some(([gpu, wired, wired + app, wired + app + compressed], colors))
        },
    );
    true
}

/// What the rows under what is left hold: the GPU note's own row, the
/// legend, who holds the memory, and how many history charts.
#[derive(PartialEq)]
struct Below {
    note: bool,
    legend: Vec<Vec<Span<'static>>>,
    holders: bool,
    charts: u8,
}

/// A level over time against `top`: up to four stacked bands per column,
/// each given by where it ends, drawn dim with the stack's top to the
/// eighth of a row. The gutter gives the top, then the chart's name where
/// there are two charts, else zero at the bottom.
fn level_chart(
    frame: &mut Frame,
    area: Rect,
    name: Option<&str>,
    top: u64,
    points: &[Option<Column>],
    stack: impl Fn(&Column) -> Option<([u64; 4], [Color; 4])>,
) {
    const BLOCKS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let rows = area.height;
    let plot = Rect::new(area.x + AXIS_GUTTER, area.y, area.width - AXIS_GUTTER, rows);
    let gutter = |y: u16| Rect::new(area.x, y, AXIS_GUTTER, 1);
    frame_label(frame, gutter(plot.y), bytes(top));
    match name {
        Some(name) if rows >= 2 => frame_label(frame, gutter(plot.y + 1), name.to_owned()),
        Some(_) => {}
        None => frame_label(frame, gutter(plot.bottom() - 1), "0".into()),
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
        let height = tops[3].max(u16::from(ends[3] > 0));
        let mut top_level = 0;
        for level in 0..rows {
            let base = level * 8;
            if base >= height {
                break;
            }
            // The band at the cell's middle, or the topmost where the stack
            // ends inside the cell; its top is drawn to the eighth.
            let middle = (base + 4).min(height - 1);
            let band = tops.iter().position(|end| middle < *end).unwrap_or(3);
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

/// One column of the panel's history: RAM by kind, what the GPU holds (its
/// own memory, or its part of RAM), and the tone its sample was marked in.
#[derive(Clone, Copy)]
pub(crate) struct Column {
    pub(crate) sizes: [u64; 5],
    pub(crate) gpu: Option<u64>,
    pub(crate) tone: Tone,
}

fn frame_label(frame: &mut Frame, area: Rect, value: String) {
    frame.render_widget(
        Paragraph::new(format!("{value:>8} ")).style(Style::default().fg(MUTED)),
        area,
    );
}
