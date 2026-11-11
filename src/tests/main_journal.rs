// SPDX-License-Identifier: MIT
//! The Journal and alarms: what is written, and when.
use super::*;

#[test]
fn critical_host_conditions_drive_alerts_but_gpu_usage_does_not() {
    assert!(is_critical_state("SWAP THRASHING"));
    assert!(is_critical_state("HEAVY PAGING"));
    assert!(is_critical_state("PAGE-IN RECOVERY"));
    assert!(!is_critical_state("PAGING ACTIVE"));
    assert!(!is_critical_state("WATCH PAGING"));
    assert!(is_critical_state("MEMORY BOTTLENECK"));
    assert!(!is_critical_state("GPU BUSY"));
    assert!(!is_critical_state("THERMAL LIMIT"));
    assert!(!is_critical_state("LLM READY"));
    assert!(!is_critical_state(""));
}

#[test]
fn alarms_off_never_rings_or_raises_the_banner() {
    let (mut app, views) = test_app_with_sender(0);
    app.alarms = false;
    let mut critical = view_with_impact("HEAVY PAGING", "12:00:00");
    critical.current.pressure = "RED".into();
    views.send(critical).unwrap();
    app.tick();
    assert_eq!(app.alert_bells, 0);
    assert!(app.alert.is_none());
    assert_eq!(
        app.collector.current.impact, "HEAVY PAGING",
        "the reading itself still arrives"
    );
}

#[test]
fn critical_memory_alarm_survives_missing_counters_and_acknowledgment() {
    let (mut app, views) = test_app_with_sender(0);
    for utilization in [80, 99, 100] {
        let mut view = view_with_impact("GPU BUSY", "12:00:00");
        view.current.gpu_util = Some(utilization);
        views.send(view).unwrap();
        app.tick();
    }
    assert_eq!(app.alert_bells, 0);
    let mut critical = view_with_impact("DATA LIMITED", "12:00:01");
    critical.current.pressure = "RED".into();
    views.send(critical.clone()).unwrap();
    app.tick();
    assert_eq!(app.alert_bells, 1);
    assert_eq!(app.alert.as_ref().unwrap().state, "MEMORY BOTTLENECK");
    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    views
        .send(view_with_impact("DATA LIMITED", "12:00:02"))
        .unwrap();
    views.send(critical.clone()).unwrap();
    app.tick();
    assert_eq!(app.alert_bells, 1);
    assert!(app.alert.is_none());
    views
        .send(view_with_impact("GPU BUSY", "12:00:03"))
        .unwrap();
    views.send(critical).unwrap();
    app.tick();
    assert_eq!(app.alert_bells, 2);
}

#[test]
fn paging_alert_banner_shows_until_acknowledged_or_reset() {
    let (mut app, views) = test_app_with_sender(0);
    app.collector.current.impact = "LLM READY".into();
    views
        .send(view_with_impact("HEAVY PAGING", "00:52:54"))
        .expect("test channel should accept views");
    app.tick();
    assert!(app.alert.is_some());

    let rendered = render_app(&app, 180, 50);
    assert!(rendered.contains("⚠ HEAVY PAGING"));
    assert!(rendered.contains("a acknowledge"));

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(app.alert.is_none());
    let rendered = render_app(&app, 180, 50);
    assert!(!rendered.contains("⚠ HEAVY PAGING"));

    // A fresh episode after recovery raises again; reset retires it.
    views
        .send(view_with_impact("LLM READY", "00:52:55"))
        .expect("test channel should accept views");
    app.tick();
    views
        .send(view_with_impact("SWAP THRASHING", "00:52:56"))
        .expect("test channel should accept views");
    app.tick();
    assert_eq!(app.alert_bells, 2);
    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    assert!(app.alert.is_none());
    let rendered = render_app(&app, 180, 50);
    assert!(!rendered.contains("⚠ SWAP THRASHING"));
}

#[test]
fn paging_alert_survives_a_quiet_sample_while_paging_persists() {
    // A momentary 0 B/s sample inside an aggressive episode is normal
    // churn noise; the episode ends only when the classifier leaves the
    // aggressive states, so the alert must not flap on a single dip.
    let (mut app, views) = test_app_with_sender(0);
    app.collector.current.impact = "WATCH PAGING".into();
    views
        .send(view_with_impact("PAGE-IN RECOVERY", "00:52:54"))
        .expect("test channel should accept views");
    app.tick();
    assert!(app.alert.is_some());
    assert_eq!(app.alert_bells, 1);
}

#[test]
fn prompt_counts_are_visible_and_request_events_use_the_llm_filter() {
    let mut app = test_app(0);
    app.collector.current.llm_prompt_tokens = Some(32768);
    for (width, height) in [(80, 24), (100, 40), (180, 50)] {
        let rendered = render_app(&app, width, height);
        assert!(
            rendered.contains("PROMPT 32.8k"),
            "prompt missing at {width}x{height}"
        );
    }
    assert!(JournalFilter::Llm.matches(EventKind::from_state("PROMPT")));
    let reported = Sample {
        llm_source: TelemetrySource::Report,
        llm_generation_tps: Some(30.0),
        ..Sample::default()
    };
    assert!(llm_generation_rate_label(&reported).starts_with("LAST GEN"));
    assert_eq!(chart_rate_value(&reported, ChartMetric::Generation), None);
}

#[test]
fn overview_journal_retains_three_recent_events_at_screenshot_size() {
    let mut app = test_app(0);
    for index in 0..12 {
        app.collector.signals.push_back(SignalEvent {
            time: format!("12:00:{index:02}"),
            recorded_at: SystemTime::now(),
            kind: EventKind::Llm,
            state: "PROMPT".into(),
            summary: format!("Event {index:02} · request observed"),
            tone: Tone::Cyan,
        });
    }
    let screen = render_app(&app, 170, 42);
    for index in 7..12 {
        assert!(screen.contains(&format!("Event {index:02}")), "{screen}");
    }
    assert!(!screen.contains("Event 06"), "capacity is five event rows");
    assert!(screen.find("Event 11") < screen.find("Event 09"));
    assert!(screen.contains("3 full journal"));
    let compact = render_app(&app, 80, 24);
    assert!(
        !compact.contains("Event 11"),
        "compact Overview uses the journal preview space for the assessment"
    );
    assert!(compact.contains("? help q quit"));
}

#[test]
fn recent_journal_wraps_messages_without_overwriting_the_next_event() {
    let mut app = test_app(0);
    let long_summary = "Process footprint increased while a request was running; memory pressure stayed normal and throughput recovered after the request completed. ".repeat(3);
    for (time, summary) in [
        ("12:00:00", "older event".into()),
        ("12:00:01", long_summary),
    ] {
        app.collector.signals.push_back(SignalEvent {
            time: time.into(),
            recorded_at: SystemTime::now(),
            kind: EventKind::Llm,
            state: "LLM READY".into(),
            summary,
            tone: Tone::Green,
        });
    }
    let screen = render_view(80, 6, |frame| app.draw_signal_log(frame, frame.area()));
    let lines: Vec<_> = screen.lines().collect();
    assert!(lines[1].contains("12:00:01"));
    assert!(lines[1].contains("Process footprint"));
    assert!(
        lines[2].contains("…"),
        "long messages are explicitly shortened"
    );
    assert!(
        !lines[2].contains("LLM READY"),
        "continuation keeps the timestamp/state columns clear"
    );
    assert!(lines[3].contains("12:00:00"));
    assert!(lines[3].contains("older event"));
    assert!(lines[5].contains(" 2 events "));
}

#[test]
fn recent_journal_keeps_two_event_rows_after_header_compaction() {
    let mut app = test_app(0);
    app.collector.signals = VecDeque::from([
        SignalEvent {
            time: "12:00:00".into(),
            recorded_at: SystemTime::now(),
            kind: EventKind::Llm,
            state: "LLM".into(),
            summary: "request started".into(),
            tone: Tone::Cyan,
        },
        SignalEvent {
            time: "12:00:01".into(),
            recorded_at: SystemTime::now(),
            kind: EventKind::Gpu,
            state: "GPU".into(),
            summary: "compute burst started".into(),
            tone: Tone::Yellow,
        },
    ]);
    let rendered = render_app(&app, 180, 50);
    assert!(rendered.contains("request started"));
    assert!(rendered.contains("compute burst started"));
    assert!(!rendered.contains("Live · sampling system counters"));
}

#[test]
fn journal_filters_use_structured_event_kinds() {
    assert!(JournalFilter::Llm.matches(EventKind::Llm));
    assert!(JournalFilter::Llm.matches(EventKind::Queue));
    assert!(!JournalFilter::Llm.matches(EventKind::Gpu));
    assert!(JournalFilter::Paging.matches(EventKind::Paging));
}

#[test]
fn overview_review_fixes_keep_one_reading_per_fact_and_identity_during_alarms() {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);

    // The live request speed equals the generation headline, so it appears once;
    // a different per-request speed (concurrency) is still shown.
    let screen = render_app(&app, 120, 40);
    assert_eq!(screen.matches("LIVE 35.5 tok/s").count(), 1, "{screen}");
    assert!(screen.contains("Healthy · no bottleneck"), "{screen}");
    assert!(screen.contains("PRESSURE normal"));
    app.collector.current.llm_generation_tps = Some(50.0);
    assert!(render_app(&app, 120, 40).contains("OUT 1,200 · LIVE 35.5 tok/s"));
    app.collector.current.llm_generation_tps = Some(35.5);

    // Full view names fit at the minimum width; narrow charts keep units.
    let narrow = render_app(&app, 80, 24);
    for label in [
        "1 Overview",
        "3 Journal",
        "35.5 tok/s",
        "decoding",
        "17.2 / 36.0 GiB",
    ] {
        assert!(narrow.contains(label), "missing {label}\n{narrow}");
    }
    assert!(!narrow.contains("int 64%"));

    // Tall terminals give surplus rows to requests, not throughput.
    render_app(&app, 220, 56);
    let regions = app.charts.regions.borrow().clone();
    let height = |chart| {
        regions
            .iter()
            .find(|(id, _)| *id == chart)
            .unwrap()
            .1
            .height
    };
    // Tall terminals spend extra rows on host resources, not sparse panels.
    assert!(height(Chart::Generation) <= 10);
    assert!(height(Chart::Prompt) <= 14);
    assert!(height(Chart::Memory) > height(Chart::Prompt));

    // An alarm replaces the assessment rows, never SYSINFO's identity and age.
    app.alert = Some(ActiveAlert {
        state: "MEMORY BOTTLENECK".into(),
        summary: "Critical memory pressure".into(),
        time: "16:30:52".into(),
    });
    let alarm = render_app(&app, 120, 40);
    for label in [
        "Qwen3.8-27B-oQ4e-mtp",
        "Qwen3.8-27B-oQ4e-mtp",
        "LIVE",
        "⚠ MEMORY BOTTLENECK",
        "a acknowledge",
    ] {
        assert!(alarm.contains(label), "missing {label}\n{alarm}");
    }
    assert!(!alarm.contains("Healthy"));
    app.diagnostics_open = true;
    assert!(render_app(&app, 120, 40).contains("a acknowledge"));
    app.diagnostics_open = false;
    app.tab = 1;
    assert!(render_app(&app, 120, 40).contains("⚠ MEMORY BOTTLENECK"));
    app.tab = 0;
    app.alert = None;

    // Compact paging uses the assessment's warning threshold for "active".
    for (rate, label) in [
        (4_096, "Light paging · below warning"),
        (2 * 1024 * 1024, "Watch paging"),
        (4 * 1024 * 1024, "Paging active"),
    ] {
        let mut history = VecDeque::new();
        push_history_with_tone(&mut history, Some(rate), Tone::Green, 10);
        let screen = render_view(40, 5, |frame| {
            app.render_indicator_chart(frame, frame.area(), "paging", &history, ChartMetric::Swap)
        });
        assert!(screen.contains(label), "missing {label}\n{screen}");
    }
}

#[test]
fn the_journal_has_one_row_as_a_request_starts_and_one_as_it_ends() {
    let mut collector = Collector::with_host(
        60,
        offline_config(),
        Box::new(FakeHost::default()),
        Platform::MacOs,
        None,
    );
    let idle = Sample {
        impact: "LLM READY".into(),
        llm_status: "idle".into(),
        llm_generation_tps: Some(55.1),
        llm_provider: "oMLX".into(),
        llm_model: "Qwen3.8-27B-oQ4e-mtp".into(),
        llm_source: TelemetrySource::Live,
        gpu_util: Some(2),
        llm_active_requests: Some(0),
        ..Sample::default()
    };
    let mut prefill = decoding(1, 0.0, 0, 0.0, 99);
    prefill.impact = "GPU BUSY".into();
    prefill.llm_requests[0].output = None;
    prefill.llm_requests[0].prefilled = Some(67_776);
    let mut generating = decoding(2, 45.0, 1_200, 0.05, 99);
    generating.impact = "GPU BUSY".into();
    let mut second = generating.clone();
    second.llm_requests[0].id = "f00baa".into();
    second.llm_requests[0].output = None;
    second.llm_requests[0].prefilled = Some(0);
    let mut queued = second.clone();
    queued.llm_requests[0].id = "c0ffee".into();
    queued.llm_requests[0].queued = true;
    let samples = [
        idle.clone(),
        prefill,
        generating,
        second,
        idle.clone(),
        queued,
        idle,
    ];
    collector.gpu_band = None;
    for pair in samples.windows(2) {
        collector.record_journal_events(Some(&pair[0]), &pair[1]);
    }
    let rows: Vec<String> = collector
        .signals
        .iter()
        .map(|event| format!("{} {}", event.state, event.summary))
        .collect();
    assert_eq!(
        rows,
        [
            "PROMPT e28cde · 67.8k prompt · 99% cached",
            "PROMPT f00baa · 67.8k prompt · 99% cached",
            "DONE e28cde · 1.2k out",
            "DONE f00baa · ended while prefilling",
            "PROMPT c0ffee · 67.8k prompt · 99% cached · queued",
        ],
        "{rows:#?}"
    );
    assert_eq!(EventKind::from_state("DONE"), EventKind::Llm);

    // Output already reported, a rate and a first-token time end the row.
    let finished = domain::RequestUsage {
        provider: "llama.cpp".into(),
        model: "m".into(),
        id: "r1".into(),
        prompt: 800,
        cached: Some(200),
        output: Some(64),
        completed: true,
        queued: false,
        prefilled: None,
        idle_for: None,
        ttft_ms: Some(310),
        output_tps: Some(41.2),
        observed_at: None,
    };
    assert_eq!(
        finished.journal_line(),
        "r1 · 800 prompt · 25% cached · 64 out · 41.2 tok/s · first token 310 ms"
    );
}
