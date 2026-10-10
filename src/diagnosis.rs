// SPDX-License-Identifier: MIT
//! Separate measured problems from high resource use during normal work.
use crate::analysis::correlation_evidence_label;
use crate::domain::{CorrelationCause, Sample, TelemetrySource, ThroughputDirection, Tone};
use crate::formatting::{count, llm_generation_rate_label, percent_u8, pressure_state_label, rate};
use std::time::{Duration, SystemTime};

pub(super) struct Finding {
    pub title: String,
    pub evidence: String,
    pub context: String,
    pub next: &'static str,
    pub actionable: bool,
    pub tone: Tone,
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
                finding.next = "Remote inference is not correlated with local hardware.";
            }
        }
    }
    finding
}

fn assess_observation(sample: &Sample) -> Finding {
    let paging = if sample.swap_available && sample.vm_available && sample.rate_ready {
        rate(sample.swap_in.saturating_add(sample.swap_out))
    } else {
        "—".into()
    };
    let gpu = if sample.has_nvidia_gpus() {
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
        next: "Waiting for a live throughput baseline.",
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
        finding.next = action;
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
        finding.next = issue.next();
        finding.actionable = true;
        finding.tone = Tone::Yellow;
        return finding;
    }
    if sample.impact == "SAMPLING" || !sample.rate_ready {
        finding.title = "Collecting system baseline".into();
        finding.next = "Wait for the next sample.";
        return finding;
    }
    if sample.impact == "DATA LIMITED" {
        finding.title = "System counters incomplete".into();
        finding.next = "Restore missing counters before tuning.";
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
    let live = fresh && sample.llm_generation_tps_live;
    if live && !sample.llm_remote && sample.correlation.is_material_drop() {
        finding.title = "Generation slowed".into();
        finding.evidence = sample
            .correlation
            .summary
            .split(" · ")
            .next()
            .unwrap_or("")
            .to_owned();
        let evidence = correlation_evidence_label(sample);
        finding.context =
            if sample.correlation.cause == CorrelationCause::Runtime || evidence.is_empty() {
                "No matching system signal".into()
            } else {
                format!(
                    "WITH {evidence} · {} confidence",
                    sample.correlation.confidence_label()
                )
            };
        finding.next = match sample.correlation.cause {
            CorrelationCause::Paging => "Reduce active models; recheck paging.",
            CorrelationCause::Compression => "Check whether paging also rises.",
            CorrelationCause::MemoryPressure
            | CorrelationCause::ModelMemory
            | CorrelationCause::MetalMemory => "Free memory; recheck generation speed.",
            CorrelationCause::Thermal => "Reduce concurrency; recheck thermals.",
            CorrelationCause::Queueing | CorrelationCause::GpuSaturation => {
                "Compare speed with one active request."
            }
            CorrelationCause::ContextGrowth => "Compare a request with a shorter prompt.",
            CorrelationCause::Runtime | CorrelationCause::None => {
                "Compare prompt size and runtime settings."
            }
        };
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
        finding.next = "Reduce concurrency; recheck queue length.";
        finding.actionable = true;
        finding.tone = Tone::Yellow;
        return finding;
    }
    // Lead with the verdict. High GPU use stays evidence, never the headline.
    if pressure_state_label(sample) == "normal" {
        finding.title = "Healthy · no bottleneck".into();
        finding.tone = Tone::Green;
    }
    if live
        && matches!(
            sample.correlation.direction,
            ThroughputDirection::Flat | ThroughputDirection::Up
        )
    {
        finding.next = "No generation slowdown measured.";
    } else if !live && matches!(sample.llm_status.as_str(), "idle" | "ready") {
        finding.next = "Idle · waiting for the next request.";
    } else if !live {
        finding.next = "Live generation rate unavailable.";
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
