// SPDX-License-Identifier: MIT
//! Health classification and generation slowdowns; no terminal rendering.
//!
//! A slowdown is found by anomaly detection (`crate::anomaly`): generation
//! speed unusually far below its usual value, three samples running. Its
//! reason is whatever else is unusual at the same time.
use crate::anomaly::{median, Usual, WINDOW};
use crate::config::{Thresholds, PAGING_ACTIVE_ENTER_RATE};
use crate::domain::{
    CorrelationCause, CorrelationInsight, CorrelationKey, DiskActivity, DropEnd, DropEpisode,
    Sample, TelemetrySource, ThroughputDirection, Tone, MIB,
};
use crate::formatting::{
    bytes, compact_tokens, llm_generation_rate_label, llm_prefill_rate_label, percent, rate,
    signed_rate,
};
use std::collections::VecDeque;
use std::time::SystemTime;
/// A prompt this much longer than usual, and half as long again, is unusual.
pub(crate) const CONTEXT_GROWTH_TOKENS: u64 = 1024;
/// A generating request that goes this long without a token has stalled:
/// at 20–90 tok/s a healthy decode produces one every 50 ms or so.
pub(crate) const STALL_SECONDS: f64 = 1.5;
/// Runtime storage traffic worth naming. A decoding runtime touches its disk
/// only to load or save prompt cache; weights are already in memory.
pub(crate) const DISK_ACTIVE_RATE: u64 = 16 * MIB;
/// Generation is slow when it is unusually far below its usual speed, and
/// by at least this share of it and this many tok/s.
pub(crate) const SLOW_SHARE: f64 = 0.20;
pub(crate) const SLOW_MIN_TPS: f64 = 2.0;
/// Slow samples in a row that begin a slowdown, and samples back at the
/// usual speed that end one: a single jumpy poll does neither.
pub(crate) const SLOW_SAMPLES: usize = 3;
/// A slowdown with nothing else unusual becomes the usual speed after this
/// long, and one whose request has been idle this long is over.
pub(crate) const SETTLE_SECONDS: f64 = 30.0;
/// A slowdown with the same reason as one seen within this long is the same
/// story: the Journal does not tell it again.
pub(crate) const SAME_STORY_SECONDS: f64 = 60.0;

#[derive(Clone, Default)]
pub(crate) struct CorrelationObservation {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) generation_tps: Option<f64>,
    pub(crate) gpu_util: Option<u8>,
    pub(crate) renderer_util: Option<u8>,
    pub(crate) tiler_util: Option<u8>,
    pub(crate) paging_rate: u64,
    pub(crate) compression_rate: u64,
    pub(crate) pressure: u8,
    pub(crate) thermal_limited: bool,
    pub(crate) model_memory: Option<u64>,
    pub(crate) model_memory_max: Option<u64>,
    pub(crate) metal_in_use: Option<u64>,
    pub(crate) metal_alloc: Option<u64>,
    pub(crate) active_requests: Option<u64>,
    pub(crate) waiting_requests: Option<u64>,
    /// Running requests reading their prompt, and writing their answer.
    pub(crate) prefilling: u64,
    pub(crate) generating: u64,
    /// The largest prompt among running requests, and among those prefilling.
    pub(crate) largest_prompt: Option<u64>,
    pub(crate) prefill_prompt: Option<u64>,
    pub(crate) at: Option<SystemTime>,
    /// The longest a generating request has gone without a token, in seconds.
    pub(crate) stalled_for: Option<f64>,
    /// Tokens written so far, when one request is generating.
    pub(crate) output_tokens: Option<u64>,
    /// The runtime's storage reads and writes per second, and SSD-cache
    /// blocks it loaded, saved and evicted since the previous sample.
    pub(crate) disk: DiskActivity,
    pub(crate) ssd_full: bool,
}

impl CorrelationObservation {
    /// The runtime's disk work in this sample, in words, when it is enough to
    /// matter: storage traffic above the active rate, or SSD-cache blocks moved.
    pub(crate) fn disk_label(&self) -> Option<String> {
        let mut parts = Vec::new();
        let disk = &self.disk;
        if let Some(read) = disk.read.filter(|read| *read >= DISK_ACTIVE_RATE) {
            parts.push(format!("disk read {}", rate(read)));
        }
        if let Some(write) = disk.write.filter(|write| *write >= DISK_ACTIVE_RATE) {
            parts.push(format!("disk write {}", rate(write)));
        }
        let blocks = |count: u64, verb: &str| {
            format!(
                "{count} SSD cache block{} {verb}",
                if count == 1 { "" } else { "s" }
            )
        };
        if disk.loads > 0 {
            parts.push(blocks(disk.loads, "loaded"));
        }
        if disk.saves > 0 {
            parts.push(blocks(disk.saves, "saved"));
        }
        if parts.is_empty() {
            return None;
        }
        if self.ssd_full && disk.evictions > 0 {
            parts.push("SSD cache full".into());
        }
        Some(parts.join(" · "))
    }
}

fn running(observation: &CorrelationObservation) -> u64 {
    observation.prefilling + observation.generating
}

/// What a slowdown can be explained by. Each is judged by the one rule in
/// `crate::anomaly`, against its own usual value while generation ran at its
/// usual speed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    Thermal,
    Pressure,
    Paging,
    Disk,
    Prefill,
    Requests,
    Prompt,
    Compression,
    Queue,
    ModelMemory,
    MetalMemory,
}

/// Every signal, the host's limits before the workload: when several are
/// unusual at once, the first is named as the reason.
pub(crate) const SIGNALS: [Signal; 11] = [
    Signal::Thermal,
    Signal::Pressure,
    Signal::Paging,
    Signal::Disk,
    Signal::Prefill,
    Signal::Requests,
    Signal::Prompt,
    Signal::Compression,
    Signal::Queue,
    Signal::ModelMemory,
    Signal::MetalMemory,
];

/// When a signal's value matters: the level it must reach at all, and how
/// far above its usual value it must be (a share of that value, and at least
/// a fixed step).
struct Limits {
    from: f64,
    share: f64,
    least: f64,
}

fn usual_rate(usual: f64) -> String {
    if usual < 1.0 {
        "usual none".into()
    } else {
        format!("usual {}", rate(usual as u64))
    }
}

impl Signal {
    pub(crate) fn cause(self) -> CorrelationCause {
        match self {
            Self::Thermal => CorrelationCause::Thermal,
            Self::Pressure => CorrelationCause::MemoryPressure,
            Self::Paging => CorrelationCause::Paging,
            Self::Disk => CorrelationCause::DiskIo,
            Self::Prefill => CorrelationCause::ConcurrentPrefill,
            Self::Requests => CorrelationCause::Concurrency,
            Self::Prompt => CorrelationCause::ContextGrowth,
            Self::Compression => CorrelationCause::Compression,
            Self::Queue => CorrelationCause::Queueing,
            Self::ModelMemory => CorrelationCause::ModelMemory,
            Self::MetalMemory => CorrelationCause::MetalMemory,
        }
    }

    fn value(self, o: &CorrelationObservation) -> Option<f64> {
        match self {
            Self::Thermal => Some(f64::from(u8::from(o.thermal_limited))),
            Self::Pressure => Some(f64::from(o.pressure)),
            Self::Paging => Some(o.paging_rate as f64),
            // Cache blocks moved count as disk work whatever the byte rate.
            Self::Disk => {
                let traffic = o.disk.read.unwrap_or(0).max(o.disk.write.unwrap_or(0));
                Some(if o.disk.loads + o.disk.saves > 0 {
                    traffic.max(DISK_ACTIVE_RATE) as f64
                } else {
                    traffic as f64
                })
            }
            // A prompt being read only slows an answer being written.
            Self::Prefill => Some(if o.generating > 0 {
                o.prefilling as f64
            } else {
                0.0
            }),
            Self::Requests => Some(running(o) as f64),
            Self::Prompt => o.largest_prompt.map(|prompt| prompt as f64),
            Self::Compression => Some(o.compression_rate as f64),
            Self::Queue => o.waiting_requests.map(|waiting| waiting as f64),
            Self::ModelMemory => fraction(o.model_memory, o.model_memory_max),
            Self::MetalMemory => fraction(o.metal_in_use, o.metal_alloc),
        }
    }

    fn limits(self, thresholds: Thresholds) -> Limits {
        let (from, share, least) = match self {
            Self::Thermal | Self::Prefill | Self::Queue => (1.0, 0.0, 1.0),
            Self::Pressure | Self::Requests => (2.0, 0.0, 1.0),
            Self::Paging => (thresholds.swap_warn_rate as f64, 0.5, 0.0),
            Self::Disk => (DISK_ACTIVE_RATE as f64, 0.5, 0.0),
            Self::Prompt => (
                CONTEXT_GROWTH_TOKENS as f64,
                0.5,
                CONTEXT_GROWTH_TOKENS as f64,
            ),
            Self::Compression => (thresholds.compression_warn_rate as f64, 0.5, 0.0),
            Self::ModelMemory | Self::MetalMemory => (0.90, 0.0, 0.02),
        };
        Limits { from, share, least }
    }

    /// The signal now, beside its usual value.
    fn describe(self, o: &CorrelationObservation, usual: f64) -> String {
        match self {
            Self::Thermal => "thermal limit".into(),
            Self::Pressure => format!(
                "memory pressure {}",
                if o.pressure >= 4 { "critical" } else { "watch" }
            ),
            Self::Paging => format!("paging {} ({})", rate(o.paging_rate), usual_rate(usual)),
            Self::Disk => format!(
                "{} ({})",
                o.disk_label().unwrap_or_else(|| "disk work".into()),
                usual_rate(usual)
            ),
            Self::Prefill => match o.prefill_prompt {
                Some(prompt) => format!("{} prefill beside generation", compact_tokens(prompt)),
                None => "prefill beside generation".into(),
            },
            Self::Requests => format!("{} requests running (usual {usual:.0})", running(o)),
            Self::Prompt => format!(
                "prompt {} (usual {})",
                compact_tokens(o.largest_prompt.unwrap_or_default()),
                compact_tokens(usual as u64)
            ),
            Self::Compression => format!(
                "compression {} ({})",
                rate(o.compression_rate),
                usual_rate(usual)
            ),
            Self::Queue => format!(
                "{} waiting (usual {usual:.0})",
                o.waiting_requests.unwrap_or_default()
            ),
            Self::ModelMemory => format!(
                "model MEM {:.0}% of ceiling (usual {:.0}%)",
                fraction(o.model_memory, o.model_memory_max).unwrap_or_default() * 100.0,
                usual * 100.0
            ),
            Self::MetalMemory => format!(
                "Metal MEM {:.0}% (usual {:.0}%)",
                fraction(o.metal_in_use, o.metal_alloc).unwrap_or_default() * 100.0,
                usual * 100.0
            ),
        }
    }
}

/// A signal that is unusual in this sample, in words.
struct Finding {
    signal: Signal,
    text: String,
}

fn seconds_between(from: Option<SystemTime>, to: Option<SystemTime>) -> f64 {
    from.zip(to)
        .and_then(|(from, to)| to.duration_since(from).ok())
        .map_or(0.0, |gap| gap.as_secs_f64())
}

pub(crate) fn threshold_with_hysteresis(
    value: u64,
    was_active: bool,
    enter: u64,
    exit: u64,
) -> bool {
    value >= if was_active { exit } else { enter }
}

pub(crate) fn classify(sample: &mut Sample, previous: Option<&Sample>, thresholds: Thresholds) {
    // Swap always counts; file paging counts while memory is tight.
    let swap_churn = sample.paging_churn();
    let comp_churn = sample.compress.saturating_add(sample.decompress);
    let llm = llm_is_observed(sample);
    let previous_impact = previous.map(|sample| sample.impact.as_str());
    let was_paging = matches!(
        previous_impact,
        Some("PAGING ACTIVE" | "WATCH PAGING" | "HEAVY PAGING" | "SWAP THRASHING")
    );
    let was_compressing = matches!(previous_impact, Some("COMPRESSION ACTIVE"));
    let was_gpu_busy = matches!(previous_impact, Some("GPU BUSY"));
    let swap_critical_rate = thresholds.swap_critical_rate;
    let paging_active = threshold_with_hysteresis(
        swap_churn,
        was_paging,
        PAGING_ACTIVE_ENTER_RATE,
        thresholds.swap_warn_exit,
    );
    let compression_active = threshold_with_hysteresis(
        comp_churn,
        was_compressing,
        thresholds.compression_warn_rate,
        thresholds.compression_warn_exit,
    );
    let watch_paging = swap_churn >= thresholds.swap_warn_rate;
    let swap_thrashing =
        sample.swap_in >= swap_critical_rate && sample.swap_out >= swap_critical_rate;
    // With swap off, a full Mac drops file-backed pages (model weights too)
    // and reads them straight back: that is its heavy paging.
    let file_thrashing = sample.memory_tight() && sample.page_in >= 2 * swap_critical_rate;
    let heavy_paging = sample.swap_out >= 2 * swap_critical_rate
        || sample.swap_growth >= (2 * swap_critical_rate) as i64
        || file_thrashing;
    let page_in_recovery = sample.swap_in >= 2 * swap_critical_rate && sample.swap_growth <= 0;
    let gpu_busy = threshold_with_hysteresis(
        sample.gpu_util.unwrap_or_default() as u64,
        was_gpu_busy,
        thresholds.gpu_warn_load,
        thresholds.gpu_warn_exit,
    ) && llm;
    let gpu_issue = crate::gpu_findings::detect(sample, previous_impact);
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
    } else if let Some(issue) = &gpu_issue {
        (
            issue.impact(),
            Tone::Yellow,
            Some(issue.health()),
            "DEGRADED",
            issue.limiter(),
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
            // Same band as the paging chart: red from the critical rate.
            if swap_churn >= swap_critical_rate {
                Tone::Red
            } else {
                Tone::Yellow
            },
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
        ("GPU BUSY", Tone::Cyan, Some(100), "HEALTHY", "GPU activity")
    } else if llm {
        ("LLM READY", Tone::Green, Some(100), "HEALTHY", "none")
    } else {
        ("IDLE", Tone::Green, Some(100), "HEALTHY", "none")
    };
    sample.impact = impact.into();
    sample.impact_tone = tone;
    sample.gpu_issue = gpu_issue.filter(|issue| issue.impact() == impact);
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
        sample.guidance_cause =
            "Critical pressure — the system cannot reclaim RAM fast enough.".into();
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
    } else if sample.swap_out >= swap_critical_rate
        || sample.swap_growth >= swap_critical_rate as i64
    {
        sample.guidance_badge = "ACT NOW".into();
        sample.guidance_cause =
            format!("RAM overflow — evicting {} to disk.", rate(sample.swap_out));
        sample.guidance_action = if llm {
            "Stop unused models or reduce model/context/cache.".into()
        } else {
            format!("Pause or quit {largest}; wait for swap-out to approach zero.")
        };
    } else if file_thrashing {
        sample.guidance_badge = "ACT NOW".into();
        sample.guidance_cause = format!(
            "RAM full — reading evicted pages back from disk at {}.",
            rate(sample.page_in)
        );
        sample.guidance_action = if llm {
            "Stop unused models or reduce model/context/cache.".into()
        } else {
            format!("Pause or quit {largest}; wait for page-ins to approach zero.")
        };
    } else if let Some(issue) = &sample.gpu_issue {
        let (badge, cause, action) = issue.guidance();
        sample.guidance_badge = badge.into();
        sample.guidance_cause = cause;
        sample.guidance_action = action.into();
    } else if paging_active {
        sample.guidance_badge = "WATCH".into();
        sample.guidance_cause = format!(
            "Paging active — in {} + out {}.",
            rate(sample.paging_in()),
            rate(sample.paging_out())
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
        sample.guidance_cause =
            "GPU busy; utilization alone does not establish a bottleneck.".into();
        sample.guidance_action =
            "Compare throughput at the same prompt size and concurrency.".into();
    } else {
        sample.guidance_badge = "OK".into();
        sample.guidance_cause =
            "No active memory, paging, compression, or thermal bottleneck.".into();
        sample.guidance_action =
            "Nothing to fix; used swap can remain high after pressure passes.".into();
    }
}

pub(crate) fn signal_summary(sample: &Sample) -> String {
    match sample.impact.as_str() {
        "SWAP THRASHING" => format!(
            "in {} / out {}",
            rate(sample.swap_in),
            rate(sample.swap_out)
        ),
        "HEAVY PAGING" if sample.memory_tight() && sample.page_in > 0 => format!(
            "page-in {} · swap {} / growth {}",
            rate(sample.page_in),
            rate(sample.swap_in + sample.swap_out),
            signed_rate(sample.swap_growth)
        ),
        "HEAVY PAGING" => format!(
            "swap {} / growth {}",
            rate(sample.swap_in + sample.swap_out),
            signed_rate(sample.swap_growth)
        ),
        "MEMORY BOTTLENECK" => format!(
            "native pressure critical · LLM RSS {} · RAM resident {}",
            bytes(sample.llm_rss),
            sample
                .resident_memory
                .map(bytes)
                .unwrap_or_else(|| "—".into())
        ),
        "PAGE-IN RECOVERY" => format!(
            "page-in {} · growth {}",
            rate(sample.swap_in),
            signed_rate(sample.swap_growth)
        ),
        "CPU OFFLOAD" | "VRAM FULL" | "GPU THROTTLED" if sample.gpu_issue.is_some() => {
            sample.gpu_issue.as_ref().unwrap().evidence()
        }
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

pub(crate) fn llm_is_observed(sample: &Sample) -> bool {
    sample.llm_count > 0
        || sample.llm_source == TelemetrySource::Live
        || sample.llm_generation_tps.is_some()
        || sample.llm_model != "not detected"
}

#[derive(Default)]
pub(crate) struct CorrelationEngine {
    /// The provider and model the usual values belong to.
    model: Option<(String, String)>,
    /// Generation speed, and every signal in `SIGNALS`, while speed was usual.
    pub(crate) speed: Usual,
    host: [Usual; SIGNALS.len()],
    /// Slow samples not yet enough in a row to be a slowdown.
    pending: Vec<CorrelationObservation>,
    episode: Option<Episode>,
    /// When generation was last seen running.
    seen: Option<SystemTime>,
    /// The reason last journaled, and when it was last seen.
    told: Option<(CorrelationCause, Option<SystemTime>)>,
}

/// A slowdown from its first slow sample. The speed it fell from is fixed
/// here, and nothing is learned while it lasts: a slowdown that is still
/// going on must not become the usual speed by going on.
struct Episode {
    reference: f64,
    began: Option<SystemTime>,
    samples: u32,
    low: f64,
    /// Its latest slow speeds: their middle is the speed it is reported at.
    lows: VecDeque<f64>,
    /// Samples back at the usual speed, in a row.
    back: Vec<CorrelationObservation>,
    /// Reasons already journaled for this slowdown.
    reported: Vec<CorrelationCause>,
    /// Told as part of an earlier slowdown: its end is not journaled either.
    silent: bool,
    /// Its samples since something else was last unusual.
    quiet: Vec<CorrelationObservation>,
}

impl CorrelationEngine {
    pub(crate) fn observe(
        &mut self,
        sample: &Sample,
        thresholds: Thresholds,
    ) -> CorrelationInsight {
        if sample.llm_remote {
            self.reset();
            return CorrelationInsight::default();
        }
        let current = CorrelationObservation::from_sample(sample);
        // Usual values belong to one model on one runtime.
        let model = (current.provider.clone(), current.model.clone());
        if self.model.as_ref() != Some(&model) {
            self.reset();
            self.model = Some(model);
        }
        let stalled = current.stalled_for.filter(|idle| *idle >= STALL_SECONDS);
        let speed = current.generation_tps.filter(|tps| {
            tps.is_finite() && (*tps > 0.0 || current.active_requests.unwrap_or_default() > 0)
        });
        if stalled.is_none() && speed.is_none() {
            // Nothing is generating: there is no speed to judge. A slowdown
            // waits for its next sample.
            return CorrelationInsight::default();
        }
        // A slowdown whose request went idle long ago is over.
        if seconds_between(self.seen, current.at) >= SETTLE_SECONDS {
            self.keep_pending();
            self.episode = None;
        }
        self.seen = current.at;
        // A request that stops producing tokens is observed, not inferred.
        if let Some(idle) = stalled {
            return self.stall(&current, idle, thresholds);
        }
        let speed = speed.unwrap_or_default();
        // A request that has just gone quiet may be about to stall: its
        // near-zero speed is neither slow nor usual until that is known.
        let quiet = current.stalled_for.is_some_and(|idle| idle >= 0.5);
        let usual = self.speed.level();
        let low = self
            .speed
            .below(speed, SLOW_SHARE, SLOW_MIN_TPS)
            .filter(|_| !quiet);
        let steady = |usual: Option<f64>| CorrelationInsight {
            direction: if usual.is_some() {
                ThroughputDirection::Flat
            } else {
                ThroughputDirection::Unknown
            },
            delta_percent: usual
                .filter(|usual| *usual > 0.0)
                .map(|usual| (speed - usual) / usual * 100.0),
            ..CorrelationInsight::default()
        };

        let mut open = match (self.episode.take(), low) {
            (Some(mut open), _) => {
                open.samples += 1;
                if low.is_some() {
                    open.back.clear();
                    open.low = open.low.min(speed);
                    open.lows.push_back(speed);
                } else if !quiet {
                    open.back.push(current.clone());
                }
                open
            }
            (None, Some(reference)) => {
                self.pending.push(current.clone());
                if self.pending.len() < SLOW_SAMPLES {
                    return steady(usual);
                }
                let speeds: Vec<f64> = self
                    .pending
                    .iter()
                    .filter_map(|o| o.generation_tps)
                    .collect();
                let began = self.pending[0].at;
                let samples = self.pending.len() as u32;
                self.pending.clear();
                Episode {
                    reference,
                    began,
                    samples,
                    low: speeds.iter().copied().fold(speed, f64::min),
                    lows: speeds.into(),
                    back: Vec::new(),
                    reported: Vec::new(),
                    silent: false,
                    quiet: Vec::new(),
                }
            }
            (None, None) => {
                // An ordinary sample; a slow one or two before it were too.
                if !quiet {
                    self.keep_pending();
                    self.learn(&current);
                }
                return steady(usual);
            }
        };
        while open.lows.len() > SLOW_SAMPLES {
            open.lows.pop_front();
        }
        let seconds = seconds_between(open.began, current.at);
        let ended = |settled: bool, open: &Episode| {
            (!open.silent).then_some(DropEnd {
                settled,
                seconds,
                low: open.low,
                rate: speed,
                reference: open.reference,
            })
        };

        // Back at its usual speed for three samples: it recovered.
        if open.back.len() >= SLOW_SAMPLES {
            for observation in &open.back {
                self.learn(observation);
            }
            return CorrelationInsight {
                ended: ended(false, &open),
                ..steady(usual)
            };
        }

        let findings = self.findings(&current, thresholds);
        // A longer prompt decodes more slowly for as long as it runs: that
        // is the workload, and like no reason at all it becomes usual.
        if findings.iter().any(|found| found.signal != Signal::Prompt) {
            open.quiet.clear();
        } else {
            open.quiet.push(current.clone());
            let extra = open.quiet.len().saturating_sub(WINDOW);
            open.quiet.drain(..extra);
        }
        if seconds_between(open.quiet.first().and_then(|o| o.at), current.at) >= SETTLE_SECONDS {
            // Nothing else is unusual and it has held: this is how fast
            // generation now runs.
            self.speed.relearn(std::iter::empty());
            for usual in &mut self.host {
                usual.relearn(std::iter::empty());
            }
            for observation in &open.quiet {
                self.learn(observation);
            }
            return CorrelationInsight {
                ended: ended(true, &open),
                ..steady(self.speed.level())
            };
        }

        let shown = median(open.lows.iter().copied().collect());
        let percent = (shown - open.reference) / open.reference * 100.0;
        let rate_label = format!(
            "GEN ↓{:.1}% ({:.1}→{shown:.1} tok/s)",
            percent.abs(),
            open.reference
        );
        let cause = findings
            .first()
            .map(|found| found.signal.cause())
            .unwrap_or_default();
        let texts: Vec<&str> = findings.iter().map(|found| found.text.as_str()).collect();
        // One Journal row per reason in a slowdown, and none for a slowdown
        // that repeats the last one's reason within a minute.
        let same_story = self.told.is_some_and(|(told, at)| {
            told == cause && seconds_between(at, current.at) < SAME_STORY_SECONDS
        });
        if open.reported.is_empty() && same_story {
            open.reported.push(cause);
            open.silent = true;
        }
        // What was unusual has passed while speed is still down: that is
        // the tail of the same slowdown, not a new finding.
        let faded = findings.is_empty() && !open.reported.is_empty();
        let repeat = faded || open.reported.contains(&cause);
        if !repeat {
            open.reported.push(cause);
            open.silent = false;
        }
        if !faded {
            self.told = Some((cause, current.at));
        }
        let insight = CorrelationInsight {
            direction: ThroughputDirection::Down,
            cause,
            summary: if texts.is_empty() {
                format!("{rate_label} · nothing else unusual")
            } else {
                format!(
                    "{rate_label} · unusual: {}",
                    texts
                        .iter()
                        .take(2)
                        .copied()
                        .collect::<Vec<_>>()
                        .join(" + ")
                )
            },
            details: if texts.is_empty() {
                String::new()
            } else {
                format!("{} · {}", cause.label(), texts.join(" · "))
            },
            lead: texts
                .first()
                .map(|text| (*text).to_owned())
                .unwrap_or_default(),
            stall_seconds: None,
            event_key: Some(CorrelationKey {
                direction: ThroughputDirection::Down,
                cause,
            }),
            delta_percent: Some(percent),
            episode: Some(DropEpisode {
                reference: open.reference,
                low: open.low,
                seconds,
                samples: open.samples,
            }),
            ended: None,
            repeat,
            slow: low.is_some(),
            unusual: findings.iter().map(|found| found.signal.cause()).collect(),
        };
        self.episode = Some(open);
        insight
    }

    /// The signals that are unusual in this sample, the most telling first.
    fn findings(&self, current: &CorrelationObservation, thresholds: Thresholds) -> Vec<Finding> {
        let mut findings: Vec<Finding> = SIGNALS
            .iter()
            .zip(&self.host)
            .filter_map(|(signal, usual)| {
                let value = signal.value(current)?;
                let limits = signal.limits(thresholds);
                let usual = usual
                    .above(value, limits.share, limits.least)
                    .filter(|_| value >= limits.from)?;
                Some(Finding {
                    signal: *signal,
                    text: signal.describe(current, usual),
                })
            })
            .collect();
        // A prompt prefilling beside an answer is the request that joined:
        // it is said once, in the more telling form.
        if findings.iter().any(|found| found.signal == Signal::Prefill) {
            findings.retain(|found| found.signal != Signal::Requests);
        }
        findings
    }

    /// A sample at the usual speed: what every signal is doing is usual too.
    fn learn(&mut self, observation: &CorrelationObservation) {
        if let Some(speed) = observation.generation_tps {
            self.speed.learn(speed);
        }
        for (signal, usual) in SIGNALS.iter().zip(&mut self.host) {
            if let Some(value) = signal.value(observation) {
                usual.learn(value);
            }
        }
    }

    /// Slow samples that did not become a slowdown were ordinary after all.
    fn keep_pending(&mut self) {
        for observation in std::mem::take(&mut self.pending) {
            self.learn(&observation);
        }
    }

    /// A request that produced no token for `idle` seconds, with what the
    /// host was doing while it waited.
    fn stall(
        &mut self,
        current: &CorrelationObservation,
        idle: f64,
        thresholds: Thresholds,
    ) -> CorrelationInsight {
        let mut evidence = match current.output_tokens {
            Some(out) => format!("no new token for {idle:.0}s at {} out", compact_tokens(out)),
            None => format!("no new token for {idle:.0}s"),
        };
        let gpu = [current.gpu_util, current.renderer_util, current.tiler_util]
            .into_iter()
            .flatten()
            .max();
        let mut with = Vec::new();
        match gpu {
            Some(gpu) if gpu < 10 => with.push("GPU idle".to_owned()),
            Some(gpu) => with.push(format!("GPU {gpu}%")),
            None => {}
        }
        if let Some(disk) = current.disk_label() {
            with.push(disk);
        }
        if current.paging_rate >= PAGING_ACTIVE_ENTER_RATE {
            with.push(format!("paging {}", rate(current.paging_rate)));
        }
        if current.compression_rate >= thresholds.compression_warn_rate {
            with.push(format!("compression {}", rate(current.compression_rate)));
        }
        if current.pressure >= 2 {
            with.push("memory pressure".into());
        }
        if running(current) > 1 {
            with.push(format!("{} requests running", running(current)));
        }
        if !with.is_empty() {
            evidence = format!("{evidence} · {}", with.join(" · "));
        }
        let seconds = self
            .episode
            .as_ref()
            .map_or(0.0, |open| seconds_between(open.began, current.at));
        let episode = self.episode.as_mut().map(|open| {
            open.samples += 1;
            DropEpisode {
                reference: open.reference,
                low: open.low,
                seconds,
                samples: open.samples,
            }
        });
        CorrelationInsight {
            direction: ThroughputDirection::Down,
            cause: CorrelationCause::Stall,
            summary: format!("GEN stalled · {evidence}"),
            details: format!("{} · {evidence}", CorrelationCause::Stall.label()),
            lead: evidence,
            stall_seconds: Some(idle),
            event_key: Some(CorrelationKey {
                direction: ThroughputDirection::Down,
                cause: CorrelationCause::Stall,
            }),
            delta_percent: None,
            episode,
            ended: None,
            repeat: false,
            slow: true,
            unusual: Vec::new(),
        }
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

impl CorrelationObservation {
    pub(crate) fn from_sample(sample: &Sample) -> Self {
        // Runtimes report prefill progress only while a prompt is being read.
        let running: Vec<_> = sample
            .llm_requests
            .iter()
            .filter(|r| !r.completed && !r.queued)
            .collect();
        let prefilling: Vec<_> = running.iter().filter(|r| r.prefilled.is_some()).collect();
        let generating: Vec<_> = running.iter().filter(|r| r.prefilled.is_none()).collect();
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
            paging_rate: sample.paging_churn(),
            compression_rate: sample.compress.saturating_add(sample.decompress),
            pressure: pressure_rank(&sample.pressure),
            thermal_limited: sample.thermal.starts_with("limited")
                || sample.thermal == "warning reported",
            model_memory: sample.llm_model_memory,
            model_memory_max: sample.llm_model_memory_max,
            metal_in_use: sample.gpu_in_use,
            metal_alloc: sample.gpu_alloc,
            active_requests: sample.llm_active_requests,
            waiting_requests: sample.llm_waiting_requests,
            prefilling: prefilling.len() as u64,
            generating: (running.len() - prefilling.len()) as u64,
            largest_prompt: running
                .iter()
                .map(|r| r.prompt)
                .max()
                .or(sample.llm_prompt_tokens),
            prefill_prompt: prefilling.iter().map(|r| r.prompt).max(),
            at: sample.llm_observed_at.or_else(|| Some(SystemTime::now())),
            stalled_for: running
                .iter()
                .filter(|r| r.prefilled.is_none())
                .filter_map(|r| r.idle_for)
                .map(|idle| idle.as_secs_f64())
                .reduce(f64::max),
            output_tokens: match generating.as_slice() {
                [only] => only.output,
                _ => None,
            },
            disk: sample.disk,
            ssd_full: sample.ssd_cache.is_some_and(|cache| cache.full()),
        }
    }
}

pub(crate) fn pressure_rank(pressure: &str) -> u8 {
    match pressure {
        "GREEN" => 1,
        "YELLOW" => 2,
        "RED" => 4,
        _ => 0,
    }
}

pub(crate) fn fraction(numerator: Option<u64>, denominator: Option<u64>) -> Option<f64> {
    let denominator = denominator.filter(|value| *value > 0)?;
    let numerator = numerator?;
    Some(numerator as f64 / denominator as f64)
}

pub(crate) fn llm_context_tokens(sample: &Sample) -> Option<u64> {
    match (sample.llm_prompt_tokens, sample.llm_output_tokens) {
        (Some(prompt), Some(output)) => Some(prompt.saturating_add(output)),
        (Some(prompt), None) => Some(prompt),
        // Output alone (including summed llama-server slots) is not context.
        (None, _) => None,
    }
}
