// SPDX-License-Identifier: MIT
//! Swap occupancy is capacity, independent of paging traffic or pressure.
use crate::domain::Sample;
use crate::theme::{CYAN, EDGE, MUTED, PANEL};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Render a single row inside the paging panel, without an additional border.
pub(super) fn draw(frame: &mut Frame, area: Rect, sample: &Sample) {
    if area.is_empty() {
        return;
    }
    let row = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(Paragraph::default().style(Style::default().bg(PANEL)), row);
    if !sample.swap_available || sample.swap_total == 0 {
        // macOS creates swap files only when it needs them: none yet is
        // unused swap, not swap turned off.
        let state = if sample.swap_available {
            "swap 0 B used · no swap file yet"
        } else {
            "swap —"
        };
        frame.render_widget(Paragraph::new(state).style(Style::default().fg(MUTED)), row);
        return;
    }

    let label = format!(
        "{} {}%",
        capacity(sample.swap_used, sample.swap_total),
        share(sample.swap_used, sample.swap_total)
    );
    capacity_bar(
        frame,
        row,
        "swap",
        label,
        sample.swap_used,
        sample.swap_total,
    );
}

/// The runtime's SSD prompt cache: how full it is, in the same form as swap.
/// A full cache evicts old prefixes to save new ones.
pub(super) fn draw_ssd_cache(frame: &mut Frame, area: Rect, sample: &Sample) {
    if area.is_empty() {
        return;
    }
    let row = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(Paragraph::default().style(Style::default().bg(PANEL)), row);
    let Some(cache) = sample.ssd_cache.filter(|cache| cache.capacity > 0) else {
        frame.render_widget(
            Paragraph::new("SSD cache off").style(Style::default().fg(MUTED)),
            row,
        );
        return;
    };
    let fill = if cache.full() {
        "full".to_owned()
    } else {
        format!("{}%", share(cache.used, cache.capacity))
    };
    capacity_bar(
        frame,
        row,
        "SSD",
        format!("{} {fill}", capacity(cache.used, cache.capacity)),
        cache.used,
        cache.capacity,
    );
}

fn share(used: u64, total: u64) -> u128 {
    (u128::from(used) * 100 + u128::from(total) / 2) / u128::from(total)
}

fn capacity_bar(frame: &mut Frame, row: Rect, prefix: &str, label: String, used: u64, total: u64) {
    // Shared units preserve both exact capacity readings and a useful bar at 30 columns.
    let label_width = u16::try_from(label.len()).unwrap_or(u16::MAX);
    // The bar starts after the prefix and its space, and leaves one before
    // the label.
    let start = if prefix.is_empty() {
        0
    } else {
        u16::try_from(prefix.len() + 1).unwrap_or(u16::MAX)
    };
    let bar_width = row
        .width
        .saturating_sub(label_width.saturating_add(start + 1));
    if bar_width < 3 {
        frame.render_widget(
            Paragraph::new(format!("{prefix} {label}").trim_start().to_owned())
                .style(Style::default().fg(CYAN)),
            row,
        );
        return;
    }
    frame.render_widget(
        Paragraph::new(prefix.to_owned()).style(Style::default().fg(MUTED)),
        Rect::new(row.x, row.y, start, 1),
    );
    let filled_eighths =
        u128::from(used.min(total)) * u128::from(bar_width) * 8 / u128::from(total);
    const PARTIAL: [&str; 8] = [" ", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    for column in 0..bar_width {
        let cell = &mut frame.buffer_mut()[(row.x + start + column, row.y)];
        let eighths = filled_eighths.saturating_sub(u128::from(column) * 8).min(8);
        if eighths == 8 {
            cell.set_symbol(" ").set_bg(CYAN);
        } else {
            cell.set_symbol(PARTIAL[eighths as usize])
                .set_fg(CYAN)
                .set_bg(EDGE);
        }
    }
    frame.render_widget(
        Paragraph::new(label).style(Style::default().fg(CYAN)),
        Rect::new(row.x + start + bar_width + 1, row.y, label_width, 1),
    );
}

fn capacity(used: u64, total: u64) -> String {
    let largest = used.max(total);
    for (divisor, unit) in [
        (1024_u64.pow(3), "GiB"),
        (1024_u64.pow(2), "MiB"),
        (1024, "KiB"),
    ] {
        if largest >= divisor {
            return format!(
                "{:.1}/{:.1} {unit}",
                used as f64 / divisor as f64,
                total as f64 / divisor as f64
            );
        }
    }
    format!("{used}/{total} B")
}

#[cfg(test)]
#[path = "tests/swap_usage.rs"]
mod tests;
