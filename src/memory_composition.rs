// SPDX-License-Identifier: MIT
//! Physical RAM by kind: what can be reclaimed, compressed or never moved.
//! Wired memory cannot be compressed or swapped; free and file cache are
//! headroom. A marker shows the GPU working-set limit when a runtime reports it;
//! it turns yellow, with a legend reason, once wired memory exceeds the limit.
use crate::domain::Sample;
use crate::formatting::bytes;
use crate::theme::{BLUE, CYAN, DIM, EDGE, MUTED, PANEL, YELLOW};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// One segment: legend name and fill. Solid fills that differ in hue (wired,
/// app) or lightness (compressed, cache) tell kinds apart; free is the track.
struct Kind {
    name: &'static str,
    color: Color,
}

const WIRED: Kind = Kind {
    name: "wired",
    color: BLUE,
};
const APP: Kind = Kind {
    name: "app",
    color: CYAN,
};
const COMPRESSED: Kind = Kind {
    name: "comp",
    color: Color::Rgb(128, 140, 158),
};
const CACHE: Kind = Kind {
    name: "cache",
    color: Color::Rgb(84, 98, 120),
};
const FREE: Kind = Kind {
    name: "free",
    color: EDGE,
};

/// Each kind's fill, in stacking order from the bottom: wired, app,
/// compressed, cache, free.
pub(crate) fn kind_colors() -> [Color; 5] {
    kinds().map(|kind| kind.color)
}

/// The history's fills: the same hues as the bar, softened toward the panel.
/// The one-row bar is a small mark and keeps full colour; a chart-sized
/// stack of saturated blue would outshout every other panel.
pub(crate) fn history_colors() -> [Color; 5] {
    kind_colors().map(|color| match (color, PANEL) {
        (Color::Rgb(r, g, b), Color::Rgb(pr, pg, pb)) => {
            let mix = |c: u8, p: u8| ((u16::from(c) * 60 + u16::from(p) * 40) / 100) as u8;
            Color::Rgb(mix(r, pr), mix(g, pg), mix(b, pb))
        }
        (color, _) => color,
    })
}

/// The GPU wired-memory cap: an explicit `iogpu.wired_limit_mb`, else the
/// runtime's reported Metal recommended working set.
pub(crate) fn gpu_limit(sample: &Sample) -> Option<u64> {
    sample
        .metal
        .resource_limit
        .or(sample.mlx.recommended_working_set)
        .filter(|limit| *limit > 0)
}

/// Segment sizes in bytes, clamped so they never exceed physical RAM.
pub(crate) fn segments(sample: &Sample) -> Option<[u64; 5]> {
    if !sample.vm_available || sample.total_memory == 0 {
        return None;
    }
    let mut remaining = sample.total_memory;
    let mut take = |value: u64| {
        let value = value.min(remaining);
        remaining -= value;
        value
    };
    let wired = take(sample.wired);
    let app = take(sample.anonymous);
    let compressed = take(sample.compressor);
    let cache = take(sample.file_backed);
    // Without any kind counters everything would read as free: say unknown.
    if wired + app + compressed + cache == 0 {
        return None;
    }
    Some([wired, app, compressed, cache, remaining])
}

fn kinds() -> [Kind; 5] {
    [WIRED, APP, COMPRESSED, CACHE, FREE]
}

/// A one-row bar of RAM composition. Returns false when counters are missing.
pub(crate) fn draw_bar(frame: &mut Frame, area: Rect, sample: &Sample) -> bool {
    draw_composition(frame, area, sample, true)
}

/// The composition bar, with the GPU limit marked on it or left to a meter
/// of its own.
pub(crate) fn draw_composition(
    frame: &mut Frame,
    area: Rect,
    sample: &Sample,
    marker: bool,
) -> bool {
    let Some(sizes) = segments(sample) else {
        return false;
    };
    if area.is_empty() {
        return true;
    }
    let width = u64::from(area.width);
    let total = sample.total_memory;
    // Largest-remainder rounding keeps every column assigned exactly once.
    let mut columns = sizes.map(|size| u128::from(size) * u128::from(width) / u128::from(total));
    let assigned: u128 = columns.iter().sum();
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| {
        std::cmp::Reverse(u128::from(sizes[i]) * u128::from(width) % u128::from(total))
    });
    for &i in order.iter().take((u128::from(width) - assigned) as usize) {
        columns[i] += 1;
    }
    let mut x = area.x;
    for (kind, count) in kinds().iter().zip(columns) {
        for _ in 0..count {
            frame.buffer_mut()[(x, area.y)]
                .set_symbol(" ")
                .set_bg(kind.color);
            x += 1;
        }
    }
    // GPU working-set limit: wired model memory beyond it slows generation.
    if let Some(limit) = gpu_limit(sample).filter(|_| marker) {
        let column = (u128::from(limit.min(total)) * u128::from(width) / u128::from(total))
            .min(u128::from(width) - 1) as u16;
        let over = sample.wired > limit;
        frame.buffer_mut()[(area.x + column, area.y)]
            .set_symbol("┃")
            .set_fg(if over { YELLOW } else { Color::White });
    }
    true
}

/// One column of the composition history: wired, app, compressed and cache
/// stacked from the bottom of `rows` cells at eighth-cell precision, free
/// above them left empty. A cell two kinds share shows the lower kind as a
/// block of its height over the upper kind's colour.
pub(crate) fn paint_column(frame: &mut Frame, x: u16, top: u16, rows: u16, sizes: [u64; 5]) {
    const BLOCKS: [&str; 8] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇"];
    let total: u128 = sizes.iter().map(|size| u128::from(*size)).sum();
    if total == 0 || rows == 0 {
        return;
    }
    let eighths = u128::from(rows) * 8;
    // Where each used kind ends, in eighths from the bottom.
    let mut ends = [0_u128; 4];
    let mut cumulative = 0_u128;
    for (end, size) in ends.iter_mut().zip(sizes) {
        cumulative += u128::from(size);
        *end = (cumulative * eighths + total / 2) / total;
    }
    let kind_at = |eighth: u128| ends.iter().position(|end| eighth < *end).unwrap_or(4);
    let colors = history_colors();
    for row in 0..rows {
        let base = u128::from(row) * 8;
        let lower = kind_at(base);
        if lower == 4 {
            break;
        }
        let filled = (ends[lower].min(base + 8) - base) as usize;
        let cell = &mut frame.buffer_mut()[(x, top + rows - 1 - row)];
        if filled >= 8 {
            cell.set_symbol(" ").set_bg(colors[lower]);
        } else {
            let upper = kind_at(base + filled as u128);
            cell.set_symbol(BLOCKS[filled])
                .set_fg(colors[lower])
                .set_bg(if upper == 4 { PANEL } else { colors[upper] });
        }
    }
}

/// Legend with exact sizes, flowing onto at most `max_lines` rows. Entries
/// never split; ones that do not fit are dropped whole.
pub(crate) fn legend(sample: &Sample, width: u16, max_lines: usize) -> Vec<Line<'static>> {
    let Some(sizes) = segments(sample) else {
        return vec![Line::from(Span::styled(
            "RAM composition unavailable",
            Style::default().fg(MUTED),
        ))];
    };
    // The swatch carries the kind's colour; the words stay in text colour.
    let mut entries: Vec<(&'static str, Color, String, Color)> = kinds()
        .iter()
        .zip(sizes)
        .filter(|(kind, size)| *size > 0 || kind.name == "free")
        .map(|(kind, size)| {
            let swatch = if kind.name == "free" { "□" } else { "█" };
            let text = format!("{} {}", kind.name, bytes(size));
            (swatch, kind.color, text, MUTED)
        })
        .collect();
    if let Some(limit) = gpu_limit(sample) {
        // Over the limit is worth inspecting; the legend states the reason.
        entries.push(if sample.wired > limit {
            (
                "┃",
                YELLOW,
                format!("wired over GPU limit {}", bytes(limit)),
                YELLOW,
            )
        } else {
            (
                "┃",
                Color::White,
                format!("GPU limit {}", bytes(limit)),
                Color::White,
            )
        });
    }
    if max_lines == 0 {
        return Vec::new();
    }
    let mut lines = vec![Line::default()];
    for (swatch, swatch_color, text, text_color) in entries {
        let swatch_style = Style::default().fg(if swatch_color == EDGE {
            DIM
        } else {
            swatch_color
        });
        let entry_width = Line::from(format!("{swatch}{text}")).width();
        let line = lines.last_mut().unwrap();
        let fits = line.spans.is_empty() || line.width() + 2 + entry_width <= usize::from(width);
        if !fits || entry_width > usize::from(width) {
            if entry_width > usize::from(width) || lines.len() == max_lines {
                continue;
            }
            lines.push(Line::default());
        }
        let line = lines.last_mut().unwrap();
        if !line.spans.is_empty() {
            line.spans.push(Span::raw("  "));
        }
        line.spans.push(Span::styled(swatch, swatch_style));
        line.spans
            .push(Span::styled(text, Style::default().fg(text_color)));
    }
    lines
}

/// Draw the bar and, when rows allow, its legend beneath it. Returns rows used.
pub(crate) fn draw(frame: &mut Frame, area: Rect, sample: &Sample) -> u16 {
    if area.height == 0 {
        return 0;
    }
    if !draw_bar(frame, Rect::new(area.x, area.y, area.width, 1), sample) {
        frame.render_widget(
            Paragraph::new("RAM composition unavailable").style(Style::default().fg(MUTED)),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return 1;
    }
    if area.height < 2 {
        return 1;
    }
    let lines = legend(sample, area.width, usize::from(area.height - 1));
    let rows = lines.len() as u16;
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(area.x, area.y + 1, area.width, rows),
    );
    1 + rows
}

#[cfg(test)]
#[path = "tests/memory_composition.rs"]
mod tests;
