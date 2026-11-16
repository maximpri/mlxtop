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

fn gib_pair(used: u64, total: u64) -> String {
    format!(
        "{:.1}/{:.1} GiB",
        used as f64 / (1024.0 * 1024.0 * 1024.0),
        total as f64 / (1024.0 * 1024.0 * 1024.0)
    )
}

/// VRAM used of total; for a GPU without memory of its own, the RAM it
/// holds of the most it may hold.
pub(crate) fn memory_label(device: &gpu::Device) -> String {
    if device.unified {
        return match (device.limits.ram_used, device.limits.ram_limit) {
            (Some(used), Some(limit)) => gib_pair(used, limit),
            (Some(used), None) => bytes(used),
            (None, _) => "—".into(),
        };
    }
    match (device.used, device.total) {
        (Some(used), Some(total)) => gib_pair(used, total),
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
        if device.busy() {
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

/// The table's columns, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    Card,
    Name,
    Utilization,
    State,
    MemoryLoad,
    Memory,
    Power,
    Clock,
    MemoryBusy,
    Link,
    Temperature,
}

/// Utilization, memory and temperature at every width; the load state from
/// 100 columns of table and a memory bar from 120; power from 110, clock and
/// memory activity from 150 and the PCIe link from 180, each only when a
/// card reports it, so no column is all dashes.
fn plan(width: u16, devices: &[gpu::Device]) -> Vec<Column> {
    let reports = |reading: fn(&gpu::Device) -> bool| devices.iter().any(reading);
    let mut columns = vec![Column::Card, Column::Name, Column::Utilization];
    if width >= 100 {
        columns.push(Column::State);
    }
    // RAM held has a bar only against a limit (an APU's GTT); DGX Spark's
    // limit is all of RAM, which the memory panel already shows.
    if width >= 120
        && (!devices.iter().all(|gpu| gpu.unified) || reports(|gpu| gpu.ram_percent().is_some()))
    {
        columns.push(Column::MemoryLoad);
    }
    columns.push(Column::Memory);
    if width >= 110 && reports(|gpu| gpu.limits.power_mw.is_some()) {
        columns.push(Column::Power);
    }
    if width >= 150 && reports(|gpu| gpu.limits.clock_mhz.is_some()) {
        columns.push(Column::Clock);
    }
    if width >= 150 && reports(|gpu| gpu.limits.memory_busy.is_some()) {
        columns.push(Column::MemoryBusy);
    }
    // An integrated GPU has no slot.
    if width >= 180
        && reports(|gpu| {
            !gpu.unified
                && (gpu.limits.link_gen_max.is_some() || gpu.limits.link_width_max.is_some())
        })
    {
        columns.push(Column::Link);
    }
    columns.push(Column::Temperature);
    columns
}

fn constraint(column: Column, width: u16) -> Constraint {
    match column {
        Column::Card => Constraint::Length(7),
        Column::Name => Constraint::Fill(if width >= 120 { 3 } else { 2 }),
        Column::Utilization if width >= 100 => Constraint::Fill(2),
        Column::Utilization => Constraint::Length(17),
        Column::State => Constraint::Length(13),
        Column::MemoryLoad => Constraint::Fill(2),
        Column::Memory => Constraint::Length(21),
        Column::Power => Constraint::Length(12),
        Column::Clock => Constraint::Length(15),
        Column::MemoryBusy => Constraint::Length(10),
        Column::Link => Constraint::Length(10),
        Column::Temperature => Constraint::Length(7),
    }
}

/// What the memory columns hold: a card's own VRAM, or the RAM a GPU
/// without memory of its own holds, against a limit or not.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Held {
    Vram,
    Ram,
    RamOfLimit,
}

impl Held {
    fn of(devices: &[gpu::Device]) -> Self {
        if !devices.iter().all(|gpu| gpu.unified) {
            Self::Vram
        } else if devices.iter().any(|gpu| gpu.limits.ram_limit.is_some()) {
            Self::RamOfLimit
        } else {
            Self::Ram
        }
    }
}

fn label(column: Column, held: Held) -> &'static str {
    match column {
        Column::Card => "GPU",
        Column::Name => "DEVICE",
        Column::Utilization => "UTILIZATION",
        Column::State => "STATE",
        Column::MemoryLoad if held == Held::Vram => "VRAM LOAD",
        Column::MemoryLoad => "RAM LOAD",
        Column::Memory => match held {
            Held::Vram => "VRAM USED / TOTAL",
            Held::RamOfLimit => "RAM USED / LIMIT",
            Held::Ram => "RAM USED",
        },
        Column::Power => "POWER",
        Column::Clock => "CLOCK",
        Column::MemoryBusy => "MEM BUSY",
        Column::Link => "PCIE",
        Column::Temperature => "TEMP",
    }
}

fn columns(area: Rect, plan: &[Column]) -> Vec<Rect> {
    let widths: Vec<Constraint> = plan
        .iter()
        .map(|column| constraint(*column, area.width))
        .collect();
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
        format!(" device-wide · {} ", gpu::source(devices))
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
                    "{} · {} {} ",
                    gpu::vendors(devices),
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
    let plan = plan(header.width, devices);
    let held = Held::of(devices);
    for (area, column) in columns(header, &plan).into_iter().zip(&plan) {
        text(frame, area, label(*column, held), MUTED);
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
        let load_color = device
            .utilization
            .map(|n| ChartMetric::Gpu.tone(u64::from(n), thresholds).color())
            .unwrap_or(MUTED);
        // Memory occupancy is capacity information, not GPU compute load; it
        // turns yellow only where the next allocation may spill or fail. A
        // GPU's share of RAM is no finding at all.
        let full = device
            .memory_percent()
            .is_some_and(|percent| percent >= VRAM_FULL_ENTER);
        let limits = &device.limits;
        for (cell, column) in columns(row_area, &plan).into_iter().zip(&plan) {
            match column {
                Column::Card => text(
                    frame,
                    cell,
                    format!("{} {}", if selected { "▸" } else { " " }, device.index),
                    if selected { CYAN } else { MUTED },
                ),
                Column::Name => text(
                    frame,
                    cell,
                    compact_label(&device.name, cell.width.saturating_sub(2) as usize),
                    Color::White,
                ),
                Column::Utilization => {
                    meter(frame, cell, device.utilization.map(u16::from), load_color)
                }
                Column::State => {
                    let fault =
                        device.throttle().is_some_and(|cause| cause.is_fault()) && device.busy();
                    let color = if fault { YELLOW } else { load_color };
                    text(frame, cell, state(device, thresholds), color);
                }
                Column::MemoryLoad if device.unified => {
                    meter(frame, cell, device.ram_percent(), BLUE);
                }
                Column::MemoryLoad => meter(
                    frame,
                    cell,
                    device.memory_percent(),
                    if full { YELLOW } else { BLUE },
                ),
                Column::Memory => text(
                    frame,
                    cell,
                    memory_label(device),
                    if full { YELLOW } else { Color::White },
                ),
                Column::Power => text(
                    frame,
                    cell,
                    gpu::power_label(limits.power_mw, limits.power_cap_mw),
                    MUTED,
                ),
                Column::Clock => text(
                    frame,
                    cell,
                    gpu::clock_label(limits.clock_mhz, limits.clock_max_mhz),
                    MUTED,
                ),
                Column::MemoryBusy => text(
                    frame,
                    cell,
                    limits
                        .memory_busy
                        .map(|busy| format!("{busy}%"))
                        .unwrap_or_else(|| "—".into()),
                    MUTED,
                ),
                // What the card and its slot allow: an idle card slows its
                // link, so the current one is told only when it costs.
                Column::Link => text(
                    frame,
                    cell,
                    gpu::link_label(limits.link_gen_max, limits.link_width_max),
                    MUTED,
                ),
                Column::Temperature => text(
                    frame,
                    cell,
                    device
                        .temperature
                        .map(|n| format!("{n}°C"))
                        .unwrap_or_else(|| "—".into()),
                    MUTED,
                ),
            }
        }
    }
}

/// One line per card for the static report: identity, every reading the
/// card gave, and its state. Readings it does not report are left out.
pub(crate) fn static_lines(devices: &[gpu::Device], thresholds: Thresholds) -> Vec<String> {
    devices
        .iter()
        .map(|device| {
            let limits = &device.limits;
            let mut fields = vec![
                device.name.clone(),
                device.uuid.clone(),
                format!(
                    "utilization {}",
                    device
                        .utilization
                        .map(|n| format!("{n}%"))
                        .unwrap_or_else(|| "—".into())
                ),
                format!(
                    "{} {}",
                    if device.unified { "RAM" } else { "VRAM" },
                    memory_label(device)
                ),
                format!(
                    "temperature {}",
                    device
                        .temperature
                        .map(|n| format!("{n}°C"))
                        .unwrap_or_else(|| "—".into())
                ),
            ];
            if limits.power_mw.is_some() {
                fields.push(format!(
                    "power {}",
                    gpu::power_label(limits.power_mw, limits.power_cap_mw)
                ));
            }
            if limits.clock_mhz.is_some() {
                fields.push(format!(
                    "clock {}",
                    gpu::clock_label(limits.clock_mhz, limits.clock_max_mhz)
                ));
            }
            if let Some(busy) = limits.memory_busy {
                fields.push(format!("memory busy {busy}%"));
            }
            let now = (limits.link_gen, limits.link_width);
            let max = (limits.link_gen_max, limits.link_width_max);
            if !device.unified && (now != (None, None) || max != (None, None)) {
                fields.push(if now == max || now == (None, None) {
                    format!("PCIe {}", gpu::link_label(max.0, max.1))
                } else {
                    format!(
                        "PCIe {} of {}",
                        gpu::link_label(now.0, now.1),
                        gpu::link_label(max.0, max.1)
                    )
                });
            }
            if let (false, Some(used)) = (device.unified, limits.ram_used) {
                fields.push(format!(
                    "GTT {}",
                    match limits.ram_limit {
                        Some(limit) => gib_pair(used, limit),
                        None => bytes(used),
                    }
                ));
            }
            fields.push(state(device, thresholds).into());
            format!("GPU {:<8}{}", device.index, fields.join(" · "))
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/gpu_dashboard.rs"]
mod tests;
