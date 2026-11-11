// SPDX-License-Identifier: MIT
//! The Processes view: the runtime's processes and what each holds.
use crate::app::App;
use crate::chart_render::meter_cells;
use crate::domain::Tone;
use crate::formatting::{bytes, telemetry_source};
use crate::process_memory;
use crate::processes::process_provider;
use crate::theme::{panel, CYAN, EDGE, MUTED};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

impl App {
    pub(crate) fn draw_llm_top(&self, frame: &mut Frame, area: Rect) {
        let details = self.collector.current.llm_details.as_deref();
        // The table title carries the totals; a panel above it only when the
        // runtime reports engine details worth reading.
        let rows = Layout::vertical([
            Constraint::Length(if details.is_some() { 3 } else { 0 }),
            Constraint::Min(6),
            Constraint::Length(7),
        ])
        .split(area);
        let sample = &self.collector.current;
        let filtered = self.filtered_llm_processes();
        let total_rss = filtered.iter().map(|p| p.rss).sum::<u64>();
        let total_cpu = filtered.iter().map(|p| p.cpu).sum::<f64>();
        if let Some(text) = details {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(" {text}"),
                    Style::default().fg(CYAN),
                )))
                .block(panel(
                    &format!("{} ENGINE", sample.llm_provider),
                    Tone::Cyan,
                )),
                rows[0],
            );
        }
        let selected = self.top_selected.min(filtered.len().saturating_sub(1));
        let visible = rows[1].height.saturating_sub(3).max(1) as usize;
        let start = selected
            .saturating_sub(visible.saturating_sub(1))
            .min(filtered.len().saturating_sub(visible));
        let table_mode = if rows[1].width >= 150 {
            2
        } else if rows[1].width >= 95 {
            1
        } else {
            0
        };
        let (headers, widths): (Vec<&str>, Vec<Constraint>) = match table_mode {
            2 => (
                vec![
                    "PID", "PROCESS", "CPU", "MEM%", "RSS", "PAGEIN/s", "OS STATE", "PROVIDER",
                    "COMMAND",
                ],
                vec![
                    Constraint::Length(8),
                    Constraint::Length(20),
                    Constraint::Length(8),
                    Constraint::Length(10),
                    Constraint::Length(13),
                    Constraint::Length(11),
                    Constraint::Length(9),
                    Constraint::Length(12),
                    Constraint::Min(20),
                ],
            ),
            1 => (
                vec![
                    "PID", "PROCESS", "CPU", "RSS", "PAGEIN/s", "OS STATE", "PROVIDER",
                ],
                vec![
                    Constraint::Length(7),
                    Constraint::Min(20),
                    Constraint::Length(8),
                    Constraint::Length(12),
                    Constraint::Length(10),
                    Constraint::Length(9),
                    Constraint::Length(12),
                ],
            ),
            _ => (
                vec!["PID", "PROCESS", "CPU", "RSS", "OS STATE"],
                vec![
                    Constraint::Length(7),
                    Constraint::Min(22),
                    Constraint::Length(8),
                    Constraint::Length(12),
                    Constraint::Length(9),
                ],
            ),
        };
        let table_rows = filtered
            .iter()
            .skip(start)
            .take(visible)
            .map(|process| {
                let pagein = process
                    .pagein_rate
                    .map(|value| format!("{value:.1}"))
                    .unwrap_or_else(|| "—".into());
                let state = if process.state == "?" {
                    "—"
                } else {
                    &process.state
                };
                let provider =
                    process_provider(&process.name, &process.command).unwrap_or_else(|| "—".into());
                let mut cells = vec![
                    Cell::from(process.pid.to_string()),
                    Cell::from(process.name.clone()),
                    Cell::from(format!("{:.1}%", process.cpu)),
                ];
                if table_mode == 2 {
                    // A three-cell meter on a dim track beside the share of RAM.
                    cells.push(Cell::from(match process.memory_percent {
                        Some(value) => Line::from(vec![
                            Span::styled(meter_cells(value, 3), Style::default().fg(CYAN).bg(EDGE)),
                            Span::raw(format!(" {value:.1}%")),
                        ]),
                        None => Line::from("—"),
                    }));
                }
                cells.push(Cell::from(bytes(process.rss)));
                if table_mode > 0 {
                    cells.push(Cell::from(pagein));
                }
                cells.push(Cell::from(state.to_owned()));
                if table_mode > 0 {
                    cells.push(Cell::from(provider));
                }
                if table_mode == 2 {
                    cells.push(Cell::from(process.command.clone()));
                }
                Row::new(cells)
            })
            .collect::<Vec<_>>();
        let mut title = format!(
            "LLM PROCESSES · {} · RSS {} · CPU {:.1}% · sort {}",
            filtered.len(),
            bytes(total_rss),
            total_cpu,
            self.top_sort.label()
        );
        if filtered.len() > visible {
            title.push_str(&format!(
                " · {}–{} of {}",
                start + 1,
                (start + visible).min(filtered.len()),
                filtered.len()
            ));
        }
        if !self.top_filter.is_empty() || self.top_filtering {
            title.push_str(&format!(
                " · filter /{}{}",
                self.top_filter,
                if self.top_filtering {
                    "▏ · Enter/Esc finish"
                } else {
                    ""
                }
            ));
        }
        let table = Table::new(table_rows, widths)
            .header(
                Row::new(headers).style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
            )
            .row_highlight_style(
                Style::default()
                    .bg(Color::Rgb(35, 48, 67))
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ")
            .block(panel(&title, Tone::Blue));
        let mut table_state = TableState::default();
        if !filtered.is_empty() {
            table_state.select(Some(selected.saturating_sub(start)));
        }
        frame.render_stateful_widget(table, rows[1], &mut table_state);

        let Some(process) = filtered.get(selected) else {
            let message = if self.top_filter.is_empty() {
                "No local LLM processes detected. Provider telemetry can still be available in Overview."
            } else {
                "No processes match this filter. Press c to clear it or / to edit."
            };
            frame.render_widget(
                Paragraph::new(message)
                    .wrap(Wrap { trim: true })
                    .block(panel("SELECTED PROCESS", Tone::Muted)),
                rows[2],
            );
            return;
        };
        let provider = process_provider(&process.name, &process.command);
        // The wide table already shows these; narrower ones drop some columns.
        let mut detail = Vec::new();
        if table_mode < 2 {
            detail.push(Line::from(format!(
                "CPU {:.1}% · RSS {} · RAM {} · OS {} · PAGEIN {} pages/s",
                process.cpu,
                bytes(process.rss),
                process
                    .memory_percent
                    .map(|v| format!("{v:.1}%"))
                    .unwrap_or_else(|| "—".into()),
                process.state,
                process
                    .pagein_rate
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "—".into())
            )));
        }
        if sample
            .process_memory
            .as_ref()
            .is_some_and(|reading| reading.pid == process.pid)
        {
            detail.push(Line::from(format!(
                "OS footprint {} · {}",
                bytes(sample.process_memory.as_ref().unwrap().footprint),
                process_memory::detail(sample)
            )));
        }
        // Provider telemetry is runtime-wide; never attribute it to every PID.
        if provider.as_deref() == Some(sample.llm_provider.as_str()) {
            detail.push(Line::from(Span::styled(
                format!(
                    "RUNTIME {} · {} · {} · {}",
                    sample.llm_provider,
                    sample.llm_model,
                    sample.llm_status.to_uppercase(),
                    telemetry_source(sample)
                ),
                Style::default().fg(CYAN),
            )));
        } else {
            detail.push(Line::from(Span::styled(
                format!(
                    "RUNTIME {} · model/state unavailable",
                    provider.as_deref().unwrap_or("unknown")
                ),
                Style::default().fg(MUTED),
            )));
        }
        detail.push(Line::from(format!("COMMAND {}", process.command)));
        frame.render_widget(
            Paragraph::new(detail)
                .wrap(Wrap { trim: true })
                .block(panel(
                    &format!("SELECTED PROCESS · PID {} · {}", process.pid, process.name),
                    Tone::Cyan,
                )),
            rows[2],
        );
    }
}
