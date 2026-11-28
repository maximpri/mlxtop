use crate::test_support::*;
// SPDX-License-Identifier: MIT
// Collector, sampler and oMLX client behavior driven through the host seam.
use super::*;
use crate::runtime_diagnostics::ProbeIssue;

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
        "noise\nINFO Chat completion: model=qwen-coder, 120 tokens in 3.00s (60.0 tok/s), prompt: 500, cached: 0\n",
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
    // The memory panel's GPU history holds what its meter read.
    assert_eq!(
        collector.gpu_memory_history.back().copied().flatten(),
        Some(2 * 1024 * MIB)
    );
    assert_eq!(first.metal.renderer_util, Some(91));
    assert_eq!(first.metal.tiler_util, Some(12));
    assert_eq!(first.thermal_state.level, Some(ThermalLevel::Nominal));
    assert_eq!(first.thermal, "nominal");
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
    assert!(calls.contains(&"/usr/bin/notifyutil -g com.apple.system.thermalpressurelevel".into()));
    // Apple Silicon's pmset reports no CPU limit: with the level read, it
    // is not run.
    assert!(!calls.iter().any(|call| call.starts_with("/usr/bin/pmset")));
    assert!(!calls.iter().any(|call| call.starts_with("read /proc")));
}

#[test]
fn a_sample_carries_when_it_was_taken_and_its_recording_format() {
    let mut collector = collector(macos_host(), Platform::MacOs, None);
    let before = epoch_millis(SystemTime::now()).unwrap();
    let sample = collector.sample();
    let after = epoch_millis(SystemTime::now()).unwrap();
    // The time of day repeats every day; the epoch time does not.
    assert_eq!(sample.updated, "12:00:01");
    let ts = sample.ts.expect("a sample knows when it was taken");
    assert!(
        (before..=after).contains(&ts),
        "{before} <= {ts} <= {after}"
    );
    assert_eq!(sample.schema, SAMPLE_SCHEMA);
    assert!(sample_log_details(&sample, 3).ends_with(&format!(" ts={ts}")));
    let before_epoch = SystemTime::UNIX_EPOCH
        .checked_sub(Duration::from_secs(1))
        .unwrap();
    assert_eq!(epoch_millis(before_epoch), None);
}

#[test]
fn the_sample_log_line_keeps_its_fields_and_adds_new_ones_at_the_end() {
    let sample = Sample {
        pressure: "YELLOW".into(),
        thermal: "limited 60%".into(),
        system_power_mw: Some(61_250),
        llm_draft_acceptance: Some(0.78),
        ts: Some(1_794_719_479_668),
        ..Sample::default()
    };
    let line = sample_log_details(&sample, 12);
    // Scripts read fields by name after `event=sample`: the earlier ones
    // keep their order and the new ones follow them.
    let keys: Vec<&str> = line
        .split(' ')
        .filter_map(|field| field.split_once('='))
        .map(|(key, _)| key)
        .collect();
    assert_eq!(
        keys,
        [
            "duration_ms",
            "status",
            "provider",
            "model",
            "source",
            "gen_live",
            "gen_tps",
            "prefill_live",
            "prefill_tps",
            "cache",
            "active",
            "waiting",
            "gpu",
            "renderer",
            "tiler",
            "ram_resident_percent",
            "vm_availability",
            "paging_in",
            "paging_out",
            "compress",
            "decompress",
            "generating",
            "context",
            "writers",
            "disk_read",
            "disk_write",
            "disk_weights",
            "ssd_loads",
            "ssd_saves",
            "pressure",
            "thermal",
            "power_mw",
            "draft",
            "ts",
        ]
    );
    assert!(line.starts_with("duration_ms=12 status=offline "), "{line}");
    assert!(
        line.ends_with(
            " pressure=YELLOW thermal=limited_60% power_mw=61250 draft=0.780 ts=1794719479668"
        ),
        "{line}"
    );
    // What was not read is na, never zero or a made-up word.
    let unread = sample_log_details(&Sample::default(), 0);
    assert!(
        unread.ends_with(" pressure=na thermal=na power_mw=na draft=na ts=na"),
        "{unread}"
    );
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
    // Cached + Buffers, not also the file LRU lists that count the same
    // pages; cache and free add up to MemAvailable.
    assert_eq!(first.file_backed, 3_100_000 * 1024);
    let sizes = crate::memory_composition::segments(&first).unwrap();
    assert_eq!(crate::memory_headroom::left(sizes), 4_000_000 * 1024);
    assert_eq!(
        (first.swap_total, first.swap_used),
        (4_000_000 * 1024, 1_000_000 * 1024)
    );
    if cfg!(any(target_os = "linux", target_os = "windows")) {
        assert_eq!(first.gpus.len(), 1);
        assert_eq!(first.gpus[0].uuid, "GPU-aaaa");
        assert_eq!(first.gpu_util, Some(64));
    } else {
        assert!(first.gpus.is_empty(), "NVIDIA collection is off on macOS");
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
    if cfg!(any(target_os = "linux", target_os = "windows")) {
        assert_eq!(second.gpus[0].temperature, Some(61));
    } else {
        assert!(second.gpus.is_empty());
    }
}

#[test]
fn windows_collector_reads_memory_processes_and_nvidia_without_inventing_paging() {
    let host = windows_host();
    let mut collector = collector(host.clone(), Platform::Windows, None);
    assert_eq!(collector.total_memory, 32 * 1024 * MIB);
    assert_eq!(collector.page_size, 4096);
    assert!(collector.metal.architecture.is_some());

    let first = collector.sample();
    assert_eq!(first.availability, Some(25));
    assert_eq!(first.pressure, "YELLOW", "75% in use warns");
    assert_eq!(first.resident_memory, Some(24 * 1024 * MIB));
    assert_eq!(
        (first.swap_total, first.swap_used),
        (16 * 1024 * MIB, 2 * 1024 * MIB)
    );
    assert_eq!(first.llm_provider, "Ollama");
    assert_eq!(first.llm_count, 1);
    assert_eq!(first.llm_pid, 4012);
    assert_eq!(first.llm_rss, 9 * 1024 * MIB);
    assert_eq!(first.largest_consumer.as_deref(), Some("ollama.exe"));
    // mlxtop does not count itself as a memory consumer.
    assert_eq!(collector.current.llm_processes.len(), 1);
    assert!(!host.calls().iter().any(|call| call.contains("/bin/date")));
    assert_ne!(first.updated, "waiting");

    let second = collector.sample();
    assert!(second.rate_ready);
    assert!(second.paging_unavailable);
    assert!(!second.paging_measured());
    assert_eq!(second.swap_in, 0);
    let mut report = Vec::new();
    crate::report::write_static(
        &mut report,
        &second,
        1,
        Thresholds::default(),
        Platform::Windows,
    )
    .unwrap();
    let report = String::from_utf8(report).unwrap();
    assert!(report.contains("in — · out —"), "{report}");
    if cfg!(any(target_os = "linux", target_os = "windows")) {
        assert_eq!(second.gpus[0].name, "NVIDIA GeForce RTX 4080");
        assert_eq!(second.thermal, "83°C measured");
        // 97% VRAM outranks the thermal slowdown on the same card.
        assert_eq!(second.impact, "VRAM FULL");
    } else {
        assert!(second.gpus.is_empty());
        assert_eq!(second.thermal, "unavailable");
    }
}

#[test]
fn missing_host_telemetry_stays_unavailable_instead_of_zero() {
    for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
        let mut collector = collector(FakeHost::default(), platform, None);
        let sample = collector.sample();
        if platform != Platform::Windows {
            assert_eq!(sample.updated, "??:??:??");
        }
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
        "Chat completion: model=first, 10 tokens in 2.00s (10.0 tok/s)\n",
    );
    let mut collector = collector(macos_host(), Platform::MacOs, Some(home.0.clone()));
    collector.sample();
    collector.sample();
    assert_eq!(collector.generation_history.len(), 2);
    fs::write(
        &log,
        "Chat completion: model=second, 10 tokens in 2.00s (10.0 tok/s)\n",
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
    assert!(collector.session.started.is_some());
    collector.reset();
    assert!(collector.previous.is_none());
    assert!(collector.generation_history.is_empty() && collector.signals.is_empty());
    assert_eq!(collector.current.updated, "waiting");
    let view = collector.view();
    assert!(view.load_history.is_empty());
    assert!(view.session.started.is_none(), "the session starts over");
    // What the view is drawn for is the machine's, not the session's.
    assert_eq!(view.platform, Platform::Linux);
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
            summary: "throughput fell".into(),
            details: "thermal".into(),
            lead: "thermal limit".into(),
            stall_seconds: None,
            event_key: Some(CorrelationKey {
                direction: ThroughputDirection::Down,
                cause: CorrelationCause::Thermal,
            }),
            delta_percent: Some(-20.0),
            ..CorrelationInsight::default()
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
    // What happened, what else was unusual, and what to do, in one row.
    assert!(has(
        "SLOWDOWN",
        "throughput fell · thermal limit → macOS is limiting speed for heat"
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
        vendor: gpu::Vendor::Nvidia,
        name: "card".into(),
        utilization: Some(1),
        used: None,
        total: None,
        temperature: None,
        throttle_reasons: None,
        unified: false,
        limits: Default::default(),
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
    let (port, server) = serve(vec![
        reply("GET /health ", 200, HEALTH),
        reply("GET /admin/api/stats?scope=session ", 401, "{}"),
    ]);
    let mut client = omlx_client(port, None);
    client.next_metadata_poll = Instant::now() + Duration::from_secs(3600);
    let telemetry = client.poll_once().expect("health-only telemetry");
    let requests = server.join().unwrap();
    assert!(!requests[1].contains("Cookie") && !requests[1].contains("Authorization"));
    assert_eq!(telemetry.model.as_deref(), Some("qwen"));
    assert_eq!(telemetry.generation_tps, None, "stats need a session");
    assert_eq!(client.last_stats_available, Some(false));
    let login = client
        .runtime
        .probes
        .iter()
        .find(|probe| probe.path == "/admin/api/login")
        .unwrap();
    assert_eq!(login.issue, Some(ProbeIssue::CredentialsMissing));
    assert!(
        !client
            .runtime
            .probes
            .iter()
            .any(|probe| probe.path.starts_with("/admin/api/stats")),
        "the missing key explains the statistics"
    );

    // Within a minute the refusal stands: nothing is asked again.
    let (port, server) = serve(vec![reply("GET /health ", 200, HEALTH)]);
    client.port = port;
    client.poll_once().expect("health-only telemetry");
    assert_eq!(server.join().unwrap().len(), 1);
}

#[test]
fn omlx_without_authentication_serves_statistics_without_a_login() {
    // oMLX's "Disable all authentication" on a loopback bind, no key set.
    let (port, server) = serve(vec![
        reply("GET /health ", 200, HEALTH),
        reply("GET /admin/api/stats?scope=session ", 200, STATS),
        reply("GET /health ", 200, HEALTH),
        reply("GET /admin/api/stats?scope=session ", 200, STATS),
    ]);
    let mut client = omlx_client(port, None);
    client.next_metadata_poll = Instant::now() + Duration::from_secs(3600);
    let telemetry = client.poll_once().expect("live telemetry");
    assert_eq!(telemetry.generation_tps, Some(31.5));
    assert_eq!(telemetry.prefill_tps, Some(900.0));
    assert!(client
        .runtime
        .probes
        .iter()
        .all(|probe| probe.issue.is_none()));
    client.poll_once().expect("live telemetry again");
    let requests = server.join().unwrap();
    assert!(requests
        .iter()
        .all(|request| !request.contains("/admin/api/login")));
    assert!(client.session_cookie.is_none());
}

#[test]
fn a_key_omlx_does_not_have_falls_back_to_statistics_without_a_login() {
    let home = TempDir::new("omlx-no-server-key");
    home.write(".config/omlx-coding/server.env", "API_KEY=unneeded\n");
    let no_key = r#"{"detail":"No API key configured. Please set up an API key first."}"#;
    let (port, server) = serve(vec![
        reply("GET /health ", 200, HEALTH),
        reply("POST /admin/api/login ", 400, no_key),
        reply("GET /admin/api/stats?scope=session ", 200, STATS),
    ]);
    let mut client = omlx_client(port, Some(home.0.clone()));
    client.next_metadata_poll = Instant::now() + Duration::from_secs(3600);
    let telemetry = client.poll_once().expect("live telemetry");
    let requests = server.join().unwrap();
    assert!(!requests[2].contains("Cookie") && !requests[2].contains("Authorization"));
    assert_eq!(telemetry.generation_tps, Some(31.5));
    assert!(client
        .runtime
        .probes
        .iter()
        .all(|probe| probe.issue.is_none()));

    // When the statistics need a login after all, doctor names the reason
    // and the login is not repeated within the minute.
    let (port, server) = serve(vec![
        reply("GET /health ", 200, HEALTH),
        reply("POST /admin/api/login ", 400, no_key),
        reply("GET /admin/api/stats?scope=session ", 401, "{}"),
        reply("GET /health ", 200, HEALTH),
    ]);
    let mut client = omlx_client(port, Some(home.0.clone()));
    client.next_metadata_poll = Instant::now() + Duration::from_secs(3600);
    client.poll_once().expect("health-only telemetry");
    let login = |client: &LlmTelemetryClient| {
        client
            .runtime
            .probes
            .iter()
            .find(|probe| probe.path == "/admin/api/login")
            .and_then(|probe| probe.issue.clone())
    };
    assert_eq!(login(&client), Some(ProbeIssue::NoServerKey));
    client.runtime.begin();
    client.poll_once().expect("health-only telemetry");
    assert_eq!(login(&client), Some(ProbeIssue::NoServerKey));
    assert_eq!(server.join().unwrap().len(), 4);
    assert!(ProbeIssue::NoServerKey
        .to_string()
        .contains("Disable all authentication"));
}

#[test]
fn an_omlx_lockout_is_not_asked_again_within_the_minute() {
    let home = TempDir::new("omlx-lockout");
    home.write(".config/omlx-coding/server.env", "API_KEY=k\n");
    let (port, server) = serve(vec![reply("POST /admin/api/login ", 429, "{}")]);
    let mut client = omlx_client(port, Some(home.0.clone()));
    client.login();
    client.login();
    assert_eq!(server.join().unwrap().len(), 1);
    assert!(client.session_cookie.is_none());
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
        Duration::from_secs(4),
        "a refused connection is retried within seconds"
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
fn omlx_client_waits_longer_for_a_runtime_that_accepts_and_never_answers() {
    // The operating system completes the handshake for a server that
    // listens but never accepts: a hung runtime. It times out when read,
    // on every platform, and is asked again later than a refused one.
    let hung = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = omlx_client(hung.local_addr().unwrap().port(), None);
    client.retry_backoff = Duration::from_secs(16);
    assert!(client.poll(None).is_none());
    assert_eq!(client.retry_backoff, Duration::from_secs(30));
    drop(hung);
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
fn a_stock_omlx_is_found_from_its_own_settings_and_port_variable() {
    assert_eq!(DEFAULT_OMLX_PORT, 8000, "oMLX's own default");
    let home = TempDir::new("omlx-settings");
    home.write(
        ".omlx/settings.json",
        r#"{"server":{"host":"0.0.0.0","port":8123}}"#,
    );
    let found = discover_omlx_endpoint(Some(&home.0), None, None);
    assert_eq!(found.port, Some(8123));
    assert_eq!(
        found.host, None,
        "a wildcard bind is not an address to call"
    );
    assert_eq!(
        discover_omlx_endpoint(Some(&home.0), None, Some("8200")).port,
        Some(8200),
        "OMLX_PORT overrides the settings file"
    );
    let base = TempDir::new("omlx-base");
    base.write(
        "settings.json",
        r#"{"server":{"host":"10.0.0.7","port":8300}}"#,
    );
    let moved = discover_omlx_endpoint(Some(&home.0), Some(&base.0), Some("bad"));
    assert_eq!(moved.port, Some(8300));
    assert_eq!(moved.host.as_deref(), Some("10.0.0.7"));
    home.write(
        ".config/omlx-coding/server.env",
        "PORT=8080
",
    );
    assert_eq!(
        discover_omlx_endpoint(Some(&home.0), None, Some("8200")).port,
        Some(8080),
        "a server.env written for oMLX wins"
    );
    assert_eq!(
        parse_omlx_settings("not json"),
        DiscoveredEndpoint::default()
    );
    assert_eq!(
        parse_omlx_settings(r#"{"server":{"port":70000}}"#).port,
        None
    );
    assert_eq!(
        discover_omlx_endpoint(None, None, None),
        DiscoveredEndpoint::default()
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

#[test]
fn omlx_serving_through_splash_hides_mlx_readings_and_flags_untracked_work() {
    use crate::collector::apply_hosted_splash;
    let omlx = |status: &str, active: Option<u64>, gpu: Option<u8>| {
        let mut sample = Sample {
            llm_provider: "oMLX".into(),
            llm_status: status.into(),
            llm_active_requests: active,
            llm_waiting_requests: Some(0),
            gpu_util: gpu,
            ..Sample::default()
        };
        sample.mlx.version = Some("0.31.4".into());
        sample.mlx.active_memory = Some(1 << 30);
        sample.mlx.peak_memory = Some(2 << 30);
        sample.mlx.process_footprint = Some(3 << 30);
        sample.mlx.resource_limit = Some(32 << 30);
        sample.mlx.device_name = Some("Apple M4 Max".into());
        sample
    };

    // The model lives in Splash: oMLX's MLX runtime readings go, device facts stay.
    let mut sample = omlx("idle", Some(0), Some(5));
    apply_hosted_splash(&mut sample, Thresholds::default());
    assert_eq!(sample.llm_engine.as_deref(), Some("Splash"));
    assert_eq!(sample.mlx.version, None);
    assert_eq!(sample.mlx.active_memory, None);
    assert_eq!(sample.mlx.peak_memory, None);
    assert_eq!(sample.mlx.process_footprint, None);
    assert_eq!(sample.mlx.resource_limit, Some(32 << 30));
    assert_eq!(sample.mlx.device_name.as_deref(), Some("Apple M4 Max"));
    assert_eq!(sample.llm_status, "idle");

    // A saturated GPU with nothing tracked is work oMLX cannot see (its
    // benchmark talks to Splash directly), not an idle server.
    for status in ["idle", "healthy", "ready"] {
        let mut sample = omlx(status, Some(0), Some(97));
        apply_hosted_splash(&mut sample, Thresholds::default());
        assert_eq!(sample.llm_status, "untracked", "{status}");
    }
    // Tracked work, a quiet GPU or a missing reading keep oMLX's own status.
    for (status, active, gpu) in [
        ("generating", Some(1), Some(97)),
        ("healthy", Some(1), Some(97)),
        ("idle", Some(0), Some(74)),
        ("idle", Some(0), None),
    ] {
        let mut sample = omlx(status, active, gpu);
        apply_hosted_splash(&mut sample, Thresholds::default());
        assert_eq!(sample.llm_status, status, "{status} {active:?} {gpu:?}");
    }
    let mut queued = omlx("idle", Some(0), Some(97));
    queued.llm_waiting_requests = Some(2);
    apply_hosted_splash(&mut queued, Thresholds::default());
    assert_eq!(queued.llm_status, "idle");

    // Only oMLX hosts Splash this way.
    let mut other = omlx("idle", Some(0), Some(97));
    other.llm_provider = "Ollama".into();
    apply_hosted_splash(&mut other, Thresholds::default());
    assert_eq!(other.llm_engine, None);
    assert_eq!(other.mlx.version.as_deref(), Some("0.31.4"));
}

#[test]
fn a_saved_config_changes_thresholds_and_the_endpoint_only_when_omx_changes() {
    let mut collector = collector(macos_host(), Platform::MacOs, None);
    let original = collector.llm_client.port;
    let mut config = offline_config();
    config.omx.clone_from(&collector.omx);
    config.gpu_warn_load = Some(50);
    collector.apply_config(&config);
    assert_eq!(collector.thresholds.gpu_warn_load, 50);
    assert_eq!(
        collector.llm_client.port, original,
        "same omx keeps the client"
    );
    config.omx = Some(OmxConfig {
        host: Some("127.0.0.1".into()),
        port: Some(original.wrapping_add(1).max(1)),
    });
    collector.apply_config(&config);
    assert_eq!(collector.llm_client.port, original.wrapping_add(1).max(1));
}

fn gpu_sample(load: u8) -> Sample {
    Sample {
        gpu_util: Some(load),
        ..Sample::default()
    }
}

/// Feed GPU loads one sample at a time and return the GPU journal rows.
fn gpu_rows(collector: &mut Collector, loads: &[u8]) -> Vec<String> {
    let mut previous = gpu_sample(loads[0]);
    collector.record_journal_events(None, &previous);
    for &load in &loads[1..] {
        let next = gpu_sample(load);
        collector.record_journal_events(Some(&previous), &next);
        previous = next;
    }
    collector
        .signals
        .iter()
        .filter(|event| event.kind == EventKind::Gpu)
        .map(|event| event.summary.clone())
        .collect()
}

#[test]
fn gpu_journal_ignores_wobble_around_the_busy_and_saturated_lines() {
    let mut collector = collector(FakeHost::default(), Platform::Linux, None);
    // The reported journal: 84, 79, 81, 77 logged four rows. Saturation
    // wobbling 99 ↔ 94 is normal processing too.
    let rows = gpu_rows(
        &mut collector,
        &[20, 84, 79, 81, 77, 84, 95, 99, 94, 99, 94, 96],
    );
    assert_eq!(
        rows,
        [
            "busy burst started · 84% busy",
            "GPU load saturated · 95% busy"
        ]
    );
}

#[test]
fn gpu_journal_logs_a_real_drop_once() {
    let mut collector = collector(FakeHost::default(), Platform::Linux, None);
    let rows = gpu_rows(&mut collector, &[20, 99, 85, 75, 65, 60, 0]);
    assert_eq!(
        rows,
        [
            "GPU load saturated · 99% busy",
            "GPU load eased · 75% busy",
            "GPU load eased · 65% busy",
        ]
    );
}

#[test]
fn gpu_journal_latch_starts_over_after_reset() {
    let mut collector = collector(FakeHost::default(), Platform::Linux, None);
    gpu_rows(&mut collector, &[20, 95]);
    collector.reset();
    // A fresh session at 95% is the baseline, not a new burst.
    assert!(gpu_rows(&mut collector, &[95, 96]).is_empty());
}

#[test]
fn gpu_busy_impact_is_a_gpu_event_and_replaces_the_busy_burst_row() {
    let mut collector = collector(FakeHost::default(), Platform::Linux, None);
    let previous = Sample {
        impact: "HEALTHY".into(),
        ..gpu_sample(20)
    };
    collector.record_journal_events(None, &previous);
    let busy = Sample {
        impact: "GPU BUSY".into(),
        ..gpu_sample(84)
    };
    collector.record_journal_events(Some(&previous), &busy);
    let gpu: Vec<_> = collector
        .signals
        .iter()
        .filter(|event| event.kind == EventKind::Gpu)
        .map(|event| event.state.clone())
        .collect();
    assert_eq!(gpu, ["GPU BUSY"]);
}

#[test]
fn journal_toggles_drop_routine_events_but_keep_system_and_critical() {
    let config = Config {
        journal: Some(JournalConfig {
            gpu: Some(false),
            llm: Some(false),
            memory: Some(false),
            paging: Some(false),
            ..JournalConfig::default()
        }),
        ..offline_config()
    };
    let mut collector = Collector::with_host(
        60,
        config,
        Box::new(FakeHost::default()),
        Platform::Linux,
        None,
    );
    let previous = Sample {
        impact: "HEALTHY".into(),
        llm_status: "idle".into(),
        ..gpu_sample(20)
    };
    collector.record_journal_events(None, &previous);
    let next = Sample {
        impact: "HEAVY PAGING".into(),
        llm_status: "generating".into(),
        pressure: "YELLOW".into(),
        swap_out: 64 * MIB,
        ..gpu_sample(95)
    };
    collector.record_journal_events(Some(&previous), &next);
    let states: Vec<_> = journal(&collector).into_iter().map(|(s, _)| s).collect();
    assert_eq!(states, ["SYSTEM", "HEAVY PAGING"]);

    // Settings turns GPU rows back on without a restart.
    collector.apply_config(&offline_config());
    collector.record_journal_events(Some(&next), &gpu_sample(30));
    assert!(collector
        .signals
        .iter()
        .any(|event| event.summary == "GPU load eased · 30% busy"));
}

#[test]
fn telemetry_online_is_logged_once_while_the_rate_flickers() {
    let mut collector = collector(FakeHost::default(), Platform::Linux, None);
    let serving = |tps: Option<f64>| Sample {
        llm_provider: "mlx-serve".into(),
        llm_source: TelemetrySource::Live,
        llm_generation_tps: tps,
        ..Sample::default()
    };
    let mut previous = serving(None);
    collector.record_journal_events(None, &previous);
    for tps in [Some(0.0), None, Some(0.0), None, Some(0.0)] {
        let next = serving(tps);
        collector.record_journal_events(Some(&previous), &next);
        previous = next;
    }
    let online = |collector: &Collector| {
        collector
            .signals
            .iter()
            .filter(|event| event.summary.contains("telemetry online"))
            .count()
    };
    assert_eq!(online(&collector), 1);

    // A different provider is news again.
    let other = Sample {
        llm_provider: "oMLX".into(),
        ..serving(Some(10.0))
    };
    collector.record_journal_events(Some(&serving(None)), &other);
    assert_eq!(online(&collector), 2);
}

#[test]
fn journal_toggles_keep_findings_and_critical_pressure() {
    let config = Config {
        journal: Some(JournalConfig {
            gpu: Some(false),
            memory: Some(false),
            paging: Some(false),
            ..JournalConfig::default()
        }),
        ..offline_config()
    };
    let mut collector = Collector::with_host(
        60,
        config,
        Box::new(FakeHost::default()),
        Platform::Linux,
        None,
    );
    let mut previous = Sample {
        impact: "HEALTHY".into(),
        pressure: "YELLOW".into(),
        ..Sample::default()
    };
    collector.record_journal_events(None, &previous);
    for (impact, pressure) in [
        ("GPU THROTTLED", "YELLOW"),
        ("PAGING ACTIVE", "YELLOW"),
        ("GPU BUSY", "YELLOW"),
        // Counters missing: no MEMORY BOTTLENECK, only the pressure row.
        ("DATA LIMITED", "RED"),
    ] {
        let next = Sample {
            impact: impact.into(),
            pressure: pressure.into(),
            ..Sample::default()
        };
        collector.record_journal_events(Some(&previous), &next);
        previous = next;
    }
    let states: Vec<_> = journal(&collector).into_iter().map(|(s, _)| s).collect();
    assert_eq!(
        states,
        [
            "SYSTEM",
            "GPU THROTTLED",
            "PAGING ACTIVE",
            "DATA LIMITED",
            "PRESSURE"
        ]
    );
}

#[test]
fn telemetry_online_is_logged_again_after_the_server_returns() {
    let mut collector = collector(FakeHost::default(), Platform::Linux, None);
    let serving = |source, tps: Option<f64>| Sample {
        llm_provider: "mlx-serve".into(),
        llm_source: source,
        llm_generation_tps: tps,
        ..Sample::default()
    };
    let steps = [
        serving(TelemetrySource::Live, None),
        serving(TelemetrySource::Live, Some(0.0)),
        serving(TelemetrySource::None, None),
        serving(TelemetrySource::Live, Some(0.0)),
    ];
    collector.record_journal_events(None, &steps[0]);
    for pair in steps.windows(2) {
        collector.record_journal_events(Some(&pair[0]), &pair[1]);
    }
    let online = collector
        .signals
        .iter()
        .filter(|event| event.summary.contains("telemetry online"))
        .count();
    assert_eq!(online, 2);
}

#[test]
fn saturation_survives_gpu_busy_impact_deduplication() {
    let mut c = collector(FakeHost::default(), Platform::Linux, None);
    let before = Sample {
        impact: "LLM READY".into(),
        ..gpu_sample(20)
    };
    let after = Sample {
        impact: "GPU BUSY".into(),
        ..gpu_sample(99)
    };
    c.record_journal_events(None, &before);
    c.record_journal_events(Some(&before), &after);
    c.record_journal_events(Some(&after), &after);
    // One row carries both: the busy state and that it began saturated.
    let gpu_rows: Vec<_> = c
        .signals
        .iter()
        .filter(|e| e.state.starts_with("GPU"))
        .collect();
    assert_eq!(gpu_rows.len(), 1, "{:?}", journal(&c));
    assert!(
        gpu_rows[0].summary.ends_with(" · saturated"),
        "{:?}",
        journal(&c)
    );
}

#[test]
fn cached_tokens_count_from_when_mlxtop_started_watching() {
    let mut start = None;
    assert_eq!(crate::collector::watched(&mut start, "oMLX", 80_000_000), 0);
    assert_eq!(
        crate::collector::watched(&mut start, "oMLX", 80_030_000),
        30_000
    );
    // A restarted runtime counts again from its new total.
    assert_eq!(crate::collector::watched(&mut start, "oMLX", 5_000), 0);
    assert_eq!(crate::collector::watched(&mut start, "oMLX", 6_000), 1_000);
    // So does another runtime.
    assert_eq!(crate::collector::watched(&mut start, "vLLM", 9_000), 0);
}

#[test]
fn slow_readings_are_taken_every_few_polls_and_vm_stat_once_per_poll() {
    let host = macos_host();
    let mut collector = collector(host.clone(), Platform::MacOs, None);
    let before = host.calls().len();
    let first = collector.sample();
    let second = collector.sample();
    let calls: Vec<String> = host.calls().into_iter().skip(before).collect();
    let count = |prefix: &str| calls.iter().filter(|call| call.starts_with(prefix)).count();
    assert_eq!(count("/usr/bin/vm_stat"), 2, "one per poll: {calls:?}");
    assert_eq!(
        count("/usr/bin/notifyutil"),
        1,
        "thermal is read every few seconds"
    );
    assert_eq!(
        count("/usr/sbin/ioreg -r -d 1 -w 0 -c AppleSmartBattery"),
        1
    );
    // Between readings, the last one stands.
    assert_eq!(second.thermal, first.thermal);
    assert_eq!(second.thermal_state, first.thermal_state);
    assert_eq!(second.thermal_state.level, Some(ThermalLevel::Nominal));
    assert_eq!(second.system_power_mw, first.system_power_mw);
    // Five seconds on, they are read again.
    collector.slow_read_at = collector
        .slow_read_at
        .map(|at| at - std::time::Duration::from_secs(6));
    collector.sample();
    let again = host
        .calls()
        .into_iter()
        .skip(before)
        .filter(|c| c.starts_with("/usr/bin/notifyutil"))
        .count();
    assert_eq!(again, 2);
}

#[test]
fn a_completion_log_tail_that_starts_inside_a_character_is_still_read() {
    let home = TempDir::new("omlx-log-tail");
    // 256 KiB + 2 bytes: the tail starts one byte into a three-byte `→`.
    let mut text = String::from("x→");
    text.push_str(&"y".repeat(256 * 1024 - 2));
    text.push_str(
        "\nChat completion: model=qwen, 7 tokens in 1.0s (7.0 tok/s), prompt: 3, cached: 0\n",
    );
    home.write(".omlx-coding/logs/server.log", &text);
    let stats = read_latest_completion(&home.0.join(".omlx-coding/logs/server.log"))
        .expect("the tail must be decoded lossily");
    assert_eq!(stats.model.as_deref(), Some("qwen"));
}

fn scripted_macos_host(vm_stats: &[String], ps: &[&str]) -> FakeHost {
    let mut host = FakeHost::default()
        .command("/usr/sbin/sysctl -n hw.memsize", "38654705664\n")
        .command("/usr/sbin/sysctl -n hw.machine", "arm64\n")
        .command("/usr/sbin/sysctl -n iogpu.wired_limit_mb", "28000\n")
        .command("/usr/sbin/sysctl -n hw.pagesize", "16384\n")
        .command("/usr/sbin/ioreg -r -d 1 -w 0 -c IOAccelerator", IOREG_GPU)
        .command(
            "/usr/sbin/sysctl -n kern.memorystatus_vm_pressure_level",
            "1\n",
        )
        .command(
            "/usr/bin/memory_pressure -Q",
            "System-wide memory free percentage: 63%\n",
        )
        .command(
            "/usr/sbin/sysctl -n vm.swapusage",
            "total = 2048.00M  used = 512.00M  free = 1536.00M  (encrypted)\n",
        )
        .command(
            "/usr/bin/pmset -g therm",
            "Note: No thermal warning level has been recorded\n",
        )
        .command("/bin/date +%H:%M:%S %z", "12:00:01 -0500\n");
    for vm in vm_stats {
        host = host.command("/usr/bin/vm_stat", vm);
    }
    for listing in ps {
        host = host.command(
            "/bin/ps -axo pid=,rss=,%cpu=,%mem=,state=,pagein=,comm=,args=",
            listing,
        );
    }
    host
}

fn scripted_vm_stat(pageins: u64) -> String {
    format!(
        "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
Pages free:                               10000.\n\
Pages speculative:                         2000.\n\
Pages wired down:                         50000.\n\
Pages occupied by compressor:             10000.\n\
Pages stored in compressor:               40000.\n\
Anonymous pages:                         300000.\n\
File-backed pages:                       100000.\n\
Pages reactivated:                       700000.\n\
Compressions:                          90000000.\n\
Decompressions:                        40000000.\n\
Pageins:                               {pageins}.\n\
Pageouts:                                300000.\n\
Swapins:                                5000000.\n\
Swapouts:                               6000000.\n"
    )
}

#[test]
fn a_failed_counter_read_never_becomes_a_rate_baseline() {
    let host = scripted_macos_host(
        &[
            scripted_vm_stat(50_000_000),
            String::new(),
            scripted_vm_stat(50_000_010),
            scripted_vm_stat(50_000_020),
        ],
        &[MACOS_PS],
    );
    let mut collector = collector(host, Platform::MacOs, None);
    let samples: Vec<Sample> = (0..4)
        .map(|_| {
            thread::sleep(Duration::from_millis(20));
            collector.sample()
        })
        .collect();
    assert!(!samples[1].rate_ready, "an unread counter has no rate");
    assert!(
        !samples[2].rate_ready,
        "the next good read starts a new baseline"
    );
    assert_eq!(samples[2].swap_in, 0);
    assert!(samples[3].rate_ready);
    assert_eq!(samples[3].swap_in, 0, "swap-ins did not change");
    for sample in &samples {
        assert_ne!(sample.impact, "SWAP THRASHING");
    }
    assert!(!journal(&collector)
        .iter()
        .any(|(state, _)| state == "PAGING" || state == "SWAP THRASHING"));
}

#[test]
fn a_failed_linux_counter_read_never_becomes_a_rate_baseline() {
    let mut collector = collector(linux_host(), Platform::Linux, None);
    collector.sample();
    collector.host = Box::new(FakeHost::default());
    let sample = collector.sample();
    assert!(!sample.rate_ready);
    assert!(collector.previous.is_none());
}

#[test]
fn a_failed_process_listing_keeps_the_runtime_and_its_history() {
    let host = scripted_macos_host(&[scripted_vm_stat(1)], &[MACOS_PS, "", MACOS_PS, MACOS_PS]);
    let mut collector = collector(host, Platform::MacOs, None);
    let samples: Vec<Sample> = (0..4).map(|_| collector.sample()).collect();
    let first = &samples[0];
    assert!(first.llm_count > 0, "the fixture lists a runtime");
    for sample in &samples {
        assert_eq!(sample.llm_count, first.llm_count);
        assert_eq!(sample.llm_provider, first.llm_provider);
    }
    assert_eq!(collector.generation_history.len(), 4);
    let detected = journal(&collector)
        .iter()
        .filter(|(_, summary)| summary.starts_with("detected"))
        .count();
    assert!(detected <= 1, "{:?}", journal(&collector));
}

#[test]
fn repeated_failed_process_listings_stop_reusing_the_last_one() {
    let host = scripted_macos_host(&[scripted_vm_stat(1)], &[MACOS_PS, "", "", "", "", ""]);
    let mut collector = collector(host, Platform::MacOs, None);
    let samples: Vec<Sample> = (0..5).map(|_| collector.sample()).collect();
    assert!(samples[3].llm_count > 0);
    assert_eq!(samples[4].llm_count, 0);
}

#[test]
fn an_idle_sample_naming_another_model_does_not_restart_the_rate_charts() {
    let live = |model: &str, live: bool| Sample {
        llm_provider: "oMLX".into(),
        llm_model: model.into(),
        llm_source: TelemetrySource::Live,
        llm_generation_tps: Some(40.0),
        llm_generation_tps_live: live,
        ..Sample::default()
    };
    assert_eq!(
        crate::collector::rate_identity(&live("coder", true)),
        Some(("oMLX".into(), "coder".into()))
    );
    // Idle: oMLX keeps its last rate and names the first listed model.
    assert_eq!(crate::collector::rate_identity(&live("chat", false)), None);
    // A rate read from a log has no live flag but still names its model.
    let logged = Sample {
        llm_source: TelemetrySource::Log,
        ..live("second", false)
    };
    assert_eq!(
        crate::collector::rate_identity(&logged),
        Some(("oMLX".into(), "second".into()))
    );
}

#[test]
fn host_state_flaps_write_one_journal_row_each_way() {
    let mut collector = collector(linux_host(), Platform::Linux, None);
    let mut previous: Option<Sample> = None;
    for i in 0..16 {
        let sample = Sample {
            impact: if i % 2 == 0 {
                "HEAVY PAGING"
            } else {
                "MEMORY STRESS"
            }
            .into(),
            rate_ready: true,
            vm_available: true,
            ..Sample::default()
        };
        collector.record_journal_events(previous.as_ref(), &sample);
        previous = Some(sample);
    }
    let rows: Vec<String> = journal(&collector)
        .into_iter()
        .map(|(state, _)| state)
        .filter(|state| state == "HEAVY PAGING" || state == "MEMORY STRESS")
        .collect();
    assert!(rows.len() <= 4, "{rows:?}");
}

#[test]
fn a_stock_omlx_login_uses_the_key_in_its_own_settings() {
    let home = TempDir::new("omlx-stock-key");
    home.write(
        ".omlx/settings.json",
        r#"{"server":{"port":8123},"auth":{"api_key":"from-settings"}}"#,
    );
    let key = |provider: Option<&str>, omlx: Option<&str>, base: Option<&Path>| {
        omlx_api_key_from(
            provider.map(str::to_owned),
            None,
            omlx.map(str::to_owned),
            Some(&home.0),
            base,
        )
    };
    // The key set in mlxtop's Settings outranks what oMLX keeps for itself,
    // and gives way only to mlxtop's own variable.
    let configured = |provider: Option<&str>, set: &str| {
        omlx_api_key_from(
            provider.map(str::to_owned),
            Some(set.to_owned()),
            Some("from-omlx-env".into()),
            Some(&home.0),
            None,
        )
    };
    assert_eq!(
        configured(None, "from-config").as_deref(),
        Some("from-config")
    );
    assert_eq!(
        configured(Some("from-mlxtop"), "from-config").as_deref(),
        Some("from-mlxtop")
    );
    assert_eq!(configured(None, "  ").as_deref(), Some("from-omlx-env"));
    assert_eq!(key(None, None, None).as_deref(), Some("from-settings"));
    assert_eq!(
        key(None, Some("from-omlx-env"), None).as_deref(),
        Some("from-omlx-env")
    );
    assert_eq!(
        key(Some("from-mlxtop"), Some("x"), None).as_deref(),
        Some("from-mlxtop")
    );
    assert_eq!(
        key(Some("  "), None, None).as_deref(),
        Some("from-settings")
    );
    home.write(".config/omlx-coding/server.env", "API_KEY=from-env\n");
    assert_eq!(key(None, None, None).as_deref(), Some("from-env"));

    let other = TempDir::new("omlx-base-path");
    other.write(".omlx/settings.json", r#"{"auth":{"api_key":null}}"#);
    assert_eq!(read_omlx_settings_api_key(&other.0.join(".omlx")), None);
    other.write("base/settings.json", r#"{"auth":{"api_key":"from-base"}}"#);
    let bare = TempDir::new("omlx-no-wrapper");
    assert_eq!(
        omlx_api_key_from(None, None, None, Some(&bare.0), Some(&other.0.join("base"))).as_deref(),
        Some("from-base")
    );
    assert_eq!(
        omlx_api_key_from(None, None, None, Some(&bare.0), None),
        None
    );
}

#[test]
fn omlx_key_is_not_sent_without_a_detected_or_selected_runtime() {
    let home = TempDir::new("omlx-gate");
    home.write(".config/omlx-coding/server.env", "API_KEY=secret\n");
    let mut client = omlx_client(1, Some(home.0.clone()));
    client.login_allowed = false;
    client.login();
    assert!(client.session_cookie.is_none());
    assert!(client
        .runtime
        .probes
        .iter()
        .any(|probe| probe.issue == Some(crate::runtime_diagnostics::ProbeIssue::NotDetected)));
}

#[test]
fn a_refused_omlx_login_is_retried_once_a_minute_not_every_poll() {
    let home = TempDir::new("omlx-refused");
    home.write(".config/omlx-coding/server.env", "API_KEY=wrong\n");
    let (port, server) = serve(vec![reply("POST /admin/api/login ", 401, "{}")]);
    let mut client = omlx_client(port, Some(home.0.clone()));
    client.login();
    assert_eq!(server.join().unwrap().len(), 1);
    // The next attempt within a minute sends nothing.
    client.login();
    assert!(client.session_cookie.is_none());
}

#[test]
fn omlx_server_env_reads_exported_host_and_port() {
    let found = parse_omlx_server_env("export PORT=9000\nexport HOST='10.0.0.2'\n");
    assert_eq!(found.port, Some(9000));
    assert_eq!(found.host.as_deref(), Some("10.0.0.2"));
}

fn one_connection(
    reply: impl FnOnce(std::net::TcpStream) + Send + 'static,
) -> (u16, thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        reply(stream);
    });
    (port, handle)
}

#[test]
fn slow_responses_time_out_as_a_whole() {
    use std::io::Write as _;
    let (port, server) = one_connection(|mut stream| {
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\n");
        for _ in 0..40 {
            if stream.write_all(b"x").is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
    let started = Instant::now();
    let reply = crate::transport::http_request("127.0.0.1", port, "GET", "/health", &[], None);
    assert!(matches!(
        reply,
        Err(crate::runtime_diagnostics::ProbeIssue::Timeout)
    ));
    assert!(
        started.elapsed() < Duration::from_millis(2600),
        "{:?}",
        started.elapsed()
    );
    server.join().unwrap();
}

#[test]
fn a_complete_reply_on_a_kept_open_connection_is_read() {
    use std::io::{Read as _, Write as _};
    let (port, server) = one_connection(|mut stream| {
        let mut request = [0_u8; 512];
        let _ = stream.read(&mut request);
        let body = r#"{"default_model":"q"}"#;
        let _ = stream.write_all(
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes(),
        );
        thread::sleep(Duration::from_millis(50));
        let _ = stream.write_all(body.as_bytes());
        // Keep the connection open well past the reply.
        thread::sleep(Duration::from_millis(1500));
    });
    let started = Instant::now();
    let reply = crate::transport::http_request("localhost", port, "GET", "/health", &[], None)
        .expect("localhost reaches an IPv4-only server, and the reply ends at its length");
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, r#"{"default_model":"q"}"#);
    assert!(started.elapsed() < Duration::from_millis(1000));
    server.join().unwrap();
}

#[test]
fn sampler_keeps_its_cadence_and_quit_does_not_wait_for_a_sample() {
    let host = linux_host();
    let sampler = Sampler::start(Duration::from_millis(200), move || {
        collector(host, Platform::Linux, None)
    });
    let first = sampler.views.recv_timeout(Duration::from_secs(10)).unwrap();
    let mut last = first;
    for _ in 0..5 {
        last = sampler.views.recv_timeout(Duration::from_secs(10)).unwrap();
    }
    let _ = last;
    let started = Instant::now();
    drop(sampler);
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
fn an_unread_view_channel_holds_one_snapshot() {
    let host = linux_host();
    let sampler = Sampler::start(Duration::from_millis(5), move || {
        collector(host, Platform::Linux, None)
    });
    thread::sleep(Duration::from_millis(300));
    // Pause before counting: a sampler still writing every 5 ms can add a
    // snapshot while the queue is being read, which a slow runner counts.
    sampler.send(SamplerCommand::SetPaused(true));
    thread::sleep(Duration::from_millis(200));
    let waiting = sampler.views.try_iter().count();
    assert!(waiting <= 1, "{waiting} snapshots queued");
}

#[test]
fn a_saved_history_length_reaches_the_running_sampler() {
    let sampler = Sampler::start(Duration::from_millis(20), || {
        collector(linux_host(), Platform::Linux, None)
    });
    let mut view = recv_view(&sampler);
    while view.load_history.len() < 3 {
        view = recv_view(&sampler);
    }
    sampler.send(SamplerCommand::SetHistory(1));
    // The next views are trimmed to the new length.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let view = recv_view(&sampler);
        if view.load_history.len() == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "history was never trimmed");
    }
}
