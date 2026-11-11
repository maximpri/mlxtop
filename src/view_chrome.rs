// SPDX-License-Identifier: MIT
//! What frames every view: the key bar, help, and the session summary.
use crate::app::App;
use crate::cache_impact;
use crate::theme::{centered_rect, CYAN, MUTED, PANEL, PANEL_RAISED};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

impl App {
    pub(crate) fn draw_controls(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(
            Block::default().style(Style::default().bg(PANEL_RAISED)),
            area,
        );
        let help = Line::from(vec![
            Span::styled(
                " d",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if self.diagnostics_open {
                    " close "
                } else {
                    " diagnostics "
                },
                Style::default().fg(MUTED),
            ),
            Span::styled(
                " ?",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" help ", Style::default().fg(MUTED)),
            Span::styled(
                "q",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit ", Style::default().fg(MUTED)),
        ]);
        let available = usize::from(area.width).saturating_sub(help.width());
        let mut line = Line::from(Span::styled(
            if self.diagnostics_open {
                " Diagnostics ".into()
            } else if self.tab == 0 {
                format!(
                    " {} · {}× ",
                    self.charts.focused.label(),
                    self.charts.zoom(self.charts.focused)
                )
            } else if self.tab == 1 {
                " Processes ".into()
            } else if self.tab == 2 {
                " Journal ".into()
            } else {
                " Settings ".into()
            },
            Style::default().fg(CYAN),
        ));
        let hints = if self.diagnostics_open {
            vec![
                ("↑↓", "scroll"),
                ("PgUp/Dn", "page"),
                ("Tab", "view"),
                ("p", if self.paused { "resume" } else { "pause" }),
            ]
        } else {
            match self.tab {
                0 => vec![
                    (
                        "Enter",
                        if self.charts.expanded {
                            "restore"
                        } else {
                            "expand"
                        },
                    ),
                    ("↑↓←→", "chart"),
                    ("+/−", "zoom"),
                    ("Tab", "view"),
                    ("p", if self.paused { "resume" } else { "pause" }),
                    ("Shift+↑↓", "requests"),
                    ("{ / }", "interval"),
                ],
                1 => vec![
                    ("↑↓", "select"),
                    ("s", "sort"),
                    ("/", "filter"),
                    ("Tab", "view"),
                    ("p", "pause"),
                ],
                2 => vec![
                    ("↑↓", "scroll"),
                    ("f", "filter"),
                    ("Tab", "view"),
                    ("r", "reset"),
                    ("p", "pause"),
                ],
                _ if self.settings.editing.is_some() => {
                    vec![("Enter", "save"), ("Esc", "cancel")]
                }
                _ => vec![
                    ("↑↓", "select"),
                    ("Enter", "edit"),
                    ("x", "default"),
                    ("Tab", "view"),
                ],
            }
        };
        for (key, label) in hints {
            let key = Span::styled(
                format!(" {key}"),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            );
            let label = Span::styled(format!(" {label} "), Style::default().fg(MUTED));
            if line.width() + key.width() + label.width() <= available {
                line.spans.extend([key, label]);
            }
        }
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x, area.y, available as u16, 1),
        );
        frame.render_widget(
            Paragraph::new(help),
            Rect::new(
                area.x + available as u16,
                area.y,
                area.width - available as u16,
                1,
            ),
        );
    }
    /// The session's lines: what it amounted to, the change worth making first.
    pub(crate) fn summary_lines(&self) -> Vec<String> {
        let sample = &self.collector.current;
        let saved = sample.llm_watched_cached_tokens.and_then(|cached| {
            cache_impact::saved_seconds(
                cached,
                cache_impact::prefill_speed(&self.collector.prefill_history, sample),
            )
        });
        crate::session::lines(
            &self.collector.session,
            &self.collector.request_history,
            sample,
            saved,
        )
    }
    pub(crate) fn draw_summary(&self, frame: &mut Frame, area: Rect) {
        let popup = if area.width < 100 {
            centered_rect(96, 80, area)
        } else {
            centered_rect(70, 60, area)
        };
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Session summary ")
            .title_bottom(" any key closes ")
            .border_style(Style::default().fg(CYAN))
            .style(Style::default().bg(PANEL));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        frame.render_widget(
            Paragraph::new(
                self.summary_lines()
                    .into_iter()
                    .map(Line::from)
                    .collect::<Vec<_>>(),
            )
            .wrap(Wrap { trim: false }),
            inner,
        );
    }
    pub(crate) fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let popup = if area.width < 120 || area.height < 36 {
            centered_rect(94, 88, area)
        } else {
            centered_rect(60, 58, area)
        };
        frame.render_widget(Clear, popup);
        let mut text = vec![
            Line::from(Span::styled(
                "CONTROLS",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("1 / 2 / 3 / 4   Overview / Processes / Journal / Settings"),
            Line::from("d               diagnostics and runtime setup"),
            Line::from("u               session summary: time lost, where requests' time went"),
            Line::from("Tab / Shift-Tab next / previous view"),
            Line::from("p / Space       pause or resume"),
            Line::from("r               reset history, chart zoom and alarm"),
            Line::from("{ / }           change refresh interval (1–60s)"),
            Line::from("a               acknowledge critical system alarm"),
            Line::from("q / Ctrl-C      quit; Esc asks first · ? / h closes help"),
            Line::from(""),
        ];
        text.extend(match self.tab {
            0 => vec![
                Line::from("Arrow keys      select a neighboring chart"),
                Line::from("+ / - / wheel   zoom (time series linked); 0 reset"),
                Line::from("Enter / Esc     enlarge / restore chart"),
                Line::from("Mouse click     select chart; double / right-click enlarge"),
                Line::from("Shift-↑↓        newer / older prompt"),
                Line::from("Home / End      newest / oldest prompt"),
                Line::from("PgUp / PgDn     move by ten requests"),
                Line::from(if self.collector.current.has_discrete_gpus() {
                    "[ / ] GPUs      select previous / next GPU card"
                } else {
                    ""
                }),
            ],
            1 => vec![
                Line::from("↑↓ / PgUp/PgDn  select process / move ten rows"),
                Line::from("Home / End      first / last process"),
                Line::from("s               cycle RSS / CPU / PID / name sort"),
                Line::from("f or /          filter processes; c clears filter"),
            ],
            3 => vec![
                Line::from("↑↓ / Home / End select a setting"),
                Line::from("Enter           edit, or toggle on / off"),
                Line::from("x / Delete      reset to the default"),
                Line::from("Esc             cancel an edit"),
            ],
            _ => vec![
                Line::from("↑↓ / PgUp/PgDn  newer / older events"),
                Line::from("Home / End      newest / oldest event"),
                Line::from("f / [ / ]       cycle event filters"),
            ],
        });
        frame.render_widget(
            Paragraph::new(text).wrap(Wrap { trim: true }).block(
                Block::default()
                    .title(" HELP ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(CYAN))
                    .style(Style::default().bg(PANEL)),
            ),
            popup,
        );
    }
}
