// SPDX-License-Identifier: MIT
use super::*;
use crate::domain::MIB;
use crate::gpu::Device;

fn card(index: u32, utilization: u8, used_mib: u64, reasons: Option<u64>) -> Device {
    Device {
        index,
        uuid: format!("GPU-{index}"),
        vendor: crate::gpu::Vendor::Nvidia,
        name: "NVIDIA RTX 4090".into(),
        utilization: Some(utilization),
        used: Some(used_mib * MIB),
        total: Some(24_000 * MIB),
        temperature: Some(84),
        throttle_reasons: reasons,
        unified: false,
        limits: Default::default(),
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn with_cards(gpus: Vec<Device>) -> Sample {
    Sample {
        gpus,
        ..Sample::default()
    }
}

#[test]
fn offload_needs_a_local_gpu_and_ignores_small_buffers() {
    let mut sample = Sample {
        llm_model_size: Some(10_000),
        llm_model_offloaded: Some(4_000),
        ..Sample::default()
    };
    let expected = GpuIssue::Offload {
        offloaded: 4_000,
        size: 10_000,
    };
    // macOS always has a GPU; elsewhere an NVIDIA card must be present.
    let gpu = cfg!(target_os = "macos");
    assert_eq!(detect(&sample, None), gpu.then(|| expected.clone()));
    sample.gpus = vec![card(0, 50, 1_000, None)];
    let has_gpu = gpu || sample.has_discrete_gpus();
    assert_eq!(detect(&sample, None), has_gpu.then(|| expected.clone()));
    if has_gpu {
        assert_eq!(expected.impact(), "CPU OFFLOAD");
        assert_eq!(expected.title(), "Model partly on CPU");
        assert!(expected.evidence().starts_with("40% of model on CPU"));
    }
    sample.llm_model_offloaded = Some(100);
    assert_eq!(detect(&sample, None), None, "1% is runtime buffers");
    sample.llm_model_offloaded = Some(10_000);
    if has_gpu {
        assert_eq!(
            detect(&sample, None).unwrap().title(),
            "Model running on CPU"
        );
    }
    sample.llm_remote = true;
    assert_eq!(detect(&sample, None), None);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn full_vram_uses_hysteresis_and_reports_the_fullest_card() {
    let sample = with_cards(vec![card(0, 90, 23_400, None), card(1, 90, 23_700, None)]);
    let issue = detect(&sample, None).unwrap();
    assert_eq!(
        issue,
        GpuIssue::VramFull {
            index: 1,
            used: 23_700 * MIB,
            total: 24_000 * MIB,
            percent: 98,
        }
    );
    assert_eq!(issue.title(), "GPU 1 VRAM full");
    // 95% does not enter the state but keeps it once entered.
    let near = with_cards(vec![card(0, 90, 22_900, None)]);
    assert_eq!(detect(&near, None), None);
    assert_eq!(
        detect(&near, Some("VRAM FULL")).unwrap().impact(),
        "VRAM FULL"
    );
    let below = with_cards(vec![card(0, 90, 22_000, None)]);
    assert_eq!(detect(&below, Some("VRAM FULL")), None);
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn throttling_counts_only_faults_on_a_busy_card() {
    let thermal = with_cards(vec![card(0, 95, 1_000, Some(0x40))]);
    let issue = detect(&thermal, None).unwrap();
    assert_eq!(issue.impact(), "GPU THROTTLED");
    assert_eq!(issue.title(), "GPU 0 thermal slowdown");
    assert_eq!(
        issue.evidence(),
        "Clocks limited by thermal slowdown · 84°C"
    );
    // A power cap under load is the configured limit, not a fault.
    assert_eq!(
        detect(&with_cards(vec![card(0, 95, 1_000, Some(0x4))]), None),
        None
    );
    // An idle card holding low clocks costs nothing.
    assert_eq!(
        detect(&with_cards(vec![card(0, 5, 1_000, Some(0x40))]), None),
        None
    );
    let brake = detect(&with_cards(vec![card(2, 60, 1_000, Some(0x80))]), None).unwrap();
    assert_eq!(brake.next(), "Check the GPU power supply and cables.");
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn placement_outranks_memory_and_memory_outranks_clocks() {
    let mut sample = with_cards(vec![card(0, 95, 23_900, Some(0x40))]);
    assert_eq!(detect(&sample, None).unwrap().impact(), "VRAM FULL");
    sample.llm_model_size = Some(100);
    sample.llm_model_offloaded = Some(50);
    assert_eq!(detect(&sample, None).unwrap().impact(), "CPU OFFLOAD");
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[test]
fn classify_reports_the_issue_as_impact_guidance_and_signal() {
    use crate::analysis::{classify, signal_summary};
    use crate::config::Thresholds;
    let mut sample = Sample {
        total_memory: 64 * 1024 * MIB,
        availability: Some(60),
        pressure: "GREEN".into(),
        vm_available: true,
        swap_available: true,
        rate_ready: true,
        gpus: vec![card(0, 95, 23_800, Some(0x40))],
        ..Sample::default()
    };
    classify(&mut sample, None, Thresholds::default());
    assert_eq!(sample.impact, "VRAM FULL");
    assert_eq!(sample.grade, "DEGRADED");
    assert!(signal_summary(&sample).starts_with("VRAM 99%"));
    // Host memory faults still outrank the card.
    sample.pressure = "RED".into();
    classify(&mut sample, None, Thresholds::default());
    assert_eq!(sample.impact, "MEMORY BOTTLENECK");
    assert_eq!(sample.gpu_issue, None);
    // Freeing VRAM leaves the thermal limit as the next finding.
    sample.pressure = "GREEN".into();
    sample.gpus[0].used = Some(1_000 * MIB);
    let previous = sample.clone();
    classify(&mut sample, Some(&previous), Thresholds::default());
    assert_eq!(sample.impact, "GPU THROTTLED");
}

#[test]
fn evidence_names_what_holds_a_busy_card_back_in_words() {
    use crate::domain::LlmProcess;
    use crate::gpu::{Limits, Vendor};
    let mut capped = card(0, 97, 1_000, Some(0x4));
    capped.limits = Limits {
        power_mw: Some(448_000),
        power_cap_mw: Some(450_000),
        clock_mhz: Some(2010),
        clock_max_mhz: Some(3105),
        ..Limits::default()
    };
    let mut hot = card(1, 95, 1_000, Some(0x40));
    hot.limits.clock_mhz = Some(1395);
    hot.limits.clock_max_mhz = Some(2790);
    let mut held = card(2, 96, 1_000, None);
    held.limits = Limits {
        clock_mhz: Some(1140),
        clock_max_mhz: Some(2790),
        link_gen: Some(3),
        link_gen_max: Some(4),
        link_width: Some(8),
        link_width_max: Some(16),
        ..Limits::default()
    };
    let mut full = card(3, 90, 23_500, None);
    full.limits.processes = vec![(77, 18 * 1024 * MIB), (912, 4 * 1024 * MIB)];
    let mut spill = card(4, 80, 15_900, None);
    spill.vendor = Vendor::Amd;
    spill.limits.ram_used = Some(6 * 1024 * MIB);
    let mut braked = card(5, 70, 1_000, Some(0x80));
    braked.limits.clock_mhz = Some(1200);
    let sample = Sample {
        gpus: vec![capped, hot, held, full, spill, braked],
        llm_processes: vec![LlmProcess {
            pid: 77,
            name: "ollama".into(),
            command: "ollama serve".into(),
            rss: 0,
            cpu: 0.0,
            memory_percent: None,
            state: "S".into(),
            pageins: None,
            pagein_rate: None,
            footprint: None,
        }],
        ..Sample::default()
    };
    assert_eq!(
        evidence(&sample),
        [
            "GPU 0 clocks limited by power cap · 448/450 W · 2010/3105 MHz",
            "GPU 1 clocks limited by thermal slowdown · 84°C · 1395/2790 MHz",
            "GPU 2 clock at 40% of its maximum (1140/2790 MHz) while 96% busy",
            "GPU 2 PCIe link at Gen3 x8 while busy; it and its slot allow Gen4 x16",
            "GPU 3 memory by process: ollama (77) 18.0 GiB · pid 912 4.0 GiB",
            "GPU 4 also holds 6.0 GiB of system RAM (GTT)",
            "GPU 5 clocks limited by power brake · 1200 MHz",
        ]
    );
    // Many processes: the largest four, then a count.
    let mut crowded = card(0, 90, 23_500, None);
    crowded.limits.processes = (1..=6).map(|pid| (pid, (7 - pid as u64) * MIB)).collect();
    let line = evidence(&Sample {
        gpus: vec![crowded],
        ..Sample::default()
    })
    .remove(0);
    assert!(line.ends_with("pid 4 3.0 MiB · 2 more"), "{line}");
}

#[test]
fn evidence_says_nothing_about_a_card_at_ease() {
    use crate::gpu::{Limits, Vendor};
    // An idle card holds its clocks and slows its link on purpose.
    let mut idle = card(0, 5, 1_000, Some(0x4));
    idle.limits = Limits {
        power_mw: Some(30_000),
        power_cap_mw: Some(450_000),
        clock_mhz: Some(210),
        clock_max_mhz: Some(3105),
        link_gen: Some(1),
        link_gen_max: Some(4),
        ..Limits::default()
    };
    // A busy card at its boost clock on its full link.
    let mut working = card(1, 97, 1_000, None);
    working.limits = Limits {
        clock_mhz: Some(2520),
        clock_max_mhz: Some(3105),
        link_gen: Some(4),
        link_gen_max: Some(4),
        ..Limits::default()
    };
    // An APU's RAM is its memory, not a spill.
    let mut apu = card(2, 97, 0, None);
    apu.vendor = Vendor::Amd;
    apu.unified = true;
    apu.limits.ram_used = Some(58 * 1024 * MIB);
    // A card with VRAM of its own and only staging buffers in RAM.
    let mut staging = card(3, 60, 1_000, None);
    staging.limits.ram_used = Some(512 * MIB);
    let sample = Sample {
        gpus: vec![idle, working, apu, staging],
        ..Sample::default()
    };
    assert!(evidence(&sample).is_empty(), "{:?}", evidence(&sample));
    assert!(evidence(&Sample::default()).is_empty());
}

#[test]
fn heat_on_a_card_names_a_slowdown_only_when_generation_slowed() {
    use crate::analysis::{CorrelationEngine, CorrelationObservation};
    use crate::config::Thresholds;
    use crate::domain::{CorrelationCause, TelemetrySource};
    use std::time::{Duration, SystemTime};
    let generating = |second: u64, tps: f64, reasons: Option<u64>| Sample {
        llm_provider: "llama.cpp".into(),
        llm_model: "local-model".into(),
        llm_source: TelemetrySource::Live,
        llm_status: "generating".into(),
        llm_generation_tps: Some(tps),
        llm_generation_tps_live: true,
        llm_active_requests: Some(1),
        llm_observed_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(second)),
        gpu_util: Some(97),
        gpus: vec![card(0, 97, 1_000, reasons)],
        ..Sample::default()
    };
    // The engine's heat input is a thermal slowdown, not the power cap.
    for (reasons, heat) in [
        (Some(0x40), true),
        (Some(0x20), true),
        (Some(0x4), false),
        (None, false),
    ] {
        assert_eq!(
            CorrelationObservation::from_sample(&generating(0, 50.0, reasons)).thermal_limited,
            heat,
            "{reasons:?}"
        );
    }
    let run = |slow: f64| {
        let mut engine = CorrelationEngine::default();
        let mut samples: Vec<Sample> = (0..10)
            .map(|second| generating(second, 50.0, None))
            .collect();
        samples.extend((10..13).map(|second| generating(second, slow, Some(0x40))));
        for sample in samples.iter_mut() {
            sample.correlation = engine.observe(sample, Thresholds::default());
        }
        samples.pop().unwrap()
    };
    // Slower while the card is held for heat: heat is the reason, and the
    // advice is that card's cooling, not the Mac's.
    let slowed = run(25.0);
    assert!(slowed.correlation.is_material_drop());
    assert_eq!(slowed.correlation.cause, CorrelationCause::Thermal);
    assert_eq!(slowed.correlation.lead, "thermal limit");
    let step = crate::diagnosis::slowdown_next_step(&slowed);
    assert_eq!(step, heat_step(&slowed));
    assert_eq!(
        step,
        "GPU 0 slows its clocks for heat at 84°C: improve its cooling or airflow."
    );
    // Held for heat while generation keeps its speed: words in Diagnostics,
    // never a cause or a colour.
    let steady = run(50.0);
    assert!(!steady.correlation.is_material_drop());
    assert_eq!(
        steady.correlation.note(CorrelationCause::Thermal),
        Some("thermal limit")
    );
    assert_eq!(steady.correlation.impact(CorrelationCause::Thermal), None);
    // Without a card that says why, the advice still names a GPU.
    let anonymous = Sample::default();
    assert_eq!(
        heat_step(&anonymous),
        "A GPU slows its clocks for heat: improve its cooling or airflow."
    );
}
