//! The Settings view: edit `~/.config/mlxtop/config.json` from the dashboard.
//!
//! The file is the source of truth. Settings reads it each time the view
//! opens, changes one key at a time, checks the result still parses as a
//! `Config`, and writes it back atomically, so keys it does not know survive
//! and a file it cannot parse is never overwritten.

use crate::app::App;
use crate::config::{
    Config, COMPRESSION_WARN_EXIT, COMPRESSION_WARN_RATE, GPU_CRITICAL_LOAD, GPU_WARN_EXIT,
    GPU_WARN_LOAD, HISTORY_DEFAULT, HISTORY_MAX, HISTORY_MIN, INTERVAL_DEFAULT, INTERVAL_MAX,
    INTERVAL_MIN, MEMORY_CRITICAL_LOAD, MEMORY_WARN_LOAD, SWAP_CRITICAL_RATE, SWAP_WARN_EXIT,
    SWAP_WARN_RATE,
};
use crate::domain::MIB;
use crate::theme::{CYAN, DIM, GREEN, MUTED, PANEL_RAISED, RED, YELLOW};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use serde_json::{Map, Value};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    /// A whole number within a range, shown with a unit.
    Number {
        min: u64,
        max: u64,
        default: u64,
        unit: &'static str,
    },
    /// Bytes per second in the file, MiB/s on screen.
    Rate {
        default: u64,
    },
    Toggle {
        default: bool,
    },
    Host,
    Port,
}

pub(crate) struct Field {
    /// The JSON key; `omx.host` and `omx.port` live in the `omx` object.
    pub(crate) key: &'static str,
    pub(crate) section: &'static str,
    pub(crate) label: &'static str,
    pub(crate) help: &'static str,
    pub(crate) kind: Kind,
}

const fn percent(default: u64) -> Kind {
    Kind::Number {
        min: 0,
        max: 100,
        default,
        unit: "%",
    }
}

pub(crate) const FIELDS: &[Field] = &[
    Field {
        key: "interval",
        section: "General",
        label: "Refresh interval",
        help: "Seconds between samples. `{` and `}` change it for this session only.",
        kind: Kind::Number {
            min: INTERVAL_MIN,
            max: INTERVAL_MAX,
            default: INTERVAL_DEFAULT,
            unit: "s",
        },
    },
    Field {
        key: "history",
        section: "General",
        label: "History",
        help: "Samples kept for charts and the Journal. A shorter history drops the oldest samples now.",
        kind: Kind::Number {
            min: HISTORY_MIN as u64,
            max: HISTORY_MAX as u64,
            default: HISTORY_DEFAULT as u64,
            unit: "samples",
        },
    },
    Field {
        key: "alarms",
        section: "General",
        label: "Critical alarms",
        help: "The bell and banner for critical memory pressure and heavy paging. Off still shows the condition in charts, the assessment and the Journal.",
        kind: Kind::Toggle { default: true },
    },
    Field {
        key: "journal.prompts",
        section: "Journal",
        label: "New prompts",
        help: "A row for each new request, with its prompt size. Off stops recording new rows; charts are unchanged.",
        kind: Kind::Toggle { default: true },
    },
    Field {
        key: "journal.llm",
        section: "Journal",
        label: "Serving changes",
        help: "Model, status, telemetry source and request start/finish rows. Off stops recording new rows; charts are unchanged.",
        kind: Kind::Toggle { default: true },
    },
    Field {
        key: "journal.queue",
        section: "Journal",
        label: "Queue",
        help: "Requests starting to wait, and the queue clearing. Off stops recording new rows; charts are unchanged.",
        kind: Kind::Toggle { default: true },
    },
    Field {
        key: "journal.memory",
        section: "Journal",
        label: "Memory pressure",
        help: "Pressure level changes. A move to critical pressure is always recorded. Off stops recording new rows; charts are unchanged.",
        kind: Kind::Toggle { default: true },
    },
    Field {
        key: "journal.paging",
        section: "Journal",
        label: "Paging",
        help: "Light paging starting and clearing. Paging findings such as HEAVY PAGING are always recorded. Off stops recording new rows; charts are unchanged.",
        kind: Kind::Toggle { default: true },
    },
    Field {
        key: "journal.gpu",
        section: "Journal",
        label: "GPU load",
        help: "GPU BUSY, saturated and eased rows. GPU findings such as GPU THROTTLED are always recorded. Off stops recording new rows; charts are unchanged.",
        kind: Kind::Toggle { default: true },
    },
    Field {
        key: "journal.thermal",
        section: "Journal",
        label: "Thermal",
        help: "Thermal warning changes. Off stops recording new rows; charts are unchanged.",
        kind: Kind::Toggle { default: true },
    },
    Field {
        key: "omx.host",
        section: "oMLX server",
        label: "Host",
        help: "oMLX server host. Unset, mlxtop reads ~/.config/omlx-coding/server.env, then uses 127.0.0.1.",
        kind: Kind::Host,
    },
    Field {
        key: "omx.port",
        section: "oMLX server",
        label: "Port",
        help: "oMLX server port. Unset, mlxtop reads ~/.config/omlx-coding/server.env, then uses 8080.",
        kind: Kind::Port,
    },
    Field {
        key: "memory_warn_load",
        section: "Thresholds",
        label: "Memory warning",
        help: "Unavailable memory that turns Linux pressure yellow. macOS uses its own pressure level.",
        kind: percent(MEMORY_WARN_LOAD),
    },
    Field {
        key: "memory_critical_load",
        section: "Thresholds",
        label: "Memory critical",
        help: "Unavailable memory that turns Linux pressure red. Never below the warning level.",
        kind: percent(MEMORY_CRITICAL_LOAD),
    },
    Field {
        key: "gpu_warn_load",
        section: "Thresholds",
        label: "GPU busy",
        help: "GPU load reported as busy and shown in yellow.",
        kind: percent(GPU_WARN_LOAD),
    },
    Field {
        key: "gpu_critical_load",
        section: "Thresholds",
        label: "GPU saturated",
        help: "GPU load reported as saturated and shown in red. Never an alarm by itself.",
        kind: percent(GPU_CRITICAL_LOAD),
    },
    Field {
        key: "gpu_warn_exit",
        section: "Thresholds",
        label: "GPU busy clears below",
        help: "GPU load below which GPU BUSY clears.",
        kind: percent(GPU_WARN_EXIT),
    },
    Field {
        key: "swap_warn_rate",
        section: "Thresholds",
        label: "Light paging",
        help: "Swap churn that counts as light paging.",
        kind: Kind::Rate {
            default: SWAP_WARN_RATE,
        },
    },
    Field {
        key: "swap_critical_rate",
        section: "Thresholds",
        label: "Swap thrashing",
        help: "Swap rate, in and out, that counts as thrashing and turns the paging chart red.",
        kind: Kind::Rate {
            default: SWAP_CRITICAL_RATE,
        },
    },
    Field {
        key: "swap_warn_exit",
        section: "Thresholds",
        label: "Paging clears below",
        help: "Swap churn below which PAGING ACTIVE clears.",
        kind: Kind::Rate {
            default: SWAP_WARN_EXIT,
        },
    },
    Field {
        key: "compression_warn_rate",
        section: "Thresholds",
        label: "Compression active",
        help: "Compression churn that raises COMPRESSION ACTIVE.",
        kind: Kind::Rate {
            default: COMPRESSION_WARN_RATE,
        },
    },
    Field {
        key: "compression_warn_exit",
        section: "Thresholds",
        label: "Compression clears below",
        help: "Compression churn below which COMPRESSION ACTIVE clears.",
        kind: Kind::Rate {
            default: COMPRESSION_WARN_EXIT,
        },
    },
];

/// MiB/s without trailing zeros: 1, 0.5, 16.
fn mib(bytes: u64) -> String {
    let text = format!("{:.2}", bytes as f64 / MIB as f64);
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

impl Field {
    /// The value as the user types it, without a unit.
    fn edit_text(&self, value: &Value) -> String {
        match (self.kind, value) {
            (Kind::Rate { .. }, Value::Number(n)) => n.as_u64().map(mib).unwrap_or_default(),
            (_, Value::String(text)) => text.clone(),
            (_, other) => other.to_string(),
        }
    }

    /// The value as shown in the list, with its unit.
    pub(crate) fn display(&self, value: &Value) -> String {
        match (self.kind, value) {
            (Kind::Number { unit: "%", .. }, _) => format!("{}%", self.edit_text(value)),
            (Kind::Number { unit, .. }, _) => format!("{} {unit}", self.edit_text(value)),
            (Kind::Rate { .. }, _) => format!("{} MiB/s", self.edit_text(value)),
            (Kind::Toggle { .. }, Value::Bool(on)) => if *on { "on" } else { "off" }.into(),
            _ => self.edit_text(value),
        }
    }

    pub(crate) fn default_display(&self) -> String {
        match self.kind {
            Kind::Number { default, .. } => self.display(&Value::from(default)),
            Kind::Rate { default } => self.display(&Value::from(default)),
            Kind::Toggle { default } => self.display(&Value::from(default)),
            Kind::Host | Kind::Port => "auto".into(),
        }
    }

    /// Check typed text and turn it into the JSON value the file stores.
    pub(crate) fn parse(&self, text: &str) -> Result<Value, String> {
        let text = text.trim();
        match self.kind {
            Kind::Number { min, max, unit, .. } => match text.parse::<u64>() {
                Ok(n) if (min..=max).contains(&n) => Ok(Value::from(n)),
                _ if unit == "%" => Err(format!("Enter a whole number from {min} to {max}%")),
                _ => Err(format!("Enter a whole number from {min} to {max} {unit}")),
            },
            Kind::Rate { .. } => match text.parse::<f64>() {
                Ok(n) if n.is_finite() && n >= 0.0 && n * (MIB as f64) < u64::MAX as f64 => {
                    Ok(Value::from((n * MIB as f64).round() as u64))
                }
                _ => Err("Enter a rate in MiB/s, such as 1 or 0.5".into()),
            },
            Kind::Port => match text.parse::<u16>() {
                Ok(port) if port > 0 => Ok(Value::from(port)),
                _ => Err("Enter a port from 1 to 65535".into()),
            },
            Kind::Host if !text.is_empty() && !text.contains(char::is_whitespace) => {
                Ok(Value::from(text))
            }
            Kind::Host => Err("Enter a host name or address, without spaces".into()),
            Kind::Toggle { .. } => match text {
                "on" | "true" | "yes" => Ok(Value::Bool(true)),
                "off" | "false" | "no" => Ok(Value::Bool(false)),
                _ => Err("Enter on or off".into()),
            },
        }
    }
}

/// What the view shows below the list after an action.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Status {
    Saved(String),
    Error(String),
}

pub(crate) struct Settings {
    pub(crate) path: PathBuf,
    /// The file's top-level object as last read or written.
    pub(crate) doc: Map<String, Value>,
    /// Why the file cannot be edited here, when it exists but does not parse.
    pub(crate) broken: Option<String>,
    pub(crate) selected: usize,
    /// The text being typed into the selected field.
    pub(crate) editing: Option<String>,
    pub(crate) status: Option<Status>,
}

impl Settings {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            doc: Map::new(),
            broken: None,
            selected: 0,
            editing: None,
            status: None,
        }
    }

    /// Re-read the file, so edits made by hand since the last visit show up.
    pub(crate) fn reload(&mut self) {
        self.editing = None;
        self.status = None;
        self.broken = None;
        self.doc = Map::new();
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                self.broken = Some(format!("Can't read the file: {error}"));
                return;
            }
        };
        if text.trim().is_empty() {
            return;
        }
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(doc)) => {
                if let Err(error) = serde_json::from_value::<Config>(Value::Object(doc.clone())) {
                    self.broken = Some(format!("The file has a value of the wrong type: {error}"));
                } else {
                    self.doc = doc;
                }
            }
            Ok(_) => self.broken = Some("The file is not a JSON object".into()),
            Err(error) => self.broken = Some(format!("The file is not valid JSON: {error}")),
        }
    }

    /// The file's path with the home directory shown as `~`.
    pub(crate) fn display_path(&self) -> String {
        crate::config::home_dir()
            .and_then(|home| {
                self.path
                    .strip_prefix(home)
                    .ok()
                    .map(|rest| rest.to_owned())
            })
            .map(|rest| format!("~/{}", rest.display()))
            .unwrap_or_else(|| self.path.display().to_string())
    }

    pub(crate) fn field(&self) -> &'static Field {
        &FIELDS[self.selected.min(FIELDS.len() - 1)]
    }

    /// The value the file sets for `field`, if any.
    pub(crate) fn value(&self, field: &Field) -> Option<&Value> {
        match field.key.split_once('.') {
            Some((object, key)) => self.doc.get(object)?.get(key),
            None => self.doc.get(field.key),
        }
        .filter(|value| !value.is_null())
    }

    pub(crate) fn move_selection(&mut self, down: bool) {
        self.selected = if down {
            (self.selected + 1).min(FIELDS.len() - 1)
        } else {
            self.selected.saturating_sub(1)
        };
    }

    pub(crate) fn start_editing(&mut self) {
        let field = self.field();
        self.editing = Some(
            self.value(field)
                .map(|value| field.edit_text(value))
                .unwrap_or_default(),
        );
        self.status = None;
    }

    /// Set `field` to `value`, or remove it with `None`, and save the file.
    /// Returns the config the file now holds.
    pub(crate) fn store(&mut self, field: &Field, value: Option<Value>) -> Result<Config, String> {
        if let Some(reason) = &self.broken {
            return Err(format!(
                "{reason}. Fix it by hand; Settings won't overwrite it"
            ));
        }
        let mut doc = self.doc.clone();
        match field.key.split_once('.') {
            Some((object, key)) => {
                let mut inner = match doc.remove(object) {
                    Some(Value::Object(inner)) => inner,
                    _ => Map::new(),
                };
                match value {
                    Some(value) => inner.insert(key.into(), value),
                    None => inner.remove(key),
                };
                if !inner.is_empty() {
                    doc.insert(object.into(), Value::Object(inner));
                }
            }
            None => {
                match value {
                    Some(value) => doc.insert(field.key.into(), value),
                    None => doc.remove(field.key),
                };
            }
        }
        let config = serde_json::from_value::<Config>(Value::Object(doc.clone()))
            .map_err(|error| format!("Not saved: {error}"))?;
        self.write(&doc)
            .map_err(|error| format!("Not saved: {error}"))?;
        self.doc = doc;
        Ok(config)
    }

    /// Write to a temporary file beside the config, then rename it over the
    /// config, so an interrupted save never leaves half a file.
    fn write(&self, doc: &Map<String, Value>) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let mut text = serde_json::to_string_pretty(doc)?;
        text.push('\n');
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, text)?;
        fs::rename(&temporary, &self.path)
    }
}

impl App {
    /// Keys while the Settings view is open. Returns whether the key was used.
    pub(crate) fn handle_settings_key(&mut self, code: crossterm::event::KeyCode) -> bool {
        use crossterm::event::KeyCode;
        if let Some(text) = self.settings.editing.as_mut() {
            match code {
                KeyCode::Esc => self.settings.editing = None,
                KeyCode::Enter => {
                    let text = text.clone();
                    let field = self.settings.field();
                    match field.parse(&text) {
                        Ok(value) => self.save_setting(field, Some(value)),
                        Err(message) => self.settings.status = Some(Status::Error(message)),
                    }
                }
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(c) if text.chars().count() < 64 => text.push(c),
                _ => {}
            }
            return true;
        }
        let field = self.settings.field();
        match code {
            KeyCode::Up => self.settings.move_selection(false),
            KeyCode::Down => self.settings.move_selection(true),
            KeyCode::Home => self.settings.selected = 0,
            KeyCode::End => self.settings.selected = FIELDS.len() - 1,
            KeyCode::Enter | KeyCode::Char(' ') => match field.kind {
                Kind::Toggle { default } => {
                    let on = self
                        .settings
                        .value(field)
                        .and_then(Value::as_bool)
                        .unwrap_or(default);
                    self.save_setting(field, Some(Value::Bool(!on)));
                }
                _ if code == KeyCode::Enter => self.settings.start_editing(),
                _ => return false,
            },
            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => {
                self.save_setting(field, None)
            }
            _ => return false,
        }
        true
    }

    fn save_setting(&mut self, field: &'static Field, value: Option<Value>) {
        let shown = value
            .as_ref()
            .map(|value| field.display(value))
            .unwrap_or_else(|| format!("default ({})", field.default_display()));
        match self.settings.store(field, value) {
            Ok(config) => {
                self.settings.editing = None;
                self.apply_setting(field.key, &config);
                self.settings.status = Some(Status::Saved(format!(
                    "{} set to {shown}, saved and applied",
                    field.label
                )));
            }
            Err(message) => self.settings.status = Some(Status::Error(message)),
        }
    }

    pub(crate) fn draw_settings(&self, frame: &mut Frame, area: Rect) {
        let [list_area, detail_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(6)])
            .areas(area);
        let settings = &self.settings;
        let label_width = FIELDS.iter().map(|f| f.label.len()).max().unwrap_or(0) + 4;
        let mut lines = Vec::new();
        let mut selected_line = 0;
        let mut section = "";
        for (index, field) in FIELDS.iter().enumerate() {
            if field.section != section {
                if !section.is_empty() {
                    lines.push(Line::from(""));
                }
                section = field.section;
                lines.push(Line::from(Span::styled(
                    format!(" {section}"),
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                )));
            }
            let selected = index == settings.selected;
            if selected {
                selected_line = lines.len();
            }
            let row_style = if selected {
                Style::default().bg(PANEL_RAISED)
            } else {
                Style::default()
            };
            let mut spans = vec![
                Span::styled(
                    if selected { " ▶ " } else { "   " },
                    Style::default().fg(CYAN),
                ),
                Span::styled(
                    format!("{:<label_width$}", field.label),
                    Style::default().fg(Color::White),
                ),
            ];
            match (selected, &settings.editing) {
                (true, Some(text)) => {
                    spans.push(Span::styled(
                        format!("{text}▏"),
                        Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
                    ));
                    if let Kind::Rate { .. } = field.kind {
                        spans.push(Span::styled(" MiB/s", Style::default().fg(MUTED)));
                    }
                }
                _ => match settings.value(field) {
                    Some(value) => spans.push(Span::styled(
                        format!("{:<16}", field.display(value)),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    )),
                    None => spans.push(Span::styled(
                        format!("{:<16}", field.default_display()),
                        Style::default().fg(MUTED),
                    )),
                },
            }
            if settings.editing.is_none() || !selected {
                spans.push(Span::styled(
                    if settings.value(field).is_some() {
                        format!("default {}", field.default_display())
                    } else {
                        "default".into()
                    },
                    Style::default().fg(DIM),
                ));
            }
            lines.push(Line::from(spans).style(row_style));
        }
        let visible = usize::from(list_area.height.saturating_sub(2));
        let scroll = (selected_line + 1).saturating_sub(visible);
        frame.render_widget(
            Paragraph::new(lines)
                .scroll((u16::try_from(scroll).unwrap_or(0), 0))
                .block(
                    Block::default()
                        .title(" settings ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(DIM)),
                ),
            list_area,
        );

        let field = settings.field();
        let mut detail = vec![Line::from(Span::styled(
            field.help,
            Style::default().fg(Color::White),
        ))];
        detail.push(Line::from(Span::styled(
            if settings.editing.is_some() {
                "Enter save · Esc cancel"
            } else if matches!(field.kind, Kind::Toggle { .. }) {
                "Enter / Space toggle · x reset to default · changes apply now and are saved"
            } else {
                "Enter edit · x reset to default · changes apply now and are saved"
            },
            Style::default().fg(MUTED),
        )));
        if let Some(reason) = &settings.broken {
            detail.push(Line::from(Span::styled(
                format!("{reason}. Fix it by hand; Settings won't overwrite it."),
                Style::default().fg(RED),
            )));
        } else if let Some(status) = &settings.status {
            detail.push(match status {
                Status::Saved(text) => Line::from(Span::styled(
                    format!("✓ {text}"),
                    Style::default().fg(GREEN),
                )),
                Status::Error(text) => {
                    Line::from(Span::styled(format!("✖ {text}"), Style::default().fg(RED)))
                }
            });
        }
        frame.render_widget(
            Paragraph::new(detail).wrap(Wrap { trim: true }).block(
                Block::default()
                    .title(format!(" {} ", settings.display_path()))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(DIM)),
            ),
            detail_area,
        );
    }
}

#[cfg(test)]
#[path = "tests/settings.rs"]
mod tests;
