// SPDX-License-Identifier: MIT
//! Draws a slowdown onto the dashboards: the panel that holds its cause.
use crate::chart_navigation::Chart;
use crate::domain::{CorrelationCause, CorrelationInsight};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::Frame;

/// The panel showing the signal a slowdown or stall is attributed to, when
/// the attribution is strong enough to be offered as its reason.
pub(crate) fn linked(insight: &CorrelationInsight) -> Option<Chart> {
    if !insight.is_material_drop() || insight.lead.is_empty() {
        return None;
    }
    let lead = insight.lead.as_str();
    Some(match insight.cause {
        CorrelationCause::Paging => Chart::Paging,
        CorrelationCause::Compression => Chart::Compression,
        CorrelationCause::MemoryPressure
        | CorrelationCause::ModelMemory
        | CorrelationCause::MetalMemory => Chart::Memory,
        CorrelationCause::Thermal | CorrelationCause::GpuSaturation => Chart::Gpu,
        CorrelationCause::DiskIo => Chart::Cache,
        // A stall names what the host was doing beside it.
        CorrelationCause::Stall if lead.contains("disk ") || lead.contains("SSD cache") => {
            Chart::Cache
        }
        CorrelationCause::Stall if lead.contains("paging") => Chart::Paging,
        CorrelationCause::Stall if lead.contains("memory pressure") => Chart::Memory,
        CorrelationCause::ConcurrentPrefill => Chart::Prefill,
        CorrelationCause::Concurrency | CorrelationCause::Queueing => Chart::Queue,
        CorrelationCause::ContextGrowth => Chart::Prompt,
        CorrelationCause::Stall | CorrelationCause::Runtime | CorrelationCause::None => {
            return None
        }
    })
}

/// What the linked panel is tagged with: the same ▼ the generation chart
/// puts before its reason.
pub(crate) fn tag(insight: &CorrelationInsight) -> &'static str {
    if insight.cause == CorrelationCause::Stall {
        " ▼ STALL "
    } else {
        " ▼ GEN "
    }
}

/// Marks the panel at `area` as the one behind the slowdown: its border in
/// the slowdown's colour and the tag at the right of its bottom edge.
pub(crate) fn mark(frame: &mut Frame, area: Rect, insight: &CorrelationInsight) {
    let tag = tag(insight);
    let width = Line::from(tag).width() as u16;
    if area.height < 2 || area.width < width + 4 {
        return;
    }
    let color = insight.tone().color();
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
        tag,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    );
}
