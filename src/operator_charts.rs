// SPDX-License-Identifier: MIT
use crate::chart_render::{bar_cells, chart_window_label, meter_cells};
use crate::chart_scale;
use crate::domain::Tone;
use crate::formatting::{compact_tokens, count, progress_percent, telemetry_age};
use crate::operator_history::{History, Phase, Running};
use crate::theme::{panel, BLUE, CYAN, DIM, EDGE, MUTED, PANEL, YELLOW};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::time::Duration;
// At 1×, one column per sample; no connections across missing readings
// or changed process/provider identity. Callers fit the scale to visible samples.
fn trace(
    frame: &mut Frame,
    area: Rect,
    values: &[Option<u64>],
    breaks: &[bool],
    ceiling: u64,
    color: Color,
    zoom: u16,
) {
    if area.is_empty() || ceiling == 0 {
        return;
    }
    let mut previous: Option<u16> = None;
    let mut previous_index = None;
    for column in 0..area.width {
        let from_right = usize::from((area.width - 1 - column) / zoom.max(1));
        let Some(i) = values.len().checked_sub(from_right + 1) else {
            continue;
        };
        let value = &values[i];
        if previous_index != Some(i) && breaks.get(i).copied().unwrap_or(false) {
            previous = None;
        }
        previous_index = Some(i);
        let Some(value) = value else {
            previous = None;
            continue;
        };
        let x = area.x + column;
        let scaled = ((u128::from((*value).min(ceiling)) * u128::from(area.height - 1)
            + u128::from(ceiling) / 2)
            / u128::from(ceiling)) as u16;
        let y = area.bottom() - 1 - scaled;
        let mut glyph = "━";
        if let Some(old_y) = previous.filter(|old_y| *old_y != y) {
            for row in old_y.min(y) + 1..old_y.max(y) {
                frame.buffer_mut()[(x, row)].set_symbol("┃").set_fg(color);
            }
            frame.buffer_mut()[(x, old_y)]
                .set_symbol(if y > old_y { "┓" } else { "┛" })
                .set_fg(color);
            glyph = if y > old_y { "┗" } else { "┏" };
        }
        frame.buffer_mut()[(x, y)]
            .set_symbol(if *value > ceiling { "↑" } else { glyph })
            .set_fg(color);
        previous = Some(y);
    }
}

fn panel_area(
    frame: &mut Frame,
    area: Rect,
    name: &str,
    caption: String,
    subtitle: String,
    footer: Option<String>,
) -> Rect {
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .style(Style::default().bg(PANEL))
        .title(Line::from(vec![
            Span::styled(
                format!(" {name} "),
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(caption, Style::default().fg(MUTED)),
        ]));
    if let Some(footer) = footer {
        block = block.title_bottom(Line::from(Span::styled(
            format!(" {footer} "),
            Style::default().fg(MUTED),
        )));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() || subtitle.is_empty() {
        return inner;
    }
    frame.render_widget(
        Paragraph::new(subtitle).style(Style::default().fg(MUTED)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(1),
    )
}

/// One mark per request, as many as fit, so a queue reads at a glance:
/// `●●` running, `○○○○` waiting. A queue longer than the room ends in `+`.
fn request_icons(count: Option<u64>, icon: char, room: usize) -> String {
    let count = usize::try_from(count.unwrap_or(0)).unwrap_or(usize::MAX);
    if count <= room {
        std::iter::repeat_n(icon, count).collect()
    } else {
        std::iter::repeat_n(icon, room.saturating_sub(1))
            .chain(std::iter::once('+'))
            .collect()
    }
}

fn queue_tick(value: u64) -> String {
    if value < 10_000 {
        return value.to_string();
    }
    let (divisor, suffix) = [
        (1_000_000_000_000_000_000_u64, "E"),
        (1_000_000_000_000_000, "P"),
        (1_000_000_000_000, "T"),
        (1_000_000_000, "G"),
        (1_000_000, "M"),
        (1_000, "k"),
    ]
    .into_iter()
    .find(|(divisor, _)| value >= *divisor)
    .unwrap();
    format!("{:.1}{suffix}", value as f64 / divisor as f64)
}

fn queue_axis(
    history: &History,
    width: u16,
    zoom: u16,
    window: Option<usize>,
) -> (u16, u64, usize) {
    let mut label_width = 3.min(width.saturating_sub(1));
    loop {
        let visible = window
            .unwrap_or(usize::from(width.saturating_sub(label_width)))
            .div_ceil(usize::from(zoom.max(1)))
            .max(1);
        let ceiling = chart_scale::ceiling(
            history
                .points
                .iter()
                .rev()
                .take(visible)
                .flat_map(|point| [point.active, point.waiting])
                .flatten()
                .max()
                .unwrap_or(0),
            1,
        );
        let required = (queue_tick(ceiling).len() as u16 + 1).min(width.saturating_sub(1));
        if required <= label_width {
            return (label_width, ceiling, visible);
        }
        // Recalculate after reserving the tick gutter. Only grow the gutter:
        // an old spike leaving the visible window must not toggle its width.
        label_width = required;
    }
}

// Merge actual stroke directions instead of stamping a cross into every
// occupied cell. Coincident vertical connectors must remain vertical lines.
fn queue_junction(active: char, waiting: char) -> char {
    if active == '↑' || waiting == '↑' {
        return '↑';
    }
    if active == '━' && waiting == '━' {
        return '≈';
    }
    let directions = |glyph| match glyph {
        '━' => 0b1010, // left + right
        '┃' => 0b0101, // up + down
        '┏' => 0b0110,
        '┓' => 0b1100,
        '┗' => 0b0011,
        '┛' => 0b1001,
        _ => 0,
    };
    match directions(active) | directions(waiting) {
        0b0101 => '┃',
        0b0110 => '┏',
        0b1100 => '┓',
        0b0011 => '┗',
        0b1001 => '┛',
        0b0111 => '┣',
        0b1101 => '┫',
        0b1110 => '┳',
        0b1011 => '┻',
        _ => '╋',
    }
}

/// The tail of a request id: enough to match the request in Journal, which
/// prints the whole id.
fn short_id(id: &str) -> String {
    let chars: Vec<char> = id.chars().collect();
    chars[chars.len().saturating_sub(6)..].iter().collect()
}

/// One request as a row of fixed columns, so states line up down the panel:
/// a state mark, its label, the id's tail, then the phase's own progress.
///
/// ```text
/// ◐ PREFILL 9ce7ef 35.6k ▕███████████▏ 99%
/// ● GEN     5381d8 1.4k out      25.8 tok/s
/// ○ QUEUED  a1b2c3 8.2k in
/// ```
fn request_row(request: &Running, width: usize) -> Line<'static> {
    let (mark, mark_color, label) = match request.phase {
        Phase::Prefill(_) => ('◐', CYAN, "PREFILL"),
        Phase::Generate => ('●', CYAN, "GEN"),
        Phase::Queued => ('○', YELLOW, "QUEUED"),
    };
    let mut spans = vec![
        Span::styled(format!("{mark} "), Style::default().fg(mark_color)),
        Span::styled(format!("{label:<7} "), Style::default().fg(Color::White)),
    ];
    // The id goes first when room is short: state and progress outrank it.
    let id = short_id(&request.id);
    let with_id = 2 + 8 + id.chars().count() + 1 + 12;
    if width >= with_id {
        spans.push(Span::styled(format!("{id} "), Style::default().fg(MUTED)));
    }
    let used: usize = spans.iter().map(Span::width).sum();
    let room = width.saturating_sub(used);
    let reading = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    match request.phase {
        Phase::Prefill(done) => {
            let percent = progress_percent(done, request.prompt);
            let tail = format!(" {percent:>2}%");
            let size = format!("{} ", compact_tokens(request.prompt));
            let bar_room = room.saturating_sub(tail.len());
            let size = if bar_room >= size.len() + 4 {
                size
            } else {
                String::new()
            };
            let bar = bar_room.saturating_sub(size.len());
            if !size.is_empty() {
                spans.push(Span::styled(size, Style::default().fg(MUTED)));
            }
            if bar >= 3 {
                spans.push(Span::styled(
                    meter_cells(percent as f64, bar),
                    Style::default().fg(CYAN).bg(EDGE),
                ));
            }
            spans.push(Span::styled(tail, reading));
        }
        Phase::Generate => {
            let rate = request
                .tps
                .filter(|tps| tps.is_finite() && *tps >= 0.0)
                .map(|tps| format!("{tps:.1} tok/s"));
            let output = request.output.map(compact_tokens);
            let rate_width = rate.as_ref().map_or(1, String::len);
            let detail = [
                output.as_ref().map(|output| format!("{output} out")),
                output.clone(),
            ]
            .into_iter()
            .flatten()
            .find(|detail| detail.len() + 1 + rate_width <= room)
            .unwrap_or_default();
            let gap = room.saturating_sub(detail.len() + rate_width);
            spans.push(Span::styled(detail, Style::default().fg(MUTED)));
            spans.push(Span::raw(" ".repeat(gap)));
            spans.push(match rate {
                // Without its own rate the row keeps its place and says so.
                Some(rate) => Span::styled(rate, reading),
                None => Span::styled("—", Style::default().fg(MUTED)),
            });
        }
        Phase::Queued => {
            let prompt = format!("{} in", compact_tokens(request.prompt));
            if prompt.len() <= room {
                spans.push(Span::styled(prompt, Style::default().fg(MUTED)));
            }
        }
    }
    Line::from(spans)
}

/// The requests in the server, a row each, with the runtime's counts in the
/// top border and what did not fit in the bottom one.
fn request_list(frame: &mut Frame, area: Rect, history: &History, counts: bool) {
    let last = history.points.back();
    let active = last.and_then(|p| p.active);
    let waiting = last.and_then(|p| p.waiting);
    let waiting_color = if waiting.is_some_and(|n| n > 0) {
        YELLOW
    } else {
        MUTED
    };
    let inner_width = usize::from(area.width.saturating_sub(2));
    let rows = usize::from(area.height.saturating_sub(2));
    let words = format!("{} active · {} waiting", count(active), count(waiting));
    // Marks stand in for words when the border is short.
    let counts_line = if words.chars().count() + 10 <= inner_width {
        vec![
            Span::styled(
                format!(" {} active", count(active)),
                Style::default().fg(Color::White),
            ),
            Span::styled(" · ", Style::default().fg(MUTED)),
            Span::styled(
                format!("{} waiting ", count(waiting)),
                Style::default().fg(waiting_color),
            ),
        ]
    } else {
        vec![
            Span::styled(format!(" ●{}", count(active)), Style::default().fg(CYAN)),
            Span::styled(
                format!(" ○{} ", count(waiting)),
                Style::default().fg(waiting_color),
            ),
        ]
    };
    let mut block = panel("queue", Tone::Cyan);
    // Enlarged, the history below names the counts.
    if counts {
        block = block.title(Line::from(counts_line).right_aligned());
    }
    let hidden = history.running.len().saturating_sub(rows);
    if hidden > 0 {
        block = block.title_bottom(
            Line::from(Span::styled(
                format!(" +{hidden} more "),
                Style::default().fg(MUTED),
            ))
            .right_aligned(),
        );
    }
    let lines: Vec<Line> = history
        .running
        .iter()
        .take(rows)
        .map(|request| request_row(request, inner_width))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

pub(super) fn queue_plot_width(history: &History, width: u16) -> u16 {
    let inner = width.saturating_sub(2);
    let (gutter, _, _) = queue_axis(history, inner, 1, None);
    inner.saturating_sub(gutter)
}

pub(super) fn queue(
    frame: &mut Frame,
    area: Rect,
    history: &History,
    interval: Duration,
    zoom: u16,
    window: Option<usize>,
) {
    // When the runtime reports each request, the panel lists them: on
    // Overview in place of the history, enlarged above it.
    // One request is the one prompt load already shows in full.
    if history.running.len() > 1 && area.height >= 3 {
        if window.is_some() {
            request_list(frame, area, history, true);
            return;
        }
        if area.height >= 12 {
            let rows = (history.running.len() as u16 + 2).min(area.height / 2);
            request_list(
                frame,
                Rect::new(area.x, area.y, area.width, rows),
                history,
                false,
            );
            queue_history(
                frame,
                Rect::new(area.x, area.y + rows, area.width, area.height - rows),
                history,
                interval,
                zoom,
                window,
            );
            return;
        }
    }
    queue_history(frame, area, history, interval, zoom, window);
}

fn queue_history(
    frame: &mut Frame,
    area: Rect,
    history: &History,
    interval: Duration,
    zoom: u16,
    window: Option<usize>,
) {
    let last = history.points.back();
    let active = last.and_then(|p| p.active);
    let waiting = last.and_then(|p| p.waiting);
    // Yellow means "worth inspecting": an empty wait queue stays neutral.
    let waiting_color = |queued: bool| if queued { YELLOW } else { MUTED };
    // A short terminal still gets a separate, selectable queue panel with
    // both exact counts. Expanding it reveals the full history.
    if area.height <= 5 {
        let queued_color = waiting_color(waiting.is_some_and(|n| n > 0));
        let active_label = format!("active {}", count(active));
        let waiting_label = format!("waiting {}", count(waiting));
        let mut block = panel(
            &if area.height <= 2 {
                format!("queue {active_label}")
            } else {
                "queue".into()
            },
            Tone::Cyan,
        );
        let mut lines = Vec::new();
        // Counts first, then a mark per request, starting in one column just
        // past the longer label so both rows line up.
        let start = active_label.len().max(waiting_label.len()) + 1;
        let room = usize::from(area.width.saturating_sub(2)).saturating_sub(start);
        let row = |label: &str, value: Option<u64>, icon: char, color: Color| {
            Line::from(vec![
                Span::styled(
                    format!("{:<start$}", format!("{label} {}", count(value))),
                    Style::default().fg(color),
                ),
                Span::styled(request_icons(value, icon, room), Style::default().fg(color)),
            ])
        };
        if area.height >= 3 {
            lines.push(row("active", active, '●', CYAN));
        }
        if area.height >= 4 {
            lines.push(row("waiting", waiting, '○', queued_color));
        } else {
            // In the border there is room for the marks after the label too.
            let room =
                usize::from(area.width.saturating_sub(4)).saturating_sub(waiting_label.len() + 3);
            let icons = request_icons(waiting, '○', room);
            block = block.title_bottom(Line::from(Span::styled(
                if icons.is_empty() {
                    format!(" {waiting_label} ")
                } else {
                    format!(" {waiting_label} {icons} ")
                },
                Style::default().fg(queued_color),
            )));
        }

        frame.render_widget(Paragraph::new(lines).block(block), area);
        return;
    }
    let narrow = area.width < 38;
    let (label_width, ceiling, visible) =
        queue_axis(history, area.width.saturating_sub(2), zoom, window);
    let waiting_tone = waiting_color(
        history
            .points
            .iter()
            .rev()
            .take(visible)
            .any(|point| point.waiting.is_some_and(|n| n > 0)),
    );
    let ceiling_label = queue_tick(ceiling);
    let counts = format!("active {} waiting {}", count(active), count(waiting));
    let split_counts = narrow && counts.len() > usize::from(area.width.saturating_sub(2));
    let overview = window.is_some();
    let window = chart_window_label(
        if window.is_some() {
            visible
        } else {
            history.points.len().min(visible)
        },
        interval,
    );
    // On the Overview the axis ticks give the range and the top bar the
    // window, so the plot gets the row; an expanded queue names its window.
    let subtitle = if split_counts {
        format!("active {}", count(active))
    } else if overview {
        String::new()
    } else {
        window
    };
    let plot = panel_area(
        frame,
        area,
        "queue",
        if narrow {
            String::new()
        } else {
            format!("active {} · waiting {} ", count(active), count(waiting))
        },
        subtitle,
        None,
    );
    if plot.height < 2 || plot.width <= label_width {
        return;
    }
    let legend_width = usize::from(area.width.saturating_sub(if narrow { 2 } else { 4 }));
    let legend = if narrow {
        Line::from(vec![
            Span::styled(
                if split_counts {
                    String::new()
                } else {
                    format!("active {} ", count(active))
                },
                Style::default().fg(CYAN),
            ),
            Span::styled(
                format!("waiting {}", count(waiting)),
                Style::default().fg(waiting_tone),
            ),
        ])
    } else {
        // Swatches name each stroke; ═ marks samples where both counts match.
        let mut legend = Line::from(vec![
            Span::styled("━ active  ", Style::default().fg(CYAN)),
            Span::styled("━ waiting", Style::default().fg(waiting_tone)),
        ]);
        let equal = Span::styled("  ═ equal", Style::default().fg(Color::White));
        if legend.width() + equal.width() <= legend_width {
            legend.spans.push(equal);
        }
        legend
    };
    frame.render_widget(
        Paragraph::new(legend),
        // Keep the legend in the bottom border so a six-row panel still has
        // three plot rows: the 0, 1 and 2 levels must remain distinguishable.
        Rect::new(
            area.x + if narrow { 1 } else { 2 },
            area.bottom() - 1,
            area.width.saturating_sub(if narrow { 2 } else { 4 }),
            1,
        ),
    );
    let graph = Rect::new(
        plot.x + label_width,
        plot.y,
        plot.width - label_width,
        plot.height,
    );
    for (y, label) in [(graph.y, ceiling_label), (graph.bottom() - 1, "0".into())] {
        frame.render_widget(
            Paragraph::new(label).style(Style::default().fg(MUTED)),
            Rect::new(plot.x, y, label_width, 1),
        );
    }
    for x in graph.x..graph.right() {
        frame.buffer_mut()[(x, graph.bottom() - 1)]
            .set_symbol("─")
            .set_fg(DIM);
    }
    // Expand the shared ordinal sample window across this graph, retaining gaps.
    let indices: Vec<_> = (0..usize::from(graph.width))
        .map(|column| {
            let offset = column * visible / usize::from(graph.width);
            history.points.len().checked_sub(visible - offset)
        })
        .collect();
    let active_values: Vec<_> = indices
        .iter()
        .map(|i| i.and_then(|i| history.points[i].active))
        .collect();
    let waiting_values: Vec<_> = indices
        .iter()
        .map(|i| i.and_then(|i| history.points[i].waiting))
        .collect();
    let breaks: Vec<_> = indices
        .iter()
        .enumerate()
        .map(|(column, i)| {
            i.is_some_and(|i| i > 0 && history.points[i].provider != history.points[i - 1].provider)
                && (column == 0 || indices[column - 1] != *i)
        })
        .collect();
    trace(frame, graph, &active_values, &breaks, ceiling, CYAN, 1);
    let active_cells: Vec<_> = (graph.y..graph.bottom())
        .flat_map(|y| (graph.x..graph.right()).map(move |x| (x, y)))
        .filter_map(|(x, y)| {
            let cell = &frame.buffer_mut()[(x, y)];
            (cell.fg == CYAN).then_some((x, y, cell.symbol().chars().next().unwrap_or(' ')))
        })
        .collect();
    trace(
        frame,
        graph,
        &waiting_values,
        &breaks,
        ceiling,
        waiting_tone,
        1,
    );
    // Equality is a fact about the samples, not an intersection of rasterized
    // connectors. Preserve stroke geometry and mark rounded collisions separately.
    for (x, y, active_glyph) in active_cells {
        if frame.buffer_mut()[(x, y)].fg == waiting_tone {
            let column = usize::from(x - graph.x);
            let equal = active_values[column]
                .zip(waiting_values[column])
                .filter(|(active, waiting)| active == waiting)
                .is_some_and(|(value, _)| {
                    let scaled = ((u128::from(value.min(ceiling)) * u128::from(graph.height - 1)
                        + u128::from(ceiling) / 2)
                        / u128::from(ceiling)) as u16;
                    y == graph.bottom() - 1 - scaled
                });
            let waiting_glyph = frame.buffer_mut()[(x, y)]
                .symbol()
                .chars()
                .next()
                .unwrap_or(' ');
            let glyph = if equal && active_glyph != '↑' && waiting_glyph != '↑' {
                '═'
            } else {
                queue_junction(active_glyph, waiting_glyph)
            };
            frame.buffer_mut()[(x, y)]
                .set_symbol(&glyph.to_string())
                .set_fg(if equal { Color::White } else { MUTED });
        }
    }
    if active_values
        .iter()
        .chain(&waiting_values)
        .all(Option::is_none)
    {
        // Same empty-state vocabulary as the indicator charts.
        let earlier = history
            .points
            .iter()
            .any(|point| point.active.is_some() || point.waiting.is_some());
        frame.render_widget(
            Paragraph::new(if earlier {
                "No samples in this window"
            } else {
                "No queue samples yet"
            })
            .style(Style::default().fg(MUTED)),
            graph,
        );
    }
}

pub(super) fn latency(frame: &mut Frame, area: Rect, history: &History, zoom: u16) {
    let latest = history.timings.iter().rev().find(|p| p.1.is_some());
    let caption = latest
        .map(|p| format!("{} ms · REPORTED ", p.1.unwrap()))
        .unwrap_or_else(|| "— ".into());
    let visible = area.width.saturating_sub(2).div_ceil(zoom.max(1)) as usize;
    let ceiling = chart_scale::ceiling(
        history
            .timings
            .iter()
            .rev()
            .take(visible)
            .filter_map(|point| point.1)
            .max()
            .unwrap_or(0),
        1,
    );
    let subtitle = latest
        .map(|p| format!("0–{ceiling} ms · {}", telemetry_age(Some(p.2))))
        .unwrap_or_default();
    let plot = panel_area(frame, area, "first token", caption, subtitle, None);
    let values: Vec<_> = history.timings.iter().map(|p| p.1).collect();
    // Each bar is one observed request; zoom only widens it.
    for column in 0..plot.width {
        let i = usize::from(column / zoom.max(1));
        if let Some(Some(value)) = values
            .len()
            .checked_sub(i + 1)
            .and_then(|index| values.get(index))
        {
            let x = plot.right() - 1 - column;
            // Eighth-block tops and filled cells, as the throughput bars.
            let percent = ((*value).min(ceiling) as u128 * 100 / ceiling.max(1) as u128) as u64;
            let mut glyphs = bar_cells(percent, usize::from(plot.height));
            if glyphs.is_empty() {
                glyphs.push('▁');
            }
            for (height, glyph) in glyphs.into_iter().enumerate() {
                let cell = &mut frame.buffer_mut()[(x, plot.bottom() - 1 - height as u16)];
                if glyph == '█' {
                    cell.set_symbol(" ").set_bg(BLUE);
                } else {
                    cell.set_symbol(&glyph.to_string()).set_fg(BLUE);
                }
            }
            if *value > ceiling {
                frame.buffer_mut()[(x, plot.y)]
                    .set_symbol("↑")
                    .set_fg(BLUE)
                    .set_bg(PANEL);
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/operator_charts.rs"]
mod tests;
