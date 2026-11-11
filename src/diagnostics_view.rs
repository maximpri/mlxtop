// SPDX-License-Identifier: MIT
//! Compact assessment and scrollable read-only diagnostics over existing samples.
use crate::app::App;
use crate::diagnosis;
use crate::domain::{Sample, Tone};
use crate::report::diagnostic_lines;
use crate::theme::{CYAN, MUTED, PANEL};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

pub(crate) fn summary(frame: &mut Frame, area: Rect, sample: &Sample, paused: bool) {
    let finding = diagnosis::assess(sample);
    // An icon carries the verdict's severity before any words do.
    let icon = match finding.tone {
        Tone::Green => '✓',
        Tone::Yellow => '▲',
        Tone::Red => '✖',
        _ => '●',
    };
    let title = format!(
        " {icon} {}{}",
        if paused { "PAUSED · " } else { "" },
        finding.title
    );
    let mut headline = Line::from(Span::styled(
        title,
        Style::default()
            .fg(finding.tone.color())
            .add_modifier(Modifier::BOLD),
    ));
    // A healthy verdict needs no supporting numbers; the panels show them.
    let evidence = format!(" · {}", finding.evidence);
    if finding.tone != Tone::Green
        && !finding.evidence.is_empty()
        && headline.width() + Line::from(evidence.as_str()).width() <= usize::from(area.width)
    {
        headline
            .spans
            .push(Span::styled(evidence, Style::default().fg(MUTED)));
    }
    let mut lines = vec![headline];
    if !finding.next.is_empty() {
        // Advice that does not fit ends at a word, marked, never mid-word.
        let room = usize::from(area.width).saturating_sub(3);
        let next = fit_words(&finding.next, room);
        lines.push(if finding.actionable {
            Line::from(Span::styled(
                format!(" → {next}"),
                Style::default().fg(Color::White),
            ))
        } else {
            Line::from(Span::styled(
                format!("   {next}"),
                Style::default().fg(MUTED),
            ))
        });
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// `text` as it fits in `room` cells: whole, or cut at a word with `…`.
pub(crate) fn fit_words(text: &str, room: usize) -> String {
    if Line::from(text).width() <= room {
        return text.to_owned();
    }
    let mut fitted = String::new();
    for word in text.split_whitespace() {
        let next = if fitted.is_empty() {
            word.to_owned()
        } else {
            format!("{fitted} {word}")
        };
        if Line::from(next.as_str()).width() + 1 > room {
            break;
        }
        fitted = next;
    }
    format!("{}…", fitted.trim_end_matches([',', ':', ';', '·', ' ']))
}

/// Wrap once so rendering and scroll bounds use identical terminal-cell widths.
fn wrapped(text: &[String], width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut result = Vec::new();
    for line in text {
        let start = result.len();
        if line.is_empty() {
            result.push(Line::default());
            continue;
        }
        let mut current = String::new();
        for word in line.split_whitespace() {
            let additional = Line::from(word).width() + usize::from(!current.is_empty());
            if !current.is_empty() && Line::from(current.as_str()).width() + additional > width {
                result.push(Line::from(std::mem::take(&mut current)));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            for ch in word.chars().filter(|ch| !ch.is_control()) {
                if Line::from(current.as_str()).width() + Line::from(ch.to_string()).width() > width
                {
                    result.push(Line::from(std::mem::take(&mut current)));
                }
                current.push(ch);
            }
        }
        result.push(Line::from(current));
        if matches!(line.as_str(), "ASSESSMENT" | "CONNECTION" | "SETUP")
            || line.starts_with("CAPABILITIES")
        {
            for row in &mut result[start..] {
                *row = row
                    .clone()
                    .style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD));
            }
        }
    }
    result
}

impl App {
    pub(crate) fn draw_diagnostics(&self, frame: &mut Frame, mut area: Rect) {
        let reserved = self.alert_rows().min(area.height);
        area.y += reserved;
        area.height -= reserved;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Diagnostics ")
            .border_style(Style::default().fg(CYAN))
            .style(Style::default().bg(PANEL));
        let inner = block.inner(area);
        let mut lines = diagnostic_lines(&self.collector.current);
        if self.paused {
            lines.insert(0, "PAUSED · retained observations; resume with p".into());
        }
        let lines = wrapped(&lines, usize::from(inner.width));
        let max = lines
            .len()
            .saturating_sub(usize::from(inner.height))
            .min(u16::MAX as usize) as u16;
        self.diagnostics_max_scroll.set(max);
        let scroll = self.diagnostics_scroll.min(max);
        frame.render_widget(Clear, area);
        let block = block.title_bottom(format!(
            " {}–{} / {} lines · Esc closes ",
            scroll + 1,
            (usize::from(scroll) + usize::from(inner.height)).min(lines.len()),
            lines.len()
        ));
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(lines)
                .scroll((scroll, 0))
                .style(Style::default().fg(MUTED)),
            inner,
        );
    }
}

#[cfg(test)]
#[path = "tests/diagnostics_view.rs"]
mod tests;
