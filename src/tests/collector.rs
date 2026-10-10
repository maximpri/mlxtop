use crate::test_support::*;
// SPDX-License-Identifier: MIT
// Collector, sampler and oMLX client behavior driven through the host seam.
use super::*;

fn collector(host: FakeHost, platform: Platform, home: Option<PathBuf>) -> Collector {
    Collector::with_host(60, offline_config(), Box::new(host), platform, home)
}

fn journal(collector: &Collector) -> Vec<(String, String)> {
    collector
        .signals
        .iter()
        .map(|event| (event.state.clone(), event.summary.clone()))
        .collect()
}

#[test]
fn macos_collector_reads_hardware_memory_paging_and_processes() {
    let host = macos_host();
    let home = TempDir::new("macos-home");
    home.write(
        ".omlx-coding/logs/server.log",
        "noise\nINFO Chat completion: model=qwen-coder, 120 tokens in 2.00s (60.0 tok/s), prompt: 500, cached: 0\n",
    );
    let mut collector = collector(host.clone(), Platform::MacOs, Some(home.0.clone()));
    assert_eq!(collector.total_memory, 36 * 1024 * MIB);
    assert_eq!(collector.page_size, 16_384);
    assert_eq!(collector.metal.device_name.as_deref(), Some("Apple M3 Max"));
    assert_eq!(collector.metal.gpu_cores, Some(40));
    assert_eq!(collector.metal.architecture.as_deref(), Some("arm64"));
    assert_eq!(collector.metal.resource_limit, Some(28_000 * MIB));

    let first = collector.sample();
    assert_eq!(first.updated, "12:00:01");
    assert_eq!(first.pressure, "GREEN");
    assert_eq!(first.pressure_meaning, "normal");
    assert_eq!(first.availability, Some(63));
    assert_eq!(
        first.resident_memory,
        Some(36 * 1024 * MIB - 12_000 * 16_384)
    );
    assert_eq!(first.wired, 50_000 * 16_384);
    assert_eq!(first.compressor, 10_000 * 16_384);
    assert_eq!(first.compressed_logical, 40_000 * 16_384);
    assert_eq!((first.swap_total, first.swap_used), (2048 * MIB, 512 * MIB));
    assert!(first.swap_available && first.vm_available);
    assert!(!first.rate_ready, "the first sample has no rate baseline");
    assert_eq!((first.swap_in, first.swap_out), (0, 0));
    assert_eq!(first.gpu_util, Some(93));
    assert_eq!(first.gpu_in_use, Some(2 * 1024 * MIB));
    assert_eq!(first.metal.renderer_util, Some(91));
    assert_eq!(first.metal.tiler_util, Some(12));
    assert_eq!(first.thermal, "no warning");
    assert_eq!(first.llm_count, 1);
    assert_eq!(first.llm_pid, 101);
    assert_eq!(first.llm_top, "omlx-server");
    // The live API is offline, so the completion log is the only source.
    assert_eq!(first.llm_provider, "oMLX");
    assert_eq!(first.llm_source, TelemetrySource::Log);
    assert_eq!(first.llm_status, "last result");
    assert_eq!(first.llm_model, "qwen-coder");
    assert_eq!(first.llm_generation_tps, Some(60.0));
    assert!(!first.llm_generation_tps_live);
    assert_eq!(first.llm_output_tokens, Some(120));
    assert_eq!(first.llm_prompt_tokens, Some(500));
    assert!(first.llm_observed_at.is_some());
    let events = journal(&collector);
    assert_eq!(events[0].0, "SYSTEM");
    assert!(events[0].1.starts_with("journal started"));
    assert!(events
        .iter()
        .any(|(state, summary)| state == "LLM" && summary.contains("detected oMLX")));

    let second = collector.sample();
    assert_eq!(second.updated, "12:00:02");
    assert_eq!(second.pressure, "YELLOW");
    assert!(second.rate_ready);
    assert!(second.swap_in > 0 && second.swap_out > second.swap_in);
    assert!(second.compress > 0 && second.decompress > 0 && second.reactivated > 0);
    assert_eq!(collector.generation_history.len(), 2);
    assert_eq!(collector.load_history.len(), 2);
    assert_eq!(
        collector.swap_history.back().and_then(|point| point.value),
        Some(second.swap_in + second.swap_out)
    );
    assert_eq!(collector.swap_history.front().unwrap().value, None);
    assert_eq!(collector.gpu_history.back().unwrap().value, Some(93));
    let events = journal(&collector);
    assert!(events
        .iter()
        .any(|(state, summary)| state == "PRESSURE" && summary.contains("memory state")));
    assert!(events
        .iter()
        .any(|(state, summary)| state == "PAGING" && summary.starts_with("active")));
    let calls = host.calls();
    assert!(calls.contains(&"/usr/bin/pmset -g therm".to_string()));
    assert!(!calls.iter().any(|call| call.starts_with("read /proc")));
}

#[test]
fn linux_collector_reads_proc_nvidia_and_thermal_zones() {
    let host = linux_host();
    let mut collector = collector(host.clone(), Platform::Linux, None);
    assert_eq!(collector.total_memory, 16_000_000 * 1024);
    assert_eq!(collector.page_size, 4096);
    assert_eq!(collector.metal.architecture.as_deref(), Some("x86_64"));

    let first = collector.sample();
    assert_eq!(first.availability, Some(25));
    assert_eq!(first.pressure, "YELLOW", "75% unavailable memory warns");
    assert_eq!(first.pressure_meaning, "warning");
    assert_eq!(first.resident_memory, Some(14_000_000 * 1024));
    assert_eq!(first.anonymous, 7_000_000 * 1024);
    assert_eq!(first.file_backed, 5_600_000 * 1024);
    assert_eq!(
        (first.swap_total, first.swap_used),
        (4_000_000 * 1024, 1_000_000 * 1024)
    );
    if cfg!(target_os = "linux") {
        assert_eq!(first.gpus.len(), 1);
        assert_eq!(first.gpus[0].uuid, "GPU-aaaa");
        assert_eq!(first.gpu_util, Some(64));
    } else {
        assert!(first.gpus.is_empty(), "NVIDIA collection is Linux-only");
        assert_eq!(first.gpu_util, None);
        assert!(!host
            .calls()
            .iter()
            .any(|call| call.starts_with("nvidia-smi")));
    }
    assert_eq!(first.gpu_in_use, None, "VRAM stays per card");
    assert_eq!(first.thermal, "72°C measured");
    assert_eq!(first.llm_provider, "Ollama");
    assert_eq!(first.llm_status, "running");
    assert_eq!(first.llm_model, "ollama");
    assert_eq!(first.llm_source, TelemetrySource::None);

    let second = collector.sample();
    assert!(second.rate_ready);
    assert_eq!(second.compress, 0, "Linux has no compressor counters");
    assert!(second.swap_in > 0 && second.swap_out > second.swap_in);
    if cfg!(target_os = "linux") {
        assert_eq!(second.gpus[0].temperature, Some(61));
    } else {
        assert!(second.gpus.is_empty());
    }
}

#[test]
fn missing_host_telemetry_stays_unavailable_instead_of_zero() {
    for platform in [Platform::Linux, Platform::MacOs] {
        let mut collector = collector(FakeHost::default(), platform, None);
        let sample = collector.sample();
        assert_eq!(sample.updated, "??:??:??");
        assert_eq!(sample.pressure, "UNKNOWN");
        assert_eq!(sample.availability, None);
        assert_eq!(sample.resident_memory, None);
        assert_eq!(sample.gpu_util, None);
        assert!(sample.gpus.is_empty());
        assert_eq!(sample.thermal, "unavailable");
        assert_eq!(sample.llm_count, 0);
        assert_eq!(sample.llm_provider, "none");
        assert_eq!(sample.llm_status, "offline");
        assert_eq!(sample.llm_model, "not detected");
        assert_eq!(sample.llm_source, TelemetrySource::None);
        assert!(!sample.swap_available || platform == Platform::MacOs);
        assert_eq!(collector.load_history.back().unwrap().value, None);
        assert_eq!(collector.gpu_history.back().unwrap().tone, Tone::Muted);
    }
    let collector = collector(FakeHost::default(), Platform::MacOs, None);
    assert_eq!(collector.page_size, 16_384, "macOS default page size");
    assert_eq!(collector.metal.resource_limit, None);
    let collector = collector_linux_without_proc();
    assert_eq!(collector.page_size, 4096, "Linux default page size");
    assert_eq!(collector.total_memory, 0);
}

fn collector_linux_without_proc() -> Collector {
    collector(FakeHost::default(), Platform::Linux, None)
}

#[test]
fn linux_memory_falls_back_to_meminfo_total_and_escalates_on_stalls() {
    let host = FakeHost::default()
        .file("/proc/meminfo", "MemTotal: 1000 kB\nMemAvailable: 900 kB\n")
        .file(
            "/proc/pressure/memory",
            "full avg10=7.50 avg60=0.00 avg300=0.00 total=1\n",
        );
    let mut sample = Sample::default();
    sample_linux_memory(&host, &mut sample, 4096, 0, defaults_thresholds());
    assert_eq!(sample.total_memory, 1000 * 1024);
    assert_eq!(sample.availability, Some(90));
    assert_eq!(sample.resident_memory, None, "MemFree is missing");
    assert_eq!(sample.pressure, "RED");
    assert_eq!(sample.pressure_meaning, "critical");

    let host = FakeHost::default().file("/proc/meminfo", "MemAvailable: 900 kB\n");
    let mut sample = Sample::default();
    sample_linux_memory(&host, &mut sample, 4096, 0, defaults_thresholds());
    assert_eq!(sample.total_memory, 0);
    assert_eq!(sample.availability, None);
    assert_eq!(sample.pressure, "UNKNOWN");
    assert!(sample.swap_available, "meminfo was readable");
}

fn defaults_thresholds() -> Thresholds {
    Thresholds::default()
}

#[test]
fn linux_thermal_ignores_unreadable_and_non_positive_zones() {
    let host = FakeHost::default()
        .dir("/sys/class/thermal", &["/z/a", "/z/b", "/z/c", "/z/d"])
        .file("/z/a/temp", "-5000\n")
        .file("/z/b/temp", "garbage\n")
        .file("/z/c/temp", "45999\n");
    assert_eq!(linux_thermal_celsius(&host, None), Some(45));
    assert_eq!(linux_thermal_celsius(&host, Some(80)), Some(80));
    assert_eq!(linux_thermal_celsius(&FakeHost::default(), None), None);
}

#[test]
fn macos_pressure_levels_map_to_states_and_unknown_levels_are_ignored() {
    for (level, state, meaning) in [
        ("1", "GREEN", "normal"),
        ("2", "YELLOW", "warning"),
        ("4", "RED", "critical"),
        ("9", "UNKNOWN", "unavailable"),
    ] {
        let host = FakeHost::default().command(
            "/usr/sbin/sysctl -n kern.memorystatus_vm_pressure_level",
            level,
        );
        let mut sample = Sample::default();
        sample_macos_memory(&host, &mut sample, 16_384);
        assert_eq!(
            (sample.pressure.as_str(), sample.pressure_meaning.as_str()),
            (state, meaning)
        );
        assert!(!sample.vm_available && !sample.swap_available);
    }
    let host = FakeHost::default().command("/usr/bin/memory_pressure -Q", "no percentage here\n");
    let mut sample = Sample::default();
    sample_macos_memory(&host, &mut sample, 16_384);
    assert_eq!(sample.availability, None);
}

#[test]
fn sampling_clears_rate_histories_when_the_model_changes() {
    let home = TempDir::new("model-switch");
    let log = home.write(
        ".omlx-coding/logs/server.log",
        "Chat completion: model=first, 10 tokens in 1.00s (10.0 tok/s)\n",
    );
    let mut collector = collector(macos_host(), Platform::MacOs, Some(home.0.clone()));
    collector.sample();
    collector.sample();
    assert_eq!(collector.generation_history.len(), 2);
    fs::write(
        &log,
        "Chat completion: model=second, 10 tokens in 1.00s (10.0 tok/s)\n",
    )
    .unwrap();
    let sample = collector.sample();
    assert_eq!(sample.llm_model, "second");
    assert_eq!(
        collector.generation_history.len(),
        1,
        "a new model starts a new throughput series"
    );
    assert!(journal(&collector)
        .iter()
        .any(|(_, summary)| summary.contains("detected oMLX · second")));
}

#[test]
fn reset_discards_baselines_history_and_journal() {
    let mut collector = collector(linux_host(), Platform::Linux, None);
    collector.sample();
    collector.sample();
    collector.reset();
    assert!(collector.previous.is_none());
    assert!(collector.generation_history.is_empty() && collector.signals.is_empty());
    assert_eq!(collector.current.updated, "waiting");
    let view = collector.view();
    assert!(view.load_history.is_empty());
    let sample = collector.sample();
    assert!(!sample.rate_ready, "reset must drop the rate baseline");
    assert!(journal(&collector)[0].1.starts_with("journal started"));
}

#[test]
fn journal_records_llm_queue_gpu_thermal_and_throughput_transitions() {
    let mut collector = collector(FakeHost::default(), Platform::Linux, None);
    let previous = Sample {
        updated: "10:00:00".into(),
        llm_provider: "oMLX".into(),
        llm_model: "a".into(),
        llm_status: "idle".into(),
        llm_source: TelemetrySource::Log,
        thermal: "no warning".into(),
        gpu_util: Some(10),
        swap_in: 5,
        ..Sample::default()
    };
    let current = Sample {
        updated: "10:00:01".into(),
        llm_provider: "oMLX".into(),
        llm_model: "b".into(),
        llm_status: "generating".into(),
        llm_source: TelemetrySource::Live,
        llm_active_requests: Some(2),
        llm_waiting_requests: Some(3),
        llm_generation_tps: Some(42.0),
        thermal: "limited 80%".into(),
        gpu_util: Some(95),
        impact: "GPU BOUND".into(),
        correlation: CorrelationInsight {
            direction: ThroughputDirection::Down,
            cause: CorrelationCause::Thermal,
            confidence: 85,
            summary: "throughput fell".into(),
            details: "thermal".into(),
            event_key: Some(CorrelationKey {
                direction: ThroughputDirection::Down,
                cause: CorrelationCause::Thermal,
            }),
            delta_percent: Some(-20.0),
        },
        ..Sample::default()
    };
    collector.record_journal_events(Some(&previous), &current);
    let events = journal(&collector);
    let has = |state: &str, text: &str| {
        events
            .iter()
            .any(|(s, summary)| s == state && summary.contains(text))
    };
    assert!(has("GPU BOUND", ""));
    assert!(has("LLM", "detected oMLX · b"));
    assert!(has("LLM", "status idle → generating"));
    assert!(has("LLM", "telemetry source · completion log → live API"));
    assert!(has("LLM", "request started · 2 active"));
    assert!(has("QUEUE", "3 request(s) waiting"));
    assert!(has(
        "LLM",
        "throughput diagnosis · throughput fell · high confidence"
    ));
    assert!(has("PAGING", "cleared"));
    assert!(has("THERMAL", "limited 80%"));
    assert!(collector
        .signals
        .iter()
        .filter(|event| event.state == "QUEUE")
        .all(|event| event.kind == EventKind::Queue && event.tone == Tone::Yellow));

    // And back: idle, cleared queue and a measured temperature are quiet.
    collector.signals.clear();
    let recovered = Sample {
        thermal: "41°C measured".into(),
        llm_active_requests: Some(0),
        llm_waiting_requests: Some(0),
        llm_source: TelemetrySource::Log,
        ..current.clone()
    };
    collector.record_journal_events(Some(&current), &recovered);
    let events = journal(&collector);
    assert!(events
        .iter()
        .any(|(_, s)| s == "request completed · serving is idle"));
    assert!(events.iter().any(|(_, s)| s == "queue cleared"));
    assert!(!events.iter().any(|(state, _)| state == "THERMAL"));
    assert!(collector
        .signals
        .iter()
        .any(|event| event.summary.contains("live API → completion log")
            && event.tone == Tone::Yellow));
}

#[test]
fn journal_is_bounded_by_the_history_limit() {
    let mut collector = collector(FakeHost::default(), Platform::Linux, None);
    let mut previous = Sample::default();
    for index in 0..100 {
        let next = Sample {
            llm_status: format!("status-{index}"),
            ..Sample::default()
        };
        collector.record_journal_events(Some(&previous), &next);
        previous = next;
    }
    assert_eq!(
        collector.signals.len(),
        60,
        "the journal follows a 60-sample history"
    );
    collector.history_limit = 5;
    collector.record_journal_events(Some(&previous), &Sample::default());
    assert_eq!(
        collector.signals.len(),
        40,
        "but always keeps at least 40 events"
    );
    assert_eq!(
        collector.signals.back().unwrap().summary,
        "status status-99 → offline · not detected"
    );
}

fn recv_view(sampler: &Sampler) -> CollectorView {
    sampler
        .views
        .recv_timeout(Duration::from_secs(10))
        .expect("sampler view")
}

#[test]
fn sampler_thread_follows_pause_interval_reset_and_stop_commands() {
    let sampler = Sampler::start(Duration::from_secs(3600), || {
        collector(linux_host(), Platform::Linux, None)
    });
    let first = recv_view(&sampler);
    assert_eq!(first.current.updated, "08:00:00");
    assert_eq!(first.load_history.len(), 1);

    // A short interval produces the next sample without user input.
    sampler.send(SamplerCommand::SetInterval(Duration::from_millis(10)));
    let second = recv_view(&sampler);
    assert!(second.current.rate_ready);
    assert_eq!(second.load_history.len(), 2);

    sampler.send(SamplerCommand::SetInterval(Duration::from_secs(3600)));
    sampler.send(SamplerCommand::SetPaused(true));
    while sampler
        .views
        .recv_timeout(Duration::from_millis(300))
        .is_ok()
    {}
    assert!(
        sampler
            .views
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "a paused sampler sends nothing"
    );
    sampler.send(SamplerCommand::SetPaused(false));
    let resumed = recv_view(&sampler);
    assert!(resumed.load_history.len() >= 3);

    sampler.send(SamplerCommand::Reset);
    let reset = recv_view(&sampler);
    assert_eq!(reset.load_history.len(), 1, "reset clears history");
    assert!(!reset.current.rate_ready);
    sampler.send(SamplerCommand::Stop);
    assert!(sampler.views.recv_timeout(Duration::from_secs(10)).is_err());
    drop(sampler);
}

#[test]
fn sampler_stops_when_either_channel_closes() {
    let mut sampler = Sampler::start(Duration::from_millis(5), || {
        collector(linux_host(), Platform::Linux, None)
    });
    recv_view(&sampler);
    let (_sender, replacement) = mpsc::channel();
    drop(std::mem::replace(&mut sampler.views, replacement));
    let handle = sampler.handle.take().unwrap();
    handle
        .join()
        .expect("sampler exits once nobody reads views");

    let mut sampler = Sampler::start(Duration::from_secs(3600), || {
        collector(linux_host(), Platform::Linux, None)
    });
    recv_view(&sampler);
    let (replacement, _receiver) = mpsc::channel();
    drop(std::mem::replace(&mut sampler.commands, replacement));
    let handle = sampler.handle.take().unwrap();
    handle
        .join()
        .expect("sampler exits once the command channel closes");
}

#[test]
fn sampler_panic_is_contained_and_reported_as_a_disconnect() {
    let host = linux_host().panic_on("/bin/date +%H:%M:%S %z");
    let sampler = Sampler::start(Duration::from_secs(1), move || {
        collector(host, Platform::Linux, None)
    });
    assert!(matches!(
        sampler.views.recv_timeout(Duration::from_secs(10)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    let mut app = test_app_with(sampler);
    app.tick();
    assert!(app.sampler_disconnected);
    app.tick();
    assert!(
        app.sampler_disconnected,
        "the disconnect is reported once and kept"
    );
}

fn test_app_with(sampler: Sampler) -> App {
    App::with_sampler(1, 60, Config::default(), sampler)
}

#[test]
fn app_applies_new_views_and_keeps_the_selected_gpu_by_uuid() {
    let (commands, _command_receiver) = mpsc::channel();
    let (view_sender, views) = mpsc::channel();
    let sampler = Sampler {
        commands,
        views,
        handle: None,
    };
    let mut app = App::with_sampler(
        3,
        120,
        Config {
            gpu_warn_load: Some(10),
            ..Config::default()
        },
        sampler,
    );
    assert_eq!(app.interval, Duration::from_secs(3));
    assert_eq!(app.thresholds.gpu_warn_load, 10);
    assert_eq!(app.tab, 0);
    let device = |index: u32, uuid: &str| gpu::Device {
        index,
        uuid: uuid.into(),
        name: "card".into(),
        utilization: Some(1),
        used: None,
        total: None,
        temperature: None,
        throttle_reasons: None,
    };
    let mut view = empty_view();
    view.current.gpus = vec![device(0, "GPU-a"), device(1, "GPU-b")];
    view_sender.send(view.clone()).unwrap();
    app.tick();
    app.gpu_selected = 1;
    view.current.gpus = vec![device(0, "GPU-b"), device(1, "GPU-c"), device(2, "GPU-a")];
    view_sender.send(view.clone()).unwrap();
    app.tick();
    assert_eq!(app.gpu_selected, 0, "selection follows the UUID");
    app.gpu_selected = 2;
    view.current.gpus = vec![device(0, "GPU-z")];
    view_sender.send(view).unwrap();
    app.tick();
    assert_eq!(app.gpu_selected, 0, "an unknown selection is clamped");
    drop(view_sender);
    app.tick();
    assert!(app.sampler_disconnected);
}

#[test]
fn app_new_spawns_a_live_sampler_on_this_host() {
    let mut app = App::new(1, 20, offline_config());
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.collector.current.updated == "waiting" && Instant::now() < deadline {
        app.tick();
        thread::sleep(Duration::from_millis(20));
    }
    assert_ne!(app.collector.current.updated, "waiting");
    assert!(app.collector.current.total_memory > 0);
    assert_eq!(app.collector.signals.front().unwrap().state, "SYSTEM");
}

// --- oMLX client ----------------------------------------------------------

fn omlx_client(port: u16, home: Option<PathBuf>) -> LlmTelemetryClient {
    let config = Config {
        omx: Some(OmxConfig {
            host: Some("127.0.0.1".into()),
            port: Some(port),
        }),
        ..Config::default()
    };
    LlmTelemetryClient::from_config(&config, home)
}

const HEALTH: &str = r#"{"status":"healthy","default_model":"qwen","engine_pool":{"current_model_memory":1000,"final_ceiling":2000}}"#;
const STATS: &str = r#"{"avg_generation_tps":31.5,"avg_prefill_tps":900.0,"cache_efficiency":50.0,"total_prompt_tokens":100,"total_cached_tokens":50}"#;

#[test]
fn configured_omlx_failure_is_not_mistaken_for_no_runtime() {
    let (port, server) = serve(vec![reply("GET /health ", 503, "{}")]);
    let mut client = omlx_client(port, None);
    assert!(client.poll(None).is_none());
    let report = client.report();
    assert_eq!(report.provider.as_deref(), Some("oMLX"));
    assert_eq!(report.selection, "oMLX endpoint config");
    assert!(report.failed());
    server.join().unwrap();
}

#[test]
fn omlx_client_logs_in_polls_stats_and_caches_metadata() {
    let home = TempDir::new("omlx-auth");
    home.write(
        ".config/omlx-coding/server.env",
        "export API_KEY=\"secret-key\"\nPORT=1\n",
    );
    let (port, server) = serve(vec![
        reply("GET /health ", 200, HEALTH),
        Exchange {
            request: "POST /admin/api/login ",
            status: 200,
            headers: "Set-Cookie: session=abc; Path=/; HttpOnly\r\n",
            body: "{}".into(),
        },
        reply("GET /admin/api/stats?scope=session ", 200, STATS),
        reply(
            "GET /admin/api/device-info ",
            200,
            r#"{"device_name":"Apple M4","memory_size":68719476736}"#,
        ),
        reply("GET /admin/api/global-settings ", 404, "{}"),
        reply(
            "GET /admin/api/settings ",
            200,
            r#"{"mlx_version":"0.29.1"}"#,
        ),
    ]);
    let mut client = omlx_client(port, Some(home.0.clone()));
    let telemetry = client.poll(None).expect("live telemetry");
    let requests = server.join().unwrap();
    assert!(requests[1].contains(r#""api_key":"secret-key""#));
    assert!(requests[1].contains("Content-Length: "));
    assert!(requests[2].contains("Cookie: session=abc\r\n"));
    assert_eq!(telemetry.source, TelemetrySource::Live);
    assert_eq!(telemetry.provider.as_deref(), Some("oMLX"));
    assert_eq!(telemetry.model.as_deref(), Some("qwen"));
    assert_eq!(telemetry.generation_tps, Some(31.5));
    assert_eq!(telemetry.prefill_tps, Some(900.0));
    assert_eq!(telemetry.model_memory, Some(1000));
    assert_eq!(telemetry.mlx.device_name.as_deref(), Some("Apple M4"));
    assert_eq!(telemetry.mlx.memory_size, Some(64 * 1024 * MIB));
    assert!(telemetry.observed_at.is_some());
    assert_eq!(client.session_cookie.as_deref(), Some("session=abc"));
    assert_eq!(client.last_stats_available, Some(true));

    // Within the poll interval the cached reading is reused without I/O.
    let cached = client.poll(None).expect("cached telemetry");
    assert_eq!(cached.model.as_deref(), Some("qwen"));
}

#[test]
fn omlx_client_retries_an_expired_session_once() {
    let home = TempDir::new("omlx-expired");
    home.write(".config/omlx-coding/server.env", "API_KEY=k\n");
    let (port, server) = serve(vec![
        reply("GET /health ", 200, HEALTH),
        reply("GET /admin/api/stats?scope=session ", 401, "{}"),
        Exchange {
            request: "POST /admin/api/login ",
            status: 200,
            headers: "set-cookie: session=new\r\n",
            body: "{}".into(),
        },
        reply("GET /admin/api/stats?scope=session ", 200, STATS),
    ]);
    let mut client = omlx_client(port, Some(home.0.clone()));
    client.session_cookie = Some("session=old".into());
    client.next_metadata_poll = Instant::now() + Duration::from_secs(3600);
    let telemetry = client.poll_once().expect("telemetry after re-login");
    let requests = server.join().unwrap();
    assert!(requests[1].contains("Cookie: session=old"));
    assert!(requests[3].contains("Cookie: session=new"));
    assert_eq!(telemetry.generation_tps, Some(31.5));
}

#[test]
fn omlx_client_without_credentials_still_reports_health() {
    let (port, server) = serve(vec![reply("GET /health ", 200, HEALTH)]);
    let mut client = omlx_client(port, None);
    client.next_metadata_poll = Instant::now() + Duration::from_secs(3600);
    let telemetry = client.poll_once().expect("health-only telemetry");
    server.join().unwrap();
    assert_eq!(telemetry.model.as_deref(), Some("qwen"));
    assert_eq!(telemetry.generation_tps, None, "stats need a session");
    assert_eq!(client.last_stats_available, Some(false));
}

#[test]
fn omlx_client_rejects_failed_or_unrecognized_health() {
    for (status, body) in [
        (503, HEALTH),
        (200, "not json"),
        (200, r#"{"status":"ok"}"#),
    ] {
        let (port, server) = serve(vec![reply("GET /health ", status, body)]);
        let mut client = omlx_client(port, None);
        client.last_stats_available = Some(true);
        assert!(client.poll_once().is_none(), "{status} {body}");
        assert_eq!(client.last_stats_available, None);
        server.join().unwrap();
    }
}

#[test]
fn omlx_client_backs_off_while_the_server_is_unreachable() {
    let config = offline_config();
    let mut client = LlmTelemetryClient::from_config(&config, None);
    assert!(client.poll(None).is_none());
    assert_eq!(client.retry_backoff, Duration::from_secs(2));
    client.next_poll = Instant::now();
    assert!(client.poll(None).is_none());
    assert_eq!(client.retry_backoff, Duration::from_secs(4));
    for _ in 0..10 {
        client.next_poll = Instant::now();
        client.poll(None);
    }
    assert_eq!(
        client.retry_backoff,
        Duration::from_secs(30),
        "backoff is capped"
    );
    // A cached success survives a failed poll.
    client.cached = Some(LlmTelemetry {
        model: Some("kept".into()),
        ..LlmTelemetry::default()
    });
    client.next_poll = Instant::now();
    assert_eq!(client.poll(None).unwrap().model.as_deref(), Some("kept"));
}

#[test]
fn omlx_login_requires_a_loopback_host_and_a_key() {
    let home = TempDir::new("omlx-remote");
    home.write(".config/omlx-coding/server.env", "API_KEY=k\n");
    let mut client = omlx_client(1, Some(home.0.clone()));
    client.host = "192.0.2.10".into();
    client.login();
    assert!(client.session_cookie.is_none(), "keys never leave the host");
    assert!(client.fetch_json("/admin/api/stats").is_none());

    // Loopback with a key, but the login endpoint refuses.
    let (port, server) = serve(vec![reply("POST /admin/api/login ", 403, "{}")]);
    let mut client = omlx_client(port, Some(home.0.clone()));
    client.login();
    server.join().unwrap();
    assert!(client.session_cookie.is_none());

    assert_eq!(read_omlx_api_key(None), None);
    let empty = TempDir::new("omlx-empty-key");
    empty.write(".config/omlx-coding/server.env", "API_KEY=\"\"\n");
    assert_eq!(read_omlx_api_key(Some(&empty.0)), None);
}

#[test]
fn omlx_fetch_rejects_non_success_and_malformed_bodies() {
    let (port, server) = serve(vec![
        reply("GET /a ", 500, "{}"),
        reply("GET /b ", 200, "not json"),
        reply("GET /c ", 401, "{}"),
    ]);
    let mut client = omlx_client(port, None);
    client.session_cookie = Some("s=1".into());
    assert!(client.fetch_json("/a").is_none());
    assert!(client.fetch_json("/b").is_none());
    assert!(client.fetch_json("/c").is_none(), "no key to log in again");
    assert!(client.session_cookie.is_none());
    server.join().unwrap();

    let home = TempDir::new("omlx-retry-fail");
    home.write(".config/omlx-coding/server.env", "API_KEY=k\n");
    let (port, server) = serve(vec![
        reply("GET /d ", 401, "{}"),
        Exchange {
            request: "POST /admin/api/login ",
            status: 200,
            headers: "Set-Cookie: s=2\r\n",
            body: "{}".into(),
        },
        reply("GET /d ", 500, "{}"),
    ]);
    let mut client = omlx_client(port, Some(home.0.clone()));
    client.session_cookie = Some("s=1".into());
    assert!(client.fetch_json("/d").is_none());
    server.join().unwrap();
}

#[test]
fn http_request_rejects_oversized_and_malformed_responses() {
    use std::net::TcpListener;
    let respond = |payload: Vec<u8>| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0; 1024];
            let _ = stream.read(&mut bytes);
            let _ = stream.write_all(&payload);
        });
        let response = http_request("127.0.0.1", port, "GET", "/", &[], None);
        server.join().unwrap();
        response
    };
    let mut huge = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
    huge.resize(MAX_HTTP_RESPONSE_BYTES + 10, b'x');
    assert!(respond(huge).is_err());
    assert!(respond(b"HTTP/1.1 200 OK\r\nno blank line".to_vec()).is_err());
    assert!(respond(b"HTTP/1.1 abc OK\r\n\r\n".to_vec()).is_err());
    let response =
        respond(b"HTTP/1.1 204 No Content\r\nX-Test: yes\r\nbad header\r\n\r\n".to_vec())
            .expect("valid response");
    assert_eq!(response.status, 204);
    assert_eq!(response.header("x-test"), Some("yes"));
    assert_eq!(response.header("missing"), None);
    assert!(http_request("host.invalid.", 1, "GET", "/", &[], None).is_err());
}

#[test]
fn omlx_endpoint_discovery_reads_server_env_and_honors_config() {
    let home = TempDir::new("omlx-endpoint");
    home.write(
        ".config/omlx-coding/server.env",
        "HOST=\"10.0.0.5\"\nPORT=9001\n",
    );
    assert_eq!(
        read_omlx_endpoint(&Config::default(), Some(&home.0)),
        ("10.0.0.5".to_string(), 9001)
    );
    let configured = Config {
        omx: Some(OmxConfig {
            host: Some("127.0.0.1".into()),
            port: Some(8000),
        }),
        ..Config::default()
    };
    assert_eq!(
        read_omlx_endpoint(&configured, Some(&home.0)),
        ("127.0.0.1".to_string(), 8000)
    );
    assert_eq!(
        read_omlx_endpoint(&Config::default(), None),
        (DEFAULT_OMLX_HOST.to_string(), DEFAULT_OMLX_PORT)
    );
}

#[test]
fn completion_logs_pick_the_newest_parsable_result() {
    assert_eq!(read_llm_stats(None).model, None);
    let home = TempDir::new("omlx-logs");
    assert_eq!(read_llm_stats(Some(&home.0)).model, None, "no logs yet");
    home.write(
        ".omlx-coding/logs/launchd.stdout.log",
        "Responses API: model=older, 5 tokens in 1.0s (5.0 tok/s)\n",
    );
    thread::sleep(Duration::from_millis(20));
    home.write(
        ".omlx-coding/logs/server.log",
        "Chat completion: model=newer, 7 tokens in 1.0s (7.0 tok/s), prompt: 3, cached: 0\nunrelated\n",
    );
    home.write(".omlx-coding/logs/launchd.stderr.log", "no completions\n");
    let stats = read_llm_stats(Some(&home.0));
    assert_eq!(stats.model.as_deref(), Some("newer"));
    assert_eq!(stats.prompt_tokens, Some(3));
    let older = read_latest_completion(&home.0.join(".omlx-coding/logs/launchd.stdout.log"))
        .expect("older completion");
    assert_eq!(older.model.as_deref(), Some("older"));
    assert_eq!(older.prompt_tokens, None);
    assert!(read_latest_completion(&home.0.join("missing.log")).is_none());
}
