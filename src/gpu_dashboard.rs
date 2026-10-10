// SPDX-License-Identifier: MIT
//! Responsive per-card comparison in Overview, with no aggregate VRAM pool.
use crate::config::Thresholds;
use crate::domain::ChartMetric;
use crate::formatting::{bytes, compact_label};
use crate::gpu;
use crate::gpu_findings::VRAM_FULL_ENTER;
use crate::theme::{BLUE, CYAN, DIM, MUTED, PANEL, PANEL_RAISED, YELLOW};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::ops::Range;

pub(crate) fn height(count: usize, available: u16) -> u16 {
    // Border + column labels + at most eight devices. The caller reserves
    // room for serving status and request history before giving us a budget.
    (count.min(8) as u16 + 3).min(available)
}

fn visible(count: usize, rows: usize, selected: usize) -> Range<usize> {
    if count == 0 || rows == 0 {
        return 0..0;
    }
    let start = selected.min(count - 1) / rows * rows;
    start..(start + rows).min(count)
}

pub(crate) fn memory_label(device: &gpu::Device) -> String {
    match (device.used, device.total) {
        (Some(used), Some(total)) => format!(
            "{:.1}/{:.1} GiB",
            used as f64 / (1024.0 * 1024.0 * 1024.0),
            total as f64 / (1024.0 * 1024.0 * 1024.0)
        ),
        (used, total) => format!(
            "{} / {}",
            used.map(bytes).unwrap_or_else(|| "—".into()),
            total.map(bytes).unwrap_or_else(|| "—".into())
        ),
    }
}

/// A clock limit names the state while the card is working; otherwise load.
fn state(device: &gpu::Device, thresholds: Thresholds) -> &'static str {
    if let Some(cause) = device.throttle() {
        if device.utilization.is_some_and(|load| load >= 20) {
            return cause.short_label();
        }
    }
    match device.utilization.map(u64::from) {
        None => "unavailable",
        Some(0) => "idle",
        Some(load) if load >= thresholds.gpu_critical_load => "saturated",
        Some(load) if load >= thresholds.gpu_warn_load => "busy",
        Some(_) => "active",
    }
}

fn columns(area: Rect) -> Vec<Rect> {
    let mut widths = vec![
        Constraint::Length(7),
        Constraint::Fill(if area.width >= 120 { 3 } else { 2 }),
        if area.width >= 100 {
            Constraint::Fill(2)
        } else {
            Constraint::Length(17)
        },
    ];
    if area.width >= 100 {
        widths.push(Constraint::Length(13)); // semantic load state
    }
    if area.width >= 120 {
        widths.push(Constraint::Fill(2)); // additional VRAM bar
    }
    widths.extend([Constraint::Length(21), Constraint::Length(7)]);
    Layout::horizontal(widths).split(area).to_vec()
}

fn text(frame: &mut Frame, area: Rect, value: impl Into<String>, color: Color) {
    frame.render_widget(
        Paragraph::new(value.into()).style(Style::default().fg(color)),
        area,
    );
}

fn meter(frame: &mut Frame, area: Rect, percent: Option<u16>, color: Color) {
    let bar_width = area.width.saturating_sub(6);
    if let Some(percent) = percent {
        frame.render_widget(
            Block::default().style(Style::default().bg(DIM)),
            Rect::new(area.x, area.y, bar_width, 1),
        );
        let filled = (u32::from(bar_width) * u32::from(percent.min(100))).div_ceil(100) as u16;
        frame.render_widget(
            Block::default().style(Style::default().bg(color)),
            Rect::new(area.x, area.y, filled, 1),
        );
    } else {
        text(frame, Rect::new(area.x, area.y, bar_width, 1), "—", MUTED);
    }
    frame.render_widget(
        Paragraph::new(
            percent
                .map(|n| format!("{n}%"))
                .unwrap_or_else(|| "—".into()),
        )
        .alignment(Alignment::Right)
        .style(Style::default().fg(if percent.is_some() {
            Color::White
        } else {
            MUTED
        })),
        Rect::new(area.x + bar_width, area.y, 5.min(area.width), 1),
    );
}

pub(crate) fn draw(
    frame: &mut Frame,
    area: Rect,
    devices: &[gpu::Device],
    selected: usize,
    thresholds: Thresholds,
) {
    if area.width < 4 || area.height < 4 || devices.is_empty() {
        return;
    }
    let range = visible(
        devices.len(),
        area.height.saturating_sub(3) as usize,
        selected,
    );
    let footer = if range.len() < devices.len() {
        format!(
            " device-wide · cards {}–{} / {} · [ / ] select GPU ",
            range.start + 1,
            range.end,
            devices.len()
        )
    } else if devices.len() > 1 {
        " device-wide · VRAM belongs to each card · [ / ] select GPU ".into()
    } else {
        " device-wide · nvidia-smi ".into()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .style(Style::default().bg(PANEL))
        .title(Line::from(vec![
            Span::styled(
                " GPU DEVICES ",
                Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "NVIDIA · {} {} ",
                    devices.len(),
                    if devices.len() == 1 { "card" } else { "cards" }
                ),
                Style::default().fg(MUTED),
            ),
        ]))
        .title_bottom(Line::from(Span::styled(footer, Style::default().fg(MUTED))));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let header = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1);
    let cols = columns(header);
    let mut labels = vec!["GPU", "DEVICE", "UTILIZATION"];
    if header.width >= 100 {
        labels.push("STATE");
    }
    if header.width >= 120 {
        labels.push("VRAM LOAD");
    }
    labels.extend(["VRAM USED / TOTAL", "TEMP"]);
    for (column, label) in cols.iter().zip(labels) {
        text(frame, *column, label, MUTED);
    }
    for (row, i) in range.enumerate() {
        let device = &devices[i];
        let row_area = Rect::new(header.x, header.y + 1 + row as u16, header.width, 1);
        let selected = i == selected.min(devices.len() - 1);
        frame.render_widget(
            Block::default().style(Style::default().bg(if selected {
                PANEL_RAISED
            } else {
                PANEL
            })),
            row_area,
        );
        let cols = columns(row_area);
        let load_color = device
            .utilization
            .map(|n| ChartMetric::Gpu.tone(u64::from(n), thresholds).color())
            .unwrap_or(MUTED);
        text(
            frame,
            cols[0],
            format!("{} {}", if selected { "▸" } else { " " }, device.index),
            if selected { CYAN } else { MUTED },
        );
        text(
            frame,
            cols[1],
            compact_label(&device.name, cols[1].width.saturating_sub(2) as usize),
            Color::White,
        );
        meter(
            frame,
            cols[2],
            device.utilization.map(u16::from),
            load_color,
        );
        let mut next = 3;
        if header.width >= 100 {
            let fault = device.throttle().is_some_and(|cause| cause.is_fault())
                && device.utilization.is_some_and(|load| load >= 20);
            let color = if fault { YELLOW } else { load_color };
            text(frame, cols[next], state(device, thresholds), color);
            next += 1;
        }
        // Memory occupancy is capacity information, not GPU compute load; it
        // turns yellow only where the next allocation may spill or fail.
        let full = device
            .memory_percent()
            .is_some_and(|percent| percent >= VRAM_FULL_ENTER);
        if header.width >= 120 {
            meter(
                frame,
                cols[next],
                device.memory_percent(),
                if full { YELLOW } else { BLUE },
            );
            next += 1;
        }
        text(
            frame,
            cols[next],
            memory_label(device),
            if full { YELLOW } else { Color::White },
        );
        text(
            frame,
            cols[next + 1],
            device
                .temperature
                .map(|n| format!("{n}°C"))
                .unwrap_or_else(|| "—".into()),
            MUTED,
        );
    }
}

pub(crate) fn static_lines(devices: &[gpu::Device], thresholds: Thresholds) -> Vec<String> {
    devices
        .iter()
        .map(|device| {
            format!(
                "GPU {:<8}{} · {} · utilization {} · VRAM {} · temperature {} · {}",
                device.index,
                device.name,
                device.uuid,
                device
                    .utilization
                    .map(|n| format!("{n}%"))
                    .unwrap_or_else(|| "—".into()),
                memory_label(device),
                device
                    .temperature
                    .map(|n| format!("{n}°C"))
                    .unwrap_or_else(|| "—".into()),
                state(device, thresholds)
            )
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/gpu_dashboard.rs"]
mod tests;
