// SPDX-License-Identifier: MIT
//! Shared observation and event types. Values remain independent of terminal widgets.
use crate::config::{load_tone, Thresholds};
use crate::{gpu, process_memory};
use std::time::SystemTime;
pub(crate) const MIB: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Green,
    Yellow,
    Red,
    Cyan,
    Blue,
    Muted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TelemetrySource {
    #[default]
    None,
    Live,
    Log,
    Report,
}

impl TelemetrySource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::None => "unavailable",
            Self::Live => "live API",
            Self::Log => "completion log",
            Self::Report => "reported usage",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EventKind {
    System,
    Llm,
    Queue,
    Pressure,
    Paging,
    Gpu,
    Thermal,
}

impl EventKind {
    pub(crate) fn from_state(state: &str) -> Self {
        match state {
            "LLM" | "PROMPT" => Self::Llm,
            "QUEUE" => Self::Queue,
            "PRESSURE" | "MEMORY BOTTLENECK" | "MEMORY STRESS" => Self::Pressure,
            "PAGING" | "SWAP THRASHING" | "HEAVY PAGING" | "PAGE-IN RECOVERY" | "PAGING ACTIVE"
            | "WATCH PAGING" => Self::Paging,
            "GPU" | "CPU OFFLOAD" | "VRAM FULL" | "GPU THROTTLED" => Self::Gpu,
            "THERMAL" => Self::Thermal,
            _ => Self::System,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChartMetric {
    Generation,
    Prefill,
    Cache,
    Memory,
    Swap,
    Compression,
    Gpu,
}

impl ChartMetric {
    /// Byte-rate series share automatic zero-based ceilings and rate labels.
    pub(crate) fn is_byte_rate(self) -> bool {
        matches!(self, Self::Swap | Self::Compression)
    }

    pub(crate) fn chart_tone(self) -> Tone {
        match self {
            Self::Generation | Self::Prefill | Self::Cache => Tone::Cyan,
            Self::Memory | Self::Swap | Self::Compression => Tone::Cyan,
            Self::Gpu => Tone::Blue,
        }
    }

    pub(crate) fn tone(self, value: u64, thresholds: Thresholds) -> Tone {
        match self {
            // Throughput is graded against its rolling baseline when captured.
            Self::Generation | Self::Prefill => Tone::Cyan,
            Self::Cache if value >= CACHE_GOOD_PERCENT => Tone::Green,
            Self::Cache if value >= CACHE_LOW_PERCENT => Tone::Yellow,
            Self::Cache => Tone::Red,
            // Linux derives pressure from unavailable memory (MemAvailable),
            // using these bands. Resident RAM history records pressure from
            // the sample instead; occupied file cache is not a pressure signal.
            Self::Memory => load_tone(
                value,
                thresholds.memory_warn_load,
                thresholds.memory_critical_load,
            ),
            // Saturation is graded red on the chart; it never rings the
            // alarm or becomes a finding without a measured slowdown.
            Self::Gpu => load_tone(
                value,
                thresholds.gpu_warn_load,
                thresholds.gpu_critical_load,
            ),
            Self::Swap => load_tone(
                value,
                thresholds.swap_warn_rate,
                thresholds.swap_critical_rate,
            ),
            // Compression trades CPU for RAM; alone it is a warning, never critical.
            Self::Compression if value >= thresholds.compression_warn_rate => Tone::Yellow,
            Self::Compression => Tone::Green,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ChartPoint {
    pub(crate) value: Option<u64>,
    pub(crate) tone: Tone,
    pub(crate) observed_at: SystemTime,
}

#[derive(Default)]
pub(crate) struct Consumer {
    pub(crate) name: String,
    pub(crate) rss: u64,
    pub(crate) processes: u32,
}

#[derive(Clone)]
pub(crate) struct LlmProcess {
    pub(crate) pid: u32,
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) rss: u64,
    pub(crate) cpu: f64,
    pub(crate) memory_percent: Option<f64>,
    pub(crate) state: String,
    pub(crate) pageins: Option<u64>,
    pub(crate) pagein_rate: Option<f64>,
}

pub(crate) struct ProcessSnapshot {
    pub(crate) llm_count: u32,
    pub(crate) llm_rss: u64,
    pub(crate) llm_cpu: f64,
    pub(crate) top_llm: Option<LlmProcess>,
    pub(crate) provider: Option<String>,
    pub(crate) largest_consumer: Option<String>,
    pub(crate) llm_processes: Vec<LlmProcess>,
}

#[derive(Clone)]
pub(crate) struct SignalEvent {
    pub(crate) time: String,
    pub(crate) recorded_at: SystemTime,
    pub(crate) kind: EventKind,
    pub(crate) state: String,
    pub(crate) summary: String,
    pub(crate) tone: Tone,
}

#[derive(Default)]
pub(crate) struct LlmLogStats {
    pub(crate) model: Option<String>,
    pub(crate) tokens_per_second: Option<f64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) prompt_tokens: Option<u64>,
    pub(crate) observed_at: Option<SystemTime>,
}

/// MLX allocator and device counters reported by the serving runtime.
///
/// MLX keeps allocator state inside the process that owns the arrays.  The
/// monitor therefore never invents these values from RSS or GPU utilization;
/// they are populated only from a provider endpoint that can observe the
/// serving process directly.
#[derive(Clone, Default)]
pub(crate) struct MlxTelemetry {
    pub(crate) version: Option<String>,
    pub(crate) active_memory: Option<u64>,
    pub(crate) cache_memory: Option<u64>,
    pub(crate) peak_memory: Option<u64>,
    pub(crate) device_name: Option<String>,
    pub(crate) architecture: Option<String>,
    pub(crate) memory_size: Option<u64>,
    pub(crate) recommended_working_set: Option<u64>,
    pub(crate) max_buffer_size: Option<u64>,
    pub(crate) resource_limit: Option<u64>,
    pub(crate) process_footprint: Option<u64>,
}

#[derive(Clone, Default)]
pub(crate) struct MetalTelemetry {
    pub(crate) device_name: Option<String>,
    pub(crate) architecture: Option<String>,
    pub(crate) gpu_cores: Option<u16>,
    pub(crate) renderer_util: Option<u8>,
    pub(crate) tiler_util: Option<u8>,
    pub(crate) resource_limit: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ThroughputDirection {
    #[default]
    Unknown,
    Down,
    Up,
    Flat,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CorrelationCause {
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
    pub(crate) fn label(self) -> &'static str {
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

    pub(crate) fn tone(self) -> Tone {
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
pub(crate) struct CorrelationKey {
    pub(crate) direction: ThroughputDirection,
    pub(crate) cause: CorrelationCause,
}

#[derive(Clone, Default)]
pub(crate) struct CorrelationInsight {
    pub(crate) direction: ThroughputDirection,
    pub(crate) cause: CorrelationCause,
    pub(crate) confidence: u8,
    pub(crate) summary: String,
    pub(crate) details: String,
    pub(crate) event_key: Option<CorrelationKey>,
    /// Change from the rolling throughput baseline, in percent.
    pub(crate) delta_percent: Option<f64>,
}

/// Cache reuse bands, shared by the cache chart and the prompt's CACHED share.
pub(crate) const CACHE_GOOD_PERCENT: u64 = 50;
pub(crate) const CACHE_LOW_PERCENT: u64 = 20;

/// A drop this far below the rolling baseline is critical on its own.
pub(crate) const THROUGHPUT_CRITICAL_DROP_PERCENT: f64 = 30.0;

impl CorrelationInsight {
    pub(crate) fn is_material_drop(&self) -> bool {
        self.direction == ThroughputDirection::Down
    }

    pub(crate) fn tone(&self) -> Tone {
        match self.direction {
            ThroughputDirection::Down => {
                let severe = self
                    .delta_percent
                    .is_some_and(|percent| percent <= -THROUGHPUT_CRITICAL_DROP_PERCENT);
                if severe
                    || matches!(
                        self.cause,
                        CorrelationCause::Paging
                            | CorrelationCause::MemoryPressure
                            | CorrelationCause::Thermal
                    )
                {
                    Tone::Red
                } else {
                    Tone::Yellow
                }
            }
            ThroughputDirection::Up => Tone::Green,
            ThroughputDirection::Unknown | ThroughputDirection::Flat => self.cause.tone(),
        }
    }

    pub(crate) fn confidence_label(&self) -> &'static str {
        match self.confidence {
            80..=u8::MAX => "high",
            55..=79 => "medium",
            1..=54 => "low",
            _ => "unavailable",
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct LlmTelemetry {
    pub(crate) source: TelemetrySource,
    pub(crate) observed_at: Option<SystemTime>,
    pub(crate) provider: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) generation_tps: Option<f64>,
    pub(crate) generation_tps_live: bool,
    pub(crate) prefill_tps: Option<f64>,
    pub(crate) prefill_tps_live: bool,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) prompt_tokens: Option<u64>,
    pub(crate) requests: Vec<RequestUsage>,
    pub(crate) cache_efficiency: Option<f64>,
    pub(crate) prefix_hit_rate: Option<f64>,
    pub(crate) total_prompt_tokens: Option<u64>,
    pub(crate) total_cached_tokens: Option<u64>,
    pub(crate) active_requests: Option<u64>,
    pub(crate) waiting_requests: Option<u64>,
    pub(crate) model_memory: Option<u64>,
    pub(crate) model_memory_max: Option<u64>,
    /// Model weights and buffers the runtime placed outside GPU memory.
    /// Only runtimes that report placement (Ollama) set it.
    pub(crate) model_offloaded: Option<u64>,
    pub(crate) model_size: Option<u64>,
    pub(crate) details: Option<String>,
    pub(crate) remote: bool,
    pub(crate) cache_interval_efficiency: Option<f64>,
    pub(crate) mlx: MlxTelemetry,
}

#[derive(Clone)]
pub(crate) struct Sample {
    pub(crate) runtime: crate::runtime_diagnostics::RuntimeReport,
    pub(crate) updated: String,
    /// Local offset from UTC in seconds, read alongside `updated`. Request
    /// times use it so every clock on screen shares the Journal's zone.
    pub(crate) utc_offset: Option<i32>,
    pub(crate) pressure: String,
    pub(crate) pressure_meaning: String,
    pub(crate) pressure_tone: Tone,
    pub(crate) availability: Option<u8>,
    // Physical RAM occupied, including file cache. Separate from macOS's
    // memorystatus level, which includes pageable application memory.
    pub(crate) resident_memory: Option<u64>,
    pub(crate) total_memory: u64,
    pub(crate) wired: u64,
    pub(crate) compressor: u64,
    pub(crate) compressed_logical: u64,
    pub(crate) anonymous: u64,
    pub(crate) file_backed: u64,
    pub(crate) swap_total: u64,
    pub(crate) swap_used: u64,
    pub(crate) swap_available: bool,
    /// The platform has no page-in/page-out counters (Windows), so paging
    /// rates are unknown rather than zero.
    pub(crate) paging_unavailable: bool,
    pub(crate) swap_in: u64,
    pub(crate) swap_out: u64,
    pub(crate) swap_growth: i64,
    pub(crate) compress: u64,
    pub(crate) decompress: u64,
    pub(crate) reactivated: u64,
    pub(crate) vm_available: bool,
    pub(crate) gpu_util: Option<u8>,
    pub(crate) gpu_alloc: Option<u64>,
    pub(crate) gpu_in_use: Option<u64>,
    pub(crate) gpus: Vec<gpu::Device>,
    pub(crate) metal: MetalTelemetry,
    pub(crate) mlx: MlxTelemetry,
    pub(crate) thermal: String,
    pub(crate) llm_count: u32,
    pub(crate) llm_rss: u64,
    pub(crate) process_memory: Option<process_memory::Reading>,
    pub(crate) process_memory_growth: Option<i64>,
    pub(crate) llm_cpu: f64,
    pub(crate) llm_processes: Vec<LlmProcess>,
    pub(crate) largest_consumer: Option<String>,
    pub(crate) llm_model: String,
    pub(crate) llm_provider: String,
    pub(crate) llm_status: String,
    pub(crate) llm_source: TelemetrySource,
    pub(crate) llm_observed_at: Option<SystemTime>,
    pub(crate) llm_generation_tps: Option<f64>,
    pub(crate) llm_generation_tps_live: bool,
    pub(crate) llm_prefill_tps: Option<f64>,
    pub(crate) llm_prefill_tps_live: bool,
    pub(crate) llm_output_tokens: Option<u64>,
    pub(crate) llm_prompt_tokens: Option<u64>,
    pub(crate) llm_requests: Vec<RequestUsage>,
    pub(crate) llm_cache_efficiency: Option<f64>,
    pub(crate) llm_cache_interval_efficiency: Option<f64>,
    pub(crate) llm_prefix_hit_rate: Option<f64>,
    pub(crate) llm_active_requests: Option<u64>,
    pub(crate) llm_waiting_requests: Option<u64>,
    pub(crate) llm_model_memory: Option<u64>,
    pub(crate) llm_model_memory_max: Option<u64>,
    pub(crate) llm_model_offloaded: Option<u64>,
    pub(crate) llm_model_size: Option<u64>,
    /// Most severe NVIDIA or placement problem, set by `classify`.
    pub(crate) gpu_issue: Option<crate::gpu_findings::GpuIssue>,
    pub(crate) llm_details: Option<String>,
    pub(crate) llm_remote: bool,
    pub(crate) correlation: CorrelationInsight,
    pub(crate) llm_top: String,
    pub(crate) llm_pid: u32,
    pub(crate) impact: String,
    pub(crate) impact_tone: Tone,
    pub(crate) health: Option<u16>,
    pub(crate) grade: String,
    pub(crate) limiter: String,
    pub(crate) guidance_badge: String,
    pub(crate) guidance_cause: String,
    pub(crate) guidance_action: String,
    pub(crate) rate_ready: bool,
}

impl Sample {
    /// Paging rates are measured and the rate baseline is ready.
    pub(crate) fn paging_measured(&self) -> bool {
        self.swap_available && self.rate_ready && !self.paging_unavailable
    }

    pub(crate) fn has_nvidia_gpus(&self) -> bool {
        cfg!(any(target_os = "linux", target_os = "windows")) && !self.gpus.is_empty()
    }
}

impl Default for Sample {
    fn default() -> Self {
        Self {
            runtime: Default::default(),
            updated: "waiting".into(),
            utc_offset: None,
            pressure: "UNKNOWN".into(),
            pressure_meaning: "unavailable".into(),
            pressure_tone: Tone::Muted,
            availability: None,
            resident_memory: None,
            total_memory: 0,
            wired: 0,
            compressor: 0,
            compressed_logical: 0,
            anonymous: 0,
            file_backed: 0,
            swap_total: 0,
            swap_used: 0,
            swap_available: false,
            paging_unavailable: false,
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
            gpus: Vec::new(),
            metal: MetalTelemetry::default(),
            mlx: MlxTelemetry::default(),
            thermal: "unavailable".into(),
            llm_count: 0,
            llm_rss: 0,
            process_memory: None,
            process_memory_growth: None,
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
            llm_requests: Vec::new(),
            llm_cache_efficiency: None,
            llm_cache_interval_efficiency: None,
            llm_prefix_hit_rate: None,
            llm_active_requests: None,
            llm_waiting_requests: None,
            llm_model_memory: None,
            llm_model_memory_max: None,
            llm_model_offloaded: None,
            llm_model_size: None,
            gpu_issue: None,
            llm_details: None,
            llm_remote: false,
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

#[derive(Default)]
pub(crate) struct VmCounters {
    pub(crate) free: Option<u64>,
    pub(crate) speculative: Option<u64>,
    pub(crate) wired: u64,
    pub(crate) compressor: u64,
    pub(crate) compressed_logical: u64,
    pub(crate) anonymous: u64,
    pub(crate) file_backed: u64,
    pub(crate) swapins: u64,
    pub(crate) swapouts: u64,
    pub(crate) compressions: u64,
    pub(crate) decompressions: u64,
    pub(crate) reactivations: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct RequestUsage {
    pub provider: String,
    pub model: String,
    pub id: String,
    pub prompt: u64,
    pub cached: Option<u64>,
    pub output: Option<u64>,
    /// Request-scoped output throughput. Never populated from server averages
    /// or the prefill phase; a completed record supplies its final average.
    pub output_tps: Option<f64>,
    pub completed: bool,
    pub ttft_ms: Option<u64>,
    pub observed_at: Option<SystemTime>,
}
