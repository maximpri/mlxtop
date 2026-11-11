// SPDX-License-Identifier: MIT
//! Application state, input handling, selection and critical-alarm lifecycle.
use crate::analysis::signal_summary;
use crate::chart_navigation::Chart;
use crate::collector::CollectorView;
use crate::config::{config_history, config_interval, config_path, Config, Thresholds};
use crate::domain::{EventKind, LlmProcess, Sample, SignalEvent};
use crate::formatting::bytes;
use crate::logging::{diagnostics_log, log_field};
use crate::sampler::{Sampler, SamplerCommand};
use crate::settings::Settings;
use crate::{chart_navigation, operator_history, request_history};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::collections::VecDeque;
use std::io::{stdout, Write};
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

/// Two left clicks on one chart within this time enlarge or restore it.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JournalFilter {
    All,
    Llm,
    Pressure,
    Paging,
    Gpu,
    Thermal,
    System,
}

impl JournalFilter {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Llm => "LLM",
            Self::Pressure => "PRESSURE",
            Self::Paging => "PAGING",
            Self::Gpu => "GPU",
            Self::Thermal => "THERMAL",
            Self::System => "SYSTEM",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::All => Self::Llm,
            Self::Llm => Self::Pressure,
            Self::Pressure => Self::Paging,
            Self::Paging => Self::Gpu,
            Self::Gpu => Self::Thermal,
            Self::Thermal => Self::System,
            Self::System => Self::All,
        }
    }

    pub(crate) fn previous(self) -> Self {
        match self {
            Self::All => Self::System,
            Self::Llm => Self::All,
            Self::Pressure => Self::Llm,
            Self::Paging => Self::Pressure,
            Self::Gpu => Self::Paging,
            Self::Thermal => Self::Gpu,
            Self::System => Self::Thermal,
        }
    }

    pub(crate) fn matches(self, kind: EventKind) -> bool {
        match self {
            Self::All => true,
            Self::Llm => matches!(kind, EventKind::Llm | EventKind::Queue),
            Self::Pressure => kind == EventKind::Pressure,
            Self::Paging => kind == EventKind::Paging,
            Self::Gpu => kind == EventKind::Gpu,
            Self::Thermal => kind == EventKind::Thermal,
            Self::System => kind == EventKind::System,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TopSort {
    Rss,
    Cpu,
    Pid,
    Name,
}

impl TopSort {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Rss => "RSS",
            Self::Cpu => "CPU",
            Self::Pid => "PID",
            Self::Name => "NAME",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Rss => Self::Cpu,
            Self::Cpu => Self::Pid,
            Self::Pid => Self::Name,
            Self::Name => Self::Rss,
        }
    }
}

/// A raised aggressive-paging alert. It survives until the user acknowledges
/// it or the paging episode ends, whichever comes first.
pub(crate) struct ActiveAlert {
    pub(crate) state: String,
    pub(crate) summary: String,
    pub(crate) time: String,
}

/// Critical host conditions, deliberately excluding GPU utilization.
pub(crate) fn is_critical_state(state: &str) -> bool {
    matches!(
        state,
        "MEMORY BOTTLENECK" | "SWAP THRASHING" | "HEAVY PAGING" | "PAGE-IN RECOVERY"
    )
}

pub(crate) fn critical_state(sample: &Sample) -> Option<&str> {
    // A reported critical memory condition is valid even if another counter
    // is unavailable and the overall classifier says DATA LIMITED.
    if sample.pressure == "RED" {
        Some("MEMORY BOTTLENECK")
    } else {
        is_critical_state(&sample.impact).then_some(sample.impact.as_str())
    }
}

pub(crate) fn critical_summary(sample: &Sample) -> String {
    if critical_state(sample) == Some("MEMORY BOTTLENECK") {
        format!(
            "critical memory pressure · RAM resident {}",
            sample
                .resident_memory
                .map(bytes)
                .unwrap_or_else(|| "—".into())
        )
    } else {
        signal_summary(sample)
    }
}

/// BEL passes through the alternate screen to the terminal emulator, so the
/// user's audible/visual bell setting decides how the alert sounds. Called
/// between frames only: writing mid-draw could interleave with the buffer.
pub(crate) fn ring_terminal_bell() {
    write_terminal_bell(&mut stdout());
}

pub(crate) fn write_terminal_bell(out: &mut impl Write) {
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}

pub(crate) struct App {
    pub(crate) collector: CollectorView,
    pub(crate) sampler: Sampler,
    pub(crate) interval: Duration,
    pub(crate) paused: bool,
    pub(crate) tab: usize,
    pub(crate) top_sort: TopSort,
    pub(crate) top_filter: String,
    pub(crate) top_filtering: bool,
    pub(crate) top_selected: usize,
    pub(crate) journal_filter: JournalFilter,
    pub(crate) journal_scroll: usize,
    pub(crate) request_scroll: usize,
    pub(crate) charts: chart_navigation::Navigation,
    pub(crate) gpu_selected: usize,
    pub(crate) help: bool,
    /// The session summary overlay.
    pub(crate) summary_open: bool,
    pub(crate) diagnostics_open: bool,
    pub(crate) diagnostics_scroll: u16,
    pub(crate) diagnostics_max_scroll: std::cell::Cell<u16>,
    pub(crate) quit: bool,
    /// Esc at the top level asks before quitting, because quitting discards the session's history.
    pub(crate) quit_prompt: bool,
    pub(crate) sampler_disconnected: bool,
    pub(crate) alert: Option<ActiveAlert>,
    pub(crate) alert_bells: usize,
    /// Rings the terminal bell; replaced in tests so no BEL reaches stdout.
    pub(crate) bell: fn(),
    pub(crate) critical_episode: bool,
    /// Whether critical conditions ring the bell and raise the banner (`alarms` in the config).
    pub(crate) alarms: bool,
    pub(crate) thresholds: Thresholds,
    pub(crate) settings: Settings,
    /// The last left click on a chart, to recognise a double click.
    pub(crate) last_click: Option<(Instant, Chart)>,
}

impl App {
    pub(crate) fn new(interval: u64, history: usize, config: Config) -> Self {
        let sampler = Sampler::spawn(Duration::from_secs(interval), history, config.clone());
        Self::with_sampler(interval, history, config, sampler)
    }

    pub(crate) fn with_sampler(
        interval: u64,
        _history: usize,
        config: Config,
        sampler: Sampler,
    ) -> Self {
        let interval = Duration::from_secs(interval);
        let thresholds = Thresholds::from_config(&config);
        Self {
            collector: CollectorView {
                current: Sample::default(),
                generation_history: VecDeque::new(),
                prefill_history: VecDeque::new(),
                cache_history: VecDeque::new(),
                disk_history: VecDeque::new(),
                decode_reads: VecDeque::new(),
                session: Default::default(),
                load_history: VecDeque::new(),
                composition_history: VecDeque::new(),
                swap_history: VecDeque::new(),
                compression_history: VecDeque::new(),
                platform: crate::host::Platform::current(),
                gpu_history: VecDeque::new(),
                signals: VecDeque::new(),
                request_history: request_history::History::default(),
                operator_history: operator_history::History::default(),
            },
            sampler,
            interval,
            paused: false,
            tab: 0,
            top_sort: TopSort::Rss,
            top_filter: String::new(),
            top_filtering: false,
            top_selected: 0,
            journal_filter: JournalFilter::All,
            journal_scroll: 0,
            request_scroll: 0,
            charts: chart_navigation::Navigation::default(),
            gpu_selected: 0,
            help: false,
            summary_open: false,
            diagnostics_open: false,
            diagnostics_scroll: 0,
            diagnostics_max_scroll: Default::default(),
            quit: false,
            quit_prompt: false,
            sampler_disconnected: false,
            alert: None,
            alert_bells: 0,
            bell: ring_terminal_bell,
            critical_episode: false,
            alarms: config.alarms.unwrap_or(true),
            thresholds,
            settings: Settings::new(config_path()),
            last_click: None,
        }
    }

    pub(crate) fn tick(&mut self) {
        loop {
            match self.sampler.views.try_recv() {
                Ok(view) => {
                    self.track_critical_alert(&view);
                    self.gpu_selected = self
                        .collector
                        .current
                        .gpus
                        .get(self.gpu_selected)
                        .and_then(|selected| {
                            view.current
                                .gpus
                                .iter()
                                .position(|gpu| gpu.uuid == selected.uuid)
                        })
                        .unwrap_or_else(|| {
                            self.gpu_selected
                                .min(view.current.gpus.len().saturating_sub(1))
                        });
                    self.collector = view;
                    self.sampler_disconnected = false;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.sampler_disconnected {
                        diagnostics_log(
                            "ERROR",
                            "sampler_disconnected",
                            "no_more_samples_received",
                        );
                        self.sampler_disconnected = true;
                    }
                    break;
                }
            }
        }
    }

    /// Ring once per critical episode. Acknowledgment silences the episode;
    /// recovery re-arms it. Escalation refreshes the banner without repeating.
    pub(crate) fn track_critical_alert(&mut self, view: &CollectorView) {
        if !self.alarms {
            return;
        }
        let state = critical_state(&view.current);
        if state.is_none() && matches!(view.current.impact.as_str(), "SAMPLING" | "DATA LIMITED") {
            return;
        }
        let newly_critical = state.is_some() && !self.critical_episode;
        self.critical_episode = state.is_some();
        if let Some(alert) = &mut self.alert {
            if let Some(state) = state {
                if alert.state != state {
                    alert.time = view.current.updated.clone();
                }
                alert.state = state.into();
                alert.summary = critical_summary(&view.current);
            } else {
                diagnostics_log(
                    "INFO",
                    "critical_alert_cleared",
                    format!("state={}", log_field(&alert.state)),
                );
                self.alert = None;
            }
            return;
        }
        if let Some(state) = state.filter(|_| newly_critical) {
            let summary = critical_summary(&view.current);
            diagnostics_log(
                "WARN",
                "critical_alert_raised",
                format!("state={} summary={}", log_field(state), log_field(&summary)),
            );
            (self.bell)();
            self.alert_bells += 1;
            self.alert = Some(ActiveAlert {
                state: state.into(),
                summary,
                time: view.current.updated.clone(),
            });
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if self.quit_prompt {
            self.quit_prompt = false;
            let ctrl_c =
                key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
            if ctrl_c || matches!(key.code, KeyCode::Char('y' | 'Y' | 'q') | KeyCode::Enter) {
                self.quit = true;
            }
            return;
        }
        if self.diagnostics_open {
            let scroll = self
                .diagnostics_scroll
                .min(self.diagnostics_max_scroll.get());
            match key.code {
                KeyCode::Char('d') | KeyCode::Esc => self.diagnostics_open = false,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.quit = true
                }
                KeyCode::Up => self.diagnostics_scroll = scroll.saturating_sub(1),
                KeyCode::Down => {
                    self.diagnostics_scroll = scroll
                        .saturating_add(1)
                        .min(self.diagnostics_max_scroll.get())
                }
                KeyCode::PageUp => self.diagnostics_scroll = scroll.saturating_sub(10),
                KeyCode::PageDown => {
                    self.diagnostics_scroll = scroll
                        .saturating_add(10)
                        .min(self.diagnostics_max_scroll.get())
                }
                KeyCode::Home => self.diagnostics_scroll = 0,
                KeyCode::End => self.diagnostics_scroll = self.diagnostics_max_scroll.get(),
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('1' | '2' | '3' | '4') => {
                    self.diagnostics_open = false;
                    self.charts.expanded = false;
                    self.handle_global_key(key);
                }
                KeyCode::Char('?' | 'h') => {
                    self.diagnostics_open = false;
                    self.help = true;
                }
                KeyCode::Char('q' | 'a' | 'p' | ' ') => self.handle_global_key(key),
                _ => {}
            }
            return;
        }
        if self.help {
            if matches!(
                key.code,
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('h')
            ) {
                self.help = false;
            }
            return;
        }
        if self.summary_open {
            match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.quit = true
                }
                KeyCode::Char('q') => self.quit = true,
                _ => self.summary_open = false,
            }
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        // View navigation stays global, including while editing a process filter.
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.top_filtering = false;
            self.settings.editing = None;
            self.charts.expanded = false;
            self.handle_global_key(key);
            return;
        }
        if self.tab == 3 && self.handle_settings_key(key.code) {
            return;
        }
        if self.top_filtering {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => self.top_filtering = false,
                KeyCode::Backspace => {
                    self.top_filter.pop();
                }
                KeyCode::Char(value)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    self.top_filter.push(value);
                }
                _ => {}
            }
            return;
        }
        if self.tab == 1 {
            match key.code {
                KeyCode::Up => {
                    self.top_selected = self.top_selected.saturating_sub(1);
                    return;
                }
                KeyCode::Down => {
                    self.top_selected = self.top_selected.saturating_add(1);
                    return;
                }
                KeyCode::PageUp => {
                    self.top_selected = self.top_selected.saturating_sub(10);
                    return;
                }
                KeyCode::PageDown => {
                    self.top_selected = self.top_selected.saturating_add(10);
                    return;
                }
                KeyCode::Home => {
                    self.top_selected = 0;
                    return;
                }
                KeyCode::End => {
                    self.top_selected = self.filtered_llm_processes().len().saturating_sub(1);
                    return;
                }
                KeyCode::Char('s') => {
                    self.top_sort = self.top_sort.next();
                    self.top_selected = 0;
                    return;
                }
                KeyCode::Char('f') | KeyCode::Char('/') => {
                    self.top_filtering = true;
                    return;
                }
                KeyCode::Char('c') => {
                    self.top_filter.clear();
                    self.top_selected = 0;
                    return;
                }
                _ => {}
            }
        }
        if self.tab == 0 {
            let last = self.collector.request_history.len().saturating_sub(1);
            match key.code {
                KeyCode::Char('[') => self.gpu_selected = self.gpu_selected.saturating_sub(1),
                KeyCode::Char(']') => {
                    self.gpu_selected = self
                        .gpu_selected
                        .saturating_add(1)
                        .min(self.collector.current.gpus.len().saturating_sub(1));
                }
                KeyCode::Up | KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.charts.focused = Chart::Prompt;
                    self.request_scroll = if key.code == KeyCode::Up {
                        self.request_scroll.saturating_sub(1)
                    } else {
                        self.request_scroll.saturating_add(1).min(last)
                    };
                }
                direction @ (KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right) => {
                    if self.charts.expanded {
                        self.charts
                            .cycle(matches!(direction, KeyCode::Up | KeyCode::Left));
                    } else {
                        self.charts.move_focus(direction);
                    }
                }
                KeyCode::Char('+') | KeyCode::Char('=') => self.charts.change_zoom(true),
                KeyCode::Char('-') => self.charts.change_zoom(false),
                KeyCode::Char('0') => self.charts.reset_zoom(),
                KeyCode::Enter => self.charts.expanded = !self.charts.expanded,
                KeyCode::Esc if self.charts.expanded => self.charts.expanded = false,
                KeyCode::PageUp => self.request_scroll = self.request_scroll.saturating_sub(10),
                KeyCode::PageDown => {
                    self.request_scroll = self.request_scroll.saturating_add(10).min(last)
                }
                KeyCode::Home => self.request_scroll = 0,
                KeyCode::End => self.request_scroll = last,
                _ => {
                    self.handle_global_key(key);
                    return;
                }
            }
            return;
        }
        if self.tab == 2 {
            match key.code {
                // The journal is newest-first: Up returns toward the live edge,
                // Down moves toward older records.
                KeyCode::Up => {
                    self.journal_scroll = self.journal_scroll.saturating_sub(1);
                    return;
                }
                KeyCode::Down => {
                    self.journal_scroll = self.journal_scroll.saturating_add(1);
                    return;
                }
                KeyCode::PageUp => {
                    self.journal_scroll = self.journal_scroll.saturating_sub(10);
                    return;
                }
                KeyCode::PageDown => {
                    self.journal_scroll = self.journal_scroll.saturating_add(10);
                    return;
                }
                KeyCode::Home => {
                    self.journal_scroll = 0;
                    return;
                }
                KeyCode::End => {
                    self.journal_scroll = self.filtered_journal_events().len().saturating_sub(1);
                    return;
                }
                KeyCode::Char('f') | KeyCode::Char(']') => {
                    self.journal_filter = self.journal_filter.next();
                    self.journal_scroll = 0;
                    return;
                }
                KeyCode::Char('[') => {
                    self.journal_filter = self.journal_filter.previous();
                    self.journal_scroll = 0;
                    return;
                }
                _ => {}
            }
        }
        self.handle_global_key(key);
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.diagnostics_open {
            let key = match mouse.kind {
                MouseEventKind::ScrollUp => Some(KeyCode::Up),
                MouseEventKind::ScrollDown => Some(KeyCode::Down),
                _ => None,
            };
            if let Some(key) = key {
                self.handle_key(KeyEvent::new(key, KeyModifiers::NONE));
            }
            return;
        }
        if self.help || self.tab != 0 {
            return;
        }
        let Some(chart) = self.charts.at(mouse.column, mouse.row) else {
            return;
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let now = Instant::now();
                let double = self.last_click.is_some_and(|(at, clicked)| {
                    clicked == chart && now.saturating_duration_since(at) <= DOUBLE_CLICK
                });
                self.charts.focused = chart;
                if double {
                    self.charts.expanded = !self.charts.expanded;
                    self.last_click = None;
                } else {
                    self.last_click = Some((now, chart));
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                self.charts.focused = chart;
                self.charts.expanded = !self.charts.expanded;
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.charts.focused = chart;
                self.charts
                    .change_zoom(mouse.kind == MouseEventKind::ScrollUp);
            }
            _ => {}
        }
    }

    pub(crate) fn handle_global_key(&mut self, key: KeyEvent) {
        let previous_tab = self.tab;
        self.handle_global_action(key);
        if self.tab == 3 && previous_tab != 3 {
            self.settings.reload();
        }
    }

    /// A config saved from Settings takes effect now. Interval and history
    /// change only when they are the edited key, so a value given on the
    /// command line stays until that setting itself is changed.
    pub(crate) fn apply_setting(&mut self, key: &str, config: &Config) {
        match key {
            "interval" => {
                self.interval = Duration::from_secs(config_interval(config).0);
                self.sampler
                    .send(SamplerCommand::SetInterval(self.interval));
            }
            "history" => self
                .sampler
                .send(SamplerCommand::SetHistory(config_history(config).0)),
            "alarms" => {
                self.alarms = config.alarms.unwrap_or(true);
                if !self.alarms {
                    self.alert = None;
                    self.critical_episode = false;
                }
            }
            _ => {}
        }
        self.thresholds = Thresholds::from_config(config);
        self.sampler
            .send(SamplerCommand::SetConfig(Box::new(config.clone())));
    }

    fn handle_global_action(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('d') => {
                self.diagnostics_open = true;
                self.diagnostics_scroll = 0;
            }
            KeyCode::Char('u') => self.summary_open = true,
            KeyCode::Char('q') => self.quit = true,
            // Esc backs out of whatever is open; with nothing open it asks first.
            KeyCode::Esc => self.quit_prompt = true,
            KeyCode::Char('a') => {
                if let Some(alert) = self.alert.take() {
                    diagnostics_log(
                        "INFO",
                        "critical_alert_acknowledged",
                        format!("state={}", log_field(&alert.state)),
                    );
                }
            }
            KeyCode::Char('p') | KeyCode::Char(' ') => {
                self.paused = !self.paused;
                self.sampler.send(SamplerCommand::SetPaused(self.paused));
            }
            KeyCode::Char('r') => {
                self.sampler.send(SamplerCommand::Reset);
                self.journal_scroll = 0;
                self.request_scroll = 0;
                self.charts = chart_navigation::Navigation::default();
                self.gpu_selected = 0;
                self.journal_filter = JournalFilter::All;
                self.alert = None;
                self.critical_episode = false;
            }
            KeyCode::Char('t') | KeyCode::Char('2') => {
                self.tab = 1;
                self.top_selected = 0;
            }
            KeyCode::Char('j') | KeyCode::Char('3') => {
                self.tab = 2;
                self.journal_scroll = 0;
            }
            KeyCode::Char('o') | KeyCode::Char('1') => self.tab = 0,
            KeyCode::Char('4') => self.tab = 3,
            KeyCode::Tab | KeyCode::Right => self.tab = (self.tab + 1) % 4,
            KeyCode::BackTab | KeyCode::Left => self.tab = (self.tab + 3) % 4,
            KeyCode::Char('?') | KeyCode::Char('h') => self.help = true,
            KeyCode::Char('}') => {
                let seconds = self.interval.as_secs().saturating_add(1).min(60);
                self.interval = Duration::from_secs(seconds);
                self.sampler
                    .send(SamplerCommand::SetInterval(self.interval));
            }
            KeyCode::Char('{') => {
                let seconds = self.interval.as_secs().saturating_sub(1).max(1);
                self.interval = Duration::from_secs(seconds);
                self.sampler
                    .send(SamplerCommand::SetInterval(self.interval));
            }
            _ => {}
        }
    }

    pub(crate) fn filtered_llm_processes(&self) -> Vec<LlmProcess> {
        let query = self.top_filter.to_ascii_lowercase();
        let mut rows = self
            .collector
            .current
            .llm_processes
            .iter()
            .filter(|process| {
                query.is_empty()
                    || process.name.to_ascii_lowercase().contains(&query)
                    || process.command.to_ascii_lowercase().contains(&query)
            })
            .cloned()
            .collect::<Vec<_>>();
        match self.top_sort {
            TopSort::Rss => rows.sort_by(|left, right| {
                right
                    .rss
                    .cmp(&left.rss)
                    .then_with(|| left.pid.cmp(&right.pid))
            }),
            TopSort::Cpu => rows.sort_by(|left, right| {
                right
                    .cpu
                    .partial_cmp(&left.cpu)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.pid.cmp(&right.pid))
            }),
            TopSort::Pid => rows.sort_by_key(|process| process.pid),
            TopSort::Name => rows.sort_by(|left, right| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
                    .then_with(|| left.pid.cmp(&right.pid))
            }),
        }
        rows
    }

    pub(crate) fn filtered_journal_events(&self) -> Vec<&SignalEvent> {
        self.collector
            .signals
            .iter()
            .filter(|event| self.journal_filter.matches(event.kind))
            .collect()
    }
}
