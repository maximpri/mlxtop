use crate::test_support::*;
// SPDX-License-Identifier: MIT
// Process-level behavior: diagnostics, configuration, CLI, static report,
// the interactive loop, keyboard/mouse navigation and classification.
use super::*;
use crate::tests::{
    populate_dashboard_fixture, render_app, render_view, test_app, test_app_with_sender,
};
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn press(app: &mut App, codes: &[KeyCode]) {
    for code in codes {
        app.handle_key(key(*code));
    }
}

// --- diagnostics, configuration and CLI -----------------------------------

#[test]
fn diagnostics_log_escapes_newlines_and_rotates_at_the_size_limit() {
    let dir = TempDir::new("diagnostics");
    let path = dir.0.join("nested/mlxtop.log");
    let diagnostics = Diagnostics::open(path.clone(), 120).expect("log opens");
    diagnostics.log("INFO", "first", "line one\nline two\rend");
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("level=INFO event=first line one\\nline two\\nend"));
    assert!(text.starts_with("ts_ms="));
    assert_eq!(text.lines().count(), 1);

    diagnostics.log("INFO", "second", &"x".repeat(200));
    diagnostics.log("WARN", "after_rotation", "fresh");
    let rotated = fs::read_to_string(path.with_extension("log.1")).unwrap();
    assert!(rotated.contains("event=second"));
    let current = fs::read_to_string(&path).unwrap();
    assert!(current.contains("event=after_rotation") && !current.contains("event=second"));

    // When the rotated name cannot be used the log is truncated in place.
    let blocked = dir.0.join("blocked.log");
    fs::create_dir_all(blocked.with_extension("log.1").join("occupied")).unwrap();
    let diagnostics = Diagnostics::open(blocked.clone(), 10).unwrap();
    diagnostics.log("INFO", "one", &"y".repeat(50));
    diagnostics.log("INFO", "two", "small");
    let text = fs::read_to_string(&blocked).unwrap();
    assert!(text.contains("event=two") && !text.contains("event=one"));

    assert!(Diagnostics::open(dir.0.join("blocked.log.1"), 10).is_none());
}

#[test]
fn diagnostics_path_prefers_override_then_platform_locations() {
    let home = Some(PathBuf::from("/home/u"));
    assert_eq!(
        diagnostics_path_from(
            Some(" /tmp/x.log ".into()),
            None,
            home.clone(),
            Platform::Linux
        ),
        Some(PathBuf::from("/tmp/x.log"))
    );
    assert_eq!(
        diagnostics_path_from(
            Some("  ".into()),
            Some("/state".into()),
            home.clone(),
            Platform::Linux
        ),
        Some(PathBuf::from("/state/mlxtop/mlxtop.log"))
    );
    assert_eq!(
        diagnostics_path_from(None, Some(" ".into()), home.clone(), Platform::Linux),
        Some(PathBuf::from("/home/u/.local/state/mlxtop/mlxtop.log"))
    );
    assert_eq!(
        diagnostics_path_from(None, None, None, Platform::Linux),
        Some(PathBuf::from("mlxtop.log"))
    );
    assert_eq!(
        diagnostics_path_from(None, Some("/state".into()), home.clone(), Platform::MacOs),
        Some(PathBuf::from("/home/u/Library/Logs/mlxtop/mlxtop.log"))
    );
    // Windows: %LOCALAPPDATA%, then the profile's AppData\Local.
    assert_eq!(
        diagnostics_path_from(
            None,
            Some("/appdata".into()),
            home.clone(),
            Platform::Windows
        ),
        Some(PathBuf::from("/appdata").join("mlxtop").join("mlxtop.log"))
    );
    assert_eq!(
        diagnostics_path_from(None, Some(" ".into()), home, Platform::Windows),
        Some(PathBuf::from("/home/u/AppData/Local/mlxtop/mlxtop.log"))
    );
    assert_eq!(
        diagnostics_path_from(None, None, None, Platform::MacOs),
        Some(PathBuf::from("mlxtop.log"))
    );
    assert!(diagnostics_path().is_some());
    assert_eq!(
        diagnostics_default_hint(Platform::Linux),
        "~/.local/state/mlxtop/mlxtop.log"
    );
    assert_eq!(
        diagnostics_default_hint(Platform::MacOs),
        "~/Library/Logs/mlxtop/mlxtop.log"
    );
}

#[test]
fn process_diagnostics_record_session_events_and_panics() {
    // The process-wide log is shared by every test; this test owns it.
    let dir = TempDir::new("process-log");
    let path = dir.0.join("mlxtop.log");
    let diagnostics = install_diagnostics(path.clone()).expect("installed");
    assert_eq!(diagnostics.path, path);
    assert!(install_diagnostics(dir.0.join("ignored.log")).is_some());
    assert!(
        !dir.0.join("ignored.log").exists(),
        "the first log stays active"
    );
    diagnostics_log("INFO", "custom_event", "detail=1");

    install_panic_hook();
    let result = panic::catch_unwind(|| panic!("hooked failure"));
    assert!(result.is_err());
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("event=session_start version="));
    assert!(text.contains("event=custom_event detail=1"));
    assert!(text.contains("event=panic message=hooked_failure location=src/tests/app.rs:"));
    assert!(init_diagnostics().is_some(), "an existing log is reused");
}

#[test]
fn panic_payloads_keep_messages_and_label_other_values() {
    assert_eq!(panic_payload(&"static"), "static");
    assert_eq!(panic_payload(&String::from("owned")), "owned");
    assert_eq!(panic_payload(&42_u8), "non-string panic payload");
}

#[test]
fn config_files_load_fall_back_and_report_parse_errors() {
    let dir = TempDir::new("config");
    assert_eq!(
        load_config_from(&dir.0.join("missing.json")),
        Config::default()
    );
    let valid = dir.write("valid.json", r#"{"interval": 5, "omx": {"port": 9000}}"#);
    let config = load_config_from(&valid);
    assert_eq!(config.interval, Some(5));
    assert_eq!(config.omx.unwrap().port, Some(9000));
    let invalid = dir.write("invalid.json", r#"{"interval": "fast"}"#);
    assert_eq!(load_config_from(&invalid), Config::default());
    assert_eq!(
        config_path_in(Some(PathBuf::from("/home/u"))),
        PathBuf::from("/home/u/.config/mlxtop/config.json")
    );
    assert_eq!(config_path_in(None), PathBuf::from("config.json"));
    assert!(config_path().ends_with("config.json"));
    let _ = load_config();
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn command_line_options_override_config_and_validate_ranges() {
    let config = Config {
        interval: Some(5),
        history: Some(600),
        ..Config::default()
    };
    let run = |interval, history, once| CliAction::Run {
        interval,
        history,
        once,
    };
    assert_eq!(parse_args(&[], &config).unwrap(), run(5, 600, false));
    assert_eq!(
        parse_args(&args(&["2", "-n", "40", "--once"]), &config).unwrap(),
        run(2, 40, true)
    );
    assert_eq!(
        parse_args(
            &args(&["--interval", "9", "--history", "50", "-1"]),
            &config
        )
        .unwrap(),
        run(9, 50, true)
    );
    assert_eq!(
        parse_args(&args(&["-i", "3"]), &Config::default()).unwrap(),
        run(3, HISTORY_DEFAULT, false)
    );
    // Out-of-range file values fall back to defaults instead of failing.
    let rejected = Config {
        interval: Some(0),
        history: Some(5),
        ..Config::default()
    };
    assert_eq!(
        parse_args(&[], &rejected).unwrap(),
        run(INTERVAL_DEFAULT, HISTORY_DEFAULT, false)
    );
    assert_eq!(
        parse_args(&args(&["-V"]), &config).unwrap(),
        CliAction::Print(format!("mlxtop {VERSION}"))
    );
    let CliAction::Print(help) = parse_args(&args(&["--help"]), &config).unwrap() else {
        panic!("help prints");
    };
    assert!(help.starts_with("Usage: mlxtop [refresh-seconds] [options]"));
    assert!(help.contains(diagnostics_default_hint(Platform::current())));
    assert!(help.contains("{/} interval"));
    for (input, error) in [
        (&["-i"][..], "missing interval"),
        (&["--history"][..], "missing history"),
        (
            &["-i", "61"][..],
            "interval must be between 1 and 60 seconds",
        ),
        (&["-n", "19"][..], "history must be between 20 and 3600"),
        (&["--bogus"][..], "unknown option: --bogus"),
        (&["1", "2"][..], "unknown option: 2"),
    ] {
        assert_eq!(
            parse_args(&args(input), &Config::default())
                .unwrap_err()
                .to_string(),
            error
        );
    }
    assert!(parse_args(&args(&["fast"]), &Config::default()).is_err());
    assert!(help_text(Platform::MacOs).contains("~/Library/Logs/mlxtop/mlxtop.log"));
}

// --- static report ----------------------------------------------------------

fn report(sample: &Sample, platform: Platform) -> String {
    let mut out = Vec::new();
    write_static(&mut out, sample, 2, Thresholds::default(), platform).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn static_report_lists_memory_paging_runtime_and_diagnosis() {
    let mut sample = Sample {
        impact: "LLM READY".into(),
        grade: "HEALTHY".into(),
        pressure: "GREEN".into(),
        total_memory: 32 * 1024 * MIB,
        resident_memory: Some(16 * 1024 * MIB),
        swap_available: true,
        swap_total: 4 * 1024 * MIB,
        swap_used: 1024 * MIB,
        swap_in: 2048,
        thermal: "no warning".into(),
        llm_count: 1,
        llm_provider: "oMLX".into(),
        llm_status: "ready".into(),
        llm_source: TelemetrySource::Live,
        llm_model: "qwen".into(),
        llm_active_requests: Some(1),
        llm_prompt_tokens: Some(1200),
        llm_output_tokens: Some(30),
        llm_requests: vec![domain::RequestUsage {
            provider: "oMLX".into(),
            model: "qwen".into(),
            id: "req-1".into(),
            prompt: 1200,
            cached: Some(600),
            output: Some(30),
            output_tps: Some(20.0),
            completed: true,
            ttft_ms: Some(150),
            observed_at: None,
        }],
        process_memory: Some(process_memory::Reading {
            pid: 42,
            started: 1,
            resident: 2 * 1024 * MIB,
            footprint: 3 * 1024 * MIB,
            peak: 4 * 1024 * MIB,
            at: Instant::now(),
        }),
        process_memory_growth: Some(1024),
        metal: MetalTelemetry {
            device_name: Some("Apple M3".into()),
            gpu_cores: Some(10),
            renderer_util: Some(40),
            ..MetalTelemetry::default()
        },
        gpu_util: Some(45),
        correlation: CorrelationInsight {
            summary: "GEN 20.0 tok/s".into(),
            details: "no factor".into(),
            confidence: 60,
            ..CorrelationInsight::default()
        },
        ..Sample::default()
    };
    sample.mlx.version = Some("0.29".into());
    sample.llm_details = Some("remote API · context capacity 8192 tokens".into());
    let text = report(&sample, Platform::MacOs);
    assert!(text.starts_with("mlxtop · static report (2s sample)\n\n"));
    assert!(text.contains("SIGNAL       LLM READY · HEALTHY"));
    assert!(text.contains("MEMORY       16.0 GiB resident / 32.0 GiB total"));
    assert!(text.contains("PAGING       1.0 GiB / 4.0 GiB · 25% used · in 2.0 KiB/s · out 0 B/s"));
    assert!(text.contains("METAL        Apple M3 · 10 cores · GPU 45% · renderer 40%"));
    assert!(text.contains("LLM          oMLX · ready"));
    assert!(text.contains("PROVIDER     remote API · context capacity 8192 tokens"));
    assert!(text.contains("active 1"));
    assert!(text.contains("TOKENS       PROMPT 1.2k · OUT 30"));
    assert!(text.contains("REQUEST      "));
    assert!(text.contains("PROCESS OS   pid 42 · footprint 3.0 GiB · lifetime peak 4.0 GiB"));
    assert!(text.contains("growth +1.0 KiB/s"));
    assert!(text.contains("MLX          version 0.29"));
    assert!(text.contains("CORRELATION   GEN 20.0 tok/s"));
    assert!(text.contains("EVIDENCE      no factor · medium confidence"));
    assert!(text.contains("DIAGNOSIS    "));

    let linux = report(&sample, Platform::Linux);
    assert!(linux.contains("GPU          NVIDIA counters unavailable"));
    assert!(!linux.contains("METAL"));
}

#[test]
fn static_report_marks_missing_swap_and_unknown_hardware() {
    let empty = report(&Sample::default(), Platform::MacOs);
    assert!(empty.contains("PAGING       —"));
    assert!(empty.contains("METAL        unavailable · — cores · GPU — "));
    assert!(empty.contains("MEMORY       — resident"));
    assert!(empty.contains("MLX          version — · active —"));
    assert!(!empty.contains("CORRELATION"));
    assert!(!empty.contains("PROCESS OS"));
    let unallocated = Sample {
        swap_available: true,
        ..Sample::default()
    };
    assert!(report(&unallocated, Platform::Linux).contains("PAGING       SWAP 0 B · not allocated"));
    if cfg!(any(target_os = "linux", target_os = "windows")) {
        let nvidia = Sample {
            gpus: gpu::parse("0, GPU-1, RTX, 50, 1000, 2000, 60\n"),
            ..Sample::default()
        };
        assert!(report(&nvidia, Platform::Linux).contains("RTX"));
    }
}

#[test]
fn static_report_propagates_write_failures() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("closed"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let error = write_static(
        &mut Broken,
        &Sample::default(),
        1,
        Thresholds::default(),
        Platform::Linux,
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "closed");
}

#[test]
fn once_mode_samples_twice_and_reports_measured_rates() {
    let mut collector = Collector::with_host(
        60,
        offline_config(),
        Box::new(linux_host()),
        Platform::Linux,
        None,
    );
    let mut out = Vec::new();
    run_once(&mut collector, Duration::ZERO, &mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.starts_with("mlxtop · static report (0s sample)"));
    assert!(text.contains("PRESSURE     "));
    assert!(text.contains("PAGING       976.6 MiB / 3.8 GiB · 25% used · in "));
    assert!(
        !text.contains("in 0 B/s"),
        "the second sample measures paging"
    );
    assert!(text.contains("RUNTIME      thermal 72°C measured · LLM 1"));
}

#[test]
fn terminal_bell_writes_bel_and_flushes() {
    let mut out = Vec::new();
    write_terminal_bell(&mut out);
    assert_eq!(out, b"\x07");
    let mut guard = TerminalGuard::new();
    assert!(guard.active);
    guard.disarm();
    assert!(!guard.active);
}

// --- interactive loop -------------------------------------------------------

#[test]
fn interactive_loop_draws_dispatches_input_and_stops_on_quit() {
    let (mut app, views) = test_app_with_sender(0);
    let mut view = empty_view();
    view.current.updated = "09:30:00".into();
    views.send(view).unwrap();
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let mut script = VecDeque::from([
        None,
        Some(Event::Resize(100, 30)),
        Some(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        })),
        Some(Event::Key(key(KeyCode::Char('2')))),
        Some(Event::Key(key(KeyCode::Char('q')))),
    ]);
    let mut timeouts = Vec::new();
    run_app(&mut terminal, &mut app, &mut |timeout| {
        timeouts.push(timeout);
        Ok(script
            .pop_front()
            .expect("loop stops before the script ends"))
    })
    .unwrap();
    assert!(app.quit);
    assert_eq!(app.tab, 1);
    assert_eq!(app.collector.current.updated, "09:30:00");
    assert_eq!(timeouts.len(), 5);
    assert!(timeouts
        .iter()
        .all(|timeout| *timeout == Duration::from_millis(100)));
    let screen = format!("{:?}", terminal.backend().buffer());
    assert!(screen.contains("MLX Top"));
}

#[test]
fn interactive_loop_returns_input_errors() {
    let mut app = test_app(0);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let error = run_app(&mut terminal, &mut app, &mut |_| {
        Err(io::Error::other("tty closed"))
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "tty closed");
    assert!(!app.quit);
}

#[test]
fn terminal_events_time_out_without_input() {
    // Without a terminal the poll either times out or reports an error; it
    // must never block past the timeout.
    let started = Instant::now();
    let _ = next_terminal_event(Duration::from_millis(1));
    assert!(started.elapsed() < Duration::from_secs(5));
}

// --- keyboard and mouse -------------------------------------------------------

#[test]
fn help_overlay_swallows_keys_until_dismissed() {
    let mut app = test_app(0);
    press(&mut app, &[KeyCode::Char('?')]);
    assert!(app.help);
    press(&mut app, &[KeyCode::Char('2'), KeyCode::Char('q')]);
    assert!(app.help && !app.quit && app.tab == 0);
    press(&mut app, &[KeyCode::Esc]);
    assert!(!app.help);
    press(&mut app, &[KeyCode::Char('h')]);
    press(&mut app, &[KeyCode::Char('h')]);
    assert!(!app.help);
    app.handle_key(KeyEvent::new_with_kind(
        KeyCode::Char('q'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    ));
    assert!(!app.quit, "key releases are ignored");
    app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.quit);
}

fn process(pid: u32, name: &str, rss: u64, cpu: f64) -> LlmProcess {
    LlmProcess {
        pid,
        name: name.into(),
        command: format!("/usr/bin/{name} --serve"),
        rss,
        cpu,
        memory_percent: None,
        state: "S".into(),
        pageins: None,
        pagein_rate: None,
    }
}

#[test]
fn process_view_keys_select_sort_and_filter() {
    let mut app = test_app(1);
    app.collector.current.llm_processes = vec![
        process(30, "beta", 300, 1.0),
        process(10, "Alpha", 100, 9.0),
        process(20, "gamma", 200, 5.0),
    ];
    let names = |app: &App| {
        app.filtered_llm_processes()
            .into_iter()
            .map(|process| process.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&app), ["beta", "gamma", "Alpha"], "RSS first");
    press(&mut app, &[KeyCode::End]);
    assert_eq!(app.top_selected, 2);
    press(&mut app, &[KeyCode::Up, KeyCode::PageUp]);
    assert_eq!(app.top_selected, 0);
    press(&mut app, &[KeyCode::Down, KeyCode::PageDown]);
    assert_eq!(app.top_selected, 11);
    press(&mut app, &[KeyCode::Home]);
    assert_eq!(app.top_selected, 0);
    press(&mut app, &[KeyCode::Char('s')]);
    assert_eq!(app.top_sort, TopSort::Cpu);
    assert_eq!(names(&app), ["Alpha", "gamma", "beta"]);
    press(&mut app, &[KeyCode::Char('s')]);
    assert_eq!(names(&app), ["Alpha", "gamma", "beta"], "PID order");
    assert_eq!(app.top_sort.label(), "PID");
    press(&mut app, &[KeyCode::Char('s')]);
    assert_eq!(app.top_sort.label(), "NAME");
    assert_eq!(names(&app), ["Alpha", "beta", "gamma"]);
    let sorted = render_app(&app, 120, 32);
    assert!(sorted.contains("NAME"));

    press(&mut app, &[KeyCode::Char('/')]);
    assert!(app.top_filtering);
    let typing = render_app(&app, 120, 32);
    assert!(typing.contains("typing · Enter/Esc finish"));
    press(
        &mut app,
        &[
            KeyCode::Char('g'),
            KeyCode::Char('x'),
            KeyCode::Backspace,
            KeyCode::F(1),
        ],
    );
    app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::ALT));
    assert_eq!(app.top_filter, "g");
    assert_eq!(names(&app), ["gamma"]);
    press(&mut app, &[KeyCode::Enter]);
    assert!(!app.top_filtering);
    press(
        &mut app,
        &[KeyCode::Char('f'), KeyCode::Esc, KeyCode::Char('c')],
    );
    assert!(app.top_filter.is_empty());
    assert_eq!(names(&app).len(), 3);
    // Tab leaves an in-progress filter and switches views.
    press(&mut app, &[KeyCode::Char('f'), KeyCode::Tab]);
    assert!(!app.top_filtering);
    assert_eq!(app.tab, 2);
    press(&mut app, &[KeyCode::BackTab]);
    assert_eq!(app.tab, 1);
}

#[test]
fn journal_keys_scroll_newest_first_and_cycle_filters() {
    let mut app = test_app(2);
    for index in 0..30 {
        let kind = if index % 2 == 0 {
            EventKind::Paging
        } else {
            EventKind::Gpu
        };
        app.collector.signals.push_back(SignalEvent {
            time: format!("10:00:{index:02}"),
            recorded_at: SystemTime::now(),
            kind,
            state: if kind == EventKind::Paging {
                "PAGING"
            } else {
                "GPU"
            }
            .into(),
            summary: format!("event {index}"),
            tone: Tone::Yellow,
        });
    }
    press(&mut app, &[KeyCode::Down, KeyCode::PageDown]);
    assert_eq!(app.journal_scroll, 11);
    press(&mut app, &[KeyCode::Up, KeyCode::PageUp]);
    assert_eq!(app.journal_scroll, 0);
    press(&mut app, &[KeyCode::End]);
    assert_eq!(app.journal_scroll, 29);
    let oldest = render_app(&app, 120, 30);
    assert!(oldest.contains("│  event 0"));
    assert!(
        !oldest.contains("│  event 29"),
        "only the header names the latest"
    );
    press(&mut app, &[KeyCode::Home]);
    let newest = render_app(&app, 120, 30);
    assert!(newest.contains("JOURNAL"));
    assert!(newest.contains("30 events · f/[/] filter"));
    assert!(newest.contains("LATEST  event 29"));
    assert!(newest.contains("EVENTS · ALL · 1–"));
    assert!(newest.contains("of 30"));

    let mut seen = Vec::new();
    for _ in 0..7 {
        press(&mut app, &[KeyCode::Char('f')]);
        seen.push(app.journal_filter.label());
    }
    assert_eq!(
        seen,
        ["LLM", "PRESSURE", "PAGING", "GPU", "THERMAL", "SYSTEM", "ALL"]
    );
    let mut back = Vec::new();
    for _ in 0..7 {
        press(&mut app, &[KeyCode::Char('[')]);
        back.push(app.journal_filter.label());
    }
    assert_eq!(
        back,
        ["SYSTEM", "THERMAL", "GPU", "PAGING", "PRESSURE", "LLM", "ALL"]
    );
    press(
        &mut app,
        &[KeyCode::Char(']'), KeyCode::Char(']'), KeyCode::Char(']')],
    );
    assert_eq!(app.journal_filter, JournalFilter::Paging);
    let paging = render_app(&app, 120, 30);
    assert!(paging.contains("15 PAGING of 30 events"));
    assert!(paging.contains("│  event 28") && !paging.contains("event 29"));
    press(&mut app, &[KeyCode::Char(']')]);
    assert_eq!(app.filtered_journal_events().len(), 15, "GPU events");
    press(&mut app, &[KeyCode::Char(']')]);
    let thermal = render_app(&app, 120, 30);
    assert!(thermal.contains("waiting for the first recorded event"));
    assert!(thermal.contains("EVENTS · THERMAL · 0–0 of 0"));
    assert!(thermal.contains("age —"));
    // Unhandled journal keys fall through to global navigation.
    press(&mut app, &[KeyCode::Char('1')]);
    assert_eq!(app.tab, 0);
}

#[test]
fn global_keys_pause_resize_interval_switch_tabs_and_reset() {
    let (commands, command_receiver) = mpsc::channel();
    let (_views_sender, views) = mpsc::channel();
    let mut app = test_app(1);
    app.sampler = Sampler {
        commands,
        views,
        handle: None,
    };
    press(&mut app, &[KeyCode::Char('p')]);
    assert!(app.paused);
    assert!(render_app(&app, 120, 32).contains("PAUSED · 1s"));
    press(&mut app, &[KeyCode::Char(' ')]);
    assert!(!app.paused);
    press(
        &mut app,
        &[KeyCode::Char('}'), KeyCode::Char('}'), KeyCode::Char('{')],
    );
    assert_eq!(app.interval, Duration::from_secs(2));
    for _ in 0..70 {
        press(&mut app, &[KeyCode::Char('}')]);
    }
    assert_eq!(app.interval, Duration::from_secs(60), "interval is capped");
    for _ in 0..70 {
        press(&mut app, &[KeyCode::Char('{')]);
    }
    assert_eq!(app.interval, Duration::from_secs(1), "and floored");
    let sent: Vec<_> = command_receiver.try_iter().collect();
    assert!(matches!(sent[0], SamplerCommand::SetPaused(true)));
    assert!(matches!(sent[1], SamplerCommand::SetPaused(false)));
    assert!(matches!(sent[2], SamplerCommand::SetInterval(d) if d == Duration::from_secs(2)));

    press(&mut app, &[KeyCode::Char('j')]);
    assert_eq!(app.tab, 2);
    press(&mut app, &[KeyCode::Char('o')]);
    assert_eq!(app.tab, 0);
    press(&mut app, &[KeyCode::Char('3')]);
    assert_eq!(app.tab, 2);
    press(&mut app, &[KeyCode::Char('t')]);
    assert_eq!(app.tab, 1);
    press(&mut app, &[KeyCode::Right]);
    assert_eq!(app.tab, 2);
    press(&mut app, &[KeyCode::Left]);
    assert_eq!(app.tab, 1);
    press(&mut app, &[KeyCode::Char('x')]);
    assert_eq!(app.tab, 1, "unbound keys do nothing");

    app.journal_filter = JournalFilter::Gpu;
    app.alert = Some(ActiveAlert {
        state: "HEAVY PAGING".into(),
        summary: "swap".into(),
        time: "now".into(),
    });
    press(&mut app, &[KeyCode::Char('r')]);
    assert!(app.alert.is_none());
    assert_eq!(app.journal_filter, JournalFilter::All);
    assert!(matches!(
        command_receiver.try_iter().last(),
        Some(SamplerCommand::Reset)
    ));
    press(&mut app, &[KeyCode::Char('a')]);
    press(&mut app, &[KeyCode::Esc]);
    assert!(app.quit);
}

#[test]
fn overview_keys_scroll_requests_zoom_and_cycle_expanded_charts() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    render_app(&app, 160, 48);
    let last = app.collector.request_history.len().saturating_sub(1);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    assert_eq!(app.charts.focused, Chart::Prompt);
    assert_eq!(app.request_scroll, 1.min(last));
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
    assert_eq!(app.request_scroll, 0);
    press(&mut app, &[KeyCode::PageDown]);
    assert_eq!(app.request_scroll, 10.min(last));
    press(&mut app, &[KeyCode::PageUp]);
    assert_eq!(app.request_scroll, 0);
    press(&mut app, &[KeyCode::End]);
    assert_eq!(app.request_scroll, last);
    press(&mut app, &[KeyCode::Home]);
    assert_eq!(app.request_scroll, 0);

    press(&mut app, &[KeyCode::Char('+'), KeyCode::Char('=')]);
    assert_eq!(app.charts.zoom(Chart::Prompt), 4);
    press(&mut app, &[KeyCode::Char('-')]);
    assert_eq!(app.charts.zoom(Chart::Prompt), 2);
    press(&mut app, &[KeyCode::Char('0')]);
    assert_eq!(app.charts.zoom(Chart::Prompt), 1);

    press(&mut app, &[KeyCode::Enter]);
    assert!(app.charts.expanded);
    let mut visited = vec![app.charts.focused];
    for _ in 0..12 {
        press(&mut app, &[KeyCode::Right]);
        let screen = render_app(&app, 160, 48);
        assert!(!screen.is_empty());
        visited.push(app.charts.focused);
    }
    for chart in [
        Chart::Generation,
        Chart::Prefill,
        Chart::Cache,
        Chart::Memory,
        Chart::Paging,
        Chart::Queue,
    ] {
        assert!(visited.contains(&chart), "{chart:?} is reachable");
    }
    press(&mut app, &[KeyCode::Up]);
    press(&mut app, &[KeyCode::Esc]);
    assert!(!app.charts.expanded && !app.quit);
    press(&mut app, &[KeyCode::Char('[')]);
    assert_eq!(app.gpu_selected, 0);
    press(&mut app, &[KeyCode::Char('q')]);
    assert!(app.quit, "other keys fall through to global handling");
}

#[test]
fn expanded_charts_render_each_metric_with_its_own_title() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    app.charts.expanded = true;
    for (chart, text) in [
        (Chart::Prompt, "PROMPT"),
        (Chart::Generation, "GENERATION"),
        (Chart::Prefill, "PREFILL"),
        (Chart::Cache, "CACHE"),
        (Chart::Gpu, "GPU"),
        (Chart::Memory, "MEMORY"),
        (Chart::Paging, "PAGING"),
        (Chart::Queue, "QUEUE"),
        (Chart::Latency, "FIRST TOKEN"),
    ] {
        app.charts.focused = chart;
        let screen = render_app(&app, 140, 40).to_uppercase();
        assert!(screen.contains(text), "{chart:?} shows {text}");
    }
}

#[test]
fn mouse_focus_expand_and_zoom_follow_the_chart_under_the_pointer() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    render_app(&app, 160, 48);
    let (chart, area) = app
        .charts
        .regions
        .borrow()
        .iter()
        .copied()
        .find(|(chart, _)| *chart != Chart::Prompt)
        .unwrap();
    let mouse = |kind| MouseEvent {
        kind,
        column: area.x + 1,
        row: area.y + 1,
        modifiers: KeyModifiers::NONE,
    };
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right)));
    assert_eq!(app.charts.focused, chart);
    assert!(app.charts.expanded);
    app.handle_mouse(mouse(MouseEventKind::ScrollUp));
    assert_eq!(app.charts.zoom(chart), 2);
    app.handle_mouse(mouse(MouseEventKind::ScrollDown));
    assert_eq!(app.charts.zoom(chart), 1);
    app.handle_mouse(mouse(MouseEventKind::Moved));
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right)));
    assert!(!app.charts.expanded);
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(
        app.charts.focused, chart,
        "clicks outside charts are ignored"
    );
    app.tab = 1;
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right)));
    assert!(!app.charts.expanded, "only the overview has charts");
}

#[test]
fn help_and_controls_describe_the_active_view() {
    for (tab, text) in [
        (0, "PgUp / PgDn     move by ten requests"),
        (1, "s               cycle RSS / CPU / PID / name sort"),
        (2, "f / [ / ]       cycle event filters"),
    ] {
        let mut app = test_app(tab);
        app.help = true;
        let screen = render_app(&app, 140, 40);
        assert!(screen.contains("HELP"));
        assert!(screen.contains(text), "tab {tab}: {text}");
    }
    let journal = render_app(&test_app(2), 140, 40);
    assert!(journal.contains("filter"));
    assert!(journal.contains("Journal"));
    let mut small = test_app(0);
    small.help = true;
    assert!(render_app(&small, 80, 24).contains("HELP"));
}

#[test]
fn tiny_terminals_get_a_resize_message_and_narrow_alerts_are_skipped() {
    let screen = render_app(&test_app(0), 60, 20);
    assert!(screen.contains("This dashboard needs at least 72×24 terminal cells."));
    assert!(screen.contains("Current size: 60×20"));
    let mut app = test_app(0);
    app.alert = Some(ActiveAlert {
        state: "MEMORY BOTTLENECK".into(),
        summary: "critical".into(),
        time: "now".into(),
    });
    let narrow = render_view(19, 5, |frame| app.draw_alert_banner(frame, frame.area()));
    assert!(!narrow.contains("MEMORY BOTTLENECK"));
    let flat = render_view(40, 1, |frame| {
        app.draw_alert_banner(frame, Rect::new(0, 0, 40, 0))
    });
    assert!(!flat.contains("MEMORY"));
    app.alert = None;
    let none = render_view(40, 3, |frame| app.draw_alert_banner(frame, frame.area()));
    assert!(!none.contains('⚠'));
}

// --- labels, classification and correlation --------------------------------

#[test]
fn enum_labels_and_tones_are_stable() {
    assert_eq!(TelemetrySource::Report.label(), "reported usage");
    assert_eq!(TopSort::Name.next(), TopSort::Rss);
    for (cause, label, tone) in [
        (CorrelationCause::None, "no correlated cause", Tone::Muted),
        (CorrelationCause::Paging, "paging", Tone::Red),
        (
            CorrelationCause::Compression,
            "compression churn",
            Tone::Yellow,
        ),
        (
            CorrelationCause::MemoryPressure,
            "memory pressure",
            Tone::Red,
        ),
        (CorrelationCause::Thermal, "thermal limiting", Tone::Red),
        (
            CorrelationCause::MetalMemory,
            "Metal memory pressure",
            Tone::Yellow,
        ),
        (
            CorrelationCause::GpuSaturation,
            "GPU saturation",
            Tone::Yellow,
        ),
        (CorrelationCause::Queueing, "queueing", Tone::Yellow),
        (
            CorrelationCause::ContextGrowth,
            "context/KV growth",
            Tone::Yellow,
        ),
        (
            CorrelationCause::ModelMemory,
            "model memory growth",
            Tone::Yellow,
        ),
        (
            CorrelationCause::Runtime,
            "workload/runtime change",
            Tone::Yellow,
        ),
    ] {
        assert_eq!(cause.label(), label);
        assert_eq!(cause.tone(), tone);
    }
    let insight = |direction, cause, confidence| CorrelationInsight {
        direction,
        cause,
        confidence,
        ..CorrelationInsight::default()
    };
    assert_eq!(
        insight(ThroughputDirection::Up, CorrelationCause::None, 0).tone(),
        Tone::Green
    );
    assert_eq!(
        insight(ThroughputDirection::Flat, CorrelationCause::Paging, 0).tone(),
        Tone::Red
    );
    assert_eq!(
        insight(ThroughputDirection::Down, CorrelationCause::Queueing, 0).tone(),
        Tone::Yellow
    );
    for (confidence, label) in [
        (90, "high"),
        (60, "medium"),
        (10, "low"),
        (0, "unavailable"),
    ] {
        assert_eq!(
            insight(
                ThroughputDirection::Down,
                CorrelationCause::Runtime,
                confidence
            )
            .confidence_label(),
            label
        );
    }
    for (filter, kind) in [
        (JournalFilter::All, EventKind::Queue),
        (JournalFilter::Llm, EventKind::Queue),
        (JournalFilter::Pressure, EventKind::Pressure),
        (JournalFilter::Gpu, EventKind::Gpu),
        (JournalFilter::Thermal, EventKind::Thermal),
        (JournalFilter::System, EventKind::System),
    ] {
        assert!(filter.matches(kind));
    }
    assert!(!JournalFilter::System.matches(EventKind::Llm));
    for status in ["error", "unknown-state"] {
        assert_ne!(llm_status_tone(status), Tone::Green);
    }
    assert_eq!(llm_status_tone("error"), Tone::Red);
}

fn measured(pressure: &str) -> Sample {
    Sample {
        total_memory: 16 * 1024 * MIB,
        availability: Some(50),
        pressure: pressure.into(),
        vm_available: true,
        swap_available: true,
        rate_ready: true,
        thermal: "no warning".into(),
        largest_consumer: Some("Xcode".into()),
        ..Sample::default()
    }
}

fn classified(mut sample: Sample) -> Sample {
    classify(&mut sample, None, Thresholds::default());
    sample
}

#[test]
fn classification_ranks_memory_paging_and_thermal_conditions() {
    let critical = classified(measured("RED"));
    assert_eq!(
        (
            critical.impact.as_str(),
            critical.grade.as_str(),
            critical.health
        ),
        ("MEMORY BOTTLENECK", "CRITICAL", Some(10))
    );
    assert_eq!(critical.guidance_badge, "ACT NOW");
    assert!(critical
        .guidance_action
        .starts_with("Pause or quit the largest app"));
    assert!(signal_summary(&critical).starts_with("native pressure critical"));
    let critical_llm = classified(Sample {
        llm_count: 1,
        ..measured("RED")
    });
    assert!(critical_llm
        .guidance_action
        .starts_with("Stop unused models/requests"));

    let thrash = classified(Sample {
        swap_in: 32 * MIB,
        swap_out: 32 * MIB,
        ..measured("GREEN")
    });
    assert_eq!(thrash.impact, "SWAP THRASHING");
    assert!(thrash
        .guidance_cause
        .starts_with("Swap thrash — 32.0 MiB/s in"));
    assert_eq!(signal_summary(&thrash), "in 32.0 MiB/s / out 32.0 MiB/s");

    let heavy = classified(Sample {
        swap_out: 40 * MIB,
        ..measured("GREEN")
    });
    assert_eq!(heavy.impact, "HEAVY PAGING");
    assert!(heavy.guidance_cause.starts_with("RAM overflow"));
    assert!(heavy.guidance_action.contains("Pause or quit Xcode"));
    assert!(signal_summary(&heavy).starts_with("swap 40.0 MiB/s / growth"));
    let heavy_llm = classified(Sample {
        swap_out: 40 * MIB,
        llm_count: 1,
        ..measured("GREEN")
    });
    assert!(heavy_llm.guidance_action.starts_with("Stop unused models"));

    let recovery = classified(Sample {
        swap_in: 40 * MIB,
        swap_growth: -1,
        ..measured("GREEN")
    });
    assert_eq!(recovery.impact, "PAGE-IN RECOVERY");
    assert_eq!(recovery.grade, "RECOVERING");
    assert!(signal_summary(&recovery).starts_with("page-in 40.0 MiB/s"));

    let stress = classified(measured("YELLOW"));
    assert_eq!(stress.impact, "MEMORY STRESS");
    assert_eq!(stress.guidance_badge, "REDUCE");
    assert!(stress.guidance_action.contains("start with Xcode"));
    let stress_llm = classified(Sample {
        llm_count: 1,
        ..measured("YELLOW")
    });
    assert!(stress_llm
        .guidance_action
        .starts_with("Reduce model/context/cache"));

    for thermal in ["limited 70%", "warning reported"] {
        let hot = classified(Sample {
            thermal: thermal.into(),
            ..measured("GREEN")
        });
        assert_eq!(hot.impact, "THERMAL LIMIT");
        assert_eq!(hot.guidance_badge, "COOL");
    }

    let idle = classified(measured("GREEN"));
    assert_eq!((idle.impact.as_str(), idle.health), ("IDLE", Some(100)));
    assert_eq!(idle.guidance_badge, "OK");
    assert_eq!(signal_summary(&idle), "none");

    let limited = classified(Sample {
        availability: None,
        ..measured("GREEN")
    });
    assert_eq!(limited.impact, "DATA LIMITED");
    assert_eq!(limited.guidance_badge, "CHECK");
}

#[test]
fn gpu_and_llm_summaries_prefer_correlation_and_rates() {
    let mut busy = Sample {
        impact: "GPU BUSY".into(),
        gpu_util: Some(91),
        ..Sample::default()
    };
    assert!(signal_summary(&busy).starts_with("GPU 91% busy · "));
    busy.correlation.summary = "GEN ↓20% · GPU".into();
    assert_eq!(signal_summary(&busy), "GEN ↓20% · GPU");
    let ready = Sample {
        impact: "LLM READY".into(),
        llm_count: 2,
        ..Sample::default()
    };
    assert!(signal_summary(&ready).starts_with("2 process(es) · "));
}

#[test]
fn correlation_evidence_labels_name_the_measured_signal() {
    let with = |cause, sample: Sample| {
        correlation_evidence_label(&Sample {
            correlation: CorrelationInsight {
                cause,
                ..CorrelationInsight::default()
            },
            ..sample
        })
    };
    let base = Sample::default;
    assert_eq!(
        with(
            CorrelationCause::Paging,
            Sample {
                swap_in: 1024,
                ..base()
            }
        ),
        "I/O 1.0 KiB/s"
    );
    assert_eq!(with(CorrelationCause::Paging, base()), "");
    assert_eq!(
        with(
            CorrelationCause::Compression,
            Sample {
                compress: 2048,
                ..base()
            }
        ),
        "compress 2.0 KiB/s"
    );
    assert_eq!(with(CorrelationCause::Compression, base()), "");
    assert!(with(CorrelationCause::MemoryPressure, base()).starts_with("pressure "));
    assert_eq!(with(CorrelationCause::Thermal, base()), "thermal limit");
    assert_eq!(
        with(
            CorrelationCause::MetalMemory,
            Sample {
                gpu_in_use: Some(9),
                gpu_alloc: Some(10),
                ..base()
            }
        ),
        "Metal mem 90%"
    );
    assert_eq!(with(CorrelationCause::MetalMemory, base()), "Metal mem");
    let mut gpu = base();
    gpu.metal.tiler_util = Some(97);
    gpu.gpu_util = Some(50);
    assert_eq!(with(CorrelationCause::GpuSaturation, gpu), "GPU 97%");
    assert_eq!(with(CorrelationCause::GpuSaturation, base()), "GPU");
    assert_eq!(
        with(
            CorrelationCause::Queueing,
            Sample {
                llm_waiting_requests: Some(3),
                ..base()
            }
        ),
        "queue 3 waiting"
    );
    assert_eq!(with(CorrelationCause::Queueing, base()), "queueing");
    assert_eq!(
        with(
            CorrelationCause::ContextGrowth,
            Sample {
                llm_prompt_tokens: Some(2_500_000),
                ..base()
            }
        ),
        "context 2.5M"
    );
    assert_eq!(with(CorrelationCause::ContextGrowth, base()), "context/KV");
    assert_eq!(
        with(
            CorrelationCause::ModelMemory,
            Sample {
                llm_model_memory: Some(3),
                llm_model_memory_max: Some(4),
                ..base()
            }
        ),
        "model mem 75%"
    );
    assert_eq!(with(CorrelationCause::ModelMemory, base()), "model memory");
    assert_eq!(with(CorrelationCause::Runtime, base()), "workload/runtime");
    assert_eq!(with(CorrelationCause::None, base()), "");
}

fn live(tps: f64) -> Sample {
    Sample {
        llm_provider: "oMLX".into(),
        llm_model: "m".into(),
        llm_generation_tps: Some(tps),
        llm_generation_tps_live: true,
        pressure: "GREEN".into(),
        thermal: "no warning".into(),
        ..Sample::default()
    }
}

#[test]
fn correlation_engine_explains_throughput_changes_against_a_median_baseline() {
    let thresholds = Thresholds::default();
    let mut engine = CorrelationEngine::default();
    for tps in [50.0, 52.0, 48.0, 50.0] {
        engine.observe(&live(tps), thresholds);
    }
    assert_eq!(
        engine.baseline("oMLX", "m"),
        Some(50.0),
        "even-length median"
    );
    let up = engine.observe(&live(70.0), thresholds);
    assert_eq!(up.direction, ThroughputDirection::Up);
    assert_eq!(
        up.summary, "",
        "a rise without a factor needs no explanation"
    );
    let mut busy = live(70.0);
    busy.gpu_util = Some(85);
    let up = engine.observe(&busy, thresholds);
    assert!(up.summary.starts_with("GEN ↑"), "{}", up.summary);
    assert!(up.summary.ends_with("correlated: GPU 85% busy"));
    let flat = engine.observe(&live(50.5), thresholds);
    assert_eq!(flat.direction, ThroughputDirection::Flat);

    let mut queued = live(30.0);
    queued.llm_waiting_requests = Some(1);
    queued.llm_active_requests = Some(1);
    let insight = engine.observe(&queued, thresholds);
    assert_eq!(insight.direction, ThroughputDirection::Down);
    assert_eq!(insight.cause, CorrelationCause::Queueing);
    assert_eq!(insight.confidence, 70);

    let mut hot = live(30.0);
    hot.thermal = "limited 60%".into();
    hot.pressure = "RED".into();
    let insight = engine.observe(&hot, thresholds);
    assert_eq!(insight.cause, CorrelationCause::MemoryPressure);
    assert_eq!(insight.confidence, 100);

    let mut gpu = live(30.0);
    gpu.gpu_util = Some(85);
    let insight = engine.observe(&gpu, thresholds);
    assert_eq!(insight.cause, CorrelationCause::GpuSaturation);
    assert_eq!(insight.confidence, 58);

    let mut model = live(30.0);
    model.llm_model_memory = Some(95);
    model.llm_model_memory_max = Some(100);
    let insight = engine.observe(&model, thresholds);
    assert_eq!(insight.cause, CorrelationCause::ModelMemory);
    assert!(insight.details.contains("model MEM 95% of ceiling"));

    let mut grow = live(30.0);
    grow.llm_model_memory = Some(1);
    engine.observe(&grow, thresholds);
    grow.llm_model_memory = Some(1 + MODEL_MEMORY_GROWTH);
    let insight = engine.observe(&grow, thresholds);
    assert_eq!(insight.cause, CorrelationCause::ModelMemory);
    assert!(insight.details.contains("model MEM +256.0 MiB"));

    let mut long = live(30.0);
    long.llm_prompt_tokens = Some(20_000);
    let insight = engine.observe(&long, thresholds);
    assert_eq!(insight.cause, CorrelationCause::ContextGrowth);
    assert_eq!(insight.confidence, 48);
    assert!(insight.details.contains("context 20.0k"));

    let mut slow = live(30.0);
    slow.metal.renderer_util = None;
    let insight = engine.observe(&slow, thresholds);
    assert_eq!(insight.cause, CorrelationCause::Runtime);

    for _ in 0..CORRELATION_HISTORY_LIMIT + 4 {
        engine.observe(&live(40.0), thresholds);
    }
    assert_eq!(engine.observations.len(), CORRELATION_HISTORY_LIMIT);
}

#[test]
fn command_runner_times_out_and_reports_spawn_failures() {
    let started = Instant::now();
    assert_eq!(command_text("sleep", &["5"]), None);
    let elapsed = started.elapsed();
    assert!(elapsed >= COMMAND_TIMEOUT && elapsed < Duration::from_secs(4));
    assert_eq!(command_text("/nonexistent/mlxtop-command", &[]), None);
}

#[test]
fn small_formatters_cover_every_unit() {
    assert_eq!(compact_tokens(2_500_000), "2.5M");
    assert_eq!(parse_unit("2T"), 2 * 1024_u64.pow(4));
    assert_eq!(parse_unit("7"), 7);
    assert_eq!(optional_bytes(None), "—");
    assert_eq!(optional_bytes(Some(2048)), "2.0 KiB");
    assert_eq!(signed_rate(1024), "+1.0 KiB/s");
    assert_eq!(signed_rate(-1024), "-1.0 KiB/s");
    assert_eq!(signed_rate(0), "0 B/s");
    assert_eq!(
        signed_rate_bytes(u64::MAX, 0, Duration::from_nanos(1)),
        i64::MAX
    );
    assert_eq!(
        signed_rate_bytes(0, u64::MAX, Duration::from_nanos(1)),
        i64::MIN
    );
    assert_eq!(parse_thermal("CPU_Speed_Limit = 80"), "limited 80%");
    assert_eq!(parse_thermal("CPU_Speed_Limit = 100"), "no limit");
    assert_eq!(
        parse_thermal("Thermal warning level set to 2"),
        "warning reported"
    );
    assert_eq!(find_number("Limit = 42", "Limit"), Some(42));
    assert_eq!(find_number("Limit 42", "Limit"), None);
    let sample = Sample {
        vm_available: true,
        compressor: 1024,
        ..Sample::default()
    };
    assert_eq!(compressed_memory_label(&sample), "1.0 KiB");
    let mut history = VecDeque::new();
    for value in 0..5 {
        push_history_with_tone(&mut history, Some(value), Tone::Green, 3);
    }
    assert_eq!(
        history.iter().map(|point| point.value).collect::<Vec<_>>(),
        [Some(2), Some(3), Some(4)]
    );
    assert!(
        merge_mlx_telemetry(&MlxTelemetry::default(), &MlxTelemetry::default())
            .version
            .is_none()
    );
    assert!(mlx_metadata_is_empty(&MlxTelemetry::default()));
    assert!(!mlx_metadata_is_empty(&MlxTelemetry {
        process_footprint: Some(1),
        ..MlxTelemetry::default()
    }));
}

#[test]
fn doctor_arguments_configuration_and_exit_conditions_are_explicit() {
    use crate::config::load_doctor_config;
    use crate::report::{diagnostic_lines, write_doctor};
    assert!(matches!(
        parse_args(&args(&["doctor", "-i", "2"]), &Config::default()).unwrap(),
        CliAction::Doctor { interval: 2, .. }
    ));
    assert!(parse_args(&args(&["doctor", "--once"]), &Config::default()).is_err());
    assert!(matches!(
        parse_args(&args(&["doctor", "--help"]), &Config::default()).unwrap(),
        CliAction::Print(_)
    ));
    let dir = TempDir::new("doctor-config");
    assert_eq!(
        load_doctor_config(&dir.0.join("missing")).unwrap(),
        Config::default()
    );
    for text in [
        "invalid",
        r#"{"interval":0}"#,
        r#"{"history":9999}"#,
        r#"{"omx":{"port":0}}"#,
    ] {
        let path = dir.write("config.json", text);
        assert!(load_doctor_config(&path).is_err());
    }
    assert!(load_doctor_config(&dir.0).is_err());
    assert_eq!(
        load_doctor_config(&dir.write("config.json", r#"{"interval":2}"#))
            .unwrap()
            .interval,
        Some(2)
    );
    let mut sample = Sample {
        total_memory: 1024,
        vm_available: true,
        resident_memory: Some(512),
        pressure: "RED".into(),
        rate_ready: true,
        ..Sample::default()
    };
    let mut text = Vec::new();
    assert!(
        write_doctor(&mut text, &sample).unwrap(),
        "pressure is a measurement, not a doctor failure"
    );
    let text = String::from_utf8(text).unwrap();
    for line in diagnostic_lines(&sample) {
        assert!(text.contains(&line));
    }
    sample.runtime.provider = Some("Ollama".into());
    sample.runtime.begin();
    sample.runtime.finish(false);
    assert!(!write_doctor(&mut Vec::new(), &sample).unwrap());
    sample.runtime.finish(true);
    assert!(write_doctor(&mut Vec::new(), &sample).unwrap());
    sample.vm_available = false;
    assert!(!write_doctor(&mut Vec::new(), &sample).unwrap());
}
