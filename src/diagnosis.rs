// SPDX-License-Identifier: MIT
//! Separate measured problems from high resource use during normal work.
use crate::domain::{CorrelationCause, Sample, TelemetrySource, ThroughputDirection, Tone};
use crate::formatting::{
    bytes, count, llm_generation_rate_label, percent_u8, pressure_state_label, rate,
};
use std::time::{Duration, SystemTime};

pub(super) struct Finding {
    pub title: String,
    pub evidence: String,
    pub context: String,
    pub next: String,
    pub actionable: bool,
    pub tone: Tone,
}

/// What to change for a measured slowdown, by its cause, with the readings
/// that make it worth changing: advice a person can act on, not a category.
pub(crate) fn slowdown_next_step(sample: &Sample) -> String {
    let lead = &sample.correlation.lead;
    // What RAM has left, when the OS reports it: ` (4.3 GiB of RAM left)`.
    let headroom = crate::memory_composition::segments(sample)
        .map(|sizes| {
            format!(
                " ({} of RAM left)",
                bytes(crate::memory_headroom::left(sizes))
            )
        })
        .unwrap_or_default();
    let limit = crate::memory_composition::gpu_limit(sample);
    match sample.correlation.cause {
        CorrelationCause::Paging => format!(
            "The model no longer fits in RAM{headroom}: close other apps or models, or use a smaller quant."
        ),
        CorrelationCause::Compression => {
            "macOS is compressing memory to make room: free memory before paging starts.".into()
        }
        CorrelationCause::MemoryPressure => format!(
            "Memory is under pressure{headroom}: close other apps or models, or shorten the context."
        ),
        CorrelationCause::ModelMemory | CorrelationCause::MetalMemory => match limit {
            Some(limit) if sample.wired.saturating_mul(10) >= limit.saturating_mul(9) => format!(
                "Wired memory is {} of the {} GPU limit: raise it (sudo sysctl iogpu.wired_limit_mb=…) or shorten the context.",
                bytes(sample.wired),
                bytes(limit)
            ),
            _ => "The model's memory is near its ceiling: shorten the context or lower concurrency.".into(),
        },
        CorrelationCause::Thermal => match sample.system_power_mw {
            Some(milliwatts) => format!(
                "macOS is limiting speed for heat at {:.0} W: improve airflow, or run fewer requests at once.",
                milliwatts as f64 / 1000.0
            ),
            None => "macOS is limiting speed for heat: improve airflow, or run fewer requests at once.".into(),
        },
        // The engine stopped with nothing on the host to show for it:
        // the reason is inside the runtime, in its log at this time.
        // Any disk work beside a stall is the runtime's own prompt cache.
        CorrelationCause::Stall | CorrelationCause::DiskIo if lead.contains("SSD cache full") => {
            "The SSD prompt cache is evicting to make room as it reads: a faster disk, or a larger cache, keeps prefixes ready.".into()
        }
        CorrelationCause::Stall | CorrelationCause::DiskIo
            if lead.contains("disk ") || lead.contains("SSD cache") =>
        {
            format!("The runtime waited on its disk ({lead}): put its cache on a faster disk.")
        }
        CorrelationCause::DiskIo => format!(
            "The runtime used its disk mid-answer ({lead}): check what it reads, and that disk's speed."
        ),
        CorrelationCause::Stall if lead.contains("GPU idle") && !lead.contains("paging") => {
            "The runtime stopped with the machine idle: the cause is inside it; check its log at this time.".into()
        }
        CorrelationCause::Stall => format!("Generation stopped beside {lead}: start there."),
        CorrelationCause::ConcurrentPrefill => {
            "A prompt is being read beside this answer: expected; it recovers when the prompt is read.".into()
        }
        CorrelationCause::Queueing => format!(
            "{} waiting: each answer slows as more run at once; lower the client's concurrency if latency matters more than throughput.",
            match sample.llm_waiting_requests.filter(|waiting| *waiting > 0) {
                Some(1) => "A request is".to_owned(),
                Some(waiting) => format!("{waiting} requests are"),
                None => "Requests are".to_owned(),
            }
        ),
        CorrelationCause::Cpu => {
            "The runtime is busy on the CPU: check that the model runs on the GPU, and for tokenizer or sampling work.".into()
        }
        CorrelationCause::GpuIdle => {
            "The GPU is waiting on the runtime: check its CPU and disk work beside it.".into()
        }
        CorrelationCause::DraftAcceptance => {
            "The draft model is guessing worse on this text: the machine is not the limit; a better-matched draft, or none, may be faster.".into()
        }
        CorrelationCause::None => {
            "Nothing on the machine explains it: the text, the draft model or the runtime itself; check the runtime's log at this time.".into()
        }
    }
}

pub(super) fn assess(sample: &Sample) -> Finding {
    let mut finding = assess_observation(sample);
    if sample.llm_remote {
        if finding.title.ends_with("requests waiting") {
            finding.title = format!("Remote runtime: {}", finding.title);
            finding.evidence = "Remote request queue".into();
        } else {
            finding.title = format!("Local host: {}", finding.title);
            if !finding.actionable {
                finding.next = "Remote inference is not correlated with local hardware.".into();
            }
        }
    }
    finding
}

fn assess_observation(sample: &Sample) -> Finding {
    let paging = if sample.paging_measured() && sample.vm_available {
        rate(sample.paging_churn())
    } else {
        "—".into()
    };
    let gpu = if sample.has_discrete_gpus() {
        "GPU max"
    } else {
        "GPU"
    };
    let mut finding = Finding {
        title: "No bottleneck established".into(),
        evidence: format!("{gpu} {} · paging {paging}", percent_u8(sample.gpu_util)),
        context: format!(
            "Pressure {} · thermal {}",
            pressure_state_label(sample),
            sample.thermal
        ),
        next: "Waiting for a live throughput baseline.".into(),
        actionable: false,
        tone: Tone::Muted,
    };

    // Host faults remain visible even without live serving telemetry.
    let impact = if sample.pressure == "RED" {
        "MEMORY BOTTLENECK"
    } else {
        sample.impact.as_str()
    };
    let action = match impact {
        "MEMORY BOTTLENECK" | "MEMORY STRESS" => Some("Free memory; stop unused models or apps."),
        "SWAP THRASHING" | "HEAVY PAGING" => Some("Reduce active models; recheck swap-out."),
        "PAGE-IN RECOVERY" => Some("Let page-ins settle before adding work."),
        "PAGING ACTIVE" | "WATCH PAGING" => Some("If paging persists, reduce concurrency."),
        "COMPRESSION ACTIVE" => Some("Check whether paging also rises."),
        "THERMAL LIMIT" => Some("Reduce concurrency; recheck thermals."),
        _ => None,
    };
    if let Some(action) = action {
        finding.title = sentence_case(impact);
        finding.next = action.into();
        finding.actionable = true;
        finding.tone = if sample.pressure == "RED" {
            Tone::Red
        } else {
            sample.impact_tone
        };
        if sample.impact == "COMPRESSION ACTIVE" {
            finding.evidence = format!(
                "Compression {} · paging {paging}",
                rate(sample.compress.saturating_add(sample.decompress))
            );
        }
        return finding;
    }
    if let Some(issue) = &sample.gpu_issue {
        finding.title = issue.title();
        finding.evidence = issue.evidence();
        finding.next = issue.next().into();
        finding.actionable = true;
        finding.tone = Tone::Yellow;
        return finding;
    }
    if sample.impact == "SAMPLING" || !sample.rate_ready {
        finding.title = "Collecting system baseline".into();
        finding.next = "Wait for the next sample.".into();
        return finding;
    }
    if sample.impact == "DATA LIMITED" {
        finding.title = "System counters incomplete".into();
        finding.next = "Restore missing counters before tuning.".into();
        finding.actionable = true;
        return finding;
    }

    // Historical/reported rates must not become a current slowdown diagnosis.
    let fresh = sample.llm_source == TelemetrySource::Live
        && sample.llm_status != "stale"
        && sample.llm_observed_at.is_some_and(|at| {
            SystemTime::now()
                .duration_since(at)
                .is_ok_and(|age| age <= Duration::from_secs(5))
        });
    let prefill = sample.correlation.phase == crate::domain::Phase::Prefill;
    let live = fresh
        && if prefill {
            sample.llm_prefill_tps_live
        } else {
            sample.llm_generation_tps_live
        };
    if live && !sample.llm_remote && sample.correlation.is_material_drop() {
        finding.title = if sample.correlation.cause == CorrelationCause::Stall {
            "Generation stalled".into()
        } else if prefill {
            "Prefill slowed".into()
        } else {
            "Generation slowed".into()
        };
        finding.evidence = sample
            .correlation
            .summary
            .split(" · ")
            .next()
            .unwrap_or("")
            .to_owned();
        // A slowdown that has lasted says for how long.
        if let Some(episode) = sample
            .correlation
            .episode
            .filter(|episode| episode.seconds >= 5.0)
        {
            finding.evidence = format!("{} for {:.0}s", finding.evidence, episode.seconds);
        }
        // The reason is whatever else is unusual at the same time.
        finding.context = if sample.correlation.lead.is_empty() {
            "Nothing else unusual".into()
        } else {
            format!("WITH {}", sample.correlation.lead)
        };
        finding.next = slowdown_next_step(sample);
        finding.actionable = true;
        finding.tone = sample.correlation.tone();
        return finding;
    }
    if live
        && sample
            .llm_waiting_requests
            .is_some_and(|waiting| waiting > 0)
    {
        finding.title = format!("{} requests waiting", count(sample.llm_waiting_requests));
        finding.context = format!(
            "{} active · {}",
            count(sample.llm_active_requests),
            llm_generation_rate_label(sample)
        );
        finding.next = "Reduce concurrency; recheck queue length.".into();
        finding.actionable = true;
        finding.tone = Tone::Yellow;
        return finding;
    }
    // Lead with the verdict. High GPU use stays evidence, never the headline.
    if pressure_state_label(sample) == "normal" {
        finding.title = "Healthy · no bottleneck".into();
        finding.tone = Tone::Green;
    }
    if live && matches!(sample.correlation.direction, ThroughputDirection::Flat) {
        // Nothing to say: an empty note keeps the strip quiet.
        finding.next = "".into();
    } else if sample.llm_status == "untracked" {
        finding.next =
            "GPU busy outside oMLX's request tracking (oMLX benchmark on Splash?) · no live rates."
                .into();
    } else if !live
        && (matches!(sample.llm_status.as_str(), "idle" | "ready")
            // oMLX reports `healthy` once no model has requests left.
            || (fresh
                && sample.llm_status == "healthy"
                && sample.llm_active_requests == Some(0)
                && sample.llm_waiting_requests.unwrap_or(0) == 0))
    {
        // The status badge already says idle.
        finding.next = "".into();
    } else if matches!(
        sample.llm_status.as_str(),
        "prefilling" | "loading" | "waiting" | "restarting" | "paused"
    ) {
        // No generation rate is expected before the first token.
        finding.next = "".into();
    } else if !live {
        finding.next = "Live generation rate unavailable.".into();
    }
    finding
}

/// Findings share one headline slot, so native state names use sentence case.
fn sentence_case(state: &str) -> String {
    let lower = state.to_ascii_lowercase();
    let mut chars = lower.chars();
    chars
        .next()
        .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "tests/diagnosis.rs"]
mod tests;
