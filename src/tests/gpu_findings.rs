// SPDX-License-Identifier: MIT
use super::*;
use crate::domain::MIB;
use crate::gpu::Device;

fn card(index: u32, utilization: u8, used_mib: u64, reasons: Option<u64>) -> Device {
    Device {
        index,
        uuid: format!("GPU-{index}"),
        name: "NVIDIA RTX 4090".into(),
        utilization: Some(utilization),
        used: Some(used_mib * MIB),
        total: Some(24_000 * MIB),
        temperature: Some(84),
        throttle_reasons: reasons,
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
    let has_gpu = gpu || sample.has_nvidia_gpus();
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
    assert_eq!(issue.guidance().0, "COOL");
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
    assert_eq!(brake.guidance().0, "POWER");
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
    assert_eq!(sample.guidance_badge, "VRAM");
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
    assert_eq!(sample.guidance_badge, "COOL");
}
