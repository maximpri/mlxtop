// SPDX-License-Identifier: MIT
//! Health classification and measured throughput correlation; no terminal rendering.
use crate::config::{Thresholds, PAGING_ACTIVE_ENTER_RATE};
use crate::domain::{
    CorrelationCause, CorrelationInsight, CorrelationKey, Sample, TelemetrySource,
    ThroughputDirection, Tone, MIB,
};
use crate::formatting::{
    bytes, compact_tokens, count, llm_generation_rate_label, llm_prefill_rate_label, percent,
    pressure_state_label, rate, signed_rate,
};
use std::collections::VecDeque;
use std::time::SystemTime;
pub(crate) const CORRELATION_HISTORY_LIMIT: usize = 16;
pub(crate) const THROUGHPUT_CHANGE_MIN_TPS: f64 = 2.0;
pub(crate) const THROUGHPUT_CHANGE_RATIO: f64 = 0.10;
pub(crate) const CONTEXT_GROWTH_TOKENS: u64 = 1024;
pub(crate) const MODEL_MEMORY_GROWTH: u64 = 256 * MIB;

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
    pub(crate) context_tokens: Option<u64>,
    pub(crate) active_requests: Option<u64>,
    pub(crate) waiting_requests: Option<u64>,
    /// Running requests reading their prompt, and writing their answer.
    pub(crate) prefilling: u64,
    pub(crate) generating: u64,
    /// The largest prompt among running requests, and among those prefilling.
    pub(crate) largest_prompt: Option<u64>,
    pub(crate) prefill_prompt: Option<u64>,
    pub(crate) at: Option<SystemTime>,
}

/// Lookback before a drop's first slow sample in which a change is a candidate
/// cause: samples are taken every second or two, and a runtime's rate lags.
const EVENT_LOOKBACK: usize = 3;

/// Something that changed between two samples, which a slowdown that starts
/// soon after can be attributed to.
struct Event {
    cause: CorrelationCause,
    text: String,
    /// Position in the history, oldest first; the current sample is last.
    index: usize,
    at: Option<SystemTime>,
}

fn running(observation: &CorrelationObservation) -> u64 {
    observation.prefilling + observation.generating
}

/// The changes from one sample to the next that can slow generation, most
/// specific first: a prompt starting to prefill beside a running answer
/// explains a request joining, so it is reported instead.
fn events_between(
    previous: &CorrelationObservation,
    current: &CorrelationObservation,
    index: usize,
    thresholds: Thresholds,
) -> Vec<Event> {
    let mut events = Vec::new();
    let mut push = |cause, text: String| {
        events.push(Event {
            cause,
            text,
            index,
            at: current.at,
        })
    };
    let beside_generation = current.generating > 0 || previous.generating > 0;
    if current.prefilling > previous.prefilling && beside_generation {
        push(
            CorrelationCause::ConcurrentPrefill,
            match current.prefill_prompt {
                Some(prompt) => {
                    format!("{} prefill began beside generation", compact_tokens(prompt))
                }
                None => "a prefill began beside generation".into(),
            },
        );
    } else if running(current) > running(previous) && running(current) > 1 {
        push(
            CorrelationCause::Concurrency,
            format!(
                "request joined ({} → {} running)",
                running(previous),
                running(current)
            ),
        );
    }
    if let (Some(before), Some(after)) = (previous.largest_prompt, current.largest_prompt) {
        if after >= before.saturating_add(CONTEXT_GROWTH_TOKENS) && after >= before * 3 / 2 {
            push(
                CorrelationCause::ContextGrowth,
                format!(
                    "prompt {} → {}",
                    compact_tokens(before),
                    compact_tokens(after)
                ),
            );
        }
    }
    if current.pressure > previous.pressure && current.pressure >= 2 {
        push(
            CorrelationCause::MemoryPressure,
            format!(
                "memory pressure rose to {}",
                if current.pressure >= 4 {
                    "critical"
                } else {
                    "watch"
                }
            ),
        );
    }
    if current.paging_rate >= thresholds.swap_warn_rate
        && previous.paging_rate < thresholds.swap_warn_rate
    {
        push(
            CorrelationCause::Paging,
            format!("paging began ({})", rate(current.paging_rate)),
        );
    }
    if current.thermal_limited && !previous.thermal_limited {
        push(CorrelationCause::Thermal, "thermal limiting began".into());
    }
    let metal = |o: &CorrelationObservation| fraction(o.metal_in_use, o.metal_alloc);
    if metal(current).is_some_and(|r| r >= 0.90) && metal(previous).is_none_or(|r| r < 0.90) {
        push(
            CorrelationCause::MetalMemory,
            format!(
                "Metal memory crossed {:.0}%",
                metal(current).unwrap_or(0.0) * 100.0
            ),
        );
    }
    events
}

/// Whether the change an event marks is still in effect: a prefill that has
/// finished or pressure that has eased no longer explains a slow sample.
fn still_holds(
    cause: CorrelationCause,
    then: &CorrelationObservation,
    now: &CorrelationObservation,
    thresholds: Thresholds,
) -> bool {
    match cause {
        CorrelationCause::ConcurrentPrefill => now.prefilling > 0 && now.generating > 0,
        CorrelationCause::Concurrency => running(now) > 1,
        CorrelationCause::ContextGrowth => now.largest_prompt >= then.largest_prompt,
        CorrelationCause::MemoryPressure => now.pressure >= 2,
        CorrelationCause::Paging => now.paging_rate >= thresholds.swap_warn_rate,
        CorrelationCause::Thermal => now.thermal_limited,
        CorrelationCause::MetalMemory => {
            fraction(now.metal_in_use, now.metal_alloc).is_some_and(|r| r >= 0.90)
        }
        _ => true,
    }
}

/// Seconds between two sample times, in words, when both are known.
fn gap_label(event: Option<SystemTime>, onset: Option<SystemTime>) -> String {
    match event
        .zip(onset)
        .and_then(|(event, onset)| onset.duration_since(event).ok())
    {
        Some(gap) if gap.as_secs() == 0 => " as the drop began".into(),
        Some(gap) => format!(" {}s before the drop", gap.as_secs()),
        None => String::new(),
    }
}

#[derive(Default)]
pub(crate) struct CorrelationEngine {
    pub(crate) observations: VecDeque<CorrelationObservation>,
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

pub(crate) fn correlation_evidence_label(sample: &Sample) -> String {
    match sample.correlation.cause {
        CorrelationCause::Paging if sample.paging_churn() > 0 => {
            format!("I/O {}", rate(sample.paging_churn()))
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
        CorrelationCause::ConcurrentPrefill => sample
            .llm_requests
            .iter()
            .filter(|r| !r.completed && !r.queued && r.prefilled.is_some())
            .map(|r| r.prompt)
            .max()
            .map(|prompt| format!("{} prefill beside generation", compact_tokens(prompt)))
            .unwrap_or_else(|| "prefill beside generation".into()),
        CorrelationCause::Concurrency => sample
            .llm_active_requests
            .map(|active| format!("{active} requests running"))
            .unwrap_or_else(|| "concurrent requests".into()),
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

impl CorrelationEngine {
    pub(crate) fn observe(
        &mut self,
        sample: &Sample,
        thresholds: Thresholds,
    ) -> CorrelationInsight {
        if sample.llm_remote {
            self.observations.clear();
            return CorrelationInsight::default();
        }
        let current = CorrelationObservation::from_sample(sample);
        // The run of samples from this provider and model up to now: a
        // change of either starts a new history.
        let comparable: Vec<&CorrelationObservation> = self
            .observations
            .iter()
            .rev()
            .take_while(|o| o.provider == current.provider && o.model == current.model)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let baseline = self.baseline(&current.provider, &current.model);
        let insight = correlate_history(&current, &comparable, baseline, thresholds);

        self.observations.push_back(current);
        while self.observations.len() > CORRELATION_HISTORY_LIMIT {
            self.observations.pop_front();
        }
        insight
    }

    pub(crate) fn baseline(&self, provider: &str, model: &str) -> Option<f64> {
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

    pub(crate) fn reset(&mut self) {
        self.observations.clear();
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
            // With several requests the runtime reports no one prompt size;
            // the longest running context still slows its own decode.
            context_tokens: llm_context_tokens(sample).or_else(|| {
                running
                    .iter()
                    .map(|r| r.prompt.saturating_add(r.output.unwrap_or(0)))
                    .max()
            }),
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
        }
    }
}

pub(crate) struct CorrelationFactor {
    pub(crate) cause: CorrelationCause,
    pub(crate) score: u8,
    pub(crate) evidence: String,
}

#[cfg(test)]
pub(crate) fn correlate_observations(
    current: &CorrelationObservation,
    previous: Option<&CorrelationObservation>,
    baseline: Option<f64>,
    thresholds: Thresholds,
) -> CorrelationInsight {
    let history: Vec<&CorrelationObservation> = previous.into_iter().collect();
    correlate_history(current, &history, baseline, thresholds)
}

/// Attribute a throughput change to what the host and the workload are doing
/// now, and above all to what changed just before the slowdown began: a
/// request joining, a prefill starting beside generation, a longer prompt, a
/// pressure or paging onset. Conditions that hold through normal work, such
/// as a busy GPU, rank below a change that lines up with the drop.
pub(crate) fn correlate_history(
    current: &CorrelationObservation,
    history: &[&CorrelationObservation],
    baseline: Option<f64>,
    thresholds: Thresholds,
) -> CorrelationInsight {
    let previous = history.last().copied();
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

    // Requests sharing the engine. A prefill beside generation stalls decode
    // while the prompt is read; concurrent answers split the GPU.
    if current.prefilling > 0 && current.generating > 0 {
        push_correlation_factor(
            &mut factors,
            CorrelationCause::ConcurrentPrefill,
            85,
            match current.prefill_prompt {
                Some(prompt) => format!("{} prefill beside generation", compact_tokens(prompt)),
                None => "prefill beside generation".into(),
            },
        );
    } else if running(current) > 1 {
        push_correlation_factor(
            &mut factors,
            CorrelationCause::Concurrency,
            70,
            format!("{} requests running", running(current)),
        );
    }
    // A prompt much longer than the recent ones decodes more slowly.
    let mut recent_prompts: Vec<u64> = history
        .iter()
        .filter_map(|o| o.largest_prompt)
        .filter(|prompt| Some(*prompt) != current.largest_prompt)
        .collect();
    recent_prompts.sort_unstable();
    recent_prompts.dedup();
    if let (Some(prompt), Some(&reference)) = (
        current.largest_prompt,
        recent_prompts.get(recent_prompts.len().saturating_sub(1) / 2),
    ) {
        if direction == ThroughputDirection::Down
            && reference > 0
            && prompt >= reference.saturating_add(CONTEXT_GROWTH_TOKENS)
            && prompt >= reference * 3 / 2
        {
            push_correlation_factor(
                &mut factors,
                CorrelationCause::ContextGrowth,
                80,
                format!(
                    "prompt {} ({:.1}× recent {})",
                    compact_tokens(prompt),
                    prompt as f64 / reference as f64,
                    compact_tokens(reference)
                ),
            );
        }
    }

    // What changed just before the drop began leads the explanation.
    if direction == ThroughputDirection::Down {
        let baseline = baseline.unwrap_or_default();
        let slow = |o: &CorrelationObservation| {
            o.generation_tps.is_some_and(|tps| {
                tps <= baseline - THROUGHPUT_CHANGE_MIN_TPS
                    && tps <= baseline * (1.0 - THROUGHPUT_CHANGE_RATIO)
            })
        };
        let all: Vec<&CorrelationObservation> = history
            .iter()
            .copied()
            .chain(std::iter::once(current))
            .collect();
        let mut onset = all.len() - 1;
        while onset > 0 && slow(all[onset - 1]) {
            onset -= 1;
        }
        let from = onset.saturating_sub(EVENT_LOOKBACK).max(1);
        let events: Vec<Event> = (from..all.len())
            .flat_map(|index| events_between(all[index - 1], all[index], index, thresholds))
            .collect();
        for event in events
            .into_iter()
            .filter(|event| still_holds(event.cause, all[event.index], current, thresholds))
        {
            // Nearest before the onset scores highest; changes during the
            // drop may deepen it but did not start it.
            let score = if event.index <= onset {
                97 - (onset - event.index) as u8 * 4
            } else {
                78
            };
            let when = if event.index <= onset {
                gap_label(event.at, all[onset].at)
            } else {
                " during the drop".into()
            };
            push_correlation_factor(
                &mut factors,
                event.cause,
                score,
                format!("{}{when}", event.text),
            );
        }
    }

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

    if current.paging_rate >= thresholds.swap_warn_rate {
        let score = if current.paging_rate >= thresholds.swap_critical_rate {
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

    if current.compression_rate >= thresholds.compression_warn_rate {
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
        if u64::from(gpu) >= thresholds.gpu_critical_load {
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
                .is_none_or(|value| u64::from(value) < thresholds.gpu_critical_load);
            push_correlation_factor(
                &mut factors,
                CorrelationCause::GpuSaturation,
                // Decode keeps a GPU saturated; only the crossing is news.
                if crossed { 95 } else { 50 },
                format!("GPU {gpu}% busy"),
            );
        } else if u64::from(gpu) >= thresholds.gpu_warn_load {
            push_correlation_factor(
                &mut factors,
                CorrelationCause::GpuSaturation,
                40,
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
            if crossed { 90 } else { 55 },
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
    // One line per cause: the strongest evidence for it, usually its onset.
    let mut seen = Vec::new();
    factors.retain(|factor| {
        let first = !seen.contains(&factor.cause);
        seen.push(factor.cause);
        first
    });

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
        lead: factors
            .first()
            .map(|factor| factor.evidence.clone())
            .unwrap_or_default(),
        event_key,
        delta_percent,
    }
}

pub(crate) fn push_correlation_factor(
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
