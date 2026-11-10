// SPDX-License-Identifier: MIT
//! Health classification and measured throughput correlation; no terminal rendering.
use crate::config::{Thresholds, PAGING_ACTIVE_ENTER_RATE};
use crate::domain::{
    CorrelationCause, CorrelationInsight, CorrelationKey, DiskActivity, DropEnd, DropEpisode,
    Sample, TelemetrySource, ThroughputDirection, Tone, MIB,
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
/// A generating request that goes this long without a token has stalled:
/// at 20–90 tok/s a healthy decode produces one every 50 ms or so.
pub(crate) const STALL_SECONDS: f64 = 1.5;
/// Speculative decoding makes one-second speeds jump by a third between
/// polls; slowdowns are judged on the mean of this many samples.
const SMOOTHING_SAMPLES: usize = 3;
/// Runtime storage traffic worth naming. A decoding runtime touches its disk
/// only to load or save prompt cache; weights are already in memory.
pub(crate) const DISK_ACTIVE_RATE: u64 = 16 * MIB;
/// A slowdown nothing on the host explains becomes the request's normal
/// speed after this long: content alone moves a request's speed by a third.
pub(crate) const SETTLE_SECONDS: f64 = 30.0;
/// A request this far below earlier requests of its size is slow from its
/// start, when the host also shows why. Content alone never differs so much.
const SLOW_START_RATIO: f64 = 0.6;
/// Samples at normal speed, just before a drop, that a condition is
/// compared with: one unchanged from then does not explain the drop.
const BEFORE_SAMPLES: usize = 5;
/// Finished requests remembered as the normal speed for their prompt size.
const NORM_LIMIT: usize = 16;

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
    /// The longest a generating request has gone without a token, in seconds.
    pub(crate) stalled_for: Option<f64>,
    /// The one request generating, when only one is: a request is compared
    /// with its own earlier speed before any other.
    pub(crate) request: Option<String>,
    pub(crate) output_tokens: Option<u64>,
    /// The runtime's storage reads and writes per second, and SSD-cache
    /// blocks it loaded, saved and evicted since the previous sample.
    pub(crate) disk: DiskActivity,
    pub(crate) ssd_full: bool,
    /// Taken during a slowdown or stall: not the speed to compare with.
    pub(crate) slow: bool,
}

/// What the host was doing while generation ran at its normal speed, just
/// before a drop. A condition that held then, at the same level, was there
/// when speed was fine: it is not what changed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Levels {
    pub(crate) paging: u64,
    pub(crate) compression: u64,
    pub(crate) pressure: u8,
    pub(crate) thermal: bool,
    pub(crate) waiting: u64,
    pub(crate) model: f64,
    pub(crate) disk: bool,
    /// Requests running, and whether a prompt prefilled beside an answer.
    pub(crate) running: u64,
    pub(crate) prefill_beside: bool,
}

/// The middle of a few values; the upper one of an even count.
fn middle<T: Ord + Copy>(mut values: Vec<T>) -> T {
    values.sort_unstable();
    values[values.len() / 2]
}

/// The host's levels over the samples just before `onset`, when there are any.
fn levels_before(all: &[&CorrelationObservation], onset: usize) -> Option<Levels> {
    // Only samples where generation ran at its normal speed count: what
    // the host did while nothing was generating says nothing about speed.
    let before: Vec<&CorrelationObservation> = all[..onset]
        .iter()
        .rev()
        .filter(|o| !o.slow && o.generation_tps.is_some_and(|tps| tps > 0.0))
        .take(BEFORE_SAMPLES)
        .copied()
        .collect();
    if before.is_empty() {
        return None;
    }
    let most = |held: &dyn Fn(&CorrelationObservation) -> bool| {
        before.iter().filter(|o| held(o)).count() * 2 > before.len()
    };
    Some(Levels {
        paging: middle(before.iter().map(|o| o.paging_rate).collect()),
        compression: middle(before.iter().map(|o| o.compression_rate).collect()),
        pressure: middle(before.iter().map(|o| o.pressure).collect()),
        thermal: most(&|o| o.thermal_limited),
        waiting: middle(
            before
                .iter()
                .map(|o| o.waiting_requests.unwrap_or(0))
                .collect(),
        ),
        model: before
            .iter()
            .filter_map(|o| fraction(o.model_memory, o.model_memory_max))
            .fold(0.0, f64::max),
        disk: most(&|o| o.disk_label().is_some()),
        running: middle(before.iter().map(|o| running(o)).collect()),
        prefill_beside: most(&|o| o.prefilling > 0 && o.generating > 0),
    })
}

/// A rate now against the same rate before the drop: `None` when it was
/// below `floor` then (it is new), otherwise whether it has grown by half.
fn grew(now: u64, was: u64, floor: u64) -> Option<bool> {
    (was >= floor).then(|| now >= was.saturating_add(was / 2))
}

/// Marks evidence that held, unchanged, while speed was still normal.
const SAME_BEFORE: &str = "same before the drop";

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
    if previous.disk_label().is_none() {
        if let Some(disk) = current.disk_label() {
            push(CorrelationCause::DiskIo, format!("{disk} began"));
        }
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
        CorrelationCause::DiskIo => now.disk_label().is_some(),
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
    /// The slowdown in progress, if any.
    episode: Option<Episode>,
    /// The request whose slowdown last ended, and the speed it returned to.
    normal: Option<(String, f64)>,
    /// Finished requests: their prompt size and normal speed.
    norms: Vec<Norm>,
    stalled: bool,
}

/// A slowdown from its first slow sample. The speed it fell from is fixed
/// here: a rolling baseline would absorb the slow samples within seconds
/// and call a slowdown that is still going on normal.
struct Episode {
    request: Option<String>,
    /// Requests generating when it began: several are judged on their sum.
    generating: u64,
    reference: f64,
    began: Option<SystemTime>,
    samples: u32,
    low: f64,
    /// The change that began the drop, and the sample it was seen in: kept
    /// after it leaves the history, for as long as it still holds.
    lead: Option<(CorrelationFactor, CorrelationObservation)>,
    before: Option<Levels>,
    /// Causes already journaled for this slowdown.
    reported: Vec<CorrelationCause>,
    /// Since when nothing on the host has explained it.
    unexplained: Option<SystemTime>,
    /// Measured against earlier requests rather than this one's own speed.
    slow_start: bool,
}

struct Norm {
    provider: String,
    model: String,
    prompt: u64,
    tps: f64,
}

/// What the analysis of one sample found beyond the insight it reports.
pub(crate) struct Trace {
    /// Samples since the drop's first slow one; zero when this is it.
    onset_ago: usize,
    onset_at: Option<SystemTime>,
    before: Option<Levels>,
    /// The strongest factor, when it is a change that began the drop.
    onset_lead: Option<CorrelationFactor>,
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
        CorrelationCause::Stall | CorrelationCause::DiskIo => sample.correlation.lead.clone(),
        CorrelationCause::None => String::new(),
        CorrelationCause::Paging | CorrelationCause::Compression => String::new(),
    }
}

/// A lone request's measured speeds at its normal pace. Its first poll
/// carries the runtime's average, not a measured speed; stalls and samples
/// inside a slowdown are not what it is compared with.
fn healthy(comparable: &[&CorrelationObservation], id: &str) -> Vec<f64> {
    comparable
        .iter()
        .filter(|o| o.request.as_deref() == Some(id))
        .skip(1)
        .filter(|o| !o.slow && o.stalled_for.is_none_or(|idle| idle < STALL_SECONDS))
        .filter_map(|o| o.generation_tps)
        .collect()
}

/// The middle speed of one provider and model's samples at a normal pace.
pub(crate) fn rolling_baseline<'a>(
    observations: impl Iterator<Item = &'a CorrelationObservation>,
    provider: &str,
    model: &str,
) -> Option<f64> {
    let rates: Vec<f64> = observations
        .filter(|o| o.provider == provider && o.model == model && !o.slow)
        .filter_map(|o| o.generation_tps)
        .filter(|value| value.is_finite() && *value > 0.0)
        .collect();
    (!rates.is_empty()).then(|| median(rates))
}

/// A slowdown is pinned on the host from its first sample only for what
/// the host itself does to a decode; workload signals need an onset.
fn host_explained(insight: &CorrelationInsight) -> bool {
    insight.confidence >= 60
        && matches!(
            insight.cause,
            CorrelationCause::Paging
                | CorrelationCause::MemoryPressure
                | CorrelationCause::Thermal
                | CorrelationCause::Compression
                | CorrelationCause::DiskIo
                | CorrelationCause::ModelMemory
        )
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
        let mut current = CorrelationObservation::from_sample(sample);
        let Self {
            observations,
            episode,
            normal,
            norms,
            stalled,
        } = self;
        // The run of samples from this provider and model up to now: a
        // change of either starts a new history.
        let comparable: Vec<&CorrelationObservation> = observations
            .iter()
            .rev()
            .take_while(|o| o.provider == current.provider && o.model == current.model)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        // A lone request that has just ended is the normal speed for its
        // prompt size, which later requests of that size are held to.
        if let Some(ended) = comparable
            .last()
            .and_then(|o| o.request.as_ref())
            .filter(|id| Some(*id) != current.request.as_ref())
        {
            let speeds = healthy(&comparable, ended);
            let prompt = comparable
                .iter()
                .rev()
                .find(|o| o.request.as_ref() == Some(ended))
                .and_then(|o| o.largest_prompt);
            if let (3.., Some(prompt)) = (speeds.len(), prompt) {
                norms.push(Norm {
                    provider: current.provider.clone(),
                    model: current.model.clone(),
                    prompt,
                    tps: median(speeds),
                });
                if norms.len() > NORM_LIMIT {
                    norms.remove(0);
                }
            }
        }
        // A slowdown belongs to one request (or to several at once) on one
        // model: when that changes it is over, with nothing to compare.
        if episode.as_ref().is_some_and(|open| {
            open.request != current.request
                || open.generating != current.generating
                || comparable.is_empty()
        }) {
            *episode = None;
        }
        // One request is measured against its own earlier speed: requests
        // differ by a third with content alone.
        let own = current
            .request
            .as_deref()
            .map(|id| healthy(&comparable, id))
            .unwrap_or_default();
        // Until a lone request has three measured speeds there is nothing
        // like-for-like to compare with: other requests' speeds differ by
        // content, and a request boundary is not a slowdown.
        let noise = (own.len() >= 3).then(|| spread(&own));
        let own_baseline = match (&current.request, own.len()) {
            (Some(_), 3..) => Some(median(own.clone())),
            // After a long slowdown none of its normal samples are left:
            // the speed it returned to stands in.
            (Some(id), _) => normal
                .as_ref()
                .filter(|(request, _)| request == id)
                .map(|(_, tps)| *tps),
            (None, _) => rolling_baseline(
                comparable.iter().copied(),
                &current.provider,
                &current.model,
            ),
        };
        // Judge the recent mean, not one jumpy poll.
        let judge = |baseline: Option<f64>, hold_quiet: bool| {
            let mut judged = current.clone();
            if current.request.is_some() {
                let recent: Vec<f64> = comparable
                    .iter()
                    .rev()
                    .take_while(|o| o.request == current.request)
                    // A stall was reported as one; its zeros are not the speed
                    // the request returns to.
                    .filter(|o| o.stalled_for.is_none_or(|idle| idle < 0.5))
                    .take(SMOOTHING_SAMPLES - 1)
                    .filter_map(|o| o.generation_tps)
                    .chain(current.generation_tps)
                    .collect();
                if !recent.is_empty() {
                    let mean = recent.iter().sum::<f64>() / recent.len() as f64;
                    // A dip inside the request's own jitter is not a slowdown:
                    // its mean of n polls varies by spread / √n, and a drop must
                    // clear twice that.
                    let jitter = noise.unwrap_or(0.0) * 2.0 / (recent.len() as f64).sqrt();
                    judged.generation_tps = match baseline {
                        Some(baseline) if baseline - mean < jitter => Some(baseline),
                        _ => Some(mean),
                    };
                }
            }
            // A request that has just gone quiet may be about to stall; wait for
            // the stall verdict rather than journaling a slowdown first.
            if hold_quiet
                && current
                    .stalled_for
                    .is_some_and(|idle| (0.5..STALL_SECONDS).contains(&idle))
            {
                judged.generation_tps = baseline;
            }
            judged
        };
        // A request far slower than earlier ones of its size, from its first
        // samples, has no drop of its own to find. It is held to theirs, but
        // only when the host shows a reason: content alone differs too.
        let expected = (episode.is_none() && own.len() >= 3)
            .then(|| expected_speed(norms, &current))
            .flatten()
            .filter(|expected| own_baseline.is_some_and(|own| own <= expected * SLOW_START_RATIO))
            .filter(|expected| {
                let judged = judge(Some(*expected), true);
                let (insight, _) = correlate(
                    &judged,
                    &comparable,
                    Some(*expected),
                    thresholds,
                    None,
                    true,
                );
                insight.direction == ThroughputDirection::Down && host_explained(&insight)
            });
        let (baseline, slow_start) = match (episode.as_ref(), expected) {
            (Some(open), _) => (Some(open.reference), open.slow_start),
            (None, Some(expected)) => (Some(expected), true),
            (None, None) => (own_baseline, false),
        };
        let judged = judge(baseline, episode.is_none());
        let (mut insight, trace) = correlate(
            &judged,
            &comparable,
            baseline,
            thresholds,
            episode.as_ref().and_then(|open| open.before),
            slow_start,
        );
        drop(comparable);

        let stalled_now = insight.stall_seconds.is_some();
        let down = insight.direction == ThroughputDirection::Down;
        let speed = judged.generation_tps.unwrap_or_default();
        // Samples just before this one that were already slow, and whether
        // the request's slow samples become its normal ones.
        let (mut slow_before, mut settle) = (0, false);
        match episode.take() {
            Some(mut open) if down => {
                open.samples += 1;
                // A request going quiet is about to stall, not slowing further.
                if current.stalled_for.is_none_or(|idle| idle < 0.5) {
                    open.low = open.low.min(speed);
                }
                // The change that began the drop explains it for as long as
                // it holds, also once it has left the history.
                match open.lead.take() {
                    Some((lead, then)) if still_holds(lead.cause, &then, &current, thresholds) => {
                        if !stalled_now && insight.confidence <= lead.score {
                            insight.cause = lead.cause;
                            insight.confidence = lead.score;
                            insight.summary =
                                format!("{} · correlated: {}", insight.rate_label(), lead.evidence);
                            insight.lead = lead.evidence.clone();
                            insight.event_key = Some(CorrelationKey {
                                direction: ThroughputDirection::Down,
                                cause: lead.cause,
                            });
                        }
                        open.lead = Some((lead, then));
                    }
                    _ => {
                        open.lead = trace
                            .onset_lead
                            .filter(|_| !stalled_now)
                            .map(|lead| (lead, current.clone()));
                    }
                }
                // One Journal row per cause in a slowdown, however often
                // the strongest one changes back and forth.
                insight.repeat = !stalled_now
                    && (open.reported.contains(&insight.cause)
                        || (insight.lead.is_empty() && !open.reported.is_empty()));
                if !insight.repeat && !stalled_now {
                    open.reported.push(insight.cause);
                }
                // A longer prompt decodes more slowly for as long as it runs:
                // that is the workload, and like no reason at all it settles.
                let settles =
                    insight.lead.is_empty() || insight.cause == CorrelationCause::ContextGrowth;
                open.unexplained = if settles && !stalled_now {
                    open.unexplained.or(current.at)
                } else {
                    None
                };
                let seconds = seconds_between(open.began, current.at);
                if open.unexplained.is_some()
                    && seconds_between(open.unexplained, current.at) >= SETTLE_SECONDS
                {
                    // Nothing on the host changed and it has held: this is
                    // how fast the request now runs.
                    insight = CorrelationInsight {
                        direction: ThroughputDirection::Flat,
                        delta_percent: insight.delta_percent,
                        ended: Some(DropEnd {
                            settled: true,
                            seconds,
                            low: open.low,
                            rate: speed,
                            reference: open.reference,
                        }),
                        ..CorrelationInsight::default()
                    };
                    settle = true;
                    *normal = None;
                } else {
                    insight.episode = Some(DropEpisode {
                        reference: open.reference,
                        low: open.low,
                        seconds,
                        samples: open.samples,
                    });
                    *episode = Some(open);
                }
            }
            Some(open) => {
                // Back within the request's normal range: it recovered.
                if matches!(
                    insight.direction,
                    ThroughputDirection::Flat | ThroughputDirection::Up
                ) {
                    insight.ended = Some(DropEnd {
                        settled: false,
                        seconds: seconds_between(open.began, current.at),
                        low: open.low,
                        rate: current.generation_tps.unwrap_or(speed),
                        reference: open.reference,
                    });
                    if let Some(id) = open.request {
                        *normal = Some((id, open.reference));
                    }
                }
            }
            // A stall is its own finding, with its own end.
            None if down && !stalled_now => {
                if let Some(reference) = baseline {
                    slow_before = trace.onset_ago;
                    let began = trace.onset_at.or(current.at);
                    insight.episode = Some(DropEpisode {
                        reference,
                        low: speed,
                        seconds: seconds_between(began, current.at),
                        samples: trace.onset_ago as u32 + 1,
                    });
                    *episode = Some(Episode {
                        request: current.request.clone(),
                        generating: current.generating,
                        reference,
                        began,
                        samples: trace.onset_ago as u32 + 1,
                        low: speed,
                        lead: trace.onset_lead.map(|lead| (lead, current.clone())),
                        before: trace.before,
                        reported: vec![insight.cause],
                        unexplained: (insight.lead.is_empty()
                            || insight.cause == CorrelationCause::ContextGrowth)
                            .then_some(current.at)
                            .flatten(),
                        slow_start,
                    });
                }
            }
            None => {}
        }
        *stalled = stalled_now;

        // Slow samples are not what later ones are compared with.
        current.slow = down && !settle;
        let count = observations.len();
        for observation in observations.iter_mut().skip(count - slow_before.min(count)) {
            observation.slow = true;
        }
        if settle {
            for observation in observations
                .iter_mut()
                .filter(|o| o.request == current.request)
            {
                observation.slow = false;
            }
        }
        observations.push_back(current);
        while observations.len() > CORRELATION_HISTORY_LIMIT {
            observations.pop_front();
        }
        insight
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The normal speed of earlier requests whose prompts were about this size
/// (half to double): the middle of them, once there are two.
fn expected_speed(norms: &[Norm], current: &CorrelationObservation) -> Option<f64> {
    let prompt = current.largest_prompt?;
    let similar: Vec<f64> = norms
        .iter()
        .filter(|norm| norm.provider == current.provider && norm.model == current.model)
        .filter(|norm| norm.prompt >= prompt / 2 && norm.prompt <= prompt.saturating_mul(2))
        .map(|norm| norm.tps)
        .collect();
    (similar.len() >= 2).then(|| median(similar))
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
            stalled_for: running
                .iter()
                .filter(|r| r.prefilled.is_none())
                .filter_map(|r| r.idle_for)
                .map(|idle| idle.as_secs_f64())
                .reduce(f64::max),
            request: match generating.as_slice() {
                [only] => Some(only.id.clone()),
                _ => None,
            },
            output_tokens: match generating.as_slice() {
                [only] => only.output,
                _ => None,
            },
            disk: sample.disk,
            ssd_full: sample.ssd_cache.is_some_and(|cache| cache.full()),
            slow: false,
        }
    }
}

#[derive(Clone)]
pub(crate) struct CorrelationFactor {
    pub(crate) cause: CorrelationCause,
    pub(crate) score: u8,
    pub(crate) evidence: String,
    /// A change that began just before the drop, rather than a condition
    /// that holds during it.
    pub(crate) onset: bool,
}

/// Attribute a throughput change to what the host and the workload are doing
/// now, and above all to what changed just before the slowdown began: a
/// request joining, a prefill starting beside generation, a longer prompt, a
/// pressure or paging onset. Conditions that hold through normal work, such
/// as a busy GPU, rank below a change that lines up with the drop.
/// `before` is what the host was doing ahead of a slowdown that began before
/// this history; `slow_start` says the baseline is earlier requests' speed.
pub(crate) fn correlate(
    current: &CorrelationObservation,
    history: &[&CorrelationObservation],
    baseline: Option<f64>,
    thresholds: Thresholds,
    before: Option<Levels>,
    slow_start: bool,
) -> (CorrelationInsight, Trace) {
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

    // A request that stops producing tokens is observed, not inferred: it
    // leads, with whatever else the host was doing while it waited.
    let stalled = current.stalled_for.filter(|idle| *idle >= STALL_SECONDS);
    let direction = if stalled.is_some() {
        ThroughputDirection::Down
    } else {
        direction
    };
    // Where the drop began: the first of the slow samples that run up to
    // this one. What the host was doing just before is what a condition
    // during the drop is compared with.
    let all: Vec<&CorrelationObservation> = history
        .iter()
        .copied()
        .chain(std::iter::once(current))
        .collect();
    let mut onset = all.len() - 1;
    if direction == ThroughputDirection::Down {
        let baseline = baseline.unwrap_or_default();
        let slow = |o: &CorrelationObservation| {
            o.generation_tps.is_some_and(|tps| {
                tps <= baseline - THROUGHPUT_CHANGE_MIN_TPS
                    && tps <= baseline * (1.0 - THROUGHPUT_CHANGE_RATIO)
            })
        };
        while onset > 0 && slow(all[onset - 1]) {
            onset -= 1;
        }
    }
    let before = (direction == ThroughputDirection::Down)
        .then(|| before.or_else(|| levels_before(&all, onset)))
        .flatten();
    if let Some(idle) = stalled {
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
        push_correlation_factor(&mut factors, CorrelationCause::Stall, 99, evidence);
    }

    // Requests sharing the engine. A prefill beside generation stalls decode
    // while the prompt is read; concurrent answers split the GPU. Neither
    // explains a drop if speed was normal with just as many running.
    let score_held = |score: u8, held: bool| if held { 45 } else { score };
    if current.prefilling > 0 && current.generating > 0 {
        push_correlation_factor(
            &mut factors,
            CorrelationCause::ConcurrentPrefill,
            score_held(85, before.is_some_and(|before| before.prefill_beside)),
            match current.prefill_prompt {
                Some(prompt) => format!("{} prefill beside generation", compact_tokens(prompt)),
                None => "prefill beside generation".into(),
            },
        );
    } else if running(current) > 1 {
        push_correlation_factor(
            &mut factors,
            CorrelationCause::Concurrency,
            score_held(
                70,
                before.is_some_and(|before| before.running >= running(current)),
            ),
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
            factors.push(CorrelationFactor {
                cause: event.cause,
                score,
                evidence: format!("{}{when}", event.text),
                onset: event.index <= onset,
            });
        }
    }

    // A condition that holds during the drop explains it only if it was not
    // there, or was milder, while speed was normal. One that held just the
    // same is not what changed: a serious one stays a possible reason and
    // says so, a mild one drops out of the explanation.
    let mut condition = |cause, score: u8, serious: bool, held: bool, evidence: String| {
        let (score, evidence) = match (held, serious) {
            (false, _) => (score, evidence),
            (true, true) => (64, format!("{evidence} · {SAME_BEFORE}")),
            (true, false) => (45, format!("{evidence} · {SAME_BEFORE}")),
        };
        push_correlation_factor(&mut factors, cause, score, evidence);
    };

    if current.thermal_limited {
        condition(
            CorrelationCause::Thermal,
            95,
            true,
            before.is_some_and(|before| before.thermal),
            "thermal limit".into(),
        );
    }

    // A decoding runtime has no reason to touch its disk; when it does during
    // a drop, it is loading or saving prompt cache instead of producing tokens.
    if direction == ThroughputDirection::Down {
        if let Some(disk) = current.disk_label() {
            condition(
                CorrelationCause::DiskIo,
                76,
                false,
                before.is_some_and(|before| before.disk),
                disk,
            );
        }
    }

    let was_pressure = before.map_or(0, |before| before.pressure);
    match current.pressure {
        4 => condition(
            CorrelationCause::MemoryPressure,
            100,
            true,
            was_pressure >= 4,
            "memory pressure critical".into(),
        ),
        2 => condition(
            CorrelationCause::MemoryPressure,
            72,
            false,
            was_pressure >= 2,
            "memory pressure watch".into(),
        ),
        _ => {}
    }

    if current.paging_rate >= thresholds.swap_warn_rate {
        let critical = current.paging_rate >= thresholds.swap_critical_rate;
        let was = before.map_or(0, |before| before.paging);
        let grown = grew(current.paging_rate, was, thresholds.swap_warn_rate);
        condition(
            CorrelationCause::Paging,
            if critical { 100 } else { 88 },
            critical,
            grown == Some(false),
            match grown {
                Some(true) => format!("paging {} (was {})", rate(current.paging_rate), rate(was)),
                _ => format!("paging {}", rate(current.paging_rate)),
            },
        );
    }

    if current.compression_rate >= thresholds.compression_warn_rate {
        let was = before.map_or(0, |before| before.compression);
        let grown = grew(
            current.compression_rate,
            was,
            thresholds.compression_warn_rate,
        );
        condition(
            CorrelationCause::Compression,
            68,
            false,
            grown == Some(false),
            match grown {
                Some(true) => format!(
                    "compression {} (was {})",
                    rate(current.compression_rate),
                    rate(was)
                ),
                _ => format!("compression {}", rate(current.compression_rate)),
            },
        );
    }

    if let Some(waiting) = current.waiting_requests.filter(|waiting| *waiting > 0) {
        condition(
            CorrelationCause::Queueing,
            if waiting > 1 { 82 } else { 70 },
            false,
            before.is_some_and(|before| before.waiting >= waiting),
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
                // Decode keeps a GPU saturated, and returns it there after
                // any pause: a crossing is supporting evidence at most.
                if crossed { 62 } else { 50 },
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
        // At its ceiling while speed was normal too: not what changed.
        let held = before.is_some_and(|before| before.model >= 0.90);
        model_memory_score = if held { 45 } else { 84 };
        model_memory_evidence.push(if held {
            format!("model MEM {:.0}% of ceiling · {SAME_BEFORE}", ratio * 100.0)
        } else {
            format!("model MEM {:.0}% of ceiling", ratio * 100.0)
        });
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
    // When a condition is both present and seen arriving, its arrival is the
    // evidence: it says when, and carries the condition's own weight.
    for index in 0..factors.len() {
        if factors[index].onset {
            let cause = factors[index].cause;
            factors[index].score = factors
                .iter()
                .filter(|factor| factor.cause == cause && !factor.onset)
                .map(|factor| factor.score)
                .fold(factors[index].score, u8::max);
        }
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
        _ if stalled.is_some() => "GEN stalled".to_owned(),
        // Against earlier requests of this size, not this one's own speed.
        (Some(current), Some(baseline), ThroughputDirection::Down, Some(percent)) if slow_start => {
            format!(
                "GEN ↓{:.1}% vs earlier requests ({baseline:.1}→{current:.1} tok/s)",
                percent.abs()
            )
        }
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
    // A condition that holds through normal decoding (a busy GPU, a full
    // Metal heap) scores below 60. It stays in the details but is never
    // offered as the explanation.
    let evidence = factors
        .iter()
        .filter(|factor| factor.score >= 60)
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

    let insight = CorrelationInsight {
        direction,
        cause,
        confidence,
        summary,
        details,
        lead: factors
            .first()
            .filter(|factor| factor.score >= 60)
            .map(|factor| factor.evidence.clone())
            .unwrap_or_default(),
        stall_seconds: stalled,
        event_key,
        delta_percent,
        episode: None,
        ended: None,
        repeat: false,
    };
    let trace = Trace {
        onset_ago: all.len() - 1 - onset,
        onset_at: all[onset].at,
        before,
        onset_lead: factors
            .into_iter()
            .next()
            .filter(|factor| factor.onset && factor.score >= 60),
    };
    (insight, trace)
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
        onset: false,
    });
}

/// A robust standard deviation: the median absolute deviation, scaled to
/// match a normal spread, so one burst or stall does not inflate it.
fn spread(values: &[f64]) -> f64 {
    let middle = median(values.to_vec());
    median(values.iter().map(|value| (value - middle).abs()).collect()) * 1.4826
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
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
