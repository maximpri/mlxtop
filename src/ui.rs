// SPDX-License-Identifier: MIT
//! Rendering for Overview, Top, Journal and their overlays.
use crate::app::App;
use crate::chart_navigation::Chart;
use crate::chart_render::{
    bar_cells, chart_axis_label, chart_columns, chart_columns_for_plot, chart_display_value,
    chart_form, chart_inactive_rate_label, chart_scale, chart_stat_label, chart_stats_for_width,
    chart_window_label, meter_cells, normalize_chart_value, stretch_chart_columns, trace_connector,
    trace_point, ChartForm, TraceCell, TraceStyle,
};
use crate::cli::VERSION;
use crate::config::PAGING_ACTIVE_ENTER_RATE;
use crate::domain::{ChartMetric, ChartPoint, Tone};
use crate::formatting::{
    activity_frame, bytes, compact_label, gpu_load_label, llm_generation_rate_label,
    llm_prefill_progress_label, llm_prefill_rate_label, percent, pressure_state_label, rate,
    telemetry_age, telemetry_source,
};
use crate::host::Platform;
use crate::processes::process_provider;
use crate::theme::{
    centered_rect, panel, AREA_FILL, CYAN, DIM, EDGE, MUTED, PANEL, PANEL_RAISED, RED, YELLOW,
};
use crate::{
    gpu_dashboard, memory_composition, model_dashboard, operator_charts, process_memory,
    request_dashboard, swap_usage,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Table, TableState, Tabs, Wrap,
};
use ratatui::Frame;
use std::collections::VecDeque;

/// Throughput is the outcome, shown compactly; host resources get the space.
const RATES_HEIGHT: u16 = 7;
const RATES_MAX_HEIGHT: u16 = 10;
/// Short terminals keep each rate in its title; Enter expands the history.
const RATES_COMPACT_HEIGHT: u16 = 4;
/// Prompt bars and Journal stop growing once more rows add no information.
const REQUEST_MAX_HEIGHT: u16 = 14;
const JOURNAL_MAX_HEIGHT: u16 = 10;
/// The host row takes all remaining height, never less than this.
const HOST_MIN_HEIGHT: u16 = 6;
/// SYSINFO strip, then the borderless two-row assessment beneath it.
const OVERVIEW_INFO_HEIGHT: u16 = 3;
const OVERVIEW_SUMMARY_HEIGHT: u16 = 2;

/// The shared Overview column grid: three equal columns.
fn overview_grid(area: Rect) -> [Rect; 3] {
    Layout::horizontal([Constraint::Ratio(1, 3); 3]).areas(area)
}

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
                    Chart::Cache => ("cache", &self.collector.cache_history, ChartMetric::Cache),
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
        // Host-critical resources come first and take the space: memory,
        // compression and paging decide whether a model keeps its speed.
        // Prompt size drives KV-cache memory, so it follows. Throughput is the
        // outcome and stays a compact, fixed-height row. Geometry is stable
        // across idle and live states.
        let summary_height = OVERVIEW_SUMMARY_HEIGHT;
        // Rows left for charts and Journal once the context strips are placed.
        let available = area
            .height
            .saturating_sub(info_height + summary_height + device_height);
        // Supporting rows grow slowly and stop where more height adds nothing:
        // prompt bars, a short throughput trace and a few Journal events. The
        // host row takes every remaining row, so tall terminals widen the
        // resource histories instead of stretching sparse panels.
        let journal_height = if compact {
            0
        } else {
            (5 + available.saturating_sub(35) / 3).min(JOURNAL_MAX_HEIGHT)
        };
        // With no requests yet, prompt load is one line: the row keeps only
        // what cache and queue need in their compact forms, and the rest goes
        // to the host charts until there is something to draw.
        let request_height = match (compact, devices) {
            _ if self.collector.request_history.len() == 0 => 6,
            (true, true) => 6,
            (true, false) => 7,
            (false, _) => (available * 28 / 100).clamp(9, REQUEST_MAX_HEIGHT),
        };
        let mut rates_height = if compact {
            RATES_COMPACT_HEIGHT
        } else {
            (available * 15 / 100).clamp(RATES_HEIGHT, RATES_MAX_HEIGHT)
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

    /// macOS has compressor counters; Linux has none to chart, so its host
    /// row keeps memory and paging at half width each.
    pub(crate) fn shows_compression(&self) -> bool {
        self.collector.platform == Platform::MacOs
    }

    /// Every Overview row shares one three-column grid, so panel edges line
    /// up vertically. Linux has no compression panel; memory spans two columns.
    pub(crate) fn host_columns(&self, area: Rect) -> Vec<Rect> {
        let [first, second, third] = overview_grid(area);
        if self.shows_compression() {
            vec![first, second, third]
        } else {
            vec![first.union(second), third]
        }
    }

    pub(crate) fn draw_resource_charts(&self, frame: &mut Frame, area: Rect) {
        let columns = self.host_columns(area);
        self.render_indicator_chart(
            frame,
            columns[0],
            "memory",
            &self.collector.load_history,
            ChartMetric::Memory,
        );
        if self.shows_compression() {
            self.render_indicator_chart(
                frame,
                columns[1],
                "compression",
                &self.collector.compression_history,
                ChartMetric::Compression,
            );
        }
        self.render_indicator_chart(
            frame,
            columns[columns.len() - 1],
            "paging / I/O",
            &self.collector.swap_history,
            ChartMetric::Swap,
        );
    }

    /// Prompt load spans two grid columns for its bars; cache and queue stack
    /// in the third, since each carries one or two readings.
    pub(crate) fn draw_supporting_charts(&self, frame: &mut Frame, area: Rect) {
        let [first, second, third] = overview_grid(area);
        self.draw_request_chart(frame, first.union(second));
        let [cache, queue] =
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).areas(third);
        self.render_indicator_chart(
            frame,
            cache,
            "cache",
            &self.collector.cache_history,
            ChartMetric::Cache,
        );
        self.draw_operator_chart(frame, queue, Chart::Queue);
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
        let current_label = match metric {
            // Drop qualifiers before units: a bare number is never the reading.
            ChartMetric::Generation | ChartMetric::Prefill => current
                .map(|value| {
                    let rate = format!("{:.1} tok/s", value as f64 / 10.0);
                    if area.width < 38 {
                        rate
                    } else {
                        let live = match metric {
                            ChartMetric::Generation => sample.llm_generation_tps_live,
                            ChartMetric::Prefill => sample.llm_prefill_tps_live,
                            _ => false,
                        };
                        if live {
                            format!("LIVE {rate}")
                        } else {
                            format!("LAST {rate}")
                        }
                    }
                })
                .unwrap_or_else(|| {
                    if metric == ChartMetric::Prefill
                        && sample.llm_prefill_tps.is_some()
                        && area.width >= 38
                    {
                        return llm_prefill_rate_label(sample);
                    }
                    let label = chart_inactive_rate_label(metric, &self.collector.current);
                    if area.width < 38 {
                        match label.as_str() {
                            "decode active" => "decoding".into(),
                            "prefill active" => "prefilling".into(),
                            _ => label,
                        }
                    } else {
                        label
                    }
                }),
            ChartMetric::Cache => {
                let value = current.map_or_else(|| "—".into(), |value| format!("{value}%"));
                if area.width < 28 {
                    value
                } else {
                    format!("hit {value}")
                }
            }
            ChartMetric::Gpu => current
                .map(|value| format!("{value}%"))
                .unwrap_or_else(|| "—".into()),
            // OS pressure is the memory verdict; resident occupancy (which
            // includes file cache) is the supporting reading beneath it.
            ChartMetric::Memory => {
                let state = pressure_state_label(&self.collector.current);
                if area.width >= 34 {
                    format!("PRESSURE {state}")
                } else {
                    state.into()
                }
            }
            ChartMetric::Swap | ChartMetric::Compression => {
                current.map(rate).unwrap_or_else(|| "—".into())
            }
        };
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
        let scale = chart_scale(history, metric, visible_count);
        let scale = match warning {
            Some(rate) => (0, scale.1.max(crate::chart_scale::ceiling(rate, 1))),
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
        let summary = match metric {
            ChartMetric::Generation
                if self.charts.expanded
                    && sample.llm_generation_tps.is_some()
                    && !sample.llm_generation_tps_live =>
            {
                Some(llm_generation_rate_label(sample))
            }
            ChartMetric::Prefill
                if self.charts.expanded
                    && sample.llm_prefill_tps.is_some()
                    && !sample.llm_prefill_tps_live =>
            {
                Some(llm_prefill_rate_label(sample))
            }
            // While a prefill runs, its position replaces the unit: the rate is already in the title,
            // and 40.9k / 61.5k tells you how long the wait is.
            ChartMetric::Prefill => {
                Some(llm_prefill_progress_label(sample).unwrap_or_else(|| "tok/s".into()))
            }
            ChartMetric::Generation => Some("tok/s".into()),
            ChartMetric::Memory => None,
            ChartMetric::Gpu => Some(if current == Some(0) {
                "idle".into()
            } else {
                gpu_load_label(sample.gpu_util, self.thresholds).into()
            }),
            ChartMetric::Compression => {
                let (compress, decompress) = if sample.rate_ready && sample.vm_available {
                    (rate(sample.compress), rate(sample.decompress))
                } else {
                    ("—".into(), "—".into())
                };
                // Narrow panels share one unit instead of abbreviating names.
                let shared_unit = compress
                    .rsplit_once(' ')
                    .zip(decompress.rsplit_once(' '))
                    .filter(|((_, a), (_, b))| a == b)
                    .map(|((c, unit), (d, _))| {
                        format!("compressing {c} · decompressing {d} {unit}")
                    });
                let fits = |text: &String| {
                    Line::from(text.as_str()).width() + 2
                        <= usize::from(area.width.saturating_sub(2))
                };
                [
                    Some(format!(
                        "compressing {compress} · decompressing {decompress}"
                    )),
                    shared_unit,
                    compress
                        .rsplit_once(' ')
                        .zip(decompress.rsplit_once(' '))
                        .filter(|((_, a), (_, b))| a == b)
                        .map(|((c, unit), (d, _))| format!("comp {c} · decomp {d} {unit}")),
                    Some(format!("comp {compress} · decomp {decompress}")),
                ]
                .into_iter()
                .flatten()
                .find(fits)
                .or_else(|| Some(format!("comp {compress}")))
            }
            ChartMetric::Swap => {
                // All disk paging, as plotted: swap plus file page-ins/outs.
                let (swap_in, swap_out) = if sample.rate_ready && !sample.paging_unavailable {
                    (
                        rate(sample.swap_in.saturating_add(sample.page_in)),
                        rate(sample.swap_out.saturating_add(sample.page_out)),
                    )
                } else {
                    ("—".into(), "—".into())
                };
                let spaced = format!("page-in {swap_in} · page-out {swap_out}");
                Some(
                    if Line::from(spaced.as_str()).width() + 2
                        <= usize::from(area.width.saturating_sub(2))
                    {
                        spaced
                    } else {
                        format!("in {swap_in} · out {swap_out}")
                    },
                )
            }
            // The title has the latest interval's hit rate; these are the
            // session-wide figures it is measured against.
            ChartMetric::Cache => Some(if area.width < 38 {
                format!("since start {}", percent(sample.llm_cache_efficiency))
            } else {
                format!(
                    "since start {} · prefix {}",
                    percent(sample.llm_cache_efficiency),
                    percent(sample.llm_prefix_hit_rate)
                )
            }),
        };
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
            block = block.title_bottom(Line::from(Span::styled(
                format!(" {summary} "),
                Style::default().fg(if metric == ChartMetric::Memory {
                    MUTED
                } else {
                    chart_tone.color()
                }),
            )));
        }
        let mut inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.is_empty() {
            return;
        }
        if metric == ChartMetric::Memory {
            let usage = match (sample.resident_memory, current) {
                (Some(used), _) => format!("{} / {}", bytes(used), bytes(sample.total_memory)),
                (None, Some(_)) if sample.total_memory > 0 => {
                    format!("RAM {}", bytes(sample.total_memory))
                }
                (None, Some(_)) => "RAM —".into(),
                (None, None) => "RAM reading unavailable".into(),
            };
            // Keep the occupancy percentage on narrow panels by sharing the unit.
            let compact_usage = sample.resident_memory.and_then(|used| {
                let (used, unit) = bytes(used)
                    .rsplit_once(' ')
                    .map(|(n, u)| (n.to_owned(), u.to_owned()))?;
                let (total, total_unit) = bytes(sample.total_memory)
                    .rsplit_once(' ')
                    .map(|(n, u)| (n.to_owned(), u.to_owned()))?;
                (unit == total_unit).then(|| format!("{used}/{total} {unit}"))
            });
            let mut line = Line::from(Span::styled(usage.clone(), Style::default().fg(MUTED)));
            if let Some(value) = current {
                let candidates = [
                    (usage, format!(" · {value}% resident")),
                    (
                        compact_usage.clone().unwrap_or_default(),
                        format!(" · {value}% resident"),
                    ),
                    (compact_usage.unwrap_or_default(), format!(" · {value}%")),
                ];
                if let Some((text, resident)) = candidates.into_iter().find(|(text, resident)| {
                    !text.is_empty()
                        && Line::from(format!("{text}{resident}").as_str()).width()
                            <= usize::from(inner.width)
                }) {
                    line = Line::from(vec![
                        Span::styled(text, Style::default().fg(MUTED)),
                        Span::styled(resident, Style::default().fg(CYAN)),
                    ]);
                }
            }
            frame.render_widget(
                Paragraph::new(line),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
            if inner.height < 2 {
                return;
            }
            inner.y += 1;
            inner.height -= 1;
            // What the RAM holds matters more than how full it is: wired memory
            // cannot be compressed or swapped; cache and free are headroom.
            // Short panels give the legend every row; taller ones keep at
            // least three rows for the occupancy trace.
            let rows = if inner.height < 6 {
                inner.height
            } else {
                (inner.height - 3).min(3)
            };
            let used = memory_composition::draw(
                frame,
                Rect::new(inner.x, inner.y, inner.width, rows),
                sample,
            );
            inner.y += used;
            inner.height -= used;
            // Short panels cannot hold a meaningful trace: the exact reading and
            // composition replace a flat line against a 0–100% axis.
            if inner.height < 3 {
                return;
            }
        }
        if metric == ChartMetric::Compression {
            if inner.height >= 5 && sample.compressor > 0 && sample.compressed_logical > 0 {
                let (stored, held) = (bytes(sample.compressed_logical), bytes(sample.compressor));
                let ratio = sample.compressed_logical as f64 / sample.compressor as f64;
                let text = [
                    format!("{stored} stored in {held} · {ratio:.1}× ratio"),
                    format!("{stored} in {held} · {ratio:.1}×"),
                    format!("{ratio:.1}× ratio"),
                ]
                .into_iter()
                .find(|text| Line::from(text.as_str()).width() <= usize::from(inner.width))
                .unwrap_or_default();
                frame.render_widget(
                    Paragraph::new(text).style(Style::default().fg(MUTED)),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );
                inner.y += 1;
                inner.height -= 1;
            }
            swap_usage::draw_compressor(
                frame,
                Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
                sample,
            );
            inner.height = inner.height.saturating_sub(1);
            if inner.is_empty() {
                return;
            }
            if inner.height < 3 {
                frame.render_widget(
                    Paragraph::new(match current {
                        Some(0) => "No compression traffic",
                        Some(rate) if rate >= self.thresholds.compression_warn_rate => {
                            "Compression active"
                        }
                        Some(_) => "Light compression · below warning",
                        None => "Compression unavailable",
                    })
                    .style(Style::default().fg(current_tone.color())),
                    inner,
                );
                return;
            }
        }
        if metric == ChartMetric::Swap {
            swap_usage::draw(
                frame,
                Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
                sample,
            );
            inner.height = inner.height.saturating_sub(1);
            if inner.is_empty() {
                return;
            }
            if inner.height < 3 {
                // Graded on what counts against the system, as the assessment
                // is: file paging only while memory is tight.
                let uncounted = if sample.memory_tight() {
                    0
                } else {
                    sample.page_in.saturating_add(sample.page_out)
                };
                let counted = current.map(|rate| rate.saturating_sub(uncounted));
                frame.render_widget(
                    // Match the assessment: "active" starts at the warning rate.
                    Paragraph::new(match (current, counted) {
                        (Some(0), _) => "No paging traffic",
                        (_, Some(rate)) if rate >= PAGING_ACTIVE_ENTER_RATE => "Paging active",
                        (_, Some(rate)) if rate >= self.thresholds.swap_warn_rate => "Watch paging",
                        (_, Some(0)) => "Disk reads · memory not tight",
                        (Some(_), _) => "Light paging · below warning",
                        (None, _) => "Paging unavailable",
                    })
                    .style(Style::default().fg(current_tone.color())),
                    inner,
                );
                return;
            }
        }
        if metric == ChartMetric::Cache && area.width < 38 && inner.height >= 4 {
            frame.render_widget(
                Paragraph::new(format!(
                    "PREFIX HIT {}",
                    percent(sample.llm_prefix_hit_rate)
                ))
                .style(Style::default().fg(MUTED)),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );
            inner.y += 1;
            inner.height -= 1;
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
        if current.is_none() && inner.height >= 4 {
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
        // Memory shows what the RAM holds over time: its kinds stacked in each
        // column, with the pressure grade as a ribbon along the bottom row.
        let composition = (metric == ChartMetric::Memory && plot_height >= 4)
            .then(|| self.composition_columns(visible_count, plot_width))
            .filter(|columns| columns.iter().any(Option::is_some));
        let stack = composition.is_some();
        let axis_rows = if stack { plot_height - 1 } else { plot_height };
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
        for (row, row_cells) in cells.iter_mut().enumerate() {
            let guide = if row + 1 == axis_rows {
                Some(('─', Tone::Muted))
            } else if Some(row) == guide_row {
                Some((
                    '┄',
                    if warning.is_some() {
                        Tone::Yellow
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
        for (column, point) in visible_points
            .iter()
            .enumerate()
            .filter(|_| !bars && !stack)
        {
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

        if stack {
            // The ribbon: each sample's pressure grade, under its column.
            for (column, point) in visible_points.iter().enumerate() {
                if point.value.is_some() {
                    cells[plot_height - 1][column] = TraceCell {
                        glyph: '▀',
                        tone: point.tone,
                        fill: false,
                    };
                }
            }
        } else if form == ChartForm::Area {
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
            let label = if stack && row + 1 == plot_height {
                String::new()
            } else if row == 0 && !paging_idle {
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
            let style = |(tone, filled, fill): (Tone, bool, bool)| {
                let style = Style::default().fg(tone.color());
                match (filled, fill) {
                    (true, _) => style.bg(tone.color()),
                    (false, true) => style.bg(AREA_FILL),
                    (false, false) => style,
                }
            };
            for cell in row_cells.iter().take(plot_width).copied() {
                let filled = cell.glyph == '█';
                let key = (cell.tone, filled, cell.fill);
                if run_tone != Some(key) {
                    if let Some(key) = run_tone {
                        spans.push(Span::styled(std::mem::take(&mut run), style(key)));
                    }
                    run_tone = Some(key);
                }
                run.push(if filled { ' ' } else { cell.glyph });
            }
            if let Some(key) = run_tone {
                spans.push(Span::styled(run, style(key)));
            }
            lines.push(Line::from(spans));
        }
        frame.render_widget(Paragraph::new(Text::from(lines)), inner);
        if let Some(columns) = composition {
            let x = inner.x + label_width as u16;
            for (column, sizes) in columns.into_iter().enumerate() {
                if let Some(sizes) = sizes {
                    memory_composition::paint_column(
                        frame,
                        x + column as u16,
                        inner.y,
                        axis_rows as u16,
                        sizes,
                    );
                }
            }
        }
        if paging_idle && inner.height >= 4 {
            frame.render_widget(
                Paragraph::new(match (metric, inner.width >= 32) {
                    (ChartMetric::Compression, true) => "No compression in this window",
                    (ChartMetric::Compression, false) => "No compression",
                    (_, true) => "No paging traffic in this window",
                    (_, false) => "No paging traffic",
                })
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED)),
                Rect::new(inner.x, inner.y + inner.height / 2 - 1, inner.width, 1),
            );
        }
    }

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

    pub(crate) fn draw_journal(&self, frame: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(10)])
            .split(area);
        self.draw_journal_header(frame, rows[0]);
        self.draw_journal_events(frame, rows[1]);
    }

    pub(crate) fn draw_llm_top(&self, frame: &mut Frame, area: Rect) {
        let details = self.collector.current.llm_details.as_deref();
        // The table title carries the totals; a panel above it only when the
        // runtime reports engine details worth reading.
        let detail_rows = details.map_or(0, |text| {
            let width = usize::from(area.width.saturating_sub(4)).max(1);
            let lines = Line::from(text).width().div_ceil(width).max(1);
            (lines.min(5) as u16 + 2).min(area.height.saturating_sub(13).max(3))
        });
        let rows = Layout::vertical([
            Constraint::Length(detail_rows),
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
                .wrap(Wrap { trim: true })
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
