// SPDX-License-Identifier: MIT
//! The shared terminal palette, semantic tones and panel primitives.
use crate::domain::Tone;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
pub(crate) const GREEN: Color = Color::Rgb(88, 211, 147);
/// Warnings: orange, so that yellow means impact alone.
pub(crate) const YELLOW: Color = Color::Rgb(251, 146, 60);
pub(crate) const RED: Color = Color::Rgb(248, 113, 113);
pub(crate) const CYAN: Color = Color::Rgb(90, 202, 225);
pub(crate) const BLUE: Color = Color::Rgb(120, 153, 255);
/// Reserved for impact: no grade or series uses it. A clear yellow; the
/// warning grade is orange beside it.
pub(crate) const UNUSUAL: Color = Color::Rgb(250, 230, 70);
pub(crate) const MUTED: Color = Color::Rgb(139, 151, 168);
pub(crate) const DIM: Color = Color::Rgb(78, 90, 108);
pub(crate) const PANEL: Color = Color::Rgb(18, 24, 34);
pub(crate) const PANEL_RAISED: Color = Color::Rgb(24, 32, 46);
pub(crate) const EDGE: Color = Color::Rgb(54, 68, 88);

impl Tone {
    pub(crate) fn color(self) -> Color {
        match self {
            Self::Green => GREEN,
            Self::Yellow => YELLOW,
            Self::Red => RED,
            Self::Cyan => CYAN,
            Self::Blue => BLUE,
            Self::Muted => MUTED,
            Self::Unusual => UNUSUAL,
        }
    }
}

pub(crate) fn tone_badge(tone: Tone, label: &str) -> Span<'static> {
    Span::styled(
        format!(" {label} "),
        Style::default()
            .fg(Color::Black)
            .bg(tone.color())
            .add_modifier(Modifier::BOLD),
    )
}

pub(crate) fn card_block<'a>(title: Line<'a>, tone: Tone) -> Block<'a> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(EDGE))
        .style(Style::default().bg(PANEL_RAISED).fg(tone.color()))
}

/// The body of an area beneath its line: one quiet slate for every chart.
/// The line carries the level and its grade; a graded fill would only repeat
/// it, and stripe wherever the grade changes from one sample to the next.
pub(crate) const AREA_FILL: Color = Color::Rgb(33, 42, 56);

pub(crate) fn llm_status_tone(status: &str) -> Tone {
    match status.to_ascii_lowercase().as_str() {
        "ready" | "idle" => Tone::Green,
        "waiting" | "busy" | "generating" => Tone::Cyan,
        "stale" => Tone::Yellow,
        "last result" | "offline" => Tone::Muted,
        "error" => Tone::Red,
        _ => Tone::Cyan,
    }
}

pub(crate) fn panel(title: &str, tone: Tone) -> Block<'static> {
    Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(tone.color())
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .style(Style::default().bg(PANEL))
}

pub(crate) fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
