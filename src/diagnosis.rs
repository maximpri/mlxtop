// SPDX-License-Identifier: MIT
//! Separate measured problems from high resource use during normal work.
use crate::domain::{CorrelationCause, Sample, ThroughputDirection, Tone};
use crate::formatting::{
    bytes, count, llm_generation_rate_label, percent_u8, pressure_state_label, rate,
};

pub(crate) struct Finding {
    /// What the verdict is, by name; `title` says it in words.
    pub verdict: Verdict,
    pub title: String,
    pub evidence: String,
    pub context: String,
    pub next: String,
    pub actionable: bool,
    pub tone: Tone,
}

/// The verdict by name, for a program to match where a person reads the
/// title. One name per kind of headline; the title can carry more (which
/// GPU, how many requests, whether the runtime is remote).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Verdict {
    /// Nothing limits the work and memory pressure is normal.
    Healthy,
    /// Nothing measured limits the work, but pressure is not reported normal.
    NoBottleneck,
    /// The first sample: rates need a second one.
    CollectingBaseline,
    /// Some of the host's counters could not be read.
    CountersIncomplete,
    GenerationSlowed,
    PrefillSlowed,
    GenerationStalled,
    RequestsWaiting,
    MemoryBottleneck,
    MemoryStress,
    SwapThrashing,
    HeavyPaging,
    PageInRecovery,
    PagingActive,
    WatchPaging,
    CompressionActive,
    ThermalLimit,
    /// Part or all of the model runs on the CPU although a GPU is present.
    CpuOffload,
    VramFull,
    GpuThrottled,
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
    let blamed = |cause| sample.correlation.unusual.contains(&cause);
    // A stall blames the disk only when it ran differently while fast.
    let disk =
        sample.correlation.cause == CorrelationCause::DiskIo || blamed(CorrelationCause::DiskIo);
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
        CorrelationCause::ModelMemory | CorrelationCause::MetalMemory => wired_limit_step(sample)
            .unwrap_or_else(|| {
                "The model's memory is near its ceiling: shorten the context or lower concurrency."
                    .into()
            }),
        // A card holding its clocks down for heat: its cooling, not the Mac's.
        CorrelationCause::Thermal if crate::gpu::heat_limited(&sample.gpus) => {
            crate::gpu_findings::heat_step(sample)
        }
        CorrelationCause::Thermal => match sample.system_power_mw {
            Some(milliwatts) => format!(
                "macOS is limiting speed for heat at {:.0} W: improve airflow, or run fewer requests at once.",
                milliwatts as f64 / 1000.0
            ),
            None => "macOS is limiting speed for heat: improve airflow, or run fewer requests at once.".into(),
        },
        // The engine stopped with nothing on the host running differently
        // from when it was fast: the reason is inside the runtime.
        CorrelationCause::Stall
            if sample.correlation.unusual.is_empty() && lead.contains("GPU idle") =>
        {
            "The runtime paused with the GPU idle and nothing on this machine out of the ordinary: the cause is inside it; check its log at this time.".into()
        }
        CorrelationCause::KvPreemption => KV_FULL.into(),
        CorrelationCause::Stall if blamed(CorrelationCause::KvPreemption) => KV_FULL.into(),
        CorrelationCause::Stall | CorrelationCause::DiskIo if disk && lead.contains("SSD cache full") => {
            "The SSD prompt cache is evicting to make room as it reads: a faster disk, or a larger cache, keeps prefixes ready.".into()
        }
        CorrelationCause::Stall | CorrelationCause::DiskIo
            if disk && (lead.contains("disk ") || lead.contains("SSD cache")) =>
        {
            format!("The runtime waited on its disk ({lead}): put its cache on a faster disk.")
        }
        CorrelationCause::DiskIo => format!(
            "The runtime used its disk mid-answer ({lead}): check what it reads, and that disk's speed."
        ),
        CorrelationCause::Stall => format!("Generation stopped beside {lead}: start there."),
        CorrelationCause::ConcurrentPrefill if lead.ends_with(" earlier") => {
            "A prompt was read beside this answer: expected; its speed comes back on its own.".into()
        }
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
        // The finding is the sentence itself: below earlier sessions, and
        // the update between them where there was one.
        CorrelationCause::EarlierSessions if !sample.correlation.earlier.is_empty() => {
            format!("{}.", sample.correlation.earlier)
        }
        CorrelationCause::EarlierSessions => {
            "This model now generates below its usual from earlier sessions at this depth.".into()
        }
        // Judged per verify pass, how well the draft guesses is already
        // allowed for: the passes themselves ran slower.
        CorrelationCause::None if judged_per_pass(sample) => {
            "Nothing on the machine explains it, and it is slower per verify pass, so not the text: the runtime or its draft model; check the runtime's log at this time.".into()
        }
        CorrelationCause::None => {
            "Nothing on the machine explains it: the text, the draft model or the runtime itself; check the runtime's log at this time.".into()
        }
    }
}

/// What a full KV cache costs, and what to change.
const KV_FULL: &str = "The KV cache was full, so requests were recomputed: give the runtime more KV memory, or run fewer requests at once.";

/// Whether generation speed was judged per verify pass: the runtime counts
/// the tokens each pass emits, so the draft's guessing is allowed for.
fn judged_per_pass(sample: &Sample) -> bool {
    sample.correlation.phase == crate::domain::Phase::Generation
        && sample
            .llm_tokens_per_pass
            .is_some_and(|tokens| tokens.is_finite() && tokens >= 1.0)
}

/// The wired-limit advice for Diagnostics, unless the assessment's next
/// step, `next`, already gives it.
pub(crate) fn wired_limit_note(sample: &Sample, next: &str) -> Option<String> {
    wired_limit_step(sample).filter(|step| step != next)
}

/// The wired-limit advice, once GPU memory reaches 90% of the kernel's GPU
/// wired limit (`iogpu.wired_limit_mb`, or the runtime's report of it) or
/// of Metal's recommended working set: where mlx-lm warns that generation
/// "can be slow". Metal's own count of GPU memory is held against the
/// nearer limit, else wired RAM, which bounds it.
pub(crate) fn wired_limit_step(sample: &Sample) -> Option<String> {
    let used = sample.gpu_in_use.unwrap_or(sample.wired);
    let (limit, working_set) = [
        (sample.metal.resource_limit, false),
        (sample.mlx.recommended_working_set, true),
    ]
    .into_iter()
    .filter_map(|(limit, working_set)| Some((limit.filter(|limit| *limit > 0)?, working_set)))
    .filter(|(limit, _)| used.saturating_mul(10) >= limit.saturating_mul(9))
    .min_by_key(|(limit, _)| *limit)?;
    let reading = if sample.gpu_in_use.is_some() {
        "GPU memory"
    } else {
        "Wired memory"
    };
    let (used, limit) = (bytes(used), bytes(limit));
    Some(if working_set {
        format!("{reading} is {used} of Metal's {limit} recommended working set, where MLX warns that generation can be slow: raise iogpu.wired_limit_mb (sudo sysctl iogpu.wired_limit_mb=…) or shorten the context.")
    } else {
        format!("{reading} is {used} of the {limit} GPU limit, where MLX warns that generation can be slow: raise it (sudo sysctl iogpu.wired_limit_mb=…) or shorten the context.")
    })
}

/// The verdict the screen shows for `sample`: its headline, evidence and
/// next step, as the Overview strip, Diagnostics, `--once` and the JSON
/// snapshot all say it.
pub(crate) fn assess(sample: &Sample) -> Finding {
    let mut finding = assess_observation(sample);
    if sample.llm_remote {
        if finding.title.ends_with(" waiting") {
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

/// The verdict as the dashboard shows it, having watched the session:
/// `assess`, and what the session's decode record adds to a healthy one.
pub(crate) fn assess_session(sample: &Sample, decode: &crate::efficiency::Record) -> Finding {
    let mut finding = assess(sample);
    if let Some(note) = decode_note(sample, decode, &finding) {
        finding.next = note;
    }
    finding
}

/// Under `✓ Healthy · no bottleneck`, while generation keeps its usual
/// speed and decode runs near the best this machine has reached with the
/// model: only the model's side can make it faster. An estimate, said as
/// one, and a note rather than a check: nothing is wrong.
fn decode_note(
    sample: &Sample,
    decode: &crate::efficiency::Record,
    finding: &Finding,
) -> Option<String> {
    let usual =
        sample.correlation.direction == ThroughputDirection::Flat && !sample.correlation.slow;
    if finding.verdict != Verdict::Healthy || !finding.next.is_empty() || !usual {
        return None;
    }
    let estimate = crate::efficiency::of_sample(sample)?;
    decode.near_best(sample, &estimate).then(|| {
        if estimate.draft {
            "Decode is near its estimated limit: fewer bytes/token or a better draft."
        } else {
            "Decode is near its estimated limit: fewer bytes/token or a draft model."
        }
        .into()
    })
}

/// Whether the sample's rate for the phase being judged is live and current.
fn live_rate(sample: &Sample) -> bool {
    let prefill = sample.correlation.phase == crate::domain::Phase::Prefill;
    sample.llm_fresh()
        && if prefill {
            sample.llm_prefill_tps_live
        } else {
            sample.llm_generation_tps_live
        }
}

/// A measured slowdown or stall, written into `finding`. It leads the
/// headline over any host state that is not critical: the host reading it
/// coincides with is already its reason.
fn measured_slowdown(sample: &Sample, finding: &mut Finding) -> bool {
    let prefill = sample.correlation.phase == crate::domain::Phase::Prefill;
    if live_rate(sample) && !sample.llm_remote && sample.correlation.is_material_drop() {
        (finding.verdict, finding.title) = if sample.correlation.cause == CorrelationCause::Stall {
            (Verdict::GenerationStalled, "Generation stalled".into())
        } else if prefill {
            (Verdict::PrefillSlowed, "Prefill slowed".into())
        } else {
            (Verdict::GenerationSlowed, "Generation slowed".into())
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
        // The reason is whatever else is unusual at the same time; against
        // earlier sessions, it is what the speed is compared with.
        finding.context = if sample.correlation.cause == CorrelationCause::EarlierSessions {
            "Usual from earlier sessions".into()
        } else if sample.correlation.lead.is_empty() {
            "Nothing else unusual".into()
        } else {
            format!("WITH {}", sample.correlation.lead)
        };
        finding.next = slowdown_next_step(sample);
        finding.actionable = true;
        finding.tone = sample.correlation.tone();
        return true;
    }
    false
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
        verdict: Verdict::NoBottleneck,
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
    let free_memory = "Free memory; stop unused models or apps.";
    let reduce_models = "Reduce active models; recheck swap-out.";
    let reduce_concurrency = "If paging persists, reduce concurrency.";
    let action = match impact {
        "MEMORY BOTTLENECK" => Some((Verdict::MemoryBottleneck, free_memory)),
        "MEMORY STRESS" => Some((Verdict::MemoryStress, free_memory)),
        "SWAP THRASHING" => Some((Verdict::SwapThrashing, reduce_models)),
        "HEAVY PAGING" => Some((Verdict::HeavyPaging, reduce_models)),
        "PAGE-IN RECOVERY" => Some((
            Verdict::PageInRecovery,
            "Let page-ins settle before adding work.",
        )),
        "PAGING ACTIVE" => Some((Verdict::PagingActive, reduce_concurrency)),
        "WATCH PAGING" => Some((Verdict::WatchPaging, reduce_concurrency)),
        "COMPRESSION ACTIVE" => Some((
            Verdict::CompressionActive,
            "Check whether paging also rises.",
        )),
        "THERMAL LIMIT" => Some((
            Verdict::ThermalLimit,
            "Reduce concurrency; recheck thermals.",
        )),
        _ => None,
    };
    let critical = matches!(
        impact,
        "MEMORY BOTTLENECK" | "SWAP THRASHING" | "HEAVY PAGING" | "PAGE-IN RECOVERY"
    );
    if !critical && measured_slowdown(sample, &mut finding) {
        return finding;
    }
    if let Some((verdict, action)) = action {
        finding.verdict = verdict;
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
        finding.verdict = match issue {
            crate::gpu_findings::GpuIssue::Offload { .. } => Verdict::CpuOffload,
            crate::gpu_findings::GpuIssue::VramFull { .. } => Verdict::VramFull,
            crate::gpu_findings::GpuIssue::Throttled { .. } => Verdict::GpuThrottled,
        };
        finding.title = issue.title();
        finding.evidence = issue.evidence();
        finding.next = issue.next().into();
        finding.actionable = true;
        finding.tone = Tone::Yellow;
        return finding;
    }
    if sample.impact == "SAMPLING" || !sample.rate_ready {
        finding.verdict = Verdict::CollectingBaseline;
        finding.title = "Collecting system baseline".into();
        finding.next = "Wait for the next sample.".into();
        return finding;
    }
    if sample.impact == "DATA LIMITED" {
        finding.verdict = Verdict::CountersIncomplete;
        finding.title = "System counters incomplete".into();
        finding.next = "Restore missing counters before tuning.".into();
        finding.actionable = true;
        return finding;
    }

    // Historical/reported rates must not become a current slowdown diagnosis.
    let fresh = sample.llm_fresh();
    let live = live_rate(sample);
    if live
        && sample
            .llm_waiting_requests
            .is_some_and(|waiting| waiting > 0)
    {
        let waiting = sample.llm_waiting_requests.unwrap_or_default();
        finding.verdict = Verdict::RequestsWaiting;
        finding.title = format!(
            "{} request{} waiting",
            count(sample.llm_waiting_requests),
            if waiting == 1 { "" } else { "s" }
        );
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
        finding.verdict = Verdict::Healthy;
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
    } else if !live
        && sample.llm_source == crate::domain::TelemetrySource::None
        && matches!(sample.llm_provider.as_str(), "none" | "not detected")
    {
        finding.next = "No runtime found · d for setup".into();
    } else if !live && omlx_key_missing(sample) {
        // Connected, but the admin statistics need a key nobody gave.
        finding.next = "No live rates: oMLX admin key not set · 4 for Settings".into();
    } else if let Some(need) = sample.runtime.key_need().filter(|_| !live) {
        // The runtime wants a key, or refused the one it was sent.
        let engine = sample.runtime.provider.as_deref().unwrap_or("the runtime");
        finding.next = match need {
            crate::runtime_diagnostics::KeyNeed::Missing => {
                format!("No live rates: {engine} asks for an API key · 4 for Settings")
            }
            crate::runtime_diagnostics::KeyNeed::Rejected => {
                format!("No live rates: {engine} refused the API key · 4 for Settings")
            }
        };
    } else if !live {
        finding.next = "Live generation rate unavailable.".into();
    }
    finding
}

/// oMLX answered its health check, but its statistics were not read because
/// no admin key was found.
fn omlx_key_missing(sample: &Sample) -> bool {
    sample.runtime.provider.as_deref() == Some("oMLX")
        && sample.runtime.probes.iter().any(|probe| {
            probe.path == "/admin/api/login"
                && probe.issue == Some(crate::runtime_diagnostics::ProbeIssue::CredentialsMissing)
        })
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
