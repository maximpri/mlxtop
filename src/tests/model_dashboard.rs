use crate::test_support::*;
// SPDX-License-Identifier: MIT
use super::*;

fn live_sample() -> Sample {
    Sample {
        llm_provider: "oMLX".into(),
        llm_model: "Qwen3.8-27B-oQ4e-mtp".into(),
        llm_status: "generating".into(),
        llm_source: TelemetrySource::Live,
        llm_observed_at: Some(SystemTime::now()),
        llm_prompt_tokens: Some(32_768),
        llm_output_tokens: Some(256),
        llm_count: 1,
        llm_cpu: 0.8,
        llm_rss: 16 * 1024_u64.pow(3),
        total_memory: 32 * 1024_u64.pow(3),
        gpu_in_use: Some(8 * 1024_u64.pow(3)),
        gpu_alloc: Some(16 * 1024_u64.pow(3)),
        thermal: "no warning".into(),
        metal: MetalTelemetry {
            device_name: Some("Apple M4 Max".into()),
            gpu_cores: Some(32),
            ..MetalTelemetry::default()
        },
        ..Sample::default()
    }
}

fn render(sample: &Sample, history: &request_history::History, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| draw(frame, frame.area(), sample, history))
        .unwrap();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

#[test]
fn dense_sysinfo_preserves_state_work_and_hardware_at_supported_sizes() {
    let sample = live_sample();
    let history = request_history::History::default();
    for (width, height) in [(80, 3), (180, 3), (80, 5), (100, 5), (180, 5)] {
        let screen = render(&sample, &history, width, height);
        for label in [
            "Qwen3.8-27B-oQ4e-mtp",
            "GENERATING",
            "LIVE",
            "PROMPT 32.8k",
            "OUT 256",
            "Apple M4 Max",
        ] {
            assert!(
                screen.contains(label),
                "missing {label} at {width}x{height}\n{screen}"
            );
        }
        // No label for the card, no age on a fresh reading, no normal thermals.
        for noise in ["SYSINFO", "0s old", "THERMAL no warning"] {
            assert!(
                !screen.contains(noise),
                "{noise} at {width}x{height}\n{screen}"
            );
        }
        if height == 3 {
            // The Overview strip: what the runtime uses, GPU memory as a meter.
            let wanted: &[&str] = if width >= 100 {
                &["GPU mem ", "8.0 GiB / 16.0 GiB", "LLM 16.0 GiB · CPU 1%"]
            } else {
                // Least important fields drop first on a narrow strip.
                &["GPU mem ", "8.0 GiB / 16.0 GiB"]
            };
            for label in wanted {
                assert!(
                    screen.contains(label),
                    "missing {label} at {width}\n{screen}"
                );
            }
            assert!(
                !screen.contains("RAM 32.0 GiB"),
                "RAM is in the memory panel"
            );
        }
        if height >= 5 {
            for label in [
                "32 GPU cores",
                "CPU 0.8%",
                "LLM RSS 16.0 GiB",
                "GPU mem 8.0 GiB / 16.0 GiB",
            ] {
                assert!(
                    screen.contains(label),
                    "missing {label} at {width}x{height}\n{screen}"
                );
            }
        }
        assert_eq!(
            screen.matches('┌').count(),
            1,
            "SYSINFO should use one border\n{screen}"
        );
    }
}

#[test]
fn long_model_never_hides_stale_state_or_source_age() {
    let mut sample = live_sample();
    sample.llm_model = "a-very-long-model-name-".repeat(8);
    sample.llm_status = "stale".into();
    sample.llm_observed_at = Some(SystemTime::now() - Duration::from_secs(3_600));
    for height in [3, 5] {
        let screen = render(&sample, &request_history::History::default(), 80, height);
        for label in ["STALE", "LIVE", "1h old", "PROMPT 32.8k"] {
            assert!(screen.contains(label), "missing {label}\n{screen}");
        }
        assert!(screen.contains('…'));
    }
}

#[test]
fn last_prompt_stays_scoped_to_runtime_and_unknown_os_values_stay_unknown() {
    let mut sample = live_sample();
    sample.llm_prompt_tokens = None;
    sample.llm_output_tokens = None;
    sample.llm_active_requests = Some(0);
    sample.llm_status = "idle".into();
    let mut history = request_history::History::default();
    let mut request = domain::RequestUsage {
        provider: "oMLX".into(),
        model: "previous-model".into(),
        id: "request-1".into(),
        prompt: 12_000,
        cached: None,
        output: Some(80),
        completed: true,
        queued: false,
        prefilled: None,
        idle_for: None,
        ttft_ms: None,
        output_tps: None,
        observed_at: Some(SystemTime::now() - Duration::from_secs(120)),
    };
    history.observe(&[request.clone()]);
    assert!(!render(&sample, &history, 180, 5).contains("LAST PROMPT"));
    request.model = sample.llm_model.clone();
    history.observe(&[request]);
    let screen = render(&sample, &history, 80, 5);
    assert!(screen.contains("LAST PROMPT 12.0k"), "{screen}");
    assert!(screen.contains("2m old"), "{screen}");
    // Overview's strip leaves the latest request to the prompt load panel.
    let strip = render(&sample, &history, 80, 3);
    assert!(!strip.contains("LAST PROMPT"), "{strip}");
    assert!(strip.contains("Qwen3.8-27B-oQ4e-mtp"), "{strip}");
    sample.llm_provider = "Ollama".into();
    assert!(!render(&sample, &history, 180, 5).contains("LAST PROMPT"));

    let screen = render(&Sample::default(), &history, 100, 5);
    for label in ["RAM —", "CPU —", "LLM RSS —"] {
        assert!(screen.contains(label), "missing {label}\n{screen}");
    }
    assert!(!screen.contains("CPU 0.0%"));
    assert!(!screen.contains("RAM 0 B"));
}

#[test]
fn strip_keeps_runtime_counters_only_when_they_differ_from_the_latest_request() {
    let sample = live_sample();
    let mut history = request_history::History::default();
    let mut request = domain::RequestUsage {
        provider: sample.llm_provider.clone(),
        model: sample.llm_model.clone(),
        id: "request-1".into(),
        prompt: 12_000,
        cached: None,
        output: Some(80),
        completed: false,
        queued: false,
        prefilled: None,
        idle_for: None,
        ttft_ms: None,
        output_tps: None,
        observed_at: Some(SystemTime::now()),
    };
    history.observe(&[request.clone()]);
    // Concurrent slots: the runtime total is a different reading, so it stays.
    assert!(render(&sample, &history, 80, 3).contains("PROMPT 32.8k"));
    request.prompt = sample.llm_prompt_tokens.unwrap();
    history.observe(&[request]);
    let strip = render(&sample, &history, 80, 3);
    assert!(
        !strip.contains("PROMPT"),
        "prompt load already shows it\n{strip}"
    );
}

#[test]
fn the_header_colours_a_reading_that_is_unusual() {
    let line_text = |line: &Line| -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    };
    let colour_of = |line: &Line, text: &str| {
        line.spans
            .iter()
            .find(|span| span.content.contains(text))
            .and_then(|span| span.style.fg)
    };
    let mut sample = live_sample();
    sample.llm_cpu = 380.0;
    sample.thermal = "limited 60%".into();
    // Ordinarily: a quiet CPU figure, and thermal in its warning colour.
    let footer = host_footer(&sample, 160);
    assert!(line_text(&footer).contains("CPU 380%"));
    assert_eq!(colour_of(&footer, "CPU 380%"), Some(MUTED));
    assert_eq!(colour_of(&footer, "THERMAL limited 60%"), Some(YELLOW));

    // Beside a slowdown they were unusual for: the anomaly colour, and the
    // CPU says what is usual.
    sample.correlation = CorrelationInsight {
        direction: ThroughputDirection::Down,
        cause: CorrelationCause::Thermal,
        unusual: vec![CorrelationCause::Thermal, CorrelationCause::Cpu],
        notes: vec![
            "thermal limit".into(),
            "runtime CPU 380% (usual 95%)".into(),
        ],
        ..CorrelationInsight::default()
    };
    let footer = host_footer(&sample, 160);
    assert!(
        line_text(&footer).contains("· CPU 380% (usual 95%)"),
        "{}",
        line_text(&footer)
    );
    assert_eq!(
        colour_of(&footer, "CPU 380% (usual 95%)"),
        Some(Tone::Unusual.color())
    );
    assert_eq!(
        colour_of(&footer, "THERMAL limited 60%"),
        Some(Tone::Unusual.color())
    );

    // Colour means impact: with no slowdown the same unusual readings keep
    // their ordinary colours.
    sample.correlation.direction = ThroughputDirection::Flat;
    let footer = host_footer(&sample, 160);
    assert_eq!(colour_of(&footer, "CPU 380%"), Some(MUTED));
    assert_eq!(colour_of(&footer, "THERMAL limited 60%"), Some(YELLOW));
}

#[test]
fn gpu_memory_reads_against_the_limit_and_thermal_and_power_always_show() {
    let mut sample = live_sample();
    let history = request_history::History::default();
    // Without a limit, the driver's allocation is all there is to compare with.
    let screen = render(&sample, &history, 160, 6);
    assert!(screen.contains("8.0 GiB / 16.0 GiB"), "{screen}");
    // With one, the header agrees with the memory legend's GPU limit.
    sample.metal.resource_limit = Some(32 * 1024_u64.pow(3));
    sample.system_power_mw = Some(61_400);
    let screen = render(&sample, &history, 160, 6);
    assert!(screen.contains("8.0 GiB / 32.0 GiB"), "{screen}");
    assert!(screen.contains("thermal ok · 61 W"), "{screen}");
    sample.thermal = "unavailable".into();
    assert!(render(&sample, &history, 160, 6).contains("thermal — · 61 W"));
    sample.system_power_mw = None;
    assert!(!render(&sample, &history, 160, 6).contains("thermal"));
    sample.thermal = "serious".into();
    sample.system_power_mw = Some(90_000);
    assert!(render(&sample, &history, 160, 6).contains("THERMAL serious · 90 W"));
}

#[test]
fn with_no_runtime_the_header_says_so_once() {
    let sample = Sample::default();
    let screen = render(&sample, &request_history::History::default(), 120, 3);
    assert!(screen.contains("No LLM runtime detected"), "{screen}");
    assert!(!screen.contains("not detected · not detected"), "{screen}");
}
