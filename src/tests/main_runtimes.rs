// SPDX-License-Identifier: MIT
//! Runtime telemetry: parsing each runtime, detection and reports.
use super::*;

#[test]
fn parses_macos_swapusage_with_spaced_equals() {
    let (total, used) = parse_swap_usage("total = 8.00G used = 1.50G free = 6.50G");
    assert_eq!(total, 8 * 1024_u64.pow(3));
    assert_eq!(used, 1_536 * 1024_u64.pow(2));
}

#[test]
fn parses_vm_stat_punctuation_and_rates() {
    let counters = parse_vm_stat(
        "Pages wired down: 10.\nPages occupied by compressor: 2.\n\
         Pages stored in compressor: 4.\nPageins: 900.\nPageouts: 12.\nSwapins: 7.\nSwapouts: 3.",
        16_384,
    );
    assert_eq!(counters.pageins, 900);
    assert_eq!(counters.pageouts, 12);
    assert_eq!(counters.wired, 10 * 16_384);
    assert_eq!(counters.compressor, 2 * 16_384);
    assert_eq!(counters.compressed_logical, 4 * 16_384);
    assert_eq!(counters.swapins, 7);
    assert_eq!(counters.swapouts, 3);
}

#[test]
fn parses_linux_meminfo_and_swap() {
    let info = parse_linux_meminfo(
        "MemTotal:       16290024 kB\nMemAvailable:    7947480 kB\n\
         Active(anon):    4973124 kB\nInactive(anon):  1904636 kB\n\
         Active(file):    3432840 kB\nInactive(file):  4277316 kB\n\
         Cached:          7662848 kB\nBuffers:          171612 kB\n\
         SwapTotal:       4194300 kB\nSwapFree:         309148 kB\n",
    );
    assert_eq!(info.total_kb, 16_290_024);
    assert_eq!(info.available_kb, 7_947_480);
    assert_eq!(info.swap_total_kb, 4_194_300);
    assert_eq!(info.swap_free_kb, 309_148);
    assert_eq!(info.anon_kb, 4_973_124 + 1_904_636);
}

#[test]
fn parses_linux_vmstat_paging_counters() {
    let (swapins, swapouts) =
        parse_linux_paging("pswpin 237839\npswpout 1172451\npgpgin 67709690\n");
    assert_eq!(swapins, 237_839);
    assert_eq!(swapouts, 1_172_451);
}

#[test]
fn nvidia_dashboard_requires_linux_and_detected_cards() {
    for count in [0, 2] {
        let mut app = nvidia_app(count);
        let expected = cfg!(any(target_os = "linux", target_os = "windows")) && count > 0;
        assert_eq!(app.collector.current.has_discrete_gpus(), expected);
        for (width, height) in [(80, 24), (180, 50)] {
            let screen = render_app(&app, width, height);
            assert_eq!(screen.contains("GPU DEVICES"), expected);
            assert_eq!(screen.contains("VRAM USED / TOTAL"), expected);
        }
        assert_eq!(
            app.gpu_chart_title(),
            if expected { "GPU max" } else { "GPU" }
        );
        app.help = true;
        assert_eq!(render_app(&app, 100, 40).contains("[ / ] GPUs"), expected);
    }
}

#[test]
fn process_details_follow_selection_and_keep_runtime_scoped() {
    let mut app = test_app(1);
    populate_dashboard_fixture(&mut app);
    for (width, height) in [(80, 24), (100, 40), (180, 50)] {
        let screen = render_app(&app, width, height);
        for label in [
            "PID 41441",
            "COMMAND",
            "--port 8000",
            "RUNTIME oMLX",
            "OS footprint",
        ] {
            assert!(
                screen.contains(label),
                "missing {label} at {width}x{height}\n{screen}"
            );
        }
        assert!(
            !screen.contains("┌ throughput"),
            "no extra throughput panel"
        );
        assert!(!screen.contains("SYSINFO"));
    }
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let screen = render_app(&app, 100, 40);
    assert!(screen.contains("PID 42002"));
    assert!(screen.contains("--model small-model.gguf"));
    assert!(screen.contains("model/state unavailable"));
    assert!(!screen.contains("Qwen3.8"));
    assert!(!screen.contains("OS footprint"));
    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    for ch in "missing".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(render_app(&app, 80, 24).contains("No processes match"));
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
    assert!(render_app(&app, 80, 24).contains("PID 41441"));
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert!(render_app(&app, 80, 24).contains("PID 42002"));
}

#[test]
fn parses_detected_llm_processes_for_top_view() {
    let snapshot = parse_processes(
        "123 15728640 12.5 48.5 Rs 20 /usr/local/bin/omlx-server --port 8080\n\
         456 2048 0.1 0.0 S 0 /usr/bin/other-worker --task test",
    );
    let processes = snapshot.llm_processes;
    assert_eq!(processes.len(), 1);
    assert_eq!(snapshot.largest_consumer.as_deref(), Some("omlx-server"));
    assert_eq!(snapshot.provider.as_deref(), Some("oMLX"));
    assert_eq!(processes[0].pid, 123);
    assert_eq!(processes[0].name, "omlx-server");
    assert_eq!(processes[0].cpu, 12.5);
    assert_eq!(processes[0].memory_percent, Some(48.5));
    assert_eq!(processes[0].state, "Rs");
    assert_eq!(processes[0].pageins, Some(20));
    assert!(processes[0].command.contains("--port 8080"));
}

#[test]
fn detects_tensorfold_without_matching_model_paths() {
    for executable in [
        "tensorfold",
        "/opt/homebrew/bin/tensorfold",
        "/usr/local/bin/tensorfold",
        "/home/test/.tensorfold/bin/tensorfold",
        "/opt/homebrew/opt/tensorfold/libexec/bin/tensorfold-native",
        r"C:\tools\tensorfold.exe",
        r"C:\tools\tensorfold-native.exe",
    ] {
        assert_eq!(
            process_provider("unknown", &format!("{executable} serve model")).as_deref(),
            Some("TensorFold"),
            "command executable: {executable}"
        );
        assert_eq!(
            process_provider(executable, "").as_deref(),
            Some("TensorFold"),
            "process name: {executable}"
        );
    }
    assert_eq!(
        process_provider("python", "python /tmp/client.py TensorFold/model"),
        None
    );
}

#[test]
fn detects_strata_processes_without_matching_other_uses_of_the_name() {
    for executable in [
        "strata",
        "/usr/local/bin/strata",
        "/opt/strata/engines/cuda/strata-engine",
        r"C:\tools\Strata.exe",
        r"C:\tools\strata-engine.exe",
    ] {
        assert_eq!(
            process_provider("unknown", &format!("{executable} serve --port 8080")).as_deref(),
            Some("Strata"),
            "command executable: {executable}"
        );
        assert_eq!(
            process_provider(executable, "").as_deref(),
            Some("Strata"),
            "process name: {executable}"
        );
    }
    assert_eq!(
        process_provider("python", "python /tmp/client.py strata/model"),
        None
    );
    assert_eq!(process_provider("stratagem", "stratagem run"), None);
}

#[test]
fn detects_runtime_entrypoints_with_consistent_provider_names() {
    for (name, command, provider) in [
        ("Python", "python -m mlx_lm.server --model test", "mlx-lm"),
        ("mlx_lm.server", "mlx_lm.server --model test", "mlx-lm"),
        ("ollama", "/usr/local/bin/ollama serve", "Ollama"),
        ("llama-server", "llama-server --model test", "llama.cpp"),
        ("Python", "python KoboldCpp.py --model test", "KoboldCpp"),
        (
            "LM",
            "Studio /Applications/LM Studio.app/Contents/MacOS/LM Studio",
            "LM Studio",
        ),
        ("llmster", "llmster", "LM Studio"),
        (
            "llama-server",
            "/home/user/.lmstudio/engines/llama-server",
            "LM Studio",
        ),
        ("local-ai", "local-ai run", "LocalAI"),
        (
            "mlx-serve",
            "/home/user/.local/bin/mlx-serve --serve --port 11234",
            "mlx-serve",
        ),
        (
            "sushi",
            "/Users/samuelreed/opt/sushi-release/v1.4.0/sushi-macos-arm64/sushi serve",
            "mlx-serve",
        ),
        (
            "splash",
            "/opt/homebrew/Cellar/splash/1.0.0/libexec/engine/splash",
            "Splash",
        ),
    ] {
        assert!(is_llm_process(name, command), "{command}");
        assert_eq!(
            process_provider(name, command).as_deref(),
            Some(provider),
            "{command}"
        );
        let snapshot = parse_processes(&format!("42 1024 1.0 0.1 S 0 {name} {command}"));
        assert_eq!(snapshot.provider.as_deref(), Some(provider), "{command}");
    }
    assert!(!is_llm_process("python", "python client.py --model ollama"));
    assert!(!is_llm_process("mlxtop", "mlxtop --help"));
    for (name, command) in [
        ("mlx-server", "/usr/local/bin/mlx-server --port 8080"),
        ("python3", "python3 -m mlx_server --port 8080"),
    ] {
        assert_ne!(
            process_provider(name, command).as_deref(),
            Some("mlx-serve"),
            "{command}"
        );
    }
    // Only Splash's engine binary: not its client launcher or a file named splash.
    for (name, command) in [
        ("vim", "vim splash"),
        (
            "python3.13",
            "python3.13 -u /opt/homebrew/Cellar/splash/1.0.0/libexec/install/launcher.py opencode",
        ),
    ] {
        assert_eq!(process_provider(name, command), None, "{command}");
    }
}

#[test]
fn detects_bionic_and_lmstudio_helpers_with_resource_totals() {
    let snapshot = parse_processes(
        "101 4096 12.5 0.1 S 0 Bionic /Applications/Bionic.app/Contents/MacOS/Bionic\n\
         102 2048 2.0 0.1 S 0 lmlink-connector /Users/test/.lmstudio/extensions/frameworks/lmlink-connector-test/lmlink-connector\n\
         103 1024 0.5 0.1 S 0 node /Users/test/.lmstudio/.internal/utils/node script.js --lmstudio-window-key=test",
    );
    assert_eq!(snapshot.provider.as_deref(), Some("LM Studio"));
    assert_eq!(snapshot.llm_count, 3);
    assert_eq!(snapshot.llm_rss, 7168 * 1024);
    assert_eq!(snapshot.llm_cpu, 15.0);
    assert_eq!(snapshot.llm_processes.len(), 3);
    assert_eq!(snapshot.top_llm.as_ref().unwrap().pid, 101);

    // The app is enough to detect LM Studio even without its helpers.
    for (name, command) in [
        ("Bionic", "/Applications/Bionic.app/Contents/MacOS/Bionic"),
        (
            "Bionic",
            "/Users/test/Applications/Bionic.app/Contents/MacOS/Bionic --some-option",
        ),
    ] {
        let snapshot = parse_processes(&format!("101 4096 12.5 0.1 S 0 {name} {command}"));
        assert_eq!(snapshot.provider.as_deref(), Some("LM Studio"));
        assert_eq!(snapshot.llm_count, 1);
    }
}

#[test]
fn bionic_detection_does_not_match_unrelated_names_or_arguments() {
    for (name, command) in [
        ("bionic", "/usr/local/bin/bionic"),
        ("python", "python bionic.py"),
        ("cat", "cat /Applications/Bionic.app/Contents/MacOS/Bionic"),
        (
            "Bionic-tools",
            "/Applications/Bionic.app/Contents/MacOS/Bionic-tools",
        ),
    ] {
        assert!(!is_llm_process(name, command), "{command}");
    }
}

#[test]
fn chart_columns_keep_the_visible_tail_and_its_captured_tones() {
    let history = VecDeque::from([
        ChartPoint::new(Some(10), Tone::Green),
        ChartPoint::new(Some(20), Tone::Yellow),
        ChartPoint::new(Some(30), Tone::Red),
    ]);
    let columns = chart_columns(&history, 2);

    assert_eq!(
        columns.iter().map(|point| point.value).collect::<Vec<_>>(),
        vec![Some(20), Some(30)]
    );
    assert_eq!(columns[0].tone, Tone::Yellow);
    assert_eq!(columns[1].tone, Tone::Red);
}

#[test]
fn telemetry_source_and_loopback_policy_are_explicit() {
    assert_eq!(TelemetrySource::Live.label(), "live API");
    assert_eq!(TelemetrySource::Log.label(), "completion log");
    assert!(is_loopback_host("127.0.0.1"));
    assert!(is_loopback_host("[::1]"));
    assert!(!is_loopback_host("192.168.1.10"));
}

#[test]
fn classification_reports_unknown_data_and_actionable_compression() {
    let mut sample = Sample {
        rate_ready: true,
        total_memory: 32 * 1024 * 1024 * 1024,
        availability: Some(50),
        pressure: "GREEN".into(),
        vm_available: true,
        swap_available: true,
        thermal: "no warning".into(),
        llm_count: 1,
        ..Sample::default()
    };
    classify(&mut sample, None, defaults());
    assert_eq!(sample.impact, "LLM READY");

    sample.vm_available = false;
    classify(&mut sample, None, defaults());
    assert_eq!(sample.impact, "DATA LIMITED");
    assert_eq!(sample.guidance_badge, "CHECK");

    sample.vm_available = true;
    sample.compress = COMPRESSION_WARN_RATE;
    classify(&mut sample, None, defaults());
    assert_eq!(sample.impact, "COMPRESSION ACTIVE");
    assert_eq!(sample.guidance_badge, "WATCH");
}

#[test]
fn parses_llm_completion_stats() {
    let stats = parse_llm_completion_line(
        "2026-08-25 03:00:00 Chat completion: model=oQ4e-mtp, 128 tokens in 4.0s (32.0 tok/s), prompt: 4096, finish_reason=stop, max_tokens=512",
    )
    .expect("completion should parse");
    assert_eq!(stats.model.as_deref(), Some("oQ4e-mtp"));
    assert_eq!(stats.output_tokens, Some(128));
    assert_eq!(stats.prompt_tokens, Some(4096));
    assert_eq!(stats.tokens_per_second, Some(32.0));
}

#[test]
fn parses_omlx_responses_api_stats_without_prompt_count() {
    let stats = parse_llm_completion_line(
        "2026-08-25 01:08:35,903 - omlx.server - INFO - [-] - Responses API: model=Qwen3.8-27B-oQ4e-mtp, 8407 tokens in 449.88s (18.7 tok/s)",
    )
    .expect("oMLX completion should parse");
    assert_eq!(stats.model.as_deref(), Some("Qwen3.8-27B-oQ4e-mtp"));
    assert_eq!(stats.output_tokens, Some(8407));
    assert_eq!(stats.prompt_tokens, None);
    assert_eq!(stats.tokens_per_second, Some(18.7));
}

#[test]
fn parses_omlx_live_stats_and_active_request() {
    let health: Value = serde_json::from_str(
        r#"{"status":"healthy","default_model":"Qwen3.8-27B-oQ4e-mtp","engine_pool":{"final_ceiling":36507222016,"current_model_memory":16852732434}}"#,
    )
    .unwrap();
    let stats: Value = serde_json::from_str(
        r#"{
            "avg_generation_tps": 30.7,
            "avg_prefill_tps": 132.9,
            "cache_efficiency": 62.1,
            "engines": {"mlx-lm": {"version": "0.31.3"}},
            "active_models": {
                "model_memory_used": 20990983024,
                "model_memory_max": 36507222016,
                "models": [{
                    "id": "Qwen3.8-27B-oQ4e-mtp",
                    "active_requests": 1,
                    "waiting_requests": 0,
                    "generating": [{"generated_tokens": 3849,"prompt_tokens": 12632,"tokens_per_second": 29.4}]
                }]
            },
            "runtime_cache": {
                "hot_cache_size_bytes": 388562944,
                "hot_cache_max_bytes": 536870912,
                "models": [{"cache_rates":{"cumulative":{"prefix_hit_rate":0.5471,"ssd_hot_rate":0.2772}}}]
            }
        }"#,
    )
    .unwrap();
    let telemetry = parse_omlx_telemetry(&health, Some(&stats));
    assert_eq!(telemetry.source, TelemetrySource::Live);
    assert_eq!(telemetry.provider.as_deref(), Some("oMLX"));
    assert_eq!(telemetry.status.as_deref(), Some("generating"));
    assert_eq!(telemetry.model.as_deref(), Some("Qwen3.8-27B-oQ4e-mtp"));
    assert_eq!(telemetry.generation_tps, Some(29.4));
    assert!(telemetry.generation_tps_live);
    assert_eq!(telemetry.prefill_tps, Some(132.9));
    assert!(!telemetry.prefill_tps_live);
    assert_eq!(telemetry.prompt_tokens, Some(12632));
    assert_eq!(telemetry.prefix_hit_rate, Some(54.71));
    assert_eq!(telemetry.mlx.version.as_deref(), Some("0.31.3"));
}

#[test]
fn parses_omlx_prefill_progress_speed_as_live_rate() {
    let health = json!({
        "status": "healthy",
        "default_model": "model"
    });
    let stats = json!({
        "avg_generation_tps": 23.5,
        "avg_prefill_tps": 14.1,
        "active_models": {
            "models": [{
                "id": "model",
                "active_requests": 1,
                "waiting_requests": 0,
                "prefilling": [{
                    "processed": 2048,
                    "total": 32768,
                    "speed": 14.1,
                    "prompt_tokens": 32768
                }],
                "generating": []
            }]
        }
    });

    let telemetry = parse_omlx_telemetry(&health, Some(&stats));

    assert_eq!(telemetry.status.as_deref(), Some("prefilling"));
    assert_eq!(telemetry.prefill_tps, Some(14.1));
    assert!(telemetry.prefill_tps_live);
    assert_eq!(telemetry.prompt_tokens, Some(32768));
}

#[test]
fn parses_captured_live_omlx_stats_and_keeps_prefilling_rows() {
    // Captured from a live oMLX server mid-prefill on 2026-11-03. A stats
    // change moved the prompt to `total` and dropped `prompt_tokens` from
    // prefilling entries; the row then failed to survive parsing, which took
    // prompt load's PREFILL progress bar and its time-left readout away on
    // every 3.2.0 release candidate. This fixture is the regression net:
    // if a row cannot be built from this exact payload again, the bug is
    // back before it reaches a terminal.
    let stats: Value =
        serde_json::from_str(include_str!("fixtures/omlx_stats_prefilling.json")).unwrap();
    let health = json!({
        "status": "healthy",
        "default_model": "Qwen3.8-27B-oQ4e-mtp",
        "engine_pool": {
            "current_model_memory": 26713869008u64,
            "final_ceiling": 36507222016u64
        }
    });

    let telemetry = parse_omlx_telemetry(&health, Some(&stats));

    assert_eq!(telemetry.source, TelemetrySource::Live);
    assert_eq!(telemetry.provider.as_deref(), Some("oMLX"));
    assert_eq!(telemetry.status.as_deref(), Some("prefilling"));
    assert_eq!(telemetry.model.as_deref(), Some("Qwen3.8-27B-oQ4e-mtp"));
    // The prefilling row survives on the captured shape: the prompt comes
    // from `total` and the progress from `processed`. Losing either dropped
    // the row, and with it the bar.
    assert_eq!(telemetry.requests.len(), 1);
    let request = &telemetry.requests[0];
    assert_eq!(request.prompt, 14479);
    assert_eq!(request.prefilled, Some(0));
    assert!(!request.queued);
    assert_eq!(request.provider, "oMLX");
    assert_eq!(request.id, "2369ab37-02ca-492f-9aff-8a4280134949");
    assert_eq!(telemetry.active_requests, Some(1));
    assert_eq!(telemetry.waiting_requests, Some(0));
    assert_eq!(telemetry.cache_efficiency, Some(87.2));
    assert_eq!(telemetry.total_prompt_tokens, Some(710838));
    assert_eq!(telemetry.model_memory, Some(26713869008));
}

#[test]
fn provider_telemetry_counts_as_an_observed_llm_without_a_matching_process() {
    let sample = Sample {
        llm_provider: "oMLX".into(),
        llm_model: "model".into(),
        llm_source: TelemetrySource::Live,
        llm_generation_tps: Some(25.0),
        llm_generation_tps_live: true,
        ..Sample::default()
    };

    assert!(llm_is_observed(&sample));
}

#[test]
fn parses_omlx_waiting_prompt_tokens() {
    let health: Value =
        serde_json::from_str(r#"{"status":"healthy","default_model":"Qwen3.8-27B-oQ4e-mtp"}"#)
            .unwrap();
    let stats = json!({
        "active_models": {"models": [{
            "id": "Qwen3.8-27B-oQ4e-mtp",
            "active_requests": 0,
            "waiting_requests": 1,
            "waiting": [{"prompt_tokens": 12632}]
        }]}
    });
    let telemetry = parse_omlx_telemetry(&health, Some(&stats));
    assert_eq!(telemetry.status.as_deref(), Some("waiting"));
    assert_eq!(telemetry.prompt_tokens, Some(12632));
}

#[test]
fn omlx_headline_values_cover_every_loaded_model() {
    let health = json!({"status": "healthy", "default_model": "a"});
    let stats = json!({
        "avg_generation_tps": 31.6,
        "active_models": {
            "total_active_requests": 3,
            "total_waiting_requests": 2,
            "models": [
                {"id": "idle", "active_requests": 0, "waiting_requests": 0},
                {"id": "a", "active_requests": 1, "waiting_requests": 2,
                 "waiting": [{"request_id": "w1", "prompt_tokens": 0}],
                 "generating": [{"request_id": "1", "generated_tokens": 40,
                                 "prompt_tokens": 900, "tokens_per_second": 20.0}]},
                {"id": "b", "active_requests": 2, "waiting_requests": 0,
                 "generating": [{"request_id": "2", "generated_tokens": 10,
                                 "prompt_tokens": 300, "tokens_per_second": 8.5}],
                 "prefilling": [{"request_id": "3", "speed": 150.0, "processed": 64,
                                 "total": 2048}]}
            ]
        },
        "runtime_cache": {"models": [
            {"id": "a", "cache_rates": {"cumulative": {"prefix_hit_rate": 0.9}}}
        ]}
    });

    let telemetry = parse_omlx_telemetry(&health, Some(&stats));

    assert_eq!(telemetry.model.as_deref(), Some("2 models · a"));
    assert_eq!(telemetry.status.as_deref(), Some("prefilling"));
    assert_eq!(telemetry.active_requests, Some(3));
    assert_eq!(telemetry.waiting_requests, Some(2));
    assert_eq!(telemetry.generation_tps, Some(28.5));
    assert!(telemetry.generation_tps_live);
    assert_eq!(telemetry.prefill_tps, Some(150.0));
    assert_eq!(telemetry.output_tokens, Some(50));
    // Three requests are in flight, so no single prompt size or cache
    // reuse rate describes the server.
    assert_eq!(telemetry.prompt_tokens, None);
    assert_eq!(telemetry.prefix_hit_rate, None);
}

#[test]
fn omlx_queue_totals_fall_back_to_complete_model_counts() {
    let health = json!({"status": "healthy"});
    let mut stats = json!({"active_models": {"models": [
        {"id": "a", "active_requests": 0, "waiting_requests": 1},
        {"id": "b", "active_requests": 2, "waiting_requests": 0}
    ]}});
    let telemetry = parse_omlx_telemetry(&health, Some(&stats));
    assert_eq!(telemetry.active_requests, Some(2));
    assert_eq!(telemetry.waiting_requests, Some(1));
    assert_eq!(telemetry.status.as_deref(), Some("waiting"));

    stats["active_models"]["models"][0]["is_loading"] = json!(true);
    let telemetry = parse_omlx_telemetry(&health, Some(&stats));
    assert_eq!(telemetry.status.as_deref(), Some("loading"));
    stats["active_models"]["models"][1]["generating"] =
        json!([{"request_id": "1", "tokens_per_second": 5.0}]);
    let telemetry = parse_omlx_telemetry(&health, Some(&stats));
    assert_eq!(telemetry.status.as_deref(), Some("generating"));

    stats["active_models"]["models"][1]
        .as_object_mut()
        .unwrap()
        .remove("waiting_requests");
    let telemetry = parse_omlx_telemetry(&health, Some(&stats));
    assert_eq!(telemetry.waiting_requests, None);
}

#[test]
fn omlx_concurrent_rates_are_live_only_when_every_request_reports() {
    let health = json!({"status": "healthy"});
    let stats = json!({
        "avg_generation_tps": 31.6,
        "active_models": {"models": [{"id": "a", "active_requests": 2,
            "waiting_requests": 0,
            "generating": [
                {"request_id": "1", "generated_tokens": 5, "prompt_tokens": 10,
                 "tokens_per_second": 20.0},
                {"request_id": "2", "prompt_tokens": 12}
            ]}]}
    });
    let telemetry = parse_omlx_telemetry(&health, Some(&stats));
    assert_eq!(telemetry.model.as_deref(), Some("a"));
    assert_eq!(telemetry.generation_tps, Some(31.6));
    assert!(!telemetry.generation_tps_live);
    assert_eq!(telemetry.output_tokens, None);
}

#[test]
fn omlx_cluster_rates_come_from_rank_zero_requests() {
    let health = json!({"status": "healthy"});
    let running = |id: &str, decode: f64| {
        json!({"status": "running", "request_id": id, "prompt_tokens": 400,
            "completion_tokens": 30, "decode_tps": decode,
            "prefill_progress": {"active": false, "speed": 0.0}})
    };
    let stats = json!({"active_models": {"models": [{
        "id": "dist", "active_requests": 2, "waiting_requests": 0,
        "prefilling": [],
        "generating": [{"request_id": "rank0", "generated_tokens": 30,
                        "prompt_tokens": 400, "tokens_per_second": 11.0}],
        "cluster": {"live": {"age_seconds": 0.4, "stale": false, "metrics": {
            "active_requests": 2,
            "active_request_metrics": [running("r1", 9.0), running("r2", 11.0)],
            "last_request": running("r2", 11.0)}}}
    }]}});

    let telemetry = parse_omlx_telemetry(&health, Some(&stats));

    assert_eq!(telemetry.generation_tps, Some(20.0));
    assert!(telemetry.generation_tps_live);
    assert_eq!(telemetry.output_tokens, Some(60));
    assert_eq!(telemetry.prompt_tokens, None);
    let ids: Vec<_> = telemetry.requests.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["r1", "r2"]);
}

#[test]
fn retained_omlx_rates_are_not_marked_live_when_idle() {
    let health: Value =
        serde_json::from_str(r#"{"status":"healthy","default_model":"Qwen3.8-27B-oQ4e-mtp"}"#)
            .unwrap();
    let stats = json!({
        "avg_generation_tps": 31.6,
        "avg_prefill_tps": 133.2,
        "active_models": {"models": [{
            "id": "Qwen3.8-27B-oQ4e-mtp",
            "active_requests": 0,
            "waiting_requests": 0,
            "generating": []
        }]}
    });
    let telemetry = parse_omlx_telemetry(&health, Some(&stats));

    assert_eq!(telemetry.status.as_deref(), Some("idle"));
    assert_eq!(telemetry.generation_tps, Some(31.6));
    assert_eq!(telemetry.prefill_tps, Some(133.2));
    assert!(!telemetry.generation_tps_live);
    assert!(!telemetry.prefill_tps_live);

    let sample = Sample {
        llm_source: TelemetrySource::Live,
        llm_generation_tps: telemetry.generation_tps,
        llm_prefill_tps: telemetry.prefill_tps,
        ..Sample::default()
    };
    assert_eq!(llm_generation_rate_label(&sample), "AVG GEN 31.6 tok/s");
    assert_eq!(llm_prefill_rate_label(&sample), "AVG PREFILL 133.2 tok/s");
    let native = Sample {
        llm_provider: "mlx-serve".into(),
        ..sample
    };
    assert_eq!(llm_prefill_rate_label(&native), "LAST PREFILL 133.2 tok/s");
}

#[test]
fn parses_mlx_allocator_and_device_metrics_without_inference() {
    let health = json!({
        "status": "healthy",
        "default_model": "Qwen3.8-27B-oQ4e-mtp",
        "mlx_version": "0.29.3",
        "mlx_memory": {"active_bytes": 4_000, "peak_bytes": 8_000}
    });
    let stats = json!({"mlx": {"cache_bytes": 2_000}});
    let telemetry = parse_mlx_runtime_telemetry(&health, Some(&stats));

    assert_eq!(telemetry.version.as_deref(), Some("0.29.3"));
    assert_eq!(telemetry.active_memory, Some(4_000));
    assert_eq!(telemetry.cache_memory, Some(2_000));
    assert_eq!(telemetry.peak_memory, Some(8_000));
}

#[test]
fn parses_mlx_device_metadata_and_metal_limit() {
    let device = json!({
        "chip_name": "Apple M4",
        "gpu_cores": 10,
        "device_name": "Apple M4 GPU",
        "architecture": "arm64",
        "max_recommended_working_set_size": 27_000
    });
    let settings = json!({
        "system": {
            "active_memory_bytes": 18_000,
            "omlx_phys_footprint_bytes": 12_000,
            "iogpu_wired_limit_bytes": 24_000
        }
    });
    let telemetry = parse_mlx_metadata(Some(&device), Some(&settings));

    assert_eq!(telemetry.device_name.as_deref(), Some("Apple M4 GPU"));
    assert_eq!(telemetry.architecture.as_deref(), Some("arm64"));
    assert_eq!(telemetry.memory_size, None);
    assert_eq!(telemetry.active_memory, None);
    assert_eq!(telemetry.recommended_working_set, Some(27_000));
    assert_eq!(telemetry.process_footprint, Some(12_000));
    assert_eq!(telemetry.resource_limit, Some(24_000));
}

#[test]
fn converts_omlx_device_memory_gb_to_bytes() {
    let device = json!({"memory_gb": 36});
    let telemetry = parse_mlx_metadata(Some(&device), None);

    assert_eq!(telemetry.memory_size, Some(36 * 1024 * 1024 * 1024));
}

#[test]
fn parses_metal_stats_without_charging_driver_memory_as_gpu_memory() {
    let ioreg = r#"
        "PerformanceStatistics" = {"In use system memory (driver)"=0,"Alloc system memory"=3224567808,"Tiler Utilization %"=16,"Renderer Utilization %"=15,"Device Utilization %"=16,"In use system memory"=850280448}
        "model" = "Apple M4"
        "gpu-core-count" = 10
    "#;
    assert_eq!(
        parse_gpu(ioreg),
        (Some(16), Some(3_224_567_808), Some(850_280_448))
    );
    let metal = parse_metal_hardware(ioreg);
    assert_eq!(metal.device_name.as_deref(), Some("Apple M4"));
    assert_eq!(metal.gpu_cores, Some(10));
    assert_eq!(metal.renderer_util, Some(15));
    assert_eq!(metal.tiler_util, Some(16));
}

#[test]
fn config_parses_from_json() {
    let json_str = r#"{"interval":5,"history":500,"omx":{"host":"0.0.0.0","port":9090},"memory_warn_load":80,"memory_critical_load":90,"gpu_warn_load":85,"gpu_critical_load":95,"swap_warn_rate":1048576,"swap_critical_rate":16777216}"#;
    let config: Config = serde_json::from_str(json_str).expect("valid json should parse");
    assert_eq!(config.interval, Some(5));
    assert_eq!(config.history, Some(500));
    assert_eq!(
        config.omx.as_ref().unwrap().host.as_deref(),
        Some("0.0.0.0")
    );
    assert_eq!(config.omx.as_ref().unwrap().port, Some(9090));
    assert_eq!(config.memory_warn_load, Some(80));
    assert_eq!(config.memory_critical_load, Some(90));
    assert_eq!(config.gpu_warn_load, Some(85));
    assert_eq!(config.gpu_critical_load, Some(95));
    assert_eq!(config.swap_warn_rate, Some(1048576));
    assert_eq!(config.swap_critical_rate, Some(16777216));
}

#[test]
fn config_omlx_defaults_when_missing() {
    let json_str = r#"{"interval":2}"#;
    let config: Config = serde_json::from_str(json_str).unwrap();
    assert_eq!(config.interval, Some(2));
    assert_eq!(config.omx, None);
}

#[test]
fn configured_gpu_thresholds_change_the_reported_load_label() {
    let config: Config = serde_json::from_str(r#"{"gpu_warn_load":85,"gpu_critical_load":95}"#)
        .expect("valid json should parse");
    let thresholds = Thresholds::from_config(&config);

    assert_eq!(gpu_load_label(Some(80), defaults()), "busy");
    assert_eq!(gpu_load_label(Some(80), thresholds), "within target");
    assert_eq!(gpu_load_label(Some(90), thresholds), "busy");
    assert_eq!(gpu_load_label(Some(96), thresholds), "saturated");
}

#[test]
fn explicit_endpoint_settings_win_over_discovery() {
    let discovered = parse_omlx_server_env("HOST=10.0.0.5\nPORT=9000\n");
    assert_eq!(discovered.host.as_deref(), Some("10.0.0.5"));
    assert_eq!(discovered.port, Some(9000));

    let config: Config = serde_json::from_str(r#"{"omx":{"host":"192.168.1.20","port":8123}}"#)
        .expect("valid json should parse");
    assert_eq!(
        resolve_omlx_endpoint(&discovered, &config),
        ("192.168.1.20".to_owned(), 8123)
    );
}

#[test]
fn endpoint_falls_back_to_discovery_then_built_in_defaults() {
    let discovered = parse_omlx_server_env("HOST=10.0.0.5\nPORT=9000\n");
    assert_eq!(
        resolve_omlx_endpoint(&discovered, &Config::default()),
        ("10.0.0.5".to_owned(), 9000)
    );
    assert_eq!(
        resolve_omlx_endpoint(&DiscoveredEndpoint::default(), &Config::default()),
        (DEFAULT_OMLX_HOST.to_owned(), DEFAULT_OMLX_PORT)
    );

    let port_only: Config =
        serde_json::from_str(r#"{"omx":{"port":8123}}"#).expect("valid json should parse");
    assert_eq!(
        resolve_omlx_endpoint(&DiscoveredEndpoint::default(), &port_only),
        (DEFAULT_OMLX_HOST.to_owned(), 8123)
    );
}

#[test]
fn shared_history_stretch_keeps_every_spike_gap_and_captured_tone() {
    let history = VecDeque::from([
        ChartPoint::new(Some(10), Tone::Green),
        ChartPoint::new(None, Tone::Muted),
        ChartPoint::new(Some(99), Tone::Red),
        ChartPoint::new(Some(20), Tone::Yellow),
    ]);
    let points = chart_columns(&history, 6);
    for width in [6, 17, 60] {
        let stretched = stretch_chart_columns(&points, width);
        assert_eq!(stretched.len(), width);
        for (column, point) in stretched.iter().enumerate() {
            let original = &points[column * 6 / width];
            assert_eq!(point.value, original.value);
            assert_eq!(point.tone, original.tone);
            assert_eq!(point.break_before, original.break_before);
        }
        assert!(stretched
            .iter()
            .any(|p| p.value == Some(99) && p.tone == Tone::Red));
        assert_eq!(stretched.last().unwrap().value, Some(20));
    }
    assert!(stretch_chart_columns(&[], 12).is_empty());
    assert!(stretch_chart_columns(&points, 0).is_empty());
}

#[test]
fn provider_capacity_details_appear_in_top_without_becoming_prompt_counts() {
    let mut app = test_app(1);
    app.collector.current.llm_details =
        Some("1 loaded models · context capacity 8192 tokens".into());
    let text = render_app(&app, 120, 30);
    assert!(text.contains("context capacity 8192 tokens"));
    assert_eq!(app.collector.current.llm_prompt_tokens, None);
}

#[test]
fn reported_prefill_keeps_its_last_result_label() {
    let sample = Sample {
        llm_provider: "KoboldCpp".into(),
        llm_source: TelemetrySource::Report,
        llm_prefill_tps: Some(133.2),
        ..Sample::default()
    };
    assert_eq!(llm_prefill_rate_label(&sample), "LAST PREFILL 133.2 tok/s");
}

#[test]
fn parses_captured_live_omlx_generating_and_cached_prefill_rows() {
    // Captured from a live oMLX server on 2026-11-09, one request generating
    // and the next one prefilling. These pin the fields the decode tracker
    // and the stall verdict rely on.
    let health = json!({"status": "healthy", "default_model": "Qwen3.8-27B-oQ4e-mtp"});
    let stats: Value =
        serde_json::from_str(include_str!("fixtures/omlx_stats_generating_live.json")).unwrap();
    let telemetry = parse_omlx_telemetry(&health, Some(&stats));
    assert_eq!(telemetry.status.as_deref(), Some("generating"));
    let [request] = telemetry.requests.as_slice() else {
        panic!("one generating request: {:?}", telemetry.requests);
    };
    assert_eq!(request.id, "e28cdebf-4a73-4147-9528-f18806e700f9");
    assert_eq!(request.prompt, 72_031);
    assert_eq!(request.output, Some(947));
    assert!(request.prefilled.is_none());
    // `last_activity_age_seconds` is how a stall is seen at all.
    let idle = request.idle_for.expect("idle age parsed").as_secs_f64();
    assert!((idle - 0.047).abs() < 0.001, "{idle}");
    assert!(stats.get("api_key").and_then(Value::as_str) == Some("REDACTED"));

    // The prefill's first poll reports the cached prefix as processed: 72,000
    // of 74,666, so 96% before any new token is read. The tracker turns it
    // into the request's cache reuse, and progress counts only new tokens.
    let stats: Value =
        serde_json::from_str(include_str!("fixtures/omlx_stats_prefilling_live.json")).unwrap();
    let mut telemetry = parse_omlx_telemetry(&health, Some(&stats));
    assert_eq!(telemetry.status.as_deref(), Some("prefilling"));
    let mut tracker = crate::decode_tracker::DecodeTracker::default();
    tracker.apply(&mut telemetry.requests, Instant::now());
    let [request] = telemetry.requests.as_slice() else {
        panic!("one prefilling request: {:?}", telemetry.requests);
    };
    assert_eq!((request.prompt, request.prefilled), (74_666, Some(72_000)));
    assert_eq!(request.cached, Some(72_000));
    assert!(
        request.idle_for.is_none(),
        "a prefill is not a stalled decode"
    );
    let mut history = request_history::History::default();
    history.observe(&telemetry.requests);
    let sample = Sample {
        llm_source: TelemetrySource::Live,
        llm_status: "prefilling".into(),
        llm_observed_at: Some(SystemTime::now()),
        llm_requests: telemetry.requests.clone(),
        ..Sample::default()
    };
    let screen = render_view(100, 10, |frame| {
        crate::request_dashboard::draw(frame, frame.area(), &history, &sample, 0, 1)
    });
    // Reuse shows as reuse; progress is over the 2,666 new tokens.
    assert!(screen.contains("CACHED 96%"), "{screen}");
    assert!(screen.contains("PREFILL"), "{screen}");
    assert!(screen.contains("0% · 0 / 2,666"), "{screen}");
}

#[test]
fn captured_omlx_stats_report_a_full_ssd_cache_and_its_block_counters() {
    let stats: Value =
        serde_json::from_str(include_str!("fixtures/omlx_stats_generating_live.json")).unwrap();
    let cache = crate::omlx::parse_ssd_cache(&stats).expect("SSD cache reported");
    assert_eq!(
        cache,
        domain::SsdCache {
            used: 128_780_156_928,
            capacity: 128_849_018_880,
            loads: 1520,
            saves: 1314,
            evictions: 327,
            hit_tokens: Some(26_303_360),
            disk_hit_tokens: Some(48_640),
        }
    );
    // 99.95% of capacity: new saves evict old prefixes.
    assert!(cache.full());
    let telemetry = parse_omlx_telemetry(&json!({"status": "healthy"}), Some(&stats));
    assert_eq!(telemetry.ssd_cache, Some(cache));

    let mut off = stats.clone();
    off["runtime_cache"]["ssd_cache_unavailable"] = json!(true);
    assert_eq!(crate::omlx::parse_ssd_cache(&off), None);
    let mut no_dir = stats.clone();
    no_dir["runtime_cache"]["ssd_cache_dir"] = Value::Null;
    assert_eq!(crate::omlx::parse_ssd_cache(&no_dir), None);
    assert_eq!(crate::omlx::parse_ssd_cache(&json!({})), None);
    assert!(!domain::SsdCache::default().full());
}
