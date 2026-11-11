// SPDX-License-Identifier: MIT
//! Draws a slowdown onto the dashboards: the panel that holds its cause.
use crate::chart_navigation::Chart;
use crate::domain::{CorrelationCause, CorrelationInsight};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::Frame;

/// The panel that shows a signal a slowdown can be explained by.
pub(crate) fn chart_of(cause: CorrelationCause) -> Option<Chart> {
    Some(match cause {
        CorrelationCause::Paging => Chart::Paging,
        CorrelationCause::Compression => Chart::Compression,
        CorrelationCause::MemoryPressure
        | CorrelationCause::ModelMemory
        | CorrelationCause::MetalMemory => Chart::Memory,
        CorrelationCause::Thermal | CorrelationCause::GpuIdle => Chart::Gpu,
        CorrelationCause::DiskIo => Chart::Cache,
        CorrelationCause::ConcurrentPrefill => Chart::Prefill,
        CorrelationCause::Queueing => Chart::Queue,
        // The runtime's CPU is a figure in the header, not a chart.
        CorrelationCause::Cpu | CorrelationCause::Stall | CorrelationCause::None => return None,
    })
}

/// The panel showing the signal a slowdown or stall is given as the reason
/// for: the first unusual one, or what the host was doing beside a stall.
pub(crate) fn linked(insight: &CorrelationInsight) -> Option<Chart> {
    if !insight.is_material_drop() || insight.lead.is_empty() {
        return None;
    }
    let lead = insight.lead.as_str();
    match insight.cause {
        CorrelationCause::Stall if lead.contains("disk ") || lead.contains("SSD cache") => {
            Some(Chart::Cache)
        }
        CorrelationCause::Stall if lead.contains("paging") => Some(Chart::Paging),
        CorrelationCause::Stall if lead.contains("memory pressure") => Some(Chart::Memory),
        cause => chart_of(cause),
    }
}

/// What the linked panel is tagged with: that this signal is what slowed
/// generation, and by how much, in the widest form that fits `room` cells.
pub(crate) fn tag(insight: &CorrelationInsight, room: usize) -> String {
    let stall = insight.cause == CorrelationCause::Stall;
    let drop = insight
        .delta_percent
        .filter(|_| !stall)
        .map(|percent| format!(" ↓{:.0}%", percent.abs()))
        .unwrap_or_default();
    let (verb, short) = match (stall, insight.phase) {
        (true, _) => ("stalled GEN", " ▼ STALL "),
        (false, crate::domain::Phase::Generation) => ("slowed GEN", " ▼ GEN "),
        (false, crate::domain::Phase::Prefill) => ("slowed PREFILL", " ▼ PREFILL "),
    };
    [
        format!(" ▲ {verb}{drop} "),
        format!(" ▲ {verb} "),
        short.to_owned(),
    ]
    .into_iter()
    .find(|text| Line::from(text.as_str()).width() <= room)
    .unwrap_or_else(|| short.to_owned())
}

/// Room for the tag on a panel `width` cells wide: the corner, a rule cell
/// on each side, and the other corner.
pub(crate) fn tag_room(width: u16) -> usize {
    usize::from(width).saturating_sub(4)
}

/// Marks the panel at `area` as the one behind the slowdown: its border in
/// the slowdown's colour and the tag at the right of its bottom edge.
pub(crate) fn mark(frame: &mut Frame, area: Rect, insight: &CorrelationInsight) {
    let tag = tag(insight, tag_room(area.width) / 2);
    let width = Line::from(tag.as_str()).width() as u16;
    if area.height < 2 || area.width < width + 4 {
        return;
    }
    let color = crate::domain::Tone::Unusual.color();
    let buffer = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let edge = y == area.top()
                || y + 1 == area.bottom()
                || x == area.left()
                || x + 1 == area.right();
            let cell = &mut buffer[(x, y)];
            if edge && matches!(cell.symbol(), "─" | "│" | "┌" | "┐" | "└" | "┘") {
                cell.set_fg(color);
            }
        }
    }
    buffer.set_string(
        area.right() - 2 - width,
        area.bottom() - 1,
        &tag,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    );
}
