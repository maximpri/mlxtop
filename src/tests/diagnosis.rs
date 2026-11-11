use crate::test_support::*;
// SPDX-License-Identifier: MIT
use super::*;

fn busy_sample() -> Sample {
    Sample {
        impact: "GPU BUSY".into(),
        gpu_util: Some(99),
        pressure: "GREEN".into(),
        thermal: "no warning".into(),
        rate_ready: true,
        vm_available: true,
        swap_available: true,
        llm_source: TelemetrySource::Live,
        llm_generation_tps: Some(24.0),
        llm_generation_tps_live: true,
        llm_observed_at: Some(SystemTime::now()),
        ..Sample::default()
    }
}

#[test]
fn high_gpu_usage_is_evidence_not_a_slowdown_or_alarm() {
    let mut sample = busy_sample();
    sample.correlation.direction = ThroughputDirection::Flat;
    let finding = assess(&sample);
    assert!(!finding.actionable);
    assert_eq!(finding.title, "Healthy · no bottleneck");
    assert_eq!(finding.tone, Tone::Green);
    assert!(finding.evidence.contains("99%"));
    assert_eq!(finding.next, "", "nothing to say beyond Healthy");
    // The chart grades saturation red; the finding still reports no bottleneck.
    assert_eq!(ChartMetric::Gpu.tone(100, Thresholds::default()), Tone::Red);
    // Without normal OS pressure the verdict stays neutral.
    sample.pressure = "YELLOW".into();
    let finding = assess(&sample);
    assert_eq!(finding.title, "No bottleneck established");
    assert_eq!(finding.tone, Tone::Muted);
    // An idle runtime is waiting, not missing a reading.
    sample.pressure = "GREEN".into();
    sample.llm_generation_tps_live = false;
    sample.llm_status = "idle".into();
    assert_eq!(assess(&sample).next, "");
    // oMLX says `healthy` after its last request; with nothing active or
    // queued that is idle too, but only while the reading is fresh.
    sample.llm_status = "healthy".into();
    sample.llm_active_requests = Some(0);
    sample.llm_waiting_requests = Some(0);
    assert_eq!(assess(&sample).next, "");
    sample.llm_active_requests = Some(1);
    assert_eq!(assess(&sample).next, "Live generation rate unavailable.");
    sample.llm_active_requests = Some(0);
    sample.llm_observed_at = Some(SystemTime::now() - Duration::from_secs(60));
    assert_eq!(assess(&sample).next, "Live generation rate unavailable.");
    // oMLX's Splash engine busy with requests oMLX does not track.
    sample.llm_observed_at = Some(SystemTime::now());
    sample.llm_status = "untracked".into();
    assert_eq!(
        assess(&sample).next,
        "GPU busy outside oMLX's request tracking (oMLX benchmark on Splash?) · no live rates."
    );
}

#[test]
fn measured_slowdown_keeps_rate_change_and_conditional_guidance() {
    let mut sample = busy_sample();
    sample.correlation = CorrelationInsight {
        direction: ThroughputDirection::Down,
        cause: CorrelationCause::Queueing,
        summary: "GEN ↓20.0% (30.0→24.0 tok/s) · unusual: 2 waiting (usual 0)".into(),
        lead: "2 waiting (usual 0)".into(),
        ..CorrelationInsight::default()
    };
    let finding = assess(&sample);
    assert_eq!(finding.title, "Generation slowed");
    assert!(finding.evidence.contains("30.0→24.0 tok/s"));
    // A slowdown that has lasted says for how long.
    let mut lasting = sample.clone();
    lasting.correlation.episode = Some(crate::domain::DropEpisode {
        reference: 30.0,
        low: 22.0,
        seconds: 34.4,
        samples: 30,
    });
    assert_eq!(
        assess(&lasting).evidence,
        "GEN ↓20.0% (30.0→24.0 tok/s) for 34s"
    );
    lasting.correlation.episode.as_mut().unwrap().seconds = 3.0;
    assert_eq!(assess(&lasting).evidence, "GEN ↓20.0% (30.0→24.0 tok/s)");
    assert_eq!(finding.context, "WITH 2 waiting (usual 0)");
    assert!(
        finding.next.starts_with("Requests are waiting"),
        "{}",
        finding.next
    );
    sample.llm_status = "stale".into();
    assert!(!assess(&sample).actionable);
    assert_eq!(assess(&sample).next, "Live generation rate unavailable.");
}

#[test]
fn a_slowdown_names_what_else_was_unusual() {
    let mut sample = busy_sample();
    sample.correlation = CorrelationInsight {
        direction: ThroughputDirection::Down,
        cause: CorrelationCause::ConcurrentPrefill,
        summary: "GEN ↓34.3% (39.3→25.8 tok/s) · unusual: 35.6k prefill beside generation".into(),
        lead: "35.6k prefill beside generation".into(),
        ..CorrelationInsight::default()
    };
    let finding = assess(&sample);
    assert_eq!(finding.title, "Generation slowed");
    assert_eq!(finding.context, "WITH 35.6k prefill beside generation");
    assert!(
        finding
            .next
            .starts_with("A prompt is being read beside this answer"),
        "{}",
        finding.next
    );
}

#[test]
fn missing_live_rates_do_not_hide_critical_memory_pressure() {
    let mut sample = Sample {
        pressure: "RED".into(),
        impact: "DATA LIMITED".into(),
        ..Sample::default()
    };
    let finding = assess(&sample);
    assert_eq!(finding.title, "Memory bottleneck");
    assert_eq!(finding.tone, Tone::Red);
    assert!(finding.actionable);
    sample.pressure = "UNKNOWN".into();
    sample.rate_ready = true;
    assert_eq!(assess(&sample).title, "System counters incomplete");
}

#[test]
fn compression_and_queue_findings_name_their_evidence() {
    let compression = Sample {
        impact: "COMPRESSION ACTIVE".into(),
        impact_tone: Tone::Yellow,
        compress: 64 * MIB,
        decompress: 16 * MIB,
        ..busy_sample()
    };
    let finding = assess(&compression);
    assert_eq!(finding.title, "Compression active");
    assert_eq!(finding.evidence, "Compression 80.0 MiB/s · paging 0 B/s");
    assert_eq!(finding.next, "Check whether paging also rises.");
    assert_eq!(finding.tone, Tone::Yellow);

    let queued = Sample {
        impact: "LLM READY".into(),
        gpu_util: Some(10),
        llm_waiting_requests: Some(3),
        llm_active_requests: Some(1),
        ..busy_sample()
    };
    let finding = assess(&queued);
    assert_eq!(finding.title, "3 requests waiting");
    assert!(finding.context.starts_with("1 active · "));
    assert_eq!(finding.next, "Reduce concurrency; recheck queue length.");
    assert!(finding.actionable);

    // A stale or old reading must not produce a queue diagnosis.
    let mut stale = queued.clone();
    stale.llm_observed_at = Some(SystemTime::now() - Duration::from_secs(60));
    let finding = assess(&stale);
    assert_eq!(finding.next, "Live generation rate unavailable.");
    assert!(!finding.actionable);
}

#[test]
fn slowdown_findings_recommend_a_check_for_each_cause() {
    for (cause, next) in [
        (CorrelationCause::Paging, "The model no longer fits in RAM"),
        (
            CorrelationCause::Compression,
            "macOS is compressing memory to make room",
        ),
        (CorrelationCause::MemoryPressure, "Memory is under pressure"),
        (
            CorrelationCause::ModelMemory,
            "The model's memory is near its ceiling",
        ),
        (
            CorrelationCause::MetalMemory,
            "The model's memory is near its ceiling",
        ),
        (
            CorrelationCause::Thermal,
            "macOS is limiting speed for heat",
        ),
        (CorrelationCause::Queueing, "are waiting: each answer slows"),
        (CorrelationCause::Cpu, "The runtime is busy on the CPU"),
        (
            CorrelationCause::GpuIdle,
            "The GPU is waiting on the runtime",
        ),
        (CorrelationCause::None, "Nothing on the machine explains it"),
    ] {
        let mut sample = busy_sample();
        sample.impact = "LLM READY".into();
        sample.swap_in = 4 * MIB;
        sample.compress = 64 * MIB;
        sample.correlation = CorrelationInsight {
            direction: ThroughputDirection::Down,
            cause,
            summary: "GEN ↓30.0% (30.0→21.0 tok/s) · unusual: x".into(),
            lead: if cause == CorrelationCause::None {
                String::new()
            } else {
                "x".into()
            },
            ..CorrelationInsight::default()
        };
        let finding = assess(&sample);
        assert_eq!(finding.title, "Generation slowed");
        assert_eq!(finding.evidence, "GEN ↓30.0% (30.0→21.0 tok/s)");
        assert!(finding.next.contains(next), "{cause:?}: {}", finding.next);
        assert!(finding.actionable);
        if cause == CorrelationCause::None {
            assert_eq!(finding.context, "Nothing else unusual");
        } else {
            assert_eq!(finding.context, "WITH x", "{cause:?}");
        }
    }
}

#[test]
fn gpu_issue_leads_with_its_own_title_evidence_and_action() {
    let mut sample = busy_sample();
    sample.impact = "CPU OFFLOAD".into();
    sample.gpu_issue = Some(crate::gpu_findings::GpuIssue::Offload {
        offloaded: 4 * 1024 * 1024 * 1024,
        size: 8 * 1024 * 1024 * 1024,
    });
    let finding = assess(&sample);
    assert!(finding.actionable);
    assert_eq!(finding.tone, Tone::Yellow);
    assert_eq!(finding.title, "Model partly on CPU");
    assert!(finding.evidence.starts_with("50% of model on CPU"));
    assert_eq!(
        finding.next,
        "Use a smaller quantization or context to fit VRAM."
    );
}

#[test]
fn advice_carries_the_readings_that_make_it_worth_acting_on() {
    let gib = 1024 * 1024 * 1024;
    let mut sample = busy_sample();
    sample.vm_available = true;
    sample.total_memory = 36 * gib;
    sample.wired = 31 * gib;
    sample.anonymous = 2 * gib;
    sample.compressor = gib;
    sample.file_backed = gib;
    sample.metal.resource_limit = Some(32 * gib);
    sample.system_power_mw = Some(92_000);
    sample.llm_waiting_requests = Some(3);
    let next = |sample: &mut Sample, cause| {
        sample.correlation.cause = cause;
        slowdown_next_step(sample)
    };
    assert!(next(&mut sample, CorrelationCause::Paging).contains("(2.0 GiB of RAM left)"));
    assert!(next(&mut sample, CorrelationCause::MetalMemory)
        .contains("Wired memory is 31.0 GiB of the 32.0 GiB GPU limit"));
    assert!(next(&mut sample, CorrelationCause::Thermal).contains("at 92 W"));
    assert!(next(&mut sample, CorrelationCause::Queueing).starts_with("3 requests are waiting"));
    sample.llm_waiting_requests = Some(1);
    assert!(next(&mut sample, CorrelationCause::Queueing).starts_with("A request is waiting"));
    // Without the readings, the advice still stands on its own.
    sample.wired = 4 * gib;
    sample.system_power_mw = None;
    assert!(next(&mut sample, CorrelationCause::ModelMemory).starts_with("The model's memory"));
    assert!(!next(&mut sample, CorrelationCause::Thermal).contains(" W"));
    sample.correlation.lead = "disk read 900.0 MiB/s".into();
    sample.correlation.unusual = vec![CorrelationCause::DiskIo];
    assert!(next(&mut sample, CorrelationCause::Stall).contains("waited on its disk (disk read"));
    // Disk work at the rate it ran while the model was fast is not blamed.
    sample.correlation.lead =
        "no new token for 2s · GPU idle · as when it ran fast: disk write 38.0 MiB/s".into();
    sample.correlation.unusual.clear();
    assert!(next(&mut sample, CorrelationCause::Stall)
        .starts_with("The runtime paused with the GPU idle and nothing on this machine"));
    sample.correlation.lead = "paging 90.0 MiB/s".into();
    sample.correlation.unusual = vec![CorrelationCause::Paging];
    assert!(next(&mut sample, CorrelationCause::Stall).contains("beside paging 90.0 MiB/s"));
}
