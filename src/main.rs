// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

//! A dark terminal monitor for macOS memory pressure and local LLM serving.
//!
//! The collector owns the sampling path used by both the interactive TUI and
//! the static report. Provider-specific telemetry is optional; missing data
//! remains explicitly unavailable instead of being inferred.

use std::any::Any;
use std::backtrace::Backtrace;
use std::collections::VecDeque;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, stdout, IsTerminal, Read, Seek, SeekFrom, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table, TableState, Tabs, Wrap,
    },
    Frame, Terminal,
};

const GREEN: Color = Color::Rgb(88, 211, 147);
const YELLOW: Color = Color::Rgb(246, 193, 79);
const RED: Color = Color::Rgb(248, 113, 113);
const CYAN: Color = Color::Rgb(90, 202, 225);
const BLUE: Color = Color::Rgb(120, 153, 255);
const MUTED: Color = Color::Rgb(139, 151, 168);
const DIM: Color = Color::Rgb(78, 90, 108);
const PANEL: Color = Color::Rgb(18, 24, 34);
const PANEL_RAISED: Color = Color::Rgb(24, 32, 46);
const EDGE: Color = Color::Rgb(54, 68, 88);
const VERSION: &str = env!("CARGO_PKG_VERSION");
const MIN_CHART_HEIGHT: u16 = 4;
const CHART_GRID_ROWS: u16 = 2;
const MIB: u64 = 1024 * 1024;
const MEMORY_WARN_LOAD: u64 = 70;
const MEMORY_CRITICAL_LOAD: u64 = 85;
const GPU_WARN_LOAD: u64 = 75;
const GPU_CRITICAL_LOAD: u64 = 90;
const SWAP_WARN_RATE: u64 = MIB;
const SWAP_CRITICAL_RATE: u64 = 16 * MIB;
const COMPRESSION_WARN_RATE: u64 = 64 * MIB;
const SWAP_WARN_EXIT: u64 = 2 * MIB;
const COMPRESSION_WARN_EXIT: u64 = 32 * MIB;
const GPU_WARN_EXIT: u64 = 70;
const CORRELATION_HISTORY_LIMIT: usize = 16;
const THROUGHPUT_CHANGE_MIN_TPS: f64 = 2.0;
const THROUGHPUT_CHANGE_RATIO: f64 = 0.10;
const GENERATION_CHART_SCALE_MAX: u64 = 1000;
const PREFILL_CHART_SCALE_MAX: u64 = 2000;
const CHART_VISUAL_DEADBAND_FRACTION: f64 = 0.25;
// Paging bursts span KiB/s to far beyond the critical rate, so the chart
// plots them on a log curve that tops out at the critical rate; the anchor
// keeps sub-anchor churn at zero instead of a solid bottom-row wall.
const SWAP_CHART_SCALE: u64 = SWAP_CRITICAL_RATE;
const SWAP_CHART_LOG_ANCHOR: u64 = 64 * 1024;
const CONTEXT_GROWTH_TOKENS: u64 = 1024;
const MODEL_MEMORY_GROWTH: u64 = 256 * MIB;
const MAX_HTTP_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(1);
const DIAGNOSTICS_LOG_ENV: &str = "MLXTOP_LOG_PATH";
const DIAGNOSTICS_MAX_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_OMLX_HOST: &str = "127.0.0.1";
const DEFAULT_OMLX_PORT: u16 = 8080;
const LLM_PROCESS_MARKERS: &[&str] = &[
    "omlx",
    "mlx-lm",
    "ollama",
    "llama-server",
    "llama.cpp",
    "lm studio",
    "koboldcpp",
    "localai",
];

static DIAGNOSTICS: OnceLock<Diagnostics> = OnceLock::new();

struct Diagnostics {
    path: PathBuf,
    file: Mutex<File>,
}

impl Diagnostics {
    fn log(&self, level: &str, event: &str, details: &str) {
        let details = details.replace(['\r', '\n'], "\\n");
        if let Ok(mut file) = self.file.lock() {
            if file
                .metadata()
                .map(|metadata| metadata.len() >= DIAGNOSTICS_MAX_BYTES)
                .unwrap_or(false)
            {
                let rotated = self.path.with_extension("log.1");
                let _ = file.flush();
                if fs::rename(&self.path, rotated).is_ok() {
                    if let Ok(replacement) = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&self.path)
                    {
                        *file = replacement;
                    }
                } else if file.set_len(0).is_ok() {
                    let _ = file.seek(SeekFrom::Start(0));
                }
            }
            let _ = writeln!(
                file,
                "ts_ms={} level={} event={} {}",
                diagnostics_timestamp_ms(),
                level,
                event,
                details
            );
            let _ = file.flush();
        }
    }
}

fn init_diagnostics() -> Option<&'static Diagnostics> {
    if DIAGNOSTICS.get().is_none() {
        let path = diagnostics_path()?;
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).ok()?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        let _ = DIAGNOSTICS.set(Diagnostics {
            path,
            file: Mutex::new(file),
        });
    }
    let diagnostics = DIAGNOSTICS.get()?;
    diagnostics.log(
        "INFO",
        "session_start",
        &format!(
            "version={} pid={} log_path={}",
            VERSION,
            std::process::id(),
            log_field(&diagnostics.path.display().to_string())
        ),
    );
    Some(diagnostics)
}

fn diagnostics_path() -> Option<PathBuf> {
    if let Ok(path) = env::var(DIAGNOSTICS_LOG_ENV) {
        let path = path.trim();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Logs/mlxtop/mlxtop.log"))
        .or_else(|| Some(PathBuf::from("mlxtop.log")))
}

fn diagnostics_log(level: &str, event: &str, details: impl AsRef<str>) {
    if let Some(diagnostics) = DIAGNOSTICS.get() {
        diagnostics.log(level, event, details.as_ref());
    }
}

fn diagnostics_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn log_field(value: &str) -> String {
    let mut field = String::new();
    for character in value.chars().take(160) {
        if character.is_ascii_alphanumeric()
            || matches!(character, '-' | '_' | '.' | '/' | ':' | '%' | '@')
        {
            field.push(character);
        } else if character.is_whitespace() {
            field.push('_');
        } else {
            field.push('?');
        }
    }
    field
}

fn log_optional_f64(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map(|value| format!("{value:.3}"))
        .unwrap_or_else(|| "na".into())
}

fn log_optional_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "na".into())
}

fn log_optional_u8(value: Option<u8>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "na".into())
}

fn panic_payload(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".into()
    }
}

fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "unknown".into());
        let message = panic_payload(info.payload());
        let backtrace = Backtrace::force_capture();
        diagnostics_log(
            "ERROR",
            "panic",
            format!(
                "message={} location={} backtrace={backtrace}",
                log_field(&message),
                log_field(&location),
            ),
        );
        previous(info);
    }));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Green,
    Yellow,
    Red,
    Cyan,
    Blue,
    Muted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JournalFilter {
    All,
    Llm,
    Pressure,
    Paging,
    Gpu,
    Thermal,
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TelemetrySource {
    #[default]
    None,
    Live,
    Log,
}

impl TelemetrySource {
    fn label(self) -> &'static str {
        match self {
            Self::None => "unavailable",
            Self::Live => "live API",
            Self::Log => "completion log",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EventKind {
    System,
    Llm,
    Queue,
    Pressure,
    Paging,
    Gpu,
    Thermal,
}

impl EventKind {
    fn from_state(state: &str) -> Self {
        match state {
            "LLM" => Self::Llm,
            "QUEUE" => Self::Queue,
            "PRESSURE" | "MEMORY BOTTLENECK" | "MEMORY STRESS" => Self::Pressure,
            "PAGING" | "SWAP THRASHING" | "HEAVY PAGING" | "PAGE-IN RECOVERY" | "PAGING ACTIVE"
            | "WATCH PAGING" => Self::Paging,
            "GPU" => Self::Gpu,
            "THERMAL" => Self::Thermal,
            _ => Self::System,
        }
    }
}

impl JournalFilter {
    fn label(self) -> &'static str {
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

    fn next(self) -> Self {
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

    fn previous(self) -> Self {
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

    fn matches(self, kind: EventKind) -> bool {
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

impl Tone {
    fn color(self) -> Color {
        match self {
            Self::Green => GREEN,
            Self::Yellow => YELLOW,
            Self::Red => RED,
            Self::Cyan => CYAN,
            Self::Blue => BLUE,
            Self::Muted => MUTED,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Green => "✓",
            Self::Yellow => "!",
            Self::Red => "×",
            Self::Cyan | Self::Blue => "•",
            Self::Muted => "·",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChartMetric {
    Generation,
    Prefill,
    Cache,
    Memory,
    Swap,
    Gpu,
}

impl ChartMetric {
    fn chart_tone(self) -> Tone {
        match self {
            Self::Generation | Self::Prefill | Self::Cache => Tone::Cyan,
            Self::Memory => Tone::Cyan,
            Self::Swap => Tone::Yellow,
            Self::Gpu => Tone::Blue,
        }
    }

    fn tone(self, value: u64) -> Tone {
        match self {
            Self::Generation | Self::Prefill | Self::Cache => Tone::Cyan,
            Self::Memory => match value {
                MEMORY_CRITICAL_LOAD.. => Tone::Red,
                MEMORY_WARN_LOAD..MEMORY_CRITICAL_LOAD => Tone::Yellow,
                _ => Tone::Green,
            },
            Self::Gpu => match value {
                GPU_CRITICAL_LOAD.. => Tone::Red,
                GPU_WARN_LOAD..GPU_CRITICAL_LOAD => Tone::Yellow,
                _ => Tone::Green,
            },
            Self::Swap => {
                if value >= SWAP_CRITICAL_RATE {
                    Tone::Red
                } else if value >= SWAP_WARN_RATE {
                    Tone::Yellow
                } else {
                    Tone::Green
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct ChartPoint {
    value: Option<u64>,
    tone: Tone,
}

#[cfg(test)]
impl ChartPoint {
    fn new(value: Option<u64>, tone: Tone) -> Self {
        Self { value, tone }
    }
}

#[derive(Clone, Copy)]
struct TraceCell {
    glyph: char,
    tone: Tone,
}

#[derive(Clone, Copy)]
struct RenderPoint {
    value: Option<u64>,
    tone: Tone,
    break_before: bool,
}

#[derive(Default)]
struct Consumer {
    name: String,
    rss: u64,
    processes: u32,
}

#[derive(Clone)]
struct LlmProcess {
    pid: u32,
    name: String,
    command: String,
    rss: u64,
    cpu: f64,
    memory_percent: Option<f64>,
    state: String,
    pageins: Option<u64>,
    pagein_rate: Option<f64>,
}

struct ProcessSnapshot {
    llm_count: u32,
    llm_rss: u64,
    llm_cpu: f64,
    top_llm: Option<LlmProcess>,
    provider: Option<String>,
    largest_consumer: Option<String>,
    llm_processes: Vec<LlmProcess>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TopSort {
    Rss,
    Cpu,
    Pid,
    Name,
}

impl TopSort {
    fn label(self) -> &'static str {
        match self {
            Self::Rss => "RSS",
            Self::Cpu => "CPU",
            Self::Pid => "PID",
            Self::Name => "NAME",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Rss => Self::Cpu,
            Self::Cpu => Self::Pid,
            Self::Pid => Self::Name,
            Self::Name => Self::Rss,
        }
    }
}

#[derive(Clone)]
struct SignalEvent {
    time: String,
    recorded_at: SystemTime,
    kind: EventKind,
    state: String,
    summary: String,
    tone: Tone,
}

#[derive(Default)]
struct LlmLogStats {
    model: Option<String>,
    tokens_per_second: Option<f64>,
    output_tokens: Option<u64>,
    prompt_tokens: Option<u64>,
    observed_at: Option<SystemTime>,
}

/// MLX allocator and device counters reported by the serving runtime.
///
/// MLX keeps allocator state inside the process that owns the arrays.  The
/// monitor therefore never invents these values from RSS or GPU utilization;
/// they are populated only from a provider endpoint that can observe the
/// serving process directly.
#[derive(Clone, Default)]
struct MlxTelemetry {
    version: Option<String>,
    active_memory: Option<u64>,
    cache_memory: Option<u64>,
    peak_memory: Option<u64>,
    device_name: Option<String>,
    architecture: Option<String>,
    memory_size: Option<u64>,
    recommended_working_set: Option<u64>,
    max_buffer_size: Option<u64>,
    resource_limit: Option<u64>,
    process_footprint: Option<u64>,
}

#[derive(Clone, Default)]
struct MetalTelemetry {
    device_name: Option<String>,
    architecture: Option<String>,
    gpu_cores: Option<u16>,
    renderer_util: Option<u8>,
    tiler_util: Option<u8>,
    resource_limit: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ThroughputDirection {
    #[default]
    Unknown,
    Down,
    Up,
    Flat,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CorrelationCause {
    #[default]
    None,
    Paging,
    Compression,
    MemoryPressure,
    Thermal,
    MetalMemory,
    GpuSaturation,
    Queueing,
    ContextGrowth,
    ModelMemory,
    Runtime,
}

impl CorrelationCause {
    fn label(self) -> &'static str {
        match self {
            Self::None => "no correlated cause",
            Self::Paging => "paging",
            Self::Compression => "compression churn",
            Self::MemoryPressure => "memory pressure",
            Self::Thermal => "thermal limiting",
            Self::MetalMemory => "Metal memory pressure",
            Self::GpuSaturation => "GPU saturation",
            Self::Queueing => "queueing",
            Self::ContextGrowth => "context/KV growth",
            Self::ModelMemory => "model memory growth",
            Self::Runtime => "workload/runtime change",
        }
    }

    fn tone(self) -> Tone {
        match self {
            Self::Paging | Self::MemoryPressure | Self::Thermal => Tone::Red,
            Self::Compression
            | Self::MetalMemory
            | Self::GpuSaturation
            | Self::Queueing
            | Self::ContextGrowth
            | Self::ModelMemory
            | Self::Runtime => Tone::Yellow,
            Self::None => Tone::Muted,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CorrelationKey {
    direction: ThroughputDirection,
    cause: CorrelationCause,
}

#[derive(Clone, Default)]
struct CorrelationInsight {
    direction: ThroughputDirection,
    cause: CorrelationCause,
    confidence: u8,
    summary: String,
    details: String,
    event_key: Option<CorrelationKey>,
}

impl CorrelationInsight {
    fn is_material_drop(&self) -> bool {
        self.direction == ThroughputDirection::Down
    }

    fn tone(&self) -> Tone {
        match self.direction {
            ThroughputDirection::Down => {
                if matches!(
                    self.cause,
                    CorrelationCause::Paging
                        | CorrelationCause::MemoryPressure
                        | CorrelationCause::Thermal
                ) {
                    Tone::Red
                } else {
                    Tone::Yellow
                }
            }
            ThroughputDirection::Up => Tone::Green,
            ThroughputDirection::Unknown | ThroughputDirection::Flat => self.cause.tone(),
        }
    }

    fn confidence_label(&self) -> &'static str {
        match self.confidence {
            80..=u8::MAX => "high",
            55..=79 => "medium",
            1..=54 => "low",
            _ => "unavailable",
        }
    }
}

#[derive(Clone, Default)]
struct CorrelationObservation {
    provider: String,
    model: String,
    generation_tps: Option<f64>,
    gpu_util: Option<u8>,
    renderer_util: Option<u8>,
    tiler_util: Option<u8>,
    paging_rate: u64,
    compression_rate: u64,
    pressure: u8,
    thermal_limited: bool,
    model_memory: Option<u64>,
    model_memory_max: Option<u64>,
    metal_in_use: Option<u64>,
    metal_alloc: Option<u64>,
    context_tokens: Option<u64>,
    active_requests: Option<u64>,
    waiting_requests: Option<u64>,
}

#[derive(Default)]
struct CorrelationEngine {
    observations: VecDeque<CorrelationObservation>,
}

#[derive(Clone, Default)]
struct LlmTelemetry {
    source: TelemetrySource,
    observed_at: Option<SystemTime>,
    provider: Option<String>,
    status: Option<String>,
    model: Option<String>,
    generation_tps: Option<f64>,
    generation_tps_live: bool,
    prefill_tps: Option<f64>,
    prefill_tps_live: bool,
    output_tokens: Option<u64>,
    prompt_tokens: Option<u64>,
    cache_efficiency: Option<f64>,
    prefix_hit_rate: Option<f64>,
    total_prompt_tokens: Option<u64>,
    total_cached_tokens: Option<u64>,
    active_requests: Option<u64>,
    waiting_requests: Option<u64>,
    model_memory: Option<u64>,
    model_memory_max: Option<u64>,
    mlx: MlxTelemetry,
}

struct LlmTelemetryClient {
    host: String,
    port: u16,
    session_cookie: Option<String>,
    cached: Option<LlmTelemetry>,
    mlx_metadata: MlxTelemetry,
    last_stats_available: Option<bool>,
    next_metadata_poll: Instant,
    next_poll: Instant,
    retry_backoff: Duration,
}

#[derive(Clone)]
struct Sample {
    updated: String,
    pressure: String,
    pressure_meaning: String,
    pressure_tone: Tone,
    availability: Option<u8>,
    total_memory: u64,
    wired: u64,
    compressor: u64,
    compressed_logical: u64,
    anonymous: u64,
    file_backed: u64,
    swap_total: u64,
    swap_used: u64,
    swap_available: bool,
    swap_in: u64,
    swap_out: u64,
    swap_growth: i64,
    compress: u64,
    decompress: u64,
    reactivated: u64,
    vm_available: bool,
    gpu_util: Option<u8>,
    gpu_alloc: Option<u64>,
    gpu_in_use: Option<u64>,
    metal: MetalTelemetry,
    mlx: MlxTelemetry,
    thermal: String,
    llm_count: u32,
    llm_rss: u64,
    llm_cpu: f64,
    llm_processes: Vec<LlmProcess>,
    largest_consumer: Option<String>,
    llm_model: String,
    llm_provider: String,
    llm_status: String,
    llm_source: TelemetrySource,
    llm_observed_at: Option<SystemTime>,
    llm_generation_tps: Option<f64>,
    llm_generation_tps_live: bool,
    llm_prefill_tps: Option<f64>,
    llm_prefill_tps_live: bool,
    llm_output_tokens: Option<u64>,
    llm_prompt_tokens: Option<u64>,
    llm_cache_efficiency: Option<f64>,
    llm_cache_interval_efficiency: Option<f64>,
    llm_prefix_hit_rate: Option<f64>,
    llm_active_requests: Option<u64>,
    llm_waiting_requests: Option<u64>,
    llm_model_memory: Option<u64>,
    llm_model_memory_max: Option<u64>,
    correlation: CorrelationInsight,
    llm_top: String,
    llm_pid: u32,
    impact: String,
    impact_tone: Tone,
    health: Option<u16>,
    grade: String,
    limiter: String,
    guidance_badge: String,
    guidance_cause: String,
    guidance_action: String,
    rate_ready: bool,
}

impl Default for Sample {
    fn default() -> Self {
        Self {
            updated: "waiting".into(),
            pressure: "UNKNOWN".into(),
            pressure_meaning: "unavailable".into(),
            pressure_tone: Tone::Muted,
            availability: None,
            total_memory: 0,
            wired: 0,
            compressor: 0,
            compressed_logical: 0,
            anonymous: 0,
            file_backed: 0,
            swap_total: 0,
            swap_used: 0,
            swap_available: false,
            swap_in: 0,
            swap_out: 0,
            swap_growth: 0,
            compress: 0,
            decompress: 0,
            reactivated: 0,
            vm_available: false,
            gpu_util: None,
            gpu_alloc: None,
            gpu_in_use: None,
            metal: MetalTelemetry::default(),
            mlx: MlxTelemetry::default(),
            thermal: "unavailable".into(),
            llm_count: 0,
            llm_rss: 0,
            llm_cpu: 0.0,
            llm_processes: Vec::new(),
            largest_consumer: None,
            llm_model: "not detected".into(),
            llm_provider: "not detected".into(),
            llm_status: "offline".into(),
            llm_source: TelemetrySource::None,
            llm_observed_at: None,
            llm_generation_tps: None,
            llm_generation_tps_live: false,
            llm_prefill_tps: None,
            llm_prefill_tps_live: false,
            llm_output_tokens: None,
            llm_prompt_tokens: None,
            llm_cache_efficiency: None,
            llm_cache_interval_efficiency: None,
            llm_prefix_hit_rate: None,
            llm_active_requests: None,
            llm_waiting_requests: None,
            llm_model_memory: None,
            llm_model_memory_max: None,
            correlation: CorrelationInsight::default(),
            llm_top: "none".into(),
            llm_pid: 0,
            impact: "SAMPLING".into(),
            impact_tone: Tone::Cyan,
            health: None,
            grade: "SAMPLING".into(),
            limiter: "collecting baseline".into(),
            guidance_badge: "WAIT".into(),
            guidance_cause: "Collecting the live-rate baseline.".into(),
            guidance_action: "Wait one refresh before acting.".into(),
            rate_ready: false,
        }
    }
}

struct PreviousCounters {
    at: Instant,
    swapins: u64,
    swapouts: u64,
    compressions: u64,
    decompressions: u64,
    reactivations: u64,
    swap_used: u64,
}

#[derive(Clone)]
struct CollectorView {
    current: Sample,
    generation_history: VecDeque<ChartPoint>,
    prefill_history: VecDeque<ChartPoint>,
    cache_history: VecDeque<ChartPoint>,
    load_history: VecDeque<ChartPoint>,
    swap_history: VecDeque<ChartPoint>,
    gpu_history: VecDeque<ChartPoint>,
    signals: VecDeque<SignalEvent>,
}

#[derive(Default)]
struct VmCounters {
    wired: u64,
    compressor: u64,
    compressed_logical: u64,
    anonymous: u64,
    file_backed: u64,
    swapins: u64,
    swapouts: u64,
    compressions: u64,
    decompressions: u64,
    reactivations: u64,
}

#[derive(Clone, Copy)]
struct CacheCounters {
    prompt_tokens: u64,
    cached_tokens: u64,
}

struct Collector {
    page_size: u64,
    total_memory: u64,
    metal: MetalTelemetry,
    llm_client: LlmTelemetryClient,
    correlation: CorrelationEngine,
    previous: Option<PreviousCounters>,
    previous_llm_cache: Option<CacheCounters>,
    current: Sample,
    generation_history: VecDeque<ChartPoint>,
    prefill_history: VecDeque<ChartPoint>,
    cache_history: VecDeque<ChartPoint>,
    load_history: VecDeque<ChartPoint>,
    swap_history: VecDeque<ChartPoint>,
    gpu_history: VecDeque<ChartPoint>,
    signals: VecDeque<SignalEvent>,
    history_limit: usize,
}

enum SamplerCommand {
    SetPaused(bool),
    SetInterval(Duration),
    Reset,
    Stop,
}

struct Sampler {
    commands: Sender<SamplerCommand>,
    views: Receiver<CollectorView>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Sampler {
    fn spawn(interval: Duration, history_limit: usize) -> Self {
        diagnostics_log(
            "INFO",
            "sampler_start",
            format!(
                "interval_seconds={} history_limit={history_limit}",
                interval.as_secs()
            ),
        );
        let (command_tx, command_rx) = mpsc::channel();
        let (view_tx, view_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut collector = Collector::new(history_limit);
            let mut interval = interval;
            let mut paused = false;
            let mut next_sample = Instant::now();

            loop {
                if !paused && Instant::now() >= next_sample {
                    let sampled = panic::catch_unwind(AssertUnwindSafe(|| collector.sample()));
                    if let Err(payload) = sampled.as_ref() {
                        diagnostics_log(
                            "ERROR",
                            "sampler_panic",
                            format!("message={}", log_field(&panic_payload(payload.as_ref()))),
                        );
                    }
                    if sampled.is_err() {
                        break;
                    }
                    if view_tx.send(collector.view()).is_err() {
                        diagnostics_log("INFO", "sampler_stop", "reason=view_receiver_closed");
                        break;
                    }
                    next_sample = Instant::now() + interval;
                }

                let wait = if paused {
                    Duration::from_millis(100)
                } else {
                    next_sample
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(100))
                };
                match command_rx.recv_timeout(wait) {
                    Ok(SamplerCommand::SetPaused(value)) => {
                        paused = value;
                        diagnostics_log("INFO", "sampler_paused", format!("paused={paused}"));
                        if !paused {
                            next_sample = Instant::now();
                        }
                    }
                    Ok(SamplerCommand::SetInterval(value)) => {
                        interval = value;
                        diagnostics_log(
                            "INFO",
                            "sampler_interval_changed",
                            format!("interval_seconds={}", interval.as_secs()),
                        );
                        next_sample = Instant::now() + interval;
                    }
                    Ok(SamplerCommand::Reset) => {
                        collector.reset();
                        diagnostics_log("INFO", "sampler_reset", "history_and_baselines_cleared");
                        next_sample = Instant::now();
                    }
                    Ok(SamplerCommand::Stop) => {
                        diagnostics_log("INFO", "sampler_stop", "reason=shutdown");
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        diagnostics_log("WARN", "sampler_stop", "reason=command_channel_closed");
                        break;
                    }
                }
            }
        });
        Self {
            commands: command_tx,
            views: view_rx,
            handle: Some(handle),
        }
    }

    fn send(&self, command: SamplerCommand) {
        let _ = self.commands.send(command);
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        let _ = self.commands.send(SamplerCommand::Stop);
        if let Some(handle) = self.handle.take() {
            if let Err(payload) = handle.join() {
                diagnostics_log(
                    "ERROR",
                    "sampler_thread_panic",
                    format!("message={}", log_field(&panic_payload(payload.as_ref()))),
                );
            }
        }
    }
}

impl Collector {
    fn new(history_limit: usize) -> Self {
        let total_memory = command_u64("/usr/sbin/sysctl", &["-n", "hw.memsize"]).unwrap_or(0);
        let mut metal = parse_metal_hardware(
            &command_text(
                "/usr/sbin/ioreg",
                &["-r", "-d", "1", "-w", "0", "-c", "IOAccelerator"],
            )
            .unwrap_or_default(),
        );
        metal.architecture = command_text("/usr/sbin/sysctl", &["-n", "hw.machine"])
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        metal.resource_limit = command_u64("/usr/sbin/sysctl", &["-n", "iogpu.wired_limit_mb"])
            .filter(|value| *value > 0)
            .map(|value| value.saturating_mul(MIB));
        Self {
            page_size: command_u64("/usr/sbin/sysctl", &["-n", "hw.pagesize"]).unwrap_or(16_384),
            total_memory,
            metal,
            llm_client: LlmTelemetryClient::new(),
            correlation: CorrelationEngine::default(),
            previous: None,
            previous_llm_cache: None,
            current: Sample::default(),
            generation_history: VecDeque::with_capacity(history_limit),
            prefill_history: VecDeque::with_capacity(history_limit),
            cache_history: VecDeque::with_capacity(history_limit),
            load_history: VecDeque::with_capacity(history_limit),
            swap_history: VecDeque::with_capacity(history_limit),
            gpu_history: VecDeque::with_capacity(history_limit),
            signals: VecDeque::with_capacity(8),
            history_limit,
        }
    }

    fn sample(&mut self) -> Sample {
        let sample_started = Instant::now();
        let now = sample_started;
        let mut sample = Sample {
            total_memory: self.total_memory,
            metal: self.metal.clone(),
            ..Sample::default()
        };

        let level = command_text(
            "/usr/sbin/sysctl",
            &["-n", "kern.memorystatus_vm_pressure_level"],
        )
        .unwrap_or_default();
        match level.trim() {
            "1" => {
                sample.pressure = "GREEN".into();
                sample.pressure_meaning = "normal".into();
                sample.pressure_tone = Tone::Green;
            }
            "2" => {
                sample.pressure = "YELLOW".into();
                sample.pressure_meaning = "warning".into();
                sample.pressure_tone = Tone::Yellow;
            }
            "4" => {
                sample.pressure = "RED".into();
                sample.pressure_meaning = "critical".into();
                sample.pressure_tone = Tone::Red;
            }
            _ => {}
        }

        if let Some(output) = command_text("/usr/bin/memory_pressure", &["-Q"]) {
            if let Some(line) = output.lines().find(|line| line.contains("free percentage")) {
                sample.availability = line
                    .split(|c: char| !c.is_ascii_digit())
                    .find(|v| !v.is_empty())
                    .and_then(|v| v.parse::<u8>().ok());
            }
        }

        let vm = command_text("/usr/bin/vm_stat", &[]).unwrap_or_default();
        sample.vm_available = !vm.trim().is_empty();
        let counters = parse_vm_stat(&vm, self.page_size);
        sample.wired = counters.wired;
        sample.compressor = counters.compressor;
        sample.compressed_logical = counters.compressed_logical;
        sample.anonymous = counters.anonymous;
        sample.file_backed = counters.file_backed;
        let swap_usage = command_text("/usr/sbin/sysctl", &["-n", "vm.swapusage"]);
        sample.swap_available = swap_usage.is_some();
        (sample.swap_total, sample.swap_used) = parse_swap_usage(&swap_usage.unwrap_or_default());
        let process_elapsed = self
            .previous
            .as_ref()
            .map(|previous| now.duration_since(previous.at));

        if let Some(previous) = &self.previous {
            let elapsed = now.duration_since(previous.at);
            sample.swap_in = rate_bytes(
                delta(counters.swapins, previous.swapins),
                self.page_size,
                elapsed,
            );
            sample.swap_out = rate_bytes(
                delta(counters.swapouts, previous.swapouts),
                self.page_size,
                elapsed,
            );
            sample.compress = rate_bytes(
                delta(counters.compressions, previous.compressions),
                self.page_size,
                elapsed,
            );
            sample.decompress = rate_bytes(
                delta(counters.decompressions, previous.decompressions),
                self.page_size,
                elapsed,
            );
            sample.reactivated = rate_bytes(
                delta(counters.reactivations, previous.reactivations),
                self.page_size,
                elapsed,
            );
            sample.swap_growth = signed_rate_bytes(sample.swap_used, previous.swap_used, elapsed);
            sample.rate_ready = true;
        }
        self.previous = Some(PreviousCounters {
            at: now,
            swapins: counters.swapins,
            swapouts: counters.swapouts,
            compressions: counters.compressions,
            decompressions: counters.decompressions,
            reactivations: counters.reactivations,
            swap_used: sample.swap_used,
        });

        let ioreg = command_text(
            "/usr/sbin/ioreg",
            &["-r", "-d", "1", "-w", "0", "-c", "IOAccelerator"],
        )
        .unwrap_or_default();
        (sample.gpu_util, sample.gpu_alloc, sample.gpu_in_use) = parse_gpu(&ioreg);
        let live_metal = parse_metal_hardware(&ioreg);
        sample.metal.device_name = live_metal.device_name.or(sample.metal.device_name);
        sample.metal.gpu_cores = live_metal.gpu_cores.or(sample.metal.gpu_cores);
        sample.metal.renderer_util = live_metal.renderer_util;
        sample.metal.tiler_util = live_metal.tiler_util;
        sample.thermal =
            parse_thermal(&command_text("/usr/bin/pmset", &["-g", "therm"]).unwrap_or_default());

        let process_snapshot = parse_processes(
            &command_text(
                "/bin/ps",
                &["-axo", "pid=,rss=,%cpu=,%mem=,state=,pagein=,comm=,args="],
            )
            .unwrap_or_default(),
        );
        sample.llm_count = process_snapshot.llm_count;
        sample.llm_rss = process_snapshot.llm_rss;
        sample.llm_cpu = process_snapshot.llm_cpu;
        let detected_provider = process_snapshot.provider.clone();
        sample.llm_pid = process_snapshot
            .top_llm
            .as_ref()
            .map(|process| process.pid)
            .unwrap_or_default();
        sample.llm_top = process_snapshot
            .top_llm
            .as_ref()
            .map(|process| process.name.clone())
            .unwrap_or_else(|| "none".into());
        sample.largest_consumer = process_snapshot.largest_consumer;
        let mut llm_processes = process_snapshot.llm_processes;
        if let Some(elapsed) = process_elapsed {
            annotate_process_pagein_rates(&mut llm_processes, &self.current.llm_processes, elapsed);
        }
        sample.llm_processes = llm_processes;
        let live_stats = self.llm_client.poll();
        let should_read_log = live_stats.is_none()
            && detected_provider
                .as_deref()
                .map(|provider| provider == "oMLX")
                .unwrap_or(true);
        let log_stats = if should_read_log {
            read_llm_stats()
        } else {
            LlmLogStats::default()
        };
        let llm_stats = live_stats.as_ref();
        sample.mlx = llm_stats.map(|stats| stats.mlx.clone()).unwrap_or_default();
        sample.metal.resource_limit = sample.metal.resource_limit.or(sample.mlx.resource_limit);
        let live_is_stale = llm_stats
            .and_then(|stats| stats.observed_at)
            .and_then(|observed_at| SystemTime::now().duration_since(observed_at).ok())
            .is_some_and(|age| age > Duration::from_secs(5));
        let use_omlx_log = log_stats.model.is_some()
            && detected_provider
                .as_deref()
                .map(|provider| provider == "oMLX")
                .unwrap_or(true);
        sample.llm_provider = llm_stats
            .and_then(|stats| stats.provider.clone())
            .unwrap_or_else(|| {
                if let Some(provider) = detected_provider.clone() {
                    provider
                } else if use_omlx_log {
                    "oMLX".into()
                } else {
                    "none".into()
                }
            });
        sample.llm_status = if live_is_stale {
            "stale".into()
        } else {
            llm_stats
                .and_then(|stats| stats.status.clone())
                .unwrap_or_else(|| {
                    if use_omlx_log {
                        "last result".into()
                    } else if sample.llm_count > 0 {
                        "running".into()
                    } else {
                        "offline".into()
                    }
                })
        };
        sample.llm_model = llm_stats
            .and_then(|stats| stats.model.clone())
            .or_else(|| use_omlx_log.then(|| log_stats.model.clone()).flatten())
            .unwrap_or_else(|| {
                if sample.llm_count > 0 {
                    sample.llm_top.clone()
                } else {
                    "not detected".into()
                }
            });
        sample.llm_source = live_stats
            .as_ref()
            .map(|stats| stats.source)
            .unwrap_or_else(|| {
                if use_omlx_log {
                    TelemetrySource::Log
                } else {
                    TelemetrySource::None
                }
            });
        sample.llm_observed_at = live_stats
            .as_ref()
            .and_then(|stats| stats.observed_at)
            .or_else(|| use_omlx_log.then_some(log_stats.observed_at).flatten());
        sample.llm_generation_tps =
            llm_stats
                .and_then(|stats| stats.generation_tps)
                .or_else(|| {
                    if live_stats.is_none() && use_omlx_log {
                        log_stats.tokens_per_second
                    } else {
                        None
                    }
                });
        sample.llm_generation_tps_live = !live_is_stale
            && llm_stats
                .map(|stats| stats.generation_tps_live)
                .unwrap_or(false);
        sample.llm_prefill_tps = llm_stats.and_then(|stats| stats.prefill_tps);
        sample.llm_prefill_tps_live = !live_is_stale
            && llm_stats
                .map(|stats| stats.prefill_tps_live)
                .unwrap_or(false);
        sample.llm_output_tokens = llm_stats.and_then(|stats| stats.output_tokens).or_else(|| {
            if live_stats.is_none() && use_omlx_log {
                log_stats.output_tokens
            } else {
                None
            }
        });
        sample.llm_prompt_tokens = llm_stats.and_then(|stats| stats.prompt_tokens).or_else(|| {
            if live_stats.is_none() && use_omlx_log {
                log_stats.prompt_tokens
            } else {
                None
            }
        });
        sample.llm_cache_efficiency = llm_stats.and_then(|stats| stats.cache_efficiency);
        sample.llm_cache_interval_efficiency =
            cache_interval_efficiency(&mut self.previous_llm_cache, llm_stats, live_is_stale);
        sample.llm_prefix_hit_rate = llm_stats.and_then(|stats| stats.prefix_hit_rate);
        sample.llm_active_requests = llm_stats.and_then(|stats| stats.active_requests);
        sample.llm_waiting_requests = llm_stats.and_then(|stats| stats.waiting_requests);
        sample.llm_model_memory = llm_stats.and_then(|stats| stats.model_memory);
        sample.llm_model_memory_max = llm_stats.and_then(|stats| stats.model_memory_max);
        sample.updated = now_clock();
        let previous = if self.current.updated == "waiting" {
            None
        } else {
            Some(self.current.clone())
        };
        sample.correlation = self.correlation.observe(&sample);
        classify(&mut sample, previous.as_ref());

        push_history(
            &mut self.generation_history,
            chart_rate_value(&sample, ChartMetric::Generation),
            ChartMetric::Generation,
            self.history_limit,
        );
        push_history(
            &mut self.prefill_history,
            chart_rate_value(&sample, ChartMetric::Prefill),
            ChartMetric::Prefill,
            self.history_limit,
        );
        push_history(
            &mut self.cache_history,
            sample
                .llm_cache_interval_efficiency
                .filter(|value| value.is_finite() && *value >= 0.0)
                .map(|value| value.round() as u64),
            ChartMetric::Cache,
            self.history_limit,
        );
        if let Some(point) = self.generation_history.back_mut() {
            point.tone = if point.value.is_none() {
                Tone::Muted
            } else if sample.correlation.is_material_drop() {
                sample.correlation.tone()
            } else {
                Tone::Cyan
            };
        }
        push_history(
            &mut self.load_history,
            sample.availability.map(|v| 100_u8.saturating_sub(v) as u64),
            ChartMetric::Memory,
            self.history_limit,
        );
        push_history(
            &mut self.swap_history,
            (sample.swap_available && sample.rate_ready)
                .then_some(sample.swap_in.saturating_add(sample.swap_out)),
            ChartMetric::Swap,
            self.history_limit,
        );
        push_history(
            &mut self.gpu_history,
            sample.gpu_util.map(u64::from),
            ChartMetric::Gpu,
            self.history_limit,
        );

        self.record_journal_events(previous.as_ref(), &sample);
        diagnostics_log(
            "INFO",
            "sample",
            format!(
                "duration_ms={} status={} provider={} model={} source={} gen_live={} gen_tps={} prefill_live={} prefill_tps={} cache={} active={} waiting={} gpu={} renderer={} tiler={} memory_load={} free={} paging_in={} paging_out={} compress={} decompress={}",
                sample_started.elapsed().as_millis(),
                log_field(&sample.llm_status),
                log_field(&sample.llm_provider),
                log_field(&sample.llm_model),
                log_field(sample.llm_source.label()),
                sample.llm_generation_tps_live,
                log_optional_f64(sample.llm_generation_tps),
                sample.llm_prefill_tps_live,
                log_optional_f64(sample.llm_prefill_tps),
                log_optional_f64(sample.llm_cache_efficiency),
                log_optional_u64(sample.llm_active_requests),
                log_optional_u64(sample.llm_waiting_requests),
                log_optional_u8(sample.gpu_util),
                log_optional_u8(sample.metal.renderer_util),
                log_optional_u8(sample.metal.tiler_util),
                log_optional_u8(sample.availability.map(|free| 100_u8.saturating_sub(free))),
                log_optional_u8(sample.availability),
                sample.swap_in,
                sample.swap_out,
                sample.compress,
                sample.decompress,
            ),
        );
        self.current = sample.clone();
        sample
    }

    fn view(&self) -> CollectorView {
        CollectorView {
            current: self.current.clone(),
            generation_history: self.generation_history.clone(),
            prefill_history: self.prefill_history.clone(),
            cache_history: self.cache_history.clone(),
            load_history: self.load_history.clone(),
            swap_history: self.swap_history.clone(),
            gpu_history: self.gpu_history.clone(),
            signals: self.signals.clone(),
        }
    }

    fn record_journal_events(&mut self, previous: Option<&Sample>, sample: &Sample) {
        let mut events = Vec::new();
        let mut add = |state: &str, summary: String, tone: Tone| {
            events.push((state.to_string(), summary, tone));
        };

        if let Some(previous) = previous {
            if previous.impact != sample.impact && sample.impact != "SAMPLING" {
                add(&sample.impact, signal_summary(sample), sample.impact_tone);
            }

            if (previous.llm_provider != sample.llm_provider
                || previous.llm_model != sample.llm_model)
                && sample.llm_provider != "none"
                && sample.llm_model != "not detected"
            {
                add(
                    "LLM",
                    format!(
                        "detected {} · {}",
                        sample.llm_provider,
                        llm_model_label(sample, 42)
                    ),
                    Tone::Cyan,
                );
            }

            if previous.llm_status != sample.llm_status {
                add(
                    "LLM",
                    format!(
                        "status {} → {} · {}",
                        previous.llm_status, sample.llm_status, sample.llm_provider
                    ),
                    Tone::Cyan,
                );
            }

            if previous.llm_source != sample.llm_source {
                add(
                    "LLM",
                    format!(
                        "telemetry source · {} → {}",
                        previous.llm_source.label(),
                        sample.llm_source.label()
                    ),
                    if sample.llm_source == TelemetrySource::Live {
                        Tone::Green
                    } else {
                        Tone::Yellow
                    },
                );
            }

            let previous_active = previous.llm_active_requests.unwrap_or(0);
            let active = sample.llm_active_requests.unwrap_or(0);
            if (previous_active == 0) != (active == 0) {
                add(
                    "LLM",
                    if active == 0 {
                        "request completed · serving is idle".into()
                    } else {
                        format!("request started · {active} active")
                    },
                    Tone::Cyan,
                );
            }

            let previous_waiting = previous.llm_waiting_requests.unwrap_or(0);
            let waiting = sample.llm_waiting_requests.unwrap_or(0);
            if (previous_waiting == 0) != (waiting == 0) {
                add(
                    "QUEUE",
                    if waiting == 0 {
                        "queue cleared".into()
                    } else {
                        format!("{waiting} request(s) waiting")
                    },
                    if waiting == 0 {
                        Tone::Green
                    } else {
                        Tone::Yellow
                    },
                );
            }

            if previous.llm_generation_tps.is_none() && sample.llm_generation_tps.is_some() {
                add(
                    "LLM",
                    format!(
                        "{} telemetry online · {} · {}",
                        sample.llm_source.label(),
                        llm_generation_rate_label(sample),
                        llm_prefill_rate_label(sample)
                    ),
                    Tone::Green,
                );
            }

            if sample.correlation.is_material_drop()
                && previous.correlation.event_key != sample.correlation.event_key
            {
                add(
                    "LLM",
                    format!(
                        "throughput diagnosis · {} · {} confidence",
                        sample.correlation.summary,
                        sample.correlation.confidence_label()
                    ),
                    sample.correlation.tone(),
                );
            }

            if previous.pressure != sample.pressure && !sample.pressure.is_empty() {
                add(
                    "PRESSURE",
                    format!("memory state {}", pressure_state_label(sample)),
                    sample.pressure_tone,
                );
            }

            let previous_paging = previous.swap_in.saturating_add(previous.swap_out) > 0;
            let paging = sample.swap_in.saturating_add(sample.swap_out) > 0;
            if previous_paging != paging {
                add(
                    "PAGING",
                    if paging {
                        format!(
                            "active · in {} · out {}",
                            rate(sample.swap_in),
                            rate(sample.swap_out)
                        )
                    } else {
                        "cleared · no current paging traffic".into()
                    },
                    if paging { Tone::Yellow } else { Tone::Green },
                );
            }

            // GPU utilization is sampled every second. Logging every 75↔74%
            // threshold crossing creates noise without explaining LLM impact.
            // Journal only meaningful busy/critical transitions; the chart
            // still retains the full per-sample color history.
            let previous_gpu = previous.gpu_util.unwrap_or(0);
            let gpu = sample.gpu_util.unwrap_or(0);
            let previous_gpu_busy = previous_gpu >= 80;
            let gpu_busy = gpu >= 80;
            let previous_gpu_critical = previous_gpu >= 90;
            let gpu_critical = gpu >= 90;
            if previous_gpu_critical != gpu_critical {
                add(
                    "GPU",
                    format!(
                        "{} GPU load · {}% busy",
                        if gpu_critical {
                            "critical"
                        } else {
                            "critical cleared"
                        },
                        gpu
                    ),
                    if gpu_critical {
                        Tone::Red
                    } else {
                        Tone::Yellow
                    },
                );
            } else if previous_gpu_busy != gpu_busy {
                add(
                    "GPU",
                    format!(
                        "{} · {}% busy",
                        if gpu_busy {
                            "busy burst started"
                        } else {
                            "busy burst cleared"
                        },
                        gpu
                    ),
                    if gpu_busy { Tone::Yellow } else { Tone::Green },
                );
            }

            if previous.thermal != sample.thermal && sample.thermal != "unavailable" {
                add(
                    "THERMAL",
                    sample.thermal.clone(),
                    if sample.thermal == "no warning" {
                        Tone::Green
                    } else {
                        Tone::Yellow
                    },
                );
            }
        } else {
            add(
                "SYSTEM",
                format!(
                    "journal started · {} free · pressure {}",
                    sample
                        .availability
                        .map(|value| format!("{value}%"))
                        .unwrap_or_else(|| "—".into()),
                    pressure_state_label(sample)
                ),
                Tone::Cyan,
            );
            if sample.llm_provider != "none" && sample.llm_model != "not detected" {
                add(
                    "LLM",
                    format!(
                        "detected {} · {}",
                        sample.llm_provider,
                        llm_model_label(sample, 42)
                    ),
                    Tone::Cyan,
                );
            }
        }

        for (state, summary, tone) in events {
            self.signals.push_back(SignalEvent {
                time: sample.updated.clone(),
                recorded_at: SystemTime::now(),
                kind: EventKind::from_state(&state),
                state,
                summary,
                tone,
            });
        }
        let journal_limit = self.history_limit.clamp(40, 240);
        while self.signals.len() > journal_limit {
            self.signals.pop_front();
        }
    }

    fn reset(&mut self) {
        self.previous = None;
        self.previous_llm_cache = None;
        self.correlation.reset();
        self.generation_history.clear();
        self.prefill_history.clear();
        self.cache_history.clear();
        self.load_history.clear();
        self.swap_history.clear();
        self.gpu_history.clear();
        self.signals.clear();
        self.current = Sample::default();
    }
}

/// A raised aggressive-paging alert. It survives until the user acknowledges
/// it or the paging episode ends, whichever comes first.
struct ActiveAlert {
    state: String,
    summary: String,
    time: String,
}

/// Impact states the classifier reserves for disk-bound swap churn. Crossing
/// into any of them from a quieter state is what triggers the paging alert.
fn is_aggressive_paging(state: &str) -> bool {
    matches!(
        state,
        "SWAP THRASHING" | "HEAVY PAGING" | "PAGE-IN RECOVERY"
    )
}

/// BEL passes through the alternate screen to the terminal emulator, so the
/// user's audible/visual bell setting decides how the alert sounds. Called
/// between frames only: writing mid-draw could interleave with the buffer.
fn ring_terminal_bell() {
    let mut out = stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}

struct App {
    collector: CollectorView,
    sampler: Sampler,
    interval: Duration,
    paused: bool,
    tab: usize,
    top_sort: TopSort,
    top_filter: String,
    top_filtering: bool,
    top_selected: usize,
    journal_filter: JournalFilter,
    journal_scroll: usize,
    help: bool,
    quit: bool,
    sampler_disconnected: bool,
    alert: Option<ActiveAlert>,
    alert_bells: usize,
}

impl App {
    fn new(interval: u64, history: usize) -> Self {
        let interval = Duration::from_secs(interval);
        Self {
            collector: CollectorView {
                current: Sample::default(),
                generation_history: VecDeque::new(),
                prefill_history: VecDeque::new(),
                cache_history: VecDeque::new(),
                load_history: VecDeque::new(),
                swap_history: VecDeque::new(),
                gpu_history: VecDeque::new(),
                signals: VecDeque::new(),
            },
            sampler: Sampler::spawn(interval, history),
            interval,
            paused: false,
            tab: 0,
            top_sort: TopSort::Rss,
            top_filter: String::new(),
            top_filtering: false,
            top_selected: 0,
            journal_filter: JournalFilter::All,
            journal_scroll: 0,
            help: false,
            quit: false,
            sampler_disconnected: false,
            alert: None,
            alert_bells: 0,
        }
    }

    fn tick(&mut self) {
        loop {
            match self.sampler.views.try_recv() {
                Ok(view) => {
                    self.track_paging_alert(&view);
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

    /// Raise the paging alert when the classifier crosses into an
    /// aggressive-paging state, and retire it once the episode ends. The
    /// rising edge guards against re-ringing while paging stays aggressive;
    /// dropping back below the aggressive states re-arms the alert. An
    /// escalation inside the episode (say HEAVY PAGING → SWAP THRASHING)
    /// refreshes the banner without ringing the bell again.
    fn track_paging_alert(&mut self, view: &CollectorView) {
        let aggressive = is_aggressive_paging(&view.current.impact);
        if let Some(alert) = &mut self.alert {
            if !aggressive {
                diagnostics_log(
                    "INFO",
                    "paging_alert_cleared",
                    format!("state={}", log_field(&alert.state)),
                );
                self.alert = None;
            } else if alert.state != view.current.impact {
                alert.state = view.current.impact.clone();
                alert.summary = signal_summary(&view.current);
                alert.time = view.current.updated.clone();
            }
            return;
        }
        let previous_impact = self.collector.current.impact.clone();
        if aggressive && !is_aggressive_paging(&previous_impact) {
            let summary = signal_summary(&view.current);
            diagnostics_log(
                "WARN",
                "paging_alert_raised",
                format!(
                    "state={} summary={}",
                    log_field(&view.current.impact),
                    log_field(&summary)
                ),
            );
            ring_terminal_bell();
            self.alert_bells += 1;
            self.alert = Some(ActiveAlert {
                state: view.current.impact.clone(),
                summary,
                time: view.current.updated.clone(),
            });
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
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
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
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
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('a') => {
                if let Some(alert) = self.alert.take() {
                    diagnostics_log(
                        "INFO",
                        "paging_alert_acknowledged",
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
                self.journal_filter = JournalFilter::All;
                self.alert = None;
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
            KeyCode::Tab | KeyCode::Right => self.tab = (self.tab + 1) % 3,
            KeyCode::BackTab | KeyCode::Left => self.tab = (self.tab + 2) % 3,
            KeyCode::Char('?') | KeyCode::Char('h') => self.help = true,
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let seconds = self.interval.as_secs().saturating_add(1).min(60);
                self.interval = Duration::from_secs(seconds);
                self.sampler
                    .send(SamplerCommand::SetInterval(self.interval));
            }
            KeyCode::Char('-') => {
                let seconds = self.interval.as_secs().saturating_sub(1).max(1);
                self.interval = Duration::from_secs(seconds);
                self.sampler
                    .send(SamplerCommand::SetInterval(self.interval));
            }
            _ => {}
        }
    }

    fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < 72 || area.height < 24 {
            self.draw_compact_warning(frame, area);
            return;
        }
        frame.render_widget(
            Block::default().style(Style::default().bg(Color::Rgb(10, 14, 21))),
            area,
        );
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(5)])
            .split(area);
        self.draw_header(frame, outer[0]);
        match self.tab {
            0 => self.draw_overview(frame, outer[1]),
            1 => self.draw_llm_top(frame, outer[1]),
            _ => self.draw_journal(frame, outer[1]),
        }
        if self.alert.is_some() {
            self.draw_alert_banner(frame, outer[1]);
        }
        if self.help {
            self.draw_help(frame, area);
        }
    }

    /// Overlay strip at the top of the tab content: aggressive paging demands
    /// attention without stealing a permanent layout row from the panels.
    fn draw_alert_banner(&self, frame: &mut Frame, area: Rect) {
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

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let compact_tabs = area.width < 140;
        let tab_labels = if compact_tabs {
            vec![
                Line::from("1 OVR"),
                Line::from("2 TOP"),
                Line::from("3 JRN"),
            ]
        } else {
            vec![
                Line::from("◉ Overview"),
                Line::from("▥ MLX Top"),
                Line::from("▤ Journal"),
            ]
        };
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(if compact_tabs { 20 } else { 30 }),
                Constraint::Length(if compact_tabs { 30 } else { 40 }),
                Constraint::Min(if compact_tabs { 30 } else { 50 }),
            ])
            .split(area);
        let title = Paragraph::new(Line::from(vec![
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
        ]))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(DIM)),
        );
        frame.render_widget(title, chunks[0]);

        let tabs = Tabs::new(tab_labels)
            .select(self.tab)
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(CYAN)
                    .add_modifier(Modifier::BOLD),
            )
            .style(Style::default().fg(MUTED))
            .divider(Span::styled(" · ", Style::default().fg(DIM)))
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(DIM)),
            );
        frame.render_widget(tabs, chunks[1]);
        self.draw_controls(frame, chunks[2]);
    }

    fn draw_overview(&self, frame: &mut Frame, area: Rect) {
        let operations_height = if area.width < 120 { 10 } else { 12 };
        let chart_minimum = MIN_CHART_HEIGHT * CHART_GRID_ROWS;
        let show_log = area.height >= operations_height + chart_minimum + 4;
        let mut constraints = vec![
            Constraint::Length(operations_height),
            Constraint::Min(chart_minimum),
        ];
        if show_log {
            constraints.push(Constraint::Length(4));
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);
        self.draw_operations_panel(frame, rows[0]);
        self.draw_trend_strip(frame, rows[1]);
        if show_log {
            self.draw_signal_log(frame, rows[2]);
        }
        if self.collector.current.total_memory == 0 && self.collector.current.updated != "waiting" {
            frame.render_widget(
                Paragraph::new(" macOS counters unavailable — run this binary on Apple silicon.")
                    .style(Style::default().fg(YELLOW)),
                area,
            );
        }
    }

    fn draw_operations_panel(&self, frame: &mut Frame, area: Rect) {
        if area.width < 120 {
            self.draw_compact_operations_panel(frame, area);
            return;
        }

        let s = &self.collector.current;
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(7), Constraint::Min(5)])
            .split(area);
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(27),
                Constraint::Percentage(34),
                Constraint::Percentage(39),
            ])
            .split(sections[0]);
        let metrics = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(25),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
                Constraint::Percentage(25),
            ])
            .split(sections[1]);

        let llm_tone = llm_status_tone(&s.llm_status);
        let process_summary = if s.llm_count > 0 {
            format!(
                "{} process(es) · RSS {} · CPU {:.1}%",
                s.llm_count,
                bytes(s.llm_rss),
                s.llm_cpu
            )
        } else {
            "provider live · process match unavailable".into()
        };
        let health_label = s
            .health
            .map(|score| format!("{} · {score}/100", s.grade))
            .unwrap_or_else(|| s.grade.clone());
        let compressed_memory = compressed_memory_label(s);
        let availability = s.availability;
        let memory_load = availability.map(|value| 100_u8.saturating_sub(value));
        let load_tone = memory_load
            .map(|value| ChartMetric::Memory.tone(value as u64))
            .unwrap_or(Tone::Muted);
        let memory_tone = match s.pressure_tone {
            Tone::Yellow | Tone::Red => s.pressure_tone,
            _ => load_tone,
        };
        let paging_rates_available = s.swap_available && s.vm_available && s.rate_ready;
        let paging_rate = if paging_rates_available {
            s.swap_in.saturating_add(s.swap_out)
        } else {
            0
        };
        let paging_tone = if paging_rates_available {
            ChartMetric::Swap.tone(paging_rate)
        } else {
            Tone::Muted
        };
        let paging_percent = if s.swap_available && s.swap_total > 0 {
            s.swap_used
                .saturating_mul(100)
                .checked_div(s.swap_total)
                .unwrap_or(0)
                .min(100) as u16
        } else {
            0
        };
        let paging_usage = if s.swap_available {
            format!("USED {} / {}", bytes(s.swap_used), bytes(s.swap_total))
        } else {
            "USED —".into()
        };
        let gpu_tone = s
            .gpu_util
            .map(|value| ChartMetric::Gpu.tone(value as u64))
            .unwrap_or(Tone::Muted);
        let gpu_memory = match (s.gpu_in_use, s.gpu_alloc) {
            (Some(used), Some(allocated)) => {
                format!("{} / {}", bytes(used), bytes(allocated))
            }
            (Some(used), None) => format!("{} used", bytes(used)),
            _ => "not exposed".into(),
        };

        render_card(
            frame,
            top[0],
            Line::from(vec![
                Span::styled(
                    format!(" {} ", llm_tone.icon()),
                    Style::default()
                        .fg(llm_tone.color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "MODEL / STATE",
                    Style::default()
                        .fg(llm_tone.color())
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            vec![
                Line::from(vec![
                    tone_badge(
                        llm_tone,
                        &format!("{} {}", llm_tone.icon(), s.llm_status.to_ascii_uppercase()),
                    ),
                    Span::styled(
                        format!("  {health_label}"),
                        Style::default()
                            .fg(llm_tone.color())
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    compact_label(
                        &format!(
                            "{} · {}",
                            compact_label(&s.llm_provider, 10),
                            llm_model_label(s, 38),
                        ),
                        top[0].width.saturating_sub(4) as usize,
                    ),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    compact_label(&process_summary, top[0].width.saturating_sub(4) as usize),
                    Style::default().fg(MUTED),
                )),
                Line::from(Span::styled(
                    format!(
                        "{} · {}",
                        telemetry_source(s),
                        telemetry_age(s.llm_observed_at)
                    ),
                    Style::default().fg(CYAN),
                )),
            ],
            llm_tone,
        );

        render_card(
            frame,
            top[1],
            Line::from(vec![
                Span::styled(
                    " ↯ ",
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "THROUGHPUT",
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
            ]),
            vec![
                Line::from(vec![
                    Span::styled(
                        llm_generation_rate_label(s),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" · ", Style::default().fg(MUTED)),
                    Span::styled(
                        llm_prefill_rate_label(s),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    format!(
                        "CONTEXT {} · OUT {} · PROMPT {}",
                        llm_context_label(s),
                        optional_tokens(s.llm_output_tokens),
                        optional_tokens(s.llm_prompt_tokens)
                    ),
                    Style::default().fg(MUTED),
                )),
                Line::from(Span::styled(
                    format!(
                        "CACHE {} · HIT {} · REQ {}/{}",
                        percent(s.llm_cache_efficiency),
                        percent(s.llm_prefix_hit_rate),
                        count(s.llm_active_requests),
                        count(s.llm_waiting_requests)
                    ),
                    Style::default().fg(CYAN),
                )),
                Line::from(Span::styled(
                    mlx_runtime_summary(&s.mlx),
                    Style::default().fg(MUTED),
                )),
            ],
            Tone::Cyan,
        );

        let action_width = top[2].width.saturating_sub(4) as usize;
        render_card(
            frame,
            top[2],
            Line::from(vec![
                Span::styled(
                    format!(" {} ", s.impact_tone.icon()),
                    Style::default()
                        .fg(s.impact_tone.color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "DIAGNOSIS",
                    Style::default()
                        .fg(s.impact_tone.color())
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            vec![
                Line::from(vec![
                    tone_badge(s.impact_tone, &s.guidance_badge),
                    Span::styled(
                        format!(
                            "  {}",
                            compact_label(&s.impact, action_width.saturating_sub(8))
                        ),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    format!(
                        "CAUSE  {}",
                        correlation_display(s, action_width.saturating_sub(7)).unwrap_or_else(
                            || { compact_label(&hero_impact(s), action_width.saturating_sub(7)) }
                        )
                    ),
                    Style::default().fg(CYAN),
                )),
                Line::from(Span::styled(
                    compact_label(&diagnostic_signal_line(s), action_width),
                    Style::default().fg(MUTED),
                )),
                Line::from(Span::styled(
                    format!(
                        "ACTION  {}",
                        compact_label(&s.guidance_action, action_width.saturating_sub(8))
                    ),
                    Style::default().fg(s.impact_tone.color()),
                )),
            ],
            s.impact_tone,
        );

        render_metric_card(
            frame,
            metrics[0],
            Line::from(vec![
                Span::styled(
                    " ◉ ",
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "MEMORY",
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(
                    memory_load
                        .map(|value| format!("{value}% load"))
                        .unwrap_or_else(|| "— load".into()),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    availability
                        .map(|value| format!("  ·  {value}% free"))
                        .unwrap_or_else(|| "  ·  — free".into()),
                    Style::default().fg(memory_tone.color()),
                ),
            ]),
            memory_load.map(|value| (value as u16, format!("{value}%"), memory_tone)),
            vec![Line::from(Span::styled(
                compact_label(
                    &format!(
                        "MODEL {} · COMP {} · PRESSURE {}",
                        compact_model_memory(s),
                        compressed_memory,
                        pressure_state_label(s)
                    ),
                    metrics[0].width.saturating_sub(4) as usize,
                ),
                Style::default().fg(memory_tone.color()),
            ))],
            memory_tone,
        );

        let paging_label = if paging_rates_available && paging_rate > 0 {
            "ACTIVE"
        } else if paging_rates_available {
            "IDLE"
        } else if s.swap_available && s.vm_available {
            "SAMPLING"
        } else {
            "UNAVAILABLE"
        };
        render_metric_card(
            frame,
            metrics[1],
            Line::from(vec![
                Span::styled(
                    " ⇄ ",
                    Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "PAGING / I/O",
                    Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(
                    if paging_rates_available {
                        rate(paging_rate)
                    } else {
                        "—".into()
                    },
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  ·  {paging_usage} · {paging_label}"),
                    Style::default().fg(paging_tone.color()),
                ),
            ]),
            s.swap_available
                .then_some((paging_percent, format!("{paging_percent}%"), paging_tone)),
            vec![Line::from(Span::styled(
                if paging_rates_available {
                    compact_label(
                        &format!(
                            "IN {} · OUT {} · COMP {}",
                            rate(s.swap_in),
                            rate(s.swap_out),
                            rate(s.compress.saturating_add(s.decompress))
                        ),
                        metrics[1].width.saturating_sub(4) as usize,
                    )
                } else {
                    "rate baseline sampling".into()
                },
                Style::default().fg(Color::White),
            ))],
            Tone::Yellow,
        );

        render_metric_card(
            frame,
            metrics[2],
            Line::from(vec![
                Span::styled(
                    " ◇ ",
                    Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "GPU / COMPUTE",
                    Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(
                    s.gpu_util
                        .map(|value| format!("{value}% busy"))
                        .unwrap_or_else(|| "— busy".into()),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  ·  {}", gpu_load_label(s.gpu_util)),
                    Style::default().fg(gpu_tone.color()),
                ),
            ]),
            s.gpu_util
                .map(|value| (value as u16, format!("{value}%"), gpu_tone)),
            vec![Line::from(Span::styled(
                compact_label(
                    &metal_signal_line(s, &gpu_memory),
                    metrics[2].width.saturating_sub(4) as usize,
                ),
                Style::default().fg(MUTED),
            ))],
            Tone::Blue,
        );

        let cache_tone = if s.llm_cache_efficiency.is_some() {
            Tone::Cyan
        } else {
            Tone::Muted
        };
        render_metric_card(
            frame,
            metrics[3],
            Line::from(vec![
                Span::styled(
                    " ◈ ",
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "CACHE / QUEUE",
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("CACHE {}", percent(s.llm_cache_efficiency)),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  ·  HIT {}", percent(s.llm_prefix_hit_rate)),
                    Style::default().fg(CYAN),
                ),
            ]),
            s.llm_cache_efficiency.map(|value| {
                let value = value.clamp(0.0, 100.0).round() as u16;
                (value, format!("{value}%"), cache_tone)
            }),
            vec![Line::from(Span::styled(
                compact_label(
                    &format!(
                        "REQ {}/{} · CONTEXT {}",
                        count(s.llm_active_requests),
                        count(s.llm_waiting_requests),
                        llm_context_label(s)
                    ),
                    metrics[3].width.saturating_sub(4) as usize,
                ),
                Style::default().fg(MUTED),
            ))],
            cache_tone,
        );
    }

    fn draw_compact_operations_panel(&self, frame: &mut Frame, area: Rect) {
        let sample = &self.collector.current;
        let width = area.width.saturating_sub(4) as usize;
        let llm_tone = llm_status_tone(&sample.llm_status);
        let paging_rate = sample.swap_in.saturating_add(sample.swap_out);
        let generation = llm_generation_rate_label(sample);
        let prefill = llm_prefill_rate_label(sample);
        let diagnosis = correlation_display(sample, width.saturating_sub(11))
            .unwrap_or_else(|| compact_label(&hero_impact(sample), width.saturating_sub(11)));
        let signals = format!(
            "MEM {} load · GPU {} · PAGE {} · PRESSURE {}",
            sample
                .availability
                .map(|value| format!("{}%", 100_u8.saturating_sub(value)))
                .unwrap_or_else(|| "—".into()),
            percent_u8(sample.gpu_util),
            if sample.rate_ready {
                rate(paging_rate)
            } else {
                "sampling".into()
            },
            pressure_state_label(sample),
        );
        let runtime = format!(
            "COMP {} · THERMAL {} · {} · {}",
            compressed_memory_label(sample),
            sample.thermal,
            telemetry_source(sample),
            telemetry_age(sample.llm_observed_at),
        );
        let lines = vec![
            Line::from(vec![
                Span::styled(" STATUS  ", Style::default().fg(MUTED)),
                tone_badge(llm_tone, &sample.llm_status.to_ascii_uppercase()),
                Span::styled(
                    format!("  {} · {}", sample.grade, sample.llm_provider),
                    Style::default()
                        .fg(llm_tone.color())
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(" MODEL   ", Style::default().fg(MUTED)),
                Span::styled(
                    compact_label(&sample.llm_model, width.saturating_sub(9)),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(" RATE    ", Style::default().fg(MUTED)),
                Span::styled(
                    format!("{generation} · {prefill}"),
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(" WORK    ", Style::default().fg(MUTED)),
                Span::styled(
                    compact_label(
                        &format!(
                            "CONTEXT {} · CACHE {} · REQ {}/{}",
                            llm_context_label(sample),
                            percent(sample.llm_cache_efficiency),
                            count(sample.llm_active_requests),
                            count(sample.llm_waiting_requests),
                        ),
                        width.saturating_sub(9),
                    ),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled(" DIAG    ", Style::default().fg(MUTED)),
                Span::styled(
                    compact_label(&diagnosis, width.saturating_sub(9)),
                    Style::default()
                        .fg(sample.correlation.tone().color())
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(" ACTION  ", Style::default().fg(MUTED)),
                Span::styled(
                    compact_label(&sample.guidance_action, width.saturating_sub(9)),
                    Style::default().fg(sample.impact_tone.color()),
                ),
            ]),
            Line::from(vec![
                Span::styled(" SIGNAL  ", Style::default().fg(MUTED)),
                Span::styled(
                    compact_label(&signals, width.saturating_sub(9)),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled(" RUNTIME ", Style::default().fg(MUTED)),
                Span::styled(
                    compact_label(&runtime, width.saturating_sub(9)),
                    Style::default().fg(MUTED),
                ),
            ]),
        ];
        frame.render_widget(
            Paragraph::new(lines)
                .block(panel("LLM OPERATIONS · LIVE IMPACT", sample.impact_tone))
                .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn draw_trend_strip(&self, frame: &mut Frame, area: Rect) {
        self.render_indicator_charts(frame, area);
    }

    fn render_indicator_charts(&self, frame: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
            .split(area);
        for (row, series) in rows.iter().zip([
            [
                (
                    "generation",
                    &self.collector.generation_history,
                    ChartMetric::Generation,
                ),
                (
                    "prefill",
                    &self.collector.prefill_history,
                    ChartMetric::Prefill,
                ),
                ("cache", &self.collector.cache_history, ChartMetric::Cache),
            ],
            [
                ("GPU", &self.collector.gpu_history, ChartMetric::Gpu),
                ("memory", &self.collector.load_history, ChartMetric::Memory),
                ("paging", &self.collector.swap_history, ChartMetric::Swap),
            ],
        ]) {
            let columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Ratio(1, 3),
                    Constraint::Ratio(1, 3),
                    Constraint::Ratio(1, 3),
                ])
                .split(*row);
            for (column, (name, history, metric)) in columns.iter().zip(series) {
                self.render_indicator_chart(frame, *column, name, history, metric);
            }
        }
    }

    fn render_indicator_chart(
        &self,
        frame: &mut Frame,
        area: Rect,
        name: &'static str,
        history: &VecDeque<ChartPoint>,
        metric: ChartMetric,
    ) {
        let current = history.back().and_then(|point| point.value);
        let current_label = match metric {
            ChartMetric::Generation | ChartMetric::Prefill => current
                .map(|value| format!("{:.1} tok/s", value as f64 / 10.0))
                .unwrap_or_else(|| chart_inactive_rate_label(metric, &self.collector.current)),
            ChartMetric::Cache => current
                .map(|value| format!("{value}%"))
                .unwrap_or_else(|| "—".into()),
            ChartMetric::Memory | ChartMetric::Gpu => current
                .map(|value| format!("{value}%"))
                .unwrap_or_else(|| "—".into()),
            ChartMetric::Swap => current.map(rate).unwrap_or_else(|| "—".into()),
        };
        let chart_tone = metric.chart_tone();
        let current_tone = history
            .back()
            .map(|point| point.tone)
            .unwrap_or(Tone::Muted);
        let scale_max = chart_scale_max(history, metric);
        let axis_label = if matches!(metric, ChartMetric::Generation | ChartMetric::Prefill) {
            format!("0–{:.0}", scale_max as f64 / 10.0)
        } else if matches!(metric, ChartMetric::Swap) {
            "0–100 log".into()
        } else {
            "0–100".into()
        };
        let top_axis = if matches!(metric, ChartMetric::Generation | ChartMetric::Prefill) {
            format!("{:.0}", scale_max as f64 / 10.0)
        } else {
            "100".into()
        };
        let label_width = (top_axis.chars().count() + 1)
            .clamp(3, 6)
            .min(area.width.saturating_sub(2) as usize);
        let plot_width = (area.width.saturating_sub(2) as usize).saturating_sub(label_width);
        let (average, peak) = chart_stats_for_width(history, metric, plot_width);
        let window = chart_window_label(history.len().min(plot_width), self.interval);
        let stats = if area.width >= 68 {
            format!(
                "  · avg {} · peak {} · {} · {} · older → now",
                chart_stat_label(metric, average),
                chart_stat_label(metric, peak),
                window,
                axis_label
            )
        } else {
            format!(
                "  · avg {} · peak {} · {}",
                chart_stat_label(metric, average),
                chart_stat_label(metric, peak),
                window
            )
        };
        let title = Line::from(vec![
            Span::styled(
                format!(" {name} "),
                Style::default()
                    .fg(chart_tone.color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::default().fg(MUTED)),
            Span::styled(
                current_label,
                Style::default()
                    .fg(current_tone.color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(stats, Style::default().fg(MUTED)),
        ]);
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(DIM))
            .style(Style::default().bg(PANEL));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.is_empty() {
            return;
        }

        let plot_height = inner.height as usize;
        // Render the newest sample at the right edge and let older samples
        // leave from the left. Every displayed column maps to one captured
        // sample: smoothing uses only the causal prefix and the current chart
        // resolution; no re-bucketing, future-sample smoothing, recoloring,
        // or rescaling can rewrite history while the chart is scrolling.
        let visible_points = chart_columns_for_plot(history, plot_width, metric, plot_height);
        let mut cells = vec![
            vec![
                TraceCell {
                    glyph: ' ',
                    tone: Tone::Muted,
                };
                plot_width
            ];
            plot_height
        ];
        for (row, row_cells) in cells.iter_mut().enumerate() {
            let guide = if row + 1 == plot_height {
                Some('─')
            } else if row * 2 == plot_height {
                Some('┄')
            } else {
                None
            };
            if let Some(glyph) = guide {
                row_cells.fill(TraceCell {
                    glyph,
                    tone: Tone::Muted,
                });
            }
        }

        let mut point_rows = vec![None; plot_width];
        let mut connect_before = vec![false; plot_width];
        let mut previous_point = None;
        for (column, point) in visible_points.iter().enumerate() {
            let Some(value) = point.value else {
                previous_point = None;
                continue;
            };
            if point.break_before {
                previous_point = None;
            }
            let display_value = chart_display_value(metric, value, scale_max);
            let Some((row, glyph)) = trace_point(display_value, plot_height) else {
                previous_point = None;
                continue;
            };
            if previous_point.is_some() {
                connect_before[column] = true;
            }
            point_rows[column] = Some((row, point.tone));
            cells[row][column] = TraceCell {
                glyph,
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
                metric,
                previous_tone,
                tone,
            );
        }

        let mut lines = Vec::with_capacity(plot_height);
        for (row, row_cells) in cells.iter().enumerate() {
            let label = if row == 0 {
                format!("{top_axis} ")
            } else if row + 1 == plot_height {
                "0 ".into()
            } else if row * 2 == plot_height {
                if matches!(metric, ChartMetric::Generation | ChartMetric::Prefill) {
                    format!("{:.0} ", scale_max as f64 / 20.0)
                } else {
                    "50 ".into()
                }
            } else {
                String::new()
            };
            let mut spans = vec![Span::styled(
                format!("{label:>label_width$}"),
                Style::default().fg(DIM),
            )];
            let mut run = String::new();
            let mut run_tone = None;
            for cell in row_cells.iter().take(plot_width).copied() {
                let (cell, tone) = (cell.glyph, cell.tone);
                if run_tone != Some(tone) {
                    if let Some(tone) = run_tone {
                        spans.push(Span::styled(
                            std::mem::take(&mut run),
                            Style::default().fg(tone.color()),
                        ));
                    }
                    run_tone = Some(tone);
                }
                run.push(cell);
            }
            if let Some(tone) = run_tone {
                spans.push(Span::styled(run, Style::default().fg(tone.color())));
            }
            lines.push(Line::from(spans));
        }
        frame.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    fn draw_signal_log(&self, frame: &mut Frame, area: Rect) {
        let capacity = area.height.saturating_sub(2) as usize;
        let rows = self
            .collector
            .signals
            .iter()
            .rev()
            .take(capacity)
            .map(|event| {
                Line::from(vec![
                    Span::styled(format!(" {} ", event.time), Style::default().fg(DIM)),
                    Span::styled(
                        format!("{:^19}", event.state),
                        Style::default()
                            .fg(event.tone.color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {}", event.summary), Style::default().fg(MUTED)),
                ])
            })
            .collect::<Vec<_>>();
        let text = if rows.is_empty() {
            Text::from(Line::from(Span::styled(
                " waiting for the first classified sample",
                Style::default().fg(MUTED),
            )))
        } else {
            Text::from(rows)
        };
        frame.render_widget(
            Paragraph::new(text)
                .block(panel("RECENT JOURNAL", Tone::Muted))
                .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn draw_journal(&self, frame: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(5), Constraint::Min(10)])
            .split(area);
        self.draw_journal_header(frame, rows[0]);
        self.draw_journal_events(frame, rows[1]);
    }

    fn filtered_llm_processes(&self) -> Vec<LlmProcess> {
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

    fn draw_llm_top(&self, frame: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Length(6),
                Constraint::Min(10),
            ])
            .split(area);
        let sample = &self.collector.current;
        let filtered = self.filtered_llm_processes();
        let filter_label = if self.top_filter.is_empty() {
            "all detected processes".to_owned()
        } else {
            format!("filter /{}", self.top_filter)
        };
        let editing = if self.top_filtering {
            " · typing filter · Enter/Esc close"
        } else {
            ""
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        " MLX TOP  ",
                        Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            "{} processes · sort {} · {}",
                            filtered.len(),
                            self.top_sort.label(),
                            filter_label
                        ),
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("  ↑↓ select  ", Style::default().fg(MUTED)),
                    Span::styled(
                        "s",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" sort  ", Style::default().fg(MUTED)),
                    Span::styled(
                        "f / /",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" filter  ", Style::default().fg(MUTED)),
                    Span::styled(
                        "c",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" clear", Style::default().fg(MUTED)),
                    Span::styled(editing, Style::default().fg(YELLOW)),
                ]),
            ])
            .block(panel("PROCESS MONITOR · LIVE LLM WORKLOAD", Tone::Cyan))
            .wrap(Wrap { trim: true }),
            rows[0],
        );

        let status_tone = llm_status_tone(&sample.llm_status);
        if rows[1].width < 120 {
            self.draw_compact_top_summary(frame, rows[1]);
        } else {
            let columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(33),
                    Constraint::Percentage(34),
                    Constraint::Percentage(33),
                ])
                .split(rows[1]);
            let summary_width = columns[0].width.saturating_sub(4) as usize;
            render_card(
                frame,
                columns[0],
                Line::from(vec![
                    Span::styled(
                        " ◉ ",
                        Style::default()
                            .fg(status_tone.color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "MODEL / STATE",
                        Style::default()
                            .fg(status_tone.color())
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                vec![
                    Line::from(vec![
                        tone_badge(status_tone, &sample.llm_status.to_ascii_uppercase()),
                        Span::styled(
                            format!("  {}", compact_label(&sample.llm_provider, 12)),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(Span::styled(
                        compact_label(&sample.llm_model, summary_width),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(
                        format!(
                            "{} · {} · {}",
                            process_count_label(sample),
                            telemetry_source(sample),
                            telemetry_age(sample.llm_observed_at)
                        ),
                        Style::default().fg(MUTED),
                    )),
                    Line::from(Span::styled(
                        format!("RSS {} · CPU {:.1}%", bytes(sample.llm_rss), sample.llm_cpu),
                        Style::default().fg(MUTED),
                    )),
                ],
                status_tone,
            );
            render_card(
                frame,
                columns[1],
                Line::from(vec![
                    Span::styled(
                        " ↯ ",
                        Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "THROUGHPUT",
                        Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                    ),
                ]),
                vec![
                    Line::from(vec![
                        Span::styled(
                            llm_generation_rate_label(sample),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(" · ", Style::default().fg(MUTED)),
                        Span::styled(
                            llm_prefill_rate_label(sample),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(Span::styled(
                        format!(
                            "CONTEXT {} · OUT {} · PROMPT {}",
                            llm_context_label(sample),
                            optional_tokens(sample.llm_output_tokens),
                            optional_tokens(sample.llm_prompt_tokens)
                        ),
                        Style::default().fg(MUTED),
                    )),
                    Line::from(Span::styled(
                        format!(
                            "CACHE {} · HIT {} · REQ {}/{}",
                            percent(sample.llm_cache_efficiency),
                            percent(sample.llm_prefix_hit_rate),
                            count(sample.llm_active_requests),
                            count(sample.llm_waiting_requests)
                        ),
                        Style::default().fg(CYAN),
                    )),
                    Line::from(Span::styled(
                        mlx_runtime_summary(&sample.mlx),
                        Style::default().fg(MUTED),
                    )),
                ],
                Tone::Cyan,
            );
            render_card(
                frame,
                columns[2],
                Line::from(vec![
                    Span::styled(
                        " ◇ ",
                        Style::default()
                            .fg(sample.impact_tone.color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "DIAGNOSIS",
                        Style::default()
                            .fg(sample.impact_tone.color())
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                vec![
                    Line::from(vec![
                        tone_badge(sample.impact_tone, &sample.guidance_badge),
                        Span::styled(
                            format!("  {}", compact_label(&sample.impact, 18)),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(Span::styled(
                        format!(
                            "CAUSE  {}",
                            correlation_display(sample, summary_width.saturating_sub(7))
                                .unwrap_or_else(|| compact_label(
                                    &hero_impact(sample),
                                    summary_width.saturating_sub(7),
                                ))
                        ),
                        Style::default().fg(sample.correlation.tone().color()),
                    )),
                    Line::from(Span::styled(
                        compact_label(&diagnostic_signal_line(sample), summary_width),
                        Style::default().fg(MUTED),
                    )),
                    Line::from(Span::styled(
                        format!(
                            "ACTION  {}",
                            compact_label(&sample.guidance_action, summary_width.saturating_sub(8))
                        ),
                        Style::default().fg(sample.impact_tone.color()),
                    )),
                ],
                sample.impact_tone,
            );
        }

        let selected = if filtered.is_empty() {
            0
        } else {
            self.top_selected.min(filtered.len() - 1)
        };
        let visible = rows[2].height.saturating_sub(3).max(1) as usize;
        let start = selected
            .saturating_sub(visible.saturating_sub(1))
            .min(filtered.len().saturating_sub(visible));
        let table_mode = if rows[2].width >= 150 {
            2
        } else if rows[2].width >= 95 {
            1
        } else {
            0
        };
        let (headers, widths): (Vec<&str>, Vec<Constraint>) = match table_mode {
            2 => (
                vec![
                    "PID",
                    "PROCESS / COMMAND",
                    "CPU",
                    "MEM%",
                    "RSS",
                    "PAGEIN/s",
                    "OS",
                    "LLM STATE",
                    "MODEL",
                ],
                vec![
                    Constraint::Length(8),
                    Constraint::Min(26),
                    Constraint::Length(8),
                    Constraint::Length(8),
                    Constraint::Length(13),
                    Constraint::Length(11),
                    Constraint::Length(7),
                    Constraint::Length(12),
                    Constraint::Min(22),
                ],
            ),
            1 => (
                vec!["PID", "PROCESS", "CPU", "RSS", "PAGEIN/s", "STATE", "MODEL"],
                vec![
                    Constraint::Length(7),
                    Constraint::Min(20),
                    Constraint::Length(7),
                    Constraint::Length(12),
                    Constraint::Length(10),
                    Constraint::Length(11),
                    Constraint::Min(16),
                ],
            ),
            _ => (
                vec!["PID", "PROCESS", "CPU", "RSS", "STATE"],
                vec![
                    Constraint::Length(7),
                    Constraint::Min(22),
                    Constraint::Length(7),
                    Constraint::Length(12),
                    Constraint::Length(11),
                ],
            ),
        };
        let table_rows = filtered
            .iter()
            .skip(start)
            .take(visible)
            .map(|process| {
                let command = if process.command == process.name {
                    process.name.clone()
                } else {
                    format!("{} {}", process.name, process.command)
                };
                let status = sample.llm_status.to_ascii_uppercase();
                let memory_percent = process
                    .memory_percent
                    .map(|value| format!("{value:.1}%"))
                    .unwrap_or_else(|| "—".into());
                let memory_tone = process
                    .memory_percent
                    .map(|value| ChartMetric::Memory.tone(value.max(0.0).round() as u64))
                    .unwrap_or(Tone::Muted);
                let pagein_rate = process
                    .pagein_rate
                    .map(|value| format!("{value:.1}"))
                    .unwrap_or_else(|| "—".into());
                let os_state = if process.state == "?" {
                    "—".into()
                } else {
                    compact_label(&process.state, 7)
                };
                let state = Cell::from(tone_badge(status_tone, &compact_label(&status, 10)));
                let cells = match table_mode {
                    2 => vec![
                        Cell::from(process.pid.to_string()),
                        Cell::from(compact_label(&command, 34)),
                        Cell::from(format!("{:.1}%", process.cpu)),
                        Cell::from(Span::styled(
                            memory_percent,
                            Style::default().fg(memory_tone.color()),
                        )),
                        Cell::from(bytes(process.rss)),
                        Cell::from(pagein_rate),
                        Cell::from(os_state),
                        state,
                        Cell::from(compact_label(&sample.llm_model, 24)),
                    ],
                    1 => vec![
                        Cell::from(process.pid.to_string()),
                        Cell::from(compact_label(&command, 28)),
                        Cell::from(format!("{:.1}%", process.cpu)),
                        Cell::from(bytes(process.rss)),
                        Cell::from(pagein_rate),
                        state,
                        Cell::from(compact_label(&sample.llm_model, 20)),
                    ],
                    _ => vec![
                        Cell::from(process.pid.to_string()),
                        Cell::from(compact_label(&command, 28)),
                        Cell::from(format!("{:.1}%", process.cpu)),
                        Cell::from(bytes(process.rss)),
                        state,
                    ],
                };
                Row::new(cells)
            })
            .collect::<Vec<_>>();
        let table_rows = if table_rows.is_empty() {
            let mut cells = (0..headers.len())
                .map(|_| Cell::from("—"))
                .collect::<Vec<_>>();
            cells[1] = Cell::from(if filtered.is_empty() && !self.top_filter.is_empty() {
                "No process matches this filter"
            } else {
                "No local LLM process; provider telemetry may still be live"
            });
            vec![Row::new(cells)]
        } else {
            table_rows
        };
        let title = format!(
            "LLM PROCESSES · SORT {} · {}–{} of {}",
            self.top_sort.label(),
            if filtered.is_empty() { 0 } else { start + 1 },
            (start + visible).min(filtered.len()),
            filtered.len()
        );
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
            .block(panel(&title, Tone::Blue));
        let mut table_state = TableState::default();
        if !filtered.is_empty() {
            table_state.select(Some(selected.saturating_sub(start)));
        }
        frame.render_stateful_widget(table, rows[2], &mut table_state);
    }

    fn draw_compact_top_summary(&self, frame: &mut Frame, area: Rect) {
        let sample = &self.collector.current;
        let width = area.width.saturating_sub(4) as usize;
        let status_tone = llm_status_tone(&sample.llm_status);
        let diagnosis = correlation_display(sample, width.saturating_sub(8))
            .unwrap_or_else(|| compact_label(&hero_impact(sample), width.saturating_sub(8)));
        render_card(
            frame,
            area,
            Line::from(vec![
                Span::styled(" ◉ ", Style::default().fg(status_tone.color())),
                Span::styled(
                    "WORKLOAD SUMMARY · THROUGHPUT / IMPACT",
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
            ]),
            vec![
                Line::from(vec![
                    tone_badge(status_tone, &sample.llm_status.to_ascii_uppercase()),
                    Span::styled(
                        format!(
                            "  {} · {}",
                            compact_label(&sample.llm_provider, 10),
                            compact_label(&sample.llm_model, width.saturating_sub(20)),
                        ),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("RATE  ", Style::default().fg(MUTED)),
                    Span::styled(
                        compact_label(
                            &format!(
                                "{} · {} · CACHE {} · REQ {}/{}",
                                llm_generation_rate_label(sample),
                                llm_prefill_rate_label(sample),
                                percent(sample.llm_cache_efficiency),
                                count(sample.llm_active_requests),
                                count(sample.llm_waiting_requests),
                            ),
                            width.saturating_sub(6),
                        ),
                        Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("HOST  ", Style::default().fg(MUTED)),
                    Span::styled(
                        compact_label(
                            &format!(
                                "RSS {} · CPU {:.1}% · GPU {} · PAGE {}",
                                bytes(sample.llm_rss),
                                sample.llm_cpu,
                                percent_u8(sample.gpu_util),
                                if sample.rate_ready {
                                    rate(sample.swap_in.saturating_add(sample.swap_out))
                                } else {
                                    "sampling".into()
                                },
                            ),
                            width.saturating_sub(6),
                        ),
                        Style::default().fg(MUTED),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("DIAG  ", Style::default().fg(MUTED)),
                    Span::styled(
                        diagnosis,
                        Style::default()
                            .fg(sample.correlation.tone().color())
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
            ],
            sample.impact_tone,
        );
    }

    fn filtered_journal_events(&self) -> Vec<&SignalEvent> {
        self.collector
            .signals
            .iter()
            .filter(|event| self.journal_filter.matches(event.kind))
            .collect()
    }

    fn draw_journal_header(&self, frame: &mut Frame, area: Rect) {
        let filtered_events = self.filtered_journal_events();
        let event_count = filtered_events.len();
        let total_count = self.collector.signals.len();
        let latest = filtered_events
            .last()
            .map(|event| event.summary.as_str())
            .unwrap_or("waiting for the first recorded event");
        let latest_tone = filtered_events
            .last()
            .map(|event| event.tone)
            .unwrap_or(Tone::Muted);
        let latest_age = filtered_events
            .last()
            .map(|event| telemetry_age(Some(event.recorded_at)))
            .unwrap_or_else(|| "age —".into());
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        " JOURNAL  ",
                        Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            "{event_count} {} events · {total_count} total · f/[/] filter",
                            self.journal_filter.label(),
                        ),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled("  ·  ", Style::default().fg(DIM)),
                    Span::styled("meaningful changes only", Style::default().fg(MUTED)),
                ]),
                Line::from(vec![
                    Span::styled("  LATEST  ", Style::default().fg(MUTED)),
                    Span::styled(
                        format!("{latest} · {latest_age}"),
                        Style::default().fg(latest_tone.color()),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("  SCOPE   ", Style::default().fg(MUTED)),
                    Span::styled(
                        "what changed · why it matters · what recovered",
                        Style::default().fg(CYAN),
                    ),
                    Span::styled("  ·  ↑ newer · ↓ older", Style::default().fg(DIM)),
                ]),
            ])
            .block(panel("EVENT JOURNAL · IMPACT TIMELINE", Tone::Cyan))
            .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn draw_journal_events(&self, frame: &mut Frame, area: Rect) {
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
                        format!("{:^16}", compact_label(&event.state, 16)),
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
                " waiting for the first recorded event",
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

    fn draw_controls(&self, frame: &mut Frame, area: Rect) {
        let compact = area.width < 60;
        let hints = match (self.tab, compact) {
            (1, true) => vec![("↑↓", "select"), ("/", "filter"), ("q", "quit")],
            (2, true) => vec![("↑↓", "scroll"), ("f", "filter"), ("q", "quit")],
            (_, true) => vec![("p", "pause"), ("?", "help"), ("q", "quit")],
            (1, false) => vec![
                ("↑↓", "select"),
                ("s", "sort"),
                ("/", "filter"),
                ("tab", "view"),
                ("?", "help"),
                ("q", "quit"),
            ],
            (2, false) => vec![
                ("↑↓", "scroll"),
                ("f", "event filter"),
                ("tab", "view"),
                ("r", "reset"),
                ("?", "help"),
                ("q", "quit"),
            ],
            (_, false) => vec![
                ("p", if self.paused { "resume" } else { "pause" }),
                ("r", "reset"),
                ("tab", "view"),
                ("+/−", "interval"),
                ("?", "help"),
                ("q", "quit"),
            ],
        };
        let spans = hints
            .into_iter()
            .flat_map(|(key, label)| {
                [
                    Span::styled(
                        format!(" {key}"),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" {label}  "), Style::default().fg(MUTED)),
                ]
            })
            .collect::<Vec<_>>();
        let controls = Paragraph::new(Line::from(spans))
            .alignment(Alignment::Right)
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(DIM)),
            );
        frame.render_widget(controls, area);
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let popup = if area.width < 120 || area.height < 36 {
            centered_rect(94, 88, area)
        } else {
            centered_rect(60, 58, area)
        };
        frame.render_widget(Clear, popup);
        let text = vec![
            Line::from(Span::styled(
                "KEYBOARD",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("q / Esc / Ctrl-C   quit"),
            Line::from("p / Space          pause or resume sampling"),
            Line::from("r                  reset rates, charts, and journal"),
            Line::from("a                  acknowledge the active paging alert"),
            Line::from(
                "Alerts             aggressive paging rings the terminal bell and shows a banner",
            ),
            Line::from("Tab / ← →          cycle Overview, MLX Top, and Journal"),
            Line::from("1 / 2 / 3          jump to Overview, MLX Top, or Journal"),
            Line::from(
                "MLX Top            live process/resource monitor: PID, CPU, MEM%, RSS, PAGEIN/s, state, model",
            ),
            Line::from("↑ / ↓ / PgUp/PgDn  select a process; Home/End jump to first/last"),
            Line::from("s                  cycle sort: RSS, CPU, PID, NAME"),
            Line::from("f or /             filter processes; c clears the filter"),
            Line::from("Journal            historical transitions only: what changed and why"),
            Line::from("↑ / PgUp / Home newer; ↓ / PgDn / End older in the Journal"),
            Line::from("f / [ / ]          cycle Journal event filters"),
            Line::from("+ / -              change refresh interval (1–60s)"),
            Line::from("? / h              close this help"),
            Line::from(""),
            Line::from(Span::styled(
                "Every panel is framed around what the current machine state does to local LLM latency, headroom, or throughput.",
                Style::default().fg(MUTED),
            )),
        ];
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

    fn draw_compact_warning(&self, frame: &mut Frame, area: Rect) {
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

fn threshold_with_hysteresis(value: u64, was_active: bool, enter: u64, exit: u64) -> bool {
    value >= if was_active { exit } else { enter }
}

fn classify(sample: &mut Sample, previous: Option<&Sample>) {
    let swap_churn = sample.swap_in.saturating_add(sample.swap_out);
    let comp_churn = sample.compress.saturating_add(sample.decompress);
    let llm = llm_is_observed(sample);
    let previous_impact = previous.map(|sample| sample.impact.as_str());
    let was_paging = matches!(
        previous_impact,
        Some("PAGING ACTIVE" | "WATCH PAGING" | "HEAVY PAGING" | "SWAP THRASHING")
    );
    let was_compressing = matches!(previous_impact, Some("COMPRESSION ACTIVE"));
    let was_gpu_busy = matches!(previous_impact, Some("GPU BUSY"));
    let paging_active = threshold_with_hysteresis(swap_churn, was_paging, 4 * MIB, SWAP_WARN_EXIT);
    let compression_active = threshold_with_hysteresis(
        comp_churn,
        was_compressing,
        COMPRESSION_WARN_RATE,
        COMPRESSION_WARN_EXIT,
    );
    let watch_paging = swap_churn >= SWAP_WARN_RATE;
    let swap_thrashing =
        sample.swap_in >= SWAP_CRITICAL_RATE && sample.swap_out >= SWAP_CRITICAL_RATE;
    let heavy_paging = sample.swap_out >= 2 * SWAP_CRITICAL_RATE
        || sample.swap_growth >= (2 * SWAP_CRITICAL_RATE) as i64;
    let page_in_recovery = sample.swap_in >= 2 * SWAP_CRITICAL_RATE && sample.swap_growth <= 0;
    let gpu_busy = threshold_with_hysteresis(
        sample.gpu_util.unwrap_or_default() as u64,
        was_gpu_busy,
        80,
        GPU_WARN_EXIT,
    ) && llm;
    let native_counters_available = sample.total_memory > 0
        && sample.availability.is_some()
        && sample.pressure != "UNKNOWN"
        && sample.vm_available
        && sample.swap_available;
    let (impact, tone, health, grade, limiter) = if !sample.rate_ready {
        (
            "SAMPLING",
            Tone::Cyan,
            None,
            "SAMPLING",
            "collecting baseline",
        )
    } else if !native_counters_available {
        (
            "DATA LIMITED",
            Tone::Muted,
            None,
            "UNKNOWN",
            "native counter unavailable",
        )
    } else if sample.pressure == "RED" {
        (
            "MEMORY BOTTLENECK",
            Tone::Red,
            Some(10),
            "CRITICAL",
            "memory pressure",
        )
    } else if swap_thrashing {
        ("SWAP THRASHING", Tone::Red, Some(20), "POOR", "swap thrash")
    } else if heavy_paging {
        ("HEAVY PAGING", Tone::Red, Some(30), "POOR", "disk paging")
    } else if page_in_recovery {
        (
            "PAGE-IN RECOVERY",
            Tone::Red,
            Some(45),
            "RECOVERING",
            "page-in recovery",
        )
    } else if sample.pressure == "YELLOW" {
        (
            "MEMORY STRESS",
            Tone::Yellow,
            Some(55),
            "DEGRADED",
            "tight memory",
        )
    } else if sample.thermal.starts_with("limited") || sample.thermal == "warning reported" {
        (
            "THERMAL LIMIT",
            Tone::Yellow,
            Some(60),
            "DEGRADED",
            "thermal limit",
        )
    } else if paging_active {
        (
            "PAGING ACTIVE",
            Tone::Yellow,
            Some(65),
            "CONSTRAINED",
            "active paging",
        )
    } else if compression_active {
        (
            "COMPRESSION ACTIVE",
            Tone::Yellow,
            Some(70),
            "CONSTRAINED",
            "compression churn",
        )
    } else if watch_paging {
        (
            "WATCH PAGING",
            Tone::Yellow,
            Some(85),
            "GOOD",
            "light paging",
        )
    } else if gpu_busy {
        ("GPU BUSY", Tone::Cyan, Some(90), "GOOD", "GPU compute")
    } else if llm {
        ("LLM READY", Tone::Green, Some(100), "HEALTHY", "none")
    } else {
        ("IDLE", Tone::Green, Some(100), "HEALTHY", "none")
    };
    sample.impact = impact.into();
    sample.impact_tone = tone;
    sample.health = health;
    sample.grade = grade.into();
    sample.limiter = limiter.into();

    let largest = sample.largest_consumer.as_deref().unwrap_or("largest app");
    if !sample.rate_ready {
        sample.guidance_badge = "WAIT".into();
        sample.guidance_cause = "Collecting the live-rate baseline.".into();
        sample.guidance_action = "Wait one refresh before acting.".into();
    } else if !native_counters_available {
        sample.guidance_badge = "CHECK".into();
        sample.guidance_cause = "One or more native counters are unavailable.".into();
        sample.guidance_action =
            "Run on supported macOS hardware and verify system command access.".into();
    } else if sample.pressure == "RED" {
        sample.guidance_badge = "ACT NOW".into();
        sample.guidance_cause = "Critical pressure — macOS cannot reclaim RAM fast enough.".into();
        sample.guidance_action = if llm {
            "Stop unused models/requests; reduce context or concurrency.".into()
        } else {
            "Pause or quit the largest app first; wait for critical pressure to clear.".into()
        };
    } else if swap_thrashing {
        sample.guidance_badge = "ACT NOW".into();
        sample.guidance_cause = format!(
            "Swap thrash — {} in + {} out.",
            rate(sample.swap_in),
            rate(sample.swap_out)
        );
        sample.guidance_action = "Reduce model/context/KV cache or parallel requests.".into();
    } else if sample.swap_out >= SWAP_CRITICAL_RATE
        || sample.swap_growth >= SWAP_CRITICAL_RATE as i64
    {
        sample.guidance_badge = "ACT NOW".into();
        sample.guidance_cause =
            format!("RAM overflow — evicting {} to disk.", rate(sample.swap_out));
        sample.guidance_action = if llm {
            "Stop unused models or reduce model/context/cache.".into()
        } else {
            format!("Pause or quit {largest}; wait for swap-out to approach zero.")
        };
    } else if paging_active {
        sample.guidance_badge = "WATCH".into();
        sample.guidance_cause = format!(
            "Paging active — in {} + out {}.",
            rate(sample.swap_in),
            rate(sample.swap_out)
        );
        sample.guidance_action =
            "Reduce model/context/cache if paging persists during inference.".into();
    } else if compression_active {
        sample.guidance_badge = "WATCH".into();
        sample.guidance_cause = format!(
            "Compression active — {} compressed + {} decompressed.",
            rate(sample.compress),
            rate(sample.decompress)
        );
        sample.guidance_action =
            "Watch for rising paging or pressure; compression alone is not a bottleneck.".into();
    } else if watch_paging {
        sample.guidance_badge = "WATCH".into();
        sample.guidance_cause = format!("Light paging — {} total.", rate(swap_churn));
        sample.guidance_action =
            "No immediate action; investigate if the rate persists or rises.".into();
    } else if sample.pressure == "YELLOW" {
        sample.guidance_badge = "REDUCE".into();
        sample.guidance_cause = "Resident memory is tight; paging may follow.".into();
        sample.guidance_action = if llm {
            "Reduce model/context/cache or concurrency before adding requests.".into()
        } else {
            format!("Close an unneeded large app (start with {largest}).")
        };
    } else if sample.thermal.starts_with("limited") || sample.thermal == "warning reported" {
        sample.guidance_badge = "COOL".into();
        sample.guidance_cause = "Thermal limiting — memory is not the bottleneck.".into();
        sample.guidance_action =
            "Reduce batch/concurrency or pause until the warning clears.".into();
    } else if gpu_busy {
        sample.guidance_badge = "GPU".into();
        sample.guidance_cause = "GPU saturated — inference is compute-bound.".into();
        sample.guidance_action =
            "For lower latency, reduce work or use a smaller/faster model.".into();
    } else {
        sample.guidance_badge = "OK".into();
        sample.guidance_cause =
            "No active memory, paging, compression, or thermal bottleneck.".into();
        sample.guidance_action =
            "Nothing to fix; used swap can remain high after pressure passes.".into();
    }
}

fn signal_summary(sample: &Sample) -> String {
    match sample.impact.as_str() {
        "SWAP THRASHING" => format!(
            "in {} / out {}",
            rate(sample.swap_in),
            rate(sample.swap_out)
        ),
        "HEAVY PAGING" => format!(
            "swap {} / growth {}",
            rate(sample.swap_in + sample.swap_out),
            signed_rate(sample.swap_growth)
        ),
        "MEMORY BOTTLENECK" => format!(
            "native pressure critical · LLM RSS {} · {}% free",
            bytes(sample.llm_rss),
            sample.availability.unwrap_or(0)
        ),
        "PAGE-IN RECOVERY" => format!(
            "page-in {} · growth {}",
            rate(sample.swap_in),
            signed_rate(sample.swap_growth)
        ),
        "GPU BUSY" if !sample.correlation.summary.is_empty() => sample.correlation.summary.clone(),
        "GPU BUSY" => format!(
            "GPU {}% busy · {}",
            sample.gpu_util.unwrap_or(0),
            llm_generation_rate_label(sample)
        ),
        "LLM READY" => format!(
            "{} process(es) · {} · {} · cache {}",
            sample.llm_count,
            llm_generation_rate_label(sample),
            llm_prefill_rate_label(sample),
            percent(sample.llm_cache_efficiency)
        ),
        _ => sample.limiter.clone(),
    }
}

fn llm_is_observed(sample: &Sample) -> bool {
    sample.llm_count > 0
        || sample.llm_source == TelemetrySource::Live
        || sample.llm_generation_tps.is_some()
        || sample.llm_model != "not detected"
}

fn hero_impact(sample: &Sample) -> String {
    if sample.impact == "DATA LIMITED" {
        return "Native counters are incomplete; current LLM impact cannot be classified safely."
            .into();
    }
    if !llm_is_observed(sample) {
        return "No local LLM process detected; this score describes machine capacity.".into();
    }
    if !sample.correlation.summary.is_empty() {
        return sample.correlation.summary.clone();
    }
    if sample.pressure == "RED" {
        return "Memory pressure can evict model pages and increase prefill/decode latency.".into();
    }
    let paging = sample.swap_in.saturating_add(sample.swap_out);
    if paging > 0 {
        return format!(
            "Live paging ({}) can stall model prefill or decode.",
            rate(paging)
        );
    }
    if sample.gpu_util.unwrap_or(0) >= 80 {
        return "GPU is carrying the workload; throughput is compute-bound.".into();
    }
    let headroom = sample
        .availability
        .map(|value| format!("{value}% system free"))
        .unwrap_or_else(|| "unknown system headroom".into());
    format!(
        "{} RSS · {headroom} · no active latency stall.",
        bytes(sample.llm_rss)
    )
}

/// Keep the card diagnosis readable while retaining the full evidence in the
/// journal and static report. A card has one `WHY` row; putting every byte
/// counter in that row makes the actual cause disappear behind an ellipsis.
fn correlation_display(sample: &Sample, max_chars: usize) -> Option<String> {
    if sample.correlation.summary.is_empty() || max_chars == 0 {
        return None;
    }

    let rate_prefix = sample
        .correlation
        .summary
        .split(" · correlated:")
        .next()
        .unwrap_or(sample.correlation.summary.as_str());
    let rate_label = rate_prefix
        .split(" · no matching system signal")
        .next()
        .unwrap_or(rate_prefix);
    let rate = rate_label
        .split_once(" (")
        .map(|(value, _)| value)
        .unwrap_or(rate_label);
    let evidence = correlation_evidence_label(sample);
    let full = if evidence.is_empty() {
        rate.to_owned()
    } else {
        format!("{rate} · {evidence}")
    };
    if full.chars().count() <= max_chars {
        Some(full)
    } else {
        let compact_rate = sample
            .llm_generation_tps
            .map(|_| llm_generation_rate_label(sample))
            .unwrap_or_else(|| "GEN —".into());
        let compact = if evidence.is_empty() {
            compact_rate
        } else {
            format!("{compact_rate} · {evidence}")
        };
        Some(compact_label(&compact, max_chars))
    }
}

fn correlation_evidence_label(sample: &Sample) -> String {
    match sample.correlation.cause {
        CorrelationCause::Paging if sample.swap_in.saturating_add(sample.swap_out) > 0 => {
            format!(
                "I/O {}",
                rate(sample.swap_in.saturating_add(sample.swap_out))
            )
        }
        CorrelationCause::Compression if sample.compress.saturating_add(sample.decompress) > 0 => {
            format!(
                "compress {}",
                rate(sample.compress.saturating_add(sample.decompress))
            )
        }
        CorrelationCause::MemoryPressure => {
            format!("pressure {}", pressure_state_label(sample))
        }
        CorrelationCause::Thermal => "thermal limit".into(),
        CorrelationCause::MetalMemory => fraction(sample.gpu_in_use, sample.gpu_alloc)
            .map(|ratio| format!("Metal mem {:.0}%", ratio * 100.0))
            .unwrap_or_else(|| "Metal mem".into()),
        CorrelationCause::GpuSaturation => [
            sample.gpu_util,
            sample.metal.renderer_util,
            sample.metal.tiler_util,
        ]
        .into_iter()
        .flatten()
        .max()
        .map(|value| format!("GPU {value}%"))
        .unwrap_or_else(|| "GPU".into()),
        CorrelationCause::Queueing => sample
            .llm_waiting_requests
            .map(|waiting| format!("queue {waiting} waiting"))
            .unwrap_or_else(|| "queueing".into()),
        CorrelationCause::ContextGrowth => llm_context_tokens(sample)
            .map(|tokens| format!("context {}", compact_tokens(tokens)))
            .unwrap_or_else(|| "context/KV".into()),
        CorrelationCause::ModelMemory => {
            fraction(sample.llm_model_memory, sample.llm_model_memory_max)
                .map(|ratio| format!("model mem {:.0}%", ratio * 100.0))
                .unwrap_or_else(|| "model memory".into())
        }
        CorrelationCause::Runtime => "workload/runtime".into(),
        CorrelationCause::None => String::new(),
        CorrelationCause::Paging | CorrelationCause::Compression => String::new(),
    }
}

fn normalize_chart_value(metric: ChartMetric, value: u64) -> u64 {
    match metric {
        ChartMetric::Generation | ChartMetric::Prefill => value,
        ChartMetric::Cache | ChartMetric::Memory | ChartMetric::Gpu => value.min(100),
        ChartMetric::Swap => swap_chart_percent(value),
    }
}

fn swap_chart_percent(value: u64) -> u64 {
    if value == 0 {
        return 0;
    }
    let anchor = SWAP_CHART_LOG_ANCHOR as f64;
    let numerator = (1.0 + value as f64 / anchor).ln();
    let denominator = (1.0 + SWAP_CHART_SCALE as f64 / anchor).ln();
    (100.0 * numerator / denominator).round().min(100.0) as u64
}

fn chart_stats<'a, I>(points: I, metric: ChartMetric) -> (Option<u64>, Option<u64>)
where
    I: IntoIterator<Item = &'a ChartPoint>,
{
    let mut count = 0_u64;
    let mut total = 0_u64;
    let mut peak = 0_u64;
    for value in points.into_iter().filter_map(|point| point.value) {
        // Paging stats stay in bytes/s: a percent of the log scale would be
        // meaningless next to the byte-rate label shown for the live value.
        let value = if matches!(
            metric,
            ChartMetric::Generation | ChartMetric::Prefill | ChartMetric::Swap
        ) {
            value
        } else {
            normalize_chart_value(metric, value)
        };
        count += 1;
        total = total.saturating_add(value);
        peak = peak.max(value);
    }
    if count == 0 {
        (None, None)
    } else {
        (
            Some(total.checked_div(count).unwrap_or_default()),
            Some(peak),
        )
    }
}

fn chart_stats_for_width(
    history: &VecDeque<ChartPoint>,
    metric: ChartMetric,
    width: usize,
) -> (Option<u64>, Option<u64>) {
    let visible_start = history.len().saturating_sub(width);
    chart_stats(history.iter().skip(visible_start), metric)
}

fn chart_stat_label(metric: ChartMetric, value: Option<u64>) -> String {
    match metric {
        ChartMetric::Generation | ChartMetric::Prefill => value
            .map(|value| format!("{:.1}", value as f64 / 10.0))
            .unwrap_or_else(|| "—".into()),
        ChartMetric::Swap => value.map(rate).unwrap_or_else(|| "—".into()),
        ChartMetric::Cache | ChartMetric::Memory | ChartMetric::Gpu => value
            .map(|value| format!("{value}%"))
            .unwrap_or_else(|| "—".into()),
    }
}

fn chart_scale_max(_history: &VecDeque<ChartPoint>, metric: ChartMetric) -> u64 {
    match metric {
        ChartMetric::Generation => GENERATION_CHART_SCALE_MAX,
        ChartMetric::Prefill => PREFILL_CHART_SCALE_MAX,
        ChartMetric::Cache | ChartMetric::Memory | ChartMetric::Swap | ChartMetric::Gpu => 100,
    }
}

fn chart_display_value(metric: ChartMetric, value: u64, scale_max: u64) -> u64 {
    if matches!(metric, ChartMetric::Generation | ChartMetric::Prefill) {
        value
            .saturating_mul(100)
            .checked_div(scale_max.max(1))
            .unwrap_or(0)
            .min(100)
    } else {
        normalize_chart_value(metric, value)
    }
}

fn chart_window_label(samples: usize, interval: Duration) -> String {
    let seconds = (samples as u64).saturating_mul(interval.as_secs().max(1));
    if seconds >= 60 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

fn chart_columns(history: &VecDeque<ChartPoint>, width: usize) -> Vec<RenderPoint> {
    if width == 0 {
        return Vec::new();
    }

    let visible_start = history.len().saturating_sub(width);
    let left_padding = width.saturating_sub(history.len() - visible_start);
    let mut columns = vec![
        RenderPoint {
            value: None,
            tone: Tone::Muted,
            break_before: true,
        };
        width
    ];

    for (offset, point) in history.iter().skip(visible_start).enumerate() {
        columns[left_padding + offset] = RenderPoint {
            value: point.value,
            tone: point.tone,
            break_before: point.value.is_none(),
        };
    }
    columns
}

fn chart_columns_for_plot(
    history: &VecDeque<ChartPoint>,
    width: usize,
    metric: ChartMetric,
    plot_height: usize,
) -> Vec<RenderPoint> {
    let mut columns = chart_columns(history, width);
    let visible_start = history.len().saturating_sub(width);
    let left_padding = width.saturating_sub(history.len() - visible_start);
    let plot_values = chart_plot_values(history, metric, plot_height);
    for (offset, value) in plot_values.iter().skip(visible_start).enumerate() {
        columns[left_padding + offset].value = *value;
    }
    columns
}

fn chart_plot_values(
    history: &VecDeque<ChartPoint>,
    metric: ChartMetric,
    plot_height: usize,
) -> Vec<Option<u64>> {
    let scale_max = chart_scale_max(history, metric);
    let deadband = chart_visual_deadband(plot_height);
    let mut anchor = None;
    let mut values = Vec::with_capacity(history.len());
    for point in history {
        let Some(value) = point.value else {
            anchor = None;
            values.push(None);
            continue;
        };
        let plotted = match anchor {
            Some(previous)
                if chart_display_delta(metric, previous, value, scale_max) <= deadband =>
            {
                previous
            }
            _ => value,
        };
        anchor = Some(plotted);
        values.push(Some(plotted));
    }
    values
}

fn chart_visual_deadband(plot_height: usize) -> f64 {
    let drawable_rows = plot_height.saturating_sub(1);
    if drawable_rows == 0 {
        100.0
    } else {
        100.0 / drawable_rows as f64 * CHART_VISUAL_DEADBAND_FRACTION
    }
}

fn chart_display_delta(metric: ChartMetric, left: u64, right: u64, scale_max: u64) -> f64 {
    let left = chart_display_value(metric, left, scale_max) as f64;
    let right = chart_display_value(metric, right, scale_max) as f64;
    (left - right).abs()
}

fn trace_point(value: u64, height: usize) -> Option<(usize, char)> {
    if height == 0 {
        return None;
    }
    let value = value.min(100);
    let row_count = height.saturating_sub(1) as u64;
    let row_from_bottom = value.saturating_mul(row_count) / 100;
    let row = height - 1 - row_from_bottom.min(row_count) as usize;
    Some((row, '━'))
}

fn trace_connector(
    cells: &mut [Vec<TraceCell>],
    column: usize,
    previous_row: usize,
    row: usize,
    metric: ChartMetric,
    previous_tone: Tone,
    tone: Tone,
) {
    if column >= cells.first().map(Vec::len).unwrap_or(0)
        || previous_row == row
        || previous_row >= cells.len()
        || row >= cells.len()
    {
        return;
    }
    let upper = previous_row.min(row);
    let lower = previous_row.max(row);
    let height = cells.len();
    for (offset, row_cells) in cells.iter_mut().enumerate().take(lower).skip(upper + 1) {
        let display_value = chart_row_value(offset, height);
        row_cells[column] = TraceCell {
            glyph: '┃',
            tone: chart_transition_tone(metric, display_value, previous_tone, tone),
        };
    }
    cells[upper][column] = TraceCell {
        glyph: if row > previous_row { '┓' } else { '┏' },
        tone: if upper == previous_row {
            previous_tone
        } else {
            tone
        },
    };
    cells[lower][column] = TraceCell {
        glyph: if row > previous_row { '┗' } else { '┛' },
        tone: if lower == previous_row {
            previous_tone
        } else {
            tone
        },
    };
}

fn chart_row_value(row: usize, height: usize) -> u64 {
    let row_count = height.saturating_sub(1) as u64;
    if row_count == 0 {
        return 0;
    }
    (height.saturating_sub(1).saturating_sub(row) as u64)
        .saturating_mul(100)
        .checked_div(row_count)
        .unwrap_or(0)
}

fn chart_transition_tone(
    metric: ChartMetric,
    display_value: u64,
    previous_tone: Tone,
    tone: Tone,
) -> Tone {
    match metric {
        ChartMetric::Generation | ChartMetric::Prefill => {
            if previous_tone == tone {
                tone
            } else {
                Tone::Muted
            }
        }
        ChartMetric::Cache => tone,
        ChartMetric::Memory | ChartMetric::Gpu => metric.tone(display_value),
        ChartMetric::Swap => metric.tone(display_value.saturating_mul(16 * MIB) / 100),
    }
}

fn llm_model_label(sample: &Sample, max_chars: usize) -> String {
    compact_label(&sample.llm_model, max_chars)
}

fn compact_label(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.into();
    }
    let mut label: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    label.push('…');
    label
}

fn tokens_per_second(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1} tok/s"))
        .unwrap_or_else(|| "—".into())
}

/// Return a live request rate for a chart. Aggregate session rates and
/// completion-log values are deliberately excluded because they are not
/// measurements of the current sample.
fn chart_rate_value(sample: &Sample, metric: ChartMetric) -> Option<u64> {
    let (value, live) = match metric {
        ChartMetric::Generation => (sample.llm_generation_tps, sample.llm_generation_tps_live),
        ChartMetric::Prefill => (sample.llm_prefill_tps, sample.llm_prefill_tps_live),
        _ => return None,
    };
    if !live {
        return None;
    }
    value
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| (value * 10.0).round() as u64)
}

fn cache_interval_efficiency(
    previous: &mut Option<CacheCounters>,
    telemetry: Option<&LlmTelemetry>,
    stale: bool,
) -> Option<f64> {
    if stale {
        return None;
    }
    let Some(telemetry) = telemetry else {
        *previous = None;
        return None;
    };
    let Some(counters) = telemetry
        .total_prompt_tokens
        .zip(telemetry.total_cached_tokens)
        .map(|(prompt_tokens, cached_tokens)| CacheCounters {
            prompt_tokens,
            cached_tokens,
        })
    else {
        *previous = None;
        return None;
    };
    let previous = previous.replace(counters)?;
    let prompt_delta = counters
        .prompt_tokens
        .saturating_sub(previous.prompt_tokens);
    if prompt_delta == 0 {
        return None;
    }
    let cached_delta = counters
        .cached_tokens
        .saturating_sub(previous.cached_tokens)
        .min(prompt_delta);
    Some(cached_delta as f64 / prompt_delta as f64 * 100.0)
}

fn llm_rate_label(sample: &Sample, metric: &str, value: Option<f64>, live: bool) -> String {
    let label = if live {
        metric.to_owned()
    } else if value.is_some() {
        match sample.llm_source {
            TelemetrySource::Live => format!("AVG {metric}"),
            TelemetrySource::Log => format!("LAST {metric}"),
            TelemetrySource::None => metric.to_owned(),
        }
    } else {
        metric.to_owned()
    };
    format!("{label} {}", tokens_per_second(value))
}

fn llm_generation_rate_label(sample: &Sample) -> String {
    llm_rate_label(
        sample,
        "GEN",
        sample.llm_generation_tps,
        sample.llm_generation_tps_live,
    )
}

fn llm_prefill_rate_label(sample: &Sample) -> String {
    llm_rate_label(
        sample,
        "PREFILL",
        sample.llm_prefill_tps,
        sample.llm_prefill_tps_live,
    )
}

fn chart_inactive_rate_label(metric: ChartMetric, sample: &Sample) -> String {
    match (metric, sample.llm_status.as_str()) {
        (ChartMetric::Generation, "prefilling") => "prefill active".into(),
        (ChartMetric::Generation, "generating") => "active".into(),
        (ChartMetric::Prefill, "prefilling") => "active".into(),
        (ChartMetric::Prefill, "generating") => "decode active".into(),
        (_, "idle" | "last result") => "idle".into(),
        (_, "waiting") => "waiting".into(),
        (_, "offline") => "offline".into(),
        _ => "—".into(),
    }
}

fn percent(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "—".into())
}

fn percent_u8(value: Option<u8>) -> String {
    value
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "—".into())
}

fn optional_tokens(value: Option<u64>) -> String {
    value.map(compact_tokens).unwrap_or_else(|| "—".into())
}

fn mlx_runtime_summary(mlx: &MlxTelemetry) -> String {
    let mut parts = vec![mlx
        .version
        .as_deref()
        .map(|version| format!("MLX v{version}"))
        .unwrap_or_else(|| "MLX runtime".into())];
    if let Some(value) = mlx.active_memory {
        parts.push(format!("active {}", bytes(value)));
    }
    if let Some(value) = mlx.cache_memory {
        parts.push(format!("cache {}", bytes(value)));
    }
    if let Some(value) = mlx.peak_memory {
        parts.push(format!("peak {}", bytes(value)));
    }
    if parts.len() == 1 {
        parts.push("allocator counters not exposed".into());
    }
    parts.join(" · ")
}

fn llm_context_label(sample: &Sample) -> String {
    llm_context_tokens(sample)
        .map(compact_tokens)
        .unwrap_or_else(|| "—".into())
}

/// Translate native pressure levels into words that describe the operating
/// condition. Color remains a secondary visual cue; it is never the diagnosis
/// shown to the user.
fn pressure_state_label(sample: &Sample) -> &'static str {
    match sample.pressure.as_str() {
        "GREEN" => "normal",
        "YELLOW" => "watch",
        "RED" => "critical",
        _ => match sample.pressure_meaning.as_str() {
            "normal" => "normal",
            "warning" => "watch",
            "critical" => "critical",
            _ => "unavailable",
        },
    }
}

fn gpu_load_label(value: Option<u8>) -> &'static str {
    match value.map(u64::from) {
        Some(value) if value >= GPU_CRITICAL_LOAD => "saturated",
        Some(value) if value >= GPU_WARN_LOAD => "loaded",
        Some(_) => "within target",
        None => "unavailable",
    }
}

fn diagnostic_signal_line(sample: &Sample) -> String {
    let gpu = sample
        .gpu_util
        .map(|value| format!("GPU {value}%"))
        .unwrap_or_else(|| "GPU —".into());
    let paging = if sample.swap_available && sample.vm_available && sample.rate_ready {
        format!(
            "I/O {}",
            rate(sample.swap_in.saturating_add(sample.swap_out))
        )
    } else {
        "I/O sampling".into()
    };
    let confidence = if sample.correlation.summary.is_empty() {
        String::new()
    } else {
        format!(" · {} confidence", sample.correlation.confidence_label())
    };
    format!(
        "{gpu} · {paging} · PRESSURE {}{confidence}",
        pressure_state_label(sample)
    )
}

fn metal_signal_line(sample: &Sample, gpu_memory: &str) -> String {
    let mut parts = Vec::new();
    if let Some(device) = sample.metal.device_name.as_deref() {
        parts.push(compact_label(device, 14));
    }
    if let Some(cores) = sample.metal.gpu_cores {
        parts.push(format!("{cores} cores"));
    }
    if gpu_memory != "not exposed" {
        parts.push(format!("mem {gpu_memory}"));
    }
    if let Some(renderer) = sample.metal.renderer_util {
        parts.push(format!("R {renderer}%"));
    }
    if let Some(tiler) = sample.metal.tiler_util {
        parts.push(format!("T {tiler}%"));
    }
    parts.push(format!("thermal {}", sample.thermal));
    parts.join(" · ")
}

fn telemetry_source(sample: &Sample) -> String {
    match sample.llm_source {
        TelemetrySource::Live => {
            let age = sample
                .llm_observed_at
                .and_then(|observed_at| SystemTime::now().duration_since(observed_at).ok())
                .map(|age| age.as_secs())
                .unwrap_or_default();
            if age >= 2 {
                format!("LIVE {}s old", age)
            } else {
                "LIVE".into()
            }
        }
        TelemetrySource::Log => format!("LOG {}", telemetry_age(sample.llm_observed_at)),
        TelemetrySource::None => "SOURCE —".into(),
    }
}

fn telemetry_age(observed_at: Option<SystemTime>) -> String {
    let Some(observed_at) = observed_at else {
        return "age —".into();
    };
    let seconds = SystemTime::now()
        .duration_since(observed_at)
        .map(|age| age.as_secs())
        .unwrap_or(0);
    if seconds < 60 {
        format!("{seconds}s old")
    } else if seconds < 3600 {
        format!("{}m old", seconds / 60)
    } else {
        format!("{}h old", seconds / 3600)
    }
}

fn count(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "—".into())
}

fn process_count_label(sample: &Sample) -> String {
    if sample.llm_count == 1 {
        "1 process".into()
    } else {
        format!("{} processes", sample.llm_count)
    }
}

fn compact_model_memory(sample: &Sample) -> String {
    match (sample.llm_model_memory, sample.llm_model_memory_max) {
        (Some(used), Some(max)) if max > 0 => format!(
            "{:.1}/{:.1}G",
            used as f64 / 1024_f64.powi(3),
            max as f64 / 1024_f64.powi(3)
        ),
        (Some(used), None) => format!("{:.1}G", used as f64 / 1024_f64.powi(3)),
        _ => "—".into(),
    }
}

impl CorrelationEngine {
    fn observe(&mut self, sample: &Sample) -> CorrelationInsight {
        let current = CorrelationObservation::from_sample(sample);
        let previous = self.observations.back().cloned();
        let comparable_previous = previous
            .as_ref()
            .filter(|previous| {
                previous.provider == current.provider && previous.model == current.model
            })
            .cloned();
        let baseline = self.baseline(&current.provider, &current.model);
        let insight = correlate_observations(&current, comparable_previous.as_ref(), baseline);

        self.observations.push_back(current);
        while self.observations.len() > CORRELATION_HISTORY_LIMIT {
            self.observations.pop_front();
        }
        insight
    }

    fn baseline(&self, provider: &str, model: &str) -> Option<f64> {
        let mut rates = self
            .observations
            .iter()
            .filter(|observation| observation.provider == provider && observation.model == model)
            .filter_map(|observation| observation.generation_tps)
            .filter(|value| value.is_finite() && *value > 0.0)
            .collect::<Vec<_>>();
        if rates.is_empty() {
            return None;
        }
        rates.sort_by(f64::total_cmp);
        let middle = rates.len() / 2;
        if rates.len().is_multiple_of(2) {
            Some((rates[middle - 1] + rates[middle]) / 2.0)
        } else {
            Some(rates[middle])
        }
    }

    fn reset(&mut self) {
        self.observations.clear();
    }
}

impl CorrelationObservation {
    fn from_sample(sample: &Sample) -> Self {
        Self {
            provider: sample.llm_provider.clone(),
            model: sample.llm_model.clone(),
            generation_tps: sample
                .llm_generation_tps_live
                .then_some(sample.llm_generation_tps)
                .flatten(),
            gpu_util: sample.gpu_util,
            renderer_util: sample.metal.renderer_util,
            tiler_util: sample.metal.tiler_util,
            paging_rate: sample.swap_in.saturating_add(sample.swap_out),
            compression_rate: sample.compress.saturating_add(sample.decompress),
            pressure: pressure_rank(&sample.pressure),
            thermal_limited: sample.thermal.starts_with("limited")
                || sample.thermal == "warning reported",
            model_memory: sample.llm_model_memory,
            model_memory_max: sample.llm_model_memory_max,
            metal_in_use: sample.gpu_in_use,
            metal_alloc: sample.gpu_alloc,
            context_tokens: llm_context_tokens(sample),
            active_requests: sample.llm_active_requests,
            waiting_requests: sample.llm_waiting_requests,
        }
    }
}

struct CorrelationFactor {
    cause: CorrelationCause,
    score: u8,
    evidence: String,
}

fn correlate_observations(
    current: &CorrelationObservation,
    previous: Option<&CorrelationObservation>,
    baseline: Option<f64>,
) -> CorrelationInsight {
    let current_tps = current.generation_tps.filter(|value| {
        value.is_finite() && (*value > 0.0 || current.active_requests.unwrap_or_default() > 0)
    });
    let baseline = baseline.filter(|value| value.is_finite() && *value > 0.0);
    let (direction, delta_percent) = match (current_tps, baseline) {
        (Some(current), Some(baseline)) => {
            let delta = current - baseline;
            let percent = delta / baseline * 100.0;
            let direction = if delta <= -THROUGHPUT_CHANGE_MIN_TPS
                && percent <= -(THROUGHPUT_CHANGE_RATIO * 100.0)
            {
                ThroughputDirection::Down
            } else if delta >= THROUGHPUT_CHANGE_MIN_TPS
                && percent >= THROUGHPUT_CHANGE_RATIO * 100.0
            {
                ThroughputDirection::Up
            } else {
                ThroughputDirection::Flat
            };
            (direction, Some(percent))
        }
        _ => (ThroughputDirection::Unknown, None),
    };

    let mut factors = Vec::new();
    if current.thermal_limited {
        push_correlation_factor(&mut factors, CorrelationCause::Thermal, 95, "thermal limit");
    }

    match current.pressure {
        4 => push_correlation_factor(
            &mut factors,
            CorrelationCause::MemoryPressure,
            100,
            "memory pressure critical",
        ),
        2 => push_correlation_factor(
            &mut factors,
            CorrelationCause::MemoryPressure,
            72,
            "memory pressure watch",
        ),
        _ => {}
    }

    if current.paging_rate >= SWAP_WARN_RATE {
        let score = if current.paging_rate >= SWAP_CRITICAL_RATE {
            100
        } else {
            88
        };
        push_correlation_factor(
            &mut factors,
            CorrelationCause::Paging,
            score,
            format!("paging {}", rate(current.paging_rate)),
        );
    }

    if current.compression_rate >= COMPRESSION_WARN_RATE {
        push_correlation_factor(
            &mut factors,
            CorrelationCause::Compression,
            68,
            format!("compression {}", rate(current.compression_rate)),
        );
    }

    if let Some(waiting) = current.waiting_requests.filter(|waiting| *waiting > 0) {
        let score = if waiting > 1 { 82 } else { 70 };
        push_correlation_factor(
            &mut factors,
            CorrelationCause::Queueing,
            score,
            format!(
                "queue {waiting} waiting · active {}",
                count(current.active_requests)
            ),
        );
    }

    let current_gpu = [current.gpu_util, current.renderer_util, current.tiler_util]
        .into_iter()
        .flatten()
        .max();
    if let Some(gpu) = current_gpu {
        if u64::from(gpu) >= GPU_CRITICAL_LOAD {
            let crossed = previous
                .and_then(|previous| {
                    [
                        previous.gpu_util,
                        previous.renderer_util,
                        previous.tiler_util,
                    ]
                    .into_iter()
                    .flatten()
                    .max()
                })
                .is_none_or(|value| u64::from(value) < GPU_CRITICAL_LOAD);
            push_correlation_factor(
                &mut factors,
                CorrelationCause::GpuSaturation,
                if crossed { 95 } else { 72 },
                format!("GPU {gpu}% busy"),
            );
        } else if gpu >= 80 {
            push_correlation_factor(
                &mut factors,
                CorrelationCause::GpuSaturation,
                58,
                format!("GPU {gpu}% busy"),
            );
        }
    }

    let current_metal_ratio = fraction(current.metal_in_use, current.metal_alloc);
    if let Some(ratio) = current_metal_ratio.filter(|ratio| *ratio >= 0.90) {
        let crossed = previous
            .and_then(|previous| fraction(previous.metal_in_use, previous.metal_alloc))
            .is_none_or(|value| value < 0.90);
        let evidence = match (current.metal_in_use, current.metal_alloc) {
            (Some(used), Some(allocated)) => format!(
                "Metal MEM {:.0}% ({}/{})",
                ratio * 100.0,
                bytes(used),
                bytes(allocated)
            ),
            _ => format!("Metal MEM {:.0}%", ratio * 100.0),
        };
        push_correlation_factor(
            &mut factors,
            CorrelationCause::MetalMemory,
            if crossed { 90 } else { 68 },
            evidence,
        );
    }

    let model_memory_delta = previous.and_then(|previous| {
        current
            .model_memory
            .zip(previous.model_memory)
            .map(|(current, previous)| current.saturating_sub(previous))
    });
    let current_model_ratio = fraction(current.model_memory, current.model_memory_max);
    let mut model_memory_evidence = Vec::new();
    let mut model_memory_score = 0;
    if let Some(ratio) = current_model_ratio.filter(|ratio| *ratio >= 0.90) {
        model_memory_score = 84;
        model_memory_evidence.push(format!("model MEM {:.0}% of ceiling", ratio * 100.0));
    }
    if let Some(delta) = model_memory_delta.filter(|delta| *delta >= MODEL_MEMORY_GROWTH) {
        model_memory_score = model_memory_score.max(72);
        model_memory_evidence.push(format!("model MEM +{}", bytes(delta)));
    }
    if model_memory_score > 0 {
        push_correlation_factor(
            &mut factors,
            CorrelationCause::ModelMemory,
            model_memory_score,
            model_memory_evidence.join(" · "),
        );
    }

    let context_delta = previous.and_then(|previous| {
        current
            .context_tokens
            .zip(previous.context_tokens)
            .map(|(current, previous)| current.saturating_sub(previous))
    });
    let mut context_evidence = Vec::new();
    let mut context_score = 0;
    if let Some(delta) = context_delta.filter(|delta| *delta >= CONTEXT_GROWTH_TOKENS) {
        context_score = 82;
        if let Some(total) = current.context_tokens {
            context_evidence.push(format!(
                "context +{} → {}",
                compact_tokens(delta),
                compact_tokens(total)
            ));
        }
    } else if direction == ThroughputDirection::Down
        && current
            .context_tokens
            .is_some_and(|tokens| tokens >= 16_384)
    {
        context_score = 48;
        context_evidence.push(format!(
            "context {}",
            compact_tokens(current.context_tokens.unwrap_or_default())
        ));
    }
    if context_score > 0 {
        push_correlation_factor(
            &mut factors,
            CorrelationCause::ContextGrowth,
            context_score,
            context_evidence.join(" · "),
        );
    }

    if direction == ThroughputDirection::Down && factors.is_empty() {
        push_correlation_factor(
            &mut factors,
            CorrelationCause::Runtime,
            20,
            "no matching system signal; workload/runtime changed",
        );
    }
    factors.sort_by_key(|factor| std::cmp::Reverse(factor.score));

    let cause = factors
        .first()
        .map(|factor| factor.cause)
        .unwrap_or_default();
    let confidence = factors.first().map(|factor| factor.score).unwrap_or(0);
    let rate_label = match (current_tps, baseline, direction, delta_percent) {
        (Some(current), Some(baseline), ThroughputDirection::Down, Some(percent)) => format!(
            "GEN ↓{:.1}% ({baseline:.1}→{current:.1} tok/s)",
            percent.abs()
        ),
        (Some(current), Some(baseline), ThroughputDirection::Up, Some(percent)) => {
            format!("GEN ↑{percent:.1}% ({baseline:.1}→{current:.1} tok/s)")
        }
        (Some(current), _, _, _) => format!("GEN {current:.1} tok/s"),
        _ => String::new(),
    };
    let evidence = factors
        .iter()
        .take(2)
        .map(|factor| factor.evidence.as_str())
        .collect::<Vec<_>>()
        .join(" + ");
    let summary = if rate_label.is_empty() {
        String::new()
    } else if evidence.is_empty() {
        if direction == ThroughputDirection::Down {
            format!("{rate_label} · no matching system signal")
        } else {
            String::new()
        }
    } else {
        format!("{rate_label} · correlated: {evidence}")
    };
    let details = if rate_label.is_empty() || factors.is_empty() {
        String::new()
    } else {
        let all_evidence = factors
            .iter()
            .map(|factor| factor.evidence.as_str())
            .collect::<Vec<_>>()
            .join(" · ");
        format!("{} · {all_evidence}", cause.label())
    };
    let event_key =
        (direction == ThroughputDirection::Down).then_some(CorrelationKey { direction, cause });

    CorrelationInsight {
        direction,
        cause,
        confidence,
        summary,
        details,
        event_key,
    }
}

fn push_correlation_factor(
    factors: &mut Vec<CorrelationFactor>,
    cause: CorrelationCause,
    score: u8,
    evidence: impl Into<String>,
) {
    factors.push(CorrelationFactor {
        cause,
        score,
        evidence: evidence.into(),
    });
}

fn pressure_rank(pressure: &str) -> u8 {
    match pressure {
        "GREEN" => 1,
        "YELLOW" => 2,
        "RED" => 4,
        _ => 0,
    }
}

fn fraction(numerator: Option<u64>, denominator: Option<u64>) -> Option<f64> {
    let denominator = denominator.filter(|value| *value > 0)?;
    let numerator = numerator?;
    Some(numerator as f64 / denominator as f64)
}

fn llm_context_tokens(sample: &Sample) -> Option<u64> {
    match (sample.llm_prompt_tokens, sample.llm_output_tokens) {
        (Some(prompt), Some(output)) => Some(prompt.saturating_add(output)),
        (Some(prompt), None) => Some(prompt),
        (None, Some(output)) => Some(output),
        (None, None) => None,
    }
}

fn compact_tokens(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn parse_vm_stat(text: &str, page_size: u64) -> VmCounters {
    let mut c = VmCounters::default();
    for line in text.lines() {
        let value = line
            .split(':')
            .nth(1)
            .and_then(|v| {
                v.split_whitespace()
                    .next()
                    .map(|value| value.trim_matches(|c: char| !c.is_ascii_digit()))
            })
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let bytes = value.saturating_mul(page_size);
        if line.starts_with("Pages wired") {
            c.wired = bytes;
        } else if line.starts_with("Pages occupied by compressor")
            || line.starts_with("Pages used by compressor")
        {
            c.compressor = bytes;
        } else if line.starts_with("Pages stored in compressor")
            || line.starts_with("Uncompressed pages")
        {
            c.compressed_logical = bytes;
        } else if line.starts_with("Anonymous pages") {
            c.anonymous = bytes;
        } else if line.starts_with("File-backed pages") {
            c.file_backed = bytes;
        } else if line.starts_with("Swapins") || line.contains("\"Swapins\"") {
            c.swapins = value;
        } else if line.starts_with("Swapouts") || line.contains("\"Swapouts\"") {
            c.swapouts = value;
        } else if line.starts_with("Compressions") || line.starts_with("Pages compressed") {
            c.compressions = value;
        } else if line.starts_with("Decompressions") || line.starts_with("Pages decompressed") {
            c.decompressions = value;
        } else if line.starts_with("Pages reactivated") {
            c.reactivations = value;
        }
    }
    c
}

fn parse_swap_usage(text: &str) -> (u64, u64) {
    let mut total = 0;
    let mut used = 0;
    let normalized = text.replace('=', " = ");
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        if (tokens[i] == "total" || tokens[i] == "used") && i + 1 < tokens.len() {
            let value_index = if tokens[i + 1] == "=" { i + 2 } else { i + 1 };
            if let Some(value) = tokens.get(value_index) {
                if tokens[i] == "total" {
                    total = parse_unit(value);
                } else {
                    used = parse_unit(value);
                }
                i = value_index;
            }
        }
        i += 1;
    }
    (total, used)
}

fn parse_gpu(text: &str) -> (Option<u8>, Option<u64>, Option<u64>) {
    (
        find_named_number(text, "Device Utilization %")
            .or_else(|| find_named_number(text, "Renderer Utilization %"))
            .map(|v| v as u8),
        find_named_numbers(text, "Alloc system memory")
            .into_iter()
            .next_back(),
        find_named_numbers(text, "In use system memory")
            .into_iter()
            .next_back(),
    )
}

fn parse_metal_hardware(text: &str) -> MetalTelemetry {
    MetalTelemetry {
        device_name: find_named_string(text, "model")
            .or_else(|| find_named_string(text, "MetalPluginName")),
        architecture: None,
        gpu_cores: find_named_number(text, "gpu-core-count")
            .and_then(|value| value.try_into().ok()),
        renderer_util: find_named_number(text, "Renderer Utilization %")
            .and_then(|value| value.try_into().ok()),
        tiler_util: find_named_number(text, "Tiler Utilization %")
            .and_then(|value| value.try_into().ok()),
        resource_limit: None,
    }
}

fn parse_thermal(text: &str) -> String {
    if let Some(value) = find_number(text, "CPU_Speed_Limit") {
        if value < 100 {
            return format!("limited {value}%");
        }
        return "no limit".into();
    }
    if text.is_empty() {
        "unavailable".into()
    } else if text.contains("No thermal warning") || text.contains("No performance warning") {
        "no warning".into()
    } else {
        "warning reported".into()
    }
}

fn parse_processes(text: &str) -> ProcessSnapshot {
    let mut llm_count = 0;
    let mut llm_rss = 0;
    let mut llm_cpu = 0.0;
    let mut provider: Option<String> = None;
    let mut consumers: Vec<Consumer> = Vec::new();
    let mut llm_processes: Vec<LlmProcess> = Vec::new();

    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 {
            continue;
        }
        let Ok(pid) = fields[0].parse::<u32>() else {
            continue;
        };
        let Ok(rss_kib) = fields[1].parse::<u64>() else {
            continue;
        };
        let Ok(cpu) = fields[2].parse::<f64>() else {
            continue;
        };
        let modern_memory_percent = fields
            .get(3)
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0);
        let (memory_percent, state, pageins, name_index, command_index) =
            if modern_memory_percent.is_some() {
                (
                    modern_memory_percent,
                    fields.get(4).copied().unwrap_or("?").to_string(),
                    fields.get(5).and_then(|value| value.parse::<u64>().ok()),
                    6,
                    7,
                )
            } else {
                (None, "?".into(), None, 3, 4)
            };
        let Some(name_field) = fields.get(name_index) else {
            continue;
        };
        let name = name_field
            .rsplit('/')
            .next()
            .unwrap_or(name_field)
            .to_string();
        let command = fields
            .get(command_index..)
            .map(|parts| parts.join(" "))
            .unwrap_or_else(|| name.clone());
        let lower = line.to_ascii_lowercase();
        let is_llm = is_llm_process(&name, &command);
        if is_llm {
            if provider.is_none() {
                provider = process_provider(&name, &lower);
            }
            llm_count += 1;
            llm_rss += rss_kib * 1024;
            llm_cpu += cpu;
            llm_processes.push(LlmProcess {
                pid,
                name: name.clone(),
                command: command.clone(),
                rss: rss_kib * 1024,
                cpu,
                memory_percent,
                state,
                pageins,
                pagein_rate: None,
            });
        }

        if lower.contains("mlxtop") || name == "ps" || name == "awk" {
            continue;
        }
        if let Some(consumer) = consumers.iter_mut().find(|c| c.name == name) {
            consumer.rss += rss_kib * 1024;
            consumer.processes += 1;
        } else {
            consumers.push(Consumer {
                name,
                rss: rss_kib * 1024,
                processes: 1,
            });
        }
    }
    consumers.sort_by_key(|consumer| std::cmp::Reverse(consumer.rss));
    let largest_consumer = consumers.first().map(|consumer| consumer.name.clone());
    llm_processes.sort_by_key(|process| std::cmp::Reverse(process.rss));
    llm_processes.truncate(32);
    let top_llm = llm_processes.first().cloned();
    ProcessSnapshot {
        llm_count,
        llm_rss,
        llm_cpu,
        top_llm,
        provider,
        largest_consumer,
        llm_processes,
    }
}

fn annotate_process_pagein_rates(
    processes: &mut [LlmProcess],
    previous: &[LlmProcess],
    elapsed: Duration,
) {
    let seconds = elapsed.as_secs_f64().max(0.001);
    for process in processes {
        process.pagein_rate = process.pageins.and_then(|current| {
            previous
                .iter()
                .find(|old| old.pid == process.pid)
                .and_then(|old| old.pageins)
                .map(|old| delta(current, old) as f64 / seconds)
        });
    }
}

fn is_llm_process(name: &str, command: &str) -> bool {
    LLM_PROCESS_MARKERS.iter().any(|marker| {
        let marker = normalize_process_token(marker);
        std::iter::once(name)
            .chain(command.split_whitespace().take(4))
            .any(|candidate| {
                let candidate = normalize_process_token(candidate);
                !candidate.contains("mlxtop") && candidate.contains(&marker)
            })
    })
}

fn normalize_process_token(value: &str) -> String {
    value
        .trim_matches(['"', '\''])
        .rsplit('/')
        .next()
        .unwrap_or(value)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn process_provider(name: &str, command: &str) -> Option<String> {
    let executable = name.to_ascii_lowercase();
    let provider = if executable.contains("omlx") || command.contains("omlx") {
        "oMLX"
    } else if executable.contains("ollama") || command.contains("ollama") {
        "Ollama"
    } else if executable.contains("llama") || command.contains("llama-server") {
        "llama.cpp"
    } else if executable.contains("lmstudio") || command.contains("lm studio") {
        "LM Studio"
    } else if executable.contains("kobold") || command.contains("koboldcpp") {
        "KoboldCpp"
    } else if executable.contains("localai") || command.contains("localai") {
        "LocalAI"
    } else if executable.contains("mlx") || command.contains("mlx-lm") {
        "mlx-lm"
    } else {
        return None;
    };
    Some(provider.into())
}

impl LlmTelemetryClient {
    fn new() -> Self {
        let (host, port) = read_omlx_endpoint();
        Self {
            host,
            port,
            session_cookie: None,
            cached: None,
            mlx_metadata: MlxTelemetry::default(),
            last_stats_available: None,
            next_metadata_poll: Instant::now(),
            next_poll: Instant::now(),
            retry_backoff: Duration::from_secs(1),
        }
    }

    fn poll(&mut self) -> Option<LlmTelemetry> {
        let now = Instant::now();
        if now < self.next_poll {
            return self.cached.clone();
        }
        let telemetry = self.poll_once();
        if let Some(telemetry) = telemetry {
            self.cached = Some(telemetry);
            self.retry_backoff = Duration::from_secs(1);
            self.next_poll = now + self.retry_backoff;
        } else {
            diagnostics_log(
                "WARN",
                "llm_api_poll_failed",
                format!(
                    "host={} port={} retry_seconds={}",
                    log_field(&self.host),
                    self.port,
                    self.retry_backoff.as_secs()
                ),
            );
            self.next_poll = now + self.retry_backoff;
            self.retry_backoff = (self.retry_backoff * 2).min(Duration::from_secs(30));
        }
        self.cached.clone()
    }

    fn poll_once(&mut self) -> Option<LlmTelemetry> {
        let health_response = match http_request(&self.host, self.port, "GET", "/health", &[], None)
        {
            Some(response) => response,
            None => {
                self.last_stats_available = None;
                diagnostics_log(
                    "WARN",
                    "llm_health_unreachable",
                    format!("host={} port={}", log_field(&self.host), self.port),
                );
                return None;
            }
        };
        if health_response.status != 200 {
            self.last_stats_available = None;
            diagnostics_log(
                "WARN",
                "llm_health_http_error",
                format!(
                    "host={} port={} status={}",
                    log_field(&self.host),
                    self.port,
                    health_response.status
                ),
            );
            return None;
        }
        let health: Value = match serde_json::from_str(&health_response.body) {
            Ok(health) => health,
            Err(error) => {
                self.last_stats_available = None;
                diagnostics_log(
                    "WARN",
                    "llm_health_invalid_json",
                    format!(
                        "host={} port={} error={}",
                        log_field(&self.host),
                        self.port,
                        log_field(&error.to_string())
                    ),
                );
                return None;
            }
        };
        if health.get("default_model").is_none() && health.get("engine_pool").is_none() {
            self.last_stats_available = None;
            diagnostics_log(
                "WARN",
                "llm_health_unrecognized",
                format!("host={} port={}", log_field(&self.host), self.port),
            );
            return None;
        }

        let stats = self.fetch_stats();
        let stats_available = stats.is_some();
        if self.last_stats_available != Some(stats_available) {
            diagnostics_log(
                if stats_available { "INFO" } else { "WARN" },
                "llm_api_stats",
                format!(
                    "host={} port={} available={stats_available}",
                    log_field(&self.host),
                    self.port
                ),
            );
            self.last_stats_available = Some(stats_available);
        }
        let now = Instant::now();
        if now >= self.next_metadata_poll {
            let device_info = self.fetch_json("/admin/api/device-info");
            let settings = self
                .fetch_json("/admin/api/global-settings")
                .or_else(|| self.fetch_json("/admin/api/settings"));
            let metadata = parse_mlx_metadata(device_info.as_ref(), settings.as_ref());
            let metadata_available = !mlx_metadata_is_empty(&metadata);
            self.mlx_metadata = merge_mlx_telemetry(&self.mlx_metadata, &metadata);
            self.next_metadata_poll = now
                + if metadata_available {
                    Duration::from_secs(60)
                } else {
                    Duration::from_secs(10)
                };
        }
        let mut telemetry = parse_omlx_telemetry(&health, stats.as_ref());
        telemetry.mlx = merge_mlx_telemetry(
            &self.mlx_metadata,
            &parse_mlx_runtime_telemetry(&health, stats.as_ref()),
        );
        telemetry.observed_at = Some(SystemTime::now());
        Some(telemetry)
    }

    fn fetch_stats(&mut self) -> Option<Value> {
        self.fetch_json("/admin/api/stats?scope=session")
    }

    fn fetch_json(&mut self, path: &str) -> Option<Value> {
        if self.session_cookie.is_none() {
            self.login();
        }
        let cookie = self.session_cookie.clone()?;
        let response = http_request(
            &self.host,
            self.port,
            "GET",
            path,
            &[("Cookie", cookie.as_str())],
            None,
        )?;
        if response.status == 401 {
            self.session_cookie = None;
            self.login();
            let cookie = self.session_cookie.clone()?;
            let response = http_request(
                &self.host,
                self.port,
                "GET",
                path,
                &[("Cookie", cookie.as_str())],
                None,
            )?;
            if response.status != 200 {
                return None;
            }
            return serde_json::from_str(&response.body).ok();
        }
        if response.status != 200 {
            return None;
        }
        serde_json::from_str(&response.body).ok()
    }

    fn login(&mut self) {
        if !is_loopback_host(&self.host)
            && env::var("MLXTOP_ALLOW_REMOTE_AUTH").as_deref() != Ok("1")
        {
            return;
        }
        let Some(api_key) = read_omlx_api_key() else {
            return;
        };
        let body = json!({ "api_key": api_key, "remember": true }).to_string();
        let Some(response) = http_request(
            &self.host,
            self.port,
            "POST",
            "/admin/api/login",
            &[("Content-Type", "application/json")],
            Some(&body),
        ) else {
            return;
        };
        if response.status == 200 {
            self.session_cookie = response
                .header("set-cookie")
                .and_then(|value| value.split(';').next())
                .map(str::to_owned);
        }
    }
}

struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

fn http_request(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Option<HttpResponse> {
    let address = (host, port).to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(250)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(250)))
        .ok()?;
    let body = body.unwrap_or("");
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n");
    for (key, value) in headers {
        request.push_str(key);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    if !body.is_empty() {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).ok()?;

    let mut raw = Vec::new();
    stream
        .take((MAX_HTTP_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut raw)
        .ok()?;
    if raw.len() > MAX_HTTP_RESPONSE_BYTES {
        return None;
    }
    let raw = String::from_utf8_lossy(&raw);
    let (head, body) = raw.split_once("\r\n\r\n")?;
    let mut lines = head.lines();
    let status = lines
        .next()?
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse().ok())?;
    let headers = lines
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((key.trim().to_owned(), value.trim().to_owned()))
        })
        .collect();
    Some(HttpResponse {
        status,
        headers,
        body: body.to_owned(),
    })
}

fn read_omlx_endpoint() -> (String, u16) {
    let mut host = DEFAULT_OMLX_HOST.to_owned();
    let mut port = DEFAULT_OMLX_PORT;
    if let Some(home) = env::var_os("HOME") {
        let path = Path::new(&home).join(".config/omlx-coding/server.env");
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let value = value.trim().trim_matches('"');
                match key.trim() {
                    "HOST" | "OMLX_HOST" if !value.is_empty() && value != "0.0.0.0" => {
                        host = value.to_owned()
                    }
                    "PORT" | "OMLX_PORT" => {
                        port = value.parse().unwrap_or(port);
                    }
                    _ => {}
                }
            }
        }
    }
    (host, port)
}

fn is_loopback_host(host: &str) -> bool {
    matches!(
        host.trim_matches(['[', ']']),
        "127.0.0.1" | "localhost" | "::1"
    )
}

fn read_omlx_api_key() -> Option<String> {
    let home = env::var_os("HOME")?;
    let path = Path::new(&home).join(".config/omlx-coding/server.env");
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|line| {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            let (key, value) = line.split_once('=')?;
            (key.trim() == "API_KEY").then(|| value.trim().trim_matches('"').to_owned())
        })
        .filter(|value| !value.is_empty())
}

fn parse_omlx_telemetry(health: &Value, stats: Option<&Value>) -> LlmTelemetry {
    let mut telemetry = LlmTelemetry {
        source: TelemetrySource::Live,
        provider: Some("oMLX".into()),
        status: Some(json_string(health, &["status"]).unwrap_or_else(|| "healthy".into())),
        model: json_string(health, &["default_model"]),
        model_memory: json_u64(health, &["engine_pool", "current_model_memory"]),
        model_memory_max: json_u64(health, &["engine_pool", "final_ceiling"]),
        mlx: parse_mlx_runtime_telemetry(health, stats),
        ..LlmTelemetry::default()
    };

    let Some(stats) = stats else {
        return telemetry;
    };
    telemetry.generation_tps =
        json_f64(stats, &["avg_generation_tps"]).filter(|value| *value >= 0.0);
    telemetry.prefill_tps = json_f64(stats, &["avg_prefill_tps"]).filter(|value| *value >= 0.0);
    telemetry.cache_efficiency =
        json_f64(stats, &["cache_efficiency"]).map(|value| value.clamp(0.0, 100.0));
    telemetry.total_prompt_tokens = json_u64(stats, &["total_prompt_tokens"]);
    telemetry.total_cached_tokens = json_u64(stats, &["total_cached_tokens"]);
    telemetry.model_memory =
        json_u64(stats, &["active_models", "model_memory_used"]).or(telemetry.model_memory);
    telemetry.model_memory_max =
        json_u64(stats, &["active_models", "model_memory_max"]).or(telemetry.model_memory_max);

    let models = stats
        .get("active_models")
        .and_then(|value| value.get("models"))
        .and_then(Value::as_array);
    let model = models.and_then(|models| {
        models
            .iter()
            .find(|model| {
                ["generating", "prefilling"].into_iter().any(|phase| {
                    model
                        .get(phase)
                        .and_then(Value::as_array)
                        .is_some_and(|requests| !requests.is_empty())
                })
            })
            .or_else(|| models.first())
    });
    if let Some(model) = model {
        telemetry.model = json_string(model, &["id"]).or(telemetry.model);
        telemetry.active_requests = json_u64(model, &["active_requests"]);
        telemetry.waiting_requests = json_u64(model, &["waiting_requests"]);
        if let Some(request) = model
            .get("generating")
            .and_then(Value::as_array)
            .and_then(|requests| requests.first())
        {
            if let Some(rate) = request_rate(request, &["tokens_per_second"]) {
                telemetry.generation_tps = Some(rate);
                telemetry.generation_tps_live = true;
            }
            telemetry.output_tokens = json_u64(request, &["generated_tokens"]);
            telemetry.prompt_tokens = json_u64(request, &["prompt_tokens"]);
        }
        if let Some(request) = model
            .get("prefilling")
            .and_then(Value::as_array)
            .and_then(|requests| requests.first())
        {
            // Recent oMLX admin responses expose prefill progress as
            // `speed`; older responses used `tokens_per_second`.
            if let Some(rate) = request_rate(
                request,
                &[
                    "tokens_per_second",
                    "speed",
                    "prefill_tps",
                    "prompt_tokens_per_second",
                ],
            ) {
                telemetry.prefill_tps = Some(rate);
                telemetry.prefill_tps_live = true;
            }
            telemetry.prompt_tokens = telemetry
                .prompt_tokens
                .or_else(|| json_u64(request, &["prompt_tokens"]));
        }
        if telemetry.prompt_tokens.is_none() {
            telemetry.prompt_tokens = model
                .get("waiting")
                .and_then(Value::as_array)
                .and_then(|requests| requests.first())
                .and_then(|request| json_u64(request, &["prompt_tokens"]));
        }
        let loading = model
            .get("is_loading")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let generating = model
            .get("generating")
            .and_then(Value::as_array)
            .is_some_and(|requests| !requests.is_empty());
        let prefilling = model
            .get("prefilling")
            .and_then(Value::as_array)
            .is_some_and(|requests| !requests.is_empty());
        let waiting = telemetry.waiting_requests.unwrap_or(0) > 0;
        telemetry.status = Some(
            if loading {
                "loading"
            } else if prefilling {
                "prefilling"
            } else if generating {
                "generating"
            } else if waiting {
                "waiting"
            } else if telemetry.active_requests.unwrap_or(0) > 0 {
                "active"
            } else {
                "idle"
            }
            .into(),
        );
    }

    if let Some(model_cache) = stats
        .get("runtime_cache")
        .and_then(|value| value.get("models"))
        .and_then(Value::as_array)
        .and_then(|models| models.first())
    {
        let cumulative = model_cache
            .get("cache_rates")
            .and_then(|value| value.get("cumulative"));
        telemetry.prefix_hit_rate = cumulative
            .and_then(|value| json_f64(value, &["prefix_hit_rate"]))
            .map(|value| (value * 100.0).clamp(0.0, 100.0));
    }
    telemetry
}

fn parse_mlx_runtime_telemetry(health: &Value, stats: Option<&Value>) -> MlxTelemetry {
    let mut sources = vec![health];
    if let Some(stats) = stats {
        sources.push(stats);
    }
    parse_mlx_sources(&sources)
}

fn parse_mlx_metadata(device_info: Option<&Value>, settings: Option<&Value>) -> MlxTelemetry {
    let mut sources = Vec::new();
    if let Some(device_info) = device_info {
        sources.push(device_info);
    }
    if let Some(settings) = settings {
        sources.push(settings);
    }
    parse_mlx_sources(&sources)
}

fn parse_mlx_sources(sources: &[&Value]) -> MlxTelemetry {
    let mut telemetry = MlxTelemetry::default();
    for source in sources {
        let active_memory = json_u64_paths_or_keys(
            source,
            &[
                &["mlx_memory", "active_bytes"],
                &["mlx", "active_bytes"],
                &["mlx", "active_memory"],
                &["active_memory_bytes"],
            ],
            &["mlx_active_memory_bytes"],
        );
        let cache_memory = json_u64_paths_or_keys(
            source,
            &[
                &["mlx_memory", "cache_bytes"],
                &["mlx", "cache_bytes"],
                &["mlx", "cache_memory"],
                &["cache_memory_bytes"],
            ],
            &["mlx_cache_memory_bytes"],
        );
        let peak_memory = json_u64_paths_or_keys(
            source,
            &[
                &["mlx_memory", "peak_bytes"],
                &["mlx", "peak_bytes"],
                &["mlx", "peak_memory"],
                &["peak_memory_bytes"],
            ],
            &["mlx_peak_memory_bytes"],
        );
        let process_footprint = json_u64_paths_or_keys(
            source,
            &[
                &["system", "omlx_phys_footprint_bytes"],
                &["system", "phys_footprint_bytes"],
            ],
            &["omlx_phys_footprint_bytes", "phys_footprint_bytes"],
        );
        let resource_limit = json_u64_paths_or_keys(
            source,
            &[
                &["system", "iogpu_wired_limit_bytes"],
                &["system", "metal_limit_bytes"],
                &["mlx", "resource_limit"],
            ],
            &[
                "iogpu_wired_limit_bytes",
                "metal_limit_bytes",
                "resource_limit",
            ],
        );
        let next = MlxTelemetry {
            version: json_string_paths_or_keys(
                source,
                &[
                    &["mlx_version"],
                    &["mlx", "version"],
                    &["engines", "mlx-lm", "version"],
                    &["engines", "mlx-vlm", "version"],
                    &["engines", "mlx-embeddings", "version"],
                    &["engines", "mlx-audio", "version"],
                ],
                &["mlx_version"],
            ),
            active_memory,
            cache_memory,
            peak_memory,
            device_name: json_string_paths_or_keys(
                source,
                &[
                    &["device_name"],
                    &["mlx_device_name"],
                    &["hardware", "device_name"],
                    &["chip_name"],
                ],
                &["device_name", "mlx_device_name", "chip_name"],
            ),
            architecture: json_string_paths_or_keys(
                source,
                &[&["architecture"], &["mlx", "architecture"]],
                &["architecture"],
            ),
            memory_size: json_u64_paths_or_keys(
                source,
                &[
                    &["memory_size"],
                    &["mlx", "memory_size"],
                    &["hardware", "memory_size"],
                    &["system", "total_memory_bytes"],
                ],
                &["memory_size", "total_memory_bytes"],
            )
            .or_else(|| json_f64_key(source, "memory_gb").and_then(gib_to_bytes)),
            recommended_working_set: json_u64_paths_or_keys(
                source,
                &[
                    &["max_recommended_working_set_size"],
                    &["mlx", "max_recommended_working_set_size"],
                    &["recommended_working_set_bytes"],
                ],
                &[
                    "max_recommended_working_set_size",
                    "recommended_working_set_bytes",
                ],
            ),
            max_buffer_size: json_u64_paths_or_keys(
                source,
                &[
                    &["max_buffer_size"],
                    &["mlx", "max_buffer_size"],
                    &["max_buffer_length"],
                ],
                &["max_buffer_size", "max_buffer_length"],
            ),
            resource_limit,
            process_footprint,
        };
        telemetry = merge_mlx_telemetry(&telemetry, &next);
    }
    telemetry
}

fn mlx_metadata_is_empty(telemetry: &MlxTelemetry) -> bool {
    telemetry.version.is_none()
        && telemetry.active_memory.is_none()
        && telemetry.cache_memory.is_none()
        && telemetry.peak_memory.is_none()
        && telemetry.device_name.is_none()
        && telemetry.architecture.is_none()
        && telemetry.memory_size.is_none()
        && telemetry.recommended_working_set.is_none()
        && telemetry.max_buffer_size.is_none()
        && telemetry.resource_limit.is_none()
        && telemetry.process_footprint.is_none()
}

fn merge_mlx_telemetry(base: &MlxTelemetry, update: &MlxTelemetry) -> MlxTelemetry {
    MlxTelemetry {
        version: update.version.clone().or_else(|| base.version.clone()),
        active_memory: update.active_memory.or(base.active_memory),
        cache_memory: update.cache_memory.or(base.cache_memory),
        peak_memory: update.peak_memory.or(base.peak_memory),
        device_name: update
            .device_name
            .clone()
            .or_else(|| base.device_name.clone()),
        architecture: update
            .architecture
            .clone()
            .or_else(|| base.architecture.clone()),
        memory_size: update.memory_size.or(base.memory_size),
        recommended_working_set: update
            .recommended_working_set
            .or(base.recommended_working_set),
        max_buffer_size: update.max_buffer_size.or(base.max_buffer_size),
        resource_limit: update.resource_limit.or(base.resource_limit),
        process_footprint: update.process_footprint.or(base.process_footprint),
    }
}

fn json_u64_paths_or_keys(value: &Value, paths: &[&[&str]], keys: &[&str]) -> Option<u64> {
    paths
        .iter()
        .find_map(|path| json_u64(value, path))
        .or_else(|| keys.iter().find_map(|key| json_u64_key(value, key)))
}

fn json_string_paths_or_keys(value: &Value, paths: &[&[&str]], keys: &[&str]) -> Option<String> {
    paths
        .iter()
        .find_map(|path| json_string(value, path))
        .or_else(|| keys.iter().find_map(|key| json_string_key(value, key)))
}

fn json_u64_key(value: &Value, key: &str) -> Option<u64> {
    match value {
        Value::Object(object) => object
            .get(key)
            .and_then(value_as_u64)
            .or_else(|| object.values().find_map(|child| json_u64_key(child, key))),
        Value::Array(values) => values.iter().find_map(|child| json_u64_key(child, key)),
        _ => None,
    }
}

fn json_string_key(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(object) => object
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                object
                    .values()
                    .find_map(|child| json_string_key(child, key))
            }),
        Value::Array(values) => values.iter().find_map(|child| json_string_key(child, key)),
        _ => None,
    }
}

fn json_f64_key(value: &Value, key: &str) -> Option<f64> {
    match value {
        Value::Object(object) => object
            .get(key)
            .and_then(value_as_f64)
            .or_else(|| object.values().find_map(|child| json_f64_key(child, key))),
        Value::Array(values) => values.iter().find_map(|child| json_f64_key(child, key)),
        _ => None,
    }
}

fn value_as_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_u64().map(|value| value as f64))
}

fn gib_to_bytes(value: f64) -> Option<u64> {
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let bytes = value * 1024_f64.powi(3);
    (bytes.is_finite() && bytes <= u64::MAX as f64).then_some(bytes.round() as u64)
}

fn value_as_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| value as u64)
    })
}

fn json_string(value: &Value, path: &[&str]) -> Option<String> {
    json_value(value, path)?.as_str().map(str::to_owned)
}

fn json_u64(value: &Value, path: &[&str]) -> Option<u64> {
    let value = json_value(value, path)?;
    value_as_u64(value)
}

fn json_f64(value: &Value, path: &[&str]) -> Option<f64> {
    let value = json_value(value, path)?;
    value
        .as_f64()
        .or_else(|| value.as_u64().map(|value| value as f64))
}

fn request_rate(request: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| json_f64(request, &[*key]))
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn json_value<'a>(mut value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    for key in path {
        value = value.get(*key)?;
    }
    Some(value)
}

fn read_llm_stats() -> LlmLogStats {
    let Some(home) = env::var_os("HOME") else {
        return LlmLogStats::default();
    };
    let candidates = [
        Path::new(&home).join(".omlx-coding/logs/server.log"),
        Path::new(&home).join(".omlx-coding/logs/launchd.stdout.log"),
        Path::new(&home).join(".omlx-coding/logs/launchd.stderr.log"),
    ];
    candidates
        .into_iter()
        .filter_map(|path| read_latest_completion(&path))
        .max_by(|left, right| match (left.observed_at, right.observed_at) {
            (Some(left), Some(right)) => left.cmp(&right),
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => std::cmp::Ordering::Equal,
        })
        .unwrap_or_default()
}

fn read_latest_completion(path: &Path) -> Option<LlmLogStats> {
    let mut file = File::open(path).ok()?;
    let observed_at = file
        .metadata()
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(256 * 1024);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    let mut stats = text
        .lines()
        .filter_map(parse_llm_completion_line)
        .next_back()?;
    stats.observed_at = observed_at;
    Some(stats)
}

fn parse_llm_completion_line(line: &str) -> Option<LlmLogStats> {
    let (marker, marker_start) = if let Some(start) = line.find("Responses API: model=") {
        ("Responses API: model=", start)
    } else {
        let start = line.find("Chat completion: model=")?;
        ("Chat completion: model=", start)
    };
    let start = marker_start + marker.len();
    let (model, rest) = line[start..].split_once(", ")?;
    let (output, rest) = rest.split_once(" tokens in ")?;
    let output_tokens = output.trim().parse().ok()?;
    let (seconds, rest) = rest.split_once("s (")?;
    seconds.trim().parse::<f64>().ok()?;
    let (throughput, rest) = rest.split_once(" tok/s)")?;
    let tokens_per_second = throughput.trim().parse().ok()?;
    let prompt_tokens = rest.find("prompt: ").and_then(|prompt_start| {
        rest[prompt_start + "prompt: ".len()..]
            .split(',')
            .next()?
            .trim()
            .parse()
            .ok()
    });
    Some(LlmLogStats {
        model: Some(model.trim().into()),
        tokens_per_second: Some(tokens_per_second),
        output_tokens: Some(output_tokens),
        prompt_tokens,
        observed_at: None,
    })
}

fn find_number(text: &str, needle: &str) -> Option<u64> {
    let start = text.find(needle)? + needle.len();
    let start = text[start..].find('=')? + start + 1;
    let digits: String = text[start..]
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn find_named_number(text: &str, key: &str) -> Option<u64> {
    find_named_numbers(text, key).into_iter().next()
}

fn find_named_numbers(text: &str, key: &str) -> Vec<u64> {
    let needle = format!("\"{key}\"");
    let mut values = Vec::new();
    let mut offset = 0;
    while let Some(relative) = text[offset..].find(&needle) {
        let start = offset + relative + needle.len();
        let rest = text[start..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            offset = start;
            continue;
        };
        let digits: String = rest
            .chars()
            .skip_while(|character| character.is_whitespace())
            .take_while(|character| character.is_ascii_digit())
            .collect();
        if let Ok(value) = digits.parse() {
            values.push(value);
        }
        offset = start;
    }
    values
}

fn find_named_string(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = text.find(&needle)? + needle.len();
    let rest = text[start..].trim_start().strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    let value = rest[..end].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn command_text(program: &str, args: &[&str]) -> Option<String> {
    let command = || {
        format!(
            "program={} args={}",
            log_field(program),
            args.iter()
                .map(|arg| log_field(arg))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let mut child = match Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            diagnostics_log(
                "WARN",
                "command_spawn_failed",
                format!("{} error={}", command(), log_field(&error.to_string())),
            );
            return None;
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        diagnostics_log("WARN", "command_stdout_unavailable", command());
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let command_context = command();
    let reader = thread::spawn(move || {
        let mut output = String::new();
        match stdout.read_to_string(&mut output) {
            Ok(_) => Some(output),
            Err(error) => {
                diagnostics_log(
                    "WARN",
                    "command_read_failed",
                    format!(
                        "{} error={}",
                        command_context,
                        log_field(&error.to_string())
                    ),
                );
                None
            }
        }
    });
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = match reader.join() {
                    Ok(Some(output)) => output,
                    Ok(None) => return None,
                    Err(_) => {
                        diagnostics_log("WARN", "command_reader_panicked", command());
                        return None;
                    }
                };
                if !status.success() {
                    diagnostics_log(
                        "WARN",
                        "command_nonzero_exit",
                        format!("{} code={:?}", command(), status.code()),
                    );
                    return None;
                }
                return Some(output);
            }
            Err(error) => {
                diagnostics_log(
                    "WARN",
                    "command_wait_failed",
                    format!("{} error={}", command(), log_field(&error.to_string())),
                );
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
            Ok(None) if Instant::now() >= deadline => {
                diagnostics_log("WARN", "command_timeout", command());
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn command_u64(program: &str, args: &[&str]) -> Option<u64> {
    command_text(program, args)?.trim().parse().ok()
}

fn delta(current: u64, previous: u64) -> u64 {
    current.saturating_sub(previous)
}

fn rate_bytes(delta_units: u64, unit_bytes: u64, elapsed: Duration) -> u64 {
    let seconds = elapsed.as_secs_f64().max(0.001);
    let value = (delta_units as f64 * unit_bytes as f64 / seconds).round();
    if value.is_finite() && value > 0.0 {
        value.min(u64::MAX as f64) as u64
    } else {
        0
    }
}

fn signed_rate_bytes(current: u64, previous: u64, elapsed: Duration) -> i64 {
    let seconds = elapsed.as_secs_f64().max(0.001);
    let delta = current as f64 - previous as f64;
    let value = (delta / seconds).round();
    if !value.is_finite() {
        0
    } else if value > i64::MAX as f64 {
        i64::MAX
    } else if value < i64::MIN as f64 {
        i64::MIN
    } else {
        value as i64
    }
}

fn push_history(
    history: &mut VecDeque<ChartPoint>,
    value: Option<u64>,
    metric: ChartMetric,
    limit: usize,
) {
    history.push_back(ChartPoint {
        tone: value.map(|value| metric.tone(value)).unwrap_or(Tone::Muted),
        value,
    });
    while history.len() > limit {
        history.pop_front();
    }
}

fn parse_unit(value: &str) -> u64 {
    let trimmed = value.trim_matches(',');
    let split = trimmed
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(trimmed.len());
    let number = trimmed[..split].parse::<f64>().unwrap_or(0.0);
    let unit = trimmed[split..].to_ascii_uppercase();
    let multiplier = match unit.as_str() {
        "K" | "KB" => 1024.0,
        "M" | "MB" => 1024.0_f64.powi(2),
        "G" | "GB" => 1024.0_f64.powi(3),
        "T" | "TB" => 1024.0_f64.powi(4),
        _ => 1.0,
    };
    (number * multiplier) as u64
}

fn bytes(value: u64) -> String {
    if value >= 1024_u64.pow(3) {
        format!("{:.1} GiB", value as f64 / 1024_f64.powi(3))
    } else if value >= 1024_u64.pow(2) {
        format!("{:.1} MiB", value as f64 / 1024_f64.powi(2))
    } else if value >= 1024 {
        format!("{:.1} KiB", value as f64 / 1024.0)
    } else {
        format!("{value} B")
    }
}

fn optional_bytes(value: Option<u64>) -> String {
    value.map(bytes).unwrap_or_else(|| "—".into())
}

fn compressed_memory_label(sample: &Sample) -> String {
    if !sample.vm_available {
        return "—".into();
    }
    if sample.compressor == 0 || sample.compressed_logical == 0 {
        return bytes(sample.compressor);
    }
    format!(
        "{} · {:.1}×",
        bytes(sample.compressor),
        sample.compressed_logical as f64 / sample.compressor as f64
    )
}

fn rate(value: u64) -> String {
    format!("{}/s", bytes(value))
}

fn signed_rate(value: i64) -> String {
    if value > 0 {
        format!("+{}", rate(value as u64))
    } else if value < 0 {
        format!("-{}", rate(value.unsigned_abs()))
    } else {
        "0 B/s".into()
    }
}

fn now_clock() -> String {
    if let Some(value) = command_text("/bin/date", &["+%H:%M:%S"]) {
        return value.trim().to_string();
    }
    "??:??:??".into()
}

fn tone_badge(tone: Tone, label: &str) -> Span<'static> {
    Span::styled(
        format!(" {label} "),
        Style::default()
            .fg(Color::Black)
            .bg(tone.color())
            .add_modifier(Modifier::BOLD),
    )
}

fn card_block<'a>(title: Line<'a>, tone: Tone) -> Block<'a> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(EDGE))
        .style(Style::default().bg(PANEL_RAISED).fg(tone.color()))
}

fn render_card<'a>(
    frame: &mut Frame,
    area: Rect,
    title: Line<'a>,
    lines: Vec<Line<'a>>,
    tone: Tone,
) {
    let block = card_block(title, tone);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if !inner.is_empty() {
        frame.render_widget(
            Paragraph::new(lines)
                .style(Style::default().bg(PANEL_RAISED))
                .wrap(Wrap { trim: true }),
            inner,
        );
    }
}

fn render_metric_card<'a>(
    frame: &mut Frame,
    area: Rect,
    title: Line<'a>,
    headline: Line<'a>,
    gauge: Option<(u16, String, Tone)>,
    details: Vec<Line<'a>>,
    tone: Tone,
) {
    let block = card_block(title, tone);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(headline)
            .style(Style::default().bg(PANEL_RAISED))
            .wrap(Wrap { trim: true }),
        rows[0],
    );
    if let Some((value, label, gauge_tone)) = gauge {
        frame.render_widget(
            Gauge::default()
                .gauge_style(Style::default().fg(gauge_tone.color()).bg(DIM))
                .label(label)
                .percent(value.min(100)),
            rows[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new("—")
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED).bg(PANEL_RAISED)),
            rows[1],
        );
    }
    frame.render_widget(
        Paragraph::new(details)
            .style(Style::default().bg(PANEL_RAISED))
            .wrap(Wrap { trim: true }),
        rows[2],
    );
}

fn llm_status_tone(status: &str) -> Tone {
    match status.to_ascii_lowercase().as_str() {
        "ready" | "idle" => Tone::Green,
        "waiting" | "busy" | "generating" | "stale" | "last result" => Tone::Yellow,
        "offline" | "error" => Tone::Red,
        _ => Tone::Cyan,
    }
}

fn panel(title: &str, tone: Tone) -> Block<'static> {
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

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
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

fn print_static(sample: &Sample, interval: u64) {
    println!("mlxtop · static report ({interval}s sample)\n");
    println!("SIGNAL       {} · {}", sample.impact, sample.grade);
    println!("PRESSURE     {}", pressure_state_label(sample));
    println!(
        "MEMORY       {} free · {} total",
        sample
            .availability
            .map(|v| format!("{v}%"))
            .unwrap_or_else(|| "—".into()),
        bytes(sample.total_memory)
    );
    if sample.swap_available {
        let used_percent = if sample.swap_total > 0 {
            sample
                .swap_used
                .saturating_mul(100)
                .checked_div(sample.swap_total)
                .unwrap_or(0)
        } else {
            0
        };
        println!(
            "PAGING       {} / {} · {}% used · in {} · out {}",
            bytes(sample.swap_used),
            bytes(sample.swap_total),
            used_percent,
            rate(sample.swap_in),
            rate(sample.swap_out)
        );
    } else {
        println!("PAGING       —");
    }
    println!(
        "METAL        {} · {} cores · GPU {} · renderer {} · tiler {}",
        sample.metal.device_name.as_deref().unwrap_or("unavailable"),
        sample
            .metal
            .gpu_cores
            .map(|value| value.to_string())
            .unwrap_or_else(|| "—".into()),
        sample
            .gpu_util
            .map(|v| format!("{v}%"))
            .unwrap_or_else(|| "—".into()),
        percent_u8(sample.metal.renderer_util),
        percent_u8(sample.metal.tiler_util)
    );
    println!(
        "RUNTIME      thermal {} · LLM {} · footprint {} · Metal limit {}",
        sample.thermal,
        sample.llm_count,
        optional_bytes(sample.mlx.process_footprint),
        optional_bytes(sample.metal.resource_limit)
    );
    println!(
        "LLM          {} · {} · {} · {}",
        sample.llm_provider,
        sample.llm_status,
        telemetry_source(sample),
        llm_model_label(sample, 40)
    );
    println!(
        "SERVING      {} · {} · active {} · cache {}",
        llm_generation_rate_label(sample),
        llm_prefill_rate_label(sample),
        sample
            .llm_active_requests
            .map(|value| value.to_string())
            .unwrap_or_else(|| "—".into()),
        percent(sample.llm_cache_efficiency)
    );
    println!(
        "MLX          version {} · active {} · cache {} · peak {}",
        sample.mlx.version.as_deref().unwrap_or("—"),
        optional_bytes(sample.mlx.active_memory),
        optional_bytes(sample.mlx.cache_memory),
        optional_bytes(sample.mlx.peak_memory)
    );
    if !sample.correlation.summary.is_empty() {
        println!("CORRELATION   {}", sample.correlation.summary);
        println!(
            "EVIDENCE      {} · {} confidence",
            sample.correlation.details,
            sample.correlation.confidence_label()
        );
    }
    println!(
        "NEXT         {} — {}",
        sample.guidance_badge, sample.guidance_action
    );
}

struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn new() -> Self {
        Self { active: true }
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            let mut out = stdout();
            let _ = execute!(out, crossterm::cursor::Show, LeaveAlternateScreen);
        }
    }
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<(), Box<dyn std::error::Error>> {
    diagnostics_log("INFO", "tui_start", "interactive_session_started");
    loop {
        if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| app.tick())) {
            diagnostics_log(
                "ERROR",
                "tui_tick_panic",
                format!(
                    "tab={} status={} message={}",
                    app.tab,
                    log_field(&app.collector.current.llm_status),
                    log_field(&panic_payload(payload.as_ref()))
                ),
            );
            panic::resume_unwind(payload);
        }
        let terminal_size = terminal
            .size()
            .ok()
            .map(|size| format!("{}x{}", size.width, size.height))
            .unwrap_or_else(|| "unknown".into());
        let app_for_draw = &mut *app;
        let draw_result = panic::catch_unwind(AssertUnwindSafe(|| {
            terminal.draw(move |frame| app_for_draw.draw(frame))
        }));
        let draw_result = match draw_result {
            Ok(result) => result,
            Err(payload) => {
                diagnostics_log(
                    "ERROR",
                    "tui_draw_panic",
                    format!(
                        "tab={} terminal={} status={} message={}",
                        app.tab,
                        terminal_size,
                        log_field(&app.collector.current.llm_status),
                        log_field(&panic_payload(payload.as_ref()))
                    ),
                );
                panic::resume_unwind(payload);
            }
        };
        if let Err(error) = draw_result {
            diagnostics_log(
                "ERROR",
                "terminal_draw_error",
                format!("error={}", log_field(&error.to_string())),
            );
            return Err(error.into());
        }
        if app.quit {
            break;
        }
        let input_ready = match event::poll(Duration::from_millis(100)) {
            Ok(input_ready) => input_ready,
            Err(error) => {
                diagnostics_log(
                    "ERROR",
                    "input_poll_error",
                    format!("error={}", log_field(&error.to_string())),
                );
                return Err(error.into());
            }
        };
        if input_ready {
            match event::read() {
                Ok(Event::Key(key)) => app.handle_key(key),
                Ok(_) => {}
                Err(error) => {
                    diagnostics_log(
                        "ERROR",
                        "input_read_error",
                        format!("error={}", log_field(&error.to_string())),
                    );
                    return Err(error.into());
                }
            }
        }
    }
    diagnostics_log("INFO", "tui_stop", "interactive_session_stopped");
    Ok(())
}

fn main() {
    let _ = init_diagnostics();
    install_panic_hook();
    match run() {
        Ok(()) => diagnostics_log("INFO", "process_exit", "code=0"),
        Err(error) => {
            diagnostics_log(
                "ERROR",
                "process_error",
                format!("error={}", log_field(&error.to_string())),
            );
            eprintln!("mlxtop: {error}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut interval = 1_u64;
    let mut history = 300_usize;
    let mut once = false;
    let args: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-i" | "--interval" => {
                i += 1;
                interval = args.get(i).ok_or("missing interval")?.parse()?;
            }
            "-n" | "--history" => {
                i += 1;
                history = args.get(i).ok_or("missing history")?.parse()?;
            }
            "-1" | "--once" => once = true,
            "-V" | "--version" => {
                println!("mlxtop {VERSION}");
                return Ok(());
            }
            "-h" | "--help" => {
                println!(
                    "Usage: mlxtop [refresh-seconds] [options]\n\n\
                     Options: -i, --interval N  refresh interval (default 1)\n\
                     -n, --history N    chart/journal history (20–3600)\n\
                     -1, --once         static report\n\
                     -V, --version      show version\n\
                     -h, --help         show help\n\
                     Diagnostics: ~/Library/Logs/mlxtop/mlxtop.log (override with MLXTOP_LOG_PATH)\n\n\
                     Interactive keys: q quit · 1 overview · 2 top · 3 journal · tab views · +/- interval · ? help"
                );
                return Ok(());
            }
            value if !value.starts_with('-') && i == 0 => interval = value.parse()?,
            value => return Err(format!("unknown option: {value}").into()),
        }
        i += 1;
    }
    if !(1..=60).contains(&interval) {
        return Err("interval must be between 1 and 60 seconds".into());
    }
    if !(20..=3600).contains(&history) {
        return Err("history must be between 20 and 3600".into());
    }

    diagnostics_log(
        "INFO",
        "configuration",
        format!(
            "interval_seconds={interval} history_limit={history} once={once} interactive={}",
            io::stdin().is_terminal() && io::stdout().is_terminal()
        ),
    );

    if once {
        let mut collector = Collector::new(history);
        collector.sample();
        thread::sleep(Duration::from_secs(interval));
        let sample = collector.sample();
        print_static(&sample, interval);
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("run interactively, or use --once for a static report".into());
    }

    enable_raw_mode()?;
    let mut terminal_guard = TerminalGuard::new();
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new(interval, history);
    let result = run_app(&mut terminal, &mut app);
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        crossterm::cursor::Show,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    terminal_guard.disarm();
    drop(app);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app(tab: usize) -> App {
        let (app, _views) = test_app_with_sender(tab);
        app
    }

    fn test_app_with_sender(tab: usize) -> (App, Sender<CollectorView>) {
        let (commands, _command_receiver) = mpsc::channel();
        let (view_sender, views) = mpsc::channel();
        let app = App {
            collector: CollectorView {
                current: Sample {
                    total_memory: 1,
                    ..Sample::default()
                },
                generation_history: VecDeque::new(),
                prefill_history: VecDeque::new(),
                cache_history: VecDeque::new(),
                load_history: VecDeque::new(),
                swap_history: VecDeque::new(),
                gpu_history: VecDeque::new(),
                signals: VecDeque::new(),
            },
            sampler: Sampler {
                commands,
                views,
                handle: None,
            },
            interval: Duration::from_secs(1),
            paused: false,
            tab,
            top_sort: TopSort::Rss,
            top_filter: String::new(),
            top_filtering: false,
            top_selected: 0,
            journal_filter: JournalFilter::All,
            journal_scroll: 0,
            help: false,
            quit: false,
            sampler_disconnected: false,
            alert: None,
            alert_bells: 0,
        };
        (app, view_sender)
    }

    fn view_with_impact(impact: &str, updated: &str) -> CollectorView {
        CollectorView {
            current: Sample {
                impact: impact.into(),
                updated: updated.into(),
                ..Sample::default()
            },
            generation_history: VecDeque::new(),
            prefill_history: VecDeque::new(),
            cache_history: VecDeque::new(),
            load_history: VecDeque::new(),
            swap_history: VecDeque::new(),
            gpu_history: VecDeque::new(),
            signals: VecDeque::new(),
        }
    }

    fn render_app(app: &App, width: u16, height: u16) -> String {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        terminal
            .draw(|frame| app.draw(frame))
            .expect("application should render");
        let mut rendered = String::new();
        for y in 0..height {
            for x in 0..width {
                rendered.push_str(
                    terminal
                        .backend()
                        .buffer()
                        .cell((x, y))
                        .expect("rendered cell should exist")
                        .symbol(),
                );
            }
            rendered.push('\n');
        }
        rendered
    }

    #[test]
    fn parses_macos_swapusage_with_spaced_equals() {
        let (total, used) = parse_swap_usage("total = 8.00G used = 1.50G free = 6.50G");
        assert_eq!(total, 8 * 1024_u64.pow(3));
        assert_eq!(used, 1_536 * 1024_u64.pow(2));
    }

    #[test]
    fn parses_vm_stat_punctuation_and_rates() {
        let counters = parse_vm_stat(
            "Pages wired down: 10.\nPages occupied by compressor: 2.\n\
             Pages stored in compressor: 4.\nSwapins: 7.\nSwapouts: 3.",
            16_384,
        );
        assert_eq!(counters.wired, 10 * 16_384);
        assert_eq!(counters.compressor, 2 * 16_384);
        assert_eq!(counters.compressed_logical, 4 * 16_384);
        assert_eq!(counters.swapins, 7);
        assert_eq!(counters.swapouts, 3);
    }

    #[test]
    fn compression_summary_keeps_footprint_and_ratio_visible() {
        let sample = Sample {
            vm_available: true,
            compressor: 512 * 1024 * 1024,
            compressed_logical: 4 * 1024 * 1024 * 1024,
            ..Sample::default()
        };
        assert_eq!(compressed_memory_label(&sample), "512.0 MiB · 8.0×");
    }

    #[test]
    fn chart_colors_follow_load_thresholds() {
        assert_eq!(ChartMetric::Memory.tone(69), Tone::Green);
        assert_eq!(ChartMetric::Memory.tone(70), Tone::Yellow);
        assert_eq!(ChartMetric::Memory.tone(85), Tone::Red);
        assert_eq!(ChartMetric::Swap.tone(0), Tone::Green);
        assert_eq!(ChartMetric::Swap.tone(1024 * 1024), Tone::Yellow);
        assert_eq!(ChartMetric::Swap.tone(16 * 1024 * 1024), Tone::Red);
    }

    #[test]
    fn user_facing_load_labels_describe_conditions_not_colors() {
        let sample = Sample {
            pressure: "GREEN".into(),
            pressure_meaning: "normal".into(),
            ..Sample::default()
        };
        assert_eq!(pressure_state_label(&sample), "normal");

        let sample = Sample {
            pressure: "YELLOW".into(),
            pressure_meaning: "warning".into(),
            ..Sample::default()
        };
        assert_eq!(pressure_state_label(&sample), "watch");

        let sample = Sample {
            pressure: "RED".into(),
            pressure_meaning: "critical".into(),
            ..Sample::default()
        };
        assert_eq!(pressure_state_label(&sample), "critical");
        assert_eq!(gpu_load_label(Some(40)), "within target");
        assert_eq!(gpu_load_label(Some(80)), "loaded");
        assert_eq!(gpu_load_label(Some(95)), "saturated");
        assert_eq!(gpu_load_label(None), "unavailable");
    }

    #[test]
    fn chart_stats_use_normalized_values_and_keep_the_window_label_honest() {
        let history = VecDeque::from([
            ChartPoint::new(Some(0), Tone::Green),
            ChartPoint::new(Some(16 * 1024 * 1024), Tone::Red),
            ChartPoint::new(None, Tone::Muted),
        ]);
        // Paging stats stay in raw bytes/s so they match the live label.
        assert_eq!(
            chart_stats(&history, ChartMetric::Swap),
            (Some(8 * 1024 * 1024), Some(16 * 1024 * 1024))
        );
        assert_eq!(
            chart_stat_label(ChartMetric::Swap, Some(57 * 1024 + 640)),
            "57.6 KiB/s"
        );
        assert_eq!(
            chart_window_label(history.len(), Duration::from_secs(60)),
            "3m"
        );
    }

    #[test]
    fn overview_renders_the_first_glance_cards_at_reference_size() {
        let rendered = render_app(&test_app(0), 180, 50);
        for label in [
            "MODEL / STATE",
            "THROUGHPUT",
            "DIAGNOSIS",
            "MEMORY",
            "PAGING / I/O",
            "GPU / COMPUTE",
            "CACHE / QUEUE",
            "generation",
            "prefill",
            "cache",
            "avg",
            "pause",
        ] {
            assert!(rendered.contains(label), "missing rendered label: {label}");
        }
        assert!(!rendered.contains("/ local LLM performance"));
        assert!(!rendered.contains("LLM READY  HEALTHY"));
    }

    #[test]
    fn aggressive_paging_impact_states_drive_the_alert() {
        assert!(is_aggressive_paging("SWAP THRASHING"));
        assert!(is_aggressive_paging("HEAVY PAGING"));
        assert!(is_aggressive_paging("PAGE-IN RECOVERY"));
        assert!(!is_aggressive_paging("PAGING ACTIVE"));
        assert!(!is_aggressive_paging("WATCH PAGING"));
        assert!(!is_aggressive_paging("MEMORY BOTTLENECK"));
        assert!(!is_aggressive_paging("LLM READY"));
        assert!(!is_aggressive_paging(""));
    }

    #[test]
    fn paging_alert_rings_once_per_episode_and_rearms() {
        let (mut app, views) = test_app_with_sender(0);
        app.collector.current.impact = "LLM READY".into();

        views
            .send(view_with_impact("HEAVY PAGING", "00:52:54"))
            .expect("test channel should accept views");
        app.tick();
        let alert = app.alert.as_ref().expect("alert should raise");
        assert_eq!(alert.state, "HEAVY PAGING");
        assert_eq!(alert.summary, "swap 0 B/s / growth 0 B/s");
        assert_eq!(alert.time, "00:52:54");
        assert_eq!(app.alert_bells, 1);

        // Staying aggressive must not re-ring; escalation refreshes the
        // banner without raising a second alert.
        views
            .send(view_with_impact("SWAP THRASHING", "00:52:55"))
            .expect("test channel should accept views");
        app.tick();
        let alert = app.alert.as_ref().expect("alert persists while paging");
        assert_eq!(alert.state, "SWAP THRASHING");
        assert_eq!(alert.time, "00:52:55");
        assert_eq!(app.alert_bells, 1);

        // Recovery retires the alert and re-arms it for the next episode.
        views
            .send(view_with_impact("LLM READY", "00:52:56"))
            .expect("test channel should accept views");
        app.tick();
        assert!(app.alert.is_none());
        views
            .send(view_with_impact("HEAVY PAGING", "00:53:10"))
            .expect("test channel should accept views");
        app.tick();
        assert_eq!(app.alert_bells, 2);
        assert_eq!(app.alert.as_ref().expect("rearmed").time, "00:53:10");
    }

    #[test]
    fn paging_alert_banner_shows_until_acknowledged_or_reset() {
        let (mut app, views) = test_app_with_sender(0);
        app.collector.current.impact = "LLM READY".into();
        views
            .send(view_with_impact("HEAVY PAGING", "00:52:54"))
            .expect("test channel should accept views");
        app.tick();
        assert!(app.alert.is_some());

        let rendered = render_app(&app, 180, 50);
        assert!(rendered.contains("⚠ HEAVY PAGING"));
        assert!(rendered.contains("a acknowledge"));

        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(app.alert.is_none());
        let rendered = render_app(&app, 180, 50);
        assert!(!rendered.contains("⚠ HEAVY PAGING"));

        // A fresh episode after recovery raises again; reset retires it.
        views
            .send(view_with_impact("LLM READY", "00:52:55"))
            .expect("test channel should accept views");
        app.tick();
        views
            .send(view_with_impact("SWAP THRASHING", "00:52:56"))
            .expect("test channel should accept views");
        app.tick();
        assert_eq!(app.alert_bells, 2);
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(app.alert.is_none());
        let rendered = render_app(&app, 180, 50);
        assert!(!rendered.contains("⚠ SWAP THRASHING"));
    }

    #[test]
    fn paging_alert_survives_a_quiet_sample_while_paging_persists() {
        // A momentary 0 B/s sample inside an aggressive episode is normal
        // churn noise; the episode ends only when the classifier leaves the
        // aggressive states, so the alert must not flap on a single dip.
        let (mut app, views) = test_app_with_sender(0);
        app.collector.current.impact = "WATCH PAGING".into();
        views
            .send(view_with_impact("PAGE-IN RECOVERY", "00:52:54"))
            .expect("test channel should accept views");
        app.tick();
        assert!(app.alert.is_some());
        assert_eq!(app.alert_bells, 1);
    }

    #[test]
    fn overview_uses_semantic_state_labels_instead_of_color_names() {
        let mut app = test_app(0);
        app.collector.current.pressure = "GREEN".into();
        app.collector.current.pressure_meaning = "normal".into();
        app.collector.current.pressure_tone = Tone::Green;
        app.collector.current.availability = Some(50);
        let rendered = render_app(&app, 180, 50);

        assert!(rendered.contains("PRESSURE normal"));
        assert!(!rendered.contains("GREEN"));
        assert!(!rendered.contains("YELLOW"));
        assert!(!rendered.contains("RED"));
    }

    #[test]
    fn overview_renders_stepped_traces_instead_of_braille_points() {
        let mut app = test_app(0);
        app.collector.load_history = VecDeque::from([
            ChartPoint::new(Some(10), Tone::Green),
            ChartPoint::new(Some(100), Tone::Red),
        ]);
        let rendered = render_app(&app, 180, 50);

        assert!(rendered.contains('━'));
        assert!(!rendered.contains('⠁'));
    }

    #[test]
    fn recent_journal_keeps_two_event_rows_after_header_compaction() {
        let mut app = test_app(0);
        app.collector.signals = VecDeque::from([
            SignalEvent {
                time: "12:00:00".into(),
                recorded_at: SystemTime::now(),
                kind: EventKind::Llm,
                state: "LLM".into(),
                summary: "request started".into(),
                tone: Tone::Cyan,
            },
            SignalEvent {
                time: "12:00:01".into(),
                recorded_at: SystemTime::now(),
                kind: EventKind::Gpu,
                state: "GPU".into(),
                summary: "compute burst started".into(),
                tone: Tone::Yellow,
            },
        ]);
        let rendered = render_app(&app, 180, 50);
        assert!(rendered.contains("request started"));
        assert!(rendered.contains("compute burst started"));
        assert!(!rendered.contains("Live · sampling system counters"));
    }

    #[test]
    fn overview_uses_a_compact_operational_strip_on_medium_terminals() {
        let rendered = render_app(&test_app(0), 100, 40);
        for label in [
            "LLM OPERATIONS",
            "STATUS",
            "RATE",
            "DIAG",
            "generation",
            "GPU",
            "memory",
            "paging",
        ] {
            assert!(rendered.contains(label), "missing compact label: {label}");
        }
        assert!(!rendered.contains("Terminal too small"));
    }

    #[test]
    fn overview_remains_useful_at_a_classic_eighty_column_size() {
        let rendered = render_app(&test_app(0), 80, 24);
        for label in ["LLM OPERATIONS", "generation", "GPU", "memory", "paging"] {
            assert!(rendered.contains(label), "missing 80-column label: {label}");
        }
        assert!(!rendered.contains("This dashboard needs"));
    }

    #[test]
    fn process_table_adapts_to_medium_terminals() {
        let rendered = render_app(&test_app(1), 100, 40);
        for label in ["PROCESS MONITOR", "THROUGHPUT", "PAGEIN/s", "STATE"] {
            assert!(rendered.contains(label), "missing process label: {label}");
        }
        assert!(!rendered.contains("MEM%"));
    }

    #[test]
    fn process_view_remains_scannable_at_eighty_columns() {
        let rendered = render_app(&test_app(1), 80, 24);
        for label in ["WORKLOAD SUMMARY", "RATE", "LLM PROCESSES", "PROCESS"] {
            assert!(
                rendered.contains(label),
                "missing narrow process label: {label}"
            );
        }
        assert!(!rendered.contains("This dashboard needs"));
    }

    #[test]
    fn help_remains_readable_at_eighty_columns() {
        let mut app = test_app(0);
        app.help = true;
        let rendered = render_app(&app, 80, 24);
        for label in ["KEYBOARD", "cycle Overview", "change refresh interval"] {
            assert!(
                rendered.contains(label),
                "missing narrow help label: {label}"
            );
        }
    }

    #[test]
    fn llm_top_sort_cycles_in_a_predictable_order() {
        assert_eq!(TopSort::Rss.next(), TopSort::Cpu);
        assert_eq!(TopSort::Cpu.next(), TopSort::Pid);
        assert_eq!(TopSort::Pid.next(), TopSort::Name);
        assert_eq!(TopSort::Name.next(), TopSort::Rss);
    }

    #[test]
    fn parses_detected_llm_processes_for_top_view() {
        let snapshot = parse_processes(
            "123 15728640 12.5 48.5 Rs 20 /usr/local/bin/omlx-server --port 8080\n\
             456 2048 0.1 0.0 S 0 /usr/bin/other-worker --task test",
        );
        let processes = snapshot.llm_processes;
        assert_eq!(processes.len(), 1);
        assert_eq!(snapshot.largest_consumer.as_deref(), Some("omlx-server"));
        assert_eq!(snapshot.provider.as_deref(), Some("oMLX"));
        assert_eq!(processes[0].pid, 123);
        assert_eq!(processes[0].name, "omlx-server");
        assert_eq!(processes[0].cpu, 12.5);
        assert_eq!(processes[0].memory_percent, Some(48.5));
        assert_eq!(processes[0].state, "Rs");
        assert_eq!(processes[0].pageins, Some(20));
        assert!(processes[0].command.contains("--port 8080"));
    }

    #[test]
    fn process_pagein_rates_are_pid_scoped() {
        let mut current = vec![LlmProcess {
            pid: 123,
            name: "omlx-server".into(),
            command: "omlx-server".into(),
            rss: 1,
            cpu: 0.0,
            memory_percent: Some(1.0),
            state: "S".into(),
            pageins: Some(30),
            pagein_rate: None,
        }];
        let previous = vec![LlmProcess {
            pid: 123,
            name: "omlx-server".into(),
            command: "omlx-server".into(),
            rss: 1,
            cpu: 0.0,
            memory_percent: Some(1.0),
            state: "S".into(),
            pageins: Some(10),
            pagein_rate: None,
        }];
        annotate_process_pagein_rates(&mut current, &previous, Duration::from_secs(2));
        assert_eq!(current[0].pagein_rate, Some(10.0));
    }

    #[test]
    fn command_text_drains_large_child_output() {
        let output = command_text(
            "/bin/sh",
            &[
                "-c",
                "i=0; while [ $i -lt 2048 ]; do printf 0123456789012345678901234567890123456789012345678901234567890123; i=$((i + 1)); done",
            ],
        )
        .expect("large child output should be collected");
        assert_eq!(output.len(), 2048 * 64);
    }

    #[test]
    fn composite_chart_normalizes_rate_indicators() {
        assert_eq!(normalize_chart_value(ChartMetric::Memory, 72), 72);
        assert_eq!(normalize_chart_value(ChartMetric::Gpu, 120), 100);
        assert_eq!(normalize_chart_value(ChartMetric::Cache, 120), 100);
        assert_eq!(normalize_chart_value(ChartMetric::Swap, 0), 0);
        // Log scale: small bursts stay visible, warn lands mid-plot, the
        // critical rate fills the axis.
        assert_eq!(
            normalize_chart_value(ChartMetric::Swap, 57 * 1024 + 640),
            12
        );
        assert_eq!(normalize_chart_value(ChartMetric::Swap, MIB), 51);
        assert_eq!(normalize_chart_value(ChartMetric::Swap, 8 * MIB), 88);
        assert_eq!(
            normalize_chart_value(ChartMetric::Swap, SWAP_CHART_SCALE),
            100
        );
        assert_eq!(normalize_chart_value(ChartMetric::Swap, 64 * MIB), 100);
    }

    #[test]
    fn generation_chart_uses_a_stable_scale() {
        let history = VecDeque::from([
            ChartPoint::new(Some(239), Tone::Cyan),
            ChartPoint::new(Some(302), Tone::Cyan),
        ]);
        assert_eq!(
            chart_scale_max(&history, ChartMetric::Generation),
            GENERATION_CHART_SCALE_MAX
        );
        assert_eq!(
            chart_display_value(ChartMetric::Generation, 175, GENERATION_CHART_SCALE_MAX),
            17
        );
        assert_eq!(chart_stat_label(ChartMetric::Generation, Some(239)), "23.9");
        assert_eq!(
            chart_scale_max(&history, ChartMetric::Prefill),
            PREFILL_CHART_SCALE_MAX
        );
        assert_eq!(
            chart_display_value(ChartMetric::Prefill, 1000, PREFILL_CHART_SCALE_MAX),
            50
        );
        assert_eq!(chart_stat_label(ChartMetric::Prefill, Some(1098)), "109.8");
        assert_eq!(chart_stat_label(ChartMetric::Cache, Some(67)), "67%");
    }

    #[test]
    fn inactive_rate_chart_identifies_the_other_active_phase() {
        let prefilling = Sample {
            llm_status: "prefilling".into(),
            ..Sample::default()
        };
        assert_eq!(
            chart_inactive_rate_label(ChartMetric::Generation, &prefilling),
            "prefill active"
        );
        assert_eq!(
            chart_inactive_rate_label(ChartMetric::Prefill, &prefilling),
            "active"
        );

        let generating = Sample {
            llm_status: "generating".into(),
            ..Sample::default()
        };
        assert_eq!(
            chart_inactive_rate_label(ChartMetric::Prefill, &generating),
            "decode active"
        );
    }

    #[test]
    fn stepped_chart_uses_thin_trace_bars() {
        assert_eq!(trace_point(0, 4), Some((3, '━')));
        assert_eq!(trace_point(1, 4), Some((3, '━')));
        assert_eq!(trace_point(50, 4), Some((2, '━')));
        assert_eq!(trace_point(100, 4), Some((0, '━')));
        assert_eq!(trace_point(100, 0), None);
    }

    #[test]
    fn stepped_chart_connectors_follow_the_level_being_crossed() {
        let mut cells = vec![
            vec![
                TraceCell {
                    glyph: ' ',
                    tone: Tone::Muted,
                };
                3
            ];
            7
        ];
        trace_connector(
            &mut cells,
            1,
            0,
            6,
            ChartMetric::Gpu,
            Tone::Red,
            Tone::Green,
        );
        assert_eq!(cells[0][1].glyph, '┓');
        assert_eq!(cells[0][1].tone, Tone::Red);
        assert_eq!(cells[1][1].glyph, '┃');
        assert_eq!(cells[1][1].tone, Tone::Yellow);
        assert_eq!(cells[2][1].tone, Tone::Green);
        assert_eq!(cells[6][1].glyph, '┗');
        assert_eq!(cells[6][1].tone, Tone::Green);

        trace_connector(
            &mut cells,
            2,
            6,
            0,
            ChartMetric::Gpu,
            Tone::Green,
            Tone::Yellow,
        );
        assert_eq!(cells[0][2].glyph, '┏');
        assert_eq!(cells[0][2].tone, Tone::Yellow);
        assert_eq!(cells[1][2].tone, Tone::Yellow);
        assert_eq!(cells[2][2].tone, Tone::Green);
        assert_eq!(cells[6][2].glyph, '┛');
        assert_eq!(cells[6][2].tone, Tone::Green);
    }

    #[test]
    fn chart_columns_keep_samples_in_a_scrolling_ring_buffer() {
        let history = VecDeque::from([
            ChartPoint::new(Some(10), Tone::Green),
            ChartPoint::new(Some(20), Tone::Yellow),
        ]);
        let columns = chart_columns(&history, 6);
        assert_eq!(
            columns.iter().map(|point| point.value).collect::<Vec<_>>(),
            vec![None, None, None, None, Some(10), Some(20)]
        );
        assert_eq!(columns[4].tone, Tone::Green);
        assert_eq!(columns[5].tone, Tone::Yellow);
    }

    #[test]
    fn chart_columns_keep_the_visible_tail_and_its_captured_tones() {
        let history = VecDeque::from([
            ChartPoint::new(Some(10), Tone::Green),
            ChartPoint::new(Some(20), Tone::Yellow),
            ChartPoint::new(Some(30), Tone::Red),
        ]);
        let columns = chart_columns(&history, 2);

        assert_eq!(
            columns.iter().map(|point| point.value).collect::<Vec<_>>(),
            vec![Some(20), Some(30)]
        );
        assert_eq!(columns[0].tone, Tone::Yellow);
        assert_eq!(columns[1].tone, Tone::Red);
    }

    #[test]
    fn chart_columns_keep_gaps_disconnected_when_scrolled() {
        let history = VecDeque::from([
            ChartPoint::new(Some(10), Tone::Green),
            ChartPoint::new(None, Tone::Muted),
            ChartPoint::new(Some(30), Tone::Green),
        ]);
        let columns = chart_columns(&history, 4);
        assert_eq!(
            columns.iter().map(|point| point.value).collect::<Vec<_>>(),
            vec![None, Some(10), None, Some(30)]
        );
        assert!(columns[2].break_before);
        assert!(!columns[3].break_before);
    }

    #[test]
    fn chart_columns_render_all_missing_data_without_dividing_by_zero() {
        let history = VecDeque::from([
            ChartPoint::new(None, Tone::Muted),
            ChartPoint::new(None, Tone::Muted),
            ChartPoint::new(None, Tone::Muted),
            ChartPoint::new(None, Tone::Muted),
            ChartPoint::new(None, Tone::Muted),
        ]);
        let columns = chart_columns(&history, 3);
        assert_eq!(columns.len(), 3);
        assert!(columns.iter().all(|point| point.value.is_none()));
    }

    #[test]
    fn bar_chart_keeps_each_sample_tone_independent() {
        let history = VecDeque::from([
            ChartPoint::new(Some(69), ChartMetric::Memory.tone(69)),
            ChartPoint::new(Some(85), ChartMetric::Memory.tone(85)),
        ]);
        assert_eq!(history[0].tone, Tone::Green);
        assert_eq!(history[1].tone, Tone::Red);
    }

    #[test]
    fn chart_smoothing_uses_dynamic_visual_resolution() {
        let history = VecDeque::from([
            ChartPoint::new(Some(100), Tone::Red),
            ChartPoint::new(Some(99), Tone::Red),
            ChartPoint::new(Some(98), Tone::Red),
            ChartPoint::new(Some(80), Tone::Yellow),
        ]);
        assert_eq!(
            chart_plot_values(&history, ChartMetric::Gpu, 10),
            vec![Some(100), Some(100), Some(100), Some(80)]
        );
        assert_eq!(
            chart_plot_values(&history, ChartMetric::Gpu, 20),
            vec![Some(100), Some(100), Some(98), Some(80)]
        );

        let generation_history = VecDeque::from([
            ChartPoint::new(Some(300), Tone::Cyan),
            ChartPoint::new(Some(299), Tone::Cyan),
            ChartPoint::new(Some(250), Tone::Cyan),
        ]);
        assert_eq!(
            chart_plot_values(&generation_history, ChartMetric::Generation, 10),
            vec![Some(300), Some(300), Some(250)]
        );

        let burst_history = VecDeque::from([
            ChartPoint::new(Some(0), Tone::Green),
            ChartPoint::new(Some(100), Tone::Red),
            ChartPoint::new(Some(0), Tone::Green),
            ChartPoint::new(Some(100), Tone::Red),
        ]);
        assert_eq!(
            chart_plot_values(&burst_history, ChartMetric::Gpu, 10),
            vec![Some(0), Some(100), Some(0), Some(100)]
        );
    }

    #[test]
    fn chart_smoothing_resets_after_missing_data() {
        let history = VecDeque::from([
            ChartPoint::new(Some(100), Tone::Red),
            ChartPoint::new(None, Tone::Muted),
            ChartPoint::new(Some(99), Tone::Red),
        ]);

        assert_eq!(
            chart_plot_values(&history, ChartMetric::Gpu, 10),
            vec![Some(100), None, Some(99)]
        );
    }

    #[test]
    fn rates_use_fractional_elapsed_time() {
        assert_eq!(rate_bytes(100, 4096, Duration::from_millis(1500)), 273_067);
        assert_eq!(signed_rate_bytes(150, 100, Duration::from_millis(2000)), 25);
        assert_eq!(
            signed_rate_bytes(100, 150, Duration::from_millis(2000)),
            -25
        );
    }

    #[test]
    fn severity_thresholds_have_a_recovery_band() {
        assert!(!threshold_with_hysteresis(69, false, 70, GPU_WARN_EXIT));
        assert!(threshold_with_hysteresis(75, false, 70, GPU_WARN_EXIT));
        assert!(threshold_with_hysteresis(71, true, 80, GPU_WARN_EXIT));
        assert!(!threshold_with_hysteresis(69, true, 80, GPU_WARN_EXIT));
    }

    #[test]
    fn journal_filters_use_structured_event_kinds() {
        assert!(JournalFilter::Llm.matches(EventKind::Llm));
        assert!(JournalFilter::Llm.matches(EventKind::Queue));
        assert!(!JournalFilter::Llm.matches(EventKind::Gpu));
        assert!(JournalFilter::Paging.matches(EventKind::Paging));
    }

    #[test]
    fn telemetry_source_and_loopback_policy_are_explicit() {
        assert_eq!(TelemetrySource::Live.label(), "live API");
        assert_eq!(TelemetrySource::Log.label(), "completion log");
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("[::1]"));
        assert!(!is_loopback_host("192.168.1.10"));
    }

    #[test]
    fn classification_reports_unknown_data_and_actionable_compression() {
        let mut sample = Sample {
            rate_ready: true,
            total_memory: 32 * 1024 * 1024 * 1024,
            availability: Some(50),
            pressure: "GREEN".into(),
            vm_available: true,
            swap_available: true,
            thermal: "no warning".into(),
            llm_count: 1,
            ..Sample::default()
        };
        classify(&mut sample, None);
        assert_eq!(sample.impact, "LLM READY");

        sample.vm_available = false;
        classify(&mut sample, None);
        assert_eq!(sample.impact, "DATA LIMITED");
        assert_eq!(sample.guidance_badge, "CHECK");

        sample.vm_available = true;
        sample.compress = COMPRESSION_WARN_RATE;
        classify(&mut sample, None);
        assert_eq!(sample.impact, "COMPRESSION ACTIVE");
        assert_eq!(sample.guidance_badge, "WATCH");
    }

    #[test]
    fn parses_llm_completion_stats() {
        let stats = parse_llm_completion_line(
            "2026-08-25 03:00:00 Chat completion: model=oQ4e-mtp, 128 tokens in 4.0s (32.0 tok/s), prompt: 4096, finish_reason=stop, max_tokens=512",
        )
        .expect("completion should parse");
        assert_eq!(stats.model.as_deref(), Some("oQ4e-mtp"));
        assert_eq!(stats.output_tokens, Some(128));
        assert_eq!(stats.prompt_tokens, Some(4096));
        assert_eq!(stats.tokens_per_second, Some(32.0));
    }

    #[test]
    fn parses_omlx_responses_api_stats_without_prompt_count() {
        let stats = parse_llm_completion_line(
            "2026-08-25 01:08:35,903 - omlx.server - INFO - [-] - Responses API: model=Qwen3.8-27B-oQ4e-mtp, 8407 tokens in 449.88s (18.7 tok/s)",
        )
        .expect("oMLX completion should parse");
        assert_eq!(stats.model.as_deref(), Some("Qwen3.8-27B-oQ4e-mtp"));
        assert_eq!(stats.output_tokens, Some(8407));
        assert_eq!(stats.prompt_tokens, None);
        assert_eq!(stats.tokens_per_second, Some(18.7));
    }

    #[test]
    fn parses_omlx_live_stats_and_active_request() {
        let health: Value = serde_json::from_str(
            r#"{"status":"healthy","default_model":"Qwen3.8-27B-oQ4e-mtp","engine_pool":{"final_ceiling":36507222016,"current_model_memory":16852732434}}"#,
        )
        .unwrap();
        let stats: Value = serde_json::from_str(
            r#"{
                "avg_generation_tps": 30.7,
                "avg_prefill_tps": 132.9,
                "cache_efficiency": 62.1,
                "engines": {"mlx-lm": {"version": "0.31.3"}},
                "active_models": {
                    "model_memory_used": 20990983024,
                    "model_memory_max": 36507222016,
                    "models": [{
                        "id": "Qwen3.8-27B-oQ4e-mtp",
                        "active_requests": 1,
                        "waiting_requests": 0,
                        "generating": [{"generated_tokens": 3849,"prompt_tokens": 12632,"tokens_per_second": 29.4}]
                    }]
                },
                "runtime_cache": {
                    "hot_cache_size_bytes": 388562944,
                    "hot_cache_max_bytes": 536870912,
                    "models": [{"cache_rates":{"cumulative":{"prefix_hit_rate":0.5471,"ssd_hot_rate":0.2772}}}]
                }
            }"#,
        )
        .unwrap();
        let telemetry = parse_omlx_telemetry(&health, Some(&stats));
        assert_eq!(telemetry.source, TelemetrySource::Live);
        assert_eq!(telemetry.provider.as_deref(), Some("oMLX"));
        assert_eq!(telemetry.status.as_deref(), Some("generating"));
        assert_eq!(telemetry.model.as_deref(), Some("Qwen3.8-27B-oQ4e-mtp"));
        assert_eq!(telemetry.generation_tps, Some(29.4));
        assert!(telemetry.generation_tps_live);
        assert_eq!(telemetry.prefill_tps, Some(132.9));
        assert!(!telemetry.prefill_tps_live);
        assert_eq!(telemetry.prompt_tokens, Some(12632));
        assert_eq!(telemetry.prefix_hit_rate, Some(54.71));
        assert_eq!(telemetry.mlx.version.as_deref(), Some("0.31.3"));
    }

    #[test]
    fn parses_omlx_prefill_progress_speed_as_live_rate() {
        let health = json!({
            "status": "healthy",
            "default_model": "model"
        });
        let stats = json!({
            "avg_generation_tps": 23.5,
            "avg_prefill_tps": 14.1,
            "active_models": {
                "models": [{
                    "id": "model",
                    "active_requests": 1,
                    "waiting_requests": 0,
                    "prefilling": [{
                        "processed": 2048,
                        "total": 32768,
                        "speed": 14.1,
                        "prompt_tokens": 32768
                    }],
                    "generating": []
                }]
            }
        });

        let telemetry = parse_omlx_telemetry(&health, Some(&stats));

        assert_eq!(telemetry.status.as_deref(), Some("prefilling"));
        assert_eq!(telemetry.prefill_tps, Some(14.1));
        assert!(telemetry.prefill_tps_live);
        assert_eq!(telemetry.prompt_tokens, Some(32768));
    }

    #[test]
    fn rate_charts_skip_aggregate_and_log_fallbacks() {
        let sample = Sample {
            llm_source: TelemetrySource::Live,
            llm_status: "prefilling".into(),
            llm_generation_tps: Some(23.5),
            llm_prefill_tps: Some(14.1),
            ..Sample::default()
        };
        assert_eq!(chart_rate_value(&sample, ChartMetric::Generation), None);
        assert_eq!(chart_rate_value(&sample, ChartMetric::Prefill), None);

        let active = Sample {
            llm_source: TelemetrySource::Live,
            llm_generation_tps: Some(23.5),
            llm_generation_tps_live: true,
            ..Sample::default()
        };
        assert_eq!(
            chart_rate_value(&active, ChartMetric::Generation),
            Some(235)
        );

        let log_sample = Sample {
            llm_source: TelemetrySource::Log,
            llm_generation_tps: Some(23.5),
            ..Sample::default()
        };
        assert_eq!(chart_rate_value(&log_sample, ChartMetric::Generation), None);
    }

    #[test]
    fn cache_chart_uses_interval_counter_deltas() {
        let first = LlmTelemetry {
            total_prompt_tokens: Some(100),
            total_cached_tokens: Some(20),
            ..LlmTelemetry::default()
        };
        let second = LlmTelemetry {
            total_prompt_tokens: Some(150),
            total_cached_tokens: Some(50),
            ..LlmTelemetry::default()
        };
        let mut previous = None;

        assert_eq!(
            cache_interval_efficiency(&mut previous, Some(&first), false),
            None
        );
        assert_eq!(
            cache_interval_efficiency(&mut previous, Some(&second), false),
            Some(60.0)
        );
        assert_eq!(
            cache_interval_efficiency(&mut previous, Some(&second), false),
            None
        );
    }

    #[test]
    fn correlation_explains_a_generation_drop_with_contemporaneous_signals() {
        let mut engine = CorrelationEngine::default();
        let baseline = Sample {
            llm_provider: "oMLX".into(),
            llm_model: "model".into(),
            llm_generation_tps: Some(30.0),
            llm_generation_tps_live: true,
            llm_prompt_tokens: Some(20_000),
            llm_output_tokens: Some(1_000),
            llm_active_requests: Some(1),
            gpu_util: Some(99),
            gpu_in_use: Some(96),
            gpu_alloc: Some(100),
            ..Sample::default()
        };
        engine.observe(&baseline);

        let current = Sample {
            llm_provider: "oMLX".into(),
            llm_model: "model".into(),
            llm_generation_tps: Some(25.0),
            llm_generation_tps_live: true,
            llm_prompt_tokens: Some(30_000),
            llm_output_tokens: Some(3_000),
            llm_active_requests: Some(1),
            gpu_util: Some(99),
            gpu_in_use: Some(96),
            gpu_alloc: Some(100),
            ..Sample::default()
        };
        let insight = engine.observe(&current);

        assert_eq!(insight.direction, ThroughputDirection::Down);
        assert_eq!(insight.cause, CorrelationCause::ContextGrowth);
        assert!(insight.summary.contains("GEN ↓16.7%"));
        assert!(insight.summary.contains("GPU 99% busy"));
        assert!(insight.summary.contains("context"));
        assert!(insight.details.contains("Metal MEM 96%"));
        assert_eq!(
            insight.event_key,
            Some(CorrelationKey {
                direction: ThroughputDirection::Down,
                cause: CorrelationCause::ContextGrowth,
            })
        );
    }

    #[test]
    fn correlation_is_honest_when_no_system_signal_moved_with_rate() {
        let mut engine = CorrelationEngine::default();
        engine.observe(&Sample {
            llm_provider: "oMLX".into(),
            llm_model: "model".into(),
            llm_generation_tps: Some(30.0),
            llm_generation_tps_live: true,
            ..Sample::default()
        });
        let insight = engine.observe(&Sample {
            llm_provider: "oMLX".into(),
            llm_model: "model".into(),
            llm_generation_tps: Some(25.0),
            llm_generation_tps_live: true,
            ..Sample::default()
        });

        assert_eq!(insight.cause, CorrelationCause::Runtime);
        assert_eq!(insight.confidence_label(), "low");
        assert!(insight.summary.contains("no matching system signal"));
        assert!(insight.details.contains("workload/runtime change"));
    }

    #[test]
    fn correlation_card_prefers_a_readable_primary_signal() {
        let sample = Sample {
            llm_generation_tps: Some(23.9),
            gpu_in_use: Some(96),
            gpu_alloc: Some(100),
            correlation: CorrelationInsight {
                cause: CorrelationCause::MetalMemory,
                summary: "GEN 23.9 tok/s · correlated: Metal MEM 96% (21.0 GiB / 21.7 GiB)".into(),
                ..CorrelationInsight::default()
            },
            ..Sample::default()
        };

        assert_eq!(
            correlation_display(&sample, 48).as_deref(),
            Some("GEN 23.9 tok/s · Metal mem 96%")
        );
        assert!(
            correlation_display(&sample, 22)
                .expect("compact diagnosis should be present")
                .chars()
                .count()
                <= 22
        );
    }

    #[test]
    fn provider_telemetry_counts_as_an_observed_llm_without_a_matching_process() {
        let sample = Sample {
            llm_provider: "oMLX".into(),
            llm_model: "model".into(),
            llm_source: TelemetrySource::Live,
            llm_generation_tps: Some(25.0),
            llm_generation_tps_live: true,
            ..Sample::default()
        };

        assert!(llm_is_observed(&sample));
        assert!(!hero_impact(&sample).contains("No local LLM process detected"));
    }

    #[test]
    fn parses_omlx_waiting_prompt_tokens() {
        let health: Value =
            serde_json::from_str(r#"{"status":"healthy","default_model":"Qwen3.8-27B-oQ4e-mtp"}"#)
                .unwrap();
        let stats = json!({
            "active_models": {"models": [{
                "id": "Qwen3.8-27B-oQ4e-mtp",
                "active_requests": 0,
                "waiting_requests": 1,
                "waiting": [{"prompt_tokens": 12632}]
            }]}
        });
        let telemetry = parse_omlx_telemetry(&health, Some(&stats));
        assert_eq!(telemetry.status.as_deref(), Some("waiting"));
        assert_eq!(telemetry.prompt_tokens, Some(12632));
    }

    #[test]
    fn retained_omlx_rates_are_not_marked_live_when_idle() {
        let health: Value =
            serde_json::from_str(r#"{"status":"healthy","default_model":"Qwen3.8-27B-oQ4e-mtp"}"#)
                .unwrap();
        let stats = json!({
            "avg_generation_tps": 31.6,
            "avg_prefill_tps": 133.2,
            "active_models": {"models": [{
                "id": "Qwen3.8-27B-oQ4e-mtp",
                "active_requests": 0,
                "waiting_requests": 0,
                "generating": []
            }]}
        });
        let telemetry = parse_omlx_telemetry(&health, Some(&stats));

        assert_eq!(telemetry.status.as_deref(), Some("idle"));
        assert_eq!(telemetry.generation_tps, Some(31.6));
        assert_eq!(telemetry.prefill_tps, Some(133.2));
        assert!(!telemetry.generation_tps_live);
        assert!(!telemetry.prefill_tps_live);

        let sample = Sample {
            llm_source: TelemetrySource::Live,
            llm_generation_tps: telemetry.generation_tps,
            llm_prefill_tps: telemetry.prefill_tps,
            ..Sample::default()
        };
        assert_eq!(llm_generation_rate_label(&sample), "AVG GEN 31.6 tok/s");
        assert_eq!(llm_prefill_rate_label(&sample), "AVG PREFILL 133.2 tok/s");
    }

    #[test]
    fn log_rates_are_labeled_as_last_results() {
        let sample = Sample {
            llm_source: TelemetrySource::Log,
            llm_generation_tps: Some(28.5),
            ..Sample::default()
        };

        assert_eq!(llm_generation_rate_label(&sample), "LAST GEN 28.5 tok/s");
    }

    #[test]
    fn correlation_ignores_retained_idle_rates() {
        let mut engine = CorrelationEngine::default();
        engine.observe(&Sample {
            llm_provider: "oMLX".into(),
            llm_model: "model".into(),
            llm_generation_tps: Some(30.0),
            ..Sample::default()
        });
        let insight = engine.observe(&Sample {
            llm_provider: "oMLX".into(),
            llm_model: "model".into(),
            llm_generation_tps: Some(25.0),
            ..Sample::default()
        });

        assert_eq!(insight.direction, ThroughputDirection::Unknown);
        assert_eq!(insight.cause, CorrelationCause::None);
        assert!(insight.summary.is_empty());
    }

    #[test]
    fn parses_mlx_allocator_and_device_metrics_without_inference() {
        let health = json!({
            "status": "healthy",
            "default_model": "Qwen3.8-27B-oQ4e-mtp",
            "mlx_version": "0.29.3",
            "mlx_memory": {"active_bytes": 4_000, "peak_bytes": 8_000}
        });
        let stats = json!({"mlx": {"cache_bytes": 2_000}});
        let telemetry = parse_mlx_runtime_telemetry(&health, Some(&stats));

        assert_eq!(telemetry.version.as_deref(), Some("0.29.3"));
        assert_eq!(telemetry.active_memory, Some(4_000));
        assert_eq!(telemetry.cache_memory, Some(2_000));
        assert_eq!(telemetry.peak_memory, Some(8_000));
    }

    #[test]
    fn parses_mlx_device_metadata_and_metal_limit() {
        let device = json!({
            "chip_name": "Apple M4",
            "gpu_cores": 10,
            "device_name": "Apple M4 GPU",
            "architecture": "arm64",
            "max_recommended_working_set_size": 27_000
        });
        let settings = json!({
            "system": {
                "active_memory_bytes": 18_000,
                "omlx_phys_footprint_bytes": 12_000,
                "iogpu_wired_limit_bytes": 24_000
            }
        });
        let telemetry = parse_mlx_metadata(Some(&device), Some(&settings));

        assert_eq!(telemetry.device_name.as_deref(), Some("Apple M4 GPU"));
        assert_eq!(telemetry.architecture.as_deref(), Some("arm64"));
        assert_eq!(telemetry.memory_size, None);
        assert_eq!(telemetry.active_memory, None);
        assert_eq!(telemetry.recommended_working_set, Some(27_000));
        assert_eq!(telemetry.process_footprint, Some(12_000));
        assert_eq!(telemetry.resource_limit, Some(24_000));
    }

    #[test]
    fn converts_omlx_device_memory_gb_to_bytes() {
        let device = json!({"memory_gb": 36});
        let telemetry = parse_mlx_metadata(Some(&device), None);

        assert_eq!(telemetry.memory_size, Some(36 * 1024 * 1024 * 1024));
    }

    #[test]
    fn parses_metal_stats_without_charging_driver_memory_as_gpu_memory() {
        let ioreg = r#"
            "PerformanceStatistics" = {"In use system memory (driver)"=0,"Alloc system memory"=3224567808,"Tiler Utilization %"=16,"Renderer Utilization %"=15,"Device Utilization %"=16,"In use system memory"=850280448}
            "model" = "Apple M4"
            "gpu-core-count" = 10
        "#;
        assert_eq!(
            parse_gpu(ioreg),
            (Some(16), Some(3_224_567_808), Some(850_280_448))
        );
        let metal = parse_metal_hardware(ioreg);
        assert_eq!(metal.device_name.as_deref(), Some("Apple M4"));
        assert_eq!(metal.gpu_cores, Some(10));
        assert_eq!(metal.renderer_util, Some(15));
        assert_eq!(metal.tiler_util, Some(16));
    }

    #[test]
    fn diagnostic_fields_are_single_line_and_bounded() {
        let field = log_field("model name\nwith\tunsafe=characters/and a very long suffix");

        assert!(!field.contains('\n'));
        assert!(!field.contains('\r'));
        assert!(!field.contains('\t'));
        assert!(!field.contains(' '));
        assert!(field.chars().count() <= 160);
        assert!(field.contains("model_name_with"));
    }

    #[test]
    fn panic_payload_formats_string_and_static_string_panics() {
        let owned: Box<dyn Any + Send> = Box::new(String::from("owned panic"));
        let static_text: Box<dyn Any + Send> = Box::new("static panic");

        assert_eq!(panic_payload(owned.as_ref()), "owned panic");
        assert_eq!(panic_payload(static_text.as_ref()), "static panic");
    }
}
