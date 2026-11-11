// SPDX-License-Identifier: MIT
//! Everything else.
use super::*;

#[test]
fn physical_ram_occupancy_is_independent_of_macos_pressure_accounting() {
    let vm = parse_vm_stat("Pages free: 10.\nPages speculative: 5.\n", 16_384);
    let sample = Sample {
        total_memory: 100 * 16_384,
        availability: Some(93),
        resident_memory: resident_memory_bytes(
            100 * 16_384,
            vm.free.zip(vm.speculative).map(|(a, b)| a + b),
        ),
        ..Sample::default()
    };
    assert_eq!(resident_memory_percent(&sample), Some(85));
    assert_eq!(sample.resident_memory, Some(85 * 16_384));
    for text in ["", "Pages free: unavailable\nPages speculative: 5."] {
        let vm = parse_vm_stat(text, 16_384);
        assert!(vm.free.is_none());
    }
    assert_eq!(resident_memory_bytes(100, None), None);
    assert_eq!(resident_memory_bytes(100, Some(101)), None);
    assert_eq!(resident_memory_bytes(0, Some(0)), None);
    assert_eq!(resident_memory_bytes(100, Some(100)), Some(0));
    assert_eq!(resident_memory_bytes(100, Some(0)), Some(100));
    let info = parse_linux_meminfo("MemTotal: 100 kB\nMemFree: 15 kB\nMemAvailable: 60 kB");
    assert_eq!(
        resident_memory_bytes(info.total_kb * 1024, info.free_kb.map(|free| free * 1024)),
        Some(85 * 1024)
    );
    assert!(parse_linux_meminfo("MemTotal: 100 kB").free_kb.is_none());
}

#[test]
fn linux_temperature_does_not_invent_a_throttling_signal() {
    assert_eq!(linux_thermal_label(Some(95)), "95°C measured");
    assert_eq!(linux_thermal_label(Some(85)), "85°C measured");
    assert_eq!(linux_thermal_label(Some(40)), "40°C measured");
    assert_eq!(linux_thermal_label(None), "unavailable");
}

#[test]
fn nvidia_selection_follows_uuid_and_missing_utilization_remains_visible() {
    let (mut app, views) = test_app_with_sender(0);
    app.collector = nvidia_app(2).collector.clone();
    app.gpu_selected = 1;
    let mut next = app.collector.clone();
    next.current.gpus.swap(0, 1);
    next.current.gpus[0].index = 0;
    next.current.gpus[1].index = 1;
    next.current.gpus[0].utilization = None;
    next.current.gpus[0].used = None;
    next.current.gpus[0].temperature = None;
    next.current.gpu_util = gpu::peak_utilization(&next.current.gpus);
    views.send(next).unwrap();
    app.tick();
    assert_eq!(app.gpu_selected, 0);
    assert_eq!(
        app.collector.current.gpus[app.gpu_selected].uuid,
        "GPU-fixture-1"
    );
    let screen = render_nvidia_overview(&app, 180, 50);
    assert!(screen.contains("unavailable"));
    assert!(screen.contains("— / 24.0 GiB"));
    assert!(!render_app(&test_app(0), 80, 24).contains("GPU DEVICES"));
}

#[test]
fn operator_grid_shows_available_metrics_and_conditionally_shows_latency() {
    let mut app = test_app(0);
    app.collector.current = Sample {
        total_memory: 32 * 1024 * 1024 * 1024,
        llm_provider: "oMLX".into(),
        llm_source: TelemetrySource::Live,
        llm_status: "generating".into(),
        llm_observed_at: Some(SystemTime::now()),
        llm_active_requests: Some(2),
        llm_waiting_requests: Some(1),
        ..Sample::default()
    };
    for i in 0..40 {
        app.collector.current.process_memory = Some(process_memory::Reading {
            pid: 37966,
            started: 1,
            resident: 16 * 1024 * 1024 * 1024,
            footprint: (12 + i / 10) * 1024 * 1024 * 1024,
            peak: 29 * 1024 * 1024 * 1024,
            disk_read: 0,
            disk_written: 0,
            at: Instant::now(),
        });
        app.collector
            .operator_history
            .observe(&app.collector.current, 120);
        app.collector
            .generation_history
            .push_back(ChartPoint::new(Some(280), Tone::Cyan));
        app.collector
            .prefill_history
            .push_back(ChartPoint::new(Some(1560), Tone::Cyan));
        app.collector
            .gpu_history
            .push_back(ChartPoint::new(Some(70), Tone::Green));
        app.collector
            .load_history
            .push_back(ChartPoint::new(Some(58), Tone::Green));
        app.collector
            .swap_history
            .push_back(ChartPoint::new(Some(0), Tone::Green));
        app.collector
            .cache_history
            .push_back(ChartPoint::new(Some(50), Tone::Cyan));
    }
    for (i, prompt) in [22000, 23000, 12000, 10000, 18000, 24000, 40000]
        .into_iter()
        .enumerate()
    {
        app.collector.current.llm_requests = vec![domain::RequestUsage {
            provider: "oMLX".into(),
            model: "test".into(),
            id: i.to_string(),
            prompt,
            cached: Some(prompt * 3 / 4),
            output: Some(100),
            completed: false,
            observed_at: Some(SystemTime::now()),
            queued: false,
            prefilled: None,
            idle_for: None,
            ttft_ms: None,
            output_tps: None,
        }];
        app.collector
            .request_history
            .observe(&app.collector.current.llm_requests);
        app.collector
            .operator_history
            .observe(&app.collector.current, 120);
    }
    for (width, height) in [(180, 46), (100, 40)] {
        let screen = render_app(&app, width, height);
        for label in [
            "prompt load",
            "memory",
            "queue",
            "generation",
            "prefill",
            "paging",
            "cache",
        ] {
            assert!(
                screen.contains(label),
                "missing {label} at {width}x{height}"
            );
        }
        assert!(!screen.contains("first token"));
    }
    app.collector.current.llm_requests[0].ttft_ms = Some(1250);
    app.collector
        .operator_history
        .observe(&app.collector.current, 120);
    let screen = render_app(&app, 180, 46);
    assert!(screen.contains("first token"));
    assert!(screen.contains("1250 ms"));
    assert!(screen.contains("REPORTED"));
}

#[test]
fn help_remains_readable_at_eighty_columns() {
    let mut app = test_app(0);
    app.help = true;
    let rendered = render_app(&app, 80, 24);
    for label in [
        "CONTROLS",
        "next / previous view",
        "change refresh interval",
        "newest / oldest prompt",
    ] {
        assert!(
            rendered.contains(label),
            "missing narrow help label: {label}"
        );
    }
}

#[test]
fn process_pagein_rates_are_pid_scoped() {
    let mut current = vec![LlmProcess {
        pid: 123,
        name: "omlx-server".into(),
        command: "omlx-server".into(),
        rss: 1,
        cpu: 0.0,
        memory_percent: Some(1.0),
        state: "S".into(),
        pageins: Some(30),
        pagein_rate: None,
    }];
    let previous = vec![LlmProcess {
        pid: 123,
        name: "omlx-server".into(),
        command: "omlx-server".into(),
        rss: 1,
        cpu: 0.0,
        memory_percent: Some(1.0),
        state: "S".into(),
        pageins: Some(10),
        pagein_rate: None,
    }];
    annotate_process_pagein_rates(&mut current, &previous, Duration::from_secs(2));
    assert_eq!(current[0].pagein_rate, Some(10.0));
}

#[cfg(unix)]
#[test]
fn command_text_drains_large_child_output() {
    let output = command_text(
        "/bin/sh",
        &[
            "-c",
            "i=0; while [ $i -lt 2048 ]; do printf 0123456789012345678901234567890123456789012345678901234567890123; i=$((i + 1)); done",
        ],
    )
    .expect("large child output should be collected");
    assert_eq!(output.len(), 2048 * 64);
}

#[cfg(windows)]
#[test]
fn command_text_runs_windows_commands_and_rejects_failures() {
    assert_eq!(
        command_text("cmd", &["/C", "echo 42"])
            .as_deref()
            .map(str::trim),
        Some("42")
    );
    assert_eq!(command_text("cmd", &["/C", "exit 3"]), None);
    assert_eq!(command_text("mlxtop-no-such-program", &[]), None);
}

#[test]
fn numeric_axes_keep_units_and_only_percentages_use_zero_to_one_hundred() {
    assert_eq!(normalize_chart_value(ChartMetric::Memory, 72), 72);
    assert_eq!(normalize_chart_value(ChartMetric::Gpu, 120), 100);
    assert_eq!(normalize_chart_value(ChartMetric::Cache, 120), 100);
    for value in [0, 57 * 1024 + 640, MIB, 64 * MIB] {
        assert_eq!(normalize_chart_value(ChartMetric::Swap, value), value);
    }
    let history = VecDeque::from([ChartPoint::new(Some(4096), Tone::Green)]);
    let scale = chart_scale(&history, ChartMetric::Swap, 1);
    assert!(scale.1 >= 4096 && scale.1 < 8192);
    assert!(chart_axis_label(ChartMetric::Swap, scale.1).contains("KiB/s"));
    assert_eq!(chart_scale(&history, ChartMetric::Memory, 1), (0, 100));
    assert_eq!(chart_scale(&history, ChartMetric::Cache, 1), (0, 100));
    assert_eq!(chart_scale(&history, ChartMetric::Gpu, 1), (0, 100));
}

#[test]
fn rates_use_fractional_elapsed_time() {
    assert_eq!(rate_bytes(100, 4096, Duration::from_millis(1500)), 273_067);
    assert_eq!(signed_rate_bytes(150, 100, Duration::from_millis(2000)), 25);
    assert_eq!(
        signed_rate_bytes(100, 150, Duration::from_millis(2000)),
        -25
    );
}

#[test]
fn diagnostic_fields_are_single_line_and_bounded() {
    let field = log_field("model name\nwith\tunsafe=characters/and a very long suffix");

    assert!(!field.contains('\n'));
    assert!(!field.contains('\r'));
    assert!(!field.contains('\t'));
    assert!(!field.contains(' '));
    assert!(field.chars().count() <= 160);
    assert!(field.contains("model_name_with"));
}

#[test]
fn panic_payload_formats_string_and_static_string_panics() {
    let owned: Box<dyn Any + Send> = Box::new(String::from("owned panic"));
    let static_text: Box<dyn Any + Send> = Box::new("static panic");

    assert_eq!(panic_payload(owned.as_ref()), "owned panic");
    assert_eq!(panic_payload(static_text.as_ref()), "static panic");
}

#[test]
fn server_env_discovery_ignores_wildcard_hosts_and_invalid_ports() {
    let discovered =
        parse_omlx_server_env("# oMLX server\nOMLX_HOST=\"0.0.0.0\"\nOMLX_PORT=not-a-port\n");
    assert_eq!(discovered, DiscoveredEndpoint::default());

    let blank_host: Config =
        serde_json::from_str(r#"{"omx":{"host":"  "}}"#).expect("valid json should parse");
    assert_eq!(
        resolve_omlx_endpoint(&parse_omlx_server_env("HOST=10.0.0.5\n"), &blank_host),
        ("10.0.0.5".to_owned(), DEFAULT_OMLX_PORT)
    );
}

#[test]
fn recognizes_new_server_processes_and_keeps_jan_worker_ownership() {
    for (name, command, provider) in [
        (
            "python",
            "python -m vllm.entrypoints.openai.api_server --model test",
            "vLLM",
        ),
        (
            "python",
            "python -m sglang.launch_server --model test",
            "SGLang",
        ),
        ("vllm", "vllm serve test", "vLLM"),
        (
            "GPT4All",
            "/Applications/GPT4All.app/Contents/MacOS/GPT4All",
            "GPT4All",
        ),
        ("jan", "jan serve", "Jan"),
        (
            "llama-server",
            "/Users/test/.jan/engines/llama-server --port 1337",
            "Jan",
        ),
    ] {
        assert_eq!(process_provider(name, command).as_deref(), Some(provider));
    }
    assert_eq!(
        process_provider("python", "python client.py --model vllm"),
        None
    );
    assert_eq!(process_provider("january", "january"), None);
}
