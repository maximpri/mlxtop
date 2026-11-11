// SPDX-License-Identifier: MIT
//! Rendering for Overview, Top, Journal and their overlays.
use crate::app::App;
use crate::chart_navigation::Chart;
use crate::chart_render::{
    bar_cells, chart_axis_label, chart_columns, chart_columns_for_plot, chart_display_value,
    chart_form, chart_scale, chart_stat_label, chart_stats_for_width, chart_window_label,
    normalize_chart_value, stretch_chart_columns, trace_connector, trace_point, ChartForm,
    TraceCell, TraceStyle,
};
use crate::cli::VERSION;
use crate::domain::{ChartMetric, ChartPoint, Tone};
use crate::formatting::{activity_frame, telemetry_age};
use crate::host::Platform;
use crate::theme::{panel, AREA_FILL, CYAN, DIM, EDGE, MUTED, PANEL, RED, YELLOW};
use crate::{drop_marks, gpu_dashboard, model_dashboard, operator_charts, request_dashboard};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph, Tabs};
use ratatui::Frame;
use std::collections::VecDeque;

/// Throughput is the outcome, shown compactly; host resources get the space.
/// Every row scales with the terminal between a floor and a ceiling, so no
/// single window dwarfs the rest at any size.
const RATES_MAX_HEIGHT: u16 = 12;
/// Short terminals keep each rate in its title; Enter expands the history.
const RATES_COMPACT_HEIGHT: u16 = 4;
/// Prompt bars and Journal stop growing once more rows add no information.
const REQUEST_MAX_HEIGHT: u16 = 16;
const JOURNAL_MAX_HEIGHT: u16 = 14;
/// The host row takes all remaining height, never less than this.
const HOST_MIN_HEIGHT: u16 = 6;
/// SYSINFO strip, then the borderless two-row assessment beneath it.
const OVERVIEW_INFO_HEIGHT: u16 = 3;
const OVERVIEW_SUMMARY_HEIGHT: u16 = 2;

/// The shared Overview column grid: three equal columns.
fn overview_grid(area: Rect) -> [Rect; 3] {
    Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(area)
}

/// A bar cell the usual-speed line runs through: drawn as `┄` cut into the
/// bar's colour.
const USUAL_THROUGH_BAR: char = '╍';

impl App {
    pub(crate) fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        self.charts.regions.borrow_mut().clear();
        self.charts.overview_samples.set(None);
        if area.width < 72 || area.height < 24 {
            self.draw_compact_warning(frame, area);
            if self.quit_prompt {
                self.draw_quit_prompt(frame, area);
            }
            return;
        }
        frame.render_widget(
            Block::default().style(Style::default().bg(Color::Rgb(10, 14, 21))),
            area,
        );
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(5),
                Constraint::Length(1),
            ])
            .split(area);
        self.draw_controls(frame, outer[2]);
        match self.tab {
            0 => self.draw_overview(frame, outer[1]),
            1 => self.draw_llm_top(frame, outer[1]),
            2 => self.draw_journal(frame, outer[1]),
            _ => self.draw_settings(frame, outer[1]),
        }
        // After the content: the header names the window the Overview chose.
        self.draw_header(frame, outer[0]);
        if self.tab == 0 {
            if self.charts.expanded {
                frame.render_widget(Clear, outer[1]);
                self.charts.overview_samples.set(None);
                self.draw_selected_chart(frame, outer[1]);
            }
            self.charts.decorate(frame);
        }
        if self.alert.is_some() {
            if self.alert_uses_assessment_strip() {
                self.draw_alert_strip(frame, outer[1]);
            } else {
                self.draw_alert_banner(frame, outer[1]);
            }
        }
        if self.diagnostics_open {
            self.draw_diagnostics(frame, outer[1]);
        }
        if self.summary_open {
            self.draw_summary(frame, outer[1]);
        }
        if self.help {
            self.draw_help(frame, area);
        }
        if self.quit_prompt {
            self.draw_quit_prompt(frame, area);
        }
    }

    /// Esc with nothing open: quitting discards the session's charts, prompts and Journal.
    pub(crate) fn draw_quit_prompt(&self, frame: &mut Frame, area: Rect) {
        let width = area.width.min(44);
        let height = area.height.min(5);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let key = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);
        let text = vec![
            Line::from("Session history will be lost."),
            Line::from(""),
            Line::from(vec![
                Span::styled("y", key),
                Span::styled(" / ", Style::default().fg(MUTED)),
                Span::styled("q", key),
                Span::styled(" quit · any other key stays", Style::default().fg(MUTED)),
            ]),
        ];
        frame.render_widget(
            Paragraph::new(text).alignment(Alignment::Center).block(
                Block::default()
                    .title(" QUIT MLXTOP? ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(YELLOW))
                    .style(Style::default().bg(PANEL)),
            ),
            popup,
        );
    }

    /// Overview keeps SYSINFO (what is running, and how fresh) visible during an
    /// incident: the alarm replaces the assessment rows that would repeat it.
    pub(crate) fn alert_uses_assessment_strip(&self) -> bool {
        self.tab == 0 && !self.charts.expanded
    }

    /// Rows at the top of the tab content that an active alarm occupies.
    pub(crate) fn alert_rows(&self) -> u16 {
        match (&self.alert, self.alert_uses_assessment_strip()) {
            (None, _) => 0,
            (Some(_), true) => OVERVIEW_INFO_HEIGHT + OVERVIEW_SUMMARY_HEIGHT,
            (Some(_), false) => 3,
        }
    }

    pub(crate) fn draw_alert_strip(&self, frame: &mut Frame, area: Rect) {
        let Some(alert) = &self.alert else {
            return;
        };
        if area.height < OVERVIEW_INFO_HEIGHT + OVERVIEW_SUMMARY_HEIGHT || area.width < 20 {
            return;
        }
        let strip = Rect::new(
            area.x,
            area.y + OVERVIEW_INFO_HEIGHT,
            area.width,
            OVERVIEW_SUMMARY_HEIGHT,
        );
        frame.render_widget(Clear, strip);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!(" ⚠ {} ", alert.state),
                        Style::default().fg(RED).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        alert.summary.clone(),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    format!(" raised {}  ·  a acknowledge", alert.time),
                    Style::default().fg(MUTED),
                )),
            ])
            .style(Style::default().bg(PANEL)),
            strip,
        );
    }

    /// Overlay strip at the top of the tab content: a critical condition demands
    /// attention without stealing a permanent layout row from the panels.
    pub(crate) fn draw_alert_banner(&self, frame: &mut Frame, area: Rect) {
        let Some(alert) = &self.alert else {
            return;
        };
        let height = area.height.min(3);
        if height == 0 || area.width < 20 {
            return;
        }
        let banner = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height,
        };
        frame.render_widget(Clear, banner);
        let text = Line::from(vec![
            Span::styled(
                format!(" ⚠ {} ", alert.state),
                Style::default().fg(RED).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                alert.summary.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  ·  raised {}  ·  a acknowledge", alert.time),
                Style::default().fg(MUTED),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(RED))
                    .style(Style::default().bg(PANEL)),
            ),
            banner,
        );
    }

    pub(crate) fn draw_header(&self, frame: &mut Frame, area: Rect) {
        // Full view names fit from the 80-column minimum supported width.
        let compact_tabs = area.width < 80;
        let tab_labels = if compact_tabs {
            vec![
                Line::from("1 OVR"),
                Line::from("2 TOP"),
                Line::from("3 JRN"),
                Line::from("4 SET"),
            ]
        } else {
            vec![
                Line::from("1 Overview"),
                Line::from("2 Processes"),
                Line::from("3 Journal"),
                Line::from("4 Settings"),
            ]
        };
        // Tabs sit flush against their dividers so four names fit beside the
        // title and the heartbeat at 80 columns.
        let tabs_width = tab_labels.iter().map(Line::width).sum::<usize>() + 3 * 3;
        let title = Line::from(vec![
            Span::styled(
                " mlxtop ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("v{VERSION}"),
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            ),
        ]);
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(u16::try_from(title.width() + 1).unwrap_or(21)),
                // One cell keeps the last tab off the status.
                Constraint::Length(u16::try_from(tabs_width + 1).unwrap_or(50)),
                Constraint::Fill(1),
            ])
            .split(area);
        frame.render_widget(Paragraph::new(title), chunks[0]);

        let tabs = Tabs::new(tab_labels)
            .select(self.tab)
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(CYAN)
                    .add_modifier(Modifier::BOLD),
            )
            .style(Style::default().fg(MUTED))
            .padding("", "")
            .divider(Span::styled(" · ", Style::default().fg(DIM)));
        frame.render_widget(tabs, chunks[1]);
        // A heartbeat that turns with every sample, the refresh interval, and
        // the one time window every Overview chart shares.
        let mut status = if self.paused {
            vec![Span::styled(
                "PAUSED",
                Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
            )]
        } else {
            vec![Span::styled(
                activity_frame(&self.collector.current).to_string(),
                Style::default().fg(CYAN),
            )]
        };
        status.push(Span::styled(
            format!(" every {}s", self.interval.as_secs()),
            Style::default().fg(MUTED),
        ));
        if let (0, false, Some(samples)) = (
            self.tab,
            self.charts.expanded,
            self.charts.overview_samples.get(),
        ) {
            let window = Span::styled(
                format!(" · {}", chart_window_label(samples, self.interval)),
                Style::default().fg(MUTED),
            );
            // The window goes first when space is short; the heartbeat stays.
            let used: usize = status.iter().map(Span::width).sum();
            if used + window.width() < usize::from(chunks[2].width) {
                status.push(window);
            }
        }
        status.push(Span::raw(" "));
        frame.render_widget(
            Paragraph::new(Line::from(status)).alignment(Alignment::Right),
            chunks[2],
        );
    }

    pub(crate) fn draw_request_chart(&self, frame: &mut Frame, area: Rect) {
        self.charts.register(Chart::Prompt, area);
        request_dashboard::draw(
            frame,
            area,
            &self.collector.request_history,
            &self.collector.current,
            self.request_scroll,
            self.charts.zoom(Chart::Prompt),
        );
        self.mark_if_linked(frame, area, Chart::Prompt);
    }

    /// Tags the panel a slowdown is attributed to, in the slowdown's colour.
    pub(crate) fn mark_if_linked(&self, frame: &mut Frame, area: Rect, chart: Chart) {
        let insight = &self.collector.current.correlation;
        if drop_marks::linked(insight) == Some(chart) {
            drop_marks::mark(frame, area, insight);
        }
    }

    pub(crate) fn draw_operator_chart(&self, frame: &mut Frame, area: Rect, chart: Chart) {
        self.charts.register(chart, area);
        let zoom = self.charts.zoom(chart);
        match chart {
            Chart::Queue => operator_charts::queue(
                frame,
                area,
                &self.collector.operator_history,
                self.interval,
                zoom,
                self.charts.overview_samples.get(),
            ),
            Chart::Latency => {
                operator_charts::latency(frame, area, &self.collector.operator_history, zoom)
            }
            _ => unreachable!("operator chart expected"),
        }
        self.mark_if_linked(frame, area, chart);
    }

    pub(crate) fn draw_selected_chart(&self, frame: &mut Frame, area: Rect) {
        match self.charts.focused {
            Chart::Prompt => self.draw_request_chart(frame, area),
            chart @ (Chart::Queue | Chart::Latency) => self.draw_operator_chart(frame, area, chart),
            chart => {
                let (name, history, metric) = match chart {
                    Chart::Generation => (
                        "generation",
                        &self.collector.generation_history,
                        ChartMetric::Generation,
                    ),
                    Chart::Prefill => (
                        "prefill",
                        &self.collector.prefill_history,
                        ChartMetric::Prefill,
                    ),
                    Chart::Cache => self.cache_panel(),
                    Chart::Gpu => (
                        self.gpu_chart_title(),
                        &self.collector.gpu_history,
                        ChartMetric::Gpu,
                    ),
                    Chart::Memory => ("memory", &self.collector.load_history, ChartMetric::Memory),
                    Chart::Paging => ("paging", &self.collector.swap_history, ChartMetric::Swap),
                    Chart::Compression => (
                        "compression",
                        &self.collector.compression_history,
                        ChartMetric::Compression,
                    ),
                    _ => unreachable!("indicator chart expected"),
                };
                self.render_indicator_chart(frame, area, name, history, metric);
            }
        }
    }

    pub(crate) fn draw_overview(&self, frame: &mut Frame, area: Rect) {
        if self.collector.current.has_discrete_gpus() {
            self.draw_gpu_overview(frame, area);
        } else {
            self.draw_overview_layout(frame, area, false);
        }
    }

    pub(crate) fn draw_gpu_overview(&self, frame: &mut Frame, area: Rect) {
        self.draw_overview_layout(frame, area, true);
    }

    pub(crate) fn draw_overview_layout(&self, frame: &mut Frame, area: Rect, devices: bool) {
        let compact = area.height < 30;
        let info_height = OVERVIEW_INFO_HEIGHT;
        let device_height = if devices {
            gpu_dashboard::height(
                self.collector.current.gpus.len(),
                if compact { 4 } else { 6 },
            )
        } else {
            0
        };
        // Host-critical resources come first and take the space: memory, the
        // runtime's cache and paging decide whether a model keeps its speed.
        // Prompt size drives KV-cache memory, so it follows. Throughput is the
        // outcome and stays a compact, fixed-height row. Geometry is stable
        // across idle and live states.
        let summary_height = OVERVIEW_SUMMARY_HEIGHT;
        // Rows left for charts and Journal once the context strips are placed.
        let available = area
            .height
            .saturating_sub(info_height + summary_height + device_height);
        // Supporting rows grow with the terminal and stop where more height
        // adds nothing: prompt bars, a short throughput trace and a few
        // Journal events. The host row takes every remaining row, so the
        // shares below keep it near two fifths of the charts' height instead
        // of letting it swallow the screen on tall terminals.
        let journal_height = if compact {
            0
        } else {
            (available * 22 / 100).clamp(5, JOURNAL_MAX_HEIGHT)
        };
        // With no requests yet, prompt load is one line: the row hugs what
        // compression and queue need but still scales with the terminal, like the
        // Journal, instead of pinning six rows on every screen size.
        let request_height = match (compact, devices) {
            (true, _) if self.collector.request_history.len() == 0 => 6,
            (true, true) => 6,
            (true, false) => 7,
            _ if self.collector.request_history.len() == 0 => (available * 20 / 100).clamp(5, 12),
            _ => (available * 28 / 100).clamp(9, REQUEST_MAX_HEIGHT),
        };
        let mut rates_height = if compact {
            RATES_COMPACT_HEIGHT
        } else {
            (available * 20 / 100).clamp(RATES_COMPACT_HEIGHT, RATES_MAX_HEIGHT)
        };
        let mut host_height =
            available.saturating_sub(journal_height + request_height + rates_height);
        // Very short terminals keep a readable host row before throughput.
        if host_height < HOST_MIN_HEIGHT {
            let borrowed = (HOST_MIN_HEIGHT - host_height).min(rates_height.saturating_sub(3));
            rates_height -= borrowed;
            host_height += borrowed;
        }
        // Fit every captured observation into even the narrowest plot, then
        // widen those same observations across larger panels. No decimation.
        let host = self.host_columns(area);
        let grid = overview_grid(area);
        let queue_width =
            operator_charts::queue_plot_width(&self.collector.operator_history, grid[2].width);
        let samples = [
            host[0].width.saturating_sub(7),
            host[1].width.saturating_sub(14),
            host.last()
                .map_or(0, |column| column.width.saturating_sub(14)),
            grid[0].width.saturating_sub(8),
            grid[1].width.saturating_sub(8),
            grid[2].width.saturating_sub(7),
            queue_width,
        ]
        .into_iter()
        .min()
        .unwrap_or(1)
        .max(1);
        self.charts.overview_samples.set(Some(usize::from(samples)));
        let rows = Layout::vertical([
            Constraint::Length(info_height),
            Constraint::Length(summary_height),
            Constraint::Length(device_height),
            Constraint::Length(host_height),
            Constraint::Length(request_height),
            Constraint::Length(rates_height),
            Constraint::Length(journal_height),
        ])
        .split(area);
        model_dashboard::draw(
            frame,
            rows[0],
            &self.collector.current,
            &self.collector.request_history,
        );
        crate::diagnostics_view::summary(frame, rows[1], &self.collector.current, self.paused);
        if devices {
            gpu_dashboard::draw(
                frame,
                rows[2],
                &self.collector.current.gpus,
                self.gpu_selected,
                self.thresholds,
            );
        }
        self.draw_resource_charts(frame, rows[3]);
        self.draw_supporting_charts(frame, rows[4]);
        self.draw_workload_charts(frame, rows[5]);
        if rows[6].height > 0 {
            if self.collector.operator_history.has_latency() && area.width >= 120 {
                let columns =
                    Layout::horizontal([Constraint::Percentage(70), Constraint::Percentage(30)])
                        .split(rows[6]);
                self.draw_signal_log(frame, columns[0]);
                self.draw_operator_chart(frame, columns[1], Chart::Latency);
            } else {
                self.draw_signal_log(frame, rows[6]);
            }
        }
    }

    /// macOS has compressor counters; Linux has none to chart, so its queue
    /// takes the whole third supporting column.
    pub(crate) fn shows_compression(&self) -> bool {
        self.collector.platform == Platform::MacOs
    }

    /// Every Overview row shares one three-column grid, so panel edges line
    /// up vertically.
    pub(crate) fn host_columns(&self, area: Rect) -> Vec<Rect> {
        overview_grid(area).to_vec()
    }

    /// Memory, the runtime's cache and paging: what the model's memory holds,
    /// what the cache saves and costs on disk, and what the OS pages.
    pub(crate) fn draw_resource_charts(&self, frame: &mut Frame, area: Rect) {
        let columns = self.host_columns(area);
        self.render_indicator_chart(
            frame,
            columns[0],
            "memory",
            &self.collector.load_history,
            ChartMetric::Memory,
        );
        let (name, history, metric) = self.cache_panel();
        self.render_indicator_chart(frame, columns[1], name, history, metric);
        self.render_indicator_chart(
            frame,
            columns[2],
            "paging / I/O",
            &self.collector.swap_history,
            ChartMetric::Swap,
        );
    }

    /// Prompt load spans two grid columns for its bars; compression and queue
    /// stack in the third, since each carries one or two readings.
    pub(crate) fn draw_supporting_charts(&self, frame: &mut Frame, area: Rect) {
        let [first, second, third] = overview_grid(area);
        self.draw_request_chart(frame, first.union(second));
        if !self.shows_compression() {
            self.draw_operator_chart(frame, third, Chart::Queue);
            return;
        }
        let [compression, queue] =
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).areas(third);
        self.render_indicator_chart(
            frame,
            compression,
            "compression",
            &self.collector.compression_history,
            ChartMetric::Compression,
        );
        self.draw_operator_chart(frame, queue, Chart::Queue);
    }

    /// The cache panel plots the runtime's own disk traffic where the OS
    /// reports it: per-request reuse is in prompt load, and disk is where a
    /// cache stalls an answer. Without it, the reuse of each interval.
    pub(crate) fn cache_panel(&self) -> (&'static str, &VecDeque<ChartPoint>, ChartMetric) {
        let disk = &self.collector.disk_history;
        if disk.iter().any(|point| point.value.is_some()) {
            ("KV cache / model I/O", disk, ChartMetric::Disk)
        } else {
            (
                "KV cache",
                &self.collector.cache_history,
                ChartMetric::Cache,
            )
        }
    }

    pub(crate) fn draw_workload_charts(&self, frame: &mut Frame, area: Rect) {
        let columns = overview_grid(area);
        self.render_indicator_chart(
            frame,
            columns[0],
            "generation",
            &self.collector.generation_history,
            ChartMetric::Generation,
        );
        self.render_indicator_chart(
            frame,
            columns[1],
            "prefill",
            &self.collector.prefill_history,
            ChartMetric::Prefill,
        );
        self.render_indicator_chart(
            frame,
            columns[2],
            self.gpu_chart_title(),
            &self.collector.gpu_history,
            ChartMetric::Gpu,
        );
    }

    /// Recent history as bars across `area`, for panels too short for axes:
    /// zero at the bottom, the visible peak (or 100%) at the top, one column
    /// per sample in its grade, the newest at the right.
    pub(crate) fn draw_sparkline(
        &self,
        frame: &mut Frame,
        area: Rect,
        history: &VecDeque<ChartPoint>,
        metric: ChartMetric,
    ) {
        if area.is_empty() {
            return;
        }
        let points = chart_columns(history, usize::from(area.width));
        let percent = matches!(
            metric,
            ChartMetric::Memory | ChartMetric::Gpu | ChartMetric::Cache
        );
        let ceiling = if percent {
            100
        } else {
            points
                .iter()
                .filter_map(|point| point.value)
                .max()
                .unwrap_or(0)
                .max(1)
        };
        for (column, point) in points.iter().enumerate() {
            let Some(value) = point.value else {
                continue;
            };
            let value = if percent {
                normalize_chart_value(metric, value)
            } else {
                value
            };
            let display = (u128::from(value) * 100 / u128::from(ceiling)).min(100) as u64;
            let mut glyphs = bar_cells(display, usize::from(area.height));
            if glyphs.is_empty() {
                glyphs.push(if value > 0 { '▁' } else { '─' });
            }
            let color = point.tone.color();
            for (height, glyph) in glyphs.into_iter().enumerate() {
                let cell = &mut frame.buffer_mut()
                    [(area.x + column as u16, area.bottom() - 1 - height as u16)];
                if glyph == '█' {
                    cell.set_symbol(" ").set_bg(color);
                } else {
                    cell.set_symbol(&glyph.to_string()).set_fg(color);
                }
            }
        }
    }

    /// The composition samples behind each memory chart column, aligned with
    /// the memory history exactly as its points are: the newest at the right,
    /// spread over the plot width.
    pub(crate) fn composition_columns(
        &self,
        visible: usize,
        width: usize,
    ) -> Vec<Option<[u64; 5]>> {
        let history = &self.collector.composition_history;
        if visible == 0 || width == 0 {
            return Vec::new();
        }
        let start = history.len().saturating_sub(visible);
        let padding = visible - (history.len() - start);
        (0..width)
            .map(|column| {
                let index = column * visible / width;
                index
                    .checked_sub(padding)
                    .and_then(|offset| history.get(start + offset).copied().flatten())
            })
            .collect()
    }

    pub(crate) fn gpu_chart_title(&self) -> &'static str {
        if self.collector.current.has_discrete_gpus() && self.collector.current.gpus.len() > 1 {
            "GPU max"
        } else {
            "GPU"
        }
    }

    pub(crate) fn render_indicator_chart(
        &self,
        frame: &mut Frame,
        area: Rect,
        name: &'static str,
        history: &VecDeque<ChartPoint>,
        metric: ChartMetric,
    ) {
        self.charts.register(Chart::from(metric), area);
        let zoom = usize::from(self.charts.zoom(Chart::from(metric)));
        let sample = &self.collector.current;
        let current = history.back().and_then(|point| point.value);
        let current_label = self.chart_reading(area, history, metric);
        let chart_tone = metric.chart_tone();
        let current_tone = history
            .back()
            .map(|point| point.tone)
            .unwrap_or(Tone::Muted);
        let current_tone = if metric == ChartMetric::Memory {
            self.collector.current.pressure_tone
        } else {
            current_tone
        };
        let rate_chart = matches!(metric, ChartMetric::Generation | ChartMetric::Prefill);
        let label_width = if metric.is_byte_rate() {
            12
        } else if rate_chart {
            6
        } else {
            5
        }
        .min(area.width.saturating_sub(2) as usize);
        let plot_width = (area.width.saturating_sub(2) as usize).saturating_sub(label_width);
        let visible_count = self.charts.visible_samples(Chart::from(metric), plot_width);
        let visible_missing = history
            .iter()
            .rev()
            .take(visible_count)
            .all(|point| point.value.is_none());
        // Paging and compression are judged against their warning rates: the
        // axis never shrinks below one, so a trickle stays a small bar and the
        // threshold has a place on the chart.
        let warning = match metric {
            ChartMetric::Swap => Some(self.thresholds.swap_warn_rate),
            ChartMetric::Compression => Some(self.thresholds.compression_warn_rate),
            _ => None,
        };
        // The generation chart's dashed line is its usual speed, once that
        // is known. It is quiet until generation is slow, and then takes the
        // anomaly colour: the gap under it is the drop.
        let insight = &sample.correlation;
        let slowed = insight.concerns(metric) && insight.is_material_drop();
        let normal = insight
            .episode
            .filter(|_| slowed)
            .map(|episode| episode.reference)
            .or(insight.usual_speed.map(|(usual, _)| usual))
            .filter(|usual| insight.concerns(metric) && *usual > 0.0)
            .map(|usual| (usual * 10.0).round() as u64);
        let (warning, guide_tone) = match normal {
            Some(normal) if slowed => (Some(normal), Tone::Unusual),
            Some(normal) => (Some(normal), Tone::Muted),
            None => (warning, Tone::Yellow),
        };
        // The usual speed each sample was judged against, where known: the
        // line follows the work, so the axis must reach its highest point.
        let usual_peak = history
            .iter()
            .rev()
            .take(visible_count)
            .filter_map(|point| point.usual)
            .max();
        let scale = chart_scale(history, metric, visible_count);
        let scale = match usual_peak {
            Some(peak) => (scale.0, scale.1.max(crate::chart_scale::ceiling(peak, 10))),
            None => scale,
        };
        let scale = match warning {
            Some(rate) => (
                0,
                scale.1.max(crate::chart_scale::ceiling(
                    rate,
                    if normal.is_some() { 10 } else { 1 },
                )),
            ),
            None => scale,
        };
        let top_axis = chart_axis_label(metric, scale.1);
        let (average, peak) = chart_stats_for_width(history, metric, visible_count);
        let window = format!(
            "{} · {zoom}×",
            chart_window_label(history.len().min(visible_count), self.interval)
        );
        let name = if metric == ChartMetric::Memory && area.width < 20 {
            "RAM"
        } else if metric == ChartMetric::Swap && area.width < 34 {
            "paging"
        } else {
            name
        };
        let mut title = Line::from(vec![Span::styled(
            format!(" {name} "),
            Style::default()
                .fg(chart_tone.color())
                .add_modifier(Modifier::BOLD),
        )]);
        let reading = Line::from(Span::styled(
            format!(" {current_label} "),
            Style::default()
                .fg(current_tone.color())
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Right);
        // Whole metadata fields fit or disappear. Never clip a token rate,
        // unit, time window or PID to squeeze in lower-priority statistics.
        let mut details = vec![window.clone()];
        if average.is_some() {
            details.extend([
                format!("avg {}", chart_stat_label(metric, average)),
                format!("peak {}", chart_stat_label(metric, peak)),
            ]);
        }
        // What this chart's slowdowns are judged against.
        if let Some((usual, slow_below)) = insight.usual_speed.filter(|_| insight.concerns(metric))
        {
            details.push(format!("usual {usual:.1} · slow below {slow_below:.1}"));
        }
        if let Some(episode) = insight.episode.filter(|_| slowed) {
            details.push(format!(
                "slow {:.0}s · low {:.1}",
                episode.seconds, episode.low
            ));
        }
        // The Overview shares one window, shown once in the top bar; only an
        // expanded chart, which zooms on its own, names its window here.
        if self.charts.expanded && area.width >= 38 {
            for detail in details {
                let span = Span::styled(format!(" · {detail}"), Style::default().fg(MUTED));
                if title.width() + span.width() + reading.width() + 2
                    <= usize::from(area.width.saturating_sub(2))
                {
                    title.spans.push(span);
                }
            }
        }
        let mut block = Block::default()
            .title(title)
            .title(reading)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(DIM))
            .style(Style::default().bg(PANEL));
        let summary = self.chart_summary(area, history, metric);
        if let Some(summary) = summary {
            let summary = if rate_chart {
                if area.width < 38 {
                    summary.replace(" GEN", "").replace(" PREFILL", "")
                } else {
                    summary.replacen("AVG ", "SERVER AVG ", 1)
                }
            } else {
                summary
            };
            // The panel behind a slowdown carries its tag at the right of
            // this border; a summary that would run into it gives way.
            let tag = if drop_marks::linked(insight) == Some(Chart::from(metric)) {
                let room = drop_marks::tag_room(area.width) / 2;
                Line::from(drop_marks::tag(insight, room).as_str()).width()
            } else {
                0
            };
            let summary = if tag > 0
                && Line::from(summary.as_str()).width() + tag + 6 > usize::from(area.width)
            {
                String::new()
            } else {
                summary
            };
            // While generation is slow, its chart says why under the bars
            // that show it, in the anomaly colour: ▼ and what else is unusual.
            let room = usize::from(area.width.saturating_sub(4));
            let reason = (insight.concerns(metric) && insight.is_material_drop())
                .then(|| {
                    let lead = insight.lead.split(" → ").next().unwrap_or("");
                    // With nothing else unusual the label says so, rather
                    // than leave the arrow on its own.
                    let mut reasons = Vec::new();
                    if !lead.is_empty() {
                        reasons.push(format!("▼ {lead}"));
                        reasons.push(format!("▼ {}", lead.split(" · ").next().unwrap_or("")));
                        reasons.push(format!("▼ {}", lead.split(" (").next().unwrap_or("")));
                    }
                    reasons.push(format!("▼ {}", insight.cause.label()));
                    reasons.push("▼".to_owned());
                    reasons
                        .into_iter()
                        .find(|text| Line::from(text.as_str()).width() + 2 <= room)
                })
                .flatten();
            block = block.title_bottom(match reason {
                Some(reason) => Line::from(Span::styled(
                    format!(" {reason} "),
                    Style::default()
                        .fg(Tone::Unusual.color())
                        .add_modifier(Modifier::BOLD),
                )),
                None if summary.is_empty() => Line::default(),
                None => Line::from(Span::styled(
                    format!(" {summary} "),
                    Style::default().fg(if metric == ChartMetric::Memory {
                        MUTED
                    } else {
                        chart_tone.color()
                    }),
                )),
            });
        }
        let mut inner = block.inner(area);
        frame.render_widget(block, area);
        self.mark_if_linked(frame, area, Chart::from(metric));
        if inner.is_empty() {
            return;
        }
        // Each panel's own rows above its plot; a panel that says all it
        // has to say in them is done.
        if self.draw_panel_rows(frame, area, &mut inner, history, metric, current_tone) {
            return;
        }
        let compact_percent = inner.height < 3
            && matches!(
                metric,
                ChartMetric::Memory | ChartMetric::Gpu | ChartMetric::Cache
            );
        if compact_percent {
            let measured = current.map(|value| (value as f64, format!("{value}%"), current_tone));
            if let Some((value, label, tone)) = measured {
                frame.render_widget(
                    Gauge::default()
                        .ratio((value / 100.0).clamp(0.0, 1.0))
                        .label(label)
                        .use_unicode(true)
                        .gauge_style(Style::default().fg(tone.color()).bg(EDGE)),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );
                if inner.height > 1 {
                    self.draw_sparkline(
                        frame,
                        Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1),
                        history,
                        metric,
                    );
                }
            } else {
                frame.render_widget(
                    Paragraph::new(if inner.width < 19 {
                        "Unavailable"
                    } else {
                        "Reading unavailable"
                    })
                    .style(Style::default().fg(MUTED)),
                    inner,
                );
            }
            return;
        }
        if visible_missing {
            // One vocabulary for absent data: "in this window" when older
            // samples exist, "yet" when none have been captured.
            let idle = sample.llm_status == "idle";
            let message = match metric {
                // Hit rate needs prompt tokens read in the window.
                ChartMetric::Cache
                    if history.iter().any(|point| point.value.is_some()) && inner.width >= 30 =>
                {
                    "No prompts read in this window"
                }
                _ if history.iter().any(|point| point.value.is_some()) => {
                    if idle && inner.width >= 32 {
                        "Idle · no samples in this window"
                    } else {
                        "No samples in this window"
                    }
                }
                ChartMetric::Cache if inner.width < 20 => "No cache samples",
                ChartMetric::Cache => "No cache samples yet",
                ChartMetric::Generation | ChartMetric::Prefill
                    if sample.llm_status == "untracked" =>
                {
                    if inner.width >= 34 {
                        "Busy outside oMLX · no live rates"
                    } else {
                        "No live rates"
                    }
                }
                ChartMetric::Generation | ChartMetric::Prefill if idle && inner.width >= 26 => {
                    "Idle · no rate samples yet"
                }
                ChartMetric::Generation | ChartMetric::Prefill => "No rate samples yet",
                _ => "No samples yet",
            };
            frame.render_widget(
                Paragraph::new(message)
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(MUTED)),
                Rect::new(
                    inner.x,
                    inner.y + inner.height.saturating_sub(1) / 2,
                    inner.width,
                    1,
                ),
            );
            return;
        }
        // Zero is a measurement, not an absent sample. Keep the zero trace
        // and its gaps, but do not invent a 1 B/s ceiling for an idle window.
        let paging_idle = metric.is_byte_rate() && peak == Some(0);
        if rate_chart && inner.height < 3 {
            self.draw_sparkline(frame, inner, history, metric);
            return;
        }

        // Historical observations end where sampling stopped. Explain the
        // trailing gap without drawing a zero or holding the old rate live.
        if current.is_none() && inner.height >= 4 && !rate_chart && metric != ChartMetric::Cache {
            if let Some((observed_at, value)) = history
                .iter()
                .rev()
                .take(visible_count)
                .find_map(|point| point.value.map(|value| (point.observed_at, value)))
            {
                let value_label = chart_stat_label(metric, Some(value));
                let mut label = if rate_chart {
                    format!("LAST SAMPLE {value_label} tok/s")
                } else if metric == ChartMetric::Cache {
                    format!("LAST INTERVAL {value_label}")
                } else {
                    format!("LAST {value_label}")
                };
                if Line::from(label.as_str()).width() > usize::from(inner.width) {
                    label = if rate_chart {
                        format!("LAST {value_label} tok/s")
                    } else {
                        format!("LAST INT {value_label}")
                    };
                }
                let with_age = format!("{label} · {}", telemetry_age(Some(observed_at)));
                if Line::from(with_age.as_str()).width() <= usize::from(inner.width) {
                    label = with_age;
                }
                frame.render_widget(
                    Paragraph::new(label).style(Style::default().fg(MUTED)),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );
                inner.y += 1;
                inner.height -= 1;
            }
        }

        let plot_height = inner.height as usize;
        // Render the newest sample at the right edge and let older samples
        // leave from the left. Every displayed column maps to one captured
        // sample: smoothing uses only the causal prefix and the current chart
        // resolution. Auto-scaling changes coordinates, never captured values,
        // ordering or colors; there is no future-sample smoothing.
        let form = chart_form(metric);
        let bars = form == ChartForm::Bars;
        // Bars show each sample as measured; the line's deadband, which keeps a
        // trace from jittering, would misstate a bar's height.
        let points = if bars {
            chart_columns(history, visible_count)
        } else {
            chart_columns_for_plot(history, visible_count, metric, plot_height, scale)
        };
        let visible_points = stretch_chart_columns(&points, plot_width);
        let axis_rows = plot_height;
        let mut cells = vec![
            vec![
                TraceCell {
                    glyph: ' ',
                    tone: Tone::Muted,
                    fill: false,
                };
                plot_width
            ];
            plot_height
        ];
        // The reference line: a rate chart's warning threshold where it has
        // one, otherwise the middle of the axis.
        let guide_row = match warning {
            Some(rate) => trace_point(chart_display_value(metric, rate, scale), plot_height)
                .map(|(row, _)| row)
                .filter(|row| row + 1 < plot_height),
            None => (!paging_idle).then_some(axis_rows.saturating_sub(1) / 2),
        };
        // Samples that carry their own usual value draw it column by column:
        // a line that steps down when the work gets heavier, and a bar
        // falling short of its own column's line is the anomaly.
        let own_usual = visible_points.iter().any(|point| point.usual.is_some());
        for (row, row_cells) in cells.iter_mut().enumerate() {
            let guide = if row + 1 == axis_rows {
                Some(('─', Tone::Muted))
            } else if Some(row) == guide_row && !own_usual {
                Some((
                    '┄',
                    if warning.is_some() {
                        guide_tone
                    } else {
                        Tone::Muted
                    },
                ))
            } else {
                None
            };
            if let Some((glyph, tone)) = guide {
                row_cells.fill(TraceCell {
                    glyph,
                    tone,
                    fill: false,
                });
            }
        }

        let usual_rows: Vec<Option<usize>> = visible_points
            .iter()
            .map(|point| {
                point.usual.and_then(|usual| {
                    trace_point(chart_display_value(metric, usual, scale), plot_height)
                        .map(|(row, _)| row)
                        .filter(|row| row + 1 < axis_rows)
                })
            })
            .collect();
        if own_usual {
            for (column, point) in visible_points.iter().enumerate() {
                let Some(row) = usual_rows[column] else {
                    continue;
                };
                cells[row][column] = TraceCell {
                    glyph: '┄',
                    // Over a slow sample, the gap under it is the drop.
                    tone: if point.tone == Tone::Unusual {
                        Tone::Unusual
                    } else {
                        guide_tone
                    },
                    fill: false,
                };
            }
        }

        let mut point_rows = vec![None; plot_width];
        let mut connect_before = vec![false; plot_width];
        let mut previous_point = None;
        if bars {
            // With two or more columns per sample, the last one of each stays
            // empty so neighbouring bars read as separate samples.
            let samples = points.len().max(1);
            let gaps = plot_width >= 2 * samples;
            for (column, point) in visible_points.iter().enumerate() {
                let Some(value) = point.value else {
                    continue;
                };
                if gaps && (column + 1) * samples / plot_width != column * samples / plot_width {
                    continue;
                }
                let display_value = chart_display_value(metric, value, scale);
                let mut glyphs = bar_cells(display_value, plot_height);
                if glyphs.is_empty() {
                    // A measured zero is a reading, unlike a missing sample:
                    // it marks the baseline in its grade. Any rate shows.
                    glyphs.push(if value > 0 { '▁' } else { '─' });
                }
                for (height, glyph) in glyphs.into_iter().enumerate() {
                    cells[plot_height - 1 - height][column] = TraceCell {
                        glyph,
                        tone: point.tone,
                        fill: false,
                    };
                }
            }
        }
        // Where a bar reaches past its usual speed the line runs through
        // it, cut into the bar, so the line reads across the whole chart.
        for (column, row) in usual_rows.iter().enumerate() {
            if let Some(row) = row {
                let cell = &mut cells[*row][column];
                if cell.glyph == '█' {
                    cell.glyph = USUAL_THROUGH_BAR;
                }
            }
        }
        for (column, point) in visible_points.iter().enumerate().filter(|_| !bars) {
            let Some(value) = point.value else {
                previous_point = None;
                continue;
            };
            if point.break_before {
                previous_point = None;
            }
            let display_value = chart_display_value(metric, value, scale);
            let Some((row, glyph)) = trace_point(display_value, plot_height) else {
                previous_point = None;
                continue;
            };
            if previous_point.is_some() {
                connect_before[column] = true;
            }
            point_rows[column] = Some((row, point.tone));
            cells[row][column] = TraceCell {
                fill: false,
                glyph: if previous_point.is_none()
                    && visible_points
                        .get(column + 1)
                        .is_none_or(|next| next.value.is_none() || next.break_before)
                {
                    '●'
                } else {
                    glyph
                },
                tone: point.tone,
            };
            previous_point = Some((row, point.tone));
        }

        for column in 1..plot_width {
            if !connect_before[column] {
                continue;
            }
            let (Some((previous_row, previous_tone)), Some((row, tone))) =
                (point_rows[column - 1], point_rows[column])
            else {
                continue;
            };
            trace_connector(
                &mut cells,
                column,
                previous_row,
                row,
                TraceStyle {
                    metric,
                    previous_tone,
                    tone,
                    thresholds: self.thresholds,
                    scale,
                },
            );
        }

        if form == ChartForm::Area {
            // The level's body beneath its line, quiet so a full chart is not
            // a solid block; guides stay visible through it.
            // The line's own cell is filled too, so the line is the area's top
            // edge rather than floating above it.
            for (column, point) in point_rows.iter().enumerate() {
                if let Some((row, _)) = point {
                    for below in cells.iter_mut().skip(*row) {
                        below[column].fill = true;
                    }
                }
            }
        }

        let mut lines = Vec::with_capacity(plot_height);
        for (row, row_cells) in cells.iter().enumerate() {
            let label = if row == 0 && !paging_idle {
                format!("{top_axis} ")
            } else if row + 1 == axis_rows {
                format!("{} ", chart_axis_label(metric, scale.0))
            } else if Some(row) == guide_row && scale.1.saturating_sub(scale.0) > 1 {
                format!(
                    "{} ",
                    chart_axis_label(metric, warning.unwrap_or(scale.0 + (scale.1 - scale.0) / 2))
                )
            } else {
                String::new()
            };
            let mut spans = vec![Span::styled(
                format!("{label:>label_width$}"),
                Style::default().fg(DIM),
            )];
            let mut run = String::new();
            let mut run_tone = None;
            // A full block is drawn as a filled cell: terminals leave a gap
            // between rows of glyphs, but not of backgrounds.
            let style = |(tone, filled, fill, through): (Tone, bool, bool, bool)| {
                let style = Style::default().fg(tone.color());
                match (filled, fill) {
                    _ if through => Style::default().fg(PANEL).bg(tone.color()),
                    (true, _) => style.bg(tone.color()),
                    (false, true) => style.bg(AREA_FILL),
                    (false, false) => style,
                }
            };
            for cell in row_cells.iter().take(plot_width).copied() {
                let through = cell.glyph == USUAL_THROUGH_BAR;
                let filled = cell.glyph == '█';
                let key = (cell.tone, filled, cell.fill, through);
                if run_tone != Some(key) {
                    if let Some(key) = run_tone {
                        spans.push(Span::styled(std::mem::take(&mut run), style(key)));
                    }
                    run_tone = Some(key);
                }
                run.push(match cell.glyph {
                    '█' => ' ',
                    USUAL_THROUGH_BAR => '┄',
                    glyph => glyph,
                });
            }
            if let Some(key) = run_tone {
                spans.push(Span::styled(run, style(key)));
            }
            lines.push(Line::from(spans));
        }
        frame.render_widget(Paragraph::new(Text::from(lines)), inner);
        // An idle disk panel keeps its baseline: the rows above it say what
        // the cache holds, and a sentence would cover them.
        if paging_idle && inner.height >= 4 && metric != ChartMetric::Disk {
            frame.render_widget(
                Paragraph::new(match (metric, inner.width >= 32) {
                    (ChartMetric::Compression, true) => "No compression in this window",
                    (ChartMetric::Compression, false) => "No compression",
                    (ChartMetric::Disk, true) => "No disk reads or writes in this window",
                    (ChartMetric::Disk, false) => "Disk idle",
                    (_, true) => "No paging traffic in this window",
                    (_, false) => "No paging traffic",
                })
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED)),
                Rect::new(inner.x, inner.y + inner.height / 2 - 1, inner.width, 1),
            );
        }
    }

    pub(crate) fn draw_journal(&self, frame: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(10)])
            .split(area);
        self.draw_journal_header(frame, rows[0]);
        self.draw_journal_events(frame, rows[1]);
    }

    /// One line: how many events, which filter, and how recent the last one
    /// is. The list below shows the events themselves, newest first.
    pub(crate) fn draw_journal_header(&self, frame: &mut Frame, area: Rect) {
        let filtered_events = self.filtered_journal_events();
        let event_count = filtered_events.len();
        let total_count = self.collector.signals.len();
        let mut line = Line::from(Span::styled(
            if event_count == total_count {
                format!(" {total_count} events")
            } else {
                format!(
                    " {event_count} {} of {total_count} events",
                    self.journal_filter.label()
                )
            },
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
        if let Some(event) = filtered_events.last() {
            line.spans.push(Span::styled(
                format!(" · latest {}", telemetry_age(Some(event.recorded_at))),
                Style::default().fg(MUTED),
            ));
        }
        frame.render_widget(
            Paragraph::new(line).block(panel("JOURNAL", Tone::Cyan)),
            area,
        );
    }

    pub(crate) fn draw_compact_warning(&self, frame: &mut Frame, area: Rect) {
        let text = vec![
            Line::from(Span::styled(
                "mlxtop",
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("This dashboard needs at least 72×24 terminal cells."),
            Line::from(format!("Current size: {}×{}", area.width, area.height)),
            Line::from("Resize the terminal, or use --once for a static report."),
            Line::from("q quit"),
        ];
        frame.render_widget(
            Paragraph::new(text).alignment(Alignment::Center).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(YELLOW)),
            ),
            area,
        );
    }
}

/// Requests generating now, from the runtime's request list.
pub(crate) fn generating_requests(sample: &crate::domain::Sample) -> usize {
    sample
        .llm_requests
        .iter()
        .filter(|r| !r.completed && !r.queued && r.prefilled.is_none())
        .count()
}
