// SPDX-License-Identifier: MIT
//! The Journal: the Overview's recent events and the full, filterable view.
use crate::app::App;
use crate::domain::Tone;
use crate::formatting::compact_label;
use crate::theme::{panel, CYAN, DIM, MUTED};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};
use ratatui::Frame;

impl App {
    pub(crate) fn draw_signal_log(&self, frame: &mut Frame, area: Rect) {
        let mut block = panel("RECENT JOURNAL", Tone::Muted).title(
            Line::from(Span::styled(" 3 full journal ", Style::default().fg(CYAN)))
                .alignment(Alignment::Right),
        );
        if !self.collector.signals.is_empty() {
            block = block.title_bottom(Line::from(Span::styled(
                format!(" {} events ", self.collector.signals.len()),
                Style::default().fg(MUTED),
            )));
        }
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.is_empty() {
            return;
        }
        if self.collector.signals.is_empty() {
            frame.render_widget(
                Paragraph::new("No events yet.")
                    .style(Style::default().fg(MUTED))
                    .wrap(Wrap { trim: true }),
                inner,
            );
            return;
        }
        let message_x = inner.x + 24.min(inner.width);
        let message_width = inner.right().saturating_sub(message_x);
        let mut y = inner.y;
        for event in self.collector.signals.iter().rev() {
            if y >= inner.bottom() || message_width == 0 {
                break;
            }
            let mut lines = Vec::new();
            let mut line = String::new();
            for word in event.summary.split_whitespace() {
                let next = if line.is_empty() {
                    word.to_owned()
                } else {
                    format!("{line} {word}")
                };
                if Line::from(next.as_str()).width() > usize::from(message_width)
                    && !line.is_empty()
                {
                    lines.push(std::mem::take(&mut line));
                    line = compact_label(word, usize::from(message_width));
                } else {
                    line = compact_label(&next, usize::from(message_width));
                }
            }
            if !line.is_empty() {
                lines.push(line);
            }
            if lines.is_empty() {
                lines.push(String::new());
            }
            let height = lines.len().min(2).min(usize::from(inner.bottom() - y));
            if lines.len() > height {
                lines[height - 1] = format!(
                    "{}…",
                    compact_label(
                        &lines[height - 1],
                        usize::from(message_width).saturating_sub(1)
                    )
                );
            }
            frame.render_widget(
                Paragraph::new(event.time.as_str()).style(Style::default().fg(MUTED)),
                Rect::new(inner.x, y, 9, 1),
            );
            frame.render_widget(
                Paragraph::new(compact_label(&event.state, 13)).style(
                    Style::default()
                        .fg(event.tone.color())
                        .add_modifier(Modifier::BOLD),
                ),
                Rect::new(inner.x + 10, y, 13, 1),
            );
            frame.render_widget(
                Paragraph::new(
                    lines
                        .into_iter()
                        .take(height)
                        .map(Line::from)
                        .collect::<Vec<_>>(),
                )
                .style(Style::default().fg(Color::White)),
                Rect::new(message_x, y, message_width, height as u16),
            );
            y += height as u16;
        }
    }
    pub(crate) fn draw_journal_events(&self, frame: &mut Frame, area: Rect) {
        let capacity = area.height.saturating_sub(2) as usize;
        let filtered_events = self.filtered_journal_events();
        let max_scroll = filtered_events.len().saturating_sub(capacity);
        let scroll = self.journal_scroll.min(max_scroll);
        let lines = filtered_events
            .iter()
            .rev()
            .skip(scroll)
            .take(capacity)
            .map(|event| {
                Line::from(vec![
                    Span::styled(format!(" {} ", event.time), Style::default().fg(DIM)),
                    Span::styled(
                        format!("{:<16}", compact_label(&event.state, 16)),
                        Style::default()
                            .fg(event.tone.color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  │  ", Style::default().fg(DIM)),
                    Span::styled(&event.summary, Style::default().fg(Color::White)),
                ])
            })
            .collect::<Vec<_>>();
        let text = if lines.is_empty() {
            Text::from(Line::from(Span::styled(
                " No events yet.",
                Style::default().fg(MUTED),
            )))
        } else {
            Text::from(lines)
        };
        let title = format!(
            "EVENTS · {} · {}–{} of {}",
            self.journal_filter.label(),
            if filtered_events.is_empty() {
                0
            } else {
                scroll + 1
            },
            (scroll + capacity).min(filtered_events.len()),
            filtered_events.len()
        );
        frame.render_widget(
            Paragraph::new(text)
                .block(panel(&title, Tone::Blue))
                .wrap(Wrap { trim: true }),
            area,
        );
        if filtered_events.len() > capacity {
            let mut scrollbar_state = ScrollbarState::new(filtered_events.len()).position(scroll);
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .thumb_style(Style::default().fg(CYAN))
                    .track_style(Style::default().fg(DIM)),
                area,
                &mut scrollbar_state,
            );
        }
    }
}
